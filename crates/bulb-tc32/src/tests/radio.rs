use super::*;
use crate::sim::{self, Op};

const VENDOR_INIT_TABLE: [(usize, u8); 35] = [
    (0x12d2, 0x9b),
    (0x12d3, 0x19),
    (0x127b, 0x0e),
    (0x1276, 0x50),
    (0x1277, 0x73),
    (0x0430, 0x3e),
    (0x1220, 0x04),
    (0x1221, 0x2b),
    (0x1222, 0x43),
    (0x1223, 0x86),
    (0x122a, 0x90),
    (0x1254, 0x0e),
    (0x1255, 0x09),
    (0x1256, 0x0c),
    (0x1257, 0x08),
    (0x1258, 0x09),
    (0x1259, 0x0f),
    (0x0400, 0x13),
    (0x0420, 0x18),
    (0x0402, 0x46),
    (0x0404, 0xc0),
    (0x0405, 0x04),
    (0x0421, 0x23),
    (0x0422, 0x04),
    (0x0408, 0xa7),
    (0x0409, 0x00),
    (0x040a, 0x00),
    (0x040b, 0x00),
    (0x0460, 0x36),
    (0x0461, 0x46),
    (0x0462, 0x51),
    (0x0463, 0x61),
    (0x0464, 0x6d),
    (0x0465, 0x78),
    (0x0401, 0x00),
];

const VENDOR_CHANNELS: [(u8, u8, u8, u8); 16] = [
    (11, 0x95, 0x25, 0x14),
    (12, 0xa9, 0x25, 0x14),
    (13, 0xbd, 0x25, 0x14),
    (14, 0xd1, 0x25, 0x14),
    (15, 0xe5, 0x25, 0x14),
    (16, 0xf9, 0x25, 0x14),
    (17, 0x0d, 0x26, 0x10),
    (18, 0x21, 0x26, 0x10),
    (19, 0x35, 0x26, 0x10),
    (20, 0x49, 0x26, 0x10),
    (21, 0x5d, 0x26, 0x10),
    (22, 0x71, 0x26, 0x10),
    (23, 0x85, 0x26, 0x0c),
    (24, 0x99, 0x26, 0x0c),
    (25, 0xad, 0x26, 0x0c),
    (26, 0xc1, 0x26, 0x0c),
];

fn w8(writes: &[Op]) -> Vec<(usize, u8)> {
    writes
        .iter()
        .filter_map(|o| match *o {
            Op::W8(a, v) => Some((a, v)),
            _ => None,
        })
        .collect()
}

fn channel_writes(mhz_low: u8, mhz_high: u8, band: u8) -> Vec<(usize, u8)> {
    vec![(0xf02, 0x45), (0x1228, 0x13), (0x1244, mhz_low), (0x1245, mhz_high), (0x1229, band)]
}

fn reset_radio_state() {
    FREQ_OFFSET.set(5);
    HEAD.set(0);
    TAIL.set(0);
    ACK_WANTED.set(0);
    ACK_SEEN.set(0);
    PAN.set(0xffff);
    SHORT.set(0xffff);
    EXT.set([0; 8]);
    N_PENDING_SHORT.set(0);
    N_PENDING_EXT.set(0);
    N_CHILDREN.set(0);
    RADIO_DROPPED.set(0);
    CCA_BUSY.set(0);
    LAST_RX.set(0);
}

#[test]
fn init_loads_the_vendor_tables_and_ends_in_receive_mode() {
    let _s = sim::session();
    reset_radio_state();
    init();
    let writes = sim::with(|c| c.writes());
    let table: Vec<(usize, u8)> = w8(&writes[..35]);
    assert_eq!(table, VENDOR_INIT_TABLE.to_vec());
    assert_eq!(writes[35], Op::W8(0xc20, 0x0c));
    let mut expected = channel_writes(0x95, 0x25, 0x14);
    expected.extend([(0xf16, 0x29), (0x428, 0xe0), (0xf02, 0x45)]);
    expected.extend([(0x1225, 0x00), (0x1226, 0x80), (0x1227, 0x1f)]);
    assert_eq!(w8(&writes[36..47]), expected);
    let rest: Vec<Op> = writes[47..].to_vec();
    assert!(matches!(rest[0], Op::W16(0xc08, _)));
    assert_eq!(
        rest[1..].to_vec(),
        vec![
            Op::W8(0xf03, 0x00),
            Op::W8(0xc0a, 0x09),
            Op::W8(0xc0b, 0x01),
            Op::W8(0xc21, 0x00),
            Op::W16(0xf1c, 0x0001),
            Op::W32(0x640, 0x0000_2000),
            Op::W8(0xc20, 0x0c),
            Op::W8(0xc20, 0x0c),
            Op::W8(0x430, 0x3e),
            Op::W8(0xf02, 0x45),
            Op::W8(0x1228, 0x13),
            Op::W8(0x1244, 0x95),
            Op::W8(0x1245, 0x25),
            Op::W8(0x1229, 0x14),
            Op::W8(0xf02, 0x65),
            Op::W8(0x428, 0xe1),
            Op::W8(0x643, 0x01),
        ]
    );
    assert!(sim::with(|c| c.analog_writes()).is_empty());
}

#[test]
fn a_frequency_trim_in_flash_sets_the_crystal_capacitor() {
    let _s = sim::session();
    reset_radio_state();
    sim::with(|c| {
        c.flash[CAP_TRIM_ADDR] = 0x2b;
        c.set_analog(0x8a, 0xff);
    });
    init();
    assert_eq!(sim::with(|c| c.analog_writes()), vec![(0x8a, 0x40 | 0x2b)]);
}

#[test]
fn every_channel_is_tuned_as_the_vendor_driver_tunes_it() {
    for (ch, low, high, band) in VENDOR_CHANNELS {
        let _s = sim::session();
        reset_radio_state();
        sim::with(|c| {
            c.poke8(0x1245, 0xc0);
            c.poke8(0x1229, 0xc3);
            c.poke8(0x428, 0xe0);
        });
        set_channel(ch);
        let mut expected = vec![(0x643, 0x00)];
        expected.extend(channel_writes(low, 0xc0 | high, 0xc3 | band));
        expected.extend([(0xf02, 0x65), (0x428, 0xe1), (0x643, 0x00)]);
        assert_eq!(w8(&sim::with(|c| c.writes())), expected, "channel {ch}");
    }
}

#[test]
fn channels_outside_11_to_26_are_ignored() {
    let _s = sim::session();
    reset_radio_state();
    set_channel(10);
    set_channel(27);
    assert!(sim::with(|c| c.writes()).is_empty());
    assert_eq!(FREQ_OFFSET.get(), 5);
}

fn kick_ops(writes: &[Op]) -> usize {
    writes.windows(3).filter(|w| matches!(w, [Op::W8(0xc43, 0x04), Op::W16(0xc0c, _), Op::W8(0xc24, v)] if v & 0x08 != 0)).count()
}

#[test]
fn a_frame_without_ack_request_is_sent_once_in_transmit_mode() {
    let _s = sim::session();
    reset_radio_state();
    let mpdu = [0x03, 0x08, 0x46, 0xff, 0xff, 0xff, 0xff, 0x07];
    assert_eq!(transmit(&mpdu, || 0), TxStatus::Ok);
    let writes = sim::with(|c| c.writes());
    assert_eq!(kick_ops(&writes), 1);
    let kick = writes.iter().position(|o| matches!(o, Op::W8(0xc43, 0x04))).unwrap_or(0);
    assert!(w8(&writes[..kick]).ends_with(&[(0xf02, 0x55), (0x428, 0xe0), (0xf20, 0x02)]));
    let mut buf = [0u8; 13];
    TX_BUF.copy_to(0, &mut buf);
    assert_eq!(buf, [9, 0, 0, 0, 10, 0x03, 0x08, 0x46, 0xff, 0xff, 0xff, 0xff, 0x07]);
    assert_eq!(counters().3, 1);
}

#[test]
fn an_unacknowledged_frame_is_sent_four_times() {
    let _s = sim::session();
    reset_radio_state();
    let mpdu = [0x61, 0x88, 0x10, 0x62, 0x1a, 0x00, 0x00, 0x01, 0x00];
    assert_eq!(transmit(&mpdu, || 0), TxStatus::NoAck);
    assert_eq!(sim::with(|c| c.tx_kicks), 4);
    assert_eq!(ACK_WANTED.get(), 0);
}

#[test]
fn a_busy_channel_ends_in_channel_busy_without_sending() {
    let _s = sim::session();
    reset_radio_state();
    sim::with(|c| c.poke8(RF_RSSI, 0x40));
    assert_eq!(transmit(&[0x41, 0x88, 0x01, 0x62, 0x1a, 0xff, 0xff, 0x00, 0x00], || 7), TxStatus::ChannelBusy);
    assert_eq!(sim::with(|c| c.tx_kicks), 0);
    assert_eq!(CCA_BUSY.get(), 4 * MAX_CCA_ATTEMPTS);
    assert!(receiving());
}

#[test]
fn frames_too_short_or_too_long_are_not_sent() {
    let _s = sim::session();
    reset_radio_state();
    assert_eq!(transmit(&[1, 2], || 0), TxStatus::Ok);
    assert_eq!(transmit(&[0; 126], || 0), TxStatus::Ok);
    assert_eq!(sim::with(|c| c.tx_kicks), 0);
}

fn deliver(bytes: &[u8]) {
    RX_BUF.copy_from(0, bytes);
    sim::with(|c| {
        c.poke8(RF_IRQ_STATUS, RF_IRQ_RX);
        c.poke8(DMA_CHANNEL_ENABLE, 0x0c);
        c.log.clear();
    });
    irq_handler();
}

const BEACON: [u8; 41] = [
    0x25, 0x00, 0x00, 0x00, 0x1c, 0x00, 0x80, 0x40, 0x62, 0x1a, 0x00, 0x00, 0xff, 0xcf, 0x00, 0x00, 0x00, 0x22, 0x84, 0xdd,
    0xdd, 0xdd, 0xdd, 0xdd, 0xdd, 0xdd, 0xdd, 0xff, 0xff, 0xff, 0x00, 0xd1, 0x0f, 0x21, 0xd6, 0x29, 0x01, 0x00, 0x00, 0x46,
    0x10,
];

#[test]
fn the_vendor_traces_beacon_is_queued_with_its_rssi() {
    let _s = sim::session();
    reset_radio_state();
    deliver(&BEACON);
    assert_eq!(
        sim::with(|c| c.writes()),
        vec![Op::W8(0xf20, 0x01), Op::W8(0xc20, 0x08), Op::W8(0xc20, 0x0c), Op::W8(0xc26, 0xfc)]
    );
    let mut buf = [0u8; 127];
    let (len, rssi, end) = receive(&mut buf).expect("the beacon is queued");
    assert_eq!(len, 26);
    assert_eq!(rssi, -40);
    assert_eq!(&buf[..len], &BEACON[5..31]);
    assert_eq!(end, 0x0129_d621u32.wrapping_add(29 * 32 * 16));
    assert!(receive(&mut buf).is_none());
    assert_eq!(RX_BUF.read(0), 0);
}

#[test]
fn frames_with_a_bad_crc_status_or_length_are_dropped() {
    let _s = sim::session();
    reset_radio_state();
    let mut bad = BEACON;
    bad[40] = 0x11;
    deliver(&bad);
    let mut short = BEACON;
    short[0] = 0x24;
    deliver(&short);
    assert!(!has_frame());
}

const ASSOCIATION_RESPONSE: [u8; 40] = [
    0x24, 0x00, 0x00, 0x00, 0x1b, 0x63, 0xcc, 0x41, 0x62, 0x1a, 0x88, 0xd4, 0xe9, 0xe7, 0x80, 0x38, 0xc1, 0xa4, 0x01, 0x00,
    0x00, 0x00, 0x00, 0x4b, 0x12, 0x00, 0x02, 0x21, 0x7a, 0x00, 0xaa, 0xc5, 0x20, 0x0f, 0x4c, 0x02, 0x00, 0x00, 0x46, 0x10,
];
const OUR_EXT: u64 = 0xa4c1_3880_e7e9_d488;

fn frame_end(bytes: &[u8]) -> u32 {
    let dma_len = bytes[0] as usize;
    let stamp = u32::from_le_bytes([bytes[dma_len - 4], bytes[dma_len - 3], bytes[dma_len - 2], bytes[dma_len - 1]]);
    stamp.wrapping_add((bytes[4] as u32 + 1) * 32 * 16)
}

#[test]
fn a_frame_for_our_long_address_is_acknowledged_after_the_turnaround() {
    let _s = sim::session();
    reset_radio_state();
    set_filter(0x1a62, 0x0000, OUR_EXT);
    let end = frame_end(&ASSOCIATION_RESPONSE);
    sim::with(|c| c.tick = end.wrapping_sub(50 * 16));
    deliver(&ASSOCIATION_RESPONSE);
    let writes = sim::with(|c| c.writes());
    assert_eq!(writes[0], Op::W8(0xf20, 0x01));
    assert_eq!(writes[1], Op::W8(0xc20, 0x08));
    assert_eq!(w8(&writes[2..9]), {
        let mut v = channel_writes(0x95, 0x25, 0x14);
        v.extend([(0xf02, 0x55), (0x428, 0x00)]);
        v
    });
    assert_eq!(kick_ops(&writes), 1);
    assert!(sim::with(|c| c.tick).wrapping_sub(end) >= ACK_TURNAROUND_US * 16);
    let mut ack = [0u8; 8];
    ACK_BUF.copy_to(0, &mut ack);
    assert_eq!(ack, [4, 0, 0, 0, 5, 0x02, 0x00, 0x41]);
    assert_eq!(*writes.last().unwrap_or(&Op::W8(0, 0)), Op::W8(0xc26, 0xfc));
    assert!(has_frame());
}

#[test]
fn a_late_frame_is_queued_but_not_acknowledged() {
    let _s = sim::session();
    reset_radio_state();
    set_filter(0x1a62, 0x0000, OUR_EXT);
    let end = frame_end(&ASSOCIATION_RESPONSE);
    sim::with(|c| c.tick = end.wrapping_add(700 * 16));
    deliver(&ASSOCIATION_RESPONSE);
    assert_eq!(sim::with(|c| c.tx_kicks), 0);
    assert!(has_frame());
}

#[test]
fn frames_for_another_pan_or_address_are_dropped() {
    let _s = sim::session();
    reset_radio_state();
    set_filter(0x1234, 0x0000, OUR_EXT);
    deliver(&ASSOCIATION_RESPONSE);
    set_filter(0x1a62, 0x0000, OUR_EXT ^ 1);
    deliver(&ASSOCIATION_RESPONSE);
    assert!(!has_frame());
    assert_eq!(sim::with(|c| c.tx_kicks), 0);
}

fn data_request(seq: u8, src: u16) -> Vec<u8> {
    let psdu = [0x63, 0x88, seq, 0x62, 0x1a, 0x00, 0x00, src as u8, (src >> 8) as u8, 0x04, 0x00, 0x00];
    let phr = psdu.len() as u8;
    let mut v = vec![phr + 9, 0, 0, 0, phr];
    v.extend_from_slice(&psdu);
    v.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0x46, 0x10]);
    v
}

fn ack_for(bytes: &[u8]) -> u8 {
    let end = frame_end(bytes);
    sim::with(|c| c.tick = end.wrapping_sub(16));
    deliver(bytes);
    let mut buf = [0u8; 127];
    let _ = receive(&mut buf);
    ACK_BUF.read(5)
}

#[test]
fn a_data_request_learns_from_the_ack_whether_data_is_pending() {
    let _s = sim::session();
    reset_radio_state();
    set_filter(0x1a62, 0x0000, OUR_EXT);
    set_children(&[0x1111, 0x2222]);
    set_pending(&[0x2222], &[]);
    assert_eq!(ack_for(&data_request(1, 0x1111)), 0x02);
    assert_eq!(ack_for(&data_request(2, 0x2222)), 0x12);
    assert_eq!(ack_for(&data_request(3, 0x3333)), 0x12);
}

#[test]
fn a_full_queue_counts_the_dropped_frame() {
    let _s = sim::session();
    reset_radio_state();
    for _ in 0..SLOTS {
        deliver(&BEACON);
    }
    assert_eq!(RADIO_DROPPED.get(), 1);
    assert_eq!(counters().1, 1);
    let mut buf = [0u8; 127];
    let mut n = 0;
    while receive(&mut buf).is_some() {
        n += 1;
    }
    assert_eq!(n, SLOTS - 1);
    assert_eq!(counters().2 as usize, SLOTS - 1);
}

#[test]
fn an_ack_to_our_frame_is_seen() {
    let _s = sim::session();
    reset_radio_state();
    ACK_WANTED.set(1);
    ACK_SEQ.set(0x33);
    let ack = [0x0e, 0, 0, 0, 0x05, 0x02, 0x00, 0x33, 0x00, 0x00, 0, 0, 0, 0, 0, 0, 0x46, 0x10];
    deliver(&ack);
    assert_eq!(ACK_SEEN.get(), 1);
    assert!(!has_frame());
}

#[test]
fn energy_detection_and_noise_read_the_rssi_register() {
    let _s = sim::session();
    reset_radio_state();
    sim::with(|c| c.poke8(RF_RSSI, 30));
    assert_eq!(energy_detect(15, 1), -80);
    LAST_AIR.set(0);
    sim::with(|c| c.tick = 10_000 * 16);
    assert_eq!(noise_dbm(), Some(-80));
    LAST_AIR.set(sim::with(|c| c.tick));
    assert_eq!(noise_dbm(), None);
}

#[test]
fn suspend_and_resume_give_back_the_registers_the_ble_events_borrow() {
    let _s = sim::session();
    reset_radio_state();
    for (i, a) in LENT8.iter().enumerate() {
        sim::with(|c| c.poke8(*a as usize, 0x10 + i as u8));
    }
    let s = suspend();
    for a in LENT8 {
        sim::with(|c| c.poke8(a as usize, 0));
    }
    sim::with(|c| c.log.clear());
    resume(s);
    let writes = w8(&sim::with(|c| c.writes()));
    for (i, a) in LENT8.iter().enumerate() {
        let kept = if *a == 0x1245 { 0x25 } else { 0x10 + i as u8 };
        assert!(writes.contains(&(*a as usize, kept)), "{a:#x}");
    }
    assert_eq!(writes[..4], [(0x61, 0x01), (0x61, 0x00), (0x61, 0x01), (0x61, 0x00)]);
}

#[test]
fn a_frame_sent_at_a_given_tick_waits_for_it() {
    let _s = sim::session();
    reset_radio_state();
    let at = sim::with(|c| c.tick).wrapping_add(1_000 * 16);
    transmit_at(&[0x01, 0x08, 0x01, 0xff, 0xff], at);
    assert_eq!(sim::with(|c| c.tx_kicks), 1);
    assert!((sim::with(|c| c.tick).wrapping_sub(at) as i32) >= 0);
    transmit_at(&[0x01, 0x08, 0x01, 0xff, 0xff], at);
    assert_eq!(sim::with(|c| c.tx_kicks), 1);
}
