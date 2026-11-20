extern crate std;
use std::{boxed::Box, format, println, string::String, vec::Vec};

use super::{Neighbor, AGE_LIMIT, LINK_STATUS_MS};

struct Rng(u64);

impl Rng {
    fn f(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545F4914F6CDD1D) >> 11) as f64 / (1u64 << 53) as f64
    }
}

enum Channel {
    Steady(f64),
    Step(f64, f64, usize),
    Bursty { pg: f64, pb: f64, enter: f64, leave: f64, bad: bool },
}

impl Channel {
    fn attempt(&mut self, frame: usize, rng: &mut Rng) -> bool {
        let p = match self {
            Channel::Steady(p) => *p,
            Channel::Step(p0, p1, at) => {
                if frame < *at {
                    *p0
                } else {
                    *p1
                }
            }
            Channel::Bursty { pg, pb, enter, leave, bad } => {
                let flip = rng.f() < if *bad { *leave } else { *enter };
                if flip {
                    *bad = !*bad;
                }
                if *bad {
                    *pb
                } else {
                    *pg
                }
            }
        };
        rng.f() < p
    }

    fn truth(&self, frame: usize) -> f64 {
        match self {
            Channel::Steady(p) => *p,
            Channel::Step(p0, p1, at) => {
                if frame < *at {
                    *p0
                } else {
                    *p1
                }
            }
            Channel::Bursty { pg, pb, enter, leave, .. } => {
                let bad = enter / (enter + leave);
                pg * (1.0 - bad) + pb * bad
            }
        }
    }
}

fn ideal(p: f64) -> u8 {
    if p <= 0.0 {
        7
    } else {
        (1.0 / p.powi(4)).round().min(7.0) as u8
    }
}

const MAC_ATTEMPTS: u8 = 4;

trait Estimator {
    fn sample(&mut self, attempts: u8, ok: bool);
    fn measured(&self) -> bool;
    fn cost(&self) -> u8;
    fn p(&self) -> f64;
}

impl Estimator for Neighbor {
    fn sample(&mut self, attempts: u8, ok: bool) {
        self.delivery_sample(attempts, ok)
    }
    fn measured(&self) -> bool {
        Neighbor::measured(self)
    }
    fn cost(&self) -> u8 {
        self.link_cost()
    }
    fn p(&self) -> f64 {
        self.delivery as f64 / 65535.0
    }
}

#[derive(Clone, Copy)]
struct Params {
    shift: u32,
    margin: f64,
    warmup: u8,
    first_only: bool,
}

impl Params {
    fn name(&self) -> String {
        format!(
            "1/{:<2} h{:.2} w{:<2} {}",
            1u32 << self.shift,
            self.margin,
            self.warmup,
            if self.first_only { "frame" } else { "attempt" }
        )
    }
}

struct Candidate {
    k: Params,
    n: u8,
    hits: u8,
    p: u32,
    cost: u8,
}

impl Candidate {
    fn new(shift: u32, margin: f64) -> Candidate {
        Candidate::with(Params { shift, margin, warmup: 16, first_only: false })
    }
    fn with(k: Params) -> Candidate {
        Candidate { k, n: 0, hits: 0, p: 0, cost: 0 }
    }
    fn spec(p: f64) -> u8 {
        ideal(p.clamp(0.0, 1.0))
    }
    fn one(&mut self, got: bool) {
        let k = self.k;
        if self.n < k.warmup {
            self.n += 1;
            self.hits += got as u8;
            if self.n == k.warmup {
                self.p = self.hits as u32 * 65535 / k.warmup as u32;
            }
        } else {
            let target: i64 = if got { 65535 } else { 0 };
            let p = self.p as i64;
            let half = 1i64 << (k.shift - 1);
            let d = target - p;
            let step = if d >= 0 { (d + half) >> k.shift } else { -((-d + half) >> k.shift) };
            self.p = (p + step).clamp(0, 65535) as u32;
        }
        if self.n >= k.warmup {
            let p = self.p();
            let new = Self::spec(p);
            self.cost = if self.cost == 0 || k.margin == 0.0 {
                new
            } else if new < self.cost && Self::spec(p - k.margin) < self.cost {
                new
            } else if new > self.cost && Self::spec(p + k.margin) > self.cost {
                new
            } else {
                self.cost
            };
        }
    }
}

impl Estimator for Candidate {
    fn sample(&mut self, attempts: u8, ok: bool) {
        if self.k.first_only {
            self.one(ok && attempts == 1);
            return;
        }
        for k in 0..attempts {
            self.one(ok && k + 1 == attempts);
        }
    }
    fn measured(&self) -> bool {
        self.n >= self.k.warmup
    }
    fn cost(&self) -> u8 {
        self.cost
    }
    fn p(&self) -> f64 {
        self.p as f64 / 65535.0
    }
}

#[derive(Clone, Copy)]
struct EtxParams {
    shift: u32,
    margin: f64,
    penalty: u8,
    warmup: u8,
}

impl EtxParams {
    fn name(&self) -> String {
        format!("ETX 1/{:<2} h{:.2} +{:<2}", 1u32 << self.shift, self.margin, self.penalty)
    }
}

struct Etx {
    k: EtxParams,
    frames: u8,
    sum: u32,
    etx: u32,
    cost: u8,
}

impl Etx {
    fn with(k: EtxParams) -> Etx {
        Etx { k, frames: 0, sum: 0, etx: 0, cost: 0 }
    }
}

impl Estimator for Etx {
    fn sample(&mut self, attempts: u8, ok: bool) {
        let k = self.k;
        let x = 256 * (attempts as u32 + if ok { 0 } else { k.penalty as u32 });
        if self.frames < k.warmup {
            self.frames += 1;
            self.sum += x;
            if self.frames == k.warmup {
                self.etx = self.sum / k.warmup as u32;
            }
        } else {
            let e = self.etx as i64;
            let d = x as i64 - e;
            let half = 1i64 << (k.shift - 1);
            let step = if d >= 0 { (d + half) >> k.shift } else { -((-d + half) >> k.shift) };
            self.etx = (e + step).max(256) as u32;
        }
        if self.frames >= k.warmup {
            let p = self.p();
            let new = Candidate::spec(p);
            self.cost = if self.cost == 0 || k.margin == 0.0 {
                new
            } else if new < self.cost && Candidate::spec(p - k.margin) < self.cost {
                new
            } else if new > self.cost && Candidate::spec(p + k.margin) > self.cost {
                new
            } else {
                self.cost
            };
        }
    }
    fn measured(&self) -> bool {
        self.frames >= self.k.warmup
    }
    fn cost(&self) -> u8 {
        self.cost
    }
    fn p(&self) -> f64 {
        256.0 / self.etx.max(256) as f64
    }
}

#[derive(Clone, Copy)]
enum Point {
    Ewma(Params),
    Etx(EtxParams),
}

impl Point {
    fn name(&self) -> String {
        match self {
            Point::Ewma(k) => k.name(),
            Point::Etx(k) => k.name(),
        }
    }
    fn make(self) -> Box<dyn Estimator> {
        match self {
            Point::Ewma(k) => Box::new(Candidate::with(k)),
            Point::Etx(k) => Box::new(Etx::with(k)),
        }
    }
}

fn send(n: &mut dyn Estimator, ch: &mut Channel, frame: usize, rng: &mut Rng) -> bool {
    for a in 1..=MAC_ATTEMPTS {
        if ch.attempt(frame, rng) {
            n.sample(a, true);
            return true;
        }
    }
    n.sample(MAC_ATTEMPTS, false);
    false
}

#[derive(Default)]
struct Score {
    frames: usize,
    exact: usize,
    within1: usize,
    flaps: usize,
    err_p: f64,
    react: Vec<usize>,
}

impl Score {
    fn add(&mut self, cost: u8, truth: f64, prev: u8, p_est: f64) {
        let want = ideal(truth);
        self.frames += 1;
        self.exact += (cost == want) as usize;
        self.within1 += (cost.abs_diff(want) <= 1) as usize;
        self.flaps += (cost != prev) as usize;
        self.err_p += p_est - truth;
    }
    fn within1(&self) -> f64 {
        100.0 * self.within1 as f64 / self.frames as f64
    }
    fn flaps(&self) -> f64 {
        1000.0 * self.flaps as f64 / self.frames as f64
    }
    fn bias(&self) -> f64 {
        self.err_p / self.frames as f64
    }
    fn react(&self) -> (usize, usize) {
        let mut s = self.react.clone();
        s.sort();
        if s.is_empty() {
            (0, 0)
        } else {
            (s[s.len() / 2], s[s.len() * 9 / 10])
        }
    }
    fn row(&self, name: &str) {
        let react = if self.react.is_empty() {
            "      -".into()
        } else {
            let (m, p) = self.react();
            format!("{m:>3}/{p:>3}")
        };
        println!(
            "{name:34} {:>6.1}% {:>6.1}% {:>6.1} {:>+7.3} {react}",
            100.0 * self.exact as f64 / self.frames as f64,
            self.within1(),
            self.flaps(),
            self.bias(),
        );
    }
}

const SEEDS: u64 = 200;
const FRAMES: usize = 1000;

type Make<'a> = &'a dyn Fn() -> Box<dyn Estimator>;

fn run(est: Make, make: &dyn Fn() -> Channel, step_at: Option<usize>) -> Score {
    run_seeds(est, make, step_at, 1..SEEDS + 1)
}

fn run_seeds(est: Make, make: &dyn Fn() -> Channel, step_at: Option<usize>, seeds: core::ops::Range<u64>) -> Score {
    let mut sc = Score::default();
    for seed in seeds {
        let mut rng = Rng(seed.wrapping_mul(0x9E3779B97F4A7C15) | 1);
        let mut ch = make();
        let mut n = est();
        let mut prev = 0;
        let mut reacted = None;
        for f in 0..FRAMES {
            send(n.as_mut(), &mut ch, f, &mut rng);
            if !n.measured() {
                continue;
            }
            let c = n.cost();
            let t = ch.truth(f);
            if prev != 0 {
                sc.add(c, t, prev, n.p());
            }
            prev = c;
            if let Some(at) = step_at {
                if f >= at && reacted.is_none() && c.abs_diff(ideal(t)) <= 1 {
                    reacted = Some(f - at);
                }
            }
        }
        if step_at.is_some() {
            sc.react.push(reacted.unwrap_or(FRAMES));
        }
    }
    sc
}

fn header(title: &str) {
    println!("\n{title}");
    println!("{:34} {:>7} {:>7} {:>6} {:>7} {:>7}", "channel", "exact", "+-1", "flaps", "bias p", "react");
    println!("{:34} {:>7} {:>7} {:>6} {:>7} {:>7}", "", "", "", "/1000f", "", "med/p90");
}

fn firmware() -> Box<dyn Estimator> {
    let mut n = Neighbor::EMPTY;
    n.used = true;
    Box::new(n)
}

const STEADY: [f64; 10] = [0.99, 0.95, 0.9, 0.85, 0.8, 0.75, 0.7, 0.6, 0.5, 0.3];
const STEPS: [(f64, f64); 5] = [(0.98, 0.6), (0.6, 0.98), (0.95, 0.8), (0.98, 0.0), (0.0, 0.98)];
const BURSTS: [(&str, f64, f64, f64, f64); 3] = [
    ("rare short bursts (BLE-like)", 0.97, 0.1, 0.02, 0.5),
    ("frequent short bursts", 0.97, 0.1, 0.1, 0.5),
    ("long fades", 0.97, 0.2, 0.01, 0.05),
];

#[test]
fn link_eval_scorecard() {
    println!("\nthe firmware's estimator (average 1/128 an attempt, hysteresis 0.06)");
    header("steady links (truth = spec cost of p)");
    let mut worst_within1 = 100.0f64;
    let mut worst_flaps = 0.0f64;
    for p in STEADY {
        let s = run(&firmware, &|| Channel::Steady(p), None);
        s.row(&format!("p = {p:.2} (cost {})", ideal(p)));
        if p >= 0.9 || p <= 0.3 {
            worst_within1 = worst_within1.min(s.within1());
        }
        worst_flaps = worst_flaps.max(s.flaps());
    }
    header("a link that changes at frame 300");
    for (p0, p1) in STEPS {
        let s = run(&firmware, &|| Channel::Step(p0, p1, 300), Some(300));
        s.row(&format!("{p0:.2} -> {p1:.2}"));
    }
    header("bursts (Gilbert-Elliott; truth = long-run mean)");
    for (name, pg, pb, enter, leave) in BURSTS {
        let s = run(&firmware, &|| Channel::Bursty { pg, pb, enter, leave, bad: false }, None);
        s.row(name);
    }
    let (wrong, regret, wrong_ideal) = route_choice(&firmware);
    println!("\nroute choice, direct vs two hops by the sum of costs (against the lower airtime):");
    println!("  measured costs wrong {wrong:.1}%, mean extra airtime when wrong {regret:.0}%; exact spec costs wrong {wrong_ideal:.1}%");
    aging();
    assert!(worst_within1 > 90.0, "within one of the spec cost only {worst_within1:.1}%");
    assert!(worst_flaps < 50.0, "the cost changed {worst_flaps:.0} times per 1000 frames on a steady link");
}

#[test]
#[ignore = "a comparison to read, not a check"]
fn link_eval_variants() {
    let variants: [(&str, Make); 10] = [
        ("firmware", &firmware),
        ("1/8 rounded", &|| Box::new(Candidate::new(3, 0.0))),
        ("1/16 rounded", &|| Box::new(Candidate::new(4, 0.0))),
        ("1/32 rounded", &|| Box::new(Candidate::new(5, 0.0))),
        ("1/8 + hysteresis 0.03", &|| Box::new(Candidate::new(3, 0.03))),
        ("1/16 + hysteresis 0.03", &|| Box::new(Candidate::new(4, 0.03))),
        ("1/16 + hysteresis 0.05", &|| Box::new(Candidate::new(4, 0.05))),
        ("1/32 + hysteresis 0.03", &|| Box::new(Candidate::new(5, 0.03))),
        ("1/8 + hysteresis 0.08", &|| Box::new(Candidate::new(3, 0.08))),
        ("1/16 + hysteresis 0.12", &|| Box::new(Candidate::new(4, 0.12))),
    ];
    println!();
    println!(
        "{:24} | {:>5} {:>5} {:>5} | {:>5} {:>5} | {:>5} | {:>7} {:>7} {:>7} | {:>5} {:>5}",
        "", "+-1", "+-1", "|bias", "flaps", "flaps", "flaps", "react", "react", "react", "route", "extra"
    );
    println!(
        "{:24} | {:>5} {:>5} {:>5} | {:>5} {:>5} | {:>5} | {:>7} {:>7} {:>7} | {:>5} {:>5}",
        "estimator", "ends", ".7-.9", "p|", ".7-.9", "ends", "burst", ".98>.6", ".6>.98", ".98>0", "wrong", "air"
    );
    for (name, est) in variants {
        let (mut w_ends, mut w_mid, mut f_ends, mut f_mid, mut bias) = (0.0f64, 0.0, 0.0, 0.0, 0.0);
        let (mut n_ends, mut n_mid) = (0.0, 0.0);
        for p in STEADY {
            let s = run(est, &|| Channel::Steady(p), None);
            bias += s.bias().abs() / STEADY.len() as f64;
            if (0.7..=0.9).contains(&p) {
                w_mid += s.within1();
                f_mid += s.flaps();
                n_mid += 1.0;
            } else {
                w_ends += s.within1();
                f_ends += s.flaps();
                n_ends += 1.0;
            }
        }
        let mut f_burst = 0.0;
        for (_, pg, pb, enter, leave) in BURSTS {
            f_burst += run(est, &|| Channel::Bursty { pg, pb, enter, leave, bad: false }, None).flaps() / BURSTS.len() as f64;
        }
        let react = |p0: f64, p1: f64| {
            let (m, p90) = run(est, &|| Channel::Step(p0, p1, 300), Some(300)).react();
            format!("{m}/{p90}")
        };
        let (wrong, regret, _) = route_choice(est);
        println!(
            "{name:24} | {:>4.1}% {:>4.1}% {:>5.3} | {:>5.0} {:>5.0} | {:>5.0} | {:>7} {:>7} {:>7} | {:>4.1}% {:>4.0}%",
            w_ends / n_ends,
            w_mid / n_mid,
            bias,
            f_mid / n_mid,
            f_ends / n_ends,
            f_burst,
            react(0.98, 0.6),
            react(0.6, 0.98),
            react(0.98, 0.0),
            wrong,
            regret,
        );
    }
    println!("\n+-1: frames with the cost within one of the spec's (steady links; ends = 0.99, 0.95, 0.6, 0.5, 0.3)");
    println!("flaps: cost changes per 1000 frames; react: frames after a change until the cost is first within one (median/p90)");
    println!("route wrong: direct vs two hops chosen against the lower airtime; extra air: mean extra airtime when wrong");
}

#[derive(Clone, Copy, Default)]
struct Eval {
    flaps: f64,
    react: f64,
    react90: f64,
    miss: f64,
}

impl Eval {
    fn dominates(&self, o: &Eval) -> bool {
        let le = self.flaps <= o.flaps && self.react <= o.react && self.miss <= o.miss;
        let lt = self.flaps < o.flaps || self.react < o.react || self.miss < o.miss;
        le && lt
    }
}

fn evaluate(est: Make, seeds: core::ops::Range<u64>) -> Eval {
    let mut e = Eval::default();
    let mut nf = 0.0;
    for p in STEADY {
        let s = run_seeds(est, &|| Channel::Steady(p), None, seeds.clone());
        e.miss += (100.0 - s.within1()) / STEADY.len() as f64;
        if (0.7..=0.9).contains(&p) {
            e.flaps += s.flaps();
            nf += 1.0;
        }
    }
    for (_, pg, pb, enter, leave) in BURSTS {
        e.flaps += run_seeds(est, &|| Channel::Bursty { pg, pb, enter, leave, bad: false }, None, seeds.clone()).flaps();
        nf += 1.0;
    }
    e.flaps /= nf;
    for (p0, p1) in [(0.98, 0.6), (0.6, 0.98)] {
        let (m, p90) = run_seeds(est, &|| Channel::Step(p0, p1, 300), Some(300), seeds.clone()).react();
        e.react += m as f64 / 2.0;
        e.react90 += p90 as f64 / 2.0;
    }
    e
}

#[test]
#[ignore = "a search to read, not a check"]
fn link_eval_pareto() {
    let train = 1..101u64;
    let check = 5001..5101u64;
    let mut grid: Vec<Point> = Vec::new();
    for shift in 1..=5 {
        for m in 0..=20 {
            for penalty in [0u8, 4, 12] {
                grid.push(Point::Etx(EtxParams { shift, margin: m as f64 / 100.0, penalty, warmup: 4 }));
            }
        }
    }
    for shift in 2..=6 {
        for m in 0..=20 {
            for warmup in [16u8] {
                for first_only in [false, true] {
                    grid.push(Point::Ewma(Params { shift, margin: m as f64 / 100.0, warmup, first_only }));
                }
            }
        }
    }
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let chunk = grid.len().div_ceil(threads);
    let scored: Vec<(Point, Eval)> = std::thread::scope(|sc| {
        let hs: Vec<_> = grid
            .chunks(chunk)
            .map(|part| {
                let train = train.clone();
                sc.spawn(move || {
                    part.iter()
                        .map(|&k| {
                            let mk = move || k.make();
                            (k, evaluate(&mk, train.clone()))
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        hs.into_iter().flat_map(|h| h.join().unwrap()).collect()
    });
    let mut front: Vec<(Point, Eval)> =
        scored.iter().filter(|(_, e)| !scored.iter().any(|(_, o)| o.dominates(e))).copied().collect();
    front.sort_by(|a, b| a.1.react.partial_cmp(&b.1.react).unwrap().then(a.1.flaps.partial_cmp(&b.1.flaps).unwrap()));

    let fw = evaluate(&firmware, train.clone());
    let fw_check = evaluate(&firmware, check.clone());
    println!("\n{} combinations, {} on the Pareto front (scored on seeds {:?}, checked on {:?})", grid.len(), front.len(), train, check);
    println!(
        "{:28} | {:>6} {:>6} {:>5} {:>5} | {:>6} {:>6} {:>5} | {:>5} {:>5}",
        "estimator", "flaps", "react", "p90", "miss%", "flaps'", "react'", "miss'", "route", "air"
    );
    let line = |name: &str, e: &Eval, c: &Eval, route: (f64, f64, f64)| {
        println!(
            "{name:28} | {:>6.0} {:>6.1} {:>5.0} {:>5.1} | {:>6.0} {:>6.1} {:>5.1} | {:>4.1}% {:>4.0}%",
            e.flaps, e.react, e.react90, e.miss, c.flaps, c.react, c.miss, route.0, route.1
        );
    };
    line("firmware", &fw, &fw_check, route_choice(&firmware));
    let mut checked = Vec::new();
    for (k, e) in &front {
        let k = *k;
        let mk = move || k.make();
        let c = evaluate(&mk, check.clone());
        line(&k.name(), e, &c, route_choice(&mk));
        checked.push((k, c));
    }
    println!("\nflaps: cost changes per 1000 frames (links 0.7-0.9 and bursty); react: frames until the cost follows a change,");
    println!("median (p90); miss: frames more than one off the spec's cost; ' = on the seeds not used to choose; route: wrong");
    println!("direct-vs-two-hops choices and the extra airtime when wrong");
    println!();
    for max_miss in [100.0, 12.0, 9.0] {
        for limit in [10.0, 15.0, 20.0, 30.0] {
            if let Some((k, c)) = checked
                .iter()
                .filter(|(_, c)| c.react <= limit && c.miss <= max_miss)
                .min_by(|a, b| a.1.flaps.partial_cmp(&b.1.flaps).unwrap())
            {
                println!(
                    "react <= {limit:>2}, miss <= {max_miss:>3}%: calmest {} (flaps {:.0}, react {:.1}/{:.0}, miss {:.1}%)",
                    k.name(),
                    c.flaps,
                    c.react,
                    c.react90,
                    c.miss
                );
            }
        }
    }
}

fn airtime(hops: &[f64]) -> f64 {
    let mut air = 0.0;
    let mut reach = 1.0;
    for &p in hops {
        let q = 1.0 - p;
        air += reach * (1.0 - q.powi(4)) / p.max(1e-9);
        reach *= 1.0 - q.powi(4);
    }
    air / reach.max(1e-9)
}

fn route_choice(est: Make) -> (f64, f64, f64) {
    let ps = [0.99, 0.95, 0.9, 0.85, 0.8, 0.7, 0.6, 0.5, 0.4];
    let mut cases = 0;
    let mut wrong = 0;
    let mut regret = 0.0;
    let mut wrong_ideal = 0;
    let mut rng = Rng(7);
    let c = |p: f64, rng: &mut Rng| {
        let mut n = est();
        let mut ch = Channel::Steady(p);
        for f in 0..300 {
            send(n.as_mut(), &mut ch, f, rng);
        }
        n.cost() as u32
    };
    for &pd in &ps {
        for &p1 in &ps {
            for &p2 in &ps {
                let (a, b) = (airtime(&[pd]), airtime(&[p1, p2]));
                let best_direct = a <= b;
                for _ in 0..20 {
                    cases += 1;
                    let direct = c(pd, &mut rng) <= c(p1, &mut rng) + c(p2, &mut rng);
                    if direct != best_direct {
                        wrong += 1;
                        regret += (a - b).abs() / a.min(b);
                    }
                }
                let direct = ideal(pd) as u32 <= ideal(p1) as u32 + ideal(p2) as u32;
                wrong_ideal += (direct != best_direct) as usize;
            }
        }
    }
    (
        100.0 * wrong as f64 / cases as f64,
        100.0 * regret / wrong.max(1) as f64,
        100.0 * wrong_ideal as f64 / ps.len().pow(3) as f64,
    )
}

fn aging() {
    println!("\naging: neighbour that is there, its link status lost with q; stale events per day");
    let day_ms = 24 * 3600 * 1000u64;
    for q in [0.1, 0.3, 0.5, 0.7, 0.8, 0.9] {
        let mut stales = 0u64;
        let mut stale_ticks = 0u64;
        let mut ticks = 0u64;
        let runs = 20;
        for seed in 1..=runs {
            let mut rng = Rng(seed * 1234567 + 1);
            let mut n = Neighbor::EMPTY;
            n.used = true;
            let mut next_ls = (rng.f() * 15_000.0) as u64;
            let mut next_age = LINK_STATUS_MS as u64;
            let mut t = 0u64;
            while t < day_ms {
                if next_ls <= next_age {
                    t = next_ls;
                    if rng.f() >= q {
                        n.age = 0;
                    }
                    next_ls += 14_000 + (rng.f() * 1000.0) as u64;
                } else {
                    t = next_age;
                    n.age = n.age.saturating_add(1);
                    ticks += 1;
                    stale_ticks += (n.age > AGE_LIMIT) as u64;
                    if n.age == AGE_LIMIT + 1 {
                        stales += 1;
                    }
                    next_age += LINK_STATUS_MS as u64;
                }
            }
        }
        let theory = 86400.0 / 15.0 * q.powi(AGE_LIMIT as i32 + 1) * (1.0 - q);
        println!(
            "  q = {q:.1}: {:>7.1} stale/day (about {:>7.1} by q^{}(1-q)), stale {:>5.1}% of the time",
            stales as f64 / runs as f64,
            theory,
            AGE_LIMIT + 1,
            100.0 * stale_ticks as f64 / ticks as f64
        );
    }
}

mod home;

#[test]
#[ignore = "a comparison to read, not a check"]
fn link_eval_home() {
    use home::{Algo, Floor};
    let firmware = Algo { shift: 7, margin: 0.06, expire: 0, floor: Floor::Measured, explore_s: 0 };
    let mut algos = std::vec![
        Algo { floor: Floor::Oracle, ..firmware },
        Algo { floor: Floor::LqiOnly, ..firmware },
        Algo { shift: 4, margin: 0.12, ..firmware },
        firmware,
    ];
    for shift in [4, 5, 6, 7] {
        for margin in [0.06, 0.12, 0.18] {
            for expire in [0u8, 120, 240] {
                algos.push(Algo { shift, margin, expire, ..firmware });
            }
        }
    }
    for shift in [4, 7] {
        for explore_s in [15, 30, 60, 300] {
            algos.push(Algo { shift, margin: if shift == 4 { 0.12 } else { 0.06 }, explore_s, ..firmware });
        }
    }
    let seeds = std::env::var("HOME_SEEDS").ok().and_then(|s| s.parse().ok()).unwrap_or(6);
    let hours = std::env::var("HOME_HOURS").ok().and_then(|s| s.parse().ok()).unwrap_or(6.0);
    for fades in [false, true] {
        println!(
            "\nthe owner's home: links replayed from the black boxes of nine bulbs, {seeds} runs of {hours} h each{}",
            if fades { ", and every link fades to half its delivery for 6-30 min about every 2 h" } else { "" }
        );
        for (a, o) in home::table(&algos, seeds, hours, fades) {
            o.row(&a.name());
        }
    }
}
