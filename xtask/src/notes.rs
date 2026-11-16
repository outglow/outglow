use std::path::PathBuf;

use anyhow::Result;
use outglow_image::board;

#[derive(clap::Args)]
pub struct Args {
    #[doc = " release version, e.g. 1.0.0"]
    #[arg(long)]
    version: String,
    #[doc = " release tag (default v<version>)"]
    #[arg(long)]
    tag: Option<String>,
    #[doc = " the directory with the release assets (`cargo xtask index --assets`)"]
    #[arg(long)]
    assets: PathBuf,
}

const SITE: &str = "https://outglow.github.io";

fn light(kind: &str) -> &'static str {
    match kind {
        "outglow-dimm" => "dimmable",
        "outglow-tw" => "tunable white",
        "outglow-rgbtw" => "colour + tunable white",
        "outglow-rgbw" => "colour + white",
        "outglow-ebf" => "colour (ceiling light)",
        _ => "",
    }
}

pub fn run(a: Args) -> Result<()> {
    let tag = a.tag.clone().unwrap_or_else(|| format!("v{}", a.version));
    let base = format!("https://github.com/outglow/outglow/releases/download/{tag}");
    let mut s = String::new();
    s.push_str(&format!("Outglow {} for {} boards.\n\n", a.version, board::BOARDS.len()));
    s.push_str("## Install\n\n");
    s.push_str(&format!(
        "- **First time** (bulb on its original firmware): the [web flasher]({SITE}/flasher/) in Chrome or Edge picks the right file for your bulb. \
         Step by step: [install guide]({SITE}/install.html).\n"
    ));
    s.push_str(&format!(
        "- **Update** a bulb already on Outglow: Zigbee2MQTT with `ota: zigbee_ota_override_index_location: {SITE}/ota/index.json`, \
         then *Update* on the bulb's page.\n"
    ));
    s.push_str(&format!(
        "- **From a computer**: `outglow ble push <bulb> outglow-<board>-{}-ota.bin --allow-outglow --confirm-write`.\n\n",
        a.version
    ));
    s.push_str("## Files\n\n");
    s.push_str("The board is the model and hardware revision the bulb reports (the flasher shows it). \
                `…-ota.bin` goes over Bluetooth, `.ota` over Zigbee. Checksums: `SHA256SUMS`.\n\n");
    s.push_str("| Board | Light | Bluetooth | Zigbee OTA |\n|---|---|---|---|\n");
    for b in board::BOARDS.iter() {
        let name = format!("outglow-{}-{}", b.name, a.version);
        let ble = format!("{name}-ota.bin");
        let ota = format!("{name}.ota");
        if !a.assets.join(&ble).exists() && !a.assets.join(&ota).exists() {
            continue;
        }
        s.push_str(&format!("| `{}` | {} | [{ble}]({base}/{ble}) | [{ota}]({base}/{ota}) |\n", b.name, light(b.kind)));
    }
    s.push('\n');
    print!("{s}");
    Ok(())
}
