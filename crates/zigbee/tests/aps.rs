use crate::sim::*;
use zigbee::aes::Aes128;
use zigbee::crypto;
use zigbee::stack::State;

const ON_OFF: u16 = 0x0006;
const LEVEL: u16 = 0x0008;
const DIAG: u16 = 0x0B05;
const ANALOG_INPUT: u16 = 0x000C;
const APS_UNAUTHORIZED_KEY: u16 = 0x0114;
const APS_DECRYPT_FAILURES: u16 = 0x0116;

fn bulb() -> (Net, usize, u16) {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    (net, n, short)
}

fn ext_of(net: &Net, n: usize) -> u64 {
    net.air.borrow().ports[net.nodes[n].id].ext
}

fn counter(net: &mut Net, short: u16, id: u16) -> u16 {
    let seq = net.coord.zcl(short, DIAG, 0x00, 0x00, &id.to_le_bytes());
    net.run(60);
    let rsp = net.coord.last_zcl(short, DIAG, 0x01).unwrap();
    assert_eq!(rsp[..8], [0x18, seq, 0x01, id as u8, (id >> 8) as u8, 0x00, 0x21, rsp[7]]);
    u16::from_le_bytes([rsp[7], rsp[8]])
}

fn read_on_off(net: &mut Net, short: u16, aps_fc: u8, dst_ep: u8) -> u8 {
    let seq = 0x40 ^ aps_fc ^ dst_ep;
    let aps = net.coord.aps_frame(aps_fc, dst_ep, ON_OFF, 0x0104, 1, &[0x00, seq, 0x00, 0x00, 0x00]);
    net.coord.zcl_rx.clear();
    net.coord.nwk_data(short, &aps);
    net.run(100);
    seq
}

fn secured_zcl(net: &mut Net, short: u16, sc: u8, src: u64, zcl: &[u8]) -> Vec<u8> {
    let link = net.coord.device(short).unwrap().tclk;
    let counter = net.coord.aps_fc;
    net.coord.aps_fc += 1;
    let mut f = vec![0x20, 1, 0x06, 0x00, 0x04, 0x01, 1, 0x99];
    let aux_at = f.len();
    f.push(sc | 5);
    f.extend_from_slice(&counter.to_le_bytes());
    if sc & 0x20 != 0 {
        f.extend_from_slice(&src.to_le_bytes());
    }
    let body_at = f.len();
    f.extend_from_slice(zcl);
    f.extend_from_slice(&[0; 4]);
    let (head, body) = f.split_at_mut(body_at);
    crypto::ccm_encrypt(&Aes128::new(&link), &nonce(src, counter, sc | 5), head, body, zcl.len()).unwrap();
    f[aux_at] = sc;
    f
}

#[test]
fn aps_data_with_the_reserved_delivery_mode_is_dropped() {
    let (mut net, _, short) = bulb();
    read_on_off(&mut net, short, 0x04, 1);
    assert!(net.coord.last_zcl(short, ON_OFF, 0x01).is_none());
    read_on_off(&mut net, short, 0x00, 1);
    assert!(net.coord.last_zcl(short, ON_OFF, 0x01).is_some());
}

#[test]
fn aps_data_to_an_endpoint_that_does_not_exist_is_dropped() {
    let (mut net, _, short) = bulb();
    for ep in [9u8, 0x20, 0xF1] {
        read_on_off(&mut net, short, 0x00, ep);
        assert!(net.coord.zcl_rx.is_empty(), "endpoint {ep}");
    }
    let aps = net.coord.aps_frame(0x00, 1, ON_OFF, 0xC05E, 1, &[0x00, 0x42, 0x00, 0x00, 0x00]);
    net.coord.nwk_data(short, &aps);
    net.run(100);
    assert!(net.coord.zcl_rx.is_empty(), "nor for another profile");
}

#[test]
fn the_broadcast_endpoint_reaches_every_endpoint_with_the_cluster_and_the_light_otherwise() {
    let (mut net, _, short) = bulb();
    net.coord.zcl_rx_ep.clear();
    read_on_off(&mut net, short, 0x00, 0xFF);
    assert_eq!(net.coord.zcl_rx_ep, vec![1]);
    net.coord.zcl_rx.clear();
    net.coord.zcl_rx_ep.clear();
    let aps = net.coord.aps_frame(0x00, 0xFF, ANALOG_INPUT, 0x0104, 1, &[0x00, 0x43, 0x00, 0x55, 0x00]);
    net.coord.nwk_data(short, &aps);
    net.run(300);
    let mut eps = net.coord.zcl_rx_ep.clone();
    eps.sort();
    assert_eq!(eps, vec![2, 3, 4, 5, 6]);
    assert!(net.coord.zcl_rx.iter().all(|(_, c, f)| *c == ANALOG_INPUT && f[..3] == [0x18, 0x43, 0x01]));
    net.coord.zcl_rx.clear();
    net.coord.zcl_rx_ep.clear();
    let aps = net.coord.aps_frame(0x00, 0xFF, 0x0B04, 0x0104, 1, &[0x00, 0x44, 0x00, 0x00, 0x00]);
    net.coord.nwk_data(short, &aps);
    net.run(300);
    assert_eq!(net.coord.zcl_rx_ep, vec![1], "a cluster nobody serves is answered by the light");
    assert_eq!(net.coord.zcl_rx[0].1, 0x0B04);
}

#[test]
fn a_fragment_that_is_broadcast_or_aps_secured_is_dropped() {
    let (mut net, _, short) = bulb();
    for (fc, dst) in [(0x88u8, 0xFFFDu16), (0xA0, short)] {
        let mut aps = net.coord.aps_frame(fc, 1, ON_OFF, 0x0104, 1, &[]);
        aps.extend_from_slice(&[1, 1, 0x00, 0x42, 0x00, 0x00, 0x00]);
        net.coord.zcl_rx.clear();
        net.coord.fragment_acks = 0;
        net.coord.nwk_data(dst, &aps);
        net.run(300);
        assert!(net.coord.zcl_rx.iter().all(|(s, _, _)| *s != short), "{fc:#x}");
    }
}

#[test]
fn aps_secured_data_needs_the_trust_center_its_address_and_the_data_key() {
    let (mut net, _, short) = bulb();
    let read = [0x00, 0x51, 0x00, 0x00, 0x00];
    let unauthorized = counter(&mut net, short, APS_UNAUTHORIZED_KEY);
    let f = secured_zcl(&mut net, short, 0x20, 0x00124B00000000AA, &read);
    net.coord.nwk_data(short, &f);
    net.run(100);
    assert!(net.coord.last_zcl(short, ON_OFF, 0x01).is_none(), "another source address");
    assert_eq!(counter(&mut net, short, APS_UNAUTHORIZED_KEY), unauthorized + 1);
    let f = secured_zcl(&mut net, short, 0x30, COORD_EXT, &read);
    net.coord.nwk_data(short, &f);
    net.run(100);
    assert!(net.coord.last_zcl(short, ON_OFF, 0x01).is_none(), "the key-transport key");
    let failures = counter(&mut net, short, APS_DECRYPT_FAILURES);
    assert_eq!(counter(&mut net, short, APS_UNAUTHORIZED_KEY), unauthorized + 1);
    let f = secured_zcl(&mut net, short, 0x00, COORD_EXT, &read);
    net.coord.nwk_data(short, &f);
    net.run(100);
    assert!(net.coord.last_zcl(short, ON_OFF, 0x01).is_some(), "without the address the Trust Center's is assumed");
    assert_eq!(counter(&mut net, short, APS_DECRYPT_FAILURES), failures);
}

#[test]
fn an_aps_ack_that_is_not_a_fragment_ack_changes_nothing() {
    let (mut net, n, short) = bulb();
    for ack in [vec![0x02, 1, 0x06, 0x00, 0x04, 0x01, 1, 0x10], vec![0x82, 1, 0x06, 0x00, 0x04, 0x01, 1, 0x10, 0x00, 0, 0xFF], vec![0x92, 0x10]] {
        net.coord.nwk_data(short, &ack);
    }
    net.run(300);
    assert_eq!(net.nodes[n].stack.state(), State::Up);
    read_on_off(&mut net, short, 0x00, 1);
    assert!(net.coord.last_zcl(short, ON_OFF, 0x01).is_some());
}

#[test]
fn a_group_binding_reports_to_the_group() {
    let (mut net, n, short) = bulb();
    let me = ext_of(&net, n);
    let mut bind = me.to_le_bytes().to_vec();
    bind.push(1);
    bind.extend_from_slice(&LEVEL.to_le_bytes());
    bind.extend_from_slice(&[1, 0x34, 0x12]);
    net.coord.zdo(short, 0x0021, &bind);
    net.run(100);
    assert_eq!(net.coord.last_zdo(short, 0x8021).unwrap()[1], 0x00);
    let t = net.now();
    net.coord.zcl(short, LEVEL, 0x11, 0x04, &[90, 0, 0, 0, 0]);
    net.run(2_000);
    let groupcasts: Vec<Vec<u8>> = net
        .nwk_sent_by(net.nodes[n].id, t)
        .into_iter()
        .filter(|(_, h, p)| h.dst == 0xFFFD && p[0] == 0x0C)
        .map(|(_, _, p)| p)
        .collect();
    assert!(!groupcasts.is_empty());
    let g = &groupcasts[0];
    assert_eq!(g[..8], [0x0C, 0x34, 0x12, 0x08, 0x00, 0x04, 0x01, 1]);
    assert_eq!(g[9..11], [0x18, g[9 + 1]]);
    assert_eq!(g[11], 0x0A, "Report Attributes");
    assert_eq!(g[12..15], [0x00, 0x00, 0x20]);
}

#[test]
fn a_network_key_for_another_device_is_not_taken() {
    let (mut net, _, short) = bulb();
    let link = net.coord.device(short).unwrap().tclk;
    let mut body = vec![0x05, 0x01];
    body.extend_from_slice(&[0x5A; 16]);
    body.push(1);
    body.extend_from_slice(&0x00124B00000000BBu64.to_le_bytes());
    body.extend_from_slice(&COORD_EXT.to_le_bytes());
    let aps = net.coord.aps_command(&link, 2, &body);
    net.coord.nwk_data(short, &aps);
    net.run(100);
    net.coord.switch_key(1);
    net.run(100);
    net.coord.use_key(&[0x5A; 16], 1, false);
    read_on_off(&mut net, short, 0x00, 1);
    assert!(net.coord.last_zcl(short, ON_OFF, 0x01).is_none(), "the bulb still has the old key");
}

#[test]
fn a_trust_center_link_key_outside_the_key_exchange_and_unknown_key_types_and_commands_are_ignored() {
    let (mut net, n, short) = bulb();
    let link = net.coord.device(short).unwrap().tclk;
    for body in [
        [&[0x05u8, 0x04][..], &[0x6B; 16], &COORD_EXT.to_le_bytes()].concat(),
        [&[0x05u8, 0x03][..], &[0x6C; 16], &COORD_EXT.to_le_bytes()].concat(),
        vec![0x2A, 0x00],
    ] {
        let aps = net.coord.aps_command(&link, 2, &body);
        net.coord.nwk_data(short, &aps);
        net.run(100);
    }
    net.power_cycle(n);
    net.run(3_000);
    assert_eq!(net.nodes[n].stack.state(), State::Up);
    let f = secured_zcl(&mut net, short, 0x00, COORD_EXT, &[0x00, 0x52, 0x00, 0x00, 0x00]);
    net.coord.nwk_data(short, &f);
    net.run(100);
    assert!(net.coord.last_zcl(short, ON_OFF, 0x01).is_some(), "the link key is the one from joining");
}

#[test]
fn an_aps_command_secured_with_the_data_key_of_a_reserved_key_identifier_is_dropped() {
    let (mut net, n, short) = bulb();
    let ext = ext_of(&net, n);
    let link = net.coord.device(short).unwrap().tclk;
    let mut remove = vec![0x07];
    remove.extend_from_slice(&ext.to_le_bytes());
    let aps = net.coord.aps_command(&link, 1, &remove);
    net.coord.nwk_data(short, &aps);
    net.run(2_000);
    assert_eq!(net.nodes[n].stack.state(), State::Up);
    let aps = net.coord.aps_command(&link, 0, &remove);
    net.coord.permit_join = false;
    net.coord.nwk_data(short, &aps);
    net.run(2_000);
    assert_ne!(net.nodes[n].stack.state(), State::Up);
}
