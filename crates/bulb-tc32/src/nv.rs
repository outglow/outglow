//! Non-volatile store for the Zigbee stack: `bulb_core::nv::NvLog` (a log of
//! records in two 4 KB sectors that take turns, safe against a power cut at
//! any moment: crates/bulb-sim/tests/nv_power_cut.rs) on the chip's flash.

use bulb_core::nv::NvLog;

use crate::board::BoardFlash;

/// The two sectors, after the first configuration sector (see `board::CONFIG_SECTORS`).
pub const SECTORS: [usize; 2] = [0xf_1000, 0xf_2000];
const SECTOR_SIZE: usize = 0x1000;

/// Items 1..=`NvItem::MAX_ID` (`zigbee::hal::NvItem`): a sector switch copies these.
pub type Nv = NvLog<BoardFlash, { zigbee::hal::NvItem::MAX_ID as usize + 1 }>;

pub fn open() -> Nv {
    // a sector switch copies every item: longer than the watchdog's period
    NvLog::new(BoardFlash, SECTORS, SECTOR_SIZE, crate::board::watchdog_feed)
}
