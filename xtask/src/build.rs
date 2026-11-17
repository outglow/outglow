use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use outglow_image::{board, elf, ota, telink};

const TARGET: &str = "thumbv4t-none-eabi";
const MANUFACTURER: u16 = 0x1141;

#[derive(clap::Args)]
pub struct Args {
    #[doc = " the board (crates/bulb-tc32/boards.tsv), e.g. EGLO_ZM_TW-4.62; default the dimmable filament bulb"]
    #[arg(long)]
    board: Option<String>,
    #[doc = " all boards of boards.tsv, one after another"]
    #[arg(long, conflicts_with = "board")]
    all: bool,
    #[doc = " Zigbee only: no Bluetooth LE (output directory suffix -noble)"]
    #[arg(long)]
    no_ble: bool,
    #[doc = " diagnostics for the emulator's test harness (suffix -trace); not for bulbs"]
    #[arg(long)]
    trace: bool,
    #[doc = " no motion sensing (suffix -nosense): every image has it otherwise, off"]
    #[doc = " until switched on (Occupancy Sensing 0x0406, manufacturer cluster 0xFC5E)"]
    #[arg(long)]
    no_sense: bool,
    #[doc = " OTA file version, hexadecimal: MAJOR, MINOR (a byte each), PATCH (16 bits); default $BULB_FILE_VERSION or 01000000 (1.0.0)"]
    #[arg(long)]
    file_version: Option<String>,
    #[doc = " output directory (one board only)"]
    #[arg(long, conflicts_with = "all")]
    out_dir: Option<PathBuf>,
}

fn run_tool(cmd: &mut Command) -> Result<()> {
    let st = cmd.status().with_context(|| format!("{cmd:?}"))?;
    if !st.success() {
        bail!("{:?} failed ({st})", cmd.get_program());
    }
    Ok(())
}

fn tool_output(cmd: &mut Command) -> Result<String> {
    let o = cmd.output().with_context(|| format!("{cmd:?}"))?;
    if !o.status.success() {
        bail!("{:?} failed: {}", cmd.get_program(), String::from_utf8_lossy(&o.stderr));
    }
    Ok(String::from_utf8_lossy(&o.stdout).into_owned())
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

const PINNED_RUSTC: [(&str, &str); 3] =
    [("release", "1.99.0"), ("commit-hash", "b940084d7eb6a299eb4bfeb8e34901bc051e7ac4"), ("LLVM version", "21.1.8")];

const WRAPPER_RULES: u32 = 1;

fn rustc() -> std::ffi::OsString {
    std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into())
}

fn check_rustc() -> Result<()> {
    let vv = tool_output(Command::new(rustc()).arg("-vV"))?;
    let field = |k: &str| vv.lines().find_map(|l| l.strip_prefix(k).and_then(|r| r.strip_prefix(": "))).unwrap_or("?").trim().to_string();
    let path = tool_output(Command::new("sh").arg("-c").arg("command -v \"$0\"").arg(rustc())).unwrap_or_default();
    println!("== rustc: {} (LLVM {}) {}", vv.lines().next().unwrap_or("?"), field("LLVM version"), path.trim());
    let off: Vec<String> = PINNED_RUSTC.iter().filter(|(k, v)| field(k) != *v).map(|(k, v)| format!("{k} {} (pinned {v})", field(k))).collect();
    if !off.is_empty() {
        eprintln!(
            "\nWARNING: this rustc is not the pinned one: {}.\n\
             WARNING: the image will NOT be byte-identical to CI's (other inlining, other layout, other timing in the emulator).\n\
             WARNING: build in the Nix shell: `nix develop`, then `cargo xtask build` (or `nix develop --command cargo xtask build`).\n",
            off.join(", ")
        );
    }
    Ok(())
}

fn locked_nixpkgs(root: &Path) -> Result<String> {
    let lock: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(root.join("flake.lock"))?)?;
    lock["nodes"]["nixpkgs"]["locked"]["rev"].as_str().map(String::from).ok_or_else(|| anyhow!("no nixpkgs revision in flake.lock"))
}

fn rust_lib_src(root: &Path) -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("RUST_LIB_SRC") {
        return Ok(p.into());
    }
    let sysroot = tool_output(Command::new(rustc()).args(["--print", "sysroot"]))?;
    let src = Path::new(sysroot.trim()).join("lib/rustlib/src/rust/library");
    if src.join("core").is_dir() {
        return Ok(src);
    }
    let flake = format!("github:NixOS/nixpkgs/{}#rustPlatform.rustLibSrc", locked_nixpkgs(root)?);
    let p = tool_output(Command::new("nix").args(["build", "--no-link", "--print-out-paths", &flake]))
        .context("no Rust standard library sources: run in `nix develop`, or set RUST_LIB_SRC")?;
    Ok(PathBuf::from(p.trim()))
}

const WRAPPER_ENV: &str = "OUTGLOW_RUSTC_WRAPPER";

pub fn rustc_wrapper() -> Option<i32> {
    std::env::var_os(WRAPPER_ENV)?;
    let mut args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    if args.is_empty() {
        return Some(2);
    }
    let rustc = args.remove(0);
    let s: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
    let target = s.windows(2).any(|w| w[0] == "--target" && w[1] == TARGET) || s.iter().any(|a| *a == format!("--target={TARGET}"));
    let name = s.windows(2).find(|w| w[0] == "--crate-name").map(|w| w[1].clone());
    if let (true, Some(name)) = (target, name) {
        let mut i = 0;
        while i < args.len() {
            if s[i] == "-C" && s.get(i + 1).is_some_and(|n| n.starts_with("metadata=")) {
                args[i + 1] = format!("metadata=outglow-{name}").into();
                i += 1;
            } else if s[i].starts_with("-Cmetadata=") {
                args[i] = format!("-Cmetadata=outglow-{name}").into();
            }
            i += 1;
        }
    }
    match Command::new(rustc).args(&args).status() {
        Ok(st) => Some(st.code().unwrap_or(1)),
        Err(e) => {
            eprintln!("outglow rustc wrapper: {e}");
            Some(1)
        }
    }
}

fn boards_tsv(root: &Path) -> Result<Vec<String>> {
    let t = std::fs::read_to_string(root.join("crates/bulb-tc32/boards.tsv"))?;
    Ok(t.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()).filter_map(|l| l.split('\t').next()).map(String::from).collect())
}

pub fn run(a: Args) -> Result<()> {
    let root = crate::root();
    if a.all {
        for b in boards_tsv(&root)? {
            build(&root, &b, &a, None)?;
        }
        return Ok(());
    }
    let b = a.board.clone().unwrap_or_else(|| "ESMLFzm_w6_Dimm-4.63".into());
    build(&root, &b, &a, a.out_dir.clone())
}

pub fn board_default(board_name: &str) -> Result<PathBuf> {
    let a = Args { board: Some(board_name.into()), all: false, no_ble: false, trace: false, no_sense: false, file_version: None, out_dir: None };
    build(&crate::root(), board_name, &a, None)?;
    Ok(default_dir(&crate::root(), board_name))
}

pub fn default_dir(root: &Path, board_name: &str) -> PathBuf {
    root.join("target").join(match board_name {
        "ESMLFzm_w6_Dimm-4.63" => "tc32".to_string(),
        "ESMLFzm_w6_TW-4.63" => "tc32-tw".to_string(),
        b => format!("tc32-{}", b.to_lowercase()),
    })
}

fn build(root: &Path, board_name: &str, a: &Args, out_dir: Option<PathBuf>) -> Result<()> {
    let here = root.join("crates/bulb-tc32");
    let known = board::known(board_name).ok_or_else(|| {
        let names: Vec<&str> = board::BOARDS.iter().map(|b| b.name).collect();
        anyhow!("unknown board {board_name}; the boards: {}", names.join(", "))
    })?;
    let llvm = std::env::var_os("TC32_LLVM").map(PathBuf::from).unwrap_or_else(|| {
        let cached = crate::toolchain::installed_bin();
        if cached.join("clang").exists() { cached } else { home().join("oss/llvm-tc32/build/bin") }
    });
    if !llvm.join("clang").exists() {
        bail!("no clang in {}: run `cargo xtask toolchain` (downloads llvm-tc32) or set TC32_LLVM", llvm.display());
    }
    let version = a.file_version.clone().or_else(|| std::env::var("BULB_FILE_VERSION").ok()).unwrap_or_else(|| "01000000".into());
    let file_version = u32::from_str_radix(version.trim_start_matches("0x"), 16).with_context(|| format!("file version {version}"))?;
    let version = format!("{file_version:08x}");
    let release = format!("{}.{}.{}", file_version >> 24, (file_version >> 16) & 0xff, file_version & 0xffff);
    let mut features = Vec::new();
    let mut suffix = String::new();
    if !a.no_ble {
        features.push("ble");
    } else {
        suffix.push_str("-noble");
    }
    if !a.no_sense {
        features.push("sense");
    } else {
        suffix.push_str("-nosense");
    }
    if a.trace {
        features.push("trace");
        suffix.push_str("-trace");
    }
    let dir = match board_name {
        "ESMLFzm_w6_Dimm-4.63" => "tc32".to_string(),
        "ESMLFzm_w6_TW-4.63" => "tc32-tw".to_string(),
        b => format!("tc32-{}", b.to_lowercase()),
    };
    let out = out_dir.unwrap_or_else(|| root.join("target").join(format!("{dir}{suffix}")));
    check_rustc()?;
    let rust_src = rust_lib_src(root)?;
    std::fs::create_dir_all(&out)?;
    let bc_dir = out.join("bc");
    let _ = std::fs::remove_dir_all(&bc_dir);
    std::fs::create_dir_all(&bc_dir)?;

    let key = std::env::var("TOUCHLINK_MASTER_KEY").ok().filter(|k| !k.is_empty());
    println!("== touchlink: {}", if key.is_some() { "key index 4 enabled (key given to the build)" } else { "no master key (key indices 0 and 15 only)" });

    println!("== rustc -> bitcode ({TARGET}, board {board_name}, OTA image type {:#06x}, version {version})", known.image_type);
    let rustflags = format!(
        "-C linker-plugin-lto -C force-frame-pointers=no --emit=llvm-bc --remap-path-prefix={}=~ --remap-path-prefix={}=rust --remap-path-prefix={}=outglow",
        home().display(), rust_src.display(), root.display()
    );
    let cache_id = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        tool_output(Command::new(rustc()).arg("-vV"))?.hash(&mut h);
        WRAPPER_RULES.hash(&mut h);
        h.finish()
    };
    let cargo_dir = out.join(format!("cargo-{cache_id:016x}"));
    for e in std::fs::read_dir(&out)?.flatten() {
        let n = e.file_name().to_string_lossy().to_string();
        if (n == "cargo" || n.starts_with("cargo-")) && e.path() != cargo_dir {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
    let mut cargo = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    cargo.current_dir(&here)
        .env("RUSTC_BOOTSTRAP", "1")
        .env("__CARGO_TESTS_ONLY_SRC_ROOT", &rust_src)
        .env("BULB_FILE_VERSION", &version)
        .env("BULB_VERSION", &release)
        .env("BULB_BOARD", board_name)
        .env("RUSTFLAGS", rustflags)
        .env("CARGO_TARGET_DIR", &cargo_dir)
        .env("RUSTC_WRAPPER", std::env::current_exe()?)
        .env(WRAPPER_ENV, "1")
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .args(["build", "--release", "--target", TARGET, "--no-default-features"])
        .arg(format!("--features={}", features.join(",")))
        .args(["-Zbuild-std=core", "-Zbuild-std-features=compiler-builtins-mem"]);
    match &key {
        Some(k) => cargo.env("TOUCHLINK_MASTER_KEY", k),
        None => cargo.env_remove("TOUCHLINK_MASTER_KEY"),
    };
    run_tool(&mut cargo)?;

    let clang_version = tool_output(Command::new(llvm.join("clang")).arg("--version"))?;
    println!("== bitcode -> TC32 object (llvm-tc32 {})", clang_version.lines().next().unwrap_or(""));
    let release = cargo_dir.join(TARGET).join("release");
    run_tool(Command::new(llvm.join("llvm-ar")).current_dir(&bc_dir).arg("x").arg(release.join("libbulb_tc32.a")))?;
    let mut members: Vec<PathBuf> = std::fs::read_dir(&bc_dir)?.filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "o")).collect();
    members.sort_by_key(|p| {
        let n = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
        n.split_once('.').map_or(n.clone(), |(_, rest)| rest.to_string())
    });
    let mut modules: Vec<PathBuf> = Vec::new();
    for m in members {
        let head = std::fs::read(&m)?;
        if head.starts_with(b"BC") {
            modules.push(m);
            continue;
        }
        let name = m.file_name().unwrap().to_string_lossy().into_owned();
        let krate = name.split('.').next().unwrap_or(&name);
        let bc = release.join("deps").join(format!("{krate}.bc"));
        if !bc.exists() {
            bail!("{} is machine code and {} does not exist", m.display(), bc.display());
        }
        if !modules.contains(&bc) {
            modules.push(bc);
        }
    }
    run_tool(Command::new(llvm.join("llvm-link")).args(&modules).arg("-o").arg(out.join("rust.bc")))?;
    run_tool(Command::new(llvm.join("llvm-dis")).arg(out.join("rust.bc")).arg("-o").arg(out.join("rust.thumb.ll")))?;
    let ll = retarget(&std::fs::read_to_string(out.join("rust.thumb.ll"))?)?;
    std::fs::write(out.join("rust.ll"), ll)?;

    let cflags = [format!("--target={TARGET}"), "-mcpu=tc32".into(), "-mthumb".into(), "-mfloat-abi=soft".into()];
    run_tool(Command::new(llvm.join("clang")).args(&cflags)
        .args(["-Oz", "-ffunction-sections", "-fdata-sections", "-mllvm", "-arm-load-store-opt=false", "-Wno-override-module", "-c"])
        .arg(out.join("rust.ll")).arg("-o").arg(out.join("rust.o")))?;
    run_tool(Command::new(llvm.join("clang")).args(&cflags).arg(format!("-DIMAGE_TYPE={:#06x}", known.image_type))
        .arg("-c").arg(here.join("boot/start.S")).arg("-o").arg(out.join("start.o")))?;

    println!("== link");
    let elf_path = out.join("bulb-tc32.elf");
    run_tool(Command::new(llvm.join("ld.lld")).arg("-T").arg(here.join("boot/link.ld")).arg("--gc-sections")
        .arg(format!("-Map={}", out.join("bulb-tc32.map").display()))
        .arg(out.join("start.o")).arg(out.join("rust.o")).arg("-o").arg(&elf_path))?;
    let (image, segments) = elf::image_of(&std::fs::read(&elf_path)?).map_err(|e| anyhow!("{}: {e}", elf_path.display()))?;
    std::fs::write(out.join("bulb-tc32.bin"), &image)?;
    println!("{}: {} bytes from {segments} segment(s)", out.join("bulb-tc32.bin").display(), image.len());
    run_tool(Command::new(llvm.join("llvm-size")).arg(&elf_path))?;

    println!("== OTA image");
    let staged = telink::finalize(&image);
    telink::check(&staged).map_err(|e| anyhow!("the finished image fails its own check: {e}"))?;
    std::fs::write(out.join("bulb-tc32-ota.bin"), &staged)?;
    let file = ota::File::with_image(ota::Header::new(MANUFACTURER, known.image_type, file_version, &format!("outglow {board_name}")), staged.clone());
    let packed = file.pack();
    std::fs::write(out.join("bulb-tc32.ota"), &packed)?;
    println!("OK: {} ({} B), firmware {} B, mfg={MANUFACTURER:#06x} imageType={:#06x} fileVersion={file_version:#010x}",
             out.join("bulb-tc32.ota").display(), packed.len(), staged.len(), known.image_type);
    Ok(())
}

fn retarget(ll: &str) -> Result<String> {
    let ll = retarget_module_asm(ll)?;
    let ll = ll.as_str();
    const KEY: &str = "\"target-cpu\"=\"";
    let mut out = String::with_capacity(ll.len() + ll.len() / 50);
    let mut rest = ll;
    while let Some(i) = rest.find(KEY) {
        out.push_str(&rest[..i]);
        let after = &rest[i + KEY.len()..];
        let end = after.find('"').ok_or_else(|| anyhow!("unterminated target-cpu attribute"))?;
        out.push_str("\"no-jump-tables\"=\"true\" \"target-cpu\"=\"tc32\"");
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    let all_tc32 = out.match_indices(KEY).all(|(i, _)| out[i + KEY.len()..].starts_with("tc32\""));
    if !all_tc32 || !out.contains("\"target-cpu\"=\"tc32\"") {
        bail!("could not retarget the bitcode to tc32");
    }
    Ok(out)
}

fn retarget_module_asm(ll: &str) -> Result<String> {
    const KEY: &str = "target_cpu: \"";
    let mut out = String::with_capacity(ll.len());
    let mut rest = ll;
    while let Some(i) = rest.find(KEY) {
        let line_start = rest[..i].rfind('\n').map_or(0, |n| n + 1);
        let in_header = rest[line_start..i].starts_with("module asm(");
        out.push_str(&rest[..i + KEY.len()]);
        let after = &rest[i + KEY.len()..];
        let end = after.find('"').ok_or_else(|| anyhow!("unterminated target_cpu of module asm"))?;
        out.push_str(if in_header { "tc32" } else { &after[..end] });
        rest = &after[end..];
    }
    out.push_str(rest);
    if out.lines().any(|l| l.starts_with("module asm(") && l.contains(KEY) && !l.contains("target_cpu: \"tc32\"")) {
        bail!("could not retarget the module's assembly to tc32");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn retarget_replaces_every_cpu() {
        let ll = "attributes #0 = { \"target-cpu\"=\"arm7tdmi\" nounwind }\nattributes #1 = { \"target-cpu\"=\"generic\" }";
        let r = super::retarget(ll).unwrap();
        assert_eq!(r.matches("\"no-jump-tables\"=\"true\" \"target-cpu\"=\"tc32\"").count(), 2);
        assert!(!r.contains("arm7tdmi"));
    }

    #[test]
    fn retarget_replaces_the_cpu_of_the_module_asm() {
        let ll = "module asm(target_features: \"+soft-float,+strict-align\", target_cpu: \"generic\")\n    \".thumb\"\n\nattributes #0 = { \"target-cpu\"=\"generic\" }";
        let r = super::retarget(ll).unwrap();
        assert!(r.starts_with("module asm(target_features: \"+soft-float,+strict-align\", target_cpu: \"tc32\")\n"), "{r}");
        assert!(!r.contains("generic"));
        let plain = "module asm\n    \".thumb\"\n\nattributes #0 = { \"target-cpu\"=\"generic\" }";
        assert!(super::retarget(plain).unwrap().starts_with("module asm\n"));
    }
}
