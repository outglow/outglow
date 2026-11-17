use crate::sim::*;
use zigbee::nwk;

const NWK_ADDR_REQ: u16 = 0x0000;
const NODE_DESC_REQ: u16 = 0x0002;
const POWER_DESC_REQ: u16 = 0x0003;
const SIMPLE_DESC_REQ: u16 = 0x0004;
const ACTIVE_EP_REQ: u16 = 0x0005;
const MATCH_DESC_REQ: u16 = 0x0006;
const BIND_REQ: u16 = 0x0021;
const UNBIND_REQ: u16 = 0x0022;
const MGMT_BIND_REQ: u16 = 0x0033;
const MGMT_LEAVE_REQ: u16 = 0x0034;
const MGMT_NWK_UPDATE_REQ: u16 = 0x0038;
const ON_OFF: u16 = 0x0006;
const LEVEL: u16 = 0x0008;

fn bulb() -> (Net, usize, u16) {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    (net, n, short)
}

fn ext_of(net: &Net, n: usize) -> u64 {
    net.air.borrow().ports[net.nodes[n].id].ext
}

fn ask(net: &mut Net, dst: u16, cluster: u16, payload: &[u8]) -> Option<Vec<u8>> {
    let src = if dst >= 0xFFF8 { net.nodes[0].short() } else { dst };
    net.coord.zdo_rx.clear();
    let seq = net.coord.zdo(dst, cluster, payload);
    net.run(300);
    let rsp = net.coord.last_zdo(src, cluster | 0x8000)?;
    assert_eq!(rsp[0], seq, "the response carries the request's sequence number");
    Some(rsp[1..].to_vec())
}

fn bind_payload(src_ext: u64, src_ep: u8, cluster: u16, dst: &[u8]) -> Vec<u8> {
    let mut p = src_ext.to_le_bytes().to_vec();
    p.push(src_ep);
    p.extend_from_slice(&cluster.to_le_bytes());
    p.extend_from_slice(dst);
    p
}

fn unicast_dst(ext: u64, ep: u8) -> Vec<u8> {
    let mut d = vec![3];
    d.extend_from_slice(&ext.to_le_bytes());
    d.push(ep);
    d
}

#[test]
fn a_zdo_frame_without_a_sequence_number_is_dropped() {
    let (mut net, _, short) = bulb();
    for cluster in [NODE_DESC_REQ, 0x0013, 0x001F, 0x8000] {
        let aps = net.coord.aps_frame(0x00, 0, cluster, 0x0000, 0, &[]);
        net.coord.nwk_data(short, &aps);
    }
    net.run(300);
    assert!(net.coord.zdo_rx.iter().all(|(s, c, _)| *s != short || *c & 0x8000 == 0));
}

#[test]
fn descriptor_requests_about_another_device_are_device_not_found_and_only_unicast_ones_are_answered() {
    let (mut net, _, short) = bulb();
    let other = 0x1234u16.to_le_bytes();
    assert_eq!(ask(&mut net, short, NODE_DESC_REQ, &other), Some(vec![0x81, 0x34, 0x12]));
    assert_eq!(ask(&mut net, short, POWER_DESC_REQ, &other), Some(vec![0x81, 0x34, 0x12]));
    assert_eq!(ask(&mut net, short, ACTIVE_EP_REQ, &other), Some(vec![0x81, 0x34, 0x12, 0]));
    assert_eq!(ask(&mut net, short, SIMPLE_DESC_REQ, &[other[0], other[1], 1]), Some(vec![0x81, 0x34, 0x12, 0]));
    for cluster in [NODE_DESC_REQ, POWER_DESC_REQ, ACTIVE_EP_REQ] {
        assert_eq!(ask(&mut net, 0xFFFD, cluster, &other), None);
    }
}

#[test]
fn the_power_descriptor_is_mains_powered_always_on() {
    let (mut net, _, short) = bulb();
    let s = short.to_le_bytes();
    assert_eq!(ask(&mut net, short, POWER_DESC_REQ, &s), Some(vec![0x00, s[0], s[1], 0x10, 0xC1]));
}

#[test]
fn the_node_descriptor_is_a_router_with_the_r22_stack_revision() {
    let (mut net, _, short) = bulb();
    let s = short.to_le_bytes();
    assert_eq!(
        ask(&mut net, short, NODE_DESC_REQ, &s),
        Some(vec![0x00, s[0], s[1], 0x01, 0x40, 0x8E, 0x41, 0x11, 82, 0xFA, 0x00, 0x00, 22 << 1, 0xFA, 0x00, 0x00])
    );
}

#[test]
fn a_broadcast_simple_descriptor_request_for_a_missing_endpoint_is_not_answered_but_one_for_the_light_is() {
    let (mut net, _, short) = bulb();
    let s = short.to_le_bytes();
    assert_eq!(ask(&mut net, 0xFFFD, SIMPLE_DESC_REQ, &[s[0], s[1], 0]), None);
    assert_eq!(ask(&mut net, 0xFFFD, SIMPLE_DESC_REQ, &[s[0], s[1], 1]).unwrap()[..6], [0x00, s[0], s[1], 26, 1, 0x04]);
}

#[test]
fn match_descriptor_lists_the_light_and_the_objects_serving_the_clusters() {
    let (mut net, _, short) = bulb();
    let s = short.to_le_bytes();
    let req = |about: [u8; 2], profile: u16, clusters: &[u16]| {
        let mut p = about.to_vec();
        p.extend_from_slice(&profile.to_le_bytes());
        p.push(clusters.len() as u8);
        for c in clusters {
            p.extend_from_slice(&c.to_le_bytes());
        }
        p.push(0);
        p
    };
    assert_eq!(ask(&mut net, short, MATCH_DESC_REQ, &req(s, 0x0104, &[ON_OFF])), Some(vec![0x00, s[0], s[1], 1, 1]));
    assert_eq!(ask(&mut net, short, MATCH_DESC_REQ, &req(s, 0x0104, &[0x000C])), Some(vec![0x00, s[0], s[1], 5, 2, 3, 4, 5, 6]));
    assert_eq!(
        ask(&mut net, short, MATCH_DESC_REQ, &req(s, 0xFFFF, &[0x0010, LEVEL, 0x000D])),
        Some(vec![0x00, s[0], s[1], 3, 1, 7, 8])
    );
    assert_eq!(ask(&mut net, short, MATCH_DESC_REQ, &req([0xFD, 0xFF], 0x0104, &[0x000D])), Some(vec![0x00, s[0], s[1], 1, 8]));
    assert_eq!(ask(&mut net, short, MATCH_DESC_REQ, &req(s, 0x0109, &[ON_OFF])), Some(vec![0x00, s[0], s[1], 0]));
    assert_eq!(ask(&mut net, short, MATCH_DESC_REQ, &req([0x34, 0x12], 0x0104, &[ON_OFF])), Some(vec![0x00, s[0], s[1], 0]));
    assert_eq!(ask(&mut net, short, MATCH_DESC_REQ, &req(s, 0x0104, &[0x0B04])), Some(vec![0x00, s[0], s[1], 0]));
    assert_eq!(ask(&mut net, 0xFFFD, MATCH_DESC_REQ, &req([0xFD, 0xFF], 0x0104, &[0x0B04])), None);
    assert_eq!(ask(&mut net, 0xFFFD, MATCH_DESC_REQ, &req([0xFD, 0xFF], 0x0104, &[ON_OFF])), Some(vec![0x00, s[0], s[1], 1, 1]));
    assert_eq!(ask(&mut net, short, MATCH_DESC_REQ, &[s[0], s[1], 0x04, 0x01, 2, 0x06, 0x00]), None);
}

#[test]
fn bind_requests_are_checked_against_the_spec_statuses() {
    let (mut net, n, short) = bulb();
    let me = ext_of(&net, n);
    let coord = unicast_dst(COORD_EXT, 1);
    assert_eq!(ask(&mut net, short, BIND_REQ, &bind_payload(me, 1, ON_OFF, &[2, 0x34, 0x12])), Some(vec![0x84]));
    assert_eq!(ask(&mut net, short, BIND_REQ, &bind_payload(me ^ 1, 1, ON_OFF, &coord)), Some(vec![0x84]));
    assert_eq!(ask(&mut net, short, BIND_REQ, &bind_payload(me, 9, ON_OFF, &coord)), Some(vec![0x82]));
    assert_eq!(ask(&mut net, short, BIND_REQ, &bind_payload(me, 7, 0x0010, &coord)), Some(vec![0x00]));
    assert_eq!(ask(&mut net, short, BIND_REQ, &bind_payload(me, 1, ON_OFF, &coord)), Some(vec![0x00]));
    assert_eq!(ask(&mut net, short, BIND_REQ, &bind_payload(me, 1, ON_OFF, &coord)), Some(vec![0x00]), "already bound");
    assert_eq!(ask(&mut net, short, MGMT_BIND_REQ, &[0]).unwrap()[..3], [0x00, 2, 0]);
    assert_eq!(ask(&mut net, 0xFFFD, BIND_REQ, &bind_payload(me, 1, LEVEL, &coord)), None);
    assert_eq!(ask(&mut net, short, MGMT_BIND_REQ, &[0]).unwrap()[..3], [0x00, 2, 0], "a broadcast bind does nothing");
    assert_eq!(ask(&mut net, short, UNBIND_REQ, &bind_payload(me, 1, ON_OFF, &coord)), Some(vec![0x00]));
    assert_eq!(ask(&mut net, short, UNBIND_REQ, &bind_payload(me, 1, ON_OFF, &coord)), Some(vec![0x88]));
    assert_eq!(ask(&mut net, short, MGMT_BIND_REQ, &[0]).unwrap()[..3], [0x00, 1, 0]);
}

#[test]
fn the_binding_table_holds_eight_entries_and_then_is_full() {
    let (mut net, n, short) = bulb();
    let me = ext_of(&net, n);
    for i in 0..8u16 {
        assert_eq!(ask(&mut net, short, BIND_REQ, &bind_payload(me, 1, 0x0100 + i, &unicast_dst(COORD_EXT, 1))), Some(vec![0x00]));
    }
    assert_eq!(ask(&mut net, short, BIND_REQ, &bind_payload(me, 1, 0x0200, &unicast_dst(COORD_EXT, 1))), Some(vec![0x8C]));
    assert_eq!(ask(&mut net, short, BIND_REQ, &bind_payload(me, 1, 0x0107, &unicast_dst(COORD_EXT, 1))), Some(vec![0x00]));
    assert_eq!(ask(&mut net, short, UNBIND_REQ, &bind_payload(me, 1, 0x0103, &unicast_dst(COORD_EXT, 1))), Some(vec![0x00]));
    assert_eq!(ask(&mut net, short, BIND_REQ, &bind_payload(me, 1, 0x0200, &unicast_dst(COORD_EXT, 1))), Some(vec![0x00]));
    let page = ask(&mut net, short, MGMT_BIND_REQ, &[6]).unwrap();
    assert_eq!(page[..3], [0x00, 8, 6]);
    assert_eq!(page[3], 2);
    assert_eq!(page.len(), 4 + 2 * 21);
    assert_eq!(page[4 + 9..4 + 11], 0x0106u16.to_le_bytes());
    assert_eq!(page[4 + 21 + 9..4 + 21 + 11], 0x0107u16.to_le_bytes());
    let first = ask(&mut net, short, MGMT_BIND_REQ, &[0]).unwrap();
    assert_eq!(first[3], 3, "at most three entries per response");
    let middle = ask(&mut net, short, MGMT_BIND_REQ, &[3]).unwrap();
    assert_eq!(middle[4 + 9..4 + 11], 0x0200u16.to_le_bytes(), "the freed slot is reused");
}

#[test]
fn a_group_binding_is_listed_with_the_group_address_mode() {
    let (mut net, n, short) = bulb();
    let me = ext_of(&net, n);
    assert_eq!(ask(&mut net, short, BIND_REQ, &bind_payload(me, 1, ON_OFF, &[1, 0x34, 0x12])), Some(vec![0x00]));
    let mut entry = me.to_le_bytes().to_vec();
    entry.extend_from_slice(&[1, 0x06, 0x00, 1, 0x34, 0x12]);
    assert_eq!(ask(&mut net, short, MGMT_BIND_REQ, &[0]), Some([&[0x00, 1, 0, 1][..], &entry].concat()));
    assert_eq!(ask(&mut net, short, MGMT_BIND_REQ, &[1]), Some(vec![0x00, 1, 1, 0]));
}

#[test]
fn reports_to_a_binding_with_an_unknown_address_ask_for_it_and_follow_the_answer() {
    let (mut net, n, short) = bulb();
    let me = ext_of(&net, n);
    let far = 0x00124B00000000F0u64;
    assert_eq!(ask(&mut net, short, BIND_REQ, &bind_payload(me, 1, LEVEL, &unicast_dst(far, 3))), Some(vec![0x00]));
    let t = net.now();
    net.coord.zcl(short, LEVEL, 0x11, 0x04, &[90, 0, 0, 0, 0]);
    net.run(2_000);
    let asks: Vec<Vec<u8>> = net
        .nwk_sent_by(net.nodes[n].id, t)
        .into_iter()
        .filter(|(_, h, p)| h.dst == 0xFFFD && p.len() > 8 && p[2..4] == [0x00, 0x00] && p[4..6] == [0x00, 0x00])
        .map(|(_, h, p)| (h.seq, p[8..].to_vec()))
        .collect::<std::collections::BTreeMap<u8, Vec<u8>>>()
        .into_values()
        .collect();
    assert_eq!(asks.len(), 1, "one NWK_addr_req, not one per report");
    assert_eq!(asks[0][1..], [&far.to_le_bytes()[..], &[0, 0]].concat()[..]);
    let mut rsp = vec![0x77, 0x00];
    rsp.extend_from_slice(&far.to_le_bytes());
    rsp.extend_from_slice(&0x4321u16.to_le_bytes());
    let aps = net.coord.aps_frame(0x00, 0, NWK_ADDR_REQ | 0x8000, 0x0000, 0, &rsp);
    net.coord.nwk_data(short, &aps);
    net.run(200);
    let t = net.now();
    net.coord.zcl(short, LEVEL, 0x11, 0x04, &[10, 0, 0, 0, 0]);
    net.run(2_000);
    let sent = net.nwk_sent_by(net.nodes[n].id, t);
    assert!(sent.iter().any(|(_, h, p)| h.frame_type == nwk::FRAME_CMD && p[0] == nwk::CMD_ROUTE_REQUEST && p.get(3..5) == Some(&[0x21, 0x43][..])));
    assert!(!sent.iter().any(|(_, h, p)| h.dst == 0xFFFD && p.get(2..4) == Some(&[0x00, 0x00])), "no second NWK_addr_req");
}

#[test]
fn a_failed_nwk_addr_response_teaches_nothing() {
    let (mut net, n, short) = bulb();
    let me = ext_of(&net, n);
    let far = 0x00124B00000000F1u64;
    assert_eq!(ask(&mut net, short, BIND_REQ, &bind_payload(me, 1, LEVEL, &unicast_dst(far, 3))), Some(vec![0x00]));
    let mut rsp = vec![0x77, 0x81];
    rsp.extend_from_slice(&far.to_le_bytes());
    rsp.extend_from_slice(&0x4321u16.to_le_bytes());
    let aps = net.coord.aps_frame(0x00, 0, NWK_ADDR_REQ | 0x8000, 0x0000, 0, &rsp);
    net.coord.nwk_data(short, &aps);
    net.run(200);
    let t = net.now();
    net.coord.zcl(short, LEVEL, 0x11, 0x04, &[10, 0, 0, 0, 0]);
    net.run(2_000);
    assert!(!net.nwk_sent_by(net.nodes[n].id, t).iter().any(|(_, h, p)| h.frame_type == nwk::FRAME_CMD && p.get(3..5) == Some(&[0x21, 0x43][..])));
}

#[test]
fn mgmt_leave_for_a_device_that_is_not_a_child_is_unknown_device() {
    let (mut net, _, short) = bulb();
    let mut p = 0x00124B00000000F2u64.to_le_bytes().to_vec();
    p.push(0);
    assert_eq!(ask(&mut net, short, MGMT_LEAVE_REQ, &p), Some(vec![0xC8]));
    assert_eq!(ask(&mut net, 0xFFFD, MGMT_LEAVE_REQ, &[0; 9]), None);
    assert_eq!(net.nodes[0].stack.state(), zigbee::stack::State::Up, "a broadcast leave request is ignored");
}

#[test]
fn an_energy_scan_is_answered_only_when_unicast_and_other_durations_are_invalid() {
    let (mut net, _, short) = bulb();
    let scan = |duration: u8| {
        let mut p = 0x0000_1800u32.to_le_bytes().to_vec();
        p.push(duration);
        p
    };
    let rsp = ask(&mut net, short, MGMT_NWK_UPDATE_REQ, &scan(5)).unwrap();
    assert_eq!(rsp[..5], [0x00, 0x00, 0x18, 0x00, 0x00]);
    assert_eq!(rsp[9], 2);
    assert_eq!(rsp.len(), 12);
    assert_eq!(ask(&mut net, 0xFFFD, MGMT_NWK_UPDATE_REQ, &scan(0)), None);
    for duration in [6, 0x80, 0xFD] {
        assert_eq!(ask(&mut net, short, MGMT_NWK_UPDATE_REQ, &scan(duration)), Some(vec![0x80]));
        assert_eq!(ask(&mut net, 0xFFFD, MGMT_NWK_UPDATE_REQ, &scan(duration)), None);
    }
}

#[test]
fn an_unsupported_request_is_not_supported_when_unicast_and_ignored_when_broadcast() {
    let (mut net, _, short) = bulb();
    for cluster in [0x0010u16, 0x0030, 0x0035] {
        assert_eq!(ask(&mut net, short, cluster, &[0, 0]), Some(vec![0x84]));
        assert_eq!(ask(&mut net, 0xFFFD, cluster, &[0, 0]), None);
    }
}

fn zdo_from(net: &mut Net, short: u16, src: u16, cluster: u16, payload: &[u8]) {
    let aps = net.coord.aps_frame(0x00, 0, cluster, 0x0000, 0, payload);
    let mut h = net.coord.header(nwk::FRAME_DATA, short, 30);
    h.src = src;
    net.coord.nwk_send(h, &aps, None);
}

#[test]
fn a_device_announce_cut_short_teaches_nothing() {
    let (mut net, n, short) = bulb();
    zdo_from(&mut net, short, 0x0000, 0x0013, &[0x50, 0x21, 0x43]);
    net.run(200);
    assert_eq!(net.nodes[n].stack.state(), zigbee::stack::State::Up);
}

#[test]
fn a_match_descriptor_response_without_a_match_finds_no_ota_server() {
    let mut net = Net::new();
    net.coord.ota_hidden = true;
    let n = net.join_node();
    net.run(31_000);
    let short = net.nodes[n].short();
    zdo_from(&mut net, short, 0x0000, 0x8006, &[0x40, 0x00, 0x00, 0x00, 0]);
    net.run(200);
    assert_eq!(net.coord.ota_queries, 0);
    zdo_from(&mut net, short, 0x0000, 0x8006, &[0x41, 0x80, 0x00, 0x00, 1, 1]);
    net.run(200);
    assert_eq!(net.coord.ota_queries, 0, "nor a failed one");
    zdo_from(&mut net, short, 0x0000, 0x8006, &[0x42, 0x00, 0x00, 0x00, 1, 1]);
    net.run(200);
    assert_eq!(net.coord.ota_queries, 1, "one with a match does");
}

#[test]
fn a_router_without_children_announces_none_after_a_restart() {
    let (mut net, n, short) = bulb();
    net.power_cycle(n);
    net.run(30_000);
    assert!(net.coord.zdo_rx.iter().all(|(s, c, _)| *s != short || *c != 0x001F));
}

#[test]
fn a_parent_announce_of_its_own_cut_short_or_naming_strangers_is_not_answered() {
    let (mut net, _, short) = bulb();
    let stranger = 0x00124B00000000E1u64.to_le_bytes();
    zdo_from(&mut net, short, short, 0x001F, &[[0x51, 1].as_slice(), &stranger].concat());
    zdo_from(&mut net, short, 0x0000, 0x001F, &[0x52, 2, 1, 2, 3, 4, 5, 6, 7, 8]);
    zdo_from(&mut net, short, 0x0000, 0x001F, &[[0x53, 1].as_slice(), &stranger].concat());
    net.run(500);
    assert!(net.coord.zdo_rx.iter().all(|(s, c, _)| *s != short || *c != 0x801F));
}

#[test]
fn reports_to_a_neighbouring_router_go_straight_to_its_address() {
    let mut net = Net::new();
    let a = net.join_node();
    let b = net.join_node();
    net.run(40_000);
    let (sa, sb) = (net.nodes[a].short(), net.nodes[b].short());
    let me = ext_of(&net, a);
    let eb = ext_of(&net, b);
    assert_eq!(ask(&mut net, sa, BIND_REQ, &bind_payload(me, 1, LEVEL, &unicast_dst(eb, 1))), Some(vec![0x00]));
    let t = net.now();
    net.coord.zcl(sa, LEVEL, 0x11, 0x04, &[77, 0, 0, 0, 0]);
    net.run(2_000);
    let sent = net.nwk_sent_by(net.nodes[a].id, t);
    assert!(sent.iter().any(|(_, h, p)| h.dst == sb && p.get(2..4) == Some(&LEVEL.to_le_bytes()[..])));
    assert!(!sent.iter().any(|(_, h, p)| h.dst == 0xFFFD && p.get(2..4) == Some(&[0, 0][..])), "no address asked for");
}
