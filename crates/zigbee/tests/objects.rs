use crate::sim::*;
use zigbee::hal::NvItem;

const COLOR: u16 = 0x0300;
const ANALOG_INPUT: u16 = 0x000C;

fn bind(net: &mut Net, n: usize, ep: u8, cluster: u16) {
    let short = net.nodes[n].short();
    let ext = net.air.borrow().ports[net.nodes[n].id].ext;
    let mut p = ext.to_le_bytes().to_vec();
    p.push(ep);
    p.extend_from_slice(&cluster.to_le_bytes());
    p.push(3);
    p.extend_from_slice(&COORD_EXT.to_le_bytes());
    p.push(1);
    net.coord.zdo(short, 0x0021, &p);
    net.run(100);
    assert_eq!(net.coord.last_zdo(short, 0x8021).unwrap()[1], 0x00);
}

fn present_value(net: &mut Net, short: u16, ep: u8) -> f32 {
    let seq = net.coord.zcl_ep(short, ep, ANALOG_INPUT, 0x00, 0x00, &[0x55, 0x00]);
    net.run(60);
    let rsp = net.coord.last_zcl_ep(short, ep, ANALOG_INPUT, 0x01).unwrap();
    assert_eq!(rsp[..8], [0x18, seq, 0x01, 0x55, 0x00, 0x00, 0x39, rsp[7]]);
    f32::from_le_bytes([rsp[7], rsp[8], rsp[9], rsp[10]])
}

#[test]
fn the_end_device_children_object_counts_them() {
    let mut net = Net::new();
    let a = net.join_node();
    let sa = net.nodes[a].short();
    assert_eq!(present_value(&mut net, sa, 6), 0.0);
    net.coord.permit(120);
    net.run(600);
    let mut zed = Zed::new(&net.air, true);
    net.link(0, zed.id, 0);
    assert!(zed.join(&mut net, sa));
    net.run(200);
    assert_eq!(present_value(&mut net, sa, 6), 1.0);
}

#[test]
fn saturation_y_and_the_colour_mode_are_reported_once_configured() {
    let mut net = Net::new();
    net.identity = IDENTITY_COLOR;
    let n = net.join_node();
    let short = net.nodes[n].short();
    bind(&mut net, n, 1, COLOR);
    let seq = net.coord.zcl(
        short,
        COLOR,
        0x00,
        0x06,
        &[0x00, 0x01, 0x00, 0x20, 1, 0, 0x10, 0x0E, 1, 0x00, 0x04, 0x00, 0x21, 1, 0, 0x10, 0x0E, 0x10, 0x00, 0x00, 0x08, 0x00, 0x30, 1, 0, 0x10, 0x0E],
    );
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, COLOR, 0x07).unwrap(), vec![0x18, seq, 0x07, 0x00]);
    let report = |net: &Net, attr: u8| net.coord.zcl_rx.iter().rev().find(|e| e.1 == COLOR && e.2[2] == 0x0A && e.2[3] == attr).map(|e| e.2[3..].to_vec());
    net.coord.zcl(short, COLOR, 0x11, 0x07, &[0x00, 0x40, 0x00, 0x50, 0, 0]);
    net.run(3_000);
    assert_eq!(report(&net, 0x04), Some(vec![0x04, 0x00, 0x21, 0x00, 0x50]));
    assert_eq!(report(&net, 0x08), Some(vec![0x08, 0x00, 0x30, 0x01]), "now in xy");
    net.coord.zcl(short, COLOR, 0x11, 0x06, &[80, 90, 0, 0]);
    net.run(3_000);
    assert_eq!(report(&net, 0x01), Some(vec![0x01, 0x00, 0x20, 90]));
    assert_eq!(report(&net, 0x08), Some(vec![0x08, 0x00, 0x30, 0x00]), "now in hue and saturation");
}

#[test]
fn the_status_flags_of_an_object_are_reported_periodically() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    bind(&mut net, n, 2, ANALOG_INPUT);
    let seq = net.coord.zcl_ep(short, 2, ANALOG_INPUT, 0x00, 0x06, &[0x00, 0x6F, 0x00, 0x18, 1, 0, 2, 0]);
    net.run(50);
    assert_eq!(net.coord.last_zcl_ep(short, 2, ANALOG_INPUT, 0x07).unwrap(), vec![0x18, seq, 0x07, 0x00]);
    net.run(5_000);
    let rep = net.coord.last_zcl_ep(short, 2, ANALOG_INPUT, 0x0A).expect("a periodic report");
    assert_eq!(rep[3..], [0x6F, 0x00, 0x18, 0x00]);
}

#[test]
fn reporting_configurations_stored_in_another_version_are_not_restored() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    bind(&mut net, n, 2, ANALOG_INPUT);
    net.coord.zcl_ep(short, 2, ANALOG_INPUT, 0x00, 0x06, &[0x00, 0x6F, 0x00, 0x18, 1, 0, 2, 0]);
    net.run(2_000);
    let id = net.nodes[n].id;
    let mut stored = net.air.borrow().ports[id].nv[&(NvItem::Reports as u8)].clone();
    assert_eq!(stored[0], 1);
    stored[0] = 2;
    net.air.borrow_mut().ports[id].nv.insert(NvItem::Reports as u8, stored);
    net.power_cycle(n);
    net.run(500);
    net.coord.zcl_rx.clear();
    net.run(5_000);
    assert!(net.coord.last_zcl_ep(short, 2, ANALOG_INPUT, 0x0A).is_none());
}

#[test]
fn a_stored_configuration_for_what_is_not_reportable_reports_nothing() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    bind(&mut net, n, 2, ANALOG_INPUT);
    let mut record = vec![1];
    for (ep, attr) in [(2u8, 0x0051u16), (9, 0x0055)] {
        record.extend_from_slice(&[ep, 0]);
        record.extend_from_slice(&ANALOG_INPUT.to_le_bytes());
        record.extend_from_slice(&attr.to_le_bytes());
        record.extend_from_slice(&[1, 0, 2, 0, 0, 0, 0, 0]);
    }
    let id = net.nodes[n].id;
    net.air.borrow_mut().ports[id].nv.insert(NvItem::Reports as u8, record);
    net.power_cycle(n);
    net.run(500);
    net.coord.zcl_rx.clear();
    net.run(5_000);
    assert!(net.coord.zcl_rx.iter().all(|(s, _, f)| *s != short || f[2] != 0x0A), "no reports");
}

fn out_of_service(net: &mut Net, short: u16, ep: u8, cluster: u16) -> Vec<u8> {
    let seq = net.coord.zcl_ep(short, ep, cluster, 0x00, 0x00, &[0x51, 0x00]);
    net.run(60);
    let rsp = net.coord.last_zcl_ep(short, ep, cluster, 0x01).unwrap();
    assert_eq!(rsp[1], seq);
    rsp[3..].to_vec()
}

#[test]
fn the_ble_window_object_is_out_of_service_while_ble_is_off() {
    let mut net = Net::new();
    net.identity = IDENTITY_BLE;
    let n = net.join_node();
    let short = net.nodes[n].short();
    assert_eq!(out_of_service(&mut net, short, 8, 0x000D), vec![0x51, 0x00, 0x00, 0x10, 0]);
    net.coord.zcl_mfr(short, 0xFC5F, 0x00, 0x1141, 0x02, &[0x00, 0x00, 0x30, 2]);
    net.run(100);
    assert_eq!(out_of_service(&mut net, short, 8, 0x000D), vec![0x51, 0x00, 0x00, 0x10, 1]);
    assert_eq!(out_of_service(&mut net, short, 7, 0x0010), vec![0x51, 0x00, 0x00, 0x10, 0], "only the window");
}

#[test]
fn reports_go_only_to_bindings_of_their_cluster_that_are_in_use() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    bind(&mut net, n, 1, 0x0006);
    let ext = net.air.borrow().ports[net.nodes[n].id].ext;
    let mut unbind = ext.to_le_bytes().to_vec();
    unbind.push(1);
    unbind.extend_from_slice(&0x0008u16.to_le_bytes());
    unbind.push(3);
    unbind.extend_from_slice(&COORD_EXT.to_le_bytes());
    unbind.push(1);
    net.coord.zdo(short, 0x0021, &unbind);
    net.run(100);
    net.coord.zdo(short, 0x0022, &unbind);
    net.run(100);
    net.coord.zcl_rx.clear();
    net.coord.zcl_rx_ep.clear();
    net.coord.zcl(short, 0x0008, 0x11, 0x00, &[77, 0, 0]);
    net.run(3_000);
    assert!(net.coord.zcl_rx.iter().all(|(s, c, f)| *s != short || *c != 0x0008 || f[2] != 0x0A), "no level report");
}

#[test]
fn hue_and_x_are_reported_once_configured_and_no_sooner_than_the_minimum_interval() {
    let mut net = Net::new();
    net.identity = IDENTITY_COLOR;
    let n = net.join_node();
    let short = net.nodes[n].short();
    bind(&mut net, n, 1, COLOR);
    let seq = net.coord.zcl(short, COLOR, 0x00, 0x06, &[0x00, 0x00, 0x00, 0x20, 5, 0, 0x10, 0x0E, 1, 0x00, 0x03, 0x00, 0x21, 1, 0, 0x10, 0x0E, 0x10, 0x00]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, COLOR, 0x07).unwrap(), vec![0x18, seq, 0x07, 0x00]);
    let reports = |net: &Net, attr: u8| -> Vec<Vec<u8>> { net.coord.zcl_rx.iter().filter(|e| e.0 == short && e.1 == COLOR && e.2[2] == 0x0A && e.2[3] == attr).map(|e| e.2[3..].to_vec()).collect() };
    net.coord.zcl(short, COLOR, 0x11, 0x07, &[0x00, 0x40, 0x00, 0x50, 0, 0]);
    net.run(3_000);
    assert_eq!(reports(&net, 0x03).last(), Some(&vec![0x03, 0x00, 0x21, 0x00, 0x40]));
    net.run(6_000);
    net.coord.zcl(short, COLOR, 0x11, 0x06, &[80, 90, 0, 0]);
    net.run(1_000);
    let hue = reports(&net, 0x00).len();
    assert_eq!(reports(&net, 0x00).last(), Some(&vec![0x00, 0x00, 0x20, 80]));
    net.coord.zcl(short, COLOR, 0x11, 0x06, &[100, 90, 0, 0]);
    net.run(3_000);
    assert_eq!(reports(&net, 0x00).len(), hue, "not within the minimum interval");
    net.run(3_000);
    assert_eq!(reports(&net, 0x00).last(), Some(&vec![0x00, 0x00, 0x20, 100]));
}

#[test]
fn a_present_value_is_reported_only_for_a_change_of_at_least_the_reportable_change() {
    for (step, reported) in [(5.0f32, false), (0.5, true)] {
        let mut net = Net::new();
        let a = net.join_node();
        let sa = net.nodes[a].short();
        bind(&mut net, a, 6, ANALOG_INPUT);
        let mut cfg = vec![0x00, 0x55, 0x00, 0x39, 1, 0, 0x10, 0x0E];
        cfg.extend_from_slice(&step.to_le_bytes());
        net.coord.zcl_ep(sa, 6, ANALOG_INPUT, 0x00, 0x06, &cfg);
        net.run(100);
        net.coord.permit(120);
        net.run(600);
        let n0 = net.coord.zcl_rx.len();
        let mut zed = Zed::new(&net.air, true);
        net.link(0, zed.id, 0);
        assert!(zed.join(&mut net, sa));
        net.run(3_000);
        let got = net.coord.zcl_rx[n0..].iter().any(|e| e.0 == sa && e.1 == ANALOG_INPUT && e.2[2] == 0x0A);
        assert_eq!(got, reported, "a change of 1 against {step}");
    }
}

#[test]
fn end_devices_are_allowed_until_written_otherwise() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    assert!(net.nodes[n].stack.allows_children());
    net.coord.zcl_ep(short, 7, 0x0010, 0x00, 0x02, &[0x55, 0x00, 0x10, 0]);
    net.run(100);
    assert!(!net.nodes[n].stack.allows_children());
}
