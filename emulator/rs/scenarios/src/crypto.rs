//! Zigbee security primitives for the scenario coordinator and the sniffer:
//! AES-CCM* at security level 5 (MIC-32), the Matyas-Meyer-Oseas hash and
//! HMAC-MMO (Zigbee R22 4.5 and B.6).
//!
//! Deliberately independent of `crates/zigbee`: the AES block cipher is the
//! one of tc32emu (a port of tc32-devtools go/ble/aes128), so a mutation of
//! the firmware's crypto can never change the coordinator's crypto too.

use tc32emu::aes128;

/// MIC length of security level 5.
pub const MIC_LEN: usize = 4;

/// CCM* encryption: ciphertext || MIC-32.
pub fn ccm_encrypt(key: &[u8; 16], nonce: &[u8; 13], aad: &[u8], payload: &[u8]) -> Vec<u8> {
    aes128::ccm(key, nonce, aad, payload, MIC_LEN)
}

/// CCM* decryption of `body` (ciphertext || MIC-32); None when the MIC is wrong.
pub fn ccm_decrypt(key: &[u8; 16], nonce: &[u8; 13], aad: &[u8], body: &[u8]) -> Option<Vec<u8>> {
    if body.len() < MIC_LEN {
        return None;
    }
    let (ct, mic) = body.split_at(body.len() - MIC_LEN);
    // CTR with the same counter blocks as the encryption: A_i = 01 || nonce || i (L = 2)
    let mut plain = Vec::with_capacity(ct.len());
    for (k, chunk) in ct.chunks(16).enumerate() {
        let s = aes128::encrypt(key, &ctr_block(nonce, k + 1));
        plain.extend(chunk.iter().zip(s.iter()).map(|(c, s)| c ^ s));
    }
    // recompute the whole thing and compare: the MIC is over the plaintext
    let again = aes128::ccm(key, nonce, aad, &plain, MIC_LEN);
    if again[..ct.len()] == *ct && again[ct.len()..] == *mic {
        Some(plain)
    } else {
        None
    }
}

fn ctr_block(nonce: &[u8; 13], i: usize) -> [u8; 16] {
    let mut b = [0u8; 16];
    b[0] = 0x01; // L - 1 with L = 2
    b[1..14].copy_from_slice(nonce);
    b[14] = (i >> 8) as u8;
    b[15] = i as u8;
    b
}

/// Matyas-Meyer-Oseas hash on AES-128 (Zigbee B.6), for messages shorter than 2^16 bits.
pub fn mmo_hash(data: &[u8]) -> [u8; 16] {
    let mut m = data.to_vec();
    m.push(0x80);
    while (m.len() + 2) % 16 != 0 {
        m.push(0);
    }
    let bits = (data.len() * 8) as u16;
    m.extend_from_slice(&bits.to_be_bytes());
    let mut h = [0u8; 16];
    for blk in m.chunks(16) {
        let e = aes128::encrypt(&h, blk);
        for i in 0..16 {
            h[i] = e[i] ^ blk[i];
        }
    }
    h
}

/// HMAC-MMO (Zigbee B.1.4) with a 16-byte key.
pub fn hmac_mmo(key: &[u8; 16], msg: &[u8]) -> [u8; 16] {
    let mut inner: Vec<u8> = key.iter().map(|k| k ^ 0x36).collect();
    inner.extend_from_slice(msg);
    let ih = mmo_hash(&inner);
    let mut outer: Vec<u8> = key.iter().map(|k| k ^ 0x5C).collect();
    outer.extend_from_slice(&ih);
    mmo_hash(&outer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    const TCLK: [u8; 16] = *b"ZigBeeAlliance09";

    // vectors from emulator/zigbee_coord.py (pycryptodome)
    #[test]
    fn key_transport_and_key_load_keys() {
        assert_eq!(hmac_mmo(&TCLK, &[0x00]).to_vec(), h("4bab0f173e1434a2d572e1c1ef478782"));
        assert_eq!(hmac_mmo(&TCLK, &[0x02]).to_vec(), h("c5a47035c332ccbf251571d8baded188"));
    }

    #[test]
    fn mmo() {
        assert_eq!(mmo_hash(b"").to_vec(), h("bad78e726c1ec02b7ebfe92b23d9ec34"));
        assert_eq!(mmo_hash(b"abc").to_vec(), h("bd2f2ebd93fadc48bc00174d95422741"));
        let d: Vec<u8> = (0..33).collect();
        assert_eq!(mmo_hash(&d).to_vec(), h("edbdfe762f4008401227646cfb5da404"));
    }

    #[test]
    fn ccm_star_round_trip() {
        let key: [u8; 16] = core::array::from_fn(|i| i as u8 + 1);
        let nonce: [u8; 13] = core::array::from_fn(|i| 0x20 + i as u8);
        let aad = h("0802fcff00001e5a28");
        let pl = h("0800360000008b123f01");
        let ct = ccm_encrypt(&key, &nonce, &aad, &pl);
        assert_eq!(ct, h("da4c7e08608e70b94c630529621e"));
        assert_eq!(ccm_decrypt(&key, &nonce, &aad, &ct), Some(pl));
        assert_eq!(ccm_encrypt(&key, &nonce, &aad, &[]), h("ad7d4dea"));
        let mut bad = ct.clone();
        bad[0] ^= 1;
        assert_eq!(ccm_decrypt(&key, &nonce, &aad, &bad), None);
    }
}

/// SHA-256 (FIPS 180-4), for the AwoX "unpaired" mesh password of the BLE tests.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98,
        0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
        0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8,
        0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
        0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819,
        0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a,
        0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] =
        [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let mut m = data.to_vec();
    m.push(0x80);
    while m.len() % 64 != 56 {
        m.push(0);
    }
    m.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for blk in m.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(blk[4 * i..4 * i + 4].try_into().unwrap());
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7].wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v = [t1.wrapping_add(t2), v[0], v[1], v[2], v[3].wrapping_add(t1), v[4], v[5], v[6]];
        }
        for i in 0..8 {
            h[i] = h[i].wrapping_add(v[i]);
        }
    }
    let mut out = [0u8; 32];
    for i in 0..8 {
        out[4 * i..4 * i + 4].copy_from_slice(&h[i].to_be_bytes());
    }
    out
}

/// Telink's aes_att_encryption: AES-128 on the reversed bytes of key and data.
pub fn telink_aes(key: &[u8; 16], data: &[u8; 16]) -> [u8; 16] {
    let mut k = *key;
    k.reverse();
    let mut d = *data;
    d.reverse();
    let mut o = aes128::encrypt(&k, &d);
    o.reverse();
    o
}

#[cfg(test)]
mod sha_tests {
    use super::*;
    #[test]
    fn sha256_vectors() {
        let hx = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        assert_eq!(hx(&sha256(b"")), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(hx(&sha256(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let long = vec![b'a'; 1000];
        assert_eq!(hx(&sha256(&long)), "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3");
    }
}
