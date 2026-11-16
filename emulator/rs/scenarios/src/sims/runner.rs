//! Runs the integration scenarios and reports them as `cargo test` does: one
//! `test sims::NAME ... ok|FAILED` per scenario after its own output (the
//! Python script's lines), then `test result:`.

use std::collections::VecDeque;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Instant;

use super::{sims, SimCtx, SimDef};
use crate::runner::{parse_args_with, Args};
use crate::session::Stop;

fn no_images() -> Vec<crate::image::Image> {
    Vec::new()
}

fn selected(a: &Args, name: &str) -> bool {
    (a.filters.is_empty() || a.filters.iter().any(|f| if a.exact { name == f } else { name.contains(f.as_str()) }))
        && !a.skip.iter().any(|f| name.contains(f.as_str()))
}

/// Rough wall time of the long scenarios [s] (one core, cpi 3), to balance
/// `--shard`; every other scenario counts as `OTHER_S`.
const COST_S: &[(&str, u32)] = &[
    ("addr_conflict_house", 620),
    ("addr_conflict_child", 260),
    ("addr_conflict_echo", 215),
    ("migrate", 190),
    ("addr_conflict_channel", 160),
    ("enddev_wallswitch", 115),
    ("back_to_stock_ble", 95),
    ("rust_sense", 50),
];
const OTHER_S: u32 = 20;

fn cost(name: &str) -> u32 {
    let n = name.trim_start_matches("sims::");
    COST_S.iter().find(|(p, _)| n.starts_with(p)).map_or(OTHER_S, |c| c.1)
}

/// Part `k` (1-based) of `n` of `idx`: the longest first, each to the part
/// with the least so far (ties: the lower part, then the name), so that every
/// CI job computes the same split from the same list.
fn shard(idx: Vec<usize>, names: &[String], k: usize, n: usize) -> Vec<usize> {
    let mut by_cost = idx;
    by_cost.sort_by(|&a, &b| cost(&names[b]).cmp(&cost(&names[a])).then(names[a].cmp(&names[b])));
    let mut load = vec![0u32; n];
    let mut mine = Vec::new();
    for i in by_cost {
        let part = (0..n).min_by_key(|&p| (load[p], p)).unwrap();
        load[part] += cost(&names[i]);
        if part + 1 == k {
            mine.push(i);
        }
    }
    mine.sort();
    mine
}

pub fn main_with(argv: &[String]) -> i32 {
    main_with_extra(argv, Vec::new())
}

/// `main_with` with scenarios defined outside this crate next to these.
pub fn main_with_extra(argv: &[String], extra: Vec<SimDef>) -> i32 {
    let a = match parse_args_with(argv, no_images) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("scenarios sims: {e}");
            return 2;
        }
    };
    let mut all = sims();
    all.extend(extra);
    let names: Vec<String> = all.iter().map(|d| format!("sims::{}", d.name)).collect();
    let mut chosen: Vec<usize> = (0..all.len()).filter(|i| selected(&a, &names[*i])).collect();
    if let Some((k, n)) = a.shard {
        chosen = shard(chosen, &names, k, n);
    }
    if a.list {
        for &i in &chosen {
            println!("{}: test", names[i]);
        }
        return 0;
    }
    // the longest first, so a long one does not start last
    chosen.sort_by(|&x, &y| cost(&names[y]).cmp(&cost(&names[x])).then(x.cmp(&y)));
    let todo: VecDeque<usize> = chosen.into_iter().collect();
    let n_sel = todo.len();
    let short = |i: &usize| names[*i].trim_start_matches("sims::").to_string();
    println!(
        "\nrunning {n_sel} scenario{}: {}",
        if n_sel == 1 { "" } else { "s" },
        todo.iter().map(short).collect::<Vec<_>>().join(", ")
    );
    // which scenarios run now and since when: a scenario's output comes in
    // one piece when it ends, so a long one would leave the log silent
    let running: Arc<Mutex<Vec<(usize, Instant)>>> = Arc::new(Mutex::new(Vec::new()));
    let heartbeat = std::env::var("SIMS_HEARTBEAT_S").ok().and_then(|v| v.parse().ok()).unwrap_or(60u64);
    let t0 = Instant::now();
    let q = Arc::new(Mutex::new(todo));
    let (tx, rx) = mpsc::channel();
    let paint = |st: &str| -> String {
        if !a.color {
            return st.to_string();
        }
        format!("\x1b[{}m{st}\x1b[0m", match st { "ok" => "32", "skipped" => "33", _ => "31" })
    };
    let mut failed = Vec::new();
    let mut skipped = 0;
    std::thread::scope(|sc| {
        for _ in 0..a.jobs.max(1) {
            let (q, tx, all, running) = (q.clone(), tx.clone(), &all, running.clone());
            let (cpi, verbose, images) = (a.cpi, a.verbose, a.images.clone());
            sc.spawn(move || loop {
                let Some(i) = q.lock().unwrap().pop_front() else { break };
                running.lock().unwrap().push((i, Instant::now()));
                let mut cx = SimCtx { cpi, verbose, images: images.clone(), out: String::new(), results: Vec::new(), t_wall: Instant::now(), missing: Default::default() };
                let r = (all[i].run)(&mut cx);
                let err = match r {
                    Ok(()) => None,
                    Err(Stop::Error(m)) | Err(Stop::Fail(m)) | Err(Stop::Inconclusive(m)) | Err(Stop::NotApplicable(m))
                    | Err(Stop::SessionLost(m)) => Some(m),
                };
                // stopped for want of an image nobody gave: skipped
                let missing = cx.missing.borrow().clone();
                let skip = (err.is_some() && !missing.is_empty()).then(|| {
                    missing.iter().map(|m| format!("{m} ({})", SimCtx::how_to_get(m))).collect::<Vec<_>>().join(", ")
                });
                running.lock().unwrap().retain(|r| r.0 != i);
                let _ = tx.send((i, cx.out.clone(), cx.passed() && err.is_none(), err, skip));
            });
        }
        drop(tx);
        let tick = std::time::Duration::from_secs(heartbeat.max(1));
        loop {
            let (i, out, ok, err, skip) = match rx.recv_timeout(tick) {
                Ok(m) => m,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let now: Vec<String> = running
                        .lock()
                        .unwrap()
                        .iter()
                        .map(|(i, t)| {
                            let s = t.elapsed().as_secs();
                            format!("{} ({}:{:02})", short(i), s / 60, s % 60)
                        })
                        .collect();
                    if !now.is_empty() {
                        println!("… still running: {}", now.join(", "));
                    }
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };
            if let Some(why) = skip {
                println!("test {} ... {}: no image {why}\n", names[i], paint("skipped"));
                skipped += 1;
                continue;
            }
            if a.verbose == 0 {
                print!("{out}");
            }
            if let Some(e) = &err {
                println!("ERROR: {e}");
            }
            println!("test {} ... {}\n", names[i], paint(if ok { "ok" } else { "FAILED" }));
            if !ok {
                failed.push(names[i].clone());
            }
        }
    });
    if !failed.is_empty() {
        println!("failures:");
        for f in &failed {
            println!("    {f}");
        }
    }
    println!(
        "\ntest result: {}. {} passed; {} failed; {} ignored; 0 measured; {} filtered out; finished in {:.2}s\n",
        paint(if failed.is_empty() { "ok" } else { "FAILED" }),
        n_sel - failed.len() - skipped,
        failed.len(),
        skipped,
        names.len() - n_sel,
        t0.elapsed().as_secs_f64()
    );
    if failed.is_empty() {
        0
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shards_cover_every_scenario_once() {
        let names: Vec<String> = sims().iter().map(|d| format!("sims::{}", d.name)).collect();
        for n in 1..=8 {
            let mut seen = vec![0; names.len()];
            for k in 1..=n {
                for i in shard((0..names.len()).collect(), &names, k, n) {
                    seen[i] += 1;
                }
            }
            assert!(seen.iter().all(|&c| c == 1), "{n} shards: {seen:?}");
        }
    }

    #[test]
    fn no_other_long_scenario_shares_the_longest_ones_shard() {
        let names: Vec<String> = sims().iter().map(|d| format!("sims::{}", d.name)).collect();
        let part = shard((0..names.len()).collect(), &names, 1, 6);
        assert!(part.iter().any(|&i| names[i] == "sims::addr_conflict_house"));
        assert!(part.iter().all(|&i| names[i] == "sims::addr_conflict_house" || cost(&names[i]) == OTHER_S), "{part:?}");
    }
}
