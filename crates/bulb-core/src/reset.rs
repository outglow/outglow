pub trait WordStore {
    fn slots(&self) -> usize;
    fn read(&self, slot: usize) -> u16;
    fn write(&mut self, slot: usize, word: u16);
    fn erase(&mut self);
}

const MAGIC_BITS: u8 = 7;
const LONG_MS: u64 = 6_000;
const COMMIT_MS: u64 = 30_000;

fn word(bits: u8) -> u16 {
    (0xFFFFu32 << bits) as u16
}

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
    pub fn power_on(store: S, factory_reset: impl FnOnce()) -> (Self, bool) {
        let mut s = PowerCycles { store, slot: 0, open: false, step: 0, long_done: false };
        let slots = s.store.slots();
        let active = (1..slots).find(|&k| s.store.read(k) != 0);
        let sound = s.store.read(0) == 0 && active.map_or(true, |a| (a + 1..slots).all(|k| s.store.read(k) == 0xFFFF));
        let w = match active {
            Some(slot) if sound => {
                s.slot = slot;
                s.store.read(slot)
            }
            _ => {
                s.restart();
                0xFFFF
            }
        };
        let mut factory = false;
        let mut n = match bits(w) {
            Some(MAGIC_BITS) => {
                factory_reset();
                s.next();
                factory = true;
                0
            }
            Some(n @ (0 | 1 | 2 | 3 | 5)) => n,
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

    pub fn idle(store: S) -> Self {
        PowerCycles { store, slot: 0, open: false, step: 0, long_done: true }
    }

    fn next(&mut self) {
        self.store.write(self.slot, 0x7FFF);
        self.store.write(self.slot, 0x0000);
        self.slot += 1;
        if self.slot >= self.store.slots() {
            self.restart();
        }
    }

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

    pub fn tick(&mut self, up_ms: u64) {
        if !self.open {
            return;
        }
        if !self.long_done && up_ms >= LONG_MS {
            self.long_done = true;
            if self.step <= 3 {
                self.commit();
            } else {
                self.step += 1;
                self.store.write(self.slot, word(self.step));
            }
        } else if up_ms >= COMMIT_MS {
            self.commit();
        }
    }

    pub fn step(&self) -> u8 {
        self.step
    }

    pub fn store(&self) -> &S {
        &self.store
    }
}
