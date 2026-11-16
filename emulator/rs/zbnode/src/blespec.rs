use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::rc::Rc;

use ble::central::LlCtrlFn;
use ble::gofmt::{f as ff, hex, unhex};
use ble::{default_options, Air, Central, CentralRc, Peer, PeerRc, RadioRef, TO_HOST, TO_KEYBOARD};
use tc32emu::{parse_go_int, Machine};

use crate::gostr::{atoi, go_quote, parse_float};

#[derive(Clone)]
struct SpecPdu {
    ms: f64,
    event: i64,
    ch: u8,
    fresh: bool,
    pdu: Vec<u8>,
}

struct SpecAdvReply {
    left: i64,
    ch: i64,
    pdu: Vec<u8>,
}

struct SpecAdvSent {
    ms: f64,
    ch: u8,
    pdu: Vec<u8>,
}

#[derive(Default)]
pub struct SpecPeer {
    c: Option<CentralRc>,
    pdus: Vec<SpecPdu>,
    lost: Vec<SpecPdu>,
    replies: Vec<SpecAdvReply>,
    adv_sent: Vec<SpecAdvSent>,
    last_fake: bool,
    ll_auto: bool,
    version_sent: bool,
    sca: i64,
}

pub struct SpecCell(RefCell<SpecPeer>);

impl SpecCell {
    fn borrow(&self) -> std::cell::Ref<'_, SpecPeer> {
        self.0.borrow()
    }
    fn borrow_mut(&self) -> std::cell::RefMut<'_, SpecPeer> {
        self.0.borrow_mut()
    }
}

pub type SpecRc = Rc<SpecCell>;

impl Peer for SpecCell {
    fn on_adv(&self, m: &mut Machine, pdu: &[u8], ch: u8) -> Option<Vec<u8>> {
        let c = {
            let mut p = self.borrow_mut();
            if pdu.len() >= 8 && pdu[0] & 0xF == 0 {
                let ms = m.ms();
                let mut hit = None;
                for r in p.replies.iter_mut() {
                    if r.left > 0 && (r.ch < 0 || r.ch == ch as i64) {
                        r.left -= 1;
                        hit = Some(r.pdu.clone());
                        break;
                    }
                }
                if let Some(out) = hit {
                    p.last_fake = true;
                    p.adv_sent.push(SpecAdvSent { ms, ch, pdu: out.clone() });
                    return Some(out);
                }
            }
            p.c.clone()?
        };
        let rsp = c.on_adv(m, pdu, ch);
        if let Some(mut rsp) = rsp {
            let mut p = self.borrow_mut();
            p.last_fake = false;
            if p.sca >= 0 && rsp[0] & 0xF == 5 && rsp.len() == 36 {
                rsp[35] = rsp[35] & 0x1F | (p.sca << 5) as u8;
            }
            return Some(rsp);
        }
        None
    }

    fn on_connect_sent(&self, m: &mut Machine, pdu: &[u8], t_end: f64) {
        let c = {
            let p = self.borrow();
            if p.last_fake {
                return;
            }
            match &p.c {
                None => return,
                Some(c) => c.clone(),
            }
        };
        c.borrow_mut().on_connect_sent(m, pdu, t_end);
    }

    fn drop_pdu(&self, m: &mut Machine, pdu: &[u8]) -> bool {
        let Some(c) = self.borrow().c.clone() else {
            return true;
        };
        let dropped = c.borrow_mut().drop_pdu(m, pdu);
        if dropped {
            let (event, ch) = {
                let cc = c.borrow();
                (cc.event, cc.cur_ch)
            };
            self.borrow_mut().lost.push(SpecPdu { ms: m.ms(), event, ch, fresh: false, pdu: pdu.to_vec() });
            return true;
        }
        false
    }

    fn on_air(&self, m: &mut Machine, pdu: &[u8], ch: u8, tx_addr: u32) -> Option<Vec<u8>> {
        let c = self.borrow().c.clone()?;
        let (event, nesn) = {
            let cc = c.borrow();
            (cc.event, cc.nesn)
        };
        let fresh = ((pdu[0] >> 3) & 1) as i64 == nesn;
        self.borrow_mut().pdus.push(SpecPdu { ms: m.ms(), event, ch, fresh, pdu: pdu.to_vec() });
        let r = c.borrow_mut().on_air(m, pdu, ch, tx_addr);
        r
    }

    fn scan_request(&self, m: &mut Machine, pdu: &[u8], ch: u8) -> Option<Vec<u8>> {
        let c = self.borrow().c.clone()?;
        let r = c.borrow_mut().scan_request(m, pdu, ch);
        r
    }

    fn air_lost(&self, m: &mut Machine, pdu: &[u8]) {
        let Some(c) = self.borrow().c.clone() else {
            return;
        };
        let (event, ch) = {
            let cc = c.borrow();
            (cc.event, cc.cur_ch)
        };
        self.borrow_mut().lost.push(SpecPdu { ms: m.ms(), event, ch, fresh: false, pdu: pdu.to_vec() });
        c.borrow_mut().air_lost(m, pdu);
    }

    fn air_lost_tx(&self, m: &mut Machine, pdu: &[u8]) {
        let Some(c) = self.borrow().c.clone() else {
            return;
        };
        c.borrow_mut().air_lost_tx(m, pdu);
    }
}

fn spec_ll_ctrl(spec: &SpecRc, orig: LlCtrlFn) -> LlCtrlFn {
    let spec = spec.clone();
    Rc::new(move |c: &mut Central, m: &mut Machine, pl: &[u8]| {
        let op = pl[0];
        if op == 0x02 {
            orig(c, m, pl);
            return;
        }
        if !spec.borrow().ll_auto {
            return;
        }
        match op {
            0x0C => {
                let mut s = spec.borrow_mut();
                if !s.version_sent {
                    s.version_sent = true;
                    drop(s);
                    c.queue_ll(vec![0x0C, 0x0B, 0x0F, 0x00, 0x34, 0x12]);
                }
            }
            0x0E => c.queue_ll(vec![0x09, 0, 0, 0, 0, 0, 0, 0, 0]),
            0x14 => c.queue_ll(vec![0x15, 27, 0, 0x48, 0x01, 27, 0, 0x48, 0x01]),
            0x12 => c.queue_ll(vec![0x13]),
            0x0F | 0x16 => c.queue_ll(vec![0x07, op]),
            0x00 | 0x01 | 0x03 | 0x04 | 0x05 | 0x06 | 0x07 | 0x08 | 0x09 | 0x0A | 0x0B | 0x0D | 0x10 | 0x11 | 0x13 | 0x15 => {}
            _ => c.queue_ll(vec![0x07, op]),
        }
    })
}

fn spec_kv(f: &[String]) -> BTreeMap<String, String> {
    let mut kv = BTreeMap::new();
    for x in f {
        if let Some(i) = x.find('=') {
            if i > 0 {
                kv.insert(x[..i].to_string(), x[i + 1..].to_string());
            }
        }
    }
    kv
}

fn new_spec() -> SpecRc {
    Rc::new(SpecCell(RefCell::new(SpecPeer::default())))
}

#[derive(Default)]
pub struct Spec {
    spec: Option<SpecRc>,
}

fn replace_state(c: &CentralRc) {
    let mut cc = c.borrow_mut();
    if cc.state == "connected" || cc.state == "terminate-ack" {
        cc.state = "replaced";
    }
}

impl Spec {
    fn central(&mut self, m: &mut Machine, radio: &RadioRef, kv: &BTreeMap<String, String>) -> Result<(), String> {
        let num = |k: &str, d: i64| -> i64 {
            if let Some(v) = kv.get(k) {
                if let Some(n) = parse_go_int(v) {
                    return n;
                }
            }
            d
        };
        let flt = |k: &str, d: f64| -> f64 {
            if let Some(v) = kv.get(k) {
                if let Some(x) = parse_float(v) {
                    return x;
                }
            }
            d
        };
        let mut o = default_options();
        o.log = None;
        o.pair = false;
        o.interval = num("interval", 24);
        o.latency = num("latency", 0);
        o.timeout = num("timeout", 400);
        o.hop = num("hop", 7);
        o.win_size = num("winsize", 2);
        o.win_offset = num("winoffset", 0);
        o.anchor_in_window_ms = flt("anchor", 0.3);
        o.max_exch = num("maxexch", 8);
        o.connect_after_ms = m.ms() + flt("after", 0.0);
        o.active_scan = num("scan", 0) != 0;
        o.send_version = num("version", 0) != 0;
        o.send_features = num("features", 0) != 0;
        o.no_terminate_ack = num("noack", 0) != 0;
        o.ll_rules = num("llrules", 0) != 0;
        o.update_lead = num("lead", 0);
        if let Some(v) = kv.get("chm") {
            let (b, ok) = unhex(v);
            if !ok || b.len() != 5 {
                return Err(format!("chm: {}", go_quote(v)));
            }
            o.chm = b;
        }
        if let Some(v) = kv.get("aa") {
            match crate::gostr::parse_uint_radix(v, 16, 32) {
                Some(x) => o.aa = x as u32,
                None => return Err(format!("aa: {}", go_quote(v))),
            }
        }
        if o.interval <= 0 {
            return Err(format!("interval: {}", o.interval));
        }
        let spec = self.spec.get_or_insert_with(new_spec).clone();
        if let Some(c) = spec.borrow().c.clone() {
            replace_state(&c);
        }
        let send_version = o.send_version;
        let c = Central::new(m, radio, o);
        let peer: PeerRc = spec.clone();
        radio.b().central = Some(peer);
        {
            let mut s = spec.borrow_mut();
            s.c = Some(c.clone());
            s.pdus.clear();
            s.lost.clear();
            s.ll_auto = num("llauto", 1) != 0;
            s.version_sent = send_version;
            s.sca = num("sca", -1);
        }
        let mut cc = c.borrow_mut();
        cc.rx_data = Some(Rc::new(|_: u8, _: &[u8]| {}));
        let orig = cc.ll_ctrl_fn.clone();
        cc.ll_ctrl_fn = spec_ll_ctrl(&spec, orig);
        Ok(())
    }

    fn need(&self, out: &mut String, name: &str) -> Option<CentralRc> {
        if let Some(s) = &self.spec {
            if let Some(c) = &s.borrow().c {
                return Some(c.clone());
            }
        }
        let _ = writeln!(out, "ERROR {}: no central (BSCENTRAL first)", name);
        None
    }

    pub fn command(&mut self, f: &[String], out: &mut String, m: &mut Machine, radio: &RadioRef) -> bool {
        if !f[0].starts_with("BS") {
            return false;
        }
        let arg = |i: usize| -> &str {
            if i < f.len() {
                f[i].as_str()
            } else {
                ""
            }
        };
        let num = |i: usize| -> i64 { crate::gostr::parse_int_base0_lossy(arg(i)) };
        match f[0].as_str() {
            "BSCENTRAL" => {
                let kv = spec_kv(&f[1..]);
                match self.central(m, radio, &kv) {
                    Err(e) => {
                        let _ = writeln!(out, "ERROR BSCENTRAL {}", e);
                    }
                    Ok(()) => out.push_str("OK\n"),
                }
            }
            "BSDETACH" => {
                let spec = self.spec.get_or_insert_with(new_spec).clone();
                let c = spec.borrow_mut().c.take();
                if let Some(c) = c {
                    replace_state(&c);
                }
                let peer: PeerRc = spec;
                radio.b().central = Some(peer);
                out.push_str("OK\n");
            }
            "BSADVREPLY" => {
                if self.spec.is_none() {
                    let s = new_spec();
                    self.spec = Some(s.clone());
                    let peer: PeerRc = s;
                    radio.b().central = Some(peer);
                }
                let (pdu, ok) = unhex(arg(3));
                if !ok || pdu.len() < 2 {
                    out.push_str("ERROR BSADVREPLY pdu\n");
                    return true;
                }
                let spec = self.spec.clone().unwrap();
                let mut s = spec.borrow_mut();
                if num(1) <= 0 {
                    s.replies.clear();
                } else {
                    s.replies.push(SpecAdvReply { left: num(1), ch: num(2), pdu });
                }
                out.push_str("OK\n");
            }
            "BSADVSENT" => {
                if let Some(s) = &self.spec {
                    let s = s.borrow();
                    let from = num(1).max(0) as usize;
                    for a in s.adv_sent.iter().skip(from) {
                        let _ = writeln!(out, "A {} {} {}", ff(a.ms, 6), a.ch, hex(&a.pdu));
                    }
                }
                out.push_str("END\n");
            }
            "BSSTATE" => {
                let c = self.spec.as_ref().and_then(|s| s.borrow().c.clone());
                let Some(c) = c else {
                    out.push_str("BSSTATE state=none\n");
                    return true;
                };
                let (npdus, nlost) = {
                    let s = self.spec.as_ref().unwrap().borrow();
                    (s.pdus.len(), s.lost.len())
                };
                let nmisses = radio.b().misses.len();
                let c = c.borrow();
                let adva = if c.adv_a.is_empty() { "-".to_string() } else { hex(&c.adv_a) };
                let _ = writeln!(
                    out,
                    "BSSTATE state={} event={} sn={} nesn={} ch={} heard={} events={} replies={} missed={} retx={} pdus={} lost={} log={} sent={} misses={} interval_ms={} anchor0={} tconnend={} adva={} advarandom={} now={}",
                    c.state,
                    c.event,
                    c.sn,
                    c.nesn,
                    c.cur_ch,
                    c.heard,
                    c.stats.events,
                    c.stats.replies,
                    c.stats.missed,
                    c.stats.retx,
                    npdus,
                    nlost,
                    c.transcript.len(),
                    c.sent.len(),
                    nmisses,
                    ff(c.interval_ms, 6),
                    ff(c.anchor0, 6),
                    ff(c.t_conn_end, 6),
                    adva,
                    c.adv_a_random,
                    ff(m.ms(), 6)
                );
            }
            "BSPDUS" => {
                if let Some(s) = &self.spec {
                    let s = s.borrow();
                    for x in s.pdus.iter().skip(num(1).max(0) as usize) {
                        let _ = writeln!(out, "P {} {} {} {} {}", ff(x.ms, 6), x.event, x.ch, x.fresh as i32, hex(&x.pdu));
                    }
                }
                out.push_str("END\n");
            }
            "BSLOST" => {
                if let Some(s) = &self.spec {
                    let s = s.borrow();
                    for x in s.lost.iter().skip(num(1).max(0) as usize) {
                        let _ = writeln!(out, "X {} {} {} {}", ff(x.ms, 6), x.event, x.ch, hex(&x.pdu));
                    }
                }
                out.push_str("END\n");
            }
            "BSLOG" => {
                if let Some(c) = self.spec.as_ref().and_then(|s| s.borrow().c.clone()) {
                    for l in c.borrow().transcript.iter().skip(num(1).max(0) as usize) {
                        let _ = writeln!(out, "L {}", l);
                    }
                }
                out.push_str("END\n");
            }
            "BSSENT" => {
                if let Some(c) = self.spec.as_ref().and_then(|s| s.borrow().c.clone()) {
                    for s in c.borrow().sent.iter().skip(num(1).max(0) as usize) {
                        let _ = writeln!(out, "S {} {}", s[0] as i64, ff(s[1], 6));
                    }
                }
                out.push_str("END\n");
            }
            "BSMISSES" => {
                let r = radio.b();
                for x in r.misses.iter().skip(num(1).max(0) as usize) {
                    let _ = writeln!(out, "M {} {} {}", ff(x.ms, 6), x.ch, x.why);
                }
                out.push_str("END\n");
            }
            "BSLL" => {
                let Some(c) = self.need(out, &f[0]) else {
                    return true;
                };
                let (pl, ok) = unhex(arg(1));
                if !ok || pl.is_empty() {
                    out.push_str("ERROR BSLL payload\n");
                    return true;
                }
                if pl[0] == 0x0C {
                    self.spec.as_ref().unwrap().borrow_mut().version_sent = true;
                }
                c.borrow_mut().queue_ll(pl);
                out.push_str("OK\n");
            }
            "BSDATA" => {
                let Some(c) = self.need(out, &f[0]) else {
                    return true;
                };
                let (pl, ok) = unhex(arg(2));
                if !ok {
                    out.push_str("ERROR BSDATA payload\n");
                    return true;
                }
                c.borrow_mut().queue_data(num(1) as u8, &pl, None);
                out.push_str("OK\n");
            }
            "BSL2CAP" => {
                let Some(c) = self.need(out, &f[0]) else {
                    return true;
                };
                let (pl, ok) = unhex(arg(2));
                if !ok {
                    out.push_str("ERROR BSL2CAP payload\n");
                    return true;
                }
                c.borrow_mut().queue_l2cap(num(1), &pl);
                out.push_str("OK\n");
            }
            "BSUPDATE" => {
                let Some(c) = self.need(out, &f[0]) else {
                    return true;
                };
                if num(3) <= 0 {
                    out.push_str("ERROR BSUPDATE interval\n");
                    return true;
                }
                c.borrow_mut().connection_update(num(1), num(2), num(3), num(4), num(5), num(6));
                out.push_str("OK\n");
            }
            "BSCHM" => {
                let Some(c) = self.need(out, &f[0]) else {
                    return true;
                };
                let (chm, ok) = unhex(arg(1));
                if !ok || chm.len() != 5 {
                    out.push_str("ERROR BSCHM map\n");
                    return true;
                }
                c.borrow_mut().channel_map_update(&chm, num(2));
                out.push_str("OK\n");
            }
            "BSTERM" => {
                let Some(c) = self.need(out, &f[0]) else {
                    return true;
                };
                c.borrow_mut().terminate(num(1) as u8);
                out.push_str("OK\n");
            }
            "BSSILENT" => {
                let Some(c) = self.need(out, &f[0]) else {
                    return true;
                };
                if arg(1) == "off" {
                    c.borrow_mut().go_silent(1 << 30);
                } else {
                    c.borrow_mut().go_silent(num(1));
                }
                out.push_str("OK\n");
            }
            "BSDROP" => {
                let Some(c) = self.need(out, &f[0]) else {
                    return true;
                };
                c.borrow_mut().drop_at.insert(num(1));
                out.push_str("OK\n");
            }
            "BSAIR" => {
                if arg(1) == "off" {
                    radio.b().air = None;
                } else {
                    let mut r = radio.b();
                    if r.air.is_none() {
                        r.air = Some(Air::new(1, 0, 0, &[], 0, 0.0, 0).unwrap());
                    }
                    let air = r.air.as_mut().unwrap();
                    let kv = spec_kv(&f[1..]);
                    if let Some(v) = kv.get("lossk") {
                        air.loss[TO_KEYBOARD] = atoi(v);
                    }
                    if let Some(v) = kv.get("lossh") {
                        air.loss[TO_HOST] = atoi(v);
                    }
                    if let Some(v) = kv.get("jitter") {
                        air.jitter_us = atoi(v);
                    }
                    if let Some(v) = kv.get("ppm") {
                        air.clock_ppm = parse_float(v).unwrap_or(0.0);
                    }
                    if let Some(v) = kv.get("blackout") {
                        air.blackout_until = parse_float(v).unwrap_or(0.0);
                    }
                }
                if let Some(c) = self.spec.as_ref().and_then(|s| s.borrow().c.clone()) {
                    c.borrow_mut().reclock();
                }
                out.push_str("OK\n");
            }
            _ => {
                let _ = writeln!(out, "ERROR unknown command {}", f[0]);
            }
        }
        true
    }
}
