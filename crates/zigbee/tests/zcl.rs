use bulb_core::command::Command;
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
    let (mut net, n, short) = bulb_with(Identity { app_version: 7, hw_version: 3, sw_build: "1.2.3", ble: true, ..IDENTITY });
    net.nodes[n].stack.set_ble_active(true);
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
    let (mut net, n, short) = bulb_with(IDENTITY_BLE);
    net.nodes[n].stack.set_ble_active(true);
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
    assert_eq!(read(&mut net, short, LEVEL, &[0x4000]), vec![ok(0x4000, &[0x20, 0xFF])], "the previous level at power-on");
    assert_eq!(read(&mut net, short, TOUCHLINK, &[0xFFFD, 0x0000]), vec![ok(0xFFFD, &[0x21, 3, 0]), missing(0)]);
    assert_eq!(read(&mut net, short, DIAG, &[0xFFFD]), vec![ok(0xFFFD, &[0x21, 3, 0])]);
}

#[test]
fn the_colour_attributes_of_a_tunable_white_and_a_colour_light() {
    let (mut net, n, short) = bulb_with(Identity { ble: true, ..IDENTITY_TW });
    net.nodes[n].stack.set_ble_active(true);
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
    for (cluster, received) in [(DIAG, vec![1, 0x00]), (SETTINGS, vec![1])] {
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
    net.coord.zcl_rx_ep.clear();
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
    net.coord.zcl_rx_ep.clear();
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

fn global(net: &mut Net, short: u16, cluster: u16, cmd: u8, payload: &[u8]) -> Option<Vec<u8>> {
    net.coord.zcl_rx.clear();
    net.coord.zcl_rx_ep.clear();
    let seq = net.coord.zcl(short, cluster, 0x00, cmd, payload);
    net.run(100);
    let rsp = net.coord.zcl_rx.iter().rev().find(|(s, c, f)| *s == short && *c == cluster && f[1] == seq)?.2.clone();
    assert_eq!(rsp[0], 0x18);
    Some(rsp[2..].to_vec())
}

#[test]
fn a_write_with_a_type_of_unknown_length_stops_at_that_record() {
    let (mut net, _, short) = bulb_with(IDENTITY);
    assert_eq!(
        global(&mut net, short, LEVEL, 0x02, &[0x11, 0x00, 0x99, 40, 0x11, 0x00, 0x20, 41]),
        Some(vec![0x04, 0x8D, 0x11, 0x00])
    );
    assert_eq!(read(&mut net, short, LEVEL, &[0x0011]), vec![ok(0x11, &[0x20, 0xFF])], "nor is the next record written");
    assert_eq!(global(&mut net, short, LEVEL, 0x02, &[0x11, 0x00, 0x20]), Some(vec![0x04, 0x8D, 0x11, 0x00]), "a value cut short");
    assert_eq!(
        global(&mut net, short, ON_OFF, 0x02, &[0x01, 0x40, 0x22, 1, 2, 3, 0x01, 0x40, 0x21, 30, 0]),
        Some(vec![0x04, 0x8D, 0x01, 0x40])
    );
    assert_eq!(read(&mut net, short, ON_OFF, &[0x4001]), vec![ok(0x4001, &[0x21, 30, 0])], "a three-byte value is skipped as one");
    assert_eq!(global(&mut net, short, BASIC, 0x02, &[0x06, 0x00, 0x42, 3, b'a', b'b', b'c']), Some(vec![0x04, 0x88, 0x06, 0x00]));
    assert_eq!(global(&mut net, short, BASIC, 0x02, &[0x0A, 0x00, 0x41, 0xFF]), Some(vec![0x04, 0x88, 0x0A, 0x00]), "an invalid string is no length");
}

#[test]
fn write_attributes_no_response_writes_without_an_answer() {
    let (mut net, _, short) = bulb_with(IDENTITY);
    assert_eq!(global(&mut net, short, ON_OFF, 0x05, &[0x02, 0x40, 0x21, 0x2C, 0x01]), None);
    assert_eq!(read(&mut net, short, ON_OFF, &[0x4002]), vec![ok(0x4002, &[0x21, 0x2C, 0x01])]);
    assert_eq!(global(&mut net, short, ON_OFF, 0x05, &[0x00, 0x00, 0x10, 0]), None, "nor for a failed one");
}

#[test]
fn an_undivided_write_with_a_record_cut_short_writes_nothing() {
    let (mut net, _, short) = bulb_with(IDENTITY);
    assert_eq!(global(&mut net, short, LEVEL, 0x03, &[0x11, 0x00, 0x20, 40, 0x0F, 0x00, 0x18]), Some(vec![0x04, 0x8D, 0x0F, 0x00]));
    assert_eq!(read(&mut net, short, LEVEL, &[0x0011]), vec![ok(0x11, &[0x20, 0xFF])]);
}

#[test]
fn configure_reporting_answers_each_failed_record_and_stops_at_one_cut_short() {
    let (mut net, _, short) = bulb_with(IDENTITY);
    assert_eq!(global(&mut net, short, ON_OFF, 0x06, &[0x01, 0x00, 0x00, 0x10, 0x00]), Some(vec![0x07, 0x00]), "a timeout for received reports");
    assert_eq!(global(&mut net, short, ON_OFF, 0x06, &[0x01, 0x00, 0x00, 0x10]), Some(vec![0x07, 0x00]), "its timeout cut short");
    assert_eq!(
        global(&mut net, short, ON_OFF, 0x06, &[0x00, 0x01, 0x00, 0x10, 1, 0, 2, 0, 0x00, 0x00, 0x40, 0x10, 1, 0, 2, 0]),
        Some(vec![0x07, 0x86, 0x00, 0x01, 0x00, 0x8C, 0x00, 0x00, 0x40])
    );
    assert_eq!(global(&mut net, short, ON_OFF, 0x06, &[0x00, 0x00, 0x00, 0x20, 1, 0, 2, 0, 0]), Some(vec![0x07, 0x8D, 0x00, 0x00, 0x00]));
    assert_eq!(global(&mut net, short, LEVEL, 0x06, &[0x00, 0x00, 0x00, 0x20, 1, 0, 2, 0]), Some(vec![0x07, 0x00]), "the change cut short");
    assert_eq!(global(&mut net, short, LEVEL, 0x06, &[0x00, 0x00, 0x00, 0x20, 1, 0]), Some(vec![0x07, 0x00]), "the interval cut short");
    assert_eq!(
        global(&mut net, short, LEVEL, 0x08, &[0x00, 0x00, 0x00]),
        Some(vec![0x09, 0x00, 0x00, 0x00, 0x00, 0x20, 1, 0, 0x10, 0x0E, 1]),
        "none of them changed the defaults"
    );
}

#[test]
fn read_reporting_configuration_of_what_is_not_reported() {
    let (mut net, _, short) = bulb_with(IDENTITY);
    assert_eq!(
        global(&mut net, short, ON_OFF, 0x08, &[0x01, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x09, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]),
        Some(vec![0x09, 0x8C, 0x01, 0x00, 0x00, 0x8C, 0x00, 0x00, 0x40, 0x86, 0x00, 0x09, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0, 0, 0x10, 0x0E])
    );
    assert_eq!(
        global(&mut net, short, DIAG, 0x08, &[0x00, 0x00, 0x01]),
        Some(vec![0x09, 0x00, 0x00, 0x00, 0x01, 0x23, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0]),
        "a reportable counter without a configuration"
    );
}

fn level_to(net: &mut Net, short: u16, level: u8) {
    net.coord.zcl(short, LEVEL, 0x11, 0x04, &[level, 0, 0, 0, 0]);
    net.run(50);
}

fn last_command(net: &mut Net, n: usize, short: u16, cluster: u16, cmd: u8, payload: &[u8]) -> Option<bulb_core::command::Command> {
    let before = net.nodes[n].commands.len();
    net.coord.zcl(short, cluster, 0x11, cmd, payload);
    net.run(50);
    net.nodes[n].commands[before..].last().copied()
}

#[test]
fn move_runs_at_the_given_rate_or_the_default_one_and_a_rate_of_zero_is_invalid() {
    use bulb_core::command::Command::MoveToLevel;
    let (mut net, n, short) = bulb_with(IDENTITY);
    level_to(&mut net, short, 100);
    assert_eq!(last_command(&mut net, n, short, LEVEL, 0x01, &[0x00, 0xFF, 0, 0]), Some(MoveToLevel { level: 254, transition_ms: 154 * 1000 / 50 }));
    level_to(&mut net, short, 100);
    assert_eq!(last_command(&mut net, n, short, LEVEL, 0x01, &[0x01, 10, 0, 0]), Some(MoveToLevel { level: 1, transition_ms: 99 * 100 }));
    level_to(&mut net, short, 100);
    assert_eq!(last_command(&mut net, n, short, LEVEL, 0x05, &[0x01, 20]), Some(MoveToLevel { level: 0, transition_ms: 100 * 1000 / 20 }));
    level_to(&mut net, short, 100);
    let seq = net.coord.zcl(short, LEVEL, 0x01, 0x01, &[0x00, 0x00, 0, 0]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, LEVEL, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x01, 0x87]);
}

#[test]
fn on_with_recall_global_scene_when_there_was_no_room_for_it_just_turns_on() {
    let (mut net, n, short) = bulb_with(IDENTITY);
    net.coord.zcl(short, GROUPS, 0x01, 0x00, &[0x34, 0x12, 0]);
    net.run(50);
    for id in 1..=8u8 {
        net.coord.zcl(short, SCENES, 0x01, 0x00, &[0x34, 0x12, id, 0, 0, 0]);
        net.run(50);
    }
    net.coord.zcl(short, ON_OFF, 0x11, 0x40, &[0x00, 0x00]);
    net.run(50);
    assert_eq!(read(&mut net, short, ON_OFF, &[0x4000]), vec![ok(0x4000, &[0x10, 0])]);
    assert_eq!(last_command(&mut net, n, short, ON_OFF, 0x41, &[]), Some(bulb_core::command::Command::On));
    assert_eq!(read(&mut net, short, ON_OFF, &[0x4000]), vec![ok(0x4000, &[0x10, 1])]);
}

#[test]
fn the_scene_table_holds_eight_scenes() {
    let (mut net, _, short) = bulb_with(IDENTITY);
    net.coord.zcl(short, GROUPS, 0x01, 0x00, &[0x34, 0x12, 0]);
    net.run(50);
    for id in 1..=8u8 {
        let seq = net.coord.zcl(short, SCENES, 0x01, 0x00, &[0x34, 0x12, id, 0, 0, 0]);
        net.run(50);
        assert_eq!(net.coord.last_zcl(short, SCENES, 0x00).unwrap(), vec![0x19, seq, 0x00, 0x00, 0x34, 0x12, id]);
    }
    let seq = net.coord.zcl(short, SCENES, 0x01, 0x00, &[0x34, 0x12, 9, 0, 0, 0]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, SCENES, 0x00).unwrap(), vec![0x19, seq, 0x00, 0x89, 0x34, 0x12, 9]);
    let seq = net.coord.zcl(short, SCENES, 0x01, 0x00, &[0x34, 0x12, 8, 0, 0, 0]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, SCENES, 0x00).unwrap(), vec![0x19, seq, 0x00, 0x00, 0x34, 0x12, 8], "one that is there is replaced");
    let seq = net.coord.zcl(short, SCENES, 0x01, 0x42, &[0x00, 0x34, 0x12, 1, 0x34, 0x12, 20]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, SCENES, 0x42).unwrap(), vec![0x19, seq, 0x42, 0x89, 0x34, 0x12, 1], "no room for the copy");
    let seq = net.coord.zcl(short, SCENES, 0x01, 0x02, &[0x34, 0x12, 20]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, SCENES, 0x02).unwrap(), vec![0x19, seq, 0x02, 0x8B, 0x34, 0x12, 20]);
    let seq = net.coord.zcl(short, SCENES, 0x01, 0x02, &[0x34, 0x12, 8]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, SCENES, 0x02).unwrap(), vec![0x19, seq, 0x02, 0x00, 0x34, 0x12, 8]);
    let seq = net.coord.zcl(short, SCENES, 0x01, 0x42, &[0x00, 0x34, 0x12, 1, 0x34, 0x12, 20]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, SCENES, 0x42).unwrap(), vec![0x19, seq, 0x42, 0x00, 0x34, 0x12, 1]);
}

#[test]
fn a_colour_scene_without_a_mode_takes_temperature_when_it_has_one_and_xy_otherwise() {
    use bulb_core::command::Command;
    let (mut net, n, short) = bulb_with(IDENTITY_COLOR);
    net.coord.zcl(short, GROUPS, 0x01, 0x00, &[0x34, 0x12, 0]);
    net.run(50);
    let scene = |id: u8, ct: u16| {
        let mut p = vec![0x34, 0x12, id, 0, 0, 0, 0x00, 0x03, 13];
        p.extend_from_slice(&[0x10, 0x20, 0x30, 0x40, 0x00, 0x80, 100, 0, 0, 0, 0]);
        p.extend_from_slice(&ct.to_le_bytes());
        p
    };
    net.coord.zcl(short, SCENES, 0x11, 0x00, &scene(1, 300));
    net.coord.zcl(short, SCENES, 0x11, 0x00, &scene(2, 0));
    net.run(50);
    assert_eq!(last_command(&mut net, n, short, SCENES, 0x05, &[0x34, 0x12, 1]), Some(Command::MoveToColorTemperature { mireds: 300, transition_ms: 0 }));
    assert_eq!(last_command(&mut net, n, short, SCENES, 0x05, &[0x34, 0x12, 2]), Some(Command::MoveToXy { x: 0x2010, y: 0x4030, transition_ms: 0 }));
}

#[test]
fn the_identify_time_counts_down_to_zero_and_stays_there() {
    let (mut net, _, short) = bulb_with(IDENTITY);
    net.coord.zcl(short, IDENTIFY, 0x01, 0x00, &[3, 0]);
    net.run(1_100);
    assert_eq!(read(&mut net, short, IDENTIFY, &[0x0000]), vec![ok(0, &[0x21, 2, 0])]);
    net.run(2_500);
    assert_eq!(read(&mut net, short, IDENTIFY, &[0x0000]), vec![ok(0, &[0x21, 0, 0])]);
    net.run(60_000);
    assert_eq!(read(&mut net, short, IDENTIFY, &[0x0000]), vec![ok(0, &[0x21, 0, 0])]);
}

fn light_clusters(net: &mut Net, short: u16) -> Vec<u16> {
    let s = short.to_le_bytes();
    net.coord.zdo(short, 0x0004, &[s[0], s[1], 1]);
    net.run(100);
    let d = net.coord.last_zdo(short, 0x8004).unwrap();
    let n = d[11] as usize;
    d[12..12 + 2 * n].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect()
}

#[test]
fn the_light_endpoint_serves_the_clusters_of_its_variant() {
    const OCC: u16 = 0x0406;
    const SENSE: u16 = 0xFC5E;
    let base = [BASIC, IDENTIFY, GROUPS, SCENES, ON_OFF, LEVEL];
    for (tw, ble, sense) in [(true, true, false), (true, false, true), (false, true, true), (true, true, true)] {
        let (mut net, _, short) = bulb_with(Identity { color_temperature: tw, ble, sense, ..IDENTITY });
        let mut want = base.to_vec();
        if tw {
            want.push(COLOR);
        }
        want.extend_from_slice(&[DIAG, TOUCHLINK]);
        if ble {
            want.push(SETTINGS);
        }
        if sense {
            want.extend_from_slice(&[OCC, SENSE]);
        }
        assert_eq!(light_clusters(&mut net, short), want, "tw {tw} ble {ble} sense {sense}");
    }
}

#[test]
fn writing_the_identify_time_starts_identifying() {
    let (mut net, n, short) = bulb_with(IDENTITY);
    assert_eq!(global(&mut net, short, IDENTIFY, 0x02, &[0x00, 0x00, 0x21, 5, 0]), Some(vec![0x04, 0x00]));
    assert_eq!(net.nodes[n].commands.last(), Some(&bulb_core::command::Command::Identify { times: 5 }));
    assert_eq!(read(&mut net, short, IDENTIFY, &[0x0000]), vec![ok(0, &[0x21, 5, 0])]);
}

fn discover(net: &mut Net, short: u16, ep: u8, cluster: u16, mfr: bool, start: u16, max: u8) -> Vec<u8> {
    let p = [&start.to_le_bytes()[..], &[max]].concat();
    net.coord.zcl_rx.clear();
    net.coord.zcl_rx_ep.clear();
    let seq = if mfr { net.coord.zcl_mfr(short, cluster, 0x00, MFR, 0x15, &p) } else { net.coord.zcl_ep(short, ep, cluster, 0x00, 0x15, &p) };
    net.run(100);
    let rsp = net.coord.zcl_rx.iter().rev().find(|(s, c, f)| *s == short && *c == cluster && f.contains(&seq)).unwrap().2.clone();
    let at = if mfr { 5 } else { 3 };
    assert_eq!(rsp[at - 1], 0x16);
    rsp[at..].to_vec()
}

#[test]
fn the_attributes_of_an_object_and_of_the_sense_clusters_can_be_discovered() {
    let (mut net, _, short) = bulb_with(Identity { sense: true, ..IDENTITY });
    assert_eq!(
        discover(&mut net, short, 7, 0x0010, false, 0, 20),
        vec![1, 0x1C, 0x00, 0x42, 0x01, 0x51, 0x00, 0x10, 0x01, 0x55, 0x00, 0x10, 0x07, 0x6F, 0x00, 0x18, 0x05, 0xFD, 0xFF, 0x21, 0x01]
    );
    assert_eq!(discover(&mut net, short, 7, 0x0010, false, 0x52, 1), vec![0, 0x55, 0x00, 0x10, 0x07]);
    assert_eq!(
        discover(&mut net, short, 1, 0x0406, false, 0, 3),
        vec![0, 0x00, 0x00, 0x18, 0x05, 0x01, 0x00, 0x30, 0x01, 0x02, 0x00, 0x18, 0x01]
    );
    assert_eq!(discover(&mut net, short, 1, 0x0406, false, 0x0010, 9), vec![1, 0x10, 0x00, 0x21, 0x03, 0xFD, 0xFF, 0x21, 0x01]);
}

#[test]
fn writes_to_the_diagnostics_and_to_objects_answer_what_is_wrong() {
    let (mut net, _, short) = bulb_with(IDENTITY);
    assert_eq!(global(&mut net, short, DIAG, 0x02, &[0x00, 0x00, 0x21, 1, 0]), Some(vec![0x04, 0x88, 0x00, 0x00]));
    assert_eq!(global(&mut net, short, DIAG, 0x02, &[0x00, 0x00, 0x20, 1]), Some(vec![0x04, 0x8D, 0x00, 0x00]));
    assert_eq!(global(&mut net, short, DIAG, 0x02, &[0x99, 0x00, 0x20, 1]), Some(vec![0x04, 0x86, 0x99, 0x00]));
    let seq = net.coord.zcl_ep(short, 7, 0x0010, 0x00, 0x02, &[0x99, 0x00, 0x10, 1]);
    net.run(100);
    assert_eq!(net.coord.last_zcl_ep(short, 7, 0x0010, 0x04).unwrap(), vec![0x18, seq, 0x04, 0x86, 0x99, 0x00]);
}

#[test]
fn the_object_reports_fill_a_table_of_eleven() {
    let (mut net, _, short) = bulb_with(IDENTITY);
    let mut statuses = vec![];
    for ep in 2..=6u8 {
        for attr in [0x0055u16, 0x006F] {
            let a = attr.to_le_bytes();
            let ty = if attr == 0x0055 { 0x39 } else { 0x18 };
            let mut p = vec![0x00, a[0], a[1], ty, 1, 0, 0x10, 0x0E];
            if ty == 0x39 {
                p.extend_from_slice(&1.0f32.to_le_bytes());
            }
            let seq = net.coord.zcl_ep(short, ep, 0x000C, 0x00, 0x06, &p);
            net.run(60);
            statuses.push(net.coord.last_zcl_ep(short, ep, 0x000C, 0x07).unwrap()[1..].to_vec() == vec![seq, 0x07, 0x00]);
        }
    }
    assert!(statuses.iter().all(|s| *s), "ten fit");
    let seq = net.coord.zcl_ep(short, 7, 0x0010, 0x00, 0x06, &[0x00, 0x55, 0x00, 0x10, 1, 0, 0x10, 0x0E]);
    net.run(60);
    assert_eq!(net.coord.last_zcl_ep(short, 7, 0x0010, 0x07).unwrap(), vec![0x18, seq, 0x07, 0x00], "the eleventh");
    let seq = net.coord.zcl_ep(short, 7, 0x0010, 0x00, 0x06, &[0x00, 0x6F, 0x00, 0x18, 1, 0, 0x10, 0x0E]);
    net.run(60);
    assert_eq!(net.coord.last_zcl_ep(short, 7, 0x0010, 0x07).unwrap(), vec![0x18, seq, 0x07, 0x89, 0x00, 0x6F, 0x00], "no room for a twelfth");
}

#[test]
fn a_scene_extension_for_a_cluster_without_scenes_is_skipped() {
    let (mut net, _, short) = bulb_with(IDENTITY);
    net.coord.zcl(short, GROUPS, 0x01, 0x00, &[0x34, 0x12, 0]);
    net.run(50);
    let seq = net.coord.zcl(short, SCENES, 0x01, 0x00, &[0x34, 0x12, 1, 0, 0, 0, 0x05, 0x0B, 2, 9, 9, 0x08, 0x00, 1, 33]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, SCENES, 0x00).unwrap(), vec![0x19, seq, 0x00, 0x00, 0x34, 0x12, 1]);
    net.coord.zcl(short, SCENES, 0x01, 0x01, &[0x34, 0x12, 1]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, SCENES, 0x01).unwrap()[10..], [0x08, 0x00, 1, 33], "only the level");
}

#[test]
fn the_group_table_holds_eight_groups() {
    let (mut net, _, short) = bulb_with(IDENTITY);
    for g in 1..=8u16 {
        let seq = net.coord.zcl(short, GROUPS, 0x01, 0x00, &[g as u8, 0, 0]);
        net.run(50);
        assert_eq!(net.coord.last_zcl(short, GROUPS, 0x00).unwrap(), vec![0x19, seq, 0x00, 0x00, g as u8, 0]);
    }
    let seq = net.coord.zcl(short, GROUPS, 0x01, 0x00, &[9, 0, 0]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, GROUPS, 0x00).unwrap(), vec![0x19, seq, 0x00, 0x89, 9, 0]);
}

#[test]
fn switching_off_on_or_toggling_ends_the_remaining_time_of_a_transition() {
    for cmd in [0x00u8, 0x01, 0x02] {
        let (mut net, _, short) = bulb_with(IDENTITY_TW);
        net.coord.zcl(short, LEVEL, 0x11, 0x00, &[100, 100, 0]);
        net.coord.zcl(short, COLOR, 0x11, 0x0A, &[0x2C, 0x01, 100, 0]);
        net.run(1_000);
        assert_ne!(read(&mut net, short, LEVEL, &[0x0001]), vec![ok(1, &[0x21, 0, 0])]);
        net.coord.zcl(short, ON_OFF, 0x11, cmd, &[]);
        net.run(100);
        assert_eq!(read(&mut net, short, LEVEL, &[0x0001]), vec![ok(1, &[0x21, 0, 0])], "command {cmd}");
        assert_eq!(read(&mut net, short, COLOR, &[0x0002]), vec![ok(2, &[0x21, 0, 0])], "command {cmd}");
    }
}

#[test]
fn a_string_written_to_a_number_is_skipped_whole_and_the_next_record_is_written() {
    let (mut net, _, short) = bulb_with(IDENTITY);
    assert_eq!(
        global(&mut net, short, ON_OFF, 0x02, &[0x01, 0x40, 0x42, 3, b'a', b'b', b'c', 0x02, 0x40, 0x21, 0x2C, 0x01]),
        Some(vec![0x04, 0x8D, 0x01, 0x40])
    );
    assert_eq!(read(&mut net, short, ON_OFF, &[0x4002]), vec![ok(0x4002, &[0x21, 0x2C, 0x01])]);
}

#[test]
fn start_up_on_off_takes_only_its_defined_values() {
    let (mut net, _, short) = bulb_with(IDENTITY);
    for v in [0u8, 1, 2, 0xFF] {
        assert_eq!(global(&mut net, short, ON_OFF, 0x02, &[0x03, 0x40, 0x30, v]), Some(vec![0x04, 0x00]), "{v}");
    }
    for v in [3u8, 0x80, 0xFE] {
        assert_eq!(global(&mut net, short, ON_OFF, 0x02, &[0x03, 0x40, 0x30, v]), Some(vec![0x04, 0x87, 0x03, 0x40]), "{v}");
    }
}

#[test]
fn the_firmware_sets_the_options_only_where_the_cluster_is() {
    let (mut net, n, short) = bulb_with(IDENTITY_TW);
    assert!(net.nodes[n].stack.set_color_options(0x00));
    assert!(!net.nodes[n].stack.set_color_options(0x04), "a reserved bit");
    assert_eq!(read(&mut net, short, COLOR, &[0x000F]), vec![ok(0x0F, &[0x18, 0x00])]);
    let (mut net, n, _) = bulb_with(IDENTITY);
    assert!(!net.nodes[n].stack.set_color_options(0x01), "no Color Control on a dimmable light");
    net.run(1);
}

#[test]
fn a_broadcast_global_command_is_answered() {
    let (mut net, _, short) = bulb_with(IDENTITY);
    net.coord.zcl_rx.clear();
    net.coord.zcl_rx_ep.clear();
    let seq = net.coord.zcl(0xFFFD, ON_OFF, 0x00, 0x00, &[0x00, 0x00]);
    net.run(500);
    assert_eq!(net.coord.last_zcl(short, ON_OFF, 0x01).unwrap()[..3], [0x18, seq, 0x01]);
}

#[test]
fn on_level_zero_and_a_start_up_colour_temperature_out_of_range_are_invalid() {
    let (mut net, _, short) = bulb_with(IDENTITY_TW);
    assert_eq!(global(&mut net, short, LEVEL, 0x02, &[0x11, 0x00, 0x20, 0]), Some(vec![0x04, 0x87, 0x11, 0x00]));
    assert_eq!(global(&mut net, short, LEVEL, 0x02, &[0x11, 0x00, 0x20, 1]), Some(vec![0x04, 0x00]));
    let (lo, hi) = (bulb_core::CT_MIN, bulb_core::CT_MAX);
    for (v, ok) in [(0xFFFFu16, true), (lo, true), (hi, true), (lo - 1, false), (hi + 1, false)] {
        let b = v.to_le_bytes();
        let want = if ok { vec![0x04, 0x00] } else { vec![0x04, 0x87, 0x10, 0x40] };
        assert_eq!(global(&mut net, short, COLOR, 0x02, &[0x10, 0x40, 0x21, b[0], b[1]]), Some(want), "{v}");
    }
}

#[test]
fn the_colour_mode_is_reportable_on_a_colour_light_only() {
    for (identity, status) in [(IDENTITY_TW, vec![0x07, 0x8C, 0x00, 0x08, 0x00]), (IDENTITY_COLOR, vec![0x07, 0x00])] {
        let (mut net, _, short) = bulb_with(identity);
        assert_eq!(global(&mut net, short, COLOR, 0x06, &[0x00, 0x08, 0x00, 0x30, 1, 0, 0x10, 0x0E]), Some(status));
    }
}

#[test]
fn a_read_answers_as_many_attributes_as_fit_in_the_longest_answer() {
    let (mut net, _, short) = bulb_with(IDENTITY);
    for (asked, answered) in [(70u16, 70usize), (100, 70)] {
        let request: Vec<u8> = (0..asked).flat_map(|i| (0x2000 + i).to_le_bytes()).collect();
        let seq = net.coord.zcl_fragmented(short, BASIC, 0x00, 0x00, &request, 50);
        net.run(2_000);
        let rsp = net.coord.last_zcl(short, BASIC, 0x01).unwrap();
        assert_eq!(rsp[1], seq);
        assert_eq!(rsp.len(), 3 + 3 * answered, "{asked} asked");
        assert!(rsp[3..].chunks(3).enumerate().all(|(i, r)| r == [(0x2000 + i as u16) as u8, 0x20, 0x86]));
    }
    let seq = net.coord.zcl_mfr(short, DIAG, 0x00, MFR, 0x00, &[0x50, 0x50, 0x51, 0x50, 0x52, 0x50, 0x53, 0x50]);
    net.run(300);
    let rsp = net.coord.last_zcl_mfr(short, DIAG, 0x01).unwrap();
    assert_eq!(rsp[3], seq);
    let records = read_records(&rsp[5..]);
    assert_eq!(records.iter().map(|r| r.0).collect::<Vec<u16>>(), vec![0x5050, 0x5051, 0x5052], "room for three pages of 65 bytes; a fourth is left for the next read");
}

#[test]
fn a_reportable_change_of_two_bytes_is_kept_whole() {
    let (mut net, _, short) = bulb_with(IDENTITY_TW);
    assert_eq!(global(&mut net, short, COLOR, 0x06, &[0x00, 0x07, 0x00, 0x21, 1, 0, 0x10, 0x0E, 0x02, 0x01]), Some(vec![0x07, 0x00]));
    assert_eq!(
        global(&mut net, short, COLOR, 0x08, &[0x00, 0x07, 0x00]),
        Some(vec![0x09, 0x00, 0x00, 0x07, 0x00, 0x21, 1, 0, 0x10, 0x0E, 0x02, 0x01])
    );
}

#[test]
fn the_sense_cluster_has_no_manufacturer_commands_to_discover() {
    let (mut net, _, short) = bulb_with(Identity { sense: true, ..IDENTITY });
    let seq = net.coord.zcl_mfr(short, 0xFC5E, 0x00, MFR, 0x11, &[0, 0xFF]);
    net.run(100);
    assert_eq!(net.coord.last_zcl_mfr(short, 0xFC5E, 0x12).unwrap(), vec![0x1C, 0x41, 0x11, seq, 0x12, 1]);
}

#[test]
fn remove_all_groups_keeps_the_global_scene() {
    let (mut net, _, short) = bulb_with(IDENTITY);
    net.coord.zcl(short, GROUPS, 0x01, 0x00, &[0x34, 0x12, 0]);
    net.run(50);
    net.coord.zcl(short, SCENES, 0x01, 0x00, &[0x34, 0x12, 1, 0, 0, 0]);
    net.coord.zcl(short, ON_OFF, 0x11, 0x40, &[0x00, 0x00]);
    net.run(50);
    assert_eq!(read(&mut net, short, SCENES, &[0x0000]), vec![ok(0, &[0x20, 2])]);
    net.coord.zcl(short, GROUPS, 0x01, 0x04, &[]);
    net.run(50);
    assert_eq!(read(&mut net, short, SCENES, &[0x0000]), vec![ok(0, &[0x20, 1])], "the global scene stays");
}

#[test]
fn on_with_recall_global_scene_ends_a_waiting_off_guard_when_there_is_no_on_time() {
    let (mut net, _, short) = bulb_with(IDENTITY);
    net.coord.zcl(short, ON_OFF, 0x11, 0x42, &[0x00, 0x00, 0x00, 0x1E, 0x00]);
    net.run(50);
    net.coord.zcl(short, ON_OFF, 0x11, 0x40, &[0x00, 0x00]);
    net.run(50);
    assert_ne!(read(&mut net, short, ON_OFF, &[0x4002]), vec![ok(0x4002, &[0x21, 0, 0])]);
    net.coord.zcl(short, ON_OFF, 0x11, 0x41, &[]);
    net.run(50);
    assert_eq!(read(&mut net, short, ON_OFF, &[0x4002]), vec![ok(0x4002, &[0x21, 0, 0])]);
}

#[test]
fn step_moves_by_its_size_in_a_time_scaled_to_how_far_it_gets() {
    use bulb_core::command::Command::MoveToLevel;
    let (mut net, n, short) = bulb_with(IDENTITY);
    level_to(&mut net, short, 100);
    assert_eq!(last_command(&mut net, n, short, LEVEL, 0x02, &[0x01, 30, 10, 0]), Some(MoveToLevel { level: 70, transition_ms: 1_000 }));
    net.run(1_000);
    assert_eq!(last_command(&mut net, n, short, LEVEL, 0x02, &[0x00, 200, 10, 0]), Some(MoveToLevel { level: 254, transition_ms: 1_000 * 184 / 200 }));
    net.run(1_000);
    assert_eq!(last_command(&mut net, n, short, LEVEL, 0x02, &[0x01, 0, 10, 0]), Some(MoveToLevel { level: 254, transition_ms: 0 }), "a step of nothing");
    assert_eq!(last_command(&mut net, n, short, LEVEL, 0x02, &[0x01, 54, 0xFF, 0xFF]), Some(MoveToLevel { level: 200, transition_ms: 0 }), "as fast as it can");
}

#[test]
fn a_tunable_white_coupled_to_the_level_does_not_follow_it_to_off() {
    use bulb_core::command::Command;
    let (mut net, n, short) = bulb_with(IDENTITY_TW);
    assert_eq!(global(&mut net, short, LEVEL, 0x02, &[0x0F, 0x00, 0x18, 0x02]), Some(vec![0x04, 0x00]));
    let before = net.nodes[n].commands.len();
    net.coord.zcl(short, LEVEL, 0x11, 0x04, &[0, 10, 0]);
    net.run(50);
    assert!(net.nodes[n].commands[before..].iter().all(|c| !matches!(c, Command::MoveToColorTemperature { .. })), "{:?}", &net.nodes[n].commands[before..]);
}

#[test]
fn a_scene_group_of_0xfff7_is_a_group_and_0xfff8_is_none() {
    let (mut net, _, short) = bulb_with(IDENTITY);
    for (group, status) in [(0xFFF7u16, 0x85u8), (0xFFF8, 0x87)] {
        let g = group.to_le_bytes();
        let seq = net.coord.zcl(short, SCENES, 0x01, 0x01, &[g[0], g[1], 1]);
        net.run(50);
        assert_eq!(net.coord.last_zcl(short, SCENES, 0x01).unwrap()[..4], [0x19, seq, 0x01, status], "group {group:#06x}");
    }
}

#[test]
fn on_and_toggle_end_a_waiting_off_guard_when_there_is_no_on_time() {
    for cmd in [0x01u8, 0x02] {
        let (mut net, _, short) = bulb_with(IDENTITY);
        net.coord.zcl(short, ON_OFF, 0x11, 0x42, &[0x00, 0x00, 0x00, 0x1E, 0x00]);
        net.run(50);
        net.coord.zcl(short, ON_OFF, 0x11, 0x00, &[]);
        net.run(50);
        assert_ne!(read(&mut net, short, ON_OFF, &[0x4002]), vec![ok(0x4002, &[0x21, 0, 0])]);
        net.coord.zcl(short, ON_OFF, 0x11, cmd, &[]);
        net.run(50);
        assert_eq!(read(&mut net, short, ON_OFF, &[0x4002]), vec![ok(0x4002, &[0x21, 0, 0])], "command {cmd}");
    }
}

#[test]
fn the_enhanced_forms_of_remove_store_recall_and_membership_are_not_supported() {
    let (mut net, _, short) = bulb_with(IDENTITY);
    net.coord.zcl(short, GROUPS, 0x01, 0x00, &[0x34, 0x12, 0]);
    net.run(50);
    for cmd in [0x43u8, 0x44, 0x45, 0x46] {
        let seq = net.coord.zcl(short, SCENES, 0x01, cmd, &[0x34, 0x12, 1]);
        net.run(50);
        assert_eq!(net.coord.last_zcl(short, SCENES, 0x0B).unwrap(), vec![0x18, seq, 0x0B, cmd, 0x81], "command {cmd:#04x}");
    }
}

#[test]
fn a_scene_with_a_name_a_short_colour_set_and_an_explicit_colour_mode() {
    use bulb_core::command::{Command, HueDirection};
    let (mut net, n, short) = bulb_with(IDENTITY_COLOR);
    net.coord.zcl(short, GROUPS, 0x01, 0x00, &[0x34, 0x12, 0]);
    net.run(50);
    let colour = |mode: u8| vec![0x00, 0x03, 14, 0x10, 0x20, 0x30, 0x40, 0x00, 0x80, 100, 0, 0, 0, 0, 0x2C, 0x01, mode];
    let mut named = vec![0x34, 0x12, 1, 0, 0, 2, b'a', b'b', 0x08, 0x00, 1, 42];
    named.extend_from_slice(&[0x00, 0x03, 3, 1, 2, 3]);
    net.coord.zcl(short, SCENES, 0x01, 0x00, &named);
    let mut hue = vec![0x34, 0x12, 2, 0, 0, 0];
    hue.extend_from_slice(&colour(0));
    net.coord.zcl(short, SCENES, 0x01, 0x00, &hue);
    let mut odd = vec![0x34, 0x12, 3, 0, 0, 0];
    odd.extend_from_slice(&colour(3));
    net.coord.zcl(short, SCENES, 0x01, 0x00, &odd);
    net.run(100);
    let recall = |net: &mut Net, id: u8| {
        let before = net.nodes[n].commands.len();
        net.coord.zcl(short, SCENES, 0x01, 0x05, &[0x34, 0x12, id]);
        net.run(50);
        net.nodes[n].commands[before..].to_vec()
    };
    assert_eq!(recall(&mut net, 1), vec![Command::MoveToLevel { level: 42, transition_ms: 0 }], "the name skipped, a colour set without y ignored");
    assert_eq!(recall(&mut net, 2), vec![Command::MoveToHueSaturation { hue: 128, sat: 100, direction: HueDirection::Shortest, transition_ms: 0 }]);
    assert_eq!(recall(&mut net, 3), vec![Command::MoveToColorTemperature { mireds: 300, transition_ms: 0 }], "a reserved mode: by the temperature");
}

#[test]
fn remove_all_scenes_removes_those_of_its_group_only() {
    let (mut net, _, short) = bulb_with(IDENTITY);
    for g in [0x34u8, 0x35] {
        net.coord.zcl(short, GROUPS, 0x01, 0x00, &[g, 0x12, 0]);
        net.coord.zcl(short, SCENES, 0x01, 0x00, &[g, 0x12, 1, 0, 0, 0]);
        net.run(50);
    }
    net.coord.zcl(short, SCENES, 0x01, 0x03, &[0x34, 0x12]);
    net.run(50);
    assert_eq!(read(&mut net, short, SCENES, &[0x0000]), vec![ok(0, &[0x20, 1])]);
    let seq = net.coord.zcl(short, SCENES, 0x01, 0x01, &[0x35, 0x12, 1]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, SCENES, 0x01).unwrap()[..4], [0x19, seq, 0x01, 0x00]);
}

#[test]
fn a_store_scene_with_no_room_leaves_the_current_scene_alone() {
    let (mut net, _, short) = bulb_with(IDENTITY);
    net.coord.zcl(short, GROUPS, 0x01, 0x00, &[0x34, 0x12, 0]);
    for id in 1..=8u8 {
        net.coord.zcl(short, SCENES, 0x01, 0x00, &[0x34, 0x12, id, 0, 0, 0]);
    }
    net.run(100);
    let seq = net.coord.zcl(short, SCENES, 0x01, 0x04, &[0x34, 0x12, 9]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, SCENES, 0x04).unwrap(), vec![0x19, seq, 0x04, 0x89, 0x34, 0x12, 9]);
    assert_eq!(read(&mut net, short, SCENES, &[1, 2, 3]), vec![ok(1, &[0x20, 0]), ok(2, &[0x21, 0, 0]), ok(3, &[0x10, 0])]);
}

#[test]
fn move_color_temperature_stops_at_the_bounds_it_is_given_or_the_lights_own() {
    use bulb_core::command::Command;
    let (mut net, n, short) = bulb_with(IDENTITY_TW);
    let target = |c: Option<bulb_core::command::Command>| match c {
        Some(Command::MoveToColorTemperature { mireds, .. }) => mireds,
        other => panic!("{other:?}"),
    };
    assert_eq!(target(last_command(&mut net, n, short, COLOR, 0x4B, &[3, 10, 0, 200, 0, 0, 0])), 200);
    assert_eq!(target(last_command(&mut net, n, short, COLOR, 0x4B, &[3, 10, 0, 0, 0, 0, 0])), bulb_core::CT_MIN);
    assert_eq!(target(last_command(&mut net, n, short, COLOR, 0x4B, &[1, 10, 0, 0, 0, 0x2C, 0x01])), 300);
    assert_eq!(target(last_command(&mut net, n, short, COLOR, 0x4B, &[1, 10, 0, 0, 0, 0, 0])), bulb_core::CT_MAX);
}

#[test]
fn a_change_is_reported_no_sooner_than_the_minimum_interval_after_the_last_report() {
    let (mut net, n, short) = bulb_with(IDENTITY);
    let ext = net.air.borrow().ports[net.nodes[n].id].ext;
    let mut bind = ext.to_le_bytes().to_vec();
    bind.push(1);
    bind.extend_from_slice(&LEVEL.to_le_bytes());
    bind.push(3);
    bind.extend_from_slice(&COORD_EXT.to_le_bytes());
    bind.push(1);
    net.coord.zdo(short, 0x0021, &bind);
    net.run(100);
    assert_eq!(global(&mut net, short, LEVEL, 0x06, &[0x00, 0x00, 0x00, 0x20, 5, 0, 0x10, 0x0E, 1]), Some(vec![0x07, 0x00]));
    net.run(6_000);
    let t = net.now();
    net.coord.zcl(short, LEVEL, 0x11, 0x00, &[60, 0, 0]);
    net.run(200);
    net.coord.zcl(short, LEVEL, 0x11, 0x00, &[90, 0, 0]);
    let times: Vec<u32> = {
        net.run(7_000);
        net.timed_nwk_sent_by(net.nodes[n].id, t)
            .into_iter()
            .filter(|(_, _, h, p)| h.dst == 0x0000 && p.len() > 11 && p[2..4] == LEVEL.to_le_bytes() && p[10] == 0x0A)
            .map(|e| e.0)
            .collect()
    };
    assert_eq!(times.len(), 2, "{times:?}");
    assert!(times[1] - times[0] >= 5_000 && times[1] - times[0] < 6_000, "{times:?}");
}

#[test]
fn the_power_source_is_mains() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let seq = net.coord.zcl(short, 0x0000, 0x00, 0x00, &[0x07, 0x00]);
    net.run(100);
    assert_eq!(net.coord.last_zcl(short, 0x0000, 0x01).unwrap(), vec![0x18, seq, 0x01, 0x07, 0x00, 0x00, 0x30, 0x01]);
}

fn read_u16(net: &mut Net, short: u16, cluster: u16, id: u16) -> u16 {
    let seq = net.coord.zcl(short, cluster, 0x00, 0x00, &id.to_le_bytes());
    net.run(100);
    let rsp = net.coord.last_zcl(short, cluster, 0x01).unwrap();
    assert_eq!(rsp[1], seq);
    u16::from_le_bytes([rsp[7], rsp[8]])
}

#[test]
fn recalling_the_global_scene_clears_the_off_wait_only_without_an_on_time() {
    for on_time in [0u16, 30] {
        let mut net = Net::new();
        let n = net.join_node();
        let short = net.nodes[n].short();
        net.coord.zcl(short, 0x0006, 0x01, 0x40, &[0, 0]);
        net.run(100);
        let mut w = vec![0x01, 0x40, 0x21];
        w.extend_from_slice(&on_time.to_le_bytes());
        w.extend_from_slice(&[0x02, 0x40, 0x21, 50, 0]);
        net.coord.zcl(short, 0x0006, 0x00, 0x02, &w);
        net.run(100);
        net.coord.zcl(short, 0x0006, 0x01, 0x41, &[]);
        net.run(100);
        assert_eq!(read_u16(&mut net, short, 0x0006, 0x4002) != 0, on_time != 0, "on time {on_time}");
    }
}

#[test]
fn removing_all_groups_keeps_the_scenes_of_no_group() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    net.coord.zcl(short, 0x0004, 0x01, 0x00, &[0x34, 0x12, 0]);
    net.run(100);
    net.coord.zcl(short, 0x0005, 0x01, 0x04, &[0x00, 0x00, 1]);
    net.coord.zcl(short, 0x0005, 0x01, 0x04, &[0x34, 0x12, 2]);
    net.run(100);
    net.coord.zcl(short, 0x0004, 0x01, 0x04, &[]);
    net.run(100);
    let count = |net: &mut Net, group: u16| {
        let g = group.to_le_bytes();
        net.coord.zcl(short, 0x0005, 0x01, 0x06, &[g[0], g[1]]);
        net.run(100);
        let rsp = net.coord.last_zcl(short, 0x0005, 0x06).unwrap();
        (rsp[3], rsp.get(7).copied().unwrap_or(0))
    };
    assert_eq!(count(&mut net, 0x0000).1, 1, "the scene of no group stays");
    assert_ne!(count(&mut net, 0x1234).0, 0x00, "the group is gone with its scene");
}

#[test]
fn a_copy_scene_is_no_remove_scene() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    net.coord.zcl(short, 0x0005, 0x01, 0x04, &[0x00, 0x00, 1]);
    net.run(100);
    net.coord.zcl(short, 0x0005, 0x01, 0x42, &[0x00, 0x00, 0x01, 3, 0x00, 0x00, 2]);
    net.run(100);
    net.coord.zcl(short, 0x0005, 0x01, 0x01, &[0x00, 0x00, 1]);
    net.run(100);
    assert_eq!(net.coord.last_zcl(short, 0x0005, 0x01).unwrap()[3], 0x00, "scene 1 is still there");
}

#[test]
fn a_scene_added_with_an_unknown_colour_mode_takes_the_mode_its_fields_give() {
    let mut net = Net::new();
    net.identity = IDENTITY_COLOR;
    let n = net.join_node();
    let short = net.nodes[n].short();
    for (mode, ct, want) in [(3u8, 300u16, 2u8), (3, 0, 1), (0, 300, 0)] {
        let mut p = vec![0x00, 0x00, 5, 0x00, 0x00, 0x00, 0x00, 0x03, 14];
        p.extend_from_slice(&[0x00, 0x40, 0x00, 0x50, 0x00, 0x60, 0x80, 0, 0, 0, 0]);
        p.extend_from_slice(&ct.to_le_bytes());
        p.push(mode);
        net.coord.zcl(short, 0x0005, 0x01, 0x00, &p);
        net.run(100);
        net.coord.zcl(short, 0x0005, 0x01, 0x01, &[0x00, 0x00, 5]);
        net.run(100);
        let rsp = net.coord.last_zcl(short, 0x0005, 0x01).unwrap();
        assert_eq!((rsp[3], *rsp.last().unwrap()), (0x00, want), "mode {mode}, colour temperature {ct}");
    }
}

fn commands_since(net: &mut Net, n: usize, frames: &[(u16, u8, &[u8])]) -> Vec<bulb_core::command::Command> {
    let before = net.nodes[n].commands.len();
    for &(cluster, cmd, payload) in frames {
        net.coord.zcl(net.nodes[n].short(), cluster, 0x11, cmd, payload);
    }
    net.run(200);
    net.nodes[n].commands[before..].to_vec()
}

#[test]
fn commands_in_one_poll_see_the_light_the_ones_before_left() {
    use bulb_core::command::Command::{self, MoveToLevel, Off, On, SetLevelWhileOff};
    let (mut net, n, _) = bulb_with(IDENTITY);
    assert_eq!(commands_since(&mut net, n, &[(ON_OFF, 0x02, &[]), (ON_OFF, 0x02, &[]), (ON_OFF, 0x02, &[])]), vec![Off, On, Off], "three toggles");
    assert!(!net.nodes[n].on);
    let sent = commands_since(&mut net, n, &[(LEVEL, 0x04, &[50, 0, 0]), (LEVEL, 0x06, &[0, 10, 0, 0]), (LEVEL, 0x04, &[0, 0, 0]), (ON_OFF, 0x02, &[])]);
    assert_eq!(sent, vec![MoveToLevel { level: 50, transition_ms: 0 }, MoveToLevel { level: 60, transition_ms: 0 }, MoveToLevel { level: 0, transition_ms: 0 }, On]);
    commands_since(&mut net, n, &[(ON_OFF, 0x00, &[])]);
    net.coord.zcl(net.nodes[n].short(), LEVEL, 0x10, 0x02, &[0x0F, 0x00, 0x18, 0x01]);
    net.run(50);
    let sent: Vec<Command> = commands_since(&mut net, n, &[(LEVEL, 0x02, &[0, 20, 0, 0]), (LEVEL, 0x02, &[0, 20, 0, 0])]);
    assert_eq!(sent, vec![SetLevelWhileOff(80), SetLevelWhileOff(100)]);
}

#[test]
fn a_step_during_a_fade_counts_from_where_the_light_is() {
    use bulb_core::command::Command::{MoveToLevel, StopTransition};
    let (mut net, n, _) = bulb_with(IDENTITY);
    commands_since(&mut net, n, &[(LEVEL, 0x04, &[254, 0, 0])]);
    commands_since(&mut net, n, &[(LEVEL, 0x00, &[1, 100, 0])]);
    net.run(4_800);
    assert_eq!(commands_since(&mut net, n, &[(LEVEL, 0x02, &[0, 20, 0, 0])]), vec![MoveToLevel { level: 148, transition_ms: 0 }], "half way from 254 to 1");
    commands_since(&mut net, n, &[(LEVEL, 0x00, &[1, 100, 0])]);
    net.run(4_800);
    let sent = commands_since(&mut net, n, &[(LEVEL, 0x03, &[]), (LEVEL, 0x02, &[0, 20, 0, 0])]);
    assert_eq!(sent, vec![StopTransition, MoveToLevel { level: 95, transition_ms: 0 }], "stopped half way from 148 to 1");
    commands_since(&mut net, n, &[(ON_OFF, 0x00, &[])]);
    commands_since(&mut net, n, &[(LEVEL, 0x04, &[201, 100, 0])]);
    net.run(4_800);
    assert_eq!(commands_since(&mut net, n, &[(LEVEL, 0x02, &[0, 20, 0, 0])]), vec![MoveToLevel { level: 120, transition_ms: 0 }], "half way up from off");
}
#[test]
fn a_step_25_days_after_the_last_fade_does_not_divide_by_zero() {
    use bulb_core::command::Command::MoveToLevel;
    let (mut net, n, short) = bulb_with(IDENTITY);
    level_to(&mut net, short, 100);
    net.air.borrow_mut().now += 0x8000_0000 + 1_000;
    net.run(2_000);
    assert_eq!(last_command(&mut net, n, short, LEVEL, 0x02, &[0x00, 20, 0, 0]), Some(MoveToLevel { level: 120, transition_ms: 0 }));
}

#[test]
fn a_step_25_days_after_an_off_counts_from_the_level() {
    use bulb_core::command::Command::MoveToLevel;
    let (mut net, n, short) = bulb_with(IDENTITY);
    level_to(&mut net, short, 100);
    net.coord.zcl(short, LEVEL, 0x11, 0x00, &[200, 100, 0]);
    net.run(10_050);
    net.coord.zcl(short, ON_OFF, 0x11, 0x00, &[]);
    net.run(50);
    net.coord.zcl(short, ON_OFF, 0x11, 0x01, &[]);
    net.run(50);
    net.air.borrow_mut().now += 0x8000_0000 + 1_000;
    net.run(2_000);
    assert_eq!(last_command(&mut net, n, short, LEVEL, 0x02, &[0x00, 20, 0, 0]), Some(MoveToLevel { level: 220, transition_ms: 0 }));
}

#[test]
fn a_long_read_reporting_configuration_answers_whole_records_only() {
    let (mut net, _, short) = bulb_with(IDENTITY);
    let mut request = Vec::new();
    for _ in 0..30 {
        request.extend_from_slice(&[0x00, 0x00, 0x00]);
    }
    let seq = net.coord.zcl_fragmented(short, ON_OFF, 0x00, 0x08, &request, 50);
    net.run(500);
    let rsp = net.coord.last_zcl(short, ON_OFF, 0x09).expect("an answer");
    assert_eq!(rsp[..3], [0x18, seq, 0x09]);
    let records = &rsp[3..];
    assert_eq!(records.len() % 9, 0, "every record whole: {} bytes, tail {:02x?}", records.len(), &records[records.len() - 4..]);
}

#[test]
fn a_read_reporting_configuration_fills_the_frame_and_stops_at_the_first_record_that_does_not_fit() {
    let on_off = [0x00, 0x00, 0x00];
    let unknown = [0x00, 0xFF, 0xFF];
    let cases: [(&[[u8; 3]], usize); 2] = [(&[unknown, on_off], 27 * 9 + 4), (&[on_off, unknown], 27 * 9)];
    for (tail, want) in cases {
        let (mut net, _, short) = bulb_with(IDENTITY);
        let mut request = Vec::new();
        for _ in 0..27 {
            request.extend_from_slice(&on_off);
        }
        tail.iter().for_each(|r| request.extend_from_slice(r));
        let seq = net.coord.zcl_fragmented(short, ON_OFF, 0x00, 0x08, &request, 50);
        net.run(500);
        let rsp = net.coord.last_zcl(short, ON_OFF, 0x09).expect("an answer");
        assert_eq!(rsp[..3], [0x18, seq, 0x09]);
        assert_eq!(rsp[3..].len(), want, "tail {tail:02x?}");
    }
}

fn dimming_to_off_with_on_off(net: &mut Net, n: usize, short: u16) {
    net.coord.zcl(short, LEVEL, 0x11, 0x04, &[254, 0, 0, 0, 0]);
    net.run(50);
    net.coord.zcl(short, LEVEL, 0x11, 0x05, &[1, 40, 0, 0]);
    net.run(1_000);
    assert!(matches!(*net.nodes[n].commands.last().unwrap(), Command::MoveToLevel { level: 0, .. }));
}

#[test]
fn a_stop_ends_a_move_with_on_off_down_as_zigbee2mqtt_sends_it() {
    let (mut net, n, short) = bulb_with(IDENTITY);
    dimming_to_off_with_on_off(&mut net, n, short);
    net.coord.zcl(short, LEVEL, 0x11, 0x03, &[0, 0]);
    net.run(50);
    assert_eq!(
        *net.nodes[n].commands.last().unwrap(),
        Command::StopTransition,
        "ZCL 8 3.10.2.3.4: Stop ends any Move in process, the dependency on On/Off comes from the command being stopped"
    );
}

#[test]
fn a_step_up_during_a_move_with_on_off_down_is_taken() {
    let (mut net, n, short) = bulb_with(IDENTITY);
    dimming_to_off_with_on_off(&mut net, n, short);
    let before = net.nodes[n].commands.len();
    net.coord.zcl(short, LEVEL, 0x11, 0x02, &[0, 20, 0, 0, 0, 0]);
    net.run(50);
    assert_eq!(net.nodes[n].commands.len(), before + 1, "ZCL 8 3.10.2.3.6: OnOff goes Off only once the level reaches the minimum");
}

#[test]
fn in_one_poll_a_dim_to_off_leaves_the_light_on_and_an_off_with_effect_leaves_it_off() {
    let (mut net, n, _) = bulb_with(IDENTITY);
    let dim: &[u8] = &[0, 10, 0];
    assert_eq!(
        commands_since(&mut net, n, &[(LEVEL, 0x04, dim), (ON_OFF, 0x02, &[])]),
        vec![Command::MoveToLevel { level: 0, transition_ms: 1_000 }, Command::Off],
        "ZCL 8 3.10.2.3.6: on until the minimum level, so the toggle turns it off"
    );
    assert_eq!(
        commands_since(&mut net, n, &[(LEVEL, 0x04, dim), (ON_OFF, 0x02, &[])]),
        vec![Command::MoveToLevel { level: 0, transition_ms: 1_000 }, Command::On],
        "a light that is off stays off"
    );
    assert_eq!(
        commands_since(&mut net, n, &[(ON_OFF, 0x40, &[0, 0]), (ON_OFF, 0x02, &[])]),
        vec![Command::FadeOff { transition_ms: 800 }, Command::On],
        "ZCL 8 3.8.2.3.4.3: Off With Effect is off at once"
    );
}
