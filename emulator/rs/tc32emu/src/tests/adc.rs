// adc_test.go
use std::collections::{BTreeSet, HashMap};

use super::{digest_u32, rbg_machine};
use crate::{Machine, ADC_VBAT_CODE, SRAM_BASE};

fn adc_digests() -> HashMap<&'static str, &'static str> {
    [
        ("noise:0", "f39668f34e48954261e54f075e63acdd58c586ad3614b641fb3ebea7258247b8"),
        ("noise:0x0705", "49ddb0545586e3801bb99555ddd93e03df8f3787a57a03902297b4e16255610e"),
        ("off:0", "07b257c68af081ace0683be5705c4a6aa8334c38652412184434348c22c37f64"),
        ("stuck:0", "78459bb79475b8bdad8f407c1c9b428aa63b3f2a812470e31f90200d295ecf26"),
        ("biased:0", "4062d5b703c991556d2663c3efa46fa12b3d0e2a454759c779bd2a26ccf8c654"),
        ("ramp:4", "29dd47c52318dd68f273e99af8290af437f88e55947b8553bdc4ca65d225dfbd"),
        ("two:3", "3a3bed0fd0e3e7174d2a8dcfa515c4e295dff4d5c97a60e91e8413e1b17bceee"),
        ("attacker:4660", "ba3c7835e44eb5e04df37807ab3a4179f4e38c7152ae448448d081e7f0c43a74"),
        ("level:0x800", "32628d239eccabaa2186c444bcdbf4a4da58816be3767565321202fcfb58c772"),
    ]
    .into_iter()
    .collect()
}

const ADC_BUF: u32 = 0x848000;
const ADC_SLOTS: i64 = 8;
const ADC_PERIOD: i64 = 500;

fn adc_machine(mode: i64, param: u32) -> Machine {
    let mut m = rbg_machine();
    if mode >= 0 {
        m.reg_write(0xFFE8, 4, mode as u32 | param << 8).unwrap();
    }
    m
}

fn adc_slot(m: &Machine, i: i64) -> u32 {
    let o = (ADC_BUF - SRAM_BASE) as usize + 4 * i as usize;
    u32::from_le_bytes(m.sram[o..o + 4].try_into().unwrap())
}

fn adc_clear(m: &mut Machine) {
    let o = (ADC_BUF - SRAM_BASE) as usize;
    for i in 0..4 * ADC_SLOTS as usize {
        m.sram[o + i] = 0;
    }
}

fn adc_update(m: &mut Machine) {
    m.update_time().unwrap();
}

fn adc_start(m: &mut Machine, vbat: bool) {
    m.analog[0xEB] = if vbat { 0xFF } else { 0x4F };
    m.analog[0xEF] = 0xF0;
    m.analog[0xF1] = 0x0A;
    m.analog[0xFC] = 0x00;
    adc_clear(m);
    m.regs[0xB08] = (ADC_BUF & 0xFF) as u8;
    m.regs[0xB09] = ((ADC_BUF >> 8) & 0xFF) as u8;
    m.regs[0xB0A] = 1;
    m.regs[0xB10] |= 0x04;
    adc_update(m);
}

fn adc_codes(m: &mut Machine, n: usize) -> Vec<u32> {
    let mut out = vec![0; n];
    for x in out.iter_mut() {
        m.cycles += ADC_PERIOD;
        adc_update(m);
        *x = adc_slot(m, (m.adc_done - 1) % ADC_SLOTS);
    }
    out
}

#[test]
fn adc_timing() {
    let code = ADC_VBAT_CODE as u32;
    let mut m = adc_machine(-1, 0);
    adc_start(&mut m, false);
    for i in 0..ADC_SLOTS {
        assert_eq!(adc_slot(&m, i), m.adc_code, "pin input: slot {}", i);
    }
    let mut m = adc_machine(1, 0);
    adc_start(&mut m, true);
    assert_eq!(adc_slot(&m, 0), 0, "VBAT: a conversion at once");
    m.cycles += ADC_PERIOD - 1;
    adc_update(&mut m);
    assert_eq!(adc_slot(&m, 0), 0, "VBAT: a conversion before the first period");
    m.cycles += 1;
    adc_update(&mut m);
    assert!(adc_slot(&m, 0) == code && adc_slot(&m, 1) == 0, "VBAT: the first conversion");
    adc_clear(&mut m);
    m.cycles += ADC_PERIOD * (ADC_SLOTS + 2);
    adc_update(&mut m);
    assert_eq!(m.adc_done, ADC_SLOTS + 3, "VBAT: conversions after 11 periods");
    for i in 0..ADC_SLOTS {
        assert_eq!(adc_slot(&m, i), code, "VBAT: the ring did not go round (slot {})", i);
    }
    m.cycles += ADC_PERIOD * 1000;
    adc_update(&mut m);
    assert_eq!(m.adc_done, ADC_SLOTS + 1003, "VBAT: after a jump of 1000 periods");
    m.regs[0xB10] &= !0x04;
    adc_update(&mut m);
    assert!(!m.adc_on, "VBAT: the DMA off does not end the run");
    adc_clear(&mut m);
    m.regs[0xB10] |= 0x04;
    adc_update(&mut m);
    m.cycles += ADC_PERIOD;
    adc_update(&mut m);
    assert!(adc_slot(&m, 0) == code && adc_slot(&m, 1) == 0, "VBAT: enabled again");
    let mut m = adc_machine(1, 0);
    adc_start(&mut m, true);
    m.analog[0xFC] = 0x20;
    m.cycles += ADC_PERIOD * 4;
    adc_update(&mut m);
    assert_eq!(adc_slot(&m, 0), 0, "VBAT: a conversion while powered down");
    let mut m = adc_machine(7, 0);
    adc_start(&mut m, true);
    m.cycles += ADC_PERIOD * 4;
    adc_update(&mut m);
    assert_eq!(adc_slot(&m, 0), 0, "never: a conversion arrived");
    let mut m = adc_machine(1, 0);
    m.set_cpu_hz(48_000_000);
    adc_start(&mut m, true);
    m.cycles += 2 * ADC_PERIOD - 1;
    adc_update(&mut m);
    assert_eq!(adc_slot(&m, 0), 0, "VBAT at 48 MHz: a conversion before 1000 cycles");
    m.cycles += 1;
    adc_update(&mut m);
    assert_eq!(adc_slot(&m, 0), code, "VBAT at 48 MHz: no conversion after 1000 cycles");
}

fn adc_stats(cs: &[u32]) -> (f64, f64) {
    let mut mean = 0.0;
    for &c in cs {
        mean += c as f64;
    }
    mean /= cs.len() as f64;
    let mut sd = 0.0;
    for &c in cs {
        sd += (c as f64 - mean) * (c as f64 - mean);
    }
    (mean, (sd / cs.len() as f64).sqrt())
}

#[test]
fn adc_modes() {
    let code = ADC_VBAT_CODE as u32;
    let digests = adc_digests();
    for (name, mode, param) in [
        ("noise:0", 0i64, 0u32),
        ("noise:0x0705", 0, 0x0705),
        ("off:0", 1, 0),
        ("stuck:0", 2, 0),
        ("biased:0", 3, 0),
        ("ramp:4", 4, 4),
        ("two:3", 5, 3),
        ("attacker:4660", 6, 4660),
        ("level:0x800", 8, 0x800),
    ] {
        let mut m = adc_machine(mode, param);
        adc_start(&mut m, true);
        let cs = adc_codes(&mut m, 64);
        assert_eq!(digest_u32(&cs), digests[name], "{}: digest", name);
        let mut more = cs.clone();
        more.extend(adc_codes(&mut m, 4096 - 64));
        let (mean, sd) = adc_stats(&more);
        match name {
            "noise:0" => assert!((sd - 2.0).abs() < 0.1 && (mean - code as f64).abs() < 0.15, "noise 2.0 LSB"),
            "noise:0x0705" => assert!(sd > 0.45 && sd < 0.65, "noise 0.5 LSB: {}", sd),
            "level:0x800" => assert!((sd - 2.0).abs() < 0.1 && (mean - 0x800 as f64).abs() < 0.15, "level"),
            "off:0" | "stuck:0" => {
                let want = if mode == 2 { 0x1555 } else { code };
                assert!(more.iter().all(|&v| v == want), "{}", name);
            }
            "biased:0" => {
                let same = more.iter().filter(|&&v| v == code).count();
                let f = same as f64 / more.len() as f64;
                assert!(f > 0.93 && f < 0.99, "biased: {}", f);
            }
            "ramp:4" => {
                for i in 1..2048 {
                    let d = more[i] as i64 - more[i - 1] as i64;
                    assert!(d == 0 || d == 1, "ramp: {} after {}", more[i], more[i - 1]);
                }
                let mut m2 = adc_machine(4, 1);
                adc_start(&mut m2, true);
                let wrap = adc_codes(&mut m2, 0x1FFF - code as usize + 2);
                assert!(wrap[wrap.len() - 2] == 0x1FFF && wrap[wrap.len() - 1] == 1, "ramp at the wrap");
            }
            "two:3" => assert!(more.iter().all(|&v| v == code || v == code + 3), "two"),
            "attacker:4660" => {
                let seen: BTreeSet<u32> = cs.iter().copied().collect();
                assert!(more.iter().all(|&v| v >= code - 32 && v <= code + 31), "attacker range");
                assert!(seen.len() > 30, "attacker: {} codes", seen.len());
            }
            _ => {}
        }
        m.reg_write(0xFFE8, 4, mode as u32 | param << 8).unwrap();
        m.regs[0xB10] &= !0x04;
        adc_update(&mut m);
        adc_start(&mut m, true);
        let again = adc_codes(&mut m, 64);
        assert_eq!(again, cs, "{}: setting it again does not start its sequence afresh", name);
    }
    assert!(adc_machine(-1, 0).reg_write(0xFFE8, 4, 9).is_err(), "an unknown mode is not refused");
}
