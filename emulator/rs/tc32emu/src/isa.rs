// SPDX-License-Identifier: Apache-2.0
//! The TC32 instruction encoding as far as it differs from ARMv4T Thumb
//! (tc32-devtools go/tc32isa): TOP maps the TC32 top five bits of a halfword
//! to the Thumb top five bits; the other eleven bits are the same.

pub const TOP: [u16; 32] = [
    0x08, 0x09, 0x0a, 0x0b, 0x10, 0x11, 0x12, 0x13, 0x0e, 0x0f, 0x0c, 0x0d, 0x16, 0x17, 0x14, 0x15, 0x1c, 0x1d, 0x1e,
    0x1f, 0x04, 0x05, 0x06, 0x07, 0x1a, 0x1b, 0x18, 0x19, 0x02, 0x03, 0x00, 0x01,
];

/// INV maps the Thumb top five bits to the TC32 top five bits.
pub const INV: [u16; 32] = {
    let mut inv = [0u16; 32];
    let mut tc = 0;
    while tc < 32 {
        inv[TOP[tc] as usize] = tc as u16;
        tc += 1;
    }
    inv
};

/// ToThumb: the Thumb halfword with the same meaning as TC32 halfword hw.
#[inline]
pub fn to_thumb(hw: u16) -> u16 {
    (TOP[(hw >> 11) as usize] << 11) | (hw & 0x7FF)
}

/// ToTC32: the TC32 halfword with the same meaning as Thumb halfword hw.
#[inline]
pub fn to_tc32(hw: u16) -> u16 {
    (INV[(hw >> 11) as usize] << 11) | (hw & 0x7FF)
}
