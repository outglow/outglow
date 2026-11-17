use crate::sim::*;
use zigbee::stack::Identity;

const BASIC: u16 = 0x0000;
const IDENTIFY: u16 = 0x0003;
const GROUPS: u16 = 0x0004;
const SCENES: u16 = 0x0005;
const ON_OFF: u16 = 0x0006;
const LEVEL: u16 = 0x0008;
const COLOR: u16 = 0x0300;
const DIAG: u16 = 0x0B05;
const TOUCHLINK: u16 = 0x1000;
const SETTINGS: u16 = 0xFC5F;
const MFR: u16 = 0x1141;

fn bulb_with(identity: Identity) -> (Net, usize, u16) {
    let mut net = Net::new();
    net.identity = identity;
    let n = net.join_node();
    let short = net.nodes[n].short();
    (net, n, short)
}

fn value_len(ty: u8, rest: &[u8]) -> usize {
    match ty {
        0x08 | 0x10 | 0x18 | 0x20 | 0x28 | 0x30 => 1,
        0x09 | 0x19 | 0x21 | 0x29 | 0x31 => 2,
        0x0A | 0x1A | 0x22 | 0x2A => 3,
        0x0B | 0x1B | 0x23 | 0x2B | 0x39 => 4,
        0x41 | 0x42 => 1 + rest[0] as usize,
        _ => panic!("type {ty:#04x}"),
    }
}

fn read_records(rsp: &[u8]) -> Vec<(u16, u8, Vec<u8>)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < rsp.len() {
        let id = u16::from_le_bytes([rsp[i], rsp[i + 1]]);
        let status = rsp[i + 2];
        if status != 0 {
            out.push((id, status, vec![]));
            i += 3;
            continue;
        }
        let ty = rsp[i + 3];
        let n = value_len(ty, &rsp[i + 4..]);
        let mut v = vec![ty];
        v.extend_from_slice(&rsp[i + 4..i + 4 + n]);
        out.push((id, 0, v));
        i += 4 + n;
    }
    out
}

fn read(net: &mut Net, short: u16, cluster: u16, ids: &[u16]) -> Vec<(u16, u8, Vec<u8>)> {
    let mut p = Vec::new();
    for id in ids {
        p.extend_from_slice(&id.to_le_bytes());
    }
    let seq = net.coord.zcl(short, cluster, 0x00, 0x00, &p);
    net.run(100);
    let rsp = net.coord.last_zcl(short, cluster, 0x01).unwrap();
    assert_eq!(rsp[..3], [0x18, seq, 0x01]);
    read_records(&rsp[3..])
}

fn s(text: &str) -> Vec<u8> {
    let mut v = vec![0x42, text.len() as u8];
    v.extend_from_slice(text.as_bytes());
    v
}

fn ok(id: u16, v: &[u8]) -> (u16, u8, Vec<u8>) {
    (id, 0, v.to_vec())
}

fn missing(id: u16) -> (u16, u8, Vec<u8>) {
    (id, 0x86, vec![])
}

#[test]
fn every_basic_attribute_has_its_type_and_value() {
    let (mut net, _, short) = bulb_with(Identity { app_version: 7, hw_version: 3, sw_build: "1.2.3", ..IDENTITY });
    let ids = [0x0000, 0x0001, 0x0002, 0x0003, 0x0004, 0x0005, 0x0006, 0x0007, 0x0008, 0x0009, 0x000A, 0x000B, 0x0012, 0x4000, 0xFFFD, 0x0010];
    assert_eq!(
        read(&mut net, short, BASIC, &ids),
        vec![
            ok(0x0000, &[0x20, 8]),
            ok(0x0001, &[0x20, 7]),
            ok(0x0002, &[0x20, 2]),
            ok(0x0003, &[0x20, 3]),
            ok(0x0004, &s("AwoX")),
            ok(0x0005, &s("ESMLFzm_w6_Dimm")),
            ok(0x0006, &s("")),
            ok(0x0007, &[0x30, 0x01]),
            ok(0x0008, &[0x30, 0x00]),
            ok(0x0009, &[0x30, 0x05]),
            ok(0x000A, &[0x41, 0]),
            ok(0x000B, &[0x42, 0]),
            ok(0x0012, &[0x10, 1]),
            ok(0x4000, &s("1.2.3")),
            ok(0xFFFD, &[0x21, 3, 0]),
            missing(0x0010),
        ]
    );
}

#[test]
fn every_attribute_of_the_light_clusters_has_its_type_and_value() {
    let (mut net, _, short) = bulb_with(IDENTITY);
    assert_eq!(read(&mut net, short, IDENTIFY, &[0x0000, 0xFFFD, 0x0001]), vec![ok(0, &[0x21, 0, 0]), ok(0xFFFD, &[0x21, 2, 0]), missing(1)]);
    assert_eq!(read(&mut net, short, GROUPS, &[0x0000, 0xFFFD]), vec![ok(0, &[0x18, 0]), ok(0xFFFD, &[0x21, 3, 0])]);
    assert_eq!(
        read(&mut net, short, SCENES, &[0, 1, 2, 3, 4, 0xFFFD, 5]),
        vec![
            ok(0, &[0x20, 0]),
            ok(1, &[0x20, 0]),
            ok(2, &[0x21, 0, 0]),
            ok(3, &[0x10, 0]),
            ok(4, &[0x18, 0]),
            ok(0xFFFD, &[0x21, 3, 0]),
            missing(5)
        ]
    );
    assert_eq!(
        read(&mut net, short, ON_OFF, &[0x0000, 0x4000, 0x4001, 0x4002, 0x4003, 0xFFFD, 0x0001]),
        vec![
            ok(0, &[0x10, 1]),
            ok(0x4000, &[0x10, 1]),
            ok(0x4001, &[0x21, 0, 0]),
            ok(0x4002, &[0x21, 0, 0]),
            ok(0x4003, &[0x30, 0xFF]),
            ok(0xFFFD, &[0x21, 2, 0]),
            missing(1)
        ]
    );
    let level = read(&mut net, short, LEVEL, &[0x0001, 0x0002, 0x0003, 0x000F, 0x0011, 0xFFFD, 0x0004]);
    assert_eq!(
        level,
        vec![ok(1, &[0x21, 0, 0]), ok(2, &[0x20, 1]), ok(3, &[0x20, 254]), ok(0x0F, &[0x18, 0]), ok(0x11, &[0x20, 0xFF]), ok(0xFFFD, &[0x21, 2, 0]), missing(4)]
    );
    assert_eq!(read(&mut net, short, LEVEL, &[0x4000])[0].2[0], 0x20);
    assert_eq!(read(&mut net, short, TOUCHLINK, &[0xFFFD, 0x0000]), vec![ok(0xFFFD, &[0x21, 3, 0]), missing(0)]);
    assert_eq!(read(&mut net, short, DIAG, &[0xFFFD]), vec![ok(0xFFFD, &[0x21, 3, 0])]);
}

#[test]
fn the_colour_attributes_of_a_tunable_white_and_a_colour_light() {
    let (mut net, _, short) = bulb_with(IDENTITY_TW);
    let ids = [0x0000, 0x0001, 0x0002, 0x0003, 0x0004, 0x0007, 0x0008, 0x000F, 0x0010, 0x4001, 0x400A, 0x400B, 0x400C, 0x400D, 0x4010, 0xFFFD];
    let tw = read(&mut net, short, COLOR, &ids);
    let ct_min = bulb_core::CT_MIN.to_le_bytes();
    let ct_max = bulb_core::CT_MAX.to_le_bytes();
    assert_eq!(tw[..5], [missing(0), missing(1), ok(2, &[0x21, 0, 0]), missing(3), missing(4)]);
    assert_eq!(tw[5].2[0], 0x21);
    assert_eq!(
        tw[6..],
        [
            ok(8, &[0x30, 2]),
            ok(0x0F, &[0x18, 1]),
            ok(0x10, &[0x20, 0]),
            ok(0x4001, &[0x30, 2]),
            ok(0x400A, &[0x19, 0x10, 0]),
            ok(0x400B, &[0x21, ct_min[0], ct_min[1]]),
            ok(0x400C, &[0x21, ct_max[0], ct_max[1]]),
            ok(0x400D, &[0x21, ct_min[0], ct_min[1]]),
            tw[14].clone(),
            ok(0xFFFD, &[0x21, 3, 0]),
        ]
    );
    assert_eq!(tw[14].2[0], 0x21);
    let (mut net, _, short) = bulb_with(IDENTITY_COLOR);
    let c = read(&mut net, short, COLOR, &ids);
    assert_eq!(c.iter().map(|r| r.1).collect::<Vec<_>>(), vec![0; 16]);
    assert_eq!(c[0].2[0], 0x20);
    assert_eq!(c[1].2[0], 0x20);
    assert_eq!(c[3].2[0], 0x21);
    assert_eq!(c[4].2[0], 0x21);
    assert_eq!(c[10], ok(0x400A, &[0x19, 0x19, 0]));
    assert_eq!(c[11], ok(0x400B, &[0x21, 153, 0]));
    assert_eq!(c[12], ok(0x400C, &[0x21, 0x72, 0x01]));
    assert_eq!(c[13], ok(0x400D, &[0x21, 153, 0]));
}

#[test]
fn the_ble_settings_attributes_and_their_revision() {
    let (mut net, _, short) = bulb_with(IDENTITY_BLE);
    assert_eq!(read(&mut net, short, SETTINGS, &[0xFFFD, 0x0003])[..], [ok(0xFFFD, &[0x21, 1, 0]), missing(3)]);
}

fn discover_commands(net: &mut Net, short: u16, cluster: u16, generated: bool, start: u8, max: u8) -> Vec<u8> {
    let cmd = if generated { 0x13 } else { 0x11 };
    let seq = net.coord.zcl(short, cluster, 0x00, cmd, &[start, max]);
    net.run(100);
    let rsp = net.coord.last_zcl(short, cluster, cmd + 1).unwrap();
    assert_eq!(rsp[..3], [0x18, seq, cmd + 1]);
    rsp[3..].to_vec()
}

#[test]
fn the_commands_each_cluster_receives_and_generates() {
    let (mut net, _, short) = bulb_with(IDENTITY_TW);
    let all = [
        (BASIC, vec![1, 0x00], vec![1]),
        (IDENTIFY, vec![1, 0x00, 0x01, 0x40], vec![1, 0x00]),
        (GROUPS, vec![1, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05], vec![1, 0x00, 0x01, 0x02, 0x03]),
        (SCENES, vec![1, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x40, 0x41, 0x42], vec![1, 0x00, 0x01, 0x02, 0x03, 0x04, 0x06, 0x40, 0x41, 0x42]),
        (ON_OFF, vec![1, 0x00, 0x01, 0x02, 0x40, 0x41, 0x42], vec![1]),
        (LEVEL, vec![1, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07], vec![1]),
        (COLOR, vec![1, 0x0A, 0x47, 0x4B, 0x4C], vec![1]),
        (TOUCHLINK, vec![1, 0x41, 0x42], vec![1, 0x41, 0x42]),
        (DIAG, vec![1], vec![1]),
    ];
    for (cluster, received, generated) in all {
        assert_eq!(discover_commands(&mut net, short, cluster, false, 0, 0xFF), received, "{cluster:#06x} received");
        assert_eq!(discover_commands(&mut net, short, cluster, true, 0, 0xFF), generated, "{cluster:#06x} generated");
    }
    let (mut net, _, short) = bulb_with(IDENTITY_COLOR);
    assert_eq!(
        discover_commands(&mut net, short, COLOR, false, 0, 0xFF),
        vec![1, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x47, 0x4B, 0x4C]
    );
    assert_eq!(discover_commands(&mut net, short, COLOR, false, 0x09, 2), vec![0, 0x09, 0x0A]);
    assert_eq!(discover_commands(&mut net, short, COLOR, false, 0x4C, 1), vec![1, 0x4C]);
    assert_eq!(discover_commands(&mut net, short, COLOR, true, 0, 0xFF), vec![1]);
}

#[test]
fn the_manufacturer_commands_are_the_diagnostics_clear_and_no_others() {
    let (mut net, _, short) = bulb_with(IDENTITY_BLE);
    for (cluster, received) in [(DIAG, vec![1, 0x00]), (SETTINGS, vec![1, 0x00])] {
        let seq = net.coord.zcl_mfr(short, cluster, 0x00, MFR, 0x11, &[0, 0xFF]);
        net.run(100);
        let rsp = net.coord.last_zcl_mfr(short, cluster, 0x12).unwrap();
        assert_eq!(rsp[..5], [0x1C, 0x41, 0x11, seq, 0x12]);
        assert_eq!(rsp[5..], received[..], "{cluster:#06x}");
        let seq = net.coord.zcl_mfr(short, cluster, 0x00, MFR, 0x13, &[0, 0xFF]);
        net.run(100);
        assert_eq!(net.coord.last_zcl_mfr(short, cluster, 0x14).unwrap(), vec![0x1C, 0x41, 0x11, seq, 0x14, 1]);
    }
}

#[test]
fn a_discover_command_cut_short_is_malformed() {
    let (mut net, _, short) = bulb_with(IDENTITY);
    for (cmd, payload) in [(0x11u8, vec![0u8]), (0x13, vec![]), (0x0C, vec![0, 0]), (0x15, vec![0])] {
        let seq = net.coord.zcl(short, ON_OFF, 0x00, cmd, &payload);
        net.run(100);
        assert_eq!(net.coord.last_zcl(short, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, cmd, 0x80], "command {cmd:#04x}");
    }
}

#[test]
fn a_default_response_is_not_answered_and_an_unknown_global_command_is_unsupported() {
    let (mut net, _, short) = bulb_with(IDENTITY);
    net.coord.zcl_rx.clear();
    net.coord.zcl(short, ON_OFF, 0x00, 0x0B, &[0x01, 0x00]);
    net.run(100);
    assert!(net.coord.zcl_rx.iter().all(|(s, _, _)| *s != short));
    for cmd in [0x0Eu8, 0x17, 0x40] {
        let seq = net.coord.zcl(short, ON_OFF, 0x00, cmd, &[]);
        net.run(100);
        assert_eq!(net.coord.last_zcl(short, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, cmd, 0x81]);
    }
}

#[test]
fn a_zcl_frame_cut_short_is_dropped() {
    let (mut net, _, short) = bulb_with(IDENTITY);
    for zcl in [vec![], vec![0x04], vec![0x04, 0x41], vec![0x04, 0x41, 0x11, 0x01], vec![0x01], vec![0x01, 0x05]] {
        let aps = net.coord.aps_frame(0x00, 1, ON_OFF, 0x0104, 1, &zcl);
        net.coord.nwk_data(short, &aps);
    }
    net.coord.zcl_rx.clear();
    net.run(200);
    assert!(net.coord.zcl_rx.iter().all(|(s, _, _)| *s != short));
}

#[test]
fn a_cluster_command_for_the_diagnostics_other_than_clear_and_one_for_an_object_endpoint_are_unsupported() {
    let (mut net, _, short) = bulb_with(IDENTITY);
    let seq = net.coord.zcl_mfr(short, DIAG, 0x01, MFR, 0x01, &[]);
    net.run(100);
    assert_eq!(net.coord.last_zcl_mfr(short, DIAG, 0x0B).unwrap(), vec![0x1C, 0x41, 0x11, seq, 0x0B, 0x01, 0x81]);
    for ep in [2u8, 7, 8] {
        let cluster = match ep {
            7 => 0x0010,
            8 => 0x000D,
            _ => 0x000C,
        };
        let seq = net.coord.zcl_ep(short, ep, cluster, 0x01, 0x00, &[]);
        net.run(100);
        assert_eq!(net.coord.last_zcl_ep(short, ep, cluster, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x00, 0x81], "endpoint {ep}");
    }
}
