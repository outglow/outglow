use super::thumb::*;

const WORD: u32 = 0x8090_A0B0;

fn load_one(hw: u16, off: u32) -> u32 {
    let m = {
        let mut m = machine();
        set_sram_u32(&mut m, DATA, WORD);
        set_sram_u32(&mut m, DATA + 4, 0);
        put(&mut m, CODE, &[hw]);
        m.r[15] = CODE;
        m.r[1] = DATA;
        m.r[2] = off;
        m.step().unwrap();
        m
    };
    m.r[0]
}

#[test]
fn byte_loads_zero_or_sign_extend_each_byte_of_a_word() {
    for off in 0..4u32 {
        let byte = (WORD >> (8 * off)) & 0xFF;
        assert_eq!(load_one(ldrb_reg(0, 1, 2), off), byte, "LDRB offset {}", off);
        assert_eq!(load_one(ldrsb_reg(0, 1, 2), off), byte as u8 as i8 as i32 as u32, "LDRSB offset {}", off);
        assert_eq!(load_one(ldrb_imm(0, 1, off as u16), 0), byte, "LDRB #{}", off);
    }
}

#[test]
fn halfword_loads_zero_or_sign_extend_each_half_of_a_word() {
    for off in [0u32, 2] {
        let half = (WORD >> (8 * off)) & 0xFFFF;
        assert_eq!(load_one(ldrh_reg(0, 1, 2), off), half, "LDRH offset {}", off);
        assert_eq!(load_one(ldrsh_reg(0, 1, 2), off), half as u16 as i16 as i32 as u32, "LDRSH offset {}", off);
        assert_eq!(load_one(ldrh_imm(0, 1, off as u16 / 2), 0), half, "LDRH #{}", off);
    }
    assert_eq!(load_one(ldrsh_reg(0, 1, 2), 4), 0, "LDRSH of a positive half");
}

#[test]
fn word_loads_by_register_and_immediate_offset() {
    assert_eq!(load_one(ldr_reg(0, 1, 2), 0), WORD);
    assert_eq!(load_one(ldr_imm(0, 1, 1), 0), 0);
    assert_eq!(load_one(ldr_imm(0, 1, 0), 0), WORD);
}

#[test]
fn narrow_stores_change_only_their_bytes() {
    for (hw, off, val, want) in [
        (strb_reg(0, 1, 2), 1u32, 0x1234_56CDu32, 0x1122_CD44u32),
        (strb_imm(0, 1, 3), 0, 0xEE, 0xEE22_3344),
        (strh_reg(0, 1, 2), 2, 0xABCD_BEEF, 0xBEEF_3344),
        (strh_imm(0, 1, 0), 0, 0x9876, 0x1122_9876),
        (str_reg(0, 1, 2), 0, 0xCAFE_F00D, 0xCAFE_F00D),
    ] {
        let mut m = machine();
        set_sram_u32(&mut m, DATA - 4, 0);
        set_sram_u32(&mut m, DATA, 0x1122_3344);
        set_sram_u32(&mut m, DATA + 4, 0);
        put(&mut m, CODE, &[hw]);
        m.r[15] = CODE;
        m.r[0] = val;
        m.r[1] = DATA;
        m.r[2] = off;
        m.step().unwrap();
        assert_eq!(sram_u32(&m, DATA), want, "store 0x{:04x}", hw);
        assert_eq!((sram_u32(&m, DATA - 4), sram_u32(&m, DATA + 4)), (0, 0), "neighbours of 0x{:04x}", hw);
    }
}

#[test]
fn push_stores_ascending_below_sp_and_pop_restores_in_order() {
    let mut m = machine();
    let code = [push(0x0F | 0x100), mov_imm(0, 0), mov_imm(1, 0), mov_imm(2, 0), mov_imm(3, 0), pop(0x0F), pop(0x10)];
    load(&mut m, &code, &[(0, 0xA0), (1, 0xA1), (2, 0xA2), (3, 0xA3), (14, 0xBEEF)]);
    m.step().unwrap();
    assert_eq!(m.r[13], STACK - 20);
    let words: Vec<u32> = (0..5).map(|i| sram_u32(&m, STACK - 20 + 4 * i)).collect();
    assert_eq!(words, vec![0xA0, 0xA1, 0xA2, 0xA3, 0xBEEF]);
    for _ in 0..6 {
        m.step().unwrap();
    }
    assert_eq!((m.r[0], m.r[1], m.r[2], m.r[3], m.r[4], m.r[13]), (0xA0, 0xA1, 0xA2, 0xA3, 0xBEEF, STACK));
}

#[test]
fn sp_relative_store_and_load() {
    let mut m = machine();
    let code = [str_sp(0, 3), mov_imm(0, 0), ldr_sp(1, 3)];
    load(&mut m, &code, &[(0, 0x1357_9BDF)]);
    for _ in 0..3 {
        m.step().unwrap();
    }
    assert_eq!(sram_u32(&m, STACK + 12), 0x1357_9BDF);
    assert_eq!(m.r[1], 0x1357_9BDF);
}

#[test]
fn pc_relative_load_uses_the_word_aligned_pc_plus_4() {
    for at in [0u32, 2] {
        let mut m = machine();
        let base = CODE + at;
        let lit = (base + 4) & !3;
        set_sram_u32(&mut m, lit + 8, 0x0BAD_CAFE);
        put(&mut m, base, &[ldr_pc(0, 2)]);
        m.r[15] = base;
        m.step().unwrap();
        assert_eq!(m.r[0], 0x0BAD_CAFE, "LDR at pc offset {}", at);
    }
}

#[test]
fn stmia_writes_back_past_the_last_word() {
    let mut m = machine();
    for i in 0..4 {
        set_sram_u32(&mut m, DATA + 4 * i, 0);
    }
    put(&mut m, CODE, &[stmia(0, 0x0E)]);
    m.r[15] = CODE;
    m.r[0] = DATA;
    m.r[1] = 1;
    m.r[2] = 2;
    m.r[3] = 3;
    m.step().unwrap();
    let words: Vec<u32> = (0..4).map(|i| sram_u32(&m, DATA + 4 * i)).collect();
    assert_eq!(words, vec![1, 2, 3, 0]);
    assert_eq!(m.r[0], DATA + 12);
}
