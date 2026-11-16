use crate::{isa, Error, Flash, Machine, FLASH_MEM};

fn machine() -> Machine {
    let mut fl = Flash::new(FLASH_MEM);
    fl.mem[8] = 0x4B;
    let mut m = Machine::new(fl, -1, 1).unwrap();
    m.cycles += 1000;
    m
}

fn timer(m: &mut Machine, n: usize, cap: u32) {
    m.reg_write(0x630 + 4 * n, 4, 0).unwrap();
    m.reg_write(0x624 + 4 * n, 4, cap).unwrap();
    let en = [0x01, 0x08][n];
    let c = m.regs[0x620] as u32 | en;
    m.reg_write(0x620, 1, c).unwrap();
}

#[test]
fn stall_wakes_at_the_timer_match() {
    let mut m = machine();
    m.reg_write(0x78, 4, 0x01).unwrap();
    let c0 = m.cycles;
    timer(&mut m, 0, 2400);
    m.reg_write(0x6F, 1, 0x80).unwrap();
    assert!(m.stalled && m.asleep.is_none());
    m.stall_until(c0 + 100_000).unwrap();
    assert!(!m.stalled);
    assert_eq!(m.cycles, c0 + 2400);
    assert_eq!(m.stall_cycles, 2400);
}

#[test]
fn a_source_outside_the_wake_mask_does_not_wake() {
    let mut m = machine();
    m.reg_write(0x78, 4, 0x02).unwrap();
    let c0 = m.cycles;
    timer(&mut m, 0, 2400);
    timer(&mut m, 1, 4800);
    m.reg_write(0x6F, 1, 0x80).unwrap();
    m.stall_until(c0 + 100_000).unwrap();
    assert!(!m.stalled);
    assert_eq!(m.cycles, c0 + 4800);
    assert_eq!(m.regs[0x623] & 0x03, 0x03);
}

#[test]
fn timer2_matches_its_capture() {
    let mut m = machine();
    m.reg_write(0x78, 4, 0x04).unwrap();
    let c0 = m.cycles;
    m.reg_write(0x638, 4, 0).unwrap();
    m.reg_write(0x62C, 4, 7200).unwrap();
    m.reg_write(0x620, 1, 0x40).unwrap();
    m.reg_write(0x6F, 1, 0x80).unwrap();
    m.stall_until(c0 + 100_000).unwrap();
    assert!(!m.stalled);
    assert_eq!(m.cycles, c0 + 7200);
    assert_eq!(m.regs[0x623] & 0x04, 0x04);
}

#[test]
fn no_wake_source_stalls_to_the_limit() {
    let mut m = machine();
    m.reg_write(0x78, 4, 0).unwrap();
    timer(&mut m, 0, 2400);
    let c0 = m.cycles;
    m.reg_write(0x6F, 1, 0x80).unwrap();
    m.stall_until(c0 + 50_000).unwrap();
    assert!(m.stalled);
    assert_eq!(m.cycles, c0 + 50_000);
    let pc = m.r[15];
    let s = m.step().unwrap();
    assert!(!s.ran && m.stalled && m.r[15] == pc);
    assert_eq!(m.cycles, c0 + 50_000 + m.cpu_hz / 1000);
}

#[test]
fn suspend_is_bit_0() {
    let mut m = machine();
    m.reg_write(0x6F, 1, 0x81).unwrap();
    assert!(m.asleep.is_some() && !m.stalled);
}

#[test]
fn the_watchdog_runs_while_stalled() {
    let mut m = machine();
    m.reg_write(0x78, 4, 0).unwrap();
    m.reg_write(0x638, 4, 0).unwrap();
    m.reg_write(0x620, 4, (1 << 23) | (1 << 9) | 0x40).unwrap();
    let c0 = m.cycles;
    m.reg_write(0x6F, 1, 0x80).unwrap();
    let r = m.stall_until(c0 + 10_000_000);
    assert!(matches!(r, Err(Error::WatchdogReset)), "{r:?}");
    assert_eq!(m.cycles, c0 + (1 << 18));
}

#[test]
fn run_takes_the_waking_interrupt_first() {
    let mut m = machine();
    let pc = 0x840100u32;
    let b = isa::to_tc32(0xE7FE);
    m.sram[0x100] = b as u8;
    m.sram[0x101] = (b >> 8) as u8;
    m.r[15] = pc;
    m.regs[0x640] = 0x01;
    m.regs[0x643] = 0x01;
    m.ibit = 0;
    m.reg_write(0x78, 4, 0x01).unwrap();
    let c0 = m.cycles;
    timer(&mut m, 0, 2400);
    m.reg_write(0x6F, 1, 0x80).unwrap();
    m.run(c0 + 2400, &[], 32).unwrap();
    assert!(!m.stalled);
    assert_eq!(m.cycles, c0 + 2400);
    assert_eq!((m.r[15], m.r[14], m.mode), (0x10, pc, 0x12));
}
