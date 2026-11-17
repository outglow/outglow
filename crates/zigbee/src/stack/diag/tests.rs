use super::super::address_tests::{Scripted, IDENTITY};
use super::{attribute, k, Attr, Diag, Octets, Src, T_OCTETS, T_U32};
use crate::stack::Stack;
extern crate std;
use std::vec::Vec;

#[test]
fn a_diagnostics_attribute_is_a_number_of_its_type_or_octets() {
    assert_eq!(attribute(false, 0x0100).map(Attr::ty), Some(T_U32));
    assert!(attribute(false, 0x0100).and_then(Attr::number).is_some_and(|(ty, src)| ty == T_U32 && src == Src::Counter(k::MAC_RX_BCAST)));
    assert!(attribute(true, 0x5032) == Some(Attr::Octets(Octets::ExtPan)));
    assert_eq!(attribute(true, 0x5032).map(Attr::ty), Some(T_OCTETS));
    assert!(attribute(true, 0x5032).and_then(Attr::number).is_none());
}

#[test]
fn the_average_retries_of_nothing_sent_is_zero() {
    let mut d = Diag::new(0);
    assert_eq!((d.average_retries(), d.average_retries_x100()), (0, 0));
    d.n[k::APS_SENT] = 4;
    d.n[k::APS_MAC_RETRIES] = 6;
    assert_eq!((d.average_retries(), d.average_retries_x100()), (2, 150));
}

#[test]
fn without_a_noise_measurement_the_next_try_is_a_second_later() {
    let mut s = Stack::new(Scripted { random: Vec::new(), now: 1_000, sent: Vec::new(), deaf: Vec::new(), busy: false, attempts: 0 }, IDENTITY);
    s.diag.noise_at = 1_000;
    s.diag_noise(1_000);
    assert_eq!(s.diag.noise_at, 2_000);
}

#[test]
fn rssi_falls_in_buckets_split_at_minus_90_80_70_and_60_dbm() {
    let mut d = Diag::new(0);
    for rssi in [-91, -90, -81, -80, -71, -70, -61, -60, 0] {
        d.rssi_add(rssi);
    }
    assert_eq!(d.rssi, [1, 2, 2, 2, 2]);
}

#[test]
fn a_peak_keeps_the_largest_value() {
    let mut d = Diag::new(0);
    d.peak(k::APS_SENT, 5);
    d.peak(k::APS_SENT, 4);
    assert_eq!(d.n[k::APS_SENT], 5);
    d.peak(k::APS_SENT, 6);
    assert_eq!(d.n[k::APS_SENT], 6);
}

#[test]
fn relays_lost_add_up_the_three_causes_and_ble_time_is_the_hals() {
    let mut d = Diag::new(0);
    d.n[k::RELAY_DROP_QUEUE_FULL] = 1;
    d.n[k::RELAY_DROP_NO_ROUTE] = 20;
    d.n[k::RELAY_DROP_MAC_FAIL] = 300;
    assert_eq!(d.relays_lost(), 321);
    d.hal_seen.ble_ms = 1234;
    assert_eq!(d.ble_ms_since_start(), 1234);
}

#[test]
fn the_event_log_round_trips_and_one_with_an_impossible_head_or_length_or_cut_short_is_not_taken() {
    let mut d = Diag::new(0);
    for k in 0..3u16 {
        d.event(1_000 * k as u32, 7, k, 100 + k);
    }
    let mut b = [0u8; super::EVENTS_LEN];
    d.encode_events(&mut b);
    let mut e = Diag::new(0);
    e.decode_events(&b);
    assert!((e.ev_head, e.ev_len, e.events[1].a, e.events[2].b, e.events[2].t) == (d.ev_head, d.ev_len, 1, 102, 2));
    for (at, v, taken) in [(0, super::EVENTS as u8 - 1, true), (0, super::EVENTS as u8, false), (1, super::EVENTS as u8, true), (1, super::EVENTS as u8 + 1, false)] {
        let mut c = b;
        c[at] = v;
        let mut e = Diag::new(0);
        e.decode_events(&c);
        assert_eq!(e.ev_len == c[1] && e.ev_head == c[0], taken, "{at} {v}");
    }
    let mut e = Diag::new(0);
    e.decode_events(&b[..super::EVENTS_LEN - 1]);
    assert_eq!(e.ev_len, 0, "cut short");
}

#[test]
fn a_frame_sent_is_counted_as_unicast_or_broadcast_and_one_not_sent_on_a_busy_channel_is_not() {
    let stack = || Stack::new(Scripted { random: Vec::new(), now: 1_000, sent: Vec::new(), deaf: Vec::new(), busy: false, attempts: 0 }, IDENTITY);
    let mut s = stack();
    s.neighbors[0].used = true;
    s.neighbors[0].short = 0x1000;
    let h = crate::nwk::Header::new(crate::nwk::FRAME_DATA, 0x1000, 0x0042, 5, 1);
    s.transmit_nwk(0x1000, &h, &[0x00]);
    assert_eq!((s.diag.n[k::MAC_TX_UCAST], s.diag.n[k::MAC_TX_BCAST], s.neighbors[0].tx), (1, 0, 1));
    s.transmit_nwk(crate::mac::BROADCAST, &h, &[0x00]);
    assert_eq!((s.diag.n[k::MAC_TX_UCAST], s.diag.n[k::MAC_TX_BCAST]), (1, 1));
    s.hal.busy = true;
    s.transmit_nwk(0x1000, &h, &[0x00]);
    assert_eq!((s.diag.n[k::MAC_TX_UCAST], s.diag.n[k::CHANNEL_ACCESS_FAILURES], s.neighbors[0].tx), (1, 1, 1));
    let mut s = stack();
    s.state = crate::stack::State::Up;
    s.neighbors[0].used = true;
    s.neighbors[0].short = 0x1000;
    s.children[0].used = true;
    s.children[0].ext = 0x00124B00_0000_1000;
    let mh = crate::mac::Header {
        frame_type: crate::mac::FRAME_CMD,
        security: false,
        pending: false,
        ack_request: false,
        seq: 1,
        dst_pan: 0xFFFF,
        dst: crate::mac::Addr::Short(0xFFFF),
        src_pan: 0xFFFF,
        src: crate::mac::Addr::Ext(0x00124B00_0000_1000),
    };
    s.on_mac_command(&mh, &[crate::mac::CMD_ORPHAN]);
    assert_eq!((s.hal.sent.len(), s.diag.n[k::MAC_TX_UCAST], s.neighbors[0].tx), (1, 1, 0), "to an extended address: no neighbour's");
}

#[test]
fn a_frame_sent_after_attempts_on_a_busy_channel_counts_as_sent() {
    let mut s = Stack::new(Scripted { random: Vec::new(), now: 1_000, sent: Vec::new(), deaf: Vec::new(), busy: true, attempts: 3 }, IDENTITY);
    let h = crate::nwk::Header::new(crate::nwk::FRAME_DATA, 0x1000, 0x0042, 5, 1);
    s.transmit_nwk(0x1000, &h, &[0x00]);
    assert_eq!((s.diag.n[k::MAC_TX_UCAST], s.diag.n[k::MAC_TX_UCAST_RETRY], s.diag.n[k::CHANNEL_ACCESS_FAILURES]), (1, 2, 1));
}
