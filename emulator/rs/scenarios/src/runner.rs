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
    pub json: Option<PathBuf>,
    pub meta_json: Option<PathBuf>,
    pub matrix: bool,
    pub skip: Vec<String>,
    pub color: bool,
    pub shard: Option<(usize, usize)>,
    pub expect: Option<String>,
    pub no_expect: bool,
    pub write_expect: bool,
    pub default_images: bool,
}

pub const NOT_STARTED: &str = "session could not start: ";
pub const NOT_RUN: &str = "not run: ";

pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

pub fn default_images() -> Vec<Image> {
    let r = repo_root();
    [("stock", "stock/stock-dimm.bin"), ("rust", "target/tc32/bulb-tc32.bin")]
        .iter()
        .map(|(l, p)| Image { label: l.to_string(), path: if p.starts_with("stock/") { crate::sims::image_path(p) } else { r.join(p) }, link_shift: None })
        .collect()
}

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

pub fn parse_args_with(argv: &[String], defaults: fn() -> Vec<Image>) -> Result<Args, String> {
    let mut a = Args {
        filters: Vec::new(),
        exact: false,
        list: false,
        images: Vec::new(),
        cpi: 3,
        jobs: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1),
        verbose: 0,
        json: None,
        meta_json: None,
        matrix: false,
        skip: Vec::new(),
        color: std::io::IsTerminal::is_terminal(&std::io::stdout()) && std::env::var_os("NO_COLOR").is_none(),
        shard: None,
        expect: None,
        no_expect: false,
        write_expect: false,
        default_images: false,
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
            "--expect" => a.expect = Some(it.next().ok_or("--expect needs a name or a file")?.clone()),
            "--no-expect" => a.no_expect = true,
            "--write-expect" => a.write_expect = true,
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
    if a.no_expect && (a.expect.is_some() || a.write_expect) {
        return Err("--no-expect with --expect or --write-expect".into());
    }
    if a.images.is_empty() {
        a.default_images = true;
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
            s.set_note(spec::misc_ota_gp::IMAGE_PATH, image.path.clone());
            s.w.dut_rx_log();
            s.w.ble_retry = true;
            s
        }
        Err(e) => {
            let (v, why) = match e {
                Stop::Error(m) => (Verdict::Error, m),
                Stop::SessionLost(m) | Stop::Fail(m) | Stop::Inconclusive(m) | Stop::NotApplicable(m) => {
                    (Verdict::Inconclusive, format!("{NOT_STARTED}{m}"))
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
            send(t.id, Verdict::Inconclusive, format!("{NOT_RUN}{d}"), 0.0);
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

pub fn run(a: &Args, on: &mut dyn FnMut(&Outcome)) -> Vec<Outcome> {
    let all = spec::tests();
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

pub fn sweep_offset(j: u32) -> u32 {
    j.wrapping_mul(0x9E37_79B9) & 0x0FFF_FFF8
}

pub fn sweep_main(argv: &[String], cargo_alias: &str) -> Option<i32> {
    use std::io::BufRead;
    let i = argv.iter().position(|x| x == "--sweep")?;
    let n: u32 = match argv.get(i + 1).and_then(|v| v.parse().ok()) {
        Some(n) if n > 0 => n,
        _ => {
            eprintln!("scenarios: --sweep needs a number of runs (1 or more)");
            return Some(2);
        }
    };
    let rest: Vec<String> = argv.iter().enumerate().filter(|(k, _)| *k != i && *k != i + 1).map(|(_, x)| x.clone()).collect();
    if rest.iter().any(|x| x == "--write-expect" || x == "--json" || x == "--list") {
        eprintln!("scenarios: --sweep runs the tests, not with --write-expect, --json or --list");
        return Some(2);
    }
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("scenarios: --sweep: {e}");
            return Some(2);
        }
    };
    let t0 = Instant::now();
    let mut runs: Vec<(u32, i32, Vec<String>)> = Vec::new();
    for j in 1..=n {
        let off = sweep_offset(j);
        println!("\n==== sweep {j}/{n}: TC32EMU_TICK_OFFSET={off:#x}");
        let child = std::process::Command::new(&exe)
            .args(&rest)
            .env("TC32EMU_TICK_OFFSET", format!("{off:#x}"))
            .stdout(std::process::Stdio::piped())
            .spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(e) => {
                eprintln!("scenarios: --sweep: {e}");
                return Some(2);
            }
        };
        let mut failed = Vec::new();
        if let Some(out) = child.stdout.take() {
            for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
                println!("{line}");
                if let Some(name) = line.strip_prefix("test ").and_then(|l| l.split_once(" ... FAILED")).map(|(n, _)| n) {
                    failed.push(name.to_string());
                }
            }
        }
        let code = child.wait().ok().and_then(|s| s.code()).unwrap_or(1);
        runs.push((off, code, failed));
    }
    println!("\n==== sweep over {n} timer offsets ({:.0} s)", t0.elapsed().as_secs_f64());
    let mut bad = 0;
    for (off, code, failed) in &runs {
        if *code == 0 {
            println!("  {off:#010x}  ok");
            continue;
        }
        bad += 1;
        let what = if failed.is_empty() { format!("exit code {code}") } else { failed.join(", ") };
        println!("  {off:#010x}  FAILED: {what}");
    }
    if bad > 0 {
        let args = rest.join(" ");
        let (off, _, _) = runs.iter().find(|r| r.1 != 0).unwrap();
        println!("{bad} of {n} runs failed; to repeat one: TC32EMU_TICK_OFFSET={off:#x} cargo {cargo_alias} -- {args}");
    }
    Some(if bad > 0 { 1 } else { 0 })
}

pub fn main_with(argv: &[String]) -> i32 {
    if let Some(code) = sweep_main(argv, "spec") {
        return code;
    }
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
    drive(&a, "zigbee", &meta, &mut |on| run(&a, on))
}

pub struct TestMeta {
    pub id: &'static str,
    pub module: &'static str,
    pub doc: &'static str,
    pub clause: &'static str,
    pub req: &'static str,
    pub mandatory: bool,
    pub level: &'static str,
    pub ambiguous: Option<&'static str>,
}

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

pub fn not_run(o: &Outcome) -> bool {
    o.obs.starts_with(NOT_STARTED) || o.obs.starts_with(NOT_RUN)
}

pub fn status(o: &Outcome, expect: Option<&Expect>) -> &'static str {
    if not_run(o) {
        return "FAILED";
    }
    match expect {
        Some(e) if e.want(o.id) != o.verdict.name() => "FAILED",
        Some(_) if o.verdict == Verdict::Pass => "ok",
        Some(_) => "ignored",
        None => libtest_status(o.verdict),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExpectEntry {
    pub id: String,
    pub verdict: String,
    pub reason: String,
}

#[derive(Clone, Debug, Default)]
pub struct Expect {
    pub path: PathBuf,
    pub header: Vec<String>,
    pub entries: Vec<ExpectEntry>,
}

const EXPECT_VERDICTS: [&str; 5] = ["FAIL", "N/A", "INCONCLUSIVE", "NOTE", "ERROR"];
const TODO: &str = "TODO";

impl Expect {
    pub fn parse(path: PathBuf, text: &str) -> Result<Expect, String> {
        let mut e = Expect { path, ..Default::default() };
        let mut body = false;
        for (k, line) in text.lines().enumerate() {
            let at = format!("{}:{}", e.path.display(), k + 1);
            let l = line.trim();
            if l.is_empty() || l.starts_with('#') {
                if !body {
                    e.header.push(line.trim_end().to_string());
                }
                continue;
            }
            body = true;
            let word = |s: &str| -> (String, String) {
                let (w, rest) = s.split_once(char::is_whitespace).unwrap_or((s, ""));
                (w.to_string(), rest.trim_start().to_string())
            };
            let (id, rest) = word(l);
            let (verdict, reason) = word(&rest);
            if verdict == "PASS" {
                return Err(format!("{at}: {id} PASS: a test that passes is not listed"));
            }
            if !EXPECT_VERDICTS.contains(&verdict.as_str()) {
                return Err(format!("{at}: verdict {verdict:?}; one of {}", EXPECT_VERDICTS.join(", ")));
            }
            if reason.is_empty() {
                return Err(format!("{at}: {id} {verdict} without a reason"));
            }
            if e.entries.iter().any(|x| x.id == id) {
                return Err(format!("{at}: {id} listed twice"));
            }
            e.entries.push(ExpectEntry { id, verdict, reason });
        }
        while e.header.last().is_some_and(|l| l.is_empty()) {
            e.header.pop();
        }
        Ok(e)
    }

    pub fn load(path: PathBuf) -> Result<Expect, String> {
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        Expect::parse(path, &text)
    }

    pub fn want(&self, id: &str) -> &str {
        self.entries.iter().find(|e| e.id == id).map_or("PASS", |e| e.verdict.as_str())
    }

    pub fn entry(&self, id: &str) -> Option<&ExpectEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    pub fn problems(&self, meta: &[TestMeta]) -> Vec<String> {
        let mut v = Vec::new();
        for e in &self.entries {
            if !meta.iter().any(|t| t.id == e.id) {
                v.push(format!("{} is listed in {} but is not a test of this suite: remove its line", e.id, self.path.display()));
            }
            if e.reason.starts_with(TODO) {
                v.push(format!("{} {}: write the reason in {} (it says {TODO})", e.id, e.verdict, self.path.display()));
            }
        }
        v
    }

    pub fn updated(&self, meta: &[TestMeta], out: &[Outcome]) -> Expect {
        let mut entries = Vec::new();
        for t in meta {
            if let Some(o) = out.iter().find(|o| o.id == t.id) {
                if o.verdict == Verdict::Pass {
                    continue;
                }
                let verdict = o.verdict.name().to_string();
                let reason = match self.entry(t.id) {
                    Some(e) if e.verdict == verdict => e.reason.clone(),
                    _ => format!("{TODO}: {}", o.obs.chars().take(160).collect::<String>().replace('\n', " ")),
                };
                entries.push(ExpectEntry { id: t.id.to_string(), verdict, reason });
            } else if let Some(e) = self.entry(t.id) {
                entries.push(e.clone());
            }
        }
        Expect { path: self.path.clone(), header: self.header.clone(), entries }
    }

    pub fn text(&self) -> String {
        let mut s = String::new();
        for l in &self.header {
            s.push_str(l);
            s.push('\n');
        }
        if !self.header.is_empty() {
            s.push('\n');
        }
        let w = self.entries.iter().map(|e| e.id.len()).max().unwrap_or(0);
        for e in &self.entries {
            s.push_str(&format!("{:w$}  {:12}  {}\n", e.id, e.verdict, e.reason));
        }
        s
    }
}

pub fn expect_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("expect")
}

fn expect_path(v: &str) -> PathBuf {
    let p = PathBuf::from(v);
    if !v.contains('/') && !v.ends_with(".txt") {
        return expect_dir().join(format!("{v}.txt"));
    }
    if p.is_relative() && !p.exists() {
        return repo_root().join(p);
    }
    p
}

fn expect_of(a: &Args, suite: &str) -> Result<Option<Expect>, String> {
    if a.no_expect || a.list || (a.matrix && a.expect.is_none()) {
        return Ok(None);
    }
    let path = match &a.expect {
        Some(v) => expect_path(v),
        None if a.default_images => expect_dir().join(format!("{suite}-dimm.txt")),
        None => {
            if a.write_expect {
                return Err("--write-expect needs --expect NAME|FILE for an image given with --image".into());
            }
            return Ok(None);
        }
    };
    if a.matrix {
        return Err("--expect checks a run on one image, not --matrix".into());
    }
    if a.images.len() != 1 {
        return Err(format!("--expect checks a run on one image; {} given", a.images.len()));
    }
    if a.write_expect && !path.exists() {
        let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let header = vec![
            format!("# The expected verdicts of the {suite} conformance tests ({name}): one line per test that does"),
            "# not PASS, with why (deviation, not applicable by design, known red test); every test not".to_string(),
            "# listed must PASS. Any difference fails the run, a newly passing test too. Update: run with".to_string(),
            "# --write-expect, then write the reason of every TODO line (runner.rs, Expect).".to_string(),
        ];
        return Ok(Some(Expect { path, header, entries: Vec::new() }));
    }
    if !path.exists() && a.expect.is_none() {
        return Ok(None);
    }
    Expect::load(path).map(Some)
}

pub fn drive(a: &Args, suite: &str, meta: &[TestMeta], run: &mut dyn FnMut(&mut dyn FnMut(&Outcome)) -> Vec<Outcome>) -> i32 {
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
    let expect = match expect_of(a, suite) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("scenarios: {e}");
            return 2;
        }
    };
    let ex = if a.write_expect { None } else { expect.as_ref() };
    let st = |o: &Outcome| -> &'static str {
        match (a.write_expect, not_run(o), o.verdict) {
            (true, true, _) => "FAILED",
            (true, false, Verdict::Pass) => "ok",
            (true, false, _) => "ignored",
            _ => status(o, ex),
        }
    };
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
        let against = ex.map(|e| format!("; expected verdicts: {}", e.path.display())).unwrap_or_default();
        println!("\nrunning {n_sel} test{} (up to {} session{} in parallel{against})", if n_sel == 1 { "" } else { "s" },
                 a.jobs, if a.jobs == 1 { "" } else { "s" });
        run(&mut |o| {
            let st = st(o);
            let name = format!("{}::{}", o.image, o.id);
            let p = paint(st, a.color);
            match st {
                "ignored" => println!("test {name} ... {p}, {}: {}", o.verdict.name(), o.obs),
                "FAILED" if ex.is_some_and(|e| e.want(o.id) != o.verdict.name()) => {
                    println!("test {name} ... {p}, {} where {} is expected: {}", o.verdict.name(), ex.unwrap().want(o.id), o.obs)
                }
                _ if a.verbose > 0 && !o.obs.is_empty() => println!("test {name} ... {p} ({})", o.obs),
                _ => println!("test {name} ... {p}"),
            }
        })
    };
    let count = |os: &[&Outcome], v: Verdict| os.iter().filter(|o| o.verdict == v).count();
    let mut broken: Vec<String> = Vec::new();
    for l in &labels {
        let mine: Vec<&Outcome> = out.iter().filter(|o| o.image == *l).collect();
        if !mine.is_empty() && count(&mine, Verdict::Pass) == 0 {
            broken.push(format!("{l}: none of the {} tests passed, so the run checked nothing", mine.len()));
        }
        let lost: Vec<&&Outcome> = mine.iter().filter(|o| not_run(o)).collect();
        if !lost.is_empty() {
            broken.push(format!("{l}: {} tests did not run, as {}: {}", lost.len(), lost[0].id, lost[0].obs));
        }
    }
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
    if a.matrix {
        for b in &broken {
            println!("== broken run: {b}");
        }
        return if count(&all, Verdict::Error) > 0 || !broken.is_empty() { 1 } else { 0 };
    }
    let failed: Vec<&&Outcome> = all.iter().filter(|o| st(o) == "FAILED").collect();
    if !failed.is_empty() {
        println!("\nfailures:\n");
        for o in &failed {
            let t = find(o.id);
            println!("---- {}::{} ----", o.image, o.id);
            println!("{}: {} {}: {}", o.verdict.name(), t.doc, t.clause, t.req);
            if let Some(e) = ex {
                match e.entry(o.id) {
                    Some(x) => println!("expected: {} ({})", x.verdict, x.reason),
                    None => println!("expected: PASS"),
                }
            }
            println!("observed: {}\n", o.obs);
        }
        println!("failures:");
        for o in &failed {
            println!("    {}::{}", o.image, o.id);
        }
    }
    let mut problems = broken.clone();
    if let Some(e) = ex {
        problems.extend(e.problems(meta));
        let newly: Vec<&str> = failed.iter().filter(|o| o.verdict == Verdict::Pass && !not_run(o)).map(|o| o.id).collect();
        if !failed.is_empty() {
            problems.push(format!(
                "{} verdict{} differ{} from {}{}; if the change is meant, run again with --write-expect and give every \
                 new non-PASS line its reason",
                failed.len(),
                if failed.len() == 1 { "" } else { "s" },
                if failed.len() == 1 { "s" } else { "" },
                e.path.display(),
                if newly.is_empty() { String::new() } else { format!(" ({} now pass{}: {})", newly.len(), if newly.len() == 1 { "es" } else { "" }, newly.join(", ")) }
            ));
        }
    }
    if a.write_expect {
        if let Some(e) = &expect {
            let new = e.updated(meta, &out);
            if let Some(d) = new.path.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            if let Err(err) = std::fs::write(&new.path, new.text()) {
                eprintln!("scenarios: {}: {err}", new.path.display());
                return 2;
            }
            let todo = new.entries.iter().filter(|x| x.reason.starts_with(TODO)).count();
            println!("\nwrote {} ({} tests expected not to pass{})", new.path.display(), new.entries.len(),
                     if todo > 0 { format!("; {todo} lines say {TODO}: write their reasons") } else { String::new() });
        }
    }
    if !problems.is_empty() {
        println!();
        for p in &problems {
            println!("error: {p}");
        }
    }
    let passed = count(&all, Verdict::Pass);
    let bad = failed.len();
    let ignored = all.iter().filter(|o| st(o) == "ignored").count();
    let ok = bad == 0 && problems.is_empty();
    println!(
        "\ntest result: {}. {passed} passed; {bad} failed; {ignored} ignored; 0 measured; {} filtered out; finished in {:.2}s\n",
        paint(if ok { "ok" } else { "FAILED" }, a.color),
        names.len() - n_sel,
        t0.elapsed().as_secs_f64()
    );
    if ok {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(id: &'static str) -> TestMeta {
        TestMeta { id, module: "m", doc: "", clause: "", req: "", mandatory: true, level: "shall", ambiguous: None }
    }

    fn out(id: &'static str, verdict: Verdict, obs: &str) -> Outcome {
        Outcome { image: "rust".into(), id, verdict, obs: obs.into(), sim_s: 0.0 }
    }

    const FILE: &str = "# header\n# more\n\nA-01   N/A    optional feature absent\nB-02  FAIL  deliberate, see why\n";

    #[test]
    fn parse_and_want() {
        let e = Expect::parse("x.txt".into(), FILE).unwrap();
        assert_eq!(e.header, ["# header", "# more"]);
        assert_eq!(e.want("A-01"), "N/A");
        assert_eq!(e.entry("A-01").unwrap().reason, "optional feature absent");
        assert_eq!(e.want("B-02"), "FAIL");
        assert_eq!(e.want("C-03"), "PASS");
        assert_eq!(Expect::parse("x.txt".into(), &e.text()).unwrap().entries, e.entries);
    }

    #[test]
    fn parse_refuses_bad_lines() {
        for bad in ["A-01 PASS fine", "A-01 MAYBE why", "A-01 N/A", "A-01 N/A x\nA-01 FAIL y"] {
            assert!(Expect::parse("x.txt".into(), bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn status_against_the_baseline() {
        let e = Expect::parse("x.txt".into(), FILE).unwrap();
        let st = |o: &Outcome| status(o, Some(&e));
        assert_eq!(st(&out("A-01", Verdict::NotApplicable, "")), "ignored");
        assert_eq!(st(&out("B-02", Verdict::Fail, "")), "ignored");
        assert_eq!(st(&out("C-03", Verdict::Pass, "")), "ok");
        assert_eq!(st(&out("A-01", Verdict::Pass, "")), "FAILED");
        assert_eq!(st(&out("C-03", Verdict::Inconclusive, "")), "FAILED");
        assert_eq!(st(&out("C-03", Verdict::NotApplicable, "")), "FAILED");
        let lost = Expect::parse("x.txt".into(), "C-03 INCONCLUSIVE why").unwrap();
        assert_eq!(status(&out("C-03", Verdict::Inconclusive, &format!("{NOT_STARTED}no join")), Some(&lost)), "FAILED");
        assert_eq!(status(&out("C-03", Verdict::Inconclusive, &format!("{NOT_RUN}lost")), None), "FAILED");
        assert_eq!(status(&out("C-03", Verdict::Inconclusive, "could not provoke"), None), "ignored");
    }

    #[test]
    fn problems_and_update() {
        let e = Expect::parse("x.txt".into(), "A-01 N/A why\nGONE-01 FAIL old\nB-02 FAIL TODO: obs").unwrap();
        let m = [meta("A-01"), meta("B-02"), meta("C-03"), meta("D-04")];
        let p = e.problems(&m);
        assert_eq!(p.len(), 2, "{p:?}");
        assert!(p[0].contains("GONE-01") && p[1].contains("B-02"));
        let n = e.updated(&m, &[out("A-01", Verdict::NotApplicable, "x"), out("B-02", Verdict::Pass, ""),
                                out("C-03", Verdict::Inconclusive, "nothing seen")]);
        let got: Vec<(&str, &str, &str)> =
            n.entries.iter().map(|x| (x.id.as_str(), x.verdict.as_str(), x.reason.as_str())).collect();
        assert_eq!(got, [("A-01", "N/A", "why"), ("C-03", "INCONCLUSIVE", "TODO: nothing seen")]);
    }
}
