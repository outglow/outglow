use memcap as _;

mod sim;

mod aps;
mod clusters;
mod colour;
mod conflicts;
mod diagnostics;
mod fragments;
mod frames;
mod green_power;
mod hal_defaults;
mod mac_rx;
mod network;
mod nv;
mod objects;
mod ota;
mod parent;
mod persistence;
mod sense;
mod settings;
mod touchlink;
mod zcl;
mod zdo;

#[test]
fn every_file_of_tests_is_a_module_of_this_binary() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let main = std::fs::read_to_string(dir.join("main.rs")).unwrap();
    let mut files: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.unwrap().file_name().into_string().ok())
        .filter_map(|n| n.strip_suffix(".rs").map(str::to_string))
        .filter(|n| n != "main")
        .collect();
    files.sort();
    let missing: Vec<&String> = files.iter().filter(|n| !main.lines().any(|l| l == format!("mod {n};"))).collect();
    assert!(missing.is_empty(), "tests/main.rs does not declare {missing:?}: autotests is off, so they would not be built");
}
