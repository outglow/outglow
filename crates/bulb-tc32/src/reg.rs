#![allow(dead_code)]

const BASE: usize = 0x80_0000;

#[cfg(target_os = "none")]
#[inline(always)]
pub fn read8(off: usize) -> u8 {
    unsafe { ((BASE + off) as *const u8).read_volatile() }
}

#[cfg(not(target_os = "none"))]
#[inline(always)]
pub fn read8(off: usize) -> u8 {
    crate::sim::read(off, 1) as u8
}

#[cfg(target_os = "none")]
#[inline(always)]
pub fn write8(off: usize, v: u8) {
    unsafe { ((BASE + off) as *mut u8).write_volatile(v) }
}

#[cfg(not(target_os = "none"))]
#[inline(always)]
pub fn write8(off: usize, v: u8) {
    crate::sim::write(off, 1, v as u32)
}

#[cfg(target_os = "none")]
#[inline(always)]
pub fn write16(off: usize, v: u16) {
    unsafe { ((BASE + off) as *mut u16).write_volatile(v) }
}

#[cfg(not(target_os = "none"))]
#[inline(always)]
pub fn write16(off: usize, v: u16) {
    crate::sim::write(off, 2, v as u32)
}

#[cfg(target_os = "none")]
#[inline(always)]
pub fn read16(off: usize) -> u16 {
    unsafe { ((BASE + off) as *const u16).read_volatile() }
}

#[cfg(not(target_os = "none"))]
#[inline(always)]
pub fn read16(off: usize) -> u16 {
    crate::sim::read(off, 2) as u16
}

#[cfg(target_os = "none")]
#[inline(always)]
pub fn write32(off: usize, v: u32) {
    unsafe { ((BASE + off) as *mut u32).write_volatile(v) }
}

#[cfg(not(target_os = "none"))]
#[inline(always)]
pub fn write32(off: usize, v: u32) {
    crate::sim::write(off, 4, v as u32)
}

#[inline(always)]
pub fn set8(off: usize, bits: u8) {
    write8(off, read8(off) | bits);
}

#[inline(always)]
pub fn clear8(off: usize, bits: u8) {
    write8(off, read8(off) & !bits);
}

#[inline(always)]
pub fn modify8(off: usize, keep: u8, bits: u8) {
    write8(off, read8(off) & keep | bits);
}

#[inline(always)]
pub fn modify16(off: usize, keep: u16, bits: u16) {
    write16(off, read16(off) & keep | bits);
}

#[inline(always)]
pub fn modify32(off: usize, keep: u32, bits: u32) {
    write32(off, read32(off) & keep | bits);
}

#[inline(always)]
pub fn tick() -> u32 {
    read32(SYSTEM_TICK)
}

#[inline(always)]
pub fn delay_us(us: u32) {
    let t0 = tick();
    while tick().wrapping_sub(t0) <= us * TICKS_PER_US {}
}

#[inline(always)]
pub fn irq_disable() -> u8 {
    let s = read8(IRQ_ENABLE);
    write8(IRQ_ENABLE, 0);
    s
}

#[inline(always)]
pub fn irq_restore(s: u8) {
    write8(IRQ_ENABLE, s);
}

#[inline(always)]
pub fn analog_write(addr: u8, value: u8) {
    let s = irq_disable();
    write8(ANA_ADDR, addr);
    write8(ANA_DATA, value);
    write8(ANA_CTRL, 0x60);
    while read8(ANA_CTRL) & 0x01 != 0 {}
    write8(ANA_CTRL, 0x00);
    irq_restore(s);
}

#[inline(always)]
pub fn analog_modify(addr: u8, keep: u8, bits: u8) {
    analog_write(addr, analog_read(addr) & keep | bits);
}

#[inline(always)]
pub fn analog_read(addr: u8) -> u8 {
    let s = irq_disable();
    write8(ANA_ADDR, addr);
    write8(ANA_CTRL, 0x40);
    while read8(ANA_CTRL) & 0x01 != 0 {}
    let v = read8(ANA_DATA);
    write8(ANA_CTRL, 0x00);
    irq_restore(s);
    v
}

#[cfg(target_os = "none")]
#[inline(always)]
pub fn read32(off: usize) -> u32 {
    unsafe { ((BASE + off) as *const u32).read_volatile() }
}

#[cfg(not(target_os = "none"))]
#[inline(always)]
pub fn read32(off: usize) -> u32 {
    crate::sim::read(off, 4)
}

pub const MSPI_DATA: usize = 0x0c;
pub const MSPI_CTRL: usize = 0x0d;
pub const MSPI_CS_HIGH: u8 = 0x01;
pub const MSPI_BUSY: u8 = 0x10;

pub const CLK_SEL: usize = 0x66;
pub const CLK_SEL_48M_CRYSTAL: u8 = 0x20;

pub const ANA_ADDR: usize = 0xb8;
pub const ANA_DATA: usize = 0xb9;
pub const ANA_CTRL: usize = 0xba;

pub const SYSTEM_TICK: usize = 0x740;
pub const SYSTEM_TICK_CTRL: usize = 0x74f;
pub const SYSTEM_TICK_IRQ: usize = 0x744;
pub const SYSTEM_TICK_MODE: usize = 0x74c;

pub const PWM0_ENABLE: usize = 0x781;
pub const PWM_CLK_DIV: usize = 0x782;
pub const fn pwm_cmp(n: u8) -> usize {
    0x794 + 4 * n as usize
}
pub const fn pwm_max(n: u8) -> usize {
    0x796 + 4 * n as usize
}

pub const TICKS_PER_US: u32 = 16;

pub const IRQ_MASK: usize = 0x640;
pub const IRQ_MASK_RF: u32 = 0x2000;
pub const IRQ_ENABLE: usize = 0x643;

pub const PWDN_CTRL: usize = 0x6f;

pub const PWM_ENABLE: usize = 0x780;

pub const TIMER_CTRL: usize = 0x620;
pub const TIMER_STATUS: usize = 0x623;
pub const TIMER2_TICK: usize = 0x638;
