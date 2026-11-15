//! The Diagnostics cluster (ZCL 8 clause 3.15) and the object endpoints
//! (Analog Input, Binary Output, Analog Output; 3.14): what the counters
//! count as frames are received, sent, relayed, retried and lost, how they
//! survive restarts without per-frame flash writes, and the settings.

mod sim;

use std::collections::HashMap;

use sim::*;
use zigbee::hal::{ResetCause, TxStatus};
use zigbee::nwk;

const ON_OFF: u16 = 0x0006;
const DIAG: u16 = 0x0B05;
const MFR: u16 = 0x1141;
const ANALOG_INPUT: u16 = 0x000C;
const ANALOG_OUTPUT: u16 = 0x000D;
const BINARY_OUTPUT: u16 = 0x0010;

// standard attributes
const RESETS: u16 = 0x0000;
const PERSISTENT_WRITES: u16 = 0x0001;
const MAC_RX_BCAST: u16 = 0x0100;
const MAC_RX_UCAST: u16 = 0x0102;
const MAC_TX_UCAST: u16 = 0x0103;
const MAC_TX_UCAST_RETRY: u16 = 0x0104;
const MAC_TX_UCAST_FAIL: u16 = 0x0105;
const APS_RX_UCAST: u16 = 0x0108;
const APS_TX_UCAST_SUCCESS: u16 = 0x0109;
const ROUTE_DISC: u16 = 0x010C;
const NEIGHBOR_ADDED: u16 = 0x010D;
const NWK_FC_FAILURE: u16 = 0x0112;
const BUFFER_FAILURES: u16 = 0x0117;
const RELAYED_UCAST: u16 = 0x0118;
const AVERAGE_RETRIES: u16 = 0x011B;
// manufacturer's
const UPTIME: u16 = 0x5000;
const CAUSE: u16 = 0x5001;
const WATCHDOG_RESETS: u16 = 0x5002;
const NEIGHBORS: u16 = 0x5003;
const RELAY_QUEUE_FULL: u16 = 0x5005;
const RELAY_NO_ROUTE: u16 = 0x5006;
const RELAY_MAC_FAIL: u16 = 0x5007;
const RELAY_DUPLICATE: u16 = 0x5008;
const NWK_STATUS_SENT: u16 = 0x5009;
const BCAST_QUEUE_FULL: u16 = 0x500B;

/// Bytes a value of a ZCL type takes after the type in a record.
fn value_len(ty: u8, rest: &[u8]) -> usize {
    match ty {
        0x10 | 0x18 | 0x20 | 0x28 | 0x30 => 1,
        0x19 | 0x21 | 0x31 => 2,
        0x23 | 0x39 => 4,
        0x41 | 0x42 => 1 + rest[0] as usize,
        _ => panic!("type {ty:#x}"),
    }
}

/// Read Attributes records: id -> (type, value bytes); an unsupported one has no entry.
fn records(rsp: &[u8]) -> HashMap<u16, (u8, Vec<u8>)> {
    let mut out = HashMap::new();
    let mut i = 0;
    while i + 3 <= rsp.len() {
        let id = u16::from_le_bytes([rsp[i], rsp[i + 1]]);
        let status = rsp[i + 2];
        i += 3;
        if status != 0 {
            continue;
        }
        let ty = rsp[i];
        let n = value_len(ty, &rsp[i + 1..]);
        out.insert(id, (ty, rsp[i + 1..i + 1 + n].to_vec()));
        i += 1 + n;
    }
    out
}

fn number(v: &[u8]) -> u32 {
    v.iter().rev().fold(0u32, |a, b| a << 8 | *b as u32)
}

fn ids(list: &[u16]) -> Vec<u8> {
    list.iter().flat_map(|a| a.to_le_bytes()).collect()
}

/// Reads standard Diagnostics attributes.
fn diag(net: &mut Net, short: u16, attrs: &[u16]) -> HashMap<u16, (u8, Vec<u8>)> {
    let seq = net.coord.zcl(short, DIAG, 0x00, 0x00, &ids(attrs));
    net.run(60);
    let rsp = net.coord.last_zcl(short, DIAG, 0x01).unwrap();
    assert_eq!(&rsp[..2], &[0x18, seq]);
    records(&rsp[3..])
}

fn diag1(net: &mut Net, short: u16, attr: u16) -> u32 {
    number(&diag(net, short, &[attr])[&attr].1)
}

/// Reads manufacturer-specific Diagnostics attributes (our code).
fn mfr(net: &mut Net, short: u16, attrs: &[u16]) -> HashMap<u16, (u8, Vec<u8>)> {
    let seq = net.coord.zcl_mfr(short, DIAG, 0x00, MFR, 0x00, &ids(attrs));
    net.run(60);
    let rsp = net.coord.last_zcl_mfr(short, DIAG, 0x01).unwrap();
    // the response is manufacturer-specific too
    assert_eq!(&rsp[..4], &[0x1C, 0x41, 0x11, seq]);
    records(&rsp[5..])
}

fn mfr1(net: &mut Net, short: u16, attr: u16) -> u32 {
    number(&mfr(net, short, &[attr])[&attr].1)
}

fn read_ep(net: &mut Net, short: u16, ep: u8, cluster: u16, attrs: &[u16]) -> HashMap<u16, (u8, Vec<u8>)> {
    let seq = net.coord.zcl_ep(short, ep, cluster, 0x00, 0x00, &ids(attrs));
    net.run(60);
    let rsp = net.coord.last_zcl_ep(short, ep, cluster, 0x01).unwrap();
    assert_eq!(&rsp[..2], &[0x18, seq]);
    records(&rsp[3..])
}

fn single(v: &[u8]) -> f32 {
    f32::from_le_bytes([v[0], v[1], v[2], v[3]])
}

/// Discover Attributes (extended): (id, type, access) from `start`.
fn discover(net: &mut Net, short: u16, ep: u8, cluster: u16, mfr_code: bool, start: u16) -> (bool, Vec<(u16, u8, u8)>) {
    let s = start.to_le_bytes();
    let rsp = if mfr_code {
        net.coord.zcl_mfr(short, cluster, 0x00, MFR, 0x15, &[s[0], s[1], 20]);
        net.run(60);
        net.coord.last_zcl_mfr(short, cluster, 0x16).unwrap()[5..].to_vec()
    } else {
        net.coord.zcl_ep(short, ep, cluster, 0x00, 0x15, &[s[0], s[1], 20]);
        net.run(60);
        net.coord.last_zcl_ep(short, ep, cluster, 0x16).unwrap()[3..].to_vec()
    };
    let complete = rsp[0] == 1;
    (complete, rsp[1..].chunks(4).map(|c| (u16::from_le_bytes([c[0], c[1]]), c[2], c[3])).collect())
}

fn peer(net: &mut Net, n: usize, short: u16) -> Zed {
    let me = net.nodes[n].short();
    let mut z = Zed::router(&net.air, short, me);
    net.link(0, z.id, 0);
    z.link_status(&[(me, 1)]);
    net.run(50);
    z
}

fn data_through(z: &mut Zed, dst: u16, discover: u8) -> TxStatus {
    let mut h = z.header(nwk::FRAME_DATA, dst);
    h.discover = discover;
    let seq = h.seq;
    z.nwk_send(h, &[0x00, 0x01, 0x06, 0x00, 0x04, 0x01, 0x01, seq, 0x18, seq, 0x0A, 0x00, 0x00, 0x10, 0x01])
}

/// Moves the clock on by `ms` at once, then lets a few seconds run.
fn skip(net: &mut Net, ms: u32) {
    net.air.borrow_mut().now += ms;
    net.run(3_000);
}

fn bind(net: &mut Net, n: usize, ep: u8, cluster: u16) -> u8 {
    let short = net.nodes[n].short();
    let ext = net.air.borrow().ports[net.nodes[n].id].ext;
    let mut p = ext.to_le_bytes().to_vec();
    p.push(ep);
    p.extend_from_slice(&cluster.to_le_bytes());
    p.push(3);
    p.extend_from_slice(&COORD_EXT.to_le_bytes());
    p.push(1);
    net.coord.zdo(short, 0x0021, &p);
    net.run(50);
    net.coord.last_zdo(short, 0x8021).unwrap()[1]
}

#[test]
fn the_diagnostics_attributes_are_those_of_the_table_with_its_types() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    // Tables 3-130 and 3-131: every attribute, each with the type of the table, readable and reportable
    let (complete, first) = discover(&mut net, short, 1, DIAG, false, 0);
    assert!(!complete);
    let (complete, rest) = discover(&mut net, short, 1, DIAG, false, first.last().unwrap().0 + 1);
    assert!(complete);
    let all: Vec<(u16, u8)> = first.iter().chain(rest.iter()).map(|a| (a.0, a.1)).collect();
    let mut expected = vec![(0x0000, 0x21), (0x0001, 0x21), (0x0100, 0x23), (0x0101, 0x23), (0x0102, 0x23), (0x0103, 0x23)];
    expected.extend((0x0104..=0x011B).map(|id| (id, 0x21)));
    expected.extend([(0x011C, 0x20), (0x011D, 0x28), (0xFFFD, 0x21)]);
    assert_eq!(all, expected);
    for a in first.iter().chain(rest.iter()).filter(|a| a.0 != 0xFFFD) {
        assert_eq!(a.2, 0x05, "{:#06x}: readable, not writable, reportable", a.0);
    }
    // revision 3 of the cluster (3.15.1.1)
    assert_eq!(diag(&mut net, short, &[0xFFFD])[&0xFFFD], (0x21, vec![3, 0]));
    // the manufacturer's: under its code only
    let mut m = Vec::new();
    loop {
        let from = m.last().map(|a: &(u16, u8, u8)| a.0 + 1).unwrap_or(0);
        let (complete, part) = discover(&mut net, short, 1, DIAG, true, from);
        m.extend(part);
        if complete {
            break;
        }
    }
    let mut expected: Vec<u16> = (0x5000..=0x501B).collect();
    expected.extend((0x5020..=0x5026).chain(0x5030..=0x5037).chain(0x5040..=0x5044).chain(0x5048..=0x504C));
    expected.extend((0x5050..=0x5053).chain(0x5058..=0x505D));
    assert_eq!(m.iter().map(|a| a.0).collect::<Vec<_>>(), expected);
    // the numbers are reportable, the tables (octet strings) only readable
    for a in &m {
        assert_eq!(a.2, if a.1 == 0x41 { 0x01 } else { 0x05 }, "{:#06x}", a.0);
    }
    assert!(diag(&mut net, short, &[UPTIME]).is_empty(), "no 0x5000 without the manufacturer code");
    let seq = net.coord.zcl_mfr(short, DIAG, 0x00, 0x1234, 0x00, &ids(&[UPTIME]));
    net.run(60);
    assert_eq!(net.coord.last_zcl_mfr(short, DIAG, 0x0B).unwrap(), vec![0x1C, 0x34, 0x12, seq, 0x0B, 0x00, 0x81], "another code");
    // nothing is writable
    let seq = net.coord.zcl(short, DIAG, 0x00, 0x02, &[0x02, 0x01, 0x23, 0, 0, 0, 0]);
    net.run(60);
    assert_eq!(net.coord.last_zcl(short, DIAG, 0x04).unwrap(), vec![0x18, seq, 0x04, 0x88, 0x02, 0x01]);
}

#[test]
fn last_message_lqi_and_rssi_are_those_of_the_read_itself() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    for lqi in [255u8, 180, 60] {
        net.link(0, net.nodes[n].id, lqi);
        let r = diag(&mut net, short, &[0x011C, 0x011D]);
        assert_eq!(r[&0x011C], (0x20, vec![lqi]));
        assert_eq!(r[&0x011D], (0x28, vec![(lqi as i32 * 84 / 255 - 99) as i8 as u8]));
    }
}

#[test]
fn the_counters_follow_what_the_bulb_receives_and_sends() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let attrs = [MAC_RX_UCAST, MAC_TX_UCAST, APS_RX_UCAST, APS_TX_UCAST_SUCCESS, MAC_RX_BCAST];
    let before = diag(&mut net, short, &attrs);
    for _ in 0..5 {
        net.coord.zcl(short, ON_OFF, 0x01, 0x02, &[]);
        net.run(100);
    }
    let after = diag(&mut net, short, &attrs);
    let d = |a: u16| number(&after[&a].1) - number(&before[&a].1);
    // five commands and the second read; each asks for an APS acknowledgement,
    // and each is answered (the read, and a Default Response for each command)
    assert!(d(MAC_RX_UCAST) >= 6, "{}", d(MAC_RX_UCAST));
    assert_eq!(d(APS_RX_UCAST), 6);
    assert!(d(MAC_TX_UCAST) >= 12, "{}", d(MAC_TX_UCAST));
    assert_eq!(d(APS_TX_UCAST_SUCCESS), 12, "6 acknowledgements and 6 responses");
    // the coordinator's Link Status every 15 s
    net.run(31_000);
    assert!(diag1(&mut net, short, MAC_RX_BCAST) >= number(&after[&MAC_RX_BCAST].1) + 2);
}

#[test]
fn mac_retries_are_counted_per_transmission_and_per_aps_message() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let id = net.nodes[n].id;
    assert_eq!(diag1(&mut net, short, AVERAGE_RETRIES), 0);
    let r0 = diag1(&mut net, short, MAC_TX_UCAST_RETRY);
    // the next acknowledged unicast of the bulb goes out four times
    net.air.borrow_mut().ports[id].next_attempts = 4;
    diag1(&mut net, short, RESETS);
    assert_eq!(diag1(&mut net, short, MAC_TX_UCAST_RETRY), r0 + 3);
    // a lot of retries for one message moves the average
    net.air.borrow_mut().ports[id].next_attempts = 200;
    diag1(&mut net, short, RESETS);
    let avg = diag1(&mut net, short, AVERAGE_RETRIES);
    assert!(avg >= 1, "{avg}");
    // the Analog Input of endpoint 3 shows the same average with two decimals
    // (the reads in between are APS messages too, sent the first time: the average drops a little)
    let pv = single(&read_ep(&mut net, short, 3, ANALOG_INPUT, &[0x0055])[&0x0055].1);
    assert!(pv <= avg as f32 + 0.5 && pv >= avg as f32 * 0.8, "{pv} vs {avg}");
    assert_eq!(pv, (pv * 100.0).round() / 100.0, "hundredths");
}

#[test]
fn relayed_frames_are_counted_and_so_are_those_lost_by_cause() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let mut x = peer(&mut net, n, 0x2B02);
    let y = peer(&mut net, n, 0x3C03);
    net.link(x.id, y.id, 0);
    let all = [RELAY_QUEUE_FULL, RELAY_NO_ROUTE, RELAY_MAC_FAIL, RELAY_DUPLICATE, NWK_STATUS_SENT, NEIGHBORS];
    let m0 = mfr(&mut net, short, &all);
    let m = |net: &mut Net, a: u16| mfr1(net, short, a) - number(&m0[&a].1);
    assert_eq!(number(&m0[&NEIGHBORS].1), 3, "the coordinator, x and y");
    let s0 = diag(&mut net, short, &[RELAYED_UCAST, MAC_TX_UCAST_FAIL, MAC_TX_UCAST_RETRY, NWK_FC_FAILURE, ROUTE_DISC, BUFFER_FAILURES]);
    let s = |net: &mut Net, a: u16| diag1(net, short, a) - number(&s0[&a].1);
    // a frame passed on
    data_through(&mut x, y.short, 0);
    net.run(100);
    assert_eq!(s(&mut net, RELAYED_UCAST), 1);
    // a copy of it (the same secured frame again) is a replay, dropped
    let copy = net.air.borrow().log.iter().rev().find(|e| e.1 == x.id).unwrap().2.clone();
    net.air.borrow_mut().transmit(x.id, &copy);
    net.run(100);
    assert_eq!(s(&mut net, NWK_FC_FAILURE), 1);
    assert_eq!(m(&mut net, RELAY_DUPLICATE), 1);
    assert_eq!(s(&mut net, RELAYED_UCAST), 1);
    // y is gone: three attempts of four transmissions each, then a link failure for x
    net.air.borrow_mut().ports[y.id].powered = false;
    data_through(&mut x, y.short, 0);
    net.run(1_000);
    assert_eq!(m(&mut net, RELAY_MAC_FAIL), 1);
    assert_eq!(s(&mut net, MAC_TX_UCAST_FAIL), 3);
    assert_eq!(s(&mut net, MAC_TX_UCAST_RETRY), 9);
    assert_eq!(m(&mut net, NWK_STATUS_SENT), 1);
    // no route, none may be looked for
    data_through(&mut x, 0x4D4D, 0);
    net.run(500);
    assert_eq!(m(&mut net, RELAY_NO_ROUTE), 1);
    assert_eq!(m(&mut net, NWK_STATUS_SENT), 2);
    // one may be looked for: a discovery, and when it finds nothing, lost for want of a route
    data_through(&mut x, 0x4D4E, 1);
    net.run(7_000);
    assert_eq!(s(&mut net, ROUTE_DISC), 1);
    assert_eq!(m(&mut net, RELAY_NO_ROUTE), 2);
    // more frames waiting for routes than there is room for: the oldest make room
    for k in 0..14u16 {
        data_through(&mut x, 0x4E00 + k, 1);
        net.run(30);
    }
    net.run(8_000);
    assert_eq!(m(&mut net, RELAY_QUEUE_FULL), 4);
    // the four, and the route requests that found the broadcast queue full (sent once, not repeated)
    let bcast_full = mfr1(&mut net, short, BCAST_QUEUE_FULL);
    assert!(bcast_full > 0);
    assert_eq!(s(&mut net, BUFFER_FAILURES), 4 + bcast_full);
    assert_eq!(m(&mut net, RELAY_NO_ROUTE), 12);
    assert_eq!(s(&mut net, RELAYED_UCAST), 1);
}

#[test]
fn no_frame_costs_a_flash_write_and_the_counters_are_stored_once_an_hour() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let id = net.nodes[n].id;
    net.run(5_000);
    let w0 = net.air.borrow().ports[id].nv_writes;
    let p0 = diag1(&mut net, short, PERSISTENT_WRITES);
    for _ in 0..100 {
        net.coord.zcl(short, ON_OFF, 0x00, 0x00, &[0x00, 0x00]);
        net.run(50);
    }
    assert_eq!(net.air.borrow().ports[id].nv_writes, w0);
    assert_eq!(diag1(&mut net, short, PERSISTENT_WRITES), p0);
    assert!(!net.air.borrow().ports[id].nv.contains_key(&15), "no snapshot yet");
    // the first snapshot half an hour after the start: one record; then one an hour
    let rx = diag1(&mut net, short, MAC_RX_UCAST);
    skip(&mut net, 1_800_000);
    assert_eq!(net.air.borrow().ports[id].nv_writes, w0 + 1);
    assert!(net.air.borrow().ports[id].nv.contains_key(&15));
    assert_eq!(diag1(&mut net, short, PERSISTENT_WRITES), p0 + 1);
    skip(&mut net, 1_800_000);
    assert_eq!(net.air.borrow().ports[id].nv_writes, w0 + 1, "not before the hour is over");
    skip(&mut net, 1_800_000);
    assert_eq!(net.air.borrow().ports[id].nv_writes, w0 + 2);
    // after a power cut the counters are back as stored
    net.power_cycle(n);
    net.run(3_000);
    let after = diag1(&mut net, short, MAC_RX_UCAST);
    assert!(after > rx && after < rx + 20, "{rx} -> {after}");
}

#[test]
fn every_start_is_counted_exactly_and_a_warm_one_keeps_every_counter() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let id = net.nodes[n].id;
    assert_eq!(diag1(&mut net, short, RESETS), 1);
    assert_eq!(mfr1(&mut net, short, CAUSE), 0, "power-on");
    // power-on starts: one write each, as before the counter (it goes with the frame counters)
    for k in 2..=4 {
        let w = net.air.borrow().ports[id].nv_writes;
        net.power_cycle(n);
        assert_eq!(net.air.borrow().ports[id].nv_writes, w + 1);
        net.run(3_000);
        assert_eq!(diag1(&mut net, short, RESETS), k);
    }
    assert_eq!(mfr1(&mut net, short, WATCHDOG_RESETS), 0);
    // traffic, then a watchdog reset: what the RAM kept comes back, and is stored once
    for _ in 0..10 {
        net.coord.zcl(short, ON_OFF, 0x00, 0x00, &[0x00, 0x00]);
        net.run(50);
    }
    net.run(1_100);
    let rx = diag1(&mut net, short, MAC_RX_UCAST);
    let w = net.air.borrow().ports[id].nv_writes;
    net.air.borrow_mut().ports[id].reset_cause = ResetCause::Watchdog;
    net.power_cycle(n);
    assert_eq!(net.air.borrow().ports[id].nv_writes, w + 2, "the start, and the counters the RAM kept");
    net.run(3_000);
    let m = mfr(&mut net, short, &[CAUSE, WATCHDOG_RESETS, UPTIME]);
    assert_eq!(number(&m[&CAUSE].1), 1);
    assert_eq!(number(&m[&WATCHDOG_RESETS].1), 1);
    assert!(number(&m[&UPTIME].1) <= 4);
    assert_eq!(diag1(&mut net, short, RESETS), 5);
    assert!(diag1(&mut net, short, MAC_RX_UCAST) > rx);
    // a power cut after it: the counters stored at that start are still there
    net.air.borrow_mut().ports[id].reset_cause = ResetCause::PowerOn;
    net.power_cycle(n);
    net.run(3_000);
    // (what the RAM kept was taken up to a second before the reset: the last read is not in it)
    assert!(diag1(&mut net, short, MAC_RX_UCAST) >= rx);
    assert_eq!(mfr1(&mut net, short, WATCHDOG_RESETS), 1);
}

#[test]
fn the_manufacturers_command_clears_every_counter() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    net.power_cycle(n);
    net.run(3_000);
    assert!(diag1(&mut net, short, MAC_RX_UCAST) > 0);
    let seq = net.coord.zcl_mfr(short, DIAG, 0x01, MFR, 0x00, &[]);
    net.run(100);
    assert_eq!(net.coord.last_zcl_mfr(short, DIAG, 0x0B).unwrap(), vec![0x1C, 0x41, 0x11, seq, 0x0B, 0x00, 0x00]);
    assert_eq!(diag1(&mut net, short, RESETS), 0);
    // (the read itself counts)
    assert!(diag1(&mut net, short, MAC_RX_UCAST) <= 3);
    // without our code the cluster has no commands (3.15.2.3)
    let seq = net.coord.zcl(short, DIAG, 0x01, 0x00, &[]);
    net.run(100);
    assert_eq!(net.coord.last_zcl(short, DIAG, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x00, 0x81]);
    // cleared counters stay cleared over a power cut
    net.power_cycle(n);
    net.run(3_000);
    assert_eq!(diag1(&mut net, short, RESETS), 1);
}

#[test]
fn diagnostic_attributes_are_reported_and_the_configuration_is_kept() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    assert_eq!(bind(&mut net, n, 1, DIAG), 0x00);
    // MacRxUcast (uint32): at least 1 s apart, on a change of 5
    let seq = net.coord.zcl(short, DIAG, 0x00, 0x06, &[0x00, 0x02, 0x01, 0x23, 1, 0, 0, 0, 5, 0, 0, 0]);
    net.run(60);
    assert_eq!(net.coord.last_zcl(short, DIAG, 0x07).unwrap(), vec![0x18, seq, 0x07, 0x00]);
    // the manufacturer's uptime every 10 s
    let seq = net.coord.zcl_mfr(short, DIAG, 0x00, MFR, 0x06, &[0x00, 0x00, 0x50, 0x23, 0, 0, 10, 0, 0xFF, 0xFF, 0xFF, 0xFF]);
    net.run(60);
    assert_eq!(net.coord.last_zcl_mfr(short, DIAG, 0x07).unwrap(), vec![0x1C, 0x41, 0x11, seq, 0x07, 0x00]);
    let reports = |net: &Net, mfr: bool| {
        net.coord.zcl_rx.iter().filter(|e| e.1 == DIAG && (e.2[0] & 0x04 != 0) == mfr && e.2[if mfr { 4 } else { 2 }] == 0x0A).count()
    };
    let r0 = reports(&net, false);
    for _ in 0..6 {
        net.coord.zcl(short, ON_OFF, 0x00, 0x00, &[0x00, 0x00]);
        net.run(300);
    }
    net.run(1_100);
    assert!(reports(&net, false) > r0);
    let last = net.coord.zcl_rx.iter().rev().find(|e| e.1 == DIAG && e.2[2] == 0x0A).unwrap().2.clone();
    assert_eq!(&last[3..6], &[0x02, 0x01, 0x23]);
    net.run(11_000);
    assert!(reports(&net, true) >= 1);
    let last = net.coord.zcl_rx.iter().rev().find(|e| e.1 == DIAG && e.2[0] == 0x1C && e.2[4] == 0x0A).unwrap().2.clone();
    assert_eq!(&last[..3], &[0x1C, 0x41, 0x11]);
    assert_eq!(&last[5..8], &[0x00, 0x50, 0x23]);
    // the configuration reads back, also after a power cut
    net.power_cycle(n);
    net.run(3_000);
    let seq = net.coord.zcl(short, DIAG, 0x00, 0x08, &[0x00, 0x02, 0x01]);
    net.run(60);
    assert_eq!(net.coord.last_zcl(short, DIAG, 0x09).unwrap(), vec![0x18, seq, 0x09, 0x00, 0x00, 0x02, 0x01, 0x23, 1, 0, 0, 0, 5, 0, 0, 0]);
    // off again
    net.coord.zcl(short, DIAG, 0x00, 0x06, &[0x00, 0x02, 0x01, 0x23, 0, 0, 0xFF, 0xFF, 0, 0, 0, 0]);
    net.run(60);
    let seq = net.coord.zcl(short, DIAG, 0x00, 0x08, &[0x00, 0x02, 0x01]);
    net.run(60);
    assert_eq!(net.coord.last_zcl(short, DIAG, 0x09).unwrap(), vec![0x18, seq, 0x09, 0x00, 0x00, 0x02, 0x01, 0x23, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0]);
}

#[test]
fn the_object_endpoints_describe_themselves() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let r = read_ep(&mut net, short, 2, ANALOG_INPUT, &[0x001C, 0x0041, 0x0045, 0x0051, 0x0055, 0x006A, 0x006F, 0x0075, 0x0100, 0xFFFD]);
    assert_eq!(r[&0x001C].1[1..], *b"Relayed frames lost");
    assert_eq!(single(&r[&0x0045].1), 0.0);
    assert!(single(&r[&0x0041].1) >= 65535.0);
    assert_eq!(single(&r[&0x006A].1), 1.0);
    assert_eq!(r[&0x0051], (0x10, vec![0]));
    assert_eq!(r[&0x006F], (0x18, vec![0]));
    assert_eq!(r[&0x0055].0, 0x39);
    assert!(!r.contains_key(&0x0075), "a count has no unit");
    assert_eq!(number(&r[&0x0100].1), 0x000C_0000, "Count");
    assert_eq!(r[&0xFFFD], (0x21, vec![1, 0]));
    let r = read_ep(&mut net, short, 4, ANALOG_INPUT, &[0x001C, 0x0075, 0x0100, 0x0055]);
    assert_eq!(r[&0x001C].1[1..], *b"Radio time lent to BLE");
    assert_eq!(r[&0x0075], (0x31, vec![98, 0]), "percent");
    assert_eq!(number(&r[&0x0100].1), 0x0004_0000, "Percentage");
    assert_eq!(single(&r[&0x0055].1), 0.0, "no BLE in the simulation");
    let r = read_ep(&mut net, short, 5, ANALOG_INPUT, &[0x0055]);
    assert_eq!(single(&r[&0x0055].1), 1.0, "the coordinator");
    let r = read_ep(&mut net, short, 7, BINARY_OUTPUT, &[0x001C, 0x0055, 0x0051, 0x006F]);
    assert_eq!(r[&0x001C].1[1..], *b"Allow end devices to join");
    assert_eq!(r[&0x0055], (0x10, vec![1]));
    let r = read_ep(&mut net, short, 8, ANALOG_OUTPUT, &[0x001C, 0x0055, 0x0041, 0x0075]);
    assert_eq!(r[&0x001C].1[1..], *b"BLE window after power-on");
    assert_eq!(single(&r[&0x0055].1), 0.0);
    assert_eq!(single(&r[&0x0041].1), 1440.0);
    assert_eq!(r[&0x0075], (0x31, vec![72, 0]), "minutes");
    // the endpoints have no other clusters, and no cluster commands
    let seq = net.coord.zcl_ep(short, 2, ON_OFF, 0x01, 0x00, &[]);
    net.run(60);
    assert_eq!(net.coord.last_zcl_ep(short, 2, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x00, 0xC3]);
    // the inputs are read-only
    let seq = net.coord.zcl_ep(short, 2, ANALOG_INPUT, 0x00, 0x02, &[0x55, 0x00, 0x39, 0, 0, 0, 0]);
    net.run(60);
    assert_eq!(net.coord.last_zcl_ep(short, 2, ANALOG_INPUT, 0x04).unwrap(), vec![0x18, seq, 0x04, 0x88, 0x55, 0x00]);
}

fn beacon(net: &mut Net, n: usize) -> zigbee::mac::Beacon {
    let id = net.nodes[n].id;
    let t = net.now();
    let scanner = net.air.borrow_mut().add_port(0x1111);
    net.air.borrow_mut().transmit(scanner, &[0x03, 0x08, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0x07]);
    net.run(60);
    let f = net.sent_by(id, t).into_iter().find(|f| f[0] & 7 == 0).unwrap();
    let (h, off) = zigbee::mac::Header::parse(&f).unwrap();
    zigbee::mac::Beacon::parse(&h, &f[off..]).unwrap()
}

fn write_ep(net: &mut Net, short: u16, ep: u8, cluster: u16, ty: u8, value: &[u8]) -> Vec<u8> {
    let mut p = vec![0x55, 0x00, ty];
    p.extend_from_slice(value);
    net.coord.zcl_ep(short, ep, cluster, 0x00, 0x02, &p);
    net.run(60);
    net.coord.last_zcl_ep(short, ep, cluster, 0x04).unwrap()[3..].to_vec()
}

#[test]
fn with_end_devices_not_allowed_none_joins_and_those_there_stay() {
    let mut net = Net::new();
    let a = net.join_node();
    let sa = net.nodes[a].short();
    net.coord.permit(250);
    net.run(600);
    let mut first = Zed::new(&net.air, true);
    net.link(0, first.id, 0);
    assert!(first.join(&mut net, sa));
    net.run(200);
    assert!(beacon(&mut net, a).end_device_capacity);
    assert_eq!(write_ep(&mut net, sa, 7, BINARY_OUTPUT, 0x10, &[0]), vec![0x00]);
    assert!(!net.nodes[a].stack.allows_children());
    let b = beacon(&mut net, a);
    assert!(!b.end_device_capacity && b.router_capacity);
    // a new end device is turned away: PAN at capacity
    let mut second = Zed::new(&net.air, true);
    net.link(0, second.id, 0);
    assert!(!second.join(&mut net, sa));
    assert_eq!(second.mac_rx.iter().filter(|(_, p)| p[0] == 0x02).map(|(_, p)| p[3]).next(), Some(0x01));
    assert_eq!(net.nodes[a].stack.child_count(), 1, "the child that was there stays");
    // the setting outlives a power cut; the generated value is checked
    net.power_cycle(a);
    net.run(3_000);
    assert!(!net.nodes[a].stack.allows_children());
    assert_eq!(write_ep(&mut net, sa, 7, BINARY_OUTPUT, 0x10, &[2]), vec![0x87, 0x55, 0x00]);
    assert_eq!(write_ep(&mut net, sa, 7, BINARY_OUTPUT, 0x10, &[1]), vec![0x00]);
    assert!(beacon(&mut net, a).end_device_capacity);
}

#[test]
fn the_ble_window_takes_whole_minutes_in_range_and_is_kept() {
    use zigbee::stack::BleMode;
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    // endpoint 8 is the BLE setting of the settings cluster 0xFC5F seen as a window: 0, no limit
    assert_eq!(net.nodes[n].stack.ble_mode(), BleMode::On);
    let r = read_ep(&mut net, short, 8, ANALOG_OUTPUT, &[0x0055]);
    assert_eq!(single(&r[&0x0055].1), 0.0);
    assert_eq!(write_ep(&mut net, short, 8, ANALOG_OUTPUT, 0x39, &30.0f32.to_le_bytes()), vec![0x00]);
    assert_eq!((net.nodes[n].stack.ble_mode(), net.nodes[n].stack.ble_window_minutes()), (BleMode::Window, 30));
    for bad in [30.5f32, -1.0, 1441.0, f32::NAN] {
        assert_eq!(write_ep(&mut net, short, 8, ANALOG_OUTPUT, 0x39, &bad.to_le_bytes()), vec![0x87, 0x55, 0x00], "{bad}");
    }
    assert_eq!(write_ep(&mut net, short, 8, ANALOG_OUTPUT, 0x21, &[1, 0]), vec![0x8D, 0x55, 0x00], "wrong type");
    net.power_cycle(n);
    net.run(3_000);
    assert_eq!((net.nodes[n].stack.ble_mode(), net.nodes[n].stack.ble_window_minutes()), (BleMode::Window, 30));
    let r = read_ep(&mut net, short, 8, ANALOG_OUTPUT, &[0x0055]);
    assert_eq!(single(&r[&0x0055].1), 30.0);
    // 0: no limit again
    assert_eq!(write_ep(&mut net, short, 8, ANALOG_OUTPUT, 0x39, &0.0f32.to_le_bytes()), vec![0x00]);
    assert_eq!(net.nodes[n].stack.ble_mode(), BleMode::On);
    assert_eq!(write_ep(&mut net, short, 8, ANALOG_OUTPUT, 0x39, &30.0f32.to_le_bytes()), vec![0x00]);
    // leaving the network takes it back to the default
    net.coord.zdo(short, 0x0034, &[0; 9]);
    net.run(2_000);
    assert_eq!(net.nodes[n].stack.ble_mode(), BleMode::On);
}

#[test]
fn an_analog_input_is_reported_through_the_binding_of_its_endpoint() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    assert_eq!(bind(&mut net, n, 5, ANALOG_INPUT), 0x00);
    // (endpoint 6 has a binding of the same cluster: it is not endpoint 5's)
    assert_eq!(bind(&mut net, n, 6, ANALOG_INPUT), 0x00);
    assert_eq!(bind(&mut net, n, 9, ANALOG_INPUT), 0x82, "no such endpoint");
    // PresentValue on every change of 1, as Zigbee2MQTT configures it
    let seq = net.coord.zcl_ep(short, 5, ANALOG_INPUT, 0x00, 0x06, &[0x00, 0x55, 0x00, 0x39, 0, 0, 0xE8, 0xFD, 0, 0, 0x80, 0x3F]);
    net.run(60);
    assert_eq!(net.coord.last_zcl_ep(short, 5, ANALOG_INPUT, 0x07).unwrap(), vec![0x18, seq, 0x07, 0x00]);
    // StatusFlags is reportable too (3.14.2.4.2.2); Description is not
    let seq = net.coord.zcl_ep(short, 5, ANALOG_INPUT, 0x00, 0x06, &[0x00, 0x1C, 0x00, 0x42, 0, 0, 10, 0]);
    net.run(60);
    assert_eq!(net.coord.last_zcl_ep(short, 5, ANALOG_INPUT, 0x07).unwrap(), vec![0x18, seq, 0x07, 0x8C, 0x00, 0x1C, 0x00]);
    // a second router neighbour: 2.0 from endpoint 5, to the light's binding nothing
    let reports = |net: &Net| {
        net.coord.zcl_rx.iter().zip(net.coord.zcl_rx_ep.iter()).filter(|((_, c, f), e)| *c == ANALOG_INPUT && **e == 5 && f.get(2) == Some(&0x0A)).count()
    };
    let r0 = reports(&net);
    let _x = peer(&mut net, n, 0x2B02);
    net.run(1_500);
    assert_eq!(reports(&net), r0 + 1, "one report, through the one binding of endpoint 5");
    let rep = net.coord.last_zcl_ep(short, 5, ANALOG_INPUT, 0x0A).expect("a report");
    assert_eq!(&rep[3..6], &[0x55, 0x00, 0x39]);
    assert_eq!(single(&rep[6..10]), 2.0);
    assert!(net.coord.last_zcl_ep(short, 1, ANALOG_INPUT, 0x0A).is_none());
}

#[test]
fn the_network_tables_and_the_event_log_can_be_read() {
    let mut net = Net::new();
    let a = net.join_node();
    let b = net.join_node();
    // past the first noise measurement, a minute after the start
    net.run(65_000);
    let short = net.nodes[a].short();
    let other = net.nodes[b].short();
    // network parameters as the network has them
    let p = mfr(&mut net, short, &[0x5030, 0x5031, 0x5032, 0x5034, 0x5036, 0x5037]);
    let channel = net.air.borrow().ports[net.nodes[a].id].channel;
    assert_eq!(number(&p[&0x5030].1), channel as u32);
    assert_eq!(p[&0x5032].0, 0x41);
    assert_eq!(p[&0x5032].1.len(), 9, "an 8-byte extended PAN identifier");
    // the noise floor, measured each minute
    let noise = mfr(&mut net, short, &[0x501A, 0x501B]);
    assert_eq!(noise[&0x501A].1, vec![(-97i8) as u8]);
    // the way to the coordinator: straight there
    let r = mfr(&mut net, short, &[0x5020, 0x5021, 0x5022]);
    assert_eq!(number(&r[&0x5020].1), 0x0000, "first hop to the coordinator");
    assert!(number(&r[&0x5021].1) >= 1 && number(&r[&0x5021].1) <= 7, "a link cost");
    // received frames counted by signal strength
    let h = mfr(&mut net, short, &[0x5015, 0x5016, 0x5017, 0x5018, 0x5019]);
    let heard: u32 = h.values().map(|v| number(&v.1)).sum();
    assert!(heard > 0, "frames in the RSSI histogram");
    // the neighbor table: the coordinator and the other bulb, with what was heard of them
    let mut entries = Vec::new();
    for page in 0x5050..=0x5053u16 {
        let t = mfr(&mut net, short, &[page]);
        let (ty, v) = &t[&page];
        assert_eq!(*ty, 0x41);
        assert_eq!(v[0] as usize, 4 * 15, "four entries of 15 bytes a page");
        for e in v[1..].chunks(15) {
            if e[5] & 1 != 0 {
                entries.push((u16::from_le_bytes([e[0], e[1]]), e[2], u16::from_le_bytes([e[7], e[8]]), e[11]));
            }
        }
    }
    let coord = entries.iter().find(|e| e.0 == 0x0000).expect("the coordinator in the neighbor table");
    assert!(coord.1 > 0 && coord.2 > 0, "LQI and frames heard: {coord:?}");
    assert!(entries.iter().any(|e| e.0 == other), "the other bulb in the neighbor table: {entries:?}");
    // the event log, newest first: the start and the joining are in it
    let log = mfr(&mut net, short, &[0x5058]);
    let (ty, v) = &log[&0x5058];
    assert_eq!(*ty, 0x41);
    let mut kinds = Vec::new();
    let mut seqs = Vec::new();
    for page in 0x5058..=0x505Du16 {
        let l = mfr(&mut net, short, &[page]);
        let v = &l[&page].1;
        assert_eq!(v[0] as usize % 12, 0);
        kinds.extend(v[1..].chunks(12).map(|e| e[3]));
        seqs.extend(v[1..].chunks(12).map(|e| u16::from_le_bytes([e[0], e[1]])));
    }
    // numbered one after the other, newest first
    assert!(seqs.windows(2).all(|w| w[0] == w[1] + 1), "{seqs:?}");
    assert!(v[0] > 0);
    assert!(kinds.contains(&1), "start: {kinds:?}");
    assert!(kinds.contains(&2), "on the network: {kinds:?}");
    assert!(kinds.contains(&5), "a neighbor added: {kinds:?}");
    // the tables are not reportable
    let seq = net.coord.zcl_mfr(short, DIAG, 0x00, MFR, 0x06, &[0x00, 0x50, 0x50, 0x41, 0x00, 0x00, 0x10, 0x0e]);
    net.run(60);
    let rsp = net.coord.last_zcl_mfr(short, DIAG, 0x07).unwrap();
    assert_eq!(rsp[3], seq);
    assert_eq!(rsp[5], 0x8C, "UNREPORTABLE_ATTRIBUTE");
}

#[test]
fn the_link_cost_comes_from_the_measured_delivery_once_there_is_enough() {
    // R22 3.6.3.1: the cost reflects the probability of delivery p,
    // C = min(7, round(1/p^4)); the first estimate from the average LQI.
    let mut net = Net::new();
    let a = net.join_node();
    let b = net.join_node();
    net.run(20_000);
    let (ida, idb) = (net.nodes[a].id, net.nodes[b].id);
    let (sa, sb) = (net.nodes[a].short(), net.nodes[b].short());
    // a: a strong signal (cost 1 by the LQI) that loses a third of the frames each way
    net.link(0, ida, 230);
    net.lossy(0, ida, 35);
    // b: a middling signal (cost 3 by the LQI) that loses nothing
    net.link(0, idb, 80);
    net.link(ida, idb, 0);
    // the running average of the LQI follows the new signal
    net.run(60_000);
    let entry = |net: &mut Net, short: u16, of: u16| -> Option<(u8, bool, u8, u8)> {
        for page in 0x5050..=0x5053u16 {
            let seq = net.coord.zcl_mfr(short, DIAG, 0x00, MFR, 0x00, &ids(&[page]));
            net.run(200);
            let Some(rsp) = net.coord.last_zcl_mfr(short, DIAG, 0x01) else { continue };
            if rsp[3] != seq {
                continue;
            }
            let r = records(&rsp[5..]);
            let Some((_, v)) = r.get(&page) else { continue };
            for e in v[1..].chunks(15) {
                if e[5] & 1 != 0 && u16::from_le_bytes([e[0], e[1]]) == of {
                    return Some(((e[5] >> 4) & 7, e[5] & 0x80 != 0, e[14], e[2]));
                }
            }
        }
        None
    };
    // before any traffic of note: the LQI estimate
    let (cost_b, measured_b, _, lqi_b) = entry(&mut net, sb, 0x0000).expect("the coordinator in b's table");
    assert!(!measured_b);
    assert!(cost_b == nwk::cost_from_lqi(lqi_b) && cost_b > 1, "by the LQI ({lqi_b}): cost {cost_b}");
    // traffic to the coordinator: answers to reads
    for _ in 0..40 {
        net.coord.zcl(sa, DIAG, 0x00, 0x00, &ids(&[RESETS]));
        net.coord.zcl(sb, DIAG, 0x00, 0x00, &ids(&[RESETS]));
        net.run(300);
    }
    net.lossy(0, ida, 0);
    let (cost_a, measured_a, p_a, _) = entry(&mut net, sa, 0x0000).expect("the coordinator in a's table");
    assert!(measured_a, "enough attempts to measure");
    assert!(p_a < 160 && cost_a == 7, "a third lost each way: p {p_a}/255, cost {cost_a}");
    let (cost_b, measured_b, p_b, _) = entry(&mut net, sb, 0x0000).unwrap();
    assert!(measured_b && p_b >= 231 && cost_b == 1, "nothing lost: p {p_b}/255, cost {cost_b}");
    // and that is the cost of the way to the coordinator
    assert_eq!(mfr1(&mut net, sb, 0x5021), 1);
}

/// The neighbors that went stale according to the event log.
fn stale_events(net: &mut Net, short: u16) -> Vec<u16> {
    let mut out = Vec::new();
    for page in 0x5058..=0x505Du16 {
        let l = mfr(net, short, &[page]);
        for e in l[&page].1[1..].chunks(12) {
            if e[3] == 6 {
                out.push(u16::from_le_bytes([e[8], e[9]]));
            }
        }
    }
    out
}

#[test]
fn a_neighbor_never_confirmed_by_a_link_status_does_not_go_stale() {
    // A router heard by a frame of another kind is a neighbor on probation:
    // usable at once, dropped from use at the next aging step unless its own
    // Link Status comes. It never was a two-way neighbor, so its lapse is
    // not a neighbor going stale (counter 0x010F, event 6): on a bulb every
    // start counted five of them, the best link among them.
    let mut net = Net::new();
    let a = net.join_node();
    net.run(20_000);
    let short = net.nodes[a].short();
    let before = diag1(&mut net, short, 0x010F);
    let me = net.nodes[a].short();
    let mut z = Zed::router(&net.air, 0x3C01, me);
    net.link(0, z.id, 0);
    data_through(&mut z, 0x0000, 0);
    net.run(70_000);
    assert_eq!(diag1(&mut net, short, 0x010F), before, "nothing went stale");
    assert!(!stale_events(&mut net, short).contains(&0x3C01));
    // one confirmed by its Link Status, then silent: that one goes stale
    let _y = peer(&mut net, a, 0x3C02);
    net.run(70_000);
    assert_eq!(diag1(&mut net, short, 0x010F), before + 1, "the confirmed one went stale");
    let ev = stale_events(&mut net, short);
    assert!(ev.contains(&0x3C02) && !ev.contains(&0x3C01), "{ev:?}");
}

/// The black box as one blob, read page by page (manufacturer attributes
/// from 0x6000; page 0 first).
fn black_box(net: &mut Net, short: u16) -> Vec<u8> {
    let mut blob = Vec::new();
    let mut page = 0u16;
    loop {
        let id = 0x6000 + page;
        let r = mfr(net, short, &[id]);
        let (ty, v) = &r[&id];
        assert_eq!(*ty, 0x41);
        if v[0] == 0 {
            break;
        }
        blob.extend_from_slice(&v[1..]);
        let total = u16::from_le_bytes([blob[20], blob[21]]) as usize;
        if blob.len() >= total {
            break;
        }
        page += 1;
    }
    blob
}

#[test]
fn the_black_box_keeps_the_unicasts_per_neighbor_the_frames_sent_and_rows() {
    let mut net = Net::new();
    let a = net.join_node();
    net.run(20_000);
    let short = net.nodes[a].short();
    let id = net.nodes[a].id;
    // a link to the coordinator that loses a third of the attempts, for traffic to it
    net.lossy(0, id, 33);
    for _ in 0..30 {
        net.coord.zcl(short, DIAG, 0x00, 0x00, &ids(&[RESETS]));
        net.run(500);
    }
    net.lossy(0, id, 0);
    // past five rows of two minutes
    net.run(10 * 60_000);
    let b = black_box(&mut net, short);
    // BLACK_BOX_OUT=file keeps the blob, for `outglow box --from`
    if let Ok(path) = std::env::var("BLACK_BOX_OUT") {
        std::fs::write(path, &b).unwrap();
    }
    let total = u16::from_le_bytes([b[20], b[21]]) as usize;
    assert_eq!(b.len(), total, "the whole blob");
    assert_eq!(b[0], 1, "version");
    let (neighbors, link_entry, ring_cap, ring_len, ring_entry) = (b[1] as usize, b[2] as usize, b[3] as usize, b[4] as usize, b[5] as usize);
    let rows_cap = u16::from_le_bytes([b[6], b[7]]) as usize;
    let rows_len = u16::from_le_bytes([b[8], b[9]]) as usize;
    let row_entry = b[10] as usize;
    assert_eq!(b[11] as u32 * 10, 120, "a row every two minutes");
    // per neighbor: the coordinator's slot, its unicasts oldest first, with losses among them
    let links = &b[24..24 + neighbors * link_entry];
    let coord = links.chunks(link_entry).find(|e| u16::from_le_bytes([e[0], e[1]]) == 0x0000).expect("the coordinator's slot");
    let len = coord[2] as usize;
    assert!(len >= 30, "unicasts kept: {len}");
    let frames: Vec<u8> = (0..len).map(|i| (coord[4 + i / 2] >> (4 * (i % 2))) & 0x0F).collect();
    assert!(frames.iter().all(|f| (1..=4).contains(&(f & 7))), "attempts 1..4: {frames:?}");
    assert!(frames.iter().any(|f| f & 7 > 1), "retries among them: {frames:?}");
    assert!(frames.iter().filter(|f| *f & 8 != 0).count() * 10 > len * 8, "mostly delivered: {frames:?}");
    // the frames sent, oldest first: answers to the coordinator, cluster 0x0B05, and Link Status commands
    let ring_at = 24 + neighbors * link_entry;
    let ring: Vec<&[u8]> = b[ring_at..ring_at + ring_len * ring_entry].chunks(ring_entry).collect();
    assert!(ring_len > 0 && ring_len <= ring_cap);
    let times: Vec<u32> = ring.iter().map(|e| u32::from_le_bytes([e[0], e[1], e[2], e[3]])).collect();
    assert!(times.windows(2).all(|w| w[0] <= w[1]), "oldest first: {times:?}");
    assert!(ring.iter().any(|e| e[10] & 0x60 == 0x20 && u16::from_le_bytes([e[11], e[12]]) == 0x08 && e[10] & 0x08 != 0),
        "a Link Status (NWK command 0x08) to everyone in range");
    // rows: one every two minutes, with the unicasts sent and the way to the coordinator
    let rows_at = ring_at + ring_cap * ring_entry;
    assert!(rows_len >= 5 && rows_len <= rows_cap, "rows: {rows_len}");
    let rows: Vec<&[u8]> = b[rows_at..rows_at + rows_len * row_entry].chunks(row_entry).collect();
    let minutes: Vec<u16> = rows.iter().map(|r| u16::from_le_bytes([r[0], r[1]])).collect();
    // the first row counts from the start, not from zero (the counters carry over restarts)
    assert!(minutes[0] >= 2, "no row at the start itself: {minutes:?}");
    assert!(minutes.windows(2).all(|w| w[1] == w[0] + 2), "two minutes apart: {minutes:?}");
    let sent: u32 = rows.iter().map(|r| u16::from_le_bytes([r[2], r[3]]) as u32).sum();
    assert!(sent >= 30, "unicasts sent in the rows: {sent}");
    let last = rows.last().unwrap();
    assert!(last[8] & 0x10 != 0 && last[8] & 7 >= 1, "straight to the coordinator, with a cost: {:#04x}", last[8]);
    assert_eq!(last[11] as i8, -97, "the noise floor");
    // past the end: an empty page; past the pages: not an attribute
    let pages = total.div_ceil(64) as u16;
    let r = mfr(&mut net, short, &[0x6000 + pages]);
    assert!(!r.contains_key(&(0x6000 + pages)) || r[&(0x6000 + pages)].1 == vec![0]);
}

/// (id, type, value) of `Stack::diag_counters`.
fn counters_snapshot(net: &mut Net, n: usize) -> Vec<(u16, u8, i64)> {
    let mut b = [0u8; zigbee::stack::DIAG_COUNTERS_MAX];
    let len = net.nodes[n].stack.diag_counters(&mut b);
    assert_eq!(len, b.len(), "every attribute written");
    assert_eq!(b[0], 1, "format 1");
    let mut out = Vec::new();
    let mut i = 1;
    while i < len {
        let (id, ty) = (u16::from_le_bytes([b[i], b[i + 1]]), b[i + 2]);
        let w = value_len(ty, &[]);
        let mut v = number(&b[i + 3..i + 3 + w]) as i64;
        if ty == 0x28 {
            v = v as u8 as i8 as i64;
        }
        out.push((id, ty, v));
        i += 3 + w;
    }
    out
}

fn read_all(net: &mut Net, short: u16, ids: &[u16]) -> HashMap<u16, (u8, Vec<u8>)> {
    let mut out = HashMap::new();
    for chunk in ids.chunks(8) {
        out.extend(if chunk[0] >= 0x5000 { mfr(net, short, chunk) } else { diag(net, short, chunk) });
    }
    out
}

#[test]
fn the_counters_snapshot_holds_what_the_cluster_answers() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    net.run(5_000);
    let first = counters_snapshot(&mut net, n);
    let ids: Vec<u16> = first.iter().map(|c| c.0).collect();
    // the standard attributes of the table and the manufacturer's numbers up to FlashErases, in order
    let mut expected = vec![0x0000, 0x0001];
    expected.extend(0x0100..=0x011D);
    expected.extend(0x5000..=0x5014);
    assert_eq!(ids, expected);
    let before = read_all(&mut net, short, &ids);
    let snap = counters_snapshot(&mut net, n);
    let after = read_all(&mut net, short, &ids);
    for (id, ty, v) in snap {
        let (a, b) = (&before[&id], &after[&id]);
        assert_eq!((a.0, b.0), (ty, ty), "{id:#06x}: the cluster's type");
        let num = |x: &[u8]| if ty == 0x28 { x[0] as i8 as i64 } else { number(x) as i64 };
        let (lo, hi) = (num(&a.1).min(num(&b.1)), num(&a.1).max(num(&b.1)));
        assert!((lo..=hi).contains(&v), "{id:#06x}: {v} between the reads {lo}..={hi}");
    }
}

#[test]
fn the_light_settings_and_the_options_as_the_device_page_sets_them() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let s = net.nodes[n].stack.light_settings();
    assert_eq!((s.start_up_on_off, s.start_up_level, s.level_options), (0xFF, 0xFF, 0));
    assert_eq!((s.color_options, s.ct_min, s.ct_max, s.start_up_color_temperature), (None, 0, 0, 0xFFFF), "a dimmable bulb");
    assert_eq!((s.min_level, s.max_level, s.global_scene_control), (1, 254, true));
    // a dimmable bulb has no colour temperature to couple, and no Color Control
    assert!(!net.nodes[n].stack.set_level_options(0x02));
    assert!(!net.nodes[n].stack.set_color_options(0x01));
    assert!(net.nodes[n].stack.set_level_options(0x01));
    assert_eq!(net.nodes[n].stack.light_settings().level_options, 1);
    let seq = net.coord.zcl(short, 0x0008, 0x00, 0x00, &ids(&[0x000F]));
    net.run(60);
    let rsp = net.coord.last_zcl(short, 0x0008, 0x01).unwrap();
    assert_eq!(rsp, vec![0x18, seq, 0x01, 0x0F, 0x00, 0x00, 0x18, 0x01], "the same through Zigbee");
}

#[test]
fn identify_on_request_is_logged_and_frames_to_the_light_are_handed_out() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let seq = net.nodes[n].stack.event_seq();
    net.nodes[n].stack.log_identify();
    assert_eq!(net.nodes[n].stack.event_seq(), seq.wrapping_add(1));
    let e = net.nodes[n].stack.event(0).unwrap();
    assert_eq!((e.kind, e.seq, e.this_start), (zigbee::stack::ev::IDENTIFY, seq, true));
    let count = (0..30).take_while(|i| net.nodes[n].stack.event(*i).is_some()).count();
    assert!(count < 30 && net.nodes[n].stack.event(count).is_none());
    assert_eq!(net.nodes[n].stack.event(count - 1).unwrap().kind, zigbee::stack::ev::START, "the oldest");

    let id = net.nodes[n].id;
    net.air.borrow_mut().ports[id].zcl_received.clear();
    net.link(0, id, 180);
    net.coord.zcl(short, ON_OFF, 0x01, 0x01, &[]);
    net.run(60);
    let got = net.air.borrow().ports[id].zcl_received.clone();
    assert_eq!(got.len(), 1, "{got:?}");
    let (src, cluster, cmd, fc, lqi, rssi) = got[0];
    assert_eq!((src, cluster, cmd, fc & 0x07, lqi), (0x0000, ON_OFF, 0x01, 0x01, 180));
    assert!(rssi < 0);
    // the Diagnostics cluster on the same endpoint counts too
    net.air.borrow_mut().ports[id].zcl_received.clear();
    diag(&mut net, short, &[RESETS]);
    assert_eq!(net.air.borrow().ports[id].zcl_received.iter().map(|f| (f.1, f.2)).collect::<Vec<_>>(), [(DIAG, 0x00)]);
}
