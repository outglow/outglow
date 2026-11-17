use std::collections::HashMap;

use crate::{isa, Flash, Machine, FLASH_MEM, SRAM_BASE};

pub const CODE: u32 = SRAM_BASE;
pub const DATA: u32 = SRAM_BASE + 0x1000;
pub const STACK: u32 = SRAM_BASE + 0x4000;

pub const EQ: u16 = 0;
pub const NE: u16 = 1;
pub const CS: u16 = 2;
pub const CC: u16 = 3;
pub const HI: u16 = 8;

pub fn machine() -> Machine {
    let mut fl = Flash::new(FLASH_MEM);
    fl.mem[8] = 0x4B;
    Machine::new(fl, -1, 1).unwrap()
}

pub fn put(m: &mut Machine, addr: u32, thumbs: &[u16]) {
    let off = (addr - SRAM_BASE) as usize;
    for (i, hw) in thumbs.iter().enumerate() {
        let tc = isa::to_tc32(*hw);
        m.sram[off + 2 * i] = tc as u8;
        m.sram[off + 2 * i + 1] = (tc >> 8) as u8;
    }
}

pub fn sram_u32(m: &Machine, addr: u32) -> u32 {
    let o = (addr - SRAM_BASE) as usize;
    u32::from_le_bytes(m.sram[o..o + 4].try_into().unwrap())
}

pub fn set_sram_u32(m: &mut Machine, addr: u32, v: u32) {
    let o = (addr - SRAM_BASE) as usize;
    m.sram[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

pub fn lsl_imm(rd: u16, rm: u16, imm: u16) -> u16 {
    (imm & 31) << 6 | rm << 3 | rd
}
pub fn lsr_imm(rd: u16, rm: u16, imm: u16) -> u16 {
    0x0800 | (imm & 31) << 6 | rm << 3 | rd
}
pub fn asr_imm(rd: u16, rm: u16, imm: u16) -> u16 {
    0x1000 | (imm & 31) << 6 | rm << 3 | rd
}
pub fn add_rrr(rd: u16, rn: u16, rm: u16) -> u16 {
    0x1800 | rm << 6 | rn << 3 | rd
}
pub fn sub_rrr(rd: u16, rn: u16, rm: u16) -> u16 {
    0x1A00 | rm << 6 | rn << 3 | rd
}
pub fn add_imm3(rd: u16, rn: u16, imm: u16) -> u16 {
    0x1C00 | imm << 6 | rn << 3 | rd
}
pub fn sub_imm3(rd: u16, rn: u16, imm: u16) -> u16 {
    0x1E00 | imm << 6 | rn << 3 | rd
}
pub fn mov_imm(rd: u16, imm: u16) -> u16 {
    0x2000 | rd << 8 | imm
}
pub fn cmp_imm(rd: u16, imm: u16) -> u16 {
    0x2800 | rd << 8 | imm
}
pub fn add_imm(rd: u16, imm: u16) -> u16 {
    0x3000 | rd << 8 | imm
}
pub fn sub_imm(rd: u16, imm: u16) -> u16 {
    0x3800 | rd << 8 | imm
}
pub fn alu(op: u16, rd: u16, rs: u16) -> u16 {
    0x4000 | op << 6 | rs << 3 | rd
}
pub const AND: u16 = 0;
pub const EOR: u16 = 1;
pub const LSL: u16 = 2;
pub const LSR: u16 = 3;
pub const ASR: u16 = 4;
pub const ADC: u16 = 5;
pub const SBC: u16 = 6;
pub const ROR: u16 = 7;
pub const TST: u16 = 8;
pub const NEG: u16 = 9;
pub const CMP: u16 = 10;
pub const CMN: u16 = 11;
pub const ORR: u16 = 12;
pub const MUL: u16 = 13;
pub const BIC: u16 = 14;
pub const MVN: u16 = 15;
pub fn mov_lo(rd: u16, rm: u16) -> u16 {
    lsl_imm(rd, rm, 0)
}
pub fn hi_add(rd: u16, rm: u16) -> u16 {
    0x4400 | (rd & 8) << 4 | rm << 3 | (rd & 7)
}
pub fn hi_cmp(rd: u16, rm: u16) -> u16 {
    0x4500 | (rd & 8) << 4 | rm << 3 | (rd & 7)
}
pub fn hi_mov(rd: u16, rm: u16) -> u16 {
    0x4600 | (rd & 8) << 4 | rm << 3 | (rd & 7)
}
pub fn ldr_reg(rd: u16, rb: u16, ro: u16) -> u16 {
    0x5800 | ro << 6 | rb << 3 | rd
}
pub fn str_reg(rd: u16, rb: u16, ro: u16) -> u16 {
    0x5000 | ro << 6 | rb << 3 | rd
}
pub fn ldrb_reg(rd: u16, rb: u16, ro: u16) -> u16 {
    0x5C00 | ro << 6 | rb << 3 | rd
}
pub fn strb_reg(rd: u16, rb: u16, ro: u16) -> u16 {
    0x5400 | ro << 6 | rb << 3 | rd
}
pub fn strh_reg(rd: u16, rb: u16, ro: u16) -> u16 {
    0x5200 | ro << 6 | rb << 3 | rd
}
pub fn ldrh_reg(rd: u16, rb: u16, ro: u16) -> u16 {
    0x5A00 | ro << 6 | rb << 3 | rd
}
pub fn ldrsb_reg(rd: u16, rb: u16, ro: u16) -> u16 {
    0x5600 | ro << 6 | rb << 3 | rd
}
pub fn ldrsh_reg(rd: u16, rb: u16, ro: u16) -> u16 {
    0x5E00 | ro << 6 | rb << 3 | rd
}
pub fn str_imm(rd: u16, rb: u16, words: u16) -> u16 {
    0x6000 | words << 6 | rb << 3 | rd
}
pub fn ldr_imm(rd: u16, rb: u16, words: u16) -> u16 {
    0x6800 | words << 6 | rb << 3 | rd
}
pub fn strb_imm(rd: u16, rb: u16, imm: u16) -> u16 {
    0x7000 | imm << 6 | rb << 3 | rd
}
pub fn ldrb_imm(rd: u16, rb: u16, imm: u16) -> u16 {
    0x7800 | imm << 6 | rb << 3 | rd
}
pub fn strh_imm(rd: u16, rb: u16, halves: u16) -> u16 {
    0x8000 | halves << 6 | rb << 3 | rd
}
pub fn ldrh_imm(rd: u16, rb: u16, halves: u16) -> u16 {
    0x8800 | halves << 6 | rb << 3 | rd
}
pub fn str_sp(rd: u16, words: u16) -> u16 {
    0x9000 | rd << 8 | words
}
pub fn ldr_sp(rd: u16, words: u16) -> u16 {
    0x9800 | rd << 8 | words
}
pub fn ldr_pc(rd: u16, words: u16) -> u16 {
    0x4800 | rd << 8 | words
}
pub fn push(list: u16) -> u16 {
    0xB400 | list
}
pub fn pop(list: u16) -> u16 {
    0xBC00 | list
}
pub fn stmia(rb: u16, list: u16) -> u16 {
    0xC000 | rb << 8 | list
}
pub fn ldmia(rb: u16, list: u16) -> u16 {
    0xC800 | rb << 8 | list
}
pub const HALT: u16 = 0xE7FE;

enum Fix {
    Cond(u16),
    Always,
}

#[derive(Default)]
pub struct Asm {
    code: Vec<u16>,
    labels: HashMap<&'static str, usize>,
    fixes: Vec<(usize, &'static str, Fix)>,
}

impl Asm {
    pub fn new() -> Asm {
        Asm::default()
    }
    pub fn op(&mut self, hw: u16) -> &mut Asm {
        self.code.push(hw);
        self
    }
    pub fn ops(&mut self, hws: &[u16]) -> &mut Asm {
        self.code.extend_from_slice(hws);
        self
    }
    pub fn label(&mut self, l: &'static str) -> &mut Asm {
        self.labels.insert(l, self.code.len());
        self
    }
    pub fn bc(&mut self, cond: u16, l: &'static str) -> &mut Asm {
        self.fixes.push((self.code.len(), l, Fix::Cond(cond)));
        self.code.push(0);
        self
    }
    pub fn b(&mut self, l: &'static str) -> &mut Asm {
        self.fixes.push((self.code.len(), l, Fix::Always));
        self.code.push(0);
        self
    }
    pub fn finish(&mut self) -> Vec<u16> {
        let mut code = self.code.clone();
        for (at, l, fix) in &self.fixes {
            let target = self.labels[l] as i64;
            let off = target - (*at as i64 + 2);
            code[*at] = match fix {
                Fix::Cond(c) => {
                    assert!((-128..128).contains(&off), "branch to {} out of range", l);
                    0xD000 | c << 8 | (off as u16 & 0xFF)
                }
                Fix::Always => 0xE000 | (off as u16 & 0x7FF),
            };
        }
        code.push(HALT);
        code
    }
}

pub fn load(m: &mut Machine, code: &[u16], regs: &[(usize, u32)]) {
    put(m, CODE, code);
    m.r[15] = CODE;
    m.r[13] = STACK;
    for (r, v) in regs {
        m.r[*r] = *v;
    }
}

pub fn run_to_halt(m: &mut Machine, code_len: usize, limit: usize) {
    let halt = CODE + 2 * (code_len as u32 - 1);
    for _ in 0..limit {
        if m.r[15] == halt {
            return;
        }
        m.step().unwrap();
    }
    panic!("no halt after {} steps, pc 0x{:x}", limit, m.r[15]);
}

pub fn run(code: &[u16], regs: &[(usize, u32)]) -> Machine {
    let mut m = machine();
    load(&mut m, code, regs);
    run_to_halt(&mut m, code.len(), 1_000_000);
    m
}

pub fn exec1(hw: u16, setup: impl FnOnce(&mut Machine)) -> Machine {
    let mut m = machine();
    put(&mut m, CODE, &[hw]);
    m.r[15] = CODE;
    m.r[13] = STACK;
    setup(&mut m);
    m.step().unwrap();
    m
}
