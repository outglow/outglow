fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(scenarios::ble::runner::main_with(&argv));
}
