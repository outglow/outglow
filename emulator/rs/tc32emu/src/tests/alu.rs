use super::thumb::*;
use crate::Machine;

const VALUES: [u32; 16] = [
    0,
    1,
    2,
    5,
    37,
    0x7FFF_FFFF,
    0x8000_0000,
    0x8000_0001,
    0xFFFF_FFFF,
    0xFFFF_FFFE,
    0xFFFF_FFF7,
    0xFFFF_FF9C,
    0xDEAD_BEEF,
    0x1234_5678,
    0xF0F0_F0F0,
    0x0FF0_0FF0,
];

const AMOUNTS: [u32; 14] = [0, 1, 4, 12, 16, 31, 32, 33, 40, 63, 64, 255, 256, 0x120];

#[derive(Clone, Copy, Debug, PartialEq)]
struct Out {
    r: u32,
    n: u32,
    z: u32,
    c: u32,
    v: u32,
}

fn bit(x: u32, i: u32) -> u32 {
    (x >> i) & 1
}

fn nz(r: u32, c: u32, v: u32) -> Out {
    Out { r, n: r >> 31, z: (r == 0) as u32, c, v }
}

fn add_with_carry(a: u32, b: u32, c: u32) -> Out {
    let wide = a as u64 + b as u64 + c as u64;
    let r = wide as u32;
    let sv = (a as i32 as i64) + (b as i32 as i64) + c as i64;
    nz(r, (wide >> 32) as u32, (sv != r as i32 as i64) as u32)
}

fn lsl(a: u32, s: u32, c: u32) -> (u32, u32) {
    match s {
        0 => (a, c),
        1..=31 => (a << s, bit(a, 32 - s)),
        32 => (0, a & 1),
        _ => (0, 0),
    }
}

fn lsr(a: u32, s: u32, c: u32) -> (u32, u32) {
    match s {
        0 => (a, c),
        1..=31 => (a >> s, bit(a, s - 1)),
        32 => (0, a >> 31),
        _ => (0, 0),
    }
}

fn asr(a: u32, s: u32, c: u32) -> (u32, u32) {
    match s {
        0 => (a, c),
        1..=31 => (((a as i32) >> s) as u32, bit(a, s - 1)),
        _ => (((a as i32) >> 31) as u32, a >> 31),
    }
}

fn ror(a: u32, s: u32, c: u32) -> (u32, u32) {
    if s == 0 {
        return (a, c);
    }
    let r = a.rotate_right(s & 31);
    (r, r >> 31)
}

fn reference(op: u16, a: u32, b: u32, c: u32, v: u32) -> Out {
    let s = b & 0xFF;
    match op {
        AND => nz(a & b, c, v),
        EOR => nz(a ^ b, c, v),
        ORR => nz(a | b, c, v),
        BIC => nz(a & !b, c, v),
        MVN => nz(!b, c, v),
        TST => Out { r: a, ..nz(a & b, c, v) },
        LSL => {
            let (r, c) = lsl(a, s, c);
            nz(r, c, v)
        }
        LSR => {
            let (r, c) = lsr(a, s, c);
            nz(r, c, v)
        }
        ASR => {
            let (r, c) = asr(a, s, c);
            nz(r, c, v)
        }
        ROR => {
            let (r, c) = ror(a, s, c);
            nz(r, c, v)
        }
        ADC => add_with_carry(a, b, c),
        SBC => add_with_carry(a, !b, c),
        NEG => add_with_carry(0, !b, 1),
        CMP => Out { r: a, ..add_with_carry(a, !b, 1) },
        CMN => Out { r: a, ..add_with_carry(a, b, 0) },
        MUL => nz(a.wrapping_mul(b), c, v),
        _ => unreachable!(),
    }
}

fn exec(m: &mut Machine, a: u32, b: u32, c: u32, v: u32) -> Out {
    m.r[15] = CODE;
    m.r[0] = a;
    m.r[1] = b;
    m.n = 0;
    m.z = 0;
    m.c = c;
    m.v = v;
    m.step().unwrap();
    Out { r: m.r[0], n: m.n, z: m.z, c: m.c, v: m.v }
}

fn check(hw: u16, name: &str, a_set: &[u32], b_set: &[u32], want: impl Fn(u32, u32, u32, u32) -> Out, ignore_c: bool) {
    let mut m = machine();
    put(&mut m, CODE, &[hw]);
    let mut bad = vec![];
    for &a in a_set {
        for &b in b_set {
            for c in [0, 1] {
                for v in [0, 1] {
                    let mut got = exec(&mut m, a, b, c, v);
                    let mut exp = want(a, b, c, v);
                    if ignore_c {
                        got.c = 0;
                        exp.c = 0;
                    }
                    if got != exp {
                        bad.push(format!("a=0x{:x} b=0x{:x} c={} v={}: got {:?}, want {:?}", a, b, c, v, got, exp));
                    }
                }
            }
        }
    }
    assert!(bad.is_empty(), "{}: {} mismatches, first {}", name, bad.len(), bad[0]);
}

#[test]
fn data_processing_ops_match_armv4t_semantics() {
    for (op, name) in [
        (AND, "AND"),
        (EOR, "EOR"),
        (ORR, "ORR"),
        (BIC, "BIC"),
        (MVN, "MVN"),
        (TST, "TST"),
        (ADC, "ADC"),
        (SBC, "SBC"),
        (NEG, "NEG"),
        (CMP, "CMP"),
        (CMN, "CMN"),
    ] {
        check(alu(op, 0, 1), name, &VALUES, &VALUES, |a, b, c, v| reference(op, a, b, c, v), false);
    }
}

#[test]
fn mul_matches_the_low_word_and_nz_while_carry_is_unpredictable() {
    check(alu(MUL, 0, 1), "MUL", &VALUES, &VALUES, |a, b, c, v| reference(MUL, a, b, c, v), true);
}

#[test]
fn register_shifts_and_rotate_match_armv4t_semantics() {
    let mut amounts = AMOUNTS.to_vec();
    amounts.extend_from_slice(&[0xFFFF_FFE0, 0xFFFF_FFF8, 0x8000_0020]);
    for (op, name) in [(LSL, "LSL"), (LSR, "LSR"), (ASR, "ASR"), (ROR, "ROR")] {
        check(alu(op, 0, 1), name, &VALUES, &amounts, |a, b, c, v| reference(op, a, b, c, v), false);
    }
}

#[test]
fn immediate_shifts_match_armv4t_semantics() {
    for imm in 0..32u32 {
        check(lsl_imm(0, 1, imm as u16), "LSL #imm", &[0], &VALUES, |_, b, c, v| {
            let (r, c) = lsl(b, imm, c);
            nz(r, c, v)
        }, false);
        let s = if imm == 0 { 32 } else { imm };
        check(lsr_imm(0, 1, imm as u16), "LSR #imm", &[0], &VALUES, |_, b, c, v| {
            let (r, c) = lsr(b, s, c);
            nz(r, c, v)
        }, false);
        check(asr_imm(0, 1, imm as u16), "ASR #imm", &[0], &VALUES, |_, b, c, v| {
            let (r, c) = asr(b, s, c);
            nz(r, c, v)
        }, false);
    }
}

#[test]
fn three_register_add_and_sub_match_armv4t_semantics() {
    let mut m = machine();
    for (hw, sub) in [(add_rrr(0, 1, 2), false), (sub_rrr(0, 1, 2), true)] {
        put(&mut m, CODE, &[hw]);
        for &a in &VALUES {
            for &b in &VALUES {
                m.r[15] = CODE;
                m.r[1] = a;
                m.r[2] = b;
                m.c = 0;
                m.v = 0;
                m.step().unwrap();
                let exp = if sub { add_with_carry(a, !b, 1) } else { add_with_carry(a, b, 0) };
                assert_eq!(
                    Out { r: m.r[0], n: m.n, z: m.z, c: m.c, v: m.v },
                    exp,
                    "{} 0x{:x} 0x{:x}",
                    if sub { "SUB" } else { "ADD" },
                    a,
                    b
                );
            }
        }
    }
}

#[test]
fn immediate_forms_match_armv4t_semantics() {
    let mut m = machine();
    for imm in [0u16, 1, 7] {
        for (hw, sub) in [(add_imm3(0, 1, imm), false), (sub_imm3(0, 1, imm), true)] {
            put(&mut m, CODE, &[hw]);
            for &a in &VALUES {
                m.r[15] = CODE;
                m.r[1] = a;
                m.step().unwrap();
                let exp = if sub { add_with_carry(a, !(imm as u32), 1) } else { add_with_carry(a, imm as u32, 0) };
                assert_eq!(Out { r: m.r[0], n: m.n, z: m.z, c: m.c, v: m.v }, exp, "imm3 {} 0x{:x}", imm, a);
            }
        }
    }
    for imm in [0u16, 1, 0x7F, 0x80, 0xFF] {
        for kind in 0..4 {
            let hw = [mov_imm(0, imm), cmp_imm(0, imm), add_imm(0, imm), sub_imm(0, imm)][kind];
            put(&mut m, CODE, &[hw]);
            for &a in &VALUES {
                for c in [0, 1] {
                    m.r[15] = CODE;
                    m.r[0] = a;
                    m.c = c;
                    m.v = 1 - c;
                    m.step().unwrap();
                    let i = imm as u32;
                    let exp = match kind {
                        0 => nz(i, c, 1 - c),
                        1 => Out { r: a, ..add_with_carry(a, !i, 1) },
                        2 => add_with_carry(a, i, 0),
                        _ => add_with_carry(a, !i, 1),
                    };
                    assert_eq!(Out { r: m.r[0], n: m.n, z: m.z, c: m.c, v: m.v }, exp, "imm8 form {} #{} 0x{:x}", kind, imm, a);
                }
            }
        }
    }
}

#[test]
fn high_register_add_and_mov_leave_the_flags_and_cmp_sets_them() {
    let mut m = machine();
    for &a in &VALUES {
        for &b in &VALUES {
            put(&mut m, CODE, &[hi_add(8, 1)]);
            m.r[15] = CODE;
            m.r[8] = a;
            m.r[1] = b;
            m.n = 1;
            m.z = 1;
            m.c = 1;
            m.v = 1;
            m.step().unwrap();
            assert_eq!((m.r[8], m.n, m.z, m.c, m.v), (a.wrapping_add(b), 1, 1, 1, 1), "ADD r8 0x{:x} 0x{:x}", a, b);
            put(&mut m, CODE, &[hi_mov(0, 9)]);
            m.r[15] = CODE;
            m.r[9] = b;
            m.step().unwrap();
            assert_eq!((m.r[0], m.n, m.z, m.c, m.v), (b, 1, 1, 1, 1), "MOV r0, r9");
            put(&mut m, CODE, &[hi_cmp(10, 1)]);
            m.r[15] = CODE;
            m.r[10] = a;
            m.step().unwrap();
            let exp = add_with_carry(a, !b, 1);
            assert_eq!((m.n, m.z, m.c, m.v), (exp.n, exp.z, exp.c, exp.v), "CMP r10 0x{:x} 0x{:x}", a, b);
        }
    }
}

#[test]
fn conditional_branches_follow_the_flags() {
    let conds: [(u16, fn(u32, u32, u32, u32) -> bool); 14] = [
        (0, |_, z, _, _| z == 1),
        (1, |_, z, _, _| z == 0),
        (2, |_, _, c, _| c == 1),
        (3, |_, _, c, _| c == 0),
        (4, |n, _, _, _| n == 1),
        (5, |n, _, _, _| n == 0),
        (6, |_, _, _, v| v == 1),
        (7, |_, _, _, v| v == 0),
        (8, |_, z, c, _| c == 1 && z == 0),
        (9, |_, z, c, _| c == 0 || z == 1),
        (10, |n, _, _, v| n == v),
        (11, |n, _, _, v| n != v),
        (12, |n, z, _, v| z == 0 && n == v),
        (13, |n, z, _, v| z == 1 || n != v),
    ];
    let mut m = machine();
    for (cond, taken) in conds {
        put(&mut m, CODE, &[0xD000 | cond << 8 | 0x10]);
        for bits in 0..16u32 {
            let (n, z, c, v) = (bits >> 3 & 1, bits >> 2 & 1, bits >> 1 & 1, bits & 1);
            m.r[15] = CODE;
            m.n = n;
            m.z = z;
            m.c = c;
            m.v = v;
            m.step().unwrap();
            let want = if taken(n, z, c, v) { CODE + 4 + 0x20 } else { CODE + 2 };
            assert_eq!(m.r[15], want, "cond {} nzcv {:04b}", cond, bits);
        }
    }
}

#[test]
fn signed_and_unsigned_compares_differ_on_the_same_bits() {
    let mut m = machine();
    for (a, b) in [(0xFFFF_FFFFu32, 1u32), (5, 5), (7, 3), (0x8000_0000, 0x7FFF_FFFF)] {
        put(&mut m, CODE, &[alu(CMP, 0, 1)]);
        m.r[15] = CODE;
        m.r[0] = a;
        m.r[1] = b;
        m.step().unwrap();
        let lt_signed = m.n != m.v;
        let lo_unsigned = m.c == 0;
        assert_eq!(lt_signed, (a as i32) < (b as i32), "LT 0x{:x} 0x{:x}", a, b);
        assert_eq!(lo_unsigned, a < b, "CC 0x{:x} 0x{:x}", a, b);
        assert_eq!(m.z == 1, a == b);
    }
}
