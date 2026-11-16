use crate::error::{emu_err, Error};
use crate::isa::to_thumb;
use crate::machine::{Machine, ICACHE_LINES, ICACHE_LINE_SHIFT};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stepped {
    pub pc: u32,
    pub hw: u16,
    pub ran: bool,
}

#[inline]
fn sext(v: u32, bits: u32) -> u32 {
    if v & (1 << (bits - 1)) != 0 {
        v | (u32::MAX << bits)
    } else {
        v
    }
}

impl Machine {
    #[inline]
    fn nz(&mut self, x: u32) {
        self.n = x >> 31;
        self.z = (x == 0) as u32;
    }

    #[inline]
    fn add(&mut self, a: u32, b: u32, carry: u32) -> u32 {
        let s = a as u64 + b as u64 + carry as u64;
        let r = s as u32;
        self.nz(r);
        self.c = (s > 0xFFFFFFFF) as u32;
        self.v = ((!(a ^ b)) & (a ^ r) & 0x80000000 != 0) as u32;
        r
    }

    #[inline]
    fn sub(&mut self, a: u32, b: u32, borrow_in: u32) -> u32 {
        self.add(a, !b, borrow_in)
    }

    #[inline(always)]
    fn cond(&self, cc: u32) -> bool {
        let (n, z, c, v) = (self.n != 0, self.z != 0, self.c != 0, self.v != 0);
        match cc {
            0 => z,
            1 => !z,
            2 => c,
            3 => !c,
            4 => n,
            5 => !n,
            6 => v,
            7 => !v,
            8 => c && !z,
            9 => !c || z,
            10 => n == v,
            11 => n != v,
            12 => !z && n == v,
            13 => z || n != v,
            14 => true,
            _ => false,
        }
    }

    #[inline(always)]
    fn chk(&mut self, a: u32, size: usize) -> Result<u32, Error> {
        if a % size as u32 != 0 {
            if let Some(h) = self.chk_hook.clone() {
                if let Some(addr) = h(self, a, size) {
                    return Ok(addr);
                }
            }
            return Ok(a & !(size as u32 - 1));
        }
        Ok(a)
    }

    #[inline]
    fn setreg(&mut self, rd: usize, val: u32) {
        if rd == 15 {
            self.r[15] = val & !1;
        } else {
            self.r[rd] = val;
        }
    }

    pub fn step(&mut self) -> Result<Stepped, Error> {
        if self.stalled {
            let to = self.cycles + (self.cpu_hz / 1000).max(1);
            self.stall_until(to)?;
            return Ok(Stepped { pc: 0, hw: 0, ran: false });
        }
        self.step_inline()
    }

    #[inline(always)]
    pub(crate) fn step_inline(&mut self) -> Result<Stepped, Error> {
        let (pc, hw, ran) = self.step_inner()?;
        if ran {
            if self.cycle_accurate {
                self.cycles += self.instr_extra_cycles(pc, hw);
            }
            if let Some(h) = self.trace_step.clone() {
                h(self, pc, hw);
            }
        }
        Ok(Stepped { pc, hw, ran })
    }

    fn instr_extra_cycles(&self, pc: u32, hw: u16) -> i64 {
        let taken = self.r[15] != pc.wrapping_add(2);
        let t = to_thumb(hw) as u32;
        let top = t >> 11;
        if top == 0x09 || (top == 0x0B && ((t >> 9) & 7) >= 3) || top == 0x0D || top == 0x0F || top == 0x11 || top == 0x13 {
            return 2;
        }
        if top == 0x0A || top == 0x0C || top == 0x0E || top == 0x10 || top == 0x12 {
            return 1;
        }
        if top == 0x18 || top == 0x19 || top == 0x16 || top == 0x17 {
            let mut n = 0i64;
            for i in 0..8 {
                if t & (1 << i) != 0 {
                    n += 1;
                }
            }
            if t & 0x0100 != 0 && (top == 0x16 || top == 0x17) {
                n += 1;
            }
            if taken {
                return n + 2;
            }
            return n + 1;
        }
        if t >> 10 == 0x10 && ((t >> 6) & 15) == 13 {
            return 3;
        }
        if taken {
            return 2;
        }
        0
    }

    #[inline(always)]
    fn step_inner(&mut self) -> Result<(u32, u16, bool), Error> {
        if self.asleep.is_some() || self.stalled {
            return Ok((0, 0, false));
        }
        let pc = self.r[15];
        if !self.hooks.is_empty() {
            if let Some(hook) = self.hooks.get(&pc).cloned() {
                hook(self)?;
                if self.r[15] != pc {
                    return Ok((pc, 0, false));
                }
            }
        }
        let v = self.fetch(pc)?;
        let hw = v as u16;
        self.r[15] = pc.wrapping_add(2);
        self.cycles += self.cpi;
        self.exec(pc, hw)?;
        Ok((pc, hw, true))
    }

    #[cfg_attr(debug_assertions, inline(always))]
    fn exec(&mut self, pc: u32, hw: u16) -> Result<(), Error> {
        if hw & 0xFFE0 == 0x6BC0 {
            let (op, rd) = ((hw >> 3) & 3, (hw & 7) as usize);
            match op {
                0 => return self.set_cpsr(self.r[rd]),
                1 => self.r[rd] = self.cpsr(),
                2 => {
                    let v = self.r[rd];
                    self.set_spsr_cur(v);
                }
                _ => self.r[rd] = self.spsr_cur(),
            }
            return Ok(());
        }
        if hw & 0xFE00 == 0x6800 {
            if hw & 0x100 == 0 {
                return Err(emu_err(format!(
                    "treti without pc (0x{:04x}) at {}: not modelled",
                    hw,
                    self.symbolize(pc)
                )));
            }
            let mut sp = self.r[13];
            for i in 0..8 {
                if hw & (1 << i) != 0 {
                    match self.read(sp, 4) {
                        Ok(v) => self.r[i] = v,
                        Err(e) => {
                            self.r[i] = 0;
                            return Err(e);
                        }
                    }
                    sp = sp.wrapping_add(4);
                }
            }
            let newpc = self.read(sp, 4)?;
            sp = sp.wrapping_add(4);
            self.r[13] = sp;
            let s = self.spsr_cur();
            self.set_cpsr(s)?;
            self.r[15] = newpc & !1;
            if let Some(h) = self.on_treti.clone() {
                h(self);
            }
            return Ok(());
        }
        if hw & 0xFF00 == 0xCF00 {
            return Err(emu_err(format!("tserv {} at {}", hw & 0xff, self.symbolize(pc))));
        }

        let t = to_thumb(hw) as u32;
        let top = t >> 11;
        match top {
        0..=2 => {
            let (mut imm, rm, rd) = ((t >> 6) & 31, ((t >> 3) & 7) as usize, (t & 7) as usize);
            let mut x = self.r[rm];
            match top {
                0 => {
                    if imm != 0 {
                        self.c = (x >> (32 - imm)) & 1;
                        x <<= imm;
                    }
                }
                1 => {
                    if imm == 0 {
                        imm = 32;
                    }
                    self.c = (x >> (imm - 1)) & 1;
                    x = if imm < 32 { x >> imm } else { 0 };
                }
                _ => {
                    if imm == 0 {
                        imm = 32;
                    }
                    if imm < 32 {
                        self.c = (x >> (imm - 1)) & 1;
                    } else {
                        self.c = x >> 31;
                    }
                    let sh = imm.min(31);
                    x = ((x as i32) >> sh) as u32;
                }
            }
            self.r[rd] = x;
            self.nz(x);
        } 3 => {
            let (rd, rn, f) = ((t & 7) as usize, ((t >> 3) & 7) as usize, (t >> 6) & 7);
            let b = if t & 0x400 != 0 { f } else { self.r[f as usize] };
            let a = self.r[rn];
            self.r[rd] = if t & 0x200 != 0 { self.sub(a, b, 1) } else { self.add(a, b, 0) };
        } 4..=7 => {
            let (rd, imm) = (((t >> 8) & 7) as usize, t & 0xFF);
            match top - 4 {
                0 => {
                    self.r[rd] = imm;
                    self.nz(imm);
                }
                1 => {
                    let a = self.r[rd];
                    self.sub(a, imm, 1);
                }
                2 => {
                    let a = self.r[rd];
                    self.r[rd] = self.add(a, imm, 0);
                }
                _ => {
                    let a = self.r[rd];
                    self.r[rd] = self.sub(a, imm, 1);
                }
            }
        } 8 if t >> 10 == 0x10 => {
            self.alu((t >> 6) & 15, (t & 7) as usize, ((t >> 3) & 7) as usize);
        } 8 => {
            let op = (t >> 8) & 3;
            let rd = ((t & 7) | ((t >> 4) & 8)) as usize;
            let rm = ((t >> 3) & 15) as usize;
            if op == 3 && t & 0x87 != 0 {
                return Err(emu_err(format!(
                    "bx with bit 7 or bits 2:0 set (0x{:04x}) at {}: UNPREDICTABLE in ARMv4T",
                    hw,
                    self.symbolize(pc)
                )));
            }
            if op < 3 && rd < 8 && rm < 8 {
                return Err(emu_err(format!(
                    "high-register add/cmp/mov with two low registers (0x{:04x}) at {}: UNPREDICTABLE in ARMv4T",
                    hw,
                    self.symbolize(pc)
                )));
            }
            let reads_pc = rm == 15 || (rd == 15 && op <= 1);
            if reads_pc && pc.wrapping_add(4) & 2 != 0 {
                return Err(emu_err(format!(
                    "PC read by a high-register add/cmp/mov at 0x{:x}, a halfword address: unknown on TC32",
                    pc
                )));
            }
            let vm = if rm == 15 { pc.wrapping_add(4) } else { self.r[rm] };
            match op {
                0 => {
                    let vd = if rd == 15 { pc.wrapping_add(4) } else { self.r[rd] };
                    self.setreg(rd, vd.wrapping_add(vm));
                }
                1 => {
                    let vd = if rd == 15 { pc.wrapping_add(4) } else { self.r[rd] };
                    self.sub(vd, vm, 1);
                }
                2 => self.setreg(rd, vm),
                _ => self.r[15] = vm & !1,
            }
        } 9 => {
            let a = (pc.wrapping_add(4) & !3).wrapping_add((t & 0xFF) * 4);
            let v = self.read(a, 4)?;
            self.r[((t >> 8) & 7) as usize] = v;
        } 0x0A | 0x0B => {
            let (op, ro, rb, rd) = ((t >> 9) & 7, ((t >> 6) & 7) as usize, ((t >> 3) & 7) as usize, (t & 7) as usize);
            let a = self.r[rb].wrapping_add(self.r[ro]);
            match op {
                0 => return self.write_chk(a, 4, self.r[rd]),
                1 => return self.write_chk(a, 2, self.r[rd]),
                2 => return self.write(a, 1, self.r[rd]),
                3 => {
                    let x = self.read(a, 1)?;
                    self.r[rd] = sext(x, 8);
                }
                4 => return self.load(rd, |m| m.read_chk(a, 4)),
                5 => return self.load(rd, |m| m.read_chk(a, 2)),
                6 => return self.load(rd, |m| m.read(a, 1)),
                _ => {
                    let x = self.read_chk(a, 2)?;
                    self.r[rd] = sext(x, 16);
                }
            }
        } 0x0C | 0x0D => {
            let (imm, rb, rd) = ((t >> 6) & 31, ((t >> 3) & 7) as usize, (t & 7) as usize);
            let a = self.r[rb].wrapping_add(imm * 4);
            if top == 0x0D {
                return self.load(rd, |m| m.read_chk(a, 4));
            }
            return self.write_chk(a, 4, self.r[rd]);
        } 0x0E | 0x0F => {
            let (imm, rb, rd) = ((t >> 6) & 31, ((t >> 3) & 7) as usize, (t & 7) as usize);
            let a = self.r[rb].wrapping_add(imm);
            if top == 0x0F {
                return self.load(rd, |m| m.read(a, 1));
            }
            return self.write(a, 1, self.r[rd]);
        } 0x10 | 0x11 => {
            let (imm, rb, rd) = ((t >> 6) & 31, ((t >> 3) & 7) as usize, (t & 7) as usize);
            let a = self.r[rb].wrapping_add(imm * 2);
            if top == 0x11 {
                return self.load(rd, |m| m.read_chk(a, 2));
            }
            return self.write_chk(a, 2, self.r[rd]);
        } 0x12 | 0x13 => {
            let (rd, a) = (((t >> 8) & 7) as usize, self.r[13].wrapping_add((t & 0xFF) * 4));
            if top == 0x13 {
                return self.load(rd, |m| m.read_chk(a, 4));
            }
            return self.write_chk(a, 4, self.r[rd]);
        } 0x14 | 0x15 => {
            let rd = ((t >> 8) & 7) as usize;
            let base = if top == 0x15 { self.r[13] } else { pc.wrapping_add(4) & !3 };
            self.r[rd] = base.wrapping_add((t & 0xFF) * 4);
        } 0x16 | 0x17 => {
            if t & 0xFF00 == 0xB000 {
                let imm = (t & 0x7F) * 4;
                if t & 0x80 != 0 {
                    self.r[13] = self.r[13].wrapping_sub(imm);
                } else {
                    self.r[13] = self.r[13].wrapping_add(imm);
                }
            } else if t & 0xF600 == 0xB400 {
                if t & 0x1FF == 0 {
                    return Err(emu_err(format!(
                        "push/pop of no registers (0x{:04x}) at {}: UNPREDICTABLE in ARMv4T",
                        hw,
                        self.symbolize(pc)
                    )));
                }
                if t & 0x0800 != 0 {
                    let mut sp = self.r[13];
                    for i in 0..8 {
                        if t & (1 << i) != 0 {
                            match self.read_chk(sp, 4) {
                                Ok(v) => self.r[i] = v,
                                Err(e) => {
                                    self.r[i] = 0;
                                    return Err(e);
                                }
                            }
                            sp = sp.wrapping_add(4);
                        }
                    }
                    if t & 0x100 != 0 {
                        let v = self.read_chk(sp, 4)?;
                        self.r[15] = v & !1;
                        sp = sp.wrapping_add(4);
                    }
                    self.r[13] = sp;
                } else {
                    let mut regs = [0usize; 9];
                    let mut nregs = 0;
                    for i in 0..8 {
                        if t & (1 << i) != 0 {
                            regs[nregs] = i;
                            nregs += 1;
                        }
                    }
                    if t & 0x100 != 0 {
                        regs[nregs] = 14;
                        nregs += 1;
                    }
                    let mut sp = self.r[13].wrapping_sub(4 * nregs as u32);
                    self.r[13] = sp;
                    for &i in &regs[..nregs] {
                        self.write_chk(sp, 4, self.r[i])?;
                        sp = sp.wrapping_add(4);
                    }
                }
            } else {
                return Err(emu_err(format!("undefined 0x{:04x} at {}", hw, self.symbolize(pc))));
            }
        } 0x18 | 0x19 => {
            let rb = ((t >> 8) & 7) as usize;
            if t & 0xFF == 0 {
                return Err(emu_err(format!(
                    "ldm/stm of no registers (0x{:04x}) at {}: UNPREDICTABLE in ARMv4T",
                    hw,
                    self.symbolize(pc)
                )));
            }
            if top == 0x18 && t & (1 << rb) != 0 && t & ((1 << rb) - 1) != 0 {
                return Err(emu_err(format!(
                    "stm storing its base register after a lower one (0x{:04x}) at {}: the stored value is UNPREDICTABLE in ARMv4T",
                    hw,
                    self.symbolize(pc)
                )));
            }
            let mut a = self.r[rb];
            for i in 0..8 {
                if t & (1 << i) != 0 {
                    if top == 0x19 {
                        match self.read_chk(a, 4) {
                            Ok(v) => self.r[i] = v,
                            Err(e) => {
                                self.r[i] = 0;
                                return Err(e);
                            }
                        }
                    } else {
                        self.write_chk(a, 4, self.r[i])?;
                    }
                    a = a.wrapping_add(4);
                }
            }
            if !(top == 0x19 && t & (1 << rb) != 0) {
                self.r[rb] = a;
            }
        } 0x1A | 0x1B => {
            let cc = (t >> 8) & 15;
            if cc >= 14 {
                return Err(emu_err(format!("undefined 0x{:04x} at {}", hw, self.symbolize(pc))));
            }
            if self.cond(cc) {
                self.r[15] = pc.wrapping_add(4).wrapping_add(sext(t & 0xFF, 8).wrapping_mul(2));
            }
        } 0x1C => {
            self.r[15] = pc.wrapping_add(4).wrapping_add(sext(t & 0x7FF, 11).wrapping_mul(2));
        } 0x1E => {
            self.r[14] = pc.wrapping_add(4).wrapping_add(sext(t & 0x7FF, 11) << 12);
        } 0x1F => {
            let target = self.r[14].wrapping_add((t & 0x7FF) * 2);
            self.r[14] = pc.wrapping_add(2);
            self.r[15] = target;
        } _ => {
            return Err(emu_err(format!("undefined 0x{:04x} at {}", hw, self.symbolize(pc))));
        }
        }
        Ok(())
    }

    #[inline]
    fn load<F: FnOnce(&mut Machine) -> Result<u32, Error>>(&mut self, rd: usize, f: F) -> Result<(), Error> {
        match f(self) {
            Ok(v) => {
                self.r[rd] = v;
                Ok(())
            }
            Err(e) => {
                self.r[rd] = 0;
                Err(e)
            }
        }
    }

    #[cfg_attr(debug_assertions, inline(always))]
    #[cfg_attr(not(debug_assertions), inline)]
    fn fetch(&mut self, pc: u32) -> Result<u32, Error> {
        if (self.mem_read_hook.is_some() && pc < self.mem_read_hook_below)
            || pc & 1 != 0
            || pc as usize >= self.cfg.flash_size
            || self.flash.dpd
            || self.flash.busy_until != 0
            || self.flash.cs_low
        {
            return self.read(pc, 2);
        }
        let (mut p, s) = (pc as usize, self.boot_slot);
        if p < s {
            p += s;
        } else if p < 2 * s {
            return self.read(pc, 2);
        }
        if p + 2 > self.flash.size {
            return self.read(pc, 2);
        }
        if self.icache_miss != 0 {
            let line = (pc as i64) >> ICACHE_LINE_SHIFT;
            let slot = (line % ICACHE_LINES as i64) as usize;
            if self.icache_tags[slot] != line && pc as usize >= self.sram_code_end() {
                self.icache_tags[slot] = line;
                self.icache_misses += 1;
                self.cycles += self.icache_miss;
            }
        }
        Ok(self.flash.mem[p] as u32 | (self.flash.mem[p + 1] as u32) << 8)
    }

    #[inline(always)]
    fn read_chk(&mut self, a: u32, size: usize) -> Result<u32, Error> {
        let aligned = self.chk(a, size)?;
        let mut v = self.read(aligned, size)?;
        let off = a & (size as u32 - 1);
        if off != 0 {
            let bits = size as u32 * 8;
            let r = 8 * off;
            let mask = if bits >= 32 { u32::MAX } else { (1u32 << bits) - 1 };
            v = ((v >> r) | (v << (bits - r))) & mask;
        }
        Ok(v)
    }

    #[inline(always)]
    fn write_chk(&mut self, a: u32, size: usize, val: u32) -> Result<(), Error> {
        let a = self.chk(a, size)?;
        self.write(a, size, val)
    }

    fn alu(&mut self, op: u32, rd: usize, rs: usize) {
        let (a, b) = (self.r[rd], self.r[rs]);
        match op {
            0 => {
                self.r[rd] = a & b;
                self.nz(self.r[rd]);
            }
            1 => {
                self.r[rd] = a ^ b;
                self.nz(self.r[rd]);
            }
            2 | 3 | 4 | 7 => {
                let s = b & 0xFF;
                let mut x = a;
                match op {
                    2 => {
                        if s != 0 {
                            self.c = if s <= 32 { (x >> (32 - s)) & 1 } else { 0 };
                            x = if s < 32 { x << s } else { 0 };
                        }
                    }
                    3 => {
                        if s != 0 {
                            self.c = if s <= 32 { (x >> (s - 1)) & 1 } else { 0 };
                            x = if s < 32 { x >> s } else { 0 };
                        }
                    }
                    4 => {
                        if s != 0 {
                            let sx = x as i32;
                            let cs = s.min(32);
                            self.c = ((sx >> (cs - 1)) as u32) & 1;
                            let xs = s.min(31);
                            x = (sx >> xs) as u32;
                        }
                    }
                    _ => {
                        if s != 0 {
                            let k = s & 31;
                            if k != 0 {
                                x = x.rotate_right(k);
                            }
                            self.c = x >> 31;
                        }
                    }
                }
                self.r[rd] = x;
                self.nz(x);
            }
            5 => {
                let c = self.c;
                self.r[rd] = self.add(a, b, c);
            }
            6 => {
                let c = self.c;
                self.r[rd] = self.sub(a, b, c);
            }
            8 => self.nz(a & b),
            9 => self.r[rd] = self.sub(0, b, 1),
            10 => {
                self.sub(a, b, 1);
            }
            11 => {
                self.add(a, b, 0);
            }
            12 => {
                self.r[rd] = a | b;
                self.nz(self.r[rd]);
            }
            13 => {
                self.r[rd] = a.wrapping_mul(b);
                self.nz(self.r[rd]);
            }
            14 => {
                self.r[rd] = a & !b;
                self.nz(self.r[rd]);
            }
            _ => {
                self.r[rd] = !b;
                self.nz(self.r[rd]);
            }
        }
    }
}
