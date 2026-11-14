//! Link Layer, advertising: PDU formats and advertising events (Core 5.4 Vol 6
//! Part B 2.3, 4.4.2); emulator/spec_ble/ll_adv_pdu.py.

use crate::ble::harness::{hx, BleSession, LinkOpts};
use crate::ble::BleTest;
use crate::py;
use crate::session::{expect, Stop, R};

const DOC: &str = "Core 5.4 Vol 6 Part B";

pub fn tests() -> Vec<BleTest> {
    vec![
        BleTest {
            id: "ll_adv_pdu_header",
            doc: DOC,
            clause: "2.3, 2.3.1.1",
            req: "An ADV_IND has PDU type 0, reserved header bits zero and a payload of AdvA plus 0 to 31 octets of \
                  AdvData.",
            run: adv_pdu_header,
            ..BleTest::D
        },
        BleTest {
            id: "ll_adv_adva_stable",
            doc: DOC,
            clause: "2.3.1.1, 1.3",
            req: "AdvA is the advertiser's device address of the type TxAdd says, the same in every ADV_IND; a random \
                  address has a defined sub-type.",
            run: adv_adva,
            ..BleTest::D
        },
        BleTest {
            id: "ll_adv_chsel_feature",
            doc: DOC,
            clause: "2.3.1.1, 4.6.14",
            req: "ChSel in ADV_IND is 1 exactly when the advertiser supports Channel Selection Algorithm #2 \
                  (feature bit 14).",
            run: adv_chsel,
            ..BleTest::D
        },
        BleTest {
            id: "ll_adv_one_pdu_per_channel",
            doc: DOC,
            clause: "4.4.2.2",
            req: "At most one ADV_IND is sent on each advertising channel index in one advertising event.",
            run: adv_one_per_channel,
            ..BleTest::D
        },
        BleTest {
            id: "ll_adv_pdu_spacing",
            doc: DOC,
            clause: "4.4.2.3",
            req: "Consecutive ADV_IND PDUs of one advertising event start no more than 10 ms apart.",
            run: adv_spacing,
            ..BleTest::D
        },
        BleTest {
            id: "ll_adv_interval",
            doc: DOC,
            clause: "4.4.2.2.1",
            req: "Advertising events start advInterval + advDelay apart: advInterval a multiple of 0.625 ms of at \
                  least 20 ms, advDelay a pseudo-random value from 0 to 10 ms drawn for each event.",
            run: adv_interval,
            ambiguous: Some(
                "\"a (pseudo-)random value with a range 0 ms to 10 ms\": read here as 'every value lies in 0..10 ms \
                 and the values differ'; a delay that uses only part of the range passes and is reported as an \
                 observation",
            ),
            ..BleTest::D
        },
        BleTest {
            id: "ll_adv_no_other_pdu_types",
            doc: DOC,
            clause: "2.3 (Table 2.3)",
            req: "An advertiser that is not asked anything sends only its advertising PDU type; reserved PDU types \
                  are never sent.",
            run: adv_only_adv_ind,
            ..BleTest::D
        },
    ]
}

fn rev(a: &[u8]) -> Vec<u8> {
    a.iter().rev().copied().collect()
}

fn adv_pdu_header(s: &mut BleSession) -> R<String> {
    // 2.3 Figure 2.5: header = PDU Type(4) RFU(1) ChSel(1) TxAdd(1) RxAdd(1) Length(8); fields a PDU type
    // does not define are reserved (Vol 1 Part E 2.4.1: zero when sent). 2.3.1.1 Figure 2.6: ADV_IND
    // payload = AdvA (6) + AdvData (0-31), so Length is 6..37; RxAdd is not defined for ADV_IND.
    let pk = s.wait_adv(9, 6000.0, Some(0.0), 0)?;
    if pk.is_empty() {
        return Err(Stop::Inconclusive("no ADV_IND seen".into()));
    }
    let mut bad = Vec::new();
    for p in s.adv(0.0, false) {
        if p.ty() != 0 {
            bad.push(format!("PDU type {} sent while only advertising: {}", p.ty(), hx(&p.pdu)));
            continue;
        }
        if p.rfu() != 0 {
            bad.push(format!("RFU bit set: {}", hx(&p.pdu[..2])));
        }
        if p.rxadd() != 0 {
            bad.push(format!("RxAdd (not defined for ADV_IND) set: {}", hx(&p.pdu[..2])));
        }
        if !(6..=37).contains(&p.length()) {
            bad.push(format!("Length {} outside 6..37", p.length()));
        }
        if !(37..=39).contains(&p.ch) {
            bad.push(format!("sent on channel index {}", p.ch));
        }
    }
    bad.sort();
    bad.dedup();
    bad.truncate(4);
    expect(bad.is_empty(), bad.join("; "))?;
    Ok(format!("header {}, Length {}", hx(&pk[0].pdu[..2]), pk[0].length()))
}

fn adv_adva(s: &mut BleSession) -> R<String> {
    // 2.3.1.1: "The AdvA field shall contain the advertiser's public or random device address as indicated
    // by TxAdd." 1.3.2 Table 1.2: the two most significant bits of a random address select the sub-type;
    // 0b10 is reserved.
    let pk = s.wait_adv(12, 6000.0, Some(0.0), 0)?;
    if pk.len() < 6 {
        return Err(Stop::Inconclusive(format!("only {} ADV_IND seen", pk.len())));
    }
    let mut addrs: Vec<(Vec<u8>, u8)> = pk.iter().map(|p| (p.adva().to_vec(), p.txadd())).collect();
    addrs.sort();
    addrs.dedup();
    expect(addrs.len() == 1, format!("{} different (AdvA, TxAdd) pairs in {} packets", addrs.len(), pk.len()))?;
    let (a, tx) = addrs[0].clone();
    if tx != 0 {
        expect(a[5] >> 6 != 0b10, format!("random address {} has the reserved sub-type 0b10", hx(&rev(&a))))?;
    }
    expect(a.iter().any(|b| *b != 0) && a != [0xff; 6], format!("AdvA {} is not an address", hx(&rev(&a))))?;
    Ok(format!("AdvA {} ({})", py::hex_sep(&rev(&a), ":"), if tx != 0 { "random" } else { "public" }))
}

fn adv_chsel(s: &mut BleSession) -> R<String> {
    // 2.3.1.1: "The ChSel field ... shall be set to 1 if the advertiser supports the LE Channel Selection
    // Algorithm #2 feature". Table 4.7: bit 14, sent to the peer ("Y": set correctly).
    let pk = s.wait_adv(3, 4000.0, Some(0.0), 0)?;
    if pk.is_empty() {
        return Err(Stop::Inconclusive("no ADV_IND seen".into()));
    }
    let mut chsel: Vec<u8> = pk.iter().map(|p| p.chsel()).collect();
    chsel.sort();
    chsel.dedup();
    s.connect(LinkOpts::new())?;
    let mut q = vec![0x08];
    q.extend_from_slice(&[0; 8]);
    let (rsp, _) = s.ll_req(&q, &[0x09], 3000.0)?;
    let Some(rsp) = rsp.filter(|r| r.length() >= 9) else {
        return Err(Stop::Inconclusive("no LL_FEATURE_RSP to compare ChSel with".into()));
    };
    let mut fb = [0u8; 8];
    fb.copy_from_slice(&rsp.payload()[1..9]);
    let feat = u64::from_le_bytes(fb);
    let csa2 = (feat >> 14 & 1) as u8;
    expect(
        chsel == [csa2],
        format!(
            "ChSel {} in ADV_IND, feature bit 14 = {csa2} (FeatureSet {feat:#x})",
            py::ints(chsel.iter().map(|c| *c as i64))
        ),
    )?;
    Ok(format!("ChSel {csa2}, FeatureSet {feat:#018x}"))
}

fn adv_one_per_channel(s: &mut BleSession) -> R<String> {
    // 4.4.2.2: "At most one PDU shall be sent on each used advertising channel index". Events are told
    // apart by time: packets closer than 10 ms belong to one event (4.4.2.3), events are >= 20 ms apart.
    let pk = s.wait_adv(60, 12000.0, Some(0.0), 0)?;
    if pk.len() < 30 {
        return Err(Stop::Inconclusive(format!("only {} ADV_IND seen", pk.len())));
    }
    let mut ev = vec![vec![pk[0].clone()]];
    for p in &pk[1..] {
        if p.ms - ev.last().unwrap().last().unwrap().ms <= 10.0 {
            ev.last_mut().unwrap().push(p.clone());
        } else {
            ev.push(vec![p.clone()]);
        }
    }
    let chans = |e: &Vec<crate::ble::harness::AdvPkt>| -> Vec<u8> { e.iter().map(|p| p.ch).collect() };
    let bad: Vec<&Vec<_>> = ev
        .iter()
        .filter(|e| {
            let mut c = chans(e);
            c.sort();
            c.dedup();
            c.len() != e.len()
        })
        .collect();
    let eg = bad.first().map(|e| py::ints(chans(e).iter().map(|c| *c as i64))).unwrap_or_default();
    expect(bad.is_empty(), format!("{} of {} events repeat a channel, e.g. {eg}", bad.len(), ev.len()))?;
    let mut orders: Vec<Vec<u8>> = ev.iter().map(chans).collect();
    orders.sort();
    orders.dedup();
    let o = py::list(orders.iter().map(|c| py::tuple(c.iter().map(|x| x.to_string()))));
    Ok(format!("{} events, channel sequences seen: {o}", ev.len()))
}

fn adv_spacing(s: &mut BleSession) -> R<String> {
    // 4.4.2.3: "The time between the beginning of two consecutive ADV_IND PDUs within an advertising event
    // shall be less than or equal to 10 ms." An event that uses fewer than three channels is allowed; an
    // event split by a gap over 10 ms shows up as two short events much closer than advInterval (>= 20 ms).
    let pk = s.wait_adv(90, 15000.0, Some(0.0), 0)?;
    if pk.len() < 30 {
        return Err(Stop::Inconclusive(format!("only {} ADV_IND seen", pk.len())));
    }
    let gaps: Vec<f64> = pk.windows(2).map(|w| w[1].ms - w[0].ms).collect();
    let inside: Vec<f64> = gaps.iter().copied().filter(|g| *g <= 10.0).collect();
    let odd: Vec<f64> = gaps.iter().copied().filter(|g| 10.0 < *g && *g < 20.0).collect();
    expect(
        odd.is_empty(),
        format!(
            "gaps between 10 and 20 ms between ADV_IND PDUs: {} (too long for one event, too short for two)",
            py::floats(odd.iter().take(5).map(|g| py::round(*g, 3)))
        ),
    )?;
    if inside.is_empty() {
        return Err(Stop::Inconclusive("every event has a single ADV_IND: no spacing to measure".into()));
    }
    let lo = inside.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = inside.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    Ok(format!("spacing inside events {lo:.3}..{hi:.3} ms"))
}

fn adv_interval(s: &mut BleSession) -> R<String> {
    // 4.4.2.2.1: T_advEvent = advInterval + advDelay. With N events the spread of T_advEvent cannot exceed
    // 10 ms, the smallest cannot be under 20 ms, and some multiple of 0.625 ms must fit under all of them
    // with every T_advEvent within 10 ms above it. A delay drawn per event makes the intervals differ.
    let n = 160;
    s.until(200000.0, 500.0, |s| {
        let a = s.adv(0.0, false);
        Ok(if BleSession::events(&a, 0).len() >= n + 1 { Some(()) } else { None })
    })?;
    let a = s.adv(0.0, false);
    let ev = BleSession::events(&a, 0);
    if ev.len() < 40 {
        return Err(Stop::Inconclusive(format!("only {} advertising events in {:.0} s", ev.len(), s.now() / 1000.0)));
    }
    let t: Vec<f64> = ev.iter().map(|e| e[0].ms).collect();
    let d: Vec<f64> = t.windows(2).map(|w| w[1] - w[0]).collect();
    let lo = d.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = d.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    expect(lo >= 20.0 - 0.01, format!("advertising events {lo:.3} ms apart: under the 20 ms minimum"))?;
    expect(
        hi - lo <= 10.0 + 0.05,
        format!(
            "T_advEvent from {lo:.3} to {hi:.3} ms: a spread of {:.3} ms cannot come from an advDelay of 0..10 ms",
            hi - lo
        ),
    )?;
    let k = ((lo + 0.01) / 0.625) as i64;
    expect(
        hi <= k as f64 * 0.625 + 10.0 + 0.05,
        format!("no multiple of 0.625 ms fits: T_advEvent {lo:.3}..{hi:.3} ms"),
    )?;
    let mut vals: Vec<String> = d.iter().map(|x| py::float(py::round(*x, 2))).collect();
    vals.sort();
    vals.dedup();
    let distinct = vals.len();
    expect(
        distinct > d.len() / 8 && hi - lo > 0.3,
        format!(
            "T_advEvent takes {distinct} values in {} intervals, spread {:.3} ms: no per-event random delay",
            d.len(),
            hi - lo
        ),
    )?;
    Ok(format!(
        "{} intervals: T_advEvent {lo:.3}..{hi:.3} ms (spread {:.3} ms), which fits an advInterval between {:.3} and \
         {:.3} ms; the advDelay values seen span {:.1} ms of the 0..10 ms range",
        d.len(),
        hi - lo,
        20.0f64.max(hi - 10.0),
        k as f64 * 0.625,
        hi - lo
    ))
}

fn adv_only_adv_ind(s: &mut BleSession) -> R<String> {
    // 2.3: "The PDU Types marked as Reserved for Future Use shall not be sent". With no scanner and no
    // initiator on the air there is nothing to answer, so no SCAN_RSP either (4.4.2.3: SCAN_RSP is a reply).
    s.run(6000.0)?;
    let mut kinds: Vec<(u8, i64)> = Vec::new();
    for p in s.adv(0.0, false) {
        match kinds.iter_mut().find(|(k, _)| *k == p.ty()) {
            Some(e) => e.1 += 1,
            None => kinds.push((p.ty(), 1)),
        }
    }
    if kinds.is_empty() {
        return Err(Stop::Inconclusive("nothing sent on the advertising channels".into()));
    }
    let shown = format!("{{{}}}", kinds.iter().map(|(k, n)| format!("{k}: {n}")).collect::<Vec<_>>().join(", "));
    expect(kinds.iter().all(|(k, _)| [0, 2, 6].contains(k)), format!("PDU types sent with nobody asking: {shown}"))?;
    if kinds.iter().any(|(k, _)| *k != 0) {
        return Err(Stop::Fail(format!("legacy connectable device sends PDU types {shown}")));
    }
    Ok(format!("{} ADV_IND", kinds[0].1))
}
