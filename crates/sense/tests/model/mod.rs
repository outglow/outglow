//! The radio channel between bulbs, for the tests of the motion sensing,
//! taken from what four bulbs on the Rust firmware in a corridor measured
//! on 2026-11-14, read through Zigbee2MQTT (the Diagnostics counters of
//! `outglow diag` about 20 minutes after the start and again 3.5 hours
//! later, and one bulb's black box, `outglow box`, for an hour between).
//!
//! What the bulbs measure and what they do not:
//!
//! - per neighbour, the RSSI of the frames heard from it as a running average
//!   (`rssi_avg`, 1/4 of every new frame, the division rounding towards
//!   zero, which puts it up to 0.75 dB above the mean) and the lowest RSSI
//!   since the neighbour was added (`rssi_min`), with the number of frames
//!   (`rx`); per attempt, the share of our unicasts to it that got their
//!   acknowledgement (`delivery`); the noise floor, now and its highest;
//! - not the RSSI of single frames over time. So the spread of a link's RSSI
//!   is known only through how far its lowest frame lies below its average,
//!   with every dip in it: people walking along the corridor, doors. Taken
//!   as Gaussian noise that gives an upper bound (`sigma_hi`, the largest of
//!   a link's readings; the smallest is `sigma_lo`). How long a dip lasts,
//!   how deep a person makes it and how the noise is correlated in time is
//!   not measured at all: the crossings in the tests are an assumption
//!   (6..10 dB for about 1.5 s, the depth the C firmware's model and the
//!   device-free localisation papers use).
//!
//! The bulbs: K1, K2, K3, and D (it was off the network at the second
//! read). W1..W3 are neighbours on other firmware.

#![allow(dead_code)]

/// One neighbour table entry: the bulb `at` hears `from`.
#[derive(Clone, Copy, Debug)]
pub struct Reading {
    pub at: &'static str,
    pub from: &'static str,
    /// 'A': 12:09, about 20 minutes after the bulbs started; 'B': 15:20, 3.5 hours after.
    pub when: char,
    pub avg: i8,
    pub min: i8,
    pub frames: u32,
}

const fn r(at: &'static str, from: &'static str, when: char, avg: i8, min: i8, frames: u32) -> Reading {
    Reading { at, from, when, avg, min, frames }
}

/// The links between the four bulbs on the Rust firmware, both ways.
pub const READINGS: [Reading; 18] = [
    r("K1", "K2", 'A', -55, -61, 586),
    r("K1", "K3", 'A', -67, -75, 300),
    r("K1", "D", 'A', -56, -59, 286),
    r("K2", "K1", 'A', -55, -62, 446),
    r("K2", "K3", 'A', -56, -60, 304),
    r("K2", "D", 'A', -61, -62, 107),
    r("K3", "K1", 'A', -67, -75, 370),
    r("K3", "K2", 'A', -56, -60, 413),
    r("K3", "D", 'A', -73, -76, 113),
    r("D", "K1", 'A', -56, -59, 228),
    r("D", "K2", 'A', -61, -65, 136),
    r("D", "K3", 'A', -73, -78, 97),
    r("K1", "K2", 'B', -59, -62, 4072),
    r("K1", "K3", 'B', -69, -76, 2815),
    r("K2", "K1", 'B', -58, -62, 3568),
    r("K2", "K3", 'B', -56, -60, 2831),
    r("K3", "K1", 'B', -68, -79, 3436),
    r("K3", "K2", 'B', -55, -60, 3841),
];

/// Weak links, to bulbs without sensing, near the detector's floor of
/// -90 dBm: what a line looks like whose frames are often too weak to count.
pub const WEAK: [Reading; 4] = [
    r("K2", "W1", 'B', -83, -97, 2660),
    r("K1", "W2", 'B', -84, -95, 1644),
    r("K3", "W1", 'B', -81, -95, 2639),
    r("K2", "W3", 'B', -86, -96, 1404),
];

/// Per attempt delivery of our unicasts to a neighbour (a frame and its
/// acknowledgement), percent: (from, to, when, delivery, transmissions).
pub const DELIVERY: [(&str, &str, char, u8, u32); 10] = [
    ("K1", "K2", 'A', 94, 86),
    ("K1", "D", 'A', 91, 89),
    ("K2", "K1", 'A', 100, 202),
    ("D", "K1", 'A', 99, 206),
    ("K1", "K2", 'B', 98, 255),
    ("K1", "K3", 'B', 100, 154),
    ("K2", "K1", 'B', 90, 255),
    ("K2", "K3", 'B', 93, 214),
    ("K3", "K1", 'B', 98, 30),
    ("K3", "K2", 'B', 100, 25),
];

/// Noise floor, dBm, now and its highest since the start: (bulb, when, now, max).
/// The black box of D (a row every 2 minutes for an hour) has -103..-105.
pub const NOISE: [(&str, char, i8, i8); 7] = [
    ("K1", 'A', -101, -99),
    ("K2", 'A', -104, -103),
    ("K3", 'A', -103, -102),
    ("D", 'A', -104, -102),
    ("K1", 'B', -97, -79),
    ("K2", 'B', -104, -92),
    ("K3", 'B', -102, -71),
];

/// Ordinary frames heard from one neighbour, per second (read B: the
/// frames of a Rust neighbour over the bulb's uptime, 12684..12931 s): link
/// status, relayed broadcasts, routing. They are samples too.
pub const FRAMES_PER_S: f64 = 0.30;
/// The largest change of a link's average between the two reads (3.2 h
/// apart): the level drifts by that much over hours.
pub const DRIFT_DB: f64 = 4.0;

/// -E[min of n standard normal samples], Blom's approximation.
pub fn expected_min_sigmas(n: u32) -> f64 {
    -inverse_normal((1.0 - 0.375) / (n as f64 + 0.25))
}

/// The quantile function of the standard normal distribution (Acklam's
/// rational approximation, relative error below 1.2e-9).
pub fn inverse_normal(p: f64) -> f64 {
    const A: [f64; 6] = [-3.969683028665376e+01, 2.209460984245205e+02, -2.759285104469687e+02, 1.383577518672690e+02, -3.066479806614716e+01, 2.506628277459239e+00];
    const B: [f64; 5] = [-5.447609879822406e+01, 1.615858368580409e+02, -1.556989798598866e+02, 6.680131188771972e+01, -1.328068155288572e+01];
    const C: [f64; 6] = [-7.784894002430293e-03, -3.223964580411365e-01, -2.400758277161838e+00, -2.549732539343734e+00, 4.374664141464968e+00, 2.938163982698783e+00];
    const D: [f64; 4] = [7.784695709041462e-03, 3.224671290700398e-01, 2.445134137142996e+00, 3.754408661907416e+00];
    let lo = 0.02425;
    if p < lo {
        let q = (-2.0 * p.ln()).sqrt();
        (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5]) / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    } else if p <= 1.0 - lo {
        let q = p - 0.5;
        let r = q * q;
        (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q
            / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
    } else {
        -inverse_normal(1.0 - p)
    }
}

impl Reading {
    /// The spread this reading allows if all of `avg - min` were Gaussian noise.
    pub fn sigma(&self) -> f64 {
        (self.avg as f64 - self.min as f64) / expected_min_sigmas(self.frames)
    }
}

/// A link between two bulbs as the tests use it.
#[derive(Clone, Copy, Debug)]
pub struct Link {
    pub a: &'static str,
    pub b: &'static str,
    /// Mean of the averages, both ways, both reads [dBm].
    pub level: f64,
    pub sigma_lo: f64,
    pub sigma_hi: f64,
    /// Share of the probes lost, one way: 1 - the worst delivery measured
    /// on the link (which counts the acknowledgement as well, so it is a bound).
    pub loss: f64,
}

fn same(r: &Reading, a: &str, b: &str) -> bool {
    (r.at == a && r.from == b) || (r.at == b && r.from == a)
}

/// The link between two bulbs from the readings.
pub fn link(a: &'static str, b: &'static str) -> Link {
    link_from(&READINGS, a, b)
}

pub fn link_from(readings: &[Reading], a: &'static str, b: &'static str) -> Link {
    let rs: Vec<&Reading> = readings.iter().filter(|r| same(r, a, b)).collect();
    assert!(!rs.is_empty(), "no readings for {a}-{b}");
    let level = rs.iter().map(|r| r.avg as f64).sum::<f64>() / rs.len() as f64;
    let sigmas: Vec<f64> = rs.iter().map(|r| r.sigma()).collect();
    let sigma_lo = sigmas.iter().cloned().fold(f64::INFINITY, f64::min);
    let sigma_hi = sigmas.iter().cloned().fold(0.0, f64::max);
    let worst = DELIVERY.iter().filter(|d| (d.0 == a && d.1 == b) || (d.0 == b && d.1 == a)).map(|d| d.3).min();
    // a link nobody sent unicasts over: the worst delivery seen between these bulbs
    let worst = worst.unwrap_or_else(|| DELIVERY.iter().map(|d| d.3).min().unwrap());
    Link { a, b, level, sigma_lo, sigma_hi, loss: 1.0 - worst as f64 / 100.0 }
}

/// The links between the bulbs with sensing, as the tests run them.
pub fn links() -> Vec<Link> {
    [("K1", "K2"), ("K1", "K3"), ("K2", "K3"), ("K1", "D"), ("K2", "D"), ("K3", "D")].iter().map(|(a, b)| link(a, b)).collect()
}

/// splitmix64 with a Gaussian: the tests need varied, repeatable numbers.
#[derive(Clone)]
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    pub fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    pub fn gauss(&mut self, sigma: f64) -> f64 {
        let u = self.unit().max(1e-300);
        let v = self.unit();
        (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos() * sigma
    }
}

/// A person crossing a line: from `t0` to `t1` [ms] the level is `depth` dB lower
/// (each frame 0.6..1.4 times that, as in the C firmware's model).
#[derive(Clone, Copy, Debug)]
pub struct Crossing {
    pub t0: u32,
    pub t1: u32,
    pub depth: f64,
}

/// One line's channel: what a receiver measures of one sender's frames.
#[derive(Clone, Debug)]
pub struct Channel {
    pub level: f64,
    pub sigma: f64,
    pub loss: f64,
    /// Added to the level: a step (a light change of the sender, a door) or a drift.
    pub offset: f64,
    /// dB per hour the level drifts by, from `drift_from`.
    pub drift_per_h: f64,
    pub drift_from: u32,
    pub crossings: Vec<Crossing>,
    /// Interference: with this probability per frame a burst at `burst_dbm`
    /// adds its power to the frame's (the highest noise floors measured).
    pub burst_p: f64,
    pub burst_dbm: f64,
}

impl Channel {
    pub fn new(level: f64, sigma: f64, loss: f64) -> Channel {
        Channel { level, sigma, loss, offset: 0.0, drift_per_h: 0.0, drift_from: 0, crossings: vec![], burst_p: 0.0, burst_dbm: -120.0 }
    }

    pub fn of(link: &Link, sigma: f64) -> Channel {
        Channel::new(link.level, sigma, link.loss)
    }

    /// The RSSI of a frame at `t` [ms], whole dB as the radio gives it, or `None` if it is lost.
    pub fn rssi(&mut self, g: &mut Rng, t: u32) -> Option<i8> {
        if g.unit() < self.loss {
            return None;
        }
        let drift = self.drift_per_h * t.saturating_sub(self.drift_from) as f64 / 3.6e6;
        let mut x = self.level + self.offset + drift + g.gauss(self.sigma);
        for c in &self.crossings {
            if c.t0 <= t && t < c.t1 {
                x -= c.depth * (0.6 + 0.8 * g.unit());
            }
        }
        if self.burst_p > 0.0 && g.unit() < self.burst_p {
            x = 10.0 * (10f64.powf(x / 10.0) + 10f64.powf(self.burst_dbm / 10.0)).log10();
        }
        Some(x.round().clamp(-110.0, -15.0) as i8)
    }
}
