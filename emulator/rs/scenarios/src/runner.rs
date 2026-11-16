//! The image x scenario runner behind `cargo test -p scenarios` (a
//! `harness = false` test, no external crates): runs every selected test on
//! every image, one joined bulb per (image, session), and reports a verdict
//! per test as emulator/spec_suite.py does.
//!
//! Arguments (after `--`): substring filters on `label::TEST-ID` (`a|b` = a or
//! b), `--list`, `--exact`, `--image label=path[@0xOFFSET]` (repeatable),
//! `--json out.json` (the verdicts), `--meta-json tests.json` (what each test checks),
//! `--cpi N` (3 = full fidelity), `--jobs N` / `--test-threads N`, `-v`,
//! `--shard K/N` (the scenarios only: the K-th of N parts, for parallel CI jobs).
//! Environment: SCENARIO_IMAGES=label=path,... SCENARIO_CPI, SCENARIO_JOBS (or RUST_TEST_THREADS), NO_COLOR.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Instant;

use crate::air::Log;
use crate::image::Image;
use crate::session::{Session, SessionOpts, Stop};
use crate::spec::{self, Test};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Fail,
    Inconclusive,
    NotApplicable,
    Error,
    /// a "should" clause (or one whose failure depends on its reading) not followed
    Note,
}

impl Verdict {
    pub fn name(self) -> &'static str {
        match self {
            Verdict::Pass => "PASS",
            Verdict::Fail => "FAIL",
            Verdict::Inconclusive => "INCONCLUSIVE",
            Verdict::NotApplicable => "N/A",
            Verdict::Error => "ERROR",
            Verdict::Note => "NOTE",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Outcome {
    pub image: String,
    pub id: &'static str,
    pub verdict: Verdict,
    pub obs: String,
    pub sim_s: f64,
}

pub struct Args {
    pub filters: Vec<String>,
    pub exact: bool,
    pub list: bool,
    pub images: Vec<Image>,
    pub cpi: i64,
    pub jobs: usize,
    pub verbose: u8,
    /// write the outcomes as JSON here
    pub json: Option<PathBuf>,
    /// write what every test checks (module, document, clause, requirement) as JSON here
    pub meta_json: Option<PathBuf>,
    /// the conformance report over stock, C and Rust instead of the red/green run
    pub matrix: bool,
    /// libtest's --skip: names containing one of these are left out
    pub skip: Vec<String>,
    /// colour the verdicts (libtest's --color auto|always|never, NO_COLOR)
    pub color: bool,
    /// run only part K (1-based) of N of the selected scenarios
    pub shard: Option<(usize, usize)>,
}

/// The repository root, from this crate's place in it (emulator/rs/scenarios).
pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// The images of `--matrix`: the stock firmware and the Rust firmware.
pub fn default_images() -> Vec<Image> {
    let r = repo_root();
    [("stock", "stock/stock-dimm.bin"), ("rust", "target/tc32/bulb-tc32.bin")]
        .iter()
        .map(|(l, p)| Image { label: l.to_string(), path: if p.starts_with("stock/") { crate::sims::image_path(p) } else { r.join(p) }, link_shift: None })
        .collect()
}

/// `K/N` with 1 <= K <= N.
fn parse_shard(v: &str) -> Result<(usize, usize), String> {
    let (k, n) = v.split_once('/').ok_or(format!("--shard {v}: expected K/N"))?;
    let k: usize = k.parse().map_err(|e| format!("--shard {v}: {e}"))?;
    let n: usize = n.parse().map_err(|e| format!("--shard {v}: {e}"))?;
    if k == 0 || k > n {
        return Err(format!("--shard {v}: K must be 1..=N"));
    }
    Ok((k, n))
}

pub fn parse_args(argv: &[String]) -> Result<Args, String> {
    parse_args_with(argv, default_images)
}

/// `parse_args` with the images of `--matrix`; without `--matrix` and
/// without images the run is the Rust image alone (the last of them).
pub fn parse_args_with(argv: &[String], defaults: fn() -> Vec<Image>) -> Result<Args, String> {
    let mut a = Args {
        filters: Vec::new(),
        exact: false,
        list: false,
        images: Vec::new(),
        cpi: 3,
        // libtest's default: one session per CPU, or RUST_TEST_THREADS
        jobs: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1),
        verbose: 0,
        json: None,
        meta_json: None,
        matrix: false,
        skip: Vec::new(),
        color: std::io::IsTerminal::is_terminal(&std::io::stdout()) && std::env::var_os("NO_COLOR").is_none(),
        shard: None,
    };
    if let Ok(v) = std::env::var("RUST_TEST_THREADS") {
        a.jobs = v.parse::<usize>().map_err(|e| format!("RUST_TEST_THREADS: {e}"))?.max(1);
    }
    if let Ok(v) = std::env::var("SCENARIO_CPI") {
        a.cpi = v.parse().map_err(|e| format!("SCENARIO_CPI: {e}"))?;
    }
    if let Ok(v) = std::env::var("SCENARIO_JOBS") {
        a.jobs = v.parse().map_err(|e| format!("SCENARIO_JOBS: {e}"))?;
    }
    if let Ok(v) = std::env::var("SCENARIO_IMAGES") {
        for spec in v.split(',').filter(|s| !s.is_empty()) {
            a.images.push(Image::parse(spec)?);
        }
    }
    let mut it = argv.iter();
    let num = |v: Option<&String>, what: &str| -> Result<i64, String> {
        v.ok_or(format!("{what} needs a value"))?.parse().map_err(|e| format!("{what}: {e}"))
    };
    let mut cli_images = Vec::new();
    while let Some(x) = it.next() {
        match x.as_str() {
            "--list" => a.list = true,
            "--exact" => a.exact = true,
            "-v" | "--verbose" => a.verbose += 1,
            "--cpi" => a.cpi = num(it.next(), "--cpi")?,
            "--jobs" | "--test-threads" => a.jobs = num(it.next(), x)?.max(1) as usize,
            "--json" => a.json = Some(PathBuf::from(it.next().ok_or("--json needs a value")?)),
            "--meta-json" => a.meta_json = Some(PathBuf::from(it.next().ok_or("--meta-json needs a value")?)),
            "--matrix" => a.matrix = true,
            "--skip" => a.skip.push(it.next().ok_or("--skip needs a value")?.clone()),
            "--shard" => a.shard = Some(parse_shard(it.next().ok_or("--shard needs K/N")?)?),
            "--image" => cli_images.push(Image::parse(it.next().ok_or("--image needs a value")?)?),
            // libtest flags cargo/nextest may pass: accepted, no effect here
            "--nocapture" | "--show-output" | "-q" | "--quiet" | "--include-ignored" | "--ignored" => {}
            "--color" => match it.next().map(|s| s.as_str()) {
                Some("always") => a.color = true,
                Some("never") => a.color = false,
                Some("auto") => {}
                v => return Err(format!("--color: {v:?} (auto, always or never)")),
            },
            "--format" | "-Z" | "--logfile" => {
                it.next();
            }
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            s => a.filters.extend(s.split('|').map(|f| f.to_string())),
        }
    }
    if !cli_images.is_empty() {
        a.images = cli_images;
    }
    if a.images.is_empty() {
        a.images = defaults();
        if !a.matrix && !a.images.is_empty() {
            a.images = a.images.split_off(a.images.len() - 1);
        }
    }
    Ok(a)
}

pub fn selected(a: &Args, name: &str) -> bool {
    (a.filters.is_empty() || a.filters.iter().any(|f| if a.exact { name == f } else { name.contains(f.as_str()) }))
        && !a.skip.iter().any(|f| name.contains(f.as_str()))
}

/// One (image, session): the tests run in order on one joined bulb (`spec_suite.run_group`).
fn run_group(image: &Image, session: &str, tests: &[&Test], cpi: i64, verbose: u8, out: &mpsc::Sender<Outcome>) {
    let def = tests[0].setup;
    let log: Option<Log> = if verbose > 1 {
        let label = image.label.clone();
        Some(std::rc::Rc::new(move |s: &str| println!("      [{label}] {s}")))
    } else {
        None
    };
    let send = |id: &'static str, verdict: Verdict, obs: String, sim_s: f64| {
        if verbose > 0 {
            println!("   [{} {session}] {:12} {id}  {obs}", image.label, verdict.name());
        }
        let _ = out.send(Outcome { image: image.label.clone(), id, verdict, obs, sim_s });
    };
    let mut s = match Session::new(image, SessionOpts { cpi, join: def.join, log, tc: def.tc, ..Default::default() }) {
        Ok(mut s) => {
            // a broadcast lost to the bulb's BLE event is sent again (`World::ble_retry`)
            s.w.dut_rx_log();
            s.w.ble_retry = true;
            s
        }
        Err(e) => {
            let (v, why) = match e {
                Stop::Error(m) => (Verdict::Error, m),
                Stop::SessionLost(m) | Stop::Fail(m) | Stop::Inconclusive(m) | Stop::NotApplicable(m) => {
                    (Verdict::Inconclusive, format!("session could not start: {m}"))
                }
            };
            for t in tests {
                send(t.id, v, why.clone(), 0.0);
            }
            return;
        }
    };
    let shared = tests.len() > 1;
    let mut dead: Option<String> = None;
    for t in tests {
        if let Some(d) = &dead {
            send(t.id, Verdict::Inconclusive, format!("not run: {d}"), 0.0);
            continue;
        }
        let sim0 = s.now();
        let r = (|| {
            if shared && s.joined && def.baseline {
                s.baseline()?;
            }
            (t.run)(&mut s)
        })();
        let sim_s = ((s.now() - sim0) / 100.0).round() / 10.0;
        match r {
            Ok(obs) => send(t.id, Verdict::Pass, obs, sim_s),
            Err(Stop::Fail(m)) => send(t.id, Verdict::Fail, m, sim_s),
            Err(Stop::Inconclusive(m)) => send(t.id, Verdict::Inconclusive, m, sim_s),
            Err(Stop::NotApplicable(m)) => send(t.id, Verdict::NotApplicable, m, sim_s),
            Err(Stop::SessionLost(m)) => {
                send(t.id, Verdict::Inconclusive, m.clone(), sim_s);
                dead = Some(format!("session lost in {}: {m}", t.id));
                continue;
            }
            Err(Stop::Error(m)) => {
                send(t.id, Verdict::Error, m, sim_s);
                dead = Some(format!("the emulator node stopped in {}", t.id));
                continue;
            }
        }
        if shared && s.joined && def.alive_check {
            match s.alive() {
                Ok(true) => {}
                Ok(false) => dead = Some(format!("the bulb stopped answering after {}", t.id)),
                Err(e) => dead = Some(format!("the emulator node stopped after {}: {e:?}", t.id)),
            }
        }
    }
}

/// Runs the selected tests; `on` sees every outcome as it comes; returns them in (image, test) order.
pub fn run(a: &Args, on: &mut dyn FnMut(&Outcome)) -> Vec<Outcome> {
    let all = spec::tests();
    // (image, session, tests of that session in order)
    let mut groups: Vec<(Image, String, Vec<&Test>)> = Vec::new();
    for img in &a.images {
        let mut by_session: Vec<(String, Vec<&Test>)> = Vec::new();
        for t in &all {
            if !selected(a, &format!("{}::{}", img.label, t.id)) {
                continue;
            }
            let g = t.group();
            match by_session.iter_mut().find(|(s, _)| *s == g) {
                Some((_, v)) => v.push(t),
                None => by_session.push((g, vec![t])),
            }
        }
        for (s, v) in by_session {
            groups.push((img.clone(), s, v));
        }
    }
    // long sessions first, as spec_suite does
    groups.sort_by_key(|g| std::cmp::Reverse(g.2.len()));
    let (tx, rx) = mpsc::channel();
    let queue = Arc::new(Mutex::new(groups.into_iter().collect::<VecDeque<_>>()));
    let out = std::thread::scope(|sc| {
        for _ in 0..a.jobs.max(1) {
            let (q, tx) = (queue.clone(), tx.clone());
            sc.spawn(move || loop {
                let Some((img, session, tests)) = q.lock().unwrap().pop_front() else { break };
                if !img.path.exists() {
                    for t in &tests {
                        let _ = tx.send(Outcome {
                            image: img.label.clone(),
                            id: t.id,
                            verdict: Verdict::Error,
                            obs: format!("no image at {}", img.path.display()),
                            sim_s: 0.0,
                        });
                    }
                    continue;
                }
                run_group(&img, &session, &tests, a.cpi, a.verbose, &tx);
            });
        }
        drop(tx);
        rx.iter().map(|o| {
            on(&o);
            o
        }).collect::<Vec<_>>()
    });
    let mut out = out;
    let img_pos = |l: &str| a.images.iter().position(|i| i.label == l).unwrap_or(usize::MAX);
    let test_pos = |id: &str| all.iter().position(|t| t.id == id).unwrap_or(usize::MAX);
    out.sort_by_key(|o| (img_pos(&o.image), test_pos(o.id)));
    out
}

/// The test binary's main: list or run, print, exit code.
pub fn main_with(argv: &[String]) -> i32 {
    let a = match parse_args(argv) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("scenarios: {e}");
            return 2;
        }
    };
    let meta: Vec<TestMeta> = spec::modules()
        .into_iter()
        .flat_map(|(module, v)| {
            v.into_iter().map(move |t| TestMeta {
                id: t.id,
                module,
                doc: t.doc,
                clause: t.clause,
                req: t.req,
                mandatory: t.mandatory,
                level: "shall",
                ambiguous: t.ambiguous,
            })
        })
        .collect();
    drive(&a, &meta, &mut |on| run(&a, on))
}

/// What the reports need of a test.
pub struct TestMeta {
    pub id: &'static str,
    /// the module the test is in (its area)
    pub module: &'static str,
    pub doc: &'static str,
    pub clause: &'static str,
    pub req: &'static str,
    pub mandatory: bool,
    /// "shall", "should" (a failure is a NOTE) or "reading" (it depends on how the clause is read: NOTE too)
    pub level: &'static str,
    pub ambiguous: Option<&'static str>,
}

/// How a verdict counts in the red/green run: PASS is `ok`, FAIL and ERROR are
/// `FAILED`, the rest (the clause could not be judged on this image) `ignored`.
/// The status word in libtest's colours: ok green, FAILED red, ignored yellow.
fn paint(st: &str, color: bool) -> String {
    if !color {
        return st.to_string();
    }
    let c = match st {
        "ok" => "32",
        "FAILED" => "31",
        _ => "33",
    };
    format!("\x1b[{c}m{st}\x1b[0m")
}

pub fn libtest_status(v: Verdict) -> &'static str {
    match v {
        Verdict::Pass => "ok",
        Verdict::Fail | Verdict::Error => "FAILED",
        _ => "ignored",
    }
}

/// List, run and report, shared by the Zigbee and the BLE test binaries.
/// Without --matrix: libtest's format, exit 1 on any FAIL or ERROR. With
/// --matrix: the conformance report over all images (FAILs of stock and C
/// are findings, not a broken run: exit 1 only on ERROR).
pub fn drive(a: &Args, meta: &[TestMeta], run: &mut dyn FnMut(&mut dyn FnMut(&Outcome)) -> Vec<Outcome>) -> i32 {
    let names: Vec<String> =
        a.images.iter().flat_map(|img| meta.iter().map(move |t| format!("{}::{}", img.label, t.id))).collect();
    if let Some(p) = &a.meta_json {
        if let Err(e) = std::fs::write(p, meta_json(meta)) {
            eprintln!("scenarios: {}: {e}", p.display());
            return 2;
        }
    }
    if a.list {
        for n in names.iter().filter(|n| selected(a, n)) {
            println!("{n}: test");
        }
        return 0;
    }
    let n_sel = names.iter().filter(|n| selected(a, n)).count();
    let labels: Vec<&str> = a.images.iter().map(|i| i.label.as_str()).collect();
    let t0 = Instant::now();
    let find = |id: &str| meta.iter().find(|t| t.id == id).unwrap();
    let out = if a.matrix {
        println!("== images: {}; cpi {}; {} parallel", labels.join(", "), a.cpi, a.jobs);
        let out = run(&mut |_| {});
        for o in &out {
            let t = find(o.id);
            println!("{:12} {:6} {:28} {} {}: {}", o.verdict.name(), o.image, o.id, t.doc, t.clause, t.req);
            if !o.obs.is_empty() && (o.verdict != Verdict::Pass || a.verbose > 0) {
                println!("{:12} -> {}", "", o.obs);
            }
        }
        out
    } else {
        println!("\nrunning {n_sel} test{} (up to {} session{} in parallel)", if n_sel == 1 { "" } else { "s" }, a.jobs,
                 if a.jobs == 1 { "" } else { "s" });
        run(&mut |o| {
            let st = libtest_status(o.verdict);
            let name = format!("{}::{}", o.image, o.id);
            let p = paint(st, a.color);
            match st {
                "ignored" => println!("test {name} ... {p}, {}: {}", o.verdict.name(), o.obs),
                _ if a.verbose > 0 && !o.obs.is_empty() => println!("test {name} ... {p} ({})", o.obs),
                _ => println!("test {name} ... {p}"),
            }
        })
    };
    let count = |os: &[&Outcome], v: Verdict| os.iter().filter(|o| o.verdict == v).count();
    if a.matrix {
        for l in &labels {
            let mine: Vec<&Outcome> = out.iter().filter(|o| o.image == *l).collect();
            if mine.is_empty() {
                continue;
            }
            let parts: Vec<String> = [Verdict::Pass, Verdict::Fail, Verdict::Note, Verdict::Inconclusive, Verdict::NotApplicable, Verdict::Error]
                .iter()
                .filter(|v| count(&mine, **v) > 0)
                .map(|v| format!("{} {}", v.name(), count(&mine, *v)))
                .collect();
            println!("== {l}: {}; {} tests", parts.join(", "), mine.len());
        }
        println!("== wall time {:.0} s", t0.elapsed().as_secs_f64());
    }
    if let Some(p) = &a.json {
        if let Err(e) = std::fs::write(p, outcomes_json(&labels, &out)) {
            eprintln!("scenarios: {}: {e}", p.display());
            return 2;
        }
    }
    let all: Vec<&Outcome> = out.iter().collect();
    let (fail, err) = (count(&all, Verdict::Fail), count(&all, Verdict::Error));
    if a.matrix {
        return if err > 0 { 1 } else { 0 };
    }
    let failed: Vec<&&Outcome> = all.iter().filter(|o| libtest_status(o.verdict) == "FAILED").collect();
    if !failed.is_empty() {
        println!("\nfailures:\n");
        for o in &failed {
            let t = find(o.id);
            println!("---- {}::{} ----", o.image, o.id);
            println!("{}: {} {}: {}", o.verdict.name(), t.doc, t.clause, t.req);
            println!("observed: {}\n", o.obs);
        }
        println!("failures:");
        for o in &failed {
            println!("    {}::{}", o.image, o.id);
        }
    }
    let passed = count(&all, Verdict::Pass);
    let ignored = all.len() - passed - fail - err;
    println!(
        "\ntest result: {}. {passed} passed; {} failed; {ignored} ignored; 0 measured; {} filtered out; finished in {:.2}s\n",
        paint(if failed.is_empty() { "ok" } else { "FAILED" }, a.color),
        fail + err,
        names.len() - n_sel,
        t0.elapsed().as_secs_f64()
    );
    if failed.is_empty() {
        0
    } else {
        1
    }
}

pub fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// `{"label": {"TEST-ID": {"verdict": ..., "obs": ..., "sim_s": ...}}}`, the shape of
/// spec_suite.py --json `results`.
pub fn outcomes_json(labels: &[&str], out: &[Outcome]) -> String {
    let mut s = String::from("{\n");
    for (k, l) in labels.iter().enumerate() {
        s.push_str(&format!(" {}: {{\n", json_str(l)));
        let mine: Vec<&Outcome> = out.iter().filter(|o| o.image == *l).collect();
        for (j, o) in mine.iter().enumerate() {
            s.push_str(&format!(
                "  {}: {{\"verdict\": {}, \"obs\": {}, \"sim_s\": {}}}{}\n",
                json_str(o.id),
                json_str(o.verdict.name()),
                json_str(&o.obs),
                o.sim_s,
                if j + 1 < mine.len() { "," } else { "" }
            ));
        }
        s.push_str(&format!(" }}{}\n", if k + 1 < labels.len() { "," } else { "" }));
    }
    s.push_str("}\n");
    s
}

/// `[{"id", "module", "doc", "clause", "req", "mandatory", "level", "ambiguous"}]`: what each test checks.
pub fn meta_json(meta: &[TestMeta]) -> String {
    let mut s = String::from("[\n");
    for (k, t) in meta.iter().enumerate() {
        s.push_str(&format!(
            " {{\"id\": {}, \"module\": {}, \"doc\": {}, \"clause\": {}, \"req\": {}, \"mandatory\": {}, \"level\": {}, \"ambiguous\": {}}}{}\n",
            json_str(t.id),
            json_str(t.module),
            json_str(t.doc),
            json_str(t.clause),
            json_str(t.req),
            t.mandatory,
            json_str(t.level),
            t.ambiguous.map(json_str).unwrap_or_else(|| "null".into()),
            if k + 1 < meta.len() { "," } else { "" }
        ));
    }
    s.push_str("]\n");
    s
}
