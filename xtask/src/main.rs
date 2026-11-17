#[cfg(test)]
use memcap as _;
use clap::{Parser, Subcommand};

mod boards;
mod build;
mod comments;
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
    #[doc = " Build the firmware of a board (crates/bulb-tc32); in `nix develop` (its pinned rustc) the image is the one CI builds"]
    Build(build::Args),
    #[doc = " The board table from the stock profiles (analysis/profiles/)"]
    Boards(boards::Args),
    #[doc = " The website's comparison data: the conformance tests on the original firmware and on Outglow"]
    Compare(compare::Args),
    #[doc = " The Zigbee2MQTT OTA index and the web flasher's release list from built images"]
    Index(index::Args),
    #[doc = " Mutation tests: break the code on purpose, the tests must notice"]
    Mutants(mutants::Args),
    #[doc = " The top of a release's notes: how to install, the files of every board"]
    ReleaseNotes(notes::Args),
    #[doc = " Download the prebuilt llvm-tc32 compiler (or say how to build it)"]
    Toolchain(toolchain::Args),
    #[doc = " Rust sources carry no comments: list them, or remove them with --fix"]
    Comments(comments::Args),
}

fn main() -> anyhow::Result<()> {
    if let Some(code) = build::rustc_wrapper() {
        std::process::exit(code);
    }
    match Cli::parse().task {
        Task::Build(a) => build::run(a),
        Task::Boards(a) => boards::run(a),
        Task::Compare(a) => compare::run(a),
        Task::Index(a) => index::run(a),
        Task::Mutants(a) => mutants::run(a),
        Task::ReleaseNotes(a) => notes::run(a),
        Task::Toolchain(a) => toolchain::run(a),
        Task::Comments(a) => comments::run(&a),
    }
}

pub fn root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}
