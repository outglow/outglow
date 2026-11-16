//! Events that change the bulb's place in the network; each test has a session
//! of its own: leaving (R22 3.6.1.10, 2.4.3.3.5, BDB 9.3/9.4), rejoining
//! (3.6.1.4.2, 3.4.6/3.4.7), address conflict (3.6.1.9), PAN identifier update
//! (3.6.1.13.3), channel change (2.4.3.3.9). Ported from emulator/spec/nwk_network.py.

use super::{SessionDef, Test};
use crate::air::{Air, NodeId};
use crate::coord::Coord;
use crate::crypto::hmac_mmo;
use crate::frames::ext_bytes;
use crate::session::{expect, is_mac_cmd, is_nwk_cmd, CapFrame, Session, Stop, R};
use crate::station::{SrcIeee, Tx};

const LR: &str = "leave-rejoin";
const LR_SETUP: SessionDef = SessionDef { baseline: false, alive_check: false, ..SessionDef::D };
const DUT: Option<NodeId> = Some(NodeId::Dut);

pub fn tests() -> Vec<Test> {
    vec![
        Test {
            id: "NWK-LEAVE-01",
            doc: "R22",
            clause: "3.6.1.10.3.1, 3.6.1.10.1, 3.4.4, 3.6.1.10.4",
            req: "A router that receives a unicast NWK Leave with request = 1 and rejoin = 0 announces its own leave \
                  (MAC broadcast, NWK destination 0xFFFD, radius 1, request = 0, rejoin = 0, source IEEE address) and \
                  is then no longer on the network.",
            run: leave_request,
            ..Test::D
        },
        Test {
            id: "NWK-LEAVE-02",
            doc: "R22",
            clause: "3.6.1.10.3.1, 3.6.1.10.1",
            req: "A router that receives a unicast NWK Leave with request = 1 and rejoin = 1 announces its leave with \
                  request = 0 and the rejoin flag copied (1).",
            session: LR,
            setup: LR_SETUP,
            run: leave_request_rejoin,
            ..Test::D
        },
        Test {
            id: "NWK-REJOIN-01",
            doc: "R22",
            clause: "3.6.1.10.4 step 1, 3.6.1.4.2, 3.4.6, 3.4.7",
            req: "A device that left with rejoin = 1 may rejoin: its Rejoin Request goes to the prospective parent \
                  with radius 1, its IEEE address in the NWK header and its capability information; after a \
                  successful Rejoin Response it operates on the network again under the address the response carries.",
            session: LR,
            setup: LR_SETUP,
            mandatory: false,
            run: rejoin_after_leave,
            ..Test::D
        },
        Test {
            id: "NWK-REJOIN-02",
            doc: "R22",
            clause: "4.6.3.3.2, 4.7.3.10.2, 3.4.6, 3.6.2.2",
            req: "A device that missed a network key update can get back by a Trust Center rejoin: an unsecured \
                  Rejoin Request, then the unsecured Rejoin Response and the active network key under its link key \
                  bring it back onto the network.",
            mandatory: false,
            ambiguous: Some(
                "no clause obliges a router to notice by itself that the network moved to a key it does not have; \
                 INCONCLUSIVE when the bulb never tries",
            ),
            run: tc_rejoin_after_missed_key,
            ..Test::D
        },
        Test {
            id: "NWK-LEAVE-03",
            doc: "R22",
            clause: "3.6.1.10.3.1 step 1",
            req: "A NWK Leave with request = 1 that was sent to a broadcast address is dropped.",
            run: leave_request_broadcast,
            ..Test::D
        },
        Test {
            id: "ZDO-LEAVE-01",
            doc: "R22 / BDB 3.0.1",
            clause: "2.4.3.3.5.2; BDB 6.6, 9.4",
            req: "A unicast Mgmt_Leave_req naming the device itself is answered with Mgmt_Leave_rsp SUCCESS before \
                  the device leaves; the device then announces its leave (request = 0, rejoin = 0) and is no longer \
                  on the network.",
            run: mgmt_leave,
            ..Test::D
        },
        Test {
            id: "ZDO-LEAVE-02",
            doc: "R22",
            clause: "3.6.1.10.3.1 step 1, 2.4.3.3.5.2",
            req: "A Mgmt_Leave_req that was sent to a broadcast address is dropped: no response, no leave.",
            run: mgmt_leave_broadcast,
            ..Test::D
        },
        Test {
            id: "ZDO-LEAVE-03",
            doc: "R22",
            clause: "2.4.3.3.5.2",
            req: "A Mgmt_Leave_req naming a device that is neither the router itself nor one of its children does \
                  not make the router leave; the response carries an error status.",
            run: mgmt_leave_other,
            ..Test::D
        },
        Test {
            id: "NWK-CONFLICT-01",
            doc: "R22",
            clause: "3.6.1.9.2, 3.6.1.9.3",
            req: "A frame addressed to the device's network address whose destination IEEE address is not the \
                  device's own reveals a local address conflict: the router broadcasts a Network Status 0x0d to \
                  0xFFFD with its previous address, picks a new address and announces it.",
            mandatory: false,
            ambiguous: Some(
                "address conflict detection is enabled when nwkUniqueAddr is FALSE; R22 Table 3-58 gives TRUE as the \
                 default and the Zigbee PRO stack profile (not among the suite's documents) sets FALSE",
            ),
            run: address_conflict,
            ..Test::D
        },
        Test {
            id: "NWK-PANID-01",
            doc: "R22",
            clause: "3.6.1.13.3, 3.4.10.1",
            req: "On a Network Update command (PAN identifier update) the device starts a timer of \
                  nwkNetworkBroadcastDeliveryTime and changes its PAN identifier only when it expires; the copies of \
                  the command it relays still go out under the old PAN identifier.",
            ambiguous: Some(
                "the value of nwkNetworkBroadcastDeliveryTime follows from stack profile parameters (commonly 9 s); \
                 the test only requires that the old PAN identifier is still in use 1 s after the command and that \
                 the new one is in use within 60 s",
            ),
            run: pan_id_update,
            ..Test::D
        },
        Test {
            id: "ZDO-CHAN-01",
            doc: "R22",
            clause: "2.4.3.3.9.2 step 3",
            req: "A Mgmt_NWK_Update_req with ScanDuration 0xFE and a single supported channel makes the device \
                  change to that channel.",
            mandatory: false,
            run: channel_change,
            ..Test::D
        },
    ]
}

fn tc(s: &mut Session) -> (&mut Coord, &mut Air) {
    let w = &mut s.w;
    (&mut w.coord, &mut w.air)
}

fn py_opt_u64(v: Option<u64>) -> String {
    v.map(|x| x.to_string()).unwrap_or_else(|| "None".into())
}

fn check_leave_frame(s: &Session, f: &CapFrame, rejoin: bool) -> R<()> {
    let n = f.nwk.as_ref().unwrap();
    let m = f.mac.as_ref().unwrap();
    let o = n.payload.as_ref().unwrap()[1];
    expect(o & 0x40 == 0, format!("Leave options {o:#04x}: request must be 0 in the device's own announcement"))?;
    expect((o & 0x20 != 0) == rejoin, format!("Leave options {o:#04x}: rejoin must be {}", rejoin as u8))?;
    expect(
        m.dst == Some(0xFFFF),
        format!("MAC destination {:#06x}; the announcement is a MAC broadcast", m.dst.unwrap_or(0)),
    )?;
    expect(n.dst == 0xFFFD, format!("NWK destination {:#06x}; with request = 0 it is 0xFFFD", n.dst))?;
    expect(n.radius == 1, format!("radius {}", n.radius))?;
    expect(n.src_ieee == Some(s.dut_ext), "no source IEEE address in the NWK header of the Leave")?;
    expect(n.secured, "the Leave is not secured with the network key (4.6.3.6.3)")
}

fn ignored_or_gone(s: &mut Session) -> R<String> {
    if s.alive()? {
        return Err(Stop::Inconclusive(
            "the Leave request was ignored; 3.6.1.10.3.1 step 2c allows that when nwkLeaveRequestAllowed is FALSE"
                .into(),
        ));
    }
    Err(Stop::Fail("the bulb stopped answering without announcing its leave".into()))
}

fn leave_request(s: &mut Session) -> R<String> {
    s.w.coord.permit_join = None; // no beacons: the bulb cannot come back unnoticed
    let i = s.mark();
    let (dut, ext) = (s.dut_short, s.dut_ext);
    let (c, air) = tc(s);
    c.tx_nwk_cmd(air, dut, &[0x04, 0x40], &Tx::new().radius(1).dst_ieee(ext));
    s.run(3000.0)?;
    let lv = s.frames(i, DUT, is_nwk_cmd(0x04));
    if lv.is_empty() {
        return ignored_or_gone(s);
    }
    check_leave_frame(s, &lv[0], false)?;
    expect(!s.alive()?, "the bulb still answers at its address after announcing a leave without rejoin")?;
    Ok(String::new())
}

fn leave_request_rejoin(s: &mut Session) -> R<String> {
    let i = s.mark();
    let (dut, ext) = (s.dut_short, s.dut_ext);
    let (c, air) = tc(s);
    c.tx_nwk_cmd(air, dut, &[0x04, 0x60], &Tx::new().radius(1).dst_ieee(ext));
    let f = s.wait(is_nwk_cmd(0x04), 3000.0, Some(i), DUT, 5.0)?;
    s.set_note("leave_mark", i);
    let Some(f) = f else { return ignored_or_gone(s) };
    s.set_note("left", true);
    check_leave_frame(s, &f, true)?;
    Ok(String::new())
}

fn rejoin_after_leave(s: &mut Session) -> R<String> {
    if !s.has_note("leave_mark") {
        // run alone (--exact): the leave with rejoin of NWK-LEAVE-02 first, judged there
        if let Err(e @ (Stop::Error(_) | Stop::SessionLost(_))) = leave_request_rejoin(s) {
            return Err(e);
        }
    }
    if !s.note::<bool>("left").unwrap_or(false) {
        return Err(Stop::Inconclusive("the bulb did not announce a leave with rejoin in the test before this one".into()));
    }
    let i: usize = s.note("leave_mark").unwrap();
    let Some(f) = s.wait(is_nwk_cmd(0x06), 25000.0, Some(i), DUT, 5.0)? else {
        let again = s.frames(i, DUT, is_mac_cmd(0x01));
        return Err(Stop::Inconclusive(format!(
            "no Rejoin Request within 25 s of the leave (the rejoin is optional: 'may'){}",
            if again.is_empty() { "" } else { "; the bulb started a new MAC association instead" }
        )));
    };
    let n = f.nwk.clone().unwrap();
    let m = f.mac.as_ref().unwrap();
    let cap = n.payload.as_ref().unwrap()[1];
    expect(n.dst == 0x0000 && m.dst == Some(0x0000), format!("Rejoin Request sent to {:#06x}", n.dst))?;
    expect(n.radius == 1, format!("radius {}", n.radius))?;
    expect(n.src_ieee == Some(s.dut_ext), "no source IEEE address in the NWK header of the Rejoin Request")?;
    expect(cap & 0x02 != 0 && cap & 0x08 != 0, format!("capability information {cap:#04x} of a router"))?;
    // the parent's answer (3.4.7): same security level as the request, both IEEE addresses, radius 1
    let short = n.src;
    let ext = s.dut_ext;
    let lk = s.tclk();
    let (c, air) = tc(s);
    let mut o = Tx::new().radius(1).dst_ieee(ext);
    o.insecure = !n.secured;
    let mut rsp = vec![0x07];
    rsp.extend_from_slice(&short.to_le_bytes());
    rsp.push(0x00);
    c.tx_nwk_cmd(air, short, &rsp, &o);
    if !n.secured {
        // Trust Center rejoin: the active network key, APS-encrypted with the device's link key
        let mut body = vec![0x05, 0x01];
        body.extend_from_slice(&c.st.nwk_key);
        body.push(c.st.key_seq);
        body.extend_from_slice(&ext_bytes(ext));
        body.extend_from_slice(&ext_bytes(c.ext));
        let aps = c.aps_cmd_secured(&body, &hmac_mmo(&lk, &[0x00]), 2);
        c.nwk_send(air, short, &aps, 0, false, 1, None, false);
    }
    s.run(3000.0)?;
    s.dut_short = short;
    expect(s.alive()?, "after the Rejoin Response (status 0) the bulb does not answer at its address")?;
    let annce = s.frames(f.i, DUT, |g| g.zdo().is_some_and(|z| z.cluster == 0x0013));
    Ok(format!(
        "{} Rejoin Request{}",
        if n.secured { "secured" } else { "unsecured (Trust Center rejoin)" },
        if annce.is_empty() { "; no Device_annce after the rejoin" } else { "; Device_annce after the rejoin" }
    ))
}

fn tc_rejoin_after_missed_key(s: &mut Session) -> R<String> {
    let new_key: [u8; 16] = [
        0xb0, 0xb1, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xbb, 0xbc, 0xbd, 0xbe, 0xbf,
    ];
    {
        let (c, air) = tc(s);
        c.use_network_key(air, new_key, 1, false); // the network moved on; the bulb was not told
    }
    let i = s.mark();
    let mut f = None;
    let t0 = s.now();
    let dut = s.dut_short;
    while s.now() - t0 < 100000.0 && f.is_none() {
        let (c, air) = tc(s);
        c.st.tx_link_status(air, &[(dut, 1, 1)], &Tx::new()); // what a neighbour under the new key sounds like
        f = s.wait(
            |f| f.nwk_cmd() == Some(0x06) && !f.nwk.as_ref().unwrap().secured,
            5000.0,
            Some(i),
            DUT,
            5.0,
        )?;
    }
    let Some(f) = f else {
        let sec = s.frames(i, DUT, |f| f.nwk_cmd() == Some(0x06));
        return Err(Stop::Inconclusive(format!(
            "no unsecured Rejoin Request within 100 s of the network moving to a new key{}",
            if sec.is_empty() { "" } else { "; the bulb sent Rejoin Requests secured with the old key" }
        )));
    };
    let n = f.nwk.clone().unwrap();
    expect(
        n.radius == 1 && n.src_ieee == Some(s.dut_ext),
        format!("Rejoin Request: radius {}, source IEEE {}", n.radius, py_opt_u64(n.src_ieee)),
    )?;
    let short = n.src;
    let ext = s.dut_ext;
    let lk = s.tclk();
    let (c, air) = tc(s);
    let mut rsp = vec![0x07];
    rsp.extend_from_slice(&short.to_le_bytes());
    rsp.push(0x00);
    c.tx_nwk_cmd(air, short, &rsp, &Tx::new().radius(1).dst_ieee(ext).insecure());
    let mut body = vec![0x05, 0x01];
    body.extend_from_slice(&new_key);
    body.push(1);
    body.extend_from_slice(&ext_bytes(ext));
    body.extend_from_slice(&ext_bytes(c.ext));
    let aps = c.aps_cmd_secured(&body, &hmac_mmo(&lk, &[0x00]), 2);
    c.nwk_send(air, short, &aps, 0, false, 1, None, false);
    s.run(3000.0)?;
    s.dut_short = short;
    expect(
        s.alive()?,
        "after the Rejoin Response and the Transport Key (new network key under the link key) the bulb does not \
         answer under the new key",
    )?;
    Ok(format!("unsecured Rejoin Request {:.0} s after the key change", (f.t0 - t0) / 1000.0))
}

fn leave_request_broadcast(s: &mut Session) -> R<String> {
    s.w.coord.permit_join = None;
    let i = s.mark();
    for _ in 0..3 {
        // three copies, so that a lost frame cannot pass the test
        let (c, air) = tc(s);
        c.tx_nwk_cmd(air, 0xFFFD, &[0x04, 0x40], &Tx::new().radius(1));
        s.run(1000.0)?;
    }
    s.run(1500.0)?;
    let lv = s.frames(i, DUT, is_nwk_cmd(0x04));
    let alive = s.alive()?;
    expect(
        lv.is_empty() && alive,
        format!(
            "a Leave request broadcast to 0xFFFD was executed: {}",
            if lv.is_empty() { "the bulb stopped answering" } else { "the bulb announced its leave" }
        ),
    )?;
    Ok(String::new())
}

fn leave_payload(ext: u64) -> Vec<u8> {
    let mut p = ext_bytes(ext).to_vec();
    p.push(0x00);
    p
}

fn mgmt_leave(s: &mut Session) -> R<String> {
    s.w.coord.permit_join = None;
    let i = s.mark();
    let pl = leave_payload(s.dut_ext);
    let f = s.zdo(0x0034, &pl, 2000.0)?;
    s.run(3000.0)?;
    let lv = s.frames(i, DUT, is_nwk_cmd(0x04));
    expect(
        f.is_some(),
        format!("no Mgmt_Leave_rsp{}", if lv.is_empty() { "" } else { "; the bulb announced its leave" }),
    )?;
    let f = f.unwrap();
    let st = f.zdo().unwrap().payload[0];
    expect(st == 0x00, format!("Mgmt_Leave_rsp status {st:#04x}"))?;
    expect(!lv.is_empty(), "Mgmt_Leave_rsp SUCCESS, but no NWK Leave announcement followed within 3 s")?;
    expect(f.i < lv[0].i, "the response was sent after the leave announcement")?;
    check_leave_frame(s, &lv[0], false)?;
    expect(!s.alive()?, "the bulb still answers at its address after leaving")?;
    Ok(String::new())
}

fn mgmt_leave_broadcast(s: &mut Session) -> R<String> {
    s.w.coord.permit_join = None;
    let i = s.mark();
    let pl = leave_payload(s.dut_ext);
    let f = s.zdo_with(0x0034, &pl, Tx::new().dst(0xFFFD).max_ms(2000.0))?;
    s.run(3000.0)?;
    let lv = s.frames(i, DUT, is_nwk_cmd(0x04));
    let alive = s.alive()?;
    let what: Vec<&str> = [
        ("it answered with Mgmt_Leave_rsp", f.is_some()),
        ("it announced its leave", !lv.is_empty()),
        ("it stopped answering", !alive),
    ]
    .iter()
    .filter(|(_, c)| *c)
    .map(|(w, _)| *w)
    .collect();
    expect(what.is_empty(), format!("a broadcast Mgmt_Leave_req was executed: {}", what.join(", ")))?;
    Ok(String::new())
}

fn mgmt_leave_other(s: &mut Session) -> R<String> {
    s.w.coord.permit_join = None;
    let i = s.mark();
    let f = s.zdo(0x0034, &leave_payload(0x00124B00DEAD0003), 2000.0)?;
    s.run(2500.0)?;
    let lv = s.frames(i, DUT, is_nwk_cmd(0x04));
    expect(lv.is_empty() && s.alive()?, "the bulb left the network on a Mgmt_Leave_req naming another device")?;
    expect(f.is_some(), "no Mgmt_Leave_rsp")?;
    let st = f.unwrap().zdo().unwrap().payload[0];
    expect(st != 0x00, "Mgmt_Leave_rsp SUCCESS for a device the router does not know")?;
    Ok(format!("status {st:#04x}"))
}

fn conflict_status(s: &mut Session, i: usize) -> Vec<CapFrame> {
    s.frames(i, DUT, is_nwk_cmd(0x03))
        .into_iter()
        .filter(|f| f.nwk.as_ref().unwrap().payload.as_ref().unwrap()[1] == 0x0D)
        .collect()
}

fn address_conflict(s: &mut Session) -> R<String> {
    let i = s.mark();
    let dut = s.dut_short;
    s.tx_zcl(0x0000, 0x00, &[0x00, 0x00], Tx::new().general().ddr().dst_ieee(0x00124B00DEAD0004));
    s.run(4000.0)?;
    let mut st = conflict_status(s, i);
    let mut how = "a frame for the bulb's address with another device's IEEE address in the NWK header";
    if st.is_empty() {
        // second way to learn of it (3.6.1.9.2): a new broadcast whose NWK source is the bulb's own address,
        // received more than nwkNetworkBroadcastDeliveryTime after the bulb started operating
        let wait = 20000.0 - (s.now() - s.t_joined);
        if wait > 0.0 {
            s.run(wait)?;
        }
        s.tx_zcl(
            0x0003,
            0x01,
            &[],
            Tx::new().dst(0xFFFD).ddr().radius(3).src(dut).src_ieee(SrcIeee::Is(0x00124B00DEAD0004)).nwk_seq(0xB7),
        );
        s.run(4000.0)?;
        st = conflict_status(s, i);
        how = "a broadcast from another device under the bulb's network address";
    }
    let an = s.frames(i, DUT, |f| f.zdo().is_some_and(|z| z.cluster == 0x0013));
    expect(
        !st.is_empty(),
        format!(
            "no Network Status 'address conflict' after a frame for the bulb's address with another device's IEEE \
             address in the NWK header, nor after a broadcast from another device under the bulb's network address{}",
            if an.is_empty() { "" } else { "; a Device_annce was sent" }
        ),
    )?;
    let f = &st[0];
    let n = f.nwk.as_ref().unwrap();
    expect(n.dst == 0xFFFD, format!("Network Status broadcast to {:#06x}", n.dst))?;
    let p = n.payload.as_ref().unwrap();
    expect(
        u16::from_le_bytes([p[2], p[3]]) == s.dut_short,
        "the destination address field is not the address in conflict",
    )?;
    expect(!an.is_empty(), "no Device_annce with a new address after the conflict")?;
    let z = an.last().unwrap().zdo().unwrap().payload;
    let new_short = u16::from_le_bytes([z[0], z[1]]);
    let new_ext = u64::from_le_bytes(z[2..10].try_into().unwrap());
    expect(
        new_ext == s.dut_ext && new_short != s.dut_short,
        format!(
            "announced {new_short:#06x} for {new_ext:#018x}; a new address different from {:#06x} is required",
            s.dut_short
        ),
    )?;
    Ok(format!("new address {new_short:#06x} after {how}"))
}

fn pan_id_update(s: &mut Session) -> R<String> {
    let old = s.w.coord.st.pan;
    let new = 0x2B7Cu16;
    let i = s.mark();
    let (c, air) = tc(s);
    let mut body = vec![0x0A, 0x01];
    body.extend_from_slice(&ext_bytes(c.ext_pan));
    body.push(1);
    body.extend_from_slice(&new.to_le_bytes());
    c.tx_nwk_cmd(air, 0xFFFF, &body, &Tx::new().radius(30));
    let t0 = s.now();
    s.run(1000.0)?;
    let rel = s.frames(i, DUT, is_nwk_cmd(0x0A));
    for f in &rel {
        let dpan = f.mac.as_ref().unwrap().dpan;
        expect(
            dpan == Some(old),
            format!(
                "the bulb relayed the Network Update under PAN {:#06x} {:.0} ms after receiving it; it must go out \
                 under the old PAN identifier {old:#06x}",
                dpan.unwrap_or(0),
                f.t0 - t0
            ),
        )?;
    }
    let r = s.read(0x0000, &[0x0000], 800.0)?;
    expect(r.is_some(), "1 s after the Network Update the bulb no longer answers under the old PAN identifier")?;
    let mut switched = None;
    while s.now() - t0 < 60000.0 {
        s.w.coord.st.pan = new;
        if s.read(0x0000, &[0x0000], 700.0)?.is_some() {
            switched = Some(s.now() - t0);
            break;
        }
        s.w.coord.st.pan = old;
        s.run(1000.0)?;
    }
    s.w.coord.st.pan = new;
    expect(switched.is_some(), "the bulb did not move to the new PAN identifier within 60 s")?;
    Ok(format!(
        "moved to the new PAN identifier {:.1} s after the command; relayed copies: {}",
        switched.unwrap() / 1000.0,
        rel.len()
    ))
}

fn channel_change(s: &mut Session) -> R<String> {
    let new = 20i64;
    let i = s.mark();
    let dut = s.dut_short;
    let mut pl = (1u32 << new).to_le_bytes().to_vec();
    pl.extend_from_slice(&[0xFE, 1]);
    let (c, air) = tc(s);
    c.tx_zdo(air, dut, 0x0038, &pl, &Tx::new());
    s.run(500.0)?;
    let t0 = s.now();
    let mut moved = None;
    s.w.coord.st.channel = new;
    while s.now() - t0 < 30000.0 {
        if s.read(0x0000, &[0x0000], 900.0)?.is_some() {
            moved = Some(s.now() - t0);
            break;
        }
        s.run(1000.0)?;
    }
    let here = s.frames(i, DUT, |f| f.ch == new);
    expect(
        moved.is_some() && !here.is_empty(),
        "the bulb did not answer on the new channel within 30 s of the request",
    )?;
    Ok(format!("answering on channel {new} {:.1} s after the request", moved.unwrap() / 1000.0))
}
