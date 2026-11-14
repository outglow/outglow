//! Light command interop with AwoX stock in the real HomeControl app format
//! (emulator/interop_sim.py): the same ZCL-tunnel packet (AES-ECB with the
//! session key, CRC-8/0x6c) is executed by our firmware (BLE=1) and by stock.

use super::ble_central::{discover, fmt_light, login, secure_pw, tel_get, zcl_level, zcl_onoff, Bench, Transport, RND8};
use super::{SimCtx, SimDef};
use crate::air::NodeId;
use crate::image::Image;
use crate::session::{Light, Stop, R};

pub fn def() -> SimDef {
    SimDef { name: "interop", doc: "light commands in the AwoX app format on our firmware and on stock", run }
}

const D: NodeId = NodeId::Dut;

/// The node of an image, linked where the image says: an offset given with
/// it (`--image role=path@0x6a000`), else its ELF's (`Image::shift`), else
/// the prologue heuristic that the Python scripts use (they pass no path).
/// The heuristic alone took some builds of the Rust firmware for linked
/// elsewhere (the same code with another layout came up without BLE), and
/// gives the ELF's offset for the C images and the stock (no ELF) as before.
pub fn node_heuristic(img: &Image, cpi: i64) -> R<zbnode::Node> {
    let bytes = img.bytes().map_err(Stop::Error)?;
    let shift = img.shift(&bytes);
    zbnode::Node::new(&bytes, cpi, shift).map_err(Stop::Error)
}

/// `session()`: boot, connect at 30 ms, discover, log in as "unpaired".
fn session(cx: &SimCtx, img: &Image) -> R<Result<(Bench, u16, [u8; 16]), String>> {
    let node = node_heuristic(img, cx.cpi)?;
    let mut b = Bench::new(node, "dut", false, cx.log());
    b.run_ms(D, 1500.0)?;
    b.ble_connect(D, 24, None);
    let Some(adva) = b.ble_wait_connected(D, 15000.0)? else {
        return Ok(Err(format!("no BLE connection to {}", img.path.display())));
    };
    let mut mac6 = super::ble_central::unhx(&adva);
    mac6.reverse();
    let (_, tel, _) = discover(&mut b, D)?;
    let mut tr = Transport::new(D, &tel);
    let key = match login(&mut b, &mut tr, b"unpaired", &secure_pw(&mac6), &RND8)? {
        Ok(k) => k,
        Err(e) => return Ok(Err(e.text())),
    };
    let h = tel_get(&tel, 0x12).ok_or_else(|| Stop::Error("KeyError: 18".into()))?;
    Ok(Ok((b, h, key)))
}

fn exercise(cx: &SimCtx, img: &Image, label: &str, results: &mut Vec<(String, bool, String)>, tw: bool) -> R<()> {
    let (mut b, h, key) = match session(cx, img)? {
        Ok(x) => x,
        Err(e) => {
            results.push((format!("{label}: connection+login"), false, e));
            return Ok(());
        }
    };
    let tx = |b: &mut Bench, pkt: Vec<u8>| -> R<Light> {
        let mut q = vec![0x12];
        q.extend_from_slice(&h.to_le_bytes());
        q.extend_from_slice(&pkt);
        b.att_ok(D, &q)?;
        b.run(D, 1200.0)?;
        Ok(b.light(D))
    };
    let mut chk = |name: &str, ok: bool, detail: String| results.push((format!("{label}: {name}"), ok, detail));
    let a = tx(&mut b, zcl_onoff(&key, 0x0000, false))?;
    chk("ZCL On/Off off -> light off", !a.on, fmt_light(&a));
    let a = tx(&mut b, zcl_onoff(&key, 0x0000, true))?;
    chk("ZCL On/Off on -> light on", a.on, fmt_light(&a));
    let a = tx(&mut b, zcl_level(&key, 0x0000, 64))?;
    chk("ZCL Level 64 -> dimmed (<15%)", a.on && a.duty < 15.0, fmt_light(&a));
    let a = tx(&mut b, zcl_level(&key, 0x0000, 254))?;
    chk("ZCL Level 254 -> 100%", a.duty > 95.0, fmt_light(&a));
    if tw {
        let a = tx(&mut b, super::ble_central::zcl_color_temp(&key, 0x0000, 153))?;
        chk("ZCL CT 153 (cold) -> PWM5 dominates", a.duty5 > a.duty0, fmt_light(&a));
        let a = tx(&mut b, super::ble_central::zcl_color_temp(&key, 0x0000, 454))?;
        chk("ZCL CT 454 (warm) -> PWM0 dominates", a.duty0 > a.duty5, fmt_light(&a));
    }
    Ok(())
}

fn run(cx: &mut SimCtx) -> R<()> {
    let ours = cx.image("bin", "target/tc32/bulb-tc32.bin");
    let stock = cx.image("stock", "stock/stock-dimm.bin");
    let tw = cx.images.iter().any(|i| i.label == "tw");
    let mut results = Vec::new();
    exercise(cx, &ours, "our firmware", &mut results, tw)?;
    if stock.path.exists() {
        exercise(cx, &stock, "stock", &mut results, false)?;
    } else {
        results.push(("stock available".into(), false, format!("missing {}", stock.path.display())));
    }
    let ok = results.iter().filter(|r| r.1).count();
    for (n, o, d) in &results {
        cx.say(&format!("{}: {n}  {d}", if *o { "PASS" } else { "FAIL" }));
        cx.results.push((n.clone(), *o, d.clone()));
    }
    let wall = cx.t_wall.elapsed().as_secs_f64();
    cx.say(&format!("\n== result: {ok}/{}; time {wall:.0} s", results.len()));
    Ok(())
}
