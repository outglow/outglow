use super::ble_central::{
    discover, fmt_light, hx, ok, secure_pw, tel_get, tel_text, unhx, Bench, MeshClient, Status, Transport, OP_MESH_RESET,
    OP_NOTIFY, OP_POWER, OP_STATUS_REQ, OP_STATUS_RSP, OP_WHITE_BRIGHTNESS, OP_WHITE_TEMPERATURE,
};
use super::interop::node_heuristic;
use super::{SimCtx, SimDef};
use crate::air::NodeId;
use crate::frames::py_bytes;
use crate::py;
use crate::session::{Light, Stop, R};

pub fn def() -> SimDef {
    SimDef { name: "ble_mesh", doc: "our firmware (BLE=1) seen from the AwoX app: advertising, GATT, login, commands, pairing", run }
}

const D: NodeId = NodeId::Dut;
const STOCK_HANDLES: [(u8, u16); 5] = [(0x11, 0x12), (0x12, 0x15), (0x13, 0x18), (0x14, 0x1E), (0x15, 0x1B)];
const DEVINFO_HANDLES: [(u16, &str); 4] = [(0x09, "2a26 fw"), (0x0B, "2a29 mfr"), (0x0D, "2a24 model"), (0x0F, "2a27 hw")];

pub fn parse_adv(adv: &[u8]) -> Vec<(u8, Vec<u8>)> {
    let mut out: Vec<(u8, Vec<u8>)> = Vec::new();
    let mut i = 0;
    while i + 1 < adv.len() {
        let (ln, typ) = (adv[i] as usize, adv[i + 1]);
        if ln == 0 {
            break;
        }
        let val = adv[(i + 2).min(adv.len())..(i + 1 + ln).min(adv.len())].to_vec();
        match out.iter_mut().find(|(t, _)| *t == typ) {
            Some(e) => e.1 = val,
            None => out.push((typ, val)),
        }
        i += 1 + ln;
    }
    out
}

pub fn adv_get(adv: &[(u8, Vec<u8>)], t: u8) -> Option<Vec<u8>> {
    adv.iter().find(|(k, _)| *k == t).map(|(_, v)| v.clone())
}

pub fn adv_repr(adv: &[(u8, Vec<u8>)], t: u8) -> String {
    adv_get(adv, t).map(|v| py_bytes(&v)).unwrap_or_else(|| "None".into())
}

pub fn adv_dict(adv: &[(u8, Vec<u8>)]) -> String {
    format!("{{{}}}", adv.iter().map(|(k, v)| format!("{k}: {}", py_bytes(v))).collect::<Vec<_>>().join(", "))
}

fn st_opt(s: Option<&Status>) -> String {
    s.map(|x| x.repr()).unwrap_or_else(|| "None".into())
}

struct T<'a> {
    cx: &'a mut SimCtx,
}

impl T<'_> {
    fn check(&mut self, name: &str, okv: bool, detail: &str) {
        self.cx.results.push((name.to_string(), okv, detail.to_string()));
        self.cx.say(&format!("{}: {name} {detail}", if okv { "PASS" } else { "FAIL" }));
    }
}

fn connect(b: &mut Bench) -> R<Vec<u8>> {
    b.ble_connect(D, 6, None);
    let Some(adva) = b.ble_wait_connected(D, 15000.0)? else { return Err(Stop::Error("AssertionError: no BLE connection".into())) };
    Ok(unhx(&adva))
}

fn cmd(b: &mut Bench, tr: &mut Transport, cl: &mut MeshClient, op: u8, params: &[u8], wait: f64) -> R<(Light, Vec<Status>)> {
    let n = b.ntfs_len(D);
    ok(cl.send(b, tr, op, params, 0xFFFF)?)?;
    tr.sleep(b, wait)?;
    let got = cl.statuses(&b.ntfs(D)[n..]);
    Ok((b.light(D), got))
}

fn cccd_on(b: &mut Bench, tel: &[(u8, u16)]) -> R<Vec<u8>> {
    let mut q = vec![0x12];
    q.extend_from_slice(&(tel_get(tel, 0x11).unwrap() + 1).to_le_bytes());
    q.extend_from_slice(&[0x01, 0x00]);
    b.att_ok(D, &q)
}

fn run(cx: &mut SimCtx) -> R<()> {
    let img = cx.image("bin", "target/tc32/bulb-tc32.bin");
    let tw = cx.images.iter().any(|i| i.label == "tw");
    let node = node_heuristic(&img, cx.cpi)?;
    let mut b = Bench::new(node, "dut", false, cx.log());
    b.run_ms(D, 1500.0)?;
    let mut t = T { cx };

    if let Some(l) = super::util::stack_rearm(b.w.node_mut(D), &img) {
        t.cx.say(&l);
    }
    let mac_le = connect(&mut b)?;
    let mut mac6 = mac_le.clone();
    mac6.reverse();
    t.cx.say(&format!("   MAC BLE {}", py::hex_sep(&mac6, ":")));

    let adv = parse_adv(&b.ble_adv(D));
    let md = adv_get(&adv, 0xFF).unwrap_or_default();
    let product = if md.len() >= 8 { Some(u16::from_le_bytes([md[2], md[3]])) } else { None };
    let exp_product = if tw { 0x20A6 } else { 0x20A7 };
    let good = adv_get(&adv, 0x01).as_deref() == Some(&[0x05][..])
        && adv_get(&adv, 0x02).as_deref() == Some(&[0x00, 0x18][..])
        && adv_get(&adv, 0x09).as_deref() == Some(&b"unpaired"[..])
        && md.get(..2) == Some(&[0x60, 0x01][..])
        && product == Some(exp_product)
        && md.get(4..8) == mac_le.get(..4);
    let detail = match product {
        Some(p) if p != 0 => format!("name {} product {p:#06x} mfr {}", adv_repr(&adv, 0x09), hx(&md)),
        _ => format!("adv {}", adv_dict(&adv)),
    };
    t.check("AwoX advertising: flags, UUID 0x1800, name, vendor 0x0160, product, MAC", good, &detail);

    let (_, tel, _) = discover(&mut b, D)?;
    t.check("Telink mesh handles like stock", STOCK_HANDLES.iter().all(|(k, v)| tel_get(&tel, *k) == Some(*v)), &tel_text(&tel));
    let mut tr = Transport::new(D, &tel);
    let mut di: Vec<(&str, Option<String>)> = Vec::new();
    for (h, nm) in DEVINFO_HANDLES {
        let mut q = vec![0x0a];
        q.extend_from_slice(&h.to_le_bytes());
        let rsp = b.att_ok(D, &q)?;
        di.push((nm, if rsp[0] == 0x0B { Some(String::from_utf8_lossy(&rsp[1..]).into_owned()) } else { None }));
    }
    let get = |k: &str| di.iter().find(|(n, _)| *n == k).and_then(|(_, v)| v.clone());
    let exp_model = if tw { "ESMLFzm_w6_TW" } else { "ESMLFzm_w6_Dimm" };
    let di_text = format!(
        "{{{}}}",
        di.iter()
            .map(|(k, v)| format!("'{k}': {}", v.as_deref().map(py::str_repr).unwrap_or_else(|| "None".into())))
            .collect::<Vec<_>>()
            .join(", ")
    );
    t.check(
        "Device Information: the vendor's manufacturer, model and hardware revision, the firmware's own version",
        get("2a29 mfr").as_deref() == Some("AwoX")
            && get("2a24 model").as_deref() == Some(exp_model)
            && get("2a26 fw").is_some_and(|v| super::util::firmware_revision_ok(&img, v.as_bytes()))
            && get("2a27 hw").as_deref() == Some("4.63"),
        &di_text,
    );

    let mut cl = MeshClient::new(&mac_le);
    let bad = match cl.login(&mut b, &mut tr, b"unpaired", b"wrong-password")? {
        Ok(_) => "accepted".to_string(),
        Err(super::ble_central::TrErr::Ota(e)) => e,
        Err(e) => return Err(Stop::Error(format!("TimeoutError: {}", e.text()))),
    };
    t.check("wrong password rejected (0x0e)", bad.contains("0e"), &bad);
    let ok1234 = match cl.login(&mut b, &mut tr, b"unpaired", b"1234")? {
        Ok(_) => true,
        Err(super::ble_central::TrErr::Ota(_)) => false,
        Err(e) => return Err(Stop::Error(format!("TimeoutError: {}", e.text()))),
    };
    t.check("login with password '1234' (old apps)", ok1234, "");
    ok(cl.login(&mut b, &mut tr, b"unpaired", &secure_pw(&mac6))?)?;
    t.check("login with the secure password (SHA-256 of the MAC)", cl.key.is_some(), "");

    let rsp = cccd_on(&mut b, &tel)?;
    let n0 = b.ntfs_len(D);
    ok(tr.write(&mut b, 0x11, &[0x01])?)?;
    tr.sleep(&mut b, 400.0)?;
    let ntfs = cl.statuses(&b.ntfs(D)[n0..]);
    let st0 = b.light(D);
    let last = ntfs.last();
    t.check(
        "0xdc notification after enabling status (CCCD + 0x01)",
        rsp[0] == 0x13 && last.is_some_and(|x| x.op == OP_NOTIFY && x.on && x.white_brightness == 127),
        &format!("{}; {}", last.map(|x| x.repr()).unwrap_or_else(|| "none".into()), fmt_light(&st0)),
    );

    let (st, nt) = cmd(&mut b, &mut tr, &mut cl, OP_POWER, &[0], 700.0)?;
    t.check("0xd0 power off -> PWM off, notification on=0", !st.on && nt.last().is_some_and(|x| !x.on), &fmt_light(&st));
    let (st, nt) = cmd(&mut b, &mut tr, &mut cl, OP_POWER, &[1], 700.0)?;
    t.check("0xd0 power on -> PWM on", st.on && nt.last().is_some_and(|x| x.on), &fmt_light(&st));
    let (st, nt) = cmd(&mut b, &mut tr, &mut cl, OP_WHITE_BRIGHTNESS, &[64], 1500.0)?;
    let (lo, hi) = if tw { (17.0, 23.0) } else { (22.0, 28.0) };
    t.check(
        &format!("0xf1 brightness 64/127 -> level 128 ({}% on the stock curve)", if tw { "~20" } else { "~25" }),
        lo < st.duty && st.duty < hi && nt.last().is_some_and(|x| x.white_brightness == 64),
        &format!("{} ntf {}", fmt_light(&st), st_opt(nt.last())),
    );
    let (st, _) = cmd(&mut b, &mut tr, &mut cl, OP_WHITE_BRIGHTNESS, &[127], 1500.0)?;
    t.check("0xf1 brightness 127 -> 100%", st.duty > 99.0, &fmt_light(&st));
    let (_, nt) = cmd(&mut b, &mut tr, &mut cl, OP_STATUS_REQ, &[0x10], 700.0)?;
    t.check(
        "0xda status -> 0xdb response with brightness 127",
        nt.last().is_some_and(|x| x.op == OP_STATUS_RSP && x.white_brightness == 127 && x.on),
        &st_opt(nt.last()),
    );
    if tw {
        let (st, nt) = cmd(&mut b, &mut tr, &mut cl, OP_WHITE_TEMPERATURE, &[0], 1500.0)?;
        t.check(
            "TW 0xf0 temperature 0 -> cold (PWM5 ~100%, PWM0 ~0%)",
            st.duty5 > 95.0 && st.duty0 < 5.0 && nt.last().is_some_and(|x| x.white_temperature == 0),
            &format!("{} ntf {}", fmt_light(&st), st_opt(nt.last())),
        );
        let (st, nt) = cmd(&mut b, &mut tr, &mut cl, OP_WHITE_TEMPERATURE, &[127], 1500.0)?;
        t.check(
            "TW 0xf0 temperature 127 -> warm (PWM0 ~100%, PWM5 ~0%)",
            st.duty0 > 95.0 && st.duty5 < 5.0 && nt.last().is_some_and(|x| x.white_temperature == 127),
            &format!("{} ntf {}", fmt_light(&st), st_opt(nt.last())),
        );
        let (st, _) = cmd(&mut b, &mut tr, &mut cl, OP_WHITE_TEMPERATURE, &[64], 1500.0)?;
        t.check("TW 0xf0 temperature 64 -> mix, sum 100%", 40.0 < st.duty0 && st.duty0 < 60.0 && 98.0 < st.duty && st.duty < 102.0, &fmt_light(&st));
    }

    let good_key = cl.key;
    cl.key = Some([0; 16]);
    let st_before = b.light(D);
    let (st, nt) = cmd(&mut b, &mut tr, &mut cl, OP_POWER, &[0], 700.0)?;
    cl.key = good_key;
    t.check("command with a wrong session key ignored", st.on == st_before.on && nt.is_empty(), &fmt_light(&st));

    for pkt in super::ble_central::set_mesh_packets(&cl.key.unwrap(), b"EgloTest", b"s3cret", b"0123456789abcdef") {
        ok(tr.write(&mut b, 0x14, &pkt)?)?;
        tr.sleep(&mut b, 50.0)?;
    }
    let rsp = ok(tr.read(&mut b, 0x14)?)?;
    t.check("setMesh (0x04/0x05/0x06) -> 0x07", rsp.first() == Some(&0x07), &hx(&rsp));
    b.ble_disconnect(D, 3000.0)?;
    b.run(D, 800.0)?;
    let mac_le2 = connect(&mut b)?;
    let adv = parse_adv(&b.ble_adv(D));
    t.check(
        "after pairing, advertising with the new mesh name",
        adv_get(&adv, 0x09).as_deref() == Some(&b"EgloTest"[..]) && mac_le2 == mac_le,
        &format!("name {}", adv_repr(&adv, 0x09)),
    );
    let (_, tel, _) = discover(&mut b, D)?;
    let mut tr = Transport::new(D, &tel);
    let mut cl = MeshClient::new(&mac_le);
    let old_ok = match cl.login(&mut b, &mut tr, b"unpaired", &secure_pw(&mac6))? {
        Ok(_) => true,
        Err(super::ble_central::TrErr::Ota(_)) => false,
        Err(e) => return Err(Stop::Error(format!("TimeoutError: {}", e.text()))),
    };
    t.check("old password rejected after pairing", !old_ok, "");
    ok(cl.login(&mut b, &mut tr, b"EgloTest", b"s3cret")?)?;
    cccd_on(&mut b, &tel)?;
    ok(tr.write(&mut b, 0x11, &[0x01])?)?;
    tr.sleep(&mut b, 300.0)?;
    let (st, _) = cmd(&mut b, &mut tr, &mut cl, OP_POWER, &[0], 700.0)?;
    t.check("after pairing: login with the new name/password and a command work", !st.on, &fmt_light(&st));
    cmd(&mut b, &mut tr, &mut cl, OP_POWER, &[1], 700.0)?;

    ok(cl.send(&mut b, &mut tr, OP_MESH_RESET, &[0], 0xFFFF)?)?;
    let end = b.t(D) + 3000.0;
    while b.t(D) < end && b.ble_state(D).0 == "connected" {
        b.run(D, 20.0)?;
    }
    let stt = b.ble_state(D).0;
    t.check("0xe3 mesh reset -> device disconnects", stt != "connected", &format!("state {stt}"));
    b.run(D, 800.0)?;
    connect(&mut b)?;
    let adv = parse_adv(&b.ble_adv(D));
    let (_, tel, _) = discover(&mut b, D)?;
    let mut tr = Transport::new(D, &tel);
    let mut cl = MeshClient::new(&mac_le);
    let fresh = match cl.login(&mut b, &mut tr, b"unpaired", &secure_pw(&mac6))? {
        Ok(_) => true,
        Err(super::ble_central::TrErr::Ota(_)) => false,
        Err(e) => return Err(Stop::Error(format!("TimeoutError: {}", e.text()))),
    };
    t.check(
        "after reset: name 'unpaired' and the secure password work",
        fresh && adv_get(&adv, 0x09).as_deref() == Some(&b"unpaired"[..]),
        &format!("name {}", adv_repr(&adv, 0x09)),
    );

    let cx = t.cx;
    cx.say(&format!("   firmware resets: {}", b.resets(D)));
    if let Some(l) = super::util::stack_report(b.w.node(D), &img) {
        cx.say(&l);
    }
    let okn = cx.results.iter().filter(|r| r.1).count();
    let wall = cx.t_wall.elapsed().as_secs_f64();
    cx.say(&format!("\n== result: {okn}/{}; wall time {wall:.0} s", cx.results.len()));
    let rs = cx.results.clone();
    for (n, o, d) in rs {
        cx.say(&format!("  {}  {n}  {d}", if o { "PASS" } else { "FAIL" }));
    }
    Ok(())
}
