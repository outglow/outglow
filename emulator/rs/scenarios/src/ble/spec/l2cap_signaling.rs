//! L2CAP: the LE signalling channel (Core 5.4 Vol 3 Part A 4, 4.1, 4.20);
//! emulator/spec_ble/l2cap_signaling.py.

use crate::ble::harness::{hx, BleSession, L2Frame, LinkOpts, ParamReq, CID_SIG};
use crate::ble::{BleTest, Level};
use crate::py;
use crate::session::{expect, Stop, R};

const DOC: &str = "Core 5.4 Vol 3 Part A";

pub fn tests() -> Vec<BleTest> {
    vec![
        BleTest {
            id: "l2cap_sig_reject_unknown_code",
            doc: DOC,
            clause: "4, 4.1",
            req: "A signalling command with a reserved code is answered with a Command Reject (reason 0x0000, same \
                  identifier, no reason data).",
            run: reject_unknown,
            ..BleTest::D
        },
        BleTest {
            id: "l2cap_sig_reject_disallowed_code",
            doc: DOC,
            clause: "4 (Table 4.2), 4.1",
            req: "A command that is only allowed on the BR/EDR signalling channel is answered with a Command Reject \
                  on the LE one.",
            run: reject_disallowed,
            ..BleTest::D
        },
        BleTest {
            id: "l2cap_sig_credit_based_requests",
            doc: DOC,
            clause: "4.1, 4.22, 4.25, 4.27",
            req: "The connection-oriented channel requests (LE credit based 0x14, enhanced credit based 0x17, \
                  reconfigure 0x19) are answered: with their response refusing the channel, or with a Command Reject.",
            run: credit_based,
            ..BleTest::D
        },
        BleTest {
            id: "l2cap_sig_identifier",
            doc: DOC,
            clause: "4",
            req: "The response carries the identifier of the request, for any identifier from 0x01 to 0xFF.",
            run: identifier,
            ..BleTest::D
        },
        BleTest {
            id: "l2cap_sig_param_update_req_to_peripheral",
            doc: DOC,
            clause: "4.20",
            req: "A peripheral that receives L2CAP_CONNECTION_PARAMETER_UPDATE_REQ answers with a Command Reject, \
                  reason 0x0000.",
            run: param_req_to_peripheral,
            ..BleTest::D
        },
        BleTest {
            id: "l2cap_sig_unsolicited_response",
            doc: DOC,
            clause: "4, 4.1",
            req: "A response nobody asked for is discarded silently; no Command Reject for it.",
            run: unsolicited_response,
            level: Level::Should,
            ..BleTest::D
        },
        BleTest {
            id: "l2cap_sig_mtu_exceeded",
            doc: DOC,
            clause: "4, 4.1",
            req: "A signalling packet larger than the device's MTUsig is answered with a Command Reject: 'Signaling \
                  MTU exceeded' with the MTUsig (at least 23), or it is processed if it fits.",
            run: mtu_exceeded,
            ..BleTest::D
        },
        BleTest {
            id: "l2cap_param_update_request",
            doc: DOC,
            clause: "4.20; Vol 6 Part B 2.4.2.16, 4.5.1, 4.5.2",
            req: "A Connection Parameter Update Request of the peripheral has a non-zero identifier and legal values: \
                  6 <= Interval_Min <= Interval_Max <= 3200, latency <= 499, timeout 10..3200 and longer than \
                  (1 + latency) x Interval_Max x 2.",
            run: param_update_request,
            mandatory: false,
            ..BleTest::D
        },
    ]
}

/// Python's `format(x, 'g')` for the moderate values these tests print.
fn g(x: f64) -> String {
    if x == 0.0 {
        return "0".into();
    }
    let e = x.abs().log10().floor() as i32;
    let d = (5 - e).max(0) as usize;
    let s = format!("{x:.d$}");
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

fn cmd(code: u8, ident: u8, data: &[u8]) -> Vec<u8> {
    let mut v = vec![code, ident];
    v.extend_from_slice(&(data.len() as u16).to_le_bytes());
    v.extend_from_slice(data);
    v
}

/// (identifier, reason, reason data) of a Command Reject, else None.
fn reject_of(fr: Option<&L2Frame>) -> Option<(u8, u16, Vec<u8>)> {
    let d: &[u8] = fr.map(|f| &f.data[..]).unwrap_or(&[]);
    if d.len() >= 6 && d[0] == 0x01 {
        return Some((d[1], u16::from_le_bytes([d[4], d[5]]), d[6..].to_vec()));
    }
    None
}

fn expect_reject(s: &mut BleSession, code: u8, ident: u8, data: &[u8], what: &str) -> R<L2Frame> {
    let fr = s.sig(&cmd(code, ident, data), 3000.0)?;
    expect(
        fr.is_some(),
        format!("{what} (code {code:#04x}, identifier {ident:#04x}): no answer on the signalling channel"),
    )?;
    let fr = fr.unwrap();
    let rj = reject_of(Some(&fr));
    expect(rj.is_some(), format!("{what}: answered with {}, not a Command Reject", hx(&fr.data)))?;
    let rj = rj.unwrap();
    expect(rj.0 == ident, format!("{what}: Command Reject with identifier {:#04x}, the command had {ident:#04x}", rj.0))?;
    let ln = u16::from_le_bytes([fr.data[2], fr.data[3]]) as usize;
    expect(
        ln == fr.data.len() - 4,
        format!("{what}: Data Length {ln} in a packet with {} data octets", fr.data.len() - 4),
    )?;
    expect(
        rj.1 == 0x0000 && rj.2.is_empty(),
        format!(
            "{what}: reason {:#06x} with data {}; required 0x0000 (Command not understood) without data",
            rj.1,
            hx(&rj.2)
        ),
    )?;
    Ok(fr)
}

fn link12(s: &mut BleSession) -> R<()> {
    s.connect(LinkOpts::new().set("interval", 12))?;
    s.link_run(100.0)?;
    Ok(())
}

fn reject_unknown(s: &mut BleSession) -> R<String> {
    // 4 / 4.1: an unknown code gets an L2CAP_COMMAND_REJECT_RSP with the same identifier; Table 4.2 codes
    // above 0x1A are reserved, 0x0C-0x11 are now none.
    link12(s)?;
    let cases: [(u8, &[u8]); 6] =
        [(0x1C, &[]), (0x1B, &[0x01, 0x02]), (0x0D, &[]), (0x30, &[0, 0, 0, 0]), (0xFF, &[]), (0x7E, &[0xaa])];
    for (i, (code, data)) in cases.iter().enumerate() {
        expect_reject(s, *code, 0x20 + i as u8, data, "reserved code")?;
    }
    Ok("codes 0x1c, 0x1b, 0x0d, 0x30, 0xff, 0x7e rejected".into())
}

fn reject_disallowed(s: &mut BleSession) -> R<String> {
    // Table 4.2: 0x02, 0x04, 0x08 and 0x0A are allowed on CID 0x0001 only; 4: a disallowed code gets a reject.
    link12(s)?;
    let cases: [(u8, Vec<u8>, &str); 4] = [
        (0x02, vec![1, 0, 0x40, 0], "L2CAP_CONNECTION_REQ"),
        (0x08, vec![], "L2CAP_ECHO_REQ"),
        (0x0A, vec![2, 0], "L2CAP_INFORMATION_REQ"),
        (0x04, vec![0x40, 0, 0, 0], "L2CAP_CONFIGURATION_REQ"),
    ];
    for (i, (code, data, name)) in cases.iter().enumerate() {
        expect_reject(s, *code, 0x31 + i as u8, data, name)?;
    }
    Ok("connection, echo, information and configuration requests rejected".into())
}

fn le16s(v: &[u16]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn credit_based(s: &mut BleSession) -> R<String> {
    // 4.1: "Implementations shall always send these packets in response to unidentified signaling packets."
    link12(s)?;
    let reqs: [(u8, Vec<u8>, u8, &str); 3] = [
        (0x14, le16s(&[0x0080, 0x0040, 64, 64, 1]), 0x15, "L2CAP_LE_CREDIT_BASED_CONNECTION_REQ"),
        (0x17, le16s(&[0x0027, 64, 64, 1, 0x0040]), 0x18, "L2CAP_CREDIT_BASED_CONNECTION_REQ"),
        (0x19, le16s(&[64, 64, 0x0040]), 0x1A, "L2CAP_CREDIT_BASED_RECONFIGURE_REQ"),
    ];
    let (mut notes, mut bad) = (Vec::new(), Vec::new());
    for (i, (code, data, rsp, name)) in reqs.iter().enumerate() {
        let ident = 0x41 + i as u8;
        let Some(fr) = s.sig(&cmd(*code, ident, data), 3000.0)? else {
            bad.push(format!("{name} (code {code:#04x}): no answer"));
            continue;
        };
        if fr.data[0] == 0x01 {
            if fr.data[1] != ident {
                bad.push(format!("{name}: Command Reject with identifier {:#04x}, the request had {ident:#04x}", fr.data[1]));
            }
            notes.push(format!("{code:#04x}: Command Reject"));
        } else if fr.data[0] == *rsp {
            if fr.data[1] != ident {
                bad.push(format!("{name}: response with identifier {:#04x}", fr.data[1]));
            }
            notes.push(format!("{code:#04x}: response {}", hx(&fr.data[4..])));
        } else {
            bad.push(format!("{name}: answered with {}", hx(&fr.data)));
        }
    }
    expect(bad.is_empty(), bad.join("; "))?;
    Ok(notes.join(", "))
}

fn identifier(s: &mut BleSession) -> R<String> {
    // 4: "The requesting device sets this field and the responding device uses the same value in its response."
    link12(s)?;
    for ident in [0x01u8, 0x02, 0x7F, 0x80, 0xFE, 0xFF] {
        expect_reject(s, 0x1C, ident, &[], "reserved code")?;
    }
    Ok("identifiers 0x01, 0x02, 0x7f, 0x80, 0xfe, 0xff echoed".into())
}

fn param_req_to_peripheral(s: &mut BleSession) -> R<String> {
    // 4.20: a Peripheral's Host that receives the request "shall respond with an L2CAP_COMMAND_REJECT_RSP
    // packet with reason 0x0000 (Command not understood)."
    link12(s)?;
    expect_reject(s, 0x12, 0x55, &le16s(&[24, 40, 0, 400]), "L2CAP_CONNECTION_PARAMETER_UPDATE_REQ")?;
    expect(s.alive(3000.0)?, "the connection ended")?;
    Ok("Command Reject, reason 0x0000".into())
}

fn unsolicited_response(s: &mut BleSession) -> R<String> {
    // 4: "A command response with an invalid identifier is silently discarded." 4.1: no reject for an
    // identified Response packet.
    s.connect(LinkOpts::new().set("interval", 12).param_req(ParamReq::Ignore))?;
    let n0 = s.lk().frames.len();
    s.l2cap(CID_SIG, &cmd(0x13, 0x77, &[0, 0]))?;
    s.l2cap(CID_SIG, &cmd(0x01, 0x78, &[0, 0]))?;
    s.link_run(600.0)?;
    let got: Vec<String> = s.lk().frames[n0..]
        .iter()
        .filter(|f| f.cid == CID_SIG && f.data.first() != Some(&0x12))
        .map(|f| format!("'{}'", hx(&f.data)))
        .collect();
    expect(got.is_empty(), format!("answered an unsolicited response with {}", py::list(got)))?;
    let fr = s.sig(&cmd(0x1C, 0x79, &[]), 3000.0)?;
    if fr.is_none() {
        return Ok("discarded (a following command with a reserved code is not answered either, see \
                   l2cap_sig_reject_unknown_code)"
            .into());
    }
    Ok("discarded; a following request is still answered".into())
}

fn mtu_exceeded(s: &mut BleSession) -> R<String> {
    // 4: a C-frame over MTUsig gets a Command Reject with the supported MTUsig (Table 4.1: at least 23 on
    // LE-U; 4.1 Table 4.4: reason 0x0001 carries 2 octets).
    link12(s)?;
    let data: Vec<u8> = (0..120).collect();
    let fr = s.sig(&cmd(0x1C, 0x61, &data), 4000.0)?;
    expect(fr.is_some(), "a 124-octet signalling packet got no answer")?;
    let fr = fr.unwrap();
    let rj = reject_of(Some(&fr));
    expect(rj.is_some(), format!("answered with {}", hx(&fr.data)))?;
    let rj = rj.unwrap();
    let note = if rj.1 == 0x0001 {
        expect(rj.2.len() == 2, format!("'Signaling MTU exceeded' with {} octets of reason data", rj.2.len()))?;
        let mtu = u16::from_le_bytes([rj.2[0], rj.2[1]]);
        expect((23..124).contains(&mtu), format!("'Signaling MTU exceeded' reports MTUsig {mtu}"))?;
        format!("Signaling MTU exceeded, MTUsig {mtu}")
    } else {
        expect(rj.1 == 0x0000 && rj.0 == 0x61, format!("Command Reject {}", hx(&fr.data)))?;
        "accepted as a whole and rejected as not understood".to_string()
    };
    expect_reject(s, 0x1C, 0x62, &[], "a short command afterwards")?;
    Ok(note)
}

fn param_update_request(s: &mut BleSession) -> R<String> {
    // 4.20 with Vol 6 Part B 2.4.2.16, 4.5.1, 4.5.2; Part A 4: identifier 0x00 is invalid.
    s.connect(LinkOpts::new().set("interval", 24).set("timeout", 400).param_req(ParamReq::Reject))?;
    s.link_until(12000.0, Some(20.0), |s| Ok(if s.lk().param_reqs.is_empty() { None } else { Some(()) }))?;
    if s.lk().param_reqs.is_empty() {
        return Err(Stop::NotApplicable("the bulb sent no Connection Parameter Update Request in 12 s".into()));
    }
    let fr = s.lk().param_reqs[0].clone();
    let d = &fr.data;
    expect(d.len() == 12 && u16::from_le_bytes([d[2], d[3]]) == 8, format!("request {}: wrong length", hx(d)))?;
    expect(d[1] != 0, "identifier 0x00")?;
    let u = |i: usize| u16::from_le_bytes([d[i], d[i + 1]]) as i64;
    let (mn, mx, lat, to) = (u(4), u(6), u(8), u(10));
    let mut bad = Vec::new();
    if !(6 <= mn && mn <= mx && mx <= 3200) {
        bad.push(format!("interval {mn}..{mx}"));
    }
    if lat > 499 {
        bad.push(format!("latency {lat}"));
    }
    if !(10..=3200).contains(&to) {
        bad.push(format!("timeout {to}"));
    }
    if (to * 10) as f64 <= (1 + lat) as f64 * mx as f64 * 1.25 * 2.0 {
        bad.push(format!(
            "timeout {} ms not longer than (1 + {lat}) x {} ms x 2 = {} ms",
            to * 10,
            g(mx as f64 * 1.25),
            g((1 + lat) as f64 * mx as f64 * 2.5)
        ));
    }
    expect(bad.is_empty(), format!("request {}: {}", hx(&d[4..]), bad.join(", ")))?;
    let tconnend = s.lk().st.f("tconnend").unwrap_or(0.0);
    Ok(format!(
        "{:.2} s after the CONNECT_IND: interval {}..{} ms, latency {lat}, timeout {} ms",
        (fr.t - tconnend) / 1000.0,
        g(mn as f64 * 1.25),
        g(mx as f64 * 1.25),
        to * 10
    ))
}
