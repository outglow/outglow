//! `cargo test -p scenarios --test ble [-- filters] [--image label=path] [--cpi N] [--jobs N] [-v] [--json f]`:
//! the BLE conformance tests on every image (stock, C with BLE=1, Rust by default).

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(scenarios::ble::runner::main_with(&argv));
}
