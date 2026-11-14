//! ATT: Read By Type, Read, Read Blob, Read By Group Type (Core 5.4 Vol 3 Part F
//! 3.4.4); emulator/spec_ble/att_read.py.

use crate::ble::harness::{att_error, hx, BleSession, Gatt, LinkOpts};
use crate::ble::util::{
    absent_uuid16, blob, describe, error_cases, error_problem, full_value, leading_run, link, read, read_all, some,
    table, uuid128, value, Case, MTU,
};
use crate::ble::{BleTest, Level};
use crate::py;
use crate::session::{expect, Stop, R};

const DOC: &str = "Core 5.4 Vol 3 Part F";
const S: &str = "att";
const CHAR: [u8; 2] = [0x03, 0x28];
const PRIMARY: [u8; 2] = [0x00, 0x28];
const ALL: [u8; 4] = [0x01, 0x00, 0xff, 0xff];

const AMB_REVERSED: &str = "for a range with Starting Handle greater than Ending Handle the clause names Invalid \
     Handle; 'no attributes will be returned' (Attribute Not Found) is true of such a range as well, and 3.4.1.1 \
     leaves the choice to the server 'if more than one error code applies'. The test requires Invalid Handle, the \
     code the clause gives for exactly this case";

pub fn tests() -> Vec<BleTest> {
    vec![
        BleTest {
            id: "att_read_by_type_values",
            doc: DOC,
            clause: "3.4.4.1, 3.4.4.2",
            req: "ATT_READ_BY_TYPE_RSP for a 16-bit and for a 128-bit attribute type: Length = 2 + value size, the \
                  lowest attribute of that type first with its handle and its value (the first ATT_MTU - 4 octets of \
                  a longer one), whole pairs only.",
            session: S,
            run: read_by_type_values,
            ..BleTest::D
        },
        BleTest {
            id: "att_read_by_type_uuid_forms",
            doc: DOC,
            clause: "3.4.4.1; Vol 3 Part B 2.5.1",
            req: "An attribute type given as a 16-bit UUID and as the same UUID in its 128-bit form find the same \
                  attributes.",
            session: S,
            run: read_by_type_uuid_forms,
            level: Level::Reading,
            ambiguous: Some(
                "the rule is a Note in 3.4.4.1 ('All attribute types are effectively compared as 128-bit UUIDs, even \
                 if a 16-bit UUID is provided in this request or defined for an attribute') resting on Part B 2.5.1 (a \
                 16-bit UUID 'actually represents a 128-bit UUID value'); read as binding for the server",
            ),
            ..BleTest::D
        },
        BleTest {
            id: "att_read_by_type_fill",
            doc: DOC,
            clause: "3.4.4.1, 3.4.4.2",
            req: "A Read By Type Response holds the attributes of the type from the lowest handle on, for as long as \
                  their values have the same size and fit into ATT_MTU - 2 octets: no fewer, and none of another size.",
            session: S,
            run: read_by_type_fill,
            ..BleTest::D
        },
        BleTest {
            id: "att_read_by_type_not_found",
            doc: DOC,
            clause: "3.4.4.1",
            req: "ATT_READ_BY_TYPE_REQ for a type that no attribute in the range has is answered with Attribute Not \
                  Found, Attribute Handle In Error = the Starting Handle.",
            session: S,
            run: read_by_type_not_found,
            ..BleTest::D
        },
        BleTest {
            id: "att_read_by_type_invalid_handles",
            doc: DOC,
            clause: "3.4.4.1",
            req: "ATT_READ_BY_TYPE_REQ with Starting Handle 0x0000 or greater than the Ending Handle is answered with \
                  Invalid Handle, Attribute Handle In Error = the Starting Handle.",
            session: S,
            run: read_by_type_invalid_handles,
            ambiguous: Some(AMB_REVERSED),
            ..BleTest::D
        },
        BleTest {
            id: "att_read_by_type_not_readable",
            doc: DOC,
            clause: "3.4.4.1",
            req: "ATT_READ_BY_TYPE_REQ whose first matching attribute cannot be read is answered with Read Not \
                  Permitted, Attribute Handle In Error = the handle of that attribute.",
            session: S,
            run: read_by_type_not_readable,
            ..BleTest::D
        },
        BleTest {
            id: "att_read_invalid_handles",
            doc: DOC,
            clause: "3.4.4.3, 3.2.2",
            req: "ATT_READ_REQ for handle 0x0000 or a handle without an attribute is answered with Invalid Handle.",
            session: S,
            run: read_invalid_handles,
            ..BleTest::D
        },
        BleTest {
            id: "att_read_long_value",
            doc: DOC,
            clause: "3.4.4.4, 3.4.4.6, 3.2.9",
            req: "Of a value longer than ATT_MTU - 1 octets a Read Response carries the first ATT_MTU - 1; Read Blob \
                  Responses carry ATT_MTU - 1 octets from the offset, or the rest when it is shorter.",
            session: S,
            run: read_long_value,
            mandatory: false,
            ..BleTest::D
        },
        BleTest {
            id: "att_read_blob_offsets",
            doc: DOC,
            clause: "3.4.4.5, 3.4.4.6",
            req: "ATT_READ_BLOB_RSP carries the value from the offset on; for an offset equal to the length of the \
                  value it is empty.",
            session: S,
            run: read_blob_offsets,
            mandatory: false,
            ..BleTest::D
        },
        BleTest {
            id: "att_read_blob_invalid_offset",
            doc: DOC,
            clause: "3.4.4.5",
            req: "ATT_READ_BLOB_REQ with an offset greater than the length of the value is answered with Invalid \
                  Offset.",
            session: S,
            run: read_blob_invalid_offset,
            mandatory: false,
            ..BleTest::D
        },
        BleTest {
            id: "att_read_blob_invalid_handles",
            doc: DOC,
            clause: "3.4.4.5",
            req: "ATT_READ_BLOB_REQ for handle 0x0000 or a handle without an attribute is answered with Invalid \
                  Handle.",
            session: S,
            run: read_blob_invalid_handles,
            mandatory: false,
            ..BleTest::D
        },
        BleTest {
            id: "att_read_by_group_type_fill",
            doc: DOC,
            clause: "3.4.4.9, 3.4.4.10",
            req: "A Read By Group Type Response for «Primary Service» holds whole Attribute Data of one size (handle, \
                  end group handle, value), from the lowest service in the range on, as many as have the same size \
                  and fit into ATT_MTU - 2 octets.",
            session: S,
            run: read_by_group_type_fill,
            ..BleTest::D
        },
        BleTest {
            id: "att_read_by_group_type_unsupported",
            doc: DOC,
            clause: "3.4.4.9; Part G 2.5.3",
            req: "ATT_READ_BY_GROUP_TYPE_REQ with a type that is not a grouping attribute usable in this request is \
                  answered with Unsupported Group Type, Attribute Handle In Error = the Starting Handle.",
            session: S,
            run: read_by_group_type_unsupported,
            ..BleTest::D
        },
        BleTest {
            id: "att_read_by_group_type_secondary",
            doc: DOC,
            clause: "3.4.4.9; Part G 2.5.3",
            req: "ATT_READ_BY_GROUP_TYPE_REQ for «Secondary Service» on a server without secondary services is \
                  answered with an ATT_ERROR_RSP naming the Starting Handle: Attribute Not Found or Unsupported Group \
                  Type.",
            session: S,
            run: read_by_group_type_secondary,
            ambiguous: Some(
                "Part G 2.5.3 lets «Secondary Service» be used in this request, so by 3.4.4.9 a server without such \
                 services answers Attribute Not Found; but 3.4.4.9 speaks of 'a supported grouping attribute', which \
                 a server that supports no secondary services may read as Unsupported Group Type. The test accepts \
                 both",
            ),
            ..BleTest::D
        },
        BleTest {
            id: "att_read_by_group_type_errors",
            doc: DOC,
            clause: "3.4.4.9",
            req: "ATT_READ_BY_GROUP_TYPE_REQ with Starting Handle 0x0000 or greater than the Ending Handle is answered \
                  with Invalid Handle, for a range without services with Attribute Not Found; Attribute Handle In \
                  Error = the Starting Handle.",
            session: S,
            run: read_by_group_type_errors,
            ambiguous: Some(AMB_REVERSED),
            ..BleTest::D
        },
    ]
}

// ---- helpers
fn le(v: u16) -> [u8; 2] {
    v.to_le_bytes()
}

fn cat(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

fn rev(b: &[u8]) -> Vec<u8> {
    b.iter().rev().copied().collect()
}

fn hxo(b: &Option<Vec<u8>>) -> String {
    b.as_deref().map(hx).unwrap_or_else(|| "None".into())
}

fn rbt(s: &mut BleSession, a: u16, b: u16, typ: &[u8]) -> R<Option<Vec<u8>>> {
    s.att(&cat(&[&[0x08], &le(a), &le(b), typ]))
}

fn rbgt(s: &mut BleSession, a: u16, b: u16, typ: &[u8]) -> R<Option<Vec<u8>>> {
    s.att(&cat(&[&[0x10], &le(a), &le(b), typ]))
}

/// [Attribute Data] of a Read By Type (op 0x09, head 2) or Read By Group Type
/// (0x11, head 4) response; None when it is not well formed.
fn list_(rsp: &Option<Vec<u8>>, op: u8, head: u8) -> Option<Vec<Vec<u8>>> {
    let r = rsp.as_ref()?;
    if r.len() < 2 || r[0] != op || r[1] <= head || r.len() < 2 + r[1] as usize || (r.len() - 2) % r[1] as usize != 0 {
        return None;
    }
    let w = r[1] as usize;
    Some((2..r.len()).step_by(w).map(|i| r[i..i + w].to_vec()).collect())
}

fn h16(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[0], b[1]])
}

fn case(what: String, pdu: Vec<u8>, codes: &[u8], handle: Option<u16>) -> Case {
    (what, pdu, codes.to_vec(), handle)
}

fn start_link(s: &mut BleSession) -> R<Gatt> {
    link(s, LinkOpts::new())?;
    table(s)
}

// ------------------------------------------------------------------ Read By Type
fn read_by_type_values(s: &mut BleSession) -> R<String> {
    // 3.4.4.1: the attributes with the lowest handles within the range. 3.4.4.2: complete handle-value
    // pairs, Length = the size of one pair, of a longer value the first (ATT_MTU - 4) octets. The types are
    // those of the first readable characteristic value with a 16-bit and with a 128-bit UUID; the value to
    // compare with comes from a Read Request (and Read Blob Requests).
    let g = start_link(s)?;
    let (mut notes, mut bad) = (Vec::<String>::new(), Vec::<String>::new());
    for size in [2usize, 16] {
        let mut found = None;
        for c in &g.chars {
            if c.uuid.len() == size && c.props & 0x02 != 0 {
                let r = read(s, c.value)?;
                if value(r.as_deref()).is_some() {
                    found = Some(c.clone());
                    break;
                }
            }
        }
        let Some(c) = found else {
            notes.push(format!("no readable characteristic with a {}-bit UUID", size * 8));
            continue;
        };
        let h0 = *g.attrs.iter().filter(|(_, u)| **u == c.uuid).map(|(h, _)| h).min().unwrap();
        let (mut v, mut rsp) = (None, None);
        for _ in 0..3 {
            // again when the value was another one after the request
            v = full_value(s, h0, MTU)?.0;
            rsp = rbt(s, 1, 0xFFFF, &c.uuid)?;
            if v.is_none() || full_value(s, h0, MTU)?.0 == v {
                break;
            }
        }
        let ut = hx(&rev(&c.uuid));
        let Some(v) = v else {
            notes.push(format!("attribute {h0:#06x} of type {ut} is not readable"));
            continue;
        };
        let want = cat(&[&le(h0), &v[..v.len().min(MTU - 4)]]);
        let items = list_(&rsp, 0x09, 2);
        match items {
            None => bad.push(format!(
                "type {ut}: {}; not a Length and whole handle-value pairs",
                describe(rsp.as_deref())
            )),
            Some(items) => {
                let r = rsp.as_ref().unwrap();
                if r.len() > MTU || items[0] != want {
                    bad.push(format!(
                        "type {ut}: {}; required Length {} and first pair {} (handle {h0:#06x}, value of {} octets)",
                        hx(r),
                        want.len(),
                        hx(&want),
                        v.len()
                    ));
                } else if items.iter().any(|it| g.attrs.get(&h16(it)) != Some(&c.uuid)) {
                    bad.push(format!("type {ut}: {} holds handles of other types", hx(r)));
                } else {
                    notes.push(format!(
                        "{}-bit type {ut}: handle {h0:#06x}, {} of {} value octets",
                        size * 8,
                        want.len() - 2,
                        v.len()
                    ));
                }
            }
        }
    }
    expect(bad.is_empty(), some(&bad, 5))?;
    if !notes.iter().any(|n| n.contains("type ") && !n.contains("not readable")) {
        return Err(Stop::Inconclusive(notes.join("; ")));
    }
    Ok(notes.join("; "))
}

fn read_by_type_uuid_forms(s: &mut BleSession) -> R<String> {
    // 3.4.4.1, Note: "All attribute types are effectively compared as 128-bit UUIDs, even if a 16-bit UUID
    // is provided in this request or defined for an attribute." Part B 2.5.1. Types: «Characteristic»
    // (0x2803) and the type of the first readable characteristic value with a 16-bit UUID.
    let g = start_link(s)?;
    let mut types = vec![0x2803u16];
    if let Some(c) = g.chars.iter().find(|c| c.uuid.len() == 2 && c.props & 0x02 != 0) {
        types.push(h16(&c.uuid));
    }
    let mut bad = Vec::new();
    for t in &types {
        let short = rbt(s, 1, 0xFFFF, &le(*t))?;
        if list_(&short, 0x09, 2).is_none() {
            return Err(Stop::Inconclusive(format!("type {t:#06x} in 2 octets: {}", describe(short.as_deref()))));
        }
        let long = rbt(s, 1, 0xFFFF, &uuid128(*t))?;
        if long != short {
            bad.push(format!(
                "type {t:#06x}: as 2 octets {}, as 16 octets ({}) {}",
                hxo(&short),
                hx(&uuid128(*t)),
                describe(long.as_deref())
            ));
        }
    }
    expect(bad.is_empty(), some(&bad, 5))?;
    Ok(format!(
        "types {}: the same response for the 2-octet and the 16-octet form",
        types.iter().map(|t| format!("{t:#06x}")).collect::<Vec<_>>().join(", ")
    ))
}

fn read_by_type_fill(s: &mut BleSession) -> R<String> {
    // 3.4.4.1: "The ATT Server shall include as many attributes as possible in the response"; values of
    // different lengths need several requests. 3.4.4.2: pairs in handle order, up to ATT_MTU - 2 octets.
    // Type «Characteristic»: declarations with 16-bit UUIDs are 5 octets, with 128-bit UUIDs 19. What each
    // declaration holds is first asked one handle at a time (start = end).
    let g = start_link(s)?;
    let decls: Vec<u16> = g.attrs.iter().filter(|(_, u)| u[..] == CHAR).map(|(h, _)| *h).collect();
    let mut pairs: Vec<(u16, Vec<u8>)> = Vec::new();
    for &h in &decls {
        let items = list_(&rbt(s, h, h, &CHAR)?, 0x09, 2);
        match items {
            Some(items) if items.len() == 1 && items[0][..2] == le(h) => pairs.push((h, items[0].clone())),
            _ => {
                return Err(Stop::Inconclusive(format!("declaration {h:#06x} asked alone: not one pair for that handle")))
            }
        }
    }
    if pairs.is_empty() {
        return Err(Stop::Inconclusive("no characteristic declarations".into()));
    }
    let mut starts = vec![1u16, decls[0], decls[decls.len() / 2], decls[decls.len() - 1]];
    for k in 1..pairs.len() {
        if pairs[k].1.len() != pairs[k - 1].1.len() {
            starts.push(pairs[k].0);
        }
    }
    starts.sort();
    starts.dedup();
    let (mut bad, mut sizes) = (Vec::<String>::new(), Vec::<i64>::new());
    for &a in &starts {
        let want = leading_run(&pairs, a, 0xFFFF, MTU);
        let rsp = rbt(s, a, 0xFFFF, &CHAR)?;
        let items = list_(&rsp, 0x09, 2);
        if !sizes.contains(&(want.len() as i64)) {
            sizes.push(want.len() as i64);
        }
        match items {
            None => bad.push(format!("from {a:#06x}: {}", describe(rsp.as_deref()))),
            Some(items) if items != want => {
                let r = rsp.as_ref().unwrap();
                bad.push(format!(
                    "from {a:#06x}: {} pairs of {} octets ({}); required {} of {} octets, handles {}",
                    items.len(),
                    r[1],
                    hx(r),
                    want.len(),
                    want.first().map(|w| w.len()).unwrap_or(0),
                    want.iter().map(|w| h16(w).to_string()).collect::<Vec<_>>().join(", ")
                ));
            }
            _ => {}
        }
    }
    expect(bad.is_empty(), some(&bad, 3))?;
    sizes.sort();
    Ok(format!(
        "{} declarations; requests from {} starting handles answered with {} pairs as the sizes allow",
        decls.len(),
        starts.len(),
        py::ints(sizes)
    ))
}

fn read_by_type_not_found(s: &mut BleSession) -> R<String> {
    // 3.4.4.1: no attribute of the type in the range -> Attribute Not Found (0x0A), Attribute Handle In
    // Error = the starting handle.
    let g = start_link(s)?;
    let absent = absent_uuid16(&g)?;
    let nx = g.last_handle().wrapping_add(1);
    let decls: Vec<u16> = g.attrs.iter().filter(|(_, u)| u[..] == CHAR).map(|(h, _)| *h).collect();
    let d1 = decls[0] + 1;
    let cases = vec![
        case(format!("16-bit type {absent:#06x}, which no attribute has"), cat(&[&[0x08], &ALL, &le(absent)]), &[0x0A], Some(1)),
        case(
            "a 128-bit type that no attribute has".into(),
            cat(&[&[0x08], &ALL, &(0x30u8..0x40).collect::<Vec<u8>>()]),
            &[0x0A],
            Some(1),
        ),
        case(
            "«Characteristic» above the last attribute".into(),
            cat(&[&[0x08], &le(nx), &le(0xFFFF), &CHAR]),
            &[0x0A],
            Some(nx),
        ),
        case(
            "«Characteristic» in the range of one characteristic value".into(),
            cat(&[&[0x08], &le(d1), &le(d1), &CHAR]),
            &[0x0A],
            Some(d1),
        ),
    ];
    let bad = error_cases(s, &cases, 2)?;
    expect(bad.is_empty(), some(&bad, 5))?;
    Ok(format!("{} requests without a match: Attribute Not Found with the Starting Handle", cases.len()))
}

fn read_by_type_invalid_handles(s: &mut BleSession) -> R<String> {
    // 3.4.4.1: Starting Handle greater than Ending Handle or 0x0000 -> Invalid Handle (0x01), Attribute
    // Handle In Error = the Starting Handle. The type is «Characteristic», which exists.
    start_link(s)?;
    let cases = vec![
        case("0x0000-0xFFFF".into(), cat(&[&[0x08], &le(0), &le(0xFFFF), &CHAR]), &[0x01], Some(0x0000)),
        case("0x0005-0x0002".into(), cat(&[&[0x08], &le(5), &le(2), &CHAR]), &[0x01], Some(0x0005)),
        case("0xFFFF-0x0001".into(), cat(&[&[0x08], &le(0xFFFF), &le(1), &CHAR]), &[0x01], Some(0xFFFF)),
    ];
    let bad = error_cases(s, &cases, 2)?;
    expect(bad.is_empty(), some(&bad, 5))?;
    Ok("Starting Handle 0x0000 and two reversed ranges: Invalid Handle with the Starting Handle".into())
}

fn val_of(vals: &[(u16, Option<Vec<u8>>)], h: u16) -> Option<Vec<u8>> {
    vals.iter().find(|x| x.0 == h).and_then(|x| x.1.clone())
}

fn read_by_type_not_readable(s: &mut BleSession) -> R<String> {
    // 3.4.4.1: a value that cannot be read due to permissions -> Read Not Permitted (0x02) with the handle
    // of the attribute causing the error. An attribute counts as unreadable when a Read Request on it is
    // answered with Read Not Permitted.
    let g = start_link(s)?;
    let vals = read_all(s)?;
    let locked: Vec<u16> = g
        .attrs
        .keys()
        .copied()
        .filter(|h| att_error(val_of(&vals, *h).as_deref()).map(|e| e.2).unwrap_or(0) == 0x02)
        .collect();
    if locked.is_empty() {
        return Err(Stop::NotApplicable(format!(
            "all {} attributes answer a Read Request with a value: no attribute that cannot be read",
            g.attrs.len()
        )));
    }
    let cases: Vec<Case> = locked
        .iter()
        .take(4)
        .map(|h| {
            case(
                format!("type of the unreadable attribute {h:#06x}"),
                cat(&[&[0x08], &le(*h), &le(*h), &g.attrs[h]]),
                &[0x02],
                Some(*h),
            )
        })
        .collect();
    let bad = error_cases(s, &cases, 2)?;
    expect(bad.is_empty(), some(&bad, 5))?;
    Ok(format!(
        "unreadable attributes {}: Read Not Permitted with their handle",
        locked.iter().take(4).map(|h| format!("{h:#06x}")).collect::<Vec<_>>().join(", ")
    ))
}

// ------------------------------------------------------------------ Read, Read Blob
fn read_invalid_handles(s: &mut BleSession) -> R<String> {
    // 3.4.4.3: an invalid handle -> Invalid Handle (0x01). 3.2.2: handle 0x0000 is reserved.
    let g = start_link(s)?;
    let lh = g.last_handle() as u32;
    let hs: Vec<u16> = [0u32, lh + 1, lh + 2, 0xFFFF]
        .into_iter()
        .filter(|h| *h <= 0xFFFF && !g.attrs.contains_key(&(*h as u16)))
        .map(|h| h as u16)
        .collect();
    let cases: Vec<Case> = hs.iter().map(|h| case(format!("handle {h:#06x}"), cat(&[&[0x0a], &le(*h)]), &[0x01], None)).collect();
    let bad = error_cases(s, &cases, 2)?;
    expect(bad.is_empty(), some(&bad, 5))?;
    Ok(format!(
        "handles {} (the table ends at {:#06x}): Invalid Handle",
        hs.iter().map(|h| format!("{h:#06x}")).collect::<Vec<_>>().join(", "),
        g.last_handle()
    ))
}

/// (handle, value) of the attributes longer than ATT_MTU - 1 octets, by Read Blob.
fn long_values(s: &mut BleSession, g: &Gatt) -> R<Vec<(u16, Vec<u8>)>> {
    let vals = read_all(s)?;
    let full: Vec<u16> = g
        .attrs
        .keys()
        .copied()
        .filter(|h| value(val_of(&vals, *h).as_deref()).is_some_and(|v| v.len() >= MTU - 1))
        .collect();
    if full.is_empty() {
        return Err(Stop::NotApplicable(format!(
            "no attribute value of more than {} octets: every Read Response is shorter than ATT_MTU - 1",
            MTU - 2
        )));
    }
    let (mut found, mut unknown) = (Vec::new(), Vec::<String>::new());
    for h in full {
        let (v, complete) = full_value(s, h, MTU)?;
        let v = v.unwrap_or_default();
        if v.len() > MTU - 1 && complete {
            found.push((h, v));
        } else if !complete {
            let b = blob(s, h, v.len() as u16)?;
            unknown.push(format!("{h:#06x}: {}", describe(b.as_deref())));
        }
    }
    if !found.is_empty() {
        return Ok(found);
    }
    if !unknown.is_empty() {
        return Err(Stop::Inconclusive(format!(
            "Read Responses of 22 octets whose attributes could not be read further with Read Blob: {}",
            unknown.iter().take(3).cloned().collect::<Vec<_>>().join("; ")
        )));
    }
    Err(Stop::NotApplicable("the longest attribute values have exactly 22 octets: none is longer than ATT_MTU - 1".into()))
}

fn slice(v: &[u8], off: usize, n: usize) -> Vec<u8> {
    if off >= v.len() {
        return Vec::new();
    }
    v[off..(off + n).min(v.len())].to_vec()
}

fn read_long_value(s: &mut BleSession) -> R<String> {
    // 3.4.4.4: of a longer value the first (ATT_MTU-1) octets. 3.4.4.6: (ATT_MTU-1) octets from Value Offset.
    // The value is put together from Read Blob Requests; the same value must then come out of requests at
    // other offsets. A value that changes by itself proves nothing: the next long attribute is taken.
    let g = start_link(s)?;
    let mut moving = Vec::new();
    for (h, v) in long_values(s, &g)? {
        let r = read(s, h)?;
        let rd = value(r.as_deref());
        let mut bad = Vec::new();
        if rd.as_deref() != Some(&v[..MTU - 1]) {
            let rdv = rd.clone().unwrap_or_default();
            bad.push(format!("Read Response with {} octets: {}", rdv.len(), hx(&rdv)));
        }
        let n = v.len();
        for off in [0, 1, 7, n - (MTU - 1), n - 5, n - 1, n] {
            let rsp = blob(s, h, off as u16)?;
            let want = slice(&v, off, MTU - 1);
            let ok = rsp.as_ref().is_some_and(|r| r[0] == 0x0D && r[1..] == want[..]);
            if !ok {
                bad.push(format!("offset {off}: {}; required {} octets ({})", describe(rsp.as_deref()), want.len(), hx(&want)));
            }
        }
        if !bad.is_empty() && full_value(s, h, MTU)?.0.as_deref() != Some(&v[..]) {
            moving.push(h);
            continue;
        }
        expect(bad.is_empty(), format!("handle {h:#06x}, a value of {} octets: {}", v.len(), some(&bad, 3)))?;
        return Ok(format!("handle {h:#06x}: {} octets, 22 by Read Request, the rest by Read Blob at any offset", v.len()));
    }
    Err(Stop::Inconclusive(format!(
        "the long values ({}) change between two reads",
        moving.iter().map(|h| format!("{h:#06x}")).collect::<Vec<_>>().join(", ")
    )))
}

/// (handle, value) of the Device Name, the attribute of variable length every GATT server has.
fn name(s: &mut BleSession, g: &Gatt) -> R<(u16, Vec<u8>)> {
    let Some(c) = g.char16(0x2A00).cloned() else {
        return Err(Stop::Inconclusive("no Device Name characteristic to read in parts".into()));
    };
    let (v, complete) = full_value(s, c.value, MTU)?;
    let v = match v {
        Some(v) if complete && v.len() >= 2 => v,
        _ => {
            return Err(Stop::Inconclusive(format!(
                "Device Name (handle {:#06x}) not readable as a value of 2 octets or more",
                c.value
            )))
        }
    };
    let rsp = blob(s, c.value, 0)?;
    if let Some(e) = att_error(rsp.as_deref()) {
        if e.2 == 0x06 {
            expect(
                e.1 == 0x0000,
                format!("ATT_READ_BLOB_REQ refused with Request Not Supported and handle {:#06x}; required handle 0x0000", e.1),
            )?;
            return Err(Stop::NotApplicable(
                "the server does not support ATT_READ_BLOB_REQ and says so (Request Not Supported)".into(),
            ));
        }
    }
    Ok((c.value, v))
}

fn read_blob_offsets(s: &mut BleSession) -> R<String> {
    // 3.4.4.5: a response when the offset is not greater than the length; for an offset equal to the
    // length the part is empty. The Device Name is 0 to 248 octets (Part C 12.1): not of fixed length.
    let g = start_link(s)?;
    let (h, v) = name(s, &g)?;
    let n = v.len();
    let mut offs = vec![0, 1, n / 2, n - 1, n];
    offs.sort();
    offs.dedup();
    let mut bad = Vec::new();
    for off in offs {
        let rsp = blob(s, h, off as u16)?;
        let want = slice(&v, off, MTU - 1);
        let ok = rsp.as_ref().is_some_and(|r| r[0] == 0x0D && r[1..] == want[..]);
        if !ok {
            bad.push(format!("offset {off}: {}; required 0x0d followed by {} octets", describe(rsp.as_deref()), want.len()));
        }
    }
    expect(bad.is_empty(), format!("Device Name ({n} octets, handle {h:#06x}): {}", some(&bad, 5)))?;
    Ok(format!("Device Name ({n} octets): parts from offsets 0 to {} as stored, offset {n} empty", n - 1))
}

fn read_blob_invalid_offset(s: &mut BleSession) -> R<String> {
    // 3.4.4.5: an offset greater than the length -> Invalid Offset (0x07). Control: the offset equal to the
    // length minus one is answered with a response.
    let g = start_link(s)?;
    let (h, v) = name(s, &g)?;
    let n = v.len();
    let ctl = blob(s, h, (n - 1) as u16)?;
    if !ctl.as_ref().is_some_and(|c| c[0] == 0x0D) {
        return Err(Stop::Inconclusive(format!("control: offset {} of the Device Name: {}", n - 1, describe(ctl.as_deref()))));
    }
    let cases: Vec<Case> = [n + 1, n + 9, 600, 0xFFFF]
        .iter()
        .map(|off| {
            case(
                format!("offset {off} of a value of {n} octets"),
                cat(&[&[0x0c], &le(h), &le(*off as u16)]),
                &[0x07],
                Some(h),
            )
        })
        .collect();
    let bad = error_cases(s, &cases, 2)?;
    expect(bad.is_empty(), some(&bad, 3))?;
    Ok(format!("Device Name ({n} octets): offsets {}, {}, 600, 65535 refused with Invalid Offset", n + 1, n + 9))
}

fn read_blob_invalid_handles(s: &mut BleSession) -> R<String> {
    // 3.4.4.5: an invalid handle -> Invalid Handle (0x01).
    let g = start_link(s)?;
    name(s, &g)?;
    let lh = g.last_handle() as u32;
    let hs: Vec<u16> = [0u32, lh + 1, 0xFFFF]
        .into_iter()
        .filter(|h| *h <= 0xFFFF && !g.attrs.contains_key(&(*h as u16)))
        .map(|h| h as u16)
        .collect();
    let mut cases = Vec::new();
    for h in &hs {
        for off in [0u16, 1] {
            cases.push(case(format!("handle {h:#06x}, offset {off}"), cat(&[&[0x0c], &le(*h), &le(off)]), &[0x01], None));
        }
    }
    let bad = error_cases(s, &cases, 2)?;
    expect(bad.is_empty(), some(&bad, 5))?;
    Ok(format!("handles {}: Invalid Handle", hs.iter().map(|h| format!("{h:#06x}")).collect::<Vec<_>>().join(", ")))
}

// ------------------------------------------------------------------ Read By Group Type
fn read_by_group_type_fill(s: &mut BleSession) -> R<String> {
    // 3.4.4.10: complete Attribute Data, ordered by handle, Length = the size of one. 3.4.4.9: the lowest
    // handles within the range, as many as possible. Each service declaration is first asked alone.
    let g = start_link(s)?;
    let decls: Vec<u16> = g.attrs.iter().filter(|(_, u)| u[..] == PRIMARY).map(|(h, _)| *h).collect();
    let mut data: Vec<(u16, Vec<u8>)> = Vec::new();
    for &h in &decls {
        let items = list_(&rbgt(s, h, h, &PRIMARY)?, 0x11, 4);
        match items {
            Some(items) if items.len() == 1 && items[0][..2] == le(h) => data.push((h, items[0].clone())),
            _ => {
                return Err(Stop::Inconclusive(format!(
                    "service declaration {h:#06x} asked alone: not one Attribute Data for that handle"
                )))
            }
        }
    }
    if data.is_empty() {
        return Err(Stop::Inconclusive("no primary service declarations".into()));
    }
    let mut a_set: Vec<u16> = vec![1, decls[0] + 1];
    a_set.extend(decls.iter().copied());
    a_set.sort();
    a_set.dedup();
    let mut ranges: Vec<(u16, u16)> = a_set.iter().map(|a| (*a, 0xFFFF)).collect();
    ranges.push((1, decls[0]));
    ranges.push((1, decls[decls.len() - 1]));
    let (mut bad, mut sizes) = (Vec::<String>::new(), Vec::<i64>::new());
    for &(a, b) in &ranges {
        let want = leading_run(&data, a, b, MTU);
        let rsp = rbgt(s, a, b, &PRIMARY)?;
        if want.is_empty() {
            if let Some(p) = error_problem(rsp.as_deref(), 0x10, &[0x0A], Some(a)) {
                bad.push(format!("{a:#06x}-{b:#06x}, no service declaration in it: {p}"));
            }
            continue;
        }
        let items = list_(&rsp, 0x11, 4);
        if !sizes.contains(&(want.len() as i64)) {
            sizes.push(want.len() as i64);
        }
        match items {
            None => bad.push(format!("{a:#06x}-{b:#06x}: {}", describe(rsp.as_deref()))),
            Some(_) if rsp.as_ref().unwrap().len() > MTU => bad.push(format!("{a:#06x}-{b:#06x}: {}", describe(rsp.as_deref()))),
            Some(items) if items != want => {
                let r = rsp.as_ref().unwrap();
                bad.push(format!(
                    "{a:#06x}-{b:#06x}: {} items of {} octets ({}); required {} of {} octets, services at {}",
                    items.len(),
                    r[1],
                    hx(r),
                    want.len(),
                    want[0].len(),
                    want.iter().map(|w| h16(w).to_string()).collect::<Vec<_>>().join(", ")
                ));
            }
            _ => {}
        }
    }
    expect(bad.is_empty(), some(&bad, 3))?;
    sizes.sort();
    Ok(format!(
        "{} services; {} ranges answered with {} items as sizes and ranges allow",
        decls.len(),
        ranges.len(),
        py::ints(sizes)
    ))
}

fn read_by_group_type_unsupported(s: &mut BleSession) -> R<String> {
    // 3.4.4.9: not a supported grouping attribute -> Unsupported Group Type (0x10), Starting Handle. Part G
    // 2.5.3: «Characteristic» shall not be used. Only types that attributes of this server have are asked.
    // Control: «Primary Service» is answered with a response.
    let g = start_link(s)?;
    let ctl = rbgt(s, 1, 0xFFFF, &PRIMARY)?;
    if !ctl.as_ref().is_some_and(|c| c[0] == 0x11) {
        return Err(Stop::Inconclusive(format!("control: «Primary Service» over the whole table: {}", describe(ctl.as_deref()))));
    }
    let mut types: Vec<(String, Vec<u8>)> = vec![("«Characteristic»".into(), CHAR.to_vec())];
    for size in [2usize, 16] {
        if let Some(c) = g.chars.iter().find(|c| c.uuid.len() == size) {
            types.push((format!("characteristic value type {}", hx(&rev(&c.uuid))), c.uuid.clone()));
        }
    }
    if let Some(d) = g.chars.iter().flat_map(|c| c.descs.iter().map(|(_, u)| u.clone())).next() {
        types.push((format!("descriptor type {}", hx(&rev(&d))), d));
    }
    let mut cases = Vec::new();
    for (name, t) in &types {
        for a in [1u16, 2] {
            cases.push(case(name.clone(), cat(&[&[0x10], &le(a), &le(0xFFFF), t]), &[0x10], Some(a)));
        }
    }
    let bad = error_cases(s, &cases, 2)?;
    expect(bad.is_empty(), format!("{} of {}: {}", bad.len(), cases.len(), some(&bad, 4)))?;
    Ok(format!(
        "{} existing types that are no service declarations: Unsupported Group Type with the Starting Handle",
        types.len()
    ))
}

fn read_by_group_type_secondary(s: &mut BleSession) -> R<String> {
    // 3.4.4.9: no attribute of the type in the range -> Attribute Not Found (0x0A); not a supported
    // grouping attribute -> Unsupported Group Type (0x10); either with the Starting Handle.
    let g = start_link(s)?;
    let sec: Vec<u16> = g.attrs.iter().filter(|(_, u)| u[..] == [0x01, 0x28]).map(|(h, _)| *h).collect();
    let rsp = rbgt(s, 1, 0xFFFF, &[0x01, 0x28])?;
    if !sec.is_empty() {
        let items = list_(&rsp, 0x11, 4);
        expect(
            items.is_some_and(|i| i[0][..2] == le(sec[0])),
            format!("secondary service at {:#06x}: {}", sec[0], describe(rsp.as_deref())),
        )?;
        return Ok(format!(
            "secondary services at {} returned",
            py::list(sec.iter().map(|h| format!("'{h:#x}'")))
        ));
    }
    let p = error_problem(rsp.as_deref(), 0x10, &[0x0A, 0x10], Some(1));
    expect(p.is_none(), format!("no secondary service in the table: {}", p.unwrap_or_default()))?;
    Ok(format!("no secondary services: {}", describe(rsp.as_deref())))
}

fn read_by_group_type_errors(s: &mut BleSession) -> R<String> {
    // 3.4.4.9: Starting Handle 0x0000 or greater than the Ending Handle -> Invalid Handle (0x01); no
    // attribute of the type in the range -> Attribute Not Found (0x0A); both with the Starting Handle.
    let g = start_link(s)?;
    let nx = g.last_handle().wrapping_add(1);
    let cases = vec![
        case("0x0000-0xFFFF".into(), cat(&[&[0x10], &le(0), &le(0xFFFF), &PRIMARY]), &[0x01], Some(0x0000)),
        case("0x0005-0x0002".into(), cat(&[&[0x10], &le(5), &le(2), &PRIMARY]), &[0x01], Some(0x0005)),
        case("0xFFFF-0x0001".into(), cat(&[&[0x10], &le(0xFFFF), &le(1), &PRIMARY]), &[0x01], Some(0xFFFF)),
        case(
            format!("{nx:#06x}-0xFFFF, above the last attribute"),
            cat(&[&[0x10], &le(nx), &le(0xFFFF), &PRIMARY]),
            &[0x0A],
            Some(nx),
        ),
    ];
    let bad = error_cases(s, &cases, 2)?;
    expect(bad.is_empty(), some(&bad, 5))?;
    Ok("Starting Handle 0x0000 and reversed ranges: Invalid Handle; above the table: Attribute Not Found".into())
}
