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
