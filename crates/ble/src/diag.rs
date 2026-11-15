//! The diagnostic service for the device page (a web page over Web
//! Bluetooth), byte for byte the one of the C firmware, so that the page's
//! decoder reads both (emulator/rs/scenarios/src/sims/ble_diag.rs decodes
//! it the same way).
//!
//! A service of its own after the stock handles (0x20..=0x2c, `att.rs`):
//!
//! | UUID | | |
//! |---|---|---|
//! | `e6100001-5a4b-4c3d-8e2f-1a2b3c4d5e6f` | info | read: format, variant, flags, release, IEEE, model, firmware |
//! | `e6100002-…` | state | read: 40 bytes, the firmware refreshes them about every second |
//! | `e6100003-…` | log | read: the latest events, newest first |
//! | `e6100004-…` | cmd | write: commands (`parse`) |
//! | `e6100005-…` | frames | read: the latest ZCL frames to the light (`FrameLog`) |
//! | `e6100006-…` | config | read: 20 bytes of settings |
//! | `e6100007-…` | counters | read: the Diagnostics cluster's attributes (`Ble::set_diag_counters`); not in the C firmware |
//!
//! Reading and the blink work without a login; every other command needs
//! the mesh login on this connection, as the AwoX app's commands do. The
//! result of the last command is state[39]: bits 0..3 the code, bits 4..6 a
//! counter of commands (the page sees that the result is a new one), bit 7
//! the connection is logged in. The firmware fills the values
//! (`Ble::set_diag_*`) and carries out what `Ble::take_diag_command` gives it.
//! All fields little-endian.

use bulb_core::command::{Command, PowerOnBehavior};
use zigbee::stack::{ev, LightSettings};

use crate::hal::Hal;
use crate::{Ble, Identity};

/// Longest info value: 14 bytes and the two strings.
pub const INFO_MAX: usize = 48;
pub const STATE_LEN: usize = 40;
/// Bytes of the state the firmware gives; the light and the command result are the BLE side's own.
const STATE_FIRMWARE: usize = 35;
pub const CONFIG_LEN: usize = 20;
/// Events in the log value: 6 bytes each after a count. (The C firmware
/// has 9; the page reads as many as there are.)
pub const LOG_ENTRIES: usize = 16;
pub const LOG_ENTRY: usize = 6;
pub const LOG_MAX: usize = 1 + LOG_ENTRIES * LOG_ENTRY;
/// Frames in the frames value: 12 bytes each after a count of bytes.
pub const FRAMES: usize = 8;
pub const FRAME_ENTRY: usize = 12;
pub const FRAMES_MAX: usize = 1 + FRAMES * FRAME_ENTRY;
/// Longest counters value (`zigbee::stack::DIAG_COUNTERS_MAX`).
pub const COUNTERS_MAX: usize = zigbee::stack::DIAG_COUNTERS_MAX;
/// The time of an event from before this start.
pub const TIME_PREVIOUS: u32 = 0xFFFF_FFFF;

pub(crate) const RESULT_OK: u8 = 0;
pub(crate) const RESULT_NEED_LOGIN: u8 = 1;
pub(crate) const RESULT_INVALID: u8 = 2;

const CMD_IDENTIFY: u8 = 0x01;

/// What a command written to the cmd characteristic asks of the firmware.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagCommand {
    /// Blink for a few seconds (without a login).
    Identify,
    /// A command for the light: On, Off, Toggle, MoveToLevel (with On/Off: 0
    /// turns the light off), MoveToColorTemperature, SetPowerOnBehavior,
    /// SetStartupLevel, SetStartupColorTemperature.
    Light(Command),
    /// The Options attributes of Level Control and of Color Control, the bits this light has.
    LevelOptions(u8),
    ColorOptions(u8),
    /// Restart in about half a second.
    Restart,
    /// Factory reset (the Zigbee network and settings and the BLE mesh) and restart.
    FactoryReset,
}

/// What the firmware tells of itself, once (info).
#[derive(Debug, Clone, Copy)]
pub struct Info<'a> {
    /// The bulb has a colour temperature (the page's "tw").
    pub tunable: bool,
    /// The image has the watchdog, motion sensing.
    pub watchdog: bool,
    pub sense: bool,
    pub release: u8,
    pub ieee: u64,
    pub model: &'a str,
    pub firmware: &'a str,
}

/// The firmware's part of the state value (bytes 0..35). Fields the
/// firmware has nothing for are 0.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct State {
    pub uptime_s: u32,
    /// 0 power-on, 1 watchdog, 2 software reset.
    pub reset_cause: u8,
    /// Resets in a row without a power cut, watchdog resets.
    pub consecutive_resets: u8,
    pub watchdog_resets: u8,
    pub safe_mode: bool,
    /// Step of the factory reset sequence of power cycles (0 none).
    pub reset_step: u8,
    /// Starts counted (NumberOfResets).
    pub starts: u16,
    pub joined: bool,
    pub short: u16,
    pub pan: u16,
    pub channel: u8,
    pub neighbors: u8,
    pub routers: u8,
    pub end_devices: u8,
    pub lqi: u8,
    pub rssi: i8,
    pub mac_rx: u32,
    pub mac_tx: u32,
    pub mac_retries: u16,
    pub mac_failures: u16,
}

/// An event of the log as the page shows it: when (seconds since the
/// start, `TIME_PREVIOUS` before it), what (the C firmware's codes, 1..=11;
/// `zigbee_event` maps the Rust stack's onto them), one byte that says more.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogEntry {
    pub time: u32,
    pub code: u8,
    pub arg: u8,
}

/// The page's event codes shared with the C firmware.
pub mod code {
    pub const START: u8 = 1;
    pub const SAFE_MODE: u8 = 2;
    pub const FACTORY_RESET: u8 = 3;
    pub const RESET_STEP: u8 = 4;
    pub const JOINED: u8 = 5;
    pub const LEFT: u8 = 6;
    pub const STABLE: u8 = 7;
    pub const BLE_CONNECT: u8 = 8;
    pub const BLE_DISCONNECT: u8 = 9;
    pub const IDENTIFY: u8 = 10;
    pub const FAULT: u8 = 11;
    /// The Rust stack's events without a C counterpart: this plus their own number (`zigbee::stack::ev`).
    pub const RUST: u8 = 32;
}

/// An event of the Zigbee stack's log (`zigbee::stack::ev`, its values `a`
/// and `b`) as the page's code and byte. `channel` is the network's channel,
/// which joining tells in the C firmware.
pub fn zigbee_event(kind: u8, a: u16, b: u16, channel: u8) -> (u8, u8) {
    match kind {
        ev::START => (code::START, a as u8),
        ev::UP => (code::JOINED, channel),
        ev::LEFT => (code::LEFT, 0),
        ev::BLE_CONNECT => (code::BLE_CONNECT, 0),
        ev::BLE_DISCONNECT => (code::BLE_DISCONNECT, 0),
        ev::IDENTIFY => (code::IDENTIFY, 0),
        // the link quality of the neighbor, the status code, the key sequence number, the channel
        ev::NEIGHBOR_ADDED | ev::NEIGHBOR_STALE | ev::NEIGHBOR_REMOVED | ev::TX_FAIL => (code::RUST + kind, b as u8),
        // how many links lost so far, whether the rejoin is secured, the rest by its low byte
        _ => (code::RUST + kind, a as u8),
    }
}

/// The latest ZCL frames to the light, newest first, as the frames value
/// carries them: [0] bytes of entries, then per frame: seconds since the
/// start u32, sender u16, cluster u16, command, frame control, LQI, RSSI.
#[derive(Clone, Copy)]
pub struct FrameLog {
    bytes: [u8; FRAMES_MAX],
}

impl FrameLog {
    pub const EMPTY: FrameLog = FrameLog { bytes: [0; FRAMES_MAX] };

    #[allow(clippy::too_many_arguments)]
    pub fn push(&mut self, time_s: u32, src: u16, cluster: u16, cmd: u8, frame_control: u8, lqi: u8, rssi: i8) {
        let e = &mut self.bytes[1..];
        e.copy_within(..(FRAMES - 1) * FRAME_ENTRY, FRAME_ENTRY);
        e[0..4].copy_from_slice(&time_s.to_le_bytes());
        e[4..6].copy_from_slice(&src.to_le_bytes());
        e[6..8].copy_from_slice(&cluster.to_le_bytes());
        e[8] = cmd;
        e[9] = frame_control;
        e[10] = lqi;
        e[11] = rssi as u8;
        self.bytes[0] = (self.bytes[0] as usize + FRAME_ENTRY).min(FRAMES * FRAME_ENTRY) as u8;
    }

    /// The value: the count and the entries there are.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes[..1 + self.bytes[0] as usize]
    }
}

/// The values of the service, as the BLE side keeps them.
pub(crate) struct Diag {
    info: [u8; INFO_MAX],
    info_len: u8,
    state: [u8; STATE_FIRMWARE],
    config: [u8; CONFIG_LEN],
    log: [u8; LOG_MAX],
    frames: FrameLog,
    counters: [u8; COUNTERS_MAX],
    counters_len: u16,
    result: u8,
    count: u8,
    pending: Option<DiagCommand>,
}

impl Diag {
    pub(crate) const fn new() -> Diag {
        let mut info = [0u8; INFO_MAX];
        // until the firmware tells: the format and two empty strings
        info[0] = 1;
        let mut state = [0u8; STATE_FIRMWARE];
        state[0] = 1;
        let mut config = [0u8; CONFIG_LEN];
        config[0] = 1;
        Diag {
            info,
            info_len: 14,
            state,
            config,
            log: [0; LOG_MAX],
            frames: FrameLog::EMPTY,
            counters: [1; COUNTERS_MAX],
            counters_len: 1,
            result: RESULT_OK,
            count: 0,
            pending: None,
        }
    }
}

fn u16_at(d: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*d.get(i)?, *d.get(i + 1)?]))
}

/// A command as written (after a login), as the C firmware takes it; None:
/// invalid parameters, or a command this bulb has no use for.
///
/// | code | parameters | |
/// |---|---|---|
/// | 0x10 | mode | 0 off, 1 on, 2 toggle |
/// | 0x11 | level, time u16 | Move to Level with On/Off, time in tenths of a second |
/// | 0x12 | mireds u16, time u16 | Move to Color Temperature (colour temperature bulbs) |
/// | 0x20 | value | StartUpOnOff (0, 1, 2, 0xFF) |
/// | 0x21 | value | StartUpCurrentLevel |
/// | 0x22 | bits | Level Control options (bit 0 execute if off, bit 1 colour follows the level) |
/// | 0x23 | bits | Color Control options (bit 0) |
/// | 0x24 | mireds u16 | StartUpColorTemperature (0xFFFF previous) |
/// | 0x30 | | restart |
/// | 0x31 | 'R' 'S' 'T' | factory reset |
pub fn parse(d: &[u8], id: &Identity) -> Option<DiagCommand> {
    let tw = id.tunable;
    let in_range = |m: u16| (id.ct_min..=id.ct_max).contains(&m);
    let tenths_ms = |i: usize| u16_at(d, i).map(|t| t as u32 * 100);
    Some(match *d.first()? {
        CMD_IDENTIFY => DiagCommand::Identify,
        0x10 => DiagCommand::Light(match *d.get(1)? {
            0 => Command::Off,
            1 => Command::On,
            2 => Command::Toggle,
            _ => return None,
        }),
        0x11 => DiagCommand::Light(Command::MoveToLevel { level: (*d.get(1)?).min(bulb_core::LEVEL_MAX), transition_ms: tenths_ms(2)? }),
        0x12 if tw => {
            let mireds = u16_at(d, 1).filter(|m| in_range(*m))?;
            DiagCommand::Light(Command::MoveToColorTemperature { mireds, transition_ms: tenths_ms(3)? })
        }
        0x20 => {
            let v = *d.get(1).filter(|v| matches!(v, 0 | 1 | 2 | 0xFF))?;
            DiagCommand::Light(Command::SetPowerOnBehavior(PowerOnBehavior::from_u8(v)))
        }
        0x21 => DiagCommand::Light(Command::SetStartupLevel(*d.get(1)?)),
        // the colour can follow the level only where there is a colour temperature
        0x22 => DiagCommand::LevelOptions(*d.get(1)? & if tw { 0x03 } else { 0x01 }),
        0x23 if tw => DiagCommand::ColorOptions(*d.get(1)? & 0x01),
        0x24 if tw => {
            let m = u16_at(d, 1).filter(|m| *m == 0xFFFF || in_range(*m))?;
            DiagCommand::Light(Command::SetStartupColorTemperature(m))
        }
        0x30 => DiagCommand::Restart,
        0x31 if d.get(1..4) == Some(b"RST") => DiagCommand::FactoryReset,
        _ => return None,
    })
}

impl<H: Hal> Ble<H> {
    /// The info value: once, at the start.
    pub fn set_diag_info(&mut self, i: &Info) {
        let d = &mut self.diag;
        let model = &i.model.as_bytes()[..i.model.len().min(INFO_MAX - 15)];
        let firmware = &i.firmware.as_bytes()[..i.firmware.len().min(INFO_MAX - 14 - model.len())];
        let o = &mut d.info;
        *o = [0; INFO_MAX];
        o[0] = 1;
        o[1] = i.tunable as u8;
        o[2] = i.watchdog as u8 | (i.sense as u8) << 1;
        o[3] = i.release;
        o[4..12].copy_from_slice(&i.ieee.to_le_bytes());
        o[12] = model.len() as u8;
        o[13..13 + model.len()].copy_from_slice(model);
        o[13 + model.len()] = firmware.len() as u8;
        let f = 14 + model.len();
        o[f..f + firmware.len()].copy_from_slice(firmware);
        d.info_len = (f + firmware.len()) as u8;
    }

    /// The firmware's part of the state value: about every second.
    pub fn set_diag_state(&mut self, s: &State) {
        let o = &mut self.diag.state;
        o[0] = 1;
        o[1..5].copy_from_slice(&s.uptime_s.to_le_bytes());
        o[5] = s.reset_cause;
        o[6] = s.consecutive_resets;
        o[7] = s.watchdog_resets;
        o[8] = s.safe_mode as u8;
        o[9] = s.reset_step;
        o[10..12].copy_from_slice(&s.starts.to_le_bytes());
        o[12] = s.joined as u8;
        o[13..15].copy_from_slice(&(if s.joined { s.short } else { 0xFFFF }).to_le_bytes());
        o[15..17].copy_from_slice(&(if s.joined { s.pan } else { 0xFFFF }).to_le_bytes());
        o[17] = s.channel;
        o[18] = s.neighbors;
        o[19] = s.routers;
        o[20] = s.end_devices;
        o[21] = s.lqi;
        o[22] = s.rssi as u8;
        o[23..27].copy_from_slice(&s.mac_rx.to_le_bytes());
        o[27..31].copy_from_slice(&s.mac_tx.to_le_bytes());
        o[31..33].copy_from_slice(&s.mac_retries.to_le_bytes());
        o[33..35].copy_from_slice(&s.mac_failures.to_le_bytes());
    }

    /// The config value, from the light's settings in the stack.
    pub fn set_diag_config(&mut self, c: &LightSettings) {
        let o = &mut self.diag.config;
        o[0] = 1;
        o[1] = c.start_up_on_off;
        o[2] = c.start_up_level;
        o[3] = c.level_options;
        o[4] = c.color_options.unwrap_or(0xFF);
        let sct = if c.color_options.is_some() { c.start_up_color_temperature } else { 0xFFFF };
        o[5..7].copy_from_slice(&sct.to_le_bytes());
        o[7..9].copy_from_slice(&c.ct_min.to_le_bytes());
        o[9..11].copy_from_slice(&c.ct_max.to_le_bytes());
        o[11] = c.global_scene_control as u8;
        o[12..14].copy_from_slice(&c.on_time.to_le_bytes());
        o[14..16].copy_from_slice(&c.off_wait_time.to_le_bytes());
        o[16..18].copy_from_slice(&c.remaining_tenths.to_le_bytes());
        o[18] = c.min_level;
        o[19] = c.max_level;
    }

    /// The log value: the newest events first, as many as fit (`LOG_ENTRIES`).
    pub fn set_diag_log(&mut self, events: impl IntoIterator<Item = LogEntry>) {
        let o = &mut self.diag.log;
        let mut n = 0;
        for e in events.into_iter().take(LOG_ENTRIES) {
            let p = &mut o[1 + n * LOG_ENTRY..1 + (n + 1) * LOG_ENTRY];
            p[0..4].copy_from_slice(&e.time.to_le_bytes());
            p[4] = e.code;
            p[5] = e.arg;
            n += 1;
        }
        o[0] = n as u8;
    }

    /// The frames value.
    pub fn set_diag_frames(&mut self, f: &FrameLog) {
        self.diag.frames = *f;
    }

    /// The counters value: format 1, then per attribute of the Diagnostics
    /// cluster its identifier u16, ZCL type and value (as wide as the type),
    /// as `zigbee::Stack::diag_counters` writes it. About every second.
    pub fn set_diag_counters(&mut self, value: &[u8]) {
        let n = value.len().min(COUNTERS_MAX);
        self.diag.counters[..n].copy_from_slice(&value[..n]);
        self.diag.counters_len = n as u16;
    }

    /// A command written to the cmd characteristic, for the firmware to carry out.
    pub fn take_diag_command(&mut self) -> Option<DiagCommand> {
        self.diag.pending.take()
    }

    pub(crate) fn diag_info(&self) -> &[u8] {
        &self.diag.info[..self.diag.info_len as usize]
    }

    pub(crate) fn diag_state(&self, out: &mut [u8]) -> usize {
        let d = &self.diag;
        out[..STATE_FIRMWARE].copy_from_slice(&d.state);
        let l = &self.light;
        out[35] = l.on as u8;
        out[36] = l.level;
        let mireds = if self.identity.tunable { l.mireds } else { 0xFFFF };
        out[37..39].copy_from_slice(&mireds.to_le_bytes());
        out[39] = (d.result & 0x0F) | (d.count & 0x07) << 4 | (self.mesh.logged_in() as u8) << 7;
        STATE_LEN
    }

    pub(crate) fn diag_config(&self) -> &[u8] {
        &self.diag.config
    }

    pub(crate) fn diag_log(&self) -> &[u8] {
        &self.diag.log[..1 + self.diag.log[0] as usize * LOG_ENTRY]
    }

    pub(crate) fn diag_counters(&self) -> &[u8] {
        &self.diag.counters[..self.diag.counters_len as usize]
    }

    pub(crate) fn diag_frames(&self) -> &[u8] {
        self.diag.frames.bytes()
    }

    /// A write to the cmd characteristic.
    pub(crate) fn diag_write(&mut self, value: &[u8]) {
        let Some(&first) = value.first() else { return };
        let d = if first == CMD_IDENTIFY {
            Ok(DiagCommand::Identify)
        } else if !self.mesh.logged_in() {
            Err(RESULT_NEED_LOGIN)
        } else {
            parse(value, &self.identity).ok_or(RESULT_INVALID)
        };
        self.diag.result = match d {
            Ok(c) => {
                self.diag.pending = Some(c);
                RESULT_OK
            }
            Err(r) => r,
        };
        self.diag.count = self.diag.count.wrapping_add(1);
    }
}
