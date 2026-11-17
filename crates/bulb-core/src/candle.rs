pub const OFFSETS: [i8; 33] = [-2, 0, 13, 0, 8, -4, 13, 16, 0, 8, 11, 13, 16, 0, 13, 0, 11, 0, 11, 0, 8, 11, 0, 13, 0, 16, 0, 11, 0, 13, 0, 11, 0];
pub const STILL_MS: u64 = 10;
pub const START_MS: u64 = 100;
pub const FLICKER_MS: u64 = 128;
pub const THRESHOLD: u8 = 0xD0;

pub fn base(level: u8) -> u8 {
    level.clamp(20, 238)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candle {
    flicker: Option<(u8, u8)>,
    shown: u8,
    next_ms: u64,
    rng: u32,
}

impl Candle {
    pub fn new(level: u8, now_ms: u64, seed: u32) -> Candle {
        Candle { flicker: None, shown: base(level), next_ms: now_ms + START_MS, rng: seed | 1 }
    }

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
        let gaps: Vec<u64> = changes.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(gaps.iter().any(|&g| g == 64) && gaps.iter().any(|&g| g == 128));
        assert!(gaps.iter().all(|&g| g >= 64), "never faster than a step at speed 2: {gaps:?}");
    }

    #[test]
    fn the_random_numbers_are_xorshift32() {
        let mut c = Candle::new(128, 0, 0);
        assert_eq!([c.random(), c.random(), c.random()], [1056, 2054, 52392]);
        assert_eq!(c.rng, 0x9DCC_A8C5);
    }

    #[test]
    fn a_seed_gives_the_same_flicker_as_the_stock_firmware() {
        let mut c = Candle::new(128, 0, 0x1234_5678);
        let mut changes = Vec::new();
        for t in 0..4500u64 {
            if c.tick(t, 128) {
                changes.push((t, c.level()));
            }
        }
        assert_eq!(
            changes,
            [
                (120, 126), (248, 128), (376, 141), (504, 128), (632, 136), (760, 124), (888, 141), (1016, 144),
                (1144, 128), (1272, 136), (1400, 139), (1528, 141), (1656, 144), (1784, 128), (1912, 141), (2040, 128),
                (2168, 139), (2296, 128), (2424, 139), (2552, 128), (2680, 136), (2808, 139), (2936, 128), (3064, 141),
                (3192, 128), (3320, 144), (3448, 128), (3576, 139), (3704, 128), (3832, 141), (3960, 128), (4088, 139),
                (4216, 128), (4354, 126), (4482, 128)
            ]
        );
    }

    #[test]
    fn a_tick_says_whether_the_level_changed() {
        let mut c = Candle::new(128, 0, 0x1234_5678);
        assert!(!c.tick(99, 128));
        assert!(!c.tick(119, 128));
        assert!(c.tick(120, 128));
        assert!(!c.tick(121, 128));
    }
}

