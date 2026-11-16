use tc32emu::bigint::BigUint;

#[derive(Clone)]
pub struct PyRandom {
    mt: [u32; 624],
    idx: usize,
}

impl PyRandom {
    pub fn new(seed: u32) -> PyRandom {
        let mut r = PyRandom { mt: [0; 624], idx: 624 };
        r.init_genrand(19650218);
        let key = [seed];
        let (mut i, mut j) = (1usize, 0usize);
        let n = 624.max(key.len());
        for _ in (1..=n).rev() {
            r.mt[i] = (r.mt[i] ^ ((r.mt[i - 1] ^ (r.mt[i - 1] >> 30)).wrapping_mul(1664525)))
                .wrapping_add(key[j])
                .wrapping_add(j as u32);
            i += 1;
            j += 1;
            if i >= 624 {
                r.mt[0] = r.mt[623];
                i = 1;
            }
            if j >= key.len() {
                j = 0;
            }
        }
        for _ in (1..=623).rev() {
            r.mt[i] = (r.mt[i] ^ ((r.mt[i - 1] ^ (r.mt[i - 1] >> 30)).wrapping_mul(1566083941))).wrapping_sub(i as u32);
            i += 1;
            if i >= 624 {
                r.mt[0] = r.mt[623];
                i = 1;
            }
        }
        r.mt[0] = 0x80000000;
        r.idx = 624;
        r
    }

    fn init_genrand(&mut self, s: u32) {
        self.mt[0] = s;
        for i in 1..624 {
            self.mt[i] = 1812433253u32.wrapping_mul(self.mt[i - 1] ^ (self.mt[i - 1] >> 30)).wrapping_add(i as u32);
        }
        self.idx = 624;
    }

    pub fn genrand(&mut self) -> u32 {
        if self.idx >= 624 {
            for i in 0..624 {
                let y = (self.mt[i] & 0x80000000) | (self.mt[(i + 1) % 624] & 0x7fffffff);
                let mut v = self.mt[(i + 397) % 624] ^ (y >> 1);
                if y & 1 != 0 {
                    v ^= 0x9908b0df;
                }
                self.mt[i] = v;
            }
            self.idx = 0;
        }
        let mut y = self.mt[self.idx];
        self.idx += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c5680;
        y ^= (y << 15) & 0xefc60000;
        y ^= y >> 18;
        y
    }

    pub fn getrandbits(&mut self, k: u32) -> u32 {
        let g = self.genrand();
        let s = 32 - k;
        if s >= 32 {
            0
        } else {
            g >> s
        }
    }

    pub fn randrange(&mut self, n: u32) -> u32 {
        let mut k = 0u32;
        let mut x = n;
        while x > 0 {
            k += 1;
            x >>= 1;
        }
        let mut v = self.getrandbits(k);
        while v >= n {
            v = self.getrandbits(k);
        }
        v
    }

    pub fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.randrange(256) as u8).collect()
    }

    pub fn getrandbits_big(&mut self, k: u32) -> BigUint {
        let words = (k - 1) / 32 + 1;
        let mut w = Vec::with_capacity(words as usize);
        for i in 0..words {
            let mut x = self.genrand();
            if k - 32 * i < 32 {
                x >>= 32 - (k - 32 * i);
            }
            w.push(x);
        }
        BigUint::from_words(&w)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn py_random_matches_python() {
        let mut r = PyRandom::new(1);
        let want32 = [577090037u32, 2444712010, 3639700191, 3445702192, 3280387012, 271041745, 1095513148, 506456969];
        for w in want32 {
            assert_eq!(r.getrandbits(32), w);
        }
        let mut r = PyRandom::new(1);
        let want = [68u32, 32, 130, 60, 253, 230, 241, 194, 107, 48, 249, 14, 199, 221, 1, 228, 136, 117, 52, 162, 15, 11, 13, 4];
        for w in want {
            assert_eq!(r.randrange(256), w);
        }
        let mut r = PyRandom::new(1);
        let hex: String = r.bytes(16).iter().map(|b| format!("{:02x}", b)).collect();
        assert_eq!(hex, "4420823cfde6f1c26b30f90ec7dd01e4");
    }
}
