//! `cargo xtask <task>`: the repository's tasks.

use clap::{Parser, Subcommand};

mod boards;
mod build;
mod index;
mod mutants;

#[derive(Parser)]
#[command(name = "cargo xtask")]
struct Cli {
    #[command(subcommand)]
    task: Task,
}

#[derive(Subcommand)]
enum Task {
    /// Build the firmware of a board (crates/bulb-tc32)
    Build(build::Args),
    /// The board table from the stock profiles (analysis/profiles/)
    Boards(boards::Args),
    /// The Zigbee2MQTT OTA index and the web flasher's release list from built images
    Index(index::Args),
    /// Mutation tests: break the code on purpose, the tests must notice
    Mutants(mutants::Args),
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().task {
        Task::Build(a) => build::run(a),
        Task::Boards(a) => boards::run(a),
        Task::Index(a) => index::run(a),
        Task::Mutants(a) => mutants::run(a),
    }
}

/// The repository root.
pub fn root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}
