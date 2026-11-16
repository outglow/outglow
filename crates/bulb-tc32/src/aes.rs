use crate::cell::Shared;
use crate::reg::{clear8, read32, read8, tick, write32, write8, TICKS_PER_US};

const AES_CTRL: usize = 0x540;
const AES_DATA: usize = 0x548;
const AES_KEY: usize = 0x550;
const LIMIT: u32 = 200 * TICKS_PER_US;

static STATE: Shared<u8> = Shared::new(0);

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

fn self_test() -> bool {
    let key: [u8; 16] = core::array::from_fn(|i| i as u8);
    let mut data: [u8; 16] = core::array::from_fn(|i| (i * 0x11) as u8);
    block(&key, &mut data)
        && data == [0x69, 0xc4, 0xe0, 0xd8, 0x6a, 0x7b, 0x04, 0x30, 0xd8, 0xcd, 0xb7, 0x80, 0x70, 0xb4, 0xc5, 0x5a]
}

pub fn encrypt(key: &[u8; 16], data: &mut [u8; 16]) -> bool {
    let state = STATE.get();
    let usable = match state {
        0 => {
            let good = self_test();
            STATE.set(if good { 1 } else { 2 });
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
    STATE.set(2);
    false
}
