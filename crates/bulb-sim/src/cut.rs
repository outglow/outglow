//! The power-cut model the store tests share: the ways a flash operation on
//! a [`NorFlash`] can be left half done when the power goes.
//!
//! A program is cut before it, after it, with a prefix of its bytes done,
//! with its bits done in order (low bit of the first byte first) up to some
//! point, or with random bits done; an erase with half of the sector, random
//! bits of it, or all but a few scattered bits erased.

use crate::{FlashOp, NorFlash};

/// A small xorshift generator: the tests are deterministic.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    pub fn byte(&mut self) -> u8 {
        self.next() as u8
    }
}

/// The flash after a cut during `op` (`at` is the flash just before it), for
/// every way of cutting it this model knows, with a label for each.
pub fn cuts(at: &NorFlash, op: &FlashOp, rng: &mut Rng) -> Vec<(String, NorFlash)> {
    let mut out = Vec::new();
    let mut part = |label: String, undone: &mut dyn FnMut(usize, u8) -> u8| {
        let f = at.snapshot();
        f.apply_part(op, undone);
        out.push((label, f));
    };
    part("before".into(), &mut |_, _| 0xFF);
    part("after".into(), &mut |_, _| 0);
    match op {
        FlashOp::Program { data, .. } => {
            let n = data.len();
            for k in 1..n {
                part(format!("{k} of {n} bytes programmed"), &mut |i, _| if i < k { 0 } else { 0xFF });
            }
            for bit in (1..n * 8).step_by(3) {
                part(format!("{bit} of {} bits programmed", n * 8), &mut |i, _| {
                    let done = bit.saturating_sub(i * 8).min(8);
                    (0xFFu16 << done) as u8
                });
            }
            for k in 0..8 {
                let masks: Vec<u8> = (0..n).map(|_| rng.byte()).collect();
                part(format!("random bits programmed #{k}"), &mut |i, _| masks[i]);
            }
        }
        FlashOp::Erase { .. } => {
            let size = at.sector_size();
            part("first half erased".into(), &mut |i, _| if i < size / 2 { 0 } else { 0xFF });
            part("second half erased".into(), &mut |i, _| if i < size / 2 { 0xFF } else { 0 });
            for k in 0..6 {
                let mut r = Rng(rng.next() | 1);
                // sparse, even and dense leftovers
                let density = k % 3;
                part(format!("random bits erased #{k}"), &mut |_, _| match density {
                    0 => r.byte() & r.byte(),
                    1 => r.byte(),
                    _ => r.byte() | r.byte(),
                });
            }
            for k in 0..2 {
                // nearly done: a few bits anywhere in the sector still cleared
                let left: Vec<(usize, u8)> = (0..4).map(|_| ((rng.next() as usize) % size, 1 << (rng.next() % 8))).collect();
                part(format!("all but a few bits erased #{k}"), &mut |i, _| {
                    left.iter().filter(|(at, _)| *at == i).fold(0, |m, (_, b)| m | b)
                });
            }
        }
    }
    out
}
