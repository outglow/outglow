use crate::air::{Air, Ev, NodeId};
use crate::frames::{ext_bytes, mac_build, BCAST_RXON};
use crate::session::{CapFrame, Session, R};
use crate::station::{nx, Station, Tx, MAC_RETRIES};

#[derive(Clone, Debug)]
pub enum PeerEv {
    LinkStatus,
}

pub struct Peer {
    pub st: Station,
    pub name: String,
    pub auto_ack: bool,
    pub ack_pending: bool,
    pub ls_period: Option<f64>,
    pub ls_neighbours: Option<Vec<u16>>,
    dut_short: u16,
    pub cap: u8,
    pub joined: bool,
}

impl Peer {
    pub fn on_frame(&mut self, air: &mut Air, psdu: &[u8], ch: i64, t1: f64) {
        if ch != self.st.channel {
            return;
        }
        let Some(m) = crate::frames::mac_parse(psdu) else { return };
        if m.ty == 2 {
            self.st.mac_ack_seen(m.seq, m.pending);
            return;
        }
        let to_us = m.dmode != 0
            && ((m.dmode == 2 && self.st.short.is_some() && m.dst == self.st.short.map(|s| s as u64))
                || (m.dmode == 3 && m.dst == Some(self.st.ext)))
            && (m.dpan == Some(self.st.pan) || m.dpan == Some(0xFFFF));
        if to_us && m.ar && self.auto_ack {
            let p = self.ack_pending;
            self.st.send_ack(air, m.seq, t1, p);
        }
    }

    pub fn on_peer_event(&mut self, air: &mut Air, e: PeerEv) {
        match e {
            PeerEv::LinkStatus => self.link_status_tick(air),
        }
    }

    fn link_status_tick(&mut self, air: &mut Air) {
        let Some(period) = self.ls_period else { return };
        let nb = self.ls_neighbours.clone().unwrap_or_else(|| vec![self.dut_short]);
        let entries: Vec<(u16, u8, u8)> = nb.iter().map(|a| (*a, 1, 1)).collect();
        self.st.tx_link_status(air, &entries, &Tx::new());
        let NodeId::Peer(i) = self.st.id else { unreachable!() };
        let t = air.now() + period;
        air.at(t, Ev::Peer(i, PeerEv::LinkStatus));
    }

    pub fn start_link_status(&mut self, air: &mut Air, period_ms: f64, neighbours: Option<Vec<u16>>) {
        self.ls_period = Some(period_ms);
        self.ls_neighbours = neighbours;
        self.link_status_tick(air);
    }

    pub fn stop_link_status(&mut self) {
        self.ls_period = None;
    }
}

#[derive(Clone, Debug)]
pub struct Beacon {
    pub frame: CapFrame,
    pub permit: bool,
    pub pan_coord: bool,
    pub src: Option<u64>,
    pub pan: Option<u16>,
    pub protocol_id: Option<u8>,
    pub stack_profile: Option<u8>,
    pub version: Option<u8>,
    pub router_cap: Option<bool>,
    pub depth: Option<u8>,
    pub ed_cap: Option<bool>,
    pub epid: Option<u64>,
    pub update_id: Option<u8>,
}

pub fn parse_beacon(f: &CapFrame) -> Beacon {
    let m = f.mac.as_ref().unwrap();
    let p = &m.payload;
    let sf = u16::from_le_bytes([p.first().copied().unwrap_or(0), p.get(1).copied().unwrap_or(0)]);
    let mut b = Beacon {
        frame: f.clone(),
        permit: sf & 0x8000 != 0,
        pan_coord: sf & 0x4000 != 0,
        src: m.src,
        pan: m.span,
        protocol_id: None,
        stack_profile: None,
        version: None,
        router_cap: None,
        depth: None,
        ed_cap: None,
        epid: None,
        update_id: None,
    };
    let q = p.get(4..).unwrap_or(&[]);
    if q.len() >= 15 {
        b.protocol_id = Some(q[0]);
        b.stack_profile = Some(q[1] & 0xF);
        b.version = Some(q[1] >> 4);
        b.router_cap = Some(q[2] & 0x04 != 0);
        b.depth = Some((q[2] >> 3) & 0xF);
        b.ed_cap = Some(q[2] & 0x80 != 0);
        b.epid = Some(u64::from_le_bytes(q[3..11].try_into().unwrap()));
        b.update_id = Some(q[14]);
    }
    b
}

#[derive(Clone, Debug, Default)]
pub struct Poll {
    pub acked: bool,
    pub pending: bool,
    pub frames: Vec<CapFrame>,
    pub t: f64,
}

#[derive(Clone, Debug, Default)]
pub struct EdJoin {
    pub ok: bool,
    pub why: String,
    pub beacon: Option<Beacon>,
    pub poll_pending: Option<bool>,
    pub assoc_frame: Option<CapFrame>,
    pub status: Option<u8>,
    pub short: Option<u16>,
    pub key_frame: Option<CapFrame>,
    pub update_device: Vec<CapFrame>,
    pub timeout: Option<EdTimeout>,
}

#[derive(Clone, Debug)]
pub struct EdTimeout {
    pub status: u8,
    pub parent_info: u8,
    pub frame: CapFrame,
}

fn idx(p: NodeId) -> usize {
    match p {
        NodeId::Peer(i) => i as usize,
        _ => panic!("not a peer: {p:?}"),
    }
}

impl Session {
    pub fn peer(&mut self, p: NodeId) -> &mut Peer {
        &mut self.w.peers[idx(p)]
    }

    pub fn add_peer(&mut self, name: &str, ext: u64, short: Option<u16>, hears: Option<&[&str]>) -> NodeId {
        let id = NodeId::Peer(self.w.peers.len() as u8);
        let c = &self.w.coord.st;
        let mut st = Station::new(id, c.channel, c.pan, short, ext, c.nwk_key, (0x80, 0x60, 0x70, 0x90, 0xA0), 0x100, 0);
        st.key_seq = c.key_seq;
        self.w.peers.push(Peer {
            st,
            name: name.to_string(),
            auto_ack: true,
            ack_pending: false,
            ls_period: None,
            ls_neighbours: None,
            dut_short: self.dut_short,
            cap: 0,
            joined: false,
        });
        self.w.air.nodes.push(id);
        self.w.air.names.push(name.to_string());
        self.hears.push(hears.map(|h| h.iter().map(|s| s.to_string()).collect()));
        let mut names: Vec<(String, NodeId)> = vec![("dut".into(), NodeId::Dut), ("coord".into(), NodeId::Coord)];
        for (k, p) in self.w.peers.iter().enumerate() {
            names.push((p.name.clone(), NodeId::Peer(k as u8)));
        }
        for (k, p) in self.w.peers.iter().enumerate() {
            let Some(h) = &self.hears[k] else { continue };
            for (other, oid) in &names {
                if *other != p.name && !h.contains(other) {
                    self.w.air.block(NodeId::Peer(k as u8), *oid, true);
                }
            }
        }
        id
    }

    pub fn remove_peer(&mut self, p: NodeId) {
        let ids: Vec<NodeId> = self.w.air.nodes.clone();
        let pr = self.peer(p);
        pr.auto_ack = false;
        pr.ls_period = None;
        for o in ids {
            if o != p {
                self.w.air.block(p, o, true);
            }
        }
    }

    pub fn start_link_status(&mut self, p: NodeId, period_ms: f64, neighbours: Option<Vec<u16>>) {
        let World2 { air, peers } = self.split();
        peers[idx(p)].start_link_status(air, period_ms, neighbours);
    }

    fn split(&mut self) -> World2<'_> {
        World2 { air: &mut self.w.air, peers: &mut self.w.peers }
    }

    pub fn router(&mut self, name: &str, ext: u64, short: u16, hears: Option<&[&str]>, link_status: bool) -> NodeId {
        let p = self.add_peer(name, ext, Some(short), hears);
        self.w.air.keys.learn(Some(short), Some(ext));
        if link_status {
            self.start_link_status(p, 15000.0, None);
        }
        p
    }

    pub fn router2(&mut self) -> NodeId {
        self.router("r2", 0x00124B00BB000002, 0x2B02, Some(&["dut"]), true)
    }

    pub fn end_device(&mut self, name: &str, ext: u64, cap: u8, permit: bool) -> R<NodeId> {
        if permit && !self.note::<bool>("permit_open").unwrap_or(false) {
            self.w.coord.permit_joining(&mut self.w.air, 254);
            self.run(600.0)?;
            self.set_note("permit_open", true);
        }
        Ok(self.new_end_device(name, ext, cap))
    }

    pub fn new_end_device(&mut self, name: &str, ext: u64, cap: u8) -> NodeId {
        let p = self.add_peer(name, ext, None, Some(&["dut"]));
        let dut = self.dut_short;
        let pr = self.peer(p);
        pr.cap = cap;
        pr.st.ed_parent = Some(dut);
        pr.st.ed_initiator = false;
        p
    }

    pub fn beacon_request(&mut self, p: NodeId, wait_ms: f64) -> R<Option<Beacon>> {
        let i = self.mark();
        let (st, air) = self.w.st(p);
        let seq = nx(&mut st.mac_seq);
        let mpdu = mac_build(3, seq, &[0x07], 0xFFFF, 0xFFFF, 2, 0, 0, None, false, false);
        st.send_psdu(air, mpdu, 0.4, None, None);
        let f = self.wait(|f| f.mac.as_ref().is_some_and(|m| m.ty == 0), wait_ms, Some(i), Some(NodeId::Dut), 5.0)?;
        Ok(f.map(|f| parse_beacon(&f)))
    }

    pub fn beacon(&mut self) -> R<Option<Beacon>> {
        let sc = match self.note::<NodeId>("scanner") {
            Some(p) => p,
            None => {
                let p = self.add_peer("scan", 0x00124B00CC000009, None, Some(&["dut"]));
                self.set_note("scanner", p);
                p
            }
        };
        for _ in 0..3 {
            if let Some(b) = self.beacon_request(sc, 60.0)? {
                return Ok(Some(b));
            }
        }
        Ok(None)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn ed_mac_cmd(&mut self, p: NodeId, payload: &[u8], dst: Option<u16>, src_ext: bool, span: Option<u16>, ar: bool,
                      dpan: Option<u16>, dmode: u8) -> u8 {
        let (st, air) = self.w.st(p);
        let dst = dst.or(st.ed_parent).unwrap_or(0xFFFF);
        let dpan = dpan.unwrap_or(st.pan);
        let seq = nx(&mut st.mac_seq);
        let mpdu = if src_ext || st.short.is_none() {
            mac_build(3, seq, payload, dpan, dst as u64, dmode, st.ext, 3, Some(span.unwrap_or(dpan)), ar, false)
        } else {
            mac_build(3, seq, payload, dpan, dst as u64, dmode, st.short16() as u64, 2, Some(dpan), ar, false)
        };
        st.send_psdu(air, mpdu, 0.4, if ar { Some(MAC_RETRIES) } else { None }, None);
        seq
    }

    pub fn ed_poll(&mut self, p: NodeId, wait_ms: f64, src_ext: bool) -> R<Poll> {
        let i = self.mark();
        let seq = self.ed_mac_cmd(p, &[0x04], None, src_ext, None, true, None, 2);
        let ack = self.wait(
            move |f| f.mac.as_ref().is_some_and(|m| m.ty == 2 && m.seq == seq),
            30.0,
            Some(i),
            Some(NodeId::Dut),
            1.0,
        )?;
        let mut res = Poll {
            acked: ack.is_some(),
            pending: ack.as_ref().is_some_and(|a| a.mac.as_ref().unwrap().pending),
            frames: vec![],
            t: self.now(),
        };
        let Some(ack) = ack else { return Ok(res) };
        self.run(if res.pending { wait_ms } else { 12.0 })?;
        let st = &self.peer(p).st;
        let me: Vec<u64> = [st.short.map(|s| s as u64), Some(st.ext)].into_iter().flatten().collect();
        res.frames = self.frames(ack.i + 1, Some(NodeId::Dut), |f| {
            f.mac.as_ref().is_some_and(|m| (m.ty == 1 || m.ty == 3) && m.dst.is_some_and(|d| me.contains(&d)))
        });
        Ok(res)
    }

    pub fn ed_join(&mut self, p: NodeId, timeout: Option<u8>, annce: bool) -> R<EdJoin> {
        let mut r = EdJoin::default();
        for _ in 0..4 {
            r.beacon = self.beacon_request(p, 100.0)?;
            if r.beacon.is_some() {
                break;
            }
        }
        let Some(b) = &r.beacon else {
            r.why = "the bulb sent no beacon".into();
            return Ok(r);
        };
        if !b.permit {
            r.why = "the bulb's beacon does not permit association".into();
            return Ok(r);
        }
        let i0 = self.mark();
        let cap = self.peer(p).cap;
        self.ed_mac_cmd(p, &[0x01, cap], None, true, Some(0xFFFF), true, None, 2);
        self.run(500.0)?;
        let pl = self.ed_poll(p, 60.0, true)?;
        r.poll_pending = Some(pl.pending);
        let Some(rsp) = pl.frames.iter().find(|f| f.mac_cmd() == Some(0x02)).cloned() else {
            r.why = format!(
                "no Association Response (poll acked={}, pending={})",
                if pl.acked { "True" } else { "False" },
                if pl.pending { "True" } else { "False" }
            );
            return Ok(r);
        };
        let m = rsp.mac.as_ref().unwrap();
        let short = u16::from_le_bytes([m.payload[1], m.payload[2]]);
        let status = m.payload[3];
        r.assoc_frame = Some(rsp.clone());
        r.status = Some(status);
        if status != 0 {
            r.why = format!("association refused, status {status:#04x}");
            return Ok(r);
        }
        let ext = self.peer(p).st.ext;
        self.peer(p).st.short = Some(short);
        self.w.air.keys.learn(Some(short), Some(ext));
        r.short = Some(short);
        let mut key = None;
        for _ in 0..12 {
            let pl = self.ed_poll(p, 60.0, false)?;
            for f in &pl.frames {
                if let Some(a) = &f.aps {
                    if let Some(ap) = &a.payload {
                        if a.cmd == Some(0x05) && ap.len() >= 19 && ap[1] == 0x01 {
                            key = Some(<[u8; 16]>::try_from(&ap[2..18]).unwrap());
                            r.key_frame = Some(f.clone());
                            self.peer(p).st.key_seq = ap[18];
                        }
                    }
                }
            }
            if key.is_some() {
                break;
            }
            self.run(250.0)?;
        }
        r.update_device = self.frames(i0, Some(NodeId::Dut), |f| f.aps_cmd() == Some(0x06));
        let Some(key) = key else {
            r.why = "no Transport Key reached the end device".into();
            return Ok(r);
        };
        let pr = self.peer(p);
        pr.st.nwk_key = key;
        pr.joined = true;
        r.ok = true;
        if annce {
            self.ed_device_annce(p);
            self.run(200.0)?;
        }
        if let Some(t) = timeout {
            r.timeout = self.ed_timeout_request(p, t, 0, 200.0)?;
        }
        Ok(r)
    }

    pub fn ed_device_annce(&mut self, p: NodeId) {
        let pr = self.peer(p);
        let mut body = pr.st.short16().to_le_bytes().to_vec();
        body.extend_from_slice(&ext_bytes(pr.st.ext));
        body.push(pr.cap);
        let parent = pr.st.ed_parent.unwrap_or(0);
        let (st, air) = self.w.st(p);
        st.tx_zdo(air, BCAST_RXON, 0x0013, &body, &Tx::new().mac_dst(parent).ar(true));
    }

    pub fn ed_timeout_request(&mut self, p: NodeId, enumv: u8, config: u8, wait_ms: f64) -> R<Option<EdTimeout>> {
        let i = self.mark();
        let parent = self.peer(p).st.ed_parent.unwrap_or(0);
        let dut_ext = self.dut_ext;
        let (st, air) = self.w.st(p);
        st.tx_nwk_cmd(air, parent, &[0x0B, enumv, config], &Tx::new().radius(1).dst_ieee(dut_ext));
        self.run(40.0)?;
        for _ in 0..4 {
            let pl = self.ed_poll(p, 60.0, false)?;
            let me = self.peer(p).st.short;
            let mut all = pl.frames.clone();
            all.extend(self.frames(i, Some(NodeId::Dut), |_| true));
            for f in all {
                if f.nwk_cmd() == Some(0x0C) && Some(f.nwk.as_ref().unwrap().dst) == me {
                    let pl = f.nwk.as_ref().unwrap().payload.clone().unwrap_or_default();
                    if pl.get(1) == Some(&0) {
                        self.peer(p).st.ed_initiator = true;
                    }
                    return Ok(Some(EdTimeout { status: pl[1], parent_info: pl[2], frame: f }));
                }
            }
            self.run(wait_ms)?;
        }
        Ok(None)
    }

    pub fn ed_leave(&mut self, p: NodeId, rejoin: bool) {
        let opts = if rejoin { 0x20 } else { 0 };
        let (st, air) = self.w.st(p);
        st.tx_nwk_cmd(air, BCAST_RXON, &[0x04, opts], &Tx::new().radius(1));
    }

    pub fn ed_rejoin_request(&mut self, p: NodeId, secure: bool, wait_ms: f64) -> R<Option<CapFrame>> {
        let i = self.mark();
        let pr = self.peer(p);
        let (parent, cap) = (pr.st.ed_parent.unwrap_or(0), pr.cap);
        let dut_ext = self.dut_ext;
        let mut o = Tx::new().radius(1).dst_ieee(dut_ext).ed_initiator(false);
        o.insecure = !secure;
        let (st, air) = self.w.st(p);
        st.tx_nwk_cmd(air, parent, &[0x06, cap], &o);
        self.run(60.0)?;
        for _ in 0..4 {
            let pl = self.ed_poll(p, 60.0, false)?;
            let mut all = pl.frames.clone();
            all.extend(self.frames(i, Some(NodeId::Dut), |_| true));
            if let Some(f) = all.into_iter().find(|f| f.nwk_cmd() == Some(0x07)) {
                return Ok(Some(f));
            }
            self.run(wait_ms)?;
        }
        Ok(None)
    }
}

struct World2<'a> {
    air: &'a mut Air,
    peers: &'a mut Vec<Peer>,
}

impl Peer {
    pub fn bare(st: Station, name: &str) -> Peer {
        Peer {
            st,
            name: name.to_string(),
            auto_ack: false,
            ack_pending: false,
            ls_period: None,
            ls_neighbours: None,
            dut_short: 0xFFFF,
            cap: 0,
            joined: false,
        }
    }

    pub fn bare_ack(st: Station, name: &str) -> Peer {
        Peer {
            st,
            name: name.to_string(),
            auto_ack: true,
            ack_pending: false,
            ls_period: None,
            ls_neighbours: None,
            dut_short: 0xFFFE,
            cap: 0,
            joined: false,
        }
    }
}
