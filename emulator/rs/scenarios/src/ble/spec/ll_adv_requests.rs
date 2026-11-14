//! Link Layer, advertising: scan requests and connection requests heard by the
//! advertiser (Core 5.4 Vol 6 Part B 4.4.2.3, 2.3.2, 2.3.3, 1.3);
//! emulator/spec_ble/ll_adv_requests.py.

use crate::ble::harness::{hx, AdvPkt, BleSession, ConnInd, LinkOpts};
use crate::ble::BleTest;
use crate::py;
use crate::session::{expect, Stop, R};

const DOC: &str = "Core 5.4 Vol 6 Part B";

pub fn tests() -> Vec<BleTest> {
    vec![
        BleTest {
            id: "ll_adv_scan_rsp",
            doc: DOC,
            clause: "4.4.2.3, 2.3.2.2",
            req: "A SCAN_REQ that contains the advertiser's address is answered with a SCAN_RSP on the same \
                  advertising channel index, carrying the same AdvA.",
            run: scan_rsp,
            ..BleTest::D
        },
        BleTest {
            id: "ll_adv_scan_req_other_device",
            doc: DOC,
            clause: "4.4.2.3, 2.3.2.1",
            req: "A SCAN_REQ whose AdvA is another device's address is not answered.",
            run: scan_req_other,
            ..BleTest::D
        },
        BleTest {
            id: "ll_adv_scan_req_other_addr_type",
            doc: DOC,
            clause: "4.4.2.3, 1.3",
            req: "A SCAN_REQ with the advertiser's 48 bits but the other address type (RxAdd) is not answered: the \
                  type is part of the address.",
            run: scan_req_other_type,
            ..BleTest::D
        },
        BleTest {
            id: "ll_adv_connect_ind_other_device",
            doc: DOC,
            clause: "4.4.2.3, 2.3.3.1",
            req: "A CONNECT_IND whose AdvA is another device's address does not take the advertiser into the \
                  Connection state.",
            run: connect_other,
            ..BleTest::D
        },
        BleTest {
            id: "ll_adv_connect_ind_other_addr_type",
            doc: DOC,
            clause: "4.4.2.3, 1.3",
            req: "A CONNECT_IND with the advertiser's 48 bits but the other address type (RxAdd) does not start a \
                  connection.",
            run: connect_other_type,
            ..BleTest::D
        },
        BleTest {
            id: "ll_adv_connect_ind_invalid",
            doc: DOC,
            clause: "2.3.3.1, 4.5.1-4.5.3",
            req: "A CONNECT_IND whose LLData is outside the permitted ranges does not leave the device unusable: it \
                  advertises again and accepts a valid connection, without a reset.",
            run: connect_invalid,
            mandatory: false,
            ambiguous: Some(
                "the text states the ranges of Interval, Timeout, Hop, WinSize and the channel map as obligations of \
                 the initiator and does not say what an advertiser does with a request that breaks them (ignore it, \
                 or enter the Connection state and lose the connection after six events, 4.5.2); the only \
                 expectation here is that either way the device recovers",
            ),
            ..BleTest::D
        },
    ]
}

/// (time of the ADV_IND, channel, packets sent in the 2.5 ms after it, of those the stopped ones).
type Answer = (f64, i64, Vec<AdvPkt>, Vec<AdvPkt>);

/// Answers the next n ADV_IND on channel ch with pdu.
fn answers(s: &mut BleSession, pdu: &[u8], ch: i64, n: usize, wait_ms: f64) -> R<Vec<Answer>> {
    s.address()?;
    s.run(50.0)?;
    let n0 = s.adv_sent(0)?.len();
    let t0 = s.now();
    s.adv_reply(pdu, n as i64, ch)?;
    s.until(wait_ms * n as f64, 20.0, |s| Ok(if s.adv_sent(0)?.len() >= n0 + n { Some(()) } else { None }))?;
    s.run(20.0)?;
    s.adv_reply_clear()?;
    let mut all = s.adv_sent(0)?;
    let sent = all.split_off(n0.min(all.len()));
    let pk = s.adv(t0, true);
    let mut out = Vec::new();
    for (ms, c, _) in sent {
        let adv = pk.iter().position(|p| p.ch as i64 == c && (p.ms - ms).abs() < 0.5 && p.ty() == 0);
        let t_adv = adv.map(|i| pk[i].ms).unwrap_or(ms);
        let after: Vec<AdvPkt> = pk
            .iter()
            .enumerate()
            .filter(|(i, p)| t_adv < p.ms && p.ms <= t_adv + 2.5 && Some(*i) != adv)
            .map(|(_, p)| p.clone())
            .collect();
        let uncut = after.iter().filter(|p| p.cut.is_none()).cloned().collect();
        let cut = after.iter().filter(|p| p.cut.is_some()).cloned().collect();
        out.push((t_adv, c, uncut, cut));
    }
    Ok(out)
}

/// Scan responses the firmware launched and stopped again: how long before their start.
fn cut_note(res: &[Answer]) -> String {
    let cut: Vec<f64> = res.iter().flat_map(|(_, _, _, c)| c.iter().filter(|p| p.ty() == 4).map(|p| p.cut.unwrap())).collect();
    if cut.is_empty() {
        return String::new();
    }
    let mx = cut.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if mx < 0.0 {
        return format!(
            " (a SCAN_RSP was launched and stopped {:.0} us before its first bit: counted as not sent, which assumes \
             the chip stays silent then; the radio model logs the packet)",
            -mx * 1000.0
        );
    }
    format!(" (the radio was stopped during a SCAN_RSP already on the air, {} times)", cut.len())
}

fn rev_colon(a: &[u8]) -> String {
    let v: Vec<u8> = a.iter().rev().copied().collect();
    py::hex_sep(&v, ":")
}

const SCANA: [u8; 6] = [0x66, 0x55, 0x44, 0x33, 0x22, 0x11];

fn scan_rsp(s: &mut BleSession) -> R<String> {
    // 4.4.2.3: a SCAN_REQ with the advertiser's address gets a SCAN_RSP on the same primary channel index;
    // 2.3.2.2: payload AdvA + 0-31 octets.
    let (adva, txadd) = s.address()?;
    let mut seen37: Option<AdvPkt> = None;
    for ch in [37i64, 38, 39] {
        let q = s.scan_req_default()?;
        let res = answers(s, &q, ch, 2, 1500.0)?;
        if res.is_empty() {
            return Err(Stop::Inconclusive(format!("no ADV_IND on channel {ch} to answer")));
        }
        for (_, c, after, _) in &res {
            let rsp: Vec<&AdvPkt> = after.iter().filter(|p| p.ty() == 4).collect();
            expect(
                !rsp.is_empty(),
                format!(
                    "SCAN_REQ after the ADV_IND on channel {c}: no SCAN_RSP; the bulb then sent {}",
                    py::list(after.iter().map(|p| py::tuple([p.ch.to_string(), p.ty().to_string()])))
                ),
            )?;
            let r = rsp[0];
            expect(r.ch as i64 == *c, format!("SCAN_REQ on channel {c} answered on channel {}", r.ch))?;
            expect(
                r.adva() == &adva[..] && r.txadd() == txadd,
                format!(
                    "SCAN_RSP AdvA {} TxAdd {}, ADV_IND AdvA {} TxAdd {txadd}",
                    hx(r.adva()),
                    r.txadd(),
                    hx(&adva)
                ),
            )?;
            expect((6..=37).contains(&r.length()) && r.rfu() == 0, format!("SCAN_RSP header {}", hx(&r.pdu[..2])))?;
            if *c == 37 {
                seen37 = Some(r.clone());
            }
        }
    }
    Ok(format!("SCAN_RSP on 37, 38 and 39; data {}", hx(seen37.unwrap().data())))
}

fn scan_req_other(s: &mut BleSession) -> R<String> {
    // 4.4.2.3: a reply only for a SCAN_REQ "that contains its device address"; 2.3.2.1: AdvA is the
    // address of the device the PDU is addressed to.
    let (adva, _) = s.address()?;
    let mut other = adva.clone();
    other[0] ^= 0x01;
    let far: Vec<u8> = adva.iter().map(|b| b ^ 0xFF).collect();
    let mut notes = Vec::new();
    for (name, a) in [("one bit off", other), ("unrelated", far)] {
        let q = s.scan_req(Some(&a), None, &SCANA, 0)?;
        let res = answers(s, &q, 37, 3, 1500.0)?;
        if res.is_empty() {
            return Err(Stop::Inconclusive("no ADV_IND to answer".into()));
        }
        let hit: usize = res.iter().map(|(_, _, after, _)| after.iter().filter(|p| p.ty() == 4).count()).sum();
        expect(
            hit == 0,
            format!(
                "SCAN_REQ for {} ({name}; own address {}) was answered with a SCAN_RSP {hit} times of {}",
                rev_colon(&a),
                rev_colon(&adva),
                res.len()
            ),
        )?;
        notes.push(format!("{name}: 0/{}{}", res.len(), cut_note(&res)));
    }
    // negative control: the same request with the right address is answered
    let q = s.scan_req_default()?;
    let res = answers(s, &q, 37, 2, 1500.0)?;
    if !res.iter().any(|(_, _, after, _)| after.iter().any(|p| p.ty() == 4)) {
        return Err(Stop::Inconclusive("the control request with the right address was not answered either".into()));
    }
    Ok(format!("answered {}; control with the own address answered", notes.join(", ")))
}

fn scan_req_other_type(s: &mut BleSession) -> R<String> {
    // 1.3: an address comparison includes the device address type; 2.3.2.1: RxAdd is the type of AdvA.
    let (_, txadd) = s.address()?;
    let q = s.scan_req(None, Some(txadd ^ 1), &SCANA, 0)?;
    let res = answers(s, &q, 37, 3, 1500.0)?;
    if res.is_empty() {
        return Err(Stop::Inconclusive("no ADV_IND to answer".into()));
    }
    let hit: usize = res.iter().map(|(_, _, after, _)| after.iter().filter(|p| p.ty() == 4).count()).sum();
    let q = s.scan_req_default()?;
    let ctl = answers(s, &q, 37, 2, 1500.0)?;
    if !ctl.iter().any(|(_, _, after, _)| after.iter().any(|p| p.ty() == 4)) {
        return Err(Stop::Inconclusive("the control request with the right address type was not answered either".into()));
    }
    expect(
        hit == 0,
        format!(
            "SCAN_REQ with RxAdd={} (the bulb advertises TxAdd={txadd}) answered {hit} times of {}",
            txadd ^ 1,
            res.len()
        ),
    )?;
    Ok(format!("not answered; control answered{}", cut_note(&res)))
}

/// Receive windows of a connection (radio command 0x82) opened after t0, and ADV_IND sent.
fn in_connection(s: &mut BleSession, t0: f64, wait_ms: f64) -> R<(Vec<(f64, u8)>, Vec<AdvPkt>)> {
    s.run(wait_ms)?;
    let rx = s.rx_windows(t0);
    let adv = s.adv(t0, false).into_iter().filter(|p| p.ty() == 0).collect();
    Ok((rx, adv))
}

fn wait_sent(s: &mut BleSession, more_than: usize, at_least: bool) -> R<Option<()>> {
    s.until(2000.0, 20.0, |s| {
        let n = s.adv_sent(0)?.len();
        Ok(if (at_least && n >= more_than) || (!at_least && n > more_than) { Some(()) } else { None })
    })
}

fn connect_other(s: &mut BleSession) -> R<String> {
    // 4.4.2.3: the Connection state is entered for "a CONNECT_IND PDU that contains its device address".
    let (adva, _) = s.address()?;
    let mut other = adva.clone();
    other[0] ^= 0x01;
    s.run(50.0)?;
    let ci = s.connect_ind(&ConnInd { adva: Some(other.clone()), ..ConnInd::default() })?;
    s.adv_reply(&ci, 1, 37)?;
    wait_sent(s, 1, true)?;
    let t1 = s.now();
    let (rx, adv) = in_connection(s, t1, 1500.0)?;
    expect(
        rx.is_empty(),
        format!(
            "after a CONNECT_IND for {} the bulb opened {} connection receive windows, first on data channel {}",
            rev_colon(&other),
            rx.len(),
            rx.first().map(|r| r.1.to_string()).unwrap_or_default()
        ),
    )?;
    expect(adv.len() >= 6, format!("only {} ADV_IND in the 1.5 s after a CONNECT_IND for another device", adv.len()))?;
    // negative control: the same PDU with the right address does start a connection
    let ci = s.connect_ind(&ConnInd::default())?;
    s.adv_reply(&ci, 1, 37)?;
    let n = s.adv_sent(0)?.len();
    wait_sent(s, n, false)?;
    let t2 = s.now();
    let (rx, _) = in_connection(s, t2, 300.0)?;
    if rx.is_empty() {
        return Err(Stop::Inconclusive(
            "the control CONNECT_IND with the right address opened no receive window either".into(),
        ));
    }
    Ok(format!(
        "advertising went on ({} ADV_IND in 1.5 s); control: {} receive windows after the right address",
        adv.len(),
        rx.len()
    ))
}

fn connect_other_type(s: &mut BleSession) -> R<String> {
    // 1.3 (address comparison includes the type) with 2.3.3.1 (RxAdd = type of AdvA) and 4.4.2.3.
    let (_, txadd) = s.address()?;
    s.run(50.0)?;
    let ci = s.connect_ind(&ConnInd { rxadd: Some(txadd ^ 1), ..ConnInd::default() })?;
    s.adv_reply(&ci, 1, 37)?;
    wait_sent(s, 1, true)?;
    let t1 = s.now();
    let (rx, adv) = in_connection(s, t1, 1500.0)?;
    let ci = s.connect_ind(&ConnInd::default())?;
    s.adv_reply(&ci, 1, 37)?;
    let n = s.adv_sent(0)?.len();
    wait_sent(s, n, false)?;
    let t2 = s.now();
    let (rx2, _) = in_connection(s, t2, 300.0)?;
    if rx2.is_empty() && rx.is_empty() {
        return Err(Stop::Inconclusive(
            "the control CONNECT_IND with the right address type opened no receive window either".into(),
        ));
    }
    expect(
        rx.is_empty(),
        format!(
            "CONNECT_IND with RxAdd={} (the bulb advertises TxAdd={txadd}): the bulb opened {} connection receive \
             windows and sent {} ADV_IND in the next 1.5 s",
            txadd ^ 1,
            rx.len(),
            adv.len()
        ),
    )?;
    Ok(format!("advertising went on ({} ADV_IND in 1.5 s)", adv.len()))
}

fn connect_invalid(s: &mut BleSession) -> R<String> {
    // 4.5.1: connInterval 7.5 ms..4 s; 4.5.2: timeout 100 ms..32 s and > (1+latency)*interval*2; 4.5.3:
    // WinSize 1.25 ms..min(10 ms, interval-1.25 ms); 2.3.3.1: Hop 5..16, ChM with used channels.
    s.address()?;
    let d = ConnInd::default;
    let cases: Vec<(&str, ConnInd)> = vec![
        ("interval 0", ConnInd { interval: 0, ..d() }),
        ("interval 5 (6.25 ms)", ConnInd { interval: 5, ..d() }),
        ("timeout 0", ConnInd { timeout: 0, ..d() }),
        ("timeout 20 ms under 2 x interval", ConnInd { interval: 800, timeout: 2, ..d() }),
        ("hop 0", ConnInd { hop: 0, ..d() }),
        ("empty channel map", ConnInd { chm: vec![0; 5], ..d() }),
        ("one used channel", ConnInd { chm: vec![1, 0, 0, 0, 0], ..d() }),
        ("WinSize 0", ConnInd { winsize: 0, ..d() }),
        ("WinSize 255", ConnInd { winsize: 255, ..d() }),
        ("latency 60000", ConnInd { latency: 60000, ..d() }),
    ];
    let mut notes: Vec<String> = Vec::new();
    for (name, c) in &cases {
        let resets0 = s.resets();
        let n = s.adv_sent(0)?.len();
        let ci = s.connect_ind(c)?;
        s.adv_reply(&ci, 1, 37)?;
        let ok = s.until(12000.0, 20.0, |s| Ok(if s.adv_sent(0)?.len() > n { Some(()) } else { None }))?;
        if ok.is_none() {
            let prev = notes.last().cloned().unwrap_or_else(|| "boot".into());
            expect(
                false,
                format!("no ADV_IND to answer before the case '{name}': the bulb stopped advertising after '{prev}'"),
            )?;
        }
        let t1 = s.now();
        s.run(300.0)?;
        let entered = !s.rx_windows(t1).is_empty();
        let now = s.now();
        let back = s.wait_adv(1, 40000.0, Some(now), 0)?;
        expect(s.resets() == resets0, format!("firmware reset after a CONNECT_IND with {name}"))?;
        expect(!back.is_empty(), format!("no advertising within 40 s after a CONNECT_IND with {name}"))?;
        notes.push(if entered {
            format!("{name}: connection state, advertising again after {:.1} s", (back[0].ms - t1) / 1000.0)
        } else {
            format!("{name}: ignored")
        });
    }
    s.connect(LinkOpts::new())?;
    expect(s.alive(3000.0)?, "no answer in a valid connection made after the invalid requests")?;
    Ok(notes.join("; "))
}
