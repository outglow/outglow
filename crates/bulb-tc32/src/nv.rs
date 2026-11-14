//! Non-volatile store for the Zigbee stack: a log of records in two 4 KB
//! flash sectors.
//!
//! One sector is active. A store appends a record to it; a load takes the
//! last valid record of the item. When the active sector is full, the latest
//! record of every item is copied to the other sector, which then becomes
//! the active one. The old sector is only erased at the next switch, so a
//! power cut at any moment leaves at least one complete copy of every item.
//!
//! Sector: 4-byte header `A5 gen !gen 5A` (written last when a sector is
//! filled, so a half-copied sector is never taken for valid), then records
//! `id, len, data[len], crc8`. An erased `id` (0xFF) ends the log.

use crate::flash;

/// The two sectors, after the first configuration sector (see `board::CONFIG_SECTORS`).
pub const SECTORS: [usize; 2] = [0xf_1000, 0xf_2000];
const SECTOR_SIZE: usize = 0x1000;
const HEADER: usize = 4;
/// Longest record payload.
pub const MAX_ITEM: usize = 160;
/// Item ids in use (`zigbee::hal::NvItem`): 1..=MAX_ID. A sector switch
/// copies these only; while this stood at a fixed 12, the second and third
/// parts of the child table (13, 14) were lost at every switch.
const MAX_ID: u8 = zigbee::hal::NvItem::MAX_ID;

fn crc8(id: u8, data: &[u8]) -> u8 {
    let mut crc = 0xffu8;
    let mut step = |b: u8| {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 { (crc << 1) ^ 0x07 } else { crc << 1 };
        }
    };
    step(id);
    step(data.len() as u8);
    for &b in data {
        step(b);
    }
    crc
}

/// Generation of a sector with a valid header.
fn generation(sector: usize) -> Option<u8> {
    let mut h = [0u8; HEADER];
    flash::read(sector, &mut h);
    (h[0] == 0xa5 && h[3] == 0x5a && h[1] == !h[2]).then_some(h[1])
}

use crate::flash::blank;

pub struct Nv {
    /// Index of the active sector in `SECTORS`.
    active: usize,
    /// Offset of the free space in the active sector; `SECTOR_SIZE` when the
    /// log cannot be appended to (no valid sector yet, or a torn record).
    free: usize,
}

impl Nv {
    pub fn new() -> Self {
        let active = match (generation(SECTORS[0]), generation(SECTORS[1])) {
            (Some(a), Some(b)) => {
                // the newer one; generations wrap
                if b.wrapping_sub(a) as i8 > 0 {
                    1
                } else {
                    0
                }
            }
            (None, Some(_)) => 1,
            _ => 0,
        };
        let mut nv = Nv { active, free: SECTOR_SIZE };
        if generation(SECTORS[active]).is_some() {
            let end = nv.scan(|_, _, _| {});
            // a torn record or stray bytes after the log: no appending,
            // the next store moves everything to the other sector
            if nv.erased_from(end) {
                nv.free = end;
            }
        }
        nv
    }

    /// Walks the valid records of the active sector; returns the offset of
    /// the first byte after them (`SECTOR_SIZE` if a record is damaged).
    fn scan(&self, mut each: impl FnMut(u8, usize, usize)) -> usize {
        let base = SECTORS[self.active];
        let mut off = HEADER;
        while off + 3 <= SECTOR_SIZE {
            let mut head = [0u8; 2];
            flash::read(base + off, &mut head);
            let (id, len) = (head[0], head[1] as usize);
            if id == 0xff {
                return off;
            }
            if len > MAX_ITEM || off + 3 + len > SECTOR_SIZE {
                return SECTOR_SIZE;
            }
            let mut data = [0u8; MAX_ITEM + 1];
            flash::read(base + off + 2, &mut data[..len + 1]);
            if crc8(id, &data[..len]) != data[len] {
                return SECTOR_SIZE;
            }
            each(id, off + 2, len);
            off += 3 + len;
        }
        off
    }

    /// True when the active sector is erased from `off` to its end, so that
    /// records can be appended there.
    fn erased_from(&self, off: usize) -> bool {
        let base = SECTORS[self.active];
        let mut chunk = [0u8; 32];
        let mut at = off;
        while at < SECTOR_SIZE {
            let n = (SECTOR_SIZE - at).min(chunk.len());
            flash::read(base + at, &mut chunk[..n]);
            if chunk[..n].iter().any(|&b| b != 0xff) {
                return false;
            }
            at += n;
        }
        true
    }

    pub fn load(&self, id: u8, buf: &mut [u8]) -> Option<usize> {
        if generation(SECTORS[self.active]).is_none() {
            return None;
        }
        let mut found = None;
        self.scan(|rid, at, len| {
            if rid == id {
                found = Some((at, len));
            }
        });
        let (at, len) = found?;
        if len == 0 || len > buf.len() {
            return None;
        }
        flash::read(SECTORS[self.active] + at, &mut buf[..len]);
        Some(len)
    }

    fn append(sector: usize, off: usize, id: u8, data: &[u8]) -> usize {
        let mut rec = [0u8; MAX_ITEM + 3];
        rec[0] = id;
        rec[1] = data.len() as u8;
        rec[2..2 + data.len()].copy_from_slice(data);
        rec[2 + data.len()] = crc8(id, data);
        flash::write(sector + off, &rec[..3 + data.len()]);
        off + 3 + data.len()
    }

    /// Stores two items in one flash program where they fit the active
    /// sector (the reset counters with the frame counters at every start).
    pub fn store_pair(&mut self, a: (u8, &[u8]), b: (u8, &[u8])) {
        let (la, lb) = (a.1.len(), b.1.len());
        if la == 0 || lb == 0 || la > MAX_ITEM || lb > MAX_ITEM || self.free + 6 + la + lb > SECTOR_SIZE {
            self.store(a.0, a.1);
            self.store(b.0, b.1);
            return;
        }
        let mut rec = [0u8; 2 * (MAX_ITEM + 3)];
        let mut n = 0;
        for (id, data) in [a, b] {
            rec[n] = id;
            rec[n + 1] = data.len() as u8;
            rec[n + 2..n + 2 + data.len()].copy_from_slice(data);
            rec[n + 2 + data.len()] = crc8(id, data);
            n += 3 + data.len();
        }
        flash::write(SECTORS[self.active] + self.free, &rec[..n]);
        self.free += n;
    }

    pub fn store(&mut self, id: u8, data: &[u8]) {
        if data.is_empty() || data.len() > MAX_ITEM {
            return;
        }
        if self.free + 3 + data.len() <= SECTOR_SIZE {
            self.free = Self::append(SECTORS[self.active], self.free, id, data);
            return;
        }
        // Switch sectors: the new item and the latest of every other item go
        // to the other sector, the header last.
        //
        // One pass collects the latest record of every item. Loading each
        // item separately would rescan the whole (full) log once per item,
        // CRC-checking every record each time; that is slow enough to starve
        // the watchdog before the header is written, and a reset there leaves
        // the target headerless while the old sector stays full, so the next
        // boot switches again and erases the target forever. The watchdog is
        // also fed as the copy runs, as the staging erase does.
        let old_valid = generation(SECTORS[self.active]);
        let target = 1 - self.active;
        if !blank(SECTORS[target]) {
            flash::erase_sector(SECTORS[target]);
        }
        let base = SECTORS[self.active];
        let mut latest: [Option<(usize, usize)>; MAX_ID as usize + 1] = [None; MAX_ID as usize + 1];
        self.scan(|rid, at, len| {
            if (1..=MAX_ID).contains(&rid) {
                latest[rid as usize] = Some((at, len));
            }
        });
        let mut off = HEADER;
        let mut other = 1;
        while other <= MAX_ID {
            if other != id {
                if let Some((at, len)) = latest[other as usize] {
                    let mut item = [0u8; MAX_ITEM];
                    flash::read(base + at, &mut item[..len]);
                    off = Self::append(SECTORS[target], off, other, &item[..len]);
                }
            }
            crate::board::watchdog_feed();
            other += 1;
        }
        off = Self::append(SECTORS[target], off, id, data);
        let next = old_valid.map_or(0, |g| g.wrapping_add(1));
        flash::write(SECTORS[target], &[0xa5, next, !next, 0x5a]);
        self.active = target;
        self.free = off;
    }

    /// Factory reset: forgets everything but the item `keep` (the frame
    /// counters, which must never go back).
    pub fn factory_reset(keep: u8) {
        let mut nv = Nv::new();
        let mut item = [0u8; MAX_ITEM];
        let kept = nv.load(keep, &mut item);
        nv.erase();
        if let Some(len) = kept {
            nv.store(keep, &item[..len]);
        }
    }

    /// Forgets everything.
    pub fn erase(&mut self) {
        for sector in SECTORS {
            if !blank(sector) {
                flash::erase_sector(sector);
            }
        }
        self.active = 0;
        self.free = SECTOR_SIZE;
    }
}
