//! Every board of the Rust firmware against the stock firmware of that board
//! (docs/stock-profiles.md): the same profile (`crate::profile`) taken of our
//! image (target/tc32-<board>/bulb-tc32.bin, crates/bulb-tc32/build.sh
//! BOARD=<name>) and compared with the stock one in analysis/profiles/.
//!
//! Checks per board: the pins and PWM channels, the PWM period, the outputs
//! over the level sweep and the colour temperature sweep, hue and saturation,
//! level at red and the four xy points (within one PWM step), the colour
//! temperature range, the hardware version in Basic, the light of a
//! factory-new bulb after On.
//!
//! One deviation is ours on purpose: the sweeps send Move to Level with
//! On/Off, and at level 1 (MinLevel) ZCL 8 turns the light off, which the
//! stock firmware does not; level 1 is shown apart.
//!
//! `cargo sims -- board_check [--cpi N]`; the boards run in parallel. The
//! example `board_check` does the same with --only, --build and --out.

use super::{SimCtx, SimDef};
use crate::profile::{board_check, default_jobs, Opts};
use crate::session::{Stop, R};

pub fn def() -> SimDef {
    SimDef { name: "board_check", doc: "every board of the Rust firmware against its stock firmware", run }
}

fn run(cx: &mut SimCtx) -> R<()> {
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
