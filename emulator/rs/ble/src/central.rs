use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashSet};
use std::rc::{Rc, Weak};

use tc32emu::aes128;
use tc32emu::bigint::BigUint;
use tc32emu::Machine;

use crate::gofmt::hex;
use crate::pyrandom::PyRandom;
use crate::radio::{att_name_of, ll_name_of, Peer, PeerRc, RadioRef};
use crate::sc;
use crate::{round3, settings};

#[derive(Clone, Debug, PartialEq)]
pub enum ScriptValue {
    Nil,
    Bytes(Vec<u8>),
    Pair(PairResult),
}

pub trait AttScript {
    fn next(&self, c: &mut Central, m: &mut Machine, value: &ScriptValue) -> (Option<Vec<u8>>, bool, bool);
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PairResult {
    pub ok: bool,
    pub reason: u8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Params {
    pub interval: i64,
    pub latency: i64,
    pub timeout: i64,
    pub hop: i64,
    pub win_size: i64,
    pub win_offset: i64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Update {
    pub win_size: i64,
    pub win_offset: i64,
    pub interval: i64,
    pub latency: i64,
    pub timeout: i64,
    pub instant: i64,
    pub applied: f64,
    pub is_applied: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChmUpdate {
    pub chm: Vec<u8>,
    pub instant: i64,
    pub applied: f64,
}

pub type Tag = Rc<dyn Any>;

#[derive(Clone)]
pub(crate) struct Inflight {
    pub(crate) llid: u8,
    pub(crate) payload: Vec<u8>,
    pub(crate) plain: Vec<u8>,
    pub(crate) tag: Option<Tag>,
}

#[derive(Clone)]
pub(crate) struct Queued {
    pub(crate) llid: u8,
    pub(crate) payload: Vec<u8>,
    pub(crate) tag: Option<Tag>,
}

#[derive(Clone, Debug, Default)]
pub struct Enc {
    pub sk: Vec<u8>,
    pub iv: Vec<u8>,
    pub tx: bool,
    pub rx: bool,
    pub txc: i64,
    pub rxc: i64,
    pub ltk: u128,
    pub skdm: Option<Vec<u8>>,
    pub ivm: Vec<u8>,
    pub mic_fail: i64,
}

#[derive(Clone, Debug, Default)]
pub struct Smp {
    pub preq: Vec<u8>,
    pub pres: Vec<u8>,
    pub t0: f64,
    pub keys: BTreeMap<String, String>,
    pub mrand: Vec<u8>,
    pub sconfirm: Vec<u8>,
    pub srand: Vec<u8>,
    pub sconfirm_ok: bool,
    pub failed: i64,
    pub done_ms: f64,
    pub security_request: String,
    pub started: bool,
    pub sent_keys: BTreeMap<String, String>,
    pub private: Option<BigUint>,
    pub pka: Option<Vec<u8>>,
    pub pkb: Option<Vec<u8>>,
    pub method: String,
    pub round: i64,
    pub passkey: i64,
    pub na: Vec<u8>,
    pub nb: Vec<u8>,
    pub cb: Vec<u8>,
    pub mac_key: Vec<u8>,
    pub r: Vec<u8>,
    pub ltk: String,
    pub pkb_on_curve: bool,
    pub confirms_ok: Vec<bool>,
    pub has_dhkey_check: bool,
    pub dhkey_check_ok: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stats {
    pub events: i64,
    pub replies: i64,
    pub missed: i64,
    pub retx: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Notification {
    pub ms: f64,
    pub handle: i64,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AttEntry {
    pub ms: f64,
    pub dir: &'static str,
    pub name: String,
    pub hex: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Dropped {
    pub ms: f64,
    pub event: i64,
    pub hex: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ParamRequest {
    pub t_ms: f64,
    pub event: i64,
    pub interval_min: i64,
    pub interval_max: i64,
    pub latency: i64,
    pub timeout: i64,
    pub instant: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Rec {
    F(f64),
    I(i64),
    S(String),
    Ints(Vec<i64>),
    L(Vec<Rec>),
    M(Vec<(String, Rec)>),
    U(Update),
}

pub(crate) struct L2Frame {
    pub(crate) ln: usize,
    pub(crate) cid: i64,
    pub(crate) data: Vec<u8>,
}

pub type LogFn = Rc<dyn Fn(&str)>;
pub type OnAdvFn = Rc<dyn Fn(&mut Central, &mut Machine, &[u8], u8) -> Option<Vec<u8>>>;
pub type CentralFn = Rc<dyn Fn(&mut Central, &mut Machine)>;
pub type ScriptStepFn = Rc<dyn Fn(&mut Central, &mut Machine, ScriptValue)>;
pub type LlCtrlFn = Rc<dyn Fn(&mut Central, &mut Machine, &[u8])>;
pub type EventHookFn = Rc<dyn Fn(&mut Central, &mut Machine, i64)>;
pub type OnParamRequestFn = Rc<dyn Fn(ParamRequest, &str)>;

pub struct Central {
    pub radio: RadioRef,
    pub(crate) me: Weak<RefCell<Central>>,
    pub log: Option<LogFn>,
    pub p: Params,
    pub connect_after_ms: f64,
    pub anchor_in_window_ms: f64,
    pub aa: Vec<u8>,
    pub aa_reg: Vec<u8>,
    pub init_a: Vec<u8>,
    pub init_a_type: i64,
    pub identity: Vec<u8>,
    pub own_addr: String,
    pub irk: Vec<u8>,
    pub init_key_dist: u8,
    pub active_scan: bool,
    pub ack_terminate: bool,
    pub state: &'static str,
    pub sn: i64,
    pub nesn: i64,
    pub(crate) inflight: Option<Inflight>,
    pub(crate) txq: Vec<Queued>,
    pub event: i64,
    pub(crate) unmapped: i64,
    pub chm: Vec<u8>,
    pub(crate) chm_update: Option<ChmUpdate>,
    pub transcript: Vec<String>,
    pub(crate) l2: Option<L2Frame>,
    pub max_exch: i64,
    pub(crate) exch: i64,
    pub stats: Stats,
    pub records: BTreeMap<String, Rec>,
    pub channels: Vec<[i64; 2]>,
    pub sent: Vec<[f64; 2]>,
    pub ntf: Vec<Notification>,
    pub att_log: Vec<AttEntry>,
    pub script: Option<Rc<dyn AttScript>>,
    pub(crate) att_waiting: bool,
    pub(crate) att_started: bool,
    pub event_hooks: Vec<EventHookFn>,
    pub send_version: bool,
    pub send_features: bool,
    pub(crate) version_sent: bool,
    pub(crate) silent_from: i64,
    pub(crate) silent: bool,
    pub adv_a: Vec<u8>,
    pub adv_a_random: i64,
    pub adv_pdu: Vec<u8>,
    pub pair: bool,
    pub apply_update: bool,
    pub update_lead: i64,
    pub refuse_update: bool,
    pub on_param_request: Option<OnParamRequestFn>,
    pub drop_at: HashSet<i64>,
    pub dropped: Vec<Dropped>,
    pub(crate) update: Option<Update>,
    pub(crate) rng: PyRandom,
    pub enc: Enc,
    pub smp: Smp,
    pub t_conn_end: f64,
    pub anchor0: f64,
    pub interval_ms: f64,
    pub(crate) base_t: f64,
    pub(crate) base_k: i64,
    pub cur_ch: u8,
    pub(crate) last_plain: Option<Vec<u8>>,
    pub flash_log_start: usize,
    pub on_adv_fn: OnAdvFn,
    pub start_pairing_fn: CentralFn,
    pub script_step_fn: ScriptStepFn,
    pub io_cap: u8,
    pub mitm: bool,
    pub sc_priv: Option<BigUint>,
    pub passkey: i64,
    pub displayed_passkey: i64,
    pub fault: String,
    pub ll_ctrl_fn: LlCtrlFn,
    pub establish: bool,
    pub heard: bool,
    pub mic_ends: bool,
    pub ll_rules: bool,
    pub(crate) enc_busy: bool,
    pub(crate) update_lead_k: i64,
    pub(crate) update_sent: bool,
    pub on_bond: Option<Rc<dyn Fn()>>,
    pub rx_data: Option<Rc<dyn Fn(u8, &[u8])>>,
    pub on_ack: Option<Rc<dyn Fn(&Tag)>>,
}

pub type CentralRc = Rc<RefCell<Central>>;

#[derive(Clone)]
pub struct Options {
    pub interval: i64,
    pub latency: i64,
    pub timeout: i64,
    pub hop: i64,
    pub win_size: i64,
    pub win_offset: i64,
    pub connect_after_ms: f64,
    pub anchor_in_window_ms: f64,
    pub log: Option<LogFn>,
    pub max_exch: i64,
    pub pair: bool,
    pub apply_update: bool,
    pub refuse_update: bool,
    pub on_param_request: Option<OnParamRequestFn>,
    pub drop_at: Vec<i64>,
    pub chm: Vec<u8>,
    pub att_script: Option<Rc<dyn AttScript>>,
    pub event_hooks: Vec<EventHookFn>,
    pub send_version: bool,
    pub send_features: bool,
    pub join: bool,
    pub aa: u32,
    pub active_scan: bool,
    pub own_addr: String,
    pub irk: Option<Vec<u8>>,
    pub dist_id: bool,
    pub no_terminate_ack: bool,
    pub establish: bool,
    pub mic_ends: bool,
    pub ll_rules: bool,
    pub update_lead: i64,
}

pub const DEFAULT_IRK: [u8; 16] = [0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7, 0xA8, 0xA9, 0xAA, 0xAB, 0xAC, 0xAD, 0xAE, 0xAF];

thread_local! {
    static RPA_N: Cell<u32> = const { Cell::new(0) };
}

impl Default for Options {
    fn default() -> Options {
        Options {
            interval: 24,
            latency: 0,
            timeout: 500,
            hop: 7,
            win_size: 2,
            win_offset: 0,
            connect_after_ms: 30.0,
            anchor_in_window_ms: 0.3,
            log: Some(Rc::new(|s: &str| println!("{}", s))),
            max_exch: 4,
            pair: true,
            apply_update: false,
            refuse_update: false,
            on_param_request: None,
            drop_at: Vec::new(),
            chm: vec![0xff, 0xff, 0xff, 0xff, 0x1f],
            att_script: None,
            event_hooks: Vec::new(),
            send_version: true,
            send_features: true,
            join: false,
            aa: 0,
            active_scan: false,
            own_addr: String::new(),
            irk: None,
            dist_id: false,
            no_terminate_ack: false,
            establish: false,
            mic_ends: false,
            ll_rules: false,
            update_lead: 0,
        }
    }
}

pub fn default_options() -> Options {
    Options::default()
}

fn ll_name(op: u8) -> String {
    match ll_name_of(op) {
        Some(n) => n.to_string(),
        None => format!("0x{:x}", op),
    }
}

fn att_name(op: u8) -> String {
    match att_name_of(op) {
        Some(n) => n.to_string(),
        None => format!("0x{:x}", op),
    }
}

fn u16s(p: &[u8], n: usize) -> Vec<i64> {
    (0..n).map(|i| (p[2 * i] as i64) | (p[2 * i + 1] as i64) << 8).collect()
}

fn le16(p: &[u8], o: usize) -> i64 {
    p[o] as i64 | (p[o + 1] as i64) << 8
}

fn encpdu(q: &Queued) -> bool {
    q.llid == 3 && !q.payload.is_empty() && (q.payload[0] == 0x06 || q.payload[0] == 0x02)
}

pub fn fmt_pdu(pdu: &[u8]) -> String {
    let (hdr0, ln) = (pdu[0], pdu[1] as usize);
    let llid = hdr0 & 3;
    let bits = format!("NESN{} SN{} MD{}", (hdr0 >> 2) & 1, (hdr0 >> 3) & 1, (hdr0 >> 4) & 1);
    let p = &pdu[2..2 + ln];
    let what = if llid == 1 && ln == 0 {
        "empty".to_string()
    } else if llid == 3 {
        format!("{} {}", ll_name(p[0]), hex(&p[1..]))
    } else if llid == 2 && ln >= 4 {
        let (l2len, cid) = (le16(p, 0), le16(p, 2));
        let body = &p[4..];
        if cid == 4 && !body.is_empty() {
            format!("ATT {} {}", att_name(body[0]), hex(body))
        } else if cid == 5 {
            format!("L2CAP signaling {}", hex(body))
        } else if cid == 6 {
            format!("SMP {}", hex(body))
        } else {
            format!("L2CAP cid {} len {} {}", cid, l2len, hex(body))
        }
    } else {
        format!("LLID{} {}", llid, hex(p))
    };
    format!("[{} len {:2}] {}", bits, ln, what)
}

impl Central {
    pub fn new(m: &mut Machine, radio: &RadioRef, o: Options) -> CentralRc {
        settings::init();
        let mut o = o;
        if o.own_addr.is_empty() {
            o.own_addr = "public".into();
        }
        if o.own_addr != "public" && o.own_addr != "rpa" && o.own_addr != "rpa-wrong-irk" {
            panic!("own_addr: {}", o.own_addr);
        }
        if o.aa == 0 {
            o.aa = 0x71764129;
        }
        let irk = o.irk.clone().unwrap_or_else(|| DEFAULT_IRK.to_vec());
        let _ = m;
        let c = Central {
            radio: radio.clone(),
            me: Weak::new(),
            log: o.log.clone(),
            p: Params { interval: o.interval, latency: o.latency, timeout: o.timeout, hop: o.hop, win_size: o.win_size, win_offset: o.win_offset },
            connect_after_ms: o.connect_after_ms,
            anchor_in_window_ms: o.anchor_in_window_ms,
            aa: o.aa.to_le_bytes().to_vec(),
            aa_reg: o.aa.to_be_bytes().to_vec(),
            init_a: Vec::new(),
            init_a_type: 0,
            identity: vec![0x66, 0x55, 0x44, 0x33, 0x22, 0x11],
            own_addr: o.own_addr.clone(),
            irk,
            init_key_dist: if o.dist_id { 0x02 } else { 0 },
            active_scan: o.active_scan,
            ack_terminate: !o.no_terminate_ack,
            state: "scan",
            sn: 0,
            nesn: 0,
            inflight: None,
            txq: Vec::new(),
            event: -1,
            unmapped: 0,
            chm: o.chm.clone(),
            chm_update: None,
            transcript: Vec::new(),
            l2: None,
            max_exch: o.max_exch,
            exch: 0,
            stats: Stats::default(),
            records: BTreeMap::new(),
            channels: Vec::new(),
            sent: Vec::new(),
            ntf: Vec::new(),
            att_log: Vec::new(),
            script: o.att_script.clone(),
            att_waiting: false,
            att_started: false,
            event_hooks: o.event_hooks.clone(),
            send_version: o.send_version,
            send_features: o.send_features,
            version_sent: false,
            silent_from: 0,
            silent: false,
            adv_a: Vec::new(),
            adv_a_random: 1,
            adv_pdu: Vec::new(),
            pair: o.pair,
            apply_update: o.apply_update,
            update_lead: o.update_lead,
            refuse_update: o.refuse_update,
            on_param_request: o.on_param_request.clone(),
            drop_at: o.drop_at.iter().copied().collect(),
            dropped: Vec::new(),
            update: None,
            rng: PyRandom::new(1),
            enc: Enc::default(),
            smp: Smp::default(),
            t_conn_end: 0.0,
            anchor0: 0.0,
            interval_ms: 0.0,
            base_t: 0.0,
            base_k: 0,
            cur_ch: 0,
            last_plain: None,
            flash_log_start: 0,
            on_adv_fn: Rc::new(|c: &mut Central, m: &mut Machine, pdu: &[u8], ch: u8| c.on_adv_default(m, pdu, ch)),
            start_pairing_fn: Rc::new(|c: &mut Central, m: &mut Machine| c.start_pairing_sc(m, false)),
            script_step_fn: Rc::new(|c: &mut Central, m: &mut Machine, v: ScriptValue| c.script_step_default(m, v)),
            io_cap: 0x03,
            mitm: false,
            sc_priv: None,
            passkey: -1,
            displayed_passkey: -1,
            fault: String::new(),
            ll_ctrl_fn: Rc::new(|c: &mut Central, m: &mut Machine, p: &[u8]| c.ll_ctrl(m, p)),
            establish: o.establish,
            heard: false,
            mic_ends: o.mic_ends,
            ll_rules: o.ll_rules,
            enc_busy: false,
            update_lead_k: 0,
            update_sent: false,
            on_bond: None,
            rx_data: None,
            on_ack: None,
        };
        let rc = Rc::new(RefCell::new(c));
        rc.borrow_mut().me = Rc::downgrade(&rc);
        rc.borrow_mut().new_address();
        let peer: PeerRc = rc.clone();
        if o.join {
            radio.b().centrals.push(peer);
        } else {
            radio.b().central = Some(peer);
        }
        rc
    }

    pub(crate) fn me(&self) -> CentralRc {
        self.me.upgrade().expect("central dropped")
    }

    pub fn new_address(&mut self) {
        if self.own_addr == "public" {
            self.init_a = self.identity.clone();
            self.init_a_type = 0;
            return;
        }
        let n = RPA_N.with(|c| {
            let n = c.get();
            c.set(n.wrapping_add(1));
            n
        });
        let prand = 0x15A3C7u32.wrapping_add(0x0B1D2Fu32.wrapping_mul(n)) & 0x3FFFFF;
        let mut key = self.irk.clone();
        if self.own_addr != "rpa" {
            for k in key.iter_mut() {
                *k ^= 0xFF;
            }
        }
        self.init_a = aes128::rpa(&key, prand).to_vec();
        self.init_a_type = 1;
    }

    pub fn scan_request(&mut self, m: &mut Machine, pdu: &[u8], ch: u8) -> Option<Vec<u8>> {
        if !self.active_scan || self.state != "scan" || pdu.len() < 8 || (pdu[0] & 0xF != 0 && pdu[0] & 0xF != 6) {
            return None;
        }
        let mut req = vec![0x03 | ((self.init_a_type << 6) as u8) | (pdu[0] & 0x40) << 1, 12];
        req.extend_from_slice(&self.init_a);
        req.extend_from_slice(&pdu[2..8]);
        let kind = if pdu[0] & 0xF == 6 { "ADV_SCAN_IND" } else { "ADV_IND" };
        self.rec(m, "air", format!("ch {} <- {} {}", ch, kind, hex(pdu)));
        self.rec(m, "air", format!("ch {} -> SCAN_REQ {}", ch, hex(&req)));
        Some(req)
    }

    pub fn on_adv_default(&mut self, m: &mut Machine, pdu: &[u8], ch: u8) -> Option<Vec<u8>> {
        if self.state != "scan" || m.ms() < self.connect_after_ms || pdu[0] & 0xF != 0 || pdu.len() < 8 {
            return None;
        }
        self.adv_a = pdu[2..8].to_vec();
        self.adv_a_random = ((pdu[0] >> 6) & 1) as i64;
        let mut payload = self.init_a.clone();
        payload.extend_from_slice(&self.adv_a);
        payload.extend_from_slice(&self.aa);
        payload.extend_from_slice(&[0x55, 0x44, 0x33, self.p.win_size as u8]);
        for v in [self.p.win_offset, self.p.interval, self.p.latency, self.p.timeout] {
            payload.extend_from_slice(&(v as u16).to_le_bytes());
        }
        payload.extend_from_slice(&self.chm);
        payload.push((self.p.hop | (1 << 5)) as u8);
        let hdr = [0x05 | ((self.init_a_type << 6) as u8) | ((self.adv_a_random << 7) as u8), payload.len() as u8];
        self.state = "connecting";
        self.adv_pdu = pdu.to_vec();
        let mut full = hdr.to_vec();
        full.extend_from_slice(&payload);
        self.rec(m, "air", format!("ch {} <- ADV_IND {}", ch, hex(pdu)));
        let (iv, lat, to, hop) = (self.p.interval, self.p.latency, self.p.timeout, self.p.hop);
        self.rec(m, "air", format!("ch {} -> CONNECT_IND {} interval {} latency {} timeout {} hop {}", ch, hex(&full), iv, lat, to, hop));
        Some(full)
    }

    pub fn on_connect_sent(&mut self, _m: &mut Machine, _pdu: &[u8], t_end: f64) {
        self.state = "connected";
        self.heard = false;
        self.enc_busy = false;
        self.t_conn_end = t_end;
        let clk = self.clock();
        self.anchor0 = t_end + 1.25 * clk + (1.25 * clk) * (self.p.win_offset as f64) + self.anchor_in_window_ms;
        self.interval_ms = (self.p.interval as f64 * 1.25) * clk;
        self.base_t = self.anchor0;
        self.base_k = 0;
        if self.send_version {
            self.queue_ll(vec![0x0C, 0x0B, 0x0F, 0x00, 0x34, 0x12]);
            self.version_sent = true;
        }
        if self.send_features {
            self.queue_ll(vec![0x08, 0x01, 0, 0, 0, 0, 0, 0, 0]);
        }
        let at = self.anchor0 + self.jitter_ms();
        self.schedule_event(at, 0);
    }

    pub(crate) fn schedule_event(&self, at: f64, k: i64) {
        let me = self.me();
        self.radio.at(at, Box::new(move |m| me.borrow_mut().conn_event(m, k)), "central");
    }

    pub(crate) fn clock(&self) -> f64 {
        match &self.radio.b().air {
            None => 1.0,
            Some(a) => a.clock(),
        }
    }

    pub(crate) fn jitter_ms(&self) -> f64 {
        match self.radio.b().air.as_mut() {
            None => 0.0,
            Some(a) => a.jitter_ms(),
        }
    }

    pub fn reclock(&mut self) {
        if self.state != "connected" || self.event < 0 {
            return;
        }
        let k = self.event + 1;
        self.base_t += ((k - self.base_k) as f64) * self.interval_ms;
        self.base_k = k;
        self.interval_ms = (self.p.interval as f64 * 1.25) * self.clock();
    }

    pub(crate) fn tx_note(&self, ok: bool) -> &'static str {
        if ok {
            return "";
        }
        if self.radio.b().tx_lost {
            return "   (LOST on air)";
        }
        "   (MISSED by the peripheral)"
    }

    pub fn air_lost(&mut self, m: &mut Machine, pdu: &[u8]) {
        let s = format!("ev {:4} ch {:2} <- {}  (LOST on air)", self.event, self.cur_ch, fmt_pdu(pdu));
        self.rec(m, "rx", s);
    }

    pub fn air_lost_tx(&mut self, m: &mut Machine, _pdu: &[u8]) {
        let s = format!("ev {:4} ch {:2} -> the packet above  (LOST on air)", self.event, self.cur_ch);
        self.rec(m, "tx", s);
    }

    pub(crate) fn chm_used(&self, ch: i64) -> bool {
        (self.chm[(ch >> 3) as usize] >> (ch & 7)) & 1 != 0
    }

    pub fn channel(&mut self) -> i64 {
        self.unmapped = (self.unmapped + self.p.hop) % 37;
        if self.chm_used(self.unmapped) {
            return self.unmapped;
        }
        let used: Vec<i64> = (0..37).filter(|&ch| self.chm_used(ch)).collect();
        used[(self.unmapped % used.len() as i64) as usize]
    }

    pub(crate) fn conn_event(&mut self, m: &mut Machine, k: i64) {
        if self.state != "connected" && self.state != "terminate-ack" {
            return;
        }
        if self.establish && k >= 6 && !self.heard {
            self.state = "failed";
            self.records.insert("establish_failed_ms".into(), Rec::F(round3(m.ms())));
            self.rec(m, "tx", format!("ev {:4} nothing heard from the peripheral in 6 events: the connection was not established", k));
            return;
        }
        if let Some(u) = self.update {
            if self.ll_rules && !u.is_applied && self.update_sent && k == u.instant {
                let anchor = self.base_t + ((k - self.base_k) as f64) * self.interval_ms;
                self.move_update(k, anchor);
                let at = self.base_t + self.jitter_ms();
                self.schedule_event(at, k);
                return;
            }
        }
        self.event = k;
        self.stats.events += 1;
        if let Some(u) = &mut self.chm_update {
            if k == u.instant {
                self.chm = u.chm.clone();
                u.applied = round3(m.ms());
                let rec = Rec::M(vec![
                    ("chm".into(), Rec::S(hex(&u.chm))),
                    ("instant".into(), Rec::I(u.instant)),
                    ("applied".into(), Rec::F(u.applied)),
                ]);
                self.records.insert("chm_update_applied".into(), rec);
                self.chm_update = None;
            }
        }
        let ch = self.channel();
        self.channels.push([k, ch]);
        self.exch = 0;
        self.cur_ch = ch as u8;
        self.hook_event(m, k);
        for h in self.event_hooks.clone() {
            h(self, m, k);
        }
        if self.state != "connected" && self.state != "terminate-ack" {
            return;
        }
        let se = settings::central_skip_every();
        let skipped = se > 0 && k >= 20 && k % se == 0;
        if skipped {
            self.rec(m, "tx", format!("ev {:4} ch {:2} -> nothing (TC32EMU_CENTRAL_SKIP_EVERY)", k, ch));
        }
        let silent = self.silent && k >= self.silent_from;
        if self.state == "terminate-ack" && (silent || !skipped) {
            if !silent {
                let mut pdu = vec![0x01 | ((self.nesn << 2) as u8) | ((self.sn << 3) as u8), 0];
                if self.inflight.is_some() {
                    pdu = self.next_pdu();
                }
                self.sent.push([k as f64, m.ms()]);
                let aa = self.aa_reg.clone();
                let ok = self.radio.central_tx(m, &pdu, ch as u8, &aa);
                let note = self.tx_note(ok);
                self.rec(m, "tx", format!("ev {:4} ch {:2} -> {}  (acknowledges LL_TERMINATE_IND){}", k, ch, fmt_pdu(&pdu), note));
                self.records.insert("terminate_ack_ms".into(), Rec::F(round3(m.ms())));
            }
            self.state = "terminated";
            return;
        }
        if !silent && !skipped {
            let pdu = self.next_pdu();
            self.sent.push([k as f64, m.ms()]);
            let aa = self.aa_reg.clone();
            let ok = self.radio.central_tx(m, &pdu, ch as u8, &aa);
            let note = self.tx_note(ok);
            let shown = self.fmt_tx(&pdu);
            self.rec(m, "tx", format!("ev {:4} ch {:2} -> {}{}", k, ch, shown, note));
            if !ok && !self.radio.b().tx_lost {
                self.stats.missed += 1;
            }
        }
        let mut nxt = self.base_t + ((k + 1 - self.base_k) as f64) * self.interval_ms;
        if let Some(u) = self.update {
            if !u.is_applied && (self.update_sent || !self.ll_rules) && k + 1 == u.instant {
                self.move_update(k + 1, nxt);
                nxt = self.base_t;
            }
        }
        let at = nxt + self.jitter_ms();
        self.schedule_event(at, k + 1);
    }

    pub(crate) fn move_update(&mut self, k: i64, anchor: f64) {
        let clk = self.clock();
        let mut u = self.update.unwrap();
        self.base_t = anchor + (1.25 * clk) * (u.win_offset as f64) + self.anchor_in_window_ms;
        self.base_k = k;
        self.interval_ms = (u.interval as f64 * 1.25) * clk;
        self.p.interval = u.interval;
        self.p.latency = u.latency;
        self.p.timeout = u.timeout;
        u.applied = round3(self.base_t);
        u.is_applied = true;
        self.update = Some(u);
        self.records.insert("conn_update_applied".into(), Rec::U(u));
    }

    pub(crate) fn next_pdu(&mut self) -> Vec<u8> {
        if self.inflight.is_none() {
            let (llid, mut payload, tag) = self.take_tagged();
            let plain = payload.clone();
            if self.enc.tx && !payload.is_empty() {
                let mut nonce = aes128::bytes_le((self.enc.txc | 1 << 39) as u128, 5);
                nonce.extend_from_slice(&self.enc.iv);
                payload = aes128::ccm_encrypt(&self.enc.sk, &nonce, llid, &payload);
                self.enc.txc += 1;
            }
            self.inflight = Some(Inflight { llid, payload, plain, tag });
        } else {
            self.stats.retx += 1;
        }
        let md = if self.more() { 1u8 } else { 0 };
        let f = self.inflight.as_ref().unwrap();
        let mut pdu = vec![f.llid | ((self.nesn << 2) as u8) | ((self.sn << 3) as u8) | (md << 4), f.payload.len() as u8];
        pdu.extend_from_slice(&f.payload);
        if f.plain == f.payload {
            self.last_plain = None;
        } else {
            let mut lp = vec![pdu[0], f.plain.len() as u8];
            lp.extend_from_slice(&f.plain);
            self.last_plain = Some(lp);
        }
        pdu
    }

    pub fn take(&mut self) -> (u8, Vec<u8>) {
        let (llid, payload, _) = self.take_tagged();
        (llid, payload)
    }

    pub(crate) fn take_tagged(&mut self) -> (u8, Vec<u8>, Option<Tag>) {
        if !self.ll_rules {
            if self.txq.is_empty() {
                return (1, Vec::new(), None);
            }
            let q = self.txq.remove(0);
            return (q.llid, q.payload, q.tag);
        }
        if self.enc_busy {
            if let Some(i) = self.txq.iter().position(encpdu) {
                let q = self.txq.remove(i);
                return (q.llid, q.payload, None);
            }
            return (1, Vec::new(), None);
        }
        if self.txq.is_empty() {
            return (1, Vec::new(), None);
        }
        let mut q = self.txq.remove(0);
        if q.llid == 3 && !q.payload.is_empty() && q.payload[0] == 0x03 {
            self.enc_busy = true;
        } else if q.llid == 3 && !q.payload.is_empty() && q.payload[0] == 0x00 && self.update.is_some_and(|u| !u.is_applied) {
            let inst = (self.event + self.update_lead_k) & 0xFFFF;
            self.update.as_mut().unwrap().instant = inst;
            self.update_sent = true;
            let mut p = q.payload[..10].to_vec();
            p.extend_from_slice(&(inst as u16).to_le_bytes());
            q.payload = p;
        }
        (q.llid, q.payload, q.tag)
    }

    pub(crate) fn more(&self) -> bool {
        if self.ll_rules && self.enc_busy {
            return self.txq.iter().any(encpdu);
        }
        !self.txq.is_empty()
    }

    pub fn on_air(&mut self, m: &mut Machine, pdu: &[u8], ch: u8, _tx_addr: u32) -> Option<Vec<u8>> {
        self.stats.replies += 1;
        self.heard = true;
        let hdr0 = pdu[0];
        let (llid, nesn, sn, md) = (hdr0 & 3, ((hdr0 >> 2) & 1) as i64, ((hdr0 >> 3) & 1) as i64, ((hdr0 >> 4) & 1) as i64);
        let acked = nesn != self.sn;
        if acked {
            self.sn ^= 1;
            if let Some(f) = &self.inflight {
                if f.llid == 3 && !f.plain.is_empty() && f.plain[0] == 0x02 {
                    self.state = "terminated";
                    self.records.insert("terminate_acked_ms".into(), Rec::F(round3(m.ms())));
                }
            }
            if let (Some(f), Some(cb)) = (&self.inflight, &self.on_ack) {
                if let Some(tag) = &f.tag {
                    cb(tag);
                }
            }
            self.inflight = None;
        }
        let is_new = sn == self.nesn;
        if is_new {
            self.nesn ^= 1;
        }
        let mut payload = pdu[2..2 + pdu[1] as usize].to_vec();
        let mut shown = pdu.to_vec();
        let mut note = "";
        if is_new && pdu[1] != 0 && self.enc.rx {
            let mut nonce = aes128::bytes_le(self.enc.rxc as u128, 5);
            nonce.extend_from_slice(&self.enc.iv);
            let (p, ok) = aes128::ccm_decrypt(&self.enc.sk, &nonce, hdr0, &payload);
            payload = p;
            self.enc.rxc += 1;
            shown = vec![hdr0, payload.len() as u8];
            shown.extend_from_slice(&payload);
            if ok {
                note = "  (decrypted)";
            } else {
                note = "  (decrypted, MIC FAILED)";
                self.enc.mic_fail += 1;
            }
        }
        let dup = if !is_new { "  (dup)" } else { "" };
        let nack = if !acked { "  (our packet not acked)" } else { "" };
        let s = format!("ev {:4} ch {:2} <- {}{}{}{}", self.event, ch, fmt_pdu(&shown), note, dup, nack);
        self.rec(m, "rx", s);
        if self.mic_ends && note == "  (decrypted, MIC FAILED)" {
            self.state = "mic-failed";
            self.records.insert("mic_failure_ms".into(), Rec::F(round3(m.ms())));
            let s = format!("ev {:4} the central leaves the connection: MIC failure (0x3d)", self.event);
            self.rec(m, "rx", s);
            return None;
        }
        if is_new && pdu[1] != 0 {
            self.rx_pdu(m, llid, &payload);
        }
        self.exch += 1;
        if self.state != "connected" {
            return None;
        }
        if (md != 0 || self.more() || self.inflight.is_some()) && self.exch < self.max_exch {
            let nxt = self.next_pdu();
            let s = format!("ev {:4} ch {:2} -> {}", self.event, ch, self.fmt_tx(&nxt));
            self.rec(m, "tx", s);
            return Some(nxt);
        }
        None
    }

    pub fn drop_pdu(&mut self, m: &mut Machine, pdu: &[u8]) -> bool {
        if self.drop_at.contains(&self.event) && pdu[1] != 0 {
            self.drop_at.remove(&self.event);
            self.dropped.push(Dropped { ms: round3(m.ms()), event: self.event, hex: hex(pdu) });
            let s = format!("ev {:4} ch {:2} <- {}  (LOST: the central ignores it)", self.event, self.cur_ch, fmt_pdu(pdu));
            self.rec(m, "rx", s);
            return true;
        }
        false
    }

    pub(crate) fn fmt_tx(&self, pdu: &[u8]) -> String {
        match &self.last_plain {
            Some(lp) => fmt_pdu(lp) + "  (sent encrypted)",
            None => fmt_pdu(pdu),
        }
    }

    pub fn queue_ll(&mut self, payload: Vec<u8>) {
        self.txq.push(Queued { llid: 3, payload, tag: None });
    }

    pub fn queue_data(&mut self, llid: u8, payload: &[u8], tag: Option<Tag>) {
        self.txq.push(Queued { llid, payload: payload.to_vec(), tag });
    }

    pub fn queue_l2cap(&mut self, cid: i64, data: &[u8]) {
        let mut frame = (data.len() as u16).to_le_bytes().to_vec();
        frame.extend_from_slice(&(cid as u16).to_le_bytes());
        frame.extend_from_slice(data);
        let mut first = true;
        let mut rest = &frame[..];
        while !rest.is_empty() {
            let n = rest.len().min(27);
            let llid = if first { 2 } else { 1 };
            self.txq.push(Queued { llid, payload: rest[..n].to_vec(), tag: None });
            rest = &rest[n..];
            first = false;
        }
    }

    pub fn channel_map_update(&mut self, chm: &[u8], instant: i64) {
        self.chm_update = Some(ChmUpdate { chm: chm.to_vec(), instant: instant & 0xFFFF, applied: 0.0 });
        let mut p = vec![0x01];
        p.extend_from_slice(chm);
        p.extend_from_slice(&((instant & 0xFFFF) as u16).to_le_bytes());
        self.queue_ll(p);
    }

    pub fn connection_update(&mut self, win_size: i64, win_offset: i64, interval: i64, latency: i64, timeout: i64, instant: i64) {
        self.update = Some(Update { win_size, win_offset, interval, latency, timeout, instant: instant & 0xFFFF, applied: 0.0, is_applied: false });
        self.update_lead_k = (instant - self.event) & 0xFFFF;
        self.update_sent = false;
        let mut p = vec![0x00, win_size as u8];
        for v in [win_offset, interval, latency, timeout, instant & 0xFFFF] {
            p.extend_from_slice(&(v as u16).to_le_bytes());
        }
        self.queue_ll(p);
    }

    pub fn terminate(&mut self, reason: u8) {
        self.queue_ll(vec![0x02, reason]);
    }

    pub fn go_silent(&mut self, event: i64) {
        self.silent_from = event;
        self.silent = true;
    }

    pub(crate) fn rx_pdu(&mut self, m: &mut Machine, llid: u8, payload: &[u8]) {
        if llid == 3 {
            let f = self.ll_ctrl_fn.clone();
            f(self, m, payload);
        } else if let Some(rd) = self.rx_data.clone() {
            rd(llid, payload);
        } else if llid == 2 {
            let (ln, cid) = (le16(payload, 0) as usize, le16(payload, 2));
            self.l2 = Some(L2Frame { ln, cid, data: payload[4..].to_vec() });
            self.l2_check(m);
        } else if llid == 1 && self.l2.is_some() {
            self.l2.as_mut().unwrap().data.extend_from_slice(payload);
            self.l2_check(m);
        }
    }

    pub(crate) fn l2_check(&mut self, m: &mut Machine) {
        let done = {
            let l2 = self.l2.as_ref().unwrap();
            l2.data.len() >= l2.ln
        };
        if done {
            let f = self.l2.take().unwrap();
            self.l2cap(m, f.cid, &f.data[..f.ln]);
        }
    }

    pub(crate) fn append_record(&mut self, key: &str, v: Rec) {
        match self.records.get_mut(key) {
            Some(Rec::L(list)) => list.push(v),
            _ => {
                self.records.insert(key.into(), Rec::L(vec![v]));
            }
        }
    }

    pub fn ll_ctrl(&mut self, m: &mut Machine, p: &[u8]) {
        let op = p[0];
        self.append_record("ll_from_peripheral", Rec::L(vec![Rec::F(round3(m.ms())), Rec::S(ll_name(op)), Rec::S(hex(p))]));
        match op {
            0x0C => {
                self.records.insert(
                    "peripheral_version".into(),
                    Rec::M(vec![
                        ("VersNr".into(), Rec::I(p[1] as i64)),
                        ("CompId".into(), Rec::S(format!("0x{:x}", p[2] as i64 | (p[3] as i64) << 8))),
                        ("SubVersNr".into(), Rec::S(format!("0x{:x}", p[4] as i64 | (p[5] as i64) << 8))),
                    ]),
                );
                if !self.version_sent {
                    self.queue_ll(vec![0x0C, 0x0B, 0x0F, 0x00, 0x34, 0x12]);
                    self.version_sent = true;
                }
            }
            0x09 => {
                self.records.insert("peripheral_features".into(), Rec::S(hex(&p[1..9])));
            }
            0x0E => {
                self.records.insert("peripheral_features_req".into(), Rec::S(hex(&p[1..9])));
                self.queue_ll(vec![0x09, 0x01, 0, 0, 0, 0, 0, 0, 0]);
            }
            0x14 => {
                self.records.insert("peripheral_length_req".into(), Rec::Ints(u16s(&p[1..], 4)));
                self.queue_ll(vec![0x15, 27, 0, 0x48, 0x01, 27, 0, 0x48, 0x01]);
            }
            0x15 => {
                self.records.insert("peripheral_length_rsp".into(), Rec::Ints(u16s(&p[1..], 4)));
            }
            0x0F => {
                self.records.insert("peripheral_conn_param_req".into(), Rec::S(hex(&p[1..])));
                self.queue_ll(vec![0x07, 0x0F]);
            }
            0x12 => self.queue_ll(vec![0x13]),
            0x16 => {
                self.records.insert("peripheral_phy_req".into(), Rec::S(hex(&p[1..3])));
                self.queue_ll(vec![0x07, 0x16]);
            }
            0x02 => {
                self.records.insert("peripheral_terminate".into(), Rec::S(hex(p)));
                self.records.insert("peripheral_terminate_ms".into(), Rec::F(round3(m.ms())));
                self.state = if self.ack_terminate { "terminate-ack" } else { "terminated" };
            }
            0x04 if self.enc.skdm.is_none() => {
                self.append_record("peripheral_enc_rsp", Rec::S(hex(p)));
            }
            0x04 => {
                let mut skd = self.enc.skdm.clone().unwrap();
                skd.extend_from_slice(&p[1..9]);
                let skd = aes128::from_le(&skd);
                self.enc.sk = aes128::bytes128(aes128::e(self.enc.ltk, skd)).to_vec();
                let mut iv = self.enc.ivm.clone();
                iv.extend_from_slice(&p[9..13]);
                self.enc.iv = iv;
            }
            0x05 => {
                self.enc.tx = true;
                self.enc.rx = true;
                self.txq.insert(0, Queued { llid: 3, payload: vec![0x06], tag: None });
            }
            0x06 => {
                self.records.insert("encryption_on_ms".into(), Rec::F(round3(m.ms())));
                self.enc_busy = false;
                if self.smp.pka.is_some() && self.smp.pres.len() >= 7 && self.smp.pres[6] & 0x07 == 0 {
                    self.smp_complete(m);
                }
            }
            0x0D | 0x11 => {
                self.append_record("peripheral_reject", Rec::S(hex(p)));
                self.enc_busy = false;
            }
            0x07 => self.append_record("peripheral_unknown_rsp", Rec::S(hex(p))),
            0x13 => self.append_record("peripheral_ping_rsp", Rec::F(round3(m.ms()))),
            _ => self.queue_ll(vec![0x07, op]),
        }
    }

    pub(crate) fn l2cap(&mut self, m: &mut Machine, cid: i64, data: &[u8]) {
        match cid {
            4 => self.att_rx(m, data),
            5 => {
                let (code, ident) = (data[0], data[1]);
                self.append_record("signaling_from_peripheral", Rec::L(vec![Rec::F(round3(m.ms())), Rec::S(hex(data))]));
                match code {
                    0x12 => {
                        let v = u16s(&data[4..], 4);
                        let (mn, mx, lat, to) = (v[0], v[1], v[2], v[3]);
                        self.records.insert(
                            "l2cap_conn_param_update_req".into(),
                            Rec::M(vec![
                                ("t_ms".into(), Rec::F(round3(m.ms()))),
                                ("event".into(), Rec::I(self.event)),
                                ("interval_min".into(), Rec::I(mn)),
                                ("interval_max".into(), Rec::I(mx)),
                                ("latency".into(), Rec::I(lat)),
                                ("timeout".into(), Rec::I(to)),
                            ]),
                        );
                        let mut req = ParamRequest {
                            t_ms: round3(m.ms()),
                            event: self.event,
                            interval_min: mn,
                            interval_max: mx,
                            latency: lat,
                            timeout: to,
                            instant: -1,
                        };
                        let mut action = "accepted";
                        if self.refuse_update {
                            self.queue_l2cap(5, &[0x13, ident, 2, 0, 1, 0]);
                            action = "refused";
                        } else {
                            self.queue_l2cap(5, &[0x13, ident, 2, 0, 0, 0]);
                            if self.apply_update && self.update.is_none() {
                                let lead = if self.update_lead == 0 { 12 } else { self.update_lead };
                                let ev = self.event;
                                self.connection_update(1, 0, mx, lat, to, ev + lead);
                                action = "applied";
                                req.instant = self.update.unwrap().instant;
                            }
                        }
                        if let Some(cb) = self.on_param_request.clone() {
                            cb(req, action);
                        }
                    }
                    0x13 => {}
                    _ => self.queue_l2cap(5, &[0x01, ident, 2, 0, 0, 0]),
                }
            }
            6 => {
                self.append_record("smp_from_peripheral", Rec::L(vec![Rec::F(round3(m.ms())), Rec::S(hex(data))]));
                self.smp_rx(m, data);
            }
            _ => self.append_record("l2cap_other", Rec::L(vec![Rec::I(cid), Rec::S(hex(data))])),
        }
    }

    pub fn att_send(&mut self, m: &mut Machine, pdu: &[u8]) {
        self.att_log.push(AttEntry { ms: round3(m.ms()), dir: "->", name: att_name(pdu[0]), hex: hex(pdu) });
        self.queue_l2cap(4, pdu);
    }

    pub(crate) fn att_rx(&mut self, m: &mut Machine, data: &[u8]) {
        let op = data[0];
        self.att_log.push(AttEntry { ms: round3(m.ms()), dir: "<-", name: att_name(op), hex: hex(data) });
        match op {
            0x1B => {
                self.ntf.push(Notification { ms: round3(m.ms()), handle: data[1] as i64 | (data[2] as i64) << 8, value: hex(&data[3..]) });
                return;
            }
            0x1D => {
                self.ntf.push(Notification {
                    ms: round3(m.ms()),
                    handle: data[1] as i64 | (data[2] as i64) << 8,
                    value: hex(&data[3..]) + " (ind)",
                });
                self.att_send(m, &[0x1E]);
                return;
            }
            0x02 => {
                self.records.insert("peripheral_mtu_req".into(), Rec::I(data[1] as i64 | (data[2] as i64) << 8));
                self.att_send(m, &[0x03, 247, 0]);
                return;
            }
            0x04 | 0x08 | 0x0A | 0x0C | 0x10 | 0x12 => {
                self.att_send(m, &[0x01, op, 0, 0, 0x06]);
                return;
            }
            _ => {}
        }
        if self.att_waiting {
            self.att_waiting = false;
            let f = self.script_step_fn.clone();
            f(self, m, ScriptValue::Bytes(data.to_vec()));
        }
    }

    pub fn script_step(&mut self, m: &mut Machine, value: ScriptValue) {
        let f = self.script_step_fn.clone();
        f(self, m, value);
    }

    pub fn script_step_default(&mut self, m: &mut Machine, value: ScriptValue) {
        let Some(script) = self.script.clone() else {
            return;
        };
        let (req, pair, done) = script.next(self, m, &value);
        if done {
            return;
        }
        if pair {
            let f = self.start_pairing_fn.clone();
            f(self, m);
            return;
        }
        self.att_waiting = true;
        self.att_send(m, &req.unwrap_or_default());
    }

    pub(crate) fn smp_send(&mut self, d: &[u8]) {
        self.queue_l2cap(6, d);
    }

    pub fn start_pairing(&mut self, m: &mut Machine) {
        let f = self.start_pairing_fn.clone();
        f(self, m);
    }

    pub fn start_pairing_sc(&mut self, m: &mut Machine, sc: bool) {
        let (mut auth, mut io) = (0x01u8, 0x03u8);
        if sc {
            auth |= 0x08;
            io = self.io_cap;
            if self.mitm {
                auth |= 0x04;
            }
        }
        let key_size = match self.fault.as_str() {
            "keysize7" => 7,
            "keysize6" => 6,
            _ => 0x10u8,
        };
        self.smp = Smp {
            preq: vec![0x01, io, 0x00, auth, key_size, self.init_key_dist, 0x03],
            t0: round3(m.ms()),
            started: true,
            passkey: -1,
            ..Smp::default()
        };
        self.flash_log_start = m.flash.log.len();
        let preq = self.smp.preq.clone();
        self.smp_send(&preq);
    }

    pub(crate) fn sc_method(&self) -> &'static str {
        let s = &self.smp;
        let rio = s.pres[1];
        if (s.preq[3] | s.pres[3]) & 0x04 == 0 || rio == 0x03 || self.io_cap == 0x03 {
            return "jw";
        }
        "passkey-display"
    }

    pub(crate) fn sc_start(&mut self) {
        let order = sc::p256_order();
        loop {
            let private = match self.sc_priv.clone() {
                Some(p) => p,
                None => self.rng.getrandbits_big(256),
            };
            if !private.is_zero() && private.cmp(&order) == std::cmp::Ordering::Less {
                self.smp.private = Some(private);
                break;
            }
            self.sc_priv = None;
        }
        self.smp.pkb = None;
        self.smp.pka = Some(sc::public_key(self.smp.private.as_ref().unwrap()));
        self.smp.method = self.sc_method().into();
        self.smp.round = 0;
        if self.smp.method == "passkey-display" {
            self.smp.passkey = if self.passkey >= 0 { self.passkey } else { self.rng.randrange(1000000) as i64 };
            self.displayed_passkey = self.smp.passkey;
        }
        let mut pk = self.smp.pka.clone().unwrap();
        if self.fault == "offcurve" {
            pk[63] ^= 0x01;
        }
        let mut d = vec![0x0C];
        d.extend(pk);
        self.smp_send(&d);
    }

    pub(crate) fn sc_z(&self) -> u8 {
        let s = &self.smp;
        if s.method == "jw" {
            return 0;
        }
        0x80 | ((s.passkey >> (s.round as u64)) & 1) as u8
    }

    pub(crate) fn sc_send_confirm(&mut self) {
        self.smp.na = self.rng.bytes(16);
        let z = self.sc_z();
        let s = &self.smp;
        let mut cai = sc::f4(&s.pka.as_ref().unwrap()[..32], &s.pkb.as_ref().unwrap()[..32], &s.na, z);
        if self.fault == "confirm" {
            cai[0] ^= 0x01;
        }
        let mut d = vec![0x03];
        d.extend(cai);
        self.smp_send(&d);
    }

    pub(crate) fn sc_dhkey_check(&mut self) {
        let dh = sc::dhkey(self.smp.private.as_ref().unwrap(), self.smp.pkb.as_ref().unwrap());
        let (a1, a2) = (sc::addr7(self.init_a_type, &self.init_a), sc::addr7(self.adv_a_random, &self.adv_a));
        let (mac_key, mut ltk) = sc::f5(&dh, &self.smp.na, &self.smp.nb, &a1, &a2);
        self.smp.mac_key = mac_key;
        let key_size = (self.smp.preq[4] as usize).min(self.smp.pres[4] as usize);
        for b in ltk.iter_mut().skip(key_size) {
            *b = 0;
        }
        self.smp.ltk = hex(&ltk);
        let mut r = vec![0u8; 16];
        if self.smp.method != "jw" {
            r[..4].copy_from_slice(&(self.smp.passkey as u32).to_le_bytes());
        }
        self.smp.r = r;
        let s = &self.smp;
        let mut ea = sc::f6(&s.mac_key, &s.na, &s.nb, &s.r, &sc::iocap_of(&s.preq), &a1, &a2);
        if self.fault == "dhkey" {
            ea[0] ^= 0x01;
        }
        let mut d = vec![0x0D];
        d.extend(ea);
        self.smp_send(&d);
        self.enc.ltk = aes128::from_le(&ltk);
    }

    pub(crate) fn queue_enc_req(&mut self) {
        self.enc.skdm = Some(self.rng.bytes(8));
        self.enc.ivm = self.rng.bytes(4);
        let mut p = vec![0x03];
        p.extend_from_slice(&[0u8; 10]);
        p.extend_from_slice(self.enc.skdm.as_ref().unwrap());
        p.extend_from_slice(&self.enc.ivm.clone());
        self.queue_ll(p);
    }

    pub(crate) fn sc_rx(&mut self, m: &mut Machine, code: u8, d: &[u8]) -> bool {
        if code == 0x0C {
            let pkb = d[1..65].to_vec();
            self.smp.pkb_on_curve = sc::on_curve(&pkb);
            self.smp.pkb = Some(pkb);
            if !self.smp.pkb_on_curve {
                self.smp_send(&[0x05, 0x0B]);
                return true;
            }
            if self.fault == "stall" {
                return true;
            }
            if self.smp.method == "passkey-display" {
                self.sc_send_confirm();
            }
            return true;
        }
        if code == 0x03 && self.smp.pkb.is_some() {
            self.smp.cb = d[1..17].to_vec();
            if self.fault == "stall" {
                return true;
            }
            if self.smp.method == "jw" {
                self.smp.na = self.rng.bytes(16);
            }
            let mut p = vec![0x04];
            p.extend_from_slice(&self.smp.na.clone());
            self.smp_send(&p);
            return true;
        }
        if code == 0x04 && self.smp.pkb.is_some() {
            self.smp.nb = d[1..17].to_vec();
            let z = self.sc_z();
            let s = &self.smp;
            let ok = sc::f4(&s.pkb.as_ref().unwrap()[..32], &s.pka.as_ref().unwrap()[..32], &s.nb, z) == s.cb;
            self.smp.confirms_ok.push(ok);
            if !ok {
                self.smp.sconfirm_ok = false;
                self.smp_send(&[0x05, 0x04]);
                let f = self.script_step_fn.clone();
                f(self, m, ScriptValue::Pair(PairResult { ok: false, reason: 0x04 }));
                return true;
            }
            self.smp.round += 1;
            if self.smp.method != "jw" && self.smp.round < 20 {
                self.sc_send_confirm();
                return true;
            }
            self.smp.sconfirm_ok = true;
            self.sc_dhkey_check();
            return true;
        }
        if code == 0x0D {
            let (a1, a2) = (sc::addr7(self.init_a_type, &self.init_a), sc::addr7(self.adv_a_random, &self.adv_a));
            let s = &self.smp;
            let eb = sc::f6(&s.mac_key, &s.nb, &s.na, &s.r, &sc::iocap_of(&s.pres), &a2, &a1);
            self.smp.has_dhkey_check = true;
            self.smp.dhkey_check_ok = d[1..17] == eb[..];
            if !self.smp.dhkey_check_ok {
                self.smp_send(&[0x05, 0x0B]);
                let f = self.script_step_fn.clone();
                f(self, m, ScriptValue::Pair(PairResult { ok: false, reason: 0x0B }));
                return true;
            }
            self.queue_enc_req();
            return true;
        }
        false
    }

    pub(crate) fn smp_rx(&mut self, m: &mut Machine, d: &[u8]) {
        let code = d[0];
        if code == 0x02 && d[3] & self.smp.preq[3] & 0x08 != 0 {
            self.smp.pres = d[..7].to_vec();
            self.sc_start();
        } else if self.smp.pka.is_some() && self.sc_rx(m, code, d) {
        } else if code == 0x02 {
            self.smp.pres = d[..7].to_vec();
            self.smp.mrand = self.rng.bytes(16);
            let mc = aes128::c1(
                0,
                aes128::from_le(&self.smp.mrand),
                &self.smp.preq,
                &self.smp.pres,
                self.init_a_type as u32,
                &self.init_a,
                self.adv_a_random as u32,
                &self.adv_a,
            );
            let mut p = vec![0x03];
            p.extend(aes128::bytes_le(mc, 16));
            self.smp_send(&p);
        } else if code == 0x03 {
            self.smp.sconfirm = d[1..17].to_vec();
            let mut p = vec![0x04];
            p.extend_from_slice(&self.smp.mrand.clone());
            self.smp_send(&p);
        } else if code == 0x04 {
            self.smp.srand = d[1..17].to_vec();
            let want = aes128::c1(
                0,
                aes128::from_le(&self.smp.srand),
                &self.smp.preq,
                &self.smp.pres,
                self.init_a_type as u32,
                &self.init_a,
                self.adv_a_random as u32,
                &self.adv_a,
            );
            self.smp.sconfirm_ok = aes128::bytes_le(want, 16) == self.smp.sconfirm;
            let stk = aes128::s1(0, aes128::from_le(&self.smp.srand), aes128::from_le(&self.smp.mrand));
            self.enc.ltk = stk;
            self.queue_enc_req();
        } else if code == 0x05 {
            self.smp.failed = d[1] as i64;
            let f = self.script_step_fn.clone();
            f(self, m, ScriptValue::Pair(PairResult { ok: false, reason: d[1] }));
        } else if (0x06..=0x09).contains(&code) {
            let name = match code {
                6 => "LTK",
                7 => "EDIV_Rand",
                8 => "IRK",
                _ => "identity_address",
            };
            self.smp.keys.insert(name.into(), hex(&d[1..]));
            if code == 0x09 {
                self.smp_complete(m);
            }
        } else if code == 0x0B {
            self.smp.security_request = hex(d);
        }
    }

    pub(crate) fn smp_complete(&mut self, m: &mut Machine) {
        if self.smp.done_ms != 0.0 {
            return;
        }
        if self.smp.pres.len() >= 6 && self.smp.preq[5] & self.smp.pres[5] & 0x02 != 0 {
            let mut p = vec![0x08];
            p.extend_from_slice(&self.irk.clone());
            self.smp_send(&p);
            let mut p = vec![0x09, 0x00];
            p.extend_from_slice(&self.identity.clone());
            self.smp_send(&p);
            let mut id = vec![0u8];
            id.extend_from_slice(&self.identity);
            self.smp.sent_keys = BTreeMap::from([("IRK".to_string(), hex(&self.irk)), ("identity_address".to_string(), hex(&id))]);
        }
        self.smp.done_ms = round3(m.ms());
        if let Some(cb) = self.on_bond.clone() {
            cb();
        }
        let f = self.script_step_fn.clone();
        f(self, m, ScriptValue::Pair(PairResult { ok: true, reason: 0 }));
    }

    pub(crate) fn hook_event(&mut self, m: &mut Machine, k: i64) {
        if self.script.is_some() && !self.att_started && k >= 4 {
            self.att_started = true;
            let f = self.script_step_fn.clone();
            f(self, m, ScriptValue::Nil);
        }
    }

    pub fn rec(&mut self, m: &Machine, kind: &str, text: String) {
        let line = format!("[{:9.3}] {:<3} {}", m.ms(), kind, text);
        if let Some(l) = &self.log {
            l(&line);
        }
        self.transcript.push(line);
    }
}

impl Peer for RefCell<Central> {
    fn on_adv(&self, m: &mut Machine, pdu: &[u8], ch: u8) -> Option<Vec<u8>> {
        let f = self.borrow().on_adv_fn.clone();
        f(&mut self.borrow_mut(), m, pdu, ch)
    }
    fn on_connect_sent(&self, m: &mut Machine, pdu: &[u8], t_end: f64) {
        self.borrow_mut().on_connect_sent(m, pdu, t_end)
    }
    fn drop_pdu(&self, m: &mut Machine, pdu: &[u8]) -> bool {
        self.borrow_mut().drop_pdu(m, pdu)
    }
    fn on_air(&self, m: &mut Machine, pdu: &[u8], ch: u8, tx_addr: u32) -> Option<Vec<u8>> {
        self.borrow_mut().on_air(m, pdu, ch, tx_addr)
    }
    fn air_lost(&self, m: &mut Machine, pdu: &[u8]) {
        self.borrow_mut().air_lost(m, pdu)
    }
    fn air_lost_tx(&self, m: &mut Machine, pdu: &[u8]) {
        self.borrow_mut().air_lost_tx(m, pdu)
    }
    fn scan_request(&self, m: &mut Machine, pdu: &[u8], ch: u8) -> Option<Vec<u8>> {
        self.borrow_mut().scan_request(m, pdu, ch)
    }
}
