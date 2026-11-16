use std::collections::BTreeSet;

use crate::ble::harness::{csa1, unhx, BleSession, LinkOpts};
use crate::ble::BleTest;
use crate::py;
use crate::session::{expect, Stop, R};

const DOC: &str = "Core 5.4 Vol 6 Part B";

pub fn tests() -> Vec<BleTest> {
    vec![
        BleTest {
            id: "ll_conn_first_packet_window",
            doc: DOC,
            clause: "4.5.3, 4.5.5",
            req: "The peripheral hears the central's first packet wherever it starts inside the transmit window \
                  (1.25 ms + WinOffset after the CONNECT_IND, WinSize long).",
            run: first_packet_window,
            ..BleTest::D
        },
        BleTest {
            id: "ll_conn_second_window",
            doc: DOC,
            clause: "4.5.5",
            req: "When no packet comes in a transmit window the peripheral listens again one connInterval later, on \
                  the next data channel, with connEventCount one higher.",
            run: second_window,
            ..BleTest::D
        },
        BleTest {
            id: "ll_conn_not_established_six_events",
            doc: DOC,
            clause: "4.5.2",
            req: "A connection in which nothing is received is given up after six connection events at the latest \
                  (and not before the supervision timer reaches 6 x connInterval).",
            run: six_events,
            ..BleTest::D
        },
        BleTest {
            id: "ll_conn_supervision_timeout",
            doc: DOC,
            clause: "4.5.2, 4.5.12",
            req: "An established connection is kept while packets are missing for less than connSupervisionTimeout \
                  and is given up when the timeout is reached: no reception on the data channels afterwards.",
            run: supervision_timeout,
            ..BleTest::D
        },
        BleTest {
            id: "ll_conn_peripheral_latency",
            doc: DOC,
            clause: "4.5.5, 4.5.1",
            req: "With a non-zero peripheral latency the peripheral still listens in every event until it has \
                  received a packet with NESN = 1, and at least once in every supervision timeout afterwards.",
            run: peripheral_latency,
            ..BleTest::D
        },
    ]
}

pub fn link_chm(s: &BleSession) -> Vec<u8> {
    unhx(&s.lk().opts.kv.iter().find(|(k, _)| k == "chm").map(|(_, v)| v.clone()).unwrap_or_default())
}

pub fn miss_texts(s: &mut BleSession) -> R<Vec<String>> {
    Ok(s.misses(0)?.into_iter().map(|m| m.2).collect())
}

pub fn tail(v: &[String], n: usize) -> &[String] {
    &v[v.len().saturating_sub(n)..]
}

pub fn triples(v: &[(i64, i64, i64)]) -> String {
    py::list(v.iter().map(|(a, b, c)| py::tuple([a.to_string(), b.to_string(), c.to_string()])))
}

fn connected_event(s: &mut BleSession, ev: i64, max_ms: f64, step: f64) -> R<bool> {
    Ok(s
        .until(max_ms, step, |s| {
            let st = s.poll()?;
            Ok(if st.state() == "connected" && st.event() >= ev { Some(()) } else { None })
        })?
        .is_some())
}

fn first_packet_window(s: &mut BleSession) -> R<String> {
    let cases = [(0, 1, 0.03), (0, 1, 1.22), (0, 2, 2.47), (5, 3, 0.03), (5, 3, 3.72), (24, 8, 0.03), (24, 8, 5.0),
                 (24, 8, 9.97), (13, 4, 2.5)];
    let mut missed = Vec::new();
    for (off, size, pos) in cases {
        let o = LinkOpts::new()
            .set("interval", 24)
            .set("timeout", 200)
            .set("winoffset", off)
            .set("winsize", size)
            .set("anchor", py::float(pos));
        s.connect_with(o, false, 8000.0)?;
        if !connected_event(s, 2, 6000.0, 5.0)? {
            return Err(Stop::Inconclusive(format!(
                "the central did not get to event 2 (state {})",
                s.lk().st.state()
            )));
        }
        s.link_run(5.0)?;
        let first = s.lk().pdus.first().map(|p| p.event);
        if first != Some(0) {
            let why: Vec<String> = miss_texts(s)?.into_iter().take(1).collect();
            let a = match first {
                Some(e) => format!("first answer in event {e}"),
                None => "no answer in events 0..2".into(),
            };
            let w = why.first().map(|w| format!(" ({w})")).unwrap_or_default();
            missed.push(format!(
                "WinOffset {off} WinSize {size}, first packet {} ms into the window: {a}{w}",
                py::float(pos)
            ));
        }
        s.drop_link(6000.0)?;
    }
    expect(
        missed.is_empty(),
        format!(
            "{} of {} positions not heard: {}",
            missed.len(),
            cases.len(),
            missed.iter().take(4).cloned().collect::<Vec<_>>().join("; ")
        ),
    )?;
    Ok(format!("{} positions heard in event 0, e.g. WinOffset 24 WinSize 8 at 9.97 ms", cases.len()))
}

fn second_window(s: &mut BleSession) -> R<String> {
    let o = LinkOpts::new().set("interval", 24).set("timeout", 200).set("winoffset", 3).set("winsize", 4).set("anchor", "4.0");
    s.connect_with(o, false, 8000.0)?;
    s.silent(Some(0))?;
    if !connected_event(s, 1, 6000.0, 2.0)? {
        return Err(Stop::Inconclusive(format!("no connection (state {})", s.lk().st.state())));
    }
    s.silent_off()?;
    s.link_until(500.0, None, |s| Ok(if s.lk().st.event() >= 4 { Some(()) } else { None }))?;
    s.link_run(5.0)?;
    let chm0 = link_chm(s);
    let (exp, _) = csa1(7, &chm0, 5, 0);
    let m = miss_texts(s)?;
    expect(
        !s.lk().pdus.is_empty(),
        format!(
            "no answer to the central's first packet sent 4.0 ms into the third transmit window (event 2); {}",
            m.iter().take(2).cloned().collect::<Vec<_>>().join("; ")
        ),
    )?;
    let p = s.lk().pdus[0].clone();
    expect(p.event == 2, format!("first answer in event {}, the central's first packet was in event 2", p.event))?;
    expect(p.ch == exp[2], format!("answer on data channel {}; the third event of hop 7 uses channel {}", p.ch, exp[2]))?;
    s.wait_event(6, None)?;
    let chm = [0x0f, 0x00, 0x00, 0xf0, 0x00];
    s.channel_map(&chm, 14)?;
    s.wait_event(22, None)?;
    s.link_run(5.0)?;
    let (seq, last) = csa1(7, &chm0, 14, 0);
    let (seq2, _) = csa1(7, &chm, 9, last);
    let want: Vec<i64> = seq.into_iter().chain(seq2).collect();
    let wrong: Vec<(i64, i64, i64)> = s
        .lk()
        .pdus
        .iter()
        .filter(|q| q.event >= 0 && (q.event as usize) < want.len() && q.ch != want[q.event as usize])
        .map(|q| (q.event, q.ch, want[q.event as usize]))
        .collect();
    let after = s.lk().pdus.iter().filter(|q| q.event >= 14).count();
    expect(wrong.is_empty(), format!("channels (event, used, expected): {}", triples(&wrong[..wrong.len().min(4)])))?;
    let m = miss_texts(s)?;
    expect(
        after >= 5,
        format!(
            "after a channel map change at instant 14 the bulb answered {after} of the events 14..22: its event \
             counter is not the central's; {}",
            tail(&m, 2).join("; ")
        ),
    )?;
    Ok(format!("first answer in event 2 on channel {}; map change at instant 14 followed", p.ch))
}

fn six_events(s: &mut BleSession) -> R<String> {
    let iv = 40i64;
    let o = LinkOpts::new().set("interval", iv).set("timeout", 600).set("winoffset", 0).set("winsize", 2);
    s.connect_with(o, false, 8000.0)?;
    s.silent(Some(0))?;
    let ok = s.until(6000.0, 5.0, |s| Ok(if s.poll()?.state() == "connected" { Some(()) } else { None }))?;
    if ok.is_none() {
        return Err(Stop::Inconclusive(format!("the central sent no CONNECT_IND (state {})", s.lk().st.state())));
    }
    let t_end = s.lk().st.f("tconnend").unwrap_or(0.0);
    let win0 = t_end + 1.25;
    s.run(6000.0 + 600.0)?;
    let rx = s.rx_windows(t_end);
    if rx.is_empty() {
        return Err(Stop::Inconclusive("the bulb opened no receive window after the CONNECT_IND".into()));
    }
    let chm = link_chm(s);
    let (exp, _) = csa1(7, &chm, 200, 0);
    let ivms = iv as f64 * 1.25;
    let ev_of = |ms: f64| ((ms - win0 + 5.0) / ivms).floor() as i64;
    let events: Vec<i64> = rx.iter().map(|(ms, _)| ev_of(*ms)).collect::<BTreeSet<_>>().into_iter().collect();
    let last_ms = rx.iter().map(|r| r.0).fold(f64::NEG_INFINITY, f64::max);
    let at = |i: i64| -> i64 {
        let i = i.min(199);
        if i < 0 {
            exp[(exp.len() as i64 + i) as usize]
        } else {
            exp[i as usize]
        }
    };
    let wrong: Vec<(f64, u8)> = rx.iter().filter(|(ms, ch)| *ch as i64 != at(ev_of(*ms))).cloned().collect();
    let adv: Vec<_> = s.adv(t_end, false).into_iter().filter(|p| p.ty() == 0).collect();
    let back = match adv.first() {
        Some(p) => format!("advertising again {:.0} ms after the CONNECT_IND", p.ms - t_end),
        None => "no advertising in 6.6 s".into(),
    };
    let mx = *events.last().unwrap();
    expect(
        mx <= 5,
        format!(
            "the bulb listened in {} connection events, the last one {:.0} ms after the CONNECT_IND (6 events = {:.0} \
             ms; the supervision timeout of this connection was 6000 ms); {back}",
            events.len(),
            last_ms - t_end,
            6.0 * ivms
        ),
    )?;
    expect(
        mx >= 3,
        format!(
            "the bulb gave up after {} events, {:.0} ms after the CONNECT_IND: before the timer reached 6 x connInterval",
            mx + 1,
            last_ms - t_end
        ),
    )?;
    expect(
        wrong.is_empty(),
        format!(
            "receive windows on channels other than the event's: {}",
            py::list(wrong.iter().take(3).map(|(ms, ch)| py::tuple([py::float(*ms), ch.to_string()])))
        ),
    )?;
    Ok(format!("listened in events {}..{}, then stopped; {back}", events[0], mx))
}

fn sent_at(sent: &[(i64, f64)], ev: i64) -> Option<f64> {
    sent.iter().rev().find(|(e, _)| *e == ev).map(|(_, t)| *t)
}

fn type_error() -> Stop {
    Stop::Error("TypeError: unsupported operand type(s) for -: 'NoneType' and 'float'".into())
}

fn supervision_timeout(s: &mut BleSession) -> R<String> {
    let (iv, to) = (24i64, 150i64);
    let ivms = iv as f64 * 1.25;
    s.connect(LinkOpts::new().set("interval", iv).set("timeout", to))?;
    s.wait_event(10, None)?;
    let k = s.event()?;
    s.silent(Some(k + 1))?;
    let sent = s.sent()?;
    let t_last = sent_at(&sent, k).ok_or_else(|| Stop::Error(format!("KeyError: {k}")))?;
    let back = k + ((to as f64 * 10.0 - 4.0 * ivms) / ivms).floor() as i64;
    s.wait_event(back - 1, None)?;
    s.silent_off()?;
    s.wait_event(back + 3, None)?;
    s.link_run(5.0)?;
    let got = s.lk().pdus.iter().filter(|p| p.event >= back).count();
    let t_back = sent_at(&s.sent()?, back).ok_or_else(type_error)?;
    let quiet = t_back - t_last;
    let m = miss_texts(s)?;
    expect(
        got > 0,
        format!(
            "the central was silent for {quiet:.0} ms of a {} ms timeout; the bulb did not answer when it came back: {}",
            to * 10,
            tail(&m, 2).join("; ")
        ),
    )?;
    let k = s.event()?;
    s.silent(Some(k + 1))?;
    let t_last = sent_at(&s.sent()?, k).ok_or_else(|| Stop::Error(format!("KeyError: {k}")))?;
    let n = ((to as f64 * 10.0 + 3.0 * ivms) / ivms).floor() as i64;
    s.wait_event(k + n - 1, None)?;
    s.silent_off()?;
    s.wait_event(k + n + 4, None)?;
    s.link_run(5.0)?;
    let t_resume = sent_at(&s.sent()?, k + n);
    let late = s.lk().pdus.iter().filter(|p| p.event >= k + n).count();
    let rx: Vec<f64> = s.rx_windows(t_last).into_iter().map(|r| r.0).collect();
    let last_rx = if rx.is_empty() { t_last } else { rx.iter().copied().fold(f64::NEG_INFINITY, f64::max) };
    let t_resume = t_resume.ok_or_else(type_error)?;
    expect(
        late == 0,
        format!("the bulb answered {:.0} ms after the last packet it had received (timeout {} ms)", t_resume - t_last, to * 10),
    )?;
    expect(
        last_rx <= t_last + (to * 10) as f64 + ivms,
        format!("receive windows until {:.0} ms after the last received packet (timeout {} ms)", last_rx - t_last, to * 10),
    )?;
    expect(
        last_rx >= t_last + (to * 10) as f64 - 3.0 * ivms,
        format!(
            "the last receive window was {:.0} ms after the last received packet: the bulb gave up before the {} ms timeout",
            last_rx - t_last,
            to * 10
        ),
    )?;
    let adv = s.wait_adv(1, 3000.0, Some(t_last), 0)?;
    let a = match adv.first() {
        Some(p) => format!("advertising again after {:.0} ms", p.ms - t_last),
        None => "no advertising 3 s later".into(),
    };
    Ok(format!(
        "kept through {quiet:.0} ms of silence; last receive window {:.0} ms after the last packet (timeout {} ms); {a}",
        last_rx - t_last,
        to * 10
    ))
}

fn peripheral_latency(s: &mut BleSession) -> R<String> {
    let (lat, iv, to) = (4i64, 24i64, 100i64);
    let mut tries = 0;
    let got = loop {
        tries += 1;
        s.connect(LinkOpts::new().set("interval", iv).set("latency", lat).set("timeout", to))?;
        s.wait_event(120, None)?;
        s.link_run(3.0)?;
        let got: Vec<i64> = s.lk().pdus.iter().map(|p| p.event).collect::<BTreeSet<_>>().into_iter().collect();
        if got.contains(&0) && got.contains(&1) {
            break got;
        }
        let lost = s.lost_to_154(&[0, 1])?;
        if lost.is_empty() {
            break got;
        }
        if tries == 3 {
            return Err(Stop::Inconclusive(format!(
                "in each of {tries} connections the bulb's radio was with 802.15.4 in event 0 or 1 (last: {})",
                py::ints(lost.iter().copied())
            )));
        }
        s.drop_link(6000.0)?;
    };
    expect(
        got.contains(&0) && got.contains(&1),
        format!("events answered at the start: {} (0 and 1 are required)", py::ints(got.iter().take(6).copied())),
    )?;
    let gaps: Vec<(i64, i64)> =
        got.windows(2).filter(|w| (w[1] - w[0]) as f64 * iv as f64 * 1.25 >= (to * 10) as f64).map(|w| (w[0], w[1])).collect();
    let state = s.lk().st.state().to_string();
    expect(
        state == "connected" && gaps.is_empty(),
        format!(
            "no answer between events {} ({} ms timeout); link {state}",
            py::list(gaps.iter().take(2).map(|(a, b)| py::tuple([a.to_string(), b.to_string()]))),
            to * 10
        ),
    )?;
    let used = got.windows(2).map(|w| w[1] - w[0]).max().unwrap_or(0) - 1;
    Ok(format!("events 0 and 1 answered; afterwards the bulb skipped at most {used} events in a row (latency {lat} allowed)"))
}
