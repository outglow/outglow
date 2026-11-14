//! `stress_diag`: the stress traffic on the house mesh, with the Diagnostics
//! cluster of every bulb read over ZCL before and after, and its counters
//! held against what the harness itself sees on the air (`net.rs`, the
//! accounting of `STRESS_ACCOUNT`): MAC unicasts and their retransmissions,
//! MAC broadcasts, relayed frames and the relayed frames lost. A bulb whose
//! image has no such cluster (stock, a mixed house) is left out.

use std::collections::BTreeMap;

use super::net::Mesh;
use super::scen::{traffic_phases, warm_up};
use super::StressResult;
use crate::session::R;

const DIAG: u16 = 0x0B05;
const MFR: u16 = 0x1141;
/// MacTxBcast, MacTxUcast, MacTxUcastRetry, MacTxUcastFail, RelayedUcast, NWKFCFailure
const STD: [u16; 6] = [0x0101, 0x0103, 0x0104, 0x0105, 0x0118, 0x0112];
/// relays lost: queue full, no route, MAC failure, duplicate; Network Status sent
const MAN: [u16; 5] = [0x5005, 0x5006, 0x5007, 0x5008, 0x5009];

/// Integer values of the records of a Read Attributes Response body.
fn values(b: &[u8]) -> BTreeMap<u16, u32> {
    let mut out = BTreeMap::new();
    let mut i = 0;
    while i + 3 <= b.len() {
        let id = u16::from_le_bytes([b[i], b[i + 1]]);
        let st = b[i + 2];
        i += 3;
        if st != 0 {
            continue;
        }
        let Some(&ty) = b.get(i) else { break };
        let n = match ty {
            0x20 | 0x28 | 0x30 => 1,
            0x21 => 2,
            0x23 => 4,
            _ => break,
        };
        let Some(v) = b.get(i + 1..i + 1 + n) else { break };
        out.insert(id, v.iter().rev().fold(0u32, |a, x| a << 8 | *x as u32));
        i += 1 + n;
    }
    out
}

/// One Read Attributes of bulb `b` (standard or under our code), tried up
/// up to three times: the values, and when the bulb began to send the
/// answer. It took the values as it built the answer, so its frames before
/// that moment are in them and its frames from it on are not.
fn read(m: &mut Mesh, b: usize, mfr: bool, attrs: &[u16]) -> R<Option<(BTreeMap<u16, u32>, f64)>> {
    let (short, id) = (m.bulbs[b].short, m.bulbs[b].id);
    for _ in 0..3 {
        m.coord_route(short);
        let n0 = m.s.w.coord.zcl_rsp.len();
        let t_sent = m.now();
        let seq = if mfr {
            m.s.w.coord.read_attrs_mfr(&mut m.s.w.air, short, 1, DIAG, MFR, attrs)
        } else {
            m.s.w.coord.read_attrs(&mut m.s.w.air, short, 1, DIAG, attrs)
        };
        let end = m.now() + 2000.0;
        while m.now() < end {
            m.tick(10.0)?;
            if let Some(r) = m.s.w.coord.zcl_rsp[n0..].iter().find(|r| r.src == short && r.cluster == DIAG && r.cmd == Some(0x01)) {
                let v = values(&r.body);
                let at = m
                    .dec
                    .iter()
                    .find(|d| {
                        d.t0 >= t_sent
                            && d.who == Some(id)
                            && d.aps.as_ref().is_some_and(|a| a.cluster == DIAG)
                            && d.zcl.as_ref().is_some_and(|z| z.seq == seq && z.cmd == 0x01)
                    })
                    .map_or(t_sent, |d| d.t0);
                return Ok(if v.is_empty() { None } else { Some((v, at)) });
            }
        }
    }
    Ok(None)
}

/// Every bulb's counters and the moment it took them (None: no cluster, or no answer).
fn snapshot(m: &mut Mesh) -> R<Vec<Option<(BTreeMap<u16, u32>, f64)>>> {
    let mut out = Vec::new();
    for b in 0..m.bulbs.len() {
        let std = read(m, b, false, &STD)?;
        let man = match std {
            Some(_) => read(m, b, true, &MAN)?,
            None => None,
        };
        // the manufacturer's attributes change little; the moment is that of the standard ones
        out.push(std.zip(man).map(|((mut a, at), (b, _))| {
            a.extend(b);
            (a, at)
        }));
    }
    Ok(out)
}

/// What bulb `id` did on the air between t0 and t1: unicast frames that ask
/// for an acknowledgement (first attempts), their retransmissions, frames
/// that ask for none (broadcasts, beacons).
fn air_mac(m: &Mesh, k: usize, t0: f64, t1: f64) -> (u64, u64, u64) {
    let id = m.bulbs[k].id;
    let c = m.air_count(t0, t1, Some(id));
    let (mut ucast, mut bcast) = (0u64, 0u64);
    for d in m.dec.iter().filter(|d| d.t0 >= t0 && d.t0 < t1 && d.who == Some(id) && d.mac_ty != 2) {
        if d.mac_ar {
            ucast += 1;
        } else {
            bcast += 1;
        }
    }
    (ucast - c.mac_retx.min(ucast), c.mac_retx, bcast)
}

/// Frames of others bulb `k` handed on between t0 and t1 and the next hop
/// acknowledged: each MAC frame once (its retransmissions are the same
/// frame), as RelayedUcast counts them. A copy of a frame the bulb relayed
/// before that comes secured anew (its sender tried again at the NWK
/// level) is relayed again, and counts again, here as there.
fn air_relayed(m: &Mesh, k: usize, t0: f64, t1: f64) -> u64 {
    let (id, me) = (m.bulbs[k].id, m.bulbs[k].short);
    let mut seen: Vec<(u8, u16, u16, u8, f64)> = Vec::new();
    let mut n = 0;
    for (i, d) in m.dec.iter().enumerate() {
        if d.t0 < t0 || d.t0 >= t1 || d.who != Some(id) || d.mac_ty != 1 || !d.mac_ar {
            continue;
        }
        let (Some(nw), Some(md)) = (&d.nwk, d.mac_dst) else { continue };
        if nw.src == me {
            continue;
        }
        let Some(r) = m.node_of_short(md) else { continue };
        let acked = m.dec[i + 1..].iter().take_while(|e| e.t0 < d.t1 + 2.0).any(|e| e.who == Some(r) && e.mac_ty == 2 && e.mac_seq == d.mac_seq);
        if !acked {
            continue;
        }
        let key = (d.mac_seq, nw.src, md, nw.seq);
        if seen.iter().any(|s| (s.0, s.1, s.2, s.3) == key && d.t0 - s.4 < 100.0) {
            continue;
        }
        seen.push((key.0, key.1, key.2, key.3, d.t0));
        n += 1;
    }
    n
}

fn close(a: u64, b: u64, slack: u64, pct: f64) -> bool {
    let tol = slack.max((b.max(a) as f64 * pct / 100.0).ceil() as u64);
    a.abs_diff(b) <= tol
}

pub fn run(m: &mut Mesh, res: &mut StressResult) -> R<()> {
    warm_up(m)?;
    let before = snapshot(m)?;
    m.tick(1000.0)?;
    let t0 = m.now();
    traffic_phases(m)?;
    m.end_phase();
    let t1 = m.now();
    m.tick(1000.0)?;
    let after = snapshot(m)?;
    let mut checked = 0;
    let _ = (t0, t1);
    for k in 0..m.bulbs.len() {
        let (Some((a, t0)), Some((b, t1))) = (before[k].clone(), after[k].clone()) else {
            res.notes.push(format!("{}: no Diagnostics cluster read, left out", m.bulbs[k].name));
            continue;
        };
        let (a, b) = (&a, &b);
        checked += 1;
        let d = |id: u16| b.get(&id).copied().unwrap_or(0).wrapping_sub(a.get(&id).copied().unwrap_or(0)) as u64;
        let d16 = |id: u16| d(id) & 0xFFFF;
        let (ucast, retx, bcast) = air_mac(m, k, t0, t1);
        let (acked, lost) = m.relay_drops(t0, t1, Some(m.bulbs[k].id));
        let relayed = air_relayed(m, k, t0, t1);
        let drops = d16(0x5005) + d16(0x5006) + d16(0x5007);
        let name = m.bulbs[k].name.clone();
        let line = format!(
            "{name}: MacTxUcast {} (air {ucast}), MacTxUcastRetry {} (air {retx}), MacTxBcast {} (air {bcast}), \
             MacTxUcastFail {}; RelayedUcast {} (air {relayed}) + lost by cause {} (queue {}, no route {}, MAC {}) = {} (air: acknowledged for \
             relaying {acked}, of them lost {lost}); duplicates {} (NWKFCFailure {}), Network Status sent {}",
            d(0x0103), d16(0x0104), d(0x0101), d16(0x0105), d16(0x0118), drops, d16(0x5005), d16(0x5006), d16(0x5007),
            d16(0x0118) + drops, d16(0x5008), d16(0x0112), d16(0x5009)
        );
        res.notes.push(line.clone());
        // the window runs from one answer's first transmission to the
        // next's; the slack is for frames in flight at its ends
        res.check(&format!("{name}: MacTxUcastRetry = the bulb's MAC retransmissions on the air"), close(d16(0x0104), retx, 2, 1.0), line.clone());
        res.check(&format!("{name}: MacTxUcast = the bulb's unicast frames on the air (first attempts)"), close(d(0x0103), ucast, 2, 1.0), line.clone());
        res.check(&format!("{name}: MacTxBcast = the bulb's frames without acknowledgement on the air"), close(d(0x0101), bcast, 2, 1.0), line.clone());
        res.check(&format!("{name}: RelayedUcast = the frames of others the bulb handed on, acknowledged"), close(d16(0x0118), relayed, 2, 1.0),
                  line.clone());
        // a frame acknowledged for relaying ends relayed (once or, sent again secured anew, more often) or lost by a cause
        res.check(
            &format!("{name}: every relayed frame the bulb acknowledged is relayed or lost by a counted cause"),
            d16(0x0118) + drops + 3 >= acked,
            line.clone(),
        );
        res.check(&format!("{name}: no more relays lost by cause than the air shows unanswered or failed"), drops <= acked, line);
    }
    res.check("the Diagnostics cluster of at least one bulb was read", checked > 0, format!("{checked} bulb(s)"));
    Ok(())
}
