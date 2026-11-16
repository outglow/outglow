use std::path::PathBuf;

use scenarios::profile::{default_jobs, dump, file_stem, pool, profile, repo_root, stock_images, summary, Opts};
use scenarios::sims::json::J;

fn main() {
    let mut a = Opts::default();
    let mut bins: Vec<PathBuf> = Vec::new();
    let mut only: Option<String> = None;
    let mut out = repo_root().join("analysis/profiles");
    let mut jobs = (default_jobs() / 2).max(1);
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    let num = |v: Option<&String>, what: &str| -> usize {
        v.and_then(|s| s.parse().ok()).unwrap_or_else(|| panic!("{what} needs a number"))
    };
    while i < args.len() {
        let x = args[i].as_str();
        i += 1;
        match x {
            "--bin" => {
                while i < args.len() && !args[i].starts_with("--") {
                    bins.push(PathBuf::from(&args[i]));
                    i += 1;
                }
            }
            "--only" => {
                only = args.get(i).cloned();
                i += 1;
            }
            "--out" => {
                out = PathBuf::from(&args[i]);
                i += 1;
            }
            _ => {
                let v = num(args.get(i), x);
                i += 1;
                match x {
                    "--jobs" => jobs = v.max(1),
                    "--cpi" => a.cpi = v as i64,
                    "--level-step" => a.level_step = v.max(1),
                    "--ct-step" => a.ct_step = v.max(1),
                    "--hue-step" => a.hue_step = v.max(1),
                    "--sat-step" => a.sat_step = v.max(1),
                    _ => panic!("unknown option {x}"),
                }
            }
        }
    }
    let mut paths = if bins.is_empty() {
        stock_images().unwrap_or_else(|e| {
            eprintln!("{e}");
            std::process::exit(2)
        })
    } else {
        bins
    };
    if let Some(o) = &only {
        paths.retain(|p| file_stem(p).contains(o.as_str()));
    }
    std::fs::create_dir_all(&out).expect("output directory");
    println!("{} images, {jobs} at a time -> {}", paths.len(), out.display());
    let results = pool(
        &paths,
        jobs,
        |p| profile(p, &file_stem(p), &a),
        |_, p, r| {
            let name = file_stem(p);
            match r {
                Ok(j) => {
                    std::fs::write(out.join(format!("{name}.json")), dump(j, 1)).expect("write profile");
                    let sim = j.get("sim_s");
                    println!(
                        "  {name}: joined {}, {} s simulated in {} s",
                        if j.get("joined").truthy() { "True" } else { "False" },
                        if sim.is_null() { "-".into() } else { sim.py_str() },
                        j.get("wall_s").py_str()
                    );
                }
                Err(e) => println!("  {name}: FAILED {e:?}"),
            }
        },
    );
    let mut ok: Vec<&J> = results.iter().filter_map(|r| r.as_ref().ok()).collect();
    ok.sort_by_key(|p| p.get("image").py_str());
    println!();
    println!("| image | modelId | BLE product | hw | joins | device | colour caps | CT range | PWM pins |");
    println!("|---|---|---|---|---|---|---|---|---|");
    for p in ok {
        println!("{}", summary(p));
    }
}
