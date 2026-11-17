use std::path::PathBuf;
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::build::Ctx;
use crate::sh::{self, Env};

#[derive(clap::Args)]
pub struct Args {
    #[doc = " download again even if it is there"]
    #[arg(long)]
    force: bool,
    #[doc = " only print the bin directory (empty when not installed)"]
    #[arg(long)]
    print_path: bool,
}

pub fn commit() -> String {
    include_str!("../llvm-tc32.ref").trim().to_string()
}

fn cache(env: Env) -> PathBuf {
    let base = env("XDG_CACHE_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(env("HOME").unwrap_or_default()).join(".cache"));
    base.join("outglow")
}

pub fn installed_bin() -> PathBuf {
    installed_bin_in(&sh::process_env)
}

fn installed_bin_in(env: Env) -> PathBuf {
    let short: String = commit().chars().take(7).collect();
    cache(env).join(format!("llvm-tc32-{short}")).join("bin")
}

const PLATFORMS: [((&str, &str), &str); 2] = [(("linux", "x86_64"), "linux-x86_64"), (("macos", "aarch64"), "macos-arm64")];

fn platform_of(os: &str, arch: &str) -> Option<&'static str> {
    PLATFORMS.iter().find(|(p, _)| *p == (os, arch)).map(|(_, name)| *name)
}

pub fn run(a: Args) -> Result<()> {
    crate::build::with_real(|c| run_in(c, &a, (std::env::consts::OS, std::env::consts::ARCH)))
}

fn run_in(c: &mut Ctx, a: &Args, (os, arch): (&str, &str)) -> Result<()> {
    let bin = installed_bin_in(c.env);
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
    let Some(plat) = platform_of(os, arch) else {
        bail!(
            "no prebuilt llvm-tc32 for {os}-{arch}. Build it from https://github.com/goyamamoto/llvm-tc32 at {sha} \
             (cmake -S llvm -B build -G Ninja -DCMAKE_BUILD_TYPE=Release -DLLVM_ENABLE_PROJECTS=\"clang;lld\" \
             -DLLVM_TARGETS_TO_BUILD=ARM && ninja -C build) and set TC32_LLVM=<that>/build/bin"
        );
    };
    let url = format!("https://github.com/outglow/outglow/releases/download/llvm-tc32-{short}/llvm-tc32-{short}-{plat}.tar.xz");
    let dir = cache(c.env);
    std::fs::create_dir_all(&dir)?;
    let archive = dir.join(format!("llvm-tc32-{short}-{plat}.tar.xz"));
    println!("downloading {url}");
    let st = c.sh.status(Command::new("curl").args(["-fL", "--progress-bar", "-o"]).arg(&archive).arg(&url)).context("curl")?;
    if !st.success() {
        bail!("download failed: {url}");
    }
    let st = c.sh.status(Command::new("tar").arg("-xJf").arg(&archive).arg("-C").arg(&dir)).context("tar")?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sh::fake::{env, program, Fake, Tmp};

    const HOST: (&str, &str) = ("linux", "x86_64");

    #[test]
    fn where_the_compiler_goes() {
        assert_eq!(commit().len(), 40);
        assert_eq!(cache(&env(&[("XDG_CACHE_HOME", "/x"), ("HOME", "/h")])), PathBuf::from("/x/outglow"));
        assert_eq!(cache(&env(&[("HOME", "/h")])), PathBuf::from("/h/.cache/outglow"));
        assert_eq!(cache(&env(&[])), PathBuf::from(".cache/outglow"));
        assert_eq!(installed_bin_in(&env(&[("HOME", "/h")])), PathBuf::from(format!("/h/.cache/outglow/llvm-tc32-{}/bin", &commit()[..7])));
        assert!(installed_bin().ends_with(format!("outglow/llvm-tc32-{}/bin", &commit()[..7])));
        assert_eq!(platform_of("linux", "x86_64"), Some("linux-x86_64"));
        assert_eq!(platform_of("macos", "aarch64"), Some("macos-arm64"));
        assert_eq!(platform_of("macos", "x86_64"), None);
        assert_eq!(platform_of("windows", "x86_64"), None);
    }

    fn go(t: &Tmp, a: Args, host: (&str, &str), codes: (i32, i32, bool)) -> (Result<()>, Vec<String>) {
        let cache = t.0.display().to_string();
        let e = env(&[("XDG_CACHE_HOME", cache.as_str())]);
        let bin = installed_bin_in(&e);
        let mut fake = Fake::new(|c| match program(c).as_str() {
            "curl" => {
                let args: Vec<_> = c.get_args().collect();
                std::fs::write(args[3], "xz").unwrap();
                (codes.0, String::new())
            }
            _ => {
                if codes.2 {
                    std::fs::create_dir_all(&bin).unwrap();
                    std::fs::write(bin.join("clang"), "").unwrap();
                }
                (codes.1, String::new())
            }
        });
        let mut c = Ctx { root: t.0.clone(), sh: &mut fake, env: &e, exe: "/x".into(), toolchain: "/t".into() };
        let r = run_in(&mut c, &a, host);
        let log = fake.log.iter().map(|l| l.replace(&cache, "CACHE")).collect();
        (r, log)
    }

    #[test]
    fn a_download_unpacks_the_compiler() {
        let t = Tmp::new("tc");
        let (r, log) = go(&t, Args { force: false, print_path: true }, HOST, (0, 0, true));
        r.unwrap();
        assert!(log.is_empty());
        let (r, log) = go(&t, Args { force: false, print_path: false }, HOST, (0, 0, true));
        r.unwrap();
        let short = &commit()[..7];
        let archive = format!("CACHE/outglow/llvm-tc32-{short}-linux-x86_64.tar.xz");
        assert_eq!(
            log,
            [
                format!("curl -fL --progress-bar -o {archive} https://github.com/outglow/outglow/releases/download/llvm-tc32-{short}/llvm-tc32-{short}-linux-x86_64.tar.xz"),
                format!("tar -xJf {archive} -C CACHE/outglow"),
            ]
        );
        assert!(!t.path(&format!("outglow/llvm-tc32-{short}-linux-x86_64.tar.xz")).exists());
        let (r, log) = go(&t, Args { force: false, print_path: true }, HOST, (0, 0, true));
        r.unwrap();
        assert!(log.is_empty());
        let (r, log) = go(&t, Args { force: false, print_path: false }, ("plan9", "mips"), (0, 0, true));
        r.unwrap();
        assert!(log.is_empty());
        let (r, log) = go(&t, Args { force: true, print_path: false }, ("macos", "aarch64"), (0, 0, true));
        r.unwrap();
        assert!(log[0].ends_with("-macos-arm64.tar.xz"));
    }

    #[test]
    fn what_stops_a_download() {
        let t = Tmp::new("tc-fail");
        let a = || Args { force: false, print_path: false };
        let e = go(&t, a(), ("plan9", "mips"), (0, 0, true)).0.unwrap_err().to_string();
        assert!(e.starts_with("no prebuilt llvm-tc32 for plan9-mips. Build it from https://github.com/goyamamoto/llvm-tc32 at "), "{e}");
        let short = &commit()[..7];
        let e = go(&t, a(), HOST, (22, 0, true)).0.unwrap_err().to_string();
        assert_eq!(e, format!("download failed: https://github.com/outglow/outglow/releases/download/llvm-tc32-{short}/llvm-tc32-{short}-linux-x86_64.tar.xz"));
        let e = go(&t, a(), HOST, (0, 2, false)).0.unwrap_err().to_string();
        assert_eq!(e, format!("could not unpack {}", t.path(&format!("outglow/llvm-tc32-{short}-linux-x86_64.tar.xz")).display()));
        let (r, log) = go(&t, a(), HOST, (0, 0, false));
        assert_eq!(r.unwrap_err().to_string(), format!("{} has no clang after unpacking", t.path(&format!("outglow/llvm-tc32-{short}/bin")).display()));
        assert_eq!(log.len(), 2);
        let mut real = crate::sh::Real;
        let e = env(&[("XDG_CACHE_HOME", "/nonexistent/forbidden\0")]);
        let mut c = Ctx { root: t.0.clone(), sh: &mut real, env: &e, exe: "/x".into(), toolchain: "/t".into() };
        assert!(run_in(&mut c, &a(), HOST).is_err());
    }
}
