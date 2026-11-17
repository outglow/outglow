use crate::error::Error;
use crate::machine::{Machine, STIMER_HZ, STIMER_IRQ};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub lo: u32,
    pub hi: u32,
}

const IDLE_LEAD: i64 = 32;

impl Machine {
    pub fn run(&mut self, max_cycles: i64, idle_ranges: &[Range], check_every: i64) -> Result<(), Error> {
        let check_every = if check_every <= 0 { 32 } else { check_every };
        while self.cycles < max_cycles {
            if self.asleep.is_some() {
                self.sleep_until(max_cycles);
                continue;
            }
            if self.stalled {
                self.stall_until(max_cycles)?;
                if !self.stalled && self.irq_pending() {
                    self.take_irq();
                }
                continue;
            }
            if std::mem::take(&mut self.idle_parked) && self.resume_idle(max_cycles, idle_ranges) {
                continue;
            }
            for _ in 0..check_every {
                self.step_inline()?;
            }
            if self.asleep.is_some() || self.stalled {
                continue;
            }
            self.update_time()?;
            if !self.pad_rc_since.is_empty() {
                self.pad_rc_service();
            }
            if self.irq_pending() {
                self.take_irq();
            } else if self.in_idle(idle_ranges) {
                if let Some(land) = self.idle_landing() {
                    if land - self.cycles > IDLE_LEAD {
                        self.idle_to(land, max_cycles);
                    }
                }
            }
        }
        Ok(())
    }

    fn resume_idle(&mut self, max_cycles: i64, idle_ranges: &[Range]) -> bool {
        if !self.in_idle(idle_ranges) || self.irq_pending() {
            return false;
        }
        match self.idle_landing() {
            Some(land) if land >= self.cycles => {
                self.idle_to(land, max_cycles);
                true
            }
            _ => false,
        }
    }

    pub fn idle_advance(&mut self, target_ms: f64, idle_ranges: &[Range]) -> bool {
        if !self.idle_parked || self.asleep.is_some() || self.stalled || !self.in_idle(idle_ranges) || self.irq_pending() {
            return false;
        }
        let Some(land) = self.idle_landing() else { return false };
        while self.ms() < target_ms {
            let left = (((target_ms - self.ms()) * self.cpu_hz as f64 / 1000.0) as i64 + 1).min(self.cpu_hz / 100);
            if land < self.cycles + left {
                return false;
            }
            self.idle_skipped += left;
            self.cycles += left;
        }
        true
    }

    fn in_idle(&self, idle_ranges: &[Range]) -> bool {
        if idle_ranges.is_empty() || self.regs[0x643] & 1 == 0 || self.ibit != 0 {
            return false;
        }
        let pc = self.r[15];
        idle_ranges.iter().any(|rg| rg.lo <= pc && pc < rg.hi)
    }

    fn idle_to(&mut self, land: i64, max_cycles: i64) {
        let to = if land >= max_cycles {
            self.idle_parked = true;
            max_cycles
        } else {
            land
        };
        self.idle_skipped += to - self.cycles;
        self.cycles = to;
    }

    fn idle_landing(&mut self) -> Option<i64> {
        let mut skip = 0i64;
        if self.regs[0x748] & STIMER_IRQ != 0 {
            skip = self.stimer_reached_at(self.stimer_cmp).map_or(0, |c| c - self.cycles);
        }
        if let Some(h) = self.idle_skip_hook.clone() {
            self.wake_exact = true;
            skip = h(self, skip);
            self.wake_exact = false;
        }
        if let Some(m) = self.cycles_to_timer_match() {
            if skip > 0 && m < skip {
                skip = m;
            }
        }
        if let Some(mut due) = self.pad_rc_due() {
            if due < 0 {
                due = 0;
            }
            if due + IDLE_LEAD < skip {
                skip = due + IDLE_LEAD;
            }
        }
        (skip > IDLE_LEAD).then(|| self.cycles + skip - IDLE_LEAD)
    }

    pub fn stimer_reached_at(&self, target: u32) -> Option<i64> {
        if !self.stimer_on {
            return None;
        }
        if self.stimer_stuck {
            let c = self.cycles_to_stimer(target);
            return (c > 0).then_some(self.cycles + c);
        }
        let delta = target.wrapping_sub(self.stimer_now());
        if delta >= 0x80000000 {
            return None;
        }
        let at = (target.wrapping_add(7) & !7).wrapping_sub(self.tick_base) as i128;
        let now = (self.stimer_raw().wrapping_sub(self.tick_base)) as i128;
        let e = (self.cycles - self.tick_base_cy) as i128;
        let ticks = e * STIMER_HZ as i128 / self.cpu_hz as i128;
        let goal = ticks + (at - now).rem_euclid(1 << 32);
        let hz = self.cpu_hz as i128;
        let c = (goal * hz + STIMER_HZ as i128 - 1) / STIMER_HZ as i128;
        Some(self.tick_base_cy + c as i64)
    }

    pub fn cycles_until_ms(&self, ms: f64) -> i64 {
        if self.wake_exact {
            return self.ms_base_cyc + ((ms - self.ms_base) * self.cpu_hz as f64 / 1000.0) as i64 - self.cycles;
        }
        ((ms - self.ms()) * self.cpu_hz as f64 / 1000.0) as i64
    }

    pub fn run_ms(&mut self, target_ms: f64, idle_ranges: &[Range]) -> Result<(), Error> {
        while self.ms() < target_ms {
            let mut left = ((target_ms - self.ms()) * self.cpu_hz as f64 / 1000.0) as i64 + 1;
            let c = self.cpu_hz / 100;
            if c < left {
                left = c;
            }
            self.run(self.cycles + left, idle_ranges, 32)?;
        }
        Ok(())
    }
}
