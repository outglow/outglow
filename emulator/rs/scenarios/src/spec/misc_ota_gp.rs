//! Green Power proxy and OTA client: only what a clause states crisply (GP
//! Basic 1.1.2 A.3.2, ZCL8 11.13.3.4), ported from emulator/spec/misc_ota_gp.py.

use super::Test;
use crate::session::{expect, Session, Stop, R};
use crate::station::Tx;

const S: &str = "misc";

pub fn tests() -> Vec<Test> {
    vec![
        Test {
            id: "GP-EP-01",
            doc: "GP Basic 1.1.2",
            clause: "A.3.2 (Table 13), A.3.2.6, A.1.4",
            req: "A Zigbee 3.0 router implements the Green Power Basic Proxy on the reserved endpoint 242: device id \
                  0x0061 with the client side of the Green Power cluster (0x0021).",
            session: S,
            run: gp_endpoint,
            ..Test::D
        },
        Test {
            id: "OTA-NOTIFY-01",
            doc: "ZCL8",
            clause: "11.13.3.4",
            req: "On a unicast Image Notify an OTA Upgrade client always sends a Query Next Image Request to the \
                  upgrade server.",
            session: S,
            run: ota_image_notify,
            mandatory: false,
            ..Test::D
        },
    ]
}

/// Python's repr of a list of ints.
pub fn py_list<T: std::fmt::Display>(v: &[T]) -> String {
    format!("[{}]", v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(", "))
}

/// Python's repr of `[hex(c) for c in v]`.
pub fn py_hex_list(v: &[u16]) -> String {
    format!("[{}]", v.iter().map(|x| format!("'{x:#x}'")).collect::<Vec<_>>().join(", "))
}

fn gp_endpoint(s: &mut Session) -> R<String> {
    let eps = s.endpoints()?;
    expect(eps.contains(&242), format!("active endpoints {}: the Green Power endpoint 242 is missing", py_list(&eps)))?;
    let sd = s.simple_desc(242)?;
    expect(sd.is_some(), "no simple descriptor for endpoint 242")?;
    let sd = sd.unwrap();
    expect(sd.device == 0x0061, format!("device id {:#06x} on endpoint 242; GP Proxy Basic is 0x0061", sd.device))?;
    expect(
        sd.outc.contains(&0x0021),
        format!(
            "output (client) clusters {} lack the Green Power cluster 0x0021 (input clusters: {})",
            py_hex_list(&sd.outc),
            py_hex_list(&sd.inc)
        ),
    )?;
    Ok(format!("profile {:#06x}, device {:#06x}", sd.profile, sd.device))
}

fn ota_image_notify(s: &mut Session) -> R<String> {
    let mut ep = None;
    for e in s.endpoints()? {
        if s.simple_desc(e)?.is_some_and(|d| d.outc.contains(&0x0019)) {
            ep = Some(e);
            break;
        }
    }
    let Some(ep) = ep else {
        return Err(Stop::NotApplicable("no endpoint lists the OTA Upgrade cluster (0x0019) as a client cluster".into()));
    };
    let i = s.mark();
    s.tx_zcl(0x0019, 0x00, &[0x00, 100], Tx::new().ep(ep).ddr().to_client());
    let f = s.wait_dut(
        |f| {
            f.zcl().is_some_and(|z| z.specific && z.cmd == 0x01 && !z.to_client)
                && f.aps.as_ref().and_then(|a| a.cluster) == Some(0x0019)
        },
        5000.0,
        Some(i),
    )?;
    expect(
        f.is_some(),
        "no Query Next Image Request within 5 s of a unicast Image Notify (payload type 0, query jitter 100)",
    )?;
    let f = f.unwrap();
    let p = f.zcl().unwrap().payload;
    let hex: String = p.iter().map(|x| format!("{x:02x}")).collect();
    expect(p.len() >= 9, format!("Query Next Image Request payload {hex} is shorter than its mandatory fields"))?;
    let mfg = u16::from_le_bytes([p[1], p[2]]);
    let itype = u16::from_le_bytes([p[3], p[4]]);
    let ver = u32::from_le_bytes([p[5], p[6], p[7], p[8]]);
    let dst = f.nwk.as_ref().unwrap().dst;
    expect(dst == 0x0000, format!("sent to {dst:#06x}, the Image Notify came from 0x0000"))?;
    Ok(format!("manufacturer {mfg:#06x}, image type {itype:#06x}, file version {ver:#010x}"))
}
