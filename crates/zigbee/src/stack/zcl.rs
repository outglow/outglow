use bulb_core::command::{Command, HueDirection, PowerOnBehavior};

use super::aps::{ENDPOINT, PROFILE_HA};
use super::{diag, objects};
#[cfg(feature = "sense")]
use super::sense::{self, OCCUPANCY, SENSE};
use super::{BleMode, Scene, Stack, State, BLE_WINDOW_DEFAULT_MIN, BLE_WINDOW_MAX_MIN, GROUPS, REPORT_COLOR_TEMP, REPORT_LEVEL, REPORT_ON_OFF};
use crate::hal::Hal;
use crate::util::{due, Buf, Long, Reader};

pub(crate) const BASIC: u16 = 0x0000;
pub(crate) const IDENTIFY: u16 = 0x0003;
pub(crate) const GROUPS_CLUSTER: u16 = 0x0004;
pub(crate) const SCENES_CLUSTER: u16 = 0x0005;
pub(crate) const ON_OFF: u16 = 0x0006;
pub(crate) const LEVEL: u16 = 0x0008;
pub(crate) const COLOR: u16 = 0x0300;

pub(crate) const DIAGNOSTICS: u16 = 0x0B05;
pub(crate) const TOUCHLINK: u16 = 0x1000;
pub(crate) const SETTINGS: u16 = 0xFC5F;

const CLUSTERS: [u16; 8] = [BASIC, IDENTIFY, GROUPS_CLUSTER, SCENES_CLUSTER, ON_OFF, LEVEL, DIAGNOSTICS, TOUCHLINK];
const CLUSTERS_TW: [u16; 9] = [BASIC, IDENTIFY, GROUPS_CLUSTER, SCENES_CLUSTER, ON_OFF, LEVEL, COLOR, DIAGNOSTICS, TOUCHLINK];
const CLUSTERS_BLE: [u16; 9] = [BASIC, IDENTIFY, GROUPS_CLUSTER, SCENES_CLUSTER, ON_OFF, LEVEL, DIAGNOSTICS, TOUCHLINK, SETTINGS];
const CLUSTERS_TW_BLE: [u16; 10] =
    [BASIC, IDENTIFY, GROUPS_CLUSTER, SCENES_CLUSTER, ON_OFF, LEVEL, COLOR, DIAGNOSTICS, TOUCHLINK, SETTINGS];
#[cfg(feature = "sense")]
const CLUSTERS_SENSE: [u16; 10] = [BASIC, IDENTIFY, GROUPS_CLUSTER, SCENES_CLUSTER, ON_OFF, LEVEL, DIAGNOSTICS, TOUCHLINK, OCCUPANCY, SENSE];
#[cfg(feature = "sense")]
const CLUSTERS_TW_SENSE: [u16; 11] =
    [BASIC, IDENTIFY, GROUPS_CLUSTER, SCENES_CLUSTER, ON_OFF, LEVEL, COLOR, DIAGNOSTICS, TOUCHLINK, OCCUPANCY, SENSE];
#[cfg(feature = "sense")]
const CLUSTERS_BLE_SENSE: [u16; 11] =
    [BASIC, IDENTIFY, GROUPS_CLUSTER, SCENES_CLUSTER, ON_OFF, LEVEL, DIAGNOSTICS, TOUCHLINK, SETTINGS, OCCUPANCY, SENSE];
#[cfg(feature = "sense")]
const CLUSTERS_TW_BLE_SENSE: [u16; 12] =
    [BASIC, IDENTIFY, GROUPS_CLUSTER, SCENES_CLUSTER, ON_OFF, LEVEL, COLOR, DIAGNOSTICS, TOUCHLINK, SETTINGS, OCCUPANCY, SENSE];
pub(crate) const COLOR_OPTIONS_DEFAULT: u8 = EXECUTE_IF_OFF;
const COUPLE_COLOR_TEMP: u8 = 0x02;
const DEVICE_DIMMABLE_LIGHT: u16 = 0x0101;
const DEVICE_COLOR_TEMPERATURE_LIGHT: u16 = 0x010C;
const DEVICE_EXTENDED_COLOR_LIGHT: u16 = 0x010D;
const COLOR_CAPABILITIES_FULL: u16 = 0x0019;
const COLOR_COMMANDS_FULL: [u8; 14] = [0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x47, 0x4B, 0x4C];

const SUCCESS: u8 = 0x00;
const MALFORMED_COMMAND: u8 = 0x80;
const UNSUP_COMMAND: u8 = 0x81;
const UNSUPPORTED_ATTRIBUTE: u8 = 0x86;
const INVALID_FIELD: u8 = 0x85;
const INVALID_VALUE: u8 = 0x87;
const READ_ONLY: u8 = 0x88;
const INSUFFICIENT_SPACE: u8 = 0x89;
const NOT_FOUND: u8 = 0x8B;
const UNREPORTABLE_ATTRIBUTE: u8 = 0x8C;
const INVALID_DATA_TYPE: u8 = 0x8D;
const UNSUPPORTED_CLUSTER: u8 = 0xC3;

const T_BOOL: u8 = 0x10;
const T_BITMAP8: u8 = 0x18;
const T_U8: u8 = 0x20;
const T_BITMAP16: u8 = 0x19;
const T_U16: u8 = 0x21;
const T_ENUM8: u8 = 0x30;
const T_OCTETS: u8 = 0x41;
const T_STRING: u8 = 0x42;

const READ_ATTRIBUTES: u8 = 0x00;
const READ_ATTRIBUTES_RSP: u8 = 0x01;
const EXECUTE_IF_OFF: u8 = 0x01;
const WRITE_ATTRIBUTES: u8 = 0x02;
const WRITE_ATTRIBUTES_UNDIVIDED: u8 = 0x03;
const WRITE_ATTRIBUTES_RSP: u8 = 0x04;
const WRITE_ATTRIBUTES_NO_RSP: u8 = 0x05;
const CONFIGURE_REPORTING: u8 = 0x06;
const CONFIGURE_REPORTING_RSP: u8 = 0x07;
const READ_REPORTING_CONFIG: u8 = 0x08;
const READ_REPORTING_CONFIG_RSP: u8 = 0x09;
const REPORT_ATTRIBUTES: u8 = 0x0A;
const DEFAULT_RSP: u8 = 0x0B;
const DISCOVER_ATTRIBUTES: u8 = 0x0C;
const DISCOVER_ATTRIBUTES_RSP: u8 = 0x0D;
const DISCOVER_COMMANDS_RECEIVED: u8 = 0x11;
const DISCOVER_COMMANDS_GENERATED: u8 = 0x13;
const DISCOVER_ATTRIBUTES_EXTENDED: u8 = 0x15;
const DISCOVER_ATTRIBUTES_EXTENDED_RSP: u8 = 0x16;

const FC_GLOBAL_RSP: u8 = 0x18;
const FC_CLUSTER_RSP: u8 = 0x19;

pub(crate) const CLUSTER_REVISION: u16 = 0xFFFD;
const ZCL_VERSION: u8 = 8;
const GENERIC_TYPE_LED_BULB: u8 = 0x05;
const MIN_LEVEL: u8 = bulb_core::LEVEL_MIN;
pub(crate) const SCENE_NO_ON_OFF: u8 = 0x01;
pub(crate) const SCENE_NO_LEVEL: u8 = 0x02;
pub(crate) const SCENE_NO_COLOR: u8 = 0x04;
const DEFAULT_MOVE_RATE: u32 = 50;

const ATTRIBUTES: [(u16, u16, u8); 60] = [
    (BASIC, 0x0000, T_U8),
    (BASIC, 0x0001, T_U8),
    (BASIC, 0x0002, T_U8),
    (BASIC, 0x0003, T_U8),
    (BASIC, 0x0004, T_STRING),
    (BASIC, 0x0005, T_STRING),
    (BASIC, 0x0006, T_STRING),
    (BASIC, 0x0007, T_ENUM8),
    (BASIC, 0x0008, T_ENUM8),
    (BASIC, 0x0009, T_ENUM8),
    (BASIC, 0x000A, T_OCTETS),
    (BASIC, 0x000B, T_STRING),
    (BASIC, 0x0012, T_BOOL),
    (BASIC, 0x4000, T_STRING),
    (BASIC, CLUSTER_REVISION, T_U16),
    (IDENTIFY, 0x0000, T_U16),
    (IDENTIFY, CLUSTER_REVISION, T_U16),
    (GROUPS_CLUSTER, 0x0000, T_BITMAP8),
    (GROUPS_CLUSTER, CLUSTER_REVISION, T_U16),
    (SCENES_CLUSTER, 0x0000, T_U8),
    (SCENES_CLUSTER, 0x0001, T_U8),
    (SCENES_CLUSTER, 0x0002, T_U16),
    (SCENES_CLUSTER, 0x0003, T_BOOL),
    (SCENES_CLUSTER, 0x0004, T_BITMAP8),
    (SCENES_CLUSTER, CLUSTER_REVISION, T_U16),
    (ON_OFF, 0x0000, T_BOOL),
    (ON_OFF, 0x4000, T_BOOL),
    (ON_OFF, 0x4001, T_U16),
    (ON_OFF, 0x4002, T_U16),
    (ON_OFF, 0x4003, T_ENUM8),
    (ON_OFF, CLUSTER_REVISION, T_U16),
    (LEVEL, 0x0000, T_U8),
    (LEVEL, 0x0001, T_U16),
    (LEVEL, 0x0002, T_U8),
    (LEVEL, 0x0003, T_U8),
    (LEVEL, 0x000F, T_BITMAP8),
    (LEVEL, 0x0011, T_U8),
    (LEVEL, 0x4000, T_U8),
    (LEVEL, CLUSTER_REVISION, T_U16),
    (COLOR, 0x0000, T_U8),
    (COLOR, 0x0001, T_U8),
    (COLOR, 0x0002, T_U16),
    (COLOR, 0x0003, T_U16),
    (COLOR, 0x0004, T_U16),
    (COLOR, 0x0007, T_U16),
    (COLOR, 0x0008, T_ENUM8),
    (COLOR, 0x000F, T_BITMAP8),
    (COLOR, 0x0010, T_U8),
    (COLOR, 0x4001, T_ENUM8),
    (COLOR, 0x400A, T_BITMAP16),
    (COLOR, 0x400B, T_U16),
    (COLOR, 0x400C, T_U16),
    (COLOR, 0x400D, T_U16),
    (COLOR, 0x4010, T_U16),
    (COLOR, CLUSTER_REVISION, T_U16),
    (TOUCHLINK, CLUSTER_REVISION, T_U16),
    (SETTINGS, 0x0000, T_ENUM8),
    (SETTINGS, 0x0001, T_U16),
    (SETTINGS, 0x0002, T_BOOL),
    (SETTINGS, CLUSTER_REVISION, T_U16),
];

fn execute_if_off(attribute: u8, r: &mut Reader) -> bool {
    let (mask, over) = match (r.u8(), r.u8()) {
        (Some(mask), Some(over)) => (mask, over),
        _ => (0, 0),
    };
    (attribute & !mask | over & mask) & EXECUTE_IF_OFF != 0
}

#[derive(Clone, Copy)]
struct Ctx {
    ep: u8,
    mfr: Option<u16>,
}

fn put_number(out: &mut Long, ty: u8, v: u32) {
    let n = match ty {
        0x08 | 0x10 | 0x18 | 0x20 | 0x28 | 0x30 => 1,
        0x09 | 0x19 | 0x21 | 0x29 | 0x31 => 2,
        _ => 4,
    };
    out.bytes(&v.to_le_bytes()[..n]);
}

fn writable(cluster: u16, id: u16) -> bool {
    matches!(
        (cluster, id),
        (IDENTIFY, 0x0000)
            | (ON_OFF, 0x4001..=0x4003)
            | (LEVEL, 0x000F | 0x0011 | 0x4000)
            | (COLOR, 0x000F | 0x4010)
            | (SETTINGS, 0x0000 | 0x0001)
    )
}

fn commands(cluster: u16, generated: bool) -> &'static [u8] {
    match (cluster, generated) {
        (BASIC, false) => &[0x00],
        (IDENTIFY, false) => &[0x00, 0x01, 0x40],
        (IDENTIFY, true) => &[0x00],
        (GROUPS_CLUSTER, false) => &[0x00, 0x01, 0x02, 0x03, 0x04, 0x05],
        (GROUPS_CLUSTER, true) => &[0x00, 0x01, 0x02, 0x03],
        (SCENES_CLUSTER, false) => &[0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x40, 0x41, 0x42],
        (SCENES_CLUSTER, true) => &[0x00, 0x01, 0x02, 0x03, 0x04, 0x06, 0x40, 0x41, 0x42],
        (ON_OFF, false) => &[0x00, 0x01, 0x02, 0x40, 0x41, 0x42],
        (LEVEL, false) => &[0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07],
        (COLOR, false) => &[0x0A, 0x47, 0x4B, 0x4C],
        (TOUCHLINK, false) | (TOUCHLINK, true) => &[0x41, 0x42],
        _ => &[],
    }
}

fn attribute_type(cluster: u16, id: u16) -> Option<u8> {
    ATTRIBUTES.iter().find(|a| a.0 == cluster && a.1 == id).map(|a| a.2)
}

fn value_len(ty: u8, r: &Reader) -> Option<usize> {
    match ty {
        0x08 | 0x10 | 0x18 | 0x20 | 0x28 | 0x30 => Some(1),
        0x09 | 0x19 | 0x21 | 0x29 | 0x31 => Some(2),
        0x0A | 0x1A | 0x22 | 0x2A => Some(3),
        0x0B | 0x1B | 0x23 | 0x2B | 0x39 => Some(4),
        0x41 | 0x42 => {
            let mut peek = *r;
            let n = peek.u8()?;
            Some(1 + if n == 0xFF { 0 } else { n as usize })
        }
        _ => None,
    }
}

fn is_analog(ty: u8) -> bool {
    matches!(ty, 0x20..=0x2F | 0x38..=0x3A | 0xE0..=0xE2)
}

fn report_index(cluster: u16, id: u16) -> Option<usize> {
    match (cluster, id) {
        (ON_OFF, 0x0000) => Some(REPORT_ON_OFF),
        (LEVEL, 0x0000) => Some(REPORT_LEVEL),
        (COLOR, 0x0007) => Some(REPORT_COLOR_TEMP),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LightSettings {
    pub start_up_on_off: u8,
    pub start_up_level: u8,
    pub level_options: u8,
    pub color_options: Option<u8>,
    pub start_up_color_temperature: u16,
    pub ct_min: u16,
    pub ct_max: u16,
    pub global_scene_control: bool,
    pub on_time: u16,
    pub off_wait_time: u16,
    pub remaining_tenths: u16,
    pub min_level: u8,
    pub max_level: u8,
}

impl<H: Hal> Stack<H> {
    pub(crate) fn in_clusters(&self) -> &'static [u16] {
        #[cfg(feature = "sense")]
        if self.identity.sense {
            return match (self.identity.color_temperature, self.identity.ble) {
                (false, false) => &CLUSTERS_SENSE,
                (true, false) => &CLUSTERS_TW_SENSE,
                (false, true) => &CLUSTERS_BLE_SENSE,
                (true, true) => &CLUSTERS_TW_BLE_SENSE,
            };
        }
        match (self.identity.color_temperature, self.identity.ble) {
            (false, false) => &CLUSTERS,
            (true, false) => &CLUSTERS_TW,
            (false, true) => &CLUSTERS_BLE,
            (true, true) => &CLUSTERS_TW_BLE,
        }
    }

    pub(crate) fn device_id(&self) -> u16 {
        if self.identity.color {
            DEVICE_EXTENDED_COLOR_LIGHT
        } else if self.identity.color_temperature {
            DEVICE_COLOR_TEMPERATURE_LIGHT
        } else {
            DEVICE_DIMMABLE_LIGHT
        }
    }

    pub fn light_settings(&mut self) -> LightSettings {
        let tw = self.identity.color_temperature;
        LightSettings {
            start_up_on_off: self.light.power_on,
            start_up_level: self.light.startup_level,
            level_options: self.level_options,
            color_options: tw.then_some(self.color_options),
            start_up_color_temperature: if tw { self.light.startup_color_temp } else { 0xFFFF },
            ct_min: if tw { self.identity.ct_min } else { 0 },
            ct_max: if tw { self.identity.ct_max } else { 0 },
            global_scene_control: self.global_scene_control,
            on_time: self.on_time,
            off_wait_time: self.off_wait,
            remaining_tenths: self.remaining_tenths(self.level_until),
            min_level: MIN_LEVEL,
            max_level: 254,
        }
    }

    pub fn set_level_options(&mut self, options: u8) -> bool {
        self.write_options(LEVEL, options)
    }

    pub fn set_color_options(&mut self, options: u8) -> bool {
        self.write_options(COLOR, options)
    }

    fn write_options(&mut self, cluster: u16, options: u8) -> bool {
        let Some(ty) = attribute_type(cluster, 0x000F).filter(|_| self.in_clusters().contains(&cluster)) else { return false };
        if self.write_status(cluster, 0x000F, ty, &[options]) != SUCCESS {
            return false;
        }
        self.attribute_write(cluster, 0x000F, &[options]);
        true
    }

    pub(crate) fn clusters_of(&self, ep: u8) -> &'static [u16] {
        match objects::object(ep) {
            Some(o) => core::slice::from_ref(&o.cluster),
            None => self.in_clusters(),
        }
    }

    pub(crate) fn on_zcl(&mut self, src: u16, src_ep: u8, dst_ep: u8, cluster: u16, p: &[u8], unicast: bool) {
        let ep = dst_ep;
        let mut r = Reader::new(p);
        let Some(fc) = r.u8() else {
            return;
        };
        let manufacturer = if fc & 0x04 != 0 {
            let Some(code) = r.u16() else {
                return;
            };
            Some(code)
        } else {
            None
        };
        let (Some(seq), Some(cmd)) = (r.u8(), r.u8()) else {
            return;
        };
        if ep == ENDPOINT {
            let (lqi, rssi) = self.rx_signal;
            self.hal.zcl_received(src, cluster, cmd, fc, lqi, rssi);
        }
        if fc & 0x08 != 0 {
            if cluster == super::ota::OTA && fc & 0x07 == 0x01 {
                let _ = self.on_ota(src, src_ep, unicast, cmd, r);
            }
            return;
        }
        let mut rsp = Long::new();
        let queued = self.cmds.iter().filter(|c| c.is_some()).count();
        let diag_mfr = manufacturer == Some(self.identity.manufacturer_code) && cluster == DIAGNOSTICS && ep == ENDPOINT;
        let settings = cluster == SETTINGS && self.identity.ble && ep == ENDPOINT
            && manufacturer.is_none_or(|c| c == self.identity.manufacturer_code);
        let ours = diag_mfr || settings;
        #[cfg(feature = "sense")]
        let ours = ours
            || (cluster == SENSE && self.sense_cluster(ep, cluster) && manufacturer.is_none_or(|c| c == self.identity.manufacturer_code));
        let status = if manufacturer.is_some() && !ours {
            Some(UNSUP_COMMAND)
        } else if !self.clusters_of(ep).contains(&cluster) {
            Some(UNSUPPORTED_CLUSTER)
        } else if fc & 3 == 0 {
            self.zcl_global(Ctx { ep, mfr: manufacturer }, cluster, cmd, r, seq, &mut rsp)
        } else if diag_mfr {
            if cmd == diag::CMD_CLEAR {
                self.diag_clear();
                Some(SUCCESS)
            } else {
                Some(UNSUP_COMMAND)
            }
        } else if ep != ENDPOINT {
            Some(UNSUP_COMMAND)
        } else {
            match self.zcl_cluster(cluster, cmd, r, seq, unicast, &mut rsp) {
                Some(status) => status,
                None => {
                    rsp.clear();
                    Some(MALFORMED_COMMAND)
                }
            }
        };
        if !rsp.is_empty() {
            let asks_everyone = matches!((cluster, cmd), (IDENTIFY, 0x01) | (GROUPS_CLUSTER, 0x02) | (SCENES_CLUSTER, 0x06));
            if unicast || fc & 3 == 0 || asks_everyone {
                self.aps_data(src, src_ep, cluster, PROFILE_HA, ep, rsp.as_slice());
            }
        } else if let Some(status) = status {
            if unicast && (status != SUCCESS || fc & 0x10 == 0) {
                let mut f = Buf::new();
                match manufacturer {
                    Some(code) => {
                        f.u8(FC_GLOBAL_RSP | 0x04);
                        f.u16(code);
                    }
                    None => f.u8(FC_GLOBAL_RSP),
                }
                f.bytes(&[seq, DEFAULT_RSP, cmd, status]);
                if self.cmds.iter().filter(|c| c.is_some()).count() > queued {
                    self.defer_reply(src, src_ep, cluster, f.as_slice());
                } else {
                    self.aps_data(src, src_ep, cluster, PROFILE_HA, ep, f.as_slice());
                }
            }
        }
    }

    fn defer_reply(&mut self, dst: u16, dst_ep: u8, cluster: u16, payload: &[u8]) {
        self.send_deferred();
        if payload.len() > super::DEFERRED_MAX {
            self.aps_data(dst, dst_ep, cluster, PROFILE_HA, ENDPOINT, payload);
            return;
        }
        let mut data = [0u8; super::DEFERRED_MAX];
        data[..payload.len()].copy_from_slice(payload);
        self.deferred = Some(super::Deferred { dst, dst_ep, cluster, secured: self.reply_secured, len: payload.len() as u8, data });
    }

    pub(crate) fn send_deferred(&mut self) {
        let Some(d) = self.deferred.take() else {
            return;
        };
        let secured = core::mem::replace(&mut self.reply_secured, d.secured);
        self.aps_data(d.dst, d.dst_ep, d.cluster, PROFILE_HA, ENDPOINT, &d.data[..d.len as usize]);
        self.reply_secured = secured;
    }

    fn attribute_value(&mut self, cluster: u16, id: u16, out: &mut Long) {
        let string = |out: &mut Long, s: &str| {
            out.u8(s.len() as u8);
            out.bytes(s.as_bytes());
        };
        match (cluster, id) {
            (_, CLUSTER_REVISION) => out.u16(match cluster {
                SETTINGS => 1,
                IDENTIFY | ON_OFF | LEVEL => 2,
                _ => 3,
            }),
            (BASIC, 0x0012) => out.u8(1),
            (LEVEL, 0x0002) => out.u8(1),
            (LEVEL, 0x0003) => out.u8(254),
            (BASIC, 0x0000) => out.u8(ZCL_VERSION),
            (BASIC, 0x0001) => out.u8(self.identity.app_version),
            (BASIC, 0x0002) => out.u8(2),
            (BASIC, 0x0003) => out.u8(self.identity.hw_version),
            (BASIC, 0x0004) => string(out, self.identity.manufacturer),
            (BASIC, 0x0005) => string(out, self.identity.model),
            (BASIC, 0x0006) => string(out, ""),
            (BASIC, 0x0007) => out.u8(0x01),
            (BASIC, 0x0008) => out.u8(0x00),
            (BASIC, 0x0009) => out.u8(GENERIC_TYPE_LED_BULB),
            (BASIC, 0x000A | 0x000B) => out.u8(0),
            (BASIC, 0x4000) => string(out, self.identity.sw_build),
            (IDENTIFY, 0x0000) => {
                let now = self.hal.now_ms();
                out.u16(self.identify_until.map_or(0, |t| (t.wrapping_sub(now) / 1000) as u16));
            }
            (GROUPS_CLUSTER, 0x0000) => out.u8(0),
            (ON_OFF, 0x0000) => out.u8(self.light.on as u8),
            (ON_OFF, 0x4000) => out.u8(self.global_scene_control as u8),
            (ON_OFF, 0x4001) => out.u16(self.on_time),
            (ON_OFF, 0x4002) => out.u16(self.off_wait),
            (ON_OFF, 0x4003) => out.u8(self.light.power_on),
            (LEVEL, 0x0000) => out.u8(self.light.level),
            (LEVEL, 0x0001) => out.u16(self.remaining_tenths(self.level_until)),
            (LEVEL, 0x000F) => out.u8(self.level_options),
            (LEVEL, 0x0011) => out.u8(self.on_level),
            (LEVEL, 0x4000) => out.u8(self.light.startup_level),
            (SCENES_CLUSTER, 0x0000) => out.u8(self.scenes.iter().filter(|s| s.used).count() as u8),
            (SCENES_CLUSTER, 0x0001) => out.u8(self.scene_current.1),
            (SCENES_CLUSTER, 0x0002) => out.u16(self.scene_current.0),
            (SCENES_CLUSTER, 0x0003) => out.u8(self.scene_valid as u8),
            (SCENES_CLUSTER, 0x0004) => out.u8(0),
            (COLOR, 0x0002) => out.u16(self.remaining_tenths(self.color_until)),
            (COLOR, 0x0010) => out.u8(0),
            (COLOR, 0x0007) => out.u16(self.light.color_temp),
            (COLOR, 0x0008 | 0x4001) => out.u8(self.light.color_mode),
            (COLOR, 0x0000) => out.u8(self.light.hue),
            (COLOR, 0x0001) => out.u8(self.light.sat),
            (COLOR, 0x0003) => out.u16(self.light.x),
            (COLOR, 0x0004) => out.u16(self.light.y),
            (COLOR, 0x000F) => out.u8(self.color_options),
            (COLOR, 0x400A) => out.u16(if self.identity.color { COLOR_CAPABILITIES_FULL } else { 0x0010 }),
            (COLOR, 0x400B) => out.u16(self.identity.ct_min),
            (COLOR, 0x400C) => out.u16(self.identity.ct_max),
            (COLOR, 0x400D) => out.u16(self.identity.ct_min),
            (COLOR, 0x4010) => out.u16(self.light.startup_color_temp),
            (SETTINGS, 0x0000) => out.u8(self.ble_mode.to_u8()),
            (SETTINGS, 0x0001) => out.u16(self.ble_window),
            (SETTINGS, 0x0002) => out.u8(self.ble_active as u8),
            _ => {}
        }
    }

    fn write_status(&self, cluster: u16, id: u16, ty: u8, value: &[u8]) -> u8 {
        let Some(expected) = attribute_type(cluster, id).filter(|_| self.has_attribute(cluster, id)) else {
            return UNSUPPORTED_ATTRIBUTE;
        };
        if ty != expected {
            return INVALID_DATA_TYPE;
        }
        if !writable(cluster, id) {
            return READ_ONLY;
        }
        let v = value.iter().rev().fold(0u16, |v, b| v << 8 | *b as u16);
        let valid = match (cluster, id) {
            (ON_OFF, 0x4003) => matches!(v, 0 | 1 | 2 | 0xFF),
            (LEVEL, 0x000F) if self.identity.color_temperature => v & !(EXECUTE_IF_OFF | COUPLE_COLOR_TEMP) as u16 == 0,
            (LEVEL | COLOR, 0x000F) => v & !EXECUTE_IF_OFF as u16 == 0,
            (LEVEL, 0x0011) => v != 0,
            (COLOR, 0x4010) => v == 0xFFFF || (self.identity.ct_min..=self.identity.ct_max).contains(&v),
            (SETTINGS, 0x0000) => BleMode::from_u8(v as u8).is_some(),
            (SETTINGS, 0x0001) => (1..=BLE_WINDOW_MAX_MIN).contains(&v),
            _ => true,
        };
        if valid {
            SUCCESS
        } else {
            INVALID_VALUE
        }
    }

    fn attribute_write(&mut self, cluster: u16, id: u16, value: &[u8]) {
        let v = value.iter().rev().fold(0u16, |v, b| v << 8 | *b as u16);
        match (cluster, id) {
            (ON_OFF, 0x4003) => {
                self.push_command(Command::SetPowerOnBehavior(PowerOnBehavior::from_u8(v as u8)));
                self.light.power_on = v as u8;
            }
            (LEVEL, 0x000F) => {
                self.level_options = v as u8;
                self.tables_dirty = true;
            }
            (COLOR, 0x000F) => {
                self.color_options = v as u8;
                self.tables_dirty = true;
            }
            (LEVEL, 0x0011) => {
                self.on_level = v as u8;
                self.tables_dirty = true;
            }
            (ON_OFF, 0x4001 | 0x4002) => {
                if id == 0x4001 {
                    self.on_time = v;
                } else {
                    self.off_wait = v;
                }
                self.on_tick_at = self.hal.now_ms().wrapping_add(100);
            }
            (LEVEL, 0x4000) => {
                self.push_command(Command::SetStartupLevel(v as u8));
                self.light.startup_level = v as u8;
            }
            (IDENTIFY, 0x0000) => self.identify(v),
            (COLOR, 0x4010) => {
                self.push_command(Command::SetStartupColorTemperature(v));
                self.light.startup_color_temp = v;
            }
            (SETTINGS, 0x0000) => {
                self.ble_mode = BleMode::from_u8(v as u8).unwrap_or(BleMode::On);
                self.store_settings();
            }
            (SETTINGS, 0x0001) => {
                self.ble_window = v;
                self.store_settings();
            }
            _ => {}
        }
    }

    fn identify(&mut self, seconds: u16) {
        let now = self.hal.now_ms();
        if seconds == 0 {
            self.identify_until = None;
        } else {
            self.identify_until = Some(now.wrapping_add(seconds as u32 * 1000));
        }
        self.push_command(Command::Identify { times: seconds.min(255) as u8 });
    }

    fn type_of(&self, c: Ctx, cluster: u16, id: u16) -> Option<u8> {
        let mfr = c.mfr.is_some();
        if let Some(o) = objects::object(c.ep) {
            return if mfr { None } else { objects::attribute_type(o, id) };
        }
        if cluster == DIAGNOSTICS {
            if !mfr && id == CLUSTER_REVISION {
                return Some(T_U16);
            }
            return diag::attribute(mfr, id).map(|a| a.0);
        }
        #[cfg(feature = "sense")]
        if self.sense_cluster(c.ep, cluster) {
            return sense::attribute(cluster, id).map(|a| a.0);
        }
        if (mfr && cluster != SETTINGS) || !self.has_attribute(cluster, id) {
            None
        } else {
            attribute_type(cluster, id)
        }
    }

    fn has_attribute(&self, cluster: u16, id: u16) -> bool {
        self.identity.color || !(cluster == COLOR && matches!(id, 0x0000 | 0x0001 | 0x0003 | 0x0004))
    }

    fn list_of(&self, c: Ctx, cluster: u16, start: u16, mut each: impl FnMut(u16, u8) -> bool) {
        let mut a = [(0u16, 0u8); 10];
        if let Some(o) = objects::object(c.ep) {
            if c.mfr.is_none() {
                let n = objects::attributes(o, &mut a);
                for x in a[..n].iter().filter(|x| x.0 >= start) {
                    if !each(x.0, x.1) {
                        return;
                    }
                }
            }
            return;
        }
        if cluster == DIAGNOSTICS {
            for x in diag::attributes(c.mfr.is_some()).iter().filter(|x| x.0 >= start) {
                if !each(x.0, x.1) {
                    return;
                }
            }
            if c.mfr.is_none() && start <= CLUSTER_REVISION {
                each(CLUSTER_REVISION, T_U16);
            }
            return;
        }
        #[cfg(feature = "sense")]
        if self.sense_cluster(c.ep, cluster) {
            for x in sense::attributes(cluster).iter().filter(|x| x.0 >= start) {
                if !each(x.0, x.1) {
                    return;
                }
            }
            return;
        }
        if c.mfr.is_none() || cluster == SETTINGS {
            for x in ATTRIBUTES.iter().filter(|x| x.0 == cluster && x.1 >= start && self.has_attribute(x.0, x.1)) {
                if !each(x.1, x.2) {
                    return;
                }
            }
        }
    }

    fn writable_at(&self, c: Ctx, cluster: u16, id: u16) -> bool {
        #[cfg(feature = "sense")]
        if self.sense_cluster(c.ep, cluster) {
            return sense::attribute(cluster, id).is_some_and(|a| a.1);
        }
        match objects::object(c.ep) {
            Some(o) => o.cluster != objects::ANALOG_INPUT && id == objects::A_PRESENT_VALUE,
            None => cluster != DIAGNOSTICS && writable(cluster, id),
        }
    }

    fn reportable_at(&self, c: Ctx, cluster: u16, id: u16) -> bool {
        let sense = {
            #[cfg(feature = "sense")]
            {
                c.ep == ENDPOINT && c.mfr.is_none() && self.sense_reportable(cluster, id)
            }
            #[cfg(not(feature = "sense"))]
            false
        };
        (c.ep == ENDPOINT && c.mfr.is_none() && report_index(cluster, id).is_some())
            || objects::reportable(c.ep, cluster, id, c.mfr.is_some())
            || (c.ep == ENDPOINT && c.mfr.is_none() && self.colour_reportable(cluster, id))
            || sense
    }

    pub(crate) fn colour_reportable(&self, cluster: u16, id: u16) -> bool {
        self.identity.color && cluster == COLOR && matches!(id, 0x0000 | 0x0001 | 0x0003 | 0x0004 | 0x0008)
    }

    fn value_of(&mut self, c: Ctx, cluster: u16, id: u16, out: &mut Long) {
        if let Some(o) = objects::object(c.ep) {
            let single = |out: &mut Long, v: f32| out.u32(v.to_bits());
            match id {
                objects::A_DESCRIPTION => {
                    out.u8(o.description.len() as u8);
                    out.bytes(o.description.as_bytes());
                }
                objects::A_MAX_PRESENT => single(out, o.max),
                objects::A_MIN_PRESENT => single(out, o.min),
                objects::A_RESOLUTION => single(out, o.resolution),
                objects::A_OUT_OF_SERVICE => out.u8((o.value == objects::Value::BleWindow && self.ble_mode == super::BleMode::Off) as u8),
                objects::A_STATUS_FLAGS => out.u8(0),
                objects::A_PRESENT_VALUE => {
                    let v = self.object_present(o);
                    if o.cluster == objects::BINARY_OUTPUT {
                        out.u8(v as u8);
                    } else {
                        out.u32(v);
                    }
                }
                objects::A_ENGINEERING_UNITS => out.u16(o.units.unwrap_or(0)),
                objects::A_APPLICATION_TYPE => out.u32(o.app_type.unwrap_or(0)),
                CLUSTER_REVISION => out.u16(1),
                _ => {}
            }
            return;
        }
        if cluster == DIAGNOSTICS && !(c.mfr.is_none() && id == CLUSTER_REVISION) {
            if let Some((ty, src)) = diag::attribute(c.mfr.is_some(), id) {
                if ty == diag::T_OCTETS {
                    let mut b = [0u8; 64];
                    let n = self.diag_bytes(src, &mut b);
                    out.u8(n as u8);
                    out.bytes(&b[..n]);
                } else {
                    let v = self.diag_value(src);
                    put_number(out, ty, v);
                }
            }
            return;
        }
        #[cfg(feature = "sense")]
        if self.sense_cluster(c.ep, cluster) {
            self.sense_value(cluster, id, out);
            return;
        }
        self.attribute_value(cluster, id, out);
    }

    fn write_status_at(&self, c: Ctx, cluster: u16, id: u16, ty: u8, value: &[u8]) -> u8 {
        let Some(o) = objects::object(c.ep) else {
            if cluster == DIAGNOSTICS {
                return match self.type_of(c, cluster, id) {
                    None => UNSUPPORTED_ATTRIBUTE,
                    Some(t) if t != ty => INVALID_DATA_TYPE,
                    Some(_) => READ_ONLY,
                };
            }
            #[cfg(feature = "sense")]
            if self.sense_cluster(c.ep, cluster) {
                return self.sense_write_status(cluster, id, ty, value);
            }
            return self.write_status(cluster, id, ty, value);
        };
        let Some(expected) = self.type_of(c, cluster, id) else {
            return UNSUPPORTED_ATTRIBUTE;
        };
        if ty != expected {
            return INVALID_DATA_TYPE;
        }
        if !self.writable_at(c, cluster, id) {
            return READ_ONLY;
        }
        if Self::object_value_ok(o, value) {
            SUCCESS
        } else {
            INVALID_VALUE
        }
    }

    fn write_at(&mut self, c: Ctx, cluster: u16, id: u16, value: &[u8]) {
        #[cfg(feature = "sense")]
        if self.sense_cluster(c.ep, cluster) {
            self.sense_write(cluster, id, value);
            return;
        }
        match objects::object(c.ep) {
            Some(o) => self.object_write(o, value),
            None => self.attribute_write(cluster, id, value),
        }
    }

    pub(crate) fn send_report(&mut self, ep: u8, cluster: u16, mfr: bool, attr: u16, ty: u8, value: u32) {
        self.zcl_seq = self.zcl_seq.wrapping_add(1);
        let mut f = Long::new();
        if mfr {
            f.u8(FC_GLOBAL_RSP | 0x04);
            f.u16(self.identity.manufacturer_code);
        } else {
            f.u8(FC_GLOBAL_RSP);
        }
        f.bytes(&[self.zcl_seq, REPORT_ATTRIBUTES]);
        f.u16(attr);
        f.u8(ty);
        put_number(&mut f, ty, value);
        self.send_to_bindings(ep, cluster, f.as_slice());
    }

    fn send_to_bindings(&mut self, ep: u8, cluster: u16, frame: &[u8]) {
        let now = self.hal.now_ms();
        for b in self.bindings {
            if !b.used || b.cluster != cluster || b.src_ep != ep {
                continue;
            }
            if b.group {
                self.aps_group(b.dst as u16, cluster, PROFILE_HA, ep, frame);
            } else if let Some(short) = self.resolve(b.dst) {
                self.aps_data(short, b.dst_ep, cluster, PROFILE_HA, ep, frame);
            } else if due(now, self.addr_query_at) {
                self.addr_query_at = now.wrapping_add(10_000);
                self.zdo_nwk_addr_request(b.dst);
            }
        }
    }

    fn zcl_global(&mut self, c: Ctx, cluster: u16, cmd: u8, mut r: Reader, seq: u8, rsp: &mut Long) -> Option<u8> {
        let header = |rsp: &mut Long, id: u8| match c.mfr {
            Some(code) => {
                rsp.u8(FC_GLOBAL_RSP | 0x04);
                rsp.u16(code);
                rsp.bytes(&[seq, id]);
            }
            None => rsp.bytes(&[FC_GLOBAL_RSP, seq, id]),
        };
        match cmd {
            READ_ATTRIBUTES => {
                header(rsp, READ_ATTRIBUTES_RSP);
                while let Some(id) = r.u16() {
                    let table = cluster == DIAGNOSTICS && self.type_of(c, cluster, id) == Some(diag::T_OCTETS);
                    #[cfg(feature = "sense")]
                    let table = table || self.sense_long(cluster, id);
                    if rsp.room() < if table { 70 } else { 40 } {
                        break;
                    }
                    rsp.u16(id);
                    match self.type_of(c, cluster, id) {
                        Some(ty) => {
                            rsp.u8(SUCCESS);
                            rsp.u8(ty);
                            self.value_of(c, cluster, id, rsp);
                        }
                        None => rsp.u8(UNSUPPORTED_ATTRIBUTE),
                    }
                }
                None
            }
            WRITE_ATTRIBUTES | WRITE_ATTRIBUTES_UNDIVIDED | WRITE_ATTRIBUTES_NO_RSP => {
                header(rsp, WRITE_ATTRIBUTES_RSP);
                let undivided = cmd == WRITE_ATTRIBUTES_UNDIVIDED;
                let records = r;
                let mut failed = false;
                while let (Some(id), Some(ty)) = (r.u16(), r.u8()) {
                    let value = value_len(ty, &r).and_then(|n| r.take(n));
                    let status = match value {
                        Some(v) => self.write_status_at(c, cluster, id, ty, v),
                        None => INVALID_DATA_TYPE,
                    };
                    match value {
                        Some(v) if status == SUCCESS && !undivided => self.write_at(c, cluster, id, v),
                        _ => {}
                    }
                    if status != SUCCESS {
                        failed = true;
                        rsp.u8(status);
                        rsp.u16(id);
                    }
                    if value.is_none() {
                        break;
                    }
                }
                if undivided && !failed {
                    r = records;
                    while let (Some(id), Some(ty)) = (r.u16(), r.u8()) {
                        let Some(v) = value_len(ty, &r).and_then(|n| r.take(n)) else {
                            break;
                        };
                        self.write_at(c, cluster, id, v);
                    }
                }
                if !failed {
                    rsp.u8(SUCCESS);
                }
                if cmd == WRITE_ATTRIBUTES_NO_RSP {
                    rsp.clear();
                }
                None
            }
            CONFIGURE_REPORTING => {
                header(rsp, CONFIGURE_REPORTING_RSP);
                let mut failed = false;
                while let (Some(direction), Some(id)) = (r.u8(), r.u16()) {
                    let status = if direction != 0 {
                        if r.skip(2).is_none() {
                            break;
                        }
                        SUCCESS
                    } else {
                        let (Some(ty), Some(min_s), Some(max_s)) = (r.u8(), r.u16(), r.u16()) else {
                            break;
                        };
                        let change = if is_analog(ty) {
                            value_len(ty, &r).and_then(|n| r.take(n)).map(|v| v.iter().rev().fold(0u32, |a, b| a << 8 | *b as u32))
                        } else {
                            Some(0)
                        };
                        let Some(change) = change else {
                            break;
                        };
                        let expected = self.type_of(c, cluster, id);
                        let light = if c.ep == ENDPOINT && c.mfr.is_none() { report_index(cluster, id) } else { None };
                        let other = light.is_none() && self.reportable_at(c, cluster, id);
                        match expected {
                            None => UNSUPPORTED_ATTRIBUTE,
                            Some(_) if light.is_none() && !other => UNREPORTABLE_ATTRIBUTE,
                            Some(t) if t != ty => INVALID_DATA_TYPE,
                            Some(_) if max_s != 0 && max_s < min_s && max_s != 0xFFFF => INVALID_VALUE,
                            Some(_) => match light {
                                Some(i) => {
                                    let e = &mut self.reports[i];
                                    (e.min_s, e.max_s, e.change) = if (min_s, max_s) == (0xFFFF, 0) {
                                        super::REPORT_DEFAULTS[i]
                                    } else {
                                        (min_s, max_s, change as u16)
                                    };
                                    self.tables_dirty = true;
                                    SUCCESS
                                }
                                None => match self.xreport_configure(c.ep, cluster, id, c.mfr.is_some(), min_s, max_s, change) {
                                    Some(()) => SUCCESS,
                                    None => INSUFFICIENT_SPACE,
                                },
                            },
                        }
                    };
                    if status != SUCCESS {
                        failed = true;
                        rsp.u8(status);
                        rsp.u8(direction);
                        rsp.u16(id);
                    }
                }
                if !failed {
                    rsp.u8(SUCCESS);
                }
                None
            }
            READ_REPORTING_CONFIG => {
                header(rsp, READ_REPORTING_CONFIG_RSP);
                while let (Some(direction), Some(id)) = (r.u8(), r.u16()) {
                    let ty = self.type_of(c, cluster, id);
                    let light = if c.ep == ENDPOINT && c.mfr.is_none() { report_index(cluster, id) } else { None };
                    let config = match (direction, ty, light) {
                        (0, Some(_), Some(i)) => Some((self.reports[i].min_s, self.reports[i].max_s, self.reports[i].change as u32)),
                        (0, Some(_), None) if self.reportable_at(c, cluster, id) => Some(
                            self.xreport_find(c.ep, cluster, id, c.mfr.is_some())
                                .map_or((0xFFFF, 0xFFFF, 0), |x| (x.min_s, x.max_s, x.change)),
                        ),
                        _ => None,
                    };
                    match (config, ty) {
                        (Some((min_s, max_s, change)), Some(ty)) => {
                            rsp.u8(SUCCESS);
                            rsp.u8(0);
                            rsp.u16(id);
                            rsp.u8(ty);
                            rsp.u16(min_s);
                            rsp.u16(max_s);
                            if is_analog(ty) {
                                let n = value_len(ty, &Reader::new(&[])).unwrap_or(2);
                                rsp.bytes(&change.to_le_bytes()[..n.min(4)]);
                            }
                        }
                        (_, ty) => {
                            rsp.u8(if ty.is_some() { UNREPORTABLE_ATTRIBUTE } else { UNSUPPORTED_ATTRIBUTE });
                            rsp.u8(direction);
                            rsp.u16(id);
                        }
                    }
                }
                None
            }
            DISCOVER_ATTRIBUTES | DISCOVER_ATTRIBUTES_EXTENDED => {
                let (Some(start), Some(max)) = (r.u16(), r.u8()) else {
                    return Some(MALFORMED_COMMAND);
                };
                let extended = cmd == DISCOVER_ATTRIBUTES_EXTENDED;
                header(rsp, if extended { DISCOVER_ATTRIBUTES_EXTENDED_RSP } else { DISCOVER_ATTRIBUTES_RSP });
                let complete_at = rsp.len();
                rsp.u8(1);
                let left = max.min(20);
                let mut found = [(0u16, 0u8); 20];
                let mut n = 0;
                let mut more = false;
                self.list_of(c, cluster, start, |id, ty| {
                    if n == left as usize {
                        more = true;
                        return false;
                    }
                    found[n] = (id, ty);
                    n += 1;
                    true
                });
                for (id, ty) in found[..n].iter().copied() {
                    rsp.u16(id);
                    rsp.u8(ty);
                    if extended {
                        rsp.u8(0x01 | (self.writable_at(c, cluster, id) as u8) << 1 | (self.reportable_at(c, cluster, id) as u8) << 2);
                    }
                }
                if more {
                    rsp.set(complete_at, 0);
                }
                None
            }
            DISCOVER_COMMANDS_RECEIVED | DISCOVER_COMMANDS_GENERATED => {
                let (Some(start), Some(max)) = (r.u8(), r.u8()) else {
                    return Some(MALFORMED_COMMAND);
                };
                header(rsp, cmd + 1);
                let complete_at = rsp.len();
                rsp.u8(1);
                let mut left = max;
                let list: &[u8] = match (c.mfr, c.ep == ENDPOINT) {
                    #[cfg(feature = "sense")]
                    (Some(_), _) if cluster == SENSE => &[],
                    (Some(_), _) if cmd == DISCOVER_COMMANDS_RECEIVED => &[diag::CMD_CLEAR],
                    (Some(_), _) | (None, false) => &[],
                    (None, true) if cluster == COLOR && self.identity.color && cmd == DISCOVER_COMMANDS_RECEIVED => &COLOR_COMMANDS_FULL,
                    (None, true) => commands(cluster, cmd == DISCOVER_COMMANDS_GENERATED),
                };
                for id in list.iter().filter(|id| **id >= start) {
                    if left == 0 {
                        rsp.set(complete_at, 0);
                        break;
                    }
                    rsp.u8(*id);
                    left -= 1;
                }
                None
            }
            DEFAULT_RSP => None,
            _ => Some(UNSUP_COMMAND),
        }
    }

    fn zcl_cluster(&mut self, cluster: u16, cmd: u8, mut r: Reader, seq: u8, unicast: bool, rsp: &mut Long) -> Option<Option<u8>> {
        let header = |rsp: &mut Long, id: u8| rsp.bytes(&[FC_CLUSTER_RSP, seq, id]);
        let status = match (cluster, cmd) {
            (IDENTIFY, 0x00) => {
                let seconds = r.u16()?;
                self.identify(seconds);
                SUCCESS
            }
            (IDENTIFY, 0x01) => {
                if let Some(until) = self.identify_until {
                    let now = self.hal.now_ms();
                    header(rsp, 0x00);
                    rsp.u16((until.wrapping_sub(now) / 1000) as u16);
                }
                SUCCESS
            }
            (BASIC, 0x00) => {
                self.level_options = 0;
                self.color_options = COLOR_OPTIONS_DEFAULT;
                self.on_level = 0xFF;
                self.global_scene_control = true;
                self.on_time = 0;
                self.off_wait = 0;
                for (r, (min_s, max_s, change)) in self.reports.iter_mut().zip(super::REPORT_DEFAULTS) {
                    (r.min_s, r.max_s, r.change) = (min_s, max_s, change);
                }
                self.tables_dirty = true;
                self.identify(0);
                self.light.power_on = 0xFF;
                self.light.startup_level = 0xFF;
                self.light.startup_color_temp = 0xFFFF;
                if self.identity.ble {
                    self.ble_mode = BleMode::On;
                    self.ble_window = BLE_WINDOW_DEFAULT_MIN;
                    self.store_settings();
                }
                #[cfg(feature = "sense")]
                self.sense_reset();
                self.push_command(Command::SetPowerOnBehavior(PowerOnBehavior::Previous));
                self.push_command(Command::SetStartupLevel(0xFF));
                if self.identity.color_temperature {
                    self.push_command(Command::SetStartupColorTemperature(0xFFFF));
                }
                SUCCESS
            }
            (IDENTIFY, 0x40) => {
                self.push_command(Command::Identify { times: 1 });
                SUCCESS
            }
            (GROUPS_CLUSTER, 0x00 | 0x05) => {
                let group = r.u16()?;
                if cmd == 0x05 {
                    return Some(Some(if self.identify_until.is_none() { SUCCESS } else { self.group_add(group) }));
                }
                let status = self.group_add(group);
                header(rsp, 0x00);
                rsp.u8(status);
                rsp.u16(group);
                SUCCESS
            }
            (GROUPS_CLUSTER, 0x01) => {
                let group = r.u16()?;
                header(rsp, 0x01);
                rsp.u8(self.group_status(group));
                rsp.u16(group);
                rsp.u8(0);
                SUCCESS
            }
            (GROUPS_CLUSTER, 0x02) => {
                let asked = r.u8()?;
                header(rsp, 0x02);
                rsp.u8(self.groups.iter().filter(|g| **g == 0).count() as u8);
                let count_at = rsp.len();
                rsp.u8(0);
                let mut count = 0;
                if asked == 0 {
                    for g in self.groups.iter().filter(|g| **g != 0) {
                        rsp.u16(*g);
                        count += 1;
                    }
                } else {
                    for _ in 0..asked {
                        let g = r.u16()?;
                        if g != 0 && self.groups.contains(&g) {
                            rsp.u16(g);
                            count += 1;
                        }
                    }
                }
                rsp.set(count_at, count);
                if !unicast && asked != 0 && count == 0 {
                    rsp.clear();
                }
                SUCCESS
            }
            (GROUPS_CLUSTER, 0x03) => {
                let group = r.u16()?;
                header(rsp, 0x03);
                let status = self.group_status(group);
                if status == SUCCESS {
                    self.groups.iter_mut().filter(|g| **g == group).for_each(|g| *g = 0);
                    self.tables_dirty = true;
                    self.scenes_remove(|s| s.group == group);
                }
                rsp.u8(status);
                rsp.u16(group);
                SUCCESS
            }
            (GROUPS_CLUSTER, 0x04) => {
                self.groups = [0; GROUPS];
                self.tables_dirty = true;
                self.scenes_remove(|s| s.group != 0);
                SUCCESS
            }
            (TOUCHLINK, 0x41 | 0x42) => {
                let start = r.u8()?;
                header(rsp, cmd);
                rsp.bytes(&[0, start, 0]);
                SUCCESS
            }
            (SCENES_CLUSTER, _) => return self.zcl_scenes(cmd, r, seq, unicast, rsp),
            (COLOR, _) => return self.zcl_color(cmd, r),
            (ON_OFF, 0x00) => {
                self.light_off();
                SUCCESS
            }
            (ON_OFF, 0x01) => {
                self.light_on();
                SUCCESS
            }
            (ON_OFF, 0x02) => {
                if self.light.on {
                    self.light_off();
                } else {
                    self.light_on();
                }
                SUCCESS
            }
            (ON_OFF, 0x40) => {
                let (effect, variant) = (r.u8()?, r.u8()?);
                let fade_ms = match (effect, variant) {
                    (0, 1) => 0,
                    (0, 2) => 12_800,
                    (1, _) => 1_500,
                    _ => 800,
                };
                if self.global_scene_control {
                    self.global_scene_control = false;
                    let l = self.light;
                    let colour = self.scene_colour();
                    self.scene_put(Scene { used: true, group: 0, id: 0, on: l.on, level: l.level, color_temp: l.color_temp, colour, transition: 0, tenths: 0, omitted: 0 });
                }
                self.scene_valid = false;
                self.on_time = 0;
                if self.light.on && fade_ms != 0 {
                    self.push_command(Command::MoveToLevel { level: 0, transition_ms: fade_ms });
                } else {
                    self.push_command(Command::Off);
                }
                SUCCESS
            }
            (ON_OFF, 0x41) => {
                if !self.global_scene_control {
                    self.global_scene_control = true;
                    match self.scene_index(0, 0) {
                        Some(i) => self.scene_recall(i, None),
                        None => self.push_command(Command::On),
                    }
                    if self.on_time == 0 {
                        self.off_wait = 0;
                    }
                }
                SUCCESS
            }
            (ON_OFF, 0x42) => {
                let control = r.u8()?;
                let on_time = r.u16()?;
                let off_wait = r.u16()?;
                if control & 0x01 != 0 && !self.light.on {
                    return Some(Some(SUCCESS));
                }
                if self.off_wait > 0 && !self.light.on {
                    self.off_wait = self.off_wait.min(off_wait);
                } else {
                    self.on_time = self.on_time.max(on_time);
                    self.off_wait = off_wait;
                    self.scene_valid = false;
                    self.global_scene_control = true;
                    self.push_command(Command::On);
                }
                self.on_tick_at = self.hal.now_ms().wrapping_add(100);
                SUCCESS
            }
            (LEVEL, 0x00..=0x07) => {
                let with_on_off = cmd >= 0x04;
                let lowest = if with_on_off { 0 } else { MIN_LEVEL };
                let to_level = |level: i32| if with_on_off && level <= MIN_LEVEL as i32 { 0 } else { level.clamp(MIN_LEVEL as i32, 254) as u8 };
                let current = if self.light.on || !with_on_off { self.light.level as i32 } else { 0 };
                let command = match cmd & 3 {
                    0 => {
                        let level = to_level(r.u8()? as i32);
                        let time = r.u16()?;
                        Command::MoveToLevel { level, transition_ms: if time == 0xFFFF { 0 } else { time as u32 * 100 } }
                    }
                    1 => {
                        let up = r.u8()? == 0;
                        let rate = match r.u8()? {
                            0xFF => DEFAULT_MOVE_RATE,
                            0 => return Some(Some(INVALID_VALUE)),
                            v => v as u32,
                        };
                        let level = if up { 254 } else { lowest };
                        let distance = (level as i32 - current).unsigned_abs();
                        Command::MoveToLevel { level, transition_ms: distance * 1000 / rate }
                    }
                    2 => {
                        let up = r.u8()? == 0;
                        let size = r.u8()? as i32;
                        let time = r.u16()?;
                        let wanted = if up { current + size } else { current - size };
                        let reached = wanted.clamp(lowest as i32, 254);
                        let ms = if time == 0xFFFF || size == 0 { 0 } else { time as u32 * 100 * (reached - current).unsigned_abs() / size as u32 };
                        Command::MoveToLevel { level: to_level(reached), transition_ms: ms }
                    }
                    _ => Command::StopTransition,
                };
                if with_on_off || self.light.on {
                    self.scene_valid = false;
                    if let (Command::MoveToLevel { level, .. }, true) = (command, with_on_off) {
                        self.on_off_set(level != 0);
                    }
                    if let (Command::MoveToLevel { level, transition_ms }, true) = (command, self.level_options & COUPLE_COLOR_TEMP != 0) {
                        if level != 0 && self.identity.color_temperature {
                            let (lo, hi) = (self.identity.ct_min as u32, self.identity.ct_max as u32);
                            let mireds = (hi - (level as u32 - 1) * (hi - lo) / 253) as u16;
                            self.push_command(Command::MoveToColorTemperature { mireds, transition_ms });
                        }
                    }
                    self.push_command(command);
                } else if execute_if_off(self.level_options, &mut r) {
                    if let Command::MoveToLevel { level, .. } = command {
                        self.scene_valid = false;
                        self.push_command(Command::SetLevelWhileOff(level));
                    }
                }
                SUCCESS
            }
            _ => UNSUP_COMMAND,
        };
        Some(if rsp.is_empty() { Some(status) } else { None })
    }

    fn on_off_set(&mut self, on: bool) {
        if on {
            self.global_scene_control = true;
            if self.on_time == 0 {
                self.off_wait = 0;
            }
        } else {
            self.on_time = 0;
        }
    }

    fn scene_colour(&self) -> super::SceneColour {
        let l = &self.light;
        super::SceneColour { mode: l.color_mode, hue: l.hue, sat: l.sat, x: l.x, y: l.y }
    }

    fn scenes_remove(&mut self, which: impl Fn(&Scene) -> bool) {
        for s in self.scenes.iter_mut() {
            if s.used && which(s) {
                s.used = false;
                self.scenes_dirty = true;
            }
        }
    }

    fn scene_index(&self, group: u16, id: u8) -> Option<usize> {
        self.scenes.iter().position(|s| s.used && s.group == group && s.id == id)
    }

    fn scene_group_status(&self, group: u16) -> u8 {
        if group > 0xFFF7 {
            INVALID_VALUE
        } else if group != 0 && !self.groups.contains(&group) {
            INVALID_FIELD
        } else {
            SUCCESS
        }
    }

    fn scene_put(&mut self, scene: Scene) -> u8 {
        let status = self.scene_group_status(scene.group);
        if status != SUCCESS {
            return status;
        }
        let slot = self.scene_index(scene.group, scene.id).or_else(|| self.scenes.iter().position(|s| !s.used));
        match slot {
            Some(i) => {
                self.scenes[i] = scene;
                self.scenes_dirty = true;
                SUCCESS
            }
            None => INSUFFICIENT_SPACE,
        }
    }

    fn scene_recall(&mut self, i: usize, transition_ms: Option<u32>) {
        let s = self.scenes[i];
        let ms = transition_ms.unwrap_or(s.transition as u32 * 1000 + s.tenths as u32 * 100);
        let has = |set: u8| s.omitted & set == 0;
        let on = if has(SCENE_NO_ON_OFF) { s.on } else { self.light.on };
        if has(SCENE_NO_ON_OFF) {
            self.on_off_set(s.on);
            if !s.on {
                self.push_command(Command::Off);
            }
        }
        if has(SCENE_NO_COLOR) && self.identity.color_temperature {
            let transition_ms = if on { ms } else { 0 };
            let c = s.colour;
            let command = match (self.identity.color, c.mode) {
                (true, 0) => Command::MoveToHueSaturation { hue: c.hue, sat: c.sat, direction: HueDirection::Shortest, transition_ms },
                (true, 1) => Command::MoveToXy { x: c.x, y: c.y, transition_ms },
                _ => Command::MoveToColorTemperature { mireds: s.color_temp, transition_ms },
            };
            self.push_command(command);
        }
        match (on, has(SCENE_NO_LEVEL)) {
            (true, true) => self.push_command(Command::MoveToLevel { level: s.level, transition_ms: ms }),
            (true, false) if !self.light.on => self.push_command(Command::On),
            (false, true) => self.push_command(Command::SetLevelWhileOff(s.level)),
            _ => {}
        }
    }

    fn light_on(&mut self) {
        self.scene_valid = false;
        self.global_scene_control = true;
        if self.on_time == 0 {
            self.off_wait = 0;
        }
        if self.on_level == 0xFF {
            self.push_command(Command::On);
        } else {
            self.push_command(Command::MoveToLevel { level: self.on_level, transition_ms: 0 });
        }
    }

    fn light_off(&mut self) {
        self.scene_valid = false;
        self.on_time = 0;
        self.push_command(Command::Off);
    }

    pub(crate) fn on_off_timers(&mut self, now: u32) {
        if !due(now, self.on_tick_at) {
            return;
        }
        self.on_tick_at = now.wrapping_add(100);
        if self.light.on {
            if self.on_time != 0 && self.on_time != 0xFFFF {
                self.on_time -= 1;
                if self.on_time == 0 {
                    self.off_wait = 0;
                    self.scene_valid = false;
                    self.push_command(Command::Off);
                }
            }
        } else if self.off_wait != 0 && self.off_wait != 0xFFFF {
            self.off_wait -= 1;
        }
    }

    fn remaining_tenths(&mut self, until: u32) -> u16 {
        let left = until.wrapping_sub(self.hal.now_ms());
        if left >= 0x8000_0000 {
            0
        } else {
            (left / 100).min(0xFFFE) as u16
        }
    }

    fn zcl_scenes(&mut self, cmd: u8, mut r: Reader, seq: u8, unicast: bool, rsp: &mut Long) -> Option<Option<u8>> {
        rsp.bytes(&[FC_CLUSTER_RSP, seq, cmd]);
        if cmd == 0x42 {
            let all = r.u8()? & 0x01 != 0;
            let (from, from_id, to, to_id) = (r.u16()?, r.u8()?, r.u16()?, r.u8()?);
            let (s_from, s_to) = (self.scene_group_status(from), self.scene_group_status(to));
            let mut status = match (s_from, s_to) {
                (INVALID_VALUE, _) | (_, INVALID_VALUE) => INVALID_VALUE,
                (SUCCESS, s) | (s, _) => s,
            };
            if status == SUCCESS && !all && self.scene_index(from, from_id).is_none() {
                status = NOT_FOUND;
            }
            if status == SUCCESS {
                for i in 0..self.scenes.len() {
                    let s = self.scenes[i];
                    if s.used && s.group == from && (all || s.id == from_id) {
                        let copy = Scene { group: to, id: if all { s.id } else { to_id }, ..s };
                        let put = self.scene_put(copy);
                        if put != SUCCESS {
                            status = put;
                        }
                    }
                }
            }
            rsp.u8(status);
            rsp.u16(from);
            rsp.u8(from_id);
            return Some(None);
        }
        let group = r.u16()?;
        let group_status = self.scene_group_status(group);
        let known_group = group_status == SUCCESS;
        let enhanced = cmd & 0x40 != 0;
        match cmd & !0x40 {
            0x00 => {
                let id = r.u8()?;
                let time = r.u16()?;
                let (transition, tenths) = if enhanced { (time / 10, (time % 10) as u8) } else { (time, 0) };
                let name = r.u8()?;
                r.skip(if name == 0xFF { 0 } else { name as usize })?;
                let mut scene = Scene {
                    used: true,
                    group,
                    id,
                    on: false,
                    level: 0,
                    color_temp: 0,
                    colour: super::SceneColour::TEMPERATURE,
                    transition,
                    tenths,
                    omitted: SCENE_NO_ON_OFF | SCENE_NO_LEVEL | SCENE_NO_COLOR,
                };
                while let (Some(cluster), Some(len)) = (r.u16(), r.u8()) {
                    let data = r.take(len as usize)?;
                    match (cluster, data) {
                        (ON_OFF, [on, ..]) => {
                            scene.on = *on != 0;
                            scene.omitted &= !SCENE_NO_ON_OFF;
                        }
                        (LEVEL, [level, ..]) => {
                            scene.level = (*level).clamp(1, 254);
                            scene.omitted &= !SCENE_NO_LEVEL;
                        }
                        (COLOR, d) if d.len() >= 13 => {
                            scene.color_temp = u16::from_le_bytes([d[11], d[12]]);
                            if self.identity.color {
                                let u = |i: usize| u16::from_le_bytes([d[i], d[i + 1]]);
                                let mode = match d.get(13) {
                                    Some(&m) if m <= 2 => m,
                                    _ if scene.color_temp != 0 => 2,
                                    _ => 1,
                                };
                                scene.colour = super::SceneColour { mode, hue: (u(4) >> 8).min(254) as u8, sat: d[6], x: u(0), y: u(2) };
                            }
                            scene.omitted &= !SCENE_NO_COLOR;
                        }
                        _ => {}
                    }
                }
                let status = self.scene_put(scene);
                rsp.u8(status);
                rsp.u16(group);
                rsp.u8(id);
            }
            0x01 => {
                let id = r.u8()?;
                let found = self.scene_index(group, id);
                rsp.u8(match (known_group, found) {
                    (false, _) => group_status,
                    (true, None) => NOT_FOUND,
                    (true, Some(_)) => SUCCESS,
                });
                rsp.u16(group);
                rsp.u8(id);
                if let (true, Some(i)) = (known_group, found) {
                    let s = self.scenes[i];
                    rsp.u16(if enhanced { s.transition.saturating_mul(10).saturating_add(s.tenths as u16) } else { s.transition });
                    rsp.u8(0);
                    if s.omitted & SCENE_NO_ON_OFF == 0 {
                        rsp.u16(ON_OFF);
                        rsp.bytes(&[1, s.on as u8]);
                    }
                    if s.omitted & SCENE_NO_LEVEL == 0 {
                        rsp.u16(LEVEL);
                        rsp.bytes(&[1, s.level]);
                    }
                    if self.identity.color_temperature && s.omitted & SCENE_NO_COLOR == 0 {
                        rsp.u16(COLOR);
                        if self.identity.color {
                            let c = s.colour;
                            rsp.u8(14);
                            rsp.u16(c.x);
                            rsp.u16(c.y);
                            rsp.u16((c.hue as u16) << 8);
                            rsp.bytes(&[c.sat, 0, 0, 0, 0]);
                            rsp.u16(s.color_temp);
                            rsp.u8(c.mode);
                        } else {
                            rsp.u8(13);
                            rsp.bytes(&[0; 11]);
                            rsp.u16(s.color_temp);
                        }
                    }
                }
            }
            0x02 if !enhanced => {
                let id = r.u8()?;
                let found = self.scene_index(group, id);
                rsp.u8(match (known_group, found) {
                    (false, _) => group_status,
                    (true, None) => NOT_FOUND,
                    (true, Some(i)) => {
                        self.scenes[i].used = false;
                        self.scenes_dirty = true;
                        SUCCESS
                    }
                });
                rsp.u16(group);
                rsp.u8(id);
            }
            0x03 if !enhanced => {
                if known_group {
                    self.scenes_remove(|s| s.group == group);
                }
                rsp.u8(group_status);
                rsp.u16(group);
            }
            0x04 if !enhanced => {
                let id = r.u8()?;
                let (transition, tenths) = self.scene_index(group, id).map_or((0, 0), |i| (self.scenes[i].transition, self.scenes[i].tenths));
                let colour = self.scene_colour();
                let status = self.scene_put(Scene {
                    used: true,
                    group,
                    id,
                    on: self.light.on,
                    level: self.light.level,
                    color_temp: self.light.color_temp,
                    colour,
                    transition,
                    tenths,
                    omitted: 0,
                });
                if status == SUCCESS {
                    self.scene_current = (group, id);
                    self.scene_valid = true;
                }
                rsp.u8(status);
                rsp.u16(group);
                rsp.u8(id);
            }
            0x05 if !enhanced => {
                let id = r.u8()?;
                rsp.clear();
                if !known_group {
                    return Some(Some(group_status));
                }
                let Some(i) = self.scene_index(group, id) else {
                    return Some(Some(NOT_FOUND));
                };
                let ms = match r.u16() {
                    Some(tenths) if tenths != 0xFFFF => Some(tenths as u32 * 100),
                    _ => None,
                };
                self.scene_recall(i, ms);
                self.scene_current = (group, id);
                self.scene_valid = true;
                return Some(Some(SUCCESS));
            }
            0x06 if !enhanced => {
                rsp.u8(group_status);
                rsp.u8(self.scenes.iter().filter(|s| !s.used).count() as u8);
                rsp.u16(group);
                let mut count = 0;
                if known_group {
                    let count_at = rsp.len();
                    rsp.u8(0);
                    for s in self.scenes.iter().filter(|s| s.used && s.group == group) {
                        rsp.u8(s.id);
                        count += 1;
                    }
                    rsp.set(count_at, count);
                }
                if !unicast && count == 0 {
                    rsp.clear();
                }
            }
            _ => {
                rsp.clear();
                return Some(Some(UNSUP_COMMAND));
            }
        }
        Some(None)
    }

    fn zcl_color(&mut self, cmd: u8, mut r: Reader) -> Option<Option<u8>> {
        let (lo, hi) = (self.identity.ct_min, self.identity.ct_max);
        let current = self.light.color_temp;
        let time_ms = |tenths: u16| if tenths == 0xFFFF { 0 } else { tenths as u32 * 100 };
        let bounds = |min: u16, max: u16| (if min == 0 { lo } else { min.clamp(lo, hi) }, if max == 0 { hi } else { max.clamp(lo, hi) });
        let l = self.light;
        let hue_sat = |hue: u8, sat: u8, direction: HueDirection, transition_ms: u32| Command::MoveToHueSaturation { hue, sat, direction, transition_ms };
        let at_rate = |distance: u32, rate: u32| distance * 1000 / rate.max(1);
        let command = match cmd {
            _ if cmd <= 0x09 && !self.identity.color => return Some(Some(UNSUP_COMMAND)),
            0x00 => {
                let hue = r.u8()?;
                let dir = HueDirection::from_u8(r.u8()?);
                let time = r.u16()?;
                let Some(dir) = dir.filter(|_| hue <= 254) else {
                    return Some(Some(INVALID_FIELD));
                };
                hue_sat(hue, l.sat, dir, time_ms(time))
            }
            0x01 => {
                let mode = r.u8()?;
                let rate = r.u8()? as u32;
                match mode {
                    0 => Command::StopColorTransition,
                    1 | 3 if rate == 0 => return Some(Some(INVALID_FIELD)),
                    1 => hue_sat(l.hue.checked_sub(1).unwrap_or(254), l.sat, HueDirection::Up, at_rate(254, rate)),
                    3 => hue_sat(if l.hue >= 254 { 0 } else { l.hue + 1 }, l.sat, HueDirection::Down, at_rate(254, rate)),
                    _ => return Some(Some(INVALID_FIELD)),
                }
            }
            0x02 => {
                let mode = r.u8()?;
                let step = r.u8()? as i16;
                let time = r.u8()? as u16;
                let (hue, dir) = match mode {
                    1 => ((l.hue as i16 + step).rem_euclid(255), HueDirection::Up),
                    3 => ((l.hue as i16 - step).rem_euclid(255), HueDirection::Down),
                    _ => return Some(Some(INVALID_FIELD)),
                };
                hue_sat(hue as u8, l.sat, dir, time_ms(time))
            }
            0x03 => {
                let sat = r.u8()?;
                let time = r.u16()?;
                if sat > 254 {
                    return Some(Some(INVALID_FIELD));
                }
                hue_sat(l.hue, sat, HueDirection::Shortest, time_ms(time))
            }
            0x04 => {
                let mode = r.u8()?;
                let rate = r.u8()? as u32;
                match mode {
                    0 => Command::StopColorTransition,
                    1 | 3 if rate == 0 => return Some(Some(INVALID_FIELD)),
                    1 => hue_sat(l.hue, 254, HueDirection::Shortest, at_rate(254u8.abs_diff(l.sat) as u32, rate)),
                    3 => hue_sat(l.hue, 0, HueDirection::Shortest, at_rate(l.sat as u32, rate)),
                    _ => return Some(Some(INVALID_FIELD)),
                }
            }
            0x05 => {
                let mode = r.u8()?;
                let step = r.u8()?;
                let time = r.u8()? as u16;
                let sat = match mode {
                    1 => l.sat.saturating_add(step).min(254),
                    3 => l.sat.saturating_sub(step),
                    _ => return Some(Some(INVALID_FIELD)),
                };
                hue_sat(l.hue, sat, HueDirection::Shortest, time_ms(time))
            }
            0x06 => {
                let hue = r.u8()?;
                let sat = r.u8()?;
                let time = r.u16()?;
                if hue > 254 || sat > 254 {
                    return Some(Some(INVALID_FIELD));
                }
                hue_sat(hue, sat, HueDirection::Shortest, time_ms(time))
            }
            0x07 => {
                let x = r.u16()?;
                let y = r.u16()?;
                let time = r.u16()?;
                Command::MoveToXy { x, y, transition_ms: time_ms(time) }
            }
            0x08 => {
                let rx = r.u16()? as i16 as i32;
                let ry = r.u16()? as i16 as i32;
                if rx == 0 && ry == 0 {
                    Command::StopColorTransition
                } else {
                    let room = |c: u16, rate: i32| -> u32 {
                        match rate {
                            0 => u32::MAX,
                            r if r > 0 => (0xFEFF - c.min(0xFEFF) as u32) * 1000 / r as u32,
                            r => c as u32 * 1000 / (-r) as u32,
                        }
                    };
                    let ms = room(l.x, rx).min(room(l.y, ry));
                    let to = |c: u16, rate: i32| (c as i64 + rate as i64 * ms as i64 / 1000).clamp(0, 0xFEFF) as u16;
                    Command::MoveToXy { x: to(l.x, rx), y: to(l.y, ry), transition_ms: ms }
                }
            }
            0x09 => {
                let sx = r.u16()? as i16 as i32;
                let sy = r.u16()? as i16 as i32;
                let time = r.u16()?;
                let to = |c: u16, s: i32| (c as i32 + s).clamp(0, 0xFEFF) as u16;
                Command::MoveToXy { x: to(l.x, sx), y: to(l.y, sy), transition_ms: time_ms(time) }
            }
            0x0A => {
                let mireds = r.u16()?;
                let time = r.u16()?;
                Command::MoveToColorTemperature { mireds: mireds.clamp(lo, hi), transition_ms: time_ms(time) }
            }
            0x47 => Command::StopColorTransition,
            0x4B => {
                let mode = r.u8()?;
                let rate = r.u16()?;
                let (min, max) = bounds(r.u16()?, r.u16()?);
                match mode {
                    0 => Command::StopColorTransition,
                    1 | 3 => {
                        if rate == 0 {
                            return Some(Some(INVALID_FIELD));
                        }
                        let target = if mode == 1 { max } else { min };
                        Command::MoveToColorTemperature { mireds: target, transition_ms: target.abs_diff(current) as u32 * 1000 / rate as u32 }
                    }
                    _ => return Some(Some(INVALID_FIELD)),
                }
            }
            0x4C => {
                let mode = r.u8()?;
                let step = r.u16()?;
                let time = r.u16()?;
                let (min, max) = bounds(r.u16()?, r.u16()?);
                let target = match mode {
                    1 => current.saturating_add(step).min(max),
                    3 => current.saturating_sub(step).max(min),
                    _ => return Some(Some(INVALID_FIELD)),
                };
                Command::MoveToColorTemperature { mireds: target, transition_ms: time_ms(time) }
            }
            _ => return Some(Some(UNSUP_COMMAND)),
        };
        if self.light.on {
            self.scene_valid = false;
            self.push_command(command);
        } else if execute_if_off(self.color_options, &mut r) {
            let at_once = match command {
                Command::MoveToColorTemperature { mireds, .. } => Some(Command::MoveToColorTemperature { mireds, transition_ms: 0 }),
                Command::MoveToHueSaturation { hue, sat, direction, .. } => {
                    Some(Command::MoveToHueSaturation { hue, sat, direction, transition_ms: 0 })
                }
                Command::MoveToXy { x, y, .. } => Some(Command::MoveToXy { x, y, transition_ms: 0 }),
                _ => None,
            };
            if let Some(c) = at_once {
                self.scene_valid = false;
                self.push_command(c);
            }
        }
        Some(Some(SUCCESS))
    }

    fn group_status(&self, group: u16) -> u8 {
        if !(0x0001..=0xFFF7).contains(&group) {
            INVALID_VALUE
        } else if self.groups.contains(&group) {
            SUCCESS
        } else {
            NOT_FOUND
        }
    }

    fn group_add(&mut self, group: u16) -> u8 {
        match self.group_status(group) {
            NOT_FOUND => {}
            status => return status,
        }
        if let Some(i) = self.groups.iter().position(|g| *g == 0) {
            self.groups[i] = group;
            self.tables_dirty = true;
            SUCCESS
        } else {
            INSUFFICIENT_SPACE
        }
    }

    pub(crate) fn report_timers(&mut self, now: u32) {
        if self.state != State::Up {
            return;
        }
        for i in 0..self.reports.len() {
            let (cluster, attr, value, ty) = match i {
                REPORT_ON_OFF => (ON_OFF, 0x0000u16, self.light.on as u16, T_BOOL),
                REPORT_LEVEL => (LEVEL, 0x0000, self.light.level as u16, T_U8),
                _ => {
                    if !self.identity.color_temperature {
                        continue;
                    }
                    (COLOR, 0x0007, self.light.color_temp, T_U16)
                }
            };
            let e = self.reports[i];
            if e.max_s == 0xFFFF {
                continue;
            }
            let since = now.wrapping_sub(e.last_sent);
            let changed = if ty == T_BOOL { value != e.last_value } else { value.abs_diff(e.last_value) >= e.change.max(1) };
            let periodic = e.max_s != 0 && since >= e.max_s as u32 * 1000;
            if !(periodic || (changed && since >= e.min_s as u32 * 1000)) {
                continue;
            }
            self.reports[i].last_sent = now;
            self.reports[i].last_value = value;
            self.zcl_seq = self.zcl_seq.wrapping_add(1);
            let a = attr.to_le_bytes();
            let v = value.to_le_bytes();
            let full = [FC_GLOBAL_RSP, self.zcl_seq, REPORT_ATTRIBUTES, a[0], a[1], ty, v[0], v[1]];
            let frame = &full[..if ty == T_U16 { 8 } else { 7 }];
            self.send_to_bindings(ENDPOINT, cluster, frame);
        }
    }
}
