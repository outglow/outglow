//! Basic and Identify clusters of a light (ZCL rev 8 clauses 3.2 and 3.5; ZLO
//! device specification 15-0014-05 clause 7.2 for what a dimmable light must
//! have; BDB 3.0.1 clauses 8.4/8.5 for the finding & binding target). Ported
//! from emulator/spec/zcl_basic_identify.py.

use super::zcl_foundation::{fin, int, opt_str, py_float, py_floats, py_hex, py_round, py_strs, records, retry,
                            sorted_set, type_error, Records};
use super::Test;
use crate::frames::{py_value, ZclValue};
use crate::session::{expect, CapFrame, CmdAnswer, Session, Stop, R};
use crate::station::Tx;

const S: &str = "zcl-basic";

const SUCCESS: u8 = 0x00;
const UNSUP_COMMAND: u8 = 0x81;
const BASIC: u16 = 0x0000;
const IDENTIFY: u16 = 0x0003;
const GROUPS: u16 = 0x0004;
const U8: u8 = 0x20;
const U16: u8 = 0x21;
const ENUM8: u8 = 0x30;
const STRING: u8 = 0x42;
const OCTSTR: u8 = 0x41;

macro_rules! wrapped {
    ($($w:ident => $f:ident),* $(,)?) => {
        $(fn $w(s: &mut Session) -> R<String> { retry(s, $f) })*
    };
}

wrapped!(
    w_basic_mandatory => basic_mandatory, w_basic_zcl_version => basic_zcl_version, w_basic_zlo => basic_zlo,
    w_basic_zlo_extra => basic_zlo_extra, w_basic_reset => basic_reset, w_identify_time_attr => identify_time_attr,
    w_identify_cmd => identify_cmd, w_identify_query => identify_query, w_identify_query_idle => identify_query_idle,
    w_identify_query_broadcast => identify_query_broadcast, w_identify_ends => identify_ends,
    w_identify_visible => identify_visible, w_trigger_effect => trigger_effect,
);

pub fn tests() -> Vec<Test> {
    vec![
        Test { id: "ZCL-BASIC-01", doc: "ZCL8", clause: "3.2.2.2 Table 3-7, 3.2.2.2.8",
               req: "The Basic server has the mandatory attributes ZCLVersion (uint8) and PowerSource (enum8, one of \
                     the non-reserved values of Table 3-8).",
               session: S, run: w_basic_mandatory, ..Test::D },
        Test { id: "ZCL-BASIC-02", doc: "ZCL8", clause: "3.2.2.2.1",
               req: "ZCLVersion is 8 for a device built to this revision of the ZCL.",
               session: S, run: w_basic_zcl_version, mandatory: false,
               ambiguous: Some("the attribute states which ZCL revision the device was built to; a device built and \
                                certified to ZCL6/ZCL7 correctly reports 3. A FAIL here means only 'does not claim \
                                ZCL8' and explains the retired status codes seen in other tests"),
               ..Test::D },
        Test { id: "ZCL-BASIC-03", doc: "ZLO", clause: "7.2.1 Table 5; ZCL8 3.2.2.2 Table 3-7",
               req: "A dimmable light supports the Basic attributes ApplicationVersion, StackVersion, HWVersion, \
                     ManufacturerName, ModelIdentifier, DateCode and SWBuildID, with the types and string lengths of \
                     the ZCL.",
               session: S, run: w_basic_zlo,
               ambiguous: Some("ZLO 7.2.1 says a dimmable light SHALL support the attributes of Table 5, but that \
                                table also lists four attributes that ZLO 27.2.1.1 itself calls optional, and ZCL8 \
                                Table 3-7 marks all seven tested here optional; whether Table 5 overrides the \
                                cluster's M/O column is not clear from the text"),
               ..Test::D },
        Test { id: "ZCL-BASIC-04", doc: "ZLO", clause: "7.2.1 Table 5, 27.2.1.1",
               req: "A dimmable light supports the Basic attributes GenericDeviceClass, GenericDeviceType, ProductCode \
                     and ProductURL; for the lighting class (0x00) the type is a non-reserved value of Table 68.",
               session: S, run: w_basic_zlo_extra, mandatory: false,
               ambiguous: Some("ZLO Table 5 lists these four among the attributes a dimmable light SHALL support, \
                                while ZLO Table 66 (27.2.1.1) and ZCL8 Table 3-7 mark them optional"),
               ..Test::D },
        Test { id: "ZCL-BASIC-05", doc: "ZCL8", clause: "3.2.2.3.1.1",
               req: "Reset to Factory Defaults sets the attributes of all clusters back to their defaults \
                     (IdentifyTime: 0) and leaves network membership and groups untouched.",
               session: S, run: w_basic_reset, mandatory: false, ..Test::D },
        Test { id: "ZCL-IDENT-01", doc: "ZCL8", clause: "3.5.2.2 Table 3-31, 3.5.2.2.1",
               req: "IdentifyTime is a writable uint16 that is decremented every second while it is not zero.",
               session: S, run: w_identify_time_attr, ..Test::D },
        Test { id: "ZCL-IDENT-02", doc: "ZCL8", clause: "3.5.2.3.1.2",
               req: "The Identify command sets IdentifyTime to its Identify Time field; a value of 0 stops identifying.",
               session: S, run: w_identify_cmd, ..Test::D },
        Test { id: "ZCL-IDENT-03", doc: "ZCL8", clause: "3.5.2.3.2.1, 3.5.2.4.1",
               req: "While identifying, the device answers a unicast Identify Query with an Identify Query Response \
                     sent to the requester, whose Timeout is the current IdentifyTime.",
               session: S, run: w_identify_query, ..Test::D },
        Test { id: "ZCL-IDENT-04", doc: "ZCL8", clause: "3.5.2.3.2.1",
               req: "A device that is not identifying takes no action on an Identify Query: no Identify Query \
                     Response is sent.",
               session: S, run: w_identify_query_idle, ..Test::D },
        Test { id: "ZCL-IDENT-05", doc: "BDB 3.0.1", clause: "8.4 step 4, 8.5 step 3; ZCL8 3.5.2.3.2; ZLO 7.1",
               req: "A finding & binding target answers the initiator's Identify Query, which is broadcast to 0xffff, \
                     with a unicast Identify Query Response while it is identifying, and stays silent when it is not.",
               session: S, run: w_identify_query_broadcast, ..Test::D },
        Test { id: "ZCL-IDENT-06", doc: "ZCL8", clause: "3.5.2.2.1",
               req: "When IdentifyTime reaches zero the device stops identifying: the attribute stays 0 and Identify \
                     Query is no longer answered.",
               session: S, run: w_identify_ends, ..Test::D },
        Test { id: "ZCL-IDENT-07", doc: "ZCL8", clause: "3.5.2.2.1",
               req: "While IdentifyTime is not zero the device runs an identification procedure that shows an \
                     observer which device it is; a lamp has only its light for this, so the light output changes \
                     during identification (flashing is recommended) and is steady again afterwards.",
               session: S, run: w_identify_visible,
               ambiguous: Some("the text requires an identification procedure that indicates the device to an \
                                observer and only recommends flashing; the test assumes the light output is the \
                                indicator"),
               ..Test::D },
        Test { id: "ZCL-IDENT-08", doc: "ZLO", clause: "7.2.2 Table 6; ZCL8 3.5.2.3.3.3",
               req: "A dimmable light supports the Trigger Effect command: blink with the default variant is accepted \
                     (Default Response SUCCESS, not UNSUP_COMMAND).",
               session: S, run: w_trigger_effect, ..Test::D },
    ]
}

// ------------------------------------------------------------------ helpers
fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// `{id: (status, type, value)}` read two at a time (strings may not fit one frame together).
fn read_all(s: &mut Session, cluster: u16, attrs: &[u16]) -> R<Records> {
    let mut out = Records::new();
    for pair in attrs.chunks(2) {
        match s.read(cluster, pair, 1500.0)? {
            Some(r) => out.extend(records(r)),
            None => {
                let h: Vec<String> = pair.iter().map(|a| py_hex(*a as u64)).collect();
                return Err(Stop::Fail(format!(
                    "no Read Attributes Response for {} of cluster {cluster:#06x}",
                    py_strs(&h)
                )));
            }
        }
    }
    Ok(out)
}

fn identify_time(s: &mut Session) -> R<i64> {
    match int(&s.value(IDENTIFY, 0x0000)?) {
        Some(v) => Ok(v),
        None => Err(Stop::Inconclusive("IdentifyTime could not be read".into())),
    }
}

fn identify(s: &mut Session, seconds: u16) -> R<CmdAnswer> {
    s.cmd(IDENTIFY, 0x00, &seconds.to_le_bytes())
}

fn stop_identify(s: &mut Session) -> R<()> {
    s.zcl(IDENTIFY, 0x00, &0u16.to_le_bytes(), Tx::new().ddr().max_ms(300.0))?;
    s.run(300.0)
}

/// `st if st is None else hex(st)`; a frame cannot be formatted (TypeError in Python).
fn status_text(a: &CmdAnswer) -> R<String> {
    match a {
        CmdAnswer::None => Ok("None".into()),
        CmdAnswer::Status(x) => Ok(py_hex(*x as u64)),
        CmdAnswer::Frame(_) => Err(type_error("'Frame' object cannot be interpreted as an integer")),
    }
}

fn is_status(a: &CmdAnswer, v: u8) -> bool {
    matches!(a, CmdAnswer::Status(x) if *x == v)
}

/// A broadcast or groupcast request for which an answer is expected, repeated
/// before 'no answer' is concluded (a single copy can be lost on the air).
fn ask_broadcast(s: &mut Session, cluster: u16, cmd: u8, payload: &[u8], tries: usize, o: Tx) -> R<Option<CapFrame>> {
    let mut f = None;
    for _ in 0..tries {
        f = s.zcl(cluster, cmd, payload, o.clone())?;
        if f.is_some() {
            break;
        }
        s.run(300.0)?;
    }
    Ok(f)
}

fn is_query_rsp(f: &Option<CapFrame>) -> bool {
    f.as_ref().and_then(|f| f.zcl()).is_some_and(|z| z.specific && z.cmd == 0x00 && z.to_client)
}

fn text(f: &Option<CapFrame>) -> String {
    let Some(f) = f else { return "no response".into() };
    let z = f.zcl().unwrap();
    format!(
        "{} command {:#04x} payload {} to {:#06x}",
        if z.specific { "cluster" } else { "general" },
        z.cmd,
        hex(&z.payload),
        f.nwk.as_ref().unwrap().dst
    )
}

fn get(r: &Records, a: u16) -> (Option<u8>, Option<u8>, Option<ZclValue>) {
    match r.get(&a) {
        Some((st, t, v)) => (Some(*st), *t, v.clone()),
        None => (None, None, None),
    }
}

// ------------------------------------------------------------------ Basic
fn basic_mandatory(s: &mut Session) -> R<String> {
    s.need_cluster(BASIC)?;
    let r = read_all(s, BASIC, &[0x0000, 0x0007])?;
    let (stt, t, v) = get(&r, 0x0000);
    expect(
        stt == Some(SUCCESS) && t == Some(U8),
        format!("ZCLVersion: status {}, type {}", opt_str(stt), opt_str(t)),
    )?;
    let zv = v;
    let (stt, t, v) = get(&r, 0x0007);
    expect(
        stt == Some(SUCCESS) && t == Some(ENUM8),
        format!("PowerSource: status {}, type {}", opt_str(stt), opt_str(t)),
    )?;
    let Some(v) = int(&v) else { return Err(type_error("unsupported operand type(s) for &")) };
    expect(
        v & 0x7F <= 0x06,
        format!("PowerSource {v:#04x} is a reserved value (0x00-0x06, bit 7 = battery backup)"),
    )?;
    Ok(format!("ZCLVersion {}, PowerSource {v:#04x}", py_value(&zv)))
}

fn basic_zcl_version(s: &mut Session) -> R<String> {
    s.need_cluster(BASIC)?;
    let v = s.value(BASIC, 0x0000)?;
    expect(int(&v) == Some(8), format!("ZCLVersion is {}; ZCL8 says 8", py_value(&v)))?;
    Ok(String::new())
}

const ZLO_BASIC: [(u16, u8, &str, Option<usize>); 7] = [
    (0x0001, U8, "ApplicationVersion", None),
    (0x0002, U8, "StackVersion", None),
    (0x0003, U8, "HWVersion", None),
    (0x0004, STRING, "ManufacturerName", Some(32)),
    (0x0005, STRING, "ModelIdentifier", Some(32)),
    (0x0006, STRING, "DateCode", Some(16)),
    (0x4000, STRING, "SWBuildID", Some(16)),
];

fn bad_record(name: &str, a: u16, stt: Option<u8>, t: Option<u8>, typ: u8) -> Option<String> {
    if stt != Some(SUCCESS) {
        return Some(match stt {
            Some(x) => format!("{name} ({a:#06x}): status {x:#04x}"),
            None => format!("{name}: no record"),
        });
    }
    if t != Some(typ) {
        return Some(format!("{name}: type {:#04x}, {typ:#04x} required", t.unwrap_or(0)));
    }
    None
}

fn basic_zlo(s: &mut Session) -> R<String> {
    s.need_cluster(BASIC)?;
    let ids: Vec<u16> = ZLO_BASIC.iter().map(|x| x.0).collect();
    let r = read_all(s, BASIC, &ids)?;
    let mut bad: Vec<String> = Vec::new();
    let mut vals: Vec<String> = Vec::new();
    for (a, typ, name, maxlen) in ZLO_BASIC {
        let (stt, t, v) = get(&r, a);
        if let Some(b) = bad_record(name, a, stt, t, typ) {
            bad.push(b);
            continue;
        }
        let bytes = v.as_ref().and_then(|x| x.bytes());
        if let Some(m) = maxlen {
            if bytes.map_or(true, |b| b.len() > m) {
                let n = bytes.map(|b| b.len().to_string()).unwrap_or_else(|| "invalid".into());
                bad.push(format!("{name}: {n} octets, at most {m}"));
                continue;
            }
        }
        match bytes {
            Some(b) => vals.push(format!("{name}={}", String::from_utf8_lossy(b))),
            None => vals.push(format!("{name}={}", py_value(&v))),
        }
    }
    expect(bad.is_empty(), bad.join("; "))?;
    Ok(vals.join(", "))
}

const ZLO_BASIC_EXTRA: [(u16, u8, &str); 4] = [
    (0x0008, ENUM8, "GenericDeviceClass"),
    (0x0009, ENUM8, "GenericDeviceType"),
    (0x000A, OCTSTR, "ProductCode"),
    (0x000B, STRING, "ProductURL"),
];

fn basic_zlo_extra(s: &mut Session) -> R<String> {
    s.need_cluster(BASIC)?;
    let ids: Vec<u16> = ZLO_BASIC_EXTRA.iter().map(|x| x.0).collect();
    let r = read_all(s, BASIC, &ids)?;
    let mut bad: Vec<String> = Vec::new();
    for (a, typ, name) in ZLO_BASIC_EXTRA {
        let (stt, t, _) = get(&r, a);
        if let Some(b) = bad_record(name, a, stt, t, typ) {
            bad.push(b);
        }
    }
    expect(bad.is_empty(), bad.join("; "))?;
    let (Some(cls), Some(typ)) = (int(&get(&r, 0x0008).2), int(&get(&r, 0x0009).2)) else {
        return Err(type_error("class or type is not an integer"));
    };
    if cls == 0x00 {
        expect(
            typ <= 0x0C || (0xE0..=0xE3).contains(&typ) || (0xF0..=0xF4).contains(&typ) || typ == 0xFF,
            format!("GenericDeviceType {typ:#04x} is reserved for the lighting class"),
        )?;
    } else {
        expect(cls == 0xFF, format!("GenericDeviceClass {cls:#04x} is reserved (0x00 lighting; 0xff is the default)"))?;
    }
    Ok(format!("class {cls:#04x}, type {typ:#04x}"))
}

fn basic_reset(s: &mut Session) -> R<String> {
    s.need_cluster(BASIC)?;
    let mut groups = s.has_cluster(GROUPS)?;
    let r = (|| {
        if groups {
            let f = s.zcl(GROUPS, 0x00, &[0x0B, 0x0B, 0x00], Tx::new())?;
            let ok = f.as_ref().and_then(|f| f.zcl()).is_some_and(|z| z.specific && z.payload.first() == Some(&0x00));
            if !ok {
                groups = false;
            }
        }
        identify(s, 120)?;
        if identify_time(s)? < 100 {
            return Err(Stop::Inconclusive("IdentifyTime could not be set before the reset".into()));
        }
        let st = s.cmd(BASIC, 0x00, &[])?;
        if is_status(&st, UNSUP_COMMAND) {
            return Err(Stop::NotApplicable(
                "the optional Reset to Factory Defaults command is not supported (UNSUP_COMMAND)".into(),
            ));
        }
        let txt = status_text(&st)?;
        expect(is_status(&st, SUCCESS), format!("Reset to Factory Defaults answered {txt}"))?;
        s.run(1500.0)?;
        let alive = s.alive()?;
        expect(alive, "the bulb no longer answers after Reset to Factory Defaults; networking must not be affected")?;
        let v = identify_time(s)?;
        expect(v == 0, format!("IdentifyTime is {v} after Reset to Factory Defaults; its default is 0"))?;
        if groups {
            let f = s.zcl(GROUPS, 0x01, &[0x0B, 0x0B], Tx::new())?;
            let ok = f.as_ref().and_then(|f| f.zcl()).is_some_and(|z| z.specific && z.payload.first() == Some(&0x00));
            expect(
                ok,
                format!(
                    "group 0x0b0b is gone after Reset to Factory Defaults (View Group: {}); groups must stay",
                    text(&f)
                ),
            )?;
        }
        Ok(String::new())
    })();
    let cleanup = (|| {
        stop_identify(s)?;
        if s.has_cluster(GROUPS)? {
            s.zcl(GROUPS, 0x03, &[0x0B, 0x0B], Tx::new().max_ms(600.0))?;
        }
        Ok(())
    })();
    fin(r, cleanup)
}

// ------------------------------------------------------------------ Identify
fn identify_time_attr(s: &mut Session) -> R<String> {
    s.need_cluster(IDENTIFY)?;
    let r = (|| {
        let (stt, t, _) = s.read1(IDENTIFY, 0x0000)?;
        expect(
            stt == Some(SUCCESS) && t == Some(U16),
            format!("IdentifyTime: status {}, type {}", opt_str(stt), opt_str(t)),
        )?;
        let f = s.write(IDENTIFY, &[(0x0000, U16, 60u16.to_le_bytes().to_vec())])?;
        expect(
            f.as_ref().and_then(|f| f.zcl()).is_some_and(|z| z.payload.first() == Some(&0x00)),
            format!("writing IdentifyTime answered {}", text(&f)),
        )?;
        let t0 = s.now();
        let v1 = identify_time(s)?;
        expect((58..=60).contains(&v1), format!("IdentifyTime reads {v1} right after writing 60"))?;
        s.run(5000.0)?;
        let v2 = identify_time(s)?;
        let dt = (s.now() - t0) / 1000.0;
        expect(
            ((60 - v2) as f64 - dt).abs() <= 1.5,
            format!("IdentifyTime went from 60 to {v2} in {dt:.1} s; one per second required"),
        )?;
        Ok(format!("60 -> {v2} in {dt:.1} s"))
    })();
    fin(r, stop_identify(s))
}

fn identify_cmd(s: &mut Session) -> R<String> {
    s.need_cluster(IDENTIFY)?;
    let r = (|| {
        let st = identify(s, 45)?;
        let txt = status_text(&st)?;
        expect(is_status(&st, SUCCESS), format!("Identify answered {txt}"))?;
        let v = identify_time(s)?;
        expect((43..=45).contains(&v), format!("IdentifyTime reads {v} after Identify(45)"))?;
        let st = identify(s, 0)?;
        let txt = status_text(&st)?;
        expect(is_status(&st, SUCCESS), format!("Identify(0) answered {txt}"))?;
        let v = identify_time(s)?;
        expect(v == 0, format!("IdentifyTime reads {v} after Identify(0)"))?;
        Ok(String::new())
    })();
    fin(r, stop_identify(s))
}

fn identify_query(s: &mut Session) -> R<String> {
    s.need_cluster(IDENTIFY)?;
    let r = (|| {
        identify(s, 50)?;
        let f = s.zcl(IDENTIFY, 0x01, &[], Tx::new())?;
        expect(is_query_rsp(&f), format!("answer to Identify Query while identifying: {}", text(&f)))?;
        let fr = f.as_ref().unwrap();
        let dst = fr.nwk.as_ref().unwrap().dst;
        expect(
            dst == 0x0000 && fr.mac.as_ref().unwrap().dst != Some(0xFFFF),
            format!("the response went to {dst:#06x}"),
        )?;
        let p = fr.zcl().unwrap().payload;
        expect(p.len() == 2, format!("payload {}; a 16-bit Timeout is required", hex(&p)))?;
        let to = u16::from_le_bytes([p[0], p[1]]);
        expect(
            (47..=50).contains(&to),
            format!("Timeout {to} while IdentifyTime was set to 50 less than a second before"),
        )?;
        Ok(format!("Timeout {to}"))
    })();
    fin(r, stop_identify(s))
}

fn identify_query_idle(s: &mut Session) -> R<String> {
    s.need_cluster(IDENTIFY)?;
    stop_identify(s)?;
    let v = identify_time(s)?;
    expect(v == 0, "precondition: IdentifyTime is 0")?;
    // sent with Disable Default Response = 1, so that nothing at all may come back
    let f = s.zcl(IDENTIFY, 0x01, &[], Tx::new().ddr().max_ms(1500.0))?;
    expect(f.is_none(), format!("answer to Identify Query while not identifying: {}", text(&f)))?;
    // negative control: the same query is answered once the device identifies
    let r = (|| {
        identify(s, 30)?;
        let g = s.zcl(IDENTIFY, 0x01, &[], Tx::new().ddr())?;
        if !is_query_rsp(&g) {
            return Err(Stop::Inconclusive(
                "control failed: Identify Query is not answered while identifying either".into(),
            ));
        }
        Ok(String::new())
    })();
    fin(r, stop_identify(s))
}

fn identify_query_broadcast(s: &mut Session) -> R<String> {
    s.need_cluster(IDENTIFY)?;
    stop_identify(s)?;
    let f = s.zcl(IDENTIFY, 0x01, &[], Tx::new().dst(0xFFFF).max_ms(1500.0))?;
    expect(f.is_none(), format!("a broadcast Identify Query was answered while not identifying: {}", text(&f)))?;
    let r = (|| {
        identify(s, 180)?;
        for dst in [0xFFFFu16, 0xFFFD] {
            let f = ask_broadcast(s, IDENTIFY, 0x01, &[], 3, Tx::new().dst(dst).max_ms(2000.0))?;
            expect(
                f.is_some(),
                format!(
                    "no answer to an Identify Query broadcast to {dst:#06x} while identifying (IdentifyTime 180; sent \
                     three times)"
                ),
            )?;
            expect(is_query_rsp(&f), format!("answer to the broadcast Identify Query: {}", text(&f)))?;
            let fr = f.as_ref().unwrap();
            let (n, a) = (fr.nwk.as_ref().unwrap(), fr.aps.as_ref().unwrap());
            expect(
                n.dst == 0x0000 && a.delivery == 0,
                format!(
                    "the response went to {:#06x}, delivery mode {}; it must be unicast to the requester",
                    n.dst, a.delivery
                ),
            )?;
            expect(a.src_ep == Some(1), format!("source endpoint {} in the response", opt_str(a.src_ep)))?;
        }
        Ok(String::new())
    })();
    fin(r, stop_identify(s))
}

fn identify_ends(s: &mut Session) -> R<String> {
    s.need_cluster(IDENTIFY)?;
    let r = (|| {
        identify(s, 3)?;
        let f = s.zcl(IDENTIFY, 0x01, &[], Tx::new().ddr())?;
        if !is_query_rsp(&f) {
            return Err(Stop::Inconclusive(format!("Identify Query is not answered while identifying: {}", text(&f))));
        }
        s.run(4500.0)?;
        let v = identify_time(s)?;
        expect(v == 0, format!("IdentifyTime is {v} 4.5 s after Identify(3)"))?;
        let f = s.zcl(IDENTIFY, 0x01, &[], Tx::new().ddr().max_ms(1500.0))?;
        expect(f.is_none(), format!("Identify Query still answered after IdentifyTime ran out: {}", text(&f)))?;
        Ok(String::new())
    })();
    fin(r, stop_identify(s))
}

fn duty_samples(s: &mut Session, ms: f64, step: f64) -> R<Vec<f64>> {
    let mut out = Vec::new();
    for _ in 0..(ms / step) as usize {
        s.run(step)?;
        let l = s.light();
        out.push(if l.on { py_round(l.duty, 1) } else { 0.0 });
    }
    Ok(out)
}

fn identify_visible(s: &mut Session) -> R<String> {
    s.need_cluster(IDENTIFY)?;
    let r = (|| {
        let steady = duty_samples(s, 1500.0, 100.0)?;
        if sorted_set(&steady).len() != 1 {
            return Err(Stop::Inconclusive(format!(
                "the light output is not steady before identifying: {}",
                py_floats(&sorted_set(&steady))
            )));
        }
        identify(s, 4)?;
        let during = duty_samples(s, 3000.0, 100.0)?;
        s.run(3000.0)?;
        let after = duty_samples(s, 1500.0, 100.0)?;
        expect(
            sorted_set(&during).len() > 1,
            format!("light output constant at {} % during 3 s of identifying", py_float(during[0])),
        )?;
        expect(
            sorted_set(&after).len() == 1,
            format!("light output still changing 2 s after IdentifyTime ran out: {}", py_floats(&sorted_set(&after))),
        )?;
        Ok(format!(
            "output values while identifying: {} %; before {} %, after {} %",
            py_floats(&sorted_set(&during)),
            py_float(steady[0]),
            py_float(after[0])
        ))
    })();
    fin(r, stop_identify(s))
}

fn trigger_effect(s: &mut Session) -> R<String> {
    s.need_cluster(IDENTIFY)?;
    let before = duty_samples(s, 500.0, 100.0)?;
    let st = s.cmd(IDENTIFY, 0x40, &[0x00, 0x00])?;
    let during = duty_samples(s, 2500.0, 100.0)?;
    s.zcl(IDENTIFY, 0x40, &[0xFF, 0x00], Tx::new().ddr().max_ms(300.0))?; // stop effect
    s.run(1500.0)?;
    expect(!matches!(st, CmdAnswer::None), "no Default Response to Trigger Effect")?;
    let CmdAnswer::Status(x) = st else { return Err(type_error("unsupported format string passed to Frame")) };
    expect(
        x == SUCCESS,
        format!("Trigger Effect (blink) answered with status {x:#04x}; the command is mandatory for a dimmable light"),
    )?;
    let mut all = during.clone();
    all.extend(&before);
    let changed = sorted_set(&all).len() > 1;
    // how an effect looks is left to the implementer (3.5.2.3.3), so the output is reported, not judged
    Ok(if changed {
        format!("output values during the effect: {} %", py_floats(&sorted_set(&during)))
    } else {
        format!("SUCCESS, but the light output stayed at {} % for 2.5 s", py_float(during[0]))
    })
}
