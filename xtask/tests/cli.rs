use memcap as _;

use std::path::PathBuf;
use std::process::{Command, Output};

use outglow_image::{ota, telink};

struct Tmp(PathBuf);

impl Tmp {
    fn new(name: &str) -> Tmp {
        let d = std::env::temp_dir().join(format!("xtask-cli-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Tmp(d)
    }

    fn path(&self, f: &str) -> PathBuf {
        self.0.join(f)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn xtask(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_xtask"));
    c.args(args).env_remove("OUTGLOW_RUSTC_WRAPPER").env_remove("OUTGLOW_STOCK_DIR").env_remove("CARGO").env_remove("ONLY");
    for (k, v) in env {
        c.env(k, v);
    }
    c.output().unwrap()
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

#[test]
fn as_a_rustc_wrapper_it_runs_the_compiler() {
    let o = xtask(&["sh", "-c", "echo \"$@\"; exit 5", "x", "--crate-name", "c", "--target", "thumbv4t-none-eabi", "-Cmetadata=1"], &[("OUTGLOW_RUSTC_WRAPPER", "1")]);
    assert_eq!(o.status.code(), Some(5));
    assert_eq!(text(&o.stdout), "--crate-name c --target thumbv4t-none-eabi -Cmetadata=outglow-c\n");
}

#[test]
fn tasks_that_only_read() {
    let o = xtask(&["boards", "--check"], &[]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    assert_eq!(text(&o.stdout), "21 boards, all files up to date\n");
    let o = xtask(&["comments"], &[]);
    assert!(o.status.success(), "{}", text(&o.stdout));
    assert_eq!(text(&o.stdout), "no comments in Rust sources\n");
    let o = xtask(&["mutants", "--list", "ble"], &[]);
    assert!(o.status.success());
    assert!(text(&o.stdout).lines().last().unwrap().starts_with("ble: "));
    let t = Tmp::new("toolchain");
    let short = &include_str!("../llvm-tc32.ref").trim()[..7];
    let bin = t.path(&format!("outglow/llvm-tc32-{short}/bin"));
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("clang"), "").unwrap();
    let o = xtask(&["toolchain", "--print-path"], &[("XDG_CACHE_HOME", t.0.to_str().unwrap())]);
    assert!(o.status.success());
    assert_eq!(text(&o.stdout), format!("{}\n", bin.display()));
    let o = xtask(&["--help"], &[]);
    assert!(text(&o.stdout).contains("mutants-report"));
}

#[test]
fn tasks_that_stop_before_any_tool() {
    let o = xtask(&["build", "--board", "Nope-4.60"], &[]);
    assert!(!o.status.success());
    assert!(text(&o.stderr).starts_with("Error: unknown board Nope-4.60;"), "{}", text(&o.stderr));
    let o = xtask(&["compare", "--no-build"], &[]);
    assert!(text(&o.stderr).starts_with("Error: OUTGLOW_STOCK_DIR: "), "{}", text(&o.stderr));
    let t = Tmp::new("coverage");
    let target = t.path("target");
    let o = xtask(&["coverage", "--no-run"], &[("CARGO", "/nonexistent/cargo"), ("CARGO_TARGET_DIR", target.to_str().unwrap())]);
    assert!(text(&o.stderr).starts_with("Error: starting cargo llvm-cov"), "{}", text(&o.stderr));
}

#[test]
fn tasks_that_write() {
    let t = Tmp::new("write");
    let mut body = vec![0u8; 0x400];
    body[8..12].copy_from_slice(b"KNLT");
    let tag = b"outglow-board:ESMLFzm_w6_Dimm-4.63\0";
    body[0x100..0x100 + tag.len()].copy_from_slice(tag);
    let img = telink::finalize(&body);
    let f = t.path("bulb-tc32.ota");
    std::fs::write(&f, ota::File::with_image(ota::Header::new(0x1141, 0x0390, 1, "x"), img).pack()).unwrap();
    let index = t.path("index.json");
    let o = xtask(&["index", f.to_str().unwrap(), "--version", "9.9.9", "--index", index.to_str().unwrap()], &[]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    assert!(std::fs::read_to_string(&index).unwrap().contains("outglow-ESMLFzm_w6_Dimm-4.63-9.9.9.ota"));
    let o = xtask(&["release-notes", "--version", "9.9.9", "--assets", t.0.to_str().unwrap()], &[]);
    assert!(text(&o.stdout).starts_with("Outglow 9.9.9 for 21 boards."));
    let out = t.path("outcomes.json");
    std::fs::write(&out, r#"{"outcomes": []}"#).unwrap();
    let o = xtask(&["mutants-report", out.to_str().unwrap()], &[]);
    assert_eq!(text(&o.stdout), "## Mutation testing (cargo mutants)\n\nNo mutants.\n");
}
