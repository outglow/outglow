//! Harness of the BLE conformance tests, a port of emulator/spec_ble/harness.py.
//!
//! A session boots one image with nothing else on the 802.15.4 air and gives
//! the tests an advertising bulb; `connect` makes a connection with the
//! scripted central of the emulator (zbnode's blespec, driven by the same BS
//! commands the Python suite sends over the zbnode protocol). The host side of
//! the central is here: L2CAP recombination, the attribute client, signalling.
//!
//! Nothing here encodes an expectation: verdicts come only from the tests.

use std::any::Any;
use std::collections::HashMap;

use zbnode::blespec;

use crate::air::{Log, NodeId, World};
use crate::coord::{Coord, NWK_KEY};
use crate::frames::Keys;
use crate::image::Image;
use crate::session::{Stop, R};

pub const ADV_AA: u32 = 0x8E89BED6;
pub const CID_ATT: u16 = 4;
pub const CID_SIG: u16 = 5;
pub const CID_SMP: u16 = 6;
pub const ALL_CHANNELS: [u8; 5] = [0xff, 0xff, 0xff, 0xff, 0x1f];

pub fn ll_name(op: u8) -> Option<&'static str> {
    Some(match op {
        0x00 => "LL_CONNECTION_UPDATE_IND",
        0x01 => "LL_CHANNEL_MAP_IND",
        0x02 => "LL_TERMINATE_IND",
        0x03 => "LL_ENC_REQ",
        0x04 => "LL_ENC_RSP",
        0x05 => "LL_START_ENC_REQ",
        0x06 => "LL_START_ENC_RSP",
        0x07 => "LL_UNKNOWN_RSP",
        0x08 => "LL_FEATURE_REQ",
        0x09 => "LL_FEATURE_RSP",
        0x0A => "LL_PAUSE_ENC_REQ",
        0x0B => "LL_PAUSE_ENC_RSP",
        0x0C => "LL_VERSION_IND",
        0x0D => "LL_REJECT_IND",
        0x0E => "LL_PERIPHERAL_FEATURE_REQ",
        0x0F => "LL_CONNECTION_PARAM_REQ",
        0x10 => "LL_CONNECTION_PARAM_RSP",
        0x11 => "LL_REJECT_EXT_IND",
        0x12 => "LL_PING_REQ",
        0x13 => "LL_PING_RSP",
        0x14 => "LL_LENGTH_REQ",
        0x15 => "LL_LENGTH_RSP",
        0x16 => "LL_PHY_REQ",
        0x17 => "LL_PHY_RSP",
        0x18 => "LL_PHY_UPDATE_IND",
        0x19 => "LL_MIN_USED_CHANNELS_IND",
        _ => return None,
    })
}

pub fn hx(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn unhx(s: &str) -> Vec<u8> {
    (0..s.len() / 2).filter_map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()).collect()
}

fn r3(x: f64) -> f64 {
    format!("{x:.3}").parse().unwrap()
}

fn pf(s: &str) -> f64 {
    s.parse().unwrap_or(0.0)
}

fn pi(s: &str) -> i64 {
    s.parse().unwrap_or(0)
}

// ------------------------------------------------------------------ packets
/// An advertising channel packet the bulb sent: start time, channel index, PDU.
#[derive(Clone, Debug)]
pub struct AdvPkt {
    pub ms: f64,
    pub ch: u8,
    pub pdu: Vec<u8>,
    /// ms from the packet's start to the stop of a transmission the firmware stopped
    pub cut: Option<f64>,
}

impl AdvPkt {
    pub fn ty(&self) -> u8 {
        self.pdu[0] & 0x0F
    }
    pub fn rfu(&self) -> u8 {
        (self.pdu[0] >> 4) & 1
    }
    pub fn chsel(&self) -> u8 {
        (self.pdu[0] >> 5) & 1
    }
    pub fn txadd(&self) -> u8 {
        (self.pdu[0] >> 6) & 1
    }
    pub fn rxadd(&self) -> u8 {
        (self.pdu[0] >> 7) & 1
    }
    pub fn length(&self) -> u8 {
        self.pdu[1]
    }
    pub fn payload(&self) -> &[u8] {
        &self.pdu[2..]
    }
    pub fn adva(&self) -> &[u8] {
        self.pdu.get(2..8).unwrap_or(&[])
    }
    pub fn data(&self) -> &[u8] {
        self.pdu.get(8..).unwrap_or(&[])
    }
    pub fn airtime_ms(&self) -> f64 {
        (1 + 4 + 2 + self.pdu[1] as usize + 3) as f64 * 8.0 / 1000.0
    }
    /// `repr()` in Python.
    pub fn repr(&self) -> String {
        format!("<adv {:.3} ch{} {}>", self.ms, self.ch, hx(&self.pdu))
    }
}

/// A data channel PDU of the bulb as the central heard it.
#[derive(Clone, Debug)]
pub struct Pdu {
    pub i: usize,
    pub t_end: f64,
    pub event: i64,
    pub ch: i64,
    pub new: bool,
    pub raw: Vec<u8>,
}

impl Pdu {
    pub fn llid(&self) -> u8 {
        self.raw[0] & 3
    }
    pub fn nesn(&self) -> u8 {
        (self.raw[0] >> 2) & 1
    }
    pub fn sn(&self) -> u8 {
        (self.raw[0] >> 3) & 1
    }
    pub fn md(&self) -> u8 {
        (self.raw[0] >> 4) & 1
    }
    pub fn rfu(&self) -> u8 {
        self.raw[0] >> 5
    }
    pub fn length(&self) -> u8 {
        self.raw[1]
    }
    pub fn payload(&self) -> &[u8] {
        &self.raw[2..]
    }
    pub fn t_start(&self) -> f64 {
        self.t_end - (1 + 4 + 2 + self.raw[1] as usize + 3) as f64 * 8.0 / 1000.0
    }
    pub fn empty(&self) -> bool {
        self.raw[0] & 3 == 1 && self.raw[1] == 0
    }
    pub fn ctrl(&self) -> bool {
        self.raw[0] & 3 == 3
    }
    pub fn opcode(&self) -> Option<u8> {
        if self.raw[0] & 3 == 3 && self.raw[1] != 0 {
            self.raw.get(2).copied()
        } else {
            None
        }
    }
    /// `repr()` in Python.
    pub fn repr(&self) -> String {
        let what = if self.empty() {
            format!("empty {}", hx(self.payload()))
        } else if self.ctrl() {
            let n = match self.opcode() {
                Some(op) => ll_name(op).map(String::from).unwrap_or_else(|| format!("LL {op:#x}")),
                None => "LL None".into(),
            };
            format!("{n} {}", hx(self.payload()))
        } else {
            format!("LLID{} {}", self.llid(), hx(self.payload()))
        };
        format!(
            "<ev {} ch {} NESN{} SN{} MD{}{} {what}>",
            self.event,
            self.ch,
            self.nesn(),
            self.sn(),
            self.md(),
            if self.new { "" } else { " dup" }
        )
    }
}

/// An L2CAP frame recombined from the bulb's LL data PDUs.
#[derive(Clone, Debug)]
pub struct L2Frame {
    pub t: f64,
    pub event: i64,
    pub cid: u16,
    pub data: Vec<u8>,
    pub length: u16,
    pub first: Pdu,
    pub last: Pdu,
    pub note: Option<String>,
}

impl L2Frame {
    pub fn repr(&self) -> String {
        format!("<l2cap ev {} cid {:#x} len {} {}>", self.event, self.cid, self.length, hx(&self.data))
    }
}

/// What the central sent in one packet (a line of BSLOG).
#[derive(Clone, Debug)]
pub struct CentralTx {
    pub ms: f64,
    pub event: i64,
    pub ch: i64,
    pub nesn: u8,
    pub sn: u8,
    pub md: u8,
    pub len: i64,
    pub what: String,
    pub missed: bool,
    pub lost: bool,
}

/// `TX_RE` of harness.py on a BSLOG line without its first two characters:
/// `[   12.345] tx  ev   3 ch  7 -> [NESN1 SN0 MD0 len  0] what`.
fn parse_tx(s: &str) -> Option<CentralTx> {
    let s = s.strip_prefix('[')?;
    let (ms, rest) = s.split_once(']')?;
    let ms: f64 = ms.trim().parse().ok()?;
    let rest = rest.strip_prefix(" tx")?;
    let rest = rest.trim_start().strip_prefix("ev")?;
    let rest = rest.trim_start();
    let (ev, rest) = rest.split_once(' ')?;
    let rest = rest.strip_prefix("ch")?.trim_start();
    let (ch, rest) = rest.split_once(' ')?;
    let rest = rest.strip_prefix("-> [NESN")?;
    let nesn = rest.get(..1)?.parse().ok()?;
    let rest = rest.get(1..)?.strip_prefix(" SN")?;
    let sn = rest.get(..1)?.parse().ok()?;
    let rest = rest.get(1..)?.strip_prefix(" MD")?;
    let md = rest.get(..1)?.parse().ok()?;
    let rest = rest.get(1..)?.strip_prefix(" len")?.trim_start();
    let (len, rest) = rest.split_once(']')?;
    let what = rest.strip_prefix(' ')?.to_string();
    if !ev.chars().all(|c| c.is_ascii_digit()) || !ch.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(CentralTx {
        ms,
        event: ev.parse().ok()?,
        ch: ch.parse().ok()?,
        nesn,
        sn,
        md,
        len: len.parse().ok()?,
        missed: what.contains("MISSED"),
        lost: what.contains("LOST on air"),
        what,
    })
}

/// Channel Selection Algorithm #1 (Vol 6 Part B 4.5.8.2): the data channel of
/// n consecutive events, starting after unmapped channel `last`.
pub fn csa1(hop: i64, chm: &[u8], n: usize, last: i64) -> (Vec<i64>, i64) {
    let used: Vec<i64> = (0..37).filter(|ch| chm[(*ch >> 3) as usize] >> (ch & 7) & 1 != 0).collect();
    let mut out = Vec::new();
    let mut last = last;
    for _ in 0..n {
        last = (last + hop) % 37;
        out.push(if used.contains(&last) { last } else { used[(last as usize) % used.len()] });
    }
    (out, last)
}

/// AD structures (Vol 3 Part C 11): [(type, value)], and a list of syntax problems.
pub fn parse_ad(data: &[u8]) -> (Vec<(u8, Vec<u8>)>, Vec<String>) {
    let (mut out, mut bad, mut i) = (Vec::new(), Vec::new(), 0usize);
    while i < data.len() {
        let ln = data[i] as usize;
        if ln == 0 {
            if data[i..].iter().any(|b| *b != 0) {
                bad.push(format!("non-zero octets after a zero length at offset {i}"));
            }
            break;
        }
        if i + 1 + ln > data.len() {
            bad.push(format!("structure at offset {i} (length {ln}) runs past the end of the data"));
            break;
        }
        out.push((data[i + 1], data[i + 2..i + 1 + ln].to_vec()));
        i += 1 + ln;
    }
    (out, bad)
}

// ------------------------------------------------------------------ link state
/// The central's state (BSSTATE), kept as text with typed accessors.
#[derive(Clone, Debug, Default)]
pub struct LinkSt(pub HashMap<String, String>);

impl LinkSt {
    pub fn s(&self, k: &str) -> &str {
        self.0.get(k).map(|s| s.as_str()).unwrap_or("")
    }
    pub fn i(&self, k: &str) -> Option<i64> {
        self.0.get(k).map(|v| pi(v))
    }
    pub fn f(&self, k: &str) -> Option<f64> {
        self.0.get(k).map(|v| pf(v))
    }
    pub fn state(&self) -> &str {
        self.s("state")
    }
    pub fn event(&self) -> i64 {
        self.i("event").unwrap_or(-1)
    }
    pub fn heard(&self) -> bool {
        self.s("heard") == "true"
    }
}

/// How the host side of the central answers a Connection Parameter Update Request.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParamReq {
    Reject,
    Accept,
    Ignore,
}

/// Options of a connection: the keys of BSCENTRAL plus the host's behaviour.
#[derive(Clone, Debug)]
pub struct LinkOpts {
    pub kv: Vec<(String, String)>,
    pub param_req: ParamReq,
    pub confirm: bool,
}

impl Default for LinkOpts {
    fn default() -> Self {
        LinkOpts { kv: Vec::new(), param_req: ParamReq::Reject, confirm: true }
    }
}

impl LinkOpts {
    pub fn new() -> LinkOpts {
        LinkOpts::default()
    }
    /// A BSCENTRAL option (interval, latency, timeout, hop, winsize, winoffset,
    /// anchor, maxexch, after, scan, version, features, llauto, noack, sca, lead, llrules).
    pub fn set(mut self, k: &str, v: impl ToString) -> Self {
        let v = v.to_string();
        match self.kv.iter_mut().find(|(x, _)| x == k) {
            Some(e) => e.1 = v,
            None => self.kv.push((k.to_string(), v)),
        }
        self
    }
    pub fn chm(self, chm: &[u8]) -> Self {
        self.set("chm", hx(chm))
    }
    pub fn aa(self, aa: u32) -> Self {
        self.set("aa", format!("{aa:08x}"))
    }
    pub fn param_req(mut self, p: ParamReq) -> Self {
        self.param_req = p;
        self
    }
    pub fn no_confirm(mut self) -> Self {
        self.confirm = false;
        self
    }
    fn get(&self, k: &str) -> Option<&str> {
        self.kv.iter().find(|(x, _)| x == k).map(|(_, v)| v.as_str())
    }
}

/// A connection as the scripted central sees it.
pub struct Link {
    pub opts: LinkOpts,
    pub pdus: Vec<Pdu>,
    pub frames: Vec<L2Frame>,
    pub ctrl: Vec<Pdu>,
    pub problems: Vec<String>,
    pub central_tx: Vec<CentralTx>,
    log_n: usize,
    pub param_reqs: Vec<L2Frame>,
    pub server_reqs: Vec<L2Frame>,
    pub t_created: f64,
    asm: Option<(Vec<u8>, Pdu, Pdu)>,
    pub mtu: usize,
    pub st: LinkSt,
    /// `connect_rx_mtu`: the central answers the server's ATT_EXCHANGE_MTU_REQ with this
    /// Client Rx MTU (and with `hold` only when the test releases it)
    pub rx_mtu: Option<(u16, bool)>,
    pub mtu_held: bool,
}

impl Link {
    pub fn interval_ms(&self) -> f64 {
        self.st.f("interval_ms").filter(|v| *v != 0.0).unwrap_or_else(|| {
            self.opts.get("interval").map(pf).unwrap_or(24.0) * 1.25
        })
    }
    pub fn timeout(&self) -> i64 {
        self.opts.get("timeout").map(pi).unwrap_or(400)
    }
}

// ------------------------------------------------------------------ session
pub struct BleSession {
    pub w: World,
    spec: blespec::Spec,
    pub image: String,
    pub link: Option<Link>,
    adva: Option<(Vec<u8>, u8)>,
    notes: HashMap<String, Box<dyn Any>>,
    rf_t: f64,
    cmds: Vec<(f64, u8, u8)>,
    sent: Vec<(f64, u8, Vec<u8>)>,
    pkts: Vec<AdvPkt>,
    stx_i: usize,
}

fn emu(e: String) -> Stop {
    Stop::Error(e)
}

/// The defaults of a Link (`Link.__init__`), in their order.
fn link_defaults() -> LinkOpts {
    LinkOpts::new()
        .set("interval", 24)
        .set("latency", 0)
        .set("timeout", 400)
        .set("hop", 7)
        .set("winsize", 2)
        .set("winoffset", 0)
        .set("anchor", 0.3)
        .chm(&ALL_CHANNELS)
}

impl BleSession {
    pub fn new(image: &Image, cpi: i64, log: Option<Log>, boot_ms: f64) -> R<BleSession> {
        let node = image.node(cpi).map_err(emu)?;
        // no Trust Center: the bulb is alone on the 802.15.4 air (an empty Medium in Python)
        let coord = Coord::new(11, NWK_KEY);
        let mut w = World::new(node, coord, Keys::default(), log);
        w.air.nodes.retain(|n| *n != NodeId::Coord);
        let mut s = BleSession {
            w,
            spec: blespec::Spec::default(),
            image: image.label.clone(),
            link: None,
            adva: None,
            notes: HashMap::new(),
            rf_t: 0.0,
            cmds: Vec::new(),
            sent: Vec::new(),
            pkts: Vec::new(),
            stx_i: 0,
        };
        s.bs("BSDETACH")?;
        s.run(boot_ms)?;
        Ok(s)
    }

    // ---- the BS commands (zbnode protocol, in-process)
    fn bs_raw(&mut self, line: &str) -> String {
        let f: Vec<String> = line.split_whitespace().map(String::from).collect();
        let mut out = String::new();
        let node = &mut self.w.air.dut;
        self.spec.command(&f, &mut out, &mut node.m, &node.ble_radio);
        out
    }

    /// A command answered with one line; its fields. ERROR stops the test.
    pub fn bs(&mut self, line: &str) -> R<Vec<String>> {
        let out = self.bs_raw(line);
        let first = out.lines().next().unwrap_or("").to_string();
        if first.starts_with("ERROR") {
            return Err(Stop::Error(format!("RuntimeError: {first}")));
        }
        Ok(first.split_whitespace().map(String::from).collect())
    }

    /// A command answered with lines up to END.
    pub fn bs_lines(&mut self, line: &str) -> R<Vec<String>> {
        let out = self.bs_raw(line);
        let mut v = Vec::new();
        for l in out.lines() {
            if l == "END" {
                return Ok(v);
            }
            if l.starts_with("ERROR") {
                return Err(Stop::Error(format!("RuntimeError: {l}")));
            }
            v.push(l.to_string());
        }
        Err(Stop::Error(format!("RuntimeError: {line}: no END")))
    }

    // ---- scratch space shared by the tests of a session
    pub fn note<T: Clone + 'static>(&self, k: &str) -> Option<T> {
        self.notes.get(k).and_then(|v| v.downcast_ref::<T>()).cloned()
    }
    pub fn set_note<T: 'static>(&mut self, k: &str, v: T) {
        self.notes.insert(k.to_string(), Box::new(v));
    }

    // ---- time
    pub fn now(&self) -> f64 {
        self.w.now()
    }

    pub fn run(&mut self, ms: f64) -> R<()> {
        let t = self.w.now() + ms;
        self.w.run_ms(t).map_err(emu)
    }

    fn run_to(&mut self, t: f64) -> R<()> {
        self.w.run_ms(t).map_err(emu)
    }

    /// Runs until `cond` gives Some (checked every `step` ms) or `max_ms` passed; the last value.
    pub fn until<T>(&mut self, max_ms: f64, step: f64, mut cond: impl FnMut(&mut BleSession) -> R<Option<T>>) -> R<Option<T>> {
        let end = self.now() + max_ms;
        loop {
            let v = cond(self)?;
            if v.is_some() || self.now() >= end {
                return Ok(v);
            }
            let t = end.min(self.now() + step);
            self.run_to(t)?;
        }
    }

    pub fn resets(&self) -> u32 {
        self.w.resets
    }

    // ---- what the radio did
    /// Keeps the whole radio log of the node here, fetched in increments (`_rf_sync`).
    fn rf_sync(&mut self) {
        let from = self.rf_t;
        let (cmds, sent): (Vec<(f64, u8, u8)>, Vec<(f64, u8, Vec<u8>)>) = {
            let br = self.w.air.dut.ble_radio.b();
            (
                br.cmds.iter().filter(|c| r3(c.ms) >= from).map(|c| (r3(c.ms), c.cmd, c.ch)).collect(),
                br.adv_tx.iter().filter(|a| r3(a.ms) >= from).map(|a| (r3(a.ms), a.ch, a.pdu.clone())).collect(),
            )
        };
        let have_c = self.cmds.iter().rev().take_while(|c| c.0 == self.rf_t).count();
        let have_s = self.sent.iter().rev().take_while(|p| p.0 == self.rf_t).count();
        let skip_c = cmds.iter().take(have_c).filter(|c| c.0 == self.rf_t).count();
        self.cmds.extend(cmds[skip_c..].iter().cloned());
        let skip_s = sent.iter().take(have_s).filter(|p| p.0 == self.rf_t).count();
        let new: Vec<(f64, u8, Vec<u8>)> = sent[skip_s..].to_vec();
        self.sent.extend(new.iter().cloned());
        let mut t = self.rf_t;
        for c in &cmds {
            t = t.max(c.0);
        }
        for p in &sent {
            t = t.max(p.0);
        }
        self.rf_t = t;
        // A transmission the firmware stopped (0x80) before the packet was out is marked (`cut`):
        // every send command (0x85/0x87) gives one logged packet, in order.
        let stx: Vec<f64> = self.cmds.iter().filter(|c| c.1 == 0x85 || c.1 == 0x87).map(|c| c.0).collect();
        for (ms, ch, pdu) in new {
            let mut p = AdvPkt { ms, ch, pdu, cut: None };
            if self.stx_i < stx.len() && stx[self.stx_i] <= ms + 0.002 {
                let t_cmd = stx[self.stx_i];
                self.stx_i += 1;
                let end = ms + p.airtime_ms();
                if let Some(t) = self.cmds.iter().find(|c| c.1 == 0x80 && t_cmd < c.0 && c.0 < end).map(|c| c.0) {
                    p.cut = Some(t - ms);
                }
            }
            self.pkts.push(p);
        }
    }

    /// The firmware's commands to the BLE state machine of the radio (ms, command, channel)
    /// and every packet it sent outside a connection, from `since` on.
    pub fn rf(&mut self, since: f64) -> (Vec<(f64, u8, u8)>, Vec<AdvPkt>) {
        self.rf_sync();
        (
            self.cmds.iter().filter(|c| c.0 >= since).cloned().collect(),
            self.pkts.iter().filter(|p| p.ms >= since).cloned().collect(),
        )
    }

    /// Advertising channel packets (channels 37-39) from `since` on; stopped ones only with `cut`.
    pub fn adv(&mut self, since: f64, cut: bool) -> Vec<AdvPkt> {
        self.rf(since).1.into_iter().filter(|p| p.ch >= 37 && p.pdu.len() >= 2 && (cut || p.cut.is_none())).collect()
    }

    /// (ms, channel) of every connection event receive window the firmware opened.
    pub fn rx_windows(&mut self, since: f64) -> Vec<(f64, u8)> {
        self.rf(since).0.into_iter().filter(|c| c.1 == 0x82).map(|c| (c.0, c.2)).collect()
    }

    /// Runs until n packets of the type were sent after `since` (default: now); the packets.
    pub fn wait_adv(&mut self, n: usize, timeout_ms: f64, since: Option<f64>, typ: u8) -> R<Vec<AdvPkt>> {
        let t0 = since.unwrap_or_else(|| self.now());
        let got = |s: &mut BleSession| -> Vec<AdvPkt> { s.adv(t0, false).into_iter().filter(|p| p.ty() == typ).collect() };
        self.until(timeout_ms, 20.0, |s| Ok(if got(s).len() >= n { Some(()) } else { None }))?;
        Ok(got(self))
    }

    /// (AdvA as sent, TxAdd) from the bulb's ADV_IND.
    pub fn address(&mut self) -> R<(Vec<u8>, u8)> {
        if self.adva.is_none() {
            let pk = self.wait_adv(1, 5000.0, Some(0.0), 0)?;
            let Some(p) = pk.first() else { return Err(Stop::Inconclusive("the bulb sent no ADV_IND".into())) };
            self.adva = Some((p.adva().to_vec(), p.txadd()));
        }
        Ok(self.adva.clone().unwrap())
    }

    /// Groups ADV_IND packets into advertising events: a new event starts when the channel index does not rise.
    pub fn events(pkts: &[AdvPkt], typ: u8) -> Vec<Vec<AdvPkt>> {
        let mut ev: Vec<Vec<AdvPkt>> = Vec::new();
        for p in pkts {
            if p.ty() != typ {
                continue;
            }
            match ev.last_mut() {
                Some(e) if p.ch > e.last().unwrap().ch => e.push(p.clone()),
                _ => ev.push(vec![p.clone()]),
            }
        }
        ev
    }

    // ---- scripted answers to advertising
    /// The next n ADV_IND (on channel ch; -1 any) are answered T_IFS later with this PDU.
    pub fn adv_reply(&mut self, pdu: &[u8], n: i64, ch: i64) -> R<()> {
        self.bs(&format!("BSADVREPLY {n} {ch} {}", hx(pdu))).map(|_| ())
    }

    pub fn adv_reply_clear(&mut self) -> R<()> {
        self.bs("BSADVREPLY 0 -1 0000").map(|_| ())
    }

    /// Scripted answers that went out: (ms of the ADV_IND they answer, channel, pdu).
    pub fn adv_sent(&mut self, start: usize) -> R<Vec<(f64, i64, Vec<u8>)>> {
        Ok(self
            .bs_lines(&format!("BSADVSENT {start}"))?
            .iter()
            .map(|ln| {
                let f: Vec<&str> = ln.split_whitespace().collect();
                (pf(f[1]), pi(f[2]), unhx(f[3]))
            })
            .collect())
    }

    pub fn scan_req(&mut self, adva: Option<&[u8]>, rxadd: Option<u8>, scana: &[u8], txadd: u8) -> R<Vec<u8>> {
        let (a, t) = self.address()?;
        let mut v = vec![0x03 | txadd << 6 | rxadd.unwrap_or(t) << 7, 12];
        v.extend_from_slice(scana);
        v.extend_from_slice(adva.unwrap_or(&a));
        Ok(v)
    }

    /// The default SCAN_REQ: ScanA 11:22:33:44:55:66 (public).
    pub fn scan_req_default(&mut self) -> R<Vec<u8>> {
        self.scan_req(None, None, &[0x66, 0x55, 0x44, 0x33, 0x22, 0x11], 0)
    }

    // ---- connections
    /// A central connects at the next ADV_IND. With wait, runs until the bulb
    /// answered in a connection event; Inconclusive if it never does.
    pub fn connect(&mut self, o: LinkOpts) -> R<()> {
        self.connect_with(o, true, 8000.0)
    }

    pub fn connect_with(&mut self, o: LinkOpts, wait: bool, timeout_ms: f64) -> R<()> {
        if self.link.is_some() {
            self.drop_link(6000.0)?;
        }
        let mut opts = link_defaults();
        for (k, v) in &o.kv {
            opts = opts.set(k, v);
        }
        opts.param_req = o.param_req;
        opts.confirm = o.confirm;
        let line = format!("BSCENTRAL {}", opts.kv.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(" "));
        let mut st = LinkSt::default();
        st.0.insert("state".into(), "scan".into());
        st.0.insert("event".into(), "-1".into());
        st.0.insert("heard".into(), "false".into());
        self.link = Some(Link {
            opts,
            pdus: Vec::new(),
            frames: Vec::new(),
            ctrl: Vec::new(),
            problems: Vec::new(),
            central_tx: Vec::new(),
            log_n: 0,
            param_reqs: Vec::new(),
            server_reqs: Vec::new(),
            t_created: self.now(),
            asm: None,
            mtu: 23,
            st,
            rx_mtu: None,
            mtu_held: false,
        });
        self.bs(&line)?;
        if wait {
            let ok = self.until(timeout_ms, 5.0, |s| {
                let st = s.poll()?;
                Ok(if st.state() == "connected" && st.heard() { Some(()) } else { None })
            })?;
            if ok.is_none() {
                let lk = self.lk();
                return Err(Stop::Inconclusive(format!(
                    "no connection: central state {}, {} PDUs heard",
                    lk.st.state(),
                    lk.pdus.len()
                )));
            }
        }
        Ok(())
    }

    /// Ends the current connection (LL_TERMINATE_IND from the central) and waits
    /// for advertising; SessionLost if the bulb does not advertise again.
    pub fn drop_link(&mut self, wait_adv_ms: f64) -> R<()> {
        if self.link.is_some() {
            let st = self.poll()?;
            if st.state() == "connected" && st.heard() {
                self.terminate(0x13)?;
                self.until(3000.0, 10.0, |s| Ok(if s.poll()?.state() != "connected" { Some(()) } else { None }))?;
            }
        }
        self.link = None;
        self.bs("BSAIR off")?;
        self.bs("BSDETACH")?;
        let t0 = self.now();
        if self.wait_adv(1, wait_adv_ms, Some(t0), 0)?.is_empty() {
            return Err(Stop::SessionLost("the bulb does not advertise after the connection ended".into()));
        }
        Ok(())
    }

    /// Answers a held ATT_EXCHANGE_MTU_REQ of the server (`lk.mtu_release()`).
    pub fn mtu_release(&mut self) -> R<()> {
        if self.lk().mtu_held {
            self.lk_mut().mtu_held = false;
            let rx = self.lk().rx_mtu.map(|x| x.0).unwrap_or(23);
            self.l2cap(CID_ATT, &[0x03, rx as u8, (rx >> 8) as u8])?;
        }
        Ok(())
    }

    pub fn lk(&self) -> &Link {
        self.link.as_ref().expect("no link (connect first)")
    }

    pub fn lk_mut(&mut self) -> &mut Link {
        self.link.as_mut().expect("no link (connect first)")
    }

    // ---- link state
    /// Fetches what happened since the last call; returns the state.
    pub fn poll(&mut self) -> R<LinkSt> {
        let f = self.bs("BSSTATE")?;
        let mut st = LinkSt::default();
        for x in &f[1..] {
            if let Some((k, v)) = x.split_once('=') {
                st.0.insert(k.to_string(), v.to_string());
            }
        }
        self.lk_mut().st = st.clone();
        let n = st.i("pdus");
        if let Some(n) = n {
            let have = self.lk().pdus.len();
            if n as usize > have {
                for ln in self.bs_lines(&format!("BSPDUS {have}"))? {
                    let x: Vec<&str> = ln.split_whitespace().collect();
                    let i = self.lk().pdus.len();
                    let p = Pdu { i, t_end: pf(x[1]), event: pi(x[2]), ch: pi(x[3]), new: x[4] == "1", raw: unhx(x[5]) };
                    self.lk_mut().pdus.push(p.clone());
                    self.rx(p)?;
                }
            }
        }
        Ok(st)
    }

    pub fn state(&mut self) -> R<String> {
        Ok(self.poll()?.state().to_string())
    }

    pub fn connected(&mut self) -> R<bool> {
        Ok(self.poll()?.state() == "connected")
    }

    pub fn event(&mut self) -> R<i64> {
        Ok(self.poll()?.event())
    }

    pub fn interval_ms(&self) -> f64 {
        self.lk().interval_ms()
    }

    /// What the central sent.
    pub fn central_log(&mut self) -> R<Vec<CentralTx>> {
        let st = self.poll()?;
        if let Some(n) = st.i("log") {
            if n > 0 && n as usize > self.lk().log_n {
                let start = self.lk().log_n;
                for ln in self.bs_lines(&format!("BSLOG {start}"))? {
                    let lk = self.lk_mut();
                    lk.log_n += 1;
                    if let Some(t) = ln.get(2..).and_then(parse_tx) {
                        lk.central_tx.push(t);
                    }
                }
            }
        }
        Ok(self.lk().central_tx.clone())
    }

    /// (event, ms): the start of the central's first packet in each event it transmitted in.
    pub fn sent(&mut self) -> R<Vec<(i64, f64)>> {
        Ok(self
            .bs_lines("BSSENT 0")?
            .iter()
            .map(|ln| {
                let f: Vec<&str> = ln.split_whitespace().collect();
                (pi(f[1]), pf(f[2]))
            })
            .collect())
    }

    /// Central packets the radio model did not deliver: (ms, channel, why).
    pub fn misses(&mut self, start: usize) -> R<Vec<(f64, i64, String)>> {
        Ok(self
            .bs_lines(&format!("BSMISSES {start}"))?
            .iter()
            .map(|ln| {
                let f: Vec<&str> = ln.splitn(4, char::is_whitespace).filter(|s| !s.is_empty()).collect();
                (pf(f[1]), pi(f[2]), f.get(3).map(|s| s.to_string()).unwrap_or_default())
            })
            .collect())
    }

    /// The ones of `events` in which the bulb did not hear the central
    /// because its radio was with 802.15.4 (the Zigbee side was sending, or
    /// the main loop was away for the whole event): a combined chip's
    /// coexistence, which costs a connection event now and then by design
    /// (`ble::ll`, LEAD), not anything the link layer chose. A test that
    /// needs the bulb in one particular event tries again when it lost that
    /// event this way.
    pub fn lost_to_154(&mut self, events: &[i64]) -> R<Vec<i64>> {
        let misses = self.misses(0)?;
        let mut v: Vec<i64> = self
            .central_log()?
            .iter()
            .filter(|t| t.missed && events.contains(&t.event))
            .filter(|t| misses.iter().any(|m| (m.0 - t.ms).abs() < 0.01 && m.2.contains("802.15.4")))
            .map(|t| t.event)
            .collect();
        v.dedup();
        Ok(v)
    }

    pub fn lost(&mut self) -> R<Vec<(f64, i64, i64, Vec<u8>)>> {
        Ok(self
            .bs_lines("BSLOST 0")?
            .iter()
            .map(|ln| {
                let f: Vec<&str> = ln.split_whitespace().collect();
                (pf(f[1]), pi(f[2]), pi(f[3]), unhx(f[4]))
            })
            .collect())
    }

    // ---- the host side of the central
    fn rx(&mut self, p: Pdu) -> R<()> {
        if !p.new || p.length() == 0 {
            return Ok(());
        }
        let lk = self.lk_mut();
        match p.llid() {
            3 => {
                lk.ctrl.push(p);
                return Ok(());
            }
            2 => {
                if let Some(a) = &lk.asm {
                    let n = a.0.len();
                    lk.problems.push(format!(
                        "ev {}: a new L2CAP frame starts before the previous one was complete ({n} octets of it received)",
                        p.event
                    ));
                }
                lk.asm = Some((p.payload().to_vec(), p.clone(), p.clone()));
            }
            1 => {
                match &mut lk.asm {
                    None => {
                        lk.problems.push(format!("ev {}: continuation fragment without a start: {}", p.event, hx(p.payload())));
                        return Ok(());
                    }
                    Some(a) => {
                        a.0.extend_from_slice(p.payload());
                        a.2 = p.clone();
                    }
                }
            }
            _ => {
                lk.problems.push(format!("ev {}: LLID 0 (reserved): {}", p.event, hx(&p.raw)));
                return Ok(());
            }
        }
        let buf = lk.asm.as_ref().unwrap().0.clone();
        if buf.len() < 4 {
            return Ok(());
        }
        let ln = u16::from_le_bytes([buf[0], buf[1]]);
        let cid = u16::from_le_bytes([buf[2], buf[3]]);
        if buf.len() < 4 + ln as usize {
            return Ok(());
        }
        let mut note = None;
        if buf.len() > 4 + ln as usize {
            let n = format!("{} octets more than the length field says", buf.len() - 4 - ln as usize);
            lk.problems.push(format!("ev {}: L2CAP frame on CID {cid:#x}: {n}", p.event));
            note = Some(n);
        }
        let (_, first, _) = lk.asm.take().unwrap();
        let fr = L2Frame {
            t: p.t_end,
            event: p.event,
            cid,
            data: buf[4..4 + ln as usize].to_vec(),
            length: ln,
            first,
            last: p,
            note,
        };
        lk.frames.push(fr.clone());
        self.host(fr)
    }

    /// What a host owes the peer without being asked by a test.
    fn host(&mut self, fr: L2Frame) -> R<()> {
        let d = fr.data.clone();
        if let Some((rx_mtu, hold)) = self.lk().rx_mtu {
            if fr.cid == CID_ATT && d.first() == Some(&0x02) {
                self.lk_mut().server_reqs.push(fr);
                if hold {
                    self.lk_mut().mtu_held = true;
                } else {
                    self.l2cap(CID_ATT, &[0x03, rx_mtu as u8, (rx_mtu >> 8) as u8])?;
                }
                return Ok(());
            }
        }
        if fr.cid == CID_ATT && !d.is_empty() {
            if d[0] == 0x1D && self.lk().opts.confirm {
                self.l2cap(CID_ATT, &[0x1e])?;
            } else if d[0] == 0x02 {
                self.lk_mut().server_reqs.push(fr);
                self.l2cap(CID_ATT, &[0x03, 23, 0])?;
            } else if [0x04, 0x06, 0x08, 0x0A, 0x0C, 0x0E, 0x10, 0x12, 0x16, 0x18, 0x20, 0x22].contains(&d[0]) {
                self.lk_mut().server_reqs.push(fr);
                self.l2cap(CID_ATT, &[0x01, d[0], 0, 0, 0x06])?;
            }
        } else if fr.cid == CID_SIG && d.len() >= 4 && d[0] == 0x12 {
            self.lk_mut().param_reqs.push(fr);
            match self.lk().opts.param_req {
                ParamReq::Reject => self.l2cap(CID_SIG, &[0x13, d[1], 2, 0, 1, 0])?,
                ParamReq::Accept => self.l2cap(CID_SIG, &[0x13, d[1], 2, 0, 0, 0])?,
                ParamReq::Ignore => {}
            }
        }
        Ok(())
    }

    // ---- sending
    /// Queues an LL control PDU (opcode + CtrData).
    pub fn ll(&mut self, payload: &[u8]) -> R<()> {
        self.bs(&format!("BSLL {}", hx(payload))).map(|_| ())
    }

    /// Queues one LL data PDU as it is.
    pub fn data(&mut self, llid: u8, payload: &[u8]) -> R<()> {
        self.bs(&format!("BSDATA {llid} {}", hx(payload))).map(|_| ())
    }

    /// Queues an L2CAP frame (cut into 27-octet LL PDUs by the central).
    pub fn l2cap(&mut self, cid: u16, payload: &[u8]) -> R<()> {
        self.bs(&format!("BSL2CAP {cid} {}", hx(payload))).map(|_| ())
    }

    /// LL_CONNECTION_UPDATE_IND; the central moves at the instant.
    #[allow(clippy::too_many_arguments)]
    pub fn update(&mut self, interval: i64, instant: i64, winsize: i64, winoffset: i64, latency: i64,
                  timeout: Option<i64>) -> R<()> {
        let timeout = timeout.unwrap_or_else(|| self.lk().timeout());
        self.bs(&format!("BSUPDATE {winsize} {winoffset} {interval} {latency} {timeout} {}", instant & 0xFFFF)).map(|_| ())
    }

    /// LL_CHANNEL_MAP_IND; the central uses the map from the instant on.
    pub fn channel_map(&mut self, chm: &[u8], instant: i64) -> R<()> {
        self.bs(&format!("BSCHM {} {}", hx(chm), instant & 0xFFFF)).map(|_| ())
    }

    pub fn terminate(&mut self, reason: u8) -> R<()> {
        self.bs(&format!("BSTERM {reason}")).map(|_| ())
    }

    /// The central sends nothing from this event on (None: from the next one).
    pub fn silent(&mut self, event: Option<i64>) -> R<()> {
        let e = match event {
            Some(e) => e,
            None => self.event()? + 1,
        };
        self.bs(&format!("BSSILENT {e}")).map(|_| ())
    }

    pub fn silent_off(&mut self) -> R<()> {
        self.bs("BSSILENT off").map(|_| ())
    }

    /// The central loses the bulb's first non-empty PDU of that event.
    pub fn drop_event(&mut self, event: i64) -> R<()> {
        self.bs(&format!("BSDROP {event}")).map(|_| ())
    }

    /// The air between the two: lossk / lossh (millionths), ppm, jitter (us), blackout (ms).
    pub fn air(&mut self, kv: &[(&str, String)]) -> R<()> {
        let t = kv.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(" ");
        self.bs(&format!("BSAIR {t}")).map(|_| ())
    }

    pub fn air_off(&mut self) -> R<()> {
        self.bs("BSAIR off").map(|_| ())
    }

    // ---- waiting on the link
    pub fn link_run(&mut self, ms: f64) -> R<LinkSt> {
        self.run(ms)?;
        self.poll()
    }

    /// `Link.until`: polls, then evaluates cond.
    pub fn link_until<T>(&mut self, max_ms: f64, step: Option<f64>, mut cond: impl FnMut(&mut BleSession) -> R<Option<T>>)
                         -> R<Option<T>> {
        let step = step.unwrap_or_else(|| 1.0f64.max(5.0f64.min(self.interval_ms() / 2.0)));
        self.until(max_ms, step, |s| {
            s.poll()?;
            cond(s)
        })
    }

    /// Runs until the central has started event k, and on to the middle of
    /// that event's interval (`between_events`): what a test queues then goes
    /// out in event k + 1.
    pub fn wait_event(&mut self, k: i64, max_ms: Option<f64>) -> R<i64> {
        let ev = self.event()?;
        let iv = self.interval_ms();
        let max_ms = max_ms.filter(|m| *m != 0.0).unwrap_or(((k - ev).abs() + 4) as f64 * iv * 2.0 + 100.0);
        let step = 0.5f64.max(5.0f64.min(iv / 3.0));
        self.link_until(max_ms, Some(step), |s| Ok(if s.lk().st.event() >= k { Some(()) } else { None }))?;
        self.between_events()?;
        Ok(self.lk().st.event())
    }

    /// Runs on to half an interval after the start of the central's latest
    /// event, when that event's exchange is over and the next one is still
    /// far. The event counter moves when the central sends the event's first
    /// packet; a test that polls for it at a fixed step comes back anywhere
    /// in the interval, on some code layouts of the bulb (where its
    /// advertising, and so the connection, fell) between the central's first
    /// packet and the bulb's answer. What it queued there went out in the
    /// same event, after the answer, not in the next one
    /// (ll_conn_more_data: "sent 0 of the 4 PDUs", the PING_REQs all in
    /// event k; ll_ctrl_channel_map_before_instant: the map PDU in event 9,
    /// not 10, on CI's TW image).
    pub fn between_events(&mut self) -> R<()> {
        let st = self.poll()?;
        let (Some(n), Some(now)) = (st.i("sent"), st.f("now")) else { return Ok(()) };
        if n <= 0 {
            return Ok(());
        }
        let last = self.bs_lines(&format!("BSSENT {}", n - 1))?;
        let Some((k, t)) = last.last().map(|ln| {
            let f: Vec<&str> = ln.split_whitespace().collect();
            (pi(f[1]), pf(f[2]))
        }) else {
            return Ok(());
        };
        // (times of the bulb's clock, as BSSTATE's now: it starts again at a reset)
        let left = t + self.interval_ms() / 2.0 - now;
        if k == st.event() && left > 0.0 {
            self.run(left)?;
            self.poll()?;
        }
        Ok(())
    }

    pub fn run_events(&mut self, n: i64) -> R<i64> {
        let e = self.event()?;
        self.wait_event(e + n, None)
    }

    /// The first LL control PDU of the bulb with one of the opcodes after index `start` in `ctrl`.
    pub fn ll_wait(&mut self, ops: &[u8], timeout_ms: f64, start: Option<usize>) -> R<Option<Pdu>> {
        let n0 = start.unwrap_or_else(|| self.lk().ctrl.len());
        let ops = ops.to_vec();
        self.link_until(timeout_ms, None, move |s| {
            Ok(s.lk().ctrl.get(n0..).unwrap_or(&[]).iter().find(|p| p.opcode().is_some_and(|o| ops.contains(&o))).cloned())
        })
    }

    /// Sends an LL control PDU; the first of the bulb's control PDUs with one of
    /// the opcodes, and all its control PDUs in that time.
    pub fn ll_req(&mut self, payload: &[u8], ops: &[u8], timeout_ms: f64) -> R<(Option<Pdu>, Vec<Pdu>)> {
        self.poll()?;
        let n0 = self.lk().ctrl.len();
        self.ll(payload)?;
        let got = self.ll_wait(ops, timeout_ms, Some(n0))?;
        Ok((got, self.lk().ctrl[n0..].to_vec()))
    }

    pub fn frame_wait(&mut self, cid: u16, pred: impl Fn(&L2Frame) -> bool, timeout_ms: f64, start: Option<usize>)
                      -> R<Option<L2Frame>> {
        let n0 = start.unwrap_or_else(|| self.lk().frames.len());
        self.link_until(timeout_ms, None, move |s| {
            Ok(s.lk().frames.get(n0..).unwrap_or(&[]).iter().find(|f| f.cid == cid && pred(f)).cloned())
        })
    }

    /// Sends a signalling packet; the first signalling frame of the bulb afterwards
    /// that is not its own Connection Parameter Update Request, or None.
    pub fn sig(&mut self, payload: &[u8], timeout_ms: f64) -> R<Option<L2Frame>> {
        self.poll()?;
        let n0 = self.lk().frames.len();
        self.l2cap(CID_SIG, payload)?;
        self.frame_wait(CID_SIG, |f| f.data.first() != Some(&0x12), timeout_ms, Some(n0))
    }

    /// A response or an error; not a notification, an indication or a request of the server.
    pub fn is_rsp(d: &[u8]) -> bool {
        !d.is_empty() && (d[0] == 0x01 || (d[0] & 1 != 0 && d[0] != 0x1B && d[0] != 0x1D))
    }

    /// Sends an attribute PDU; the next response of the server, or None within the timeout.
    pub fn att(&mut self, pdu: &[u8]) -> R<Option<Vec<u8>>> {
        self.att_t(pdu, 30000.0)
    }

    pub fn att_t(&mut self, pdu: &[u8], timeout_ms: f64) -> R<Option<Vec<u8>>> {
        self.poll()?;
        let n0 = self.lk().frames.len();
        self.l2cap(CID_ATT, pdu)?;
        let fr = self.frame_wait(CID_ATT, |f| BleSession::is_rsp(&f.data), timeout_ms, Some(n0))?;
        Ok(fr.map(|f| f.data))
    }

    /// Sends an attribute PDU that must not be answered; the attribute frames (other
    /// than notifications) that came in the wait.
    pub fn att_quiet(&mut self, pdu: &[u8], wait_ms: f64) -> R<Vec<L2Frame>> {
        self.poll()?;
        let n0 = self.lk().frames.len();
        self.l2cap(CID_ATT, pdu)?;
        self.link_run(wait_ms)?;
        Ok(self.lk().frames[n0..].iter().filter(|f| f.cid == CID_ATT && BleSession::is_rsp(&f.data)).cloned().collect())
    }

    pub fn notifications(&self, start: usize) -> Vec<L2Frame> {
        self.lk()
            .frames
            .get(start..)
            .unwrap_or(&[])
            .iter()
            .filter(|f| f.cid == CID_ATT && matches!(f.data.first(), Some(0x1b) | Some(0x1d)))
            .cloned()
            .collect()
    }

    /// True when the bulb still answers in connection events.
    pub fn alive(&mut self, timeout_ms: f64) -> R<bool> {
        let n = self.lk().pdus.len();
        let got = self.link_until(timeout_ms, None, |s| Ok(if s.lk().pdus.len() > n { Some(()) } else { None }))?;
        Ok(got.is_some() && self.lk().st.state() == "connected")
    }
}

// ------------------------------------------------------------------ attribute client helpers
/// (request opcode, handle, error code) of an ATT_ERROR_RSP, else None.
pub fn att_error(rsp: Option<&[u8]>) -> Option<(u8, u16, u8)> {
    let r = rsp?;
    if r.len() == 5 && r[0] == 0x01 {
        return Some((r[1], u16::from_le_bytes([r[2], r[3]]), r[4]));
    }
    None
}

/// UUID as sent (little-endian, 2 or 16 octets) -> text.
pub fn uuid_str(u: &[u8]) -> String {
    let mut v = u.to_vec();
    v.reverse();
    hx(&v)
}

#[derive(Clone, Debug)]
pub struct Service {
    pub start: u16,
    pub end: u16,
    pub uuid: Vec<u8>,
    pub primary: bool,
}

#[derive(Clone, Debug)]
pub struct Char {
    pub decl: u16,
    pub props: u8,
    pub value: u16,
    pub uuid: Vec<u8>,
    pub end: u16,
    pub descs: Vec<(u16, Vec<u8>)>,
}

/// The attribute table of the server as discovery shows it (Vol 3 Part G 4.4-4.7).
#[derive(Clone, Debug, Default)]
pub struct Gatt {
    pub problems: Vec<String>,
    /// handle -> type UUID (bytes as sent), from Find Information
    pub attrs: std::collections::BTreeMap<u16, Vec<u8>>,
    pub services: Vec<Service>,
    pub chars: Vec<Char>,
    pub raw_find_info: Vec<(u16, Option<Vec<u8>>)>,
    pub raw_groups: Vec<(u16, u16, Option<Vec<u8>>)>,
    pub raw_chars: Vec<(u16, Option<Vec<u8>>)>,
}

fn le16(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

impl Gatt {
    pub fn discover(s: &mut BleSession) -> R<Gatt> {
        let mut g = Gatt::default();
        g.find_info(s)?;
        g.groups(s, 0x2800, true)?;
        g.groups(s, 0x2801, false)?;
        g.services.sort_by_key(|x| x.start);
        g.chars_(s)?;
        Ok(g)
    }

    fn find_info(&mut self, s: &mut BleSession) -> R<()> {
        let mut start: u32 = 1;
        while start <= 0xFFFF {
            let mut q = vec![0x04];
            q.extend_from_slice(&(start as u16).to_le_bytes());
            q.extend_from_slice(&0xFFFFu16.to_le_bytes());
            let rsp = s.att(&q)?;
            self.raw_find_info.push((start as u16, rsp.clone()));
            let Some(rsp) = rsp.filter(|r| r.len() >= 2 && r[0] == 0x05) else { break };
            let w = if rsp[1] == 1 { 4 } else { 18 };
            let body = &rsp[2..];
            if !(rsp[1] == 1 || rsp[1] == 2) || body.is_empty() || body.len() % w != 0 {
                self.problems.push(format!("Find Information response malformed: {}", hx(&rsp)));
                break;
            }
            let mut last = None;
            for i in (0..body.len()).step_by(w) {
                let h = le16(body, i);
                self.attrs.insert(h, body[i + 2..i + w].to_vec());
                last = Some(h);
            }
            match last {
                Some(l) if (l as u32) >= start && l != 0xFFFF => start = l as u32 + 1,
                _ => break,
            }
        }
        Ok(())
    }

    fn groups(&mut self, s: &mut BleSession, gtype: u16, primary: bool) -> R<()> {
        let mut start: u32 = 1;
        while start <= 0xFFFF {
            let mut q = vec![0x10];
            q.extend_from_slice(&(start as u16).to_le_bytes());
            q.extend_from_slice(&0xFFFFu16.to_le_bytes());
            q.extend_from_slice(&gtype.to_le_bytes());
            let rsp = s.att(&q)?;
            self.raw_groups.push((gtype, start as u16, rsp.clone()));
            let Some(rsp) = rsp.filter(|r| r.len() >= 2 && r[0] == 0x11) else { break };
            let w = rsp[1] as usize;
            let body = &rsp[2..];
            if !(w == 6 || w == 20) || body.is_empty() || body.len() % w != 0 {
                self.problems.push(format!("Read By Group Type response malformed: {}", hx(&rsp)));
                break;
            }
            let mut end = None;
            for i in (0..body.len()).step_by(w) {
                let (st, e) = (le16(body, i), le16(body, i + 2));
                self.services.push(Service { start: st, end: e, uuid: body[i + 4..i + w].to_vec(), primary });
                end = Some(e);
            }
            match end {
                Some(e) if (e as u32) >= start && e != 0xFFFF => start = e as u32 + 1,
                _ => break,
            }
        }
        Ok(())
    }

    fn chars_(&mut self, s: &mut BleSession) -> R<()> {
        let mut start: u32 = 1;
        while start <= 0xFFFF {
            let mut q = vec![0x08];
            q.extend_from_slice(&(start as u16).to_le_bytes());
            q.extend_from_slice(&0xFFFFu16.to_le_bytes());
            q.extend_from_slice(&0x2803u16.to_le_bytes());
            let rsp = s.att(&q)?;
            self.raw_chars.push((start as u16, rsp.clone()));
            let Some(rsp) = rsp.filter(|r| r.len() >= 2 && r[0] == 0x09) else { break };
            let w = rsp[1] as usize;
            let body = &rsp[2..];
            if !(w == 7 || w == 21) || body.is_empty() || body.len() % w != 0 {
                self.problems.push(format!("Read By Type (characteristic) response malformed: {}", hx(&rsp)));
                break;
            }
            let mut last = None;
            for i in (0..body.len()).step_by(w) {
                let (h, props, vh) = (le16(body, i), body[i + 2], le16(body, i + 3));
                self.chars.push(Char { decl: h, props, value: vh, uuid: body[i + 5..i + w].to_vec(), end: 0, descs: vec![] });
                last = Some(h);
            }
            match last {
                Some(l) if (l as u32) >= start && l != 0xFFFF => start = l as u32 + 1,
                _ => break,
            }
        }
        self.chars.sort_by_key(|c| c.decl);
        let hs: Vec<u16> = self.attrs.keys().copied().collect();
        for i in 0..self.chars.len() {
            let decl = self.chars[i].decl;
            let svc_end = self.services.iter().find(|x| x.start <= decl && decl <= x.end).map(|x| x.end).unwrap_or(0xFFFF);
            let nxt = if i + 1 < self.chars.len() { self.chars[i + 1].decl - 1 } else { 0xFFFF };
            let end = nxt.min(svc_end);
            let value = self.chars[i].value;
            self.chars[i].end = end;
            self.chars[i].descs = hs.iter().filter(|h| value < **h && **h <= end).map(|h| (*h, self.attrs[h].clone())).collect();
        }
        Ok(())
    }

    pub fn char16(&self, uuid16: u16) -> Option<&Char> {
        let want = uuid16.to_le_bytes();
        self.chars.iter().find(|c| c.uuid == want)
    }

    pub fn char_uuid(&self, uuid: &[u8]) -> Option<&Char> {
        self.chars.iter().find(|c| c.uuid == uuid)
    }

    pub fn service(&self, uuid16: u16) -> Option<&Service> {
        let want = uuid16.to_le_bytes();
        self.services.iter().find(|x| x.uuid == want)
    }

    pub fn cccd(&self, c: &Char) -> Option<u16> {
        c.descs.iter().find(|(_, u)| u[..] == [0x02, 0x29]).map(|(h, _)| *h)
    }

    pub fn last_handle(&self) -> u16 {
        self.attrs.keys().next_back().copied().unwrap_or(0)
    }
}

impl BleSession {
    /// The table of the server, discovered once per session.
    pub fn gatt(&mut self) -> R<Gatt> {
        if let Some(g) = self.note::<Gatt>("gatt") {
            return Ok(g);
        }
        let g = Gatt::discover(self)?;
        self.set_note("gatt", g.clone());
        Ok(g)
    }
}

/// The LLData and header choices of a scripted CONNECT_IND (`Session.connect_ind`).
#[derive(Clone, Debug)]
pub struct ConnInd {
    pub adva: Option<Vec<u8>>,
    pub rxadd: Option<u8>,
    pub inita: Vec<u8>,
    pub txadd: u8,
    pub aa: u32,
    pub crcinit: Vec<u8>,
    pub winsize: u8,
    pub winoffset: u16,
    pub interval: u16,
    pub latency: u16,
    pub timeout: u16,
    pub chm: Vec<u8>,
    pub hop: u8,
    pub sca: u8,
    pub chsel: u8,
}

impl Default for ConnInd {
    fn default() -> Self {
        ConnInd {
            adva: None,
            rxadd: None,
            inita: vec![0x66, 0x55, 0x44, 0x33, 0x22, 0x11],
            txadd: 0,
            aa: 0x71764129,
            crcinit: vec![0x55, 0x44, 0x33],
            winsize: 2,
            winoffset: 0,
            interval: 24,
            latency: 0,
            timeout: 400,
            chm: ALL_CHANNELS.to_vec(),
            hop: 7,
            sca: 1,
            chsel: 0,
        }
    }
}

impl BleSession {
    /// A CONNECT_IND for the bulb (to send with `adv_reply`).
    pub fn connect_ind(&mut self, c: &ConnInd) -> R<Vec<u8>> {
        let (a, t) = self.address()?;
        let mut v = vec![0x05 | c.chsel << 5 | c.txadd << 6 | c.rxadd.unwrap_or(t) << 7, 34];
        v.extend_from_slice(&c.inita);
        v.extend_from_slice(c.adva.as_deref().unwrap_or(&a));
        v.extend_from_slice(&c.aa.to_le_bytes());
        v.extend_from_slice(&c.crcinit);
        v.push(c.winsize);
        for x in [c.winoffset, c.interval, c.latency, c.timeout] {
            v.extend_from_slice(&x.to_le_bytes());
        }
        v.extend_from_slice(&c.chm);
        v.push(c.hop & 0x1F | c.sca << 5);
        Ok(v)
    }
}
