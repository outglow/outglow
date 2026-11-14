//! Zigbee frame codec for the scenarios, a port of emulator/spec/frames.py
//! (and the MAC helpers of emulator/zigbee_coord.py and emulator/radio154.py),
//! written from the specification text:
//!
//! - NWK header (R22 3.3.1): frame control, destination, source, radius,
//!   sequence number, then destination IEEE, source IEEE, multicast control and
//!   the source route subframe, in that order.
//! - Auxiliary security header (4.5.1): security control, frame counter,
//!   source address with the extended nonce bit, key sequence number with key
//!   identifier 1. The level bits are 0 on the air and 5 for the computation;
//!   nonce = source address || frame counter || security control.
//! - APS header (2.2.5.1): frame control, destination endpoint, group, cluster,
//!   profile, source endpoint, counter, extended header.
//!
//! Every parser returns None where the Python one would raise (truncated
//! input), so a malformed frame is simply not decoded.

use crate::crypto::{ccm_decrypt, ccm_encrypt, hmac_mmo};

pub const NWK_DATA: u8 = 0;
pub const NWK_CMD: u8 = 1;
pub const APS_DATA: u8 = 0;
pub const APS_CMD: u8 = 1;
pub const APS_ACK: u8 = 2;
pub const BCAST_ALL: u16 = 0xFFFF;
pub const BCAST_RXON: u16 = 0xFFFD;
pub const BCAST_ROUTERS: u16 = 0xFFFC;

pub type Key = [u8; 16];

// ------------------------------------------------------------------ PHY / MAC
/// CRC-16/KERMIT, the 802.15.4 FCS.
pub fn crc16_kermit(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for &b in data {
        crc ^= b as u16;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0x8408 } else { crc >> 1 };
        }
    }
    crc
}

/// MPDU || FCS.
pub fn with_fcs(mpdu: &[u8]) -> Vec<u8> {
    let mut v = mpdu.to_vec();
    v.extend_from_slice(&crc16_kermit(mpdu).to_le_bytes());
    v
}

/// Time on the air of a PSDU (preamble, SFD and PHR included) at 250 kb/s, in µs.
pub fn airtime_us(psdu_len: usize) -> f64 {
    ((6 + psdu_len) * 32) as f64
}

pub fn ext_bytes(x: u64) -> [u8; 8] {
    x.to_le_bytes()
}

/// An extended address as on a label: big endian, colon separated.
pub fn fmt_ext(x: u64) -> String {
    x.to_be_bytes().iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(":")
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

#[derive(Clone, Debug, Default)]
pub struct Mac {
    pub ty: u8,
    pub sec: bool,
    pub pending: bool,
    pub ar: bool,
    pub panc: bool,
    pub dmode: u8,
    pub smode: u8,
    pub seq: u8,
    pub dpan: Option<u16>,
    pub dst: Option<u64>,
    pub span: Option<u16>,
    pub src: Option<u64>,
    pub payload: Vec<u8>,
}

/// MAC header of a PSDU (with FCS).
pub fn mac_parse(psdu: &[u8]) -> Option<Mac> {
    if psdu.len() < 2 {
        return None;
    }
    let p = &psdu[..psdu.len() - 2];
    let fcf = u16_at(p, 0)?;
    let seq = *p.get(2)?;
    let mut f = Mac {
        ty: (fcf & 7) as u8,
        sec: fcf & 8 != 0,
        pending: fcf & 0x10 != 0,
        ar: fcf & 0x20 != 0,
        panc: fcf & 0x40 != 0,
        dmode: ((fcf >> 10) & 3) as u8,
        smode: ((fcf >> 14) & 3) as u8,
        seq,
        ..Default::default()
    };
    let mut i = 3;
    if f.dmode != 0 {
        f.dpan = Some(u16_at(p, i)?);
        i += 2;
        if f.dmode == 2 {
            f.dst = Some(u16_at(p, i)? as u64);
            i += 2;
        } else {
            f.dst = Some(u64_at(p, i)?);
            i += 8;
        }
    }
    if f.smode != 0 {
        if !f.panc {
            f.span = Some(u16_at(p, i)?);
            i += 2;
        } else {
            f.span = f.dpan;
        }
        if f.smode == 2 {
            f.src = Some(u16_at(p, i)? as u64);
            i += 2;
        } else {
            f.src = Some(u64_at(p, i)?);
            i += 8;
        }
    }
    f.payload = p.get(i..).map(|s| s.to_vec()).unwrap_or_default();
    Some(f)
}

/// A MAC frame without FCS. `dst`/`src` are short addresses for mode 2 and
/// extended ones for mode 3.
#[allow(clippy::too_many_arguments)]
pub fn mac_build(ftype: u8, seq: u8, payload: &[u8], dpan: u16, dst: u64, dmode: u8, src: u64, smode: u8,
                 span: Option<u16>, ar: bool, pending: bool) -> Vec<u8> {
    let panc = dmode != 0 && smode != 0 && span.map_or(true, |s| s == dpan);
    let mut fcf: u16 = ftype as u16 | if pending { 0x10 } else { 0 } | if ar { 0x20 } else { 0 } | if panc { 0x40 } else { 0 };
    fcf |= (dmode as u16) << 10 | (smode as u16) << 14;
    let mut b = fcf.to_le_bytes().to_vec();
    b.push(seq);
    if dmode != 0 {
        b.extend_from_slice(&dpan.to_le_bytes());
        if dmode == 2 {
            b.extend_from_slice(&(dst as u16).to_le_bytes());
        } else {
            b.extend_from_slice(&dst.to_le_bytes());
        }
    }
    if smode != 0 {
        if !panc {
            b.extend_from_slice(&span.unwrap_or(0).to_le_bytes());
        }
        if smode == 2 {
            b.extend_from_slice(&(src as u16).to_le_bytes());
        } else {
            b.extend_from_slice(&src.to_le_bytes());
        }
    }
    b.extend_from_slice(payload);
    b
}

// ------------------------------------------------------------------ key material
/// Key material the sniffer may use, kept in insertion order as in Python
/// (the order decides which key or address is tried first).
#[derive(Clone, Debug, Default)]
pub struct Keys {
    /// key sequence number -> network key
    pub nwk: Vec<(u8, Key)>,
    /// link keys (the default TC link key, unique keys handed out, ...)
    pub link: Vec<Key>,
    /// short address -> extended address
    pub ext: Vec<(u16, u64)>,
}

impl Keys {
    pub fn set_nwk(&mut self, seq: u8, key: Key) {
        match self.nwk.iter_mut().find(|(s, _)| *s == seq) {
            Some(e) => e.1 = key,
            None => self.nwk.push((seq, key)),
        }
    }
    pub fn nwk_key(&self, seq: u8) -> Option<Key> {
        self.nwk.iter().find(|(s, _)| *s == seq).map(|(_, k)| *k)
    }
    pub fn add_link(&mut self, key: Key) {
        if !self.link.contains(&key) {
            self.link.push(key);
        }
    }
    pub fn learn(&mut self, short: Option<u16>, ext: Option<u64>) {
        if let (Some(s), Some(e)) = (short, ext) {
            if s < 0xFFF8 {
                match self.ext.iter_mut().find(|(k, _)| *k == s) {
                    Some(x) => x.1 = e,
                    None => self.ext.push((s, e)),
                }
            }
        }
    }
    pub fn ext_of(&self, short: u16) -> Option<u64> {
        self.ext.iter().find(|(k, _)| *k == short).map(|(_, e)| *e)
    }
}

// ------------------------------------------------------------------ security
#[derive(Clone, Debug)]
pub struct Aux {
    pub sc: u8,
    pub level: u8,
    pub key_id: u8,
    pub ext_nonce: bool,
    pub fc: u32,
    pub src: Option<u64>,
    pub key_seq: Option<u8>,
    pub len: usize,
}

fn aux_parse(b: &[u8]) -> Option<Aux> {
    let sc = *b.first()?;
    let fc = u32_at(b, 1)?;
    let mut j = 5;
    let mut src = None;
    let mut key_seq = None;
    if sc & 0x20 != 0 {
        src = Some(u64_at(b, j)?);
        j += 8;
    }
    if (sc >> 3) & 3 == 1 {
        key_seq = Some(*b.get(j)?);
        j += 1;
    }
    Some(Aux { sc, level: sc & 7, key_id: (sc >> 3) & 3, ext_nonce: sc & 0x20 != 0, fc, src, key_seq, len: j })
}

fn nonce(src_ext: u64, fc: u32, sc5: u8) -> [u8; 13] {
    let mut n = [0u8; 13];
    n[..8].copy_from_slice(&ext_bytes(src_ext));
    n[8..12].copy_from_slice(&fc.to_le_bytes());
    n[12] = sc5;
    n
}

fn unsecure(header: &[u8], body: &[u8], key: &Key, src_ext: u64, aux: &Aux) -> Option<Vec<u8>> {
    if body.len() < aux.len + 4 {
        return None;
    }
    let sc5 = (aux.sc & !7) | 5;
    let mut aad = header.to_vec();
    aad.push(sc5);
    aad.extend_from_slice(&body[1..aux.len]);
    ccm_decrypt(key, &nonce(src_ext, aux.fc, sc5), &aad, &body[aux.len..])
}

/// header || auxiliary header || ciphertext || MIC-32 (security level 5).
pub fn secure(header: &[u8], payload: &[u8], key: &Key, src_ext: u64, fc: u32, key_id: u8, key_seq: Option<u8>,
              ext_nonce: bool) -> Vec<u8> {
    let sc_air = (key_id << 3) | if ext_nonce { 0x20 } else { 0 };
    let mut aux = vec![sc_air];
    aux.extend_from_slice(&fc.to_le_bytes());
    if ext_nonce {
        aux.extend_from_slice(&ext_bytes(src_ext));
    }
    if key_id == 1 {
        aux.push(key_seq.unwrap_or(0));
    }
    let mut aad = header.to_vec();
    aad.push(sc_air | 5);
    aad.extend_from_slice(&aux[1..]);
    let mut out = header.to_vec();
    out.extend_from_slice(&aux);
    out.extend(ccm_encrypt(key, &nonce(src_ext, fc, sc_air | 5), &aad, payload));
    out
}

// ------------------------------------------------------------------ NWK
#[derive(Clone, Debug, Default)]
pub struct Nwk {
    pub fcf: u16,
    pub ty: u8,
    pub ver: u8,
    pub discover: u8,
    pub multicast: bool,
    pub secured: bool,
    pub source_route: bool,
    pub ed_initiator: bool,
    pub dst: u16,
    pub src: u16,
    pub radius: u8,
    pub seq: u8,
    pub dst_ieee: Option<u64>,
    pub src_ieee: Option<u64>,
    pub multicast_control: Option<u8>,
    pub relay_index: Option<u8>,
    pub relays: Option<Vec<u16>>,
    pub header: Vec<u8>,
    pub aux: Option<Aux>,
    pub fc: Option<u32>,
    pub key_seq: Option<u8>,
    pub sec_src: Option<u64>,
    /// None: secured and no known key opened it
    pub payload: Option<Vec<u8>>,
    pub key: Option<Key>,
    pub cmd: Option<u8>,
}

pub fn nwk_parse(p: &[u8], keys: &mut Keys) -> Option<Nwk> {
    if p.len() < 8 {
        return None;
    }
    let fcf = u16_at(p, 0)?;
    let mut n = Nwk {
        fcf,
        ty: (fcf & 3) as u8,
        ver: ((fcf >> 2) & 0xF) as u8,
        discover: ((fcf >> 6) & 3) as u8,
        multicast: fcf & 0x0100 != 0,
        secured: fcf & 0x0200 != 0,
        source_route: fcf & 0x0400 != 0,
        ed_initiator: fcf & 0x2000 != 0,
        dst: u16_at(p, 2)?,
        src: u16_at(p, 4)?,
        radius: p[6],
        seq: p[7],
        ..Default::default()
    };
    let mut i = 8;
    if fcf & 0x0800 != 0 {
        n.dst_ieee = Some(u64_at(p, i)?);
        i += 8;
    }
    if fcf & 0x1000 != 0 {
        let e = u64_at(p, i)?;
        n.src_ieee = Some(e);
        i += 8;
        keys.learn(Some(n.src), Some(e));
    }
    if fcf & 0x0100 != 0 {
        n.multicast_control = Some(*p.get(i)?);
        i += 1;
    }
    if fcf & 0x0400 != 0 {
        let cnt = *p.get(i)? as usize;
        n.relay_index = Some(*p.get(i + 1)?);
        let mut r = Vec::with_capacity(cnt);
        for k in 0..cnt {
            r.push(u16_at(p, i + 2 + 2 * k)?);
        }
        n.relays = Some(r);
        i += 2 + 2 * cnt;
    }
    n.header = p.get(..i)?.to_vec();
    let body = p.get(i..)?;
    if n.secured {
        let aux = aux_parse(body)?;
        n.fc = Some(aux.fc);
        n.key_seq = aux.key_seq;
        n.sec_src = aux.src;
        let mut cands: Vec<Key> = Vec::new();
        if let Some(k) = aux.key_seq.and_then(|s| keys.nwk_key(s)) {
            cands.push(k);
        }
        for (_, k) in &keys.nwk {
            if !cands.contains(k) {
                cands.push(*k);
            }
        }
        let srcs: Vec<Option<u64>> = if aux.src.is_some() { vec![aux.src] } else { vec![keys.ext_of(n.src)] };
        'outer: for k in &cands {
            for sx in srcs.iter().flatten() {
                if let Some(pl) = unsecure(&n.header, body, k, *sx, &aux) {
                    n.payload = Some(pl);
                    n.key = Some(*k);
                    break 'outer;
                }
            }
        }
        n.aux = Some(aux);
    } else {
        n.payload = Some(body.to_vec());
    }
    if n.ty == NWK_CMD {
        n.cmd = n.payload.as_ref().and_then(|p| p.first().copied());
    }
    Some(n)
}

/// Options of an NWK frame (defaults as in frames.py nwk_build).
#[derive(Clone, Debug)]
pub struct NwkOpts {
    pub radius: u8,
    pub discover: u8,
    pub src_ieee: Option<u64>,
    pub dst_ieee: Option<u64>,
    pub relays: Option<Vec<u16>>,
    pub relay_index: Option<u8>,
    pub multicast: Option<u8>,
    pub ed_initiator: bool,
    pub version: u8,
    /// (key, key sequence number, frame counter, source extended address)
    pub sec: Option<(Key, u8, u32, u64)>,
}

impl Default for NwkOpts {
    fn default() -> Self {
        NwkOpts { radius: 30, discover: 0, src_ieee: None, dst_ieee: None, relays: None, relay_index: None,
                  multicast: None, ed_initiator: false, version: 2, sec: None }
    }
}

pub fn nwk_build(ftype: u8, dst: u16, src: u16, seq: u8, payload: &[u8], o: &NwkOpts) -> Vec<u8> {
    let mut fcf: u16 = ftype as u16 | (o.version as u16) << 2 | (o.discover as u16) << 6;
    if o.multicast.is_some() {
        fcf |= 0x0100;
    }
    if o.sec.is_some() {
        fcf |= 0x0200;
    }
    if o.relays.is_some() {
        fcf |= 0x0400;
    }
    if o.dst_ieee.is_some() {
        fcf |= 0x0800;
    }
    if o.src_ieee.is_some() {
        fcf |= 0x1000;
    }
    if o.ed_initiator {
        fcf |= 0x2000;
    }
    let mut h = fcf.to_le_bytes().to_vec();
    h.extend_from_slice(&dst.to_le_bytes());
    h.extend_from_slice(&src.to_le_bytes());
    h.push(o.radius);
    h.push(seq);
    if let Some(d) = o.dst_ieee {
        h.extend_from_slice(&ext_bytes(d));
    }
    if let Some(s) = o.src_ieee {
        h.extend_from_slice(&ext_bytes(s));
    }
    if let Some(m) = o.multicast {
        h.push(m);
    }
    if let Some(r) = &o.relays {
        let ri = o.relay_index.unwrap_or((r.len() as u8).wrapping_sub(1));
        h.push(r.len() as u8);
        h.push(ri);
        for a in r {
            h.extend_from_slice(&a.to_le_bytes());
        }
    }
    match &o.sec {
        Some((key, key_seq, fc, src_ext)) => secure(&h, payload, key, *src_ext, *fc, 1, Some(*key_seq), true),
        None => {
            h.extend_from_slice(payload);
            h
        }
    }
}

// ------------------------------------------------------------------ APS
#[derive(Clone, Debug, Default)]
pub struct Aps {
    pub fc: u8,
    pub ty: u8,
    pub delivery: u8,
    pub ack_format: bool,
    pub secured: bool,
    pub ack_req: bool,
    pub ext_hdr: bool,
    pub counter: Option<u8>,
    pub dst_ep: Option<u8>,
    pub group: Option<u16>,
    pub cluster: Option<u16>,
    pub profile: Option<u16>,
    pub src_ep: Option<u8>,
    pub ext_fc: Option<u8>,
    pub frag: Option<u8>,
    pub block: Option<u8>,
    pub ack_bits: Option<u8>,
    pub header: Vec<u8>,
    pub aux: Option<Aux>,
    pub key_id: Option<u8>,
    pub payload: Option<Vec<u8>>,
    pub key: Option<Key>,
    pub sec_src: Option<u64>,
    pub link_key: Option<Key>,
    pub cmd: Option<u8>,
}

pub fn aps_parse(p: &[u8], keys: &Keys, nwk_src: Option<u16>) -> Option<Aps> {
    let fc = *p.first()?;
    let mut a = Aps {
        fc,
        ty: fc & 3,
        delivery: (fc >> 2) & 3,
        ack_format: fc & 0x10 != 0,
        secured: fc & 0x20 != 0,
        ack_req: fc & 0x40 != 0,
        ext_hdr: fc & 0x80 != 0,
        ..Default::default()
    };
    let mut i = 1;
    if a.ty == APS_CMD {
        a.counter = Some(*p.get(i)?);
        i += 1;
    } else if a.ty == APS_DATA || a.ty == APS_ACK {
        if !(a.ty == APS_ACK && a.ack_format) {
            if a.delivery == 0 || a.delivery == 2 {
                a.dst_ep = Some(*p.get(i)?);
                i += 1;
            }
            if a.delivery == 3 {
                a.group = Some(u16_at(p, i)?);
                i += 2;
            }
            a.cluster = Some(u16_at(p, i)?);
            a.profile = Some(u16_at(p, i + 2)?);
            i += 4;
            a.src_ep = Some(*p.get(i)?);
            i += 1;
        }
        a.counter = Some(*p.get(i)?);
        i += 1;
        if a.ext_hdr {
            let e = *p.get(i)?;
            a.ext_fc = Some(e);
            a.frag = Some(e & 3);
            i += 1;
            if e & 3 != 0 {
                a.block = Some(*p.get(i)?);
                i += 1;
                if a.ty == APS_ACK {
                    a.ack_bits = Some(*p.get(i)?);
                    i += 1;
                }
            }
        }
    } else {
        return Some(a);
    }
    a.header = p.get(..i)?.to_vec();
    let body = p.get(i..)?;
    if a.secured {
        let aux = aux_parse(body)?;
        a.key_id = Some(aux.key_id);
        let srcs: Vec<u64> = match aux.src {
            Some(s) => vec![s],
            None => nwk_src.and_then(|s| keys.ext_of(s)).into_iter().chain(keys.ext.iter().map(|(_, e)| *e)).collect(),
        };
        let cands: Vec<Key> = match aux.key_id {
            0 => keys.link.clone(),
            1 => keys.nwk.iter().map(|(_, k)| *k).collect(),
            id => keys.link.iter().map(|k| hmac_mmo(k, &[if id == 2 { 0x00 } else { 0x02 }])).collect(),
        };
        'outer: for (ci, k) in cands.iter().enumerate() {
            for sx in &srcs {
                if let Some(pl) = unsecure(&a.header, body, k, *sx, &aux) {
                    a.payload = Some(pl);
                    a.key = Some(*k);
                    a.sec_src = Some(*sx);
                    a.link_key = if aux.key_id != 1 { keys.link.get(ci).copied() } else { None };
                    break 'outer;
                }
            }
        }
        a.aux = Some(aux);
    } else {
        a.payload = Some(body.to_vec());
    }
    if a.ty == APS_CMD {
        a.cmd = a.payload.as_ref().and_then(|p| p.first().copied());
    }
    Some(a)
}

/// An APS data frame; `frag` = (extended frame control, block number).
#[allow(clippy::too_many_arguments)]
pub fn aps_data(dst_ep: u8, cluster: u16, profile: u16, src_ep: u8, counter: u8, payload: &[u8], delivery: u8,
                ack_req: bool, group: Option<u16>, frag: Option<(u8, u8)>, security: bool) -> Vec<u8> {
    let fc = APS_DATA | (delivery << 2) | if ack_req { 0x40 } else { 0 } | if frag.is_some() { 0x80 } else { 0 }
        | if security { 0x20 } else { 0 };
    let mut h = vec![fc];
    if delivery == 0 || delivery == 2 {
        h.push(dst_ep);
    }
    if delivery == 3 {
        h.extend_from_slice(&group.unwrap_or(0).to_le_bytes());
    }
    h.extend_from_slice(&cluster.to_le_bytes());
    h.extend_from_slice(&profile.to_le_bytes());
    h.push(src_ep);
    h.push(counter);
    if let Some((e, b)) = frag {
        h.push(e);
        h.push(b);
    }
    h.extend_from_slice(payload);
    h
}

/// An APS command; `sec` = (key, key id, frame counter, source extended address).
pub fn aps_cmd(counter: u8, payload: &[u8], ack_req: bool, sec: Option<(Key, u8, u32, u64)>, delivery: u8) -> Vec<u8> {
    let fc = APS_CMD | (delivery << 2) | if ack_req { 0x40 } else { 0 } | if sec.is_some() { 0x20 } else { 0 };
    let h = [fc, counter];
    match sec {
        Some((key, key_id, fcnt, src)) => secure(&h, payload, &key, src, fcnt, key_id, None, true),
        None => {
            let mut v = h.to_vec();
            v.extend_from_slice(payload);
            v
        }
    }
}

// ------------------------------------------------------------------ ZCL
#[derive(Clone, Debug, Default)]
pub struct Zcl {
    pub fc: u8,
    pub specific: bool,
    pub mfg_specific: bool,
    pub to_client: bool,
    pub ddr: bool,
    pub mfg: Option<u16>,
    pub seq: u8,
    pub cmd: u8,
    pub payload: Vec<u8>,
}

pub fn zcl_parse(p: &[u8]) -> Option<Zcl> {
    if p.len() < 3 {
        return None;
    }
    let fc = p[0];
    let mut z = Zcl { fc, specific: fc & 3 == 1, mfg_specific: fc & 4 != 0, to_client: fc & 8 != 0,
                      ddr: fc & 0x10 != 0, ..Default::default() };
    let mut i = 1;
    if z.mfg_specific {
        if p.len() < 5 {
            return None;
        }
        z.mfg = Some(u16_at(p, 1)?);
        i = 3;
    }
    z.seq = *p.get(i)?;
    z.cmd = *p.get(i + 1)?;
    z.payload = p.get(i + 2..).map(|s| s.to_vec()).unwrap_or_default();
    Some(z)
}

pub fn zcl_build(seq: u8, cmd: u8, payload: &[u8], specific: bool, ddr: bool, to_client: bool, mfg: Option<u16>) -> Vec<u8> {
    let fc = if specific { 1 } else { 0 } | if mfg.is_some() { 4 } else { 0 } | if to_client { 8 } else { 0 }
        | if ddr { 0x10 } else { 0 };
    let mut h = vec![fc];
    if let Some(m) = mfg {
        h.extend_from_slice(&m.to_le_bytes());
    }
    h.push(seq);
    h.push(cmd);
    h.extend_from_slice(payload);
    h
}

/// A ZCL attribute value.
#[derive(Clone, Debug, PartialEq)]
pub enum ZclValue {
    Int(i64),
    Bytes(Vec<u8>),
    /// an invalid (0xFF length) string
    None,
}

fn zcl_size(t: u8) -> Option<usize> {
    Some(match t {
        0x08 | 0x10 | 0x18 | 0x20 | 0x28 | 0x30 => 1,
        0x09 | 0x19 | 0x21 | 0x29 | 0x31 | 0xE8 | 0xE9 => 2,
        0x0A | 0x1A | 0x22 | 0x2A => 3,
        0x0B | 0x1B | 0x23 | 0x2B | 0x39 | 0xE0 | 0xE1 | 0xE2 | 0xEA => 4,
        0x24 => 5,
        0x25 => 6,
        0x26 => 7,
        0x27 | 0xF0 => 8,
        0xF1 => 16,
        _ => return None,
    })
}

/// (value, next index) of a ZCL value of type `t` at `b[i..]`.
impl ZclValue {
    /// The integer, if it is one.
    pub fn int(&self) -> Option<i64> {
        match self {
            ZclValue::Int(v) => Some(*v),
            _ => None,
        }
    }
    pub fn bytes(&self) -> Option<&[u8]> {
        match self {
            ZclValue::Bytes(b) => Some(b),
            _ => None,
        }
    }
}

/// Python's `str()` of a value as emulator/spec/frames.py decodes it (for
/// observation texts identical to the Python suite's).
pub fn py_value(v: &Option<ZclValue>) -> String {
    match v {
        None | Some(ZclValue::None) => "None".into(),
        Some(ZclValue::Int(i)) => i.to_string(),
        Some(ZclValue::Bytes(b)) => py_bytes(b),
    }
}

/// Python's `repr()` of a bytes object.
pub fn py_bytes(b: &[u8]) -> String {
    let q = if b.contains(&b'\'') && !b.contains(&b'"') { '"' } else { '\'' };
    let mut s = String::from("b");
    s.push(q);
    for &c in b {
        match c {
            b'\\' => s.push_str("\\\\"),
            b'\t' => s.push_str("\\t"),
            b'\n' => s.push_str("\\n"),
            b'\r' => s.push_str("\\r"),
            c if c as char == q => {
                s.push('\\');
                s.push(q);
            }
            0x20..=0x7e => s.push(c as char),
            c => s.push_str(&format!("\\x{c:02x}")),
        }
    }
    s.push(q);
    s
}

pub fn zcl_value(t: u8, b: &[u8], i: usize) -> Result<(ZclValue, usize), String> {
    if t == 0x41 || t == 0x42 {
        let n = *b.get(i).ok_or("truncated ZCL value")? as usize;
        if n == 0xFF {
            return Ok((ZclValue::None, i + 1));
        }
        let s = b.get(i + 1..i + 1 + n).ok_or("truncated ZCL value")?;
        return Ok((ZclValue::Bytes(s.to_vec()), i + 1 + n));
    }
    let n = zcl_size(t).ok_or_else(|| format!("unknown ZCL type {t:#04x}"))?;
    let s = b.get(i..i + n).ok_or("truncated ZCL value")?;
    if n > 8 {
        return Ok((ZclValue::Bytes(s.to_vec()), i + n));
    }
    let mut v: i64 = 0;
    for (k, x) in s.iter().enumerate() {
        v |= (*x as i64) << (8 * k);
    }
    if matches!(t, 0x28..=0x2B) && v >= 1 << (8 * n - 1) {
        v -= 1 << (8 * n);
    }
    Ok((ZclValue::Int(v), i + n))
}

/// Read Attributes Response -> [(attribute, status, type, value)], in order.
pub fn read_attr_records(b: &[u8]) -> Result<Vec<(u16, u8, Option<u8>, Option<ZclValue>)>, String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 3 <= b.len() {
        let attr = u16::from_le_bytes([b[i], b[i + 1]]);
        let st = b[i + 2];
        i += 3;
        if st != 0 {
            out.push((attr, st, None, None));
            continue;
        }
        let t = *b.get(i).ok_or("truncated record")?;
        let (v, j) = zcl_value(t, b, i + 1)?;
        i = j;
        out.push((attr, 0, Some(t), Some(v)));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fcs_and_mac_round_trip() {
        let mpdu = mac_build(1, 0x41, &[1, 2, 3], 0x1A62, 0x7A21, 2, 0x0000, 2, Some(0x1A62), true, false);
        let psdu = with_fcs(&mpdu);
        let m = mac_parse(&psdu).unwrap();
        assert_eq!((m.ty, m.seq, m.ar, m.panc, m.dpan, m.dst, m.src), (1, 0x41, true, true, Some(0x1A62), Some(0x7A21), Some(0)));
        assert_eq!(m.payload, vec![1, 2, 3]);
        assert_eq!(crc16_kermit(b"123456789"), 0x2189);
    }

    #[test]
    fn nwk_secure_round_trip() {
        let key: Key = core::array::from_fn(|i| i as u8);
        let o = NwkOpts { src_ieee: Some(0x00124B0000000001), sec: Some((key, 0, 7, 0x00124B0000000001)), ..Default::default() };
        let f = nwk_build(NWK_DATA, 0x7A21, 0x0000, 0x10, b"hello", &o);
        let mut keys = Keys::default();
        keys.set_nwk(0, key);
        let n = nwk_parse(&f, &mut keys).unwrap();
        assert_eq!(n.payload.as_deref(), Some(&b"hello"[..]));
        assert_eq!((n.fc, n.sec_src, n.src_ieee), (Some(7), Some(0x00124B0000000001), Some(0x00124B0000000001)));
        assert_eq!(keys.ext_of(0x0000), Some(0x00124B0000000001));
    }
}

#[cfg(test)]
mod py_tests {
    use super::*;
    #[test]
    fn bytes_repr_as_python() {
        assert_eq!(py_bytes(b"AwoX"), "b'AwoX'");
        assert_eq!(py_bytes(&[0, 0x41, 0xff, b'\'']), "b\"\\x00A\\xff'\"");
        assert_eq!(py_bytes(b"a\\b\n"), "b'a\\\\b\\n'");
        assert_eq!(py_value(&Some(ZclValue::Int(-3))), "-3");
    }
}
