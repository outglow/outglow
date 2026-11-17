use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::build::Ctx;
use crate::sh::{self, Env};

#[derive(clap::Args)]
#[command(about = "Line and region coverage of the host tests, bulb-tc32's on the simulated chip among them (cargo llvm-cov, in `nix develop .#coverage`)")]
pub struct Args {
    #[arg(long, help = "Also the unit tests of the emulator's workspace (emulator/rs: tc32emu, ble, zbnode, the scenarios' library)")]
    emulator: bool,
    #[arg(long, help = "Do not run the tests: report from the profiles of the previous run")]
    no_run: bool,
    #[arg(long, help = "Fail when a crate's line or region coverage is below its baseline in xtask/coverage.tsv")]
    check: bool,
    #[arg(long, help = "Write this run's coverage to xtask/coverage.tsv as the new baseline")]
    update: bool,
    #[arg(long, default_value_t = 15, help = "How many of the worst-covered files to list")]
    worst: usize,
}

const TOLERANCE: f64 = 0.05;

const TEST_ONLY: &str = "crates/zigbee/src/stack/link_eval\\.rs$|crates/memcap/|crates/bulb-tc32/src/(sim\\.rs$|tests/)";

const GLUE: &str = "crates/outglow/src/(flash/bt|diag/mqtt)\\.rs$";

struct Workspace {
    name: &'static str,
    manifest: &'static str,
    keeps: fn(&str) -> bool,
    runs: &'static [&'static [&'static str]],
}

const HOST: Workspace = Workspace {
    name: "host",
    manifest: "Cargo.toml",
    keeps: |n| !n.starts_with("emulator/") && n != "bulb-tc32",
    runs: &[&["--workspace"]],
};

const TC32: Workspace = Workspace { name: "bulb-tc32", manifest: "crates/bulb-tc32/Cargo.toml", keeps: |n| n == "bulb-tc32", runs: &[&[]] };

const EMULATOR: Workspace = Workspace {
    name: "emulator",
    manifest: "emulator/rs/Cargo.toml",
    keeps: |n| n.starts_with("emulator/"),
    runs: &[&["-p", "tc32emu", "-p", "ble", "-p", "zbnode"], &["-p", "scenarios", "--lib"]],
};

#[derive(Deserialize)]
struct Export {
    data: Vec<Data>,
}

#[derive(Deserialize)]
struct Data {
    files: Vec<File>,
}

#[derive(Deserialize)]
struct File {
    filename: String,
    summary: Summary,
}

#[derive(Deserialize, Clone, Copy, Default)]
struct Summary {
    lines: Count,
    regions: Count,
    functions: Count,
}

#[derive(Deserialize, Clone, Copy, Default)]
struct Count {
    count: u64,
    covered: u64,
}

impl Count {
    fn add(&mut self, o: Count) {
        self.count += o.count;
        self.covered += o.covered;
    }

    fn percent(self) -> f64 {
        if self.count == 0 {
            100.0
        } else {
            100.0 * self.covered as f64 / self.count as f64
        }
    }

    fn missed(self) -> u64 {
        self.count - self.covered
    }
}

impl Summary {
    fn add(&mut self, o: Summary) {
        self.lines.add(o.lines);
        self.regions.add(o.regions);
        self.functions.add(o.functions);
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Floor {
    pub lines: f64,
    pub regions: f64,
}

pub fn run(a: Args) -> Result<()> {
    crate::build::with_real(|c| run_in(c, &a))
}

pub fn run_in(c: &mut Ctx, a: &Args) -> Result<()> {
    let root = c.root.clone();
    let out = target_dir(c.env, &root).join("coverage");
    let mut spaces = vec![&HOST, &TC32];
    if a.emulator {
        spaces.push(&EMULATOR);
    }
    let mut crates: BTreeMap<String, Summary> = BTreeMap::new();
    let mut files: Vec<(String, Summary)> = Vec::new();
    let mut html = Vec::new();
    for ws in spaces {
        let dir = out.join(ws.name);
        std::fs::create_dir_all(&dir)?;
        let manifest = root.join(ws.manifest);
        if !a.no_run {
            llvm_cov(c, &manifest, &["clean", "--workspace"])?;
            for r in ws.runs {
                let mut args = vec!["--no-report"];
                args.extend_from_slice(r);
                llvm_cov(c, &manifest, &args)?;
            }
        }
        let json = dir.join("summary.json");
        let skip = format!("{TEST_ONLY}|{GLUE}");
        let ignore = ["--ignore-filename-regex", skip.as_str()];
        llvm_cov(c, &manifest, &[&["report", "--json", "--summary-only", "--output-path", path_str(&json)?][..], &ignore].concat())?;
        llvm_cov(c, &manifest, &[&["report", "--html", "--output-dir", path_str(&dir)?][..], &ignore].concat())?;
        html.push(dir.join("html").join("index.html"));
        let export: Export = serde_json::from_slice(&std::fs::read(&json)?).with_context(|| format!("reading {}", json.display()))?;
        for f in export.data.into_iter().flat_map(|d| d.files) {
            let rel = Path::new(&f.filename).strip_prefix(&root).unwrap_or(Path::new(&f.filename)).to_string_lossy().replace('\\', "/");
            let Some(name) = crate_of(&rel).filter(|n| (ws.keeps)(n)) else {
                continue;
            };
            crates.entry(name).or_default().add(f.summary);
            files.push((rel, f.summary));
        }
    }

    let baseline_path = root.join("xtask").join("coverage.tsv");
    let baseline = match std::fs::read_to_string(&baseline_path) {
        Ok(s) => parse_baseline(&s)?,
        Err(_) => BTreeMap::new(),
    };
    let report = render(&crates, &files, &baseline, a.worst);
    print!("{report}");
    if let Some(summary) = (c.env)("GITHUB_STEP_SUMMARY") {
        use std::io::Write as _;
        let mut f = std::fs::OpenOptions::new().append(true).create(true).open(summary)?;
        f.write_all(report.as_bytes())?;
    }
    for h in &html {
        println!("HTML: {}", h.display());
    }

    if a.update {
        let mut merged = baseline.clone();
        for (name, s) in &crates {
            merged.insert(name.clone(), Floor { lines: floor2(s.lines.percent()), regions: floor2(s.regions.percent()) });
        }
        std::fs::write(&baseline_path, format_baseline(&merged))?;
        println!("baseline written to {}", baseline_path.display());
    }
    if a.check {
        let drops = drops(&crates, &baseline);
        if !drops.is_empty() {
            for d in &drops {
                eprintln!("{d}");
            }
            bail!("coverage fell below xtask/coverage.tsv (add tests; or, if the drop is intended, `cargo xtask coverage --update`)");
        }
        println!("no crate below its baseline in xtask/coverage.tsv");
    }
    Ok(())
}

fn target_dir(env: Env, root: &Path) -> PathBuf {
    env("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|| root.join("target"))
}

fn path_str(p: &Path) -> Result<&str> {
    p.to_str().with_context(|| format!("not UTF-8: {}", p.display()))
}

fn llvm_cov(c: &mut Ctx, manifest: &Path, args: &[&str]) -> Result<()> {
    let cargo = sh::var(c.env, "CARGO").unwrap_or_else(|| "cargo".into());
    let (sub, rest) = match args.first() {
        Some(&s) if s == "clean" || s == "report" => (Some(s), &args[1..]),
        _ => (None, args),
    };
    let mut cmd = Command::new(cargo);
    cmd.arg("llvm-cov");
    if let Some(s) = sub {
        cmd.arg(s);
    }
    cmd.arg("--manifest-path").arg(manifest).args(rest);
    let status = c.sh.status(&mut cmd).context("starting cargo llvm-cov (in `nix develop .#coverage`)")?;
    if !status.success() {
        bail!("cargo llvm-cov {} failed ({status}); it runs in `nix develop .#coverage`", args.join(" "));
    }
    Ok(())
}

pub fn crate_of(rel: &str) -> Option<String> {
    let parts: Vec<&str> = rel.split('/').collect();
    match parts.as_slice() {
        ["crates", name, "src", ..] => Some((*name).to_string()),
        ["xtask", "src", ..] => Some("xtask".into()),
        ["emulator", "rs", name, "src", ..] => Some(format!("emulator/{name}")),
        _ => None,
    }
}

fn floor2(x: f64) -> f64 {
    (x * 100.0).floor() / 100.0
}

pub fn parse_baseline(s: &str) -> Result<BTreeMap<String, Floor>> {
    let mut out = BTreeMap::new();
    for (i, line) in s.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("crate\t") {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        let [name, lines, regions] = cols.as_slice() else {
            bail!("xtask/coverage.tsv line {}: expected crate, lines, regions", i + 1);
        };
        out.insert(name.to_string(), Floor { lines: lines.parse()?, regions: regions.parse()? });
    }
    Ok(out)
}

pub fn format_baseline(b: &BTreeMap<String, Floor>) -> String {
    let mut s = String::from("# cargo xtask coverage --check: no crate's line or region coverage (percent) may fall below these.\n# Checked in CI (Linux); raise them with `cargo xtask coverage --update` when tests are added.\ncrate\tlines\tregions\n");
    for (name, f) in b {
        let _ = writeln!(s, "{name}\t{:.2}\t{:.2}", f.lines, f.regions);
    }
    s
}

fn drops(crates: &BTreeMap<String, Summary>, baseline: &BTreeMap<String, Floor>) -> Vec<String> {
    let mut out = Vec::new();
    for (name, floor) in baseline {
        let Some(s) = crates.get(name) else { continue };
        let (l, r) = (s.lines.percent(), s.regions.percent());
        if l + TOLERANCE < floor.lines {
            out.push(format!("{name}: lines {l:.2} % < baseline {:.2} %", floor.lines));
        }
        if r + TOLERANCE < floor.regions {
            out.push(format!("{name}: regions {r:.2} % < baseline {:.2} %", floor.regions));
        }
    }
    out
}

fn delta(now: f64, base: Option<f64>) -> String {
    match base {
        None => "new".into(),
        Some(b) if (now - b).abs() < TOLERANCE => "=".into(),
        Some(b) => format!("{:+.2}", now - b),
    }
}

fn is_emulator(name: &str) -> bool {
    name.starts_with("emulator/")
}

fn table(s: &mut String, title: &str, crates: &BTreeMap<String, Summary>, baseline: &BTreeMap<String, Floor>, emulator: bool) {
    let rows: Vec<(&String, &Summary)> = crates.iter().filter(|(n, _)| is_emulator(n) == emulator).collect();
    if rows.is_empty() {
        return;
    }
    let _ = writeln!(s, "{title}\n");
    s.push_str("| crate | lines | line % | Δ | regions | region % | Δ | functions % |\n|---|--:|--:|--:|--:|--:|--:|--:|\n");
    let mut total = Summary::default();
    for (name, c) in rows {
        total.add(*c);
        let b = baseline.get(name);
        let _ = writeln!(
            s,
            "| {name} | {}/{} | {:.2} | {} | {}/{} | {:.2} | {} | {:.2} |",
            c.lines.covered,
            c.lines.count,
            c.lines.percent(),
            delta(c.lines.percent(), b.map(|b| b.lines)),
            c.regions.covered,
            c.regions.count,
            c.regions.percent(),
            delta(c.regions.percent(), b.map(|b| b.regions)),
            c.functions.percent()
        );
    }
    let _ = writeln!(
        s,
        "| **total** | {}/{} | {:.2} | | {}/{} | {:.2} | | {:.2} |\n",
        total.lines.covered,
        total.lines.count,
        total.lines.percent(),
        total.regions.covered,
        total.regions.count,
        total.regions.percent(),
        total.functions.percent()
    );
}

fn render(crates: &BTreeMap<String, Summary>, files: &[(String, Summary)], baseline: &BTreeMap<String, Floor>, worst: usize) -> String {
    let mut s = String::new();
    table(&mut s, "## Coverage of the host tests", crates, baseline, false);
    table(&mut s, "### The emulator's unit tests (its scenarios run whole firmware images, measured apart)", crates, baseline, true);
    s.push_str("Δ: percentage points against the baseline in `xtask/coverage.tsv`.\n\n");
    let mut by_missed: Vec<&(String, Summary)> =
        files.iter().filter(|(n, f)| f.lines.missed() > 0 && !n.starts_with("emulator/")).collect();
    by_missed.sort_by(|a, b| b.1.lines.missed().cmp(&a.1.lines.missed()).then(a.0.cmp(&b.0)));
    if worst > 0 && !by_missed.is_empty() {
        let _ = writeln!(s, "### The {} files with the most uncovered lines\n", worst.min(by_missed.len()));
        s.push_str("| file | uncovered lines | line % | region % |\n|---|--:|--:|--:|\n");
        for (name, f) in by_missed.into_iter().take(worst) {
            let _ = writeln!(s, "| `{name}` | {} | {:.2} | {:.2} |", f.lines.missed(), f.lines.percent(), f.regions.percent());
        }
        s.push('\n');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(lines: (u64, u64), regions: (u64, u64)) -> Summary {
        Summary {
            lines: Count { count: lines.0, covered: lines.1 },
            regions: Count { count: regions.0, covered: regions.1 },
            functions: Count::default(),
        }
    }

    #[test]
    fn files_map_to_their_crates() {
        assert_eq!(crate_of("crates/zigbee/src/stack/net.rs").as_deref(), Some("zigbee"));
        assert_eq!(crate_of("xtask/src/main.rs").as_deref(), Some("xtask"));
        assert_eq!(crate_of("emulator/rs/tc32emu/src/cpu.rs").as_deref(), Some("emulator/tc32emu"));
        assert_eq!(crate_of("crates/zigbee/tests/sim/mod.rs"), None);
        assert!((TC32.keeps)("bulb-tc32") && !(TC32.keeps)("zigbee"));
        assert!(!(HOST.keeps)("bulb-tc32") && (HOST.keeps)("zigbee") && !(HOST.keeps)("emulator/ble"));
        assert!((EMULATOR.keeps)("emulator/ble") && !(EMULATOR.keeps)("ble"));
        assert_eq!(crate_of("/home/u/.cargo/registry/x/src/lib.rs"), None);
    }

    #[test]
    fn the_baseline_reads_back_what_it_wrote() {
        let mut b = BTreeMap::new();
        b.insert("ble".to_string(), Floor { lines: 91.5, regions: 88.25 });
        b.insert("zigbee".to_string(), Floor { lines: 87.0, regions: 85.13 });
        assert_eq!(parse_baseline(&format_baseline(&b)).unwrap(), b);
        assert!(parse_baseline("crate\tlines\tregions\nble\t1\n").is_err());
    }

    #[test]
    fn only_a_drop_beyond_the_tolerance_fails() {
        let mut base = BTreeMap::new();
        base.insert("ble".to_string(), Floor { lines: 90.0, regions: 80.0 });
        base.insert("gone".to_string(), Floor { lines: 90.0, regions: 80.0 });
        let mut now = BTreeMap::new();
        now.insert("ble".to_string(), summary((10_000, 8_996), (10_000, 8_000)));
        assert!(drops(&now, &base).is_empty());
        now.insert("ble".to_string(), summary((10_000, 8_990), (10_000, 7_990)));
        assert_eq!(drops(&now, &base).len(), 2);
    }

    #[test]
    fn an_updated_baseline_passes_its_own_run() {
        let s = summary((3, 2), (7, 5));
        let mut now = BTreeMap::new();
        now.insert("x".to_string(), s);
        let mut base = BTreeMap::new();
        base.insert("x".to_string(), Floor { lines: floor2(s.lines.percent()), regions: floor2(s.regions.percent()) });
        assert!(drops(&now, &base).is_empty());
        assert!(base["x"].lines <= s.lines.percent());
    }

    #[test]
    fn the_emulator_has_a_table_of_its_own_and_no_worst_files() {
        let mut crates = BTreeMap::new();
        crates.insert("a".to_string(), summary((10, 5), (10, 5)));
        crates.insert("emulator/e".to_string(), summary((100, 0), (100, 0)));
        let files = vec![("emulator/rs/e/src/x.rs".to_string(), summary((100, 0), (100, 0)))];
        let r = render(&crates, &files, &BTreeMap::new(), 5);
        assert!(r.contains("| **total** | 5/10 | 50.00 |"));
        assert!(r.contains("| **total** | 0/100 | 0.00 |"));
        assert!(!r.contains("x.rs"));
        assert!(r.find("| a |").unwrap() < r.find("| emulator/e |").unwrap());
    }

    use crate::sh::fake::{env, Fake, Tmp};

    fn export(root: &Path, files: &[(&str, (u64, u64))]) -> String {
        let files: Vec<serde_json::Value> = files
            .iter()
            .map(|(f, (n, c))| {
                let name = if f.starts_with('/') { f.to_string() } else { root.join(f).display().to_string() };
                let count = serde_json::json!({"count": n, "covered": c});
                serde_json::json!({"filename": name, "summary": {"lines": count, "regions": count, "functions": count}})
            })
            .collect();
        serde_json::json!({"data": [{"files": files}]}).to_string()
    }

    fn flags(extra: &[&str]) -> Args {
        use clap::Parser;
        #[derive(Parser)]
        struct Cli {
            #[command(flatten)]
            a: Args,
        }
        let mut v = vec!["coverage"];
        v.extend_from_slice(extra);
        Cli::try_parse_from(v).unwrap().a
    }

    fn cover(t: &Tmp, vars: &[(&str, &str)], extra: &[&str], fail: &str) -> (Result<()>, Vec<String>) {
        let root = t.0.clone();
        let e = env(vars);
        let mut fake = Fake::new(|c: &Command| {
            let args: Vec<String> = c.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
            if !fail.is_empty() && args.join(" ").contains(fail) {
                return (1, String::new());
            }
            if let Some(i) = args.iter().position(|a| a == "--output-path") {
                let ws = &args[3];
                let files: Vec<(&str, (u64, u64))> = if ws.ends_with("crates/bulb-tc32/Cargo.toml") {
                    vec![("crates/bulb-tc32/src/radio.rs", (10, 5)), ("crates/zigbee/src/x.rs", (10, 0))]
                } else if ws.ends_with("emulator/rs/Cargo.toml") {
                    vec![("emulator/rs/tc32emu/src/cpu.rs", (4, 4))]
                } else {
                    vec![("crates/zigbee/src/a.rs", (100, 90)), ("xtask/src/main.rs", (10, 1)), ("/elsewhere/lib.rs", (5, 5)), ("crates/zigbee/tests/x.rs", (1, 0))]
                };
                std::fs::write(&args[i + 1], export(&root, &files)).unwrap();
            }
            (0, String::new())
        });
        let mut c = Ctx { root: t.0.clone(), sh: &mut fake, env: &e, exe: "/x".into(), toolchain: "/t".into() };
        let r = run_in(&mut c, &flags(extra));
        let log = fake.log.iter().map(|l| l.replace(&t.0.display().to_string(), "ROOT")).collect();
        (r, log)
    }

    #[test]
    fn a_run_cleans_tests_and_reports_each_workspace() {
        let t = Tmp::new("cov");
        let (r, log) = cover(&t, &[], &[], "");
        r.unwrap();
        let skip = format!("{TEST_ONLY}|{GLUE}");
        assert_eq!(
            log,
            [
                "cargo llvm-cov clean --manifest-path ROOT/Cargo.toml --workspace".to_string(),
                "cargo llvm-cov --manifest-path ROOT/Cargo.toml --no-report --workspace".into(),
                format!("cargo llvm-cov report --manifest-path ROOT/Cargo.toml --json --summary-only --output-path ROOT/target/coverage/host/summary.json --ignore-filename-regex {skip}"),
                format!("cargo llvm-cov report --manifest-path ROOT/Cargo.toml --html --output-dir ROOT/target/coverage/host --ignore-filename-regex {skip}"),
                "cargo llvm-cov clean --manifest-path ROOT/crates/bulb-tc32/Cargo.toml --workspace".into(),
                "cargo llvm-cov --manifest-path ROOT/crates/bulb-tc32/Cargo.toml --no-report".into(),
                format!("cargo llvm-cov report --manifest-path ROOT/crates/bulb-tc32/Cargo.toml --json --summary-only --output-path ROOT/target/coverage/bulb-tc32/summary.json --ignore-filename-regex {skip}"),
                format!("cargo llvm-cov report --manifest-path ROOT/crates/bulb-tc32/Cargo.toml --html --output-dir ROOT/target/coverage/bulb-tc32 --ignore-filename-regex {skip}"),
            ]
        );
        assert!(!t.path("xtask/coverage.tsv").exists());
    }

    #[test]
    fn the_emulator_and_a_step_summary() {
        let t = Tmp::new("cov-emu");
        let summary = t.path("summary.md");
        let target = t.path("tgt");
        let vars = [("GITHUB_STEP_SUMMARY", summary.to_str().unwrap()), ("CARGO", "/bin/cargo"), ("CARGO_TARGET_DIR", target.to_str().unwrap())];
        let (r, log) = cover(&t, &vars, &["--emulator", "--no-run"], "");
        r.unwrap();
        assert_eq!(log.len(), 6);
        assert!(log.iter().all(|l| l.starts_with("/bin/cargo llvm-cov report ")));
        assert!(log[4].contains("--output-path ROOT/tgt/coverage/emulator/summary.json"));
        let text = std::fs::read_to_string(&summary).unwrap();
        assert!(text.contains("| zigbee | 90/100 | 90.00 | new |"), "{text}");
        assert!(text.contains("| bulb-tc32 | 5/10 | 50.00 | new |"), "{text}");
        assert!(text.contains("| emulator/tc32emu | 4/4 | 100.00 | new |"), "{text}");
        assert!(text.contains("| xtask | 1/10 | 10.00 | new |"), "{text}");
        let (r, log) = cover(&t, &[("CARGO", "/bin/cargo")], &["--emulator"], "");
        r.unwrap();
        assert!(log.contains(&"/bin/cargo llvm-cov --manifest-path ROOT/emulator/rs/Cargo.toml --no-report -p tc32emu -p ble -p zbnode".to_string()));
        assert!(log.contains(&"/bin/cargo llvm-cov --manifest-path ROOT/emulator/rs/Cargo.toml --no-report -p scenarios --lib".to_string()));
    }

    #[test]
    fn the_baseline_is_checked_and_raised() {
        let t = Tmp::new("cov-base");
        t.put("xtask/coverage.tsv", "crate\tlines\tregions\nzigbee\t95.00\t80.00\nold\t1.00\t1.00\n");
        let e = cover(&t, &[], &["--no-run", "--check"], "").0.unwrap_err().to_string();
        assert_eq!(e, "coverage fell below xtask/coverage.tsv (add tests; or, if the drop is intended, `cargo xtask coverage --update`)");
        cover(&t, &[], &["--no-run", "--update"], "").0.unwrap();
        let b = parse_baseline(&std::fs::read_to_string(t.path("xtask/coverage.tsv")).unwrap()).unwrap();
        assert_eq!(b["zigbee"], Floor { lines: 90.0, regions: 90.0 });
        assert_eq!(b["old"], Floor { lines: 1.0, regions: 1.0 });
        assert_eq!(b["bulb-tc32"], Floor { lines: 50.0, regions: 50.0 });
        cover(&t, &[], &["--no-run", "--check"], "").0.unwrap();
        t.put("xtask/coverage.tsv", "zigbee\tx\t1\n");
        assert!(cover(&t, &[], &["--no-run"], "").0.is_err());
    }

    #[test]
    fn a_failing_or_missing_cargo_llvm_cov() {
        let t = Tmp::new("cov-fail");
        let e = cover(&t, &[], &[], "--no-report").0.unwrap_err().to_string();
        assert_eq!(e, "cargo llvm-cov --no-report --workspace failed (exit status: 1); it runs in `nix develop .#coverage`");
        let e = cover(&t, &[], &["--no-run"], "--html").0.unwrap_err().to_string();
        assert!(e.starts_with("cargo llvm-cov report --html --output-dir "), "{e}");
        let mut real = crate::sh::Real;
        let e = env(&[("CARGO", "/nonexistent/cargo")]);
        let mut c = Ctx { root: t.0.clone(), sh: &mut real, env: &e, exe: "/x".into(), toolchain: "/t".into() };
        let err = run_in(&mut c, &flags(&["--no-run"])).unwrap_err().to_string();
        assert_eq!(err, "starting cargo llvm-cov (in `nix develop .#coverage`)");
    }

    #[test]
    fn a_summary_that_is_not_json() {
        let t = Tmp::new("cov-json");
        let e = env(&[]);
        let mut fake = Fake::new(|c: &Command| {
            let args: Vec<String> = c.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
            if let Some(i) = args.iter().position(|a| a == "--output-path") {
                std::fs::write(&args[i + 1], "nope").unwrap();
            }
            (0, String::new())
        });
        let mut c = Ctx { root: t.0.clone(), sh: &mut fake, env: &e, exe: "/x".into(), toolchain: "/t".into() };
        let err = run_in(&mut c, &flags(&["--no-run"])).unwrap_err().to_string();
        assert_eq!(err, format!("reading {}", t.path("target/coverage/host/summary.json").display()));
    }

    #[test]
    fn paths_must_be_text() {
        use std::os::unix::ffi::OsStrExt;
        let p = Path::new(std::ffi::OsStr::from_bytes(b"/tmp/\xff"));
        assert_eq!(path_str(p).unwrap_err().to_string(), "not UTF-8: /tmp/\u{fffd}");
        assert_eq!(path_str(Path::new("/a")).unwrap(), "/a");
        assert_eq!(Count::default().percent(), 100.0);
        assert_eq!(Count { count: 8, covered: 6 }.missed(), 2);
        assert_eq!(delta(50.0, Some(50.04)), "=");
        assert_eq!(delta(50.0, Some(49.0)), "+1.00");
        assert_eq!(delta(50.0, Some(51.0)), "-1.00");
        assert_eq!(delta(50.0, None), "new");
        assert_eq!(delta(0.05, Some(0.0)), "+0.05");
        assert_eq!(delta(0.0, Some(0.05)), "-0.05");
        assert_eq!(floor2(12.349), 12.34);
    }

    #[test]
    fn the_worst_files_as_many_as_asked_ties_by_name() {
        let crates = BTreeMap::from([("a".to_string(), summary((30, 20), (30, 20)))]);
        let files = vec![
            ("crates/a/src/z.rs".to_string(), summary((10, 5), (10, 5))),
            ("crates/a/src/y.rs".to_string(), summary((10, 5), (10, 5))),
            ("crates/a/src/x.rs".to_string(), summary((10, 10), (10, 10))),
            ("crates/a/src/w.rs".to_string(), summary((10, 9), (10, 9))),
        ];
        let r = render(&crates, &files, &BTreeMap::new(), 2);
        assert!(r.contains("### The 2 files with the most uncovered lines\n\n| file | uncovered lines | line % | region % |\n|---|--:|--:|--:|\n| `crates/a/src/y.rs` | 5 | 50.00 | 50.00 |\n| `crates/a/src/z.rs` | 5 | 50.00 | 50.00 |\n\n"), "{r}");
        assert!(!r.contains("w.rs") && !r.contains("x.rs"));
        let r = render(&crates, &files, &BTreeMap::new(), 9);
        assert!(r.contains("### The 3 files with the most uncovered lines"), "{r}");
        assert!(r.contains("| `crates/a/src/w.rs` | 1 | 90.00 | 90.00 |\n\n"));
        assert!(!render(&crates, &files, &BTreeMap::new(), 0).contains("###"));
        assert!(!render(&crates, &[], &BTreeMap::new(), 5).contains("###"));
        let base = BTreeMap::from([("a".to_string(), Floor { lines: 60.0, regions: 66.65 })]);
        let r = render(&crates, &[], &base, 5);
        assert!(r.contains("| a | 20/30 | 66.67 | +6.67 | 20/30 | 66.67 | = | 100.00 |"), "{r}");
        assert!(r.ends_with("Δ: percentage points against the baseline in `xtask/coverage.tsv`.\n\n"));
        assert_eq!(render(&BTreeMap::new(), &[], &BTreeMap::new(), 5), "Δ: percentage points against the baseline in `xtask/coverage.tsv`.\n\n");
    }

    #[test]
    fn a_baseline_with_comments_and_blank_lines() {
        let b = parse_baseline("# note\n\ncrate\tlines\tregions\n  ble\t1.5\t2.5  \n").unwrap();
        assert_eq!(b, BTreeMap::from([("ble".to_string(), Floor { lines: 1.5, regions: 2.5 })]));
        assert_eq!(parse_baseline("a\tb\tc\td\n").unwrap_err().to_string(), "xtask/coverage.tsv line 1: expected crate, lines, regions");
        assert!(parse_baseline("a\t1\tx\n").is_err());
        let mut tot = summary((1, 1), (2, 1));
        tot.add(summary((3, 0), (2, 2)));
        assert_eq!((tot.lines.count, tot.lines.covered, tot.regions.count, tot.regions.covered), (4, 1, 4, 3));
    }

    #[test]
    fn a_drop_of_regions_alone_fails_too() {
        let base = BTreeMap::from([("ble".to_string(), Floor { lines: 50.0, regions: 50.0 })]);
        let now = BTreeMap::from([("ble".to_string(), summary((100, 60), (100, 40)))]);
        assert_eq!(drops(&now, &base), ["ble: regions 40.00 % < baseline 50.00 %"]);
        let now = BTreeMap::from([("ble".to_string(), summary((100, 40), (100, 60)))]);
        assert_eq!(drops(&now, &base), ["ble: lines 40.00 % < baseline 50.00 %"]);
        let now = BTreeMap::from([("ble".to_string(), summary((10_000, 4_995), (10_000, 4_995)))]);
        assert!(drops(&now, &base).is_empty());
        let now = BTreeMap::from([("ble".to_string(), summary((10_000, 4_994), (10_000, 4_994)))]);
        assert_eq!(drops(&now, &base).len(), 2);
        let edge = BTreeMap::from([("ble".to_string(), Floor { lines: 0.05, regions: 0.05 })]);
        let zero = BTreeMap::from([("ble".to_string(), summary((10, 0), (10, 0)))]);
        assert!(drops(&zero, &edge).is_empty());
    }

    #[test]
    fn the_report_lists_the_worst_files_first() {
        let mut crates = BTreeMap::new();
        crates.insert("a".to_string(), summary((20, 10), (20, 10)));
        let files = vec![("crates/a/src/x.rs".to_string(), summary((10, 9), (10, 9))), ("crates/a/src/y.rs".to_string(), summary((10, 1), (10, 1)))];
        let r = render(&crates, &files, &BTreeMap::new(), 5);
        let (x, y) = (r.find("x.rs").unwrap(), r.find("y.rs").unwrap());
        assert!(y < x);
        assert!(r.contains("| a | 10/20 | 50.00 | new |"));
    }
}
