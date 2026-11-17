use crate::aes::Aes128;
use crate::crypto;
use crate::util::{pack, Buf, Reader};

pub const FRAME_DATA: u8 = 0;
pub const FRAME_CMD: u8 = 1;

pub const BROADCAST_ALL: u16 = 0xFFFF;
pub const BROADCAST_RX_ON: u16 = 0xFFFD;
pub const BROADCAST_ROUTERS: u16 = 0xFFFC;

pub const CMD_ROUTE_REQUEST: u8 = 0x01;
pub const CMD_ROUTE_REPLY: u8 = 0x02;
pub const CMD_NETWORK_STATUS: u8 = 0x03;
pub const CMD_LEAVE: u8 = 0x04;
pub const CMD_ROUTE_RECORD: u8 = 0x05;
pub const CMD_REJOIN_REQUEST: u8 = 0x06;
pub const CMD_REJOIN_RESPONSE: u8 = 0x07;
pub const CMD_LINK_STATUS: u8 = 0x08;
pub const CMD_NETWORK_REPORT: u8 = 0x09;
pub const CMD_NETWORK_UPDATE: u8 = 0x0A;
pub const CMD_TIMEOUT_REQUEST: u8 = 0x0B;
pub const CMD_TIMEOUT_RESPONSE: u8 = 0x0C;

pub const STATUS_NO_ROUTE: u8 = 0x00;
pub const STATUS_TREE_LINK_FAILURE: u8 = 0x01;
pub const STATUS_LINK_FAILURE: u8 = 0x02;
pub const STATUS_SOURCE_ROUTE_FAILURE: u8 = 0x0B;
pub const STATUS_MANY_TO_ONE_FAILURE: u8 = 0x0C;
pub const STATUS_ADDRESS_CONFLICT: u8 = 0x0D;
pub const STATUS_UNKNOWN_COMMAND: u8 = 0x13;

pub const PROTOCOL_VERSION: u8 = 2;
pub const DEFAULT_RADIUS: u8 = 30;

pub fn is_broadcast(addr: u16) -> bool {
    addr >= 0xFFF8
}

pub const MAX_RELAYS: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub frame_type: u8,
    pub discover: u8,
    pub security: bool,
    pub dst: u16,
    pub src: u16,
    pub radius: u8,
    pub seq: u8,
    pub dst_ext: Option<u64>,
    pub src_ext: Option<u64>,
    pub multicast: Option<u8>,
    pub relays: Option<(u8, u8, [u16; MAX_RELAYS])>,
    pub end_device_initiator: bool,
}

impl Header {
    pub fn new(frame_type: u8, dst: u16, src: u16, radius: u8, seq: u8) -> Header {
        Header {
            frame_type,
            discover: 0,
            security: true,
            dst,
            src,
            radius,
            seq,
            dst_ext: None,
            src_ext: None,
            multicast: None,
            relays: None,
            end_device_initiator: false,
        }
    }

    pub fn parse(frame: &[u8]) -> Option<(Header, usize)> {
        let mut r = Reader::new(frame);
        let fc = r.u16()?;
        if ((fc >> 2) & 0x0F) as u8 != PROTOCOL_VERSION {
            return None;
        }
        let mut h = Header::new((fc & 3) as u8, r.u16()?, r.u16()?, r.u8()?, r.u8()?);
        h.discover = ((fc >> 6) & 3) as u8;
        h.security = fc & 0x0200 != 0;
        if fc & 0x0800 != 0 {
            h.dst_ext = Some(r.u64()?);
        }
        if fc & 0x1000 != 0 {
            h.src_ext = Some(r.u64()?);
        }
        if fc & 0x0100 != 0 {
            h.multicast = Some(r.u8()?);
        }
        if fc & 0x0400 != 0 {
            let count = r.u8()?;
            let index = r.u8()?;
            if count as usize > MAX_RELAYS {
                return None;
            }
            let mut list = [0u16; MAX_RELAYS];
            for slot in list.iter_mut().take(count as usize) {
                *slot = r.u16()?;
            }
            h.relays = Some((count, index, list));
        }
        h.end_device_initiator = fc & 0x2000 != 0;
        Some((h, r.pos()))
    }

    pub fn write(&self, b: &mut Buf) {
        let fc = pack(&[
            (self.frame_type as u16, 0),
            (PROTOCOL_VERSION as u16, 2),
            (self.discover as u16, 6),
            (self.multicast.is_some() as u16, 8),
            (self.security as u16, 9),
            (self.relays.is_some() as u16, 10),
            (self.dst_ext.is_some() as u16, 11),
            (self.src_ext.is_some() as u16, 12),
            (self.end_device_initiator as u16, 13),
        ]);
        b.u16(fc);
        b.u16(self.dst);
        b.u16(self.src);
        b.u8(self.radius);
        b.u8(self.seq);
        if let Some(e) = self.dst_ext {
            b.u64(e);
        }
        if let Some(e) = self.src_ext {
            b.u64(e);
        }
        if let Some(m) = self.multicast {
            b.u8(m);
        }
        if let Some((count, index, list)) = &self.relays {
            b.u8(*count);
            b.u8(*index);
            for relay in list.iter().take(*count as usize) {
                b.u16(*relay);
            }
        }
    }
}

pub const AUX_LEN: usize = 14;
const SC_AIR: u8 = 0x28;
const SC_CALC: u8 = 0x2D;

pub(crate) fn nonce(ext: u64, counter: u32, sc: u8) -> [u8; 13] {
    let mut n = [0u8; 13];
    n[..8].copy_from_slice(&ext.to_le_bytes());
    n[8..12].copy_from_slice(&counter.to_le_bytes());
    n[12] = sc;
    n
}

pub fn secure(key: &Aes128, key_seq: u8, ext: u64, counter: u32, out: &mut Buf, header_at: usize, payload: &[u8]) {
    let aux_at = out.len();
    out.u8(SC_CALC);
    out.u32(counter);
    out.u64(ext);
    out.u8(key_seq);
    let body_at = out.len();
    out.bytes(payload);
    out.bytes(&[0; crypto::MIC_LEN]);
    if !out.ok() {
        return;
    }
    let (head, body) = out.as_mut_slice().split_at_mut(body_at);
    let _ = crypto::ccm_encrypt(key, &nonce(ext, counter, SC_CALC), &head[header_at..], body, payload.len());
    out.set(aux_at, SC_AIR);
}

#[derive(Debug, Clone, Copy)]
pub struct Aux {
    pub counter: u32,
    pub ext: u64,
    pub key_seq: u8,
}

pub fn parse_aux(frame: &[u8], header_len: usize) -> Option<Aux> {
    let mut r = Reader::new(frame.get(header_len..)?);
    let sc = r.u8()?;
    if sc & 0x38 != 0x28 {
        return None;
    }
    Some(Aux { counter: r.u32()?, ext: r.u64()?, key_seq: r.u8()? })
}

pub fn unsecure(key: &Aes128, aux: &Aux, frame: &mut [u8], header_len: usize) -> Option<usize> {
    let (aad, body) = frame.split_at_mut_checked(header_len + AUX_LEN)?;
    let sc = pack(&[(aad[header_len] & !7, 0), (5, 0)]);
    aad[header_len] = sc;
    crypto::ccm_decrypt(key, &nonce(aux.ext, aux.counter, sc), aad, body)
}

pub fn cost_from_lqi(lqi: u8) -> u8 {
    match lqi {
        119..=255 => 1,
        95..=118 => 2,
        70..=94 => 3,
        46..=69 => 5,
        _ => 7,
    }
}

pub fn cost_from_delivery(p: u16) -> u8 {
    match p {
        59218..=65535 => 1,
        52119..=59217 => 2,
        47914..=52118 => 3,
        44996..=47913 => 4,
        42794..=44995 => 5,
        41044..=42793 => 6,
        _ => 7,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_cost_from_the_probability_of_delivery_is_the_formula_of_the_spec() {
        for q in 0..=65535u16 {
            let p = q as f64 / 65535.0;
            let expected = if q == 0 { 7 } else { (1.0 / p.powi(4)).round().min(7.0) as u8 };
            assert_eq!(cost_from_delivery(q), expected, "p = {q}/65535");
        }
    }

    #[test]
    fn the_initial_link_cost_follows_the_radio_vendors_table() {
        let lqi = |dbm: i32| (255 * (dbm + 99) / 84) as u8;
        assert_eq!(cost_from_lqi(lqi(-55)), 1);
        assert_eq!(cost_from_lqi(lqi(-65)), 2);
        assert_eq!(cost_from_lqi(lqi(-72)), 3);
        assert_eq!(cost_from_lqi(lqi(-80)), 5);
        assert_eq!(cost_from_lqi(lqi(-90)), 7);
        assert_eq!((cost_from_lqi(119), cost_from_lqi(118)), (1, 2));
        assert_eq!((cost_from_lqi(95), cost_from_lqi(94)), (2, 3));
        assert_eq!((cost_from_lqi(70), cost_from_lqi(69)), (3, 5));
        assert_eq!((cost_from_lqi(46), cost_from_lqi(45)), (5, 7));
    }

    #[test]
    fn header_round_trip_with_all_optional_fields() {
        let mut h = Header::new(FRAME_DATA, 0x1234, 0x0000, 29, 7);
        h.discover = 1;
        h.dst_ext = Some(0x1122334455667788);
        h.src_ext = Some(0x8877665544332211);
        let mut list = [0u16; MAX_RELAYS];
        list[0] = 0xAAAA;
        list[1] = 0xBBBB;
        h.relays = Some((2, 1, list));
        h.end_device_initiator = true;
        let mut b = Buf::new();
        h.write(&mut b);
        assert_eq!(b.len(), 8 + 8 + 8 + 2 + 4);
        assert_eq!(Header::parse(b.as_slice()), Some((h, b.len())));
    }

    #[test]
    fn wrong_protocol_version_is_rejected() {
        let mut b = Buf::new();
        Header::new(FRAME_DATA, 1, 2, 3, 4).write(&mut b);
        let mut raw = [0u8; 8];
        raw.copy_from_slice(b.as_slice());
        raw[0] = (raw[0] & !0x3C) | (3 << 2);
        assert_eq!(Header::parse(&raw), None);
    }

    #[test]
    fn secure_then_unsecure_recovers_the_payload() {
        let key = Aes128::new(&[7u8; 16]);
        let mut full = Buf::from_slice(&[0xAA, 0xBB, 0xCC]);
        Header::new(FRAME_CMD, 0xFFFC, 0x7A21, 1, 9).write(&mut full);
        let payload = [0x08, 0x61, 0x00, 0x00, 0x11];
        secure(&key, 0, 0x00124B0000000001, 0x1000, &mut full, 3, &payload);
        assert!(full.ok());
        let out = Buf::from_slice(&full.as_slice()[3..]);
        assert_eq!(out.len(), 8 + AUX_LEN + payload.len() + 4);
        assert_eq!(out.get(8), 0x28);
        let aux = parse_aux(out.as_slice(), 8).unwrap();
        assert_eq!((aux.counter, aux.ext, aux.key_seq), (0x1000, 0x00124B0000000001, 0));
        let mut frame = [0u8; 64];
        let n = out.len();
        frame[..n].copy_from_slice(out.as_slice());
        let plen = unsecure(&key, &aux, &mut frame[..n], 8).unwrap();
        assert_eq!(&frame[8 + AUX_LEN..8 + AUX_LEN + plen], &payload);
        frame[..n].copy_from_slice(out.as_slice());
        frame[6] ^= 1;
        assert!(unsecure(&key, &aux, &mut frame[..n], 8).is_none());
    }
}
