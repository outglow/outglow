use std::collections::VecDeque;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Instant;

use super::{is_weekly, needs_stock, sims, Firmware, SimCtx, SimDef, FIRMWARE_VAR};
use crate::runner::{parse_args_with, Args};
use crate::session::Stop;

fn no_images() -> Vec<crate::image::Image> {
    Vec::new()
}

fn named(a: &Args, name: &str, f: &str) -> bool {
    let short = name.trim_start_matches("sims::");
    if a.exact {
        name == f || short == f
    } else {
        name.contains(f)
    }
}

#[derive(Clone, Copy, Default)]
pub struct Choice {
    pub weekly: bool,
    pub with_stock: bool,
    pub firmware: Option<Firmware>,
}

impl Choice {
    fn firmware(&self) -> Firmware {
        self.firmware.unwrap_or_default()
    }

    fn firmware_of(&self, stock_kind: bool, extra: bool) -> Firmware {
        if stock_kind {
            if self.firmware == Some(Firmware::C) { Firmware::C } else { Firmware::Stock }
        } else if extra {
            self.firmware.unwrap_or(Firmware::Stock)
        } else {
            self.firmware()
        }
    }
}

fn selected(a: &Args, c: Choice, name: &str, stock_kind: bool) -> bool {
    let by_name = a.filters.iter().any(|f| f == name || f == name.trim_start_matches("sims::"));
    let kind = if c.weekly {
        is_weekly(name)
    } else if c.with_stock {
        stock_kind
    } else {
        (!is_weekly(name) && (!stock_kind || c.firmware() == Firmware::Stock)) || by_name
    };
    kind && (a.filters.is_empty() || a.filters.iter().any(|f| named(a, name, f))) && !a.skip.iter().any(|f| name.contains(f.as_str()))
}

fn take_choice(argv: &[String]) -> Result<(Choice, Vec<String>), String> {
    let mut c = Choice::default();
    if let Ok(v) = std::env::var(FIRMWARE_VAR) {
        if !v.is_empty() {
            c.firmware = Some(Firmware::parse(&v).map_err(|e| format!("{FIRMWARE_VAR}: {e}"))?);
        }
    }
    let mut rest = Vec::new();
    let mut it = argv.iter();
    while let Some(x) = it.next() {
        match x.as_str() {
            "--weekly" => c.weekly = true,
            "--with-stock" => c.with_stock = true,
            "--firmware" => c.firmware = Some(Firmware::parse(it.next().ok_or("--firmware needs rust, stock or c")?)?),
            s if s.starts_with("--firmware=") => c.firmware = Some(Firmware::parse(&s["--firmware=".len()..])?),
            _ => rest.push(x.clone()),
        }
    }
    if c.weekly && c.with_stock {
        return Err("--weekly with --with-stock".into());
    }
    Ok((c, rest))
}

const COST_S: &[(&str, u32)] = &[
    ("addr_conflict_house", 620),
    ("addr_conflict_child", 260),
    ("addr_conflict_echo", 215),
    ("migrate", 190),
    ("addr_conflict_channel", 160),
    ("enddev_wallswitch", 115),
    ("back_to_stock_ble", 95),
    ("ble_wireshark", 90),
    ("wireshark", 85),
    ("rust_sense", 50),
    ("stress_", 30),
];
const OTHER_S: u32 = 20;

fn cost(name: &str) -> u32 {
    let n = name.trim_start_matches("sims::");
    COST_S.iter().find(|(p, _)| n.starts_with(p)).map_or(OTHER_S, |c| c.1)
}

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

pub fn main_with_extra(argv: &[String], extra: Vec<SimDef>) -> i32 {
    if let Some(code) = crate::runner::sweep_main(argv, "sims") {
        return code;
    }
    let parsed = take_choice(argv).and_then(|(c, argv)| parse_args_with(&argv, no_images).map(|a| (c, a)));
    let (choice, a) = match parsed {
        Ok(x) => x,
        Err(e) => {
            eprintln!("scenarios sims: {e}");
            return 2;
        }
    };
    let mut all = sims();
    let public = all.len();
    all.extend(extra);
    let names: Vec<String> = all.iter().map(|d| format!("sims::{}", d.name)).collect();
    let stock_kind = |i: usize| i < public && needs_stock(&names[i]);
    let firmware: Vec<Firmware> = (0..all.len()).map(|i| choice.firmware_of(stock_kind(i), i >= public)).collect();
    let mut chosen: Vec<usize> = (0..all.len()).filter(|&i| selected(&a, choice, &names[i], stock_kind(i))).collect();
    if let Some((k, n)) = a.shard {
        chosen = shard(chosen, &names, k, n);
    }
    if a.list {
        for &i in &chosen {
            println!("{}: test", names[i]);
        }
        return 0;
    }
    chosen.sort_by(|&x, &y| cost(&names[y]).cmp(&cost(&names[x])).then(x.cmp(&y)));
    let todo: VecDeque<usize> = chosen.into_iter().collect();
    let n_sel = todo.len();
    let short = |i: &usize| names[*i].trim_start_matches("sims::").to_string();
    println!(
        "\nrunning {n_sel} scenario{}: {}",
        if n_sel == 1 { "" } else { "s" },
        todo.iter().map(short).collect::<Vec<_>>().join(", ")
    );
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
            let (q, tx, all, running, firmware) = (q.clone(), tx.clone(), &all, running.clone(), &firmware);
            let (cpi, verbose, images) = (a.cpi, a.verbose, a.images.clone());
            sc.spawn(move || loop {
                let Some(i) = q.lock().unwrap().pop_front() else { break };
                running.lock().unwrap().push((i, Instant::now()));
                let mut cx = SimCtx::new(cpi, verbose, images.clone(), firmware[i]);
                let r = (all[i].run)(&mut cx);
                if let Some(line) = cx.firmware_line() {
                    cx.say(&line);
                }
                let err = match r {
                    Ok(()) => None,
                    Err(Stop::Error(m)) | Err(Stop::Fail(m)) | Err(Stop::Inconclusive(m)) | Err(Stop::NotApplicable(m))
                    | Err(Stop::SessionLost(m)) => Some(m),
                };
                let missing = cx.missing.borrow().clone();
                let skip = (err.is_some() && !missing.is_empty()).then(|| {
                    missing.iter().map(|m| format!("{m} ({})", SimCtx::how_to_get(m))).collect::<Vec<_>>().join(", ")
                });
                running.lock().unwrap().retain(|r| r.0 != i);
                let wall = cx.t_wall.elapsed().as_secs_f64();
                let _ = tx.send((i, cx.out.clone(), cx.passed() && err.is_none(), err, skip, wall));
            });
        }
        drop(tx);
        let tick = std::time::Duration::from_secs(heartbeat.max(1));
        loop {
            let (i, out, ok, err, skip, wall) = match rx.recv_timeout(tick) {
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
            println!("test {} ... {} ({wall:.0} s)\n", names[i], paint(if ok { "ok" } else { "FAILED" }));
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

    fn nightly(names: &[String]) -> Vec<usize> {
        let a = parse_args_with(&[], no_images).unwrap();
        (0..names.len()).filter(|&i| selected(&a, Choice::default(), &names[i], needs_stock(&names[i]))).collect()
    }

    #[test]
    fn no_other_long_scenario_shares_the_longest_ones_shard() {
        let names: Vec<String> = sims().iter().map(|d| format!("sims::{}", d.name)).collect();
        let idx = nightly(&names);
        let longest = *idx.iter().max_by_key(|&&i| cost(&names[i])).unwrap();
        let part = (1..=6).map(|k| shard(idx.clone(), &names, k, 6)).find(|p| p.contains(&longest)).unwrap();
        assert!(part.iter().all(|&i| i == longest || cost(&names[i]) == OTHER_S), "{part:?}");
    }

    fn pick(names: &[String], argv: &[&str]) -> Vec<String> {
        let argv: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
        let (c, argv) = take_choice(&argv).unwrap();
        let a = parse_args_with(&argv, no_images).unwrap();
        names.iter().filter(|n| selected(&a, c, n, needs_stock(n))).map(|n| n.trim_start_matches("sims::").to_string()).collect()
    }

    #[test]
    fn weekly_scenarios_only_with_weekly_or_by_their_name() {
        let names: Vec<String> = sims().iter().map(|d| format!("sims::{}", d.name)).collect();
        for w in super::super::WEEKLY {
            assert!(names.iter().any(|n| n == &format!("sims::{w}")), "{w} is no scenario");
        }
        let all = pick(&names, &[]);
        assert!(!all.is_empty() && all.iter().all(|n| !is_weekly(n)), "{all:?}");
        let weekly = pick(&names, &["--weekly"]);
        assert_eq!(weekly.len(), super::super::WEEKLY.len());
        assert_eq!(pick(&names, &["addr_conflict_house"]), ["addr_conflict_house"]);
        assert_eq!(pick(&names, &["addr_conflict_ch"]), Vec::<String>::new());
        assert_eq!(pick(&names, &["--weekly", "addr_conflict_ch"]), ["addr_conflict_child", "addr_conflict_channel"]);
        assert_eq!(pick(&names, &["--exact", "rust_fw", "sims::ota_rust"]), ["rust_fw", "ota_rust"]);
        assert_eq!(pick(&names, &["--weekly", "house"]), ["addr_conflict_house"]);
    }

    #[test]
    fn stock_scenarios_only_with_stock_or_by_their_name() {
        let names: Vec<String> = sims().iter().map(|d| format!("sims::{}", d.name)).collect();
        for w in super::super::WITH_STOCK {
            assert!(names.iter().any(|n| n == &format!("sims::{w}")), "{w} is no scenario");
            assert!(!is_weekly(w), "{w} both weekly and with the stock firmware");
        }
        let all = pick(&names, &[]);
        assert!(all.iter().all(|n| !needs_stock(n)), "{all:?}");
        let mut stock = pick(&names, &["--with-stock"]);
        stock.sort();
        let mut want: Vec<String> = super::super::WITH_STOCK.iter().map(|s| s.to_string()).collect();
        want.sort();
        assert_eq!(stock, want);
        let every = pick(&names, &["--firmware", "stock"]);
        assert_eq!(every.len(), all.len() + want.len());
        assert_eq!(pick(&names, &["mixed_parent"]), ["mixed_parent_rust"]);
        assert_eq!(pick(&names, &["--firmware=stock", "mixed_parent"]), ["mixed_parent_rust", "mixed_parent_stock"]);
        assert_eq!(pick(&names, &["--exact", "rust_radio", "rust_fw"]), ["rust_fw", "rust_radio"]);
        assert_eq!(pick(&names, &["--with-stock", "rust_"]), ["rust_gp", "rust_radio", "rust_touchlink"]);
    }

    #[test]
    fn the_stock_firmware_runs_only_where_it_is_asked_for() {
        let c = Choice::default();
        assert_eq!(c.firmware_of(false, false), Firmware::Rust);
        assert_eq!(c.firmware_of(true, false), Firmware::Stock);
        assert_eq!(c.firmware_of(false, true), Firmware::Stock);
        let c = Choice { firmware: Some(Firmware::C), ..c };
        assert_eq!(c.firmware_of(false, false), Firmware::C);
        assert_eq!(c.firmware_of(true, false), Firmware::C);
        let c = Choice { firmware: Some(Firmware::Rust), ..c };
        assert_eq!(c.firmware_of(true, false), Firmware::Stock);
        assert_eq!(c.firmware_of(false, true), Firmware::Rust);
    }

    #[test]
    fn stock_roles_take_the_matching_image() {
        let f = Firmware::Rust;
        assert_eq!(f.instead_of_stock("stock/stock-dimm.bin"), "target/tc32/bulb-tc32.bin");
        assert_eq!(f.instead_of_stock("stock/stock-tw.bin"), "target/tc32-tw/bulb-tc32.bin");
        assert_eq!(f.instead_of_stock("stock/apk/smartlightz/EGLO_ZM_RGB_TW__4.60.bin"), "target/tc32-eglo_zm_rgb_tw-4.60/bulb-tc32.bin");
        assert_eq!(f.instead_of_stock("target/tc32/bulb-tc32.bin"), "target/tc32/bulb-tc32.bin");
        assert_eq!(Firmware::Stock.instead_of_stock("stock/stock-tw.bin"), "stock/stock-tw.bin");
        assert_eq!(Firmware::C.instead_of_stock("stock/stock-tw.bin"), "firmware/build-tw/open-eglo.bin");
        assert_eq!(Firmware::C.instead_of_stock("stock/stock-dimm.bin"), "firmware/build/open-eglo.bin");
    }
}
