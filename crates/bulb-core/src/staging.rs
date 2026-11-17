use crate::store::Flash;

pub const BOOT_MARK: usize = 8;
pub const MARK: u8 = 0x4b;
const MIN_SIZE: u32 = 0x40;
const CHUNK: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Owner {
    Zigbee = 1,
    Ble = 2,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Ownership(u8);

impl Ownership {
    pub const FREE: Ownership = Ownership(0);

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

pub fn image_good<F: Flash>(flash: &F, base: usize, area: u32, feed: fn()) -> bool {
    let mut head = [0u8; 0x20];
    flash.read(base, &mut head);
    let size = u32::from_le_bytes([head[0x18], head[0x19], head[0x1a], head[0x1b]]);
    if head[BOOT_MARK] != 0xff || head[9..12] != *b"NLT" || !(MIN_SIZE..=area).contains(&size) {
        return false;
    }
    let body = size as usize - 4;
    let mut crc = 0xffff_ffffu32;
    for at in (0..body).step_by(CHUNK) {
        let mut chunk = [0u8; CHUNK];
        let n = (body - at).min(CHUNK);
        flash.read(base + at, &mut chunk[..n]);
        if at == 0 {
            chunk[BOOT_MARK] = MARK;
        }
        for b in &chunk[..n] {
            crc = crc32_update(crc, *b);
        }
        if (at + n) % 0x4000 == 0 {
            feed();
        }
    }
    let mut stored = [0u8; 4];
    flash.read(base + body, &mut stored);
    u32::from_le_bytes(stored) == crc
}

pub fn mark_bootable<F: Flash>(flash: &mut F, base: usize, area: u32, feed: fn()) -> bool {
    if !image_good(flash, base, area, feed) {
        return false;
    }
    flash.program(base + BOOT_MARK, &[MARK]);
    true
}
