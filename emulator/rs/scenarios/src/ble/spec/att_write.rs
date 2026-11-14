//! ATT: Write Request and queued writes (Core 5.4 Vol 3 Part F 3.4.5.1,
//! 3.4.5.2, 3.4.6; Part G 3.1, 3.3.1); emulator/spec_ble/att_write.py.

use super::att_errors::cat;
use crate::ble::harness::{att_error, hx, BleSession, Gatt, LinkOpts};
use crate::ble::util::{cccds, describe, error_cases, link, read, some, table, u16b, value, Case};
use crate::ble::BleTest;
use crate::session::{expect, Stop, R};

const DOC: &str = "Core 5.4 Vol 3 Part F";

pub fn tests() -> Vec<BleTest> {
    vec![
        BleTest {
            id: "att_write_invalid_handles",
            doc: DOC,
            clause: "3.4.5.1, 3.2.2",
            req: "ATT_WRITE_REQ for handle 0x0000 or a handle without an attribute is answered with Invalid Handle.",
            run: write_invalid_handles,
            mandatory: false,
            ..BleTest::D
        },
        BleTest {
            id: "att_write_not_permitted",
            doc: "Core 5.4 Vol 3 Part F; Part G",
            clause: "F 3.4.5.1; G 3.1, 3.3.1",
            req: "ATT_WRITE_REQ on a service declaration or a characteristic declaration, which are read-only, is \
                  answered with Write Not Permitted naming the handle, and the value stays.",
            run: write_not_permitted,
            mandatory: false,
            ..BleTest::D
        },
        BleTest {
            id: "att_prepare_write",
            doc: DOC,
            clause: "3.4.6.1, 3.4.6.2, 3.4.6.3, 3.3",
            req: "ATT_PREPARE_WRITE_REQ is answered with a response repeating handle, offset and value, the attribute \
                  keeps its value, and Execute Write (cancel, or with nothing queued) is answered with \
                  ATT_EXECUTE_WRITE_RSP; a server without queued writes answers Request Not Supported, handle 0x0000.",
            run: prepare_write,
            mandatory: false,
            ..BleTest::D
        },
        BleTest {
            id: "att_queued_write_execute",
            doc: DOC,
            clause: "3.4.6.1, 3.4.6.3",
            req: "Prepared values are written only by ATT_EXECUTE_WRITE_REQ with flags 0x01, in the order they were \
                  prepared.",
            run: queued_write_execute,
            mandatory: false,
            ..BleTest::D
        },
        BleTest {
            id: "att_prepare_write_errors",
            doc: DOC,
            clause: "3.4.6.1",
            req: "ATT_PREPARE_WRITE_REQ for a handle without an attribute is answered with Invalid Handle, on a \
                  read-only attribute with Write Not Permitted, and such a request leaves nothing in the queue.",
            run: prepare_write_errors,
            mandatory: false,
            ..BleTest::D
        },
    ]
}

/// `hx(v) if v is not None else None`
fn ho(v: &Option<Vec<u8>>) -> String {
    v.as_ref().map(|v| hx(v)).unwrap_or_else(|| "None".into())
}

fn rv(s: &mut BleSession, h: u16) -> R<Option<Vec<u8>>> {
    let r = read(s, h)?;
    Ok(value(r.as_deref()))
}

/// N/A when the server has no ATT_WRITE_REQ (optional in a server, Part G 4.2) and says so.
fn write_supported(s: &mut BleSession, g: &Gatt) -> R<()> {
    let sv = g.services[0].start;
    let v = rv(s, sv)?.filter(|v| !v.is_empty()).unwrap_or_else(|| vec![0x00]);
    let rsp = s.att(&cat(&[&[0x12], &u16b(sv), &v]))?;
    if att_error(rsp.as_deref()).map_or(0, |e| e.2) == 0x06 {
        return Err(Stop::NotApplicable("the server does not support ATT_WRITE_REQ and says so (Request Not Supported)".into()));
    }
    Ok(())
}

fn free_handles(g: &Gatt) -> Vec<u16> {
    [0u32, g.last_handle() as u32 + 1, 0xFFFF]
        .into_iter()
        .filter(|h| *h <= 0xFFFF && !g.attrs.contains_key(&(*h as u16)))
        .map(|h| h as u16)
        .collect()
}

fn hlist(hs: &[u16]) -> String {
    hs.iter().map(|h| format!("{h:#06x}")).collect::<Vec<_>>().join(", ")
}

fn write_invalid_handles(s: &mut BleSession) -> R<String> {
    // 3.4.5.1: "If the handle is invalid, then an ATT_ERROR_RSP PDU shall be sent with the Error Code
    // parameter set to Invalid Handle (0x01)." 3.2.2: no attribute has handle 0x0000. The request is
    // optional in a server (Part G 4.2, C.2).
    link(s, LinkOpts::new())?;
    let g = table(s)?;
    write_supported(s, &g)?;
    let hs = free_handles(&g);
    let mut cases: Vec<Case> = Vec::new();
    for h in &hs {
        for v in [&[0x00u8][..], &[][..]] {
            cases.push((format!("handle {h:#06x}, {} octets", v.len()), cat(&[&[0x12], &u16b(*h), v]), vec![0x01], None));
        }
    }
    let bad = error_cases(s, &cases, 2)?;
    expect(bad.is_empty(), some(&bad, 5))?;
    Ok(format!("handles {}: Invalid Handle", hlist(&hs)))
}

fn write_not_permitted(s: &mut BleSession) -> R<String> {
    // Part G 3.1: a service declaration's "Attribute Permissions shall be read-only"; 3.3.1: "The Attribute
    // Value of a characteristic declaration is read only." Part F 3.4.5.1: "If the attribute value cannot
    // be written due to permissions then an ATT_ERROR_RSP PDU shall be sent with the Error Code parameter
    // set to Write Not Permitted (0x03)"; 3.4.1.1: handle of the request. The value written is the one the
    // attribute has with its first octet changed.
    link(s, LinkOpts::new())?;
    let g = table(s)?;
    write_supported(s, &g)?;
    let mut hs: Vec<u16> = g.services.iter().take(2).map(|sv| sv.start).collect();
    hs.extend(g.chars.iter().take(2).map(|c| c.decl));
    let mut before: Vec<(u16, Option<Vec<u8>>)> = Vec::new();
    for h in &hs {
        let v = rv(s, *h)?;
        match before.iter_mut().find(|(x, _)| x == h) {
            Some(e) => e.1 = v,
            None => before.push((*h, v)),
        }
    }
    if before.iter().any(|(_, v)| v.is_none()) {
        return Err(Stop::Inconclusive("a service or characteristic declaration is not readable".into()));
    }
    let bv = |h: u16| -> Vec<u8> { before.iter().find(|(x, _)| *x == h).unwrap().1.clone().unwrap() };
    let svc_hs: Vec<u16> = hs.iter().take(2).copied().collect();
    let cases: Vec<Case> = hs
        .iter()
        .map(|h| {
            let b = bv(*h);
            let mut nv = b.clone();
            if let Some(x) = nv.first_mut() {
                *x ^= 0x01;
            }
            (
                format!("{} declaration {h:#06x}", if svc_hs.contains(h) { "service" } else { "characteristic" }),
                cat(&[&[0x12], &u16b(*h), &nv]),
                vec![0x03],
                Some(*h),
            )
        })
        .collect();
    let mut bad = error_cases(s, &cases, 2)?;
    let mut changed = Vec::new();
    for h in &hs {
        let v = rv(s, *h)?;
        let b = bv(*h);
        if v.as_ref() != Some(&b) {
            changed.push(format!("{h:#06x}: {} -> {}", hx(&b), v.as_ref().map(|v| hx(v)).unwrap_or_else(|| "unreadable".into())));
        }
    }
    if !changed.is_empty() {
        bad.push(format!("values changed: {}", changed.join(", ")));
    }
    expect(bad.is_empty(), some(&bad, 5))?;
    Ok(format!("declarations {}: Write Not Permitted with the handle, values unchanged", hlist(&hs)))
}

// ------------------------------------------------------------------ queued writes
fn prepare(s: &mut BleSession, h: u16, off: u16, part: &[u8]) -> R<Option<Vec<u8>>> {
    s.att(&cat(&[&[0x16], &u16b(h), &u16b(off), part]))
}

/// An attribute that may be written: a CCCD if there is one, else the first
/// characteristic value with the Write property.
fn target(g: &Gatt) -> R<(u16, String)> {
    if let Some((_, h)) = cccds(g).first() {
        return Ok((*h, "Client Characteristic Configuration descriptor".into()));
    }
    let Some(c) = g.chars.iter().find(|c| c.props & 0x08 != 0) else {
        return Err(Stop::Inconclusive(
            "no writable attribute found (no CCCD, no characteristic with the Write property)".into(),
        ));
    };
    let mut u = c.uuid.clone();
    u.reverse();
    Ok((c.value, format!("value of characteristic {}", hx(&u))))
}

/// True when the request was refused with Request Not Supported; with `strict`
/// its handle must be 0x0000 (3.3).
fn unsupported(rsp: Option<&[u8]>, what: &str, strict: bool) -> R<bool> {
    if let Some(e) = att_error(rsp) {
        if e.2 == 0x06 {
            expect(
                !strict || e.1 == 0x0000,
                format!("{what} refused with Request Not Supported and handle {:#06x}; required handle 0x0000", e.1),
            )?;
            return Ok(true);
        }
    }
    Ok(false)
}

fn echo(h: u16, off: u16, v: &[u8]) -> Vec<u8> {
    cat(&[&[0x17], &u16b(h), &u16b(off), v])
}

fn prepare_write(s: &mut BleSession) -> R<String> {
    // 3.4.6.2: "The attribute handle shall be set to the same value as in the corresponding
    // ATT_PREPARE_WRITE_REQ PDU. The value offset and part attribute value shall be set to the same values
    // as in the corresponding ATT_PREPARE_WRITE_REQ PDU." 3.4.6.1: "The server shall not change the value
    // of the attribute until an ATT_EXECUTE_WRITE_REQ PDU is received." 3.4.6.3: "When the flags parameter
    // is set to 0x00 all pending prepare write values shall be discarded ... and an
    // ATT_EXECUTE_WRITE_RSP PDU shall be sent"; "If there are no pending prepared write values, then no
    // values are written, and an ATT_EXECUTE_WRITE_RSP PDU shall be sent." Queued writes are optional in a
    // server (Part G 4.2); 3.3 then requires Request Not Supported with handle 0x0000. Nothing is ever
    // executed here: the attribute is not changed by the test.
    link(s, LinkOpts::new())?;
    let g = table(s)?;
    let (h, what) = target(&g)?;
    let v0 = rv(s, h)?;
    let part = [0x01u8, 0x00];
    let rsp = prepare(s, h, 0, &part)?;
    expect(rsp.is_some(), format!("ATT_PREPARE_WRITE_REQ on the {what} ({h:#06x}): {}", describe(rsp.as_deref())))?;
    if unsupported(rsp.as_deref(), "ATT_PREPARE_WRITE_REQ", true)? {
        let ex = s.att(&[0x18, 0x00])?;
        let ok = ex.as_deref() == Some(&[0x19][..]) || unsupported(ex.as_deref(), "ATT_EXECUTE_WRITE_REQ", true)?;
        expect(ok, format!("ATT_EXECUTE_WRITE_REQ: {}", describe(ex.as_deref())))?;
        return Ok("queued writes not supported: Request Not Supported, handle 0x0000".into());
    }
    expect(
        rsp.as_deref() == Some(&echo(h, 0, &part)[..]),
        format!(
            "ATT_PREPARE_WRITE_REQ (handle {h:#06x}, offset 0, value {}): {}; required 0x17 with the same handle, \
             offset and value",
            hx(&part),
            describe(rsp.as_deref())
        ),
    )?;
    let rsp = prepare(s, h, 1, &[0xa5])?;
    expect(
        rsp.as_deref() == Some(&echo(h, 1, &[0xa5])[..]) || att_error(rsp.as_deref()).map_or(0, |e| e.2) == 0x09,
        format!(
            "second ATT_PREPARE_WRITE_REQ (offset 1, value a5): {}; required the echo or Prepare Queue Full",
            describe(rsp.as_deref())
        ),
    )?;
    let mid = rv(s, h)?;
    expect(mid == v0, format!("the value changed before any Execute Write: {} -> {}", ho(&v0), ho(&mid)))?;
    let ex = s.att(&[0x18, 0x00])?;
    expect(
        ex.as_deref() == Some(&[0x19][..]),
        format!("ATT_EXECUTE_WRITE_REQ, flags 0x00 (cancel): {}; required 0x19", describe(ex.as_deref())),
    )?;
    let end = rv(s, h)?;
    expect(
        end == v0,
        format!(
            "after ATT_EXECUTE_WRITE_REQ with flags 0x00 (cancel) the {what} reads {}, before {}; required unchanged, \
             the prepared values discarded",
            ho(&end),
            ho(&v0)
        ),
    )?;
    for (flags, why) in [(0x00u8, "cancel with nothing queued"), (0x01, "execute with nothing queued")] {
        let ex = s.att(&[0x18, flags])?;
        expect(
            ex.as_deref() == Some(&[0x19][..]),
            format!(
                "ATT_EXECUTE_WRITE_REQ, flags {flags:#04x} ({why}; the two prepared values had been cancelled with \
                 flags 0x00 and that was answered with 0x19): {}; required 0x19",
                describe(ex.as_deref())
            ),
        )?;
    }
    let end = rv(s, h)?;
    expect(
        end == v0,
        format!(
            "after an ATT_EXECUTE_WRITE_REQ with flags 0x01 that followed a cancel the {what} reads {}, before {}: the \
             cancelled values were written",
            ho(&end),
            ho(&v0)
        ),
    )?;
    Ok(format!("{what} ({h:#06x}): prepared values echoed, cancelled, value unchanged; empty queue executed"))
}

fn queued_write_execute(s: &mut BleSession) -> R<String> {
    // 3.4.6.1: "The server shall not change the value of the attribute until an ATT_EXECUTE_WRITE_REQ PDU
    // is received." 3.4.6.3: "When the flags parameter is set to 0x01, all pending prepare write values
    // that are currently queued shall be written in the order they were received in the corresponding
    // ATT_PREPARE_WRITE_REQ PDUs. The queue shall then be cleared and an ATT_EXECUTE_WRITE_RSP PDU shall
    // be sent." 3.4.6.1: "These will then be executed in the order received, causing multiple writes for
    // this attribute to occur." The attribute is a Client Characteristic Configuration descriptor: two
    // octets, readable and writable by Part G 3.3.3.3, so the written value can be read back.
    link(s, LinkOpts::new())?;
    let g = table(s)?;
    let cc = cccds(&g);
    let Some((c, h)) = cc.first().cloned() else {
        return Err(Stop::NotApplicable(
            "no Client Characteristic Configuration descriptor: no attribute whose written value the test can read \
             back without knowing the application"
                .into(),
        ));
    };
    let on: [u8; 2] = if c.props & 0x10 != 0 { [0x01, 0x00] } else { [0x02, 0x00] };
    let v0 = rv(s, h)?;
    let rsp = prepare(s, h, 0, &on)?;
    expect(rsp.is_some(), format!("ATT_PREPARE_WRITE_REQ: {}", describe(rsp.as_deref())))?;
    if unsupported(rsp.as_deref(), "ATT_PREPARE_WRITE_REQ", false)? {
        return Err(Stop::NotApplicable("the server does not support queued writes and says so (Request Not Supported)".into()));
    }
    expect(
        rsp.as_deref() == Some(&echo(h, 0, &on)[..]),
        format!("ATT_PREPARE_WRITE_REQ on the CCCD {h:#06x}: {}", describe(rsp.as_deref())),
    )?;
    let mid = rv(s, h)?;
    expect(
        mid == v0,
        format!(
            "CCCD {h:#06x} reads {} after the Prepare Write and before the Execute Write; required the old value {}",
            ho(&mid),
            ho(&v0)
        ),
    )?;
    let ex = s.att(&[0x18, 0x01])?;
    expect(
        ex.as_deref() == Some(&[0x19][..]),
        format!("ATT_EXECUTE_WRITE_REQ, flags 0x01: {}; required 0x19", describe(ex.as_deref())),
    )?;
    let got = rv(s, h)?;
    expect(
        got.as_deref() == Some(&on[..]),
        format!("CCCD {h:#06x} reads {} after the Execute Write; required the prepared value {}", ho(&got), hx(&on)),
    )?;
    // two prepared values for the same attribute: the later one stays
    for v in [&on[..], &[0x00, 0x00][..]] {
        let rsp = prepare(s, h, 0, v)?;
        expect(
            rsp.as_deref() == Some(&echo(h, 0, v)[..]),
            format!("ATT_PREPARE_WRITE_REQ with {}: {}", hx(v), describe(rsp.as_deref())),
        )?;
    }
    let ex = s.att(&[0x18, 0x01])?;
    expect(ex.as_deref() == Some(&[0x19][..]), format!("second ATT_EXECUTE_WRITE_REQ: {}", describe(ex.as_deref())))?;
    let got = rv(s, h)?;
    expect(
        got.as_deref() == Some(&[0x00, 0x00][..]),
        format!(
            "after preparing {} and then 0000 and executing, the CCCD reads {}; required 0000, the value prepared last",
            hx(&on),
            ho(&got)
        ),
    )?;
    Ok(format!(
        "CCCD {h:#06x}: unchanged until the Execute Write, then {}; of two prepared values the later one stays",
        hx(&on)
    ))
}

fn prepare_write_errors(s: &mut BleSession) -> R<String> {
    // 3.4.6.1: "If the handle is invalid, then an ATT_ERROR_RSP PDU shall be sent with the Error Code
    // parameter set to Invalid Handle (0x01). If the attribute value cannot be written then an
    // ATT_ERROR_RSP PDU shall be sent with the Error Code parameter set to Write Not Permitted (0x03)."
    // "If an ATT_PREPARE_WRITE_REQ PDU was invalid, and therefore an ATT_ERROR_RSP PDU has been issued,
    // then this prepared write will be considered to have not been received." Read-only: a service
    // declaration (Part G 3.1).
    link(s, LinkOpts::new())?;
    let g = table(s)?;
    let (h, _what) = target(&g)?;
    let probe = prepare(s, h, 0, &[])?;
    expect(probe.is_some(), format!("ATT_PREPARE_WRITE_REQ: {}", describe(probe.as_deref())))?;
    if unsupported(probe.as_deref(), "ATT_PREPARE_WRITE_REQ", false)? {
        return Err(Stop::NotApplicable("the server does not support queued writes and says so (Request Not Supported)".into()));
    }
    s.att(&[0x18, 0x00])?;
    let sv = g.services[0].start;
    let v0 = rv(s, sv)?.unwrap_or_default();
    let hs = free_handles(&g);
    let mut cases: Vec<Case> = hs
        .iter()
        .map(|x| (format!("handle {x:#06x}"), cat(&[&[0x16], &u16b(*x), &u16b(0), &[0x00]]), vec![0x01], None))
        .collect();
    let mut nv = v0.clone();
    if let Some(x) = nv.first_mut() {
        *x ^= 1;
    }
    cases.push((format!("service declaration {sv:#06x}"), cat(&[&[0x16], &u16b(sv), &u16b(0), &nv]), vec![0x03], Some(sv)));
    let mut bad = error_cases(s, &cases, 2)?;
    let ex = s.att(&[0x18, 0x01])?;
    if ex.as_deref() != Some(&[0x19][..]) {
        bad.push(format!("ATT_EXECUTE_WRITE_REQ after the refused requests: {}; required 0x19", describe(ex.as_deref())));
    }
    let v1 = rv(s, sv)?;
    if v1.as_ref() != Some(&v0) {
        bad.push(format!(
            "the service declaration reads {} after the Execute Write, before {}",
            ho(&v1),
            hx(&v0)
        ));
    }
    expect(bad.is_empty(), some(&bad, 5))?;
    Ok(format!(
        "handles {}: Invalid Handle; service declaration: Write Not Permitted; nothing queued",
        hlist(&hs)
    ))
}
