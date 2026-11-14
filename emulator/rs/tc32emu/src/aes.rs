// SPDX-License-Identifier: Apache-2.0
//! The AES block (aes.go).

use crate::aes128;
use crate::machine::Machine;

impl Machine {
    pub(crate) fn aes_write(&mut self, o: usize, val: u32) {
        if o == 0x540 {
            let ccm = val & 0x80 != 0;
            if ccm && !self.aes_ccm {
                self.aes_prev = [0; 16];
            }
            self.aes_ccm = ccm;
            self.regs[0x540] = (val & 0x01) as u8 | 0x02 | (val & 0x80) as u8;
            self.aes_in = Vec::new();
            self.aes_out = Vec::new();
            return;
        }
        self.aes_in.extend_from_slice(&val.to_le_bytes());
        if self.aes_in.len() == 16 {
            let key: Vec<u8> = self.regs[0x550..0x560].to_vec();
            let mut inp = self.aes_in.clone();
            if self.aes_ccm {
                for (i, b) in inp.iter_mut().enumerate() {
                    *b ^= self.aes_prev[i];
                }
            }
            let out = if self.regs[0x540] & 1 != 0 { aes128::decrypt(&key, &inp) } else { aes128::encrypt(&key, &inp) };
            self.aes_out = out.to_vec();
            self.aes_prev.copy_from_slice(&out);
            self.aes_in = Vec::new();
            self.regs[0x540] = (self.regs[0x540] & 0x81) | 0x04;
        }
    }

    pub(crate) fn aes_read(&mut self) -> u32 {
        let mut w = [0u8; 4];
        let n = self.aes_out.len().min(4);
        w[..n].copy_from_slice(&self.aes_out[..n]);
        if self.aes_out.len() > 4 {
            self.aes_out.drain(..4);
        } else {
            self.aes_out = Vec::new();
        }
        u32::from_le_bytes(w)
    }
}
