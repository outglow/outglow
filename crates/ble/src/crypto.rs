use zigbee::aes::Aes128;

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

pub fn crc32_update(mut crc: u32, byte: u8) -> u32 {
    crc ^= byte as u32;
    for _ in 0..8 {
        crc = if crc & 1 != 0 { crc >> 1 ^ 0xedb8_8320 } else { crc >> 1 };
    }
    crc
}

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
        assert_eq!(b"123456789".iter().fold(0xffff_ffff, |c, b| crc32_update(c, *b)), 0x340b_c6d9);
    }

    #[test]
    fn telink_aes_is_aes_with_reversed_bytes() {
        let mut key: [u8; 16] = core::array::from_fn(|i| i as u8);
        let mut plain: [u8; 16] = core::array::from_fn(|i| (i * 0x11) as u8);
        let mut cipher = [0x69, 0xc4, 0xe0, 0xd8, 0x6a, 0x7b, 0x04, 0x30, 0xd8, 0xcd, 0xb7, 0x80, 0x70, 0xb4, 0xc5, 0x5a];
        key.reverse();
        plain.reverse();
        cipher.reverse();
        assert_eq!(tl_aes_encrypt(&key, &plain), cipher);
        assert_eq!(tl_aes_decrypt(&key, &cipher), plain);
    }

    #[test]
    fn the_access_code_breaks_long_alternating_runs_and_codes_too_close_to_0x55555555() {
        let name = *b"unpaired\0\0\0\0\0\0\0\0";
        let password = |head: &[u8]| {
            let mut p = [0u8; 16];
            p[..head.len()].copy_from_slice(head);
            p
        };
        for (head, code) in [
            (&b"1234"[..], 0xe29c_f430),
            (&[0x72], 0xeda5_ae6a),
            (&[0x53], 0xda30_8095),
            (&[0x4e, 0x04], 0x82f6_1dea),
            (&[0x9b, 0x07], 0x6099_9aea),
            (&[0x8b, 0x3f, 0x07], 0x5555_552b),
            (&[0x26, 0x3d, 0x25], 0x555d_5544),
        ] {
            assert_eq!(mesh_access_code(&name, &password(head)), code, "{head:02x?}");
        }
    }
}
