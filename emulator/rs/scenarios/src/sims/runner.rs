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
    if a.list {
        for n in names.iter().filter(|n| selected(&a, n)) {
            println!("{n}: test");
        }
        return 0;
    }
    let todo: VecDeque<usize> = (0..all.len()).filter(|i| selected(&a, &names[*i])).collect();
    let n_sel = todo.len();
    println!("\nrunning {n_sel} scenario{}", if n_sel == 1 { "" } else { "s" });
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
            let (q, tx, all) = (q.clone(), tx.clone(), &all);
            let (cpi, verbose, images) = (a.cpi, a.verbose, a.images.clone());
            sc.spawn(move || loop {
                let Some(i) = q.lock().unwrap().pop_front() else { break };
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
                let _ = tx.send((i, cx.out.clone(), cx.passed() && err.is_none(), err, skip));
            });
        }
        drop(tx);
        for (i, out, ok, err, skip) in rx.iter() {
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
