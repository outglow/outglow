//! Accounting of lost relays and MAC retransmissions (`STRESS_ACCOUNT=1`):
//! diagnostics only, nothing else depends on it.
//!
//! Two sources besides the air:
//! - every bulb radio's fate log (`zbnode::radio154::RxFate`): what became of
//!   each frame that reached it (received, collision, receiver off and why);
//! - for Rust images built with `cargo xtask build --trace`, the
//!   firmware's own record of what it did with each frame (`ZB_TRACE`, the
//!   kinds in `zigbee::hal::trace` and `bulb-tc32/src/trace.rs`) and the radio
//!   queue's drop counter (`RADIO_DROPPED`).
//!
//! `report` gives, for each relayed frame a bulb acknowledged and then never
//! passed on (`Mesh::relay_lost_list`), the firmware's outcome, and for each
//! MAC retransmission by a bulb, why the attempt before it got no
//! acknowledgement.

use std::collections::BTreeMap;

use super::net::{Dec, Mesh};
use crate::air::NodeId;

const SRAM: u32 = 0x840000;
const RING_ENTRIES: u32 = 512;

// zigbee::hal::trace (kept in step by hand: the emulator does not link the firmware crates)
const K_RELAY: u8 = 1;
const K_TX: u8 = 2;
const K_DROP: u8 = 3;
const K_HOLD: u8 = 4;
const K_HELD_FAIL: u8 = 5;
const K_NWK_STATUS: u8 = 6;
const K_RX_DROP: u8 = 7;
const K_BROADCAST: u8 = 8;
const K_FLASH: u8 = 16;
const K_BLE: u8 = 17;
const K_MAC: u8 = 18;

#[derive(Clone, Debug)]
pub struct TraceRec {
    /// air time (ms)
    pub t: f64,
    pub kind: u8,
    pub detail: u8,
    pub seq: u8,
    pub src: u16,
    pub peer: u16,
    pub aux: u32,
}

#[derive(Clone, Debug)]
pub struct RxRec {
    /// air time (ms)
    pub t0: f64,
    pub t1: f64,
    pub fcs: u16,
    pub fate: &'static str,
}

pub struct Account {
    ring: Vec<Option<u32>>,
    dropped_at: Vec<Option<u32>>,
    last_head: Vec<u32>,
    last_dropped: Vec<u32>,
    pub overflow: Vec<u64>,
    pub traces: Vec<Vec<TraceRec>>,
    /// per bulb: frames by start time (µs, air)
    pub rx: Vec<BTreeMap<i64, Vec<RxRec>>>,
    /// per bulb: (ms, frames dropped because the radio queue was full)
    pub qfull: Vec<Vec<(f64, u32)>>,
}

fn rd32(sram: &[u8], addr: u32) -> u32 {
    let o = (addr - SRAM) as usize;
    sram.get(o..o + 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

impl Account {
    pub fn new(m: &mut Mesh) -> Account {
        let n = m.bulbs.len();
        let mut a = Account {
            ring: Vec::new(),
            dropped_at: Vec::new(),
            last_head: vec![0; n],
            last_dropped: vec![0; n],
            overflow: vec![0; n],
            traces: vec![Vec::new(); n],
            rx: vec![BTreeMap::new(); n],
            qfull: vec![Vec::new(); n],
        };
        for b in &m.bulbs {
            a.ring.push(b.img.symbol("ZB_TRACE"));
            a.dropped_at.push(b.img.symbol("RADIO_DROPPED"));
            m.s.w.node(b.id).radio.b().rx_log = Some(Vec::new());
        }
        a
    }

    fn t_off(m: &Mesh, id: NodeId) -> f64 {
        match id {
            NodeId::Dut => m.s.w.air.t_off,
            NodeId::Bulb(i) => m.s.w.air.bulbs[i as usize].t_off,
            _ => 0.0,
        }
    }

    /// Takes what the bulbs recorded since the last call.
    pub fn poll(&mut self, m: &Mesh) {
        for (k, b) in m.bulbs.iter().enumerate() {
            let node = m.s.w.node(b.id);
            let off = Self::t_off(m, b.id);
            // (a bulb paired at the coordinator is a new node: its radio starts without the log)
            let mut radio = node.radio.b();
            for f in radio.rx_log.get_or_insert_with(Vec::new).drain(..) {
                let t0 = f.t0 + off;
                self.rx[k].entry((t0 * 1000.0).round() as i64).or_default().push(RxRec { t0, t1: f.t1 + off, fcs: f.fcs, fate: f.fate });
            }
            drop(radio);
            let now_ms = node.m.ms() + off;
            let now_tick = node.m.stimer_now();
            if let Some(at) = self.dropped_at[k] {
                let d = rd32(&node.m.sram, at);
                if d != self.last_dropped[k] {
                    self.qfull[k].push((now_ms, d.wrapping_sub(self.last_dropped[k])));
                    self.last_dropped[k] = d;
                }
            }
            let Some(ring) = self.ring[k] else { continue };
            if rd32(&node.m.sram, ring) != 0x5452_4345 {
                continue;
            }
            let head = rd32(&node.m.sram, ring + 4);
            if head < self.last_head[k] {
                self.last_head[k] = 0; // restarted
            }
            if head - self.last_head[k] > RING_ENTRIES {
                self.overflow[k] += (head - self.last_head[k] - RING_ENTRIES) as u64;
                self.last_head[k] = head - RING_ENTRIES;
            }
            for i in self.last_head[k]..head {
                let e = ring + 8 + (i % RING_ENTRIES) * 16;
                let w = [rd32(&node.m.sram, e), rd32(&node.m.sram, e + 4), rd32(&node.m.sram, e + 8), rd32(&node.m.sram, e + 12)];
                let t = now_ms - now_tick.wrapping_sub(w[0]) as f64 / 16000.0;
                self.traces[k].push(TraceRec {
                    t,
                    kind: w[1] as u8,
                    detail: (w[1] >> 8) as u8,
                    seq: (w[1] >> 16) as u8,
                    src: w[2] as u16,
                    peer: (w[2] >> 16) as u16,
                    aux: w[3],
                });
            }
            self.last_head[k] = head;
        }
    }

    /// The fate at bulb `k` of air frame `d`.
    pub fn fate(&self, k: usize, d: &Dec, psdu: &[u8]) -> Option<&'static str> {
        let fcs = if psdu.len() >= 2 { u16::from_le_bytes([psdu[psdu.len() - 2], psdu[psdu.len() - 1]]) } else { 0 };
        let key = (d.t0 * 1000.0).round() as i64;
        self.rx[k].range(key - 3..=key + 3).flat_map(|(_, v)| v.iter()).find(|r| r.fcs == fcs).map(|r| r.fate)
    }
}

/// What the ring of a TRACE=1 image holds now (at most its last
/// `RING_ENTRIES` records), with air times (`t_off`: the node's power-on
/// time on the air), for scenarios other than the stress house.
pub fn ring_records(node: &zbnode::Node, img: &crate::image::Image, t_off: f64) -> Vec<TraceRec> {
    let Some(ring) = img.symbol("ZB_TRACE") else { return Vec::new() };
    let sram = &node.m.sram;
    if rd32(sram, ring) != 0x5452_4345 {
        return Vec::new();
    }
    let head = rd32(sram, ring + 4);
    let (now_ms, now_tick) = (node.m.ms() + t_off, node.m.stimer_now());
    (head.saturating_sub(RING_ENTRIES)..head)
        .map(|i| {
            let e = ring + 8 + (i % RING_ENTRIES) * 16;
            let w = [rd32(sram, e), rd32(sram, e + 4), rd32(sram, e + 8), rd32(sram, e + 12)];
            TraceRec {
                t: now_ms - now_tick.wrapping_sub(w[0]) as f64 / 16000.0,
                kind: w[1] as u8,
                detail: (w[1] >> 8) as u8,
                seq: (w[1] >> 16) as u8,
                src: w[2] as u16,
                peer: (w[2] >> 16) as u16,
                aux: w[3],
            }
        })
        .collect()
}

/// One line about a record (`ring_records`).
pub fn describe(r: &TraceRec) -> String {
    kind_name(r)
}

fn kind_name(r: &TraceRec) -> String {
    let d = r.detail;
    match r.kind {
        K_RELAY => format!("relay from {:#06x}", r.peer),
        K_TX => {
            let st = ["ok", "no ACK", "channel busy", "for a sleepy child", "too long"].get(d as usize).copied().unwrap_or("?");
            format!(
                "TX to {:#06x}: {st} ({} attempt(s), {} busy CCA, {} us)",
                r.peer,
                r.aux & 0xff,
                (r.aux >> 8) & 0xff,
                r.aux >> 16
            )
        }
        K_DROP => format!(
            "dropped: {}",
            [
                "?",
                "radius",
                "source route index",
                "route reply not ours",
                "unauthenticated child",
                "too long to hold",
                "towards a concentrator, no other router",
                "command with no route",
                "route record with no way on",
                "failed, reported to nobody",
            ]
            .get(d as usize)
            .copied()
            .unwrap_or("?")
        ),
        K_HOLD if d == 0 => "held for a route".into(),
        K_HOLD => format!("held for another attempt via {:#06x} ({d} to come)", r.peer),
        K_HELD_FAIL => format!("held frame failed: {}", ["?", "expired", "evicted", "no room"].get(d as usize).copied().unwrap_or("?")),
        K_NWK_STATUS => format!("Network Status {:#04x} to {:#06x} about {:#06x}{}", r.seq, r.src, r.peer, if d == 1 { " NOT SENT (no route back)" } else { "" }),
        K_RX_DROP => format!(
            "dropped on receipt: {}",
            ["?", "replay (counter not new)", "bad MIC", "unknown key", "echo", "own address", "not secured", "not up", "not a child"]
                .get(d as usize)
                .copied()
                .unwrap_or("?")
        ),
        K_BROADCAST => format!("broadcast {}", ["queued", "BTT full", "queue full"].get(d as usize).copied().unwrap_or("?")),
        K_FLASH => format!("flash {} {} us", if d == 0 { "write" } else { "erase" }, r.aux / 16),
        K_BLE => format!("BLE {} {} us", ["advertising", "connection event", "mesh burst", "mesh listen"].get(d as usize).copied().unwrap_or("?"), r.aux / 16),
        K_MAC => format!(
            "MAC frame of {} B seq {} to {:#06x}: {} ({} attempt(s), {} busy CCA, {} us)",
            r.src,
            r.seq,
            r.peer,
            ["ok", "no ACK", "channel busy"].get(d as usize).copied().unwrap_or("?"),
            r.aux & 0xff,
            (r.aux >> 8) & 0xff,
            r.aux >> 16
        ),
        _ => format!("kind {}", r.kind),
    }
}

/// The outcome class of a lost relay from the firmware's record of it.
fn relay_class(chain: &[&TraceRec]) -> String {
    if chain.is_empty() {
        return "no record (never reached the NWK layer)".into();
    }
    if let Some(r) = chain.iter().find(|r| r.kind == K_RX_DROP) {
        return kind_name(r);
    }
    if let Some(r) = chain.iter().find(|r| r.kind == K_DROP) {
        return kind_name(r);
    }
    let txs: Vec<&&TraceRec> = chain.iter().filter(|r| r.kind == K_TX).collect();
    let fails = chain.iter().find(|r| r.kind == K_HELD_FAIL);
    if let Some(f) = fails {
        return kind_name(f);
    }
    if !txs.is_empty() && txs.iter().all(|r| r.detail == 2) {
        return format!("channel busy at every attempt ({} attempt(s))", txs.len());
    }
    if let Some(r) = txs.iter().find(|r| r.detail == 3 || r.detail == 4) {
        return kind_name(r);
    }
    if txs.iter().any(|r| r.detail == 0) {
        return "sent (ACKed) but not seen on the air as such".into();
    }
    if chain.iter().any(|r| r.kind == K_HOLD && r.detail == 0) {
        return "still held for a route at the end of the window".into();
    }
    if chain.iter().any(|r| r.kind == K_HOLD) {
        return "held for another attempt".into();
    }
    "relayed, no outcome".into()
}

/// What the air shows of relay `x`'s handling of frame `i` (any image): a
/// copy of a frame it acknowledged and passed on before, its own frame
/// coming back (a routing loop), passed on or reported only after the 3 s
/// of `relay_drops`, a route discovery for the destination right after.
fn air_class(m: &Mesh, i: usize, x: NodeId) -> String {
    let d = &m.dec[i];
    let n = d.nwk.as_ref().unwrap();
    let me = m.short_of(x);
    if n.src == me {
        return "its own frame came back (routing loop)".into();
    }
    let same = |e: &Dec| e.nwk.as_ref().is_some_and(|f| f.src == n.src && f.seq == n.seq);
    // an earlier copy (same sender, MAC sequence number) that it acknowledged
    let first = m.dec[..i].iter().rev().take_while(|e| e.t0 > d.t0 - 50.0).find(|e| e.who == d.who && e.mac_seq == d.mac_seq && same(e));
    if let Some(f) = first {
        let passed = m.dec[f.idx + 1..i].iter().any(|e| e.who == Some(x) && same(e));
        if passed {
            return "duplicate: MAC copy of a frame it had passed on already (sender missed the ACK)".into();
        }
        return "duplicate: MAC copy of a frame taken just before".into();
    }
    let late = m.dec[i + 1..].iter().take_while(|e| e.t0 < d.t1 + 8000.0).find(|e| {
        e.who == Some(x)
            && e.nwk.as_ref().is_some_and(|f| {
                (f.src == n.src && f.seq == n.seq) || (f.ty == crate::frames::NWK_CMD && f.payload.first() == Some(&0x03) && f.payload.get(2..4) == Some(&n.dst.to_le_bytes()[..]))
            })
    });
    let rreq = m.dec[i + 1..].iter().take_while(|e| e.t0 < d.t1 + 100.0).any(|e| {
        e.who == Some(x)
            && e.nwk.as_ref().is_some_and(|f| {
                f.src == me && f.ty == crate::frames::NWK_CMD && f.payload.first() == Some(&0x01) && f.payload.get(3..5) == Some(&n.dst.to_le_bytes()[..])
            })
    });
    match (late, rreq) {
        (Some(e), _) if e.nwk.as_ref().is_some_and(|f| f.src == n.src) => format!("passed on late ({:.1} s)", (e.t0 - d.t1) / 1000.0),
        (Some(e), r) => format!("Network Status late ({:.1} s){}", (e.t0 - d.t1) / 1000.0, if r { " after a route discovery" } else { "" }),
        (None, true) => "route discovery started, then nothing".into(),
        (None, false) => "nothing on the air".into(),
    }
}

/// Lost relays and MAC retransmissions in [t0, t1) explained, as text lines.
pub fn report(m: &Mesh, a: &Account, t0: f64, t1: f64, title: &str) -> Vec<String> {
    let mut out = vec![format!("== accounting: {title} ({:.0}-{:.0} ms)", t0, t1)];
    let kix = |id: NodeId| m.bulbs.iter().position(|b| b.id == id);
    let name = |id: NodeId| m.s.w.air.name_of(id);
    for (k, o) in a.overflow.iter().enumerate() {
        if *o > 0 {
            out.push(format!("   (trace ring of {} overflowed: {o} records lost)", m.bulbs[k].name));
        }
    }
    // ---- lost relays
    let mut classes: BTreeMap<String, usize> = BTreeMap::new();
    let mut lines = Vec::new();
    for (i, x) in m.relay_lost_list() {
        let d = &m.dec[i];
        if d.t0 < t0 || d.t0 >= t1 {
            continue;
        }
        let Some(k) = kix(x) else { continue };
        let n = d.nwk.as_ref().unwrap();
        let chain: Vec<&TraceRec> = a.traces[k]
            .iter()
            .filter(|r| r.t >= d.t0 - 1.0 && r.t < d.t1 + 6000.0)
            .filter(|r| (r.src == n.src && r.seq == n.seq && r.kind < K_FLASH && r.kind != K_NWK_STATUS)
                || (r.kind == K_NWK_STATUS && r.src == n.src && r.t < d.t1 + 6000.0 && (r.peer == n.dst || r.peer == n.src)))
            .collect();
        let fw = a.ring[k].is_some();
        let mut class = if fw { relay_class(&chain) } else { air_class(m, i, x) };
        // a copy of a frame the bulb had relayed before (its sender missed our ACK)
        if class.starts_with("dropped on receipt: replay") {
            let earlier = m.dec[..i].iter().rev().take_while(|e| e.t0 > d.t0 - 3000.0).any(|e| {
                e.who == Some(x) && e.nwk.as_ref().is_some_and(|f| f.src == n.src && f.seq == n.seq)
            });
            let copy = m.dec[..i].iter().rev().take_while(|e| e.t0 > d.t0 - 50.0).any(|e| {
                e.mac_dst == d.mac_dst && e.who == d.who && e.mac_seq == d.mac_seq && e.nwk.as_ref().is_some_and(|f| f.src == n.src && f.seq == n.seq)
            });
            if earlier {
                class = "duplicate: copy of a frame already relayed (sender missed our ACK)".into();
            } else if copy {
                class = format!("duplicate: MAC copy of a frame taken before; the first copy: {}", air_class(m, i, x));
            }
        }
        if fw {
            class = format!("{class} [air: {}]", air_class(m, i, x));
        }
        *classes.entry(class.clone()).or_default() += 1;
        lines.push(format!("   {:.3} {} relay of {:#06x}->{:#06x} s={} from {}: {class}", d.t0, name(x), n.src, n.dst, n.seq,
                           d.who.map(name).unwrap_or_default()));
        for r in &chain {
            lines.push(format!("        {:.3} {}", r.t, kind_name(r)));
        }
    }
    let total: usize = classes.values().sum();
    out.push(format!("-- lost relays: {total}"));
    let mut cv: Vec<_> = classes.into_iter().collect();
    cv.sort_by(|a, b| b.1.cmp(&a.1));
    for (c, n) in &cv {
        out.push(format!("   {n:5}  {c}"));
    }
    if std::env::var("STRESS_ACCOUNT").is_ok_and(|v| v == "2") {
        out.extend(lines);
    }
    // ---- MAC retransmissions
    let mut retx: BTreeMap<String, usize> = BTreeMap::new();
    let mut per_rx: BTreeMap<String, usize> = BTreeMap::new();
    let frames = &m.s.w.air.frames;
    let mut last: BTreeMap<NodeId, usize> = BTreeMap::new();
    let start = m.dec.partition_point(|d| d.t0 < t0 - 100.0);
    let mut examples: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for i in start..m.dec.len() {
        let d = &m.dec[i];
        let Some(w) = d.who else { continue };
        if kix(w).is_none() || d.mac_ty == 2 || !d.mac_ar {
            continue;
        }
        let prev = last.insert(w, i);
        if d.t0 < t0 || d.t0 >= t1 {
            continue;
        }
        let Some(j) = prev else { continue };
        let p = &m.dec[j];
        let body = |x: &Dec| {
            let f = &frames[x.idx].psdu;
            f[..f.len().saturating_sub(2)].to_vec()
        };
        if p.mac_seq != d.mac_seq || body(p) != body(d) {
            continue;
        }
        // attempt j got no acknowledgement: why?
        let Some(rx_short) = p.mac_dst else { continue };
        let Some(r) = m.node_of_short(rx_short) else {
            *retx.entry("receiver unknown".into()).or_default() += 1;
            continue;
        };
        let ack = m.dec[j + 1..].iter().take_while(|e| e.t0 < p.t1 + 3.0).find(|e| e.who == Some(r) && e.mac_ty == 2 && e.mac_seq == p.mac_seq);
        let air_what = m.chan.what(p.idx, r);
        let cause: String = if let Some(ack) = ack {
            // the receiver acknowledged: the sender did not get it
            let k = kix(w).unwrap();
            let late = ack.t0 - p.t1;
            match a.fate(k, ack, &frames[ack.idx].psdu) {
                Some("received") => format!("ACK reached the sender, not taken (ACK {:.0} us after the frame)", late * 1000.0),
                Some(f) => format!("ACK lost at the sender: {}", collide_name(m, a, ack, w, f)),
                None => match m.chan.what(ack.idx, w) {
                    None => "ACK lost on the channel (model)".into(),
                    Some(x) => format!("ACK not at the sender's radio ({x})"),
                },
            }
        } else if let Some(rk) = kix(r) {
            match a.fate(rk, p, &frames[p.idx].psdu) {
                None => match air_what {
                    None if m.s.w.air.off.contains(&r) => "receiver powered off".into(),
                    None => "lost on the channel (model)".into(),
                    Some(x) => format!("not at the receiver's radio ({x})"),
                },
                Some("received") => {
                    let qf = a.qfull[rk].iter().any(|(t, _)| *t >= p.t1 - 0.5 && *t < p.t1 + 6.0);
                    if qf {
                        "receiver's radio queue full".into()
                    } else {
                        let busy = a.traces[rk].iter().find(|r| (r.kind == K_FLASH || r.kind == K_BLE) && r.t >= p.t0 && r.t - (r.aux as f64 / 16000.0) <= p.t1);
                        match busy {
                            Some(b) => format!("received, not acknowledged: receiver busy ({})", kind_name(b).split(' ').take(2).collect::<Vec<_>>().join(" ")),
                            None => "received, not acknowledged (no reason recorded)".into(),
                        }
                    }
                }
                Some(f) => collide_name(m, a, p, r, f),
            }
        } else {
            match air_what {
                None => "lost on the channel (model)".into(),
                Some("collided") => "collision at a scripted receiver".into(),
                Some("corrupt") => "corrupted (interference model)".into(),
                Some(_) => "scripted receiver got it, no ACK".into(),
            }
        };
        let class = cause.split(" (").next().unwrap_or(&cause).to_string();
        let ex = examples.entry(class.clone()).or_default();
        if ex.len() < 3 {
            ex.push(format!("{:.3} {}->{} seq={}: {cause}", p.t0, name(w), name(r), p.mac_seq));
        }
        *retx.entry(class).or_default() += 1;
        *per_rx.entry(name(r)).or_default() += 1;
    }
    let total: usize = retx.values().sum();
    out.push(format!("-- MAC retransmissions by bulbs: {total}; why the attempt before got no ACK"));
    let mut rv: Vec<_> = retx.into_iter().collect();
    rv.sort_by(|a, b| b.1.cmp(&a.1));
    for (c, n) in &rv {
        out.push(format!("   {n:5}  {c}"));
        if std::env::var("STRESS_ACCOUNT").is_ok_and(|v| v == "2") {
            for e in examples.get(c).into_iter().flatten() {
                out.push(format!("            e.g. {e}"));
            }
        }
    }
    // ACK turnaround of the bulbs: from the end of the frame to the start of its ACK
    let mut turn: Vec<f64> = Vec::new();
    let mut frames_by_bulbs = 0usize;
    for i in start..m.dec.len() {
        let e = &m.dec[i];
        if e.t0 < t0 || e.t0 >= t1 {
            continue;
        }
        if e.who.is_some_and(|w| kix(w).is_some()) {
            frames_by_bulbs += 1;
        }
        if e.mac_ty != 2 || !e.who.is_some_and(|w| kix(w).is_some()) {
            continue;
        }
        let me = m.short_of(e.who.unwrap());
        if let Some(p) = m.dec[..i].iter().rev().take(8).find(|p| p.mac_dst == Some(me) && p.mac_seq == e.mac_seq && p.mac_ar) {
            turn.push((e.t0 - p.t1) * 1000.0);
        }
    }
    turn.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if !turn.is_empty() {
        let q = |p: f64| turn[((turn.len() - 1) as f64 * p) as usize];
        out.push(format!(
            "   frames sent by bulbs {frames_by_bulbs}; ACK turnaround of bulbs (us): p5 {:.0} p50 {:.0} p95 {:.0} max {:.0} ({} ACKs)",
            q(0.05),
            q(0.5),
            q(0.95),
            turn[turn.len() - 1],
            turn.len()
        ));
    }
    // what the firmware's MAC transmissions came to (TRACE=1 images)
    let mut mac: BTreeMap<(&str, &str), (usize, u64, u64)> = BTreeMap::new();
    for tr in &a.traces {
        for r in tr.iter().filter(|r| r.kind == K_MAC && r.t >= t0 && r.t < t1) {
            let to = if r.peer == 0xffff { "broadcast" } else { "unicast" };
            let st = ["ok", "no ACK", "channel busy"].get(r.detail as usize).copied().unwrap_or("?");
            let e = mac.entry((to, st)).or_default();
            e.0 += 1;
            e.1 += ((r.aux >> 8) & 0xff) as u64;
            e.2 += (r.aux >> 16) as u64;
        }
    }
    if !mac.is_empty() {
        let v: Vec<String> = mac
            .iter()
            .map(|((to, st), (n, busy, us))| format!("{to} {st} {n} (busy CCA {:.1}, {:.1} ms each)", *busy as f64 / *n as f64, *us as f64 / *n as f64 / 1000.0))
            .collect();
        out.push(format!("   MAC transmissions (firmware): {}", v.join("; ")));
    }
    let mut pv: Vec<_> = per_rx.into_iter().collect();
    pv.sort_by(|a, b| b.1.cmp(&a.1));
    out.push(format!("   by intended receiver: {}", pv.iter().map(|(n, c)| format!("{n} {c}")).collect::<Vec<_>>().join(", ")));
    out
}

/// A drop at receiver `r` of frame `d` with radio fate `f`, with what the
/// receiver or the air was doing.
fn collide_name(m: &Mesh, a: &Account, d: &Dec, r: NodeId, f: &str) -> String {
    let frames = &m.s.w.air.frames;
    if f == "collision" {
        // the other frames on the air at r at the same time
        let lo = d.idx.saturating_sub(40);
        let hi = (d.idx + 40).min(frames.len());
        let mut who = Vec::new();
        for j in lo..hi {
            let g = &frames[j];
            if j == d.idx || !(g.t0 < d.t1 && d.t0 < g.t1) || g.who == r || m.chan.what(j, r).is_none() {
                continue;
            }
            let ack = g.psdu.first().is_some_and(|b| b & 7 == 2);
            // could the sender of d have heard the other one before it started?
            let heard = d.who.is_some_and(|s| m.chan.what(j, s).is_some());
            let before = g.t0 < d.t0;
            let what = match (ack, before, heard) {
                (true, _, _) => "an ACK",
                (false, true, true) => "a frame it could hear (CCA missed or busy ignored)",
                (false, true, false) => "a hidden node's frame",
                (false, false, true) => "a frame started during it by a node that could hear it",
                (false, false, false) => "a hidden node's frame started during it",
            };
            who.push(what);
        }
        who.sort();
        who.dedup();
        return format!("collision with {}", if who.is_empty() { "?".to_string() } else { who.join(" + ") });
    }
    let _ = a;
    if f.ends_with("transmitting") || f.ends_with("TX mode") {
        // what the receiver had on the air then
        let own = frames.iter().rev().take(60).find(|g| g.who == r && g.t0 < d.t1 + 0.3 && d.t0 < g.t1 + 0.3);
        return match own {
            Some(g) if g.psdu.first().is_some_and(|b| b & 7 == 2) => format!("{f} (sending an ACK)"),
            Some(_) => format!("{f} (sending its own frame)"),
            None => format!("{f} (no frame of its own on the air: TX settling, CCA)"),
        };
    }
    f.to_string()
}
