//! The AwoX application protocol (Telink light mesh over GATT), only as a means
//! to make the bulb do something observable in the BLE tests (`MeshApp` of
//! spec_ble/harness.py with emulator/awox_mesh.py and flasher/awox_ota.py).

use super::harness::{hx, BleSession};
use crate::crypto::{sha256, telink_aes};
use crate::session::{Stop, R};

const TELINK_BASE: [u8; 13] = [0x0d, 0x0c, 0x0b, 0x0a, 0x09, 0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01];
const AWOX_SECRET: [u8; 8] = [0x56, 0xE8, 0x50, 0xED, 0xCF, 0xDD, 0x8B, 0x99];
const VENDOR: [u8; 2] = [0x60, 0x01];
pub const OP_POWER: u8 = 0xD0;

/// The login's random value. Python draws it from os.urandom; it changes only
/// the bytes of the login exchange, not what the tests observe.
const RND8: [u8; 8] = [0x5a, 0x17, 0xc3, 0x8e, 0x21, 0x9b, 0x40, 0xf6];

fn pad16(b: &[u8]) -> [u8; 16] {
    let mut o = [0u8; 16];
    o[..b.len().min(16)].copy_from_slice(&b[..b.len().min(16)]);
    o
}

fn aes(key: &[u8; 16], data: &[u8]) -> [u8; 16] {
    telink_aes(key, &pad16(data))
}

fn checksum(key: &[u8; 16], nonce8: &[u8], payload: &[u8]) -> [u8; 16] {
    let mut n = nonce8.to_vec();
    n.push(payload.len() as u8);
    let mut chk = aes(key, &n);
    for blk in payload.chunks(16) {
        let b = pad16(blk);
        let x: Vec<u8> = chk.iter().zip(b.iter()).map(|(a, b)| a ^ b).collect();
        chk = aes(key, &x);
    }
    chk
}

fn crypt(key: &[u8; 16], nonce8: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut base = [0u8; 16];
    base[1..1 + nonce8.len()].copy_from_slice(nonce8);
    let mut out = Vec::new();
    for blk in payload.chunks(16) {
        let e = aes(key, &base);
        out.extend(e.iter().zip(blk.iter()).map(|(a, b)| a ^ b));
        base[0] = base[0].wrapping_add(1);
    }
    out
}

/// 20 octets: sn(3) chk(2) payload(15); payload = dst LE16, op, vendor, parameters.
pub fn command_packet(key: &[u8; 16], mac_le: &[u8], sn: u32, dest: u16, op: u8, params: &[u8]) -> Vec<u8> {
    let s = &sn.to_le_bytes()[..3];
    let mut nonce = mac_le[..4].to_vec();
    nonce.push(0x01);
    nonce.extend_from_slice(s);
    let mut payload = dest.to_le_bytes().to_vec();
    payload.push(op);
    payload.extend_from_slice(&VENDOR);
    payload.extend_from_slice(params);
    payload.resize(15.max(payload.len()), 0);
    let chk = checksum(key, &nonce, &payload);
    let mut out = s.to_vec();
    out.extend_from_slice(&chk[..2]);
    out.extend(crypt(key, &nonce, &payload));
    out
}

/// "unpaired" network password in secure mode; mac6 in display order.
pub fn secure_password(mac6: &[u8]) -> [u8; 16] {
    let mut m = mac6.to_vec();
    m.extend_from_slice(&AWOX_SECRET);
    m.extend_from_slice(&[0, 0]);
    let d = sha256(&m);
    d[..16].try_into().unwrap()
}

fn name_pass(name: &[u8], password: &[u8]) -> [u8; 16] {
    let (n, p) = (pad16(name), pad16(password));
    let mut o = [0u8; 16];
    for i in 0..16 {
        o[i] = n[i] ^ p[i];
    }
    o
}

/// Mesh login over the attribute protocol and the status notification switch.
#[derive(Clone, Debug)]
pub struct MeshApp {
    /// value handles of ...1911 (status), ...1912 (command), ...1914 (pairing)
    pub h11: u16,
    pub h12: u16,
    pub h14: u16,
    pub mac_le: Vec<u8>,
    pub key: Option<[u8; 16]>,
    pub sn: u32,
}

impl MeshApp {
    pub fn new(s: &mut BleSession) -> R<MeshApp> {
        let g = s.gatt()?;
        let mut h = std::collections::HashMap::new();
        for c in &g.chars {
            if c.uuid.len() == 16 && c.uuid[2..15] == TELINK_BASE {
                h.insert(c.uuid[0], c.value);
            }
        }
        if ![0x11u8, 0x12, 0x14].iter().all(|k| h.contains_key(k)) {
            return Err(Stop::Inconclusive("the Telink mesh characteristics (…1911, …1912, …1914) were not found".into()));
        }
        let mac_le = super::harness::unhx(s.lk().st.s("adva"));
        Ok(MeshApp { h11: h[&0x11], h12: h[&0x12], h14: h[&0x14], mac_le, key: None, sn: 0x100 })
    }

    fn handle(&self, ch: u8) -> u16 {
        match ch {
            0x11 => self.h11,
            0x12 => self.h12,
            _ => self.h14,
        }
    }

    /// `Tr.write`: Err(text of the OtaError) when the write is not acknowledged.
    fn write(&self, s: &mut BleSession, ch: u8, data: &[u8]) -> R<Result<(), String>> {
        let mut q = vec![0x12];
        q.extend_from_slice(&self.handle(ch).to_le_bytes());
        q.extend_from_slice(data);
        let rsp = s.att_t(&q, 5000.0)?;
        match rsp {
            Some(r) if r.first() == Some(&0x13) => Ok(Ok(())),
            r => Ok(Err(format!("write {ch:#x}: {}", r.map(|r| hx(&r)).unwrap_or_else(|| "None".into())))),
        }
    }

    fn read(&self, s: &mut BleSession, ch: u8) -> R<Result<Vec<u8>, String>> {
        let mut q = vec![0x0a];
        q.extend_from_slice(&self.handle(ch).to_le_bytes());
        let rsp = s.att_t(&q, 5000.0)?;
        match rsp {
            Some(r) if r.first() == Some(&0x0B) => Ok(Ok(r[1..].to_vec())),
            r => Ok(Err(format!("read {ch:#x}: {}", r.map(|r| hx(&r)).unwrap_or_else(|| "None".into())))),
        }
    }

    /// Logs in as "unpaired" with the secure-mode password.
    pub fn login(mut self, s: &mut BleSession) -> R<MeshApp> {
        let mut mac6 = self.mac_le.clone();
        mac6.reverse();
        let pw = secure_password(&mac6);
        let np = name_pass(b"unpaired", &pw);
        let enc = telink_aes(&pad16(&RND8), &np);
        let mut pkt = vec![0x0c];
        pkt.extend_from_slice(&RND8);
        pkt.extend_from_slice(&enc[..8]);
        let fail = |e: String| Stop::Inconclusive(format!("mesh login failed ({e}): the application protocol could not be used"));
        self.write(s, 0x14, &pkt)?.map_err(fail)?;
        let rsp = self.read(s, 0x14)?.map_err(fail)?;
        if rsp.first() != Some(&0x0D) {
            let t = if rsp.is_empty() { "b''".to_string() } else { hx(&rsp) };
            return Err(fail(format!("login refused: {t}")));
        }
        let mut rr = RND8.to_vec();
        rr.extend_from_slice(&rsp[1..9]);
        self.key = Some(telink_aes(&np, &pad16(&rr)));
        Ok(self)
    }

    /// The application's own notification switch: 0x01 written to the status characteristic.
    pub fn status_on(&self, s: &mut BleSession) -> R<Result<(), String>> {
        self.write(s, 0x11, &[0x01])
    }

    pub fn command(&mut self, s: &mut BleSession, op: u8, params: &[u8]) -> R<Result<(), String>> {
        self.sn += 1;
        let pkt = command_packet(&self.key.unwrap_or([0; 16]), &self.mac_le, self.sn, 0xFFFF, op, params);
        self.write(s, 0x12, &pkt)
    }
}
