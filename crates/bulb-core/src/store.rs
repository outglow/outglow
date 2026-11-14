//! The configuration record in flash, kept so that a power cut at any moment
//! (a wall switch) leaves the last record or the one before it readable.
//!
//! Two sectors take turns. One is active: a store appends a slot to it. When
//! it is full, the other sector is erased, the new record is written into it
//! and then its header, which makes it the active one. The old sector stays
//! as it is until the next switch, so while the new one is erased, written
//! or left without a header, the old one still has the last complete record.
//!
//! A slot is 16 bytes: eight bytes and their complement. NOR flash programs
//! by clearing bits and erases by setting them, so a program or an erase cut
//! short can only leave some of the bits it was changing undone. Such a slot
//! no longer has a complement for a second half (a bit missing in one half
//! would have to be missing in the other as well, where it is the opposite
//! bit): a torn slot is never taken for a valid one, whatever the tear.
//!
//! - slot 0 of a sector: the header `A5 'L' 'T' 01 gen 00 00 5A`; of two
//!   sectors with a header the one with the newer generation is active;
//! - slots 1..: records (`config::Config::encode`), the last valid one counts.
//!
//! The firmware before this one wrote bare 8-byte records into the first of
//! the two sectors, without a header. While no sector has a header that
//! sector is read as such a log (its last record that decodes counts), and
//! the first record stored then goes to the other sector.

use crate::config::{Config, BLOCK_LEN};

/// The flash under the log.
pub trait Flash {
    fn read(&self, addr: usize, buf: &mut [u8]);
    /// Programs `data` at `addr` (bits can only go from 1 to 0); a slot never
    /// crosses a 256-byte page.
    fn program(&mut self, addr: usize, data: &[u8]);
    /// Erases the sector that starts at `addr`.
    fn erase(&mut self, addr: usize);
}

/// Bytes of one slot.
pub const SLOT: usize = 2 * BLOCK_LEN;
const HEADER: [u8; 4] = [0xA5, b'L', b'T', 0x01];
const HEADER_END: [u8; 3] = [0x00, 0x00, 0x5A];

pub struct LightLog<F> {
    flash: F,
    sectors: [usize; 2],
    size: usize,
    /// The sector appended to (index into `sectors`) and its generation.
    active: Option<(usize, u8)>,
    /// Offset of the first free slot in the active sector.
    free: usize,
    /// Address of the record `load` returns.
    current: Option<usize>,
    /// What a valid record is (a `Config`, or another record of the same size).
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
    /// Finds the active sector and the last valid record. `sectors[0]` is the
    /// one an earlier firmware may have left its log in; `size` is the sector size.
    pub fn new(flash: F, sectors: [usize; 2], size: usize) -> Self {
        Self::of_records(flash, sectors, size, is_config)
    }

    /// A log of other 8-byte records than the configuration (the colour of
    /// a colour bulb, `colour::ColourRecord`), `check` telling a valid one.
    pub fn of_records(flash: F, sectors: [usize; 2], size: usize, check: fn(&[u8]) -> bool) -> Self {
        let mut log = LightLog { flash, sectors, size, active: None, free: size, current: None, check };
        let gens = [log.generation(0), log.generation(1)];
        log.active = match gens {
            // generations wrap
            [Some(a), Some(b)] if (b.wrapping_sub(a) as i8) > 0 => Some((1, b)),
            [Some(a), _] => Some((0, a)),
            [None, Some(b)] => Some((1, b)),
            [None, None] => None,
        };
        match log.active {
            Some((idx, _)) => {
                let (last, free) = log.scan(idx);
                log.free = free;
                // no record in the active sector (only damage does that): the other one's
                let other = 1 - idx;
                log.current = last.or_else(|| gens[other].and_then(|_| log.scan(other).0));
            }
            None => log.current = log.legacy(),
        }
        log
    }

    /// The flash, for whoever needs it after the log (tests).
    pub fn into_flash(self) -> F {
        self.flash
    }

    /// The record `load` returns.
    pub fn current(&self) -> Option<[u8; BLOCK_LEN]> {
        self.current.map(|at| self.read8(at))
    }

    fn read8(&self, at: usize) -> [u8; BLOCK_LEN] {
        let mut b = [0u8; BLOCK_LEN];
        self.flash.read(at, &mut b);
        b
    }

    /// The first half of a slot whose second half is its complement.
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

    /// The last valid record of a sector with a header, and the offset after
    /// its last slot that is not erased.
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

    /// The last record that decodes in the earlier firmware's log.
    fn legacy(&self) -> Option<usize> {
        let base = self.sectors[0];
        let mut last = None;
        let mut off = 0;
        while off + BLOCK_LEN <= self.size {
            if (self.check)(&self.read8(base + off)) {
                last = Some(base + off);
            }
            off += BLOCK_LEN;
        }
        last
    }

    fn blank(&self, base: usize) -> bool {
        let mut chunk = [0u8; 32];
        let mut off = 0;
        while off < self.size {
            self.flash.read(base + off, &mut chunk);
            if chunk.iter().any(|&b| b != 0xFF) {
                return false;
            }
            off += chunk.len();
        }
        true
    }

    /// Appends a record; an unchanged record is not written again.
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
        // Switch: the other sector gets the record, then the header.
        let (target, gen) = match self.active {
            Some((idx, g)) => (1 - idx, g.wrapping_add(1)),
            // the earlier firmware's log stays readable until this one has a header
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

    /// Forgets everything (factory reset).
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
