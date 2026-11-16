fn main() {
    let max: u16 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(48_000);
    for level in 0..=bulb_core::LEVEL_MAX {
        println!("{level} {}", bulb_core::curve::duty(level, max, false));
    }
}
