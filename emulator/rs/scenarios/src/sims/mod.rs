//! The integration scenarios of emulator/*_sim.py in Rust: several emulated
//! bulbs (any images) on one 802.15.4 air with the plain coordinator, driven
//! in lockstep. Each scenario prints its checks in the Python script's own
//! format (`[  12.3 s] PASS: what  detail`), so the two outputs can be
//! compared line for line, and passes when all its checks pass.
//!
//! `cargo test -p scenarios --test sims [-- filters] [--cpi N] [-v]`.

pub mod addr_conflict;
pub mod ble_central;
pub mod ble_diag;
pub mod board_check;
pub mod ble_mesh;
pub mod ble_ota;
pub mod enddev_wallswitch;
pub mod interop;
pub mod app_colour;
pub mod json;
pub mod mesh;
pub mod mixed;
pub mod ota;
pub mod parent_load;
pub mod power_cut;
pub mod runner;
pub mod rust_ble;
pub mod rust_gp;
pub mod rust_radio;
pub mod rust_ble_switch;
pub mod rust_diag;
pub mod rust_fw;
pub mod rust_staging;
pub mod rust_touchlink;
pub mod stress;
pub mod touchlink;
pub mod transfer;
pub mod util;
pub mod wireshark;
pub mod zbase;
pub mod zcl_audit;
pub mod zigbee;

use std::time::Instant;

use zbnode::Node;

use crate::air::{Log, NodeId, World};
use crate::coord::Coord;
use crate::frames::Keys;
use crate::image::Image;
use crate::session::{Light, Stop, R};

/// A scenario: its name (the Python script's, without `_sim.py`) and body.
pub struct SimDef {
    pub name: &'static str,
    pub doc: &'static str,
    pub run: fn(&mut SimCtx) -> R<()>,
}

pub fn sims() -> Vec<SimDef> {
    let mut v = vec![
        board_check::def(),
        ble_diag::def(),
        ble_mesh::def(),
        ble_ota::def(),
        interop::def(),
        app_colour::def(),
        mesh::def(),
        parent_load::def(),
        power_cut::def(),
        rust_ble::def(),
        rust_ble_switch::def(),
        rust_diag::def(),
        rust_fw::def(),
        rust_gp::def(),
        rust_radio::def(),
        rust_touchlink::def(),
        rust_staging::def(),
        wireshark::def(),
        wireshark::def_ble(),
        zcl_audit::def(),
        zigbee::def(),
    ];
    v.extend(addr_conflict::defs());
    v.extend(enddev_wallswitch::defs());
    v.extend(ota::defs());
    v.extend(mixed::defs());
    v.extend(transfer::defs());
    v.extend(stress::defs());
    v
}

/// A default image path (repository-relative): in the repository, else in
/// the first of the directories of `SCENARIOS_IMAGE_DIRS` (separated as PATH)
/// that has it, so that images kept elsewhere need no `--image` each time.
pub fn image_path(rel: &str) -> std::path::PathBuf {
    let here = crate::runner::repo_root().join(rel);
    if here.exists() {
        return here;
    }
    std::env::var_os("SCENARIOS_IMAGE_DIRS")
        .and_then(|dirs| std::env::split_paths(&dirs).map(|d| d.join(rel)).find(|p| p.exists()))
        .or_else(|| stock_dir_path(rel))
        .unwrap_or(here)
}

/// The directory of the original firmware images kept apart from this
/// repository (a checkout of outglow/original-firmware: `firmware/<board>.bin`).
pub const STOCK_DIR_VAR: &str = "OUTGLOW_STOCK_DIR";

/// The board whose original image a default `stock/...` path names:
/// stock/stock-dimm.bin and stock/stock-tw.bin (the filament bulbs),
/// stock/apk/smartlightz/<model>__<rev>.bin (tools/extract-stock-images.sh).
pub fn stock_board_of(rel: &str) -> Option<String> {
    match rel {
        "stock/stock-dimm.bin" => Some("ESMLFzm_w6_Dimm-4.63".into()),
        "stock/stock-tw.bin" => Some("ESMLFzm_w6_TW-4.63".into()),
        _ => {
            let name = rel.strip_prefix("stock/apk/smartlightz/")?.strip_suffix(".bin")?;
            let (model, rev) = name.rsplit_once("__")?;
            Some(format!("{model}-{rev}"))
        }
    }
}

/// A default `stock/...` path found in `OUTGLOW_STOCK_DIR` (its `firmware/`
/// subdirectory or the directory itself), when it is there.
pub fn stock_dir_path(rel: &str) -> Option<std::path::PathBuf> {
    let dir = std::path::PathBuf::from(std::env::var_os(STOCK_DIR_VAR)?);
    let board = stock_board_of(rel)?;
    [dir.join("firmware").join(format!("{board}.bin")), dir.join(format!("{board}.bin"))].into_iter().find(|p| p.exists())
}

/// What a scenario gets from the runner.
pub struct SimCtx {
    pub cpi: i64,
    pub verbose: u8,
    /// image overrides: label -> image (`--image R=path`)
    pub images: Vec<Image>,
    pub out: String,
    pub results: Vec<(String, bool, String)>,
    pub t_wall: Instant,
    /// default images asked for that are not on disk: a scenario that then
    /// stops is reported as skipped, not failed
    pub missing: std::cell::RefCell<Vec<String>>,
}

impl SimCtx {
    /// The image for a role: an override with that label, else the default path (repo-relative).
    pub fn image(&self, label: &str, default: &str) -> Image {
        self.images
            .iter()
            .find(|i| i.label == label)
            .cloned()
            .unwrap_or_else(|| {
                let path = image_path(default);
                let mut missing = self.missing.borrow_mut();
                if !path.exists() && !missing.iter().any(|m| m == default) {
                    missing.push(default.into());
                }
                Image { label: label.into(), path, link_shift: None }
            })
    }

    /// Where a missing default image comes from.
    pub fn how_to_get(path: &str) -> &'static str {
        if path.starts_with("stock/") {
            "the original firmware: OUTGLOW_STOCK_DIR=<checkout of outglow/original-firmware>, or tools/extract-stock-images.sh"
        } else {
            "the Rust firmware: cargo xtask build (--board, --file-version)"
        }
    }

    pub fn say(&mut self, line: &str) {
        self.out.push_str(line);
        self.out.push('\n');
        if self.verbose > 0 {
            println!("{line}");
        }
    }

    pub fn log(&self) -> Option<Log> {
        if self.verbose > 1 {
            Some(std::rc::Rc::new(|s: &str| println!("{s}")))
        } else {
            None
        }
    }
}

/// Bulbs, the coordinator and the clock of one scenario (`lockstep(both, ...)`).
pub struct Sim {
    pub w: World,
    pub quantum: f64,
}

pub fn emu(e: String) -> Stop {
    Stop::Error(e)
}

pub fn node_of(img: &Image, cpi: i64) -> R<Node> {
    let bytes = img.bytes().map_err(Stop::Error)?;
    let shift = img.shift(&bytes);
    Node::new(&bytes, cpi, shift).map_err(emu)
}

impl Sim {
    /// The first bulb (it owns the schedule) and the coordinator; the coordinator's
    /// Link Status starts when `start` is called.
    pub fn new(first: Node, name: &str, coord: Coord, log: Option<Log>, quantum: f64) -> Sim {
        let mut w = World::new(first, coord, Keys::default(), log);
        w.air.dut_name = name.to_string();
        Sim { w, quantum }
    }

    pub fn now(&self) -> f64 {
        self.w.bulbs_now()
    }

    pub fn run(&mut self, ms: f64) -> R<()> {
        let t = self.now() + ms;
        self.w.lockstep(t, self.quantum).map_err(emu)
    }

    /// Runs until `cond` holds (checked every `step` ms) or `max_ms` passed.
    pub fn run_until(&mut self, max_ms: f64, step: f64, mut cond: impl FnMut(&Sim) -> bool) -> R<bool> {
        let end = self.now() + max_ms;
        while self.now() < end {
            if cond(self) {
                return Ok(true);
            }
            let t = end.min(self.now() + step);
            self.w.lockstep(t, self.quantum).map_err(emu)?;
        }
        Ok(cond(self))
    }

    pub fn light(&self, id: NodeId) -> Light {
        crate::session::light_of(self.w.node(id))
    }

    /// The short address of an announced device with this extended address.
    pub fn announced(&self, ext: u64) -> Option<u16> {
        self.w.coord.devices.iter().find(|d| d.ext == ext && d.announced).map(|d| d.short)
    }

    pub fn events(&self, kind: &str) -> Vec<crate::coord::CoordEvent> {
        self.w.coord.events.iter().filter(|e| e.kind == kind).cloned().collect()
    }

    /// (sent, received, dropped reasons) of a bulb's radio (`GoEngine.stats`).
    pub fn stats(&self, id: NodeId) -> (i64, i64, String) {
        let r = self.w.node(id).radio.b();
        let why = if r.dropped.is_empty() {
            "none".to_string()
        } else {
            format!("{{{}}}", r.dropped.iter().map(|(k, v)| format!("'{k}': {v}")).collect::<Vec<_>>().join(", "))
        };
        (r.sent, r.received, why)
    }
}

impl SimCtx {
    /// A check, printed as the Python scripts print it.
    pub fn check(&mut self, now_ms: f64, name: &str, ok: bool, detail: &str) -> bool {
        self.results.push((name.to_string(), ok, detail.to_string()));
        let line = format!("[{:7.1} s] {}: {name}  {detail}", now_ms / 1000.0, if ok { "PASS" } else { "FAIL" });
        self.say(&line);
        ok
    }

    /// The closing lines (`finish` of the scripts): result and per-bulb radio statistics.
    pub fn finish(&mut self, sim: &Sim, nodes: &[(NodeId, &str)]) {
        let ok = self.results.iter().filter(|r| r.1).count();
        let n = self.results.len();
        let t = sim.now();
        let wall = self.t_wall.elapsed().as_secs_f64();
        self.say(&format!(
            "\n== result: {ok}/{n} checks passed; simulation time {:.1} s, wall time {wall:.0} s",
            t / 1000.0
        ));
        for (id, name) in nodes {
            let (s, r, why) = sim.stats(*id);
            self.say(&format!("  {name}: sent {s}, received {r}, dropped {why}"));
        }
    }

    pub fn passed(&self) -> bool {
        !self.results.is_empty() && self.results.iter().all(|r| r.1)
    }
}
