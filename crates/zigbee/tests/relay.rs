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
