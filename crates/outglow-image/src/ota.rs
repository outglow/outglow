//! Zigbee OTA upgrade files (ZCL, OTA Upgrade cluster 0x0019).
//!
//! The header, little-endian, 56 bytes without optional fields:
//! identifier 0x0BEEF11E, header version 0x0100, header length, field
//! control (bit 0 security credential version, bit 1 upgrade file
//! destination, bit 2 hardware versions), manufacturer code, image type, file
//! version, Zigbee stack version (2 = PRO), a 32-byte header string, total
//! image size; then the optional fields; then sub-elements (tag u16, length
//! u32, data), tag 0 being the image itself.

pub const IDENTIFIER: u32 = 0x0BEE_F11E;
pub const HEADER_VERSION: u16 = 0x0100;
pub const STACK_PRO: u16 = 0x0002;
pub const TAG_UPGRADE_IMAGE: u16 = 0x0000;
pub const BASE_HEADER_LEN: usize = 56;

const FC_SECURITY_CREDENTIAL: u16 = 0x01;
const FC_DEVICE_SPECIFIC: u16 = 0x02;
const FC_HARDWARE_VERSIONS: u16 = 0x04;

#[derive(Clone, Debug, PartialEq)]
pub struct Header {
    pub manufacturer_code: u16,
    pub image_type: u16,
    pub file_version: u32,
    pub header_string: String,
    pub stack_version: u16,
    pub security_credential_version: Option<u8>,
    pub upgrade_file_destination: Option<[u8; 8]>,
    /// (minimum, maximum)
    pub hardware_versions: Option<(u16, u16)>,
}

impl Header {
    pub fn new(manufacturer_code: u16, image_type: u16, file_version: u32, header_string: &str) -> Header {
        Header {
            manufacturer_code,
            image_type,
            file_version,
            header_string: header_string.into(),
            stack_version: STACK_PRO,
            security_credential_version: None,
            upgrade_file_destination: None,
            hardware_versions: None,
        }
    }

    fn field_control(&self) -> u16 {
        (if self.security_credential_version.is_some() { FC_SECURITY_CREDENTIAL } else { 0 })
            | (if self.upgrade_file_destination.is_some() { FC_DEVICE_SPECIFIC } else { 0 })
            | (if self.hardware_versions.is_some() { FC_HARDWARE_VERSIONS } else { 0 })
    }

    pub fn len(&self) -> usize {
        BASE_HEADER_LEN
            + self.security_credential_version.map_or(0, |_| 1)
            + self.upgrade_file_destination.map_or(0, |_| 8)
            + self.hardware_versions.map_or(0, |_| 4)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct File {
    pub header: Header,
    /// (tag, data)
    pub sub_elements: Vec<(u16, Vec<u8>)>,
}

impl File {
    /// A file holding one image.
    pub fn with_image(header: Header, image: Vec<u8>) -> File {
        File { header, sub_elements: vec![(TAG_UPGRADE_IMAGE, image)] }
    }

    pub fn image(&self) -> Option<&[u8]> {
        self.sub_elements.iter().find(|(t, _)| *t == TAG_UPGRADE_IMAGE).map(|(_, d)| d.as_slice())
    }

    pub fn pack(&self) -> Vec<u8> {
        let h = &self.header;
        let header_len = h.len();
        let total = header_len + self.sub_elements.iter().map(|(_, d)| 6 + d.len()).sum::<usize>();
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(&IDENTIFIER.to_le_bytes());
        out.extend_from_slice(&HEADER_VERSION.to_le_bytes());
        out.extend_from_slice(&(header_len as u16).to_le_bytes());
        out.extend_from_slice(&h.field_control().to_le_bytes());
        out.extend_from_slice(&h.manufacturer_code.to_le_bytes());
        out.extend_from_slice(&h.image_type.to_le_bytes());
        out.extend_from_slice(&h.file_version.to_le_bytes());
        out.extend_from_slice(&h.stack_version.to_le_bytes());
        let mut s = [0u8; 32];
        let hs = h.header_string.as_bytes();
        let n = hs.len().min(32);
        s[..n].copy_from_slice(&hs[..n]);
        out.extend_from_slice(&s);
        out.extend_from_slice(&(total as u32).to_le_bytes());
        if let Some(v) = h.security_credential_version {
            out.push(v);
        }
        if let Some(d) = h.upgrade_file_destination {
            out.extend_from_slice(&d);
        }
        if let Some((lo, hi)) = h.hardware_versions {
            out.extend_from_slice(&lo.to_le_bytes());
            out.extend_from_slice(&hi.to_le_bytes());
        }
        for (tag, data) in &self.sub_elements {
            out.extend_from_slice(&tag.to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(data);
        }
        out
    }

    pub fn parse(data: &[u8]) -> Result<File, String> {
        let u16_at = |i: usize| data.get(i..i + 2).map(|s| u16::from_le_bytes([s[0], s[1]])).ok_or("truncated");
        let u32_at = |i: usize| data.get(i..i + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap())).ok_or("truncated");
        if data.len() < BASE_HEADER_LEN {
            return Err("shorter than an OTA header".into());
        }
        let id = u32_at(0)?;
        if id != IDENTIFIER {
            return Err(format!("not an OTA file (identifier {id:#010x})"));
        }
        let hlen = u16_at(6)? as usize;
        let fc = u16_at(8)?;
        let hs = &data[20..52];
        let hs_end = hs.iter().position(|&b| b == 0).unwrap_or(32);
        let total = u32_at(52)? as usize;
        let mut off = BASE_HEADER_LEN;
        let mut header = Header {
            manufacturer_code: u16_at(10)?,
            image_type: u16_at(12)?,
            file_version: u32_at(14)?,
            stack_version: u16_at(18)?,
            header_string: String::from_utf8_lossy(&hs[..hs_end]).into_owned(),
            security_credential_version: None,
            upgrade_file_destination: None,
            hardware_versions: None,
        };
        if fc & FC_SECURITY_CREDENTIAL != 0 {
            header.security_credential_version = Some(*data.get(off).ok_or("truncated")?);
            off += 1;
        }
        if fc & FC_DEVICE_SPECIFIC != 0 {
            header.upgrade_file_destination = Some(data.get(off..off + 8).ok_or("truncated")?.try_into().unwrap());
            off += 8;
        }
        if fc & FC_HARDWARE_VERSIONS != 0 {
            header.hardware_versions = Some((u16_at(off)?, u16_at(off + 2)?));
            off += 4;
        }
        if off != hlen {
            return Err(format!("header length {hlen}, the fields take {off}"));
        }
        if total != data.len() {
            return Err(format!("total image size {total}, the file has {}", data.len()));
        }
        let mut sub_elements = Vec::new();
        while off < data.len() {
            let tag = u16_at(off)?;
            let len = u32_at(off + 2)? as usize;
            let d = data.get(off + 6..off + 6 + len).ok_or("sub-element beyond the end")?;
            sub_elements.push((tag, d.to_vec()));
            off += 6 + len;
        }
        Ok(File { header, sub_elements })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_layout() {
        let f = File::with_image(Header::new(0x1141, 0x0390, 0x0002_0005, "outglow ESMLFzm_w6_Dimm-4.63"), vec![1, 2, 3]);
        let b = f.pack();
        assert_eq!(b.len(), 56 + 6 + 3);
        assert_eq!(&b[..4], &[0x1E, 0xF1, 0xEE, 0x0B]);
        assert_eq!(u32::from_le_bytes(b[52..56].try_into().unwrap()) as usize, b.len());
        assert_eq!(File::parse(&b).unwrap(), f);
    }

    #[test]
    fn optional_fields_round_trip() {
        let mut h = Header::new(1, 2, 3, "x");
        h.security_credential_version = Some(1);
        h.upgrade_file_destination = Some([1, 2, 3, 4, 5, 6, 7, 8]);
        h.hardware_versions = Some((1, 9));
        let f = File { header: h, sub_elements: vec![(0, vec![7]), (0xF000, vec![1, 2])] };
        assert_eq!(File::parse(&f.pack()).unwrap(), f);
    }
}
