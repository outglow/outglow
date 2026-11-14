//! The shared 802.15.4 air and the clock of a scenario, in-process.
//!
//! A port of emulator/radio154.py `Medium` together with the run loop of
//! emulator/engines.py `GoEngine.run_alone`: the emulated bulb (a
//! [`zbnode::Node`]) never runs past the next scheduled event of the other
//! nodes, it stops when it transmits, and frames of the other nodes reach its
//! radio before it gets there (conservative lockstep). Times cross between the
//! bulb and the rest with 6 decimals, as they do over the zbnode protocol, so
//! a scenario sees the same timing here as the Python suite does.

use std::collections::HashSet;
use std::rc::Rc;

use zbnode::{Frame, Node, RunEvent};

use crate::coord::{Coord, CoordEv};
use crate::frames::Keys;
use crate::peer::{Peer, PeerEv};
use crate::station::{Station, StationEv};

/// A node on the air. The bulb is attached first, then the coordinator: the
/// order in which a frame reaches its receivers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NodeId {
    Dut,
    Coord,
    /// a scripted peer: index into `World::peers`
    Peer(u8),
    /// another emulated bulb: index into `Air::bulbs` (the first bulb is `Dut`)
    Bulb(u8),
    /// a further coordinator: index into `World::coords`
    Coord2(u8),
}

impl NodeId {
    pub fn name(self) -> String {
        match self {
            NodeId::Dut => "dut".into(),
            NodeId::Coord => "coord".into(),
            NodeId::Peer(i) => format!("peer{i}"),
            NodeId::Bulb(i) => format!("bulb{i}"),
            NodeId::Coord2(i) => format!("coord{}", i + 2),
        }
    }
}

/// A frame as it went on the air.
#[derive(Clone, Debug)]
pub struct AirFrame {
    pub t0: f64,
    pub t1: f64,
    pub ch: i64,
    pub who: NodeId,
    pub psdu: Rc<Vec<u8>>,
}

/// What the channel does to one frame at one receiver (`Air::channel`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fate {
    /// received with this RSSI
    Ok(i64),
    /// never reaches the receiver (too weak, faded)
    Lost,
    /// reaches it with a bad FCS: an emulated bulb's radio gets it flagged as a
    /// CRC error (`Node::inject_rx_bad_fcs`), a scripted node's MAC drops it
    Corrupt(i64),
}

/// A fault model of the air (off unless a scenario installs one; with none
/// the air is the ideal one of `radio154.Medium`).
pub trait Channel {
    /// At the start of frame `idx` (index into `Air::frames`), for each node
    /// in range and powered: what happens to it at `rx`. `rssi` is what the
    /// air would give without the model.
    fn fate(&mut self, idx: usize, sender: NodeId, rx: NodeId, t0: f64, t1: f64, psdu: &[u8], rssi: i64) -> Fate;
    /// At the end of frame `idx`, for a scripted receiver that was not
    /// already Lost/Corrupt: whether it really got the frame (e.g. no other
    /// frame overlapped it there). Emulated bulbs decide that in their radio.
    fn heard(&mut self, idx: usize, rx: NodeId, frames: &[AirFrame]) -> bool;
}

/// Something that happens at a scheduled time.
pub enum Ev {
    /// The end of frame `idx` at its receivers (their `on_frame`).
    Deliver { idx: usize, rx: Vec<(NodeId, i64)>, psdu: Rc<Vec<u8>>, ch: i64, t0: f64, t1: f64 },
    Coord(CoordEv),
    /// The MAC of the coordinator or of a peer.
    Node(NodeId, StationEv),
    Peer(u8, PeerEv),
    /// an event of a further coordinator
    Coord2(u8, CoordEv),
}

/// A time as the zbnode protocol carries it: 6 decimals.
pub fn r6(x: f64) -> f64 {
    format!("{x:.6}").parse().unwrap()
}

pub type Log = Rc<dyn Fn(&str)>;

/// The ATT side of a bulb's BLE central (BLECONNECT/ATT of the zbnode
/// protocol, in-process): responses and notifications become visible as
/// zbnode prints them, before each step of the bulb (`flush_ble` in
/// zbnode's RUN), so a scenario sees them at the same moments the Python
/// scripts do.
pub struct AttTap {
    pub q: zbnode::bleatt::QueueRc,
    pub c: ble::CentralRc,
    rsp_sent: usize,
    ntf_sent: usize,
    /// (ms, response) of every connection of this bulb, in order (`GoEngine.att_rsps`)
    pub rsps: Vec<(f64, Vec<u8>)>,
    /// (ms, handle, value text with '_' for spaces) (`GoEngine.att_ntfs`)
    pub ntfs: Vec<(f64, i64, String)>,
}

impl AttTap {
    pub fn new(q: zbnode::bleatt::QueueRc, c: ble::CentralRc) -> AttTap {
        AttTap { q, c, rsp_sent: 0, ntf_sent: 0, rsps: Vec::new(), ntfs: Vec::new() }
    }

    /// A new central (BLECONNECT): new queue, counters from zero, lists kept.
    pub fn attach(&mut self, q: zbnode::bleatt::QueueRc, c: ble::CentralRc) {
        self.q = q;
        self.c = c;
        self.rsp_sent = 0;
        self.ntf_sent = 0;
    }

    pub fn sync(&mut self) {
        let q = self.q.borrow();
        while self.rsp_sent < q.rsps.len() {
            let x = &q.rsps[self.rsp_sent];
            self.rsps.push((r6(x.ms), x.data.clone()));
            self.rsp_sent += 1;
        }
        let c = self.c.borrow();
        while self.ntf_sent < c.ntf.len() {
            let n = &c.ntf[self.ntf_sent];
            self.ntfs.push((r6(n.ms), n.handle as i64, n.value.replace(' ', "_")));
            self.ntf_sent += 1;
        }
    }
}

/// BLE packets on the air between the bulbs (`ble_air.BleAir`).
#[derive(Default)]
pub struct BleAir {
    /// (t0, t1, channel, access address, sender, pdu)
    pub packets: Vec<(f64, f64, u8, u32, NodeId, Vec<u8>)>,
    pub blocked: HashSet<(NodeId, NodeId)>,
}

/// An emulated bulb other than the first: its own clock and power-on offset
/// (a further `GoEngine` on the same `Medium`, driven by `lockstep`).
pub struct Bulb {
    pub node: Node,
    pub name: String,
    /// its clock in air time (`GoEngine.t`)
    pub t: f64,
    /// air time at which it was last powered on (`GoEngine.t_off`)
    pub t_off: f64,
    pub resets: u32,
    /// its BLE central's ATT side, once a scenario connected one
    pub tap: Option<AttTap>,
}

pub struct Air {
    /// The scenario clock (`GoEngine.t`, what `Medium.now()` returns).
    pub t: f64,
    /// Air time at which the bulb was last powered on: its own clock starts
    /// from zero after a reboot (`GoEngine.t_off`).
    pub t_off: f64,
    sched: Vec<(f64, u64, Ev)>,
    seq: u64,
    /// Every frame put on the air, in order.
    pub frames: Vec<AirFrame>,
    /// (sender, receiver) pairs out of each other's range.
    pub blocked: HashSet<(NodeId, NodeId)>,
    pub nodes: Vec<NodeId>,
    /// names of the peers, by index
    pub names: Vec<String>,
    /// The key material of the sniffer (the session) and of the coordinator.
    pub keys: Keys,
    pub dut: Node,
    /// the first bulb's name on the air ("dut" unless a scenario names it)
    pub dut_name: String,
    /// further bulbs; the first one (`dut`) owns the schedule, as the first
    /// GoEngine attached to a Medium does
    pub bulbs: Vec<Bulb>,
    pub log: Option<Log>,
    /// `Medium.rssi_fn`: the RSSI a receiver gets from a sender at t0 (None: the fixed value)
    pub rssi_fn: Option<Box<dyn FnMut(NodeId, NodeId, f64) -> i64>>,
    /// every frame put on the air also goes here (`Medium(pcap=...)`)
    pub pcap: Option<crate::pcap::Pcap>,
    /// the first bulb's BLE central's ATT side, once a scenario connected one
    pub dut_tap: Option<AttTap>,
    /// BLE packets delivered between the bulbs (None: BLE packets go nowhere)
    pub ble_air: Option<BleAir>,
    /// log resets as `GoEngine.run_alone` prints them (for scripts that print its log)
    pub py_log: bool,
    /// BLE packets the first bulb transmitted (GoEngine's BLETX lines)
    pub ble_tx: u64,
    /// a fault model (None: the ideal air)
    pub channel: Option<Box<dyn Channel>>,
    /// bulbs without power (`World::power_off`): they do not run, send or receive
    pub off: HashSet<NodeId>,
}

impl Air {
    pub fn now(&self) -> f64 {
        self.t
    }

    /// Schedules `ev` at `ms`; events at the same time run in the order they were scheduled.
    pub fn at(&mut self, ms: f64, ev: Ev) {
        self.seq += 1;
        let pos = self.sched.partition_point(|e| e.0 <= ms);
        self.sched.insert(pos, (ms, self.seq, ev));
    }

    pub fn next_event(&self) -> Option<f64> {
        self.sched.first().map(|e| e.0)
    }

    fn pop_due(&mut self) -> Option<Ev> {
        match self.sched.first() {
            Some(e) if e.0 <= self.t => Some(self.sched.remove(0).2),
            _ => None,
        }
    }

    /// A frame from `sender`: the receivers hear its start now (the bulb's
    /// radio gets it), and its end at `t1`.
    pub fn transmit(&mut self, sender: NodeId, psdu: Vec<u8>, ch: i64, t0: f64, t1: f64, rssi: i64) {
        let psdu = Rc::new(psdu);
        self.frames.push(AirFrame { t0, t1, ch, who: sender, psdu: psdu.clone() });
        if let Some(p) = &mut self.pcap {
            let _ = p.write(t0, &psdu);
        }
        let recv: Vec<NodeId> = self
            .nodes
            .iter()
            .copied()
            .filter(|n| *n != sender && !self.blocked.contains(&(sender, *n)) && !self.off.contains(n))
            .collect();
        let rx: Vec<(NodeId, i64)> = match &mut self.rssi_fn {
            Some(f) => recv.into_iter().map(|n| (n, f(sender, n, t0))).collect(),
            None => recv.into_iter().map(|n| (n, rssi)).collect(),
        };
        let idx = self.frames.len() - 1;
        // with a fault model: what each receiver gets (a frame whose FCS is wrong is corrupt everywhere)
        let rx: Vec<(NodeId, i64, bool)> = match &mut self.channel {
            None => rx.into_iter().map(|(n, r)| (n, r, false)).collect(),
            Some(c) => {
                let fcs_ok = psdu.len() >= 2 && {
                    let n = psdu.len() - 2;
                    crate::frames::crc16_kermit(&psdu[..n]).to_le_bytes() == psdu[n..]
                };
                rx.into_iter()
                    .filter_map(|(n, r)| match c.fate(idx, sender, n, t0, t1, &psdu, r) {
                        Fate::Ok(r) if fcs_ok => Some((n, r, false)),
                        Fate::Ok(r) | Fate::Corrupt(r) => Some((n, r, true)),
                        Fate::Lost => None,
                    })
                    .collect()
            }
        };
        for (n, r, bad) in &rx {
            // a bulb's radio: not before it was powered on, in its own time
            let (node, off) = match *n {
                NodeId::Dut if t0 >= self.t_off => (&self.dut, self.t_off),
                NodeId::Bulb(i) if t0 >= self.bulbs[i as usize].t_off => {
                    let b = &self.bulbs[i as usize];
                    (&b.node, b.t_off)
                }
                _ => continue,
            };
            let mut p = (*psdu).clone();
            if *bad {
                // what the radio demodulates of a corrupted frame: some bits flipped
                if let Some(x) = p.get_mut(psdu.len() / 2) {
                    *x ^= 0x5A;
                }
                node.inject_rx_bad_fcs(Frame { t0: r6(t0 - off), t1: r6(t1 - off), channel: ch, psdu: p }, *r);
            } else {
                node.inject_rx(Frame { t0: r6(t0 - off), t1: r6(t1 - off), channel: ch, psdu: p }, *r);
            }
        }
        // a scripted node's MAC drops a corrupted frame
        let rx: Vec<(NodeId, i64)> = rx.into_iter().filter(|x| !x.2).map(|(n, r, _)| (n, r)).collect();
        self.at(t1, Ev::Deliver { idx, rx, psdu, ch, t0, t1 });
    }

    /// End of the frames in the air as far as is known now, plus an expected ACK
    /// (`Session.air_free_at`: carrier sense for the scripted nodes).
    pub fn air_free_at(&self) -> f64 {
        let now = self.t;
        let mut free: f64 = 0.0;
        let n = self.frames.len();
        for f in &self.frames[n.saturating_sub(6)..] {
            if f.t1 + 1.0 < now {
                continue;
            }
            let ack_follows = f.psdu.len() > 5 && f.psdu[0] & 0x20 != 0;
            free = free.max(f.t1 + if ack_follows { 0.9 } else { 0.0 });
        }
        free
    }

    /// Nodes `a` and `b` stop (or start again) hearing each other.
    pub fn block(&mut self, a: NodeId, b: NodeId, blocked: bool) {
        for pair in [(a, b), (b, a)] {
            if blocked {
                self.blocked.insert(pair);
            } else {
                self.blocked.remove(&pair);
            }
        }
    }

    pub fn name_of(&self, id: NodeId) -> String {
        match id {
            NodeId::Peer(i) => self.names[i as usize].clone(),
            NodeId::Bulb(i) => self.bulbs[i as usize].name.clone(),
            NodeId::Dut => self.dut_name.clone(),
            _ => id.name(),
        }
    }

    /// A BLE packet from a bulb (`ble_air.BleAir.transmit`): the other bulbs in
    /// range get it (BLERX, in their own time).
    pub fn ble_transmit(&mut self, sender: NodeId, t0: f64, t1: f64, ch: u8, aa: u32, pdu: Vec<u8>) {
        let Some(ba) = &mut self.ble_air else { return };
        ba.packets.push((t0, t1, ch, aa, sender, pdu.clone()));
        let rx: Vec<NodeId> = std::iter::once(NodeId::Dut)
            .chain((0..self.bulbs.len()).map(|i| NodeId::Bulb(i as u8)))
            .filter(|n| *n != sender && !ba.blocked.contains(&(sender, *n)))
            .collect();
        for n in rx {
            let (node, off) = match n {
                NodeId::Dut => (&self.dut, self.t_off),
                NodeId::Bulb(i) => (&self.bulbs[i as usize].node, self.bulbs[i as usize].t_off),
                _ => continue,
            };
            node.ble_radio.air_rx(&node.m, pdu.clone(), ch, aa, r6(t0 - off), r6(t1 - off));
        }
    }

    pub fn log(&self, s: &str) {
        if let Some(l) = &self.log {
            l(s);
        }
    }
}

/// The bulb, the air and the coordinator.
pub struct World {
    pub air: Air,
    pub coord: Coord,
    pub peers: Vec<Peer>,
    /// further coordinators (`World::add_coord`)
    pub coords: Vec<Coord>,
    /// Watchdog and software resets of the bulb seen by the run loop.
    pub resets: u32,
}

impl World {
    pub fn new(dut: Node, coord: Coord, keys: Keys, log: Option<Log>) -> World {
        let air = Air {
            t: 0.0,
            t_off: 0.0,
            sched: Vec::new(),
            seq: 0,
            frames: Vec::new(),
            blocked: HashSet::new(),
            nodes: vec![NodeId::Dut, NodeId::Coord],
            names: Vec::new(),
            dut_name: "dut".into(),
            bulbs: Vec::new(),
            rssi_fn: None,
            pcap: None,
            dut_tap: None,
            ble_air: None,
            py_log: false,
            ble_tx: 0,
            channel: None,
            off: HashSet::new(),
            keys,
            dut,
            log,
        };
        World { air, coord, peers: Vec::new(), coords: Vec::new(), resets: 0 }
    }

    pub fn now(&self) -> f64 {
        self.air.t
    }

    /// Power loss and return (`GoEngine.reboot`): the bulb's clock starts again from zero.
    pub fn reboot(&mut self) -> Result<(), String> {
        self.air.dut.reboot()?;
        self.air.t_off = self.air.t;
        Ok(())
    }

    /// A reset without power loss (`GoEngine.soft_reset`).
    pub fn soft_reset(&mut self, watchdog: bool) -> Result<(), String> {
        self.air.dut.soft_reset(watchdog)?;
        self.air.t_off = self.air.t;
        Ok(())
    }

    /// The MAC of a node other than the bulb, and the air.
    pub fn st(&mut self, id: NodeId) -> (&mut Station, &mut Air) {
        let World { air, coord, peers, coords, .. } = self;
        match id {
            NodeId::Coord => (&mut coord.st, air),
            NodeId::Coord2(i) => (&mut coords[i as usize].st, air),
            NodeId::Peer(i) => (&mut peers[i as usize].st, air),
            NodeId::Dut | NodeId::Bulb(_) => panic!("a bulb has no scripted station"),
        }
    }

    fn dispatch(&mut self, ev: Ev) {
        match ev {
            Ev::Deliver { idx, rx, psdu, ch, t0, t1 } => {
                for (n, rssi) in rx {
                    if let Some(c) = &mut self.air.channel {
                        if !matches!(n, NodeId::Dut | NodeId::Bulb(_)) && !c.heard(idx, n, &self.air.frames) {
                            continue;
                        }
                    }
                    match n {
                        NodeId::Coord => self.coord.on_frame(&mut self.air, &psdu, ch, t0, t1, rssi),
                        NodeId::Peer(i) => self.peers[i as usize].on_frame(&mut self.air, &psdu, ch, t1),
                        NodeId::Coord2(i) => self.coords[i as usize].on_frame(&mut self.air, &psdu, ch, t0, t1, rssi),
                        NodeId::Dut | NodeId::Bulb(_) => {}
                    }
                }
            }
            Ev::Coord(e) => self.coord.on_event(&mut self.air, e),
            Ev::Node(id, e) => {
                let (st, air) = self.st(id);
                st.on_event(air, e);
            }
            Ev::Peer(i, e) => self.peers[i as usize].on_peer_event(&mut self.air, e),
            Ev::Coord2(i, e) => self.coords[i as usize].on_event(&mut self.air, e),
        }
    }

    /// Runs the scenario to `target` ms (`GoEngine.run_alone`).
    pub fn run_ms(&mut self, target: f64) -> Result<(), String> {
        loop {
            while let Some(ev) = self.air.pop_due() {
                self.dispatch(ev);
            }
            if self.air.t >= target {
                return Ok(());
            }
            let nxt = match self.air.next_event() {
                Some(ms) => target.min(ms),
                None => target,
            };
            let off = self.air.t_off;
            let run_to = r6(nxt - off);
            if self.air.off.contains(&NodeId::Dut) {
                // no power: its clock stands still, air time goes on
                self.air.t = self.air.t.max(nxt);
                continue;
            }
            loop {
                if let Some(t) = &mut self.air.dut_tap {
                    t.sync();
                }
                match self.air.dut.step_run(run_to) {
                    RunEvent::Tx(fr) => {
                        let (t0, t1) = (r6(fr.t0) + off, r6(fr.t1) + off);
                        self.air.t = t0;
                        self.air.transmit(NodeId::Dut, fr.psdu, fr.channel, t0, t1, -40);
                        break;
                    }
                    RunEvent::BleTx(a) => {
                        // without a BLE air the packet goes nowhere
                        self.air.t = r6(a.t0) + off;
                        self.air.ble_tx += 1;
                        self.air.ble_transmit(NodeId::Dut, r6(a.t0) + off, r6(a.t1) + off, a.ch, a.aa, a.pdu);
                        break;
                    }
                    RunEvent::At => {
                        self.air.t = self.air.t.max(r6(self.air.dut.ms()) + off).max(nxt);
                        break;
                    }
                    RunEvent::Ran => {}
                    RunEvent::SramBoot { pc: _, ram_code } => {
                        // GoEngine's log line (node time, as zbnode reports it)
                        let ms = r6(self.air.dut.ms());
                        self.air.log(&format!("[{ms:9.3} ms] bootloader -> application (start from SRAM, ram_code={ram_code:#x})"));
                    }
                    RunEvent::Reset { cause, pc } => {
                        self.resets += 1;
                        let ms = r6(self.air.dut.ms());
                        let slot = self.air.dut.m.boot_slot;
                        self.air.log(&format!("[{ms:9.3} ms] RESET firmware: {} {pc:#x} @ slot={slot:#x}", cause.replace(' ', "_")));
                    }
                    RunEvent::Error(e) => return Err(format!("emulation error: {e} pc={:#x}", self.air.dut.m.r[15])),
                    RunEvent::Fatal(e) => return Err(format!("emulator: {e}")),
                }
            }
        }
    }

    // ---------------------------------------------------------- more bulbs
    /// Attaches another emulated bulb (`GoEngine(...)` on the same Medium).
    pub fn add_bulb(&mut self, node: Node, name: &str) -> NodeId {
        let id = NodeId::Bulb(self.air.bulbs.len() as u8);
        self.air.bulbs.push(Bulb { node, name: name.to_string(), t: 0.0, t_off: 0.0, resets: 0, tap: None });
        self.air.nodes.push(id);
        id
    }

    /// A bulb's emulator node.
    pub fn node(&self, id: NodeId) -> &Node {
        match id {
            NodeId::Dut => &self.air.dut,
            NodeId::Bulb(i) => &self.air.bulbs[i as usize].node,
            _ => panic!("not a bulb: {id:?}"),
        }
    }

    pub fn node_mut(&mut self, id: NodeId) -> &mut Node {
        match id {
            NodeId::Dut => &mut self.air.dut,
            NodeId::Bulb(i) => &mut self.air.bulbs[i as usize].node,
            _ => panic!("not a bulb: {id:?}"),
        }
    }

    /// A bulb's clock (`GoEngine.t`).
    pub fn bulb_t(&self, id: NodeId) -> f64 {
        match id {
            NodeId::Dut => self.air.t,
            NodeId::Bulb(i) => self.air.bulbs[i as usize].t,
            _ => panic!("not a bulb: {id:?}"),
        }
    }

    pub fn set_t_off(&mut self, id: NodeId, t_off: f64) {
        match id {
            NodeId::Dut => self.air.t_off = t_off,
            NodeId::Bulb(i) => self.air.bulbs[i as usize].t_off = t_off,
            _ => panic!("not a bulb: {id:?}"),
        }
    }

    pub fn bulb_resets(&self, id: NodeId) -> u32 {
        match id {
            NodeId::Dut => self.resets,
            NodeId::Bulb(i) => self.air.bulbs[i as usize].resets,
            _ => panic!("not a bulb: {id:?}"),
        }
    }

    /// Power loss and return of any bulb.
    pub fn reboot_bulb(&mut self, id: NodeId) -> Result<(), String> {
        match id {
            NodeId::Dut => self.reboot(),
            NodeId::Bulb(i) => {
                let b = &mut self.air.bulbs[i as usize];
                b.node.reboot()?;
                b.t_off = b.t;
                Ok(())
            }
            _ => panic!("not a bulb: {id:?}"),
        }
    }

    /// Power off a bulb (a wall switch): it stops running, sending and
    /// receiving until `power_on`.
    pub fn power_off(&mut self, id: NodeId) {
        self.air.off.insert(id);
    }

    /// Power back on: the bulb boots from scratch (`reboot_bulb`) at the current air time.
    pub fn power_on(&mut self, id: NodeId) -> Result<(), String> {
        if !self.air.off.remove(&id) {
            return Ok(());
        }
        if let NodeId::Bulb(i) = id {
            let t = self.air.t.max(self.air.bulbs[i as usize].t);
            self.air.bulbs[i as usize].t = t;
        }
        self.reboot_bulb(id)
    }

    /// `GoEngine.run_alone` of a bulb that does not own the schedule: it runs
    /// to `target` (its own schedule is empty), stopping when it transmits.
    pub fn run_bulb(&mut self, i: usize, target: f64) -> Result<(), String> {
        loop {
            let (t, off) = (self.air.bulbs[i].t, self.air.bulbs[i].t_off);
            if t >= target {
                return Ok(());
            }
            let nxt = target;
            let run_to = r6(nxt - off);
            if self.air.off.contains(&NodeId::Bulb(i as u8)) {
                self.air.bulbs[i].t = nxt;
                return Ok(());
            }
            loop {
                if let Some(t) = &mut self.air.bulbs[i].tap {
                    t.sync();
                }
                let ev = self.air.bulbs[i].node.step_run(run_to);
                match ev {
                    RunEvent::Tx(fr) => {
                        let (t0, t1) = (r6(fr.t0) + off, r6(fr.t1) + off);
                        self.air.bulbs[i].t = t0;
                        self.air.transmit(NodeId::Bulb(i as u8), fr.psdu, fr.channel, t0, t1, -40);
                        break;
                    }
                    RunEvent::BleTx(a) => {
                        self.air.bulbs[i].t = r6(a.t0) + off;
                        self.air.ble_transmit(NodeId::Bulb(i as u8), r6(a.t0) + off, r6(a.t1) + off, a.ch, a.aa, a.pdu);
                        break;
                    }
                    RunEvent::At => {
                        let b = &mut self.air.bulbs[i];
                        b.t = b.t.max(r6(b.node.ms()) + off).max(nxt);
                        break;
                    }
                    RunEvent::Ran => {}
                    RunEvent::SramBoot { pc: _, ram_code } => {
                        let ms = r6(self.air.bulbs[i].node.ms());
                        self.air.log(&format!("[{ms:9.3} ms] bootloader -> application (start from SRAM, ram_code={ram_code:#x})"));
                    }
                    RunEvent::Reset { cause, pc } => {
                        self.air.bulbs[i].resets += 1;
                        let ms = r6(self.air.bulbs[i].node.ms());
                        let slot = self.air.bulbs[i].node.m.boot_slot;
                        self.air.log(&format!("[{ms:9.3} ms] RESET firmware: {} {pc:#x} @ slot={slot:#x}", cause.replace(' ', "_")));
                    }
                    RunEvent::Error(e) => {
                        let pc = self.air.bulbs[i].node.m.r[15];
                        return Err(format!("emulation error ({}): {e} pc={pc:#x}", self.air.bulbs[i].name));
                    }
                    RunEvent::Fatal(e) => return Err(format!("emulator ({}): {e}", self.air.bulbs[i].name)),
                }
            }
        }
    }

    /// `ble_air.lockstep`: all bulbs advance to `target` alternately in quanta,
    /// the schedule's owner (the first bulb) first.
    pub fn lockstep(&mut self, target: f64, quantum: f64) -> Result<(), String> {
        loop {
            let now = self.air.bulbs.iter().map(|b| b.t).fold(self.air.t, f64::min);
            if now >= target {
                return Ok(());
            }
            let step = target.min(now + quantum);
            if self.air.t < step {
                self.run_ms(step)?;
            }
            for i in 0..self.air.bulbs.len() {
                if self.air.bulbs[i].t < step {
                    self.run_bulb(i, step)?;
                }
            }
        }
    }

    /// The earliest clock of the bulbs (`min(n.t for n in nodes)`).
    pub fn bulbs_now(&self) -> f64 {
        self.air.bulbs.iter().map(|b| b.t).fold(self.air.t, f64::min)
    }

    /// Attaches a scripted node built by the caller (`make` gets its NodeId).
    pub fn attach_peer(&mut self, make: impl FnOnce(NodeId) -> Peer) -> NodeId {
        let id = NodeId::Peer(self.peers.len() as u8);
        let p = make(id);
        self.air.names.push(p.name.clone());
        self.peers.push(p);
        self.air.nodes.push(id);
        id
    }

    /// Attaches a further coordinator (a second `Coordinator` on the same Medium);
    /// it is not started (no Link Status) unless the caller starts it.
    pub fn add_coord(&mut self, mut c: Coord) -> NodeId {
        let id = NodeId::Coord2(self.coords.len() as u8);
        c.st.id = id;
        self.coords.push(c);
        self.air.nodes.push(id);
        id
    }

    /// Attaches a scripted node built by the caller (`Peer::bare_ack`); its station
    /// gets the node's id. Returns the id.
    pub fn add_peer_node(&mut self, mut p: Peer) -> NodeId {
        let id = NodeId::Peer(self.peers.len() as u8);
        p.st.id = id;
        self.air.names.push(p.name.clone());
        self.peers.push(p);
        self.air.nodes.push(id);
        id
    }

    /// `ble_air.lockstep([first] + peers, target)`: as `lockstep`, the bulbs taken
    /// in that order (the one whose `run_ms` was called first; the first bulb
    /// still owns the schedule).
    pub fn lockstep_from(&mut self, first: NodeId, target: f64, quantum: f64) -> Result<(), String> {
        let mut order = vec![first];
        for id in std::iter::once(NodeId::Dut).chain((0..self.air.bulbs.len()).map(|i| NodeId::Bulb(i as u8))) {
            if id != first {
                order.push(id);
            }
        }
        loop {
            let now = self.bulbs_now();
            if now >= target {
                return Ok(());
            }
            let step = target.min(now + quantum);
            for id in &order {
                if self.bulb_t(*id) < step {
                    match id {
                        NodeId::Dut => self.run_ms(step)?,
                        NodeId::Bulb(i) => self.run_bulb(*i as usize, step)?,
                        _ => {}
                    }
                }
            }
        }
    }

    /// The ATT tap of a bulb.
    pub fn tap_mut(&mut self, id: NodeId) -> &mut Option<AttTap> {
        match id {
            NodeId::Dut => &mut self.air.dut_tap,
            NodeId::Bulb(i) => &mut self.air.bulbs[i as usize].tap,
            _ => panic!("not a bulb: {id:?}"),
        }
    }

    pub fn tap(&self, id: NodeId) -> Option<&AttTap> {
        match id {
            NodeId::Dut => self.air.dut_tap.as_ref(),
            NodeId::Bulb(i) => self.air.bulbs[i as usize].tap.as_ref(),
            _ => panic!("not a bulb: {id:?}"),
        }
    }
}
