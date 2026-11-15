//! The candle mode of the AwoX app (Level command 0x10), as the stock
//! firmware of the TLSR8258 lamps does it (EGLO_ZM_RGB_TW 4.60: the step at
//! 0x74e24, started at 0x75080; docs/stock-colour.md):
//!
//! - still: every 10 ms a random number; when its second byte is above
//!   0xD0 (about one in six) a flicker starts, at speed 1 or, one time in
//!   four, 2;
//! - a flicker: 33 steps of `OFFSETS` added to the level (kept within
//!   20..=238), each for 128 ms divided by the speed; then still again at
//!   the last of them.
//!
//! The light shows the level the candle gives instead of its own; colour
//! and colour temperature stay as they are. The stock firmware's random
//! numbers come from code below 0x6000, which is not in the images: these
//! are a xorshift of their own.

/// Added to the level, one a step (signed octets at 0xb1d88 of the image).
pub const OFFSETS: [i8; 33] = [-2, 0, 13, 0, 8, -4, 13, 16, 0, 8, 11, 13, 16, 0, 13, 0, 11, 0, 11, 0, 8, 11, 0, 13, 0, 16, 0, 11, 0, 13, 0, 11, 0];
/// How long a still step lasts, and the first one after the start.
pub const STILL_MS: u64 = 10;
pub const START_MS: u64 = 100;
/// A flicker step at speed 1 (at speed 2 half of it).
pub const FLICKER_MS: u64 = 128;
/// A flicker starts when the second octet of the random number is above this.
pub const THRESHOLD: u8 = 0xD0;

/// The level a flicker works around: the light's level kept within 20..=238.
pub fn base(level: u8) -> u8 {
    level.clamp(20, 238)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candle {
    /// In a flicker: its next step (0..=33) and speed; None when still.
    flicker: Option<(u8, u8)>,
    /// The level shown.
    shown: u8,
    next_ms: u64,
    rng: u32,
}

impl Candle {
    /// Starts at the light's level (the stock firmware starts at what the
    /// last candle left, 0 the first time: a dark flash we leave out).
    pub fn new(level: u8, now_ms: u64, seed: u32) -> Candle {
        Candle { flicker: None, shown: base(level), next_ms: now_ms + START_MS, rng: seed | 1 }
    }

    /// The level the light shows now.
    pub fn level(&self) -> u8 {
        self.shown
    }

    fn random(&mut self) -> u16 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x >> 8) as u16
    }

    /// Takes the steps due by `now_ms` around the light's `level`. True when the level shown changed.
    pub fn tick(&mut self, now_ms: u64, level: u8) -> bool {
        let before = self.shown;
        while now_ms >= self.next_ms {
            let (index, speed) = match self.flicker {
                Some(f) => f,
                None => {
                    let r = self.random();
                    if (r >> 8) as u8 <= THRESHOLD {
                        self.next_ms += STILL_MS;
                        continue;
                    }
                    (0, if r & 3 == 3 { 2 } else { 1 })
                }
            };
            if index as usize >= OFFSETS.len() {
                self.flicker = None;
                self.next_ms += STILL_MS;
                continue;
            }
            self.shown = (base(level) as i16 + OFFSETS[index as usize] as i16) as u8;
            self.flicker = Some((index + 1, speed));
            self.next_ms += FLICKER_MS / speed as u64;
        }
        self.shown != before
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::{vec, vec::Vec};

    #[test]
    fn the_levels_are_the_stock_firmware_s() {
        // what the stock RGB_TW lamp showed in the emulator, around 128 and around 254
        let around = |l: u8| {
            let mut v: Vec<u8> = OFFSETS.iter().map(|&o| (base(l) as i16 + o as i16) as u8).collect();
            v.sort();
            v.dedup();
            v
        };
        assert_eq!(around(128), vec![124, 126, 128, 136, 139, 141, 144]);
        assert_eq!(around(254), vec![234, 236, 238, 246, 249, 251, 254]);
        assert_eq!(around(5), vec![16, 18, 20, 28, 31, 33, 36], "never below 16");
    }

    #[test]
    fn it_flickers_in_bursts_of_33_steps_at_64_or_128_ms() {
        let mut c = Candle::new(128, 0, 0x1234_5678);
        assert_eq!(c.level(), 128);
        let mut changes = Vec::new();
        let mut last = c.level();
        for t in 0..60_000u64 {
            c.tick(t, 128);
            if c.level() != last {
                changes.push(t);
                last = c.level();
            }
        }
        assert!(changes.len() > 100, "it flickers: {} changes in a minute", changes.len());
        // steps are 64 or 128 ms apart within a flicker
        let gaps: Vec<u64> = changes.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(gaps.iter().any(|&g| g == 64) && gaps.iter().any(|&g| g == 128));
        assert!(gaps.iter().all(|&g| g >= 64), "never faster than a step at speed 2: {gaps:?}");
    }
}
