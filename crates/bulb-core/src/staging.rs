//! The rules of the firmware staging area (on the bulb 0x6000, where the
//! stock firmware's BLE update puts a new image and the resident bootloader
//! looks for one; `crates/bulb-tc32/src/staging.rs`), apart from the chip:
//!
//! - one update path at a time owns the area, Zigbee or BLE ([`Ownership`]);
//! - the image is made bootable (0x4B at offset 8, the "K" of "KNLT") only
//!   once what the flash really holds checks out ([`mark_bootable`]): the
//!   header, its size, and the CRC-32 at its end, computed with the mark in
//!   place as the bootloader computes it.

use crate::store::Flash;

/// Offset of the byte that marks a staged image as bootable.
pub const BOOT_MARK: usize = 8;
/// The bootable mark ("K" of "KNLT"); erased (0xFF) while the image comes in.
pub const MARK: u8 = 0x4b;
/// The smallest image the check takes (the header and the CRC).
const MIN_SIZE: u32 = 0x40;

/// An update path that writes to the staging area.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Owner {
    Zigbee = 1,
    Ble = 2,
}

/// Which update path has the staging area, if one has.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Ownership(u8);

impl Ownership {
    pub const FREE: Ownership = Ownership(0);

    /// Takes the area for `who` (again, when it has it already). False while
    /// the other path has it.
    pub fn claim(&mut self, who: Owner) -> bool {
        if self.0 != 0 && self.0 != who as u8 {
            return false;
        }
        self.0 = who as u8;
        true
    }

    pub fn holds(&self, who: Owner) -> bool {
        self.0 == who as u8
    }

    /// Gives the area up (the transfer ended or was abandoned); only the
    /// path that has it can.
    pub fn release(&mut self, who: Owner) {
        if self.holds(who) {
            self.0 = 0;
        }
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

/// Whether the flash at `base` holds a complete image the way the
/// bootloader wants it, short of the bootable mark: "KNLT" at 8 with its
/// first byte still blank, its own size at 0x18 (at most `area`), and at its
/// end the CRC-32 of everything before (computed with the mark in place).
/// `feed` is called every 16 KB: a large image takes a moment.
pub fn image_good<F: Flash>(flash: &F, base: usize, area: u32, feed: fn()) -> bool {
    let mut head = [0u8; 0x20];
    flash.read(base, &mut head);
    let size = u32::from_le_bytes([head[0x18], head[0x19], head[0x1a], head[0x1b]]);
    if head[BOOT_MARK] != 0xff || head[9..12] != *b"NLT" || !(MIN_SIZE..=area).contains(&size) {
        return false;
    }
    let body = size as usize - 4;
    let mut crc = 0xffff_ffffu32;
    let mut at = 0;
    let mut chunk = [0u8; 64];
    while at < body {
        let n = (body - at).min(chunk.len());
        flash.read(base + at, &mut chunk[..n]);
        if at == 0 {
            chunk[BOOT_MARK] = MARK;
        }
        for b in &chunk[..n] {
            crc = crc32_update(crc, *b);
        }
        at += n;
        if at % 0x4000 == 0 {
            feed();
        }
    }
    let mut stored = [0u8; 4];
    flash.read(base + body, &mut stored);
    u32::from_le_bytes(stored) == crc
}

/// Marks the staged image at `base` bootable if the flash holds a good one
/// ([`image_good`]); whether it did. Nothing is written otherwise.
pub fn mark_bootable<F: Flash>(flash: &mut F, base: usize, area: u32, feed: fn()) -> bool {
    if !image_good(flash, base, area, feed) {
        return false;
    }
    flash.program(base + BOOT_MARK, &[MARK]);
    true
}
