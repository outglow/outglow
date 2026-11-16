//! The four stress scenarios over the house mesh of `net.rs`.

use super::net::{Flow, Mesh, GROUP};
use super::{row, Row, StressResult};
use crate::air::NodeId;
use crate::frames::airtime_us;
use crate::session::R;
use crate::station::Tx;

/// The traffic mix (weights): Z2M-like control and polling of the bulbs,
/// reports and commands of other routers through the mesh.
pub const MIX: &[(Flow, u32)] =
    &[(Flow::CmdBulb, 35), (Flow::ReadBulb, 20), (Flow::Report, 20), (Flow::PlugReadBulb, 15), (Flow::CmdPlug, 10)];

const DRAIN: f64 = 4000.0;

fn f1(x: Option<f64>) -> String {
    x.map(|v| format!("{v:.0}")).unwrap_or_else(|| "-".into())
}

/// Many-to-one route requests every 30 s from now on, and light traffic so that routes form.
pub fn warm_up(m: &mut Mesh) -> R<()> {
    m.start_mtorr(30000.0);
    m.phase("warm-up 0.5/s");
    let ms = if m.cfg.full { 40000.0 } else { 30000.0 };
    m.traffic(ms, 0.5, MIX)?;
    m.tick(DRAIN)
}

/// Bulbs whose light does not follow the state command they answered last
/// (the order the bulb executed them in; routes may reorder what was sent).
/// Skipped: a bulb with a state command that got no answer (it may or may
/// not have run it).
fn light_mismatch(m: &Mesh, since: f64) -> (usize, Vec<String>) {
    let mut checked = 0;
    let mut bad = Vec::new();
    for b in &m.bulbs {
        if m.s.w.air.off.contains(&b.id) {
            continue;
        }
        let cmds: Vec<_> = m.msgs.iter().filter(|x| x.t >= since && x.flow == Flow::CmdBulb && x.dst == b.short).collect();
        if cmds.is_empty() || cmds.iter().any(|x| x.answered.is_none()) {
            continue;
        }
        let last = cmds.iter().max_by(|a, b| a.answered.partial_cmp(&b.answered).unwrap()).unwrap();
        checked += 1;
        let on = m.light_on(b.id);
        if Some(on) != last.want_on {
            bad.push(format!("{} is {} after {} (cluster {:#06x} cmd {:#04x} seq {}) sent at {:.3} s, answered at {:.3} s",
                             b.name, if on { "on" } else { "off" }, if last.want_on == Some(true) { "on" } else { "off" },
                             last.cluster, last.cmd, last.seq, last.t / 1000.0, last.answered.unwrap_or(-1.0) / 1000.0));
        }
    }
    (checked, bad)
}

/// The checks every scenario ends with.
pub(super) fn final_checks(m: &Mesh, res: &mut StressResult, light: (usize, Vec<String>), churn: bool) {
    let resets = m.resets();
    res.check("no watchdog or software reset of any bulb", resets == 0, format!("{resets} reset(s)"));
    let c = m.air_count(m.t_formed, f64::MAX, None);
    let mut moved = Vec::new();
    for b in &m.bulbs {
        if b.short != b.short0 {
            moved.push(format!("{}: {:#06x} -> {:#06x}", b.name, b.short0, b.short));
        }
    }
    let leaves = c.leave;
    let stay = moved.is_empty() && leaves == 0 && (churn || (c.assoc_req == 0 && c.rejoin_req == 0));
    res.check(
        "every bulb stays in the network with its address (no Leave; no association or rejoin unless power-cycled)",
        stay,
        format!("Leave {leaves}, association requests {}, rejoin requests {}{}", c.assoc_req, c.rejoin_req,
                if moved.is_empty() { String::new() } else { format!("; moved: {}", moved.join(", ")) }),
    );
    let dups: usize = m.msgs.iter().filter(|x| x.answer_ctrs.len() > 1).count();
    res.check("no request executed twice (one answer per request; R22 2.2.8.4.2 duplicate rejection)", dups == 0,
              format!("{dups} request(s) answered with more than one APS counter"));
    let (n, bad) = light;
    res.check("the light follows the command each bulb answered last (ZCL On/Off, Level)", bad.is_empty(),
              if bad.is_empty() { format!("{n} bulb state(s) checked") } else { bad.join("; ") });
}

pub(super) fn rows_from(m: &Mesh, res: &mut StressResult, from: usize) {
    for p in from..m.phases.len() {
        res.rows.push(row(m, p));
    }
}

fn merge_light(a: &mut (usize, Vec<String>), b: (usize, Vec<String>)) {
    a.0 += b.0;
    a.1.extend(b.1);
}

// ---------------------------------------------------------------------- stress_traffic
/// The phases of rising traffic; the light checks of them.
pub fn traffic_phases(m: &mut Mesh) -> R<(usize, Vec<String>)> {
    let rates: &[f64] = if m.cfg.full { &[1.0, 3.0, 6.0, 12.0, 20.0] } else { &[1.0, 4.0, 10.0] };
    let dur = if m.cfg.full { 30000.0 } else { 20000.0 };
    let mut light = (0, Vec::new());
    for r in rates {
        let t0 = m.now();
        m.phase(&format!("{r:.0} msg/s"));
        m.traffic(dur, *r, MIX)?;
        m.tick(DRAIN)?;
        merge_light(&mut light, light_mismatch(m, t0));
    }
    Ok(light)
}

pub fn traffic(m: &mut Mesh, res: &mut StressResult) -> R<()> {
    warm_up(m)?;
    let light = traffic_phases(m)?;
    m.end_phase();
    rows_from(m, res, 0);
    final_checks(m, res, light, false);
    Ok(())
}

// ---------------------------------------------------------------------- stress_noise
/// Junk frames (bad FCS, 4 ms) back to back from `j` while `on` says so.
fn jam(m: &mut Mesh, j: NodeId, next: &mut f64, on: &dyn Fn(f64) -> bool) {
    let now = m.now();
    if now < *next || !on(now) {
        return;
    }
    let n = 120usize;
    let mut psdu: Vec<u8> = (0..n).map(|k| (k as u32 * 37 + (now as u32) * 11) as u8).collect();
    psdu[0] = 0x07; // reserved frame type: nobody parses it as a frame
    let fcs = crate::frames::crc16_kermit(&psdu[..n - 2]).to_le_bytes();
    psdu[n - 2] = !fcs[0];
    psdu[n - 1] = fcs[1];
    let t1 = now + airtime_us(n) / 1000.0;
    let ch = m.s.w.coord.st.channel;
    m.s.w.air.transmit(j, psdu, ch, now, t1, -40);
    *next = t1 + 0.6;
}

pub fn noise(m: &mut Mesh, res: &mut StressResult) -> R<()> {
    warm_up(m)?;
    let full = m.cfg.full;
    let dur = if full { 30000.0 } else { 20000.0 };
    let rate = 2.0;
    let losses: &[f64] = if full { &[0.0, 0.01, 0.05, 0.15] } else { &[0.01, 0.05, 0.15] };
    let mut light = (0, Vec::new());
    for l in losses {
        let t0 = m.now();
        m.chan.m().base_loss = *l;
        m.phase(&format!("loss {:.0} %", l * 100.0));
        m.traffic(dur, rate, MIX)?;
        m.chan.m().base_loss = 0.0;
        m.tick(DRAIN)?;
        merge_light(&mut light, light_mismatch(m, t0));
    }
    // interference bursts over the kitchen and the hall: 200 ms of every 1.5 s, 80 % of frames corrupted
    let rooms = ["kitchen", "hall"];
    let rx: Vec<NodeId> = m.node_of_spot.iter().filter(|(s, _)| rooms.contains(&m.spots[**s].room)).map(|(_, n)| *n).collect();
    let t0 = m.now();
    m.phase("bursts kitchen+hall 200 ms/1.5 s");
    {
        let mut c = m.chan.m();
        let mut t = t0;
        while t < t0 + dur {
            c.bursts.push(crate::chan::Burst { t0: t, t1: t + 200.0, p: 0.8, rx: Some(rx.clone()) });
            t += 1500.0;
        }
    }
    m.traffic(dur, rate, MIX)?;
    m.chan.m().bursts.clear();
    m.tick(DRAIN)?;
    merge_light(&mut light, light_mismatch(m, t0));
    // a jammer in the kitchen: back-to-back junk frames in the same windows (energy: CCA busy, collisions)
    let (kx, ky) = room_centre(m, "kitchen");
    let j = m.add_other("jammer", "kitchen", kx, ky + 0.5, 0xFFFF, 0xFFFF, false);
    let t0 = m.now();
    m.phase("jammer kitchen 200 ms/1.5 s");
    let mut next = 0.0;
    let on = move |t: f64| (t - t0).rem_euclid(1500.0) < 200.0;
    m.traffic_with(dur, rate, MIX, &mut |m| {
        jam(m, j, &mut next, &on);
        Ok(())
    })?;
    m.tick(DRAIN)?;
    merge_light(&mut light, light_mismatch(m, t0));
    // a neighbour's network on the same channel next to the hall: 20 frames/s with acknowledgements
    let (hx, hy) = room_centre(m, "hall");
    let a = m.add_other("pan2-a", "hall", hx - 1.0, hy - 1.5, 0x5E55, 0x0001, true);
    let _b = m.add_other("pan2-b", "hall", hx + 1.0, hy - 1.5, 0x5E55, 0x0002, true);
    let t0 = m.now();
    m.phase("foreign PAN 20 frames/s");
    let mut next = t0;
    m.traffic_with(dur, rate, MIX, &mut |m| {
        if m.now() >= next {
            let pl: Vec<u8> = (0..40u8).collect();
            let (st, air) = m.s.w.st(a);
            st.tx_mac_data(air, 0x0002, &pl, Some(true), 0.4, None, None);
            next += 50.0;
        }
        Ok(())
    })?;
    m.tick(DRAIN)?;
    merge_light(&mut light, light_mismatch(m, t0));
    m.end_phase();
    rows_from(m, res, 0);
    let st = m.chan.m().stats.clone();
    let tot = st.values().fold((0, 0, 0, 0), |a, s| (a.0 + s.ok, a.1 + s.lost, a.2 + s.corrupt, a.3 + s.collided));
    res.notes.push(format!(
        "channel model at scripted receivers / all receivers: intact {}, lost {}, corrupted {}, collided {}",
        tot.0, tot.1, tot.2, tot.3
    ));
    final_checks(m, res, light, false);
    Ok(())
}

fn room_centre(m: &Mesh, room: &str) -> (f64, f64) {
    let v: Vec<_> = m.spots.iter().filter(|s| s.room == room).collect();
    let n = v.len().max(1) as f64;
    (v.iter().map(|s| s.x).sum::<f64>() / n, v.iter().map(|s| s.y).sum::<f64>() / n)
}

// ---------------------------------------------------------------------- stress_churn
/// The first answer from bulb `short` to a request sent at or after `t`: ms after `t`.
fn first_ok_after(m: &Mesh, short: u16, t: f64) -> Option<f64> {
    m.msgs
        .iter()
        .filter(|x| x.dst == short && x.t >= t && matches!(x.flow, Flow::CmdBulb | Flow::ReadBulb))
        .filter_map(|x| x.answered)
        .fold(None, |a: Option<f64>, b| Some(a.map_or(b, |a| a.min(b))))
        .map(|a| a - t)
}

/// Read Attributes to each of `targets` (bulb indices) in turn, one every `period` ms.
fn probing<'a>(targets: Vec<usize>, period: f64) -> impl FnMut(&mut Mesh) -> R<()> + 'a {
    let mut next = 0.0;
    let mut k = 0usize;
    move |m: &mut Mesh| {
        if targets.is_empty() || m.now() < next {
            return Ok(());
        }
        let b = targets[k % targets.len()];
        k += 1;
        if !m.s.w.air.off.contains(&m.bulbs[b].id) {
            m.read_bulb(b);
        }
        next = m.now() + period;
        Ok(())
    }
}

fn names(m: &Mesh, v: &[NodeId]) -> String {
    v.iter().map(|id| m.s.w.air.name_of(*id)).collect::<Vec<_>>().join("+")
}

fn shorts_text(m: &Mesh, v: &[u16]) -> String {
    v.iter()
        .map(|s| m.node_of_short(*s).map(|n| m.s.w.air.name_of(n)).unwrap_or_else(|| format!("{s:#06x}")))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Bulbs that lie beyond `gone` as seen from the coordinator (their every path crosses it).
fn beyond(m: &Mesh, gone: &[NodeId]) -> Vec<usize> {
    let coord_spot = *m.node_of_spot.iter().find(|(_, n)| **n == NodeId::Coord).unwrap().0;
    let gone_spots: Vec<usize> = m.node_of_spot.iter().filter(|(_, n)| gone.contains(n)).map(|(s, _)| *s).collect();
    let mut reach = vec![false; m.spots.len()];
    reach[coord_spot] = true;
    let mut q = vec![coord_spot];
    while let Some(x) = q.pop() {
        for y in 0..m.spots.len() {
            let relay = matches!(m.spots[y].kind, super::net::Kind::Bulb | super::net::Kind::Coord);
            if !reach[y] && relay && !gone_spots.contains(&y) && super::net::dist(&m.spots[x], &m.spots[y]) <= super::net::RANGE {
                reach[y] = true;
                q.push(y);
            }
        }
    }
    (0..m.bulbs.len()).filter(|b| !reach[m.bulbs[*b].spot] && !gone.contains(&m.bulbs[*b].id)).collect()
}

/// One churn event: `gone` lose power (bulbs) or go silent (plugs) for `off_ms`, traffic
/// goes on, the affected bulbs are probed; numbers into `row`.
fn outage(m: &mut Mesh, res: &mut StressResult, name: &str, gone: &[NodeId], off_ms: f64, after_ms: f64, rate: f64,
          lqi_of: Option<usize>) -> R<Row> {
    let p = m.phase(name);
    let t_off = m.now();
    let cut = beyond(m, gone);
    let gone_bulbs: Vec<usize> = gone.iter().filter_map(|id| m.bulb_ix(*id)).collect();
    for id in gone {
        match id {
            NodeId::Peer(_) => {
                let k = m.plugs.iter().position(|p| p.id == *id).unwrap();
                m.plug_vanish(k);
            }
            _ => m.power_off(*id),
        }
    }
    let mut targets: Vec<usize> = cut.clone();
    let others: Vec<usize> = (0..m.bulbs.len()).filter(|b| !cut.contains(b) && !gone_bulbs.contains(b)).collect();
    targets.extend(others.iter().copied());
    // during the outage: traffic, plus probes of the bulbs behind the gap
    let mut probe = probing(cut.clone(), 1000.0);
    let t_lqi = t_off + off_ms - 8000.0;
    let mut stale = None;
    let mut done_lqi = false;
    let end = t_off + off_ms;
    while m.now() < end {
        let step = (end - m.now()).min(if !done_lqi && lqi_of.is_some() { (t_lqi - m.now()).max(0.0) } else { f64::MAX });
        if step > 0.0 {
            m.traffic_with(step, rate, MIX, &mut probe)?;
        }
        if !done_lqi {
            if let Some(b) = lqi_of {
                if m.now() >= t_lqi {
                    done_lqi = true;
                    stale = Some(m.neighbours(b)?);
                }
            }
        }
        if lqi_of.is_none() {
            done_lqi = true;
        }
    }
    // power back (bulbs boot again), router back
    let t_on = m.now();
    for id in gone {
        match id {
            NodeId::Peer(_) => {
                let k = m.plugs.iter().position(|p| p.id == *id).unwrap();
                m.plug_return(k);
            }
            _ => m.power_on(*id)?,
        }
    }
    let mut back = gone_bulbs.clone();
    back.extend(cut.iter().copied());
    // the light 1.5 s after the power came back, before any command could reach it
    m.tick(1500.0)?;
    let lights: Vec<String> =
        gone_bulbs.iter().map(|b| format!("{} {}", m.bulbs[*b].name, if m.light_on(m.bulbs[*b].id) { "on" } else { "off" })).collect();
    let mut probe = probing(back.clone(), 1000.0);
    m.traffic_with(after_ms, rate, MIX, &mut probe)?;
    m.tick(DRAIN)?;
    m.end_phase();
    let mut r = row(m, p);
    let storm = m.air_count(t_off, t_off + 20000.0, None);
    let storm_on = m.air_count(t_on, t_on + 20000.0, None);
    r.extra.push(("off".into(), format!("{} for {:.0} s", names(m, gone), off_ms / 1000.0)));
    if !cut.is_empty() {
        let cut_ids: Vec<NodeId> = cut.iter().map(|b| m.bulbs[*b].id).collect();
        r.extra.push(("cut off".into(), names(m, &cut_ids)));
    }
    r.extra.push(("RREQ 20 s after off (new/frames)".into(), format!("{}/{}", storm.rreq_new, storm.rreq_frames)));
    r.extra.push(("RREQ 20 s after on (new/frames)".into(), format!("{}/{}", storm_on.rreq_new, storm_on.rreq_frames)));
    // recovery of each bulb that was off or cut off: first answer to a request sent after power-on
    let mut rec = Vec::new();
    let mut worst: Option<f64> = Some(0.0);
    for b in &back {
        let x = first_ok_after(m, m.bulbs[*b].short, t_on);
        rec.push(format!("{} {}", m.bulbs[*b].name, x.map(|v| format!("{:.1} s", v / 1000.0)).unwrap_or("never".into())));
        worst = match (worst, x) {
            (Some(w), Some(v)) => Some(w.max(v)),
            _ => None,
        };
    }
    if !back.is_empty() {
        r.extra.push(("answers again after power-on".into(), rec.join(", ")));
        res.notes.push(format!("{name}: answers again after power-on: {}", rec.join(", ")));
    }
    // bulbs not affected: did their traffic suffer?
    let ok_others = m
        .msgs
        .iter()
        .filter(|x| x.phase == p && x.t < t_on && matches!(x.flow, Flow::CmdBulb | Flow::ReadBulb))
        .filter(|x| others.iter().any(|b| m.bulbs[*b].short == x.dst));
    let (n_o, ok_o) = ok_others.fold((0, 0), |a, x| (a.0 + 1, a.1 + x.ok() as usize));
    if n_o > 0 {
        r.extra.push(("unaffected bulbs during outage".into(), format!("{ok_o}/{n_o} answered")));
    }
    if !lights.is_empty() {
        r.extra.push(("light 1.5 s after power-on".into(), lights.join(", ")));
    }
    if let Some(st) = stale {
        let gone_shorts: Vec<u16> = gone.iter().map(|id| m.short_of(*id)).collect();
        let txt = match &st {
            Some(v) => {
                let still: Vec<u16> = v.iter().copied().filter(|s| gone_shorts.contains(s)).collect();
                format!("{} of {} gone still listed {:.0} s after: [{}]", still.len(), gone_shorts.len(), (t_lqi - t_off) / 1000.0,
                        shorts_text(m, &still))
            }
            None => "no Mgmt_Lqi answer".into(),
        };
        r.extra.push((format!("{}'s neighbour table", m.bulbs[lqi_of.unwrap()].name), txt));
    }
    if worst.is_none() {
        res.notes.push(format!("{name}: some bulbs did not answer within {:.0} s after power-on", after_ms / 1000.0));
    }
    Ok(r)
}

pub fn churn(m: &mut Mesh, res: &mut StressResult) -> R<()> {
    warm_up(m)?;
    res.rows.push(row(m, 0));
    let full = m.cfg.full;
    let rate = 2.0;
    let hall = m.by_name("hall");
    let hall_ix = m.bulb_ix(hall).unwrap();
    // a scripted router vanishes and returns
    let plug = m.by_name("plug-hall");
    let r = outage(m, res, "plug-hall silent 30 s", &[plug], 30000.0, 15000.0, rate, None)?;
    res.rows.push(r);
    // a room switched off at the wall, long enough for the neighbour entries to age (3 x 15 s)
    let bedroom: Vec<NodeId> = if full {
        vec![m.by_name("bed1"), m.by_name("bed2"), m.by_name("plug-bed")]
    } else {
        vec![m.by_name("bed"), m.by_name("plug-bed")]
    };
    // longer than the neighbour ageing of R22 (nwkRouterAgeLimit 3 x 15 s Link Status)
    let off = 60000.0;
    let r = outage(m, res, "bedroom switched off", &bedroom, off, 30000.0, rate, Some(hall_ix))?;
    res.rows.push(r);
    // the bulb under test power-cycled mid-traffic (3 s without power)
    let r = outage(m, res, "hall power-cycled 3 s", &[hall], 3000.0, 30000.0, rate, None)?;
    res.rows.push(r);
    if full {
        // the kitchen off: everything beyond the living room is cut off
        let kitchen = vec![m.by_name("kit1"), m.by_name("kit2"), m.by_name("plug-kit")];
        let r = outage(m, res, "kitchen switched off", &kitchen, 30000.0, 40000.0, rate, None)?;
        res.rows.push(r);
    }
    let light = light_mismatch(m, m.now() - 20000.0);
    final_checks(m, res, light, true);
    Ok(())
}

// ---------------------------------------------------------------------- stress_broadcast
/// Per group command: (bulbs that switched, spread of their switching times, mean latency).
fn group_spread(m: &Mesh, t: f64, want: Option<bool>, window: f64) -> (usize, Option<f64>, Option<f64>, usize) {
    let mut lat = Vec::new();
    let mut extra = 0;
    for (i, b) in m.bulbs.iter().enumerate() {
        if m.s.w.air.off.contains(&b.id) {
            continue;
        }
        let ch: Vec<&(f64, bool)> = m.light_log[i].iter().filter(|(x, _)| *x >= t && *x < t + window).collect();
        let hit = ch.iter().find(|(_, on)| want.map_or(true, |w| *on == w));
        if let Some((x, _)) = hit {
            lat.push(*x - t);
        }
        if want.is_none() && ch.len() > 1 {
            extra += ch.len() - 1;
        }
    }
    let spread = if lat.is_empty() {
        None
    } else {
        Some(lat.iter().cloned().fold(f64::MIN, f64::max) - lat.iter().cloned().fold(f64::MAX, f64::min))
    };
    let mean = if lat.is_empty() { None } else { Some(lat.iter().sum::<f64>() / lat.len() as f64) };
    (lat.len(), spread, mean, extra)
}

pub fn broadcast(m: &mut Mesh, res: &mut StressResult) -> R<()> {
    warm_up(m)?;
    res.rows.push(row(m, 0));
    let full = m.cfg.full;
    let n = m.bulbs.len();
    let joined = m.join_group()?;
    res.notes.push(format!("group {GROUP:#06x}: {joined} of {n} bulbs confirmed Add Group"));
    // Z2M-style group On/Off to every bulb, 3 s apart, no other traffic
    let k = if full { 10 } else { 6 };
    let p = m.phase("group On/Off, quiet");
    let mut stats = Vec::new();
    for i in 0..k {
        let cmd = (i % 2) as u8;
        let mi = m.group_cmd(cmd);
        let t = m.msgs[mi].t;
        m.tick(3000.0)?;
        let st = group_spread(m, t, Some(cmd == 1), 3000.0);
        mark_group(m, mi, &st);
        stats.push(st);
    }
    m.end_phase();
    let mut r = row(m, p);
    group_row(&mut r, &stats, n);
    res.rows.push(r);
    // a storm: every scripted router and the coordinator broadcast to a group no bulb is in
    // (the bulbs must relay it, not execute it); a group Toggle to the bulbs every 2 s
    let rates: &[f64] = if full { &[2.0, 5.0, 10.0, 25.0] } else { &[3.0, 10.0] };
    for rate in rates {
        let p = m.phase(&format!("group Toggle in a {rate:.0}/s broadcast storm"));
        let toggles = if full { 10 } else { 6 };
        let mut ts = Vec::new();
        let mut mis = Vec::new();
        let gap = 1000.0 / rate;
        let mut next_b = m.now();
        let mut src = 0usize;
        for _ in 0..toggles {
            let mi = m.group_cmd(2);
            ts.push(m.msgs[mi].t);
            mis.push(mi);
            let end = m.now() + 2000.0;
            while m.now() < end {
                if m.now() >= next_b {
                    let np = m.plugs.len() + 1;
                    let who = src % np;
                    src += 1;
                    let o = Tx::new().group(0x0B0B).radius(8);
                    if who == np - 1 {
                        m.s.w.coord.tx_zcl(&mut m.s.w.air, 0xFFFD, 0x0006, 0x02, &[], &o);
                    } else {
                        let id = m.plugs[who].id;
                        let (st, air) = m.s.w.st(id);
                        st.tx_zcl(air, 0xFFFD, 0x0006, 0x02, &[], &o);
                    }
                    next_b += gap;
                }
                m.tick(super::net::STEP)?;
            }
        }
        m.tick(DRAIN)?;
        m.end_phase();
        let mut stats = Vec::new();
        let (mut missed, mut twice) = (0usize, 0usize);
        let mut detail = Vec::new();
        for (i, t) in ts.iter().enumerate() {
            let w = ts.get(i + 1).map(|x| x - t).unwrap_or(2000.0);
            for (k, b) in m.bulbs.iter().enumerate() {
                let ch: Vec<String> = m.light_log[k].iter().filter(|(x, _)| *x >= *t && *x < t + w)
                    .map(|(x, _)| format!("{:.0}", x - t)).collect();
                if ch.len() != 1 {
                    detail.push(format!("#{i} {}: [{}]", b.name, ch.join(" ")));
                }
                if ch.len() > 1 && std::env::var_os("STRESS_ACCOUNT").is_some() {
                    // the copies of the group command this bulb got just before each change
                    let zseq = m.msgs.iter().filter(|x| x.flow == super::net::Flow::Group).find(|x| (x.t - t).abs() < 1.0).map(|x| x.seq);
                    let first = m.dec.iter().find(|d| d.t0 >= *t && d.aps.as_ref().is_some_and(|a| a.group == Some(super::net::GROUP)) && d.zcl.as_ref().map(|z| z.seq) == zseq).map(|d| d.t0);
                    for (x, _) in m.light_log[k].iter().filter(|(x, _)| *x >= *t && *x < t + w) {
                        let got = m.dec.iter().rev().filter(|d| d.t1 <= *x && d.t1 > *t).find(|d| {
                            d.aps.as_ref().is_some_and(|a| a.group == Some(super::net::GROUP)) && d.zcl.as_ref().map(|z| z.seq) == zseq && m.chan.got(d.idx, b.id)
                        });
                        let who = got.and_then(|d| d.who).map(|w| format!("{} ({})", m.s.w.air.name_of(w), m.bulbs.iter().find(|bb| bb.id == w).map_or("not a bulb".to_string(), |bb| bb.image.clone())));
                        println!("   double: toggle #{i} {} ({}) changed at +{:.0} ms, {:.0} ms after the last copy it got, from {} sent {:.0} ms after the first",
                                 b.name, b.image, x - t, got.map_or(f64::NAN, |d| x - d.t1), who.unwrap_or("?".into()),
                                 got.zip(first).map_or(f64::NAN, |(d, f)| d.t0 - f));
                    }
                }
            }
            let s = group_spread(m, *t, None, w);
            missed += n - s.0;
            twice += s.3;
            mark_group(m, mis[i], &s);
            stats.push(s);
        }
        let mut r = row(m, p);
        group_row(&mut r, &stats, n);
        let ph = &m.phases[p];
        let bc = m.air_count(ph.t0, ph.t1, None);
        let n_bcast = (rate * 2.0 * toggles as f64) as u64 + toggles as u64;
        r.extra.push(("relays per broadcast (all bulbs)".into(), format!("{:.1}", bc.bcast_relays as f64 / n_bcast.max(1) as f64)));
        r.extra.push(("toggles missed / extra".into(), format!("{missed} / {twice}")));
        if !detail.is_empty() {
            r.extra.push(("light changes ms after each toggle, where not exactly one".into(), detail.join(", ")));
        }
        res.rows.push(r);
        res.check(&format!("a group Toggle in a {rate:.0}/s storm is executed at most once by each bulb (R22 3.6.5 BTT)"),
                  twice == 0, format!("{twice} extra light change(s) over {toggles} toggles x {n} bulbs"));
    }
    // final state: a group On, then every bulb on
    m.phase("final group On");
    let mi = m.group_cmd(1);
    let t = m.msgs[mi].t;
    m.tick(3000.0)?;
    m.end_phase();
    let s = group_spread(m, t, Some(true), 3000.0);
    mark_group(m, mi, &s);
    let all_on = m.bulbs.iter().filter(|b| m.light_on(b.id)).count();
    res.notes.push(format!("final group On: {all_on} of {n} bulbs on ({} switched)", s.0));
    let light = (0, Vec::new());
    final_checks(m, res, light, false);
    Ok(())
}

/// A group command counts as delivered when a bulb switched; its latency is the mean over the bulbs.
fn mark_group(m: &mut Mesh, mi: usize, s: &(usize, Option<f64>, Option<f64>, usize)) {
    if let Some(mean) = s.2 {
        let t = m.msgs[mi].t;
        m.msgs[mi].delivered = Some(t + mean);
    }
}

fn group_row(r: &mut Row, stats: &[(usize, Option<f64>, Option<f64>, usize)], n: usize) {
    let reached: usize = stats.iter().map(|s| s.0).sum();
    let spreads: Vec<f64> = stats.iter().filter_map(|s| s.1).collect();
    let means: Vec<f64> = stats.iter().filter_map(|s| s.2).collect();
    r.extra.push(("bulbs switched".into(), format!("{reached}/{} ({:.0} %)", stats.len() * n, 100.0 * reached as f64 / (stats.len() * n).max(1) as f64)));
    r.extra.push(("spread across bulbs p50/max ms".into(),
                  format!("{}/{}", f1(super::net::pct(&spreads, 50.0)), f1(spreads.iter().cloned().fold(None, |a: Option<f64>, b| Some(a.map_or(b, |a| a.max(b))))))));
    r.extra.push(("latency to light, mean of means ms".into(), f1(if means.is_empty() { None } else { Some(means.iter().sum::<f64>() / means.len() as f64) })));
}
