//! The firmware staging area of the Rust firmware under two update paths
//! (emulator/rust_staging_sim.py): an image intact over BLE but damaged in
//! the flash is not marked bootable; a BLE transfer is refused during a
//! Zigbee download; the Zigbee client does not download during a BLE transfer.

use super::ble_central::{data_packets, discover, end_packet, hx, ok, tel_get, Bench, Transport, OTA, PAIR};
use super::{node_of, SimCtx, SimDef};
use crate::air::NodeId;
use crate::session::{Stop, R};

pub fn def() -> SimDef {
    SimDef { name: "rust_staging", doc: "the staging area of the Rust firmware under BLE and Zigbee updates", run }
}

const D: NodeId = NodeId::Dut;
const STAGING: usize = 0x6000;

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// `telink_image.finalize`: marker, size @0x18, CRC (binascii.crc32 ^ 0xFFFFFFFF) appended.
pub fn finalize(image: &[u8]) -> Vec<u8> {
    let mut fw = image.to_vec();
    if fw[6..8] == [0x5d, 0x02] {
        return fw;
    }
    let pad = (16 - fw.len() % 16) % 16;
    fw.extend(std::iter::repeat(0xFF).take(pad));
    let size = (fw.len() + 4) as u32;
    fw[0x18..0x1C].copy_from_slice(&size.to_le_bytes());
    fw[6..8].copy_from_slice(&[0x5d, 0x02]);
    let crc = crc32(&fw) ^ 0xFFFF_FFFF;
    fw.extend_from_slice(&crc.to_le_bytes());
    fw
}

/// A well-formed image of a few kilobytes (never started).
fn small_image(seed: usize) -> Vec<u8> {
    let mut body: Vec<u8> = (0..6000).map(|i| ((i * 31 + seed) & 0xFF) as u8).collect();
    body[8..12].copy_from_slice(b"KNLT");
    body[6..8].copy_from_slice(&[0, 0]);
    finalize(&body)
}

fn until(b: &mut Bench, max_ms: f64, step: f64, mut cond: impl FnMut(&Bench) -> bool) -> R<bool> {
    let end = b.t(D) + max_ms;
    while b.t(D) < end && !cond(b) {
        b.run(D, step)?;
    }
    Ok(cond(b))
}

fn connect(b: &mut Bench) -> R<Transport> {
    b.ble_connect(D, 24, None);
    if b.ble_wait_connected(D, 15000.0)?.is_none() {
        return Err(Stop::Error("AssertionError: no BLE connection".into()));
    }
    let (_, tel, _) = discover(b, D)?;
    let h = |k: u8| tel_get(&tel, k).ok_or_else(|| Stop::Error(format!("KeyError: {k}")));
    Ok(Transport::new(D, &[(OTA, h(0x15)?), (PAIR, h(0x14)?)]))
}

fn send(b: &mut Bench, tr: &mut Transport, image: &[u8], upto: Option<usize>) -> R<usize> {
    let packets = data_packets(image, 23);
    for (_, pkt) in packets.iter().take(upto.unwrap_or(packets.len())) {
        ok(tr.write(b, OTA, pkt)?)?;
    }
    Ok(packets.len())
}

fn blocks(b: &Bench) -> usize {
    b.w.coord.ota_log.iter().filter(|(_, k, _)| k == "ota-block").count()
}

fn run(cx: &mut SimCtx) -> R<()> {
    let img = cx.image("bin", "target/tc32/bulb-tc32.bin");
    let ota_img = cx.image("ota", "target/tc32-v2/bulb-tc32.ota");
    let node = node_of(&img, cx.cpi)?;
    let mut b = Bench::new(node, "dut", true, None);
    let mut results: Vec<bool> = Vec::new();
    let mut check = |cx: &mut SimCtx, name: &str, okv: bool, detail: &str| {
        results.push(okv);
        cx.results.push((name.into(), okv, detail.into()));
        cx.say(&format!("  {}  {name}{}", if okv { "PASS" } else { "FAIL" }, if detail.is_empty() { String::new() } else { format!("  {detail}") }));
    };

    let joined = until(&mut b, 60000.0, 20.0, |b| b.w.coord.events.iter().any(|e| e.kind == "device-announce"))?;
    check(cx, "joins the Zigbee network", joined, "");
    if !joined {
        return Ok(());
    }
    if let Some(l) = super::util::stack_rearm(b.w.node_mut(D), &img) {
        cx.say(&l);
    }
    let short = b.w.coord.devices.iter().find(|d| d.announced).unwrap().short;
    b.run(D, 6000.0)?;

    // --- 1. damaged in the flash
    let mut tr = connect(&mut b)?;
    let image = small_image(1);
    send(&mut b, &mut tr, &image, None)?;
    let st = ok(tr.read(&mut b, OTA)?)?;
    check(cx, "BLE transfer of a small image: all blocks taken", st[0] == 3 && st[1] == 0, &hx(&st[..8.min(st.len())]));
    let staged = b.flash(D, STAGING, image.len());
    check(cx, "staged as sent, bootable mark held back", staged[..8] == image[..8] && staged[8] == 0xFF && staged[9..] == image[9..], "");
    // one bit of the staged image goes wrong (flash can only clear bits)
    let at = STAGING + 0x400;
    let v = b.flash(D, at, 1)[0];
    b.write_flash(D, at, &[if v & 1 != 0 { v & 0xFE } else { v & 0xFD }]);
    check(cx, "(test) one bit of the staged image changed", b.flash(D, at, 1)[..] != image[0x400..0x401], "");
    ok(tr.write(&mut b, OTA, &end_packet(image.len(), 23))?)?;
    let st = ok(tr.read(&mut b, OTA)?)?;
    check(cx, "the transfer itself ended well (stream checksum good)", st[0] == 4 && st[1] == 0, &hx(&st[..8.min(st.len())]));
    let resets = b.resets(D);
    b.run(D, 2500.0)?;
    let st = ok(tr.read(&mut b, OTA)?)?;
    let b8 = b.flash(D, STAGING + 8, 1);
    check(cx, "the flash is checked before the image is marked: no restart, status failed",
          b.resets(D) == resets && b8 == [0xff] && st[0] == 4 && st[1] == 2,
          &format!("resets {}, byte 8 {}, status {}", b.resets(D) - resets, hx(&b8), hx(&st[..2.min(st.len())])));
    b.ble_disconnect(D, 3000.0)?;
    b.run(D, 800.0)?;

    // --- 2. BLE transfer running: Zigbee does not download
    let ota = ota_img.bytes().map_err(Stop::Error)?;
    b.w.coord.add_ota_file(ota).map_err(Stop::Error)?;
    let mut tr = connect(&mut b)?;
    let image = small_image(2);
    let n = send(&mut b, &mut tr, &image, Some(40))?;
    let blocks0 = blocks(&b);
    b.w.coord.image_notify(&mut b.w.air, short, 1);
    until(&mut b, 15000.0, 20.0, |b| b.w.coord.events.iter().any(|e| e.kind == "ota-query"))?;
    b.run(D, 3000.0)?;
    let nb = blocks(&b) - blocks0;
    check(cx, "Zigbee image offered during a BLE transfer: no block requested",
          b.w.coord.events.iter().any(|e| e.kind == "ota-query") && nb == 0, &format!("{nb} blocks"));
    for (_, pkt) in data_packets(&image, 23).iter().skip(40) {
        ok(tr.write(&mut b, OTA, pkt)?)?;
    }
    let st = ok(tr.read(&mut b, OTA)?)?;
    let staged = b.flash(D, STAGING, image.len());
    check(cx, "the BLE transfer goes on undisturbed", st[0] == 3 && st[1] == 0 && staged[9..] == image[9..],
          &format!("{n} blocks, status {}", hx(&st[..4.min(st.len())])));
    b.ble_disconnect(D, 3000.0)?;
    b.run(D, 1500.0)?;

    // --- 3. Zigbee download running: BLE is refused
    let n0 = b.w.coord.events.len();
    b.w.coord.image_notify(&mut b.w.air, short, 1);
    let started = until(&mut b, 60000.0, 50.0, |b| blocks(b) - blocks0 >= 20)?;
    check(cx, "with the BLE transfer gone the Zigbee download starts", started, "");
    let mut tr = connect(&mut b)?;
    for (_, pkt) in data_packets(&small_image(3), 23).iter().take(3) {
        ok(tr.write(&mut b, OTA, pkt)?)?;
    }
    let st = ok(tr.read(&mut b, OTA)?)?;
    check(cx, "BLE transfer during a Zigbee download is refused", st[1] == 2, &hx(&st[..4.min(st.len())]));
    b.ble_disconnect(D, 3000.0)?;
    let resets = b.resets(D);
    let done = until(&mut b, 900000.0, 200.0, |b| {
        b.resets(D) > resets || b.w.coord.events[n0..].iter().any(|e| e.kind == "ota-upgrade-end")
    })?;
    let end: Vec<_> = b.w.coord.events[n0..].iter().filter(|e| e.kind == "ota-upgrade-end").cloned().collect();
    let shown = match end.last() {
        Some(e) => format!(
            "{{'src': '{}', 'status': '{}', 'version': '{}'}}",
            e.get("src").unwrap_or(""),
            e.get("status").unwrap_or(""),
            e.get("version").unwrap_or("")
        ),
        None => "None".into(),
    };
    check(cx, "the Zigbee download finishes: Upgrade End SUCCESS", done && end.last().is_some_and(|e| e.get("status") == Some("0x00")), &shown);
    until(&mut b, 20000.0, 100.0, |b| b.resets(D) > resets)?;
    let exp_path = ota_img.path.to_string_lossy().replace(".ota", "-ota.bin");
    let expect = std::fs::read(&exp_path).map_err(|e| Stop::Error(format!("{exp_path}: {e}")))?;
    let staged = b.flash(D, STAGING, expect.len());
    let ndiff = staged.iter().zip(expect.iter()).filter(|(x, y)| x != y).count();
    check(cx, "staged image is the Zigbee one, byte for byte, marked bootable, one restart",
          staged == expect && b.resets(D) == resets + 1, &format!("differing bytes {ndiff}, resets {}", b.resets(D) - resets));
    if let Some(l) = super::util::stack_report(b.w.node(D), &img) {
        cx.say(&l);
    }
    cx.say(&format!("== {}/{} passed", results.iter().filter(|x| **x).count(), results.len()));
    Ok(())
}
