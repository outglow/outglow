//! Address conflicts in a mixed network (R22 3.6.1.9): several emulated bulbs
//! on one air with the plain coordinator and a scripted router that plays an
//! EmberZNet router (an IKEA TRADFRI driver, as in a real home network): it detects conflicts on
//! its own address as R22 3.6.1.9.2 asks (a frame for its address carrying
//! another destination IEEE address, a frame or Device_annce from its address
//! with another IEEE address, a Network Status 0x0d naming its address), then
//! broadcasts a Network Status 0x0d, picks a new random address and announces
//! it. Bulbs are switched off and on (power loss: the RAM is lost, the flash
//! stays as it was at the moment of the loss), neighbours change their address
//! while a bulb is off.
//!
//! Every frame that pairs a network address with an IEEE address (NWK header
//! IEEE fields, the source of the security header, Device_annce, address
//! responses, Route Request/Reply IEEE fields, Rejoin Response, Update Device)
//! is checked against the ground truth; a pair whose network address belongs
//! to another device is what makes an Ember node declare a conflict.

use std::collections::HashSet;

use super::{node_of, Sim, SimCtx, SimDef};
use crate::air::{AirFrame, NodeId};
use crate::coord::{Coord, NWK_KEY, TCLK_DEFAULT};
use crate::frames::{aps_parse, ext_bytes, mac_build, mac_parse, nwk_parse, APS_CMD, APS_DATA, NWK_CMD, NWK_DATA};
use crate::image::Image;
use crate::peer::Peer;
use crate::session::{CapFrame, R};
use crate::station::{nx, SrcIeee, Station, Tx, MAC_RETRIES};

pub fn defs() -> Vec<SimDef> {
    vec![
        SimDef { name: "addr_conflict", doc: "address conflicts after a neighbour changed its address while a bulb was off", run },
        SimDef { name: "addr_conflict_btt", doc: "how long a bulb remembers a broadcast (relays a late copy again or not)", run: run_btt },
        SimDef { name: "addr_conflict_echo", doc: "how long the bulbs keep relaying one broadcast of a router", run: run_echo },
        SimDef { name: "addr_conflict_child", doc: "a conflict reported on the address of a router that joined through a bulb", run: run_child },
        SimDef { name: "addr_conflict_multihop", doc: "a router reached through two bulbs changes its address; route discovery and relays", run: run_multihop },
        SimDef { name: "addr_conflict_house", doc: "nine bulbs, a wall-switched corridor and an Ember router over several minutes", run: run_house },
        SimDef { name: "addr_conflict_victim", doc: "a late copy of the bulb's own broadcast: does it take it for an address conflict?", run: run_victim },
        SimDef { name: "addr_conflict_channel", doc: "a channel change while a bulb is off: how it comes back, with which address", run: run_channel },
    ]
}

const QUANTUM: f64 = 0.25;
pub const IKEA_EXT: u64 = 0x180df9fffef90302;
const COORD_EXT: u64 = 0x00124B0000000001;

fn u16_at(p: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes(p.get(i..i + 2)?.try_into().ok()?))
}
fn u64_at(p: &[u8], i: usize) -> Option<u64> {
    Some(u64::from_le_bytes(p.get(i..i + 8)?.try_into().ok()?))
}

/// A (network address, IEEE address) pair a frame states, and where it was.
#[derive(Clone, Debug)]
pub struct Claim {
    pub short: u16,
    pub ext: u64,
    pub what: &'static str,
}

/// Every address pair a frame states.
pub fn claims(f: &CapFrame) -> Vec<Claim> {
    let mut v = Vec::new();
    let Some(m) = &f.mac else { return v };
    let Some(n) = &f.nwk else { return v };
    if let Some(e) = n.src_ieee {
        v.push(Claim { short: n.src, ext: e, what: "NWK source IEEE" });
    }
    if let Some(e) = n.dst_ieee {
        if n.dst < 0xFFF8 {
            v.push(Claim { short: n.dst, ext: e, what: "NWK destination IEEE" });
        }
    }
    if let (Some(e), 2, Some(s)) = (n.sec_src, m.smode, m.src) {
        v.push(Claim { short: s as u16, ext: e, what: "security source (MAC sender)" });
    }
    let Some(pl) = &n.payload else { return v };
    if n.ty == NWK_CMD {
        match pl.first() {
            Some(0x01) if pl.len() >= 6 => {
                let o = pl[1];
                if o & 0x20 != 0 {
                    if let (Some(d), Some(e)) = (u16_at(pl, 3), u64_at(pl, 6)) {
                        v.push(Claim { short: d, ext: e, what: "Route Request destination IEEE" });
                    }
                }
            }
            Some(0x02) if pl.len() >= 8 => {
                let o = pl[1];
                let (orig, resp) = (u16_at(pl, 3).unwrap(), u16_at(pl, 5).unwrap());
                let mut i = 8;
                if o & 0x10 != 0 {
                    if let Some(e) = u64_at(pl, i) {
                        v.push(Claim { short: orig, ext: e, what: "Route Reply originator IEEE" });
                    }
                    i += 8;
                }
                if o & 0x20 != 0 {
                    if let Some(e) = u64_at(pl, i) {
                        v.push(Claim { short: resp, ext: e, what: "Route Reply responder IEEE" });
                    }
                }
            }
            Some(0x07) if pl.len() >= 4 => {
                if let Some(e) = n.dst_ieee {
                    v.push(Claim { short: u16_at(pl, 1).unwrap(), ext: e, what: "Rejoin Response address" });
                }
            }
            _ => {}
        }
        return v;
    }
    let Some(a) = &f.aps else { return v };
    let Some(p) = &a.payload else { return v };
    if a.ty == APS_DATA && a.profile == Some(0) && p.len() > 1 {
        let z = &p[1..];
        match a.cluster {
            Some(0x0013) if z.len() >= 10 => {
                v.push(Claim { short: u16_at(z, 0).unwrap(), ext: u64_at(z, 2).unwrap(), what: "Device_annce" })
            }
            Some(0x8000) | Some(0x8001) if z.len() >= 11 && z[0] == 0 => v.push(Claim {
                short: u16_at(z, 9).unwrap(),
                ext: u64_at(z, 1).unwrap(),
                what: if a.cluster == Some(0x8000) { "NWK_addr_rsp" } else { "IEEE_addr_rsp" },
            }),
            Some(0x8031) if z.len() >= 4 && z[0] == 0 => {
                let cnt = z[3] as usize;
                for k in 0..cnt {
                    let o = 4 + 22 * k;
                    if let (Some(e), Some(s)) = (u64_at(z, o + 8), u16_at(z, o + 16)) {
                        v.push(Claim { short: s, ext: e, what: "Mgmt_Lqi_rsp neighbour" });
                    }
                }
            }
            _ => {}
        }
    }
    if a.ty == APS_CMD && a.cmd == Some(0x06) && p.len() >= 12 {
        v.push(Claim { short: u16_at(p, 9).unwrap(), ext: u64_at(p, 1).unwrap(), what: "Update Device" });
    }
    v
}

/// Decodes a frame as the spec session's sniffer does (`Session::decode`).
fn decode(keys: &mut crate::frames::Keys, f: &mut CapFrame) {
    let Some(m) = mac_parse(&f.psdu) else { return };
    if m.ty == 3 && m.payload.first() == Some(&0x02) && m.payload.len() >= 4 && m.dmode == 3 && m.payload[3] == 0 {
        keys.learn(Some(u16::from_le_bytes([m.payload[1], m.payload[2]])), m.dst);
    }
    let is_data = m.ty == 1 && !m.payload.is_empty();
    let (mac_src, smode, payload) = (m.src, m.smode, m.payload.clone());
    f.mac = Some(m);
    if !is_data {
        return;
    }
    let Some(n) = nwk_parse(&payload, keys) else { return };
    if n.ver != 2 || (n.ty != NWK_DATA && n.ty != NWK_CMD) {
        return;
    }
    if n.sec_src.is_some() && smode == 2 {
        keys.learn(mac_src.map(|s| s as u16), n.sec_src);
    }
    let (src, ty, pl) = (n.src, n.ty, n.payload.clone());
    f.nwk = Some(n);
    let Some(pl) = pl else { return };
    if ty != NWK_DATA {
        return;
    }
    f.aps = aps_parse(&pl, keys, Some(src));
}

/// A bulb of the scenario.
pub struct Bulb {
    pub id: NodeId,
    pub name: String,
    /// as the bulb itself states it (the stock firmware makes its own up)
    pub ext: Option<u64>,
    /// the flash at the moment of the last power loss, while it is off
    pub off: Option<Vec<u8>>,
}

/// What the scripted Ember router does next.
#[derive(Clone, Debug)]
enum Act {
    /// Network Status 0x0d to 0xFFFD naming `addr`
    Status { addr: u16 },
    /// Device_annce with the current address
    Annce,
    /// Link Status
    LinkStatus,
}

/// The scripted EmberZNet router (the IKEA driver).
pub struct Ember {
    pub id: NodeId,
    pub ext: u64,
    rng: u64,
    acts: Vec<(f64, Act)>,
    /// (ms, old, new, why)
    pub moves: Vec<(f64, u16, u16, String)>,
    pub link_status: bool,
    /// the neighbours it lists in its Link Status
    pub neighbours: Vec<u16>,
    /// detect conflicts on its own address (R22 3.6.1.9.2)
    pub detect: bool,
    /// a conflict is being resolved: ignore further triggers until this time
    busy_until: f64,
    pub zcl_answers: u32,
    /// broadcast transaction table: (source, sequence number, ms of the last copy)
    pub btt: Vec<(u16, u8, f64)>,
    /// how long a record lives (nwkNetworkBroadcastDeliveryTime)
    pub btt_ms: f64,
    /// records it holds at most (the oldest goes first)
    pub btt_size: usize,
    /// R22 3.6.1.9.2: a new broadcast record whose source is our own address is a local conflict
    pub own_bcast_rule: bool,
    /// when it started operating on the network (the rule applies only after btt_ms)
    pub t_up: f64,
    /// relay broadcasts (once, after a jitter), as every router does
    pub relay: bool,
    /// copies of its own broadcasts heard back: (ms, seq, radius, sender, already in the table)
    pub echoes: Vec<(f64, u8, u8, String, bool)>,
    /// Route Replies sent
    pub rreps: u32,
}

impl Ember {
    /// A broadcast seen (sent or received): true when it is new.
    fn btt_new(&mut self, src: u16, seq: u8, now: f64) -> bool {
        let life = self.btt_ms;
        self.btt.retain(|e| now - e.2 < life);
        if let Some(e) = self.btt.iter_mut().find(|e| e.0 == src && e.1 == seq) {
            e.2 = now;
            return false;
        }
        if self.btt.len() >= self.btt_size {
            self.btt.remove(0);
        }
        self.btt.push((src, seq, now));
        true
    }

    fn rand16(&mut self) -> u16 {
        // xorshift64*
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        (self.rng.wrapping_mul(0x2545F4914F6CDD1D) >> 32) as u16
    }
}

/// An event of the scenario's timeline.
#[derive(Clone, Debug)]
pub struct Note {
    pub ms: f64,
    pub text: String,
}

/// The network: bulbs, coordinator, the Ember router, a sniffer, the truth.
pub struct Net {
    pub s: Sim,
    pub cap: Vec<CapFrame>,
    n_raw: usize,
    pub bulbs: Vec<Bulb>,
    pub ember: Option<Ember>,
    /// ext -> current network address
    pub truth: Vec<(u64, u16, String)>,
    /// (ext, address) pairs that were once true
    pub former: Vec<(u64, u16)>,
    /// pairs of nodes out of each other's range
    pub range: HashSet<(NodeId, NodeId)>,
    pub notes: Vec<Note>,
    /// claims that contradict the truth: (frame index, claim, what is true)
    pub bad: Vec<(usize, Claim, String)>,
    pub echo: bool,
}

impl Net {
    pub fn new(first: Node0, coord: Coord, echo: bool) -> Net {
        let mut s = Sim::new(first.node, &first.name, coord, None, QUANTUM);
        s.w.air.keys.set_nwk(0, NWK_KEY);
        s.w.air.keys.add_link(TCLK_DEFAULT);
        s.w.air.keys.learn(Some(0x0000), Some(COORD_EXT));
        s.w.coord.start(&mut s.w.air);
        let b = Bulb { id: NodeId::Dut, name: first.name.clone(), ext: None, off: None };
        Net {
            s,
            cap: Vec::new(),
            n_raw: 0,
            bulbs: vec![b],
            ember: None,
            truth: vec![(COORD_EXT, 0x0000, "coord".into())],
            former: Vec::new(),
            range: HashSet::new(),
            notes: Vec::new(),
            bad: Vec::new(),
            echo,
        }
    }

    pub fn now(&self) -> f64 {
        self.s.now()
    }

    pub fn note(&mut self, text: impl Into<String>) {
        let n = Note { ms: self.now(), text: text.into() };
        if self.echo {
            println!("[{:9.3} s] {}", n.ms / 1000.0, n.text);
        }
        self.notes.push(n);
    }

    pub fn add_bulb(&mut self, n: Node0, t_off: f64) -> NodeId {
        let id = self.s.w.add_bulb(n.node, &n.name);
        self.s.w.set_t_off(id, t_off);
        self.bulbs.push(Bulb { id, name: n.name, ext: None, off: None });
        id
    }

    pub fn bulb(&self, name: &str) -> &Bulb {
        self.bulbs.iter().find(|b| b.name == name).unwrap()
    }

    pub fn name_of(&self, id: NodeId) -> String {
        self.s.w.air.name_of(id)
    }

    pub fn short_of(&self, ext: u64) -> Option<u16> {
        self.truth.iter().find(|t| t.0 == ext).map(|t| t.1)
    }

    pub fn owner_of(&self, short: u16) -> Option<(u64, String)> {
        self.truth.iter().find(|t| t.1 == short).map(|t| (t.0, t.2.clone()))
    }

    pub fn set_truth(&mut self, ext: u64, short: u16, name: &str) {
        match self.truth.iter_mut().find(|t| t.0 == ext) {
            Some(t) if t.1 == short => return,
            Some(t) => {
                let old = t.1;
                t.1 = short;
                self.former.push((ext, old));
                let n = name.to_string();
                self.note(format!("truth: {n} {old:#06x} -> {short:#06x}"));
            }
            None => {
                self.truth.push((ext, short, name.to_string()));
                self.note(format!("truth: {name} is {short:#06x}"));
            }
        }
    }

    /// The nodes that are not powered (all bulbs that are off).
    fn is_off(&self, id: NodeId) -> bool {
        self.bulbs.iter().any(|b| b.id == id && b.off.is_some())
    }

    /// Applies range and power to the air's blocked pairs.
    pub fn apply_blocks(&mut self) {
        let nodes = self.s.w.air.nodes.clone();
        for &a in &nodes {
            for &b in &nodes {
                if a == b {
                    continue;
                }
                let blocked = self.is_off(a) || self.is_off(b) || self.range.contains(&(a, b)) || self.range.contains(&(b, a));
                if blocked {
                    self.s.w.air.blocked.insert((a, b));
                } else {
                    self.s.w.air.blocked.remove(&(a, b));
                }
            }
        }
    }

    pub fn out_of_range(&mut self, a: NodeId, b: NodeId, out: bool) {
        if out {
            self.range.insert((a, b));
        } else {
            self.range.remove(&(a, b));
            self.range.remove(&(b, a));
        }
        self.apply_blocks();
    }

    /// Power loss: the bulb goes silent; its flash is kept as it is now.
    pub fn power_off(&mut self, name: &str) {
        let i = self.bulbs.iter().position(|b| b.name == name).unwrap();
        let id = self.bulbs[i].id;
        let snap = self.s.w.node(id).m.flash.mem.to_vec();
        self.bulbs[i].off = Some(snap);
        self.apply_blocks();
        self.note(format!("power: {name} off"));
    }

    /// Power returns: the flash as it was at the power loss, a cold boot.
    pub fn power_on(&mut self, name: &str) -> R<()> {
        let i = self.bulbs.iter().position(|b| b.name == name).unwrap();
        let id = self.bulbs[i].id;
        let snap = self.bulbs[i].off.take().unwrap();
        self.s.w.node_mut(id).m.flash.mem.copy_from_slice(&snap);
        self.s.w.reboot_bulb(id).map_err(super::emu)?;
        self.apply_blocks();
        self.note(format!("power: {name} on"));
        Ok(())
    }

    // ------------------------------------------------------------- running
    /// Runs `ms`, decoding the air and letting the Ember router react every `step`.
    pub fn run(&mut self, ms: f64) -> R<()> {
        let end = self.now() + ms;
        while self.now() < end {
            let t = end.min(self.now() + 5.0);
            self.s.w.lockstep(t, self.s.quantum).map_err(super::emu)?;
            self.sync();
            self.ember_tick();
        }
        Ok(())
    }

    pub fn run_until(&mut self, max_ms: f64, mut cond: impl FnMut(&Net) -> bool) -> R<bool> {
        let end = self.now() + max_ms;
        while self.now() < end {
            if cond(self) {
                return Ok(true);
            }
            self.run(10.0)?;
        }
        Ok(cond(self))
    }

    fn sync(&mut self) {
        while self.n_raw < self.s.w.air.frames.len() {
            let AirFrame { t0, t1, ch, who, psdu } = self.s.w.air.frames[self.n_raw].clone();
            self.n_raw += 1;
            let who_name = self.s.w.air.name_of(who);
            let mut f = CapFrame { i: self.cap.len(), t0, t1, ch, who, who_name, psdu, mac: None, nwk: None, aps: None };
            decode(&mut self.s.w.air.keys, &mut f);
            self.learn_truth(&f);
            self.check_claims(&f);
            self.ember_on_frame(&f);
            self.cap.push(f);
        }
    }

    /// A bulb's own frames tell its current address.
    fn learn_truth(&mut self, f: &CapFrame) {
        let Some(bi) = self.bulbs.iter().position(|b| b.id == f.who) else { return };
        let (Some(m), Some(n)) = (&f.mac, &f.nwk) else { return };
        if m.smode != 2 || m.src != Some(n.src as u64) || n.src >= 0xFFF8 {
            return;
        }
        let Some(sx) = n.sec_src else { return };
        if self.bulbs[bi].ext.is_none() {
            self.bulbs[bi].ext = Some(sx);
            let nm = self.bulbs[bi].name.clone();
            self.note(format!("{nm} is {sx:#018x}"));
        }
        let (ext, name) = (self.bulbs[bi].ext.unwrap(), self.bulbs[bi].name.clone());
        if sx == ext {
            self.set_truth(ext, n.src, &name);
        }
    }

    pub fn short_of_bulb(&self, name: &str) -> Option<u16> {
        self.bulb(name).ext.and_then(|e| self.short_of(e))
    }

    fn check_claims(&mut self, f: &CapFrame) {
        for c in claims(f) {
            if c.short >= 0xFFF8 {
                continue;
            }
            let owner = self.owner_of(c.short);
            let cur = self.short_of(c.ext);
            let verdict = match (&owner, cur) {
                (Some((e, _)), _) if *e == c.ext => continue,
                (Some((_, who)), _) => format!("{:#06x} belongs to {who}", c.short),
                (None, Some(s)) => format!("stale: that IEEE is now {s:#06x}"),
                (None, None) => "unknown pair".to_string(),
            };
            // during a change the new address is known to the device before the truth learns it
            if self.ember.as_ref().is_some_and(|e| e.ext == c.ext && f.who == e.id) {
                continue;
            }
            let txt = format!(
                "BAD PAIR from {}: {} says {:#06x} = {:#018x} ({verdict}) in [{}]",
                f.who_name,
                c.what,
                c.short,
                c.ext,
                f.brief()
            );
            self.note(txt);
            self.bad.push((f.i, c, verdict));
        }
    }

    // ------------------------------------------------------------- the Ember router
    pub fn add_ember(&mut self, short: u16, seed: u64) -> NodeId {
        let c = &self.s.w.coord.st;
        let mut st = Station::new(NodeId::Coord, c.channel, c.pan, Some(short), IKEA_EXT, NWK_KEY, (0x11, 0x21, 0x31, 0x41, 0x51), 0x1000, 2);
        st.key_seq = c.key_seq;
        let id = self.s.w.add_peer_node(Peer::bare_ack(st, "ikea"));
        self.ember = Some(Ember {
            id,
            ext: IKEA_EXT,
            rng: seed | 1,
            acts: Vec::new(),
            moves: Vec::new(),
            link_status: true,
            neighbours: Vec::new(),
            detect: true,
            busy_until: 0.0,
            zcl_answers: 0,
            btt: Vec::new(),
            btt_ms: 9000.0,
            btt_size: 15,
            own_bcast_rule: true,
            t_up: self.s.now(),
            relay: true,
            echoes: Vec::new(),
            rreps: 0,
        });
        self.set_truth(IKEA_EXT, short, "ikea");
        self.s.w.air.keys.learn(Some(short), Some(IKEA_EXT));
        self.apply_blocks();
        id
    }

    pub fn ember_short(&self) -> u16 {
        let e = self.ember.as_ref().unwrap();
        self.s.w.peers[match e.id {
            NodeId::Peer(i) => i as usize,
            _ => unreachable!(),
        }]
        .st
        .short16()
    }

    fn ember_st(&mut self) -> (&mut Station, &mut crate::air::Air) {
        let id = self.ember.as_ref().unwrap().id;
        self.s.w.st(id)
    }

    /// Device_annce from the Ember router (broadcast to 0xFFFD, router capability).
    pub fn ember_annce(&mut self) {
        let me = self.ember_short();
        let mut body = me.to_le_bytes().to_vec();
        body.extend_from_slice(&ext_bytes(IKEA_EXT));
        body.push(0x8E);
        let (st, air) = self.ember_st();
        st.tx_zdo(air, 0xFFFD, 0x0013, &body, &Tx::new().src_ieee(SrcIeee::Own));
        self.note(format!("ikea: Device_annce {me:#06x}"));
    }

    fn ember_link_status(&mut self) {
        let me = self.ember_short();
        let mut nb: Vec<u16> = self.ember.as_ref().unwrap().neighbours.clone();
        nb.retain(|a| *a != me);
        let entries: Vec<(u16, u8, u8)> = nb.iter().map(|a| (*a, 1, 1)).collect();
        let (st, air) = self.ember_st();
        st.tx_link_status(air, &entries, &Tx::new());
    }

    /// Starts its periodic Link Status (every 15 s).
    pub fn ember_start(&mut self) {
        let t = self.now() + 1000.0;
        self.ember.as_mut().unwrap().acts.push((t, Act::LinkStatus));
    }

    /// A new address, as after a conflict: Network Status 0x0d with the old one, Device_annce.
    fn ember_conflict(&mut self, why: String, from_status: bool) {
        let now = self.now();
        let old = self.ember_short();
        let known: Vec<u16> = self.truth.iter().map(|t| t.1).collect();
        let e = self.ember.as_mut().unwrap();
        if now < e.busy_until {
            return;
        }
        e.busy_until = now + 3000.0;
        let mut new = e.rand16();
        while new == 0 || new >= 0xFFF8 || known.contains(&new) {
            new = e.rand16();
        }
        let jitter = (e.rand16() % 64) as f64;
        if !from_status {
            e.acts.push((now + jitter, Act::Status { addr: old }));
        }
        e.acts.push((now + 1000.0 + jitter, Act::Annce));
        e.moves.push((now, old, new, why.clone()));
        let pi = match e.id {
            NodeId::Peer(i) => i as usize,
            _ => unreachable!(),
        };
        self.s.w.peers[pi].st.short = Some(new);
        self.s.w.air.keys.learn(Some(new), Some(IKEA_EXT));
        self.note(format!("ikea: CONFLICT on {old:#06x} ({why}) -> new address {new:#06x}"));
        self.set_truth(IKEA_EXT, new, "ikea");
    }

    /// The Ember router changes its address now (as if a conflict had been resolved earlier).
    pub fn ember_move(&mut self, why: &str) {
        self.ember_conflict(why.to_string(), true);
    }

    fn ember_on_frame(&mut self, f: &CapFrame) {
        let Some(e) = &self.ember else { return };
        let eid = e.id;
        let me = self.ember_short();
        let now = self.now();
        if f.who == eid {
            // its own broadcasts go into its table when they are sent
            if let Some(n) = &f.nwk {
                if n.dst >= 0xFFF8 && f.mac.as_ref().is_some_and(|m| m.src == Some(me as u64)) {
                    self.ember.as_mut().unwrap().btt_new(n.src, n.seq, now);
                }
            }
            return;
        }
        if self.s.w.air.blocked.contains(&(f.who, eid)) {
            return;
        }
        let Some(n) = &f.nwk else { return };
        // the MAC delivers only frames for our address or broadcasts
        let mac_dst = f.mac.as_ref().and_then(|m| m.dst);
        if mac_dst != Some(me as u64) && mac_dst != Some(0xFFFF) && mac_dst != Some(IKEA_EXT) {
            return;
        }
        let mut why = None;
        let mut from_status = false;
        if n.dst >= 0xFFF8 && n.ver == 2 {
            let e = self.ember.as_mut().unwrap();
            let new = e.btt_new(n.src, n.seq, now);
            if n.src == me {
                e.echoes.push((now, n.seq, n.radius, f.who_name.clone(), !new));
                if new && e.own_bcast_rule && now - e.t_up > e.btt_ms {
                    why = Some(format!(
                        "new broadcast record with our own address as source (seq {}, radius {}) relayed by {}",
                        n.seq, n.radius, f.who_name
                    ));
                }
            }
            // relay a new broadcast once (radius - 1), as a router does
            if new && e.relay && n.src != me && n.radius > 1 && why.is_none() {
                if let Some(pl) = n.payload.clone() {
                    let jitter = 2.0 + (e.rand16() % 64) as f64;
                    let o = Tx {
                        src: Some(n.src),
                        nwk_seq: Some(n.seq),
                        radius: Some(n.radius - 1),
                        src_ieee: match n.src_ieee {
                            Some(x) => SrcIeee::Is(x),
                            None => SrcIeee::None,
                        },
                        mac_dst: Some(0xFFFF),
                        delay: Some(jitter),
                        discover: n.discover,
                        ..Tx::new()
                    };
                    let (ty, dst) = (n.ty, n.dst);
                    let pi = peer_ix(eid);
                    let World2 { st, air } = split(&mut self.s, pi);
                    st.tx_nwk(air, ty, dst, &pl, &o);
                }
            }
        }
        if !self.ember.as_ref().unwrap().detect {
            return;
        }
        if n.dst == me {
            if let Some(d) = n.dst_ieee.filter(|d| *d != IKEA_EXT) {
                why = Some(format!("frame for {me:#06x} with destination IEEE {d:#018x} from {}", f.who_name));
            }
        }
        if n.src == me {
            if let Some(s) = n.src_ieee.filter(|s| *s != IKEA_EXT) {
                why = Some(format!("frame from {me:#06x} with source IEEE {s:#018x} via {}", f.who_name));
            }
        }
        if f.nwk_cmd() == Some(0x03) {
            let p = n.payload.as_ref().unwrap();
            if p.get(1) == Some(&0x0D) && u16_at(p, 2) == Some(me) {
                why = Some(format!("Network Status 0x0d for {me:#06x} from {:#06x} via {}", n.src, f.who_name));
                from_status = true;
            }
        }
        for c in claims(f) {
            if c.what == "Device_annce" && c.short == me && c.ext != IKEA_EXT {
                why = Some(format!("Device_annce {me:#06x} = {:#018x} via {}", c.ext, f.who_name));
            }
        }
        // a Route Request for our address: Route Reply to the previous hop (R22 3.6.3.5.2),
        // with both IEEE addresses as an Ember router sends them
        if f.nwk_cmd() == Some(0x01) && why.is_none() {
            let p = n.payload.as_ref().unwrap();
            if p.len() >= 6 && u16_at(p, 3) == Some(me) {
                let hop = f.mac.as_ref().and_then(|m| m.src).unwrap_or(n.src as u64) as u16;
                let mut b = vec![0x02, 0x30, p[2]];
                b.extend_from_slice(&n.src.to_le_bytes());
                b.extend_from_slice(&me.to_le_bytes());
                b.push(1);
                b.extend_from_slice(&ext_bytes(n.src_ieee.unwrap_or(0)));
                b.extend_from_slice(&ext_bytes(IKEA_EXT));
                let dst_ieee = n.src_ieee;
                let src = n.src;
                let pi = peer_ix(eid);
                let World2 { st, air } = split(&mut self.s, pi);
                let mut o = Tx::new().radius(30).mac_dst(hop).delay(5.0);
                o.dst_ieee = dst_ieee;
                st.tx_nwk_cmd(air, src, &b, &o);
                self.ember.as_mut().unwrap().rreps += 1;
            }
        }
        // a unicast ZCL command for us that wants an answer: Default Response back to the source
        if n.dst == me && n.ty == NWK_DATA {
            if let (Some(a), Some(z)) = (&f.aps, f.zcl()) {
                if a.profile == Some(0x0104) && !z.ddr && z.specific {
                    let (src, cl, seq, cmd, ep) = (n.src, a.cluster.unwrap_or(0), z.seq, z.cmd, a.src_ep.unwrap_or(1));
                    let o = Tx::new().ep(ep).src_ep(1).seq(seq).general().ddr().to_client().src_ieee(SrcIeee::None);
                    let pi = match eid {
                        NodeId::Peer(i) => i as usize,
                        _ => unreachable!(),
                    };
                    let World2 { st, air } = split(&mut self.s, pi);
                    st.tx_zcl(air, src, cl, 0x0B, &[cmd, 0x00], &o);
                    self.ember.as_mut().unwrap().zcl_answers += 1;
                }
            }
        }
        if let Some(w) = why {
            self.ember_conflict(w, from_status);
        }
    }

    fn ember_tick(&mut self) {
        let now = self.now();
        let Some(e) = &mut self.ember else { return };
        let due: Vec<Act> = {
            let (d, keep): (Vec<_>, Vec<_>) = e.acts.drain(..).partition(|(t, _)| *t <= now);
            e.acts = keep;
            d.into_iter().map(|(_, a)| a).collect()
        };
        for a in due {
            match a {
                Act::Status { addr } => {
                    let (st, air) = self.ember_st();
                    st.tx_nwk_cmd(air, 0xFFFD, &[0x03, 0x0D, addr as u8, (addr >> 8) as u8], &Tx::new().radius(30));
                    self.note(format!("ikea: Network Status 0x0d for {addr:#06x}"));
                }
                Act::Annce => self.ember_annce(),
                Act::LinkStatus => {
                    let e = self.ember.as_mut().unwrap();
                    if e.link_status {
                        e.acts.push((now + 15000.0, Act::LinkStatus));
                        self.ember_link_status();
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------- joining
    /// The Ember router joins through `parent` by MAC association (as a router).
    pub fn ember_join_via(&mut self, parent: NodeId) -> R<Option<u16>> {
        let pshort = self.bulbs.iter().find(|b| b.id == parent).and_then(|b| b.ext).and_then(|e| self.short_of(e)).unwrap();
        let eid = self.ember.as_ref().unwrap().id;
        let pan = self.s.w.coord.st.pan;
        self.s.w.peers[peer_ix(eid)].st.short = None;
        {
            let (st, air) = self.s.w.st(eid);
            let seq = nx(&mut st.mac_seq);
            let mpdu = mac_build(3, seq, &[0x07], 0xFFFF, 0xFFFF, 2, 0, 0, None, false, false);
            st.send_psdu(air, mpdu, 0.4, None, None);
        }
        self.run(300.0)?;
        let mark = self.cap.len();
        {
            let (st, air) = self.s.w.st(eid);
            let seq = nx(&mut st.mac_seq);
            let mpdu = mac_build(3, seq, &[0x01, 0x8E], pan, pshort as u64, 2, IKEA_EXT, 3, Some(0xFFFF), true, false);
            st.send_psdu(air, mpdu, 0.4, Some(MAC_RETRIES), None);
        }
        self.run(500.0)?;
        let mut short = None;
        for _ in 0..6 {
            {
                let (st, air) = self.s.w.st(eid);
                let seq = nx(&mut st.mac_seq);
                let mpdu = mac_build(3, seq, &[0x04], pan, pshort as u64, 2, IKEA_EXT, 3, Some(pan), true, false);
                st.send_psdu(air, mpdu, 0.4, Some(MAC_RETRIES), None);
            }
            self.run(100.0)?;
            if let Some(f) = self.cap[mark..].iter().find(|f| f.who == parent && f.mac_cmd() == Some(0x02)) {
                let p = &f.mac.as_ref().unwrap().payload;
                if p[3] == 0 {
                    short = u16_at(p, 1);
                }
                break;
            }
        }
        let Some(short) = short else { return Ok(None) };
        self.s.w.peers[peer_ix(eid)].st.short = Some(short);
        self.s.w.air.keys.learn(Some(short), Some(IKEA_EXT));
        self.set_truth(IKEA_EXT, short, "ikea");
        // the network key comes through the parent (Update Device, Tunnel, Transport Key)
        let ok = self.run_until(4000.0, |n| {
            n.cap[mark..].iter().any(|f| f.who == parent && f.aps_cmd() == Some(0x05))
        })?;
        self.note(format!("ikea: associated through {} as {short:#06x} (network key {})", self.name_of(parent),
                          if ok { "received" } else { "NOT received" }));
        self.run(200.0)?;
        self.ember_annce();
        Ok(Some(short))
    }

    // ------------------------------------------------------------- traffic
    /// On/Off Toggle from the coordinator to `short` (ep 1), routed through `via` if given.
    pub fn coord_cmd(&mut self, short: u16, via: Option<u16>) {
        match via {
            Some(v) => {
                self.s.w.coord.st.via.insert(short, v);
            }
            None => {
                self.s.w.coord.st.via.remove(&short);
            }
        }
        // route discovery enabled, as an Ember coordinator sends it
        let o = Tx::new().discover(1);
        let (st, air) = self.s.w.st(NodeId::Coord);
        st.tx_zcl(air, short, 0x0006, 0x02, &[], &o);
    }

    /// Prints the frames of the window that pair addresses or are NWK commands / ZDO.
    pub fn dump(&self, from: usize, to: usize, filter: impl Fn(&CapFrame) -> bool) -> Vec<String> {
        let mut out = Vec::new();
        for f in &self.cap[from.min(self.cap.len())..to.min(self.cap.len())] {
            if !filter(f) {
                continue;
            }
            let mut extra = String::new();
            if let Some(n) = &f.nwk {
                if let Some(d) = n.dst_ieee {
                    extra += &format!(" dstIEEE={d:#018x}");
                }
                if let Some(s) = n.src_ieee {
                    extra += &format!(" srcIEEE={s:#018x}");
                }
                if let (Some(m), Some(s)) = (&f.mac, n.sec_src) {
                    extra += &format!(" mac={:#06x}->{:#06x} sec={s:#018x}", m.src.unwrap_or(0), m.dst.unwrap_or(0));
                }
            }
            out.push(format!("  [{:9.3} s] {}{extra}", f.t0 / 1000.0, f.brief()));
        }
        out
    }
}

fn peer_ix(id: NodeId) -> usize {
    match id {
        NodeId::Peer(i) => i as usize,
        _ => panic!("not a peer"),
    }
}

struct World2<'a> {
    st: &'a mut Station,
    air: &'a mut crate::air::Air,
}

fn split(s: &mut Sim, pi: usize) -> World2<'_> {
    let crate::air::World { air, peers, .. } = &mut s.w;
    World2 { st: &mut peers[pi].st, air }
}

/// A bulb node with its name; `ext` gives the bytes of its factory MAC record.
pub struct Node0 {
    pub node: zbnode::Node,
    pub name: String,
    pub ext: u64,
}

pub fn bulb_node(img: &Image, cpi: i64, name: &str, ext: u64) -> R<Node0> {
    let mut node = node_of(img, cpi)?;
    // the factory MAC record at 0xFF000 as the stock firmware keeps it: the
    // BLE MAC (little endian, Telink OUI a4:c1:38 last) and two more bytes;
    // anything else (an 8-byte IEEE address) it replaces by a random one, the
    // same in every emulated bulb. Our images read the 8 bytes as their IEEE address.
    let e = ext.to_le_bytes();
    node.write_flash(0xFF000, &[e[0], e[1], e[2], 0x38, 0xC1, 0xA4, e[3], e[4] & 0x7F]);
    Ok(Node0 { node, name: name.to_string(), ext })
}

/// Is `f` interesting for the timeline: NWK commands other than Link Status, ZDO, APS commands.
pub fn interesting(f: &CapFrame) -> bool {
    match (f.nwk_cmd(), &f.aps) {
        (Some(0x08), _) => false,
        (Some(_), _) => true,
        (None, Some(a)) => a.profile == Some(0) || a.ty == APS_CMD,
        _ => f.mac_cmd().is_some() && f.mac_cmd() != Some(0x07),
    }
}

// =================================================================== probes
impl Net {
    /// A ZDO request from the coordinator.
    pub fn coord_zdo(&mut self, dst: u16, cluster: u16, payload: &[u8]) {
        self.s.w.coord.zdo_send(&mut self.s.w.air, dst, cluster, payload);
    }

    /// A Route Request from the coordinator for `target` (with its IEEE address if given).
    pub fn coord_rreq(&mut self, target: u16, ieee: Option<u64>, id: u8) {
        let mut b = vec![0x01, if ieee.is_some() { 0x20 } else { 0x00 }, id];
        b.extend_from_slice(&target.to_le_bytes());
        b.push(0);
        if let Some(e) = ieee {
            b.extend_from_slice(&ext_bytes(e));
        }
        let c = &mut self.s.w.coord;
        c.st.tx_nwk_cmd(&mut self.s.w.air, 0xFFFC, &b, &Tx::new().radius(30).mac_dst(0xFFFF));
    }

    /// A bulb's neighbour table (Mgmt_Lqi_req, all pages): (address, IEEE, type/rx/relationship byte, depth, lqi).
    pub fn lqi_table(&mut self, short: u16) -> R<Option<Vec<(u16, u64, u8, u8, u8)>>> {
        let (mut out, mut start) = (Vec::new(), 0u8);
        loop {
            let n0 = self.s.w.coord.zcl_rsp.len();
            self.coord_zdo(short, 0x0031, &[start]);
            let got = self.run_until(3000.0, |n| n.s.w.coord.zcl_rsp[n0..].iter().any(|r| r.src == short && r.cluster == 0x8031))?;
            if !got {
                return Ok(None);
            }
            let p = self.s.w.coord.zcl_rsp[n0..].iter().find(|r| r.src == short && r.cluster == 0x8031).unwrap().body.clone();
            if p.len() < 5 || p[1] != 0 {
                return Ok(None);
            }
            let (total, count) = (p[2], p[4]);
            for i in 0..count as usize {
                let o = 5 + 22 * i;
                if p.len() < o + 22 {
                    break;
                }
                out.push((u16_at(&p, o + 16).unwrap(), u64_at(&p, o + 8).unwrap(), p[o + 18], p[o + 20], p[o + 21]));
            }
            start = start.wrapping_add(count);
            if start >= total || count == 0 {
                return Ok(Some(out));
            }
        }
    }

    /// The neighbour table as text, each entry checked against the truth.
    pub fn lqi_text(&mut self, short: u16) -> R<String> {
        let Some(t) = self.lqi_table(short)? else { return Ok("no answer".into()) };
        let rel = ["parent", "child", "sibling", "none", "previous child", "?", "?", "?"];
        Ok(t.iter()
            .map(|(s, e, f, d, l)| {
                let who = match self.owner_of(*s) {
                    Some((x, w)) if x == *e => w,
                    Some((_, w)) => format!("WRONG: {s:#06x} is {w}"),
                    None => match self.truth.iter().find(|t| t.0 == *e) {
                        Some(t) => format!("STALE: {} is now {:#06x}", t.2, t.1),
                        None => "unknown".into(),
                    },
                };
                format!("{s:#06x} {e:#018x} {} depth {d} lqi {l} [{who}]", rel[((f >> 4) & 7) as usize])
            })
            .collect::<Vec<_>>()
            .join("; "))
    }

    /// Where the 8 bytes of `ext` (little endian) are in a bulb's flash between `a` and `b`.
    pub fn flash_find(&self, id: NodeId, ext: u64, a: usize, b: usize) -> Vec<(usize, String)> {
        let mem = &self.s.w.node(id).m.flash.mem;
        let pat = ext.to_le_bytes();
        let mut out = Vec::new();
        let mut i = a;
        while i + 8 <= b {
            if mem[i..i + 8] == pat {
                let lo = i.saturating_sub(8);
                out.push((i, mem[lo..(i + 16).min(b)].iter().map(|x| format!("{x:02x}")).collect()));
            }
            i += 1;
        }
        out
    }
}

/// Every frame `who` sent since `mark` (None: anyone), with the pairs it states.
fn show_from(cx: &mut SimCtx, net: &Net, mark: usize, who: Option<NodeId>, skip_ls: bool) {
    let lines = net.dump(mark, net.cap.len(), |f| {
        who.map_or(true, |w| f.who == w) && !(skip_ls && f.nwk_cmd() == Some(0x08)) && f.mac.as_ref().is_some_and(|m| m.ty != 2)
    });
    for l in lines {
        cx.say(&l);
    }
}

type Probe = (&'static str, Box<dyn Fn(&mut Net)>);

// =================================================================== the scenario
fn run(cx: &mut SimCtx) -> R<()> {
    let img_d = cx.image("bulb", "stock/stock-dimm.bin");
    let echo = cx.verbose > 0;
    cx.say(&format!("image: {}", super::util::rel(&img_d)));
    let k1 = bulb_node(&img_d, cx.cpi, "K1", 0xa4c1380000000001)?;
    let k3 = bulb_node(&img_d, cx.cpi, "K3", 0xa4c1380000000003)?;
    let w2 = bulb_node(&img_d, cx.cpi, "W2", 0xa4c1380000000102)?;
    let mut net = Net::new(k1, Coord::new_plain(11, NWK_KEY), echo);
    let k3id = net.add_bulb(k3, 700.0);
    let w2id = net.add_bulb(w2, 1400.0);
    let k1id = NodeId::Dut;
    let ok = net.run_until(60000.0, |n| n.bulbs.iter().all(|b| n.short_of_bulb(&b.name).is_some()))?;
    let shorts: Vec<String> =
        net.bulbs.iter().map(|b| format!("{}={:?}", b.name, net.short_of_bulb(&b.name).map(|s| format!("{s:#06x}")))).collect();
    cx.check(net.now(), "bulbs join", ok, &shorts.join(" "));
    net.run(5000.0)?;
    // the IKEA router joins through K1, out of the coordinator's range
    let eid = net.add_ember(0x1C3B, 0x1234_5678);
    net.out_of_range(eid, NodeId::Coord, true);
    net.s.w.coord.permit_joining(&mut net.s.w.air, 254);
    net.run(1000.0)?;
    let js = net.ember_join_via(k1id)?;
    cx.check(net.now(), "ikea joins through K1", js.is_some(), &format!("{js:?}"));
    let k1s = net.short_of_bulb("K1").unwrap();
    net.ember.as_mut().unwrap().neighbours = net.bulbs.iter().filter_map(|b| net.short_of_bulb(&b.name)).collect();
    net.ember_start();
    net.s.w.peers[peer_ix(eid)].st.via.insert(0x0000, k1s);
    net.run(20000.0)?;
    for _ in 0..3 {
        let e = net.ember_short();
        net.coord_cmd(e, Some(k1s));
        net.run(2000.0)?;
    }
    let ans = net.ember.as_ref().unwrap().zcl_answers;
    cx.check(net.now(), "ikea answers commands through K1", ans > 0, &format!("{ans} answers"));
    for (a, t) in net.flash_find(k1id, IKEA_EXT, 0xC0000, 0x100000) {
        cx.say(&format!("   K1 flash (before off) {a:#x}: {t}"));
    }
    // K1 off; the ikea changes its address meanwhile; K1 on again
    net.power_off("K1");
    net.run(5000.0)?;
    net.ember_move("forced while K1 is off");
    net.run(13000.0)?;
    let ikea_old = net.ember.as_ref().unwrap().moves.last().unwrap().1;
    net.power_on("K1")?;
    let t_on = net.now();
    let mark = net.cap.len();
    net.run(20000.0)?;
    cx.say("-- K1 after power-on, 20 s");
    show_from(cx, &net, mark, Some(k1id), true);
    let ikea_new = net.ember_short();
    let k1s = net.short_of_bulb("K1").unwrap();
    let addr_req = |e: u64| {
        let mut p = ext_bytes(e).to_vec();
        p.extend_from_slice(&[0, 0]);
        p
    };
    let ieee_req = |s: u16| {
        let mut p = s.to_le_bytes().to_vec();
        p.extend_from_slice(&[0, 0]);
        p
    };
    let probes: Vec<Probe> = vec![
        ("NWK_addr_req(ikea IEEE) unicast to K1", Box::new(move |n: &mut Net| n.coord_zdo(k1s, 0x0000, &addr_req(IKEA_EXT)))),
        ("NWK_addr_req(ikea IEEE) broadcast 0xfffd", Box::new(move |n: &mut Net| n.coord_zdo(0xFFFD, 0x0000, &addr_req(IKEA_EXT)))),
        ("IEEE_addr_req(old ikea address) unicast to K1", Box::new(move |n: &mut Net| n.coord_zdo(k1s, 0x0001, &ieee_req(ikea_old)))),
        ("IEEE_addr_req(new ikea address) unicast to K1", Box::new(move |n: &mut Net| n.coord_zdo(k1s, 0x0001, &ieee_req(ikea_new)))),
        ("Mgmt_Lqi_req to K1", Box::new(move |n: &mut Net| n.coord_zdo(k1s, 0x0031, &[0]))),
        ("Route Request for the old ikea address", Box::new(move |n: &mut Net| n.coord_rreq(ikea_old, None, 0x51))),
        ("Route Request for the new ikea address", Box::new(move |n: &mut Net| n.coord_rreq(ikea_new, None, 0x52))),
        ("Route Request for the new ikea address with its IEEE", Box::new(move |n: &mut Net| n.coord_rreq(ikea_new, Some(IKEA_EXT), 0x53))),
        ("many-to-one route request, then On/Off to K1", Box::new(move |n: &mut Net| {
            n.s.w.coord.many_to_one_route_request(&mut n.s.w.air, false);
            n.coord_cmd(k1s, None);
        })),
        ("On/Off to the ikea through K1", Box::new(move |n: &mut Net| n.coord_cmd(ikea_new, Some(k1s)))),
        ("On/Off to the old ikea address through K1", Box::new(move |n: &mut Net| n.coord_cmd(ikea_old, Some(k1s)))),
    ];
    for (what, f) in probes {
        let mark = net.cap.len();
        f(&mut net);
        net.run(4000.0)?;
        cx.say(&format!("-- probe: {what}"));
        show_from(cx, &net, mark, None, true);
    }
    for (a, t) in net.flash_find(k1id, IKEA_EXT, 0xC0000, 0x100000) {
        cx.say(&format!("   K1 flash (end) {a:#x}: {t}"));
    }
    for n in net.notes.clone() {
        cx.say(&format!("[{:9.3} s] {}", n.ms / 1000.0, n.text));
    }
    let moves = net.ember.as_ref().unwrap().moves.len();
    cx.check(
        net.now(),
        "after K1 power-on",
        true,
        &format!("ikea moves {moves}, bad pairs {}, since on {:.0} s", net.bad.len(), (net.now() - t_on) / 1000.0),
    );
    let nodes: Vec<(NodeId, &str)> = vec![(k1id, "K1"), (k3id, "K3"), (w2id, "W2")];
    cx.finish(&net.s, &nodes);
    Ok(())
}


// =================================================================== broadcast table probe
fn r_tx_bcast(net: &mut Net, rid: NodeId, seq: u8) {
    let o = Tx::new().ddr().radius(5).nwk_seq(seq).src_ieee(SrcIeee::None);
    let (st, air) = net.s.w.st(rid);
    st.tx_zcl(air, 0xFFFD, 0x0003, 0x01, &[], &o);
}

fn relays_of(net: &Net, mark: usize, src: u16, seq: u8) -> usize {
    net.cap[mark..].iter().filter(|f| f.who == NodeId::Dut && f.nwk.as_ref().is_some_and(|n| n.src == src && n.seq == seq)).count()
}

/// How long a bulb remembers a broadcast (R22 3.6.5 broadcast transaction table):
/// the same frame (NWK source, sequence number) again after a gap; does the bulb relay it again?
fn run_btt(cx: &mut SimCtx) -> R<()> {
    let img = cx.image("bulb", "stock/stock-dimm.bin");
    cx.say(&format!("image: {}", super::util::rel(&img)));
    let b = bulb_node(&img, cx.cpi, "B", 0xa4c1380000000011)?;
    let mut net = Net::new(b, Coord::new_plain(11, NWK_KEY), cx.verbose > 0);
    let ok = net.run_until(60000.0, |n| n.short_of_bulb("B").is_some())?;
    cx.check(net.now(), "bulb joins", ok, "");
    net.run(5000.0)?;
    let c = &net.s.w.coord.st;
    let mut st = Station::new(NodeId::Coord, c.channel, c.pan, Some(0x4444), 0x00124B00AA004444, NWK_KEY, (1, 2, 3, 4, 5), 0x500, 0);
    st.key_seq = c.key_seq;
    let rid = net.s.w.add_peer_node(Peer::bare_ack(st, "r"));
    net.s.w.air.keys.learn(Some(0x4444), Some(0x00124B00AA004444));
    // the coordinator does not hear r: only the bulb's relays count
    net.out_of_range(rid, NodeId::Coord, true);
    let mut seq = 0x60u8;
    let mut lines = Vec::new();
    for gap in [500.0, 1000.0, 1500.0, 2000.0, 2500.0, 3000.0, 4000.0, 6000.0, 9000.0] {
        seq = seq.wrapping_add(1);
        let m1 = net.cap.len();
        r_tx_bcast(&mut net, rid, seq);
        net.run(gap)?;
        let n1 = relays_of(&net, m1, 0x4444, seq);
        let m2 = net.cap.len();
        r_tx_bcast(&mut net, rid, seq);
        net.run(3000.0)?;
        let n2 = relays_of(&net, m2, 0x4444, seq);
        lines.push(format!("gap {gap:5.0} ms: first copy relayed {n1}x, second copy relayed {n2}x"));
        net.run(10000.0)?;
    }
    // eviction: other broadcasts between the two copies, 40 ms apart
    for k in [6u8, 10, 11, 12, 13, 16] {
        seq = seq.wrapping_add(20);
        let first = seq;
        let m1 = net.cap.len();
        r_tx_bcast(&mut net, rid, first);
        net.run(100.0)?;
        for j in 0..k {
            r_tx_bcast(&mut net, rid, first.wrapping_add(1 + j));
            net.run(40.0)?;
        }
        let n1 = relays_of(&net, m1, 0x4444, first);
        let m2 = net.cap.len();
        r_tx_bcast(&mut net, rid, first);
        net.run(3000.0)?;
        let n2 = relays_of(&net, m2, 0x4444, first);
        lines.push(format!(
            "{k:2} other broadcasts in between (within {:.2} s): first copy relayed {n1}x, second copy relayed {n2}x",
            (100.0 + 40.0 * k as f64) / 1000.0
        ));
        net.run(10000.0)?;
    }
    // two different broadcasts close together: when is the second one relayed?
    for gap in [2.0, 10.0, 20.0, 40.0, 60.0, 100.0, 200.0, 400.0] {
        seq = seq.wrapping_add(10);
        let (a, b) = (seq, seq.wrapping_add(1));
        let m = net.cap.len();
        let t0 = net.now();
        r_tx_bcast(&mut net, rid, a);
        net.run(gap)?;
        let tb = net.now();
        r_tx_bcast(&mut net, rid, b);
        net.run(15000.0)?;
        let when = |s: u8| -> String {
            let v: Vec<String> = net.cap[m..]
                .iter()
                .filter(|f| f.who == NodeId::Dut && f.nwk.as_ref().is_some_and(|n| n.src == 0x4444 && n.seq == s))
                .map(|f| format!("{:.0} ms", f.t0 - if s == a { t0 } else { tb }))
                .collect();
            if v.is_empty() { "never".into() } else { v.join(", ") }
        };
        lines.push(format!("two broadcasts {gap:3.0} ms apart: the first relayed after {}, the second after {}", when(a), when(b)));
        net.run(5000.0)?;
    }
    // Route Request duplicates (route discovery table, R22 3.6.3.5.2): the same request id again after a gap
    let mut rid_n = 0x30u8;
    for gap in [1000.0, 2000.0, 4000.0, 8000.0] {
        rid_n += 1;
        seq = seq.wrapping_add(1);
        let body = [0x01, 0x00, rid_n, 0x55, 0x55, 0x00];
        let o = Tx::new().radius(30).nwk_seq(seq).mac_dst(0xFFFF);
        let count = |net: &Net, m: usize| {
            net.cap[m..].iter().filter(|f| f.who == NodeId::Dut && f.nwk_cmd() == Some(0x01)
                && f.nwk.as_ref().is_some_and(|n| n.src == 0x4444 && n.payload.as_ref().is_some_and(|p| p[2] == rid_n))).count()
        };
        let m1 = net.cap.len();
        {
            let (st, air) = net.s.w.st(rid);
            st.tx_nwk_cmd(air, 0xFFFC, &body, &o);
        }
        net.run(gap)?;
        let n1 = count(&net, m1);
        let m2 = net.cap.len();
        {
            let (st, air) = net.s.w.st(rid);
            st.tx_nwk_cmd(air, 0xFFFC, &body, &o);
        }
        net.run(3000.0)?;
        let n2 = count(&net, m2);
        lines.push(format!("Route Request, same id again after {gap:5.0} ms: first relayed {n1}x, second relayed {n2}x"));
        net.run(10000.0)?;
    }
    // Device_annce checks: a known neighbour's address announced with another IEEE address, the bulb's own address
    let bs = net.short_of_bulb("B").unwrap();
    let c = &net.s.w.coord.st;
    let mut sx = Station::new(NodeId::Coord, c.channel, c.pan, Some(0x7777), 0x00124B00AA007777, NWK_KEY, (9, 9, 9, 9, 9), 0x900, 0);
    sx.key_seq = c.key_seq;
    let xid = net.s.w.add_peer_node(Peer::bare_ack(sx, "x"));
    net.out_of_range(xid, NodeId::Coord, true);
    let annce = |net: &mut Net, who: NodeId, short: u16, ext: u64| {
        let mut body = short.to_le_bytes().to_vec();
        body.extend_from_slice(&ext_bytes(ext));
        body.push(0x8E);
        let (st, air) = net.s.w.st(who);
        st.tx_zdo(air, 0xFFFD, 0x0013, &body, &Tx::new());
    };
    // r becomes a neighbour with an IEEE address: its own announce and Link Status
    annce(&mut net, rid, 0x4444, 0x00124B00AA004444);
    for _ in 0..3 {
        let (st, air) = net.s.w.st(rid);
        st.tx_link_status(air, &[(bs, 1, 1)], &Tx::new());
        net.run(5000.0)?;
    }
    let lqi = net.lqi_text(bs)?;
    lines.push(format!("neighbour table with r: {lqi}"));
    for (what, short, ext) in [
        ("Device_annce from x: r's address 0x4444 with x's IEEE", 0x4444u16, 0x00124B00AA007777u64),
        ("Device_annce from x: the bulb's own address with x's IEEE", bs, 0x00124B00AA007777),
    ] {
        let m = net.cap.len();
        annce(&mut net, xid, short, ext);
        net.run(6000.0)?;
        let st: Vec<String> = net.cap[m..]
            .iter()
            .filter(|f| f.who == NodeId::Dut && (f.nwk_cmd() == Some(0x03) || f.zdo().is_some_and(|z| z.cluster == 0x0013) || f.nwk_cmd() == Some(0x07)))
            .map(|f| f.brief())
            .collect();
        lines.push(format!("{what}: the bulb sends [{}]", st.join(" | ")));
    }
    // as in the house: the coordinator's Mgmt_Permit_Joining_req (0xFFFC) and a router's
    // Device_annce (0xFFFD) at the same moment, neighbours relaying the announce
    for (k, relays, lead) in [(0usize, 0usize, 0.0), (1, 2, 0.0), (2, 4, 0.0), (3, 4, 10.0), (4, 4, 20.0), (5, 4, 5.0), (6, 2, 15.0)] {
        net.run(12000.0)?;
        let m = net.cap.len();
        let t0 = net.now();
        annce(&mut net, rid, 0x4444, 0x00124B00AA004444);
        let aseq = net.s.w.peers[peer_ix(rid)].st.nwk_seq.wrapping_sub(1);
        if lead > 0.0 {
            net.run(lead)?;
        }
        net.s.w.coord.permit_joining(&mut net.s.w.air, 0);
        net.run(20.0)?;
        for j in 0..relays {
            let mut body = 0x4444u16.to_le_bytes().to_vec();
            body.extend_from_slice(&ext_bytes(0x00124B00AA004444));
            body.push(0x8E);
            let o = Tx::new().src(0x4444).nwk_seq(aseq).radius(28 - (j as u8 % 2)).src_ieee(SrcIeee::Is(0x00124B00AA004444)).seq(0x41);
            let (st, air) = net.s.w.st(xid);
            st.tx_zdo(air, 0xFFFD, 0x0013, &body, &o);
            net.run(15.0)?;
        }
        net.run(15000.0)?;
        let v: Vec<String> = net.cap[m..]
            .iter()
            .filter(|f| f.who == NodeId::Dut && f.nwk.as_ref().is_some_and(|n| n.dst >= 0xFFF8 && n.radius > 1))
            .map(|f| {
                let n = f.nwk.as_ref().unwrap();
                format!("{:.0} ms {:#06x}/{} r={}", f.t0 - t0, n.src, n.seq, n.radius)
            })
            .collect();
        lines.push(format!(
            "Device_annce, {lead:.0} ms later the permit-join broadcast, then {relays} relayed copies of the announce (case {k}): the bulb relays [{}]",
            v.join(", ")
        ));
    }
    for l in lines {
        cx.say(&format!("   {l}"));
    }
    Ok(())
}


// =================================================================== broadcast echoes
/// Copies of the broadcast (src, seq) on the air since `mark`: count per sender, the last one, lowest radius.
fn echo_report(net: &Net, mark: usize, src: u16, seq: u8, t0: f64) -> (usize, f64, String) {
    let mut per: Vec<(String, usize)> = Vec::new();
    let (mut n, mut last, mut rmin) = (0usize, 0.0f64, 255u8);
    for f in &net.cap[mark..] {
        let Some(nw) = &f.nwk else { continue };
        if nw.src != src || nw.seq != seq || nw.dst < 0xFFF8 {
            continue;
        }
        n += 1;
        last = last.max(f.t0 - t0);
        rmin = rmin.min(nw.radius);
        match per.iter_mut().find(|p| p.0 == f.who_name) {
            Some(p) => p.1 += 1,
            None => per.push((f.who_name.clone(), 1)),
        }
    }
    let txt = per.iter().map(|(w, k)| format!("{w} {k}")).collect::<Vec<_>>().join(", ");
    (n, last, format!("{n} copies over {:.1} s, lowest radius {rmin} ({txt})", last / 1000.0))
}

/// The last broadcast the Ember router sent: (seq, ms).
fn ember_last_bcast(net: &Net, mark: usize) -> Option<(u16, u8, f64)> {
    let id = net.ember.as_ref()?.id;
    net.cap[mark..]
        .iter()
        .rev()
        .find(|f| f.who == id && f.nwk.as_ref().is_some_and(|n| n.dst >= 0xFFF8 && n.radius > 1))
        .map(|f| (f.nwk.as_ref().unwrap().src, f.nwk.as_ref().unwrap().seq, f.t0))
}

fn run_echo(cx: &mut SimCtx) -> R<()> {
    let img = cx.image("bulb", "stock/stock-dimm.bin");
    let echo = cx.verbose > 0;
    cx.say(&format!("image: {}", super::util::rel(&img)));
    let names = ["K1", "K2", "K3", "W1", "W2"];
    let mut nodes = Vec::new();
    for (i, nm) in names.iter().enumerate() {
        nodes.push(bulb_node(&img, cx.cpi, nm, 0xa4c1380000000021 + i as u64)?);
    }
    let mut it = nodes.into_iter();
    let mut net = Net::new(it.next().unwrap(), Coord::new_plain(11, NWK_KEY), echo);
    for (k, n) in it.enumerate() {
        net.add_bulb(n, 700.0 * (k + 1) as f64);
    }
    let ok = net.run_until(60000.0, |n| n.bulbs.iter().all(|b| n.short_of_bulb(&b.name).is_some()))?;
    cx.check(net.now(), "bulbs join", ok, "");
    let eid = net.add_ember(0x1C3B, 0x5EED_0001);
    net.s.w.coord.permit_joining(&mut net.s.w.air, 254);
    net.run(1000.0)?;
    let js = net.ember_join_via(NodeId::Dut)?;
    cx.check(net.now(), "ikea joins through K1", js.is_some(), &format!("{js:?}"));
    net.ember.as_mut().unwrap().neighbours = net.bulbs.iter().filter_map(|b| net.short_of_bulb(&b.name)).collect();
    net.ember.as_mut().unwrap().neighbours.push(0x0000);
    net.ember.as_mut().unwrap().t_up = net.now();
    net.ember_start();
    net.run(40000.0)?;
    let phases: Vec<(&str, Option<&str>)> = vec![("all bulbs on", None), ("K2 off (still in the others' neighbour tables)", Some("K2"))];
    for (what, off) in phases {
        if let Some(b) = off {
            net.power_off(b);
            net.run(10000.0)?;
        }
        for k in 0..3 {
            let mark = net.cap.len();
            let moves0 = net.ember.as_ref().unwrap().moves.len();
            net.ember_annce();
            net.run(300.0)?;
            let Some((src, seq, t0)) = ember_last_bcast(&net, mark) else { continue };
            net.run(40000.0)?;
            let (_, _, txt) = echo_report(&net, mark, src, seq, t0);
            let e = net.ember.as_ref().unwrap();
            let late: Vec<String> = e
                .echoes
                .iter()
                .filter(|x| x.0 >= t0 && x.1 == seq)
                .filter(|x| !x.4)
                .map(|x| format!("{:.1} s r={} from {}", (x.0 - t0) / 1000.0, x.2, x.3))
                .collect();
            let moved = e.moves.len() - moves0;
            cx.say(&format!(
                "   {what}, Device_annce #{k} from ikea {src:#06x} seq {seq}: {txt}; copies new to the ikea's table: [{}]; address changes: {moved}",
                late.join(", ")
            ));
        }
    }
    for n in net.notes.clone() {
        if n.text.contains("CONFLICT") || n.text.contains("power") {
            cx.say(&format!("[{:9.3} s] {}", n.ms / 1000.0, n.text));
        }
    }
    let _ = eid;
    Ok(())
}


// =================================================================== a conflict on a router child's address
/// The Ember router joined through K1 (K1's child). A conflict is reported on its
/// address (Network Status 0x0d from the coordinator, as an Ember coordinator
/// broadcasts it) while K1 is on: what do K1 and the others do?
///
/// The checks are what Outglow does (default image: the Rust firmware): K1
/// learns the router's new address from its Device_annce and Link Status,
/// lists it at the new address only, and every command through K1 reaches it
/// (4 x 3). The stock firmware keeps the old address and loses every one of
/// them (`--image bulb=stock/stock-dimm.bin`: those checks fail, as measured
/// in the lab's docs/address-conflicts.md).
fn run_child(cx: &mut SimCtx) -> R<()> {
    let img = cx.image("bulb", "target/tc32/bulb-tc32.bin");
    let echo = cx.verbose > 0;
    cx.say(&format!("image: {}", super::util::rel(&img)));
    let mut it = vec![
        bulb_node(&img, cx.cpi, "K1", 0xa4c1380000000031)?,
        bulb_node(&img, cx.cpi, "K3", 0xa4c1380000000033)?,
        bulb_node(&img, cx.cpi, "W2", 0xa4c1380000000035)?,
    ]
    .into_iter();
    let mut net = Net::new(it.next().unwrap(), Coord::new_plain(11, NWK_KEY), echo);
    for (k, n) in it.enumerate() {
        net.add_bulb(n, 700.0 * (k + 1) as f64);
    }
    let ok = net.run_until(60000.0, |n| n.bulbs.iter().all(|b| n.short_of_bulb(&b.name).is_some()))?;
    cx.check(net.now(), "bulbs join", ok, "");
    let eid = net.add_ember(0x1C3B, 0x5EED_0002);
    let via_k1 = cx.images.iter().all(|i| i.label != "direct");
    if via_k1 {
        net.out_of_range(eid, NodeId::Coord, true);
    }
    net.s.w.coord.permit_joining(&mut net.s.w.air, 254);
    net.run(1000.0)?;
    let js = net.ember_join_via(NodeId::Dut)?;
    cx.check(net.now(), "ikea joins through K1", js.is_some(), &format!("{js:?} (via K1: {via_k1})"));
    net.ember.as_mut().unwrap().neighbours = net.bulbs.iter().filter_map(|b| net.short_of_bulb(&b.name)).collect();
    net.ember.as_mut().unwrap().neighbours.push(0x0000);
    net.ember.as_mut().unwrap().t_up = net.now();
    net.ember_start();
    let k1s = net.short_of_bulb("K1").unwrap();
    if via_k1 {
        net.s.w.peers[peer_ix(eid)].st.via.insert(0x0000, k1s);
    }
    net.run(35000.0)?;
    let t = net.lqi_text(k1s)?;
    cx.say(&format!("-- K1's neighbour table before: {t}"));
    let old = net.ember_short();
    let mark = net.cap.len();
    cx.say(&format!("-- the coordinator broadcasts Network Status 0x0d for the ikea's address {old:#06x}"));
    {
        let c = &mut net.s.w.coord;
        c.st.tx_nwk_cmd(&mut net.s.w.air, 0xFFFD, &[0x03, 0x0D, old as u8, (old >> 8) as u8], &Tx::new().radius(30).mac_dst(0xFFFF));
    }
    net.run(60000.0)?;
    show_from(cx, &net, mark, None, true);
    let t = net.lqi_text(k1s)?;
    cx.say(&format!("-- K1's neighbour table after: {t}"));
    // traffic to the ikea at its new address through K1
    let new = net.ember_short();
    let t_move = net.ember.as_ref().unwrap().moves.last().map(|m| m.0).unwrap_or(0.0);
    let mut rows = Vec::new();
    let (mut reached, mut sent, mut listed_new) = (0, 0, true);
    for (k, at) in [70.0, 180.0, 300.0, 600.0].iter().enumerate() {
        let wait = t_move + at * 1000.0 - net.now();
        if wait > 0.0 {
            net.run(wait)?;
        }
        let n0 = net.ember.as_ref().unwrap().zcl_answers;
        let mark = net.cap.len();
        for _ in 0..3 {
            net.coord_cmd(new, Some(k1s));
            net.run(3000.0)?;
        }
        let got = net.ember.as_ref().unwrap().zcl_answers - n0;
        reached += got.min(3);
        sent += 3;
        if k == 0 {
            cx.say(&format!("-- On/Off to the ikea at {new:#06x} through K1, {at:.0} s after it moved"));
            show_from(cx, &net, mark, None, true);
        }
        let t = net.lqi_text(k1s)?;
        let entry = t.split("; ").filter(|e| e.contains("180df9fffef90302")).collect::<Vec<_>>().join("; ");
        // the ikea's entries in K1's table: its new address, and only that
        let ikea_shorts: Vec<Option<u16>> = t
            .split("; ")
            .filter(|e| e.contains("180df9fffef90302"))
            .map(|e| e.split(' ').next().and_then(|x| u16::from_str_radix(x.trim_start_matches("0x"), 16).ok()))
            .collect();
        listed_new &= !ikea_shorts.is_empty() && ikea_shorts.iter().all(|s| *s == Some(new));
        rows.push(format!("+{at:.0} s: {got} of 3 reached the ikea; K1 lists it as [{entry}]"));
    }
    for n in net.notes.clone() {
        cx.say(&format!("[{:9.3} s] {}", n.ms / 1000.0, n.text));
    }
    let _ = eid;
    let moves = net.ember.as_ref().unwrap().moves.len();
    cx.check(net.now(), "the ikea took a new address once (the conflict report)", moves == 1 && new != old,
             &format!("address changes {moves}: {old:#06x} -> {new:#06x}"));
    cx.check(net.now(), "after the ikea moved: K1 lists it at its new address only (70 s, 3, 5 and 10 min later)", listed_new,
             &rows.join(" | "));
    cx.check(net.now(), "after the ikea moved: every command through K1 reaches it", reached == sent,
             &format!("{reached} of {sent}: {}", rows.join(" | ")));
    // pairs stated after the conflict report (before it, a pair of a bulb not yet
    // announced counts as unknown: the ground truth learns it from its frames)
    let bad: Vec<String> = net.bad.iter().filter(|b| b.0 >= mark).map(|b| format!("{:#06x} = {:#018x} ({})", b.1.short, b.1.ext, b.2)).collect();
    cx.check(net.now(), "no frame after the conflict report pairs an address with another device's IEEE address", bad.is_empty(),
             &format!("bad pairs since the report {} [{}], before it {}", bad.len(), bad.join("; "), net.bad.len() - bad.len()));
    Ok(())
}


// =================================================================== multi-hop
/// The coordinator reaches the Ember router only through K1 and K3 (K1 does not
/// hear the router); the router changes its address with everything on, then
/// K1 is power-cycled. Route discovery, relays and every address pair.
fn run_multihop(cx: &mut SimCtx) -> R<()> {
    let img = cx.image("bulb", "stock/stock-dimm.bin");
    let echo = cx.verbose > 0;
    cx.say(&format!("image: {}", super::util::rel(&img)));
    let mut it = vec![
        bulb_node(&img, cx.cpi, "K1", 0xa4c1380000000041)?,
        bulb_node(&img, cx.cpi, "K3", 0xa4c1380000000043)?,
        bulb_node(&img, cx.cpi, "W2", 0xa4c1380000000045)?,
    ]
    .into_iter();
    let mut net = Net::new(it.next().unwrap(), Coord::new_plain(11, NWK_KEY), echo);
    let mut ids = vec![NodeId::Dut];
    for (k, n) in it.enumerate() {
        ids.push(net.add_bulb(n, 700.0 * (k + 1) as f64));
    }
    let (k1, k3, w2) = (ids[0], ids[1], ids[2]);
    let ok = net.run_until(60000.0, |n| n.bulbs.iter().all(|b| n.short_of_bulb(&b.name).is_some()))?;
    cx.check(net.now(), "bulbs join", ok, "");
    let eid = net.add_ember(0x1C3B, 0x5EED_0003);
    // ikea hears K3 only; the coordinator hears K1 and W2; W2 hears K1 and the coordinator
    for (a, b) in [(eid, NodeId::Coord), (eid, k1), (eid, w2), (k3, NodeId::Coord), (k3, w2)] {
        net.out_of_range(a, b, true);
    }
    net.s.w.coord.permit_joining(&mut net.s.w.air, 254);
    net.run(1000.0)?;
    let js = net.ember_join_via(k3)?;
    cx.check(net.now(), "ikea joins through K3", js.is_some(), &format!("{js:?}"));
    let k3s = net.short_of_bulb("K3").unwrap();
    let k1s = net.short_of_bulb("K1").unwrap();
    net.ember.as_mut().unwrap().neighbours = vec![k3s];
    net.ember.as_mut().unwrap().t_up = net.now();
    net.s.w.peers[peer_ix(eid)].st.via.insert(0x0000, k3s);
    net.ember_start();
    net.run(40000.0)?;
    let steps: Vec<&str> = vec!["first contact", "after the ikea changed its address", "after K1 was power-cycled"];
    for (k, what) in steps.iter().enumerate() {
        if k == 1 {
            net.ember_conflict("forced, everything on".into(), false);
            net.run(30000.0)?;
        }
        if k == 2 {
            net.power_off("K1");
            net.run(5000.0)?;
            net.power_on("K1")?;
            net.run(30000.0)?;
        }
        let mark = net.cap.len();
        let target = net.ember_short();
        let n0 = net.ember.as_ref().unwrap().zcl_answers;
        for _ in 0..3 {
            net.coord_cmd(target, Some(k1s));
            net.run(4000.0)?;
        }
        let got = net.ember.as_ref().unwrap().zcl_answers - n0;
        cx.say(&format!("-- {what}: On/Off x3 to the ikea {target:#06x} through K1: {got} reached it"));
        show_from(cx, &net, mark, None, true);
        for b in ["K1", "K3"] {
            let s = net.short_of_bulb(b).unwrap();
            let t = net.lqi_text(s)?;
            cx.say(&format!("   {b} neighbours: {t}"));
        }
    }
    for n in net.notes.clone() {
        cx.say(&format!("[{:9.3} s] {}", n.ms / 1000.0, n.text));
    }
    let e = net.ember.as_ref().unwrap();
    cx.check(net.now(), "observed", true, &format!("ikea address changes {}, route replies {}, bad pairs {}", e.moves.len(), e.rreps, net.bad.len()));
    let _ = w2;
    Ok(())
}


// =================================================================== the house
/// Something like a real home network: three dimm bulbs on a wall switch (K1..K3),
/// six TW bulbs (S1..S6), the coordinator, the Ember router (the IKEA driver) next
/// to the corridor, not every node in range of every other; the corridor is
/// switched off and on, the router changes its address once while it is off,
/// and the coordinator talks to everyone as Z2M does (commands, a many-to-one
/// route request every minute, an address request now and then).
fn run_house(cx: &mut SimCtx) -> R<()> {
    let img_d = cx.image("bulb", "stock/stock-dimm.bin");
    let img_t = cx.image("tw", "stock/stock-tw.bin");
    let echo = cx.verbose > 0;
    let minutes: f64 = std::env::var("ADDR_CONFLICT_MINUTES").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0);
    cx.say(&format!("images: {} (K), {} (S); {minutes} min", super::util::rel(&img_d), super::util::rel(&img_t)));
    let names = ["K1", "K2", "K3", "S1", "S2", "S3", "S4", "S5", "S6"];
    let mut nodes = Vec::new();
    for (i, nm) in names.iter().enumerate() {
        let img = if nm.starts_with('K') { &img_d } else { &img_t };
        nodes.push(bulb_node(img, cx.cpi, nm, 0xa4c1380000000051 + i as u64)?);
    }
    let mut it = nodes.into_iter();
    let mut net = Net::new(it.next().unwrap(), Coord::new_plain(11, NWK_KEY), echo);
    let mut ids = vec![NodeId::Dut];
    for (k, n) in it.enumerate() {
        ids.push(net.add_bulb(n, 500.0 * (k + 1) as f64));
    }
    let id = |nm: &str| ids[names.iter().position(|n| *n == nm).unwrap()];
    let ok = net.run_until(90000.0, |n| n.bulbs.iter().all(|b| n.short_of_bulb(&b.name).is_some()))?;
    cx.check(net.now(), "bulbs join", ok, "");
    net.run(5000.0)?;
    let eid = net.add_ember(0x1C3B, 0x5EED_0004);
    // the corridor (K*, ikea) is out of the coordinator's range; S5, S6 too
    let far = ["K1", "K2", "K3", "S5", "S6"];
    for f in far {
        net.out_of_range(id(f), NodeId::Coord, true);
    }
    net.out_of_range(eid, NodeId::Coord, true);
    for s in ["S1", "S2", "S3", "S4", "S6"] {
        net.out_of_range(eid, id(s), true);
    }
    for k in ["K1", "K2", "K3"] {
        for s in ["S1", "S2", "S6"] {
            net.out_of_range(id(k), id(s), true);
        }
    }
    net.run(60000.0)?; // the far bulbs find their way (they may rejoin through others)
    net.s.w.coord.permit_joining(&mut net.s.w.air, 254);
    net.run(1000.0)?;
    let js = net.ember_join_via(id("K1"))?;
    cx.check(net.now(), "ikea joins through K1", js.is_some(), &format!("{js:?}"));
    if std::env::var("ADDR_CONFLICT_KEEP_OPEN").is_err() {
        net.s.w.coord.permit_joining(&mut net.s.w.air, 0);
    }
    let nb: Vec<u16> = ["K1", "K2", "K3", "S5"].iter().filter_map(|b| net.short_of_bulb(b)).collect();
    net.ember.as_mut().unwrap().neighbours = nb;
    net.ember.as_mut().unwrap().t_up = net.now();
    net.ember_start();
    let k1s = net.short_of_bulb("K1").unwrap();
    net.s.w.peers[peer_ix(eid)].st.via.insert(0x0000, k1s);
    // stress: a broadcast of the ikea and one of the coordinator at nearly the same moment, many times
    let rounds: usize = std::env::var("ADDR_CONFLICT_ROUNDS").ok().and_then(|v| v.parse().ok()).unwrap_or(24);
    let (mut late1, mut late9, mut lost_relays, mut relays_total) = (0usize, 0usize, 0usize, 0usize);
    let moves0 = net.ember.as_ref().unwrap().moves.len();
    let mut worst = 0.0f64;
    for r in 0..rounds {
        net.run(2000.0)?;
        let me = net.ember_short();
        let m = net.cap.len();
        let t0 = net.now();
        let seq = {
            let pi = peer_ix(eid);
            let World2 { st, air } = split(&mut net.s, pi);
            st.tx_zcl(air, 0xFFFD, 0x0003, 0x01, &[], &Tx::new().ddr().src_ieee(SrcIeee::None)).nwk_seq
        };
        let d = [0.0, 3.0, 6.0, 10.0, 15.0][r % 5];
        if d > 0.0 {
            net.run(d)?;
        }
        if r % 2 == 0 {
            net.s.w.coord.permit_joining(&mut net.s.w.air, 0);
        } else {
            let mut p = ext_bytes(IKEA_EXT).to_vec();
            p.extend_from_slice(&[0, 0]);
            net.coord_zdo(0xFFFD, 0x0000, &p);
        }
        net.run(14000.0)?;
        let copies: Vec<(f64, NodeId)> = net.cap[m..]
            .iter()
            .filter(|f| f.nwk.as_ref().is_some_and(|n| n.src == me && n.seq == seq && n.dst >= 0xFFF8))
            .map(|f| (f.t0 - t0, f.who))
            .collect();
        let last = copies.iter().map(|c| c.0).fold(0.0, f64::max);
        worst = worst.max(last);
        if last > 1000.0 {
            late1 += 1;
        }
        if last > 9000.0 {
            late9 += 1;
        }
        // which powered bulbs that hear a relayer never relayed it
        for b in &net.bulbs {
            if b.off.is_some() {
                continue;
            }
            relays_total += 1;
            if !copies.iter().any(|c| c.1 == b.id) {
                lost_relays += 1;
            }
        }
    }
    let moves = net.ember.as_ref().unwrap().moves.len() - moves0;
    cx.check(
        net.now(),
        "stress: ikea broadcast + coordinator broadcast together",
        true,
        &format!(
            "{rounds} rounds: the ikea's broadcast still on the air after 1 s in {late1}, after 9 s in {late9} (latest copy {:.1} s); \
             bulbs that never relayed it: {lost_relays} of {relays_total}; ikea address changes {moves}",
            worst / 1000.0
        ),
    );
    let t_start = net.now();
    let end = t_start + minutes * 60000.0;
    let (mut next_cmd, mut next_mto, mut next_addr) = (t_start + 3000.0, t_start + 10000.0, t_start + 60000.0);
    let mut k = 0usize;
    let corridor = ["K1", "K2", "K3"];
    let mut switch_at = vec![(t_start + 120000.0, false), (t_start + 300000.0, true), (t_start + 420000.0, false), (t_start + 540000.0, true)];
    let mut moved = false;
    while net.now() < end {
        let now = net.now();
        if let Some(&(t, on)) = switch_at.first() {
            if now >= t {
                switch_at.remove(0);
                for c in corridor {
                    if on {
                        net.power_on(c)?;
                    } else {
                        net.power_off(c);
                    }
                }
            }
        }
        if !moved && now >= t_start + 200000.0 {
            moved = true;
            net.ember_move("while the corridor is off");
        }
        if now >= next_cmd {
            next_cmd = now + 5000.0;
            // the coordinator's routing: the corridor and S5, S6 through the bulbs it hears
            let targets: Vec<u16> = net.bulbs.iter().filter_map(|b| net.short_of_bulb(&b.name)).collect();
            let t = if k % 3 == 0 { net.ember_short() } else { targets[k % targets.len()] };
            k += 1;
            net.coord_cmd(t, None);
        }
        if now >= next_mto {
            next_mto = now + 60000.0;
            net.s.w.coord.many_to_one_route_request(&mut net.s.w.air, false);
        }
        if now >= next_addr {
            next_addr = now + 120000.0;
            let mut p = ext_bytes(IKEA_EXT).to_vec();
            p.extend_from_slice(&[0, 0]);
            net.coord_zdo(0xFFFD, 0x0000, &p);
        }
        net.run(500.0)?;
    }
    // what each bulb believes about the ikea and the corridor
    let ikea = net.ember_short();
    for b in names {
        if let Some(s) = net.short_of_bulb(b) {
            if net.bulb(b).off.is_none() {
                let t = net.lqi_text(s)?;
                cx.say(&format!("   {b} ({s:#06x}) neighbours: {t}"));
            }
        }
    }
    let mut kinds: Vec<(String, usize)> = Vec::new();
    for f in &net.cap {
        if let Some(c) = f.nwk_cmd() {
            if c == 0x03 || c == 0x07 || c == 0x04 {
                let k = format!("{} {}", f.who_name, crate::session::nwk_cmd_name(c).unwrap_or("?"));
                match kinds.iter_mut().find(|x| x.0 == k) {
                    Some(x) => x.1 += 1,
                    None => kinds.push((k, 1)),
                }
            }
        }
    }
    cx.say(&format!("   Network Status / Rejoin Response / Leave frames: {kinds:?}"));
    // the broadcasts that came back to the ikea after its record expired: every copy of them
    let e = net.ember.as_ref().unwrap();
    let mut seen = Vec::new();
    for m in &e.moves {
        if let Some(seq) = m.3.split("(seq ").nth(1).and_then(|r| r.split(',').next()).and_then(|v| v.parse::<u8>().ok()) {
            seen.push((m.1, seq));
        }
    }
    for (src, seq) in seen {
        let copies: Vec<&CapFrame> = net.cap.iter().filter(|f| f.nwk.as_ref().is_some_and(|n| n.src == src && n.seq == seq && n.dst >= 0xFFF8)).collect();
        let t0 = copies.first().map(|f| f.t0).unwrap_or(0.0);
        cx.say(&format!("   every copy of the broadcast {src:#06x} seq {seq} ({} copies):", copies.len()));
        let mut late: Option<(NodeId, f64)> = None;
        let mut prev = t0;
        for f in &copies {
            let n = f.nwk.as_ref().unwrap();
            let m = f.mac.as_ref().unwrap();
            cx.say(&format!("     +{:7.3} s {:>4} radius {:2} mac {:#06x}->{:#06x}", (f.t0 - t0) / 1000.0, f.who_name, n.radius,
                            m.src.unwrap_or(0), m.dst.unwrap_or(0)));
            if late.is_none() && f.t0 - prev > 1000.0 {
                late = Some((f.who, f.t0));
            }
            prev = f.t0;
        }
        // what the late sender did in between
        if let Some((who, tl)) = late {
            cx.say("   the late sender's frames in between:");
            for f in net.cap.iter().filter(|f| f.who == who && f.t0 > t0 && f.t0 <= tl) {
                cx.say(&format!("     +{:7.3} s {}", (f.t0 - t0) / 1000.0, f.brief()));
            }
        }
    }
    for n in net.notes.clone() {
        if !n.text.starts_with("truth: ") || n.text.contains("->") {
            cx.say(&format!("[{:9.3} s] {}", n.ms / 1000.0, n.text));
        }
    }
    let e = net.ember.as_ref().unwrap();
    let moves: Vec<String> = e.moves.iter().map(|m| format!("{:.1} s {:#06x}->{:#06x} ({})", m.0 / 1000.0, m.1, m.2, m.3)).collect();
    cx.check(net.now(), "observed", true, &format!("ikea now {ikea:#06x}; moves: [{}]; bad pairs {}", moves.join("; "), net.bad.len()));
    Ok(())
}


// =================================================================== the bulb as the victim
/// A late copy of the bulb's own broadcast, as a stock bulb relays it many
/// seconds later: another router sends a broadcast whose NWK source is the
/// bulb's address and whose sequence number the bulb really used, `d` seconds
/// after the bulb used it, with or without the source IEEE field (the bulb's
/// own IEEE address). Does the bulb take it for an address conflict (R22
/// 3.6.1.9.2: a new broadcast record with its own address as source)?
///
/// Outglow (the default image) must not: the copy carries its own IEEE address
/// or one of its recent sequence numbers (`own_late_copy` in the zigbee
/// crate), so no copy may move it or make it report a conflict. The stock
/// firmware does not apply the rule at all and passes too.
fn run_victim(cx: &mut SimCtx) -> R<()> {
    let img = cx.image("bulb", "target/tc32/bulb-tc32.bin");
    cx.say(&format!("image: {}", super::util::rel(&img)));
    let b = bulb_node(&img, cx.cpi, "B", 0xa4c1380000000071)?;
    let mut net = Net::new(b, Coord::new_plain(11, NWK_KEY), cx.verbose > 0);
    let ok = net.run_until(60000.0, |n| n.short_of_bulb("B").is_some())?;
    cx.check(net.now(), "bulb joins", ok, "");
    net.run(20000.0)?;
    let c = &net.s.w.coord.st;
    let mut st = Station::new(NodeId::Coord, c.channel, c.pan, Some(0x4444), 0x00124B00AA004444, NWK_KEY, (1, 2, 3, 4, 5), 0x500, 0);
    st.key_seq = c.key_seq;
    let rid = net.s.w.add_peer_node(Peer::bare_ack(st, "r"));
    net.s.w.air.keys.learn(Some(0x4444), Some(0x00124B00AA004444));
    let ext = net.bulb("B").ext.unwrap();
    let (mut changes, mut reactions, mut sent, mut cases) = (0usize, 0usize, 0usize, 0usize);
    let mut untested = Vec::new();
    for with_ieee in [false, true] {
        for d in [2.0, 5.0, 10.0, 20.0, 60.0] {
            cases += 1;
            // a fresh broadcast of the bulb, the way it happens in a conflict: the coordinator reports
            // a conflict on the bulb's address, the bulb (if it reacts) takes a new one and announces it
            net.run(12000.0)?;
            let me0 = net.short_of_bulb("B").unwrap();
            let mark0 = net.cap.len();
            {
                let c = &mut net.s.w.coord;
                c.st.tx_nwk_cmd(&mut net.s.w.air, 0xFFFD, &[0x03, 0x0D, me0 as u8, (me0 >> 8) as u8], &Tx::new().radius(30).mac_dst(0xFFFF));
            }
            // the last broadcast (radius > 1) the bulb originated: the frame a stock bulb would relay late
            let own_bcast = |f: &CapFrame, me: u16| {
                f.who == NodeId::Dut
                    && f.nwk.as_ref().is_some_and(|n| n.src == me && n.dst >= 0xFFF8 && n.radius > 1)
                    && f.mac.as_ref().is_some_and(|m| m.src == Some(me as u64))
            };
            // up to 3 s for the bulb's answer to the report (the 2 s copy must not start later than that)
            net.run_until(3000.0, |n| {
                let me = n.short_of_bulb("B").unwrap();
                n.cap[mark0..].iter().any(|f| own_bcast(f, me))
            })?;
            net.run(100.0)?;
            let me = net.short_of_bulb("B").unwrap();
            let last = net.cap.iter().rev().find(|f| own_bcast(f, me)).map(|f| (f.nwk.as_ref().unwrap().seq, f.t0));
            let Some((seq, t_used)) = last else {
                cx.say(&format!("   no broadcast of the bulb under {me:#06x} to copy"));
                untested.push(format!("{d:.0} s"));
                continue;
            };
            let wait = t_used + d * 1000.0 - net.now();
            if wait <= 0.0 {
                cx.say(&format!("   the bulb's last broadcast (seq {seq}) is {:.1} s old: no {d:.0} s copy of it", (net.now() - t_used) / 1000.0));
                untested.push(format!("{d:.0} s"));
                continue;
            }
            net.run(wait)?;
            sent += 1;
            let mark = net.cap.len();
            let mut body = me.to_le_bytes().to_vec();
            body.extend_from_slice(&ext_bytes(ext));
            body.push(0x8E);
            let o = Tx::new()
                .src(me)
                .nwk_seq(seq)
                .radius(28)
                .seq(0x30)
                .src_ieee(if with_ieee { SrcIeee::Is(ext) } else { SrcIeee::None });
            {
                let (st, air) = net.s.w.st(rid);
                st.tx_zdo(air, 0xFFFD, 0x0013, &body, &o);
            }
            net.run(5000.0)?;
            let reacted: Vec<String> = net.cap[mark..]
                .iter()
                .filter(|f| f.who == NodeId::Dut)
                .filter(|f| {
                    (f.nwk_cmd() == Some(0x03) && f.nwk.as_ref().unwrap().payload.as_ref().is_some_and(|p| p.get(1) == Some(&0x0D)))
                        || f.zdo().is_some_and(|z| z.cluster == 0x0013 && f.nwk.as_ref().unwrap().src != me)
                })
                .map(|f| f.brief())
                .collect();
            let now_short = net.short_of_bulb("B").unwrap();
            if now_short != me {
                changes += 1;
            }
            if !reacted.is_empty() {
                reactions += 1;
            }
            cx.say(&format!(
                "   copy of its broadcast seq {seq} {:.1} s after it was sent, source IEEE field {}: {}",
                (net.cap[mark].t0 - t_used) / 1000.0,
                if with_ieee { "its own" } else { "absent" },
                if now_short != me {
                    format!("ADDRESS CHANGED {me:#06x} -> {now_short:#06x} [{}]", reacted.join(" | "))
                } else if reacted.is_empty() {
                    "no reaction".to_string()
                } else {
                    format!("[{}]", reacted.join(" | "))
                }
            ));
        }
    }
    cx.check(net.now(), "a copy of every delay (2 to 60 s, with and without its IEEE address) reached the bulb", sent == cases,
             &format!("{sent} of {cases}{}", if untested.is_empty() { String::new() } else { format!("; not sent: {}", untested.join(", ")) }));
    cx.check(net.now(), "no late copy of its own broadcast is taken for an address conflict (no new address, no Network Status 0x0d)",
             changes == 0 && reactions == 0, &format!("address changes {changes}, copies answered with a conflict report or a new address {reactions}, of {sent}"));
    Ok(())
}


// =================================================================== a channel change while a bulb is off
/// The coordinator moves the network from channel 11 to 25 (Mgmt_NWK_Update_req
/// broadcast, then it changes its own channel) while bulb B is off; bulb A is on
/// and follows. B comes back on channel 11 with its old network in flash:
/// does it find the network, how (rejoin or a new association), and with
/// which address? Once with the network closed to new devices, once open.
///
/// The checks are what Outglow does (default image: the Rust firmware): A
/// follows the change; B, hearing nothing on channel 11, scans the channels,
/// finds the network on 25, rejoins it secured (no new association) and
/// announces itself there with the address it had, within the wait
/// (`CH_WAIT_S`, default 240 s; measured 183 s). The stock firmware stays
/// silent on channel 11 (`--image bulb=stock/stock-dimm.bin`: B's checks fail).
fn run_channel(cx: &mut SimCtx) -> R<()> {
    let img = cx.image("bulb", "target/tc32/bulb-tc32.bin");
    cx.say(&format!("image: {}", super::util::rel(&img)));
    for open in [false, true] {
        let a = bulb_node(&img, cx.cpi, "A", 0xa4c1380000000081)?;
        let b = bulb_node(&img, cx.cpi, "B", 0xa4c1380000000082)?;
        let mut net = Net::new(a, Coord::new_plain(11, NWK_KEY), cx.verbose > 0);
        let bid = net.add_bulb(b, 700.0);
        let ok = net.run_until(60000.0, |n| n.bulbs.iter().all(|b| n.short_of_bulb(&b.name).is_some()))?;
        cx.check(net.now(), "bulbs join", ok, "");
        net.run(20000.0)?;
        let (a0, b0) = (net.short_of_bulb("A").unwrap(), net.short_of_bulb("B").unwrap());
        // reporting as Z2M configures it: On/Off attribute, at least every 60 s
        net.s.w.coord.zcl_send_with(&mut net.s.w.air, b0, 1, 0x0006, 0x06, &[0x00, 0x00, 0x00, 0x10, 0x01, 0x00, 0x3C, 0x00],
                                    crate::coord::HA, false, false, None, false);
        net.run(3000.0)?;
        let reports0 = net.cap.iter().filter(|f| f.who == bid && f.zcl().is_some_and(|z| z.cmd == 0x0A && !z.specific)).count();
        cx.say(&format!("   reports from B before the change: {reports0}"));
        net.s.w.coord.permit_joining(&mut net.s.w.air, 0);
        net.run(1000.0)?;
        net.power_off("B");
        net.run(3000.0)?;
        // Mgmt_NWK_Update_req: channel mask with channel 25 only, ScanDuration 0xFE (change channel), update id 1
        let mask: u32 = 1 << 25;
        let mut p = mask.to_le_bytes().to_vec();
        p.push(0xFE);
        p.push(0x01);
        net.coord_zdo(0xFFFD, 0x0038, &p);
        net.run(9000.0)?;
        net.s.w.coord.st.channel = 25;
        let mark_a = net.cap.len();
        net.run(20000.0)?;
        let a_on_25 = net.cap[mark_a..].iter().any(|f| f.who == NodeId::Dut && f.ch == 25);
        if open {
            net.s.w.coord.permit_joining(&mut net.s.w.air, 254);
        }
        net.run(5000.0)?;
        let mark = net.cap.len();
        net.power_on("B")?;
        let t_on = net.now();
        let wait_s: f64 = std::env::var("CH_WAIT_S").ok().and_then(|v| v.parse().ok()).unwrap_or(240.0);
        let back = net.run_until(wait_s * 1000.0, |n| n.cap[mark..].iter().any(|f| f.who == bid && f.ch == 25 && f.zdo().is_some_and(|z| z.cluster == 0x0013)))?;
        if std::env::var("CH_DUMP").is_ok() {
            for l in net.dump(mark, net.cap.len(), |f| f.who == bid || f.mac.as_ref().is_some_and(|m| m.ty == 0)) {
                cx.say(&l);
            }
        }
        let fs: Vec<&CapFrame> = net.cap[mark..].iter().filter(|f| f.who == bid).collect();
        let chans: Vec<i64> = {
            let mut v: Vec<i64> = fs.iter().map(|f| f.ch).collect();
            v.dedup();
            v
        };
        let beacons = fs.iter().filter(|f| f.mac_cmd() == Some(0x07)).count();
        let assoc = fs.iter().filter(|f| f.mac_cmd() == Some(0x01)).count();
        let rejoin = fs.iter().filter(|f| f.nwk_cmd() == Some(0x06)).count();
        let annce = fs.iter().find(|f| f.ch == 25 && f.zdo().is_some_and(|z| z.cluster == 0x0013));
        let b1 = annce.map(|f| {
            let z = f.zdo().unwrap().payload;
            u16::from_le_bytes([z[0], z[1]])
        });
        let net_state = if open { "open" } else { "closed" };
        cx.check(net.now(), &format!("A, on, follows the coordinator to channel 25 (network {net_state})"), a_on_25,
                 &format!("A {a0:#06x}"));
        cx.check(
            net.now(),
            &format!("B, off during the change, rejoins on channel 25 with its address, no new association (network {net_state})"),
            back && b1 == Some(b0) && rejoin > 0 && assoc == 0,
            &format!(
                "A followed to 25: {a_on_25} (address {a0:#06x}); B: {} after {:.0} s; channels it used {chans:?}; Beacon Requests {beacons}, \
                 Association Requests {assoc}, Rejoin Requests {rejoin}; address before {b0:#06x}, after {}",
                if back { "announced on channel 25" } else { "not back" },
                (net.now() - t_on) / 1000.0,
                b1.map(|s| format!("{s:#06x}{}", if s != b0 { " (NEW)" } else { " (kept)" })).unwrap_or_else(|| "-".into())
            ),
        );
    }
    Ok(())
}
