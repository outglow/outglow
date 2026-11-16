use super::addr_conflict::{bulb_node, Net};
use super::{SimCtx, SimDef};
use crate::air::NodeId;
use crate::coord::{Coord, NWK_KEY};
use crate::frames::{ext_bytes, mac_build};
use crate::peer::{parse_beacon, Peer};
use crate::session::{CapFrame, R};
use crate::station::{nx, SrcIeee, Station, Tx, MAC_RETRIES};

pub fn defs() -> Vec<SimDef> {
    vec![
        SimDef { name: "enddev_wallswitch", doc: "a sleepy end device whose parent bulb is switched off and on (parent choice: previous, else best link)", run: run_prev },
        SimDef { name: "enddev_wallswitch_edinit", doc: "the same, the end device sets the end device initiator bit (R21 and later)", run: run_nodst },
    ]
}

pub const BUTTON_EXT: u64 = 0x6cd8fb00002e163b;
const CAP: u8 = 0x80;
const POLL_MS: f64 = 7500.0;
const PRESS_MS: f64 = 5000.0;
const LOST_AFTER: u32 = 3;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Choice {
    PrevThenBest,
    EdInitiator,
}

#[derive(Clone, Debug, PartialEq)]
enum State {
    Joined,
    Rejoining { attempt: u32, next: f64 },
}

pub struct Button {
    pub id: NodeId,
    pub parent: u16,
    pub parent_id: NodeId,
    state: State,
    fails: u32,
    next_poll: f64,
    next_press: f64,
    pub presses: Vec<(f64, bool)>,
    pub annces: Vec<(f64, u16, String)>,
    pub rejoins: Vec<(f64, String)>,
    pub received: Vec<(f64, String)>,
    pub choice: Choice,
    pub dst_ieee: bool,
    pub leaves: u32,
    pub quality: Vec<(NodeId, u8)>,
    seen_rx: usize,
}

fn peer_ix(id: NodeId) -> usize {
    match id {
        NodeId::Peer(i) => i as usize,
        _ => panic!("not a peer"),
    }
}

impl Button {
    fn st<'a>(&self, net: &'a mut Net) -> (&'a mut Station, &'a mut crate::air::Air) {
        net.s.w.st(self.id)
    }

    fn short(&self, net: &Net) -> u16 {
        net.s.w.peers[peer_ix(self.id)].st.short16()
    }

    fn mac_cmd(&self, net: &mut Net, payload: &[u8], dst: u16, src_ext: bool, ar: bool) -> u8 {
        let pan = net.s.w.coord.st.pan;
        let (st, air) = self.st(net);
        let seq = nx(&mut st.mac_seq);
        let src = if src_ext || st.short.is_none() { (st.ext, 3) } else { (st.short16() as u64, 2) };
        let span = if dst == 0xFFFF && payload[0] == 0x07 { None } else { Some(pan) };
        let mpdu = if payload[0] == 0x07 {
            mac_build(3, seq, payload, 0xFFFF, 0xFFFF, 2, 0, 0, None, false, false)
        } else {
            mac_build(3, seq, payload, pan, dst as u64, 2, src.0, src.1, span, ar, false)
        };
        st.send_psdu(air, mpdu, 0.4, if ar { Some(MAC_RETRIES) } else { None }, Some(0));
        seq
    }

    fn acked(&self, net: &Net, seq: u8) -> Option<(bool, bool)> {
        let st = &net.s.w.peers[peer_ix(self.id)].st;
        st.tx_log.iter().rev().find(|(_, m, a)| m.get(2) == Some(&seq) && a.is_some()).map(|(_, _, a)| (a.unwrap(), st.ack_pending_seen))
    }

    fn mac_wait(&self, net: &mut Net, seq: u8) -> R<(bool, bool)> {
        for _ in 0..40 {
            if let Some(r) = self.acked(net, seq) {
                return Ok(r);
            }
            net.run(5.0)?;
        }
        Ok((false, false))
    }

    fn frames_for(&self, net: &Net, from: usize) -> Vec<CapFrame> {
        let st = &net.s.w.peers[peer_ix(self.id)].st;
        let me: Vec<u64> = [st.short.map(|s| s as u64), Some(st.ext)].into_iter().flatten().collect();
        net.cap[from.min(net.cap.len())..]
            .iter()
            .filter(|f| f.who != self.id && !net.s.w.air.blocked.contains(&(f.who, self.id)))
            .filter(|f| f.mac.as_ref().is_some_and(|m| (m.ty == 1 || m.ty == 3) && m.dst.is_some_and(|d| me.contains(&d))))
            .cloned()
            .collect()
    }

    fn poll(&mut self, net: &mut Net) -> R<Option<Vec<CapFrame>>> {
        let mark = net.cap.len();
        let p = self.parent;
        let seq = self.mac_cmd(net, &[0x04], p, false, true);
        let (ok, pending) = self.mac_wait(net, seq)?;
        if !ok {
            return Ok(None);
        }
        net.run(if pending { 60.0 } else { 10.0 })?;
        Ok(Some(self.frames_for(net, mark)))
    }

    fn note_rx(&mut self, net: &Net, fs: &[CapFrame]) {
        for f in fs {
            if f.mac.as_ref().is_some_and(|m| m.ty == 1) && f.i >= self.seen_rx {
                self.received.push((f.t0, f.brief()));
            }
        }
        if let Some(l) = fs.last() {
            self.seen_rx = self.seen_rx.max(l.i + 1);
        }
        let _ = net;
    }

    fn annce(&mut self, net: &mut Net) {
        let me = self.short(net);
        let mut body = me.to_le_bytes().to_vec();
        body.extend_from_slice(&ext_bytes(BUTTON_EXT));
        body.push(CAP);
        let p = self.parent;
        let (st, air) = self.st(net);
        st.tx_zdo(air, 0xFFFD, 0x0013, &body, &Tx::new().mac_dst(p).ar(true).src_ieee(SrcIeee::Own));
        let t = net.now();
        let pn = net.name_of(self.parent_id);
        self.annces.push((t, me, pn.clone()));
        net.note(format!("button: Device_annce {me:#06x} (parent {pn})"));
    }

    fn tick(&mut self, net: &mut Net) -> R<()> {
        let now = net.now();
        match self.state.clone() {
            State::Joined => {
                if now >= self.next_press {
                    self.next_press = now + PRESS_MS;
                    let p = self.parent;
                    let mark = net.cap.len();
                    {
                        let (st, air) = self.st(net);
                        st.tx_zcl(air, 0x0000, 0x0006, 0x02, &[], &Tx::new().mac_dst(p).ar(true).ep(1).src_ep(1).src_ieee(SrcIeee::None));
                    }
                    let seq = net.cap.len();
                    let _ = seq;
                    let mseq = net.s.w.peers[peer_ix(self.id)].st.mac_seq.wrapping_sub(1);
                    let (ok, _) = self.mac_wait(net, mseq)?;
                    let _ = mark;
                    self.presses.push((now, ok));
                    self.fail_or_ok(net, ok);
                } else if now >= self.next_poll {
                    self.next_poll = now + POLL_MS;
                    let r = self.poll(net)?;
                    if let Some(fs) = &r {
                        self.note_rx(net, fs);
                        if let Some(f) = fs.iter().find(|f| f.nwk_cmd() == Some(0x04)) {
                            let o = f.nwk.as_ref().unwrap().payload.as_ref().and_then(|p| p.get(1).copied()).unwrap_or(0);
                            let t = net.now();
                            net.note(format!("button: NWK Leave (options {o:#04x}) from {}", f.who_name));
                            self.leaves += 1;
                            if o & 0x20 != 0 {
                                self.rejoins.push((t, format!("NWK Leave with rejoin from {}", f.who_name)));
                                self.state = State::Rejoining { attempt: 0, next: t };
                                return Ok(());
                            }
                        }
                    }
                    self.fail_or_ok(net, r.is_some());
                }
            }
            State::Rejoining { attempt, next } => {
                if now >= next {
                    self.rejoin(net, attempt)?;
                }
            }
        }
        Ok(())
    }

    fn fail_or_ok(&mut self, net: &mut Net, ok: bool) {
        if ok {
            self.fails = 0;
            return;
        }
        self.fails += 1;
        if self.fails >= LOST_AFTER {
            self.fails = 0;
            let t = net.now();
            net.note(format!("button: parent {:#06x} lost after {LOST_AFTER} failed transmissions", self.parent));
            self.rejoins.push((t, "parent lost".into()));
            self.state = State::Rejoining { attempt: 0, next: t };
        }
    }

    fn rejoin(&mut self, net: &mut Net, attempt: u32) -> R<()> {
        let mark = net.cap.len();
        self.mac_cmd(net, &[0x07], 0xFFFF, false, false);
        net.run(250.0)?;
        let mut beacons: Vec<(NodeId, u16, u8)> = Vec::new();
        for f in &net.cap[mark..] {
            if f.who == self.id || net.s.w.air.blocked.contains(&(f.who, self.id)) {
                continue;
            }
            if f.mac.as_ref().is_some_and(|m| m.ty == 0) {
                let b = parse_beacon(f);
                if b.ed_cap == Some(true) || b.ed_cap.is_none() {
                    if let Some(s) = b.src {
                        beacons.push((f.who, s as u16, b.depth.unwrap_or(0)));
                    }
                }
            }
        }
        let q = |id: NodeId| self.quality.iter().find(|x| x.0 == id).map(|x| x.1).unwrap_or(100);
        let pick = beacons.iter().find(|b| b.0 == self.parent_id).copied().or_else(|| {
            let mut v = beacons.clone();
            v.sort_by(|a, b| q(b.0).cmp(&q(a.0)).then(a.2.cmp(&b.2)));
            v.first().copied()
        });
        let heard: Vec<String> = beacons.iter().map(|b| format!("{} {:#06x} depth {}", net.name_of(b.0), b.1, b.2)).collect();
        let t = net.now();
        let Some((pid, pshort, _)) = pick else {
            net.note(format!("button: rejoin attempt {}: no beacon", attempt + 1));
            self.rejoins.push((t, format!("attempt {}: no beacon", attempt + 1)));
            self.state = State::Rejoining { attempt: attempt + 1, next: t + 3000.0 };
            return Ok(());
        };
        let mark = net.cap.len();
        let me = self.short(net);
        let secure = attempt % 2 == 0;
        let pext = net.bulbs.iter().find(|x| x.id == pid).and_then(|x| x.ext);
        {
            let (st, air) = self.st(net);
            let mut o = Tx::new().radius(1).mac_dst(pshort).ar(true).src_ieee(SrcIeee::Own);
            o.dst_ieee = if self.dst_ieee { pext } else { None };
            o.insecure = !secure;
            st.tx_nwk_cmd(air, pshort, &[0x06, CAP], &o);
        }
        let mseq = net.s.w.peers[peer_ix(self.id)].st.mac_seq.wrapping_sub(1);
        let (ok, _) = self.mac_wait(net, mseq)?;
        let mut got = None;
        if ok {
            for _ in 0..4 {
                net.run(100.0)?;
                let saved = (self.parent, self.parent_id);
                self.parent = pshort;
                self.parent_id = pid;
                let r = self.poll(net)?;
                self.parent = saved.0;
                self.parent_id = saved.1;
                let mut all = r.unwrap_or_default();
                all.extend(self.frames_for(net, mark));
                if let Some(f) = all.iter().find(|f| f.nwk_cmd() == Some(0x07)) {
                    let p = f.nwk.as_ref().unwrap().payload.clone().unwrap_or_default();
                    if p.len() >= 4 {
                        got = Some((u16::from_le_bytes([p[1], p[2]]), p[3]));
                    }
                    break;
                }
            }
        }
        let pn = net.name_of(pid);
        match got {
            Some((new, 0)) => {
                net.note(format!("button: rejoined through {pn} ({pshort:#06x}) as {new:#06x}; beacons [{}]", heard.join(", ")));
                self.rejoins.push((t, format!("attempt {} ({}): rejoined through {pn} as {new:#06x}", attempt + 1, if secure { "secured" } else { "TC" })));
                self.parent = pshort;
                self.parent_id = pid;
                net.s.w.peers[peer_ix(self.id)].st.short = Some(new);
                net.s.w.peers[peer_ix(self.id)].st.ed_parent = Some(pshort);
                net.s.w.air.keys.learn(Some(new), Some(BUTTON_EXT));
                net.set_truth(BUTTON_EXT, new, "button");
                self.state = State::Joined;
                self.annce(net);
                let now = net.now();
                self.next_poll = now + 300.0;
                self.next_press = self.next_press.max(now + 500.0);
                let _ = me;
            }
            other => {
                let why = match other {
                    Some((_, s)) => format!("status {s:#04x}"),
                    None if ok => "no Rejoin Response".to_string(),
                    None => "Rejoin Request not acknowledged".to_string(),
                };
                net.note(format!("button: {} rejoin attempt {} through {pn}: {why}; beacons [{}]", if secure { "secured" } else { "TC" }, attempt + 1, heard.join(", ")));
                self.rejoins.push((t, format!("attempt {} ({}) through {pn}: {why}", attempt + 1, if secure { "secured" } else { "TC" })));
                self.state = State::Rejoining { attempt: attempt + 1, next: net.now() + 3000.0 };
            }
        }
        Ok(())
    }
}

fn join_via(net: &mut Net, b: &mut Button, parent: NodeId) -> R<bool> {
    let ps = net.bulbs.iter().find(|x| x.id == parent).and_then(|x| x.ext).and_then(|e| net.short_of(e)).unwrap();
    b.parent = ps;
    b.parent_id = parent;
    net.s.w.peers[peer_ix(b.id)].st.ed_parent = Some(ps);
    let mark = net.cap.len();
    b.mac_cmd(net, &[0x01, CAP], ps, true, true);
    net.run(500.0)?;
    let mut short = None;
    for _ in 0..4 {
        let seq = b.mac_cmd(net, &[0x04], ps, true, true);
        let _ = b.mac_wait(net, seq)?;
        net.run(60.0)?;
        if let Some(f) = b.frames_for(net, mark).iter().find(|f| f.mac_cmd() == Some(0x02)) {
            let p = &f.mac.as_ref().unwrap().payload;
            if p[3] == 0 {
                short = Some(u16::from_le_bytes([p[1], p[2]]));
            }
            break;
        }
    }
    let Some(short) = short else { return Ok(false) };
    net.s.w.peers[peer_ix(b.id)].st.short = Some(short);
    net.s.w.air.keys.learn(Some(short), Some(BUTTON_EXT));
    net.set_truth(BUTTON_EXT, short, "button");
    let mut key = false;
    for _ in 0..12 {
        let fs = b.poll(net)?.unwrap_or_default();
        if fs.iter().any(|f| f.aps_cmd() == Some(0x05)) {
            key = true;
            break;
        }
        net.run(250.0)?;
    }
    b.annce(net);
    net.run(300.0)?;
    {
        let pext = net.bulbs.iter().find(|x| x.id == parent).and_then(|x| x.ext).unwrap_or(0);
        let (st, air) = b.st(net);
        st.tx_nwk_cmd(air, ps, &[0x0B, 8, 0], &Tx::new().radius(1).dst_ieee(pext).mac_dst(ps).ar(true));
    }
    net.run(100.0)?;
    let _ = b.poll(net)?;
    Ok(key)
}

fn run_prev(cx: &mut SimCtx) -> R<()> {
    run_with(cx, Choice::PrevThenBest)
}

fn run_nodst(cx: &mut SimCtx) -> R<()> {
    run_with(cx, Choice::EdInitiator)
}

fn button_entry(net: &mut Net, bulb: &str) -> R<String> {
    let Some(s) = net.short_of_bulb(bulb) else { return Ok("?".into()) };
    let Some(t) = net.lqi_table(s)? else { return Ok("no Mgmt_Lqi answer".into()) };
    let rel = ["parent", "child", "sibling", "none", "previous child", "?", "?", "?"];
    let e: Vec<String> = t
        .iter()
        .filter(|x| x.1 == BUTTON_EXT)
        .map(|(s, _, f, _, _)| format!("{s:#06x} {} (type {})", rel[((f >> 4) & 7) as usize], f & 3))
        .collect();
    Ok(if e.is_empty() { "no entry".into() } else { e.join(", ") })
}

fn run_b(net: &mut Net, b: &mut Button, ms: f64) -> R<()> {
    let end = net.now() + ms;
    while net.now() < end {
        b.tick(net)?;
        net.run(50.0)?;
    }
    Ok(())
}

fn probe(net: &mut Net, b: &mut Button, label: &str) -> R<String> {
    let bshort = b.short(net);
    let mark = net.cap.len();
    net.coord_rreq(bshort, None, 0x61u8.wrapping_add(mark as u8));
    run_b(net, b, 1500.0)?;
    let reps: Vec<CapFrame> = net.cap[mark..]
        .iter()
        .filter(|f| f.nwk_cmd() == Some(0x02) && f.nwk.as_ref().is_some_and(|n| n.dst == 0x0000) && f.who != NodeId::Coord)
        .cloned()
        .collect();
    let replies: Vec<String> = reps
        .iter()
        .map(|f| {
            let p = f.nwk.as_ref().unwrap().payload.clone().unwrap_or_default();
            format!("{} (responder {:#06x})", f.who_name, u16::from_le_bytes([p[5], p[6]]))
        })
        .collect();
    let first_hop = reps.first().and_then(|f| f.mac.as_ref().and_then(|m| m.src)).map(|s| s as u16);
    let mark = net.cap.len();
    let mut p = ext_bytes(BUTTON_EXT).to_vec();
    p.extend_from_slice(&[0, 0]);
    net.coord_zdo(0xFFFD, 0x0000, &p);
    run_b(net, b, 2000.0)?;
    let mut rsp: Vec<String> = net.cap[mark..]
        .iter()
        .filter(|f| f.who != NodeId::Coord && f.zdo().is_some_and(|z| z.cluster == 0x8000) && f.nwk.as_ref().is_some_and(|n| n.dst == 0x0000))
        .filter(|f| f.mac.as_ref().and_then(|m| m.dst) == Some(0))
        .map(|f| {
            let z = f.zdo().unwrap().payload;
            format!("{} says {:#06x} (status {:#04x})", f.who_name, u16::from_le_bytes([z[9], z[10]]), z[0])
        })
        .collect();
    rsp.dedup();
    let parent_name = net.name_of(b.parent_id);
    let mut hops: Vec<u16> = Vec::new();
    for f in &reps {
        if let Some(s) = f.mac.as_ref().and_then(|m| m.src) {
            if !hops.contains(&(s as u16)) {
                hops.push(s as u16);
            }
        }
    }
    if hops.is_empty() {
        hops.extend(first_hop.or(net.short_of_bulb(&parent_name)));
    }
    let ps = net.short_of_bulb(&parent_name);
    hops.sort_by_key(|h| Some(*h) == ps);
    let mut out = Vec::new();
    for via in hops {
        let n_rx = b.received.len();
        let mut seqs = Vec::new();
        for _ in 0..3 {
            net.s.w.coord.st.via.insert(bshort, via);
            let (st, air) = net.s.w.st(NodeId::Coord);
            let s = st.tx_zcl(air, bshort, 0x0001, 0x00, &[0x21, 0x00], &Tx::new().general().discover(1));
            seqs.push(s.zcl_seq.unwrap_or(0));
            run_b(net, b, 10000.0)?;
        }
        let mut got: Vec<u8> = Vec::new();
        for r in &b.received[n_rx..] {
            for s in &seqs {
                if r.1.contains("cl=0x0001") && r.1.contains(&format!("seq={s} ")) && !got.contains(s) {
                    got.push(*s);
                }
            }
        }
        let vname = net.owner_of(via).map(|o| o.1).unwrap_or_else(|| "?".into());
        out.push(format!("3 Read Attributes through {vname}: {} reached the button", got.len()));
    }
    Ok(format!(
        "{label}: route replies [{}]; NWK_addr_rsp [{}]; {} (its parent {parent_name})",
        replies.join(", "),
        rsp.join(", "),
        out.join("; ")
    ))
}

fn run_with(cx: &mut SimCtx, choice: Choice) -> R<()> {
    let img = cx.image("bulb", "stock/stock-tw.bin");
    let telink_free = super::util::is_outglow(&img);
    let echo = cx.verbose > 0;
    let off_s: f64 = std::env::var("ENDDEV_OFF_S").ok().and_then(|v| v.parse().ok()).unwrap_or(120.0);
    cx.say(&format!("image: {}; parent choice {choice:?}; parent off for {off_s} s", super::util::rel(&img)));
    let sg = bulb_node(&img, cx.cpi, "SG", 0xa4c1380000000061)?;
    let sw = bulb_node(&img, cx.cpi, "SW", 0xa4c1380000000062)?;
    let mut net = Net::new(sg, Coord::new_plain(11, NWK_KEY), echo);
    let swid = net.add_bulb(sw, 700.0);
    let sgid = NodeId::Dut;
    let ok = net.run_until(60000.0, |n| n.bulbs.iter().all(|b| n.short_of_bulb(&b.name).is_some()))?;
    cx.check(net.now(), "bulbs join", ok, "");
    net.run(20000.0)?;
    let c = &net.s.w.coord.st;
    let mut st = Station::new(NodeId::Coord, c.channel, c.pan, None, BUTTON_EXT, NWK_KEY, (0x71, 0x72, 0x73, 0x74, 0x75), 0x10, 0);
    st.ed_initiator = choice == Choice::EdInitiator;
    st.key_seq = c.key_seq;
    let bid = net.s.w.add_peer_node(Peer::bare_ack(st, "button"));
    net.out_of_range(bid, NodeId::Coord, true);
    let mut b = Button {
        id: bid,
        parent: 0,
        parent_id: sgid,
        state: State::Joined,
        fails: 0,
        next_poll: 0.0,
        next_press: 0.0,
        presses: Vec::new(),
        annces: Vec::new(),
        rejoins: Vec::new(),
        received: Vec::new(),
        choice,
        dst_ieee: true,
        leaves: 0,
        quality: vec![(sgid, 200), (swid, 120)],
        seen_rx: 0,
    };
    net.s.w.coord.permit_joining(&mut net.s.w.air, 254);
    net.run(1000.0)?;
    let ok = join_via(&mut net, &mut b, sgid)?;
    net.s.w.coord.permit_joining(&mut net.s.w.air, 0);
    let bshort0 = b.short(&net);
    cx.check(net.now(), "button joins through SG", ok, &format!("{bshort0:#06x}"));
    let t = button_entry(&mut net, "SG")?;
    cx.say(&format!("   SG's entry for the button: {t}"));
    let now = net.now();
    b.next_poll = now + 1000.0;
    b.next_press = now + 2000.0;
    let run_button = |net: &mut Net, b: &mut Button, ms: f64| -> R<()> {
        let end = net.now() + ms;
        while net.now() < end {
            b.tick(net)?;
            net.run(50.0)?;
        }
        Ok(())
    };
    run_button(&mut net, &mut b, 20000.0)?;
    let before: Vec<(f64, bool)> = b.presses.clone();
    let ok_before = before.iter().filter(|p| p.1).count();
    cx.check(net.now(), "presses go through SG", ok_before == before.len() && !before.is_empty(),
             &format!("{ok_before} of {} acknowledged", before.len()));
    let n_press = b.presses.len();
    net.power_off("SG");
    let t_off = net.now();
    run_button(&mut net, &mut b, off_s * 1000.0)?;
    let during: Vec<(f64, bool)> = b.presses[n_press..].to_vec();
    let lost = during.iter().filter(|p| !p.1).count();
    let rejoined = b.rejoins.iter().find(|r| r.0 >= t_off && r.1.contains("rejoined")).cloned();
    let first_ok_after = during.iter().find(|p| p.1).map(|p| (p.0 - t_off) / 1000.0);
    let refuses = choice == Choice::EdInitiator && !telink_free;
    cx.check(
        net.now(),
        if refuses {
            "SG off: Telink's stack refuses a rejoin with the end device initiator bit (docs/address-conflicts.md)"
        } else {
            "SG off: the button finds another parent"
        },
        rejoined.is_some() != refuses,
        &format!(
            "presses while SG is off: {} sent, {lost} lost; first one through after {}; rejoin: {}; Device_annce {}; NWK Leave received {}; rejoin attempts {}",
            during.len(),
            first_ok_after.map(|t| format!("{t:.1} s")).unwrap_or_else(|| "never".into()),
            rejoined.as_ref().map(|r| format!("{:.1} s after the switch, {}", (r.0 - t_off) / 1000.0, r.1)).unwrap_or_else(|| "none".into()),
            b.annces.iter().filter(|a| a.0 >= t_off).count(),
            b.leaves,
            b.rejoins.iter().filter(|r| r.0 >= t_off && r.1.starts_with("attempt")).count()
        ),
    );
    net.power_on("SG")?;
    let t_on = net.now();
    let n_press = b.presses.len();
    let mut stale = Vec::new();
    let mut probes = Vec::new();
    for wait in [5.0, 20.0, 40.0, 60.0, 120.0, 240.0] {
        let target = t_on + wait * 1000.0;
        let left = target - net.now();
        if left > 0.0 {
            run_b(&mut net, &mut b, left)?;
        }
        let e = button_entry(&mut net, "SG")?;
        stale.push(format!("+{wait:.0} s: {e}"));
        if wait == 5.0 || wait == 240.0 {
            let t = probe(&mut net, &mut b, &format!("+{wait:.0} s"))?;
            probes.push(t);
        }
    }
    cx.check(net.now(), "SG on: its neighbour table about the button", true, &stale.join("; "));
    for p in probes {
        cx.check(net.now(), "after SG is back: route request, address request, coordinator -> button", true, &p);
    }
    let sg_to_button: Vec<String> = net
        .cap
        .iter()
        .filter(|f| f.t0 >= t_on && f.who == sgid && f.nwk.as_ref().is_some_and(|n| n.dst == b.short(&net) || n.dst_ieee == Some(BUTTON_EXT)))
        .map(|f| format!("{:.1} s {}", (f.t0 - t_on) / 1000.0, f.brief()))
        .collect();
    cx.say(&format!("   SG's frames for the button after power-up: [{}]", sg_to_button.join(" | ")));
    let parent_name = net.name_of(b.parent_id);
    let after: Vec<(f64, bool)> = b.presses[n_press..].to_vec();
    let lost_after = after.iter().filter(|p| !p.1).count();
    cx.check(net.now(), "presses after SG is back", true, &format!("{} sent, {lost_after} lost; button parent {parent_name}", after.len()));
    let ann: Vec<String> = b.annces.iter().map(|a| format!("{:.1} s {:#06x} via {}", a.0 / 1000.0, a.1, a.2)).collect();
    cx.say(&format!("   Device_annce of the button: [{}]", ann.join("; ")));
    for r in &b.rejoins {
        cx.say(&format!("   [{:9.3} s] rejoin: {}", r.0 / 1000.0, r.1));
    }
    if let Ok(w) = std::env::var("ENDDEV_DUMP") {
        let (a, z) = w.split_once(':').map(|(a, z)| (a.parse::<f64>().unwrap_or(0.0), z.parse::<f64>().unwrap_or(0.0))).unwrap_or((0.0, 0.0));
        let i0 = net.cap.iter().position(|f| f.t0 >= a * 1000.0).unwrap_or(0);
        let i1 = net.cap.iter().position(|f| f.t0 > z * 1000.0).unwrap_or(net.cap.len());
        for l in net.dump(i0, i1, |f| f.nwk_cmd() != Some(0x08) && f.mac.as_ref().is_some_and(|m| m.ty != 2)) {
            cx.say(&l);
        }
    }
    if cx.verbose > 0 {
        for n in net.notes.clone() {
            cx.say(&format!("[{:9.3} s] {}", n.ms / 1000.0, n.text));
        }
    }
    let _ = swid;
    Ok(())
}
