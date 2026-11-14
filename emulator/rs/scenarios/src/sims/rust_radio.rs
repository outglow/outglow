//! The Rust firmware's radio driver against the vendor driver, in the
//! emulator (once emulator/rust_radio_check.py against the C firmware).
//!
//! Both images (the stock firmware with Telink's closed driver, and the Rust
//! firmware) join the same simulated network, each on its own air; then the
//! RF/DMA/clock registers and all analog registers are compared (differences
//! only where listed, each with its reason), the ACK turnaround is measured,
//! an energy scan is asked for, and the radio model's complaints are counted.
//!
//! Images: `--image stock=...` (default stock/stock-dimm.bin) and `--image
//! rust=...` (default target/tc32/bulb-tc32.bin).

use std::cell::RefCell;
use std::rc::Rc;

use super::{node_of, SimCtx, SimDef};
use crate::air::{NodeId, World};
use crate::coord::Coord;
use crate::frames::{mac_parse, Keys};
use crate::session::{Stop, R};

pub fn def() -> SimDef {
    SimDef { name: "rust_radio", doc: "the Rust radio driver leaves the radio as Telink's does; ACK turnaround; energy scan", run }
}

const CHANNEL: i64 = 15;
const CPI: i64 = 3;
/// zigbee_coord.Coordinator's default network key: bytes(range(1, 17))
const DEFAULT_KEY: [u8; 16] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];

/// Digital registers compared: (first, last) inclusive.
const DIGITAL: [(usize, usize); 7] =
    [(0x060, 0x066), (0x400, 0x4FF), (0xC0A, 0xC0B), (0xC20, 0xC21), (0xC40, 0xC48), (0xF00, 0xF1F), (0x1200, 0x12FF)];

/// The vendor's values of BLE registers are left in the 802.15.4 state:
/// the stock firmware runs Telink's concurrent BLE + Zigbee stack, whose BLE
/// events write these and whose return to 802.15.4 does not set them back
/// (the C firmware built on that SDK with BLE left the same values; built
/// without BLE it had the Rust driver's). The stock radio receives and sends
/// 802.15.4 with them as well.

fn expected(o: usize) -> Option<&'static str> {
    Some(match o {
        0xF00 => "last command to the link layer state machine: BLE in the Rust image stops it after every event (0x80), \
                  the vendor's BLE leaves its own last command",
        0xF1C => "radio interrupt mask: the Rust driver polls the end of a transmission, the vendor takes an interrupt",
        0x441 => "live RSSI reading",
        0x448..=0x44B => "live receiver status",
        0x40D => "BLE channel index (whitening seed); left by the vendor's BLE events",
        0x424..=0x427 => "BLE CRC init (0x555555 of the advertising channels); left by the vendor's BLE events",
        0x428 => "receive mode: the vendor's BLE writes 0x80/0x81 and its 802.15.4 receive keeps bits 5-6 clear; \
                  Telink's Zigbee driver and the Rust driver use 0xe0/0xe1",
        0xF04 | 0xF0C => "BLE TX settle time and BLE link layer setting; left by the vendor's BLE events",
        0xF16 => "bit 2, the scheduled start of a BLE event; left by the vendor's BLE events",
        0xF18..=0xF1B => "start tick of the last BLE event; left by the vendor's BLE events",
        0x124A | 0x124B => "BLE 1M modem parameters (rf_ble_1m_param_init); left by the vendor's BLE events",
        _ => return None,
    })
}

/// The analog registers the stock start-up sets apart from the Telink Zigbee
/// SDK start-up that the Rust code reproduces (the C firmware on that SDK had
/// the Rust values in all four).
const STOCK_START: &str = "set by the stock firmware's own start-up, not by the Telink Zigbee SDK's that the Rust start-up follows";

fn expected_analog(a: usize) -> Option<&'static str> {
    Some(match a {
        0xE7..=0xFF => "ADC and its random number source: the vendor seeds rand() from ADC noise, the Rust firmware does not use the ADC",
        0x0E..=0x15 => "GPIO pull resistors from the vendor's board file; not a radio setting",
        0xBD | 0xBF | 0xC0 | 0xC2 => "GPIO input/output defaults from the vendor's board file",
        0x3A | 0x3B => "retention registers: the Rust firmware marks 0x3a bit 0 \"has been running\" to tell a reset from a power-on",
        0x0C => "flash supply trim / 48 MHz setting: written 0xc6 by the Telink SDK's clock_init and by the Rust start-up, \
                 left at its reset value by the stock firmware",
        0x3C => "wake-up status: the Telink SDK's start-up (and the Rust one) clears bit 1 after a reset, the stock firmware does not",
        0x30 | 0xC6 => STOCK_START,
        _ => return None,
    })
}

fn emu(e: String) -> Stop {
    Stop::Error(e)
}

/// The radio's channel from the synthesizer registers (`Radio154.channel`), or None.
fn channel(w: &World) -> Option<i64> {
    let g = &w.air.dut.m.regs;
    let mhz = ((g[0x1245] & 0x3F) as i64) << 6 | (g[0x1244] >> 2) as i64;
    if mhz >= 2405 && (mhz - 2405) % 5 == 0 {
        Some((mhz - 2405) / 5 + 11)
    } else {
        None
    }
}

fn rx_on(w: &World) -> bool {
    let g = &w.air.dut.m.regs;
    g[0xF02] & 0x20 != 0 && g[0x428] & 0x01 != 0 && g[0xF02] & 0x10 == 0
}

fn chs(c: Option<i64>) -> String {
    c.map(|c| c.to_string()).unwrap_or_else(|| "None".into())
}

/// Boots an image next to a coordinator; the world once it has joined (and whether it did).
fn join(img: &crate::image::Image, max_ms: f64, ev: &Rc<RefCell<Vec<String>>>) -> R<(World, bool)> {
    let mut node = node_of(img, CPI)?;
    // the machine's own events, printed as the Python emulator prints them
    ev.borrow_mut().push(format!("[{:9.3} ms] reset #{}: boot slot 0x{:05x}", 0.0, node.m.reset_count, node.m.boot_slot));
    let e = ev.clone();
    node.m.event = Some(Rc::new(move |m: &mut tc32emu::Machine, text: &str| {
        e.borrow_mut().push(format!("[{:9.3} ms] {text}", m.ms()));
    }));
    let mut w = World::new(node, Coord::new_plain(CHANNEL, DEFAULT_KEY), Keys::default(), None);
    w.coord.start(&mut w.air);
    let joined = |w: &World| w.coord.has_event("device-announce");
    let end = w.now() + max_ms;
    while w.now() < end && !joined(&w) {
        let t = w.now() + 20.0;
        w.run_ms(t).map_err(emu)?;
    }
    let ok = joined(&w);
    if ok {
        // key exchange and a few exchanges with acknowledgements
        let short = w.coord.devices.iter().find(|d| d.announced).unwrap().short;
        let t = w.now() + 2500.0;
        w.run_ms(t).map_err(emu)?;
        for _ in 0..6 {
            w.coord.read_attrs(&mut w.air, short, 1, 0x0006, &[0x0000]);
            let t = w.now() + 150.0;
            w.run_ms(t).map_err(emu)?;
        }
    }
    Ok((w, ok))
}

/// Runs on until the bulb's radio is back in 802.15.4 receive on the
/// network's channel (both images lend the radio to BLE advertising events);
/// the ms it took, or None.
fn settle_rx(w: &mut World, max_ms: f64) -> R<Option<f64>> {
    let t0 = w.now();
    while w.now() < t0 + max_ms {
        if channel(w) == Some(CHANNEL) && rx_on(w) {
            // and still there a little later: not between two steps of a switch
            let t = w.now() + 0.2;
            w.run_ms(t).map_err(emu)?;
            if channel(w) == Some(CHANNEL) && rx_on(w) {
                return Ok(Some(w.now() - t0));
            }
        }
        let t = w.now() + 0.1;
        w.run_ms(t).map_err(emu)?;
    }
    Ok(None)
}

fn snapshot(w: &World) -> (Vec<(usize, u8)>, Vec<u8>) {
    let m = &w.air.dut.m;
    let mut d = Vec::new();
    for (a, b) in DIGITAL {
        for o in a..=b {
            d.push((o, m.regs[o]));
        }
    }
    (d, m.analog[..256].to_vec())
}

/// For each coordinator frame that asks for an acknowledgement: microseconds
/// from its end to the start of the bulb's ACK when the ACK came before the
/// coordinator's next frame (None: no ACK), and whether the coordinator sent
/// the same frame again (its MAC retry, which gets an entry of its own).
fn ack_delays(w: &World) -> Vec<(Option<f64>, bool)> {
    let fr = &w.air.frames;
    let mut out = Vec::new();
    for (i, f) in fr.iter().enumerate() {
        if f.who != NodeId::Coord {
            continue;
        }
        let Some(m) = mac_parse(&f.psdu) else { continue };
        if !m.ar || m.ty == 2 {
            continue;
        }
        let rest = &fr[i + 1..(i + 4).min(fr.len())];
        let next = rest.iter().position(|g| g.who == NodeId::Coord).unwrap_or(rest.len());
        let ack = rest[..next]
            .iter()
            .find(|g| g.who == NodeId::Dut && g.psdu.len() == 5 && g.psdu[0] & 7 == 2 && g.psdu[2] == m.seq);
        let retried = rest.get(next).is_some_and(|g| g.psdu == f.psdu);
        out.push((ack.map(|g| (g.t0 - f.t1) * 1000.0), retried));
    }
    out
}

fn complaints(w: &World) -> (String, bool) {
    let r = w.air.dut.radio.b();
    let why = if r.dropped.is_empty() {
        "none".to_string()
    } else {
        format!("{{{}}}", r.dropped.iter().map(|(k, v)| format!("'{k}': {v}")).collect::<Vec<_>>().join(", "))
    };
    (format!("sent {}, received {}, dropped {why}, collisions {}", r.sent, r.received, r.collisions), r.dropped.is_empty())
}

fn run(cx: &mut SimCtx) -> R<()> {
    let ci = cx.image("stock", "stock/stock-dimm.bin");
    let ri = cx.image("rust", "target/tc32/bulb-tc32.bin");
    for i in [&ci, &ri] {
        if !i.path.exists() {
            return Err(Stop::Error(format!("{} not found", i.path.display())));
        }
    }
    let rel = super::util::rel;
    let mut results: Vec<bool> = Vec::new();
    let mut check = |cx: &mut SimCtx, name: &str, ok: bool, detail: &str| {
        results.push(ok);
        cx.results.push((name.into(), ok, detail.into()));
        let d = if detail.is_empty() { String::new() } else { format!("  {detail}") };
        cx.say(&format!("  {}  {name}{d}", if ok { "PASS" } else { "FAIL" }));
        ok
    };

    let ev = Rc::new(RefCell::new(Vec::new()));
    let flush = |cx: &mut SimCtx, ev: &Rc<RefCell<Vec<String>>>| {
        let lines: Vec<String> = ev.borrow_mut().drain(..).collect();
        for l in lines {
            cx.say(&l);
        }
    };
    cx.say(&format!("vendor driver: {}", rel(&ci)));
    let (cw, c_ok) = join(&ci, 30000.0, &ev)?;
    flush(cx, &ev);
    check(cx, "the stock firmware joins (reference run)", c_ok, &format!("after {:.1} s", cw.now() / 1000.0));
    cx.say(&format!("Rust driver: {}", rel(&ri)));
    let (rw, r_ok) = join(&ri, 30000.0, &ev)?;
    flush(cx, &ev);
    check(cx, "the Rust firmware joins", r_ok, &format!("after {:.1} s", rw.now() / 1000.0));
    if c_ok && r_ok {
        let (mut cw, mut rw) = (cw, rw);
        let (cs, rs) = (settle_rx(&mut cw, 3000.0)?, settle_rx(&mut rw, 3000.0)?);
        cx.say(&format!(
            "register state after joining, receiver idle on channel {CHANNEL} (waited {} ms for the vendor, {} ms for Rust)",
            cs.map(|x| format!("{x:.1}")).unwrap_or_else(|| "in vain 3000".into()),
            rs.map(|x| format!("{x:.1}")).unwrap_or_else(|| "in vain 3000".into())
        ));
        let ((c_dig, c_ana), (r_dig, r_ana)) = (snapshot(&cw), snapshot(&rw));
        let same = c_dig.iter().zip(&r_dig).filter(|(a, b)| a.1 == b.1).count();
        let diff: Vec<(usize, u8, u8)> =
            c_dig.iter().zip(&r_dig).filter(|(a, b)| a.1 != b.1).map(|(a, b)| (a.0, a.1, b.1)).collect();
        let unexpected = diff.iter().filter(|d| expected(d.0).is_none()).count();
        for (o, cv, rv) in &diff {
            cx.say(&format!("      {o:#06x}: vendor {cv:#04x}, Rust {rv:#04x}  ({})", expected(*o).unwrap_or("UNEXPECTED")));
        }
        check(cx, "digital RF, DMA and clock registers equal the vendor's", unexpected == 0,
              &format!("{same} of {} equal, {} intended difference(s), {unexpected} unexpected", c_dig.len(), diff.len() - unexpected));
        let adiff: Vec<(usize, u8, u8)> = (0..256).filter(|i| c_ana[*i] != r_ana[*i]).map(|i| (i, c_ana[i], r_ana[i])).collect();
        let aunexpected: Vec<&(usize, u8, u8)> = adiff.iter().filter(|d| expected_analog(d.0).is_none()).collect();
        let mut reasons: Vec<(&str, Vec<String>)> = Vec::new();
        for (i, _, _) in &adiff {
            let why = expected_analog(*i).unwrap_or("UNEXPECTED");
            match reasons.iter_mut().find(|(w, _)| *w == why) {
                Some(e) => e.1.push(format!("{i:#04x}")),
                None => reasons.push((why, vec![format!("{i:#04x}")])),
            }
        }
        for (why, regs) in &reasons {
            cx.say(&format!("      analog {}: {why}", regs.join(", ")));
        }
        cx.say(&format!(
            "      analog (vendor, Rust): {}",
            adiff.iter().map(|(i, c, r)| format!("{i:#04x} {c:#04x} {r:#04x}")).collect::<Vec<_>>().join("; ")
        ));
        let ushow = format!(
            "[{}]",
            aunexpected.iter().map(|(i, c, r)| format!("('{i:#x}', '{c:#x}', '{r:#x}')")).collect::<Vec<_>>().join(", ")
        );
        check(cx, "analog registers equal the vendor's", aunexpected.is_empty(),
              &format!("{} of 256 equal, {} intended difference(s), unexpected: {ushow}", 256 - adiff.len(), adiff.len() - aunexpected.len()));
        let (c_ch, r_ch) = (channel(&cw), channel(&rw));
        let g = &rw.air.dut.m.regs;
        check(cx, "tuned to the coordinator's channel, receiver on", r_ch == Some(CHANNEL) && c_ch == Some(CHANNEL) && rx_on(&rw),
              &format!("channel {}, 0xf02={:#x}, 0x428={:#x}", chs(r_ch), g[0xF02], g[0x428]));

        cx.say("acknowledgement turnaround");
        for (name, w) in [("vendor", &cw), ("Rust", &rw)] {
            let d = ack_delays(w);
            let got: Vec<f64> = d.iter().filter_map(|x| x.0).collect();
            // a first copy the bulb did not hear (with BLE in the image the radio is
            // lent to an advertising event for about 2 ms every 300 ms), acknowledged on the retry
            let retried = d.iter().filter(|x| x.0.is_none() && x.1).count();
            let mut line = format!("{} of {} frames acknowledged", got.len(), d.len());
            if retried > 0 {
                line += &format!(" ({retried} first copy/copies missed and acknowledged on the coordinator's MAC retry)");
            }
            let (lo, hi) = (got.iter().copied().fold(f64::INFINITY, f64::min), got.iter().copied().fold(f64::NEG_INFINITY, f64::max));
            if !got.is_empty() {
                line += &format!(", start {lo:.0}..{hi:.0} us after the frame");
            }
            if name == "vendor" {
                cx.say(&format!("      vendor firmware: {line}"));
            } else {
                check(cx, "every frame with an acknowledgement request is acknowledged, at the latest on the MAC retry",
                      d.len() >= 8 && got.len() + retried == d.len() && retried <= 1, &line);
                check(cx, "the ACK starts 160..260 us after the end of the frame", !got.is_empty() && 160.0 <= lo && hi <= 260.0,
                      &if got.is_empty() { String::new() } else { format!("{lo:.0}..{hi:.0} us") });
            }
        }

        cx.say("energy detection (Mgmt_NWK_Update_req, scan of all 16 channels)");
        let short = rw.coord.devices.iter().find(|d| d.announced).unwrap().short;
        let n0 = rw.coord.zcl_rsp.len();
        let mut q = 0x07FFF800u32.to_le_bytes().to_vec();
        q.extend_from_slice(&[0, 1]);
        rw.coord.zdo_send(&mut rw.air, short, 0x0038, &q);
        let end = rw.now() + 3000.0;
        let notify = |w: &World| w.coord.zcl_rsp[n0..].iter().find(|x| x.cluster == 0x8038).map(|x| x.body.clone());
        while rw.now() < end && notify(&rw).is_none() {
            let t = rw.now() + 20.0;
            rw.run_ms(t).map_err(emu)?;
        }
        let p = notify(&rw).unwrap_or_default();
        let vals = format!("[{}]", p.get(11..).unwrap_or(&[]).iter().map(|x| x.to_string()).collect::<Vec<_>>().join(", "));
        check(cx, "the scan is answered with an energy value for every channel", p.len() == 11 + 16 && p[1] == 0 && p[10] == 16,
              &format!("status {}, values {vals}", p.get(1).map(|x| x.to_string()).unwrap_or_else(|| "None".into())));
        check(cx, "afterwards the radio is back on the network's channel and listening", channel(&rw) == Some(CHANNEL) && rx_on(&rw),
              &format!("channel {}", chs(channel(&rw))));

        cx.say("radio model's complaints");
        let (line, _) = complaints(&cw);
        cx.say(&format!("      vendor firmware: {line}"));
        let (line, clean) = complaints(&rw);
        check(cx, "no transmission outside TX mode, no frame lost to a receiver left off", clean, &line);
        check(cx, "no MAC retries were needed by the coordinator",
              !rw.coord.events.iter().any(|e| e.kind == "mac-retry-upper" || e.kind == "mac-no-ack"), "");
    }
    let n = results.len();
    let ok = results.iter().filter(|x| **x).count();
    cx.say(&format!("\n{ok}/{n} checks passed"));
    Ok(())
}
