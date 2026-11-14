//! Zigbee and BLE side by side on one radio, for the Rust firmware
//! (emulator/rust_ble_sim.py): Zigbee answers while the bulb advertises and
//! while it is connected at 30 ms and at 7.5 ms; few lost connection events;
//! a change over one radio shows on the other; the 802.15.4 registers after
//! BLE events are what they were before.

use std::collections::BTreeMap;

use super::ble_central::{
    discover, fmt_light, hx, ok, secure_pw, tel_get, unhx, Bench, MeshClient, Status, Transport, OP_POWER,
    OP_WHITE_BRIGHTNESS, OP_WHITE_TEMPERATURE,
};
use super::{node_of, SimCtx, SimDef};
use crate::air::NodeId;
use crate::session::{Stop, R};

pub fn def() -> SimDef {
    SimDef { name: "rust_ble", doc: "Zigbee and BLE side by side on one radio (Rust firmware)", run }
}

const D: NodeId = NodeId::Dut;
const EP: u8 = 1;
const RF_RANGES: [(usize, usize); 7] =
    [(0x400, 0x440), (0x460, 0x470), (0xC08, 0xC0C), (0xC20, 0xC22), (0xF02, 0xF18), (0xF1C, 0xF1E), (0x1200, 0x1300)];
const BLE_OWN: [usize; 2] = [0xF00, 0xF01];

fn rf_regs(b: &Bench) -> BTreeMap<usize, u8> {
    let mut out = BTreeMap::new();
    for (lo, hi) in RF_RANGES {
        for (i, x) in b.mem(D, 0x800000 + lo, hi - lo).iter().enumerate() {
            out.insert(lo + i, *x);
        }
    }
    out
}

fn reg_diff(before: &BTreeMap<usize, u8>, after: &BTreeMap<usize, u8>) -> Vec<(usize, u8, u8)> {
    before.iter().filter(|(k, v)| after[k] != **v && !BLE_OWN.contains(k)).map(|(k, v)| (*k, *v, after[k])).collect()
}

/// The registers against `before` outside a BLE event: an event (about 1.1 ms
/// of a connection, 2.4 ms of advertising) has the radio in BLE mode, so the
/// snapshot is taken again up to three times 2.5 ms apart, and the first that
/// matches counts. Registers left in BLE mode after the events differ in all.
fn reg_diff_between_events(b: &mut Bench, before: &BTreeMap<usize, u8>) -> R<Vec<(usize, u8, u8)>> {
    let mut d = reg_diff(before, &rf_regs(b));
    for _ in 0..3 {
        if d.is_empty() {
            break;
        }
        b.run(D, 2.5)?;
        d = reg_diff(before, &rf_regs(b));
    }
    Ok(d)
}

fn diff_text(d: &[(usize, u8, u8)]) -> String {
    d.iter().take(8).map(|(k, a, b)| format!("{k:#x}: {a:#04x}->{b:#04x}")).collect::<Vec<_>>().join(", ")
}

/// `until(cond, max_ms, step)` of the script: the condition is checked before every step.
fn until(b: &mut Bench, max_ms: f64, step: f64, mut cond: impl FnMut(&Bench) -> bool) -> R<bool> {
    let end = b.t(D) + max_ms;
    while b.t(D) < end && !cond(b) {
        b.run(D, step)?;
    }
    Ok(cond(b))
}

/// Read Attributes over Zigbee; the value bytes after the type, or None.
fn zcl_read(b: &mut Bench, short: u16, cluster: u16, attr: u16) -> R<Option<Vec<u8>>> {
    let n0 = b.w.coord.zcl_rsp.len();
    b.w.coord.read_attrs(&mut b.w.air, short, EP, cluster, &[attr]);
    let got = |b: &Bench| b.w.coord.zcl_rsp[n0..].iter().find(|r| r.cluster == cluster && r.cmd == Some(0x01)).map(|r| r.body.clone());
    if !until(b, 1500.0, 5.0, |b| got(b).is_some())? {
        return Ok(None);
    }
    let p = got(b).unwrap();
    Ok(if p.len() >= 5 && p[2] == 0 { Some(p[4..].to_vec()) } else { None })
}

fn zigbee_answers(b: &mut Bench, short: u16, n: usize, gap_ms: f64) -> R<usize> {
    let mut okn = 0;
    for i in 0..n {
        okn += zcl_read(b, short, 0x0000, 0x0004)?.is_some() as usize;
        b.run(D, gap_ms + 7.0 * (i % 5) as f64)?;
    }
    Ok(okn)
}

fn st_text(s: Option<&Status>) -> String {
    s.map(|x| x.repr()).unwrap_or_else(|| "None".into())
}

fn run(cx: &mut SimCtx) -> R<()> {
    let img = cx.image("bin", "target/tc32/bulb-tc32.bin");
    let tw = cx.images.iter().any(|i| i.label == "tw");
    let node = node_of(&img, cx.cpi)?;
    let mut b = Bench::new(node, "dut", true, None);
    let mut results: Vec<bool> = Vec::new();
    let mut check = |cx: &mut SimCtx, name: &str, okv: bool, detail: &str| -> bool {
        results.push(okv);
        cx.results.push((name.into(), okv, detail.into()));
        cx.say(&format!("  {}  {name}{}", if okv { "PASS" } else { "FAIL" }, if detail.is_empty() { String::new() } else { format!("  {detail}") }));
        okv
    };

    // --- Zigbee first
    let joined = until(&mut b, 60000.0, 20.0, |b| b.w.coord.events.iter().any(|e| e.kind == "device-announce"))?;
    let t = b.t(D);
    check(cx, "joins the Zigbee network with BLE in the image", joined, &format!("after {:.1} s", t / 1000.0));
    if !joined {
        return Ok(());
    }
    let short = b.w.coord.devices.iter().find(|d| d.announced).unwrap().short;
    if let Some(l) = super::util::stack_rearm(b.w.node_mut(D), &img) {
        cx.say(&l);
    }
    b.run(D, 6000.0)?;
    let before = rf_regs(&b);

    // --- advertising next to Zigbee
    let t0 = b.t(D);
    let okn = zigbee_answers(&mut b, short, 40, 110.0)?;
    let adv = b.ble_rf(D, t0).1.iter().filter(|(_, ch, pdu)| *ch == 37 && pdu[0] & 0x0F == 0).count();
    let span = (b.t(D) - t0) / 1000.0;
    check(cx, "Zigbee answers while the bulb advertises", okn >= 39, &format!("{okn}/40 reads answered in {span:.1} s"));
    check(cx, "advertises every 300 to 310 ms on the side", (adv as f64 - span / 0.305).abs() <= 2.0, &format!("{adv} events in {span:.1} s"));
    let d = reg_diff_between_events(&mut b, &before)?;
    check(cx, "802.15.4 radio registers are the same after advertising events", d.is_empty(), &diff_text(&d));

    // --- a scanning central, then a connection at 30 ms
    let t0 = b.t(D);
    let scans0 = b.ble_stats(D).scanreqs;
    b.ble_connect(D, 24, Some(1600.0));
    b.run(D, 1500.0)?;
    let scans = b.ble_stats(D).scanreqs - scans0;
    let sent = b.ble_rf(D, t0).1;
    let rsp: Vec<&Vec<u8>> = sent.iter().filter(|(_, _, p)| p[0] & 0x0F == 4).map(|(_, _, p)| p).collect();
    let advs: Vec<u8> = sent.iter().filter(|(_, _, p)| p[0] & 0x0F == 0).map(|(_, ch, _)| *ch).collect();
    let good = scans >= 4
        && rsp.len() as i64 == scans
        && rsp.iter().all(|r| r[1] == 16 && r[8..] == b"\x09\x09unpaired"[..]);
    check(cx, "a scan request is answered with the scan response: the full mesh name", good,
          &format!("{scans} requests, {} responses, last {}", rsp.len(), rsp.last().map(|r| hx(r)).unwrap_or_else(|| "None".into())));
    let mut chs = advs.clone();
    chs.sort();
    chs.dedup();
    check(cx, "an answered scan ends the advertising event on that channel", !advs.is_empty() && advs.iter().all(|c| *c == 37),
          &format!("channels advertised on: {}", crate::py::ints(chs.iter().map(|c| *c as i64))));
    let adva = b.ble_wait_connected(D, 15000.0)?;
    check(cx, "BLE connection while on the Zigbee network", adva.is_some(), &format!("address {}", adva.clone().unwrap_or_else(|| "None".into())));
    let Some(adva) = adva else { return Ok(()) };
    let mac_le = unhx(&adva);
    let (_, tel, _) = discover(&mut b, D)?;
    let mut tr = Transport::new(D, &tel);
    let mut cl = MeshClient::new(&mac_le);
    let mut mac6 = mac_le.clone();
    mac6.reverse();
    ok(cl.login(&mut b, &mut tr, b"unpaired", &secure_pw(&mac6))?)?;
    let mut q = vec![0x12];
    q.extend_from_slice(&(tel_get(&tel, 0x11).unwrap() + 1).to_le_bytes());
    q.extend_from_slice(&[0x01, 0x00]);
    b.att_ok(D, &q)?;
    ok(tr.write(&mut b, 0x11, &[0x01])?)?;
    b.run(D, 500.0)?;

    // BLE -> light -> Zigbee attribute
    ok(cl.send(&mut b, &mut tr, OP_POWER, &[0], 0xFFFF)?)?;
    b.run(D, 700.0)?;
    let v = zcl_read(&mut b, short, 0x0006, 0x0000)?;
    let l = b.light(D);
    check(cx, "BLE power off: light off, ZCL OnOff reads 0", !l.on && v.as_ref().is_some_and(|v| v[0] == 0),
          &format!("{}, OnOff {}", fmt_light(&l), v.as_ref().map(|v| hx(v)).unwrap_or_else(|| "None".into())));
    // Zigbee -> light -> BLE notification
    let n0 = b.ntfs_len(D);
    b.w.coord.zcl_cmd(&mut b.w.air, short, EP, 0x0006, 0x01, &[]);
    b.run(D, 900.0)?;
    let st = cl.statuses(&b.ntfs(D)[n0..]);
    let l = b.light(D);
    check(cx, "ZCL On: light on, mesh status notification says on", l.on && st.last().is_some_and(|s| s.on),
          &format!("{}, {}", fmt_light(&l), st_text(st.last())));
    let n0 = b.ntfs_len(D);
    b.w.coord.zcl_cmd(&mut b.w.air, short, EP, 0x0008, 0x04, &[100, 0, 0, 0, 0]);
    b.run(D, 900.0)?;
    let st = cl.statuses(&b.ntfs(D)[n0..]);
    check(cx, "ZCL Move to Level 100: notification with brightness 50", st.last().is_some_and(|s| s.white_brightness == 50), &st_text(st.last()));
    ok(cl.send(&mut b, &mut tr, OP_WHITE_BRIGHTNESS, &[127], 0xFFFF)?)?;
    b.run(D, 700.0)?;
    let v = zcl_read(&mut b, short, 0x0008, 0x0000)?;
    check(cx, "BLE brightness 127: ZCL CurrentLevel reads 254", v.as_ref().is_some_and(|v| v[0] == 254),
          &v.as_ref().map(|v| hx(v)).unwrap_or_else(|| "no answer".into()));
    if tw {
        ok(cl.send(&mut b, &mut tr, OP_WHITE_TEMPERATURE, &[0], 0xFFFF)?)?;
        b.run(D, 700.0)?;
        let v = zcl_read(&mut b, short, 0x0300, 0x0007)?;
        check(cx, "TW: BLE temperature 0 (cool): ZCL ColorTemperatureMireds reads 153",
              v.as_ref().is_some_and(|v| v.len() >= 2 && u16::from_le_bytes([v[0], v[1]]) == 153),
              &v.as_ref().map(|v| hx(v)).unwrap_or_else(|| "no answer".into()));
    }

    let s0 = b.ble_stats(D);
    let okn = zigbee_answers(&mut b, short, 40, 110.0)?;
    let s1 = b.ble_stats(D);
    let (events, missed) = (s1.events - s0.events, s1.missed - s0.missed);
    check(cx, "Zigbee answers during a BLE connection at 30 ms", okn >= 38, &format!("{okn}/40 reads answered"));
    check(cx, "the central loses few connection events meanwhile", events > 100 && missed as f64 <= events as f64 * 0.2,
          &format!("{missed} of {events} events unanswered"));
    let rsp = b.att_ok(D, &[0x0a, 0x0b, 0x00])?;
    check(cx, "the attribute protocol still answers", rsp == b"\x0bAwoX", &hx(&rsp));
    let d = reg_diff_between_events(&mut b, &before)?;
    check(cx, "802.15.4 radio registers are the same after connection events", d.is_empty(), &diff_text(&d));
    // --- packets damaged on their way
    b.ble_crc_errors(D, 4);
    let s0 = b.ble_stats(D);
    let want = |h: u16| -> &'static [u8] {
        match h {
            0x0B => b"AwoX",
            0x0F => b"4.63",
            _ => b"3.0.2_1610",
        }
    };
    let mut good = 0;
    for i in 0..60 {
        let h = [0x0Bu16, 0x0F, 0x09][i % 3];
        let mut q = vec![0x0a];
        q.extend_from_slice(&h.to_le_bytes());
        if let Ok(r) = b.att(D, &q, 3000.0, 2.0)? {
            let mut w = vec![0x0b];
            w.extend_from_slice(want(h));
            good += (r == w) as i64;
        }
    }
    let damaged = b.ble_crc_errors(D, 0);
    let s1 = b.ble_stats(D);
    check(cx, "packets with a wrong CRC are not acknowledged: the central repeats them, nothing is lost or doubled",
          good == 60 && damaged >= 20 && (s1.retx - s0.retx) as f64 >= damaged as f64 * 0.8,
          &format!("{good}/60 reads answered in order, {damaged} packets damaged, {} retransmissions", s1.retx - s0.retx));
    let st = b.ble_disconnect(D, 3000.0)?;
    check(cx, "the central ends the connection", st.starts_with("terminat"), &st);

    // --- a connection at 7.5 ms: BLE wants the radio most of the time
    b.run(D, 800.0)?;
    b.ble_connect(D, 6, None);
    let adva = b.ble_wait_connected(D, 15000.0)?;
    check(cx, "BLE connection at 7.5 ms", adva.is_some(), "");
    if adva.is_some() {
        let s0 = b.ble_stats(D);
        let okn = zigbee_answers(&mut b, short, 30, 110.0)?;
        let s1 = b.ble_stats(D);
        let (events, missed) = (s1.events - s0.events, s1.missed - s0.missed);
        check(cx, "Zigbee answers during a BLE connection at 7.5 ms", okn >= 24, &format!("{okn}/30 reads answered"));
        check(cx, "the connection holds, most events answered",
              b.ble_state(D).0 == "connected" && missed as f64 <= events as f64 * 0.25, &format!("{missed} of {events} events unanswered"));
        b.ble_disconnect(D, 3000.0)?;
    }
    b.run(D, 1000.0)?;
    let resets = b.resets(D);
    check(cx, "no leave, no restart", resets == 0 && !b.w.coord.events.iter().any(|e| e.kind == "leave"), &format!("resets {resets}"));
    let last = b.ble_stats(D);
    check(cx, "nothing sent with packets left in the radio's FIFO, no command refused", last.advfifo == 0 && last.refused == 0,
          &format!("advertising with the FIFO not empty {}, refused {}", last.advfifo, last.refused));
    cx.say(&format!("   connection events the bulb opened late (main loop busy): {} of {} by the last central", last.late, last.events));
    let misses = b.ble_misses(D);
    if !misses.is_empty() {
        cx.say(&format!("   last misses reported by the radio model: {}", misses[misses.len().saturating_sub(4)..].join("; ")));
    }
    let _ = Stop::Error(String::new());
    if let Some(l) = super::util::stack_report(b.w.node(D), &img) {
        cx.say(&l);
    }
    cx.say(&format!("== {}/{} passed", results.iter().filter(|x| **x).count(), results.len()));
    Ok(())
}
