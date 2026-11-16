use bulb_core::nv::NvLog;

use crate::board::BoardFlash;

pub const SECTORS: [usize; 2] = [0xf_1000, 0xf_2000];
const SECTOR_SIZE: usize = 0x1000;

pub type Nv = NvLog<BoardFlash, { zigbee::hal::NvItem::MAX_ID as usize + 1 }>;

pub fn open() -> Nv {
    NvLog::new(BoardFlash, SECTORS, SECTOR_SIZE, crate::board::watchdog_feed)
}
