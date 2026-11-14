//! ATT: what every server supports, requests it lacks, malformed requests, the
//! error response (Core 5.4 Vol 3 Part F 3.3, 3.4.1.1, 3.4.4.7, 3.4.4.11,
//! 3.4.9); emulator/spec_ble/att_errors.py.

use crate::ble::harness::{att_error, hx, BleSession, Gatt, LinkOpts};
use crate::ble::util::{
    ask, describe, ename, error_cases, error_problem, link, read, some, table, table_3_44, u16b, uuid128, value, Case,
    GENERIC_3_3, MTU,
};
use crate::ble::BleTest;
use crate::session::{expect, Stop, R};

const DOC: &str = "Core 5.4 Vol 3 Part F";

pub fn tests() -> Vec<BleTest> {
    vec![
        BleTest {
            id: "att_mandatory_requests",
            doc: DOC,
            clause: "3.3",
            req: "A server answers ATT_FIND_INFORMATION_REQ and ATT_READ_REQ properly: these two every server supports.",
            session: "att",
            run: mandatory_requests,
            ..BleTest::D
        },
        BleTest {
            id: "att_unsupported_request",
            doc: DOC,
            clause: "3.3, 3.4.1.1",
            req: "A request with an opcode the server does not know is answered with ATT_ERROR_RSP: Request Not \
                  Supported, Attribute Handle In Error 0x0000.",
            run: unsupported_request,
            ambiguous: Some(
                "a reserved opcode with the Command Flag clear is read as 'a request that it does not support'; the \
                 text does not say to which of the six PDU types an unassigned method belongs",
            ),
            ..BleTest::D
        },
        BleTest {
            id: "att_read_multiple",
            doc: DOC,
            clause: "3.4.4.7, 3.4.4.8, 3.3",
            req: "ATT_READ_MULTIPLE_REQ is answered: with the values concatenated in the order asked (at most ATT_MTU - \
                  1 octets), or by a server without the request with Request Not Supported and handle 0x0000.",
            run: read_multiple,
            ..BleTest::D
        },
        BleTest {
            id: "att_read_multiple_variable",
            doc: DOC,
            clause: "3.4.4.11, 3.4.4.12, 3.3",
            req: "ATT_READ_MULTIPLE_VARIABLE_REQ is answered: with a Length Value Tuple per handle, or by a server \
                  without the request with Request Not Supported and handle 0x0000.",
            run: read_multiple_variable,
            ..BleTest::D
        },
        BleTest {
            id: "att_invalid_pdu_short",
            doc: DOC,
            clause: "3.3, 3.4.1.1",
            req: "A request shorter than its fixed parameters is answered with ATT_ERROR_RSP: Invalid PDU, Attribute \
                  Handle In Error 0x0000.",
            run: invalid_pdu_short,
            ..BleTest::D
        },
        BleTest {
            id: "att_invalid_pdu_long",
            doc: DOC,
            clause: "3.3, 3.4.1.1",
            req: "A request of fixed length that carries an octet too many is answered with ATT_ERROR_RSP: Invalid \
                  PDU, Attribute Handle In Error 0x0000.",
            run: invalid_pdu_long,
            ambiguous: Some(
                "'the PDU is the wrong length' is the text's example of an invalid request; read as: any length other \
                 than the one the format table gives, a longer one included (the text has no rule that lets a server \
                 ignore octets after the last field)",
            ),
            ..BleTest::D
        },
        BleTest {
            id: "att_invalid_pdu_uuid_size",
            doc: DOC,
            clause: "3.3, 3.4.4.1, 3.4.4.9, 3.2.1",
            req: "Read By Type and Read By Group Type with an attribute type that is neither 2 nor 16 octets long are \
                  answered with ATT_ERROR_RSP: Invalid PDU, Attribute Handle In Error 0x0000.",
            run: invalid_pdu_uuid_size,
            ..BleTest::D
        },
        BleTest {
            id: "att_error_rsp_format",
            doc: DOC,
            clause: "3.4.1.1",
            req: "An ATT_ERROR_RSP has 5 octets and carries the opcode of the request and the attribute handle of the \
                  request, both octets of it.",
            session: "att",
            run: error_rsp_format,
            ..BleTest::D
        },
        BleTest {
            id: "att_error_codes_permitted",
            doc: DOC,
            clause: "3.4.9 (Table 3.44), 3.3",
            req: "Every ATT_ERROR_RSP carries an error code that Table 3.44 allows for the request it answers.",
            session: "att",
            run: error_codes_permitted,
            ambiguous: Some(
                "3.4.1.1 calls Table 3.44 the definitive list, but 3.3 requires Invalid PDU, Request Not Supported, \
                 Insufficient Resources and Unlikely Error for any request and the table has them for few; the test \
                 allows these four for every request besides the table",
            ),
            ..BleTest::D
        },
    ]
}

/// Concatenation of byte pieces.
pub fn cat(parts: &[&[u8]]) -> Vec<u8> {
    parts.iter().flat_map(|p| p.iter().copied()).collect()
}

fn mandatory_requests(s: &mut BleSession) -> R<String> {
    // 3.3: "A server shall be able to receive and properly respond to the following request PDUs:
    // ATT_FIND_INFORMATION_REQ, ATT_READ_REQ".
    link(s, LinkOpts::new())?;
    let rsp = s.att(&cat(&[&[0x04], &u16b(1), &u16b(0xFFFF)]))?;
    expect(
        rsp.as_ref().is_some_and(|r| r[0] == 0x05 && r.len() >= 6 && (r[1] == 1 || r[1] == 2)),
        format!("ATT_FIND_INFORMATION_REQ 0x0001-0xFFFF: {}", describe(rsp.as_deref())),
    )?;
    let rsp = rsp.unwrap();
    let h = rsp[2] as u16 | (rsp[3] as u16) << 8;
    let rd = read(s, h)?;
    expect(
        rd.as_ref().is_some_and(|r| r[0] == 0x0B),
        format!("ATT_READ_REQ on handle {h:#06x}, the first one found: {}", describe(rd.as_deref())),
    )?;
    Ok(format!(
        "Find Information Response (format {}), Read Response for handle {h:#06x} ({} octets)",
        rsp[1],
        rd.unwrap().len() - 1
    ))
}

fn unsupported_request(s: &mut BleSession) -> R<String> {
    // 3.3: "If a server receives a request that it does not support, then the server shall respond with
    // the ATT_ERROR_RSP PDU with the Error Code parameter set to Request Not Supported (0x06), with the
    // Attribute Handle In Error set to 0x0000." 3.4.1.1: "If there was no attribute handle in the original
    // request or if the request is not supported, then the value 0x0000 shall be used for this field."
    // Opcodes 0x14 and 0x3E are unassigned in Table 3.43 (Command Flag and Authentication Signature Flag
    // clear). The parameters are 0x0001, a valid handle: the answer must not echo it.
    link(s, LinkOpts::new())?;
    let cases: Vec<Case> = [0x14u8, 0x3E]
        .iter()
        .map(|op| (format!("opcode {op:#04x}"), cat(&[&[*op], &u16b(1)]), vec![0x06], Some(0x0000)))
        .collect();
    let bad = error_cases(s, &cases, 2)?;
    let ctl = read(s, 1)?;
    expect(
        bad.is_empty(),
        some(&bad, 5) + if ctl.is_some() { "" } else { "; a Read Request afterwards is not answered either" },
    )?;
    Ok("opcodes 0x14 and 0x3e: Request Not Supported, handle 0x0000".into())
}

/// Two attributes any server lets a client read: the first service declaration
/// and the first readable characteristic value; with their values.
fn two_readable(s: &mut BleSession, g: &Gatt) -> R<(u16, Vec<u8>, u16, Vec<u8>)> {
    let h1 = g.services[0].start;
    let mut c = None;
    for ch in &g.chars {
        if ch.props & 0x02 != 0 {
            let r = read(s, ch.value)?;
            if value(r.as_deref()).is_some() {
                c = Some(ch.clone());
                break;
            }
        }
    }
    let r1 = read(s, h1)?;
    let v1 = value(r1.as_deref());
    let (Some(c), Some(v1)) = (c, v1) else { return Err(Stop::Inconclusive("no two readable attributes found".into())) };
    let r2 = read(s, c.value)?;
    Ok((h1, v1, c.value, value(r2.as_deref()).unwrap_or_default()))
}

fn read_multiple(s: &mut BleSession) -> R<String> {
    // 3.4.4.8: "The Set Of Values parameter shall be a concatenation of attribute values for each of the
    // attribute handles in the request in the order that they were requested. If the Set Of Values
    // parameter is longer than (ATT_MTU-1) then only the first (ATT_MTU-1) octets shall be included".
    // 3.4.4.7: "If any of the handles are invalid ... Invalid Handle (0x01) ... the Attribute Handle In
    // Error parameter shall be set to the handle of the first attribute causing the error." The request
    // is optional in a server (Part G 4.2); 3.3 then requires Request Not Supported with handle 0x0000.
    link(s, LinkOpts::new())?;
    let g = table(s)?;
    let (h1, v1, h2, v2) = two_readable(s, &g)?;
    let pdu = cat(&[&[0x0e], &u16b(h1), &u16b(h2)]);
    let rsp = ask(s, &pdu)?;
    expect(rsp.is_some(), format!("ATT_READ_MULTIPLE_REQ {}: {}", hx(&pdu), describe(rsp.as_deref())))?;
    let rsp = rsp.unwrap();
    if att_error(Some(&rsp)).is_some() {
        let p = error_problem(Some(&rsp), 0x0E, &[0x06], Some(0x0000));
        expect(
            p.is_none(),
            format!("ATT_READ_MULTIPLE_REQ for two readable attributes ({}): {}", hx(&pdu), p.unwrap_or_default()),
        )?;
        return Ok("not supported: Request Not Supported, handle 0x0000".into());
    }
    let trunc = |v: Vec<u8>| -> Vec<u8> { v.into_iter().take(MTU - 1).collect() };
    let want = trunc(cat(&[&v1, &v2]));
    expect(
        rsp[0] == 0x0F && rsp[1..] == want[..],
        format!("ATT_READ_MULTIPLE_REQ {}: {}; required 0x0f followed by {}", hx(&pdu), hx(&rsp), hx(&want)),
    )?;
    let rev = s.att(&cat(&[&[0x0e], &u16b(h2), &u16b(h1)]))?;
    let want_r = trunc(cat(&[&v2, &v1]));
    expect(
        rev.as_ref().is_some_and(|r| r[1..] == want_r[..]),
        format!("handles in the other order: {}; required {}", describe(rev.as_deref()), hx(&want_r)),
    )?;
    let nx = g.last_handle().wrapping_add(1);
    let r = s.att(&cat(&[&[0x0e], &u16b(h1), &u16b(nx)]))?;
    let p = error_problem(r.as_deref(), 0x0E, &[0x01], Some(nx));
    expect(p.is_none(), format!("a set with the invalid handle {nx:#06x} in second place: {}", p.unwrap_or_default()))?;
    Ok(format!(
        "supported: values of {h1:#06x} and {h2:#06x} concatenated in the order asked; invalid handle refused"
    ))
}

fn read_multiple_variable(s: &mut BleSession) -> R<String> {
    // 3.4.4.12: "The Length Value Tuple List shall be a concatenation of Length Value Tuples for each of
    // the attribute handles in the request in the order that they were requested. If the Length Value
    // Tuple List is longer than (ATT_MTU-1) octets, then it shall be truncated after (ATT_MTU-1) or, if
    // that would be within the Value Length field of a Length Value Tuple, at the start of the Length
    // Value Tuple." Optional in a server without Enhanced ATT bearers (Part G 4.2, C.4); 3.3 then requires
    // Request Not Supported with handle 0x0000.
    link(s, LinkOpts::new())?;
    let g = table(s)?;
    let (h1, v1, h2, v2) = two_readable(s, &g)?;
    let pdu = cat(&[&[0x20], &u16b(h1), &u16b(h2)]);
    let rsp = ask(s, &pdu)?;
    expect(rsp.is_some(), format!("ATT_READ_MULTIPLE_VARIABLE_REQ {}: {}", hx(&pdu), describe(rsp.as_deref())))?;
    let rsp = rsp.unwrap();
    if att_error(Some(&rsp)).is_some() {
        let p = error_problem(Some(&rsp), 0x20, &[0x06], Some(0x0000));
        expect(
            p.is_none(),
            format!(
                "ATT_READ_MULTIPLE_VARIABLE_REQ for two readable attributes ({}): {}",
                hx(&pdu),
                p.unwrap_or_default()
            ),
        )?;
        return Ok("not supported: Request Not Supported, handle 0x0000".into());
    }
    let t1 = cat(&[&u16b(v1.len() as u16), &v1]);
    let t2 = cat(&[&u16b(v2.len() as u16), &v2]);
    let room = MTU as i64 - 1 - t1.len() as i64;
    let mut want = t1.clone();
    if room >= 2 {
        want.extend(t2.iter().take(room as usize));
    }
    expect(
        rsp[0] == 0x21 && rsp[1..] == want[..],
        format!("ATT_READ_MULTIPLE_VARIABLE_REQ {}: {}; required 0x21 followed by {}", hx(&pdu), hx(&rsp), hx(&want)),
    )?;
    Ok(format!("supported: Length Value Tuples of {h1:#06x} and {h2:#06x}"))
}

/// Requests a GATT server may lack (Part G 4.2: Exchange MTU, Read Long, Write,
/// Write Long): Request Not Supported is an answer 3.3 allows as well.
fn optional(op: u8) -> Vec<u8> {
    if [0x02, 0x0C, 0x12, 0x16, 0x18].contains(&op) {
        vec![0x04, 0x06]
    } else {
        vec![0x04]
    }
}

fn invalid(s: &mut BleSession, cases: &[(&str, Vec<u8>)]) -> R<Vec<String>> {
    link(s, LinkOpts::new())?;
    table(s)?;
    let cs: Vec<Case> =
        cases.iter().map(|(what, pdu)| (what.to_string(), pdu.clone(), optional(pdu[0]), Some(0x0000))).collect();
    let mut bad = error_cases(s, &cs, 1)?;
    let ctl = read(s, 1)?;
    if !ctl.as_ref().is_some_and(|c| c[0] == 0x0B) {
        bad.push(format!("a well-formed Read Request afterwards: {}", describe(ctl.as_deref())));
    }
    Ok(bad)
}

fn not_after(bad: &[String]) -> usize {
    bad.iter().filter(|b| !b.contains("afterwards")).count()
}

fn invalid_pdu_short(s: &mut BleSession) -> R<String> {
    // 3.3: "If the server receives an invalid request – for example, the PDU is the wrong length – then
    // the server shall respond with the ATT_ERROR_RSP PDU with the Error Code parameter set to Invalid PDU
    // (0x04), with the Attribute Handle In Error set to 0x0000." Each request below lacks octets of a
    // mandatory field (Tables 3.7, 3.12, 3.15, 3.18, 3.20, 3.24, 3.30, 3.5). The octets present name valid
    // handles, so that a server reading past the end of the PDU has something to answer with.
    let cases = vec![
        ("Find Information, 1 octet of the ending handle", cat(&[&[0x04], &u16b(1), &[0xff]])),
        ("Find By Type Value, 1 octet of the type", cat(&[&[0x06], &u16b(1), &u16b(0xFFFF), &[0x00]])),
        ("Read By Type without a type", cat(&[&[0x08], &u16b(1), &u16b(0xFFFF)])),
        ("Read Request without a handle", vec![0x0a]),
        ("Read Request, 1 octet of the handle", vec![0x0a, 0x03]),
        ("Read Blob without an offset", cat(&[&[0x0c], &u16b(3)])),
        ("Read Blob, 1 octet of the offset", cat(&[&[0x0c], &u16b(3), &[0x00]])),
        ("Read By Group Type without a type", cat(&[&[0x10], &u16b(1), &u16b(0xFFFF)])),
        ("Write Request, 1 octet of the handle", vec![0x12, 0x03]),
        ("Exchange MTU, 1 octet of the MTU", vec![0x02, 0x17]),
    ];
    let bad = invalid(s, &cases)?;
    expect(
        bad.is_empty(),
        format!(
            "{} of {} truncated requests not refused as required: {}",
            not_after(&bad),
            cases.len(),
            some(&bad, 4)
        ),
    )?;
    Ok(format!("{} truncated requests: Invalid PDU, handle 0x0000", cases.len()))
}

fn invalid_pdu_long(s: &mut BleSession) -> R<String> {
    // 3.3 as above ("the PDU is the wrong length"). ATT_FIND_INFORMATION_REQ (Table 3.7: 5 octets),
    // ATT_READ_REQ (Table 3.18: 3), ATT_READ_BLOB_REQ (Table 3.20: 5), ATT_EXECUTE_WRITE_REQ (Table 3.36:
    // 2) and ATT_EXCHANGE_MTU_REQ (Table 3.5: 3) have no variable field; one more octet makes the length
    // wrong.
    let cases = vec![
        ("Find Information, 6 octets", cat(&[&[0x04], &u16b(1), &u16b(0xFFFF), &[0x00]])),
        ("Read Request, 4 octets", cat(&[&[0x0a], &u16b(3), &[0x00]])),
        ("Read Blob, 6 octets", cat(&[&[0x0c], &u16b(3), &u16b(0), &[0x00]])),
        ("Execute Write, 3 octets", vec![0x18, 0x00, 0x00]),
        ("Exchange MTU, 4 octets", cat(&[&[0x02], &u16b(23), &[0x00]])),
    ];
    let bad = invalid(s, &cases)?;
    expect(
        bad.is_empty(),
        format!(
            "{} of {} over-long requests not refused as required: {}",
            not_after(&bad),
            cases.len(),
            some(&bad, 4)
        ),
    )?;
    Ok(format!("{} over-long requests: Invalid PDU, handle 0x0000", cases.len()))
}

fn invalid_pdu_uuid_size(s: &mut BleSession) -> R<String> {
    // Tables 3.15 and 3.24: "Attribute Type 2 or 16" octets. 3.2.1: "All 32-bit Attribute UUIDs shall be
    // converted to 128-bit UUIDs when the Attribute UUID is contained in an ATT PDU", so 4 octets is not a
    // valid size either. 3.3: a PDU of the wrong length is answered with Invalid PDU and handle 0x0000.
    // The types begin with the octets of «Characteristic» and «Primary Service», which exist.
    let rng = cat(&[&u16b(1), &u16b(0xFFFF)]);
    let u = uuid128(0x2803);
    let cases = vec![
        ("Read By Type, type of 3 octets", cat(&[&[0x08], &rng, &[0x03, 0x28, 0x00]])),
        ("Read By Type, type of 4 octets", cat(&[&[0x08], &rng, &[0x03, 0x28, 0x00, 0x00]])),
        ("Read By Type, type of 15 octets", cat(&[&[0x08], &rng, &u[1..]])),
        ("Read By Type, type of 17 octets", cat(&[&[0x08], &rng, &u, &[0x00]])),
        ("Read By Group Type, type of 3 octets", cat(&[&[0x10], &rng, &[0x00, 0x28, 0x00]])),
        ("Read By Group Type, type of 4 octets", cat(&[&[0x10], &rng, &[0x00, 0x28, 0x00, 0x00]])),
    ];
    let bad = invalid(s, &cases)?;
    expect(
        bad.is_empty(),
        format!("{} of {} requests not refused as required: {}", not_after(&bad), cases.len(), some(&bad, 4)),
    )?;
    Ok(format!("{} requests with types of 3, 4, 15 and 17 octets: Invalid PDU, handle 0x0000", cases.len()))
}

fn error_rsp_format(s: &mut BleSession) -> R<String> {
    // Table 3.3: opcode 0x01, Request Opcode In Error (1), Attribute Handle In Error (2), Error Code (1).
    // "The Request Opcode In Error parameter shall be set to the Attribute Opcode of the request that
    // generated this error. The Attribute Handle In Error parameter shall be set to the attribute handle in
    // the original request that generated this error. If there was no attribute handle in the original
    // request or if the request is not supported, then the value 0x0000 shall be used". Requests on handles
    // that do not exist, with distinct low and high octets; the error code is judged by other tests.
    link(s, LinkOpts::new())?;
    let g = table(s)?;
    let free: Vec<u32> = [g.last_handle() as u32 + 1, 0x0123, 0x3456, 0xFFFE]
        .into_iter()
        .filter(|h| *h <= 0xFFFF && !g.attrs.contains_key(&(*h as u16)))
        .collect();
    let mut cases: Vec<(&str, Vec<u8>, u16)> =
        free.iter().map(|h| ("Read Request", cat(&[&[0x0a], &u16b(*h as u16)]), *h as u16)).collect();
    cases.push(("Write Request", cat(&[&[0x12], &u16b(0x02F4), &[0x00]]), 0x02F4));
    cases.push(("Read Blob Request", cat(&[&[0x0c], &u16b(0x0345), &u16b(0)]), 0x0345));
    let (mut bad, mut codes) = (Vec::new(), Vec::new());
    for (what, pdu, h) in &cases {
        let rsp = ask(s, pdu)?;
        let Some(e) = att_error(rsp.as_deref()) else {
            bad.push(format!("{what} on handle {h:#06x}, which does not exist: {}", describe(rsp.as_deref())));
            continue;
        };
        if !codes.contains(&e.2) {
            codes.push(e.2);
        }
        let want = if e.2 == 0x06 { 0x0000 } else { *h };
        if e.0 != pdu[0] || e.1 != want {
            bad.push(format!(
                "{what} on handle {h:#06x}: ATT_ERROR_RSP {} names request {:#04x} and handle {:#06x}; required request \
                 {:#04x} and handle {want:#06x}",
                hx(rsp.as_deref().unwrap()),
                e.0,
                e.1,
                pdu[0]
            ));
        }
    }
    expect(bad.is_empty(), some(&bad, 5))?;
    codes.sort();
    Ok(format!(
        "{} error responses of 5 octets echo opcode and handle ({}); codes {}",
        cases.len(),
        cases.iter().map(|(_, _, h)| format!("{h:#06x}")).collect::<Vec<_>>().join(", "),
        codes.iter().map(|c| ename(*c)).collect::<Vec<_>>().join(", ")
    ))
}

fn error_codes_permitted(s: &mut BleSession) -> R<String> {
    // 3.4.1.1: "See Section 3.4.9 for the definitive list of which Error Code values are permitted."
    // Well-formed requests that cannot succeed (and reads of every attribute, which may): handle 0x0000,
    // handles beyond the table, reversed ranges, types that do not exist, a group type that is none,
    // offsets beyond the value, writes to a service declaration and to no attribute. Whether each error
    // is the right one is judged by the tests of each request; here only whether the table has it.
    link(s, LinkOpts::new())?;
    let g = table(s)?;
    let nx = g.last_handle().wrapping_add(1);
    let rng = cat(&[&u16b(1), &u16b(0xFFFF)]);
    let rev = cat(&[&u16b(5), &u16b(2)]);
    let zero = cat(&[&u16b(0), &u16b(0xFFFF)]);
    let past = cat(&[&u16b(nx), &u16b(0xFFFF)]);
    let svc = g.services[0].clone();
    let p2800 = u16b(0x2800);
    let seq: Vec<u8> = (0..16).collect();
    let mut reqs = vec![
        cat(&[&[0x04], &zero]),
        cat(&[&[0x04], &rev]),
        cat(&[&[0x04], &past]),
        cat(&[&[0x06], &zero, &p2800, &svc.uuid]),
        cat(&[&[0x06], &rev, &p2800, &svc.uuid]),
        cat(&[&[0x06], &rng, &p2800, &[0x34, 0x12]]),
        cat(&[&[0x06], &past, &p2800, &svc.uuid]),
        cat(&[&[0x08], &zero, &u16b(0x2803)]),
        cat(&[&[0x08], &rev, &u16b(0x2803)]),
        cat(&[&[0x08], &rng, &u16b(0x2A19)]),
        cat(&[&[0x08], &rng, &seq]),
        cat(&[&[0x08], &past, &u16b(0x2803)]),
        cat(&[&[0x08], &rng, &u16b(0x2802)]),
        cat(&[&[0x0a], &u16b(0)]),
        cat(&[&[0x0a], &u16b(nx)]),
        cat(&[&[0x0a], &u16b(0xFFFF)]),
        cat(&[&[0x0c], &u16b(0), &u16b(0)]),
        cat(&[&[0x0c], &u16b(nx), &u16b(0)]),
        cat(&[&[0x0c], &u16b(svc.start), &u16b(100)]),
        cat(&[&[0x0c], &u16b(svc.start), &u16b(0xFFFF)]),
        cat(&[&[0x10], &zero, &p2800]),
        cat(&[&[0x10], &rev, &p2800]),
        cat(&[&[0x10], &past, &p2800]),
        cat(&[&[0x10], &rng, &u16b(0x2803)]),
        cat(&[&[0x10], &rng, &u16b(0x2A00)]),
        cat(&[&[0x10], &rng, &u16b(0x2801)]),
        cat(&[&[0x12], &u16b(0), &[0x00]]),
        cat(&[&[0x12], &u16b(nx), &[0x00]]),
    ];
    let sv_rd = read(s, svc.start)?;
    let sv_val = value(sv_rd.as_deref()).filter(|v| !v.is_empty()).unwrap_or_else(|| vec![0x00]);
    reqs.push(cat(&[&[0x12], &u16b(svc.start), &sv_val]));
    for h in g.attrs.keys() {
        reqs.push(cat(&[&[0x0a], &u16b(*h)]));
    }
    let (mut bad, mut errs, mut silent) = (Vec::new(), 0, 0);
    for pdu in &reqs {
        let rsp = s.att_t(pdu, 3000.0)?;
        let Some(rsp) = rsp else {
            silent += 1;
            link(s, LinkOpts::new())?;
            continue;
        };
        let Some(e) = att_error(Some(&rsp)) else { continue };
        errs += 1;
        if !(table_3_44(pdu[0], e.2).unwrap_or(false) || GENERIC_3_3.contains(&e.2)) {
            bad.push(format!("request {}: {}", hx(pdu), ename(e.2)));
        }
    }
    expect(
        bad.is_empty(),
        format!("{} error codes that Table 3.44 does not allow for the request: {}", bad.len(), some(&bad, 5)),
    )?;
    if errs == 0 {
        return Err(Stop::Inconclusive(format!("none of {} requests was answered with an ATT_ERROR_RSP", reqs.len())));
    }
    Ok(format!(
        "{errs} error responses to {} requests, all with codes of Table 3.44{}",
        reqs.len(),
        if silent > 0 { format!("; {silent} requests not answered within 3 s (not judged here)") } else { String::new() }
    ))
}
