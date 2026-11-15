//! The probe: an 802.15.4 broadcast every bulb with sensing sends a few
//! times a second, so that its neighbours have frames to measure. The same
//! frame as the C firmware's (`firmware/sense/eglo_sense.c`):
//!
//! ```text
//! MHR  41 88 | seq | 55 5E | FF FF | sender's short address (2)
//!        FCF 0x8841: data, no ACK, PAN compression, short addresses; destination PAN 0x5E55
//! payload 'E' 'G' | version 1 | network PAN (2) | flags (bit0 = light on) | level |
//!        light change counter | probe counter | probes/s | 3 B spare
//! ```
//!
//! The foreign PAN makes devices without sensing drop it in their MAC
//! filter: nobody acknowledges it or passes it on.

/// Destination PAN of the probes (as in Ripple).
pub const PAN: u16 = 0x5E55;
pub const FCF: u16 = 0x8841;
pub const MHR_LEN: usize = 9;
pub const PAYLOAD_LEN: usize = 13;
/// The probe without its FCS: 24 bytes on the air with it.
pub const MPDU_LEN: usize = MHR_LEN + PAYLOAD_LEN;
pub const MAGIC: [u8; 2] = *b"EG";
pub const VERSION: u8 = 1;

/// What a probe tells its receivers about its sender.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Info {
    pub light_on: bool,
    pub level: u8,
    /// Counts the sender's light changes: a jump of the level that comes with
    /// one is not motion.
    pub light_changes: u8,
    /// Probes per second the sender sends.
    pub rate: u8,
}

/// The probe of the sender `me` in the network `network_pan`.
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

/// What a received frame is to the detector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Heard {
    /// A probe from a bulb of our network: a sample, and the frame goes no further.
    Probe { src: u16, light_changes: u8 },
    /// A frame of our network from a neighbour (short addresses, PAN
    /// compression): a sample, and the frame goes on to the stack.
    Frame { src: u16 },
    /// A probe of another network or of another kind: dropped.
    Foreign,
    /// Anything else: not a sample, on to the stack.
    Other,
}

/// Sorts a received MPDU (without FCS). `network_pan`: 0xFFFF when not in a
/// network, and then nothing is a sample.
pub fn classify(m: &[u8], network_pan: u16) -> Heard {
    if m.len() < MHR_LEN {
        return Heard::Other;
    }
    let fcf = u16::from_le_bytes([m[0], m[1]]);
    // destination and source addresses short, PAN compression; data frame or MAC command
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
        // a probe of another network, or while we are in none: dropped, no sample
        assert_eq!(classify(&p, 0x1234), Heard::Foreign);
        assert_eq!(classify(&p, 0xFFFF), Heard::Foreign);
        let mut v = p;
        v[11] = 2;
        assert_eq!(classify(&v, 0x3B7C), Heard::Foreign, "another version");
        assert_eq!(classify(&p[..20], 0x3B7C), Heard::Foreign, "too short");
    }

    #[test]
    fn frames_of_the_network_are_samples_and_others_are_not() {
        // NWK data frame, broadcast, short addresses, PAN compression, our PAN
        let f = [0x41, 0x88, 1, 0x7C, 0x3B, 0xFF, 0xFF, 0x01, 0x2C, 0x08, 0x00];
        assert_eq!(classify(&f, 0x3B7C), Heard::Frame { src: 0x2C01 });
        assert_eq!(classify(&f, 0x0001), Heard::Other, "another network");
        assert_eq!(classify(&f, 0xFFFF), Heard::Other, "no network");
        // a beacon (frame type 0), an acknowledgement, an extended source address
        assert_eq!(classify(&[0x00, 0x80, 1, 0x7C, 0x3B, 0xFF, 0xCF, 0, 0], 0x3B7C), Heard::Other);
        assert_eq!(classify(&[0x02, 0x00, 1], 0x3B7C), Heard::Other);
        assert_eq!(classify(&[0x41, 0xC8, 1, 0x7C, 0x3B, 0xFF, 0xFF, 1, 2, 3, 4, 5, 6, 7, 8], 0x3B7C), Heard::Other);
        // a MAC command with short addresses counts too (as in the C firmware)
        assert_eq!(classify(&[0x63, 0x88, 1, 0x7C, 0x3B, 0x00, 0x00, 0x01, 0x2C, 0x04], 0x3B7C), Heard::Frame { src: 0x2C01 });
    }
}
