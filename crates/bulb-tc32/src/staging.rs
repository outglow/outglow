//! The firmware staging area: where the stock firmware's BLE update puts a
//! new image and the resident bootloader looks for one
//! (docs/ble-first-flash.md). Both update paths of this firmware, Zigbee
//! and BLE, write here, one at a time, and both hand over to the
//! bootloader through `activate`, which checks what the flash really holds.

use crate::board;
use crate::flash;

pub const BASE: usize = 0x6000;
pub const SIZE: u32 = 0x6_4000;
/// Offset of the byte that marks a staged image as bootable ("K" of "KNLT").
const BOOT_MARK: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    Zigbee = 1,
    #[cfg_attr(not(feature = "ble"), allow(dead_code))]
    Ble = 2,
}

static mut OWNER: u8 = 0;

fn owner() -> u8 {
    // SAFETY: single-threaded; never touched from the interrupt handler.
    unsafe { (&raw const OWNER).read_volatile() }
}

/// Takes the staging area for one update path. False while the other has it.
pub fn claim(who: Owner) -> bool {
    if owner() != 0 && owner() != who as u8 {
        return false;
    }
    // SAFETY: as in `owner`.
    unsafe { (&raw mut OWNER).write_volatile(who as u8) };
    true
}

pub fn holds(who: Owner) -> bool {
    owner() == who as u8
}

/// Gives the staging area up (the transfer ended or was abandoned).
pub fn release(who: Owner) {
    if holds(who) {
        // SAFETY: as in `owner`.
        unsafe { (&raw mut OWNER).write_volatile(0) };
    }
}

fn crc32_update(mut crc: u32, byte: u8) -> u32 {
    const TABLE: [u32; 16] = [
        0x0000_0000, 0x1db7_1064, 0x3b6e_20c8, 0x26d9_30ac, 0x76dc_4190, 0x6b6b_51f4, 0x4db2_6158, 0x5005_713c,
        0xedb8_8320, 0xf00f_9344, 0xd6d6_a3e8, 0xcb61_b38c, 0x9b64_c2b0, 0x86d3_d2d4, 0xa00a_e278, 0xbdbd_f21c,
    ];
    crc ^= byte as u32;
    crc = (crc >> 4) ^ TABLE[(crc & 0x0f) as usize];
    (crc >> 4) ^ TABLE[(crc & 0x0f) as usize]
}

/// Whether the flash holds a complete image the way the bootloader wants
/// it, short of the bootable mark: "KNLT" at 8 with its first byte still
/// blank, its own size at 0x18, and at its end the CRC-32 of everything
/// before (computed with the mark in place).
fn image_good() -> bool {
    let mut head = [0u8; 0x20];
    flash::read(BASE, &mut head);
    let size = u32::from_le_bytes([head[0x18], head[0x19], head[0x1a], head[0x1b]]);
    if head[BOOT_MARK] != 0xff || head[9..12] != *b"NLT" || !(0x40..=SIZE).contains(&size) {
        return false;
    }
    let body = size as usize - 4;
    let mut crc = 0xffff_ffffu32;
    let mut at = 0;
    let mut chunk = [0u8; 64];
    while at < body {
        let n = (body - at).min(chunk.len());
        flash::read(BASE + at, &mut chunk[..n]);
        if at == 0 {
            chunk[BOOT_MARK] = 0x4b;
        }
        for b in &chunk[..n] {
            crc = crc32_update(crc, *b);
        }
        at += n;
        if at % 0x4000 == 0 {
            // a quarter of a megabyte takes a moment
            board::watchdog_feed();
        }
    }
    let mut stored = [0u8; 4];
    flash::read(BASE + body, &mut stored);
    u32::from_le_bytes(stored) == crc
}

/// Marks the staged image bootable and restarts into the bootloader, if
/// the flash holds a good image. Returns (false) only when it does not.
pub fn activate() -> bool {
    if !image_good() {
        return false;
    }
    flash::write(BASE + BOOT_MARK, &[0x4b]);
    board::reboot()
}
