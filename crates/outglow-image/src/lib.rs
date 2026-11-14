//! Firmware image formats of TLSR8258 bulbs.
//!
//! - [`elf`]: the flash image of a linked TC32 ELF;
//! - [`telink`]: the header the bootloader checks (`KNLT`, size, CRC-32 at
//!   the end);
//! - [`ota`]: Zigbee OTA upgrade files (ZCL OTA Upgrade cluster, the format
//!   Zigbee2MQTT serves);
//! - [`board`]: which board an image is for (the `open-eglo-board:` tag of an
//!   Outglow image, or the model and hardware revision strings of an original
//!   one).

pub mod board;
pub mod elf;
pub mod ota;
pub mod telink;

/// CRC-32 (IEEE 802.3, reflected, initial and final XOR 0xFFFFFFFF), as
/// zlib's `crc32`.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// The first occurrence of `needle` in `hay`.
pub(crate) fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    #[test]
    fn crc32_check_value() {
        assert_eq!(super::crc32(b"123456789"), 0xCBF4_3926);
    }
}
