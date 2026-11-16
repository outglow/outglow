//! ZDO server services a Zigbee router must provide (R22 2.4.3 / 2.4.4) and
//! their status codes, ported from emulator/spec/zdo_services.py.

use super::Test;
use crate::session::{expect, Session, Stop, R};
use crate::station::Tx;

const SUCCESS: u8 = 0x00;
const INV_REQUESTTYPE: u8 = 0x80;
const DEVICE_NOT_FOUND: u8 = 0x81;
const INVALID_EP: u8 = 0x82;
const NOT_ACTIVE: u8 = 0x83;
const NOT_SUPPORTED: u8 = 0x84;
const NO_ENTRY: u8 = 0x88;
/// a network address nobody has in these sessions
const ABSENT: u16 = 0x5AF3;
const ABSENT_EXT: u64 = 0x00124B00DEAD0001;
const S: &str = "zdo";

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
        t!("ZDO-NODE-01", "R22", "2.4.4.2.3.1, 2.3.2.3", "Node_Desc_req for the device's own address is answered with \
           SUCCESS and a node descriptor whose logical type is router and whose frequency band is 2.4 GHz.", node_desc),
        t!("ZDO-NODE-02", "R22", "2.4.4.2.3.1", "Node_Desc_req for an address that is neither the router's own nor \
           a child's is answered with DEVICE_NOT_FOUND and no descriptor.", node_desc_other),
        t!("ZDO-NODE-03", "R22", "2.3.2.3.10", "The server mask of the node descriptor carries the stack compliance \
           revision (bits 9-15); a device built to revision 21 or later reports that revision (not 0).",
           node_desc_revision),
        t!("ZDO-POWER-01", "R22", "2.4.4.2.4.1", "Power_Desc_req for the device's own address is answered with SUCCESS \
           and the two-octet power descriptor.", power_desc),
        t!("ZDO-ACTEP-01", "R22", "2.4.4.2.6.1", "Active_EP_req is answered with SUCCESS and an ascending list of the \
           active endpoints.", active_ep),
        t!("ZDO-SIMPLE-01", "R22", "2.4.4.2.5.1", "Simple_Desc_req for an active endpoint is answered with SUCCESS, the \
           length of the descriptor and the descriptor itself.", simple_desc),
        t!("ZDO-SIMPLE-02", "R22", "2.4.4.2.5.1", "Simple_Desc_req with an endpoint outside 1-254 is answered with \
           INVALID_EP and length 0.", simple_desc_invalid),
        t!("ZDO-SIMPLE-03", "R22", "2.4.4.2.5.1", "Simple_Desc_req for an endpoint in 1-254 that is not active is \
           answered with NOT_ACTIVE and length 0.", simple_desc_inactive),
        t!("ZDO-SIMPLE-04", "R22", "2.4.4.2.5.1", "Simple_Desc_req for an address that is neither the router's own \
           nor a child's is answered with DEVICE_NOT_FOUND.", simple_desc_other),
        t!("ZDO-MATCH-01", "R22", "2.4.4.2.7.1", "A unicast Match_Desc_req that matches a local simple descriptor is \
           answered with SUCCESS and the matching endpoints.", match_desc),
        t!("ZDO-MATCH-02", "R22", "2.4.4.2.7.1 step 7", "A unicast Match_Desc_req that matches nothing is answered \
           with SUCCESS and MatchLength 0.", match_desc_none),
        t!("ZDO-MATCH-03", "R22", "2.4.4.2.7.1 steps 3, 5", "A broadcast Match_Desc_req is answered (by unicast) when \
           something matches and is not answered at all when nothing matches.", match_desc_broadcast),
        t!("ZDO-ADDR-01", "R22", "2.4.3.1.1.2", "A unicast NWK_addr_req (single device response) for the device's \
           IEEE address is answered with SUCCESS, both addresses, and no NumAssocDev/StartIndex/list.", nwk_addr),
        t!("ZDO-ADDR-02", "R22", "2.4.3.1.1.2", "A unicast NWK_addr_req for an unknown IEEE address is answered with \
           DEVICE_NOT_FOUND, the IEEE address of the request and network address 0xFFFF.", nwk_addr_unknown),
        t!("ZDO-ADDR-03", "R22", "2.4.3.1.1.2", "A broadcast NWK_addr_req is answered when the IEEE address is the \
           device's own and silently discarded when it matches nothing.", nwk_addr_broadcast),
        t!("ZDO-ADDR-04", "R22", "2.4.3.1.1.2", "A unicast NWK_addr_req with a reserved RequestType is answered with \
           INV_REQUESTTYPE.", nwk_addr_reqtype),
        t!("ZDO-ADDR-05", "R22", "2.4.3.1.2.2", "IEEE_addr_req for the device's own network address is answered with \
           SUCCESS and both addresses; for an unknown address with DEVICE_NOT_FOUND and IEEE address all ones.",
           ieee_addr),
        t!("ZDO-ADDR-06", "R22", "2.4.3.1.1.2", "NWK_addr_req with RequestType extended response is answered by a \
           router with SUCCESS and its own addresses.", nwk_addr_extended),
        t!("ZDO-BIND-01", "R22", "2.4.3.2.2.2, 2.4.3.3.4.2", "Bind_req for the device as source creates a binding \
           table entry and is answered with SUCCESS; the entry is reported by Mgmt_Bind_rsp.", bind),
        t!("ZDO-BIND-02", "R22", "2.4.3.2.3.2", "Unbind_req for an entry that does not exist is answered with NO_ENTRY.",
           unbind_missing),
        t!("ZDO-BIND-03", "R22", "2.4.3.2.2.2", "A broadcast Bind_req is dropped: no response and no binding entry.",
           bind_broadcast),
        t!("ZDO-BIND-04", "R22", "2.4.3.3.4.2", "Mgmt_Bind_req on an empty binding table is answered with SUCCESS and \
           BindingTableEntries = StartIndex = BindingTableListCount = 0 (N/A when the optional request is \
           answered NOT_SUPPORTED).", mgmt_bind_empty, mandatory: false),
        t!("ZDO-LQI-01", "R22", "2.4.3.3.2.2", "Mgmt_Lqi_req is answered with SUCCESS and the neighbour table (mandatory \
           since R22); the coordinator the device joined through is in it with its addresses.", mgmt_lqi),
        t!("ZDO-RTG-01", "R22", "2.4.3.3.3.2", "Mgmt_Rtg_req is answered with SUCCESS and the routing table, \
           five octets per entry (N/A when the optional request is answered NOT_SUPPORTED).", mgmt_rtg, mandatory: false),
        t!("ZDO-PERMIT-01", "R22", "2.4.3.3.7.2", "A unicast Mgmt_Permit_Joining_req is answered with SUCCESS; joining \
           is permitted for PermitDuration seconds and then closes; duration 0 closes at once.", permit_joining),
        t!("ZDO-PERMIT-02", "R22", "2.4.3.3.7.2", "A broadcast Mgmt_Permit_Joining_req is acted on but not answered.",
           permit_joining_broadcast),
        t!("ZDO-NWKUPD-01", "R22", "2.4.3.3.9.2 step 5, 2.4.4.4.9", "A unicast Mgmt_NWK_Update_req with ScanDuration \
           0-5 makes the device run an energy scan and answer with Mgmt_NWK_Update_notify: the scanned channels and \
           one energy value per channel.", nwk_update_scan, mandatory: false),
        t!("ZDO-NWKUPD-02", "R22", "2.4.3.3.9.2 step 6", "A unicast Mgmt_NWK_Update_req with a ScanDuration outside \
           0-5, 0xFE, 0xFF is answered with Mgmt_NWK_Update_notify carrying INVALID_REQUEST.", nwk_update_invalid,
           mandatory: false,
           ambiguous: Some("the text names the status INVALID_REQUEST, which the ZDP enumeration (Table 2-145) does not \
           define; the test accepts ZDP INV_REQUESTTYPE (0x80) and the NWK status INVALID_REQUEST (0xC2)")),
    ]
}

// ---- helpers
fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
fn b(p: &[u8], i: usize) -> u8 {
    p.get(i).copied().unwrap_or(0)
}
fn u16_at(p: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b(p, i), b(p, i + 1)])
}
fn u32_at(p: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b(p, i), b(p, i + 1), b(p, i + 2), b(p, i + 3)])
}
fn u64_at(p: &[u8], i: usize) -> u64 {
    let mut v = 0u64;
    for k in 0..8 {
        v |= (b(p, i + k) as u64) << (8 * k);
    }
    v
}
fn sl(p: &[u8], a: usize, e: usize) -> &[u8] {
    let e = e.min(p.len());
    &p[a.min(e)..e]
}
fn py_list(v: &[u8]) -> String {
    format!("[{}]", v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(", "))
}

/// The payload of the ZDO response; FAIL when there is none.
fn rsp(s: &mut Session, cluster: u16, payload: &[u8]) -> R<Vec<u8>> {
    match s.zdo(cluster, payload, 2000.0)? {
        Some(f) => Ok(f.zdo().unwrap().payload),
        None => Err(Stop::Fail(format!("no response to ZDO request {cluster:#06x}"))),
    }
}

fn short_le(a: u16) -> Vec<u8> {
    a.to_le_bytes().to_vec()
}

// ---- tests
fn node_desc(s: &mut Session) -> R<String> {
    let p = rsp(s, 0x0002, &short_le(s.dut_short))?;
    expect(b(&p, 0) == SUCCESS, format!("status {:#04x}", b(&p, 0)))?;
    expect(u16_at(&p, 1) == s.dut_short, "NWKAddrOfInterest is not echoed")?;
    expect(p.len() >= 3 + 13, format!("node descriptor has {} octets, 13 required", p.len() as i64 - 3))?;
    let nd = sl(&p, 3, 16).to_vec();
    expect(nd[0] & 7 == 1, format!("logical type {}, router is 1", nd[0] & 7))?;
    expect(nd[1] >> 3 == 0x08, format!("frequency band field {:#04x}, 2400-2483.5 MHz is bit 3 only", nd[1] >> 3))?;
    expect(nd[2] & 0x02 != 0, format!("MAC capability flags {:#04x}: device type bit (FFD) is not set for a router", nd[2]))?;
    s.set_note("node_desc", nd.clone());
    let extra = if p.len() > 16 {
        format!(
            "; {} octets follow the descriptor ({}; R23 appends TLVs here, R22 defines none)",
            p.len() - 16,
            hex(&p[16..])
        )
    } else {
        String::new()
    };
    Ok(format!("node descriptor {}{extra}", hex(&nd)))
}

fn node_desc_other(s: &mut Session) -> R<String> {
    let p = rsp(s, 0x0002, &short_le(ABSENT))?;
    expect(b(&p, 0) == DEVICE_NOT_FOUND, format!("status {:#04x}, DEVICE_NOT_FOUND is 0x81", b(&p, 0)))?;
    expect(p.len() == 3, format!("{} octets follow the address; the descriptor must not be included", p.len() as i64 - 3))?;
    Ok(String::new())
}

fn node_desc_revision(s: &mut Session) -> R<String> {
    let p = rsp(s, 0x0002, &short_le(s.dut_short))?;
    expect(b(&p, 0) == SUCCESS && p.len() >= 16, format!("status {:#04x}", b(&p, 0)))?;
    let mask = u16_at(&p, 3 + 8);
    let rev = mask >> 9;
    expect(rev >= 21, format!("stack compliance revision {rev} (server mask {mask:#06x}); Zigbee 3.0 needs R21 or later"))?;
    Ok(format!("revision {rev}"))
}

fn power_desc(s: &mut Session) -> R<String> {
    let p = rsp(s, 0x0003, &short_le(s.dut_short))?;
    expect(b(&p, 0) == SUCCESS, format!("status {:#04x}", b(&p, 0)))?;
    expect(p.len() == 5, format!("{} octets of power descriptor, 2 required", p.len() as i64 - 3))?;
    Ok(format!("power descriptor {}", hex(sl(&p, 3, p.len()))))
}

fn active_ep(s: &mut Session) -> R<String> {
    let p = rsp(s, 0x0005, &short_le(s.dut_short))?;
    expect(b(&p, 0) == SUCCESS, format!("status {:#04x}", b(&p, 0)))?;
    let n = b(&p, 3) as usize;
    let eps = sl(&p, 4, 4 + n).to_vec();
    expect(eps.len() == n && n > 0, format!("count {n}, list {}", py_list(&eps)))?;
    let mut sorted = eps.clone();
    sorted.sort();
    expect(eps == sorted, format!("endpoint list {} is not ascending", py_list(&eps)))?;
    Ok(format!("endpoints {}", py_list(&eps)))
}

fn simple_desc(s: &mut Session) -> R<String> {
    let mut q = short_le(s.dut_short);
    q.push(1);
    let p = rsp(s, 0x0004, &q)?;
    expect(b(&p, 0) == SUCCESS, format!("status {:#04x}", b(&p, 0)))?;
    expect(
        b(&p, 3) as i64 == p.len() as i64 - 4 && b(&p, 3) >= 8,
        format!("length field {}, {} octets follow", b(&p, 3), p.len() as i64 - 4),
    )?;
    let d = sl(&p, 4, p.len()).to_vec();
    expect(d[0] == 1, format!("descriptor is for endpoint {}", d[0]))?;
    let nin = b(&d, 6) as usize;
    let nout = b(&d, 7 + 2 * nin) as usize;
    expect(d.len() == 8 + 2 * nin + 2 * nout, "cluster counts do not add up to the descriptor length")?;
    Ok(format!("profile {:#06x} device {:#06x}", u16_at(&d, 1), u16_at(&d, 3)))
}

fn simple_desc_invalid(s: &mut Session) -> R<String> {
    let mut obs: Vec<String> = Vec::new();
    for ep in [0u8, 255] {
        let mut q = short_le(s.dut_short);
        q.push(ep);
        let p = rsp(s, 0x0004, &q)?;
        obs.push(format!("endpoint {ep}: status {:#04x}", b(&p, 0))
            + &(if p.len() > 3 { format!(", length {}", p[3]) } else { String::new() }));
        expect(b(&p, 0) == INVALID_EP, obs.join("; ") + " (INVALID_EP is 0x82)")?;
        expect(p.len() == 4 && p[3] == 0, obs.join("; ") + " (length must be 0 and no descriptor)")?;
    }
    Ok(String::new())
}

fn simple_desc_inactive(s: &mut Session) -> R<String> {
    let eps = s.endpoints()?;
    let ep = *[99u8, 100, 101, 2, 5].iter().find(|e| !eps.contains(e)).unwrap();
    let mut q = short_le(s.dut_short);
    q.push(ep);
    let p = rsp(s, 0x0004, &q)?;
    expect(b(&p, 0) == NOT_ACTIVE, format!("endpoint {ep}: status {:#04x}, NOT_ACTIVE is 0x83", b(&p, 0)))?;
    let len = if p.len() > 3 { p[3].to_string() } else { "missing".into() };
    expect(p.len() == 4 && p[3] == 0, format!("length {len}; must be 0 with no descriptor"))?;
    Ok(String::new())
}

fn simple_desc_other(s: &mut Session) -> R<String> {
    let mut q = short_le(ABSENT);
    q.push(1);
    let p = rsp(s, 0x0004, &q)?;
    expect(b(&p, 0) == DEVICE_NOT_FOUND, format!("status {:#04x}, DEVICE_NOT_FOUND is 0x81", b(&p, 0)))?;
    Ok(String::new())
}

fn match_req(addr: u16, profile: u16, ins: &[u16], outs: &[u16]) -> Vec<u8> {
    let mut v = addr.to_le_bytes().to_vec();
    v.extend_from_slice(&profile.to_le_bytes());
    v.push(ins.len() as u8);
    for c in ins {
        v.extend_from_slice(&c.to_le_bytes());
    }
    v.push(outs.len() as u8);
    for c in outs {
        v.extend_from_slice(&c.to_le_bytes());
    }
    v
}

fn match_desc(s: &mut Session) -> R<String> {
    let sd = s.simple_desc(1)?;
    let Some(sd) = sd.filter(|d| !d.inc.is_empty()) else {
        return Err(Stop::Inconclusive("endpoint 1 has no simple descriptor with input clusters".into()));
    };
    let p = rsp(s, 0x0006, &match_req(s.dut_short, sd.profile, &[sd.inc[0]], &[]))?;
    expect(b(&p, 0) == SUCCESS, format!("status {:#04x}", b(&p, 0)))?;
    let n = b(&p, 3) as usize;
    let lst = sl(&p, 4, 4 + n).to_vec();
    expect(lst.contains(&1), format!("match list {} lacks endpoint 1 (cluster {:#06x})", py_list(&lst), sd.inc[0]))?;
    Ok(String::new())
}

fn match_desc_none(s: &mut Session) -> R<String> {
    let p = rsp(s, 0x0006, &match_req(s.dut_short, 0x0104, &[0x0B04], &[]))?;
    expect(b(&p, 0) == SUCCESS, format!("status {:#04x}; SUCCESS with an empty list is required", b(&p, 0)))?;
    let ml = if p.len() > 3 { p[3].to_string() } else { "missing".into() };
    expect(p.len() >= 4 && p[3] == 0, format!("MatchLength {ml}"))?;
    Ok(String::new())
}

fn match_desc_broadcast(s: &mut Session) -> R<String> {
    let sd = s.simple_desc(1)?.unwrap();
    let f = s.zdo_with(0x0006, &match_req(0xFFFD, sd.profile, &[sd.inc[0]], &[]), Tx::new().dst(0xFFFD))?;
    let Some(f) = f else { return Err(Stop::Fail("no response to a matching broadcast request".into())) };
    expect(
        f.nwk.as_ref().unwrap().dst == 0x0000 && f.zdo().unwrap().payload.first() == Some(&SUCCESS),
        format!("response {}", f.brief()),
    )?;
    let g = s.zdo_with(0x0006, &match_req(0xFFFD, 0x0104, &[0x0B04], &[]), Tx::new().dst(0xFFFD).max_ms(1500.0))?;
    expect(
        g.is_none(),
        format!("a broadcast request with no match was answered: {}", g.as_ref().map(|g| g.brief()).unwrap_or_default()),
    )?;
    Ok(String::new())
}

fn qbb(ext: u64, a: u8, c: u8) -> Vec<u8> {
    let mut v = ext.to_le_bytes().to_vec();
    v.push(a);
    v.push(c);
    v
}

fn nwk_addr(s: &mut Session) -> R<String> {
    let p = rsp(s, 0x0000, &qbb(s.dut_ext, 0, 0))?;
    expect(b(&p, 0) == SUCCESS, format!("status {:#04x}", b(&p, 0)))?;
    let (ext, short) = (u64_at(&p, 1), u16_at(&p, 9));
    expect((ext, short) == (s.dut_ext, s.dut_short), format!("addresses {ext:#018x}/{short:#06x}"))?;
    expect(p.len() == 11, format!("{} extra octets after the addresses in a single device response", p.len() as i64 - 11))?;
    Ok(String::new())
}

fn nwk_addr_unknown(s: &mut Session) -> R<String> {
    let f = s.zdo(0x0000, &qbb(ABSENT_EXT, 0, 0), 2000.0)?;
    let Some(f) = f else {
        return Err(Stop::Fail("no response to a unicast request for an unknown IEEE address".into()));
    };
    let p = f.zdo().unwrap().payload;
    expect(b(&p, 0) == DEVICE_NOT_FOUND, format!("status {:#04x}, DEVICE_NOT_FOUND is 0x81", b(&p, 0)))?;
    let (ext, short) = (u64_at(&p, 1), u16_at(&p, 9));
    expect(
        ext == ABSENT_EXT && short == 0xFFFF,
        format!("IEEEAddrRemoteDev {ext:#018x}, NWKAddrRemoteDev {short:#06x}"),
    )?;
    Ok(String::new())
}

fn nwk_addr_broadcast(s: &mut Session) -> R<String> {
    let f = s.zdo_with(0x0000, &qbb(s.dut_ext, 0, 0), Tx::new().dst(0xFFFD))?;
    let Some(f) = f else {
        return Err(Stop::Fail("no response to a broadcast request for the device's own IEEE address".into()));
    };
    expect(
        f.zdo().unwrap().payload.first() == Some(&SUCCESS) && f.nwk.as_ref().unwrap().dst == 0x0000,
        f.brief(),
    )?;
    let g = s.zdo_with(0x0000, &qbb(ABSENT_EXT, 0, 0), Tx::new().dst(0xFFFD).max_ms(1500.0))?;
    expect(
        g.is_none(),
        format!(
            "a broadcast request for an unknown address was answered: {}",
            g.as_ref().map(|g| g.brief()).unwrap_or_default()
        ),
    )?;
    Ok(String::new())
}

fn nwk_addr_reqtype(s: &mut Session) -> R<String> {
    let p = rsp(s, 0x0000, &qbb(s.dut_ext, 0x07, 0))?;
    expect(b(&p, 0) == INV_REQUESTTYPE, format!("status {:#04x}, INV_REQUESTTYPE is 0x80", b(&p, 0)))?;
    Ok(String::new())
}

fn hbb(a: u16, x: u8, y: u8) -> Vec<u8> {
    let mut v = a.to_le_bytes().to_vec();
    v.push(x);
    v.push(y);
    v
}

fn ieee_addr(s: &mut Session) -> R<String> {
    let p = rsp(s, 0x0001, &hbb(s.dut_short, 0, 0))?;
    expect(b(&p, 0) == SUCCESS, format!("status {:#04x}", b(&p, 0)))?;
    let (ext, short) = (u64_at(&p, 1), u16_at(&p, 9));
    expect((ext, short) == (s.dut_ext, s.dut_short) && p.len() == 11, hex(&p))?;
    let f = s.zdo(0x0001, &hbb(ABSENT, 0, 0), 2000.0)?;
    let Some(f) = f else {
        return Err(Stop::Fail(
            "own address answered with SUCCESS, but no response at all for an unknown address".into(),
        ));
    };
    let p = f.zdo().unwrap().payload;
    expect(b(&p, 0) == DEVICE_NOT_FOUND, format!("unknown address: status {:#04x}, DEVICE_NOT_FOUND is 0x81", b(&p, 0)))?;
    let (ext, short) = (u64_at(&p, 1), u16_at(&p, 9));
    expect(
        ext == 0xFFFFFFFFFFFFFFFF && short == ABSENT,
        format!("unknown address: IEEE {ext:#018x}, NWK {short:#06x}"),
    )?;
    Ok(String::new())
}

fn nwk_addr_extended(s: &mut Session) -> R<String> {
    let p = rsp(s, 0x0000, &qbb(s.dut_ext, 1, 0))?;
    expect(b(&p, 0) == SUCCESS, format!("status {:#04x}", b(&p, 0)))?;
    let (ext, short) = (u64_at(&p, 1), u16_at(&p, 9));
    expect((ext, short) == (s.dut_ext, s.dut_short), format!("addresses {ext:#018x}/{short:#06x}"))?;
    Ok(String::new())
}

/// A binding (unicast to an extended address) of the bulb's endpoint 1.
fn bind_payload(s: &Session, cluster: u16, dst_ext: Option<u64>, dst_ep: u8) -> Vec<u8> {
    let mut v = s.dut_ext.to_le_bytes().to_vec();
    v.push(1);
    v.extend_from_slice(&cluster.to_le_bytes());
    v.push(0x03);
    v.extend_from_slice(&dst_ext.unwrap_or(s.w.coord.ext).to_le_bytes());
    v.push(dst_ep);
    v
}

/// (src ext, src ep, cluster, mode, dst, dst ep)
type Entry = (u64, u8, u16, u8, u64, Option<u8>);

fn entry_py(e: &Entry) -> String {
    let dep = e.5.map(|d| d.to_string()).unwrap_or_else(|| "None".into());
    format!("({}, {}, {}, {}, {}, {dep})", e.0, e.1, e.2, e.3, e.4)
}

fn table_py(t: &[Entry]) -> String {
    format!("[{}]", t.iter().map(entry_py).collect::<Vec<_>>().join(", "))
}

/// The binding table as raw entries.
fn mgmt_bind(s: &mut Session) -> R<(u8, Vec<Entry>)> {
    let mut out = Vec::new();
    let mut start: usize = 0;
    loop {
        let p = rsp(s, 0x0033, &[start as u8])?;
        if b(&p, 0) != SUCCESS {
            return Ok((b(&p, 0), out));
        }
        let (total, cnt) = (b(&p, 1) as usize, b(&p, 3) as usize);
        let mut i = 4;
        for _ in 0..cnt {
            let (src, sep, cl, mode) = (u64_at(&p, i), b(&p, i + 8), u16_at(&p, i + 9), b(&p, i + 11));
            i += 12;
            let (dst, dep) = if mode == 1 {
                let d = u16_at(&p, i) as u64;
                i += 2;
                (d, None)
            } else {
                let d = (u64_at(&p, i), Some(b(&p, i + 8)));
                i += 9;
                d
            };
            out.push((src, sep, cl, mode, dst, dep));
        }
        start += cnt;
        if start >= total || cnt == 0 {
            return Ok((SUCCESS, out));
        }
    }
}

fn bind(s: &mut Session) -> R<String> {
    let p = rsp(s, 0x0021, &bind_payload(s, 0x0008, None, 1))?;
    expect(b(&p, 0) == SUCCESS, format!("Bind_rsp status {:#04x}", b(&p, 0)))?;
    let (st, table) = mgmt_bind(s)?;
    if st == NOT_SUPPORTED {
        rsp(s, 0x0022, &bind_payload(s, 0x0008, None, 1))?;
        return Err(Stop::Inconclusive(
            "Bind_rsp SUCCESS; Mgmt_Bind_req is optional and answered NOT_SUPPORTED, so the entry could not be read back"
                .into(),
        ));
    }
    expect(st == SUCCESS, format!("Mgmt_Bind_rsp status {st:#04x}"))?;
    let want: Entry = (s.dut_ext, 1, 0x0008, 3, s.w.coord.ext, Some(1));
    expect(table.contains(&want), format!("binding table {} lacks the entry just created", table_py(&table)))?;
    let p = rsp(s, 0x0022, &bind_payload(s, 0x0008, None, 1))?;
    expect(b(&p, 0) == SUCCESS, format!("Unbind_rsp status {:#04x}", b(&p, 0)))?;
    let (_, table) = mgmt_bind(s)?;
    expect(!table.contains(&want), "the entry is still reported after Unbind_rsp SUCCESS")?;
    Ok(String::new())
}

fn unbind_missing(s: &mut Session) -> R<String> {
    let p = rsp(s, 0x0022, &bind_payload(s, 0x0B05, Some(ABSENT_EXT), 7))?;
    expect(b(&p, 0) == NO_ENTRY, format!("status {:#04x}, NO_ENTRY is 0x88", b(&p, 0)))?;
    Ok(String::new())
}

fn bind_broadcast(s: &mut Session) -> R<String> {
    let pl = bind_payload(s, 0x0300, Some(ABSENT_EXT), 9);
    let f = s.zdo_with(0x0021, &pl, Tx::new().dst(0xFFFD).max_ms(1500.0))?;
    let (st, table) = mgmt_bind(s)?;
    let made = table.iter().any(|e| e.2 == 0x0300 && e.4 == ABSENT_EXT);
    if made {
        rsp(s, 0x0022, &pl)?;
    }
    expect(
        f.is_none(),
        format!("a broadcast Bind_req was answered ({})", f.as_ref().map(|f| f.brief()).unwrap_or_default())
            + if made { "; the binding was created" } else { "" },
    )?;
    if st == SUCCESS {
        expect(!made, "a broadcast Bind_req created a binding table entry (no response was sent)")?;
    }
    Ok(String::new())
}

fn mgmt_bind_empty(s: &mut Session) -> R<String> {
    let (st, table) = mgmt_bind(s)?;
    if st == NOT_SUPPORTED {
        return Err(Stop::NotApplicable("the optional Mgmt_Bind_req is answered NOT_SUPPORTED".into()));
    }
    for e in &table {
        // leftovers of other tests in this session
        let mut pl = e.0.to_le_bytes().to_vec();
        pl.push(e.1);
        pl.extend_from_slice(&e.2.to_le_bytes());
        if e.3 == 1 {
            pl.push(1);
            pl.extend_from_slice(&(e.4 as u16).to_le_bytes());
        } else {
            pl.push(3);
            pl.extend_from_slice(&e.4.to_le_bytes());
            pl.push(e.5.unwrap_or(0));
        }
        rsp(s, 0x0022, &pl)?;
    }
    let p = rsp(s, 0x0033, &[0x00])?;
    expect(b(&p, 0) == SUCCESS, format!("status {:#04x}", b(&p, 0)))?;
    expect(sl(&p, 1, 4) == [0, 0, 0] && p.len() == 4, format!("empty table reported as {}", hex(&p)))?;
    Ok(String::new())
}

fn mgmt_lqi(s: &mut Session) -> R<String> {
    // (epid, ext, short, type)
    let mut entries: Vec<(u64, u64, u16, u8)> = Vec::new();
    let mut start: usize = 0;
    loop {
        let p = rsp(s, 0x0031, &[start as u8])?;
        expect(b(&p, 0) == SUCCESS, format!("status {:#04x}", b(&p, 0)))?;
        let (total, cnt) = (b(&p, 1) as usize, b(&p, 3) as usize);
        expect(
            p.len() == 4 + 22 * cnt,
            format!("{cnt} entries announced, {} octets follow (22 per entry)", p.len() as i64 - 4),
        )?;
        for k in 0..cnt {
            let e = sl(&p, 4 + 22 * k, 26 + 22 * k);
            entries.push((u64_at(e, 0), u64_at(e, 8), u16_at(e, 16), b(e, 18) & 3));
        }
        start += cnt;
        if start >= total || cnt == 0 {
            break;
        }
    }
    let tc: Vec<_> = entries.iter().filter(|e| e.2 == 0x0000).collect();
    let shorts = format!("[{}]", entries.iter().map(|e| format!("'{:#x}'", e.2)).collect::<Vec<_>>().join(", "));
    expect(!tc.is_empty(), format!("the coordinator is not in the neighbour table: {shorts}"))?;
    let e = tc[0];
    expect(e.0 == s.w.coord.ext_pan, format!("extended PAN id {:#018x} in the coordinator's entry", e.0))?;
    expect(e.1 == s.w.coord.ext, format!("extended address {:#018x} in the coordinator's entry", e.1))?;
    expect(e.3 == 0, format!("device type {} for the coordinator (0 required)", e.3))?;
    Ok(format!("{} entries", entries.len()))
}

fn mgmt_rtg(s: &mut Session) -> R<String> {
    let p = rsp(s, 0x0032, &[0x00])?;
    if b(&p, 0) == NOT_SUPPORTED {
        return Err(Stop::NotApplicable("the optional Mgmt_Rtg_req is answered NOT_SUPPORTED".into()));
    }
    expect(b(&p, 0) == SUCCESS, format!("status {:#04x}", b(&p, 0)))?;
    expect(
        p.len() == 4 + 5 * b(&p, 3) as usize,
        format!("{} entries announced, {} octets follow (5 per entry)", b(&p, 3), p.len() as i64 - 4),
    )?;
    Ok(format!("SUCCESS, {} entries", b(&p, 3)))
}

fn permit_joining(s: &mut Session) -> R<String> {
    let f = s.zdo(0x0036, &[3, 1], 2000.0)?;
    let Some(f) = f else { return Err(Stop::Fail("no Mgmt_Permit_Joining_rsp to a unicast request".into())) };
    let st = b(&f.zdo().unwrap().payload, 0);
    expect(st == SUCCESS, format!("status {st:#04x}"))?;
    let bc = s.beacon()?;
    let Some(bc) = bc else {
        return Err(Stop::Inconclusive("the bulb sent no beacon in answer to a Beacon Request".into()));
    };
    expect(bc.permit, "association permit is 0 in the beacon right after PermitDuration = 3")?;
    s.run(4500.0)?;
    let bc = s.beacon()?;
    expect(
        bc.as_ref().is_some_and(|b| !b.permit),
        "association permit still 1 in the beacon 4.5 s after PermitDuration = 3",
    )?;
    s.zdo(0x0036, &[60, 1], 2000.0)?;
    let bc = s.beacon()?;
    expect(bc.as_ref().is_some_and(|b| b.permit), "association permit is 0 after PermitDuration = 60")?;
    let f = s.zdo(0x0036, &[0, 1], 2000.0)?;
    expect(
        f.as_ref().is_some_and(|f| f.zdo().unwrap().payload.first() == Some(&SUCCESS)),
        "no SUCCESS response to PermitDuration = 0",
    )?;
    let bc = s.beacon()?;
    expect(bc.as_ref().is_some_and(|b| !b.permit), "association permit still 1 after PermitDuration = 0")?;
    s.set_note("permit_open", false);
    Ok(String::new())
}

fn permit_joining_broadcast(s: &mut Session) -> R<String> {
    let f = s.zdo_with(0x0036, &[30, 1], Tx::new().dst(0xFFFC).max_ms(1500.0))?;
    let bc = s.beacon()?;
    s.zdo(0x0036, &[0, 1], 2000.0)?;
    s.set_note("permit_open", false);
    expect(
        f.is_none(),
        format!("a broadcast request was answered: {}", f.as_ref().map(|f| f.brief()).unwrap_or_default()),
    )?;
    let Some(bc) = bc else {
        return Err(Stop::Inconclusive("the bulb sent no beacon in answer to a Beacon Request".into()));
    };
    expect(bc.permit, "association permit is 0 in the beacon after a broadcast PermitDuration = 30")?;
    Ok(String::new())
}

fn nwk_update_scan(s: &mut Session) -> R<String> {
    let chans: u32 = (1 << 11) | (1 << 15) | (1 << 20);
    let mut q = chans.to_le_bytes().to_vec();
    q.extend_from_slice(&[1, 1]);
    let f = s.zdo(0x0038, &q, 6000.0)?;
    let Some(f) = f else { return Err(Stop::Fail("no Mgmt_NWK_Update_notify after an energy scan request".into())) };
    let p = f.zdo().unwrap().payload;
    if b(&p, 0) == NOT_SUPPORTED {
        return Err(Stop::NotApplicable("the optional Mgmt_NWK_Update_req is answered NOT_SUPPORTED".into()));
    }
    expect(b(&p, 0) == SUCCESS, format!("status {:#04x}", b(&p, 0)))?;
    let scanned = u32_at(&p, 1);
    let cnt = b(&p, 9) as usize;
    expect(scanned & 0x07FFFFFF == chans, format!("ScannedChannels {scanned:#010x}, requested {chans:#010x}"))?;
    expect(
        cnt == 3 && p.len() == 10 + cnt,
        format!("ScannedChannelsListCount {cnt}, {} energy values, 3 channels", p.len() as i64 - 10),
    )?;
    Ok(format!("energy values {}", py_list(sl(&p, 10, p.len()))))
}

fn nwk_update_invalid(s: &mut Session) -> R<String> {
    let mut q = (1u32 << 11).to_le_bytes().to_vec();
    q.extend_from_slice(&[0x06, 1]);
    let f = s.zdo(0x0038, &q, 3000.0)?;
    let Some(f) = f else {
        return Err(Stop::Fail("no Mgmt_NWK_Update_notify to an invalid ScanDuration (0x06)".into()));
    };
    let p = f.zdo().unwrap().payload;
    if b(&p, 0) == NOT_SUPPORTED {
        return Err(Stop::NotApplicable("the optional Mgmt_NWK_Update_req is answered NOT_SUPPORTED".into()));
    }
    expect(
        b(&p, 0) == 0x80 || b(&p, 0) == 0xC2,
        format!("status {:#04x}; INVALID_REQUEST is required (0x80 or 0xC2 accepted)", b(&p, 0)),
    )?;
    Ok(String::new())
}
