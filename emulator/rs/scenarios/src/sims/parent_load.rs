//! The bulb as a parent under load: many sleepy end devices joined through
//! it, polling at different intervals, while the coordinator sends them
//! unicasts and broadcasts (no Python original; a measurement of the parent
//! role of whatever image is under test, the Rust one by default).
//!
//! The bulb joins the coordinator; 24 scripted end devices (receiver off when
//! idle) join through it, a third each polling every 1 s, 5 s and 30 s. A
//! poll is a MAC Data Request; when the frame the bulb then hands over says
//! "frame pending", the device polls again at once, as end devices do. Four
//! phases of traffic from the coordinator:
//!
//! 1. steady: one unicast every 1.5 s to the children in turn, three broadcasts
//!    (each on the air three times, 500 ms apart, as routers retransmit them);
//! 2. flood: eight unicasts at once to one child polling every 5 s, one to
//!    each child polling every 1 s right after;
//! 3. burst: two unicasts to every child at once, and a broadcast;
//! 4. BLE: the steady traffic again while a BLE central is connected to the
//!    bulb at a 7.5 ms interval (the radio is away from 802.15.4 for most of
//!    every connection event).
//!
//! Every frame for a child is followed to its fate: delivered on a poll,
//! dropped (the child polled within the persistence time, 7.68 s, and did
//! not get it: the parent had no room for it or lost it), expired (no poll in
//! time: nothing a parent can do), or never acknowledged by the bulb. The
//! requirements are few and clear (all join; a child polling every second
//! gets every unicast; nothing dropped in steady traffic; broadcasts reach the
//! children that poll within the persistence time; one child's flood does not
//! push out the others' frames); the rest is printed as observations.

use std::collections::VecDeque;

use super::{ble_central, SimCtx, SimDef};
use crate::air::NodeId;
use crate::session::{CapFrame, Session, SessionOpts, R};
use crate::station::Tx;

pub fn def() -> SimDef {
    SimDef { name: "parent_load", doc: "sleepy end devices polling the bulb under unicast and broadcast load", run }
}

const N_EDS: usize = 24;
const EVERY: [f64; 3] = [1000.0, 5000.0, 30000.0];
const EXT: u64 = 0x00124B00EE000000;
/// macTransactionPersistenceTime.
const PERSISTENCE_MS: f64 = 7680.0;
/// A poll this close to the end of the persistence time does not count as a
/// chance to deliver (the frame was queued a little after it reached the bulb).
const MARGIN_MS: f64 = 150.0;
/// macAckWaitDuration (54 symbols).
const ACK_WAIT_US: f64 = 864.0;
const PHASES: [&str; 4] = ["steady", "flood", "burst", "BLE"];

struct Ed {
    id: NodeId,
    name: String,
    every: f64,
    short: Option<u16>,
    next: f64,
    /// (time, acknowledged, frame pending in the acknowledgement, frames received)
    polls: Vec<(f64, bool, bool, usize)>,
    /// frames received after a poll: (time, NWK source, NWK destination, NWK sequence number)
    got: Vec<(f64, u16, u16, u8)>,
}

#[derive(Clone, Copy)]
enum Act {
    Unicast(usize),
    Broadcast,
}

/// A frame the coordinator sent: phase, child (None: broadcast), NWK sequence number, when.
struct Sent {
    phase: usize,
    child: Option<usize>,
    seq: u8,
    t: f64,
}

#[derive(Default, Clone, Copy)]
struct Fate {
    sent: usize,
    delivered: usize,
    dropped: usize,
    expired: usize,
    lost: usize,
}

impl Fate {
    fn add(&mut self, o: &Fate) {
        self.sent += o.sent;
        self.delivered += o.delivered;
        self.dropped += o.dropped;
        self.expired += o.expired;
        self.lost += o.lost;
    }
    fn text(&self) -> String {
        format!(
            "{} sent, {} delivered, {} dropped, {} expired, {} not at the bulb",
            self.sent, self.delivered, self.dropped, self.expired, self.lost
        )
    }
}

fn class_name(every: f64) -> String {
    format!("every {:.0} s", every / 1000.0)
}

/// A poll, and again at once while the frames handed over say more are pending.
fn poll(s: &mut Session, ed: &mut Ed) -> R<()> {
    for _ in 0..8 {
        let p = s.ed_poll(ed.id, 30.0, false)?;
        let mut more = false;
        for f in &p.frames {
            if let Some(n) = &f.nwk {
                ed.got.push((f.t0, n.src, n.dst, n.seq));
            }
            more |= f.mac.as_ref().is_some_and(|m| m.pending);
        }
        ed.polls.push((p.t, p.acked, p.pending, p.frames.len()));
        if !more {
            break;
        }
    }
    Ok(())
}

/// Runs polls and the coordinator's actions (times from now) for `ms`.
fn drive(s: &mut Session, eds: &mut [Ed], acts: &[(f64, Act)], ms: f64, phase: usize, sent: &mut Vec<Sent>) -> R<()> {
    let t0 = s.now();
    let end = t0 + ms;
    let mut acts: VecDeque<(f64, Act)> = acts.iter().map(|(t, a)| (t0 + t, *a)).collect();
    for (i, ed) in eds.iter_mut().enumerate().filter(|(_, e)| e.next == 0.0) {
        // the first phase: the children of a class spread evenly over their
        // interval; from then on each keeps its own rhythm across the phases
        let (k, n) = ((i / EVERY.len()) as f64, N_EDS.div_ceil(EVERY.len()) as f64);
        ed.next = t0 + ed.every * (k + 0.5) / n;
    }
    loop {
        let ta = acts.front().map_or(f64::INFINITY, |a| a.0);
        let (ip, tp) = eds
            .iter()
            .enumerate()
            .filter(|(_, e)| e.short.is_some())
            .map(|(i, e)| (i, e.next))
            .fold((usize::MAX, f64::INFINITY), |b, x| if x.1 < b.1 { x } else { b });
        let t = ta.min(tp);
        if t >= end {
            let now = s.now();
            if end > now {
                s.run(end - now)?;
            }
            return Ok(());
        }
        let now = s.now();
        if t > now {
            s.run(t - now)?;
        }
        if ta <= tp {
            let (_, a) = acts.pop_front().unwrap();
            match a {
                Act::Unicast(c) => {
                    let Some(short) = eds[c].short else { continue };
                    let r = s.tx_zcl(0x0003, 0x01, &[], Tx::new().ddr().dst(short));
                    sent.push(Sent { phase, child: Some(c), seq: r.nwk_seq, t: s.now() });
                }
                Act::Broadcast => {
                    let r = s.tx_zcl(0x0003, 0x01, &[], Tx::new().ddr().radius(4).dst(0xFFFF));
                    // and twice more, as a router retransmits a broadcast (nwkMaxBroadcastRetries 2,
                    // R22 3.6.5): the bulb may be away on BLE when one goes by
                    for k in 1..3 {
                        s.w.coord.st.resend_last(&mut s.w.air, 500.0 * k as f64);
                    }
                    sent.push(Sent { phase, child: None, seq: r.nwk_seq, t: s.now() });
                }
            }
        } else {
            poll(s, &mut eds[ip])?;
            eds[ip].next += eds[ip].every;
        }
    }
}

/// The steady traffic: a unicast every 1.5 s to the children in turn, broadcasts at 10, 25 and 40 s.
fn steady(n_joined: &[usize]) -> Vec<(f64, Act)> {
    let mut v: Vec<(f64, Act)> = Vec::new();
    if !n_joined.is_empty() {
        for k in 0..30 {
            v.push((500.0 + 1500.0 * k as f64, Act::Unicast(n_joined[k % n_joined.len()])));
        }
    }
    for t in [10_000.0, 25_000.0, 40_000.0] {
        v.push((t + 250.0, Act::Broadcast));
    }
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    v
}

/// The coordinator's frame (an acknowledged copy) and when it ended, for a unicast.
fn on_air(cap: &[CapFrame], x: &Sent, short: u16) -> Option<f64> {
    let copies: Vec<&CapFrame> = cap
        .iter()
        .filter(|f| {
            f.who == NodeId::Coord
                && f.t0 >= x.t - 1.0
                && f.t0 < x.t + 3000.0
                && f.nwk.as_ref().is_some_and(|n| n.src == 0 && n.dst == short && n.seq == x.seq)
        })
        .collect();
    for f in copies {
        let seq = f.mac.as_ref()?.seq;
        let acked = cap[f.i + 1..(f.i + 4).min(cap.len())]
            .iter()
            .any(|a| a.who == NodeId::Dut && a.mac.as_ref().is_some_and(|m| m.ty == 2 && m.seq == seq));
        if acked {
            return Some(f.t1);
        }
    }
    None
}

/// When the coordinator's broadcast ended on the air, if the bulb took it in
/// (it passed it on, by broadcast or to a child).
fn bcast_on_air(cap: &[CapFrame], x: &Sent) -> Option<f64> {
    let is = |f: &CapFrame| {
        f.t0 >= x.t - 1.0 && f.t0 < x.t + 3000.0 && f.nwk.as_ref().is_some_and(|n| n.src == 0 && n.dst == 0xFFFF && n.seq == x.seq)
    };
    let first = cap.iter().find(|f| f.who == NodeId::Coord && is(f))?;
    cap.iter().any(|f| f.who == NodeId::Dut && is(f)).then_some(first.t1)
}

fn fate(ed: &Ed, t_air: Option<f64>, dst: u16, seq: u8) -> Fate {
    let mut r = Fate { sent: 1, ..Fate::default() };
    let Some(t) = t_air else {
        r.lost = 1;
        return r;
    };
    let got = ed.got.iter().any(|g| g.0 >= t && g.0 <= t + PERSISTENCE_MS + 2000.0 && g.1 == 0 && g.2 == dst && g.3 == seq);
    let chance = ed.polls.iter().any(|p| p.1 && p.0 > t + 20.0 && p.0 < t + PERSISTENCE_MS - MARGIN_MS);
    if got {
        r.delivered = 1;
    } else if chance {
        r.dropped = 1;
    } else {
        r.expired = 1;
    }
    r
}

fn run(cx: &mut SimCtx) -> R<()> {
    let img = cx.image("bin", "target/tc32/bulb-tc32.bin");
    let mut s = Session::new(&img, SessionOpts { cpi: cx.cpi, log: cx.log(), ..Default::default() })?;
    cx.say(&format!(
        "== parent: {}; {N_EDS} sleepy end devices polling every 1, 5 and 30 s; channel 11",
        super::util::rel(&img)
    ));
    if let Some(l) = super::util::stack_rearm(&mut s.w.air.dut, &img) {
        cx.say(&l);
    }
    let mut eds: Vec<Ed> = Vec::new();
    let mut refused = Vec::new();
    for i in 0..N_EDS {
        let name = format!("ed{i:02}");
        let id = s.end_device(&name, EXT + i as u64, 0x80, true)?;
        let r = s.ed_join(id, Some(8), true)?;
        let short = if r.ok { r.short } else { None };
        if !r.ok {
            refused.push(format!("{name}: {}", r.why));
            s.remove_peer(id);
        }
        eds.push(Ed { id, name, every: EVERY[i % 3], short, next: 0.0, polls: Vec::new(), got: Vec::new() });
    }
    let joined: Vec<usize> = (0..N_EDS).filter(|i| eds[*i].short.is_some()).collect();
    cx.check(
        s.now(),
        &format!("all {N_EDS} end devices join through the bulb"),
        joined.len() == N_EDS,
        &format!("{} joined{}", joined.len(), if refused.is_empty() { String::new() } else { format!("; first refusal: {}", refused[0]) }),
    );
    let mut sent: Vec<Sent> = Vec::new();
    let mut t_phase = [0.0f64; 4];

    // 1. steady
    t_phase[0] = s.now();
    drive(&mut s, &mut eds, &steady(&joined), 47_000.0, 0, &mut sent)?;
    // 2. flood: eight frames at once to a child polling every 5 s, then one to each child polling every second
    t_phase[1] = s.now();
    let flooded = joined.iter().copied().find(|i| eds[*i].every == 5000.0);
    let mut acts: Vec<(f64, Act)> = Vec::new();
    if let Some(c) = flooded {
        for k in 0..8 {
            acts.push((200.0 + 5.0 * k as f64, Act::Unicast(c)));
        }
    }
    for (k, c) in joined.iter().filter(|i| eds[**i].every == 1000.0).enumerate() {
        acts.push((300.0 + 5.0 * k as f64, Act::Unicast(*c)));
    }
    drive(&mut s, &mut eds, &acts, 12_000.0, 1, &mut sent)?;
    // 3. burst: two frames to every child at once, and a broadcast
    t_phase[2] = s.now();
    let mut acts: Vec<(f64, Act)> = Vec::new();
    for round in 0..2 {
        for (k, c) in joined.iter().enumerate() {
            acts.push((200.0 + 300.0 * round as f64 + 5.0 * k as f64, Act::Unicast(*c)));
        }
    }
    acts.push((900.0, Act::Broadcast));
    acts.sort_by(|a, b| a.0.total_cmp(&b.0));
    drive(&mut s, &mut eds, &acts, 12_000.0, 2, &mut sent)?;
    // 4. the steady traffic with a BLE central connected at 7.5 ms
    t_phase[3] = s.now();
    ble_central::connect_central(&mut s.w, NodeId::Dut, 6, None);
    let connected = s.run_until(15_000.0, 20.0, |s| s.w.tap(NodeId::Dut).is_some_and(|t| zbnode::bleatt::started(&t.q)))?;
    cx.say(&format!("   BLE central {}", if connected { "connected at 7.5 ms" } else { "did not connect" }));
    let ev0 = s.w.tap(NodeId::Dut).map(|t| t.c.borrow().stats).unwrap_or_default();
    drive(&mut s, &mut eds, &steady(&joined), 47_000.0, 3, &mut sent)?;
    let ev1 = s.w.tap(NodeId::Dut).map(|t| t.c.borrow().stats).unwrap_or_default();
    let state = s.w.tap(NodeId::Dut).map(|t| t.c.borrow().state.to_string()).unwrap_or_else(|| "none".into());
    cx.say(&format!(
        "   BLE during phase 4: {state}, {} connection events, {} unanswered",
        ev1.events - ev0.events,
        ev1.missed - ev0.missed
    ));
    s.sync();
    let cap = s.cap.clone();

    // fates
    let mut uni = vec![[Fate::default(); N_EDS]; PHASES.len()];
    let mut bc = vec![[Fate::default(); N_EDS]; PHASES.len()];
    for x in &sent {
        match x.child {
            Some(c) => {
                let short = eds[c].short.unwrap();
                let f = fate(&eds[c], on_air(&cap, x, short), short, x.seq);
                uni[x.phase][c].add(&f);
            }
            None => {
                let t = bcast_on_air(&cap, x);
                for &c in &joined {
                    let f = fate(&eds[c], t, 0xFFFF, x.seq);
                    bc[x.phase][c].add(&f);
                }
            }
        }
    }
    let class = |v: &[Fate; N_EDS], every: f64, only: Option<usize>| -> Fate {
        let mut t = Fate::default();
        for &c in &joined {
            if eds[c].every == every && only.is_none_or(|o| o == c) {
                t.add(&v[c]);
            }
        }
        t
    };
    for (p, name) in PHASES.iter().enumerate() {
        cx.say(&format!("   phase {} ({name}):", p + 1));
        for every in EVERY {
            let u = class(&uni[p], every, None);
            let b = class(&bc[p], every, None);
            let n = joined.iter().filter(|c| eds[**c].every == every).count();
            cx.say(&format!("     {n:2} children {:>11}: unicast {}; broadcast {}", class_name(every), u.text(), b.text()));
        }
    }
    cx.say("   per child (unicast | broadcast, all phases: sent/delivered/dropped/expired/not at the bulb):");
    for &c in &joined {
        let (mut u, mut b) = (Fate::default(), Fate::default());
        for p in 0..PHASES.len() {
            u.add(&uni[p][c]);
            b.add(&bc[p][c]);
        }
        let e = &eds[c];
        let polls = e.polls.len();
        let unacked = e.polls.iter().filter(|p| !p.1).count();
        cx.say(&format!(
            "     {} ({:#06x}, {:>11}): {}/{}/{}/{}/{} | {}/{}/{}/{}/{}; {polls} polls, {unacked} not acknowledged",
            e.name,
            e.short.unwrap(),
            class_name(e.every),
            u.sent,
            u.delivered,
            u.dropped,
            u.expired,
            u.lost,
            b.sent,
            b.delivered,
            b.dropped,
            b.expired,
            b.lost
        ));
    }

    // acknowledgements of the polls
    // (every transmission of a Data Request, retries included; an acknowledgement
    // counts when it starts within macAckWaitDuration after the request)
    let ed_ids: Vec<NodeId> = eds.iter().map(|e| e.id).collect();
    let mut delays: [Vec<f64>; 2] = [Vec::new(), Vec::new()];
    let mut no_ack = [0usize; 2];
    for f in &cap {
        let Some(m) = &f.mac else { continue };
        if !ed_ids.contains(&f.who) || m.ty != 3 || m.payload.first() != Some(&0x04) || f.t0 < t_phase[0] {
            continue;
        }
        let k = (f.t0 >= t_phase[3]) as usize;
        let ack = cap[f.i + 1..(f.i + 4).min(cap.len())].iter().find(|a| {
            a.who == NodeId::Dut && a.mac.as_ref().is_some_and(|x| x.ty == 2 && x.seq == m.seq) && (a.t0 - f.t1) * 1000.0 <= ACK_WAIT_US
        });
        match ack {
            Some(a) => delays[k].push((a.t0 - f.t1) * 1000.0),
            None => no_ack[k] += 1,
        }
    }
    let span = |v: &[f64]| v.iter().fold((f64::INFINITY, 0.0f64), |(l, h), d| (l.min(*d), h.max(*d)));
    let (lo, hi) = span(&delays[0]);
    let (blo, bhi) = span(&delays[1]);
    let all_polls: Vec<&(f64, bool, bool, usize)> = joined.iter().flat_map(|c| eds[*c].polls.iter()).collect();
    let no_bit = all_polls.iter().filter(|p| p.3 > 0 && !p.2).count();
    let idle_bit = all_polls.iter().filter(|p| p.2 && p.3 == 0).count();
    cx.say(&format!(
        "   Data Request transmissions: phases 1-3 {} acknowledged ({lo:.0}..{hi:.0} µs after the request), {} not; \
         phase 4 (BLE) {} acknowledged ({blo:.0}..{bhi:.0} µs), {} not (the end device retries)",
        delays[0].len(),
        no_ack[0],
        delays[1].len(),
        no_ack[1]
    ));
    // (A frame that reaches the bulb just before a poll, and is taken from the
    // receive queue just after it, is not in the acknowledgement yet; the
    // bulb sends it anyway. A sleeping child would miss it and the frame
    // wait for its next poll.)
    cx.say(&format!("   polls that got frames without the pending bit: {no_bit}; with the bit and nothing: {idle_bit}"));
    let (sent_n, recv_n, why) = {
        let r = s.w.air.dut.radio.b();
        (r.sent, r.received, r.dropped.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join(", "))
    };
    // the firmware's own count of frames its receive queue had no room for (Rust image)
    let overflow = img.symbol("RADIO_DROPPED").and_then(|a| {
        let m = &s.w.air.dut.m;
        let o = a.checked_sub(0x840000)? as usize;
        Some(u32::from_le_bytes(m.sram.get(o..o + 4)?.try_into().ok()?))
    });
    cx.say(&format!(
        "   bulb radio: sent {sent_n}, received {recv_n}, not received ({}); receive queue full: {}",
        if why.is_empty() { "none".to_string() } else { why },
        overflow.map_or("-".to_string(), |n| n.to_string())
    ));

    // requirements
    let now = s.now();
    let one_s = class(&uni[0], 1000.0, None);
    cx.check(now, "steady traffic: a child polling every 1 s gets every unicast", one_s.sent > 0 && one_s.delivered == one_s.sent,
             &one_s.text());
    let mut all = Fate::default();
    for every in EVERY {
        all.add(&class(&uni[0], every, None));
    }
    cx.check(now, "steady traffic: no unicast dropped while its child polls within the persistence time", all.dropped == 0 && all.lost == 0,
             &all.text());
    let mut b = class(&bc[0], 1000.0, None);
    b.add(&class(&bc[0], 5000.0, None));
    cx.check(now, "steady traffic: every broadcast reaches each child polling every 1 s or 5 s", b.sent > 0 && b.delivered == b.sent,
             &b.text());
    let b30 = class(&bc[0], 30000.0, None);
    cx.check(now, "steady traffic: no broadcast dropped for a child that polls within the persistence time", b30.dropped == 0,
             &format!("children polling every 30 s: {}", b30.text()));
    let fl = class(&uni[1], 1000.0, None);
    cx.check(now, "flood: eight frames for one child do not push out the frames of the children polling every 1 s",
             fl.sent > 0 && fl.delivered == fl.sent, &fl.text());
    if let Some(c) = flooded {
        cx.say(&format!("   flood: the flooded child {} (every 5 s) got {} of {}", eds[c].name, uni[1][c].delivered, uni[1][c].sent));
    }
    let bu1 = class(&uni[2], 1000.0, None);
    cx.say(&format!("   burst: children polling every 1 s: unicast {}", bu1.text()));
    let ble1 = class(&uni[3], 1000.0, None);
    cx.check(now, "BLE connected at 7.5 ms: a child polling every 1 s still gets every unicast",
             connected && ble1.sent > 0 && ble1.delivered == ble1.sent, &ble1.text());
    let lost_polls = all_polls.iter().filter(|p| !p.1 && p.0 < t_phase[3]).count();
    cx.check(now, "phases 1-3: every poll is acknowledged (with the end device's MAC retries)", lost_polls == 0,
             &format!("{lost_polls} polls without an acknowledgement"));
    cx.check(now, "the acknowledgement of a Data Request starts within aTurnaroundTime (192 µs) after it, however many children",
             !delays[0].is_empty() && hi <= 192.0 && bhi <= 192.0, &format!("{lo:.0}..{:.0} µs", hi.max(bhi)));
    let resets = s.w.resets;
    cx.check(now, "no resets, nobody left the network", resets == 0 && !s.w.coord.has_event("leave"), &format!("resets {resets}"));
    if let Some(l) = super::util::stack_report(&s.w.air.dut, &img) {
        cx.say(&l);
    }
    let okn = cx.results.iter().filter(|r| r.1).count();
    cx.say(&format!(
        "\n== result: {okn}/{} checks passed; simulation time {:.1} s, wall time {:.0} s",
        cx.results.len(),
        s.now() / 1000.0,
        cx.t_wall.elapsed().as_secs_f64()
    ));
    Ok(())
}
