//! `cargo test -p scenarios --test sims [-- filters] [--cpi N] [--image ROLE=path] [-v]`:
//! the integration scenarios of emulator/*_sim.py.

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(scenarios::sims::runner::main_with(&argv));
}
