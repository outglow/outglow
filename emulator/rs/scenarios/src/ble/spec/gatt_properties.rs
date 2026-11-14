//! GATT: characteristic properties against what the server lets a client do,
//! and the Characteristic User Description (Core 5.4 Vol 3 Part G 3.3.1.1,
//! 3.3.3.1, 3.3.3.2, 4.8.1, 4.9.1, 4.9.3); emulator/spec_ble/gatt_properties.py.

use crate::ble::harness::{att_error, hx, BleSession, Char, LinkOpts};
use crate::ble::util::{describe, error_problem, link, read, read_all, some, table, value, SECURITY};
use crate::ble::{BleTest, Level};
use crate::py;
use crate::session::{expect, Stop, R};

const DOC: &str = "Core 5.4 Vol 3 Part G";
const READING: &str = "Table 3.5 says what a set bit permits ('If set, permits ...'); that the action is not \
                       permitted when the bit is clear is the reading taken here. The text does not say so in as \
                       many words, and 3.3.2 leaves the permissions of the value attribute to the service or the \
                       implementation";

pub fn tests() -> Vec<BleTest> {
    vec![
        BleTest {
            id: "gatt_props_read",
            doc: DOC,
            clause: "3.3.1.1 (Table 3.5), 4.8.1",
            req: "A characteristic value with the Read property answers a Read Request with its value; one without \
                  it answers with Read Not Permitted.",
            run: props_read,
            session: "gatt",
            level: Level::Reading,
            ambiguous: Some(READING),
            ..BleTest::D
        },
        BleTest {
            id: "gatt_props_write",
            doc: DOC,
            clause: "3.3.1.1 (Table 3.5), 4.9.3",
            req: "A characteristic value without the Write property refuses a Write Request with Write Not Permitted \
                  and keeps its value; one with the property does not answer with Write Not Permitted.",
            run: props_write,
            mandatory: false,
            level: Level::Reading,
            ambiguous: Some(READING),
            ..BleTest::D
        },
        BleTest {
            id: "gatt_props_write_without_response",
            doc: DOC,
            clause: "3.3.1.1 (Table 3.5), 4.9.1; Part F 3.4.5.3",
            req: "A Write Command to a characteristic value without the Write Without Response property does not \
                  change the value.",
            run: props_write_without_response,
            mandatory: false,
            level: Level::Reading,
            ambiguous: Some(READING),
            ..BleTest::D
        },
        BleTest {
            id: "gatt_user_description",
            doc: DOC,
            clause: "3.3.3.2, 3.3.3.1 (Table 3.8)",
            req: "A Characteristic User Description (0x2901) holds a UTF-8 string and is writable only if the \
                  characteristic's Extended Properties have Writable Auxiliaries.",
            run: user_description,
            mandatory: false,
            level: Level::Reading,
            ambiguous: Some(
                "3.3.3.2: 'If the Writable Auxiliaries bit of the Characteristic Extended Properties is set then \
                 this characteristic descriptor can be written'; that it cannot be written otherwise is the reading \
                 taken (its permissions are otherwise 'specified by the profile or may be implementation specific')",
            ),
            ..BleTest::D
        },
    ]
}

fn rev(b: &[u8]) -> Vec<u8> {
    b.iter().rev().copied().collect()
}

fn name(c: &Char) -> String {
    format!("{} (value {:#06x}, properties {:#04x})", hx(&rev(&c.uuid)), c.value, c.props)
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

fn props_read(s: &mut BleSession) -> R<String> {
    // Table 3.5: Read (0x02) permits reads (4.8); 4.8.1: ATT_ERROR_RSP when a read is not permitted, Part F
    // 3.4.4.3: Read Not Permitted (0x02). An error asking for security is not counted against a set Read bit.
    link(s, LinkOpts::new())?;
    let g = table(s)?;
    let vals = read_all(s)?;
    let (mut bad, mut n_r, mut n_nr, mut sec) = (Vec::new(), 0, 0, 0);
    for c in &g.chars {
        let rsp: Option<&[u8]> = vals.iter().find(|(h, _)| *h == c.value).and_then(|(_, r)| r.as_deref());
        let e = att_error(rsp);
        if c.props & 0x02 != 0 {
            n_r += 1;
            if e.is_some_and(|e| SECURITY.contains(&e.2)) {
                sec += 1;
            } else if value(rsp).is_none() {
                bad.push(format!("{} has the Read property; Read Request: {}", name(c), describe(rsp)));
            }
        } else {
            n_nr += 1;
            if let Some(p) = error_problem(rsp, 0x0A, &[0x02], Some(c.value)) {
                bad.push(format!("{} lacks the Read property; Read Request: {p}", name(c)));
            }
        }
    }
    expect(bad.is_empty(), some(&bad, 4))?;
    Ok(format!(
        "{n_r} characteristics with Read answered with a value{}, {n_nr} without Read refused",
        if sec > 0 { format!(" ({sec} asked for security)") } else { String::new() }
    ))
}

fn props_write(s: &mut BleSession) -> R<String> {
    // Table 3.5: Write (0x08) permits writes with response; 4.9.3: ATT_ERROR_RSP when a write is not
    // permitted, Part F 3.4.5.1: Write Not Permitted (0x03).
    link(s, LinkOpts::new())?;
    let g = table(s)?;
    let (mut bad, mut n_w, mut n_nw) = (Vec::new(), 0, 0);
    let resets = s.resets();
    for c in &g.chars {
        let v0 = read_value(s, c.value)?;
        let rsp;
        if c.props & 0x08 != 0 {
            n_w += 1;
            let mut v = match &v0 {
                Some(v) if !v.is_empty() => v.clone(),
                _ => vec![0x00],
            };
            v.truncate(20);
            rsp = write(s, c.value, &v)?;
            let e = att_error(rsp.as_deref());
            if rsp.is_none() || e.is_some_and(|e| e.2 == 0x03) {
                bad.push(format!("{} has the Write property; Write Request: {}", name(c), describe(rsp.as_deref())));
            }
        } else {
            n_nw += 1;
            let mut new = match &v0 {
                Some(v) if !v.is_empty() => {
                    let mut n = vec![v[0] ^ 0x01];
                    n.extend_from_slice(&v[1..]);
                    n
                }
                _ => vec![0x01],
            };
            new.truncate(20);
            rsp = write(s, c.value, &new)?;
            let p = error_problem(rsp.as_deref(), 0x12, &[0x03], Some(c.value));
            let v1 = read_value(s, c.value)?;
            if let Some(p) = p {
                bad.push(format!("{} lacks the Write property; Write Request: {p}", name(c)));
            }
            if let Some(v1) = &v1 {
                if Some(v1) != v0.as_ref() && v1.len() >= new.len() && v1[..new.len()] == new[..] {
                    bad.push(format!(
                        "{} lacks the Write property; its value went from {} to the one written, {}",
                        name(c),
                        v0.as_ref().map(|v| hx(v)).unwrap_or_else(|| "None".into()),
                        hx(v1)
                    ));
                }
            }
        }
        if rsp.is_none() {
            link(s, LinkOpts::new())?;
        }
        if s.resets() != resets || !s.alive(3000.0)? {
            return Err(Stop::Inconclusive(format!("the bulb reset or dropped the link after the write to {}", name(c))));
        }
    }
    expect(bad.is_empty(), some(&bad, 4))?;
    Ok(format!(
        "{n_nw} characteristics without Write refused with Write Not Permitted, {n_w} with Write did not refuse"
    ))
}

fn props_write_without_response(s: &mut BleSession) -> R<String> {
    // Table 3.5: Write Without Response (0x04); Part F 3.4.5.3: a command the server cannot carry out is
    // ignored. Seen through the value, with a Write Request as control where the characteristic has Write.
    link(s, LinkOpts::new())?;
    let g = table(s)?;
    let (mut bad, mut shown, mut blind, mut answered) = (Vec::new(), 0usize, 0usize, 0usize);
    for c in &g.chars {
        if c.props & 0x04 != 0 {
            continue;
        }
        let Some(v0) = read_value(s, c.value)?.filter(|v| !v.is_empty()) else { continue };
        let mut new = vec![v0[0] ^ 0x01];
        new.extend_from_slice(&v0[1..]);
        new.truncate(20);
        let mut q = vec![0x52];
        q.extend_from_slice(&c.value.to_le_bytes());
        q.extend_from_slice(&new);
        let ans = s.att_quiet(&q, 500.0)?;
        let v1 = read_value(s, c.value)?;
        answered += usize::from(!ans.is_empty());
        let v0_20 = &v0[..v0.len().min(20)];
        if let Some(x) = &v1 {
            if *x != v0 && x.len() >= new.len() && x[..new.len()] == new[..] {
                bad.push(format!(
                    "{} lacks Write Without Response; a Write Command changed its value from {} to the one written, {}",
                    name(c),
                    hx(&v0),
                    hx(x)
                ));
                if c.props & 0x08 != 0 {
                    write(s, c.value, v0_20)?;
                }
                continue;
            }
        }
        if v1.as_ref() != Some(&v0) {
            blind += 1; // the value changes by itself: it shows nothing
            continue;
        }
        if c.props & 0x08 == 0 {
            shown += 1; // read-only: nothing else could have changed it
            continue;
        }
        let w = write(s, c.value, &new)?;
        let v2 = read_value(s, c.value)?;
        let mut expect_v = new.clone();
        expect_v.extend_from_slice(v0.get(20..).unwrap_or(&[]));
        if w.as_deref() == Some(&[0x13u8][..]) && v2.as_ref() == Some(&expect_v) {
            shown += 1;
            write(s, c.value, v0_20)?;
        } else {
            blind += 1;
        }
        if !s.alive(3000.0)? {
            return Err(Stop::Inconclusive(format!("the link ended after the writes to {}", name(c))));
        }
    }
    expect(bad.is_empty(), some(&bad, 4))?;
    if shown == 0 {
        return Err(Stop::Inconclusive(
            "no characteristic without Write Without Response whose value shows a write".into(),
        ));
    }
    let mut out = format!("{shown} characteristics without Write Without Response unchanged by a Write Command");
    if blind > 0 {
        out += &format!(
            "; {blind} more whose value changes by itself or does not show a Write Request either (not counted)"
        );
    }
    if answered > 0 {
        out += &format!("; {answered} of the commands were answered with an ATT_ERROR_RSP");
    }
    Ok(out)
}

fn user_description(s: &mut BleSession) -> R<String> {
    // 3.3.3.2: a UTF-8 string; writable if the Writable Auxiliaries bit is set (Table 3.8, bit 1). A refusal
    // must be Write Not Permitted (Part F 3.4.5.1) and leave the string as it was.
    link(s, LinkOpts::new())?;
    let g = table(s)?;
    let mut ds: Vec<(Char, u16)> = Vec::new();
    for c in &g.chars {
        for (h, u) in &c.descs {
            if u[..] == [0x01, 0x29] {
                ds.push((c.clone(), *h));
            }
        }
    }
    if ds.is_empty() {
        return Err(Stop::NotApplicable("no Characteristic User Description descriptor".into()));
    }
    let (mut bad, mut texts) = (Vec::new(), Vec::new());
    for (c, h) in &ds {
        let h = *h;
        let rsp = read(s, h)?;
        let Some(v) = value(rsp.as_deref()) else {
            let code = att_error(rsp.as_deref()).map(|e| e.2).unwrap_or(0);
            if code != 0x02 && !SECURITY.contains(&code) {
                bad.push(format!("descriptor {h:#06x}: {}", describe(rsp.as_deref())));
            }
            continue;
        };
        match py::utf8(&v) {
            Ok(t) => texts.push(t),
            Err(_) => bad.push(format!("descriptor {h:#06x}: {} is not UTF-8", hx(&v))),
        }
        let ext = c.descs.iter().find(|(_, u)| u[..] == [0x00, 0x29]).map(|(x, _)| *x);
        let aux = match ext {
            Some(x) if c.props & 0x80 != 0 => {
                let ev = read_value(s, x)?.filter(|e| !e.is_empty()).unwrap_or_else(|| vec![0]);
                ev[0] & 0x02 != 0
            }
            _ => false,
        };
        if aux {
            continue;
        }
        let new = if v.is_empty() {
            b"x".to_vec()
        } else {
            let mut n = vec![v[0] ^ 0x01];
            n.extend_from_slice(&v[1..]);
            n
        };
        let w = write(s, h, &new)?;
        let p = error_problem(w.as_deref(), 0x12, &[0x03], Some(h));
        let v1 = read_value(s, h)?;
        if let Some(p) = p {
            bad.push(format!("descriptor {h:#06x} of a characteristic without Writable Auxiliaries, Write Request: {p}"));
        }
        if v1.as_ref() != Some(&v) {
            bad.push(format!(
                "descriptor {h:#06x} went from {} to {}",
                py::bytes_repr(&v),
                v1.as_ref().map(|x| py::bytes_repr(x)).unwrap_or_else(|| "None".into())
            ));
        }
    }
    expect(bad.is_empty(), some(&bad, 3))?;
    Ok(format!(
        "{} descriptors: {}; none writable, none has Writable Auxiliaries",
        ds.len(),
        texts.iter().map(|t| py::str_repr(t)).collect::<Vec<_>>().join(", ")
    ))
}
