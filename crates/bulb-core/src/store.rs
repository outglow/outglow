use crate::config::{Config, BLOCK_LEN};

pub trait Flash {
    fn read(&self, addr: usize, buf: &mut [u8]);
    fn program(&mut self, addr: usize, data: &[u8]);
    fn erase(&mut self, addr: usize);
}

pub const SLOT: usize = 2 * BLOCK_LEN;
const HEADER: [u8; 4] = [0xA5, b'L', b'T', 0x01];
const HEADER_END: [u8; 3] = [0x00, 0x00, 0x5A];

pub struct LightLog<F> {
    flash: F,
    sectors: [usize; 2],
    size: usize,
    active: Option<(usize, u8)>,
    free: usize,
    current: Option<usize>,
    check: fn(&[u8]) -> bool,
}

fn is_config(r: &[u8]) -> bool {
    Config::decode(r).is_some()
}

fn slot_of(data: &[u8; BLOCK_LEN]) -> [u8; SLOT] {
    let mut s = [0u8; SLOT];
    for i in 0..BLOCK_LEN {
        s[i] = data[i];
        s[BLOCK_LEN + i] = !data[i];
    }
    s
}

fn header(gen: u8) -> [u8; BLOCK_LEN] {
    [HEADER[0], HEADER[1], HEADER[2], HEADER[3], gen, HEADER_END[0], HEADER_END[1], HEADER_END[2]]
}

impl<F: Flash> LightLog<F> {
    pub fn new(flash: F, sectors: [usize; 2], size: usize) -> Self {
        Self::of_records(flash, sectors, size, is_config)
    }

    pub fn of_records(flash: F, sectors: [usize; 2], size: usize, check: fn(&[u8]) -> bool) -> Self {
        let mut log = LightLog { flash, sectors, size, active: None, free: size, current: None, check };
        let gens = [log.generation(0), log.generation(1)];
        log.active = match gens {
            [Some(a), Some(b)] if (b.wrapping_sub(a) as i8) > 0 => Some((1, b)),
            [Some(a), _] => Some((0, a)),
            [None, Some(b)] => Some((1, b)),
            [None, None] => None,
        };
        match log.active {
            Some((idx, _)) => {
                let (last, free) = log.scan(idx);
                log.free = free;
                let other = 1 - idx;
                log.current = last.or_else(|| gens[other].and_then(|_| log.scan(other).0));
            }
            None => log.current = log.legacy(),
        }
        log
    }

    pub fn current(&self) -> Option<[u8; BLOCK_LEN]> {
        self.current.map(|at| self.read8(at))
    }

    fn read8(&self, at: usize) -> [u8; BLOCK_LEN] {
        let mut b = [0u8; BLOCK_LEN];
        self.flash.read(at, &mut b);
        b
    }

    fn slot(&self, at: usize) -> Option<[u8; BLOCK_LEN]> {
        let mut s = [0u8; SLOT];
        self.flash.read(at, &mut s);
        let (a, b) = s.split_at(BLOCK_LEN);
        if a.iter().zip(b).all(|(x, y)| *x == !*y) {
            Some(a.try_into().unwrap())
        } else {
            None
        }
    }

    fn generation(&self, idx: usize) -> Option<u8> {
        let h = self.slot(self.sectors[idx])?;
        (h[..4] == HEADER && h[5..] == HEADER_END).then_some(h[4])
    }

    fn scan(&self, idx: usize) -> (Option<usize>, usize) {
        let base = self.sectors[idx];
        let (mut last, mut free) = (None, SLOT);
        let mut off = SLOT;
        while off + SLOT <= self.size {
            let mut s = [0u8; SLOT];
            self.flash.read(base + off, &mut s);
            if s.iter().any(|&b| b != 0xFF) {
                free = off + SLOT;
                if self.slot(base + off).is_some_and(|r| (self.check)(&r)) {
                    last = Some(base + off);
                }
            }
            off += SLOT;
        }
        (last, free)
    }

    fn legacy(&self) -> Option<usize> {
        let base = self.sectors[0];
        (0..self.size / BLOCK_LEN).map(|k| base + k * BLOCK_LEN).filter(|&at| (self.check)(&self.read8(at))).last()
    }

    fn blank(&self, base: usize) -> bool {
        let mut chunk = [0u8; 32];
        (0..self.size).step_by(32).all(|off| {
            self.flash.read(base + off, &mut chunk);
            chunk.iter().all(|&b| b == 0xFF)
        })
    }

    pub fn store(&mut self, data: &[u8; BLOCK_LEN]) {
        if self.current() == Some(*data) {
            return;
        }
        if let Some((idx, _)) = self.active {
            if self.free + SLOT <= self.size {
                let at = self.sectors[idx] + self.free;
                self.flash.program(at, &slot_of(data));
                self.current = Some(at);
                self.free += SLOT;
                return;
            }
        }
        let (target, gen) = match self.active {
            Some((idx, g)) => (1 - idx, g.wrapping_add(1)),
            None => (if self.current.is_some() { 1 } else { 0 }, 0),
        };
        let base = self.sectors[target];
        if !self.blank(base) {
            self.flash.erase(base);
        }
        self.flash.program(base + SLOT, &slot_of(data));
        self.flash.program(base, &slot_of(&header(gen)));
        self.active = Some((target, gen));
        self.free = 2 * SLOT;
        self.current = Some(base + SLOT);
    }

    pub fn erase(&mut self) {
        for base in self.sectors {
            if !self.blank(base) {
                self.flash.erase(base);
            }
        }
        self.active = None;
        self.free = self.size;
        self.current = None;
    }
}

impl<F: Flash> crate::hal::ConfigStore for LightLog<F> {
    fn load(&self, buf: &mut [u8]) -> Option<usize> {
        let r = self.current()?;
        let b = buf.get_mut(..BLOCK_LEN)?;
        b.copy_from_slice(&r);
        Some(BLOCK_LEN)
    }

    fn store(&mut self, data: &[u8]) {
        if let Ok(d) = data.try_into() {
            LightLog::store(self, d);
        }
    }
}
