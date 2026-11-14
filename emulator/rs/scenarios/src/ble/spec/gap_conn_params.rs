//! GAP: when and how often a peripheral asks for new connection parameters
//! (Core 5.4 Vol 3 Part C 9.3.9, 9.3.12, Appendix A); emulator/spec_ble/gap_conn_params.py.

use crate::ble::harness::{BleSession, LinkOpts, ParamReq};
use crate::ble::{BleTest, Level};
use crate::py;
use crate::session::{expect, Stop, R};

const DOC: &str = "Core 5.4 Vol 3 Part C";

pub fn tests() -> Vec<BleTest> {
    vec![
        BleTest {
            id: "gap_param_update_pause",
            doc: DOC,
            clause: "9.3.12.2, Appendix A",
            req: "The peripheral does not ask for new connection parameters within T_GAP(conn_pause_peripheral) = 5 s \
                  of the connection being established.",
            run: param_update_pause,
            mandatory: false,
            level: Level::Should,
            ..BleTest::D
        },
        BleTest {
            id: "gap_param_update_no_repeat",
            doc: DOC,
            clause: "9.3.9.2, Appendix A",
            req: "After a Connection Parameter Update Response the peripheral sends no new request within \
                  T_GAP(conn_param_timeout) = 30 s.",
            run: param_update_no_repeat,
            mandatory: false,
            ..BleTest::D
        },
    ]
}

fn first_param_req(s: &mut BleSession) -> R<()> {
    s.link_until(12000.0, Some(20.0), |s| Ok(if s.lk().param_reqs.is_empty() { None } else { Some(()) }))?;
    if s.lk().param_reqs.is_empty() {
        return Err(Stop::NotApplicable("the bulb sent no Connection Parameter Update Request in 12 s".into()));
    }
    Ok(())
}

fn param_update_pause(s: &mut BleSession) -> R<String> {
    // 9.3.12.2: "The Peripheral should not perform a Connection Parameter Update procedure within
    // T_GAP(conn_pause_peripheral) after establishing a connection." Appendix A: 5 s (recommended value).
    s.connect(LinkOpts::new().set("interval", 24).set("timeout", 400).param_req(ParamReq::Reject))?;
    first_param_req(s)?;
    let dt = s.lk().param_reqs[0].t - s.lk().pdus[0].t_end;
    expect(dt >= 5000.0, format!("request {:.2} s after the first packet of the connection", dt / 1000.0))?;
    Ok(format!("request after {:.2} s", dt / 1000.0))
}

fn param_update_no_repeat(s: &mut BleSession) -> R<String> {
    // 9.3.9.2: "The Peripheral shall not send an L2CAP_CONNECTION_PARAMETER_UPDATE_REQ command within
    // T_GAP(conn_param_timeout) of an L2CAP_CONNECTION_PARAMETER_UPDATE_RSP being received." Appendix A:
    // 30 s. The central rejects the request (result 0x0001) and keeps the connection for 32 s more.
    s.connect(LinkOpts::new().set("interval", 24).set("timeout", 600).param_req(ParamReq::Reject))?;
    first_param_req(s)?;
    let t0 = s.lk().param_reqs[0].t;
    s.link_until(31000.0, Some(50.0), |s| Ok(if s.lk().param_reqs.len() > 1 { Some(()) } else { None }))?;
    let more: Vec<f64> = s.lk().param_reqs[1..].iter().map(|f| f.t - t0).collect();
    expect(
        !more.iter().any(|x| *x < 30000.0),
        format!("new requests {} s after the rejected one", py::floats(more.iter().map(|x| py::round(x / 1000.0, 1)))),
    )?;
    let mut ids: Vec<i64> = s.lk().param_reqs.iter().map(|f| f.data[1] as i64).collect();
    ids.sort();
    ids.dedup();
    let extra = if ids.len() > 1 { format!("; identifiers used {}", py::ints(ids)) } else { String::new() };
    Ok(format!("no new request in the 30 s after the response{extra}"))
}
