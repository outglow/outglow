//! NWK layer of a router among other routers (R22 3.4, 3.6.2, 3.6.3, 3.6.5,
//! 3.6.13, 4.3.1). Ported from emulator/spec/nwk_routing.py.
//!
//! Topology: the Trust Center hears the bulb; two scripted routers r2 and r3
//! hear only the bulb, so everything between them has to pass through it. The
//! scripted routers do what the text says a router does (Link Status, Route
//! Reply with the link cost of the next hop) and nothing else.

use super::Test;
use crate::air::NodeId;
use crate::frames::{ZclValue, BCAST_ROUTERS};
use crate::session::{expect, is_nwk_cmd, CapFrame, Session, Stop, R};
use crate::station::{Sent, SrcIeee, Tx};

const S: &str = "nwk-route";
/// an address nobody has
const FAR: u16 = 0x5E01;
const DUT: Option<NodeId> = Some(NodeId::Dut);
/// the payload of a Report Attributes frame used as relayed traffic
const REPORT: [u8; 4] = [0x00, 0x00, 0x10, 0x01];

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
        t!("NWK-LS-01", "R22", "3.6.3.4.1, 3.4.8",
           "A router sends a Link Status command at least every nwkLinkStatusPeriod (15 s) as a one-hop broadcast \
            without retries: destination 0xFFFC, radius 1; a status that fits one frame has the first and last frame \
            bits set, and the entries are sorted by ascending address.", link_status_format),
        t!("NWK-LS-02", "R22", "3.6.3.4.2, 3.4.8.3.2",
           "The outgoing cost a router reports for a neighbour is the incoming cost that neighbour last reported for \
            it; when the neighbour's Link Status does not list the router, the outgoing cost becomes 0.",
           link_status_outgoing),
        t!("NWK-BCAST-01", "R22", "3.6.5, 3.6.2.2",
           "A router retransmits a new broadcast whose radius is still above zero after decrementing it: same source \
            and sequence number, MAC broadcast without acknowledgement request, after a random jitter of at most \
            nwkcMaxBroadcastJitter (0x40 ms).", broadcast_relay),
        t!("NWK-BCAST-02", "R22", "3.6.5, Table 3-58",
           "While neighbours have not relayed a broadcast, the router keeps retransmitting it, \
            nwkMaxBroadcastRetries times in all.", broadcast_retries,
           ambiguous: Some("nwkMaxBroadcastRetries is a NIB attribute (R22 default 3; the Zigbee PRO stack profile, \
            not among the suite's documents, is known to set 2), and 'shall retransmit a previously broadcast frame at \
            most nwkMaxBroadcastRetries times' can count the first relay or not; the test accepts 2 to 4 transmissions \
            when no neighbour relays the frame")),
        t!("NWK-BCAST-03", "R22", "3.6.5, 3.6.2.2",
           "A broadcast received with radius 1 is delivered to the application but not retransmitted; a second copy \
            of a broadcast already in the broadcast transaction table is dropped (neither delivered again nor \
            retransmitted).", broadcast_radius_and_duplicate),
        t!("NWK-RREQ-01", "R22", "3.6.3.5.2",
           "A router that is not the destination of a Route Request rebroadcasts it with the link cost of the \
            previous hop added to the path cost (same identifier, destination and NWK source), after a jitter, and \
            retries the relay nwkcRREQRetries (2) more times.", rreq_relay),
        t!("NWK-RREP-01", "R22", "3.6.3.5.3",
           "A router that is not the originator of a route discovery forwards the Route Reply to the next hop \
            towards the originator, adding the cost of that link to the path cost and keeping identifier, originator \
            and responder.", rrep_forward),
        t!("NWK-ROUTE-01", "R22", "3.6.3.3, 3.6.2.2",
           "With an active routing table entry a router relays a unicast frame to the next hop: MAC unicast with \
            16-bit addresses, NWK sequence number unchanged, radius decremented; a frame whose radius falls to 0 is \
            not relayed.", unicast_relay),
        t!("NWK-ROUTE-02", "R22", "3.6.3.5.3 (last paragraph)",
           "With nwkSymLink = TRUE a router that relays a Route Reply also creates the reverse routing table entry, \
            so frames from the responder to the originator are relayed.", reverse_route,
           mandatory: false,
           ambiguous: Some("nwkSymLink is FALSE by default in R22 Table 3-58 and set to TRUE by the Zigbee PRO stack \
            profile, which is not among the documents the suite reads")),
        t!("NWK-RREQ-02", "R22", "3.6.3.5.2",
           "A router that is the destination of a Route Request answers with a Route Reply unicast to the device the \
            request came from: originator and identifier from the request, itself as responder, path cost = cost of \
            the link to that next hop (1-7, not 0).", rreq_for_self),
        t!("NWK-RREQ-03", "R22", "3.6.3.5.1",
           "A router that starts a route discovery broadcasts the Route Request with path cost 0 and, while no reply \
            comes, repeats it nwkcInitialRREQRetries (3) times at intervals of nwkcRREQRetryInterval (254 ms).",
           rreq_initiation),
        t!("NWK-MTO-01", "R22", "3.6.3.5.2, 3.6.3.5.4, 3.4.5",
           "After a many-to-one Route Request (no route cache) a router relays the request and, before its next \
            unicast data frame to that concentrator, unicasts a Route Record with itself as source and relay count 0.",
           many_to_one),
        t!("NWK-MTO-02", "R22", "3.6.3.5.2",
           "A many-to-one Route Request received from a device that is not in the neighbour table (or whose outgoing \
            cost is 0) is discarded.", many_to_one_stranger),
        t!("NWK-SRC-01", "R22", "3.6.3.3.2",
           "A source-routed frame whose relay index is 0 is relayed directly to the NWK destination; with a relay \
            index above 0 and the router's address at that index, the index is decremented and the frame goes to the \
            relay listed before it.", source_route_relay),
        t!("NWK-SRC-02", "R22", "3.6.3.3 (last paragraph), 3.4.3.2, 3.4.3.3.2",
           "When a relayed frame cannot be delivered to the next relay of its source route, the router sends a \
            Network Status to the source of the frame whose destination address field is the NWK destination of the \
            failed frame.", source_route_failure),
        t!("NWK-FAIL-01", "R22", "3.6.3.3 (last paragraph), 3.4.3",
           "When a frame relayed for another device cannot be delivered (no route, discover route = 0), the router \
            sends a Network Status back to the source of the frame, with the frame's destination in the destination \
            address field.", no_route_status,
           ambiguous: Some("for a frame without a route and with discover route = 0, 3.6.3.3 first says the frame \
            'shall be discarded' (naming only the local confirm) and then, in its last paragraph, that a router \
            failing to deliver a relayed frame 'for any reason' shall send a Network Status to its source; the test \
            follows the last paragraph")),
        t!("NWK-FAIL-02", "R22", "3.6.3.7.1, 3.6.3.3 (last paragraph)",
           "When the next hop of an established route stops acknowledging, the router reports the failure of a \
            relayed frame to its source with a Network Status naming the frame's destination.", link_failure_status,
           ambiguous: Some("3.6.3.7 leaves to the implementation when a link counts as failed; the test sends five \
            frames, none of which is acknowledged by the next hop, and 3.6.3.3 requires a report for a frame that \
            could not be delivered 'for any reason'")),
        t!("NWK-UNK-01", "R22", "3.6.13",
           "An unknown unicast NWK command addressed to the device is answered with a Network Status with status \
            0x13 (unknown command), sent to the source of that frame.", unknown_command),
        t!("NWK-SEC-01", "R22", "3.6.2.2",
           "A joined and authenticated device discards a NWK data frame that is not secured.", unsecured_data),
        t!("NWK-SEC-02", "R22", "4.3.1.2 step 3",
           "A secured NWK frame whose frame counter is not above the last one accepted from that sender is \
            rejected.", replayed_frame_counter),
        t!("NWK-LS-03", "R22", "3.6.3.4.3",
           "When no Link Status has been received from a router neighbour for more than nwkRouterAgeLimit (3) \
            periods, its outgoing cost is set to 0 (the entry is stale).", link_status_ageing),
    ]
}

// ---------------------------------------------------------------- helpers
type Entries = Vec<(u16, (u8, u8))>;

/// Link Status frame -> {address: (incoming cost, outgoing cost)}, in insertion order.
fn ls_entries(f: &CapFrame) -> Entries {
    let p = f.nwk.as_ref().unwrap().payload.as_ref().unwrap();
    let n = (p[1] & 0x1F) as usize;
    let mut out: Entries = Vec::new();
    for k in 0..n {
        let j = 2 + 3 * k;
        let a = u16::from_le_bytes([p[j], p[j + 1]]);
        let st = p[j + 2];
        let v = (st & 7, (st >> 4) & 7);
        match out.iter_mut().find(|e| e.0 == a) {
            Some(e) => e.1 = v,
            None => out.push((a, v)),
        }
    }
    out
}

fn eget(e: &Entries, a: u16) -> Option<(u8, u8)> {
    e.iter().find(|x| x.0 == a).map(|x| x.1)
}

fn dict_repr(e: &Entries) -> String {
    let parts: Vec<String> = e.iter().map(|(a, (i, o))| format!("{a}: ({i}, {o})")).collect();
    format!("{{{}}}", parts.join(", "))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn py_str(s: &str) -> String {
    let q = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut o = String::new();
    o.push(q);
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            c if c == q => {
                o.push('\\');
                o.push(c);
            }
            c => o.push(c),
        }
    }
    o.push(q);
    o
}

fn py_list<T: ToString>(v: &[T]) -> String {
    format!("[{}]", v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(", "))
}

fn py_floats(v: &[f64]) -> String {
    format!("[{}]", v.iter().map(|x| format!("{x:?}")).collect::<Vec<_>>().join(", "))
}

fn round1(x: f64) -> f64 {
    format!("{x:.1}").parse().unwrap()
}

/// Python's round() to an integer (ties to even).
fn round0(x: f64) -> i64 {
    x.round_ties_even() as i64
}

fn int_gaps(v: &[CapFrame]) -> Vec<i64> {
    v.windows(2).map(|w| round0(w[1].t0 - w[0].t0)).collect()
}

/// `try: body finally: fin` — an error of the clean-up replaces the body's result.
fn finally<T>(r: R<T>, fin: R<()>) -> R<T> {
    match fin {
        Err(e) => Err(e),
        Ok(()) => r,
    }
}

fn short(s: &mut Session, p: NodeId) -> u16 {
    s.peer(p).st.short.unwrap()
}

fn nwkp(f: &CapFrame) -> &[u8] {
    f.nwk.as_ref().unwrap().payload.as_deref().unwrap()
}

fn next_ls(s: &mut Session, max_ms: f64, since: Option<usize>) -> R<Option<CapFrame>> {
    s.wait(is_nwk_cmd(0x08), max_ms, since, DUT, 5.0)
}

/// r2 and r3 as router neighbours of the bulb (both listed in its Link Status
/// with a non-zero outgoing cost), or INCONCLUSIVE.
fn net(s: &mut Session) -> R<(NodeId, NodeId)> {
    if s.has_note("net") {
        return match s.note::<Option<(NodeId, NodeId)>>("net").unwrap() {
            None => Err(Stop::Inconclusive("the scripted routers never became neighbours of the bulb".into())),
            Some(p) => Ok(p),
        };
    }
    let r2 = s.router("r2", 0x00124B00BB000002, 0x2B02, Some(&["dut"]), true);
    let r3 = s.router("r3", 0x00124B00BB000003, 0x3C03, Some(&["dut"]), true);
    s.set_note("net", None::<(NodeId, NodeId)>);
    let (a2, a3) = (short(s, r2), short(s, r3));
    let end = s.now() + 50000.0;
    while s.now() < end {
        let Some(f) = next_ls(s, 18000.0, None)? else { break };
        let e = ls_entries(&f);
        if [a2, a3].iter().all(|a| eget(&e, *a).is_some_and(|x| x.1 != 0)) {
            s.set_note("net", Some((r2, r3)));
            return Ok((r2, r3));
        }
    }
    Err(Stop::Inconclusive(
        "the scripted routers did not appear in the bulb's Link Status with an outgoing cost within 50 s of sending \
         their own Link Status"
            .into(),
    ))
}

fn rreq(s: &mut Session, peer: NodeId, target: u16, rid: u8, cost: u8, opts: u8, radius: u8) -> Sent {
    let mut p = vec![0x01, opts, rid];
    p.extend_from_slice(&target.to_le_bytes());
    p.push(cost);
    let (st, air) = s.w.st(peer);
    st.tx_nwk_cmd(air, BCAST_ROUTERS, &p, &Tx::new().radius(radius))
}

fn is_rreq(rid: Option<u8>, target: Option<u16>, src: Option<u16>) -> impl Fn(&CapFrame) -> bool {
    move |f| {
        if f.nwk_cmd() != Some(0x01) {
            return false;
        }
        let p = nwkp(f);
        rid.map_or(true, |r| p[2] == r)
            && target.map_or(true, |t| u16::from_le_bytes([p[3], p[4]]) == t)
            && src.map_or(true, |x| f.nwk.as_ref().unwrap().src == x)
    }
}

fn onoff(s: &mut Session) -> R<ZclValue> {
    match s.value(0x0006, 0x0000)? {
        Some(v) => Ok(v),
        None => Err(Stop::Inconclusive("the OnOff attribute could not be read".into())),
    }
}

/// A peer's ZCL frame.
fn ptx_zcl(s: &mut Session, p: NodeId, dst: u16, cluster: u16, cmd: u8, payload: &[u8], o: Tx) -> Sent {
    let (st, air) = s.w.st(p);
    st.tx_zcl(air, dst, cluster, cmd, payload, &o)
}

/// The relayed test traffic: a Report Attributes (server to client), no Default Response.
fn report() -> Tx {
    Tx::new().general().ddr().to_client()
}

fn nwk_is(f: &CapFrame, src: u16, dst: u16, seq: u8) -> bool {
    f.nwk.as_ref().is_some_and(|n| n.src == src && n.dst == dst && n.seq == seq)
}

// ---------------------------------------------------------------- Link Status
fn link_status_format(s: &mut Session) -> R<String> {
    net(s)?;
    let i = s.mark();
    s.run(47000.0)?;
    let ls = s.frames(i, DUT, is_nwk_cmd(0x08));
    expect(ls.len() >= 3, format!("{} Link Status frames in 47 s", ls.len()))?;
    let gaps: Vec<f64> = ls.windows(2).map(|w| w[1].t0 - w[0].t0).collect();
    let gs: Vec<f64> = gaps.iter().map(|g| round1(g / 1000.0)).collect();
    expect(
        gaps.iter().cloned().fold(f64::MIN, f64::max) <= 16500.0,
        format!("gaps between Link Status frames {} s; the period is 15 s (jitter allowed)", py_floats(&gs)),
    )?;
    let seqs: Vec<u8> = ls.iter().map(|f| f.nwk.as_ref().unwrap().seq).collect();
    let mut uniq = seqs.clone();
    uniq.sort();
    uniq.dedup();
    expect(uniq.len() == seqs.len(), "a Link Status frame was transmitted twice (the text says without retries)")?;
    for f in &ls {
        let n = f.nwk.as_ref().unwrap();
        let m = f.mac.as_ref().unwrap();
        expect(
            n.dst == 0xFFFC && n.radius == 1 && m.dst == Some(0xFFFF),
            format!("destination {:#06x}, radius {}, MAC destination {:#06x}", n.dst, n.radius, m.dst.unwrap_or(0)),
        )?;
        let p = n.payload.as_ref().unwrap();
        let o = p[1];
        let cnt = (o & 0x1F) as usize;
        expect(
            p.len() == 2 + 3 * cnt,
            format!("entry count {cnt}, {} octets of list", p.len() as i64 - 2),
        )?;
        expect(
            o & 0x60 == 0x60,
            format!("command options {o:#04x}: first frame and last frame must be 1 for a single frame"),
        )?;
        let addrs: Vec<u16> = (0..cnt).map(|k| u16::from_le_bytes([p[2 + 3 * k], p[3 + 3 * k]])).collect();
        let mut sorted = addrs.clone();
        sorted.sort();
        let hexes: Vec<String> = addrs.iter().map(|a| py_str(&format!("{a:#x}"))).collect();
        expect(addrs == sorted, format!("entries not in ascending order: {}", py_list(&hexes)))?;
        for (a, (inc, _)) in ls_entries(f) {
            expect((1..=7).contains(&inc), format!("incoming cost {inc} for {a:#06x}; the range is 1-7"))?;
        }
    }
    Ok(format!("{} frames, gaps {} s", ls.len(), py_floats(&gs)))
}

fn link_status_outgoing(s: &mut Session) -> R<String> {
    let (r2, _r3) = net(s)?;
    let a2 = short(s, r2);
    let dut = s.dut_short;
    s.peer(r2).stop_link_status();
    let r = (|| -> R<String> {
        let _i = s.mark();
        let (st, air) = s.w.st(r2);
        st.tx_link_status(air, &[(dut, 3, 1)], &Tx::new());
        s.run(200.0)?;
        let f = next_ls(s, 18000.0, None)?;
        expect(f.is_some(), "no Link Status from the bulb within 18 s")?;
        let e = ls_entries(f.as_ref().unwrap());
        expect(eget(&e, a2).is_some(), format!("the neighbour is not listed: {}", dict_repr(&e)))?;
        let out = eget(&e, a2).unwrap().1;
        expect(
            out == 3,
            format!("outgoing cost {out} reported for the neighbour that reported incoming cost 3 for the bulb"),
        )?;
        let mut f = None;
        for _ in 0..2 {
            let (st, air) = s.w.st(r2);
            st.tx_link_status(air, &[(0x0000, 1, 1)], &Tx::new());
            s.run(200.0)?;
            f = next_ls(s, 18000.0, None)?;
        }
        let e = f.as_ref().map(ls_entries).unwrap_or_default();
        expect(
            f.is_some() && eget(&e, a2).map_or(true, |x| x.1 == 0),
            format!(
                "outgoing cost {} still reported after two Link Status frames of the neighbour that do not list the \
                 bulb",
                eget(&e, a2).unwrap_or((0, 0)).1
            ),
        )?;
        Ok(String::new())
    })();
    let fin = (|| -> R<()> {
        s.start_link_status(r2, 15000.0, None);
        s.run(300.0)?;
        for _ in 0..3 {
            let f = next_ls(s, 18000.0, None)?;
            if f.is_some_and(|f| eget(&ls_entries(&f), a2).unwrap_or((0, 0)).1 != 0) {
                break;
            }
        }
        Ok(())
    })();
    finally(r, fin)
}

// ---------------------------------------------------------------- broadcasts
fn broadcast_relay(s: &mut Session) -> R<String> {
    net(s)?;
    let i = s.mark();
    let r = s.tx_zcl(0x0003, 0x01, &[], Tx::new().dst(0xFFFD).ddr().radius(5));
    s.run(300.0)?;
    // (a copy that began while the bulb's radio was lent to BLE goes again: `World::ble_retry`)
    let copies = s.frames(i, Some(NodeId::Coord), |f| f.nwk.as_ref().is_some_and(|n| n.seq == r.nwk_seq && n.dst == 0xFFFD));
    if copies.is_empty() {
        return Err(Stop::Error("StopIteration: the Trust Center's broadcast is not on the air".into()));
    }
    let rel = s.frames(i, DUT, |f| nwk_is(f, 0, 0xFFFD, r.nwk_seq));
    // the copy the bulb passed on: the last one before its retransmission
    let orig = match rel.first() {
        Some(f) => copies.iter().filter(|c| c.t1 <= f.t0).last().unwrap_or(&copies[0]).clone(),
        None => copies[0].clone(),
    };
    expect(!rel.is_empty(), "the broadcast was not retransmitted")?;
    let f = &rel[0];
    let n = f.nwk.as_ref().unwrap();
    let m = f.mac.as_ref().unwrap();
    expect(n.radius == 4, format!("radius {} in the retransmission of a frame received with radius 5", n.radius))?;
    expect(m.dst == Some(0xFFFF) && !m.ar, "not a MAC broadcast without acknowledgement request")?;
    let dt = f.t0 - orig.t1;
    expect(dt <= 75.0, format!("first retransmission {dt:.1} ms after reception; the jitter bound is 64 ms"))?;
    Ok(format!("first retransmission after {dt:.1} ms"))
}

fn broadcast_retries(s: &mut Session) -> R<String> {
    net(s)?;
    let i = s.mark();
    let r = s.tx_zcl(0x0003, 0x01, &[], Tx::new().dst(0xFFFD).ddr().radius(5));
    s.run(4000.0)?;
    let rel = s.frames(i, DUT, |f| nwk_is(f, 0, 0xFFFD, r.nwk_seq));
    let gaps = int_gaps(&rel);
    expect(
        (2..=4).contains(&rel.len()),
        format!(
            "{} transmission(s) (gaps {} ms) of a broadcast that the two router neighbours never relayed",
            rel.len(),
            py_list(&gaps)
        ),
    )?;
    Ok(format!("{} transmissions, gaps {} ms", rel.len(), py_list(&gaps)))
}

fn broadcast_radius_and_duplicate(s: &mut Session) -> R<String> {
    net(s)?;
    let v0 = onoff(s)?;
    let i = s.mark();
    let r = s.tx_zcl(0x0006, 0x02, &[], Tx::new().dst(BCAST_ROUTERS).ddr().radius(1));
    s.run(1500.0)?;
    let v1 = onoff(s)?;
    let rel = s.frames(i, DUT, |f| nwk_is(f, 0, BCAST_ROUTERS, r.nwk_seq));
    expect(v1 != v0, "a Toggle broadcast to 0xFFFC with radius 1 was not delivered to the application")?;
    expect(rel.is_empty(), format!("a broadcast received with radius 1 was retransmitted {} time(s)", rel.len()))?;
    // the same broadcast again (same NWK source and sequence number): 0.3 s and 2.5 s after the first copy,
    // both far inside nwkNetworkBroadcastDeliveryTime, during which the transaction record is kept
    for gap in [300u32, 2500] {
        let v1 = onoff(s)?;
        let i = s.mark();
        let r = s.tx_zcl(0x0006, 0x02, &[], Tx::new().dst(0xFFFD).ddr().radius(3));
        s.run(gap as f64)?;
        let n1 = s.frames(i, DUT, |f| f.nwk.as_ref().is_some_and(|n| n.src == 0 && n.seq == r.nwk_seq)).len();
        let j = s.mark();
        let ctr = r.counter.unwrap().wrapping_add(9);
        s.tx_zcl(0x0006, 0x02, &[], Tx::new().dst(0xFFFD).ddr().radius(3).nwk_seq(r.nwk_seq).counter(ctr));
        s.run(3000.0)?;
        let v2 = onoff(s)?;
        let t1j = s.cap[j].t1;
        let rel = s.frames(j, DUT, |f| f.nwk.as_ref().is_some_and(|n| n.src == 0 && n.seq == r.nwk_seq) && f.t0 > t1j);
        expect(
            v2 != v1,
            format!(
                "a second copy of a broadcast ({gap} ms after the first, same source and sequence number, different \
                 APS counter) was delivered to the application again (two Toggles executed)"
            ),
        )?;
        if n1 > 0 && gap >= 2500 {
            expect(
                rel.is_empty(),
                format!("a second copy of a broadcast ({gap} ms after the first) was retransmitted again"),
            )?;
        }
    }
    Ok(String::new())
}

// ---------------------------------------------------------------- route discovery
fn rreq_relay(s: &mut Session) -> R<String> {
    let (r2, r3) = net(s)?;
    let (a2, a3) = (short(s, r2), short(s, r3));
    let i = s.mark();
    let rid = 0x51;
    let sent = rreq(s, r2, a3, rid, 0, 0, 30);
    s.run(1500.0)?;
    let Some(mine) = s.frames(i, Some(r2), |f| f.nwk_cmd() == Some(0x01)).into_iter().next() else {
        return Err(Stop::Error("StopIteration: r2's Route Request is not on the air".into()));
    };
    let rel = s.frames(i, DUT, is_rreq(Some(rid), Some(a3), Some(a2)));
    expect(!rel.is_empty(), "the Route Request was not rebroadcast")?;
    let f = &rel[0];
    let n = f.nwk.as_ref().unwrap();
    let cost = nwkp(f)[5];
    expect((1..=7).contains(&cost), format!("path cost {cost} after one hop from 0; a link cost is between 1 and 7"))?;
    expect(
        n.dst == 0xFFFC && n.radius == 29 && n.seq == sent.nwk_seq,
        format!(
            "header of the relayed request: destination {:#06x}, radius {}, sequence number {} (sent {})",
            n.dst, n.radius, n.seq, sent.nwk_seq
        ),
    )?;
    let dt = f.t0 - mine.t1;
    expect(dt <= 270.0, format!("first relay {dt:.0} ms after reception; the jitter is at most 2 x 128 ms"))?;
    expect(
        rel.len() == 3,
        format!(
            "{} transmissions of the relayed request (gaps {} ms); the original relay plus nwkcRREQRetries = 3",
            rel.len(),
            py_list(&int_gaps(&rel))
        ),
    )?;
    Ok(format!("path cost {cost}, first relay after {dt:.0} ms, {} transmissions", rel.len()))
}

fn rrep_forward(s: &mut Session) -> R<String> {
    let (r2, r3) = net(s)?;
    let (a2, a3) = (short(s, r2), short(s, r3));
    let rid = 0x52;
    rreq(s, r2, a3, rid, 0, 0, 30);
    let f = s.wait(is_rreq(Some(rid), Some(a3), Some(a2)), 600.0, None, DUT, 5.0)?;
    if f.is_none() {
        return Err(Stop::Inconclusive(
            "the bulb did not relay the Route Request, so there is no discovery to reply to".into(),
        ));
    }
    let i = s.mark();
    // the destination's reply (3.6.3.5.2): to the device the request came from, path cost = that link's cost
    let mut p = vec![0x02, 0x00, rid];
    p.extend_from_slice(&a2.to_le_bytes());
    p.extend_from_slice(&a3.to_le_bytes());
    p.push(1);
    let (dut, ext) = (s.dut_short, s.dut_ext);
    let (st, air) = s.w.st(r3);
    st.tx_nwk_cmd(air, dut, &p, &Tx::new().dst_ieee(ext));
    let g = s.wait(
        |f| f.nwk_cmd() == Some(0x02) && f.mac.as_ref().unwrap().dst == Some(a2 as u64),
        1000.0,
        Some(i),
        DUT,
        5.0,
    )?;
    expect(g.is_some(), "the Route Reply was not forwarded to the originator")?;
    let g = g.unwrap();
    let p = nwkp(&g);
    let (rid2, orig, resp, cost) = (p[2], u16::from_le_bytes([p[3], p[4]]), u16::from_le_bytes([p[5], p[6]]), p[7]);
    expect(
        (rid2, orig, resp) == (rid, a2, a3),
        format!("forwarded reply: identifier {rid2:#04x}, originator {orig:#06x}, responder {resp:#06x}"),
    )?;
    let nd = g.nwk.as_ref().unwrap().dst;
    expect(nd == a2, format!("NWK destination {nd:#06x}; it must be the next hop"))?;
    expect((2..=8).contains(&cost), format!("path cost {cost} after adding one link cost (1-7) to the received 1"))?;
    s.set_note("route", true);
    Ok(format!("path cost 1 -> {cost}"))
}

fn need_route(s: &Session) -> R<()> {
    if !s.note::<bool>("route").unwrap_or(false) {
        return Err(Stop::Inconclusive(
            "no route through the bulb was established by the route discovery tests before this one".into(),
        ));
    }
    Ok(())
}

fn unicast_relay(s: &mut Session) -> R<String> {
    let (r2, r3) = net(s)?;
    let (a2, a3) = (short(s, r2), short(s, r3));
    need_route(s)?;
    let dut = s.dut_short;
    let i = s.mark();
    let r = ptx_zcl(s, r2, a3, 0x0006, 0x0A, &REPORT, report().mac_dst(dut).radius(7));
    let f = s.wait(|f| nwk_is(f, a2, a3, r.nwk_seq), 800.0, Some(i), DUT, 5.0)?;
    expect(f.is_some(), "a frame for a destination with an active route was not relayed")?;
    let f = f.unwrap();
    let m = f.mac.as_ref().unwrap();
    let n = f.nwk.as_ref().unwrap();
    expect(
        m.dst == Some(a3 as u64) && m.src == Some(dut as u64) && m.dmode == 2 && m.smode == 2,
        format!("MAC addressing of the relayed frame: {:#x} -> {:#x}", m.src.unwrap_or(0), m.dst.unwrap_or(0)),
    )?;
    expect(n.radius == 6, format!("radius {} after one hop from 7", n.radius))?;
    expect(n.sec_src == Some(s.dut_ext), "the relayed frame is not secured by the relaying router")?;
    let i = s.mark();
    let r = ptx_zcl(s, r2, a3, 0x0006, 0x0A, &REPORT, report().mac_dst(dut).radius(1));
    s.run(600.0)?;
    let again = s.frames(i, DUT, |f| f.nwk.as_ref().is_some_and(|n| n.src == a2 && n.seq == r.nwk_seq && n.ty == 0));
    expect(again.is_empty(), "a frame received with radius 1 was relayed (the radius falls to 0)")?;
    Ok(String::new())
}

fn reverse_route(s: &mut Session) -> R<String> {
    let (r2, r3) = net(s)?;
    let (a2, a3) = (short(s, r2), short(s, r3));
    need_route(s)?;
    let dut = s.dut_short;
    let i = s.mark();
    let r = ptx_zcl(s, r3, a2, 0x0006, 0x0A, &REPORT, report().mac_dst(dut).radius(7));
    let f = s.wait(
        |f| nwk_is(f, a3, a2, r.nwk_seq) && f.nwk.as_ref().unwrap().ty == 0,
        800.0,
        Some(i),
        DUT,
        5.0,
    )?;
    expect(
        f.is_some_and(|f| f.mac.as_ref().unwrap().dst == Some(a2 as u64)),
        "a frame from the responder back to the originator of the discovery was not relayed",
    )?;
    Ok(String::new())
}

fn rreq_for_self(s: &mut Session) -> R<String> {
    let (_r2, r3) = net(s)?;
    let a3 = short(s, r3);
    let dut = s.dut_short;
    let i = s.mark();
    let rid = 0x53;
    rreq(s, r3, dut, rid, 0, 0, 30);
    let g = s.wait(|f| f.nwk_cmd() == Some(0x02) && nwkp(f)[2] == rid, 1500.0, Some(i), DUT, 5.0)?;
    expect(g.is_some(), "no Route Reply to a Route Request for the bulb's own address")?;
    let g = g.unwrap();
    let p = nwkp(&g);
    let (orig, resp, cost) = (u16::from_le_bytes([p[3], p[4]]), u16::from_le_bytes([p[5], p[6]]), p[7]);
    let md = g.mac.as_ref().unwrap().dst;
    expect(
        md == Some(a3 as u64) && g.nwk.as_ref().unwrap().dst == a3,
        format!("reply sent to {:#06x}", md.unwrap_or(0)),
    )?;
    expect((orig, resp) == (a3, dut), format!("originator {orig:#06x}, responder {resp:#06x}"))?;
    expect(
        (1..=7).contains(&cost),
        format!(
            "path cost {cost} in the reply of the destination; it must be the cost of the link to the next hop, \
             which is between 1 and 7"
        ),
    )?;
    Ok(format!("path cost {cost}"))
}

fn rreq_initiation(s: &mut Session) -> R<String> {
    net(s)?;
    let dut = s.dut_short;
    let i = s.mark();
    // a request that seems to come from a distant device: the answer needs a route to it
    s.tx_zcl(0x0000, 0x00, &[0x00, 0x00], Tx::new().general().src(FAR).src_ieee(SrcIeee::Is(0x00124B00FA000001)));
    s.run(2500.0)?;
    let rq = s.frames(i, DUT, is_rreq(None, Some(FAR), Some(dut)));
    if rq.is_empty() {
        let sent: Vec<String> = s
            .frames(i, DUT, |f| f.nwk.as_ref().is_some_and(|n| n.dst == FAR))
            .iter()
            .map(|f| py_str(&f.brief()))
            .collect();
        return Err(Stop::Inconclusive(format!(
            "the bulb started no route discovery for the unknown source of a request{}",
            if sent.is_empty() {
                " and sent no answer".to_string()
            } else {
                format!("; it sent {}", py_list(&sent[..sent.len().min(2)]))
            }
        )));
    }
    let rid = nwkp(&rq[0])[2];
    let same: Vec<CapFrame> = rq.iter().filter(|f| nwkp(f)[2] == rid).cloned().collect();
    let gaps = int_gaps(&same);
    expect(nwkp(&rq[0])[5] == 0, format!("path cost {} in the initial Route Request", nwkp(&rq[0])[5]))?;
    let d0 = rq[0].nwk.as_ref().unwrap().dst;
    expect(d0 == 0xFFFC, format!("destination {d0:#06x}"))?;
    expect(
        same.len() == 4,
        format!(
            "{} transmissions of the request (gaps {} ms); the initial broadcast plus nwkcInitialRREQRetries = 4",
            same.len(),
            py_list(&gaps)
        ),
    )?;
    expect(
        gaps.iter().all(|g| (200..=330).contains(g)),
        format!("gaps between the transmissions {} ms; the interval is 254 ms", py_list(&gaps)),
    )?;
    Ok(format!("4 transmissions, gaps {} ms", py_list(&gaps)))
}

// ---------------------------------------------------------------- many-to-one, source routes
fn mto(s: &mut Session) {
    let w = &mut s.w;
    w.coord.many_to_one_route_request(&mut w.air, true);
}

fn many_to_one(s: &mut Session) -> R<String> {
    net(s)?;
    let i = s.mark();
    mto(s);
    s.run(1200.0)?;
    let rel: Vec<CapFrame> = s
        .frames(i, DUT, is_nwk_cmd(0x01))
        .into_iter()
        .filter(|f| f.nwk.as_ref().unwrap().src == 0 && (nwkp(f)[1] >> 3) & 3 == 2)
        .collect();
    expect(!rel.is_empty(), "the many-to-one Route Request was not rebroadcast")?;
    let c = nwkp(&rel[0])[5];
    expect((1..=7).contains(&c), format!("path cost {c} in the relayed request"))?;
    let i = s.mark();
    let f = s.zcl(0x0000, 0x00, &[0x00, 0x00], Tx::new().general())?;
    expect(f.is_some(), "no answer to a Read Attributes after the many-to-one Route Request")?;
    let fi = f.unwrap().i;
    let rr: Vec<CapFrame> = s.frames(i, DUT, is_nwk_cmd(0x05)).into_iter().filter(|g| g.i < fi).collect();
    expect(!rr.is_empty(), "no Route Record before the first unicast frame to the concentrator")?;
    let g = &rr[0];
    let n = g.nwk.as_ref().unwrap();
    let rc = nwkp(g)[1];
    expect(
        n.src == s.dut_short && n.dst == 0 && rc == 0,
        format!("Route Record: source {:#06x}, destination {:#06x}, relay count {rc}", n.src, n.dst),
    )?;
    expect(n.src_ieee == Some(s.dut_ext), "the Route Record has no source IEEE address in the NWK header")?;
    Ok(String::new())
}

fn many_to_one_stranger(s: &mut Session) -> R<String> {
    net(s)?;
    let x = s.add_peer("stranger", 0x00124B00BB0000EE, Some(0x4E0E), Some(&["dut"]));
    s.w.air.keys.learn(Some(0x4E0E), Some(0x00124B00BB0000EE));
    let i = s.mark();
    let (st, air) = s.w.st(x);
    st.tx_nwk_cmd(air, BCAST_ROUTERS, &[0x01, 0x10, 0x61, 0xFC, 0xFF, 0], &Tx::new().radius(30));
    s.run(1000.0)?;
    let rel: Vec<CapFrame> =
        s.frames(i, DUT, is_nwk_cmd(0x01)).into_iter().filter(|f| f.nwk.as_ref().unwrap().src == 0x4E0E).collect();
    s.remove_peer(x);
    let i2 = s.mark();
    mto(s);
    s.run(1000.0)?;
    let ctl: Vec<CapFrame> =
        s.frames(i2, DUT, is_nwk_cmd(0x01)).into_iter().filter(|f| f.nwk.as_ref().unwrap().src == 0).collect();
    if ctl.is_empty() {
        return Err(Stop::Inconclusive(
            "control failed: a many-to-one Route Request from a known neighbour was not relayed either".into(),
        ));
    }
    expect(
        rel.is_empty(),
        "a many-to-one Route Request from a device that never sent a Link Status (no neighbour entry, no outgoing \
         cost) was accepted and rebroadcast",
    )?;
    Ok(String::new())
}

fn py_opt<T: ToString>(v: Option<T>) -> String {
    v.map(|x| x.to_string()).unwrap_or_else(|| "None".into())
}

fn source_route_relay(s: &mut Session) -> R<String> {
    let (r2, r3) = net(s)?;
    let (a2, a3) = (short(s, r2), short(s, r3));
    let dut = s.dut_short;
    let i = s.mark();
    let r = s.tx_zcl(0x0006, 0x0A, &REPORT, report().dst(a2).relays(vec![dut]).relay_index(0).mac_dst(dut).radius(9));
    let f = s.wait(|f| nwk_is(f, 0, a2, r.nwk_seq), 800.0, Some(i), DUT, 5.0)?;
    expect(f.is_some(), "a source-routed frame with relay index 0 was not relayed to its destination")?;
    let f = f.unwrap();
    let (md, rad) = (f.mac.as_ref().unwrap().dst, f.nwk.as_ref().unwrap().radius);
    expect(md == Some(a2 as u64) && rad == 8, format!("relayed to {:#06x} with radius {rad}", md.unwrap_or(0)))?;
    let i = s.mark();
    let r = s.tx_zcl(
        0x0006,
        0x0A,
        &REPORT,
        report().dst(FAR).relays(vec![a3, dut]).relay_index(1).mac_dst(dut).radius(9),
    );
    let f = s.wait(|f| nwk_is(f, 0, FAR, r.nwk_seq), 800.0, Some(i), DUT, 5.0)?;
    expect(f.is_some(), "a source-routed frame with the bulb at relay index 1 was not relayed")?;
    let f = f.unwrap();
    let md = f.mac.as_ref().unwrap().dst;
    expect(
        md == Some(a3 as u64),
        format!("relayed to {:#06x}; the relay before the bulb in the list is {a3:#06x}", md.unwrap_or(0)),
    )?;
    let n = f.nwk.as_ref().unwrap();
    expect(
        n.relay_index == Some(0) && n.relays.as_deref() == Some(&[a3, dut][..]),
        format!(
            "source route subframe after relaying: index {}, list {}",
            py_opt(n.relay_index),
            n.relays.as_ref().map(|r| py_list(r)).unwrap_or_else(|| "None".into())
        ),
    )?;
    Ok(String::new())
}

fn net_status_to(f: &CapFrame, dst: u16) -> bool {
    f.nwk_cmd() == Some(0x03) && f.nwk.as_ref().unwrap().dst == dst
}

fn source_route_failure(s: &mut Session) -> R<String> {
    let (_r2, r3) = net(s)?;
    let a3 = short(s, r3);
    let dut = s.dut_short;
    s.peer(r3).auto_ack = false;
    let r = (|| -> R<Option<CapFrame>> {
        let i = s.mark();
        let mut f = None;
        for _ in 0..5 {
            // several frames: 3.6.3.7 lets the router decide when a link has failed
            s.tx_zcl(
                0x0006,
                0x0A,
                &REPORT,
                report().dst(FAR).relays(vec![a3, dut]).relay_index(1).mac_dst(dut).radius(9),
            );
            f = s.wait(|f| net_status_to(f, 0), 3500.0, Some(i), DUT, 5.0)?;
            if f.is_some() {
                break;
            }
        }
        Ok(f)
    })();
    s.peer(r3).auto_ack = true;
    let f = r?;
    expect(
        f.is_some(),
        "no Network Status to the source after the next relay of a source route did not acknowledge five frames in \
         a row",
    )?;
    let f = f.unwrap();
    let p = nwkp(&f);
    expect(p.len() >= 4, format!("Network Status payload {} has no destination address field", hex(p)))?;
    let target = u16::from_le_bytes([p[2], p[3]]);
    expect(
        target == FAR,
        format!(
            "destination address field {target:#06x} (status {:#04x}); the failed frame was for {FAR:#06x}, the \
             relay that did not answer is {a3:#06x}",
            p[1]
        ),
    )?;
    expect(p[1] == 0x0B, format!("status code {:#04x}; source route failure is 0x0b", p[1]))?;
    Ok(String::new())
}

fn no_route_status(s: &mut Session) -> R<String> {
    let (r2, _r3) = net(s)?;
    let a2 = short(s, r2);
    let dut = s.dut_short;
    let i = s.mark();
    let mut f = None;
    for _ in 0..3 {
        ptx_zcl(s, r2, 0x4D4D, 0x0006, 0x0A, &REPORT, report().mac_dst(dut).discover(0));
        f = s.wait(|f| net_status_to(f, a2), 4500.0, Some(i), DUT, 5.0)?;
        if f.is_some() {
            break;
        }
    }
    expect(f.is_some(), "no Network Status to the source of three frames for which the bulb has no route (13 s)")?;
    let f = f.unwrap();
    let p = nwkp(&f);
    expect(
        p.len() >= 4 && u16::from_le_bytes([p[2], p[3]]) == 0x4D4D,
        format!("Network Status payload {}; the destination address field must be 0x4d4d", hex(p)),
    )?;
    expect(p[1] <= 0x13, format!("status code {:#04x} is reserved", p[1]))?;
    Ok(format!("status code {:#04x}", p[1]))
}

fn link_failure_status(s: &mut Session) -> R<String> {
    let (r2, r3) = net(s)?;
    let (a2, a3) = (short(s, r2), short(s, r3));
    need_route(s)?;
    let dut = s.dut_short;
    s.peer(r3).auto_ack = false;
    s.peer(r3).stop_link_status();
    let r = (|| -> R<(Option<CapFrame>, Vec<CapFrame>)> {
        let i = s.mark();
        let mut f = None;
        for _ in 0..5 {
            // several frames, so that the link can be classified as failed (3.6.3.7)
            ptx_zcl(s, r2, a3, 0x0006, 0x0A, &REPORT, report().mac_dst(dut).discover(0));
            f = s.wait(|f| net_status_to(f, a2), 4000.0, Some(i), DUT, 5.0)?;
            if f.is_some() {
                break;
            }
        }
        let tried = s.frames(i, DUT, |g| g.nwk.as_ref().is_some_and(|n| n.src == a2 && n.dst == a3));
        Ok((f, tried))
    })();
    s.peer(r3).auto_ack = true;
    s.start_link_status(r3, 15000.0, None);
    let (f, tried) = r?;
    if tried.is_empty() {
        return Err(Stop::Inconclusive(
            "the bulb did not try to relay the frame, so no link failure was produced".into(),
        ));
    }
    expect(
        f.is_some(),
        format!(
            "five frames for a next hop that no longer acknowledges: the bulb transmitted them {} time(s) in all and \
             sent no Network Status to their source within 20 s",
            tried.len()
        ),
    )?;
    let f = f.unwrap();
    let p = nwkp(&f);
    expect(
        p.len() >= 4 && u16::from_le_bytes([p[2], p[3]]) == a3,
        format!("Network Status payload {}; the destination address field must be {a3:#06x}", hex(p)),
    )?;
    Ok(format!("status code {:#04x}", p[1]))
}

// ---------------------------------------------------------------- unknown command, security
fn unknown_command(s: &mut Session) -> R<String> {
    let (r2, _r3) = net(s)?;
    let a2 = short(s, r2);
    let dut = s.dut_short;
    let i = s.mark();
    let (st, air) = s.w.st(r2);
    st.tx_nwk_cmd(air, dut, &[0x3F, 0x01, 0x02], &Tx::new());
    let f = s.wait(|f| net_status_to(f, a2), 1500.0, Some(i), DUT, 5.0)?;
    expect(f.is_some(), "no Network Status in answer to NWK command 0x3f (R21 and earlier do not send one)")?;
    let c = nwkp(f.as_ref().unwrap())[1];
    expect(c == 0x13, format!("status code {c:#04x}; unknown command is 0x13"))?;
    Ok(String::new())
}

fn unsecured_data(s: &mut Session) -> R<String> {
    let v0 = onoff(s)?;
    s.tx_zcl(0x0006, 0x02, &[], Tx::new().ddr().insecure());
    s.run(600.0)?;
    let v1 = onoff(s)?;
    s.tx_zcl(0x0006, 0x02, &[], Tx::new().ddr());
    s.run(600.0)?;
    let v2 = onoff(s)?;
    if v2 == v1 {
        return Err(Stop::Inconclusive("control failed: a secured Toggle was not executed either".into()));
    }
    if v1 != v0 {
        let x = s.add_peer("nokey", 0x00124B00BB0000ED, Some(0x4E0D), Some(&["dut"]));
        let dut = s.dut_short;
        ptx_zcl(s, x, dut, 0x0006, 0x02, &[], Tx::new().ddr().insecure());
        s.run(600.0)?;
        let v3 = onoff(s)?;
        s.remove_peer(x);
        return Err(Stop::Fail(format!(
            "a Toggle in an unsecured NWK data frame from the coordinator's address was executed{}",
            if v3 != v2 {
                "; so was one from a device that is not on the network at all"
            } else {
                "; one from an address that is not on the network was not"
            }
        )));
    }
    Ok(String::new())
}

fn replayed_frame_counter(s: &mut Session) -> R<String> {
    let v0 = onoff(s)?;
    let fc = s.w.coord.st.fc_out;
    s.w.coord.st.fc_out += 1;
    s.tx_zcl(0x0006, 0x02, &[], Tx::new().ddr().fc(fc));
    s.run(500.0)?;
    let v1 = onoff(s)?;
    if v1 == v0 {
        return Err(Stop::Inconclusive("control failed: a Toggle with a fresh frame counter was not executed".into()));
    }
    s.tx_zcl(0x0006, 0x02, &[], Tx::new().ddr().fc(fc));
    s.run(500.0)?;
    let v2 = onoff(s)?;
    expect(
        v2 == v1,
        "a Toggle secured with the same frame counter as an earlier frame (new NWK sequence number and APS counter) \
         was executed",
    )?;
    s.tx_zcl(0x0006, 0x02, &[], Tx::new().ddr().fc(fc - 6));
    s.run(500.0)?;
    let v3 = onoff(s)?;
    expect(v3 == v2, "a Toggle secured with a frame counter below earlier frames was executed")?;
    Ok(String::new())
}

// ---------------------------------------------------------------- ageing of router neighbours (long; last)
fn link_status_ageing(s: &mut Session) -> R<String> {
    let (_r2, r3) = net(s)?;
    let a3 = short(s, r3);
    s.peer(r3).stop_link_status();
    let t0 = s.now();
    let r = (|| -> R<Option<f64>> {
        while s.now() - t0 < 95000.0 {
            let Some(f) = next_ls(s, 18000.0, None)? else { break };
            let e = ls_entries(&f);
            if eget(&e, a3).map_or(true, |x| x.1 == 0) {
                return Ok(Some(s.now() - t0));
            }
        }
        Ok(None)
    })();
    s.start_link_status(r3, 15000.0, None);
    let seen = r?;
    expect(
        seen.is_some(),
        "95 s (more than six periods) after the neighbour's last Link Status the bulb still reports a non-zero \
         outgoing cost for it",
    )?;
    Ok(format!("outgoing cost cleared {:.0} s after the neighbour's last Link Status", seen.unwrap() / 1000.0))
}
