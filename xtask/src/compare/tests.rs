use std::cell::RefCell;

use super::*;
use crate::sh::fake::{env, program, Fake, Tmp};

fn args(extra: &[&str]) -> Args {
    use clap::Parser;
    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        a: Args,
    }
    let mut v = vec!["compare"];
    v.extend_from_slice(extra);
    Cli::try_parse_from(v).unwrap().a
}

#[test]
fn withheld_tests_and_their_areas() {
    assert_eq!(withheld("security_keys", "X"), Some(SECURITY));
    assert_eq!(withheld("nwk_network", "NWK-SEC-03"), Some(SECURITY));
    assert_eq!(withheld("zcl_basic_identify", "ZCL-BASIC-05"), Some(ACCESS));
    assert_eq!(withheld("zcl_basic_identify", "ZCL-BASIC-051"), None);
    assert_eq!(withheld("nwk_network", "XNWK-SEC-03"), None);
    assert_eq!(withheld("nwk_network", "NWK-LEAVE-"), Some(ACCESS));
    let areas = [
        ("smp_pairing", "S", "Bluetooth pairing and encryption"),
        ("ll_ctrl_enc_req", "L", "Bluetooth pairing and encryption"),
        ("x", "NWK-SEC-1", "Zigbee security"),
        ("x", "ZDO-BIND-1", "Leaving, removal and binding"),
        ("nwk_routing", "a", "Network layer"),
        ("nwk_network", "a", "Network layer"),
        ("parent_role", "a", "Parent role (end devices)"),
        ("aps_data", "a", "Application support (APS)"),
        ("zdo_services", "a", "Device and service discovery (ZDO)"),
        ("bdb_join", "a", "Joining (Base Device Behavior)"),
        ("zcl_foundation", "a", "ZCL: reading, writing, reporting"),
        ("zcl_basic_identify", "a", "ZCL: Basic and Identify"),
        ("zcl_onoff", "a", "ZCL: On/Off and Level Control"),
        ("zcl_level", "a", "ZCL: On/Off and Level Control"),
        ("zcl_color", "a", "ZCL: Color Control"),
        ("zcl_groups_scenes", "a", "ZCL: Groups and Scenes"),
        ("zcl_diagnostics", "a", "ZCL: Diagnostics"),
        ("misc_ota_gp", "GP-1", "Green Power"),
        ("misc_ota_gp", "OTA-1", "OTA updates"),
        ("ll_adv", "a", "Bluetooth link layer"),
        ("l2cap_sig", "a", "Bluetooth L2CAP"),
        ("att_read", "a", "Bluetooth ATT"),
        ("gatt_db", "a", "Bluetooth GATT and GAP"),
        ("gap_name", "a", "Bluetooth GATT and GAP"),
        ("other", "a", "Other"),
    ];
    for (m, id, want) in areas {
        assert_eq!(area(m, id), want, "{m} {id}");
    }
}

#[test]
fn the_compared_boards_are_known() {
    for (_, board, _) in BOARDS {
        assert!(outglow_image::board::known(board).is_some(), "{board}");
    }
}

#[test]
fn notes_hide_where_the_files_are() {
    let root = Path::new("/w/outglow");
    assert_eq!(note("at /w/outglow/x and /home/u/y", root, Some("/home/u".into())), "at outglow/x and ~/y");
    assert_eq!(note("/home/u", root, None), "/home/u");
    let long = "é".repeat(401);
    assert_eq!(note(&long, root, None), format!("{}…", "é".repeat(400)));
    assert_eq!(note(&"a".repeat(400), root, None), "a".repeat(400));
}

#[test]
fn the_original_images_of_a_board() {
    let t = Tmp::new("stock");
    t.put("a.bin", "x");
    std::fs::create_dir_all(t.path("dir.bin")).unwrap();
    let m = serde_json::json!({"images": [
        {"board": "B", "file": "a.bin"},
        {"board": "B", "file": "gone.bin"},
        {"board": "B", "file": "dir.bin"},
        {"board": "B"},
        {"board": "C", "file": "a.bin"},
    ]});
    let got = stock_images(&t.0, &m, "B");
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].1, t.path("a.bin"));
    assert!(stock_images(&t.0, &serde_json::json!({}), "B").is_empty());
}

struct Rig {
    t: Tmp,
    stock: Tmp,
}

const META: &str = r#"[
    {"id": "NWK-SEC-01", "module": "nwk_network", "doc": "R22", "clause": "4.3", "req": "secure", "level": "shall", "mandatory": true, "ambiguous": false},
    {"id": "ZCL-ONOFF-01", "module": "zcl_onoff", "doc": "ZCL8", "clause": "3.8", "req": "toggle", "level": "shall", "mandatory": true, "ambiguous": false},
    {"id": "LL-01", "module": "ll_adv"}
]"#;

impl Rig {
    fn new() -> Rig {
        let t = Tmp::new("compare");
        let stock = Tmp::new("compare-stock");
        stock.put("dimm-302.bin", "stock 3.0.2");
        stock.put("dimm-303.bin", "stock 3.0.3");
        stock.put("tw.bin", "stock tw");
        stock.put(
            "manifest.json",
            r#"{"source": {"app": "AwoX", "version": "9"}, "images": [
                {"board": "ESMLFzm_w6_Dimm-4.63", "file": "dimm-302.bin", "firmware_version": "3.0.2", "hw_revision": "4.63"},
                {"board": "ESMLFzm_w6_Dimm-4.63", "file": "dimm-303.bin", "hw_revision": "4.63"},
                {"board": "ESMLFzm_w6_TW-4.63", "file": "tw.bin", "firmware_version": "3.0.2", "hw_revision": "4.63"}
            ]}"#,
        );
        t.put("target/tc32/bulb-tc32.bin", "outglow dimm");
        t.put("target/tc32-tw/bulb-tc32.bin", "outglow tw");
        Rig { t, stock }
    }

    fn vars(&self) -> Vec<(String, String)> {
        vec![("OUTGLOW_STOCK_DIR".into(), self.stock.0.display().to_string()), ("HOME".into(), "/home/u".into()), ("CARGO".into(), "/bin/cargo".into())]
    }

    fn go(&self, vars: &[(String, String)], a: &Args, answer: &mut dyn FnMut(&Command) -> (i32, String)) -> (Result<()>, Vec<String>) {
        let v: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let e = env(&v);
        let mut fake = Fake::new(|c| answer(c));
        let mut c = Ctx { root: self.t.0.clone(), sh: &mut fake, env: &e, exe: "/x".into(), toolchain: "/t".into() };
        let r = run_in(&mut c, a);
        (r, fake.log)
    }
}

fn arg_after(c: &Command, flag: &str) -> PathBuf {
    let a: Vec<_> = c.get_args().collect();
    PathBuf::from(a[a.iter().position(|x| *x == flag).unwrap() + 1])
}

fn emulator(c: &Command, home: &str, dirty: &str) -> (i32, String) {
    let args: Vec<String> = c.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
    match (program(c).as_str(), args.get(2).map(String::as_str)) {
        ("git", Some("rev-parse")) => (0, "abc1234\n".into()),
        ("git", Some("status")) => (0, dirty.into()),
        ("git", Some("log")) => (0, "2026-11-17\n".into()),
        ("git", _) => (128, "no tags".into()),
        _ => {
            let bin = &args[6];
            let obs = format!("{home}/oss/x failed at {}", c.get_current_dir().unwrap().display());
            let res = serde_json::json!({
                "stock": {"NWK-SEC-01": {"verdict": "FAIL", "obs": "secret"}, "ZCL-ONOFF-01": {"verdict": "FAIL", "obs": obs}, "LL-01": {"verdict": "PASS", "obs": "x"}},
                "outglow": {"NWK-SEC-01": {"verdict": "PASS"}, "ZCL-ONOFF-01": {"verdict": "NOTE", "obs": ""}},
            });
            let meta = if bin == "spec" { META.to_string() } else { "[]".to_string() };
            std::fs::write(arg_after(c, "--json"), res.to_string()).unwrap();
            std::fs::write(arg_after(c, "--meta-json"), meta).unwrap();
            (0, String::new())
        }
    }
}

#[test]
fn the_comparison_of_both_boards() {
    let r = Rig::new();
    let out = r.t.path("compare.json");
    let a = args(&["--no-build", "--out", out.to_str().unwrap(), "--jobs", "2", "--cpi", "24"]);
    let (res, log) = r.go(&r.vars(), &a, &mut |c| emulator(c, "/home/u", ""));
    res.unwrap();
    let root = r.t.0.display().to_string();
    let suites: Vec<&String> = log.iter().filter(|l| l.contains(" test ")).collect();
    assert_eq!(suites.len(), 6);
    assert_eq!(
        suites[0].replace(&root, "ROOT").replace(&r.stock.0.display().to_string(), "STOCK"),
        "cd ROOT && /bin/cargo test --manifest-path emulator/rs/Cargo.toml -p scenarios --test spec -- --matrix --json \
         ROOT/target/compare/dimm-3.0.2/spec-results.json --meta-json ROOT/target/compare/dimm-3.0.2/spec-tests.json \
         --image stock=STOCK/dimm-302.bin --image outglow=ROOT/target/tc32/bulb-tc32.bin --cpi 24 --jobs 2"
    );
    assert!(suites[1].contains("--test ble -- "));
    assert!(suites[2].contains("/dimm-unknown/"));
    assert!(suites[4].contains("/target/compare/tw/spec-results.json"));
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(v["schema"], 1);
    assert_eq!(v["cpi"], 24);
    assert_eq!(v["outglow"]["commit"], "abc1234");
    assert_eq!(v["outglow"]["url"], "https://github.com/outglow/outglow/commit/abc1234");
    assert_eq!(v["outglow"]["commit_date"], "2026-11-17");
    assert_eq!(v["outglow"]["describe"], "");
    assert_eq!(v["generated"].as_str().unwrap().len(), 10);
    let ids: Vec<&str> = v["targets"].as_array().unwrap().iter().map(|t| t["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["dimm-3.0.2", "dimm-unknown", "tw"]);
    let t0 = &v["targets"][0];
    assert_eq!(t0["stock"]["firmware_version"], "3.0.2");
    assert_eq!(t0["stock"]["source"], "outglow/original-firmware, dimm-302.bin");
    assert_eq!(t0["stock"]["from"], "AwoX");
    assert_eq!(t0["stock"]["app_version"], "9");
    assert_eq!(t0["stock"]["sha256"], crate::index::hex::<Sha256>(b"stock 3.0.2"));
    assert_eq!(t0["outglow"]["sha256"], crate::index::hex::<Sha256>(b"outglow dimm"));
    assert_eq!(t0["outglow"]["image_type"], "0x0390");
    assert_eq!(v["targets"][2]["outglow"]["image_type"], "0x0391");
    let tests = v["tests"].as_array().unwrap();
    assert_eq!(tests.len(), 3);
    let sec = &tests[0];
    assert_eq!((sec["suite"].as_str(), sec["area"].as_str(), sec["withheld"].as_str()), (Some("zigbee"), Some("Zigbee security"), Some("security")));
    assert_eq!(sec["results"]["tw"], serde_json::json!({"stock": "WITHHELD", "outglow": "PASS"}));
    let onoff = &tests[1];
    assert_eq!(onoff["requirement"], "toggle");
    assert_eq!(onoff["withheld"], Value::Null);
    let tw = &onoff["results"]["tw"];
    assert_eq!(tw["stock"], "FAIL");
    assert_eq!(tw["stock_note"], format!("~/oss/x failed at outglow"));
    assert_eq!(tw["outglow"], "NOTE");
    assert!(tw.get("outglow_note").is_none());
    let ll = &tests[2]["results"]["tw"];
    assert_eq!(*ll, serde_json::json!({"stock": "PASS", "outglow": "ERROR"}));
}

#[test]
fn results_of_one_image_only_and_the_default_cargo() {
    let r = Rig::new();
    r.stock.put("manifest.json", r#"{"images": [{"board": "ESMLFzm_w6_TW-4.63", "file": "tw.bin"}]}"#);
    let out = r.t.path("c.json");
    let a = args(&["--no-build", "--out", out.to_str().unwrap()]);
    let vars: Vec<(String, String)> = r.vars().into_iter().filter(|(k, _)| k != "CARGO").collect();
    let mut only_stock = |c: &Command| {
        if program(c) == "git" {
            return (0, String::new());
        }
        std::fs::write(arg_after(c, "--json"), r#"{"stock": {"LL-01": {"verdict": "PASS"}}}"#).unwrap();
        std::fs::write(arg_after(c, "--meta-json"), r#"[{"id": "LL-01", "module": "ll_adv"}]"#).unwrap();
        (0, String::new())
    };
    let (res, log) = r.go(&vars, &a, &mut only_stock);
    res.unwrap();
    assert!(log.iter().any(|l| l.contains(" cargo test --manifest-path ")), "{log:?}");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(v["tests"][0]["results"]["tw"], serde_json::json!({"stock": "PASS", "outglow": "ERROR"}));
}

#[test]
fn a_changed_tree_and_a_single_original_image() {
    let r = Rig::new();
    r.stock.put("manifest.json", r#"{"images": [{"board": "ESMLFzm_w6_TW-4.63", "file": "tw.bin"}]}"#);
    std::fs::create_dir_all(r.t.path("site/public")).unwrap();
    let a = args(&["--no-build"]);
    let (res, log) = r.go(&r.vars(), &a, &mut |c| emulator(c, "/home/u", " M crates/x.rs\n"));
    res.unwrap();
    assert!(log.iter().all(|l| !l.contains("--jobs")));
    assert!(log.iter().any(|l| l.ends_with("--cpi 3")));
    let v: Value = serde_json::from_str(&std::fs::read_to_string(r.t.path("site/public/compare.json")).unwrap()).unwrap();
    assert_eq!(v["outglow"]["commit"], "abc1234+changes");
    assert_eq!(v["outglow"]["url"], "https://github.com/outglow/outglow/commit/abc1234");
    assert_eq!(v["targets"][0]["id"], "tw");
    assert_eq!(v["targets"][0]["stock"]["firmware_version"], "unknown");
}

#[test]
fn what_stops_a_comparison() {
    let r = Rig::new();
    let a = args(&["--no-build", "--out", r.t.path("c.json").to_str().unwrap()]);
    let none: Vec<(String, String)> = vec![];
    let e = r.go(&none, &a, &mut |c| emulator(c, "", "")).0.unwrap_err().to_string();
    assert!(e.starts_with("OUTGLOW_STOCK_DIR: "), "{e}");
    r.stock.put("manifest.json", r#"{"images": []}"#);
    let e = r.go(&r.vars(), &a, &mut |c| emulator(c, "", "")).0.unwrap_err().to_string();
    assert_eq!(e, format!("no original image of ESMLFzm_w6_Dimm-4.63 or ESMLFzm_w6_TW-4.63 in {}", r.stock.0.display()));
    std::fs::remove_file(r.stock.path("manifest.json")).unwrap();
    let e = r.go(&r.vars(), &a, &mut |c| emulator(c, "", "")).0.unwrap_err().to_string();
    assert_eq!(e, "manifest.json of OUTGLOW_STOCK_DIR");
    let r = Rig::new();
    let calls = RefCell::new(0);
    let e = r
        .go(&r.vars(), &a, &mut |_| {
            *calls.borrow_mut() += 1;
            (1, String::new())
        })
        .0
        .unwrap_err()
        .to_string();
    assert_eq!(e, "the spec tests did not run through (exit status: 1): an image did not start, or the emulator stopped");
    let e = r.go(&r.vars(), &a, &mut |_| (0, String::new())).0.unwrap_err().to_string();
    assert!(e.ends_with("/target/compare/dimm-3.0.2/spec-results.json"), "{e}");
    let mut odd = |c: &Command| {
        std::fs::write(arg_after(c, "--json"), "{}").unwrap();
        std::fs::write(arg_after(c, "--meta-json"), "{}").unwrap();
        (0, String::new())
    };
    let e = r.go(&r.vars(), &a, &mut odd).0.unwrap_err().to_string();
    assert_eq!(e, "test list");
    std::fs::remove_file(r.t.path("target/tc32/bulb-tc32.bin")).unwrap();
    let e = r.go(&r.vars(), &a, &mut |c| emulator(c, "", "")).0.unwrap_err().to_string();
    assert!(e.starts_with("No such file"), "{e}");
    r.t.put("target/tc32/bulb-tc32.bin", "outglow dimm");
    let mut real = crate::sh::Real;
    let v = r.vars();
    let e2 = env(&[(v[0].0.as_str(), v[0].1.as_str()), ("CARGO", "/nonexistent/cargo")]);
    let mut c = Ctx { root: r.t.0.clone(), sh: &mut real, env: &e2, exe: "/x".into(), toolchain: "/t".into() };
    let e = run_in(&mut c, &a).unwrap_err().to_string();
    assert!(e.starts_with("cd ") && e.contains(" \"/nonexistent/cargo\" \"test\" "), "{e}");
    let mut vars = r.vars();
    vars.push(("TC32_LLVM".into(), "/nonexistent".into()));
    let e = r.go(&vars, &args(&[]), &mut |c| emulator(c, "", "")).0.unwrap_err().to_string();
    assert_eq!(e, "no clang in /nonexistent: run `cargo xtask toolchain` (downloads llvm-tc32) or set TC32_LLVM");
    r.stock.put("manifest.json", "nope");
    let e = r.go(&r.vars(), &a, &mut |c| emulator(c, "", "")).0.unwrap_err().to_string();
    assert_eq!(e, "expected ident at line 1 column 2");
}
