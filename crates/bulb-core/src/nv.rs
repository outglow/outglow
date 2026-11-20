use crate::store::Flash;

const HEADER: usize = 4;
pub const MAX_ITEM: usize = 160;
const PAGE: usize = 256;
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
    feed: fn(),
    active: usize,
    free: usize,
}

impl<F: Flash, const N: usize> NvLog<F, N> {
    pub fn new(flash: F, sectors: [usize; 2], size: usize, feed: fn()) -> Self {
        let mut nv = NvLog { flash, sectors, size, feed, active: 0, free: size };
        nv.active = match (nv.generation(0), nv.generation(1)) {
            (Some(a), Some(b)) => {
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
            if nv.erased_from(end) {
                nv.free = end;
            }
        }
        nv
    }

    fn generation(&self, k: usize) -> Option<u8> {
        let mut h = [0u8; HEADER];
        self.flash.read(self.sectors[k], &mut h);
        (h[0] == 0xa5 && h[3] == 0x5a && h[1] == !h[2]).then_some(h[1])
    }

    fn blank(&self, sector: usize) -> bool {
        let mut chunk = [0u8; CHUNK];
        (0..self.size).step_by(CHUNK).all(|off| {
            self.flash.read(sector + off, &mut chunk);
            chunk.iter().all(|&b| b == 0xff)
        })
    }

    fn write(&mut self, addr: usize, data: &[u8]) {
        let mut done = 0;
        while done < data.len() {
            let at = addr + done;
            let n = (data.len() - done).min(PAGE - at % PAGE).min(CHUNK);
            self.flash.program(at, &data[done..done + n]);
            done += n;
        }
    }

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

    fn erased_from(&self, off: usize) -> bool {
        let base = self.sectors[self.active];
        let mut chunk = [0u8; CHUNK];
        (off..self.size).step_by(CHUNK).all(|at| {
            let n = (at + CHUNK).min(self.size) - at;
            self.flash.read(base + at, &mut chunk[..n]);
            chunk[..n].iter().all(|&b| b == 0xff)
        })
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
        let n = Self::record(&mut rec, id, data);
        self.write(sector + off + 1, &rec[1..n]);
        self.write(sector + off, &[id]);
        off + n
    }

    fn record(rec: &mut [u8], id: u8, data: &[u8]) -> usize {
        rec[0] = 0xff;
        rec[1] = data.len() as u8;
        rec[2..2 + data.len()].copy_from_slice(data);
        rec[2 + data.len()] = crc8(id, data);
        3 + data.len()
    }

    pub fn store_pair(&mut self, a: (u8, &[u8]), b: (u8, &[u8])) {
        let (la, lb) = (a.1.len(), b.1.len());
        if la == 0 || lb == 0 || la > MAX_ITEM || lb > MAX_ITEM || self.free + 6 + la + lb > self.size {
            self.store(a.0, a.1);
            self.store(b.0, b.1);
            return;
        }
        let mut rec = [0u8; 2 * (MAX_ITEM + 3)];
        let at = self.sectors[self.active] + self.free;
        let second = Self::record(&mut rec, a.0, a.1);
        let n = second + Self::record(&mut rec[second..], b.0, b.1);
        self.write(at + 1, &rec[1..n]);
        self.write(at + second, &[b.0]);
        self.write(at, &[a.0]);
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
        for other in 1..N {
            if other != id as usize {
                if let Some((at, len)) = latest[other] {
                    let mut item = [0u8; MAX_ITEM];
                    self.flash.read(base + at, &mut item[..len]);
                    off = self.append(self.sectors[target], off, other as u8, &item[..len]);
                }
            }
            (self.feed)();
        }
        off = self.append(self.sectors[target], off, id, data);
        let next = old_valid.map_or(0, |g| g.wrapping_add(1));
        self.write(self.sectors[target], &[0xa5, next, !next, 0x5a]);
        self.active = target;
        self.free = off;
    }

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
        if !self.blank(self.sectors[old]) {
            self.flash.erase(self.sectors[old]);
        }
    }
}
