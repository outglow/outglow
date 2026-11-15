//! Debug link: commands in and state out through two fixed blocks of SRAM.
//!
//! This stands in for the radio until the firmware has a network stack. A
//! debugger or the emulator's test harness writes a command into `BULB_CMD`
//! and increments its sequence number; the firmware executes it and copies the
//! number to `BULB_STATE.done`.

use bulb_core::command::{Command, StateReport};
use bulb_core::hal::Radio;

use crate::cell::SharedBuf;

/// Command block: `seq` (byte 0), then `bulb_core::Command::decode`'s wire
/// format (bytes 1..8).
const CMD_SEQ: usize = 0;
const CMD_PAYLOAD: usize = 1;

/// State block, rewritten on every report: the sequence number of the last
/// command taken from `BULB_CMD`, the light, reports since boot and commands
/// that did not decode (both wrapping).
const STATE_DONE: usize = 0;
const STATE_ON: usize = 1;
const STATE_LEVEL: usize = 2;
const STATE_POWER_ON_BEHAVIOR: usize = 3;
const STATE_STARTUP_LEVEL: usize = 4;
const STATE_REPORTS: usize = 5;
const STATE_REJECTED: usize = 6;

/// Network block, for tests and debuggers: the stack's state
/// (`zigbee::stack::State` as a number), channel, short address (bytes 2-3,
/// little endian), position in the factory reset sequence of power cycles
/// (0 = none), 1 when this start was a factory reset, 1 when it followed a
/// watchdog or software reset.
const NET_STATE: usize = 0;
const NET_CHANNEL: usize = 1;
const NET_SHORT: usize = 2;
const NET_RESET_STEP: usize = 4;
const NET_FACTORY_RESET: usize = 5;
const NET_WARM: usize = 6;

// The blocks are plain bytes that an outside writer (a debugger, the
// emulator's harness) reads and changes at any time, at these symbols.
#[allow(unsafe_code)] // a linker attribute
#[no_mangle]
pub static BULB_CMD: SharedBuf<8> = SharedBuf::new([0; 8]);
#[allow(unsafe_code)] // a linker attribute
#[no_mangle]
pub static BULB_STATE: SharedBuf<8> = SharedBuf::new([0; 8]);
#[allow(unsafe_code)] // a linker attribute
#[no_mangle]
pub static BULB_NET: SharedBuf<8> = SharedBuf::new([0, 0, 0xff, 0xff, 0, 0, 0, 0]);

/// Debug-link opcode that stops the main loop for good: lets a test see the
/// watchdog restart the firmware.
const OP_HANG: u8 = 0xee;

pub fn publish_reset_step(step: u8) {
    BULB_NET.write(NET_RESET_STEP, step);
}

/// The position in the factory reset sequence, as last published.
#[cfg(feature = "ble")]
pub fn reset_step() -> u8 {
    BULB_NET.read(NET_RESET_STEP)
}

pub fn publish_start(reset_step: u8, factory_reset: bool, warm: bool) {
    BULB_NET.write(NET_RESET_STEP, reset_step);
    BULB_NET.write(NET_FACTORY_RESET, factory_reset as u8);
    BULB_NET.write(NET_WARM, warm as u8);
}

pub fn publish_network(state: u8, short: u16, channel: u8) {
    BULB_NET.write(NET_STATE, state);
    BULB_NET.write(NET_CHANNEL, channel);
    BULB_NET.copy_from(NET_SHORT, &short.to_le_bytes());
}

pub struct DebugLink;

impl Radio for DebugLink {
    fn poll(&mut self) -> Option<Command> {
        let seq = BULB_CMD.read(CMD_SEQ);
        if seq == BULB_STATE.read(STATE_DONE) {
            return None;
        }
        let mut payload = [0u8; 7];
        BULB_CMD.copy_to(CMD_PAYLOAD, &mut payload);
        BULB_STATE.write(STATE_DONE, seq);
        if payload[0] == OP_HANG {
            loop {}
        }
        let decoded = Command::decode(&payload);
        if decoded.is_none() {
            BULB_STATE.write(STATE_REJECTED, BULB_STATE.read(STATE_REJECTED).wrapping_add(1));
        }
        decoded
    }

    fn report(&mut self, s: &StateReport) {
        BULB_STATE.write(STATE_ON, s.on as u8);
        BULB_STATE.write(STATE_LEVEL, s.level);
        BULB_STATE.write(STATE_POWER_ON_BEHAVIOR, s.power_on_behavior.to_u8());
        BULB_STATE.write(STATE_STARTUP_LEVEL, s.startup_level);
        BULB_STATE.write(STATE_REPORTS, BULB_STATE.read(STATE_REPORTS).wrapping_add(1));
    }
}
