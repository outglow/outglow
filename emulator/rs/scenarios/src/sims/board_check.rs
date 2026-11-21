use super::{SimCtx, SimDef};
use crate::profile::{board_check, default_jobs, Opts};
use crate::session::{Stop, R};

pub fn def() -> SimDef {
    SimDef { name: "board_check", doc: "every board of the Rust firmware against its stock firmware", run }
}

fn run(cx: &mut SimCtx) -> R<()> {
    let missing: Vec<String> = crate::profile::boards()
        .map_err(Stop::Error)?
        .into_iter()
        .filter(|b| b.3)
        .map(|(name, _, _, _)| crate::profile::image_dir(&name).join("bulb-tc32.bin"))
        .filter(|p| !p.exists())
        .map(|p| p.strip_prefix(crate::runner::repo_root()).unwrap_or(&p).display().to_string())
        .collect();
    if !missing.is_empty() {
        let n = missing.len();
        cx.missing.borrow_mut().extend(missing.into_iter().take(1).map(|m| format!("{m} and {} more; cargo xtask build --all", n - 1)));
        return Err(Stop::Error(format!("{n} board image(s) not built")));
    }
    let a = Opts { cpi: cx.cpi, ..Default::default() };
    let mut lines = Vec::new();
    let res = board_check(None, &a, default_jobs(), None, |l| {
        if cx.verbose > 0 {
            println!("{l}");
        }
        lines.push(l.to_string());
    })
    .map_err(Stop::Error)?;
    let verbose = cx.verbose;
    cx.verbose = 0;
    for l in &lines {
        cx.say(l);
    }
    cx.verbose = verbose;
    let mut failed = 0;
    for (name, rows) in res {
        for (what, ok, detail) in rows {
            failed += !ok as usize;
            cx.results.push((format!("{name}: {what}"), ok, detail));
        }
    }
    cx.say(&if failed == 0 { "\nall boards match their stock firmware".to_string() } else { format!("\n{failed} check(s) failed") });
    Ok(())
}
