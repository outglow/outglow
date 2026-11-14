//! Groups and Scenes clusters of a light (ZCL rev 8 clauses 3.6 and 3.7; ZLO
//! device specification 15-0014-05 clause 7.2.2 for the commands a dimmable
//! light must accept), ported from emulator/spec/zcl_groups_scenes.py.
//!
//! Status values are those of ZCL8: a duplicate Add Group is SUCCESS
//! (DUPLICATE_EXISTS 0x8a is retired), a scene command for a group the
//! endpoint is not in is INVALID_FIELD. The light is observed on the PWM
//! output and through the On/Off and Level attributes.

use std::collections::HashMap;

use super::misc_ota_gp::py_hex_list;
use super::Test;
use crate::air::NodeId;
use crate::frames::{py_bytes, py_value, ZclValue};
use crate::session::{expect, CapFrame, Session, Stop, R};
use crate::station::Tx;

const S: &str = "zcl-groups";

const SUCCESS: u8 = 0x00;
const INVALID_FIELD: u8 = 0x85;
const INVALID_VALUE: u8 = 0x87;
const INSUFFICIENT_SPACE: u8 = 0x89;
const NOT_FOUND: u8 = 0x8B;
const IDENTIFY: u16 = 0x0003;
const GROUPS: u16 = 0x0004;
const SCENES: u16 = 0x0005;
const ONOFF: u16 = 0x0006;
const LEVEL: u16 = 0x0008;
const G1: u16 = 0x1001;
const G2: u16 = 0x1002;
const G_ABSENT: u16 = 0x2ABC;

// ------------------------------------------------------------------ helpers
fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Python's repr of a float.
pub fn py_float(x: f64) -> String {
    if x.is_finite() && x.fract() == 0.0 && x.abs() < 1e16 {
        format!("{x:.1}")
    } else {
        format!("{x}")
    }
}

/// Python's `round(x, 1)`.
pub fn round1(x: f64) -> f64 {
    format!("{x:.1}").parse().unwrap()
}

fn le16(p: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([p[i], p[i + 1]])
}

fn bhb(a: u8, b: u16, c: u8) -> Vec<u8> {
    let mut v = vec![a];
    v.extend_from_slice(&b.to_le_bytes());
    v.push(c);
    v
}

fn bh(a: u8, b: u16) -> Vec<u8> {
    let mut v = vec![a];
    v.extend_from_slice(&b.to_le_bytes());
    v
}

fn hb(a: u16, b: u8) -> Vec<u8> {
    let mut v = a.to_le_bytes().to_vec();
    v.push(b);
    v
}

/// True when, since the mark, a unicast frame of the Trust Center went through
/// a whole round of MAC retries without an acknowledgement.
fn air_disturbed(s: &mut Session, since: usize, lost: usize) -> bool {
    if s.w.coord.st.no_ack.len() > lost {
        return true;
    }
    let mut seen: HashMap<Vec<u8>, usize> = HashMap::new();
    for f in s.frames(since, Some(NodeId::Coord), |_| true) {
        if f.mac.as_ref().is_some_and(|m| m.ty == 1 && m.ar) {
            *seen.entry((*f.psdu).clone()).or_insert(0) += 1;
        }
    }
    seen.values().any(|n| *n > 4)
}

/// A test that fails while a request of the Trust Center went unacknowledged
/// for a whole round of MAC retries is run once more (the `T` wrapper).
fn retry(s: &mut Session, f: fn(&mut Session) -> R<String>) -> R<String> {
    let lost = s.w.coord.st.no_ack.len();
    let since = s.mark();
    match f(s) {
        Err(e @ (Stop::Fail(_) | Stop::Inconclusive(_))) => {
            if !air_disturbed(s, since, lost) {
                return Err(e);
            }
            s.run(700.0)?;
            f(s)
        }
        r => r,
    }
}

fn st(v: Option<u8>) -> String {
    match v {
        None => "none".into(),
        Some(0x8A) => format!("{:#04x} (retired in ZCL8: DUPLICATE_EXISTS)", 0x8Au8),
        Some(v) => format!("{v:#04x}"),
    }
}

fn text(f: &Option<CapFrame>) -> String {
    let Some(f) = f else { return "no response".into() };
    let z = f.zcl().unwrap();
    if !z.specific && z.cmd == 0x0B && z.payload.len() >= 2 {
        return format!("Default Response to command {:#04x} with status {}", z.payload[0], st(Some(z.payload[1])));
    }
    format!("{} command {:#04x} payload {}", if z.specific { "cluster" } else { "general" }, z.cmd, hex(&z.payload))
}

/// A cluster command that has a response command with the same identifier; returns its payload.
fn rsp(s: &mut Session, cluster: u16, cmd: u8, payload: &[u8], what: &str, o: Tx) -> R<Vec<u8>> {
    let f = s.zcl(cluster, cmd, payload, o)?;
    expect(f.is_some(), format!("no response to {what}"))?;
    let z = f.as_ref().unwrap().zcl().unwrap();
    expect(z.specific && z.cmd == cmd && z.to_client, format!("answer to {what}: {}", text(&f)))?;
    Ok(z.payload)
}

/// A broadcast or groupcast request for which an answer is expected, repeated
/// before 'no answer' is concluded.
fn ask_broadcast(s: &mut Session, cluster: u16, cmd: u8, payload: &[u8], o: Tx) -> R<Option<CapFrame>> {
    let mut f = None;
    for _ in 0..3 {
        f = s.zcl(cluster, cmd, payload, o.clone())?;
        if f.is_some() {
            break;
        }
        s.run(300.0)?;
    }
    Ok(f)
}

fn default_status(f: &Option<CapFrame>) -> Option<u8> {
    let z = f.as_ref()?.zcl()?;
    if !z.specific && z.cmd == 0x0B && z.payload.len() >= 2 {
        return Some(z.payload[1]);
    }
    None
}

// ---- Groups helpers
fn add_group(s: &mut Session, gid: u16, name: &[u8]) -> R<Vec<u8>> {
    let mut pl = hb(gid, name.len() as u8);
    pl.extend_from_slice(name);
    rsp(s, GROUPS, 0x00, &pl, &format!("Add Group {gid:#06x}"), Tx::new())
}

fn view_group(s: &mut Session, gid: u16) -> R<Vec<u8>> {
    rsp(s, GROUPS, 0x01, &gid.to_le_bytes(), &format!("View Group {gid:#06x}"), Tx::new())
}

fn remove_group(s: &mut Session, gid: u16) -> R<Vec<u8>> {
    rsp(s, GROUPS, 0x03, &gid.to_le_bytes(), &format!("Remove Group {gid:#06x}"), Tx::new())
}

/// (capacity, [group ids]) from Get Group Membership.
fn membership_with(s: &mut Session, ask: &[u16], o: Tx) -> R<(u8, Vec<u16>)> {
    let mut pl = vec![ask.len() as u8];
    for g in ask {
        pl.extend_from_slice(&g.to_le_bytes());
    }
    let p = rsp(s, GROUPS, 0x02, &pl, "Get Group Membership", o)?;
    expect(p.len() >= 2 && p.len() == 2 + 2 * p[1] as usize, format!("Get Group Membership Response payload {}", hex(&p)))?;
    Ok((p[0], (0..p[1] as usize).map(|k| le16(&p, 2 + 2 * k)).collect()))
}

fn membership(s: &mut Session) -> R<(u8, Vec<u16>)> {
    membership_with(s, &[], Tx::new())
}

/// No scenes, no groups, not identifying.
fn clean(s: &mut Session) -> R<()> {
    if s.has_cluster(SCENES)? {
        let (_, groups) = membership(s)?;
        for g in &groups {
            s.zcl(SCENES, 0x03, &g.to_le_bytes(), Tx::new().max_ms(600.0))?;
        }
        for g in [G1, G2] {
            if !groups.contains(&g) {
                // leftovers of a group that was removed with its scenes still stored
                s.zcl(GROUPS, 0x00, &hb(g, 0), Tx::new().max_ms(600.0))?;
                s.zcl(SCENES, 0x03, &g.to_le_bytes(), Tx::new().max_ms(600.0))?;
            }
        }
        s.zcl(SCENES, 0x03, &0u16.to_le_bytes(), Tx::new().max_ms(600.0))?;
    }
    s.zcl(GROUPS, 0x04, &[], Tx::new().ddr().max_ms(300.0))?;
    s.run(200.0)?;
    s.zcl(IDENTIFY, 0x00, &0u16.to_le_bytes(), Tx::new().ddr().max_ms(300.0))?;
    Ok(())
}

/// Runs `body`, then `clean` (Python try/finally: an error of clean replaces the body's result).
fn cleaned<T>(s: &mut Session, body: impl FnOnce(&mut Session) -> R<T>) -> R<T> {
    let r = body(s);
    clean(s)?;
    r
}

fn need_group(s: &mut Session, gid: u16) -> R<()> {
    let p = add_group(s, gid, b"")?;
    if p.first() != Some(&0) {
        return Err(Stop::Inconclusive(format!("group {gid:#06x} could not be added: status {}", st(p.first().copied()))));
    }
    Ok(())
}

// ---- light helpers
fn set_level(s: &mut Session, level: u8) -> R<()> {
    s.zcl(LEVEL, 0x04, &[level, 0, 0], Tx::new().ddr().max_ms(300.0))?;
    s.run(500.0)
}

fn set_on(s: &mut Session, on: bool) -> R<()> {
    s.zcl(ONOFF, if on { 0x01 } else { 0x00 }, &[], Tx::new().ddr().max_ms(300.0))?;
    s.run(500.0)
}

type V = Option<ZclValue>;

fn is_int(v: &V, x: i64) -> bool {
    v.as_ref().and_then(|v| v.int()) == Some(x)
}

/// `{v:#06x}` of an integer attribute value.
fn hx6(v: &V) -> R<String> {
    match v.as_ref().and_then(|v| v.int()) {
        Some(i) => Ok(format!("{i:#06x}")),
        None => Err(Stop::Error(format!("TypeError: unsupported format string passed to {}.__format__", py_value(v)))),
    }
}

/// (OnOff attribute, CurrentLevel attribute, light on per PWM, duty in %).
fn state(s: &mut Session) -> R<(V, V, bool, f64)> {
    let on = s.value(ONOFF, 0x0000)?;
    let lvl = s.value(LEVEL, 0x0000)?;
    let li = s.light();
    Ok((on, lvl, li.on, round1(li.duty)))
}

fn py_bool(b: bool) -> &'static str {
    if b {
        "True"
    } else {
        "False"
    }
}

// ------------------------------------------------------------------ Groups
fn group_add(s: &mut Session) -> R<String> {
    s.need_cluster(GROUPS)?;
    clean(s)?;
    cleaned(s, |s| {
        let p = add_group(s, G1, b"")?;
        expect(p == bh(SUCCESS, G1), format!("Add Group Response {}; required status 00, group {G1:#06x}", hex(&p)))?;
        let p = view_group(s, G1)?;
        expect(p.get(..3) == Some(&bh(SUCCESS, G1)[..]), format!("View Group Response {}", hex(&p)))?;
        expect(
            p.len() >= 4 && p.len() == 4 + p[3] as usize,
            format!("View Group Response {} lacks a well-formed group name string", hex(&p)),
        )?;
        let (cap, groups) = membership(s)?;
        expect(groups == [G1], format!("Get Group Membership (empty list) returns {}", py_hex_list(&groups)))?;
        Ok(format!("capacity {cap}"))
    })
}

fn group_add_invalid(s: &mut Session) -> R<String> {
    s.need_cluster(GROUPS)?;
    clean(s)?;
    cleaned(s, |s| {
        let mut obs = Vec::new();
        for gid in [0x0000u16, 0xFFF8] {
            let p = add_group(s, gid, b"")?;
            obs.push(format!("group {gid:#06x}: status {}", st(Some(p[0]))));
            expect(p[0] == INVALID_VALUE, obs.join("; ") + "; INVALID_VALUE (0x87) is required")?;
        }
        let (_, groups) = membership(s)?;
        expect(
            groups.is_empty(),
            format!("group table holds {} after two refused Add Group commands", py_hex_list(&groups)),
        )?;
        Ok(String::new())
    })
}

fn group_add_duplicate(s: &mut Session) -> R<String> {
    s.need_cluster(GROUPS)?;
    clean(s)?;
    cleaned(s, |s| {
        need_group(s, G1)?;
        let p = add_group(s, G1, b"")?;
        expect(
            p[0] == SUCCESS,
            format!("second Add Group for the same group: status {}; ZCL8 requires SUCCESS", st(Some(p[0]))),
        )?;
        let (_, groups) = membership(s)?;
        expect(groups == [G1], format!("membership after adding the same group twice: {}", py_hex_list(&groups)))?;
        Ok(String::new())
    })
}

fn group_view_errors(s: &mut Session) -> R<String> {
    s.need_cluster(GROUPS)?;
    clean(s)?;
    let p = view_group(s, G_ABSENT)?;
    expect(
        p[0] == NOT_FOUND,
        format!("View Group for an unknown group: status {}; NOT_FOUND (0x8b) is required", st(Some(p[0]))),
    )?;
    expect(le16(&p, 1) == G_ABSENT, format!("View Group Response {} does not echo the group id", hex(&p)))?;
    let p = view_group(s, 0xFFF9)?;
    expect(
        p[0] == INVALID_VALUE,
        format!("View Group for 0xfff9: status {}; INVALID_VALUE (0x87) is required", st(Some(p[0]))),
    )?;
    Ok(String::new())
}

fn group_membership(s: &mut Session) -> R<String> {
    s.need_cluster(GROUPS)?;
    clean(s)?;
    cleaned(s, |s| {
        let (cap0, groups) = membership(s)?;
        expect(groups.is_empty(), format!("precondition: the group table is not empty: {}", py_hex_list(&groups)))?;
        need_group(s, G1)?;
        need_group(s, G2)?;
        let (cap2, groups) = membership(s)?;
        let mut sorted = groups.clone();
        sorted.sort();
        expect(sorted == [G1, G2], format!("empty list: {}; both groups required", py_hex_list(&groups)))?;
        let (_, groups) = membership_with(s, &[G2, G_ABSENT], Tx::new())?;
        expect(groups == [G2], format!("list [G2, unknown]: {}; only G2 required", py_hex_list(&groups)))?;
        let (_, groups) = membership_with(s, &[G_ABSENT], Tx::new())?;
        expect(groups.is_empty(), format!("list [unknown]: {}; a response with count 0 required", py_hex_list(&groups)))?;
        if 0 < cap0 && cap0 < 0xFE {
            expect(
                cap2 as i64 == cap0 as i64 - 2,
                format!("Capacity went from {cap0} to {cap2} after adding two groups"),
            )?;
        }
        Ok(format!("capacity {cap0} -> {cap2}"))
    })
}

fn group_remove(s: &mut Session) -> R<String> {
    s.need_cluster(GROUPS)?;
    clean(s)?;
    cleaned(s, |s| {
        need_group(s, G1)?;
        let p = remove_group(s, G1)?;
        expect(p == bh(SUCCESS, G1), format!("Remove Group Response {}", hex(&p)))?;
        let (_, groups) = membership(s)?;
        expect(!groups.contains(&G1), "the group is still reported after Remove Group Response SUCCESS")?;
        let p = remove_group(s, G1)?;
        expect(
            p[0] == NOT_FOUND,
            format!("Remove Group for a group no longer present: status {}; NOT_FOUND required", st(Some(p[0]))),
        )?;
        let p = remove_group(s, 0x0000)?;
        expect(
            p[0] == INVALID_VALUE,
            format!("Remove Group 0x0000: status {}; INVALID_VALUE (0x87) required", st(Some(p[0]))),
        )?;
        Ok(String::new())
    })
}

fn group_remove_all(s: &mut Session) -> R<String> {
    s.need_cluster(GROUPS)?;
    clean(s)?;
    need_group(s, G1)?;
    need_group(s, G2)?;
    let f = s.zcl(GROUPS, 0x04, &[], Tx::new())?;
    expect(default_status(&f) == Some(SUCCESS), format!("answer to Remove All Groups: {}", text(&f)))?;
    let (_, groups) = membership(s)?;
    expect(groups.is_empty(), format!("groups left after Remove All Groups: {}", py_hex_list(&groups)))?;
    Ok(String::new())
}

fn group_add_if_identifying(s: &mut Session) -> R<String> {
    s.need_cluster(GROUPS)?;
    clean(s)?;
    cleaned(s, |s| {
        let pl = hb(G1, 0);
        let f = s.zcl(GROUPS, 0x05, &pl, Tx::new())?;
        let (_, groups) = membership(s)?;
        expect(!groups.contains(&G1), "the group was added although the device was not identifying")?;
        expect(
            default_status(&f) == Some(SUCCESS),
            format!("not identifying: answer {}; Default Response SUCCESS required", text(&f)),
        )?;
        s.zcl(IDENTIFY, 0x00, &30u16.to_le_bytes(), Tx::new())?;
        let f = s.zcl(GROUPS, 0x05, &pl, Tx::new())?;
        let (_, groups) = membership(s)?;
        expect(groups.contains(&G1), format!("identifying: the group was not added (answer: {})", text(&f)))?;
        expect(
            default_status(&f) == Some(SUCCESS),
            format!("identifying: answer {}; Default Response SUCCESS required", text(&f)),
        )?;
        Ok(String::new())
    })
}

fn group_names(s: &mut Session) -> R<String> {
    s.need_cluster(GROUPS)?;
    clean(s)?;
    cleaned(s, |s| {
        let (stt, t, v) = s.read1(GROUPS, 0x0000)?;
        expect(
            stt == Some(SUCCESS) && t == Some(0x18),
            format!("NameSupport: status {}, type {}", st(stt), t.map(|t| t.to_string()).unwrap_or("None".into())),
        )?;
        let p = add_group(s, G1, b"abc")?;
        expect(p[0] == SUCCESS, format!("Add Group with a name: status {}", st(Some(p[0]))))?;
        let p = view_group(s, G1)?;
        expect(
            p[0] == SUCCESS && p.len() >= 4 && p.len() == 4 + p[3] as usize,
            format!("View Group Response {}", hex(&p)),
        )?;
        let name = &p[4..];
        let Some(v) = v.as_ref().and_then(|v| v.int()) else {
            return Err(Stop::Error("TypeError: unsupported operand type(s) for &".into()));
        };
        if v & 0x80 != 0 {
            expect(
                name == b"abc",
                format!("names supported (NameSupport {v:#04x}) but View Group returns {}", py_bytes(name)),
            )?;
        } else {
            expect(
                name.is_empty(),
                format!("names not supported (NameSupport {v:#04x}) but View Group returns {}", py_bytes(name)),
            )?;
        }
        Ok(format!("NameSupport {v:#04x}"))
    })
}

fn groupcast(s: &mut Session) -> R<String> {
    s.need_cluster(GROUPS)?;
    s.need_cluster(ONOFF)?;
    clean(s)?;
    cleaned(s, |s| {
        need_group(s, G1)?;
        expect(s.light().on, "precondition: light is on")?;
        let gc = |g: u16| Tx::new().dst(0xFFFD).group(g).max_ms(1200.0);
        let f = s.zcl(ONOFF, 0x00, &[], gc(G_ABSENT))?;
        s.run(500.0)?;
        let still_on = s.light().on;
        expect(still_on, "an Off addressed to a group the endpoint is not in was executed")?;
        expect(f.is_none(), format!("a groupcast to a foreign group was answered: {}", text(&f)))?;
        let mut f = None;
        let mut off = false;
        for _ in 0..3 {
            // a single groupcast copy can be lost on the air
            f = s.zcl(ONOFF, 0x00, &[], gc(G1))?;
            s.run(500.0)?;
            off = !s.light().on;
            if off || f.is_some() {
                break;
            }
        }
        set_on(s, true)?;
        expect(off, "an Off addressed to a group the endpoint is in was not executed")?;
        expect(f.is_none(), format!("a groupcast command (Disable Default Response = 0) was answered: {}", text(&f)))?;
        let f = s.zcl(ONOFF, 0x7E, &[], gc(G1))?;
        expect(f.is_none(), format!("an unsupported command sent to the group was answered: {}", text(&f)))?;
        Ok(String::new())
    })
}

fn group_cmds_broadcast(s: &mut Session) -> R<String> {
    s.need_cluster(GROUPS)?;
    clean(s)?;
    cleaned(s, |s| {
        let bc = || Tx::new().dst(0xFFFD).max_ms(1200.0);
        let mut f = None;
        let mut groups = Vec::new();
        for _ in 0..3 {
            // a single broadcast copy can be lost on the air
            f = s.zcl(GROUPS, 0x00, &hb(G2, 0), bc())?;
            groups = membership(s)?.1;
            if groups.contains(&G2) || f.is_some() {
                break;
            }
        }
        expect(f.is_none(), format!("a broadcast Add Group was answered: {}", text(&f)))?;
        expect(groups.contains(&G2), "a broadcast Add Group was not carried out (negative control: nothing happened)")?;
        let f = s.zcl(GROUPS, 0x01, &G2.to_le_bytes(), bc())?;
        expect(f.is_none(), format!("a broadcast View Group was answered: {}", text(&f)))?;
        let mut f = None;
        for _ in 0..3 {
            f = s.zcl(GROUPS, 0x03, &G2.to_le_bytes(), bc().group(G2))?;
            groups = membership(s)?.1;
            if !groups.contains(&G2) || f.is_some() {
                break;
            }
        }
        expect(f.is_none(), format!("a groupcast Remove Group was answered: {}", text(&f)))?;
        expect(!groups.contains(&G2), "a groupcast Remove Group was not carried out")?;
        Ok(String::new())
    })
}

fn group_table_full(s: &mut Session) -> R<String> {
    s.need_cluster(GROUPS)?;
    clean(s)?;
    cleaned(s, |s| {
        let (cap0, _) = membership(s)?;
        let known = 0 < cap0 && cap0 < 0xFE;
        let limit = if known { cap0 as u16 } else { 40 };
        if limit > 40 {
            return Err(Stop::Inconclusive(format!("the group table reports room for {cap0} groups; not filled")));
        }
        let mut added = 0u16;
        let mut last = None;
        for k in 0..=limit {
            let p = add_group(s, 0x3000 + k, b"")?;
            last = Some(p[0]);
            if p[0] != SUCCESS {
                break;
            }
            added += 1;
        }
        if last == Some(SUCCESS) {
            if known {
                return Err(Stop::Fail(format!("Capacity said {cap0} but {added} groups were accepted")));
            }
            return Err(Stop::Inconclusive(format!(
                "{added} groups accepted, capacity unknown ({cap0:#04x}); table not filled"
            )));
        }
        expect(
            last == Some(INSUFFICIENT_SPACE),
            format!(
                "Add Group number {}: status {}; INSUFFICIENT_SPACE (0x89) is required when the table is full",
                added + 1,
                st(last)
            ),
        )?;
        if known {
            expect(added == cap0 as u16, format!("Capacity said {cap0}, the table was full after {added} groups"))?;
        }
        let (cap, _) = membership(s)?;
        expect(cap == 0 || cap == 0xFF, format!("Capacity {cap} reported by a full group table"))?;
        Ok(format!("table holds {added} groups"))
    })
}

fn group_membership_broadcast(s: &mut Session) -> R<String> {
    s.need_cluster(GROUPS)?;
    clean(s)?;
    cleaned(s, |s| {
        need_group(s, G1)?;
        let mut pl = vec![1];
        pl.extend_from_slice(&G_ABSENT.to_le_bytes());
        let f = s.zcl(GROUPS, 0x02, &pl, Tx::new().dst(0xFFFD).max_ms(1200.0))?;
        expect(
            f.is_none(),
            format!("a broadcast Get Group Membership naming only a foreign group was answered: {}", text(&f)),
        )?;
        let mut mine = vec![1];
        mine.extend_from_slice(&G1.to_le_bytes());
        for (what, pl) in [("an empty list", vec![0u8]), ("a list naming a group of the endpoint", mine)] {
            let f = ask_broadcast(s, GROUPS, 0x02, &pl, Tx::new().dst(0xFFFD).max_ms(1500.0))?;
            expect(f.is_some(), format!("no answer to a broadcast Get Group Membership with {what} (sent three times)"))?;
            let fr = f.as_ref().unwrap();
            let z = fr.zcl().unwrap();
            expect(
                z.specific && z.cmd == 0x02 && fr.nwk.as_ref().unwrap().dst == 0x0000,
                format!("answer with {what}: {}", text(&f)),
            )?;
            let p = z.payload;
            let groups: Vec<u16> = (0..p[1] as usize).map(|k| le16(&p, 2 + 2 * k)).collect();
            expect(groups == [G1], format!("{what}: groups {}", py_hex_list(&groups)))?;
        }
        Ok(String::new())
    })
}

// ------------------------------------------------------------------ Scenes
fn ext_onoff(on: bool) -> Vec<u8> {
    let mut v = ONOFF.to_le_bytes().to_vec();
    v.extend_from_slice(&[1, if on { 1 } else { 0 }]);
    v
}

fn ext_level(level: u8) -> Vec<u8> {
    let mut v = LEVEL.to_le_bytes().to_vec();
    v.extend_from_slice(&[1, level]);
    v
}

fn cat(a: Vec<u8>, b: Vec<u8>) -> Vec<u8> {
    let mut v = a;
    v.extend(b);
    v
}

fn add_scene(s: &mut Session, gid: u16, sid: u8, transition: u16, ext: &[u8], name: &[u8], enhanced: bool) -> R<Vec<u8>> {
    let mut pl = hb(gid, sid);
    pl.extend_from_slice(&transition.to_le_bytes());
    pl.push(name.len() as u8);
    pl.extend_from_slice(name);
    pl.extend_from_slice(ext);
    let what = format!("{}Add Scene {gid:#06x}/{sid}", if enhanced { "Enhanced " } else { "" });
    rsp(s, SCENES, if enhanced { 0x40 } else { 0x00 }, &pl, &what, Tx::new())
}

fn add_scene1(s: &mut Session, gid: u16, sid: u8, transition: u16, ext: &[u8]) -> R<Vec<u8>> {
    add_scene(s, gid, sid, transition, ext, b"", false)
}

/// What a View Scene Response says.
struct ViewScene {
    status: u8,
    g: u16,
    sc: u8,
    tr: Option<u16>,
    /// extension field sets in the order of the response
    sets: Vec<(u16, Vec<u8>)>,
    raw: Vec<u8>,
}

impl ViewScene {
    fn get(&self, cl: u16) -> Option<&[u8]> {
        self.sets.iter().find(|(c, _)| *c == cl).map(|(_, v)| &v[..])
    }
    fn tr_s(&self) -> String {
        self.tr.map(|t| t.to_string()).unwrap_or("None".into())
    }
}

fn view_scene(s: &mut Session, gid: u16, sid: u8, enhanced: bool) -> R<ViewScene> {
    let what = format!("{}View Scene {gid:#06x}/{sid}", if enhanced { "Enhanced " } else { "" });
    let p = rsp(s, SCENES, if enhanced { 0x41 } else { 0x01 }, &hb(gid, sid), &what, Tx::new())?;
    expect(p.len() >= 4, format!("View Scene Response payload {}", hex(&p)))?;
    let (status, g, sc) = (p[0], le16(&p, 1), p[3]);
    if status != SUCCESS {
        return Ok(ViewScene { status, g, sc, tr: None, sets: vec![], raw: p });
    }
    expect(p.len() >= 7, format!("View Scene Response SUCCESS without transition time and name: {}", hex(&p)))?;
    let tr = le16(&p, 4);
    let n = p[6] as usize;
    let mut i = 7 + n;
    let mut sets: Vec<(u16, Vec<u8>)> = Vec::new();
    while i + 3 <= p.len() {
        let (cl, ln) = (le16(&p, i), p[i + 2] as usize);
        let v = p[(i + 3).min(p.len())..(i + 3 + ln).min(p.len())].to_vec();
        match sets.iter_mut().find(|(c, _)| *c == cl) {
            Some(e) => e.1 = v,
            None => sets.push((cl, v)),
        }
        i += 3 + ln;
    }
    Ok(ViewScene { status, g, sc, tr: Some(tr), sets, raw: p })
}

fn store_scene(s: &mut Session, gid: u16, sid: u8) -> R<Vec<u8>> {
    rsp(s, SCENES, 0x04, &hb(gid, sid), &format!("Store Scene {gid:#06x}/{sid}"), Tx::new())
}

fn recall(s: &mut Session, gid: u16, sid: u8, transition: Option<u16>, wait: f64, o: Tx) -> R<Option<CapFrame>> {
    let mut pl = hb(gid, sid);
    if let Some(t) = transition {
        pl.extend_from_slice(&t.to_le_bytes());
    }
    let f = s.zcl(SCENES, 0x05, &pl, o)?;
    s.run(wait)?;
    Ok(f)
}

fn remove_scene(s: &mut Session, gid: u16, sid: u8) -> R<Vec<u8>> {
    rsp(s, SCENES, 0x02, &hb(gid, sid), &format!("Remove Scene {gid:#06x}/{sid}"), Tx::new())
}

/// (status, capacity, group, scenes or None, raw payload) from Get Scene Membership.
fn scene_membership(s: &mut Session, gid: u16) -> R<(u8, u8, u16, Option<Vec<u8>>, Vec<u8>)> {
    let p = rsp(s, SCENES, 0x06, &gid.to_le_bytes(), &format!("Get Scene Membership {gid:#06x}"), Tx::new())?;
    expect(p.len() >= 4, format!("Get Scene Membership Response payload {}", hex(&p)))?;
    let (status, cap, g) = (p[0], p[1], le16(&p, 2));
    if status != SUCCESS {
        return Ok((status, cap, g, None, p));
    }
    expect(
        p.len() >= 5 && p.len() == 5 + p[4] as usize,
        format!("Get Scene Membership Response SUCCESS payload {}", hex(&p)),
    )?;
    let sc = p[5..5 + p[4] as usize].to_vec();
    Ok((status, cap, g, Some(sc), p))
}

fn scene_attrs(s: &mut Session) -> R<HashMap<u16, V>> {
    let Some(r) = s.read(SCENES, &[0x0000, 0x0001, 0x0002, 0x0003], 1500.0)? else {
        return Err(Stop::Fail("no Read Attributes Response from the Scenes cluster".into()));
    };
    Ok(r.into_iter().map(|(a, _, _, v)| (a, v)).collect())
}

fn at(a: &HashMap<u16, V>, k: u16) -> R<V> {
    a.get(&k).cloned().ok_or_else(|| Stop::Error(format!("KeyError: {k}")))
}

fn count(s: &mut Session) -> R<i64> {
    let v = at(&scene_attrs(s)?, 0x0000)?;
    v.as_ref().and_then(|v| v.int()).ok_or_else(|| Stop::Error(format!("SceneCount {}", py_value(&v))))
}

fn scenes_setup(s: &mut Session) -> R<()> {
    s.need_cluster(SCENES)?;
    s.need_cluster(GROUPS)?;
    clean(s)?;
    need_group(s, G1)
}

fn scene_attributes(s: &mut Session) -> R<String> {
    s.need_cluster(SCENES)?;
    let want = [
        (0x0000u16, 0x20u8, "SceneCount"),
        (0x0001, 0x20, "CurrentScene"),
        (0x0002, 0x21, "CurrentGroup"),
        (0x0003, 0x10, "SceneValid"),
        (0x0004, 0x18, "NameSupport"),
    ];
    let mut r: Vec<(u16, u8, Option<u8>, V)> = Vec::new();
    for ids in [&[0x0000u16, 0x0001, 0x0002][..], &[0x0003, 0x0004][..]] {
        let x = s.read(SCENES, ids, 1500.0)?;
        expect(x.is_some(), "no Read Attributes Response from the Scenes cluster")?;
        r.extend(x.unwrap());
    }
    let mut bad = Vec::new();
    for (a, typ, name) in want {
        match r.iter().rev().find(|x| x.0 == a) {
            None => bad.push(format!("{name}: status {}", st(None))),
            Some((_, stt, _, _)) if *stt != SUCCESS => bad.push(format!("{name}: status {}", st(Some(*stt)))),
            Some((_, _, t, _)) if *t != Some(typ) => {
                bad.push(format!("{name}: type {:#04x}, {typ:#04x} required", t.unwrap_or(0)))
            }
            _ => {}
        }
    }
    expect(bad.is_empty(), bad.join("; "))?;
    Ok(String::new())
}

fn scene_add_errors(s: &mut Session) -> R<String> {
    scenes_setup(s)?;
    cleaned(s, |s| {
        let n0 = count(s)?;
        let p = add_scene1(s, G_ABSENT, 1, 0, &ext_onoff(true))?;
        expect(
            p[0] == INVALID_FIELD,
            format!("Add Scene for a foreign group: status {}; INVALID_FIELD (0x85) required", st(Some(p[0]))),
        )?;
        expect(p[1..] == hb(G_ABSENT, 1)[..], format!("Add Scene Response {} does not echo group and scene", hex(&p)))?;
        let p = add_scene1(s, 0xFFF8, 1, 0, &ext_onoff(true))?;
        expect(
            p[0] == INVALID_VALUE,
            format!("Add Scene for group 0xfff8: status {}; INVALID_VALUE (0x87) required", st(Some(p[0]))),
        )?;
        let n1 = count(s)?;
        expect(n1 == n0, format!("SceneCount went from {n0} to {n1} through two refused Add Scene commands"))?;
        Ok(String::new())
    })
}

fn scene_add_view(s: &mut Session) -> R<String> {
    scenes_setup(s)?;
    cleaned(s, |s| {
        let n0 = count(s)?;
        let p = add_scene1(s, G1, 3, 7, &cat(ext_onoff(true), ext_level(111)))?;
        expect(p == bhb(SUCCESS, G1, 3), format!("Add Scene Response {}", hex(&p)))?;
        let n1 = count(s)?;
        expect(n1 == n0 + 1, format!("SceneCount went from {n0} to {n1} after one Add Scene"))?;
        let v = view_scene(s, G1, 3, false)?;
        expect(v.status == SUCCESS && (v.g, v.sc) == (G1, 3), format!("View Scene Response {}", hex(&v.raw)))?;
        expect(v.tr == Some(7), format!("View Scene returns transition time {}, 7 was stored", v.tr_s()))?;
        let sets_txt = format!(
            "{{{}}}",
            v.sets.iter().map(|(k, b)| format!("'{k:#x}': '{}'", hex(b))).collect::<Vec<_>>().join(", ")
        );
        expect(
            v.get(ONOFF) == Some(&[1u8][..]) && v.get(LEVEL) == Some(&[111u8][..]),
            format!("View Scene returns extension field sets {sets_txt}; stored: On/Off 01, Level 6f"),
        )?;
        let p = add_scene1(s, G1, 3, 2, &cat(ext_onoff(true), ext_level(50)))?;
        expect(p[0] == SUCCESS, format!("second Add Scene for the same entry: status {}", st(Some(p[0]))))?;
        let n2 = count(s)?;
        expect(n2 == n1, format!("SceneCount went from {n1} to {n2} when an existing scene was overwritten"))?;
        let v = view_scene(s, G1, 3, false)?;
        expect(
            v.tr == Some(2) && v.get(LEVEL) == Some(&[50u8][..]),
            format!("after overwriting: View Scene Response {}", hex(&v.raw)),
        )?;
        Ok(String::new())
    })
}

fn scene_view_errors(s: &mut Session) -> R<String> {
    scenes_setup(s)?;
    cleaned(s, |s| {
        let v = view_scene(s, G1, 77, false)?;
        expect(
            v.status == NOT_FOUND,
            format!("View Scene for a missing scene: status {}; NOT_FOUND (0x8b) required", st(Some(v.status))),
        )?;
        expect(
            v.raw.len() == 4,
            format!("View Scene Response with an error status carries {} octets after the ids", v.raw.len() as i64 - 4),
        )?;
        let v = view_scene(s, G_ABSENT, 1, false)?;
        expect(
            v.status == INVALID_FIELD,
            format!("View Scene for a foreign group: status {}; INVALID_FIELD (0x85) required", st(Some(v.status))),
        )?;
        Ok(String::new())
    })
}

fn scene_store_recall(s: &mut Session) -> R<String> {
    scenes_setup(s)?;
    s.need_cluster(LEVEL)?;
    cleaned(s, |s| {
        set_on(s, true)?;
        set_level(s, 60)?;
        let (on_a, lvl_a, lit_a, duty_a) = state(s)?;
        if !is_int(&lvl_a, 60) || !lit_a {
            return Err(Stop::Inconclusive(format!(
                "the light could not be put at level 60: OnOff {}, level {}, lit {}",
                py_value(&on_a),
                py_value(&lvl_a),
                py_bool(lit_a)
            )));
        }
        let p = store_scene(s, G1, 2)?;
        expect(p == bhb(SUCCESS, G1, 2), format!("Store Scene Response {}", hex(&p)))?;
        let a = scene_attrs(s)?;
        let (sv, cs, cg) = (at(&a, 3)?, at(&a, 1)?, at(&a, 2)?);
        if !(is_int(&sv, 1) && is_int(&cs, 2) && is_int(&cg, G1 as i64)) {
            let m = format!(
                "after Store Scene: SceneValid {}, CurrentScene {}, CurrentGroup {}",
                py_value(&sv),
                py_value(&cs),
                hx6(&cg)?
            );
            return Err(Stop::Fail(m));
        }
        set_level(s, 200)?;
        let a = scene_attrs(s)?;
        expect(is_int(&at(&a, 3)?, 0), "SceneValid still TRUE after the level was changed")?;
        let (_, lvl_b, _, duty_b) = state(s)?;
        if !is_int(&lvl_b, 200) || duty_b == duty_a {
            return Err(Stop::Inconclusive(format!(
                "the light did not move away from the stored state (level {}, duty {} %)",
                py_value(&lvl_b),
                py_float(duty_b)
            )));
        }
        let f = recall(s, G1, 2, None, 700.0, Tx::new())?;
        expect(
            default_status(&f) == Some(SUCCESS),
            format!("answer to Recall Scene: {}; Default Response SUCCESS required", text(&f)),
        )?;
        let (on_c, lvl_c, lit_c, duty_c) = state(s)?;
        expect(
            (on_c.clone(), lvl_c.clone()) == (on_a.clone(), lvl_a.clone()),
            format!(
                "after Recall Scene: OnOff {}, CurrentLevel {}; stored were {} and {}",
                py_value(&on_c),
                py_value(&lvl_c),
                py_value(&on_a),
                py_value(&lvl_a)
            ),
        )?;
        expect(
            lit_c == lit_a && (duty_c - duty_a).abs() < 0.2,
            format!("light output after Recall Scene {} %, when stored {} %", py_float(duty_c), py_float(duty_a)),
        )?;
        let a = scene_attrs(s)?;
        let (sv, cs, cg) = (at(&a, 3)?, at(&a, 1)?, at(&a, 2)?);
        if !(is_int(&sv, 1) && is_int(&cs, 2) && is_int(&cg, G1 as i64)) {
            let m = format!(
                "after Recall Scene: SceneValid {}, CurrentScene {}, CurrentGroup {}",
                py_value(&sv),
                py_value(&cs),
                hx6(&cg)?
            );
            return Err(Stop::Fail(m));
        }
        Ok(format!(
            "level 60 = {} %, level 200 = {} %, recalled {} %",
            py_float(duty_a),
            py_float(duty_b),
            py_float(duty_c)
        ))
    })
}

fn scene_recall_added(s: &mut Session) -> R<String> {
    scenes_setup(s)?;
    let r = (|| {
        let p = add_scene1(s, G1, 4, 0, &ext_onoff(false))?;
        expect(p[0] == SUCCESS, format!("Add Scene: status {}", st(Some(p[0]))))?;
        set_on(s, true)?;
        let f = recall(s, G1, 99, None, 700.0, Tx::new())?;
        expect(s.light().on, "recalling a missing scene changed the light")?;
        expect(
            default_status(&f) == Some(NOT_FOUND),
            format!(
                "answer to Recall Scene for a missing scene: {}; Default Response NOT_FOUND (0x8b) required",
                text(&f)
            ),
        )?;
        let f = recall(s, G1, 4, None, 700.0, Tx::new().ddr().max_ms(800.0))?;
        let on = s.value(ONOFF, 0x0000)?;
        let lit = s.light().on;
        expect(
            is_int(&on, 0) && !lit,
            format!(
                "after recalling a scene with OnOff = 0: OnOff attribute {}, light on: {}",
                py_value(&on),
                py_bool(lit)
            ),
        )?;
        expect(
            f.is_none(),
            format!("a successful Recall Scene with Disable Default Response = 1 was answered: {}", text(&f)),
        )?;
        Ok(String::new())
    })();
    set_on(s, true)?;
    clean(s)?;
    r
}

fn scene_store_foreign(s: &mut Session) -> R<String> {
    scenes_setup(s)?;
    cleaned(s, |s| {
        let n0 = count(s)?;
        let p = store_scene(s, G_ABSENT, 1)?;
        expect(
            p[0] == INVALID_FIELD,
            format!("Store Scene for a foreign group: status {}; INVALID_FIELD (0x85) required", st(Some(p[0]))),
        )?;
        let n1 = count(s)?;
        expect(n1 == n0, format!("SceneCount went from {n0} to {n1}"))?;
        Ok(String::new())
    })
}

fn scene_remove(s: &mut Session) -> R<String> {
    scenes_setup(s)?;
    cleaned(s, |s| {
        let n0 = count(s)?;
        for sid in [1u8, 2, 3] {
            let p = add_scene1(s, G1, sid, 0, &ext_onoff(true))?;
            expect(p[0] == SUCCESS, format!("Add Scene {sid}: status {}", st(Some(p[0]))))?;
        }
        let p = remove_scene(s, G1, 2)?;
        expect(p == bhb(SUCCESS, G1, 2), format!("Remove Scene Response {}", hex(&p)))?;
        expect(count(s)? == n0 + 2, "SceneCount did not drop by one after Remove Scene")?;
        let p = remove_scene(s, G1, 2)?;
        expect(
            p[0] == NOT_FOUND,
            format!("Remove Scene for a scene already removed: status {}; NOT_FOUND required", st(Some(p[0]))),
        )?;
        let p = rsp(s, SCENES, 0x03, &G_ABSENT.to_le_bytes(), "Remove All Scenes (foreign group)", Tx::new())?;
        expect(
            p[0] == INVALID_FIELD,
            format!("Remove All Scenes for a foreign group: status {}; INVALID_FIELD required", st(Some(p[0]))),
        )?;
        let p = rsp(s, SCENES, 0x03, &G1.to_le_bytes(), "Remove All Scenes", Tx::new())?;
        expect(p == bh(SUCCESS, G1), format!("Remove All Scenes Response {}", hex(&p)))?;
        let n = count(s)?;
        expect(n == n0, format!("SceneCount is {n} after Remove All Scenes, {n0} before the scenes were added"))?;
        let v = view_scene(s, G1, 1, false)?;
        expect(
            v.status == NOT_FOUND,
            format!("scene 1 still viewable after Remove All Scenes: status {}", st(Some(v.status))),
        )?;
        Ok(String::new())
    })
}

fn scene_get_membership(s: &mut Session) -> R<String> {
    scenes_setup(s)?;
    cleaned(s, |s| {
        let (status, cap0, g, scenes, raw) = scene_membership(s, G1)?;
        expect(
            status == SUCCESS && g == G1 && scenes.as_deref() == Some(&[][..]),
            format!("empty group: response {}", hex(&raw)),
        )?;
        for sid in [5u8, 9] {
            add_scene1(s, G1, sid, 0, &ext_onoff(true))?;
        }
        let (status, cap, _, scenes, raw) = scene_membership(s, G1)?;
        let ok = status == SUCCESS && {
            let mut v = scenes.clone().unwrap_or_default();
            v.sort();
            v == [5, 9]
        };
        expect(ok, format!("two scenes added: response {}", hex(&raw)))?;
        if 0 < cap0 && cap0 < 0xFE {
            expect(
                cap as i64 == cap0 as i64 - 2,
                format!("Capacity went from {cap0} to {cap} after adding two scenes"),
            )?;
        }
        let (status, _, g, _, raw) = scene_membership(s, G_ABSENT)?;
        expect(
            status == INVALID_FIELD && g == G_ABSENT,
            format!("foreign group: response {}; INVALID_FIELD required", hex(&raw)),
        )?;
        expect(
            raw.len() == 4,
            format!(
                "foreign group: {} octets follow the group id; count and list must be omitted",
                raw.len() as i64 - 4
            ),
        )?;
        Ok(format!("capacity {cap0}"))
    })
}

fn scene_partial_fields(s: &mut Session) -> R<String> {
    scenes_setup(s)?;
    s.need_cluster(LEVEL)?;
    let r = (|| {
        let p = add_scene1(s, G1, 6, 0, &ext_onoff(true))?;
        expect(p[0] == SUCCESS, format!("Add Scene: status {}", st(Some(p[0]))))?;
        let v = view_scene(s, G1, 6, false)?;
        expect(v.status == SUCCESS, format!("View Scene: status {}", st(Some(v.status))))?;
        set_on(s, true)?;
        set_level(s, 90)?;
        set_on(s, false)?;
        recall(s, G1, 6, None, 700.0, Tx::new().ddr().max_ms(400.0))?;
        let (on, lvl, lit, _) = state(s)?;
        expect(
            is_int(&on, 1) && lit,
            format!("Recall Scene (OnOff = 1 only): OnOff attribute {}, light on: {}", py_value(&on), py_bool(lit)),
        )?;
        expect(
            is_int(&lvl, 90),
            format!(
                "CurrentLevel is {} after recalling a scene without a Level field set; it was 90 and must remain \
                 unchanged",
                py_value(&lvl)
            ),
        )?;
        let keys: Vec<u16> = v.sets.iter().map(|(k, _)| *k).collect();
        expect(
            keys == [ONOFF],
            format!(
                "View Scene returns field sets for {}; the scene was added with an On/Off set only",
                py_hex_list(&keys)
            ),
        )?;
        Ok(String::new())
    })();
    set_on(s, true)?;
    clean(s)?;
    r
}

fn scene_store_keeps_transition(s: &mut Session) -> R<String> {
    scenes_setup(s)?;
    s.need_cluster(LEVEL)?;
    cleaned(s, |s| {
        let p = add_scene1(s, G1, 8, 9, &cat(ext_onoff(true), ext_level(20)))?;
        expect(p[0] == SUCCESS, format!("Add Scene: status {}", st(Some(p[0]))))?;
        set_on(s, true)?;
        set_level(s, 150)?;
        let p = store_scene(s, G1, 8)?;
        expect(p[0] == SUCCESS, format!("Store Scene: status {}", st(Some(p[0]))))?;
        let v = view_scene(s, G1, 8, false)?;
        expect(v.status == SUCCESS, format!("View Scene: status {}", st(Some(v.status))))?;
        let lv = v.get(LEVEL).unwrap_or(&[]);
        expect(
            lv.get(..1) == Some(&[150u8][..]),
            format!("stored level field {}; the current level 150 (0x96) must replace the old 20", hex(lv)),
        )?;
        expect(
            v.tr == Some(9),
            format!("transition time {} after Store Scene over an entry added with 9 s", v.tr_s()),
        )?;
        Ok(String::new())
    })
}

fn duty(s: &Session) -> f64 {
    let li = s.light();
    if li.on {
        round1(li.duty)
    } else {
        0.0
    }
}

fn scene_recall_transition(s: &mut Session) -> R<String> {
    scenes_setup(s)?;
    s.need_cluster(LEVEL)?;
    cleaned(s, |s| {
        set_on(s, true)?;
        set_level(s, 254)?;
        let d_hi = duty(s);
        let p = add_scene1(s, G1, 10, 4, &cat(ext_onoff(true), ext_level(20)))?;
        expect(p[0] == SUCCESS, format!("Add Scene: status {}", st(Some(p[0]))))?;
        // reference: where does the scene end up (no transition)?
        recall(s, G1, 10, Some(0), 1500.0, Tx::new().ddr().max_ms(300.0))?;
        let d_lo = duty(s);
        if (d_hi - d_lo).abs() < 5.0 {
            return Err(Stop::Inconclusive(format!(
                "levels 254 and 20 give nearly the same output ({} % / {} %)",
                py_float(d_hi),
                py_float(d_lo)
            )));
        }
        let mut obs: Vec<String> = Vec::new();
        for (what, tr, total) in [
            ("Recall Scene with Transition Time 40 (4 s)", Some(40u16), 4000.0),
            ("Recall Scene without the field, scene transition time 4 s", None, 4000.0),
        ] {
            set_level(s, 254)?;
            recall(s, G1, 10, tr, 0.0, Tx::new().ddr().max_ms(300.0))?;
            s.run(900.0)?;
            let d_early = duty(s);
            s.run(total + 1500.0 - 900.0 - 300.0)?;
            let d_end = duty(s);
            obs.push(format!(
                "{what}: {} % after 1.2 s, {} % after {:.1} s",
                py_float(d_early),
                py_float(d_end),
                (total + 1500.0) / 1000.0
            ));
            let last = obs.last().unwrap().clone();
            expect((d_end - d_lo).abs() < 0.3, format!("{last}; the scene's output is {} %", py_float(d_lo)))?;
            expect(
                (d_early - d_lo).abs() > 0.3,
                format!("{last}; the target ({} %) was reached within 1.2 s of a 4 s transition", py_float(d_lo)),
            )?;
        }
        Ok(obs.join("; ") + &format!(" (start {} %, target {} %)", py_float(d_hi), py_float(d_lo)))
    })
}

fn scene_removed_with_groups(s: &mut Session) -> R<String> {
    scenes_setup(s)?;
    cleaned(s, |s| {
        let n0 = count(s)?;
        add_scene1(s, G1, 1, 0, &ext_onoff(true))?;
        add_scene1(s, G1, 2, 0, &ext_onoff(true))?;
        expect(count(s)? == n0 + 2, "the two scenes were not stored")?;
        s.zcl(GROUPS, 0x04, &[], Tx::new())?;
        s.run(300.0)?;
        let n = count(s)?;
        expect(
            n == n0,
            format!("SceneCount is {n} after Remove All Groups; {n0} before the group's two scenes were added"),
        )?;
        Ok(String::new())
    })
}

fn scene_membership_broadcast(s: &mut Session) -> R<String> {
    scenes_setup(s)?;
    cleaned(s, |s| {
        let f = s.zcl(SCENES, 0x06, &G1.to_le_bytes(), Tx::new().dst(0xFFFD).max_ms(1200.0))?;
        expect(
            f.is_none(),
            format!("a broadcast Get Scene Membership for a group without scenes was answered: {}", text(&f)),
        )?;
        add_scene1(s, G1, 5, 0, &ext_onoff(true))?;
        let f = ask_broadcast(s, SCENES, 0x06, &G1.to_le_bytes(), Tx::new().dst(0xFFFD).max_ms(1500.0))?;
        expect(
            f.is_some(),
            "no answer to a broadcast Get Scene Membership for a group that has a scene (sent three times)",
        )?;
        let fr = f.as_ref().unwrap();
        let z = fr.zcl().unwrap();
        expect(
            z.specific && z.cmd == 0x06 && fr.nwk.as_ref().unwrap().dst == 0x0000,
            format!("answer: {}", text(&f)),
        )?;
        let p = z.payload;
        let ok = p.first() == Some(&SUCCESS) && p.len() >= 5 && {
            let e = (5 + p[4] as usize).min(p.len());
            p[5..e] == [5]
        };
        expect(ok, format!("response {}", hex(&p)))?;
        Ok(String::new())
    })
}

fn scene_group_zero(s: &mut Session) -> R<String> {
    s.need_cluster(SCENES)?;
    clean(s)?;
    let r = (|| {
        let p = add_scene1(s, 0x0000, 21, 0, &ext_onoff(false))?;
        expect(p == bhb(SUCCESS, 0x0000, 21), format!("Add Scene for group 0x0000: response {}", hex(&p)))?;
        set_on(s, true)?;
        let f = recall(s, 0x0000, 21, None, 700.0, Tx::new())?;
        let lit = s.light().on;
        expect(
            default_status(&f) == Some(SUCCESS) && !lit,
            format!("Recall Scene 0x0000/21 (OnOff = 0): {}, light on: {}", text(&f), py_bool(lit)),
        )?;
        let a = scene_attrs(s)?;
        let (cg, cs, sv) = (at(&a, 2)?, at(&a, 1)?, at(&a, 3)?);
        if !(is_int(&cg, 0) && is_int(&cs, 21) && is_int(&sv, 1)) {
            let m = format!("CurrentGroup {}, CurrentScene {}, SceneValid {}", hx6(&cg)?, py_value(&cs), py_value(&sv));
            return Err(Stop::Fail(m));
        }
        Ok(String::new())
    })();
    set_on(s, true)?;
    clean(s)?;
    r
}

fn scene_enhanced(s: &mut Session) -> R<String> {
    scenes_setup(s)?;
    cleaned(s, |s| {
        let mut pl = hb(G1, 12);
        pl.extend_from_slice(&25u16.to_le_bytes());
        pl.push(0);
        pl.extend(ext_onoff(true));
        let f = s.zcl(SCENES, 0x40, &pl, Tx::new())?;
        expect(f.is_some(), "no response to Enhanced Add Scene")?;
        let z = f.as_ref().unwrap().zcl().unwrap();
        expect(z.specific && z.cmd == 0x40, format!("answer to Enhanced Add Scene: {}", text(&f)))?;
        expect(z.payload == bhb(SUCCESS, G1, 12), format!("Enhanced Add Scene Response {}", hex(&z.payload)))?;
        let v = view_scene(s, G1, 12, true)?;
        expect(
            v.status == SUCCESS && v.tr == Some(25),
            format!(
                "Enhanced View Scene: status {}, transition time {} (25 tenths were stored)",
                st(Some(v.status)),
                v.tr_s()
            ),
        )?;
        let v = view_scene(s, G1, 12, false)?;
        expect(
            v.status == SUCCESS && v.tr == Some(2),
            format!(
                "View Scene: status {}, transition time {} s (2 whole seconds of 2.5 s)",
                st(Some(v.status)),
                v.tr_s()
            ),
        )?;
        Ok(String::new())
    })
}

fn scene_copy(s: &mut Session) -> R<String> {
    scenes_setup(s)?;
    cleaned(s, |s| {
        need_group(s, G2)?;
        let p = add_scene1(s, G1, 13, 3, &cat(ext_onoff(true), ext_level(77)))?;
        expect(p[0] == SUCCESS, format!("Add Scene: status {}", st(Some(p[0]))))?;
        let mut pl = bh(0x00, G1);
        pl.push(13);
        pl.extend_from_slice(&G2.to_le_bytes());
        pl.push(14);
        let f = s.zcl(SCENES, 0x42, &pl, Tx::new())?;
        expect(f.is_some(), "no response to Copy Scene")?;
        let z = f.as_ref().unwrap().zcl().unwrap();
        expect(z.specific && z.cmd == 0x42, format!("answer to Copy Scene: {}", text(&f)))?;
        expect(
            z.payload == bhb(SUCCESS, G1, 13),
            format!(
                "Copy Scene Response {}; required: SUCCESS and the group/scene copied from",
                hex(&z.payload)
            ),
        )?;
        let v = view_scene(s, G2, 14, false)?;
        expect(v.status == SUCCESS, format!("the copy cannot be viewed: status {}", st(Some(v.status))))?;
        expect(
            v.tr == Some(3) && v.get(ONOFF) == Some(&[1u8][..]) && v.get(LEVEL) == Some(&[77u8][..]),
            format!("the copy differs from the original: View Scene Response {}", hex(&v.raw)),
        )?;
        Ok(String::new())
    })
}

// ------------------------------------------------------------------ registry
macro_rules! wrapped {
    ($($w:ident => $f:ident),* $(,)?) => {
        $(fn $w(s: &mut Session) -> R<String> { retry(s, $f) })*
    };
}

wrapped!(
    t_group_add => group_add,
    t_group_add_invalid => group_add_invalid,
    t_group_add_duplicate => group_add_duplicate,
    t_group_view_errors => group_view_errors,
    t_group_membership => group_membership,
    t_group_remove => group_remove,
    t_group_remove_all => group_remove_all,
    t_group_add_if_identifying => group_add_if_identifying,
    t_group_names => group_names,
    t_groupcast => groupcast,
    t_group_cmds_broadcast => group_cmds_broadcast,
    t_group_table_full => group_table_full,
    t_group_membership_broadcast => group_membership_broadcast,
    t_scene_attributes => scene_attributes,
    t_scene_add_errors => scene_add_errors,
    t_scene_add_view => scene_add_view,
    t_scene_view_errors => scene_view_errors,
    t_scene_store_recall => scene_store_recall,
    t_scene_recall_added => scene_recall_added,
    t_scene_store_foreign => scene_store_foreign,
    t_scene_remove => scene_remove,
    t_scene_get_membership => scene_get_membership,
    t_scene_partial_fields => scene_partial_fields,
    t_scene_store_keeps_transition => scene_store_keeps_transition,
    t_scene_recall_transition => scene_recall_transition,
    t_scene_removed_with_groups => scene_removed_with_groups,
    t_scene_membership_broadcast => scene_membership_broadcast,
    t_scene_group_zero => scene_group_zero,
    t_scene_enhanced => scene_enhanced,
    t_scene_copy => scene_copy,
);

const fn t(id: &'static str, doc: &'static str, clause: &'static str, req: &'static str,
           run: fn(&mut Session) -> R<String>) -> Test {
    Test { id, doc, clause, req, session: S, run, ..Test::D }
}

pub fn tests() -> Vec<Test> {
    vec![
        t("ZCL-GROUP-01", "ZCL8", "3.6.2.3.2.2, 3.6.2.3.3.2, 3.6.2.3.4.2",
          "A unicast Add Group for a valid new group is answered by Add Group Response SUCCESS with the group id; the \
           group is then reported by View Group (SUCCESS) and by Get Group Membership with an empty list.",
          t_group_add),
        t("ZCL-GROUP-02", "ZCL8", "3.6.2.3.2.2 step 1",
          "Add Group with a group id outside 0x0001-0xfff7 is answered with INVALID_VALUE and adds nothing.",
          t_group_add_invalid),
        t("ZCL-GROUP-03", "ZCL8", "3.6.2.3.2.2 step 2, Table 2-12",
          "Add Group for a group the endpoint is already in is answered with SUCCESS (ZCL8 retired DUPLICATE_EXISTS \
           0x8a) and the group is listed once.",
          t_group_add_duplicate),
        t("ZCL-GROUP-04", "ZCL8", "3.6.2.3.3.2",
          "View Group for a group the endpoint is not in is answered with NOT_FOUND, for an id outside 0x0001-0xfff7 \
           with INVALID_VALUE; the response carries the group id asked for.",
          t_group_view_errors),
        t("ZCL-GROUP-05", "ZCL8", "3.6.2.3.4.2, 3.6.2.4.3",
          "Get Group Membership with a group list returns exactly the listed groups the endpoint is in; a unicast \
           request matching nothing is answered with a group count of zero; Capacity is the remaining room in the \
           group table.",
          t_group_membership),
        t("ZCL-GROUP-06", "ZCL8", "3.6.2.3.5.2",
          "Remove Group removes the membership and is answered with SUCCESS; for a group the endpoint is not in the \
           status is NOT_FOUND, for an id out of range INVALID_VALUE.",
          t_group_remove),
        t("ZCL-GROUP-07", "ZCL8", "3.6.2.3.6.2",
          "Remove All Groups removes every group of the endpoint; a unicast request that asks for it gets a Default \
           Response SUCCESS.",
          t_group_remove_all),
        t("ZCL-GROUP-08", "ZCL8", "3.6.2.3.7.2",
          "Add Group If Identifying adds the group only while the device is identifying; a unicast request that asks \
           for it gets a Default Response SUCCESS in both cases.",
          t_group_add_if_identifying),
        t("ZCL-GROUP-09", "ZCL8", "3.6.2.2.1, 3.6.2.4.2.2",
          "NameSupport is a map8 whose top bit says whether group names are stored; View Group returns the stored \
           name when they are and the empty string when they are not.",
          t_group_names),
        t("ZCL-GROUP-10", "R22", "2.2.8.4.2; ZCL8 2.5.12.2",
          "A command sent to a group is executed by an endpoint that is a member and ignored by one that is not; in \
           neither case is a Default Response sent.",
          t_groupcast),
        t("ZCL-GROUP-11", "ZCL8", "3.6.2.3.1",
          "Add Group, View Group and Remove Group received by groupcast or broadcast are processed but never \
           answered.",
          t_group_cmds_broadcast),
        t("ZCL-GROUP-12", "ZCL8", "3.6.2.3.2.2 step 3",
          "Add Group when the group table has no free entry is answered with INSUFFICIENT_SPACE.",
          t_group_table_full),
        t("ZCL-GROUP-13", "ZCL8", "3.6.2.3.4.2",
          "Get Group Membership received by broadcast is answered when its list is empty or names a group of the \
           endpoint, and is not answered when the list names only foreign groups.",
          t_group_membership_broadcast),
        t("ZCL-SCENE-01", "ZCL8", "3.7.2.2.1 Table 3-40",
          "The Scenes server has SceneCount (uint8), CurrentScene (uint8), CurrentGroup (uint16), SceneValid (bool) \
           and NameSupport (map8).",
          t_scene_attributes),
        t("ZCL-SCENE-02", "ZCL8", "3.7.2.4.2.2 steps 1, 2",
          "Add Scene for a group the endpoint is not in is answered with INVALID_FIELD, for a group id above 0xfff7 \
           with INVALID_VALUE; nothing is stored.",
          t_scene_add_errors),
        t("ZCL-SCENE-03", "ZCL8", "3.7.2.4.2.2, 3.7.2.5.1, 3.7.2.5.2, 3.7.2.2.1.1",
          "Add Scene stores the scene and is answered with SUCCESS, group and scene; SceneCount counts it; View Scene \
           returns the stored transition time and extension field sets; adding the same group/scene again \
           overwrites the entry.",
          t_scene_add_view),
        t("ZCL-SCENE-04", "ZCL8", "3.7.2.4.3.2, 3.7.2.5.2.2",
          "View Scene for a scene that does not exist is answered with NOT_FOUND, for a group the endpoint is not in \
           with INVALID_FIELD; transition time, name and field sets are then omitted.",
          t_scene_view_errors),
        t("ZCL-SCENE-05", "ZCL8", "3.7.2.4.6.2, 3.7.2.4.7.2, 3.7.2.2.1.4; 3.8.2.6, 3.10.2.5",
          "Store Scene records the current On/Off and level and is answered with SUCCESS; Recall Scene brings \
           attributes and light output back to them; SceneValid is TRUE after store and recall and FALSE once the \
           state is changed; CurrentScene and CurrentGroup name the scene.",
          t_scene_store_recall),
        t("ZCL-SCENE-06", "ZCL8", "3.7.2.4.7.2 steps 3-5",
          "Recall Scene applies the extension fields of an added scene (a scene with OnOff = 0 turns the light off); \
           recalling a scene that does not exist changes nothing and is answered by a Default Response NOT_FOUND.",
          t_scene_recall_added),
        t("ZCL-SCENE-07", "ZCL8", "3.7.2.4.6.2 step 2, 3.7.2.5.5.2",
          "Store Scene for a group the endpoint is not in is answered with INVALID_FIELD and stores nothing.",
          t_scene_store_foreign),
        t("ZCL-SCENE-08", "ZCL8", "3.7.2.4.4.2, 3.7.2.4.5.2, 3.7.2.5.3, 3.7.2.5.4",
          "Remove Scene removes one scene (SUCCESS; NOT_FOUND when it is not there); Remove All Scenes removes all \
           scenes of the group and is answered with status and group (INVALID_FIELD for a group the endpoint is not \
           in).",
          t_scene_remove),
        t("ZCL-SCENE-09", "ZCL8", "3.7.2.4.8.2, 3.7.2.5.6",
          "A unicast Get Scene Membership is answered with status, capacity, group and the list of the group's \
           scenes; for a group the endpoint is not in the status is INVALID_FIELD and scene count and list are \
           omitted.",
          t_scene_get_membership),
        t("ZCL-SCENE-10", "ZCL8", "3.7.2.4.7.2 step 4, 3.7.2.5.2.2",
          "A scene holds exactly the extension field sets it was given: View Scene returns only those, and Recall \
           Scene leaves a cluster without a field set unchanged.",
          t_scene_partial_fields),
        t("ZCL-SCENE-11", "ZCL8", "3.7.2.4.6.2 step 4",
          "Store Scene over an existing entry keeps that entry's transition time (and name) and replaces only the \
           stored state.",
          t_scene_store_keeps_transition),
        Test {
            mandatory: false,
            ambiguous: Some(
                "the text says the transition time determines how long the transition takes, recommends a gradual \
                 change and then leaves 'the exact transition' to the manufacturer; the test fails an image whose \
                 output is already at the target 1.2 s into a 4 s transition",
            ),
            ..t("ZCL-SCENE-12", "ZCL8", "3.7.2.4.7.2",
                "The transition to a recalled scene takes the time given in the Recall Scene command (tenths of a \
                 second) or, without that field, the scene's own transition time; a gradual change is recommended.",
                t_scene_recall_transition)
        },
        Test {
            mandatory: false,
            ambiguous: Some(
                "stated only as a note to Remove All Groups ('necessitates the removal of all associated scenes'); \
                 for Remove Group the text says SHOULD",
            ),
            ..t("ZCL-SCENE-13", "ZCL8", "3.6.2.3.6", "Removing all groups removes the scenes associated with them.",
                t_scene_removed_with_groups)
        },
        t("ZCL-SCENE-14", "ZCL8", "3.7.2.4.8.2 step 3",
          "Get Scene Membership received by broadcast is answered only when the scene table has an entry for the \
           group.",
          t_scene_membership_broadcast),
        t("ZCL-SCENE-15", "ZCL8", "3.7.2.4.2.2 step 2, 3.7.2.3",
          "A scene may be stored with group id 0x0000 (not associated with a group) without any group membership, \
           and recalled.",
          t_scene_group_zero),
        t("ZCL-SCENE-16", "ZLO", "7.2.2 Table 6; ZCL8 3.7.2.4.9.1, 3.7.2.5.8",
          "A dimmable light supports Enhanced Add Scene and Enhanced View Scene: the transition time is given and \
           returned in tenths of a second, and View Scene shows its whole seconds.",
          t_scene_enhanced),
        t("ZCL-SCENE-17", "ZLO", "7.2.2 Table 6; ZCL8 3.7.2.4.11.6, 3.7.2.5.9",
          "A dimmable light supports Copy Scene: the entry is copied to the new group/scene pair and the response \
           carries SUCCESS and the pair copied from.",
          t_scene_copy),
    ]
}
