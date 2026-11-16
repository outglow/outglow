use std::cmp::Ordering;

use crate::bigint::BigUint;
use crate::error::{emu_err, Error};
use crate::machine::{le32, Machine};

pub(crate) const PKE_CTRL: usize = 0x2000;
pub(crate) const PKE_CONF: usize = 0x2004;
pub(crate) const PKE_MC_PTR: usize = 0x2010;
pub(crate) const PKE_STAT: usize = 0x2020;
pub(crate) const PKE_RT_CODE: usize = 0x2024;
pub(crate) const PKE_EXE_CONF: usize = 0x2050;
pub(crate) const PKE_RAM_A: usize = 0x2400;
pub(crate) const PKE_RAM_B: usize = 0x3000;
pub(crate) const PKE_STEP: usize = 0x24;
pub(crate) const PKE_RAM_END: usize = 0x3200;
pub(crate) const PKE_PMUL: u32 = 0x10;
pub(crate) const PKE_PVER: u32 = 0x0C;
pub(crate) const PKE_CAL_PRE_MON: u32 = 0x28;
pub(crate) const PKE_RT_STOPPED: u32 = 1;
pub(crate) const PKE_RT_NO_INVERSE: u32 = 2;
pub(crate) const PKE_RT_NOT_ON_CURVE: u32 = 3;

#[derive(Clone, Debug)]
pub(crate) struct PkeWrite {
    pub off: usize,
    pub value: BigUint,
    pub words: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct EcPoint {
    pub x: BigUint,
    pub y: BigUint,
}

fn modinv(g: &BigUint, p: &BigUint) -> BigUint {
    match BigUint::mod_inverse(g, p) {
        Some(v) => v,
        None => panic!("runtime error: invalid memory address or nil pointer dereference (ModInverse: no inverse)"),
    }
}

pub(crate) fn ec_add(p: &BigUint, a: &BigUint, pp: Option<&EcPoint>, q: Option<&EcPoint>) -> Option<EcPoint> {
    let pp = match pp {
        None => return q.cloned(),
        Some(v) => v,
    };
    let q = match q {
        None => return Some(pp.clone()),
        Some(v) => v,
    };
    let lam;
    if pp.x.cmp(&q.x) == Ordering::Equal {
        if pp.y.add(&q.y).rem(p).is_zero() {
            return None;
        }
        let num = pp.x.mul(&pp.x).mul(&BigUint::from_u64(3)).add(a);
        let d = pp.y.shl(1);
        lam = num.mul(&modinv(&d, p)).rem(p);
    } else {
        let d = BigUint::mod_sub(&q.x, &pp.x, p);
        let num = BigUint::mod_sub(&q.y, &pp.y, p);
        lam = num.mul(&modinv(&d, p)).rem(p);
    }
    let x3 = BigUint::mod_sub(&BigUint::mod_sub(&lam.mul(&lam), &pp.x, p), &q.x, p);
    let y3 = BigUint::mod_sub(&BigUint::mod_sub(&pp.x, &x3, p).mul(&lam), &pp.y, p);
    Some(EcPoint { x: x3, y: y3 })
}

pub(crate) fn ec_mul(p: &BigUint, a: &BigUint, k: &BigUint, pt: &EcPoint) -> Option<EcPoint> {
    let mut r: Option<EcPoint> = None;
    let mut pt = Some(pt.clone());
    for i in 0..k.bit_len() {
        if k.bit(i) != 0 {
            r = ec_add(p, a, r.as_ref(), pt.as_ref());
        }
        pt = ec_add(p, a, pt.as_ref(), pt.as_ref());
    }
    r
}

pub(crate) fn ec_on_curve(p: &BigUint, a: &BigUint, b: &BigUint, pt: &EcPoint) -> bool {
    if pt.x.cmp(p) != Ordering::Less || pt.y.cmp(p) != Ordering::Less {
        return false;
    }
    let l = pt.y.mul(&pt.y);
    let r = pt.x.mul(&pt.x).mul(&pt.x).add(&a.mul(&pt.x)).add(b);
    BigUint::mod_sub(&l, &r, p).is_zero()
}

pub(crate) fn pke_montgomery(p: &BigUint) -> (BigUint, BigUint) {
    let r = BigUint::from_u64(1).shl(256);
    let r2 = r.mul(&r).rem(p);
    let w = BigUint::from_u64(1 << 32);
    let n1 = match BigUint::mod_inverse(p, &w) {
        Some(v) => v,
        None => panic!("runtime error: invalid memory address or nil pointer dereference (ModInverse: no inverse)"),
    };
    let n1 = BigUint::mod_sub(&BigUint::zero(), &n1, &w);
    (r2, n1)
}

impl Machine {
    pub(crate) fn pke_operand(&self, off: usize, words: usize) -> BigUint {
        let w: Vec<u32> = (0..words).map(|i| le32(&self.regs, off + 4 * i)).collect();
        BigUint::from_words(&w)
    }

    pub(crate) fn pke_store(&mut self, off: usize, value: &BigUint, words: usize) {
        let w = value.to_words(words);
        for (i, x) in w.iter().enumerate() {
            self.regs[off + 4 * i..off + 4 * i + 4].copy_from_slice(&x.to_le_bytes());
        }
    }

    pub(crate) fn pke_done(&mut self) -> bool {
        if self.pke_done_at < 0 || self.cycles < self.pke_done_at {
            return false;
        }
        let pending = std::mem::take(&mut self.pke_pending);
        for w in &pending {
            self.pke_store(w.off, &w.value, w.words);
        }
        true
    }

    fn pke_stop(&mut self) {
        if self.pke_done_at >= 0 && !self.pke_done() {
            self.pke_rt = PKE_RT_STOPPED;
            self.pke_pending = Vec::new();
            self.pke_done_at = self.cycles;
            self.event("PKE: stopped");
        }
    }

    fn pke_go(&mut self) -> Result<(), Error> {
        let here = self.here();
        if self.regs[0x61] & 0x80 != 0 {
            return Err(emu_err(format!("PKE started while in reset (0x61 bit 7) at {}", here)));
        }
        if self.regs[0x64] & 0x80 == 0 {
            return Err(emu_err(format!("PKE started without its clock (0x64 bit 7) at {}", here)));
        }
        if self.pke_done_at >= 0 && !self.pke_done() {
            return Err(emu_err(format!("PKE started while running at {}", here)));
        }
        let conf = le32(&self.regs, PKE_CONF);
        if (conf >> 24) & 7 != 2 || (conf >> 16) & 0xFF != 8 {
            return Err(emu_err(format!(
                "PKE radix 0x{:08x} not modelled (256-bit operands: base 2, partial 8) at {}",
                conf, here
            )));
        }
        let exe = le32(&self.regs, PKE_EXE_CONF);
        if exe & 0x3F != 0x15 {
            return Err(emu_err(format!(
                "PKE operand form 0x{:02x} not modelled (affine, not Montgomery: 0x15) at {}",
                exe, here
            )));
        }
        let mc = le32(&self.regs, PKE_MC_PTR) & 0xFF;
        let a_slot = |i: usize| PKE_RAM_A + i * PKE_STEP;
        let b_slot = |i: usize| PKE_RAM_B + i * PKE_STEP;
        let p = self.pke_operand(b_slot(3), 8);
        let three = BigUint::from_u64(3);
        let (mut rt, mut out, text): (u32, Vec<PkeWrite>, String);
        if mc == PKE_CAL_PRE_MON {
            if p.cmp(&three) == Ordering::Less || p.bit(0) == 0 {
                return Err(emu_err(format!("PKE CAL_PRE_MON of 0x{:x} at {}", p, here)));
            }
            let (r2, n1) = pke_montgomery(&p);
            rt = 0;
            out = vec![
                PkeWrite { off: a_slot(3), value: r2, words: 9 },
                PkeWrite { off: b_slot(4), value: n1, words: 9 },
            ];
            text = "Montgomery constants".to_string();
        } else if mc == PKE_PMUL || mc == PKE_PVER {
            if p.cmp(&three) == Ordering::Less || p.bit(0) == 0 {
                return Err(emu_err(format!("PKE modulus 0x{:x} at {}", p, here)));
            }
            let (r2, n1) = pke_montgomery(&p);
            if self.pke_operand(a_slot(3), 8).cmp(&r2) != Ordering::Equal {
                return Err(emu_err(format!("PKE A3 is not R^2 mod p at {}", here)));
            }
            if self.pke_operand(b_slot(4), 1).cmp(&n1) != Ordering::Equal {
                return Err(emu_err(format!("PKE B4 is not -p^-1 mod 2^32 at {}", here)));
            }
            let pt = EcPoint { x: self.pke_operand(b_slot(0), 8), y: self.pke_operand(b_slot(1), 8) };
            let a = self.pke_operand(a_slot(5), 8);
            rt = 0;
            out = Vec::new();
            if mc == PKE_PVER {
                let b = self.pke_operand(a_slot(4), 8);
                let mut tx = "point on the curve".to_string();
                if !ec_on_curve(&p, &a, &b, &pt) {
                    rt = PKE_RT_NOT_ON_CURVE;
                    tx = "point not on the curve".to_string();
                }
                text = tx;
            } else {
                let k = self.pke_operand(a_slot(4), 8);
                if k.is_zero() {
                    self.pke_rt = 0;
                    self.pke_pending = Vec::new();
                    self.pke_done_at = i64::MAX;
                    self.event("PKE: point multiplication by 0, never done");
                    return Ok(());
                }
                let mut q = None;
                if pt.x.cmp(&p) == Ordering::Less && pt.y.cmp(&p) == Ordering::Less {
                    q = ec_mul(&p, &a, &k, &pt);
                }
                match q {
                    None => {
                        rt = PKE_RT_NO_INVERSE;
                        text = "point multiplication: no valid modulo inverse".to_string();
                    }
                    Some(q) => {
                        out = vec![
                            PkeWrite { off: a_slot(0), value: q.x, words: 9 },
                            PkeWrite { off: a_slot(1), value: q.y, words: 9 },
                        ];
                        text = "point multiplication".to_string();
                    }
                }
            }
        } else {
            return Err(emu_err(format!("PKE microcode 0x{:02x} not modelled at {}", mc, here)));
        }
        let us = if mc == PKE_PVER { self.cfg.pke_pver_us } else { self.pke_us };
        self.pke_rt = rt;
        self.pke_pending = out;
        self.pke_done_at = self.cycles + (us * self.cpu_hz + 999999) / 1000000;
        if self.has_event() {
            let t = format!("PKE: {}, done in {} us", text, us);
            self.event(&t);
        }
        Ok(())
    }

    pub(crate) fn pke_read(&mut self, o: usize, size: usize) -> Option<u32> {
        if o == PKE_STAT && (size == 1 || size == 4) {
            return Some(if self.pke_done() { 1 } else { 0 });
        }
        if o == PKE_RT_CODE && (size == 1 || size == 4) {
            return Some(if self.pke_done() { self.pke_rt } else { 0 });
        }
        if (PKE_RAM_A..PKE_RAM_END).contains(&o) {
            self.pke_done();
        }
        None
    }

    pub(crate) fn pke_write_reg(&mut self, o: usize, size: usize, val: u32) -> Option<Result<(), Error>> {
        if o == PKE_CTRL && (size == 1 || size == 4) {
            if val & 0x01 != 0 {
                if let Err(e) = self.pke_go() {
                    return Some(Err(e));
                }
            }
            if size == 4 && val & 0x10000 != 0 {
                self.pke_stop();
            }
            return Some(Ok(()));
        }
        if o == PKE_STAT && (size == 1 || size == 4) {
            if self.pke_done() {
                self.pke_done_at = -1;
            }
            return Some(Ok(()));
        }
        if o == 0xFFEC && size == 4 {
            self.pke_us = val as i64;
            if self.has_event() {
                let t = format!("PKE: {} us per operation", self.pke_us);
                self.event(&t);
            }
            self.store(o, 4, val);
            return Some(Ok(()));
        }
        None
    }
}
