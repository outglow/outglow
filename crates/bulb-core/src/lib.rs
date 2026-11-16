#![forbid(unsafe_code)]
#![no_std]
//! Bulb firmware core — independent of the hardware and of `std`.
//!
//! The model mirrors what we established from the original EGLO 12224 firmware:
//! a dimmable light (no colour adjustment), with behaviour after power returns
//! (`power-on behavior` + `startup level`), on/off/level commands with a smooth
//! transition, and a persistent configuration stored in non-volatile memory.
//!
//! The hardware layer sits behind the traits in [`hal`]; on the host we substitute
//! mocks (the `bulb-sim` crate), so all the logic is tested without a real chip.

pub mod candle;
pub mod colour;
pub mod command;
pub mod config;
pub mod curve;
pub mod hal;
pub mod light;
pub mod nv;
pub mod reset;
pub mod staging;
pub mod store;

mod firmware;

pub use command::{Command, PowerOnBehavior, StateReport};
pub use config::{Config, StartupLevel};
pub use firmware::Bulb;
pub use light::Light;

/// Minimum and maximum brightness level in the Zigbee model (genLevelCtrl: 1..=254).
pub const LEVEL_MIN: u8 = 1;
pub const LEVEL_MAX: u8 = 254;
/// The "minimum" level for `startup current level` (genLevelCtrl 0x4000 == 0x00).
pub const STARTUP_MINIMUM: u8 = 0x00;
/// The "previous" level for `startup current level` (genLevelCtrl 0x4000 == 0xFF).
pub const STARTUP_PREVIOUS: u8 = 0xFF;
/// Colour temperature range of the tunable-white filament bulb, in mireds
/// (6500 K .. 2200 K): the widest of the range, and the one a board has unless
/// it says otherwise (`hal::Pwm::ct_range`).
pub const CT_MIN: u16 = 153;
pub const CT_MAX: u16 = 454;
/// The colour temperature of a factory-new bulb, in mireds (3000 K): what the
/// stock firmware of every tunable-white board starts with (docs/stock-profiles.md).
pub const CT_DEFAULT: u16 = 333;
/// "Previous" for the startup colour temperature (lightingColorCtrl 0x4010 == 0xFFFF).
pub const STARTUP_CT_PREVIOUS: u16 = 0xFFFF;
