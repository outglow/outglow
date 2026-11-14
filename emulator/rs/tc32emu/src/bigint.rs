// SPDX-License-Identifier: Apache-2.0
//! The little unsigned big-integer arithmetic the public key engine's model
//! needs (Go's math/big in pke.go): every value it computes is reduced
//! modulo a prime below 2^256, so non-negative integers and modular
//! subtraction give the same results as big.Int's signed arithmetic followed
//! by Mod (Euclidean, non-negative).

use std::cmp::Ordering;
use std::fmt;

/// An unsigned integer, little-endian 32-bit limbs, no high zero limbs.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BigUint {
    limbs: Vec<u32>,
}

impl BigUint {
    pub fn zero() -> BigUint {
        BigUint { limbs: Vec::new() }
    }

    pub fn from_u64(v: u64) -> BigUint {
        let mut b = BigUint { limbs: vec![v as u32, (v >> 32) as u32] };
        b.norm();
        b
    }

    /// From little-endian 32-bit words.
    pub fn from_words(w: &[u32]) -> BigUint {
        let mut b = BigUint { limbs: w.to_vec() };
        b.norm();
        b
    }

    /// From big-endian bytes (big.Int SetBytes).
    #[allow(dead_code)]
    pub fn from_bytes_be(b: &[u8]) -> BigUint {
        let mut limbs = Vec::with_capacity(b.len() / 4 + 1);
        let mut i = b.len();
        while i > 0 {
            let lo = i.saturating_sub(4);
            let mut w = 0u32;
            for &x in &b[lo..i] {
                w = (w << 8) | x as u32;
            }
            limbs.push(w);
            i = lo;
        }
        let mut r = BigUint { limbs };
        r.norm();
        r
    }

    /// Parses hex digits (big.Int SetString(s, 16)).
    #[allow(dead_code)]
    pub fn from_hex(s: &str) -> Option<BigUint> {
        let mut r = BigUint::zero();
        if s.is_empty() {
            return None;
        }
        for c in s.chars() {
            let d = c.to_digit(16)?;
            r = r.shl(4).add(&BigUint::from_u64(d as u64));
        }
        Some(r)
    }

    fn norm(&mut self) {
        while let Some(&0) = self.limbs.last() {
            self.limbs.pop();
        }
    }

    pub fn is_zero(&self) -> bool {
        self.limbs.is_empty()
    }

    /// The low words, n of them (FillBytes into 4n bytes, as words); panics
    /// when the value does not fit, as Go's FillBytes does.
    pub fn to_words(&self, n: usize) -> Vec<u32> {
        if self.limbs.len() > n {
            panic!("math/big: buffer too small to fit value");
        }
        let mut w = self.limbs.clone();
        w.resize(n, 0);
        w
    }

    pub fn bit_len(&self) -> usize {
        match self.limbs.last() {
            None => 0,
            Some(&top) => (self.limbs.len() - 1) * 32 + (32 - top.leading_zeros() as usize),
        }
    }

    pub fn bit(&self, i: usize) -> u32 {
        let (w, b) = (i / 32, i % 32);
        if w >= self.limbs.len() {
            0
        } else {
            (self.limbs[w] >> b) & 1
        }
    }

    pub fn cmp(&self, o: &BigUint) -> Ordering {
        if self.limbs.len() != o.limbs.len() {
            return self.limbs.len().cmp(&o.limbs.len());
        }
        for i in (0..self.limbs.len()).rev() {
            if self.limbs[i] != o.limbs[i] {
                return self.limbs[i].cmp(&o.limbs[i]);
            }
        }
        Ordering::Equal
    }

    pub fn add(&self, o: &BigUint) -> BigUint {
        let n = self.limbs.len().max(o.limbs.len());
        let mut r = Vec::with_capacity(n + 1);
        let mut carry = 0u64;
        for i in 0..n {
            let s = *self.limbs.get(i).unwrap_or(&0) as u64 + *o.limbs.get(i).unwrap_or(&0) as u64 + carry;
            r.push(s as u32);
            carry = s >> 32;
        }
        if carry != 0 {
            r.push(carry as u32);
        }
        let mut b = BigUint { limbs: r };
        b.norm();
        b
    }

    /// self - o; o must not be larger.
    pub fn sub(&self, o: &BigUint) -> BigUint {
        assert!(self.cmp(o) != Ordering::Less, "BigUint::sub underflow");
        let mut r = Vec::with_capacity(self.limbs.len());
        let mut borrow = 0i64;
        for i in 0..self.limbs.len() {
            let mut d = self.limbs[i] as i64 - *o.limbs.get(i).unwrap_or(&0) as i64 - borrow;
            if d < 0 {
                d += 1 << 32;
                borrow = 1;
            } else {
                borrow = 0;
            }
            r.push(d as u32);
        }
        let mut b = BigUint { limbs: r };
        b.norm();
        b
    }

    pub fn mul(&self, o: &BigUint) -> BigUint {
        if self.is_zero() || o.is_zero() {
            return BigUint::zero();
        }
        let mut r = vec![0u32; self.limbs.len() + o.limbs.len()];
        for (i, &a) in self.limbs.iter().enumerate() {
            let mut carry = 0u64;
            for (j, &b) in o.limbs.iter().enumerate() {
                let t = a as u64 * b as u64 + r[i + j] as u64 + carry;
                r[i + j] = t as u32;
                carry = t >> 32;
            }
            let mut k = i + o.limbs.len();
            while carry != 0 {
                let t = r[k] as u64 + carry;
                r[k] = t as u32;
                carry = t >> 32;
                k += 1;
            }
        }
        let mut b = BigUint { limbs: r };
        b.norm();
        b
    }

    pub fn shl(&self, n: usize) -> BigUint {
        if self.is_zero() {
            return BigUint::zero();
        }
        let (w, b) = (n / 32, n % 32);
        let mut r = vec![0u32; self.limbs.len() + w + 1];
        for (i, &x) in self.limbs.iter().enumerate() {
            let v = (x as u64) << b;
            r[i + w] |= v as u32;
            r[i + w + 1] |= (v >> 32) as u32;
        }
        let mut out = BigUint { limbs: r };
        out.norm();
        out
    }

    /// (quotient, remainder); panics on a zero divisor, as Go does.
    pub fn divrem(&self, d: &BigUint) -> (BigUint, BigUint) {
        assert!(!d.is_zero(), "division by zero");
        if self.cmp(d) == Ordering::Less {
            return (BigUint::zero(), self.clone());
        }
        if d.limbs.len() == 1 {
            let dv = d.limbs[0] as u64;
            let mut q = vec![0u32; self.limbs.len()];
            let mut rem = 0u64;
            for i in (0..self.limbs.len()).rev() {
                let cur = (rem << 32) | self.limbs[i] as u64;
                q[i] = (cur / dv) as u32;
                rem = cur % dv;
            }
            let mut qb = BigUint { limbs: q };
            qb.norm();
            return (qb, BigUint::from_u64(rem));
        }
        // Knuth, TAOCP 4.3.1, algorithm D.
        let s = d.limbs.last().unwrap().leading_zeros() as usize;
        let v = d.shl(s).limbs;
        let mut u = self.shl(s).limbs;
        if u.len() == self.limbs.len() {
            u.push(0);
        }
        u.push(0);
        let n = v.len();
        let m = u.len() - n - 1;
        let mut q = vec![0u32; m];
        let b: u64 = 1 << 32;
        for j in (0..m).rev() {
            let num = ((u[j + n] as u64) << 32) | u[j + n - 1] as u64;
            let mut qhat = num / v[n - 1] as u64;
            let mut rhat = num % v[n - 1] as u64;
            while qhat >= b || qhat * v[n - 2] as u64 > ((rhat << 32) | u[j + n - 2] as u64) {
                qhat -= 1;
                rhat += v[n - 1] as u64;
                if rhat >= b {
                    break;
                }
            }
            let mut borrow: i64 = 0;
            let mut carry: u64 = 0;
            for i in 0..n {
                let p = qhat * v[i] as u64 + carry;
                carry = p >> 32;
                let t = u[i + j] as i64 - borrow - (p & 0xFFFF_FFFF) as i64;
                u[i + j] = t as u32;
                borrow = if t < 0 { 1 } else { 0 };
            }
            let t = u[j + n] as i64 - borrow - carry as i64;
            u[j + n] = t as u32;
            if t < 0 {
                qhat -= 1;
                let mut c = 0u64;
                for i in 0..n {
                    let s2 = u[i + j] as u64 + v[i] as u64 + c;
                    u[i + j] = s2 as u32;
                    c = s2 >> 32;
                }
                u[j + n] = (u[j + n] as u64 + c) as u32;
            }
            q[j] = qhat as u32;
        }
        let mut qb = BigUint { limbs: q };
        qb.norm();
        let mut rb = BigUint { limbs: u[..n].to_vec() };
        rb.norm();
        // Undo the normalization shift.
        let rb = rb.shr(s);
        (qb, rb)
    }

    pub fn shr(&self, n: usize) -> BigUint {
        let (w, b) = (n / 32, n % 32);
        if w >= self.limbs.len() {
            return BigUint::zero();
        }
        let mut r = vec![0u32; self.limbs.len() - w];
        for i in 0..r.len() {
            let lo = self.limbs[i + w] as u64;
            let hi = *self.limbs.get(i + w + 1).unwrap_or(&0) as u64;
            r[i] = (((hi << 32) | lo) >> b) as u32;
        }
        let mut out = BigUint { limbs: r };
        out.norm();
        out
    }

    pub fn rem(&self, m: &BigUint) -> BigUint {
        self.divrem(m).1
    }

    /// (a - b) mod m for any a, b.
    pub fn mod_sub(a: &BigUint, b: &BigUint, m: &BigUint) -> BigUint {
        let (a, b) = (a.rem(m), b.rem(m));
        if a.cmp(&b) != Ordering::Less {
            a.sub(&b)
        } else {
            a.add(m).sub(&b)
        }
    }

    /// The inverse of g modulo n (big.Int ModInverse); None when g and n
    /// are not relatively prime.
    pub fn mod_inverse(g: &BigUint, n: &BigUint) -> Option<BigUint> {
        // Extended Euclid on (g mod n, n), the coefficient kept modulo n.
        let g = g.rem(n);
        let (mut r0, mut r1) = (n.clone(), g);
        let (mut t0, mut t1) = (BigUint::zero(), BigUint::from_u64(1));
        while !r1.is_zero() {
            let (q, r2) = r0.divrem(&r1);
            // t2 = t0 - q * t1 (mod n)
            let t2 = BigUint::mod_sub(&t0, &q.mul(&t1).rem(n), n);
            r0 = r1;
            r1 = r2;
            t0 = t1;
            t1 = t2;
        }
        if r0.cmp(&BigUint::from_u64(1)) != Ordering::Equal {
            if n.cmp(&BigUint::from_u64(1)) == Ordering::Equal {
                // Every g is its own inverse (0) modulo 1.
                return Some(BigUint::zero());
            }
            return None;
        }
        Some(t0.rem(n))
    }
}

impl fmt::LowerHex for BigUint {
    /// big.Int %x: lowercase, no leading zeros, "0" for zero.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.limbs.is_empty() {
            return f.write_str("0");
        }
        let mut s = format!("{:x}", self.limbs.last().unwrap());
        for w in self.limbs.iter().rev().skip(1) {
            s.push_str(&format!("{:08x}", w));
        }
        f.write_str(&s)
    }
}
