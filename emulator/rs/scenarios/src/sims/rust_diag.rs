use super::{SimCtx, SimDef};
use crate::frames::ZclValue;
use crate::session::{Session, SessionOpts, Stop, R};
use crate::station::Tx;

pub fn def() -> SimDef {
    SimDef { name: "rust_diag", doc: "diagnostics, reset cause and the BLE window of the Rust firmware", run }
}

const DIAG: u16 = 0x0B05;
const MFR: u16 = 0x1141;

fn int(v: &Option<ZclValue>) -> Option<i64> {
    match v {
        Some(ZclValue::Int(i)) => Some(*i),
        _ => None,
    }
}

fn read(s: &mut Session, mfr: bool, attrs: &[u16]) -> R<Vec<Option<i64>>> {
    let o = if mfr { Tx::new().mfg(MFR) } else { Tx::new() };
    let r = s.read_with(DIAG, attrs, o)?.ok_or_else(|| Stop::Fail("no Read Attributes Response".into()))?;
    Ok(attrs.iter().map(|a| r.iter().find(|x| x.0 == *a).and_then(|x| int(&x.3))).collect())
}

fn single(s: &mut Session, ep: u8, cluster: u16) -> R<Option<f32>> {
    let r = s.read_with(cluster, &[0x0055], Tx::new().ep(ep))?;
    Ok(r.and_then(|v| v.into_iter().find(|x| x.0 == 0x0055)).and_then(|x| match (x.2, x.3) {
        (Some(0x39), Some(ZclValue::Int(i))) => Some(f32::from_bits(i as u32)),
        (Some(0x10), Some(ZclValue::Int(i))) => Some(i as f32),
        _ => None,
    }))
}

fn emu(e: String) -> Stop {
    Stop::Error(e)
}

fn run(cx: &mut SimCtx) -> R<()> {
    let img = cx.image("bin", "target/tc32/bulb-tc32.bin");
    let mut s = Session::new(&img, SessionOpts { cpi: cx.cpi, ..Default::default() })?;
    s.run(3000.0)?;
    let t = |s: &Session| s.now();

    let m = read(&mut s, true, &[0x5001, 0x5002, 0x5000])?;
    let n0 = read(&mut s, false, &[0x0000])?[0].unwrap_or(-1);
    cx.check(t(&s), "after power-on: reset cause power-on, no watchdog resets, uptime counted", m[0] == Some(0) && m[1] == Some(0)
             && m[2].is_some_and(|u| u >= 1), &format!("cause {:?} watchdog resets {:?} uptime {:?} s, NumberOfResets {n0}", m[0], m[1], m[2]));
    for _ in 0..6 {
        s.read(0x0006, &[0x0000], 1500.0)?;
    }
    s.run(1500.0)?;
    let rx0 = read(&mut s, false, &[0x0102])?[0].unwrap_or(0);

    s.w.soft_reset(true).map_err(emu)?;
    s.run(5000.0)?;
    let m = read(&mut s, true, &[0x5001, 0x5002])?;
    let v = read(&mut s, false, &[0x0000, 0x0102])?;
    cx.check(t(&s), "watchdog reset: cause watchdog, counted once, the counters kept over it", m[0] == Some(1) && m[1] == Some(1)
             && v[0] == Some(n0 + 1) && v[1].is_some_and(|x| x >= rx0),
             &format!("cause {:?}, watchdog resets {:?}, NumberOfResets {:?}, MacRxUcast {rx0} -> {:?}", m[0], m[1], v[0], v[1]));

    s.w.soft_reset(false).map_err(emu)?;
    s.run(5000.0)?;
    let m = read(&mut s, true, &[0x5001, 0x5002])?;
    let v = read(&mut s, false, &[0x0000, 0x0102])?;
    cx.check(t(&s), "software reset: cause software, the watchdog count unchanged", m[0] == Some(2) && m[1] == Some(1)
             && v[0] == Some(n0 + 2) && v[1].is_some_and(|x| x >= rx0), &format!("cause {:?}, watchdog resets {:?}, NumberOfResets {:?}", m[0], m[1], v[0]));

    s.reboot()?;
    s.run(5000.0)?;
    let m = read(&mut s, true, &[0x5001, 0x5002])?;
    let v = read(&mut s, false, &[0x0000, 0x0102])?;
    cx.check(t(&s), "power cut after them: cause power-on, the counts stored at the warm starts are back", m[0] == Some(0) && m[1] == Some(1)
             && v[0] == Some(n0 + 3) && v[1].is_some_and(|x| x >= rx0), &format!("cause {:?}, NumberOfResets {:?}, MacRxUcast {:?}", m[0], v[0], v[1]));

    let nb = single(&mut s, 5, 0x000C)?;
    let ble = single(&mut s, 4, 0x000C)?;
    let allow = single(&mut s, 7, 0x0010)?;
    cx.check(t(&s), "Analog Input: one router neighbour, BLE share in percent; Binary Output: end devices allowed",
             nb == Some(1.0) && ble.is_some_and(|b| (0.0..=100.0).contains(&b)) && allow == Some(1.0),
             &format!("neighbours {nb:?}, BLE {ble:?} %, allow {allow:?}"));

    let f = s.write_with(0x000D, &[(0x0055, 0x39, 1.0f32.to_le_bytes().to_vec())], false, false, Tx::new().ep(8))?;
    let ok = f.and_then(|f| f.zcl()).is_some_and(|z| z.payload == [0x00]);
    s.run(1000.0)?;
    s.reboot()?;
    s.w.air.ble_tx = 0;
    s.run(4000.0)?;
    let w = single(&mut s, 8, 0x000D)?;
    cx.check(t(&s), "BLE window set to 1 minute (Analog Output, endpoint 8), kept over a power cycle", ok && w == Some(1.0), &format!("{w:?}"));
    s.run(16_000.0)?;
    let early = s.w.air.ble_tx;
    s.run(42_000.0)?;
    s.w.air.ble_tx = 0;
    s.run(30_000.0)?;
    let late = s.w.air.ble_tx;
    let alive = s.alive()?;
    cx.check(t(&s), "BLE advertises in the window and is silent after it; Zigbee goes on", early > 0 && late == 0 && alive,
             &format!("BLE packets: first 20 s {early}, 62-92 s {late}; Zigbee answers: {alive}"));
    s.reboot()?;
    s.w.air.ble_tx = 0;
    s.run(10_000.0)?;
    let again = s.w.air.ble_tx;
    cx.check(t(&s), "a power cycle opens the window again (the way back over BLE)", again > 0, &format!("BLE packets in 10 s: {again}"));
    s.write_with(0x000D, &[(0x0055, 0x39, 0.0f32.to_le_bytes().to_vec())], false, false, Tx::new().ep(8))?;
    let w = single(&mut s, 8, 0x000D)?;
    cx.check(t(&s), "BLE window back to 0 (always on)", w == Some(0.0), &format!("{w:?}"));
    let st = read(&mut s, true, &[0x5043, 0x5044])?;
    match (st[0], st[1]) {
        (Some(main), Some(irq)) => {
            let detail = format!("main stack never used {main} B, interrupt stack {irq} B");
            cx.check(t(&s), "the stacks kept a margin: 4 KB of the main one, 256 B of the interrupt one never used",
                     main >= 4096 && irq >= 256, &detail);
        }
        _ => cx.say("  (stack depth not measured by this image)"),
    }
    Ok(())
}
