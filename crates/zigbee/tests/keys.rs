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

fn in_the_link_key_exchange() -> (Net, usize, u16, u64, [u8; 16]) {
    let mut net = Net::new();
    net.coord.mute_key_exchange = true;
    let n = net.add_node();
    net.run_until(20_000, "associated", |net| net.nodes[n].stack.state() == zigbee::stack::State::Up);
    net.run(1_500);
    let short = net.nodes[n].short();
    let ext = net.air.borrow().ports[net.nodes[n].id].ext;
    let link = net.coord.device(short).unwrap().tclk;
    let new_key = [0x4E; 16];
    let mut body = vec![0x05, 0x04];
    body.extend_from_slice(&new_key);
    body.extend_from_slice(&ext.to_le_bytes());
    body.extend_from_slice(&COORD_EXT.to_le_bytes());
    let aps = net.coord.aps_command(&link, 3, &body);
    net.coord.nwk_data(short, &aps);
    net.run(200);
    (net, n, short, ext, new_key)
}

fn confirm(net: &mut Net, short: u16, ext: u64, key: &[u8; 16], status: u8, key_type: u8, secured: bool) {
    let mut body = vec![0x10, status, key_type];
    body.extend_from_slice(&ext.to_le_bytes());
    let aps = if secured {
        net.coord.aps_command(key, 0, &body)
    } else {
        let mut f = vec![0x01, 0x6E];
        f.extend_from_slice(&body);
        f
    };
    net.coord.nwk_data(short, &aps);
    net.run(200);
}

#[test]
fn only_a_secured_successful_confirm_key_for_the_trust_center_link_key_ends_the_exchange() {
    let (mut net, n, short, ext, key) = in_the_link_key_exchange();
    confirm(&mut net, short, ext, &key, 0x00, 0x04, false);
    confirm(&mut net, short, ext, &key, 0xAD, 0x04, true);
    confirm(&mut net, short, ext, &key, 0x00, 0x01, true);
    net.coord.permit_join = false;
    net.run(30_000);
    assert_ne!(net.nodes[n].stack.state(), zigbee::stack::State::Up, "the exchange failed and the bulb left");
    let (mut net, n, short, ext, key) = in_the_link_key_exchange();
    confirm(&mut net, short, ext, &key, 0x00, 0x04, true);
    net.coord.permit_join = false;
    net.run(30_000);
    assert_eq!(net.nodes[n].stack.state(), zigbee::stack::State::Up);
}

#[test]
fn a_trust_center_link_key_outside_the_exchange_starts_no_verification() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let ext = net.air.borrow().ports[net.nodes[n].id].ext;
    net.run(30_000);
    let link = net.coord.device(short).unwrap().tclk;
    let mut body = vec![0x05, 0x04];
    body.extend_from_slice(&[0x4F; 16]);
    body.extend_from_slice(&ext.to_le_bytes());
    body.extend_from_slice(&COORD_EXT.to_le_bytes());
    let aps = net.coord.aps_command(&link, 3, &body);
    let t = net.now();
    net.coord.nwk_data(short, &aps);
    net.run(10_000);
    let commands = net.nwk_sent_by(net.nodes[n].id, t).into_iter().filter(|(_, h, p)| h.dst == 0 && h.frame_type == nwk::FRAME_DATA && p[0] & 3 == 1).count();
    assert_eq!(commands, 0, "no Verify-Key");
}

fn nwk_fc_failures(net: &mut Net, short: u16) -> u16 {
    let seq = net.coord.zcl(short, 0x0B05, 0x00, 0x00, &0x0112u16.to_le_bytes());
    net.run(60);
    let rsp = net.coord.last_zcl(short, 0x0B05, 0x01).unwrap();
    assert_eq!(rsp[1], seq);
    u16::from_le_bytes([rsp[7], rsp[8]])
}

#[test]
fn a_new_network_key_does_not_forget_the_frame_counters_heard_under_the_current_one() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let mut z = Zed::router(&net.air, 0x2B02, short);
    net.link(0, z.id, 0);
    z.link_status(&[(short, 1)]);
    net.run(50);
    let first = net.sent_by(z.id, 0).last().unwrap().clone();
    z.link_status(&[(short, 1)]);
    net.run(50);
    net.coord.network_key_update(short, &[0x5A; 16], 1, COORD_EXT);
    net.run(500);
    let failures = nwk_fc_failures(&mut net, short);
    net.air.borrow_mut().transmit(z.id, &first);
    net.run(50);
    assert_eq!(nwk_fc_failures(&mut net, short), failures + 1, "the old frame is still a replay");
}

#[test]
fn in_a_trust_center_rejoin_a_key_sent_in_the_name_of_another_device_is_refused() {
    let mut net = Net::new();
    let n = net.join_node();
    net.coord.permit_join = false;
    net.set_power(n, false);
    let key = net.coord.key;
    net.coord.key = [0x77; 16];
    net.coord.key_seq = 1;
    net.run(5_000);
    net.coord.aps_ext = 0x00124B00_0000_9999;
    let short = net.nodes[n].short();
    net.power_cycle(n);
    net.set_power(n, true);
    net.run_until(200_000, "Trust Center rejoin", |net| !net.coord.rejoins.is_empty());
    net.run(2_000);
    assert_eq!(net.nodes[n].stack.state(), zigbee::stack::State::WaitKey, "the key is not the Trust Center's");
    let mut beacon = zigbee::util::Buf::new();
    zigbee::mac::Beacon {
        pan: PAN,
        short: 0x0000,
        permit_join: true,
        pan_coordinator: true,
        stack_profile: 2,
        protocol_version: 2,
        router_capacity: true,
        end_device_capacity: true,
        depth: 0,
        ext_pan: EXT_PAN,
        update_id: 0,
    }
    .write(1, &mut beacon);
    net.air.borrow_mut().transmit(0, beacon.as_slice());
    net.run_until(30_000, "back on its stored network", |net| net.nodes[n].stack.state() != zigbee::stack::State::WaitKey);
    assert_eq!((net.nodes[n].stack.state(), net.nodes[n].stack.short_address()), (zigbee::stack::State::Up, short));
    net.coord.aps_ext = COORD_EXT;
    net.coord.key = key;
    net.coord.key_seq = 0;
    net.coord.zcl_rx.clear();
    net.coord.zcl(short, 0x0006, 0x00, 0x00, &[0x00, 0x00]);
    net.run(100);
    assert!(net.coord.last_zcl(short, 0x0006, 0x01).is_some(), "with the key it had");
}

fn fc_failures(net: &mut Net, short: u16) -> u16 {
    let seq = net.coord.zcl(short, 0x0B05, 0x00, 0x00, &0x0112u16.to_le_bytes());
    net.run(60);
    let rsp = net.coord.last_zcl(short, 0x0B05, 0x01).unwrap();
    assert_eq!(rsp[1], seq);
    u16::from_le_bytes([rsp[7], rsp[8]])
}

#[test]
fn a_new_network_key_forgets_the_frame_counters_only_of_devices_last_heard_under_its_number() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let ext = net.air.borrow().ports[net.nodes[n].id].ext;
    let mut z = Zed::router(&net.air, 0x4444, short);
    net.link(0, z.id, 0);
    z.fc = 100;
    z.link_status(&[(short, 1)]);
    net.run(50);
    net.coord.transport_network_key(short, ext, &[0x5A; 16], 1, true);
    net.run(100);
    let before = fc_failures(&mut net, short);
    z.fc = 50;
    z.link_status(&[(short, 1)]);
    net.run(50);
    assert_eq!(fc_failures(&mut net, short), before + 1, "still a replay under the key it was heard with");
}

#[test]
fn a_new_network_key_keeps_a_childs_frame_counter_heard_under_another_number() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let ext = net.air.borrow().ports[net.nodes[n].id].ext;
    net.coord.permit(120);
    net.run(600);
    let mut zed = Zed::new(&net.air, false);
    net.link(0, zed.id, 0);
    assert!(zed.join(&mut net, short));
    net.run(200);
    zed.fc = 100;
    let h = zed.header(zigbee::nwk::FRAME_CMD, short);
    zed.nwk_send(h, &[zigbee::nwk::CMD_TIMEOUT_REQUEST, 3, 0]);
    net.run(100);
    net.coord.transport_network_key(short, ext, &[0x5A; 16], 1, true);
    net.run(100);
    let before = fc_failures(&mut net, short);
    zed.fc = 50;
    let h = zed.header(zigbee::nwk::FRAME_CMD, short);
    zed.nwk_send(h, &[zigbee::nwk::CMD_TIMEOUT_REQUEST, 3, 0]);
    net.run(100);
    assert_eq!(fc_failures(&mut net, short), before + 1);
}
