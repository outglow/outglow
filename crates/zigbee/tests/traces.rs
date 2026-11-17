use crate::sim::*;
use zigbee::hal::trace;
use zigbee::nwk;

fn traces(net: &Net, n: usize) -> Vec<zigbee::hal::Trace> {
    net.air.borrow().ports[net.nodes[n].id].traces.clone()
}

#[test]
fn what_the_bulb_sends_and_drops_is_traced_with_who_it_was_for() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    net.air.borrow_mut().ports[net.nodes[n].id].traces.clear();
    net.coord.zcl(short, 0x0006, 0x00, 0x00, &[0x00, 0x00]);
    net.run(100);
    let t = traces(&net, n);
    assert!(t.iter().any(|e| e.kind == trace::TX && e.detail == trace::TX_OK && e.src == short && e.peer == 0x0000), "{t:?}");
    let mut h = net.coord.header(nwk::FRAME_DATA, 0x7777, 1);
    h.radius = 1;
    let aps = net.coord.aps_frame(0x00, 1, 0x0006, 0x0104, 1, &[0x00, 0x01, 0x00, 0x00, 0x00]);
    net.coord.nwk_send(h, &aps, Some(short));
    net.run(100);
    let t = traces(&net, n);
    assert!(t.iter().any(|e| e.kind == trace::DROP && e.detail == trace::DROP_RADIUS && e.peer == 0x0000), "{t:?}");
    let before = traces(&net, n).len();
    let replay = net.air.borrow().log.iter().rev().find(|e| e.1 == 0 && e.2[0] & 7 == 1).unwrap().2.clone();
    net.air.borrow_mut().transmit(0, &replay);
    net.run(50);
    let t = traces(&net, n)[before..].to_vec();
    assert!(t.iter().any(|e| e.kind == trace::RX_DROP && e.detail == trace::RX_REPLAY), "a unicast replay: {t:?}");
    let before = traces(&net, n).len();
    net.coord.zcl(0xFFFD, 0x0006, 0x00, 0x00, &[0x00, 0x00]);
    net.run(50);
    let replay = net.air.borrow().log.iter().rev().find(|e| e.1 == 0 && e.2[0] & 7 == 1).unwrap().2.clone();
    net.air.borrow_mut().transmit(0, &replay);
    net.run(50);
    let t = traces(&net, n)[before..].to_vec();
    assert!(t.iter().all(|e| e.kind != trace::RX_DROP), "a broadcast seen twice is no drop to trace: {t:?}");
}
