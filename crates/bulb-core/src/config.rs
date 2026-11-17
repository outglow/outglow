use crate::command::PowerOnBehavior;

const MAGIC: u8 = 0x45;
const VERSION: u8 = 2;
pub const BLOCK_LEN: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupLevel {
    Previous,
    Minimum,
    Fixed(u8),
}

impl StartupLevel {
    pub fn to_u8(self) -> u8 {
        match self {
            StartupLevel::Previous => crate::STARTUP_PREVIOUS,
            StartupLevel::Minimum => crate::STARTUP_MINIMUM,
            StartupLevel::Fixed(v) => v.clamp(1, crate::LEVEL_MAX),
        }
    }
    pub fn from_u8(v: u8) -> Self {
        match v {
            crate::STARTUP_PREVIOUS => StartupLevel::Previous,
            crate::STARTUP_MINIMUM => StartupLevel::Minimum,
            v => StartupLevel::Fixed(v.min(crate::LEVEL_MAX)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    pub power_on_behavior: PowerOnBehavior,
    pub startup_level: StartupLevel,
    pub last_on: bool,
    pub last_level: u8,
    pub last_color_temp: u16,
    pub startup_color_temp: u16,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            power_on_behavior: PowerOnBehavior::On,
            startup_level: StartupLevel::Previous,
            last_on: true,
            last_level: crate::LEVEL_MAX,
            last_color_temp: crate::CT_DEFAULT,
            startup_color_temp: crate::STARTUP_CT_PREVIOUS,
        }
    }
}

pub(crate) fn crc8(bytes: &[u8]) -> u8 {
    let mut c: u8 = 0;
    for &b in bytes {
        c ^= b;
        for _ in 0..8 {
            c = if c & 0x80 != 0 { (c << 1) ^ 0x07 } else { c << 1 };
        }
    }
    c
}

impl Config {
    pub fn encode(&self) -> [u8; BLOCK_LEN] {
        let mut b = [0u8; BLOCK_LEN];
        b[0] = MAGIC;
        b[1] = VERSION;
        let pob = match self.power_on_behavior {
            PowerOnBehavior::Off => 0,
            PowerOnBehavior::On => 1,
            PowerOnBehavior::Toggle => 2,
            PowerOnBehavior::Previous => 3,
        };
        let last_ct = self.last_color_temp.clamp(crate::CT_MIN, crate::CT_MAX);
        let startup_ct = match self.startup_color_temp {
            crate::STARTUP_CT_PREVIOUS => None,
            ct => Some(ct.clamp(crate::CT_MIN, crate::CT_MAX)),
        };
        b[2] = pob | ((last_ct & 1) as u8) << 2 | ((startup_ct.unwrap_or(0) & 1) as u8) << 3 | (self.last_on as u8) << 7;
        b[3] = self.startup_level.to_u8();
        b[4] = self.last_level.clamp(1, crate::LEVEL_MAX);
        b[5] = (last_ct / 2) as u8;
        b[6] = startup_ct.map_or(0xFF, |ct| (ct / 2) as u8);
        b[7] = crc8(&b[0..7]);
        b
    }

    pub fn decode(b: &[u8]) -> Option<Config> {
        if b.len() < BLOCK_LEN || b[0] != MAGIC || b[1] != VERSION {
            return None;
        }
        if crc8(&b[0..7]) != b[7] {
            return None;
        }
        Some(Config {
            power_on_behavior: match b[2] & 3 {
                0 => PowerOnBehavior::Off,
                1 => PowerOnBehavior::On,
                2 => PowerOnBehavior::Toggle,
                _ => PowerOnBehavior::Previous,
            },
            startup_level: StartupLevel::from_u8(b[3]),
            last_on: b[2] & 0x80 != 0,
            last_level: b[4].clamp(1, crate::LEVEL_MAX),
            last_color_temp: (b[5] as u16 * 2 + (b[2] >> 2 & 1) as u16).clamp(crate::CT_MIN, crate::CT_MAX),
            startup_color_temp: if b[6] == 0xFF {
                crate::STARTUP_CT_PREVIOUS
            } else {
                (b[6] as u16 * 2 + (b[2] >> 3 & 1) as u16).clamp(crate::CT_MIN, crate::CT_MAX)
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_preserves_fields() {
        let c = Config {
            power_on_behavior: PowerOnBehavior::On,
            startup_level: StartupLevel::Fixed(100),
            last_on: true,
            last_level: 200,
            last_color_temp: 250,
            startup_color_temp: 400,
        };
        assert_eq!(Config::decode(&c.encode()), Some(c));
        let previous = Config { power_on_behavior: PowerOnBehavior::Previous, last_on: false, ..Config::default() };
        assert_eq!(Config::decode(&previous.encode()), Some(previous));
    }

    #[test]
    fn colour_temperatures_are_kept_to_the_mired() {
        for (last, startup) in [(153, 193), (301, 454), (454, 153), (384, crate::STARTUP_CT_PREVIOUS)] {
            let c = Config { last_color_temp: last, startup_color_temp: startup, ..Config::default() };
            assert_eq!(Config::decode(&c.encode()), Some(c), "{last}, {startup}");
        }
        let mut old = Config { last_color_temp: 300, startup_color_temp: 192, ..Config::default() }.encode();
        assert_eq!(old[2] & 0x0C, 0);
        old[5] = 150;
        old[7] = crc8(&old[0..7]);
        let c = Config::decode(&old).unwrap();
        assert_eq!((c.last_color_temp, c.startup_color_temp), (300, 192));
    }

    #[test]
    fn bad_magic_rejected() {
        let mut b = Config::default().encode();
        b[0] = 0;
        assert_eq!(Config::decode(&b), None);
    }

    #[test]
    fn corrupted_crc_rejected() {
        let mut b = Config::default().encode();
        b[3] ^= 0xFF;
        assert_eq!(Config::decode(&b), None);
    }

    #[test]
    fn startup_level_0xff_is_previous() {
        assert_eq!(StartupLevel::from_u8(0xFF), StartupLevel::Previous);
        assert_eq!(StartupLevel::from_u8(128), StartupLevel::Fixed(128));
    }

    #[test]
    fn startup_level_0_is_the_minimum_and_reads_back_as_0() {
        assert_eq!(StartupLevel::from_u8(0), StartupLevel::Minimum);
        assert_eq!(StartupLevel::Minimum.to_u8(), 0);
        let c = Config { startup_level: StartupLevel::Minimum, ..Config::default() };
        assert_eq!(Config::decode(&c.encode()), Some(c));
    }

    #[test]
    fn the_crc_is_crc8_with_polynomial_7() {
        assert_eq!(crc8(b"123456789"), 0xF4);
        assert_eq!(crc8(&[]), 0);
        assert_eq!(crc8(&[0x80]), 0x89);
    }

    #[test]
    fn a_block_needs_its_magic_and_its_version_each() {
        let good = Config::default().encode();
        for (at, v) in [(0, MAGIC ^ 1), (1, VERSION + 1)] {
            let mut b = good;
            b[at] = v;
            b[7] = crc8(&b[0..7]);
            assert_eq!(Config::decode(&b), None, "byte {at}");
        }
        assert_eq!(Config::decode(&good[..7]), None);
        let mut longer = [0u8; 9];
        longer[..8].copy_from_slice(&good);
        assert_eq!(Config::decode(&longer), Some(Config::default()));
    }

    #[test]
    fn the_flags_byte_holds_behaviour_low_bits_and_last_on() {
        let c = Config {
            power_on_behavior: PowerOnBehavior::Toggle,
            last_on: true,
            last_color_temp: 301,
            startup_color_temp: 193,
            ..Config::default()
        };
        assert_eq!(c.encode()[2], 0x80 | 0x08 | 0x04 | 2);
    }
}
