// SPDX-License-Identifier: Apache-2.0
//! The LE Secure Connections functions (sc.go): P-256, AES-CMAC, f4, f5, f6,
//! g2, the values in on-air octet order (least significant first).
//!
//! Go uses crypto/elliptic; here P-256 is plain Jacobian double-and-add on
//! tc32emu's BigUint. The results are the curve's, so they are the same.

use std::cmp::Ordering;
use std::sync::OnceLock;

use tc32emu::aes128;
use tc32emu::bigint::BigUint;

const SC_SALT: [u8; 16] = [0x6c, 0x88, 0x83, 0x91, 0xaa, 0xf5, 0xa5, 0x38, 0x60, 0x37, 0x0b, 0xdb, 0x5a, 0x60, 0x83, 0xbe];

struct Curve {
    p: BigUint,
    n: BigUint,
    b: BigUint,
    gx: BigUint,
    gy: BigUint,
}

fn curve() -> &'static Curve {
    static C: OnceLock<Curve> = OnceLock::new();
    C.get_or_init(|| Curve {
        p: BigUint::from_hex("ffffffff00000001000000000000000000000000ffffffffffffffffffffffff").unwrap(),
        n: BigUint::from_hex("ffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632551").unwrap(),
        b: BigUint::from_hex("5ac635d8aa3a93e7b3ebbd55769886bc651d06b0cc53b0f63bce3c3e27d2604b").unwrap(),
        gx: BigUint::from_hex("6b17d1f2e12c4247f8bce6e563a440f277037d812deb33a0f4a13945d898c296").unwrap(),
        gy: BigUint::from_hex("4fe342e2fe1a7f9b8ee7eb4a7c0f9e162bce33576b315ececbb6406837bf51f5").unwrap(),
    })
}

/// The order of the P-256 group (P256Order).
pub fn p256_order() -> BigUint {
    curve().n.clone()
}

fn mmul(a: &BigUint, b: &BigUint) -> BigUint {
    a.mul(b).rem(&curve().p)
}
fn madd(a: &BigUint, b: &BigUint) -> BigUint {
    a.add(b).rem(&curve().p)
}
fn msub(a: &BigUint, b: &BigUint) -> BigUint {
    BigUint::mod_sub(a, b, &curve().p)
}
fn small(v: u64) -> BigUint {
    BigUint::from_u64(v)
}

#[derive(Clone)]
struct Jac {
    x: BigUint,
    y: BigUint,
    z: BigUint,
}

fn jdouble(q: &Jac) -> Jac {
    if q.z.is_zero() || q.y.is_zero() {
        return Jac { x: BigUint::zero(), y: BigUint::from_u64(1), z: BigUint::zero() };
    }
    // a = -3: M = 3(X - Z^2)(X + Z^2)
    let zz = mmul(&q.z, &q.z);
    let m = mmul(&small(3), &mmul(&msub(&q.x, &zz), &madd(&q.x, &zz)));
    let yy = mmul(&q.y, &q.y);
    let s = mmul(&small(4), &mmul(&q.x, &yy));
    let x3 = msub(&mmul(&m, &m), &mmul(&small(2), &s));
    let y3 = msub(&mmul(&m, &msub(&s, &x3)), &mmul(&small(8), &mmul(&yy, &yy)));
    let z3 = mmul(&small(2), &mmul(&q.y, &q.z));
    Jac { x: x3, y: y3, z: z3 }
}

fn jadd(p: &Jac, q: &Jac) -> Jac {
    if p.z.is_zero() {
        return q.clone();
    }
    if q.z.is_zero() {
        return p.clone();
    }
    let z1z1 = mmul(&p.z, &p.z);
    let z2z2 = mmul(&q.z, &q.z);
    let u1 = mmul(&p.x, &z2z2);
    let u2 = mmul(&q.x, &z1z1);
    let s1 = mmul(&p.y, &mmul(&q.z, &z2z2));
    let s2 = mmul(&q.y, &mmul(&p.z, &z1z1));
    if u1 == u2 {
        if s1 == s2 {
            return jdouble(p);
        }
        return Jac { x: BigUint::zero(), y: BigUint::from_u64(1), z: BigUint::zero() };
    }
    let h = msub(&u2, &u1);
    let r = msub(&s2, &s1);
    let hh = mmul(&h, &h);
    let hhh = mmul(&h, &hh);
    let v = mmul(&u1, &hh);
    let x3 = msub(&msub(&mmul(&r, &r), &hhh), &mmul(&small(2), &v));
    let y3 = msub(&mmul(&r, &msub(&v, &x3)), &mmul(&s1, &hhh));
    let z3 = mmul(&h, &mmul(&p.z, &q.z));
    Jac { x: x3, y: y3, z: z3 }
}

/// k * (x, y) in affine coordinates; (0, 0) for the point at infinity (as Go).
fn scalar_mult(x: &BigUint, y: &BigUint, k: &BigUint) -> (BigUint, BigUint) {
    let base = Jac { x: x.clone(), y: y.clone(), z: BigUint::from_u64(1) };
    let mut acc = Jac { x: BigUint::zero(), y: BigUint::from_u64(1), z: BigUint::zero() };
    for i in (0..k.bit_len()).rev() {
        acc = jdouble(&acc);
        if k.bit(i) != 0 {
            acc = jadd(&acc, &base);
        }
    }
    if acc.z.is_zero() {
        return (BigUint::zero(), BigUint::zero());
    }
    let p = &curve().p;
    let zi = BigUint::mod_inverse(&acc.z, p).unwrap();
    let zi2 = mmul(&zi, &zi);
    (mmul(&acc.x, &zi2), mmul(&acc.y, &mmul(&zi2, &zi)))
}

fn is_on_curve(x: &BigUint, y: &BigUint) -> bool {
    let c = curve();
    if x.cmp(&c.p) != Ordering::Less || y.cmp(&c.p) != Ordering::Less {
        return false;
    }
    let y2 = mmul(y, y);
    let x3 = mmul(&mmul(x, x), x);
    let rhs = madd(&msub(&x3, &mmul(&small(3), x)), &c.b);
    y2 == rhs
}

pub fn reversed(b: &[u8]) -> Vec<u8> {
    b.iter().rev().copied().collect()
}

/// An integer from its on-air octets (least significant first).
pub fn le_to_big(b: &[u8]) -> BigUint {
    BigUint::from_bytes_be(&reversed(b))
}

/// n on-air octets of x (FillBytes: panics when it does not fit).
pub fn big_to_le(x: &BigUint, n: usize) -> Vec<u8> {
    let words = x.to_words((n + 3) / 4);
    let mut le = Vec::with_capacity(n);
    for w in words {
        le.extend_from_slice(&w.to_le_bytes());
    }
    if le[n..].iter().any(|&b| b != 0) {
        panic!("math/big: buffer too small to fit value");
    }
    le.truncate(n);
    le
}

/// priv * G, x then y, 64 octets.
pub fn public_key(private: &BigUint) -> Vec<u8> {
    let c = curve();
    let (x, y) = scalar_mult(&c.gx, &c.gy, private);
    let mut out = big_to_le(&x, 32);
    out.extend(big_to_le(&y, 32));
    out
}

/// Whether a public key in PDU form is a point of the curve.
pub fn on_curve(pk: &[u8]) -> bool {
    if pk.len() != 64 {
        return false;
    }
    is_on_curve(&le_to_big(&pk[..32]), &le_to_big(&pk[32..]))
}

/// The x coordinate of priv * pk, 32 on-air octets.
pub fn dhkey(private: &BigUint, pk: &[u8]) -> Vec<u8> {
    let (x, y) = (le_to_big(&pk[..32]), le_to_big(&pk[32..64]));
    if !is_on_curve(&x, &y) {
        panic!("crypto/elliptic: ScalarMult was called on an invalid point");
    }
    let (rx, _) = scalar_mult(&x, &y, private);
    big_to_le(&rx, 32)
}

fn cmac_shift(k: &[u8]) -> [u8; 16] {
    let mut out = [0u8; 16];
    let mut carry = 0u8;
    for i in (0..16).rev() {
        out[i] = k[i] << 1 | carry;
        carry = k[i] >> 7;
    }
    if carry != 0 {
        out[15] ^= 0x87;
    }
    out
}

/// RFC 4493: the key and the message as written, the MAC most significant octet first.
pub fn aes_cmac(key: &[u8], m: &[u8]) -> Vec<u8> {
    let k1 = cmac_shift(&aes128::encrypt(key, &[0u8; 16]));
    let k2 = cmac_shift(&k1);
    let blocks = ((m.len() + 15) / 16).max(1);
    let whole = !m.is_empty() && m.len() % 16 == 0;
    let mut last = [0u8; 16];
    let tail = &m[(blocks - 1) * 16..];
    last[..tail.len()].copy_from_slice(tail);
    let mut sub = k1;
    if !whole {
        last[m.len() - (blocks - 1) * 16] = 0x80;
        sub = k2;
    }
    let mut x = [0u8; 16];
    for i in 0..blocks - 1 {
        for j in 0..16 {
            x[j] ^= m[i * 16 + j];
        }
        x = aes128::encrypt(key, &x);
    }
    for j in 0..16 {
        x[j] ^= last[j] ^ sub[j];
    }
    aes128::encrypt(key, &x).to_vec()
}

pub fn f4(u: &[u8], v: &[u8], x: &[u8], z: u8) -> Vec<u8> {
    let mut m = reversed(u);
    m.extend(reversed(v));
    m.push(z);
    reversed(&aes_cmac(&reversed(x), &m))
}

/// A1 or A2 of f5 and f6: the address type, then the 6 octets most significant first.
pub fn addr7(addr_type: i64, addr: &[u8]) -> Vec<u8> {
    let mut out = vec![addr_type as u8];
    out.extend(reversed(addr));
    out
}

/// MacKey and LTK (on-air order).
pub fn f5(w: &[u8], n1: &[u8], n2: &[u8], a1: &[u8], a2: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let t = aes_cmac(&SC_SALT, &reversed(w));
    let mut body = b"btle".to_vec();
    body.extend(reversed(n1));
    body.extend(reversed(n2));
    body.extend_from_slice(a1);
    body.extend_from_slice(a2);
    body.extend_from_slice(&[1, 0]);
    let mut m0 = vec![0u8];
    m0.extend_from_slice(&body);
    let mut m1 = vec![1u8];
    m1.extend_from_slice(&body);
    (reversed(&aes_cmac(&t, &m0)), reversed(&aes_cmac(&t, &m1)))
}

pub fn f6(w: &[u8], n1: &[u8], n2: &[u8], r: &[u8], iocap: &[u8], a1: &[u8], a2: &[u8]) -> Vec<u8> {
    let mut m = reversed(n1);
    m.extend(reversed(n2));
    m.extend(reversed(r));
    m.extend_from_slice(iocap);
    m.extend_from_slice(a1);
    m.extend_from_slice(a2);
    reversed(&aes_cmac(&reversed(w), &m))
}

pub fn g2(u: &[u8], v: &[u8], x: &[u8], y: &[u8]) -> u32 {
    let mut m = reversed(u);
    m.extend(reversed(v));
    m.extend(reversed(y));
    let mac = aes_cmac(&reversed(x), &m);
    u32::from_be_bytes([mac[12], mac[13], mac[14], mac[15]]) % 1000000
}

/// AuthReq, OOB flag, IO capability of a Pairing Request or Response.
pub fn iocap_of(pdu: &[u8]) -> Vec<u8> {
    vec![pdu[3], pdu[2], pdu[1]]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gofmt::{hex, unhex};

    fn h(s: &str) -> Vec<u8> {
        let (b, ok) = unhex(s);
        assert!(ok);
        b
    }

    #[test]
    fn secure_connections() {
        let k = h("2b7e151628aed2a6abf7158809cf4f3c");
        let m = h("6bc1bee22e409f96e93d7e117393172aae2d8a571e03ac9c9eb76fac45af8e5130c81c46a35ce411e5fbc1191a0a52eff69f2445df4f9b17ad2b417be66c3710");
        for (n, want) in [(0, "bb1d6929e95937287fa37d129b756746"), (16, "070a16b46b4d4144f79bdd9dd04a287c"),
            (40, "dfa66747de9ae63030ca32611497c827"), (64, "51f0bebf7e3b9d92fc49741779363cfe")] {
            assert_eq!(hex(&aes_cmac(&k, &m[..n])), want, "AES-CMAC of {} octets", n);
        }
        let u = h("e69d350e480103ccdbfdf4ac1191f4efb9a5f9e9a7832c5e2cbe97f2d203b020");
        let v = h("fdc57ff449dd4f6bfb7c9df1c29acb592ae7d4eefbfc0a909abbf6323d8b1855");
        let x = h("abae2b71ecb2ffff3e7377d15484cbd5");
        let y = h("cfc43dfff78365216e5fa725cce7e8a6");
        assert_eq!(hex(&f4(&u, &v, &x, 0)), "2d8774a9bea1edf11cbda907f116c9f2");
        let w = h("98a6bf73f3348d86f166f8b4136b79999b7d390aa610103405adc857a33402ec");
        let (a1, a2) = (addr7(0, &h("cebf37371256")), addr7(0, &h("c1cf2d7013a7")));
        let (mackey, ltk) = f5(&w, &x, &y, &a1, &a2);
        assert_eq!(hex(&mackey), "206e63ce206a3ffd024a08a176f16529");
        assert_eq!(hex(&ltk), "380a7594b522059823cdd76911798669");
        assert_eq!(hex(&f6(&mackey, &x, &y, &h("c80f2d0cd242da0854bb53b43b34a312"), &[1, 1, 2], &a1, &a2)), "618f95da090b6cd2c5e8d09c9873c4e3");
        assert_eq!(g2(&u, &v, &x, &y), 0x2f9ed5ba % 1000000);
        let da = le_to_big(&h("bd1a3ccda6b8995899b740eb7b60ff4a503f10d2e3b3c974385fc5a3d4f6493f"));
        assert_eq!(hex(&public_key(&da)), hex(&u) + "8bd28915d08e1c742430ed8fc24563765c15525abf9a32636deb2a65499c80dc");
        let db = le_to_big(&h("548d20b8970bbc439aad106f6074d46a55c17a178b60e0b45ae658f1ea12d9fb"));
        let pka = h("bcf2d8a5dba3956c99f9110d4d2ef0bdee9b69b6cd8874be40e8e5ccdc884453bfa9820e187a14f877fd8e922af85d39d16d921f387499dc6c2c9423f97256ab");
        let pkb = public_key(&db);
        assert_eq!(hex(&pkb), "728cd188d7be49b2c55c95b364e01232b6c9476337385b9c1e1b1a0609e23185193a296962d630e7e84863dc00730a707d2e29cc917771b175b8f7dcb0e29110");
        let mut off = pka.clone();
        off[32] ^= 1;
        assert!(on_curve(&pka) && !on_curve(&off));
        let dh = dhkey(&db, &pka);
        let (na, nb) = (h("a4345fb3af734364cd191b5b87583166"), h("c091fbb377a2020bc6cd6c0451454539"));
        let (ia, ra) = (addr7(0, &h("ca61a06794e0")), addr7(0, &h("33221100450a")));
        assert_eq!(hex(&f4(&pkb[..32], &pka[..32], &nb, 0)), "82edd062913d967f13c50d022b5e4316");
        let (mackey, ltk) = f5(&dh, &na, &nb, &ia, &ra);
        assert_eq!(hex(&ltk), "63598a14094b946effae5e538602a36c");
        let io = [0x09, 0x00, 0x03];
        assert_eq!(hex(&f6(&mackey, &na, &nb, &[0u8; 16], &io, &ia, &ra)), "82651d02ed891344041a147c329a1e7d");
        assert_eq!(hex(&f6(&mackey, &nb, &na, &[0u8; 16], &io, &ra, &ia)), "063c284ae5484b51654e145e2fddfa22");
        assert!(!p256_order().is_zero());
    }
}
