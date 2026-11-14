//! `cargo xtask index`: the Zigbee2MQTT OTA index and the web flasher's
//! release list from built images.
//!
//!     cargo xtask index --version 0.2.5 target/tc32*/bulb-tc32.ota \
//!         [--index ota/index.json] [--site site/public/firmware.json] [--assets DIR]
//!
//! For each Zigbee OTA file it reads the header (manufacturer code, image
//! type, file version, header string), finds the board from the image's
//! `open-eglo-board:` tag and writes an entry in the format of
//! github.com/Koenkk/zigbee-OTA with the URL where the release puts the file:
//! `<base-url>/<tag>/outglow-<board>-<version>.ota`.
//!
//! The Bluetooth image next to it (`bulb-tc32-ota.bin`) goes into the
//! flasher's release list (`--site`) under the URL the website serves it from,
//! `firmware/<tag>/outglow-<board>-<version>-ota.bin` (release assets carry no
//! CORS headers, so a browser cannot download them from GitHub directly).
//!
//! `--assets DIR` copies both files under those names into DIR, ready to be
//! uploaded to the release. Entries of other versions already in the index
//! are kept (one of the same image type and file version is replaced), so
//! the index can list earlier releases for downgrades.

use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};
use outglow_image::{board, ota};
use serde_json::{json, Value};
use sha2::{Digest, Sha256, Sha512};

const REPO: &str = "https://github.com/outglow/outglow";

#[derive(clap::Args)]
pub struct Args {
    /// bulb-tc32.ota files (one per board)
    #[arg(required = true)]
    ota: Vec<PathBuf>,
    /// release version, e.g. 0.2.5
    #[arg(long)]
    version: String,
    /// release tag (default v<version>)
    #[arg(long)]
    tag: Option<String>,
    /// the Zigbee2MQTT index to update (default ota/index.json of the repository)
    #[arg(long)]
    index: Option<PathBuf>,
    /// the web flasher's release list to update (e.g. site/public/firmware.json)
    #[arg(long)]
    site: Option<PathBuf>,
    /// directory to copy the release assets into
    #[arg(long)]
    assets: Option<PathBuf>,
    /// release notes for Zigbee2MQTT and the flasher
    #[arg(long, default_value = "")]
    notes: String,
    /// where release assets are downloaded from
    #[arg(long, default_value_t = format!("{REPO}/releases/download"))]
    base_url: String,
}

fn hex<D: Digest>(data: &[u8]) -> String {
    D::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

fn load(path: &PathBuf, default: Value) -> Result<Value> {
    match std::fs::read(path) {
        Ok(b) => Ok(serde_json::from_slice(&b).with_context(|| path.display().to_string())?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(default),
        Err(e) => Err(e.into()),
    }
}

fn today() -> String {
    // civil date from the Unix day (Howard Hinnant's algorithm), no dependency for one date
    let days = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() / 86_400).unwrap_or(0) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    format!("{y:04}-{m:02}-{d:02}")
}

pub fn run(a: Args) -> Result<()> {
    let at = |p: &PathBuf| p.clone();
    let tag = a.tag.clone().unwrap_or_else(|| format!("v{}", a.version));
    let index_path = a.index.clone().unwrap_or_else(|| crate::root().join("ota/index.json"));
    let mut index: Vec<Value> = load(&index_path, json!([]))?.as_array().cloned().ok_or_else(|| anyhow!("{}: not a list", index_path.display()))?;
    let site_path = a.site.as_ref().map(at);
    let mut site = match &site_path {
        Some(p) => Some(load(p, json!({"schema": 1, "images": []}))?),
        None => None,
    };
    if let Some(d) = &a.assets {
        std::fs::create_dir_all(at(d))?;
    }

    for path in &a.ota {
        let data = std::fs::read(path).with_context(|| path.display().to_string())?;
        let f = ota::File::parse(&data).map_err(|e| anyhow!("{}: {e}", path.display()))?;
        let h = &f.header;
        let image = f.image().ok_or_else(|| anyhow!("{}: no upgrade image sub-element", path.display()))?;
        let board = board::tag(image).ok_or_else(|| anyhow!("{}: no open-eglo-board: tag (not an Outglow image?)", path.display()))?;
        let name = format!("outglow-{board}-{}", a.version);
        let mut entry = json!({
            "fileName": format!("{name}.ota"),
            "fileVersion": h.file_version,
            "fileSize": data.len(),
            "url": format!("{}/{tag}/{name}.ota", a.base_url),
            "imageType": h.image_type,
            "manufacturerCode": h.manufacturer_code,
            "sha512": hex::<Sha512>(&data),
            "otaHeaderString": h.header_string,
        });
        if let Some(kind) = board::kind(image) {
            entry["modelId"] = json!(kind);
        }
        if !a.notes.is_empty() {
            entry["releaseNotes"] = json!(a.notes);
        }
        index.retain(|e| !(e["imageType"] == json!(h.image_type) && e["manufacturerCode"] == json!(h.manufacturer_code)
            && e["fileVersion"] == json!(h.file_version)));
        index.push(entry);
        println!("{}: {board}, image type {:#06x}, file version {:#010x}, {} B", path.display(), h.image_type, h.file_version, data.len());

        let ble = path.with_file_name("bulb-tc32-ota.bin");
        if let Some(d) = &a.assets {
            std::fs::copy(path, at(d).join(format!("{name}.ota")))?;
            if ble.exists() {
                std::fs::copy(&ble, at(d).join(format!("{name}-ota.bin")))?;
            }
        }
        if let Some(site) = site.as_mut() {
            let b = std::fs::read(&ble).with_context(|| format!("{}: no Bluetooth image next to {}", ble.display(), path.display()))?;
            if board::tag(&b).as_deref() != Some(board.as_str()) {
                bail!("{} is for {:?}, {} for {board}", ble.display(), board::tag(&b), path.display());
            }
            let images = site["images"].as_array_mut().ok_or_else(|| anyhow!("site list without images"))?;
            images.retain(|r| !(r["board"] == json!(board) && r["version"] == json!(a.version)));
            let mut r = json!({
                "board": board,
                "version": a.version,
                "fileVersion": format!("{:#010x}", h.file_version),
                "url": format!("firmware/{tag}/{name}-ota.bin"),
                "size": b.len(),
                "sha256": hex::<Sha256>(&b),
                "date": today(),
            });
            if !a.notes.is_empty() {
                r["notes"] = json!(a.notes);
            }
            images.push(r);
        }
    }

    index.sort_by_key(|e| (e["imageType"].as_u64(), e["fileVersion"].as_u64()));
    std::fs::write(&index_path, serde_json::to_string_pretty(&index)? + "\n")?;
    println!("{}: {} images", index_path.display(), index.len());
    if let (Some(site), Some(p)) = (site.as_mut(), &site_path) {
        let images = site["images"].as_array_mut().unwrap();
        images.sort_by_key(|r| {
            let v = r["fileVersion"].as_str().and_then(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok());
            (r["board"].as_str().unwrap_or("").to_string(), v)
        });
        let n = images.len();
        std::fs::write(p, serde_json::to_string_pretty(site)? + "\n")?;
        println!("{}: {n} images", p.display());
    }
    Ok(())
}
