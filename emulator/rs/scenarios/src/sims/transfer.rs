//! A bulb moved between the stock firmware and the Rust firmware, both ways,
//! the ways the README describes: the first install over BLE from the stock
//! firmware, and back to the stock firmware over Zigbee OTA or BLE from the
//! Rust firmware.
//!
//! Each update ends with the image staged at 0x6000 and marked bootable (0x4B
//! at +8); the AwoX bootloader at 0x0 then installs it at 0x6A000. Nobody has
//! that bootloader (the chip's debug port is locked), so the emulator does not
//! run it: `install_staged` does what it is inferred to do (both firmwares
//! are linked at 0x6A000 and validated by KNLT, size and CRC only). That step
//! is the one thing here not taken from a firmware; the rest is the two
//! firmwares as they are.
//!
//! Images: `--image stock=...` (default stock/stock-dimm.bin), `--image
//! rust=...` (default target/tc32/bulb-tc32.bin) and its BLE update image
//! `--image rust-ota=...` (default target/tc32/bulb-tc32-ota.bin).

use super::ble_central::{data_packets, discover, end_packet, erase_packet, hex_or_empty, hx, tel_get, Bench, Transport, OTA};
use super::mixed::{ieee_of, mac_record, Fw, SETTLE_MS};
use super::ota::{body as ota_body, Layout};
use super::util::{hexo, rel};
use super::zigbee::{decode_attrs, finish as z_finish, fmt_light, Z};
use super::{node_of, SimCtx, SimDef};
use crate::air::NodeId;
use crate::coord::EP;
use crate::session::{light_of, Stop, R};

pub fn defs() -> Vec<SimDef> {
    vec![
        SimDef { name: "migrate", doc: "stock on the network, the Rust image over BLE OTA, installed, power cycle: it joins again", run: migrate },
        SimDef { name: "back_to_stock_ota", doc: "back to the stock firmware over Zigbee OTA (the stock image as a .ota of the Rust image type)", run: back_to_stock_ota },
        SimDef { name: "back_to_stock_ble", doc: "back to the stock firmware over the Rust firmware's BLE OTA", run: back_to_stock_ble },
    ]
}

const D: NodeId = NodeId::Dut;
const STAGING: usize = 0x6000;
const APP: usize = 0x6A000;
const MAC: [u8; 5] = [0x31, 0x00, 0x56, 0x00, 0x00];

/// What the AwoX bootloader is taken to do at the next start with an image
/// staged at 0x6000 and marked bootable: copy it to 0x6A000 and start it.
/// The emulator starts every image from 0 (and runs the rest from its link
/// address, 0x6A000), so the image is written at 0 too, as it is when a node
/// is made from an image. The staged image (KNLT, size at 0x18).
pub fn install_staged(node: &mut zbnode::Node) -> Result<Vec<u8>, String> {
    let f = &node.m.flash.mem;
    if f[STAGING + 8] != 0x4B || &f[STAGING + 9..STAGING + 12] != b"NLT" {
        return Err(format!("no bootable image staged at 0x6000: {}", hx(&f[STAGING..STAGING + 12])));
    }
    let size = u32::from_le_bytes(f[STAGING + 0x18..STAGING + 0x1C].try_into().unwrap()) as usize;
    if !(0x40..=0x64000).contains(&size) {
        return Err(format!("staged image size {size:#x}"));
    }
    let img = f[STAGING..STAGING + size].to_vec();
    node.write_flash(APP, &img);
    node.write_flash(0, &img);
    Ok(img)
}

/// A Zigbee OTA file (header 0x0BEEF11E, one sub-element of tag 0) of an image.
pub fn ota_file(mfg: u16, image_type: u16, version: u32, header: &str, image: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&0x0BEEF11Eu32.to_le_bytes());
    v.extend_from_slice(&0x0100u16.to_le_bytes());
    v.extend_from_slice(&56u16.to_le_bytes());
    v.extend_from_slice(&0u16.to_le_bytes());
    v.extend_from_slice(&mfg.to_le_bytes());
    v.extend_from_slice(&image_type.to_le_bytes());
    v.extend_from_slice(&version.to_le_bytes());
    v.extend_from_slice(&2u16.to_le_bytes());
    let mut h = [0u8; 32];
    h[..header.len().min(32)].copy_from_slice(&header.as_bytes()[..header.len().min(32)]);
    v.extend_from_slice(&h);
    v.extend_from_slice(&((56 + 6 + image.len()) as u32).to_le_bytes());
    v.extend_from_slice(&0u16.to_le_bytes());
    v.extend_from_slice(&(image.len() as u32).to_le_bytes());
    v.extend_from_slice(image);
    v
}

/// The AwoX BLE update over ...1915 (`awox_ota.run_ota`, no mesh login: neither
/// firmware asks for one there): erase, every block, the end packet; Ok(final
/// status) or the error.
pub fn ble_push(b: &mut Bench, tr: &mut Transport, img: &[u8], say: &mut dyn FnMut(String)) -> R<Result<Vec<u8>, String>> {
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
    tr.sleep(b, 5000.0)?;
    let mut waited = 5.0f64;
    loop {
        let rsp = t!(tr.read(b, OTA));
        if rsp.first() == Some(&2) || rsp.first() == Some(&0) {
            break;
        }
        if waited > 90.0 {
            return Ok(Err(format!("erase did not finish: {}", hex_or_empty(&rsp))));
        }
        tr.sleep(b, 3000.0)?;
        waited += 3.0;
    }
    say(format!("   erase finished after ~{waited:.0} s"));
    for (i, pkt) in data_packets(img, mtu) {
        t!(tr.write(b, OTA, &pkt));
        if (i + 1) % read_every == 0 {
            let st = t!(tr.read(b, OTA));
            if st.len() >= 2 && st[1] == 2 {
                return Ok(Err(format!("the bulb aborted at packet {i}: {}", hx(&st))));
            }
        }
    }
    t!(tr.write(b, OTA, &end_packet(img.len(), mtu)));
    let st = t!(tr.read(b, OTA));
    if st.len() < 2 || st[1] != 0 {
        return Ok(Err(format!("end: status {}", hex_or_empty(&st))));
    }
    Ok(Ok(st))
}

/// Connects the central and finds the OTA characteristic.
fn ble_open(b: &mut Bench) -> R<Option<Transport>> {
    b.ble_connect(D, 24, None);
    if b.ble_wait_connected(D, 15000.0)?.is_none() {
        return Ok(None);
    }
    let (_, tel, _) = discover(b, D)?;
    Ok(tel_get(&tel, OTA).map(|h| Transport::new(D, &[(OTA, h)])))
}

/// A scenario's checks, printed as the others print them.
struct Run<'a> {
    cx: &'a mut SimCtx,
}

impl Run<'_> {
    fn check(&mut self, t: f64, name: &str, ok: bool, detail: &str) -> bool {
        self.cx.check(t, name, ok, detail)
    }
    fn say(&mut self, s: &str) {
        self.cx.say(s);
    }
}

fn until(b: &mut Bench, max_ms: f64, mut cond: impl FnMut(&Bench) -> bool) -> R<bool> {
    let end = b.t(D) + max_ms;
    while b.t(D) < end && !cond(b) {
        b.run(D, 20.0)?;
    }
    Ok(cond(b))
}

fn announced(b: &Bench, ext: u64, since: usize) -> Option<u16> {
    b.w.coord.events[since..]
        .iter()
        .filter(|e| e.kind == "device-announce")
        .find(|e| e.get("ext") == Some(crate::frames::fmt_ext(ext).as_str()))
        .and_then(|e| e.get("short").and_then(|s| u16::from_str_radix(s.trim_start_matches("0x"), 16).ok()))
}

/// A cluster command and the bulb's Default Response; the light once settled.
fn light_cmd(b: &mut Bench, short: u16, cluster: u16, cmd: u8, payload: &[u8]) -> R<(bool, crate::session::Light)> {
    let n0 = b.w.coord.zcl_rsp.len();
    let air = &mut b.w.air;
    b.w.coord.zcl_cmd(air, short, EP, cluster, cmd, payload);
    let got = until(b, 3000.0, |b| b.w.coord.zcl_rsp[n0..].iter().any(|r| r.src == short && r.cluster == cluster && r.cmd == Some(0x0B)))?;
    b.run(D, SETTLE_MS)?;
    Ok((got, b.light(D)))
}

/// Basic: manufacturer name, model identifier, software build.
fn basic(b: &mut Bench, short: u16) -> R<Option<String>> {
    let n0 = b.w.coord.zcl_rsp.len();
    let air = &mut b.w.air;
    b.w.coord.read_attrs(air, short, EP, 0x0000, &[0x0004, 0x0005, 0x4000]);
    until(b, 4000.0, |b| b.w.coord.zcl_rsp[n0..].iter().any(|r| r.src == short && r.cluster == 0x0000 && r.cmd == Some(0x01)))?;
    Ok(b.w.coord.zcl_rsp[n0..].iter().find(|r| r.src == short && r.cluster == 0x0000 && r.cmd == Some(0x01)).map(|r| decode_attrs(&r.body)))
}

/// After a firmware change: power cycle, joins (again), Basic, Off/On/Level.
/// `model` judges the Basic attributes the new firmware reports.
fn comes_back(r: &mut Run, b: &mut Bench, ext: u64, what: &str, model: (&str, &dyn Fn(&str) -> bool)) -> R<()> {
    let n0 = b.w.coord.events.len();
    let assoc0 = b.w.coord.events.iter().filter(|e| e.kind == "association-request").count();
    b.w.reboot().map_err(Stop::Error)?;
    b.run(D, 1500.0)?;
    let l = b.light(D);
    r.check(b.t(D), &format!("{what}: the light is on after power-on (both firmwares, factory new)"), l.on, &fmt_light(&l));
    let ok = until(b, 60000.0, |b| announced(b, ext, n0).is_some())?;
    let short = announced(b, ext, n0);
    let assoc = b.w.coord.events.iter().filter(|e| e.kind == "association-request").count() - assoc0;
    r.check(b.t(D), &format!("{what}: joins the network (again), with the same IEEE address"), ok,
            &format!("short {}, associations {assoc}", hexo(short)));
    let Some(short) = short else { return Ok(()) };
    b.run(D, 6000.0)?;
    let bas = basic(b, short)?;
    r.check(b.t(D), &format!("{what}: Basic answers {}", model.0), bas.as_deref().is_some_and(model.1),
            bas.as_deref().unwrap_or("no answer"));
    let steps: [(&str, u16, u8, Vec<u8>, fn(&crate::session::Light) -> bool); 3] = [
        ("Off", 0x0006, 0x00, vec![], |st| !st.on),
        ("On", 0x0006, 0x01, vec![], |st| st.on),
        ("Level 127", 0x0008, 0x04, vec![127, 0, 0, 0, 0], |st| st.on && (10.0..=80.0).contains(&st.duty)),
    ];
    for (name, cl, cmd, pl, exp) in steps {
        let (got, l) = light_cmd(b, short, cl, cmd, &pl)?;
        r.check(b.t(D), &format!("{what}: {name}"), got && exp(&l), &fmt_light(&l));
    }
    let r0 = b.resets(D);
    b.run(D, 20000.0)?;
    let n = b.resets(D) - r0;
    r.check(b.t(D), &format!("{what}: 20 s of operation without a reset"), n == 0, &format!("resets {n}"));
    Ok(())
}

fn node(cx: &SimCtx, fw: Fw) -> R<(zbnode::Node, crate::image::Image, u64)> {
    let img = super::mixed::image(cx, fw);
    let mut n = node_of(&img, cx.cpi)?;
    let rec = mac_record(MAC);
    n.write_flash(0xFF000, &rec);
    Ok((n, img, ieee_of(fw, &rec)))
}

/// The sectors of the stock firmware's configuration and Zigbee NV that hold data.
fn stock_nv(flash: &[u8]) -> Vec<usize> {
    (0xCF000..0xF0000).step_by(0x1000).filter(|a| flash[*a..a + 0x1000].iter().any(|x| *x != 0xFF)).collect()
}

fn ranges(v: &[usize]) -> String {
    let mut out: Vec<[usize; 2]> = Vec::new();
    for &x in v {
        match out.last_mut() {
            Some(r) if x == r[1] + 0x1000 => r[1] = x,
            _ => out.push([x, x]),
        }
    }
    if out.is_empty() {
        return "none".into();
    }
    out.iter().map(|r| format!("{:#x}-{:#x}", r[0], r[1] + 0xFFF)).collect::<Vec<_>>().join(", ")
}

// =================================================================== stock -> Rust

/// The first install: the stock firmware on the network, the Rust update
/// image sent over its BLE OTA (README: Flashing, 1), installed, power cycle.
/// The Rust firmware does not read the stock firmware's network data (other
/// format, other place), so it starts factory new and joins again.
fn migrate(cx: &mut SimCtx) -> R<()> {
    let (n, simg, ext) = node(cx, Fw::Stock)?;
    let rota = cx.image("rust-ota", "target/tc32/bulb-tc32-ota.bin");
    let payload = rota.bytes().map_err(Stop::Error)?;
    let mut b = Bench::new(n, "bulb", true, cx.log());
    let mut r = Run { cx };
    r.say(&format!("== stock: {}; Rust update image: {}", rel(&simg), rel(&rota)));
    let ok = until(&mut b, 60000.0, |b| announced(b, ext, 0).is_some())?;
    let short = announced(&b, ext, 0);
    if !r.check(b.t(D), "the stock firmware joins the network", ok, &format!("short {}", hexo(short))) {
        return Ok(());
    }
    b.run(D, 6000.0)?;
    let bas = basic(&mut b, short.unwrap())?;
    r.check(b.t(D), "stock: Basic answers", bas.is_some(), bas.as_deref().unwrap_or("no answer"));
    let nv_before = stock_nv(&b.w.node(D).m.flash.mem);
    r.say(&format!("   stock configuration and NV in use: {}", ranges(&nv_before)));

    let Some(mut tr) = ble_open(&mut b)? else {
        r.check(b.t(D), "BLE connection and the OTA characteristic (...1915)", false, "");
        return Ok(());
    };
    let t0 = b.t(D);
    let mut lines = Vec::new();
    let res = ble_push(&mut b, &mut tr, &payload, &mut |s| lines.push(s))?;
    for l in lines {
        r.say(&l);
    }
    let detail = match &res {
        Ok(st) => format!("{} B in {:.0} s of simulation, status {}", payload.len(), (b.t(D) - t0) / 1000.0, hx(st)),
        Err(e) => e.clone(),
    };
    if !r.check(b.t(D), "the Rust image sent over the stock firmware's BLE OTA", res.is_ok(), &detail) {
        return Ok(());
    }
    b.run(D, 3000.0)?;
    let f = &b.w.node(D).m.flash.mem;
    let staged = &f[STAGING..STAGING + payload.len()];
    let nd = (0..payload.len()).filter(|i| staged[*i] != payload[*i]).count();
    r.check(b.t(D), "staged at 0x6000 byte for byte, marked bootable (0x4B at +8)", nd == 0 && staged[8] == 0x4B,
            &format!("differing bytes {nd}, byte 8 {:#04x}", staged[8]));
    b.ble_disconnect(D, 3000.0)?;

    b.w.power_off(D);
    let inst = install_staged(b.w.node_mut(D)).map_err(Stop::Error)?;
    r.say(&format!("   (the AwoX bootloader's install, as inferred: {} B to 0x6a000)", inst.len()));
    b.w.power_on(D).map_err(Stop::Error)?;
    // power_on reboots; comes_back reboots once more (a second power cycle)
    comes_back(&mut r, &mut b, ext, "Rust", ("with its own model identifier (outglow-...)", &|x| x.contains("outglow")))?;
    let nv_after = stock_nv(&b.w.node(D).m.flash.mem);
    r.say(&format!("   stock configuration and NV afterwards: {}", ranges(&nv_after)));
    r.check(b.t(D), "the stock firmware's data stays where it was (the way back keeps the bulb's network)",
            nv_before.iter().all(|a| nv_after.contains(a)), &format!("before {}, after {}", ranges(&nv_before), ranges(&nv_after)));
    Ok(())
}

// =================================================================== Rust -> stock

/// Zigbee OTA from the Rust firmware to the stock image: the stock image in
/// a .ota file of the Rust firmware's manufacturer code and image type (the
/// client takes any version of its own type, README: ota/), checked against
/// the staging area as the Rust update path is (`ota_rust`); then installed
/// and started: the stock firmware, factory new, joins the network.
fn back_to_stock_ota(cx: &mut SimCtx) -> R<()> {
    let from = super::mixed::image(cx, Fw::Rust);
    let stock = super::mixed::image(cx, Fw::Stock);
    let sbytes = stock.bytes().map_err(Stop::Error)?;
    let rust_ota = cx.image("rust-ota-file", &from.path.to_string_lossy().replace(".bin", ".ota"));
    let rbytes = rust_ota.bytes().map_err(Stop::Error)?;
    if rbytes.len() < 16 {
        return Err(Stop::Error(format!("{}: not an OTA file", rust_ota.path.display())));
    }
    let (mfg, itype) = (u16::from_le_bytes([rbytes[10], rbytes[11]]), u16::from_le_bytes([rbytes[12], rbytes[13]]));
    let file = ota_file(mfg, itype, 0x0000_0001, "stock firmware (revert)", &sbytes);
    let path = std::env::temp_dir().join(format!("outglow-revert-{}-{itype:04x}.ota", std::process::id()));
    std::fs::write(&path, &file).map_err(|e| Stop::Error(format!("{}: {e}", path.display())))?;
    let ota_img = crate::image::Image { label: "ota".into(), path: path.clone(), link_shift: None };
    let mut z = Z::new(cx, &from)?;
    let rec = mac_record(MAC);
    z.w.air.dut.write_flash(0xFF000, &rec);
    z.w.reboot().map_err(Stop::Error)?;
    let ext = ieee_of(Fw::Rust, &rec);
    z.say(&format!("== Rust {} -> stock {} (.ota of mfg {mfg:#06x}, type {itype:#06x})", rel(&from), rel(&stock)));
    let res = ota_body(&mut z, &from, &ota_img, Some(&stock), Layout::Awox);
    let _ = std::fs::remove_file(&path);
    res?;
    let staged_ok = z.results.iter().any(|(n, ok, _)| n.starts_with("[awox] KNLT") && *ok);
    if staged_ok {
        let inst = install_staged(&mut z.w.air.dut).map_err(Stop::Error)?;
        z.say(&format!("   (the AwoX bootloader's install, as inferred: {} B to 0x6a000)", inst.len()));
        let n0 = z.w.coord.events.len();
        z.w.reboot().map_err(Stop::Error)?;
        z.idle(1500.0)?;
        let l = light_of(&z.w.air.dut);
        z.check("stock: the light is on after power-on (factory new)", l.on, &fmt_light(&l));
        let fe = crate::frames::fmt_ext(ext);
        let fe2 = fe.clone();
        let ok = z.run_until(60000.0, 20.0, move |z| {
            z.w.coord.events[n0..].iter().any(|e| e.kind == "device-announce" && e.get("ext") == Some(fe2.as_str()))
        })?;
        let short = z.w.coord.events[n0..]
            .iter()
            .filter(|e| e.kind == "device-announce" && e.get("ext") == Some(fe.as_str()))
            .find_map(|e| e.get("short").and_then(|s| u16::from_str_radix(s.trim_start_matches("0x"), 16).ok()));
        z.check("stock: joins the network again, with the same IEEE address", ok, &format!("short {}", hexo(short)));
        if let Some(short) = short {
            z.idle(6000.0)?;
            let rsp = z.zcl_and_wait(|z| { z.w.coord.read_attrs(&mut z.w.air, short, EP, 0x0000, &[0x0004, 0x0005, 0x4000]); }, 0x0000, &[0x01], 4000.0)?;
            let d = rsp.first().map(|r| decode_attrs(&r.body)).unwrap_or_default();
            z.check("stock: Basic answers, not with the Rust firmware's model identifier", !rsp.is_empty() && !d.contains("outglow"), &d);
            for (name, cmd, want) in [("Off", 0x00u8, false), ("On", 0x01, true)] {
                let rsp = z.zcl_and_wait(|z| { z.w.coord.zcl_cmd(&mut z.w.air, short, EP, 0x0006, cmd, &[]); }, 0x0006, &[0x0B], 3000.0)?;
                z.idle(SETTLE_MS)?;
                let l = light_of(&z.w.air.dut);
                z.check(&format!("stock: {name}"), !rsp.is_empty() && l.on == want, &fmt_light(&l));
            }
            let r0 = z.w.resets;
            z.idle(20000.0)?;
            z.check("stock: 20 s of operation without a reset", z.w.resets == r0, &format!("resets {}", z.w.resets - r0));
        }
    }
    z_finish(&mut z, cx, "back-to-stock-ota.pcap");
    Ok(())
}

/// BLE from the Rust firmware to the stock image: the Rust firmware's AwoX
/// OTA service takes the stock image into the staging area, checks it in the
/// flash, marks it and restarts; installed, the stock firmware starts,
/// advertises over BLE again and joins the network factory new.
fn back_to_stock_ble(cx: &mut SimCtx) -> R<()> {
    let (n, rimg, ext) = node(cx, Fw::Rust)?;
    let stock = super::mixed::image(cx, Fw::Stock);
    let payload = stock.bytes().map_err(Stop::Error)?;
    let mut b = Bench::new(n, "bulb", true, cx.log());
    let mut r = Run { cx };
    r.say(&format!("== Rust: {}; stock image: {}", rel(&rimg), rel(&stock)));
    let ok = until(&mut b, 60000.0, |b| announced(b, ext, 0).is_some())?;
    if !r.check(b.t(D), "the Rust firmware joins the network", ok, &format!("short {}", hexo(announced(&b, ext, 0)))) {
        return Ok(());
    }
    b.run(D, 6000.0)?;
    let Some(mut tr) = ble_open(&mut b)? else {
        r.check(b.t(D), "BLE connection and the OTA characteristic (...1915)", false, "");
        return Ok(());
    };
    let resets0 = b.resets(D);
    let t0 = b.t(D);
    let mut lines = Vec::new();
    let res = ble_push(&mut b, &mut tr, &payload, &mut |s| lines.push(s))?;
    for l in lines {
        r.say(&l);
    }
    let detail = match &res {
        Ok(st) => format!("{} B in {:.0} s of simulation, status {}", payload.len(), (b.t(D) - t0) / 1000.0, hx(st)),
        Err(e) => e.clone(),
    };
    if !r.check(b.t(D), "the stock image sent over the Rust firmware's BLE OTA", res.is_ok(), &detail) {
        return Ok(());
    }
    let restarted = until(&mut b, 10000.0, |b| b.resets(D) > resets0)?;
    let f = &b.w.node(D).m.flash.mem;
    let staged = &f[STAGING..STAGING + payload.len()];
    let nd = (0..payload.len()).filter(|i| staged[*i] != payload[*i]).count();
    r.check(b.t(D), "staged at 0x6000 byte for byte, marked bootable after the check in the flash, one restart",
            nd == 0 && staged[8] == 0x4B && restarted, &format!("differing bytes {nd}, byte 8 {:#04x}, restarted {restarted}", staged[8]));
    if staged[8] != 0x4B {
        return Ok(());
    }
    let inst = install_staged(b.w.node_mut(D)).map_err(Stop::Error)?;
    r.say(&format!("   (the AwoX bootloader's install, as inferred: {} B to 0x6a000)", inst.len()));
    comes_back(&mut r, &mut b, ext, "stock", ("with the vendor's model identifier, not outglow-...", &|x| !x.contains("outglow")))?;
    // the stock firmware's BLE is back: its advertisement names the vendor's mesh
    let adv = b.ble_adv(D);
    r.check(b.t(D), "stock: advertises over BLE again (the vendor app finds it)", !adv.is_empty(), &format!("advertising data {}", hx(&adv)));
    Ok(())
}
