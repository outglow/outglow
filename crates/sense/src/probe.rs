pub const PAN: u16 = 0x5E55;
pub const FCF: u16 = 0x8841;
pub const MHR_LEN: usize = 9;
pub const PAYLOAD_LEN: usize = 13;
pub const MPDU_LEN: usize = MHR_LEN + PAYLOAD_LEN;
pub const MAGIC: [u8; 2] = *b"EG";
pub const VERSION: u8 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Info {
    pub light_on: bool,
    pub level: u8,
    pub light_changes: u8,
    pub rate: u8,
}

pub fn build(seq: u8, me: u16, network_pan: u16, info: &Info) -> [u8; MPDU_LEN] {
    let mut p = [0u8; MPDU_LEN];
    p[0..2].copy_from_slice(&FCF.to_le_bytes());
    p[2] = seq;
    p[3..5].copy_from_slice(&PAN.to_le_bytes());
    p[5..7].copy_from_slice(&0xFFFFu16.to_le_bytes());
    p[7..9].copy_from_slice(&me.to_le_bytes());
    let b = &mut p[MHR_LEN..];
    b[0..2].copy_from_slice(&MAGIC);
    b[2] = VERSION;
    b[3..5].copy_from_slice(&network_pan.to_le_bytes());
    b[5] = info.light_on as u8;
    b[6] = info.level;
    b[7] = info.light_changes;
    b[8] = seq;
    b[9] = info.rate;
    p
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Heard {
    Probe { src: u16, light_changes: u8 },
    Frame { src: u16 },
    Foreign,
    Other,
}

pub fn classify(m: &[u8], network_pan: u16) -> Heard {
    if m.len() < MHR_LEN {
        return Heard::Other;
    }
    let fcf = u16::from_le_bytes([m[0], m[1]]);
    if fcf & 0xCC40 != 0x8840 || (fcf & 7 != 1 && fcf & 7 != 3) {
        return Heard::Other;
    }
    let pan = u16::from_le_bytes([m[3], m[4]]);
    let src = u16::from_le_bytes([m[7], m[8]]);
    if pan == PAN {
        let p = &m[MHR_LEN..];
        if fcf != FCF
            || m.len() < MPDU_LEN
            || p[0..2] != MAGIC
            || p[2] != VERSION
            || network_pan == 0xFFFF
            || u16::from_le_bytes([p[3], p[4]]) != network_pan
        {
            return Heard::Foreign;
        }
        return Heard::Probe { src, light_changes: p[7] };
    }
    if pan != network_pan || network_pan == 0xFFFF {
        return Heard::Other;
    }
    Heard::Frame { src }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_probe_is_the_c_firmwares_frame() {
        let p = build(7, 0x2C01, 0x3B7C, &Info { light_on: true, level: 127, light_changes: 3, rate: 5 });
        assert_eq!(
            p,
            [0x41, 0x88, 7, 0x55, 0x5E, 0xFF, 0xFF, 0x01, 0x2C, b'E', b'G', 1, 0x7C, 0x3B, 1, 127, 3, 7, 5, 0, 0, 0]
        );
        assert_eq!(classify(&p, 0x3B7C), Heard::Probe { src: 0x2C01, light_changes: 3 });
        assert_eq!(classify(&p, 0x1234), Heard::Foreign);
        assert_eq!(classify(&p, 0xFFFF), Heard::Foreign);
        let mut v = p;
        v[11] = 2;
        assert_eq!(classify(&v, 0x3B7C), Heard::Foreign, "another version");
        assert_eq!(classify(&p[..20], 0x3B7C), Heard::Foreign, "too short");
    }

    #[test]
    fn frames_of_the_network_are_samples_and_others_are_not() {
        let f = [0x41, 0x88, 1, 0x7C, 0x3B, 0xFF, 0xFF, 0x01, 0x2C, 0x08, 0x00];
        assert_eq!(classify(&f, 0x3B7C), Heard::Frame { src: 0x2C01 });
        assert_eq!(classify(&f, 0x0001), Heard::Other, "another network");
        assert_eq!(classify(&f, 0xFFFF), Heard::Other, "no network");
        assert_eq!(classify(&[0x00, 0x80, 1, 0x7C, 0x3B, 0xFF, 0xCF, 0, 0], 0x3B7C), Heard::Other);
        assert_eq!(classify(&[0x02, 0x00, 1], 0x3B7C), Heard::Other);
        assert_eq!(classify(&[0x41, 0xC8, 1, 0x7C, 0x3B, 0xFF, 0xFF, 1, 2, 3, 4, 5, 6, 7, 8], 0x3B7C), Heard::Other);
        assert_eq!(classify(&[0x63, 0x88, 1, 0x7C, 0x3B, 0x00, 0x00, 0x01, 0x2C, 0x04], 0x3B7C), Heard::Frame { src: 0x2C01 });
    }
}
