//! `cargo xtask compare`: the data of the website's comparison page
//! (site/public/compare.json): the Zigbee and the Bluetooth conformance tests
//! (emulator/rs/scenarios, `cargo spec` and `cargo ble-spec`) run in the
//! emulator on the original firmware and on Outglow, for the two filament
//! bulbs.
//!
//!     OUTGLOW_STOCK_DIR=<checkout of outglow/original-firmware> cargo xtask compare \
//!         [--out site/public/compare.json] [--no-build] [--jobs N] [--cpi N]
//!
//! The Outglow images are built first (`cargo xtask build`, default options)
//! unless `--no-build`; the original images come from OUTGLOW_STOCK_DIR (a
//! checkout of outglow/original-firmware, not part of this repository): every
//! image its `manifest.json` lists for one of the two boards is tested, so a
//! newer original version there is tested by the next run, and several
//! versions of one board give results per version. The website's workflow
//! (.github/workflows/site.yml) runs this before it builds the site; the
//! committed site/public/compare.json is what a build without the original
//! images (a fork) shows.
//!
//! The original firmware's results of the tests on security (keys, frame
//! security, Bluetooth pairing and encryption) and on access control (who
//! may make a bulb leave the network, reset it or bind it) are not written:
//! whatever they are, pass or fail, the file says "withheld" for them. The
//! Outglow results of those tests are written as for every other test.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};
use sha2::Sha256;

#[derive(clap::Args)]
pub struct Args {
    /// where to write the data (default site/public/compare.json)
    #[arg(long)]
    out: Option<PathBuf>,
    /// use the Outglow images already in target/ instead of building them
    #[arg(long)]
    no_build: bool,
    /// emulator sessions in parallel (default: one per CPU)
    #[arg(long)]
    jobs: Option<usize>,
    /// emulator clocks per instruction (3: full fidelity, as `cargo spec`)
    #[arg(long, default_value_t = 3)]
    cpi: u32,
}

/// The bulbs compared: short id, board, name on the page.
const BOARDS: [(&str, &str, &str); 2] = [
    ("dimm", "ESMLFzm_w6_Dimm-4.63", "Dimmable filament bulb (ESMLFzm_w6_Dimm)"),
    ("tw", "ESMLFzm_w6_TW-4.63", "Tunable white filament bulb (ESMLFzm_w6_TW)"),
];

/// The two test suites: id, the scenarios test binary, what it covers.
const SUITES: [(&str, &str, &str); 2] = [
    ("zigbee", "spec", "Zigbee: Zigbee PRO R22, Zigbee Cluster Library revision 8, Base Device Behavior 3.0.1, Lighting & Occupancy 1.0, Green Power Basic 1.1.2"),
    ("ble", "ble", "Bluetooth LE: Bluetooth Core Specification 5.4 and the Core Specification Supplement"),
];

const SECURITY: &str = "security";
const ACCESS: &str = "access";

/// Tests (module, id or id prefix) whose result on the original firmware is not published.
const WITHHELD: [(&str, &str); 16] = [
    ("security_keys", SECURITY),
    // what the original firmware claims about encryption, and its own
    // reset command over Bluetooth: next to findings not yet disclosed
    ("ll_ctrl_feature_used", SECURITY),
    ("ll_ctrl_peripheral_terminate", SECURITY),
    ("ll_ctrl_peripheral_terminate_timer", SECURITY),
    ("NWK-SEC-", SECURITY),
    ("NWK-REJOIN-", SECURITY),
    ("PAR-JOIN-", SECURITY),
    ("PAR-REJOIN-", SECURITY),
    ("BDB-JOIN-02", SECURITY),
    ("smp_pairing", SECURITY),
    ("ll_ctrl_enc_req", SECURITY),
    ("NWK-LEAVE-", ACCESS),
    ("ZDO-LEAVE-", ACCESS),
    ("PAR-REMOVE-", ACCESS),
    ("ZDO-BIND-", ACCESS),
    ("ZCL-BASIC-05", ACCESS),
];

fn withheld(module: &str, id: &str) -> Option<&'static str> {
    WITHHELD.iter().find(|(k, _)| module == *k || id == *k || (k.ends_with('-') && id.starts_with(k))).map(|(_, c)| *c)
}

/// The area a test counts in on the page's summary.
fn area(module: &str, id: &str) -> &'static str {
    if let Some(c) = withheld(module, id) {
        return if module.starts_with("smp") || module.starts_with("ll_") {
            "Bluetooth pairing and encryption"
        } else if c == SECURITY {
            "Zigbee security"
        } else {
            "Leaving, removal and binding"
        };
    }
    match module {
        "nwk_routing" | "nwk_network" => "Network layer",
        "parent_role" => "Parent role (end devices)",
        "aps_data" => "Application support (APS)",
        "zdo_services" => "Device and service discovery (ZDO)",
        "bdb_join" => "Joining (Base Device Behavior)",
        "zcl_foundation" => "ZCL: reading, writing, reporting",
        "zcl_basic_identify" => "ZCL: Basic and Identify",
        "zcl_onoff" | "zcl_level" => "ZCL: On/Off and Level Control",
        "zcl_color" => "ZCL: Color Control",
        "zcl_groups_scenes" => "ZCL: Groups and Scenes",
        "zcl_diagnostics" => "ZCL: Diagnostics",
        "misc_ota_gp" if id.starts_with("GP-") => "Green Power",
        "misc_ota_gp" => "OTA updates",
        m if m.starts_with("ll_") => "Bluetooth link layer",
        m if m.starts_with("l2cap") => "Bluetooth L2CAP",
        m if m.starts_with("att_") => "Bluetooth ATT",
        m if m.starts_with("gatt_") || m.starts_with("gap_") => "Bluetooth GATT and GAP",
        _ => "Other",
    }
}

fn stock_dir() -> Result<PathBuf> {
    let d = std::env::var_os("OUTGLOW_STOCK_DIR")
        .context("OUTGLOW_STOCK_DIR: the directory of the original firmware images (a checkout of outglow/original-firmware)")?;
    Ok(PathBuf::from(d))
}

fn git(root: &Path, args: &[&str]) -> String {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// The original images of a board: (manifest entry, path), every version the manifest lists.
fn stock_images(dir: &Path, manifest: &Value, board: &str) -> Vec<(Value, PathBuf)> {
    manifest["images"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|i| i["board"] == board)
                .filter_map(|i| {
                    let p = dir.join(i["file"].as_str().unwrap_or_default());
                    p.is_file().then(|| (i.clone(), p))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One suite on one board: the verdicts by image label and test id, and what every test checks.
fn run_suite(root: &Path, bin: &str, stock: &Path, outglow: &Path, a: &Args, tmp: &Path) -> Result<(Value, Value)> {
    let (res, meta) = (tmp.join(format!("{bin}-results.json")), tmp.join(format!("{bin}-tests.json")));
    let mut cmd = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    cmd.current_dir(root)
        .args(["test", "--manifest-path", "emulator/rs/Cargo.toml", "-p", "scenarios", "--test", bin, "--", "--matrix", "--json"])
        .arg(&res)
        .arg("--meta-json")
        .arg(&meta)
        .arg("--image")
        .arg(format!("stock={}", stock.display()))
        .arg("--image")
        .arg(format!("outglow={}", outglow.display()))
        .args(["--cpi", &a.cpi.to_string()]);
    if let Some(j) = a.jobs {
        cmd.args(["--jobs", &j.to_string()]);
    }
    println!("== {bin} tests: {} and {}", stock.display(), outglow.display());
    let st = cmd.status().with_context(|| format!("{cmd:?}"))?;
    if !st.success() {
        bail!("the {bin} tests did not run through ({st}): an image did not start, or the emulator stopped");
    }
    let read = |p: &Path| -> Result<Value> { Ok(serde_json::from_slice(&std::fs::read(p).with_context(|| p.display().to_string())?)?) };
    Ok((read(&res)?, read(&meta)?))
}

/// An observation as the page shows it: no paths of this machine, not too long.
fn note(obs: &str, root: &Path) -> String {
    let mut s = obs.replace(&root.display().to_string(), "outglow");
    if let Some(h) = std::env::var_os("HOME") {
        s = s.replace(&PathBuf::from(h).display().to_string(), "~");
    }
    if s.chars().count() > 400 {
        s = s.chars().take(400).collect::<String>() + "…";
    }
    s
}

pub fn run(a: Args) -> Result<()> {
    let root = crate::root();
    let out = a.out.clone().unwrap_or_else(|| root.join("site/public/compare.json"));
    let stock_dir = stock_dir()?;
    let tmp = root.join("target/compare");
    std::fs::create_dir_all(&tmp)?;

    let manifest: Value = serde_json::from_slice(
        &std::fs::read(stock_dir.join("manifest.json")).context("manifest.json of OUTGLOW_STOCK_DIR")?,
    )?;

    // one target per (board, original version)
    let mut targets = Vec::new();
    // test id -> (suite, its description)
    let mut tests: Vec<(String, &str, Value)> = Vec::new();
    // (target, verdicts by "label:id")
    let mut results: Vec<(String, Value)> = Vec::new();
    for (bid, board, name) in BOARDS {
        let stocks = stock_images(&stock_dir, &manifest, board);
        if stocks.is_empty() {
            println!("== {board}: no original image in {}, left out", stock_dir.display());
            continue;
        }
        let dir = if a.no_build { crate::build::default_dir(&root, board) } else { crate::build::board_default(board)? };
        let outglow = dir.join("bulb-tc32.bin");
        let known = outglow_image::board::known(board).with_context(|| format!("board {board}"))?;
        let several = stocks.len() > 1;
        for (entry, stock) in &stocks {
            let version = entry["firmware_version"].as_str().unwrap_or("unknown").to_string();
            let tid = if several { format!("{bid}-{version}") } else { bid.to_string() };
            targets.push(json!({
                "id": tid,
                "board": board,
                "name": name,
                "stock": {
                    "firmware_version": version,
                    "hw_revision": entry["hw_revision"],
                    "sha256": crate::index::hex::<Sha256>(&std::fs::read(stock)?),
                    "source": format!("outglow/original-firmware, {}", entry["file"].as_str().unwrap_or_default()),
                    "from": manifest["source"]["app"],
                    "app_version": manifest["source"]["version"],
                },
                "outglow": {
                    "image_type": format!("{:#06x}", known.image_type),
                    "sha256": crate::index::hex::<Sha256>(&std::fs::read(&outglow)?),
                },
            }));
            let mut merged = Map::new();
            for (sid, bin, _) in SUITES {
                let work = tmp.join(&tid);
                std::fs::create_dir_all(&work)?;
                let (res, meta) = run_suite(&root, bin, stock, &outglow, &a, &work)?;
                for t in meta.as_array().context("test list")? {
                    let id = t["id"].as_str().unwrap_or_default().to_string();
                    if !tests.iter().any(|(i, _, _)| *i == id) {
                        tests.push((id.clone(), sid, t.clone()));
                    }
                }
                for label in ["stock", "outglow"] {
                    if let Some(m) = res[label].as_object() {
                        for (id, v) in m {
                            merged.entry(format!("{label}:{id}")).or_insert(v.clone());
                        }
                    }
                }
            }
            results.push((tid, Value::Object(merged)));
        }
    }
    if targets.is_empty() {
        bail!("no original image of {} in {}", BOARDS.map(|b| b.1).join(" or "), stock_dir.display());
    }

    let mut list = Vec::new();
    for (id, suite, t) in &tests {
        let module = t["module"].as_str().unwrap_or_default();
        let held = withheld(module, id);
        let mut per_board = Map::new();
        for (bid, r) in &results {
            let mut e = Map::new();
            for label in ["stock", "outglow"] {
                let v = &r[format!("{label}:{id}")];
                let verdict = v["verdict"].as_str().unwrap_or("ERROR");
                if label == "stock" && held.is_some() {
                    e.insert(label.into(), json!("WITHHELD"));
                    continue;
                }
                e.insert(label.into(), json!(verdict));
                let obs = v["obs"].as_str().unwrap_or_default();
                if verdict != "PASS" && !obs.is_empty() {
                    e.insert(format!("{label}_note"), json!(note(obs, &root)));
                }
            }
            per_board.insert(bid.to_string(), Value::Object(e));
        }
        list.push(json!({
            "id": id,
            "suite": suite,
            "module": module,
            "area": area(module, id),
            "doc": t["doc"],
            "clause": t["clause"],
            "requirement": t["req"],
            "level": t["level"],
            "mandatory": t["mandatory"],
            "ambiguous": t["ambiguous"],
            "withheld": held,
            "results": per_board,
        }));
    }

    let commit = git(&root, &["rev-parse", "--short=7", "HEAD"]);
    let dirty = !git(&root, &["status", "--porcelain", "--", "crates", "emulator", "Cargo.lock"]).is_empty();
    let commit = if dirty { format!("{commit}+changes") } else { commit };
    let data = json!({
        "schema": 1,
        "generated": crate::index::today(),
        "how": "cargo xtask compare: the conformance tests of emulator/rs/scenarios (cargo spec, cargo ble-spec) run in the Outglow emulator on the original firmware images and on Outglow images built from this repository",
        "source": "emulator",
        "outglow": {
            "commit": commit,
            "commit_date": git(&root, &["log", "-1", "--format=%cs"]),
            "describe": git(&root, &["describe", "--tags", "--match", "v*", "--always"]),
            "url": format!("https://github.com/outglow/outglow/commit/{}", commit.trim_end_matches("+changes")),
        },
        "cpi": a.cpi,
        "suites": SUITES.iter().map(|(id, _, what)| json!({"id": id, "covers": what})).collect::<Vec<_>>(),
        "withheld": {
            "security": "Security: network and link keys, frame security, rejoining with keys, Bluetooth pairing and encryption",
            "access": "Access control: who may make a bulb leave the network, reset it or change its bindings",
            "policy": "The original firmware's results of these tests are not published, whether it passes them or not. Outglow's are.",
        },
        "verdicts": {
            "PASS": "behaves as the clause requires",
            "FAIL": "does not",
            "NOTE": "a recommendation (should) not followed, or a failure that depends on how an ambiguous clause is read",
            "INCONCLUSIVE": "the test could not provoke or observe the behaviour on this image",
            "N/A": "the clause does not apply: the image lacks the optional feature",
            "ERROR": "the emulator could not run the test",
            "WITHHELD": "not published (see withheld)",
        },
        "targets": targets,
        "tests": list,
    });
    std::fs::write(&out, serde_json::to_string_pretty(&data)? + "\n")?;
    let n = data["tests"].as_array().map(|v| v.len()).unwrap_or(0);
    println!("== {}: {n} tests on {} original images", out.display(), data["targets"].as_array().map(|v| v.len()).unwrap_or(0));
    Ok(())
}
