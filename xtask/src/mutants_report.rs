use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

#[derive(clap::Args)]
#[command(about = "Per-crate table of cargo-mutants results (caught, missed, timeout, unviable) from one or more mutants.out directories")]
pub struct Args {
    #[arg(required = true, help = "mutants.out directories (or their outcomes.json), e.g. one per shard")]
    outputs: Vec<PathBuf>,
    #[arg(long, help = "Write the missed mutants of all outputs, one per line, to this file")]
    missed: Option<PathBuf>,
    #[arg(long, help = "Exit with an error when a mutant was missed or timed out")]
    strict: bool,
}

#[derive(Deserialize)]
struct Outcomes {
    outcomes: Vec<Outcome>,
}

#[derive(Deserialize)]
struct Outcome {
    scenario: Scenario,
    summary: String,
    #[serde(default)]
    phase_results: Vec<Phase>,
}

#[derive(Deserialize)]
enum Scenario {
    Baseline,
    Mutant(Mutant),
}

#[derive(Deserialize)]
struct Mutant {
    name: String,
    package: String,
}

#[derive(Deserialize)]
struct Phase {
    duration: f64,
}

#[derive(Default, Debug, PartialEq)]
pub struct Tally {
    pub total: usize,
    pub caught: usize,
    pub missed: usize,
    pub timeout: usize,
    pub unviable: usize,
    pub seconds: f64,
    pub missed_names: Vec<String>,
}

impl Tally {
    fn add(&mut self, summary: &str, name: &str, seconds: f64) {
        self.total += 1;
        self.seconds += seconds;
        match summary {
            "CaughtMutant" => self.caught += 1,
            "MissedMutant" => {
                self.missed += 1;
                self.missed_names.push(name.to_string());
            }
            "Timeout" => self.timeout += 1,
            "Unviable" => self.unviable += 1,
            _ => {}
        }
    }

    fn score(&self) -> f64 {
        let viable = self.caught + self.missed + self.timeout;
        if viable == 0 {
            100.0
        } else {
            100.0 * (self.caught + self.timeout) as f64 / viable as f64
        }
    }
}

pub fn run(a: Args) -> Result<()> {
    let mut tallies: BTreeMap<String, Tally> = BTreeMap::new();
    let mut baseline_failed = false;
    for p in &a.outputs {
        let file = if p.is_dir() { p.join("outcomes.json") } else { p.clone() };
        let text = std::fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
        baseline_failed |= tally(&text, &mut tallies).with_context(|| format!("parsing {}", file.display()))?;
    }
    let report = render(&tallies);
    print!("{report}");
    if let Some(path) = &a.missed {
        let mut all: Vec<&String> = tallies.values().flat_map(|t| &t.missed_names).collect();
        all.sort();
        let text: String = all.iter().map(|n| format!("{n}\n")).collect();
        write(path, &text)?;
    }
    if baseline_failed {
        bail!("the tests failed on the unmutated tree");
    }
    if a.strict && tallies.values().any(|t| t.missed > 0 || t.timeout > 0) {
        bail!("mutants missed or timed out");
    }
    Ok(())
}

fn write(path: &Path, text: &str) -> Result<()> {
    std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
}

pub fn tally(json: &str, into: &mut BTreeMap<String, Tally>) -> Result<bool> {
    let o: Outcomes = serde_json::from_str(json)?;
    let mut baseline_failed = false;
    for x in o.outcomes {
        let seconds = x.phase_results.iter().map(|p| p.duration).sum();
        match x.scenario {
            Scenario::Baseline => baseline_failed |= x.summary != "Success",
            Scenario::Mutant(m) => into.entry(m.package).or_default().add(&x.summary, &m.name, seconds),
        }
    }
    Ok(baseline_failed)
}

pub fn render(tallies: &BTreeMap<String, Tally>) -> String {
    let mut s = String::from("## Mutation testing (cargo mutants)\n\n");
    if tallies.is_empty() {
        s.push_str("No mutants.\n");
        return s;
    }
    s.push_str("| crate | mutants | caught | missed | timeout | unviable | caught % | time (s) |\n|---|--:|--:|--:|--:|--:|--:|--:|\n");
    let mut total = Tally::default();
    for (name, t) in tallies {
        let _ = writeln!(
            s,
            "| {name} | {} | {} | {} | {} | {} | {:.1} | {:.0} |",
            t.total,
            t.caught,
            t.missed,
            t.timeout,
            t.unviable,
            t.score(),
            t.seconds
        );
        total.total += t.total;
        total.caught += t.caught;
        total.missed += t.missed;
        total.timeout += t.timeout;
        total.unviable += t.unviable;
        total.seconds += t.seconds;
    }
    let _ = writeln!(
        s,
        "| **total** | {} | {} | {} | {} | {} | {:.1} | {:.0} |\n",
        total.total,
        total.caught,
        total.missed,
        total.timeout,
        total.unviable,
        total.score(),
        total.seconds
    );
    s.push_str("caught %: caught or timed out, of the mutants that built. time: build and test, summed over the mutants.\n\n");
    for (name, t) in tallies.iter().filter(|(_, t)| !t.missed_names.is_empty()) {
        let _ = writeln!(s, "<details><summary>{name}: {} missed</summary>\n", t.missed);
        let mut names = t.missed_names.clone();
        names.sort();
        for n in names {
            let _ = writeln!(s, "- `{}`", n.replace('`', "'"));
        }
        s.push_str("\n</details>\n\n");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUTCOMES: &str = r#"{"outcomes": [
        {"scenario": "Baseline", "summary": "Success", "phase_results": [{"phase": "Build", "duration": 1.0}]},
        {"scenario": {"Mutant": {"name": "crates/a/src/lib.rs:1:1: replace f -> u32 with 0", "package": "a"}}, "summary": "CaughtMutant", "phase_results": [{"duration": 2.0}, {"duration": 0.5}]},
        {"scenario": {"Mutant": {"name": "crates/a/src/lib.rs:2:1: replace + with -", "package": "a"}}, "summary": "MissedMutant", "phase_results": []},
        {"scenario": {"Mutant": {"name": "crates/a/src/lib.rs:3:1: replace < with <=", "package": "a"}}, "summary": "Timeout", "phase_results": []},
        {"scenario": {"Mutant": {"name": "crates/b/src/lib.rs:1:1: replace g with ()", "package": "b"}}, "summary": "Unviable", "phase_results": []}
    ]}"#;

    #[test]
    fn outcomes_are_counted_per_crate() {
        let mut t = BTreeMap::new();
        assert!(!tally(OUTCOMES, &mut t).unwrap());
        let a = &t["a"];
        assert_eq!((a.total, a.caught, a.missed, a.timeout, a.unviable), (3, 1, 1, 1, 0));
        assert_eq!(a.seconds, 2.5);
        assert_eq!(a.missed_names, vec!["crates/a/src/lib.rs:2:1: replace + with -".to_string()]);
        assert_eq!(t["b"].unviable, 1);
        assert!((a.score() - 200.0 / 3.0).abs() < 1e-9);
        assert_eq!(t["b"].score(), 100.0);
    }

    #[test]
    fn shards_add_up() {
        let mut t = BTreeMap::new();
        tally(OUTCOMES, &mut t).unwrap();
        tally(OUTCOMES, &mut t).unwrap();
        assert_eq!(t["a"].total, 6);
        assert_eq!(t["a"].missed, 2);
    }

    #[test]
    fn a_failed_baseline_is_reported() {
        let mut t = BTreeMap::new();
        let json = OUTCOMES.replacen("\"Success\"", "\"Failure\"", 1);
        assert!(tally(&json, &mut t).unwrap());
    }

    #[test]
    fn the_table_has_a_row_per_crate_and_the_missed_ones() {
        let mut t = BTreeMap::new();
        tally(OUTCOMES, &mut t).unwrap();
        let r = render(&t);
        assert!(r.contains("| a | 3 | 1 | 1 | 1 | 0 | 66.7 | 2 |"));
        assert!(r.contains("| b | 1 | 0 | 0 | 0 | 1 | 100.0 | 0 |"));
        assert!(r.contains("| **total** | 4 | 1 | 1 | 1 | 1 | 66.7 | 2 |"));
        assert!(r.contains("a: 1 missed"));
        assert!(r.contains("- `crates/a/src/lib.rs:2:1: replace + with -`"));
        assert!(render(&BTreeMap::new()).contains("No mutants"));
    }
}
