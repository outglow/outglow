//! The chip's AES-128 block, for the Zigbee stack's frame security.
//!
//! In software one secured frame costs the main loop several milliseconds;
//! the hardware does a block in microseconds. The register sequence is the
//! vendor driver's (`reference/radio/tlsr8258-radio-spec.md`, section 11):
//! key bytes to 0x550.., a write to the control register, four data words
//! to 0x548 while the block asks for them, "finished", four words back.
//!
//! Nothing here has run on silicon, so the block is not trusted blindly:
//! before its first use it must reproduce a known answer, and every use is
//! bounded in time. If either fails, `encrypt` says so and the caller
//! computes in software.

use crate::reg::{clear8, read32, read8, tick, write32, write8, TICKS_PER_US};

const AES_CTRL: usize = 0x540; // bit 0 decrypt, bit 1 wants data, bit 2 finished
const AES_DATA: usize = 0x548;
const AES_KEY: usize = 0x550;
/// A block takes a few microseconds; this is the limit before giving up.
const LIMIT: u32 = 200 * TICKS_PER_US;

/// 0 not tried yet, 1 good, 2 not usable.
static mut STATE: u8 = 0;

fn block(key: &[u8; 16], data: &mut [u8; 16]) -> bool {
    clear8(AES_CTRL, 0x01);
    for (i, b) in key.iter().enumerate() {
        write8(AES_KEY + i, *b);
    }
    let start = tick();
    let mut fed = 0;
    while read8(AES_CTRL) & 0x02 != 0 && fed < 4 {
        write32(AES_DATA, u32::from_le_bytes([data[4 * fed], data[4 * fed + 1], data[4 * fed + 2], data[4 * fed + 3]]));
        fed += 1;
    }
    while read8(AES_CTRL) & 0x04 == 0 {
        if tick().wrapping_sub(start) > LIMIT {
            return false;
        }
    }
    if fed != 4 {
        return false;
    }
    for word in data.chunks_exact_mut(4) {
        word.copy_from_slice(&read32(AES_DATA).to_le_bytes());
    }
    true
}

/// Whether the block reproduces the example of FIPS-197 (appendix C.1).
fn self_test() -> bool {
    let key: [u8; 16] = core::array::from_fn(|i| i as u8);
    let mut data: [u8; 16] = core::array::from_fn(|i| (i * 0x11) as u8);
    block(&key, &mut data)
        && data == [0x69, 0xc4, 0xe0, 0xd8, 0x6a, 0x7b, 0x04, 0x30, 0xd8, 0xcd, 0xb7, 0x80, 0x70, 0xb4, 0xc5, 0x5a]
}

/// Encrypts one block in place with the hardware. False (and `data`
/// untouched or garbage: the caller must keep its own copy) when the
/// hardware is not usable.
pub fn encrypt(key: &[u8; 16], data: &mut [u8; 16]) -> bool {
    // SAFETY: single-threaded; the interrupt handler does not use AES.
    let state = unsafe { (&raw const STATE).read_volatile() };
    let usable = match state {
        0 => {
            let good = self_test();
            // SAFETY: as above.
            unsafe { (&raw mut STATE).write_volatile(if good { 1 } else { 2 }) };
            good
        }
        1 => true,
        _ => false,
    };
    if !usable {
        return false;
    }
    if block(key, data) {
        return true;
    }
    // SAFETY: as above.
    unsafe { (&raw mut STATE).write_volatile(2) };
    false
}
