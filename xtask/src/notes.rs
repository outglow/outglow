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

const LIGHTS: [(&str, &str); 5] = [
    ("outglow-dimm", "dimmable"),
    ("outglow-tw", "tunable white"),
    ("outglow-rgbtw", "colour + tunable white"),
    ("outglow-rgbw", "colour + white"),
    ("outglow-ebf", "colour (ceiling light)"),
];

fn light(kind: &str) -> &'static str {
    LIGHTS.iter().find(|(k, _)| *k == kind).map_or("", |(_, l)| *l)
}

pub fn run(a: Args) -> Result<()> {
    print!("{}", notes(&a));
    Ok(())
}

pub fn notes(a: &Args) -> String {
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
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sh::fake::Tmp;

    #[test]
    fn the_files_of_the_boards_built() {
        let t = Tmp::new("notes");
        t.put("outglow-ESMLFzm_w6_Dimm-4.63-1.2.3-ota.bin", "");
        t.put("outglow-EBF_RGB_Zm-4.60-1.2.3.ota", "");
        t.put("outglow-ESMLFzm_w6_TW-4.63-1.2.2.ota", "");
        let a = Args { version: "1.2.3".into(), tag: None, assets: t.0.clone() };
        let n = notes(&a);
        assert!(n.starts_with(&format!("Outglow 1.2.3 for {} boards.\n\n## Install\n\n", board::BOARDS.len())), "{n}");
        assert!(n.contains("`outglow ble push <bulb> outglow-<board>-1.2.3-ota.bin --allow-outglow --confirm-write`"));
        let base = "https://github.com/outglow/outglow/releases/download/v1.2.3";
        assert!(n.ends_with(&format!(
            "| Board | Light | Bluetooth | Zigbee OTA |\n|---|---|---|---|\n\
             | `ESMLFzm_w6_Dimm-4.63` | dimmable | [outglow-ESMLFzm_w6_Dimm-4.63-1.2.3-ota.bin]({base}/outglow-ESMLFzm_w6_Dimm-4.63-1.2.3-ota.bin) | [outglow-ESMLFzm_w6_Dimm-4.63-1.2.3.ota]({base}/outglow-ESMLFzm_w6_Dimm-4.63-1.2.3.ota) |\n\
             | `EBF_RGB_Zm-4.60` | colour (ceiling light) | [outglow-EBF_RGB_Zm-4.60-1.2.3-ota.bin]({base}/outglow-EBF_RGB_Zm-4.60-1.2.3-ota.bin) | [outglow-EBF_RGB_Zm-4.60-1.2.3.ota]({base}/outglow-EBF_RGB_Zm-4.60-1.2.3.ota) |\n\n"
        )), "{n}");
        let tagged = notes(&Args { version: "1.2.3".into(), tag: Some("rc".into()), assets: t.0.clone() });
        assert!(tagged.contains("releases/download/rc/outglow-ESMLFzm_w6_Dimm-4.63-1.2.3.ota"));
    }

    #[test]
    fn the_kinds_of_light() {
        let all: Vec<&str> = board::KINDS.iter().map(|k| light(k)).collect();
        assert_eq!(all, ["colour + tunable white", "colour + white", "dimmable", "tunable white", "colour (ceiling light)"]);
        assert_eq!(light("other"), "");
    }
}
