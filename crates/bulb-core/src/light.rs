//! Light logic: on/off state, brightness and a smooth transition (fade) over time.
//!
//! Everything uses integers (the firmware has no FPU). Brightness 1..=254 is
//! mapped to PWM duty through a gamma curve (perceived brightness grows
//! non-linearly), which mirrors the behaviour of a real bulb.

use crate::colour::{self, ColorMode, ColourRecord, HueDirection};
use crate::hal::Pwm;

/// Hardware-independent light model; computes the duty itself and applies it to the PWM.
#[derive(Debug, Clone, Copy)]
pub struct Light {
    on: bool,
    /// Last non-zero brightness 1..=254 (as in Zigbee — remembered after turning off).
    level: u8,
    // Smooth transition as interpolation over time (hundredths of a level for precision).
    cur_centi: i32,    // current brightness *100
    start_centi: i32,  // brightness *100 at the start of the transition
    target_centi: i32, // target brightness *100
    start_ms: u64,     // transition start time
    end_ms: u64,       // transition end time (hard deadline)
    // Colour temperature in mireds (tunable-white bulbs), with its own transition.
    ct: u16,
    ct_start: u16,
    ct_target: u16,
    ct_start_ms: u64,
    ct_end_ms: u64,
    // Colour bulbs: the colour mode, the colour the colour channels show
    // (hue and saturation, or CIE xy) and its own transition.
    mode: ColorMode,
    xy: bool,
    hue: u8,
    sat: u8,
    x: u16,
    y: u16,
    hs_from: (u8, u8),
    hue_delta: i16,
    hs_to: (u8, u8),
    xy_from: (u16, u16),
    xy_to: (u16, u16),
    col_start_ms: u64,
    col_end_ms: u64,
    // The x and y last taken to hue and saturation: the stock firmware takes
    // an xy to hue and saturation only when it differs from this one.
    xy_seen: (u16, u16),
    // In the xy mode, the colour channels still show the hue and the
    // saturation: after an xy command from another mode, until something
    // else draws the light (as the stock firmware).
    hs_shown: bool,
    // CurrentX and CurrentY read 0 after a colour temperature command in
    // the xy mode (the stock firmware clears them; the light keeps them).
    xy_cleared: bool,
    // A hue turning round (Enhanced Move Hue): enhanced hue steps (1/256 of
    // a hue step) a second, signed, 0 when still; from where and since when.
    hue_rate: i32,
    enh_start: u32,
    hue_start_ms: u64,
}

const CENTI: i32 = 100;

impl Light {
    pub fn new() -> Self {
        Light {
            on: false,
            level: crate::LEVEL_MAX,
            cur_centi: 0,
            start_centi: 0,
            target_centi: 0,
            start_ms: 0,
            end_ms: 0,
            ct: crate::CT_DEFAULT,
            ct_start: crate::CT_DEFAULT,
            ct_target: crate::CT_DEFAULT,
            ct_start_ms: 0,
            ct_end_ms: 0,
            mode: ColorMode::Temperature,
            xy: false,
            hue: 170,
            sat: 254,
            x: 0,
            y: 0,
            hs_from: (170, 254),
            hue_delta: 0,
            hs_to: (170, 254),
            xy_from: (0, 0),
            xy_to: (0, 0),
            col_start_ms: 0,
            col_end_ms: 0,
            xy_seen: (0, 0),
            hs_shown: false,
            xy_cleared: false,
            hue_rate: 0,
            enh_start: 0,
            hue_start_ms: 0,
        }
    }

    /// The colour a factory-new colour bulb shows when its colour channels
    /// first come on (`hal::Pwm::colour_default`).
    pub fn set_colour_default(&mut self, hue: u8, sat: u8) {
        (self.hue, self.sat) = (hue, sat);
        self.hs_from = (hue, sat);
        self.hs_to = (hue, sat);
    }

    pub fn color_mode(&self) -> ColorMode {
        self.mode
    }

    /// Hue, saturation, x and y the colour is set to (the targets of a running transition).
    pub fn colour(&self) -> (u8, u8, u16, u16) {
        let (x, y) = if self.xy_cleared { (0, 0) } else { self.xy_to };
        (self.hs_to.0, self.hs_to.1, x, y)
    }

    /// What a colour bulb keeps over a power cut.
    pub fn colour_record(&self) -> ColourRecord {
        let (hue, sat) = self.hs_to;
        let (x, y) = self.xy_to;
        ColourRecord { mode: self.mode, xy: self.xy, hue, sat, x, y }
    }

    /// The colour after a power cut, at once.
    pub fn restore_colour<P: Pwm>(&mut self, pwm: &mut P, r: &ColourRecord) {
        self.mode = r.mode;
        self.xy = r.xy;
        (self.hue, self.sat, self.x, self.y) = (r.hue, r.sat, r.x, r.y);
        self.hs_from = (r.hue, r.sat);
        self.hs_to = (r.hue, r.sat);
        self.hue_delta = 0;
        self.xy_from = (r.x, r.y);
        self.xy_to = (r.x, r.y);
        self.xy_seen = (r.x, r.y);
        self.xy_cleared = false;
        self.hue_rate = 0;
        self.col_end_ms = self.col_start_ms;
        self.render(pwm);
    }

    fn colour_moving(&self) -> bool {
        (self.hue, self.sat) != self.hs_to || (self.x, self.y) != self.xy_to
    }

    /// Moves to a hue and a saturation, the hue the given way round.
    pub fn move_to_hue_sat<P: Pwm>(&mut self, pwm: &mut P, now_ms: u64, hue: u8, sat: u8, dir: HueDirection, transition_ms: u32) {
        let hue = hue.min(254);
        self.hue_rate = 0;
        if self.mode == ColorMode::Xy {
            // the stock firmware takes the xy it leaves to hue and saturation
            (self.hue, self.sat) = colour::xy_hue_sat(self.x, self.y);
        }
        self.mode = ColorMode::HueSaturation;
        if self.xy {
            // from xy the move starts at the hue and saturation as they are set
            self.xy = false;
            self.xy_from = (self.x, self.y);
            self.xy_to = (self.x, self.y);
        }
        self.hs_from = (self.hue, self.sat);
        self.hue_delta = dir.delta(self.hue, hue);
        self.hs_to = (hue, sat);
        self.col_start_ms = now_ms;
        self.col_end_ms = now_ms + transition_ms as u64;
        if transition_ms == 0 {
            (self.hue, self.sat) = self.hs_to;
        }
        self.render(pwm);
    }

    /// Moves to CIE x and y, as the stock firmware does:
    /// - x and y both 0 (no colour at all) leave the light as it is;
    /// - an xy other than the last one taken to hue and saturation sets those
    ///   too (`colour::xy_hue_sat`);
    /// - from the xy mode (and always on a bulb whose colour is a light of
    ///   its own) the light moves along xy;
    /// - from another mode x and y are set at once and the colour channels
    ///   show the hue and the saturation, moving to the new ones; whatever
    ///   draws the light next (a level, the next xy) draws it from xy.
    pub fn move_to_xy<P: Pwm>(&mut self, pwm: &mut P, now_ms: u64, x: u16, y: u16, transition_ms: u32) {
        if x == 0 && y == 0 {
            return;
        }
        self.hue_rate = 0;
        let from_xy = self.mode == ColorMode::Xy || pwm.rgb_beside_white();
        self.mode = ColorMode::Xy;
        self.xy = true;
        self.xy_cleared = false;
        let target = if (x, y) != self.xy_seen {
            self.xy_seen = (x, y);
            Some(colour::xy_hue_sat(x, y))
        } else {
            None
        };
        self.col_start_ms = now_ms;
        self.col_end_ms = now_ms + transition_ms as u64;
        if from_xy {
            self.hs_shown = false;
            if let Some(hs) = target {
                (self.hue, self.sat) = hs;
                self.hs_from = hs;
                self.hs_to = hs;
                self.hue_delta = 0;
            }
            self.xy_from = (self.x, self.y);
            self.xy_to = (x, y);
            if transition_ms == 0 {
                (self.x, self.y) = self.xy_to;
            }
        } else {
            self.hs_shown = true;
            (self.x, self.y) = (x, y);
            self.xy_from = (x, y);
            self.xy_to = (x, y);
            if let Some(hs) = target {
                self.hs_from = (self.hue, self.sat);
                self.hue_delta = HueDirection::Shortest.delta(self.hue, hs.0);
                self.hs_to = hs;
                if transition_ms == 0 {
                    (self.hue, self.sat) = hs;
                }
            }
        }
        self.apply(pwm);
    }

    /// Turns the hue round and round, up or down, at `rate` enhanced hue
    /// steps (1/256 of a hue step) a second, from where it is, until a
    /// colour command or a stop (Enhanced Move Hue, the AwoX app's colour
    /// animation; the stock firmware moves its enhanced hue the same way and
    /// shows its top byte). The saturation stays. A rate of 0 stops.
    pub fn move_hue<P: Pwm>(&mut self, pwm: &mut P, now_ms: u64, up: bool, rate: u16) {
        if rate == 0 {
            self.stop_color_temp(pwm);
            return;
        }
        if self.mode == ColorMode::Xy {
            (self.hue, self.sat) = colour::xy_hue_sat(self.x, self.y);
        }
        self.mode = ColorMode::HueSaturation;
        self.xy = false;
        self.xy_from = (self.x, self.y);
        self.xy_to = (self.x, self.y);
        // a colour transition on its way stops where it is
        self.hs_from = (self.hue, self.sat);
        self.hs_to = (self.hue.min(254), self.sat);
        self.hue_delta = 0;
        self.col_end_ms = self.col_start_ms;
        self.hue_rate = if up { rate as i32 } else { -(rate as i32) };
        self.enh_start = (self.hue as u32) << 8;
        self.hue_start_ms = now_ms;
        self.render(pwm);
    }

    /// Whether the hue is turning (`move_hue`).
    pub fn hue_moving(&self) -> bool {
        self.hue_rate != 0
    }

    /// Colour temperature the light is set to (the target of a running transition), in mireds.
    pub fn color_temp(&self) -> u16 {
        self.ct_target
    }

    /// Sets the colour temperature with a transition time.
    pub fn move_to_color_temp<P: Pwm>(&mut self, pwm: &mut P, now_ms: u64, mireds: u16, transition_ms: u32) {
        if self.mode == ColorMode::Xy && pwm.has_rgb() {
            self.xy_cleared = true;
        }
        self.hue_rate = 0;
        self.mode = ColorMode::Temperature;
        self.ct_start = self.ct;
        let (lo, hi) = pwm.ct_range();
        self.ct_target = mireds.clamp(lo, hi);
        self.ct_start_ms = now_ms;
        self.ct_end_ms = now_ms + transition_ms as u64;
        if transition_ms == 0 {
            self.ct = self.ct_target;
        }
        self.render(pwm);
    }

    /// Writes the current state to the outputs again (after something else drove them).
    pub fn refresh<P: Pwm>(&mut self, pwm: &mut P) {
        self.render(pwm);
    }

    pub fn is_on(&self) -> bool {
        self.on
    }

    /// Last non-zero brightness (1..=254).
    pub fn level(&self) -> u8 {
        self.level
    }

    /// Current (instantaneous) brightness during a transition, 0..=254.
    pub fn current_level(&self) -> u8 {
        (self.cur_centi / CENTI).clamp(0, crate::LEVEL_MAX as i32) as u8
    }

    /// Whether a smooth transition is still in progress.
    pub fn is_fading(&self) -> bool {
        self.cur_centi != self.target_centi
    }

    fn start_fade(&mut self, now_ms: u64, target_level: i32, transition_ms: u32) {
        self.start_centi = self.cur_centi;
        self.target_centi = target_level * CENTI;
        self.start_ms = now_ms;
        self.end_ms = now_ms + transition_ms as u64;
        if transition_ms == 0 {
            self.cur_centi = self.target_centi;
        }
    }

    /// Turn on with target brightness `level` (1..=254) and a transition time.
    pub fn turn_on<P: Pwm>(&mut self, pwm: &mut P, now_ms: u64, level: u8, transition_ms: u32) {
        let lvl = level.clamp(1, crate::LEVEL_MAX);
        self.on = true;
        self.level = lvl;
        self.start_fade(now_ms, lvl as i32, transition_ms);
        self.render(pwm);
    }

    /// Turn on to the remembered brightness.
    pub fn turn_on_previous<P: Pwm>(&mut self, pwm: &mut P, now_ms: u64, transition_ms: u32) {
        let lvl = self.level;
        self.turn_on(pwm, now_ms, lvl, transition_ms);
    }

    /// Turn off (with a smooth fade down to zero), remembering the last brightness.
    pub fn turn_off<P: Pwm>(&mut self, pwm: &mut P, now_ms: u64, transition_ms: u32) {
        self.on = false;
        self.start_fade(now_ms, 0, transition_ms);
        self.render(pwm);
    }

    /// Set brightness 1..=254; 0 is treated as turning off.
    pub fn move_to_level<P: Pwm>(&mut self, pwm: &mut P, now_ms: u64, level: u8, transition_ms: u32) {
        if level == 0 {
            self.turn_off(pwm, now_ms, transition_ms);
        } else {
            self.turn_on(pwm, now_ms, level, transition_ms);
        }
    }

    /// Sets the brightness the light will come on with, leaving it off. On a
    /// light that is on this is a plain change of brightness.
    pub fn set_level_while_off<P: Pwm>(&mut self, pwm: &mut P, now_ms: u64, level: u8) {
        if self.on {
            self.move_to_level(pwm, now_ms, level, 0);
        } else {
            self.level = level.clamp(1, crate::LEVEL_MAX);
        }
    }

    /// Stops a running brightness transition where it is. A light that was
    /// fading up or down keeps the brightness it has reached as its level; one
    /// that was fading out is left on at that brightness. A colour
    /// temperature on its way is not this command's to stop.
    pub fn stop<P: Pwm>(&mut self, pwm: &mut P) {
        if !self.is_fading() {
            return;
        }
        let reached = self.current_level();
        if reached == 0 {
            self.on = false;
            self.cur_centi = 0;
        } else {
            self.on = true;
            self.level = reached;
            self.cur_centi = reached as i32 * CENTI;
        }
        self.target_centi = self.cur_centi;
        self.render(pwm);
    }

    /// Stops a running colour temperature or colour transition where it
    /// is; the brightness is left alone.
    pub fn stop_color_temp<P: Pwm>(&mut self, pwm: &mut P) {
        self.hue_rate = 0;
        self.ct_target = self.ct;
        self.hs_to = (self.hue, self.sat);
        self.xy_to = (self.x, self.y);
        self.render(pwm);
    }

    /// Sets the state immediately without a transition (e.g. at startup).
    pub fn set_immediate<P: Pwm>(&mut self, pwm: &mut P, now_ms: u64, on: bool, level: u8) {
        if on {
            self.turn_on(pwm, now_ms, level, 0);
        } else {
            self.level = level.clamp(1, crate::LEVEL_MAX);
            self.on = false;
            self.cur_centi = 0;
            self.start_centi = 0;
            self.target_centi = 0;
            self.start_ms = now_ms;
            self.end_ms = now_ms;
            self.render(pwm);
        }
    }

    /// Advances the smooth transition according to elapsed time and applies the PWM.
    pub fn tick<P: Pwm>(&mut self, pwm: &mut P, now_ms: u64) {
        if self.cur_centi == self.target_centi && self.ct == self.ct_target && !self.colour_moving() && self.hue_rate == 0 {
            return;
        }
        if self.hue_rate != 0 {
            let elapsed = now_ms.saturating_sub(self.hue_start_ms) as i64;
            let enh = (self.enh_start as i64 + self.hue_rate as i64 * elapsed / 1000).rem_euclid(1 << 16);
            self.hue = (enh >> 8) as u8;
            // what is reported is where the hue is
            self.hs_to = (self.hue.min(254), self.sat);
            self.hs_from = self.hs_to;
        }
        // Linear interpolation from (start_ms, start_centi) to (end_ms, target_centi);
        // past the deadline it snaps to the target, so it always lands exactly.
        if self.cur_centi != self.target_centi {
            self.cur_centi = if now_ms >= self.end_ms {
                self.target_centi
            } else {
                let span = (self.end_ms - self.start_ms) as i64;
                let elapsed = (now_ms - self.start_ms) as i64;
                let delta = (self.target_centi - self.start_centi) as i64;
                (self.start_centi as i64 + delta * elapsed / span) as i32
            };
        }
        if self.ct != self.ct_target {
            self.ct = if now_ms >= self.ct_end_ms {
                self.ct_target
            } else {
                let span = (self.ct_end_ms - self.ct_start_ms) as i64;
                let elapsed = (now_ms - self.ct_start_ms) as i64;
                let delta = self.ct_target as i64 - self.ct_start as i64;
                (self.ct_start as i64 + delta * elapsed / span) as u16
            };
        }
        if self.colour_moving() {
            if now_ms >= self.col_end_ms {
                (self.hue, self.sat) = self.hs_to;
                (self.x, self.y) = self.xy_to;
            } else {
                let span = (self.col_end_ms - self.col_start_ms) as i64;
                let elapsed = (now_ms - self.col_start_ms) as i64;
                let along = |from: i64, delta: i64| from + delta * elapsed / span;
                if self.xy && !self.hs_shown {
                    self.x = along(self.xy_from.0 as i64, self.xy_to.0 as i64 - self.xy_from.0 as i64) as u16;
                    self.y = along(self.xy_from.1 as i64, self.xy_to.1 as i64 - self.xy_from.1 as i64) as u16;
                } else {
                    self.hue = along(self.hs_from.0 as i64, self.hue_delta as i64).rem_euclid(255) as u8;
                    self.sat = along(self.hs_from.1 as i64, self.hs_to.1 as i64 - self.hs_from.1 as i64) as u8;
                }
            }
        }
        self.apply(pwm);
    }

    /// Draws the light anew: from xy in the xy mode.
    fn render<P: Pwm>(&mut self, pwm: &mut P) {
        self.hs_shown = false;
        self.apply(pwm);
    }

    fn apply<P: Pwm>(&mut self, pwm: &mut P) {
        let level = self.current_level();
        if pwm.has_rgb() {
            // a colour bulb: the colour channels in a colour mode, the white
            // ones in the colour temperature mode; both beside each other on
            // a bulb whose colour is a light of its own (the strip at full value)
            let beside = pwm.rgb_beside_white();
            let colour = beside || self.mode != ColorMode::Temperature;
            let white = beside || self.mode == ColorMode::Temperature;
            let v = if beside { crate::LEVEL_MAX } else { level };
            let rgb = if !colour || level == 0 {
                [0; 3]
            } else if self.xy && !self.hs_shown {
                colour::xy_duties(self.x, self.y, v, pwm.max_duty())
            } else {
                colour::hs_duties(self.hue, self.sat, v, pwm.max_duty())
            };
            pwm.set_rgb(rgb[0], rgb[1], rgb[2]);
            self.apply_white(pwm, if white { level } else { 0 });
        } else {
            self.apply_white(pwm, level);
        }
    }

    fn apply_white<P: Pwm>(&mut self, pwm: &mut P, level: u8) {
        let duty = crate::curve::duty(level, pwm.max_duty(), pwm.white_tw_curve());
        if pwm.has_cold_channel() {
            // The brightness curve gives the sum of both channels; the colour
            // temperature splits it linearly in mireds (as the stock bulb does).
            let (warm, _) = split_duty(duty, self.ct, pwm.ct_range());
            let warm = warm_dead_band(warm, pwm.max_duty()).min(duty);
            pwm.set_duty(warm);
            pwm.set_cold_duty(duty - warm);
        } else {
            pwm.set_duty(duty);
        }
    }
}

/// Splits a total duty between the warm and the cold channel for a colour
/// temperature in mireds: all warm at the top of `range`, all cold at its bottom.
pub fn split_duty(total: u16, mireds: u16, range: (u16, u16)) -> (u16, u16) {
    let (lo, hi) = range;
    let m = mireds.clamp(lo, hi);
    // rounded to the nearest step, as the stock firmware does
    let span = (hi - lo).max(1) as u32;
    let warm = ((total as u32 * (m - lo) as u32 + span / 2) / span) as u16;
    (warm, total - warm)
}

/// The stock firmware's dead band of the warm channel (measured in the
/// emulator, docs/stock-profiles.md): a duty is never within 1.5 % of the
/// period from either end. Below 0.75 % it is 0, from there to 1.5 % it is
/// 1.5 %, and the same at the top (98.5 % or 100 %); the cold channel takes
/// what is left of the total.
pub fn warm_dead_band(warm: u16, max: u16) -> u16 {
    let edge = (max as u32 * 15 / 1000) as u16;
    let half = edge / 2;
    if warm < half {
        0
    } else if warm < edge {
        edge
    } else if warm > max - half {
        max
    } else if warm > max - edge {
        max - edge
    } else {
        warm
    }
}

impl Default for Light {
    fn default() -> Self {
        Self::new()
    }
}

/// Duty of the dimmable bulb for a brightness level: the stock bulb's curve
/// (`curve::DIMM`), roughly duty = max * (level/254)^2.
pub fn gamma_duty(level: u8, max_duty: u16) -> u16 {
    crate::curve::duty(level, max_duty, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestPwm(u16);
    impl Pwm for TestPwm {
        fn max_duty(&self) -> u16 {
            1000
        }
        fn set_duty(&mut self, d: u16) {
            self.0 = d;
        }
    }

    /// Tunable white on a 48000-tick PWM, warm channel first.
    struct TwPwm(u16, u16, (u16, u16));
    impl Pwm for TwPwm {
        fn max_duty(&self) -> u16 {
            48000
        }
        fn set_duty(&mut self, d: u16) {
            self.0 = d;
        }
        fn has_cold_channel(&self) -> bool {
            true
        }
        fn set_cold_duty(&mut self, d: u16) {
            self.1 = d;
        }
        fn ct_range(&self) -> (u16, u16) {
            self.2
        }
    }

    /// A colour bulb on a 48000-tick PWM: red, green and blue, beside the
    /// whites or not.
    struct RgbPwm {
        rgb: [u16; 3],
        beside: bool,
    }
    impl Pwm for RgbPwm {
        fn max_duty(&self) -> u16 {
            48000
        }
        fn set_duty(&mut self, _d: u16) {}
        fn has_rgb(&self) -> bool {
            true
        }
        fn set_rgb(&mut self, r: u16, g: u16, b: u16) {
            self.rgb = [r, g, b];
        }
        fn rgb_beside_white(&self) -> bool {
            self.beside
        }
    }

    fn rgb_light(beside: bool) -> (Light, RgbPwm) {
        let mut pwm = RgbPwm { rgb: [0; 3], beside };
        let mut l = Light::new();
        l.set_immediate(&mut pwm, 0, true, 254);
        (l, pwm)
    }

    const RED: (u16, u16) = (41943, 21627);
    const GREEN: (u16, u16) = (19661, 39321);

    // what the stock RGB_TW and RGB_W firmware shows and reports (emulator)
    #[test]
    fn an_xy_from_another_mode_shows_its_hue_and_saturation_until_redrawn() {
        let (mut l, mut p) = rgb_light(false);
        l.move_to_hue_sat(&mut p, 0, 0, 254, HueDirection::Shortest, 0);
        assert_eq!(p.rgb, [48000, 0, 0]);
        l.move_to_xy(&mut p, 0, RED.0, RED.1, 0);
        assert_eq!((p.rgb, l.colour(), l.color_mode()), ([48000, 0, 0], (0, 255, RED.0, RED.1), ColorMode::Xy));
        l.move_to_level(&mut p, 0, 200, 0);
        assert_eq!(p.rgb, [29669, 0, 0]);
        l.move_to_level(&mut p, 0, 254, 0);
        assert_eq!(p.rgb, [48377, 0, 0], "drawn from xy now");
        l.move_to_xy(&mut p, 0, GREEN.0, GREEN.1, 0);
        assert_eq!((p.rgb, l.colour()), ([0, 48377, 0], (85, 255, GREEN.0, GREEN.1)), "xy to xy: along xy");
        l.move_to_hue_sat(&mut p, 0, 85, 254, HueDirection::Shortest, 0);
        assert_eq!(p.rgb, [0, 48000, 0]);
        l.move_to_xy(&mut p, 0, GREEN.0, GREEN.1, 0);
        assert_eq!((p.rgb, l.colour()), ([0, 48000, 0], (85, 254, GREEN.0, GREEN.1)), "the xy last taken: the hue stays");
        l.move_to_level(&mut p, 0, 128, 0);
        l.move_to_xy(&mut p, 0, 20454, 17992, 0);
        assert_eq!(p.rgb, [5858, 2078, 6425]);
        assert_eq!(l.colour(), (213, 72, 20454, 17992));
    }

    #[test]
    fn a_colour_temperature_from_xy_clears_x_and_y() {
        let (mut l, mut p) = rgb_light(false);
        l.move_to_xy(&mut p, 0, GREEN.0, GREEN.1, 0);
        l.move_to_color_temp(&mut p, 0, 300, 0);
        assert_eq!((p.rgb, l.colour()), ([0; 3], (85, 255, 0, 0)));
        l.move_to_xy(&mut p, 0, RED.0, RED.1, 0);
        assert_eq!((p.rgb, l.colour()), ([48000, 0, 0], (0, 255, RED.0, RED.1)));
        l.move_to_color_temp(&mut p, 0, 250, 0);
        l.move_to_hue_sat(&mut p, 0, 0, 100, HueDirection::Shortest, 0);
        l.move_to_xy(&mut p, 0, RED.0, RED.1, 0);
        assert_eq!(l.colour(), (0, 100, RED.0, RED.1), "red was the xy last taken");
    }

    // the stock RGB_TW firmware, the app's colour animation (Enhanced Move Hue up, 0x0C00 a second)
    #[test]
    fn the_hue_turns_until_stopped() {
        let (mut l, mut p) = rgb_light(false);
        l.move_to_hue_sat(&mut p, 0, 85, 254, HueDirection::Shortest, 0);
        l.move_hue(&mut p, 1000, true, 0x0C00);
        assert!(l.hue_moving());
        l.tick(&mut p, 1000 + 6270);
        assert_eq!(l.colour().0, 160, "where the stock bulb is after 6.27 s");
        assert_eq!(p.rgb, colour::hs_duties(160, 254, 254, 48000));
        // round the circle: past 255 to the start again
        l.tick(&mut p, 1000 + 20_000);
        assert_eq!(l.colour().0, ((85 * 256 + 3072 * 20) % 65536 / 256) as u8);
        l.stop_color_temp(&mut p);
        assert!(!l.hue_moving());
        let stopped = l.colour().0;
        l.tick(&mut p, 1000 + 30_000);
        assert_eq!(l.colour().0, stopped, "stays where it stopped");
        // down, and any other colour command ends it
        l.move_hue(&mut p, 40_000, false, 0x0100);
        l.tick(&mut p, 41_000);
        assert_eq!(l.colour().0, stopped - 1);
        l.move_to_hue_sat(&mut p, 41_000, 10, 254, HueDirection::Shortest, 0);
        assert!(!l.hue_moving());
    }

    // the stock EBF firmware: its strip always from xy
    #[test]
    fn a_strip_beside_the_whites_is_drawn_from_xy() {
        let (mut l, mut p) = rgb_light(true);
        l.move_to_hue_sat(&mut p, 0, 0, 254, HueDirection::Shortest, 0);
        l.move_to_xy(&mut p, 0, RED.0, RED.1, 0);
        assert_eq!(p.rgb, [48377, 0, 0]);
        l.move_to_color_temp(&mut p, 0, 250, 0);
        assert_eq!((p.rgb, l.colour().2), ([48377, 0, 0], 0), "the strip keeps its colour, x reads 0");
    }

    /// Duties in thousandths of a percent, warm then cold, at a level and a colour temperature.
    fn tw(range: (u16, u16), level: u8, mireds: u16) -> (u32, u32) {
        let mut p = TwPwm(0, 0, range);
        let mut l = Light::new();
        l.move_to_color_temp(&mut p, 0, mireds, 0);
        l.turn_on(&mut p, 0, level, 0);
        ((p.0 as u64 * 100_000 / 48000) as u32, (p.1 as u64 * 100_000 / 48000) as u32)
    }

    #[test]
    fn the_warm_channel_keeps_the_stock_dead_band() {
        // what the stock firmware drives (emulator, docs/stock-profiles.md), to the PWM step
        let eglo = (153, 370);
        assert_eq!(tw(eglo, 254, 154), (0, 100_000), "0.46 % warm: none");
        assert_eq!(tw(eglo, 254, 155), (1_500, 98_500), "0.92 %: up to 1.5 %");
        assert_eq!(tw(eglo, 254, 157), (1_843, 98_156));
        assert_eq!(tw(eglo, 254, 368), (98_500, 1_500), "99.08 %: down to 98.5 %");
        assert_eq!(tw(eglo, 254, 369), (100_000, 0), "99.54 %: all warm");
        let filament = (153, 454);
        assert_eq!(tw(filament, 254, 155), (0, 100_000));
        assert_eq!(tw(filament, 254, 156), (1_500, 98_500));
        assert_eq!(tw(filament, 254, 452), (100_000, 0));
        // the lowest levels: the warm channel at 1.5 % or nothing, the cold one the rest
        let mid = 261;
        assert_eq!(tw(eglo, 1, mid), (0, 1_500), "0.75 % of 1.5 % rounds down");
        assert_eq!(tw(eglo, 4, mid).0, 1_500);
        // the cold channel has no dead band
        assert_eq!(tw(eglo, 128, 368).1, 183, "0.183 % in the stock firmware");
    }

    #[test]
    fn a_new_light_is_3000_kelvin() {
        assert_eq!(Light::new().color_temp(), crate::CT_DEFAULT);
        assert_eq!(crate::CT_DEFAULT, 333);
    }

    #[test]
    fn gamma_endpoints() {
        assert_eq!(gamma_duty(0, 1000), 0);
        assert_eq!(gamma_duty(254, 1000), 1000);
        // half the level gives ~1/4 of the power (quadratic curve)
        let half = gamma_duty(127, 1000);
        assert!((240..=260).contains(&half), "half={half}");
    }

    #[test]
    fn immediate_turn_on_sets_pwm() {
        let mut p = TestPwm(0);
        let mut l = Light::new();
        l.turn_on(&mut p, 0, 254, 0);
        assert!(l.is_on());
        assert_eq!(p.0, 1000);
        assert!(!l.is_fading());
    }

    #[test]
    fn fade_reaches_target_and_finishes() {
        let mut p = TestPwm(0);
        let mut l = Light::new();
        l.turn_on(&mut p, 0, 254, 1000); // 1 s transition from 0 to 254
        assert!(l.is_fading());
        // after half the time the brightness is roughly halfway
        l.tick(&mut p, 500);
        let mid = l.current_level();
        assert!((110..=150).contains(&mid), "mid={mid}");
        // after the full time it reaches the end and stops changing
        l.tick(&mut p, 1000);
        assert_eq!(l.current_level(), 254);
        assert!(!l.is_fading());
        assert_eq!(p.0, 1000);
    }

    #[test]
    fn turn_off_remembers_level() {
        let mut p = TestPwm(0);
        let mut l = Light::new();
        l.turn_on(&mut p, 0, 180, 0);
        l.turn_off(&mut p, 0, 0);
        assert!(!l.is_on());
        assert_eq!(p.0, 0);
        assert_eq!(l.level(), 180); // remembered
    }
}
