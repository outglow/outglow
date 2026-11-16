//! Prints the PWM duty the light logic gives for every brightness level, one
//! "level duty" pair a line, for the PWM period given as the argument.
//!
//! The emulator's scenarios (`rust_fw`, `power_cut`) compare the firmware
//! built for the chip against this table from the host build of the same
//! code: the dimmable bulb's curve (`bulb_core::curve`).

fn main() {
    let max: u16 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(48_000);
    for level in 0..=bulb_core::LEVEL_MAX {
        println!("{level} {}", bulb_core::curve::duty(level, max, false));
    }
}
