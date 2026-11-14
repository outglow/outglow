//! The mesh every stress scenario runs on: a house of rooms with emulated
//! bulbs (any images), scripted routers ("plugs") and the coordinator, on
//! one 802.15.4 air with a seeded channel model, and the bookkeeping that
//! turns the air into numbers (what was sent, what arrived, how fast, what
//! the bulbs did on the way).
//!
//! Who does what:
//! - the emulated bulbs route on their own (the firmware under test);
//! - the coordinator (`Coord::new`, the spec Trust Center: carrier sense, MAC
//!   retries) is a concentrator: many-to-one route requests, source routes
//!   from the route records it gets; without one it hands a frame to a
//!   neighbour router of its own (heard in Link Status) closest to the
//!   destination and lets the mesh carry it on, and drops a source route that
//!   failed;
//! - a scripted router acknowledges at MAC and APS level, sends Link Status,
//!   answers a Route Request for itself, answers commands with a Default
//!   Response and sends its own traffic to a neighbour router; it does not
//!   relay (the bulbs are the mesh);
//! - the scenario loop advances everything in small steps (`STEP` ms),
//!   decodes what went on the air with the network key and reacts for the
//!   scripted routers.

use std::collections::{BTreeMap, BTreeSet};

use super::super::{node_of, Sim, SimCtx};
use crate::air::NodeId;
use crate::chan::{ChanRef, Link, Rng};
use crate::coord::{Coord, NWK_KEY};
use crate::frames::{aps_parse, mac_parse, nwk_parse, zcl_parse, Keys, Zcl, APS_ACK, APS_DATA, NWK_CMD, NWK_DATA};
use crate::image::Image;
use crate::peer::Peer;
use crate::session::{light_of, Stop, R};
use crate::station::{Station, Tx, HA};

/// Loop granularity (ms): reactions of the scripted routers come at most this late.
pub const STEP: f64 = 2.0;
const QUANTUM: f64 = 0.25;
/// A neighbour counts as alive this long after its last Link Status.
const LS_ALIVE_MS: f64 = 40000.0;
pub const PAN: u16 = 0x1A62;
pub const GROUP: u16 = 0x0001;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Bulb,
    Plug,
    Coord,
    /// not in the network (jammer, foreign PAN)
    Other,
}

/// A place in the house.
#[derive(Clone, Debug)]
pub struct Spot {
    pub name: &'static str,
    pub room: &'static str,
    pub kind: Kind,
    pub x: f64,
    pub y: f64,
}

const fn spot(name: &'static str, room: &'static str, kind: Kind, x: f64, y: f64) -> Spot {
    Spot { name, room, kind, x, y }
}

/// Radio range (m): nodes farther apart do not hear each other.
pub const RANGE: f64 = 6.5;

/// The full house: 9 bulbs (as in a real home network), 3 scripted routers, the
/// coordinator in the living room. The hall bulb is the bulb under test and
/// the main bridge between the kitchen and the bedroom; the only other way
/// is the weak kitchen-bedroom link (kit1-bed1, 6 m).
pub fn house_full() -> Vec<Spot> {
    vec![
        spot("coord", "living", Kind::Coord, 0.0, 0.0),
        spot("hall", "hall", Kind::Bulb, 14.5, 0.0),
        spot("liv1", "living", Kind::Bulb, 3.0, 1.0),
        spot("liv2", "living", Kind::Bulb, 2.0, -2.5),
        spot("liv3", "living", Kind::Bulb, 5.5, -1.0),
        spot("kit1", "kitchen", Kind::Bulb, 11.0, 1.5),
        spot("kit2", "kitchen", Kind::Bulb, 11.0, -2.0),
        spot("bed1", "bedroom", Kind::Bulb, 17.0, 2.0),
        spot("bed2", "bedroom", Kind::Bulb, 19.0, -2.0),
        spot("office", "office", Kind::Bulb, 22.5, 0.0),
        spot("plug-kit", "kitchen", Kind::Plug, 8.5, -1.5),
        spot("plug-hall", "hall", Kind::Plug, 13.5, 2.8),
        spot("plug-bed", "bedroom", Kind::Plug, 17.5, -1.0),
    ]
}

/// The quick house: 4 bulbs in a chain (one weak shortcut kit-bed), 3 scripted routers.
pub fn house_quick() -> Vec<Spot> {
    vec![
        spot("coord", "living", Kind::Coord, 0.0, 0.0),
        spot("hall", "hall", Kind::Bulb, 13.0, 0.0),
        spot("liv", "living", Kind::Bulb, 4.0, 0.0),
        spot("kit", "kitchen", Kind::Bulb, 9.0, 1.0),
        spot("bed", "bedroom", Kind::Bulb, 15.2, 1.2),
        spot("plug-kit", "kitchen", Kind::Plug, 8.0, -1.5),
        spot("plug-hall", "hall", Kind::Plug, 12.5, 3.0),
        spot("plug-bed", "bedroom", Kind::Plug, 16.5, -2.0),
    ]
}

pub fn dist(a: &Spot, b: &Spot) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

/// The RSSI of a link (dBm) at distance d: synthetic, -40 dBm at 0 m, 7 dB/m.
pub fn rssi_at(d: f64) -> i64 {
    (-40.0 - 7.0 * d).round() as i64
}

/// Extra loss of a weak link: 0 up to 5 m, rising to 10 % at the edge of range.
pub fn weak_loss(d: f64) -> f64 {
    ((d - 5.0) / 1.5).clamp(0.0, 1.0) * 0.10
}

/// What a scenario is run with.
#[derive(Clone, Debug)]
pub struct Cfg {
    pub full: bool,
    pub seed: u32,
    /// images per bulb: the bulb under test ("hall") gets `dut`, the others
    /// `dut` too, or every second one `mix` when given
    pub dut: Image,
    pub mix: Option<Image>,
    pub cpi: i64,
}

impl Cfg {
    pub fn from_ctx(cx: &SimCtx) -> Cfg {
        let full = std::env::var("STRESS").map(|v| v == "full").unwrap_or(false);
        let seed = std::env::var("STRESS_SEED").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
        Cfg {
            full,
            seed,
            dut: cx.image("dut", "target/tc32/bulb-tc32.bin"),
            mix: cx.images.iter().find(|i| i.label == "mix").cloned(),
            cpi: cx.cpi,
        }
    }
    pub fn variant(&self) -> &'static str {
        if self.full {
            "full"
        } else {
            "quick"
        }
    }
}

#[derive(Clone, Debug)]
pub struct BulbInfo {
    pub id: NodeId,
    pub spot: usize,
    pub name: String,
    pub ext: u64,
    /// its address now (as the coordinator knows it)
    pub short: u16,
    /// its address when the house was complete
    pub short0: u16,
    pub image: String,
    pub img: Image,
}

#[derive(Clone, Debug)]
pub struct PlugInfo {
    pub id: NodeId,
    pub spot: usize,
    pub name: String,
    pub ext: u64,
    pub short: u16,
    pub alive: bool,
}

/// One decoded air frame.
#[derive(Clone, Debug, Default)]
pub struct Dec {
    pub idx: usize,
    pub t0: f64,
    pub t1: f64,
    pub who: Option<NodeId>,
    pub mac_ty: u8,
    pub mac_seq: u8,
    pub mac_ar: bool,
    pub mac_src: Option<u16>,
    pub mac_dst: Option<u16>,
    pub pan: Option<u16>,
    pub nwk: Option<NwkInfo>,
    pub aps: Option<ApsInfo>,
    pub zcl: Option<Zcl>,
}

#[derive(Clone, Debug, Default)]
pub struct NwkInfo {
    pub ty: u8,
    pub src: u16,
    pub dst: u16,
    pub seq: u8,
    pub radius: u8,
    /// decrypted payload
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, Default)]
pub struct ApsInfo {
    pub ty: u8,
    pub delivery: u8,
    pub ack_req: bool,
    pub counter: u8,
    pub dst_ep: u8,
    pub src_ep: u8,
    pub cluster: u16,
    pub profile: u16,
    pub group: Option<u16>,
}

/// A traffic flow (what a message is).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Flow {
    /// coordinator -> bulb: On/Off or Move to Level, Default Response back
    CmdBulb,
    /// coordinator -> bulb: Read Attributes, response back
    ReadBulb,
    /// scripted router -> coordinator: Report Attributes through the mesh (one way)
    Report,
    /// scripted router -> bulb: Read Attributes, response back to the router
    PlugReadBulb,
    /// coordinator -> scripted router: On/Off, Default Response back through the mesh
    CmdPlug,
    /// coordinator -> group (broadcast): On/Off/Toggle to every bulb
    Group,
}

impl Flow {
    pub fn tag(self) -> &'static str {
        match self {
            Flow::CmdBulb => "cmd>bulb",
            Flow::ReadBulb => "read>bulb",
            Flow::Report => "plug>coord",
            Flow::PlugReadBulb => "plug>bulb",
            Flow::CmdPlug => "cmd>plug",
            Flow::Group => "group",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Msg {
    pub flow: Flow,
    pub phase: usize,
    pub src: u16,
    pub dst: u16,
    pub cluster: u16,
    pub cmd: u8,
    pub seq: u8,
    pub t: f64,
    /// the destination got it (last hop heard intact)
    pub delivered: Option<f64>,
    /// the answer reached the sender (Default Response / Read Attributes Response)
    pub answered: Option<f64>,
    /// APS counters of the answers seen at the sender (more than one: executed again)
    pub answer_ctrs: Vec<u8>,
    /// the light state the command asks for
    pub want_on: Option<bool>,
}

impl Msg {
    /// Done: answered, or (one-way flows) delivered.
    pub fn ok(&self) -> bool {
        match self.flow {
            Flow::Report => self.delivered.is_some(),
            Flow::Group => self.delivered.is_some(),
            _ => self.answered.is_some(),
        }
    }
    pub fn latency(&self) -> Option<f64> {
        match self.flow {
            Flow::Report | Flow::Group => self.delivered.map(|d| d - self.t),
            _ => self.answered.map(|a| a - self.t),
        }
    }
}

/// A phase of a scenario: a named time span the numbers are grouped by.
#[derive(Clone, Debug)]
pub struct Phase {
    pub name: String,
    pub t0: f64,
    pub t1: f64,
    /// watchdog/software resets of the bulbs at its start and end
    pub r0: u32,
    pub r1: u32,
}

/// What the bulbs did on the air in a time span.
#[derive(Clone, Debug, Default)]
pub struct AirCount {
    pub frames: u64,
    pub mac_retx: u64,
    pub rreq_new: u64,
    pub rreq_frames: u64,
    pub rrep_frames: u64,
    pub nwk_status: u64,
    pub link_status: u64,
    pub route_records: u64,
    pub bcast_relays: u64,
    pub leave: u64,
    pub rejoin_req: u64,
    pub assoc_req: u64,
    /// unicast frames a bulb acknowledged for relaying and then neither
    /// forwarded nor answered with a Network Status (within 3 s), each
    /// once (`Mesh::relays`)
    pub relay_lost: u64,
    /// ... of all unicast frames bulbs acknowledged for relaying, each once
    pub relay_acked: u64,
    /// Route Requests (originator, id; not many-to-one) that reached a bulb
    /// intact on the air, neither its own nor for it, and how many it passed on
    pub rreq_heard: u64,
    pub rreq_passed: u64,
}

pub struct Mesh {
    pub s: Sim,
    pub cfg: Cfg,
    pub spots: Vec<Spot>,
    pub chan: ChanRef,
    pub bulbs: Vec<BulbInfo>,
    pub plugs: Vec<PlugInfo>,
    /// spot index -> node
    pub node_of_spot: BTreeMap<usize, NodeId>,
    keys: Keys,
    pub dec: Vec<Dec>,
    n_dec: usize,
    pub msgs: Vec<Msg>,
    pub phases: Vec<Phase>,
    /// per bulb (index into `bulbs`): (ms, on) at every change of the light
    pub light_log: Vec<Vec<(f64, bool)>>,
    last_on: Vec<Option<bool>>,
    /// (receiver, sender short) -> (ms, neighbours listed in its Link Status)
    pub ls: BTreeMap<(NodeId, u16), (f64, Vec<u16>)>,
    /// next hops that failed recently: (node, next hop) -> until ms
    suspect: BTreeMap<(NodeId, u16), f64>,
    coord_noack: usize,
    plug_noack: Vec<usize>,
    route_rec_seen: usize,
    rrep_done: BTreeSet<(u16, u16, u8)>,
    /// per bulb: last (mac seq, psdu) for MAC retransmission counting
    last_tx: BTreeMap<NodeId, (u8, Vec<u8>)>,
    /// NWK broadcasts by (src, seq): first seen ms, for relay counting
    seen_rreq: BTreeSet<(u16, u8)>,
    /// time-stamped bulb air events (ms, which counter, bulb)
    pub air_events: Vec<(f64, &'static str, NodeId)>,
    /// plug requests received: (plug, src, aps counter) -> times
    pub plug_rx: BTreeMap<(u16, u16, u8, u8), u32>,
    pub mtorr_period: Option<f64>,
    next_mtorr: f64,
    pub traffic_rng: Rng,
    pub notes: Vec<String>,
    pub join_ms: Vec<Option<f64>>,
    /// air time when the house was complete (joined, ranges applied, routers started)
    pub t_formed: f64,
    /// print the decoded frames in this time span (STRESS_TRACE=t0-t1, ms)
    trace: Option<(f64, f64)>,
    next_refresh: f64,
    reset_seen: Vec<u32>,
    last_reset_line: std::rc::Rc<std::cell::RefCell<String>>,
    /// STRESS_ACCOUNT: what became of every frame (`account.rs`)
    pub acct: Option<super::account::Account>,
}

pub fn emu(e: String) -> Stop {
    Stop::Error(e)
}

impl Mesh {
    /// Boots the house: every bulb hears the coordinator while it joins
    /// (powered on 1.5 s apart); then the house's ranges apply.
    pub fn build(cfg: Cfg, log: Option<crate::air::Log>) -> R<Mesh> {
        let spots = if cfg.full { house_full() } else { house_quick() };
        let mut bulbs = Vec::new();
        let mut nodes = Vec::new();
        let mut k = 0usize;
        for (i, sp) in spots.iter().enumerate() {
            if sp.kind != Kind::Bulb {
                continue;
            }
            let img = match (&cfg.mix, k % 2) {
                (Some(m), 1) => m.clone(),
                _ => cfg.dut.clone(),
            };
            let mut n = node_of(&img, cfg.cpi)?;
            // the MAC sector: stock keeps it only with the Telink OUI in bytes 3..6 (else
            // it writes its own) and makes its IEEE address of bytes 6..8 + 0..6; the C and
            // Rust firmware take the 8 bytes as they are. The bulb is found by its marker
            // (bytes 0..3). The other bytes differ from bulb to bulb as factory MACs do
            // (the firmware seeds its backoff generator from them).
            let h = mac_bytes(k);
            let mac = [h[0], h[1], h[2], 0x38, 0xC1, 0xA4, h[3], h[4]];
            n.write_flash(0xFF000, &mac);
            seed_rbg(&mut n, k);
            let ext = u64::from_le_bytes(mac);
            nodes.push(n);
            bulbs.push(BulbInfo { id: NodeId::Dut, spot: i, name: sp.name.to_string(), ext, short: 0, short0: 0, image: img.label.clone(), img: img.clone() });
            k += 1;
        }
        let mut it = nodes.into_iter();
        let first = it.next().ok_or_else(|| Stop::Error("no bulbs".into()))?;
        let mut coord = Coord::new(11, NWK_KEY);
        coord.nwk_discover = 1;
        // keep the emulator's last RESET line (cause, pc) for the notes
        let last_reset = std::rc::Rc::new(std::cell::RefCell::new(String::new()));
        let lr = last_reset.clone();
        let inner = log.clone();
        let log: crate::air::Log = std::rc::Rc::new(move |line: &str| {
            if line.contains("RESET") {
                *lr.borrow_mut() = line.to_string();
            }
            if let Some(l) = &inner {
                l(line);
            }
        });
        let mut s = Sim::new(first, &bulbs[0].name, coord, Some(log), QUANTUM);
        for (j, n) in it.enumerate() {
            let id = s.w.add_bulb(n, &bulbs[j + 1].name);
            bulbs[j + 1].id = id;
        }
        let chan = ChanRef::new(cfg.seed);
        chan.install(&mut s.w.air);
        let mut keys = Keys::default();
        keys.set_nwk(0, NWK_KEY);
        keys.learn(Some(0x0000), Some(s.w.coord.ext));
        let mut node_of_spot = BTreeMap::new();
        for b in &bulbs {
            node_of_spot.insert(b.spot, b.id);
        }
        let n_b = bulbs.len();
        let mut m = Mesh {
            s,
            traffic_rng: Rng::new(cfg.seed.wrapping_mul(7919).wrapping_add(17)),
            cfg,
            spots,
            chan,
            bulbs,
            plugs: Vec::new(),
            node_of_spot,
            keys,
            dec: Vec::new(),
            n_dec: 0,
            msgs: Vec::new(),
            phases: Vec::new(),
            light_log: vec![Vec::new(); n_b],
            last_on: vec![None; n_b],
            ls: BTreeMap::new(),
            suspect: BTreeMap::new(),
            coord_noack: 0,
            plug_noack: Vec::new(),
            route_rec_seen: 0,
            rrep_done: BTreeSet::new(),
            last_tx: BTreeMap::new(),
            seen_rreq: BTreeSet::new(),
            air_events: Vec::new(),
            plug_rx: BTreeMap::new(),
            mtorr_period: None,
            next_mtorr: 0.0,
            notes: Vec::new(),
            join_ms: vec![None; n_b],
            t_formed: 0.0,
            next_refresh: 0.0,
            reset_seen: vec![0; n_b],
            last_reset_line: last_reset.clone(),
            trace: std::env::var("STRESS_TRACE").ok().and_then(|v| {
                let (a, b) = v.split_once('-')?;
                Some((a.parse().ok()?, b.parse().ok()?))
            }),
            acct: None,
        };
        if std::env::var_os("STRESS_ACCOUNT").is_some() {
            m.acct = Some(super::account::Account::new(&mut m));
        }
        for (i, sp) in m.spots.clone().iter().enumerate() {
            if sp.kind == Kind::Coord {
                m.node_of_spot.insert(i, NodeId::Coord);
            }
        }
        m.s.w.coord.start(&mut m.s.w.air);
        m.form()?;
        m.t_formed = m.now();
        Ok(m)
    }

    pub fn now(&self) -> f64 {
        self.s.now()
    }

    pub fn bulb_ix(&self, id: NodeId) -> Option<usize> {
        self.bulbs.iter().position(|b| b.id == id)
    }

    pub fn by_name(&self, name: &str) -> NodeId {
        if let Some(b) = self.bulbs.iter().find(|b| b.name == name) {
            return b.id;
        }
        if let Some(p) = self.plugs.iter().find(|p| p.name == name) {
            return p.id;
        }
        if name == "coord" {
            return NodeId::Coord;
        }
        panic!("no node {name}")
    }

    pub fn short_of(&self, id: NodeId) -> u16 {
        match id {
            NodeId::Coord => 0x0000,
            NodeId::Peer(_) => self.plugs.iter().find(|p| p.id == id).map(|p| p.short).unwrap_or(0xFFFE),
            _ => self.bulbs.iter().find(|b| b.id == id).map(|b| b.short).unwrap_or(0xFFFE),
        }
    }

    pub fn node_of_short(&self, short: u16) -> Option<NodeId> {
        if short == 0x0000 {
            return Some(NodeId::Coord);
        }
        if let Some(b) = self.bulbs.iter().find(|b| b.short == short) {
            return Some(b.id);
        }
        self.plugs.iter().find(|p| p.short == short).map(|p| p.id)
    }

    fn spot_of(&self, id: NodeId) -> usize {
        *self.node_of_spot.iter().find(|(_, n)| **n == id).map(|(s, _)| s).unwrap()
    }

    pub fn room_of(&self, id: NodeId) -> &'static str {
        self.spots[self.spot_of(id)].room
    }

    /// What the coordinator knows of the bulbs (their addresses, also those
    /// of joiners not yet announced), and its next hop to each: the
    /// coordinator's own frames (keys, ZDO) take the same way as the traffic.
    fn refresh_coord(&mut self) {
        // a joiner is reached the coordinator's own way until it has announced itself
        let devs: Vec<(u64, u16)> = self.s.w.coord.devices.iter().filter(|d| d.announced).map(|d| (d.ext, d.short)).collect();
        for (ext, short) in devs {
            let b = ext.to_le_bytes();
            let Some(k) = (0..self.bulbs.len()).find(|k| {
                let h = mac_bytes(*k);
                b.windows(3).any(|w| w == [h[0], h[1], h[2]])
            }) else {
                continue;
            };
            if self.bulbs[k].short != short {
                if self.t_formed > 0.0 {
                    self.notes.push(format!("{:.1} s: {} has a new address {short:#06x} (was {:#06x})", self.now() / 1000.0,
                                            self.bulbs[k].name, self.bulbs[k].short));
                }
                self.bulbs[k].short = short;
                self.bulbs[k].ext = ext;
                self.keys.learn(Some(short), Some(ext));
            }
        }
        for k in 0..self.bulbs.len() {
            let sh = self.bulbs[k].short;
            if sh != 0 && !self.s.w.air.off.contains(&self.bulbs[k].id) {
                self.coord_route(sh);
            }
        }
    }

    /// The IEEE address a bulb announced, found by the marker in its MAC sector.
    fn announced_ext(&self, k: usize) -> Option<u64> {
        let h = mac_bytes(k);
        let mark = [h[0], h[1], h[2]];
        self.s.w.coord.devices.iter().filter(|d| d.announced).map(|d| d.ext).find(|e| e.to_le_bytes().windows(3).any(|w| w == mark))
    }

    /// The house forms in place: ranges apply from the start, bulbs are
    /// powered one after the other, nearest to the coordinator first, each once
    /// the previous one has joined (through the coordinator or a bulb: the
    /// coordinator opens joining with Mgmt_Permit_Joining_req, again after
    /// every join); then the link key exchanges, then the scripted routers.
    fn form(&mut self) -> R<()> {
        let n = self.bulbs.len();
        self.apply_ranges();
        let cs = self.spot_of(NodeId::Coord);
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by_key(|i| (self.hops(self.bulbs[*i].spot, cs), *i));
        for b in &self.bulbs {
            self.s.w.power_off(b.id);
        }
        self.s.w.coord.permit_joining(&mut self.s.w.air, 254);
        self.tick(500.0)?;
        for &i in &order {
            let id = self.bulbs[i].id;
            self.power_on(id)?;
            let t_on = self.now();
            let mut helped = false;
            let mut t_help = 0.0;
            while self.now() - t_on < 50000.0 || (helped && self.now() - t_help < 40000.0) {
                self.tick(100.0)?;
                if let Some(e) = self.announced_ext(i) {
                    self.bulbs[i].ext = e;
                    self.join_ms[i] = Some(self.now() - t_on);
                    break;
                }
                if helped {
                    // the coordinator's next hop from the failed join through a router is stale
                    let h = mac_bytes(i);
                    let mark = [h[0], h[1], h[2]];
                    let stale: Vec<u16> = self.s.w.coord.devices.iter()
                        .filter(|d| d.ext.to_le_bytes().windows(3).any(|w| w == mark)).map(|d| d.short).collect();
                    for sh in stale {
                        self.s.w.coord.st.via.remove(&sh);
                    }
                }
                // a join through a router that the Trust Center answered three times without the
                // newcomer ever announcing itself will not complete; nor one that took 45 s
                let h = mac_bytes(i);
                let mark = format!("{:02x}:{:02x}:{:02x}", h[2], h[1], h[0]);
                let tries = self.s.w.coord.events.iter().filter(|e| e.ms >= t_on && e.kind == "update-device"
                    && e.get("ext").is_some_and(|x| x.contains(&mark))).count();
                if !helped && (tries >= 3 || self.now() - t_on > 45000.0) {
                    // no join through the mesh: let it hear the coordinator (paired next to it)
                    helped = true;
                    let all: Vec<NodeId> = self.s.w.air.nodes.clone();
                    for o in all {
                        if o != id {
                            self.s.w.air.block(id, o, o != NodeId::Coord);
                        }
                    }
                    // and start over as from the factory (its flash as first loaded: no network)
                    let mut n = node_of(&self.bulbs[i].img, self.cfg.cpi)?;
                    let h = mac_bytes(i);
                    n.write_flash(0xFF000, &[h[0], h[1], h[2], 0x38, 0xC1, 0xA4, h[3], h[4]]);
                    seed_rbg(&mut n, i);
                    match id {
                        NodeId::Dut => {
                            self.s.w.air.dut = n;
                            self.s.w.air.t_off = self.s.w.air.t;
                        }
                        NodeId::Bulb(b) => {
                            let bb = &mut self.s.w.air.bulbs[b as usize];
                            bb.node = n;
                            bb.t_off = bb.t;
                        }
                        _ => {}
                    }
                    t_help = self.now();
                    self.notes.push(format!("{} did not join through the mesh ({}); joined in the coordinator's range",
                                            self.bulbs[i].name,
                                            if tries >= 3 { format!("{tries} Update-Device, no announce") } else { "45 s".into() }));
                }
            }
            if helped && self.join_ms[i].is_some() {
                // its link key exchange, then back to its place, and a power cycle there so that
                // what it heard next to the coordinator is not in its neighbour table
                self.tick(8000.0)?;
                self.apply_ranges();
                self.s.w.reboot_bulb(id).map_err(emu)?;
                self.tick(6000.0)?;
            }
            self.tick(1500.0)?;
            self.s.w.coord.permit_joining(&mut self.s.w.air, 254);
            self.tick(500.0)?;
        }
        for i in 0..n {
            let e = self.bulbs[i].ext;
            match self.s.announced(e).filter(|_| self.join_ms[i].is_some()) {
                Some(sh) => {
                    self.bulbs[i].short = sh;
                    self.bulbs[i].short0 = sh;
                    self.keys.learn(Some(sh), Some(e));
                }
                None => self.bulbs[i].short = 0,
            }
        }
        let missing: Vec<String> = self.bulbs.iter().filter(|b| b.short == 0).map(|b| b.name.clone()).collect();
        if !missing.is_empty() {
            return Err(Stop::Fail(format!("bulbs did not join: {} ({})", missing.join(", "), self.notes.join("; "))));
        }
        // link key exchange of every bulb that does one, then a little calm
        self.tick(8000.0)?;
        // scripted routers
        self.add_plugs();
        self.start_plugs()?;
        Ok(())
    }

    /// Ranges and link qualities of the house (static positions).
    fn apply_ranges(&mut self) {
        let spots = self.spots.clone();
        let ids: Vec<(usize, NodeId)> = self.node_of_spot.iter().map(|(s, n)| (*s, *n)).collect();
        for &(sa, a) in &ids {
            for &(sb, b) in &ids {
                if a == b {
                    continue;
                }
                let d = dist(&spots[sa], &spots[sb]);
                if d > RANGE {
                    self.s.w.air.block(a, b, true);
                } else {
                    self.s.w.air.block(a, b, false);
                    // asymmetric by a fixed amount per direction (transmit power, antenna)
                    let skew = ((sa * 7 + sb * 3) % 5) as i64 - 2;
                    self.chan.m().links.insert((a, b), Link { rssi: rssi_at(d) + skew, loss: weak_loss(d) });
                }
            }
        }
    }

    fn add_plugs(&mut self) {
        let spots = self.spots.clone();
        let (ch, key_seq) = (self.s.w.coord.st.channel, self.s.w.coord.st.key_seq);
        for (i, sp) in spots.iter().enumerate() {
            if sp.kind != Kind::Plug {
                continue;
            }
            let k = self.plugs.len();
            let ext = 0x00124B00AA000000u64 | (k as u64 + 1);
            let short = 0x6A01 + k as u16 * 0x0111;
            let id = NodeId::Peer(self.s.w.peers.len() as u8);
            let mut st = Station::new(id, ch, PAN, Some(short), ext, NWK_KEY, (0x10, 0x20, 0x30, 0x40, 0x50), 0x100, 0);
            st.key_seq = key_seq;
            let p = Peer::bare_ack(st, sp.name);
            let id2 = self.s.w.add_peer_node(p);
            assert_eq!(id, id2);
            self.node_of_spot.insert(i, id);
            self.keys.learn(Some(short), Some(ext));
            self.plugs.push(PlugInfo { id, spot: i, name: sp.name.to_string(), ext, short, alive: true });
            self.plug_noack.push(0);
        }
        self.apply_ranges();
    }

    /// Starts the scripted routers' Link Status (every 15 s, 4 s apart).
    fn start_plugs(&mut self) -> R<()> {
        for k in 0..self.plugs.len() {
            let nb = self.plug_neighbours(k);
            let id = self.plugs[k].id;
            let NodeId::Peer(pi) = id else { unreachable!() };
            let w = &mut self.s.w;
            w.peers[pi as usize].start_link_status(&mut w.air, 15000.0, Some(nb));
            self.tick(4000.0)?;
        }
        Ok(())
    }

    /// Another node that is not part of the mesh (a jammer, a station of a
    /// neighbour's network): a scripted node at (x, y) with its own PAN;
    /// `ack`: it acknowledges frames addressed to it.
    pub fn add_other(&mut self, name: &'static str, room: &'static str, x: f64, y: f64, pan: u16, short: u16, ack: bool) -> NodeId {
        let id = NodeId::Peer(self.s.w.peers.len() as u8);
        let ch = self.s.w.coord.st.channel;
        let st = Station::new(id, ch, pan, Some(short), 0x00124B00EE000000 | short as u64, [0; 16], (0, 0, 0, 0, 0), 0, 0);
        let p = if ack { Peer::bare_ack(st, name) } else { Peer::bare(st, name) };
        let id2 = self.s.w.add_peer_node(p);
        assert_eq!(id, id2);
        self.spots.push(Spot { name, room, kind: Kind::Other, x, y });
        self.node_of_spot.insert(self.spots.len() - 1, id);
        self.apply_ranges();
        id
    }

    /// The routers a scripted router lists in its Link Status: those in range and powered.
    fn plug_neighbours(&self, k: usize) -> Vec<u16> {
        let me = self.plugs[k].spot;
        let mut v = Vec::new();
        for (s, n) in &self.node_of_spot {
            if *s == me || self.spots[*s].kind == Kind::Other || dist(&self.spots[*s], &self.spots[me]) > RANGE {
                continue;
            }
            if self.s.w.air.off.contains(n) {
                continue;
            }
            if let NodeId::Peer(_) = n {
                if !self.plugs.iter().any(|p| p.id == *n && p.alive) {
                    continue;
                }
            }
            v.push(self.short_of(*n));
        }
        v.sort();
        v
    }

    /// Static hop count over the routers that relay (bulbs, coordinator) from spot `a` to spot `b`.
    pub fn hops(&self, a: usize, b: usize) -> u32 {
        let relay = |s: usize| matches!(self.spots[s].kind, Kind::Bulb | Kind::Coord);
        let mut dist_v = vec![u32::MAX; self.spots.len()];
        dist_v[b] = 0;
        let mut q = std::collections::VecDeque::from([b]);
        while let Some(x) = q.pop_front() {
            if x != b && !relay(x) {
                continue;
            }
            for y in 0..self.spots.len() {
                if dist_v[y] == u32::MAX && dist(&self.spots[x], &self.spots[y]) <= RANGE {
                    dist_v[y] = dist_v[x] + 1;
                    q.push_back(y);
                }
            }
        }
        dist_v[a]
    }

    // ------------------------------------------------------------------ running
    /// Runs `ms` in steps of `STEP`, decoding the air and reacting.
    pub fn tick(&mut self, ms: f64) -> R<()> {
        let end = self.now() + ms;
        while self.now() < end - 1e-9 {
            let t = end.min(self.now() + STEP);
            self.s.w.lockstep(t, QUANTUM).map_err(emu)?;
            self.after_step()?;
        }
        Ok(())
    }

    fn after_step(&mut self) -> R<()> {
        // watchdog/software resets: when, which bulb, and the emulator's line about it
        for k in 0..self.bulbs.len() {
            let n = self.s.w.bulb_resets(self.bulbs[k].id);
            if n != self.reset_seen[k] {
                self.reset_seen[k] = n;
                let line = self.last_reset_line.borrow().clone();
                self.notes.push(format!("{:.1} s: {} reset ({})", self.now() / 1000.0, self.bulbs[k].name, line.trim()));
            }
        }
        if self.now() >= self.next_refresh {
            self.next_refresh = self.now() + 100.0;
            self.refresh_coord();
        }
        self.decode_new();
        if let Some(mut a) = self.acct.take() {
            a.poll(self);
            self.acct = Some(a);
        }
        self.sample_lights();
        self.watch_failures();
        self.take_route_records();
        if let Some(p) = self.mtorr_period {
            if self.now() >= self.next_mtorr {
                self.s.w.coord.many_to_one_route_request(&mut self.s.w.air, false);
                self.next_mtorr = self.now() + p;
            }
        }
        Ok(())
    }

    pub fn start_mtorr(&mut self, period: f64) {
        self.mtorr_period = Some(period);
        self.next_mtorr = self.now();
    }

    fn sample_lights(&mut self) {
        let t = self.now();
        for i in 0..self.bulbs.len() {
            let id = self.bulbs[i].id;
            if self.s.w.air.off.contains(&id) {
                continue;
            }
            let on = light_of(self.s.w.node(id)).on;
            if self.last_on[i] != Some(on) {
                if self.last_on[i].is_some() {
                    self.light_log[i].push((t, on));
                }
                self.last_on[i] = Some(on);
            }
        }
    }

    pub fn light_on(&self, id: NodeId) -> bool {
        light_of(self.s.w.node(id)).on
    }

    /// Decodes every frame that has ended by now.
    fn decode_new(&mut self) {
        let now = self.now();
        while self.n_dec < self.s.w.air.frames.len() {
            let f = self.s.w.air.frames[self.n_dec].clone();
            if f.t1 > now {
                break;
            }
            let idx = self.n_dec;
            self.n_dec += 1;
            let mut d = Dec { idx, t0: f.t0, t1: f.t1, who: Some(f.who), ..Default::default() };
            if let Some(m) = mac_parse(&f.psdu) {
                d.mac_ty = m.ty;
                d.mac_seq = m.seq;
                d.mac_ar = m.ar;
                d.pan = m.dpan;
                d.mac_src = if m.smode == 2 { m.src.map(|s| s as u16) } else { None };
                d.mac_dst = if m.dmode == 2 { m.dst.map(|s| s as u16) } else { None };
                if m.ty == 1 && m.dpan == Some(PAN) && !m.payload.is_empty() {
                    if let Some(n) = nwk_parse(&m.payload, &mut self.keys) {
                        if n.ver == 2 {
                            let pl = n.payload.clone().unwrap_or_default();
                            if n.ty == NWK_DATA && n.payload.is_some() {
                                if let Some(a) = aps_parse(&pl, &self.keys, Some(n.src)) {
                                    let ai = ApsInfo {
                                        ty: a.ty,
                                        delivery: a.delivery,
                                        ack_req: a.ack_req,
                                        counter: a.counter.unwrap_or(0),
                                        dst_ep: a.dst_ep.unwrap_or(0),
                                        src_ep: a.src_ep.unwrap_or(0),
                                        cluster: a.cluster.unwrap_or(0),
                                        profile: a.profile.unwrap_or(0),
                                        group: a.group,
                                    };
                                    if a.ty == APS_DATA && ai.profile == HA && a.frag.unwrap_or(0) == 0 {
                                        d.zcl = a.payload.as_ref().and_then(|p| zcl_parse(p));
                                    }
                                    d.aps = Some(ai);
                                }
                            }
                            d.nwk = Some(NwkInfo { ty: n.ty, src: n.src, dst: n.dst, seq: n.seq, radius: n.radius, payload: pl });
                        }
                    }
                }
            }
            self.count_air(&d, &f.psdu);
            if let Some((a, b)) = self.trace {
                if d.t0 >= a && d.t0 < b {
                    println!("{}", self.brief(&d));
                }
            }
            self.react(&d);
            self.match_msgs(&d);
            self.dec.push(d);
        }
    }

    fn is_bulb(&self, id: NodeId) -> bool {
        matches!(id, NodeId::Dut | NodeId::Bulb(_))
    }

    fn count_air(&mut self, d: &Dec, psdu: &[u8]) {
        let Some(who) = d.who else { return };
        if !self.is_bulb(who) {
            // the coordinator and the plugs: only Link Status heard matters (below)
        } else {
            self.air_events.push((d.t0, "frame", who));
            if d.mac_ty == 1 && d.mac_ar {
                let body = psdu[..psdu.len().saturating_sub(2)].to_vec();
                if self.last_tx.get(&who).is_some_and(|(s, b)| *s == d.mac_seq && *b == body) {
                    self.air_events.push((d.t0, "mac_retx", who));
                }
                self.last_tx.insert(who, (d.mac_seq, body));
            }
            if d.mac_ty == 3 {
                if let Some(m) = mac_parse(psdu) {
                    if m.payload.first() == Some(&0x01) {
                        self.air_events.push((d.t0, "assoc_req", who));
                    }
                }
            }
            if let Some(n) = &d.nwk {
                if n.ty == NWK_CMD {
                    match n.payload.first() {
                        Some(0x01) => {
                            self.air_events.push((d.t0, "rreq_frame", who));
                            let id = n.payload.get(2).copied().unwrap_or(0);
                            if self.seen_rreq.insert((n.src, id)) && Some(n.src) == Some(self.short_of(who)) {
                                self.air_events.push((d.t0, "rreq_new", who));
                            }
                        }
                        Some(0x02) => self.air_events.push((d.t0, "rrep", who)),
                        Some(0x03) => self.air_events.push((d.t0, "nwk_status", who)),
                        Some(0x04) => self.air_events.push((d.t0, "leave", who)),
                        Some(0x05) => self.air_events.push((d.t0, "route_record", who)),
                        Some(0x06) => self.air_events.push((d.t0, "rejoin_req", who)),
                        Some(0x08) => self.air_events.push((d.t0, "link_status", who)),
                        _ => {}
                    }
                } else if n.dst >= 0xFFF8 && n.src != self.short_of(who) {
                    self.air_events.push((d.t0, "bcast_relay", who));
                }
            }
        }
        // Link Status as each receiver got it
        if let Some(n) = &d.nwk {
            if n.ty == NWK_CMD && n.payload.first() == Some(&0x08) {
                let cnt = (n.payload.get(1).copied().unwrap_or(0) & 0x1F) as usize;
                let nb: Vec<u16> = (0..cnt)
                    .filter_map(|k| n.payload.get(2 + 3 * k..4 + 3 * k).map(|b| u16::from_le_bytes([b[0], b[1]])))
                    .collect();
                let rx: Vec<NodeId> = std::iter::once(NodeId::Coord).chain(self.plugs.iter().map(|p| p.id)).collect();
                for r in rx {
                    if self.chan.got(d.idx, r) {
                        self.ls.insert((r, n.src), (d.t1, nb.clone()));
                    }
                }
            }
        }
    }

    /// The air counters of the bulbs between t0 and t1 (optionally one bulb only).
    pub fn air_count(&self, t0: f64, t1: f64, only: Option<NodeId>) -> AirCount {
        let mut c = AirCount::default();
        let (acked, lost) = self.relay_drops(t0, t1, only);
        c.relay_acked = acked;
        c.relay_lost = lost;
        let (h, p) = self.rreq_relays(t0, t1, only);
        c.rreq_heard = h;
        c.rreq_passed = p;
        for (t, k, who) in &self.air_events {
            if *t < t0 || *t >= t1 || only.is_some_and(|o| o != *who) {
                continue;
            }
            match *k {
                "frame" => c.frames += 1,
                "mac_retx" => c.mac_retx += 1,
                "rreq_new" => c.rreq_new += 1,
                "rreq_frame" => c.rreq_frames += 1,
                "rrep" => c.rrep_frames += 1,
                "nwk_status" => c.nwk_status += 1,
                "link_status" => c.link_status += 1,
                "route_record" => c.route_records += 1,
                "bcast_relay" => c.bcast_relays += 1,
                "leave" => c.leave += 1,
                "rejoin_req" => c.rejoin_req += 1,
                "assoc_req" => c.assoc_req += 1,
                _ => {}
            }
        }
        c
    }

    /// The relayed frames a bulb acknowledged and lost (debugging): (index into `dec`, bulb).
    pub fn relay_lost_list(&self) -> Vec<(usize, NodeId)> {
        self.relays(0.0, f64::MAX, None).into_iter().filter(|r| r.2).map(|r| (r.0, r.1)).collect()
    }

    /// (Route Requests heard, passed on) by the bulbs between t0 and t1.
    pub fn rreq_relays(&self, t0: f64, t1: f64, only: Option<NodeId>) -> (u64, u64) {
        let mut heard: BTreeMap<(NodeId, u16, u8), f64> = BTreeMap::new();
        let mut passed: BTreeSet<(NodeId, u16, u8)> = BTreeSet::new();
        let start = self.dec.partition_point(|d| d.t0 < t0);
        for d in &self.dec[start..] {
            if d.t0 >= t1 + 2000.0 {
                break;
            }
            let Some(n) = &d.nwk else { continue };
            if n.ty != NWK_CMD || n.payload.first() != Some(&0x01) || n.payload.len() < 6 {
                continue;
            }
            if (n.payload[1] >> 3) & 3 != 0 {
                continue; // many-to-one
            }
            let (id, target) = (n.payload[2], u16::from_le_bytes([n.payload[3], n.payload[4]]));
            if let Some(w) = d.who.filter(|w| self.is_bulb(*w)) {
                if n.src != self.short_of(w) {
                    passed.insert((w, n.src, id));
                }
            }
            if d.t0 >= t1 {
                continue;
            }
            for b in &self.bulbs {
                if only.is_some_and(|o| o != b.id) || b.short == n.src || b.short == target || Some(b.id) == d.who {
                    continue;
                }
                if self.chan.got(d.idx, b.id) {
                    heard.entry((b.id, n.src, id)).or_insert(d.t0);
                }
            }
        }
        let p = heard.keys().filter(|k| passed.contains(k)).count() as u64;
        (heard.len() as u64, p)
    }

    /// (relayed frames acknowledged, of those lost) by the bulbs between t0 and t1.
    pub fn relay_drops(&self, t0: f64, t1: f64, only: Option<NodeId>) -> (u64, u64) {
        let v = self.relays(t0, t1, only);
        (v.len() as u64, v.iter().filter(|r| r.2).count() as u64)
    }

    /// The unicast frames for someone else that a bulb acknowledged between
    /// t0 and t1, each once: (index into `dec` of the first copy it
    /// acknowledged, bulb, lost). Lost: within 3 s of that copy the bulb
    /// neither passed it on nor answered it with a Network Status. A frame
    /// is the same frame when the same neighbour hands the same NWK frame
    /// (source, sequence number) to the same bulb again within 5 s, as it
    /// does when it missed the acknowledgement: a MAC retransmission, or the
    /// same secured frame sent once more, which the bulb rightly drops as a
    /// replay. Counting every copy would count one frame several times, and
    /// a copy of a frame passed on before it as lost.
    fn relays(&self, t0: f64, t1: f64, only: Option<NodeId>) -> Vec<(usize, NodeId, bool)> {
        let mut out = Vec::new();
        let mut first: BTreeMap<(NodeId, Option<NodeId>, u16, u8), f64> = BTreeMap::new();
        // copies before t0 still make a later one a copy
        let start = self.dec.partition_point(|d| d.t0 < t0 - 5000.0);
        for i in start..self.dec.len() {
            let d = &self.dec[i];
            if d.t0 >= t1 {
                break;
            }
            let (Some(n), Some(md)) = (&d.nwk, d.mac_dst) else { continue };
            if md >= 0xFFF8 || n.dst == md || d.mac_ty != 1 {
                continue;
            }
            let Some(x) = self.node_of_short(md).filter(|x| self.is_bulb(*x)) else { continue };
            if only.is_some_and(|o| o != x) {
                continue;
            }
            // its acknowledgement right after
            let ack = self.dec[i + 1..].iter().take_while(|e| e.t0 < d.t1 + 2.0).any(|e| e.who == Some(x) && e.mac_ty == 2 && e.mac_seq == d.mac_seq);
            if !ack {
                continue;
            }
            let key = (x, d.who, n.src, n.seq);
            if first.get(&key).is_some_and(|t| d.t0 - t < 5000.0) {
                continue;
            }
            first.insert(key, d.t0);
            if d.t0 < t0 {
                continue;
            }
            let fwd = self.dec[i + 1..].iter().take_while(|e| e.t0 < d.t1 + 3000.0).any(|e| handled(e, x, n));
            out.push((i, x, !fwd));
        }
        out
    }

    // ------------------------------------------------------------------ scripted routers
    /// What the scripted routers do with frames they received.
    fn react(&mut self, d: &Dec) {
        let Some(n) = d.nwk.clone() else { return };
        for k in 0..self.plugs.len() {
            let p = self.plugs[k].clone();
            if !p.alive || !self.chan.got(d.idx, p.id) {
                continue;
            }
            let to_me = d.mac_dst == Some(p.short) || d.mac_dst == Some(0xFFFF);
            if !to_me {
                continue;
            }
            let hop = d.mac_src.unwrap_or(n.src);
            // a Route Request for us: Route Reply to the router we heard it from
            if n.ty == NWK_CMD && n.payload.first() == Some(&0x01) && n.payload.len() >= 6 {
                let target = u16::from_le_bytes([n.payload[3], n.payload[4]]);
                let rid = n.payload[2];
                if target == p.short && self.rrep_done.insert((p.short, n.src, rid)) {
                    let cost = n.payload[5].saturating_add(1);
                    let mut rep = vec![0x02, 0x00, rid];
                    rep.extend_from_slice(&n.src.to_le_bytes());
                    rep.extend_from_slice(&p.short.to_le_bytes());
                    rep.push(cost);
                    let (st, air) = self.s.w.st(p.id);
                    st.tx_nwk_cmd(air, hop, &rep, &Tx::new().mac_dst(hop).radius(30));
                }
                continue;
            }
            if n.ty != NWK_DATA || n.dst != p.short {
                continue;
            }
            let Some(a) = d.aps.clone() else { continue };
            if a.ty == APS_ACK {
                continue;
            }
            if a.ack_req {
                let mut ack = vec![0x02, a.src_ep];
                ack.extend_from_slice(&a.cluster.to_le_bytes());
                ack.extend_from_slice(&a.profile.to_le_bytes());
                ack.push(a.dst_ep);
                ack.push(a.counter);
                self.plug_route(k, n.src);
                let (st, air) = self.s.w.st(p.id);
                st.tx_nwk(air, NWK_DATA, n.src, &ack, &Tx::new().discover(1));
            }
            if a.ty != APS_DATA {
                continue;
            }
            let Some(z) = d.zcl.clone() else { continue };
            let e = self.plug_rx.entry((p.short, n.src, a.counter, z.seq)).or_insert(0);
            *e += 1;
            if *e > 1 {
                continue; // the same APS frame again: rejected as a duplicate
            }
            // a command (not a response) that wants a Default Response
            if !z.to_client && !z.ddr && z.cmd != 0x0B && (z.specific || z.cmd != 0x0A) {
                self.plug_route(k, n.src);
                let (st, air) = self.s.w.st(p.id);
                let o = Tx::new().general().to_client().ddr().seq(z.seq).ep(a.src_ep).src_ep(a.dst_ep).discover(1);
                st.tx_zcl(air, n.src, a.cluster, 0x0B, &[z.cmd, 0x00], &o);
            }
        }
    }

    /// Next hop of scripted router k towards `dst`: the destination itself if
    /// it is a neighbour, else the live neighbour router closest to it.
    pub fn plug_route(&mut self, k: usize, dst: u16) {
        let p = self.plugs[k].clone();
        let now = self.now();
        let alive: Vec<u16> = self
            .ls
            .iter()
            .filter(|((r, _), (t, nb))| *r == p.id && now - *t < LS_ALIVE_MS && nb.contains(&p.short))
            .map(|((_, s), _)| *s)
            .filter(|s| self.suspect.get(&(p.id, *s)).map_or(true, |u| *u < now))
            .collect();
        let hop = self.pick_hop(&alive, dst).or_else(|| self.pick_hop(&self.in_range(p.id), dst));
        let (st, _) = self.s.w.st(p.id);
        match hop {
            Some(h) => {
                st.via.insert(dst, h);
            }
            None => {
                st.via.remove(&dst);
            }
        }
    }

    /// The routers in range of `id` (static positions), powered and not suspect: the
    /// fallback before any Link Status was heard.
    fn in_range(&self, id: NodeId) -> Vec<u16> {
        let me = self.spot_of(id);
        let now = self.now();
        self.node_of_spot
            .iter()
            .filter(|(s, n)| {
                **s != me
                    && matches!(self.spots[**s].kind, Kind::Bulb | Kind::Coord)
                    && dist(&self.spots[**s], &self.spots[me]) <= RANGE
                    && !self.s.w.air.off.contains(n)
            })
            .map(|(_, n)| self.short_of(*n))
            .filter(|s| self.suspect.get(&(id, *s)).map_or(true, |u| *u < now))
            .collect()
    }

    /// Among neighbour routers `alive`, the destination itself or the one closest to it.
    fn pick_hop(&self, alive: &[u16], dst: u16) -> Option<u16> {
        if alive.contains(&dst) {
            return Some(dst);
        }
        let ds = self.node_of_short(dst).map(|n| self.spot_of(n))?;
        alive
            .iter()
            .filter(|s| self.node_of_short(**s).is_some_and(|n| n != NodeId::Coord || dst == 0) && !self.is_plug_short(**s))
            .min_by_key(|s| (self.hops(self.spot_of(self.node_of_short(**s).unwrap()), ds), **s))
            .copied()
    }

    fn is_plug_short(&self, s: u16) -> bool {
        self.plugs.iter().any(|p| p.short == s)
    }

    /// The coordinator's way to `dst`: direct if a neighbour, a source route
    /// from a route record, else via its neighbour router closest to `dst`.
    pub fn coord_route(&mut self, dst: u16) {
        let now = self.now();
        let alive: Vec<u16> = self
            .ls
            .iter()
            .filter(|((r, _), (t, nb))| *r == NodeId::Coord && now - *t < LS_ALIVE_MS && nb.contains(&0x0000))
            .map(|((_, s), _)| *s)
            .filter(|s| self.suspect.get(&(NodeId::Coord, *s)).map_or(true, |u| *u < now))
            .filter(|s| !self.is_plug_short(*s))
            .collect();
        let c = &mut self.s.w.coord;
        if alive.contains(&dst) {
            c.source_routes.remove(&dst);
            c.st.via.remove(&dst);
            return;
        }
        if let Some(r) = c.source_routes.get(&dst) {
            if r.last().is_some_and(|h| alive.contains(h)) {
                return;
            }
            c.source_routes.remove(&dst);
        }
        let hop = self.pick_hop(&alive, dst).or_else(|| self.pick_hop(&self.in_range(NodeId::Coord), dst));
        let c = &mut self.s.w.coord;
        match hop {
            Some(h) => {
                c.st.via.insert(dst, h);
            }
            None => {
                c.st.via.remove(&dst);
            }
        }
    }

    /// MAC failures of the coordinator and the plugs: that next hop is suspect for a while.
    fn watch_failures(&mut self) {
        let now = self.now();
        let nwk_dst = |mpdu: &[u8]| -> Option<(u16, u16)> {
            let m = mac_parse(&crate::frames::with_fcs(mpdu))?;
            let hop = m.dst? as u16;
            let p = &m.payload;
            Some((hop, u16::from_le_bytes([*p.get(2)?, *p.get(3)?])))
        };
        let na = self.s.w.coord.st.no_ack.len();
        for i in self.coord_noack..na {
            let mpdu = self.s.w.coord.st.no_ack[i].1.clone();
            if let Some((hop, dst)) = nwk_dst(&mpdu) {
                self.suspect.insert((NodeId::Coord, hop), now + 20000.0);
                self.s.w.coord.source_routes.remove(&dst);
                self.s.w.coord.st.via.remove(&dst);
            }
        }
        self.coord_noack = na;
        for k in 0..self.plugs.len() {
            let id = self.plugs[k].id;
            let NodeId::Peer(pi) = id else { continue };
            let na = self.s.w.peers[pi as usize].st.no_ack.len();
            for i in self.plug_noack[k]..na {
                let mpdu = self.s.w.peers[pi as usize].st.no_ack[i].1.clone();
                if let Some((hop, _)) = nwk_dst(&mpdu) {
                    self.suspect.insert((id, hop), now + 20000.0);
                }
            }
            self.plug_noack[k] = na;
        }
        // Network Status to the coordinator about a destination: its source route is stale
        // (handled in match_msgs through the decoded frames)
    }

    fn take_route_records(&mut self) {
        let n = self.s.w.coord.route_records.len();
        for i in self.route_rec_seen..n {
            let (_, src, relays) = self.s.w.coord.route_records[i].clone();
            if relays.is_empty() {
                self.s.w.coord.source_routes.remove(&src);
            } else {
                self.s.w.coord.source_routes.insert(src, relays);
            }
        }
        self.route_rec_seen = n;
    }

    // ------------------------------------------------------------------ traffic
    pub fn phase(&mut self, name: &str) -> usize {
        if self.phases.is_empty() {
            // the addresses the bulbs start the test with
            self.refresh_coord();
            for b in self.bulbs.iter_mut() {
                b.short0 = b.short;
            }
        }
        self.end_phase();
        let r = self.resets();
        self.phases.push(Phase { name: name.to_string(), t0: self.now(), t1: -1.0, r0: r, r1: r });
        self.phases.len() - 1
    }

    pub fn end_phase(&mut self) {
        let t = self.now();
        let r = self.resets();
        if let Some(p) = self.phases.last_mut() {
            if p.t1 < 0.0 {
                p.t1 = t;
                p.r1 = r;
            }
        }
    }

    fn cur_phase(&self) -> usize {
        self.phases.len().saturating_sub(1)
    }

    fn push_msg(&mut self, flow: Flow, src: u16, dst: u16, cluster: u16, cmd: u8, seq: u8, want_on: Option<bool>) -> usize {
        self.msgs.push(Msg {
            flow,
            phase: self.cur_phase(),
            src,
            dst,
            cluster,
            cmd,
            seq,
            t: self.now(),
            delivered: None,
            answered: None,
            answer_ctrs: Vec::new(),
            want_on,
        });
        self.msgs.len() - 1
    }

    /// Coordinator -> bulb: On, Off or Move to Level (transition 0).
    pub fn cmd_bulb(&mut self, b: usize, what: u8) -> usize {
        let dst = self.bulbs[b].short;
        self.coord_route(dst);
        let (cluster, cmd, pl, want): (u16, u8, Vec<u8>, Option<bool>) = match what {
            0 => (0x0006, 0x00, vec![], Some(false)),
            1 => (0x0006, 0x01, vec![], Some(true)),
            l => (0x0008, 0x04, vec![l, 0, 0], Some(true)),
        };
        let seq = self.s.w.coord.zcl_cmd(&mut self.s.w.air, dst, 1, cluster, cmd, &pl);
        self.push_msg(Flow::CmdBulb, 0, dst, cluster, cmd, seq, want)
    }

    pub fn read_bulb(&mut self, b: usize) -> usize {
        let dst = self.bulbs[b].short;
        self.coord_route(dst);
        let seq = self.s.w.coord.read_attrs(&mut self.s.w.air, dst, 1, 0x0006, &[0x0000]);
        self.push_msg(Flow::ReadBulb, 0, dst, 0x0006, 0x00, seq, None)
    }

    /// Scripted router -> coordinator: a temperature report.
    pub fn report(&mut self, k: usize) -> Option<usize> {
        let p = self.plugs[k].clone();
        if !p.alive {
            return None;
        }
        self.plug_route(k, 0x0000);
        let v = (2000 + (self.msgs.len() % 500) as i16).to_le_bytes();
        let (st, air) = self.s.w.st(p.id);
        let o = Tx::new().general().ddr().discover(1);
        let sent = st.tx_zcl(air, 0x0000, 0x0402, 0x0A, &[0x00, 0x00, 0x29, v[0], v[1]], &o);
        Some(self.push_msg(Flow::Report, p.short, 0x0000, 0x0402, 0x0A, sent.zcl_seq.unwrap_or(0), None))
    }

    /// Scripted router -> bulb: Read Attributes (On/Off).
    pub fn plug_read_bulb(&mut self, k: usize, b: usize) -> Option<usize> {
        let p = self.plugs[k].clone();
        if !p.alive {
            return None;
        }
        let dst = self.bulbs[b].short;
        self.plug_route(k, dst);
        let (st, air) = self.s.w.st(p.id);
        let o = Tx::new().general().discover(1);
        let sent = st.tx_zcl(air, dst, 0x0006, 0x00, &[0x00, 0x00], &o);
        Some(self.push_msg(Flow::PlugReadBulb, p.short, dst, 0x0006, 0x00, sent.zcl_seq.unwrap_or(0), None))
    }

    /// Coordinator -> scripted router: On/Off with a Default Response.
    pub fn cmd_plug(&mut self, k: usize) -> usize {
        let dst = self.plugs[k].short;
        self.coord_route(dst);
        let cmd = (self.msgs.len() % 2) as u8;
        let seq = self.s.w.coord.zcl_cmd(&mut self.s.w.air, dst, 1, 0x0006, cmd, &[]);
        self.push_msg(Flow::CmdPlug, 0, dst, 0x0006, cmd, seq, None)
    }

    /// Coordinator -> group (NWK broadcast to rx-on-when-idle devices, APS group addressing).
    pub fn group_cmd(&mut self, cmd: u8) -> usize {
        let o = Tx::new().group(GROUP).radius(8);
        let sent = self.s.w.coord.tx_zcl(&mut self.s.w.air, 0xFFFD, 0x0006, cmd, &[], &o);
        let want = match cmd {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        };
        self.push_msg(Flow::Group, 0, GROUP, 0x0006, cmd, sent.zcl_seq.unwrap_or(0), want)
    }

    /// One message of the traffic mix, drawn from the traffic stream.
    pub fn random_msg(&mut self, mix: &[(Flow, u32)]) -> Option<usize> {
        let total: u32 = mix.iter().map(|x| x.1).sum();
        let mut r = self.traffic_rng.below(total);
        let mut flow = mix[0].0;
        for (f, w) in mix {
            if r < *w {
                flow = *f;
                break;
            }
            r -= w;
        }
        let nb = self.bulbs.len() as u32;
        let np = self.plugs.len() as u32;
        match flow {
            Flow::CmdBulb => {
                let b = self.traffic_rng.below(nb) as usize;
                let what = match self.traffic_rng.below(4) {
                    0 => 0,
                    1 => 1,
                    _ => 20 + self.traffic_rng.below(200) as u8,
                };
                Some(self.cmd_bulb(b, what))
            }
            Flow::ReadBulb => {
                let b = self.traffic_rng.below(nb) as usize;
                Some(self.read_bulb(b))
            }
            Flow::Report => {
                let k = self.traffic_rng.below(np) as usize;
                self.report(k)
            }
            Flow::PlugReadBulb => {
                let k = self.traffic_rng.below(np) as usize;
                let b = self.traffic_rng.below(nb) as usize;
                self.plug_read_bulb(k, b)
            }
            Flow::CmdPlug => {
                let k = self.traffic_rng.below(np) as usize;
                if !self.plugs[k].alive {
                    return None;
                }
                Some(self.cmd_plug(k))
            }
            Flow::Group => Some(self.group_cmd(2)),
        }
    }

    /// Runs `ms` with messages of `mix` at `rate` per second (exponential gaps from the traffic stream).
    pub fn traffic(&mut self, ms: f64, rate: f64, mix: &[(Flow, u32)]) -> R<()> {
        self.traffic_with(ms, rate, mix, &mut |_| Ok(()))
    }

    /// `traffic` with `hook` called after every step (jammers, probes, foreign traffic).
    pub fn traffic_with(&mut self, ms: f64, rate: f64, mix: &[(Flow, u32)], hook: &mut dyn FnMut(&mut Mesh) -> R<()>) -> R<()> {
        let end = self.now() + ms;
        let mut next = if rate > 0.0 { self.now() + self.gap(rate) } else { f64::MAX };
        while self.now() < end - 1e-9 {
            let t = end.min(next).min(self.now() + STEP);
            let d = t - self.now();
            if d > 1e-9 {
                self.tick(d)?;
            }
            hook(self)?;
            if self.now() + 1e-9 >= next && self.now() < end {
                self.random_msg(mix);
                next = self.now() + self.gap(rate);
            }
        }
        Ok(())
    }

    fn gap(&mut self, rate: f64) -> f64 {
        let u = self.traffic_rng.random();
        // at least a step apart
        (-(1.0 - u).ln() / rate * 1000.0).max(STEP)
    }

    /// Matches a decoded frame against the open messages.
    fn match_msgs(&mut self, d: &Dec) {
        let Some(n) = &d.nwk else { return };
        // Network Status to the coordinator: the source route to that destination is stale
        if n.ty == NWK_CMD && n.payload.first() == Some(&0x03) && n.dst == 0 && d.mac_dst == Some(0) && self.chan.got(d.idx, NodeId::Coord) {
            if let Some(b) = n.payload.get(2..4) {
                let about = u16::from_le_bytes([b[0], b[1]]);
                self.s.w.coord.source_routes.remove(&about);
            }
        }
        let (Some(z), Some(a)) = (&d.zcl, &d.aps) else { return };
        if a.ty != APS_DATA {
            return;
        }
        let rx_node = d.mac_dst.and_then(|m| if m == 0xFFFF { None } else { self.node_of_short(m) });
        // the last hop: MAC destination is the NWK destination, and it got the frame
        let last_hop = d.mac_dst == Some(n.dst) && rx_node.is_some_and(|r| self.chan.got(d.idx, r));
        if !last_hop {
            return;
        }
        let t = d.t1;
        let is_answer = z.to_client || (!z.specific && (z.cmd == 0x0B || z.cmd == 0x01));
        for i in (0..self.msgs.len()).rev() {
            let m = &self.msgs[i];
            if t - m.t > 60000.0 {
                break;
            }
            if m.t > d.t0 || m.seq != z.seq {
                continue;
            }
            if !is_answer && m.src == n.src && m.dst == n.dst && m.cluster == a.cluster && m.cmd == z.cmd {
                if self.msgs[i].delivered.is_none() {
                    self.msgs[i].delivered = Some(t);
                }
                break;
            }
            if is_answer && m.src == n.dst && m.dst == n.src && m.cluster == a.cluster {
                let mm = &mut self.msgs[i];
                if mm.answered.is_none() {
                    mm.answered = Some(t);
                }
                if !mm.answer_ctrs.contains(&a.counter) {
                    mm.answer_ctrs.push(a.counter);
                }
                break;
            }
        }
    }

    /// One line per decoded frame (debugging).
    pub fn brief(&self, d: &Dec) -> String {
        let who = d.who.map(|w| self.s.w.air.name_of(w)).unwrap_or_default();
        let nm = |s: Option<u16>| s.map(|x| self.node_of_short(x).map(|n| self.s.w.air.name_of(n)).unwrap_or(format!("{x:#06x}"))).unwrap_or("-".into());
        let mut t = format!("{:10.3} {:<9} mac {}->{} seq={}", d.t0, who, nm(d.mac_src), nm(d.mac_dst), d.mac_seq);
        if d.mac_ty == 2 {
            return format!("{:10.3} {:<9} ACK seq={}", d.t0, who, d.mac_seq);
        }
        if let Some(n) = &d.nwk {
            t += &format!(" nwk {}->{} r={} s={}", nm(Some(n.src)), nm(Some(n.dst)), n.radius, n.seq);
            if n.ty == NWK_CMD {
                t += &format!(" cmd {:02x?}", n.payload);
            }
        }
        if let Some(a) = &d.aps {
            t += &format!(" aps ty={} ctr={} cl={:#06x}", a.ty, a.counter, a.cluster);
        }
        if let Some(z) = &d.zcl {
            t += &format!(" zcl seq={} cmd={:#04x}{}", z.seq, z.cmd, if z.to_client { " rsp" } else { "" });
        }
        let rx: Vec<String> = self.s.w.air.nodes.iter().filter(|n| self.chan.got(d.idx, **n)).map(|n| self.s.w.air.name_of(*n)).collect();
        t + &format!("  got: {}", rx.join(","))
    }

    /// Failed messages of phase `p` with the frames around them (STRESS_DEBUG=1).
    pub fn debug_failed(&self, p: usize, max: usize) -> Vec<String> {
        let mut out = Vec::new();
        for m in self.msgs.iter().filter(|x| x.phase == p && !x.ok()).take(max) {
            out.push(format!("-- failed {:?} {:#06x}->{:#06x} seq={} at {:.3}", m.flow, m.src, m.dst, m.seq, m.t));
            for d in self.dec.iter().filter(|d| d.t0 >= m.t - 1.0 && d.t0 < m.t + 1500.0) {
                let rel = d.nwk.as_ref().is_some_and(|n| [n.src, n.dst].contains(&m.src) || [n.src, n.dst].contains(&m.dst));
                if rel || d.mac_ty == 2 {
                    out.push(format!("   {}", self.brief(d)));
                }
            }
        }
        out
    }

    // ------------------------------------------------------------------ churn
    /// A scripted router goes silent and deaf.
    pub fn plug_vanish(&mut self, k: usize) {
        let id = self.plugs[k].id;
        self.plugs[k].alive = false;
        let NodeId::Peer(pi) = id else { return };
        let ids: Vec<NodeId> = self.s.w.air.nodes.clone();
        let p = &mut self.s.w.peers[pi as usize];
        p.auto_ack = false;
        p.stop_link_status();
        for o in ids {
            if o != id {
                self.s.w.air.block(id, o, true);
            }
        }
    }

    /// It comes back (same address, same keys, counters going on).
    pub fn plug_return(&mut self, k: usize) {
        self.plugs[k].alive = true;
        let id = self.plugs[k].id;
        self.apply_ranges();
        let nb = self.plug_neighbours(k);
        let NodeId::Peer(pi) = id else { return };
        let w = &mut self.s.w;
        w.peers[pi as usize].auto_ack = true;
        w.peers[pi as usize].start_link_status(&mut w.air, 15000.0, Some(nb));
    }

    pub fn power_off(&mut self, id: NodeId) {
        self.s.w.power_off(id);
        if let Some(i) = self.bulb_ix(id) {
            self.last_on[i] = None;
        }
    }

    pub fn power_on(&mut self, id: NodeId) -> R<()> {
        self.s.w.power_on(id).map_err(emu)
    }

    pub fn resets(&self) -> u32 {
        self.bulbs.iter().map(|b| self.s.w.bulb_resets(b.id)).sum()
    }

    /// Short addresses in a bulb's neighbour table (Mgmt_Lqi, all pages); None: no answer.
    pub fn neighbours(&mut self, b: usize) -> R<Option<Vec<u16>>> {
        let short = self.bulbs[b].short;
        let (mut out, mut start) = (Vec::new(), 0u8);
        loop {
            self.coord_route(short);
            let n0 = self.s.w.coord.zcl_rsp.len();
            self.s.w.coord.zdo_send(&mut self.s.w.air, short, 0x0031, &[start]);
            let end = self.now() + 3000.0;
            let mut got = None;
            while self.now() < end {
                self.tick(10.0)?;
                if let Some(r) = self.s.w.coord.zcl_rsp[n0..].iter().find(|r| r.src == short && r.cluster == 0x8031) {
                    got = Some(r.body.clone());
                    break;
                }
            }
            let Some(p) = got else { return Ok(None) };
            if p.len() < 5 || p[1] != 0 {
                return Ok(None);
            }
            let (total, count) = (p[2], p[4]);
            for i in 0..count as usize {
                let o = 5 + 22 * i + 16;
                if let Some(b) = p.get(o..o + 2) {
                    out.push(u16::from_le_bytes([b[0], b[1]]));
                }
            }
            start = start.wrapping_add(count);
            if start >= total || count == 0 {
                return Ok(Some(out));
            }
        }
    }

    /// Adds every bulb to the group (Add Group, unicast with Default Response); how many confirmed.
    pub fn join_group(&mut self) -> R<usize> {
        let mut ok = 0;
        for b in 0..self.bulbs.len() {
            for _ in 0..3 {
                let dst = self.bulbs[b].short;
                self.coord_route(dst);
                let mut pl = GROUP.to_le_bytes().to_vec();
                pl.push(0);
                self.s.w.coord.zcl_cmd(&mut self.s.w.air, dst, 1, 0x0004, 0x00, &pl);
                let n0 = self.s.w.coord.zcl_rsp.len();
                let end = self.now() + 3000.0;
                let mut got = false;
                while self.now() < end {
                    self.tick(10.0)?;
                    if self.s.w.coord.zcl_rsp[n0..].iter().any(|r| r.src == dst && r.cluster == 0x0004) {
                        got = true;
                        break;
                    }
                }
                if got {
                    ok += 1;
                    break;
                }
            }
        }
        Ok(ok)
    }
}

/// Whether frame `e` shows relay `x` dealing with frame `n`: an attempt to
/// pass it on, or a Network Status about its source or destination.
fn handled(e: &Dec, x: NodeId, n: &NwkInfo) -> bool {
    e.who == Some(x)
        && e.nwk.as_ref().is_some_and(|m| {
            (m.src == n.src && m.seq == n.seq)
                || (m.ty == NWK_CMD
                    && m.payload.first() == Some(&0x03)
                    && (m.payload.get(2..4) == Some(&n.dst.to_le_bytes()[..]) || m.payload.get(2..4) == Some(&n.src.to_le_bytes()[..])))
        })
}

/// Bulb k's sources of randomness, each with its own seed: the random number
/// generator (the emulator's "healthy" source, register 0xfff4) and the ADC
/// noise (register 0xffe8), which Telink's driver seeds its generator from.
/// Identical machines would otherwise all draw the same numbers (the same
/// stochastic addresses and backoffs), which real bulbs do not.
fn seed_rbg(n: &mut zbnode::Node, k: usize) {
    let seed = 0x5EED00u32 + k as u32 * 0x101;
    let _ = n.m.reg_write(0xFFF4, 4, (seed << 8) | 1);
    let _ = n.m.reg_write(0xFFE8, 4, (seed.wrapping_mul(7) & 0xFFFFFF) << 8);
}

/// The varying bytes of bulb k's factory MAC (fixed per bulb, splitmix64 of k).
pub fn mac_bytes(k: usize) -> [u8; 5] {
    let mut z = (k as u64 + 1).wrapping_mul(0x9E3779B97F4A7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^= z >> 31;
    let b = z.to_le_bytes();
    [b[0], b[1], b[2], b[3], b[4]]
}

// ---------------------------------------------------------------------- statistics
pub fn pct(v: &[f64], p: f64) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    // nearest rank
    let k = ((p / 100.0) * s.len() as f64).ceil().max(1.0) as usize;
    Some(s[k.min(s.len()) - 1])
}
