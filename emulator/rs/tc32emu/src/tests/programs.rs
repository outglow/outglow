use super::thumb::*;

const CHECK: &[u8] = b"123456789";

fn udiv_body(a: &mut Asm) {
    a.ops(&[mov_imm(2, 0), mov_imm(3, 0), mov_imm(4, 32)])
        .label("udiv")
        .ops(&[lsl_imm(0, 0, 1), alu(ADC, 3, 3), lsl_imm(2, 2, 1), alu(CMP, 3, 1)])
        .bc(CC, "skip")
        .ops(&[sub_rrr(3, 3, 1), add_imm(2, 1)])
        .label("skip")
        .op(sub_imm(4, 1))
        .bc(NE, "udiv");
}

fn udiv() -> Vec<u16> {
    let mut a = Asm::new();
    udiv_body(&mut a);
    a.ops(&[mov_lo(0, 2), mov_lo(1, 3)]).finish()
}

fn sdiv() -> Vec<u16> {
    let mut a = Asm::new();
    a.ops(&[
        asr_imm(5, 0, 31),
        asr_imm(6, 1, 31),
        alu(EOR, 0, 5),
        sub_rrr(0, 0, 5),
        alu(EOR, 1, 6),
        sub_rrr(1, 1, 6),
        alu(EOR, 6, 5),
    ]);
    udiv_body(&mut a);
    a.ops(&[alu(EOR, 2, 6), sub_rrr(2, 2, 6), alu(EOR, 3, 5), sub_rrr(3, 3, 5), mov_lo(0, 2), mov_lo(1, 3)]).finish()
}

#[test]
fn shift_subtract_unsigned_division_gives_quotient_and_remainder() {
    let code = udiv();
    for (n, d) in [(0xFFFF_FFFFu32, 7u32), (1000, 3), (0x8000_0000, 2), (123456, 1000), (0, 9), (6, 6), (5, 9)] {
        let m = run(&code, &[(0, n), (1, d)]);
        assert_eq!((m.r[0], m.r[1]), (n / d, n % d), "{} / {}", n, d);
    }
}

#[test]
fn signed_division_truncates_toward_zero() {
    let code = sdiv();
    for (n, d) in [(-100i32, 7i32), (100, -7), (i32::MIN, 3), (i32::MAX, 2), (-1, -1), (17, 5), (-17, -5), (0, -3)] {
        let m = run(&code, &[(0, n as u32), (1, d as u32)]);
        assert_eq!((m.r[0] as i32, m.r[1] as i32), (n / d, n % d), "{} / {}", n, d);
    }
}

fn wide(lo: u32, hi: u32) -> u64 {
    (hi as u64) << 32 | lo as u64
}

fn split(x: u64) -> [u32; 2] {
    [x as u32, (x >> 32) as u32]
}

#[test]
fn sixty_four_bit_add_and_sub_carry_through_adc_and_sbc() {
    let add = Asm::new().ops(&[add_rrr(0, 0, 2), alu(ADC, 1, 3)]).finish();
    let sub = Asm::new().ops(&[sub_rrr(0, 0, 2), alu(SBC, 1, 3)]).finish();
    for (x, y) in [
        (0xFFFF_FFFFu64, 1u64),
        (0x1122_3344_5566_7788, 0x8877_6655_4433_2211),
        (u64::MAX, 1),
        (0x1_0000_0000, 0x2_0000_0000),
        (0, 1),
    ] {
        let [xl, xh] = split(x);
        let [yl, yh] = split(y);
        let regs = [(0, xl), (1, xh), (2, yl), (3, yh)];
        let m = run(&add, &regs);
        assert_eq!(wide(m.r[0], m.r[1]), x.wrapping_add(y), "0x{:x} + 0x{:x}", x, y);
        let m = run(&sub, &regs);
        assert_eq!(wide(m.r[0], m.r[1]), x.wrapping_sub(y), "0x{:x} - 0x{:x}", x, y);
    }
}

#[test]
fn unsigned_32_by_32_multiply_from_16_bit_halves_gives_the_64_bit_product() {
    let code = Asm::new()
        .ops(&[
            lsl_imm(2, 0, 16),
            lsr_imm(2, 2, 16),
            lsr_imm(3, 0, 16),
            lsl_imm(4, 1, 16),
            lsr_imm(4, 4, 16),
            lsr_imm(5, 1, 16),
            mov_lo(0, 2),
            alu(MUL, 0, 4),
            alu(MUL, 2, 5),
            mov_lo(1, 3),
            alu(MUL, 1, 5),
            alu(MUL, 3, 4),
            mov_imm(6, 0),
            add_rrr(2, 2, 3),
            alu(ADC, 6, 6),
            lsl_imm(6, 6, 16),
            add_rrr(1, 1, 6),
            lsl_imm(3, 2, 16),
            lsr_imm(2, 2, 16),
            add_rrr(0, 0, 3),
            alu(ADC, 1, 2),
        ])
        .finish();
    for (a, b) in [
        (0xFFFF_FFFFu32, 0xFFFF_FFFFu32),
        (0x1234_5678, 0x9ABC_DEF0),
        (0x8000_0000, 2),
        (0xFFFF, 0x10001),
        (0, 0xDEAD_BEEF),
        (0xDEAD_BEEF, 0xCAFE_F00D),
    ] {
        let m = run(&code, &[(0, a), (1, b)]);
        assert_eq!(wide(m.r[0], m.r[1]), a as u64 * b as u64, "0x{:x} * 0x{:x}", a, b);
    }
}

#[test]
fn branch_free_64_bit_left_shift_relies_on_register_shifts_of_32_and_more() {
    let code = Asm::new()
        .ops(&[
            alu(LSL, 1, 2),
            mov_lo(3, 0),
            mov_imm(4, 32),
            sub_rrr(4, 4, 2),
            alu(LSR, 3, 4),
            alu(ORR, 1, 3),
            mov_lo(3, 0),
            mov_lo(4, 2),
            sub_imm(4, 32),
            alu(LSL, 3, 4),
            alu(ORR, 1, 3),
            alu(LSL, 0, 2),
        ])
        .finish();
    for (x, n) in [(0x1122_3344_5566_7788u64, 4u32), (1, 40), (0xFF, 56), (0xDEAD_BEEF_0BAD_F00D, 0), (0x8000_0001, 32), (3, 63)] {
        let [l, h] = split(x);
        let m = run(&code, &[(0, l), (1, h), (2, n)]);
        assert_eq!(wide(m.r[0], m.r[1]), x << n, "0x{:x} << {}", x, n);
    }
}

#[test]
fn branch_free_64_bit_logical_right_shift() {
    let code = Asm::new()
        .ops(&[
            alu(LSR, 0, 2),
            mov_lo(3, 1),
            mov_imm(4, 32),
            sub_rrr(4, 4, 2),
            alu(LSL, 3, 4),
            alu(ORR, 0, 3),
            mov_lo(3, 1),
            mov_lo(4, 2),
            sub_imm(4, 32),
            alu(LSR, 3, 4),
            alu(ORR, 0, 3),
            alu(LSR, 1, 2),
        ])
        .finish();
    for (x, n) in [(u64::MAX, 8u32), (0x8000_0000_0000_0000, 1), (0x8000_0000_0000_0000, 63), (0x1234_5678_9ABC_DEF0, 32), (5, 0)] {
        let [l, h] = split(x);
        let m = run(&code, &[(0, l), (1, h), (2, n)]);
        assert_eq!(wide(m.r[0], m.r[1]), x >> n, "0x{:x} >> {}", x, n);
    }
}

#[test]
fn sixty_four_bit_arithmetic_right_shift_branches_on_the_amount() {
    let code = Asm::new()
        .op(cmp_imm(2, 32))
        .bc(CS, "big")
        .ops(&[alu(LSR, 0, 2), mov_lo(3, 1), mov_imm(4, 32), sub_rrr(4, 4, 2), alu(LSL, 3, 4), alu(ORR, 0, 3), alu(ASR, 1, 2)])
        .b("end")
        .label("big")
        .ops(&[mov_lo(4, 2), sub_imm(4, 32), mov_lo(0, 1), alu(ASR, 0, 4), asr_imm(1, 1, 31)])
        .label("end")
        .finish();
    for (x, n) in [(-16i64, 2u32), (i64::MAX, 8), (i64::MIN, 63), (-0x1_2345_6789, 36), (0x7654_3210_0123_4567, 32), (-3, 0)] {
        let [l, h] = split(x as u64);
        let m = run(&code, &[(0, l), (1, h), (2, n)]);
        assert_eq!(wide(m.r[0], m.r[1]) as i64, x >> n, "{} >> {}", x, n);
    }
}

fn put_bytes(m: &mut crate::Machine, addr: u32, b: &[u8]) {
    let o = (addr - crate::SRAM_BASE) as usize;
    m.sram[o..o + b.len()].copy_from_slice(b);
}

fn crc_msb_first(width: u32, poly: u32, init: u32) -> u32 {
    let sh = 32 - width as u16;
    let code = Asm::new()
        .label("byte")
        .op(cmp_imm(1, 0))
        .bc(EQ, "done")
        .ops(&[ldrb_imm(4, 0, 0), add_imm(0, 1), lsl_imm(4, 4, 24), alu(EOR, 3, 4), mov_imm(5, 8)])
        .label("bit")
        .op(lsl_imm(3, 3, 1))
        .bc(CC, "skip")
        .op(alu(EOR, 3, 2))
        .label("skip")
        .op(sub_imm(5, 1))
        .bc(NE, "bit")
        .op(sub_imm(1, 1))
        .b("byte")
        .label("done")
        .op(lsr_imm(0, 3, sh))
        .finish();
    let mut m = machine();
    put_bytes(&mut m, DATA, CHECK);
    load(&mut m, &code, &[(0, DATA), (1, CHECK.len() as u32), (2, poly << sh), (3, init << sh)]);
    run_to_halt(&mut m, code.len(), 100_000);
    m.r[0]
}

fn crc_reflected(poly: u32, init: u32, xorout: u32) -> u32 {
    let code = Asm::new()
        .label("byte")
        .op(cmp_imm(1, 0))
        .bc(EQ, "done")
        .ops(&[ldrb_imm(4, 0, 0), add_imm(0, 1), alu(EOR, 3, 4), mov_imm(5, 8)])
        .label("bit")
        .op(lsr_imm(3, 3, 1))
        .bc(CC, "skip")
        .op(alu(EOR, 3, 2))
        .label("skip")
        .op(sub_imm(5, 1))
        .bc(NE, "bit")
        .op(sub_imm(1, 1))
        .b("byte")
        .label("done")
        .ops(&[alu(EOR, 3, 6), mov_lo(0, 3)])
        .finish();
    let mut m = machine();
    put_bytes(&mut m, DATA, CHECK);
    load(&mut m, &code, &[(0, DATA), (1, CHECK.len() as u32), (2, poly), (3, init), (6, xorout)]);
    run_to_halt(&mut m, code.len(), 100_000);
    m.r[0]
}

fn crc8_reference(poly: u8, data: &[u8]) -> u8 {
    let mut c = 0u8;
    for &d in data {
        c ^= d;
        for _ in 0..8 {
            c = if c & 0x80 != 0 { (c << 1) ^ poly } else { c << 1 };
        }
    }
    c
}

#[test]
fn bitwise_crc32_loop_gives_the_catalogue_check_value() {
    assert_eq!(crc_reflected(0xEDB8_8320, 0xFFFF_FFFF, 0xFFFF_FFFF), 0xCBF4_3926);
}

#[test]
fn bitwise_crc16_loops_give_the_catalogue_check_values() {
    assert_eq!(crc_msb_first(16, 0x1021, 0xFFFF), 0x29B1, "CRC-16/IBM-3740 (CCITT-FALSE)");
    assert_eq!(crc_msb_first(16, 0x1021, 0), 0x31C3, "CRC-16/XMODEM");
    assert_eq!(crc_reflected(0xA001, 0, 0), 0xBB3D, "CRC-16/ARC");
}

#[test]
fn bitwise_crc8_loops_give_the_catalogue_and_reference_values() {
    assert_eq!(crc_msb_first(8, 0x07, 0), 0xF4, "CRC-8/SMBUS");
    assert_eq!(crc_msb_first(8, 0x6C, 0), crc8_reference(0x6C, CHECK) as u32, "poly 0x6c");
}

fn memmove() -> Vec<u16> {
    Asm::new()
        .op(alu(CMP, 0, 1))
        .bc(HI, "back")
        .label("fwd")
        .op(cmp_imm(2, 0))
        .bc(EQ, "done")
        .ops(&[ldrb_imm(3, 1, 0), strb_imm(3, 0, 0), add_imm(0, 1), add_imm(1, 1), sub_imm(2, 1)])
        .b("fwd")
        .label("back")
        .ops(&[add_rrr(0, 0, 2), add_rrr(1, 1, 2)])
        .label("bloop")
        .op(cmp_imm(2, 0))
        .bc(EQ, "done")
        .ops(&[sub_imm(0, 1), sub_imm(1, 1), ldrb_imm(3, 1, 0), strb_imm(3, 0, 0), sub_imm(2, 1)])
        .b("bloop")
        .label("done")
        .finish()
}

#[test]
fn byte_memmove_handles_both_overlap_directions() {
    let code = memmove();
    for (dst, src, n) in [(4usize, 0usize, 32usize), (10, 0, 50), (1, 0, 16), (0, 4, 32), (0, 7, 100), (180, 0, 64)] {
        let mut m = machine();
        let pattern: Vec<u8> = (0..256).map(|i| (i * 13 + 1) as u8).collect();
        put_bytes(&mut m, DATA, &pattern);
        load(&mut m, &code, &[(0, DATA + dst as u32), (1, DATA + src as u32), (2, n as u32)]);
        run_to_halt(&mut m, code.len(), 100_000);
        let mut want = pattern.clone();
        want.copy_within(src..src + n, dst);
        let o = (DATA - crate::SRAM_BASE) as usize;
        assert_eq!(&m.sram[o..o + 256], &want[..], "memmove dst {} src {} n {}", dst, src, n);
    }
}

#[test]
fn word_copy_with_ldmia_and_stmia_writes_back_both_pointers() {
    let code = Asm::new()
        .label("loop")
        .op(cmp_imm(2, 0))
        .bc(EQ, "done")
        .ops(&[ldmia(1, 0x78), stmia(0, 0x78), sub_imm(2, 1)])
        .b("loop")
        .label("done")
        .finish();
    let mut m = machine();
    let src: Vec<u8> = (0..64).map(|i| (i * 7 + 3) as u8).collect();
    put_bytes(&mut m, DATA, &src);
    put_bytes(&mut m, DATA + 0x100, &[0u8; 72]);
    load(&mut m, &code, &[(0, DATA + 0x100), (1, DATA), (2, 4)]);
    run_to_halt(&mut m, code.len(), 10_000);
    let o = (DATA - crate::SRAM_BASE) as usize + 0x100;
    assert_eq!(&m.sram[o..o + 64], &src[..]);
    assert_eq!(&m.sram[o + 64..o + 72], &[0u8; 8]);
    assert_eq!((m.r[0], m.r[1]), (DATA + 0x140, DATA + 0x40));
}

#[test]
fn array_sum_through_register_offset_loads() {
    let code = Asm::new()
        .ops(&[mov_imm(3, 0), mov_imm(4, 0)])
        .label("loop")
        .op(alu(CMP, 4, 1))
        .bc(CS, "done")
        .ops(&[lsl_imm(5, 4, 2), ldr_reg(6, 0, 5), add_rrr(3, 3, 6), add_imm(4, 1)])
        .b("loop")
        .label("done")
        .op(mov_lo(0, 3))
        .finish();
    for (base, n) in [(10i32, 5u32), (0, 10), (-4, 8), (-1000, 32)] {
        let mut m = machine();
        for i in 0..n {
            set_sram_u32(&mut m, DATA + 4 * i, (base + (i * i) as i32) as u32);
        }
        load(&mut m, &code, &[(0, DATA), (1, n)]);
        run_to_halt(&mut m, code.len(), 10_000);
        let want: i32 = (0..n as i32).map(|i| base + i * i).sum();
        assert_eq!(m.r[0] as i32, want, "base {} n {}", base, n);
    }
}

#[test]
fn bl_and_pop_pc_call_and_return_through_the_stack() {
    let f = 8u32;
    let mut code = vec![0u16; 12];
    let off = f as i32 - (0 + 2);
    code[0] = 0xF000 | ((off >> 11) as u16 & 0x7FF);
    code[1] = 0xF800 | (off as u16 & 0x7FF);
    code[2] = add_imm(0, 1);
    code[3] = HALT;
    code[f as usize] = push(0x100 | 0x10);
    code[f as usize + 1] = mov_imm(4, 0x55);
    code[f as usize + 2] = add_rrr(0, 0, 4);
    code[f as usize + 3] = pop(0x100 | 0x10);
    let mut m = machine();
    load(&mut m, &code, &[(0, 1), (4, 0xABCD)]);
    let halt = CODE + 6;
    for _ in 0..100 {
        if m.r[15] == halt {
            break;
        }
        m.step().unwrap();
    }
    assert_eq!(m.r[15], halt);
    assert_eq!((m.r[0], m.r[4], m.r[13]), (0x57, 0xABCD, STACK));
}
