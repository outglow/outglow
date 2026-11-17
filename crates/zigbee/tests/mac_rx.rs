use crate::sim::*;
use zigbee::stack::State;

const DIAG: u16 = 0x0B05;

fn counter(net: &mut Net, short: u16, id: u16) -> u32 {
    let seq = net.coord.zcl(short, DIAG, 0x00, 0x00, &id.to_le_bytes());
    net.run(60);
    let rsp = net.coord.last_zcl(short, DIAG, 0x01).unwrap();
    assert_eq!(rsp[..6], [0x18, seq, 0x01, id as u8, (id >> 8) as u8, 0x00]);
    match rsp[6] {
        0x21 => u16::from_le_bytes([rsp[7], rsp[8]]) as u32,
        _ => u32::from_le_bytes([rsp[7], rsp[8], rsp[9], rsp[10]]),
    }
}

fn inject(net: &mut Net, n: usize, mpdu: &[u8]) {
    let id = net.nodes[n].id;
    net.air.borrow_mut().ports[id].queue.push_back((mpdu.to_vec(), 255));
    net.run(20);
}

#[test]
fn a_frame_whose_mac_header_does_not_parse_is_a_validation_drop() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let drops = counter(&mut net, short, 0x011A);
    inject(&mut net, n, &[0x41]);
    inject(&mut net, n, &[0x41, 0x88, 0x01, 0x62]);
    assert_eq!(counter(&mut net, short, 0x011A), drops + 2);
}

#[test]
fn mac_secured_frames_acknowledgements_and_reserved_frame_types_are_ignored() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let s = short.to_le_bytes();
    for fcf in [0x8849u16, 0x8802, 0x8844, 0x8845, 0x8846, 0x8847] {
        let f = fcf.to_le_bytes();
        inject(&mut net, n, &[f[0], f[1], 0x07, 0x62, 0x1A, s[0], s[1], 0x00, 0x00, 0x08, 0x02, s[0], s[1], 0, 0, 1, 1]);
    }
    net.run(200);
    assert_eq!(net.nodes[n].stack.state(), State::Up);
}

#[test]
fn data_frames_for_another_pan_or_from_an_extended_address_are_not_taken() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let rx = counter(&mut net, short, 0x0102);
    let s = short.to_le_bytes();
    inject(&mut net, n, &[0x41, 0x88, 0x07, 0x63, 0x1A, s[0], s[1], 0x00, 0x00, 0x08, 0x00, s[0], s[1], 0, 0, 1, 1]);
    inject(&mut net, n, &[0x41, 0xC8, 0x07, 0x62, 0x1A, s[0], s[1], 1, 2, 3, 4, 5, 6, 7, 8, 0x08, 0x00, s[0], s[1], 0, 0, 1, 1]);
    assert_eq!(counter(&mut net, short, 0x0102), rx + 3, "counted as received (and the reads themselves), nothing more");
    assert_eq!(net.nodes[n].stack.state(), State::Up);
}

#[test]
fn a_data_request_without_a_source_address_is_answered_by_nothing() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let s = short.to_le_bytes();
    let t = net.now();
    inject(&mut net, n, &[0x03, 0x08, 0x07, 0x62, 0x1A, s[0], s[1], 0x04]);
    net.run(100);
    let sent = net.sent_by(net.nodes[n].id, t);
    assert!(sent.iter().all(|f| f[0] & 7 != 1 || f[5..7] == [0xFF, 0xFF]), "no frame for anyone in particular: {sent:02x?}");
}
