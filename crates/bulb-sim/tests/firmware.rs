use memcap as _;

use std::cell::RefCell;
use std::rc::Rc;

use bulb_core::colour::{ColorMode, HueDirection};
use bulb_core::command::PowerOnBehavior;
use bulb_core::hal::ConfigStore;
use bulb_core::{Bulb, Command, Config, StartupLevel};
use bulb_sim::{Harness, MockClock, MockFlash, MockPwm, MockRadio};

#[derive(Clone, Default)]
struct Store {
    config: MockFlash,
    colour: Rc<RefCell<Option<Vec<u8>>>>,
}

impl ConfigStore for Store {
    fn load(&self, buf: &mut [u8]) -> Option<usize> {
        self.config.load(buf)
    }
    fn store(&mut self, data: &[u8]) {
        self.config.store(data)
    }
    fn load_colour(&self, buf: &mut [u8]) -> Option<usize> {
        let c = self.colour.borrow();
        let d = c.as_ref()?;
        buf[..d.len()].copy_from_slice(d);
        Some(d.len())
    }
    fn store_colour(&mut self, data: &[u8]) {
        *self.colour.borrow_mut() = Some(data.to_vec());
    }
}

type ColourBulb = Bulb<MockPwm, MockClock, Store, MockRadio>;

fn boot(pwm: MockPwm, store: &Store) -> (ColourBulb, MockRadio) {
    let radio = MockRadio::new();
    (Bulb::boot(pwm, MockClock::new(), store.clone(), radio.handle()), radio)
}

fn send(b: &mut ColourBulb, r: &MockRadio, cmd: Command) {
    r.send(cmd);
    b.tick();
}

#[test]
fn hue_and_saturation_survive_a_power_cycle() {
    let store = Store::default();
    let (mut b, r) = boot(MockPwm::colour(1000), &store);
    send(&mut b, &r, Command::MoveToHueSaturation { hue: 85, sat: 200, direction: HueDirection::Shortest, transition_ms: 0 });
    assert!(store.colour.borrow().is_some());
    let shown = b.pwm().rgb;
    let (b, _) = boot(MockPwm::colour(1000), &store);
    let s = b.state();
    assert_eq!((s.color_mode, s.hue, s.sat), (ColorMode::HueSaturation, 85, 200));
    assert_eq!(b.pwm().rgb, shown);
}

#[test]
fn xy_survives_a_power_cycle() {
    let store = Store::default();
    let (mut b, r) = boot(MockPwm::colour(1000), &store);
    send(&mut b, &r, Command::MoveToXy { x: 19661, y: 39321, transition_ms: 0 });
    let (b, _) = boot(MockPwm::colour(1000), &store);
    let s = b.state();
    assert_eq!((s.color_mode, s.x, s.y), (ColorMode::Xy, 19661, 39321));
}

#[test]
fn a_colour_temperature_survives_a_power_cycle_on_a_colour_bulb() {
    let store = Store::default();
    let (mut b, r) = boot(MockPwm::colour(1000), &store);
    send(&mut b, &r, Command::MoveToHueSaturation { hue: 85, sat: 200, direction: HueDirection::Shortest, transition_ms: 0 });
    send(&mut b, &r, Command::MoveToColorTemperature { mireds: 250, transition_ms: 0 });
    let (b, _) = boot(MockPwm::colour(1000), &store);
    assert_eq!((b.state().color_mode, b.state().color_temp), (ColorMode::Temperature, 250));
}

#[test]
fn a_white_bulb_keeps_no_colour() {
    let store = Store::default();
    let (mut b, r) = boot(MockPwm::tunable(1000), &store);
    send(&mut b, &r, Command::MoveToColorTemperature { mireds: 250, transition_ms: 0 });
    send(&mut b, &r, Command::MoveToHueSaturation { hue: 85, sat: 200, direction: HueDirection::Shortest, transition_ms: 0 });
    assert!(store.colour.borrow().is_none());
    *store.colour.borrow_mut() = Some(
        bulb_core::colour::ColourRecord { mode: ColorMode::HueSaturation, xy: false, hue: 9, sat: 9, x: 0, y: 0 }.encode().to_vec(),
    );
    let (b, _) = boot(MockPwm::tunable(1000), &store);
    assert_eq!(b.state().color_mode, ColorMode::Temperature);
}

#[test]
fn a_hue_move_is_kept_where_it_is_stopped() {
    let store = Store::default();
    let radio = MockRadio::new();
    let clock = MockClock::new();
    let mut b = Bulb::boot(MockPwm::colour(1000), clock.handle(), store.clone(), radio.handle());
    send(&mut b, &radio, Command::MoveToHueSaturation { hue: 10, sat: 254, direction: HueDirection::Shortest, transition_ms: 0 });
    let before = store.colour.borrow().clone();
    send(&mut b, &radio, Command::StopColorTransition);
    assert_eq!(*store.colour.borrow(), before, "nothing moved: nothing stored");
    send(&mut b, &radio, Command::MoveHue { up: true, rate: 0x0C00 });
    clock.advance(1000);
    b.tick();
    assert_eq!(*store.colour.borrow(), before, "a hue move is stored when it stops");
    send(&mut b, &radio, Command::StopColorTransition);
    let (b, _) = boot(MockPwm::colour(1000), &store);
    assert_eq!(b.state().hue, 22);
}

#[test]
fn the_end_of_a_transition_is_reported_once() {
    let mut h = Harness::new();
    h.command(Command::MoveToLevel { level: 100, transition_ms: 1000 });
    let n = h.radio.reports.borrow().len();
    h.run_for(999, 1);
    assert_eq!(h.radio.reports.borrow().len(), n);
    h.run_for(2000, 1);
    assert_eq!(h.radio.reports.borrow().len(), n + 1);
    assert_eq!(h.radio.last_report().unwrap().shown.level, 100);
}

#[test]
fn identify_blinks_bright_in_the_odd_half_seconds_before_the_end() {
    let mut h = Harness::new();
    h.command(Command::Identify { times: 2 });
    let mut duty = vec![];
    for _ in 0..2000 {
        duty.push(h.bulb.pwm().duty);
        h.clock.advance(1);
        h.tick();
    }
    let expect: Vec<u16> = (0..2000u64).map(|t| if (2000 - t) / 500 % 2 == 1 { 1000 } else { 0 }).collect();
    assert_eq!(duty[1..], expect[1..]);
    assert_eq!((duty[1], duty[500], duty[501], duty[1001], duty[1999]), (1000, 1000, 0, 1000, 0));
    assert!(!h.bulb.is_identifying());
    assert_eq!(h.bulb.pwm().duty, 1000);
}

#[test]
fn the_default_transition_is_used_by_on_and_off() {
    let mut h = Harness::new();
    h.bulb.set_default_transition(1000);
    h.command(Command::Off);
    assert!(!h.is_on());
    assert_eq!(h.level(), 254);
    h.run_for(500, 10);
    assert!((100..=160).contains(&h.level()), "{}", h.level());
    h.run_for(500, 10);
    assert_eq!(h.level(), 0);
}

#[test]
fn a_record_shorter_than_a_block_is_not_read() {
    let crc_zero = (1..=254u8)
        .flat_map(|level| (1..=200u8).map(move |s| (level, s)))
        .map(|(level, s)| Config {
            power_on_behavior: PowerOnBehavior::On,
            startup_level: StartupLevel::Fixed(s),
            last_on: true,
            last_level: level,
            ..Config::default()
        })
        .find(|c| c.encode()[7] == 0)
        .expect("a block whose CRC is 0");
    let flash = MockFlash::new();
    flash.handle().store(&crc_zero.encode()[..7]);
    let b = Bulb::boot(MockPwm::new(1000), MockClock::new(), flash.handle(), MockRadio::new());
    assert_eq!(b.current_level(), 254);
    flash.handle().store(&crc_zero.encode());
    let b = Bulb::boot(MockPwm::new(1000), MockClock::new(), flash.handle(), MockRadio::new());
    assert_ne!(b.current_level(), 254);
}

#[test]
fn the_candle_command_turns_the_candle_on_and_off() {
    let mut h = Harness::new();
    h.command(Command::Candle(true));
    assert!(h.state().candle);
    h.command(Command::Candle(false));
    assert!(!h.state().candle);
}
