mod ble;
mod zigbee;

use anyhow::{bail, Context, Result};
use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct Mutation {
    pub name: &'static str,
    pub file: &'static str,
    pub old: &'static str,
    pub new: &'static str,
}

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Suite {
    Zigbee,
    Ble,
    All,
}

#[derive(clap::Args)]
pub struct Args {
    #[doc = " Which list to run"]
    #[arg(value_enum, default_value = "all")]
    suite: Suite,
    #[doc = " Only the mutations whose names contain one of these words, separated by"]
    #[doc = " `|` (e.g. \"GP|touchlink\"); the `ONLY` variable of the old scripts works too"]
    #[arg(long, short)]
    filter: Option<String>,
    #[doc = " Print the mutations that would run and stop"]
    #[arg(long)]
    list: bool,
    #[doc = " Mutations tested at once, each in its own scratch copy and target directory"]
    #[arg(long, short, default_value_t = 1)]
    jobs: usize,
    #[doc = " Stop at the first mutation that is missed or stale"]
    #[arg(long)]
    fail_fast: bool,
    #[doc = " Seconds one test run may take; a run that takes longer is killed and"]
    #[doc = " counted as caught (the suite did not pass). 0 = no limit"]
    #[arg(long, default_value_t = 900)]
    timeout: u64,
}

const CRATES: &[&str] = &["crates/bulb-core", "crates/bulb-sim", "crates/zigbee", "crates/ble"];

pub struct List {
    krate: &'static str,
    mutations: &'static [Mutation],
}

const LISTS: &[List] = &[
    List { krate: "zigbee", mutations: zigbee::MUTATIONS },
    List { krate: "ble", mutations: ble::MUTATIONS },
];

pub fn run(a: Args) -> Result<()> {
    let env = |k: &str| std::env::var_os(k);
    let cargo = env("CARGO").unwrap_or_else(|| "cargo".into());
    run_in(&Space { root: crate::root(), crates: CRATES, lists: LISTS, cargo }, &a, env("ONLY"))
}

pub struct Space {
    pub root: PathBuf,
    pub crates: &'static [&'static str],
    pub lists: &'static [List],
    pub cargo: OsString,
}

fn chosen(suite: Suite, krate: &str) -> bool {
    match suite {
        Suite::All => true,
        Suite::Zigbee => krate == "zigbee",
        Suite::Ble => krate == "ble",
    }
}

pub fn run_in(sp: &Space, a: &Args, only: Option<OsString>) -> Result<()> {
    let filter = a.filter.clone().or_else(|| only.map(|o| o.to_string_lossy().into_owned())).filter(|f| !f.is_empty());
    let words: Vec<&str> = filter.as_deref().map(|f| f.split('|').collect()).unwrap_or_default();
    let picked = |m: &Mutation| words.is_empty() || words.iter().any(|w| m.name.contains(w));
    let lists: Vec<(&List, Vec<&Mutation>)> = sp
        .lists
        .iter()
        .filter(|l| chosen(a.suite, l.krate))
        .map(|l| (l, l.mutations.iter().filter(|m| picked(m)).collect()))
        .collect();

    if a.list {
        for (l, ms) in &lists {
            for m in ms {
                println!("{:<7}{:<22}{}", l.krate, m.file, m.name);
            }
            println!("{}: {} of {} mutations", l.krate, ms.len(), l.mutations.len());
        }
        return Ok(());
    }

    let jobs = a.jobs.max(1);
    let scratch = sp.root.join("target").join("mutants");
    let timeout = (a.timeout > 0).then(|| Duration::from_secs(a.timeout));
    let mut ok = true;
    for (l, ms) in lists.iter().filter(|(_, ms)| !ms.is_empty()) {
        let started = Instant::now();
        let r = run_list(sp, l.krate, ms, &scratch, jobs, a.fail_fast, timeout)?;
        println!(
            "{}: {} mutations run: {} caught, {} missed, {} stale ({:.0} s)",
            l.krate,
            r.ran,
            r.ran - r.missed.len(),
            r.missed.len(),
            r.stale.len(),
            started.elapsed().as_secs_f64()
        );
        for name in &r.missed {
            println!("  missed: {name}");
        }
        for name in &r.stale {
            println!("  stale:  {name}");
        }
        ok &= r.missed.is_empty() && r.stale.is_empty();
        if !ok && a.fail_fast {
            break;
        }
    }
    if !ok {
        bail!("mutations missed or stale");
    }
    Ok(())
}

#[derive(Default)]
struct Outcome {
    ran: usize,
    missed: Vec<&'static str>,
    stale: Vec<&'static str>,
}

enum Verdict {
    Caught(String),
    Missed,
    Stale(String),
}

fn run_list(
    sp: &Space,
    krate: &str,
    ms: &[&'static Mutation],
    scratch: &Path,
    jobs: usize,
    fail_fast: bool,
    timeout: Option<Duration>,
) -> Result<Outcome> {
    let workers = jobs.min(ms.len());
    let next = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    let verdicts: Mutex<Vec<Option<Verdict>>> = Mutex::new((0..ms.len()).map(|_| None).collect());
    std::thread::scope(|s| -> Result<()> {
        let handles: Vec<_> = (0..workers)
            .map(|w| {
                let (next, stop, verdicts) = (&next, &stop, &verdicts);
                s.spawn(move || -> Result<()> {
                    let dir = scratch.join(format!("w{w}"));
                    let ws = dir.join("ws");
                    mirror_workspace(sp, &ws).with_context(|| format!("mirroring the workspace to {}", ws.display()))?;
                    loop {
                        if stop.load(Ordering::SeqCst) {
                            return Ok(());
                        }
                        let i = next.fetch_add(1, Ordering::SeqCst);
                        let Some(m) = ms.get(i) else { return Ok(()) };
                        let v = run_one(&sp.cargo, krate, m, &ws, &dir, timeout)?;
                        let line = match &v {
                            Verdict::Caught(why) => format!("caught  {}: {why}", m.name),
                            Verdict::Missed => format!("MISSED  {}", m.name),
                            Verdict::Stale(why) => why.clone(),
                        };
                        {
                            let out = std::io::stdout();
                            let mut out = out.lock();
                            let _ = writeln!(out, "{line}");
                            let _ = out.flush();
                        }
                        if fail_fast && !matches!(v, Verdict::Caught(_)) {
                            stop.store(true, Ordering::SeqCst);
                        }
                        verdicts.lock().unwrap()[i] = Some(v);
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().expect("a mutation worker panicked")?;
        }
        Ok(())
    })?;
    let mut r = Outcome::default();
    for (m, v) in ms.iter().zip(verdicts.into_inner().unwrap()) {
        match v {
            None => {}
            Some(Verdict::Caught(_)) => r.ran += 1,
            Some(Verdict::Missed) => {
                r.ran += 1;
                r.missed.push(m.name);
            }
            Some(Verdict::Stale(why)) => {
                if why.starts_with("BROKEN") {
                    r.ran += 1;
                }
                r.stale.push(m.name);
            }
        }
    }
    Ok(r)
}

fn run_one(cargo: &OsStr, krate: &str, m: &Mutation, ws: &Path, dir: &Path, timeout: Option<Duration>) -> Result<Verdict> {
    let file = ws.join("crates").join(krate).join("src").join(m.file);
    let Ok(source) = fs::read_to_string(&file) else {
        return Ok(Verdict::Stale(format!("STALE   {}: no file {}", m.name, m.file)));
    };
    let n = source.matches(m.old).count();
    if n != 1 {
        return Ok(Verdict::Stale(format!("STALE   {}: the text occurs {n} times in {}", m.name, m.file)));
    }
    write_whole(&file, source.replacen(m.old, m.new, 1).as_bytes())?;
    let result = cargo_test(cargo, krate, ws, dir, timeout);
    write_whole(&file, source.as_bytes()).with_context(|| format!("restoring {}", file.display()))?;
    let (out, success, timed_out) = result?;
    Ok(verdict(m.name, &out, success, timed_out))
}

fn verdict(name: &str, out: &str, success: bool, timed_out: bool) -> Verdict {
    let failed: Vec<&str> = out
        .lines()
        .filter(|l| l.starts_with("test ") && l.trim_end().ends_with("FAILED"))
        .filter_map(|l| l.split_whitespace().nth(1))
        .collect();
    if out.contains("error[") || out.contains("could not compile") {
        Verdict::Stale(format!("BROKEN  {name}: does not compile"))
    } else if let Some(first) = failed.first() {
        Verdict::Caught(format!("{} test(s), e.g. {first}", failed.len()))
    } else if timed_out {
        Verdict::Caught("the tests did not finish in time".into())
    } else if !success {
        Verdict::Caught("a test process failed without a test result".into())
    } else {
        Verdict::Missed
    }
}

fn cargo_test(cargo: &OsStr, krate: &str, ws: &Path, dir: &Path, timeout: Option<Duration>) -> Result<(String, bool, bool)> {
    let (out_path, err_path) = (dir.join("stdout.txt"), dir.join("stderr.txt"));
    let mut child = Command::new(cargo)
        .args(["test", "-p", krate])
        .current_dir(ws)
        .env("CARGO_TARGET_DIR", dir.join("target"))
        .stdin(Stdio::null())
        .stdout(fs::File::create(&out_path)?)
        .stderr(fs::File::create(&err_path)?)
        .spawn()
        .context("starting cargo test")?;
    let started = Instant::now();
    let (status, timed_out) = loop {
        if let Some(status) = child.try_wait()? {
            break (Some(status), false);
        }
        if timeout.is_some_and(|t| started.elapsed() > t) {
            kill_tree(child.id());
            let _ = child.kill();
            let _ = child.wait();
            break (None, true);
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let mut out = fs::read_to_string(&out_path).unwrap_or_default();
    out.push_str(&fs::read_to_string(&err_path).unwrap_or_default());
    Ok((out, status.is_some_and(|s| s.success()), timed_out))
}

fn kill_tree(pid: u32) {
    let children = Command::new("pgrep").args(["-P", &pid.to_string()]).output().map(|o| o.stdout).unwrap_or_default();
    String::from_utf8_lossy(&children).split_whitespace().filter_map(|c| c.parse().ok()).for_each(kill_tree);
    let _ = Command::new("kill").args(["-KILL", &pid.to_string()]).status();
}

fn mirror_workspace(sp: &Space, ws: &Path) -> Result<()> {
    let root = &sp.root;
    fs::create_dir_all(ws)?;
    let members: Vec<String> = sp.crates.iter().map(|c| format!("\"{c}\"")).collect();
    let manifest = format!(
        "# Scratch copy for `cargo xtask mutants`; rewritten on every run.\n\
         [workspace]\nresolver = \"2\"\nmembers = [{}]\n",
        members.join(", ")
    );
    put(&ws.join("Cargo.toml"), manifest.as_bytes())?;
    for f in ["Cargo.lock", "rust-toolchain.toml"] {
        let src = root.join(f);
        if src.exists() {
            put(&ws.join(f), &fs::read(&src)?)?;
        }
    }
    for c in sp.crates {
        mirror_dir(&root.join(c), &ws.join(c))?;
    }
    Ok(())
}

fn mirror_dir(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    let mut seen = BTreeSet::new();
    for e in fs::read_dir(src)? {
        let e = e?;
        let name = e.file_name();
        if name == "target" {
            continue;
        }
        seen.insert(name.clone());
        let (s, d) = (e.path(), dst.join(&name));
        if e.file_type()?.is_dir() {
            if d.is_file() {
                fs::remove_file(&d)?;
            }
            mirror_dir(&s, &d)?;
        } else {
            if d.is_dir() {
                fs::remove_dir_all(&d)?;
            }
            put(&d, &fs::read(&s)?)?;
        }
    }
    for e in fs::read_dir(dst)? {
        let e = e?;
        if !seen.contains(&e.file_name()) {
            let p: PathBuf = e.path();
            if e.file_type()?.is_dir() {
                fs::remove_dir_all(&p)?;
            } else {
                fs::remove_file(&p)?;
            }
        }
    }
    Ok(())
}

fn put(path: &Path, data: &[u8]) -> Result<()> {
    if fs::read(path).ok().as_deref() == Some(data) {
        return Ok(());
    }
    path.parent().map(fs::create_dir_all).transpose()?;
    write_whole(path, data)
}

fn write_whole(path: &Path, data: &[u8]) -> Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".mutants-tmp");
    let tmp = PathBuf::from(tmp);
    let r = fs::write(&tmp, data).and_then(|()| fs::rename(&tmp, path));
    if r.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    r.with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests;
