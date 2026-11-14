//! Zigbee security primitives on top of AES-128: CCM* with a 4-byte MIC
//! (security level 5, the only one Zigbee PRO uses), the Matyas-Meyer-Oseas
//! hash and the keyed hash that derives the key-transport, key-load and
//! verify-key hashes from a link key (ZigBee spec 05-3474, annexes A and B).

use crate::aes::Aes128;

pub const MIC_LEN: usize = 4;

fn xor_block(x: &mut [u8; 16], data: &[u8]) {
    for (a, b) in x.iter_mut().zip(data) {
        *a ^= *b;
    }
}

/// CBC-MAC over the length-prefixed additional data and the message.
fn ccm_tag(aes: &Aes128, nonce: &[u8; 13], aad: &[u8], msg: &[u8]) -> [u8; 16] {
    let mut x = [0u8; 16];
    // B0: flags (Adata, M = 4, L = 2), nonce, message length
    x[0] = (if aad.is_empty() { 0 } else { 0x40 }) | (((MIC_LEN as u8 - 2) / 2) << 3) | 1;
    x[1..14].copy_from_slice(nonce);
    x[14] = (msg.len() >> 8) as u8;
    x[15] = msg.len() as u8;
    aes.encrypt(&mut x);
    if !aad.is_empty() {
        // first block: 2-byte length, then up to 14 bytes of data
        let first = aad.len().min(14);
        x[0] ^= (aad.len() >> 8) as u8;
        x[1] ^= aad.len() as u8;
        for i in 0..first {
            x[2 + i] ^= aad[i];
        }
        aes.encrypt(&mut x);
        for chunk in aad[first..].chunks(16) {
            xor_block(&mut x, chunk);
            aes.encrypt(&mut x);
        }
    }
    for chunk in msg.chunks(16) {
        xor_block(&mut x, chunk);
        aes.encrypt(&mut x);
    }
    x
}

fn ccm_ctr(aes: &Aes128, nonce: &[u8; 13], counter: u16) -> [u8; 16] {
    let mut a = [0u8; 16];
    a[0] = 1;
    a[1..14].copy_from_slice(nonce);
    a[14] = (counter >> 8) as u8;
    a[15] = counter as u8;
    aes.encrypt(&mut a);
    a
}

fn ccm_crypt(aes: &Aes128, nonce: &[u8; 13], msg: &mut [u8]) {
    for (i, chunk) in msg.chunks_mut(16).enumerate() {
        let s = ccm_ctr(aes, nonce, i as u16 + 1);
        for (b, k) in chunk.iter_mut().zip(&s) {
            *b ^= *k;
        }
    }
}

/// Encrypts `buf[..len]` in place and writes the MIC into
/// `buf[len..len + 4]`. Returns the new length, or `None` when the MIC does
/// not fit.
pub fn ccm_encrypt(aes: &Aes128, nonce: &[u8; 13], aad: &[u8], buf: &mut [u8], len: usize) -> Option<usize> {
    if buf.len() < len + MIC_LEN {
        return None;
    }
    let tag = ccm_tag(aes, nonce, aad, &buf[..len]);
    ccm_crypt(aes, nonce, &mut buf[..len]);
    let s0 = ccm_ctr(aes, nonce, 0);
    for i in 0..MIC_LEN {
        buf[len + i] = tag[i] ^ s0[i];
    }
    Some(len + MIC_LEN)
}

/// Decrypts `buf` (ciphertext followed by the MIC) in place. Returns the
/// plaintext length when the MIC verifies.
pub fn ccm_decrypt(aes: &Aes128, nonce: &[u8; 13], aad: &[u8], buf: &mut [u8]) -> Option<usize> {
    let len = buf.len().checked_sub(MIC_LEN)?;
    ccm_crypt(aes, nonce, &mut buf[..len]);
    let tag = ccm_tag(aes, nonce, aad, &buf[..len]);
    let s0 = ccm_ctr(aes, nonce, 0);
    let mut diff = 0u8;
    for i in 0..MIC_LEN {
        diff |= buf[len + i] ^ tag[i] ^ s0[i];
    }
    if diff == 0 {
        Some(len)
    } else {
        None
    }
}

/// Matyas-Meyer-Oseas hash with AES-128 over the concatenation of `parts`
/// (total length below 8 KiB, which is all Zigbee asks of it).
pub fn mmo_hash(parts: &[&[u8]]) -> [u8; 16] {
    let mut h = [0u8; 16];
    let mut block = [0u8; 16];
    let mut fill = 0usize;
    let mut total = 0usize;
    let absorb = |h: &mut [u8; 16], block: &[u8; 16]| {
        let mut e = *block;
        Aes128::new(h).encrypt(&mut e);
        for i in 0..16 {
            h[i] = e[i] ^ block[i];
        }
    };
    for part in parts {
        for &b in part.iter() {
            block[fill] = b;
            fill += 1;
            total += 1;
            if fill == 16 {
                absorb(&mut h, &block);
                fill = 0;
            }
        }
    }
    // padding: 0x80, zeros, 16-bit big-endian bit length in the last two bytes
    block[fill] = 0x80;
    fill += 1;
    if fill > 14 {
        for b in block[fill..].iter_mut() {
            *b = 0;
        }
        absorb(&mut h, &block);
        fill = 0;
    }
    for b in block[fill..14].iter_mut() {
        *b = 0;
    }
    let bits = total * 8;
    block[14] = (bits >> 8) as u8;
    block[15] = bits as u8;
    absorb(&mut h, &block);
    h
}

/// HMAC with the MMO hash (FIPS 198 construction, block size 16).
/// CRC-16/X-25 (as in the 802.15.4 FCS, but started at 0xFFFF and
/// inverted at the end): the checksum that ends an install code.
pub fn crc16_x25(data: &[u8]) -> u16 {
    let mut crc = 0xFFFFu16;
    for b in data {
        crc ^= *b as u16;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { crc >> 1 ^ 0x8408 } else { crc >> 1 };
        }
    }
    !crc
}

/// The link key an install code stands for: the hash of the code with its
/// checksum. `code` is the code without the checksum.
pub fn install_code_key(code: &[u8]) -> [u8; 16] {
    mmo_hash(&[code, &crc16_x25(code).to_le_bytes()])
}

pub fn hmac_mmo(key: &[u8; 16], msg: &[u8]) -> [u8; 16] {
    let mut ipad = [0u8; 16];
    let mut opad = [0u8; 16];
    for i in 0..16 {
        ipad[i] = key[i] ^ 0x36;
        opad[i] = key[i] ^ 0x5c;
    }
    let inner = mmo_hash(&[&ipad, msg]);
    mmo_hash(&[&opad, &inner])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> std::vec::Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    // Reference values computed with PyCryptodome (emulator/zigbee_coord.py).

    #[test]
    fn install_code_gives_the_key_of_the_published_example() {
        // the example of the Base Device Behavior specification: code, checksum, key
        let code = hex("83FED3407A939723A5C639B26916D505");
        assert_eq!(crc16_x25(&code), 0xB5C3);
        assert_eq!(install_code_key(&code).to_vec(), hex("66B6900981E1EE3CA4206B6B861C02BB"));
    }

    #[test]
    fn mmo_hash_matches_reference() {
        let data: std::vec::Vec<u8> = (0..33).collect();
        assert_eq!(mmo_hash(&[]).to_vec(), hex("bad78e726c1ec02b7ebfe92b23d9ec34"));
        assert_eq!(mmo_hash(&[&data[..16]]).to_vec(), hex("a85c3815c209171c854b4c3fc21af55b"));
        assert_eq!(mmo_hash(&[&data[..7], &data[7..]]).to_vec(), hex("edbdfe762f4008401227646cfb5da404"));
    }

    #[test]
    fn link_key_derivatives_of_the_default_tc_key() {
        let k = *b"ZigBeeAlliance09";
        assert_eq!(hmac_mmo(&k, &[0x00]).to_vec(), hex("4bab0f173e1434a2d572e1c1ef478782"));
        assert_eq!(hmac_mmo(&k, &[0x02]).to_vec(), hex("c5a47035c332ccbf251571d8baded188"));
        assert_eq!(hmac_mmo(&k, &[0x03]).to_vec(), hex("1ab128df1639a1246aaba72a6a559124"));
    }

    fn vector() -> (Aes128, [u8; 13], std::vec::Vec<u8>, std::vec::Vec<u8>) {
        let mut key = [0u8; 16];
        for i in 0..16 {
            key[i] = 0xc0 + i as u8;
        }
        let mut nonce = [0u8; 13];
        for i in 0..13 {
            nonce[i] = i as u8;
        }
        (Aes128::new(&key), nonce, (0x40..0x40 + 23).collect(), (0x80..0x80 + 29).collect())
    }

    #[test]
    fn ccm_star_encrypts_like_the_reference() {
        let (aes, nonce, aad, pl) = vector();
        let mut buf = [0u8; 64];
        buf[..29].copy_from_slice(&pl);
        let n = ccm_encrypt(&aes, &nonce, &aad, &mut buf, 29).unwrap();
        assert_eq!(buf[..n].to_vec(), hex("448299ceedafe06c20123b22fbc2ff8215ff02903275578f0d413740d8bfa8d027"));
        let n = ccm_encrypt(&aes, &nonce, &aad, &mut buf, 0).unwrap();
        assert_eq!(buf[..n].to_vec(), hex("2a2b849f"));
        let mut buf = [0u8; 20];
        let n = ccm_encrypt(&aes, &nonce, &[0u8; 14], &mut buf, 16).unwrap();
        assert_eq!(buf[..n].to_vec(), hex("c4031b4d692a66eba89bb1a9774f710d48e98979"));
    }

    #[test]
    fn ccm_star_round_trip_and_tamper_detection() {
        let (aes, nonce, aad, pl) = vector();
        let mut buf = [0u8; 33];
        buf[..29].copy_from_slice(&pl);
        ccm_encrypt(&aes, &nonce, &aad, &mut buf, 29).unwrap();
        let mut good = buf;
        assert_eq!(ccm_decrypt(&aes, &nonce, &aad, &mut good), Some(29));
        assert_eq!(good[..29].to_vec(), pl);
        let mut bad = buf;
        bad[3] ^= 1;
        assert_eq!(ccm_decrypt(&aes, &nonce, &aad, &mut bad), None);
        let mut bad_aad = aad.clone();
        bad_aad[0] ^= 1;
        let mut c = buf;
        assert_eq!(ccm_decrypt(&aes, &nonce, &bad_aad, &mut c), None);
        assert_eq!(ccm_decrypt(&aes, &nonce, &aad, &mut [0u8; 3]), None);
    }
}
