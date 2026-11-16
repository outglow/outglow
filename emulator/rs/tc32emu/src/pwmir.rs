use crate::machine::Machine;

const PWM_FIFO_DEPTH: usize = 8;

impl Machine {
    fn pwm_fifo_mode(&self) -> bool {
        let n = self.regs[0x783] & 0x0F;
        n == 0x7 || n == 0xF
    }

    pub(crate) fn pwm_fifo_push(&mut self, v: u32) {
        if self.pwm_fifo.len() < PWM_FIFO_DEPTH {
            self.pwm_fifo.push(v);
        }
    }

    pub(crate) fn pwm_fifo_sr(&self) -> u32 {
        let n = self.pwm_fifo.len();
        let mut v = (n & 0x0F) as u32;
        if n == 0 {
            v |= 0x10;
        }
        if n >= PWM_FIFO_DEPTH {
            v |= 0x20;
        }
        v
    }

    pub(crate) fn pwm_fifo_drain(&mut self) {
        if !self.pwm_fifo_mode() || !self.pwm_enabled(0) || self.pwm_fifo.is_empty() {
            return;
        }
        self.pwm_fifo.clear();
        self.regs[0x7B3] |= 0x01;
        self.pwm_ir_done = true;
    }
}
