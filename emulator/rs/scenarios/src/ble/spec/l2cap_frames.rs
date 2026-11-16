use crate::ble::harness::{hx, BleSession, LinkOpts, CID_ATT};
use crate::ble::BleTest;
use crate::frames::py_bytes;
use crate::py;
use crate::session::{expect, Stop, R};

const DOC: &str = "Core 5.4 Vol 3 Part A";
const REQ: [u8; 7] = [0x08, 0x01, 0x00, 0xFF, 0xFF, 0x03, 0x28];

pub fn tests() -> Vec<BleTest> {
    vec![
        BleTest {
            id: "l2cap_recombination",
            doc: DOC,
            clause: "7.2.2; Vol 6 Part B 2.4.1",
            req: "An L2CAP frame that arrives in several LL data PDUs (start fragment, continuation fragments) is \
                  recombined and processed like the same frame in one PDU.",
            run: recombination,
            ..BleTest::D
        },
        BleTest {
            id: "l2cap_recombination_split_header",
            doc: DOC,
            clause: "7.2.2; Vol 6 Part B 2.4.1",
            req: "A start fragment shorter than the 4-octet L2CAP header is legal; the frame is recombined all the \
                  same.",
            run: split_header,
            ..BleTest::D
        },
        BleTest {
            id: "l2cap_fragmentation",
            doc: DOC,
            clause: "7.2.1; Vol 6 Part B 2.4.1, 4.5.10",
            req: "A frame longer than one LL PDU is sent as a start fragment (LLID 2) followed by continuation \
                  fragments (LLID 1) that add up to the L2CAP length.",
            run: fragmentation,
            ..BleTest::D
        },
        BleTest {
            id: "l2cap_length_mismatch",
            doc: DOC,
            clause: "7.2.2",
            req: "A frame whose fragments do not match its Length field is discarded, and the next frame is \
                  processed normally.",
            run: length_mismatch,
            ..BleTest::D
        },
        BleTest {
            id: "l2cap_unassigned_cid",
            doc: DOC,
            clause: "3",
            req: "A frame on a channel identifier that is not assigned is ignored.",
            run: unassigned_cid,
            ..BleTest::D
        },
    ]
}

fn frame(cid: u16, payload: &[u8]) -> Vec<u8> {
    let mut v = (payload.len() as u16).to_le_bytes().to_vec();
    v.extend_from_slice(&cid.to_le_bytes());
    v.extend_from_slice(payload);
    v
}

fn opt_repr(r: &Option<Vec<u8>>) -> String {
    r.as_ref().map(|b| py_bytes(b)).unwrap_or_else(|| "None".into())
}

fn send_split(s: &mut BleSession, fr: &[u8], cuts: &[usize]) -> R<Option<Vec<u8>>> {
    s.poll()?;
    let n0 = s.lk().frames.len();
    let mut pos = 0;
    let mut ends = cuts.to_vec();
    ends.push(fr.len());
    for (i, end) in ends.iter().enumerate() {
        s.data(if i == 0 { 2 } else { 1 }, &fr[pos..*end])?;
        pos = *end;
    }
    let f = s.frame_wait(CID_ATT, |f| BleSession::is_rsp(&f.data), 4000.0, Some(n0))?;
    Ok(f.map(|f| f.data))
}

fn reference(s: &mut BleSession) -> R<Vec<u8>> {
    s.connect(LinkOpts::new().set("interval", 6))?;
    s.link_run(100.0)?;
    let r = s.att_t(&REQ, 3000.0)?;
    match r {
        Some(x) if x[0] == 0x09 => Ok(x),
        r => Err(Stop::Inconclusive(format!("reference request not answered: {}", opt_repr(&r)))),
    }
}

fn cuts_str(c: &[usize]) -> String {
    py::ints(c.iter().map(|x| *x as i64))
}

fn recombination(s: &mut BleSession) -> R<String> {
    let rf = reference(s)?;
    let fr = frame(CID_ATT, &REQ);
    let mut bad = Vec::new();
    let all: Vec<usize> = (4..fr.len()).collect();
    for cuts in [vec![7], vec![4], vec![5, 10], vec![10], all] {
        let rsp = send_split(s, &fr, &cuts)?;
        if rsp.as_ref() != Some(&rf) {
            bad.push(format!("cut at {}: {}", cuts_str(&cuts), rsp.map(|r| hx(&r)).unwrap_or("no answer".into())));
            if !s.alive(1500.0)? {
                break;
            }
        }
    }
    expect(bad.is_empty(), format!("frame of {} octets: {}", fr.len(), bad.join("; ")))?;
    Ok("one request in 2, 3 and 8 fragments answered like the unfragmented one".into())
}

fn split_header(s: &mut BleSession) -> R<String> {
    let rf = reference(s)?;
    let fr = frame(CID_ATT, &REQ);
    let mut bad = Vec::new();
    for cuts in [vec![2], vec![1], vec![3], vec![1, 2, 3, 4]] {
        let rsp = send_split(s, &fr, &cuts)?;
        if rsp.as_ref() != Some(&rf) {
            bad.push(format!(
                "start fragment of {} octet(s): {}",
                cuts[0],
                rsp.map(|r| hx(&r)).unwrap_or("no answer".into())
            ));
        }
        if !s.alive(1500.0)? {
            bad.push("the connection ended".into());
            break;
        }
    }
    let after = s.att_t(&REQ, 3000.0)?;
    let tail = if after.as_ref() == Some(&rf) {
        "; a whole request afterwards is answered"
    } else {
        "; a whole request afterwards is not answered either"
    };
    expect(bad.is_empty(), bad.join("; ") + tail)?;
    Ok("start fragments of 1, 2 and 3 octets recombined".into())
}

fn fragmentation(s: &mut BleSession) -> R<String> {
    s.connect(LinkOpts::new().set("interval", 6))?;
    s.link_run(100.0)?;
    let rsp = s.att_t(&[0x02, 158, 0], 3000.0)?;
    let rsp = match rsp {
        Some(r) if r[0] == 0x03 => r,
        r => {
            let t = match &r {
                Some(x) if !x.is_empty() => hx(x),
                _ => "None".into(),
            };
            return Err(Stop::NotApplicable(format!("no Exchange MTU Response ({t}): responses stay within one PDU")));
        }
    };
    let mtu = 158u16.min(u16::from_le_bytes([rsp[1], rsp[2]]));
    if mtu <= 23 {
        return Err(Stop::NotApplicable(format!("the server's MTU is {mtu}: every response fits one PDU")));
    }
    let n0 = s.lk().frames.len();
    let reqs: [Vec<u8>; 3] = [REQ.to_vec(), vec![0x04, 1, 0, 0xFF, 0xFF], vec![0x10, 1, 0, 0xFF, 0xFF, 0x00, 0x28]];
    for q in &reqs {
        s.att_t(q, 3000.0)?;
    }
    s.link_run(50.0)?;
    let lk = s.lk();
    let long: Vec<_> = lk.frames[n0..].iter().filter(|f| f.cid == CID_ATT && f.length > 23).collect();
    expect(lk.problems.is_empty(), lk.problems.iter().take(3).cloned().collect::<Vec<_>>().join("; "))?;
    if long.is_empty() {
        return Err(Stop::Inconclusive(format!("ATT_MTU {mtu}, but no response longer than 23 octets")));
    }
    let mut f = long[0];
    for x in &long {
        if x.length > f.length {
            f = x;
        }
    }
    let parts: Vec<_> =
        lk.pdus.iter().filter(|p| f.first.i <= p.i && p.i <= f.last.i && p.new && p.length() != 0).collect();
    expect(
        parts[0].llid() == 2 && parts[1..].iter().all(|p| p.llid() == 1),
        format!("LLIDs of the fragments: {}", py::ints(parts.iter().map(|p| p.llid() as i64))),
    )?;
    let sum: usize = parts.iter().map(|p| p.length() as usize).sum();
    expect(
        sum == f.length as usize + 4,
        format!("fragments carry {sum} octets, the frame is {}", f.length as usize + 4),
    )?;
    Ok(format!(
        "ATT_MTU {mtu}: a {}-octet response in fragments of {} octets",
        f.length,
        py::ints(parts.iter().map(|p| p.length() as i64))
    ))
}

fn att_rsps(s: &BleSession, n0: usize) -> Vec<Vec<u8>> {
    s.lk().frames[n0..].iter().filter(|f| f.cid == CID_ATT && BleSession::is_rsp(&f.data)).map(|f| f.data.clone()).collect()
}

fn length_mismatch(s: &mut BleSession) -> R<String> {
    let rf = reference(s)?;
    let fr = frame(CID_ATT, &REQ);
    let mut notes = Vec::new();
    let n0 = s.lk().frames.len();
    s.data(2, &fr[..8])?;
    let rsp = s.att_t(&REQ, 3000.0)?;
    s.link_run(200.0)?;
    let got = att_rsps(s, n0);
    expect(
        rsp.as_ref() == Some(&rf) && got.len() == 1,
        format!(
            "incomplete frame then a whole request: {} responses, {}",
            got.len(),
            py::list(got.iter().take(2).map(|d| format!("'{}'", hx(d))))
        ),
    )?;
    notes.push("incomplete frame dropped");
    let n0 = s.lk().frames.len();
    s.data(2, &fr[..6])?;
    let mut cont = fr[6..].to_vec();
    cont.extend_from_slice(&[0x0a, 0x03, 0x00, 0x00, 0x00, 0x00]);
    s.data(1, &cont)?;
    s.link_run(600.0)?;
    let got = att_rsps(s, n0);
    let after = s.att_t(&REQ, 3000.0)?;
    expect(
        after.as_ref() == Some(&rf),
        format!(
            "after a frame with surplus octets a normal request gets {}",
            after.as_ref().map(|r| hx(r)).unwrap_or("no answer".into())
        ),
    )?;
    expect(
        got.is_empty(),
        format!(
            "a frame with 6 octets more than its Length field was processed: answered {}",
            got.first().map(|d| hx(d)).unwrap_or_default()
        ),
    )?;
    notes.push("frame with surplus octets dropped");
    Ok(notes.join("; "))
}

fn unassigned_cid(s: &mut BleSession) -> R<String> {
    s.connect(LinkOpts::new().set("interval", 6))?;
    s.link_run(100.0)?;
    let n0 = s.lk().frames.len();
    for cid in [0x0001u16, 0x0002, 0x0003, 0x0007, 0x0020, 0x003F, 0x0040, 0x007F, 0xFFFF] {
        s.l2cap(cid, &REQ)?;
    }
    s.link_run(1000.0)?;
    let got: Vec<_> =
        s.lk().frames[n0..].iter().filter(|f| !(f.cid == 5 && f.data.first() == Some(&0x12))).cloned().collect();
    expect(
        got.is_empty(),
        format!(
            "answers to frames on unassigned channels: {}",
            got.iter().take(3).map(|f| format!("cid {:#06x} {}", f.cid, hx(&f.data))).collect::<Vec<_>>().join("; ")
        ),
    )?;
    let rsp = s.att_t(&REQ, 3000.0)?;
    expect(rsp.is_some_and(|r| r[0] == 0x09), "the attribute channel does not answer afterwards")?;
    Ok("nine unassigned CIDs ignored".into())
}
