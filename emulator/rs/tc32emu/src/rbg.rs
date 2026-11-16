use crate::error::{emu_err, Error};
use crate::machine::{Machine, M32};

pub const RBG_MODES: &[&str] = &["lcg", "healthy", "stuck", "biased", "cycle", "attacker", "never"];

pub(crate) const K32_PERIOD_MS: f64 = 1000.0 / 32768.0;
const K32_JITTER_SPAN: i64 = 4096;
const K32_SUM4_SD: f64 = 37837.2264;

#[inline]
pub(crate) fn splitmix64(state: u64) -> (u64, u64) {
    let state = state.wrapping_add(0x9E3779B97F4A7C15);
    let mut z = state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    (state, z ^ (z >> 31))
}

impl Machine {
    pub(crate) fn rbg_set(&mut self, mode: usize, param: u32) {
        self.rbg_mode = mode;
        self.rbg_param = param;
        self.rbg_state = param as u64;
        self.rbg_index = 0;
        self.rbg_cycle = Vec::new();
        if mode == 4 {
            let mut n = param as usize;
            if n == 0 {
                n = 8;
            }
            let mut s = 0u64;
            for _ in 0..n {
                let (ns, z) = splitmix64(s);
                s = ns;
                self.rbg_cycle.push((z >> 32) as u32);
            }
        }
    }

    pub(crate) fn rbg_ready(&self) -> bool {
        let r = &self.regs;
        if self.rbg_mode == 0 {
            return true;
        }
        self.rbg_mode != 6 && r[0x62] & 0x08 == 0 && r[0x65] & 0x08 != 0 && r[0x4400] & 0x01 != 0
    }

    pub(crate) fn rbg_word(&mut self) -> u32 {
        if self.rbg_mode == 0 {
            self.rng = self.rng.wrapping_mul(1103515245).wrapping_add(12345);
            return self.rng;
        }
        if !self.rbg_ready() {
            return 0;
        }
        if self.rbg_mode == 2 {
            return 0x5A5A5A5A ^ self.rbg_param;
        }
        if self.rbg_mode == 3 {
            let mut p = self.rbg_param as u64;
            if p == 0 {
                p = 243;
            }
            let mut w = 0u32;
            for i in 0..4 {
                let (s, z) = splitmix64(self.rbg_state);
                self.rbg_state = s;
                for j in 0..8 {
                    if (z >> (8 * j)) & 0xFF < p {
                        w |= 1 << (8 * i + j);
                    }
                }
            }
            return w;
        }
        if self.rbg_mode == 4 {
            let w = self.rbg_cycle[self.rbg_index % self.rbg_cycle.len()];
            self.rbg_index += 1;
            return w;
        }
        let (s, z) = splitmix64(self.rbg_state);
        self.rbg_state = s;
        (z >> 32) as u32
    }

    fn k32_jitter_ms(&mut self) -> f64 {
        let (s, z) = splitmix64(self.k32_rng);
        self.k32_rng = s;
        let s = (z & 0xFFFF) as i64 + ((z >> 16) & 0xFFFF) as i64 + ((z >> 32) & 0xFFFF) as i64 + (z >> 48) as i64 - 131070;
        s as f64 * self.k32_sigma_ns as f64 / K32_SUM4_SD * 1e-6
    }

    fn k32_set_jitter(&mut self, sigma_ns: i64) {
        if sigma_ns != 0 && self.k32_sigma_ns == 0 {
            self.k32_count = (self.ms() * 32.768) as i64;
            self.k32_next = (self.k32_count + 1) as f64 * K32_PERIOD_MS;
        }
        self.k32_sigma_ns = sigma_ns;
    }

    pub(crate) fn k32_jittered(&mut self) -> u32 {
        let t = self.ms();
        if t >= self.k32_next {
            let n = ((t - self.k32_next) / K32_PERIOD_MS) as i64;
            if n > K32_JITTER_SPAN {
                let skip = n - K32_JITTER_SPAN;
                self.k32_count += skip;
                self.k32_next = (skip as f64).mul_add(K32_PERIOD_MS, self.k32_next);
            }
            while t >= self.k32_next {
                self.k32_count += 1;
                let j = self.k32_jitter_ms();
                self.k32_next += K32_PERIOD_MS + j;
            }
        }
        (self.k32_count & M32) as u32
    }

    pub(crate) fn rbg_control(&mut self, o: usize, val: u32) -> Result<(), Error> {
        match o {
            0xFFF4 => {
                if (val & 0xFF) as usize >= RBG_MODES.len() {
                    return Err(emu_err(format!("0xfff4: no random number source {}", val & 0xFF)));
                }
                self.rbg_set((val & 0xFF) as usize, val >> 8);
                if self.has_event() {
                    let text = format!(
                        "random number source: {}, parameter {}",
                        RBG_MODES[(val & 0xFF) as usize],
                        val >> 8
                    );
                    self.event(&text);
                }
            }
            0xFFF8 => {
                self.k32_set_jitter((val & 0x7FFFFFFF) as i64);
                self.k32_stopped = None;
                let mut text = format!("32 kHz jitter: {} ns", val & 0x7FFFFFFF);
                if val >> 31 != 0 {
                    let c = self.k32_now();
                    self.k32_stopped = Some(c);
                    text.push_str(", count stopped");
                }
                self.event(&text);
            }
            _ => self.k32_rng = val as u64,
        }
        self.store(o, 4, val);
        Ok(())
    }
}
