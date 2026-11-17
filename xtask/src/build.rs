use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use outglow_image::{board, elf, ota, telink};

use crate::sh::{self, Env, Sh};

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

pub struct Ctx<'a> {
    pub root: PathBuf,
    pub sh: &'a mut dyn Sh,
    pub env: Env<'a>,
    pub exe: PathBuf,
    pub toolchain: PathBuf,
}

impl Ctx<'_> {
    fn var(&self, k: &str) -> Option<String> {
        sh::var(self.env, k)
    }

    fn home(&self) -> PathBuf {
        (self.env)("HOME").map(PathBuf::from).unwrap_or_default()
    }

    fn rustc(&self) -> OsString {
        (self.env)("RUSTC").unwrap_or_else(|| "rustc".into())
    }

    fn run(&mut self, cmd: &mut Command) -> Result<()> {
        sh::run(self.sh, cmd)
    }

    fn output(&mut self, cmd: &mut Command) -> Result<String> {
        sh::output(self.sh, cmd)
    }
}

pub fn with_real<T>(f: impl FnOnce(&mut Ctx) -> Result<T>) -> Result<T> {
    let exe = std::env::current_exe()?;
    let mut real = sh::Real;
    let mut ctx = Ctx { root: crate::root(), sh: &mut real, env: &sh::process_env, exe, toolchain: crate::toolchain::installed_bin() };
    f(&mut ctx)
}

const PINNED_RUSTC: [(&str, &str); 3] =
    [("release", "1.99.0"), ("commit-hash", "b940084d7eb6a299eb4bfeb8e34901bc051e7ac4"), ("LLVM version", "21.1.8")];

const WRAPPER_RULES: u32 = 1;

pub fn rustc_field(vv: &str, k: &str) -> String {
    vv.lines().find_map(|l| l.strip_prefix(k).and_then(|r| r.strip_prefix(": "))).unwrap_or("?").trim().to_string()
}

pub fn unpinned(vv: &str) -> Vec<String> {
    PINNED_RUSTC
        .iter()
        .filter(|(k, v)| rustc_field(vv, k) != *v)
        .map(|(k, v)| format!("{k} {} (pinned {v})", rustc_field(vv, k)))
        .collect()
}

fn check_rustc(c: &mut Ctx) -> Result<()> {
    let vv = c.output(Command::new(c.rustc()).arg("-vV"))?;
    let path = c.output(Command::new("sh").arg("-c").arg("command -v \"$0\"").arg(c.rustc())).unwrap_or_default();
    println!("== rustc: {} (LLVM {}) {}", vv.lines().next().unwrap_or("?"), rustc_field(&vv, "LLVM version"), path.trim());
    if let Some(w) = rustc_warning(&vv) {
        eprintln!("{w}");
    }
    Ok(())
}

pub fn rustc_warning(vv: &str) -> Option<String> {
    let off = unpinned(vv);
    (!off.is_empty()).then(|| {
        format!(
            "\nWARNING: this rustc is not the pinned one: {}.\n\
             WARNING: the image will NOT be byte-identical to CI's (other inlining, other layout, other timing in the emulator).\n\
             WARNING: build in the Nix shell: `nix develop`, then `cargo xtask build` (or `nix develop --command cargo xtask build`).\n",
            off.join(", ")
        )
    })
}

fn locked_nixpkgs(root: &Path) -> Result<String> {
    let lock: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(root.join("flake.lock"))?)?;
    lock["nodes"]["nixpkgs"]["locked"]["rev"].as_str().map(String::from).ok_or_else(|| anyhow!("no nixpkgs revision in flake.lock"))
}

fn rust_lib_src(c: &mut Ctx) -> Result<PathBuf> {
    if let Some(p) = (c.env)("RUST_LIB_SRC") {
        return Ok(p.into());
    }
    let sysroot = c.output(Command::new(c.rustc()).args(["--print", "sysroot"]))?;
    let src = Path::new(sysroot.trim()).join("lib/rustlib/src/rust/library");
    if src.join("core").is_dir() {
        return Ok(src);
    }
    let flake = format!("github:NixOS/nixpkgs/{}#rustPlatform.rustLibSrc", locked_nixpkgs(&c.root)?);
    let p = c
        .output(Command::new("nix").args(["build", "--no-link", "--print-out-paths", &flake]))
        .context("no Rust standard library sources: run in `nix develop`, or set RUST_LIB_SRC")?;
    Ok(PathBuf::from(p.trim()))
}

pub const WRAPPER_ENV: &str = "OUTGLOW_RUSTC_WRAPPER";

pub fn wrapped(args: Vec<OsString>) -> Vec<OsString> {
    let s: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
    let target = s.windows(2).any(|w| w[0] == "--target" && w[1] == TARGET) || s.contains(&format!("--target={TARGET}"));
    let Some(name) = s.windows(2).find(|w| w[0] == "--crate-name").map(|w| w[1].clone()).filter(|_| target) else {
        return args;
    };
    let after_c = std::iter::once(false).chain(s.iter().map(|a| a == "-C"));
    args.into_iter()
        .zip(s.iter().zip(after_c))
        .map(|(arg, (text, after_c))| {
            if after_c && text.starts_with("metadata=") {
                format!("metadata=outglow-{name}").into()
            } else if text.starts_with("-Cmetadata=") {
                format!("-Cmetadata=outglow-{name}").into()
            } else {
                arg
            }
        })
        .collect()
}

pub fn rustc_wrapper(env: Env, args: Vec<OsString>) -> Option<i32> {
    env(WRAPPER_ENV)?;
    let mut args = args.into_iter().skip(1);
    let Some(rustc) = args.next() else { return Some(2) };
    match Command::new(rustc).args(wrapped(args.collect())).status() {
        Ok(st) => Some(st.code().unwrap_or(1)),
        Err(e) => {
            eprintln!("outglow rustc wrapper: {e}");
            Some(1)
        }
    }
}

pub fn boards_tsv(root: &Path) -> Result<Vec<String>> {
    let t = std::fs::read_to_string(root.join("crates/bulb-tc32/boards.tsv"))?;
    Ok(t.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()).filter_map(|l| l.split('\t').next()).map(String::from).collect())
}

pub fn run(a: Args) -> Result<()> {
    with_real(|c| run_in(c, &a))
}

pub fn run_in(c: &mut Ctx, a: &Args) -> Result<()> {
    if a.all {
        for b in boards_tsv(&c.root)? {
            build(c, &b, a, None)?;
        }
        return Ok(());
    }
    let b = a.board.clone().unwrap_or_else(|| "ESMLFzm_w6_Dimm-4.63".into());
    build(c, &b, a, a.out_dir.clone())
}

pub fn board_default(c: &mut Ctx, board_name: &str) -> Result<PathBuf> {
    let a = Args { board: Some(board_name.into()), all: false, no_ble: false, trace: false, no_sense: false, file_version: None, out_dir: None };
    build(c, board_name, &a, None)?;
    Ok(default_dir(&c.root, board_name))
}

pub fn dir_name(board_name: &str) -> String {
    match board_name {
        "ESMLFzm_w6_Dimm-4.63" => "tc32".to_string(),
        "ESMLFzm_w6_TW-4.63" => "tc32-tw".to_string(),
        b => format!("tc32-{}", b.to_lowercase()),
    }
}

pub fn default_dir(root: &Path, board_name: &str) -> PathBuf {
    root.join("target").join(dir_name(board_name))
}

pub fn features(a: &Args) -> (Vec<&'static str>, String) {
    let flags = [(!a.no_ble, "ble", ""), (a.no_ble, "", "-noble"), (!a.no_sense, "sense", ""), (a.no_sense, "", "-nosense"), (a.trace, "trace", "-trace")];
    let on: Vec<_> = flags.iter().filter(|f| f.0).collect();
    (on.iter().map(|f| f.1).filter(|f| !f.is_empty()).collect(), on.iter().map(|f| f.2).collect())
}

pub fn file_version(a: &Args, env: Env) -> Result<u32> {
    let version = a.file_version.clone().or_else(|| sh::var(env, "BULB_FILE_VERSION")).unwrap_or_else(|| "01000000".into());
    u32::from_str_radix(version.trim_start_matches("0x"), 16).with_context(|| format!("file version {version}"))
}

pub fn member_order(p: &Path) -> String {
    let n = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
    n.split_once('.').map_or(n.clone(), |(_, rest)| rest.to_string())
}

fn build(c: &mut Ctx, board_name: &str, a: &Args, out_dir: Option<PathBuf>) -> Result<()> {
    let root = c.root.clone();
    let here = root.join("crates/bulb-tc32");
    let known = board::known(board_name).ok_or_else(|| {
        let names: Vec<&str> = board::BOARDS.iter().map(|b| b.name).collect();
        anyhow!("unknown board {board_name}; the boards: {}", names.join(", "))
    })?;
    let llvm = (c.env)("TC32_LLVM").map(PathBuf::from).unwrap_or_else(|| {
        if c.toolchain.join("clang").exists() { c.toolchain.clone() } else { c.home().join("oss/llvm-tc32/build/bin") }
    });
    if !llvm.join("clang").exists() {
        bail!("no clang in {}: run `cargo xtask toolchain` (downloads llvm-tc32) or set TC32_LLVM", llvm.display());
    }
    let file_version = file_version(a, c.env)?;
    let version = format!("{file_version:08x}");
    let release = format!("{}.{}.{}", file_version >> 24, (file_version >> 16) & 0xff, file_version & 0xffff);
    let (features, suffix) = features(a);
    let out = out_dir.unwrap_or_else(|| root.join("target").join(format!("{}{suffix}", dir_name(board_name))));
    check_rustc(c)?;
    let rust_src = rust_lib_src(c)?;
    std::fs::create_dir_all(&out)?;
    let bc_dir = out.join("bc");
    let _ = std::fs::remove_dir_all(&bc_dir);
    std::fs::create_dir_all(&bc_dir)?;

    let key = c.var("TOUCHLINK_MASTER_KEY").filter(|k| !k.is_empty());
    println!("== touchlink: {}", if key.is_some() { "key index 4 enabled (key given to the build)" } else { "no master key (key indices 0 and 15 only)" });

    println!("== rustc -> bitcode ({TARGET}, board {board_name}, OTA image type {:#06x}, version {version})", known.image_type);
    let rustflags = format!(
        "-C linker-plugin-lto -C force-frame-pointers=no --emit=llvm-bc --remap-path-prefix={}=~ --remap-path-prefix={}=rust --remap-path-prefix={}=outglow",
        c.home().display(), rust_src.display(), root.display()
    );
    let cache_id = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        c.output(Command::new(c.rustc()).arg("-vV"))?.hash(&mut h);
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
    let mut cargo = Command::new((c.env)("CARGO").unwrap_or_else(|| "cargo".into()));
    cargo.current_dir(&here)
        .env("RUSTC_BOOTSTRAP", "1")
        .env("__CARGO_TESTS_ONLY_SRC_ROOT", &rust_src)
        .env("BULB_FILE_VERSION", &version)
        .env("BULB_VERSION", &release)
        .env("BULB_BOARD", board_name)
        .env("RUSTFLAGS", rustflags)
        .env("CARGO_TARGET_DIR", &cargo_dir)
        .env("RUSTC_WRAPPER", &c.exe)
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
    c.run(&mut cargo)?;

    let clang_version = c.output(Command::new(llvm.join("clang")).arg("--version"))?;
    println!("== bitcode -> TC32 object (llvm-tc32 {})", clang_version.lines().next().unwrap_or(""));
    let release = cargo_dir.join(TARGET).join("release");
    c.run(Command::new(llvm.join("llvm-ar")).current_dir(&bc_dir).arg("x").arg(release.join("libbulb_tc32.a")))?;
    let mut members: Vec<PathBuf> = std::fs::read_dir(&bc_dir)?.filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "o")).collect();
    members.sort_by_key(|p| member_order(p));
    let mut modules: Vec<PathBuf> = Vec::new();
    for m in members {
        let head = std::fs::read(&m)?;
        if head.starts_with(b"BC") {
            modules.push(m);
            continue;
        }
        let name = m.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let krate = name.split('.').next().unwrap_or(&name);
        let bc = release.join("deps").join(format!("{krate}.bc"));
        if !bc.exists() {
            bail!("{} is machine code and {} does not exist", m.display(), bc.display());
        }
        if !modules.contains(&bc) {
            modules.push(bc);
        }
    }
    c.run(Command::new(llvm.join("llvm-link")).args(&modules).arg("-o").arg(out.join("rust.bc")))?;
    c.run(Command::new(llvm.join("llvm-dis")).arg(out.join("rust.bc")).arg("-o").arg(out.join("rust.thumb.ll")))?;
    let ll = retarget(&std::fs::read_to_string(out.join("rust.thumb.ll"))?)?;
    std::fs::write(out.join("rust.ll"), ll)?;

    let cflags = [format!("--target={TARGET}"), "-mcpu=tc32".into(), "-mthumb".into(), "-mfloat-abi=soft".into()];
    c.run(Command::new(llvm.join("clang")).args(&cflags)
        .args(["-Oz", "-ffunction-sections", "-fdata-sections", "-mllvm", "-arm-load-store-opt=false", "-Wno-override-module", "-c"])
        .arg(out.join("rust.ll")).arg("-o").arg(out.join("rust.o")))?;
    c.run(Command::new(llvm.join("clang")).args(&cflags).arg(format!("-DIMAGE_TYPE={:#06x}", known.image_type))
        .arg("-c").arg(here.join("boot/start.S")).arg("-o").arg(out.join("start.o")))?;

    println!("== link");
    let elf_path = out.join("bulb-tc32.elf");
    c.run(Command::new(llvm.join("ld.lld")).arg("-T").arg(here.join("boot/link.ld")).arg("--gc-sections")
        .arg(format!("-Map={}", out.join("bulb-tc32.map").display()))
        .arg(out.join("start.o")).arg(out.join("rust.o")).arg("-o").arg(&elf_path))?;
    let (image, segments) = elf::image_of(&std::fs::read(&elf_path)?).map_err(|e| anyhow!("{}: {e}", elf_path.display()))?;
    std::fs::write(out.join("bulb-tc32.bin"), &image)?;
    println!("{}: {} bytes from {segments} segment(s)", out.join("bulb-tc32.bin").display(), image.len());
    c.run(Command::new(llvm.join("llvm-size")).arg(&elf_path))?;

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
    if !out.contains("\"target-cpu\"=\"tc32\"") {
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
    Ok(out)
}

#[cfg(test)]
mod tests;
