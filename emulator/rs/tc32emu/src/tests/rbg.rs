use std::collections::HashMap;

use super::{digest_u32, hex, rbg_machine, rbg_read, unhex};
use crate::Machine;

fn rbg_digests() -> HashMap<&'static str, &'static str> {
    [
        ("lcg", "21fec69d8d40f43d3db8cc0aa5b50187ee93f5753249756e287297ec53dcfef3"),
        ("healthy:7", "5f3e4c48b587b6a658ddb749fca054f558a383328ba57710b6861bec4e12d041"),
        ("stuck:0", "8bfe96b7ab7217459a0d2f0b4b020a21e5976fec991eba4803711536093ca1b2"),
        ("biased:243", "03e6ae5ee4cfd96a59b7ff5d6bdb3bc0f9fdcb94017963cd02219c6fc50f58c4"),
        ("cycle:8", "6401d64776273163028f7e7a56f521db75ebc3e786e39498a3b263ef3d9c0f1a"),
        ("attacker:4660", "7fe500de9c002c813d5f8d993debc20c3be1c1a42995a6b30f56bb7f202abfdf"),
    ]
    .into_iter()
    .collect()
}

const K32_DIGEST: &str = "7b2833aff6ee20cb0b0d95f91a7da645d29e9ef052bbf5fe729553c0b5127307";

fn rbg_words(m: &mut Machine, n: usize) -> Vec<u32> {
    (0..n).map(|_| rbg_read(m, 0x440C, 4)).collect()
}

#[test]
fn rbg_sources() {
    let d = rbg_digests();
    let mut m = rbg_machine();
    assert_eq!(rbg_read(&mut m, 0x4408, 1) & 1, 1, "lcg: SR bit 0 reads 0 with the block off");
    let ws = rbg_words(&mut m, 64);
    assert_eq!(ws[0], 0x0B719151, "lcg: first word");
    assert_eq!(digest_u32(&ws), d["lcg"], "lcg: digest");
    for (name, mode, param) in
        [("healthy:7", 1u32, 7u32), ("stuck:0", 2, 0), ("biased:243", 3, 243), ("cycle:8", 4, 8), ("attacker:4660", 5, 4660)]
    {
        let mut m = rbg_machine();
        m.reg_write(0xFFF4, 4, mode | param << 8).unwrap();
        assert!(
            rbg_read(&mut m, 0x4408, 1) & 1 == 0 && rbg_read(&mut m, 0x440C, 4) == 0,
            "{}: ready or DR not 0 with the block off",
            name
        );
        m.regs[0x65] |= 0x08;
        m.regs[0x4400] |= 0x01;
        assert_eq!(rbg_read(&mut m, 0x4408, 1) & 1, 1, "{}: not ready when clocked and enabled", name);
        m.regs[0x62] |= 0x08;
        assert_eq!(rbg_read(&mut m, 0x4408, 1) & 1, 0, "{}: ready in reset", name);
        m.regs[0x62] &= !0x08;
        let ws = rbg_words(&mut m, 64);
        assert_eq!(digest_u32(&ws), d[name], "{}: digest", name);
        if mode == 3 {
            let ones: u32 = rbg_words(&mut m, 1024).iter().map(|w| w.count_ones()).sum();
            let f = ones as f64 / (1024.0 * 32.0);
            assert!(f >= 243.0 / 256.0 - 0.01 && f <= 243.0 / 256.0 + 0.01, "biased: {}", f);
        }
        m.reg_write(0xFFF4, 4, mode | param << 8).unwrap();
        assert_eq!(digest_u32(&rbg_words(&mut m, 64)), d[name], "{}: setting the source again", name);
    }
    let mut m = rbg_machine();
    m.regs[0x65] |= 0x08;
    m.regs[0x4400] |= 0x01;
    m.reg_write(0xFFF4, 4, 6).unwrap();
    assert_eq!(rbg_read(&mut m, 0x4408, 1) & 1, 0, "never: SR bit 0 reads 1");
    assert!(m.reg_write(0xFFF4, 4, 7).is_err(), "an unknown source is taken");
}

#[test]
fn k32_jitter() {
    let mut m = rbg_machine();
    assert_eq!(m.k32_now(), 0, "power-on: the count does not start at 0");
    m.reg_write(0xFFFC, 4, 7).unwrap();
    m.reg_write(0xFFF8, 4, 200).unwrap();
    let mut counts = vec![];
    let (mut worst, mut moved) = (0i64, 0);
    for _ in 0..4000 {
        m.cycles += 977;
        let c = m.k32_now();
        let e = (m.ms() * 32.768) as i64;
        counts.push(c);
        let d = (c as i64 - e).abs();
        if d > worst {
            worst = d;
        }
        if c as i64 != e {
            moved += 1;
        }
    }
    assert!(worst <= 3, "jitter 200 ns: the count {} ticks from the exact one", worst);
    assert!(moved > 0, "jitter 200 ns: the count never differs from the exact one");
    for i in 1..counts.len() {
        assert!(counts[i] >= counts[i - 1], "jitter: the count goes back");
    }
    m.cycles += 24_000_000;
    let c = m.k32_now();
    let d = c as i64 - (m.ms() * 32.768) as i64;
    assert!((-3..=3).contains(&d), "jitter: after a jump of 1 s the count is {} off", d);
    counts.push(c);
    assert_eq!(digest_u32(&counts), K32_DIGEST, "k32 jitter digest");
    m.reg_write(0xFFF8, 4, 0).unwrap();
    assert_eq!(m.k32_now(), (m.ms() * 32.768) as i64 as u32, "jitter 0: not the exact count");
    m.reg_write(0xFFF8, 4, 0x80000000).unwrap();
    let (c0, b0) = (m.k32_now(), rbg_read(&mut m, 0x74B, 1));
    m.cycles += 48_000;
    assert!(
        m.k32_now() == c0 && rbg_read(&mut m, 0x750, 4) == c0 && rbg_read(&mut m, 0x74B, 1) == b0,
        "0xfff8 bit 31: the 32 kHz count moves"
    );
    m.reg_write(0xFFF8, 4, 0).unwrap();
    assert_eq!(m.k32_now(), (m.ms() * 32.768) as i64 as u32, "0xfff8 bit 31 clear: the count does not go on");
}

#[test]
fn aes_block() {
    let mut m = rbg_machine();
    let key = unhex("2b7e151628aed2a6abf7158809cf4f3c");
    let pt = unhex("3243f6a8885a308d313198a2e0370734");
    let ct = unhex("3925841d02dc09fbdc118597196a0b32");
    for dec in [0u32, 1] {
        let (inp, want) = if dec == 1 { (&ct, &pt) } else { (&pt, &ct) };
        m.reg_write(0x540, 1, dec).unwrap();
        m.regs[0x550..0x560].copy_from_slice(&key);
        for i in (0..16).step_by(4) {
            m.reg_write(0x548, 4, u32::from_le_bytes(inp[i..i + 4].try_into().unwrap())).unwrap();
        }
        assert!(rbg_read(&mut m, 0x540, 1) & 0x04 != 0, "AES block: bit 2 not set after the fourth word");
        let mut got = vec![];
        for _ in 0..4 {
            got.extend_from_slice(&rbg_read(&mut m, 0x548, 4).to_le_bytes());
        }
        assert_eq!(hex(&got), hex(want), "AES block (decrypt {})", dec);
    }
}
