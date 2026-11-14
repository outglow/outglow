//! Bluetooth flashing: `outglow ble scan | probe | push` (a port of
//! the Python ble_ota.py and awox_ota.py).
//!
//! - [`protocol`]: packets, CRCs, mesh login, status layouts and the transfer
//!   over a [`protocol::Transport`], independent of Bluetooth;
//! - [`ble`]: the btleplug transport, scanning and the commands;
//! - [`vectors`]: reference values for the browser flasher's tests.

use std::path::PathBuf;

use anyhow::{bail, Result};
use outglow_image::{board, telink};

pub mod ble;
pub mod protocol;
pub mod vectors;

/// The AwoX company id in the manufacturer data of the advertisement.
pub const AWOX_VENDOR: u16 = 0x0160;
/// The advertisement carries only the 4 low MAC bytes; the 2 high ones are
/// the Telink prefix.
pub const TELINK_PREFIX: [u8; 2] = [0xa4, 0xc1];

#[derive(clap::Subcommand)]
pub enum Ble {
    /// Lists the AwoX/EGLO bulbs in range from their advertisements (address,
    /// mesh name, product, MAC). Connects to nothing, writes nothing.
    Scan {
        #[arg(long, default_value_t = 10.0)]
        seconds: f64,
    },
    /// Connects and reads: the Telink mesh characteristics, Device Information
    /// and the OTA channel status. Writes nothing.
    Probe {
        /// the Bluetooth address (a UUID on macOS) or the MAC from `ble scan`
        target: String,
        /// how long to look for the bulb's advertisement
        #[arg(long, default_value_t = 15.0)]
        seconds: f64,
    },
    /// Sends an image over the OTA channel (...1915): the first install onto the
    /// original firmware, or an update of a bulb running Outglow over Bluetooth.
    ///
    /// Locks, each lifted only by its own flag:
    ///
    /// * the image must be finished (KNLT, size, CRC) and fit the 400 KB
    ///   staging area;
    ///
    /// * by default only an image of the ORIGINAL firmware is sent (the canary:
    ///   it proves the AwoX bootloader installs what we send, and changes
    ///   nothing). An Outglow image needs --allow-outglow (the earlier name
    ///   --allow-nonstock still works). Images that are neither are refused;
    ///
    /// * the product in the bulb's advertisement must be the one of the image's
    ///   model (--product sets another expected product);
    ///
    /// * the bulb's hardware revision (Device Information 0x2A27) must equal
    ///   the image's: in another revision the LEDs may be on other pins
    ///   (--any-revision);
    ///
    /// * without --confirm-write the checks run (they connect and read) but
    ///   nothing is written.
    ///
    /// Block size: 16 B by default (the classic transfer at ATT_MTU 23, the one
    /// verified on hardware). With --mtu N the block is N-7, capped at what the
    /// bulb's OTA handler takes (told by the length of its idle status: original
    /// firmware 240 B, Outglow 57 B, anything else 16 B). btleplug does not report
    /// the negotiated ATT_MTU, so N is taken as given: ask only for what the
    /// operating system really negotiates. After a CRC or sequence error
    /// reported by the original firmware the blocks are sent again from the one
    /// after its last written block.
    #[command(verbatim_doc_comment)]
    Push {
        /// the Bluetooth address (a UUID on macOS) or the MAC from `ble scan`
        target: String,
        /// the Bluetooth image (…-ota.bin, not a Zigbee .ota file)
        image: PathBuf,
        /// write; without it nothing is written
        #[arg(long)]
        confirm_write: bool,
        /// allow an Outglow image (by default only the original firmware is sent)
        #[arg(long, visible_alias = "allow-nonstock")]
        allow_outglow: bool,
        /// send although the bulb's hardware revision differs from the image's
        #[arg(long)]
        any_revision: bool,
        /// expected product in the advertisement (default: the image's, e.g. 0x20a7 for ESMLFzm_w6_Dimm)
        #[arg(long, value_parser = parse_int)]
        product: Option<u16>,
        /// ATT_MTU; 23 (default) = classic 16 B blocks
        #[arg(long, default_value_t = protocol::DEFAULT_MTU)]
        mtu: usize,
        /// block size in bytes (16..MTU-7, within the handler's limit); default from the MTU
        #[arg(long)]
        chunk: Option<usize>,
        /// resends after a CRC/sequence error, in a row without progress
        #[arg(long, default_value_t = 10)]
        max_resends: u32,
        /// pause after each write, in milliseconds
        #[arg(long, default_value_t = 0.0)]
        delay_ms: f64,
        /// mesh login on ...1914 before the update (the OTA channel needs none)
        #[arg(long)]
        login: bool,
        #[arg(long, default_value = "unpaired")]
        mesh_name: String,
        /// 16 bytes in hex; default the secure password SHA-256(MAC || secret)
        #[arg(long)]
        mesh_password: Option<String>,
        /// how long to look for the bulb's advertisement
        #[arg(long, default_value_t = 15.0)]
        seconds: f64,
    },
    /// Reference values for site/test/awox-ota.test.ts, as JSON
    #[command(hide = true)]
    Vectors { image: Option<PathBuf> },
}

fn parse_int(s: &str) -> Result<u16, String> {
    let r = match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(h) => u16::from_str_radix(h, 16),
        None => s.parse(),
    };
    r.map_err(|e| format!("{s}: {e}"))
}

pub fn run(c: Ble) -> Result<()> {
    if let Ble::Vectors { image } = &c {
        let img = match image {
            Some(p) => std::fs::read(p)?,
            None => vectors::synthetic(),
        };
        print!("{}", vectors::json(&img));
        return Ok(());
    }
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async move {
        match c {
            Ble::Scan { seconds } => ble::scan(seconds).await,
            Ble::Probe { target, seconds } => ble::probe(&target, seconds).await,
            p @ Ble::Push { .. } => ble::push(p).await,
            Ble::Vectors { .. } => unreachable!(),
        }
    })
}

/// Product and MAC (display order) from the AwoX manufacturer data: product
/// LE16 || the 4 low MAC bytes, reversed.
pub fn parse_adv(md: &[u8]) -> Option<(u16, [u8; 6])> {
    if md.len() < 6 {
        return None;
    }
    let product = u16::from_le_bytes([md[0], md[1]]);
    Some((product, [TELINK_PREFIX[0], TELINK_PREFIX[1], md[5], md[4], md[3], md[2]]))
}

pub fn fmt_mac(mac6: &[u8; 6]) -> String {
    mac6.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(":")
}

/// The model a BLE product id belongs to.
pub fn product_model(product: u16) -> Option<&'static str> {
    board::BOARDS.iter().find(|b| b.ble_product == product).map(|b| b.model)
}

/// What an image is for and whether it may be sent.
#[derive(Debug)]
pub struct ImageInfo {
    pub model: String,
    pub revision: String,
    pub product: u16,
    pub outglow: bool,
}

/// The checks before anything is sent: the bootloader's header and CRC, the
/// staging size, the board, and the canary rule (an Outglow image only with
/// `allow_outglow`).
pub fn check_image(img: &[u8], allow_outglow: bool) -> Result<ImageInfo> {
    if let Err(e) = telink::check(img) {
        bail!("{e}: the bootloader would reject this image, not sending (a Zigbee .ota file is not what goes over Bluetooth: take the -ota.bin)");
    }
    if img.len() > telink::STAGING_SIZE {
        bail!("image {} B does not fit in the OTA staging area ({} B)", img.len(), telink::STAGING_SIZE);
    }
    let id = board::identify(img).map_err(anyhow::Error::msg)?;
    let (model, revision) = id.model_and_revision();
    let outglow = matches!(id, board::Identity::Outglow { .. });
    let Some(product) = board::BOARDS.iter().find(|b| b.model == model).map(|b| b.ble_product) else {
        bail!("board {model} is not known to this flasher");
    };
    if outglow && !allow_outglow {
        bail!(
            "this is an Outglow image, not the original firmware of {model}. The canary is the original image first; \
             an Outglow image needs --allow-outglow"
        );
    }
    Ok(ImageInfo { model, revision, product, outglow })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(text: &[u8]) -> Vec<u8> {
        let mut body = vec![0u8; 0x400];
        body[8..12].copy_from_slice(b"KNLT");
        body[0x100..0x100 + text.len()].copy_from_slice(text);
        telink::finalize(&body)
    }

    #[test]
    fn advertisement() {
        let (p, mac) = parse_adv(&[0xa7, 0x20, 0x56, 0x34, 0x12, 0x38]).unwrap();
        assert_eq!(p, 0x20a7);
        assert_eq!(fmt_mac(&mac), "a4:c1:38:12:34:56");
        assert!(parse_adv(&[0xa7, 0x20, 0x56]).is_none());
        assert_eq!(product_model(0x20a6), Some("ESMLFzm_w6_TW"));
    }

    #[test]
    fn the_original_firmware_passes() {
        let i = check_image(&image(b"ESMLFzm_w6_Dimm\0 v3.0.2 4.63\0"), false).unwrap();
        assert_eq!((i.model.as_str(), i.revision.as_str(), i.product, i.outglow), ("ESMLFzm_w6_Dimm", "4.63", 0x20a7, false));
    }

    #[test]
    fn outglow_only_when_allowed() {
        let img = image(b"outglow-board:ESMLFzm_w6_TW-4.63\0");
        let e = check_image(&img, false).unwrap_err().to_string();
        assert!(e.contains("--allow-outglow"), "{e}");
        let i = check_image(&img, true).unwrap();
        assert_eq!((i.model.as_str(), i.revision.as_str(), i.product, i.outglow), ("ESMLFzm_w6_TW", "4.63", 0x20a6, true));
    }

    #[test]
    fn damaged_or_unknown_images_are_refused() {
        let mut img = image(b"ESMLFzm_w6_Dimm\0 4.63\0");
        img[0x200] ^= 1;
        assert!(check_image(&img, true).unwrap_err().to_string().contains("CRC"));
        assert!(check_image(&image(b"something else 4.63\0"), true).is_err());
        assert!(check_image(&image(b"ESMLFzm_w6_Dimm\0 4.60 4.63\0"), true).is_err());
        assert!(check_image(&image(b"outglow-board:Nonexistent-4.63\0"), true).is_err());
    }

    #[test]
    fn product_numbers() {
        assert_eq!(parse_int("0x20a7"), Ok(0x20a7));
        assert_eq!(parse_int("8359"), Ok(8359));
        assert!(parse_int("zz").is_err());
    }
}
