use super::att_errors::cat;
use crate::ble::harness::{att_error, hx, BleSession, L2Frame, LinkOpts, CID_ATT};
use crate::ble::util::{
    blob, cccds, connect_rx_mtu, decl_pairs, describe, fits, leading_run, link, mesh_app, read, server_mtu_req,
    stimulate, table, u16b, value, MTU,
};
use crate::ble::{BleTest, Level};
use crate::py;
use crate::session::{expect, Stop, R};

const DOC: &str = "Core 5.4 Vol 3 Part F";
const CHAR: [u8; 2] = [0x03, 0x28];

pub fn tests() -> Vec<BleTest> {
    vec![
        BleTest {
            id: "att_mtu_exchange_rsp",
            doc: DOC,
            clause: "3.4.2.2",
            req: "An ATT_EXCHANGE_MTU_REQ is answered with an ATT_EXCHANGE_MTU_RSP of 3 octets whose Server Rx MTU is \
                  at least 23, and equal to the Client Rx MTU of the server's own request if it sent one.",
            run: exchange_rsp,
            mandatory: false,
            ..BleTest::D
        },
        BleTest {
            id: "att_mtu_applied",
            doc: DOC,
            clause: "3.4.2.2, 3.4.4.4",
            req: "After the exchange ATT_MTU is the minimum of Client Rx MTU and Server Rx MTU: a value longer than 22 \
                  octets is read up to ATT_MTU - 1 octets, and no PDU of the server exceeds ATT_MTU.",
            run: applied,
            mandatory: false,
            ..BleTest::D
        },
        BleTest {
            id: "att_mtu_read_by_type_fill",
            doc: DOC,
            clause: "3.4.2.2, 3.4.4.1, 3.4.4.2",
            req: "After the exchange a Read By Type response holds as many same-sized attributes as fit into the new \
                  ATT_MTU - 2 octets.",
            run: read_by_type_fill,
            mandatory: false,
            level: Level::Reading,
            ambiguous: Some(
                "'as many attributes as possible' is read as: as many as fit into the Attribute Data List of ATT_MTU \
                 - 2 octets; the text names no other limit (such as a response buffer of the implementation that is \
                 smaller than the ATT_MTU it agreed to)",
            ),
            ..BleTest::D
        },
        BleTest {
            id: "att_mtu_below_default",
            doc: DOC,
            clause: "3.4.2.2",
            req: "A Client Rx MTU below 23 does not change ATT_MTU: responses keep the size they have at the default \
                  ATT_MTU of 23.",
            run: below_default,
            mandatory: false,
            ..BleTest::D
        },
        BleTest {
            id: "att_mtu_never_exceeded",
            doc: DOC,
            clause: "3.2.8; Part G 5.2.1",
            req: "Without a larger ATT_MTU agreed, no attribute PDU of the server is longer than 23 octets.",
            session: "att",
            run: never_exceeded,
            ..BleTest::D
        },
        BleTest {
            id: "att_mtu_server_request",
            doc: DOC,
            clause: "3.4.2.1, 3.4.2.2 (rule 4)",
            req: "A server that sends ATT_EXCHANGE_MTU_REQ itself sends it once, with a Client Rx MTU of at least 23, \
                  and answers requests within the default ATT_MTU until the response arrives.",
            run: server_request,
            mandatory: false,
            ..BleTest::D
        },
        BleTest {
            id: "att_mtu_server_request_notifications",
            doc: DOC,
            clause: "3.4.2.2 (rule 5)",
            req: "A server that has sent ATT_EXCHANGE_MTU_REQ sends no notification or indication until the \
                  ATT_EXCHANGE_MTU_RSP has arrived.",
            run: server_request_notifications,
            mandatory: false,
            ..BleTest::D
        },
    ]
}

fn rbt_char() -> Vec<u8> {
    cat(&[&[0x08], &u16b(1), &u16b(0xFFFF), &CHAR])
}

fn exchange(s: &mut BleSession, client_rx: u16) -> R<u16> {
    let rsp = s.att(&cat(&[&[0x02], &u16b(client_rx)]))?;
    expect(rsp.is_some(), format!("ATT_EXCHANGE_MTU_REQ (Client Rx MTU {client_rx}): {}", describe(rsp.as_deref())))?;
    let rsp = rsp.unwrap();
    if let Some(e) = att_error(Some(&rsp)) {
        expect(
            e == (0x02, 0x0000, 0x06),
            format!(
                "ATT_EXCHANGE_MTU_REQ answered with {}; the only error allowed is Request Not Supported with handle \
                 0x0000",
                describe(Some(&rsp))
            ),
        )?;
        return Err(Stop::NotApplicable(
            "the server does not support the MTU exchange and says so (Request Not Supported, handle 0x0000): \
             ATT_MTU stays 23"
                .into(),
        ));
    }
    expect(
        rsp[0] == 0x03 && rsp.len() == 3,
        format!(
            "ATT_EXCHANGE_MTU_REQ answered with {}; required ATT_EXCHANGE_MTU_RSP: opcode 0x03 and a 2-octet Server \
             Rx MTU",
            hx(&rsp)
        ),
    )?;
    Ok(u16::from_le_bytes([rsp[1], rsp[2]]))
}

fn whole(s: &mut BleSession, h: u16, first: &[u8]) -> R<Option<Vec<u8>>> {
    let mut v = first.to_vec();
    for _ in 0..64 {
        let rsp = blob(s, h, v.len() as u16)?;
        let Some(rsp) = rsp.filter(|r| r.first() == Some(&0x0d)) else { return Ok(None) };
        if rsp.len() == 1 {
            return Ok(Some(v));
        }
        v.extend_from_slice(&rsp[1..]);
    }
    Ok(None)
}

fn exchange_rsp(s: &mut BleSession) -> R<String> {
    connect_rx_mtu(s, 100, false, LinkOpts::new())?;
    let own = server_mtu_req(s);
    let srv = exchange(s, 100)?;
    expect(srv as usize >= MTU, format!("Server Rx MTU {srv}; required at least 23"))?;
    if let Some(own) = own {
        expect(
            srv == own,
            format!(
                "the server's own ATT_EXCHANGE_MTU_REQ said {own}, its ATT_EXCHANGE_MTU_RSP says {srv}; required the \
                 same value (rule 1)"
            ),
        )?;
    }
    expect(s.alive(3000.0)?, "the connection ended")?;
    Ok(format!(
        "Server Rx MTU {srv}{}",
        own.map(|o| format!(", the same as in the server's own request ({o})")).unwrap_or_default()
    ))
}

fn applied(s: &mut BleSession) -> R<String> {
    connect_rx_mtu(s, 100, false, LinkOpts::new())?;
    let srv = exchange(s, 100)?;
    let a = 100.min(srv as usize);
    if a <= MTU {
        return Err(Stop::NotApplicable(format!("Server Rx MTU {srv}: the exchange leaves ATT_MTU at 23")));
    }
    let n0 = s.lk().frames.len();
    let g = table(s)?;
    let (mut seen, mut bad) = (Vec::new(), Vec::new());
    for h in g.attrs.keys().copied() {
        let r = read(s, h)?;
        let Some(v) = value(r.as_deref()) else { continue };
        if v.len() < MTU - 1 {
            continue;
        }
        let Some(w) = whole(s, h, &v)? else { continue };
        if w.len() <= MTU - 1 {
            continue;
        }
        seen.push(format!("handle {h:#06x} ({} octets) read as {}", w.len(), v.len()));
        let want = w.len().min(a - 1);
        if v.len() != want {
            bad.push(format!(
                "Read Request on handle {h:#06x}, a value of {} octets: {} octets returned; required the first {want} \
                 at ATT_MTU {a}",
                w.len(),
                v.len()
            ));
        }
        if seen.len() >= 3 {
            break;
        }
    }
    s.att(&rbt_char())?;
    s.poll()?;
    let longest =
        s.lk().frames.get(n0..).unwrap_or(&[]).iter().filter(|f| f.cid == CID_ATT).map(|f| f.data.len()).max().unwrap_or(0);
    let big = fits(s, n0, a)?;
    let mut sizes: Vec<i64> = big.iter().map(|f| f.data.len() as i64).collect();
    sizes.sort();
    sizes.dedup();
    expect(
        big.is_empty(),
        format!(
            "ATT_MTU is {a} (Client Rx MTU 100, Server Rx MTU {srv}) but the server sent PDUs of {} octets",
            py::ints(sizes)
        ),
    )?;
    expect(bad.is_empty(), format!("Server Rx MTU {srv}, ATT_MTU {a}: {}", bad.join("; ")))?;
    if seen.is_empty() && longest <= MTU {
        return Err(Stop::Inconclusive(format!(
            "Server Rx MTU {srv}, ATT_MTU {a}: the server has no value longer than 22 octets and none of its \
             responses grew beyond 23 octets, so nothing shows which ATT_MTU it uses"
        )));
    }
    Ok(format!(
        "Server Rx MTU {srv}, ATT_MTU {a}: {}; longest PDU {longest} octets",
        if seen.is_empty() { "no value longer than 22 octets".to_string() } else { seen.join("; ") }
    ))
}

fn read_by_type_fill(s: &mut BleSession) -> R<String> {
    connect_rx_mtu(s, 100, false, LinkOpts::new())?;
    let srv = exchange(s, 100)?;
    let a = 100.min(srv as usize);
    if a <= MTU {
        return Err(Stop::NotApplicable(format!("Server Rx MTU {srv}: the exchange leaves ATT_MTU at 23")));
    }
    let g = table(s)?;
    let want = leading_run(&decl_pairs(&g), 1, 0xFFFF, a);
    if want.is_empty() || 2 + want.len() * want[0].len() <= MTU {
        return Err(Stop::Inconclusive(
            "the first characteristic declarations of the same size fit into 23 octets: nothing to gain from a larger \
             ATT_MTU"
                .into(),
        ));
    }
    let rsp = s.att(&rbt_char())?;
    expect(
        rsp.as_ref().is_some_and(|r| r[0] == 0x09 && r.len() > 2 && r[1] != 0),
        format!("Read By Type «Characteristic»: {}", describe(rsp.as_deref())),
    )?;
    let rsp = rsp.unwrap();
    let got = (rsp.len() - 2) / rsp[1] as usize;
    let joined: Vec<u8> = want.concat();
    expect(
        got == want.len() && rsp[2..] == joined[..],
        format!(
            "Server Rx MTU {srv}, ATT_MTU {a}: the response holds {got} declarations in {} octets; {} of {} octets \
             each have the same size and fit",
            rsp.len(),
            want.len(),
            want[0].len()
        ),
    )?;
    Ok(format!("Server Rx MTU {srv}, ATT_MTU {a}: {got} declarations in one response of {} octets", rsp.len()))
}

fn below_default(s: &mut BleSession) -> R<String> {
    link(s, LinkOpts::new())?;
    let g = table(s)?;
    let mut reqs = vec![
        rbt_char(),
        cat(&[&[0x04], &u16b(1), &u16b(0xFFFF)]),
        cat(&[&[0x10], &u16b(1), &u16b(0xFFFF), &u16b(0x2800)]),
    ];
    let mut longest: Option<(usize, u16)> = None;
    for h in g.attrs.keys().copied() {
        let r = read(s, h)?;
        if let Some(v) = value(r.as_deref()).filter(|v| !v.is_empty()) {
            if longest.map_or(true, |l| (v.len(), h) > l) {
                longest = Some((v.len(), h));
            }
        }
    }
    if let Some((_, h)) = longest {
        reqs.push(cat(&[&[0x0a], &u16b(h)]));
    }
    let mut before = Vec::new();
    for r in &reqs {
        before.push(s.att(r)?);
    }
    if !before.iter().any(|b| b.as_ref().is_some_and(|b| b.len() > 10)) {
        return Err(Stop::Inconclusive("no response longer than 10 octets to compare".into()));
    }
    let rsp = s.att(&cat(&[&[0x02], &u16b(10)]))?;
    expect(rsp.is_some(), format!("ATT_EXCHANGE_MTU_REQ with Client Rx MTU 10: {}", describe(rsp.as_deref())))?;
    let rsp = rsp.unwrap();
    let note = if let Some(e) = att_error(Some(&rsp)) {
        expect(e == (0x02, 0x0000, 0x06), format!("ATT_EXCHANGE_MTU_REQ answered with {}", describe(Some(&rsp))))?;
        "request refused with Request Not Supported".to_string()
    } else {
        expect(
            rsp[0] == 0x03 && rsp.len() == 3 && u16::from_le_bytes([rsp[1], rsp[2]]) as usize >= MTU,
            format!(
                "ATT_EXCHANGE_MTU_REQ with Client Rx MTU 10 answered with {}; required an ATT_EXCHANGE_MTU_RSP with a \
                 Server Rx MTU of at least 23",
                hx(&rsp)
            ),
        )?;
        format!("Server Rx MTU {}", u16::from_le_bytes([rsp[1], rsp[2]]))
    };
    let mut after = Vec::new();
    for r in &reqs {
        after.push(s.att(r)?);
    }
    let len_of = |x: &Option<Vec<u8>>| x.as_ref().map(|v| v.len()).unwrap_or(0);
    let mut bad = Vec::new();
    for ((r, b), x) in reqs.iter().zip(&before).zip(&after) {
        let differs = b.is_none() != x.is_none()
            || (b.is_some() && (len_of(b) != len_of(x) || (r[0] != 0x0A && b != x)));
        if differs {
            bad.push(format!(
                "request {}: {} octets before, {} after ({})",
                hx(r),
                len_of(b),
                len_of(x),
                describe(x.as_deref())
            ));
        }
    }
    expect(
        bad.is_empty(),
        format!(
            "after an ATT_EXCHANGE_MTU_REQ with Client Rx MTU 10 the responses are not what they were at ATT_MTU 23: {}",
            bad.join("; ")
        ),
    )?;
    Ok(format!(
        "{note}; {} responses of {} octets are the same before and after",
        reqs.len(),
        before.iter().filter(|b| b.as_ref().is_some_and(|b| !b.is_empty())).map(|b| len_of(b).to_string()).collect::<Vec<_>>().join(", ")
    ))
}

fn never_exceeded(s: &mut BleSession) -> R<String> {
    link(s, LinkOpts::new())?;
    let g = table(s)?;
    let n0 = s.lk().frames.len();
    for h in g.attrs.keys().copied() {
        s.att(&cat(&[&[0x04], &u16b(h), &u16b(0xFFFF)]))?;
        s.att(&cat(&[&[0x0a], &u16b(h)]))?;
    }
    for c in &g.chars {
        s.att(&cat(&[&[0x08], &u16b(c.decl), &u16b(0xFFFF), &CHAR]))?;
        if c.props & 0x02 != 0 {
            s.att(&cat(&[&[0x08], &u16b(1), &u16b(0xFFFF), &c.uuid]))?;
        }
    }
    for sv in &g.services {
        s.att(&cat(&[&[0x10], &u16b(sv.start), &u16b(0xFFFF), &u16b(0x2800)]))?;
    }
    let frames: Vec<L2Frame> =
        s.lk().frames.get(n0..).unwrap_or(&[]).iter().filter(|f| f.cid == CID_ATT).cloned().collect();
    let big = fits(s, n0, MTU)?;
    expect(
        big.is_empty(),
        format!(
            "{} attribute PDUs longer than 23 octets: {}",
            big.len(),
            big.iter().take(3).map(|f| hx(&f.data)).collect::<Vec<_>>().join(", ")
        ),
    )?;
    let full = frames.iter().filter(|f| f.data.len() == MTU).count();
    if full == 0 {
        return Err(Stop::Inconclusive(format!(
            "none of {} responses reached 23 octets: the limit was not put to the test",
            frames.len()
        )));
    }
    Ok(format!(
        "{} responses, the longest {} octets, {full} of exactly 23",
        frames.len(),
        frames.iter().map(|f| f.data.len()).max().unwrap_or(0)
    ))
}

fn wait_server_req(s: &mut BleSession) -> R<()> {
    s.link_until(1000.0, None, |s| Ok(if s.lk().server_reqs.is_empty() { None } else { Some(()) }))?;
    Ok(())
}

fn server_request(s: &mut BleSession) -> R<String> {
    connect_rx_mtu(s, 100, true, LinkOpts::new())?;
    wait_server_req(s)?;
    let Some(own) = server_mtu_req(s) else {
        return Err(Stop::NotApplicable(
            "the server sent no ATT_EXCHANGE_MTU_REQ in the first second of the connection".into(),
        ));
    };
    let fr = s.lk().server_reqs[0].clone();
    expect(fr.data.len() == 3, format!("ATT_EXCHANGE_MTU_REQ of the server: {}; required 3 octets", hx(&fr.data)))?;
    expect(own as usize >= MTU, format!("Client Rx MTU {own} in the server's ATT_EXCHANGE_MTU_REQ; required at least 23"))?;
    let rsp = s.att(&rbt_char())?;
    expect(
        rsp.is_some(),
        format!("Read By Type while the server's MTU request is unanswered: {}", describe(rsp.as_deref())),
    )?;
    let rsp = rsp.unwrap();
    expect(
        rsp.len() <= MTU,
        format!(
            "while its ATT_EXCHANGE_MTU_REQ ({own}) was unanswered the server sent a response of {} octets; required \
             the default ATT_MTU, 23",
            rsp.len()
        ),
    )?;
    s.mtu_release()?;
    s.link_run(1500.0)?;
    let again = s.lk().server_reqs.iter().filter(|f| f.data.first() == Some(&0x02)).count();
    expect(again == 1, format!("the server sent ATT_EXCHANGE_MTU_REQ {again} times in one connection"))?;
    expect(s.alive(3000.0)?, "the connection ended")?;
    let tconnend = s.lk().st.f("tconnend").unwrap_or(0.0);
    Ok(format!(
        "Client Rx MTU {own}, sent once, {:.0} ms after the CONNECT_IND; a response of {} octets while it was unanswered",
        fr.t - tconnend,
        rsp.len()
    ))
}

fn provoke(s: &mut BleSession) -> R<Vec<L2Frame>> {
    let g = table(s)?;
    for (c, h) in cccds(&g) {
        let v: [u8; 2] = if c.props & 0x10 != 0 { [0x01, 0x00] } else { [0x02, 0x00] };
        s.att(&cat(&[&[0x12], &u16b(h), &v]))?;
    }
    let n0 = s.lk().frames.len();
    let mut app = mesh_app(s)?;
    stimulate(s, &mut app, 500.0)?;
    Ok(s.notifications(n0))
}

fn server_request_notifications(s: &mut BleSession) -> R<String> {
    connect_rx_mtu(s, MTU as u16, true, LinkOpts::new())?;
    wait_server_req(s)?;
    if server_mtu_req(s).is_none() {
        return Err(Stop::NotApplicable(
            "the server sent no ATT_EXCHANGE_MTU_REQ in the first second of the connection".into(),
        ));
    }
    let early = provoke(s)?;
    s.mtu_release()?;
    s.link_run(300.0)?;
    link(s, LinkOpts::new())?;
    let ctl = provoke(s)?;
    if ctl.is_empty() {
        return Err(Stop::Inconclusive(
            "control: the bulb sent no notification in a connection whose MTU exchange was completed either".into(),
        ));
    }
    expect(
        early.is_empty(),
        format!(
            "{} notification(s) while the server's ATT_EXCHANGE_MTU_REQ was unanswered: {}...",
            early.len(),
            early.first().map(|f| hx(&f.data[..f.data.len().min(8)])).unwrap_or_default()
        ),
    )?;
    Ok(format!("no notification while the request was unanswered; {} in a connection where it was answered", ctl.len()))
}
