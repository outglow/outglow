use super::ble_central::{data_packets, discover, end_packet, erase_packet, hx, Bench, Transport, OTA};
use super::{node_of, SimCtx, SimDef};
use crate::air::NodeId;
use crate::session::{Stop, R};
use crate::station::nx;

pub fn def() -> SimDef {
    SimDef { name: "rust_ble_switch", doc: "Zigbee switches BLE off, on and to a window after power-on (Rust firmware)", run }
}

const D: NodeId = NodeId::Dut;
const EP: u8 = 1;
const SETTINGS: u16 = 0xFC5F;
const CODE: u16 = 0x1141;
const T_ENUM8: u8 = 0x30;
const T_U16: u8 = 0x21;

fn until(b: &mut Bench, max_ms: f64, step: f64, mut cond: impl FnMut(&Bench) -> bool) -> R<bool> {
    let end = b.t(D) + max_ms;
    while b.t(D) < end && !cond(b) {
        b.run(D, step)?;
    }
    Ok(cond(b))
}

fn settings_cmd(b: &mut Bench, short: u16, cmd: u8, resp: u8, payload: &[u8], code: Option<u16>) -> R<Option<Vec<u8>>> {
    let c = &mut b.w.coord;
    let seq = nx(&mut c.st.zcl_seq);
    let mut aps = vec![0x40, EP];
    aps.extend_from_slice(&SETTINGS.to_le_bytes());
    aps.extend_from_slice(&0x0104u16.to_le_bytes());
    aps.extend_from_slice(&[0x01, nx(&mut c.st.aps_ctr)]);
    match code {
        Some(code) => {
            aps.push(0x04);
            aps.extend_from_slice(&code.to_le_bytes());
        }
        None => aps.push(0x00),
    }
    aps.extend_from_slice(&[seq, cmd]);
    aps.extend_from_slice(payload);
    let n0 = c.zcl_rsp.len();
    c.nwk_send_data(&mut b.w.air, short, &aps);
    let got = |b: &Bench| b.w.coord.zcl_rsp[n0..].iter().find(|r| r.cluster == SETTINGS && r.cmd == Some(resp)).map(|r| r.body.clone());
    until(b, 1500.0, 5.0, |b| got(b).is_some())?;
    Ok(got(b))
}

fn read(b: &mut Bench, short: u16, attr: u16) -> R<Option<Vec<u8>>> {
    let rsp = settings_cmd(b, short, 0x00, 0x01, &attr.to_le_bytes(), Some(CODE))?;
    Ok(rsp.filter(|p| p.len() >= 5 && p[2] == 0).map(|p| p[4..].to_vec()))
}

fn write(b: &mut Bench, short: u16, attr: u16, ty: u8, value: &[u8], code: Option<u16>) -> R<Option<Vec<u8>>> {
    let mut p = attr.to_le_bytes().to_vec();
    p.push(ty);
    p.extend_from_slice(value);
    settings_cmd(b, short, 0x02, 0x04, &p, code)
}

#[derive(Clone, Copy)]
struct Mark(usize);

fn mark(b: &Bench) -> Mark {
    Mark(b.ble_rf(D, f64::NEG_INFINITY).1.len())
}

fn adverts(b: &Bench, m: Mark, t0: f64) -> usize {
    adverts_between(b, m, t0, f64::INFINITY)
}

fn adverts_between(b: &Bench, m: Mark, t0: f64, t1: f64) -> usize {
    let off = b.w.air.t_off;
    let rf = b.ble_rf(D, f64::NEG_INFINITY).1;
    rf[m.0.min(rf.len())..].iter().filter(|(t, ch, pdu)| *t >= t0 - off && *t < t1 - off && *ch == 37 && pdu[0] & 0x0F == 0).count()
}

fn show(v: &Option<Vec<u8>>) -> String {
    v.as_ref().map(|v| hx(v)).unwrap_or_else(|| "no answer".into())
}

fn basic_answers(b: &mut Bench, short: u16, n: usize) -> R<Vec<Option<f64>>> {
    let mut out = Vec::new();
    for _ in 0..n {
        let n0 = b.w.coord.zcl_rsp.len();
        let t0 = b.t(D);
        b.w.coord.read_attrs(&mut b.w.air, short, EP, 0x0000, &[0x0004]);
        let got = until(b, 1500.0, 5.0, |b| b.w.coord.zcl_rsp[n0..].iter().any(|r| r.cluster == 0x0000 && r.cmd == Some(0x01)))?;
        out.push(got.then(|| b.t(D) - t0));
        b.run(D, 150.0)?;
    }
    Ok(out)
}

fn run(cx: &mut SimCtx) -> R<()> {
    let img = cx.image("bin", "target/tc32/bulb-tc32.bin");
    let node = node_of(&img, cx.cpi)?;
    let mut b = Bench::new(node, "dut", true, None);
    let mut results: Vec<bool> = Vec::new();
    let mut check = |cx: &mut SimCtx, name: &str, okv: bool, detail: &str| -> bool {
        results.push(okv);
        cx.results.push((name.into(), okv, detail.into()));
        cx.say(&format!("  {}  {name}{}", if okv { "PASS" } else { "FAIL" }, if detail.is_empty() { String::new() } else { format!("  {detail}") }));
        okv
    };

    let joined = until(&mut b, 60000.0, 20.0, |b| b.w.coord.events.iter().any(|e| e.kind == "device-announce"))?;
    check(cx, "joins the Zigbee network", joined, &format!("after {:.1} s", b.t(D) / 1000.0));
    if !joined {
        return Ok(());
    }
    let short = b.w.coord.devices.iter().find(|d| d.announced).unwrap().short;
    let mut m = Mark(0);
    b.run(D, 3000.0)?;
    let answers_on = basic_answers(&mut b, short, 10)?;

    let t0 = b.t(D);
    b.run(D, 3000.0)?;
    let n = adverts(&b, m, t0);
    let (mode, window, active) = (read(&mut b, short, 0x0000)?, read(&mut b, short, 0x0001)?, read(&mut b, short, 0x0002)?);
    check(cx, "out of the box: mode 0 (always), window 20 min, BLE running", mode == Some(vec![0]) && window == Some(vec![20, 0]) && active == Some(vec![1]),
          &format!("mode {}, window {}, running {}", show(&mode), show(&window), show(&active)));
    check(cx, "and the bulb advertises", n >= 8, &format!("{n} advertising events in 3 s"));

    let w = write(&mut b, short, 0x0000, T_ENUM8, &[2], Some(CODE))?;
    check(cx, "mode 2 (off) written with the manufacturer code", w == Some(vec![0]), &show(&w));
    b.run(D, 300.0)?;
    let t0 = b.t(D);
    b.run(D, 5000.0)?;
    let n = adverts(&b, m, t0);
    check(cx, "BLE off: no advertising for 5 s", n == 0, &format!("{n} advertising events"));
    let active = read(&mut b, short, 0x0002)?;
    check(cx, "attribute 0x0002 says BLE is not running", active == Some(vec![0]), &show(&active));
    let answers = basic_answers(&mut b, short, 10)?;
    let okn = answers.iter().filter(|a| a.is_some()).count();
    let fmt = |v: &[Option<f64>]| v.iter().map(|a| a.map_or("-".into(), |t| format!("{t:.0}"))).collect::<Vec<_>>().join(" ");
    check(cx, "Zigbee goes on", okn == 10, &format!("{okn}/10 reads answered (ms): {}; with BLE on before: {}", fmt(&answers), fmt(&answers_on)));

    let w = write(&mut b, short, 0x0000, T_ENUM8, &[0], None)?;
    let t0 = b.t(D);
    b.run(D, 3000.0)?;
    let n = adverts(&b, m, t0);
    check(cx, "mode 0 written without the code: advertising again", w == Some(vec![0]) && n >= 8, &format!("write {}, {n} advertising events in 3 s", show(&w)));

    b.ble_connect(D, 24, Some(1600.0));
    let adva = b.ble_wait_connected(D, 15000.0)?;
    check(cx, "a central connects", adva.is_some(), &adva.clone().unwrap_or_default());
    if adva.is_some() {
        let rsp = b.att_ok(D, &[0x0a, 0x0b, 0x00])?;
        let w = write(&mut b, short, 0x0000, T_ENUM8, &[2], Some(CODE))?;
        b.run(D, 200.0)?;
        let s0 = b.ble_stats(D);
        b.run(D, 3000.0)?;
        let s1 = b.ble_stats(D);
        let (events, missed) = (s1.events - s0.events, s1.missed - s0.missed);
        check(cx, "mode 2 during the connection: the bulb leaves it, no connection event is answered",
              rsp == b"\x0bAwoX" && w == Some(vec![0]) && events > 50 && missed == events,
              &format!("{missed} of {events} events unanswered"));
        let t0 = b.t(D);
        b.run(D, 3000.0)?;
        let n = adverts(&b, m, t0);
        check(cx, "and nothing is advertised after it", n == 0, &format!("{n} advertising events in 3 s"));
    }

    write(&mut b, short, 0x0000, T_ENUM8, &[0], Some(CODE))?;
    b.run(D, 1000.0)?;
    b.ble_connect(D, 24, Some(1600.0));
    let adva = b.ble_wait_connected(D, 15000.0)?;
    if adva.is_some() {
        let (_, tel, _) = discover(&mut b, D)?;
        let mut tr = Transport::new(D, &tel);
        let mut fake = vec![0u8; 8192];
        fake[8..12].copy_from_slice(b"KNLT");
        let declared = (fake.len() as u32).to_le_bytes();
        fake[0x18..0x1C].copy_from_slice(&declared);
        let mtu = 23;
        let mut sent = 0;
        let mut aborted = None;
        let mut status = Vec::new();
        let r: Result<(), String> = (|| -> R<Result<(), String>> {
            if let Err(e) = tr.write(&mut b, OTA, &erase_packet(fake.len(), mtu))? {
                return Ok(Err(e.text()));
            }
            tr.sleep(&mut b, 3000.0)?;
            let pkts = data_packets(&fake, mtu);
            let half = pkts.len() / 2;
            for (i, pkt) in pkts {
                if i == half {
                    let w = write(&mut b, short, 0x0000, T_ENUM8, &[2], Some(CODE))?;
                    if w != Some(vec![0]) {
                        return Ok(Err(format!("write {}", show(&w))));
                    }
                }
                if let Err(e) = tr.write(&mut b, OTA, &pkt)? {
                    aborted = Some(i);
                    return Ok(Err(e.text()));
                }
                sent += 1;
            }
            if let Err(e) = tr.write(&mut b, OTA, &end_packet(fake.len(), mtu))? {
                return Ok(Err(e.text()));
            }
            match tr.read(&mut b, OTA)? {
                Ok(st) => status = st,
                Err(e) => return Ok(Err(e.text())),
            }
            Ok(Ok(()))
        })()?;
        let total = fake.len().div_ceil(mtu - 7);
        check(cx, "BLE off asked for in the middle of an update over BLE: the transfer runs to its end",
              r.is_ok() && aborted.is_none() && sent == total && status.first() == Some(&4),
              &format!("{sent}/{total} packets, end status {}{}", hx(&status), r.err().map(|e| format!(", {e}")).unwrap_or_default()));
        let still = b.ble_state(D).0 == "connected";
        let st = b.ble_disconnect(D, 3000.0)?;
        let t0 = b.t(D);
        b.run(D, 3000.0)?;
        let n = adverts(&b, m, t0);
        check(cx, "BLE stays for the connection of the update, and stops when it ends", still && st.starts_with("terminat") && n == 0,
              &format!("central {st}, {n} advertising events in 3 s after it"));
    } else {
        check(cx, "a central connects for the update", false, "");
    }

    let w1 = write(&mut b, short, 0x0001, T_U16, &[1, 0], Some(CODE))?;
    let w2 = write(&mut b, short, 0x0000, T_ENUM8, &[1], Some(CODE))?;
    b.w.reboot().map_err(Stop::Error)?;
    m = mark(&b);
    let boot = b.t(D);
    b.run(D, 1500.0)?;
    let t0 = b.t(D);
    b.run(D, 3000.0)?;
    let n = adverts(&b, m, t0);
    check(cx, "window 1 min, power cycle: advertising right after power-on", w1 == Some(vec![0]) && w2 == Some(vec![0]) && n >= 8,
          &format!("{n} advertising events in 3 s"));
    let (mode, window, active) = (read(&mut b, short, 0x0000)?, read(&mut b, short, 0x0001)?, read(&mut b, short, 0x0002)?);
    check(cx, "the setting outlives the power cycle; BLE runs inside the window",
          mode == Some(vec![1]) && window == Some(vec![1, 0]) && active == Some(vec![1]),
          &format!("mode {}, window {}, running {}", show(&mode), show(&window), show(&active)));
    b.run_ms(D, boot + 60_500.0)?;
    let inside = adverts_between(&b, m, boot + 55_000.0, boot + 59_500.0);
    let t0 = b.t(D);
    b.run(D, 5000.0)?;
    let n = adverts(&b, m, t0);
    let active = read(&mut b, short, 0x0002)?;
    check(cx, "still advertising at 55 to 60 s after power-on", inside >= 8, &format!("{inside} advertising events"));
    check(cx, "a minute after power-on: no advertising, attribute 0x0002 reads 0", n == 0 && active == Some(vec![0]),
          &format!("{n} advertising events from {:.1} s after power-on, running {}", (t0 - boot) / 1000.0, show(&active)));

    write(&mut b, short, 0x0000, T_ENUM8, &[2], Some(CODE))?;
    b.run(D, 1000.0)?;
    let mut during = 0;
    for s in [1.0, 1.0, 1.0, 12.0, 12.0] {
        b.w.reboot().map_err(Stop::Error)?;
    m = mark(&b);
        let t0 = b.t(D);
        b.run(D, s * 1000.0)?;
        during += adverts(&b, m, t0);
    }
    check(cx, "mode 2: nothing advertised over five power-ons", during == 0, &format!("{during} advertising events"));
    b.w.reboot().map_err(Stop::Error)?;
    m = mark(&b);
    b.run(D, 1500.0)?;
    let t0 = b.t(D);
    b.run(D, 3000.0)?;
    let n = adverts(&b, m, t0);
    check(cx, "the sixth power-on (factory reset sequence 1, 1, 1, 12, 12 s): BLE is back", n >= 8, &format!("{n} advertising events in 3 s"));

    let total = results.len();
    let passed = results.iter().filter(|x| **x).count();
    cx.say(&format!("== {passed}/{total} passed"));
    Ok(())
}
