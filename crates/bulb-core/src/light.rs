use crate::colour::{self, ColorMode, ColourRecord, HueDirection};
use crate::hal::Pwm;

#[derive(Debug, Clone, Copy)]
pub struct Light {
    on: bool,
    level: u8,
    cur_centi: i32,
    start_centi: i32,
    target_centi: i32,
    start_ms: u64,
    end_ms: u64,
    ct: u16,
    ct_start: u16,
    ct_target: u16,
    ct_start_ms: u64,
    ct_end_ms: u64,
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
    xy_seen: (u16, u16),
    hs_shown: bool,
    xy_cleared: bool,
    candle: Option<crate::candle::Candle>,
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
            candle: None,
        }
    }

    pub fn set_colour_default(&mut self, hue: u8, sat: u8) {
        (self.hue, self.sat) = (hue, sat);
        self.hs_from = (hue, sat);
        self.hs_to = (hue, sat);
    }

    pub fn color_mode(&self) -> ColorMode {
        self.mode
    }

    pub fn colour(&self) -> (u8, u8, u16, u16) {
        let (x, y) = if self.xy_cleared { (0, 0) } else { self.xy_to };
        (self.hs_to.0, self.hs_to.1, x, y)
    }

    pub fn colour_record(&self) -> ColourRecord {
        let (hue, sat) = self.hs_to;
        let (x, y) = self.xy_to;
        ColourRecord { mode: self.mode, xy: self.xy, hue, sat, x, y }
    }

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

    pub fn colour_moving(&self) -> bool {
        (self.hue, self.sat) != self.hs_to || (self.x, self.y) != self.xy_to
    }

    pub fn move_to_hue_sat<P: Pwm>(&mut self, pwm: &mut P, now_ms: u64, hue: u8, sat: u8, dir: HueDirection, transition_ms: u32) {
        let hue = hue.min(254);
        self.hue_rate = 0;
        if self.mode == ColorMode::Xy {
            (self.hue, self.sat) = colour::xy_hue_sat(self.x, self.y);
        }
        self.mode = ColorMode::HueSaturation;
        if self.xy {
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
        self.hs_from = (self.hue, self.sat);
        self.hs_to = (self.hue.min(254), self.sat);
        self.hue_delta = 0;
        self.col_end_ms = self.col_start_ms;
        self.hue_rate = if up { rate as i32 } else { -(rate as i32) };
        self.enh_start = (self.hue as u32) << 8;
        self.hue_start_ms = now_ms;
        self.render(pwm);
    }

    pub fn set_candle<P: Pwm>(&mut self, pwm: &mut P, now_ms: u64, on: bool) {
        if on == self.candle.is_some() {
            return;
        }
        self.candle = on.then(|| crate::candle::Candle::new(self.level, now_ms, now_ms as u32 ^ 0x9e37_79b9));
        self.apply(pwm);
    }

    pub fn in_transition(&self) -> bool {
        self.is_fading() || self.ct != self.ct_target || self.colour_moving()
    }

    pub fn level_fade(&self, now_ms: u64) -> Option<crate::command::LevelFade> {
        self.is_fading().then(|| crate::command::LevelFade {
            at: self.current_level(),
            to: (self.target_centi / CENTI) as u8,
            left_ms: self.end_ms.saturating_sub(now_ms) as u32,
        })
    }

    pub fn ct_fade_ms(&self, now_ms: u64) -> Option<u32> {
        (self.ct != self.ct_target).then_some(self.ct_end_ms.saturating_sub(now_ms) as u32)
    }

    pub fn shown(&self) -> crate::command::Shown {
        let level = match self.current_level() {
            0 => self.level,
            l => l,
        };
        crate::command::Shown { level, mireds: self.ct, hue: self.hue.min(254), sat: self.sat }
    }

    pub fn candle(&self) -> bool {
        self.candle.is_some()
    }

    pub fn hue_moving(&self) -> bool {
        self.hue_rate != 0
    }

    pub fn color_temp(&self) -> u16 {
        self.ct_target
    }

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

    pub fn refresh<P: Pwm>(&mut self, pwm: &mut P) {
        self.render(pwm);
    }

    pub fn is_on(&self) -> bool {
        self.on
    }

    pub fn stays_on(&self) -> bool {
        self.on && self.target_centi != 0
    }

    pub fn level(&self) -> u8 {
        self.level
    }

    pub fn current_level(&self) -> u8 {
        (self.cur_centi / CENTI).clamp(0, crate::LEVEL_MAX as i32) as u8
    }

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

    pub fn turn_on<P: Pwm>(&mut self, pwm: &mut P, now_ms: u64, level: u8, transition_ms: u32) {
        let lvl = level.clamp(1, crate::LEVEL_MAX);
        self.on = true;
        self.level = lvl;
        self.start_fade(now_ms, lvl as i32, transition_ms);
        self.render(pwm);
    }

    pub fn turn_on_previous<P: Pwm>(&mut self, pwm: &mut P, now_ms: u64, transition_ms: u32) {
        let lvl = self.level;
        self.turn_on(pwm, now_ms, lvl, transition_ms);
    }

    pub fn turn_off<P: Pwm>(&mut self, pwm: &mut P, now_ms: u64, transition_ms: u32) {
        self.on = false;
        self.start_fade(now_ms, 0, transition_ms);
        self.render(pwm);
    }

    pub fn move_to_level<P: Pwm>(&mut self, pwm: &mut P, now_ms: u64, level: u8, transition_ms: u32) {
        if level == 0 {
            self.start_fade(now_ms, 0, transition_ms);
            self.on &= self.is_fading();
            self.render(pwm);
        } else {
            self.turn_on(pwm, now_ms, level, transition_ms);
        }
    }

    pub fn set_level_while_off<P: Pwm>(&mut self, pwm: &mut P, now_ms: u64, level: u8) {
        if self.on {
            self.move_to_level(pwm, now_ms, level, 0);
        } else {
            self.level = level.clamp(1, crate::LEVEL_MAX);
        }
    }

    pub fn stop<P: Pwm>(&mut self, pwm: &mut P) {
        if !self.on || !self.is_fading() {
            return;
        }
        match (self.current_level(), self.target_centi) {
            (0, 0) => {
                self.on = false;
                self.cur_centi = 0;
            }
            (reached, _) => {
                self.level = reached.max(1);
                self.cur_centi = self.level as i32 * CENTI;
            }
        }
        self.target_centi = self.cur_centi;
        self.render(pwm);
    }

    pub fn stop_color_temp<P: Pwm>(&mut self, pwm: &mut P) {
        self.hue_rate = 0;
        self.ct_target = self.ct;
        self.hs_to = (self.hue, self.sat);
        self.xy_to = (self.x, self.y);
        self.render(pwm);
    }

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

    pub fn tick<P: Pwm>(&mut self, pwm: &mut P, now_ms: u64) {
        let level = self.level;
        let candle = self.candle.as_mut().is_some_and(|c| c.tick(now_ms, level));
        if self.cur_centi == self.target_centi && self.ct == self.ct_target && !self.colour_moving() && self.hue_rate == 0 {
            if candle {
                self.apply(pwm);
            }
            return;
        }
        if self.hue_rate != 0 {
            let elapsed = now_ms.saturating_sub(self.hue_start_ms) as i64;
            let enh = (self.enh_start as i64 + self.hue_rate as i64 * elapsed / 1000).rem_euclid(1 << 16);
            self.hue = (enh >> 8) as u8;
            self.hs_to = (self.hue.min(254), self.sat);
            self.hs_from = self.hs_to;
        }
        if self.cur_centi != self.target_centi {
            self.cur_centi = if now_ms >= self.end_ms {
                self.target_centi
            } else {
                let span = (self.end_ms - self.start_ms) as i64;
                let elapsed = (now_ms - self.start_ms) as i64;
                let delta = (self.target_centi - self.start_centi) as i64;
                (self.start_centi as i64 + delta * elapsed / span) as i32
            };
            self.on &= self.target_centi != 0 || self.is_fading();
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

    fn render<P: Pwm>(&mut self, pwm: &mut P) {
        self.hs_shown = false;
        self.apply(pwm);
    }

    fn apply<P: Pwm>(&mut self, pwm: &mut P) {
        let level = match &self.candle {
            Some(c) if self.on && !self.is_fading() => c.level(),
            _ => self.current_level(),
        };
        if pwm.has_rgb() {
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
            let (warm, _) = split_duty(duty, self.ct, pwm.ct_range());
            let warm = warm_dead_band(warm, pwm.max_duty()).min(duty);
            pwm.set_duty(warm);
            pwm.set_cold_duty(duty - warm);
        } else {
            pwm.set_duty(duty);
        }
    }
}

pub fn split_duty(total: u16, mireds: u16, range: (u16, u16)) -> (u16, u16) {
    let (lo, hi) = range;
    let m = mireds.clamp(lo, hi);
    let span = (hi - lo).max(1) as u32;
    let warm = ((total as u32 * (m - lo) as u32 + span / 2) / span) as u16;
    (warm, total - warm)
}

pub fn warm_dead_band(warm: u16, max: u16) -> u16 {
    let edge = (max as u32 * 15 / 1000) as u16;
    let half = edge / 2;
    if warm < half {
        0
    } else if warm > max - half {
        max
    } else {
        warm.clamp(edge, max - edge)
    }
}

impl Default for Light {
    fn default() -> Self {
        Self::new()
    }
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

    struct RgbPwm {
        rgb: [u16; 3],
        white: u16,
        beside: bool,
    }
    impl Pwm for RgbPwm {
        fn max_duty(&self) -> u16 {
            48000
        }
        fn set_duty(&mut self, d: u16) {
            self.white = d;
        }
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
        let mut pwm = RgbPwm { rgb: [0; 3], white: 0, beside };
        let mut l = Light::new();
        l.set_immediate(&mut pwm, 0, true, 254);
        (l, pwm)
    }

    const RED: (u16, u16) = (41943, 21627);
    const GREEN: (u16, u16) = (19661, 39321);

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

    #[test]
    fn the_hue_turns_until_stopped() {
        let (mut l, mut p) = rgb_light(false);
        l.move_to_hue_sat(&mut p, 0, 85, 254, HueDirection::Shortest, 0);
        l.move_hue(&mut p, 1000, true, 0x0C00);
        assert!(l.hue_moving());
        l.tick(&mut p, 1000 + 6270);
        assert_eq!(l.colour().0, 160, "where the stock bulb is after 6.27 s");
        assert_eq!(p.rgb, colour::hs_duties(160, 254, 254, 48000));
        l.tick(&mut p, 1000 + 20_000);
        assert_eq!(l.colour().0, ((85 * 256 + 3072 * 20) % 65536 / 256) as u8);
        l.stop_color_temp(&mut p);
        assert!(!l.hue_moving());
        let stopped = l.colour().0;
        l.tick(&mut p, 1000 + 30_000);
        assert_eq!(l.colour().0, stopped, "stays where it stopped");
        l.move_hue(&mut p, 40_000, false, 0x0100);
        l.tick(&mut p, 41_000);
        assert_eq!(l.colour().0, stopped - 1);
        l.move_to_hue_sat(&mut p, 41_000, 10, 254, HueDirection::Shortest, 0);
        assert!(!l.hue_moving());
    }

    #[test]
    fn a_strip_beside_the_whites_is_drawn_from_xy() {
        let (mut l, mut p) = rgb_light(true);
        l.move_to_hue_sat(&mut p, 0, 0, 254, HueDirection::Shortest, 0);
        l.move_to_xy(&mut p, 0, RED.0, RED.1, 0);
        assert_eq!(p.rgb, [48377, 0, 0]);
        l.move_to_color_temp(&mut p, 0, 250, 0);
        assert_eq!((p.rgb, l.colour().2), ([48377, 0, 0], 0), "the strip keeps its colour, x reads 0");
    }

    fn tw(range: (u16, u16), level: u8, mireds: u16) -> (u32, u32) {
        let mut p = TwPwm(0, 0, range);
        let mut l = Light::new();
        l.move_to_color_temp(&mut p, 0, mireds, 0);
        l.turn_on(&mut p, 0, level, 0);
        ((p.0 as u64 * 100_000 / 48000) as u32, (p.1 as u64 * 100_000 / 48000) as u32)
    }

    #[test]
    fn the_warm_channel_keeps_the_stock_dead_band() {
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
        let mid = 261;
        assert_eq!(tw(eglo, 1, mid), (0, 1_500), "0.75 % of 1.5 % rounds down");
        assert_eq!(tw(eglo, 4, mid).0, 1_500);
        assert_eq!(tw(eglo, 128, 368).1, 183, "0.183 % in the stock firmware");
    }

    #[test]
    fn a_new_light_is_3000_kelvin() {
        assert_eq!(Light::new().color_temp(), crate::CT_DEFAULT);
        assert_eq!(crate::CT_DEFAULT, 333);
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
        l.turn_on(&mut p, 0, 254, 1000);
        assert!(l.is_fading());
        l.tick(&mut p, 500);
        let mid = l.current_level();
        assert!((110..=150).contains(&mid), "mid={mid}");
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
        assert_eq!(l.level(), 180);
    }

    #[test]
    fn the_dead_band_edges() {
        assert_eq!([359, 360, 719, 720, 721].map(|w| warm_dead_band(w, 48000)), [0, 720, 720, 720, 721]);
        assert_eq!([47279, 47280, 47281, 47640, 47641].map(|w| warm_dead_band(w, 48000)), [47279, 47280, 47280, 47280, 48000]);
        assert_eq!([0, 1, 66].map(|w| warm_dead_band(w, 66)), [0, 1, 66]);
    }

    #[test]
    fn the_colour_default_of_the_board() {
        let mut l = Light::new();
        l.set_colour_default(10, 20);
        assert_eq!(l.colour(), (10, 20, 0, 0));
        assert_eq!((l.shown().hue, l.shown().sat), (10, 20));
        assert_eq!((l.colour_record().hue, l.colour_record().sat), (10, 20));
        assert!(!l.in_transition());
    }

    #[test]
    fn a_restored_colour_is_drawn() {
        let (mut l, mut p) = rgb_light(false);
        let r = ColourRecord { mode: ColorMode::HueSaturation, xy: false, hue: 85, sat: 254, x: 5, y: 6 };
        l.restore_colour(&mut p, &r);
        assert_eq!((l.color_mode(), l.colour(), l.colour_record()), (ColorMode::HueSaturation, (85, 254, 5, 6), r));
        assert_eq!(p.rgb, colour::hs_duties(85, 254, 254, 48000));
        assert!(!l.in_transition());
    }

    #[test]
    fn hue_and_saturation_fade_in_a_straight_line() {
        let (mut l, mut p) = rgb_light(false);
        l.move_to_hue_sat(&mut p, 0, 250, 254, HueDirection::Shortest, 0);
        assert!(!l.in_transition());
        l.move_to_hue_sat(&mut p, 1000, 10, 100, HueDirection::Shortest, 1000);
        assert!(l.in_transition());
        assert_eq!(l.colour(), (10, 100, 0, 0), "the target");
        l.tick(&mut p, 1500);
        assert_eq!((l.shown().hue, l.shown().sat), (2, 177));
        assert_eq!(p.rgb, colour::hs_duties(2, 177, 254, 48000));
        l.tick(&mut p, 1999);
        assert_eq!((l.shown().hue, l.shown().sat), (9, 101));
        assert!(l.in_transition());
        l.tick(&mut p, 2000);
        assert_eq!((l.shown().hue, l.shown().sat), (10, 100));
        assert!(!l.in_transition());
    }

    #[test]
    fn a_hue_move_starts_from_the_shown_hue_not_an_old_xy() {
        let (mut l, mut p) = rgb_light(false);
        l.move_to_xy(&mut p, 0, RED.0, RED.1, 0);
        l.move_to_hue_sat(&mut p, 0, 85, 254, HueDirection::Shortest, 0);
        l.move_to_hue_sat(&mut p, 0, 170, 254, HueDirection::Shortest, 1000);
        l.tick(&mut p, 500);
        assert_eq!(l.shown().hue, 127);
    }

    #[test]
    fn xy_fades_along_xy() {
        let (mut l, mut p) = rgb_light(false);
        l.move_to_xy(&mut p, 0, RED.0, RED.1, 0);
        l.move_to_xy(&mut p, 1000, GREEN.0, GREEN.1, 1000);
        assert!(l.in_transition());
        l.tick(&mut p, 1500);
        assert_eq!(p.rgb, colour::xy_duties(30802, 30474, 254, 48000));
        l.tick(&mut p, 2000);
        assert_eq!(p.rgb, colour::xy_duties(GREEN.0, GREEN.1, 254, 48000));
        assert!(!l.in_transition());
    }

    #[test]
    fn xy_0_0_is_ignored_one_zero_is_not() {
        let (mut l, mut p) = rgb_light(false);
        l.move_to_xy(&mut p, 0, 0, 0, 0);
        assert_eq!(l.color_mode(), ColorMode::Temperature);
        l.move_to_xy(&mut p, 0, 0, 5, 0);
        assert_eq!((l.color_mode(), l.colour().2, l.colour().3), (ColorMode::Xy, 0, 5));
        l.move_to_xy(&mut p, 0, 7, 0, 0);
        assert_eq!((l.colour().2, l.colour().3), (7, 0));
    }

    #[test]
    fn a_colour_temperature_from_hue_and_saturation_keeps_the_last_xy() {
        let (mut l, mut p) = rgb_light(false);
        l.move_to_xy(&mut p, 0, RED.0, RED.1, 0);
        l.move_to_hue_sat(&mut p, 0, 85, 254, HueDirection::Shortest, 0);
        l.move_to_color_temp(&mut p, 0, 300, 0);
        assert_eq!(l.colour(), (85, 254, RED.0, RED.1));
        assert_eq!((p.rgb, p.white), ([0; 3], 48000));
    }

    #[test]
    fn the_whites_are_off_while_an_rgb_bulb_shows_a_colour() {
        let (mut l, mut p) = rgb_light(false);
        assert_eq!(p.white, 48000);
        l.move_to_hue_sat(&mut p, 0, 85, 254, HueDirection::Shortest, 0);
        assert_eq!(p.white, 0);
        let (mut l, mut p) = rgb_light(true);
        l.move_to_hue_sat(&mut p, 0, 85, 254, HueDirection::Shortest, 0);
        assert_eq!(p.white, 48000);
        l.move_to_color_temp(&mut p, 0, 300, 0);
        assert_eq!(p.white, 48000);
    }

    #[test]
    fn colour_temperature_fades_in_a_straight_line() {
        let mut p = TwPwm(0, 0, (153, 454));
        let mut l = Light::new();
        l.turn_on(&mut p, 0, 254, 0);
        l.move_to_color_temp(&mut p, 1000, 433, 1000);
        assert!(l.in_transition());
        assert_eq!(l.color_temp(), 433);
        l.tick(&mut p, 1500);
        assert_eq!(l.shown().mireds, 383);
        l.tick(&mut p, 1999);
        assert_eq!(l.shown().mireds, 432);
        l.tick(&mut p, 2000);
        assert_eq!(l.shown().mireds, 433);
        assert!(!l.in_transition());
    }

    #[test]
    fn a_level_fade_tells_where_it_is_where_it_goes_and_what_is_left() {
        let mut p = TestPwm(0);
        let mut l = Light::new();
        let fade = |at, to, left_ms| Some(crate::command::LevelFade { at, to, left_ms });
        l.turn_on(&mut p, 0, 200, 1000);
        assert_eq!(l.level_fade(0), fade(0, 200, 1000));
        l.tick(&mut p, 400);
        assert_eq!(l.level_fade(400), fade(80, 200, 600));
        assert_eq!(l.level_fade(1300), fade(80, 200, 0));
        l.tick(&mut p, 1000);
        assert_eq!(l.level_fade(1000), None);
        l.turn_off(&mut p, 1000, 500);
        assert_eq!(l.level_fade(1100), fade(200, 0, 400));
    }

    #[test]
    fn a_colour_temperature_fade_tells_what_is_left() {
        let mut p = TwPwm(0, 0, (153, 454));
        let mut l = Light::new();
        l.turn_on(&mut p, 0, 254, 0);
        assert_eq!(l.ct_fade_ms(0), None);
        l.move_to_color_temp(&mut p, 1000, 433, 1000);
        assert_eq!(l.ct_fade_ms(1000), Some(1000));
        l.tick(&mut p, 1500);
        assert_eq!(l.ct_fade_ms(1500), Some(500));
        assert_eq!(l.ct_fade_ms(2600), Some(0));
        l.tick(&mut p, 2000);
        assert_eq!(l.ct_fade_ms(2000), None);
    }

    #[test]
    fn a_fade_alone_is_a_transition() {
        let mut p = TestPwm(0);
        let mut l = Light::new();
        assert!(!l.in_transition());
        l.turn_on(&mut p, 0, 200, 1000);
        assert!(l.in_transition());
        l.tick(&mut p, 1000);
        assert!(!l.in_transition());
    }

    #[test]
    fn what_is_shown_while_off_is_the_level_to_come_back_to() {
        let mut p = TestPwm(0);
        let mut l = Light::new();
        l.turn_on(&mut p, 0, 99, 0);
        l.turn_off(&mut p, 0, 0);
        assert_eq!(l.shown(), crate::command::Shown { level: 99, mireds: 333, hue: 170, sat: 254 });
        l.turn_on(&mut p, 0, 120, 0);
        assert_eq!(l.shown().level, 120);
    }

    #[test]
    fn the_candle_is_the_stock_candle_seeded_from_the_time() {
        let mut p = TestPwm(0);
        let mut l = Light::new();
        l.turn_on(&mut p, 0, 128, 0);
        assert!(!l.candle());
        l.set_candle(&mut p, 1234, true);
        assert!(l.candle());
        let mut c = crate::candle::Candle::new(128, 1234, 1234 ^ 0x9e37_79b9);
        let mut changes = 0;
        for t in 1234..6000u64 {
            l.tick(&mut p, t);
            if c.tick(t, 128) {
                changes += 1;
            }
            assert_eq!(p.0, crate::curve::duty(c.level(), 1000, false), "at {t} ms");
        }
        assert!(changes > 20);
        l.set_candle(&mut p, 6000, true);
        l.tick(&mut p, 6000);
        assert_eq!(p.0, crate::curve::duty(c.level(), 1000, false), "turning it on again keeps it");
        l.set_candle(&mut p, 6000, false);
        assert!(!l.candle());
        assert_eq!(p.0, crate::curve::duty(128, 1000, false));
    }

    #[test]
    fn the_candle_waits_for_a_fade_and_is_dark_while_off() {
        let mut p = TestPwm(0);
        let mut l = Light::new();
        l.turn_on(&mut p, 0, 128, 0);
        l.set_candle(&mut p, 0, true);
        l.turn_on(&mut p, 0, 254, 1000);
        l.tick(&mut p, 500);
        assert_eq!(p.0, crate::curve::duty(l.current_level(), 1000, false));
        assert_ne!(l.current_level(), 238);
        l.turn_off(&mut p, 2000, 0);
        for t in 2000..3000 {
            l.tick(&mut p, t);
            assert_eq!(p.0, 0, "at {t} ms");
        }
    }

    #[test]
    fn a_hue_move_at_rate_0_stops_and_one_from_xy_starts_at_its_hue() {
        let (mut l, mut p) = rgb_light(false);
        l.move_to_xy(&mut p, 0, GREEN.0, GREEN.1, 0);
        l.move_to_xy(&mut p, 0, RED.0, RED.1, 0);
        l.move_hue(&mut p, 0, true, 0x0100);
        assert!(l.hue_moving());
        assert_eq!((l.color_mode(), l.colour().0), (ColorMode::HueSaturation, 0));
        l.move_hue(&mut p, 0, true, 0);
        assert!(!l.hue_moving());
    }

    #[test]
    fn stopping_a_fade_to_off_where_it_shows_0_is_off() {
        let mut p = TestPwm(0);
        let mut l = Light::default();
        assert_eq!(l.level(), 254);
        l.turn_on(&mut p, 0, 254, 0);
        l.stop(&mut p);
        assert!(l.is_on());
        l.move_to_level(&mut p, 0, 0, 1000);
        l.tick(&mut p, 999);
        assert_eq!(l.current_level(), 0);
        assert!(l.is_on() && l.is_fading());
        l.stop(&mut p);
        assert!(!l.is_on() && !l.is_fading());
        assert_eq!(p.0, 0);
    }

    #[test]
    fn stopping_a_fade_from_off_before_it_shows_1_keeps_the_light_on_at_1() {
        let mut p = TestPwm(0);
        let mut l = Light::default();
        l.turn_on(&mut p, 0, 200, 0);
        l.turn_off(&mut p, 0, 0);
        l.turn_on(&mut p, 10, 254, 3000);
        l.stop(&mut p);
        assert!(l.is_on() && !l.is_fading());
        assert_eq!((l.level(), l.current_level()), (1, 1));
        assert_ne!(p.0, 0);
    }

    #[test]
    fn a_stop_leaves_the_fade_of_an_off_running() {
        let mut p = TestPwm(0);
        let mut l = Light::default();
        l.turn_on(&mut p, 0, 254, 0);
        l.turn_off(&mut p, 0, 1000);
        l.tick(&mut p, 500);
        let reached = p.0;
        l.stop(&mut p);
        assert!(!l.is_on() && l.is_fading());
        assert_eq!(p.0, reached);
        l.tick(&mut p, 1000);
        assert!(!l.is_on() && !l.is_fading());
        assert_eq!(p.0, 0);
    }
}
