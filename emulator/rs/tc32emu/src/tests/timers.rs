use super::thumb::*;
use crate::{Error, STIMER_HZ};

#[test]
fn timer0_status_sets_only_once_the_capture_is_reached() {
    let mut m = machine();
    m.reg_write(0x624, 4, 1000).unwrap();
    m.reg_write(0x620, 4, 0x01).unwrap();
    m.cycles += 999;
    m.update_time().unwrap();
    assert_eq!(m.regs[0x623] & 0x01, 0);
    m.cycles += 1;
    m.update_time().unwrap();
    assert_ne!(m.regs[0x623] & 0x01, 0);
}

#[test]
fn timer0_status_clears_on_writing_one() {
    let mut m = machine();
    m.reg_write(0x624, 4, 1000).unwrap();
    m.reg_write(0x620, 4, 0x01).unwrap();
    m.cycles += 1500;
    m.update_time().unwrap();
    m.reg_write(0x623, 1, 0x01).unwrap();
    assert_eq!(m.regs[0x623] & 0x01, 0);
}

#[test]
fn timer0_match_raises_a_pending_interrupt() {
    let mut m = machine();
    m.regs[0x643] = 1;
    m.regs[0x640] = 0x01;
    m.ibit = 0;
    m.reg_write(0x624, 4, 1000).unwrap();
    m.reg_write(0x620, 4, 0x01).unwrap();
    assert!(!m.irq_pending());
    m.cycles += 1000;
    m.update_time().unwrap();
    assert!(m.irq_pending());
}

#[test]
fn watchdog_resets_only_after_its_capture_in_units_of_2_to_the_18_cycles() {
    let mut m = machine();
    m.reg_write(0x638, 4, 0).unwrap();
    let c0 = m.cycles;
    m.reg_write(0x620, 4, (1 << 23) | (1 << 6) | (2 << 9)).unwrap();
    m.cycles = c0 + 2 * (1 << 18) - 1;
    assert!(m.update_time().is_ok());
    m.cycles = c0 + 2 * (1 << 18);
    assert!(matches!(m.update_time(), Err(Error::WatchdogReset)));
}

#[test]
fn watchdog_without_its_enable_bit_never_resets() {
    let mut m = machine();
    m.reg_write(0x638, 4, 0).unwrap();
    m.reg_write(0x620, 4, (1 << 6) | (2 << 9)).unwrap();
    m.cycles += 10 * (1 << 18);
    assert!(m.update_time().is_ok());
}

#[test]
fn system_timer_started_through_0x74a_counts_at_16_mhz() {
    let mut m = machine();
    m.reg_write(0x74A, 1, m.regs[0x74A] as u32 | 0x02).unwrap();
    let t0 = m.reg_read(0x740, 4).unwrap();
    m.cycles += m.cpu_hz / 1000;
    let t1 = m.reg_read(0x740, 4).unwrap();
    let want = (STIMER_HZ / 1000) as u32;
    assert!(t1.wrapping_sub(t0).abs_diff(want) <= 8, "{} ticks in 1 ms", t1.wrapping_sub(t0));
}

#[test]
fn system_timer_reads_in_steps_of_8_ticks() {
    let mut m = machine();
    m.reg_write(0x74A, 1, m.regs[0x74A] as u32 | 0x02).unwrap();
    for _ in 0..50 {
        m.cycles += 7;
        assert_eq!(m.reg_read(0x740, 4).unwrap() & 7, 0);
    }
}

#[test]
fn system_timer_start_and_stop_through_0x74f() {
    let mut m = machine();
    m.reg_write(0x74F, 1, 0x01).unwrap();
    assert_eq!(m.reg_read(0x74F, 1).unwrap() & 0x02, 0x02);
    let t0 = m.reg_read(0x740, 4).unwrap();
    m.cycles += 24_000;
    let t1 = m.reg_read(0x740, 4).unwrap();
    assert!(t1 > t0);
    m.reg_write(0x74F, 1, 0x02).unwrap();
    assert_eq!(m.reg_read(0x74F, 1).unwrap() & 0x02, 0);
    let t2 = m.reg_read(0x740, 4).unwrap();
    m.cycles += 24_000;
    assert_eq!(m.reg_read(0x740, 4).unwrap(), t2);
}

#[test]
fn system_timer_is_monotonic_across_a_running_program() {
    let mut m = machine();
    m.reg_write(0x74A, 1, m.regs[0x74A] as u32 | 0x02).unwrap();
    let code = Asm::new().label("l").op(sub_imm(0, 1)).bc(NE, "l").finish();
    let mut last = m.reg_read(0x740, 4).unwrap();
    let mut deltas = vec![];
    for loops in [50u32, 2000] {
        load(&mut m, &code, &[(0, loops)]);
        run_to_halt(&mut m, code.len(), 100_000);
        let now = m.reg_read(0x740, 4).unwrap();
        assert!(now > last);
        deltas.push(now - last);
        last = now;
    }
    assert!(deltas[1] > deltas[0]);
}

#[test]
fn pwm_compare_register_reads_back_what_was_written() {
    let mut m = machine();
    for v in [0u32, 0x1234, 0xFFFF, 0xABCDEF, 0xFFFF_FFFF] {
        m.write(0x800794, 4, v).unwrap();
        assert_eq!(m.read(0x800794, 4).unwrap(), v);
    }
}
