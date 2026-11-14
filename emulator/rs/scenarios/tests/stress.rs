//! `cargo test -p scenarios --test stress -- [--full] [--cpi N] [--jobs N] [--seed N]
//! [--sets stock,c,rust,rust+stock] [--out docs/stress-results.md] [--dump file] [filters]`:
//! the stress scenarios on every image set, side by side (sims/stress/report.rs).

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(scenarios::sims::stress::report::main_with(&argv));
}
