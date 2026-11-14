//! Joining and what follows it (BDB 3.0.1 8.2, 10.2.5; R22 2.4.3.1.11,
//! 3.6.1.4.1), ported from emulator/spec/bdb_join.py. The first tests only read
//! what the bulb sent while the session's Trust Center admitted it.

use super::misc_ota_gp::py_list;
use super::{SessionDef, Test};
use crate::air::NodeId;
use crate::coord::Coord;
use crate::crypto::hmac_mmo;
use crate::session::{expect, is_aps_cmd, is_mac_cmd, is_zdo, CapFrame, Session, Stop, R};

const S: &str = "bdb";
const PRIMARY: [i64; 4] = [11, 15, 20, 25];
const SETUP: SessionDef = SessionDef { baseline: false, ..SessionDef::D };

fn closed(c: &mut Coord) {
    c.permit_join = Some(false);
}

fn silent(c: &mut Coord) {
    c.permit_join = None;
}

pub fn tests() -> Vec<Test> {
    vec![
        Test {
            id: "BDB-JOIN-04",
            doc: "BDB 3.0.1",
            clause: "8.2 step 15",
            req: "A router that has joined activates its own permit joining flag for at least \
                  bdbcMinCommissioningTime (180 s): its beacon offers association right after the join and still \
                  does 100 s later.",
            session: S,
            run: own_permit_join,
            setup: SETUP,
            ..Test::D
        },
        Test {
            id: "BDB-JOIN-01",
            doc: "BDB 3.0.1 / R22",
            clause: "8.2 step 11; 2.4.3.1.11; Table 3-62",
            req: "After receiving the network key the node broadcasts Device_annce to 0xFFFD with its network \
                  address, IEEE address and the capability information it associated with (router: FFD, receiver \
                  on when idle, allocate address = 1).",
            session: S,
            run: device_annce,
            setup: SETUP,
            ..Test::D
        },
        Test {
            id: "BDB-JOIN-02",
            doc: "BDB 3.0.1",
            clause: "10.2.5 steps 3, 7, 11; 8.2 step 12",
            req: "On a centralized network whose Trust Center reports revision 21 or later, the joined node asks for \
                  the Trust Center's node descriptor, requests a new link key (Request Key, key type 4, \
                  APS-encrypted with the initial link key) and verifies the key it gets (Verify Key with the hash \
                  of the new key).",
            session: S,
            run: tclk_exchange,
            setup: SETUP,
            ..Test::D
        },
        Test {
            id: "BDB-JOIN-03",
            doc: "BDB 3.0.1 / R22",
            clause: "8.2 step 14; 2.4.3.3.7.1",
            req: "After joining (and the link key exchange) the node broadcasts Mgmt_Permit_Joining_req with \
                  PermitDuration of at least 180 s and TC_Significance 1; the broadcast addressing of this request \
                  is 'all routers and coordinator' (0xFFFC).",
            session: S,
            run: permit_join_broadcast,
            setup: SETUP,
            ..Test::D
        },
        Test {
            id: "BDB-JOIN-05",
            doc: "BDB 3.0.1",
            clause: "8.2 step 15, 5.1.2",
            req: "The permit joining flag activated after the join stays on for bdbcMinCommissioningTime: 100 s after \
                  the join the beacon still offers association.",
            session: S,
            run: own_permit_join_lasts,
            setup: SETUP,
            ..Test::D
        },
        Test {
            id: "BDB-JOIN-06",
            doc: "BDB 3.0.1",
            clause: "8.2 steps 5-7",
            req: "A node that is not on a network joins only networks whose beacon has permit joining set: it does \
                  not try to associate with a closed network, and joins once the network opens.",
            session: "",
            run: closed_network,
            setup: SessionDef { join: false, tc: Some(closed), ..SessionDef::D },
            ..Test::D
        },
        Test {
            id: "BDB-JOIN-07",
            doc: "BDB 3.0.1",
            clause: "8.2 steps 2-3, 13; 5.3.8",
            req: "Network steering scans the primary channel set (default 11, 15, 20, 25) first and the secondary \
                  set only after that.",
            session: "",
            run: scan_order,
            setup: SessionDef { join: false, tc: Some(silent), ..SessionDef::D },
            ambiguous: Some(
                "bdbPrimaryChannelSet is an attribute whose default is 0x02108800; an application may configure \
                 another set. The test checks the default",
            ),
            ..Test::D
        },
    ]
}

fn join_frames(s: &Session, pred: impl Fn(&CapFrame) -> bool) -> Vec<CapFrame> {
    s.cap[..s.join_mark.min(s.cap.len())].iter().filter(|f| f.who == NodeId::Dut && pred(f)).cloned().collect()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn own_permit_join(s: &mut Session) -> R<String> {
    let Some(b) = s.beacon()? else {
        return Err(Stop::Inconclusive("the bulb sent no beacon in answer to a Beacon Request".into()));
    };
    let t = (s.now() - s.t_joined) / 1000.0;
    expect(b.permit, format!("association permit is 0 in the bulb's beacon {t:.1} s after it joined"))?;
    let now = s.now();
    s.set_note("permit_checked", now);
    expect(
        b.router_cap.unwrap_or(false) || b.ed_cap.unwrap_or(false),
        "the beacon announces neither router nor end device capacity",
    )?;
    Ok(format!("association permit 1 at {t:.1} s after the join"))
}

fn device_annce(s: &mut Session) -> R<String> {
    let assoc = join_frames(s, is_mac_cmd(0x01));
    expect(!assoc.is_empty(), "no Association Request in the capture of the join")?;
    let cap = assoc.last().unwrap().mac.as_ref().unwrap().payload[1];
    expect(
        cap & 0x02 != 0 && cap & 0x08 != 0 && cap & 0x80 != 0,
        format!(
            "capability information {cap:#04x} in the Association Request: device type (bit 1), receiver on when \
             idle (bit 3) and allocate address (bit 7) must be 1"
        ),
    )?;
    let an = join_frames(s, is_zdo(0x0013, None));
    expect(!an.is_empty(), "no Device_annce after the network key arrived")?;
    let f = &an[0];
    let dst = f.nwk.as_ref().unwrap().dst;
    expect(dst == 0xFFFD, format!("Device_annce broadcast to {dst:#06x}; it goes to 0xFFFD"))?;
    let p = f.zdo().unwrap().payload;
    let short = u16::from_le_bytes([p[0], p[1]]);
    let ext = u64::from_le_bytes(p[2..10].try_into().unwrap());
    let c2 = p[10];
    expect((short, ext) == (s.dut_short, s.dut_ext), format!("announced {short:#06x} / {ext:#018x}"))?;
    expect(c2 == cap, format!("capability {c2:#04x} in Device_annce, {cap:#04x} in the Association Request"))?;
    Ok(format!("capability {cap:#04x}"))
}

fn tclk_exchange(s: &mut Session) -> R<String> {
    let nd = join_frames(s, is_zdo(0x0002, None));
    let rk = join_frames(s, is_aps_cmd(0x08));
    let vk = join_frames(s, is_aps_cmd(0x0F));
    expect(!nd.is_empty() && nd[0].nwk.as_ref().unwrap().dst == 0, "no Node_Desc_req to the Trust Center after joining")?;
    expect(!rk.is_empty(), "no Request Key after the Trust Center's Node_Desc_rsp (stack revision 22)")?;
    let a = rk[0].aps.as_ref().unwrap();
    expect(
        a.secured && a.key_id == Some(0) && a.payload.is_some(),
        "the Request Key is not APS-encrypted with a link key (data key)",
    )?;
    let kt = a.payload.as_ref().unwrap()[1];
    expect(kt == 0x04, format!("requested key type {kt:#04x}; Trust Center link key is 0x04"))?;
    expect(nd[0].i < rk[0].i, "Request Key was sent before the Node_Desc_req")?;
    expect(!vk.is_empty(), "no Verify Key after the Transport Key with the new link key")?;
    let p = vk[0].aps.as_ref().unwrap().payload.clone().unwrap_or_default();
    let dut_ext = s.dut_ext;
    let Some(dev) = s.w.coord.devices.iter().find(|d| d.ext == dut_ext) else {
        return Err(Stop::Error(format!("KeyError: {dut_ext}")));
    };
    let newk = dev.tclk_new;
    expect(
        p[1] == 0x04 && u64::from_le_bytes(p[2..10].try_into().unwrap()) == dut_ext,
        format!("Verify Key: key type {:#04x}, source address {}", p[1], hex(&p[2..10])),
    )?;
    expect(
        newk.is_some_and(|k| p.get(10..26) == Some(&hmac_mmo(&k, &[0x03])[..])),
        "the hash in the Verify Key is not that of the link key the Trust Center sent",
    )?;
    expect(!vk[0].aps.as_ref().unwrap().secured, "the Verify Key is APS-encrypted (R22 Table 4-7: shall not be)")?;
    Ok(String::new())
}

fn is_pj(f: &CapFrame) -> bool {
    f.zdo().is_some_and(|z| z.cluster == 0x0036)
}

fn permit_join_broadcast(s: &mut Session) -> R<String> {
    let mut pj = join_frames(s, is_zdo(0x0036, None));
    let jm = s.join_mark;
    pj.extend(s.dut_frames(jm, is_pj));
    if pj.is_empty() {
        s.run(3000.0)?;
        pj = s.dut_frames(jm, is_pj);
    }
    expect(!pj.is_empty(), "no Mgmt_Permit_Joining_req broadcast by the bulb after it joined")?;
    let f = &pj[0];
    let z = f.zdo().unwrap();
    let (dur, sig) = (z.payload[0], z.payload[1]);
    let dst = f.nwk.as_ref().unwrap().dst;
    expect(dst >= 0xFFF8, format!("sent to {dst:#06x}, not broadcast"))?;
    expect(dur >= 180, format!("PermitDuration {dur} s; at least bdbcMinCommissioningTime = 180 s"))?;
    expect(sig == 1, format!("TC_Significance {sig}"))?;
    expect(
        dst == 0xFFFC,
        format!(
            "PermitDuration {dur} s and TC_Significance 1 are right, but the request is broadcast to {dst:#06x}; \
             R22 2.4.3.3.7.1 names 'all routers and coordinator' (0xFFFC)"
        ),
    )?;
    let an = join_frames(s, is_zdo(0x0013, None));
    expect(!an.is_empty() && an[0].i < f.i, "the broadcast came before the Device_annce")?;
    Ok(format!("PermitDuration {dur} s"))
}

fn own_permit_join_lasts(s: &mut Session) -> R<String> {
    if s.note::<f64>("permit_checked").is_none() {
        return Err(Stop::Inconclusive("permit joining was not on right after the join (see BDB-JOIN-04)".into()));
    }
    let left = 100000.0 - (s.now() - s.t_joined);
    if left > 0.0 {
        s.run(left)?;
    }
    let Some(b) = s.beacon()? else {
        return Err(Stop::Inconclusive("the bulb sent no beacon in answer to a Beacon Request".into()));
    };
    expect(
        b.permit,
        format!("association permit is 0 in the beacon {:.0} s after the join", (s.now() - s.t_joined) / 1000.0),
    )?;
    Ok(String::new())
}

fn closed_network(s: &mut Session) -> R<String> {
    s.run(20000.0)?;
    let beacons = s.frames(0, Some(NodeId::Coord), |f| f.mac.as_ref().is_some_and(|m| m.ty == 0));
    let assoc = s.dut_frames(0, is_mac_cmd(0x01));
    if beacons.is_empty() {
        return Err(Stop::Inconclusive(
            "the bulb sent no Beacon Request in 20 s, so the closed network was never offered".into(),
        ));
    }
    expect(
        assoc.is_empty(),
        format!(
            "{} Association Request(s) to a network whose beacons say association is not permitted ({} beacons heard)",
            assoc.len(),
            beacons.len()
        ),
    )?;
    s.w.coord.permit_join = Some(true);
    let ok = s.run_until(90000.0, 20.0, |s| s.w.coord.devices.iter().any(|d| d.announced))?;
    expect(ok, "the bulb did not join within 90 s after the network opened")?;
    Ok(format!("{} beacons of the closed network ignored, joined after it opened", beacons.len()))
}

fn scan_order(s: &mut Session) -> R<String> {
    s.run(45000.0)?;
    let chs: Vec<i64> = s.dut_frames(0, is_mac_cmd(0x07)).iter().map(|f| f.ch).collect();
    if chs.is_empty() {
        return Err(Stop::Inconclusive(
            "no Beacon Request in 45 s: the bulb does not start network steering by itself".into(),
        ));
    }
    let first_other = chs.iter().position(|c| !PRIMARY.contains(c)).unwrap_or(chs.len());
    let head = &chs[..first_other];
    let mut seq: Vec<i64> = Vec::new();
    for c in &chs {
        if seq.last() != Some(c) {
            seq.push(*c);
        }
    }
    let covers = PRIMARY.iter().all(|p| head.contains(p)) && head.iter().all(|c| PRIMARY.contains(c));
    expect(
        covers,
        format!(
            "channels of the Beacon Requests in order: {}; the scan must cover 11, 15, 20, 25 before any other channel",
            py_list(&seq[..seq.len().min(24)])
        ),
    )?;
    Ok(format!("channel order {}", py_list(&seq[..seq.len().min(20)])))
}
