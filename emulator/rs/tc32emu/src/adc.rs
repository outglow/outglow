// SPDX-License-Identifier: Apache-2.0
//! The ADC on the VBAT input and its DMA (DFIFO2) (adc.go).

use crate::error::{emu_err, Error};
use crate::machine::{Machine, SRAM_BASE};
use crate::rbg::splitmix64;

/// ADCModes: the VBAT conversions TC32EMU_ADC names and register 0xffe8 numbers.
pub const ADC_MODES: &[&str] = &["noise", "off", "stuck", "biased", "ramp", "two", "attacker", "never", "level"];

/// 3.3 V through the 1/3 divider against the 1175 mV reference.
pub const ADC_VBAT_CODE: i64 = 0x1DF5;
const ADC_SD_X10: i64 = 378372;

/// Python's // for a positive divisor.
fn floor_div(a: i64, b: i64) -> i64 {
    let mut q = a / b;
    if a % b != 0 && a < 0 {
        q -= 1;
    }
    q
}

fn or(v: i64, d: i64) -> i64 {
    if v == 0 {
        d
    } else {
        v
    }
}

impl Machine {
    pub(crate) fn adc_set(&mut self, mode: usize, param: u32) {
        self.adc_mode = mode;
        self.adc_param = param;
        self.adc_state = param as u64;
        self.adc_index = 0;
    }

    fn adc_noise(&mut self, sigma_x10: i64) -> i64 {
        let (s, z) = splitmix64(self.adc_state);
        self.adc_state = s;
        let s = (z & 0xFFFF) as i64 + ((z >> 16) & 0xFFFF) as i64 + ((z >> 32) & 0xFFFF) as i64 + (z >> 48) as i64 - 131070;
        floor_div(2 * s * sigma_x10 + ADC_SD_X10, 2 * ADC_SD_X10)
    }

    pub(crate) fn adc_next(&mut self) -> u32 {
        let (p, code) = (self.adc_param as i64, ADC_VBAT_CODE);
        let k = self.adc_index;
        self.adc_index += 1;
        match self.adc_mode {
            1 => return code as u32,
            2 => return or(p & 0x1FFF, 0x1555) as u32,
            4 => return ((code + k / or(p, 1) - 1) % 0x1FFF + 1) as u32,
            0 | 8 => {
                let mut v = if self.adc_mode == 8 {
                    or(p & 0x1FFF, code) + self.adc_noise(20)
                } else {
                    code + self.adc_noise(or(p & 0xFF, 20))
                };
                if v < 0 {
                    v = 0;
                }
                if v > 0x1FFF {
                    v = 0x1FFF;
                }
                return v as u32;
            }
            _ => {}
        }
        let (s, z) = splitmix64(self.adc_state);
        self.adc_state = s;
        match self.adc_mode {
            3 => {
                if ((z >> 56) as i64) < or(p, 16) {
                    return (code + self.adc_noise(20)) as u32;
                }
                return code as u32;
            }
            5 => return ((code + or(p, 1) * (z >> 63) as i64) & 0x1FFF) as u32,
            _ => {}
        }
        (code + (z & 63) as i64 - 32) as u32
    }

    pub(crate) fn dfifo2_fill(&mut self) {
        let mut hi = self.regs[0xB0B] as i64;
        if hi == 0 {
            hi = 0x04;
        }
        let addr = 0x800000 | hi << 16 | self.regs[0xB08] as i64 | (self.regs[0xB09] as i64) << 8;
        let size = (self.regs[0xB0A] as i64 + 1) * 16;
        let o = addr - SRAM_BASE as i64;
        if o < 0 || o + size > self.cfg.sram_size as i64 {
            return;
        }
        let (o, size) = (o as usize, size as usize);
        if self.analog[0xEB] >> 4 != 0xF {
            self.adc_on = false;
            let code = match self.adc_pin_codes.get(&((self.analog[0xEB] >> 4) as i64)) {
                Some(&c) => c,
                None => self.adc_code,
            };
            let sample = code.to_le_bytes();
            let mut i = 0;
            while i + 4 <= size {
                self.sram[o + i..o + i + 4].copy_from_slice(&sample);
                i += 4;
            }
            return;
        }
        if !self.adc_on {
            let a = &self.analog;
            let mut states = (a[0xEF] as i64 | ((a[0xF1] >> 6) as i64) << 8) + (a[0xF1] & 0x0F) as i64;
            if states == 0 {
                states = 250;
            }
            self.adc_on = true;
            self.adc_t0 = self.cycles;
            self.adc_done = 0;
            self.adc_period = states * self.cpu_hz / self.cfg.adc_state_hz;
            if self.adc_period < 1 {
                self.adc_period = 1;
            }
        }
        let due = (self.cycles - self.adc_t0) / self.adc_period;
        if self.adc_mode == 7 || self.analog[0xFC] & 0x20 != 0 {
            self.adc_done = due;
            return;
        }
        let slots = (size / 4) as i64;
        if due - self.adc_done > slots {
            self.adc_done = due - slots;
        }
        while self.adc_done < due {
            let i = o + 4 * (self.adc_done % slots) as usize;
            let v = self.adc_next();
            self.sram[i..i + 4].copy_from_slice(&v.to_le_bytes());
            self.adc_done += 1;
        }
    }

    pub(crate) fn adc_pin_control(&mut self, o: usize, val: u32) -> Result<(), Error> {
        let input = ((val >> 16) & 0xFF) as i64;
        if val >> 24 == 0 {
            self.adc_pin_codes.clear();
        } else if (1..=10).contains(&input) {
            self.adc_pin_codes.insert(input, val & 0xFFFF);
        } else {
            return Err(emu_err(format!("0xffe4: no ADC pin input {}", input)));
        }
        self.store(o, 4, val);
        Ok(())
    }

    pub(crate) fn adc_control(&mut self, o: usize, val: u32) -> Result<(), Error> {
        if (val & 0xFF) as usize >= ADC_MODES.len() {
            return Err(emu_err(format!("0xffe8: no ADC mode {}", val & 0xFF)));
        }
        self.adc_set((val & 0xFF) as usize, val >> 8);
        if self.has_event() {
            let text = format!("ADC on VBAT: {}, parameter {}", ADC_MODES[(val & 0xFF) as usize], val >> 8);
            self.event(&text);
        }
        self.store(o, 4, val);
        Ok(())
    }

    pub(crate) fn adc_polled_code(&mut self) -> u32 {
        if self.analog[0xEB] >> 4 == 0xF {
            return self.adc_next() & 0x3FFF;
        }
        if let Some(&c) = self.adc_pin_codes.get(&((self.analog[0xEB] >> 4) as i64)) {
            return c & 0x3FFF;
        }
        self.adc_code & 0x3FFF
    }

    fn dfifo2_period(&self) -> i64 {
        let a = &self.analog;
        let mut states = (a[0xEF] as i64 | ((a[0xF1] >> 6) as i64) << 8) + (a[0xF1] & 0x0F) as i64;
        if states == 0 {
            states = 250;
        }
        let mut p = states * self.cpu_hz / self.cfg.adc_state_hz;
        if p < 1 {
            p = 1;
        }
        p
    }

    pub(crate) fn dfifo2_samples(&self) -> i64 {
        if !self.dfifo2_on {
            return 0;
        }
        (self.cycles - self.dfifo2_t0) / self.dfifo2_period()
    }

    pub(crate) fn dfifo2_depth(&self) -> usize {
        (self.regs[0xB0A] as usize + 1) * 16
    }

    pub(crate) fn dfifo_audio_samples(&self, on: bool, t0: i64) -> i64 {
        if !on {
            return 0;
        }
        let mut per = self.cpu_hz / 16000;
        if per < 1 {
            per = 1;
        }
        (self.cycles - t0) / per
    }
}

