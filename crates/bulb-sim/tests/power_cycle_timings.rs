use memcap as _;

use bulb_core::reset::{PowerCycles, WordStore};
use bulb_core::store::Flash;
use bulb_sim::cut::{cuts, Rng};
use bulb_sim::NorFlash;

const BASE: usize = 0xC_F000;

#[derive(Clone)]
struct NorWords(NorFlash);

impl WordStore for NorWords {
    fn slots(&self) -> usize {
        self.0.sector_size() / 2
    }
    fn read(&self, slot: usize) -> u16 {
        let mut b = [0u8; 2];
        self.0.read(BASE + slot * 2, &mut b);
        u16::from_le_bytes(b)
    }
    fn write(&mut self, slot: usize, word: u16) {
        self.0.program(BASE + slot * 2, &word.to_le_bytes());
    }
    fn erase(&mut self) {
        self.0.erase(BASE);
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Short,
    Medium,
    Long,
}

fn kind(ms: u64) -> Kind {
    match ms {
        0..=5_999 => Kind::Short,
        6_000..=29_999 => Kind::Medium,
        _ => Kind::Long,
    }
}

fn on_time(r: &mut Rng) -> u64 {
    match r.next() % 10 {
        0..=3 => r.next() % 6_000,
        4..=6 => 6_000 + r.next() % 24_000,
        7 => 30_000 + r.next() % 600_000,
        8 => [5_999, 6_000, 6_001, 29_999, 30_000, 30_001, 0, 1][(r.next() % 8) as usize],
        _ => 1_000 * (r.next() % 40),
    }
}

fn run_on(f: &NorFlash, ms: u64, warm_at: Option<u64>, r: &mut Rng) -> bool {
    let (mut c, factory) = PowerCycles::power_on(NorWords(f.clone()), || {});
    let until = warm_at.map_or(ms, |w| w.min(ms));
    let mut t = 0u64;
    loop {
        c.tick(t);
        if t >= until {
            break;
        }
        t = (t + 1 + r.next() % 700).min(until);
    }
    factory
}

fn walk(seed: u64, cycles: usize, sector: usize, cut_every: u64) -> Result<(usize, usize, usize, usize), String> {
    let mut r = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut f = NorFlash::new(BASE, sector, sector);
    let mut history: Vec<(Kind, bool)> = Vec::new();
    let (mut resets, mut patterns, mut cuts_done, mut misses_allowed) = (0, 0, 0, 0);
    let clean = true;
    let mut last_reset_fuzzy = false;
    for i in 0..cycles {
        let ms = on_time(&mut r);
        let warm = (r.next() % 12 == 0).then(|| r.next() % (ms + 1));
        let eff = warm.map_or(ms, |w| w.min(ms));
        let pre = f.snapshot();
        f.clear_ops();
        let factory = run_on(&f, ms, warm, &mut r);
        let mut fuzzy = false;
        let ops = f.ops();
        if cut_every > 0 && !ops.is_empty() && r.next() % cut_every == 0 {
            let j = (r.next() as usize) % ops.len();
            let replay = pre.snapshot();
            for o in &ops[..j] {
                replay.apply(o);
            }
            let variants = cuts(&replay, &ops[j], &mut r);
            let k = (r.next() as usize) % variants.len();
            f = variants.into_iter().nth(k).unwrap().1;
            fuzzy = true;
            cuts_done += 1;
        }
        let n = history.len();
        fn ends(h: &[(Kind, bool)], want: &[Kind]) -> bool {
            match (h.split_last(), want.split_last()) {
                (_, None) => true,
                (None, Some(_)) => false,
                (Some((&(k, fz), rest)), Some((&w, wrest))) => ((fz || k == w) && ends(rest, wrest)) || (fz && ends(rest, want)),
            }
        }
        let tail_ok = |want: &[Kind]| ends(&history, want);
        let retry = last_reset_fuzzy;
        let pattern = tail_ok(&[Kind::Short, Kind::Short, Kind::Short, Kind::Medium, Kind::Medium]);
        let before_clean = n >= 6 && matches!(history[n - 6], (Kind::Long, false)) && history[n - 5..].iter().all(|(_, fz)| !fz);
        if factory {
            resets += 1;
            if !pattern && !retry {
                return Err(format!("seed {seed} power-on {i}: a factory reset after {:?}", &history[n.saturating_sub(6)..]));
            }
        } else if pattern && before_clean && clean {
            patterns += 1;
            return Err(format!("seed {seed} power-on {i}: no factory reset after {:?}", &history[n - 6..]));
        } else if pattern {
            misses_allowed += 1;
        }
        if pattern && before_clean && factory {
            patterns += 1;
        }
        let _ = patterns;
        last_reset_fuzzy = factory && fuzzy || (retry && fuzzy && !factory && false);
        history.push((kind(eff), fuzzy || (eff >= 5_400 && eff <= 6_600) || (eff >= 29_400 && eff <= 30_600)));
        if factory {
            history.clear();
            history.push((kind(eff), fuzzy || (eff >= 5_400 && eff <= 6_600) || (eff >= 29_400 && eff <= 30_600)));
        }
    }
    Ok((resets, patterns, cuts_done, misses_allowed))
}

fn fuzz(sector: usize, cycles: usize, cut_every: u64) {
    let cases: u64 = std::env::var("FUZZ_CASES").ok().and_then(|s| s.parse().ok()).unwrap_or(20);
    let start: u64 = std::env::var("FUZZ_START").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
    let (mut a, mut b, mut c, mut d, mut fails) = (0, 0, 0, 0, Vec::new());
    for seed in start..start + cases {
        match walk(seed, cycles, sector, cut_every) {
            Ok((x, y, z, w)) => {
                a += x;
                b += y;
                c += z;
                d += w;
            }
            Err(e) => fails.push(e),
        }
    }
    eprintln!("sector {sector:#x}: {cases} walks of {cycles} power-ons, {} failed; {a} resets, {b} clean patterns all reset, {c} cuts, {d} patterns after an unclean start", fails.len());
    for f in fails.iter().take(10) {
        eprintln!("  {f}");
    }
    assert!(fails.is_empty());
}

#[test]
fn random_power_timings_reset_exactly_on_the_pattern() {
    fuzz(0x1000, 3000, 0);
}

#[test]
fn random_power_timings_small_sector() {
    fuzz(32, 3000, 0);
}

#[test]
fn random_power_timings_with_cuts_never_reset_falsely() {
    fuzz(0x1000, 3000, 7);
}
