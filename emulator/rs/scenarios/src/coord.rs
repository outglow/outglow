use std::collections::{HashMap, HashSet};

use crate::air::{Air, Ev, NodeId};
use crate::crypto::{ccm_decrypt, ccm_encrypt, hmac_mmo};
use crate::frames::{airtime_us, aps_cmd, ext_bytes, with_fcs, fmt_ext, mac_build, mac_parse, secure, Key, Mac, BCAST_ALL, BCAST_ROUTERS, BCAST_RXON};
pub use crate::station::{Sent, SrcIeee, Tx, EP, HA};
use crate::station::{nx, Station, StationEv};

pub const TCLK_DEFAULT: Key = *b"ZigBeeAlliance09";
pub const NWK_KEY: Key = [0x01, 0x03, 0x05, 0x07, 0x09, 0x0b, 0x0d, 0x0f, 0x00, 0x02, 0x04, 0x06, 0x08, 0x0a, 0x0c, 0x0d];
const UPPER_RETRIES: u32 = 2;

pub enum CoordEv {
    LinkStatusTick,
    TransportKey { ext: u64 },
    AckTimeout(u8),
}

#[derive(Clone, Debug)]
pub struct Device {
    pub ext: u64,
    pub short: u16,
    pub cap: u8,
    pub fc_in: i64,
    pub aps_fc_in: i64,
    pub joined: bool,
    pub announced: bool,
    pub tclk: Option<Key>,
    pub tclk_new: Option<Key>,
    pub link_status: bool,
    pub parent: Option<u16>,
}

#[derive(Clone, Debug)]
pub struct CoordEvent {
    pub ms: f64,
    pub kind: String,
    pub details: String,
}

#[derive(Clone, Debug)]
pub struct ZclRsp {
    pub ms: f64,
    pub src: u16,
    pub ep: u8,
    pub cluster: u16,
    pub cmd: Option<u8>,
    pub body: Vec<u8>,
}

pub struct Coord {
    pub st: Station,
    pub ext: u64,
    pub ext_pan: u64,
    pub tclk: Key,
    key_transport: Key,
    pub permit_join: Option<bool>,
    pub stack_revision: u16,
    pub short: u16,
    pub aps_fc_out: u32,
    next_short: u16,
    pub devices: Vec<Device>,
    pub by_short: HashMap<u16, u64>,
    pending: HashSet<u64>,
    seen_bcast: HashMap<(u16, u8), f64>,
    pub events: Vec<CoordEvent>,
    pub ls_period_ms: f64,
    pub zcl_rsp: Vec<ZclRsp>,
    pub source_routes: HashMap<u16, Vec<u16>>,
    fc_in_ext: HashMap<u64, i64>,
    pub route_records: Vec<(f64, u16, Vec<u16>)>,
    pub mute_aps_ack: bool,
    pub mute_route_reply: bool,
    pub mute_tclk: bool,
    pub tclk_key_load: bool,
    pub aps_fc_per_key: bool,
    aps_fc_by_key: Vec<(Key, u32)>,
    tclk_gen: u8,
    pub aps_cmds: Vec<(f64, u16, Vec<u8>)>,
    pub confirm_keys: Vec<(u16, u8)>,
    pub plain: bool,
    waiting_ack: HashMap<u8, (Vec<u8>, u32, u32)>,
    pub ota_images: Vec<((u16, u16), (u32, Vec<u8>))>,
    pub ota_log: Vec<(f64, String, String)>,
    pub nwk_discover: u8,
}

fn u16_at(p: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes(p.get(i..i + 2)?.try_into().ok()?))
}
fn u32_at(p: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(p.get(i..i + 4)?.try_into().ok()?))
}
fn u64_at(p: &[u8], i: usize) -> Option<u64> {
    Some(u64::from_le_bytes(p.get(i..i + 8)?.try_into().ok()?))
}
fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

impl CoordEvent {
    pub fn get(&self, k: &str) -> Option<&str> {
        self.details.split(", ").find_map(|kv| kv.strip_prefix(k).and_then(|r| r.strip_prefix('=')))
    }
}

impl Coord {
    pub fn new_plain(channel: i64, nwk_key: Key) -> Coord {
        let mut c = Coord::new(channel, nwk_key);
        c.plain = true;
        c.tclk_key_load = false;
        c
    }

    pub fn new(channel: i64, nwk_key: Key) -> Coord {
        let tclk = TCLK_DEFAULT;
        let ext = 0x00124B0000000001;
        Coord {
            st: Station::new(NodeId::Coord, channel, 0x1A62, Some(0x0000), ext, nwk_key, (0x40, 0x10, 0x20, 0x30, 0x50), 1,
                             UPPER_RETRIES),
            ext,
            ext_pan: 0xDDDDDDDDDDDDDDDD,
            tclk,
            key_transport: hmac_mmo(&tclk, &[0x00]),
            permit_join: Some(true),
            stack_revision: 22,
            short: 0x0000,
            aps_fc_out: 1,
            next_short: 0x7A21,
            devices: Vec::new(),
            by_short: HashMap::new(),
            pending: HashSet::new(),
            seen_bcast: HashMap::new(),
            events: Vec::new(),
            ls_period_ms: 15000.0,
            zcl_rsp: Vec::new(),
            source_routes: HashMap::new(),
            fc_in_ext: HashMap::new(),
            route_records: Vec::new(),
            mute_aps_ack: false,
            mute_route_reply: false,
            mute_tclk: false,
            tclk_key_load: true,
            aps_fc_per_key: false,
            aps_fc_by_key: Vec::new(),
            tclk_gen: 0,
            aps_cmds: Vec::new(),
            confirm_keys: Vec::new(),
            plain: false,
            waiting_ack: HashMap::new(),
            ota_images: Vec::new(),
            ota_log: Vec::new(),
            nwk_discover: 0,
        }
    }

    fn dev_mut(&mut self, ext: u64) -> Option<&mut Device> {
        self.devices.iter_mut().find(|d| d.ext == ext)
    }
    pub fn set_next_short(&mut self, short: u16) {
        self.next_short = short;
    }

    pub fn dev_by_short(&self, short: u16) -> Option<&Device> {
        let ext = *self.by_short.get(&short)?;
        self.devices.iter().find(|d| d.ext == ext)
    }
    fn dev_by_short_mut(&mut self, short: u16) -> Option<&mut Device> {
        let ext = *self.by_short.get(&short)?;
        self.dev_mut(ext)
    }

    pub fn has_event(&self, kind: &str) -> bool {
        self.events.iter().any(|e| e.kind == kind)
    }

    fn ev(&mut self, air: &Air, kind: &str, details: String) {
        let t = air.now();
        air.log(&format!("[{t:9.3} ms] coord: {kind} {details}"));
        self.events.push(CoordEvent { ms: t, kind: kind.to_string(), details });
    }

    pub fn start(&mut self, air: &mut Air) {
        let t = air.now() + self.ls_period_ms;
        air.at(t, self.cev(CoordEv::LinkStatusTick));
    }

    pub fn on_event(&mut self, air: &mut Air, e: CoordEv) {
        match e {
            CoordEv::LinkStatusTick => self.link_status_tick(air),
            CoordEv::TransportKey { ext } => self.send_transport_key(air, ext),
            CoordEv::AckTimeout(seq) => self.ack_timeout(air, seq),
        }
    }

    pub fn send_psdu(&mut self, air: &mut Air, mpdu: Vec<u8>, delay_ms: f64, ack_wait: Option<u32>, upper: Option<u32>) {
        if !self.plain {
            return self.st.send_psdu(air, mpdu, delay_ms, ack_wait, upper);
        }
        let psdu = with_fcs(&mpdu);
        let now = air.now();
        let t0 = (now + delay_ms).max(self.st.busy_until + 0.2);
        let t1 = t0 + airtime_us(psdu.len()) / 1000.0;
        self.st.busy_until = t1;
        air.at(t0, Ev::Node(self.st.id, StationEv::Transmit { psdu, t0, t1 }));
        if let Some(r) = ack_wait {
            let seq = mpdu[2];
            self.waiting_ack.insert(seq, (mpdu, r, upper.unwrap_or(2)));
            air.at(t1 + 1.5, self.cev(CoordEv::AckTimeout(seq)));
        }
    }

    fn ack_timeout(&mut self, air: &mut Air, seq: u8) {
        let Some((mpdu, retries, upper)) = self.waiting_ack.get(&seq).cloned() else { return };
        self.waiting_ack.remove(&seq);
        if retries == 0 {
            if upper > 0 {
                self.ev(air, "mac-retry-upper", format!("seq={seq}, left={}", upper - 1));
                self.send_psdu(air, mpdu, 100.0, Some(3), Some(upper - 1));
                return;
            }
            self.ev(air, "mac-no-ack", format!("seq={seq}"));
            return;
        }
        self.send_psdu(air, mpdu, 1.0, Some(retries - 1), Some(upper));
    }

    fn mac_ack(&mut self, air: &mut Air, seq: u8, t_end: f64, pending: bool) {
        self.st.send_ack(air, seq, t_end, pending);
    }

    pub fn on_frame(&mut self, air: &mut Air, psdu: &[u8], ch: i64, _t0: f64, t1: f64, rssi: i64) {
        if ch == self.st.channel && psdu.len() == 5 && psdu[0] & 7 == 2 {
            if self.plain {
                self.waiting_ack.remove(&psdu[2]);
            } else {
                self.st.mac_ack_seen(psdu[2], psdu[0] & 0x10 != 0);
            }
        }
        if ch != self.st.channel || psdu.len() < 5 {
            return;
        }
        let Some(f) = mac_parse(psdu) else { return };
        let to_us = f.dmode != 0
            && (f.dst == Some(self.short as u64) || f.dst == Some(self.ext))
            && (f.dpan == Some(self.st.pan) || f.dpan == Some(0xFFFF));
        match f.ty {
            3 => self.on_mac_cmd(air, &f, t1, to_us),
            1 => {
                if f.ar && to_us {
                    self.mac_ack(air, f.seq, t1, false);
                }
                if f.dpan == Some(self.st.pan) && (to_us || f.dst == Some(0xFFFF)) {
                    let payload = f.payload.clone();
                    self.on_nwk(air, &payload, &f, rssi);
                }
            }
            _ => {}
        }
    }

    fn on_mac_cmd(&mut self, air: &mut Air, f: &Mac, t_end: f64, to_us: bool) {
        let Some(&cmd) = f.payload.first() else { return };
        if cmd == 0x07 && self.permit_join.is_some() {
            self.ev(air, "beacon-request", String::new());
            self.send_beacon(air);
        } else if cmd == 0x01 && to_us {
            let Some(&cap) = f.payload.get(1) else { return };
            let Some(ext) = f.src else { return };
            self.mac_ack(air, f.seq, t_end, true);
            if self.permit_join != Some(true) {
                return;
            }
            if self.dev_mut(ext).is_none() {
                let short = self.next_short;
                self.next_short = self.next_short.wrapping_add(0x1111) & 0xFFF7;
                self.devices.push(Device {
                    ext,
                    short,
                    cap,
                    fc_in: -1,
                    aps_fc_in: -1,
                    joined: false,
                    announced: false,
                    tclk: None,
                    tclk_new: None,
                    link_status: false,
                    parent: None,
                });
                self.by_short.insert(short, ext);
            }
            let d = self.dev_mut(ext).unwrap();
            d.tclk = None;
            d.tclk_new = None;
            d.link_status = false;
            d.fc_in = -1;
            d.aps_fc_in = -1;
            d.joined = false;
            d.announced = false;
            d.cap = cap;
            self.fc_in_ext.remove(&ext);
            self.pending.insert(ext);
            self.ev(air, "association-request", format!("ext={}, cap={cap:#04x}", fmt_ext(ext)));
        } else if cmd == 0x04 && to_us {
            let ext = if f.smode == 3 { f.src } else { None };
            let pend = ext.is_some_and(|e| self.pending.contains(&e));
            if f.ar {
                self.mac_ack(air, f.seq, t_end, pend);
            }
            if pend {
                let e = ext.unwrap();
                self.pending.remove(&e);
                self.send_assoc_response(air, e);
            }
        }
    }

    fn send_beacon(&mut self, air: &mut Air) {
        let sf: u16 = 0x0FFF | 0x4000 | if self.permit_join == Some(true) { 0x8000 } else { 0 };
        let mut payload = sf.to_le_bytes().to_vec();
        payload.extend_from_slice(&[0, 0, 0x00, 0x22, 0x84]);
        payload.extend_from_slice(&ext_bytes(self.ext_pan));
        payload.extend_from_slice(&[0xff, 0xff, 0xff, 0x00]);
        let seq = nx(&mut self.st.mac_seq);
        let mpdu = mac_build(0, seq, &payload, 0, 0, 0, self.short as u64, 2, Some(self.st.pan), false, false);
        self.send_psdu(air, mpdu, 1.0 + (seq % 4) as f64 * 0.32, None, None);
    }

    fn send_assoc_response(&mut self, air: &mut Air, ext: u64) {
        let short = self.dev_mut(ext).map(|d| d.short).unwrap_or(0xFFFF);
        let mut payload = vec![0x02];
        payload.extend_from_slice(&short.to_le_bytes());
        payload.push(0x00);
        let seq = nx(&mut self.st.mac_seq);
        let mpdu = mac_build(3, seq, &payload, self.st.pan, ext, 3, self.ext, 3, Some(self.st.pan), true, false);
        self.send_psdu(air, mpdu, 0.5, Some(3), None);
        self.ev(air, "association-response", format!("short={short:#06x}"));
        let t = air.now() + 30.0;
        air.at(t, self.cev(CoordEv::TransportKey { ext }));
    }

    fn mac_data(&mut self, air: &mut Air, dst: u16, nwk_frame: Vec<u8>, ar: bool) {
        let ar = ar && dst < 0xFFF8;
        let seq = nx(&mut self.st.mac_seq);
        let mpdu = mac_build(1, seq, &nwk_frame, self.st.pan, dst as u64, 2, self.short as u64, 2, Some(self.st.pan), ar, false);
        self.send_psdu(air, mpdu, 0.4, if ar { Some(3) } else { None }, None);
    }

    #[allow(clippy::too_many_arguments)]
    fn nwk_header(&mut self, ftype: u8, dst: u16, radius: u8, secure: bool, src_ieee: bool, dst_ieee: Option<u64>,
                  discover: u8, relays: Option<&Vec<u16>>) -> Vec<u8> {
        let mut fcf: u16 = ftype as u16 | (2 << 2) | (discover as u16) << 6;
        if secure {
            fcf |= 0x0200;
        }
        if src_ieee {
            fcf |= 0x1000;
        }
        if dst_ieee.is_some() {
            fcf |= 0x0800;
        }
        if relays.is_some_and(|r| !r.is_empty()) {
            fcf |= 0x0400;
        }
        let mut h = fcf.to_le_bytes().to_vec();
        h.extend_from_slice(&dst.to_le_bytes());
        h.extend_from_slice(&self.short.to_le_bytes());
        h.push(radius);
        h.push(nx(&mut self.st.nwk_seq));
        if let Some(d) = dst_ieee {
            h.extend_from_slice(&ext_bytes(d));
        }
        if src_ieee {
            h.extend_from_slice(&ext_bytes(self.ext));
        }
        if let Some(r) = relays.filter(|r| !r.is_empty()) {
            h.push(r.len() as u8);
            h.push(r.len() as u8 - 1);
            for a in r {
                h.extend_from_slice(&a.to_le_bytes());
            }
        }
        h
    }

    fn nwk_secure(&mut self, header: &[u8], payload: &[u8]) -> Vec<u8> {
        let fc = self.st.fc_out;
        self.st.fc_out = self.st.fc_out.wrapping_add(1);
        secure(header, payload, &self.st.nwk_key, self.ext, fc, 1, Some(self.st.key_seq), true)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn nwk_send(&mut self, air: &mut Air, dst: u16, aps: &[u8], ftype: u8, secure: bool, radius: u8,
                    mac_dst: Option<u16>, src_ieee: bool) {
        let relays = if ftype == 0 { self.source_routes.get(&dst).cloned() } else { None };
        let disc = if ftype == 0 { self.nwk_discover } else { 0 };
        let h = self.nwk_header(ftype, dst, radius, secure, src_ieee, None, disc, relays.as_ref());
        let frame = if secure {
            self.nwk_secure(&h, aps)
        } else {
            let mut v = h;
            v.extend_from_slice(aps);
            v
        };
        let mac_dst = mac_dst.unwrap_or_else(|| match relays.as_ref().filter(|r| !r.is_empty()) {
            Some(r) => *r.last().unwrap(),
            None if dst >= 0xFFF8 => 0xFFFF,
            None => *self.st.via.get(&dst).unwrap_or(&dst),
        });
        self.mac_data(air, mac_dst, frame, true);
    }

    pub fn nwk_send_data(&mut self, air: &mut Air, dst: u16, aps: &[u8]) {
        self.nwk_send(air, dst, aps, 0, true, 30, None, true);
    }

    fn on_nwk(&mut self, air: &mut Air, p: &[u8], mac: &Mac, _rssi: i64) {
        if p.len() < 8 {
            return;
        }
        let Some(fcf) = u16_at(p, 0) else { return };
        let (dst, src, seq) = (u16_at(p, 2).unwrap(), u16_at(p, 4).unwrap(), p[7]);
        let ftype = (fcf & 3) as u8;
        let mut i = 8;
        let mut src_ieee = None;
        if fcf & 0x0100 != 0 {
            i += 1;
        }
        if fcf & 0x0800 != 0 {
            i += 8;
        }
        if fcf & 0x1000 != 0 {
            let Some(e) = u64_at(p, i) else { return };
            src_ieee = Some(e);
            i += 8;
        }
        if fcf & 0x0400 != 0 {
            let Some(&cnt) = p.get(i) else { return };
            i += 2 + 2 * cnt as usize;
        }
        let Some(header) = p.get(..i) else { return };
        let header = header.to_vec();
        if dst >= 0xFFF8 {
            let now = air.now();
            if now - self.seen_bcast.get(&(src, seq)).copied().unwrap_or(-1e9) < 9000.0 {
                return;
            }
            self.seen_bcast.insert((src, seq), now);
        }
        if ![self.short, BCAST_ALL, BCAST_RXON, BCAST_ROUTERS].contains(&dst) {
            return;
        }
        let mut payload = p[i..].to_vec();
        if fcf & 0x0200 != 0 {
            let Some(&sc) = payload.first() else { return };
            let Some(fc) = u32_at(&payload, 1) else { return };
            let mut j = 5;
            let mut sext = None;
            if sc & 0x20 != 0 {
                sext = u64_at(&payload, j);
                j += 8;
            }
            if (sc >> 3) & 3 == 1 {
                j += 1;
            }
            if payload.len() < j + 4 {
                return;
            }
            let sc5 = (sc & !7) | 5;
            let sext = sext.unwrap_or_else(|| self.by_short.get(&src).copied().unwrap_or(0));
            let mut aad = header.clone();
            aad.push(sc5);
            aad.extend_from_slice(&payload[1..j]);
            let mut nonce = [0u8; 13];
            nonce[..8].copy_from_slice(&ext_bytes(sext));
            nonce[8..12].copy_from_slice(&fc.to_le_bytes());
            nonce[12] = sc5;
            match ccm_decrypt(&self.st.nwk_key, &nonce, &aad, &payload[j..]) {
                Some(pl) => payload = pl,
                None => {
                    self.ev(air, "nwk-mic-fail", format!("src={src:#06x}"));
                    return;
                }
            }
            if fc as i64 <= self.fc_in_ext.get(&sext).copied().unwrap_or(-1) {
                return;
            }
            self.fc_in_ext.insert(sext, fc as i64);
            if let Some(d) = self.dev_by_short_mut(src) {
                if d.ext == sext {
                    d.fc_in = fc as i64;
                }
            }
        }
        let hop = mac.src.map(|s| s as u16).unwrap_or(src);
        if ftype == 1 {
            self.on_nwk_cmd(air, src, &payload, src_ieee, hop);
        } else if ftype == 0 {
            self.on_aps(air, src, dst, &payload);
        }
    }

    fn on_nwk_cmd(&mut self, air: &mut Air, src: u16, p: &[u8], _src_ieee: Option<u64>, hop: u16) {
        let Some(&cmd) = p.first() else { return };
        if cmd == 0x01 && self.mute_route_reply {
            return;
        }
        match cmd {
            0x08 => {
                if let Some(d) = self.dev_by_short_mut(src) {
                    if !d.link_status {
                        d.link_status = true;
                        let entries = p.get(1).copied().unwrap_or(0) & 0x1F;
                        self.ev(air, "link-status", format!("src={src:#06x}, entries={entries}"));
                    }
                }
            }
            0x04 => {
                let o = p.get(1).copied().unwrap_or(0);
                self.ev(air, "leave", format!("src={src:#06x}, options={o:#04x}"));
            }
            0x01 => {
                let (Some(&rid), Some(target)) = (p.get(2), u16_at(p, 3)) else { return };
                if target == self.short {
                    let mut rep = vec![0x02, 0x00, rid];
                    rep.extend_from_slice(&src.to_le_bytes());
                    rep.extend_from_slice(&self.short.to_le_bytes());
                    rep.push(1);
                    self.nwk_send(air, hop, &rep, 1, true, 30, Some(hop), true);
                }
            }
            0x05 => {
                let n = p.get(1).copied().unwrap_or(0) as usize;
                let relays: Vec<u16> = (0..n).filter_map(|k| u16_at(p, 2 + 2 * k)).collect();
                let txt = relays.iter().map(|r| format!("{r:#06x}")).collect::<Vec<_>>().join(", ");
                self.route_records.push((air.now(), src, relays));
                self.ev(air, "route-record", format!("src={src:#06x}, relays=[{txt}]"));
            }
            _ => {
                let names = [(0x02, "route-reply"), (0x03, "network-status"), (0x06, "rejoin-request"),
                             (0x0B, "end-device-timeout-request")];
                let name = names.iter().find(|(c, _)| *c == cmd).map(|(_, n)| n.to_string()).unwrap_or(format!("{cmd:#04x}"));
                self.ev(air, "nwk-cmd", format!("src={src:#06x}, cmd={name}, data={}", hex(&p[1..])));
            }
        }
    }

    fn on_aps(&mut self, air: &mut Air, src: u16, nwk_dst: u16, p: &[u8]) {
        let mut p = p.to_vec();
        if self.mute_aps_ack && p.first().is_some_and(|f| f & 0x40 != 0) {
            p[0] &= !0x40;
        }
        if !self.plain && p.len() >= 2 && p[0] & 0x43 == 0x41 && nwk_dst == self.short {
            let ack = [0x12, p[1]];
            self.nwk_send_data(air, src, &ack);
        }
        let Some(&fc) = p.first() else { return };
        let ftype = fc & 3;
        let delivery = (fc >> 2) & 3;
        let ack_req = fc & 0x40 != 0;
        let mut i = 1;
        if ftype == 2 {
            return;
        }
        if ftype == 1 {
            let Some(_) = p.get(i) else { return };
            i += 1;
            let mut body = p[i..].to_vec();
            if fc & 0x20 != 0 {
                match self.aps_unsecure(air, &p[..i], &body, src) {
                    Some(b) => body = b,
                    None => return,
                }
            }
            self.on_aps_cmd(air, src, &body);
            return;
        }
        let mut dst_ep = 0u8;
        if delivery == 0 || delivery == 2 {
            let Some(&e) = p.get(i) else { return };
            dst_ep = e;
            i += 1;
        }
        if delivery == 3 {
            i += 2;
        }
        let (Some(cluster), Some(profile)) = (u16_at(&p, i), u16_at(&p, i + 2)) else { return };
        i += 4;
        let (Some(&src_ep), Some(&ctr)) = (p.get(i), p.get(i + 1)) else { return };
        i += 2;
        let payload = p[i..].to_vec();
        if ack_req && nwk_dst == self.short {
            let mut ack = vec![0x02, src_ep];
            ack.extend_from_slice(&cluster.to_le_bytes());
            ack.extend_from_slice(&profile.to_le_bytes());
            ack.push(dst_ep);
            ack.push(ctr);
            self.nwk_send_data(air, src, &ack);
        }
        if profile == 0x0000 {
            self.on_zdo(air, src, cluster, &payload);
        } else {
            self.on_zcl(air, src, src_ep, cluster, &payload);
        }
    }

    fn aps_unsecure(&mut self, air: &Air, header: &[u8], body: &[u8], src: u16) -> Option<Vec<u8>> {
        let sc = *body.first()?;
        let fc = u32_at(body, 1)?;
        let mut j = 5;
        let mut sext = None;
        if sc & 0x20 != 0 {
            sext = Some(u64_at(body, j)?);
            j += 8;
        }
        let keyid = (sc >> 3) & 3;
        let sext = sext.unwrap_or_else(|| self.by_short.get(&src).copied().unwrap_or(0));
        let dev_tclk = self.dev_by_short(src).map(|d| d.tclk.unwrap_or(self.tclk));
        let key = match keyid {
            0 => dev_tclk.unwrap_or(self.tclk),
            1 => self.st.nwk_key,
            2 => self.key_transport,
            _ => hmac_mmo(&self.tclk, &[0x02]),
        };
        if body.len() < j + 4 {
            return None;
        }
        let sc5 = (sc & !7) | 5;
        let mut aad = header.to_vec();
        aad.push(sc5);
        aad.extend_from_slice(&body[1..j]);
        let mut nonce = [0u8; 13];
        nonce[..8].copy_from_slice(&ext_bytes(sext));
        nonce[8..12].copy_from_slice(&fc.to_le_bytes());
        nonce[12] = sc5;
        let r = ccm_decrypt(&key, &nonce, &aad, &body[j..]);
        if r.is_none() {
            let t = air.now();
            air.log(&format!("[{t:9.3} ms] coord: aps-mic-fail src={src:#06x}, keyid={keyid}"));
            self.events.push(CoordEvent { ms: t, kind: "aps-mic-fail".into(), details: format!("src={src:#06x}, keyid={keyid}") });
        }
        r
    }

    pub fn aps_cmd_secured(&mut self, cmd_payload: &[u8], key: &Key, keyid: u8) -> Vec<u8> {
        let owner = self.link_key_owner(key);
        if self.aps_fc_per_key && owner != self.tclk {
            let fc = match self.aps_fc_by_key.iter_mut().find(|(k, _)| *k == owner) {
                Some(e) => {
                    let v = e.1;
                    e.1 += 1;
                    v
                }
                None => {
                    self.aps_fc_by_key.push((owner, 1));
                    0
                }
            };
            let ctr = nx(&mut self.st.aps_ctr);
            return aps_cmd(ctr, cmd_payload, false, Some((*key, keyid, fc, self.ext)), 0);
        }
        let ctr = nx(&mut self.st.aps_ctr);
        let header = [0x21u8, ctr];
        let fc = self.aps_fc_out;
        self.aps_fc_out = self.aps_fc_out.wrapping_add(1);
        let sc_air = (keyid << 3) | 0x20;
        let mut aux_air = vec![sc_air];
        aux_air.extend_from_slice(&fc.to_le_bytes());
        aux_air.extend_from_slice(&ext_bytes(self.ext));
        let mut aad = header.to_vec();
        aad.push(sc_air | 5);
        aad.extend_from_slice(&aux_air[1..]);
        let mut nonce = [0u8; 13];
        nonce[..8].copy_from_slice(&ext_bytes(self.ext));
        nonce[8..12].copy_from_slice(&fc.to_le_bytes());
        nonce[12] = sc_air | 5;
        let mut out = header.to_vec();
        out.extend_from_slice(&aux_air);
        out.extend(ccm_encrypt(key, &nonce, &aad, cmd_payload));
        out
    }

    pub fn link_key_owner(&self, key: &Key) -> Key {
        let mut cands = vec![self.tclk];
        for d in &self.devices {
            cands.extend(d.tclk.iter().chain(d.tclk_new.iter()).copied());
        }
        for lk in cands {
            if *key == lk || *key == hmac_mmo(&lk, &[0x00]) || *key == hmac_mmo(&lk, &[0x02]) {
                return lk;
            }
        }
        *key
    }

    #[allow(clippy::too_many_arguments)]
    pub fn send_network_key(&mut self, air: &mut Air, dev_short: u16, dev_ext: u64, key: &Key, seq: u8,
                            link_key: Option<Key>, broadcast: bool, src_field: Option<u64>) {
        let src_field = src_field.unwrap_or(self.ext);
        let mut body = vec![0x05, 0x01];
        body.extend_from_slice(key);
        body.push(seq);
        if broadcast {
            body.extend_from_slice(&[0; 8]);
            body.extend_from_slice(&ext_bytes(src_field));
            let ctr = nx(&mut self.st.aps_ctr);
            let a = aps_cmd(ctr, &body, false, None, 2);
            self.tx_nwk(air, BCAST_RXON, &a, &Tx::new());
            return;
        }
        body.extend_from_slice(&ext_bytes(dev_ext));
        body.extend_from_slice(&ext_bytes(src_field));
        let kt = hmac_mmo(&link_key.unwrap_or(self.tclk), &[0x00]);
        let a = self.aps_cmd_secured(&body, &kt, 2);
        self.nwk_send_data(air, dev_short, &a);
    }

    pub fn send_switch_key(&mut self, air: &mut Air, seq: u8) {
        let ctr = nx(&mut self.st.aps_ctr);
        let a = aps_cmd(ctr, &[0x09, seq], false, None, 2);
        self.tx_nwk(air, BCAST_RXON, &a, &Tx::new());
    }

    pub fn use_network_key(&mut self, air: &mut Air, key: Key, seq: u8, restart_counter: bool) {
        self.st.nwk_key = key;
        self.st.key_seq = seq;
        air.keys.set_nwk(seq, key);
        if restart_counter {
            self.st.fc_out = 0;
        }
    }

    pub fn permit_joining(&mut self, air: &mut Air, seconds: u8) {
        self.permit_join = Some(seconds != 0);
        let seq = nx(&mut self.st.zdo_seq);
        let mut aps = vec![0x08, 0x00];
        aps.extend_from_slice(&0x0036u16.to_le_bytes());
        aps.extend_from_slice(&0u16.to_le_bytes());
        aps.push(0x00);
        aps.push(nx(&mut self.st.aps_ctr));
        aps.extend_from_slice(&[seq, seconds, 1]);
        self.nwk_send(air, BCAST_ROUTERS, &aps, 0, true, 30, Some(0xFFFF), true);
        self.ev(air, "permit-joining", format!("seconds={seconds}"));
    }

    pub fn many_to_one_route_request(&mut self, air: &mut Air, no_cache: bool) {
        let opts = (if no_cache { 2 } else { 1 }) << 3;
        let mut cmd = vec![0x01, opts, nx(&mut self.st.zdo_seq)];
        cmd.extend_from_slice(&BCAST_ROUTERS.to_le_bytes());
        cmd.push(0);
        self.nwk_send(air, BCAST_ROUTERS, &cmd, 1, true, 30, Some(0xFFFF), true);
    }

    fn send_transport_key(&mut self, air: &mut Air, ext: u64) {
        let Some(short) = self.dev_mut(ext).map(|d| d.short) else { return };
        let mut body = vec![0x05, 0x01];
        body.extend_from_slice(&self.st.nwk_key);
        body.push(0x00);
        body.extend_from_slice(&ext_bytes(ext));
        body.extend_from_slice(&ext_bytes(self.ext));
        let kt = self.key_transport;
        let aps = self.aps_cmd_secured(&body, &kt, 2);
        self.nwk_send(air, short, &aps, 0, false, 1, None, false);
        self.ev(air, "transport-key", format!("short={short:#06x}, key=network"));
    }

    fn on_aps_cmd(&mut self, air: &mut Air, src: u16, b: &[u8]) {
        self.aps_cmds.push((air.now(), src, b.to_vec()));
        let Some(&cmd) = b.first() else { return };
        if self.mute_tclk && (cmd == 0x08 || cmd == 0x0F) {
            return;
        }
        let dev = self.dev_by_short(src).cloned();
        if cmd == 0x08 && b.get(1) == Some(&0x04) && dev.is_some() && self.tclk_key_load {
            let dev = dev.unwrap();
            self.ev(air, "request-key", format!("src={src:#06x}, key_type=4"));
            self.tclk_gen = self.tclk_gen.wrapping_add(1);
            let mut m = ext_bytes(dev.ext).to_vec();
            m.push(self.tclk_gen);
            let newk = hmac_mmo(&self.tclk, &m);
            if let Some(d) = self.dev_mut(dev.ext) {
                d.tclk_new = Some(newk);
            }
            air.keys.add_link(newk);
            let mut body = vec![0x05, 0x04];
            body.extend_from_slice(&newk);
            body.extend_from_slice(&ext_bytes(dev.ext));
            body.extend_from_slice(&ext_bytes(self.ext));
            let kl = hmac_mmo(&dev.tclk.unwrap_or(self.tclk), &[0x02]);
            let aps = self.aps_cmd_secured(&body, &kl, 3);
            self.nwk_send_data(air, src, &aps);
            self.ev(air, "transport-key", format!("short={src:#06x}, key=tc-link"));
            return;
        }
        match cmd {
            0x08 => {
                let ktype = b.get(1).copied().unwrap_or(0);
                self.ev(air, "request-key", format!("src={src:#06x}, key_type={ktype}"));
                if ktype == 0x04 {
                    if let Some(dev) = dev {
                        let newk = hmac_mmo(&self.tclk, &ext_bytes(dev.ext));
                        if let Some(d) = self.dev_mut(dev.ext) {
                            d.tclk_new = Some(newk);
                        }
                        let mut body = vec![0x05, 0x04];
                        body.extend_from_slice(&newk);
                        body.extend_from_slice(&ext_bytes(dev.ext));
                        body.extend_from_slice(&ext_bytes(self.ext));
                        let key = hmac_mmo(&dev.tclk.unwrap_or(self.tclk), &[0x00]);
                        let aps = self.aps_cmd_secured(&body, &key, 2);
                        self.nwk_send_data(air, src, &aps);
                        self.ev(air, "transport-key", format!("short={src:#06x}, key=tc-link"));
                    }
                }
            }
            0x0F => {
                let ktype = b.get(1).copied().unwrap_or(0);
                let sext = u64_at(b, 2).unwrap_or(0);
                let h = b.get(10..26).unwrap_or(&[]);
                let k = dev.as_ref().and_then(|d| d.tclk_new);
                let ok = k.is_some_and(|k| hmac_mmo(&k, &[0x03])[..] == *h);
                self.ev(air, "verify-key", format!("src={src:#06x}, ok={}", if ok { "True" } else { "False" }));
                if ok {
                    if let Some(d) = dev.as_ref().and_then(|d| self.devices.iter_mut().find(|x| x.ext == d.ext)) {
                        d.tclk = k;
                    }
                }
                let mut body = vec![0x10, if ok { 0x00 } else { 0xAD }, ktype];
                body.extend_from_slice(&ext_bytes(sext));
                let key = if ok { k.unwrap() } else { self.tclk };
                let aps = self.aps_cmd_secured(&body, &key, 0);
                self.confirm_keys.push((src, self.st.nwk_seq));
                self.nwk_send_data(air, src, &aps);
            }
            0x06 => {
                let (Some(ext), Some(short), Some(&status)) = (u64_at(b, 1), u16_at(b, 9), b.get(11)) else { return };
                self.ev(air, "update-device", format!("router={src:#06x}, ext={}, short={short:#06x}, status={status}", fmt_ext(ext)));
                if status == 0x01 || status == 0x03 {
                    if let Some(old) = self.devices.iter().find(|d| d.ext == ext).map(|d| d.short) {
                        self.by_short.remove(&old);
                        self.devices.retain(|d| d.ext != ext);
                    }
                    self.devices.push(Device {
                        ext,
                        short,
                        cap: 0,
                        fc_in: -1,
                        aps_fc_in: -1,
                        joined: false,
                        announced: false,
                        tclk: None,
                        tclk_new: None,
                        link_status: false,
                        parent: Some(src),
                    });
                    self.by_short.insert(short, ext);
                    self.fc_in_ext.remove(&ext);
                    self.st.via.insert(short, src);
                    let mut body = vec![0x05, 0x01];
                    body.extend_from_slice(&self.st.nwk_key);
                    body.push(0x00);
                    body.extend_from_slice(&ext_bytes(ext));
                    body.extend_from_slice(&ext_bytes(self.ext));
                    let kt = self.key_transport;
                    let inner = self.aps_cmd_secured(&body, &kt, 2);
                    let mut tunnel = vec![0x01, nx(&mut self.st.aps_ctr), 0x0E];
                    tunnel.extend_from_slice(&ext_bytes(ext));
                    tunnel.extend_from_slice(&inner);
                    self.nwk_send_data(air, src, &tunnel);
                    self.ev(air, "tunnel", format!("router={src:#06x}, ext={}", fmt_ext(ext)));
                }
            }
            _ => {
                self.ev(air, "aps-cmd", format!("src={src:#06x}, cmd={cmd:#04x}, data={}", hex(&b[1..])));
            }
        }
    }

    fn zdo_rsp_aps(&mut self, cluster: u16) -> Vec<u8> {
        let mut aps = vec![0x00, 0x00];
        aps.extend_from_slice(&cluster.to_le_bytes());
        aps.extend_from_slice(&0u16.to_le_bytes());
        aps.push(0x00);
        aps.push(nx(&mut self.st.aps_ctr));
        aps
    }

    fn on_zdo(&mut self, air: &mut Air, src: u16, cluster: u16, p: &[u8]) {
        let seq = p.first().copied().unwrap_or(0);
        match cluster {
            0x0013 => {
                let (Some(short), Some(ext), Some(&cap)) = (u16_at(p, 1), u64_at(p, 3), p.get(11)) else { return };
                if let Some(d) = self.dev_mut(ext) {
                    d.announced = true;
                    d.joined = true;
                    d.cap = cap;
                }
                self.ev(air, "device-announce", format!("short={short:#06x}, ext={}, cap={cap:#04x}", fmt_ext(ext)));
            }
            0x0002 => {
                let server_mask: u16 = 0x0001 | (self.stack_revision << 9);
                let mut nd = vec![0x00, 0x40, 0x8F];
                nd.extend_from_slice(&0x1141u16.to_le_bytes());
                nd.push(0x50);
                nd.extend_from_slice(&0x00A0u16.to_le_bytes());
                nd.extend_from_slice(&server_mask.to_le_bytes());
                nd.extend_from_slice(&0x00A0u16.to_le_bytes());
                nd.push(0x00);
                self.ev(air, "node-desc-request", format!("src={src:#06x}"));
                let mut aps = self.zdo_rsp_aps(0x8002);
                aps.extend_from_slice(&[seq, 0x00, 0x00, 0x00]);
                aps.extend_from_slice(&nd);
                self.nwk_send_data(air, src, &aps);
            }
            0x0000 | 0x0001 => {
                let mine = if cluster == 0x0001 { u16_at(p, 1) == Some(0x0000) } else { u64_at(p, 1) == Some(self.ext) };
                self.ev(air, "zdo-addr-request", format!("src={src:#06x}, cluster={cluster:#06x}, ours={}", if mine { "True" } else { "False" }));
                let mut aps = self.zdo_rsp_aps(cluster | 0x8000);
                aps.push(seq);
                aps.push(if mine { 0x00 } else { 0x81 });
                aps.extend_from_slice(&ext_bytes(self.ext));
                aps.extend_from_slice(&0u16.to_le_bytes());
                self.nwk_send_data(air, src, &aps);
            }
            0x0006 => {
                let Some(&n_in) = p.get(5) else { return };
                let ins: Vec<u16> = (0..n_in as usize).filter_map(|k| u16_at(p, 6 + 2 * k)).collect();
                let n_out = p.get(6 + 2 * n_in as usize).copied().unwrap_or(0);
                let outs: Vec<u16> = (0..n_out as usize).filter_map(|k| u16_at(p, 7 + 2 * n_in as usize + 2 * k)).collect();
                let m = ins.contains(&0x0019) || outs.contains(&0x0019);
                let prof = u16_at(p, 3).unwrap_or(0);
                let l = |v: &[u16]| format!("[{}]", v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(", "));
                self.ev(air, "zdo-match-desc", format!("src={src:#06x}, profile={prof:#06x}, ins={}, outs={}, match={}",
                                                       l(&ins), l(&outs), if m { "True" } else { "False" }));
                let mut aps = self.zdo_rsp_aps(0x8006);
                aps.extend_from_slice(&[seq, 0x00, 0x00, 0x00, if m { 1 } else { 0 }]);
                if m {
                    aps.push(0x01);
                }
                self.nwk_send_data(air, src, &aps);
            }
            c if c & 0x8000 != 0 => {
                self.ev(air, "zdo-rsp", format!("src={src:#06x}, cluster={c:#06x}, data={}", hex(p)));
                self.zcl_rsp.push(ZclRsp { ms: air.now(), src, ep: 0, cluster: c, cmd: None, body: p.to_vec() });
            }
            c => self.ev(air, "zdo-req", format!("src={src:#06x}, cluster={c:#06x}, data={}", hex(p))),
        }
    }

    fn on_zcl(&mut self, air: &mut Air, src: u16, ep: u8, cluster: u16, p: &[u8]) {
        if p.len() < 3 {
            return;
        }
        let (fc, mut seq, mut cmd) = (p[0], p[1], p[2]);
        let body = if fc & 0x04 != 0 {
            let (Some(&s), Some(&c)) = (p.get(3), p.get(4)) else { return };
            seq = s;
            cmd = c;
            p.get(5..).unwrap_or(&[]).to_vec()
        } else {
            p[3..].to_vec()
        };
        self.zcl_rsp.push(ZclRsp { ms: air.now(), src, ep, cluster, cmd: Some(cmd), body: body.clone() });
        if cluster == 0x0019 && fc & 1 != 0 && fc & 0x08 == 0 {
            self.on_ota(air, src, ep, seq, cmd, &body);
            return;
        }
        let kind = if fc & 1 != 0 {
            format!("cluster-cmd{cmd:#04x}")
        } else {
            match cmd {
                0x01 => "read-attr-rsp".into(),
                0x07 => "configure-reporting-rsp".into(),
                0x0A => "report-attr".into(),
                0x0B => "default-rsp".into(),
                c => format!("cmd{c:#04x}"),
            }
        };
        self.ev(air, "zcl", format!("src={src:#06x}, ep={ep}, cluster={cluster:#06x}, kind={kind}, data={}", hex(&body)));
    }

    #[allow(clippy::too_many_arguments)]
    pub fn zcl_send_with(&mut self, air: &mut Air, dst: u16, ep: u8, cluster: u16, cmd: u8, payload: &[u8], profile: u16,
                         specific: bool, ddr: bool, seq: Option<u8>, server_to_client: bool) -> u8 {
        let seq = seq.unwrap_or_else(|| nx(&mut self.st.zcl_seq));
        let fc = if specific { 0x01 } else { 0 } | if ddr { 0x10 } else { 0 } | if server_to_client { 0x08 } else { 0 };
        let mut aps = vec![0x40, ep];
        aps.extend_from_slice(&cluster.to_le_bytes());
        aps.extend_from_slice(&profile.to_le_bytes());
        aps.push(0x01);
        aps.push(nx(&mut self.st.aps_ctr));
        aps.extend_from_slice(&[fc, seq, cmd]);
        aps.extend_from_slice(payload);
        self.nwk_send_data(air, dst, &aps);
        seq
    }

    pub fn zcl_cmd(&mut self, air: &mut Air, dst: u16, ep: u8, cluster: u16, cmd: u8, payload: &[u8]) -> u8 {
        self.zcl_send_with(air, dst, ep, cluster, cmd, payload, HA, true, false, None, false)
    }

    pub fn read_attrs_mfr(&mut self, air: &mut Air, dst: u16, ep: u8, cluster: u16, code: u16, attrs: &[u16]) -> u8 {
        let seq = nx(&mut self.st.zcl_seq);
        let mut aps = vec![0x40, ep];
        aps.extend_from_slice(&cluster.to_le_bytes());
        aps.extend_from_slice(&HA.to_le_bytes());
        aps.push(0x01);
        aps.push(nx(&mut self.st.aps_ctr));
        aps.push(0x04);
        aps.extend_from_slice(&code.to_le_bytes());
        aps.extend_from_slice(&[seq, 0x00]);
        aps.extend(attrs.iter().flat_map(|a| a.to_le_bytes()));
        self.nwk_send_data(air, dst, &aps);
        seq
    }

    pub fn read_attrs(&mut self, air: &mut Air, dst: u16, ep: u8, cluster: u16, attrs: &[u16]) -> u8 {
        let pl: Vec<u8> = attrs.iter().flat_map(|a| a.to_le_bytes()).collect();
        self.zcl_send_with(air, dst, ep, cluster, 0x00, &pl, HA, false, false, None, false)
    }

    pub fn zdo_send(&mut self, air: &mut Air, dst: u16, cluster: u16, payload: &[u8]) -> u8 {
        let seq = nx(&mut self.st.zdo_seq);
        let mut aps = vec![0x40, 0x00];
        aps.extend_from_slice(&cluster.to_le_bytes());
        aps.extend_from_slice(&0u16.to_le_bytes());
        aps.push(0x00);
        aps.push(nx(&mut self.st.aps_ctr));
        aps.push(seq);
        aps.extend_from_slice(payload);
        self.nwk_send_data(air, dst, &aps);
        seq
    }

    pub fn add_ota_file(&mut self, data: Vec<u8>) -> Result<(u16, u16, u32), String> {
        if u32_at(&data, 0) != Some(0x0BEEF11E) {
            return Err("this is not a Zigbee OTA file".into());
        }
        let (mfg, itype, ver) = (u16_at(&data, 10).unwrap(), u16_at(&data, 12).unwrap(), u32_at(&data, 14).unwrap());
        self.ota_images.retain(|(k, _)| *k != (mfg, itype));
        self.ota_images.push(((mfg, itype), (ver, data)));
        Ok((mfg, itype, ver))
    }

    pub fn image_notify(&mut self, air: &mut Air, dst: u16, ep: u8) {
        self.zcl_send_with(air, dst, ep, 0x0019, 0x00, &[0x00, 100], HA, true, true, None, true);
        self.ev(air, "ota-image-notify", format!("dst={dst:#06x}"));
    }

    fn ota_ev(&mut self, air: &Air, what: &str, d: String) {
        self.ota_log.push((air.now(), what.to_string(), d.clone()));
        if what != "ota-block" {
            self.ev(air, what, d);
        }
    }

    fn on_ota(&mut self, air: &mut Air, src: u16, ep: u8, seq: u8, cmd: u8, b: &[u8]) {
        let img = |s: &Coord, mfg: u16, itype: u16| s.ota_images.iter().find(|(k, _)| *k == (mfg, itype)).map(|(_, v)| v.clone());
        match cmd {
            0x01 => {
                let (mfg, itype, cur) = (u16_at(b, 1).unwrap_or(0), u16_at(b, 3).unwrap_or(0), u32_at(b, 5).unwrap_or(0));
                let im = img(self, mfg, itype);
                let offer = im.as_ref().map(|(v, _)| format!("{v:#010x}")).unwrap_or_else(|| "None".into());
                self.ota_ev(air, "ota-query", format!("src={src:#06x}, mfg={mfg:#06x}, type={itype:#06x}, current={cur:#010x}, offer={offer}"));
                match im {
                    None => {
                        self.zcl_send_with(air, src, ep, 0x0019, 0x02, &[0x98], HA, true, true, Some(seq), true);
                    }
                    Some((ver, data)) => {
                        let mut pl = vec![0x00];
                        pl.extend_from_slice(&mfg.to_le_bytes());
                        pl.extend_from_slice(&itype.to_le_bytes());
                        pl.extend_from_slice(&ver.to_le_bytes());
                        pl.extend_from_slice(&(data.len() as u32).to_le_bytes());
                        self.zcl_send_with(air, src, ep, 0x0019, 0x02, &pl, HA, true, true, Some(seq), true);
                    }
                }
            }
            0x03 => {
                let (mfg, itype) = (u16_at(b, 1).unwrap_or(0), u16_at(b, 3).unwrap_or(0));
                let (ver, off, maxlen) = (u32_at(b, 5).unwrap_or(0), u32_at(b, 9).unwrap_or(0) as usize, b.get(13).copied().unwrap_or(0) as usize);
                let Some((iv, data)) = img(self, mfg, itype).filter(|(v, _)| *v == ver) else {
                    self.zcl_send_with(air, src, ep, 0x0019, 0x05, &[0x96], HA, true, true, Some(seq), true);
                    return;
                };
                let chunk = data.get(off..(off + maxlen).min(data.len())).unwrap_or(&[]).to_vec();
                self.ota_ev(air, "ota-block", format!("offset={off}, size={}", chunk.len()));
                if off == 0 || off / 32768 != off.wrapping_sub(chunk.len()) / 32768 {
                    let pct = format!("{:.0}%", 100.0 * off as f64 / data.len() as f64);
                    self.ev(air, "ota-progress", format!("offset={off}, total={}, pct={pct}", data.len()));
                }
                let mut pl = vec![0x00];
                pl.extend_from_slice(&mfg.to_le_bytes());
                pl.extend_from_slice(&itype.to_le_bytes());
                pl.extend_from_slice(&iv.to_le_bytes());
                pl.extend_from_slice(&(off as u32).to_le_bytes());
                pl.push(chunk.len() as u8);
                pl.extend_from_slice(&chunk);
                self.zcl_send_with(air, src, ep, 0x0019, 0x05, &pl, HA, true, true, Some(seq), true);
            }
            0x06 => {
                let status = b.first().copied().unwrap_or(0xFF);
                let ver = u32_at(b, 5).unwrap_or(0);
                self.ota_ev(air, "ota-upgrade-end", format!("src={src:#06x}, status={status:#04x}, version={ver:#010x}"));
                if status == 0x00 && b.len() >= 9 {
                    let mut pl = b[1..9].to_vec();
                    pl.extend_from_slice(&[0; 8]);
                    self.zcl_send_with(air, src, ep, 0x0019, 0x07, &pl, HA, true, true, Some(seq), true);
                }
            }
            c => self.ota_ev(air, "ota-cmd", format!("src={src:#06x}, cmd={c:#04x}, data={}", hex(b))),
        }
    }

    fn link_status_tick(&mut self, air: &mut Air) {
        let mut routers: Vec<u16> = self.devices.iter().filter(|d| d.joined && d.cap & 0x02 != 0).map(|d| d.short).collect();
        routers.sort();
        let mut body = vec![0x08, (routers.len() as u8 & 0x1F) | 0x60];
        for s in &routers {
            body.extend_from_slice(&s.to_le_bytes());
            body.push(0x11);
        }
        self.nwk_send(air, BCAST_ROUTERS, &body, 1, true, 1, Some(0xFFFF), true);
        let t = air.now() + self.ls_period_ms;
        air.at(t, self.cev(CoordEv::LinkStatusTick));
    }

    pub fn tx_nwk(&mut self, air: &mut Air, dst: u16, payload: &[u8], o: &Tx) -> Sent {
        self.st.tx_nwk(air, crate::frames::NWK_DATA, dst, payload, o)
    }
    pub fn tx_nwk_cmd(&mut self, air: &mut Air, dst: u16, payload: &[u8], o: &Tx) -> Sent {
        self.st.tx_nwk_cmd(air, dst, payload, o)
    }
    pub fn tx_aps_data(&mut self, air: &mut Air, dst: u16, cluster: u16, payload: &[u8], o: &Tx) -> Sent {
        self.st.tx_aps_data(air, dst, cluster, payload, o)
    }
    pub fn tx_zcl(&mut self, air: &mut Air, dst: u16, cluster: u16, cmd: u8, payload: &[u8], o: &Tx) -> Sent {
        self.st.tx_zcl(air, dst, cluster, cmd, payload, o)
    }
    pub fn tx_zdo(&mut self, air: &mut Air, dst: u16, cluster: u16, payload: &[u8], o: &Tx) -> Sent {
        self.st.tx_zdo(air, dst, cluster, payload, o)
    }
}

impl Coord {
    fn cev(&self, e: CoordEv) -> Ev {
        match self.st.id {
            NodeId::Coord2(i) => Ev::Coord2(i, e),
            _ => Ev::Coord(e),
        }
    }

    pub fn set_network(&mut self, channel: i64, pan: u16, ext: u64, ext_pan: u64, nwk_key: Key, permit_join: Option<bool>,
                       short: u16) {
        self.st.channel = channel;
        self.st.pan = pan;
        self.st.ext = ext;
        self.ext = ext;
        self.ext_pan = ext_pan;
        self.st.nwk_key = nwk_key;
        self.permit_join = permit_join;
        self.st.short = Some(short);
        self.short = short;
    }
}
