use crate::ble::harness::{hx, ll_name, BleSession, LinkOpts, Pdu};
use crate::ble::mesh::MeshApp;
use crate::ble::BleTest;
use crate::session::{expect, Stop, R};

const DOC: &str = "Core 5.4 Vol 6 Part B";
const UNKNOWN_RSP: u8 = 0x07;
const REJECT: u8 = 0x0D;
const REJECT_EXT: u8 = 0x11;
const OP_MESH_RESET: u8 = 0xE3;

pub fn tests() -> Vec<BleTest> {
    vec![
        BleTest {
            id: "ll_ctrl_version_exchange",
            doc: DOC,
            clause: "5.1.5, 2.4.2.13",
            req: "An LL_VERSION_IND from the central is answered with one LL_VERSION_IND (VersNr, CompId, SubVersNr: \
                  five octets of CtrData).",
            run: version_exchange,
            ..BleTest::D
        },
        BleTest {
            id: "ll_ctrl_version_once",
            doc: DOC,
            clause: "5.1.5",
            req: "A link layer that has sent its LL_VERSION_IND does not send another one in the same connection, \
                  whatever it receives.",
            run: version_once,
            ..BleTest::D
        },
        BleTest {
            id: "ll_ctrl_feature_rsp",
            doc: DOC,
            clause: "5.1.4.1, 2.4.2.10",
            req: "LL_FEATURE_REQ is answered with LL_FEATURE_RSP (eight octets of FeatureSet).",
            run: feature_rsp,
            ..BleTest::D
        },
        BleTest {
            id: "ll_ctrl_feature_used",
            doc: DOC,
            clause: "5.1.4, 5.1.4.1",
            req: "Octet 0 of the FeatureSet in LL_FEATURE_RSP is FeatureSet_USED: the AND of the central's and the \
                  peripheral's least significant octets.",
            run: feature_used,
            ..BleTest::D
        },
        BleTest {
            id: "ll_ctrl_unknown_opcode",
            doc: DOC,
            clause: "2.4.2, 2.4.2.8",
            req: "An LL control PDU with a reserved opcode is answered with LL_UNKNOWN_RSP whose UnknownType is that \
                  opcode.",
            run: unknown_opcode,
            ..BleTest::D
        },
        BleTest {
            id: "ll_ctrl_ping",
            doc: DOC,
            clause: "5.1.8, 4.6 (Table 4.7), 2.4.2",
            req: "LL_PING_REQ is answered with LL_PING_RSP, or with LL_UNKNOWN_RSP by a device without LE Ping.",
            run: ping,
            ..BleTest::D
        },
        BleTest {
            id: "ll_ctrl_length_req",
            doc: DOC,
            clause: "5.1.9, 2.4.2.21, 4.6",
            req: "LL_LENGTH_REQ is answered with LL_LENGTH_RSP (octets >= 27, times >= 328 us) by a device that \
                  claims LE Data Packet Length Extension, with LL_UNKNOWN_RSP by one that does not.",
            run: length_req,
            ..BleTest::D
        },
        BleTest {
            id: "ll_ctrl_own_length_req",
            doc: DOC,
            clause: "5.1.9, 2.4.2.21, 4.6",
            req: "A device that starts the Data Length Update procedure itself claims the feature and sends legal \
                  values.",
            run: own_length_req,
            mandatory: false,
            ..BleTest::D
        },
        BleTest {
            id: "ll_ctrl_phy_req",
            doc: DOC,
            clause: "5.1.10, 4.6.9, 2.4.2",
            req: "LL_PHY_REQ is answered with LL_PHY_RSP by a device with another PHY than LE 1M, with LL_UNKNOWN_RSP \
                  by a device with LE 1M only.",
            run: phy_req,
            ..BleTest::D
        },
        BleTest {
            id: "ll_ctrl_conn_param_req",
            doc: DOC,
            clause: "5.1.7, 4.6.2, 2.4.2",
            req: "LL_CONNECTION_PARAM_REQ is answered with LL_CONNECTION_PARAM_RSP or a reject by a device that \
                  claims the procedure, with LL_UNKNOWN_RSP by one that does not.",
            run: conn_param_req,
            ..BleTest::D
        },
        BleTest {
            id: "ll_ctrl_other_optional",
            doc: DOC,
            clause: "2.4.2, 4.6",
            req: "Requests of procedures whose feature bit is not set (CTE, clock accuracy, power control) are \
                  answered with LL_UNKNOWN_RSP naming the opcode.",
            run: other_optional,
            ..BleTest::D
        },
        BleTest {
            id: "ll_ctrl_wrong_length",
            doc: DOC,
            clause: "2.4.2",
            req: "A known control PDU with the wrong length is either processed or answered with LL_UNKNOWN_RSP \
                  naming its opcode; it is not left without an answer.",
            run: wrong_length,
            ..BleTest::D
        },
        BleTest {
            id: "ll_ctrl_enc_req",
            doc: DOC,
            clause: "5.1.3.1, 4.6.1",
            req: "LL_ENC_REQ: a device without LE Encryption rejects it with Unsupported Remote Feature (0x1A); a \
                  device that claims it answers LL_ENC_RSP and, having no key, rejects with PIN or Key Missing \
                  (0x06).",
            run: enc_req,
            ..BleTest::D
        },
        BleTest {
            id: "ll_ctrl_terminate",
            doc: DOC,
            clause: "5.1.6, 4.5.12",
            req: "LL_TERMINATE_IND is acknowledged and the peripheral leaves the Connection state: nothing more is \
                  received or sent on the data channels.",
            run: terminate,
            ambiguous: Some(
                "the clause gives no time for leaving the Connection state after the acknowledgment; a receive \
                 window in the one connection event that follows is reported, not failed; windows in later events \
                 are a FAIL",
            ),
            ..BleTest::D
        },
        BleTest {
            id: "ll_ctrl_peripheral_terminate",
            doc: DOC,
            clause: "5.1.6, 2.4.2.3",
            req: "A peripheral that ends the connection sends LL_TERMINATE_IND (one octet of error code) and leaves \
                  once it is acknowledged.",
            run: peripheral_terminate,
            ..BleTest::D
        },
        BleTest {
            id: "ll_ctrl_peripheral_terminate_timer",
            doc: DOC,
            clause: "5.1.6",
            req: "A peripheral whose LL_TERMINATE_IND is not acknowledged keeps sending it until T_terminate \
                  (connSupervisionTimeout) expires, then leaves.",
            run: peripheral_terminate_timer,
            ..BleTest::D
        },
        BleTest {
            id: "ll_ctrl_procedure_timeout",
            doc: DOC,
            clause: "5.2",
            req: "A peripheral whose own control procedure gets no response considers the connection lost after \
                  40 s.",
            run: procedure_timeout,
            mandatory: false,
            ..BleTest::D
        },
    ]
}

fn pl(p: &Pdu) -> &[u8] {
    p.payload()
}

fn le(b: &[u8], i: usize) -> u64 {
    let s = b.get(i..(i + 2).min(b.len())).unwrap_or(&[]);
    s.iter().enumerate().map(|(k, x)| (*x as u64) << (8 * k)).sum()
}

fn feature_set(p: &Pdu) -> u64 {
    let b = pl(p).get(1..9).unwrap_or(&[]);
    b.iter().enumerate().map(|(k, x)| (*x as u64) << (8 * k)).sum()
}

fn len_i(p: &Pdu) -> i64 {
    p.length() as i64
}

fn at(p: &Pdu, i: usize) -> R<u8> {
    pl(p).get(i).copied().ok_or_else(|| Stop::Error("IndexError: index out of range".into()))
}

fn last(p: &Pdu) -> R<u8> {
    pl(p).last().copied().ok_or_else(|| Stop::Error("IndexError: index out of range".into()))
}

fn op(p: &Pdu) -> u8 {
    p.opcode().unwrap_or(0)
}

fn features(s: &mut BleSession) -> R<u64> {
    let mut q = vec![0x08];
    q.extend_from_slice(&[0xff; 8]);
    let (rsp, _) = s.ll_req(&q, &[0x09], 3000.0)?;
    match rsp {
        Some(r) if r.length() == 9 => Ok(feature_set(&r)),
        r => Err(Stop::Inconclusive(format!(
            "no LL_FEATURE_RSP ({})",
            r.map(|p| p.repr()).unwrap_or_else(|| "None".into())
        ))),
    }
}

fn names(pdus: &[Pdu]) -> Vec<String> {
    pdus.iter()
        .map(|p| {
            let o = op(p);
            let n = ll_name(o).map(String::from).unwrap_or_else(|| format!("{o:#x}"));
            format!("{n} {}", hx(pl(p).get(1..).unwrap_or(&[])))
        })
        .collect()
}

fn strlist(v: &[String]) -> String {
    format!("[{}]", v.iter().map(|x| format!("'{x}'")).collect::<Vec<_>>().join(", "))
}

fn names_or(pdus: &[Pdu], alt: &str) -> String {
    let n = names(pdus);
    if n.is_empty() {
        alt.to_string()
    } else {
        strlist(&n)
    }
}

fn version_exchange(s: &mut BleSession) -> R<String> {
    s.connect(LinkOpts::new())?;
    s.link_run(100.0)?;
    let mine: Vec<Pdu> = s.lk().ctrl.iter().filter(|p| p.opcode() == Some(0x0C)).cloned().collect();
    let (mut rsp, _) = s.ll_req(&[0x0c, 0x0d, 0x0f, 0x00, 0x34, 0x12], &[0x0C], 3000.0)?;
    s.link_run(300.0)?;
    let got = s.lk().ctrl.iter().filter(|p| p.opcode() == Some(0x0C)).count();
    if let Some(m) = mine.first() {
        expect(got == 1, format!("the bulb sent its LL_VERSION_IND first and then {} more", got as i64 - 1))?;
        rsp = Some(m.clone());
    }
    expect(rsp.is_some(), "no LL_VERSION_IND in answer to the central's")?;
    let rsp = rsp.unwrap();
    expect(
        rsp.length() == 6,
        format!("LL_VERSION_IND with {} octets of CtrData: {}", len_i(&rsp) - 1, hx(pl(&rsp))),
    )?;
    expect(got == 1, format!("{got} LL_VERSION_IND PDUs from the bulb"))?;
    let (v, comp, sub) = (at(&rsp, 1)?, le(pl(&rsp), 2), le(pl(&rsp), 4));
    Ok(format!("VersNr {v:#04x}, CompId {comp:#06x}, SubVersNr {sub:#06x}"))
}

fn version_once(s: &mut BleSession) -> R<String> {
    s.connect(LinkOpts::new())?;
    let ver = [0x0c, 0x0d, 0x0f, 0x00, 0x34, 0x12];
    let (rsp, _) = s.ll_req(&ver, &[0x0C], 3000.0)?;
    if rsp.is_none() {
        return Err(Stop::Inconclusive("no LL_VERSION_IND at all".into()));
    }
    for _ in 0..3 {
        s.ll(&ver)?;
        s.link_run(400.0)?;
    }
    let got = s.lk().ctrl.iter().filter(|p| p.opcode() == Some(0x0C)).count();
    expect(got == 1, format!("{got} LL_VERSION_IND PDUs from the bulb in one connection"))?;
    let alive = s.alive(3000.0)?;
    expect(alive, "the connection did not survive a repeated LL_VERSION_IND")?;
    let others: Vec<Pdu> = s.lk().ctrl.iter().filter(|p| p.opcode() != Some(0x0C)).cloned().collect();
    let n = names(&others).join(", ");
    Ok(format!("one LL_VERSION_IND; answers to the repeats: {}", if n.is_empty() { "none".into() } else { n }))
}

fn feature_rsp(s: &mut BleSession) -> R<String> {
    s.connect(LinkOpts::new())?;
    let mut q = vec![0x08];
    q.extend_from_slice(&[0xff; 8]);
    let (rsp, allp) = s.ll_req(&q, &[0x09, UNKNOWN_RSP], 3000.0)?;
    expect(
        rsp.as_ref().is_some_and(|r| r.opcode() == Some(0x09)),
        format!("LL_FEATURE_REQ answered with {}", names_or(&allp, "nothing")),
    )?;
    let rsp = rsp.unwrap();
    expect(rsp.length() == 9, format!("LL_FEATURE_RSP with {} octets of FeatureSet", len_i(&rsp) - 1))?;
    Ok(format!("FeatureSet {:#018x}", feature_set(&rsp)))
}

fn feature_used(s: &mut BleSession) -> R<String> {
    let mut seen: Vec<(u8, Vec<u8>)> = Vec::new();
    for c0 in [0xFFu8, 0x00, 0x01, 0x24] {
        s.connect(LinkOpts::new())?;
        let mut q = vec![0x08, c0];
        q.extend_from_slice(&[0; 7]);
        let (rsp, _) = s.ll_req(&q, &[0x09], 3000.0)?;
        let Some(rsp) = rsp.filter(|r| r.length() == 9) else {
            return Err(Stop::Inconclusive("no LL_FEATURE_RSP".into()));
        };
        seen.push((c0, pl(&rsp)[1..9].to_vec()));
        s.drop_link(6000.0)?;
    }
    let p0 = seen[0].1[0];
    let bad: Vec<String> = seen
        .iter()
        .filter(|(c0, v)| v[0] != p0 & c0)
        .map(|(c0, v)| format!("central octet 0 = {c0:#04x}: response octet 0 = {:#04x}, expected {:#04x}", v[0], p0 & c0))
        .collect();
    let mut rest: Vec<Vec<u8>> = Vec::new();
    for (_, v) in &seen {
        if !rest.contains(&v[1..].to_vec()) {
            rest.push(v[1..].to_vec());
        }
    }
    expect(
        rest.len() == 1,
        format!(
            "octets 1-7 of the FeatureSet change with the request: {}",
            strlist(&rest.iter().map(|r| hx(r)).collect::<Vec<_>>())
        ),
    )?;
    if p0 == 0 {
        return Ok("the peripheral's octet 0 is zero, so FeatureSet_USED is zero for any request (nothing to tell apart)"
            .into());
    }
    expect(bad.is_empty(), format!("FeatureSet_P octet 0 = {p0:#04x}; {}", bad.join("; ")))?;
    Ok(format!("FeatureSet_P octet 0 = {p0:#04x}, masked by the central's octet"))
}

fn unknown_opcode(s: &mut BleSession) -> R<String> {
    s.connect(LinkOpts::new())?;
    let mut bad = Vec::new();
    let cases: [(u8, Vec<u8>); 5] =
        [(0x2B, vec![]), (0x40, vec![1, 2, 3]), (0x7F, vec![0]), (0xFF, vec![]), (0xF0, vec![0; 8])];
    for (o, data) in &cases {
        let mut q = vec![*o];
        q.extend_from_slice(data);
        let (rsp, allp) = s.ll_req(&q, &[UNKNOWN_RSP], 2500.0)?;
        match rsp {
            None => bad.push(format!("opcode {o:#04x}: {}", names_or(&allp, "no answer"))),
            Some(r) if r.length() != 2 || pl(&r).get(1) != Some(o) => {
                bad.push(format!("opcode {o:#04x}: LL_UNKNOWN_RSP {}", hx(pl(&r))))
            }
            _ => {}
        }
    }
    expect(bad.is_empty(), bad.join("; "))?;
    let alive = s.alive(3000.0)?;
    expect(alive, "the connection ended")?;
    Ok("0x2b, 0x40, 0x7f, 0xff, 0xf0 each answered with LL_UNKNOWN_RSP".into())
}

fn optional(s: &mut BleSession, name: &str, opcode: u8, ctr: &[u8], bits: &[u32], ok_ops: &[u8], feat_note: &str)
            -> R<String> {
    s.connect(LinkOpts::new())?;
    s.link_run(200.0)?;
    let f = features(s)?;
    let claimed = bits.iter().any(|b| f >> b & 1 != 0);
    let bits_s = bits.iter().map(|b| b.to_string()).collect::<Vec<_>>().join("/");
    let mut q = vec![opcode];
    q.extend_from_slice(ctr);
    let mut ops = ok_ops.to_vec();
    ops.extend_from_slice(&[UNKNOWN_RSP, REJECT, REJECT_EXT]);
    let (rsp, allp) = s.ll_req(&q, &ops, 3000.0)?;
    let mut what = names(&allp);
    if what.is_empty() {
        what = vec!["no answer".into()];
    }
    expect(rsp.is_some(), format!("{name}: {} (FeatureSet {f:#x})", what[0]))?;
    let rsp = rsp.unwrap();
    if rsp.opcode() == Some(UNKNOWN_RSP) {
        expect(
            rsp.length() == 2 && pl(&rsp).get(1) == Some(&opcode),
            format!("{name}: LL_UNKNOWN_RSP {}", hx(pl(&rsp))),
        )?;
        expect(
            !claimed,
            format!("{name} answered with LL_UNKNOWN_RSP although FeatureSet {f:#x} claims bit {bits_s}"),
        )?;
        return Ok(format!("feature bit {bits_s} not set, LL_UNKNOWN_RSP"));
    }
    if rsp.opcode() == Some(REJECT_EXT) {
        expect(
            f >> 2 & 1 != 0,
            format!(
                "{name}: LL_REJECT_EXT_IND from a device whose FeatureSet {f:#x} lacks Extended Reject Indication (bit 2)"
            ),
        )?;
        expect(
            rsp.length() == 3 && pl(&rsp).get(1) == Some(&opcode),
            format!("{name}: LL_REJECT_EXT_IND {}", hx(pl(&rsp))),
        )?;
        let e = at(&rsp, 2)?;
        expect(
            claimed,
            format!(
                "{name}: rejected with error {e:#04x} although the feature bit is not set (an unsupported PDU is \
                 answered with LL_UNKNOWN_RSP)"
            ),
        )?;
        return Ok(format!("feature claimed, LL_REJECT_EXT_IND error {e:#04x}"));
    }
    if rsp.opcode() == Some(REJECT) {
        expect(false, format!("{name}: LL_REJECT_IND {} (not an answer this procedure allows)", hx(pl(&rsp))))?;
    }
    expect(
        claimed || !feat_note.is_empty(),
        format!(
            "{name} answered with {} although FeatureSet {f:#x} does not claim bit {bits_s}: 4.6 forbids sending a \
             PDU of a feature that is not supported",
            what[0]
        ),
    )?;
    Ok(format!("{}{}", what[0], if !claimed { format!(" ({feat_note})") } else { String::new() }))
}

fn ping(s: &mut BleSession) -> R<String> {
    optional(s, "LL_PING_REQ", 0x12, &[], &[4], &[0x13], "bit 4 may be sent as zero by a device that has the feature")
}

fn length_req(s: &mut BleSession) -> R<String> {
    let ctr = [0x1b, 0x00, 0x48, 0x01, 0x1b, 0x00, 0x48, 0x01];
    let mut note = optional(s, "LL_LENGTH_REQ", 0x14, &ctr, &[5], &[0x15], "")?;
    let rsp = s.lk().ctrl.iter().find(|p| p.opcode() == Some(0x15)).cloned();
    if let Some(rsp) = rsp {
        expect(rsp.length() == 9, format!("LL_LENGTH_RSP {}", hx(pl(&rsp))))?;
        let v: Vec<u64> = [1, 3, 5, 7].iter().map(|i| le(pl(&rsp), *i)).collect();
        expect(
            v[0] >= 27 && v[2] >= 27 && v[1] >= 328 && v[3] >= 328,
            format!("LL_LENGTH_RSP rx {} octets/{} us, tx {} octets/{} us: under the minimum", v[0], v[1], v[2], v[3]),
        )?;
        expect(
            v[1] <= 2128 && v[3] <= 2128,
            format!("LL_LENGTH_RSP times {}/{} us over 2128 to a peer without the Coded PHY", v[1], v[3]),
        )?;
        note += &format!("; rx {}/{} us, tx {}/{} us", v[0], v[1], v[2], v[3]);
    }
    Ok(note)
}

fn own_length_req(s: &mut BleSession) -> R<String> {
    s.connect(LinkOpts::new())?;
    s.link_run(1500.0)?;
    let Some(req) = s.lk().ctrl.iter().find(|p| p.opcode() == Some(0x14)).cloned() else {
        return Err(Stop::NotApplicable("the bulb does not start the Data Length Update procedure".into()));
    };
    let f = features(s)?;
    let v: Vec<u64> = [1, 3, 5, 7].iter().map(|i| le(pl(&req), *i)).collect();
    expect(
        req.length() == 9 && v[0] >= 27 && v[2] >= 27 && v[1] >= 328 && v[3] >= 328,
        format!("LL_LENGTH_REQ {}", hx(pl(&req))),
    )?;
    expect(f >> 5 & 1 != 0, format!("LL_LENGTH_REQ sent, FeatureSet {f:#x} without bit 5"))?;
    Ok(format!("LL_LENGTH_REQ in event {}: rx {}/{} us, tx {}/{} us", req.event, v[0], v[1], v[2], v[3]))
}

fn phy_req(s: &mut BleSession) -> R<String> {
    optional(s, "LL_PHY_REQ", 0x16, &[0x03, 0x03], &[8, 11], &[0x17], "")
}

fn conn_param_req(s: &mut BleSession) -> R<String> {
    let mut ctr = vec![0x18, 0x00, 0x28, 0x00, 0x00, 0x00, 0x90, 0x01, 0x00, 0x00, 0x00];
    ctr.extend_from_slice(&[0xff; 12]);
    optional(s, "LL_CONNECTION_PARAM_REQ", 0x0F, &ctr, &[1], &[0x10], "")
}

fn other_optional(s: &mut BleSession) -> R<String> {
    let mut notes = Vec::new();
    let cases: [(&str, u8, Vec<u8>, u32, u8); 3] = [
        ("LL_CTE_REQ", 0x1A, vec![0x14], 18, 0x1B),
        ("LL_CLOCK_ACCURACY_REQ", 0x1D, vec![0x01], 26, 0x1E),
        ("LL_POWER_CONTROL_REQ", 0x23, vec![0x01, 0x00, 0x7f], 33, 0x24),
    ];
    for (name, o, ctr, bit, ok) in &cases {
        let r = optional(s, name, *o, ctr, &[*bit], &[*ok], "")?;
        notes.push(format!("{name}: {r}"));
        s.drop_link(6000.0)?;
    }
    Ok(notes.join("; "))
}

fn wrong_length(s: &mut BleSession) -> R<String> {
    let mut notes = Vec::new();
    let mut long = vec![0xff];
    long.extend_from_slice(&[0; 11]);
    for ctr in [vec![0xff, 0x00], long] {
        s.connect(LinkOpts::new())?;
        let mut q = vec![0x08];
        q.extend_from_slice(&ctr);
        let (rsp, allp) = s.ll_req(&q, &[0x09, UNKNOWN_RSP, REJECT, REJECT_EXT], 3000.0)?;
        expect(
            rsp.is_some(),
            format!("LL_FEATURE_REQ with {} octets of CtrData: {}", ctr.len(), names_or(&allp, "no answer")),
        )?;
        let rsp = rsp.unwrap();
        if rsp.opcode() == Some(UNKNOWN_RSP) {
            let u = at(&rsp, 1)?;
            expect(u == 0x08, format!("LL_UNKNOWN_RSP names {u:#04x}, the PDU was LL_FEATURE_REQ"))?;
        }
        let alive = s.alive(3000.0)?;
        expect(alive, "the connection ended")?;
        notes.push(format!("{} octets: {}", ctr.len(), names(std::slice::from_ref(&rsp))[0]));
        s.drop_link(6000.0)?;
    }
    Ok(notes.join("; "))
}

fn enc_req(s: &mut BleSession) -> R<String> {
    s.connect(LinkOpts::new())?;
    let f = features(s)?;
    let mut req = vec![0x03];
    req.extend_from_slice(&[0; 10]);
    req.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    req.extend_from_slice(&[0xa1, 0xa2, 0xa3, 0xa4]);
    let (rsp, _allp) = s.ll_req(&req, &[0x04, REJECT, REJECT_EXT, UNKNOWN_RSP], 3000.0)?;
    expect(rsp.is_some(), format!("LL_ENC_REQ: no answer (FeatureSet {f:#x})"))?;
    let rsp = rsp.unwrap();
    let nm = |p: &Pdu| names(std::slice::from_ref(p))[0].clone();
    if f & 1 == 0 {
        expect(
            rsp.opcode() == Some(REJECT) || rsp.opcode() == Some(REJECT_EXT),
            format!("no LE Encryption in FeatureSet {f:#x}; LL_ENC_REQ answered with {}", nm(&rsp)),
        )?;
        let code = last(&rsp)?;
        if rsp.opcode() == Some(REJECT_EXT) {
            expect(
                rsp.length() == 3 && pl(&rsp).get(1) == Some(&0x03),
                format!("LL_REJECT_EXT_IND {}", hx(pl(&rsp))),
            )?;
        }
        expect(code == 0x1A, format!("reject with error {code:#04x}, required 0x1A (Unsupported Remote Feature)"))?;
        let alive = s.alive(3000.0)?;
        expect(alive, "the connection ended after the reject")?;
        return Ok(format!("no LE Encryption: {}", nm(&rsp)));
    }
    expect(
        rsp.opcode() == Some(0x04),
        format!("FeatureSet {f:#x} claims LE Encryption; LL_ENC_REQ answered with {}", nm(&rsp)),
    )?;
    expect(rsp.length() == 13, format!("LL_ENC_RSP with {} octets (SKD_P 8 + IV_P 4)", len_i(&rsp) - 1))?;
    let idx = s.lk().ctrl.iter().position(|p| p.i == rsp.i).unwrap_or(0);
    let nxt = s.ll_wait(&[REJECT, REJECT_EXT, 0x05, 0x02], 10000.0, Some(idx + 1))?;
    let Some(nxt) = nxt else {
        let rd = s.att_t(&[0x0a, 0x03, 0x00], 3000.0)?;
        return Err(Stop::Fail(format!(
            "LL_ENC_RSP sent, then neither a reject (no key) nor LL_START_ENC_REQ within 10 s: the procedure is left \
             open; an attribute read afterwards is {}, link {}",
            if rd.as_ref().is_some_and(|r| !r.is_empty()) { "answered" } else { "not answered" },
            s.lk().st.state()
        )));
    };
    if nxt.opcode() == Some(0x05) {
        expect(false, "LL_START_ENC_REQ: the bulb claims to hold a Long Term Key for a central it never paired with")?;
    }
    expect(
        nxt.opcode() == Some(REJECT) || nxt.opcode() == Some(REJECT_EXT),
        format!("after LL_ENC_RSP: {}", nm(&nxt)),
    )?;
    let code = last(&nxt)?;
    expect(code == 0x06, format!("reject with error {code:#04x}, required 0x06 (PIN or Key Missing)"))?;
    if nxt.opcode() == Some(REJECT_EXT) {
        let o = at(&nxt, 1)?;
        expect(o == 0x03, format!("LL_REJECT_EXT_IND names opcode {o:#04x}, not LL_ENC_REQ"))?;
    }
    let alive = s.alive(3000.0)?;
    expect(alive, "the connection ended after the reject")?;
    Ok(format!("LL_ENC_RSP, then {}", nm(&nxt)))
}

fn terminate(s: &mut BleSession) -> R<String> {
    s.connect(LinkOpts::new().set("interval", 24).set("timeout", 300))?;
    s.wait_event(8, None)?;
    s.terminate(0x13)?;
    s.link_until(1000.0, None, |s| Ok(if s.lk().st.state() != "connected" { Some(()) } else { None }))?;
    let tx: Vec<_> = s.central_log()?.into_iter().filter(|t| t.what.contains("LL_TERMINATE_IND")).collect();
    expect(!tx.is_empty(), "the central did not send LL_TERMINATE_IND")?;
    let st = s.lk().st.state().to_string();
    expect(
        st == "terminated",
        format!(
            "the central's LL_TERMINATE_IND (sent {} times) was not acknowledged; central state {st}",
            tx.len()
        ),
    )?;
    let ev = tx.last().unwrap().event;
    let Some(ack) = s.lk().pdus.iter().find(|p| p.event == ev).cloned() else {
        return Err(Stop::Error("StopIteration: no PDU of the bulb in the event of the LL_TERMINATE_IND".into()));
    };
    let t_ack = ack.t_end;
    s.run(1500.0)?;
    let rx: Vec<f64> = s.rx_windows(t_ack + 1.0).into_iter().map(|w| w.0).collect();
    let iv = s.interval_ms();
    let late = rx.iter().any(|ms| *ms > t_ack + 1.5 * iv);
    let msg = if rx.is_empty() {
        String::new()
    } else {
        let mx = rx.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        format!(
            "{} connection receive windows after the acknowledgment, the last {:.0} ms after it",
            rx.len(),
            mx - t_ack
        )
    };
    expect(!late, msg)?;
    let adv: Vec<_> = s.adv(t_ack, false).into_iter().filter(|p| p.ty() == 0).collect();
    let mut out = format!("acknowledged in event {} (NESN {}), ", ack.event, ack.nesn());
    out += &if rx.is_empty() {
        "no receive window afterwards; ".to_string()
    } else {
        format!("one more receive window {:.0} ms later, then none; ", rx[0] - t_ack)
    };
    out += &match adv.first() {
        Some(a) => format!("advertising again after {:.0} ms", a.ms - t_ack),
        None => "no advertising in the next 1.5 s".into(),
    };
    Ok(out)
}

fn make_it_leave(s: &mut BleSession) -> R<usize> {
    s.connect(LinkOpts::new().set("interval", 24).set("timeout", 300))?;
    let mut app = MeshApp::new(s)?.login(s)?;
    s.link_run(200.0)?;
    let n0 = s.lk().ctrl.len();
    if let Err(e) = app.command(s, OP_MESH_RESET, &[0])? {
        return Err(Stop::Error(format!("OtaError: {e}")));
    }
    Ok(n0)
}

fn peripheral_terminate(s: &mut BleSession) -> R<String> {
    let n0 = make_it_leave(s)?;
    let term = s.ll_wait(&[0x02], 5000.0, Some(n0))?;
    let Some(term) = term else {
        s.link_run(500.0)?;
        if s.alive(1000.0)? {
            return Err(Stop::Inconclusive("the application's reset command did not make the bulb end the connection".into()));
        }
        return Err(Stop::Fail(format!(
            "the bulb left the connection without sending LL_TERMINATE_IND (the central heard none; firmware resets: {})",
            s.resets()
        )));
    };
    expect(term.length() == 2, format!("LL_TERMINATE_IND {}", hx(pl(&term))))?;
    s.link_until(1000.0, None, |s| Ok(if s.lk().st.state() == "terminated" { Some(()) } else { None }))?;
    s.run(1500.0)?;
    let since = term.t_end + s.interval_ms() * 2.5;
    let rx = s.rx_windows(since);
    expect(
        rx.is_empty(),
        format!(
            "{} connection receive windows later than two events after the LL_TERMINATE_IND was acknowledged",
            rx.len()
        ),
    )?;
    Ok(format!("LL_TERMINATE_IND error code {:#04x} in event {}, then silence", at(&term, 1)?, term.event))
}

fn peripheral_terminate_timer(s: &mut BleSession) -> R<String> {
    let n0 = make_it_leave(s)?;
    let to = 3000.0;
    let t0 = s.now();
    let ev = s.event()?;
    let iv = s.interval_ms();
    for e in ev + 1..ev + ((to + 2500.0) / iv) as i64 {
        s.drop_event(e)?;
    }
    s.run(to + 2000.0)?;
    s.poll()?;
    let lost: Vec<(f64, Vec<u8>)> = s
        .lost()?
        .into_iter()
        .filter(|(ms, _, _, raw)| {
            *ms >= t0 && raw.first().is_some_and(|b| b & 3 == 3) && raw.get(1).is_some_and(|b| *b != 0) && raw.get(2) == Some(&0x02)
        })
        .map(|(ms, _, _, raw)| (ms, raw))
        .collect();
    if lost.is_empty() {
        if s.lk().ctrl.get(n0..).unwrap_or(&[]).iter().any(|p| p.opcode() == Some(0x02)) {
            return Err(Stop::Inconclusive("the LL_TERMINATE_IND got through before the central started losing PDUs".into()));
        }
        if s.alive(1000.0)? {
            return Err(Stop::Inconclusive("the application's reset command did not make the bulb end the connection".into()));
        }
        return Err(Stop::Fail("the bulb left the connection without an LL_TERMINATE_IND on the air".into()));
    }
    let (first, lastt) = (lost[0].0, lost.last().unwrap().0);
    let rx: Vec<f64> = s.rx_windows(first).into_iter().map(|w| w.0).collect();
    let span = lastt - first;
    let iv = s.interval_ms();
    expect(span <= to + 2.0 * iv, format!("LL_TERMINATE_IND sent for {span:.0} ms, T_terminate is 3000 ms"))?;
    expect(
        span >= to - 3.0 * iv,
        format!(
            "LL_TERMINATE_IND sent {} times over {span:.0} ms without an acknowledgment, then the bulb left; \
             T_terminate is connSupervisionTimeout = 3000 ms",
            lost.len()
        ),
    )?;
    let mx = rx.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    expect(rx.is_empty() || mx <= first + to + 2.0 * iv, "receive windows after T_terminate")?;
    Ok(format!("LL_TERMINATE_IND repeated {} times over {span:.0} ms (T_terminate 3000 ms)", lost.len()))
}

fn procedure_timeout(s: &mut BleSession) -> R<String> {
    s.connect(LinkOpts::new().set("interval", 80).set("timeout", 600).set("llauto", 0))?;
    s.link_run(3000.0)?;
    let reqs: Vec<Pdu> = s
        .lk()
        .ctrl
        .iter()
        .filter(|p| p.opcode().is_some_and(|o| [0x0E, 0x0F, 0x12, 0x14, 0x16, 0x0C].contains(&o)))
        .cloned()
        .collect();
    let Some(req) = reqs.last().cloned() else {
        return Err(Stop::NotApplicable("the bulb starts no control procedure of its own in the first 3 s".into()));
    };
    let t0 = req.t_end;
    let max = 60000.0 - (s.now() - t0);
    s.link_until(max, Some(100.0), |s| {
        let quiet = s.lk().pdus.last().is_some_and(|p| s.now() - p.t_end > 1500.0);
        Ok(if quiet { Some(()) } else { None })
    })?;
    let Some(lastp) = s.lk().pdus.last().cloned() else {
        return Err(Stop::Error("IndexError: list index out of range".into()));
    };
    let quiet = s.now() - lastp.t_end > 1500.0;
    let t_last = lastp.t_end;
    let what = names(std::slice::from_ref(&req))[0].clone();
    expect(
        quiet,
        format!("{what} left unanswered; the bulb still answers connection events {:.0} ms later", s.now() - t0),
    )?;
    expect(
        t_last - t0 >= 39000.0,
        format!("{what} left unanswered; the bulb left the connection after {:.0} ms", t_last - t0),
    )?;
    Ok(format!("{what} unanswered: connection given up after {:.1} s", (t_last - t0) / 1000.0))
}
