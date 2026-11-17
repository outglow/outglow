use super::thumb::machine;
use crate::Machine;

fn pc1_as_pwm0(cmp: u16, max: u16, invert: bool) -> Machine {
    let mut m = machine();
    m.regs[0x781] |= 0x01;
    m.regs[0x782] = 0;
    m.regs[0x794..0x796].copy_from_slice(&cmp.to_le_bytes());
    m.regs[0x796..0x798].copy_from_slice(&max.to_le_bytes());
    if invert {
        m.regs[0x784] |= 0x01;
    } else {
        m.regs[0x784] &= !0x01;
    }
    m.regs[0x5AC] = 0x08;
    m.regs[0x596] &= !0x02;
    m.regs[0x591] |= 0x02;
    m.analog[0xC0] |= 0x02;
    m
}

fn duty_percent(m: &mut Machine, max: u16) -> f64 {
    let n = 4 * max as usize;
    let mut high = 0;
    for _ in 0..n {
        m.cycles += 1;
        if m.reg_read(0x590, 1).unwrap() & 0x02 != 0 {
            high += 1;
        }
    }
    100.0 * high as f64 / n as f64
}

#[test]
fn pwm0_on_pc1_has_the_duty_of_compare_over_period() {
    for cmp in [25u16, 50, 75] {
        let mut m = pc1_as_pwm0(cmp, 100, false);
        let d = duty_percent(&mut m, 100);
        assert!((d - cmp as f64).abs() < 1.0, "{} / 100: {}%", cmp, d);
    }
}

#[test]
fn pwm0_invert_gives_the_complementary_duty() {
    let mut m = pc1_as_pwm0(25, 100, true);
    let d = duty_percent(&mut m, 100);
    assert!((d - 75.0).abs() < 1.0, "{}%", d);
}

#[test]
fn pwm0_disabled_does_not_drive_the_waveform() {
    let mut m = pc1_as_pwm0(25, 100, false);
    m.regs[0x781] &= !0x01;
    let d = duty_percent(&mut m, 100);
    assert!(d == 0.0 || d == 100.0, "{}%", d);
}

#[test]
fn pwm0_counter_runs_through_the_whole_period_below_max() {
    let mut m = pc1_as_pwm0(25, 100, false);
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..500 {
        m.cycles += 1;
        seen.insert(m.reg_read(0x7B4, 2).unwrap());
    }
    assert_eq!(seen.len(), 100);
    assert_eq!(*seen.iter().max().unwrap(), 99);
}

#[test]
fn pwm0_clock_divider_slows_the_counter() {
    let mut m = pc1_as_pwm0(25, 100, false);
    m.regs[0x782] = 3;
    m.cycles = 0;
    let c0 = m.reg_read(0x7B4, 2).unwrap();
    m.cycles += 40;
    assert_eq!(m.reg_read(0x7B4, 2).unwrap(), (c0 + 10) % 100);
}

#[test]
fn pwm0_frame_done_sets_after_a_period_and_clears_on_writing_one() {
    let mut m = pc1_as_pwm0(25, 100, false);
    m.cycles = 0;
    m.reg_write(0x7B1, 1, 0x04).unwrap();
    assert_eq!(m.reg_read(0x7B1, 1).unwrap() & 0x04, 0);
    m.cycles += 400;
    assert_ne!(m.reg_read(0x7B1, 1).unwrap() & 0x04, 0);
    m.reg_write(0x7B1, 1, 0x04).unwrap();
    assert_eq!(m.reg_read(0x7B1, 1).unwrap() & 0x04, 0);
}

fn ir_fifo() -> Machine {
    let mut m = machine();
    m.regs[0x781] |= 0x01;
    m.regs[0x783] = 0x07;
    m
}

fn fifo_status(m: &mut Machine) -> u32 {
    m.reg_read(0x7CD, 1).unwrap()
}

#[test]
fn pwm_ir_fifo_counts_entries_and_reports_empty_and_full() {
    let mut m = ir_fifo();
    assert_ne!(fifo_status(&mut m) & 0x10, 0, "empty at start");
    for i in 0..3 {
        m.reg_write(0x7C8, 2, 0x0100 + i).unwrap();
    }
    let s = fifo_status(&mut m);
    assert_eq!((s & 0x0F, s & 0x10), (3, 0), "three entries");
    for _ in 0..8 {
        m.reg_write(0x7C8, 2, 0x0200).unwrap();
    }
    assert_ne!(fifo_status(&mut m) & 0x20, 0, "full");
}

#[test]
fn pwm_ir_fifo_drains_and_raises_its_done_and_interrupt_status() {
    let mut m = ir_fifo();
    for i in 0..3 {
        m.reg_write(0x7C8, 2, 0x0100 + i).unwrap();
    }
    m.update_time().unwrap();
    assert_ne!(fifo_status(&mut m) & 0x10, 0, "empty after the run");
    assert_ne!(m.reg_read(0x7B1, 1).unwrap() & 0x02, 0, "done");
    assert_ne!(m.reg_read(0x7B3, 1).unwrap() & 0x01, 0, "interrupt status");
    m.reg_write(0x7B1, 1, 0x02).unwrap();
    assert_eq!(m.reg_read(0x7B1, 1).unwrap() & 0x02, 0, "done clears on writing one");
}

#[test]
fn pwm_ir_fifo_clear_empties_it() {
    let mut m = ir_fifo();
    m.regs[0x781] &= !0x01;
    for i in 0..3 {
        m.reg_write(0x7C8, 2, 0x0100 + i).unwrap();
    }
    m.reg_write(0x7CE, 1, 0x01).unwrap();
    assert_ne!(fifo_status(&mut m) & 0x10, 0);
}
