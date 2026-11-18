static SBOX: [u8; 256] = [
    0x63, 0x7c, 0x77, 0x7b, 0xf2, 0x6b, 0x6f, 0xc5, 0x30, 0x01, 0x67, 0x2b, 0xfe, 0xd7, 0xab, 0x76,
    0xca, 0x82, 0xc9, 0x7d, 0xfa, 0x59, 0x47, 0xf0, 0xad, 0xd4, 0xa2, 0xaf, 0x9c, 0xa4, 0x72, 0xc0,
    0xb7, 0xfd, 0x93, 0x26, 0x36, 0x3f, 0xf7, 0xcc, 0x34, 0xa5, 0xe5, 0xf1, 0x71, 0xd8, 0x31, 0x15,
    0x04, 0xc7, 0x23, 0xc3, 0x18, 0x96, 0x05, 0x9a, 0x07, 0x12, 0x80, 0xe2, 0xeb, 0x27, 0xb2, 0x75,
    0x09, 0x83, 0x2c, 0x1a, 0x1b, 0x6e, 0x5a, 0xa0, 0x52, 0x3b, 0xd6, 0xb3, 0x29, 0xe3, 0x2f, 0x84,
    0x53, 0xd1, 0x00, 0xed, 0x20, 0xfc, 0xb1, 0x5b, 0x6a, 0xcb, 0xbe, 0x39, 0x4a, 0x4c, 0x58, 0xcf,
    0xd0, 0xef, 0xaa, 0xfb, 0x43, 0x4d, 0x33, 0x85, 0x45, 0xf9, 0x02, 0x7f, 0x50, 0x3c, 0x9f, 0xa8,
    0x51, 0xa3, 0x40, 0x8f, 0x92, 0x9d, 0x38, 0xf5, 0xbc, 0xb6, 0xda, 0x21, 0x10, 0xff, 0xf3, 0xd2,
    0xcd, 0x0c, 0x13, 0xec, 0x5f, 0x97, 0x44, 0x17, 0xc4, 0xa7, 0x7e, 0x3d, 0x64, 0x5d, 0x19, 0x73,
    0x60, 0x81, 0x4f, 0xdc, 0x22, 0x2a, 0x90, 0x88, 0x46, 0xee, 0xb8, 0x14, 0xde, 0x5e, 0x0b, 0xdb,
    0xe0, 0x32, 0x3a, 0x0a, 0x49, 0x06, 0x24, 0x5c, 0xc2, 0xd3, 0xac, 0x62, 0x91, 0x95, 0xe4, 0x79,
    0xe7, 0xc8, 0x37, 0x6d, 0x8d, 0xd5, 0x4e, 0xa9, 0x6c, 0x56, 0xf4, 0xea, 0x65, 0x7a, 0xae, 0x08,
    0xba, 0x78, 0x25, 0x2e, 0x1c, 0xa6, 0xb4, 0xc6, 0xe8, 0xdd, 0x74, 0x1f, 0x4b, 0xbd, 0x8b, 0x8a,
    0x70, 0x3e, 0xb5, 0x66, 0x48, 0x03, 0xf6, 0x0e, 0x61, 0x35, 0x57, 0xb9, 0x86, 0xc1, 0x1d, 0x9e,
    0xe1, 0xf8, 0x98, 0x11, 0x69, 0xd9, 0x8e, 0x94, 0x9b, 0x1e, 0x87, 0xe9, 0xce, 0x55, 0x28, 0xdf,
    0x8c, 0xa1, 0x89, 0x0d, 0xbf, 0xe6, 0x42, 0x68, 0x41, 0x99, 0x2d, 0x0f, 0xb0, 0x54, 0xbb, 0x16,
];

const fn gen_inverse(s: &[u8; 256]) -> [u8; 256] {
    let mut inv = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        inv[s[i] as usize] = i as u8;
        i += 1;
    }
    inv
}

static INV_SBOX: [u8; 256] = gen_inverse(&SBOX);

fn gmul(mut a: u8, mut b: u8) -> u8 {
    let mut p = 0;
    while b != 0 {
        if b & 1 != 0 {
            p ^= a;
        }
        a = xtime(a);
        b >>= 1;
    }
    p
}

fn xtime(x: u8) -> u8 {
    (x << 1) ^ (if x & 0x80 != 0 { 0x1b } else { 0 })
}

pub type Engine = fn(&[u8; 16], &mut [u8; 16]) -> bool;

#[derive(Clone)]
pub struct Aes128 {
    rk: [u8; 176],
    engine: Option<Engine>,
}

impl Aes128 {
    pub fn with_engine(key: &[u8; 16], engine: Option<Engine>) -> Self {
        let mut aes = Aes128::new(key);
        aes.engine = engine;
        aes
    }

    pub fn new(key: &[u8; 16]) -> Self {
        let mut rk = [0u8; 176];
        rk[..16].copy_from_slice(key);
        let mut rcon: u8 = 1;
        let mut i = 16;
        while i < 176 {
            let mut t = [rk[i - 4], rk[i - 3], rk[i - 2], rk[i - 1]];
            if i % 16 == 0 {
                t = [
                    SBOX[t[1] as usize] ^ rcon,
                    SBOX[t[2] as usize],
                    SBOX[t[3] as usize],
                    SBOX[t[0] as usize],
                ];
                rcon = xtime(rcon);
            }
            for j in 0..4 {
                rk[i + j] = rk[i + j - 16] ^ t[j];
            }
            i += 4;
        }
        Aes128 { rk, engine: None }
    }

    fn add_round_key(&self, s: &mut [u8; 16], round: usize) {
        for (b, k) in s.iter_mut().zip(&self.rk[round * 16..round * 16 + 16]) {
            *b ^= *k;
        }
    }

    pub fn encrypt(&self, s: &mut [u8; 16]) {
        if let Some(engine) = self.engine {
            let mut key = [0u8; 16];
            key.copy_from_slice(&self.rk[..16]);
            let mut block = *s;
            if engine(&key, &mut block) {
                *s = block;
                return;
            }
        }
        self.add_round_key(s, 0);
        for round in 1..=10 {
            let t = *s;
            for col in 0..4 {
                for row in 0..4 {
                    s[4 * col + row] = SBOX[t[4 * ((col + row) % 4) + row] as usize];
                }
            }
            if round != 10 {
                for col in 0..4 {
                    let c = [s[4 * col], s[4 * col + 1], s[4 * col + 2], s[4 * col + 3]];
                    let all = c[0] ^ c[1] ^ c[2] ^ c[3];
                    for row in 0..4 {
                        s[4 * col + row] = c[row] ^ all ^ xtime(c[row] ^ c[(row + 1) % 4]);
                    }
                }
            }
            self.add_round_key(s, round);
        }
    }

    pub fn decrypt(&self, s: &mut [u8; 16]) {
        self.add_round_key(s, 10);
        for round in (0..10).rev() {
            let t = *s;
            for col in 0..4 {
                for row in 0..4 {
                    s[4 * ((col + row) % 4) + row] = INV_SBOX[t[4 * col + row] as usize];
                }
            }
            self.add_round_key(s, round);
            if round != 0 {
                for col in 0..4 {
                    let c = [s[4 * col], s[4 * col + 1], s[4 * col + 2], s[4 * col + 3]];
                    for row in 0..4 {
                        s[4 * col + row] = gmul(c[row], 14)
                            ^ gmul(c[(row + 1) % 4], 11)
                            ^ gmul(c[(row + 2) % 4], 13)
                            ^ gmul(c[(row + 3) % 4], 9);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicUsize, Ordering};

    static BLOCKS: AtomicUsize = AtomicUsize::new(0);

    fn counting(key: &[u8; 16], block: &mut [u8; 16]) -> bool {
        BLOCKS.fetch_add(1, Ordering::SeqCst);
        Aes128::new(key).encrypt(block);
        true
    }

    fn broken(_: &[u8; 16], block: &mut [u8; 16]) -> bool {
        block.fill(0xee);
        false
    }

    #[test]
    fn a_hardware_engine_does_the_blocks_and_software_covers_for_a_failing_one() {
        let key: [u8; 16] = core::array::from_fn(|i| i as u8);
        let plain: [u8; 16] = core::array::from_fn(|i| (i * 0x11) as u8);
        let mut expected = plain;
        Aes128::new(&key).encrypt(&mut expected);

        let before = BLOCKS.load(Ordering::SeqCst);
        let mut block = plain;
        Aes128::with_engine(&key, Some(counting)).encrypt(&mut block);
        assert_eq!(block, expected);
        assert_eq!(BLOCKS.load(Ordering::SeqCst), before + 1, "the engine was asked");

        let mut block = plain;
        Aes128::with_engine(&key, Some(broken)).encrypt(&mut block);
        assert_eq!(block, expected, "computed in software from the untouched input");

        let aes = Aes128::with_engine(&key, Some(counting));
        aes.decrypt(&mut block);
        assert_eq!(block, plain);
    }

    const fn gen_sbox() -> [u8; 256] {
        let mut s = [0u8; 256];
        let mut p: u8 = 1;
        let mut q: u8 = 1;
        loop {
            p = p ^ (p << 1) ^ (if p & 0x80 != 0 { 0x1b } else { 0 });
            q ^= q << 1;
            q ^= q << 2;
            q ^= q << 4;
            if q & 0x80 != 0 {
                q ^= 0x09;
            }
            let x = q ^ q.rotate_left(1) ^ q.rotate_left(2) ^ q.rotate_left(3) ^ q.rotate_left(4);
            s[p as usize] = x ^ 0x63;
            if p == 1 {
                break;
            }
        }
        s[0] = 0x63;
        s
    }

    #[test]
    fn the_s_box_table_is_the_one_computed_from_the_field_and_the_affine_map_and_that_of_fips_197() {
        let sbox = gen_sbox();
        assert_eq!(sbox, SBOX);
        assert_eq!([sbox[0x00], sbox[0x01], sbox[0x53], sbox[0x8F], sbox[0xC9], sbox[0xFF]], [0x63, 0x7C, 0xED, 0x73, 0xDD, 0x16]);
        let inv = gen_inverse(&sbox);
        assert_eq!(inv, INV_SBOX);
        assert_eq!([inv[0x63], inv[0x7C], inv[0xED], inv[0x16], inv[0x00]], [0x00, 0x01, 0x53, 0xFF, 0x52]);
    }

    #[test]
    fn fips_197_appendix_c1() {
        let mut key = [0u8; 16];
        let mut block = [0u8; 16];
        for i in 0..16 {
            key[i] = i as u8;
            block[i] = (i as u8) * 0x11;
        }
        Aes128::new(&key).encrypt(&mut block);
        assert_eq!(
            block,
            [0x69, 0xc4, 0xe0, 0xd8, 0x6a, 0x7b, 0x04, 0x30, 0xd8, 0xcd, 0xb7, 0x80, 0x70, 0xb4, 0xc5, 0x5a]
        );
        Aes128::new(&key).decrypt(&mut block);
        for i in 0..16 {
            assert_eq!(block[i], (i as u8) * 0x11);
        }
    }
}
