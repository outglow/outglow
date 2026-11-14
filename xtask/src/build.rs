//! `cargo xtask build`: the firmware of a board for the TLSR8258 (TC32 core).
//!
//! Stock rustc has no TC32 backend, and TC32 runs ARMv4T Thumb-1 code in its
//! own encoding. So rustc only goes as far as LLVM bitcode for thumbv4t, and
//! the llvm-tc32 fork (clang/lld with -mcpu=tc32) generates the TC32 code:
//!
//! | step | tool |
//! |---|---|
//! | bitcode of every crate | cargo (build-std, `-C linker-plugin-lto`, `--emit=llvm-bc`) |
//! | one module | llvm-link |
//! | every function's CPU = tc32, no jump tables | llvm-dis + text replacement |
//! | TC32 object | clang -mcpu=tc32 -Oz |
//! | start-up code | clang -mcpu=tc32 boot/start.S |
//! | ELF | ld.lld -T boot/link.ld --gc-sections |
//! | flash image | outglow_image::elf |
//! | image with size and CRC, Zigbee OTA file | outglow_image::{telink, ota} |
//!
//! Output in target/tc32 (the dimmable filament bulb), target/tc32-tw (the
//! tunable white one) or target/tc32-<board in lower case>:
//! bulb-tc32.elf, .bin (the image as linked), bulb-tc32-ota.bin (with size
//! and CRC: what goes into the OTA staging area) and bulb-tc32.ota.
//!
//! Needs: `TC32_LLVM`, the bin directory of a built llvm-tc32 (default
//! ~/oss/llvm-tc32/build/bin); the Rust standard library sources for
//! build-std (the rust-src component, or `RUST_LIB_SRC`). rustc is the stable
//! one; RUSTC_BOOTSTRAP=1 lets it take build-std.
//!
//! `TOUCHLINK_MASTER_KEY` (32 hexadecimal digits, not in this repository):
//! the image then offers and accepts touchlink key index 4, the one commercial
//! initiators use; without it, key indices 0 and 15 only.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use outglow_image::{board, elf, ota, telink};

const TARGET: &str = "thumbv4t-none-eabi";
const MANUFACTURER: u16 = 0x1141;

#[derive(clap::Args)]
pub struct Args {
    /// the board (crates/bulb-tc32/boards.tsv), e.g. EGLO_ZM_TW-4.62; default the dimmable filament bulb
    #[arg(long)]
    board: Option<String>,
    /// all boards of boards.tsv, one after another
    #[arg(long, conflicts_with = "board")]
    all: bool,
    /// Zigbee only: no Bluetooth LE (output directory suffix -noble)
    #[arg(long)]
    no_ble: bool,
    /// diagnostics for the emulator's test harness (suffix -trace); not for bulbs
    #[arg(long)]
    trace: bool,
    /// OTA file version, hexadecimal (default $BULB_FILE_VERSION or 00020000)
    #[arg(long)]
    file_version: Option<String>,
    /// output directory (one board only)
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

fn rust_lib_src() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("RUST_LIB_SRC") {
        return Ok(p.into());
    }
    let sysroot = tool_output(Command::new("rustc").args(["--print", "sysroot"]))?;
    let src = Path::new(sysroot.trim()).join("lib/rustlib/src/rust/library");
    if src.join("core").is_dir() {
        return Ok(src);
    }
    // nixpkgs' rustc has no rust-src component
    let p = tool_output(Command::new("nix").args(["build", "--no-link", "--print-out-paths", "nixpkgs#rustPlatform.rustLibSrc"]))
        .context("no Rust standard library sources: install the rust-src component or set RUST_LIB_SRC")?;
    Ok(PathBuf::from(p.trim()))
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

fn build(root: &Path, board_name: &str, a: &Args, out_dir: Option<PathBuf>) -> Result<()> {
    let here = root.join("crates/bulb-tc32");
    let known = board::known(board_name).ok_or_else(|| {
        let names: Vec<&str> = board::BOARDS.iter().map(|b| b.name).collect();
        anyhow!("unknown board {board_name}; the boards: {}", names.join(", "))
    })?;
    let llvm = std::env::var_os("TC32_LLVM").map(PathBuf::from).unwrap_or_else(|| home().join("oss/llvm-tc32/build/bin"));
    if !llvm.join("clang").exists() {
        bail!("no clang in {} (set TC32_LLVM to the bin directory of a built llvm-tc32)", llvm.display());
    }
    let version = a.file_version.clone().or_else(|| std::env::var("BULB_FILE_VERSION").ok()).unwrap_or_else(|| "00020000".into());
    let file_version = u32::from_str_radix(version.trim_start_matches("0x"), 16).with_context(|| format!("file version {version}"))?;
    let version = format!("{file_version:08x}");
    let mut features = Vec::new();
    let mut suffix = String::new();
    if !a.no_ble {
        features.push("ble");
    } else {
        suffix.push_str("-noble");
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
    let rust_src = rust_lib_src()?;
    std::fs::create_dir_all(&out)?;
    let bc_dir = out.join("bc");
    let _ = std::fs::remove_dir_all(&bc_dir);
    std::fs::create_dir_all(&bc_dir)?;

    let key = std::env::var("TOUCHLINK_MASTER_KEY").ok().filter(|k| !k.is_empty());
    println!("== touchlink: {}", if key.is_some() { "key index 4 enabled (key given to the build)" } else { "no master key (key indices 0 and 15 only)" });

    println!("== rustc -> bitcode ({TARGET}, board {board_name}, OTA image type {:#06x}, version {version})", known.image_type);
    // (source paths in panic locations relative, so that an image does not
    // depend on where it was built)
    let rustflags = format!(
        "-C linker-plugin-lto -C force-frame-pointers=no --emit=llvm-bc --remap-path-prefix={}=~ --remap-path-prefix={}=rust --remap-path-prefix={}=outglow",
        home().display(), rust_src.display(), root.display()
    );
    let cargo_dir = out.join("cargo");
    let mut cargo = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    cargo.current_dir(&here)
        .env("RUSTC_BOOTSTRAP", "1")
        .env("__CARGO_TESTS_ONLY_SRC_ROOT", &rust_src)
        .env("BULB_FILE_VERSION", &version)
        .env("BULB_BOARD", board_name)
        .env("RUSTFLAGS", rustflags)
        .env("CARGO_TARGET_DIR", &cargo_dir)
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
    // With -C linker-plugin-lto the library's members are bitcode, except
    // those of compiler_builtins, which rustc always compiles to machine code
    // (Thumb here). For such a member take the crate's bitcode that
    // --emit=llvm-bc left beside the rlib (same name and hash).
    let mut members: Vec<PathBuf> = std::fs::read_dir(&bc_dir)?.filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "o")).collect();
    members.sort();
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
    // rustc names its own CPU for the target in every function's attributes,
    // and those win over clang's -mcpu. Jump tables are off as for all TC32
    // code (a table dispatch reads PC in a way no vendor code does).
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
    let file = ota::File::with_image(ota::Header::new(MANUFACTURER, known.image_type, file_version, &format!("eglo-rs {board_name}")), staged.clone());
    let packed = file.pack();
    std::fs::write(out.join("bulb-tc32.ota"), &packed)?;
    println!("OK: {} ({} B), firmware {} B, mfg={MANUFACTURER:#06x} imageType={:#06x} fileVersion={file_version:#010x}",
             out.join("bulb-tc32.ota").display(), packed.len(), staged.len(), known.image_type);
    Ok(())
}

/// Every function's `"target-cpu"` becomes tc32, with jump tables off.
fn retarget(ll: &str) -> Result<String> {
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

#[cfg(test)]
mod tests {
    #[test]
    fn retarget_replaces_every_cpu() {
        let ll = "attributes #0 = { \"target-cpu\"=\"arm7tdmi\" nounwind }\nattributes #1 = { \"target-cpu\"=\"generic\" }";
        let r = super::retarget(ll).unwrap();
        assert_eq!(r.matches("\"no-jump-tables\"=\"true\" \"target-cpu\"=\"tc32\"").count(), 2);
        assert!(!r.contains("arm7tdmi"));
    }
}
