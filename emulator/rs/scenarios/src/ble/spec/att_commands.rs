use crate::ble::harness::{hx, BleSession, L2Frame, LinkOpts, CID_ATT};
use crate::ble::util::{cccds, describe, link, read, table, value};
use crate::ble::BleTest;
use crate::session::{expect, Stop, R};

const DOC: &str = "Core 5.4 Vol 3 Part F";

pub fn tests() -> Vec<BleTest> {
    vec![
        BleTest {
            id: "att_write_cmd_not_answered",
            doc: DOC,
            clause: "3.4.5.3, 3.4.1.1",
            req: "ATT_WRITE_CMD is never answered, whether the handle is writable, read-only or no attribute at all, \
                  and a read-only attribute keeps its value.",
            run: write_cmd_not_answered,
            ..BleTest::D
        },
        BleTest {
            id: "att_unknown_command_ignored",
            doc: DOC,
            clause: "3.3, 3.3.1",
            req: "A PDU with the Command Flag set and a method that is no command of the server is ignored: no \
                  answer, also when the method is that of a request the server supports.",
            run: unknown_command_ignored,
            ..BleTest::D
        },
        BleTest {
            id: "att_signed_write_ignored",
            doc: DOC,
            clause: "3.4.5.4",
            req: "ATT_SIGNED_WRITE_CMD with a signature that cannot be verified (no bond, so no key) is ignored: no \
                  answer, value unchanged.",
            run: signed_write_ignored,
            ..BleTest::D
        },
    ]
}

fn answers(s: &mut BleSession, n0: usize) -> R<Vec<L2Frame>> {
    s.poll()?;
    Ok(s.lk().frames[n0..].iter().filter(|f| f.cid == CID_ATT && BleSession::is_rsp(&f.data)).cloned().collect())
}

fn flip(v: &Option<Vec<u8>>) -> Vec<u8> {
    match v {
        Some(v) if !v.is_empty() => {
            let mut o = v.clone();
            o[0] ^= 0x01;
            o
        }
        _ => vec![0x01],
    }
}

fn hxo(v: &Option<Vec<u8>>) -> String {
    v.as_deref().map(hx).unwrap_or_else(|| "None".into())
}

fn joined(fr: &[L2Frame]) -> String {
    fr.iter().map(|f| hx(&f.data)).collect::<Vec<_>>().join(", ")
}

fn pdu(op: u8, h: u16, rest: &[u8]) -> Vec<u8> {
    let mut v = vec![op];
    v.extend_from_slice(&h.to_le_bytes());
    v.extend_from_slice(rest);
    v
}

fn write_cmd_not_answered(s: &mut BleSession) -> R<String> {
    link(s, LinkOpts::new())?;
    let g = table(s)?;
    let sv = g.services[0].start;
    let mut ro = None;
    for c in &g.chars {
        if c.props & 0x4C == 0 {
            let r = read(s, c.value)?;
            if value(r.as_deref()).is_some() {
                ro = Some(c.value);
                break;
            }
        }
    }
    let mut before: Vec<(u16, Option<Vec<u8>>)> = Vec::new();
    for h in [Some(sv), ro].into_iter().flatten() {
        let r = read(s, h)?;
        before.push((h, value(r.as_deref())));
    }
    let bv = |h: u16, b: &Vec<(u16, Option<Vec<u8>>)>| b.iter().find(|x| x.0 == h).unwrap().1.clone();
    let mut targets: Vec<(String, u16, Vec<u8>)> = vec![
        ("handle 0x0000".into(), 0x0000, vec![0]),
        (format!("handle {:#06x}, above the table", g.last_handle() as u32 + 1), g.last_handle().wrapping_add(1), vec![0]),
        ("handle 0xFFFF".into(), 0xFFFF, vec![0]),
        (format!("service declaration {sv:#06x}"), sv, flip(&bv(sv, &before))),
    ];
    if let Some(ro) = ro {
        targets.push((format!("read-only characteristic value {ro:#06x}"), ro, flip(&bv(ro, &before))));
    }
    let cc = cccds(&g);
    let wwr = g.chars.iter().find(|c| c.props & 0x04 != 0);
    if let Some((_, h)) = cc.first() {
        targets.push((format!("CCCD {h:#06x}"), *h, vec![0, 0]));
    } else if let Some(w) = wwr {
        targets.push((format!("characteristic value {:#06x} (Write Without Response)", w.value), w.value, vec![0]));
    }
    let n0 = s.lk().frames.len();
    let mut got = Vec::new();
    for (what, h, v) in &targets {
        let fr = s.att_quiet(&pdu(0x52, *h, v), 600.0)?;
        if !fr.is_empty() {
            got.push(format!("{what}: answered with {}", joined(&fr)));
        }
    }
    s.link_run(1000.0)?;
    let total = answers(s, n0)?;
    let mut after: Vec<(u16, Option<Vec<u8>>)> = Vec::new();
    for (h, _) in &before {
        let r = read(s, *h)?;
        after.push((*h, value(r.as_deref())));
    }
    let mut changed = Vec::new();
    for (h, b) in &before {
        let a = bv(*h, &after);
        if a.as_deref() == Some(&flip(b)[..]) {
            changed.push(format!("{h:#06x}: {} -> {}", hxo(b), hxo(&a)));
        }
    }
    let ctl = s.att(&pdu(0x12, sv, &flip(&bv(sv, &before))))?;
    let Some(ctl) = ctl else {
        return Err(Stop::Inconclusive("control: an ATT_WRITE_REQ after the commands is not answered".into()));
    };
    expect(
        got.is_empty() && total.is_empty(),
        format!("{} answers to {} Write Commands: {}", total.len(), targets.len(), got.iter().take(4).cloned().collect::<Vec<_>>().join("; ")),
    )?;
    expect(changed.is_empty(), format!("read-only attributes changed by a Write Command: {}", changed.join("; ")))?;
    Ok(format!(
        "{} Write Commands ({}): no answer, read-only values unchanged; the same write as a request is answered with {}",
        targets.len(),
        targets.iter().map(|t| t.0.clone()).collect::<Vec<_>>().join("; "),
        describe(Some(&ctl))
    ))
}

fn unknown_command_ignored(s: &mut BleSession) -> R<String> {
    link(s, LinkOpts::new())?;
    let g = table(s)?;
    let h = g.services[0].start;
    let all = [1u8, 0, 0xff, 0xff];
    let mut p50 = vec![0x50];
    p50.extend_from_slice(&all);
    p50.extend_from_slice(&0x2800u16.to_le_bytes());
    let mut p44 = vec![0x44];
    p44.extend_from_slice(&all);
    let pdus = vec![pdu(0x54, h, &[]), pdu(0x7E, h, &[]), pdu(0x4a, h, &[]), p44, p50];
    let n0 = s.lk().frames.len();
    let mut got = Vec::new();
    for p in &pdus {
        let fr = s.att_quiet(p, 600.0)?;
        if !fr.is_empty() {
            got.push(format!("{}: answered with {}", hx(p), joined(&fr)));
        }
    }
    s.link_run(1000.0)?;
    let total = answers(s, n0)?;
    let mut ctl = Vec::new();
    for p in &pdus[2..] {
        let mut q = vec![p[0] & 0x3F];
        q.extend_from_slice(&p[1..]);
        ctl.push(s.att(&q)?);
    }
    if ctl.iter().any(|c| c.as_ref().map_or(true, |c| c[0] == 0x01)) {
        return Err(Stop::Inconclusive(format!(
            "control: the requests without the Command Flag are not answered with responses: {}",
            ctl.iter().map(|c| describe(c.as_deref())).collect::<Vec<_>>().join(", ")
        )));
    }
    expect(
        got.is_empty() && total.is_empty(),
        format!(
            "{} answers to {} unsupported commands: {}",
            total.len(),
            pdus.len(),
            got.iter().take(4).cloned().collect::<Vec<_>>().join("; ")
        ),
    )?;
    Ok("opcodes 0x54, 0x7e, 0x4a, 0x44, 0x50 ignored; 0x0a, 0x04, 0x10 (the same without the Command Flag) answered".into())
}

fn signed_write_ignored(s: &mut BleSession) -> R<String> {
    link(s, LinkOpts::new())?;
    let g = table(s)?;
    let cc = cccds(&g);
    let (h, new, what) = if let Some((c, h)) = cc.first() {
        (*h, if c.props & 0x10 != 0 { vec![1, 0] } else { vec![2, 0] }, format!("CCCD {h:#06x}"))
    } else {
        let mut found = None;
        for c in &g.chars {
            if c.props & 0x4C != 0 && c.props & 0x02 != 0 {
                let r = read(s, c.value)?;
                if value(r.as_deref()).is_some() {
                    found = Some(c.clone());
                    break;
                }
            }
        }
        let Some(c) = found else { return Err(Stop::Inconclusive("no attribute that is both writable and readable".into())) };
        let r = read(s, c.value)?;
        let mut n = flip(&value(r.as_deref()));
        n.truncate(8);
        (c.value, n, format!("characteristic value {:#06x}", c.value))
    };
    let r = read(s, h)?;
    let v0 = value(r.as_deref());
    let n0 = s.lk().frames.len();
    let sig = [0x01, 0x00, 0x00, 0x00, 0xf1, 0x87, 0x1e, 0x93, 0x3c, 0x90, 0x0f, 0xf2];
    let mut body = new.clone();
    body.extend_from_slice(&sig);
    let mut fr = s.att_quiet(&pdu(0xd2, h, &body), 800.0)?;
    let mut body2 = vec![0x00];
    body2.extend_from_slice(&sig);
    fr.extend(s.att_quiet(&pdu(0xd2, g.last_handle().wrapping_add(1), &body2), 800.0)?);
    s.link_run(600.0)?;
    let total = answers(s, n0)?;
    let v1 = read(s, h)?;
    let Some(v1) = v1 else {
        return Err(Stop::Inconclusive("control: a Read Request after the commands is not answered".into()));
    };
    expect(fr.is_empty() && total.is_empty(), format!("ATT_SIGNED_WRITE_CMD answered with {}", joined(&total)))?;
    expect(
        value(Some(&v1)) == v0,
        format!(
            "{what}: value {} before the signed write of {}, {} after; required unchanged",
            hxo(&v0),
            hx(&new),
            describe(Some(&v1))
        ),
    )?;
    Ok(format!("signed writes to the {what} and to a handle above the table: no answer, value unchanged ({})", hxo(&v0)))
}
