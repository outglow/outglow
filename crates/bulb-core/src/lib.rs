#![forbid(unsafe_code)]
#![no_std]

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

pub const LEVEL_MIN: u8 = 1;
pub const LEVEL_MAX: u8 = 254;
pub const STARTUP_MINIMUM: u8 = 0x00;
pub const STARTUP_PREVIOUS: u8 = 0xFF;
pub const CT_MIN: u16 = 153;
pub const CT_MAX: u16 = 454;
pub const CT_DEFAULT: u16 = 333;
pub const STARTUP_CT_PREVIOUS: u16 = 0xFFFF;
