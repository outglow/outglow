use std::collections::BTreeMap;

use super::util::verified;
use super::mixed::{ieee_of, mac_record, Fw};
use super::{node_of, Sim, SimCtx, SimDef};
use crate::air::{Ev, NodeId};
use crate::coord::{Coord, NWK_KEY};
use crate::crypto::{ccm_decrypt, ccm_encrypt};
use crate::frames::{airtime_us, mac_parse, with_fcs, Key};
use crate::peer::Peer;
use crate::session::R;
use crate::station::{nx, Station, StationEv};

pub fn def() -> SimDef {
    SimDef { name: "rust_gp", doc: "Green Power proxy of the Rust firmware next to the stock firmware's", run }
}

const QUANTUM: f64 = 0.25;
const REC_R: [u8; 8] = mac_record([0x01, 0x00, 0x52, 0x00, 0x00]);
const REC_T: [u8; 8] = mac_record([0x02, 0x00, 0x54, 0x00, 0x00]);
const EXT_R: u64 = ieee_of(Fw::Rust, &REC_R);
const EXT_T: u64 = ieee_of(Fw::Stock, &REC_T);
const GP_EP: u8 = 242;
const GP_PROFILE: u16 = 0xA1E0;
const GP_CLUSTER: u16 = 0x0021;
const SWITCH: u32 = 0x01712345;
const SENSOR: u64 = 0x0011223344556677;

fn switch_key() -> Key {
    let mut k = [0u8; 16];
    for (i, b) in k.iter_mut().enumerate() {
        *b = 0xC0 + i as u8;
    }
    k
}

fn shared_key() -> Key {
    let mut k = [0u8; 16];
    for (i, b) in k.iter_mut().enumerate() {
        *b = 0x50 + i as u8;
    }
    k
}

fn hx(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn hex_or_none(v: &Option<Vec<u8>>) -> String {
    match v {
        Some(b) if !b.is_empty() => hx(b),
        _ => "None".into(),
    }
}

fn got_text(r: &Option<Vec<u8>>, t: &Option<Vec<u8>>) -> String {
    let f = |v: &Option<Vec<u8>>| match v {
        None => "None".to_string(),
        Some(b) if b.is_empty() => "b''".to_string(),
        Some(b) => format!("'{}'", hx(b)),
    };
    format!("{{'R': {}, 'T': {}}}", f(r), f(t))
}

#[derive(Clone)]
struct Gpdf {
    cmd: u8,
    payload: Vec<u8>,
    src_id: u32,
    ieee: Option<u64>,
    endpoint: u8,
    key: Option<Key>,
    counter: u32,
    level: u8,
    individual: bool,
    rx_after_tx: bool,
    auto: bool,
    maintenance: bool,
    repeats: usize,
}

impl Gpdf {
    fn new(cmd: u8, payload: &[u8]) -> Gpdf {
        Gpdf {
            cmd,
            payload: payload.to_vec(),
            src_id: SWITCH,
            ieee: None,
            endpoint: 0,
            key: None,
            counter: 0,
            level: 3,
            individual: true,
            rx_after_tx: false,
            auto: false,
            maintenance: false,
            repeats: 3,
        }
    }
}

struct Gp {
    s: Sim,
    gpd: NodeId,
    gpd_seq: u8,
    short: BTreeMap<&'static str, u16>,
    ids: BTreeMap<&'static str, NodeId>,
}

impl Gp {
    fn now(&self) -> f64 {
        self.s.now()
    }

    fn send_mpdu(&mut self, mpdu: &[u8], delay_ms: f64) -> f64 {
        let psdu = with_fcs(mpdu);
        let t0 = self.s.w.air.now() + delay_ms;
        let t1 = t0 + airtime_us(psdu.len()) / 1000.0;
        self.s.w.air.at(t0, Ev::Node(self.gpd, StationEv::Transmit { psdu, t0, t1 }));
        t1
    }

    fn frame(&mut self, f: Gpdf) -> f64 {
        self.gpd_seq = self.gpd_seq.wrapping_add(1);
        let seq = self.gpd_seq;
        let mut body = vec![f.cmd];
        body.extend_from_slice(&f.payload);
        let mut mac;
        let mut nwk;
        if f.maintenance {
            mac = 0x0801u16.to_le_bytes().to_vec();
            mac.push(seq);
            mac.extend_from_slice(&[0xff, 0xff, 0xff, 0xff]);
            nwk = vec![0x0D | if f.auto { 0x40 } else { 0 }];
        } else {
            let ext = (if f.key.is_some() { f.level << 3 | if f.individual { 0x20 } else { 0 } } else { 0 })
                | if f.rx_after_tx { 0x40 } else { 0 };
            let fc = 0x0C | if f.auto { 0x40 } else { 0 };
            let mut nonce = Vec::new();
            match f.ieee {
                Some(ieee) => {
                    mac = 0xC841u16.to_le_bytes().to_vec();
                    mac.push(seq);
                    mac.extend_from_slice(&[0xff, 0xff, 0xff, 0xff]);
                    mac.extend_from_slice(&ieee.to_le_bytes());
                    nwk = vec![fc | 0x80, ext | 0x02, f.endpoint];
                    nonce.extend_from_slice(&ieee.to_le_bytes());
                    nonce.extend_from_slice(&f.counter.to_le_bytes());
                    nonce.push(0x05);
                }
                None => {
                    mac = 0x0801u16.to_le_bytes().to_vec();
                    mac.push(seq);
                    mac.extend_from_slice(&[0xff, 0xff, 0xff, 0xff]);
                    nwk = if ext != 0 { vec![fc | 0x80, ext] } else { vec![fc] };
                    nwk.extend_from_slice(&f.src_id.to_le_bytes());
                    nonce.extend_from_slice(&f.src_id.to_le_bytes());
                    nonce.extend_from_slice(&f.src_id.to_le_bytes());
                    nonce.extend_from_slice(&f.counter.to_le_bytes());
                    nonce.push(0x05);
                }
            }
            if let Some(key) = f.key {
                nwk.extend_from_slice(&f.counter.to_le_bytes());
                let n: [u8; 13] = nonce[..13].try_into().unwrap();
                if f.level == 3 {
                    body = ccm_encrypt(&key, &n, &nwk, &body);
                } else {
                    let mut aad = nwk.clone();
                    aad.extend_from_slice(&body);
                    body.extend(ccm_encrypt(&key, &n, &aad, &[]));
                }
            }
        }
        let mut mpdu = mac;
        mpdu.extend_from_slice(&nwk);
        mpdu.extend_from_slice(&body);
        let mut t1 = 0.0;
        for i in 0..f.repeats {
            t1 = self.send_mpdu(&mpdu, 1.0 + 5.0 * i as f64);
        }
        t1
    }

    fn gp_send(&mut self, dst: u16, cmd: u8, payload: &[u8], specific: bool) -> u8 {
        let c = &mut self.s.w.coord;
        let seq = nx(&mut c.st.zcl_seq);
        let fc = if specific { 0x01 } else { 0x00 } | 0x08 | 0x10;
        let mut aps = vec![if dst >= 0xFFF8 { 0x08 } else { 0x00 }, GP_EP];
        aps.extend_from_slice(&GP_CLUSTER.to_le_bytes());
        aps.extend_from_slice(&GP_PROFILE.to_le_bytes());
        aps.push(GP_EP);
        aps.push(nx(&mut c.st.aps_ctr));
        aps.extend_from_slice(&[fc, seq, cmd]);
        aps.extend_from_slice(payload);
        c.nwk_send_data(&mut self.s.w.air, dst, &aps);
        seq
    }

    fn broadcast(&mut self, cmd: u8, payload: &[u8]) -> R<()> {
        for _ in 0..2 {
            self.gp_send(0xFFFD, cmd, payload, true);
            self.s.run(300.0)?;
        }
        Ok(())
    }

    fn gp_from(&self, short: u16, cmd: u8, since: f64) -> Vec<Vec<u8>> {
        self.s
            .w
            .coord
            .zcl_rsp
            .iter()
            .filter(|r| r.ms >= since && r.src == short && r.cluster == GP_CLUSTER && r.cmd == Some(cmd))
            .map(|r| r.body.clone())
            .collect()
    }

    fn both_answer(&mut self, send: &dyn Fn(&mut Gp, u16), cmd: u8) -> R<(Option<Vec<u8>>, Option<Vec<u8>>)> {
        let mut out = [None, None];
        for (k, name) in ["R", "T"].iter().enumerate() {
            let t0 = self.now();
            let sh = self.short[name];
            send(self, sh);
            self.run_until_gp(sh, cmd, t0, 1500.0)?;
            out[k] = self.gp_from(sh, cmd, t0).into_iter().next();
        }
        let [r, t] = out;
        Ok((r, t))
    }

    fn run_until_gp(&mut self, sh: u16, cmd: u8, t0: f64, max_ms: f64) -> R<bool> {
        self.s.run_until(max_ms, 10.0, |s| {
            s.w.coord.zcl_rsp.iter().any(|r| r.ms >= t0 && r.src == sh && r.cluster == GP_CLUSTER && r.cmd == Some(cmd))
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn pairing(&mut self, gpd_id: u32, ieee: Option<u64>, endpoint: u8, add: bool, remove_gpd: bool, mode: u32, group: u16,
               security: u32, key_type: u32, counter: Option<u32>, key: Option<Key>, alias: Option<u16>, radius: Option<u8>,
               seq_cap: bool) -> R<()> {
        let options: u32 = (if ieee.is_some() { 2 } else { 0 })
            | (if add && !remove_gpd { 1 << 3 } else { 0 })
            | (if remove_gpd { 1 << 4 } else { 0 })
            | mode << 5
            | (if seq_cap { 1 << 8 } else { 0 })
            | security << 9
            | key_type << 11
            | (if counter.is_some() { 1 << 14 } else { 0 })
            | (if key.is_some() { 1 << 15 } else { 0 })
            | (if alias.is_some() { 1 << 16 } else { 0 })
            | (if radius.is_some() { 1 << 17 } else { 0 });
        let mut p = options.to_le_bytes()[..3].to_vec();
        match ieee {
            Some(i) => {
                p.extend_from_slice(&i.to_le_bytes());
                p.push(endpoint);
            }
            None => p.extend_from_slice(&gpd_id.to_le_bytes()),
        }
        if !remove_gpd {
            if mode == 3 {
                p.extend_from_slice(&self.s.w.coord.ext.to_le_bytes());
                p.extend_from_slice(&0u16.to_le_bytes());
            } else {
                p.extend_from_slice(&group.to_le_bytes());
            }
            if add {
                p.push(0x02);
                if let Some(c) = counter {
                    p.extend_from_slice(&c.to_le_bytes());
                }
                if let Some(k) = key {
                    p.extend_from_slice(&k);
                }
                if let Some(a) = alias {
                    p.extend_from_slice(&a.to_le_bytes());
                }
                if let Some(r) = radius {
                    p.push(r);
                }
            }
        }
        self.broadcast(0x01, &p)
    }

    fn commissioning_mode(&mut self, enter: bool, unicast: bool, exit_on_pairing: bool) -> R<()> {
        let b = if enter { 0x01 | if exit_on_pairing { 0x04 } else { 0 } | if unicast { 0x20 } else { 0 } } else { 0x00 };
        self.broadcast(0x02, &[b])
    }

    fn table_entry(&mut self, gpd_id: u32, ieee: Option<u64>, endpoint: u8) -> R<(Option<Vec<u8>>, Option<Vec<u8>>)> {
        let req = match ieee {
            Some(i) => {
                let mut v = vec![0x02];
                v.extend_from_slice(&i.to_le_bytes());
                v.push(endpoint);
                v
            }
            None => {
                let mut v = vec![0x00];
                v.extend_from_slice(&gpd_id.to_le_bytes());
                v
            }
        };
        self.both_answer(&move |g: &mut Gp, s| {
            g.gp_send(s, 0x0B, &req, true);
        }, 0x0B)
    }

    fn same_notification(&mut self, cx: &mut SimCtx, what: &str, cmd: u8, since: f64, want: usize) {
        self.notifications(cx, what, cmd, since, want, want, "")
    }

    #[allow(clippy::too_many_arguments)]
    fn notifications(&mut self, cx: &mut SimCtx, what: &str, cmd: u8, since: f64, want: usize, stock: usize, why: &str) {
        let gr: Vec<Notif> = self.gp_from(self.short["R"], cmd, since).iter().map(|b| notification(b)).collect();
        let gt: Vec<Notif> = self.gp_from(self.short["T"], cmd, since).iter().map(|b| notification(b)).collect();
        let strip = |v: &Vec<Notif>| v.iter().map(|n| n.stripped()).collect::<Vec<_>>();
        let ok = gr.len() == want
            && gt.len() == stock
            && (stock == 0 || strip(&gr) == strip(&gt))
            && gr.iter().all(|n| n.gpp == self.short["R"])
            && gt.iter().all(|n| n.gpp == self.short["T"]);
        let detail = if stock == 0 && !gr.is_empty() {
            let r = &gr[0];
            format!("Rust: options {:#06x}, command {:#04x}, payload {}; stock sent {} ({why})", r.options, r.cmd,
                    if r.payload.is_empty() { "-".to_string() } else { r.payload.clone() }, gt.len())
        } else if !gr.is_empty() && !gt.is_empty() {
            let (r, t) = (&gr[0], &gt[0]);
            let dash = |s: &str| if s.is_empty() { "-".to_string() } else { s.to_string() };
            format!(
                "options {:#06x}/{:#06x}, command {:#04x}, payload {}/{}, link byte Rust {:#04x} stock {:#04x}",
                r.options,
                t.options,
                r.cmd,
                dash(&r.payload),
                dash(&t.payload),
                r.link,
                t.link
            )
        } else {
            format!("Rust sent {}, stock {}", gr.len(), gt.len())
        };
        cx.check(self.now(), what, ok, &detail);
    }

    fn counts(&self, cmd: u8, since: f64) -> (usize, usize) {
        (self.gp_from(self.short["R"], cmd, since).len(), self.gp_from(self.short["T"], cmd, since).len())
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Notif {
    options: u16,
    gpd: String,
    counter: u32,
    cmd: u8,
    payload: String,
    gpp: u16,
    link: u8,
    mic: String,
}

impl Notif {
    fn stripped(&self) -> (u16, String, u32, u8, String, String) {
        (self.options, self.gpd.clone(), self.counter, self.cmd, self.payload.clone(), self.mic.clone())
    }
}

fn attr_records(b: &[u8]) -> Vec<(u16, Vec<u8>)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 3 <= b.len() {
        let id = u16::from_le_bytes([b[i], b[i + 1]]);
        if b[i + 2] != 0 {
            out.push((id, vec![]));
            i += 3;
            continue;
        }
        let n = match b.get(i + 3) {
            Some(0x18) | Some(0x20) => 1,
            Some(0x21) => 2,
            Some(0x1A) => 3,
            Some(0xF1) => 16,
            _ => break,
        };
        out.push((id, b.get(i + 4..i + 4 + n).unwrap_or(&[]).to_vec()));
        i += 4 + n;
    }
    out
}

fn notification(body: &[u8]) -> Notif {
    let g = |i: usize| body.get(i).copied().unwrap_or(0);
    let u16_ = |i: usize| u16::from_le_bytes([g(i), g(i + 1)]);
    let options = u16_(0);
    let app = options & 7;
    let i = 2 + if app == 2 { 9 } else { 4 };
    let counter = u32::from_le_bytes([g(i), g(i + 1), g(i + 2), g(i + 3)]);
    let cmd = g(i + 4);
    let n = g(i + 5) as usize;
    let n = if n == 0xFF { 0 } else { n };
    let j = i + 6 + n;
    let gpp = u16_(j);
    let link = g(j + 2);
    let sl = |a: usize, b: usize| hx(body.get(a.min(body.len())..b.min(body.len())).unwrap_or(&[]));
    Notif {
        options,
        gpd: sl(2, i),
        counter,
        cmd,
        payload: sl(i + 6, j),
        gpp,
        link,
        mic: sl(j + 3, body.len()),
    }
}

fn nwk_plain(psdu: &[u8], pan: u16) -> Option<(u16, u16, u8, u8, Vec<u8>)> {
    let f = mac_parse(psdu)?;
    let p = &f.payload;
    if f.ty != 1 || f.dpan != Some(pan) || p.len() < 8 + 14 + 4 {
        return None;
    }
    let fcf = u16::from_le_bytes([p[0], p[1]]);
    let (dst, src, radius, seq) = (u16::from_le_bytes([p[2], p[3]]), u16::from_le_bytes([p[4], p[5]]), p[6], p[7]);
    if fcf & 3 != 0 || fcf & 0x0200 == 0 {
        return None;
    }
    let i = 8 + if fcf & 0x0800 != 0 { 8 } else { 0 } + if fcf & 0x1000 != 0 { 8 } else { 0 };
    if p.len() < i + 14 + 4 {
        return None;
    }
    let sc = p[i];
    let fc = u32::from_le_bytes(p[i + 1..i + 5].try_into().unwrap());
    let sext = u64::from_le_bytes(p[i + 5..i + 13].try_into().unwrap());
    let sc5 = (sc & !7) | 5;
    let mut aad = p[..i].to_vec();
    aad.push(sc5);
    aad.extend_from_slice(&p[i + 1..i + 14]);
    let mut nonce = [0u8; 13];
    nonce[..8].copy_from_slice(&sext.to_le_bytes());
    nonce[8..12].copy_from_slice(&fc.to_le_bytes());
    nonce[12] = sc5;
    let plain = ccm_decrypt(&NWK_KEY, &nonce, &aad, &p[i + 14..])?;
    Some((dst, src, radius, seq, plain))
}

fn run(cx: &mut SimCtx) -> R<()> {
    let ri = super::mixed::image(cx, Fw::Rust);
    let ti = super::mixed::image(cx, Fw::Stock);
    let mut rn = node_of(&ri, cx.cpi)?;
    rn.write_flash(0xFF000, &REC_R);
    let mut tn = node_of(&ti, cx.cpi)?;
    tn.write_flash(0xFF000, &REC_T);
    let mut s = Sim::new(rn, "R", Coord::new_plain(11, NWK_KEY), cx.log(), QUANTUM);
    let (rid, tid) = (NodeId::Dut, s.w.add_bulb(tn, "T"));
    s.w.set_t_off(tid, 1500.0);
    s.w.coord.start(&mut s.w.air);
    let gpd = s.w.attach_peer(|id| Peer::bare(Station::new(id, 11, 0xFFFF, None, 0, NWK_KEY, (0, 0, 0, 0, 0), 0, 0), "gpd"));
    let nodes = [(rid, "R"), (tid, "T")];
    let mut g = Gp { s, gpd, gpd_seq: 0x30, short: BTreeMap::new(), ids: BTreeMap::new() };
    g.ids.insert("R", rid);
    g.ids.insert("T", tid);
    cx.say(&format!(
        "== Rust proxy: {}; stock proxy: {}; channel 11",
        super::util::rel(&ri),
        super::util::rel(&ti)
    ));
    let ok = g.s.run_until(20000.0, 10.0, |s| s.announced(EXT_R).is_some() && s.announced(EXT_T).is_some())?;
    let (r, t) = (g.s.announced(EXT_R), g.s.announced(EXT_T));
    let ho = |v: Option<u16>| v.map(|x| format!("'{x:#x}'")).unwrap_or_else(|| "None".into());
    if !cx.check(g.now(), "both bulbs join the coordinator", ok, &format!("{{'R': {}, 'T': {}}}", ho(r), ho(t))) {
        cx.finish(&g.s, &nodes);
        return Ok(());
    }
    g.short.insert("R", r.unwrap());
    g.short.insert("T", t.unwrap());
    g.s.run_until(15000.0, 10.0, |s| verified(s) >= 2)?;
    g.s.run(1000.0)?;

    let mut desc = [None, None];
    for (k, name) in ["R", "T"].iter().enumerate() {
        let t0 = g.now();
        let sh = g.short[name];
        let mut q = sh.to_le_bytes().to_vec();
        q.push(GP_EP);
        g.s.w.coord.zdo_send(&mut g.s.w.air, sh, 0x0004, &q);
        g.s.run_until(1500.0, 10.0, |s| s.w.coord.zcl_rsp.iter().any(|r| r.ms >= t0 && r.src == sh && r.cluster == 0x8004))?;
        desc[k] = g
            .s
            .w
            .coord
            .zcl_rsp
            .iter()
            .find(|r| r.ms >= t0 && r.src == sh && r.cluster == 0x8004)
            .map(|r| r.body.get(4..).unwrap_or(&[]).to_vec());
    }
    cx.check(g.now(), "the Green Power endpoint's simple descriptor is the same", desc[0].is_some() && desc[0] == desc[1],
             &format!("Rust {}  stock {}", hex_or_none(&desc[0]), hex_or_none(&desc[1])));
    let attrs: [u16; 7] = [0x0010, 0x0016, 0x0017, 0x0020, 0x0021, 0x0022, 0xFFFD];
    let read: Vec<u8> = attrs.iter().flat_map(|a| a.to_le_bytes()).collect();
    let (ar, at) = g.both_answer(&move |g: &mut Gp, s| {
        g.gp_send(s, 0x00, &read, false);
    }, 0x01)?;
    let (pr, pt) = (ar.as_deref().map(attr_records).unwrap_or_default(), at.as_deref().map(attr_records).unwrap_or_default());
    let val = |v: &[(u16, Vec<u8>)], id: u16| v.iter().find(|x| x.0 == id).map(|x| x.1.clone());
    let own = [0x0020u16, 0x0021, 0xFFFD];
    let common = pr.len() == attrs.len() && pr.iter().filter(|x| !own.contains(&x.0)).all(|x| val(&pt, x.0).as_ref() == Some(&x.1));
    let rust_spec = val(&pr, 0x0020) == Some(vec![0x00]) && val(&pr, 0xFFFD) == Some(vec![0x02, 0x00]);
    let stock_measured = val(&pt, 0x0020) == Some(vec![0x02]) && val(&pt, 0xFFFD) == Some(vec![0x01, 0x00])
        && val(&pt, 0x0021) == Some((0xC0..=0xCF).collect());
    cx.check(g.now(), "the cluster's attributes read the same (table size, functionality, link key) but for the shared key \
                       (Rust: type 0b000, GP 1.1.2 default; stock: 0b010 with c0..cf) and the revision (Rust 2, stock 1)",
             common && rust_spec && stock_measured, &format!("Rust {}  stock {}", hex_or_none(&ar), hex_or_none(&at)));
    let (er, et) = g.table_entry(SWITCH, None, 0)?;
    cx.check(g.now(), "an empty proxy table: Rust answers NOT_FOUND (GP 1.1.2 A.3.4.3); stock SUCCESS with no entry (measured)",
             er.as_deref() == Some(&[0x8B, 0x00, 0xFF, 0x00][..]) && et.as_deref() == Some(&[0x00, 0x00, 0xFF, 0x00][..]),
             &got_text(&er, &et));

    g.commissioning_mode(true, true, false)?;
    let t0 = g.now();
    let mut comm = vec![0x02, 0x81, 0xB2];
    comm.extend_from_slice(&switch_key());
    comm.extend_from_slice(&1u32.to_le_bytes());
    g.frame(Gpdf::new(0xE0, &comm));
    g.s.run(600.0)?;
    g.same_notification(cx, "commissioning mode: the device's Commissioning frame is tunnelled alike", 0x04, t0, 1);
    let t0 = g.now();
    g.frame(Gpdf { maintenance: true, repeats: 1, ..Gpdf::new(0xE3, &[0x85]) });
    g.s.run(600.0)?;
    g.same_notification(cx, "a Channel Request (maintenance frame) as well", 0x04, t0, 1);
    let t0 = g.now();
    g.frame(Gpdf { key: Some(switch_key()), counter: 50, ..Gpdf::new(0x22, &[]) });
    g.s.run(600.0)?;
    g.notifications(cx, "a frame under a key neither knows: Rust passes it on unprocessed, with its MIC (GP 1.1.2 A.3.3.4.3, \
                         SecurityProcessingFailed); stock drops it (measured)", 0x04, t0, 1, 0, "dropped");
    let t0 = g.now();
    g.frame(Gpdf { auto: true, ..Gpdf::new(0xE0, &comm) });
    g.frame(Gpdf::new(0xF0, &[0x00]));
    g.s.run(600.0)?;
    let (nr, nt) = g.counts(0x04, t0);
    cx.check(g.now(), "neither tunnels a Commissioning frame marked auto-commissioning, nor a Commissioning Reply",
             (nr, nt) == (0, 0), &format!("{{'R': {nr}, 'T': {nt}}}"));

    for name in ["R", "T"] {
        let mut reply = vec![0x00];
        reply.extend_from_slice(&g.short[name].to_le_bytes());
        reply.push(0);
        reply.extend_from_slice(&SWITCH.to_le_bytes());
        reply.extend_from_slice(&[0xF0, 3, 0x01, 0x62, 0x1A]);
        g.broadcast(0x06, &reply)?;
        let n0 = g.s.w.air.frames.len();
        let t_end = g.frame(Gpdf { rx_after_tx: true, repeats: 1, ..Gpdf::new(0xE0, &[0x02, 0x23]) });
        g.s.run(300.0)?;
        let t_start = g.s.w.air.frames[n0..].iter().find(|f| (f.t1 - t_end).abs() < 1e-6).map_or(t_end, |f| f.t0);
        let mut to_gpd = Vec::new();
        for f in &g.s.w.air.frames[n0..] {
            let who = if f.who == rid { "R" } else if f.who == tid { "T" } else { continue };
            let p = &f.psdu;
            if p.len() > 9 && p[3..7] == [0xff; 4] && p[7] & 0x3C == 0x0C {
                to_gpd.push((f.t0, who, p.clone()));
            }
        }
        let detail = to_gpd
            .iter()
            .map(|(t0, who, p)| format!("{who} {:.2} ms after the start of the frame: {}", t0 - t_start, hx(&p[..p.len() - 2])))
            .collect::<Vec<_>>()
            .join("; ");
        let mut want = vec![0xff, 0xff, 0xff, 0xff, 0x8C, 0x80];
        want.extend_from_slice(&SWITCH.to_le_bytes());
        want.extend_from_slice(&[0xF0, 0x01, 0x62, 0x1A]);
        let ok = to_gpd.len() == 1
            && to_gpd[0].1 == name
            && (20.0..=25.0).contains(&(to_gpd[0].0 - t_start))
            && to_gpd[0].2[3..to_gpd[0].2.len() - 2] == want[..];
        let who = if name == "R" { "Rust" } else { "stock" };
        cx.check(g.now(), &format!("the sink's Commissioning Reply is sent by the proxy it names ({who}), 20-25 ms after the start of the device's frame"),
                 ok, if detail.is_empty() { "nothing sent" } else { &detail });
    }

    g.pairing(SWITCH, None, 0, true, false, 3, 0, 3, 4, Some(100), Some(switch_key()), None, None, false)?;
    let (er, et) = g.table_entry(SWITCH, None, 0)?;
    cx.check(g.now(), "after GP Pairing the two proxy table entries are the same, byte for byte", er.is_some() && er == et,
             &format!("Rust {}  stock {}", hex_or_none(&er), hex_or_none(&et)));
    g.commissioning_mode(false, true, false)?;
    let t0 = g.now();
    g.frame(Gpdf { key: Some(switch_key()), counter: 101, ..Gpdf::new(0x22, &[]) });
    g.s.run(800.0)?;
    g.notifications(cx, "operation: an encrypted Toggle of a device paired with its own key (key type 0b100) is decrypted and \
                         forwarded by Rust; stock forwards none (measured)", 0x00, t0, 1, 0, "no notification: it takes no frame of such a device");
    let t0 = g.now();
    g.frame(Gpdf { key: Some(switch_key()), counter: 102, ..Gpdf::new(0x30, &[0x55]) });
    g.s.run(800.0)?;
    g.notifications(cx, "a command with a payload as well", 0x00, t0, 1, 0, "the same");
    let t0 = g.now();
    g.frame(Gpdf { key: Some(switch_key()), counter: 102, ..Gpdf::new(0x22, &[]) });
    g.frame(Gpdf { key: Some([0; 16]), counter: 200, ..Gpdf::new(0x22, &[]) });
    g.frame(Gpdf::new(0x22, &[]));
    g.s.run(800.0)?;
    let (nr, nt) = g.counts(0x00, t0);
    cx.check(g.now(), "a replayed counter, a wrong key and an unsecured frame are dropped by both", (nr, nt) == (0, 0),
             &format!("{{'R': {nr}, 'T': {nt}}}"));
    let (er, et) = g.table_entry(SWITCH, None, 0)?;
    let counter_at = |v: &Option<Vec<u8>>| v.as_ref().and_then(|b| b.get(11..15)).map(|c| u32::from_le_bytes(c.try_into().unwrap()));
    let opts_at = |v: &Option<Vec<u8>>| v.as_ref().and_then(|b| b.get(4..6)).map(|c| u16::from_le_bytes([c[0], c[1]]));
    let same_but_counter = match (&er, &et) {
        (Some(a), Some(b)) => a.len() == b.len() && a.iter().zip(b).enumerate().all(|(i, (x, y))| (4..6).contains(&i) || (11..15).contains(&i) || x == y),
        _ => false,
    };
    cx.check(g.now(), "the table entries still agree but for the frame counter (Rust 102: moved on; stock 100) and Rust's InRange bit",
             same_but_counter && counter_at(&er) == Some(102) && counter_at(&et) == Some(100)
                 && opts_at(&er).zip(opts_at(&et)).is_some_and(|(r, t)| r == t | 0x0400),
             &format!("Rust {}  stock {}", hex_or_none(&er), hex_or_none(&et)));

    let other: u32 = 0x00000042;
    g.pairing(other, None, 0, true, false, 2, 0x0B84, 0, 0, None, None, None, None, true)?;
    let (er, et) = g.table_entry(other, None, 0)?;
    let n = er.as_ref().map_or(0, |v| v.len());
    let same_but_alias = match (&er, &et) {
        (Some(a), Some(b)) => a.len() == b.len() && n >= 3 && a[..n - 3] == b[..n - 3] && a[n - 1] == b[n - 1],
        _ => false,
    };
    let alias = |v: &Option<Vec<u8>>| v.as_ref().filter(|v| v.len() >= 3).map(|v| u16::from_le_bytes([v[v.len() - 3], v[v.len() - 2]]));
    cx.check(g.now(), "a pairing to a group: the same entries but for the alias with the group (stock 0x0042 derived, Rust 0xffff)",
             same_but_alias && alias(&et) == Some(0x0042) && alias(&er) == Some(0xFFFF),
             &format!("Rust {}  stock {}", hex_or_none(&er), hex_or_none(&et)));
    let t0 = g.now();
    g.frame(Gpdf { src_id: other, ..Gpdf::new(0x22, &[]) });
    g.s.run(1500.0)?;
    let pan = g.s.w.coord.st.pan;
    let mut sent: BTreeMap<&str, Vec<(u16, u8, u8, Vec<u8>)>> = BTreeMap::new();
    sent.insert("R", vec![]);
    sent.insert("T", vec![]);
    for f in &g.s.w.air.frames {
        let who = if f.who == rid { "R" } else if f.who == tid { "T" } else { continue };
        if f.t0 < t0 {
            continue;
        }
        if let Some((dst, src, radius, seq, aps)) = nwk_plain(&f.psdu, pan) {
            if src == 0x0042 {
                let item = (dst, radius, seq, aps);
                let v = sent.get_mut(who).unwrap();
                if !v.contains(&item) {
                    v.push(item);
                }
            }
        }
    }
    let seq = g.gpd_seq;
    let mut ok = !sent["R"].is_empty() && !sent["T"].is_empty();
    let mut detail = String::new();
    if ok {
        let (rd, rr, rs, raps) = sent["R"][0].clone();
        let (td, tr, ts, taps) = sent["T"][0].clone();
        let (rn, tn) = (notification(raps.get(12..).unwrap_or(&[])), notification(taps.get(12..).unwrap_or(&[])));
        ok = (rd, rs) == (td, ts)
            && (td, ts) == (0xFFFD, seq.wrapping_sub(9))
            && raps.get(..8) == taps.get(..8)
            && raps.get(9) == taps.get(9)
            && raps.get(11) == taps.get(11)
            && rn.stripped() == tn.stripped();
        detail = format!(
            "NWK to {rd:#06x} from the alias 0x0042, sequence number {rs} (the device's {seq} - 9), radius Rust {rr} stock {tr}; APS {}; options {:#06x}/{:#06x}",
            hx(&raps[..8.min(raps.len())]),
            rn.options,
            tn.options
        );
    }
    cx.check(g.now(), "the groupcast under the device's alias: same NWK addressing and sequence number, same notification", ok, &detail);
    let n = g.s.w.coord.zcl_rsp.iter().filter(|r| r.ms >= t0 && r.src == 0x0042 && r.cluster == GP_CLUSTER && r.cmd == Some(0x00)).count();
    cx.check(g.now(), "the two broadcasts collapse into one at the sink", n == 1, &format!("{n} received"));

    let mut write = vec![0x20, 0x00, 0x18, 0x02, 0x21, 0x00, 0xF1];
    write.extend_from_slice(&shared_key());
    let (wr, wt) = g.both_answer(&move |g: &mut Gp, s| {
        g.gp_send(s, 0x02, &write, false);
    }, 0x04)?;
    cx.check(g.now(), "both take gpSharedSecurityKeyType and gpSharedSecurityKey",
             wr.is_some() && wr == wt && wt.as_deref() == Some(&[0x00][..]), &got_text(&wr, &wt));
    let third: u32 = 0x00000099;
    g.pairing(third, None, 0, true, false, 3, 0, 3, 2, Some(10), None, None, None, false)?;
    let t0 = g.now();
    g.frame(Gpdf { src_id: third, key: Some(shared_key()), counter: 11, individual: false, ..Gpdf::new(0x21, &[]) });
    g.s.run(800.0)?;
    g.same_notification(cx, "a device paired with key type 'group key' and no key: both use the shared key", 0x00, t0, 1);

    g.pairing(0, Some(SENSOR), 5, true, false, 3, 0, 3, 4, Some(100), Some(switch_key()), None, None, false)?;
    let (er, et) = g.table_entry(0, Some(SENSOR), 5)?;
    cx.check(g.now(), "a device with an IEEE address: the same entries", er.is_some() && er == et,
             &format!("Rust {}  stock {}", hex_or_none(&er), hex_or_none(&et)));
    let t0 = g.now();
    g.frame(Gpdf { ieee: Some(SENSOR), endpoint: 5, key: Some(switch_key()), counter: 101, ..Gpdf::new(0x22, &[]) });
    g.s.run(800.0)?;
    g.notifications(cx, "its encrypted frame (own key) is decrypted and forwarded by Rust; stock forwards none (measured)", 0x00, t0, 1, 0,
                    "no notification");

    g.pairing(SWITCH, None, 0, true, true, 3, 0, 0, 0, None, None, None, None, false)?;
    let (er, et) = g.table_entry(SWITCH, None, 0)?;
    cx.check(g.now(), "GP Pairing 'remove the device': gone from both tables",
             er.is_some() && er == et && er.as_ref().and_then(|v| v.first().copied()) == Some(0x8B), &got_text(&er, &et));
    let t0 = g.now();
    g.frame(Gpdf { key: Some(switch_key()), counter: 300, ..Gpdf::new(0x22, &[]) });
    g.s.run(800.0)?;
    let (nr, nt) = g.counts(0x00, t0);
    cx.check(g.now(), "and its frames are no longer forwarded", (nr, nt) == (0, 0), &format!("{{'R': {nr}, 'T': {nt}}}"));
    let (rr, tr) = (g.s.w.bulb_resets(rid), g.s.w.bulb_resets(tid));
    cx.check(g.now(), "no resets", rr == 0 && tr == 0, &format!("resets R={rr} T={tr}"));
    cx.finish(&g.s, &nodes);
    Ok(())
}
