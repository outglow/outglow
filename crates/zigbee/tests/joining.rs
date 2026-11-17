use crate::sim::*;
use zigbee::stack::State;

fn ext_of(net: &Net, n: usize) -> u64 {
    net.air.borrow().ports[net.nodes[n].id].ext
}

fn association_response(net: &mut Net, n: usize, src: [u8; 8], short: u16, status: u8, to: u64) {
    let mut f = vec![0x63, 0xCC, 0x09, 0x62, 0x1A];
    f.extend_from_slice(&to.to_le_bytes());
    f.extend_from_slice(&src);
    let s = short.to_le_bytes();
    f.extend_from_slice(&[0x02, s[0], s[1], status]);
    let id = net.nodes[n].id;
    net.air.borrow_mut().ports[id].queue.push_back((f, 255));
    net.run(5);
}

fn waiting_for_the_response(net: &mut Net, n: usize) {
    net.run_until(30_000, "association sent", |net| net.nodes[n].stack.state() == State::AssocWait);
}

#[test]
fn an_association_response_refusing_the_bulb_or_with_a_reserved_address_moves_on() {
    for (status, short) in [(0x01u8, 0x1234u16), (0x00, 0xFFF8)] {
        let mut net = Net::new();
        net.coord.ignore_associations = true;
        let n = net.add_node();
        let ext = ext_of(&net, n);
        waiting_for_the_response(&mut net, n);
        association_response(&mut net, n, COORD_EXT.to_le_bytes(), short, status, ext);
        assert_ne!(net.nodes[n].stack.state(), State::WaitKey, "status {status:#04x} address {short:#06x}");
        assert_ne!(net.nodes[n].stack.state(), State::AssocWait);
    }
}

#[test]
fn an_association_response_for_another_device_or_cut_short_is_not_taken() {
    let mut net = Net::new();
    net.coord.ignore_associations = true;
    let n = net.add_node();
    let ext = ext_of(&net, n);
    waiting_for_the_response(&mut net, n);
    association_response(&mut net, n, COORD_EXT.to_le_bytes(), 0x1234, 0, ext ^ 1);
    assert_eq!(net.nodes[n].stack.state(), State::AssocWait);
    let id = net.nodes[n].id;
    let mut f = vec![0x63, 0xCC, 0x09, 0x62, 0x1A];
    f.extend_from_slice(&ext.to_le_bytes());
    f.extend_from_slice(&COORD_EXT.to_le_bytes());
    f.extend_from_slice(&[0x02, 0x34, 0x12]);
    net.air.borrow_mut().ports[id].queue.push_back((f, 255));
    net.run(5);
    assert_eq!(net.nodes[n].stack.state(), State::AssocWait);
    association_response(&mut net, n, COORD_EXT.to_le_bytes(), 0x1234, 0, ext);
    assert_eq!(net.nodes[n].stack.state(), State::WaitKey);
    assert_eq!(net.nodes[n].stack.short_address(), 0x1234);
}

#[test]
fn a_busy_channel_while_joining_moves_on_to_the_next_try_and_the_bulb_joins_later() {
    let mut net = Net::new();
    let n = net.add_node();
    let id = net.nodes[n].id;
    net.run_until(30_000, "association sent", |net| net.nodes[n].stack.state() == State::AssocPoll);
    net.air.borrow_mut().ports[id].busy = 1_000_000;
    net.run(3_000);
    assert_ne!(net.nodes[n].stack.state(), State::Up);
    net.air.borrow_mut().ports[id].busy = 0;
    net.run_until(60_000, "joined after all", |net| net.nodes[n].stack.state() == State::Up);
}

#[test]
fn after_joining_the_bulb_opens_the_network_once_and_only_after_announcing_itself() {
    let mut net = Net::new();
    let n = net.add_node();
    let id = net.nodes[n].id;
    net.run_until(30_000, "joined", |net| net.nodes[n].stack.state() == State::Up);
    net.run(10_000);
    let zdo: Vec<(u32, u16, u8)> = net
        .timed_nwk_sent_by(id, 0)
        .into_iter()
        .filter(|(_, _, h, p)| h.frame_type == zigbee::nwk::FRAME_DATA && p.len() > 6 && p[4..6] == [0x00, 0x00] && h.src == net.nodes[n].short())
        .map(|(t, _, h, p)| (t, u16::from_le_bytes([p[2], p[3]]), h.seq))
        .collect();
    let announce = zdo.iter().find(|e| e.1 == 0x0013).expect("Device_annce").0;
    let opens: std::collections::BTreeSet<u8> = zdo.iter().filter(|e| e.1 == 0x0036).map(|e| e.2).collect();
    assert_eq!(opens.len(), 1, "one Mgmt_Permit_Joining_req, sent again only as a broadcast is: {zdo:?}");
    assert!(zdo.iter().filter(|e| e.1 == 0x0036).all(|e| e.0 > announce));
}

#[test]
fn a_node_descriptor_response_after_the_key_exchange_changes_nothing() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    net.run(10_000);
    let id = net.nodes[n].id;
    let t = net.now();
    let mut rsp = vec![0x44, 0x00, 0x00, 0x00, 0x00, 0x40, 0x8F, 0x41, 0x11, 0x50, 0xA0, 0x00];
    rsp.extend_from_slice(&(0x0001u16 | 20 << 9).to_le_bytes());
    rsp.extend_from_slice(&[0xA0, 0x00, 0x00]);
    let aps = net.coord.aps_frame(0x00, 0, 0x8002, 0x0000, 0, &rsp);
    net.coord.nwk_data(short, &aps);
    net.run(2_000);
    let opens = net
        .nwk_sent_by(id, t)
        .into_iter()
        .filter(|(_, h, p)| h.frame_type == zigbee::nwk::FRAME_DATA && p.len() > 6 && p[2..4] == [0x36, 0x00])
        .count();
    assert_eq!(opens, 0, "the network is not opened again");
}

#[test]
fn a_rejoin_response_counts_only_from_the_parent_asked_for_this_bulb_and_whole() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let ext = ext_of(&net, n);
    net.coord.ignore_rejoin = true;
    net.coord.permit_join = false;
    net.coord.nwk_command(short, 1, &[zigbee::nwk::CMD_LEAVE, 0x60]);
    net.run_until(10_000, "rejoin", |net| !net.coord.rejoins.is_empty());
    let respond = |net: &mut Net, src_ext: Option<u64>, dst_ext: Option<u64>, body: &[u8]| {
        let mut rh = net.coord.header(zigbee::nwk::FRAME_CMD, short, 1);
        rh.src_ext = src_ext;
        rh.dst_ext = dst_ext;
        let mut f = vec![zigbee::nwk::CMD_REJOIN_RESPONSE];
        f.extend_from_slice(body);
        net.coord.nwk_send(rh, &f, Some(short));
        net.run(20);
    };
    respond(&mut net, Some(0x00124B00000000AB), Some(ext), &[0x77, 0x77, 0x00]);
    respond(&mut net, Some(COORD_EXT), Some(ext ^ 1), &[0x77, 0x77, 0x00]);
    respond(&mut net, Some(COORD_EXT), Some(ext), &[0x77, 0x77]);
    assert_ne!(net.nodes[n].stack.short_address(), 0x7777, "none of those was taken");
    respond(&mut net, Some(COORD_EXT), Some(ext), &[0x78, 0x77, 0x00]);
    assert_eq!(net.nodes[n].stack.short_address(), 0x7778);
}

#[test]
fn an_unsecured_rejoin_response_does_not_answer_a_secured_rejoin_and_unsecured_data_is_dropped() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let ext = ext_of(&net, n);
    net.coord.zcl_rx.clear();
    let aps = net.coord.aps_frame(0x00, 1, 0x0006, 0x0104, 1, &[0x00, 0x21, 0x00, 0x00, 0x00]);
    let mut h = net.coord.header(zigbee::nwk::FRAME_DATA, short, 1);
    h.security = false;
    net.coord.nwk_send(h, &aps, Some(short));
    net.run(200);
    assert!(net.coord.zcl_rx.iter().all(|(s, _, _)| *s != short), "unsecured data is not taken from a network that is up");
    net.coord.ignore_rejoin = true;
    net.coord.permit_join = false;
    net.coord.nwk_command(short, 1, &[zigbee::nwk::CMD_LEAVE, 0x60]);
    net.run_until(10_000, "rejoin", |net| !net.coord.rejoins.is_empty());
    let mut rh = net.coord.header(zigbee::nwk::FRAME_CMD, short, 1);
    rh.dst_ext = Some(ext);
    rh.security = false;
    net.coord.nwk_send(rh, &[zigbee::nwk::CMD_REJOIN_RESPONSE, 0x79, 0x77, 0x00], Some(short));
    net.run(20);
    assert_ne!(net.nodes[n].stack.short_address(), 0x7779, "a secured rejoin wants a secured answer");
}

#[test]
fn a_search_for_the_network_passes_over_beacons_of_another_network() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let port = net.nodes[n].id;
    net.run(1_000);
    net.coord.key = [0x6A; 16];
    net.coord.key_seq = 7;
    for _ in 0..6 {
        net.coord.zcl(short, 0x0006, 0x00, 0x00, &[0x00, 0x00]);
        net.run(100);
    }
    net.run_until(60_000, "searching", |net| net.nodes[n].stack.state() == State::Scan);
    let other = Zed::new(&net.air, false);
    net.link(port, other.id, 200);
    let mut beacon = zigbee::util::Buf::new();
    zigbee::mac::Beacon {
        pan: PAN,
        short: 0x7777,
        permit_join: true,
        pan_coordinator: false,
        stack_profile: 2,
        protocol_version: 2,
        router_capacity: true,
        end_device_capacity: true,
        depth: 1,
        ext_pan: 0x1234,
        update_id: 0,
    }
    .write(1, &mut beacon);
    for _ in 0..200 {
        if net.nodes[n].stack.state() != State::Scan {
            break;
        }
        let channel = net.air.borrow().ports[port].channel;
        net.air.borrow_mut().ports[other.id].channel = channel;
        net.air.borrow_mut().ports[other.id].powered = true;
        net.air.borrow_mut().transmit(other.id, beacon.as_slice());
        net.run(20);
    }
    net.run_until(400_000, "back up", |net| net.nodes[n].stack.state() == State::Up);
    assert_eq!((net.nodes[n].stack.channel(), net.nodes[n].stack.pan_id()), (11, PAN), "not on the other network's channel");
}

#[test]
fn a_search_after_silence_that_finds_the_network_on_its_channel_does_not_rejoin() {
    let mut net = Net::new();
    let n = net.join_node();
    let port = net.nodes[n].id;
    net.link(0, port, 0);
    net.run_until(400_000, "searching", |net| net.nodes[n].stack.state() == State::Scan);
    let other = Zed::new(&net.air, false);
    net.link(port, other.id, 200);
    let mut beacon = zigbee::util::Buf::new();
    zigbee::mac::Beacon {
        pan: PAN,
        short: 0x7777,
        permit_join: true,
        pan_coordinator: false,
        stack_profile: 2,
        protocol_version: 2,
        router_capacity: true,
        end_device_capacity: true,
        depth: 1,
        ext_pan: EXT_PAN,
        update_id: 0,
    }
    .write(1, &mut beacon);
    let t = net.now();
    for _ in 0..400 {
        if net.nodes[n].stack.state() != State::Scan {
            break;
        }
        let channel = net.air.borrow().ports[port].channel;
        if channel == 11 {
            net.air.borrow_mut().ports[other.id].channel = channel;
            net.air.borrow_mut().ports[other.id].powered = true;
            net.air.borrow_mut().transmit(other.id, beacon.as_slice());
        }
        net.run(20);
    }
    net.run_until(60_000, "up again", |net| net.nodes[n].stack.state() == State::Up);
    net.run(2_000);
    let to_router = net.sent_by(port, t).iter().filter(|f| f[0] & 7 == 1 && f[5..7] == [0x77, 0x77]).count();
    assert_eq!(to_router, 0, "no rejoin request");
}
