//! `cargo xtask toolchain`: the llvm-tc32 compiler `cargo xtask build` needs,
//! downloaded from this repository's releases (the toolchain workflow
//! publishes it) into the user's cache:
//! `~/.cache/outglow/llvm-tc32-<commit>` (`$XDG_CACHE_HOME` if set).
//! `cargo xtask build` finds it there without TC32_LLVM.

use std::path::PathBuf;
use std::process::Command;

use anyhow::{bail, Context, Result};

#[derive(clap::Args)]
pub struct Args {
    /// download again even if it is there
    #[arg(long)]
    force: bool,
    /// only print the bin directory (empty when not installed)
    #[arg(long)]
    print_path: bool,
}

/// The llvm-tc32 commit this repository builds with.
pub fn commit() -> String {
    include_str!("../llvm-tc32.ref").trim().to_string()
}

fn cache() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache")
    });
    base.join("outglow")
}

/// The bin directory of the downloaded compiler.
pub fn installed_bin() -> PathBuf {
    let short: String = commit().chars().take(7).collect();
    cache().join(format!("llvm-tc32-{short}")).join("bin")
}

fn platform() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some("linux-x86_64"),
        ("macos", "aarch64") => Some("macos-arm64"),
        _ => None,
    }
}

pub fn run(a: Args) -> Result<()> {
    let bin = installed_bin();
    if a.print_path {
        if bin.join("clang").exists() {
            println!("{}", bin.display());
        }
        return Ok(());
    }
    if bin.join("clang").exists() && !a.force {
        println!("llvm-tc32 is in {}", bin.display());
        return Ok(());
    }
    let sha = commit();
    let short = &sha[..7];
    let Some(plat) = platform() else {
        bail!(
            "no prebuilt llvm-tc32 for {}-{}. Build it from https://github.com/goyamamoto/llvm-tc32 at {sha} \
             (cmake -S llvm -B build -G Ninja -DCMAKE_BUILD_TYPE=Release -DLLVM_ENABLE_PROJECTS=\"clang;lld\" \
             -DLLVM_TARGETS_TO_BUILD=ARM && ninja -C build) and set TC32_LLVM=<that>/build/bin",
            std::env::consts::OS, std::env::consts::ARCH
        );
    };
    let url = format!("https://github.com/outglow/outglow/releases/download/llvm-tc32-{short}/llvm-tc32-{short}-{plat}.tar.xz");
    let dir = cache();
    std::fs::create_dir_all(&dir)?;
    let archive = dir.join(format!("llvm-tc32-{short}-{plat}.tar.xz"));
    println!("downloading {url}");
    let st = Command::new("curl").args(["-fL", "--progress-bar", "-o"]).arg(&archive).arg(&url).status().context("curl")?;
    if !st.success() {
        bail!("download failed: {url}");
    }
    let st = Command::new("tar").arg("-xJf").arg(&archive).arg("-C").arg(&dir).status().context("tar")?;
    if !st.success() {
        bail!("could not unpack {}", archive.display());
    }
    let _ = std::fs::remove_file(&archive);
    if !bin.join("clang").exists() {
        bail!("{} has no clang after unpacking", bin.display());
    }
    println!("llvm-tc32 is in {}\n`cargo xtask build` uses it from there (or: export TC32_LLVM={})", bin.display(), bin.display());
    Ok(())
}
