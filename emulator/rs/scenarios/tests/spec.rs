//! `cargo test -p scenarios [-- filters] [--image label=path] [--cpi N] [--jobs N] [-v]`:
//! the spec tests on every image (stock, C and Rust firmware by default).

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(scenarios::runner::main_with(&argv));
}
