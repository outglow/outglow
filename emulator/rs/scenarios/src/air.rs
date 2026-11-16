//! The shared 802.15.4 air and the clock of a scenario, in-process.
//!
//! A port of emulator/radio154.py `Medium` together with the run loop of
//! emulator/engines.py `GoEngine.run_alone`: the emulated bulb (a
//! [`zbnode::Node`]) never runs past the next scheduled event of the other
//! nodes, it stops when it transmits, and frames of the other nodes reach its
//! radio before it gets there (conservative lockstep). Times cross between the
//! bulb and the rest with 6 decimals, as they do over the zbnode protocol, so
//! a scenario sees the same timing here as the Python suite does.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use zbnode::{Frame, Node, RunEvent};

use crate::coord::{Coord, CoordEv};
use crate::frames::Keys;
use crate::par::{BulbPtr, Job, Phase, Pool};
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
    /// A scripted node's broadcast again (`World::ble_retry`).
    Again { sender: NodeId, psdu: Rc<Vec<u8>>, ch: i64, dur: f64, rssi: i64 },
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

/// What reaches a further bulb while it runs on a worker thread of the
/// parallel lockstep: kept until the phase is over (`World::drain_lent`).
enum Deferred {
    /// an 802.15.4 frame, in air time
    Rx { bulb: u8, t0: f64, t1: f64, ch: i64, psdu: Vec<u8>, rssi: i64, bad: bool },
    /// a BLE packet, in air time
    Ble { bulb: u8, t0: f64, t1: f64, ch: u8, aa: u32, pdu: Vec<u8> },
}

/// The further bulbs are out on the worker threads (`World::lockstep` in
/// parallel): their number, and what the air sent them meanwhile.
struct Lent {
    n: usize,
    q: Vec<Deferred>,
}

/// An 802.15.4 frame reaching a bulb's radio (in air time; `off`: the air
/// time of the bulb's power-on).
#[allow(clippy::too_many_arguments)]
fn inject_154(node: &Node, off: f64, psdu: &[u8], ch: i64, t0: f64, t1: f64, rssi: i64, bad: bool) {
    let mut p = psdu.to_vec();
    if bad {
        // what the radio demodulates of a corrupted frame: some bits flipped
        if let Some(x) = p.get_mut(psdu.len() / 2) {
            *x ^= 0x5A;
        }
        node.inject_rx_bad_fcs(Frame { t0: r6(t0 - off), t1: r6(t1 - off), channel: ch, psdu: p }, rssi);
    } else {
        node.inject_rx(Frame { t0: r6(t0 - off), t1: r6(t1 - off), channel: ch, psdu: p }, rssi);
    }
}

/// What a bulb that runs on its own (`bulb_next`) put out, for the air.
pub enum BulbOut {
    /// an 802.15.4 frame, air time
    Tx { t0: f64, t1: f64, ch: i64, psdu: Vec<u8> },
    /// a BLE packet, air time
    Ble { t0: f64, t1: f64, ch: u8, aa: u32, pdu: Vec<u8> },
    /// a line for the scenario's log
    Log(String),
}

/// `GoEngine.run_alone` of a bulb that does not own the schedule, up to its
/// next output: runs it towards `target` (air time) and returns the first
/// frame, packet or log line it puts out, or None at `target`. Touches
/// nothing but the bulb. With `commit`, it also stops (None) as soon as its
/// radio no longer waits in TX mode (`Node::tx_committed`).
pub(crate) fn bulb_next(b: &mut Bulb, target: f64, powered: bool, commit: bool) -> Result<Option<BulbOut>, String> {
    loop {
        let (t, off) = (b.t, b.t_off);
        if t >= target {
            return Ok(None);
        }
        let run_to = r6(target - off);
        if !powered {
            b.t = target;
            return Ok(None);
        }
        loop {
            if commit && !b.node.tx_committed_or_started() {
                b.t = b.t.max(r6(b.node.ms()) + off);
                return Ok(None);
            }
            if let Some(t) = &mut b.tap {
                t.sync();
            }
            match b.node.step_run(run_to) {
                RunEvent::Tx(fr) => {
                    let (t0, t1) = (r6(fr.t0) + off, r6(fr.t1) + off);
                    b.t = t0;
                    return Ok(Some(BulbOut::Tx { t0, t1, ch: fr.channel, psdu: fr.psdu }));
                }
                RunEvent::BleTx(a) => {
                    b.t = r6(a.t0) + off;
                    return Ok(Some(BulbOut::Ble { t0: r6(a.t0) + off, t1: r6(a.t1) + off, ch: a.ch, aa: a.aa, pdu: a.pdu }));
                }
                RunEvent::At => {
                    b.t = b.t.max(r6(b.node.ms()) + off).max(target);
                    break;
                }
                RunEvent::Ran => {}
                RunEvent::SramBoot { pc: _, ram_code } => {
                    let ms = r6(b.node.ms());
                    return Ok(Some(BulbOut::Log(format!(
                        "[{ms:9.3} ms] bootloader -> application (start from SRAM, ram_code={ram_code:#x})"
                    ))));
                }
                RunEvent::Reset { cause, pc } => {
                    b.resets += 1;
                    let ms = r6(b.node.ms());
                    let slot = b.node.m.boot_slot;
                    return Ok(Some(BulbOut::Log(format!(
                        "[{ms:9.3} ms] RESET firmware: {} {pc:#x} @ slot={slot:#x}",
                        cause.replace(' ', "_")
                    ))));
                }
                RunEvent::Error(e) => {
                    let pc = b.node.m.r[15];
                    return Err(format!("emulation error ({}): {e} pc={pc:#x}", b.name));
                }
                RunEvent::Fatal(e) => return Err(format!("emulator ({}): {e}", b.name)),
            }
        }
    }
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
    /// (parallel lockstep) the further bulbs are out on worker threads and
    /// `bulbs` is empty: what the air sends them waits here
    lent: Option<Lent>,
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
        for &(n, r, bad) in &rx {
            // a bulb's radio: not before it was powered on, in its own time
            match n {
                NodeId::Dut if t0 >= self.t_off => inject_154(&self.dut, self.t_off, &psdu, ch, t0, t1, r, bad),
                NodeId::Bulb(i) => match &mut self.lent {
                    Some(l) => l.q.push(Deferred::Rx { bulb: i, t0, t1, ch, psdu: (*psdu).clone(), rssi: r, bad }),
                    None => {
                        let b = &self.bulbs[i as usize];
                        if t0 >= b.t_off {
                            inject_154(&b.node, b.t_off, &psdu, ch, t0, t1, r, bad);
                        }
                    }
                },
                _ => {}
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
        let n_bulbs = self.lent.as_ref().map_or(self.bulbs.len(), |l| l.n);
        let rx: Vec<NodeId> = std::iter::once(NodeId::Dut)
            .chain((0..n_bulbs).map(|i| NodeId::Bulb(i as u8)))
            .filter(|n| *n != sender && !ba.blocked.contains(&(sender, *n)))
            .collect();
        for n in rx {
            match n {
                NodeId::Dut => self.dut.ble_radio.air_rx(&self.dut.m, pdu.clone(), ch, aa, r6(t0 - self.t_off), r6(t1 - self.t_off)),
                NodeId::Bulb(i) => match &mut self.lent {
                    Some(l) => l.q.push(Deferred::Ble { bulb: i, t0, t1, ch, aa, pdu: pdu.clone() }),
                    None => {
                        let b = &self.bulbs[i as usize];
                        b.node.ble_radio.air_rx(&b.node.m, pdu.clone(), ch, aa, r6(t0 - b.t_off), r6(t1 - b.t_off));
                    }
                },
                _ => {}
            }
        }
    }

    /// What a bulb that ran on its own put out goes on the air.
    fn put_out(&mut self, id: NodeId, o: BulbOut) {
        match o {
            BulbOut::Tx { t0, t1, ch, psdu } => self.transmit(id, psdu, ch, t0, t1, -40),
            BulbOut::Ble { t0, t1, ch, aa, pdu } => self.ble_transmit(id, t0, t1, ch, aa, pdu),
            BulbOut::Log(s) => self.log(&s),
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
    /// how `lockstep` runs the bulbs (`ZB_PARALLEL`...)
    pub par: ParOptions,
    /// the worker threads of the parallel lockstep, once it ran
    pool: Option<Pool>,
    pub par_stats: ParStats,
    /// A broadcast of a scripted node (coordinator, peer) that began while the
    /// bulb's radio was lent to a BLE event goes again 5 ms later, at most
    /// three times: the broadcast retry of R22 3.6.5 (a sender that does not
    /// hear its neighbour pass the frame on sends it again), limited to the
    /// copies the bulb could not have heard (its receive log says so). A
    /// frame the bulb heard and ignored is never sent again. Off by default;
    /// the conformance sessions turn it on (with `dut_rx_log`).
    pub ble_retry: bool,
    ble_again: HashMap<Vec<u8>, u8>,
}

/// How `World::lockstep` advances the bulbs.
///
/// Sequential (the default): the first bulb and the scripted nodes, then
/// each further bulb in turn, each to the end of the quantum; a bulb sees
/// what an earlier one sent in the same quantum, an earlier one does not
/// see a later one's until the next.
///
/// Parallel (`ZB_PARALLEL=1`, with at least two bulbs): all bulbs run the
/// quantum at the same time, the further ones on worker threads; the scripted
/// nodes stay with the first bulb on the scenario's thread. What they put on
/// the air reaches the others at the end of the quantum, in a fixed order
/// (the scenario's thread first, then the bulbs by index, each in time
/// order), so the run does not depend on the threads' timing. With the
/// lookahead (`ZB_PARALLEL_LOOKAHEAD`, on unless 0), every bulb whose radio
/// waits in TX mode at the start of a quantum is first run alone until its
/// frame starts: the drivers spend the synthesizer's settling time in TX
/// mode, deaf, before a frame (at least 119.6 µs over a thousand frames of
/// the stock and the Rust firmware, `ZB_TX_LEADS`), so with a quantum no longer
/// than that, every frame a bulb starts in a quantum is known before the
/// others run it, and they hear it from its first bit. The quantum is the
/// scenario's, at most `ZB_PARALLEL_QUANTUM` ms (default 0.1).
#[derive(Clone, Copy, Debug)]
pub struct ParOptions {
    pub parallel: bool,
    pub lookahead: bool,
    pub max_quantum: f64,
}

impl ParOptions {
    pub fn from_env() -> ParOptions {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        ParOptions {
            parallel: var("ZB_PARALLEL").is_some_and(|v| v != "0"),
            lookahead: var("ZB_PARALLEL_LOOKAHEAD").map_or(true, |v| v != "0"),
            max_quantum: var("ZB_PARALLEL_QUANTUM").and_then(|v| v.parse().ok()).unwrap_or(0.1),
        }
    }
}

/// Counts of the parallel lockstep (`ZB_PARALLEL_STATS=1` prints them when
/// the world goes).
#[derive(Clone, Copy, Debug, Default)]
pub struct ParStats {
    pub quanta: u64,
    /// quanta with a lookahead phase of the further bulbs / of the first bulb
    pub commit_phases: u64,
    pub dut_commits: u64,
    /// frames started in the lookahead: heard by everybody from their start
    pub frames_ahead: u64,
    /// frames of further bulbs started in the quantum they were decided in:
    /// the other bulbs learn of them at its end
    pub frames_late_bulb: u64,
    /// frames of the first bulb and the scripted nodes in the parallel phase:
    /// the further bulbs learn of them at its end
    pub frames_late_main: u64,
    /// BLE packets that reached other bulbs at the end of a quantum
    pub ble_late: u64,
    /// emulated time the further bulbs ran in lookahead phases, ms
    pub commit_ms: f64,
    /// wall time the scenario's thread spent running the first bulb and the
    /// scripted nodes in the parallel phases
    pub main_busy: std::time::Duration,
}

impl World {
    /// Whether the frame on the air at `t0` (air time), `len` bytes long, began
    /// while the bulb's radio was lent to a BLE event: its receiver was in BLE
    /// mode (on a BLE channel, or still on the network's). The bulb does that
    /// for about 2 ms every 300 ms (an advertising event); a frame that starts
    /// then is lost whatever the driver does. Which frames those are depends on
    /// where the advertising events fall, which moves with every change of the
    /// code's timing (the firmware's random numbers are stirred with the timer).
    /// Needs the bulb's receive log (`dut_rx_log`).
    pub fn lent_to_ble(&self, t0: f64, len: usize) -> bool {
        let off = self.air.t_off;
        self.air.dut.radio.b().rx_log.as_ref().is_some_and(|log| {
            log.iter().any(|e| e.len == len && (e.t0 + off - t0).abs() < 0.01 && matches!(e.fate, "BLE mode" | "off at start: BLE mode"))
        })
    }

    /// Turns on the bulb's receive log (what became of every frame at its receiver).
    pub fn dut_rx_log(&mut self) {
        self.air.dut.radio.b().rx_log.get_or_insert_with(Vec::new);
    }

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
            lent: None,
            keys,
            dut,
            log,
        };
        World {
            air,
            coord,
            peers: Vec::new(),
            coords: Vec::new(),
            resets: 0,
            par: ParOptions::from_env(),
            pool: None,
            par_stats: ParStats::default(),
            ble_retry: false,
            ble_again: HashMap::new(),
        }
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
                let dut_rssi = rx.iter().find(|x| x.0 == NodeId::Dut).map(|x| x.1);
                if self.ble_retry {
                    let sender = self.air.frames[idx].who;
                    let scripted = matches!(sender, NodeId::Coord | NodeId::Peer(_) | NodeId::Coord2(_));
                    // a MAC data frame to the broadcast address: no acknowledgement, no MAC retry
                    let broadcast = psdu.len() > 7 && psdu[0] & 0x27 == 0x01 && psdu[5..7] == [0xFF, 0xFF];
                    if let (true, true, Some(rssi)) = (scripted, broadcast, dut_rssi) {
                        if self.lent_to_ble(t0, psdu.len()) {
                            let n = self.ble_again.entry((*psdu).clone()).or_insert(0);
                            if *n < 3 {
                                *n += 1;
                                let ev = Ev::Again { sender, psdu: psdu.clone(), ch, dur: t1 - t0, rssi };
                                self.air.at(t1 + 5.0, ev);
                            }
                        }
                    }
                }
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
            Ev::Again { sender, psdu, ch, dur, rssi } => {
                let now = self.air.t;
                self.air.transmit(sender, (*psdu).clone(), ch, now, now + dur, rssi);
            }
        }
    }

    /// Runs the scenario to `target` ms (`GoEngine.run_alone`).
    pub fn run_ms(&mut self, target: f64) -> Result<(), String> {
        self.run_ms_inner(target, false)
    }

    /// `run_ms` while the first bulb waits in TX mode (the parallel
    /// lockstep's lookahead): it stops once the bulb's frame has started or
    /// its radio has left TX mode.
    fn run_ms_commit(&mut self, target: f64) -> Result<(), String> {
        self.air.dut.set_stop_uncommitted(true);
        let r = self.run_ms_inner(target, true);
        self.air.dut.set_stop_uncommitted(false);
        r
    }

    fn run_ms_inner(&mut self, target: f64, commit: bool) -> Result<(), String> {
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
                if commit && !self.air.dut.tx_committed_or_started() {
                    self.air.t = self.air.t.max(r6(self.air.dut.ms()) + off);
                    return Ok(());
                }
                if let Some(t) = &mut self.air.dut_tap {
                    t.sync();
                }
                match self.air.dut.step_run(run_to) {
                    RunEvent::Tx(fr) => {
                        let (t0, t1) = (r6(fr.t0) + off, r6(fr.t1) + off);
                        self.air.t = t0;
                        self.air.transmit(NodeId::Dut, fr.psdu, fr.channel, t0, t1, -40);
                        if commit {
                            return Ok(());
                        }
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
        let powered = !self.air.off.contains(&NodeId::Bulb(i as u8));
        while let Some(o) = bulb_next(&mut self.air.bulbs[i], target, powered, false)? {
            self.air.put_out(NodeId::Bulb(i as u8), o);
        }
        Ok(())
    }

    /// `ble_air.lockstep`: all bulbs advance to `target` alternately in quanta,
    /// the schedule's owner (the first bulb) first. In parallel (`ParOptions`)
    /// when there is more than one bulb.
    pub fn lockstep(&mut self, target: f64, quantum: f64) -> Result<(), String> {
        if self.par.parallel && !self.air.bulbs.is_empty() {
            return self.lockstep_par(target, quantum);
        }
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

    /// `lockstep` with the bulbs in parallel (`ParOptions`): the further bulbs
    /// are taken out of the air for the run and lent to the worker threads.
    fn lockstep_par(&mut self, target: f64, quantum: f64) -> Result<(), String> {
        let mut bulbs = std::mem::take(&mut self.air.bulbs);
        let n = bulbs.len();
        let pool = match self.pool.take() {
            Some(p) if p.len() == n => p,
            _ => Pool::new(n),
        };
        self.air.lent = Some(Lent { n, q: Vec::new() });
        let r = self.lockstep_par_with(&mut bulbs, &pool, target, quantum.min(self.par.max_quantum));
        // whatever an error left undelivered goes with it
        self.air.lent = None;
        self.air.bulbs = bulbs;
        self.pool = Some(pool);
        r
    }

    fn lockstep_par_with(&mut self, bulbs: &mut [Bulb], pool: &Pool, target: f64, quantum: f64) -> Result<(), String> {
        let powered: Vec<bool> = (0..bulbs.len()).map(|i| !self.air.off.contains(&NodeId::Bulb(i as u8))).collect();
        let job = |b: &mut Bulb, i: usize, step: f64, phase: Phase| Job { bulb: BulbPtr::new(b), target: step, powered: powered[i], phase };
        loop {
            let now = bulbs.iter().map(|b| b.t).fold(self.air.t, f64::min);
            if now >= target {
                return Ok(());
            }
            let step = target.min(now + quantum);
            self.par_stats.quanta += 1;
            if self.par.lookahead {
                // the further bulbs that wait in TX mode: run until their frames start
                let jobs: Vec<Option<Job>> = bulbs
                    .iter_mut()
                    .enumerate()
                    .map(|(i, b)| (b.t < step && powered[i] && b.node.tx_committed()).then(|| job(b, i, step, Phase::Commit)))
                    .collect();
                if jobs.iter().any(Option::is_some) {
                    self.par_stats.commit_phases += 1;
                    let t_before: f64 = jobs.iter().zip(bulbs.iter()).filter(|(j, _)| j.is_some()).map(|(_, b)| b.t).sum();
                    let ran: Vec<bool> = jobs.iter().map(Option::is_some).collect();
                    // SAFETY: the bulbs are not touched until `wait` returns
                    unsafe { pool.start(jobs) };
                    pool.wait();
                    let t_after: f64 = ran.iter().zip(bulbs.iter()).filter(|(r, _)| **r).map(|(_, b)| b.t).sum();
                    self.par_stats.commit_ms += t_after - t_before;
                    let sent = self.collect_outs(bulbs, pool)?;
                    self.par_stats.frames_ahead += sent;
                }
                // the first bulb, likewise, with the scripted nodes up to then
                let off = self.air.off.contains(&NodeId::Dut);
                if self.air.t < step && !off && self.air.dut.tx_committed() {
                    self.par_stats.dut_commits += 1;
                    let n0 = self.air.frames.len();
                    self.run_ms_commit(step)?;
                    self.par_stats.frames_ahead += (self.air.frames.len() - n0) as u64;
                    self.drain_lent(bulbs);
                }
            }
            let jobs: Vec<Option<Job>> =
                bulbs.iter_mut().enumerate().map(|(i, b)| (b.t < step).then(|| job(b, i, step, Phase::Run))).collect();
            // SAFETY: the bulbs are not touched until `wait` returns (the air
            // sends them nothing directly while they are lent)
            unsafe { pool.start(jobs) };
            let n0 = self.air.frames.len();
            let t_main = std::time::Instant::now();
            let main = if self.air.t < step { self.run_ms(step) } else { Ok(()) };
            self.par_stats.main_busy += t_main.elapsed();
            pool.wait();
            self.par_stats.frames_late_main += (self.air.frames.len() - n0) as u64;
            main?;
            self.drain_lent(bulbs);
            self.par_stats.frames_late_bulb += self.collect_outs(bulbs, pool)?;
        }
    }

    /// What the workers' bulbs put out in the last phase goes on the air, bulb
    /// by bulb (each in its own order), and reaches the other further bulbs.
    /// Returns the frames.
    fn collect_outs(&mut self, bulbs: &mut [Bulb], pool: &Pool) -> Result<u64, String> {
        let mut frames = 0;
        let mut err = None;
        for k in 0..bulbs.len() {
            let (outs, e) = pool.take(k);
            for o in outs {
                match &o {
                    BulbOut::Tx { .. } => frames += 1,
                    BulbOut::Ble { .. } => self.par_stats.ble_late += 1,
                    BulbOut::Log(_) => {}
                }
                self.air.put_out(NodeId::Bulb(k as u8), o);
            }
            if err.is_none() {
                err = e;
            }
        }
        self.drain_lent(bulbs);
        match err {
            Some(e) => Err(e),
            None => Ok(frames),
        }
    }

    /// Delivers what the air kept for the lent bulbs, in the order it was sent.
    fn drain_lent(&mut self, bulbs: &[Bulb]) {
        let Some(l) = &mut self.air.lent else { return };
        for d in std::mem::take(&mut l.q) {
            match d {
                Deferred::Rx { bulb, t0, t1, ch, psdu, rssi, bad } => {
                    let b = &bulbs[bulb as usize];
                    if t0 >= b.t_off {
                        inject_154(&b.node, b.t_off, &psdu, ch, t0, t1, rssi, bad);
                    }
                }
                Deferred::Ble { bulb, t0, t1, ch, aa, pdu } => {
                    let b = &bulbs[bulb as usize];
                    b.node.ble_radio.air_rx(&b.node.m, pdu, ch, aa, r6(t0 - b.t_off), r6(t1 - b.t_off));
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
        if self.par.parallel && !self.air.bulbs.is_empty() {
            // all bulbs at once: the order does not matter
            return self.lockstep_par(target, quantum);
        }
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

impl Drop for World {
    /// `ZB_PARALLEL_STATS=1`: the parallel lockstep's counts on stderr.
    /// `ZB_AIR_LOG_DIR=dir`: every frame that went on the air, one file per
    /// world named by a hash of its content, and a line `name frames hash`
    /// in `dir/index.txt` (two runs of a scenario compare as these).
    fn drop(&mut self) {
        let set = |k: &str| std::env::var_os(k).is_some_and(|v| !v.is_empty());
        if set("ZB_PARALLEL_STATS") && self.par_stats.quanta > 0 {
            let busy: Vec<String> = self.pool.as_ref().map_or(Vec::new(), |p| p.busy()).iter().map(|d| format!("{:.1}", d.as_secs_f64())).collect();
            eprintln!("parallel lockstep ({}): {:?}; workers busy {} s", self.air.dut_name, self.par_stats, busy.join(" "));
        }
        if let Some(dir) = std::env::var_os("ZB_AIR_LOG_DIR").filter(|v| !v.is_empty()) {
            use std::fmt::Write as _;
            let mut text = String::new();
            for f in &self.air.frames {
                let hex: String = f.psdu.iter().map(|b| format!("{b:02x}")).collect();
                let _ = writeln!(text, "{:.6} {:.6} {} {} {hex}", f.t0, f.t1, f.ch, self.air.name_of(f.who));
            }
            // FNV-1a
            let h = text.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
            let dir = std::path::PathBuf::from(dir);
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(dir.join(format!("{h:016x}.txt")), &text);
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("index.txt")) {
                use std::io::Write as _;
                let _ = writeln!(f, "{} {} {h:016x}", self.air.dut_name, self.air.frames.len());
            }
        }
    }
}
