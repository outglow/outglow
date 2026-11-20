use std::{format, println, string::String, vec, vec::Vec};

use super::super::LINK_STATUS_MS;
use super::{ideal, Candidate, Estimator, Params, Rng, MAC_ATTEMPTS};
use crate::nwk;

#[path = "home_data.rs"]
mod data;
use data::{HEARD, NODES, SENT_PER_S, TRACES, WINDOWS};

const TICK_MS: u64 = 100;
const MTORR_MS: u64 = 60_000;
const WINDOW_MS: u64 = 120_000;
const BLOCK: usize = 16;
const MAX_HOPS: usize = 5;
const FADE_PER_WINDOW: f64 = 1.0 / 60.0;
const FADE_DEPTH: f64 = 0.5;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Floor {
    Measured,
    Lqi,
    LqiOnly,
    Oracle,
}

#[derive(Clone, Copy)]
pub(super) struct Algo {
    pub shift: u32,
    pub margin: f64,
    pub expire: u8,
    pub floor: Floor,
    pub explore_s: u32,
}

impl Algo {
    pub(super) fn name(&self) -> String {
        let explore = match self.explore_s {
            0 => String::new(),
            s => format!(" direct every {s:>3} s"),
        };
        let base: String = match self.floor {
            Floor::LqiOnly => "LQI and link status only".into(),
            Floor::Oracle => "oracle (true mean p)".into(),
            f => format!(
                "1/{:<2} h{:.2} {} {}",
                1u32 << self.shift,
                self.margin,
                if self.expire == 0 { "kept       ".into() } else { format!("forget {:>3}min", self.expire as u32 * LINK_STATUS_MS / 60_000) },
                if f == Floor::Lqi { "max LQI" } else { "       " }
            ),
        };
        base + &explore
    }
}

struct LinkEst {
    algo: Algo,
    est: Candidate,
    lqi_cost: u8,
    out_cost: u8,
    sampled: bool,
    quiet: u8,
    truth: u8,
}

impl LinkEst {
    fn fresh(algo: Algo) -> Candidate {
        Candidate::with(Params { shift: algo.shift, margin: algo.margin, warmup: 16, first_only: false })
    }

    fn cost(&self) -> u8 {
        let base = self.lqi_cost.max(self.out_cost);
        match self.algo.floor {
            Floor::Oracle => self.truth,
            Floor::LqiOnly => base,
            _ if !self.est.measured() => base,
            Floor::Measured => self.est.cost(),
            Floor::Lqi => self.est.cost().max(self.lqi_cost),
        }
    }

    fn sample(&mut self, attempts: u8, ok: bool) {
        self.est.sample(attempts, ok);
        self.sampled = true;
    }

    fn tick(&mut self) {
        if self.algo.expire == 0 {
            return;
        }
        if self.sampled {
            self.quiet = 0;
        } else {
            self.quiet = self.quiet.saturating_add(1);
            if self.quiet >= self.algo.expire && self.est.measured() {
                self.est = Self::fresh(self.algo);
            }
        }
        self.sampled = false;
    }
}

struct Channel {
    bits: Vec<bool>,
    at: usize,
    left: usize,
}

impl Channel {
    fn attempt(&mut self, rng: &mut Rng, mult: f64) -> bool {
        if self.left == 0 {
            self.at = (rng.f() * self.bits.len() as f64) as usize % self.bits.len();
            self.left = BLOCK;
        }
        let b = self.bits[self.at];
        self.at = (self.at + 1) % self.bits.len();
        self.left -= 1;
        b && rng.f() < mult
    }
}

fn bits(trace: &str) -> Vec<bool> {
    let mut v = Vec::new();
    for c in trace.chars() {
        match c {
            'x' => v.extend([false; MAC_ATTEMPTS as usize]),
            d => {
                let n = d.to_digit(10).unwrap() as usize;
                v.extend(core::iter::repeat_n(false, n - 1));
                v.push(true);
            }
        }
    }
    v
}

fn median(v: &[u16]) -> u16 {
    let mut s = v.to_vec();
    s.sort();
    s[s.len() / 2]
}

#[derive(Default, Clone, Copy)]
pub(super) struct Outcome {
    pub frames: u64,
    pub lost: u64,
    pub attempts: u64,
    pub changes: u64,
    pub hops: u64,
    pub hours: f64,
    pub stuck: u64,
    pub mtorrs: u64,
    pub direct: u64,
}

impl Outcome {
    pub(super) fn row(&self, name: &str) {
        println!(
            "{name:58} lost {:>6.3}%  attempts/frame {:>5.3}  hops {:>4.2}  route changes/h {:>5.1}  off the oracle {:>5.1}%  direct {:>4.2}%",
            100.0 * self.lost as f64 / self.frames as f64,
            self.attempts as f64 / (self.frames - self.lost).max(1) as f64,
            self.hops as f64 / self.frames as f64,
            self.changes as f64 / self.hours,
            100.0 * self.stuck as f64 / self.mtorrs.max(1) as f64,
            100.0 * self.direct as f64 / self.frames.max(1) as f64,
        );
    }
    pub(super) fn add(&mut self, o: &Outcome) {
        self.frames += o.frames;
        self.lost += o.lost;
        self.attempts += o.attempts;
        self.changes += o.changes;
        self.hops += o.hops;
        self.hours += o.hours;
        self.stuck += o.stuck;
        self.mtorrs += o.mtorrs;
        self.direct += o.direct;
    }
}

struct Home {
    usable: [[bool; NODES]; NODES],
    channel: Vec<Vec<Option<Channel>>>,
    est: Vec<Vec<Option<LinkEst>>>,
    windows: Vec<(Vec<u16>, u16)>,
}

fn true_p(a: usize, b: usize, pooled: &[bool]) -> f64 {
    let own: Vec<bool> = TRACES.iter().filter(|t| t.0 == a && t.1 == b).flat_map(|t| bits(t.2)).collect();
    let v = if own.is_empty() { pooled } else { &own[..] };
    v.iter().filter(|&&x| x).count() as f64 / v.len() as f64
}

impl Home {
    fn new(algo: Algo) -> Home {
        let pooled: Vec<bool> = TRACES.iter().flat_map(|t| bits(t.2)).collect();
        let mut usable = [[false; NODES]; NODES];
        let mut channel: Vec<Vec<Option<Channel>>> = (0..NODES).map(|_| (0..NODES).map(|_| None).collect()).collect();
        let mut est: Vec<Vec<Option<LinkEst>>> = (0..NODES).map(|_| (0..NODES).map(|_| None).collect()).collect();
        let pooled_windows: Vec<u16> = WINDOWS.iter().flat_map(|w| w.1.iter().copied()).collect();
        let windows: Vec<(Vec<u16>, u16)> = (0..NODES)
            .map(|n| {
                let w = WINDOWS.iter().find(|w| w.0 == n).map(|w| w.1.to_vec()).unwrap_or_else(|| pooled_windows.clone());
                let m = median(&w);
                (w, m)
            })
            .collect();
        for &(a, b, lqi, out_cost, _) in HEARD {
            if out_cost == 0 {
                continue;
            }
            usable[a][b] = true;
            let own: Vec<bool> = TRACES.iter().filter(|t| t.0 == a && t.1 == b).flat_map(|t| bits(t.2)).collect();
            let back: Vec<bool> = TRACES.iter().filter(|t| t.0 == b && t.1 == a).flat_map(|t| bits(t.2)).collect();
            let src = if !own.is_empty() {
                own
            } else if !back.is_empty() {
                back
            } else {
                pooled.clone()
            };
            channel[a][b] = Some(Channel { bits: src, at: 0, left: 0 });
            est[a][b] = Some(LinkEst {
                algo,
                est: LinkEst::fresh(algo),
                lqi_cost: nwk::cost_from_lqi(lqi),
                out_cost,
                sampled: false,
                quiet: 0,
                truth: ideal(true_p(a, b, &pooled)),
            });
        }
        Home { usable, channel, est, windows }
    }

    fn hop(&mut self, rng: &mut Rng, a: usize, b: usize, mult: f64, o: &mut Outcome) -> bool {
        let ch = self.channel[a][b].as_mut().unwrap();
        let mut n = 0u8;
        let mut ok = false;
        while n < MAC_ATTEMPTS && !ok {
            n += 1;
            ok = ch.attempt(rng, mult);
        }
        o.attempts += n as u64;
        self.est[a][b].as_mut().unwrap().sample(n, ok);
        ok
    }

    fn routes(&self, oracle: bool) -> [Option<usize>; NODES] {
        let mut pc = [(u32::MAX, 0usize); NODES];
        let mut next = [None; NODES];
        pc[0] = (0, 0);
        for _ in 0..NODES {
            for a in 1..NODES {
                for b in 0..NODES {
                    if !self.usable[a][b] || pc[b].0 == u32::MAX {
                        continue;
                    }
                    let e = self.est[a][b].as_ref().unwrap();
                    let c = if oracle { e.truth } else { e.cost() } as u32;
                    let cand = (pc[b].0 + c, pc[b].1 + 1);
                    if cand < pc[a] {
                        pc[a] = cand;
                        next[a] = Some(b);
                    }
                }
            }
        }
        next
    }
}

pub(super) fn simulate(algo: Algo, seed: u64, hours: f64, fades: bool) -> Outcome {
    let mut rng = Rng(seed.wrapping_mul(0x9E3779B97F4A7C15) | 1);
    let mut h = Home::new(algo);
    let mut o = Outcome { hours, ..Outcome::default() };
    let end = (hours * 3_600_000.0) as u64;
    let mut route = [None; NODES];
    let mut window_at = [0usize; NODES];
    for (n, w) in window_at.iter_mut().enumerate() {
        *w = (rng.f() * h.windows[n].0.len() as f64) as usize + n;
    }
    let mut mult = [1.0f64; NODES];
    let mut fade = [[0u32; NODES]; NODES];
    let mut explored = [0u64; NODES];
    let explore_ms = algo.explore_s as u64 * 1000;
    let mut t = 0u64;
    while t < end {
        if t % WINDOW_MS == 0 {
            for n in 1..NODES {
                let (w, m) = &h.windows[n];
                let r = w[window_at[n] % w.len()] as f64;
                window_at[n] += 1;
                mult[n] = ((1000.0 + *m as f64) / (1000.0 + r)).min(1.0);
            }
            for a in 0..NODES {
                for b in a + 1..NODES {
                    if fade[a][b] > 0 {
                        fade[a][b] -= 1;
                    } else if fades && h.usable[a][b] && rng.f() < FADE_PER_WINDOW {
                        fade[a][b] = 3 + (rng.f() * 12.0) as u32;
                    }
                    fade[b][a] = fade[a][b];
                }
            }
        }
        if t % MTORR_MS == 0 {
            let new = h.routes(false);
            let best = h.routes(true);
            for n in 1..NODES {
                if t > 0 && new[n] != route[n] {
                    o.changes += 1;
                }
                o.mtorrs += 1;
                o.stuck += (new[n] != best[n]) as u64;
            }
            route = new;
        }
        if t % LINK_STATUS_MS as u64 == 0 {
            for row in h.est.iter_mut() {
                for e in row.iter_mut().flatten() {
                    e.tick();
                }
            }
        }
        for src in 1..NODES {
            if rng.f() >= SENT_PER_S[src] / 2.0 * TICK_MS as f64 / 1000.0 {
                continue;
            }
            o.frames += 1;
            let depth = |a: usize, b: usize| if fade[a][b] > 0 { FADE_DEPTH } else { 1.0 };
            let mut at = src;
            let mut hops = 0;
            let mut delivered = false;
            if explore_ms > 0 && route[src] != Some(0) && h.usable[src][0] && t >= explored[src] + explore_ms {
                explored[src] = t;
                o.direct += 1;
                hops += 1;
                delivered = h.hop(&mut rng, src, 0, mult[src] * depth(src, 0), &mut o);
            }
            while hops < MAX_HOPS && !delivered {
                let Some(b) = route[at] else { break };
                hops += 1;
                if !h.hop(&mut rng, at, b, mult[at] * depth(at, b), &mut o) {
                    break;
                }
                if b == 0 {
                    delivered = true;
                    break;
                }
                at = b;
            }
            o.hops += hops as u64;
            o.lost += !delivered as u64;
        }
        t += TICK_MS;
    }
    o
}

pub(super) fn table(algos: &[Algo], seeds: u64, hours: f64, fades: bool) -> Vec<(Algo, Outcome)> {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let jobs: Vec<(usize, u64)> = (0..algos.len()).flat_map(|i| (1..=seeds).map(move |s| (i, s))).collect();
    let chunk = jobs.len().div_ceil(threads);
    let parts: Vec<Vec<(usize, Outcome)>> = std::thread::scope(|sc| {
        let hs: Vec<_> = jobs
            .chunks(chunk)
            .map(|part| sc.spawn(move || part.iter().map(|&(i, s)| (i, simulate(algos[i], s, hours, fades))).collect::<Vec<_>>()))
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let mut sum = vec![Outcome::default(); algos.len()];
    for (i, o) in parts.iter().flatten() {
        sum[*i].add(o);
    }
    algos.iter().copied().zip(sum).collect()
}
