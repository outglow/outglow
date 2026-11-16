use crate::ble::harness::{hx, parse_ad, BleSession, Gatt, LinkOpts, Service};
use crate::ble::util::{describe, full_value, link, some, table, value, MTU};
use crate::ble::BleTest;
use crate::py;
use crate::session::{expect, Stop, R};

const DOC: &str = "Core 5.4 Vol 3 Part C";
const GAP: [u8; 2] = [0x00, 0x18];
const NAME: [u8; 2] = [0x00, 0x2A];
const APPEARANCE: [u8; 2] = [0x01, 0x2A];

pub fn tests() -> Vec<BleTest> {
    vec![
        BleTest {
            id: "gatt_gap_service",
            doc: DOC,
            clause: "12 (Tables 12.1, 12.2)",
            req: "A GATT server in the Peripheral role has exactly one GAP service (0x1800), a primary service, with \
                  one Device Name and one Appearance characteristic in it.",
            run: gap_service,
            session: "gatt",
            ..BleTest::D
        },
        BleTest {
            id: "gatt_gap_device_name",
            doc: DOC,
            clause: "12.1, 3.2.2.3",
            req: "The Device Name of a discoverable device is readable without security, 0 to 248 octets long and \
                  valid UTF-8.",
            run: gap_device_name,
            session: "gatt",
            ..BleTest::D
        },
        BleTest {
            id: "gatt_gap_appearance",
            doc: DOC,
            clause: "12.2",
            req: "The Appearance is readable without security and two octets long.",
            run: gap_appearance,
            session: "gatt",
            ..BleTest::D
        },
        BleTest {
            id: "gatt_gap_name_advertised",
            doc: "CSS 11 Part A",
            clause: "1.2; Core 5.4 Vol 3 Part C 3.2.2.1",
            req: "A Complete Local Name in the advertising or scan response data equals the Device Name \
                  characteristic; a Shortened Local Name is its beginning.",
            run: gap_name_advertised,
            mandatory: false,
            session: "gatt",
            ..BleTest::D
        },
    ]
}

fn rev(b: &[u8]) -> Vec<u8> {
    b.iter().rev().copied().collect()
}

fn read_req(h: u16) -> Vec<u8> {
    let mut q = vec![0x0a];
    q.extend_from_slice(&h.to_le_bytes());
    q
}

fn gap(g: &Gatt) -> R<Vec<Service>> {
    let sv: Vec<Service> = g.services.iter().filter(|x| x.uuid == GAP).cloned().collect();
    expect(
        !sv.is_empty(),
        format!(
            "no GAP service (0x1800) among the primary services: {}",
            g.services.iter().map(|x| hx(&rev(&x.uuid))).collect::<Vec<_>>().join(", ")
        ),
    )?;
    Ok(sv)
}

fn gap_service(s: &mut BleSession) -> R<String> {
    link(s, LinkOpts::new())?;
    let g = table(s)?;
    let sv = gap(&g)?;
    expect(
        sv.len() == 1 && sv[0].primary,
        format!("{} GAP services at {}", sv.len(), py::list(sv.iter().map(|x| py::hexq(x.start as u64)))),
    )?;
    let mut bad = Vec::new();
    for (u, name) in [(NAME, "Device Name"), (APPEARANCE, "Appearance")] {
        let cs: Vec<_> = g.chars.iter().filter(|c| c.uuid == u).collect();
        let inside = cs.iter().filter(|c| sv[0].start < c.decl && c.decl <= sv[0].end).count();
        if cs.len() != 1 || inside != 1 {
            bad.push(format!("{name}: {} characteristic(s) on the server, {inside} in the GAP service", cs.len()));
        }
    }
    expect(bad.is_empty(), bad.join("; "))?;
    Ok(format!("GAP service {:#06x}-{:#06x} with one Device Name and one Appearance", sv[0].start, sv[0].end))
}

fn gap_device_name(s: &mut BleSession) -> R<String> {
    let adv = s.wait_adv(1, 5000.0, None, 0)?;
    let flags: Vec<u8> = match adv.first() {
        Some(p) => parse_ad(p.data()).0.into_iter().find(|(t, _)| *t == 0x01).map(|(_, v)| v).unwrap_or_default(),
        None => Vec::new(),
    };
    link(s, LinkOpts::new())?;
    let g = table(s)?;
    let c = g.char_uuid(&NAME).cloned();
    expect(c.is_some(), "no Device Name characteristic")?;
    let c = c.unwrap();
    if !(flags.first().is_some_and(|f| f & 0x03 != 0)) {
        let f = if flags.is_empty() { "absent".to_string() } else { hx(&flags) };
        return Err(Stop::NotApplicable(format!(
            "the bulb is not discoverable (Flags {f}): the name need not be readable"
        )));
    }
    let (v, complete) = full_value(s, c.value, MTU)?;
    let again = s.att(&read_req(c.value))?;
    expect(
        v.is_some(),
        format!(
            "Device Name (handle {:#06x}) of a discoverable device (Flags {}): {}",
            c.value,
            hx(&flags),
            describe(again.as_deref())
        ),
    )?;
    let v = v.unwrap();
    expect(
        complete && v.len() <= 248,
        format!("Device Name of {} octets{}", v.len(), if complete { "" } else { " or more" }),
    )?;
    let text = match py::utf8(&v) {
        Ok(t) => t,
        Err(e) => return Err(Stop::Fail(format!("Device Name {} is not UTF-8: {e}", hx(&v)))),
    };
    Ok(format!(
        "{}, {} octets, readable on a plain link{}",
        py::str_repr(&text),
        v.len(),
        if v.last() == Some(&0) { " (the value ends with a NUL octet)" } else { "" }
    ))
}

fn gap_appearance(s: &mut BleSession) -> R<String> {
    link(s, LinkOpts::new())?;
    let g = table(s)?;
    let c = g.char_uuid(&APPEARANCE).cloned();
    expect(c.is_some(), "no Appearance characteristic")?;
    let c = c.unwrap();
    let rsp = s.att(&read_req(c.value))?;
    let v = value(rsp.as_deref());
    expect(
        v.as_ref().is_some_and(|v| v.len() == 2),
        format!("Appearance (handle {:#06x}): {}; required a value of 2 octets", c.value, describe(rsp.as_deref())),
    )?;
    let v = v.unwrap();
    Ok(format!("Appearance {:#06x}, readable on a plain link", v[0] as u16 | (v[1] as u16) << 8))
}

fn gap_name_advertised(s: &mut BleSession) -> R<String> {
    s.address()?;
    let adv = s.wait_adv(1, 5000.0, None, 0)?;
    if adv.is_empty() {
        return Err(Stop::Inconclusive("no ADV_IND".into()));
    }
    let t0 = s.now();
    let n0 = s.adv_sent(0)?.len();
    let sr = s.scan_req_default()?;
    s.adv_reply(&sr, 2, -1)?;
    s.until(4000.0, 20.0, |s| Ok(if s.adv_sent(0)?.len() >= n0 + 2 { Some(()) } else { None }))?;
    s.run(20.0)?;
    s.adv_reply_clear()?;
    let rsp: Vec<_> = s.adv(t0, false).into_iter().filter(|p| p.ty() == 4).collect();
    let places: [(&str, Vec<u8>); 2] = [
        ("advertising data", adv[0].data().to_vec()),
        ("scan response data", rsp.first().map(|p| p.data().to_vec()).unwrap_or_default()),
    ];
    let mut names: Vec<(&str, u8, Vec<u8>)> = Vec::new();
    for (where_, data) in &places {
        for (t, v) in parse_ad(data).0 {
            if t == 0x08 || t == 0x09 {
                names.push((where_, t, v));
            }
        }
    }
    link(s, LinkOpts::new())?;
    let g = table(s)?;
    let Some(c) = g.char_uuid(&NAME).cloned() else {
        return Err(Stop::Inconclusive("no Device Name characteristic".into()));
    };
    let (v, complete) = full_value(s, c.value, MTU)?;
    let v = match v {
        Some(v) if complete => v,
        _ => {
            let r = s.att(&read_req(c.value))?;
            return Err(Stop::Inconclusive(format!("the Device Name could not be read whole: {}", describe(r.as_deref()))));
        }
    };
    if names.is_empty() {
        return Err(Stop::NotApplicable(format!(
            "no Local Name in the advertising data{}",
            if rsp.is_empty() { " (no scan response heard)" } else { " or the scan response data" }
        )));
    }
    let mut bad = Vec::new();
    for (where_, t, n) in &names {
        if *t == 0x09 && *n != v {
            bad.push(format!(
                "Complete Local Name {} in the {where_}; the Device Name characteristic holds {}",
                py::bytes_repr(n),
                py::bytes_repr(&v)
            ));
        } else if *t == 0x08 && (!v.starts_with(n) || n.is_empty()) {
            bad.push(format!(
                "Shortened Local Name {} in the {where_} is not the beginning of the Device Name {}",
                py::bytes_repr(n),
                py::bytes_repr(&v)
            ));
        }
    }
    expect(bad.is_empty(), some(&bad, 5))?;
    let parts: Vec<String> = names
        .iter()
        .map(|(w, t, n)| {
            format!("{} Local Name {} in the {w}", if *t == 0x09 { "Complete" } else { "Shortened" }, py::bytes_repr(n))
        })
        .collect();
    Ok(format!("{}; Device Name {}", parts.join("; "), py::bytes_repr(&v)))
}
