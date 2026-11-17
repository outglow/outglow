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

fn mfr_counter(net: &mut Net, short: u16, id: u16) -> u32 {
    let seq = net.coord.zcl_mfr(short, 0x0B05, 0x00, 0x1141, 0x00, &id.to_le_bytes());
    net.run(60);
    let rsp = net.coord.last_zcl_mfr(short, 0x0B05, 0x01).unwrap();
    assert_eq!(rsp[3], seq);
    rsp[9..].iter().rev().fold(0u32, |a, b| a << 8 | *b as u32)
}

#[test]
fn a_replayed_frame_for_another_device_counts_as_a_duplicate_relay_drop_and_one_for_the_bulb_does_not() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let mut h = net.coord.header(nwk::FRAME_DATA, 0x7777, 5);
    h.discover = 0;
    let aps = net.coord.aps_frame(0x00, 1, 0x0006, 0x0104, 1, &[0x00, 0x09, 0x00, 0x00, 0x00]);
    net.coord.nwk_send(h, &aps, Some(short));
    net.run(100);
    let relayed = net.air.borrow().log.iter().rev().find(|e| e.1 == 0 && e.2[0] & 7 == 1 && e.2[11..13] == [0x77, 0x77]).unwrap().2.clone();
    let before = mfr_counter(&mut net, short, 0x5008);
    net.air.borrow_mut().transmit(0, &relayed);
    net.run(50);
    assert_eq!(mfr_counter(&mut net, short, 0x5008), before + 1);
    net.coord.zcl(short, 0x0006, 0x00, 0x00, &[0x00, 0x00]);
    net.run(50);
    let mine = net.air.borrow().log.iter().rev().find(|e| e.1 == 0 && e.2[0] & 7 == 1 && e.2[11..13] == short.to_le_bytes()).unwrap().2.clone();
    let before = mfr_counter(&mut net, short, 0x5008);
    net.air.borrow_mut().transmit(0, &mine);
    net.run(50);
    assert_eq!(mfr_counter(&mut net, short, 0x5008), before);
}
