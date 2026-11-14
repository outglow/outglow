//! Update over BLE in emulation (emulator/ble_ota_sim.py): the original AwoX
//! firmware, the scripted central and our protocol module (awox_ota): login,
//! erase, the whole image with CRC, final status; then where the original
//! wrote the image in flash, which flags it set, what it does after a restart.

use std::cell::RefCell;
use std::rc::Rc;

use super::ble_central::{
    data_packets, discover, end_packet, erase_packet, hex_or_empty, hx, login, secure_pw, tel_get, tel_text, unhx, Bench,
    TrErr, Transport, OTA, OTA_OLD, PAIR, RND8, STATUS, COMMAND,
};
use super::interop::node_heuristic;
use super::{SimCtx, SimDef};
use crate::air::NodeId;
use crate::py;
use crate::session::{Stop, R};

pub fn def() -> SimDef {
    SimDef { name: "ble_ota", doc: "OTA of our image into stock over BLE (awox_ota), where stock stores it", run }
}

const D: NodeId = NodeId::Dut;

/// The script's output in order: its own lines and the engine's log (log=print).
#[derive(Clone, Default)]
struct Out(Rc<RefCell<Vec<String>>>);

impl Out {
    fn say(&self, s: &str) {
        self.0.borrow_mut().push(s.to_string());
    }
}

/// `awox_ota.run_ota`: Ok(final status) or the error text (OtaError / TimeoutError).
fn run_ota(b: &mut Bench, tr: &mut Transport, img: &[u8], out: &Out) -> R<Result<Vec<u8>, String>> {
    let (mtu, read_every) = (23usize, 8usize);
    macro_rules! t {
        ($e:expr) => {
            match $e? {
                Ok(v) => v,
                Err(e) => return Ok(Err(e.text())),
            }
        };
    }
    t!(tr.write(b, OTA, &erase_packet(img.len(), mtu)));
    out.say(&format!("erasing: {} 4K sectors", img.len() / 4096 + 2));
    tr.sleep(b, 5000.0)?;
    let mut waited = 5.0f64;
    loop {
        let rsp = t!(tr.read(b, OTA));
        if rsp.first() == Some(&2) {
            break;
        }
        if waited > 90.0 {
            return Ok(Err(format!("erase did not finish: {}", hex_or_empty(&rsp))));
        }
        tr.sleep(b, 3000.0)?;
        waited += 3.0;
    }
    out.say(&format!("erase finished after ~{waited:.0} s"));
    let total = (img.len() + mtu - 8) / (mtu - 7);
    for (i, pkt) in data_packets(img, mtu) {
        t!(tr.write(b, OTA, &pkt));
        if (i + 1) % read_every == 0 {
            let st = t!(tr.read(b, OTA));
            if st.len() >= 2 && st[1] == 2 {
                return Ok(Err(format!("bulb aborted OTA at packet {i}: {}", hx(&st))));
            }
        }
        if i % 2000 == 0 {
            out.say(&format!("packet {i}/{total}"));
        }
    }
    t!(tr.write(b, OTA, &end_packet(img.len(), mtu)));
    let st = t!(tr.read(b, OTA));
    if st.len() < 2 || st[1] != 0 {
        return Ok(Err(format!("OTA end: status {}", hex_or_empty(&st))));
    }
    out.say("OTA finished successfully (status 0)");
    Ok(Ok(st))
}

fn run(cx: &mut SimCtx) -> R<()> {
    let stock = cx.image("stock", "stock/stock-dimm.bin");
    let image = cx.image("image", "target/tc32/bulb-tc32-ota.bin");
    let no_login = cx.images.iter().any(|i| i.label == "no-login");
    let out = Out::default();
    let o2 = out.clone();
    let node = node_heuristic(&stock, cx.cpi)?;
    let mut b = Bench::new(node, "dut", false, Some(Rc::new(move |s: &str| o2.say(s))));
    b.w.air.py_log = true;
    let mut results: Vec<(String, bool, String)> = Vec::new();
    let mut check = |out: &Out, name: &str, ok: bool, detail: &str| {
        results.push((name.into(), ok, detail.into()));
        out.say(&format!("{}: {name} {detail}", if ok { "PASS" } else { "FAIL" }));
    };
    let finish = |cx: &mut SimCtx, out: &Out, results: &[(String, bool, String)]| {
        for l in out.0.borrow().iter() {
            cx.say(l);
        }
        cx.results.extend(results.iter().cloned());
    };

    b.run_ms(D, 1500.0)?;
    let flash_before = b.flash(D, 0, 0x100000);
    b.ble_connect(D, 6, None);
    let end = b.t(D) + 15000.0;
    let (mut st, mut adva, mut started) = (String::new(), String::new(), false);
    while b.t(D) < end {
        (st, adva, started) = b.ble_state(D);
        if started {
            break;
        }
        b.run(D, 20.0)?;
    }
    check(&out, "BLE connection to the original", started, &format!("state {st}, address {adva}"));
    if !started {
        finish(cx, &out, &results);
        return Ok(());
    }
    let mut mac6 = unhx(&adva);
    mac6.reverse();
    out.say(&format!("   bulb BLE address: {}", py::hex_sep(&mac6, ":")));
    let (_, tel, lines) = discover(&mut b, D)?;
    for l in lines {
        out.say(&l);
    }
    check(&out, "Telink Mesh service (1911..1915)", [0x11, 0x12, 0x14, 0x15].iter().all(|k| tel_get(&tel, *k).is_some()), &tel_text(&tel));
    let hs: Vec<(u8, u16)> = [STATUS, COMMAND, PAIR, OTA, OTA_OLD].iter().filter_map(|k| tel_get(&tel, *k).map(|v| (*k, v))).collect();
    let mut tr = Transport::new(D, &hs);

    let mut logged: Option<&str> = None;
    let cands: Vec<(&str, Vec<u8>)> = if no_login {
        vec![]
    } else {
        vec![("secure (SHA-256 of the BLE MAC)", secure_pw(&mac6)), ("'1234'", b"1234".to_vec())]
    };
    for (label, pw) in &cands {
        // awox_ota.login with log=log: the "logged in" line, from the response read
        let n0 = b.w.tap(D).map(|t| t.rsps.len()).unwrap_or(0);
        match login(&mut b, &mut tr, b"unpaired", pw, &RND8)? {
            Ok(_) => {
                let rsps = &b.w.tap(D).unwrap().rsps;
                let rsp = &rsps[rsps.len() - 1].1;
                let _ = n0;
                out.say(&super::ble_central::login_line(&rsp[1..10.min(rsp.len())]));
                logged = Some(label);
                break;
            }
            Err(TrErr::Ota(e)) => out.say(&format!("   password {label}: {e}")),
            Err(TrErr::Timeout(e)) => return Err(Stop::Error(format!("TimeoutError: {e}"))),
        }
    }
    if no_login {
        out.say("   mesh login SKIPPED (--no-login): OTA goes without a packet on ...1914");
    } else {
        check(&out, "Telink Mesh login", logged.is_some(), &format!("accepted password: {}", logged.unwrap_or("None")));
        if logged.is_none() {
            finish(cx, &out, &results);
            return Ok(());
        }
    }

    let payload = image.bytes().map_err(Stop::Error)?;
    let t0 = b.t(D);
    match run_ota(&mut b, &mut tr, &payload, &out)? {
        Ok(st) => check(
            &out,
            "OTA over 1915: erase, transfer, final status",
            true,
            &format!(
                "{} B, {} writes, {} reads, {:.0} s of simulation, status {}",
                payload.len(),
                tr.writes,
                tr.reads,
                (b.t(D) - t0) / 1000.0,
                hx(&st)
            ),
        ),
        Err(e) => check(&out, "OTA over 1915: erase, transfer, final status", false, &e),
    }

    b.run(D, 3000.0)?;
    let after = b.flash(D, 0, 0x100000);
    let changed: Vec<usize> =
        (0..0x100000).step_by(0x1000).filter(|i| after[*i..*i + 0x1000] != flash_before[*i..*i + 0x1000]).collect();
    let mut rng: Vec<[usize; 2]> = Vec::new();
    for x in changed {
        match rng.last_mut() {
            Some(r) if x == r[1] + 0x1000 => r[1] = x,
            _ => rng.push([x, x]),
        }
    }
    out.say(&format!(
        "   flash sectors changed after OTA: {}",
        rng.iter().map(|r| format!("{:#x}-{:#x}", r[0], r[1] + 0xfff)).collect::<Vec<_>>().join(", ")
    ));
    let found: Vec<usize> =
        (0..0x100000).step_by(0x1000).filter(|i| i + 0x60 <= after.len() && after[i + 0x20..i + 0x60] == payload[0x20..0x60]).collect();
    for base in &found {
        let nd = (0..payload.len()).filter(|i| after.get(base + i) != Some(&payload[*i])).count();
        out.say(&format!(
            "   image found at {base:#x}: differing bytes {nd}, byte8={:#04x}, header {}",
            after[base + 8],
            hx(&after[*base..base + 12])
        ));
    }
    check(&out, "image written to flash", !found.is_empty(), &found.iter().map(|x| format!("{x:#x}")).collect::<Vec<_>>().join(", "));
    let (cf0, cf1) = (&flash_before[0xCF000..0xCF010], &after[0xCF000..0xCF010]);
    out.say(&format!(
        "   cfg sector 0xcf000 before: {}  after: {}  ('new image' flag for the AwoX bootloader)",
        hx(cf0),
        hx(cf1)
    ));
    let cfd: Vec<i64> = (0..0x1000).filter(|i| flash_before[0xCF000 + i] != after[0xCF000 + i]).map(|i| i as i64).collect();
    out.say(&format!("   0xcf000: changed bytes {}, offsets {}", cfd.len(), py::ints(cfd.iter().take(8).copied())));
    out.say(&format!("   firmware resets during and after: {}", b.resets(D)));
    let okn = results.iter().filter(|r| r.1).count();
    let wall = cx.t_wall.elapsed().as_secs_f64();
    out.say(&format!("\n== result: {okn}/{}; wall time {wall:.0} s", results.len()));
    for (n, o, d) in &results {
        out.say(&format!("  {}  {n}  {d}", if *o { "PASS" } else { "FAIL" }));
    }
    finish(cx, &out, &results);
    Ok(())
}
