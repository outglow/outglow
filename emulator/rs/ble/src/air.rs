use std::collections::BTreeSet;

use crate::pyrandom::PyRandom;
use crate::round3;

pub const MILLION: i64 = 1_000_000;

pub const TO_KEYBOARD: usize = 0;
pub const TO_HOST: usize = 1;

pub const WAY_NAMES: [&str; 2] = ["keyboard", "host"];

#[derive(Clone, Debug, PartialEq)]
pub struct AirLoss {
    pub ms: f64,
    pub way: usize,
    pub ch: u8,
}

pub struct Air {
    pub seed: u32,
    pub loss: [i64; 2],
    pub bad: BTreeSet<u8>,
    pub bad_loss: i64,
    pub clock_ppm: f64,
    pub jitter_us: i64,
    pub blackout_until: f64,
    pub(crate) rng: [PyRandom; 2],
    jit: PyRandom,
    pub sent: [i64; 2],
    pub lost_n: [i64; 2],
    pub losses: Vec<AirLoss>,
}

pub fn millionths(p: f64) -> Result<i64, String> {
    let n = (p * MILLION as f64).round_ties_even() as i64;
    if n < 0 || n > MILLION {
        return Err(format!("probability {}: not in 0..1", crate::gofmt::g(p)));
    }
    Ok(n)
}

impl Air {
    pub fn new(seed: u32, loss_keyboard: i64, loss_host: i64, bad: &[u8], bad_loss: i64, clock_ppm: f64, jitter_us: i64) -> Result<Air, String> {
        for (name, n) in [("loss_keyboard", loss_keyboard), ("loss_host", loss_host), ("bad_loss", bad_loss)] {
            if n < 0 || n > MILLION {
                return Err(format!("{}: {} is not 0..{} millionths", name, n, MILLION));
            }
        }
        let mut set = BTreeSet::new();
        for &ch in bad {
            if ch > 39 {
                return Err(format!("bad: a channel index outside 0..39 ({})", ch));
            }
            set.insert(ch);
        }
        if jitter_us < 0 {
            return Err(format!("jitter_us: {}", jitter_us));
        }
        Ok(Air {
            seed,
            loss: [loss_keyboard, loss_host],
            bad: set,
            bad_loss,
            clock_ppm,
            jitter_us,
            blackout_until: 0.0,
            rng: [PyRandom::new(seed.wrapping_mul(4)), PyRandom::new(seed.wrapping_mul(4).wrapping_add(1))],
            jit: PyRandom::new(seed.wrapping_mul(4).wrapping_add(2)),
            sent: [0; 2],
            lost_n: [0; 2],
            losses: Vec::new(),
        })
    }

    pub fn clock(&self) -> f64 {
        1.0 + self.clock_ppm / MILLION as f64
    }

    pub fn lost(&mut self, way: usize, ch: u8, now_ms: f64) -> bool {
        self.sent[way] += 1;
        let mut p = self.loss[way];
        if self.bad.contains(&ch) && self.bad_loss > p {
            p = self.bad_loss;
        }
        let gone = if now_ms < self.blackout_until {
            true
        } else if p <= 0 {
            false
        } else {
            (self.rng[way].randrange(MILLION as u32) as i64) < p
        };
        if gone {
            self.lost_n[way] += 1;
            self.losses.push(AirLoss { ms: round3(now_ms), way, ch });
        }
        gone
    }

    pub fn jitter_ms(&mut self) -> f64 {
        if self.jitter_us <= 0 {
            return 0.0;
        }
        ((self.jit.randrange((2 * self.jitter_us + 1) as u32) as i64) - self.jitter_us) as f64 / 1000.0
    }

    pub fn summary(&self) -> String {
        format!(
            "to the keyboard {} of {} packets lost, to the host {} of {}",
            self.lost_n[TO_KEYBOARD], self.sent[TO_KEYBOARD], self.lost_n[TO_HOST], self.sent[TO_HOST]
        )
    }
}
