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
    #[doc = " Stop at the first mutation that is missed, unviable or stale"]
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
        println!("{} ({:.0} s)", r.summary(l.krate), started.elapsed().as_secs_f64());
        for (kind, names) in [("missed:  ", &r.missed), ("unviable:", &r.unviable), ("stale:   ", &r.stale)] {
            for name in names {
                println!("  {kind} {name}");
            }
        }
        ok &= r.missed.is_empty() && r.unviable.is_empty() && r.stale.is_empty();
        if !ok && a.fail_fast {
            break;
        }
    }
    if !ok {
        bail!("mutations missed, unviable or stale");
    }
    Ok(())
}

#[derive(Default)]
struct Outcome {
    caught: usize,
    missed: Vec<&'static str>,
    unviable: Vec<&'static str>,
    stale: Vec<&'static str>,
}

impl Outcome {
    fn summary(&self, krate: &str) -> String {
        format!(
            "{krate}: {} mutations run: {} caught, {} missed, {} unviable, {} stale",
            self.caught + self.missed.len() + self.unviable.len(),
            self.caught,
            self.missed.len(),
            self.unviable.len(),
            self.stale.len()
        )
    }
}

enum Verdict {
    Caught(String),
    Missed,
    Unviable(String),
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
    let crates = path_closure(sp)?;
    baseline(sp, &crates, krate, scratch, timeout)?;
    let workers = jobs.min(ms.len());
    let next = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    let verdicts: Mutex<Vec<Option<Verdict>>> = Mutex::new((0..ms.len()).map(|_| None).collect());
    std::thread::scope(|s| -> Result<()> {
        let handles: Vec<_> = (0..workers)
            .map(|w| {
                let (next, stop, verdicts, crates) = (&next, &stop, &verdicts, &crates);
                s.spawn(move || -> Result<()> {
                    let dir = scratch.join(format!("w{w}"));
                    let ws = dir.join("ws");
                    mirror_workspace(sp, crates, &ws).with_context(|| format!("mirroring the workspace to {}", ws.display()))?;
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
                            Verdict::Unviable(why) => format!("UNVIABLE {}: {why}", m.name),
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
            Some(Verdict::Caught(_)) => r.caught += 1,
            Some(Verdict::Missed) => r.missed.push(m.name),
            Some(Verdict::Unviable(_)) => r.unviable.push(m.name),
            Some(Verdict::Stale(_)) => r.stale.push(m.name),
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
    Ok(verdict(&out, success, timed_out))
}

fn verdict(out: &str, success: bool, timed_out: bool) -> Verdict {
    let failed: Vec<&str> = out
        .lines()
        .filter(|l| l.starts_with("test ") && l.trim_end().ends_with("FAILED"))
        .filter_map(|l| l.split_whitespace().nth(1))
        .collect();
    let ran_tests = out.lines().any(|l| l.trim_start().starts_with("Running "));
    if out.contains("error[") || out.contains("could not compile") {
        Verdict::Unviable("does not compile".into())
    } else if let Some(first) = failed.first() {
        Verdict::Caught(format!("{} test(s), e.g. {first}", failed.len()))
    } else if timed_out {
        Verdict::Caught("the tests did not finish in time".into())
    } else if !success && ran_tests {
        Verdict::Caught("a test process failed without a test result".into())
    } else if !success {
        Verdict::Unviable(format!("cargo failed before running a test: {}", last_error(out)))
    } else {
        Verdict::Missed
    }
}

fn last_error(out: &str) -> &str {
    out.lines().rev().find(|l| l.starts_with("error")).or_else(|| out.lines().rev().find(|l| !l.trim().is_empty())).unwrap_or("no output")
}

fn baseline(sp: &Space, crates: &[String], krate: &str, scratch: &Path, timeout: Option<Duration>) -> Result<()> {
    let dir = scratch.join("w0");
    let ws = dir.join("ws");
    mirror_workspace(sp, crates, &ws).with_context(|| format!("mirroring the workspace to {}", ws.display()))?;
    let (out, success, timed_out) = cargo_test(&sp.cargo, krate, &ws, &dir, timeout)?;
    match verdict(&out, success, timed_out) {
        Verdict::Missed => Ok(()),
        Verdict::Caught(why) | Verdict::Unviable(why) | Verdict::Stale(why) => bail!(
            "the tests of {krate} fail without a mutation ({why}); output in {}",
            dir.join("stdout.txt").display()
        ),
    }
}

fn path_closure(sp: &Space) -> Result<Vec<String>> {
    let out = Command::new(&sp.cargo)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(&sp.root)
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .context("running cargo metadata")?;
    if !out.status.success() {
        bail!("cargo metadata failed");
    }
    let meta: serde_json::Value = serde_json::from_slice(&out.stdout).context("reading the output of cargo metadata")?;
    let root = sp.root.canonicalize().with_context(|| format!("resolving {}", sp.root.display()))?;
    let mut deps: std::collections::BTreeMap<PathBuf, Vec<PathBuf>> = Default::default();
    for p in meta["packages"].as_array().into_iter().flatten() {
        let dir = p["manifest_path"].as_str().map(Path::new).and_then(Path::parent).map(Path::to_path_buf).unwrap_or_default();
        let paths = p["dependencies"].as_array().into_iter().flatten().filter_map(|d| d["path"].as_str()).map(PathBuf::from).collect();
        deps.insert(dir.canonicalize().unwrap_or(dir), paths);
    }
    let mut found: Vec<PathBuf> = vec![];
    let mut todo: Vec<PathBuf> = vec![];
    for c in sp.crates {
        let dir = root.join(c).canonicalize().with_context(|| format!("no crate {c}"))?;
        todo.push(dir);
    }
    while let Some(dir) = todo.pop() {
        if found.contains(&dir) {
            continue;
        }
        let Some(next) = deps.get(&dir) else {
            bail!("{} is not a package of the workspace", dir.display());
        };
        todo.extend(next.iter().map(|d| d.canonicalize().unwrap_or_else(|_| d.clone())));
        found.push(dir);
    }
    let rel = |d: &PathBuf| -> Result<String> {
        let r = d.strip_prefix(&root).with_context(|| format!("{} lies outside {}", d.display(), root.display()))?;
        Ok(r.to_string_lossy().into_owned())
    };
    let mut extra: Vec<String> = found.iter().map(rel).collect::<Result<_>>()?;
    extra.retain(|c| !sp.crates.contains(&c.as_str()));
    extra.sort();
    Ok(sp.crates.iter().map(|c| c.to_string()).chain(extra).collect())
}

fn scratch_manifest(root: &str, members: &[String]) -> Result<String> {
    let list: Vec<String> = members.iter().map(|c| format!("\"{c}\"")).collect();
    let mut lines: Vec<String> = vec!["# Scratch copy for `cargo xtask mutants`; rewritten on every run.".into()];
    let mut replaced = false;
    for l in root.lines() {
        if l.trim_start().starts_with("members") && !replaced {
            replaced = true;
            lines.push(format!("members = [{}]", list.join(", ")));
        } else {
            lines.push(l.to_string());
        }
    }
    if !replaced {
        bail!("no workspace members in the root Cargo.toml");
    }
    lines.push(String::new());
    Ok(lines.join("\n"))
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
    let deadline = timeout.map(|t| Instant::now() + t);
    let (status, timed_out) = loop {
        if let Some(status) = child.try_wait()? {
            break (Some(status), false);
        }
        if deadline.is_some_and(|d| Instant::now().checked_duration_since(d).is_some()) {
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

fn mirror_workspace(sp: &Space, crates: &[String], ws: &Path) -> Result<()> {
    let root = &sp.root;
    fs::create_dir_all(ws)?;
    let manifest = scratch_manifest(&fs::read_to_string(root.join("Cargo.toml"))?, crates)?;
    put(&ws.join("Cargo.toml"), manifest.as_bytes())?;
    for f in ["Cargo.lock", "rust-toolchain.toml"] {
        let src = root.join(f);
        if src.exists() {
            put(&ws.join(f), &fs::read(&src)?)?;
        }
    }
    for c in crates {
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
