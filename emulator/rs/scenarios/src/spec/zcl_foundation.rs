use std::collections::HashMap;

use super::Test;
use crate::air::NodeId;
use crate::frames::ZclValue;
use crate::session::{expect, is_zcl, CapFrame, Session, Stop, R};
use crate::station::Tx;

const S: &str = "zcl-foundation";

const SUCCESS: u8 = 0x00;
const FAILURE: u8 = 0x01;
const MALFORMED: u8 = 0x80;
const UNSUP_COMMAND: u8 = 0x81;
const INVALID_FIELD: u8 = 0x85;
const UNSUP_ATTR: u8 = 0x86;
const INVALID_VALUE: u8 = 0x87;
const READ_ONLY: u8 = 0x88;
const INVALID_TYPE: u8 = 0x8D;
const UNSUP_CLUSTER: u8 = 0xC3;
const BASIC: u16 = 0x0000;
const IDENTIFY: u16 = 0x0003;
const ONOFF: u16 = 0x0006;
const ABSENT_ATTR: u16 = 0x0F00;
const U8: u8 = 0x20;
const U16: u8 = 0x21;
const BOOL: u8 = 0x10;
const ENUM8: u8 = 0x30;

pub fn py_float(x: f64) -> String {
    let s = format!("{x}");
    if s.contains('.') || s.contains('e') || s.contains("inf") || s.contains("NaN") {
        s
    } else {
        s + ".0"
    }
}

pub fn py_round(x: f64, n: usize) -> f64 {
    format!("{x:.n$}").parse().unwrap()
}

pub fn py_strs<S: AsRef<str>>(v: &[S]) -> String {
    let parts: Vec<String> = v.iter().map(|s| format!("'{}'", s.as_ref())).collect();
    format!("[{}]", parts.join(", "))
}

pub fn py_floats(v: &[f64]) -> String {
    let parts: Vec<String> = v.iter().map(|x| py_float(*x)).collect();
    format!("[{}]", parts.join(", "))
}

pub fn py_hex(x: u64) -> String {
    format!("{x:#x}")
}

pub fn opt_str<T: ToString>(v: Option<T>) -> String {
    v.map(|x| x.to_string()).unwrap_or_else(|| "None".into())
}

pub fn sorted_set(v: &[f64]) -> Vec<f64> {
    let mut o = v.to_vec();
    o.sort_by(|a, b| a.partial_cmp(b).unwrap());
    o.dedup();
    o
}

pub fn fin<T>(r: R<T>, cleanup: R<()>) -> R<T> {
    cleanup?;
    r
}

pub fn int(v: &Option<ZclValue>) -> Option<i64> {
    v.as_ref().and_then(|x| x.int())
}

pub fn type_error(what: &str) -> Stop {
    Stop::Error(format!("TypeError: {what}"))
}

pub fn air_disturbed(s: &mut Session, since: usize, lost: usize) -> bool {
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

pub fn retry(s: &mut Session, f: fn(&mut Session) -> R<String>) -> R<String> {
    let (lost, since) = (s.w.coord.st.no_ack.len(), s.mark());
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

pub type Records = HashMap<u16, (u8, Option<u8>, Option<ZclValue>)>;

pub fn records(v: Vec<(u16, u8, Option<u8>, Option<ZclValue>)>) -> Records {
    v.into_iter().map(|(a, st, t, x)| (a, (st, t, x))).collect()
}

fn st(v: u8) -> String {
    let retired = match v {
        0x82 => Some("UNSUP_GENERAL_COMMAND"),
        0x83 => Some("UNSUP_MANUF_CLUSTER_COMMAND"),
        0x84 => Some("UNSUP_MANUF_GENERAL_COMMAND"),
        0x8A => Some("DUPLICATE_EXISTS"),
        0x8F => Some("WRITE_ONLY"),
        0x90 => Some("INCONSISTENT_STARTUP_STATE"),
        0x91 => Some("DEFINED_OUT_OF_BAND"),
        0x93 => Some("ACTION_DENIED"),
        0xC0 => Some("HARDWARE_FAILURE"),
        0xC1 => Some("SOFTWARE_FAILURE"),
        0xC4 => Some("LIMIT_REACHED"),
        _ => None,
    };
    format!("{v:#04x}") + &retired.map(|r| format!(" (retired in ZCL8: {r})")).unwrap_or_default()
}

fn gen(s: &mut Session, cluster: u16, cmd: u8, payload: &[u8]) -> R<Option<CapFrame>> {
    s.zcl(cluster, cmd, payload, Tx::new().general())
}

fn default_rsp(f: &Option<CapFrame>) -> Option<(u8, u8)> {
    let z = f.as_ref()?.zcl()?;
    if !z.specific && z.cmd == 0x0B && z.payload.len() >= 2 {
        return Some((z.payload[0], z.payload[1]));
    }
    None
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn describe(f: &Option<CapFrame>) -> String {
    let Some(fr) = f else { return "no response".into() };
    if let Some(d) = default_rsp(f) {
        return format!("Default Response to command {:#04x} with status {}", d.0, st(d.1));
    }
    let z = fr.zcl().unwrap();
    format!("{} command {:#04x} payload {}", if z.specific { "cluster" } else { "general" }, z.cmd, hex(&z.payload))
}

fn identify_time(s: &mut Session) -> R<i64> {
    match int(&s.value(IDENTIFY, 0x0000)?) {
        Some(v) => Ok(v),
        None => Err(Stop::Inconclusive("IdentifyTime could not be read".into())),
    }
}

fn stop_identify(s: &mut Session) -> R<()> {
    s.zcl(IDENTIFY, 0x00, &0u16.to_le_bytes(), Tx::new().ddr().max_ms(300.0)).map(|_| ())
}

fn write_records(f: &CapFrame) -> Vec<(u8, Option<u16>)> {
    let p = f.zcl().unwrap().payload;
    if p.len() == 1 {
        return vec![(p[0], None)];
    }
    let mut v = Vec::new();
    let mut i = 0;
    while i + 2 < p.len() {
        v.push((p[i], Some(u16::from_le_bytes([p[i + 1], p[i + 2]]))));
        i += 3;
    }
    v
}

fn recs_text(recs: &[(u8, Option<u16>)]) -> String {
    let parts: Vec<String> = recs.iter().map(|(a, b)| format!("('{}', {})", st(*a), opt_str(*b))).collect();
    format!("[{}]", parts.join(", "))
}

fn write_rsp(s: &mut Session, cluster: u16, records: &[(u16, u8, Vec<u8>)]) -> R<CapFrame> {
    let f = s.write(cluster, records)?;
    expect(f.is_some(), "no response to Write Attributes")?;
    let z = f.as_ref().unwrap().zcl().unwrap();
    expect(!z.specific && z.cmd == 0x04, format!("answer to Write Attributes: {}", describe(&f)))?;
    Ok(f.unwrap())
}

fn u16v(v: u16) -> Vec<u8> {
    v.to_le_bytes().to_vec()
}

fn ids_hex(v: &[u16]) -> String {
    let h: Vec<String> = v.iter().map(|a| py_hex(*a as u64)).collect();
    py_strs(&h)
}

macro_rules! wrapped {
    ($($w:ident => $f:ident),* $(,)?) => {
        $(fn $w(s: &mut Session) -> R<String> { retry(s, $f) })*
    };
}

wrapped!(
    w_read_supported => read_supported, w_read_unsupported => read_unsupported, w_read_several => read_several,
    w_write_success => write_success, w_write_read_only => write_read_only, w_write_wrong_type => write_wrong_type,
    w_write_unsupported => write_unsupported, w_write_out_of_range => write_out_of_range, w_write_mixed => write_mixed,
    w_write_undivided => write_undivided, w_write_no_response => write_no_response, w_discover_basic => discover_basic,
    w_discover_complete => discover_complete, w_dr_success => dr_success, w_dr_disabled => dr_disabled,
    w_dr_error_when_disabled => dr_error_when_disabled, w_dr_broadcast => dr_broadcast,
    w_dr_to_default_response => dr_to_default_response, w_dr_unsupported_general => dr_unsupported_general,
    w_dr_manufacturer_specific => dr_manufacturer_specific, w_dr_unsupported_cluster => dr_unsupported_cluster,
    w_malformed => malformed, w_cluster_revision => cluster_revision, w_cfg_success => cfg_success,
    w_cfg_unsupported => cfg_unsupported, w_cfg_wrong_type => cfg_wrong_type, w_cfg_max_below_min => cfg_max_below_min,
    w_read_reporting_cfg => read_reporting_cfg, w_report_on_change => report_on_change,
    w_report_periodic => report_periodic, w_report_min_interval => report_min_interval,
    w_report_max_zero => report_max_zero, w_report_off => report_off, w_report_without_binding => report_without_binding,
);

pub fn tests() -> Vec<Test> {
    vec![
        Test { id: "ZCL-FND-READ-01", doc: "ZCL8", clause: "2.5.1.3, 2.5.2, 2.4.1.1.4, 2.4.1.3",
               req: "Read Attributes for a supported attribute is answered by a Read Attributes Response with the \
                     attribute id, SUCCESS, the data type and the value; the response carries the request's sequence \
                     number and has Disable Default Response set.",
               session: S, run: w_read_supported, ..Test::D },
        Test { id: "ZCL-FND-READ-02", doc: "ZCL8", clause: "2.5.1.3",
               req: "Read Attributes for an attribute that does not exist yields a status record with \
                     UNSUPPORTED_ATTRIBUTE and no data type or value.",
               session: S, run: w_read_unsupported, ..Test::D },
        Test { id: "ZCL-FND-READ-03", doc: "ZCL8", clause: "2.5.1.3, 2.5.2.2",
               req: "A Read Attributes with several identifiers yields one status record per identifier, each \
                     carrying the identifier asked for; an unsupported one in the middle does not stop the others.",
               session: S, run: w_read_several, ..Test::D },
        Test { id: "ZCL-FND-WRITE-01", doc: "ZCL8", clause: "2.5.3.3, 2.5.5.1.2",
               req: "A Write Attributes in which every record succeeds writes the values and is answered by a Write \
                     Attributes Response holding a single status SUCCESS without an attribute id.",
               session: S, run: w_write_success, ..Test::D },
        Test { id: "ZCL-FND-WRITE-02", doc: "ZCL8", clause: "2.5.3.3 check 3",
               req: "Writing a read-only attribute yields a status record READ_ONLY with the attribute id, and the \
                     value is unchanged.",
               session: S, run: w_write_read_only, ..Test::D },
        Test { id: "ZCL-FND-WRITE-03", doc: "ZCL8", clause: "2.5.3.3 check 2",
               req: "Writing a supported attribute with the wrong data type yields a status record INVALID_DATA_TYPE, \
                     and the value is unchanged.",
               session: S, run: w_write_wrong_type, ..Test::D },
        Test { id: "ZCL-FND-WRITE-04", doc: "ZCL8", clause: "2.5.3.3 check 1",
               req: "Writing an attribute that does not exist yields a status record UNSUPPORTED_ATTRIBUTE with the \
                     attribute id.",
               session: S, run: w_write_unsupported, ..Test::D },
        Test { id: "ZCL-FND-WRITE-05", doc: "ZCL8", clause: "2.5.3.3 checks 5, 6",
               req: "Writing a value outside the attribute's specified range yields a status record INVALID_VALUE and \
                     the attribute keeps its old value.",
               session: S, run: w_write_out_of_range, ..Test::D },
        Test { id: "ZCL-FND-WRITE-06", doc: "ZCL8", clause: "2.5.3.3, 2.5.5.1.2",
               req: "In a Write Attributes with a good and a bad record the good one is written and the response \
                     holds a status record only for the bad one.",
               session: S, run: w_write_mixed, ..Test::D },
        Test { id: "ZCL-FND-WRITE-07", doc: "ZCL8", clause: "2.5.4",
               req: "Write Attributes Undivided changes no attribute when any record cannot be written, and writes \
                     all of them when every record is valid.",
               session: S, run: w_write_undivided, ..Test::D },
        Test { id: "ZCL-FND-WRITE-08", doc: "ZCL8", clause: "2.5.6.3",
               req: "Write Attributes No Response writes the valid records and is never answered, neither by a \
                     response nor by a Default Response, also when a record is in error.",
               session: S, run: w_write_no_response, ..Test::D },
        Test { id: "ZCL-FND-DISC-01", doc: "ZCL8", clause: "2.5.13.3, 2.5.14.1",
               req: "Discover Attributes returns at most the requested number of attribute records, in ascending \
                     order starting at the first identifier not below the start identifier.",
               session: S, run: w_discover_basic, ..Test::D },
        Test { id: "ZCL-FND-DISC-02", doc: "ZCL8", clause: "2.5.14.1.2",
               req: "The discovery-complete field is 0 exactly when attributes with a higher identifier than the last \
                     one returned remain, and 1 when none remain.",
               session: S, run: w_discover_complete, ..Test::D },
        Test { id: "ZCL-FND-DR-01", doc: "ZCL8", clause: "2.5.12.2, 2.5.12.1, 2.4.1.1.4",
               req: "A unicast command that succeeds, has no response of its own and does not disable the Default \
                     Response is answered by a Default Response naming the command, with SUCCESS, the same sequence \
                     number and Disable Default Response set.",
               session: S, run: w_dr_success, ..Test::D },
        Test { id: "ZCL-FND-DR-02", doc: "ZCL8", clause: "2.5.12.2 criterion 3, 2.4.1.1.4",
               req: "A unicast command that succeeds with Disable Default Response = 1 is executed and not answered.",
               session: S, run: w_dr_disabled, ..Test::D },
        Test { id: "ZCL-FND-DR-03", doc: "ZCL8", clause: "2.5.12.2",
               req: "A unicast command the cluster does not support is answered by a Default Response with \
                     UNSUP_COMMAND (0x81) even when Disable Default Response = 1.",
               session: S, run: w_dr_error_when_disabled, ..Test::D },
        Test { id: "ZCL-FND-DR-04", doc: "ZCL8", clause: "2.5.12.2, 2.3.2.1",
               req: "A command received by broadcast is never answered by a Default Response, neither on success nor \
                     in error.",
               session: S, run: w_dr_broadcast, ..Test::D },
        Test { id: "ZCL-FND-DR-05", doc: "ZCL8", clause: "2.5.12.2",
               req: "A Default Response is never generated in answer to a Default Response.",
               session: S, run: w_dr_to_default_response, ..Test::D },
        Test { id: "ZCL-FND-DR-06", doc: "ZCL8", clause: "2.5.12.2, Table 2-12",
               req: "A unicast general command the device does not support is answered by a Default Response with \
                     UNSUP_COMMAND (0x81); ZCL8 retired UNSUP_GENERAL_COMMAND (0x82).",
               session: S, run: w_dr_unsupported_general, ..Test::D },
        Test { id: "ZCL-FND-DR-07", doc: "ZCL8", clause: "2.3.3, 2.5.12.2, Table 2-12",
               req: "A unicast manufacturer-specific cluster command with an unknown manufacturer code is not carried \
                     out and is answered by a Default Response with UNSUP_COMMAND (0x81); ZCL8 retired \
                     UNSUP_MANUF_CLUSTER_COMMAND (0x83).",
               session: S, run: w_dr_manufacturer_specific, ..Test::D },
        Test { id: "ZCL-FND-DR-08", doc: "ZCL8", clause: "2.5.12.2",
               req: "A unicast cluster command for a cluster that does not exist on the addressed endpoint is \
                     answered by a Default Response with UNSUPPORTED_CLUSTER (0xc3).",
               session: S, run: w_dr_unsupported_cluster, ..Test::D },
        Test { id: "ZCL-FND-MALF-01", doc: "ZCL8", clause: "2.3.2, Table 2-12",
               req: "A unicast cluster command with fields missing is not carried out and is answered by a Default \
                     Response with MALFORMED_COMMAND (0x80) or, at the implementer's discretion, INVALID_FIELD (0x85).",
               session: S, run: w_malformed, mandatory: false,
               ambiguous: Some("Table 2-12 describes MALFORMED_COMMAND as the status for missing fields ('command not \
                                carried out'), but no clause says in so many words that a short payload SHALL be \
                                detected; 2.3.2 only requires ignoring octets appended to a frame"),
               ..Test::D },
        Test { id: "ZCL-FND-CLREV-01", doc: "ZCL8", clause: "2.3.4.5",
               req: "Every server cluster instance supports the global ClusterRevision attribute (0xfffd, uint16) with \
                     a value in 1..0xfffe.",
               session: S, run: w_cluster_revision, ..Test::D },
        Test { id: "ZCL-FND-REP-01", doc: "ZCL8", clause: "2.5.7.3, 2.5.8.1.3",
               req: "A Configure Reporting in which every record is accepted is answered by a Configure Reporting \
                     Response holding a single status SUCCESS, without direction and attribute id.",
               session: S, run: w_cfg_success, ..Test::D },
        Test { id: "ZCL-FND-REP-02", doc: "ZCL8", clause: "2.5.7.3",
               req: "Configure Reporting for an attribute that does not exist yields a status record \
                     UNSUPPORTED_ATTRIBUTE with direction and attribute id.",
               session: S, run: w_cfg_unsupported, ..Test::D },
        Test { id: "ZCL-FND-REP-03", doc: "ZCL8", clause: "2.5.7.3",
               req: "Configure Reporting with the wrong data type for a reportable attribute yields a status record \
                     INVALID_DATA_TYPE.",
               session: S, run: w_cfg_wrong_type, ..Test::D },
        Test { id: "ZCL-FND-REP-04", doc: "ZCL8", clause: "2.5.7.3",
               req: "Configure Reporting with a non-zero maximum interval smaller than the minimum interval yields a \
                     status record INVALID_VALUE.",
               session: S, run: w_cfg_max_below_min, ..Test::D },
        Test { id: "ZCL-FND-REP-05", doc: "ZCL8", clause: "2.5.9.2, 2.5.10.1",
               req: "Read Reporting Configuration returns, for a configured attribute, SUCCESS with direction, \
                     attribute id, data type and the configured intervals; for an attribute that does not exist, \
                     UNSUPPORTED_ATTRIBUTE with only direction and attribute id.",
               session: S, run: w_read_reporting_cfg, ..Test::D },
        Test { id: "ZCL-FND-REP-06", doc: "ZCL8", clause: "2.5.11.2.2, 2.5.7.1.2 Table 2-4, 2.5.11.1",
               req: "With a binding and a reporting configuration, a change of a discrete attribute is reported to the \
                     bound destination in a Report Attributes command carrying attribute id, data type and the new \
                     value.",
               session: S, run: w_report_on_change, ..Test::D },
        Test { id: "ZCL-FND-REP-07", doc: "ZCL8", clause: "2.5.11.2.1, 2.5.11.2.5",
               req: "With a maximum reporting interval set, a report is generated whenever that interval has elapsed \
                     since the previous report, without any change of the attribute (the interval may be shortened \
                     by timer consolidation, never lengthened).",
               session: S, run: w_report_periodic, ..Test::D },
        Test { id: "ZCL-FND-REP-08", doc: "ZCL8", clause: "2.5.11.2.2, 2.5.7.1.5",
               req: "After a report, no further report of the attribute is sent before the minimum reporting interval \
                     has elapsed, even if the attribute changes again.",
               session: S, run: w_report_min_interval,
               ambiguous: Some("2.5.11.2.5 lets a device shorten the minimum interval when it consolidates the timers \
                                of several attributes; the test configures a single attribute, where nothing is \
                                consolidated"),
               ..Test::D },
        Test { id: "ZCL-FND-REP-09", doc: "ZCL8", clause: "2.5.7.1.6",
               req: "With the maximum reporting interval set to 0x0000 (and the minimum not 0xffff) there is no \
                     periodic reporting, but changes are still reported.",
               session: S, run: w_report_max_zero, ..Test::D },
        Test { id: "ZCL-FND-REP-10", doc: "ZCL8", clause: "2.5.7.1.6",
               req: "With the maximum reporting interval set to 0xffff the device issues no reports for the attribute.",
               session: S, run: w_report_off, ..Test::D },
        Test { id: "ZCL-FND-REP-11", doc: "ZCL8", clause: "2.5.11.2, 2.5.7.1.2 Table 2-4",
               req: "Reports go to the destinations resolved by the bindings of the cluster; when there is no binding, \
                     no Report Attributes command is generated.",
               session: S, run: w_report_without_binding, ..Test::D },
    ]
}

fn read_supported(s: &mut Session) -> R<String> {
    let f = gen(s, BASIC, 0x00, &u16v(0x0000))?;
    expect(f.is_some(), "no response to Read Attributes (Basic ZCLVersion)")?;
    let z = f.as_ref().unwrap().zcl().unwrap();
    expect(!z.specific && z.cmd == 0x01, format!("answer: {}", describe(&f)))?;
    expect(z.to_client, "direction bit of the response says client to server")?;
    let p = &z.payload;
    expect(
        p.len() == 5 && u16::from_le_bytes([p[0], p[1]]) == 0x0000 && p[2] == SUCCESS && p[3] == U8,
        format!("status record {}; required: attribute 0x0000, status 0x00, type 0x20 (uint8), one octet of value", hex(p)),
    )?;
    expect(z.ddr, "the Read Attributes Response has Disable Default Response = 0; a response frame must set it to 1")?;
    Ok(format!("ZCLVersion = {}", p[4]))
}

fn read_unsupported(s: &mut Session) -> R<String> {
    let f = gen(s, BASIC, 0x00, &u16v(ABSENT_ATTR))?;
    let z = f.as_ref().and_then(|f| f.zcl());
    expect(z.as_ref().is_some_and(|z| !z.specific && z.cmd == 0x01), format!("answer: {}", describe(&f)))?;
    let p = z.unwrap().payload;
    let mut want = u16v(ABSENT_ATTR);
    want.push(UNSUP_ATTR);
    expect(
        p == want,
        format!("status record {}; required: attribute {ABSENT_ATTR:#06x}, status 0x86 and nothing else", hex(&p)),
    )?;
    Ok(String::new())
}

fn read_several(s: &mut Session) -> R<String> {
    let ids = [0x0000, ABSENT_ATTR, 0x0007];
    let pl: Vec<u8> = ids.iter().flat_map(|a| a.to_le_bytes()).collect();
    let f = gen(s, BASIC, 0x00, &pl)?;
    let z = f.as_ref().and_then(|f| f.zcl());
    expect(z.as_ref().is_some_and(|z| !z.specific && z.cmd == 0x01), format!("answer: {}", describe(&f)))?;
    let p = z.unwrap().payload;
    let want_len = 5 + 3 + 5;
    let mut got: Vec<(u16, u8)> = Vec::new();
    let mut i = 0;
    while i < p.len() {
        if i + 3 > p.len() {
            break;
        }
        let a = u16::from_le_bytes([p[i], p[i + 1]]);
        let stt = p[i + 2];
        i += 3;
        if stt == SUCCESS {
            i += 2;
        }
        got.push((a, stt));
    }
    let gids: Vec<u16> = got.iter().map(|x| x.0).collect();
    expect(
        gids == ids,
        format!("records for attributes {}, asked {} (payload {})", ids_hex(&gids), ids_hex(&ids), hex(&p)),
    )?;
    let sts: Vec<u8> = got.iter().map(|x| x.1).collect();
    let stt_txt: Vec<String> = sts.iter().map(|x| st(*x)).collect();
    expect(
        sts == [SUCCESS, UNSUP_ATTR, SUCCESS] && p.len() == want_len,
        format!("statuses {}, payload {}", py_strs(&stt_txt), hex(&p)),
    )?;
    Ok(String::new())
}

fn write_success(s: &mut Session) -> R<String> {
    let r = (|| {
        let f = write_rsp(s, IDENTIFY, &[(0x0000, U16, u16v(40))])?;
        let z = f.zcl().unwrap();
        expect(z.payload == [0x00], format!("Write Attributes Response payload {}; required: the single octet 00", hex(&z.payload)))?;
        expect(z.ddr && z.to_client, "response frame control: Disable Default Response or direction not set")?;
        let v = identify_time(s)?;
        expect((36..=40).contains(&v), format!("IdentifyTime reads {v} right after writing 40"))?;
        Ok(String::new())
    })();
    fin(r, stop_identify(s))
}

fn write_read_only(s: &mut Session) -> R<String> {
    let before = s.value(BASIC, 0x0000)?;
    let b = int(&before).unwrap_or(0) as u8;
    let f = write_rsp(s, BASIC, &[(0x0000, U8, vec![b ^ 0x10])])?;
    let recs = write_records(&f);
    expect(
        recs == [(READ_ONLY, Some(0x0000))],
        format!("status records {}; required: READ_ONLY (0x88) for attribute 0x0000", recs_text(&recs)),
    )?;
    let after = s.value(BASIC, 0x0000)?;
    expect(after == before, "ZCLVersion changed although the write was refused")?;
    Ok(String::new())
}

fn write_wrong_type(s: &mut Session) -> R<String> {
    let r = (|| {
        let f = write_rsp(s, IDENTIFY, &[(0x0000, U8, vec![33])])?;
        let recs = write_records(&f);
        let v = identify_time(s)?;
        expect(
            recs == [(INVALID_TYPE, Some(0x0000))],
            format!(
                "status records {}; required: INVALID_DATA_TYPE (0x8d) for attribute 0x0000 (IdentifyTime afterwards: {v})",
                recs_text(&recs)
            ),
        )?;
        expect(v == 0, format!("IdentifyTime is {v} after a write with the wrong type"))?;
        Ok(String::new())
    })();
    fin(r, stop_identify(s))
}

fn write_unsupported(s: &mut Session) -> R<String> {
    let f = write_rsp(s, IDENTIFY, &[(ABSENT_ATTR, U16, u16v(1))])?;
    let recs = write_records(&f);
    expect(
        recs == [(UNSUP_ATTR, Some(ABSENT_ATTR))],
        format!("status records {}; required: UNSUPPORTED_ATTRIBUTE (0x86) for attribute {ABSENT_ATTR:#06x}", recs_text(&recs)),
    )?;
    Ok(String::new())
}

fn ranged_candidates(s: &mut Session) -> R<Vec<(u16, u16, u8, i64, String)>> {
    let mut out = Vec::new();
    let mn = if s.has_cluster(0x0008)? { int(&s.value(0x0008, 0x0002)?) } else { None };
    let mn = mn.unwrap_or(1);
    if mn >= 1 {
        out.push((0x0008, 0x0011, U8, mn - 1, format!("Level OnLevel (range MinLevel..MaxLevel, MinLevel {mn})")));
    }
    out.push((BASIC, 0x0012, BOOL, 0x02, "Basic DeviceEnabled (bool; 0 or 1)".to_string()));
    out.push((ONOFF, 0x4003, ENUM8, 0x05, "On/Off StartUpOnOff (enum8; Table 3-46 calls 0x03-0xfe reserved, and \
              Table 2-12 gives INVALID_VALUE for 'set to a reserved value')".to_string()));
    Ok(out)
}

fn write_out_of_range(s: &mut Session) -> R<String> {
    let mut tried: Vec<String> = Vec::new();
    for (cluster, attr, typ, bad, name) in ranged_candidates(s)? {
        let (stt, t, before) = s.read1(cluster, attr)?;
        if stt != Some(SUCCESS) || t != Some(typ) {
            tried.push(format!("{name}: not supported"));
            continue;
        }
        let Some(b) = int(&before) else { return Err(type_error("bytes([before])")) };
        let f = s.write(cluster, &[(attr, typ, vec![b as u8])])?;
        if f.as_ref().map_or(true, |f| f.zcl().unwrap().payload != [0x00]) {
            tried.push(format!("{name}: not writable"));
            continue;
        }
        let f = write_rsp(s, cluster, &[(attr, typ, vec![bad as u8])])?;
        let recs = write_records(&f);
        let after = s.value(cluster, attr)?;
        if after != before {
            s.write(cluster, &[(attr, typ, vec![b as u8])])?;
        }
        let Some(a) = int(&after) else { return Err(type_error("format of None")) };
        expect(
            recs == [(INVALID_VALUE, Some(attr))],
            format!(
                "{name}: writing {bad:#04x} gives status records {}; required: INVALID_VALUE (0x87); value afterwards {a:#04x}",
                recs_text(&recs)
            ),
        )?;
        expect(after == before, format!("{name}: value changed from {b:#04x} to {a:#04x} despite INVALID_VALUE"))?;
        return Ok(name);
    }
    Err(Stop::Inconclusive(format!("no writable attribute with a restricted range found: {}", tried.join("; "))))
}

fn write_mixed(s: &mut Session) -> R<String> {
    let r = (|| {
        let f = write_rsp(s, IDENTIFY, &[(0x0000, U16, u16v(40)), (ABSENT_ATTR, U16, u16v(1))])?;
        let recs = write_records(&f);
        let v = identify_time(s)?;
        expect(
            recs == [(UNSUP_ATTR, Some(ABSENT_ATTR))],
            format!(
                "status records {}; required: only UNSUPPORTED_ATTRIBUTE for {ABSENT_ATTR:#06x} (IdentifyTime afterwards: {v})",
                recs_text(&recs)
            ),
        )?;
        expect((36..=40).contains(&v), format!("IdentifyTime reads {v}; the valid record (40) must be written"))?;
        Ok(String::new())
    })();
    fin(r, stop_identify(s))
}

fn write_undivided(s: &mut Session) -> R<String> {
    let r = (|| {
        let f = s.write_with(IDENTIFY, &[(0x0000, U16, u16v(40)), (ABSENT_ATTR, U16, u16v(1))], true, false, Tx::new())?;
        expect(f.is_some(), "no response to Write Attributes Undivided")?;
        let z = f.as_ref().unwrap().zcl().unwrap();
        expect(!z.specific && z.cmd == 0x04, format!("answer to Write Attributes Undivided: {}", describe(&f)))?;
        let recs = write_records(f.as_ref().unwrap());
        let v = identify_time(s)?;
        expect(
            v == 0,
            format!(
                "IdentifyTime is {v} after an undivided write with one bad record; nothing may be written (status records {})",
                recs_text(&recs)
            ),
        )?;
        expect(
            recs.contains(&(UNSUP_ATTR, Some(ABSENT_ATTR))),
            format!("status records {} lack UNSUPPORTED_ATTRIBUTE for the bad record", recs_text(&recs)),
        )?;
        let f = s.write_with(IDENTIFY, &[(0x0000, U16, u16v(40))], true, false, Tx::new())?;
        expect(
            f.as_ref().is_some_and(|f| f.zcl().unwrap().payload == [0x00]),
            format!("all-valid undivided write answered: {}", describe(&f)),
        )?;
        let v = identify_time(s)?;
        expect((36..=40).contains(&v), format!("IdentifyTime reads {v} after an all-valid undivided write of 40"))?;
        Ok(String::new())
    })();
    fin(r, stop_identify(s))
}

fn write_no_response(s: &mut Session) -> R<String> {
    let r = (|| {
        let f = s.write_with(IDENTIFY, &[(0x0000, U16, u16v(40))], false, true, Tx::new().max_ms(1200.0))?;
        let v = identify_time(s)?;
        expect(f.is_none(), format!("Write Attributes No Response was answered: {}", describe(&f)))?;
        expect((35..=40).contains(&v), format!("IdentifyTime reads {v} after Write Attributes No Response of 40 (not written)"))?;
        let f = s.write_with(IDENTIFY, &[(ABSENT_ATTR, U16, u16v(1))], false, true, Tx::new().max_ms(1200.0))?;
        expect(
            f.is_none(),
            format!("Write Attributes No Response with an unsupported attribute was answered: {}", describe(&f)),
        )?;
        Ok(String::new())
    })();
    fin(r, stop_identify(s))
}

fn discover(s: &mut Session, cluster: u16, start: u16, maxn: u8) -> R<(u8, Vec<(u16, u8)>)> {
    let mut pl = u16v(start);
    pl.push(maxn);
    let f = gen(s, cluster, 0x0C, &pl)?;
    expect(f.is_some(), "no response to Discover Attributes")?;
    let z = f.as_ref().unwrap().zcl().unwrap();
    expect(!z.specific && z.cmd == 0x0D, format!("answer to Discover Attributes: {}", describe(&f)))?;
    let p = z.payload;
    expect(
        !p.is_empty() && (p.len() - 1) % 3 == 0,
        format!("Discover Attributes Response payload {} is not 1 + 3n octets", hex(&p)),
    )?;
    let recs = (1..p.len()).step_by(3).map(|i| (u16::from_le_bytes([p[i], p[i + 1]]), p[i + 2])).collect();
    Ok((p[0], recs))
}

fn ascending(ids: &[u16]) -> bool {
    let mut v = ids.to_vec();
    v.sort();
    v.dedup();
    v == ids
}

fn discover_basic(s: &mut Session) -> R<String> {
    let (done, recs) = discover(s, BASIC, 0x0000, 2)?;
    let ids: Vec<u16> = recs.iter().map(|x| x.0).collect();
    expect(ids.len() <= 2, format!("{} records returned, at most 2 were asked for", ids.len()))?;
    expect(ascending(&ids), format!("identifiers {} are not strictly ascending", ids_hex(&ids)))?;
    expect(
        ids.first() == Some(&0x0000),
        format!(
            "first identifier is {}; ZCLVersion (0x0000) is mandatory",
            ids.first().map(|a| py_hex(*a as u64)).unwrap_or_else(|| "none".into())
        ),
    )?;
    let (_, recs2) = discover(s, BASIC, 0x0004, 3)?;
    let ids2: Vec<u16> = recs2.iter().map(|x| x.0).collect();
    expect(
        ids2.iter().all(|a| *a >= 0x0004) && ascending(&ids2) && ids2.len() <= 3,
        format!("start 0x0004, maximum 3: identifiers {}", ids_hex(&ids2)),
    )?;
    Ok(format!("from 0x0000: {}, complete={done}", ids_hex(&ids)))
}

fn discover_complete(s: &mut Session) -> R<String> {
    let mut all_ids: Vec<u16> = Vec::new();
    let mut start: u16 = 0;
    let mut rounds = 0;
    loop {
        let (done, recs) = discover(s, BASIC, start, 3)?;
        rounds += 1;
        let ids: Vec<u16> = recs.iter().map(|x| x.0).collect();
        expect(
            ids.iter().all(|a| *a >= start),
            format!("start {start:#06x}: identifiers {} below the start", ids_hex(&ids)),
        )?;
        all_ids.extend(&ids);
        if done != 0 && done != 1 {
            return Err(Stop::Fail(format!("discovery complete field is {done}")));
        }
        if done == 1 {
            break;
        }
        expect(!ids.is_empty(), format!("start {start:#06x}: complete = 0 but no record returned"))?;
        start = ids.last().unwrap().wrapping_add(1);
        expect(rounds < 40, "discovery does not terminate")?;
    }
    let last = all_ids.last().copied().unwrap_or(0);
    if last < 0xFFFF {
        let (done, recs) = discover(s, BASIC, last + 1, 3)?;
        let ids: Vec<u16> = recs.iter().map(|x| x.0).collect();
        expect(
            recs.is_empty() && done == 1,
            format!("after complete = 1, a discovery from {:#06x} returned {} (complete = {done})", last + 1, ids_hex(&ids)),
        )?;
    }
    expect(ascending(&all_ids), format!("identifiers are not in ascending order: {}", ids_hex(&all_ids)))?;
    expect(
        all_ids.contains(&0x0000) && all_ids.contains(&0x0007),
        format!("discovered {}; the mandatory ZCLVersion and PowerSource must be among them", ids_hex(&all_ids)),
    )?;
    let first6: Vec<u16> = all_ids.iter().take(6).copied().collect();
    let r = s.read(BASIC, &first6, 1500.0)?.map(records);
    let bad: Vec<String> = first6
        .iter()
        .filter(|a| r.as_ref().map_or(true, |r| r.is_empty() || r.get(a).map(|x| x.0) != Some(SUCCESS)))
        .map(|a| py_hex(*a as u64))
        .collect();
    expect(bad.is_empty(), format!("discovered attributes {} cannot be read", py_strs(&bad)))?;
    let (done, recs) = discover(s, BASIC, 0x0000, 1)?;
    if all_ids.len() > 1 {
        expect(
            done == 0 && recs.len() == 1,
            format!(
                "maximum 1 with {} attributes present: {} records, complete = {done}",
                all_ids.len(),
                recs.len()
            ),
        )?;
    }
    Ok(format!("{} attributes in {rounds} responses", all_ids.len()))
}

fn dr_success(s: &mut Session) -> R<String> {
    s.need_cluster(ONOFF)?;
    let f = s.zcl(ONOFF, 0x01, &[], Tx::new())?;
    expect(f.is_some(), "no Default Response to a unicast On command with Disable Default Response = 0")?;
    let d = default_rsp(&f);
    expect(
        d == Some((0x01, SUCCESS)),
        format!("answer: {}; required: Default Response to command 0x01 with SUCCESS", describe(&f)),
    )?;
    let z = f.as_ref().unwrap().zcl().unwrap();
    expect(z.to_client && !z.mfg_specific, "direction/manufacturer bits of the Default Response are wrong")?;
    expect(z.ddr, "the Default Response itself has Disable Default Response = 0")?;
    Ok(String::new())
}

fn dr_disabled(s: &mut Session) -> R<String> {
    s.need_cluster(ONOFF)?;
    expect(s.light().on, "precondition: light is on")?;
    let f = s.zcl(ONOFF, 0x00, &[], Tx::new().ddr().max_ms(1500.0))?;
    s.run(800.0)?;
    let off = !s.light().on;
    s.zcl(ONOFF, 0x01, &[], Tx::new().ddr().max_ms(400.0))?;
    expect(f.is_none(), format!("Off with Disable Default Response = 1 was answered: {}", describe(&f)))?;
    expect(off, "the Off command was not executed (negative control: the frame did not act)")?;
    Ok(String::new())
}

fn dr_error_when_disabled(s: &mut Session) -> R<String> {
    s.need_cluster(ONOFF)?;
    let f = s.zcl(ONOFF, 0x7E, &[], Tx::new().ddr())?;
    expect(f.is_some(), "no Default Response to an unsupported cluster command sent with Disable Default Response = 1")?;
    let d = default_rsp(&f);
    expect(d.is_some_and(|d| d.0 == 0x7E), format!("answer: {}", describe(&f)))?;
    let d = d.unwrap();
    expect(d.1 == UNSUP_COMMAND, format!("status {}; UNSUP_COMMAND (0x81) is required", st(d.1)))?;
    Ok(String::new())
}

fn dr_broadcast(s: &mut Session) -> R<String> {
    s.need_cluster(ONOFF)?;
    let f = s.zcl(ONOFF, 0x7E, &[], Tx::new().dst(0xFFFD).max_ms(1500.0))?;
    expect(f.is_none(), format!("an unsupported command sent by broadcast was answered: {}", describe(&f)))?;
    let mut f = None;
    let mut off = false;
    for _ in 0..3 {
        f = s.zcl(ONOFF, 0x00, &[], Tx::new().dst(0xFFFD).max_ms(1500.0))?;
        s.run(600.0)?;
        off = !s.light().on;
        if off || f.is_some() {
            break;
        }
    }
    s.zcl(ONOFF, 0x01, &[], Tx::new().ddr().max_ms(400.0))?;
    expect(f.is_none(), format!("a broadcast Off (Disable Default Response = 0) was answered: {}", describe(&f)))?;
    expect(off, "the broadcast Off was not executed (negative control: the broadcast did not reach the cluster)")?;
    Ok(String::new())
}

fn dr_to_default_response(s: &mut Session) -> R<String> {
    s.need_cluster(ONOFF)?;
    let f = s.zcl(ONOFF, 0x0B, &[0x00, SUCCESS], Tx::new().general().max_ms(1500.0))?;
    expect(f.is_none(), format!("a Default Response (status SUCCESS) was answered: {}", describe(&f)))?;
    let f = s.zcl(ONOFF, 0x0B, &[0x00, FAILURE], Tx::new().general().max_ms(1500.0))?;
    expect(f.is_none(), format!("a Default Response (status FAILURE) was answered: {}", describe(&f)))?;
    let f = gen(s, ONOFF, 0x00, &u16v(0x0000))?;
    expect(f.is_some(), "control failed: Read Attributes on the same cluster is not answered either")?;
    Ok(String::new())
}

fn dr_unsupported_general(s: &mut Session) -> R<String> {
    let f = gen(s, BASIC, 0x7D, &[])?;
    expect(f.is_some(), "no Default Response to an undefined general command (0x7d)")?;
    let d = default_rsp(&f);
    expect(d.is_some_and(|d| d.0 == 0x7D), format!("answer: {}", describe(&f)))?;
    let d = d.unwrap();
    expect(d.1 == UNSUP_COMMAND, format!("status {}; ZCL8 requires UNSUP_COMMAND (0x81)", st(d.1)))?;
    Ok(String::new())
}

fn dr_manufacturer_specific(s: &mut Session) -> R<String> {
    s.need_cluster(ONOFF)?;
    expect(s.light().on, "precondition: light is on")?;
    let f = s.zcl(ONOFF, 0x00, &[], Tx::new().mfg(0x1ABC))?;
    s.run(600.0)?;
    let on = s.light().on;
    if !on {
        s.zcl(ONOFF, 0x01, &[], Tx::new().ddr().max_ms(400.0))?;
    }
    expect(
        on,
        format!(
            "a command with an unknown manufacturer code was carried out as the standard Off command (answer: {})",
            describe(&f)
        ),
    )?;
    expect(f.is_some(), "no Default Response to a manufacturer-specific command with an unknown manufacturer code")?;
    let d = default_rsp(&f);
    expect(d.is_some_and(|d| d.0 == 0x00), format!("answer: {}", describe(&f)))?;
    let d = d.unwrap();
    expect(d.1 == UNSUP_COMMAND, format!("status {}; ZCL8 requires UNSUP_COMMAND (0x81)", st(d.1)))?;
    Ok(String::new())
}

fn dr_unsupported_cluster(s: &mut Session) -> R<String> {
    let Some(sd) = s.simple_desc(1)? else { return Err(type_error("simple descriptor is None")) };
    let Some(cluster) = [0x0101u16, 0x0201, 0x0402].into_iter().find(|c| !sd.inc.contains(c) && !sd.outc.contains(c))
    else {
        return Err(Stop::Error("StopIteration".into()));
    };
    let f = s.zcl(cluster, 0x00, &[], Tx::new())?;
    expect(
        f.is_some(),
        format!("no answer at all to a command for cluster {cluster:#06x}, which endpoint 1 does not have"),
    )?;
    let d = default_rsp(&f);
    expect(d.is_some(), format!("answer: {}", describe(&f)))?;
    let d = d.unwrap();
    expect(d.1 == UNSUP_CLUSTER, format!("status {}; UNSUPPORTED_CLUSTER (0xc3) is required", st(d.1)))?;
    Ok(String::new())
}

fn malformed(s: &mut Session) -> R<String> {
    s.need_cluster(IDENTIFY)?;
    let r = (|| {
        let f = s.zcl(IDENTIFY, 0x00, &[0x28], Tx::new())?;
        let v = identify_time(s)?;
        expect(
            v == 0,
            format!("a truncated Identify command was carried out: IdentifyTime = {v} (answer: {})", describe(&f)),
        )?;
        expect(f.is_some(), "no Default Response to a truncated Identify command")?;
        let d = default_rsp(&f);
        expect(d.is_some_and(|d| d.0 == 0x00), format!("answer: {}", describe(&f)))?;
        let d = d.unwrap();
        expect(
            d.1 == MALFORMED || d.1 == INVALID_FIELD,
            format!("status {}; MALFORMED_COMMAND (0x80) or INVALID_FIELD (0x85) is required", st(d.1)),
        )?;
        Ok(String::new())
    })();
    fin(r, stop_identify(s))
}

fn cluster_revision(s: &mut Session) -> R<String> {
    let Some(sd) = s.simple_desc(1)? else { return Err(type_error("simple descriptor is None")) };
    let mut bad: Vec<String> = Vec::new();
    let mut good: Vec<String> = Vec::new();
    for c in sd.inc {
        if c >= 0xFC00 {
            continue;
        }
        let (stt, t, v) = s.read1(c, 0xFFFD)?;
        let iv = int(&v);
        if stt == Some(SUCCESS) && t == Some(U16) && iv.is_some_and(|v| (1..=0xFFFE).contains(&v)) {
            good.push(format!("{c:#06x}={}", iv.unwrap()));
        } else if stt.is_none() {
            bad.push(format!("{c:#06x}: no Read Attributes Response"));
        } else if stt != Some(SUCCESS) {
            bad.push(format!("{c:#06x}: status {}", st(stt.unwrap())));
        } else {
            let Some(t) = t else { return Err(type_error("type is None")) };
            bad.push(format!("{c:#06x}: type {t:#04x} value {}", crate::frames::py_value(&v)));
        }
    }
    let extra = if good.is_empty() { String::new() } else { format!(" (present: {})", good.join(", ")) };
    expect(bad.is_empty(), format!("ClusterRevision missing or malformed on {}{extra}", bad.join("; ")))?;
    Ok(good.join(", "))
}

fn cfg_record(attr: u16, typ: u8, mn: u16, mx: u16, change: &[u8]) -> Vec<u8> {
    let mut v = vec![0x00];
    v.extend_from_slice(&attr.to_le_bytes());
    v.push(typ);
    v.extend_from_slice(&mn.to_le_bytes());
    v.extend_from_slice(&mx.to_le_bytes());
    v.extend_from_slice(change);
    v
}

fn configure(s: &mut Session, cluster: u16, record: &[u8]) -> R<Vec<u8>> {
    let what = "Configure Reporting";
    let f = gen(s, cluster, 0x06, record)?;
    expect(f.is_some(), format!("no response to {what}"))?;
    let z = f.as_ref().unwrap().zcl().unwrap();
    expect(!z.specific && z.cmd == 0x07, format!("answer to {what}: {}", describe(&f)))?;
    Ok(z.payload)
}

fn configured(s: &mut Session, record: &[u8]) -> R<()> {
    let p = configure(s, ONOFF, record)?;
    if !(p.first() == Some(&0x00) && (p.len() == 1 || p.len() == 4)) {
        return Err(Stop::Inconclusive(format!("reporting could not be configured: Configure Reporting Response {}", hex(&p))));
    }
    Ok(())
}

fn bind_onoff(s: &mut Session, unbind: bool) -> R<()> {
    let mut pl = s.dut_ext.to_le_bytes().to_vec();
    pl.push(1);
    pl.extend_from_slice(&ONOFF.to_le_bytes());
    pl.push(0x03);
    pl.extend_from_slice(&s.w.coord.ext.to_le_bytes());
    pl.push(1);
    let f = s.zdo(if unbind { 0x0022 } else { 0x0021 }, &pl, 2000.0)?;
    let st0 = f.as_ref().map(|f| f.zdo().unwrap().payload[0]);
    if st0.map_or(true, |x| x != 0x00 && x != 0x88) {
        return Err(Stop::Inconclusive(format!(
            "{}_req for On/Off failed: {}",
            if unbind { "Unbind" } else { "Bind" },
            st0.map(|x| py_hex(x as u64)).unwrap_or_else(|| "no response".into())
        )));
    }
    s.set_note("onoff_bound", !unbind);
    Ok(())
}

fn reports(s: &mut Session, ms: f64, since: Option<usize>) -> R<Vec<CapFrame>> {
    s.collect(is_zcl(Some(ONOFF), Some(0x0A), None, Some(false), None), ms, since)
}

fn set_onoff(s: &mut Session, on: bool) -> R<()> {
    s.zcl(ONOFF, if on { 0x01 } else { 0x00 }, &[], Tx::new().ddr().max_ms(250.0)).map(|_| ())
}

fn quiet(s: &mut Session) -> R<()> {
    gen(s, ONOFF, 0x06, &cfg_record(0x0000, BOOL, 0, 0xFFFF, &[]))?;
    set_onoff(s, true)
}

fn gaps_of(r: &[CapFrame]) -> Vec<f64> {
    r.windows(2).map(|w| (w[1].t0 - w[0].t0) / 1000.0).collect()
}

fn gaps_text(g: &[f64]) -> String {
    py_floats(&g.iter().map(|x| py_round(*x, 2)).collect::<Vec<_>>())
}

fn cfg_success(s: &mut Session) -> R<String> {
    s.need_cluster(ONOFF)?;
    let r = (|| {
        let p = configure(s, ONOFF, &cfg_record(0x0000, BOOL, 1, 300, &[]))?;
        expect(p == [0x00], format!("Configure Reporting Response payload {}; required: the single octet 00", hex(&p)))?;
        Ok(String::new())
    })();
    fin(r, quiet(s))
}

fn cfg_unsupported(s: &mut Session) -> R<String> {
    s.need_cluster(ONOFF)?;
    let p = configure(s, ONOFF, &cfg_record(ABSENT_ATTR, U8, 1, 300, &[0x01]))?;
    let mut want = vec![UNSUP_ATTR, 0x00];
    want.extend_from_slice(&ABSENT_ATTR.to_le_bytes());
    expect(
        p == want,
        format!(
            "Configure Reporting Response payload {}; required: status 0x86, direction 0x00, attribute {ABSENT_ATTR:#06x}",
            hex(&p)
        ),
    )?;
    Ok(String::new())
}

fn cfg_wrong_type(s: &mut Session) -> R<String> {
    s.need_cluster(ONOFF)?;
    let r = (|| {
        let p = configure(s, ONOFF, &cfg_record(0x0000, U8, 1, 300, &[0x01]))?;
        expect(
            p.len() == 4 && p[0] == INVALID_TYPE && p[1..] == [0, 0, 0],
            format!(
                "Configure Reporting Response payload {}; required: status 0x8d, direction 0x00, attribute 0x0000",
                hex(&p)
            ),
        )?;
        Ok(String::new())
    })();
    fin(r, quiet(s))
}

fn cfg_max_below_min(s: &mut Session) -> R<String> {
    s.need_cluster(ONOFF)?;
    let r = (|| {
        let p = configure(s, ONOFF, &cfg_record(0x0000, BOOL, 60, 10, &[]))?;
        expect(
            p.len() == 4 && p[0] == INVALID_VALUE && p[1..] == [0, 0, 0],
            format!(
                "Configure Reporting Response payload {} for minimum 60 s, maximum 10 s; required: status 0x87, \
                 direction 0x00, attribute 0x0000",
                hex(&p)
            ),
        )?;
        Ok(String::new())
    })();
    fin(r, quiet(s))
}

fn read_reporting_cfg(s: &mut Session) -> R<String> {
    s.need_cluster(ONOFF)?;
    let r = (|| {
        configured(s, &cfg_record(0x0000, BOOL, 2, 311, &[]))?;
        let f = gen(s, ONOFF, 0x08, &[0x00, 0x00, 0x00])?;
        expect(f.is_some(), "no response to Read Reporting Configuration")?;
        let z = f.as_ref().unwrap().zcl().unwrap();
        expect(!z.specific && z.cmd == 0x09, format!("answer to Read Reporting Configuration: {}", describe(&f)))?;
        let mut want = vec![SUCCESS, 0x00, 0x00, 0x00, BOOL];
        want.extend_from_slice(&2u16.to_le_bytes());
        want.extend_from_slice(&311u16.to_le_bytes());
        expect(
            z.payload == want,
            format!(
                "record {}; required {} (status, direction, attribute, type bool, minimum 2, maximum 311; no \
                 reportable change for a discrete type)",
                hex(&z.payload),
                hex(&want)
            ),
        )?;
        let mut q = vec![0x00];
        q.extend_from_slice(&ABSENT_ATTR.to_le_bytes());
        let f = gen(s, ONOFF, 0x08, &q)?;
        let z = f.as_ref().and_then(|f| f.zcl());
        expect(
            z.as_ref().is_some_and(|z| !z.specific && z.cmd == 0x09),
            format!("unsupported attribute: {}", describe(&f)),
        )?;
        let mut want = vec![UNSUP_ATTR, 0x00];
        want.extend_from_slice(&ABSENT_ATTR.to_le_bytes());
        let p = z.unwrap().payload;
        expect(p == want, format!("unsupported attribute: record {}; required {}", hex(&p), hex(&want)))?;
        Ok(String::new())
    })();
    fin(r, quiet(s))
}

fn report_on_change(s: &mut Session) -> R<String> {
    s.need_cluster(ONOFF)?;
    bind_onoff(s, false)?;
    let r = (|| {
        configured(s, &cfg_record(0x0000, BOOL, 0, 300, &[]))?;
        s.run(1500.0)?;
        let i = s.mark();
        set_onoff(s, false)?;
        let r = reports(s, 3000.0, Some(i))?;
        expect(
            !r.is_empty(),
            "no Report Attributes within 3 s of OnOff changing to 0 (binding to the coordinator, minimum 0 s)",
        )?;
        let f = &r[0];
        let (n, a, z) = (f.nwk.as_ref().unwrap(), f.aps.as_ref().unwrap(), f.zcl().unwrap());
        expect(
            n.dst == 0x0000 && a.dst_ep == Some(1) && a.src_ep == Some(1),
            format!(
                "report addressed to {:#06x} endpoint {}; the binding names 0x0000 endpoint 1",
                n.dst,
                opt_str(a.dst_ep)
            ),
        )?;
        expect(z.to_client, "direction bit of the report says client to server")?;
        expect(
            z.payload == [0x00, 0x00, BOOL, 0],
            format!("report payload {}; required attribute 0x0000, type 0x10, value 00", hex(&z.payload)),
        )?;
        Ok(String::new())
    })();
    fin(r, quiet(s))
}

fn report_periodic(s: &mut Session) -> R<String> {
    s.need_cluster(ONOFF)?;
    bind_onoff(s, false)?;
    let r = (|| {
        configured(s, &cfg_record(0x0000, BOOL, 0, 3, &[]))?;
        let r = reports(s, 13500.0, None)?;
        expect(r.len() >= 3, format!("{} reports in 13.5 s with a maximum interval of 3 s and no change", r.len()))?;
        let gaps = gaps_of(&r);
        let mx = gaps.iter().cloned().fold(f64::MIN, f64::max);
        expect(mx <= 3.4, format!("gaps between reports {} s; the maximum interval is 3 s", gaps_text(&gaps)))?;
        Ok(format!("gaps {} s", gaps_text(&gaps)))
    })();
    fin(r, quiet(s))
}

fn report_min_interval(s: &mut Session) -> R<String> {
    s.need_cluster(ONOFF)?;
    bind_onoff(s, false)?;
    let r = (|| {
        configured(s, &cfg_record(0x0000, BOOL, 5, 120, &[]))?;
        s.run(6000.0)?;
        let i = s.mark();
        for k in 0..4 {
            set_onoff(s, k % 2 == 1)?;
            s.run(750.0)?;
        }
        let r = reports(s, 9000.0, Some(i))?;
        expect(!r.is_empty(), "no report at all after four changes of OnOff (minimum 5 s, maximum 120 s)")?;
        let gaps = gaps_of(&r);
        expect(
            gaps.iter().all(|g| *g >= 4.7),
            format!("{} reports, gaps {} s; the minimum interval is 5 s", r.len(), gaps_text(&gaps)),
        )?;
        Ok(format!("{} reports for four changes, gaps {} s", r.len(), gaps_text(&gaps)))
    })();
    fin(r, quiet(s))
}

fn report_max_zero(s: &mut Session) -> R<String> {
    s.need_cluster(ONOFF)?;
    bind_onoff(s, false)?;
    let r = (|| {
        configured(s, &cfg_record(0x0000, BOOL, 0, 0, &[]))?;
        s.run(1500.0)?;
        let i = s.mark();
        set_onoff(s, false)?;
        let r = reports(s, 3000.0, Some(i))?;
        expect(!r.is_empty(), "no report of a change with maximum interval 0x0000; change-based reporting must still work")?;
        let i = s.mark();
        let r = reports(s, 12000.0, Some(i))?;
        expect(
            r.is_empty(),
            format!("{} reports in 12 s without any change; maximum interval 0x0000 means no periodic reports", r.len()),
        )?;
        Ok(String::new())
    })();
    fin(r, quiet(s))
}

fn report_off(s: &mut Session) -> R<String> {
    s.need_cluster(ONOFF)?;
    bind_onoff(s, false)?;
    let r = (|| {
        configured(s, &cfg_record(0x0000, BOOL, 0, 300, &[]))?;
        s.run(1500.0)?;
        let i = s.mark();
        set_onoff(s, false)?;
        if reports(s, 3000.0, Some(i))?.is_empty() {
            return Err(Stop::Inconclusive("control failed: no report even with reporting enabled".into()));
        }
        configured(s, &cfg_record(0x0000, BOOL, 0, 0xFFFF, &[]))?;
        s.run(500.0)?;
        let i = s.mark();
        set_onoff(s, true)?;
        s.run(1500.0)?;
        set_onoff(s, false)?;
        let r = reports(s, 5000.0, Some(i))?;
        let pls: Vec<String> = r.iter().map(|x| hex(&x.zcl().unwrap().payload)).collect();
        expect(
            r.is_empty(),
            format!("{} reports after the maximum interval was set to 0xffff: {}", r.len(), py_strs(&pls)),
        )?;
        Ok(String::new())
    })();
    fin(r, quiet(s))
}

fn report_without_binding(s: &mut Session) -> R<String> {
    s.need_cluster(ONOFF)?;
    bind_onoff(s, false)?;
    let r = (|| {
        configured(s, &cfg_record(0x0000, BOOL, 0, 4, &[]))?;
        s.run(1500.0)?;
        let i = s.mark();
        set_onoff(s, false)?;
        if reports(s, 3000.0, Some(i))?.is_empty() {
            return Err(Stop::Inconclusive("control failed: no report even with a binding".into()));
        }
        bind_onoff(s, true)?;
        let f = s.zdo(0x0033, &[0x00], 2000.0)?;
        if let Some(f) = &f {
            let p = f.zdo().unwrap().payload;
            if p.first() == Some(&0x00) && p.get(1).is_some_and(|x| *x != 0) {
                let mut k = 4;
                let mut left = Vec::new();
                let n = p.get(3).copied().ok_or_else(|| Stop::Error("struct.error".into()))?;
                for _ in 0..n {
                    let (Some(lo), Some(hi), Some(mode)) = (p.get(k + 9), p.get(k + 10), p.get(k + 11)) else {
                        return Err(Stop::Error("struct.error: unpack_from requires a buffer".into()));
                    };
                    left.push(u16::from_le_bytes([*lo, *hi]));
                    k += if *mode == 1 { 14 } else { 21 };
                }
                if left.contains(&ONOFF) {
                    return Err(Stop::Inconclusive("an On/Off binding remains after Unbind_req".into()));
                }
            }
        }
        s.run(500.0)?;
        let i = s.mark();
        set_onoff(s, true)?;
        s.run(1500.0)?;
        set_onoff(s, false)?;
        let r = reports(s, 9000.0, Some(i))?;
        let mut dsts: Vec<String> = r.iter().map(|x| py_hex(x.nwk.as_ref().unwrap().dst as u64)).collect();
        dsts.sort();
        dsts.dedup();
        expect(
            r.is_empty(),
            format!("{} Report Attributes commands without any On/Off binding, sent to {}", r.len(), py_strs(&dsts)),
        )?;
        Ok(String::new())
    })();
    fin(r, quiet(s))
}
