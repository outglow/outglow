use crate::error::Error;
use crate::machine::{Machine, STIMER_IRQ};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub lo: u32,
    pub hi: u32,
}

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
            } else if !idle_ranges.is_empty() && self.regs[0x643] & 1 != 0 && self.ibit == 0 {
                let pc = self.r[15];
                let idle = idle_ranges.iter().any(|rg| rg.lo <= pc && pc < rg.hi);
                if idle {
                    let mut skip = 0i64;
                    if self.regs[0x748] & STIMER_IRQ != 0 {
                        skip = self.cycles_to_stimer(self.stimer_cmp);
                    }
                    if let Some(h) = self.idle_skip_hook.clone() {
                        skip = h(self, skip);
                    }
                    if let Some(m) = self.cycles_to_timer_match() {
                        if skip > 0 && m < skip {
                            skip = m;
                        }
                    }
                    let left = max_cycles - self.cycles;
                    if left < skip {
                        skip = left;
                    }
                    if let Some(mut due) = self.pad_rc_due() {
                        if due < 0 {
                            due = 0;
                        }
                        if due + 32 < skip {
                            skip = due + 32;
                        }
                    }
                    if skip > 64 {
                        self.cycles += skip - 32;
                        self.idle_skipped += skip - 32;
                    }
                }
            }
        }
        Ok(())
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
