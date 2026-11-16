use std::sync::OnceLock;

pub(crate) struct Tables {
    pub(crate) sbox: [u8; 256],
    pub(crate) inv: [u8; 256],
}

pub(crate) fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let mut sbox = [0u8; 256];
        let mut inv = [0u8; 256];
        let (mut p, mut q): (i32, i32) = (1, 1);
        loop {
            if p & 0x80 != 0 {
                p = p ^ ((p << 1) & 0xFF) ^ 0x1B;
            } else {
                p = p ^ ((p << 1) & 0xFF);
            }
            q ^= q << 1;
            q ^= q << 2;
            q ^= q << 4;
            q &= 0xFF;
            if q & 0x80 != 0 {
                q ^= 0x09;
            }
            let mut x = q ^ ((q << 1) | (q >> 7)) ^ ((q << 2) | (q >> 6)) ^ ((q << 3) | (q >> 5)) ^ ((q << 4) | (q >> 4));
            x = (x ^ 0x63) & 0xFF;
            sbox[p as usize] = x as u8;
            inv[x as usize] = p as u8;
            if p == 1 {
                break;
            }
        }
        sbox[0] = 0x63;
        inv[0x63] = 0;
        Tables { sbox, inv }
    })
}

fn xt(a: u8) -> u8 {
    if a & 0x80 != 0 {
        (a << 1) ^ 0x1B
    } else {
        a << 1
    }
}

fn mul(mut a: u8, mut b: u8) -> u8 {
    let mut r = 0u8;
    while b != 0 {
        if b & 1 != 0 {
            r ^= a;
        }
        a = xt(a);
        b >>= 1;
    }
    r
}

fn expand(key: &[u8]) -> [[u8; 16]; 11] {
    let sbox = &tables().sbox;
    let mut w = [[0u8; 4]; 44];
    for i in 0..4 {
        w[i].copy_from_slice(&key[4 * i..4 * i + 4]);
    }
    let mut rcon = 1u8;
    for i in 4..44 {
        let mut t = w[i - 1];
        if i % 4 == 0 {
            t = [sbox[t[1] as usize], sbox[t[2] as usize], sbox[t[3] as usize], sbox[t[0] as usize]];
            t[0] ^= rcon;
            rcon = xt(rcon);
        }
        for j in 0..4 {
            w[i][j] = w[i - 4][j] ^ t[j];
        }
    }
    let mut rk = [[0u8; 16]; 11];
    for r in 0..11 {
        for c in 0..4 {
            rk[r][4 * c..4 * c + 4].copy_from_slice(&w[4 * r + c]);
        }
    }
    rk
}

pub fn encrypt(key: &[u8], block: &[u8]) -> [u8; 16] {
    let sbox = &tables().sbox;
    let rk = expand(key);
    let mut s = [0u8; 16];
    for i in 0..16 {
        s[i] = block[i] ^ rk[0][i];
    }
    for rnd in 1..=10 {
        for x in s.iter_mut() {
            *x = sbox[*x as usize];
        }
        let mut t = [0u8; 16];
        for i in 0..16 {
            t[i] = s[(i + 4 * (i % 4)) % 16];
        }
        s = t;
        if rnd != 10 {
            let mut n = [0u8; 16];
            for c in 0..4 {
                let a = &s[4 * c..4 * c + 4];
                n[4 * c] = mul(a[0], 2) ^ mul(a[1], 3) ^ a[2] ^ a[3];
                n[4 * c + 1] = a[0] ^ mul(a[1], 2) ^ mul(a[2], 3) ^ a[3];
                n[4 * c + 2] = a[0] ^ a[1] ^ mul(a[2], 2) ^ mul(a[3], 3);
                n[4 * c + 3] = mul(a[0], 3) ^ a[1] ^ a[2] ^ mul(a[3], 2);
            }
            s = n;
        }
        for i in 0..16 {
            s[i] ^= rk[rnd][i];
        }
    }
    s
}

pub fn decrypt(key: &[u8], block: &[u8]) -> [u8; 16] {
    let inv = &tables().inv;
    let rk = expand(key);
    let mut s = [0u8; 16];
    for i in 0..16 {
        s[i] = block[i] ^ rk[10][i];
    }
    for rnd in (0..=9).rev() {
        let mut t = [0u8; 16];
        for i in 0..16i64 {
            t[i as usize] = s[((((i - 4 * (i % 4)) % 16) + 16) % 16) as usize];
        }
        s = t;
        for x in s.iter_mut() {
            *x = inv[*x as usize];
        }
        for i in 0..16 {
            s[i] ^= rk[rnd][i];
        }
        if rnd != 0 {
            let mut n = [0u8; 16];
            for c in 0..4 {
                let a = &s[4 * c..4 * c + 4];
                n[4 * c] = mul(a[0], 14) ^ mul(a[1], 11) ^ mul(a[2], 13) ^ mul(a[3], 9);
                n[4 * c + 1] = mul(a[0], 9) ^ mul(a[1], 14) ^ mul(a[2], 11) ^ mul(a[3], 13);
                n[4 * c + 2] = mul(a[0], 13) ^ mul(a[1], 9) ^ mul(a[2], 14) ^ mul(a[3], 11);
                n[4 * c + 3] = mul(a[0], 11) ^ mul(a[1], 13) ^ mul(a[2], 9) ^ mul(a[3], 14);
            }
            s = n;
        }
    }
    s
}

pub fn big128(b: &[u8]) -> u128 {
    b.iter().fold(0u128, |v, &x| (v << 8) | x as u128)
}

pub fn bytes128(x: u128) -> [u8; 16] {
    x.to_be_bytes()
}

pub fn bytes_le(x: u128, n: usize) -> Vec<u8> {
    x.to_le_bytes()[..n].to_vec()
}

pub fn from_le(b: &[u8]) -> u128 {
    b.iter().rev().fold(0u128, |v, &x| (v << 8) | x as u128)
}

pub fn e(k: u128, p: u128) -> u128 {
    big128(&encrypt(&bytes128(k), &bytes128(p)))
}

pub fn c1(k: u128, r: u128, preq: &[u8], pres: &[u8], iat: u32, ia: &[u8], rat: u32, ra: &[u8]) -> u128 {
    let p1 = (from_le(pres) << 72) | (from_le(preq) << 16) | ((rat as u128) << 8 | iat as u128);
    let p2 = (from_le(ia) << 48) | from_le(ra);
    let x = e(k, r ^ p1);
    e(k, x ^ p2)
}

pub fn s1(k: u128, r1: u128, r2: u128) -> u128 {
    let mask = (1u128 << 64) - 1;
    e(k, ((r1 & mask) << 64) | (r2 & mask))
}

pub fn ah(k: u128, r: u32) -> u32 {
    (e(k, (r & 0xFFFFFF) as u128) & 0xFFFFFF) as u32
}

pub fn rpa(irk: &[u8], prand: u32) -> [u8; 6] {
    let prand = (prand & 0x3FFFFF) | 0x400000;
    let h = ah(from_le(irk), prand);
    [h as u8, (h >> 8) as u8, (h >> 16) as u8, prand as u8, (prand >> 8) as u8, (prand >> 16) as u8]
}

fn len_bytes(mut n: usize, lf: usize) -> Vec<u8> {
    let mut b = vec![0u8; lf];
    for i in (0..lf).rev() {
        b[i] = n as u8;
        n >>= 8;
    }
    b
}

pub fn ccm(key: &[u8], nonce: &[u8], aad: &[u8], data: &[u8], mic_len: usize) -> Vec<u8> {
    let n = data.len();
    let lf = 15 - nonce.len();
    let mut flags = ((((mic_len - 2) / 2) << 3) | (lf - 1)) as u8;
    if !aad.is_empty() {
        flags |= 0x40;
    }
    let mut b0 = vec![flags];
    b0.extend_from_slice(nonce);
    b0.extend(len_bytes(n, lf));
    let mut x = encrypt(key, &b0);
    let mut blocks = Vec::new();
    if !aad.is_empty() {
        let mut a = vec![(aad.len() >> 8) as u8, aad.len() as u8];
        a.extend_from_slice(aad);
        let pad = (16 - a.len() % 16) % 16;
        blocks.extend(a);
        blocks.extend(std::iter::repeat(0u8).take(pad));
    }
    blocks.extend_from_slice(data);
    blocks.extend(std::iter::repeat(0u8).take((16 - n % 16) % 16));
    let mut i = 0;
    while i < blocks.len() {
        let mut t = [0u8; 16];
        for j in 0..16 {
            t[j] = x[j] ^ blocks[i + j];
        }
        x = encrypt(key, &t);
        i += 16;
    }
    let ctr = |i: usize| {
        let mut b = vec![(lf - 1) as u8];
        b.extend_from_slice(nonce);
        b.extend(len_bytes(i, lf));
        encrypt(key, &b)
    };
    let mut out = Vec::with_capacity(n + mic_len);
    let mut i = 0;
    while i < n {
        let s = ctr(i / 16 + 1);
        let end = (i + 16).min(n);
        for j in i..end {
            out.push(data[j] ^ s[j - i]);
        }
        i += 16;
    }
    let s0 = ctr(0);
    for i in 0..mic_len {
        out.push(x[i] ^ s0[i]);
    }
    out
}

pub fn ccm_encrypt(sk: &[u8], nonce: &[u8], hdr0: u8, payload: &[u8]) -> Vec<u8> {
    ccm(sk, nonce, &[hdr0 & 0xE3], payload, 4)
}

pub fn ccm_decrypt(sk: &[u8], nonce: &[u8], hdr0: u8, data: &[u8]) -> (Vec<u8>, bool) {
    if data.len() < 4 {
        return (Vec::new(), false);
    }
    let n = data.len() - 4;
    let lf = 15 - nonce.len();
    let mut out = Vec::with_capacity(n);
    let mut i = 0;
    while i < n {
        let mut b = vec![(lf - 1) as u8];
        b.extend_from_slice(nonce);
        b.extend(len_bytes(i / 16 + 1, lf));
        let s = encrypt(sk, &b);
        let end = (i + 16).min(n);
        for j in i..end {
            out.push(data[j] ^ s[j - i]);
        }
        i += 16;
    }
    let mic = ccm_encrypt(sk, nonce, hdr0, &out);
    let ok = mic[n..] == data[n..];
    (out, ok)
}
