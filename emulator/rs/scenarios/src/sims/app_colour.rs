//! Colour as the AwoX HomeControl app drives it (decompiled app, docs/stock-colour.md):
//! the ZCL tunnel on 1912 for colour (Move to Hue and Saturation), brightness,
//! colour temperature and the colour animation (Enhanced Move Hue / Stop Move
//! Step), and the status the app asks for (`value[0]=2`, `[0x0a addr]`) and
//! reads from the 0x80 notifications on 1911 (flags, level, mireds, hue,
//! saturation). The same packets go to the stock colour image and to ours;
//! the outputs and the status after each are compared.
//!
//! Not in the script: Move to Color through the tunnel. The app does not
//! send it; the stock firmware takes xy to hue and saturation only in the
//! routine that saves the Zigbee attributes, which the tunnel does not
//! start, so there an xy after another mode shows the old hue
//! (docs/stock-colour.md).
//!
//! `cargo sims -- app_colour [-v] [--image stock=...] [--image rust=...]`

use super::ble_central::{crc8_6c, discover, hx, login, pad16, secure_pw, tel_get, unhx, zcl_tunnel_packet, Bench, Transport, RND8};
use super::{SimCtx, SimDef};
use crate::air::NodeId;
use crate::crypto::telink_aes;
use crate::image::Image;
use crate::session::{Stop, R};

pub fn def() -> SimDef {
    SimDef { name: "app_colour", doc: "colour in the AwoX app's protocol on the stock colour lamp and on ours", run }
}

const D: NodeId = NodeId::Dut;

/// Telink's aes_att_decryption: AES-128 decryption on the reversed bytes.
fn telink_aes_dec(key: &[u8; 16], data: &[u8]) -> [u8; 16] {
    let mut k = *key;
    k.reverse();
    let mut d: [u8; 16] = data[..16].try_into().unwrap();
    d.reverse();
    let mut o = tc32emu::aes128::decrypt(&k, &d);
    o.reverse();
    o
}

/// The app's status request (`get_status_notification`, i9.f): type 2, [0x0a addr LE].
fn status_request(key: &[u8; 16], addr: u16) -> Vec<u8> {
    let body = [3u8, 0x0a, addr as u8, (addr >> 8) as u8];
    let mut blk = vec![crc8_6c(&body)];
    blk.extend_from_slice(&body);
    let mut v = vec![2u8];
    v.extend_from_slice(&telink_aes(key, &pad16(&blk)));
    v
}

/// The 19 octets of an app status (0x80, one AES block, three in the clear), or None.
fn app_status(key: &[u8; 16], v: &[u8]) -> Option<[u8; 19]> {
    if v.len() != 20 || v[0] != 0x80 {
        return None;
    }
    let mut d = [0u8; 19];
    d[..16].copy_from_slice(&telink_aes_dec(key, &v[1..17]));
    d[16..].copy_from_slice(&v[17..20]);
    Some(d)
}

/// What the app makes of it (n7.k): flags, level, mireds, hue, saturation.
fn show(d: &[u8; 19]) -> String {
    format!(
        "on {} colour {} turning {} candle {} level {} mireds {} hue {} sat {}",
        d[12] & 1,
        d[12] >> 1 & 1,
        d[12] >> 2 & 1,
        d[12] >> 4 & 1,
        d[13],
        u16::from_le_bytes([d[14], d[15]]),
        d[16],
        d[17]
    )
}

/// The compare values of the six PWM channels (0 when off).
fn pwm(b: &Bench) -> [u16; 6] {
    let g = &b.node(D).m.regs;
    let p = &g[0x780..0x7c0];
    let mut out = [0u16; 6];
    for (i, o) in out.iter_mut().enumerate() {
        let en = if i == 0 { p[1] & 1 } else { (p[0] >> i) & 1 };
        let at = 0x14 + 4 * i;
        *o = if en != 0 { u16::from_le_bytes([p[at], p[at + 1]]) } else { 0 };
    }
    out
}

/// The outcome of one step: the outputs once it settled, the last app status, and how many
/// app statuses and other notifications came.
struct Step {
    what: &'static str,
    pwm: [u16; 6],
    status: Option<[u8; 19]>,
    statuses: usize,
    other: usize,
}

/// The app's packets, in order: (what, tunnel cluster and ZCL, or the status request; ms to wait).
enum Pkt {
    Zcl(u16, &'static [u8]),
    Status,
}

const SCRIPT: &[(&str, Pkt, f64)] = &[
    ("status request", Pkt::Status, 800.0),
    ("on", Pkt::Zcl(0x0006, &[1]), 800.0),
    ("colour red (hue 0 sat 254)", Pkt::Zcl(0x0300, &[0x06, 0, 254, 2, 0]), 1200.0),
    ("colour green (hue 84 sat 254)", Pkt::Zcl(0x0300, &[0x06, 84, 254, 2, 0]), 1200.0),
    ("colour brightness 50 %", Pkt::Zcl(0x0008, &[0x04, 128, 2, 0, 0, 0, 0]), 1200.0),
    ("status request", Pkt::Status, 800.0),
    ("animation on (Enhanced Move Hue up 0x0C00)", Pkt::Zcl(0x0300, &[0x41, 1, 0x00, 0x0C]), 6300.0),
    ("status request (animating)", Pkt::Status, 800.0),
    ("animation off (Stop Move Step)", Pkt::Zcl(0x0300, &[0x47, 0, 0, 0]), 1200.0),
    ("status request", Pkt::Status, 800.0),
    ("white 50 % (CT 303)", Pkt::Zcl(0x0300, &[0x0a, 0x2f, 0x01, 2, 0]), 1200.0),
    ("candle mode on", Pkt::Zcl(0x0008, &[0x10, 1, 1]), 1500.0),
    ("candle mode off", Pkt::Zcl(0x0008, &[0x10, 0, 1]), 1500.0),
    ("off", Pkt::Zcl(0x0006, &[0]), 1500.0),
    ("status request (off)", Pkt::Status, 800.0),
    ("idle 10 s", Pkt::Status, 10000.0),
    ("on", Pkt::Zcl(0x0006, &[1]), 1500.0),
];

fn exercise(cx: &mut SimCtx, img: &Image, label: &str) -> R<Vec<Step>> {
    let node = super::interop::node_heuristic(img, cx.cpi)?;
    let mut b = Bench::new(node, "dut", false, cx.log());
    b.run_ms(D, 1500.0)?;
    b.ble_connect(D, 24, None);
    let Some(adva) = b.ble_wait_connected(D, 15000.0)? else {
        return Err(Stop::Error(format!("{label}: no BLE connection")));
    };
    let mut mac6 = unhx(&adva);
    mac6.reverse();
    let (_, tel, _) = discover(&mut b, D)?;
    let mut tr = Transport::new(D, &tel);
    let key = match login(&mut b, &mut tr, b"unpaired", &secure_pw(&mac6), &RND8)? {
        Ok(k) => k,
        Err(e) => return Err(Stop::Error(format!("{label}: login {}", e.text()))),
    };
    let hc = tel_get(&tel, 0x12).unwrap();
    let hs = tel_get(&tel, 0x11).unwrap();
    // the app's notification switch, and the CCCD
    let mut q = vec![0x12];
    q.extend_from_slice(&hs.to_le_bytes());
    q.push(0x01);
    b.att_ok(D, &q)?;
    let mut q = vec![0x12];
    q.extend_from_slice(&(hs + 1).to_le_bytes());
    q.extend_from_slice(&[0x01, 0x00]);
    let _ = b.att(D, &q, 2000.0, 2.0)?;
    b.run(D, 500.0)?;
    let mut seen = b.ntfs_len(D);
    let mut steps = Vec::new();
    for (what, pkt, ms) in SCRIPT {
        let v = match pkt {
            Pkt::Zcl(cluster, zcl) => zcl_tunnel_packet(&key, 0x0000, *cluster, zcl),
            Pkt::Status => status_request(&key, 0x0000),
        };
        let mut q = vec![0x12];
        q.extend_from_slice(&hc.to_le_bytes());
        q.extend_from_slice(&v);
        b.att_ok(D, &q)?;
        b.run(D, *ms)?;
        let n = b.ntfs(D);
        let mut step = Step { what, pwm: pwm(&b), status: None, statuses: 0, other: 0 };
        for (_, h, v) in &n[seen..] {
            let v = unhx(&v.replace('_', ""));
            match app_status(&key, &v) {
                Some(d) if *h as u16 == hs => {
                    step.status = Some(d);
                    step.statuses += 1;
                }
                _ => step.other += 1,
            }
        }
        seen = n.len();
        cx.say(&format!(
            "[{label}] {what}: pwm {:?}; {} status(es){}{}",
            step.pwm,
            step.statuses,
            step.status.map(|d| format!(", last: {}", show(&d))).unwrap_or_default(),
            if step.other > 0 { format!(", {} other notification(s)", step.other) } else { String::new() }
        ));
        steps.push(step);
    }
    Ok(steps)
}

fn run(cx: &mut SimCtx) -> R<()> {
    let stock = cx.image("stock", "stock/apk/smartlightz/EGLO_ZM_RGB_TW__4.60.bin");
    let ours = cx.image("rust", "target/tc32-eglo_zm_rgb_tw-4.60/bulb-tc32.bin");
    if !stock.path.exists() {
        return Err(Stop::Error(format!("{} missing: tools/extract-stock-images.sh", stock.path.display())));
    }
    let s = exercise(cx, &stock, "stock")?;
    let r = exercise(cx, &ours, "rust")?;
    let near = |a: u16, b: u16| (a as i32 - b as i32).abs() <= 1;
    for (a, b) in s.iter().zip(&r) {
        let candle = a.what.starts_with("candle");
        let animating = a.what.contains("animating") || a.what.starts_with("animation on");
        // the outputs, settled (a candle flickers; a turning hue is where the clock has it)
        if !candle && !animating {
            cx.check(0.0, &format!("{}: the outputs", a.what), a.pwm.iter().zip(&b.pwm).all(|(x, y)| near(*x, *y)),
                     &format!("stock {:?}, ours {:?}", a.pwm, b.pwm));
        }
        // the status the app reads
        let same = match (a.status, b.status) {
            (Some(x), Some(y)) if candle => x[13..] == y[13..] && x[12] & !0x10 == y[12],
            // the stock firmware sends its status some 90 ms after the command, by when its hue has moved on
            (Some(x), Some(y)) if animating => {
                x[..7] == y[..7] && x[11..16] == y[11..16] && (x[16] as i32 - y[16] as i32).abs() <= 3 && x[17..] == y[17..]
            }
            (x, y) => x.map(|x| [&x[..7], &x[11..]].concat()) == y.map(|y| [&y[..7], &y[11..]].concat()),
        };
        let text = |s: &Step| s.status.map(|d| show(&d)).unwrap_or_else(|| "none".into());
        let name = if candle { format!("{}: the status (apart from the candle bit)", a.what) } else { format!("{}: the status", a.what) };
        cx.check(0.0, &name, same, &format!("stock {}; ours {}", text(a), text(b)));
        cx.check(0.0, &format!("{}: no mesh notification to the app", a.what), b.other == 0, &format!("{} other", b.other));
    }
    let candle_on = &s[SCRIPT.iter().position(|x| x.0 == "candle mode on").unwrap()];
    cx.say(&format!(
        "\nnot done: the candle mode (Level command 0x10 [1, 1]): stock {} and flickers its whites ({:?}); ours keeps the light as it was",
        candle_on.status.map(|d| show(&d)).unwrap_or_default(),
        candle_on.pwm
    ));
    let _ = hx(&[]);
    Ok(())
}
