//! GATT: the Client Characteristic Configuration descriptor (Core 5.4 Vol 3
//! Part G 3.3.1.1, 3.3.3.3, 4.10, 4.12.3; Part F 3.4.5.1); emulator/spec_ble/gatt_cccd.py.

use crate::ble::harness::{att_error, hx, BleSession, Char, Gatt, LinkOpts};
use crate::ble::util::{cccds, describe, error_problem, link, mesh_app, read, some, stimulate, table, value};
use crate::ble::BleTest;
use crate::session::{expect, Stop, R};

const DOC: &str = "Core 5.4 Vol 3 Part G";

pub fn tests() -> Vec<BleTest> {
    vec![
        BleTest {
            id: "gatt_cccd_present",
            doc: DOC,
            clause: "3.3.1.1, 3.3.3.3",
            req: "Every characteristic with the Notify or Indicate property has exactly one Client Characteristic \
                  Configuration descriptor (0x2902) in its definition.",
            run: cccd_present,
            mandatory: false,
            session: "gatt",
            ..BleTest::D
        },
        BleTest {
            id: "gatt_cccd_default",
            doc: DOC,
            clause: "3.3.3.3",
            req: "A CCCD is readable, two octets long and 0x0000 on a new connection of a device that is not bonded, \
                  also when the connection before had set it.",
            run: cccd_default,
            mandatory: false,
            ..BleTest::D
        },
        BleTest {
            id: "gatt_cccd_write",
            doc: DOC,
            clause: "3.3.3.3, 4.12.3",
            req: "A CCCD is writable with a two-octet value: the write is answered with ATT_WRITE_RSP and the value \
                  reads back.",
            run: cccd_write,
            mandatory: false,
            ..BleTest::D
        },
        BleTest {
            id: "gatt_cccd_write_length",
            doc: "Core 5.4 Vol 3 Part F; Part G",
            clause: "F 3.4.5.1; G 3.3.3.3, 4.12.3",
            req: "A write of more than two octets to a CCCD is refused (Invalid Attribute Value Length, or an \
                  Application Error) and leaves the value as it was; a shorter write either changes only the octets \
                  written or is refused.",
            run: cccd_write_length,
            mandatory: false,
            ambiguous: Some(
                "for a value that is too long Part F 3.4.5.1 names Invalid Attribute Value Length (0x0D) and Part G \
                 4.12.3 an Application Error (0x80-0x9F) for a 'wrong size': both are accepted. For a value that is \
                 too short Part F says the octets given shall be written and the others left unchanged, Part G \
                 4.12.3 that a wrong size shall not be written: both are accepted",
            ),
            ..BleTest::D
        },
        BleTest {
            id: "gatt_cccd_bits_follow_properties",
            doc: DOC,
            clause: "3.3.3.3 (Table 3.11)",
            req: "The Notification bit of a CCCD can be set only for a characteristic with the Notify property, the \
                  Indication bit only with the Indicate property.",
            run: cccd_bits_follow_properties,
            mandatory: false,
            level: crate::ble::Level::Reading,
            ambiguous: Some(
                "'This value can only be set if the characteristic's properties have the indicate bit set' does not \
                 say who sees to it; read as binding the server (the bit must not read back as set). It can also be \
                 read as a rule for the client alone",
            ),
            ..BleTest::D
        },
        BleTest {
            id: "gatt_cccd_gates_notifications",
            doc: DOC,
            clause: "3.3.3.3, 4.10, 3.3.1.1",
            req: "A characteristic value is notified only while the Notification bit of its CCCD is set: not with the \
                  default 0x0000, not after the bit is cleared again.",
            run: cccd_gates_notifications,
            mandatory: false,
            ..BleTest::D
        },
    ]
}

fn rev(b: &[u8]) -> Vec<u8> {
    b.iter().rev().copied().collect()
}

fn opt_hx(v: &Option<Vec<u8>>) -> String {
    v.as_ref().map(|v| hx(v)).unwrap_or_else(|| "None".into())
}

fn write(s: &mut BleSession, h: u16, v: &[u8]) -> R<Option<Vec<u8>>> {
    let mut q = vec![0x12];
    q.extend_from_slice(&h.to_le_bytes());
    q.extend_from_slice(v);
    s.att(&q)
}

fn read_value(s: &mut BleSession, h: u16) -> R<Option<Vec<u8>>> {
    let r = read(s, h)?;
    Ok(value(r.as_deref()))
}

/// (table, characteristic, CCCD handle, the bit its properties allow) of the first CCCD; N/A without.
fn first_cccd(s: &mut BleSession) -> R<(Gatt, Char, u16, Vec<u8>)> {
    let g = table(s)?;
    let cc = cccds(&g);
    let Some((c, h)) = cc.first().cloned() else {
        return Err(Stop::NotApplicable("the server has no Client Characteristic Configuration descriptor".into()));
    };
    let on = if c.props & 0x10 != 0 { vec![0x01, 0x00] } else { vec![0x02, 0x00] };
    Ok((g, c, h, on))
}

const W13: [u8; 1] = [0x13];

fn cccd_present(s: &mut BleSession) -> R<String> {
    // 3.3.1.1, Table 3.5: Notify (0x10) "If set, the Client Characteristic Configuration Descriptor shall
    // exist."; Indicate (0x20) the same. 3.3.3.3: "Only one Client Characteristic Configuration declaration
    // shall exist in a characteristic definition."
    link(s, LinkOpts::new())?;
    let g = table(s)?;
    let need = g.chars.iter().filter(|c| c.props & 0x30 != 0).count();
    let mut bad = Vec::new();
    for c in &g.chars {
        let n = c.descs.iter().filter(|(_, u)| u[..] == [0x02, 0x29]).count();
        if c.props & 0x30 != 0 && n != 1 {
            let ds: Vec<String> = c.descs.iter().map(|(_, u)| hx(&rev(u))).collect();
            let ds = if ds.is_empty() { "none".to_string() } else { ds.join(", ") };
            bad.push(format!(
                "characteristic {} (properties {:#04x}, handles {:#06x}-{:#06x}) has {n} CCCDs; its descriptors: {ds}",
                hx(&rev(&c.uuid)),
                c.props,
                c.decl,
                c.end
            ));
        } else if n > 1 {
            bad.push(format!("characteristic {} has {n} CCCDs", hx(&rev(&c.uuid))));
        }
    }
    expect(bad.is_empty(), some(&bad, 5))?;
    if need == 0 {
        return Err(Stop::NotApplicable("no characteristic with the Notify or Indicate property".into()));
    }
    Ok(format!("{need} characteristic(s) with Notify or Indicate, each with one CCCD"))
}

fn cccd_default(s: &mut BleSession) -> R<String> {
    // 3.3.3.3: the CCCD value "shall be set to the default value at each connection with non-bonded devices";
    // default 0x0000; two octets; Table 3.10: readable without authentication or authorization.
    link(s, LinkOpts::new())?;
    let (_g, _c, h, on) = first_cccd(s)?;
    let v = read(s, h)?;
    expect(
        value(v.as_deref()).as_deref() == Some(&[0u8, 0][..]),
        format!("first connection after power-up: CCCD {h:#06x} reads {}; required 0000", describe(v.as_deref())),
    )?;
    let w = write(s, h, &on)?;
    let ok = w.as_deref() == Some(&W13[..]) && read_value(s, h)?.as_deref() == Some(&on[..]);
    if !ok {
        return Err(Stop::Inconclusive(format!(
            "the CCCD could not be set to {} ({}): nothing to reset",
            hx(&on),
            describe(w.as_deref())
        )));
    }
    link(s, LinkOpts::new())?;
    let v = read(s, h)?;
    expect(
        value(v.as_deref()).as_deref() == Some(&[0u8, 0][..]),
        format!(
            "CCCD {h:#06x} set to {} in one connection reads {} in the next; required 0000",
            hx(&on),
            describe(v.as_deref())
        ),
    )?;
    Ok(format!(
        "CCCD {h:#06x}: 0000 after power-up, and 0000 again in the connection after one that set it to {}",
        hx(&on)
    ))
}

fn cccd_write(s: &mut BleSession) -> R<String> {
    // 3.3.3.3: "shall be readable and writable." 4.12.3: ATT_WRITE_RSP if the write succeeded.
    link(s, LinkOpts::new())?;
    let (_g, _c, h, on) = first_cccd(s)?;
    for v in [on.clone(), vec![0, 0], on.clone()] {
        let w = write(s, h, &v)?;
        expect(
            w.as_deref() == Some(&W13[..]),
            format!("Write Request of {} to the CCCD {h:#06x}: {}; required 0x13", hx(&v), describe(w.as_deref())),
        )?;
        let got = read(s, h)?;
        expect(
            value(got.as_deref()).as_deref() == Some(&v[..]),
            format!("CCCD {h:#06x} after writing {}: {}", hx(&v), describe(got.as_deref())),
        )?;
    }
    Ok(format!("CCCD {h:#06x}: {}, 0000 and {} written and read back", hx(&on), hx(&on)))
}

fn cccd_write_length(s: &mut BleSession) -> R<String> {
    // Part G 3.3.3.3 (two octets), Part F 3.4.5.1 (too long: Invalid Attribute Value Length; shorter: the
    // octets given are written), Part G 4.12.3 (wrong size: not written, Application Error).
    link(s, LinkOpts::new())?;
    let (_g, _c, h, on) = first_cccd(s)?;
    let mut refuse = vec![0x0Du8];
    refuse.extend(0x80u8..0xA0);
    let (mut bad, mut notes) = (Vec::new(), Vec::new());
    let mut long1 = on.clone();
    long1.push(0);
    let mut long2 = on.clone();
    long2.extend_from_slice(&[0; 4]);
    for v in [long1, long2] {
        let w = write(s, h, &v)?;
        let p = error_problem(w.as_deref(), 0x12, &refuse, Some(h));
        let got = read_value(s, h)?;
        if let Some(p) = p {
            bad.push(format!("{} octets ({}): {p}", v.len(), hx(&v)));
        }
        if got.as_deref() != Some(&[0u8, 0][..]) {
            bad.push(format!(
                "after the write of {} octets the CCCD reads {}; required 0000 as before",
                v.len(),
                opt_hx(&got)
            ));
            write(s, h, &[0, 0])?;
        }
    }
    for v in [on[..1].to_vec(), Vec::new()] {
        let w = write(s, h, &v)?;
        let got = read_value(s, h)?;
        let mut want = v.clone();
        want.extend_from_slice(&[0u8, 0][v.len()..]);
        if w.as_deref() == Some(&W13[..]) && got.as_deref() == Some(&want[..]) {
            notes.push(format!("{} octet(s) written over the first octets", v.len()));
        } else if error_problem(w.as_deref(), 0x12, &refuse, Some(h)).is_none() && got.as_deref() == Some(&[0u8, 0][..]) {
            notes.push(format!("{} octet(s) refused", v.len()));
        } else {
            let hv = if v.is_empty() { "empty".to_string() } else { hx(&v) };
            bad.push(format!(
                "{} octet(s) ({hv}) written to a CCCD that read 0000: {}, then the CCCD reads {}; required either 0x13 \
                 and {}, or a refusal and 0000",
                v.len(),
                describe(w.as_deref()),
                opt_hx(&got),
                hx(&want)
            ));
        }
        write(s, h, &[0, 0])?;
    }
    expect(bad.is_empty(), some(&bad, 3))?;
    Ok(format!("CCCD {h:#06x}: 3 and 6 octets refused, value kept; {}", notes.join(", ")))
}

fn cccd_bits_follow_properties(s: &mut BleSession) -> R<String> {
    // Table 3.11: the Notification / Indication bit "can only be set if the characteristic's properties
    // have the notify / indicate bit set". Control: the bit the properties allow reads back as set.
    link(s, LinkOpts::new())?;
    let g = table(s)?;
    let cc = cccds(&g);
    if cc.is_empty() {
        return Err(Stop::NotApplicable("the server has no Client Characteristic Configuration descriptor".into()));
    }
    let (mut bad, mut notes, mut tried) = (Vec::new(), Vec::new(), 0);
    for (c, h) in &cc {
        let h = *h;
        for (bit, prop, name) in [(0x01u8, 0x10u8, "Notification"), (0x02, 0x20, "Indication")] {
            if c.props & prop != 0 {
                continue;
            }
            tried += 1;
            let w = write(s, h, &[bit, 0])?;
            let got = read_value(s, h)?;
            match &got {
                Some(g) if g.first().map_or(true, |b| b & bit == 0) && !g.is_empty() => {
                    let how = if att_error(w.as_deref()).is_some() { "refused" } else { "accepted, not set" };
                    notes.push(format!("{name} bit on {h:#06x}: {how}"));
                }
                _ => bad.push(format!(
                    "CCCD {h:#06x} of characteristic {} (properties {:#04x}): write of the {name} bit answered with \
                     {}, then the CCCD reads {}",
                    hx(&rev(&c.uuid)),
                    c.props,
                    describe(w.as_deref()),
                    opt_hx(&got)
                )),
            }
            write(s, h, &[0, 0])?;
        }
        let ok = vec![if c.props & 0x10 != 0 { 0x01 } else { 0x02 }, 0];
        write(s, h, &ok)?;
        if read_value(s, h)?.as_deref() != Some(&ok[..]) {
            return Err(Stop::Inconclusive(format!(
                "control: the bit the properties allow ({}) does not read back on {h:#06x}",
                hx(&ok)
            )));
        }
        write(s, h, &[0, 0])?;
    }
    expect(bad.is_empty(), some(&bad, 5))?;
    if tried == 0 {
        return Err(Stop::NotApplicable(
            "every characteristic with a CCCD has both the Notify and the Indicate property".into(),
        ));
    }
    Ok(notes.join("; "))
}

fn cccd_gates_notifications(s: &mut BleSession) -> R<String> {
    // 3.3.3.3: "When a bit is set, that action shall be enabled, otherwise it will not be used." Three rounds
    // of what makes the bulb notify: CCCD at its default, CCCD = 0x0001 (control), CCCD = 0x0000 again.
    link(s, LinkOpts::new())?;
    let g = table(s)?;
    // {value handle: CCCD handle}, in insertion order
    let mut cc: Vec<(u16, u16)> = Vec::new();
    for (c, h) in cccds(&g) {
        match cc.iter_mut().find(|(v, _)| *v == c.value) {
            Some(e) => e.1 = h,
            None => cc.push((c.value, h)),
        }
    }
    let n0 = s.lk().frames.len();
    let mut app = mesh_app(s)?;
    stimulate(s, &mut app, 500.0)?;
    let first = s.notifications(n0);
    let handle = |d: &[u8]| d[1] as u16 | (d[2] as u16) << 8;
    let loose: Vec<_> = first.iter().filter(|f| f.data.len() >= 3 && !cc.iter().any(|(v, _)| *v == handle(&f.data))).collect();
    if let Some(l0) = loose.first() {
        let h = handle(&l0.data);
        let c = g.chars.iter().find(|c| c.value == h);
        let mut m = format!("{} notification(s) on handle {h:#06x}", loose.len());
        if let Some(c) = c {
            m += &format!(" (characteristic {}, properties {:#04x})", hx(&rev(&c.uuid)), c.props);
        }
        m += ", which has no CCCD: sent without a Notification bit that a client could have set";
        if let Some(c) = c {
            let ds: Vec<String> = c.descs.iter().map(|(_, u)| hx(&rev(u))).collect();
            m += &format!("; its descriptors: {}", if ds.is_empty() { "none".to_string() } else { ds.join(", ") });
        }
        expect(false, m)?;
    }
    if cc.is_empty() {
        return Err(Stop::NotApplicable(
            "no CCCD, and no notification after login, the application's switch and two commands".into(),
        ));
    }
    for (_, h) in cc.clone() {
        write(s, h, &[0x01, 0x00])?;
    }
    let on = stimulate(s, &mut app, 500.0)?;
    for (_, h) in cc.clone() {
        write(s, h, &[0x00, 0x00])?;
    }
    s.link_run(300.0)?;
    let off = stimulate(s, &mut app, 500.0)?;
    if on.is_empty() && first.is_empty() && off.is_empty() {
        return Err(Stop::Inconclusive("no notification in any round: the stimulus does not make this bulb notify".into()));
    }
    if !first.is_empty() {
        expect(
            false,
            format!(
                "{} notification(s) with the CCCD at its default 0x0000 (then {} with the bit set, {} after clearing \
                 it): {}...",
                first.len(),
                on.len(),
                off.len(),
                hx(&first[0].data[..first[0].data.len().min(8)])
            ),
        )?;
    }
    expect(
        off.is_empty(),
        format!(
            "{} notification(s) after the Notification bit was cleared again ({} while it was set, none before)",
            off.len(),
            on.len()
        ),
    )?;
    if on.is_empty() {
        return Err(Stop::Inconclusive(
            "no notification while the Notification bit was set either: the gate was not shown".into(),
        ));
    }
    Ok(format!(
        "no notification with the CCCD at 0x0000, {} with the Notification bit set, none after clearing it",
        on.len()
    ))
}
