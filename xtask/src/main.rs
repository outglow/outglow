//! `cargo xtask <task>`: the repository's tasks.

use clap::{Parser, Subcommand};

mod boards;
mod build;
mod compare;
mod index;
mod mutants;
mod notes;
mod toolchain;

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
    /// The website's comparison data: the conformance tests on the original firmware and on Outglow
    Compare(compare::Args),
    /// The Zigbee2MQTT OTA index and the web flasher's release list from built images
    Index(index::Args),
    /// Mutation tests: break the code on purpose, the tests must notice
    Mutants(mutants::Args),
    /// The top of a release's notes: how to install, the files of every board
    ReleaseNotes(notes::Args),
    /// Download the prebuilt llvm-tc32 compiler (or say how to build it)
    Toolchain(toolchain::Args),
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().task {
        Task::Build(a) => build::run(a),
        Task::Boards(a) => boards::run(a),
        Task::Compare(a) => compare::run(a),
        Task::Index(a) => index::run(a),
        Task::Mutants(a) => mutants::run(a),
        Task::ReleaseNotes(a) => notes::run(a),
        Task::Toolchain(a) => toolchain::run(a),
    }
}

/// The repository root.
pub fn root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}
