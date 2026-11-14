//! What the Trust Center and the scripted peers share (`NodeMixin` in
//! emulator/spec/harness.py): an ordered MAC queue with carrier sense and
//! stop-and-wait retries, and frame builders where every header field of
//! every layer can be forced by a test.

use std::collections::{HashMap, VecDeque};

use crate::air::{Air, Ev, NodeId};
use crate::frames::{
    airtime_us, aps_cmd, aps_data, mac_build, nwk_build, secure, with_fcs, zcl_build, Key, NwkOpts, BCAST_ROUTERS,
    NWK_CMD, NWK_DATA,
};

pub const EP: u8 = 1;
pub const HA: u16 = 0x0104;
pub const MAC_RETRIES: u32 = 3;

/// What a station scheduled for itself.
#[derive(Clone, Debug)]
pub enum StationEv {
    /// A frame transmitted at a fixed time (a MAC acknowledgement).
    Transmit { psdu: Vec<u8>, t0: f64, t1: f64 },
    PumpFire,
    TxDone,
    TxCheck,
}

#[derive(Clone, Debug)]
struct TxEntry {
    mpdu: Vec<u8>,
    t: f64,
    /// MAC retries left; None: no acknowledgement expected
    retries: Option<u32>,
    upper: u32,
    acked: bool,
}

/// Sequence numbers and counters of a frame a station sent.
#[derive(Clone, Debug, Default)]
pub struct Sent {
    pub nwk_seq: u8,
    pub fc: Option<u32>,
    pub counter: Option<u8>,
    pub zcl_seq: Option<u8>,
    pub zdo_seq: Option<u8>,
}

/// The source IEEE address field of an NWK header.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum SrcIeee {
    /// the station's own address
    #[default]
    Own,
    None,
    Is(u64),
}

/// Everything a test may force in a frame it sends (the keyword arguments of
/// `NodeMixin.tx_nwk`/`tx_aps_data`/`tx_zcl`/`tx_zdo` and of `Session.zcl`).
/// `None` means the default of the layer.
#[derive(Clone, Debug, Default)]
pub struct Tx {
    // NWK
    pub mac_dst: Option<u16>,
    pub src: Option<u16>,
    pub nwk_seq: Option<u8>,
    pub radius: Option<u8>,
    pub insecure: bool,
    pub src_ieee: SrcIeee,
    pub dst_ieee: Option<u64>,
    pub discover: u8,
    pub relays: Option<Vec<u16>>,
    pub relay_index: Option<u8>,
    pub ed_initiator: Option<bool>,
    pub key: Option<Key>,
    pub key_seq: Option<u8>,
    pub fc: Option<u32>,
    pub sec_src: Option<u64>,
    pub ar: Option<bool>,
    pub delay: Option<f64>,
    pub mac_src: Option<u16>,
    pub multicast: Option<u8>,
    pub pan: Option<u16>,
    pub version: Option<u8>,
    // APS
    pub ep: Option<u8>,
    pub src_ep: Option<u8>,
    pub profile: Option<u16>,
    pub ack_req: bool,
    pub counter: Option<u8>,
    pub group: Option<u16>,
    pub delivery: Option<u8>,
    pub frag: Option<(u8, u8)>,
    /// APS data secured with a link key (key identifier 0): (key, frame counter)
    pub aps_sec: Option<(Key, u32)>,
    // ZCL / ZDO
    pub general: bool,
    pub ddr: bool,
    pub to_client: bool,
    pub mfg: Option<u16>,
    pub seq: Option<u8>,
    pub fc_extra: u8,
    // Session.zcl / Session.zdo
    pub max_ms: Option<f64>,
    pub sender: Option<NodeId>,
    pub dst: Option<u16>,
}

macro_rules! setters {
    ($($name:ident: $t:ty),* $(,)?) => {
        $(pub fn $name(mut self, v: $t) -> Self { self.$name = Some(v); self })*
    };
}

impl Tx {
    pub fn new() -> Tx {
        Tx::default()
    }
    setters!(mac_dst: u16, src: u16, nwk_seq: u8, radius: u8, dst_ieee: u64, relays: Vec<u16>, relay_index: u8,
             ed_initiator: bool, key: Key, key_seq: u8, fc: u32, sec_src: u64, ar: bool, delay: f64, mac_src: u16,
             multicast: u8, pan: u16, version: u8, ep: u8, src_ep: u8, profile: u16, counter: u8, group: u16,
             delivery: u8, frag: (u8, u8), aps_sec: (Key, u32), mfg: u16, seq: u8, max_ms: f64, sender: NodeId,
             dst: u16);
    pub fn insecure(mut self) -> Self {
        self.insecure = true;
        self
    }
    pub fn src_ieee(mut self, v: SrcIeee) -> Self {
        self.src_ieee = v;
        self
    }
    pub fn discover(mut self, v: u8) -> Self {
        self.discover = v;
        self
    }
    pub fn ack_req(mut self) -> Self {
        self.ack_req = true;
        self
    }
    /// a general (profile-wide) ZCL command instead of a cluster-specific one
    pub fn general(mut self) -> Self {
        self.general = true;
        self
    }
    pub fn ddr(mut self) -> Self {
        self.ddr = true;
        self
    }
    pub fn to_client(mut self) -> Self {
        self.to_client = true;
        self
    }
    pub fn fc_extra(mut self, v: u8) -> Self {
        self.fc_extra = v;
        self
    }
}

pub fn nx(c: &mut u8) -> u8 {
    let v = *c;
    *c = v.wrapping_add(1);
    v
}

/// A node's MAC and frame builders.
pub struct Station {
    pub id: NodeId,
    pub channel: i64,
    pub pan: u16,
    /// None: no network address yet (an end device before association)
    pub short: Option<u16>,
    pub ext: u64,
    pub nwk_key: Key,
    pub key_seq: u8,
    pub fc_out: u32,
    pub mac_seq: u8,
    pub nwk_seq: u8,
    pub aps_ctr: u8,
    pub zdo_seq: u8,
    pub zcl_seq: u8,
    /// NWK destination -> next hop
    pub via: HashMap<u16, u16>,
    /// the end device initiator bit of the NWK frames it sends
    pub ed_initiator: bool,
    /// an end device: everything goes to the parent with an acknowledgement request (R22 3.6.5)
    pub ed_parent: Option<u16>,
    /// attempts of the layer above after the MAC retries are spent
    pub upper_retries: u32,
    txq: VecDeque<TxEntry>,
    tx_active: Option<TxEntry>,
    pump_at: Option<f64>,
    pub busy_until: f64,
    /// (ms, mpdu) of unicast frames nobody acknowledged
    pub no_ack: Vec<(f64, Vec<u8>)>,
    /// (ms, mpdu, acked or None)
    pub tx_log: Vec<(f64, Vec<u8>, Option<bool>)>,
    /// the pending bit of the acknowledgement of the frame in flight
    pub ack_pending_seen: bool,
    /// the MPDU queued last (`resend_last`)
    last_queued: Vec<u8>,
}

impl Station {
    #[allow(clippy::too_many_arguments)]
    pub fn new(id: NodeId, channel: i64, pan: u16, short: Option<u16>, ext: u64, nwk_key: Key,
               seqs: (u8, u8, u8, u8, u8), fc_out: u32, upper_retries: u32) -> Station {
        Station {
            id,
            channel,
            pan,
            short,
            ext,
            nwk_key,
            key_seq: 0,
            fc_out,
            mac_seq: seqs.0,
            nwk_seq: seqs.1,
            aps_ctr: seqs.2,
            zdo_seq: seqs.3,
            zcl_seq: seqs.4,
            via: HashMap::new(),
            ed_initiator: false,
            ed_parent: None,
            upper_retries,
            txq: VecDeque::new(),
            tx_active: None,
            pump_at: None,
            busy_until: 0.0,
            no_ack: Vec::new(),
            tx_log: Vec::new(),
            ack_pending_seen: false,
            last_queued: Vec::new(),
        }
    }

    pub fn short16(&self) -> u16 {
        self.short.unwrap_or(0xFFFE)
    }

    pub fn on_event(&mut self, air: &mut Air, e: StationEv) {
        match e {
            StationEv::Transmit { psdu, t0, t1 } => air.transmit(self.id, psdu, self.channel, t0, t1, -40),
            StationEv::PumpFire => {
                self.pump_at = None;
                self.pump(air);
            }
            StationEv::TxDone => self.tx_done(air),
            StationEv::TxCheck => self.tx_check(air),
        }
    }

    // ---------------------------------------------------------- MAC
    /// Queues an MPDU (the FCS is appended). `ack_wait`: MAC retries when no ACK
    /// comes (None: none expected); `upper`: extra attempts 100 ms later.
    pub fn send_psdu(&mut self, air: &mut Air, mpdu: Vec<u8>, delay_ms: f64, ack_wait: Option<u32>, upper: Option<u32>) {
        let upper = upper.unwrap_or(self.upper_retries);
        self.last_queued.clone_from(&mpdu);
        self.txq.push_back(TxEntry { mpdu, t: air.now() + delay_ms, retries: ack_wait, upper, acked: false });
        self.pump(air);
    }

    /// The MPDU queued last, once more `delay_ms` from now, as it was (the
    /// retransmission of a broadcast, R22 3.6.5: same NWK frame, and here the
    /// same MAC sequence number too).
    pub fn resend_last(&mut self, air: &mut Air, delay_ms: f64) {
        let mpdu = self.last_queued.clone();
        if !mpdu.is_empty() {
            self.send_psdu(air, mpdu, delay_ms, None, Some(0));
        }
    }

    fn pump(&mut self, air: &mut Air) {
        if self.tx_active.is_some() || self.txq.is_empty() {
            return;
        }
        let now = air.now();
        let e = &self.txq[0];
        let t = e.t.max(self.busy_until + 0.2).max(air.air_free_at() + 0.3);
        if t > now + 1e-9 {
            if self.pump_at.map_or(true, |p| p > t || p <= now) {
                self.pump_at = Some(t);
                air.at(t, Ev::Node(self.id, StationEv::PumpFire));
            }
            return;
        }
        let e = self.txq.pop_front().unwrap();
        let psdu = with_fcs(&e.mpdu);
        let t1 = now + airtime_us(psdu.len()) / 1000.0;
        self.busy_until = t1;
        let expects_ack = e.retries.is_some();
        self.tx_active = Some(e);
        air.transmit(self.id, psdu, self.channel, now, t1, -40);
        if expects_ack {
            air.at(t1 + 1.2, Ev::Node(self.id, StationEv::TxCheck)); // macAckWaitDuration is 0.864 ms
        } else {
            air.at(t1, Ev::Node(self.id, StationEv::TxDone));
        }
    }

    fn tx_done(&mut self, air: &mut Air) {
        if let Some(e) = self.tx_active.take() {
            let acked = e.retries.map(|_| e.acked);
            self.tx_log.push((air.now(), e.mpdu, acked));
        }
        self.pump(air);
    }

    fn tx_check(&mut self, air: &mut Air) {
        let Some(mut e) = self.tx_active.take() else { return };
        if e.acked {
            self.tx_active = Some(e);
            return self.tx_done(air);
        }
        let now = air.now();
        let retries = e.retries.unwrap_or(0);
        if retries > 0 {
            e.retries = Some(retries - 1);
            e.t = now + 1.0;
            self.txq.push_front(e);
            return self.pump(air);
        }
        if e.upper > 0 {
            let e2 = TxEntry { mpdu: e.mpdu, t: now + 100.0, retries: Some(MAC_RETRIES), upper: e.upper - 1, acked: e.acked };
            self.txq.push_back(e2);
            return self.pump(air);
        }
        self.no_ack.push((now, e.mpdu.clone()));
        self.tx_active = Some(e);
        self.tx_done(air);
    }

    pub fn mac_ack_seen(&mut self, seq: u8, pending: bool) {
        if let Some(e) = &mut self.tx_active {
            if e.retries.is_some() && e.mpdu.get(2) == Some(&seq) {
                e.acked = true;
                self.ack_pending_seen = pending;
            }
        }
    }

    /// An immediate MAC acknowledgement, aTurnaroundTime after the frame.
    pub fn send_ack(&mut self, air: &mut Air, seq: u8, t_end: f64, pending: bool) {
        let fcf: u16 = 0x0002 | if pending { 0x10 } else { 0 };
        let mut mpdu = fcf.to_le_bytes().to_vec();
        mpdu.push(seq);
        let psdu = with_fcs(&mpdu);
        let t0 = t_end + 0.192;
        let t1 = t0 + airtime_us(psdu.len()) / 1000.0;
        self.busy_until = self.busy_until.max(t1);
        air.at(t0, Ev::Node(self.id, StationEv::Transmit { psdu, t0, t1 }));
    }

    /// A MAC data frame (`NodeMixin.tx_mac_data`).
    pub fn tx_mac_data(&mut self, air: &mut Air, dst: u16, nwk_frame: &[u8], ar: Option<bool>, delay: f64,
                       mac_src: Option<u16>, pan: Option<u16>) {
        let ar = ar.unwrap_or(dst < 0xFFF8);
        let pan = pan.unwrap_or(self.pan);
        let seq = nx(&mut self.mac_seq);
        let src = mac_src.unwrap_or(self.short16());
        let mpdu = mac_build(1, seq, nwk_frame, pan, dst as u64, 2, src as u64, 2, Some(pan), ar, false);
        self.send_psdu(air, mpdu, delay, if ar { Some(MAC_RETRIES) } else { None }, None);
    }

    // ---------------------------------------------------------- NWK
    /// Sends an NWK frame; everything in the header can be forced.
    pub fn tx_nwk(&mut self, air: &mut Air, ftype: u8, dst: u16, payload: &[u8], o: &Tx) -> Sent {
        let seq = match o.nwk_seq {
            Some(s) => s,
            None => nx(&mut self.nwk_seq),
        };
        let src = o.src.unwrap_or(self.short16());
        let si = match o.src_ieee {
            SrcIeee::Own => Some(self.ext),
            SrcIeee::None => None,
            SrcIeee::Is(e) => Some(e),
        };
        let mut fc_used = None;
        let sec = if o.insecure {
            None
        } else {
            let fc = match o.fc {
                Some(f) => f,
                None => {
                    let f = self.fc_out;
                    self.fc_out = self.fc_out.wrapping_add(1);
                    f
                }
            };
            fc_used = Some(fc);
            Some((o.key.unwrap_or(self.nwk_key), o.key_seq.unwrap_or(self.key_seq), fc, o.sec_src.unwrap_or(self.ext)))
        };
        let no = NwkOpts {
            radius: o.radius.unwrap_or(30),
            discover: o.discover,
            src_ieee: si,
            dst_ieee: o.dst_ieee,
            relays: o.relays.clone(),
            relay_index: o.relay_index,
            multicast: o.multicast,
            ed_initiator: o.ed_initiator.unwrap_or(self.ed_initiator),
            version: o.version.unwrap_or(2),
            sec,
        };
        let frame = nwk_build(ftype, dst, src, seq, payload, &no);
        let (def_mac_dst, def_ar) = match self.ed_parent {
            Some(p) => (Some(p), Some(true)),
            None => (None, None),
        };
        let mac_dst = o.mac_dst.or(def_mac_dst).unwrap_or_else(|| {
            if dst >= 0xFFF8 {
                0xFFFF
            } else {
                *self.via.get(&dst).unwrap_or(&dst)
            }
        });
        self.tx_mac_data(air, mac_dst, &frame, o.ar.or(def_ar), o.delay.unwrap_or(0.4), o.mac_src, o.pan);
        Sent { nwk_seq: seq, fc: fc_used, ..Default::default() }
    }

    pub fn tx_nwk_cmd(&mut self, air: &mut Air, dst: u16, cmd_payload: &[u8], o: &Tx) -> Sent {
        self.tx_nwk(air, NWK_CMD, dst, cmd_payload, o)
    }

    // ---------------------------------------------------------- APS / ZDO / ZCL
    pub fn tx_aps_data(&mut self, air: &mut Air, dst: u16, cluster: u16, payload: &[u8], o: &Tx) -> Sent {
        let delivery = o.delivery.unwrap_or(if o.group.is_some() {
            3
        } else if dst >= 0xFFF8 {
            2
        } else {
            0
        });
        let counter = match o.counter {
            Some(c) => c,
            None => nx(&mut self.aps_ctr),
        };
        let (ep, src_ep, profile) = (o.ep.unwrap_or(EP), o.src_ep.unwrap_or(1), o.profile.unwrap_or(HA));
        let a = match o.aps_sec {
            Some((key, fc)) => {
                // APS data secured with a link key (key identifier 0), R22 4.4.1.1
                let hdr = aps_data(ep, cluster, profile, src_ep, counter, &[], delivery, o.ack_req, o.group, o.frag, true);
                secure(&hdr, payload, &key, self.ext, fc, 0, None, true)
            }
            None => aps_data(ep, cluster, profile, src_ep, counter, payload, delivery, o.ack_req, o.group, o.frag, false),
        };
        let mut r = self.tx_nwk(air, NWK_DATA, dst, &a, o);
        r.counter = Some(counter);
        r
    }

    pub fn tx_zcl(&mut self, air: &mut Air, dst: u16, cluster: u16, cmd: u8, payload: &[u8], o: &Tx) -> Sent {
        let seq = match o.seq {
            Some(s) => s,
            None => nx(&mut self.zcl_seq),
        };
        let mut z = zcl_build(seq, cmd, payload, !o.general, o.ddr, o.to_client, o.mfg);
        z[0] |= o.fc_extra;
        let mut r = self.tx_aps_data(air, dst, cluster, &z, o);
        r.zcl_seq = Some(seq);
        r
    }

    pub fn tx_zdo(&mut self, air: &mut Air, dst: u16, cluster: u16, payload: &[u8], o: &Tx) -> Sent {
        let seq = match o.seq {
            Some(s) => s,
            None => nx(&mut self.zdo_seq),
        };
        let mut p = vec![seq];
        p.extend_from_slice(payload);
        let o2 = Tx { ep: Some(0), src_ep: Some(0), profile: Some(0), ..o.clone() };
        let mut r = self.tx_aps_data(air, dst, cluster, &p, &o2);
        r.zdo_seq = Some(seq);
        r
    }

    /// An APS command; `sec`: (key, key identifier, frame counter, source).
    pub fn tx_aps_cmd(&mut self, air: &mut Air, dst: u16, payload: &[u8], sec: Option<(Key, u8, u32, u64)>, o: &Tx) -> Sent {
        let ctr = nx(&mut self.aps_ctr);
        let a = aps_cmd(ctr, payload, o.ack_req, sec, o.delivery.unwrap_or(0));
        let mut r = self.tx_nwk(air, NWK_DATA, dst, &a, o);
        r.counter = Some(ctr);
        r
    }

    /// One Link Status frame (R22 3.4.8): entries = (address, incoming cost, outgoing cost).
    pub fn tx_link_status(&mut self, air: &mut Air, entries: &[(u16, u8, u8)], o: &Tx) -> Sent {
        let mut e = entries.to_vec();
        e.sort();
        let mut body = vec![0x08, (e.len() as u8 & 0x1F) | 0x60];
        for (a, i, out) in e {
            body.extend_from_slice(&a.to_le_bytes());
            body.push((i & 7) | ((out & 7) << 4));
        }
        let o2 = Tx { radius: Some(1), ..o.clone() };
        self.tx_nwk_cmd(air, BCAST_ROUTERS, &body, &o2)
    }
}
