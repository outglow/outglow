//! The BLE diagnostic service of the device page (`ble::diag`) on this
//! firmware: its values from the stack and the board, and its commands
//! carried out.

use crate::cell::Shared;
use ble::diag::{zigbee_event, DiagCommand, FrameLog, Info, LogEntry, State, TIME_PREVIOUS};
use bulb_core::Command;
use zigbee::stack::DIAG_COUNTERS_MAX;
use zigbee::{Hal as _, Stack};

use crate::ble::IDENTITY;
use crate::zb::Tc32Hal;

/// The latest ZCL frames to the light (`zigbee::Hal::zcl_received`), outside
/// the stack, and how many have come (for the BLE side to copy them when new).
static FRAMES: Shared<FrameLog> = Shared::new(FrameLog::EMPTY);
static FRAMES_IN: Shared<u32> = Shared::new(0);

/// How often the state, the settings and the counters are refreshed.
const REFRESH_MS: u32 = 1_000;
/// A restart asked for waits this long: the central gets its write response.
const RESTART_MS: u32 = 500;

pub fn frame(time_s: u32, src: u16, cluster: u16, cmd: u8, frame_control: u8, lqi: u8, rssi: i8) {
    FRAMES.update(|mut log| {
        log.push(time_s, src, cluster, cmd, frame_control, lqi, rssi);
        log
    });
    FRAMES_IN.update(|n| n.wrapping_add(1));
}

fn reached(now: u32, t: u32) -> bool {
    now.wrapping_sub(t) as i32 >= 0
}

pub struct Glue {
    refresh_at: u32,
    frames_seen: u32,
    log_seq: Option<u16>,
    /// The BLE side's connections, disconnections and links lost as last seen.
    links: (u32, u32, u32),
    /// A restart asked for: when, and whether a factory reset goes first.
    restart: Option<(u32, bool)>,
}

impl Glue {
    pub const fn new() -> Glue {
        Glue { refresh_at: 0, frames_seen: 0, log_seq: None, links: (0, 0, 0), restart: None }
    }

    /// A BLE side has started: what the firmware is, and everything else at once.
    pub fn start(&mut self, ble: &mut ble::Ble<crate::ble::BleHal>, stack: &mut Stack<Tc32Hal>) {
        ble.set_diag_info(&Info {
            tunable: IDENTITY.tunable,
            watchdog: true,
            // no motion sensing in this firmware
            sense: false,
            // the release is the low byte of the OTA file version (0x00020004: 4)
            release: crate::zb::FILE_VERSION as u8,
            ieee: stack.hal.ieee(),
            model: IDENTITY.model,
            firmware: IDENTITY.firmware_revision,
        });
        self.refresh_at = stack.hal.now_ms();
        self.frames_seen = self.frames_seen.wrapping_sub(1);
        self.log_seq = None;
        self.refresh(ble, stack);
    }

    /// Every round: new frames and events at once, the rest about every second.
    pub fn refresh(&mut self, ble: &mut ble::Ble<crate::ble::BleHal>, stack: &mut Stack<Tc32Hal>) {
        let frames_in = FRAMES_IN.get();
        if frames_in != self.frames_seen {
            self.frames_seen = frames_in;
            ble.set_diag_frames(&FRAMES.get());
        }
        let st = ble.stats;
        let links = (st.connections, st.disconnects, st.links_lost);
        if links != self.links {
            // in the log at once, as the C firmware has them (the counters `zb.rs` gives the stack are this round's)
            self.links = links;
            stack.sync_hardware_counters();
        }
        if self.log_seq != Some(stack.event_seq()) {
            self.log_seq = Some(stack.event_seq());
            let channel = stack.channel();
            ble.set_diag_log((0..).map_while(|i| stack.event(i)).map(|e| {
                let (code, arg) = zigbee_event(e.kind, e.a, e.b, channel);
                LogEntry { time: if e.this_start { e.t } else { TIME_PREVIOUS }, code, arg }
            }));
        }
        let now = stack.hal.now_ms();
        if !reached(now, self.refresh_at) {
            return;
        }
        self.refresh_at = now.wrapping_add(REFRESH_MS);
        let (starts, watchdog_resets) = stack.resets();
        let n = stack.neighbor_counts();
        let (lqi, rssi) = stack.last_signal();
        let mac = stack.mac_counters();
        ble.set_diag_state(&State {
            uptime_s: stack.uptime_s(),
            reset_cause: stack.reset_cause() as u8,
            watchdog_resets: watchdog_resets.min(255) as u8,
            reset_step: crate::link::reset_step(),
            starts,
            joined: stack.is_joined(),
            short: stack.short_address(),
            pan: stack.pan_id(),
            channel: stack.channel(),
            neighbors: n.routers.saturating_add(n.end_devices),
            routers: n.routers,
            end_devices: n.end_devices,
            lqi,
            rssi,
            mac_rx: mac.rx_ucast,
            mac_tx: mac.tx_ucast,
            mac_retries: mac.tx_ucast_retries as u16,
            mac_failures: mac.tx_ucast_failures as u16,
            // no counterparts here: safe mode, resets in a row
            ..State::default()
        });
        ble.set_diag_config(&stack.light_settings());
        let mut counters = [0u8; DIAG_COUNTERS_MAX];
        let len = stack.diag_counters(&mut counters);
        ble.set_diag_counters(&counters[..len]);
    }

    /// Carries out a command of the page; a command for the light is returned.
    pub fn execute(&mut self, c: DiagCommand, stack: &mut Stack<Tc32Hal>) -> Option<Command> {
        let now = stack.hal.now_ms();
        let at = now.wrapping_add(RESTART_MS);
        // the next round shows what changed (the light reports before it)
        self.refresh_at = now;
        match c {
            DiagCommand::Identify => {
                stack.log_identify();
                return Some(Command::Identify { times: 3 });
            }
            DiagCommand::Light(c) => return Some(c),
            DiagCommand::LevelOptions(v) => {
                stack.set_level_options(v);
            }
            DiagCommand::ColorOptions(v) => {
                stack.set_color_options(v);
            }
            DiagCommand::Restart => self.restart = Some((at, false)),
            DiagCommand::FactoryReset => self.restart = Some((at, true)),
        }
        None
    }

    /// The restart asked for, once it is due.
    pub fn restart_due(&mut self, stack: &mut Stack<Tc32Hal>) {
        let Some((at, factory)) = self.restart else { return };
        if !reached(stack.hal.now_ms(), at) {
            return;
        }
        if factory {
            crate::factory_reset();
            // the counters and the log kept for a warm restart go too
            stack.hal.retain(&[]);
        } else {
            stack.keep_for_restart();
        }
        crate::board::reboot()
    }
}
