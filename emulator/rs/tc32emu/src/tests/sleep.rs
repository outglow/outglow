use super::thumb::*;
use crate::Machine;

fn gpio_wake_ready(m: &mut Machine, wake_on_low: bool) {
    m.analog[0x26] |= 0x08;
    m.analog[0x27] = 0x08;
    if wake_on_low {
        m.analog[0x21] |= 0x08;
    } else {
        m.analog[0x21] &= !0x08;
    }
    m.analog[0x0E] = (m.analog[0x0E] & !(3 << 6)) | (1 << 6);
}

fn set_mode(m: &mut Machine, mode: u8) {
    m.reg_write(0xB8, 1, 0x7E).unwrap();
    m.reg_write(0xB9, 1, mode as u32).unwrap();
    m.reg_write(0xBA, 1, 0x60).unwrap();
}

fn sleep(m: &mut Machine) {
    m.reg_write(0x6F, 1, 0x81).unwrap();
}

#[test]
fn suspend_wakes_on_a_matching_gpio_level_and_reports_it_in_analog_0x44() {
    let mut m = machine();
    set_mode(&mut m, 0);
    gpio_wake_ready(&mut m, false);
    sleep(&mut m);
    assert!(m.asleep.is_some());
    let rc = m.reset_count;
    m.sleep_until(m.cycles + 100_000);
    assert!(m.asleep.is_none());
    assert_ne!(m.analog[0x44] & 0x08, 0);
    assert_eq!(m.reset_count, rc, "a suspend wake resumes without a reboot");
}

#[test]
fn suspend_without_a_matching_source_stays_asleep() {
    let mut m = machine();
    set_mode(&mut m, 0);
    gpio_wake_ready(&mut m, true);
    sleep(&mut m);
    m.sleep_until(m.cycles + 100_000);
    assert!(m.asleep.is_some());
}

#[test]
fn a_gpio_level_without_the_wake_enable_does_not_wake() {
    let mut m = machine();
    set_mode(&mut m, 0);
    gpio_wake_ready(&mut m, false);
    m.analog[0x26] &= !0x08;
    sleep(&mut m);
    m.sleep_until(m.cycles + 100_000);
    assert!(m.asleep.is_some());
}

#[test]
fn deep_sleep_wake_reboots_and_keeps_the_wake_status() {
    let mut m = machine();
    set_mode(&mut m, 0x04);
    gpio_wake_ready(&mut m, false);
    m.r[15] = CODE + 0x200;
    sleep(&mut m);
    assert!(m.asleep.is_some());
    let rc = m.reset_count;
    m.sleep_until(m.cycles + 100_000);
    assert!(m.asleep.is_none());
    assert_eq!((m.reset_count, m.r[15]), (rc + 1, 0));
    assert_ne!(m.analog[0x44] & 0x08, 0);
}

#[test]
fn deep_sleep_keeps_the_first_32k_of_sram_and_clears_the_rest() {
    let mut m = machine();
    m.flash.mem[0x0C..0x10].copy_from_slice(&[0x10, 0, 0, 0]);
    set_mode(&mut m, 0x04);
    gpio_wake_ready(&mut m, false);
    set_sram_u32(&mut m, crate::SRAM_BASE + 0x7FFC, 0x1234_5678);
    set_sram_u32(&mut m, crate::SRAM_BASE + 0x8000, 0x9ABC_DEF0);
    sleep(&mut m);
    m.sleep_until(m.cycles + 100_000);
    assert_eq!(sram_u32(&m, crate::SRAM_BASE + 0x7FFC), 0x1234_5678);
    assert_eq!(sram_u32(&m, crate::SRAM_BASE + 0x8000), 0);
}

#[test]
fn low_power_comparator_wakes_only_when_enabled_and_fired() {
    for (enabled, fired, wakes) in [(true, true, true), (true, false, false), (false, true, false)] {
        let mut m = machine();
        set_mode(&mut m, 0);
        if enabled {
            m.analog[0x26] |= 0x80;
        }
        m.lpc_fired = fired;
        sleep(&mut m);
        m.sleep_until(m.cycles + 100_000);
        assert_eq!(m.asleep.is_none(), wakes, "enabled {} fired {}", enabled, fired);
        assert_eq!(m.analog[0x44] & 0x01 != 0, wakes, "status bit, enabled {} fired {}", enabled, fired);
    }
}
