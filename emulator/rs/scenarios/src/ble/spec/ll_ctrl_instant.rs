use std::collections::HashMap;

use crate::ble::harness::{csa1, hx, BleSession, LinkOpts, ALL_CHANNELS};
use crate::ble::BleTest;
use crate::py;
use crate::session::{expect, Stop, R};

const DOC: &str = "Core 5.4 Vol 6 Part B";

pub fn tests() -> Vec<BleTest> {
    vec![
        BleTest {
            id: "ll_ctrl_conn_update",
            doc: DOC,
            clause: "5.1.1",
            req: "After LL_CONNECTION_UPDATE_IND the peripheral hears the central's first packet anywhere in the \
                  transmit window that starts connIntervalOLD + WinOffset after the last old anchor, and follows the \
                  new interval.",
            run: conn_update,
            ..BleTest::D
        },
        BleTest {
            id: "ll_ctrl_conn_update_before_instant",
            doc: DOC,
            clause: "5.1.1, 5.5.1",
            req: "A connection update first received in the event just before its instant takes effect at the \
                  instant.",
            run: conn_update_minus_one,
            ..BleTest::D
        },
        BleTest {
            id: "ll_ctrl_conn_update_at_instant",
            doc: DOC,
            clause: "5.1.1 (Note), 5.5.1",
            req: "A connection update first received in the event of its instant is not 'in the past': the \
                  connection goes on with the new parameters, that packet being the new anchor.",
            run: conn_update_at_instant,
            ..BleTest::D
        },
        BleTest {
            id: "ll_ctrl_conn_update_instant_passed",
            doc: DOC,
            clause: "5.5.1",
            req: "An LL_CONNECTION_UPDATE_IND whose instant is in the past makes the peripheral consider the \
                  connection lost.",
            run: conn_update_passed,
            ..BleTest::D
        },
        BleTest {
            id: "ll_ctrl_conn_update_supervision",
            doc: DOC,
            clause: "5.1.1, 4.5.2",
            req: "The supervision timer is restarted at the instant of a connection update, so a new timeout shorter \
                  than the old interval does not end the connection there.",
            run: conn_update_supervision,
            ..BleTest::D
        },
        BleTest {
            id: "ll_ctrl_channel_map_update",
            doc: DOC,
            clause: "5.1.2, 4.5.8.2",
            req: "From the instant of LL_CHANNEL_MAP_IND on the new map is used; lastUnmappedChannel is not reset.",
            run: channel_map_update,
            ..BleTest::D
        },
        BleTest {
            id: "ll_ctrl_channel_map_before_instant",
            doc: DOC,
            clause: "5.1.2, 5.5.1",
            req: "A channel map update first received in the event just before its instant takes effect at the \
                  instant.",
            run: channel_map_minus_one,
            ..BleTest::D
        },
        BleTest {
            id: "ll_ctrl_channel_map_at_instant",
            doc: DOC,
            clause: "5.1.2, 5.5.1",
            req: "A channel map update first received in the event of its instant is not 'in the past': the new map \
                  is in use from then on.",
            run: channel_map_at_instant,
            ..BleTest::D
        },
    ]
}

fn g(x: f64) -> String {
    let s = py::float(x);
    s.strip_suffix(".0").map(String::from).unwrap_or(s)
}

fn ints(v: &[i64]) -> String {
    py::ints(v.iter().copied())
}

fn answered(s: &BleSession, lo: i64, hi: i64) -> Vec<i64> {
    let mut v: Vec<i64> = s.lk().pdus.iter().map(|p| p.event).filter(|e| lo <= *e && *e <= hi).collect();
    v.sort();
    v.dedup();
    v
}

fn why(s: &mut BleSession) -> R<String> {
    let m = s.misses(0)?;
    let mut w: Vec<String> = m[m.len().saturating_sub(4)..].iter().map(|x| x.2.clone()).collect();
    w.sort();
    w.dedup();
    Ok(w.join("; "))
}

fn sent_map(s: &mut BleSession) -> R<HashMap<i64, f64>> {
    Ok(s.sent()?.into_iter().collect())
}

fn state(s: &BleSession) -> String {
    s.lk().st.state().to_string()
}

#[allow(clippy::too_many_arguments)]
fn update(s: &mut BleSession, old: i64, new: i64, off: i64, size: i64, pos: f64, lead: i64, timeout: i64,
          new_timeout: Option<i64>) -> R<(i64, Option<f64>)> {
    s.connect(
        LinkOpts::new()
            .set("interval", old)
            .set("timeout", timeout)
            .set("winsize", 8)
            .set("anchor", py::float(pos)),
    )?;
    s.wait_event(6, None)?;
    let k = s.event()?;
    let inst = k + lead;
    s.update(new, inst, size, off, 0, Some(new_timeout.unwrap_or(timeout)))?;
    s.wait_event(inst + 12, Some(lead as f64 * old as f64 * 1.25 + 14.0 * new as f64 * 1.25 + 500.0))?;
    s.link_run(3.0)?;
    let sent = sent_map(s)?;
    let gap = match (sent.get(&inst), sent.get(&(inst - 1))) {
        (Some(a), Some(b)) => Some(a - b),
        _ => None,
    };
    Ok((inst, gap))
}

fn conn_update(s: &mut BleSession) -> R<String> {
    let cases: [(i64, i64, i64, i64, f64); 5] =
        [(24, 80, 0, 1, 0.03), (24, 6, 5, 4, 4.9), (40, 40, 17, 2, 1.2), (12, 320, 100, 8, 9.9), (80, 12, 0, 2, 2.45)];
    let (mut notes, mut bad) = (Vec::new(), Vec::new());
    for (old, new, off, size, pos) in cases {
        let (inst, gap) = update(s, old, new, off, size, pos, 8, 300, None)?;
        let name = format!(
            "{} -> {} ms, WinOffset {off}, WinSize {size}, packet {} ms into the window",
            g(old as f64 * 1.25),
            g(new as f64 * 1.25),
            py::float(pos)
        );
        let want = old as f64 * 1.25 + off as f64 * 1.25 + pos;
        if gap.map_or(true, |g| (g - want).abs() > 0.01) {
            return Err(Stop::Inconclusive(format!(
                "{name}: the central's packets around the instant are {} ms apart, the clause gives {}",
                gap.map(py::float).unwrap_or_else(|| "None".into()),
                py::float(want)
            )));
        }
        let got = answered(s, inst, inst + 12);
        if !got.contains(&inst) {
            let w = why(s)?;
            bad.push(format!(
                "{name}: no answer at the instant (event {inst}); answered {}; {w}",
                ints(&got[..got.len().min(6)])
            ));
        } else if got.len() < 10 || state(s) != "connected" {
            let w = why(s)?;
            bad.push(format!("{name}: answered only {} of the 13 events from the instant; {w}", ints(&got)));
        }
        notes.push(name.split(',').next().unwrap().to_string());
        s.drop_link(6000.0)?;
    }
    expect(
        bad.is_empty(),
        format!("{} of {}: {}", bad.len(), cases.len(), bad.iter().take(3).cloned().collect::<Vec<_>>().join(" | ")),
    )?;
    Ok(notes.join("; "))
}

fn update_tx_events(s: &mut BleSession, what: &str) -> R<Vec<crate::ble::harness::CentralTx>> {
    Ok(s.central_log()?.into_iter().filter(|t| t.what.contains(what)).collect())
}

fn conn_update_minus_one(s: &mut BleSession) -> R<String> {
    let (inst, _gap) = update(s, 24, 60, 4, 2, 1.0, 2, 300, None)?;
    let tx = update_tx_events(s, "LL_CONNECTION_UPDATE_IND")?;
    if tx.is_empty() || tx[0].event != inst - 1 || tx[0].missed {
        let e = tx.first().map(|t| t.event.to_string()).unwrap_or_else(|| "None".into());
        return Err(Stop::Inconclusive(format!("the update PDU went out in event {e}, wanted {}", inst - 1)));
    }
    let got = answered(s, inst, inst + 12);
    let st = state(s);
    let w = why(s)?;
    expect(
        got.contains(&inst) && got.len() >= 10 && st == "connected",
        format!(
            "update received in event {} for instant {inst}: answered events {} of {inst}..{}; link {st}; {w}",
            inst - 1,
            ints(&got),
            inst + 12
        ),
    )?;
    Ok(format!("PDU in event {}, new interval from event {inst}", inst - 1))
}

fn conn_update_at_instant(s: &mut BleSession) -> R<String> {
    let (old, new) = (24i64, 60i64);
    s.connect(LinkOpts::new().set("interval", old).set("timeout", 500).set("winsize", 2).set("anchor", "0.0"))?;
    s.wait_event(8, None)?;
    let k = s.event()?;
    let inst = k + 6;
    let t_k = *sent_map(s)?.get(&k).ok_or_else(|| Stop::Error(format!("KeyError: {k}")))?;
    s.update(new, inst, 1, 0, 0, Some(500))?;
    s.air(&[("blackout", py::float(t_k + 6.0 * old as f64 * 1.25 - 3.0))])?;
    s.wait_event(inst + 12, Some(6.0 * old as f64 * 1.25 + 14.0 * new as f64 * 1.25 + 500.0))?;
    s.link_run(3.0)?;
    s.air_off()?;
    let heard_before: Vec<i64> = s.lk().pdus.iter().map(|p| p.event).filter(|e| k < *e && *e < inst).collect();
    if !heard_before.is_empty() {
        return Err(Stop::Inconclusive(format!("the bulb was heard in events {} before the instant", ints(&heard_before))));
    }
    let tx: Vec<_> = update_tx_events(s, "LL_CONNECTION_UPDATE_IND")?.into_iter().filter(|t| t.event == inst).collect();
    if tx.is_empty() {
        return Err(Stop::Inconclusive("the central did not repeat the update PDU at the instant".into()));
    }
    let got = answered(s, inst, inst + 12);
    let w = why(s)?;
    expect(
        got.contains(&inst),
        format!(
            "the update PDU sent at its instant (event {inst}, exactly one old interval after the last anchor) was \
             not answered; answered {}; {w}",
            ints(&got)
        ),
    )?;
    let st = state(s);
    let w = why(s)?;
    expect(
        got.len() >= 10 && st == "connected",
        format!(
            "update first received at its instant {inst}: afterwards the bulb answered events {} of {inst}..{} at \
             the new interval; link {st}; {w}",
            ints(&got),
            inst + 12
        ),
    )?;
    Ok(format!("PDU first received at instant {inst}; new interval followed"))
}

fn conn_update_passed(s: &mut BleSession) -> R<String> {
    s.connect(LinkOpts::new().set("interval", 24).set("timeout", 400))?;
    s.wait_event(20, None)?;
    let k = s.event()?;
    s.update(40, k - 10, 1, 0, 0, None)?;
    s.wait_event(k + 40, None)?;
    s.link_run(3.0)?;
    let tx: Vec<_> = update_tx_events(s, "LL_CONNECTION_UPDATE_IND")?.into_iter().filter(|t| !t.missed).collect();
    if tx.is_empty() {
        return Err(Stop::Inconclusive("the update PDU did not reach the bulb".into()));
    }
    let late = answered(s, tx[0].event + 3, k + 40);
    expect(
        late.is_empty(),
        format!(
            "instant {} sent in event {}: the bulb went on answering, events {}..{} ({} of {})",
            k - 10,
            tx[0].event,
            ints(&late[..late.len().min(3)]),
            late.last().map(|x| x.to_string()).unwrap_or_default(),
            late.len(),
            k + 40 - tx[0].event - 2
        ),
    )?;
    let since = tx[0].ms + 3.0 * s.interval_ms();
    let rx = s.rx_windows(since);
    expect(rx.is_empty(), format!("{} receive windows later than three events after the PDU", rx.len()))?;
    Ok(format!("connection given up after the PDU in event {} (instant {})", tx[0].event, k - 10))
}

fn conn_update_supervision(s: &mut BleSession) -> R<String> {
    let (inst, _gap) = update(s, 800, 40, 0, 2, 1.0, 6, 800, Some(50))?;
    let got = answered(s, inst, inst + 12);
    let st = state(s);
    let w = why(s)?;
    expect(
        got.contains(&inst) && got.len() >= 10 && st == "connected",
        format!(
            "1 s interval / 8 s timeout -> 50 ms / 500 ms at instant {inst}: answered {} of {inst}..{}; {w}",
            ints(&got),
            inst + 12
        ),
    )?;
    Ok("connection kept through the instant".into())
}

fn wrong_channels(s: &BleSession, want: &HashMap<i64, i64>) -> Vec<(i64, i64, i64)> {
    s.lk()
        .pdus
        .iter()
        .filter_map(|p| want.get(&p.event).filter(|w| **w != p.ch).map(|w| (p.event, p.ch, *w)))
        .collect()
}

fn triples(w: &[(i64, i64, i64)]) -> String {
    py::list(w.iter().take(4).map(|(a, b, c)| py::tuple([a.to_string(), b.to_string(), c.to_string()])))
}

fn map_check(s: &BleSession, hop: i64, old: &[u8], new: &[u8], inst: i64, upto: i64) -> (Vec<(i64, i64, i64)>, Vec<i64>) {
    let (seq, last) = csa1(hop, old, inst as usize, 0);
    let (seq2, _) = csa1(hop, new, (upto - inst + 1) as usize, last);
    let want: HashMap<i64, i64> = seq.into_iter().chain(seq2).enumerate().map(|(i, c)| (i as i64, c)).collect();
    (wrong_channels(s, &want), answered(s, inst, upto))
}

fn channel_map_update(s: &mut BleSession) -> R<String> {
    let hop = 10;
    s.connect(LinkOpts::new().set("interval", 6).set("timeout", 200).set("hop", hop))?;
    let mut k0 = 0;
    let mut notes: Vec<(i64, Vec<u8>)> = Vec::new();
    let maps: [[u8; 5]; 3] = [[0xff, 0xf7, 0xff, 0xff, 0x1f], [0x00, 0x00, 0xff, 0x0f, 0x00], [0x55, 0x55, 0x55, 0x55, 0x15]];
    for new in maps {
        s.wait_event(k0 + 6, None)?;
        let inst = s.event()? + 8;
        s.channel_map(&new, inst)?;
        s.wait_event(inst + 40, None)?;
        s.link_run(3.0)?;
        notes.push((inst, new.to_vec()));
        let (mut last, mut want, mut pos) = (0i64, HashMap::new(), 0i64);
        let mut m: Vec<u8> = ALL_CHANNELS.to_vec();
        let mut steps: Vec<(i64, Option<Vec<u8>>)> = notes.iter().map(|(i, mp)| (*i, Some(mp.clone()))).collect();
        steps.push((inst + 41, None));
        for (i, mp) in steps {
            let (seq, l) = csa1(hop, &m, (i - pos) as usize, last);
            last = l;
            for (j, c) in seq.into_iter().enumerate() {
                want.insert(pos + j as i64, c);
            }
            pos = i;
            if let Some(mp) = mp {
                m = mp;
            }
        }
        let wrong = wrong_channels(s, &want);
        let got = answered(s, inst, inst + 40);
        expect(wrong.is_empty(), format!("map {} at instant {inst}: (event, channel, expected) {}", hx(&new), triples(&wrong)))?;
        let w = why(s)?;
        expect(
            got.contains(&inst) && got.len() >= 34,
            format!(
                "map {} at instant {inst}: answered {} of 41 events from the instant, first {}; {w}",
                hx(&new),
                got.len(),
                ints(&got[..got.len().min(3)])
            ),
        )?;
        k0 = inst + 40;
    }
    Ok("three map changes in one connection followed at their instants".into())
}

fn channel_map_minus_one(s: &mut BleSession) -> R<String> {
    let hop = 7;
    let new = [0x00u8, 0xff, 0x00, 0xff, 0x00];
    let mut tries = 0;
    let (inst, wrong, got) = loop {
        tries += 1;
        s.connect(LinkOpts::new().set("interval", 12).set("timeout", 200).set("hop", hop))?;
        s.wait_event(9, None)?;
        let inst = s.event()? + 2;
        s.channel_map(&new, inst)?;
        s.wait_event(inst + 30, None)?;
        s.link_run(3.0)?;
        let tx = update_tx_events(s, "LL_CHANNEL_MAP_IND")?;
        if tx.is_empty() || tx[0].event != inst - 1 {
            let e = tx.first().map(|t| t.event.to_string()).unwrap_or_else(|| "None".into());
            return Err(Stop::Inconclusive(format!("the map PDU went out in event {e}, wanted {}", inst - 1)));
        }
        let missed = tx[0].missed;
        let (wrong, got) = map_check(s, hop, &ALL_CHANNELS, &new, inst, inst + 30);
        if !missed && (got.contains(&inst) || s.lost_to_154(&[inst])?.is_empty()) {
            break (inst, wrong, got);
        }
        if tries == 3 {
            return Err(Stop::Inconclusive(format!(
                "in each of {tries} connections the bulb's radio was with 802.15.4 when the map PDU went out in event {} or in the instant's event",
                inst - 1
            )));
        }
        s.drop_link(6000.0)?;
    };
    expect(wrong.is_empty(), format!("(event, channel, expected) {}", triples(&wrong)))?;
    let w = why(s)?;
    expect(
        got.contains(&inst) && got.len() >= 25,
        format!(
            "map received in event {} for instant {inst}: answered {} of 31 events from the instant ({}...); {w}",
            inst - 1,
            got.len(),
            ints(&got[..got.len().min(8)])
        ),
    )?;
    Ok(format!("PDU in event {}, new map from event {inst}", inst - 1))
}

fn channel_map_at_instant(s: &mut BleSession) -> R<String> {
    let hop = 7;
    s.connect(LinkOpts::new().set("interval", 12).set("timeout", 400).set("hop", hop).set("anchor", "0.0"))?;
    s.wait_event(8, None)?;
    let k = s.event()?;
    let inst = k + 6;
    let (seq, _) = csa1(hop, &ALL_CHANNELS, (inst + 6) as usize, 0);
    let keep = seq[inst as usize];
    let mut bits: u64 = (1 << 37) - 1;
    for ch in &seq[(inst + 1) as usize..(inst + 6) as usize] {
        if *ch != keep {
            bits &= !(1u64 << ch);
        }
    }
    let new = bits.to_le_bytes()[..5].to_vec();
    let t_k = *sent_map(s)?.get(&k).ok_or_else(|| Stop::Error(format!("KeyError: {k}")))?;
    s.channel_map(&new, inst)?;
    s.air(&[("blackout", py::float(t_k + 6.0 * 15.0 - 3.0))])?;
    s.wait_event(inst + 30, None)?;
    s.link_run(3.0)?;
    s.air_off()?;
    if s.lk().pdus.iter().any(|p| k < p.event && p.event < inst) {
        return Err(Stop::Inconclusive("the bulb was heard before the instant".into()));
    }
    let (wrong, got) = map_check(s, hop, &ALL_CHANNELS, &new, inst, inst + 30);
    let w = why(s)?;
    expect(
        got.contains(&inst),
        format!("the map PDU sent at its instant (event {inst}, channel {keep} in both maps) was not answered; {w}"),
    )?;
    expect(wrong.is_empty(), format!("(event, channel, expected) {}", triples(&wrong)))?;
    let st = state(s);
    let w = why(s)?;
    expect(
        got.len() >= 25 && st == "connected",
        format!(
            "map {} first received at its instant {inst}: afterwards the bulb answered events {} ({} of 31); link \
             {st}; {w}",
            hx(&new),
            ints(&got[..got.len().min(10)]),
            got.len()
        ),
    )?;
    Ok(format!("PDU first received at instant {inst}; new map followed"))
}
