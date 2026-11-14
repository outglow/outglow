//! Primitives of the AwoX/Telink mesh protocol.

use zigbee::aes::Aes128;

/// AES-128 as Telink's mesh uses it: key, input and output byte-reversed.
pub fn tl_aes_encrypt(key: &[u8; 16], data: &[u8; 16]) -> [u8; 16] {
    let mut k = *key;
    k.reverse();
    let mut d = *data;
    d.reverse();
    Aes128::new(&k).encrypt(&mut d);
    d.reverse();
    d
}

pub fn tl_aes_decrypt(key: &[u8; 16], data: &[u8; 16]) -> [u8; 16] {
    let mut k = *key;
    k.reverse();
    let mut d = *data;
    d.reverse();
    Aes128::new(&k).decrypt(&mut d);
    d.reverse();
    d
}

/// Checksum of a mesh packet: CBC-MAC over the payload, started from the
/// nonce and the payload length.
pub fn mesh_mic(key: &[u8; 16], nonce: &[u8; 8], payload: &[u8]) -> [u8; 16] {
    let mut base = [0u8; 16];
    base[..8].copy_from_slice(nonce);
    base[8] = payload.len() as u8;
    let mut chk = tl_aes_encrypt(key, &base);
    for block in payload.chunks(16) {
        for (c, b) in chk.iter_mut().zip(block) {
            *c ^= *b;
        }
        chk = tl_aes_encrypt(key, &chk);
    }
    chk
}

/// Encrypts or decrypts a mesh payload in place: counter mode with the
/// block counter in byte 0 and the nonce behind it.
pub fn mesh_crypt(key: &[u8; 16], nonce: &[u8; 8], payload: &mut [u8]) {
    let mut base = [0u8; 16];
    base[1..9].copy_from_slice(nonce);
    for block in payload.chunks_mut(16) {
        let e = tl_aes_encrypt(key, &base);
        for (b, e) in block.iter_mut().zip(e) {
            *b ^= e;
        }
        base[0] = base[0].wrapping_add(1);
    }
}

/// CRC-16/MODBUS, the check of the firmware update packets.
pub fn crc16_modbus(data: &[u8]) -> u16 {
    let mut crc = 0xffffu16;
    for b in data {
        crc ^= *b as u16;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { crc >> 1 ^ 0xa001 } else { crc >> 1 };
        }
    }
    crc
}

/// CRC-8 with the reflected polynomial 0x6c: the check inside the stock
/// firmware's command blocks.
pub fn crc8_6c(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for b in data {
        let mut r = *b;
        for _ in 0..8 {
            let mix = (crc ^ r) & 1;
            crc >>= 1;
            if mix != 0 {
                crc ^= 0x6c;
            }
            r >>= 1;
        }
    }
    crc
}

/// One step of the CRC-32 Telink's bootloader checks images with
/// (reflected 0x04C11DB7, start 0xFFFFFFFF, no final inversion).
pub fn crc32_update(mut crc: u32, byte: u8) -> u32 {
    crc ^= byte as u32;
    for _ in 0..8 {
        crc = if crc & 1 != 0 { crc >> 1 ^ 0xedb8_8320 } else { crc >> 1 };
    }
    crc
}

/// The access code of a mesh network on air, from its name and password
/// (Telink light-mesh): AES of the two, then adjusted so that the radio
/// can synchronise on it (no long runs of alternating bits, enough transitions).
pub fn mesh_access_code(name: &[u8; 16], password: &[u8; 16]) -> u32 {
    let enc = tl_aes_encrypt(name, password);
    let mut r = u32::from_le_bytes([enc[0], enc[1], enc[2], enc[3]]);
    let mut pat = (r >> 1) ^ r;
    let mut ones = 0;
    for bit in 1..32 {
        if pat & 1 == 0 {
            break;
        }
        ones += 1;
        if ones > 5 {
            r ^= 1 << bit;
            ones = 0;
        }
        pat >>= 1;
    }
    let mut test = 0xaaaa_aaaau32;
    for _ in 0..2 {
        if test.count_ones() < 3 {
            r ^= 0xff;
        }
        test = r ^ 0x5555_5555;
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums_match_their_published_check_values() {
        assert_eq!(crc16_modbus(b"123456789"), 0x4b37);
        // CRC-32/JAMCRC is CRC-32 without the final inversion
        assert_eq!(b"123456789".iter().fold(0xffff_ffff, |c, b| crc32_update(c, *b)), 0x340b_c6d9);
    }

    #[test]
    fn telink_aes_is_aes_with_reversed_bytes() {
        // FIPS-197 appendix C.1, reversed
        let mut key: [u8; 16] = core::array::from_fn(|i| i as u8);
        let mut plain: [u8; 16] = core::array::from_fn(|i| (i * 0x11) as u8);
        let mut cipher = [0x69, 0xc4, 0xe0, 0xd8, 0x6a, 0x7b, 0x04, 0x30, 0xd8, 0xcd, 0xb7, 0x80, 0x70, 0xb4, 0xc5, 0x5a];
        key.reverse();
        plain.reverse();
        cipher.reverse();
        assert_eq!(tl_aes_encrypt(&key, &plain), cipher);
        assert_eq!(tl_aes_decrypt(&key, &cipher), plain);
    }
}
