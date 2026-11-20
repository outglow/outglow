use bulb_core::command::{Command, StateReport};
use bulb_core::hal::Radio;

use crate::cell::SharedBuf;

const CMD_SEQ: usize = 0;
const CMD_PAYLOAD: usize = 1;

const STATE_DONE: usize = 0;
const STATE_ON: usize = 1;
const STATE_LEVEL: usize = 2;
const STATE_POWER_ON_BEHAVIOR: usize = 3;
const STATE_STARTUP_LEVEL: usize = 4;
const STATE_REPORTS: usize = 5;
const STATE_REJECTED: usize = 6;
const STATE_REPORTED: usize = 7;

const NET_STATE: usize = 0;
const NET_CHANNEL: usize = 1;
const NET_SHORT: usize = 2;
const NET_RESET_STEP: usize = 4;
const NET_FACTORY_RESET: usize = 5;
const NET_WARM: usize = 6;

#[allow(unsafe_code)]
#[no_mangle]
pub static BULB_CMD: SharedBuf<8> = SharedBuf::new([0; 8]);
#[allow(unsafe_code)]
#[no_mangle]
pub static BULB_STATE: SharedBuf<8> = SharedBuf::new([0; 8]);
#[allow(unsafe_code)]
#[no_mangle]
pub static BULB_NET: SharedBuf<8> = SharedBuf::new([0, 0, 0xff, 0xff, 0, 0, 0, 0]);

const OP_HANG: u8 = 0xee;

pub fn publish_reset_step(step: u8) {
    BULB_NET.write(NET_RESET_STEP, step);
}

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
            loop {
                crate::reg::tick();
            }
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
        BULB_STATE.write(STATE_REPORTED, BULB_STATE.read(STATE_DONE));
    }
}

#[cfg(test)]
#[path = "tests/link.rs"]
mod tests;
