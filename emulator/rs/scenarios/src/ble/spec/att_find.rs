//! ATT: Find Information and Find By Type Value (Core 5.4 Vol 3 Part F 3.4.3);
//! emulator/spec_ble/att_find.py.

use super::att_errors::cat;
use crate::ble::harness::{att_error, hx, BleSession, LinkOpts};
use crate::ble::util::{
    absent_uuid16, describe, error_cases, error_problem, link, read_all, some, table, u16b, value, Case, MTU,
};
use crate::ble::BleTest;
use crate::py;
use crate::session::{expect, Stop, R};

const DOC: &str = "Core 5.4 Vol 3 Part F";
const PRIMARY: [u8; 2] = [0x00, 0x28];
const AMBIG: &str = "for a range with Starting Handle greater than Ending Handle the clause names Invalid Handle; 'no \
                     attributes will be returned' (Attribute Not Found) is true of such a range as well, and 3.4.1.1 \
                     leaves the choice to the server 'if more than one error code applies'. The test requires Invalid \
                     Handle, the code the clause gives for exactly this case";

pub fn tests() -> Vec<BleTest> {
    vec![
        BleTest {
            id: "att_find_info_invalid_handles",
            doc: DOC,
            clause: "3.4.3.1",
            req: "ATT_FIND_INFORMATION_REQ with Starting Handle 0x0000 or greater than the Ending Handle is answered \
                  with Invalid Handle, Attribute Handle In Error = the Starting Handle.",
            session: "att",
            run: find_info_invalid_handles,
            ambiguous: Some(AMBIG),
            ..BleTest::D
        },
        BleTest {
            id: "att_find_info_not_found",
            doc: DOC,
            clause: "3.4.3.1",
            req: "ATT_FIND_INFORMATION_REQ for a range without attributes is answered with Attribute Not Found, \
                  Attribute Handle In Error = the Starting Handle.",
            session: "att",
            run: find_info_not_found,
            ..BleTest::D
        },
        BleTest {
            id: "att_find_info_format",
            doc: DOC,
            clause: "3.4.3.2",
            req: "Find Information Responses carry format 0x01 or 0x02 and whole handle-UUID pairs of that one size, \
                  in ascending order of handles, within ATT_MTU; reading on from the last handle ends with Attribute \
                  Not Found.",
            session: "att",
            run: find_info_format,
            ..BleTest::D
        },
        BleTest {
            id: "att_find_info_range",
            doc: DOC,
            clause: "3.4.3.1",
            req: "A Find Information Response holds only attributes of the requested handle range, beginning with the \
                  lowest one in it, each with the same type whatever the range asked.",
            session: "att",
            run: find_info_range,
            ..BleTest::D
        },
        BleTest {
            id: "att_find_by_type_value_invalid_handles",
            doc: DOC,
            clause: "3.4.3.3",
            req: "ATT_FIND_BY_TYPE_VALUE_REQ with Starting Handle 0x0000 or greater than the Ending Handle is \
                  answered with Invalid Handle, Attribute Handle In Error = the Starting Handle.",
            session: "att",
            run: fbtv_invalid_handles,
            ambiguous: Some(AMBIG),
            ..BleTest::D
        },
        BleTest {
            id: "att_find_by_type_value_not_found",
            doc: DOC,
            clause: "3.4.3.3",
            req: "ATT_FIND_BY_TYPE_VALUE_REQ that matches no attribute (other value, other length, range without the \
                  attribute) is answered with Attribute Not Found, Attribute Handle In Error = the Starting Handle.",
            session: "att",
            run: fbtv_not_found,
            ..BleTest::D
        },
        BleTest {
            id: "att_find_by_type_value_plain_attribute",
            doc: DOC,
            clause: "3.4.3.3, 3.4.3.4",
            req: "For an attribute type that is not a grouping type, ATT_FIND_BY_TYPE_VALUE_RSP lists every attribute \
                  with that type and value once, in handle order, with Group End Handle = Found Attribute Handle.",
            session: "att",
            run: fbtv_plain_attribute,
            ..BleTest::D
        },
    ]
}

fn rev(b: &[u8]) -> Vec<u8> {
    b.iter().rev().copied().collect()
}

/// Python's `[hex(h) for h in hs]`.
fn hexes(hs: &[u16]) -> String {
    py::list(hs.iter().map(|h| format!("'{h:#x}'")))
}

/// (handle, UUID) of an ATT_FIND_INFORMATION_RSP, or None when it is not a well-formed one.
fn pairs(rsp: Option<&[u8]>) -> Option<Vec<(u16, Vec<u8>)>> {
    let r = rsp?;
    if r.len() < 6 || r[0] != 0x05 || !(r[1] == 1 || r[1] == 2) {
        return None;
    }
    let w = if r[1] == 1 { 4 } else { 18 };
    if (r.len() - 2) % w != 0 {
        return None;
    }
    Some((2..r.len()).step_by(w).map(|i| (u16::from_le_bytes([r[i], r[i + 1]]), r[i + 2..i + w].to_vec())).collect())
}

fn find_info_invalid_handles(s: &mut BleSession) -> R<String> {
    // 3.4.3.1: "If a server receives an ATT_FIND_INFORMATION_REQ PDU with the Starting Handle parameter
    // greater than the Ending Handle parameter or the Starting Handle parameter is 0x0000, an
    // ATT_ERROR_RSP PDU shall be sent with the Error Code parameter set to Invalid Handle (0x01); the
    // Attribute Handle In Error parameter shall be set to the Starting Handle parameter."
    link(s, LinkOpts::new())?;
    table(s)?;
    let c = |what: &str, a: u16, b: u16| -> Case {
        (what.into(), cat(&[&[0x04], &u16b(a), &u16b(b)]), vec![0x01], Some(a))
    };
    let cases = vec![
        c("0x0000-0xFFFF", 0, 0xFFFF),
        c("0x0000-0x0000", 0, 0),
        c("0x0005-0x0002", 5, 2),
        c("0xFFFF-0x0001", 0xFFFF, 1),
    ];
    let bad = error_cases(s, &cases, 2)?;
    expect(bad.is_empty(), some(&bad, 5))?;
    Ok("ranges 0x0000-0xFFFF, 0x0000-0x0000, 0x0005-0x0002, 0xFFFF-0x0001: Invalid Handle with the Starting Handle".into())
}

fn find_info_not_found(s: &mut BleSession) -> R<String> {
    // 3.4.3.1: "If no attributes will be returned, an ATT_ERROR_RSP PDU shall be sent with the Error Code
    // parameter set to Attribute Not Found (0x0A); the Attribute Handle In Error parameter shall be set to
    // the Starting Handle parameter."
    link(s, LinkOpts::new())?;
    let g = table(s)?;
    let nx = g.last_handle() as u32 + 1;
    if nx > 0xFFFF {
        return Err(Stop::Inconclusive("the table reaches handle 0xFFFF".into()));
    }
    let nx = nx as u16;
    let cases: Vec<Case> = [(nx, 0xFFFFu16), (nx, nx), (0x1234, 0x1300), (0xFFFF, 0xFFFF)]
        .into_iter()
        .filter(|(a, b)| !g.attrs.keys().any(|h| a <= h && h <= b))
        .map(|(a, b)| (format!("{a:#06x}-{b:#06x}"), cat(&[&[0x04], &u16b(a), &u16b(b)]), vec![0x0A], Some(a)))
        .collect();
    let bad = error_cases(s, &cases, 2)?;
    expect(bad.is_empty(), some(&bad, 5))?;
    Ok(format!(
        "{} ranges above the last attribute ({:#06x}): Attribute Not Found with the Starting Handle",
        cases.len(),
        g.last_handle()
    ))
}

fn find_info_format(s: &mut BleSession) -> R<String> {
    // 3.4.3.2: "The Find Information Response shall have complete handle-UUID pairs. Such pairs shall not
    // be split across response packets ... The handle-UUID pairs shall be returned in ascending order of
    // attribute handles." Table 3.9: format 0x01 = 16-bit UUIDs (4-octet pairs), 0x02 = 128-bit UUIDs
    // (18-octet pairs); "it is not possible to include attributes with differing UUID sizes into a single
    // response packet". Table 3.8: Information Data 4 to (ATT_MTU-2) octets.
    link(s, LinkOpts::new())?;
    let (mut start, mut last, mut n): (u16, u16, usize) = (1, 0, 0);
    let mut fmts: Vec<u8> = Vec::new();
    let mut bad = Vec::new();
    for _ in 0..300 {
        let rsp = s.att(&cat(&[&[0x04], &u16b(start), &u16b(0xFFFF)]))?;
        if att_error(rsp.as_deref()).is_some() {
            if let Some(p) = error_problem(rsp.as_deref(), 0x04, &[0x0A], Some(start)) {
                bad.push(format!("request from {start:#06x}: {p}"));
            }
            break;
        }
        let Some(pr) = pairs(rsp.as_deref()) else {
            bad.push(format!(
                "request from {start:#06x}: {}; not a format byte 1 or 2 followed by whole pairs",
                describe(rsp.as_deref())
            ));
            break;
        };
        let rsp = rsp.unwrap();
        if rsp.len() > MTU {
            bad.push(format!("request from {start:#06x}: a response of {} octets", rsp.len()));
        }
        let hs: Vec<u16> = pr.iter().map(|(h, _)| *h).collect();
        let mut prev = vec![last];
        prev.extend(&hs);
        if hs[0] < start || prev.iter().zip(hs.iter()).any(|(a, b)| b <= a) {
            bad.push(format!("request from {start:#06x}: handles {} after {last:#06x}: not ascending", hexes(&hs)));
            break;
        }
        if !fmts.contains(&rsp[1]) {
            fmts.push(rsp[1]);
        }
        n += pr.len();
        last = *hs.last().unwrap();
        if last == 0xFFFF {
            break;
        }
        start = last + 1;
    }
    expect(bad.is_empty(), some(&bad, 5))?;
    expect(n > 0, "no attribute found")?;
    fmts.sort();
    Ok(format!(
        "{n} attributes up to handle {last:#06x} in formats {}; then Attribute Not Found",
        py::ints(fmts.iter().map(|f| *f as i64))
    ))
}

fn find_info_range(s: &mut BleSession) -> R<String> {
    // 3.4.3.1: "Only attributes with attribute handles between the Starting Handle parameter and the
    // Ending Handle parameter will be returned." 3.4.3.2: ascending order, so the first pair is the lowest
    // attribute of the range. Every handle is asked alone (start = end), then some ranges that end in the
    // middle of the table.
    link(s, LinkOpts::new())?;
    let g = table(s)?;
    let hs: Vec<u16> = g.attrs.keys().copied().collect();
    let mut ranges: Vec<(u16, u16)> = hs.iter().map(|h| (*h, *h)).collect();
    ranges.extend([
        (hs[0], hs[hs.len() / 2]),
        (hs[1], hs[2]),
        (hs[hs.len() / 3], hs[hs.len() - 2]),
        (hs[hs.len() - 1], 0xFFFF),
    ]);
    let mut bad = Vec::new();
    for (a, b) in &ranges {
        let (a, b) = (*a, *b);
        let rsp = s.att(&cat(&[&[0x04], &u16b(a), &u16b(b)]))?;
        let Some(pr) = pairs(rsp.as_deref()) else {
            bad.push(format!("{a:#06x}-{b:#06x}: {}", describe(rsp.as_deref())));
            continue;
        };
        let low = hs.iter().copied().filter(|h| a <= *h && *h <= b).min().unwrap();
        let out: Vec<u16> = pr.iter().map(|(h, _)| *h).filter(|h| !(a <= *h && *h <= b)).collect();
        if !out.is_empty() {
            bad.push(format!("{a:#06x}-{b:#06x}: handles outside the range: {}", hexes(&out)));
        } else if pr[0].0 != low {
            bad.push(format!(
                "{a:#06x}-{b:#06x}: first handle {:#06x}, the lowest attribute in the range is {low:#06x}",
                pr[0].0
            ));
        } else if pr.iter().any(|(h, u)| g.attrs.get(h) != Some(u)) {
            bad.push(format!(
                "{a:#06x}-{b:#06x}: types differ from those found over 0x0001-0xFFFF: {}",
                hx(rsp.as_deref().unwrap())
            ));
        }
    }
    expect(bad.is_empty(), format!("{} of {} ranges: {}", bad.len(), ranges.len(), some(&bad, 4)))?;
    Ok(format!("{} single handles and 4 partial ranges answered from inside the range", hs.len()))
}

fn fbtv_invalid_handles(s: &mut BleSession) -> R<String> {
    // 3.4.3.3: "If a server receives an ATT_FIND_BY_TYPE_VALUE_REQ PDU with the Starting Handle parameter
    // greater than the Ending Handle parameter or the Starting Handle parameter is 0x0000, an
    // ATT_ERROR_RSP PDU shall be sent with the Error Code parameter set to Invalid Handle (0x01). The
    // Attribute Handle In Error parameter shall be set to the Starting Handle parameter." The type and
    // value are those of a primary service that exists, so there would be something to find.
    link(s, LinkOpts::new())?;
    let g = table(s)?;
    let sv = g.services[0].clone();
    let c = |what: String, a: u16, b: u16| -> Case {
        (what, cat(&[&[0x06], &u16b(a), &u16b(b), &PRIMARY, &sv.uuid]), vec![0x01], Some(a))
    };
    let cases = vec![
        c("0x0000-0xFFFF".into(), 0, 0xFFFF),
        c(format!("{:#06x}-{:#06x}", sv.end, sv.start), sv.end, sv.start),
        c("0xFFFF-0x0001".into(), 0xFFFF, 1),
    ];
    let bad = error_cases(s, &cases, 2)?;
    expect(bad.is_empty(), some(&bad, 5))?;
    Ok("Starting Handle 0x0000 and two reversed ranges: Invalid Handle with the Starting Handle".into())
}

fn fbtv_not_found(s: &mut BleSession) -> R<String> {
    // 3.4.3.3: "If no attributes will be returned, an ATT_ERROR_RSP PDU shall be sent by the server with
    // the Error Code parameter set to Attribute Not Found (0x0A). The Attribute Handle In Error parameter
    // shall be set to the starting handle." "Note: Attribute values will be compared in terms of length
    // and binary representation." Control: the same request with the value of an existing service is
    // answered with an ATT_FIND_BY_TYPE_VALUE_RSP.
    link(s, LinkOpts::new())?;
    let g = table(s)?;
    let sv = g.services[0].clone();
    let all = cat(&[&u16b(1), &u16b(0xFFFF)]);
    let ctl = s.att(&cat(&[&[0x06], &all, &PRIMARY, &sv.uuid]))?;
    if !ctl.as_ref().is_some_and(|c| c[0] == 0x07) {
        return Err(Stop::Inconclusive(format!(
            "control: the existing service {} is not found: {}",
            hx(&sv.uuid),
            describe(ctl.as_deref())
        )));
    }
    let absent = u16b(absent_uuid16(&g)?);
    let mut cases: Vec<Case> = vec![
        ("a service UUID the server does not have".into(), cat(&[&[0x06], &all, &PRIMARY, &absent]), vec![0x0A], Some(1)),
        (
            "the first octet of an existing service UUID".into(),
            cat(&[&[0x06], &all, &PRIMARY, &sv.uuid[..1]]),
            vec![0x0A],
            Some(1),
        ),
        (
            "an existing service UUID with an octet appended".into(),
            cat(&[&[0x06], &all, &PRIMARY, &sv.uuid, &[0x00]]),
            vec![0x0A],
            Some(1),
        ),
    ];
    if g.services.iter().filter(|x| x.uuid == sv.uuid).count() == 1 && sv.start < 0xFFFF {
        cases.push((
            "an existing service, range starting after its declaration".into(),
            cat(&[&[0x06], &u16b(sv.start + 1), &u16b(0xFFFF), &PRIMARY, &sv.uuid]),
            vec![0x0A],
            Some(sv.start + 1),
        ));
    }
    let bad = error_cases(s, &cases, 2)?;
    expect(bad.is_empty(), some(&bad, 5))?;
    Ok(format!("{} requests without a match: Attribute Not Found with the Starting Handle; control found", cases.len()))
}

fn fbtv_plain_attribute(s: &mut BleSession) -> R<String> {
    // 3.4.3.4: "For each handle that matches the attribute type and attribute value in the
    // ATT_FIND_BY_TYPE_VALUE_REQ PDU a Handles Information shall be returned ... If the attribute type in
    // the ATT_FIND_BY_TYPE_VALUE_REQ PDU is not a grouping attribute as defined by a higher layer
    // specification, the Group End Handle shall be equal to the Found Attribute Handle." "The Handles
    // Information List is ordered sequentially based on the found attribute handles." The attribute is a
    // characteristic value with a 16-bit type and a value of 1 to ATT_MTU - 7 octets; a characteristic
    // value is no grouping attribute (Part G 2.5.3). Second request: the same value with one octet changed,
    // which must not be found (3.4.3.3).
    link(s, LinkOpts::new())?;
    let g = table(s)?;
    let vals = read_all(s)?;
    let val_of = |h: u16| -> Option<Vec<u8>> {
        vals.iter().find(|(x, _)| *x == h).and_then(|(_, r)| value(r.as_deref()))
    };
    let pick = g
        .chars
        .iter()
        .find(|c| c.uuid.len() == 2 && val_of(c.value).is_some_and(|v| !v.is_empty() && v.len() <= MTU - 7))
        .cloned();
    let Some(pick) = pick else {
        return Err(Stop::Inconclusive("no readable characteristic value with a 16-bit type and 1 to 16 octets".into()));
    };
    let v = val_of(pick.value).unwrap();
    let hits: Vec<u16> =
        g.attrs.iter().filter(|(h, u)| **u == pick.uuid && val_of(**h).as_ref() == Some(&v)).map(|(h, _)| *h).collect();
    let want: Vec<u8> = hits.iter().take((MTU - 1) / 4).flat_map(|h| cat(&[&u16b(*h), &u16b(*h)])).collect();
    let pdu = cat(&[&[0x06], &u16b(1), &u16b(0xFFFF), &pick.uuid, &v]);
    let rsp = s.att(&pdu)?;
    let hl = hits.iter().map(|h| format!("{h:#06x}")).collect::<Vec<_>>().join(", ");
    expect(
        rsp.as_ref().is_some_and(|r| r[0] == 0x07 && r[1..] == want[..]),
        format!(
            "type {} with value {} ({}): {}; required 0x07 followed by {} (attribute {hl}, group end = the handle itself)",
            hx(&rev(&pick.uuid)),
            hx(&v),
            hx(&pdu),
            describe(rsp.as_deref()),
            hx(&want)
        ),
    )?;
    let mut other = v.clone();
    other[0] ^= 0x01;
    let r2 = s.att(&cat(&[&[0x06], &u16b(1), &u16b(0xFFFF), &pick.uuid, &other]))?;
    let p = error_problem(r2.as_deref(), 0x06, &[0x0A], Some(1));
    expect(
        p.is_none(),
        format!(
            "type {} with the value {}, which no attribute has: {}",
            hx(&rev(&pick.uuid)),
            hx(&other),
            p.unwrap_or_default()
        ),
    )?;
    Ok(format!("type {}, value {}: found at {hl}; another value not found", hx(&rev(&pick.uuid)), hx(&v)))
}
