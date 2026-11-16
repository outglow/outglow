use std::collections::BTreeSet;

use super::ll_conn_setup::{link_chm, miss_texts, tail, triples};
use crate::ble::harness::{csa1, hx, BleSession, LinkOpts, CID_ATT};
use crate::ble::{BleTest, Level};
use crate::py;
use crate::session::{expect, Stop, R};

const DOC: &str = "Core 5.4 Vol 6 Part B";

pub fn tests() -> Vec<BleTest> {
    vec![
        BleTest {
            id: "ll_conn_csa1_all_channels",
            doc: DOC,
            clause: "4.5.8.2",
            req: "With all 37 data channels used, each connection event uses (last unmapped channel + hopIncrement) \
                  mod 37, starting from 0, for every hop increment from 5 to 16.",
            run: csa1_all,
            ..BleTest::D
        },
        BleTest {
            id: "ll_conn_csa1_sparse_map",
            doc: DOC,
            clause: "4.5.8.2",
            req: "An unmapped channel that is not in the channel map is replaced by the used channel at index \
                  (unmapped mod number of used channels) of the ascending list of used channels.",
            run: csa1_sparse,
            ..BleTest::D
        },
        BleTest {
            id: "ll_conn_counts_missed_events",
            doc: DOC,
            clause: "4.5.1",
            req: "connEventCounter and the channel sequence advance for every connection event, also for those in \
                  which nothing was received.",
            run: counts_missed,
            ..BleTest::D
        },
        BleTest {
            id: "ll_conn_sn_nesn",
            doc: DOC,
            clause: "4.5.9",
            req: "transmitSeqNum and nextExpectedSeqNum start at zero; every new PDU of the central is acknowledged, \
                  nothing is resent once acknowledged.",
            run: sn_nesn,
            ..BleTest::D
        },
        BleTest {
            id: "ll_conn_retransmission",
            doc: DOC,
            clause: "4.5.9",
            req: "A PDU that was not acknowledged is sent again unchanged (LLID, SN, payload), and a PDU of the \
                  central received twice is processed once.",
            run: retransmission,
            ..BleTest::D
        },
        BleTest {
            id: "ll_conn_lossy_link",
            doc: DOC,
            clause: "4.5.9",
            req: "With packets lost in both directions nothing is lost, repeated or reordered above the link layer.",
            run: lossy_link,
            ..BleTest::D
        },
        BleTest {
            id: "ll_conn_pdu_header",
            doc: DOC,
            clause: "2.4, 2.4.1, 2.4.2, 4.5.10",
            req: "Data channel PDUs of the peripheral: LLID never 0, reserved bits zero, a start fragment and a \
                  control PDU never empty, no payload over 27 octets without the data length procedure.",
            run: pdu_header,
            ..BleTest::D
        },
        BleTest {
            id: "ll_conn_central_clock_error",
            doc: DOC,
            clause: "4.2.4, 4.5.7, 2.3.3.1",
            req: "The peripheral keeps hearing a central whose clock is off by as much as the SCA field of its \
                  CONNECT_IND allows, at short and at long connection intervals.",
            run: clock_error,
            ..BleTest::D
        },
        BleTest {
            id: "ll_conn_widening_after_silence",
            doc: DOC,
            clause: "4.2.4",
            req: "After events without reception the listening window grows with the time since the last anchor, so \
                  the central is found again while its drift is within the declared accuracy.",
            run: widening_after_silence,
            ..BleTest::D
        },
        BleTest {
            id: "ll_conn_more_data",
            doc: DOC,
            clause: "4.5.6 (Table 4.4)",
            req: "When the central sets MD the peripheral listens again after its packet, so several PDUs pass in one \
                  connection event.",
            run: more_data,
            mandatory: false,
            level: Level::Should,
            ..BleTest::D
        },
    ]
}

fn str_repr(s: &str) -> String {
    let q = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut o = String::new();
    o.push(q);
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            c if c == q => {
                o.push('\\');
                o.push(q);
            }
            c => o.push(c),
        }
    }
    o.push(q);
    o
}

fn strs(v: &[String]) -> String {
    py::list(v.iter().map(|x| str_repr(x)))
}

fn read_req(h: u16) -> Vec<u8> {
    let mut v = vec![0x0A];
    v.extend_from_slice(&h.to_le_bytes());
    v
}

fn channel_check(s: &mut BleSession, hop: i64, chm: &[u8], n: i64) -> R<(usize, Vec<(i64, i64, i64)>, Vec<String>)> {
    s.wait_event(n, None)?;
    s.link_run(3.0)?;
    let (exp, _) = csa1(hop, chm, (n + 2) as usize, 0);
    let wrong: Vec<(i64, i64, i64)> = s
        .lk()
        .pdus
        .iter()
        .filter(|p| p.event >= 0 && (p.event as usize) < exp.len() && p.ch != exp[p.event as usize])
        .map(|p| (p.event, p.ch, exp[p.event as usize]))
        .collect();
    let off: Vec<String> = miss_texts(s)?.into_iter().filter(|w| w.contains("peripheral on channel")).collect();
    let n_ev = s.lk().pdus.iter().map(|p| p.event).collect::<BTreeSet<_>>().len();
    Ok((n_ev, wrong, off))
}

fn csa1_all(s: &mut BleSession) -> R<String> {
    let mut notes = Vec::new();
    for hop in [5i64, 7, 11, 16] {
        s.connect(LinkOpts::new().set("interval", 6).set("timeout", 100).set("hop", hop))?;
        let chm = link_chm(s);
        let (n, wrong, off) = channel_check(s, hop, &chm, 80)?;
        expect(
            wrong.is_empty() && off.is_empty(),
            format!(
                "hop {hop}: (event, channel, expected) {} {}",
                triples(&wrong[..wrong.len().min(4)]),
                strs(&off[..off.len().min(2)])
            ),
        )?;
        expect(n >= 60, format!("hop {hop}: the bulb answered in {n} of 81 events"))?;
        notes.push(format!("hop {hop}: {n}/81 events"));
        s.drop_link(6000.0)?;
    }
    Ok(notes.join(", "))
}

fn csa1_sparse(s: &mut BleSession) -> R<String> {
    let maps: [(&str, [u8; 5]); 4] = [
        ("2 channels", [0x01, 0x00, 0x00, 0x00, 0x10]),
        ("5 scattered", [0x02, 0x02, 0x40, 0x00, 0x01]),
        ("all but 9, 10, 11, 30", [0xff, 0xf1, 0xff, 0xbf, 0x1f]),
        ("channels 25-36", [0x00, 0x00, 0x00, 0xfe, 0x1f]),
    ];
    let mut notes = Vec::new();
    for (name, chm) in maps {
        s.connect(LinkOpts::new().set("interval", 6).set("timeout", 100).set("hop", 9).chm(&chm))?;
        let (n, wrong, off) = channel_check(s, 9, &chm, 80)?;
        expect(
            wrong.is_empty() && off.is_empty(),
            format!(
                "map {} ({name}), hop 9: (event, channel, expected) {} {}",
                hx(&chm),
                triples(&wrong[..wrong.len().min(4)]),
                strs(&off[..off.len().min(2)])
            ),
        )?;
        let m = miss_texts(s)?;
        expect(
            n >= 60,
            format!(
                "map {} ({name}): the bulb answered in {n} of 81 events; {}",
                hx(&chm),
                m.iter().take(2).cloned().collect::<Vec<_>>().join("; ")
            ),
        )?;
        notes.push(format!("{name}: {n}/81"));
        s.drop_link(6000.0)?;
    }
    Ok(notes.join(", "))
}

fn counts_missed(s: &mut BleSession) -> R<String> {
    s.connect(LinkOpts::new().set("interval", 12).set("timeout", 300).set("hop", 6))?;
    s.wait_event(12, None)?;
    let k = s.event()?;
    s.silent(Some(k + 1))?;
    s.wait_event(k + 9, None)?;
    s.silent_off()?;
    s.wait_event(k + 14, None)?;
    let chm0 = link_chm(s);
    let (exp, _) = csa1(6, &chm0, (k + 30) as usize, 0);
    let back: Vec<_> = s.lk().pdus.iter().filter(|p| p.event > k).cloned().collect();
    let m = miss_texts(s)?;
    expect(
        back.first().is_some_and(|p| p.event == k + 10),
        format!(
            "central silent in events {}..{}; first answer afterwards in event {}; {}",
            k + 1,
            k + 9,
            back.first().map(|p| p.event.to_string()).unwrap_or("None".into()),
            tail(&m, 2).join("; ")
        ),
    )?;
    let wrong: Vec<(i64, i64, i64)> =
        back.iter().filter(|p| p.ch != exp[p.event as usize]).map(|p| (p.event, p.ch, exp[p.event as usize])).collect();
    expect(wrong.is_empty(), format!("(event, channel, expected): {}", triples(&wrong[..wrong.len().min(4)])))?;
    let inst = k + 22;
    let chm = [0x00, 0xff, 0x00, 0x00, 0x0f];
    s.channel_map(&chm, inst)?;
    s.wait_event(inst + 10, None)?;
    s.link_run(3.0)?;
    let (seq, last) = csa1(6, &chm0, inst as usize, 0);
    let (seq2, _) = csa1(6, &chm, 12, last);
    let want: Vec<i64> = seq.into_iter().chain(seq2).collect();
    let wrong: Vec<(i64, i64, i64)> = s
        .lk()
        .pdus
        .iter()
        .filter(|p| p.event >= 0 && (p.event as usize) < want.len() && p.ch != want[p.event as usize])
        .map(|p| (p.event, p.ch, want[p.event as usize]))
        .collect();
    let after = s.lk().pdus.iter().filter(|p| p.event >= inst).map(|p| p.event).collect::<BTreeSet<_>>().len();
    expect(wrong.is_empty(), format!("after the map change (event, channel, expected): {}", triples(&wrong[..wrong.len().min(4)])))?;
    expect(after >= 8, format!("the bulb answered {after} of the 11 events from instant {inst} on"))?;
    Ok(format!("silent events {}..{} counted; map change at instant {inst} applied on time", k + 1, k + 9))
}

fn sn_nesn(s: &mut BleSession) -> R<String> {
    s.connect(LinkOpts::new().set("interval", 6).set("timeout", 100))?;
    for h in 1..9u16 {
        s.att_t(&read_req(h), 2000.0)?;
    }
    s.link_run(100.0)?;
    let p0 = s.lk().pdus[0].clone();
    expect(
        p0.sn() == 0 && p0.nesn() == 1,
        format!("first PDU of the bulb: SN {} NESN {} (event {})", p0.sn(), p0.nesn(), p0.event),
    )?;
    let heard = s.lk().pdus.clone();
    let mut bad = Vec::new();
    for w in heard.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        if b.sn() == a.sn() && b.raw[1..] != a.raw[1..] {
            bad.push(format!("events {}/{}: SN {} twice with different content", a.event, b.event, a.sn()));
        }
    }
    expect(bad.is_empty(), bad.iter().take(3).cloned().collect::<Vec<_>>().join("; "))?;
    let tx = s.central_log()?;
    let resent = s.lk().st.i("retx").unwrap_or(0);
    let dups = s.lk().pdus.iter().filter(|p| !p.new).count();
    let missed = tx.iter().filter(|t| t.missed).count() as i64;
    expect(
        dups == 0 || missed != 0,
        format!("{dups} PDUs of the bulb repeated although the central acknowledged each one"),
    )?;
    expect(
        resent <= missed,
        format!("the central had to resend {resent} PDUs, {missed} of its packets were not heard by the bulb"),
    )?;
    Ok(format!("{} PDUs, first SN 0 NESN 1; central resent {resent} (bulb not listening {missed} times)", heard.len()))
}

fn opt_hx(r: &Option<Vec<u8>>) -> String {
    r.as_ref().map(|x| hx(x)).unwrap_or("None".into())
}

fn retransmission(s: &mut BleSession) -> R<String> {
    s.connect(LinkOpts::new().set("interval", 6).set("timeout", 200))?;
    s.link_run(200.0)?;
    let reqs: Vec<Vec<u8>> = [3u16, 6, 9, 11, 13, 15, 3, 9, 13, 6].iter().map(|h| read_req(*h)).collect();
    let mut plain = Vec::new();
    for r in &reqs[..6] {
        plain.push(s.att_t(r, 3000.0)?);
    }
    if plain.iter().any(|x| x.is_none()) {
        return Err(Stop::Inconclusive("no answer to plain Read Requests".into()));
    }
    let refv = |r: &Vec<u8>| plain[reqs[..6].iter().position(|q| q == r).unwrap()].clone();
    let n_frames = s.lk().frames.len();
    let mut bad = Vec::new();
    for r in &reqs {
        let ev = s.event()?;
        for e in ev + 1..ev + 4 {
            s.drop_event(e)?;
        }
        let rsp = s.att_t(r, 5000.0)?;
        if rsp != refv(r) {
            bad.push(format!("request {}: answer {}, without loss {}", hx(r), opt_hx(&rsp), opt_hx(&refv(r))));
        }
    }
    s.link_run(100.0)?;
    let lost = s.lost()?;
    if lost.is_empty() {
        return Err(Stop::Inconclusive("the central lost none of the bulb's PDUs".into()));
    }
    expect(bad.is_empty(), bad.iter().take(3).cloned().collect::<Vec<_>>().join("; "))?;
    let extra = s.lk().frames[n_frames..].iter().filter(|f| f.cid == CID_ATT && f.data.first() == Some(&0x0b)).count();
    expect(
        extra == reqs.len(),
        format!(
            "{} requests, {extra} Read Responses: a request repeated by the link layer was answered more than once or \
             not at all",
            reqs.len()
        ),
    )?;
    let mut diff = Vec::new();
    for (ms, ev, _ch, raw) in &lost {
        let nxt = s.lk().pdus.iter().find(|p| p.t_end > *ms);
        let ok = nxt.is_some_and(|n| n.raw[1..] == raw[1..] && (n.raw[0] ^ raw[0]) & 0x0B == 0);
        if !ok {
            diff.push(format!(
                "lost in event {ev}: {}, next heard: {}",
                hx(raw),
                nxt.map(|n| hx(&n.raw)).unwrap_or("None".into())
            ));
        }
    }
    expect(diff.is_empty(), diff.iter().take(3).cloned().collect::<Vec<_>>().join("; "))?;
    Ok(format!(
        "{} answers lost on the way, each resent unchanged; {} requests, {extra} responses",
        lost.len(),
        reqs.len()
    ))
}

fn lossy_link(s: &mut BleSession) -> R<String> {
    s.connect(LinkOpts::new().set("interval", 6).set("timeout", 300))?;
    s.link_run(200.0)?;
    let handles = [3u16, 6, 9, 11, 13, 15];
    let mut refs = Vec::new();
    for h in handles {
        refs.push(s.att_t(&read_req(h), 3000.0)?);
    }
    if refs.iter().any(|v| v.is_none()) {
        return Err(Stop::Inconclusive("no answer to plain Read Requests".into()));
    }
    s.air(&[("lossk", "330000".into()), ("lossh", "330000".into())])?;
    let n0 = s.lk().frames.len();
    let mut bad = Vec::new();
    for i in 0..40 {
        let j = (i * 5) % handles.len();
        let h = handles[j];
        let rsp = s.att_t(&read_req(h), 8000.0)?;
        if rsp != refs[j] {
            bad.push(format!("request {i} (handle {h}): {}", rsp.as_ref().map(|r| hx(r)).unwrap_or("no answer".into())));
            if rsp.is_none() {
                break;
            }
        }
    }
    s.air_off()?;
    s.link_run(200.0)?;
    expect(
        bad.is_empty(),
        format!("{}; link state {}", bad.iter().take(3).cloned().collect::<Vec<_>>().join("; "), s.lk().st.state()),
    )?;
    let got = s.lk().frames[n0..].iter().filter(|f| f.cid == CID_ATT).count();
    let pr = &s.lk().problems;
    expect(
        got == 40 && pr.is_empty(),
        format!("40 requests, {got} attribute frames; {}", strs(&pr[..pr.len().min(2)])),
    )?;
    Ok(format!("40 requests answered in order; the central resent {} PDUs", s.lk().st.i("retx").unwrap_or(0)))
}

fn pdu_header(s: &mut BleSession) -> R<String> {
    s.connect(LinkOpts::new().set("interval", 6).set("timeout", 200))?;
    s.ll(&[0x0c, 0x0b, 0x0f, 0x00, 0x34, 0x12])?;
    s.ll(&[0x08, 0, 0, 0, 0, 0, 0, 0, 0])?;
    s.att_t(&[0x02, 100, 0], 3000.0)?;
    let mut start: u16 = 1;
    for _ in 0..12 {
        let mut q = vec![0x08];
        q.extend_from_slice(&start.to_le_bytes());
        q.extend_from_slice(&[0xFF, 0xFF, 0x03, 0x28]);
        let rsp = s.att_t(&q, 3000.0)?;
        let Some(rsp) = rsp.filter(|r| r[0] == 0x09) else { break };
        let i = rsp.len() - rsp[1] as usize;
        start = u16::from_le_bytes([rsp[i], rsp[i + 1]]).wrapping_add(1);
    }
    for h in 1..20u16 {
        s.att_t(&read_req(h), 3000.0)?;
    }
    s.link_run(1500.0)?;
    let mut bad = BTreeSet::new();
    for p in &s.lk().pdus {
        if p.llid() == 0 {
            bad.insert(format!("LLID 0: {}", hx(&p.raw)));
        }
        if p.rfu() != 0 {
            bad.insert(format!("header bits 5-7 set: {}", hx(&p.raw[..2])));
        }
        if (p.llid() == 2 || p.llid() == 3) && p.length() == 0 {
            bad.insert(format!("LLID {} with Length 0", p.llid()));
        }
        if p.length() > 27 {
            bad.insert(format!("Length {} over 27", p.length()));
        }
    }
    expect(bad.is_empty(), bad.iter().take(4).cloned().collect::<Vec<_>>().join("; "))?;
    let lk = s.lk();
    expect(lk.problems.is_empty(), lk.problems.iter().take(3).cloned().collect::<Vec<_>>().join("; "))?;
    let empty = lk.pdus.iter().filter(|p| p.empty()).count();
    let data = lk.pdus.iter().filter(|p| (p.llid() == 1 || p.llid() == 2) && p.length() > 0).count();
    let control = lk.pdus.iter().filter(|p| p.ctrl()).count();
    let longest = lk.pdus.iter().map(|p| p.length()).max().unwrap_or(0);
    Ok(format!(
        "{} PDUs checked ({{'empty': {empty}, 'data': {data}, 'control': {control}}}), longest {longest} octets",
        lk.pdus.len()
    ))
}

fn drift_timeout(iv: i64) -> String {
    let x = (iv as f64 * 1.25 * 6.0 / 10.0).floor() + 50.0;
    let m = if x < 3200.0 { py::float(x) } else { "3200".to_string() };
    let mv = if x < 3200.0 { x } else { 3200.0 };
    if 100.0 >= mv {
        "100".into()
    } else {
        m
    }
}

fn drift(s: &mut BleSession, sca: i64, ppm: i64, interval: i64, events: i64, timeout: &str) -> R<Option<BTreeSet<i64>>> {
    let o = LinkOpts::new().set("interval", interval).set("timeout", timeout).set("sca", sca);
    s.connect_with(o, false, 8000.0)?;
    s.air(&[("ppm", ppm.to_string()), ("jitter", "8".into())])?;
    let ok = s.until(8000.0 + (interval * 3) as f64, 5.0, |s| {
        let st = s.poll()?;
        Ok(if st.state() == "connected" && st.heard() { Some(()) } else { None })
    })?;
    if ok.is_none() {
        s.air_off()?;
        return Ok(None);
    }
    s.wait_event(events, Some((events + 3) as f64 * interval as f64 * 1.25 + 2000.0))?;
    s.link_run(5.0)?;
    s.air_off()?;
    Ok(Some(s.lk().pdus.iter().map(|p| p.event).collect()))
}

fn clock_error(s: &mut BleSession) -> R<String> {
    let cases = [(1i64, 240i64, 24i64, 120i64), (1, -240, 24, 120), (0, 480, 800, 14), (0, -480, 800, 14), (5, 45, 1600, 8),
                 (7, -18, 3200, 6), (1, 240, 3200, 6)];
    let (mut notes, mut bad) = (Vec::new(), Vec::new());
    for (sca, ppm, iv, n) in cases {
        let to = drift_timeout(iv);
        let got = drift(s, sca, ppm, iv, n, &to)?;
        let drift_us = ppm.abs() as f64 * iv as f64 * 1.25 / 1000.0;
        let name = format!("SCA {sca}, {ppm:+} ppm, interval {:.0} ms ({drift_us:.0} us per event)", iv as f64 * 1.25);
        match got {
            None => bad.push(format!("{name}: no connection")),
            Some(got) => {
                let miss: Vec<i64> = (1..n).filter(|e| !got.contains(e)).collect();
                let mut run = 0;
                for e in 1..n {
                    run = if !got.contains(&e) { run + 1 } else { 0 };
                    if run >= 3 {
                        break;
                    }
                }
                if s.lk().st.state() != "connected" || run >= 3 || miss.len() as i64 > 2i64.max(n / 5) {
                    let sent = s.sent()?;
                    let tconnend = s.lk().st.f("tconnend").unwrap_or(0.0);
                    let rx = s.rx_windows(tconnend);
                    let sent_d = |e: i64| sent.iter().rev().find(|(x, _)| *x == e).map(|(_, t)| *t);
                    let mut late: Vec<f64> = Vec::new();
                    for e in &miss {
                        if let Some(t) = sent_d(*e) {
                            if !rx.is_empty() {
                                let mut best = rx[0].0 - t;
                                for w in &rx[1..] {
                                    if (w.0 - t).abs() < best.abs() {
                                        best = w.0 - t;
                                    }
                                }
                                late.push(best);
                            }
                        }
                    }
                    let m = miss_texts(s)?;
                    let why: Vec<String> = tail(&m, 3).iter().cloned().collect::<BTreeSet<_>>().into_iter().collect();
                    let why = why.join("; ");
                    let state = s.lk().st.state().to_string();
                    s.drop_link(6000.0)?;
                    let got0 = drift(s, sca, 0, iv, n, &to)?;
                    let miss0: Vec<i64> = (1..n).filter(|e| got0.as_ref().map_or(true, |g| !g.contains(e))).collect();
                    if miss0.len() * 2 >= miss.len() && state == "connected" {
                        return Err(Stop::Inconclusive(format!(
                            "{name}: {} events not answered, but {} also without the clock error",
                            miss.len(),
                            miss0.len()
                        )));
                    }
                    let lmin = late.iter().copied().fold(f64::INFINITY, f64::min);
                    let lmax = late.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                    let late_txt = if !late.is_empty() && lmin > 0.0 {
                        format!(
                            "; in those events the receiver was opened {:.0}..{:.0} us after the start of the central's packet",
                            lmin * 1000.0,
                            lmax * 1000.0
                        )
                    } else {
                        String::new()
                    };
                    bad.push(format!(
                        "{name}: events not answered {}{} of {n}; {why}{late_txt}; with an exact clock {}/{} of the same \
                         events are answered",
                        py::ints(miss.iter().take(8).copied()),
                        if miss.len() > 8 { "..." } else { "" },
                        n - 1 - miss0.len() as i64,
                        n - 1
                    ));
                }
                notes.push(format!("{name}: {}/{n}", n - miss.len() as i64));
            }
        }
        s.drop_link(6000.0)?;
    }
    expect(bad.is_empty(), bad.iter().take(3).cloned().collect::<Vec<_>>().join(" | "))?;
    Ok(notes.join("; "))
}

fn widening_after_silence(s: &mut BleSession) -> R<String> {
    let mut notes = Vec::new();
    for (ppm, n) in [(240i64, 6i64), (-240, 10), (240, 16)] {
        let o = LinkOpts::new().set("interval", 400).set("timeout", 1500).set("sca", 1);
        s.connect_with(o, false, 8000.0)?;
        s.air(&[("ppm", ppm.to_string())])?;
        let ok = s.until(9000.0, 5.0, |s| {
            let st = s.poll()?;
            Ok(if st.state() == "connected" && st.heard() { Some(()) } else { None })
        })?;
        if ok.is_none() {
            return Err(Stop::Inconclusive("no connection with a drifting central".into()));
        }
        s.wait_event(3, Some(4000.0))?;
        let k = s.event()?;
        s.silent(Some(k + 1))?;
        s.wait_event(k + n, Some(((n + 2) * 520) as f64))?;
        s.silent_off()?;
        s.wait_event(k + n + 3, Some((4 * 520) as f64))?;
        s.link_run(10.0)?;
        let back: Vec<i64> = s.lk().pdus.iter().filter(|p| p.event > k).map(|p| p.event).collect::<BTreeSet<_>>().into_iter().collect();
        let off_us = ppm.abs() as f64 * (n + 1) as f64 * 500.0 / 1000.0;
        let m = miss_texts(s)?;
        let why: Vec<String> = tail(&m, 3).iter().cloned().collect::<BTreeSet<_>>().into_iter().collect();
        expect(
            back.first() == Some(&(k + n + 1)),
            format!(
                "central {ppm:+} ppm, silent for {n} events of 500 ms ({off_us:.0} us off at its next packet): first \
                 answer in event {}, expected {}; {}",
                back.first().map(|e| e.to_string()).unwrap_or("None".into()),
                k + n + 1,
                why.join("; ")
            ),
        )?;
        notes.push(format!("{ppm:+} ppm after {n} silent events ({off_us:.0} us): heard"));
        s.air_off()?;
        s.drop_link(6000.0)?;
    }
    Ok(notes.join("; "))
}

fn more_data(s: &mut BleSession) -> R<String> {
    s.connect(LinkOpts::new().set("interval", 40).set("timeout", 200).set("maxexch", 8))?;
    s.link_run(300.0)?;
    let e = s.event()?;
    s.wait_event(e + 1, None)?;
    for _ in 0..4 {
        s.ll(&[0x12])?;
    }
    let k = s.event()?;
    s.wait_event(k + 6, None)?;
    let tx: Vec<_> = s.central_log()?.into_iter().filter(|t| t.event > k && t.len > 0).collect();
    if tx.len() < 4 {
        return Err(Stop::Inconclusive(format!("the central sent {} of the 4 PDUs", tx.len())));
    }
    let evs: Vec<i64> = tx[..4].iter().map(|t| t.event).collect();
    let lost: Vec<i64> = tx.iter().filter(|t| t.missed).map(|t| t.event).collect();
    let uniq: BTreeSet<i64> = evs.iter().copied().collect();
    let per_event = uniq.iter().map(|e| evs.iter().filter(|x| *x == e).count()).max().unwrap_or(0);
    expect(
        uniq.len() == 1 && lost.is_empty(),
        format!(
            "4 PDUs queued with MD=1 went out in events {} (not heard by the bulb in {}): at most {per_event} per event",
            py::ints(evs.iter().copied()),
            py::ints(lost.iter().copied())
        ),
    )?;
    Ok(format!("4 PDUs of the central and the bulb's answers in event {}", evs[0]))
}
