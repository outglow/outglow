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
//!
//! A power cut can interrupt any write; none may bring the reset nearer
//! (crates/bulb-sim/tests/reset_power_cut.rs). A count goes one bit at a
//! time, so it is either taken or not. A word is closed top bit first, so
//! that what is left of the rest is never a count. Slot 0 is zeroed after
//! every erase of the sector and never holds a count, and the words after
//! the active one must be untouched: a sector that is not so (an erase cut
//! short, or the stock firmware's counter in slot 0) is started over. The
//! stock firmware reads this layout as its own (the first non-zero word).

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
        let slots = s.store.slots();
        let active = (1..slots).find(|&k| s.store.read(k) != 0);
        // Slot 0 is zeroed right after every erase and is never a count, and
        // the words after the active one are untouched: an erase cut short
        // leaves a mix of bits that fails either test (or, from another
        // firmware, a counter in another place).
        let sound = s.store.read(0) == 0 && active.map_or(true, |a| (a + 1..slots).all(|k| s.store.read(k) == 0xFFFF));
        let w = match active {
            Some(slot) if sound => {
                s.slot = slot;
                s.store.read(slot)
            }
            // every word used up, or not a counter as this one leaves it
            _ => {
                s.restart();
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

    /// Closes the active word (0x0000) and moves to the next one.
    ///
    /// The top bit goes first, alone: a program cut short by a power loss
    /// leaves any mix of the bits it was clearing, and with the top bit
    /// still set such a mix can read as a count further on than the word
    /// was (0xFFF8 cut on its way to 0x0000 reads 0xFF80 when its low bits
    /// went first), even as the complete sequence: a factory reset at the
    /// next power-on. A single bit is either cleared or not; once it is, no
    /// mix of the rest is a count.
    fn next(&mut self) {
        self.store.write(self.slot, 0x7FFF);
        self.store.write(self.slot, 0x0000);
        self.slot += 1;
        if self.slot >= self.store.slots() {
            self.restart();
        }
    }

    /// Starts the sector over: erased (unless it is blank), slot 0 zeroed as
    /// the sign that the erase completed, the count in slot 1.
    fn restart(&mut self) {
        if (0..self.store.slots()).any(|k| self.store.read(k) != 0xFFFF) {
            self.store.erase();
        }
        self.store.write(0, 0x0000);
        self.slot = 1;
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
