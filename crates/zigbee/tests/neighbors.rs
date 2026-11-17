use crate::sim::*;
use zigbee::nwk;

const DIAG: u16 = 0x0B05;
const NEIGHBOR_REMOVED: u16 = 0x010E;

fn counter(net: &mut Net, short: u16, id: u16) -> u16 {
    let seq = net.coord.zcl(short, DIAG, 0x00, 0x00, &id.to_le_bytes());
    net.run(60);
    let rsp = net.coord.last_zcl(short, DIAG, 0x01).unwrap();
    assert_eq!(rsp[1], seq);
    u16::from_le_bytes([rsp[7], rsp[8]])
}

fn router(net: &mut Net, n: usize, short: u16) -> Zed {
    let me = net.nodes[n].short();
    let mut z = Zed::router(&net.air, short, me);
    net.link(0, z.id, 0);
    z.link_status(&[(me, 1)]);
    net.run(50);
    z
}

#[test]
fn a_full_neighbour_table_gives_up_the_stalest_router_for_a_new_one() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let removed = counter(&mut net, short, NEIGHBOR_REMOVED);
    let mut routers: Vec<Zed> = (0..15u16).map(|k| router(&mut net, n, 0x2000 + k)).collect();
    net.run(16_000);
    for z in routers.iter_mut().skip(1) {
        z.link_status(&[(short, 1)]);
    }
    net.run(100);
    assert_eq!(counter(&mut net, short, NEIGHBOR_REMOVED), removed);
    router(&mut net, n, 0x3000);
    assert!(counter(&mut net, short, NEIGHBOR_REMOVED) > removed, "sixteen were there already");
    let seq = net.coord.zdo(short, 0x0031, &[0]);
    net.run(100);
    let lqi = net.coord.last_zdo(short, 0x8031).unwrap();
    assert_eq!((lqi[0], lqi[2]), (seq, 16));
}

#[test]
fn a_router_not_heard_for_forty_link_status_periods_is_forgotten() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let removed = counter(&mut net, short, NEIGHBOR_REMOVED);
    let z = router(&mut net, n, 0x2222);
    net.lossy(z.id, net.nodes[n].id, 100);
    net.run(30 * 15_000);
    assert_eq!(counter(&mut net, short, NEIGHBOR_REMOVED), removed);
    net.run(15 * 15_000);
    assert_eq!(counter(&mut net, short, NEIGHBOR_REMOVED), removed + 1);
}

#[test]
fn a_link_status_cut_short_still_refreshes_the_router_and_a_sleepy_childs_is_ignored() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let mut z = router(&mut net, n, 0x2222);
    let mut h = z.header(nwk::FRAME_CMD, 0xFFFC);
    h.radius = 1;
    let s = short.to_le_bytes();
    z.mac_send(0xFFFF, h, &[nwk::CMD_LINK_STATUS, 0x63, s[0], s[1], 0x33]);
    let mut h = z.header(nwk::FRAME_CMD, 0xFFFC);
    h.radius = 1;
    z.mac_send(0xFFFF, h, &[nwk::CMD_LINK_STATUS]);
    net.run(100);
    let seq = net.coord.zdo(short, 0x0031, &[0]);
    net.run(100);
    let lqi = net.coord.last_zdo(short, 0x8031).unwrap();
    assert_eq!(lqi[0], seq);
    assert_eq!(lqi[2], 2, "the coordinator and the router");
}
