use crate::sim::*;
use zigbee::mac::{self, Addr};
use zigbee::nwk;
use zigbee::util::Buf;

const DIAG: u16 = 0x0B05;

fn counter(net: &mut Net, short: u16, id: u16) -> u16 {
    let seq = net.coord.zcl(short, DIAG, 0x00, 0x00, &id.to_le_bytes());
    net.run(60);
    let rsp = net.coord.last_zcl(short, DIAG, 0x01).unwrap();
    assert_eq!(rsp[1], seq);
    u16::from_le_bytes([rsp[7], rsp[8]])
}

fn raw(net: &mut Net, to: u16, nwk_bytes: &[u8]) {
    let mut f = Buf::new();
    mac::Header { frame_type: mac::FRAME_DATA, security: false, pending: false, ack_request: true, seq: 0x77, dst_pan: PAN, dst: Addr::Short(to), src_pan: PAN, src: Addr::Short(0x0000) }
        .write(&mut f);
    f.bytes(nwk_bytes);
    net.air.borrow_mut().transmit(0, f.as_slice());
    net.run(20);
}

#[test]
fn a_nwk_frame_without_a_whole_header_or_auxiliary_header_is_dropped_as_invalid() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let before = counter(&mut net, short, 0x011A);
    raw(&mut net, short, &[0x08]);
    assert_eq!(counter(&mut net, short, 0x011A), before + 1, "no header");
    let mut h = Buf::new();
    let mut nh = nwk::Header::new(nwk::FRAME_DATA, short, 0x0000, 5, 1);
    nh.security = true;
    nh.write(&mut h);
    h.bytes(&[0x28, 0x01]);
    raw(&mut net, short, h.as_slice());
    assert_eq!(counter(&mut net, short, 0x011A), before + 2, "no whole auxiliary header");
}

#[test]
fn a_link_status_sent_to_the_bulb_or_in_the_name_of_a_device_that_is_no_neighbour_is_no_trouble() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let me = short.to_le_bytes();
    let mut h = net.coord.header(nwk::FRAME_CMD, short, 1);
    h.src = 0x5555;
    h.src_ext = None;
    net.coord.nwk_send(h, &[nwk::CMD_LINK_STATUS, 0x61, me[0], me[1], 0x77], Some(short));
    net.run(50);
    net.coord.nwk_command(short, 1, &[nwk::CMD_LINK_STATUS, 0x61, me[0], me[1], 0x55]);
    net.run(50);
    let seq = net.coord.zcl(short, 0x0006, 0x00, 0x00, &[0x00, 0x00]);
    net.run(100);
    assert_eq!(net.coord.last_zcl(short, 0x0006, 0x01).unwrap()[1], seq, "still answering");
}

fn mac_frame(net: &mut Net, frame_type: u8, dst: Addr, src: Addr, payload: &[u8]) {
    let mut f = Buf::new();
    mac::Header { frame_type, security: false, pending: false, ack_request: false, seq: 0x78, dst_pan: PAN, dst, src_pan: PAN, src }.write(&mut f);
    f.bytes(payload);
    net.air.borrow_mut().transmit(0, f.as_slice());
    net.run(20);
}

#[test]
fn nwk_commands_cut_short_out_of_place_or_for_another_network_change_nothing() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let pan = net.nodes[n].stack.pan_id();
    net.coord.nwk_command(short, 1, &[nwk::CMD_ROUTE_REPLY, 0, 7]);
    net.coord.nwk_command(0xFFFC, 30, &[nwk::CMD_ROUTE_REQUEST, 0, 7]);
    net.coord.nwk_command(short, 1, &[nwk::CMD_NETWORK_STATUS, nwk::STATUS_NO_ROUTE, 0x34, 0x12]);
    let mut rh = net.coord.header(nwk::FRAME_CMD, short, 1);
    rh.dst_ext = Some(net.air.borrow().ports[net.nodes[n].id].ext);
    net.coord.nwk_send(rh, &[nwk::CMD_REJOIN_RESPONSE, 0x79, 0x77, 0x00], Some(short));
    let update = |options: u8, ext_pan: u64| {
        let mut p = vec![nwk::CMD_NETWORK_UPDATE, options];
        p.extend_from_slice(&ext_pan.to_le_bytes());
        p.extend_from_slice(&[9, 0x99, 0x19]);
        p
    };
    net.coord.nwk_command(short, 1, &update(0x01, 0x1234));
    net.coord.nwk_command(short, 1, &update(0x21, EXT_PAN));
    net.run(12_000);
    assert_eq!((net.nodes[n].stack.short_address(), net.nodes[n].stack.pan_id()), (short, pan));
}

#[test]
fn mac_commands_and_beacons_a_bulb_that_is_up_does_not_take_change_nothing() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    mac_frame(&mut net, mac::FRAME_BEACON, Addr::None, Addr::Short(0x0000), &[0x00]);
    mac_frame(&mut net, mac::FRAME_CMD, Addr::Short(short), Addr::Short(0x0000), &[0x7F]);
    mac_frame(&mut net, mac::FRAME_CMD, Addr::Short(short), Addr::Short(0x0005), &[mac::CMD_ASSOC_REQ, 0x8E]);
    mac_frame(&mut net, mac::FRAME_CMD, Addr::Short(0xFFFF), Addr::Ext(0x77), &[mac::CMD_ORPHAN]);
    mac_frame(&mut net, mac::FRAME_CMD, Addr::Short(0xFFFF), Addr::Short(0x0005), &[mac::CMD_ORPHAN]);
    mac_frame(&mut net, mac::FRAME_CMD, Addr::Short(short), Addr::None, &[mac::CMD_DATA_REQ]);
    net.run(500);
    assert_eq!(net.nodes[n].stack.child_count(), 0);
    let seq = net.coord.zcl(short, 0x0006, 0x00, 0x00, &[0x00, 0x00]);
    net.run(100);
    assert_eq!(net.coord.last_zcl(short, 0x0006, 0x01).unwrap()[1], seq);
}
