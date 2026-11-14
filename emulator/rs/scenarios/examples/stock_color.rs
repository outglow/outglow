//! How a stock colour lamp turns Color Control into its LED channels, measured
//! in the emulator (docs/stock-colour.md, `scenarios::profile`): the outputs
//! after each command, once settled, over fine sweeps, and the attributes the
//! bulb reports back. From emulator/rs:
//!
//!   cargo run --release -p scenarios --example stock_color -- EGLO_ZM_RGB_TW__4.60 [EGLO_ZM_RGB_W__4.62 ...]
//!       images of the app (stock/apk/smartlightz/, tools/extract-stock-images.sh) by name
//!       -> analysis/color/<name>.json
//!   cargo run --release -p scenarios --example stock_color -- --bin path.bin --out x.json
//!
//! Options: --cpi N (3), --sats N... (254 192 128 64 1), --hue-step N (1),
//! --sat-step N (1), --level-step N (1), --xy-step N (3277).

use std::path::{Path, PathBuf};

use scenarios::profile::{dump, duties_json, file_stem, pool, repo_root, stock_images, Bulb, Duties, COLOR, ON_OFF};
use scenarios::session::R;
use scenarios::sims::json::J;

struct Args {
    cpi: i64,
    sats: Vec<u8>,
    hue_step: usize,
    sat_step: usize,
    level_step: usize,
    xy_step: usize,
}

fn row(v: J, d: Duties) -> J {
    J::Arr(vec![v, duties_json(&d)])
}

fn attrs(n: &mut Bulb, ids: &[u16]) -> R<J> {
    Ok(J::Obj(n.read(COLOR, ids)?))
}

fn measure(path: &Path, a: &Args) -> R<J> {
    let mut n = Bulb::new(path, a.cpi)?;
    // join() lets 6 s pass after the Device Announce
    if !n.join()? {
        return Err(scenarios::session::Stop::Fail(format!("{}: did not join", path.display())));
    }
    n.ep = 1;
    let ids = [0x0000, 0x0001, 0x0003, 0x0004, 0x0007, 0x0008, 0x4001];
    let mut out: Vec<(String, J)> = vec![("image".into(), J::Str(file_stem(path)))];
    n.cmd(ON_OFF, 0x01, &[])?;
    n.move_level(254)?;
    let o = n.settle()?;
    let at = attrs(&mut n, &ids)?;
    out.push(("boot".into(), J::Obj(vec![("outputs".into(), duties_json(&o)), ("attrs".into(), at)])));

    let mut hue = Vec::new();
    for &s in &a.sats {
        let mut rows = Vec::new();
        for h in (0..255).step_by(a.hue_step) {
            n.hs(h as u8, s, 0)?;
            rows.push(row(J::Int(h as i64), n.settle()?));
        }
        hue.push((s.to_string(), J::Arr(rows)));
    }
    out.push(("hue".into(), J::Obj(hue)));
    let mut sat = Vec::new();
    for h in [0u8, 85, 170, 42] {
        let mut rows = Vec::new();
        for s in (0..255).step_by(a.sat_step) {
            n.hs(h, s as u8, 0)?;
            rows.push(row(J::Int(s as i64), n.settle()?));
        }
        sat.push((h.to_string(), J::Arr(rows)));
    }
    out.push(("sat".into(), J::Obj(sat)));
    let mut level = Vec::new();
    for (h, s) in [(0u8, 254u8), (30, 200), (0, 0)] {
        n.hs(h, s, 0)?;
        n.settle()?;
        let mut rows = Vec::new();
        for v in (1..255).step_by(a.level_step) {
            n.move_level(v as u8)?;
            rows.push(row(J::Int(v as i64), n.settle()?));
        }
        level.push((format!("{h},{s}"), J::Arr(rows)));
    }
    out.push(("level".into(), J::Obj(level)));
    n.move_level(254)?;
    let mut xy = Vec::new();
    for x in (0..65536usize).step_by(a.xy_step) {
        for y in (0..65536usize).step_by(a.xy_step) {
            n.xy(x as u16, y as u16, 0)?;
            xy.push(row(J::Arr(vec![J::Int(x as i64), J::Int(y as i64)]), n.settle()?));
        }
    }
    out.push(("xy".into(), J::Arr(xy)));
    // what the bulb says after each kind of command
    let mut after = Vec::new();
    for name in ["hs 40,200", "xy 0.3,0.6", "ct 250", "hs 200,100", "xy 0.7,0.3"] {
        match name {
            "hs 40,200" => n.hs(40, 200, 0)?,
            "xy 0.3,0.6" => n.xy(19661, 39321, 0)?,
            "ct 250" => n.ct(250, 0)?,
            "hs 200,100" => n.hs(200, 100, 0)?,
            _ => n.xy(45875, 19661, 0)?,
        }
        let o = n.settle()?;
        let at = attrs(&mut n, &ids)?;
        after.push((name.to_string(), J::Obj(vec![("outputs".into(), duties_json(&o)), ("attrs".into(), at)])));
    }
    out.push(("attrs".into(), J::Obj(after)));
    // a transition: hue 0 -> 170 over 2 s, sampled every 100 ms
    n.hs(0, 254, 0)?;
    n.settle()?;
    n.hs(170, 254, 20)?;
    let mut trace = Vec::new();
    for _ in 0..25 {
        n.run(100.0)?;
        let t = n.ms().round_ties_even() as i64;
        let d = n.duties();
        let at = J::Obj(n.read(COLOR, &[0x0000, 0x0001, 0x0002])?);
        trace.push(J::Arr(vec![J::Int(t), duties_json(&d), at]));
    }
    out.push(("transition_hue".into(), J::Arr(trace)));
    // and colour temperature after colour, colour after colour temperature
    n.ct(300, 0)?;
    out.push(("ct_after_hs".into(), duties_json(&n.settle()?)));
    n.hs(85, 254, 0)?;
    out.push(("hs_after_ct".into(), duties_json(&n.settle()?)));
    out.push(("sim_s".into(), J::Float(scenarios::py::round(n.ms() / 1000.0, 1))));
    Ok(J::Obj(out))
}

fn main() {
    let mut a = Args { cpi: 3, sats: vec![254, 192, 128, 64, 1], hue_step: 1, sat_step: 1, level_step: 1, xy_step: 3277 };
    let mut names = Vec::new();
    let mut bin: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
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
                bin = Some(PathBuf::from(&args[i]));
                i += 1;
            }
            "--out" => {
                out = Some(PathBuf::from(&args[i]));
                i += 1;
            }
            "--sats" => {
                a.sats.clear();
                while i < args.len() && !args[i].starts_with("--") {
                    a.sats.push(num(args.get(i), x) as u8);
                    i += 1;
                }
            }
            "--cpi" | "--hue-step" | "--sat-step" | "--level-step" | "--xy-step" => {
                let v = num(args.get(i), x);
                i += 1;
                match x {
                    "--cpi" => a.cpi = v as i64,
                    "--hue-step" => a.hue_step = v.max(1),
                    "--sat-step" => a.sat_step = v.max(1),
                    "--level-step" => a.level_step = v.max(1),
                    _ => a.xy_step = v.max(1),
                }
            }
            s if s.starts_with("--") => panic!("unknown option {s}"),
            s => names.push(s.to_string()),
        }
    }
    let mut jobs: Vec<(PathBuf, PathBuf)> = Vec::new();
    if let Some(b) = bin {
        let o = out.unwrap_or_else(|| b.with_file_name(format!("{}-colour.json", file_stem(&b))));
        jobs.push((b, o));
    }
    if !names.is_empty() {
        let paths = stock_images().unwrap_or_else(|e| {
            eprintln!("{e}");
            std::process::exit(2)
        });
        let dir = repo_root().join("analysis/color");
        std::fs::create_dir_all(&dir).expect("analysis/color");
        for name in &names {
            let p = paths.iter().find(|p| file_stem(p) == *name).unwrap_or_else(|| panic!("no image {name}"));
            jobs.push((p.clone(), dir.join(format!("{name}.json"))));
        }
    }
    let mut failed = false;
    pool(&jobs, jobs.len(), |(p, _)| measure(p, &a), |_, (_, o), r| match r {
        Ok(j) => {
            std::fs::write(o, dump(j, 0)).expect("write");
            println!("{}: {} s simulated", o.display(), j.get("sim_s").py_str());
        }
        Err(e) => {
            println!("{}: FAILED {e:?}", o.display());
            failed = true;
        }
    });
    if failed {
        std::process::exit(1);
    }
}
