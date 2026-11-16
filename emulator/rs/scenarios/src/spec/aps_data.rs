//! APS data service (Zigbee R22, 2.2.5 and 2.2.8.4): acknowledgements,
//! duplicate rejection, retransmissions, group and endpoint delivery,
//! transmissions due to bindings, fragmentation. Ported from
//! emulator/spec/aps_data.py. Constants from Table 2-23: apscMaxFrameRetries = 3,
//! apscAckWaitDuration = 0.05 * (2 * nwkcMaxDepth) + 0.1 = 1.6 s.

use super::Test;
use crate::frames::{read_attr_records, APS_ACK, APS_CMD, APS_DATA};
use crate::session::{expect, CapFrame, Session, Stop, R};
use crate::station::{nx, Sent, Tx};

const S: &str = "aps";
const ONOFF: u16 = 0x0006;
const GROUPS: u16 = 0x0004;
const ACK_WAIT_MS: f64 = 1600.0;
const MAX_RETRIES: usize = 3;

macro_rules! t {
    ($id:expr, $doc:expr, $clause:expr, $req:expr, $run:expr) => {
        Test { id: $id, doc: $doc, clause: $clause, req: $req, session: S, run: $run, ..Test::D }
    };
    ($id:expr, $doc:expr, $clause:expr, $req:expr, $run:expr, $($k:ident: $v:expr),+) => {
        Test { id: $id, doc: $doc, clause: $clause, req: $req, session: S, run: $run, $($k: $v),+, ..Test::D }
    };
}

pub fn tests() -> Vec<Test> {
    vec![
        t!("APS-ACK-01", "R22", "2.2.8.4.3, 2.2.5.2.3", "A data frame received with the acknowledgement request bit set is \
           answered with an APS acknowledgement: same cluster, profile and APS counter, endpoints swapped, no \
           acknowledgement request, ack format bit clear.", ack_on_request),
        t!("APS-ACK-02", "R22", "2.2.8.4.3", "A frame received with the acknowledgement request bit clear is not \
           acknowledged (it is still delivered).", no_ack_without_request),
        t!("APS-DUP-01", "R22", "2.2.8.4.2, 2.2.8.4.3", "A frame received a second time (same source and APS counter, \
           0.5 s later) is delivered to the application only once, and is acknowledged again.", duplicate_rejected),
        t!("APS-DUP-02", "R22", "2.2.8.4.2, 2.2.8.4.4", "All retransmissions the text defines are recognised as duplicates: \
           a sender whose acknowledgements are lost repeats the frame apscMaxFrameRetries = 3 times, every \
           apscAckWaitDuration = 1.6 s (copies 1.6, 3.2 and 4.8 s after the original).",
           duplicate_rejected_retransmissions),
        t!("APS-DUP-03", "R22", "2.2.8.4.2, 2.2.8.4.4", "The last retransmission alone (4.8 s after the original, the two \
           before it lost on the way) is still recognised as a duplicate.", duplicate_rejected_last_only),
        t!("APS-RETRY-01", "R22", "2.2.8.4.4", "A frame sent with acknowledgement request that is never acknowledged is \
           retransmitted apscMaxFrameRetries = 3 times and then given up.", retransmission_count),
        t!("APS-RETRY-02", "R22", "2.2.8.4.4, Table 2-23", "Each retransmission follows after apscAckWaitDuration (1.6 s) \
           without an acknowledgement.", retransmission_spacing,
           ambiguous: Some("apscAckWaitDuration is 0.05 * (2 * nwkcMaxDepth) + 0.1 s, and R22 defines no constant \
           nwkcMaxDepth; with nwkMaxDepth = 15 of the Zigbee PRO stack profile it is 1.6 s. The test allows 1.3-1.9 s")),
        t!("APS-COUNTER-01", "R22", "2.2.5.1.7", "The APS counter is incremented by one for each new transmission.",
           aps_counter),
        t!("APS-GROUP-01", "R22", "2.2.8.4.2", "A group-addressed frame (delivery mode 0b11) is delivered to an endpoint \
           that is a member of the group, and not delivered when no endpoint is a member.", group_delivery),
        t!("APS-EP-01", "R22", "2.2.8.4.2", "A unicast frame with destination endpoint 0xff is presented to all endpoints \
           1-254 of the device; a frame for an endpoint the device does not have reaches no application.",
           broadcast_endpoint),
        t!("APS-BIND-01", "R22", "2.2.8.4.1", "A transmission due to a binding with a unicast destination goes to the \
           network address of that device with delivery mode 0b00 and the destination endpoint of the binding.",
           binding_unicast),
        t!("APS-BIND-02", "R22", "2.2.8.4.1, 2.2.8.4.3", "A transmission due to a binding with a group destination has \
           delivery mode 0b11, the group address in the APS header, no destination endpoint, no acknowledgement request, \
           and is broadcast to 0xfffd.", binding_group),
        t!("APS-EXT-01", "R22", "2.2.5.1, 2.2.5.1.8", "A data frame that carries the extended header sub-frame with \
           fragmentation = 0b00 (not fragmented, no block number) is an ordinary frame and is delivered like one.",
           extended_header_unfragmented),
        t!("APS-FRAG-01", "R22", "2.2.8.4.5", "A fragmented unicast transmission (extended header, same APS counter in \
           every block) is reassembled and delivered; it is acknowledged with the extended header, the lowest block number \
           of the receive window and an ack bitfield in which the received blocks are set.", fragmentation_rx,
           mandatory: false),
        t!("APS-FRAG-02", "R22", "2.2.5.2.3", "In the acknowledgement of a fragment the fragmentation sub-field of the \
           extended frame control has the same value as in the frame it acknowledges (0b01 for the first block, 0b10 for \
           a later one).", fragmentation_ack_value, mandatory: false,
           ambiguous: Some("clear when every block is acknowledged on its own (receive window of one block); when one \
           acknowledgement covers a window of several blocks the text does not say which frame's value it mirrors, and \
           the test then accepts 0b01 or 0b10")),
    ]
}

// ---- Python-style texts
fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
fn opt<T: std::fmt::Display>(v: Option<T>) -> String {
    v.map(|x| x.to_string()).unwrap_or_else(|| "None".into())
}
/// repr() of a float rounded to 2 places.
fn py_round2(x: f64) -> String {
    let v: f64 = format!("{x:.2}").parse().unwrap();
    let s = format!("{v}");
    if s.contains('.') || s.contains('e') || s.contains("inf") || s.contains("NaN") {
        s
    } else {
        s + ".0"
    }
}
fn gaps_py(tx: &[CapFrame]) -> (Vec<f64>, String) {
    let g: Vec<f64> = tx.windows(2).map(|w| (w[1].t0 - w[0].t0) / 1000.0).collect();
    let r: Vec<f64> = g.iter().map(|x| format!("{x:.2}").parse::<f64>().unwrap()).collect();
    (r, format!("[{}]", g.iter().map(|x| py_round2(*x)).collect::<Vec<_>>().join(", ")))
}

// ---- local helpers
/// APS acknowledgements from the bulb for a counter, one per NWK frame (MAC retries removed).
fn acks(s: &mut Session, since: usize, counter: Option<u8>) -> Vec<CapFrame> {
    let dut = s.dut_short;
    let mut seen = Vec::new();
    let mut out = Vec::new();
    for f in s.dut_frames(since, |_| true) {
        let (Some(a), Some(n)) = (&f.aps, &f.nwk) else { continue };
        if a.ty == APS_ACK && n.src == dut && a.counter == counter && !seen.contains(&n.seq) {
            seen.push(n.seq);
            out.push(f);
        }
    }
    out
}

fn light_on(s: &mut Session) -> R<()> {
    s.tx_zcl(ONOFF, 0x01, &[], Tx::new().ddr());
    s.run(1500.0)?;
    if !s.light().on {
        return Err(Stop::Inconclusive("the light did not come on after On".into()));
    }
    Ok(())
}

fn is_on(s: &Session) -> bool {
    s.light().on
}

fn add_group(s: &mut Session, group: u16) -> R<()> {
    let mut pl = group.to_le_bytes().to_vec();
    pl.push(0);
    let f = s.zcl(GROUPS, 0x00, &pl, Tx::new())?;
    let ok = f.as_ref().is_some_and(|f| {
        let z = f.zcl().unwrap();
        z.specific && matches!(z.payload.first(), Some(0x00) | Some(0x8A))
    });
    if !ok {
        return Err(Stop::Inconclusive(
            "Add Group was not accepted: ".to_string() + &f.map(|f| f.brief()).unwrap_or_else(|| "no response".into()),
        ));
    }
    Ok(())
}

fn reporting_payload(max_s: u16) -> Vec<u8> {
    let mut v = vec![0x00];
    v.extend_from_slice(&0u16.to_le_bytes());
    v.push(0x10);
    v.extend_from_slice(&0u16.to_le_bytes());
    v.extend_from_slice(&max_s.to_le_bytes());
    v
}

fn configure_onoff_reporting(s: &mut Session, max_s: u16) -> R<()> {
    let f = s.zcl(ONOFF, 0x06, &reporting_payload(max_s), Tx::new().general())?;
    let ok = f.as_ref().is_some_and(|f| {
        let z = f.zcl().unwrap();
        !z.specific && z.cmd == 0x07 && z.payload.first() == Some(&0)
    });
    if !ok {
        return Err(Stop::Inconclusive(
            "Configure Reporting for OnOff was not accepted: ".to_string()
                + &f.map(|f| f.brief()).unwrap_or_else(|| "no response".into()),
        ));
    }
    Ok(())
}

fn reports(s: &mut Session, since: usize) -> Vec<CapFrame> {
    let dut = s.dut_short;
    let mut seen: Vec<(u8, Option<u64>)> = Vec::new();
    let mut out = Vec::new();
    for f in s.dut_frames(since, |_| true) {
        let Some(z) = f.zcl() else { continue };
        let n = f.nwk.as_ref().unwrap();
        let key = (n.seq, f.mac.as_ref().unwrap().dst);
        if f.aps.as_ref().unwrap().cluster == Some(ONOFF) && !z.specific && z.cmd == 0x0A && n.src == dut
            && !seen.contains(&key)
        {
            seen.push(key);
            out.push(f);
        }
    }
    out
}

fn bind(s: &mut Session, payload: &[u8], unbind: bool) -> R<Option<u8>> {
    let f = s.zdo(if unbind { 0x0022 } else { 0x0021 }, payload, 2000.0)?;
    Ok(f.map(|f| f.zdo().unwrap().payload.first().copied().unwrap_or(0)))
}

fn tc_zcl(s: &mut Session, dst: u16, cluster: u16, cmd: u8, payload: &[u8], o: Tx) -> Sent {
    s.tx_zcl(cluster, cmd, payload, o.dst(dst))
}

fn tc_zdo(s: &mut Session, cluster: u16, payload: &[u8], o: Tx) -> Sent {
    let dst = s.dut_short;
    let w = &mut s.w;
    w.coord.tx_zdo(&mut w.air, dst, cluster, payload, &o)
}

// ---- tests
fn ack_on_request(s: &mut Session) -> R<String> {
    let mut obs = Vec::new();
    let dut = s.dut_short;
    for k in 0..2 {
        let (what, want) = if k == 0 {
            ("ZCL (HA profile, endpoints 7 -> 1)", (Some(7u8), Some(1u8), Some(ONOFF), Some(0x0104u16)))
        } else {
            ("ZDO Active_EP_req", (Some(0), Some(0), Some(0x0005), Some(0x0000)))
        };
        let i = s.mark();
        let r = if k == 0 {
            tc_zcl(s, dut, ONOFF, 0x00, &0u16.to_le_bytes(), Tx::new().general().ack_req().src_ep(7))
        } else {
            tc_zdo(s, 0x0005, &dut.to_le_bytes(), Tx::new().ack_req())
        };
        s.run(1500.0)?;
        let a = acks(s, i, r.counter);
        expect(!a.is_empty(), format!("{what}: no APS acknowledgement within 1.5 s"))?;
        let ap = a[0].aps.as_ref().unwrap();
        let got = (ap.dst_ep, ap.src_ep, ap.cluster, ap.profile);
        let d = |g: (Option<u8>, Option<u8>, Option<u16>, Option<u16>)| {
            format!("{{'dst_ep': {}, 'src_ep': {}, 'cluster': {}, 'profile': {}}}", opt(g.0), opt(g.1), opt(g.2), opt(g.3))
        };
        expect(got == want, format!("{what}: acknowledgement fields {}, required {}", d(got), d(want)))?;
        expect(
            !ap.ack_req && !ap.ack_format && !ap.ext_hdr && ap.delivery == 0,
            format!(
                "{what}: acknowledgement frame control {:#04x} (ack request, ack format and extended header \
                 must be 0, delivery mode unicast)",
                ap.fc
            ),
        )?;
        let ndst = a[0].nwk.as_ref().unwrap().dst;
        expect(ndst == s.w.coord.short, format!("{what}: acknowledgement sent to {ndst:#06x}"))?;
        obs.push(format!("{what}: {}", a[0].brief()));
    }
    Ok(obs.join("; "))
}

fn no_ack_without_request(s: &mut Session) -> R<String> {
    let i = s.mark();
    let dut = s.dut_short;
    let r = tc_zcl(s, dut, ONOFF, 0x00, &0u16.to_le_bytes(), Tx::new().general());
    let zseq = r.zcl_seq;
    let rsp = s.wait_dut(
        move |f| f.zcl().is_some_and(|z| Some(z.seq) == zseq) && f.aps.as_ref().unwrap().cluster == Some(ONOFF),
        1500.0,
        Some(i),
    )?;
    s.run(1000.0)?;
    let a = acks(s, i, r.counter);
    if rsp.is_none() {
        return Err(Stop::Inconclusive("the Read Attributes request was not answered, so delivery is not shown".into()));
    }
    expect(
        a.is_empty(),
        format!(
            "an APS acknowledgement was sent for a frame without acknowledgement request: {}",
            a.first().map(|f| f.brief()).unwrap_or_default()
        ),
    )?;
    Ok(String::new())
}

/// Sends a Toggle and then copies of it (same APS counter, same payload, new
/// NWK sequence number) at the given times after the original. The light must
/// have toggled exactly once.
fn duplicate(s: &mut Session, copies_ms: &[f64]) -> R<String> {
    light_on(s)?;
    let cnt = nx(&mut s.w.coord.st.aps_ctr);
    let zseq = nx(&mut s.w.coord.st.zcl_seq);
    let i = s.mark();
    let t0 = s.now();
    let dut = s.dut_short;
    let copy = || Tx::new().ack_req().ddr().counter(cnt).seq(zseq);
    tc_zcl(s, dut, ONOFF, 0x02, &[], copy());
    s.run(copies_ms[0].min(1200.0))?;
    if is_on(s) {
        return Err(Stop::Inconclusive("the first copy of the Toggle was not executed".into()));
    }
    let mut again: Option<f64> = None;
    for &t in copies_ms {
        let d = (t0 + t - s.now()).max(0.0);
        s.run(d)?;
        tc_zcl(s, dut, ONOFF, 0x02, &[], copy());
        s.run(1000.0)?;
        if is_on(s) && again.is_none() {
            again = Some(t);
        }
    }
    let n = acks(s, i, Some(cnt)).len();
    // control: a new APS counter is a new frame and must be executed
    let before = is_on(s);
    tc_zcl(s, dut, ONOFF, 0x02, &[], Tx::new().ack_req().ddr());
    s.run(1200.0)?;
    let control = is_on(s) != before;
    light_on(s)?;
    expect(
        again.is_none(),
        match again {
            Some(a) => format!(
                "the copy sent {:.1} s after the original (same APS counter) was delivered again: the Toggle was \
                 executed a second time",
                a / 1000.0
            ),
            None => String::new(),
        },
    )?;
    expect(control, "control failed: a Toggle with a new APS counter was not executed")?;
    expect(
        n == 1 + copies_ms.len(),
        format!(
            "{n} acknowledgements for {} copies (every frame received with the acknowledgement request bit is to be \
             acknowledged)",
            1 + copies_ms.len()
        ),
    )?;
    Ok(String::new())
}

fn duplicate_rejected(s: &mut Session) -> R<String> {
    duplicate(s, &[500.0])
}

fn duplicate_rejected_retransmissions(s: &mut Session) -> R<String> {
    duplicate(s, &[ACK_WAIT_MS, 2.0 * ACK_WAIT_MS, 3.0 * ACK_WAIT_MS])
}

fn duplicate_rejected_last_only(s: &mut Session) -> R<String> {
    duplicate(s, &[3.0 * ACK_WAIT_MS])
}

type Retry = Option<(String, Vec<CapFrame>)>;

/// Makes the bulb send a frame with acknowledgement request while the Trust
/// Center sends no APS acknowledgements: (description, transmissions with
/// distinct NWK sequence numbers).
fn retry_experiment(s: &mut Session) -> R<Retry> {
    if s.has_note("aps_retry") {
        return Ok(s.note::<Retry>("aps_retry").unwrap());
    }
    s.w.coord.mute_aps_ack = true;
    let r = (|| -> R<Retry> {
        let dut = s.dut_short;
        let tc_short = s.w.coord.short;
        let mut first = None;
        let mut what = "";
        for k in 0..3 {
            what = [
                "Active_EP_rsp to a request with acknowledgement request",
                "Read Attributes Response to a request with acknowledgement request",
                "Unbind_rsp",
            ][k];
            let i = s.mark();
            match k {
                0 => {
                    tc_zdo(s, 0x0005, &dut.to_le_bytes(), Tx::new().ack_req());
                }
                1 => {
                    tc_zcl(s, dut, ONOFF, 0x00, &0u16.to_le_bytes(), Tx::new().general().ack_req());
                }
                _ => {
                    let mut pl = s.dut_ext.to_le_bytes().to_vec();
                    pl.push(1);
                    pl.extend_from_slice(&0x0B05u16.to_le_bytes());
                    pl.push(3);
                    pl.extend_from_slice(&0x00124B00DEAD0002u64.to_le_bytes());
                    pl.push(9);
                    tc_zdo(s, 0x0022, &pl, Tx::new());
                }
            }
            first = s.wait_dut(
                move |f| {
                    f.aps.as_ref().is_some_and(|a| (a.ty == APS_DATA || a.ty == APS_CMD) && a.ack_req)
                        && f.nwk.as_ref().is_some_and(|n| n.dst == tc_short && n.src == dut)
                },
                1500.0,
                Some(i),
            )?;
            if first.is_some() {
                break;
            }
        }
        let Some(first) = first else {
            s.set_note::<Retry>("aps_retry", None);
            return Ok(None);
        };
        s.run(MAX_RETRIES as f64 * 2500.0 + 3000.0)?;
        let fa = first.aps.clone().unwrap();
        let mut seen = Vec::new();
        let mut tx = Vec::new();
        for f in s.dut_frames(first.i, |_| true) {
            let (Some(a), Some(n)) = (&f.aps, &f.nwk) else { continue };
            if a.ty == fa.ty && a.counter == fa.counter && a.ack_req && a.cluster == fa.cluster && n.src == dut
                && !seen.contains(&n.seq)
            {
                seen.push(n.seq);
                tx.push(f.clone());
            }
        }
        let v: Retry = Some((what.to_string(), tx));
        s.set_note("aps_retry", v.clone());
        Ok(v)
    })();
    s.w.coord.mute_aps_ack = false;
    s.run(500.0)?;
    r
}

const NO_AR: &str = "the image requested an APS acknowledgement for nothing provoked (ZDO and ZCL responses to \
                     requests with and without acknowledgement request): the retransmission rule is about frames sent \
                     with one";

fn retransmission_count(s: &mut Session) -> R<String> {
    let Some((what, tx)) = retry_experiment(s)? else { return Err(Stop::NotApplicable(NO_AR.into())) };
    let (_, gaps) = gaps_py(&tx);
    let obs = format!("{what}: {} transmissions in 10.5 s, gaps {gaps} s", tx.len());
    expect(
        tx.len() == 1 + MAX_RETRIES,
        obs.clone()
            + &format!("; {} required (the original and {MAX_RETRIES} retransmissions)", 1 + MAX_RETRIES),
    )?;
    Ok(obs)
}

fn retransmission_spacing(s: &mut Session) -> R<String> {
    let Some((what, tx)) = retry_experiment(s)? else { return Err(Stop::NotApplicable(NO_AR.into())) };
    if tx.len() < 2 {
        return Err(Stop::Inconclusive(format!("{what}: the frame was not retransmitted at all")));
    }
    let (g, gaps) = gaps_py(&tx);
    expect(
        g.iter().all(|x| (x - ACK_WAIT_MS / 1000.0).abs() <= 0.3),
        format!("{what}: gaps between transmissions {gaps} s"),
    )?;
    Ok(format!("gaps {gaps} s"))
}

fn aps_counter(s: &mut Session) -> R<String> {
    let i = s.mark();
    let dut = s.dut_short;
    for _ in 0..3 {
        s.read(ONOFF, &[0x0000], 1500.0)?;
        s.zdo(0x0005, &dut.to_le_bytes(), 2000.0)?;
    }
    s.run(300.0)?;
    let mut seen = Vec::new();
    let mut ctrs: Vec<(u8, &str)> = Vec::new();
    type Key = (u8, Option<u8>, Option<u16>, Option<Vec<u8>>);
    let mut earlier: Vec<(Key, f64)> = Vec::new();
    for f in s.dut_frames(0, |_| true) {
        let Some(a) = &f.aps else { continue };
        let n = f.nwk.as_ref().unwrap();
        if !(a.ty == APS_DATA || a.ty == APS_CMD) || n.src != dut || seen.contains(&n.seq) {
            continue;
        }
        seen.push(n.seq);
        let key: Key = (a.ty, a.counter, a.cluster, a.payload.clone());
        let prev = earlier.iter().find(|(k, _)| *k == key).map(|(_, t)| *t);
        let retransmission = a.ack_req && prev.is_some_and(|t| f.t0 - t < 15000.0);
        if prev.is_none() {
            earlier.push((key, f.t0));
        }
        if f.i < i || retransmission {
            continue; // an APS retransmission keeps the counter of its original
        }
        let kind = if a.profile == Some(0) {
            "ZDO"
        } else if a.ty == APS_CMD {
            "cmd"
        } else {
            "ZCL"
        };
        ctrs.push((a.counter.unwrap_or(0), kind));
    }
    if ctrs.len() < 6 {
        return Err(Stop::Inconclusive(format!("only {} APS frames from the bulb for 6 requests", ctrs.len())));
    }
    let ok = ctrs.windows(2).all(|w| w[1].0.wrapping_sub(w[0].0) == 1);
    let lst = format!("[{}]", ctrs.iter().map(|(c, k)| format!("({c}, '{k}')")).collect::<Vec<_>>().join(", "));
    expect(ok, format!("APS counters of consecutive transmissions: {lst}"))?;
    Ok(format!("counters [{}]", ctrs.iter().map(|(c, _)| c.to_string()).collect::<Vec<_>>().join(", ")))
}

fn group_delivery(s: &mut Session) -> R<String> {
    let (g_in, g_out) = (0x4A01u16, 0x4A02u16);
    add_group(s, g_in)?;
    let r = (|| -> R<()> {
        light_on(s)?;
        tc_zcl(s, 0xFFFD, ONOFF, 0x00, &[], Tx::new().group(g_out).ddr());
        s.run(1500.0)?;
        expect(
            is_on(s),
            format!("Off addressed to group {g_out:#06x}, of which no endpoint is a member, was executed"),
        )?;
        tc_zcl(s, 0xFFFD, ONOFF, 0x00, &[], Tx::new().group(g_in).ddr());
        s.run(1500.0)?;
        expect(
            !is_on(s),
            format!("Off addressed to group {g_in:#06x}, of which endpoint 1 is a member, was not executed"),
        )
    })();
    s.zcl(GROUPS, 0x03, &g_in.to_le_bytes(), Tx::new())?;
    light_on(s)?;
    r?;
    Ok(String::new())
}

fn broadcast_endpoint(s: &mut Session) -> R<String> {
    let eps = s.endpoints()?;
    let absent = *[0x63u8, 0x64, 0x65].iter().find(|e| !eps.contains(e)).unwrap();
    light_on(s)?;
    let dut = s.dut_short;
    tc_zcl(s, dut, ONOFF, 0x00, &[], Tx::new().ep(absent).ddr());
    s.run(1500.0)?;
    expect(
        is_on(s),
        format!("Off addressed to endpoint {absent}, which the device does not have, switched the light off"),
    )?;
    tc_zcl(s, dut, ONOFF, 0x00, &[], Tx::new().ep(0xFF).ddr());
    s.run(1500.0)?;
    let off = !is_on(s);
    light_on(s)?;
    expect(off, "Off addressed to the broadcast endpoint 0xff was not executed on endpoint 1")?;
    Ok(String::new())
}

fn binding_unicast(s: &mut Session) -> R<String> {
    let mut pl = s.dut_ext.to_le_bytes().to_vec();
    pl.push(1);
    pl.extend_from_slice(&ONOFF.to_le_bytes());
    pl.push(0x03);
    pl.extend_from_slice(&s.w.coord.ext.to_le_bytes());
    pl.push(0x2A);
    let st = bind(s, &pl, false)?;
    if st != Some(0) {
        return Err(Stop::Inconclusive(format!(
            "Bind_req (On/Off -> Trust Center endpoint 42) was not accepted: status {}",
            opt(st)
        )));
    }
    let r = (|| -> R<String> {
        configure_onoff_reporting(s, 30)?;
        light_on(s)?;
        s.run(500.0)?;
        let i = s.mark();
        let dut = s.dut_short;
        tc_zcl(s, dut, ONOFF, 0x00, &[], Tx::new().ddr());
        s.run(3000.0)?;
        let reps = reports(s, i);
        expect(!reps.is_empty(), "no Report Attributes within 3 s of a change of OnOff (binding and reporting configured)")?;
        let mine: Vec<&CapFrame> = reps.iter().filter(|f| f.aps.as_ref().unwrap().dst_ep == Some(0x2A)).collect();
        expect(
            !mine.is_empty(),
            "no report addressed to the bound endpoint 42: ".to_string()
                + &reps.iter().take(3).map(|f| f.brief()).collect::<Vec<_>>().join("; "),
        )?;
        let f = mine[0];
        let (a, n) = (f.aps.as_ref().unwrap(), f.nwk.as_ref().unwrap());
        expect(
            a.delivery == 0 && n.dst == s.w.coord.short && a.src_ep == Some(1) && a.profile == Some(0x0104),
            format!("report: {} (delivery mode {})", f.brief(), a.delivery),
        )?;
        Ok(format!("report {} to {:#06x} endpoint {}", hex(&f.zcl().unwrap().payload), n.dst, opt(a.dst_ep)))
    })();
    bind(s, &pl, true)?;
    light_on(s)?;
    r
}

fn binding_group(s: &mut Session) -> R<String> {
    let group = 0x4B10u16;
    let mut pl = s.dut_ext.to_le_bytes().to_vec();
    pl.push(1);
    pl.extend_from_slice(&ONOFF.to_le_bytes());
    pl.push(0x01);
    pl.extend_from_slice(&group.to_le_bytes());
    let st = bind(s, &pl, false)?;
    if st != Some(0) {
        return Err(Stop::Inconclusive(format!(
            "Bind_req (On/Off -> group {group:#06x}) was not accepted: status {}",
            opt(st)
        )));
    }
    let r = (|| -> R<String> {
        configure_onoff_reporting(s, 30)?;
        light_on(s)?;
        s.run(500.0)?;
        let i = s.mark();
        let dut = s.dut_short;
        tc_zcl(s, dut, ONOFF, 0x00, &[], Tx::new().ddr());
        s.run(3000.0)?;
        let reps = reports(s, i);
        expect(
            !reps.is_empty(),
            "no Report Attributes within 3 s of a change of OnOff (group binding and reporting configured)",
        )?;
        let mine: Vec<&CapFrame> = reps.iter().filter(|f| f.aps.as_ref().unwrap().delivery == 3).collect();
        expect(
            !mine.is_empty(),
            "no report with group delivery mode: ".to_string()
                + &reps.iter().take(3).map(|f| f.brief()).collect::<Vec<_>>().join("; "),
        )?;
        let f = mine[0];
        let (a, n) = (f.aps.as_ref().unwrap(), f.nwk.as_ref().unwrap());
        expect(
            a.group == Some(group) && a.dst_ep.is_none(),
            format!("group field {}, destination endpoint {}", opt(a.group), opt(a.dst_ep)),
        )?;
        expect(n.dst == 0xFFFD, format!("NWK destination {:#06x}; 0xfffd required", n.dst))?;
        expect(!a.ack_req, "the group-addressed frame requests an APS acknowledgement")?;
        Ok(format!("report {} to group {:#06x}", hex(&f.zcl().unwrap().payload), a.group.unwrap_or(0)))
    })();
    bind(s, &pl, true)?;
    s.zcl(ONOFF, 0x06, &reporting_payload(0xFFFF), Tx::new().general())?;
    light_on(s)?;
    r
}

fn extended_header_unfragmented(s: &mut Session) -> R<String> {
    // ZCL sequence number 0x0b: should a receiver read the frame one octet off, it sees a Default Response
    let zcl = [0x00u8, 0x0B, 0x00, 0x00, 0x00];
    let cnt = nx(&mut s.w.coord.st.aps_ctr);
    let mut aps = vec![0x80, 1];
    aps.extend_from_slice(&ONOFF.to_le_bytes());
    aps.extend_from_slice(&0x0104u16.to_le_bytes());
    aps.extend_from_slice(&[1, cnt, 0x00]);
    aps.extend_from_slice(&zcl);
    let i = s.mark();
    let dut = s.dut_short;
    {
        let w = &mut s.w;
        w.coord.tx_nwk(&mut w.air, dut, &aps, &Tx::new());
    }
    let rsp = s.wait_dut(
        move |f| {
            f.zcl().is_some_and(|z| z.seq == 0x0B && !z.specific && z.cmd == 0x01)
                && f.aps.as_ref().unwrap().cluster == Some(ONOFF)
                && f.nwk.as_ref().unwrap().src == dut
        },
        1500.0,
        Some(i),
    )?;
    s.run(300.0)?;
    let others: Vec<String> = s
        .dut_frames(i, |f| f.aps.as_ref().is_some_and(|a| a.ty == APS_DATA))
        .iter()
        .map(|f| f.brief())
        .collect();
    if s.read(ONOFF, &[0x0000], 1500.0)?.is_none() {
        return Err(Stop::Inconclusive(
            "control failed: the same Read Attributes without extended header is not answered".into(),
        ));
    }
    let others = if others.is_empty() { "nothing".to_string() } else { others.join("; ") };
    expect(
        rsp.is_some(),
        "Read Attributes carried in a frame with an extended header (fragmentation 0b00) was not answered with a \
         Read Attributes Response; the bulb sent: "
            .to_string()
            + &others,
    )?;
    Ok(String::new())
}

#[derive(Clone)]
struct Frag {
    max_buf: u8,
    max_in: u16,
    attrs: Vec<u16>,
    early: Vec<CapFrame>,
    rsp: Option<CapFrame>,
    acks: Vec<CapFrame>,
    desc: String,
}

fn frag_experiment(s: &mut Session) -> R<Frag> {
    if let Some(r) = s.note::<Frag>("aps_frag") {
        return Ok(r);
    }
    let dut = s.dut_short;
    let f = s.zdo(0x0002, &dut.to_le_bytes(), 2000.0)?;
    let p = f.as_ref().map(|f| f.zdo().unwrap().payload).unwrap_or_default();
    if f.is_none() || p.first() != Some(&0) {
        return Err(Stop::Inconclusive("no Node_Desc_rsp".into()));
    }
    let nd = &p[3..16.min(p.len())];
    let mut r = Frag {
        max_buf: nd[5],
        max_in: u16::from_le_bytes([nd[6], nd[7]]),
        attrs: vec![0x0000, 0x4000, 0x4001, 0x4002],
        early: vec![],
        rsp: None,
        acks: vec![],
        desc: String::new(),
    };
    let zseq = nx(&mut s.w.coord.st.zcl_seq);
    // frame control 0x10 (general command, default response disabled)
    let mut zcl = vec![0x10, zseq, 0x00];
    for a in &r.attrs {
        zcl.extend_from_slice(&a.to_le_bytes());
    }
    let blocks = [zcl[..5].to_vec(), zcl[5..].to_vec()];
    let cnt = nx(&mut s.w.coord.st.aps_ctr);
    let i = s.mark();
    {
        let w = &mut s.w;
        w.coord.tx_aps_data(&mut w.air, dut, ONOFF, &blocks[0], &Tx::new().ack_req().counter(cnt).frag((0x01, 2)));
    }
    s.run(400.0)?;
    r.early = acks(s, i, Some(cnt));
    {
        let w = &mut s.w;
        w.coord.tx_aps_data(&mut w.air, dut, ONOFF, &blocks[1], &Tx::new().ack_req().counter(cnt).frag((0x02, 1)));
    }
    r.rsp = s.wait_dut(
        move |f| {
            f.zcl().is_some_and(|z| z.seq == zseq && !z.specific && z.cmd == 0x01)
                && f.aps.as_ref().unwrap().cluster == Some(ONOFF)
                && f.nwk.as_ref().unwrap().src == dut
        },
        2500.0,
        Some(i),
    )?;
    s.run(500.0)?;
    r.acks = acks(s, i, Some(cnt));
    let parts: Vec<String> = r
        .acks
        .iter()
        .map(|a| {
            let a = a.aps.as_ref().unwrap();
            if a.ext_hdr {
                format!(
                    "ack extended frame control {}, block {}, bitfield {}",
                    opt(a.ext_fc),
                    opt(a.block),
                    a.ack_bits.map(|b| format!("{b:#x}")).unwrap_or_else(|| "None".into())
                )
            } else {
                "ack without extended header".into()
            }
        })
        .collect();
    r.desc = if parts.is_empty() { "no acknowledgement".into() } else { parts.join("; ") };
    light_on(s)?;
    s.set_note("aps_frag", r.clone());
    Ok(r)
}

fn frag_supported(r: &Frag) -> R<()> {
    if r.rsp.is_none() && r.max_in <= r.max_buf as u16 {
        return Err(Stop::NotApplicable(format!(
            "the image does not reassemble fragments, and its node descriptor does not claim to \
             (maximum incoming transfer size {} <= maximum buffer size {}); it answered the two blocks with: {}",
            r.max_in, r.max_buf, r.desc
        )));
    }
    Ok(())
}

fn hex_list(v: &[u16]) -> String {
    format!("[{}]", v.iter().map(|a| format!("'{a:#x}'")).collect::<Vec<_>>().join(", "))
}

fn fragmentation_rx(s: &mut Session) -> R<String> {
    let r = frag_experiment(s)?;
    frag_supported(&r)?;
    let desc = &r.desc;
    let Some(rsp) = &r.rsp else {
        return Err(Stop::Fail(format!(
            "no Read Attributes Response to a request sent in two blocks, although the node descriptor gives maximum \
             incoming transfer size {} > maximum buffer size {}; {desc}",
            r.max_in, r.max_buf
        )));
    };
    let got: Vec<u16> =
        read_attr_records(&rsp.zcl().unwrap().payload).map(|v| v.iter().map(|x| x.0).collect()).unwrap_or_default();
    expect(
        got == r.attrs,
        format!(
            "the response covers attributes {}; the two blocks together ask for {}",
            hex_list(&got),
            hex_list(&r.attrs)
        ),
    )?;
    let all_acks = &r.acks;
    expect(!all_acks.is_empty(), "the command was executed but no APS acknowledgement was sent for the fragments")?;
    expect(
        all_acks.iter().all(|a| {
            let a = a.aps.as_ref().unwrap();
            a.ext_hdr && a.frag.unwrap_or(0) != 0
        }),
        format!("an acknowledgement of a fragment lacks the extended header with block number and ack bitfield: {desc}"),
    )?;
    let lastf = all_acks.last().unwrap();
    let last = lastf.aps.as_ref().unwrap();
    expect(
        last.dst_ep == Some(1) && last.src_ep == Some(1) && last.cluster == Some(ONOFF) && last.profile == Some(0x0104),
        format!("acknowledgement addressing: {}", lastf.brief()),
    )?;
    if let Some(e) = r.early.first() {
        // the receive window holds one block: every block is acknowledged on its own
        let e = e.aps.as_ref().unwrap();
        expect(
            e.block == Some(0) && e.ack_bits.unwrap_or(0) & 1 != 0,
            format!("acknowledgement of the first block: {desc}"),
        )?;
        expect(
            all_acks.len() >= 2 && last.block == Some(1) && last.ack_bits.unwrap_or(0) & 1 != 0,
            format!("acknowledgement of the second block (block number 1, bit 0 set): {desc}"),
        )?;
    } else {
        expect(
            last.block == Some(0) && last.ack_bits.unwrap_or(0) & 3 == 3,
            format!(
                "one acknowledgement for both blocks: block number 0 and bits 0 and 1 of the ack bitfield expected: {desc}"
            ),
        )?;
    }
    Ok(desc.clone())
}

fn fragmentation_ack_value(s: &mut Session) -> R<String> {
    let r = frag_experiment(s)?;
    frag_supported(&r)?;
    let all_acks: Vec<&CapFrame> = r.acks.iter().filter(|a| a.aps.as_ref().unwrap().ext_hdr).collect();
    let desc = &r.desc;
    if all_acks.is_empty() {
        return Err(Stop::Inconclusive(format!("no acknowledgement with an extended header: {desc}")));
    }
    let fr = |f: &CapFrame| f.aps.as_ref().unwrap().frag;
    if !r.early.is_empty() && all_acks.len() >= 2 {
        let vals = (fr(all_acks[0]), fr(all_acks[all_acks.len() - 1]));
        expect(
            vals == (Some(1), Some(2)),
            format!(
                "fragmentation values {:#04b} for the first block and {:#04b} for the second; 0b01 and 0b10 required \
                 ({desc})",
                vals.0.unwrap_or(0),
                vals.1.unwrap_or(0)
            ),
        )?;
    } else {
        let v = fr(all_acks[all_acks.len() - 1]);
        expect(v == Some(1) || v == Some(2), format!("fragmentation value {} ({desc})", opt(v)))?;
    }
    Ok(desc.clone())
}
