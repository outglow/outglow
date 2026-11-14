//! Helpers of the ATT and GATT test modules (Core 5.4 Vol 3 Part F and Part G),
//! a port of emulator/spec_ble/att_util.py. No expectations here: the
//! functions send requests, name what came back and compare it with what the
//! calling test says its clause requires.

use super::harness::{att_error, hx, BleSession, Char, Gatt, LinkOpts, CID_ATT};
use super::mesh::{MeshApp, OP_POWER};
use crate::session::{Stop, R};

/// The default ATT_MTU on LE (Vol 3 Part G 5.2.1).
pub const MTU: usize = 23;
/// The Bluetooth Base UUID, as sent, without the alias.
pub const BASE_LE: [u8; 12] = [0xfb, 0x34, 0x9b, 0x5f, 0x80, 0x00, 0x00, 0x80, 0x00, 0x10, 0x00, 0x00];
/// Errors that say "not at this security level".
pub const SECURITY: [u8; 4] = [0x05, 0x08, 0x0C, 0x0F];

pub fn err_name(code: u8) -> Option<&'static str> {
    Some(match code {
        0x01 => "Invalid Handle",
        0x02 => "Read Not Permitted",
        0x03 => "Write Not Permitted",
        0x04 => "Invalid PDU",
        0x05 => "Insufficient Authentication",
        0x06 => "Request Not Supported",
        0x07 => "Invalid Offset",
        0x08 => "Insufficient Authorization",
        0x09 => "Prepare Queue Full",
        0x0A => "Attribute Not Found",
        0x0B => "Attribute Not Long",
        0x0C => "Encryption Key Size Too Short",
        0x0D => "Invalid Attribute Value Length",
        0x0E => "Unlikely Error",
        0x0F => "Insufficient Encryption",
        0x10 => "Unsupported Group Type",
        0x11 => "Insufficient Resources",
        0x12 => "Database Out Of Sync",
        0x13 => "Value Not Allowed",
        _ => return None,
    })
}

/// Application Error (0x80-0x9f).
pub fn is_app(code: u8) -> bool {
    (0x80..0xA0).contains(&code)
}

fn app_css(code: u8) -> bool {
    (0x80..0xA0).contains(&code) || code >= 0xE0
}

/// Part F 3.4.9, Table 3.44: is `code` valid in an ATT_ERROR_RSP for request `req`?
pub fn table_3_44(req: u8, code: u8) -> Option<bool> {
    let base: &[u8] = match req {
        0x02 => return Some(code == 0x06),
        0x04 => return Some([0x01, 0x0A].contains(&code)),
        0x06 => return Some([0x01, 0x06, 0x0A].contains(&code)),
        0x08 => &[0x01, 0x12, 0x06, 0x0A, 0x08, 0x05, 0x0F, 0x0C, 0x02],
        0x0A => &[0x01, 0x12, 0x08, 0x05, 0x0F, 0x0C, 0x02],
        0x0C => &[0x01, 0x12, 0x06, 0x08, 0x05, 0x0F, 0x0C, 0x02, 0x07, 0x0B],
        0x0E => &[0x01, 0x12, 0x06, 0x08, 0x05, 0x0F, 0x0C, 0x02],
        0x10 => &[0x01, 0x06, 0x0A, 0x08, 0x05, 0x0F, 0x0C, 0x02, 0x10],
        0x20 => &[0x01, 0x12, 0x06, 0x08, 0x05, 0x0F, 0x0C, 0x02],
        0x12 => &[0x01, 0x12, 0x06, 0x08, 0x05, 0x0F, 0x0C, 0x03, 0x0D],
        0x16 => &[0x01, 0x12, 0x06, 0x08, 0x05, 0x03, 0x09, 0x0F, 0x0C],
        0x18 => &[0x07, 0x0D],
        _ => return None,
    };
    Some(base.contains(&code) || app_css(code))
}

/// Part F 3.3: required for any request they apply to.
pub const GENERIC_3_3: [u8; 4] = [0x04, 0x06, 0x0E, 0x11];

pub fn u16b(v: u16) -> [u8; 2] {
    v.to_le_bytes()
}

/// The 128-bit form, as sent, of a 16-bit UUID (Vol 3 Part B 2.5.1).
pub fn uuid128(alias16: u16) -> Vec<u8> {
    let mut v = BASE_LE.to_vec();
    v.extend_from_slice(&alias16.to_le_bytes());
    v.extend_from_slice(&[0, 0]);
    v
}

pub fn ename(code: u8) -> String {
    let n = err_name(code).unwrap_or(if is_app(code) { "Application Error" } else { "error" });
    format!("{n} ({code:#04x})")
}

/// An answer in words.
pub fn describe(rsp: Option<&[u8]>) -> String {
    let Some(r) = rsp else { return "no response within the 30 s transaction timeout".into() };
    if let Some(e) = att_error(Some(r)) {
        return format!("ATT_ERROR_RSP {}: {}, handle {:#06x}", hx(r), ename(e.2), e.1);
    }
    hx(r)
}

/// None when rsp is an ATT_ERROR_RSP for request `req` with one of `codes` (and
/// the handle, when given); else what came instead and what the caller required.
pub fn error_problem(rsp: Option<&[u8]>, req: u8, codes: &[u8], handle: Option<u16>) -> Option<String> {
    if let Some(e) = att_error(rsp) {
        if e.0 == req && codes.contains(&e.2) && handle.map_or(true, |h| e.1 == h) {
            return None;
        }
    }
    let mut names: Vec<String> = codes.iter().filter(|c| !is_app(**c)).map(|c| ename(*c)).collect();
    if codes.iter().any(|c| is_app(*c)) {
        names.push("an Application Error (0x80-0x9f)".into());
    }
    Some(format!(
        "{}; required {}{}",
        describe(rsp),
        names.join(" or "),
        handle.map(|h| format!(", handle {h:#06x}")).unwrap_or_default()
    ))
}

/// The first n of a list of deviations, as one text.
pub fn some(bad: &[String], n: usize) -> String {
    let mut s = bad.iter().take(n).cloned().collect::<Vec<_>>().join("; ");
    if bad.len() > n {
        s += &format!("; and {} more", bad.len() - n);
    }
    s
}

/// A connection with a short interval (default 6), then 60 ms: a server that
/// sends an ATT_EXCHANGE_MTU_REQ of its own does so right after the connection.
pub fn link(s: &mut BleSession, o: LinkOpts) -> R<()> {
    let o = if o.kv.iter().any(|(k, _)| k == "interval") { o } else { o.set("interval", 6) };
    s.connect(o)?;
    s.link_run(60.0)?;
    Ok(())
}

/// Sends a request and waits the transaction timeout; after a timeout the link
/// is a new connection (no further request may be sent on that bearer, Part F 3.3.3).
pub fn ask(s: &mut BleSession, pdu: &[u8]) -> R<Option<Vec<u8>>> {
    let rsp = s.att(pdu)?;
    if rsp.is_none() {
        link(s, LinkOpts::new())?;
    }
    Ok(rsp)
}

/// One error case: (what, pdu, codes, handle).
pub type Case = (String, Vec<u8>, Vec<u8>, Option<u16>);

/// Sends each request; the deviations as texts. Stops after `max_timeouts` unanswered requests.
pub fn error_cases(s: &mut BleSession, cases: &[Case], max_timeouts: usize) -> R<Vec<String>> {
    let (mut bad, mut timeouts) = (Vec::new(), 0);
    for (i, (what, pdu, codes, handle)) in cases.iter().enumerate() {
        let rsp = ask(s, pdu)?;
        if let Some(p) = error_problem(rsp.as_deref(), pdu[0], codes, *handle) {
            bad.push(format!("{what} ({}): {p}", hx(pdu)));
        }
        if rsp.is_none() {
            timeouts += 1;
            if timeouts >= max_timeouts && i + 1 < cases.len() {
                bad.push(format!("{} more cases not tried after {timeouts} unanswered requests", cases.len() - i - 1));
                break;
            }
        }
    }
    Ok(bad)
}

/// A connection whose central answers an ATT_EXCHANGE_MTU_REQ of the server
/// with rx_mtu; with hold, only when the test calls `s.mtu_release()`.
pub fn connect_rx_mtu(s: &mut BleSession, rx_mtu: u16, hold: bool, o: LinkOpts) -> R<()> {
    let o = if o.kv.iter().any(|(k, _)| k == "interval") { o } else { o.set("interval", 6) };
    s.connect_with(o, false, 0.0)?;
    s.lk_mut().rx_mtu = Some((rx_mtu, hold));
    let ok = s.until(8000.0, 5.0, |s| {
        let st = s.poll()?;
        Ok(if st.state() == "connected" && st.heard() { Some(()) } else { None })
    })?;
    if ok.is_none() {
        return Err(Stop::Inconclusive(format!(
            "no connection: central state {}, {} PDUs heard",
            s.lk().st.state(),
            s.lk().pdus.len()
        )));
    }
    s.link_run(60.0)?;
    Ok(())
}

/// Client Rx MTU of the ATT_EXCHANGE_MTU_REQ the server sent on this link, or None.
pub fn server_mtu_req(s: &BleSession) -> Option<u16> {
    s.lk()
        .server_reqs
        .iter()
        .find(|f| f.data.first() == Some(&0x02) && f.data.len() >= 3)
        .map(|f| u16::from_le_bytes([f.data[1], f.data[2]]))
}

/// The attribute table as discovery shows it; Inconclusive when discovery gave nothing.
pub fn table(s: &mut BleSession) -> R<Gatt> {
    let g = s.gatt()?;
    if g.attrs.is_empty() || g.services.is_empty() {
        let p = if g.problems.is_empty() { String::new() } else { format!(": {}", g.problems.join("; ")) };
        return Err(Stop::Inconclusive(format!("the attribute table could not be discovered{p}")));
    }
    Ok(g)
}

pub fn read(s: &mut BleSession, h: u16) -> R<Option<Vec<u8>>> {
    let mut q = vec![0x0a];
    q.extend_from_slice(&h.to_le_bytes());
    s.att(&q)
}

/// The value of an ATT_READ_RSP, else None.
pub fn value(rsp: Option<&[u8]>) -> Option<Vec<u8>> {
    rsp.filter(|r| r.first() == Some(&0x0b)).map(|r| r[1..].to_vec())
}

/// {handle: answer to a Read Request}, every attribute of the table read once per session.
pub fn read_all(s: &mut BleSession) -> R<Vec<(u16, Option<Vec<u8>>)>> {
    if let Some(v) = s.note::<Vec<(u16, Option<Vec<u8>>)>>("att_values") {
        return Ok(v);
    }
    let g = table(s)?;
    let mut vals = Vec::new();
    for h in g.attrs.keys() {
        let r = read(s, *h)?;
        vals.push((*h, r));
    }
    s.set_note("att_values", vals.clone());
    Ok(vals)
}

pub fn blob(s: &mut BleSession, h: u16, offset: u16) -> R<Option<Vec<u8>>> {
    let mut q = vec![0x0c];
    q.extend_from_slice(&h.to_le_bytes());
    q.extend_from_slice(&offset.to_le_bytes());
    s.att(&q)
}

/// (value, complete): the value by Read Request and, when the response is full,
/// Read Blob Requests. (None, false) when the attribute gives no value.
pub fn full_value(s: &mut BleSession, h: u16, mtu: usize) -> R<(Option<Vec<u8>>, bool)> {
    let r = read(s, h)?;
    let Some(mut v) = value(r.as_deref()) else { return Ok((None, false)) };
    if v.len() < mtu - 1 {
        return Ok((Some(v), true));
    }
    while v.len() <= 512 {
        let rsp = blob(s, h, v.len() as u16)?;
        let Some(rsp) = rsp.filter(|r| r.first() == Some(&0x0d)) else { return Ok((Some(v), false)) };
        v.extend_from_slice(&rsp[1..]);
        if rsp.len() - 1 < mtu - 1 {
            return Ok((Some(v), true));
        }
    }
    Ok((Some(v), false))
}

/// Attribute PDUs of the server on this link, from frame n0 on, longer than mtu.
pub fn fits(s: &mut BleSession, n0: usize, mtu: usize) -> R<Vec<super::harness::L2Frame>> {
    s.poll()?;
    Ok(s.lk().frames.get(n0..).unwrap_or(&[]).iter().filter(|f| f.cid == CID_ATT && f.data.len() > mtu).cloned().collect())
}

/// The characteristic whose value attribute has handle h.
pub fn char_of(g: &Gatt, h: u16) -> Option<&Char> {
    g.chars.iter().find(|c| c.value == h)
}

/// (characteristic, handle of its Client Characteristic Configuration descriptor).
pub fn cccds(g: &Gatt) -> Vec<(Char, u16)> {
    let mut v = Vec::new();
    for c in &g.chars {
        for (h, u) in &c.descs {
            if u[..] == [0x02, 0x29] {
                v.push((c.clone(), *h));
            }
        }
    }
    v
}

/// A 16-bit UUID that is neither an attribute type nor a service UUID on this server.
pub fn absent_uuid16(g: &Gatt) -> R<u16> {
    let used: Vec<Vec<u8>> = g
        .attrs
        .values()
        .cloned()
        .chain(g.services.iter().map(|s| s.uuid.clone()))
        .chain(g.chars.iter().map(|c| c.uuid.clone()))
        .collect();
    for c in [0x2A19u16, 0x2A6E, 0x2AFF, 0x2B00] {
        if !used.contains(&c.to_le_bytes().to_vec()) && !used.contains(&uuid128(c)) {
            return Ok(c);
        }
    }
    Err(Stop::Inconclusive("no unused 16-bit UUID among the candidates".into()))
}

/// The application protocol of the bulb, logged in: only a means to make it
/// notify (the vendor firmware notifies only when the login comes 300 ms or
/// more after the connection was made).
pub fn mesh_app(s: &mut BleSession) -> R<MeshApp> {
    let app = MeshApp::new(s)?;
    s.link_run(400.0)?;
    app.login(s)
}

/// What makes these bulbs notify, in the vendor application's order: its own
/// notification switch, then two commands. The notifications and indications that came.
pub fn stimulate(s: &mut BleSession, app: &mut MeshApp, wait_ms: f64) -> R<Vec<super::harness::L2Frame>> {
    s.poll()?;
    let n0 = s.lk().frames.len();
    macro_rules! app_try {
        ($e:expr) => {
            if let Err(e) = $e? {
                return Ok(Err(e));
            }
        };
    }
    let r = (|| -> R<Result<(), String>> {
        app_try!(app.status_on(s));
        s.link_run(wait_ms)?;
        app_try!(app.command(s, OP_POWER, &[0]));
        s.link_run(wait_ms)?;
        app_try!(app.command(s, OP_POWER, &[1]));
        s.link_run(wait_ms)?;
        Ok(Ok(()))
    })();
    match r {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            return Err(Stop::Inconclusive(format!(
                "the application protocol could not be used to provoke a notification ({e})"
            )))
        }
        Err(e) => return Err(e),
    }
    Ok(s.notifications(n0))
}

/// (handle, handle-value pair) of the characteristic declarations, as Read By Type returns them.
pub fn decl_pairs(g: &Gatt) -> Vec<(u16, Vec<u8>)> {
    g.chars
        .iter()
        .map(|c| {
            let mut d = c.decl.to_le_bytes().to_vec();
            d.push(c.props);
            d.extend_from_slice(&c.value.to_le_bytes());
            d.extend_from_slice(&c.uuid);
            (c.decl, d)
        })
        .collect()
}

/// Of (handle, data) in handle order: those in start..end that one Read By Type /
/// Read By Group Type response holds when the server puts in as many as possible.
pub fn leading_run(pairs: &[(u16, Vec<u8>)], start: u16, end: u16, mtu: usize) -> Vec<Vec<u8>> {
    let inr: Vec<&Vec<u8>> = pairs.iter().filter(|(h, _)| start <= *h && *h <= end).map(|(_, d)| d).collect();
    let mut out: Vec<Vec<u8>> = Vec::new();
    for d in &inr {
        if d.len() != inr[0].len() || (out.len() + 1) * d.len() > mtu - 2 {
            break;
        }
        out.push((*d).clone());
    }
    out
}
