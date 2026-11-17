const HOLD_MS: u32 = 60_000;

use super::NEIGHBORS;

pub(crate) const LINK_FRAMES: usize = 64;
pub(crate) const RING: usize = 64;
pub(crate) const ROWS: usize = 360;
pub(crate) const ROW_MS: u32 = 120_000;

pub const HEADER: usize = 24;
pub(crate) const LINK_ENTRY: usize = 36;
pub(crate) const RING_ENTRY: usize = 16;
pub(crate) const ROW_ENTRY: usize = 12;
const LINKS_AT: usize = HEADER;
const RING_AT: usize = LINKS_AT + NEIGHBORS * LINK_ENTRY;
const ROWS_AT: usize = RING_AT + RING * RING_ENTRY;
pub const BOX_LEN: usize = ROWS_AT + ROWS * ROW_ENTRY;
pub const BOX_PAGE: usize = 64;
pub const BOX_PAGES: usize = BOX_LEN.div_ceil(BOX_PAGE);
pub const BOX_FIRST: u16 = 0x6000;
pub const VERSION: u8 = 1;

pub mod kind {
    pub const SECURED: u8 = 1 << 2;
    pub const BROADCAST: u8 = 1 << 3;
    pub const RELAYED: u8 = 1 << 4;
    pub const ID_COMMAND: u8 = 1 << 5;
    pub const ID_CLUSTER: u8 = 2 << 5;
    pub const ID_APS_ACK: u8 = 3 << 5;
}

#[derive(Clone, Copy, Default)]
pub(crate) struct Totals {
    pub tx: u32,
    pub rx: u32,
    pub retries: u32,
    pub fails: u32,
    pub route_disc: u32,
}

pub(crate) struct RowNow {
    pub now: u32,
    pub totals: Totals,
    pub coord_hop: u16,
    pub coord_cost: u8,
    pub coord_measured: bool,
    pub coord_delivery: u8,
    pub stale: u8,
    pub noise: i8,
}

pub struct BlackBox {
    links: [[u8; LINK_FRAMES / 2]; NEIGHBORS],
    link_head: [u8; NEIGHBORS],
    link_len: [u8; NEIGHBORS],
    ring: [[u8; RING_ENTRY]; RING],
    ring_head: u8,
    ring_len: u8,
    ring_written: u32,
    rows: [[u8; ROW_ENTRY]; ROWS],
    row_head: u16,
    row_len: u16,
    pub(crate) row_at: u32,
    last: Totals,
    primed: bool,
    frozen: (u8, u8, u16, u16, [u8; NEIGHBORS], [u8; NEIGHBORS], u32),
    hold_until: Option<u32>,
}

impl BlackBox {
    pub const EMPTY: BlackBox = BlackBox {
        links: [[0; LINK_FRAMES / 2]; NEIGHBORS],
        link_head: [0; NEIGHBORS],
        link_len: [0; NEIGHBORS],
        ring: [[0; RING_ENTRY]; RING],
        ring_head: 0,
        ring_len: 0,
        ring_written: 0,
        rows: [[0; ROW_ENTRY]; ROWS],
        row_head: 0,
        row_len: 0,
        row_at: 0,
        last: Totals { tx: 0, rx: 0, retries: 0, fails: 0, route_disc: 0 },
        primed: false,
        frozen: (0, 0, 0, 0, [0; NEIGHBORS], [0; NEIGHBORS], 0),
        hold_until: None,
    };

    pub(crate) fn held(&mut self, now: u32) -> bool {
        match self.hold_until {
            Some(t) if (now.wrapping_sub(t) as i32) < 0 => true,
            Some(_) => {
                self.hold_until = None;
                false
            }
            None => false,
        }
    }

    pub(crate) fn clear_link(&mut self, slot: usize) {
        self.link_head[slot] = 0;
        self.link_len[slot] = 0;
    }

    pub(crate) fn link_frame(&mut self, now: u32, slot: usize, attempts: u8, ok: bool) {
        if self.held(now) {
            return;
        }
        let v = attempts.min(7) | (ok as u8) << 3;
        let at = self.link_head[slot] as usize;
        let b = &mut self.links[slot][at / 2];
        *b = if at % 2 == 0 { (*b & 0xF0) | v } else { (*b & 0x0F) | v << 4 };
        self.link_head[slot] = ((at + 1) % LINK_FRAMES) as u8;
        self.link_len[slot] = (self.link_len[slot] + 1).min(LINK_FRAMES as u8);
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn frame(&mut self, now: u32, mac_dst: u16, dst: u16, src: u16, seq: u8, kind: u8, id: u16, attempts: u8, status: u8) {
        if self.held(now) {
            return;
        }
        let e = &mut self.ring[self.ring_head as usize];
        e[0..4].copy_from_slice(&now.to_le_bytes());
        e[4..6].copy_from_slice(&mac_dst.to_le_bytes());
        e[6..8].copy_from_slice(&dst.to_le_bytes());
        e[8..10].copy_from_slice(&src.to_le_bytes());
        e[10] = kind;
        e[11..13].copy_from_slice(&id.to_le_bytes());
        e[13] = seq;
        e[14] = attempts.min(15) | status << 4;
        e[15] = 0;
        self.ring_head = ((self.ring_head as usize + 1) % RING) as u8;
        self.ring_len = (self.ring_len + 1).min(RING as u8);
        self.ring_written = self.ring_written.wrapping_add(1);
    }

    pub(crate) fn row(&mut self, r: RowNow) {
        let t = r.totals;
        if !self.primed {
            self.last = t;
            self.primed = true;
            return;
        }
        let l = self.last;
        let d = |a: u32, b: u32| a.wrapping_sub(b);
        let e = &mut self.rows[self.row_head as usize];
        e[0..2].copy_from_slice(&((r.now / 60_000) as u16).to_le_bytes());
        e[2..4].copy_from_slice(&(d(t.tx, l.tx).min(0xFFFF) as u16).to_le_bytes());
        e[4..6].copy_from_slice(&(d(t.rx, l.rx).min(0xFFFF) as u16).to_le_bytes());
        e[6] = d(t.retries, l.retries).min(255) as u8;
        e[7] = d(t.fails, l.fails).min(255) as u8;
        e[8] = (r.coord_cost & 7)
            | (r.coord_measured as u8) << 3
            | ((r.coord_hop == 0x0000) as u8) << 4
            | (d(t.route_disc, l.route_disc).min(7) as u8) << 5;
        e[9] = r.coord_delivery;
        e[10] = r.stale;
        e[11] = r.noise as u8;
        self.last = t;
        self.row_head = ((self.row_head as usize + 1) % ROWS) as u16;
        self.row_len = (self.row_len + 1).min(ROWS as u16);
    }

    fn byte_at(&self, off: usize, shorts: &[u16; NEIGHBORS]) -> u8 {
        let (ring_head, ring_len, row_head, row_len, link_head, link_len, now) = self.frozen;
        if off < HEADER {
            let mut h = [0u8; HEADER];
            h[0] = VERSION;
            h[1] = NEIGHBORS as u8;
            h[2] = LINK_ENTRY as u8;
            h[3] = RING as u8;
            h[4] = ring_len;
            h[5] = RING_ENTRY as u8;
            h[6..8].copy_from_slice(&(ROWS as u16).to_le_bytes());
            h[8..10].copy_from_slice(&row_len.to_le_bytes());
            h[10] = ROW_ENTRY as u8;
            h[11] = (ROW_MS / 1000 / 10) as u8;
            h[12..16].copy_from_slice(&now.to_le_bytes());
            h[16..20].copy_from_slice(&self.ring_written.to_le_bytes());
            h[20..22].copy_from_slice(&(BOX_LEN as u16).to_le_bytes());
            h[22] = LINK_FRAMES as u8;
            return h[off];
        }
        if off < RING_AT {
            let (slot, o) = ((off - LINKS_AT) / LINK_ENTRY, (off - LINKS_AT) % LINK_ENTRY);
            let len = link_len[slot] as usize;
            return match o {
                0 => shorts[slot] as u8,
                1 => (shorts[slot] >> 8) as u8,
                2 => len as u8,
                3 => 0,
                _ => {
                    let nib = |i: usize| -> u8 {
                        if i >= len {
                            return 0;
                        }
                        let at = (link_head[slot] as usize + LINK_FRAMES - len + i) % LINK_FRAMES;
                        let b = self.links[slot][at / 2];
                        if at % 2 == 0 { b & 0x0F } else { b >> 4 }
                    };
                    let i = (o - 4) * 2;
                    nib(i) | nib(i + 1) << 4
                }
            };
        }
        if off < ROWS_AT {
            let (i, o) = ((off - RING_AT) / RING_ENTRY, (off - RING_AT) % RING_ENTRY);
            if i >= ring_len as usize {
                return 0;
            }
            let at = (ring_head as usize + RING - ring_len as usize + i) % RING;
            return self.ring[at][o];
        }
        if off < BOX_LEN {
            let (i, o) = ((off - ROWS_AT) / ROW_ENTRY, (off - ROWS_AT) % ROW_ENTRY);
            if i >= row_len as usize {
                return 0;
            }
            let at = (row_head as usize + ROWS - row_len as usize + i) % ROWS;
            return self.rows[at][o];
        }
        0
    }

    pub(crate) fn page(&mut self, page: u16, now: u32, shorts: &[u16; NEIGHBORS], out: &mut [u8; 64]) -> usize {
        if page == 0 {
            self.frozen = (self.ring_head, self.ring_len, self.row_head, self.row_len, self.link_head, self.link_len, now);
            self.hold_until = Some(now.wrapping_add(HOLD_MS));
        }
        let from = page as usize * BOX_PAGE;
        let n = BOX_PAGE.min(BOX_LEN - from);
        if from + n == BOX_LEN {
            self.hold_until = None;
        }
        for (i, o) in out[..n].iter_mut().enumerate() {
            *o = self.byte_at(from + i, shorts);
        }
        n
    }
}
