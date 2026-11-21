use crate::hal::{ADV_PDU_MAX, CONNECT_LEN};

pub const ADV_ACCESS_ADDRESS: u32 = 0x8E89_BED6;
pub const ADV_CRC_INIT: u32 = 0x55_5555;

pub const ADV_IND: u8 = 0;
pub const SCAN_REQ: u8 = 3;
pub const SCAN_RSP: u8 = 4;
pub const CONNECT_IND: u8 = 5;

pub const LLID_CONTINUE: u8 = 1;
pub const LLID_START: u8 = 2;
pub const LLID_CONTROL: u8 = 3;

pub fn advertising(kind: u8, addr: &[u8; 6], data: &[u8], out: &mut [u8; ADV_PDU_MAX]) -> usize {
    let n = data.len().min(31);
    out[0] = kind;
    out[1] = (6 + n) as u8;
    out[2..8].copy_from_slice(addr);
    out[8..8 + n].copy_from_slice(&data[..n]);
    8 + n
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectInd {
    pub access_address: u32,
    pub crc_init: u32,
    pub win_size: u8,
    pub win_offset: u16,
    pub interval: u16,
    pub latency: u16,
    pub timeout: u16,
    pub channel_map: [u8; 5],
    pub hop: u8,
    pub sca: u8,
}

pub fn parse_connect(p: &[u8; CONNECT_LEN]) -> Option<ConnectInd> {
    let d = &p[12..];
    let c = ConnectInd {
        access_address: u32::from_le_bytes([d[0], d[1], d[2], d[3]]),
        crc_init: u32::from_le_bytes([d[4], d[5], d[6], 0]),
        win_size: d[7],
        win_offset: u16::from_le_bytes([d[8], d[9]]),
        interval: u16::from_le_bytes([d[10], d[11]]),
        latency: u16::from_le_bytes([d[12], d[13]]),
        timeout: u16::from_le_bytes([d[14], d[15]]),
        channel_map: [d[16], d[17], d[18], d[19], d[20] & 0x1f],
        hop: d[21] & 0x1f,
        sca: d[21] >> 5,
    };
    let valid = timing_valid(c.win_size, c.win_offset, c.interval, c.latency, c.timeout)
        && (5..=16).contains(&c.hop)
        && used_channels(&c.channel_map) >= 2;
    valid.then_some(c)
}

pub fn timing_valid(win_size: u8, win_offset: u16, interval: u16, latency: u16, timeout: u16) -> bool {
    (6..=3200).contains(&interval)
        && (10..=3200).contains(&timeout)
        && latency <= 499
        && (1..=8.min(interval - 1)).contains(&u16::from(win_size))
        && win_offset <= interval
}

pub fn used_channels(map: &[u8; 5]) -> u8 {
    map.iter().map(|b| b.count_ones() as u8).sum()
}

fn is_used(map: &[u8; 5], channel: u8) -> bool {
    map[(channel >> 3) as usize] >> (channel & 7) & 1 != 0
}

pub fn next_channel(unmapped: &mut u8, hop: u8, map: &[u8; 5]) -> u8 {
    *unmapped = (*unmapped + hop) % 37;
    if is_used(map, *unmapped) {
        return *unmapped;
    }
    let index = *unmapped % used_channels(map).max(1);
    (0..37).filter(|c| is_used(map, *c)).nth(index as usize).unwrap_or(*unmapped)
}

pub fn sca_ppm(sca: u8) -> u32 {
    [500, 250, 150, 100, 75, 50, 30, 20][(sca & 7) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_selection_follows_the_specification_samples() {
        let all = [0xff, 0xff, 0xff, 0xff, 0x1f];
        let mut u = 0;
        let got: [u8; 6] = core::array::from_fn(|_| next_channel(&mut u, 7, &all));
        assert_eq!(got, [7, 14, 21, 28, 35, 5]);
        let some = [0x00, 0xfe, 0x03, 0x00, 0x00];
        let mut u = 0;
        assert_eq!(next_channel(&mut u, 7, &some), 9 + 7);
        assert_eq!(next_channel(&mut u, 7, &some), 14);
        assert_eq!(next_channel(&mut u, 7, &some), 9 + 21 % 9);
    }

    #[test]
    fn a_connect_request_with_bad_parameters_is_refused() {
        let mut p = [0u8; CONNECT_LEN];
        p[12..16].copy_from_slice(&0x7176_4129u32.to_le_bytes());
        p[16..19].copy_from_slice(&[0x55, 0x44, 0x33]);
        p[19] = 2;
        p[22..24].copy_from_slice(&24u16.to_le_bytes());
        p[26..28].copy_from_slice(&500u16.to_le_bytes());
        p[28..33].copy_from_slice(&[0xff, 0xff, 0xff, 0xff, 0x1f]);
        p[33] = 7 | 1 << 5;
        let c = parse_connect(&p).unwrap();
        assert_eq!((c.access_address, c.crc_init, c.interval, c.timeout, c.hop, c.sca), (0x7176_4129, 0x33_4455, 24, 500, 7, 1));
        let mut bad = p;
        bad[33] = 3;
        assert!(parse_connect(&bad).is_none(), "hop below 5");
        let mut bad = p;
        bad[22] = 5;
        assert!(parse_connect(&bad).is_none(), "interval below 7.5 ms");
        let mut bad = p;
        bad[28..33].copy_from_slice(&[1, 0, 0, 0, 0]);
        assert!(parse_connect(&bad).is_none(), "one channel");
    }

    #[test]
    fn connection_timing_within_the_limits_of_the_specification() {
        let cases = [
            ((2, 0, 24, 0, 500), true),
            ((2, 0, 5, 0, 500), false),
            ((5, 6, 6, 0, 500), true),
            ((6, 0, 6, 0, 500), false),
            ((2, 0, 3200, 0, 3200), true),
            ((2, 0, 3201, 0, 3200), false),
            ((2, 0, 24, 0, 9), false),
            ((2, 0, 24, 0, 10), true),
            ((2, 0, 24, 0, 3201), false),
            ((2, 0, 24, 499, 500), true),
            ((2, 0, 24, 500, 500), false),
            ((0, 0, 24, 0, 500), false),
            ((1, 0, 24, 0, 500), true),
            ((8, 0, 24, 0, 500), true),
            ((9, 0, 24, 0, 500), false),
            ((2, 24, 24, 0, 500), true),
            ((2, 25, 24, 0, 500), false),
        ];
        for ((size, offset, interval, latency, timeout), ok) in cases {
            assert_eq!(timing_valid(size, offset, interval, latency, timeout), ok, "{size} {offset} {interval} {latency} {timeout}");
        }
    }
}
