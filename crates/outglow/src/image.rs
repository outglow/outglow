use std::fmt::Write as _;

use outglow_image::{board, ota, telink};

#[derive(clap::Args)]
pub struct ImageArgs {
    #[doc = " a Bluetooth image (…-ota.bin), a flash image (.bin) or a Zigbee OTA file (.ota)"]
    pub file: std::path::PathBuf,
}

pub fn run(a: ImageArgs) -> anyhow::Result<()> {
    print!("{}", describe(&std::fs::read(&a.file)?)?);
    Ok(())
}

pub fn describe(data: &[u8]) -> anyhow::Result<String> {
    let mut o = String::new();
    let image = if data.starts_with(&ota::IDENTIFIER.to_le_bytes()) {
        let f = ota::File::parse(data).map_err(anyhow::Error::msg)?;
        let h = &f.header;
        let _ = writeln!(
            o,
            "Zigbee OTA file: manufacturer {:#06x}, image type {:#06x}, file version {:#010x}, \"{}\", {} B",
            h.manufacturer_code,
            h.image_type,
            h.file_version,
            h.header_string,
            data.len()
        );
        f.image().map(<[u8]>::to_vec).ok_or_else(|| anyhow::anyhow!("no upgrade image in the file"))?
    } else {
        data.to_vec()
    };
    let _ = match board::identify(&image) {
        Ok(board::Identity::Outglow { board: b }) => writeln!(o, "Outglow for {b}{}", known(&b)),
        Ok(board::Identity::Original { model, hw_revision }) => writeln!(o, "original firmware of {model}, hardware {hw_revision}"),
        Err(e) => writeln!(o, "board: {e}"),
    };
    let _ = match telink::check(&image) {
        Ok(()) => writeln!(o, "header and CRC: ok ({} B; the bootloader takes it)", image.len()),
        Err(e) => writeln!(o, "header and CRC: {e} ({} B)", image.len()),
    };
    Ok(o)
}

fn known(b: &str) -> String {
    match board::known(b) {
        Some(k) => format!(" (BLE product {:#06x}, OTA image type {:#06x}, {})", k.ble_product, k.image_type, k.kind),
        None => " (a board this tool does not know)".into(),
    }
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
    fn an_outglow_image_of_a_known_board() {
        assert_eq!(
            describe(&image(b"outglow-board:ESMLFzm_w6_TW-4.63\0")).unwrap(),
            "Outglow for ESMLFzm_w6_TW-4.63 (BLE product 0x20a6, OTA image type 0x0391, outglow-tw)\n\
             header and CRC: ok (1028 B; the bootloader takes it)\n"
        );
    }

    #[test]
    fn an_outglow_image_of_an_unknown_board_with_a_bad_crc() {
        let mut img = image(b"outglow-board:Nonexistent-4.61\0");
        img[0x300] ^= 1;
        assert_eq!(
            describe(&img).unwrap(),
            "Outglow for Nonexistent-4.61 (a board this tool does not know)\nheader and CRC: CRC mismatch (1028 B)\n"
        );
    }

    #[test]
    fn an_original_image_and_one_nobody_knows() {
        assert_eq!(
            describe(&image(b"ESMLFzm_w6_Dimm\0 v3.0.2 4.63\0")).unwrap(),
            "original firmware of ESMLFzm_w6_Dimm, hardware 4.63\nheader and CRC: ok (1028 B; the bootloader takes it)\n"
        );
        assert_eq!(
            describe(b"hello").unwrap(),
            "board: cannot tell the board of the image: model None, revisions []\nheader and CRC: too short to be an image (5 B)\n"
        );
    }

    #[test]
    fn a_zigbee_ota_file() {
        let img = image(b"outglow-board:ESMLFzm_w6_Dimm-4.63\0");
        let f = ota::File::with_image(ota::Header::new(0x1141, 0x0390, 0x0100_0002, "outglow x"), img).pack();
        assert_eq!(
            describe(&f).unwrap(),
            format!(
                "Zigbee OTA file: manufacturer 0x1141, image type 0x0390, file version 0x01000002, \"outglow x\", {} B\n\
                 Outglow for ESMLFzm_w6_Dimm-4.63 (BLE product 0x20a7, OTA image type 0x0390, outglow-dimm)\n\
                 header and CRC: ok (1028 B; the bootloader takes it)\n",
                f.len()
            )
        );
    }

    #[test]
    fn a_zigbee_ota_file_without_an_image_or_broken() {
        let empty = ota::File { header: ota::Header::new(0x1141, 0x0390, 1, "x"), sub_elements: vec![] }.pack();
        assert_eq!(describe(&empty).unwrap_err().to_string(), "no upgrade image in the file");
        let cut = &empty[..10];
        assert!(describe(cut).is_err());
    }
}
