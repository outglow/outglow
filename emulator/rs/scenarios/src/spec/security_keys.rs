//! Keys (R22 4.3.1, 4.4, 4.6.3.4, 4.7.3.10; BDB 3.0.1 10.2.5). Every test has a
//! session of its own, because each one changes the key material of the network.
//! The Trust Center side of the key update is written from 4.6.3.4.1: Transport
//! Key with sequence number N+1 (unicast under the key-transport key of the
//! device's link key, or broadcast with NWK security only and an all-zero
//! destination field), then Switch Key broadcast to 0xFFFD without APS security.
//! Ported from emulator/spec/security_keys.py.

use super::{SessionDef, Test};
use crate::air::{Air, NodeId};
use crate::coord::Coord;
use crate::crypto::hmac_mmo;
use crate::frames::{aps_cmd, ext_bytes, Key, NWK_DATA};
use crate::session::{expect, is_aps_cmd, is_nwk_cmd, Session, Stop, R};
use crate::station::{nx, Tx};

const NEW_KEY: Key = [
    0xa0, 0xa1, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xab, 0xac, 0xad, 0xae, 0xaf,
];
const DUT: Option<NodeId> = Some(NodeId::Dut);

fn fc_per_key(c: &mut Coord) {
    c.aps_fc_per_key = true;
}

fn mute_tclk(c: &mut Coord) {
    c.mute_tclk = true;
}

pub fn tests() -> Vec<Test> {
    vec![
        Test {
            id: "KEY-TCLK-01",
            doc: "BDB 3.0.1 / R22",
            clause: "10.2.5 step 9; R22 4.4.8.2.3 step 9",
            req: "When the device takes a new Trust Center link key it sets the incoming frame counter for that key \
                  to 0, so a Trust Center that starts the new key's counter at 0 is accepted: the Confirm Key ends \
                  the exchange.",
            setup: SessionDef { tc: Some(fc_per_key), ..SessionDef::D },
            run: tclk_counter_restart,
            ..Test::D
        },
        Test {
            id: "KEY-TCLK-02",
            doc: "BDB 3.0.1",
            clause: "8.2 step 12, 10.2.5 steps 7-8, 5.1.4",
            req: "When the Trust Center does not answer the Request Key, the node retries every \
                  bdbcTCLinkKeyExchangeTimeout (5 s), up to bdbTCLinkKeyExchangeAttemptsMax attempts (an attribute, \
                  default 3), and then leaves the network.",
            setup: SessionDef { tc: Some(mute_tclk), ..SessionDef::D },
            run: tclk_failure,
            ..Test::D
        },
        Test {
            id: "KEY-NWK-01",
            doc: "R22",
            clause: "4.6.3.4.2, 4.7.3.10.6",
            req: "A network key sent by the Trust Center by unicast (under the link key) becomes the alternate key, \
                  and the broadcast Switch Key makes it the active key: from then on the device secures its frames \
                  with the new key sequence number and never with the old key.",
            run: key_update_unicast,
            ..Test::D
        },
        Test {
            id: "KEY-NWK-02",
            doc: "R22",
            clause: "4.6.3.4.2",
            req: "With a new network key all incoming frame counters start at 0 again: a neighbour that restarts its \
                  outgoing counter with the new key is accepted after the switch.",
            run: key_update_counter_restart,
            ..Test::D
        },
        Test {
            id: "KEY-NWK-03",
            doc: "R22",
            clause: "4.4.1.5, 4.7.3.10.3",
            req: "A device in the joined and authorized state accepts a network key update that the Trust Center \
                  broadcasts with NWK security only (no APS encryption), and switches to it on Switch Key.",
            ambiguous: Some(
                "4.4.1.5 says 'shall accept', 4.4.2.3 says the device 'may accept' the same command; R23 allows a \
                 policy that refuses it",
            ),
            run: key_update_broadcast,
            ..Test::D
        },
        Test {
            id: "KEY-NWK-04",
            doc: "R22",
            clause: "4.3.1.2 step 7, 4.7.3.10.5",
            req: "A device that holds the new network key as alternate and receives a frame that decrypts under the \
                  newer key sequence number switches to that key by itself, without a Switch Key command.",
            run: implicit_switch,
            ..Test::D
        },
        Test {
            id: "KEY-NWK-05",
            doc: "R22",
            clause: "4.6.3.4.2, 4.4.2.3",
            req: "A network key is accepted only from the Trust Center: a unicast Transport Key that is not secured \
                  with a link key, sent by another router, is discarded, and a Switch Key from that router does not \
                  change the active key.",
            run: key_from_stranger,
            ..Test::D
        },
        Test {
            id: "KEY-APS-01",
            doc: "R22",
            clause: "4.4.1.2 step 4",
            req: "An APS-secured frame under a unique link key whose frame counter is not above the last one \
                  accepted from that device is rejected.",
            run: aps_replay,
            ..Test::D
        },
        Test {
            id: "KEY-REMOVE-01",
            doc: "R22",
            clause: "4.4.1.3 (Table 4-6), 4.6.3.6.2",
            req: "A Remove Device command without APS encryption is dropped; one from the Trust Center, \
                  APS-encrypted, naming the router itself makes it leave the network.",
            run: remove_self,
            ..Test::D
        },
    ]
}

fn tc(s: &mut Session) -> (&mut Coord, &mut Air) {
    let w = &mut s.w;
    (&mut w.coord, &mut w.air)
}

/// Python's `sorted(set(xs))` printed.
fn sorted_set(xs: &[Option<u8>]) -> String {
    let mut v: Vec<u8> = xs.iter().flatten().copied().collect();
    v.sort();
    v.dedup();
    let mut parts: Vec<String> = Vec::new();
    if xs.iter().any(|x| x.is_none()) {
        parts.push("None".into());
    }
    parts.extend(v.iter().map(|x| x.to_string()));
    format!("[{}]", parts.join(", "))
}

/// Key sequence numbers of the secured NWK frames the bulb sent since the mark, in order.
fn bulb_key_seq(s: &mut Session, since: usize) -> Vec<Option<u8>> {
    s.frames(since, DUT, |f| f.nwk.as_ref().is_some_and(|n| n.secured))
        .iter()
        .map(|f| f.nwk.as_ref().unwrap().key_seq)
        .collect()
}

/// Is a Read Attributes from the Trust Center (under its current key) answered?
fn probe(s: &mut Session, max_ms: f64) -> R<bool> {
    Ok(s.read(0x0000, &[0x0000], max_ms)?.is_some())
}

/// Asks once a second for up to total_ms; the time until the first answer or None.
fn probe_for(s: &mut Session, total_ms: f64) -> R<Option<f64>> {
    let t0 = s.now();
    while s.now() - t0 < total_ms {
        if probe(s, 900.0)? {
            return Ok(Some(s.now() - t0));
        }
        s.run(300.0)?;
    }
    Ok(None)
}

/// Makes the bulb transmit (it relays a broadcast of the Trust Center), so that the
/// key it uses for its own frames shows.
fn provoke(s: &mut Session, ms: f64) -> R<usize> {
    let i = s.mark();
    // A broadcast nobody passed on goes again, as a coordinator sends it
    // (R22 3.6.5): the bulb may have been on BLE for the one copy.
    for _ in 0..3 {
        s.tx_zcl(0x0003, 0x01, &[], Tx::new().dst(0xFFFD).ddr().radius(3));
        s.run(ms)?;
        if !s.frames(i, DUT, |_| true).is_empty() {
            break;
        }
    }
    Ok(i)
}

fn round1(x: f64) -> f64 {
    format!("{x:.1}").parse().unwrap()
}

fn py_floats(v: &[f64]) -> String {
    format!("[{}]", v.iter().map(|x| format!("{x:?}")).collect::<Vec<_>>().join(", "))
}

fn tclk_counter_restart(s: &mut Session) -> R<String> {
    let ck = s.frames(0, Some(NodeId::Coord), is_aps_cmd(0x10));
    if ck.is_empty() {
        return Err(Stop::Inconclusive("no Confirm Key was sent: the bulb did not get as far as Verify Key".into()));
    }
    let fc0 = ck[0].aps.as_ref().and_then(|a| a.aux.as_ref()).map(|x| x.fc);
    expect(fc0 == Some(0), "harness error: the Confirm Key did not carry frame counter 0")?;
    let i = ck[0].i;
    s.run(14000.0)?;
    let again = s.frames(i, DUT, |f| matches!(f.aps_cmd(), Some(0x0F) | Some(0x08)));
    let left = s.frames(i, DUT, is_nwk_cmd(0x04));
    let n_vk = again.iter().filter(|f| f.aps_cmd() == Some(0x0F)).count();
    let n_rk = again.iter().filter(|f| f.aps_cmd() == Some(0x08)).count();
    expect(
        again.is_empty() && left.is_empty(),
        format!(
            "after a Confirm Key with frame counter 0 under the new link key the bulb sent {n_vk} more Verify Key \
             and {n_rk} more Request Key command(s) in 14 s{}: the Confirm Key was not accepted",
            if left.is_empty() { "" } else { " and then left the network" }
        ),
    )?;
    // the new key is really in use: a command under it, again with a low counter, is acted on
    let (dut, ext, lk) = (s.dut_short, s.dut_ext, s.tclk());
    let (c, air) = tc(s);
    let mut body = vec![0x05, 0x01];
    body.extend_from_slice(&NEW_KEY);
    body.push(1);
    body.extend_from_slice(&ext_bytes(ext));
    body.extend_from_slice(&ext_bytes(c.ext));
    let aps = c.aps_cmd_secured(&body, &hmac_mmo(&lk, &[0x00]), 2);
    c.nwk_send_data(air, dut, &aps);
    s.run(500.0)?;
    let (c, air) = tc(s);
    c.send_switch_key(air, 1);
    s.run(1500.0)?;
    let (c, air) = tc(s);
    c.use_network_key(air, NEW_KEY, 1, true);
    expect(probe(s, 1500.0)?, "a network key sent under the new link key (frame counter 1) was not taken into use")?;
    Ok(String::new())
}

fn tclk_failure(s: &mut Session) -> R<String> {
    s.w.coord.permit_join = None;
    s.run(45000.0)?;
    let rk = s.frames(0, DUT, is_aps_cmd(0x08));
    if rk.is_empty() {
        return Err(Stop::Inconclusive("the bulb sent no Request Key at all".into()));
    }
    let gaps: Vec<f64> = rk.windows(2).map(|w| round1((w[1].t0 - w[0].t0) / 1000.0)).collect();
    let lv = s.frames(0, DUT, is_nwk_cmd(0x04));
    expect(
        gaps.iter().all(|g| (4.0..=6.5).contains(g)),
        format!(
            "{} Request Key attempts with gaps {} s; the timeout bdbcTCLinkKeyExchangeTimeout is 5 s",
            rk.len(),
            py_floats(&gaps)
        ),
    )?;
    expect(
        !lv.is_empty(),
        format!(
            "after {} unanswered Request Key attempts the bulb stayed on the network (no Leave within {:.0} s of the \
             last attempt)",
            rk.len(),
            (s.now() - rk.last().unwrap().t0) / 1000.0
        ),
    )?;
    let o = lv[0].nwk.as_ref().unwrap().payload.as_ref().unwrap()[1];
    expect(o & 0x60 == 0, format!("Leave options {o:#04x}: request = 0, rejoin = 0 required"))?;
    Ok(format!("{} attempts (attribute default 3), gaps {} s, then Leave", rk.len(), py_floats(&gaps)))
}

fn unicast_update(s: &mut Session, switch: bool) -> R<()> {
    let (dut, ext, lk) = (s.dut_short, s.dut_ext, s.tclk());
    let (c, air) = tc(s);
    c.send_network_key(air, dut, ext, &NEW_KEY, 1, Some(lk), false, None);
    s.run(800.0)?;
    if switch {
        let (c, air) = tc(s);
        c.send_switch_key(air, 1);
        s.run(1500.0)?;
    }
    Ok(())
}

fn use_key(s: &mut Session, key: Key, seq: u8, restart: bool) {
    let (c, air) = tc(s);
    c.use_network_key(air, key, seq, restart);
}

fn key_update_unicast(s: &mut Session) -> R<String> {
    if !probe(s, 1500.0)? {
        return Err(Stop::Inconclusive("control failed: the bulb does not answer before the key update".into()));
    }
    unicast_update(s, true)?;
    use_key(s, NEW_KEY, 1, false);
    let dt = probe_for(s, 12000.0)?;
    expect(dt.is_some(), "12 s after the Switch Key the bulb does not answer a request secured with the new key")?;
    let i = provoke(s, 1200.0)?;
    let seqs = bulb_key_seq(s, i);
    expect(
        !seqs.is_empty() && seqs.iter().all(|q| *q == Some(1)),
        format!(
            "key sequence numbers of the bulb's frames after the switch: {}; only the new key (1) may be used",
            sorted_set(&seqs)
        ),
    )?;
    let undec = s.frames(i, DUT, |f| f.nwk.as_ref().is_some_and(|n| n.secured && n.payload.is_none()));
    expect(undec.is_empty(), "frames marked with the new key sequence number do not decrypt with the new key")?;
    Ok(format!("answering under the new key {:.1} s after the Switch Key", dt.unwrap() / 1000.0))
}

fn key_update_counter_restart(s: &mut Session) -> R<String> {
    for _ in 0..40 {
        // raise the Trust Center's counter under the old key
        s.w.coord.st.fc_out += 50;
    }
    if !probe(s, 1500.0)? {
        return Err(Stop::Inconclusive("control failed: the bulb does not answer before the key update".into()));
    }
    let old_fc = s.w.coord.st.fc_out;
    unicast_update(s, true)?;
    use_key(s, NEW_KEY, 1, true);
    let ok = probe_for(s, 12000.0)?.is_some();
    expect(
        ok,
        format!(
            "12 s after the switch the bulb still ignores the Trust Center's frames under the new key with frame \
             counters starting at 0 (its last counter under the old key was {old_fc})"
        ),
    )?;
    Ok(String::new())
}

fn key_update_broadcast(s: &mut Session) -> R<String> {
    if !probe(s, 1500.0)? {
        return Err(Stop::Inconclusive("control failed: the bulb does not answer before the key update".into()));
    }
    let (c, air) = tc(s);
    c.send_network_key(air, 0, 0, &NEW_KEY, 1, None, true, None);
    s.run(1500.0)?;
    let (c, air) = tc(s);
    c.send_switch_key(air, 1);
    s.run(1500.0)?;
    use_key(s, NEW_KEY, 1, false);
    let ok = probe_for(s, 12000.0)?.is_some();
    let i = provoke(s, 1200.0)?;
    let seqs = bulb_key_seq(s, i);
    if !ok {
        let k0 = s.w.air.keys.nwk_key(0).unwrap();
        use_key(s, k0, 0, false);
        let old_ok = probe(s, 1500.0)? || probe(s, 1500.0)?;
        return Err(Stop::Fail(format!(
            "12 s after a broadcast key update (NWK security only) and Switch Key the bulb does not answer under the \
             new key{}",
            if old_ok { "; it still answers under the old key" } else { " and not under the old key either" }
        )));
    }
    expect(
        !seqs.is_empty() && seqs.iter().all(|q| *q == Some(1)),
        format!("the bulb secures its frames with key sequence number(s) {} after the switch", sorted_set(&seqs)),
    )?;
    Ok(String::new())
}

fn implicit_switch(s: &mut Session) -> R<String> {
    if !probe(s, 1500.0)? {
        return Err(Stop::Inconclusive("control failed: the bulb does not answer before the key update".into()));
    }
    unicast_update(s, false)?;
    let i = provoke(s, 1200.0)?;
    let before = bulb_key_seq(s, i);
    if before.is_empty() || before.iter().any(|q| *q != Some(0)) {
        return Err(Stop::Inconclusive(format!(
            "before any frame under the new key the bulb already uses key sequence number(s) {}",
            sorted_set(&before)
        )));
    }
    use_key(s, NEW_KEY, 1, false);
    let dt = probe_for(s, 8000.0)?; // frames under the new key, no Switch Key
    let i = provoke(s, 1200.0)?;
    let seqs = bulb_key_seq(s, i);
    expect(
        dt.is_some(),
        "the bulb holds the new key as alternate, but does not answer frames secured with it (8 s of requests \
         under the new key sequence number, no Switch Key)",
    )?;
    expect(
        !seqs.is_empty() && seqs.iter().all(|q| *q == Some(1)),
        format!(
            "after receiving frames secured with the new key the bulb {}",
            if seqs.is_empty() {
                "transmits nothing".to_string()
            } else {
                format!("still sends with key sequence number(s) {}", sorted_set(&seqs))
            }
        ),
    )?;
    Ok(String::new())
}

fn key_from_stranger(s: &mut Session) -> R<String> {
    let r2 = s.router("r2", 0x00124B00BB000002, 0x2B02, Some(&["dut"]), true);
    s.run(1000.0)?;
    if !probe(s, 1500.0)? {
        return Err(Stop::Inconclusive("control failed: the bulb does not answer before the attack".into()));
    }
    let evil: Key = core::array::from_fn(|k| 0x30 + k as u8);
    let mut body = vec![0x05, 0x01];
    body.extend_from_slice(&evil);
    body.push(1);
    body.extend_from_slice(&ext_bytes(s.dut_ext));
    body.extend_from_slice(&ext_bytes(s.w.coord.ext));
    let dut = s.dut_short;
    let (st, air) = s.w.st(r2);
    st.tx_aps_cmd(air, dut, &body, None, &Tx::new());
    s.run(800.0)?;
    for _ in 0..2 {
        // two copies, so that a lost broadcast cannot pass the test
        let (st, air) = s.w.st(r2);
        let ctr = nx(&mut st.aps_ctr);
        let a = aps_cmd(ctr, &[0x09, 1], false, None, 2);
        st.tx_nwk(air, NWK_DATA, 0xFFFD, &a, &Tx::new());
        s.run(700.0)?;
    }
    s.w.air.keys.set_nwk(1, evil);
    let i = s.mark();
    s.run(17000.0)?; // long enough for a Link Status of the bulb
    let seqs = bulb_key_seq(s, i);
    let ok = probe(s, 1500.0)? || probe(s, 1500.0)?;
    expect(
        ok && seqs.iter().all(|q| *q == Some(0)),
        format!(
            "a network key sent by a router that is not the Trust Center (unicast, no APS encryption, the Trust \
             Center's address in the source field) and that router's Switch Key were accepted: the bulb {}{}",
            if seqs.is_empty() {
                "is silent".to_string()
            } else {
                format!("now secures its frames with key sequence number(s) {}", sorted_set(&seqs))
            },
            if ok { "" } else { " and no longer answers the Trust Center under the network key" }
        ),
    )?;
    Ok(String::new())
}

fn aps_replay(s: &mut Session) -> R<String> {
    let lk = s.tclk();
    if lk == s.w.coord.tclk {
        return Err(Stop::Inconclusive(
            "no unique link key was established (the rule applies to unique link keys)".into(),
        ));
    }
    let fc: u32 = 0x4000;
    let v0 = s.value(0x0006, 0x0000)?;
    s.tx_zcl(0x0006, 0x02, &[], Tx::new().ddr().aps_sec((lk, fc)));
    s.run(600.0)?;
    let v1 = s.value(0x0006, 0x0000)?;
    if v0.is_none() || v1 == v0 {
        return Err(Stop::Inconclusive(
            "the bulb does not act on a ZCL command that is APS-encrypted with its link key, so a replay cannot be \
             told from a first delivery"
                .into(),
        ));
    }
    s.tx_zcl(0x0006, 0x02, &[], Tx::new().ddr().aps_sec((lk, fc)));
    s.run(600.0)?;
    let v2 = s.value(0x0006, 0x0000)?;
    expect(
        v2 == v1,
        "a second APS-encrypted Toggle with the same APS frame counter (new APS counter, new NWK frame) was executed",
    )?;
    s.tx_zcl(0x0006, 0x02, &[], Tx::new().ddr().aps_sec((lk, fc - 0x100)));
    s.run(600.0)?;
    let v3 = s.value(0x0006, 0x0000)?;
    expect(v3 == v2, "an APS-encrypted Toggle with a lower APS frame counter was executed")?;
    Ok(String::new())
}

fn remove_self(s: &mut Session) -> R<String> {
    s.w.coord.permit_join = None;
    let mut body = vec![0x07];
    body.extend_from_slice(&ext_bytes(s.dut_ext));
    let dut = s.dut_short;
    let i = s.mark();
    let (c, air) = tc(s);
    c.st.tx_aps_cmd(air, dut, &body, None, &Tx::new());
    s.run(2500.0)?;
    let lv = s.frames(i, DUT, is_nwk_cmd(0x04));
    expect(lv.is_empty() && s.alive()?, "a Remove Device command without APS encryption made the bulb leave")?;
    let i = s.mark();
    let lk = s.tclk();
    let (c, air) = tc(s);
    let aps = c.aps_cmd_secured(&body, &lk, 0);
    c.nwk_send_data(air, dut, &aps);
    s.run(3000.0)?;
    let lv = s.frames(i, DUT, is_nwk_cmd(0x04));
    let gone = !s.alive()?;
    expect(
        gone,
        format!(
            "an APS-encrypted Remove Device from the Trust Center naming the bulb itself was not obeyed: the bulb is \
             still on the network{}",
            if lv.is_empty() { "" } else { " (it did announce a leave)" }
        ),
    )?;
    Ok(format!("left{}", if lv.is_empty() { " without a Leave announcement" } else { " after announcing it" }))
}
