//! Security Manager: a device without pairing (Core 5.4 Vol 3 Part H 2.3, 3.3,
//! 3.5.5); emulator/spec_ble/smp_pairing.py.

use crate::ble::harness::{hx, BleSession, LinkOpts, CID_SMP};
use crate::ble::BleTest;
use crate::session::{expect, Stop, R};

const DOC: &str = "Core 5.4 Vol 3 Part H";
/// NoInputNoOutput, no OOB, bonding, 16-octet key.
const PAIRING_REQ: [u8; 7] = [0x01, 0x03, 0x00, 0x01, 0x10, 0x00, 0x01];

pub fn tests() -> Vec<BleTest> {
    vec![
        BleTest {
            id: "smp_pairing_request",
            doc: DOC,
            clause: "2.3, 3.5.5",
            req: "A Pairing Request is answered: with a Pairing Response, or by a device without pairing with Pairing \
                  Failed, reason Pairing Not Supported (0x05).",
            run: pairing_request,
            ..BleTest::D
        },
        BleTest {
            id: "smp_any_command",
            doc: DOC,
            clause: "3.3",
            req: "A device without pairing answers every Security Manager command with Pairing Failed, reason Pairing \
                  Not Supported.",
            run: any_command,
            ..BleTest::D
        },
        BleTest {
            id: "smp_reserved_code",
            doc: DOC,
            clause: "3.3",
            req: "A Security Manager packet with a reserved code is ignored.",
            run: reserved_code,
            ambiguous: Some(
                "3.3 says a reserved code 'shall be ignored' and, three paragraphs on, that a device without pairing \
                 answers Pairing Failed 'when any command is received'; read here as: a reserved code is not a \
                 command, so it gets no answer",
            ),
            ..BleTest::D
        },
    ]
}

fn smp(s: &mut BleSession, pdu: &[u8], wait_ms: f64) -> R<Option<Vec<u8>>> {
    s.poll()?;
    let n0 = s.lk().frames.len();
    s.l2cap(CID_SMP, pdu)?;
    let fr = s.frame_wait(CID_SMP, |_| true, wait_ms, Some(n0))?;
    Ok(fr.map(|f| f.data))
}

fn open(s: &mut BleSession) -> R<()> {
    s.connect(LinkOpts::new().set("interval", 12))?;
    s.link_run(100.0)?;
    Ok(())
}

fn pairing_request(s: &mut BleSession) -> R<String> {
    // 2.3: "If the responding device does not support pairing or pairing cannot be performed then the
    // responding device shall respond using the Pairing Failed message with the error code 'Pairing Not
    // Supported' otherwise it responds with a Pairing Response message."
    open(s)?;
    let rsp = smp(s, &PAIRING_REQ, 4000.0)?;
    expect(rsp.is_some(), "Pairing Request: no answer on the Security Manager channel")?;
    let rsp = rsp.unwrap();
    let Some(&c0) = rsp.first() else { return Err(Stop::Error("IndexError: index out of range".into())) };
    if c0 == 0x02 {
        expect(rsp.len() == 7, format!("Pairing Response {}", hx(&rsp)))?;
        return Ok(format!("pairing supported: Pairing Response {}", hx(&rsp)));
    }
    expect(rsp == [0x05, 0x05], format!("answered with {}; required Pairing Failed 0505", hx(&rsp)))?;
    let alive = s.alive(3000.0)?;
    expect(alive, "the connection ended")?;
    Ok("Pairing Failed, Pairing Not Supported".into())
}

fn any_command(s: &mut BleSession) -> R<String> {
    // 3.3: "If a device does not support pairing then it shall respond with a Pairing Failed command with
    // the reason set to 'Pairing Not Supported' ... when any command is received." Commands of Table 3.3
    // other than the Pairing Request, each with its defined length (Pairing Failed itself is left out).
    open(s)?;
    let first = smp(s, &PAIRING_REQ, 4000.0)?;
    if first.as_ref().is_some_and(|f| f.first() == Some(&0x02)) {
        return Err(Stop::NotApplicable("the device supports pairing".into()));
    }
    let with = |c: u8, n: usize| -> Vec<u8> {
        let mut v = vec![c];
        v.extend(std::iter::repeat(0).take(n));
        v
    };
    let cmds: Vec<(&str, Vec<u8>)> = vec![
        ("Pairing Confirm", with(0x03, 16)),
        ("Pairing Random", with(0x04, 16)),
        ("Encryption Information", with(0x06, 16)),
        ("Central Identification", with(0x07, 10)),
        ("Identity Information", with(0x08, 16)),
        ("Signing Information", with(0x0A, 16)),
        ("Pairing Keypress Notification", vec![0x0e, 0x00]),
    ];
    let mut bad = Vec::new();
    for (name, pdu) in &cmds {
        let rsp = smp(s, pdu, 2000.0)?;
        if rsp.as_deref() != Some(&[0x05, 0x05][..]) {
            let t = match &rsp {
                Some(r) if !r.is_empty() => hx(r),
                _ => "no answer".into(),
            };
            bad.push(format!("{name} ({:#04x}): {t}", pdu[0]));
        }
    }
    expect(
        bad.is_empty(),
        format!(
            "{} of {} commands not answered with Pairing Failed 0505: {}",
            bad.len(),
            cmds.len(),
            bad.iter().take(4).cloned().collect::<Vec<_>>().join("; ")
        ),
    )?;
    Ok(format!("{} commands answered with Pairing Failed, Pairing Not Supported", cmds.len()))
}

fn reserved_code(s: &mut BleSession) -> R<String> {
    // 3.3: "If a packet is received with a Code that is reserved for future use it shall be ignored."
    // Table 3.3 ends at 0x0E.
    open(s)?;
    let mut got = Vec::new();
    for code in [0x0Fu8, 0x00, 0x40, 0xFF] {
        if let Some(rsp) = smp(s, &[code, 0x01, 0x02], 800.0)? {
            got.push(format!("{code:#04x}: {}", hx(&rsp)));
        }
    }
    let alive = s.alive(3000.0)?;
    expect(alive, "the connection ended")?;
    expect(got.is_empty(), format!("answers to reserved codes: {}", got.join("; ")))?;
    Ok("codes 0x0f, 0x00, 0x40, 0xff ignored".into())
}
