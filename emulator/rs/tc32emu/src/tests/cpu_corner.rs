use super::thumb::*;
use crate::Machine;

fn flags(m: &Machine) -> (u32, u32, u32, u32) {
    (m.n, m.z, m.c, m.v)
}

#[test]
fn lsr_immediate_zero_means_shift_by_32() {
    let m = exec1(lsr_imm(0, 1, 0), |m| m.r[1] = 0x8000_0001);
    assert_eq!((m.r[0], m.c), (0, 1));
}

#[test]
fn asr_immediate_zero_means_shift_by_32() {
    let m = exec1(asr_imm(0, 1, 0), |m| m.r[1] = 0x8000_0000);
    assert_eq!((m.r[0], m.c), (0xFFFF_FFFF, 1));
    let m = exec1(asr_imm(0, 1, 0), |m| m.r[1] = 0x7FFF_FFFF);
    assert_eq!((m.r[0], m.c, m.z), (0, 0, 1));
}

#[test]
fn lsl_register_by_32_gives_zero_and_carries_bit_0() {
    let m = exec1(alu(LSL, 0, 1), |m| {
        m.r[0] = 1;
        m.r[1] = 32;
    });
    assert_eq!((m.r[0], m.c), (0, 1));
}

#[test]
fn lsl_register_by_33_gives_zero_and_clears_carry() {
    let m = exec1(alu(LSL, 0, 1), |m| {
        m.r[0] = 1;
        m.r[1] = 33;
        m.c = 1;
    });
    assert_eq!((m.r[0], m.c), (0, 0));
}

#[test]
fn lsr_register_by_32_gives_zero_and_carries_bit_31() {
    let m = exec1(alu(LSR, 0, 1), |m| {
        m.r[0] = 0x8000_0000;
        m.r[1] = 32;
    });
    assert_eq!((m.r[0], m.c), (0, 1));
}

#[test]
fn asr_register_by_40_fills_with_the_sign() {
    let m = exec1(alu(ASR, 0, 1), |m| {
        m.r[0] = 0x8000_0000;
        m.r[1] = 40;
    });
    assert_eq!((m.r[0], m.c), (0xFFFF_FFFF, 1));
}

#[test]
fn ror_register_by_32_keeps_the_value_and_carries_bit_31() {
    let m = exec1(alu(ROR, 0, 1), |m| {
        m.r[0] = 0x8000_0001;
        m.r[1] = 32;
    });
    assert_eq!((m.r[0], m.c), (0x8000_0001, 1));
}

#[test]
fn register_shift_uses_only_the_bottom_byte_of_the_amount() {
    let m = exec1(alu(LSL, 0, 1), |m| {
        m.r[0] = 1;
        m.r[1] = 0x104;
    });
    assert_eq!(m.r[0], 0x10);
}

#[test]
fn register_shift_by_zero_keeps_the_carry() {
    for c in [0, 1] {
        let m = exec1(alu(LSR, 0, 1), |m| {
            m.r[0] = 0x8000_0001;
            m.r[1] = 0x100;
            m.c = c;
        });
        assert_eq!((m.r[0], m.c), (0x8000_0001, c));
    }
}

#[test]
fn adc_wraps_to_zero_with_carry_out() {
    let m = exec1(alu(ADC, 0, 1), |m| {
        m.r[0] = 0xFFFF_FFFF;
        m.r[1] = 0;
        m.c = 1;
    });
    assert_eq!((m.r[0], m.c, m.z), (0, 1, 1));
}

#[test]
fn sbc_with_borrow_in_wraps_below_zero() {
    let m = exec1(alu(SBC, 0, 1), |m| {
        m.r[0] = 0;
        m.r[1] = 0;
        m.c = 0;
    });
    assert_eq!((m.r[0], m.c), (0xFFFF_FFFF, 0));
}

#[test]
fn mul_keeps_the_low_32_bits_of_the_product() {
    let m = exec1(alu(MUL, 0, 1), |m| {
        m.r[0] = 0x10000;
        m.r[1] = 0x10000;
    });
    assert_eq!((m.r[0], m.z, m.n), (0, 1, 0));
}

#[test]
fn cmp_int_min_with_one_sets_overflow() {
    let m = exec1(alu(CMP, 0, 1), |m| {
        m.r[0] = 0x8000_0000;
        m.r[1] = 1;
    });
    assert_eq!(flags(&m), (0, 0, 1, 1));
    assert_eq!(m.r[0], 0x8000_0000);
}

#[test]
fn ldmia_with_the_base_in_the_list_loads_the_base_without_writeback() {
    let mut m = machine();
    for (i, w) in [0xAAAA_0000u32, 0xBBBB_1111, 0xCCCC_2222].iter().enumerate() {
        set_sram_u32(&mut m, DATA + 4 * i as u32, *w);
    }
    put(&mut m, CODE, &[ldmia(0, 0x03)]);
    m.r[15] = CODE;
    m.r[0] = DATA;
    m.step().unwrap();
    assert_eq!((m.r[0], m.r[1]), (0xAAAA_0000, 0xBBBB_1111));
}

#[test]
fn unaligned_ldr_rotates_the_aligned_word() {
    for (off, want) in [(1u32, 0x4411_2233u32), (2, 0x3344_1122), (3, 0x2233_4411)] {
        let mut m = machine();
        set_sram_u32(&mut m, DATA, 0x1122_3344);
        put(&mut m, CODE, &[ldr_imm(0, 1, 0)]);
        m.r[15] = CODE;
        m.r[1] = DATA + off;
        m.step().unwrap();
        assert_eq!(m.r[0], want, "offset {}", off);
    }
}

#[test]
fn unaligned_str_writes_the_aligned_word() {
    let mut m = machine();
    for a in [DATA + 0xFC, DATA + 0x100, DATA + 0x104] {
        set_sram_u32(&mut m, a, 0);
    }
    put(&mut m, CODE, &[str_imm(0, 1, 0)]);
    m.r[15] = CODE;
    m.r[0] = 0xAABB_CCDD;
    m.r[1] = DATA + 0x102;
    m.step().unwrap();
    assert_eq!(sram_u32(&m, DATA + 0x100), 0xAABB_CCDD);
    assert_eq!((sram_u32(&m, DATA + 0xFC), sram_u32(&m, DATA + 0x104)), (0, 0));
}
