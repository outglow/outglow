use super::aps::{ENDPOINT, PROFILE_HA, PROFILE_ZDO};
use super::Stack;
use crate::hal::{Hal, NvItem};
use crate::nwk;
use crate::util::{due, pack, Buf, Reader};

pub(crate) const OTA: u16 = 0x0019;

const IMAGE_NOTIFY: u8 = 0x00;
const QUERY_NEXT_IMAGE_REQ: u8 = 0x01;
const QUERY_NEXT_IMAGE_RSP: u8 = 0x02;
const IMAGE_BLOCK_REQ: u8 = 0x03;
const IMAGE_BLOCK_RSP: u8 = 0x05;
const UPGRADE_END_REQ: u8 = 0x06;
const UPGRADE_END_RSP: u8 = 0x07;

const STATUS_SUCCESS: u8 = 0x00;
const STATUS_ABORT: u8 = 0x95;
const STATUS_INVALID_IMAGE: u8 = 0x96;
const STATUS_WAIT_FOR_DATA: u8 = 0x97;

const FILE_MAGIC: u32 = 0x0BEE_F11E;
const TAG_IMAGE: u16 = 0x0000;
const BLOCK: u8 = 48;
const RETRY_MS: u32 = 2_500;
const RETRIES: u8 = 8;
const DISCOVERY_MS: u32 = 5_000;
const END_WAIT_MS: u32 = 20_000;
const END_RETRIES: u8 = 3;
const HELD_PERIOD_MS: u32 = 3_600_000;
const MAX_WAIT_S: u32 = 1_728_000;
const FIRST_QUERY_MS: u32 = 30_000;
const QUERY_PERIOD_MS: u32 = 86_400_000;
const _: () = assert!(HELD_PERIOD_MS == 3600 * 1000 && MAX_WAIT_S == 20 * 24 * 3600 && QUERY_PERIOD_MS == 24 * 3600 * 1000, "an hour, twenty days, a day");
const CHECKPOINT: u32 = 0x1000;
const PROGRESS_VERSION: u8 = 1;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OtaState {
    Idle,
    Querying,
    Downloading,
    Finishing,
    Waiting,
    Held,
}

pub(crate) struct Ota {
    pub state: OtaState,
    pub at: u32,
    tries: u8,
    server: Option<(u16, u8)>,
    discovery: u8,
    seq: u8,
    version: u32,
    file_size: u32,
    offset: u32,
    skip: u32,
    element_left: u32,
    in_image: bool,
    head: [u8; 8],
    head_len: u8,
    seen_file_header: bool,
    image_len: u32,
    crc: u32,
    tail: [u8; 4],
    tail_len: u8,
    declared: u32,
    marker_ok: bool,
}

impl Ota {
    pub(crate) fn new(now: u32) -> Ota {
        Ota {
            state: OtaState::Idle,
            at: now.wrapping_add(FIRST_QUERY_MS),
            tries: 0,
            server: None,
            discovery: 0,
            seq: 0,
            version: 0,
            file_size: 0,
            offset: 0,
            skip: 0,
            element_left: 0,
            in_image: false,
            head: [0; 8],
            head_len: 0,
            seen_file_header: false,
            image_len: 0,
            crc: 0xFFFF_FFFF,
            tail: [0; 4],
            tail_len: 0,
            declared: 0,
            marker_ok: true,
        }
    }
}

fn crc32_update(mut crc: u32, byte: u8) -> u32 {
    const TABLE: [u32; 16] = [
        0x0000_0000, 0x1DB7_1064, 0x3B6E_20C8, 0x26D9_30AC, 0x76DC_4190, 0x6B6B_51F4, 0x4DB2_6158, 0x5005_713C,
        0xEDB8_8320, 0xF00F_9344, 0xD6D6_A3E8, 0xCB61_B38C, 0x9B64_C2B0, 0x86D3_D2D4, 0xA00A_E278, 0xBDBD_F21C,
    ];
    crc ^= byte as u32;
    crc = (crc >> 4) ^ TABLE[(crc & 0x0F) as usize];
    (crc >> 4) ^ TABLE[(crc & 0x0F) as usize]
}

impl<H: Hal> Stack<H> {
    pub fn ota_state(&self) -> OtaState {
        self.ota.state
    }

    fn ota_send(&mut self, cmd: u8, payload: &[u8]) {
        let mut f = Buf::new();
        f.u8(0x11);
        self.ota.seq = self.ota.seq.wrapping_add(1);
        f.u8(self.ota.seq);
        f.u8(cmd);
        f.bytes(payload);
        if let Some((server, ep)) = self.ota.server {
            self.aps_data(server, ep, OTA, PROFILE_HA, ENDPOINT, f.as_slice());
        }
    }

    fn ota_image_id(&self, version: u32) -> Buf {
        let mut b = Buf::new();
        b.u16(self.identity.manufacturer_code);
        b.u16(self.identity.image_type);
        b.u32(version);
        b
    }

    fn ota_query(&mut self, now: u32) {
        self.ota.state = OtaState::Querying;
        if self.ota.server.is_none() {
            self.ota.discovery = 1;
            self.ota_discover(0x0000);
            self.ota.at = now.wrapping_add(DISCOVERY_MS);
            return;
        }
        self.ota.discovery = 0;
        let mut p = Buf::new();
        p.u8(0x00);
        p.bytes(self.ota_image_id(self.identity.file_version).as_slice());
        self.ota_send(QUERY_NEXT_IMAGE_REQ, p.as_slice());
        self.ota.at = now.wrapping_add(RETRY_MS * 4);
    }

    fn ota_discover(&mut self, dst: u16) {
        let seq = self.zdo_seq.wrapping_add(1);
        self.zdo_seq = seq;
        let (d, o) = (dst.to_le_bytes(), OTA.to_le_bytes());
        self.aps_data(dst, 0, 0x0006, PROFILE_ZDO, 0, &[seq, d[0], d[1], 0x04, 0x01, 1, o[0], o[1], 0]);
    }

    pub(crate) fn ota_server_found(&mut self, src: u16, endpoint: u8) {
        if self.ota.state == OtaState::Querying && self.ota.discovery != 0 {
            self.ota.server = Some((src, endpoint));
            let now = self.hal.now_ms();
            self.ota_query(now);
        }
    }

    fn ota_upgrade_end(&mut self, status: u8) {
        let mut p = Buf::new();
        p.u8(status);
        p.bytes(self.ota_image_id(self.ota.version).as_slice());
        self.ota_send(UPGRADE_END_REQ, p.as_slice());
    }

    fn ota_request_block(&mut self, now: u32) {
        let mut p = Buf::new();
        p.u8(0x00);
        p.bytes(self.ota_image_id(self.ota.version).as_slice());
        p.u32(self.ota.offset);
        p.u8(BLOCK);
        self.ota_send(IMAGE_BLOCK_REQ, p.as_slice());
        self.ota.at = now.wrapping_add(RETRY_MS);
    }

    fn ota_idle(&mut self, now: u32) {
        self.ota.state = OtaState::Idle;
        self.ota.discovery = 0;
        self.ota.at = now.wrapping_add(QUERY_PERIOD_MS);
    }

    pub(crate) fn ota_timers(&mut self, now: u32) {
        if !due(now, self.ota.at) {
            return;
        }
        match self.ota.state {
            OtaState::Idle => self.ota_query(now),
            OtaState::Querying => match self.ota.discovery {
                1 => {
                    self.ota.discovery = 2;
                    self.ota_discover(nwk::BROADCAST_RX_ON);
                    self.ota.at = now.wrapping_add(DISCOVERY_MS);
                }
                _ => {
                    self.ota.server = None;
                    self.ota_idle(now);
                }
            },
            OtaState::Finishing => {
                self.ota.tries += 1;
                if self.ota.tries > END_RETRIES {
                    self.ota.state = OtaState::Waiting;
                } else {
                    self.ota_upgrade_end(STATUS_SUCCESS);
                    self.ota.at = now.wrapping_add(END_WAIT_MS);
                }
            }
            OtaState::Held => {
                self.ota.state = OtaState::Finishing;
                self.ota.tries = 0;
                self.ota_upgrade_end(STATUS_SUCCESS);
                self.ota.at = now.wrapping_add(END_WAIT_MS);
            }
            OtaState::Downloading => {
                self.ota.tries += 1;
                if self.ota.tries > RETRIES {
                    self.ota_idle(now);
                } else {
                    self.ota_request_block(now);
                }
            }
            OtaState::Waiting => {
                self.ota_forget();
                self.diag_retain_now();
                self.hal.ota_activate();
                self.ota_idle(now);
            }
        }
    }

    pub(crate) fn on_ota(&mut self, src: u16, src_ep: u8, unicast: bool, cmd: u8, mut r: Reader) -> Option<()> {
        let now = self.hal.now_ms();
        match cmd {
            IMAGE_NOTIFY => {
                if self.ota.state == OtaState::Idle {
                    if unicast {
                        self.ota.server = Some((src, src_ep));
                    }
                    self.ota.at = now.wrapping_add(100 + self.hal.random() % 400);
                }
            }
            QUERY_NEXT_IMAGE_RSP if self.ota.state == OtaState::Querying => {
                if r.u8()? != STATUS_SUCCESS {
                    self.ota_idle(now);
                    return None;
                }
                let (manufacturer, image_type, version, size) = (r.u16()?, r.u16()?, r.u32()?, r.u32()?);
                let ours = manufacturer == self.identity.manufacturer_code && image_type == self.identity.image_type;
                if !ours || version == self.identity.file_version {
                    self.ota_idle(now);
                    return None;
                }
                let endpoint = self.ota.server.map_or(src_ep, |s| s.1);
                self.ota = Ota::new(now);
                self.ota.server = Some((src, endpoint));
                if !self.ota_restore(version, size) {
                    if !self.hal.ota_begin(size) {
                        self.ota_idle(now);
                        return None;
                    }
                    self.ota_forget();
                    self.ota.version = version;
                    self.ota.file_size = size;
                }
                self.ota.state = OtaState::Downloading;
                self.ota_request_block(now);
            }
            IMAGE_BLOCK_RSP if self.ota.state == OtaState::Downloading => {
                match r.u8()? {
                    STATUS_SUCCESS => {}
                    STATUS_WAIT_FOR_DATA => {
                        let (current, request) = (r.u32()?, r.u32()?);
                        self.ota.tries = 0;
                        self.ota.at = now.wrapping_add(request.saturating_sub(current).clamp(1, MAX_WAIT_S) * 1000);
                        return None;
                    }
                    STATUS_ABORT => {
                        self.ota_idle(now);
                        return None;
                    }
                    _ => return None,
                }
                let (_, _, version, offset, size) = (r.u16()?, r.u16()?, r.u32()?, r.u32()?, r.u8()?);
                let data = r.take(size as usize)?;
                if version != self.ota.version || offset != self.ota.offset || data.is_empty() {
                    return None;
                }
                self.ota.tries = 0;
                self.ota.offset += data.len() as u32;
                let before = self.ota.image_len;
                let ok = self.ota_feed(data);
                if ok && self.ota.image_len / CHECKPOINT != before / CHECKPOINT {
                    self.ota_checkpoint();
                }
                if !ok {
                    self.ota_finish(now, false);
                } else if self.ota.offset >= self.ota.file_size {
                    let good = self.ota_image_good();
                    self.ota_finish(now, good);
                } else {
                    self.ota_request_block(now);
                }
            }
            UPGRADE_END_RSP if matches!(self.ota.state, OtaState::Finishing | OtaState::Held | OtaState::Waiting) => {
                let (manufacturer, image_type, version, current, upgrade) = (r.u16()?, r.u16()?, r.u32()?, r.u32()?, r.u32()?);
                let ours = (manufacturer == self.identity.manufacturer_code || manufacturer == 0xFFFF)
                    && (image_type == self.identity.image_type || image_type == 0xFFFF)
                    && (version == self.ota.version || version == 0xFFFF_FFFF);
                if !ours {
                    return None;
                }
                self.ota.tries = 0;
                let wait = upgrade.saturating_sub(current);
                if upgrade == 0xFFFF_FFFF {
                    self.ota.state = OtaState::Held;
                    self.ota.at = now.wrapping_add(HELD_PERIOD_MS);
                } else if wait > MAX_WAIT_S {
                    self.ota.state = OtaState::Held;
                    self.ota.at = now.wrapping_add(MAX_WAIT_S * 1000);
                } else {
                    self.ota.state = OtaState::Waiting;
                    self.ota.at = now.wrapping_add(wait * 1000 + 500);
                }
            }
            _ => {}
        }
        Some(())
    }

    fn ota_checkpoint(&mut self) {
        let o = &self.ota;
        let mut b = Buf::new();
        b.u8(PROGRESS_VERSION);
        b.u32(o.version);
        b.u32(o.file_size);
        b.u32(o.offset);
        b.u32(o.skip);
        b.u32(o.element_left);
        b.u8(pack(&[(o.in_image as u16, 0), (o.seen_file_header as u16, 1), (o.marker_ok as u16, 2)]) as u8);
        b.bytes(&o.head);
        b.u8(o.head_len);
        b.u32(o.image_len);
        b.u32(o.crc);
        b.bytes(&o.tail);
        b.u8(o.tail_len);
        b.u32(o.declared);
        self.nv_put(NvItem::OtaProgress, b.as_slice());
    }

    fn ota_forget(&mut self) {
        let mut stored = [0u8; 64];
        if self.hal.nv_load(NvItem::OtaProgress, &mut stored).is_some_and(|_| stored[0] == PROGRESS_VERSION) {
            self.nv_put(NvItem::OtaProgress, &[0]);
        }
    }

    fn ota_restore(&mut self, version: u32, size: u32) -> bool {
        let mut raw = [0u8; 64];
        let Some(len) = self.hal.nv_load(NvItem::OtaProgress, &mut raw) else {
            return false;
        };
        let restored = (|| {
            let mut r = Reader::new(raw.get(..len)?);
            if r.u8()? != PROGRESS_VERSION || r.u32()? != version || r.u32()? != size {
                return None;
            }
            let o = &mut self.ota;
            o.offset = r.u32()?;
            o.skip = r.u32()?;
            o.element_left = r.u32()?;
            let flags = r.u8()?;
            o.in_image = flags & 1 != 0;
            o.seen_file_header = flags & 2 != 0;
            o.marker_ok = flags & 4 != 0;
            o.head.copy_from_slice(r.take(8)?);
            o.head_len = r.u8()?;
            o.image_len = r.u32()?;
            o.crc = r.u32()?;
            o.tail.copy_from_slice(r.take(4)?);
            o.tail_len = r.u8()?;
            o.declared = r.u32()?;
            Some(())
        })();
        if restored.is_none() || !self.hal.ota_resume(self.ota.image_len) {
            let server = self.ota.server;
            let now = self.hal.now_ms();
            self.ota = Ota::new(now);
            self.ota.server = server;
            return false;
        }
        self.ota.version = version;
        self.ota.file_size = size;
        true
    }

    fn ota_finish(&mut self, now: u32, good: bool) {
        self.ota_upgrade_end(if good { STATUS_SUCCESS } else { STATUS_INVALID_IMAGE });
        if good {
            self.ota.state = OtaState::Finishing;
            self.ota.tries = 0;
            self.ota.at = now.wrapping_add(END_WAIT_MS);
        } else {
            self.ota_forget();
            self.ota_idle(now);
        }
    }

    fn ota_feed(&mut self, mut data: &[u8]) -> bool {
        while !data.is_empty() {
            let o = &mut self.ota;
            if o.skip > 0 {
                let n = (o.skip as usize).min(data.len());
                o.skip -= n as u32;
                data = &data[n..];
            } else if o.element_left > 0 {
                let n = (o.element_left as usize).min(data.len());
                o.element_left -= n as u32;
                if o.in_image {
                    self.ota_image_bytes(&data[..n]);
                }
                data = &data[n..];
            } else {
                let want = if o.seen_file_header { 6 } else { 8 };
                o.head[o.head_len as usize] = data[0];
                o.head_len += 1;
                data = &data[1..];
                if o.head_len < want {
                    continue;
                }
                o.head_len = 0;
                let h = o.head;
                if !o.seen_file_header {
                    let magic = u32::from_le_bytes([h[0], h[1], h[2], h[3]]);
                    let header_len = u16::from_le_bytes([h[6], h[7]]) as u32;
                    if magic != FILE_MAGIC || header_len < 56 {
                        return false;
                    }
                    o.seen_file_header = true;
                    o.skip = header_len - 8;
                } else {
                    let tag = u16::from_le_bytes([h[0], h[1]]);
                    o.element_left = u32::from_le_bytes([h[2], h[3], h[4], h[5]]);
                    o.in_image = tag == TAG_IMAGE && o.image_len == 0;
                }
            }
        }
        true
    }

    fn ota_image_bytes(&mut self, data: &[u8]) {
        let mut chunk = [0u8; 64];
        let mut done = 0;
        while done < data.len() {
            let n = (data.len() - done).min(chunk.len());
            chunk[..n].copy_from_slice(&data[done..done + n]);
            let base = self.ota.image_len;
            for (i, b) in chunk[..n].iter_mut().enumerate() {
                let at = base + i as u32;
                let o = &mut self.ota;
                if (0x18..0x1C).contains(&at) {
                    o.declared |= (*b as u32) << (8 * (at - 0x18));
                }
                if (8..12).contains(&at) && *b != b"KNLT"[(at - 8) as usize] {
                    o.marker_ok = false;
                }
                if o.tail_len == 4 {
                    o.crc = crc32_update(o.crc, o.tail[0]);
                    o.tail.rotate_left(1);
                    o.tail[3] = *b;
                } else {
                    o.tail[o.tail_len as usize] = *b;
                    o.tail_len += 1;
                }
                if at == 8 {
                    *b = 0xFF;
                }
            }
            self.hal.ota_write(base, &chunk[..n]);
            self.ota.image_len += n as u32;
            done += n;
        }
    }

    fn ota_image_good(&self) -> bool {
        let o = &self.ota;
        o.image_len >= 0x20
            && o.skip == 0
            && o.element_left == 0
            && o.marker_ok
            && o.declared == o.image_len
            && o.tail_len == 4
            && u32::from_le_bytes(o.tail) == o.crc
    }
}
