//! Orchestrator: ties the HAL (PWM, clock, storage, radio) to the light logic and
//! the configuration. This is the firmware's "main loop" — hardware-independent.

use crate::command::{Command, PowerOnBehavior, StateReport};
use crate::config::{Config, StartupLevel};
use crate::hal::{Clock, ConfigStore, Pwm, Radio};
use crate::light::Light;

/// The bulb: state + configuration + hardware access through the HAL.
pub struct Bulb<P, C, S, R> {
    pwm: P,
    clock: C,
    store: S,
    radio: R,
    light: Light,
    config: Config,
    /// Default transition time for commands that have none of their own (ms).
    default_transition_ms: u32,
    /// Identify: blinking until this time.
    identify_until: Option<u64>,
}

impl<P, C, S, R> Bulb<P, C, S, R>
where
    P: Pwm,
    C: Clock,
    S: ConfigStore,
    R: Radio,
{
    /// Boot after power-on. Loads the configuration from storage and applies
    /// `power-on behavior` and `startup level` — like a real bulb after the
    /// wall switch is flicked.
    pub fn boot(pwm: P, clock: C, store: S, radio: R) -> Self {
        let mut buf = [0u8; crate::config::BLOCK_LEN];
        let config = match store.load(&mut buf) {
            Some(n) if n >= crate::config::BLOCK_LEN => {
                Config::decode(&buf).unwrap_or_default()
            }
            _ => Config::default(),
        };

        let mut light = Light::new();
        let (hue, sat) = pwm.colour_default();
        light.set_colour_default(hue, sat);
        let mut bulb = Bulb {
            pwm,
            clock,
            store,
            radio,
            light,
            config,
            default_transition_ms: 0,
            identify_until: None,
        };
        if bulb.pwm.has_rgb() {
            let mut b = [0u8; crate::colour::RECORD_LEN];
            if let Some(r) = bulb.store.load_colour(&mut b).and_then(|_| crate::colour::ColourRecord::decode(&b)) {
                bulb.light.restore_colour(&mut bulb.pwm, &r);
            }
        }
        bulb.apply_power_on();
        bulb
    }

    /// A colour bulb keeps its colour over a power cut, as the stock bulb does.
    fn persist_colour(&mut self) {
        if self.pwm.has_rgb() {
            self.store.store_colour(&self.light.colour_record().encode());
        }
    }

    fn now(&self) -> u64 {
        self.clock.now_ms()
    }

    /// Target brightness when turning on, according to `startup level`.
    fn startup_brightness(&self) -> u8 {
        match self.config.startup_level {
            StartupLevel::Previous => self.config.last_level,
            StartupLevel::Minimum => crate::LEVEL_MIN,
            StartupLevel::Fixed(v) => v,
        }
    }

    fn apply_power_on(&mut self) {
        let now = self.now();
        let bright = self.startup_brightness();
        let turn_on = match self.config.power_on_behavior {
            PowerOnBehavior::Off => false,
            PowerOnBehavior::On => true,
            PowerOnBehavior::Toggle => !self.config.last_on,
            PowerOnBehavior::Previous => self.config.last_on,
        };
        let ct = match self.config.startup_color_temp {
            crate::STARTUP_CT_PREVIOUS => self.config.last_color_temp,
            fixed => fixed,
        };
        // the colour temperature as configured (it does not change the
        // colour mode of a colour bulb that comes back in a colour)
        let mode = self.light.color_mode();
        self.light.move_to_color_temp(&mut self.pwm, now, ct, 0);
        if mode != crate::colour::ColorMode::Temperature {
            let r = crate::colour::ColourRecord { mode, ..self.light.colour_record() };
            self.light.restore_colour(&mut self.pwm, &r);
        }
        self.light.set_immediate(&mut self.pwm, now, turn_on, bright);
        self.persist_last();
        self.report();
    }

    fn persist_last(&mut self) {
        self.config.last_on = self.light.is_on();
        self.config.last_level = self.light.level();
        self.config.last_color_temp = self.light.color_temp();
        self.store.store(&self.config.encode());
    }

    fn report(&mut self) {
        self.radio.report(&self.state());
    }

    /// Current device state.
    pub fn state(&self) -> StateReport {
        StateReport {
            on: self.light.is_on(),
            level: self.light.level(),
            power_on_behavior: self.config.power_on_behavior,
            startup_level: self.config.startup_level.to_u8(),
            color_temp: self.light.color_temp(),
            startup_color_temp: self.config.startup_color_temp,
            color_mode: self.light.color_mode(),
            hue: self.light.colour().0,
            sat: self.light.colour().1,
            x: self.light.colour().2,
            y: self.light.colour().3,
            hue_moving: self.light.hue_moving(),
            candle: self.light.candle(),
            shown: self.light.shown(),
        }
    }

    pub fn is_on(&self) -> bool {
        self.light.is_on()
    }

    pub fn current_level(&self) -> u8 {
        self.light.current_level()
    }

    /// Handles a single command.
    pub fn handle(&mut self, cmd: Command) {
        let now = self.now();
        let t = self.default_transition_ms;
        match cmd {
            Command::On => self.light.turn_on_previous(&mut self.pwm, now, t),
            Command::Off => self.light.turn_off(&mut self.pwm, now, t),
            Command::Toggle => {
                if self.light.is_on() {
                    self.light.turn_off(&mut self.pwm, now, t);
                } else {
                    self.light.turn_on_previous(&mut self.pwm, now, t);
                }
            }
            Command::MoveToLevel { level, transition_ms } => {
                self.light.move_to_level(&mut self.pwm, now, level, transition_ms);
            }
            Command::SetStartupLevel(v) => {
                self.config.startup_level = StartupLevel::from_u8(v);
                self.store.store(&self.config.encode());
            }
            Command::SetPowerOnBehavior(b) => {
                self.config.power_on_behavior = b;
                self.store.store(&self.config.encode());
            }
            Command::Identify { times } => {
                // `times` seconds of blinking; 0 stops it. Not a change of state:
                // nothing is stored or reported.
                self.identify_until = if times == 0 { None } else { Some(now + times as u64 * 1000) };
                if times == 0 {
                    self.light.refresh(&mut self.pwm);
                }
                return;
            }
            Command::MoveToColorTemperature { mireds, transition_ms } => {
                self.light.move_to_color_temp(&mut self.pwm, now, mireds, transition_ms);
                self.persist_colour();
            }
            Command::MoveToHueSaturation { hue, sat, direction, transition_ms } => {
                if self.pwm.has_rgb() {
                    self.light.move_to_hue_sat(&mut self.pwm, now, hue, sat, direction, transition_ms);
                    self.persist_colour();
                }
            }
            Command::MoveToXy { x, y, transition_ms } => {
                if self.pwm.has_rgb() {
                    self.light.move_to_xy(&mut self.pwm, now, x, y, transition_ms);
                    self.persist_colour();
                }
            }
            Command::Candle(on) => self.light.set_candle(&mut self.pwm, now, on),
            Command::MoveHue { up, rate } => {
                if self.pwm.has_rgb() {
                    self.light.move_hue(&mut self.pwm, now, up, rate);
                }
            }
            Command::SetStartupColorTemperature(mireds) => {
                self.config.startup_color_temp = match mireds {
                    crate::STARTUP_CT_PREVIOUS => mireds,
                    m => {
                        let (lo, hi) = self.pwm.ct_range();
                        m.clamp(lo, hi)
                    }
                };
                self.store.store(&self.config.encode());
            }
            Command::ReadState => {}
            Command::StopTransition => self.light.stop(&mut self.pwm),
            Command::StopColorTransition => {
                let moving = self.light.hue_moving();
                self.light.stop_color_temp(&mut self.pwm);
                if moving {
                    // a turning hue stops where it is: that is the colour now
                    self.persist_colour();
                }
            }
            Command::SetLevelWhileOff(level) => self.light.set_level_while_off(&mut self.pwm, now, level),
        }
        // After every command we persist the last state and report.
        if !matches!(cmd, Command::ReadState) {
            self.persist_last();
        }
        self.report();
    }

    /// Processes pending commands from the radio and advances smooth transitions.
    /// Called from the firmware's main loop.
    pub fn tick(&mut self) {
        while let Some(cmd) = self.radio.poll() {
            self.handle(cmd);
        }
        let now = self.now();
        let moving = self.light.in_transition();
        self.light.tick(&mut self.pwm, now);
        if moving && !self.light.in_transition() {
            // where it was headed is where it is: said once more (the AwoX app's status)
            self.report();
        }
        if let Some(until) = self.identify_until {
            if now >= until {
                self.identify_until = None;
                self.light.refresh(&mut self.pwm);
            } else {
                // half a second bright, half a second dark
                let bright = (until - now) / 500 % 2 == 1;
                let max = self.pwm.max_duty();
                self.pwm.set_duty(if bright { max } else { 0 });
                self.pwm.set_cold_duty(0);
            }
        }
    }

    /// Whether the bulb is blinking to identify itself.
    pub fn is_identifying(&self) -> bool {
        self.identify_until.is_some()
    }

    /// Sets the default transition time for on/off/toggle commands (ms).
    pub fn set_default_transition(&mut self, ms: u32) {
        self.default_transition_ms = ms;
    }

    /// The PWM outputs (for tests and diagnostics).
    pub fn pwm(&self) -> &P {
        &self.pwm
    }

    /// Breaks the bulb down into its HAL parts (e.g. for a "power cycle" in tests/the emulator).
    pub fn into_parts(self) -> (P, C, S, R) {
        (self.pwm, self.clock, self.store, self.radio)
    }
}
