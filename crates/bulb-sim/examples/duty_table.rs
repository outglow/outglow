//! Prints the PWM duty the light logic gives for every brightness level, one
//! "level duty" pair a line, for the PWM period given as the argument.
//!
//! `emulator/rust_fw_sim.py` compares the firmware built for the chip against
//! this table from the host build of the same code.

use bulb_core::light::gamma_duty;

fn main() {
    let max: u16 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(48_000);
    for level in 0..=bulb_core::LEVEL_MAX {
        println!("{level} {}", gamma_duty(level, max));
    }
}
