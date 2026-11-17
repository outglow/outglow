use crate::sim::*;
use zigbee::nwk;

fn far_router(net: &mut Net, n: usize, short: u16) -> Zed {
    let me = net.nodes[n].short();
    let mut z = Zed::router(&net.air, short, me);
    net.link(0, z.id, 0);
    z.link_status(&[(me, 1)]);
    net.run(50);
    z
}

#[test]
fn a_route_record_passed_on_gets_the_relay_added_and_one_without_a_count_is_dropped() {
    let mut net = Net::new();
    let n = net.join_node();
    let me = net.nodes[n].short();
    let mut z = far_router(&mut net, n, 0x4444);
    net.coord.route_records.clear();
    let h = z.header(nwk::FRAME_CMD, 0x0000);
    z.nwk_send(h, &[nwk::CMD_ROUTE_RECORD]);
    net.run(100);
    assert!(net.coord.route_records.is_empty(), "a record without its relay count");
    let h = z.header(nwk::FRAME_CMD, 0x0000);
    z.nwk_send(h, &[nwk::CMD_ROUTE_RECORD, 0]);
    net.run(100);
    assert_eq!(net.coord.route_records, vec![(0x4444, vec![me])]);
    net.coord.route_records.clear();
    let h = z.header(nwk::FRAME_CMD, 0x0000);
    z.nwk_send(h, &[nwk::CMD_ROUTE_RECORD, 2, 0x01, 0x30, 0x02, 0x30]);
    net.run(100);
    assert_eq!(net.coord.route_records, vec![(0x4444, vec![0x3001, 0x3002, me])]);
}

#[test]
fn a_broadcast_is_passed_on_with_one_hop_less_and_not_at_all_at_the_end_of_its_radius() {
    for (radius, passed_on) in [(1u8, false), (2, true)] {
        let mut net = Net::new();
        let n = net.join_node();
        let port = net.nodes[n].id;
        net.run(2_000);
        let t = net.now();
        let h = net.coord.header(nwk::FRAME_DATA, 0xFFFD, radius);
        let seq = h.seq;
        let aps = net.coord.aps_frame(0x08, 1, 0x0006, 0x0104, 1, &[0x01, 0x30, 0x02]);
        net.coord.nwk_send(h, &aps, Some(0xFFFF));
        net.run(300);
        let copies: Vec<u8> = net
            .sent_by(port, t)
            .iter()
            .filter(|f| f[0] & 7 == 1)
            .filter_map(|f| nwk::Header::parse(&f[9..]).map(|(h, _)| h))
            .filter(|h| h.src == 0x0000 && h.seq == seq)
            .map(|h| h.radius)
            .collect();
        assert_eq!(!copies.is_empty(), passed_on, "radius {radius}: {copies:?}");
        assert!(copies.iter().all(|&r| r == radius - 1));
    }
}

#[test]
fn a_route_record_passed_on_may_fill_the_largest_payload_and_one_without_room_for_the_relay_is_dropped() {
    let mut net = Net::new();
    let n = net.join_node();
    let me = net.nodes[n].short();
    let mut z = far_router(&mut net, n, 0x4444);
    for (relays, passed_on) in [(43usize, true), (44, false)] {
        net.coord.route_records.clear();
        let mut p = vec![nwk::CMD_ROUTE_RECORD, relays as u8];
        for k in 0..relays {
            p.extend_from_slice(&(0x3000 + k as u16).to_le_bytes());
        }
        if relays == 44 {
            p.pop();
        }
        let mut h = z.header(nwk::FRAME_CMD, 0x0000);
        h.src_ext = None;
        z.nwk_send(h, &p);
        net.run(100);
        let got = net.coord.route_records.iter().find(|(src, _)| *src == 0x4444).map(|(_, r)| r.len());
        assert_eq!(got, passed_on.then_some(relays + 1), "{} bytes", p.len());
        if passed_on {
            assert_eq!(net.coord.route_records[0].1.last(), Some(&me));
        }
    }
}

fn status_via(net: &mut Net, bulb: u16, dst: u16, status: u8) {
    let mut h = net.coord.header(nwk::FRAME_CMD, dst, 5);
    h.src_ext = None;
    net.coord.nwk_send(h, &[nwk::CMD_NETWORK_STATUS, status, 0x00, 0x50], Some(bulb));
    net.run(200);
}

#[test]
fn a_network_status_passed_on_goes_to_its_router_or_another_and_is_dropped_without_one() {
    let mut net = Net::new();
    let n = net.join_node();
    let me = net.nodes[n].short();
    let mut far = far_router(&mut net, n, 0x4444);
    let t = net.now();
    status_via(&mut net, me, 0x4444, nwk::STATUS_LINK_FAILURE);
    assert!(net.sent_by(net.nodes[n].id, t).iter().any(|f| f[5..7] == [0x44, 0x44]), "to the router");
    let port = net.nodes[n].id;
    net.lossy(port, far.id, 100);
    let t = net.now();
    status_via(&mut net, me, 0x4444, nwk::STATUS_LINK_FAILURE);
    assert!(net.sent_by(net.nodes[n].id, t).iter().filter(|f| f[5..7] == [0x44, 0x44]).count() >= 1, "tried");
    net.lossy(port, far.id, 0);
    far.link_status(&[(me, 1)]);
    net.run(50);
    let t = net.now();
    status_via(&mut net, me, 0x7777, nwk::STATUS_MANY_TO_ONE_FAILURE);
    assert!(net.sent_by(net.nodes[n].id, t).iter().any(|f| f[5..7] == [0x44, 0x44]), "another router for a many-to-one failure");
    let t = net.now();
    status_via(&mut net, me, 0x7777, nwk::STATUS_NO_ROUTE);
    assert!(net.sent_by(net.nodes[n].id, t).iter().all(|f| f[5..7] != [0x44, 0x44]), "no route, not passed on");
}

#[test]
fn a_network_status_of_a_link_failure_for_an_end_device_child_drops_the_route_it_tells_of() {
    let mut net = Net::new();
    let a = net.join_node();
    net.coord.permit(120);
    net.run(600);
    let mut zed = Zed::new(&net.air, false);
    net.link(0, zed.id, 0);
    let sa = net.nodes[a].short();
    assert!(zed.join(&mut net, sa));
    net.run(200);
    zed.rx.clear();
    let mut h = net.coord.header(nwk::FRAME_CMD, zed.short, 5);
    h.src_ext = None;
    net.coord.nwk_send(h, &[nwk::CMD_NETWORK_STATUS, nwk::STATUS_LINK_FAILURE, 0x00, 0x50], Some(sa));
    net.run(200);
    zed.drain();
    assert!(zed.rx.iter().any(|(h, p)| h.src == 0x0000 && p.first() == Some(&nwk::CMD_NETWORK_STATUS)));
}

#[test]
fn a_source_routed_frame_whose_index_points_at_another_relay_is_dropped() {
    let mut net = Net::new();
    let n = net.join_node();
    let me = net.nodes[n].short();
    let _far = far_router(&mut net, n, 0x4444);
    let mut list = [0u16; nwk::MAX_RELAYS];
    list[..2].copy_from_slice(&[0x4444, 0x1234]);
    let t = net.now();
    let mut h = net.coord.header(nwk::FRAME_DATA, 0x4444, 5);
    h.relays = Some((2, 1, list));
    let aps = net.coord.aps_frame(0x00, 1, 0x0006, 0x0104, 1, &[0x00, 0x09, 0x00, 0x00, 0x00]);
    net.coord.nwk_send(h, &aps, Some(me));
    net.run(200);
    assert!(net.sent_by(net.nodes[n].id, t).iter().all(|f| f[5..7] != [0x44, 0x44]));
}

#[test]
fn a_many_to_one_failure_status_another_router_does_not_take_or_with_no_other_router_is_dropped() {
    let mut net = Net::new();
    let n = net.join_node();
    let me = net.nodes[n].short();
    let port = net.nodes[n].id;
    let far = far_router(&mut net, n, 0x4444);
    net.lossy(port, far.id, 100);
    let t = net.now();
    status_via(&mut net, me, 0x7777, nwk::STATUS_MANY_TO_ONE_FAILURE);
    assert!(net.sent_by(port, t).iter().any(|f| f[5..7] == [0x44, 0x44]), "tried the other router");
    let mut net = Net::new();
    let n = net.join_node();
    let me = net.nodes[n].short();
    let port = net.nodes[n].id;
    let t = net.now();
    status_via(&mut net, me, 0x7777, nwk::STATUS_MANY_TO_ONE_FAILURE);
    assert!(net.sent_by(port, t).iter().all(|f| f[5..7] == [0xFF, 0xFF] || f[5..7] == [0x00, 0x00] && f[0] & 7 != 1), "no other router: dropped");
}

#[test]
fn a_frame_without_a_payload_is_passed_on_as_it_is() {
    let mut net = Net::new();
    let n = net.join_node();
    let me = net.nodes[n].short();
    let port = net.nodes[n].id;
    let _far = far_router(&mut net, n, 0x4444);
    let t = net.now();
    let mut h = net.coord.header(nwk::FRAME_DATA, 0x4444, 5);
    h.discover = 0;
    net.coord.nwk_send(h, &[], Some(me));
    net.run(200);
    assert_eq!(net.sent_by(port, t).iter().filter(|f| f[5..7] == [0x44, 0x44]).count(), 1, "no payload");
}

fn route_request(z: &mut Zed, id: u8, cost: u8, radius: u8) -> u8 {
    let mut h = z.header(nwk::FRAME_CMD, 0xFFFC);
    h.src = 0x2000;
    h.src_ext = None;
    h.radius = radius;
    z.mac_send(0xFFFF, h, &[nwk::CMD_ROUTE_REQUEST, 0, id, 0x55, 0x55, cost]);
    h.seq
}

#[test]
fn a_cheaper_copy_of_a_route_request_replaces_the_one_waiting_and_one_at_the_end_of_its_radius_is_not_passed_on() {
    let mut net = Net::new();
    let n = net.join_node();
    let port = net.nodes[n].id;
    let mut a = far_router(&mut net, n, 0x4444);
    let mut b = far_router(&mut net, n, 0x4445);
    let me = net.nodes[n].short();
    for z in [&mut a, &mut b] {
        z.link_status(&[(me, 1)]);
    }
    net.run(50);
    let traces = |net: &Net| net.air.borrow().ports[port].traces.clone();
    net.air.borrow_mut().ports[port].traces.clear();
    route_request(&mut a, 7, 9, 5);
    net.run(1);
    route_request(&mut b, 7, 1, 5);
    net.run(500);
    let passed: Vec<u32> = traces(&net).iter().filter(|t| t.kind == zigbee::hal::trace::TX && t.src == 0x2000).map(|t| t.aux).collect();
    assert!(!passed.is_empty(), "passed on");
    net.air.borrow_mut().ports[port].traces.clear();
    let seq = route_request(&mut a, 8, 1, 1);
    net.run(500);
    assert!(traces(&net).iter().all(|t| !(t.kind == zigbee::hal::trace::TX && t.src == 0x2000 && t.seq == seq)), "at the end of its radius");
}
