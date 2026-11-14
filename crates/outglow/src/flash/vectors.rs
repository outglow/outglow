//! `outglow ble vectors [image]`: reference values for the browser flasher's
//! tests (site/test/awox-ota.test.ts), as JSON on stdout.
//!
//! Without an image a deterministic synthetic one is built: 5000 bytes from
//! Python's `random.Random(1).getrandbits(8)` (the generator the earlier
//! Python reference script used, so the output is the same byte for byte),
//! `KNLT` at 0x08, an Outglow board tag at 0x100, finalized with the Telink
//! header and CRC.

use super::protocol::{crc16_modbus, data_packet, end_packet, erase_packet, hex, login_packet, packet_count, secure_password};
use outglow_image::telink;

/// MT19937 seeded the way CPython's `random.seed(n)` seeds it for a small
/// non-negative integer (`init_by_array([n])`).
struct Mt {
    s: [u32; 624],
    i: usize,
}

impl Mt {
    fn new(seed: u32) -> Mt {
        let mut s = [0u32; 624];
        s[0] = 19_650_218;
        for k in 1..624 {
            s[k] = 1_812_433_253u32.wrapping_mul(s[k - 1] ^ (s[k - 1] >> 30)).wrapping_add(k as u32);
        }
        let key = [seed];
        let (mut i, mut j) = (1usize, 0usize);
        for _ in 0..624.max(key.len()) {
            s[i] = (s[i] ^ (s[i - 1] ^ (s[i - 1] >> 30)).wrapping_mul(1_664_525)).wrapping_add(key[j]).wrapping_add(j as u32);
            i += 1;
            j += 1;
            if i >= 624 {
                s[0] = s[623];
                i = 1;
            }
            if j >= key.len() {
                j = 0;
            }
        }
        for _ in 0..623 {
            s[i] = (s[i] ^ (s[i - 1] ^ (s[i - 1] >> 30)).wrapping_mul(1_566_083_941)).wrapping_sub(i as u32);
            i += 1;
            if i >= 624 {
                s[0] = s[623];
                i = 1;
            }
        }
        s[0] = 0x8000_0000;
        Mt { s, i: 624 }
    }

    fn next_u32(&mut self) -> u32 {
        if self.i >= 624 {
            for k in 0..624 {
                let y = (self.s[k] & 0x8000_0000) | (self.s[(k + 1) % 624] & 0x7FFF_FFFF);
                self.s[k] = self.s[(k + 397) % 624] ^ (y >> 1) ^ if y & 1 != 0 { 0x9908_B0DF } else { 0 };
            }
            self.i = 0;
        }
        let mut y = self.s[self.i];
        self.i += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9D2C_5680;
        y ^= (y << 15) & 0xEFC6_0000;
        y ^ (y >> 18)
    }

    /// `getrandbits(8)`
    fn byte(&mut self) -> u8 {
        (self.next_u32() >> 24) as u8
    }
}

pub fn synthetic() -> Vec<u8> {
    let mut mt = Mt::new(1);
    let mut body: Vec<u8> = (0..5000).map(|_| mt.byte()).collect();
    body[8..12].copy_from_slice(b"KNLT");
    let tag = b"open-eglo-board:ESMLFzm_w6_TW-4.63\0";
    body[0x100..0x100 + tag.len()].copy_from_slice(tag);
    telink::finalize(&body)
}

fn base64(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for k in 0..4 {
            if k <= c.len() {
                out.push(A[(n >> (18 - 6 * k)) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn packets(img: &[u8], idx: &[usize], d: usize) -> String {
    let items: Vec<String> = idx.iter().map(|i| format!("\"{i}\": \"{}\"", hex(&data_packet(img, *i, d)))).collect();
    format!("{{{}}}", items.join(", "))
}

/// The JSON, formatted as Python's `json.dump` does (", " and ": ").
pub fn json(img: &[u8]) -> String {
    let n = packet_count(img.len(), 16);
    let mut idx = vec![0, 1, 2, n / 2, n.saturating_sub(2), n.saturating_sub(1)];
    idx.sort();
    idx.dedup();
    let mac6 = [0xa4, 0xc1, 0x38, 0x12, 0x34, 0x56];
    let rnd8 = [1, 2, 3, 4, 5, 6, 7, 8];
    let pw = secure_password(&mac6);
    let n40 = packet_count(img.len(), 40);
    let mut idx40 = vec![0, n40.saturating_sub(1)];
    idx40.dedup();
    format!(
        concat!(
            "{{\"image\": \"{}\", \"verify\": {}, \"count\": {}, \"erase\": \"{}\", \"data\": {}, \"end\": \"{}\", ",
            "\"crc16\": {}, \"secure_password\": \"{}\", \"login\": \"{}\", \"login_1234\": \"{}\", ",
            "\"chunk40\": {{\"erase\": \"{}\", \"data\": {}, \"end\": \"{}\"}}}}"
        ),
        base64(img),
        telink::verify(img),
        n,
        hex(&erase_packet(img.len(), 16)),
        packets(img, &idx, 16),
        hex(&end_packet(img.len(), 16)),
        crc16_modbus(b"123456789"),
        hex(&pw),
        hex(&login_packet(b"unpaired", &pw, &rnd8)),
        hex(&login_packet(b"telink_mesh1", b"123", &rnd8)),
        hex(&erase_packet(img.len(), 40)),
        packets(img, &idx40, 40),
        hex(&end_packet(img.len(), 40)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[test]
    fn python_random_1() {
        // random.Random(1): getrandbits(32) x3, then the synthetic image's digest
        let mut mt = Mt::new(1);
        assert_eq!([mt.next_u32(), mt.next_u32(), mt.next_u32()], PY_U32);
        assert_eq!(hex(&Sha256::digest(json(&synthetic()).as_bytes())), PY_JSON_SHA256);
    }

    #[test]
    fn base64_padding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
    }

    // from Python 3.11: random.Random(1).getrandbits(32) three times, and the
    // SHA-256 of what the earlier Python reference printed
    const PY_U32: [u32; 3] = [0x2265_b1f5, 0x91b7_584a, 0xd8f1_6adf];
    const PY_JSON_SHA256: &str = "80155ca212f74337ea2d47db1b016b466e21fe2f5593f7d55ebcff4cc11314ec";
}
