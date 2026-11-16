use super::{rbg_machine, rbg_read};
use crate::bigint::BigUint;
use crate::pke::*;
use crate::Machine;

fn bi(h: &str) -> BigUint {
    BigUint::from_hex(h).unwrap()
}

fn pke_machine() -> Machine {
    let mut m = rbg_machine();
    m.regs[0x61] &= !0x80;
    m.regs[0x64] |= 0x80;
    m.reg_write(PKE_CONF, 4, 2 << 24 | 8 << 16).unwrap();
    m.reg_write(PKE_EXE_CONF, 4, 0x15).unwrap();
    m
}

fn pke_run(m: &mut Machine, mc: u32) -> Result<u32, crate::Error> {
    m.reg_write(PKE_MC_PTR, 4, mc).unwrap();
    m.reg_write(PKE_STAT, 4, 0).unwrap();
    m.reg_write(PKE_CTRL, 4, 1)?;
    m.cycles = m.pke_done_at;
    Ok(rbg_read(m, PKE_RT_CODE, 4))
}

fn a_slot(i: usize) -> usize {
    PKE_RAM_A + i * PKE_STEP
}
fn b_slot(i: usize) -> usize {
    PKE_RAM_B + i * PKE_STEP
}

struct Curve {
    p: BigUint,
    a: BigUint,
    b: BigUint,
    r2: BigUint,
    n1: BigUint,
}

fn curve(m: &mut Machine, c: &Curve) {
    m.pke_store(b_slot(3), &c.p, 9);
    m.pke_store(a_slot(3), &c.r2, 9);
    m.pke_store(b_slot(4), &c.n1, 9);
    m.pke_store(a_slot(5), &c.a, 9);
}

fn mul(m: &mut Machine, c: &Curve, k: &BigUint, x: &BigUint, y: &BigUint) -> (u32, BigUint, BigUint) {
    curve(m, c);
    m.pke_store(b_slot(0), x, 9);
    m.pke_store(b_slot(1), y, 9);
    m.pke_store(a_slot(4), k, 9);
    let rt = pke_run(m, PKE_PMUL).unwrap();
    (rt, m.pke_operand(a_slot(0), 9), m.pke_operand(a_slot(1), 9))
}

fn verify(m: &mut Machine, c: &Curve, x: &BigUint, y: &BigUint) -> u32 {
    curve(m, c);
    m.pke_store(b_slot(0), x, 9);
    m.pke_store(b_slot(1), y, 9);
    m.pke_store(a_slot(4), &c.b, 9);
    pke_run(m, PKE_PVER).unwrap()
}

#[test]
fn pke() {
    let p = bi("ffffffff00000001000000000000000000000000ffffffffffffffffffffffff");
    let a = p.sub(&BigUint::from_u64(3));
    let b = bi("5ac635d8aa3a93e7b3ebbd55769886bc651d06b0cc53b0f63bce3c3e27d2604b");
    let gx = bi("6b17d1f2e12c4247f8bce6e563a440f277037d812deb33a0f4a13945d898c296");
    let gy = bi("4fe342e2fe1a7f9b8ee7eb4a7c0f9e162bce33576b315ececbb6406837bf51f5");
    let (r2, n1) = pke_montgomery(&p);
    let c = Curve { p: p.clone(), a, b, r2: r2.clone(), n1: n1.clone() };
    let da = bi("3f49f6d4a3c55f3874c9b3e3d2103f504aff607beb40b7995899b8a6cd3c1abd");
    let ax = bi("20b003d2f297be2c5e2c83a7e9f9a5b9eff49111acf4fddbcc0301480e359de6");
    let ay = bi("dc809c49652aeb6d63329abf5a52155c766345c28fed3024741c8ed01589d28b");
    let db = bi("55188b3d32f6bb9a900afcfbeed4e72a59cb9ac2f19d7cfb6b4fdd49f47fc5fd");
    let bx = bi("1ea1f0f01faf1d9609592284f19e4c0047b58afd8615a69f559077b22faaa190");
    let by = bi("4c55f33e429dad377356703a9ab85160472d1130e28e36765f89aff915b1214a");
    let dhkey = bi("ec0234a357c8ad05341010a60a397d9b99796b13b4f866f1868d34f373bfa698");
    let mut m = pke_machine();
    let (rt, x, y) = mul(&mut m, &c, &da, &gx, &gy);
    assert!(rt == 0 && x == ax && y == ay, "PMUL dA*G: {} {:x} {:x}", rt, x, y);
    let (rt, x, y) = mul(&mut m, &c, &db, &gx, &gy);
    assert!(rt == 0 && x == bx && y == by, "PMUL dB*G");
    let (rt, x, y) = mul(&mut m, &c, &da, &bx, &by);
    assert!(rt == 0 && x == dhkey, "PMUL dA*B");
    let (rt2, x2, y2) = mul(&mut m, &c, &db, &ax, &ay);
    assert!(rt2 == 0 && x2 == dhkey && y2 == y, "PMUL dB*A");
    curve(&mut m, &c);
    m.pke_store(b_slot(0), &gx, 9);
    m.pke_store(b_slot(1), &gy, 9);
    m.pke_store(a_slot(4), &BigUint::zero(), 9);
    for (off, v) in [(PKE_MC_PTR, PKE_PMUL), (PKE_STAT, 0), (PKE_CTRL, 1)] {
        m.reg_write(off, 4, v).unwrap();
    }
    m.cycles += 100 * m.cfg.pke_us * m.cpu_hz / 1000000;
    assert_eq!(rbg_read(&mut m, PKE_STAT, 4), 0, "PMUL 0*G: Done after 100 times the operation's time");
    m.reg_write(PKE_CTRL, 4, 1 << 16).unwrap();
    assert!(
        rbg_read(&mut m, PKE_STAT, 4) == 1 && rbg_read(&mut m, PKE_RT_CODE, 4) == PKE_RT_STOPPED,
        "PMUL 0*G: Stop"
    );
    let c0 = m.cycles;
    assert_eq!(verify(&mut m, &c, &ax, &ay), 0, "PVER A");
    assert_eq!(m.cycles - c0, (m.cfg.pke_pver_us * m.cpu_hz + 999999) / 1000000, "PVER time");
    assert_eq!(verify(&mut m, &c, &ax, &ay.add(&BigUint::from_u64(1))), PKE_RT_NOT_ON_CURVE, "PVER off the curve");
    assert_eq!(verify(&mut m, &c, &ax, &p), PKE_RT_NOT_ON_CURVE, "PVER y = p");
    m.pke_store(a_slot(3), &BigUint::zero(), 9);
    m.pke_store(b_slot(4), &BigUint::zero(), 9);
    let rt = pke_run(&mut m, PKE_CAL_PRE_MON).unwrap();
    assert!(
        rt == 0 && m.pke_operand(a_slot(3), 8) == r2 && m.pke_operand(b_slot(4), 1) == n1,
        "CAL_PRE_MON"
    );

    let mut m = pke_machine();
    curve(&mut m, &c);
    m.pke_store(b_slot(0), &gx, 9);
    m.pke_store(b_slot(1), &gy, 9);
    m.pke_store(a_slot(4), &da, 9);
    for (o, v) in [(0xFFEC, 1234), (PKE_MC_PTR, PKE_PMUL), (PKE_CTRL, 1)] {
        m.reg_write(o, 4, v).unwrap();
    }
    let t0 = m.cycles;
    assert!(
        rbg_read(&mut m, PKE_STAT, 4) == 0 && rbg_read(&mut m, PKE_RT_CODE, 4) == 0 && m.pke_operand(a_slot(0), 8) != ax,
        "seen while the engine runs"
    );
    m.cycles = t0 + 1234 * m.cpu_hz / 1000000 - 1;
    assert_eq!(rbg_read(&mut m, PKE_STAT, 4), 0, "Done one cycle early");
    m.cycles += 1;
    assert!(rbg_read(&mut m, PKE_STAT, 1) == 1 && m.pke_operand(a_slot(0), 8) == ax, "Done or the result missing");
    m.reg_write(PKE_STAT, 4, 0).unwrap();
    assert_eq!(rbg_read(&mut m, PKE_STAT, 4), 0, "a write to STAT does not clear Done");
    m.reg_write(PKE_CTRL, 4, 1).unwrap();
    m.cycles += 10;
    m.reg_write(PKE_CTRL, 4, 0x10000).unwrap();
    assert!(
        rbg_read(&mut m, PKE_STAT, 4) == 1 && rbg_read(&mut m, PKE_RT_CODE, 4) == PKE_RT_STOPPED,
        "Stop does not end the operation with STOP_LOG 1"
    );

    let mut m = pke_machine();
    curve(&mut m, &c);
    m.pke_store(b_slot(0), &gx, 9);
    m.pke_store(b_slot(1), &gy, 9);
    m.pke_store(a_slot(4), &da, 9);
    let expect = |what: &str, text: &str, r: Result<u32, crate::Error>| {
        let e = super::err_text(&r);
        assert!(!e.is_empty() && e.contains(text), "{}: {}", what, e);
    };
    m.regs[0x64] &= !0x80;
    expect("without the clock", "without its clock", pke_run(&mut m, PKE_PMUL));
    m.regs[0x64] |= 0x80;
    m.regs[0x61] |= 0x80;
    expect("in reset", "in reset", pke_run(&mut m, PKE_PMUL));
    m.regs[0x61] &= !0x80;
    let _ = m.reg_write(PKE_CONF, 4, 2 << 24 | 6 << 16);
    expect("192-bit operands", "radix", pke_run(&mut m, PKE_PMUL));
    let _ = m.reg_write(PKE_CONF, 4, 2 << 24 | 8 << 16);
    let _ = m.reg_write(PKE_EXE_CONF, 4, 0x2A);
    expect("Montgomery operands", "operand form", pke_run(&mut m, PKE_PMUL));
    let _ = m.reg_write(PKE_EXE_CONF, 4, 0x15);
    expect("MODMUL", "microcode 0x18", pke_run(&mut m, 0x18));
    let mut r2x = r2.to_words(8);
    r2x[0] ^= 1;
    m.pke_store(a_slot(3), &BigUint::from_words(&r2x), 9);
    expect("wrong R^2", "R^2 mod p", pke_run(&mut m, PKE_PMUL));
    m.pke_store(a_slot(3), &r2, 9);
    let _ = m.reg_write(PKE_MC_PTR, 4, PKE_PMUL);
    m.reg_write(PKE_CTRL, 4, 1).unwrap();
    let e = super::err_text(&m.reg_write(PKE_CTRL, 4, 1));
    assert!(e.contains("while running"), "while running: {}", e);
}
