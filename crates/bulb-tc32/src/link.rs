//! Debug link: commands in and state out through two fixed blocks of SRAM.
//!
//! This stands in for the radio until the firmware has a network stack. A
//! debugger or the emulator's test harness writes a command into `BULB_CMD`
//! and increments its sequence number; the firmware executes it and copies the
//! number to `BULB_STATE.done`.

use bulb_core::command::{Command, StateReport};
use bulb_core::hal::Radio;

/// Command block. `payload` is `bulb_core::Command::decode`'s wire format.
#[repr(C)]
pub struct CommandBlock {
    pub seq: u8,
    pub payload: [u8; 7],
}

/// State block, rewritten on every report.
#[repr(C)]
pub struct StateBlock {
    /// Sequence number of the last command taken from `BULB_CMD`.
    pub done: u8,
    pub on: u8,
    pub level: u8,
    pub power_on_behavior: u8,
    pub startup_level: u8,
    /// Number of reports since boot, wrapping.
    pub reports: u8,
    /// Number of commands that did not decode, wrapping.
    pub rejected: u8,
    pub _reserved: u8,
}

#[no_mangle]
pub static mut BULB_CMD: CommandBlock = CommandBlock { seq: 0, payload: [0; 7] };
#[no_mangle]
pub static mut BULB_STATE: StateBlock = StateBlock {
    done: 0,
    on: 0,
    level: 0,
    power_on_behavior: 0,
    startup_level: 0,
    reports: 0,
    rejected: 0,
    _reserved: 0,
};

/// Network state, for tests and debuggers: the stack's state
/// (`zigbee::stack::State` as a number), short address and channel.
#[repr(C)]
pub struct NetworkBlock {
    pub state: u8,
    pub channel: u8,
    pub short: u16,
    /// Position in the factory reset sequence of power cycles (0 = none).
    pub reset_step: u8,
    /// 1 when this start was a factory reset.
    pub factory_reset: u8,
    /// 1 when this start followed a watchdog or software reset.
    pub warm: u8,
    pub _reserved: u8,
}

#[no_mangle]
pub static mut BULB_NET: NetworkBlock =
    NetworkBlock { state: 0, channel: 0, short: 0xffff, reset_step: 0, factory_reset: 0, warm: 0, _reserved: 0 };

/// Debug-link opcode that stops the main loop for good: lets a test see the
/// watchdog restart the firmware.
const OP_HANG: u8 = 0xee;

pub fn publish_reset_step(step: u8) {
    // SAFETY: single-threaded; read from outside only.
    unsafe { (&raw mut (*(&raw mut BULB_NET)).reset_step).write_volatile(step) };
}

pub fn publish_start(reset_step: u8, factory_reset: bool, warm: bool) {
    // SAFETY: single-threaded; read from outside only.
    unsafe {
        let net = &raw mut BULB_NET;
        (&raw mut (*net).reset_step).write_volatile(reset_step);
        (&raw mut (*net).factory_reset).write_volatile(factory_reset as u8);
        (&raw mut (*net).warm).write_volatile(warm as u8);
    }
}

pub fn publish_network(state: u8, short: u16, channel: u8) {
    // SAFETY: single-threaded; read from outside only.
    unsafe {
        let net = &raw mut BULB_NET;
        (&raw mut (*net).state).write_volatile(state);
        (&raw mut (*net).channel).write_volatile(channel);
        (&raw mut (*net).short).write_volatile(short);
    }
}

pub struct DebugLink;

impl Radio for DebugLink {
    fn poll(&mut self) -> Option<Command> {
        // SAFETY: single-threaded; the blocks are plain bytes that an outside
        // writer may change at any time, hence the volatile accesses.
        unsafe {
            let cmd = &raw const BULB_CMD;
            let state = &raw mut BULB_STATE;
            let seq = (&raw const (*cmd).seq).read_volatile();
            if seq == (&raw const (*state).done).read_volatile() {
                return None;
            }
            let payload = (&raw const (*cmd).payload).read_volatile();
            (&raw mut (*state).done).write_volatile(seq);
            if payload[0] == OP_HANG {
                loop {}
            }
            let decoded = Command::decode(&payload);
            if decoded.is_none() {
                let n = (&raw const (*state).rejected).read_volatile();
                (&raw mut (*state).rejected).write_volatile(n.wrapping_add(1));
            }
            decoded
        }
    }

    fn report(&mut self, s: &StateReport) {
        // SAFETY: as in `poll`.
        unsafe {
            let state = &raw mut BULB_STATE;
            (&raw mut (*state).on).write_volatile(s.on as u8);
            (&raw mut (*state).level).write_volatile(s.level);
            (&raw mut (*state).power_on_behavior).write_volatile(s.power_on_behavior.to_u8());
            (&raw mut (*state).startup_level).write_volatile(s.startup_level);
            let n = (&raw const (*state).reports).read_volatile();
            (&raw mut (*state).reports).write_volatile(n.wrapping_add(1));
        }
    }
}
