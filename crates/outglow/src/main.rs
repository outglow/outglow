#[cfg(test)]
use memcap as _;
use clap::{Parser, Subcommand};

mod diag;
mod flash;
mod image;

#[derive(Parser)]
#[command(name = "outglow", version, about)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    #[doc = " Bluetooth: scan, probe, flash an image (first install from the original firmware, or an update)"]
    #[command(subcommand)]
    Ble(flash::Ble),
    #[doc = " Diagnostics of a bulb running Outglow, through Zigbee2MQTT (MQTT)"]
    Diag(diag::DiagArgs),
    #[doc = " The black box of a bulb running Outglow, through Zigbee2MQTT (MQTT)"]
    Box(diag::BoxArgs),
    #[doc = " What an image is: board, version, header and CRC check"]
    Image(image::ImageArgs),
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().cmd {
        Cmd::Ble(c) => flash::bt::run(c),
        Cmd::Diag(a) => diag::mqtt::run_diag(a),
        Cmd::Box(a) => diag::run_box(a),
        Cmd::Image(a) => image::run(a),
    }
}
