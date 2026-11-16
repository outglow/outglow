//! Motion sensing from RSSI (feature `sense`; docs/rf-sensing.md): the
//! probes, the samples, the detector (`sense::Core`), the Occupancy Sensing
//! cluster and the manufacturer cluster 0xFC5E, the reports and the stored
//! configuration. What the C firmware does in `firmware/sense/eglo_sense.c`,
//! on this stack:
//!
//! - samples are taken from every frame the radio hands up, before the MAC
//!   layer looks at it; probes go no further;
//! - a probe is sent from the main loop between the stack's own frames, when
//!   the radio's receive queue is empty (the MAC has nothing in hand), with
//!   CSMA-CA and no acknowledgement; it uses no buffer of the stack;
//! - the reports go to the coordinator (0x0000, endpoint 1) without a
//!   binding, as the C firmware sends them, but without an APS
//!   acknowledgement: no report of this stack asks for one. A lost report is
//!   put right by the next change or by the half-hourly one.
//!
//! Only a stack whose `Identity::sense` is set has any of this; with the
//! feature built in and the flag clear it behaves as without the feature.
//!
//! Out of the box the sensing is off (rate 0, `DEFAULT_RATE`): the clusters
//! are there, but no probe goes out, the radio takes none, no frame is a
//! sample, the detector stands still and nothing is reported; Occupancy
//! reads 0. On the air the bulb is then what it is without the feature. A
//! rate written from ZCL switches it on (`ON_RATE` is what a switch
//! writes); the detector's hours, and the first day of its automatic
//! calibration, count from then (`Core::start`).

use sense::probe::{self, Heard};
use sense::{Core, LINES, NO_SRC};

use super::aps::{ENDPOINT, PROFILE_HA};
use super::{Stack, State};
use crate::hal::{Hal, NvItem, TxStatus};
use crate::util::{due, Long, Reader};

/// Occupancy Sensing.
pub(crate) const OCCUPANCY: u16 = 0x0406;
/// The manufacturer cluster of the sensing (code `Identity::manufacturer_code`).
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

/// Out of the box: sensitivity, probes per second, seconds until OFF. 40,
/// not the C firmware's 45: on the links measured between the owner's
/// bulbs 45 takes their spread for motion (crates/sense/tests/measured.rs);
/// the automatic calibration sets every line's own after the first day.
/// The rate 0: the sensing is off until it is switched on.
pub const DEFAULT_S: u8 = 40;
pub const DEFAULT_RATE: u8 = 0;
/// The rate that switches the sensing on (the C firmware's default, and
/// this one's before the sensing was in every image).
pub const ON_RATE: u8 = 5;
pub const DEFAULT_OFF_S: u16 = 30;
pub const MAX_RATE: u8 = 10;
/// No probes this long after the start (as the C firmware).
pub const HOLDOFF_MS: u32 = 60_000;
/// Reports: at a change, not more often than this, and every half hour.
pub const REPORT_MIN_MS: u32 = 500;
pub const HEARTBEAT_MS: u32 = 30 * 60 * 1000;
/// Version of the stored record (`NvItem::Sense`). A record is written only
/// when an attribute is written or the detector learnt something, so a bulb
/// that never had the sensing on has none and comes up off; one that ran an
/// image with the sensing on by default (built with `--sense`, before it
/// was in every image) keeps the rate it stored, 5 unless someone changed it.
const NV_VERSION: u8 = 3;
/// The record: version, s, rate, OFF delay (2), self-tuning, automatic
/// calibration; per line the sender (2), s, anchor s, anchor noise (2).
const NV_HEAD: usize = 7;
const NV_LEN: usize = NV_HEAD + 6 * LINES;

/// The attributes of the two clusters, in ascending order: id, type, writable.
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

/// The attribute table of a sensing cluster.
pub(crate) fn attributes(cluster: u16) -> &'static [(u16, u8, bool)] {
    match cluster {
        OCCUPANCY => &OCC_ATTRS,
        SENSE => &SENSE_ATTRS,
        _ => &[],
    }
}

/// The type of an attribute of a sensing cluster, and whether it may be written.
pub(crate) fn attribute(cluster: u16, id: u16) -> Option<(u8, bool)> {
    attributes(cluster).iter().find(|a| a.0 == id).map(|a| (a.1, a.2))
}

/// The sensing's state in the stack.
pub(crate) struct Sense {
    pub core: Core,
    /// Probes per second; 0: the sensing is off.
    pub rate: u8,
    /// When the stack was made: no probes for `HOLDOFF_MS` after.
    started: u32,
    next_probe: u32,
    seq: u8,
    /// The light as last told (on, level, colour temperature), and how often it changed.
    light: Option<(bool, u8, u16)>,
    light_changes: u8,
    /// The lines that are ON, as last reported, and when.
    reported: u16,
    last_report: u32,
    /// The radio's receive queue was found empty in this poll.
    pub drained: bool,
    /// An update over BLE is running (`Stack::sense_pause`).
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

    /// Back to the defaults, lines and their calibration forgotten (the clock stays).
    fn defaults(&mut self, now: u32) {
        self.core = Core::new(now, DEFAULT_S, DEFAULT_OFF_S);
        self.core.auto_cal = true;
        self.rate = DEFAULT_RATE;
    }
}

impl<H: Hal> Stack<H> {
    /// The sensing runs: this bulb probes, takes samples and reports.
    pub fn sense_on(&self) -> bool {
        self.identity.sense && self.sense.rate != 0
    }

    /// The radio takes the probes only while the sensing runs.
    fn sense_listen(&mut self) {
        let pan = if self.sense_on() { probe::PAN } else { 0xFFFF };
        self.hal.sense_listen(pan);
    }

    /// The ON lines as last reported (the attributes show that state).
    pub fn sense_lines_on(&self) -> u16 {
        self.sense.reported
    }

    /// The detector, for tests and diagnostics.
    pub fn sense_core(&self) -> &Core {
        &self.sense.core
    }

    /// While the firmware updates itself over BLE no probe goes out.
    pub fn sense_pause(&mut self, paused: bool) {
        self.sense.paused = paused;
    }

    /// At the start: the radio is told to take probes, and the stored
    /// configuration and line sensitivities come back.
    pub(crate) fn sense_load(&mut self, now: u32) {
        if !self.identity.sense {
            return;
        }
        self.sense_load_record(now);
        self.sense_listen();
    }

    fn sense_load_record(&mut self, now: u32) {
        let mut raw = [0u8; NV_LEN];
        // version 2 (before the automatic calibration): one octet less, the automatic calibration on
        let (len, head) = match self.hal.nv_load(NvItem::Sense, &mut raw) {
            Some(n) if n == NV_LEN && raw[0] == NV_VERSION => (n, NV_HEAD),
            Some(n) if n == NV_LEN - 1 && raw[0] == 2 => (n, NV_HEAD - 1),
            _ => return,
        };
        let mut r = Reader::new(&raw[1..len]);
        let (Some(s), Some(rate), Some(off), Some(tune)) = (r.u8(), r.u8(), r.u16(), r.u8()) else {
            return;
        };
        let auto = if head == NV_HEAD { r.u8() } else { Some(1) };
        self.sense.core.cfg(s.min(100), off.max(1));
        self.sense.core.selftune = tune != 0;
        self.sense.core.auto_cal = auto != Some(0);
        self.sense.rate = rate.min(MAX_RATE);
        for _ in 0..LINES {
            let (Some(src), Some(s), Some(anchor_s), Some(anchor_noise)) = (r.u16(), r.u8(), r.u8(), r.u16()) else {
                break;
            };
            if src != NO_SRC {
                self.sense.core.line_restore(now, src, s, anchor_s, anchor_noise);
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

    /// Leaving the network: the configuration went with the store, and the
    /// lines with the network.
    pub(crate) fn sense_forget(&mut self) {
        let now = self.hal.now_ms();
        self.sense.defaults(now);
        self.sense.reported = 0;
        if self.identity.sense {
            self.sense_listen();
        }
    }

    /// Reset to Factory Defaults (Basic): the attributes back to what they
    /// were out of the box, the calibration of the lines with them.
    pub(crate) fn sense_reset(&mut self) {
        if self.identity.sense {
            // off again: the coordinator is told the occupancy is over
            if self.sense.reported != 0 {
                self.sense_report(0);
            }
            self.sense_forget();
            self.sense_store();
        }
    }

    /// A received frame: a sample if it is a probe or a frame of our
    /// network. True when it is a probe (of ours or another's), which the
    /// stack does not see.
    pub(crate) fn sense_heard(&mut self, mpdu: &[u8], rssi: i8) -> bool {
        if !self.sense_on() {
            // off: no samples; and the radio takes no probes then, so nothing
            // but what reaches a bulb without the sensing comes here
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

    /// The light changed (`set_light`): that moves the signal too, so the
    /// lines start again, and the neighbours learn of it from the probes.
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
            // the light the firmware starts with is no change
            None => self.sense.light = Some(now_light),
        }
    }

    pub(crate) fn sense_timers(&mut self, now: u32) {
        if !self.sense_on() {
            // off: the detector stands still, nothing is reported
            return;
        }
        let mask = self.sense.core.tick(now);
        if self.sense.core.dirty {
            // calibration or self-tuning changed a line's s
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
            // frames are waiting: they go first
            self.sense.next_probe = now.wrapping_add(2 + (self.hal.random() & 7));
            return;
        }
        let (on, level, _) = self.sense.light.unwrap_or((false, 0, 0));
        let info = probe::Info { light_on: on, level, light_changes: self.sense.light_changes, rate: self.sense.rate };
        let p = probe::build(self.sense.seq, self.nib.short, self.nib.pan, &info);
        match self.hal.transmit(&p) {
            TxStatus::Ok => {
                self.sense.seq = self.sense.seq.wrapping_add(1);
                // period with +-12 % jitter, so that the probes of different bulbs do not synchronise
                let period = 1000 / self.sense.rate as u32;
                let jitter = self.hal.random() % ((period >> 2) + 1);
                self.sense.next_probe = now.wrapping_add(period - (period >> 3) + jitter);
            }
            // channel busy: again in 2..9 ms
            _ => self.sense.next_probe = now.wrapping_add(2 + (self.hal.random() & 7)),
        }
    }

    // ------------------------------------------------------------ ZCL

    /// A sensing cluster on the light's endpoint of this device.
    pub(crate) fn sense_cluster(&self, ep: u8, cluster: u16) -> bool {
        self.identity.sense && ep == ENDPOINT && matches!(cluster, OCCUPANCY | SENSE)
    }

    /// Occupancy is reportable (ZCL 8 4.8.2.2.1.1, access RP): a coordinator may configure
    /// its reports like those of the attributes outside the light's three
    /// (`objects::XReport`); they go to the bindings, as configured, whether
    /// the sensing runs or not. The sensing's own reports (`sense_report`)
    /// are another matter: they go only while it runs.
    pub(crate) fn sense_reportable(&self, cluster: u16, id: u16) -> bool {
        self.identity.sense && cluster == OCCUPANCY && id == A_OCCUPANCY
    }

    /// An attribute whose value can be long (the line tables).
    pub(crate) fn sense_long(&self, cluster: u16, id: u16) -> bool {
        self.identity.sense && cluster == SENSE && matches!(id, A_LINES | A_LINE_SENS)
    }

    pub(crate) fn sense_value(&mut self, cluster: u16, id: u16, out: &mut Long) {
        let reported = self.sense.reported;
        let now = self.hal.now_ms();
        let c = &self.sense.core;
        match (cluster, id) {
            (OCCUPANCY, A_OCCUPANCY) => out.u8((reported != 0) as u8),
            // 0: PIR, as the C firmware (the cluster knows no RF sensor), and its bit in the bitmap
            (OCCUPANCY, A_SENSOR_TYPE) => out.u8(0),
            (OCCUPANCY, A_SENSOR_TYPE_BITMAP) => out.u8(0x01),
            (OCCUPANCY, A_OFF_DELAY) => out.u16(c.off_s),
            (OCCUPANCY, REVISION) => out.u16(2),
            (SENSE, A_SENSITIVITY) => out.u8(c.s),
            (SENSE, A_RATE) => out.u8(self.sense.rate),
            (SENSE, A_LINES) => {
                // per line: sender, RSSI [dBm], noise [1/8 dB], window sum, state (bit 0 ON, bits 4-7 the line)
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
            (SENSE, REVISION) => out.u16(1),
            _ => {}
        }
    }

    /// What stands against a write (type, access, range); 0 (SUCCESS) if nothing.
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
            // a recording needs the sensing on (0, an abort, is always fine)
            (SENSE, A_CALIBRATE) => v == 0 || self.sense.rate != 0,
            _ => true,
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
            // a new default s overrides the sensitivity of all lines, and the
            // automatic calibration with it: a sensitivity given by hand stays
            (SENSE, A_SENSITIVITY) => {
                let off = c.off_s;
                c.cfg(v as u8, off);
                c.auto_cal = false;
            }
            (SENSE, A_AUTO_CAL) => c.auto_cal = v != 0,
            (SENSE, A_RATE) => {
                let was = self.sense.rate;
                self.sense.rate = v as u8;
                // the new period from now on
                self.sense.next_probe = now;
                if (was == 0) != (v == 0) {
                    self.sense_switched(now, v != 0);
                }
            }
            (SENSE, A_SELF_TUNE) => c.selftune = v != 0,
            // N seconds of empty-room recording; 0 aborts it
            (SENSE, A_CALIBRATE) => {
                c.cal_start(now, v as u16);
                return;
            }
            _ => return,
        }
        self.sense_store();
    }

    /// The sensing switched on or off (the rate from or to 0).
    fn sense_switched(&mut self, now: u32, on: bool) {
        // the detector's hours, and the first day of the automatic
        // calibration with them, from now: it does not see the time the
        // sensing is off; a recording that was running is abandoned
        self.sense.core.start(now);
        self.sense.last_report = now;
        if !on && self.sense.reported != 0 {
            // the coordinator is told the occupancy is over, and then nothing more is sent
            self.sense.reported = 0;
            self.sense_report(0);
        }
        self.sense_listen();
    }
}
