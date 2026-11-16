//! Non-volatile store for the Zigbee stack: a log of records in two flash
//! sectors (on the bulb 0xF1000 and 0xF2000, `crates/bulb-tc32/src/nv.rs`).
//!
//! One sector is active. A store appends a record to it; a load takes the
//! last valid record of the item. When the active sector is full, the latest
//! record of every item is copied to the other sector, which then becomes
//! the active one. The old sector is only erased at the next switch, so a
//! power cut at any moment leaves at least one complete copy of every item
//! (crates/bulb-sim/tests/nv_power_cut.rs).
//!
//! Sector: 4-byte header `A5 gen !gen 5A` (written last when a sector is
//! filled, so a half-copied sector is never taken for valid), then records
//! `id, len, data[len], crc8`. An erased `id` (0xFF) ends the log.
//!
//! `N` is one more than the highest item id (`zigbee::hal::NvItem::MAX_ID`):
//! a sector switch copies the items 1..N only. While the firmware had that
//! at a fixed 12, the second and third parts of the child table (13, 14)
//! were lost at every switch.

use crate::store::Flash;

const HEADER: usize = 4;
/// Longest record payload.
pub const MAX_ITEM: usize = 160;
/// Flash page: a program never crosses one.
const PAGE: usize = 256;
/// Longest single program (the firmware's flash driver programs from a
/// 32-byte buffer on the stack).
const CHUNK: usize = 32;

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

pub struct NvLog<F, const N: usize> {
    flash: F,
    sectors: [usize; 2],
    size: usize,
    /// Called as a sector switch runs: it can take longer than the watchdog's period.
    feed: fn(),
    /// Index of the active sector in `sectors`.
    active: usize,
    /// Offset of the free space in the active sector; `size` when the
    /// log cannot be appended to (no valid sector yet, or a torn record).
    free: usize,
}

impl<F: Flash, const N: usize> NvLog<F, N> {
    pub fn new(flash: F, sectors: [usize; 2], size: usize, feed: fn()) -> Self {
        let mut nv = NvLog { flash, sectors, size, feed, active: 0, free: size };
        nv.active = match (nv.generation(0), nv.generation(1)) {
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
        if nv.generation(nv.active).is_some() {
            let end = nv.scan(|_, _, _| {});
            // a torn record or stray bytes after the log: no appending,
            // the next store moves everything to the other sector
            if nv.erased_from(end) {
                nv.free = end;
            }
        }
        nv
    }

    /// Generation of a sector with a valid header.
    fn generation(&self, k: usize) -> Option<u8> {
        let mut h = [0u8; HEADER];
        self.flash.read(self.sectors[k], &mut h);
        (h[0] == 0xa5 && h[3] == 0x5a && h[1] == !h[2]).then_some(h[1])
    }

    /// True when the sector at `sector` is erased.
    fn blank(&self, sector: usize) -> bool {
        let mut chunk = [0u8; 32];
        let mut off = 0;
        while off < self.size {
            self.flash.read(sector + off, &mut chunk);
            if chunk.iter().any(|&b| b != 0xff) {
                return false;
            }
            off += chunk.len();
        }
        true
    }

    /// Writes `data` at `addr` as the firmware's flash driver does: in
    /// programs of at most 32 bytes that do not cross a page.
    fn write(&mut self, addr: usize, data: &[u8]) {
        let mut done = 0;
        while done < data.len() {
            let at = addr + done;
            let n = (data.len() - done).min(PAGE - at % PAGE).min(CHUNK);
            self.flash.program(at, &data[done..done + n]);
            done += n;
        }
    }

    /// Walks the valid records of the active sector; returns the offset of
    /// the first byte after them (`size` if a record is damaged).
    fn scan(&self, mut each: impl FnMut(u8, usize, usize)) -> usize {
        let base = self.sectors[self.active];
        let mut off = HEADER;
        while off + 3 <= self.size {
            let mut head = [0u8; 2];
            self.flash.read(base + off, &mut head);
            let (id, len) = (head[0], head[1] as usize);
            if id == 0xff {
                return off;
            }
            if len > MAX_ITEM || off + 3 + len > self.size {
                return self.size;
            }
            let mut data = [0u8; MAX_ITEM + 1];
            self.flash.read(base + off + 2, &mut data[..len + 1]);
            if crc8(id, &data[..len]) != data[len] {
                return self.size;
            }
            each(id, off + 2, len);
            off += 3 + len;
        }
        off
    }

    /// True when the active sector is erased from `off` to its end, so that
    /// records can be appended there.
    fn erased_from(&self, off: usize) -> bool {
        let base = self.sectors[self.active];
        let mut chunk = [0u8; 32];
        let mut at = off;
        while at < self.size {
            let n = (self.size - at).min(chunk.len());
            self.flash.read(base + at, &mut chunk[..n]);
            if chunk[..n].iter().any(|&b| b != 0xff) {
                return false;
            }
            at += n;
        }
        true
    }

    pub fn load(&self, id: u8, buf: &mut [u8]) -> Option<usize> {
        self.generation(self.active)?;
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
        self.flash.read(self.sectors[self.active] + at, &mut buf[..len]);
        Some(len)
    }

    /// The latest record of every item 1..N, as `load` finds them, in one
    /// pass over the log.
    pub fn each(&self, mut f: impl FnMut(u8, &[u8])) {
        if self.generation(self.active).is_none() {
            return;
        }
        let mut latest: [Option<(usize, usize)>; N] = [None; N];
        self.scan(|rid, at, len| {
            if (1..N).contains(&(rid as usize)) {
                latest[rid as usize] = Some((at, len));
            }
        });
        for (id, l) in latest.iter().enumerate() {
            if let Some((at, len)) = *l {
                if len > 0 {
                    let mut item = [0u8; MAX_ITEM];
                    self.flash.read(self.sectors[self.active] + at, &mut item[..len]);
                    f(id as u8, &item[..len]);
                }
            }
        }
    }

    fn append(&mut self, sector: usize, off: usize, id: u8, data: &[u8]) -> usize {
        let mut rec = [0u8; MAX_ITEM + 3];
        rec[0] = id;
        rec[1] = data.len() as u8;
        rec[2..2 + data.len()].copy_from_slice(data);
        rec[2 + data.len()] = crc8(id, data);
        self.write(sector + off, &rec[..3 + data.len()]);
        off + 3 + data.len()
    }

    /// Stores two items in one flash write where they fit the active
    /// sector (the reset counters with the frame counters at every start).
    pub fn store_pair(&mut self, a: (u8, &[u8]), b: (u8, &[u8])) {
        let (la, lb) = (a.1.len(), b.1.len());
        if la == 0 || lb == 0 || la > MAX_ITEM || lb > MAX_ITEM || self.free + 6 + la + lb > self.size {
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
        self.write(self.sectors[self.active] + self.free, &rec[..n]);
        self.free += n;
    }

    pub fn store(&mut self, id: u8, data: &[u8]) {
        if data.is_empty() || data.len() > MAX_ITEM {
            return;
        }
        if self.free + 3 + data.len() <= self.size {
            self.free = self.append(self.sectors[self.active], self.free, id, data);
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
        let old_valid = self.generation(self.active);
        let target = 1 - self.active;
        if !self.blank(self.sectors[target]) {
            self.flash.erase(self.sectors[target]);
        }
        let base = self.sectors[self.active];
        let mut latest: [Option<(usize, usize)>; N] = [None; N];
        self.scan(|rid, at, len| {
            if (1..N).contains(&(rid as usize)) {
                latest[rid as usize] = Some((at, len));
            }
        });
        let mut off = HEADER;
        let mut other = 1;
        while other < N {
            if other != id as usize {
                if let Some((at, len)) = latest[other] {
                    let mut item = [0u8; MAX_ITEM];
                    self.flash.read(base + at, &mut item[..len]);
                    off = self.append(self.sectors[target], off, other as u8, &item[..len]);
                }
            }
            (self.feed)();
            other += 1;
        }
        off = self.append(self.sectors[target], off, id, data);
        let next = old_valid.map_or(0, |g| g.wrapping_add(1));
        self.write(self.sectors[target], &[0xa5, next, !next, 0x5a]);
        self.active = target;
        self.free = off;
    }

    /// Forgets every item but `keep` (the frame counters, which must never
    /// go back): leaving the network, factory reset.
    ///
    /// Cut short anywhere, it leaves either every item as it was or only
    /// `keep`, as it was. The kept record goes to the other sector under a
    /// newer header, like a switch, and only then is the old sector erased.
    /// (Both sectors erased first and the item stored again after lost it to
    /// a cut in between: the frame counters back at 0 for the neighbours that
    /// remember them.)
    pub fn forget_all_but(&mut self, keep: u8) {
        let mut item = [0u8; MAX_ITEM];
        let kept = self.load(keep, &mut item);
        let old_valid = self.generation(self.active);
        let old = self.active;
        let target = 1 - old;
        if !self.blank(self.sectors[target]) {
            self.flash.erase(self.sectors[target]);
        }
        let mut off = HEADER;
        if let Some(len) = kept {
            off = self.append(self.sectors[target], off, keep, &item[..len]);
        }
        let next = old_valid.map_or(0, |g| g.wrapping_add(1));
        self.write(self.sectors[target], &[0xa5, next, !next, 0x5a]);
        self.active = target;
        self.free = off;
        // the old records must not come back: a cut in this erase leaves the
        // old header whole (and older) or broken, never newer
        if !self.blank(self.sectors[old]) {
            self.flash.erase(self.sectors[old]);
        }
    }

    pub fn flash(&self) -> &F {
        &self.flash
    }
}
