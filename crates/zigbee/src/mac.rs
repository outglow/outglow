use crate::util::{Buf, Reader};

pub const FRAME_BEACON: u8 = 0;
pub const FRAME_DATA: u8 = 1;
pub const FRAME_ACK: u8 = 2;
pub const FRAME_CMD: u8 = 3;

pub const CMD_ASSOC_REQ: u8 = 0x01;
pub const CMD_ASSOC_RSP: u8 = 0x02;
pub const CMD_DATA_REQ: u8 = 0x04;
pub const CMD_ORPHAN: u8 = 0x06;
pub const CMD_BEACON_REQ: u8 = 0x07;
pub const CMD_REALIGNMENT: u8 = 0x08;

pub const BROADCAST: u16 = 0xFFFF;

pub const CAPABILITY_ROUTER: u8 = 0x8E;
pub const CAPABILITY_ALLOCATE_ADDRESS: u8 = 0x80;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Addr {
    None,
    Short(u16),
    Ext(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub frame_type: u8,
    pub security: bool,
    pub pending: bool,
    pub ack_request: bool,
    pub seq: u8,
    pub dst_pan: u16,
    pub dst: Addr,
    pub src_pan: u16,
    pub src: Addr,
}

impl Header {
    pub fn parse(mpdu: &[u8]) -> Option<(Header, usize)> {
        let mut r = Reader::new(mpdu);
        let fcf = r.u16()?;
        let seq = r.u8()?;
        let pan_compression = fcf & 0x0040 != 0;
        let mut h = Header {
            frame_type: (fcf & 7) as u8,
            security: fcf & 0x0008 != 0,
            pending: fcf & 0x0010 != 0,
            ack_request: fcf & 0x0020 != 0,
            seq,
            dst_pan: 0,
            dst: Addr::None,
            src_pan: 0,
            src: Addr::None,
        };
        match (fcf >> 10) & 3 {
            0 => {}
            2 => {
                h.dst_pan = r.u16()?;
                h.dst = Addr::Short(r.u16()?);
            }
            3 => {
                h.dst_pan = r.u16()?;
                h.dst = Addr::Ext(r.u64()?);
            }
            _ => return None,
        }
        let src_mode = (fcf >> 14) & 3;
        if src_mode != 0 {
            h.src_pan = if pan_compression && h.dst != Addr::None { h.dst_pan } else { r.u16()? };
            h.src = match src_mode {
                2 => Addr::Short(r.u16()?),
                3 => Addr::Ext(r.u64()?),
                _ => return None,
            };
        }
        Some((h, r.pos()))
    }

    pub fn write(&self, b: &mut Buf) {
        let both = self.dst != Addr::None && self.src != Addr::None;
        let compress = both && self.src_pan == self.dst_pan;
        let mode = |a: &Addr| match a {
            Addr::None => 0u16,
            Addr::Short(_) => 2,
            Addr::Ext(_) => 3,
        };
        let fcf = self.frame_type as u16
            | (self.security as u16) << 3
            | (self.pending as u16) << 4
            | (self.ack_request as u16) << 5
            | (compress as u16) << 6
            | mode(&self.dst) << 10
            | mode(&self.src) << 14;
        b.u16(fcf);
        b.u8(self.seq);
        let put = |b: &mut Buf, a: &Addr| match a {
            Addr::None => {}
            Addr::Short(s) => b.u16(*s),
            Addr::Ext(e) => b.u64(*e),
        };
        if self.dst != Addr::None {
            b.u16(self.dst_pan);
            put(b, &self.dst);
        }
        if self.src != Addr::None {
            if !compress {
                b.u16(self.src_pan);
            }
            put(b, &self.src);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Beacon {
    pub pan: u16,
    pub short: u16,
    pub permit_join: bool,
    pub pan_coordinator: bool,
    pub stack_profile: u8,
    pub protocol_version: u8,
    pub router_capacity: bool,
    pub end_device_capacity: bool,
    pub depth: u8,
    pub ext_pan: u64,
    pub update_id: u8,
}

impl Beacon {
    pub fn parse(h: &Header, payload: &[u8]) -> Option<Beacon> {
        let short = match h.src {
            Addr::Short(s) => s,
            _ => return None,
        };
        let mut r = Reader::new(payload);
        let superframe = r.u16()?;
        let gts = r.u8()?;
        r.skip(if gts & 7 != 0 { 1 + 3 * (gts & 7) as usize } else { 0 })?;
        let pend = r.u8()?;
        r.skip(2 * (pend & 7) as usize + 8 * ((pend >> 4) & 7) as usize)?;
        if r.u8()? != 0 {
            return None;
        }
        let a = r.u8()?;
        let b = r.u8()?;
        let ext_pan = r.u64()?;
        r.skip(3)?;
        let update_id = r.u8().unwrap_or(0);
        Some(Beacon {
            pan: h.src_pan,
            short,
            permit_join: superframe & 0x8000 != 0,
            pan_coordinator: superframe & 0x4000 != 0,
            stack_profile: a & 0x0F,
            protocol_version: a >> 4,
            router_capacity: b & 0x04 != 0,
            end_device_capacity: b & 0x80 != 0,
            depth: (b >> 3) & 0x0F,
            ext_pan,
            update_id,
        })
    }

    pub fn write(&self, seq: u8, b: &mut Buf) {
        Header {
            frame_type: FRAME_BEACON,
            security: false,
            pending: false,
            ack_request: false,
            seq,
            dst_pan: 0,
            dst: Addr::None,
            src_pan: self.pan,
            src: Addr::Short(self.short),
        }
        .write(b);
        b.u16(0x0FFF | (self.pan_coordinator as u16) << 14 | (self.permit_join as u16) << 15);
        b.u8(0);
        b.u8(0);
        b.u8(0);
        b.u8(self.stack_profile | self.protocol_version << 4);
        b.u8((self.router_capacity as u8) << 2 | (self.depth & 0x0F) << 3 | (self.end_device_capacity as u8) << 7);
        b.u64(self.ext_pan);
        b.bytes(&[0xFF, 0xFF, 0xFF]);
        b.u8(self.update_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_frame_header_round_trip() {
        let h = Header {
            frame_type: FRAME_DATA,
            security: false,
            pending: false,
            ack_request: true,
            seq: 0x42,
            dst_pan: 0x1A62,
            dst: Addr::Short(0x0000),
            src_pan: 0x1A62,
            src: Addr::Short(0x7A21),
        };
        let mut b = Buf::new();
        h.write(&mut b);
        assert_eq!(b.as_slice(), &[0x61, 0x88, 0x42, 0x62, 0x1A, 0x00, 0x00, 0x21, 0x7A]);
        assert_eq!(Header::parse(b.as_slice()), Some((h, 9)));
    }

    #[test]
    fn association_request_has_both_pans() {
        let h = Header {
            frame_type: FRAME_CMD,
            security: false,
            pending: false,
            ack_request: true,
            seq: 1,
            dst_pan: 0x1A62,
            dst: Addr::Short(0),
            src_pan: 0xFFFF,
            src: Addr::Ext(0x0011223344556677),
        };
        let mut b = Buf::new();
        h.write(&mut b);
        assert_eq!(&b.as_slice()[..2], &[0x23, 0xC8]);
        assert_eq!(b.len(), 3 + 4 + 2 + 8);
        assert_eq!(Header::parse(b.as_slice()), Some((h, 17)));
    }

    #[test]
    fn beacon_round_trip() {
        let bc = Beacon {
            pan: 0x1A62,
            short: 0,
            permit_join: true,
            pan_coordinator: true,
            stack_profile: 2,
            protocol_version: 2,
            router_capacity: true,
            end_device_capacity: true,
            depth: 0,
            ext_pan: 0xDDDDDDDDDDDDDDDD,
            update_id: 0,
        };
        let mut b = Buf::new();
        bc.write(7, &mut b);
        let (h, off) = Header::parse(b.as_slice()).unwrap();
        assert_eq!(h.frame_type, FRAME_BEACON);
        assert_eq!(Beacon::parse(&h, &b.as_slice()[off..]), Some(bc));
    }

    #[test]
    fn truncated_frames_do_not_parse() {
        assert_eq!(Header::parse(&[0x61, 0x88, 0x42, 0x62]), None);
        assert_eq!(Header::parse(&[]), None);
    }
}
