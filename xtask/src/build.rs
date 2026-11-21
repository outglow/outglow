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
    #[doc = " all boards of boards.tsv: the crates they share compiled once, the boards side by side"]
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

    fn run_all(&mut self, jobs: impl IntoIterator<Item = (String, Command)>) -> Result<()> {
        sh::run_all(self.sh, jobs.into_iter().collect())
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

fn check_rustc(c: &mut Ctx) -> Result<String> {
    let vv = c.output(Command::new(c.rustc()).arg("-vV"))?;
    let path = c.output(Command::new("sh").arg("-c").arg("command -v \"$0\"").arg(c.rustc())).unwrap_or_default();
    println!("== rustc: {} (LLVM {}) {}", vv.lines().next().unwrap_or("?"), rustc_field(&vv, "LLVM version"), path.trim());
    if let Some(w) = rustc_warning(&vv) {
        eprintln!("{w}");
    }
    Ok(vv)
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
    let boards = match a.all {
        true => boards_tsv(&c.root)?,
        false => vec![a.board.clone().unwrap_or_else(|| "ESMLFzm_w6_Dimm-4.63".into())],
    };
    build(c, &boards, a)
}

pub fn board_default(c: &mut Ctx, board_name: &str) -> Result<PathBuf> {
    let a = Args { board: Some(board_name.into()), all: false, no_ble: false, trace: false, no_sense: false, file_version: None, out_dir: None };
    build(c, &[board_name.into()], &a)?;
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

pub fn cache_id(vv: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    vv.hash(&mut h);
    WRAPPER_RULES.hash(&mut h);
    format!("{:016x}", h.finish())
}

const LOCK: &str = "outglow.lock";

fn cargo_dirs(dir: &Path) -> impl Iterator<Item = PathBuf> {
    std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| {
        let n = p.file_name().unwrap_or_default().to_string_lossy();
        n == "cargo" || n.starts_with("cargo-")
    })
}

pub fn remove_cargo_dirs(dir: &Path) {
    cargo_dirs(dir).for_each(|d| {
        let _ = std::fs::remove_dir_all(d);
    });
}

fn idle(dir: &Path) -> Option<std::fs::File> {
    let f = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.join(LOCK)).ok()?;
    f.try_lock().ok().map(|()| f)
}

pub fn prune(dir: &Path, keep: &str) {
    cargo_dirs(dir).filter(|d| !d.to_string_lossy().ends_with(keep)).for_each(|d| {
        if let Some(_held) = idle(&d) {
            let _ = std::fs::remove_dir_all(&d);
        }
    });
}

fn lock(dir: &Path) -> Result<std::fs::File> {
    std::fs::create_dir_all(dir)?;
    let f = std::fs::File::create(dir.join(LOCK))?;
    f.lock().map(|()| f).map_err(anyhow::Error::from)
}

struct Plan {
    llvm: PathBuf,
    clang: String,
    file_version: u32,
    version: String,
    release: String,
    features: Vec<&'static str>,
    suffix: String,
    rust_src: PathBuf,
    key: Option<String>,
    cargo_dir: PathBuf,
}

struct Unit<'a> {
    known: &'a board::Known,
    tag: String,
    out: PathBuf,
    modules: Vec<PathBuf>,
}

impl Plan {
    fn tag(&self, k: &board::Known) -> String {
        format!("board {} [{}]", k.name, self.features.join(","))
    }
}

fn plan(c: &mut Ctx, a: &Args) -> Result<Plan> {
    let llvm = (c.env)("TC32_LLVM").map(PathBuf::from).unwrap_or_else(|| {
        if c.toolchain.join("clang").exists() { c.toolchain.clone() } else { c.home().join("oss/llvm-tc32/build/bin") }
    });
    if !llvm.join("clang").exists() {
        bail!("no clang in {}: run `cargo xtask toolchain` (downloads llvm-tc32) or set TC32_LLVM", llvm.display());
    }
    let file_version = file_version(a, c.env)?;
    let (features, suffix) = features(a);
    let vv = check_rustc(c)?;
    let rust_src = rust_lib_src(c)?;
    let key = c.var("TOUCHLINK_MASTER_KEY").filter(|k| !k.is_empty());
    println!("== touchlink: {}", if key.is_some() { "key index 4 enabled (key given to the build)" } else { "no master key (key indices 0 and 15 only)" });
    let clang = c.output(Command::new(llvm.join("clang")).arg("--version"))?.lines().next().unwrap_or("").to_string();
    let id = cache_id(&vv);
    let shared = c.root.join("target/tc32-cargo");
    let cargo_dir = shared.join(format!("cargo{suffix}-{id}"));
    prune(&shared, &format!("-{id}"));
    Ok(Plan {
        llvm,
        clang,
        file_version,
        version: format!("{file_version:08x}"),
        release: format!("{}.{}.{}", file_version >> 24, (file_version >> 16) & 0xff, file_version & 0xffff),
        features,
        suffix,
        rust_src,
        key,
        cargo_dir,
    })
}

fn build(c: &mut Ctx, boards: &[String], a: &Args) -> Result<()> {
    let known = boards
        .iter()
        .map(|b| {
            board::known(b).ok_or_else(|| {
                let names: Vec<&str> = board::BOARDS.iter().map(|b| b.name).collect();
                anyhow!("unknown board {b}; the boards: {}", names.join(", "))
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let p = plan(c, a)?;
    let Some((first, rest)) = known.split_first() else { return Ok(()) };
    let target = c.root.join("target");
    let out_of = |k: &board::Known| a.out_dir.clone().unwrap_or_else(|| target.join(format!("{}{}", dir_name(k.name), p.suffix)));
    let job = |k, target| Job { known: k, tag: p.tag(k), out: out_of(k), target };
    let first = job(first, p.cargo_dir.clone());
    let rest: Vec<Job> = rest.iter().map(|k| job(k, out_of(k).join("cargo"))).collect();
    std::iter::once(&first).chain(&rest).try_for_each(|j| fresh_out(&j.out))?;
    let held = lock(&p.cargo_dir)?;
    let mut units = compile(c, &p, std::slice::from_ref(&first))?;
    let copied = c.run_all(rest.iter().map(|j| {
        let mut cp = Command::new("cp");
        cp.arg("-Rp").arg(&p.cargo_dir).arg(&j.target);
        (j.tag.clone(), cp)
    }));
    drop(held);
    let compiled = copied.and_then(|()| compile(c, &p, &rest));
    rest.iter().for_each(|j| {
        let _ = std::fs::remove_dir_all(&j.target);
    });
    units.extend(compiled?);
    link(c, &p, &units)
}

fn fresh_out(out: &Path) -> std::io::Result<()> {
    let _ = std::fs::remove_dir_all(out.join("bc"));
    remove_cargo_dirs(out);
    std::fs::create_dir_all(out.join("bc"))
}

struct Job<'a> {
    known: &'a board::Known,
    tag: String,
    out: PathBuf,
    target: PathBuf,
}

fn compile<'a>(c: &mut Ctx, p: &Plan, jobs: &[Job<'a>]) -> Result<Vec<Unit<'a>>> {
    let root = c.root.clone();
    for j in jobs {
        println!("== rustc -> bitcode ({TARGET}, board {}, OTA image type {:#06x}, version {})", j.known.name, j.known.image_type, p.version);
    }
    let rustflags = format!(
        "-C linker-plugin-lto -C force-frame-pointers=no --emit=llvm-bc --remap-path-prefix={}=~ --remap-path-prefix={}=rust --remap-path-prefix={}=outglow",
        c.home().display(), p.rust_src.display(), root.display()
    );
    let cargo_bin = (c.env)("CARGO").unwrap_or_else(|| "cargo".into());
    let exe = c.exe.clone();
    c.run_all(jobs.iter().map(|j| {
        let mut cargo = Command::new(&cargo_bin);
        cargo.current_dir(root.join("crates/bulb-tc32"))
            .env("RUSTC_BOOTSTRAP", "1")
            .env("__CARGO_TESTS_ONLY_SRC_ROOT", &p.rust_src)
            .env("BULB_FILE_VERSION", &p.version)
            .env("BULB_VERSION", &p.release)
            .env("BULB_BOARD", j.known.name)
            .env("RUSTFLAGS", &rustflags)
            .env("CARGO_TARGET_DIR", &j.target)
            .env("RUSTC_WRAPPER", &exe)
            .env(WRAPPER_ENV, "1")
            .env_remove("RUSTC_WORKSPACE_WRAPPER")
            .env_remove("CARGO_ENCODED_RUSTFLAGS")
            .args(["build", "--release", "--target", TARGET, "--no-default-features"])
            .arg(format!("--features={}", p.features.join(",")))
            .args(["-Zbuild-std=core", "-Zbuild-std-features=compiler-builtins-mem"]);
        match &p.key {
            Some(k) => cargo.env("TOUCHLINK_MASTER_KEY", k),
            None => cargo.env_remove("TOUCHLINK_MASTER_KEY"),
        };
        (j.tag.clone(), cargo)
    }))?;
    c.run_all(jobs.iter().map(|j| {
        let mut ar = Command::new(p.llvm.join("llvm-ar"));
        ar.current_dir(j.out.join("bc")).arg("x").arg(j.target.join(TARGET).join("release/libbulb_tc32.a"));
        (j.tag.clone(), ar)
    }))?;
    jobs.iter()
        .map(|j| {
            let modules = modules(&j.target.join(TARGET).join("release"), &j.out.join("bc")).context(j.tag.clone())?;
            Ok(Unit { known: j.known, tag: j.tag.clone(), out: j.out.clone(), modules })
        })
        .collect()
}

fn modules(release: &Path, bc_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut members: Vec<PathBuf> = std::fs::read_dir(bc_dir)?.filter_map(|e| e.ok().map(|e| e.path()))
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
        let copy = bc_dir.join(format!("{krate}.bc"));
        if !modules.contains(&copy) {
            std::fs::copy(&bc, &copy).with_context(|| format!("{} is machine code and {} does not exist", m.display(), bc.display()))?;
            modules.push(copy);
        }
    }
    Ok(modules)
}

fn link(c: &mut Ctx, p: &Plan, units: &[Unit]) -> Result<()> {
    let llvm = &p.llvm;
    let here = c.root.join("crates/bulb-tc32");
    println!("== bitcode -> TC32 object (llvm-tc32 {}): {} board(s)", p.clang, units.len());
    c.run_all(units.iter().map(|u| {
        let mut cmd = Command::new(llvm.join("llvm-link"));
        cmd.args(&u.modules).arg("-o").arg(u.out.join("rust.bc"));
        (u.tag.clone(), cmd)
    }))?;
    c.run_all(units.iter().map(|u| {
        let mut cmd = Command::new(llvm.join("llvm-dis"));
        cmd.arg(u.out.join("rust.bc")).arg("-o").arg(u.out.join("rust.thumb.ll"));
        (u.tag.clone(), cmd)
    }))?;
    units.iter().try_for_each(|u| ll_file(u).context(u.tag.clone()))?;
    let cflags = [format!("--target={TARGET}"), "-mcpu=tc32".into(), "-mthumb".into(), "-mfloat-abi=soft".into()];
    c.run_all(units.iter().flat_map(|u| {
        let mut code = Command::new(llvm.join("clang"));
        code.args(&cflags)
            .args(["-Oz", "-ffunction-sections", "-fdata-sections", "-mllvm", "-arm-load-store-opt=false", "-Wno-override-module", "-c"])
            .arg(u.out.join("rust.ll")).arg("-o").arg(u.out.join("rust.o"));
        let mut start = Command::new(llvm.join("clang"));
        start.args(&cflags).arg(format!("-DIMAGE_TYPE={:#06x}", u.known.image_type))
            .arg("-c").arg(here.join("boot/start.S")).arg("-o").arg(u.out.join("start.o"));
        [(u.tag.clone(), code), (u.tag.clone(), start)]
    }))?;

    println!("== link");
    c.run_all(units.iter().map(|u| {
        let mut cmd = Command::new(llvm.join("ld.lld"));
        cmd.arg("-T").arg(here.join("boot/link.ld")).arg(format!("--defsym=FW_OFFSET={:#x}", u.known.fw_offset))
            .arg(format!("--defsym=IMAGE_LIMIT={:#x}", ota::IMAGE_LIMIT)).arg("--gc-sections")
            .arg(format!("-Map={}", u.out.join("bulb-tc32.map").display()))
            .arg(u.out.join("start.o")).arg(u.out.join("rust.o")).arg("-o").arg(u.out.join("bulb-tc32.elf"));
        (u.tag.clone(), cmd)
    }))?;
    let images = units.iter().map(|u| bin_file(u).context(u.tag.clone())).collect::<Result<Vec<_>>>()?;
    c.run(Command::new(llvm.join("llvm-size")).args(units.iter().map(|u| u.out.join("bulb-tc32.elf"))))?;

    println!("== OTA image");
    units.iter().zip(images).try_for_each(|(u, image)| ota_file(p, u, &image).context(u.tag.clone()))
}

fn ll_file(u: &Unit) -> Result<()> {
    let ll = retarget(&std::fs::read_to_string(u.out.join("rust.thumb.ll"))?)?;
    Ok(std::fs::write(u.out.join("rust.ll"), ll)?)
}

fn bin_file(u: &Unit) -> Result<Vec<u8>> {
    let elf_path = u.out.join("bulb-tc32.elf");
    let (image, segments) = elf::image_of(&std::fs::read(&elf_path)?).map_err(|e| anyhow!("{}: {e}", elf_path.display()))?;
    std::fs::write(u.out.join("bulb-tc32.bin"), &image)?;
    println!("{}: {} bytes from {segments} segment(s)", u.out.join("bulb-tc32.bin").display(), image.len());
    Ok(image)
}

fn ota_file(p: &Plan, u: &Unit, image: &[u8]) -> Result<()> {
    let staged = telink::finalize(image);
    telink::check(&staged).map_err(|e| anyhow!("the finished image fails its own check: {e}"))?;
    std::fs::write(u.out.join("bulb-tc32-ota.bin"), &staged)?;
    let (image_type, file_version) = (u.known.image_type, p.file_version);
    let file = ota::File::with_image(ota::Header::new(MANUFACTURER, image_type, file_version, &format!("outglow {}", u.known.name)), staged.clone());
    let packed = file.pack();
    let at = u.out.join("bulb-tc32.ota");
    ota::fits_staging(&packed).map_err(|e| anyhow!("{}: {e}", at.display()))?;
    std::fs::write(&at, &packed)?;
    println!("OK: {} ({} B), firmware {} B, mfg={MANUFACTURER:#06x} imageType={image_type:#06x} fileVersion={file_version:#010x}",
             at.display(), packed.len(), staged.len());
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
