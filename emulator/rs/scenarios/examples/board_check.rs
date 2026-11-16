use std::path::PathBuf;
use std::time::Instant;

use scenarios::profile::{board_check, boards, build_board, default_jobs, Opts};

fn main() {
    let mut a = Opts::default();
    let (mut only, mut build, mut jobs, mut out) = (None::<String>, false, default_jobs(), None::<PathBuf>);
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let x = args[i].as_str();
        i += 1;
        let mut val = || {
            i += 1;
            args.get(i - 1).cloned().unwrap_or_else(|| panic!("{x} needs a value"))
        };
        let num = |s: String| -> usize { s.parse().unwrap_or_else(|_| panic!("{x} needs a number")) };
        match x {
            "--only" => only = Some(val()),
            "--build" => build = true,
            "--out" => out = Some(PathBuf::from(val())),
            "--jobs" => jobs = num(val()).max(1),
            "--cpi" => a.cpi = num(val()) as i64,
            "--level-step" => a.level_step = num(val()).max(1),
            "--ct-step" => a.ct_step = num(val()).max(1),
            "--hue-step" => a.hue_step = num(val()).max(1),
            "--sat-step" => a.sat_step = num(val()).max(1),
            _ => panic!("unknown option {x}"),
        }
    }
    let t0 = Instant::now();
    if build {
        for (name, _, _) in boards().expect("boards.tsv") {
            if only.as_deref().is_some_and(|o| !name.contains(o)) {
                continue;
            }
            if let Err(e) = build_board(&name) {
                eprintln!("{e}");
                std::process::exit(1);
            }
            println!("built {name}");
        }
    }
    let res = board_check(only.as_deref(), &a, jobs, out.as_deref(), |l| println!("{l}")).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(2)
    });
    let failed: usize = res.iter().map(|(_, rows)| rows.iter().filter(|r| !r.1).count()).sum();
    println!("\n{}", if failed == 0 { "all boards match their stock firmware".to_string() } else { format!("{failed} check(s) failed") });
    println!("({} boards in {:.0} s)", res.len(), t0.elapsed().as_secs_f64());
    std::process::exit(if failed == 0 { 0 } else { 1 });
}
