use super::*;
use crate::sim::{self, Op};

const VENDOR_BLE_CHANNELS: [(u8, u8, u8, u8); 8] = [
    (37, 0x25, 0x00, 0x18),
    (38, 0x26, 0x0c, 0x14),
    (39, 0x27, 0x27, 0x0c),
    (1, 0x01, 0x02, 0x14),
    (7, 0x07, 0x08, 0x14),
    (8, 0x08, 0x09, 0x14),
    (14, 0x0e, 0x10, 0x14),
    (21, 0x15, 0x17, 0x10),
];

const VENDOR_BLE_1M_INIT: [Op; 23] = [
    Op::W8(0x1223, 0x23),
    Op::W8(0x1221, 0x0a),
    Op::W8(0x12d2, 0x9b),
    Op::W8(0x12d3, 0x19),
    Op::W8(0x127b, 0x0e),
    Op::W8(0x1276, 0x50),
    Op::W32(0x1220, 0x2320_0a16),
    Op::W8(0x124a, 0x0e),
    Op::W8(0x124b, 0x09),
    Op::W8(0x400, 0x1f),
    Op::W8(0x401, 0x01),
    Op::W8(0x402, 0x46),
    Op::W8(0x404, 0xf5),
    Op::W8(0x405, 0x04),
    Op::W8(0x420, 0x1e),
    Op::W8(0x421, 0xa1),
    Op::W8(0x430, 0x3e),
    Op::W32(0x460, 0x5f4f_4434),
    Op::W16(0x464, 0x766b),
    Op::W8(0xf06, 0x00),
    Op::W8(0xf0c, 0x50),
    Op::W8(0xf0e, 0x00),
    Op::W8(0xf10, 0x00),
];

#[test]
fn ble_channels_are_tuned_as_the_vendor_library_tunes_them() {
    for (chn, whitening, rf, band) in VENDOR_BLE_CHANNELS {
        let _s = sim::session();
        set_channel(chn);
        assert_eq!(
            sim::with(|c| c.writes()),
            vec![Op::W8(0x40d, whitening), Op::W8(0x1244, 0x20), Op::W8(0x1245, 0x26), Op::W8(0x1228, rf), Op::W8(0x1229, band)],
            "channel {chn}"
        );
    }
}

fn quiet_air() {
    crate::radio::tests_support::forget_rx();
    sim::with(|c| c.tick = 0x1000_0000);
}

#[test]
fn an_unanswered_advertising_event_matches_the_vendor_trace() {
    let _s = sim::session();
    quiet_air();
    let events = BLE_RADIO.at(ADV_EVENTS).unwrap_or(0);
    let lends = lent().0;
    let mut connect = [0u8; CONNECT_LEN];
    assert_eq!(advertise(&[0x00, 0x06, 1, 2, 3, 4, 5, 6], &[0x04, 0x06, 1, 2, 3, 4, 5, 6], &mut connect), None);
    let w = sim::with(|c| c.writes());
    let table = w.iter().position(|o| *o == VENDOR_BLE_1M_INIT[0]).expect("the BLE 1M table");
    assert_eq!(w[table..table + 23].to_vec(), VENDOR_BLE_1M_INIT.to_vec());
    let at = w.iter().position(|o| *o == Op::W32(0x408, ADV_ACCESS)).expect("the advertising access address");
    assert_eq!(w[at - 3..at].to_vec(), vec![Op::W8(0xf16, 0x29), Op::W8(0x428, 0x80), Op::W8(0xf02, 0x45)]);
    assert_eq!(w[at + 1..at + 4].to_vec(), vec![Op::W32(0x424, 0x0055_5555), Op::W16(0xf04, 0x004a), Op::W16(0xf1c, 0x0000)]);
    for (chn, whitening, rf, band) in VENDOR_BLE_CHANNELS[..3].iter().copied() {
        let i = w.iter().position(|o| *o == Op::W8(0x40d, whitening)).unwrap_or_else(|| panic!("channel {chn}"));
        assert_eq!(w[i - 1], Op::W8(0xf00, 0x80));
        assert_eq!(w[i + 1..i + 5].to_vec(), vec![Op::W8(0x1244, 0x20), Op::W8(0x1245, 0x26), Op::W8(0x1228, rf), Op::W8(0x1229, band)]);
        assert_eq!(w[i + 5], Op::W16(0xf20, 0x0003));
        assert!(matches!(w[i + 6], Op::W32(0xf18, _)));
        assert_eq!(w[i + 7..i + 9].to_vec(), vec![Op::W8(0xf16, 0x2d), Op::W8(0xf00, 0x87)]);
        assert!(matches!(w[i + 9], Op::W16(0xc0c, _)));
        assert_eq!(w[i + 10], Op::W16(0xf20, 0x0002));
    }
    assert_eq!(BLE_RADIO.at(ADV_EVENTS), Some(events + 1));
    assert_eq!(lent().0, lends.wrapping_add(1));
    assert!(lent_max_us() > 0);
}

#[test]
fn a_recently_busy_zigbee_air_advertises_on_channel_37_only() {
    let _s = sim::session();
    sim::with(|c| c.tick = 0x1000_0000);
    crate::radio::tests_support::heard_now();
    let mut connect = [0u8; CONNECT_LEN];
    let _ = advertise(&[0x00, 0x06, 1, 2, 3, 4, 5, 6], &[0x04, 0x06, 1, 2, 3, 4, 5, 6], &mut connect);
    let w = sim::with(|c| c.writes());
    assert!(w.contains(&Op::W8(0x40d, 0x25)));
    assert!(!w.contains(&Op::W8(0x40d, 0x26)));
}

#[test]
fn advertising_data_of_a_wrong_length_is_refused() {
    let _s = sim::session();
    let mut connect = [0u8; CONNECT_LEN];
    assert_eq!(advertise(&[0], &[0, 0], &mut connect), None);
    assert_eq!(advertise(&[0, 0], &[0; ADV_PDU_MAX + 1], &mut connect), None);
    assert!(sim::with(|c| c.writes()).is_empty());
}

#[test]
fn the_tx_queue_holds_eight_packets_and_resets_with_the_connection() {
    let _s = sim::session();
    connection_reset();
    assert!(tx_idle());
    assert_eq!(tx_free(), TX_SLOTS);
    assert!(!tx_push(2, &[0; LL_PAYLOAD_MAX + 1]));
    for i in 0..TX_SLOTS {
        assert!(tx_push(2, &[i as u8; 3]));
    }
    assert!(!tx_push(2, &[0]));
    assert_eq!(tx_free(), 0);
    assert!(!tx_idle());
    assert_eq!(TX_QUEUE.read(4), 2);
    assert_eq!(TX_QUEUE.read(5), 3);
    assert_eq!(TX_QUEUE.read32(0), 5);
    connection_reset();
    assert!(tx_idle());
    assert_eq!(SN_NESN.get(), 0x10);
    assert_eq!(RESET_PENDING.get(), 1);
}

#[test]
fn received_packets_come_out_in_order() {
    let _s = sim::session();
    connection_reset();
    let mut p = [0u8; LL_PAYLOAD_MAX];
    assert_eq!(rx_pop(&mut p), None);
    for (i, llid) in [(0usize, 2u8), (1, 3)] {
        let slot = i * RX_SLOT;
        RX_QUEUE.write(slot, llid);
        RX_QUEUE.write(slot + 1, 2);
        RX_QUEUE.copy_from(slot + 2, &[llid, llid]);
    }
    RX_HEAD.set(2);
    assert_eq!(rx_pop(&mut p), Some((2, 2)));
    assert_eq!(p[..2], [2, 2]);
    assert_eq!(rx_pop(&mut p), Some((3, 2)));
    assert_eq!(rx_pop(&mut p), None);
}

#[test]
fn the_crc_helpers_follow_the_ble_polynomial() {
    assert_eq!(reverse24(0x55_5555), 0xaa_aaaa);
    assert_eq!(reverse24(0x00_0001), 0x80_0000);
    assert_eq!(reverse24(0x80_0000), 0x00_0001);
    let t = crc_table();
    assert_eq!(t[0], 0);
    assert_eq!(t[8], 0xda_6000);
    assert!(reached(5, 5));
    assert!(reached(6, 5));
    assert!(!reached(4, 5));
    assert!(reached(0, u32::MAX));
}

#[test]
fn a_mesh_burst_goes_out_on_every_channel_and_a_silent_listen_finds_nothing() {
    let _s = sim::session();
    quiet_air();
    assert!(mesh_send(&[37], 0x1234_5678, &[1]));
    assert!(sim::with(|c| c.writes()).is_empty());
    let bursts = BLE_RADIO.at(MESH_BURSTS).unwrap_or(0);
    assert!(mesh_send(&[37, 38, 39], 0x1234_5678, &[0x02, 0x03, 1, 2, 3]));
    let w = sim::with(|c| c.writes());
    assert!(w.contains(&Op::W32(0x408, 0x1234_5678)));
    assert_eq!(w.iter().filter(|o| **o == Op::W8(0xf00, 0x85)).count(), 3);
    assert_eq!(BLE_RADIO.at(MESH_BURSTS), Some(bursts + 1));
    let mut pdu = [0u8; ADV_PDU_MAX];
    assert_eq!(mesh_listen(37, 0x1234_5678, 1000 * TICKS_PER_US, &mut pdu), None);
}
