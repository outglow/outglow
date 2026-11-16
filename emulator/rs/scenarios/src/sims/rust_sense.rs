//! Motion detection from RSSI on the Rust firmware (the `sense` feature, in
//! every image, off until switched on): three emulated bulbs and the
//! coordinator, which switches the sensing on in each (the probe rate), the channel between the bulbs the one
//! measured between the owner's corridor bulbs K1, K2, K3
//! (`crates/sense/tests/model`: the level of every link, the smaller spread
//! its readings allow, the share of probes lost). It checks what the C
//! firmware's scenario (emulator/sense_sim.py) checks, and the
//! calibration the measured links need: uncalibrated, the default
//! sensitivity reports an empty corridor (`crates/sense/tests/measured.rs`).

use std::cell::RefCell;
use std::rc::Rc;

#[path = "../../../../../crates/sense/tests/model/mod.rs"]
mod model;

use super::sense_zcl::{hexo, reports_text, sens_text, Sense, A_CAL, A_LINE, A_LINES, A_RATE, A_SENS, A_TUNE, A_ZONE, CL_OCC, CL_SENSE, MANU, QUANTUM, SENSE_PAN};
use super::{node_of, Sim, SimCtx, SimDef};
use crate::air::{AirFrame, Channel as AirChannel, Fate, NodeId};
use crate::coord::{Coord, NWK_KEY};
use crate::frames::mac_parse;
use crate::py;
use crate::session::R;

pub fn def() -> SimDef {
    SimDef { name: "rust_sense", doc: "motion detection from RSSI between three Rust bulbs, switched on from ZCL, on the measured corridor links", run }
}

const NAMES: [&str; 3] = ["K1", "K2", "K3"];
const EXTS: [u64; 3] = [0xa4c1381122330001, 0xa4c1384455660002, 0xa4c1387788990003];

fn index(id: NodeId) -> Option<usize> {
    match id {
        NodeId::Dut => Some(0),
        NodeId::Bulb(i) => Some(i as usize + 1),
        _ => None,
    }
}

fn is_probe(psdu: &[u8]) -> bool {
    psdu.len() >= 9 && u16::from_le_bytes([psdu[3], psdu[4]]) == SENSE_PAN
}

/// The corridor: per directed link between the bulbs its measured channel.
struct Corridor {
    ch: [[Option<model::Channel>; 3]; 3],
    g: model::Rng,
}

impl Corridor {
    fn new() -> Corridor {
        let mut ch: [[Option<model::Channel>; 3]; 3] = Default::default();
        for i in 0..3 {
            for j in 0..3 {
                if i != j {
                    let l = model::link(NAMES[i], NAMES[j]);
                    let mut c = model::Channel::of(&l, l.sigma_lo);
                    c.drift_per_h = model::DRIFT_DB / 3.2;
                    ch[i][j] = Some(c);
                }
            }
        }
        Corridor { ch, g: model::Rng(11) }
    }

    /// Both directions of the link between bulbs `a` and `b`.
    fn both(&mut self, a: usize, b: usize, mut f: impl FnMut(&mut model::Channel)) {
        f(self.ch[a][b].as_mut().unwrap());
        f(self.ch[b][a].as_mut().unwrap());
    }
}

/// The air between the bulbs: the corridor's RSSI for every frame, and its
/// loss for the probes (Zigbee frames keep the ideal air of the other
/// scenarios, so that only the sensing sees the model).
struct Air(Rc<RefCell<Corridor>>);

impl AirChannel for Air {
    fn fate(&mut self, _idx: usize, sender: NodeId, rx: NodeId, t0: f64, _t1: f64, psdu: &[u8], rssi: i64) -> Fate {
        let (Some(s), Some(r)) = (index(sender), index(rx)) else {
            return Fate::Ok(rssi);
        };
        let mut c = self.0.borrow_mut();
        let Corridor { ch, g } = &mut *c;
        let link = ch[s][r].as_mut().unwrap();
        match link.rssi(g, t0.max(0.0) as u32) {
            Some(v) => Fate::Ok(v as i64),
            None if is_probe(psdu) => Fate::Lost,
            None => Fate::Ok((link.level + link.offset).round() as i64),
        }
    }

    fn heard(&mut self, _idx: usize, _rx: NodeId, _frames: &[AirFrame]) -> bool {
        true
    }
}

/// Probes sent from t0 to t1 by bulb `who` (or by any).
fn probes(x: &Sense, t0: f64, t1: f64, who: Option<usize>) -> Vec<(f64, usize, Vec<u8>)> {
    let mut out = vec![];
    for f in &x.s.w.air.frames {
        let Some(i) = index(f.who) else { continue };
        if !(t0 <= f.t0 && f.t0 < t1) || who.is_some_and(|w| w != i) || !is_probe(&f.psdu) {
            continue;
        }
        out.push((f.t0, i, (*f.psdu).clone()));
    }
    out
}

fn occupied_since(x: &Sense, short: u16, t: f64) -> Vec<f64> {
    x.reports(short, CL_OCC, 0, t).iter().filter(|r| r.1 != 0).map(|r| r.0).collect()
}

/// Runs until every bulb in `who` reported occupancy after `t`, at most `ms`.
fn wait_occupied(x: &mut Sense, shorts: &[u16], t: f64, ms: f64) -> R<()> {
    let end = x.now() + ms;
    while x.now() < end {
        if shorts.iter().all(|s| !occupied_since(x, *s, t).is_empty()) {
            break;
        }
        let to = end.min(x.now() + 20.0);
        x.s.w.lockstep(to, QUANTUM).map_err(super::emu)?;
    }
    Ok(())
}

fn run(cx: &mut SimCtx) -> R<()> {
    let img = cx.image("bin", "target/tc32/bulb-tc32.bin");
    let mut nodes = vec![];
    for ext in EXTS {
        let mut n = node_of(&img, cx.cpi)?;
        n.write_flash(0xFF000, &ext.to_le_bytes());
        nodes.push(n);
    }
    let corridor = Rc::new(RefCell::new(Corridor::new()));
    let mut it = nodes.into_iter();
    let mut s = Sim::new(it.next().unwrap(), NAMES[0], Coord::new_plain(11, NWK_KEY), cx.log(), QUANTUM);
    s.w.air.channel = Some(Box::new(Air(corridor.clone())));
    let ids = [NodeId::Dut, s.w.add_bulb(it.next().unwrap(), NAMES[1]), s.w.add_bulb(it.next().unwrap(), NAMES[2])];
    s.w.coord.start(&mut s.w.air);
    // the bulbs power on one after another
    s.w.set_t_off(ids[1], 1234.5);
    s.w.set_t_off(ids[2], 2471.0);
    let mut x = Sense { s, results: Vec::new(), retries: Vec::new() };
    let base = img.path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
    cx.say(&format!("== firmware: {base} x3; coordinator PAN {:#06x} channel 11", x.s.w.coord.st.pan));
    {
        let c = corridor.borrow();
        for (i, j) in [(0, 1), (0, 2), (1, 2)] {
            let l = c.ch[i][j].as_ref().unwrap();
            cx.say(&format!("   {}-{}: {:.2} dBm, spread {:.2} dB, probes lost {:.0} %", NAMES[i], NAMES[j], l.level, l.sigma, 100.0 * l.loss));
        }
    }

    // 1. joining
    let ok = x.run_until(60_000.0, |s| s.w.coord.devices.iter().filter(|d| d.announced).count() == 3)?;
    let now = x.now();
    x.check(cx, "the three bulbs joined the network", ok, &format!("after {:.1} s", now / 1000.0));
    if !ok {
        x.finish(cx);
        return Ok(());
    }
    let mut devs: Vec<_> = x.s.w.coord.devices.iter().filter(|d| d.announced).cloned().collect();
    devs.sort_by_key(|d| d.ext);
    let sh: Vec<u16> = devs.iter().map(|d| d.short).collect();
    cx.say(&format!("   K1 -> {:#06x}, K2 -> {:#06x}, K3 -> {:#06x}", sh[0], sh[1], sh[2]));
    x.s.run(4000.0)?;

    // 2. clusters and defaults
    let occ = x.read(sh[0], CL_OCC, 0x0000, None)?;
    let delay = x.read(sh[0], CL_OCC, 0x0010, None)?;
    let sens = x.read(sh[0], CL_SENSE, A_SENS, Some(MANU))?;
    let rate = x.read(sh[0], CL_SENSE, A_RATE, Some(MANU))?;
    let tune = x.read(sh[0], CL_SENSE, A_TUNE, Some(MANU))?;
    x.check(cx, "Occupancy Sensing 0x0406: occupancy=0, OFF delay=30 s",
            occ.as_deref() == Some(&[0x00][..]) && delay.as_deref() == Some(&30u16.to_le_bytes()[..]),
            &format!("occ={} delay={}", hexo(&occ), hexo(&delay)));
    // 40, not the C firmware's 45 (crates/zigbee DEFAULT_S: 45 takes the measured spread for motion)
    x.check(cx, "manufacturer cluster 0xFC5E: sensitivity=40, probes=0/s (the sensing off out of the box), self-tuning off",
            sens.as_deref() == Some(&[40][..]) && rate.as_deref() == Some(&[0][..]) && tune.as_deref() == Some(&[0][..]),
            &format!("s={} rate={} tune={}", hexo(&sens), hexo(&rate), hexo(&tune)));
    // switched on in every bulb, as the converter's motion_sensing switch does
    let mut on = true;
    for s in &sh {
        on &= x.write(*s, CL_SENSE, A_RATE, 0x20, &[5], Some(MANU))?;
    }
    x.check(cx, "the sensing switched on in every bulb (rate 5)", on, "");

    // 3. no probes in the first minute of each bulb
    x.run_until(70_000.0, |s| s.now() >= 61_000.0)?;
    let early: Vec<(f64, usize)> = probes(&x, 0.0, 61_000.0, None).iter().map(|p| (p.0, p.1)).collect();
    let starts = [0.0, 1234.5, 2471.0];
    let too_early = early.iter().filter(|(t, i)| *t < starts[*i] + 60_000.0).count();
    x.check(cx, "no probes in the first 60 s after each bulb's start", too_early == 0, &format!("probes before their minute={too_early}"));

    // 4. probes: rate, format, foreign PAN, no acknowledgement
    x.run_until(30_000.0, |s| s.now() >= 76_000.0)?;
    let ps = probes(&x, 64_000.0, 76_000.0, None);
    let counts: Vec<usize> = (0..3).map(|i| ps.iter().filter(|p| p.1 == i).count()).collect();
    x.check(cx, "probes from every bulb ~5/s", counts.iter().all(|n| (45..=75).contains(n)), &format!("K1={} K2={} K3={} in 12 s", counts[0], counts[1], counts[2]));
    if let Some((_, who, psdu)) = ps.first() {
        let pan = x.s.w.coord.st.pan;
        let f = mac_parse(psdu);
        let fmt_ok = psdu.len() == 24
            && psdu[..2] == [0x41, 0x88]
            && f.as_ref().is_some_and(|f| f.dst == Some(0xFFFF) && f.src == Some(sh[*who] as u64) && f.payload.len() == 13)
            && psdu[9..12] == *b"EG\x01"
            && u16::from_le_bytes([psdu[12], psdu[13]]) == pan
            && psdu[18] == 5;
        let hexp: String = psdu.iter().map(|b| format!("{b:02x}")).collect();
        x.check(cx, "probe format as the C firmware's: FCF 0x8841, PAN 0x5E55, 13 B payload with the network PAN", fmt_ok, &hexp);
    }
    let acks = x.s.w.air.frames.iter().filter(|f| 64_000.0 <= f.t0 && f.t0 < 76_000.0 && f.psdu.len() == 5 && ps.iter().any(|p| 0.0 < f.t0 - p.0 && f.t0 - p.0 < 2.0)).count();
    x.check(cx, "nobody acknowledges a probe", acks == 0, &format!("ACK={acks}"));

    // 5. the lines and their levels
    let ln = x.read(sh[0], CL_SENSE, A_LINES, Some(MANU))?.unwrap_or_default();
    let lines: Vec<(u16, i8, u8, u8, u8)> = (0..ln.len() / 6).map(|k| {
        let i = 6 * k;
        (u16::from_le_bytes([ln[i], ln[i + 1]]), ln[i + 2] as i8, ln[i + 3], ln[i + 4], ln[i + 5])
    }).collect();
    let level_of = |src: u16| lines.iter().find(|l| l.0 == src).map(|l| l.1 as i32);
    let (l12, l13) = (model::link("K1", "K2").level, model::link("K1", "K3").level);
    let near = |v: Option<i32>, want: f64| v.is_some_and(|v| (v as f64 - want).abs() <= 4.0);
    let ltext = py::list(lines.iter().map(|(s, r, n, sm, st)| py::tuple([format!("'{s:#x}'"), r.to_string(), py::float(*n as f64 / 8.0), sm.to_string(), st.to_string()])));
    x.check(cx, &format!("K1's lines: from K2 near {l12:.1} dBm, from K3 near {l13:.1} dBm (the measured levels)"),
            lines.len() == 2 && near(level_of(sh[1]), l12) && near(level_of(sh[2]), l13), &ltext);

    // 6. Zigbee alongside the probes
    let r = x.zcl(sh[0], 0x0006, 0x00, &[], None, true)?;
    x.s.run(1500.0)?;
    let off = !x.s.light(ids[0]).on;
    let r2 = x.zcl(sh[0], 0x0006, 0x01, &[], None, true)?;
    x.s.run(3000.0)?;
    x.check(cx, "On/Off to K1 during probes", r.is_some() && r2.is_some() && off && x.s.light(ids[0]).on, "");

    // 7. uncalibrated: what the default does on these links (no check: the
    //    red spec `the_default_sensitivity_is_quiet_on_the_measured_links`)
    x.s.run(30_000.0)?;
    let early: Vec<usize> = sh.iter().map(|s| occupied_since(&x, *s, 60_000.0).len()).collect();
    cx.say(&format!(
        "   uncalibrated, s = 40, the empty corridor from the first probes on ({:.0} s): occupancy=1 reports K1={} K2={} K3={}",
        (x.now() - 60_000.0) / 1000.0,
        early[0],
        early[1],
        early[2]
    ));

    // 8. calibration of every bulb from ZCL
    let before = x.line_sens(sh[0])?;
    let mut w_ok = true;
    for s in &sh {
        w_ok &= x.write(*s, CL_SENSE, A_CAL, 0x20, &[60], Some(MANU))?;
        w_ok &= x.write(*s, CL_OCC, 0x0010, 0x21, &5u16.to_le_bytes(), None)?;
    }
    x.s.run(2000.0)?;
    let left = x.read(sh[0], CL_SENSE, A_CAL, Some(MANU))?;
    x.check(cx, "write calibration 60 s to every bulb: countdown in the attribute",
            w_ok && left.as_ref().is_some_and(|l| l.len() == 1 && (50..=59).contains(&l[0])), &format!("left={}", hexo(&left)));
    x.s.run(60_000.0)?;
    let mut after = vec![];
    for s in &sh {
        after.push(x.line_sens(*s)?);
    }
    let anchored = after.iter().all(|v| v.len() == 2 && v.iter().all(|l| l.1 == l.2 && l.1 <= 49));
    let k1k3 = after[0].iter().find(|l| l.0 == sh[2]).map(|l| l.1);
    let k1k2 = after[0].iter().find(|l| l.0 == sh[1]).map(|l| l.1);
    x.check(cx, "after calibration: every line has its own s (= its anchor), the noisier K1-K3 line less sensitive than K1-K2",
            before.len() == 2 && before.iter().all(|l| l.1 == 40) && anchored && k1k3.zip(k1k2).is_some_and(|(a, b)| a < b),
            &format!("K1 before={} after: K1={} K2={} K3={}", sens_text(&before), sens_text(&after[0]), sens_text(&after[1]), sens_text(&after[2])));

    // 9. the empty corridor after calibration
    x.s.run(5000.0)?;
    let t0 = x.now();
    x.s.run(180_000.0)?;
    let quiet: Vec<usize> = sh.iter().map(|s| occupied_since(&x, *s, t0).len()).collect();
    x.check(cx, "after calibration: 3 minutes of the empty corridor without an occupancy report", quiet.iter().all(|n| *n == 0),
            &format!("K1={} K2={} K3={}", quiet[0], quiet[1], quiet[2]));

    // 10. a person crosses the K1-K2 line
    let t_cross = x.now() + 500.0;
    corridor.borrow_mut().both(0, 1, |c| c.crossings.push(model::Crossing { t0: t_cross as u32, t1: t_cross as u32 + 1500, depth: 8.0 }));
    wait_occupied(&mut x, &sh[..2], t_cross, 6000.0)?;
    let (o1, o2) = (occupied_since(&x, sh[0], t_cross), occupied_since(&x, sh[1], t_cross));
    let d = match (o1.first(), o2.first()) {
        (Some(a), Some(b)) => format!("K1 after {:.0} ms, K2 after {:.0} ms", a - t_cross, b - t_cross),
        _ => format!("K1={} K2={}", py::floats(o1.iter().copied()), py::floats(o2.iter().copied())),
    };
    x.check(cx, "crossing the K1-K2 line (8 dB, 1.5 s): occupancy=1 from K1 and K2 within 3 s",
            o1.first().zip(o2.first()).is_some_and(|(a, b)| a - t_cross < 3000.0 && b - t_cross < 3000.0), &d);
    x.s.run(300.0)?;
    let zone = x.reports(sh[0], CL_SENSE, A_ZONE, t_cross);
    let lstate = x.reports(sh[0], CL_SENSE, A_LINE, t_cross);
    let k3 = occupied_since(&x, sh[2], t_cross);
    x.check(cx, "manufacturer cluster report from K1: zone=1 and the bit of one line; K3, off that line, reports nothing",
            zone.first().is_some_and(|z| z.1 == 1) && lstate.first().is_some_and(|z| z.1.count_ones() == 1) && k3.is_empty(),
            &format!("zone={} line={} K3={}", reports_text(&zone[..zone.len().min(1)]), reports_text(&lstate[..lstate.len().min(1)]), k3.len()));
    let end = x.now() + 15_000.0;
    while x.now() < end && !x.reports(sh[0], CL_OCC, 0, t_cross).iter().any(|r| r.1 == 0) {
        let to = end.min(x.now() + 20.0);
        x.s.w.lockstep(to, QUANTUM).map_err(super::emu)?;
    }
    let off_at = x.reports(sh[0], CL_OCC, 0, t_cross).iter().find(|r| r.1 == 0).map(|r| r.0 - t_cross);
    x.check(cx, "occupancy=0 about 5 s after the last hit", off_at.is_some_and(|d| (4500.0..=12_000.0).contains(&d)),
            &off_at.map(|d| format!("{d:.0} ms after the start of the crossing")).unwrap_or_default());
    x.s.run(8000.0)?;

    // 11. K2's light goes off and the K1-K2 link jumps by 8 dB with it: not motion
    let t0 = x.now();
    corridor.borrow_mut().both(0, 1, |c| c.offset = -8.0);
    x.zcl(sh[1], 0x0006, 0x00, &[], None, true)?;
    x.s.run(12_000.0)?;
    let (a, b) = (occupied_since(&x, sh[0], t0).len(), occupied_since(&x, sh[1], t0).len());
    x.check(cx, "K2 off + a jump of 8 dB on K1-K2: no occupancy (K1 from the counter in the probe, K2 from its own light)",
            a == 0 && b == 0, &format!("K1={a} K2={b}"));
    corridor.borrow_mut().both(0, 1, |c| c.offset = 0.0);
    x.s.run(6000.0)?;
    let ctl = !occupied_since(&x, sh[0], t0 + 12_000.0).is_empty();
    x.check(cx, "the same jump back without a light change: occupancy (the blanking does something)", ctl, "");
    x.s.run(10_000.0)?;

    // 12. the rate from ZCL
    let w = x.write(sh[0], CL_SENSE, A_RATE, 0x20, &[0], Some(MANU))?;
    x.s.run(500.0)?;
    let t0 = x.now();
    x.s.run(5000.0)?;
    let n0 = probes(&x, t0, x.now(), Some(0)).len();
    x.write(sh[0], CL_SENSE, A_RATE, 0x20, &[10], Some(MANU))?;
    x.s.run(500.0)?;
    let t0 = x.now();
    x.s.run(5000.0)?;
    let n10 = probes(&x, t0, x.now(), Some(0)).len();
    x.check(cx, "K1 sends no probes at rate=0 and ~10/s at rate=10", w && n0 == 0 && (40..=62).contains(&n10), &format!("{n0} and {n10} in 5 s"));
    x.write(sh[0], CL_SENSE, A_RATE, 0x20, &[5], Some(MANU))?;
    let tw = x.write(sh[0], CL_SENSE, A_TUNE, 0x20, &[1], Some(MANU))?;

    // 13. after a restart: configuration and the calibrated lines from NV
    x.s.run(1000.0)?;
    x.s.w.reboot_bulb(ids[0]).map_err(super::emu)?;
    let t = x.s.w.bulb_t(ids[0]);
    x.s.w.set_t_off(ids[0], t);
    x.s.run(8000.0)?;
    let delay = x.read(sh[0], CL_OCC, 0x0010, None)?;
    let rate = x.read(sh[0], CL_SENSE, A_RATE, Some(MANU))?;
    let tune = x.read(sh[0], CL_SENSE, A_TUNE, Some(MANU))?;
    let ls = x.line_sens(sh[0])?;
    x.check(cx, "after K1's restart: OFF delay=5 s, probes=5/s, self-tuning=1 and the lines' s from NV",
            tw && delay.as_deref() == Some(&5u16.to_le_bytes()[..]) && rate.as_deref() == Some(&[5][..]) && tune.as_deref() == Some(&[1][..]) && ls == after[0],
            &format!("delay={} rate={} tune={} lines={}", hexo(&delay), hexo(&rate), hexo(&tune), sens_text(&ls)));

    // 14. switched off: no probes, Occupancy 0, nothing reported
    let w = x.write(sh[0], CL_SENSE, A_RATE, 0x20, &[0], Some(MANU))?;
    x.s.run(500.0)?;
    let t0 = x.now();
    let n_occ = x.reports(sh[0], CL_OCC, 0, t0).len();
    x.s.run(10_000.0)?;
    let n_probes = probes(&x, t0, x.now(), Some(0)).len();
    let occ = x.read(sh[0], CL_OCC, 0x0000, None)?;
    let n_occ = x.reports(sh[0], CL_OCC, 0, t0).len() - n_occ;
    x.check(cx, "K1 switched off (rate 0): no probes, no reports, Occupancy 0",
            w && n_probes == 0 && n_occ == 0 && occ.as_deref() == Some(&[0][..]),
            &format!("probes={n_probes} reports={n_occ} occ={}", hexo(&occ)));

    // 15. stability
    let resets: Vec<u32> = ids.iter().map(|i| x.s.w.bulb_resets(*i)).collect();
    x.check(cx, "no firmware resets and no network leave", resets.iter().all(|r| *r == 0) && x.s.events("leave").is_empty(),
            &format!("resets K1={} K2={} K3={}", resets[0], resets[1], resets[2]));
    cx.say(&format!("   ZCL commands retried for lack of a response: {}", x.retries.len()));
    for (i, id) in ids.iter().enumerate() {
        let (sent, recv, why) = x.s.stats(*id);
        cx.say(&format!("   {}: sent {sent}, received {recv}, dropped by radio {why}", NAMES[i]));
    }
    x.finish(cx);
    Ok(())
}
