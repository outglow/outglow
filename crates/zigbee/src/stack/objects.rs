use super::diag;
use super::Stack;
use crate::hal::{Hal, NvItem};
use crate::util::{due, Reader};

pub(crate) const ANALOG_INPUT: u16 = 0x000C;
pub(crate) const ANALOG_OUTPUT: u16 = 0x000D;
pub(crate) const BINARY_OUTPUT: u16 = 0x0010;

pub(crate) const A_DESCRIPTION: u16 = 0x001C;
pub(crate) const A_MAX_PRESENT: u16 = 0x0041;
pub(crate) const A_MIN_PRESENT: u16 = 0x0045;
pub(crate) const A_OUT_OF_SERVICE: u16 = 0x0051;
pub(crate) const A_PRESENT_VALUE: u16 = 0x0055;
pub(crate) const A_RESOLUTION: u16 = 0x006A;
pub(crate) const A_STATUS_FLAGS: u16 = 0x006F;
pub(crate) const A_ENGINEERING_UNITS: u16 = 0x0075;
pub(crate) const A_APPLICATION_TYPE: u16 = 0x0100;

pub(crate) const T_BOOL: u8 = 0x10;
pub(crate) const T_BITMAP8: u8 = 0x18;
pub(crate) const T_U32: u8 = 0x23;
pub(crate) const T_ENUM16: u8 = 0x31;
pub(crate) const T_SINGLE: u8 = 0x39;
pub(crate) const T_STRING: u8 = 0x42;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Value {
    RelaysLost,
    MacRetries,
    BleShare,
    Neighbors,
    Children,
    AllowChildren,
    BleWindow,
}

pub(crate) struct Object {
    pub ep: u8,
    pub cluster: u16,
    pub value: Value,
    pub description: &'static str,
    pub units: Option<u16>,
    pub app_type: Option<u32>,
    pub min: f32,
    pub max: f32,
    pub resolution: f32,
}

const UNIT_PERCENT: u16 = 98;
const UNIT_MINUTES: u16 = 72;
const APP_COUNT: u32 = 0x000C_0000;
const APP_PERCENT: u32 = 0x0004_0000;
const COUNT_MAX: f32 = 16_777_216.0;
pub(crate) const BLE_WINDOW_MAX: u16 = 1440;

pub(crate) const OBJECTS: [Object; 7] = [
    Object { ep: 2, cluster: ANALOG_INPUT, value: Value::RelaysLost, description: "Relayed frames lost", units: None, app_type: Some(APP_COUNT),
             min: 0.0, max: COUNT_MAX, resolution: 1.0 },
    Object { ep: 3, cluster: ANALOG_INPUT, value: Value::MacRetries, description: "MAC retries per APS message", units: None, app_type: None,
             min: 0.0, max: 12.0, resolution: 0.01 },
    Object { ep: 4, cluster: ANALOG_INPUT, value: Value::BleShare, description: "Radio time lent to BLE", units: Some(UNIT_PERCENT),
             app_type: Some(APP_PERCENT), min: 0.0, max: 100.0, resolution: 0.1 },
    Object { ep: 5, cluster: ANALOG_INPUT, value: Value::Neighbors, description: "Router neighbours", units: None, app_type: Some(APP_COUNT),
             min: 0.0, max: super::NEIGHBORS as f32, resolution: 1.0 },
    Object { ep: 6, cluster: ANALOG_INPUT, value: Value::Children, description: "End-device children", units: None, app_type: Some(APP_COUNT),
             min: 0.0, max: super::CHILDREN as f32, resolution: 1.0 },
    Object { ep: 7, cluster: BINARY_OUTPUT, value: Value::AllowChildren, description: "Allow end devices to join", units: None, app_type: None,
             min: 0.0, max: 1.0, resolution: 1.0 },
    Object { ep: 8, cluster: ANALOG_OUTPUT, value: Value::BleWindow, description: "BLE window after power-on", units: Some(UNIT_MINUTES),
             app_type: None, min: 0.0, max: BLE_WINDOW_MAX as f32, resolution: 1.0 },
];

pub(crate) const ENDPOINTS: [u8; 7] = [2, 3, 4, 5, 6, 7, 8];
const DEVICE_SIMPLE_SENSOR: u16 = 0x000C;
const DEVICE_CONFIGURATION_TOOL: u16 = 0x0005;

pub(crate) fn object(ep: u8) -> Option<&'static Object> {
    OBJECTS.iter().find(|o| o.ep == ep)
}

pub(crate) fn device_id(o: &Object) -> u16 {
    if o.cluster == ANALOG_INPUT {
        DEVICE_SIMPLE_SENSOR
    } else {
        DEVICE_CONFIGURATION_TOOL
    }
}

pub(crate) fn attributes(o: &Object, out: &mut [(u16, u8); 10]) -> usize {
    let pv = if o.cluster == BINARY_OUTPUT { T_BOOL } else { T_SINGLE };
    let mut n = 0;
    let mut add = |id: u16, ty: u8| {
        out[n] = (id, ty);
        n += 1;
    };
    add(A_DESCRIPTION, T_STRING);
    if o.cluster != BINARY_OUTPUT {
        add(A_MAX_PRESENT, T_SINGLE);
        add(A_MIN_PRESENT, T_SINGLE);
    }
    add(A_OUT_OF_SERVICE, T_BOOL);
    add(A_PRESENT_VALUE, pv);
    if o.cluster != BINARY_OUTPUT {
        add(A_RESOLUTION, T_SINGLE);
    }
    add(A_STATUS_FLAGS, T_BITMAP8);
    if o.units.is_some() {
        add(A_ENGINEERING_UNITS, T_ENUM16);
    }
    if o.app_type.is_some() {
        add(A_APPLICATION_TYPE, T_U32);
    }
    add(super::zcl::CLUSTER_REVISION, 0x21);
    n
}

pub(crate) fn attribute_type(o: &Object, id: u16) -> Option<u8> {
    let mut a = [(0u16, 0u8); 10];
    let n = attributes(o, &mut a);
    a[..n].iter().find(|x| x.0 == id).map(|x| x.1)
}

#[derive(Clone, Copy)]
pub(crate) struct XReport {
    pub ep: u8,
    pub cluster: u16,
    pub attr: u16,
    pub mfr: bool,
    pub min_s: u16,
    pub max_s: u16,
    pub change: u32,
    pub last: u32,
    pub last_sent: u32,
}

impl XReport {
    pub(crate) const EMPTY: XReport = XReport { ep: 0, cluster: 0, attr: 0, mfr: false, min_s: 0, max_s: 0, change: 0, last: 0, last_sent: 0 };
}

pub(crate) const XREPORTS: usize = 11;
const REPORTS_VERSION: u8 = 1;
const XREPORT_LEN: usize = 14;
const _: () = assert!(1 + XREPORTS * XREPORT_LEN <= 160, "the reporting configurations fit one record");

pub(crate) fn reportable(ep: u8, cluster: u16, attr: u16, mfr: bool) -> bool {
    if cluster == super::zcl::DIAGNOSTICS && ep == super::aps::ENDPOINT {
        return diag::attribute(mfr, attr).is_some_and(|a| a.0 != diag::T_OCTETS);
    }
    !mfr && object(ep).is_some_and(|o| o.cluster == cluster) && matches!(attr, A_PRESENT_VALUE | A_STATUS_FLAGS)
}

impl<H: Hal> Stack<H> {
    pub(crate) fn object_present(&mut self, o: &Object) -> u32 {
        let now = self.hal.now_ms();
        let v: f32 = match o.value {
            Value::RelaysLost => self.diag.relays_lost() as f32,
            Value::MacRetries => self.diag.average_retries_x100() as f32 / 100.0,
            Value::BleShare => {
                self.diag_sync();
                let up = self.diag.uptime_s(now).max(1);
                let ms = self.diag.ble_ms_since_start();
                (ms as u64 / up as u64).min(1000) as f32 / 10.0
            }
            Value::Neighbors => self.live_neighbors() as f32,
            Value::Children => self.child_count() as f32,
            Value::AllowChildren => return self.allow_children as u32,
            Value::BleWindow => match self.ble_mode {
                super::BleMode::Window => self.ble_window as f32,
                super::BleMode::On | super::BleMode::Off => 0.0,
            },
        };
        v.to_bits()
    }

    pub(crate) fn xreport_value(&mut self, r: &XReport) -> Option<(u32, u8)> {
        if r.ep == super::aps::ENDPOINT && !r.mfr && self.colour_reportable(r.cluster, r.attr) {
            let l = &self.light;
            return Some(match r.attr {
                0x0000 => (l.hue as u32, diag::T_U8),
                0x0001 => (l.sat as u32, diag::T_U8),
                0x0003 => (l.x as u32, diag::T_U16),
                0x0004 => (l.y as u32, diag::T_U16),
                _ => (l.color_mode as u32, diag::T_ENUM8),
            });
        }
        #[cfg(feature = "sense")]
        if r.ep == super::aps::ENDPOINT && !r.mfr && self.sense_reportable(r.cluster, r.attr) {
            return Some(((self.sense_lines_on() != 0) as u32, T_BITMAP8));
        }
        if r.cluster == super::zcl::DIAGNOSTICS {
            let (ty, src) = diag::attribute(r.mfr, r.attr)?;
            return Some((self.diag_value(src), ty));
        }
        let o = object(r.ep)?;
        match r.attr {
            A_PRESENT_VALUE => Some((self.object_present(o), if o.cluster == BINARY_OUTPUT { T_BOOL } else { T_SINGLE })),
            A_STATUS_FLAGS => Some((0, T_BITMAP8)),
            _ => None,
        }
    }

    pub(crate) fn xreport_configure(&mut self, ep: u8, cluster: u16, attr: u16, mfr: bool, min_s: u16, max_s: u16, change: u32) -> Option<()> {
        let at = self.xreports.iter().position(|r| r.ep == ep && r.cluster == cluster && r.attr == attr && r.mfr == mfr);
        let now = self.hal.now_ms();
        if max_s == 0xFFFF || (min_s, max_s) == (0xFFFF, 0) {
            if let Some(i) = at {
                self.xreports[i] = XReport::EMPTY;
                self.xreports_dirty = true;
            }
            return Some(());
        }
        let i = at.or_else(|| self.xreports.iter().position(|r| r.ep == 0))?;
        let mut r = XReport { ep, cluster, attr, mfr, min_s, max_s, change, last: 0, last_sent: now };
        r.last = self.xreport_value(&r).map_or(0, |v| v.0);
        self.xreports[i] = r;
        self.xreports_dirty = true;
        Some(())
    }

    pub(crate) fn xreport_find(&self, ep: u8, cluster: u16, attr: u16, mfr: bool) -> Option<XReport> {
        self.xreports.iter().find(|r| r.ep == ep && r.cluster == cluster && r.attr == attr && r.mfr == mfr).copied()
    }

    pub(crate) fn xreport_timers(&mut self, now: u32) {
        for i in 0..XREPORTS {
            let r = self.xreports[i];
            if r.ep == 0 {
                continue;
            }
            let since = now.wrapping_sub(r.last_sent);
            if since < r.min_s as u32 * 1000 {
                continue;
            }
            let Some((value, ty)) = self.xreport_value(&r) else {
                continue;
            };
            let changed = match ty {
                T_SINGLE => {
                    let (a, b) = (f32::from_bits(value), f32::from_bits(r.last));
                    let d = if a > b { a - b } else { b - a };
                    let step = f32::from_bits(r.change);
                    a != b && d >= if step > 0.0 { step } else { 0.0 }
                }
                T_BOOL | T_BITMAP8 | diag::T_ENUM8 => value != r.last,
                _ => value != r.last && value.wrapping_sub(r.last).min(r.last.wrapping_sub(value)) >= r.change.max(1),
            };
            let periodic = r.max_s != 0 && since >= r.max_s as u32 * 1000;
            if !(changed || periodic) {
                continue;
            }
            self.xreports[i].last_sent = now;
            self.xreports[i].last = value;
            self.send_report(r.ep, r.cluster, r.mfr, r.attr, ty, value);
        }
    }

    pub(crate) fn store_xreports(&mut self) {
        let mut b = [0u8; 1 + XREPORTS * XREPORT_LEN];
        b[0] = REPORTS_VERSION;
        let mut n = 1;
        for r in self.xreports.iter().filter(|r| r.ep != 0) {
            b[n] = r.ep;
            b[n + 1] = r.mfr as u8;
            b[n + 2..n + 4].copy_from_slice(&r.cluster.to_le_bytes());
            b[n + 4..n + 6].copy_from_slice(&r.attr.to_le_bytes());
            b[n + 6..n + 8].copy_from_slice(&r.min_s.to_le_bytes());
            b[n + 8..n + 10].copy_from_slice(&r.max_s.to_le_bytes());
            b[n + 10..n + 14].copy_from_slice(&r.change.to_le_bytes());
            n += XREPORT_LEN;
        }
        self.nv_put(NvItem::Reports, &b[..n]);
    }

    pub(crate) fn load_xreports(&mut self, now: u32) {
        let mut raw = [0u8; 1 + XREPORTS * XREPORT_LEN];
        let Some(len) = self.hal.nv_load(NvItem::Reports, &mut raw) else {
            return;
        };
        let mut r = Reader::new(&raw[..len.min(raw.len())]);
        if r.u8() != Some(REPORTS_VERSION) {
            return;
        }
        for slot in self.xreports.iter_mut() {
            let (Some(ep), Some(flags), Some(cluster), Some(attr), Some(min_s), Some(max_s), Some(change)) =
                (r.u8(), r.u8(), r.u16(), r.u16(), r.u16(), r.u16(), r.u32())
            else {
                break;
            };
            *slot = XReport { ep, cluster, attr, mfr: flags & 1 != 0, min_s, max_s, change, last: 0, last_sent: now };
        }
        for i in 0..XREPORTS {
            let r = self.xreports[i];
            if r.ep != 0 {
                self.xreports[i].last = self.xreport_value(&r).map_or(0, |v| v.0);
            }
        }
    }

    pub(crate) fn object_write(&mut self, o: &Object, value: &[u8]) {
        match o.value {
            Value::AllowChildren => {
                self.allow_children = value.first().is_some_and(|b| *b != 0);
                self.tables_dirty = true;
            }
            Value::BleWindow => {
                let mut v = [0u8; 4];
                v.copy_from_slice(value.get(..4).unwrap_or(&[0; 4]));
                let minutes = (f32::from_le_bytes(v) + 0.5) as u16;
                if minutes == 0 {
                    self.ble_mode = super::BleMode::On;
                } else {
                    self.ble_mode = super::BleMode::Window;
                    self.ble_window = minutes;
                }
                self.store_settings();
            }
            _ => {}
        }
    }

    pub(crate) fn object_value_ok(o: &Object, value: &[u8]) -> bool {
        match o.value {
            Value::AllowChildren => value.first().is_some_and(|b| *b <= 1),
            Value::BleWindow => {
                let mut v = [0u8; 4];
                v.copy_from_slice(value.get(..4).unwrap_or(&[0; 4]));
                let f = f32::from_le_bytes(v);
                f >= o.min && f <= o.max && f == (f as u16) as f32
            }
            _ => false,
        }
    }

    pub fn allows_children(&self) -> bool {
        self.allow_children
    }

    pub(crate) fn xreports_due(&mut self, now: u32) {
        if self.xreports_dirty {
            self.xreports_dirty = false;
            self.store_xreports();
        }
        if due(now, self.xreport_at) {
            self.xreport_at = now.wrapping_add(1000);
            self.xreport_timers(now);
        }
    }
}
