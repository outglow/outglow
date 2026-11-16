//! Memory-mapped registers of the TLSR8258 (digital registers at 0x800000).
//!
//! Offsets and bit meanings are from Telink's public register header
//! (`platform/chip_8258/register.h` of the Apache-2.0 Zigbee SDK).

#![allow(dead_code)] // a register map: not every build uses every entry

const BASE: usize = 0x80_0000;

#[inline(always)]
pub fn read8(off: usize) -> u8 {
    // SAFETY: `off` is a register offset inside the chip's register block.
    unsafe { ((BASE + off) as *const u8).read_volatile() }
}

#[inline(always)]
pub fn write8(off: usize, v: u8) {
    // SAFETY: as in `read8`.
    unsafe { ((BASE + off) as *mut u8).write_volatile(v) }
}

#[inline(always)]
pub fn write16(off: usize, v: u16) {
    // SAFETY: as in `read8`; 16-bit registers are at even offsets.
    unsafe { ((BASE + off) as *mut u16).write_volatile(v) }
}

#[inline(always)]
pub fn read16(off: usize) -> u16 {
    // SAFETY: as in `read8`; 16-bit registers are at even offsets.
    unsafe { ((BASE + off) as *const u16).read_volatile() }
}

#[inline(always)]
pub fn write32(off: usize, v: u32) {
    // SAFETY: as in `read8`; 32-bit registers are at offsets divisible by 4.
    unsafe { ((BASE + off) as *mut u32).write_volatile(v) }
}

/// Sets bits in an 8-bit register.
#[inline(always)]
pub fn set8(off: usize, bits: u8) {
    write8(off, read8(off) | bits);
}

/// Clears bits in an 8-bit register.
#[inline(always)]
pub fn clear8(off: usize, bits: u8) {
    write8(off, read8(off) & !bits);
}

/// Current system timer tick (16 per microsecond).
#[inline(always)]
pub fn tick() -> u32 {
    read32(SYSTEM_TICK)
}

/// Busy-waits for `us` microseconds on the system timer.
#[inline(always)]
pub fn delay_us(us: u32) {
    let t0 = tick();
    while tick().wrapping_sub(t0) <= us * TICKS_PER_US {}
}

/// Disables interrupts; returns the previous state for `irq_restore`.
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

/// Writes an analog register through the analog port.
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

/// Reads an analog register through the analog port.
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

#[inline(always)]
pub fn read32(off: usize) -> u32 {
    // SAFETY: as in `read8`; 32-bit registers are at offsets divisible by 4.
    unsafe { ((BASE + off) as *const u32).read_volatile() }
}

/// MSPI (the flash's SPI): data byte.
pub const MSPI_DATA: usize = 0x0c;
/// MSPI control: bit 0 chip select high, bit 4 busy.
pub const MSPI_CTRL: usize = 0x0d;
pub const MSPI_CS_HIGH: u8 = 0x01;
pub const MSPI_BUSY: u8 = 0x10;

/// System clock select.
pub const CLK_SEL: usize = 0x66;
/// 48 MHz from the crystal doubler (the SDK's SYS_CLK_48M_Crystal).
pub const CLK_SEL_48M_CRYSTAL: u8 = 0x20;

/// Analog register port: address, data, control (bit 6 start, bit 5 write, bit 0 busy).
pub const ANA_ADDR: usize = 0xb8;
pub const ANA_DATA: usize = 0xb9;
pub const ANA_CTRL: usize = 0xba;

/// System timer: free-running 32-bit counter at 16 MHz.
pub const SYSTEM_TICK: usize = 0x740;
/// System timer control: bit 0 start.
pub const SYSTEM_TICK_CTRL: usize = 0x74f;
/// System timer compare: its interrupt source is set when the tick passes it.
pub const SYSTEM_TICK_IRQ: usize = 0x744;
/// Bit 1: the compare's interrupt source enabled (still masked by 0x640).
pub const SYSTEM_TICK_MODE: usize = 0x74c;

/// PWM0 enable (bit 0).
pub const PWM0_ENABLE: usize = 0x781;
/// PWM clock divider: PWM clock = system clock / (value + 1).
pub const PWM_CLK_DIV: usize = 0x782;
/// Compare (high time) and period of PWM channel `n`, in PWM clock ticks.
pub const fn pwm_cmp(n: u8) -> usize {
    0x794 + 4 * n as usize
}
pub const fn pwm_max(n: u8) -> usize {
    0x796 + 4 * n as usize
}

pub const TICKS_PER_US: u32 = 16;

/// Interrupt mask (32 bits; the top byte is `IRQ_ENABLE`).
pub const IRQ_MASK: usize = 0x640;
/// The radio's interrupt source in `IRQ_MASK`.
pub const IRQ_MASK_RF: u32 = 0x2000;
/// Global interrupt enable (bit 0).
pub const IRQ_ENABLE: usize = 0x643;

/// Writing 0x20 reboots the chip.
pub const PWDN_CTRL: usize = 0x6f;

/// PWM enables of channels 1..5 (bit n = PWMn).
pub const PWM_ENABLE: usize = 0x780;

/// Timer control (32 bits): bit 6 Timer2 enable, bits 9..22 watchdog
/// capture (in units of 2^18 system clock cycles), bit 23 watchdog enable.
pub const TIMER_CTRL: usize = 0x620;
/// Timer status; writing bit 3 restarts the watchdog count.
pub const TIMER_STATUS: usize = 0x623;
pub const TIMER2_TICK: usize = 0x638;
