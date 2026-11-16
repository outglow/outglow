mod sim;

use sim::*;
use zigbee::stack::BleMode;

const SETTINGS: u16 = 0xFC5F;
const CODE: u16 = 0x1141;
const FC_RSP_MFR: u8 = 0x1C;

fn ble_bulb() -> (Net, usize, u16) {
    let mut net = Net::new();
    net.identity = IDENTITY_BLE;
    let n = net.join_node();
    let short = net.nodes[n].short();
    (net, n, short)
}

fn read(net: &mut Net, short: u16, id: u16) -> Vec<u8> {
    let i = id.to_le_bytes();
    let seq = net.coord.zcl_mfr(short, SETTINGS, 0x00, CODE, 0x00, &i);
    net.run(50);
    let rsp = net.coord.last_zcl_mfr(short, SETTINGS, 0x01).expect("a read attributes response");
    let c = CODE.to_le_bytes();
    assert_eq!(&rsp[..7], &[FC_RSP_MFR, c[0], c[1], seq, 0x01, i[0], i[1]], "manufacturer-specific, like the request");
    rsp[7..].to_vec()
}

fn write(net: &mut Net, short: u16, id: u16, ty: u8, value: &[u8]) -> Vec<u8> {
    let mut p = id.to_le_bytes().to_vec();
    p.push(ty);
    p.extend_from_slice(value);
    let seq = net.coord.zcl_mfr(short, SETTINGS, 0x00, CODE, 0x02, &p);
    net.run(50);
    let rsp = net.coord.last_zcl_mfr(short, SETTINGS, 0x04).expect("a write attributes response");
    assert_eq!(rsp[3], seq);
    rsp[5..].to_vec()
}

#[test]
fn the_cluster_is_on_the_endpoint_of_firmware_with_ble_only() {
    let (mut net, _, short) = ble_bulb();
    let s = short.to_le_bytes();
    net.coord.zdo(short, 0x0004, &[s[0], s[1], 1]);
    net.run(50);
    let desc = net.coord.last_zdo(short, 0x8004).unwrap();
    assert_eq!(desc[11], 9);
    assert_eq!(&desc[12 + 16..12 + 18], &[0x5F, 0xFC]);
    assert_eq!(&desc[12 + 18..], &[1, 0x19, 0x00]);

    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let seq = net.coord.zcl_mfr(short, SETTINGS, 0x00, CODE, 0x00, &[0, 0]);
    net.run(50);
    let rsp = net.coord.zcl_rx.iter().rev().find(|(s, c, _)| *s == short && *c == SETTINGS).unwrap().2.clone();
    assert_eq!(rsp, vec![0x1C, 0x41, 0x11, seq, 0x0B, 0x00, 0x81]);
    let seq = net.coord.zcl(short, SETTINGS, 0x00, 0x00, &[0, 0]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, SETTINGS, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x00, 0xC3], "UNSUPPORTED_CLUSTER");
}

#[test]
fn out_of_the_box_ble_runs_always_with_a_window_of_twenty_minutes() {
    let (mut net, n, short) = ble_bulb();
    assert_eq!(net.nodes[n].stack.ble_mode(), BleMode::On);
    assert_eq!(net.nodes[n].stack.ble_window_minutes(), 20);
    assert_eq!(read(&mut net, short, 0x0000), vec![0x00, 0x30, 0]);
    assert_eq!(read(&mut net, short, 0x0001), vec![0x00, 0x21, 20, 0]);
    assert_eq!(read(&mut net, short, 0x0002), vec![0x00, 0x10, 0]);
    assert_eq!(read(&mut net, short, 0xFFFD), vec![0x00, 0x21, 1, 0], "ClusterRevision");
    net.nodes[n].stack.set_ble_active(true);
    assert_eq!(read(&mut net, short, 0x0002), vec![0x00, 0x10, 1]);
}

#[test]
fn the_mode_and_the_window_are_written_and_survive_a_power_cycle() {
    let (mut net, n, short) = ble_bulb();
    assert_eq!(write(&mut net, short, 0x0000, 0x30, &[2]), vec![0x00]);
    assert_eq!(net.nodes[n].stack.ble_mode(), BleMode::Off);
    assert_eq!(write(&mut net, short, 0x0001, 0x21, &[90, 0]), vec![0x00]);
    assert_eq!(write(&mut net, short, 0x0000, 0x30, &[1]), vec![0x00]);
    net.power_cycle(n);
    assert_eq!(net.nodes[n].stack.ble_mode(), BleMode::Window);
    assert_eq!(net.nodes[n].stack.ble_window_minutes(), 90);
    net.run(1000);
    assert_eq!(read(&mut net, short, 0x0000), vec![0x00, 0x30, 1]);
    assert_eq!(read(&mut net, short, 0x0001), vec![0x00, 0x21, 90, 0]);
}

#[test]
fn values_out_of_range_the_wrong_type_and_the_state_are_refused() {
    let (mut net, n, short) = ble_bulb();
    assert_eq!(write(&mut net, short, 0x0000, 0x30, &[3]), vec![0x87, 0x00, 0x00]);
    assert_eq!(write(&mut net, short, 0x0001, 0x21, &[0, 0]), vec![0x87, 0x01, 0x00]);
    assert_eq!(write(&mut net, short, 0x0001, 0x21, &[0xA1, 0x05]), vec![0x87, 0x01, 0x00]);
    assert_eq!(write(&mut net, short, 0x0001, 0x21, &[0xA0, 0x05]), vec![0x00], "1440 minutes is the longest");
    assert_eq!(write(&mut net, short, 0x0000, 0x20, &[2]), vec![0x8D, 0x00, 0x00]);
    assert_eq!(write(&mut net, short, 0x0002, 0x10, &[1]), vec![0x88, 0x02, 0x00]);
    assert_eq!(net.nodes[n].stack.ble_mode(), BleMode::On);
}

#[test]
fn frames_without_the_manufacturer_code_are_taken_and_another_code_is_not() {
    let (mut net, n, short) = ble_bulb();
    let seq = net.coord.zcl(short, SETTINGS, 0x00, 0x02, &[0x00, 0x00, 0x30, 2]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, SETTINGS, 0x04).unwrap(), vec![0x18, seq, 0x04, 0x00]);
    assert_eq!(net.nodes[n].stack.ble_mode(), BleMode::Off);
    let seq = net.coord.zcl_mfr(short, SETTINGS, 0x00, 0x1234, 0x02, &[0x00, 0x00, 0x30, 0]);
    net.run(50);
    let rsp = net.coord.last_zcl_mfr(short, SETTINGS, 0x0B).unwrap();
    assert_eq!(rsp, vec![0x1C, 0x34, 0x12, seq, 0x0B, 0x02, 0x81]);
    assert_eq!(net.nodes[n].stack.ble_mode(), BleMode::Off);
    let seq = net.coord.zcl_mfr(short, 0x0006, 0x00, CODE, 0x00, &[0, 0]);
    net.run(50);
    let rsp = net.coord.last_zcl_mfr(short, 0x0006, 0x0B).unwrap();
    assert_eq!(rsp, vec![0x1C, 0x41, 0x11, seq, 0x0B, 0x00, 0x81]);
}

#[test]
fn the_attributes_can_be_discovered_with_their_access() {
    let (mut net, _, short) = ble_bulb();
    let seq = net.coord.zcl_mfr(short, SETTINGS, 0x00, CODE, 0x15, &[0, 0, 10]);
    net.run(50);
    let rsp = net.coord.last_zcl_mfr(short, SETTINGS, 0x16).unwrap();
    assert_eq!(
        rsp,
        vec![0x1C, 0x41, 0x11, seq, 0x16, 1, 0x00, 0x00, 0x30, 0x03, 0x01, 0x00, 0x21, 0x03, 0x02, 0x00, 0x10, 0x01, 0xFD, 0xFF, 0x21, 0x01]
    );
}

#[test]
fn leaving_the_network_brings_ble_back() {
    let (mut net, n, short) = ble_bulb();
    write(&mut net, short, 0x0000, 0x30, &[2]);
    net.coord.permit_join = false;
    net.nodes[n].stack.leave();
    net.run(100);
    assert!(!net.nodes[n].stack.is_joined());
    assert_eq!(net.nodes[n].stack.ble_mode(), BleMode::On, "out of the network BLE is the way to the bulb");
    net.power_cycle(n);
    assert_eq!(net.nodes[n].stack.ble_mode(), BleMode::On, "and nothing of the setting is stored");
}

#[test]
fn reset_to_factory_defaults_brings_ble_back() {
    let (mut net, n, short) = ble_bulb();
    write(&mut net, short, 0x0001, 0x21, &[5, 0]);
    write(&mut net, short, 0x0000, 0x30, &[2]);
    net.coord.zcl(short, 0x0000, 0x01, 0x00, &[]);
    net.run(50);
    assert_eq!(net.nodes[n].stack.ble_mode(), BleMode::On);
    assert_eq!(net.nodes[n].stack.ble_window_minutes(), 20);
    net.power_cycle(n);
    assert_eq!(net.nodes[n].stack.ble_mode(), BleMode::On);
    assert!(net.nodes[n].stack.is_joined(), "the attributes, not the network");
}

#[test]
fn a_damaged_settings_record_leaves_ble_on() {
    let (mut net, n, short) = ble_bulb();
    write(&mut net, short, 0x0000, 0x30, &[2]);
    let id = net.nodes[n].id;
    for bad in [vec![1, 7, 20, 0], vec![1, 2, 0, 0], vec![9, 2, 20, 0], vec![1, 2]] {
        net.air.borrow_mut().ports[id].nv.insert(zigbee::hal::NvItem::Settings as u8, bad.clone());
        net.power_cycle(n);
        assert_eq!(net.nodes[n].stack.ble_mode(), BleMode::On, "record {bad:?}");
    }
}
