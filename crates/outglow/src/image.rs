use outglow_image::{board, ota, telink};

#[derive(clap::Args)]
pub struct ImageArgs {
    #[doc = " a Bluetooth image (…-ota.bin), a flash image (.bin) or a Zigbee OTA file (.ota)"]
    pub file: std::path::PathBuf,
}

pub fn run(a: ImageArgs) -> anyhow::Result<()> {
    let data = std::fs::read(&a.file)?;
    let image = if data.starts_with(&ota::IDENTIFIER.to_le_bytes()) {
        let f = ota::File::parse(&data).map_err(anyhow::Error::msg)?;
        let h = &f.header;
        println!("Zigbee OTA file: manufacturer {:#06x}, image type {:#06x}, file version {:#010x}, \"{}\", {} B",
                 h.manufacturer_code, h.image_type, h.file_version, h.header_string, data.len());
        f.image().map(<[u8]>::to_vec).ok_or_else(|| anyhow::anyhow!("no upgrade image in the file"))?
    } else {
        data
    };
    match board::identify(&image) {
        Ok(board::Identity::Outglow { board: b }) => {
            let k = board::known(&b);
            println!("Outglow for {b}{}", k.map(|k| format!(" (BLE product {:#06x}, OTA image type {:#06x}, {})", k.ble_product, k.image_type, k.kind)).unwrap_or_else(|| " (a board this tool does not know)".into()));
        }
        Ok(board::Identity::Original { model, hw_revision }) => println!("original firmware of {model}, hardware {hw_revision}"),
        Err(e) => println!("board: {e}"),
    }
    match telink::check(&image) {
        Ok(()) => println!("header and CRC: ok ({} B; the bootloader takes it)", image.len()),
        Err(e) => println!("header and CRC: {e} ({} B)", image.len()),
    }
    Ok(())
}
