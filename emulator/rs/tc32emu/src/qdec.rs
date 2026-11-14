// SPDX-License-Identifier: Apache-2.0
//! Quadrature decoder (qdec.go).

use crate::machine::Machine;

const QDEC_PINS: [[usize; 2]; 8] = [[0, 2], [0, 3], [1, 6], [1, 7], [2, 2], [2, 3], [3, 6], [3, 7]];

/// The CW Gray sequence of state (A<<1|B): 0->1->3->2->0 (Go's cwNext map).
const CW_NEXT: [i64; 4] = [1, 3, 0, 2];

impl Machine {
    fn qdec_pin_level(&mut self, sel: usize) -> i64 {
        let p = QDEC_PINS[sel & 7];
        if self.pad_inputs(p[0]) & (1 << p[1]) != 0 {
            1
        } else {
            0
        }
    }

    pub(crate) fn qdec_sample(&mut self) {
        let a = self.qdec_pin_level((self.regs[0xD2] & 7) as usize);
        let b = self.qdec_pin_level((self.regs[0xD3] & 7) as usize);
        let st = (a << 1) | b;
        if !self.qdec_init {
            self.qdec_init = true;
            self.qdec_last = st;
            return;
        }
        let old = self.qdec_last;
        if st == old {
            return;
        }
        self.qdec_last = st;
        let d: i64 = if CW_NEXT[old as usize] == st {
            1
        } else if CW_NEXT[st as usize] == old {
            -1
        } else {
            return;
        };
        if self.regs[0xD7] & 1 == 0 {
            self.qdec_sub += d;
            if self.qdec_sub == 4 {
                self.qdec_count = self.qdec_count.wrapping_add(1);
                self.qdec_sub = 0;
            } else if self.qdec_sub == -4 {
                self.qdec_count = self.qdec_count.wrapping_sub(1);
                self.qdec_sub = 0;
            }
            return;
        }
        self.qdec_count = self.qdec_count.wrapping_add(d as i32);
    }
}
