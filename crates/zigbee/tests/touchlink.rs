//! Touchlink target: scan, identify, reset to factory new.

mod sim;

use bulb_core::command::Command;
use sim::*;
use zigbee::aes::Aes128;
use zigbee::mac::{self, Addr};
use zigbee::nwk;
use zigbee::stack::{Identity, State};
use zigbee::util::Buf;

const INITIATOR: u64 = 0x00AA_BBCC_DDEE_FF01;

/// A touchlink initiator: a radio with an address and nothing else. It has
/// no network: its radio takes frames for the broadcast PAN only, and the
/// source PAN identifier in its frames is one it made up.
struct Initiator {
    port: usize,
    seq: u8,
    /// Destination PAN of its frames: the broadcast PAN, as inter-PAN frames have it.
    dst_pan: u16,
}

impl Initiator {
    fn new(net: &mut Net) -> Initiator {
        let port = net.air.borrow_mut().add_port(INITIATOR);
        Initiator { port, seq: 0, dst_pan: 0xFFFF }
    }

    /// Sends a Touchlink Commissioning command inter-PAN: broadcast, or to `to`.
    fn send(&mut self, net: &mut Net, to: Option<u64>, cmd: u8, payload: &[u8]) {
        self.seq += 1;
        let mut f = Buf::new();
        mac::Header {
            frame_type: mac::FRAME_DATA,
            security: false,
            pending: false,
            ack_request: to.is_some(),
            seq: self.seq,
            dst_pan: self.dst_pan,
            dst: to.map_or(Addr::Short(0xFFFF), Addr::Ext),
            src_pan: 0x1234,
            src: Addr::Ext(INITIATOR),
        }
        .write(&mut f);
        f.u16(0x000B);
        f.u8(if to.is_some() { 0x03 } else { 0x0B });
        f.u16(0x1000);
        f.u16(0xC05E);
        f.u8(0x11);
        f.u8(self.seq);
        f.u8(cmd);
        f.bytes(payload);
        net.air.borrow_mut().transmit(self.port, f.as_slice());
        net.run(20);
    }

    /// Frames received since the last call: (ZCL command, payload after it).
    fn received(&mut self, net: &mut Net) -> Vec<(u8, Vec<u8>)> {
        let mut out = vec![];
        loop {
            let next = net.air.borrow_mut().ports[self.port].queue.pop_front();
            let Some((mpdu, _)) = next else {
                break;
            };
            let (h, off) = mac::Header::parse(&mpdu).unwrap();
            let p = &mpdu[off..];
            // only frames addressed to the initiator, inter-PAN
            if h.dst == Addr::Ext(INITIATOR) && p[0] & 3 == 3 {
                assert_eq!(&p[..7], &[0x0B, 0x00, 0x03, 0x00, 0x10, 0x5E, 0xC0], "NWK and APS stubs, cluster, profile");
                assert_eq!(p[7], 0x19);
                out.push((p[9], p[10..].to_vec()));
            }
        }
        out
    }
}

const TRANSACTION: [u8; 4] = [0x78, 0x56, 0x34, 0x12];

fn scan(net: &mut Net, i: &mut Initiator) -> Vec<(u8, Vec<u8>)> {
    let mut p = TRANSACTION.to_vec();
    p.extend_from_slice(&[0x05, 0x12]);
    i.send(net, None, 0x00, &p);
    i.received(net)
}

/// MAC frame control, destination PAN and source PAN of the inter-PAN frames `port` sent since `since`.
fn interpan_headers(net: &Net, port: usize, since: u32) -> Vec<(u16, u16, u16)> {
    net.sent_by(port, since)
        .iter()
        .filter(|f| f.len() > 23 && f[0] & 7 == 1 && f[1] & 0xCC == 0xCC && f[23] & 3 == 3)
        .map(|f| (u16::from_le_bytes([f[0], f[1]]), u16::from_le_bytes([f[3], f[4]]), u16::from_le_bytes([f[13], f[14]])))
        .collect()
}

#[test]
fn responses_go_to_the_broadcast_pan_from_the_bulbs_pan_or_a_made_up_one() {
    // data frame, acknowledgement wanted, both addresses extended, both PAN identifiers present
    const FRAME_CONTROL: u16 = 0xCC21;
    // on a network: from its PAN (ZCL 13.3.2.3.1), to the broadcast PAN (13.3.4.5), which is
    // the only one the radio of an initiator without a network listens on
    let mut net = Net::new();
    let n = net.join_node();
    let mut i = Initiator::new(&mut net);
    let t = net.now();
    assert_eq!(scan(&mut net, &mut i).len(), 1);
    assert_eq!(interpan_headers(&net, net.nodes[n].id, t), vec![(FRAME_CONTROL, 0xFFFF, PAN)]);
    // factory new: from any PAN identifier that is one, the same through the transaction
    let mut net = Net::new();
    net.coord.permit_join = false;
    let n = net.add_node();
    let (port, ext) = (net.nodes[n].id, net.air.borrow().ports[net.nodes[n].id].ext);
    let mut i = Initiator::new(&mut net);
    let t = net.now();
    let (response_id, _) = scan_for(&mut net, &mut i);
    let mut q = TRANSACTION.to_vec();
    q.push(0);
    i.send(&mut net, Some(ext), 0x02, &q);
    i.send(&mut net, Some(ext), 0x10, &start_request(response_id));
    let rsp = i.received(&mut net);
    assert_eq!(rsp.iter().map(|r| r.0).collect::<Vec<_>>(), vec![0x03, 0x11]);
    let headers = interpan_headers(&net, port, t);
    assert_eq!(headers.len(), 3, "scan response, device information response, network start response");
    let made_up = headers[0].2;
    assert!((0x0001..=0xFFFE).contains(&made_up), "source PAN {made_up:#06x}");
    assert_eq!(headers, vec![(FRAME_CONTROL, 0xFFFF, made_up); 3]);
    // an initiator that answers to the PAN a new bulb's frames name, not to the broadcast PAN, is heard too
    let mut net = Net::new();
    net.coord.permit_join = false;
    let n = net.add_node();
    let (port, ext) = (net.nodes[n].id, net.air.borrow().ports[net.nodes[n].id].ext);
    let mut i = Initiator::new(&mut net);
    let t = net.now();
    scan_for(&mut net, &mut i);
    i.dst_pan = interpan_headers(&net, port, t).last().unwrap().2;
    i.send(&mut net, Some(ext), 0x02, &q);
    assert_eq!(i.received(&mut net).iter().map(|r| r.0).collect::<Vec<_>>(), vec![0x03]);
    // and once the transaction is over the bulb listens as before (and looks for a network again)
    net.run(12_000);
    assert_eq!(net.air.borrow().ports[port].pan, 0xFFFF);
}

/// A scan request of the transaction `id`.
fn scan_with(net: &mut Net, i: &mut Initiator, id: [u8; 4]) -> Vec<(u8, Vec<u8>)> {
    let mut p = id.to_vec();
    p.extend_from_slice(&[0x05, 0x12]);
    i.send(net, None, 0x00, &p);
    i.received(net)
}

#[test]
fn the_repeats_of_a_scan_are_not_answered_again_so_the_response_identifier_holds() {
    // An initiator broadcasts five scan requests of one transaction on its first
    // channel (BDB 3.0.1 8.6 step 3). The bulb answers once and discards the
    // repeats (8.7): a second answer would carry a new response identifier, and
    // the transport key comes from the one the initiator kept (ZCL 8 13.3.4.11).
    let mut net = Net::new();
    net.coord.permit_join = false;
    let n = net.add_node();
    let ext = net.air.borrow().ports[net.nodes[n].id].ext;
    let mut i = Initiator::new(&mut net);
    let (response_id, _) = scan_for(&mut net, &mut i);
    for _ in 0..4 {
        net.run(250);
        assert!(scan(&mut net, &mut i).is_empty(), "a repeat of the transaction it answered");
    }
    // the key handed over under the first (and only) response identifier is the one it takes
    i.send(&mut net, Some(ext), 0x12, &join_request(15, response_id));
    let rsp = i.received(&mut net);
    assert_eq!((rsp[0].0, rsp[0].1[4]), (0x13, 0x00), "Network Join Router Response, success");
    net.run(1_000);
    assert_eq!((net.nodes[n].stack.state(), net.nodes[n].stack.pan_id()), (State::Up, 0x4321));

    // a new transaction is answered afresh, its repeats are not; once a
    // transaction is over its identifier opens a new one
    let mut net = Net::new();
    net.join_node();
    let mut i = Initiator::new(&mut net);
    let first = scan(&mut net, &mut i);
    assert_eq!(first.len(), 1);
    assert!(scan(&mut net, &mut i).is_empty());
    let other = [0x01, 0x02, 0x03, 0x04];
    let second = scan_with(&mut net, &mut i, other);
    assert_eq!(second.len(), 1, "a new transaction");
    assert_eq!(&second[0].1[..4], &other);
    assert_ne!(second[0].1[9..13], first[0].1[9..13], "with a response identifier of its own");
    assert!(scan_with(&mut net, &mut i, other).is_empty());
    net.run(8_500);
    assert_eq!(scan(&mut net, &mut i).len(), 1, "after the transaction's lifetime");
}

#[test]
fn a_scan_request_that_is_not_from_a_link_initiator_is_ignored() {
    let mut net = Net::new();
    net.join_node();
    let mut i = Initiator::new(&mut net);
    // touchlink information without the link initiator bit (BDB 8.7 step 2)
    let mut p = TRANSACTION.to_vec();
    p.extend_from_slice(&[0x05, 0x02]);
    i.send(&mut net, None, 0x00, &p);
    assert!(i.received(&mut net).is_empty());
    // cut short before that field
    i.send(&mut net, None, 0x00, &TRANSACTION);
    assert!(i.received(&mut net).is_empty());
    assert_eq!(scan(&mut net, &mut i).len(), 1);
}

#[test]
fn a_network_of_its_own_goes_to_the_quietest_primary_channel_or_where_the_initiator_says() {
    let start = |energy: [u8; 4], channel: u8| {
        let mut net = Net::new();
        net.coord.permit_join = false;
        for (c, e) in [11, 15, 20, 25].into_iter().zip(energy) {
            net.air.borrow_mut().energy[c - 11] = e;
        }
        // loud, but not a primary channel: not looked at
        net.air.borrow_mut().energy[12 - 11] = 0;
        let n = net.add_node();
        let ext = net.air.borrow().ports[net.nodes[n].id].ext;
        let mut i = Initiator::new(&mut net);
        let (response_id, _) = scan_for(&mut net, &mut i);
        let mut p = start_request(response_id);
        p[4 + 8 + 1 + 16] = channel;
        // the answer comes on the channel the request came on, wherever the network then goes
        i.send(&mut net, Some(ext), 0x10, &p);
        let rsp = i.received(&mut net);
        assert_eq!((rsp[0].0, rsp[0].1[4]), (0x11, 0x00), "Network Start Response, success");
        net.run(1_000);
        assert_eq!(net.nodes[n].stack.state(), State::Up);
        (rsp[0].1[14], net.nodes[n].stack.channel())
    };
    // left to the bulb (BDB 8.7 step 10): the quietest of 11, 15, 20, 25; the first of equals
    assert_eq!(start([200, 90, 40, 70], 0), (20, 20));
    assert_eq!(start([200, 90, 140, 70], 0), (25, 25));
    assert_eq!(start([50, 50, 50, 50], 0), (11, 11));
    // named by the initiator: that one, however loud
    assert_eq!(start([0, 255, 0, 0], 15), (15, 15));
}

#[test]
fn a_scan_from_close_by_is_answered_with_the_bulbs_network() {
    let mut net = Net::new();
    let n = net.join_node();
    let mut i = Initiator::new(&mut net);
    let rsp = scan(&mut net, &mut i);
    assert_eq!(rsp.len(), 1);
    let (cmd, p) = &rsp[0];
    assert_eq!(*cmd, 0x01);
    assert_eq!(&p[..4], &TRANSACTION);
    assert_eq!(&p[4..9], &[0, 0x05, 0x80, 0x01, 0x80], "RSSI correction, router with receiver on, Zigbee 3.0 and not factory new, development and certification keys");
    assert_eq!(&p[13..21], &EXT_PAN.to_le_bytes());
    assert_eq!(p[22], 11, "logical channel");
    assert_eq!(&p[23..25], &PAN.to_le_bytes());
    assert_eq!(&p[25..27], &net.nodes[n].short().to_le_bytes());
    assert_eq!(&p[27..], &[1, 0, 1, 0x04, 0x01, 0x01, 0x01, 1, 0], "one sub-device: endpoint 1, HA dimmable light");
    // device information
    let mut q = TRANSACTION.to_vec();
    q.push(0);
    let ext = net.air.borrow().ports[net.nodes[n].id].ext;
    i.send(&mut net, Some(ext), 0x02, &q);
    let rsp = i.received(&mut net);
    assert_eq!(rsp[0].0, 0x03);
    assert_eq!(&rsp[0].1[4..7], &[1, 0, 1]);
    assert_eq!(&rsp[0].1[7..15], &ext.to_le_bytes());
}

#[test]
fn a_scan_from_across_the_room_is_ignored() {
    let mut net = Net::new();
    let n = net.join_node();
    let mut i = Initiator::new(&mut net);
    let (pi, pn) = (i.port, net.nodes[n].id);
    net.link(pi, pn, 120);
    assert!(scan(&mut net, &mut i).is_empty());
}

#[test]
fn identify_and_reset_need_the_scans_transaction() {
    let mut net = Net::new();
    let mut i = Initiator::new(&mut net);
    let (n, ext, _) = on_a_touchlink_network(&mut net, &mut i);
    net.nodes[n].commands.clear();
    // without a scan first: nothing
    let mut identify = TRANSACTION.to_vec();
    identify.extend_from_slice(&[5, 0]);
    i.send(&mut net, Some(ext), 0x06, &identify);
    i.send(&mut net, Some(ext), 0x07, &TRANSACTION);
    net.run(500);
    assert!(net.nodes[n].commands.is_empty());
    assert_eq!(net.nodes[n].stack.state(), State::Up);
    // after a scan: identify blinks
    assert_eq!(scan(&mut net, &mut i).len(), 1);
    i.send(&mut net, Some(ext), 0x06, &identify);
    assert_eq!(net.nodes[n].commands.last(), Some(&Command::Identify { times: 5 }));
    // a reset with another transaction identifier is not for us
    i.send(&mut net, Some(ext), 0x07, &[1, 2, 3, 4]);
    net.run(500);
    assert_eq!(net.nodes[n].stack.state(), State::Up);
    // the transaction closes after eight seconds
    net.run(9_000);
    i.send(&mut net, Some(ext), 0x07, &TRANSACTION);
    net.run(500);
    assert_eq!(net.nodes[n].stack.state(), State::Up);
}

#[test]
fn a_bulb_on_a_network_with_a_trust_center_is_not_for_the_taking() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let ext = net.air.borrow().ports[net.nodes[n].id].ext;
    let mut i = Initiator::new(&mut net);
    // Reset to Factory New: ignored
    assert_eq!(scan(&mut net, &mut i).len(), 1);
    i.send(&mut net, Some(ext), 0x07, &TRANSACTION);
    net.run(1_000);
    assert_eq!(net.nodes[n].stack.state(), State::Up);
    // Network Join Router and Network Start, under a key it has: refused
    let (response_id, _) = scan_for(&mut net, &mut i);
    i.send(&mut net, Some(ext), 0x12, &join_request(0, response_id));
    let rsp = i.received(&mut net);
    assert_eq!((rsp[0].0, rsp[0].1[4]), (0x13, 0x01), "Network Join Router Response: failure");
    let (response_id, _) = scan_for(&mut net, &mut i);
    i.send(&mut net, Some(ext), 0x10, &start_request(response_id));
    let rsp = i.received(&mut net);
    assert_eq!((rsp[0].0, rsp[0].1[4]), (0x11, 0x01), "Network Start Response: failure");
    net.run(1_000);
    assert_eq!((net.nodes[n].stack.state(), net.nodes[n].stack.pan_id(), net.nodes[n].short()), (State::Up, PAN, short));
    let seq = net.coord.zcl(short, 0x0006, 0x01, 0x00, &[]);
    net.run(100);
    assert_eq!(net.coord.last_zcl(short, 0x0006, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x00, 0x00]);
}

/// A factory-new bulb that an initiator had start a network (development key): the bulb, its
/// extended address, and the network's PAN identifier.
fn on_a_touchlink_network(net: &mut Net, i: &mut Initiator) -> (usize, u64, u16) {
    net.coord.permit_join = false;
    let n = net.add_node();
    let ext = net.air.borrow().ports[net.nodes[n].id].ext;
    let (response_id, _) = scan_for(net, i);
    // (the bulb may be mid-scan on another channel: repeat as an initiator would)
    let mut rsp = vec![];
    for _ in 0..20 {
        i.send(net, Some(ext), 0x10, &start_request(response_id));
        rsp = i.received(net);
        if !rsp.is_empty() {
            break;
        }
        net.run(300);
    }
    assert_eq!((rsp[0].0, rsp[0].1[4]), (0x11, 0x00), "Network Start Response, success");
    net.run(1_000);
    assert_eq!(net.nodes[n].stack.state(), State::Up);
    (n, ext, net.nodes[n].stack.pan_id())
}

#[test]
fn reset_to_factory_new_takes_the_bulb_off_a_touchlink_network() {
    let mut net = Net::new();
    let mut i = Initiator::new(&mut net);
    let (n, ext, _) = on_a_touchlink_network(&mut net, &mut i);
    assert_eq!(scan(&mut net, &mut i).len(), 1);
    i.send(&mut net, Some(ext), 0x07, &TRANSACTION);
    net.run(1_000);
    assert_ne!(net.nodes[n].stack.state(), State::Up);
    net.power_cycle(n);
    assert_eq!(net.nodes[n].stack.state(), State::Down, "nothing of the network is left");
}

#[test]
fn a_network_update_moves_the_bulb_to_the_networks_new_channel() {
    let mut net = Net::new();
    let mut i = Initiator::new(&mut net);
    let (n, ext, pan) = on_a_touchlink_network(&mut net, &mut i);
    let (_, scan_rsp) = scan_for(&mut net, &mut i);
    let ext_pan = &scan_rsp[13..21];
    let update = |update_id: u8, channel: u8, ext_pan: &[u8]| {
        let mut p = TRANSACTION.to_vec();
        p.extend_from_slice(ext_pan);
        p.extend_from_slice(&[update_id, channel]);
        p.extend_from_slice(&pan.to_le_bytes());
        p.extend_from_slice(&[0x01, 0x00]);
        p
    };
    // about some other network: not ours to follow
    i.send(&mut net, Some(ext), 0x16, &update(5, 20, &[9; 8]));
    net.run(200);
    assert_eq!(net.nodes[n].stack.channel(), 11);
    // ours, and newer than what the bulb knows
    i.send(&mut net, Some(ext), 0x16, &update(5, 20, ext_pan));
    net.run(200);
    assert_eq!(net.nodes[n].stack.channel(), 20);
    // old news changes nothing
    net.air.borrow_mut().ports[i.port].channel = 20;
    let (_, scan_rsp) = scan_for(&mut net, &mut i);
    assert_eq!((scan_rsp[21], scan_rsp[22]), (5, 20), "update identifier and channel in the scan response");
    i.send(&mut net, Some(ext), 0x16, &update(4, 15, ext_pan));
    net.run(200);
    assert_eq!(net.nodes[n].stack.channel(), 20);
    net.power_cycle(n);
    assert_eq!(net.nodes[n].stack.channel(), 20);
    // a request to join as an end device: this is a router
    let (response_id, _) = scan_for(&mut net, &mut i);
    i.send(&mut net, Some(ext), 0x14, &join_request(0, response_id));
    let rsp = i.received(&mut net);
    assert_eq!((rsp[0].0, rsp[0].1[4]), (0x15, 0x01));
}

// ------------------------------------------------------------ taking over a bulb

const NEW_KEY: [u8; 16] = [0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2A, 0x2B, 0x2C, 0x2D, 0x2E, 0x2F, 0x30];
const TRANSACTION_ID: u32 = 0x1234_5678;

/// The network key as an initiator encrypts it for the transaction.
fn encrypted_key(index: u8, response_id: u32) -> [u8; 16] {
    encrypted_with(index, response_id, &core::array::from_fn(|i| 0xC0 + i as u8))
}

/// The same with the key of index 4 or 15 given.
fn encrypted_with(index: u8, response_id: u32, key: &[u8; 16]) -> [u8; 16] {
    let (t, r) = (TRANSACTION_ID.to_be_bytes(), response_id.to_be_bytes());
    let mut input = [0u8; 16];
    input[4..8].copy_from_slice(&t);
    input[12..16].copy_from_slice(&r);
    let transport = if index == 0 {
        input[..4].copy_from_slice(b"PhLi");
        input[8..12].copy_from_slice(b"CLSN");
        input
    } else {
        input[..4].copy_from_slice(&t);
        input[8..12].copy_from_slice(&r);
        Aes128::new(key).encrypt(&mut input);
        input
    };
    let mut key = NEW_KEY;
    Aes128::new(&transport).encrypt(&mut key);
    key
}

/// Scans until the bulb answers; returns the response identifier it chose.
fn scan_for(net: &mut Net, i: &mut Initiator) -> (u32, Vec<u8>) {
    for _ in 0..40 {
        let rsp = scan(net, i);
        if let Some((0x01, p)) = rsp.first() {
            return (u32::from_le_bytes([p[9], p[10], p[11], p[12]]), p.clone());
        }
        net.run(300);
    }
    panic!("no scan response");
}

/// Network Join Router Request: the network 0x4321 on channel 20, address 0x0042.
fn join_request(key_index: u8, response_id: u32) -> Vec<u8> {
    let mut p = TRANSACTION.to_vec();
    p.extend_from_slice(&0x1111_2222_3333_4444u64.to_le_bytes());
    p.push(key_index);
    p.extend_from_slice(&encrypted_key(key_index, response_id));
    p.extend_from_slice(&[3, 20, 0x21, 0x43, 0x42, 0x00]);
    p.extend_from_slice(&[0; 12]);
    p
}

/// Network Start Request: extended PAN, channel and PAN left to the bulb; address 0x0001.
fn start_request(response_id: u32) -> Vec<u8> {
    let mut p = TRANSACTION.to_vec();
    p.extend_from_slice(&[0; 8]);
    p.push(0);
    p.extend_from_slice(&encrypted_key(0, response_id));
    p.extend_from_slice(&[0, 0, 0, 0x01, 0x00]);
    p.extend_from_slice(&[0; 12]);
    p.extend_from_slice(&INITIATOR.to_le_bytes());
    p.extend_from_slice(&[0x02, 0x00]);
    p
}

/// True when the frame is a NWK frame of the bulb that decrypts with `key`.
fn secured_with(frame: &[u8], key: &[u8; 16]) -> bool {
    let Some((mh, off)) = mac::Header::parse(frame) else {
        return false;
    };
    if mh.frame_type != mac::FRAME_DATA {
        return false;
    }
    let p = &frame[off..];
    let Some((h, hl)) = nwk::Header::parse(p) else {
        return false;
    };
    let Some(aux) = nwk::parse_aux(p, hl) else {
        return false;
    };
    let mut copy = p.to_vec();
    h.security && nwk::unsecure(&Aes128::new(key), &aux, &mut copy, hl).is_some()
}

#[test]
fn a_join_request_moves_the_bulb_to_the_initiators_network() {
    for key_index in [0u8, 15] {
        let mut net = Net::new();
        let mut i = Initiator::new(&mut net);
        // from a touchlink network of its own (index 0), and straight out of the box (index 15)
        let (n, ext) = if key_index == 0 {
            let (n, ext, _) = on_a_touchlink_network(&mut net, &mut i);
            (n, ext)
        } else {
            net.coord.permit_join = false;
            let n = net.add_node();
            (n, net.air.borrow().ports[net.nodes[n].id].ext)
        };
        let (response_id, _) = scan_for(&mut net, &mut i);
        let t = net.now();
        i.send(&mut net, Some(ext), 0x12, &join_request(key_index, response_id));
        let rsp = i.received(&mut net);
        assert_eq!(rsp[0].0, 0x13, "Network Join Router Response");
        assert_eq!(rsp[0].1[4], 0x00, "success (key index {key_index})");
        net.run(1_000);
        let stack = &net.nodes[n].stack;
        assert_eq!((stack.state(), stack.pan_id(), stack.channel(), stack.short_address()), (State::Up, 0x4321, 20, 0x0042));
        // what it sends now is under the key the initiator gave it
        net.air.borrow_mut().ports[i.port].channel = 20;
        net.run(20_000);
        let sent = net.sent_by(net.nodes[n].id, t + 100);
        let nwk_frames: Vec<&Vec<u8>> = sent.iter().filter(|f| f[0] & 7 == 1 && f[3..5] == [0x21, 0x43]).collect();
        assert!(nwk_frames.len() >= 2, "announce and link status on the new PAN");
        assert!(nwk_frames.iter().all(|f| secured_with(f, &NEW_KEY)));
        // and it stays there
        net.power_cycle(n);
        assert_eq!((net.nodes[n].stack.state(), net.nodes[n].stack.pan_id()), (State::Up, 0x4321));
    }
}

#[test]
fn a_request_under_a_key_the_bulb_does_not_have_is_refused() {
    let mut net = Net::new();
    let mut i = Initiator::new(&mut net);
    let (n, ext, pan) = on_a_touchlink_network(&mut net, &mut i);
    let (response_id, _) = scan_for(&mut net, &mut i);
    // key index 4: the master key of commercial products
    i.send(&mut net, Some(ext), 0x12, &join_request(4, response_id));
    let rsp = i.received(&mut net);
    assert_eq!((rsp[0].0, rsp[0].1[4]), (0x13, 0x01), "failure");
    net.run(1_000);
    assert_eq!((net.nodes[n].stack.state(), net.nodes[n].stack.pan_id()), (State::Up, pan));
    // and a join request without a scan before it is not even answered
    i.send(&mut net, Some(ext), 0x12, &join_request(0, response_id));
    assert!(i.received(&mut net).is_empty());
}

#[test]
fn a_new_bulb_starts_a_network_for_the_initiator_and_admits_devices_to_it() {
    let mut net = Net::new();
    net.coord.permit_join = false;
    let n = net.add_node();
    let ext = net.air.borrow().ports[net.nodes[n].id].ext;
    let mut i = Initiator::new(&mut net);
    let (response_id, scan_rsp) = scan_for(&mut net, &mut i);
    assert_eq!(scan_rsp[6], 0x81, "factory new");
    let p = start_request(response_id);
    // (the bulb may be mid-scan on another channel: repeat as an initiator would)
    let mut rsp = vec![];
    for _ in 0..20 {
        i.send(&mut net, Some(ext), 0x10, &p);
        rsp = i.received(&mut net);
        if !rsp.is_empty() {
            break;
        }
        net.run(300);
    }
    assert_eq!((rsp[0].0, rsp[0].1[4]), (0x11, 0x00), "Network Start Response, success");
    let ext_pan = u64::from_le_bytes(rsp[0].1[5..13].try_into().unwrap());
    let channel = rsp[0].1[14];
    let pan = u16::from_le_bytes([rsp[0].1[15], rsp[0].1[16]]);
    assert!(ext_pan != 0 && pan != 0 && pan != 0xFFFF && channel == 11);
    net.run(1_000);
    let stack = &net.nodes[n].stack;
    assert_eq!((stack.state(), stack.pan_id(), stack.channel(), stack.short_address()), (State::Up, pan, 11, 0x0001));

    // a device that was given the key (as the initiator has it) rejoins through the bulb
    let mut remote = Zed::new(&net.air, true);
    remote.pan = pan;
    remote.parent = 0x0001;
    remote.short = 0x0002;
    remote.key = Some(NEW_KEY);
    {
        let mut air = net.air.borrow_mut();
        air.ports[remote.id].pan = pan;
        air.ports[remote.id].short = 0x0002;
    }
    let mut h = remote.header(nwk::FRAME_CMD, 0x0001);
    h.radius = 1;
    let cap = remote.capability();
    remote.nwk_send(h, &[nwk::CMD_REJOIN_REQUEST, cap]);
    net.run(100);
    remote.poll(&mut net, 20);
    assert!(remote.rx.iter().any(|(_, p)| p[0] == nwk::CMD_REJOIN_RESPONSE && p[3] == 0));
    assert_eq!(net.nodes[n].stack.child_count(), 1);
    // it opens the network; a new device associates and gets the key from the bulb itself
    let mut permit = vec![0x00, 0x00, 0x36, 0x00, 0x00, 0x00, 0x00, 0x77, 0x01, 60, 1];
    permit[7] = 0x55;
    let h = remote.header(nwk::FRAME_DATA, 0x0001);
    remote.nwk_send(h, &permit);
    net.run(100);
    let mut lamp = Zed::new(&net.air, false);
    lamp.pan = pan;
    assert!(lamp.join(&mut net, 0x0001), "joined a network that has no Trust Center");
    assert_eq!(lamp.key, Some(NEW_KEY));
    assert_eq!(net.nodes[n].stack.child_count(), 2);
}

#[test]
fn a_network_without_a_trust_center_takes_no_network_key_update() {
    let mut net = Net::new();
    let mut i = Initiator::new(&mut net);
    let (n, _, pan) = on_a_touchlink_network(&mut net, &mut i);
    let bulb = net.nodes[n].short();
    // A node that has the network key plays Trust Center: NWK source 0x0000, and as the
    // key's source the address that stands for "no Trust Center", which is what the bulb has on record.
    let mut rogue = Zed::new(&net.air, false);
    rogue.pan = pan;
    rogue.parent = bulb;
    rogue.short = 0x0000;
    rogue.key = Some(NEW_KEY);
    net.air.borrow_mut().ports[rogue.id].pan = pan;
    let mut aps = vec![0x09, 0x33, 0x05, 0x01];
    aps.extend_from_slice(&[0x66; 16]);
    aps.push(1);
    aps.extend_from_slice(&[0; 8]);
    aps.extend_from_slice(&[0xFF; 8]);
    let h = rogue.header(nwk::FRAME_DATA, 0xFFFF);
    rogue.nwk_send(h, &aps);
    net.run(500);
    let h = rogue.header(nwk::FRAME_DATA, 0xFFFD);
    rogue.nwk_send(h, &[0x09, 0x34, 0x09, 1]);
    // the bulb goes on under the key it had
    let t = net.now() + 100;
    net.run(20_000);
    let sent = net.sent_by(net.nodes[n].id, t);
    let nwk_frames: Vec<&Vec<u8>> = sent.iter().filter(|f| f[0] & 7 == 1).collect();
    assert!(!nwk_frames.is_empty());
    assert!(nwk_frames.iter().all(|f| secured_with(f, &NEW_KEY)));
}

#[test]
fn a_build_that_was_given_the_master_key_offers_and_accepts_it() {
    // (a made-up key stands in for the real one, which this repository does not contain)
    let master: [u8; 16] = core::array::from_fn(|i| 0x70 + i as u8);
    let mut net = Net::new();
    net.identity = Identity { touchlink_master_key: Some(master), ..IDENTITY };
    net.coord.permit_join = false;
    let n = net.add_node();
    let ext = net.air.borrow().ports[net.nodes[n].id].ext;
    let mut i = Initiator::new(&mut net);
    let (response_id, scan_rsp) = scan_for(&mut net, &mut i);
    assert_eq!(&scan_rsp[7..9], &[0x11, 0x80], "key bitmask: development, master, certification");
    let mut request = join_request(4, response_id);
    let at = 4 + 8 + 1;
    request[at..at + 16].copy_from_slice(&encrypted_with(4, response_id, &master));
    i.send(&mut net, Some(ext), 0x12, &request);
    let rsp = i.received(&mut net);
    assert_eq!((rsp[0].0, rsp[0].1[4]), (0x13, 0x00), "success");
    net.run(3_000);
    assert_eq!((net.nodes[n].stack.state(), net.nodes[n].stack.pan_id()), (State::Up, 0x4321));
    // the key that came out is the right one: the bulb's frames in the new network are under it
    let sent = net.sent_by(net.nodes[n].id, net.now() - 2_500);
    assert!(sent.iter().any(|f| secured_with(f, &NEW_KEY)));
}

#[test]
fn a_bulb_looking_for_a_network_waits_on_the_initiators_channel_once_it_has_answered() {
    let mut net = Net::new();
    net.coord.permit_join = false;
    let n = net.add_node();
    let (port, ext) = (net.nodes[n].id, net.air.borrow().ports[net.nodes[n].id].ext);
    let mut i = Initiator::new(&mut net);
    let (response_id, _) = scan_for(&mut net, &mut i);
    // for the life of the transaction the bulb sends no beacon requests and stays put
    let t = net.now();
    net.run(6_000);
    assert_eq!(net.air.borrow().ports[port].channel, 11);
    assert!(net.sent_by(port, t).is_empty(), "no scanning meanwhile");
    // so one request, sent once and seconds later, finds it
    i.send(&mut net, Some(ext), 0x10, &start_request(response_id));
    let rsp = i.received(&mut net);
    assert_eq!((rsp[0].0, rsp[0].1[4]), (0x11, 0x00));
    // a transaction that leads nowhere ends, and the search goes on
    let mut net = Net::new();
    net.coord.permit_join = false;
    let n = net.add_node();
    let port = net.nodes[n].id;
    let mut i = Initiator::new(&mut net);
    scan_for(&mut net, &mut i);
    net.run(9_000);
    let t = net.now();
    net.run(6_000);
    assert!(net.sent_by(port, t).iter().any(|f| f[0] & 7 == 3), "beacon requests again");
}
