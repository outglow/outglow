//! ATT: notifications, and a confirmation nobody asked for (Core 5.4 Vol 3 Part F
//! 3.4.7; Part G 3.3.1.1, 4.10); emulator/spec_ble/att_server_initiated.py.

use crate::ble::harness::{hx, BleSession, LinkOpts, CID_ATT};
use crate::ble::util::{cccds, describe, link, mesh_app, read, stimulate, table, MTU};
use crate::ble::BleTest;
use crate::py;
use crate::session::{expect, Stop, R};

const DOC: &str = "Core 5.4 Vol 3 Part F";

pub fn tests() -> Vec<BleTest> {
    vec![
        BleTest {
            id: "att_notification_format",
            doc: "Core 5.4 Vol 3 Part F; Part G",
            clause: "F 3.4.7.1; G 4.10.1, 3.3.1.1",
            req: "An ATT_HANDLE_VALUE_NTF names the value handle of a characteristic that has the Notify property \
                  and carries at most ATT_MTU - 3 octets of value.",
            run: notification_format,
            mandatory: false,
            ..BleTest::D
        },
        BleTest {
            id: "att_unsolicited_confirmation",
            doc: DOC,
            clause: "3.3 (Table 3.1), 3.4.1.1, 3.4.7.3",
            req: "An ATT_HANDLE_VALUE_CFM that confirms no indication is not answered.",
            run: unsolicited_confirmation,
            ambiguous: Some(
                "no clause says what a server does with a confirmation it is not waiting for; the test reads Table \
                 3.1 (only requests invoke a response) and 3.4.1.1 (the error response states that a request cannot \
                 be performed) as: nothing is sent back",
            ),
            ..BleTest::D
        },
    ]
}

fn notification_format(s: &mut BleSession) -> R<String> {
    // Part F 3.4.7.1, Table 3.38: opcode 0x1B, Attribute Handle (2), Attribute Value 0 to (ATT_MTU-3).
    // Part G 4.10.1: the Attribute Handle is the Characteristic Value Handle being notified. Part G
    // 3.3.1.1: Notify (0x10) permits notifications. Notifications are provoked the way the vendor
    // application does it (every CCCD enabled first); ATT_MTU is 23.
    link(s, LinkOpts::new())?;
    let g = table(s)?;
    for (c, h) in cccds(&g) {
        let mut q = vec![0x12];
        q.extend_from_slice(&h.to_le_bytes());
        q.extend_from_slice(if c.props & 0x10 != 0 { &[1, 0] } else { &[2, 0] });
        s.att(&q)?;
    }
    let n0 = s.lk().frames.len();
    let mut app = mesh_app(s)?;
    stimulate(s, &mut app, 500.0)?;
    let ntf = s.notifications(n0);
    if ntf.is_empty() {
        return Err(Stop::Inconclusive(
            "the bulb sent no notification after login, the application's notification switch and two commands".into(),
        ));
    }
    let mut bad = Vec::new();
    let mut handles: Vec<u16> = Vec::new();
    for f in &ntf {
        let d = &f.data;
        if d.len() < 3 {
            bad.push(format!("{}: shorter than opcode and handle", hx(d)));
            continue;
        }
        let h = u16::from_le_bytes([d[1], d[2]]);
        if !handles.contains(&h) {
            handles.push(h);
        }
        let c = g.chars.iter().find(|c| c.value == h);
        let need = if d[0] == 0x1B { 0x10 } else { 0x20 };
        match c {
            None => bad.push(format!("{}...: handle {h:#06x} is not the value handle of a characteristic", hx(&d[..3]))),
            Some(c) if c.props & need == 0 => {
                let mut u = c.uuid.clone();
                u.reverse();
                bad.push(format!(
                    "{}...: characteristic {} has properties {:#04x}, without {}",
                    hx(&d[..3]),
                    hx(&u),
                    c.props,
                    if need == 0x10 { "Notify" } else { "Indicate" }
                ));
            }
            _ => {}
        }
        if d.len() > MTU {
            bad.push(format!("{} octets of value in a PDU of {}; ATT_MTU is 23", d.len() - 3, d.len()));
        }
    }
    expect(bad.is_empty(), bad.iter().take(4).cloned().collect::<Vec<_>>().join("; "))?;
    handles.sort();
    let mut sizes: Vec<i64> = ntf.iter().map(|f| f.data.len() as i64 - 3).collect();
    sizes.sort();
    sizes.dedup();
    Ok(format!(
        "{} notifications on value handle {} (Notify property set), value sizes {}",
        ntf.len(),
        handles.iter().map(|h| format!("{h:#06x}")).collect::<Vec<_>>().join(", "),
        py::ints(sizes)
    ))
}

fn unsolicited_confirmation(s: &mut BleSession) -> R<String> {
    // Table 3.1: requests are "PDUs sent to a server by a client that invoke a response"; a confirmation is
    // sent "to confirm receipt of an indication". 3.4.1.1: "The ATT_ERROR_RSP PDU is used to state that a
    // given request cannot be performed". Control: a Read Request after the confirmations is answered.
    link(s, LinkOpts::new())?;
    let g = table(s)?;
    let h = g.services[0].start;
    let rf = read(s, h)?;
    let n0 = s.lk().frames.len();
    let mut got = s.att_quiet(&[0x1e], 800.0)?;
    got.extend(s.att_quiet(&[0x1e], 800.0)?);
    s.poll()?;
    let extra: Vec<_> = s.lk().frames[n0..].iter().filter(|f| f.cid == CID_ATT).cloned().collect();
    let after = read(s, h)?;
    if after.is_none() {
        return Err(Stop::Inconclusive("control: a Read Request after the confirmations is not answered".into()));
    }
    expect(
        got.is_empty() && extra.is_empty(),
        format!("answered with {}", extra.iter().map(|f| hx(&f.data)).collect::<Vec<_>>().join(", ")),
    )?;
    let ok = if after == rf { s.alive(3000.0)? } else { false };
    expect(ok, format!("Read Request afterwards: {}, before {}", describe(after.as_deref()), describe(rf.as_deref())))?;
    Ok("two confirmations without an indication: nothing sent back; a Read Request afterwards is answered as before".into())
}
