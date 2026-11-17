use sense::probe::{self, Heard};
use sense::{Core, LINES, NO_SRC};

use super::aps::{ENDPOINT, PROFILE_HA};
use super::{Stack, State};
use crate::hal::{Hal, NvItem, TxStatus};
use crate::util::{due, Long};

pub(crate) const OCCUPANCY: u16 = 0x0406;
pub(crate) const SENSE: u16 = 0xFC5E;

pub(crate) const A_OCCUPANCY: u16 = 0x0000;
const A_SENSOR_TYPE: u16 = 0x0001;
const A_SENSOR_TYPE_BITMAP: u16 = 0x0002;
const A_OFF_DELAY: u16 = 0x0010;
const A_SENSITIVITY: u16 = 0x0000;
const A_RATE: u16 = 0x0001;
const A_LINES: u16 = 0x0002;
const A_CALIBRATE: u16 = 0x0003;
const A_SELF_TUNE: u16 = 0x0004;
const A_LINE_SENS: u16 = 0x0005;
const A_AUTO_CAL: u16 = 0x0006;
pub(crate) const A_ZONE: u16 = 0xFF43;
pub(crate) const A_LINE_STATE: u16 = 0xFF46;
const REVISION: u16 = 0xFFFD;

const T_BITMAP8: u8 = 0x18;
const T_BITMAP16: u8 = 0x19;
const T_U8: u8 = 0x20;
const T_U16: u8 = 0x21;
const T_ENUM8: u8 = 0x30;
const T_OCTETS: u8 = 0x41;

pub const DEFAULT_S: u8 = 40;
pub const DEFAULT_RATE: u8 = 0;
pub const ON_RATE: u8 = 5;
pub const DEFAULT_OFF_S: u16 = 30;
pub const MAX_RATE: u8 = 10;
pub const HOLDOFF_MS: u32 = 60_000;
pub const REPORT_MIN_MS: u32 = 500;
pub const HEARTBEAT_MS: u32 = 30 * 60 * 1000;
const NV_VERSION: u8 = 3;
const NV_HEAD: usize = 7;
const NV_LEN: usize = NV_HEAD + 6 * LINES;
const NV_HEAD_2: usize = 6;
const NV_LEN_2: usize = NV_HEAD_2 + 6 * LINES;

const OCC_ATTRS: [(u16, u8, bool); 5] = [
    (A_OCCUPANCY, T_BITMAP8, false),
    (A_SENSOR_TYPE, T_ENUM8, false),
    (A_SENSOR_TYPE_BITMAP, T_BITMAP8, false),
    (A_OFF_DELAY, T_U16, true),
    (REVISION, T_U16, false),
];
const SENSE_ATTRS: [(u16, u8, bool); 10] = [
    (A_SENSITIVITY, T_U8, true),
    (A_RATE, T_U8, true),
    (A_LINES, T_OCTETS, false),
    (A_CALIBRATE, T_U8, true),
    (A_SELF_TUNE, T_U8, true),
    (A_LINE_SENS, T_OCTETS, false),
    (A_AUTO_CAL, T_U8, true),
    (A_ZONE, T_U8, false),
    (A_LINE_STATE, T_BITMAP16, false),
    (REVISION, T_U16, false),
];

pub(crate) fn attributes(cluster: u16) -> &'static [(u16, u8, bool)] {
    if cluster == OCCUPANCY {
        &OCC_ATTRS
    } else {
        &SENSE_ATTRS
    }
}

pub(crate) fn attribute(cluster: u16, id: u16) -> Option<(u8, bool)> {
    attributes(cluster).iter().find(|a| a.0 == id).map(|a| (a.1, a.2))
}

pub(crate) struct Sense {
    pub core: Core,
    pub rate: u8,
    started: u32,
    next_probe: u32,
    seq: u8,
    light: Option<(bool, u8, u16)>,
    light_changes: u8,
    reported: u16,
    last_report: u32,
    pub drained: bool,
    paused: bool,
}

impl Sense {
    pub(crate) fn new(now: u32) -> Sense {
        let mut core = Core::new(now, DEFAULT_S, DEFAULT_OFF_S);
        core.auto_cal = true;
        Sense {
            core,
            rate: DEFAULT_RATE,
            started: now,
            next_probe: now.wrapping_add(HOLDOFF_MS),
            seq: 0,
            light: None,
            light_changes: 0,
            reported: 0,
            last_report: now,
            drained: false,
            paused: false,
        }
    }

    fn defaults(&mut self, now: u32) {
        self.core = Core::new(now, DEFAULT_S, DEFAULT_OFF_S);
        self.core.auto_cal = true;
        self.rate = DEFAULT_RATE;
    }
}

impl<H: Hal> Stack<H> {
    pub fn sense_on(&self) -> bool {
        self.identity.sense && self.sense.rate != 0
    }

    fn sense_listen(&mut self) {
        let pan = if self.sense_on() { probe::PAN } else { 0xFFFF };
        self.hal.sense_listen(pan);
    }

    pub fn sense_lines_on(&self) -> u16 {
        self.sense.reported
    }

    pub fn sense_core(&self) -> &Core {
        &self.sense.core
    }

    pub fn sense_pause(&mut self, paused: bool) {
        self.sense.paused = paused;
    }

    pub(crate) fn sense_load(&mut self, now: u32) {
        if !self.identity.sense {
            return;
        }
        self.sense_load_record(now);
        self.sense_listen();
    }

    fn sense_load_record(&mut self, now: u32) {
        let mut raw = [0u8; NV_LEN];
        let head = match self.hal.nv_load(NvItem::Sense, &mut raw) {
            Some(NV_LEN) if raw[0] == NV_VERSION => NV_HEAD,
            Some(NV_LEN_2) if raw[0] == 2 => NV_HEAD_2,
            _ => return,
        };
        self.sense.core.cfg(raw[1].min(100), u16::from_le_bytes([raw[3], raw[4]]).max(1));
        self.sense.core.selftune = raw[5] != 0;
        self.sense.core.auto_cal = head == NV_HEAD_2 || raw[6] != 0;
        self.sense.rate = raw[2].min(MAX_RATE);
        for l in raw[head..].chunks_exact(6) {
            let src = u16::from_le_bytes([l[0], l[1]]);
            if src != NO_SRC {
                self.sense.core.line_restore(now, src, l[2], l[3], u16::from_le_bytes([l[4], l[5]]));
            }
        }
        self.sense.core.dirty = false;
    }

    fn sense_store(&mut self) {
        let mut b = [0xFFu8; NV_LEN];
        let c = &self.sense.core;
        b[0] = NV_VERSION;
        b[1] = c.s;
        b[2] = self.sense.rate;
        b[3..5].copy_from_slice(&c.off_s.to_le_bytes());
        b[5] = c.selftune as u8;
        b[6] = c.auto_cal as u8;
        for (i, l) in c.line.iter().enumerate() {
            let o = NV_HEAD + 6 * i;
            b[o..o + 2].copy_from_slice(&l.src.to_le_bytes());
            b[o + 2] = l.s;
            b[o + 3] = l.anchor_s;
            b[o + 4..o + 6].copy_from_slice(&l.anchor_noise.to_le_bytes());
        }
        self.nv_put(NvItem::Sense, &b);
    }

    pub(crate) fn sense_forget(&mut self) {
        let now = self.hal.now_ms();
        self.sense.defaults(now);
        self.sense.reported = 0;
        if self.identity.sense {
            self.sense_listen();
        }
    }

    pub(crate) fn sense_reset(&mut self) {
        if self.identity.sense {
            if self.sense.reported != 0 {
                self.sense_report(0);
            }
            self.sense_forget();
            self.sense_store();
        }
    }

    pub(crate) fn sense_heard(&mut self, mpdu: &[u8], rssi: i8) -> bool {
        if !self.sense_on() {
            return false;
        }
        let pan = if self.state == State::Up { self.nib.pan } else { 0xFFFF };
        match probe::classify(mpdu, pan) {
            Heard::Probe { src, light_changes } => {
                if src != self.nib.short {
                    let now = self.hal.now_ms();
                    self.sense.core.frame(now, src, rssi, Some(light_changes));
                }
                true
            }
            Heard::Frame { src } => {
                let now = self.hal.now_ms();
                self.sense.core.frame(now, src, rssi, None);
                false
            }
            Heard::Foreign => true,
            Heard::Other => false,
        }
    }

    pub(crate) fn sense_light(&mut self, on: bool, level: u8, color_temp: u16) {
        if !self.identity.sense {
            return;
        }
        let now_light = (on, level, color_temp);
        match self.sense.light {
            Some(l) if l == now_light => {}
            Some(_) => {
                self.sense.light = Some(now_light);
                self.sense.light_changes = self.sense.light_changes.wrapping_add(1);
                let now = self.hal.now_ms();
                self.sense.core.blank(now);
            }
            None => self.sense.light = Some(now_light),
        }
    }

    pub(crate) fn sense_timers(&mut self, now: u32) {
        if !self.sense_on() {
            return;
        }
        let mask = self.sense.core.tick(now);
        if self.sense.core.dirty {
            self.sense.core.dirty = false;
            self.sense_store();
        }
        let since = now.wrapping_sub(self.sense.last_report);
        if since >= HEARTBEAT_MS || (mask != self.sense.reported && since >= REPORT_MIN_MS) {
            self.sense.reported = mask;
            self.sense.last_report = now;
            self.sense_report(mask);
        }
        self.sense_probe(now);
    }

    fn sense_report(&mut self, mask: u16) {
        let occupied = (mask != 0) as u8;
        self.zcl_seq = self.zcl_seq.wrapping_add(1);
        let occ = [0x18, self.zcl_seq, 0x0A, A_OCCUPANCY as u8, (A_OCCUPANCY >> 8) as u8, T_BITMAP8, occupied];
        self.aps_data(0x0000, ENDPOINT, OCCUPANCY, PROFILE_HA, ENDPOINT, &occ);
        self.zcl_seq = self.zcl_seq.wrapping_add(1);
        let code = self.identity.manufacturer_code.to_le_bytes();
        let (z, l, m) = (A_ZONE.to_le_bytes(), A_LINE_STATE.to_le_bytes(), mask.to_le_bytes());
        let mfr = [0x1C, code[0], code[1], self.zcl_seq, 0x0A, z[0], z[1], T_U8, occupied, l[0], l[1], T_BITMAP16, m[0], m[1]];
        self.aps_data(0x0000, ENDPOINT, SENSE, PROFILE_HA, ENDPOINT, &mfr);
    }

    fn sense_probe(&mut self, now: u32) {
        if !due(now, self.sense.next_probe) {
            return;
        }
        let allowed = self.sense.rate != 0
            && now.wrapping_sub(self.sense.started) >= HOLDOFF_MS
            && matches!(self.ota_state(), super::OtaState::Idle | super::OtaState::Querying)
            && !self.sense.paused;
        if !allowed {
            self.sense.next_probe = now.wrapping_add(200);
            return;
        }
        if !self.sense.drained {
            self.sense.next_probe = now.wrapping_add(2 + (self.hal.random() & 7));
            return;
        }
        let (on, level, _) = self.sense.light.unwrap_or((false, 0, 0));
        let info = probe::Info { light_on: on, level, light_changes: self.sense.light_changes, rate: self.sense.rate };
        let p = probe::build(self.sense.seq, self.nib.short, self.nib.pan, &info);
        match self.hal.transmit(&p) {
            TxStatus::Ok => {
                self.sense.seq = self.sense.seq.wrapping_add(1);
                let period = 1000 / self.sense.rate as u32;
                let jitter = self.hal.random() % ((period >> 2) + 1);
                self.sense.next_probe = now.wrapping_add(period - (period >> 3) + jitter);
            }
            _ => self.sense.next_probe = now.wrapping_add(2 + (self.hal.random() & 7)),
        }
    }

    pub(crate) fn sense_cluster(&self, ep: u8, cluster: u16) -> bool {
        self.identity.sense && ep == ENDPOINT && matches!(cluster, OCCUPANCY | SENSE)
    }

    pub(crate) fn sense_reportable(&self, cluster: u16, id: u16) -> bool {
        self.identity.sense && cluster == OCCUPANCY && id == A_OCCUPANCY
    }

    pub(crate) fn sense_long(&self, cluster: u16, id: u16) -> bool {
        self.identity.sense && cluster == SENSE && matches!(id, A_LINES | A_LINE_SENS)
    }

    pub(crate) fn sense_value(&mut self, cluster: u16, id: u16, out: &mut Long) {
        let reported = self.sense.reported;
        let now = self.hal.now_ms();
        let c = &self.sense.core;
        match (cluster, id) {
            (OCCUPANCY, A_OCCUPANCY) => out.u8((reported != 0) as u8),
            (OCCUPANCY, A_SENSOR_TYPE) => out.u8(0),
            (OCCUPANCY, A_SENSOR_TYPE_BITMAP) => out.u8(0x01),
            (OCCUPANCY, A_OFF_DELAY) => out.u16(c.off_s),
            (OCCUPANCY, REVISION) => out.u16(2),
            (SENSE, A_SENSITIVITY) => out.u8(c.s),
            (SENSE, A_RATE) => out.u8(self.sense.rate),
            (SENSE, A_LINES) => {
                let n = c.line.iter().filter(|l| l.used()).count();
                out.u8((6 * n) as u8);
                for (i, l) in c.line.iter().enumerate().filter(|(_, l)| l.used()) {
                    out.u16(l.src);
                    out.bytes(&[l.rssi as u8, l.noise, l.sum, l.on() as u8 | (i as u8) << 4]);
                }
            }
            (SENSE, A_CALIBRATE) => out.u8(c.cal_left(now).min(255) as u8),
            (SENSE, A_SELF_TUNE) => out.u8(c.selftune as u8),
            (SENSE, A_AUTO_CAL) => out.u8(c.auto_cal as u8),
            (SENSE, A_LINE_SENS) => {
                let n = c.line.iter().filter(|l| l.used()).count();
                out.u8((4 * n) as u8);
                for l in c.line.iter().filter(|l| l.used()) {
                    out.u16(l.src);
                    out.bytes(&[l.s, l.anchor_s]);
                }
            }
            (SENSE, A_ZONE) => out.u8((reported != 0) as u8),
            (SENSE, A_LINE_STATE) => out.u16(reported),
            _ => out.u16(1),
        }
    }

    pub(crate) fn sense_write_status(&self, cluster: u16, id: u16, ty: u8, value: &[u8]) -> u8 {
        let Some((expected, writable)) = attribute(cluster, id) else {
            return 0x86;
        };
        if ty != expected {
            return 0x8D;
        }
        if !writable {
            return 0x88;
        }
        let v = value.iter().rev().fold(0u32, |v, b| v << 8 | *b as u32);
        let valid = match (cluster, id) {
            (OCCUPANCY, A_OFF_DELAY) => v >= 1,
            (SENSE, A_SENSITIVITY) => v <= 100,
            (SENSE, A_RATE) => v <= MAX_RATE as u32,
            (SENSE, A_SELF_TUNE) => v <= 1,
            (SENSE, A_AUTO_CAL) => v <= 1,
            _ => v == 0 || self.sense.rate != 0,
        };
        if valid {
            0x00
        } else {
            0x87
        }
    }

    pub(crate) fn sense_write(&mut self, cluster: u16, id: u16, value: &[u8]) {
        let v = value.iter().rev().fold(0u32, |v, b| v << 8 | *b as u32);
        let now = self.hal.now_ms();
        let c = &mut self.sense.core;
        match (cluster, id) {
            (OCCUPANCY, A_OFF_DELAY) => {
                let s = c.s;
                c.cfg(s, v as u16);
            }
            (SENSE, A_SENSITIVITY) => {
                let off = c.off_s;
                c.cfg(v as u8, off);
                c.auto_cal = false;
            }
            (SENSE, A_AUTO_CAL) => c.auto_cal = v != 0,
            (SENSE, A_RATE) => {
                let was = self.sense.rate;
                self.sense.rate = v as u8;
                self.sense.next_probe = now;
                if (was == 0) != (v == 0) {
                    self.sense_switched(now, v != 0);
                }
            }
            (SENSE, A_SELF_TUNE) => c.selftune = v != 0,
            _ => {
                c.cal_start(now, v as u16);
                return;
            }
        }
        self.sense_store();
    }

    fn sense_switched(&mut self, now: u32, on: bool) {
        self.sense.core.start(now);
        self.sense.last_report = now;
        if !on && self.sense.reported != 0 {
            self.sense.reported = 0;
            self.sense_report(0);
        }
        self.sense_listen();
    }
}
