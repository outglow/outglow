//! Touchlink: the Rust firmware as a target next to the stock firmware's
//! (Telink's touchlink target as AwoX built it; once rust_touchlink_sim.py
//! against the C firmware).
//!
//! Two bulbs, Rust and stock, first on the coordinator's network. An initiator
//! written here scans, asks for the device information, makes them identify,
//! hands them a new network (Network Join Router), resets them to factory new
//! and asks each to start a network (Network Start); the answers are compared
//! field by field and what each does afterwards is watched on the air.
//!
//! Keys (ZLL 1.0 8.7.5): each bulb is sent the network key under the best key
//! index both sides have: 4 (the touchlink master key) when the bulb offers
//! it and the key is at hand, else 15 (the certification key) or 0 (the
//! development key). The master key is not in this repository:
//! `TOUCHLINK_MASTER_KEY` (32 hex digits), else it is read from the stock
//! image (the dimmable and tunable white images keep it at a recognisable
//! place). A Rust image built without it offers 0 and 15 only.
//!
//! Images: `--image rust=...` (default target/tc32/bulb-tc32.bin) and
//! `--image stock=...` (default stock/stock-dimm.bin).

use super::touchlink::{encrypted_key, nwk_decrypt, TOUCHLINK, ZLL_PROFILE};
use super::mixed::{ieee_of, mac_record, Fw};
use super::{node_of, Sim, SimCtx, SimDef};
use crate::air::{Ev, NodeId};
use crate::coord::{Coord, NWK_KEY};
use crate::frames::{airtime_us, mac_build, mac_parse, with_fcs, Key};
use crate::peer::Peer;
use crate::session::R;
use crate::station::{Station, StationEv};

pub fn def() -> SimDef {
    SimDef { name: "rust_touchlink", doc: "the Rust touchlink target next to the stock one, driven by a scripted initiator", run }
}

const QUANTUM: f64 = 0.25;
/// Scan requests per discovery on a channel: as on an initiator's first
/// primary channel (BDB 3.0.1 8.6, step 3).
const SCAN_REQUESTS: usize = 5;
/// bdbcTLScanTimeBaseDuration: the wait for responses after each scan request.
const SCAN_WAIT_MS: f64 = 250.0;
/// macMaxFrameRetries, and the time from one copy of a unicast to the next.
const MAC_RETRIES: usize = 3;
const MAC_RETRY_MS: f64 = 5.0;
const REC_R: [u8; 8] = mac_record([0x01, 0x00, 0x52, 0x00, 0x00]);
const REC_T: [u8; 8] = mac_record([0x02, 0x00, 0x54, 0x00, 0x00]);
const EXT_R: u64 = ieee_of(Fw::Rust, &REC_R);
const EXT_T: u64 = ieee_of(Fw::Stock, &REC_T);
const NEW_KEY: Key = [0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x2b, 0x2c, 0x2d, 0x2e, 0x2f, 0x30];
const NEW_PAN: u16 = 0x4321;
const NEW_EXT_PAN: u64 = 0x4444333322221111;
const NEW_CHANNEL: i64 = 15;
const INI_EXT: u64 = 0x00AABBCCDDEEFF01;

fn new_short(name: &str) -> u16 {
    if name == "R" {
        0x0043
    } else {
        0x0042
    }
}

/// The scripted initiator: inter-PAN frames out; what it heard is read back from the air.
struct Ini {
    id: NodeId,
    mac_seq: u8,
    zcl_seq: u8,
    /// (air time it was set, channel)
    chans: Vec<(f64, i64)>,
}

/// A Scan Response's fields in the Python dict's order.
#[derive(Clone, Debug)]
struct ScanRsp(Vec<(&'static str, u64)>);

impl ScanRsp {
    fn parse(p: &[u8]) -> ScanRsp {
        let u16_ = |i: usize| u16::from_le_bytes([p[i], p[i + 1]]) as u64;
        let u32_ = |i: usize| u32::from_le_bytes(p[i..i + 4].try_into().unwrap()) as u64;
        let mut v = vec![
            ("transaction", u32_(0)),
            ("rssi_correction", p[4] as u64),
            ("zigbee_info", p[5] as u64),
            ("touchlink_info", p[6] as u64),
            ("keys", u16_(7)),
            ("response_id", u32_(9)),
            ("ext_pan", u64::from_le_bytes(p[13..21].try_into().unwrap())),
            ("update_id", p[21] as u64),
            ("channel", p[22] as u64),
            ("pan", u16_(23)),
            ("short", u16_(25)),
            ("sub_devices", p[27] as u64),
            ("group_ids", p[28] as u64),
        ];
        if p[27] == 1 && p.len() >= 36 {
            v.extend([("endpoint", p[29] as u64), ("profile", u16_(30)), ("device", u16_(32)), ("version", p[34] as u64),
                      ("group_count", p[35] as u64)]);
        }
        ScanRsp(v)
    }
    fn get(&self, k: &str) -> Option<u64> {
        self.0.iter().find(|(x, _)| *x == k).map(|(_, v)| *v)
    }
    fn g(&self, k: &str) -> u64 {
        self.get(k).unwrap_or(0)
    }
    fn describe(&self) -> String {
        self.0.iter().filter(|(k, _)| *k != "transaction" && *k != "response_id").map(|(k, v)| format!("{k}={v:#x}")).collect::<Vec<_>>().join(", ")
    }
}

struct Ctx {
    s: Sim,
    ini: Ini,
    r: NodeId,
    t: NodeId,
    c2: Option<usize>,
    transaction: u32,
    master: Option<[u8; 16]>,
    /// key index used with each bulb ("R", "T")
    index: Vec<(&'static str, u8)>,
}

fn ext_of(name: &str) -> u64 {
    if name == "R" {
        EXT_R
    } else {
        EXT_T
    }
}

fn who_of(ext: u64) -> Option<&'static str> {
    match ext {
        EXT_R => Some("R"),
        EXT_T => Some("T"),
        _ => None,
    }
}

impl Ctx {
    fn now(&self) -> f64 {
        self.s.now()
    }
    fn run(&mut self, ms: f64) -> R<()> {
        self.s.run(ms)
    }
    fn run_until(&mut self, max_ms: f64, step: f64, cond: impl FnMut(&Sim) -> bool) -> R<bool> {
        self.s.run_until(max_ms, step, cond)
    }
    fn set_channel(&mut self, ch: i64) {
        let t = self.s.w.air.t;
        self.ini.chans.push((t, ch));
        let (st, _) = self.s.w.st(self.ini.id);
        st.channel = ch;
    }
    fn channel_at(chans: &[(f64, i64)], t1: f64) -> i64 {
        let mut ch = chans[0].1;
        for (t, c) in &chans[1..] {
            if *t < t1 {
                ch = *c;
            }
        }
        ch
    }

    /// `Initiator.rx`: (t1, source ext, ZCL command, payload after it) of the touchlink frames it received.
    fn rx(s: &Sim, chans: &[(f64, i64)], id: NodeId) -> Vec<(f64, u64, u8, Vec<u8>)> {
        let mut out = Vec::new();
        for f in &s.w.air.frames {
            if f.who == id || f.t1 > s.w.air.t || f.psdu.len() < 12 || f.ch != Self::channel_at(chans, f.t1) {
                continue;
            }
            let Some(m) = mac_parse(&f.psdu) else { continue };
            if m.ty != 1 || m.dmode != 3 || m.dst != Some(INI_EXT) {
                continue;
            }
            let p = &m.payload;
            if p.len() < 10 || p[0] & 3 != 3 || p[2] & 3 != 3 {
                continue;
            }
            let (cl, pr) = (u16::from_le_bytes([p[3], p[4]]), u16::from_le_bytes([p[5], p[6]]));
            if (cl, pr) != (TOUCHLINK, ZLL_PROFILE) {
                continue;
            }
            out.push((f.t1, m.src.unwrap_or(0), p[9], p[10..].to_vec()));
        }
        out
    }

    fn my_rx(&self) -> Vec<(f64, u64, u8, Vec<u8>)> {
        Self::rx(&self.s, &self.ini.chans, self.ini.id)
    }

    /// A touchlink command: broadcast (dst None) or to one device.
    fn send(&mut self, dst: Option<u64>, cmd: u8, payload: &[u8], delay_ms: f64) {
        let psdu = self.frame(dst, cmd, payload);
        self.put(psdu, delay_ms);
    }

    /// The PSDU of a touchlink command, with the next MAC and ZCL sequence numbers.
    fn frame(&mut self, dst: Option<u64>, cmd: u8, payload: &[u8]) -> Vec<u8> {
        self.ini.zcl_seq = self.ini.zcl_seq.wrapping_add(1);
        self.ini.mac_seq = self.ini.mac_seq.wrapping_add(1);
        let mut body = vec![0x0B, 0x00, if dst.is_none() { 0x0B } else { 0x03 }];
        body.extend_from_slice(&TOUCHLINK.to_le_bytes());
        body.extend_from_slice(&ZLL_PROFILE.to_le_bytes());
        body.extend_from_slice(&[0x11, self.ini.zcl_seq, cmd]);
        body.extend_from_slice(payload);
        let mpdu = match dst {
            None => mac_build(1, self.ini.mac_seq, &body, 0xFFFF, 0xFFFF, 2, INI_EXT, 3, Some(0x7777), false, false),
            Some(d) => mac_build(1, self.ini.mac_seq, &body, 0xFFFF, d, 3, INI_EXT, 3, Some(0x7777), true, false),
        };
        with_fcs(&mpdu)
    }

    /// Puts a PSDU on the air `delay_ms` from now.
    fn put(&mut self, psdu: Vec<u8>, delay_ms: f64) {
        let t0 = self.s.w.air.now() + delay_ms;
        let t1 = t0 + airtime_us(psdu.len()) / 1000.0;
        self.s.w.air.at(t0, Ev::Node(self.ini.id, StationEv::Transmit { psdu, t0, t1 }));
    }

    /// Whether `who` acknowledged MAC sequence number `seq` since `since`.
    fn acked(&self, who: NodeId, seq: u8, since: f64) -> bool {
        self.s.w.air.frames.iter().any(|f| f.t0 >= since && f.who == who && f.psdu.len() == 5 && f.psdu[0] & 7 == 2 && f.psdu[2] == seq)
    }

    /// A unicast to one bulb ("R" or "T") as the initiator's MAC sends it
    /// (802.15.4-2006 7.5.6.4): the same frame again while it is not
    /// acknowledged, macMaxFrameRetries (3) times at most, a few
    /// milliseconds apart (the acknowledgement wait and a backoff). Either
    /// bulb's receiver is off for about 2 ms every 300 ms while its radio
    /// is lent to a BLE advertising event; a request sent once was lost
    /// whenever its moment fell into one (with the Rust image that moment
    /// moves with the code's layout). Whether it was acknowledged.
    fn send_mac(&mut self, name: &str, cmd: u8, payload: &[u8]) -> R<bool> {
        let who = if name == "R" { self.r } else { self.t };
        let psdu = self.frame(Some(ext_of(name)), cmd, payload);
        let seq = self.ini.mac_seq;
        for _ in 0..=MAC_RETRIES {
            let sent = self.now();
            self.put(psdu.clone(), 1.0);
            self.run(MAC_RETRY_MS)?;
            if self.acked(who, seq, sent) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// A unicast to one bulb ("R" or "T"), sent again while it is not
    /// acknowledged, three times at most, as an initiator's MAC retries: the
    /// stock bulb's receiver is off now and then while its radio is on BLE.
    /// Runs `wait_ms` after each. Whether it was acknowledged.
    fn send_acked(&mut self, name: &str, cmd: u8, payload: &[u8], wait_ms: f64) -> R<bool> {
        let who = if name == "R" { self.r } else { self.t };
        for _ in 0..3 {
            let sent = self.now();
            self.send(Some(ext_of(name)), cmd, payload, 1.0);
            self.run(wait_ms)?;
            if self.acked(who, self.ini.mac_seq, sent) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Touchlink device discovery on one channel (BDB 3.0.1 8.6, steps 2
    /// and 3): one new transaction identifier for all of it, up to
    /// `requests` scan requests with it, bdbcTLScanTimeBaseDuration
    /// (0.25 s) after each, until the wanted bulbs have answered; the
    /// responses to any of them. A target answers the transaction once: the
    /// stock one (Telink's target) ignores a scan request with the
    /// transaction it has answered, and while it is not factory new also
    /// one with a new transaction, until its transaction timer
    /// (bdbcTLInterPANTransIdLifetime, 8 s) has run out. So the bulbs must
    /// not be asked to answer one and the same request: a bulb whose radio
    /// was lent to a BLE advertising event as the first request went by
    /// answers the next one.
    fn discover(&mut self, channel: i64, want: &[&str], requests: usize) -> R<Vec<(&'static str, ScanRsp)>> {
        self.set_channel(channel);
        self.transaction += 1;
        let t0 = self.now();
        let mut got: Vec<(&'static str, ScanRsp)> = Vec::new();
        for _ in 0..requests {
            let mut pl = self.transaction.to_le_bytes().to_vec();
            pl.extend_from_slice(&[0x05, 0x12]);
            self.send(None, 0x00, &pl, 1.0);
            self.run(SCAN_WAIT_MS)?;
            got.clear();
            for (t, src, cmd, p) in self.my_rx() {
                if t < t0 || cmd != 0x01 || p.len() < 4 {
                    continue;
                }
                let Some(w) = who_of(src) else { continue };
                if u32::from_le_bytes(p[0..4].try_into().unwrap()) != self.transaction {
                    continue;
                }
                // the latest response of a bulb: the one whose response identifier it keeps
                let d = ScanRsp::parse(&p);
                match got.iter_mut().find(|(k, _)| *k == w) {
                    Some(e) => e.1 = d,
                    None => got.push((w, d)),
                }
            }
            if want.iter().all(|k| got.iter().any(|(x, _)| x == k)) {
                break;
            }
        }
        Ok(got)
    }

    /// Scans on a channel as an initiator scans its first primary channel
    /// (five scan requests, `discover`) until the wanted bulbs have answered
    /// within one discovery, `tries` discoveries at most; their responses.
    fn scan(&mut self, channel: i64, want: &[&str], tries: usize) -> R<Vec<(&'static str, ScanRsp)>> {
        for _ in 0..tries {
            let got = self.discover(channel, want, SCAN_REQUESTS)?;
            if want.iter().all(|k| got.iter().any(|(x, _)| x == k)) {
                return Ok(got);
            }
            self.run(400.0)?;
        }
        Ok(Vec::new())
    }

    /// A request to one bulb (with MAC retries, `send_mac`), and its response.
    fn request(&mut self, name: &str, cmd: u8, payload: &[u8], rsp_cmd: u8, wait: f64) -> R<Option<Vec<u8>>> {
        let t0 = self.now();
        let mut pl = self.transaction.to_le_bytes().to_vec();
        pl.extend_from_slice(payload);
        self.send_mac(name, cmd, &pl)?;
        let e = ext_of(name);
        let (chans, id) = (self.ini.chans.clone(), self.ini.id);
        self.run_until(wait, 10.0, |s| Ctx::rx(s, &chans, id).iter().any(|(t, src, k, _)| *t >= t0 && *src == e && *k == rsp_cmd))?;
        Ok(self.my_rx().into_iter().find(|(t, src, k, _)| *t >= t0 && *src == e && *k == rsp_cmd).map(|x| x.3))
    }

    fn key_index(&self, name: &str) -> u8 {
        self.index.iter().find(|(k, _)| *k == name).map(|x| x.1).unwrap_or(4)
    }

    /// The network key as sent under the bulb's key index.
    fn enc_key(&self, name: &str, rid: u32) -> [u8; 16] {
        let t = self.transaction;
        match self.key_index(name) {
            0 => {
                // development key: the transport key is "PhLi" | transaction | "CLSN" | response (ZLL 8.7.5.2.3)
                let mut k = [0u8; 16];
                k[..4].copy_from_slice(b"PhLi");
                k[4..8].copy_from_slice(&t.to_be_bytes());
                k[8..12].copy_from_slice(b"CLSN");
                k[12..].copy_from_slice(&rid.to_be_bytes());
                tc32emu::aes128::encrypt(&k, &NEW_KEY)
            }
            15 => encrypted_key(&CERT_KEY, &NEW_KEY, t, rid),
            _ => encrypted_key(&self.master.unwrap_or([0; 16]), &NEW_KEY, t, rid),
        }
    }

    fn join_payload(&self, name: &str, rid: u32, start: bool) -> Vec<u8> {
        let key = self.enc_key(name, rid);
        let mut p = NEW_EXT_PAN.to_le_bytes().to_vec();
        p.push(self.key_index(name));
        p.extend_from_slice(&key);
        if !start {
            p.push(1); // network update identifier
        }
        p.push(NEW_CHANNEL as u8);
        p.extend_from_slice(&NEW_PAN.to_le_bytes());
        p.extend_from_slice(&new_short(name).to_le_bytes());
        p.extend_from_slice(&[0; 12]);
        if start {
            p.extend_from_slice(&INI_EXT.to_le_bytes());
            p.extend_from_slice(&1u16.to_le_bytes());
        }
        p
    }

    fn open_start(&self, rid: u32) -> Vec<u8> {
        let key = self.enc_key("R", rid);
        let mut p = 0u64.to_le_bytes().to_vec();
        p.push(self.key_index("R"));
        p.extend_from_slice(&key);
        p.push(0);
        p.extend_from_slice(&0u16.to_le_bytes());
        p.extend_from_slice(&0x0005u16.to_le_bytes());
        p.extend_from_slice(&[0; 12]);
        p.extend_from_slice(&INI_EXT.to_le_bytes());
        p.extend_from_slice(&1u16.to_le_bytes());
        p
    }

    /// Scans the channels until the bulb answers; its scan response (the initiator stays on that channel).
    /// Scans as a touchlink initiator does (BDB 8.4): five requests on the
    /// first primary channel, then one on each of the other primary channels,
    /// then the secondary ones, round after round. A factory-new bulb looking
    /// for a network to join leaves its channel for a couple of seconds at a
    /// time; one request per channel in turn kept landing in those passes.
    fn find(&mut self, name: &'static str, tries: usize) -> R<Option<ScanRsp>> {
        const CH: [i64; 20] = [11, 11, 11, 11, 11, 15, 20, 25, 12, 13, 14, 16, 17, 18, 19, 21, 22, 23, 24, 26];
        for i in 0..tries {
            let got = self.discover(CH[i % CH.len()], &[name], 1)?;
            if let Some((_, d)) = got.into_iter().find(|(k, _)| *k == name) {
                return Ok(Some(d));
            }
            self.run(400.0)?;
        }
        Ok(None)
    }

    /// Finds the bulb, sends the request built for the scan's response identifier.
    fn ask(&mut self, name: &'static str, cmd: u8, payload_for: &dyn Fn(&Ctx, u32) -> Vec<u8>, rsp_cmd: u8)
           -> R<(Option<ScanRsp>, Option<Vec<u8>>)> {
        for _ in 0..8 {
            let Some(d) = self.find(name, 40)? else { continue };
            let pl = payload_for(self, d.g("response_id") as u32);
            if let Some(r) = self.request(name, cmd, &pl, rsp_cmd, 500.0)? {
                return Ok((Some(d), Some(r)));
            }
        }
        Ok((None, None))
    }

    fn c2(&self) -> &Coord {
        &self.s.w.coords[self.c2.unwrap()]
    }
}

fn ev_dicts(evs: &[crate::coord::CoordEvent]) -> String {
    let items: Vec<String> = evs
        .iter()
        .map(|e| {
            let kv: Vec<String> = e.details.split(", ").filter_map(|kv| kv.split_once('=')).map(|(k, v)| format!("'{k}': '{v}'")).collect();
            format!("{{{}}}", kv.join(", "))
        })
        .collect();
    format!("[{}]", items.join(", "))
}

fn opt_hex(v: Option<u16>) -> String {
    match v {
        None => "None".into(),
        Some(0) => "0".into(),
        Some(x) => format!("'{x:#x}'"),
    }
}

fn rsp_dict(pairs: &[(&str, bool)]) -> String {
    format!("{{{}}}", pairs.iter().map(|(k, v)| format!("'{k}': {}", if *v { "True" } else { "False" })).collect::<Vec<_>>().join(", "))
}

/// The touchlink certification key (ZLL 1.0 8.7.5.2.3, key index 15).
const CERT_KEY: [u8; 16] = [0xC0, 0xC1, 0xC2, 0xC3, 0xC4, 0xC5, 0xC6, 0xC7, 0xC8, 0xC9, 0xCA, 0xCB, 0xCC, 0xCD, 0xCE, 0xCF];

/// The touchlink master key: `TOUCHLINK_MASTER_KEY`, else where the stock
/// dimmable and tunable white images keep it (16 bytes between the end of an
/// attribute table, `fd ff 21 01 .. .. 0a 00`, and `98 3a 00 00 0f 01 19 00`;
/// found once in each). (where it came from, key)
pub fn master_key(stock: &std::path::Path) -> (String, Option<[u8; 16]>) {
    if let Ok(h) = std::env::var("TOUCHLINK_MASTER_KEY") {
        let b: Vec<u8> = (0..h.len() / 2).filter_map(|i| u8::from_str_radix(h.get(2 * i..2 * i + 2)?, 16).ok()).collect();
        return ("TOUCHLINK_MASTER_KEY".into(), b.try_into().ok());
    }
    let Ok(d) = std::fs::read(stock) else { return ("no stock image".into(), None) };
    const TAIL: [u8; 8] = [0x98, 0x3a, 0x00, 0x00, 0x0f, 0x01, 0x19, 0x00];
    let hits: Vec<usize> = (0..d.len().saturating_sub(32))
        .filter(|&i| d[i..i + 4] == [0xfd, 0xff, 0x21, 0x01] && d[i + 6..i + 8] == [0x0a, 0x00] && d[i + 24..i + 32] == TAIL)
        .collect();
    match hits.as_slice() {
        [i] => (format!("the stock image at {:#x}", i + 8), d[i + 8..i + 24].try_into().ok()),
        _ => (format!("not found in the stock image ({} places)", hits.len()), None),
    }
}

fn run(cx: &mut SimCtx) -> R<()> {
    let ri = super::mixed::image(cx, Fw::Rust);
    let ti = super::mixed::image(cx, Fw::Stock);
    let (from, master) = master_key(&ti.path);
    let mut r_node = node_of(&ri, cx.cpi)?;
    r_node.write_flash(0xFF000, &REC_R);
    let mut t_node = node_of(&ti, cx.cpi)?;
    t_node.write_flash(0xFF000, &REC_T);
    let mut s = Sim::new(r_node, "R", Coord::new_plain(11, NWK_KEY), cx.log(), QUANTUM);
    let (r, t) = (NodeId::Dut, s.w.add_bulb(t_node, "T"));
    s.w.set_t_off(t, 1500.0);
    s.w.coord.start(&mut s.w.air);
    let st = Station::new(NodeId::Peer(0), 11, 0x7777, None, INI_EXT, NWK_KEY, (0x70, 0, 0, 0, 0), 0, 0);
    let ini_id = s.w.add_peer_node(Peer::bare_ack(st, "initiator"));
    let mut c = Ctx { s, ini: Ini { id: ini_id, mac_seq: 0x70, zcl_seq: 0, chans: vec![(0.0, 11)] }, r, t, c2: None, transaction: 0x1000,
                      master, index: Vec::new() };
    let rel = super::util::rel;
    cx.say(&format!("== Rust target R: {}; stock target T: {}", rel(&ri), rel(&ti)));
    cx.say(&format!("   touchlink master key: {}", if c.master.is_some() { format!("from {from}") } else { format!("none ({from})") }));
    let res = body(cx, &mut c);
    cx.finish(&c.s, &[(r, "R"), (t, "T")]);
    res
}

fn body(cx: &mut SimCtx, c: &mut Ctx) -> R<()> {
    let ok = c.run_until(20000.0, 10.0, |s| s.announced(EXT_R).is_some() && s.announced(EXT_T).is_some())?;
    let short = (c.s.announced(EXT_R), c.s.announced(EXT_T));
    if !cx.check(c.now(), "both bulbs join the coordinator's network", ok,
                 &format!("{{'R': {}, 'T': {}}}", opt_hex(short.0), opt_hex(short.1))) {
        return Ok(());
    }
    let (sr, sts) = (short.0.unwrap(), short.1.unwrap());
    let shortn = |n: &str| if n == "R" { sr } else { sts };
    c.run_until(15000.0, 10.0, |s| s.events("verify-key").iter().filter(|e| e.get("ok") == Some("True")).count() >= 2)?;
    c.s.w.coord.permit_join = Some(false);
    c.run(1000.0)?;

    // ---- scan
    let got = c.scan(11, &["R", "T"], 12)?;
    if !cx.check(c.now(), "both answer a scan from close by", got.len() == 2, &format!("{} responses", got.len())) {
        return Ok(());
    }
    let gr = |g: &Vec<(&str, ScanRsp)>, k: &str| g.iter().find(|(x, _)| *x == k).map(|(_, d)| d.clone()).unwrap();
    let (rr, tt) = (gr(&got, "R"), gr(&got, "T"));
    cx.say(&format!("     Rust:   {}", rr.describe()));
    cx.say(&format!("     stock:  {}", tt.describe()));
    let own = ["short", "keys", "response_id", "transaction", "profile"];
    let same: Vec<(&str, u64, Option<u64>)> =
        rr.0.iter().filter(|(k, v)| !own.contains(k) && Some(*v) != tt.get(k)).map(|(k, v)| (*k, *v, tt.get(k))).collect();
    let keys_eq = rr.0.len() == tt.0.len() && rr.0.iter().all(|(k, _)| tt.get(k).is_some());
    let detail = if same.is_empty() {
        String::new()
    } else {
        let items: Vec<String> = same
            .iter()
            .map(|(k, a, b)| format!("'{k}': ('{a:#x}', {})", b.map(|x| format!("'{x:#x}'")).unwrap_or_else(|| "None".into())))
            .collect();
        format!("differences (Rust, stock): {{{}}}", items.join(", "))
    };
    cx.check(c.now(), "the two scan responses agree: device type, 'not factory new', Zigbee 3.0, the network, one sub-device (endpoint, device, version)",
             same.is_empty() && keys_eq, &detail);
    let pan = c.s.w.coord.st.pan as u64;
    cx.check(c.now(), "each names its own address in the network",
             rr.g("short") == sr as u64 && tt.g("short") == sts as u64 && rr.g("pan") == pan, "");
    let pick = |bits: u64, master: bool| -> Option<u8> {
        if bits & 0x0010 != 0 && master {
            Some(4)
        } else if bits & 0x8000 != 0 {
            Some(15)
        } else if bits & 0x0001 != 0 {
            Some(0)
        } else {
            None
        }
    };
    let (ki_r, ki_t) = (pick(rr.g("keys"), c.master.is_some()), pick(tt.g("keys"), c.master.is_some()));
    cx.check(c.now(), "key bitmask: the stock target offers the master key (index 4); the Rust image offers it when built with it, \
                       else the development and certification keys (0, 15)",
             tt.g("keys") & 0x0010 != 0 && (rr.g("keys") & 0x0010 != 0 || rr.g("keys") & 0x8001 == 0x8001),
             &format!("Rust {:#06x}, stock {:#06x}", rr.g("keys"), tt.g("keys")));
    let shown = |k: Option<u8>| k.map(|k| k.to_string()).unwrap_or_else(|| "none".into());
    if !cx.check(c.now(), "a key index both the initiator and each bulb have", ki_r.is_some() && ki_t.is_some(),
                 &format!("Rust {}, stock {}{}", shown(ki_r), shown(ki_t),
                          if c.master.is_none() { " (no master key: TOUCHLINK_MASTER_KEY, or a stock image that has it)" } else { "" })) {
        return Ok(());
    }
    c.index = vec![("R", ki_r.unwrap()), ("T", ki_t.unwrap())];
    cx.say(&format!("     (profile of the sub-device: Rust {:#06x}, as in its simple descriptor; stock {:#06x})",
                    rr.g("profile"), tt.g("profile")));

    // ---- device information
    let ri = c.request("R", 0x02, &[0], 0x03, 600.0)?;
    let ti = c.request("T", 0x02, &[0], 0x03, 600.0)?;
    let mut ok = ri.is_some() && ti.is_some();
    let mut detail = String::new();
    if let (Some(ri), Some(ti)) = (&ri, &ti) {
        // transaction, sub-devices, start index, record count; record: IEEE, endpoint, profile, device, version, groups, sort
        ok = ri[..7] == ti[..7] && ri[7..15] == EXT_R.to_le_bytes() && ti[7..15] == EXT_T.to_le_bytes() && ri[15..16] == ti[15..16]
            && ri[18..] == ti[18..];
        let hx = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        detail = format!("Rust {}  stock {}", hx(&ri[4..]), hx(&ti[4..]));
    }
    cx.check(c.now(), "Device Information Response: the same record but for the profile, each with its own IEEE address", ok, &detail);

    // ---- identify
    let before = (c.s.light(c.r).duty, c.s.light(c.t).duty);
    for name in ["R", "T"] {
        let mut pl = c.transaction.to_le_bytes().to_vec();
        pl.extend_from_slice(&3u16.to_le_bytes());
        c.send_acked(name, 0x06, &pl, 30.0)?;
    }
    let (mut seen_r, mut seen_t): (Vec<i64>, Vec<i64>) = (Vec::new(), Vec::new());
    for _ in 0..40 {
        c.run(100.0)?;
        for (v, id) in [(&mut seen_r, c.r), (&mut seen_t, c.t)] {
            let x = c.s.light(id).duty.round_ties_even() as i64;
            if !v.contains(&x) {
                v.push(x);
            }
        }
    }
    seen_r.sort();
    seen_t.sort();
    cx.check(c.now(), "Identify Request makes the Rust bulb blink", seen_r.len() > 1,
             &format!("duty seen: Rust {}, stock {}", crate::py::ints(seen_r.clone()), crate::py::ints(seen_t.clone())));
    c.run(1500.0)?;
    let after_r = c.s.light(c.r).duty;
    cx.check(c.now(), "and it returns to its light afterwards", (after_r - before.0).abs() < 1.0,
             &format!("{:.1} % -> {:.1} %", before.0, after_r));

    // ---- on a network with a Trust Center nobody is taken away
    let got = c.scan(11, &["R", "T"], 12)?;
    if !cx.check(c.now(), "a second scan (new transaction)", got.len() == 2, "") {
        return Ok(());
    }
    let mut status: Vec<(&str, Option<u8>)> = Vec::new();
    for name in ["T", "R"] {
        let rid = gr(&got, name).g("response_id") as u32;
        let pl = c.join_payload(name, rid, false);
        let rsp = c.request(name, 0x12, &pl, 0x13, 600.0)?;
        status.push((name, rsp.map(|r| r[4])));
    }
    let st_ok = status.iter().all(|(_, v)| *v == Some(1));
    let st_s = format!(
        "{{{}}}",
        status.iter().map(|(k, v)| format!("'{k}': {}", v.map(|x| x.to_string()).unwrap_or_else(|| "None".into()))).collect::<Vec<_>>().join(", ")
    );
    cx.check(c.now(), "Network Join Router Request while on the coordinator's network: both refuse", st_ok, &st_s);
    c.scan(11, &["R", "T"], 12)?;
    for name in ["R", "T"] {
        let pl = c.transaction.to_le_bytes();
        c.send_acked(name, 0x07, &pl, 200.0)?;
    }
    c.run(3000.0)?;
    let mut rsp: Vec<(&str, bool)> = Vec::new();
    for name in ["R", "T"] {
        let sh = shortn(name);
        let n0 = c.s.w.coord.zcl_rsp.len();
        c.s.w.coord.zcl_cmd(&mut c.s.w.air, sh, 1, 0x0006, 0x02, &[]);
        let got = |s: &Sim| s.w.coord.zcl_rsp[n0..].iter().any(|x| x.src == sh && x.cluster == 0x0006 && x.cmd == Some(0x0B));
        c.run_until(3000.0, 10.0, got)?;
        rsp.push((name, got(&c.s)));
    }
    cx.check(c.now(), "Reset to Factory New Request there: both stay and still answer the coordinator", rsp.iter().all(|x| x.1),
             &rsp_dict(&rsp));

    // ---- out of the box: the coordinator sends both away and lets nobody back in
    for name in ["R", "T"] {
        let sh = shortn(name);
        let mut pl = vec![0u8; 8];
        pl.push(0);
        c.s.w.coord.zdo_send(&mut c.s.w.air, sh, 0x0034, &pl);
        c.run(500.0)?;
    }
    c.run(3000.0)?;

    // ---- the stock bulb starts the network, the Rust bulb joins it
    let mut c2 = Coord::new_plain(NEW_CHANNEL, NEW_KEY);
    c2.set_network(NEW_CHANNEL, NEW_PAN, INI_EXT, NEW_EXT_PAN, NEW_KEY, Some(false), 0x0001);
    let id2 = c.s.w.add_coord(c2);
    let NodeId::Coord2(k2) = id2 else { unreachable!() };
    c.c2 = Some(k2 as usize);
    let (d, rsp) = c.ask("T", 0x10, &|cx: &Ctx, rid| cx.join_payload("T", rid, true), 0x11)?;
    let start_t = rsp.clone();
    let mut ok = rsp.as_ref().is_some_and(|r| r[4] == 0) && d.as_ref().is_some_and(|d| d.g("touchlink_info") & 1 != 0);
    let mut detail = String::new();
    if let Some(r) = &rsp {
        let (st, epid, ch, pan) = (r[4], u64::from_le_bytes(r[5..13].try_into().unwrap()), r[14], u16::from_le_bytes([r[15], r[16]]));
        ok = ok && (epid, ch as i64, pan) == (NEW_EXT_PAN, NEW_CHANNEL, NEW_PAN);
        let fresh = d.as_ref().is_some_and(|d| d.g("touchlink_info") & 1 != 0);
        detail = format!(
            "factory new in its scan response: {}; status {st}, extended PAN {epid:#018x}, channel {ch}, PAN {pan:#06x}",
            if fresh { "True" } else { "False" }
        );
    }
    cx.check(c.now(), "factory new, the stock bulb accepts a Network Start Request (the key under its key index)", ok, &detail);
    let on_new = |s: &Sim, name: &str| -> bool {
        let want = format!("{:#06x}", new_short(name));
        s.w.coords[k2 as usize].events.iter().any(|e| e.kind == "device-announce" && e.get("short") == Some(want.as_str()))
    };
    c.run_until(15000.0, 10.0, |s| on_new(s, "T"))?;
    cx.check(c.now(), "and turns up on the new network with frames under the key that was handed over", on_new(&c.s, "T"),
             "so the key transport computed here is what the stock target computes");
    let (d, rsp) = c.ask("R", 0x12, &|cx: &Ctx, rid| cx.join_payload("R", rid, false), 0x13)?;
    cx.check(c.now(), "factory new, the Rust bulb accepts a Network Join Router Request for that network (same key transport)",
             rsp.as_ref().is_some_and(|r| r[4] == 0) && d.as_ref().is_some_and(|d| d.g("touchlink_info") & 1 != 0),
             &format!("status {}", rsp.as_ref().map(|r| r[4].to_string()).unwrap_or_else(|| "None".into())));
    c.run_until(15000.0, 10.0, |s| on_new(s, "R"))?;
    cx.check(c.now(), "and turns up there too, under the same key", on_new(&c.s, "R"), "");
    let mic: Vec<_> = c.c2().events.iter().filter(|e| e.kind == "nwk-mic-fail").cloned().collect();
    cx.check(c.now(), "nothing on the new network fails to decrypt", mic.is_empty(), &ev_dicts(&mic[..mic.len().min(3)]));
    c.run(3000.0)?;
    let mut rsp: Vec<(&str, bool)> = Vec::new();
    for name in ["R", "T"] {
        let sh = new_short(name);
        let n0 = c.c2().zcl_rsp.len();
        {
            let w = &mut c.s.w;
            w.coords[k2 as usize].zcl_cmd(&mut w.air, sh, 1, 0x0006, 0x02, &[]);
        }
        let got = move |s: &Sim| s.w.coords[k2 as usize].zcl_rsp[n0..].iter().any(|x| x.src == sh && x.cluster == 0x0006 && x.cmd == Some(0x0B));
        c.run_until(3000.0, 10.0, got)?;
        rsp.push((name, got(&c.s)));
    }
    cx.check(c.now(), "both take a command on the new network and answer it", rsp.iter().all(|x| x.1), &rsp_dict(&rsp));

    // ---- the two are neighbors: each one's link status lists the other
    let link_status_of = |s: &Sim, w: NodeId, since: f64| -> Vec<u16> {
        let mut listed = Vec::new();
        for f in &s.w.air.frames {
            if f.t0 < since || f.who != w || f.ch != NEW_CHANNEL {
                continue;
            }
            let Some(m) = mac_parse(&f.psdu) else { continue };
            let p = &m.payload;
            if m.ty != 1 || m.dpan != Some(NEW_PAN) || p.len() < 8 + 14 + 4 {
                continue;
            }
            let fcf = u16::from_le_bytes([p[0], p[1]]);
            if fcf & 3 != 1 {
                continue;
            }
            if let Some((_, _, _, Some(plain))) = nwk_decrypt(p, &NEW_KEY) {
                if plain[0] == 0x08 {
                    for j in 0..(plain[1] & 0x1F) as usize {
                        let a = u16::from_le_bytes([plain[2 + 3 * j], plain[3 + 3 * j]]);
                        if !listed.contains(&a) {
                            listed.push(a);
                        }
                    }
                }
            }
        }
        listed
    };
    // Link Status every 15 s: one of them may be lost (the stock bulb's
    // radio in BLE mode for its advertising just then, over other starts of
    // the system timer), so up to four periods
    let t0 = c.now();
    let (r, t) = (c.r, c.t);
    let both = |s: &Sim| link_status_of(s, r, t0).contains(&new_short("T")) && link_status_of(s, t, t0).contains(&new_short("R"));
    c.run(35000.0)?;
    let mut waited = 35000.0;
    while !both(&c.s) && waited < 65000.0 {
        c.run(1000.0)?;
        waited += 1000.0;
    }
    let (lr, lt) = (link_status_of(&c.s, c.r, t0), link_status_of(&c.s, c.t, t0));
    let hs = |v: &[u16]| {
        let mut h: Vec<String> = v.iter().map(|x| format!("{x:#x}")).collect();
        h.sort();
        crate::py::list(h.into_iter().map(|x| format!("'{x}'")))
    };
    cx.check(c.now(), "each one's link status lists the other as a neighbor", lr.contains(&new_short("T")) && lt.contains(&new_short("R")),
             &format!("{{'R': {}, 'T': {}}}", hs(&lr), hs(&lt)));

    // ---- on the touchlink network: described alike, and reset is obeyed
    let got = c.scan(NEW_CHANNEL, &["R", "T"], 12)?;
    if cx.check(c.now(), "both answer a scan on the new network's channel", got.len() == 2, "") {
        let (rr, tt) = (gr(&got, "R"), gr(&got, "T"));
        let k = |d: &ScanRsp| (d.g("pan"), d.g("ext_pan"), d.g("channel"), d.g("touchlink_info") & 1);
        cx.check(c.now(), "and describe the new network alike",
                 k(&rr) == k(&tt) && k(&tt) == (NEW_PAN as u64, NEW_EXT_PAN, NEW_CHANNEL as u64, 0),
                 &format!("Rust: {}; stock: {}", rr.describe(), tt.describe()));
        for name in ["R", "T"] {
            let pl = c.transaction.to_le_bytes();
            c.send_acked(name, 0x07, &pl, 200.0)?;
        }
        c.run(5000.0)?;
        let fr = c.find("R", 40)?;
        let ft = c.find("T", 40)?;
        let show = |d: &Option<ScanRsp>| match d {
            None => "None".to_string(),
            Some(d) => format!("'channel {}, info {:#04x}'", d.g("channel"), d.g("touchlink_info")),
        };
        let fresh = |d: &Option<ScanRsp>| d.as_ref().is_some_and(|d| d.g("touchlink_info") & 1 != 0);
        cx.check(c.now(), "on a network without a Trust Center, Reset to Factory New Request is obeyed by both",
                 fresh(&fr) && fresh(&ft), &format!("{{'R': {}, 'T': {}}}", show(&fr), show(&ft)));
    }

    // ---- the Rust bulb starts a network of its own choosing
    let (_d, rsp) = c.ask("R", 0x10, &|cx: &Ctx, rid| cx.open_start(rid), 0x11)?;
    if let Some(r) = &rsp {
        let mut ok = r[4] == 0 && start_t.as_ref().is_some_and(|s| s.len() == r.len());
        let (st, epid, ch, pan) = (r[4], u64::from_le_bytes(r[5..13].try_into().unwrap()), r[14], u16::from_le_bytes([r[15], r[16]]));
        ok = ok && epid != 0 && (11..=26).contains(&ch) && pan != 0 && pan != 0xFFFF;
        let detail = format!("status {st}, extended PAN {epid:#018x}, channel {ch}, PAN {pan:#06x}; response as long as the stock's");
        // is it there? a beacon request on that channel, sent when the air
        // is free (the scripted initiator has no CSMA-CA: one that came
        // 0.03 ms after the end of the bulb's own Link Status found its
        // radio still turning round from sending, over other starts of the
        // system timer), and again up to twice, as a scan would
        c.run(1500.0)?;
        c.set_channel(ch as i64);
        let mut beacons: Vec<(u64, u64, u64)> = Vec::new();
        for _ in 0..3 {
            let n0 = c.s.w.air.frames.len();
            let psdu = with_fcs(&mac_build(3, 0x55, &[0x07], 0xFFFF, 0xFFFF, 2, 0, 0, None, false, false));
            let b0 = (c.now() + 1.0).max(c.s.w.air.air_free_at() + 1.0);
            let b1 = b0 + airtime_us(psdu.len()) / 1000.0;
            c.s.w.air.at(b0, Ev::Node(c.ini.id, StationEv::Transmit { psdu, t0: b0, t1: b1 }));
            c.run(300.0)?;
            for f in &c.s.w.air.frames[n0..] {
                if f.who == c.r && f.ch == ch as i64 && f.psdu[0] & 7 == 0 {
                    if let Some(m) = mac_parse(&f.psdu) {
                        let e = m.payload.get(7..15).map(|b| u64::from_le_bytes(b.try_into().unwrap())).unwrap_or(0);
                        beacons.push((m.span.unwrap_or(0) as u64, m.src.unwrap_or(0), e));
                    }
                }
            }
            if !beacons.is_empty() {
                break;
            }
        }
        cx.check(c.now(), "Network Start Request leaving the choice to the Rust bulb: it answers with a network of its own", ok, &detail);
        let bs: Vec<String> = beacons.iter().map(|(a, b, e)| format!("('{a:#x}', '{b:#x}', '{e:#x}')")).collect();
        cx.check(c.now(), "and is there: its beacon names that network", beacons.contains(&(pan as u64, 0x0005, epid)),
                 &format!("[{}]", bs.join(", ")));
    } else {
        cx.check(c.now(), "Network Start Request leaving the choice to the Rust bulb: it answers with a network of its own", false,
                 "no response");
    }
    Ok(())
}
