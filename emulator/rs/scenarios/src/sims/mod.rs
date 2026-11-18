pub mod addr_conflict;
pub mod ble_central;
pub mod ble_diag;
pub mod board_check;
pub mod ble_mesh;
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
pub mod rust_sense;
pub mod sense_zcl;
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
        rust_radio::def_slow_ack(),
        rust_sense::def(),
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

pub const WEEKLY: &[&str] = &[
    "addr_conflict",
    "addr_conflict_child",
    "addr_conflict_victim",
    "addr_conflict_channel",
    "addr_conflict_echo",
    "addr_conflict_multihop",
    "addr_conflict_house",
    "enddev_wallswitch",
    "enddev_wallswitch_edinit",
];

pub fn is_weekly(name: &str) -> bool {
    WEEKLY.contains(&name.trim_start_matches("sims::"))
}

pub const WITH_STOCK: &[&str] = &[
    "rust_radio",
    "rust_gp",
    "rust_touchlink",
    "interop",
    "app_colour",
    "migrate",
    "back_to_stock_ota",
    "back_to_stock_ble",
    "mixed_parent_stock",
    "mixed_ota_relay",
];

pub fn needs_stock(name: &str) -> bool {
    WITH_STOCK.contains(&name.trim_start_matches("sims::"))
}

pub const FIRMWARE_VAR: &str = "SIMS_FIRMWARE";

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Firmware {
    #[default]
    Rust,
    Stock,
    C,
}

impl Firmware {
    pub fn parse(v: &str) -> Result<Firmware, String> {
        match v {
            "rust" | "outglow" => Ok(Firmware::Rust),
            "stock" | "original" => Ok(Firmware::Stock),
            "c" => Ok(Firmware::C),
            _ => Err(format!("firmware {v:?}: rust, stock or c")),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Firmware::Rust => "rust",
            Firmware::Stock => "stock",
            Firmware::C => "c",
        }
    }

    pub fn of(img: &Image) -> &'static str {
        if !img.path.exists() {
            "missing"
        } else if util::is_outglow(img) {
            "Rust"
        } else if img.path.to_string_lossy().contains("open-eglo") {
            "C"
        } else {
            "stock"
        }
    }

    pub fn instead_of_stock(self, rel: &str) -> String {
        let Some(board) = stock_board_of(rel) else { return rel.to_string() };
        match self {
            Firmware::Stock => rel.to_string(),
            Firmware::Rust => format!("target/{}/bulb-tc32.bin", crate::profile::board_dir(&board)),
            Firmware::C => match board.as_str() {
                "ESMLFzm_w6_Dimm-4.63" => "firmware/build/open-eglo.bin".into(),
                "ESMLFzm_w6_TW-4.63" => "firmware/build-tw/open-eglo.bin".into(),
                _ => format!("firmware/build-{}/open-eglo.bin", board.to_lowercase()),
            },
        }
    }
}

pub fn is_bulb_image(path: &std::path::Path) -> bool {
    let p = path.to_string_lossy();
    p.ends_with(".bin") && !p.ends_with("-ota.bin")
}

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

pub const STOCK_DIR_VAR: &str = "OUTGLOW_STOCK_DIR";

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

pub fn stock_dir_path(rel: &str) -> Option<std::path::PathBuf> {
    let dir = std::path::PathBuf::from(std::env::var_os(STOCK_DIR_VAR)?);
    let board = stock_board_of(rel)?;
    [dir.join("firmware").join(format!("{board}.bin")), dir.join(format!("{board}.bin"))].into_iter().find(|p| p.exists())
}

pub struct SimCtx {
    pub cpi: i64,
    pub verbose: u8,
    pub images: Vec<Image>,
    pub out: String,
    pub results: Vec<(String, bool, String)>,
    pub t_wall: Instant,
    pub missing: std::cell::RefCell<Vec<String>>,
    pub firmware: Firmware,
    pub bulbs: std::cell::RefCell<Vec<(String, std::path::PathBuf)>>,
}

impl SimCtx {
    pub fn new(cpi: i64, verbose: u8, images: Vec<Image>, firmware: Firmware) -> SimCtx {
        SimCtx {
            cpi,
            verbose,
            images,
            out: String::new(),
            results: Vec::new(),
            t_wall: Instant::now(),
            missing: Default::default(),
            firmware,
            bulbs: Default::default(),
        }
    }

    pub fn image(&self, label: &str, default: &str) -> Image {
        let img = self.images.iter().find(|i| i.label == label).cloned().unwrap_or_else(|| {
            let rel = self.firmware.instead_of_stock(default);
            let path = image_path(&rel);
            let mut missing = self.missing.borrow_mut();
            if !path.exists() && !missing.iter().any(|m| *m == rel) {
                missing.push(rel);
            }
            Image { label: label.into(), path, link_shift: None }
        });
        let mut bulbs = self.bulbs.borrow_mut();
        if is_bulb_image(&img.path) && !bulbs.iter().any(|b| b.0 == label) {
            bulbs.push((label.to_string(), img.path.clone()));
        }
        img
    }

    pub fn firmware_line(&self) -> Option<String> {
        let bulbs = self.bulbs.borrow();
        if bulbs.is_empty() {
            return None;
        }
        let each: Vec<String> = bulbs
            .iter()
            .map(|(label, path)| {
                let img = Image { label: label.clone(), path: path.clone(), link_shift: None };
                format!("{label} = {} ({})", Firmware::of(&img), util::rel(&img))
            })
            .collect();
        Some(format!("firmware: {}", each.join(", ")))
    }

    pub fn how_to_get(path: &str) -> &'static str {
        if path.starts_with("stock/") {
            "the original firmware: OUTGLOW_STOCK_DIR=<checkout of outglow/original-firmware>, or tools/extract-stock-images.sh"
        } else if path.starts_with("firmware/") {
            "the C firmware of outglow-lab: make -C firmware [VARIANT=tw] ota there, SCENARIOS_IMAGE_DIRS=<lab>"
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

pub struct Sim {
    pub w: World,
    pub quantum: f64,
}

pub fn emu(e: String) -> Stop {
    Stop::Error(e)
}

pub fn node_of(img: &Image, cpi: i64) -> R<Node> {
    img.node(cpi).map_err(emu)
}

impl Sim {
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

    pub fn announced(&self, ext: u64) -> Option<u16> {
        self.w.coord.devices.iter().find(|d| d.ext == ext && d.announced).map(|d| d.short)
    }

    pub fn events(&self, kind: &str) -> Vec<crate::coord::CoordEvent> {
        self.w.coord.events.iter().filter(|e| e.kind == kind).cloned().collect()
    }

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
    pub fn check(&mut self, now_ms: f64, name: &str, ok: bool, detail: &str) -> bool {
        self.results.push((name.to_string(), ok, detail.to_string()));
        let line = format!("[{:7.1} s] {}: {name}  {detail}", now_ms / 1000.0, if ok { "PASS" } else { "FAIL" });
        self.say(&line);
        ok
    }

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
