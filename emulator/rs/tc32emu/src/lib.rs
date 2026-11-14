// SPDX-License-Identifier: Apache-2.0
//! Rust port of tc32-devtools `go/tc32emu`: the TC32 (Telink TLSR825x/827x)
//! instruction-set emulator with the TLSR8278 peripherals. It follows the Go
//! package function by function, so that a run gives the same machine state,
//! instruction by instruction and cycle by cycle (see emulator/rs/README.md).

mod adc;
mod aes;
pub mod aes128;
pub mod bigint;
mod config;
mod cpu;
mod error;
mod flash;
mod i2c;
pub mod isa;
mod machine;
mod pke;
mod pwmir;
mod qdec;
mod rbg;
mod run;
mod spi;
mod uart;

#[cfg(test)]
mod tests;

pub use adc::{ADC_MODES, ADC_VBAT_CODE};
pub use config::{parse_go_int, parse_go_uint, Config};
pub use cpu::Stepped;
pub use error::{emu_err, other_err, stop_err, Error};
pub use flash::{Flash, FlashOp, ProtectedAddrs, OnWriteFn};
pub use i2c::I2cDevice;
pub use machine::*;
pub use rbg::RBG_MODES;
pub use run::Range;
pub use spi::{SpiDevice, SPI_FIFO_MAX};
