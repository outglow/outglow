//! Colour of the colour bulbs: what the stock firmware of the AwoX colour
//! lamps computes for its red, green and blue channels, read out of its code
//! and checked against it in the emulator (docs/stock-colour.md).
//!
//! - Hue and saturation: an HSV of its own, in 8-bit integers: the hue as
//!   360 * hue / 254 degrees, six regions of 60, the position in a region in
//!   steps of 4 (0..=236, so the middle channel never quite reaches the top
//!   one), the value the level (at least 16).
//! - CIE xy: xyY to linear sRGB in double precision with the sRGB matrix and
//!   no gamma, each component cut to 0..=1 and taken to 0..=255; the level
//!   counts twice, as Y and as the scale of the result (at least 16).
//! - A channel value c (0..=255) drives its PWM at (c * c / 254) / 254 of the
//!   period, in the stock firmware's integer steps.

/// Which colour the light shows (Color Control ColorMode).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    HueSaturation = 0,
    Xy = 1,
    Temperature = 2,
}

impl ColorMode {
    pub fn from_u8(v: u8) -> ColorMode {
        match v {
            0 => ColorMode::HueSaturation,
            1 => ColorMode::Xy,
            _ => ColorMode::Temperature,
        }
    }
}

/// The way a hue moves to another (Move to Hue direction).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HueDirection {
    Shortest,
    Longest,
    Up,
    Down,
}

impl HueDirection {
    pub fn from_u8(v: u8) -> Option<HueDirection> {
        match v {
            0 => Some(HueDirection::Shortest),
            1 => Some(HueDirection::Longest),
            2 => Some(HueDirection::Up),
            3 => Some(HueDirection::Down),
            _ => None,
        }
    }

    /// The signed distance from `from` to `to` on the hue circle of 255
    /// steps (0..=254), the way this direction goes.
    pub fn delta(self, from: u8, to: u8) -> i16 {
        let up = (to as i16 - from as i16).rem_euclid(255);
        let down = up - 255;
        match self {
            HueDirection::Up => up,
            HueDirection::Down => {
                if up == 0 {
                    0
                } else {
                    down
                }
            }
            HueDirection::Shortest => {
                if up <= -down {
                    up
                } else {
                    down
                }
            }
            HueDirection::Longest => {
                if up == 0 {
                    0
                } else if up > -down {
                    up
                } else {
                    down
                }
            }
        }
    }
}

/// The stock firmware's HSV: 8-bit red, green and blue for a hue (0..=254),
/// a saturation (0..=255) and a value.
pub fn hsv(hue: u8, sat: u8, v: u8) -> [u8; 3] {
    let (s, v) = (sat as u32, v as u32);
    if s == 0 {
        return [v as u8; 3];
    }
    let mut deg = 360 * hue as u32 / 254;
    if deg > 359 {
        // hue 254 is 360 degrees: the stock firmware shows it as hue 0
        deg = 0;
    }
    let region = deg / 60;
    let byte = |x: u32| ((x & 0xFFFF) >> 8) as u8;
    let f = ((deg - 60 * region) & 63) * 4;
    let p = byte((255 - s) * v);
    let t = byte((255 - (((255 - f) * s) >> 8)) * v);
    let q = byte((255 - ((s * f) >> 8)) * v);
    let v = v as u8;
    match region {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

/// The stock firmware's xyY to RGB: 0..=255 for CIE x and y (on the 0..=65535
/// scale of Color Control) at a level. With y 0 the divisions give infinities
/// and NaN, as in the stock firmware: an infinite component is 255, a NaN 0.
pub fn xy_rgb(x: u16, y: u16, level: u8) -> [u16; 3] {
    let mut dx = x as f64 * (1.0 / 65536.0);
    let mut dy = y as f64 * (1.0 / 65536.0);
    let t;
    if dx > 1.0 {
        dx = 1.0;
        t = 0.0;
    } else {
        t = 1.0 - dx;
    }
    if dy > 1.0 {
        dy = 1.0;
    }
    let big_y = level as f64 / 254.0;
    let big_x = dx * big_y / dy;
    let big_z = (t - dy) * big_y / dy;
    let one = |a: f64, b: f64, c: f64| {
        let v = big_x * a + big_y * b + big_z * c;
        if v < 0.0 {
            0
        } else if v > 1.0 {
            255
        } else {
            (v * 255.0) as u16
        }
    };
    [one(3.2406, -1.5372, -0.4986), one(-0.9692, 1.8758, 0.0416), one(0.0556, -0.204, 1.057)]
}

/// The stock firmware's RGB to HSV, in double precision: hue, saturation and
/// value on 0..=65535 for red, green and blue on 0..=255. A channel counts
/// as the largest within 1e-5; with none (the largest a NaN) the hue is 0.
pub fn rgb_hsv(rgb: [u16; 3]) -> [u16; 3] {
    const EPS: f64 = 1e-5;
    let [r, g, b] = rgb.map(|c| c as f64 / 255.0);
    let (max, min) = if r > g {
        (if r > b { r } else { b }, if g < b { g } else { b })
    } else {
        (if g > b { g } else { b }, if r < b { r } else { b })
    };
    let delta = max - min;
    let v = (max * 65535.0 + 0.5) as u32 as u16;
    if delta < EPS && delta > -EPS {
        return [0, 0, v];
    }
    let s = (delta / max * 65535.0 + 0.5) as u32 as u16;
    let near = |c: f64| max + EPS > c && max - EPS < c;
    let sector = if near(r) {
        (g - b) / delta + if g < b { 6.0 } else { 0.0 }
    } else if near(g) {
        (b - r) / delta + 2.0
    } else if near(b) {
        (r - g) / delta + 4.0
    } else {
        return [0, s, v];
    };
    let mut h = sector * 60.0;
    while h < 0.0 {
        h += 360.0;
    }
    [(h * 65535.0 / 360.0 + 0.5) as u32 as u16, s, v]
}

/// The hue and the saturation the stock firmware takes for CIE x and y when
/// an xy command comes in: the RGB of x and y at level 218, to HSV, the top
/// byte of each.
pub fn xy_hue_sat(x: u16, y: u16) -> (u8, u8) {
    let [h, s, _] = rgb_hsv(xy_rgb(x, y, 0xDA));
    ((h >> 8) as u8, (s >> 8) as u8)
}

/// The PWM compare value of a channel value 0..=255 (or a little more) on a
/// PWM with `max` steps: (c * c / 254) * 100 on the stock firmware's scale of
/// 25400. A value above 254 drives above the period, as the stock firmware
/// does (the PWM then stays high).
pub fn channel(c: u16, max: u16) -> u16 {
    let c = c as u32;
    (max as u32 * ((c * c / 254) * 100) / 25400) as u16
}

/// The value the colour channels scale with: the level, at least 16.
pub fn value(level: u8) -> u8 {
    level.max(16)
}

/// Compare values of red, green and blue for a hue and a saturation at a level.
pub fn hs_duties(hue: u8, sat: u8, level: u8, max: u16) -> [u16; 3] {
    hsv(hue, sat, value(level)).map(|c| channel(c as u16, max))
}

/// Compare values of red, green and blue for CIE x and y at a level.
pub fn xy_duties(x: u16, y: u16, level: u8, max: u16) -> [u16; 3] {
    let v = value(level) as u32;
    xy_rgb(x, y, level).map(|c| channel((c as u32 * v / 254) as u16, max))
}

/// The colour a bulb remembers over a power cut (a record of its own beside
/// `config::Config`, eight bytes): `[0]` magic 0xC and the mode,
/// `[1]` hue `[2]` saturation `[3..5]` x `[5..7]` y `[7]` crc8.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColourRecord {
    pub mode: ColorMode,
    /// The colour the colour channels show (hue and saturation, or xy):
    /// on most bulbs the one of `mode` when it is not the colour temperature.
    pub xy: bool,
    pub hue: u8,
    pub sat: u8,
    pub x: u16,
    pub y: u16,
}

pub const RECORD_LEN: usize = 8;
const MAGIC: u8 = 0xC0;

impl ColourRecord {
    pub fn encode(&self) -> [u8; RECORD_LEN] {
        let mut b = [0u8; RECORD_LEN];
        b[0] = MAGIC | (self.mode as u8) << 1 | self.xy as u8;
        b[1] = self.hue;
        b[2] = self.sat;
        b[3..5].copy_from_slice(&self.x.to_le_bytes());
        b[5..7].copy_from_slice(&self.y.to_le_bytes());
        b[7] = crate::config::crc8(&b[..7]);
        b
    }

    pub fn decode(b: &[u8]) -> Option<ColourRecord> {
        let b = b.get(..RECORD_LEN)?;
        if b[0] & 0xF0 != MAGIC || crate::config::crc8(&b[..7]) != b[7] {
            return None;
        }
        Some(ColourRecord {
            mode: ColorMode::from_u8(b[0] >> 1 & 3),
            xy: b[0] & 1 != 0,
            hue: b[1].min(254),
            sat: b[2],
            x: u16::from_le_bytes([b[3], b[4]]),
            y: u16::from_le_bytes([b[5], b[6]]),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // compare values the stock firmware drives (emulator, analysis/color/), PWM of 48000 steps
    #[test]
    fn hue_and_saturation_as_the_stock_firmware() {
        assert_eq!(hs_duties(42, 254, 254, 48000), [48000, 41007, 0], "the middle channel stops at 235");
        assert_eq!(hs_duties(43, 254, 254, 48000), [47622, 48000, 0], "region 1 starts with red at 253");
        assert_eq!(hs_duties(3, 254, 254, 48000), [48000, 188, 0]);
        assert_eq!(hs_duties(1, 254, 254, 48000), [48000, 0, 0]);
        assert_eq!(hs_duties(160, 254, 254, 48000)[1], 3779);
        assert_eq!(hs_duties(254, 192, 254, 48000), [48000, 2834, 2834], "hue 254 is hue 0");
        assert_eq!(hs_duties(254, 1, 254, 48000), [48000, 47622, 47244]);
        // the level, at least 16
        assert_eq!(hs_duties(0, 0, 1, 48000), [188; 3]);
        assert_eq!(hs_duties(30, 200, 1, 48000), [188, 0, 0]);
    }

    #[test]
    fn xy_as_the_stock_firmware() {
        assert_eq!(xy_duties(19661, 39321, 254, 48000), [0, 48377, 0], "above 254: past the period");
        assert_eq!(xy_duties(20454, 17992, 128, 48000), [5858, 2078, 6425]);
        assert_eq!(xy_duties(34972, 4857, 40, 48000), [1133, 0, 944]);
        assert_eq!(xy_duties(42809, 3138, 10, 48000), [188, 0, 0]);
        assert_eq!(xy_duties(0, 0, 254, 48000), [0; 3]);
        assert_eq!(xy_duties(3277, 0, 254, 48000), [0, 0, 48377], "y 0: blue infinite, the others NaN");
    }

    #[test]
    fn xy_to_hue_and_saturation_as_the_stock_firmware() {
        // CurrentHue and CurrentSaturation the stock bulb reports after these
        assert_eq!(xy_hue_sat(41943, 21627), (0, 255));
        assert_eq!(xy_hue_sat(19661, 39321), (85, 255));
        assert_eq!(xy_hue_sat(20454, 17992), (213, 72));
        assert_eq!(xy_hue_sat(9830, 3932), (170, 255));
        assert_eq!(rgb_hsv([0, 0, 0]), [0, 0, 0]);
        assert_eq!(rgb_hsv([255, 255, 255]), [0, 0, 65535]);
    }

    #[test]
    fn the_matrix_constants_are_the_stock_firmware_s() {
        assert_eq!((3.2406f64).to_bits(), 0x4009_ecbf_b15b_573f);
        assert_eq!((-1.5372f64).to_bits(), 0xbff8_985f_06f6_9446);
        assert_eq!((1.057f64).to_bits(), 0x3ff0_e978_d4fd_f3b6);
    }

    #[test]
    fn hue_directions() {
        assert_eq!(HueDirection::Shortest.delta(0, 170), -85);
        assert_eq!(HueDirection::Shortest.delta(0, 80), 80);
        assert_eq!(HueDirection::Longest.delta(0, 80), -175);
        assert_eq!(HueDirection::Up.delta(200, 10), 65);
        assert_eq!(HueDirection::Down.delta(10, 200), -65);
        assert_eq!(HueDirection::Down.delta(10, 10), 0);
    }

    #[test]
    fn the_record_round_trips_and_refuses_damage() {
        let r = ColourRecord { mode: ColorMode::Xy, xy: true, hue: 40, sat: 200, x: 19661, y: 39321 };
        assert_eq!(ColourRecord::decode(&r.encode()), Some(r));
        let mut b = r.encode();
        b[3] ^= 1;
        assert_eq!(ColourRecord::decode(&b), None);
        assert_eq!(ColourRecord::decode(&[0xFF; 8]), None);
    }
}
