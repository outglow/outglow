use crate::sim::*;
use zigbee::nwk;

#[test]
fn frames_under_an_unknown_key_make_the_bulb_rejoin_thirty_seconds_after_the_last_it_could_read() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    net.run(1_000);
    net.coord.zcl(short, 0x0006, 0x00, 0x00, &[0x00, 0x00]);
    net.run(10);
    net.run(500);
    let last_read = net.air.borrow().log.iter().rev().find(|e| e.1 == 0).unwrap().0;
    net.coord.key = [0x6A; 16];
    net.coord.key_seq = 7;
    for _ in 0..6 {
        net.coord.zcl(short, 0x0006, 0x00, 0x00, &[0x00, 0x00]);
        net.run(100);
    }
    net.run_until(60_000, "rejoin", |net| !net.coord.rejoins.is_empty());
    let rejoin = net
        .timed_nwk_sent_by(net.nodes[n].id, last_read)
        .into_iter()
        .find(|(_, _, h, p)| h.frame_type == nwk::FRAME_CMD && p.first() == Some(&nwk::CMD_REJOIN_REQUEST))
        .expect("a rejoin request")
        .0;
    assert_eq!(rejoin - last_read, 30_001 + 141, "the first moment more than thirty seconds after, then a scan of its channel");
}

#[test]
fn a_bulb_that_hears_nothing_for_three_minutes_looks_for_its_network_the_moment_after() {
    let mut net = Net::new();
    let n = net.join_node();
    net.run(1_000);
    let id = net.nodes[n].id;
    net.lossy(0, id, 100);
    let last_read = net.air.borrow().log.iter().rev().find(|e| e.1 == 0).unwrap().0;
    net.run(185_000);
    let scan = net
        .air
        .borrow()
        .log
        .iter()
        .find(|e| e.1 == id && e.0 > last_read && e.2.len() >= 8 && e.2[0] & 7 == 3 && e.2[7] == 0x07)
        .expect("a beacon request")
        .0;
    assert_eq!(scan - last_read, 180_001);
}

#[test]
fn link_status_goes_every_fourteen_to_fifteen_seconds() {
    let mut net = Net::new();
    let n = net.join_node();
    net.run(30_000);
    let t = net.now();
    net.run(120_000);
    let times: Vec<u32> = net
        .timed_nwk_sent_by(net.nodes[n].id, t)
        .into_iter()
        .filter(|(_, _, h, p)| h.frame_type == nwk::FRAME_CMD && p.first() == Some(&nwk::CMD_LINK_STATUS))
        .map(|e| e.0)
        .collect();
    assert!(times.len() >= 8, "{times:?}");
    assert!(times.windows(2).all(|w| (14_000..15_000).contains(&(w[1] - w[0]))), "{times:?}");
}

#[test]
fn aps_secured_answers_do_not_store_the_frame_counter_each_time() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    net.run(1_000);
    let id = net.nodes[n].id;
    let stores = |net: &Net| net.air.borrow().ports[id].nv.get(&(zigbee::hal::NvItem::Counters as u8)).cloned();
    let before = stores(&net);
    for _ in 0..50 {
        net.coord.zcl_encrypted(short, 0x0000, 0x00, 0x00, &[0x00, 0x00]);
        net.run(20);
    }
    assert_eq!(stores(&net), before, "fifty APS frame counters fit in the step stored");
}
