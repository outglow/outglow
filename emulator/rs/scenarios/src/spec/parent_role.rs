//! The bulb as the parent of end devices (R22 3.6.10, 3.6.2.3, 3.6.5, 4.6.3),
//! ported from emulator/spec/parent_role.py. The end devices are scripted at
//! frame level (peer.rs) and hear only the bulb.

use super::Test;
use crate::air::NodeId;
use crate::peer::EdJoin;
use crate::session::{expect, is_aps_cmd, is_nwk_cmd, CapFrame, Session, Stop, R};
use crate::station::{SrcIeee, Tx};

const SA: &str = "parent-a";
const SB: &str = "parent-b";
const EXT: u64 = 0x00124B00AA000000;

macro_rules! t {
    ($id:expr, $doc:expr, $clause:expr, $req:expr, $sess:expr, $run:expr) => {
        Test { id: $id, doc: $doc, clause: $clause, req: $req, session: $sess, run: $run, ..Test::D }
    };
    ($id:expr, $doc:expr, $clause:expr, $req:expr, $sess:expr, $run:expr, $($k:ident: $v:expr),+) => {
        Test { id: $id, doc: $doc, clause: $clause, req: $req, session: $sess, run: $run, $($k: $v),+, ..Test::D }
    };
}

pub fn tests() -> Vec<Test> {
    vec![
        t!("PAR-JOIN-01", "R22", "4.6.3.2.1, Table 4-32", "A router through which a device joins by MAC association \
           sends the Trust Center an Update Device command with the device's addresses and status 0x01 \
           (standard device unsecured join).", SA, join_update_device),
        t!("PAR-JOIN-02", "R22", "4.6.3.7.2", "On an APS Tunnel command the router sends the embedded command to the \
           named device with NWK security disabled (this is how a joiner gets the network key).", SA, join_tunnel),
        t!("PAR-POLL-01", "R22", "3.6.10.9 (Figure 3-56), 3.6.5", "A frame for a child with macRxOnWhenIdle = FALSE \
           is held and handed over after the child's data poll, whose acknowledgement has the frame pending bit \
           set; a poll with nothing waiting is acknowledged with the bit clear.", SA, poll_indirect),
        t!("PAR-POLL-02", "R22", "Table 3-58 (nwkTransactionPersistenceTime)", "A frame waiting for a sleeping child \
           is kept for nwkTransactionPersistenceTime (0x01f4 superframe periods = 7.68 s) and dropped after that.",
           SA, poll_persistence, mandatory: false,
           ambiguous: Some("the unit (superframe period, aBaseSuperframeDuration = 15.36 ms) \
           and the indirect queue itself are defined in IEEE 802.15.4, which is not among the documents the suite \
           reads; the test only requires delivery at 3 s and no delivery at 12 s")),
        t!("PAR-RELAY-01", "R22", "3.6.3.3, 3.6.2.2, 4.3.1.1", "A unicast frame from a child for another device is \
           relayed: same NWK source and sequence number, radius decremented, secured again by the relaying \
           router with its own frame counter and address.", SA, relay_from_child),
        t!("PAR-TMO-01", "R22", "3.6.10.2 step 6, 3.4.12", "A valid End Device Timeout Request from a child is \
           answered with status SUCCESS and a parent information bitmask naming at least one keepalive method; \
           the response has radius 1 and both IEEE addresses in the NWK header.", SA, timeout_ok),
        t!("PAR-TMO-02", "R22", "3.6.10.2 step 1", "An End Device Timeout Request with a timeout enumeration outside \
           0-14 is answered with status INCORRECT_VALUE (0x01).", SA, timeout_bad_enum),
        t!("PAR-TMO-03", "R22", "3.4.11.3.2, 3.6.10.2 step 3", "An End Device Timeout Request with a non-zero End \
           Device Configuration field is answered with status INCORRECT_VALUE (0x01).", SA, timeout_bad_config),
        t!("PAR-ADDR-01", "R22", "2.4.3.1.1.2", "A NWK_addr_req for the IEEE address of a router's end device child \
           is answered by the router (also when the request was broadcast to 0xFFFD) with SUCCESS and the \
           child's network address.", SA, nwk_addr_for_child),
        t!("PAR-ADDR-02", "R22", "2.4.3.1.2.2", "An IEEE_addr_req for the network address of a router's end device \
           child is answered by the router with SUCCESS and the child's IEEE address.", SA, ieee_addr_for_child),
        t!("PAR-BCAST-01", "R22", "3.6.5", "A router with a macRxOnWhenIdle = FALSE neighbour passes a broadcast \
           for 0xFFFF on to it as a MAC unicast (indirect transmission allowed); a broadcast for 0xFFFD is not \
           passed to it.", SA, broadcast_to_sleepy_child),
        t!("PAR-RREC-01", "R22", "3.6.3.5.4, 3.4.5", "When a router relays a unicast frame of its end device child \
           towards a concentrator (many-to-one route), it first unicasts a Route Record whose NWK source is the \
           child, relay count 1 and relay list holding the router's own address.", SA, route_record_for_child),
        t!("PAR-RREQ-01", "R22", "3.6.3.5.2", "A router answers a Route Request for the address of one of its end \
           device children itself, with the child's address as responder address in the Route Reply.", SA,
           route_request_for_child),
        t!("PAR-LEAVE-01", "R22", "4.6.3.6.2", "When a child announces that it leaves with Rejoin = 0, its router \
           sends the Trust Center an Update Device command with status 0x02 (device left).", SA, child_leave),
        t!("PAR-LEAVE-02", "R22", "4.6.3.6.2, 3.6.1.10.3", "A child's Leave with Rejoin = 1 is not reported to the \
           Trust Center as 'device left': Update Device with status 0x02 is issued only for Rejoin = 0.", SA,
           child_leave_rejoin),
        t!("PAR-AGE-01", "R22", "3.6.10.1, 3.6.10.2 step 4", "A child that negotiated a 10 s timeout and then sends \
           nothing is deleted from the neighbour table when the timeout runs out.", SB, age_out),
        t!("PAR-AGE-02", "R22", "3.6.10.1, 4.6.3.6.2", "Ageing out a child only deletes the entry; the Trust Center \
           is not told 'device left' (Update Device status 0x02 follows a Leave with Rejoin = 0, nothing else).", SB,
           age_out_silent),
        t!("PAR-AGE-03", "R22", "3.6.10.3, 3.6.10.4 item 2", "A keepalive (MAC data poll, or End Device Timeout \
           Request where only that method is offered) resets the child's timeout counter: a child that keeps \
           sending keepalives is not aged out.", SB, keepalive),
        t!("PAR-UNK-01", "R22", "3.6.10.4 item 1, 3.6.10.4.1", "A MAC data poll from a device that is not in the \
           neighbour table makes the router queue a leave message for it: NWK Leave with request = 1, rejoin = 1 \
           and no destination IEEE address, or Mgmt_Leave_req with a null device address and Rejoin = 1.", SB,
           poll_from_unknown),
        t!("PAR-MOVED-01", "R22", "3.6.2.3 steps 1-5", "When a unicast frame whose NWK source is one of the router's \
           end device children arrives from a different MAC source, another router has become the child's \
           parent: the neighbour table entry is deleted.", SB, moved_child_unicast),
        t!("PAR-MOVED-02", "R22", "3.6.2.3 (Device_annce), 2.4.3.1.11.2", "A broadcast Device_annce for the IEEE \
           address of one of the router's end device children that was relayed by another device (no entry in \
           the broadcast transaction table) deletes the child's neighbour table entry.", SB, moved_child_annce),
        t!("PAR-REJOIN-01", "R22", "3.6.1.4.2 (parent procedure), 3.4.7, Table 4-32", "A secured Rejoin Request from \
           a known child is answered with a Rejoin Response (status 0, the address already on record, secured \
           like the request, both IEEE addresses) and reported to the Trust Center with Update Device status \
           0x00 (secured rejoin).", SB, child_secure_rejoin),
        t!("PAR-REJOIN-02", "R22", "4.6.3.3.2, 3.4.7.2, Table 4-32", "An unsecured Rejoin Request (Trust Center \
           rejoin) in a centralized network is answered with an unsecured Rejoin Response and reported with \
           Update Device status 0x03; the network key then reaches the device through the router.", SB,
           child_tc_rejoin),
        t!("PAR-REMOVE-01", "R22", "4.6.3.6.2", "A Remove Device command from a device other than the Trust Center \
           is ignored; one from the Trust Center naming a child makes the router send that child a NWK Leave \
           with request = 1 and rejoin = 0.", SB, remove_device),
    ]
}

// ---- Python-style texts
fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
fn opt<T: std::fmt::Display>(v: Option<T>) -> String {
    v.map(|x| x.to_string()).unwrap_or_else(|| "None".into())
}
fn tf(b: bool) -> &'static str {
    if b {
        "True"
    } else {
        "False"
    }
}
/// repr() of a str
fn py_str(s: &str) -> String {
    let q = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut o = String::new();
    o.push(q);
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\t' => o.push_str("\\t"),
            '\r' => o.push_str("\\r"),
            c if c == q => {
                o.push('\\');
                o.push(q);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => o.push_str(&format!("\\x{:02x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push(q);
    o
}
fn briefs(v: &[CapFrame]) -> String {
    format!("[{}]", v.iter().map(|f| py_str(&f.brief())).collect::<Vec<_>>().join(", "))
}
fn b(p: &[u8], i: usize) -> u8 {
    p.get(i).copied().unwrap_or(0)
}
fn u16_at(p: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b(p, i), b(p, i + 1)])
}
fn u64_at(p: &[u8], i: usize) -> u64 {
    let mut v = 0u64;
    for k in 0..8 {
        v |= (b(p, i + k) as u64) << (8 * k);
    }
    v
}
fn nwk_pl(f: &CapFrame) -> Vec<u8> {
    f.nwk.as_ref().and_then(|n| n.payload.clone()).unwrap_or_default()
}

// ---- local helpers
fn ed_short(s: &mut Session, p: NodeId) -> u16 {
    s.peer(p).st.short16()
}
fn ed_ext(s: &mut Session, p: NodeId) -> u64 {
    s.peer(p).st.ext
}

/// A fresh end device joined through the bulb, or INCONCLUSIVE with the reason.
fn new_ed(s: &mut Session, n: u64, timeout: Option<u8>, cap: u8) -> R<(NodeId, EdJoin)> {
    let ed = s.end_device(&format!("ed{n}"), EXT + n, cap, true)?;
    let r = s.ed_join(ed, timeout, true)?;
    if !r.ok {
        s.remove_peer(ed);
        return Err(Stop::Inconclusive(format!("the scripted end device could not join through the bulb: {}", r.why)));
    }
    Ok((ed, r))
}

fn shared_ed(s: &mut Session) -> R<NodeId> {
    if !s.has_note("ed") {
        let (ed, r) = new_ed(s, 1, None, 0x80)?;
        s.set_note("ed", Some(ed));
        s.set_note("ed_join", r);
    }
    match s.note::<Option<NodeId>>("ed").unwrap() {
        Some(ed) => Ok(ed),
        None => Err(Stop::Inconclusive("the end device of this session has left in an earlier test".into())),
    }
}

/// (status, short, frame) of the Update Device commands for `ext` the bulb sent since the mark.
fn update_devices(s: &mut Session, since: usize, ext: u64) -> Vec<(Option<u8>, Option<u16>, CapFrame)> {
    let mut out = Vec::new();
    for f in s.dut_frames(since, is_aps_cmd(0x06)) {
        let Some(p) = f.aps.as_ref().unwrap().payload.clone() else {
            out.push((None, None, f));
            continue;
        };
        let (e, short, st) = (u64_at(&p, 1), u16_at(&p, 9), b(&p, 11));
        if e == ext {
            out.push((Some(st), Some(short), f));
        }
    }
    out
}

/// Is the device still in the bulb's neighbour table as an end device? Decided
/// by the rule of 3.6.10.2 step 2: an End Device Timeout Request from a device
/// without an entry is dropped.
fn child_known(s: &mut Session, ed: NodeId) -> R<(bool, String)> {
    let i = s.mark();
    let t = s.ed_timeout_request(ed, 8, 0, 200.0)?;
    let short = s.peer(ed).st.short;
    let leave = s.dut_frames(i, |f| {
        f.nwk.as_ref().is_some_and(|n| Some(n.dst) == short)
            && (f.nwk_cmd() == Some(0x04) || f.zdo().is_some_and(|z| z.cluster == 0x0034))
    });
    if let Some(t) = &t {
        if t.status == 0 {
            return Ok((true, "End Device Timeout Response SUCCESS".into()));
        }
    }
    let head = match &t {
        None => "no End Device Timeout Response".to_string(),
        Some(t) => format!("Timeout Response status {:#04x}", t.status),
    };
    Ok((false, head + if leave.is_empty() { "" } else { "; the bulb sent the device a leave message" }))
}

fn r2_router(s: &mut Session) -> NodeId {
    match s.note::<NodeId>("r2") {
        Some(r) => r,
        None => {
            let r = s.router2();
            s.set_note("r2", r);
            r
        }
    }
}

fn report_payload() -> Vec<u8> {
    vec![0x00, 0x00, 0x10, 0x01]
}

// ---------------------------------------------------------------- session A
fn join_update_device(s: &mut Session) -> R<String> {
    let ed = shared_ed(s)?;
    let r = s.note::<EdJoin>("ed_join").unwrap();
    let ud: Vec<&CapFrame> = r.update_device.iter().filter(|f| f.aps.as_ref().unwrap().payload.is_some()).collect();
    expect(!ud.is_empty(), "no readable Update Device command from the bulb during the join")?;
    let (ext, short) = (ed_ext(s, ed), ed_short(s, ed));
    let ok: Vec<&&CapFrame> = ud
        .iter()
        .filter(|f| {
            let p = f.aps.as_ref().unwrap().payload.as_ref().unwrap();
            (u64_at(p, 1), u16_at(p, 9), b(p, 11)) == (ext, short, 0x01)
        })
        .collect();
    expect(
        !ok.is_empty(),
        "Update Device seen: ".to_string()
            + &ud.iter().map(|f| hex(f.aps.as_ref().unwrap().payload.as_ref().unwrap())).collect::<Vec<_>>().join("; ")
            + &format!(" (expected device {ext:#018x}, short {short:#06x}, status 0x01)"),
    )?;
    let d = ok[0].nwk.as_ref().unwrap().dst;
    expect(d == 0x0000, format!("Update Device sent to {d:#06x}, the Trust Center is 0x0000"))?;
    Ok(format!(
        "{} copy/copies, APS-encrypted: [{}]",
        ud.len(),
        ud.iter().map(|f| tf(f.aps.as_ref().unwrap().secured)).collect::<Vec<_>>().join(", ")
    ))
}

fn join_tunnel(s: &mut Session) -> R<String> {
    let ed = shared_ed(s)?;
    let r = s.note::<EdJoin>("ed_join").unwrap();
    let f = r.key_frame.unwrap();
    expect(!f.nwk.as_ref().unwrap().secured, "the Transport Key was forwarded to the joiner with NWK security")?;
    let a = f.aps.as_ref().unwrap();
    expect(
        a.secured && a.sec_src == Some(s.w.coord.ext),
        "the forwarded command does not carry the Trust Center's APS auxiliary header",
    )?;
    let (short, ext) = (ed_short(s, ed), ed_ext(s, ed));
    let md = f.mac.as_ref().unwrap().dst;
    expect(
        md == Some(short as u64) || md == Some(ext),
        format!("sent to MAC destination {:#x}", md.unwrap_or(0)),
    )?;
    Ok(String::new())
}

fn poll_indirect(s: &mut Session) -> R<String> {
    let ed = shared_ed(s)?;
    let p = s.ed_poll(ed, 60.0, false)?;
    expect(p.acked, "the data poll was not acknowledged")?;
    expect(
        !p.pending && p.frames.is_empty(),
        format!(
            "nothing was queued, but the acknowledgement has frame pending = {} and {} frame(s) followed",
            tf(p.pending),
            p.frames.len()
        ),
    )?;
    let i = s.mark();
    let short = ed_short(s, ed);
    let r = s.tx_zcl(0x0006, 0x00, &[0, 0], Tx::new().general().ddr().dst(short));
    s.run(1500.0)?;
    let seq = r.nwk_seq;
    let early = s.dut_frames(i, |f| f.nwk.as_ref().is_some_and(|n| n.dst == short && n.seq == seq));
    expect(early.is_empty(), "the frame was transmitted to the sleeping child before it polled")?;
    let p = s.ed_poll(ed, 60.0, false)?;
    let got: Vec<&CapFrame> = p
        .frames
        .iter()
        .filter(|f| f.nwk.as_ref().is_some_and(|n| n.dst == short && n.src == 0x0000 && n.seq == seq))
        .collect();
    expect(p.pending, "a frame was waiting, but the acknowledgement of the poll has frame pending = 0")?;
    expect(
        !got.is_empty(),
        format!("after the poll the bulb sent: {}; the queued frame is not among them", briefs(&p.frames)),
    )?;
    let rad = got[0].nwk.as_ref().unwrap().radius;
    expect(rad == 29, format!("radius {rad} after one hop from 30"))?;
    Ok(String::new())
}

fn poll_persistence(s: &mut Session) -> R<String> {
    let ed = shared_ed(s)?;
    let short = ed_short(s, ed);
    let r = s.tx_zcl(0x0006, 0x00, &[0, 0], Tx::new().general().ddr().dst(short));
    s.run(3000.0)?;
    let p = s.ed_poll(ed, 60.0, false)?;
    let seq = r.nwk_seq;
    expect(
        p.frames.iter().any(|f| f.nwk.as_ref().is_some_and(|n| n.seq == seq && n.src == 0)),
        "a frame queued 3 s before the poll was not delivered",
    )?;
    let r = s.tx_zcl(0x0006, 0x00, &[0, 0], Tx::new().general().ddr().dst(short));
    for _ in 0..4 {
        s.run(3000.0)?;
    }
    let p = s.ed_poll(ed, 60.0, false)?;
    let seq = r.nwk_seq;
    let late = p.frames.iter().any(|f| f.nwk.as_ref().is_some_and(|n| n.seq == seq && n.src == 0));
    expect(!late, "a frame queued 12 s before the poll was still delivered (persistence time is 7.68 s)")?;
    Ok(String::new())
}

fn relay_from_child(s: &mut Session) -> R<String> {
    let ed = shared_ed(s)?;
    let i = s.mark();
    let (short, ext) = (ed_short(s, ed), ed_ext(s, ed));
    let r = {
        let (st, air) = s.w.st(ed);
        st.tx_zcl(air, 0x0000, 0x0006, 0x0A, &report_payload(), &Tx::new().general().ddr().to_client().radius(12))
    };
    let seq = r.nwk_seq;
    let f = s.wait_dut(
        move |f| f.nwk.as_ref().is_some_and(|n| n.src == short && n.dst == 0 && n.seq == seq),
        500.0,
        Some(i),
    )?;
    let Some(f) = f else { return Err(Stop::Fail("the child's frame for the coordinator was not relayed".into())) };
    let md = f.mac.as_ref().unwrap().dst.unwrap_or(0);
    expect(md == 0x0000, format!("relayed to MAC destination {md:#06x}"))?;
    let n = f.nwk.as_ref().unwrap();
    expect(n.radius == 11, format!("radius {} after one hop from 12", n.radius))?;
    expect(
        n.sec_src == Some(s.dut_ext),
        format!(
            "auxiliary header source {:#018x}: the relaying router must secure the frame itself",
            n.sec_src.unwrap_or(0)
        ),
    )?;
    expect(
        n.src_ieee.is_none() || n.src_ieee == Some(ext),
        "source IEEE address of the relayed frame is not the child's",
    )?;
    Ok(String::new())
}

fn timeout_ok(s: &mut Session) -> R<String> {
    let ed = shared_ed(s)?;
    let t = s.ed_timeout_request(ed, 8, 0, 200.0)?;
    let Some(t) = t else { return Err(Stop::Fail("no End Device Timeout Response".into())) };
    expect(t.status == 0, format!("status {:#04x}", t.status))?;
    expect(
        t.parent_info & 3 != 0,
        format!(
            "parent information {:#04x}: neither MAC data poll keepalive (bit 0) nor End Device Timeout Request \
             keepalive (bit 1)",
            t.parent_info
        ),
    )?;
    let n = t.frame.nwk.as_ref().unwrap();
    expect(n.radius == 1, format!("radius {}", n.radius))?;
    let ext = ed_ext(s, ed);
    expect(
        n.src_ieee == Some(s.dut_ext) && n.dst_ieee == Some(ext),
        format!("IEEE addresses in the NWK header: source {}, destination {}", opt(n.src_ieee), opt(n.dst_ieee)),
    )?;
    s.set_note("parent_info", t.parent_info);
    Ok(format!("parent information {:#04x}", t.parent_info))
}

fn timeout_bad_enum(s: &mut Session) -> R<String> {
    let ed = shared_ed(s)?;
    let t = s.ed_timeout_request(ed, 15, 0, 200.0)?;
    let Some(t) = t else {
        return Err(Stop::Fail("no End Device Timeout Response to requested timeout enumeration 15".into()));
    };
    expect(t.status == 0x01, format!("status {:#04x} for enumeration 15", t.status))?;
    s.ed_timeout_request(ed, 8, 0, 200.0)?;
    Ok(String::new())
}

fn timeout_bad_config(s: &mut Session) -> R<String> {
    let ed = shared_ed(s)?;
    let t = s.ed_timeout_request(ed, 8, 0x01, 200.0)?;
    let Some(t) = t else {
        return Err(Stop::Fail("no End Device Timeout Response to End Device Configuration 0x01".into()));
    };
    expect(t.status == 0x01, format!("status {:#04x} for End Device Configuration 0x01", t.status))?;
    s.ed_timeout_request(ed, 8, 0, 200.0)?;
    Ok(String::new())
}

fn nwk_addr_for_child(s: &mut Session) -> R<String> {
    let ed = shared_ed(s)?;
    let (short, ext) = (ed_short(s, ed), ed_ext(s, ed));
    let mut q = ext.to_le_bytes().to_vec();
    q.extend_from_slice(&[0, 0]);
    let f = s.zdo_with(0x0000, &q, Tx::new().dst(0xFFFD))?;
    let Some(f) = f else {
        return Err(Stop::Fail(
            "no NWK_addr_rsp from the parent to a broadcast request for its child's IEEE address".into(),
        ));
    };
    let p = f.zdo().unwrap().payload;
    expect(b(&p, 0) == 0, format!("status {:#04x}", b(&p, 0)))?;
    let (e, sh) = (u64_at(&p, 1), u16_at(&p, 9));
    expect((e, sh) == (ext, short), format!("answer {e:#018x} -> {sh:#06x}, child is {short:#06x}"))?;
    let f = s.zdo(0x0000, &q, 2000.0)?;
    let ok = f.as_ref().is_some_and(|f| f.zdo().unwrap().payload.first() == Some(&0));
    expect(
        ok,
        "unicast request for the child's address: ".to_string()
            + &match &f {
                Some(f) => format!("status {:#04x}", b(&f.zdo().unwrap().payload, 0)),
                None => "no response".into(),
            },
    )?;
    Ok(String::new())
}

fn ieee_addr_for_child(s: &mut Session) -> R<String> {
    let ed = shared_ed(s)?;
    let (short, ext) = (ed_short(s, ed), ed_ext(s, ed));
    let mut q = short.to_le_bytes().to_vec();
    q.extend_from_slice(&[0, 0]);
    let f = s.zdo(0x0001, &q, 2000.0)?;
    let Some(f) = f else {
        return Err(Stop::Fail("no IEEE_addr_rsp from the parent for its child's network address".into()));
    };
    let p = f.zdo().unwrap().payload;
    expect(b(&p, 0) == 0, format!("status {:#04x}", b(&p, 0)))?;
    let (e, sh) = (u64_at(&p, 1), u16_at(&p, 9));
    expect((e, sh) == (ext, short), format!("answer {sh:#06x} -> {e:#018x}"))?;
    Ok(String::new())
}

fn broadcast_to_sleepy_child(s: &mut Session) -> R<String> {
    let ed = shared_ed(s)?;
    s.ed_poll(ed, 60.0, false)?;
    let r = s.tx_zcl(0x0003, 0x01, &[], Tx::new().ddr().radius(4).dst(0xFFFF));
    s.run(400.0)?;
    let p = s.ed_poll(ed, 60.0, false)?;
    let seq = r.nwk_seq;
    let got: Vec<&CapFrame> = p
        .frames
        .iter()
        .filter(|f| f.nwk.as_ref().is_some_and(|n| n.dst == 0xFFFF && n.seq == seq && n.src == 0))
        .collect();
    expect(
        !got.is_empty(),
        format!(
            "broadcast to 0xFFFF did not reach the sleeping child on its poll (pending bit {}, frames {})",
            tf(p.pending),
            briefs(&p.frames)
        ),
    )?;
    let short = ed_short(s, ed);
    expect(got[0].mac.as_ref().unwrap().dst == Some(short as u64), "not sent as a MAC unicast to the child")?;
    let r = s.tx_zcl(0x0003, 0x01, &[], Tx::new().ddr().radius(4).dst(0xFFFD));
    s.run(400.0)?;
    let p = s.ed_poll(ed, 60.0, false)?;
    let seq = r.nwk_seq;
    let got = p.frames.iter().any(|f| f.nwk.as_ref().is_some_and(|n| n.dst == 0xFFFD && n.seq == seq));
    expect(!got, "a broadcast for 0xFFFD (macRxOnWhenIdle = TRUE devices) was passed to the sleeping child")?;
    Ok(String::new())
}

fn route_record_for_child(s: &mut Session) -> R<String> {
    let ed = shared_ed(s)?;
    {
        let w = &mut s.w;
        w.coord.many_to_one_route_request(&mut w.air, true);
    }
    s.run(1500.0)?;
    let i = s.mark();
    let (short, ext) = (ed_short(s, ed), ed_ext(s, ed));
    let r = {
        let (st, air) = s.w.st(ed);
        st.tx_zcl(air, 0x0000, 0x0006, 0x0A, &report_payload(), &Tx::new().general().ddr().to_client())
    };
    let seq = r.nwk_seq;
    let data = s.wait_dut(
        move |f| f.nwk.as_ref().is_some_and(|n| n.ty == 0 && n.src == short && n.seq == seq),
        800.0,
        Some(i),
    )?;
    let Some(data) = data else {
        return Err(Stop::Fail("the child's frame for the concentrator was not relayed at all".into()));
    };
    let dut = s.dut_short;
    let rr: Vec<CapFrame> =
        s.dut_frames(i, is_nwk_cmd(0x05)).into_iter().filter(|f| f.nwk.as_ref().unwrap().src == short && f.i < data.i).collect();
    let own = s.dut_frames(i, is_nwk_cmd(0x05)).into_iter().any(|f| f.nwk.as_ref().unwrap().src == dut);
    expect(
        !rr.is_empty(),
        "no Route Record with the child as NWK source before the relayed frame".to_string()
            + if own { " (a Route Record with the router itself as source was sent)" } else { "" },
    )?;
    let p = nwk_pl(&rr[0]);
    let relays: Vec<u16> = (0..b(&p, 1) as usize).map(|k| u16_at(&p, 2 + 2 * k)).collect();
    expect(
        b(&p, 1) == 1 && relays == vec![dut],
        format!(
            "relay count {}, relay list [{}]",
            b(&p, 1),
            relays.iter().map(|x| format!("'{x:#x}'")).collect::<Vec<_>>().join(", ")
        ),
    )?;
    let n = rr[0].nwk.as_ref().unwrap();
    expect(
        n.dst == 0x0000 && n.src_ieee == Some(ext),
        format!("Route Record header: destination {:#06x}, source IEEE {}", n.dst, opt(n.src_ieee)),
    )?;
    Ok(String::new())
}

fn route_request_for_child(s: &mut Session) -> R<String> {
    let ed = shared_ed(s)?;
    let r2 = s.router("rq", 0x00124B00BB000005, 0x2B05, Some(&["dut"]), true);
    let res = (|| -> R<String> {
        let r2_short = s.peer(r2).st.short16();
        let mut ok = false;
        for _ in 0..3 {
            // the request is only accepted from a neighbour with a known cost
            let f = s.wait_dut(is_nwk_cmd(0x08), 18000.0, None)?;
            let Some(f) = f else { break };
            let p = nwk_pl(&f);
            let n = (b(&p, 1) & 0x1F) as usize;
            let mut cost = 0u8;
            for k in 0..n {
                if u16_at(&p, 2 + 3 * k) == r2_short {
                    cost = b(&p, 4 + 3 * k);
                }
            }
            if cost >> 4 != 0 {
                ok = true;
                break;
            }
        }
        if !ok {
            return Err(Stop::Inconclusive(
                "the scripted router did not become a neighbour of the bulb (not in its Link Status)".into(),
            ));
        }
        let i = s.mark();
        let short = ed_short(s, ed);
        let mut cmd = vec![0x01, 0x00, 0x71];
        cmd.extend_from_slice(&short.to_le_bytes());
        cmd.push(0);
        {
            let (st, air) = s.w.st(r2);
            st.tx_nwk_cmd(air, 0xFFFC, &cmd, &Tx::new().radius(30));
        }
        let g = s.wait_dut(|f| f.nwk_cmd() == Some(0x02) && nwk_pl(f).get(2) == Some(&0x71), 2000.0, Some(i))?;
        let relayed = s.dut_frames(i, is_nwk_cmd(0x01)).into_iter().any(|f| f.nwk.as_ref().unwrap().src == r2_short);
        let Some(g) = g else {
            return Err(Stop::Fail(
                "no Route Reply to a Route Request for the address of the bulb's end device child".to_string()
                    + if relayed { " (the request was rebroadcast instead)" } else { "" },
            ));
        };
        let p = nwk_pl(&g);
        let (orig, resp) = (u16_at(&p, 3), u16_at(&p, 5));
        expect(
            orig == r2_short && resp == short,
            format!(
                "Route Reply: originator {orig:#06x}, responder {resp:#06x}; the responder must be the child {short:#06x}"
            ),
        )?;
        let md = g.mac.as_ref().unwrap().dst.unwrap_or(0);
        expect(md == r2_short as u64, format!("reply sent to {md:#06x}"))?;
        Ok(String::new())
    })();
    s.remove_peer(r2);
    res
}

fn child_leave(s: &mut Session) -> R<String> {
    let ed = shared_ed(s)?;
    let i = s.mark();
    s.ed_leave(ed, false);
    s.run(3000.0)?;
    s.set_note::<Option<NodeId>>("ed", None);
    s.remove_peer(ed);
    let ext = ed_ext(s, ed);
    let ud = update_devices(s, i, ext);
    let seen = format!(
        "[{}]",
        ud.iter().map(|(st, _, f)| format!("({}, {})", opt(*st), py_str(&f.brief()))).collect::<Vec<_>>().join(", ")
    );
    expect(
        ud.iter().any(|(st, _, _)| *st == Some(0x02)),
        format!("no Update Device with status 0x02 within 3 s of the child's Leave; seen: {seen}"),
    )?;
    Ok(String::new())
}

fn child_leave_rejoin(s: &mut Session) -> R<String> {
    let (ed, _) = new_ed(s, 2, None, 0x80)?;
    let i = s.mark();
    s.ed_leave(ed, true);
    s.run(5000.0)?;
    s.remove_peer(ed);
    let ext = ed_ext(s, ed);
    let ud = update_devices(s, i, ext);
    expect(
        !ud.iter().any(|(st, _, _)| *st == Some(0x02)),
        "the bulb reported the child as 'device left' (Update Device status 0x02) after a Leave with Rejoin = 1",
    )?;
    Ok(String::new())
}

// ---------------------------------------------------------------- session B
fn age_out(s: &mut Session) -> R<String> {
    let (ed, _) = new_ed(s, 3, None, 0x80)?;
    let t = s.ed_timeout_request(ed, 0, 0, 200.0)?;
    let Some(t) = t.filter(|t| t.status == 0) else {
        return Err(Stop::Inconclusive(
            "the bulb did not accept the 10 s timeout request, so the default timeout applies".into(),
        ));
    };
    s.set_note("parent_info", t.parent_info);
    let i = s.mark();
    s.run(16000.0)?;
    s.set_note("age", (ed, i));
    let (known, seen) = child_known(s, ed)?;
    expect(!known, format!("16 s after its last frame the child with a 10 s timeout is still known ({seen})"))?;
    Ok(seen)
}

fn age_out_silent(s: &mut Session) -> R<String> {
    let Some((ed, i)) = s.note::<(NodeId, usize)>("age") else {
        return Err(Stop::Inconclusive("the ageing test before this one did not get as far as the timeout".into()));
    };
    s.run(2000.0)?;
    s.remove_peer(ed);
    let ext = ed_ext(s, ed);
    let ud = update_devices(s, i, ext);
    expect(
        !ud.iter().any(|(st, _, _)| *st == Some(0x02)),
        "the bulb sent the Trust Center Update Device status 0x02 (device left) for a child that merely timed out",
    )?;
    Ok("no Update Device (device left) for the child in the 18 s around the timeout".into())
}

fn keepalive(s: &mut Session) -> R<String> {
    let (ed, _) = new_ed(s, 4, None, 0x80)?;
    let t = s.ed_timeout_request(ed, 0, 0, 200.0)?;
    let Some(t) = t.filter(|t| t.status == 0) else {
        return Err(Stop::Inconclusive("the bulb did not accept the 10 s timeout request".into()));
    };
    s.set_note("parent_info", t.parent_info);
    for _ in 0..8 {
        s.run(3000.0)?;
        if t.parent_info & 1 != 0 {
            s.ed_poll(ed, 60.0, false)?;
        } else {
            s.ed_timeout_request(ed, 0, 0, 200.0)?;
        }
    }
    let (known, seen) = child_known(s, ed)?;
    s.set_note("ka_ed", ed);
    expect(
        known,
        format!("a child that sent a keepalive every 3 s for 24 s (timeout 10 s) is no longer known: {seen}"),
    )?;
    Ok(String::new())
}

fn poll_from_unknown(s: &mut Session) -> R<String> {
    let info = match s.note::<u8>("parent_info") {
        Some(v) => v,
        None => {
            let (ed0, _) = new_ed(s, 9, None, 0x80)?;
            let t = s.ed_timeout_request(ed0, 8, 0, 200.0)?;
            let v = t.map(|t| t.parent_info).unwrap_or(0);
            s.remove_peer(ed0);
            v
        }
    };
    if info & 1 == 0 {
        return Err(Stop::NotApplicable(format!(
            "parent information {info:#04x}: the bulb does not offer MAC data poll keepalive, and 3.6.10.4 applies \
             to routers that do"
        )));
    }
    let ghost = s.new_end_device("ghost", EXT + 0x77, 0x80);
    let gshort = 0x4E55u16;
    {
        let g = s.peer(ghost);
        g.st.short = Some(gshort);
        g.joined = true;
    }
    s.w.air.keys.learn(Some(gshort), Some(EXT + 0x77));
    let i = s.mark();
    let p1 = s.ed_poll(ghost, 60.0, false)?;
    s.run(300.0)?;
    let p2 = s.ed_poll(ghost, 60.0, false)?;
    s.run(300.0)?;
    let p3 = s.ed_poll(ghost, 60.0, false)?;
    s.remove_peer(ghost);
    expect(p1.acked, "the poll of the unknown device was not even acknowledged")?;
    let to_ghost = s.dut_frames(i, |f| f.nwk.as_ref().is_some_and(|n| n.dst == gshort));
    let leave: Vec<&CapFrame> = to_ghost.iter().filter(|f| f.nwk_cmd() == Some(0x04)).collect();
    let mgmt: Vec<&CapFrame> = to_ghost.iter().filter(|f| f.zdo().is_some_and(|z| z.cluster == 0x0034)).collect();
    expect(
        !leave.is_empty() || !mgmt.is_empty(),
        format!(
            "no leave message for the unknown device after three polls (pending bits [{}, {}, {}], frames to it: {})",
            tf(p1.pending),
            tf(p2.pending),
            tf(p3.pending),
            briefs(&to_ghost)
        ),
    )?;
    if let Some(l) = leave.first() {
        let o = b(&nwk_pl(l), 1);
        expect(
            o & 0x40 != 0 && o & 0x20 != 0,
            format!("Leave options {o:#04x}: request (bit 6) and rejoin (bit 5) must both be 1"),
        )?;
        expect(l.nwk.as_ref().unwrap().dst_ieee.is_none(), "the Leave carries a destination IEEE address")?;
        return Ok(format!("NWK Leave options {o:#04x}"));
    }
    let p = mgmt[0].zdo().unwrap().payload;
    expect(
        u64_at(&p, 0) == 0 && b(&p, 8) & 0x80 != 0 && b(&p, 8) & 0x40 == 0 && !mgmt[0].aps.as_ref().unwrap().ack_req,
        format!(
            "Mgmt_Leave_req {} (null address, Rejoin = 1, RemoveChildren = 0, no APS ack request required)",
            hex(&p)
        ),
    )?;
    Ok("Mgmt_Leave_req".into())
}

fn moved_child_unicast(s: &mut Session) -> R<String> {
    let (ed, _) = new_ed(s, 5, None, 0x80)?;
    let (known, seen) = child_known(s, ed)?;
    if !known {
        return Err(Stop::Inconclusive(format!("control failed: the freshly joined child is not known ({seen})")));
    }
    let r2 = r2_router(s);
    s.run(300.0)?;
    let (short, ext) = (ed_short(s, ed), ed_ext(s, ed));
    let dut = s.dut_short;
    // the child's traffic now arrives through r2: one frame the bulb has to relay, one addressed to the bulb
    {
        let (st, air) = s.w.st(r2);
        st.tx_zcl(
            air,
            0x0000,
            0x0006,
            0x0A,
            &report_payload(),
            &Tx::new().general().ddr().to_client().src(short).src_ieee(SrcIeee::Is(ext)).mac_dst(dut),
        );
    }
    s.run(400.0)?;
    {
        let (st, air) = s.w.st(r2);
        st.tx_zcl(air, dut, 0x0000, 0x00, &[0, 0], &Tx::new().general().ddr().src(short).src_ieee(SrcIeee::Is(ext)));
    }
    s.run(500.0)?;
    let (known, seen) = child_known(s, ed)?;
    s.remove_peer(ed);
    expect(
        !known,
        format!(
            "after two unicast frames of the child arrived through another router the child is still in the neighbour \
             table ({seen})"
        ),
    )?;
    Ok(seen)
}

fn moved_child_annce(s: &mut Session) -> R<String> {
    let (ed, _) = new_ed(s, 6, None, 0x80)?;
    let (known, seen) = child_known(s, ed)?;
    if !known {
        return Err(Stop::Inconclusive(format!("control failed: the freshly joined child is not known ({seen})")));
    }
    let r2 = r2_router(s);
    s.run(300.0)?;
    let (short, ext, cap) = (ed_short(s, ed), ed_ext(s, ed), s.peer(ed).cap);
    let mut body = short.to_le_bytes().to_vec();
    body.extend_from_slice(&ext.to_le_bytes());
    body.push(cap);
    {
        let (st, air) = s.w.st(r2);
        st.tx_zdo(air, 0xFFFD, 0x0013, &body, &Tx::new().src(short).src_ieee(SrcIeee::Is(ext)).seq(0xE7).radius(29));
    }
    s.run(800.0)?;
    let (known, seen) = child_known(s, ed)?;
    s.remove_peer(ed);
    expect(
        !known,
        format!(
            "after the child's Device_annce arrived through another router the child is still in the neighbour table \
             ({seen})"
        ),
    )?;
    Ok(seen)
}

fn child_secure_rejoin(s: &mut Session) -> R<String> {
    let (ed, _) = new_ed(s, 7, None, 0x80)?;
    s.set_note("rj_ed", ed);
    let i = s.mark();
    let f = s.ed_rejoin_request(ed, true, 300.0)?;
    let Some(f) = f else {
        return Err(Stop::Fail("no Rejoin Response to a secured Rejoin Request from a child".into()));
    };
    let p = nwk_pl(&f);
    let (addr, st) = (u16_at(&p, 1), b(&p, 3));
    let (short, ext) = (ed_short(s, ed), ed_ext(s, ed));
    expect(
        st == 0 && addr == short,
        format!("Rejoin Response: address {addr:#06x}, status {st:#04x} (the child is {short:#06x})"),
    )?;
    let n = f.nwk.as_ref().unwrap();
    expect(n.secured, "the response to a secured request is not secured")?;
    expect(n.src_ieee == Some(s.dut_ext) && n.dst_ieee == Some(ext), "IEEE addresses missing in the NWK header")?;
    s.run(1500.0)?;
    let ud = update_devices(s, i, ext);
    expect(
        ud.iter().any(|(st, _, _)| *st == Some(0x00)),
        format!(
            "no Update Device with status 0x00 (secured rejoin) for the child; seen statuses [{}]",
            ud.iter().map(|(st, _, _)| opt(*st)).collect::<Vec<_>>().join(", ")
        ),
    )?;
    Ok(String::new())
}

fn child_tc_rejoin(s: &mut Session) -> R<String> {
    let ed = match s.note::<NodeId>("rj_ed") {
        Some(e) => e,
        None => new_ed(s, 7, None, 0x80)?.0,
    };
    let i = s.mark();
    let f = s.ed_rejoin_request(ed, false, 300.0)?;
    let Some(f) = f else { return Err(Stop::Fail("no Rejoin Response to an unsecured Rejoin Request".into())) };
    let p = nwk_pl(&f);
    let (addr, st) = (u16_at(&p, 1), b(&p, 3));
    expect(st == 0, format!("Rejoin Response status {st:#04x}"))?;
    expect(
        !f.nwk.as_ref().unwrap().secured,
        "the response to an unsecured request is secured (3.4.7.2: same level as the request)",
    )?;
    s.peer(ed).st.short = Some(addr);
    let mut key: Option<CapFrame> = None;
    for _ in 0..8 {
        let p = s.ed_poll(ed, 60.0, false)?;
        if key.is_none() {
            key = p.frames.iter().find(|g| g.aps_cmd() == Some(0x05)).cloned();
        }
        if key.is_some() {
            break;
        }
        s.run(250.0)?;
    }
    let ext = ed_ext(s, ed);
    let ud = update_devices(s, i, ext);
    expect(
        ud.iter().any(|(st, _, _)| *st == Some(0x03)),
        format!(
            "no Update Device with status 0x03 (unsecured rejoin); seen statuses [{}]",
            ud.iter().map(|(st, _, _)| opt(*st)).collect::<Vec<_>>().join(", ")
        ),
    )?;
    let Some(key) = key else {
        return Err(Stop::Fail(
            "the Transport Key the Trust Center sent in answer did not reach the rejoining device".into(),
        ));
    };
    expect(!key.nwk.as_ref().unwrap().secured, "the Transport Key was forwarded with NWK security")?;
    s.remove_peer(ed);
    Ok(String::new())
}

fn remove_device(s: &mut Session) -> R<String> {
    let (ed, _) = new_ed(s, 8, None, 0x80)?;
    let r2 = r2_router(s);
    let i = s.mark();
    let (short, ext) = (ed_short(s, ed), ed_ext(s, ed));
    let mut body = vec![0x07];
    body.extend_from_slice(&ext.to_le_bytes());
    let dut = s.dut_short;
    {
        let (st, air) = s.w.st(r2);
        st.tx_aps_cmd(air, dut, &body, None, &Tx::new());
    }
    s.run(600.0)?;
    s.ed_poll(ed, 60.0, false)?;
    let stray = s.dut_frames(i, |f| f.nwk_cmd() == Some(0x04) && f.nwk.as_ref().unwrap().dst == short);
    expect(
        stray.is_empty(),
        "a Remove Device command from a router that is not the Trust Center made the bulb send the child a Leave",
    )?;
    let i = s.mark();
    let tclk = s.tclk();
    {
        let w = &mut s.w;
        let aps = w.coord.aps_cmd_secured(&body, &tclk, 0);
        w.coord.nwk_send_data(&mut w.air, dut, &aps);
    }
    s.run(600.0)?;
    for _ in 0..3 {
        s.ed_poll(ed, 60.0, false)?;
        s.run(300.0)?;
    }
    s.remove_peer(ed);
    let lv = s.dut_frames(i, |f| f.nwk_cmd() == Some(0x04) && f.nwk.as_ref().unwrap().dst == short);
    expect(
        !lv.is_empty(),
        "no NWK Leave for the child after Remove Device from the Trust Center (the child polled three times)",
    )?;
    let o = b(&nwk_pl(&lv[0]), 1);
    expect(o & 0x40 != 0 && o & 0x20 == 0, format!("Leave options {o:#04x}: request must be 1 and rejoin 0"))?;
    Ok(String::new())
}
