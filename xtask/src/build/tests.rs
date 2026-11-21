use std::cell::RefCell;
use std::path::Path;

use super::*;
use crate::sh::fake::{describe, env, program, Fake, Tmp};

const PINNED_VV: &str = "rustc 1.99.0 (b940084d7 2026-10-01)\nbinary: rustc\ncommit-hash: b940084d7eb6a299eb4bfeb8e34901bc051e7ac4\nrelease: 1.99.0\nLLVM version: 21.1.8\n";

const ATTRIBUTES: &[u8] = &[
    0x41, 0x1d, 0x00, 0x00, 0x00, 0x61, 0x65, 0x61, 0x62, 0x69, 0x00, 0x01, 0x13, 0x00, 0x00, 0x00, 0x05, 0x74, 0x63, 0x33,
    0x32, 0x00, 0x06, 0x02, 0x08, 0x00, 0x09, 0x01, 0x22, 0x00,
];

fn tc32_elf(code: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; 0x34 + 32];
    out[..6].copy_from_slice(b"\x7fELF\x01\x01");
    out[0x1C..0x20].copy_from_slice(&0x34u32.to_le_bytes());
    out[0x2A..0x2C].copy_from_slice(&32u16.to_le_bytes());
    out[0x2C..0x2E].copy_from_slice(&1u16.to_le_bytes());
    let at = out.len() as u32;
    out.extend_from_slice(code);
    out[0x34..0x38].copy_from_slice(&1u32.to_le_bytes());
    out[0x38..0x3C].copy_from_slice(&at.to_le_bytes());
    out[0x44..0x48].copy_from_slice(&(code.len() as u32).to_le_bytes());
    let attrs_at = out.len() as u32;
    out.extend_from_slice(ATTRIBUTES);
    let shoff = out.len();
    out.extend_from_slice(&[0u8; 40]);
    out[shoff + 4..shoff + 8].copy_from_slice(&0x7000_0003u32.to_le_bytes());
    out[shoff + 16..shoff + 20].copy_from_slice(&attrs_at.to_le_bytes());
    out[shoff + 20..shoff + 24].copy_from_slice(&(ATTRIBUTES.len() as u32).to_le_bytes());
    out[0x20..0x24].copy_from_slice(&(shoff as u32).to_le_bytes());
    out[0x2E..0x30].copy_from_slice(&40u16.to_le_bytes());
    out[0x30..0x32].copy_from_slice(&1u16.to_le_bytes());
    out
}

fn code() -> Vec<u8> {
    let mut c = vec![0u8; 64];
    c[8..12].copy_from_slice(b"KNLT");
    c
}

fn args(extra: &[&str]) -> Args {
    use clap::Parser;
    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        a: Args,
    }
    let mut v = vec!["build"];
    v.extend_from_slice(extra);
    Cli::try_parse_from(v).unwrap().a
}

fn arg_after(c: &Command, flag: &str) -> PathBuf {
    let a: Vec<_> = c.get_args().collect();
    PathBuf::from(a[a.iter().position(|x| *x == flag).unwrap() + 1])
}

fn env_of(c: &Command, k: &str) -> PathBuf {
    PathBuf::from(c.get_envs().find(|(n, _)| *n == k).and_then(|(_, v)| v).unwrap())
}

struct Toolchain {
    vv: String,
    machine_code: bool,
    deps_bc: bool,
    elf: Vec<u8>,
    fail: &'static str,
    fail_ending: Option<&'static str>,
}

impl Toolchain {
    fn good() -> Toolchain {
        Toolchain { vv: PINNED_VV.into(), machine_code: true, deps_bc: true, elf: tc32_elf(&code()), fail: "", fail_ending: None }
    }

    fn answer(&self, c: &Command) -> (i32, String) {
        let p = program(c);
        if p == self.fail || self.fail_ending.is_some_and(|e| describe(c).ends_with(e)) {
            return (1, "broken".into());
        }
        let first = c.get_args().next().map(|a| a.to_string_lossy().into_owned()).unwrap_or_default();
        match (p.as_str(), first.as_str()) {
            ("rustc", "-vV") => (0, self.vv.clone()),
            ("rustc", _) => (0, "/nonexistent/sysroot\n".into()),
            ("sh", _) => (0, "/usr/bin/rustc\n".into()),
            ("cargo", _) => {
                let release = env_of(c, "CARGO_TARGET_DIR").join(TARGET).join("release");
                std::fs::create_dir_all(release.join("deps")).unwrap();
                std::fs::write(release.join("libbulb_tc32.a"), "!<arch>").unwrap();
                if self.deps_bc {
                    std::fs::write(release.join("deps/compiler_builtins-9.bc"), "BC").unwrap();
                }
                (0, String::new())
            }
            ("clang", "--version") => (0, "clang version 23.1.2 (llvm-tc32)\nTarget: arm\n".into()),
            ("llvm-ar", _) => {
                let d = c.get_current_dir().unwrap();
                std::fs::write(d.join("bulb_tc32-1.b.rcgu.o"), "BC\u{c0}\u{de}").unwrap();
                std::fs::write(d.join("bulb_tc32-1.a.rcgu.o"), "BC\u{c0}\u{de}").unwrap();
                std::fs::write(d.join("lib.rmeta"), "x").unwrap();
                if self.machine_code {
                    std::fs::write(d.join("compiler_builtins-9.x1.o"), "\x7fELF").unwrap();
                    std::fs::write(d.join("compiler_builtins-9.x2.o"), "\x7fELF").unwrap();
                }
                (0, String::new())
            }
            ("llvm-link", _) => {
                std::fs::write(arg_after(c, "-o"), "BC").unwrap();
                (0, String::new())
            }
            ("llvm-dis", _) => {
                std::fs::write(arg_after(c, "-o"), "attributes #0 = { \"target-cpu\"=\"generic\" }\n").unwrap();
                (0, String::new())
            }
            ("clang", _) => {
                std::fs::write(arg_after(c, "-o"), "obj").unwrap();
                (0, String::new())
            }
            ("ld.lld", _) => {
                std::fs::write(arg_after(c, "-o"), &self.elf).unwrap();
                (0, String::new())
            }
            ("llvm-size", _) => (0, String::new()),
            ("cp", "-Rp") => {
                let a: Vec<_> = c.get_args().collect();
                assert!(Path::new(a[1]).join("outglow.lock").exists());
                std::fs::create_dir_all(a[2]).unwrap();
                (0, String::new())
            }
            _ => (127, format!("unexpected {}", describe(c))),
        }
    }
}

struct Rig {
    t: Tmp,
    tools: Toolchain,
    vars: Vec<(String, String)>,
}

impl Rig {
    fn new(name: &str) -> Rig {
        let t = Tmp::new(name);
        t.put("llvm/clang", "");
        t.put("crates/bulb-tc32/boards.tsv", "# name\ttunable\timage_type\nESMLFzm_w6_Dimm-4.63\t0\t0x0390\n\nEGLO_ZM_TW-4.62\t1\t0x0395\n");
        let llvm = t.path("llvm").display().to_string();
        let vars = [("HOME", "/home/u"), ("RUST_LIB_SRC", "/rust/library"), ("TC32_LLVM", llvm.as_str()), ("CARGO", "/bin/cargo")];
        Rig { tools: Toolchain::good(), vars: vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(), t }
    }

    fn set(&mut self, k: &str, v: &str) {
        self.vars.retain(|(n, _)| n != k);
        self.vars.push((k.into(), v.into()));
    }

    fn unset(&mut self, k: &str) {
        self.vars.retain(|(n, _)| n != k);
    }

    fn go<T>(&self, f: impl FnOnce(&mut Ctx) -> Result<T>) -> (Result<T>, Vec<String>) {
        let vars: Vec<(&str, &str)> = self.vars.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let e = env(&vars);
        let mut fake = Fake::new(|c| self.tools.answer(c));
        let mut c = Ctx { root: self.t.0.clone(), sh: &mut fake, env: &e, exe: "/bin/xtask".into(), toolchain: self.t.path("cached") };
        let r = f(&mut c);
        let root = self.t.0.display().to_string();
        let log = fake.log.iter().map(|l| l.replace(&root, "ROOT")).collect();
        (r, log)
    }

    fn build(&self, extra: &[&str]) -> (Result<()>, Vec<String>) {
        let a = args(extra);
        self.go(|c| run_in(c, &a))
    }
}

fn program_of(line: &str) -> String {
    let cmd = line.rsplit(" && ").next().unwrap();
    let words: Vec<&str> = cmd.split(' ').collect();
    let at = words.iter().position(|w| w.starts_with('/') || w.starts_with("ROOT/") || *w == "cp").unwrap();
    words[at].rsplit('/').next().unwrap().to_string()
}

fn cargo_id(log: &[String]) -> String {
    let l = log.iter().find(|l| l.contains("CARGO_TARGET_DIR=")).unwrap();
    let at = l.find("/cargo-").unwrap() + 1;
    l[at..at + 22].to_string()
}

#[test]
fn a_build_runs_the_toolchain_in_order() {
    let r = Rig::new("build");
    let id = cache_id(PINNED_VV);
    std::fs::create_dir_all(r.t.path("target/tc32/cargo-0000000000000000")).unwrap();
    std::fs::create_dir_all(r.t.path("target/tc32/cargo")).unwrap();
    std::fs::create_dir_all(r.t.path("target/tc32-cargo/cargo-0000000000000000")).unwrap();
    std::fs::create_dir_all(r.t.path(&format!("target/tc32-cargo/cargo-noble-{id}"))).unwrap();
    r.t.put("target/tc32-cargo/notes", "");
    r.t.put("target/tc32/keep", "");
    r.t.put("target/tc32/cargo-file", "");
    r.t.put("target/tc32/bc/stale.o", "BC");
    let (res, log) = r.build(&[]);
    res.unwrap();
    assert_eq!(cargo_id(&log), format!("cargo-{id}"));
    assert_eq!(id.len(), 16);
    assert!(id.chars().all(|c| c.is_ascii_hexdigit()), "{id}");
    assert_ne!(cache_id("rustc 1.98.0"), id);
    let log: Vec<String> = log.iter().map(|l| l.replace(&id, "ID")).collect();
    let out = "ROOT/target/tc32";
    let rel = "ROOT/target/tc32-cargo/cargo-ID/thumbv4t-none-eabi/release";
    let flags = "--target=thumbv4t-none-eabi -mcpu=tc32 -mthumb -mfloat-abi=soft";
    assert_eq!(
        log,
        [
            "rustc -vV".to_string(),
            "sh -c command -v \"$0\" rustc".into(),
            "ROOT/llvm/clang --version".into(),
            "cd ROOT/crates/bulb-tc32 && BULB_BOARD=ESMLFzm_w6_Dimm-4.63 BULB_FILE_VERSION=01000000 BULB_VERSION=1.0.0 \
             -u CARGO_ENCODED_RUSTFLAGS CARGO_TARGET_DIR=ROOT/target/tc32-cargo/cargo-ID OUTGLOW_RUSTC_WRAPPER=1 RUSTC_BOOTSTRAP=1 \
             -u RUSTC_WORKSPACE_WRAPPER RUSTC_WRAPPER=/bin/xtask \
             RUSTFLAGS=-C linker-plugin-lto -C force-frame-pointers=no --emit=llvm-bc --remap-path-prefix=/home/u=~ \
             --remap-path-prefix=/rust/library=rust --remap-path-prefix=ROOT=outglow \
             -u TOUCHLINK_MASTER_KEY __CARGO_TESTS_ONLY_SRC_ROOT=/rust/library /bin/cargo build \
             --release --target thumbv4t-none-eabi --no-default-features --features=ble,sense -Zbuild-std=core \
             -Zbuild-std-features=compiler-builtins-mem"
                .into(),
            format!("cd {out}/bc && ROOT/llvm/llvm-ar x {rel}/libbulb_tc32.a"),
            format!(
                "ROOT/llvm/llvm-link {out}/bc/bulb_tc32-1.a.rcgu.o {out}/bc/bulb_tc32-1.b.rcgu.o {out}/bc/compiler_builtins-9.bc -o {out}/rust.bc"
            ),
            format!("ROOT/llvm/llvm-dis {out}/rust.bc -o {out}/rust.thumb.ll"),
            format!(
                "ROOT/llvm/clang {flags} -Oz -ffunction-sections -fdata-sections -mllvm -arm-load-store-opt=false \
                 -Wno-override-module -c {out}/rust.ll -o {out}/rust.o"
            ),
            format!("ROOT/llvm/clang {flags} -DIMAGE_TYPE=0x0390 -c ROOT/crates/bulb-tc32/boot/start.S -o {out}/start.o"),
            format!(
                "ROOT/llvm/ld.lld -T ROOT/crates/bulb-tc32/boot/link.ld --defsym=FW_OFFSET=0x6a000 --defsym=IMAGE_LIMIT=0x63fb0 --gc-sections -Map={out}/bulb-tc32.map \
                 {out}/start.o {out}/rust.o -o {out}/bulb-tc32.elf"
            ),
            format!("ROOT/llvm/llvm-size {out}/bulb-tc32.elf"),
        ]
    );
    let mut shared: Vec<String> =
        std::fs::read_dir(r.t.path("target/tc32-cargo")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    shared.sort();
    assert_eq!(shared, [format!("cargo-{id}"), format!("cargo-noble-{id}"), "notes".into()]);
    assert_eq!(std::fs::read(r.t.path("target/tc32/bc/compiler_builtins-9.bc")).unwrap(), b"BC");
    assert!(r.t.path(&format!("target/tc32-cargo/cargo-{id}/outglow.lock")).exists());
    let dir = r.t.path("target/tc32");
    assert_eq!(std::fs::read(dir.join("bulb-tc32.bin")).unwrap(), code());
    let staged = std::fs::read(dir.join("bulb-tc32-ota.bin")).unwrap();
    assert_eq!(staged, telink::finalize(&code()));
    let f = ota::File::parse(&std::fs::read(dir.join("bulb-tc32.ota")).unwrap()).unwrap();
    assert_eq!((f.header.manufacturer_code, f.header.image_type, f.header.file_version), (0x1141, 0x0390, 0x0100_0000));
    assert_eq!(f.header.header_string, "outglow ESMLFzm_w6_Dimm-4.63");
    assert_eq!(f.image(), Some(&staged[..]));
    assert_eq!(
        std::fs::read_to_string(dir.join("rust.ll")).unwrap(),
        "attributes #0 = { \"no-jump-tables\"=\"true\" \"target-cpu\"=\"tc32\" }\n"
    );
    let mut left: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    left.sort();
    assert!(left.contains(&"keep".to_string()));
    assert!(!left.contains(&"cargo".to_string()) && !left.contains(&"cargo-0000000000000000".to_string()));
    assert!(left.contains(&"cargo-file".to_string()) || !left.iter().any(|n| n == "cargo-file"));
    assert!(!dir.join("bc/stale.o").exists());
}

#[test]
fn a_variant_with_a_key_a_version_and_its_own_directory() {
    let mut r = Rig::new("variant");
    r.set("TOUCHLINK_MASTER_KEY", "00112233");
    r.set("BULB_FILE_VERSION", "0x01020003");
    r.unset("RUST_LIB_SRC");
    r.set("RUSTC", "/nix/rustc");
    let sysroot = r.t.path("sysroot");
    std::fs::create_dir_all(sysroot.join("lib/rustlib/src/rust/library/core")).unwrap();
    let sys = format!("{}\n", sysroot.display());
    let tools = Toolchain { ..Toolchain::good() };
    let r = Rig { tools, ..r };
    let vars: Vec<(&str, &str)> = r.vars.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let e = env(&vars);
    let mut fake = Fake::new(|c| {
        let first = c.get_args().next().map(|a| a.to_string_lossy().into_owned()).unwrap_or_default();
        match (program(c).as_str(), first.as_str()) {
            ("rustc", "--print") => (0, sys.clone()),
            ("rustc", "-vV") => (0, PINNED_VV.into()),
            _ => r.tools.answer(c),
        }
    });
    let mut c = Ctx { root: r.t.0.clone(), sh: &mut fake, env: &e, exe: "/bin/xtask".into(), toolchain: r.t.path("cached") };
    let a = args(&["--board", "EGLO_ZM_TW-4.62", "--no-ble", "--no-sense", "--trace", "--out-dir", r.t.path("out").to_str().unwrap()]);
    run_in(&mut c, &a).unwrap();
    let cargo = fake.log.iter().find(|l| l.contains(" build --release")).unwrap();
    assert!(cargo.contains(" BULB_BOARD=EGLO_ZM_TW-4.62 BULB_FILE_VERSION=01020003 BULB_VERSION=1.2.3 "), "{cargo}");
    assert!(cargo.contains(&format!("--remap-path-prefix={}=rust ", sysroot.join("lib/rustlib/src/rust/library").display())));
    assert!(cargo.contains(" TOUCHLINK_MASTER_KEY=00112233 "), "{cargo}");
    assert!(cargo.ends_with(" --features=trace -Zbuild-std=core -Zbuild-std-features=compiler-builtins-mem"), "{cargo}");
    assert_eq!(fake.log[0], "/nix/rustc -vV");
    assert!(fake.log.iter().any(|l| l.ends_with("-DIMAGE_TYPE=0x0395 -c ") || l.contains("-DIMAGE_TYPE=0x0395 -c ")));
    let f = ota::File::parse(&std::fs::read(r.t.path("out/bulb-tc32.ota")).unwrap()).unwrap();
    assert_eq!((f.header.image_type, f.header.file_version), (0x0395, 0x0102_0003));
    let target: Vec<_> = std::fs::read_dir(r.t.path("target")).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(target, ["tc32-cargo"]);
    assert!(cargo.contains(&format!(" CARGO_TARGET_DIR={}/cargo-noble-nosense-trace-{} ", r.t.path("target/tc32-cargo").display(), cache_id(PINNED_VV))));
}

#[test]
fn every_board_of_the_table() {
    let r = Rig::new("all");
    r.t.put("target/tc32-eglo_zm_tw-4.62/cargo/stale", "");
    let (res, log) = r.build(&["--all"]);
    res.unwrap();
    let boards: Vec<&String> = log.iter().filter(|l| l.contains(" build --release")).collect();
    assert_eq!(boards.len(), 2);
    let shared = format!("ROOT/target/tc32-cargo/cargo-{}", cache_id(PINNED_VV));
    let own = "ROOT/target/tc32-eglo_zm_tw-4.62/cargo";
    assert!(boards[0].contains(&format!(" CARGO_TARGET_DIR={shared} ")), "{}", boards[0]);
    let tw = boards[0].replace("ESMLFzm_w6_Dimm-4.63", "EGLO_ZM_TW-4.62").replace(&shared, own);
    assert_eq!(tw, *boards[1]);
    assert!(log.contains(&format!("cp -Rp {shared} {own}")));
    assert!(log.contains(&format!("cd ROOT/target/tc32-eglo_zm_tw-4.62/bc && ROOT/llvm/llvm-ar x {own}/thumbv4t-none-eabi/release/libbulb_tc32.a")));
    let tools: Vec<String> = log.iter().skip(3).map(|l| program_of(l)).collect();
    assert_eq!(
        tools,
        [
            "cargo", "llvm-ar", "cp", "cargo", "llvm-ar", "llvm-link", "llvm-link", "llvm-dis", "llvm-dis", "clang", "clang", "clang", "clang",
            "ld.lld", "ld.lld", "llvm-size"
        ]
    );
    assert!(log.last().unwrap().ends_with("/target/tc32/bulb-tc32.elf ROOT/target/tc32-eglo_zm_tw-4.62/bulb-tc32.elf"));
    assert!(r.t.path("target/tc32/bulb-tc32.ota").exists());
    assert!(r.t.path("target/tc32-eglo_zm_tw-4.62/bulb-tc32.ota").exists());
    assert!(r.t.path("target/tc32-eglo_zm_tw-4.62/bc/compiler_builtins-9.bc").exists());
    assert!(!r.t.path("target/tc32-eglo_zm_tw-4.62/cargo").exists());
    {
        let vars: Vec<(&str, &str)> = r.vars.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let e = env(&vars);
        let mut fake = Fake::new(|c| match program(c) == "cargo" && env_of(c, "CARGO_TARGET_DIR").ends_with("cargo") {
            true => (1, "broken".into()),
            false => r.tools.answer(c),
        });
        let mut c = Ctx { root: r.t.0.clone(), sh: &mut fake, env: &e, exe: "/bin/xtask".into(), toolchain: r.t.path("cached") };
        assert_eq!(run_in(&mut c, &args(&["--all"])).unwrap_err().to_string(), "\"/bin/cargo\" failed (exit status: 1)");
        assert_eq!(fake.log.iter().filter(|l| l.contains(" build --release")).count(), 2);
        assert!(!fake.log.iter().any(|l| l.contains("llvm-ar x ROOT")));
    }
    let mut r = r;
    r.tools.fail = "cp";
    assert_eq!(r.build(&["--all"]).0.unwrap_err().to_string(), "\"cp\" failed (exit status: 1)");
    r.t.put("crates/bulb-tc32/boards.tsv", "# name\n");
    let (res, log) = r.build(&["--all"]);
    res.unwrap();
    assert!(!log.iter().any(|l| l.contains(" build --release")));
    std::fs::remove_file(r.t.path("crates/bulb-tc32/boards.tsv")).unwrap();
    assert!(r.build(&["--all"]).0.is_err());
}

#[test]
fn the_default_board_for_another_task() {
    let mut r = Rig::new("default");
    r.unset("CARGO");
    let (res, log) = r.go(|c| board_default(c, "ESMLFzm_w6_TW-4.63"));
    assert!(log.iter().any(|l| l.contains(" cargo build --release ")));
    assert_eq!(res.unwrap(), r.t.path("target/tc32-tw"));
    assert!(r.t.path("target/tc32-tw/bulb-tc32.bin").exists());
}

#[test]
fn what_stops_a_build() {
    let r = Rig::new("stops");
    let (res, log) = r.build(&["--board", "Nope-4.60"]);
    let e = res.unwrap_err().to_string();
    assert!(e.starts_with("unknown board Nope-4.60; the boards: ESMLFzm_w6_Dimm-4.63, ESMLFzm_w6_TW-4.63, "), "{e}");
    assert!(log.is_empty());
    let (res, _) = r.build(&["--file-version", "zz"]);
    assert_eq!(res.unwrap_err().to_string(), "file version zz");
    let mut r = Rig::new("noclang");
    r.unset("TC32_LLVM");
    let (res, log) = r.build(&[]);
    assert_eq!(res.unwrap_err().to_string(), "no clang in /home/u/oss/llvm-tc32/build/bin: run `cargo xtask toolchain` (downloads llvm-tc32) or set TC32_LLVM");
    assert!(log.is_empty());
    r.t.put("cached/clang", "");
    let (res, log) = r.build(&[]);
    res.unwrap();
    assert!(log.iter().any(|l| l.starts_with(&format!("{} --version", r.t.path("cached/clang").display()).replace(&r.t.0.display().to_string(), "ROOT"))));
}

#[test]
fn machine_code_without_its_bitcode() {
    let mut r = Rig::new("nobc");
    r.tools.deps_bc = false;
    let e = r.build(&[]).0.unwrap_err().to_string();
    assert!(e.ends_with("/release/deps/compiler_builtins-9.bc does not exist"), "{e}");
    assert!(e.contains("/bc/compiler_builtins-9.x1.o is machine code and "), "{e}");
    r.tools.machine_code = false;
    let (res, log) = r.build(&[]);
    res.unwrap();
    assert!(log.iter().any(|l| l.contains("bulb_tc32-1.b.rcgu.o -o ")));
}

#[test]
fn a_broken_tool_or_image_stops_the_build() {
    for tool in ["cargo", "llvm-ar", "llvm-link", "llvm-dis", "clang", "ld.lld", "llvm-size", "rustc"] {
        let mut r = Rig::new("broken");
        r.tools.fail = tool;
        let e = r.build(&[]).0.unwrap_err().to_string();
        assert!(e.contains(&format!("{tool}\" failed")), "{tool}: {e}");
    }
    let mut r = Rig::new("object");
    r.tools.fail_ending = Some("/target/tc32/rust.o");
    let (res, log) = r.build(&[]);
    assert!(res.unwrap_err().to_string().ends_with("/llvm/clang\" failed (exit status: 1)"));
    assert!(!log.iter().any(|l| l.contains("ld.lld")));
    let r = Rig::new("blocked");
    r.t.put("blocked", "");
    let e = r.build(&["--out-dir", r.t.path("blocked").to_str().unwrap()]).0.unwrap_err();
    assert_eq!(e.downcast_ref::<std::io::Error>().map(std::io::Error::kind), Some(std::io::ErrorKind::NotADirectory), "{e:?}");
    let r = Rig::new("nolock");
    r.t.put("target/tc32-cargo", "");
    let (res, log) = r.build(&[]);
    assert_eq!(res.unwrap_err().downcast_ref::<std::io::Error>().map(std::io::Error::kind), Some(std::io::ErrorKind::NotADirectory));
    assert!(!log.iter().any(|l| l.contains(" build --release")));
    let mut r = Rig::new("elf");
    r.tools.elf = b"not an elf".to_vec();
    let e = r.build(&[]).0.unwrap_err().to_string();
    assert!(e.ends_with("/target/tc32/bulb-tc32.elf: not a 32-bit little-endian ELF"), "{e}");
    let mut r = Rig::new("short");
    r.tools.elf = tc32_elf(&[0; 32]);
    let e = r.build(&[]).0.unwrap_err().to_string();
    assert_eq!(e, "the finished image fails its own check: no KNLT marker at 0x08");
    let mut r = Rig::new("large");
    let mut large = code();
    large.resize(ota::IMAGE_LIMIT + 1, 0);
    r.tools.elf = tc32_elf(&large);
    let e = r.build(&[]).0.unwrap_err().to_string();
    let out = r.t.path("target/tc32/bulb-tc32.ota");
    assert!(e.starts_with(&format!("{}: the OTA file is ", out.display())) && e.contains("a bulb takes at most 409600 B"), "{e}");
    assert!(!out.exists());
}

#[test]
fn an_unpinned_rustc_still_builds() {
    let mut r = Rig::new("unpinned");
    r.tools.vv = "rustc 1.90.0\nrelease: 1.90.0\n".into();
    r.build(&[]).0.unwrap();
    assert_eq!(unpinned(&r.tools.vv), ["release 1.90.0 (pinned 1.99.0)", "commit-hash ? (pinned b940084d7eb6a299eb4bfeb8e34901bc051e7ac4)", "LLVM version ? (pinned 21.1.8)"]);
    assert!(unpinned(PINNED_VV).is_empty());
    assert_eq!(rustc_warning(PINNED_VV), None);
    let w = rustc_warning("release: 1.99.0\ncommit-hash: b940084d7eb6a299eb4bfeb8e34901bc051e7ac4\nLLVM version: 23.1.2\n").unwrap();
    assert!(w.starts_with("\nWARNING: this rustc is not the pinned one: LLVM version 23.1.2 (pinned 21.1.8).\nWARNING: the image will NOT"), "{w}");
    assert!(w.ends_with("(or `nix develop --command cargo xtask build`).\n"));
    assert_eq!(rustc_field(PINNED_VV, "LLVM version"), "21.1.8");
    assert_eq!(rustc_field("release:1.0", "release"), "?");
    assert_eq!(rustc_field("release:  2.0 \n", "release"), "2.0");
}

#[test]
fn the_standard_library_sources() {
    let r = Rig::new("libsrc");
    assert_eq!(r.go(rust_lib_src).0.unwrap(), PathBuf::from("/rust/library"));
    let mut r = Rig::new("nix");
    r.unset("RUST_LIB_SRC");
    r.t.put("flake.lock", r#"{"nodes": {"nixpkgs": {"locked": {"rev": "abc123"}}}}"#);
    let calls = RefCell::new(vec![]);
    let vars: Vec<(&str, &str)> = r.vars.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let e = env(&vars);
    for (nix, want) in [(0, Ok("/nix/store/src")), (1, Err("no Rust standard library sources: run in `nix develop`, or set RUST_LIB_SRC"))] {
        let mut fake = Fake::new(|c| {
            calls.borrow_mut().push(describe(c));
            match program(c).as_str() {
                "rustc" => (0, "/nonexistent/sysroot\n".into()),
                _ => (nix, "/nix/store/src\n".into()),
            }
        });
        let mut c = Ctx { root: r.t.0.clone(), sh: &mut fake, env: &e, exe: "/x".into(), toolchain: "/t".into() };
        let got = rust_lib_src(&mut c).map(|p| p.display().to_string()).map_err(|e| e.to_string());
        assert_eq!(got, want.map(String::from).map_err(String::from));
    }
    assert_eq!(
        calls.borrow()[..2],
        ["rustc --print sysroot", "nix build --no-link --print-out-paths github:NixOS/nixpkgs/abc123#rustPlatform.rustLibSrc"]
    );
    r.t.put("flake.lock", r#"{"nodes": {}}"#);
    assert_eq!(r.go(rust_lib_src).0.unwrap_err().to_string(), "no nixpkgs revision in flake.lock");
    assert_eq!(locked_nixpkgs(&crate::root()).unwrap().len(), 40);
}

#[test]
fn the_rustc_wrapper_names_the_metadata_after_the_crate() {
    let os = |v: &[&str]| v.iter().map(OsString::from).collect::<Vec<_>>();
    let a = os(&["--crate-name", "zigbee", "--target", TARGET, "-C", "metadata=abc", "-Cmetadata=def", "metadata=x", "-C", "opt-level=z"]);
    assert_eq!(
        wrapped(a),
        os(&["--crate-name", "zigbee", "--target", TARGET, "-C", "metadata=outglow-zigbee", "-Cmetadata=outglow-zigbee", "metadata=x", "-C", "opt-level=z"])
    );
    let eq = os(&["--crate-name", "core", &format!("--target={TARGET}"), "-Cmetadata=1"]);
    assert_eq!(wrapped(eq)[3], "-Cmetadata=outglow-core");
    let host = os(&["--crate-name", "serde", "--target", "x86_64-unknown-linux-gnu", "-Cmetadata=1"]);
    assert_eq!(wrapped(host.clone()), host);
    let nameless = os(&["--target", TARGET, "-Cmetadata=1"]);
    assert_eq!(wrapped(nameless.clone()), nameless);
    let target_last = os(&["-Cmetadata=1", "--crate-name", "a", "--target"]);
    assert_eq!(wrapped(target_last.clone()), target_last);
}

#[test]
fn the_rustc_wrapper_runs_rustc() {
    let os = |v: &[&str]| v.iter().map(OsString::from).collect::<Vec<_>>();
    let on = env(&[(WRAPPER_ENV, "1")]);
    assert_eq!(rustc_wrapper(&env(&[]), os(&["xtask", "sh"])), None);
    assert_eq!(rustc_wrapper(&on, os(&["xtask"])), Some(2));
    assert_eq!(rustc_wrapper(&on, os(&["xtask", "sh", "-c", "exit 7"])), Some(7));
    assert_eq!(rustc_wrapper(&on, os(&["xtask", "sh", "-c", "kill -9 $$"])), Some(1));
    assert_eq!(rustc_wrapper(&on, os(&["xtask", "/nonexistent/rustc"])), Some(1));
}

#[test]
fn names_features_and_versions() {
    assert_eq!(dir_name("ESMLFzm_w6_Dimm-4.63"), "tc32");
    assert_eq!(dir_name("ESMLFzm_w6_TW-4.63"), "tc32-tw");
    assert_eq!(dir_name("EGLO_ZM_TW-4.62"), "tc32-eglo_zm_tw-4.62");
    assert_eq!(default_dir(Path::new("/r"), "ESMLFzm_w6_TW-4.63"), PathBuf::from("/r/target/tc32-tw"));
    assert_eq!(features(&args(&[])), (vec!["ble", "sense"], String::new()));
    assert_eq!(features(&args(&["--no-ble"])), (vec!["sense"], "-noble".into()));
    assert_eq!(features(&args(&["--no-sense", "--trace"])), (vec!["ble", "trace"], "-nosense-trace".into()));
    assert_eq!(features(&args(&["--no-ble", "--no-sense", "--trace"])), (vec!["trace"], "-noble-nosense-trace".into()));
    let none = env(&[]);
    let with = env(&[("BULB_FILE_VERSION", "02000001")]);
    assert_eq!(file_version(&args(&[]), &none).unwrap(), 0x0100_0000);
    assert_eq!(file_version(&args(&[]), &with).unwrap(), 0x0200_0001);
    assert_eq!(file_version(&args(&["--file-version", "0x0a"]), &with).unwrap(), 10);
    assert_eq!(member_order(Path::new("/d/bulb_tc32-1.abc.rcgu.o")), "abc.rcgu.o");
    assert_eq!(member_order(Path::new("/d/plain")), "plain");
}

#[test]
fn the_board_list() {
    let b = boards_tsv(&crate::root()).unwrap();
    assert_eq!(b.len(), outglow_image::board::BOARDS.len());
    assert_eq!(b[0], "ESMLFzm_w6_Dimm-4.63");
}

#[test]
fn retarget_replaces_every_cpu() {
    let ll = "attributes #0 = { \"target-cpu\"=\"arm7tdmi\" nounwind }\nattributes #1 = { \"target-cpu\"=\"generic\" }";
    let r = retarget(ll).unwrap();
    assert_eq!(
        r,
        "attributes #0 = { \"no-jump-tables\"=\"true\" \"target-cpu\"=\"tc32\" nounwind }\nattributes #1 = { \"no-jump-tables\"=\"true\" \"target-cpu\"=\"tc32\" }"
    );
    assert_eq!(retarget("no attributes").unwrap_err().to_string(), "could not retarget the bitcode to tc32");
    assert_eq!(retarget("\"target-cpu\"=\"generic").unwrap_err().to_string(), "unterminated target-cpu attribute");
}

#[test]
fn retarget_replaces_the_cpu_of_the_module_asm() {
    let ll = "module asm(target_features: \"+soft-float,+strict-align\", target_cpu: \"generic\")\n    \".thumb\"\n\nattributes #0 = { \"target-cpu\"=\"generic\" }";
    let r = retarget(ll).unwrap();
    assert!(r.starts_with("module asm(target_features: \"+soft-float,+strict-align\", target_cpu: \"tc32\")\n"), "{r}");
    assert!(!r.contains("generic"));
    let later = "source_filename = \"x\"\nmodule asm(target_cpu: \"generic\")\nattributes #0 = { \"target-cpu\"=\"g\" }";
    assert!(retarget(later).unwrap().starts_with("source_filename = \"x\"\nmodule asm(target_cpu: \"tc32\")\n"));
    let plain = "module asm\n    \".thumb\"\n\nattributes #0 = { \"target-cpu\"=\"generic\" }";
    assert!(retarget(plain).unwrap().starts_with("module asm\n"));
    let quoted = "x\n  \"target_cpu: \"keep\"\"\nattributes #0 = { \"target-cpu\"=\"g\" }";
    assert!(retarget(quoted).unwrap().contains("target_cpu: \"keep\""));
    let twice = "module asm(target_cpu: \"a\", target_cpu: \"b\")\nattributes #0 = { \"target-cpu\"=\"g\" }";
    assert!(retarget(twice).unwrap().starts_with("module asm(target_cpu: \"tc32\", target_cpu: \"b\")\n"));
    assert_eq!(retarget("module asm(target_cpu: \"a").unwrap_err().to_string(), "unterminated target_cpu of module asm");
}

#[test]
fn the_real_context() {
    let root = with_real(|c| {
        assert!(c.exe.exists());
        assert_eq!(c.toolchain, crate::toolchain::installed_bin());
        assert_eq!(c.var("OUTGLOW_SURELY_NOT_SET_42"), None);
        Ok(c.root.clone())
    })
    .unwrap();
    assert_eq!(root, crate::root());
}

#[test]
fn the_bulb_and_the_build_agree_on_the_staging_area() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let board = std::fs::read_to_string(root.join("crates/bulb-tc32/src/board.rs")).unwrap();
    let line = board.lines().find(|l| l.starts_with("pub const AWOX_1M: Flash")).unwrap();
    let field = line.split("staging: 0x").nth(1).unwrap().split(',').next().unwrap().replace('_', "");
    assert_eq!(usize::from_str_radix(&field, 16).unwrap(), telink::STAGING_SIZE);
    let link = std::fs::read_to_string(root.join("crates/bulb-tc32/boot/link.ld")).unwrap();
    assert!(link.contains("ASSERT(_bin_size_ <= IMAGE_LIMIT,"));
}
