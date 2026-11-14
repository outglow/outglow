//! Factory reset by a sequence of power cycles, in the format and timing of
//! the stock firmware (docs/stock-analysis.md), so that the habit and the
//! flash sector carry over.
//!
//! The store is a string of 16-bit words; the first non-zero one is active.
//! Every power-on clears its lowest set bit (FFFE, FFFC, ...):
//!
//! - after 6 s of operation: with up to 3 bits cleared the word is closed
//!   (zeroed); with 4 or 6, another bit is cleared (a "long" power-on);
//! - after 30 s of operation the word is closed;
//! - a start that finds 4 or 6 bits (a long power-on cut short before 6 s),
//!   or anything that is not a count, closes the word and starts over;
//! - a start that finds 7 bits (0xFF80) is the factory reset.
//!
//! That makes the sequence: power-ons of 1, 1, 1, 12 and 12 seconds, then one
//! more. A watchdog or software reset is not a power-on and does not count.

/// The words of the counter in non-volatile memory.
pub trait WordStore {
    /// Number of words.
    fn slots(&self) -> usize;
    fn read(&self, slot: usize) -> u16;
    /// Writes a word; bits can only go from 1 to 0.
    fn write(&mut self, slot: usize, word: u16);
    /// Sets every word to 0xFFFF.
    fn erase(&mut self);
}

const MAGIC_BITS: u8 = 7;
const LONG_MS: u64 = 6_000;
const COMMIT_MS: u64 = 30_000;

fn word(bits: u8) -> u16 {
    (0xFFFFu32 << bits) as u16
}

/// Number of cleared bits if the word is a count (0xFFFF << n).
fn bits(w: u16) -> Option<u8> {
    (0..16).find(|&n| w == word(n))
}

pub struct PowerCycles<S> {
    store: S,
    slot: usize,
    open: bool,
    step: u8,
    long_done: bool,
}

impl<S: WordStore> PowerCycles<S> {
    /// Counts this power-on. The flag says whether it completes the factory
    /// reset sequence.
    pub fn power_on(store: S) -> (Self, bool) {
        let mut s = PowerCycles { store, slot: 0, open: false, step: 0, long_done: false };
        let mut active = None;
        for slot in 0..s.store.slots() {
            let w = s.store.read(slot);
            if w != 0 {
                active = Some((slot, w));
                break;
            }
        }
        let w = match active {
            Some((slot, w)) => {
                s.slot = slot;
                w
            }
            None => {
                // every word used up
                s.store.erase();
                0xFFFF
            }
        };
        let mut factory = false;
        let mut n = match bits(w) {
            Some(MAGIC_BITS) => {
                // close the word first: a reset interrupted by a power loss must not repeat
                s.next();
                factory = true;
                0
            }
            Some(n @ (0 | 1 | 2 | 3 | 5)) => n,
            // 4, 6: a long power-on that was cut short; anything else: garbage
            _ => {
                s.next();
                0
            }
        };
        n += 1;
        s.store.write(s.slot, word(n));
        s.open = true;
        s.step = n;
        (s, factory)
    }

    /// For a start that is not a power-on: the counter stays as it is.
    pub fn idle(store: S) -> Self {
        PowerCycles { store, slot: 0, open: false, step: 0, long_done: true }
    }

    fn next(&mut self) {
        self.store.write(self.slot, 0x0000);
        self.slot += 1;
        if self.slot >= self.store.slots() {
            self.store.erase();
            self.slot = 0;
        }
    }

    fn commit(&mut self) {
        if self.open {
            self.open = false;
            self.next();
        }
        self.step = 0;
    }

    /// Call from the main loop with the time since power-on.
    pub fn tick(&mut self, up_ms: u64) {
        if !self.open {
            return;
        }
        if !self.long_done && up_ms >= LONG_MS {
            self.long_done = true;
            if self.step <= 3 {
                self.commit();
            } else {
                // 4 -> 5, 6 -> 7
                self.step += 1;
                self.store.write(self.slot, word(self.step));
            }
        } else if up_ms >= COMMIT_MS {
            self.commit();
        }
    }

    /// Position in the sequence (0 = not counting).
    pub fn step(&self) -> u8 {
        self.step
    }

    pub fn store(&self) -> &S {
        &self.store
    }
}
