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
                } else if up >= 128 {
                    up
                } else {
                    down
                }
            }
        }
    }
}

pub fn hsv(hue: u8, sat: u8, v: u8) -> [u8; 3] {
    let (s, v) = (sat as u32, v as u32);
    if s == 0 {
        return [v as u8; 3];
    }
    let mut deg = 360 * hue as u32 / 254;
    if deg >= 360 {
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

pub fn xy_rgb(x: u16, y: u16, level: u8) -> [u16; 3] {
    let dx = x as f64 * (1.0 / 65536.0);
    let dy = y as f64 * (1.0 / 65536.0);
    let t = 1.0 - dx;
    let big_y = level as f64 / 254.0;
    let big_x = dx * big_y / dy;
    let big_z = (t - dy) * big_y / dy;
    let one = |a: f64, b: f64, c: f64| (((big_x * a + big_y * b + big_z * c) * 255.0) as u16).min(255);
    [one(3.2406, -1.5372, -0.4986), one(-0.9692, 1.8758, 0.0416), one(0.0556, -0.204, 1.057)]
}

pub fn rgb_hsv(rgb: [u16; 3]) -> [u16; 3] {
    let [r, g, b] = rgb.map(|c| c as f64 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let v = (max * 65535.0 + 0.5) as u32 as u16;
    if delta == 0.0 {
        return [0, 0, v];
    }
    let s = (delta / max * 65535.0 + 0.5) as u32 as u16;
    let sector = if r == max {
        (g - b) / delta + if g < b { 6.0 } else { 0.0 }
    } else if g == max {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    };
    let h = sector * 60.0;
    [(h * 65535.0 / 360.0 + 0.5) as u32 as u16, s, v]
}

pub fn xy_hue_sat(x: u16, y: u16) -> (u8, u8) {
    let [h, s, _] = rgb_hsv(xy_rgb(x, y, 0xDA));
    ((h >> 8) as u8, (s >> 8) as u8)
}

pub fn channel(c: u16, max: u16) -> u16 {
    let c = c as u32;
    (max as u32 * ((c * c / 254) * 100) / 25400) as u16
}

pub fn value(level: u8) -> u8 {
    level.max(16)
}

pub fn hs_duties(hue: u8, sat: u8, level: u8, max: u16) -> [u16; 3] {
    hsv(hue, sat, value(level)).map(|c| channel(c as u16, max))
}

pub fn xy_duties(x: u16, y: u16, level: u8, max: u16) -> [u16; 3] {
    let v = value(level) as u32;
    xy_rgb(x, y, level).map(|c| channel((c as u32 * v / 254) as u16, max))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColourRecord {
    pub mode: ColorMode,
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

    #[test]
    fn hue_and_saturation_as_the_stock_firmware() {
        assert_eq!(hs_duties(42, 254, 254, 48000), [48000, 41007, 0], "the middle channel stops at 235");
        assert_eq!(hs_duties(43, 254, 254, 48000), [47622, 48000, 0], "region 1 starts with red at 253");
        assert_eq!(hs_duties(3, 254, 254, 48000), [48000, 188, 0]);
        assert_eq!(hs_duties(1, 254, 254, 48000), [48000, 0, 0]);
        assert_eq!(hs_duties(160, 254, 254, 48000)[1], 3779);
        assert_eq!(hs_duties(254, 192, 254, 48000), [48000, 2834, 2834], "hue 254 is hue 0");
        assert_eq!(hs_duties(254, 1, 254, 48000), [48000, 47622, 47244]);
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
        assert_eq!(xy_hue_sat(41943, 21627), (0, 255));
        assert_eq!(xy_hue_sat(19661, 39321), (85, 255));
        assert_eq!(xy_hue_sat(20454, 17992), (213, 72));
        assert_eq!(xy_hue_sat(9830, 3932), (170, 255));
        assert_eq!(rgb_hsv([0, 0, 0]), [0, 0, 0]);
        assert_eq!(rgb_hsv([255, 255, 255]), [0, 0, 65535]);
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

    #[test]
    fn colour_modes_and_hue_directions_by_their_zcl_values() {
        assert_eq!([0, 1, 2, 3].map(ColorMode::from_u8), [ColorMode::HueSaturation, ColorMode::Xy, ColorMode::Temperature, ColorMode::Temperature]);
        assert_eq!(
            [0, 1, 2, 3, 4].map(HueDirection::from_u8),
            [Some(HueDirection::Shortest), Some(HueDirection::Longest), Some(HueDirection::Up), Some(HueDirection::Down), None]
        );
    }

    #[test]
    fn the_longest_way_round_is_up_from_128_steps() {
        assert_eq!(HueDirection::Longest.delta(0, 127), -128);
        assert_eq!(HueDirection::Longest.delta(0, 128), 128);
        assert_eq!(HueDirection::Longest.delta(10, 210), 200);
        assert_eq!(HueDirection::Longest.delta(7, 7), 0);
        assert_eq!(HueDirection::Shortest.delta(0, 127), 127);
        assert_eq!(HueDirection::Shortest.delta(0, 128), -127);
    }

    #[test]
    fn hue_and_saturation_in_the_blue_to_magenta_regions() {
        assert_eq!(hs_duties(190, 254, 254, 48000), [10015, 0, 48000]);
        assert_eq!(hs_duties(200, 254, 254, 48000), [21732, 0, 48000]);
        assert_eq!(hs_duties(230, 254, 254, 48000), [48000, 0, 17574]);
    }

    #[test]
    fn rgb_to_hsv_in_every_sector() {
        assert_eq!(rgb_hsv([255, 0, 0]), [0, 65535, 65535]);
        assert_eq!(rgb_hsv([255, 0, 1]), [65492, 65535, 65535]);
        assert_eq!(rgb_hsv([255, 128, 0]), [5483, 65535, 65535]);
        assert_eq!(rgb_hsv([10, 200, 40]), [23570, 62258, 51400]);
        assert_eq!(rgb_hsv([200, 200, 40]), [10923, 52428, 51400]);
        assert_eq!(rgb_hsv([40, 10, 200]), [45415, 62258, 51400]);
        assert_eq!(rgb_hsv([40, 200, 200]), [32768, 52428, 51400]);
        assert_eq!(rgb_hsv([3, 3, 3]), [0, 0, 771]);
    }

    #[test]
    fn xy_components_saturate_at_255() {
        assert_eq!(xy_rgb(19661, 39321, 254), [0, 255, 0]);
        assert_eq!(xy_rgb(65535, 65535, 254), [255, 220, 0]);
        assert_eq!(xy_rgb(3277, 0, 254), [0, 0, 255]);
        assert_eq!(xy_rgb(20454, 17992, 128), [179, 107, 186]);
    }

    #[test]
    fn the_record_keeps_its_mode_and_whether_it_came_from_xy() {
        for r in [
            ColourRecord { mode: ColorMode::HueSaturation, xy: false, hue: 254, sat: 0, x: 1, y: 2 },
            ColourRecord { mode: ColorMode::Temperature, xy: true, hue: 0, sat: 254, x: 0xFFFF, y: 0 },
        ] {
            assert_eq!(ColourRecord::decode(&r.encode()), Some(r));
        }
        let r = ColourRecord { mode: ColorMode::HueSaturation, xy: false, hue: 40, sat: 200, x: 3, y: 4 };
        assert_eq!(r.encode()[0], 0xC0);
        assert_eq!(ColourRecord { xy: true, mode: ColorMode::Xy, ..r }.encode()[0], 0xC3);
        assert_eq!(ColourRecord::decode(&r.encode()[..7]), None);
    }
}
