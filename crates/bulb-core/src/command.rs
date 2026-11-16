pub use crate::colour::{ColorMode, HueDirection};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerOnBehavior {
    Off,
    On,
    Toggle,
    Previous,
}

impl PowerOnBehavior {
    pub fn to_u8(self) -> u8 {
        match self {
            PowerOnBehavior::Off => 0,
            PowerOnBehavior::On => 1,
            PowerOnBehavior::Toggle => 2,
            PowerOnBehavior::Previous => 0xFF,
        }
    }
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => PowerOnBehavior::Off,
            1 => PowerOnBehavior::On,
            2 => PowerOnBehavior::Toggle,
            _ => PowerOnBehavior::Previous,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    On,
    Off,
    Toggle,
    MoveToLevel { level: u8, transition_ms: u32 },
    SetStartupLevel(u8),
    SetPowerOnBehavior(PowerOnBehavior),
    Identify { times: u8 },
    ReadState,
    StopTransition,
    MoveToColorTemperature { mireds: u16, transition_ms: u32 },
    SetStartupColorTemperature(u16),
    SetLevelWhileOff(u8),
    StopColorTransition,
    MoveToHueSaturation { hue: u8, sat: u8, direction: HueDirection, transition_ms: u32 },
    MoveToXy { x: u16, y: u16, transition_ms: u32 },
    MoveHue { up: bool, rate: u16 },
    Candle(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Shown {
    pub level: u8,
    pub mireds: u16,
    pub hue: u8,
    pub sat: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateReport {
    pub on: bool,
    pub level: u8,
    pub power_on_behavior: PowerOnBehavior,
    pub startup_level: u8,
    pub color_temp: u16,
    pub startup_color_temp: u16,
    pub color_mode: ColorMode,
    pub hue: u8,
    pub sat: u8,
    pub x: u16,
    pub y: u16,
    pub hue_moving: bool,
    pub candle: bool,
    pub shown: Shown,
}

impl Command {
    pub fn decode(b: &[u8]) -> Option<Command> {
        let (&op, p) = b.split_first()?;
        Some(match op {
            1 => Command::On,
            2 => Command::Off,
            3 => Command::Toggle,
            4 => {
                let &[level, t0, t1, t2, t3, ..] = p else { return None };
                Command::MoveToLevel { level, transition_ms: u32::from_le_bytes([t0, t1, t2, t3]) }
            }
            5 => Command::SetStartupLevel(*p.first()?),
            6 => Command::SetPowerOnBehavior(PowerOnBehavior::from_u8(*p.first()?)),
            7 => Command::Identify { times: *p.first()? },
            8 => Command::ReadState,
            9 => Command::StopTransition,
            10 => {
                let p = p.get(..6)?;
                Command::MoveToColorTemperature {
                    mireds: u16::from_le_bytes([p[0], p[1]]),
                    transition_ms: u32::from_le_bytes([p[2], p[3], p[4], p[5]]),
                }
            }
            11 => {
                let p = p.get(..2)?;
                Command::SetStartupColorTemperature(u16::from_le_bytes([p[0], p[1]]))
            }
            12 => Command::SetLevelWhileOff(*p.first()?),
            13 => Command::StopColorTransition,
            14 => {
                let p = p.get(..7)?;
                Command::MoveToHueSaturation {
                    hue: p[0].min(254),
                    sat: p[1],
                    direction: HueDirection::from_u8(p[2])?,
                    transition_ms: u32::from_le_bytes([p[3], p[4], p[5], p[6]]),
                }
            }
            15 => {
                let p = p.get(..8)?;
                Command::MoveToXy {
                    x: u16::from_le_bytes([p[0], p[1]]),
                    y: u16::from_le_bytes([p[2], p[3]]),
                    transition_ms: u32::from_le_bytes([p[4], p[5], p[6], p[7]]),
                }
            }
            16 => {
                let p = p.get(..3)?;
                Command::MoveHue { up: p[0] != 0, rate: u16::from_le_bytes([p[1], p[2]]) }
            }
            17 => Command::Candle(*p.first()? != 0),
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_commands_without_parameters() {
        assert_eq!(Command::decode(&[1]), Some(Command::On));
        assert_eq!(Command::decode(&[2, 0xAA]), Some(Command::Off));
        assert_eq!(Command::decode(&[3]), Some(Command::Toggle));
        assert_eq!(Command::decode(&[8]), Some(Command::ReadState));
        assert_eq!(Command::decode(&[9]), Some(Command::StopTransition));
        assert_eq!(Command::decode(&[13]), Some(Command::StopColorTransition));
    }

    #[test]
    fn decode_move_to_level_reads_little_endian_transition() {
        assert_eq!(
            Command::decode(&[4, 127, 0xE8, 0x03, 0, 0]),
            Some(Command::MoveToLevel { level: 127, transition_ms: 1000 })
        );
    }

    #[test]
    fn decode_settings() {
        assert_eq!(Command::decode(&[5, 0xFF]), Some(Command::SetStartupLevel(0xFF)));
        assert_eq!(
            Command::decode(&[6, 1]),
            Some(Command::SetPowerOnBehavior(PowerOnBehavior::On))
        );
        assert_eq!(Command::decode(&[7, 3]), Some(Command::Identify { times: 3 }));
    }

    #[test]
    fn decode_rejects_unknown_opcode_and_short_input() {
        assert_eq!(Command::decode(&[]), None);
        assert_eq!(Command::decode(&[0]), None);
        assert_eq!(Command::decode(&[12]), None);
        assert_eq!(Command::decode(&[12, 77]), Some(Command::SetLevelWhileOff(77)));
        assert_eq!(Command::decode(&[16]), None);
        assert_eq!(
            Command::decode(&[14, 40, 200, 0, 0xE8, 0x03, 0, 0]),
            Some(Command::MoveToHueSaturation { hue: 40, sat: 200, direction: HueDirection::Shortest, transition_ms: 1000 })
        );
        assert_eq!(Command::decode(&[14, 40, 200, 4, 0, 0, 0, 0]), None, "no such direction");
        assert_eq!(
            Command::decode(&[15, 0xCD, 0x4C, 0x99, 0x99, 0, 0, 0, 0]),
            Some(Command::MoveToXy { x: 0x4CCD, y: 0x9999, transition_ms: 0 })
        );
        assert_eq!(Command::decode(&[10, 1, 2]), None);
        assert_eq!(
            Command::decode(&[10, 0x2C, 0x01, 0xE8, 0x03, 0, 0]),
            Some(Command::MoveToColorTemperature { mireds: 300, transition_ms: 1000 })
        );
        assert_eq!(Command::decode(&[4, 127, 0xE8]), None);
        assert_eq!(Command::decode(&[5]), None);
    }
}
