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

static SBOX: [u8; 256] = gen_sbox();

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
