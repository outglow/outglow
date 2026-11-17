pub const CRC_MARKER_AT: usize = 0x06;
pub const CRC_MARKER: [u8; 2] = [0x5D, 0x02];
pub const STARTUP_FLAG_AT: usize = 0x08;
pub const STARTUP_FLAG: &[u8; 4] = b"KNLT";
pub const SIZE_AT: usize = 0x18;
pub const STAGING_SIZE: usize = 0x64000;

pub fn image_crc(body: &[u8]) -> u32 {
    crate::crc32(body) ^ 0xFFFF_FFFF
}

pub fn is_finalized(image: &[u8]) -> bool {
    image.get(CRC_MARKER_AT..CRC_MARKER_AT + 2) == Some(&CRC_MARKER[..])
}

pub fn finalize(image: &[u8]) -> Vec<u8> {
    let mut fw = image.to_vec();
    if is_finalized(&fw) {
        return fw;
    }
    fw.resize(fw.len().div_ceil(16) * 16, 0xFF);
    let size = (fw.len() + 4) as u32;
    fw[SIZE_AT..SIZE_AT + 4].copy_from_slice(&size.to_le_bytes());
    fw[CRC_MARKER_AT..CRC_MARKER_AT + 2].copy_from_slice(&CRC_MARKER);
    let crc = image_crc(&fw);
    fw.extend_from_slice(&crc.to_le_bytes());
    fw
}

pub fn check(image: &[u8]) -> Result<(), String> {
    if image.len() < SIZE_AT + 8 {
        return Err("too short to be an image".into());
    }
    if &image[STARTUP_FLAG_AT..STARTUP_FLAG_AT + 4] != STARTUP_FLAG {
        return Err("no KNLT marker at 0x08".into());
    }
    if !is_finalized(image) {
        return Err("no CRC marker at 0x06 (not finalized)".into());
    }
    let size = u32::from_le_bytes(image[SIZE_AT..SIZE_AT + 4].try_into().unwrap()) as usize;
    if size != image.len() {
        return Err(format!("the size at 0x18 is {size:#x}, the file {:#x}", image.len()));
    }
    let (body, tail) = image.split_at(image.len() - 4);
    let stored = u32::from_le_bytes(tail.try_into().unwrap());
    if image_crc(body) != stored {
        return Err("CRC mismatch".into());
    }
    Ok(())
}

pub fn verify(image: &[u8]) -> bool {
    check(image).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(len: usize) -> Vec<u8> {
        let mut v: Vec<u8> = (0..len).map(|i| (i * 7) as u8).collect();
        v[8..12].copy_from_slice(b"KNLT");
        v[6] = 0;
        v
    }

    #[test]
    fn finalized_images_pass_the_bootloader_check() {
        let f = finalize(&raw(1001));
        assert_eq!(f.len(), 1008 + 4);
        assert!(verify(&f));
        assert_eq!(finalize(&f), f, "idempotent");
    }

    #[test]
    fn a_changed_byte_fails_the_crc() {
        let mut f = finalize(&raw(64));
        f[40] ^= 1;
        assert_eq!(check(&f), Err("CRC mismatch".into()));
    }

    fn smallest(body_len: usize) -> Vec<u8> {
        let mut v = raw(body_len);
        v[6..8].copy_from_slice(&CRC_MARKER);
        v[SIZE_AT..SIZE_AT + 4].copy_from_slice(&((body_len + 4) as u32).to_le_bytes());
        let crc = image_crc(&v);
        v.extend_from_slice(&crc.to_le_bytes());
        v
    }

    #[test]
    fn the_image_crc_is_the_crc32_register_not_inverted() {
        assert_eq!(image_crc(b"123456789"), 0x340B_C6D9);
        assert_eq!(image_crc(b""), 0xFFFF_FFFF);
    }

    #[test]
    fn the_size_and_crc_fields_of_a_finalized_image() {
        let f = finalize(&raw(32));
        assert_eq!(f.len(), 36);
        assert_eq!(&f[6..8], &[0x5D, 0x02]);
        assert_eq!(u32::from_le_bytes(f[0x18..0x1C].try_into().unwrap()), 36);
        assert_eq!(u32::from_le_bytes(f[32..36].try_into().unwrap()), image_crc(&f[..32]));
        assert!(f[..32].iter().zip(raw(32)).enumerate().all(|(i, (a, b))| *a == b || (6..8).contains(&i) || (0x18..0x1C).contains(&i)));
    }

    #[test]
    fn the_smallest_image_holds_the_size_field_and_the_crc() {
        assert_eq!(check(&smallest(0x1C)), Ok(()));
        assert!(verify(&smallest(0x1C)));
    }

    #[test]
    fn what_the_bootloader_check_refuses() {
        assert_eq!(check(&[0; 20]), Err("too short to be an image".into()));
        assert_eq!(check(&smallest(0x1C)[..31]), Err("too short to be an image".into()));
        let mut no_knlt = smallest(0x40);
        no_knlt[9] = b'X';
        assert_eq!(check(&no_knlt), Err("no KNLT marker at 0x08".into()));
        assert_eq!(check(&raw(0x40)), Err("no CRC marker at 0x06 (not finalized)".into()));
        let mut long = smallest(0x40);
        long.push(0xFF);
        assert_eq!(check(&long), Err("the size at 0x18 is 0x44, the file 0x45".into()));
        assert!(!verify(&long));
        assert!(!is_finalized(&[0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x5D]));
    }
}
