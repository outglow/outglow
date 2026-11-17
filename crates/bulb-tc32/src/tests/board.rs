use super::*;
use crate::sim::{self, Op};

#[test]
fn the_cold_start_follows_the_vendor_cpu_wakeup_sequence() {
    let _s = sim::session();
    sim::with(|c| {
        c.set_analog(0x0b, 0xff);
        c.set_analog(0x7f, 0x01);
        c.set_analog(0x3c, 0x0f);
        c.set_analog(0xcb, 0x5a);
        c.set_analog(0x88, 0x13);
        c.set_analog(0x87, 0xff);
    });
    init();
    let w: Vec<Op> = sim::with(|c| c.writes())
        .into_iter()
        .filter(|o| !matches!(o, Op::W8(a, _) if [reg::ANA_ADDR, reg::ANA_DATA, reg::ANA_CTRL, reg::IRQ_ENABLE].contains(a)))
        .collect();
    assert_eq!(
        w[..8].to_vec(),
        vec![
            Op::W32(0x640, 0),
            Op::W8(0x60, 0x00),
            Op::W8(0x61, 0x00),
            Op::W8(0x62, 0x00),
            Op::W8(0x63, 0xff),
            Op::W8(0x64, 0xff),
            Op::W8(0x65, 0xff),
            Op::W8(0x66, 0x06),
        ]
    );
    assert_eq!(
        w[8..].to_vec(),
        vec![
            Op::W32(0xc40, 0x0404_0404),
            Op::W32(0xc44, 0x0404_0404),
            Op::W8(0xc48, 0x04),
            Op::W16(0x750, 0),
            Op::W8(0x74f, 0x01),
            Op::W8(0xc20, 0x00),
            Op::W8(0xc21, 0x00),
            Op::W8(0x66, 0x20),
        ]
    );
    let a = sim::with(|c| c.analog_writes());
    assert_eq!(
        a,
        vec![
            (0x82, 0x64),
            (0x34, 0x80),
            (0x0b, 0xb8),
            (0x8c, 0x02),
            (0x02, 0xa2),
            (0x27, 0x00),
            (0x28, 0x00),
            (0x29, 0x00),
            (0x2a, 0x00),
            (0x01, 0x4c),
            (0x3c, 0x0d),
            (0xc8, 0x80),
            (0x30, 0x80),
            (0xc7, 0x0e),
            (0xc7, 0x0f),
            (0x33, 0x5a),
            (0x30, 0x00),
            (0xc7, 0x0e),
            (0x86, 0xbb),
            (0x82, 0x64),
            (0x87, 0xfe),
            (0x87, 0xfe),
            (0x87, 0x9e),
            (0x82, 0xe4),
            (0x87, 0x9c),
            (0x86, 0xfb),
            (0x0c, 0xc6),
        ]
    );
}

#[test]
fn a_revision_1_chip_gets_the_other_ldo_setting() {
    let _s = sim::session();
    sim::with(|c| c.poke8(0x7d, 1));
    init();
    assert!(sim::with(|c| c.analog_writes()).contains(&(0x01, 0x3c)));
}

#[test]
fn a_crystal_that_never_starts_reboots_the_chip() {
    let _s = sim::session();
    sim::with(|c| c.tick_step = 0);
    assert!(sim::reboots(init));
}

#[test]
fn reboot_powers_the_chip_down() {
    let _s = sim::session();
    assert!(sim::reboots(|| {
        reboot();
    }));
    assert_eq!(sim::with(|c| c.peek8(reg::PWDN_CTRL)), 0x20);
}

#[test]
fn the_reset_cause_comes_from_the_retained_analog_bit_and_the_watchdog_flag() {
    let _s = sim::session();
    assert!(!warm_reset());
    assert_eq!(reset_cause(), ResetCause::PowerOn);
    assert_eq!(sim::with(|c| c.analog(RETAINED)), RETAINED_ALIVE);
    assert!(warm_reset());
    assert_eq!(reset_cause(), ResetCause::Software);
    sim::with(|c| c.poke8(WD_STATUS, 0x01));
    assert!(warm_reset());
    assert_eq!(reset_cause(), ResetCause::Watchdog);
    assert!(sim::with(|c| c.writes()).contains(&Op::W8(WD_STATUS, 0x01)));
}

#[test]
fn the_watchdog_counts_two_seconds_of_timer_2() {
    let _s = sim::session();
    sim::with(|c| c.poke8(reg::TIMER_CTRL + 3, 0xff));
    watchdog_start(2_000);
    let capture = (2_000 * 48_000) >> 18;
    assert_eq!(capture, 366);
    assert_eq!(
        sim::with(|c| c.writes()),
        vec![Op::W32(reg::TIMER2_TICK, 0), Op::W32(reg::TIMER_CTRL, capture << 9 | 1 << 6 | 1 << 23), Op::W8(reg::TIMER_STATUS, 0x08)]
    );
}

#[test]
fn the_clock_turns_ticks_into_milliseconds_without_losing_the_rest() {
    let _s = sim::session();
    sim::with(|c| c.tick_step = 0);
    let clock = SystemClock::new();
    sim::with(|c| c.tick = 15_999);
    assert_eq!(clock.now_ms(), 0);
    sim::with(|c| c.tick = 16_001);
    assert_eq!(clock.now_ms(), 1);
    sim::with(|c| c.tick = 32_000);
    assert_eq!(clock.now_ms(), 2);
    sim::with(|c| c.tick = 32_000u32.wrapping_add(u32::MAX));
    assert_eq!(clock.now_ms(), 2 + (u32::MAX / 16_000) as u64);
}

#[test]
fn the_led_pwm_starts_every_channel_of_the_board_and_clamps_the_duty() {
    let _s = sim::session();
    let warm = BOARD.warm;
    sim::with(|c| c.poke8(warm.mux, 0xff));
    let mut led = LedPwm::new();
    let w = sim::with(|c| c.writes());
    assert_eq!(w[0], Op::W8(reg::PWM_CLK_DIV, 0));
    assert_eq!(w[1], Op::W16(reg::pwm_max(warm.pwm), BOARD.period));
    assert_eq!(w[2], Op::W16(reg::pwm_cmp(warm.pwm), 0));
    assert_eq!(w[3], Op::W8(warm.mux, (0xff & !(3 << warm.shift)) | warm.value << warm.shift));
    sim::with(|c| c.log.clear());
    led.set_duty(u16::MAX);
    led.set_cold_duty(10);
    led.set_rgb(1, 2, 3);
    let w = sim::with(|c| c.writes());
    assert_eq!(w[0], Op::W16(reg::pwm_cmp(warm.pwm), BOARD.period));
    assert_eq!(w.len(), 1 + BOARD.cold.is_some() as usize + 3 * BOARD.rgb.is_some() as usize);
    assert_eq!(led.max_duty(), BOARD.period);
    assert_eq!(led.has_cold_channel(), BOARD.cold.is_some());
    assert_eq!(led.ct_range(), (BOARD.ct_min, BOARD.ct_max));
    assert_eq!(led.has_rgb(), BOARD.rgb.is_some());
    assert_eq!(led.white_tw_curve(), BOARD.cold.is_some() || BOARD.rgb.is_some());
    assert_eq!(led.rgb_beside_white(), BOARD.rgb_beside_white);
    assert_eq!(led.colour_default(), BOARD.colour_default);
}

#[test]
fn every_board_of_the_table_is_found_by_name() {
    for b in crate::boards::BOARDS.iter() {
        assert!(str_eq(board_named(b.name).name, b.name));
    }
    assert!(!str_eq("a", "ab"));
    assert!(!str_eq("ab", "ac"));
    assert_eq!(BOARD.name, "ESMLFzm_w6_Dimm-4.63");
}

#[test]
fn the_reset_counter_words_live_in_their_flash_sector() {
    use bulb_core::reset::WordStore;
    let _s = sim::session();
    let mut w = FlashWords;
    assert_eq!(w.slots(), 0x800);
    w.write(3, 0x1234);
    assert_eq!(w.read(3), 0x1234);
    assert_eq!(sim::with(|c| (c.flash[RESET_SECTOR + 6], c.flash[RESET_SECTOR + 7])), (0x34, 0x12));
    w.erase();
    assert_eq!(w.read(3), 0xffff);
}

#[test]
fn the_light_settings_survive_a_store_and_load() {
    let _s = sim::session();
    let mut s = FlashStore::new();
    let mut buf = [0u8; 16];
    assert_eq!(s.load(&mut buf), None);
    let record = bulb_core::config::Config::default().encode();
    s.store(&record);
    let n = FlashStore::new().load(&mut buf).expect("stored");
    assert_eq!(&buf[..n], &record);
    assert_eq!(s.load_colour(&mut buf), None);
    s.store_colour(&[0; bulb_core::colour::RECORD_LEN]);
    FlashStore::erase();
    assert_eq!(FlashStore::new().load(&mut buf), None);
}

fn outputs(b: &Board) -> Vec<Out> {
    let mut v = vec![b.warm];
    v.extend(b.cold);
    v.extend(b.rgb.iter().flatten().copied());
    v
}

#[test]
fn every_board_starts_each_of_its_pwm_outputs_on_its_pin() {
    for b in crate::boards::BOARDS.iter() {
        let _s = sim::session();
        let mut regs = std::collections::HashMap::new();
        for o in outputs(b) {
            regs.insert(o.mux, 0xa5u8);
            regs.insert(o.gpio, 0xa5u8);
        }
        regs.insert(reg::PWM_ENABLE, 0x00);
        regs.insert(reg::PWM0_ENABLE, 0x00);
        sim::with(|c| regs.iter().for_each(|(a, v)| c.poke8(*a, *v)));
        let _led = LedPwm::on(b);
        let mut expected = vec![Op::W8(reg::PWM_CLK_DIV, 0)];
        for o in outputs(b) {
            expected.push(Op::W16(0x796 + 4 * o.pwm as usize, b.period));
            expected.push(Op::W16(0x794 + 4 * o.pwm as usize, 0));
            let m = regs[&o.mux] & !(3 << o.shift) | o.value << o.shift;
            regs.insert(o.mux, m);
            expected.push(Op::W8(o.mux, m));
            let g = regs[&o.gpio] & !o.bit;
            regs.insert(o.gpio, g);
            expected.push(Op::W8(o.gpio, g));
            let (en, bit) = if o.pwm == 0 { (reg::PWM0_ENABLE, 0x01) } else { (reg::PWM_ENABLE, 1 << o.pwm) };
            let e = regs[&en] | bit;
            regs.insert(en, e);
            expected.push(Op::W8(en, e));
        }
        assert_eq!(sim::with(|c| c.writes()), expected, "{}", b.name);
    }
}

#[test]
fn every_board_drives_its_channels_and_tells_its_light_kind() {
    for b in crate::boards::BOARDS.iter() {
        let _s = sim::session();
        let mut led = LedPwm::on(b);
        sim::with(|c| c.log.clear());
        led.set_duty(b.period + 1);
        led.set_duty(b.period - 1);
        led.set_cold_duty(b.period + 7);
        led.set_cold_duty(3);
        led.set_rgb(10, 20, 30);
        let mut expected = vec![Op::W16(0x794 + 4 * b.warm.pwm as usize, b.period), Op::W16(0x794 + 4 * b.warm.pwm as usize, b.period - 1)];
        if let Some(c) = b.cold {
            expected.extend([Op::W16(0x794 + 4 * c.pwm as usize, b.period), Op::W16(0x794 + 4 * c.pwm as usize, 3)]);
        }
        if let Some([r, g, bl]) = b.rgb {
            expected.extend([Op::W16(0x794 + 4 * r.pwm as usize, 10), Op::W16(0x794 + 4 * g.pwm as usize, 20), Op::W16(0x794 + 4 * bl.pwm as usize, 30)]);
        }
        assert_eq!(sim::with(|c| c.writes()), expected, "{}", b.name);
        assert_eq!(led.max_duty(), b.period);
        assert_eq!(led.has_cold_channel(), b.cold.is_some());
        assert_eq!(led.has_rgb(), b.rgb.is_some());
        assert_eq!(led.white_tw_curve(), b.cold.is_some() || b.rgb.is_some());
        assert_eq!(b.tunable(), b.cold.is_some() || b.rgb.is_some());
        assert_eq!(led.ct_range(), (b.ct_min, b.ct_max));
        assert_eq!(led.rgb_beside_white(), b.rgb_beside_white);
        assert_eq!(led.colour_default(), b.colour_default);
    }
    let kinds: Vec<(bool, bool, bool)> = crate::boards::BOARDS.iter().map(|b| (b.cold.is_some(), b.rgb.is_some(), b.tunable())).collect();
    assert!(kinds.contains(&(false, false, false)) && kinds.contains(&(true, false, true)) && kinds.contains(&(false, true, true)));
    assert!(crate::boards::BOARDS.iter().any(|b| b.rgb_beside_white) && crate::boards::BOARDS.iter().any(|b| b.colour_default != (170, 254)));
}

#[test]
fn a_colour_bulb_keeps_its_colour_record_and_rewrites_it_only_when_it_changes() {
    let _s = sim::session();
    let rgb = crate::boards::BOARDS.iter().find(|b| b.rgb.is_some()).expect("a colour board");
    let mut s = FlashStore::on(rgb);
    let mut buf = [0u8; bulb_core::colour::RECORD_LEN];
    assert_eq!(s.load_colour(&mut buf), None);
    let record = bulb_core::colour::ColourRecord { mode: bulb_core::command::ColorMode::Temperature, xy: false, hue: 10, sat: 20, x: 1, y: 2 }.encode();
    let stores = crate::flash::STORES.get();
    s.store_colour(&record);
    assert_eq!(crate::flash::STORES.get(), stores.wrapping_add(1));
    assert_eq!(FlashStore::on(rgb).load_colour(&mut buf), Some(record.len()));
    assert_eq!(buf, record);
    let programs = sim::with(|c| c.flash_ops.len());
    s.store_colour(&record);
    assert_eq!(sim::with(|c| c.flash_ops.len()), programs);
    assert_eq!(crate::flash::STORES.get(), stores.wrapping_add(1));
    let other = bulb_core::colour::ColourRecord { mode: bulb_core::command::ColorMode::Temperature, xy: false, hue: 10, sat: 21, x: 1, y: 2 }.encode();
    s.store_colour(&other);
    assert_eq!(crate::flash::STORES.get(), stores.wrapping_add(2));
    assert!(is_colour(&record));
    assert!(!is_colour(&[0xff; bulb_core::colour::RECORD_LEN]));
    s.erase_all();
    assert_eq!(FlashStore::on(rgb).load_colour(&mut buf), None);
    assert!(sim::with(|c| c.flash_ops.contains(&(0x20, COLOUR_SECTORS[0])) || c.flash_ops.contains(&(0x20, COLOUR_SECTORS[1]))));
}

#[test]
fn a_board_name_that_is_not_in_the_table_stops_the_build() {
    let _s = sim::session();
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let e = std::panic::catch_unwind(|| board_named("no such board")).err();
    std::panic::set_hook(hook);
    assert_eq!(e.and_then(|e| e.downcast_ref::<&str>().copied()), Some("BULB_BOARD names no board of boards.rs"));
    for b in crate::boards::BOARDS.iter() {
        assert!(str_eq(board_named(b.name).name, b.name));
        assert!(str_eq(board_named(b.name).tag, b.tag));
    }
    assert!(!str_eq("ab", "ba"));
    assert!(str_eq("", ""));
}

#[test]
fn the_crystal_runs_once_its_tick_has_moved_more_than_20_us() {
    let _s = sim::session();
    init();
    assert_eq!(sim::with(|c| c.ticks_read()).len(), 22);
    drop(_s);
    let _s = sim::session();
    sim::with(|c| c.tick_step = 0);
    assert!(sim::reboots(init));
    assert_eq!(sim::with(|c| c.ticks_read()).len(), 10 * 2001);
}

#[test]
fn the_wakeup_flag_of_a_deep_sleep_is_cleared_only_when_it_is_set() {
    for (a7f, a3c, cleared) in [(0x01u8, 0x0f, Some(0x0d)), (0x01, 0x0d, None), (0xfe, 0x0f, None), (0x00, 0x02, None)] {
        let _s = sim::session();
        sim::with(|c| {
            c.set_analog(0x7f, a7f);
            c.set_analog(0x3c, a3c);
        });
        init();
        let w: Vec<u8> = sim::with(|c| c.analog_writes()).iter().filter(|(a, _)| *a == 0x3c).map(|(_, v)| *v).collect();
        assert_eq!(w, cleared.into_iter().collect::<Vec<_>>(), "{a7f:#x} {a3c:#x}");
        let read_3c = sim::with(|c| c.log.iter().any(|o| *o == Op::AnaR(0x3c, a3c)));
        assert_eq!(read_3c, a7f & 1 != 0);
    }
}

#[test]
fn the_24_mhz_rc_calibration_waits_for_its_done_bit() {
    let _s = sim::session();
    let mut polls = 0;
    sim::with(|c| {
        c.set_analog(0xcf, 0x7f);
        c.on(move |c, op| {
            if op == Op::W8(reg::ANA_CTRL, 0x40) && c.peek8(reg::ANA_ADDR) == 0xcf {
                polls += 1;
                if polls == 3 {
                    c.set_analog(0xcf, 0x80);
                }
            }
        })
    });
    init();
    assert_eq!(sim::with(|c| c.log.iter().filter(|o| matches!(o, Op::AnaR(0xcf, _))).count()), 4);
}
