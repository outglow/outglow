//! The Zigbee OTA update (emulator/ota_sim.py): image A joins, the OTA server
//! gets the .ota of image B, Image Notify, the download in blocks, Upgrade End,
//! the restart; then the flash is checked (which bank, image B byte for byte)
//! and the bulb must come back to the network and work.
//!
//! Layout awox (the AwoX bootloader, which we do not have): after Upgrade End
//! and the reset the flash must look as stock leaves it after BLE OTA (image B
//! @0x6000 with 0x4B @+8, nothing outside staging and NV touched). Then either
//! the old image must keep working (`Layout::Awox`: the emulator starts it
//! again), or the bootloader's install as inferred (`transfer::install_staged`)
//! puts image B in place and image B must come back with its network and
//! report its version (`Layout::AwoxInstalled`). Layout ownbl: our bootloader
//! installs image B.

use super::zigbee::{decode_attrs, finish, fmt_light, sector_ranges, Z};
use super::{SimCtx, SimDef};
use crate::coord::{CoordEvent, EP};
use crate::session::{light_of, R};

pub fn defs() -> Vec<SimDef> {
    vec![
        SimDef { name: "ota_rust", doc: "Zigbee OTA of the Rust firmware, layout awox", run: ota_rust },
    ]
}

/// ota_sim.py --layout awox --from target/tc32/bulb-tc32.bin --ota target/tc32-v2/bulb-tc32.ota
///   --expect-bin target/tc32-v2/bulb-tc32-ota.bin (docs/rust-radio.md), then
/// the staged image installed and started: version 2 (file version
/// 0x01000001, `cargo xtask build --file-version 01000001 --out-dir
/// target/tc32-v2`) back on the network from the NV of version 1.
fn ota_rust(cx: &mut SimCtx) -> R<()> {
    let from = cx.image("from", "target/tc32/bulb-tc32.bin");
    let ota = cx.image("ota", "target/tc32-v2/bulb-tc32.ota");
    let exp = cx.image("expect", "target/tc32-v2/bulb-tc32-ota.bin");
    run(cx, &from, &ota, Some(&exp), Layout::AwoxInstalled)
}

#[derive(Clone, Copy, PartialEq)]
pub enum Layout {
    Ownbl,
    /// staged at 0x6000 for the AwoX bootloader; the old image starts again
    Awox,
    /// as `Awox`, then installed as the AwoX bootloader is taken to install it
    /// (`transfer::install_staged`) and started
    AwoxInstalled,
}

impl Layout {
    fn awox(self) -> bool {
        matches!(self, Layout::Awox | Layout::AwoxInstalled)
    }
}

/// Python's `str(d)` of an event's details, every value a string except `none` keys.
fn py_dict(e: &CoordEvent, keys: &[&str]) -> String {
    let parts: Vec<String> = keys
        .iter()
        .map(|k| {
            let v = e.get(k).unwrap_or("");
            if v == "None" {
                format!("'{k}': None")
            } else {
                format!("'{k}': '{v}'")
            }
        })
        .collect();
    format!("{{{}}}", parts.join(", "))
}

fn base(p: &std::path::Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}

fn ints(v: &[usize]) -> String {
    crate::py::ints(v.iter().map(|x| *x as i64))
}

pub fn run(cx: &mut SimCtx, from: &crate::image::Image, ota_img: &crate::image::Image, expect: Option<&crate::image::Image>,
           layout: Layout) -> R<()> {
    let mut z = Z::new(cx, from)?;
    let r = body(&mut z, from, ota_img, expect, layout);
    // one pcap per image updated: ota_rust and the lab's ota_awox (the C
    // firmware) run side by side through this function
    let stem = from.path.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    finish(&mut z, cx, &format!("ota-sim-{stem}.pcap"));
    if let Some(l) = super::util::stack_report(z.w.node(crate::air::NodeId::Dut), from) {
        cx.say(&l);
    }
    r
}

fn events<'a>(z: &'a Z, kind: &'a str) -> impl Iterator<Item = &'a CoordEvent> + 'a {
    z.w.coord.events.iter().filter(move |e| e.kind == kind)
}

fn wait_event(z: &mut Z, kind: &str, max_ms: f64, since: Option<usize>) -> R<bool> {
    let n0 = since.unwrap_or(z.w.coord.events.len());
    let kind = kind.to_string();
    z.run_until(max_ms, 20.0, move |z| z.w.coord.events[n0..].iter().any(|e| e.kind == kind))
}

pub fn body(z: &mut Z, from: &crate::image::Image, ota_img: &crate::image::Image, expect: Option<&crate::image::Image>,
        layout: Layout) -> R<()> {
    let ota = ota_img.bytes().map_err(crate::session::Stop::Error)?;
    let (mfg, itype, ver) = z.w.coord.add_ota_file(ota.clone()).map_err(crate::session::Stop::Error)?;
    z.say(&format!(
        "== OTA: {} -> {} (mfg {mfg:#06x}, type {itype:#06x}, version {ver:#010x}, {} B)",
        base(&from.path),
        base(&ota_img.path),
        ota.len()
    ));
    let ok = wait_event(z, "device-announce", 60000.0, None)?;
    if !z.check("image A joined", ok, "") {
        return Ok(());
    }
    let short = z.short();
    if let Some(l) = super::util::stack_rearm(z.w.node_mut(crate::air::NodeId::Dut), from) {
        z.say(&l);
    }
    z.idle(4000.0)?;

    let n_assoc = events(z, "association-request").count();
    let t0 = std::time::Instant::now();
    z.w.coord.image_notify(&mut z.w.air, short, EP);
    let got_query = wait_event(z, "ota-query", 15000.0, None)?;
    let q: Vec<CoordEvent> = events(z, "ota-query").cloned().collect();
    let qkeys = ["src", "mfg", "type", "current", "offer"];
    let d = q.last().map(|e| py_dict(e, &qkeys)).unwrap_or_default();
    z.check("Query Next Image from the firmware", got_query, &d);
    if !got_query {
        return Ok(());
    }
    let last = q.last().unwrap().clone();
    if last.get("offer") == Some("None") {
        let d = format!(
            "client asks for mfg {} type {}, the file has {mfg:#06x}/{itype:#06x}",
            last.get("mfg").unwrap_or(""),
            last.get("type").unwrap_or("")
        );
        z.check("server has an image for the client's mfg/type", false, &d);
        return Ok(());
    }

    let resets0 = z.w.resets;
    let done = z.run_until(900.0 * 1000.0, 200.0, move |z| {
        z.w.resets > resets0 || z.w.coord.events.iter().any(|e| e.kind == "ota-upgrade-end")
    })?;
    let blocks: Vec<(usize, usize)> = z
        .w
        .coord
        .ota_log
        .iter()
        .filter(|(_, k, _)| k == "ota-block")
        .map(|(_, _, d)| {
            let f = |k: &str| -> usize {
                d.split(", ").find_map(|kv| kv.strip_prefix(k).and_then(|r| r.strip_prefix('='))).and_then(|v| v.parse().ok()).unwrap_or(0)
            };
            (f("offset"), f("size"))
        })
        .collect();
    let got = blocks.iter().map(|(o, s)| o + s).max().unwrap_or(0);
    z.check(
        "whole image downloaded (Image Block)",
        got >= ota.len(),
        &format!("{got}/{} B in {} blocks, {:.0} s wall time", ota.len(), blocks.len(), t0.elapsed().as_secs_f64()),
    );
    let end: Vec<CoordEvent> = events(z, "ota-upgrade-end").cloned().collect();
    let ok = end.last().is_some_and(|e| e.get("status") == Some("0x00"));
    let d = end.last().map(|e| py_dict(e, &["src", "status", "version"])).unwrap_or_default();
    z.check("Upgrade End: status SUCCESS", ok, &d);
    if !done {
        return Ok(());
    }

    // restart into the new bank
    z.run_until(20000.0, 20.0, move |z| z.w.resets > resets0)?;
    let r = z.w.resets;
    z.check("restart after the update", r > resets0, &format!("resets={r}"));
    let slot = z.w.air.dut.m.boot_slot;
    z.say(&format!("   boot bank after restart: {slot:#x}"));
    let flash = z.w.air.dut.m.flash.mem.clone();
    for b in [0x0usize, 0x20000, 0x40000] {
        let hdr = &flash[b..b + 32];
        let size = u32::from_le_bytes(hdr[0x18..0x1c].try_into().unwrap());
        z.say(&format!(
            "   bank {b:#07x}: byte8={:#04x} header={} size@0x18={size:#x}",
            hdr[8],
            super::zigbee::hex(&hdr[..12])
        ));
    }
    let exp_bytes = match expect {
        Some(e) => Some(e.bytes().map_err(crate::session::Stop::Error)?),
        None => None,
    };
    if let Some(exp) = &exp_bytes {
        let sig = &exp[0x20..0x60];
        let at: Vec<usize> = (0..0x100000).step_by(0x1000).filter(|i| &flash[i + 0x20..i + 0x60] == sig).collect();
        let at_s = crate::py::list(at.iter().map(|x| format!("'{x:#x}'")));
        z.say(&format!("   image B (signature) found in flash at: {at_s}"));
        for b in &at {
            let fb = &flash[*b..(*b + exp.len()).min(flash.len())];
            let nd = (0..exp.len()).filter(|i| fb.get(*i) != Some(&exp[*i])).count();
            z.say(&format!("   bank {b:#07x} vs image B: differing bytes {nd}, byte8={:#04x}", flash[b + 8]));
        }
        let rng = sector_ranges(&flash);
        let nv: usize = rng.iter().map(|[a, b]| (b - a) / 0x1000 + 1).sum();
        let r = rng.iter().map(|[a, b]| format!("{a:#x}-{:#x}", b + 0xfff)).collect::<Vec<_>>().join(", ");
        z.say(&format!("   non-empty 4K sectors: {nv}; ranges: {r}"));
    }
    if layout.awox() {
        if let Some(exp) = &exp_bytes {
            let staged = &flash[0x6000..0x6000 + exp.len()];
            let diff: Vec<usize> = (0..exp.len()).filter(|i| staged[*i] != exp[*i]).collect();
            let first = if diff.is_empty() { String::new() } else { format!(", first {}", ints(&diff[..diff.len().min(8)])) };
            z.check(
                "[awox] staging 0x6000 == image B byte for byte (state as after stock BLE OTA)",
                diff.is_empty(),
                &format!("differing bytes {}{first}, byte8={:#04x}", diff.len(), staged[8]),
            );
            z.check("[awox] KNLT flag 0x4B @0x6008 (the AwoX bootloader sees a valid image)", staged[8] == 0x4B, "");
            let a_len = from.bytes().map_err(crate::session::Stop::Error)?.len();
            let app_end = (0x6A000 + a_len + 0xFFF) & !0xFFF;
            // 0xCF000: power-on counter (factory reset by sequence), as in stock
            let allowed = |i: usize| {
                (0x6000..0x6A000).contains(&i) || (0xE6000..0xFE000).contains(&i) || i < 0x6000 || (0x6A000..app_end).contains(&i)
                    || i >= 0xFE000 || i == 0xCF000
            };
            let bad: Vec<String> = (0..0x100000)
                .step_by(0x1000)
                .filter(|i| flash[*i..i + 0x1000].iter().any(|b| *b != 0xFF) && !allowed(*i))
                .map(|i| format!("{i:#x}"))
                .collect();
            z.check("[awox] nothing written outside staging/NV/image/power-on counter", bad.is_empty(), &bad.join(", "));
            let r = z.w.resets;
            z.check("[awox] one reset, no restart loop", r == resets0 + 1, &format!("resets={r}"));
            if layout == Layout::Awox {
                z.say("   [awox] without the AwoX bootloader in the simulation the old image starts again; below we check that it works.");
            }
        }
    }
    if layout == Layout::AwoxInstalled {
        let inst = super::transfer::install_staged(&mut z.w.air.dut).map_err(crate::session::Stop::Error)?;
        z.say(&format!("   [awox] the AwoX bootloader's install, as inferred: {} B to 0x6a000; restart", inst.len()));
        z.w.reboot().map_err(crate::session::Stop::Error)?;
    }
    if layout == Layout::Ownbl {
        if let Some(exp) = &exp_bytes {
            let app = if flash[0x6A008] == 0x4B { 0x6A000 } else { slot };
            let fl = &flash[app..(app + exp.len()).min(flash.len())];
            let diff: Vec<usize> = (0..exp.len()).filter(|i| fl.get(*i) != Some(&exp[*i])).collect();
            let first = if diff.is_empty() { String::new() } else { format!(", first {}", ints(&diff[..diff.len().min(8)])) };
            z.check("application flash == image B", diff.is_empty(), &format!("address {app:#x}, differing bytes {}{first}", diff.len()));
            let staged = &flash[0x10000..0x10010];
            if app == 0x6A000 {
                z.check("temporary OTA image (0x10000) erased by the bootloader", staged.iter().all(|x| *x == 0xFF),
                        &super::zigbee::hex(staged));
            }
        }
    }

    // the new firmware returns to the network and works
    z.idle(8000.0)?;
    let n_assoc2 = events(z, "association-request").count();
    z.check("no re-association (network from NV)", n_assoc2 == n_assoc,
            &format!("associations before {n_assoc}, after {n_assoc2}"));
    let rsp = z.zcl_and_wait(|z| { z.w.coord.read_attrs(&mut z.w.air, short, EP, 0x0000, &[0x0004, 0x0005, 0x4000]); }, 0x0000, &[0x01], 4000.0)?;
    let d = rsp.first().map(|r| decode_attrs(&r.body)).unwrap_or_default();
    z.check("after the update: Basic responds", !rsp.is_empty(), &d);
    let steps: [(&str, u16, u8, Vec<u8>, fn(&crate::session::Light) -> bool); 3] = [
        ("Off", 0x0006, 0x00, vec![], |st| !st.on),
        ("On", 0x0006, 0x01, vec![], |st| st.on),
        ("Level 127", 0x0008, 0x04, vec![127, 0, 0, 0, 0], |st| st.on && (10.0..=80.0).contains(&st.duty)),
    ];
    for (name, cl, cmd, pl, exp) in steps {
        let rsp = z.zcl_and_wait(|z| { z.w.coord.zcl_cmd(&mut z.w.air, short, EP, cl, cmd, &pl); }, cl, &[0x0B], 3000.0)?;
        z.idle(1200.0)?;
        let st = light_of(&z.w.air.dut);
        z.check(&format!("after the update: {name}"), !rsp.is_empty() && exp(&st), &fmt_light(&st));
    }
    // the firmware after the update reports its version (Query Next Image)
    let n0 = z.w.coord.events.len();
    z.w.coord.image_notify(&mut z.w.air, short, EP);
    wait_event(z, "ota-query", 15000.0, Some(n0))?;
    let q2: Vec<CoordEvent> = z.w.coord.events[n0..].iter().filter(|e| e.kind == "ota-query").cloned().collect();
    let cur = |e: &CoordEvent| u32::from_str_radix(e.get("current").unwrap_or("0x0").trim_start_matches("0x"), 16).unwrap_or(0);
    if layout == Layout::Awox {
        // without the AwoX bootloader in the simulation the new image was not installed: the OLD version answers
        let ok = q2.last().is_some_and(|e| cur(e) != ver);
        let d = q2.last().map(|e| py_dict(e, &qkeys)).unwrap_or_default();
        z.check("[awox] the old image still responds (the new one will only be installed by the AwoX bootloader)", ok, &d);
        return Ok(());
    }
    let ok = q2.last().is_some_and(|e| cur(e) == ver);
    let c = q2.last().and_then(|e| e.get("current").map(String::from)).unwrap_or_else(|| "None".into());
    z.check("after the update the firmware reports the new version", ok, &format!("current={c}, expected {ver:#010x}"));
    Ok(())
}
