use memcap as _;
use bulb_core::command::PowerOnBehavior;
use bulb_core::{Command, StartupLevel};
use bulb_sim::Harness;

fn set_pob(h: &mut Harness, b: PowerOnBehavior) {
    h.command(Command::SetPowerOnBehavior(b));
}

#[test]
fn first_boot_on_blank_flash_is_on_at_full_brightness() {
    let h = Harness::new();
    assert!(h.is_on());
    assert_eq!(h.level(), 254);
    assert_eq!(h.state().power_on_behavior, PowerOnBehavior::On);
}

#[test]
fn on_off_toggle_commands() {
    let mut h = Harness::new();
    h.command(Command::On);
    assert!(h.is_on());
    assert!(h.level() > 0);
    h.command(Command::Off);
    assert!(!h.is_on());
    assert_eq!(h.level(), 0);
    h.command(Command::Toggle);
    assert!(h.is_on());
    h.command(Command::Toggle);
    assert!(!h.is_on());
}

#[test]
fn move_to_level_sets_brightness_immediately() {
    let mut h = Harness::new();
    h.command(Command::MoveToLevel { level: 128, transition_ms: 0 });
    assert!(h.is_on());
    assert_eq!(h.level(), 128);
    h.command(Command::MoveToLevel { level: 0, transition_ms: 0 });
    assert!(!h.is_on());
}

#[test]
fn smooth_transition_reaches_target() {
    let mut h = Harness::new();
    h.command(Command::Off);
    h.command(Command::MoveToLevel { level: 254, transition_ms: 1000 });
    assert!(h.level() < 254);
    h.run_for(1000, 50);
    assert_eq!(h.level(), 254);
}

#[test]
fn power_on_off_stays_off() {
    let mut h = Harness::new();
    set_pob(&mut h, PowerOnBehavior::Off);
    h.command(Command::On);
    assert!(h.is_on());
    h.power_cycle();
    assert!(!h.is_on(), "with Off it must come up off");
}

#[test]
fn power_on_on_turns_on() {
    let mut h = Harness::new();
    set_pob(&mut h, PowerOnBehavior::On);
    h.command(Command::Off);
    h.power_cycle();
    assert!(h.is_on(), "with On it must come up on");
}

#[test]
fn power_on_previous_restores_state() {
    let mut h = Harness::new();
    set_pob(&mut h, PowerOnBehavior::Previous);
    h.command(Command::MoveToLevel { level: 200, transition_ms: 0 });
    h.power_cycle();
    assert!(h.is_on(), "Previous: it was on, it must come back on");
    assert_eq!(h.level(), 200, "it must come back at the same brightness");

    h.command(Command::Off);
    h.power_cycle();
    assert!(!h.is_on(), "Previous: it was off, it must come back off");
}

#[test]
fn power_on_toggle_toggles() {
    let mut h = Harness::new();
    set_pob(&mut h, PowerOnBehavior::Toggle);
    h.command(Command::Off);
    h.power_cycle();
    assert!(h.is_on(), "Toggle from off -> on");
    h.power_cycle();
    assert!(!h.is_on(), "Toggle from on -> off");
}

#[test]
fn fixed_startup_level_independent_of_previous_brightness() {
    let mut h = Harness::new();
    set_pob(&mut h, PowerOnBehavior::On);
    h.command(Command::SetStartupLevel(60));
    h.command(Command::MoveToLevel { level: 254, transition_ms: 0 });
    h.power_cycle();
    assert!(h.is_on());
    assert_eq!(h.level(), 60, "it must come up at the fixed startup brightness, not 254");
}

#[test]
fn startup_level_0_comes_up_at_the_lowest_brightness() {
    let mut h = Harness::new();
    h.command(Command::SetStartupLevel(0));
    h.command(Command::MoveToLevel { level: 200, transition_ms: 0 });
    assert_eq!(h.state().startup_level, 0, "the attribute reads back as written");
    h.power_cycle();
    assert!(h.is_on());
    assert_eq!(h.level(), 1);
    assert_eq!(h.state().startup_level, 0);
}

#[test]
fn startup_level_previous_uses_last_brightness() {
    let mut h = Harness::new();
    set_pob(&mut h, PowerOnBehavior::On);
    h.command(Command::SetStartupLevel(0xFF));
    h.command(Command::MoveToLevel { level: 90, transition_ms: 0 });
    h.power_cycle();
    assert_eq!(h.level(), 90);
}

#[test]
fn config_survives_reboot() {
    let mut h = Harness::new();
    set_pob(&mut h, PowerOnBehavior::On);
    h.command(Command::SetStartupLevel(123));
    h.power_cycle();
    let s = h.state();
    assert_eq!(s.power_on_behavior, PowerOnBehavior::On);
    assert_eq!(StartupLevel::from_u8(s.startup_level), StartupLevel::Fixed(123));
}

#[test]
fn flash_is_not_rewritten_needlessly() {
    let mut h = Harness::new();
    let before = h.flash.write_count();
    h.command(Command::ReadState);
    assert_eq!(h.flash.write_count(), before, "ReadState should not write to flash");
}

#[test]
fn every_change_is_reported_to_network() {
    let mut h = Harness::new();
    h.command(Command::On);
    let r = h.radio.last_report().expect("there should be a report");
    assert!(r.on);
}

#[test]
fn multiple_commands_from_radio_queue_at_once() {
    let mut h = Harness::new();
    h.radio.send(Command::On);
    h.radio.send(Command::MoveToLevel { level: 150, transition_ms: 0 });
    h.radio.send(Command::MoveToLevel { level: 30, transition_ms: 0 });
    h.tick();
    assert!(h.is_on());
    assert_eq!(h.level(), 30, "the last command from the queue wins");
}

fn channels(h: &Harness) -> (u16, u16) {
    (h.bulb.pwm().duty, h.bulb.pwm().cold.unwrap())
}

#[test]
fn color_temperature_splits_the_brightness_between_two_channels() {
    let mut h = Harness::new_tunable();
    h.command(Command::MoveToLevel { level: 254, transition_ms: 0 });
    h.command(Command::MoveToColorTemperature { mireds: 454, transition_ms: 0 });
    assert_eq!(channels(&h), (1000, 0), "warmest: the warm channel alone");
    h.command(Command::MoveToColorTemperature { mireds: 153, transition_ms: 0 });
    assert_eq!(channels(&h), (0, 1000), "coldest: the cold channel alone");
    h.command(Command::MoveToColorTemperature { mireds: 300, transition_ms: 0 });
    let (warm, cold) = channels(&h);
    assert_eq!(warm as u32, 1000 * (300 - 153) / (454 - 153));
    assert_eq!(warm + cold, 1000);
    let mut dimm = Harness::new();
    for level in [1u8, 40, 127, 200] {
        h.command(Command::MoveToLevel { level, transition_ms: 0 });
        dimm.command(Command::MoveToLevel { level, transition_ms: 0 });
        let (warm, cold) = channels(&h);
        assert_eq!(warm + cold, bulb_core::curve::duty(level, 1000, true), "level {level}");
        assert_eq!(dimm.bulb.pwm().duty, bulb_core::curve::duty(level, 1000, false), "level {level}");
    }
    assert_eq!((bulb_core::curve::duty(128, 1000, true), bulb_core::curve::duty(128, 1000, false)), (198, 252));
    h.command(Command::MoveToColorTemperature { mireds: 1000, transition_ms: 0 });
    assert_eq!(h.state().color_temp, 454);
}

#[test]
fn color_temperature_fades_and_survives_a_power_cycle() {
    let mut h = Harness::new_tunable();
    h.command(Command::MoveToColorTemperature { mireds: 200, transition_ms: 0 });
    h.command(Command::MoveToColorTemperature { mireds: 400, transition_ms: 1000 });
    h.run_for(500, 50);
    let (warm, _) = channels(&h);
    let half = 1000 * (300 - 153) / (454 - 153);
    assert!((warm as i32 - half).abs() < 30, "halfway through: warm duty {warm}");
    h.run_for(600, 50);
    assert_eq!(channels(&h).0 as i32, (1000 * (400 - 153) + 150) / (454 - 153));
    h.power_cycle();
    assert_eq!(h.state().color_temp, 400);
    h.command(Command::SetStartupColorTemperature(250));
    h.command(Command::MoveToColorTemperature { mireds: 420, transition_ms: 0 });
    h.power_cycle();
    assert_eq!(h.state().color_temp, 250);
    assert_eq!(h.state().startup_color_temp, 250);
    h.command(Command::SetStartupColorTemperature(193));
    h.power_cycle();
    assert_eq!((h.state().color_temp, h.state().startup_color_temp), (193, 193));
    h.command(Command::SetStartupColorTemperature(0xFFFF));
    h.command(Command::MoveToColorTemperature { mireds: 301, transition_ms: 0 });
    h.power_cycle();
    assert_eq!(h.state().color_temp, 301);
}

#[test]
fn the_single_channel_bulb_ignores_color_temperature_on_its_output() {
    let mut h = Harness::new();
    h.command(Command::MoveToLevel { level: 254, transition_ms: 0 });
    h.command(Command::MoveToColorTemperature { mireds: 153, transition_ms: 0 });
    assert_eq!(h.bulb.pwm().duty, 1000);
}

#[test]
fn identify_blinks_and_then_restores_the_light() {
    let mut h = Harness::new();
    h.command(Command::MoveToLevel { level: 100, transition_ms: 0 });
    let steady = h.bulb.pwm().duty;
    let writes = h.flash.write_count();
    h.command(Command::Identify { times: 3 });
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..28 {
        h.run_for(100, 100);
        seen.insert(h.bulb.pwm().duty);
    }
    assert_eq!(seen.into_iter().collect::<Vec<_>>(), vec![0, 1000], "full brightness and dark, nothing else");
    assert!(h.bulb.is_identifying());
    h.run_for(400, 100);
    assert!(!h.bulb.is_identifying());
    assert_eq!(h.bulb.pwm().duty, steady);
    assert_eq!(h.flash.write_count(), writes, "identifying is not a state worth storing");
    h.command(Command::Identify { times: 60 });
    h.run_for(700, 100);
    h.command(Command::Identify { times: 0 });
    assert_eq!(h.bulb.pwm().duty, steady);
}

#[test]
fn stop_freezes_a_transition_where_it_is() {
    let mut h = Harness::new();
    h.command(Command::MoveToLevel { level: 10, transition_ms: 0 });
    h.command(Command::MoveToLevel { level: 210, transition_ms: 2000 });
    h.run_for(1000, 50);
    h.command(Command::StopTransition);
    let reached = h.level();
    assert!((100..=120).contains(&reached), "stopped at {reached}");
    h.run_for(2000, 50);
    assert_eq!(h.level(), reached);
    assert_eq!(h.state().level, reached);
}

#[test]
fn stopping_the_colour_and_stopping_the_brightness_are_separate() {
    let mut h = Harness::new_tunable();
    h.command(Command::MoveToLevel { level: 10, transition_ms: 0 });
    h.command(Command::MoveToColorTemperature { mireds: 153, transition_ms: 0 });
    h.command(Command::MoveToLevel { level: 210, transition_ms: 2000 });
    h.command(Command::MoveToColorTemperature { mireds: 454, transition_ms: 5000 });
    h.run_for(1000, 50);
    h.command(Command::StopColorTransition);
    let reached = h.state().color_temp;
    assert!((205..=220).contains(&reached), "stopped at {reached} mireds");
    h.run_for(5000, 50);
    assert_eq!(h.state().color_temp, reached, "the colour stays where it was stopped");
    assert_eq!(h.level(), 210, "the brightness was not this command's to stop");
    let (warm, cold) = channels(&h);
    assert_eq!(warm as u32, ((warm + cold) as u32 * (reached as u32 - 153) + 150) / (454 - 153));
    h.command(Command::MoveToLevel { level: 10, transition_ms: 2000 });
    h.command(Command::MoveToColorTemperature { mireds: 454, transition_ms: 1000 });
    h.run_for(500, 50);
    h.command(Command::StopTransition);
    let level = h.level();
    assert!((150..=170).contains(&level), "stopped at level {level}");
    h.run_for(2000, 50);
    assert_eq!((h.level(), h.state().color_temp), (level, 454));
    h.command(Command::StopColorTransition);
    assert_eq!(h.state().color_temp, 454);
}

#[test]
fn a_level_set_while_off_is_used_at_the_next_turn_on() {
    let mut h = Harness::new();
    h.command(Command::MoveToLevel { level: 200, transition_ms: 0 });
    h.command(Command::Off);
    h.command(Command::SetLevelWhileOff(40));
    assert!(!h.is_on());
    assert_eq!(h.bulb.pwm().duty, 0, "the light stays dark");
    assert_eq!(h.state().level, 40);
    h.command(Command::On);
    assert_eq!(h.level(), 40);
    h.command(Command::Off);
    h.command(Command::SetLevelWhileOff(90));
    h.power_cycle();
    assert!(h.is_on());
    assert_eq!(h.level(), 90);
    h.command(Command::SetLevelWhileOff(120));
    assert!(h.is_on());
    assert_eq!(h.level(), 120);
}
