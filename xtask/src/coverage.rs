use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

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
    let root = crate::root();
    let out = target_dir(&root).join("coverage");
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
            llvm_cov(&manifest, &["clean", "--workspace"])?;
            for r in ws.runs {
                let mut args = vec!["--no-report"];
                args.extend_from_slice(r);
                llvm_cov(&manifest, &args)?;
            }
        }
        let json = dir.join("summary.json");
        let skip = format!("{TEST_ONLY}|{GLUE}");
        let ignore = ["--ignore-filename-regex", skip.as_str()];
        llvm_cov(&manifest, &[&["report", "--json", "--summary-only", "--output-path", path_str(&json)?][..], &ignore].concat())?;
        llvm_cov(&manifest, &[&["report", "--html", "--output-dir", path_str(&dir)?][..], &ignore].concat())?;
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
    if let Ok(summary) = std::env::var("GITHUB_STEP_SUMMARY") {
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

fn target_dir(root: &Path) -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|| root.join("target"))
}

fn path_str(p: &Path) -> Result<&str> {
    p.to_str().with_context(|| format!("not UTF-8: {}", p.display()))
}

fn llvm_cov(manifest: &Path, args: &[&str]) -> Result<()> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
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
    let status = cmd.status().context("starting cargo llvm-cov (in `nix develop .#coverage`)")?;
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
