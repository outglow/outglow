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

pub(crate) fn reset_radio_state() {
    FREQ_OFFSET.set(5);
    HEAD.set(0);
    TAIL.set(0);
    ACK_WANTED.set(0);
    ACK_SEEN.set(0);
    PAN.set(0xffff);
    PROBE_PAN.set(0xffff);
    SHORT.set(0xffff);
    EXT.set([0; 8]);
    N_PENDING_SHORT.set(0);
    N_PENDING_EXT.set(0);
    N_CHILDREN.set(0);
    RADIO_DROPPED.set(0);
    CCA_BUSY.set(0);
    LAST_RX.set(0);
    LAST_AIR.set(0);
    RX_PEAK.set(0);
    TX_SENT.set(0);
    ACK_SEQ.set(0);
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
    assert_eq!(transmit(&mpdu, &mut || 0), TxStatus::Ok);
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
    assert_eq!(transmit(&mpdu, &mut || 0), TxStatus::NoAck);
    assert_eq!(sim::with(|c| c.tx_kicks), 4);
    assert_eq!(ACK_WANTED.get(), 0);
}

#[test]
fn a_busy_channel_ends_in_channel_busy_after_one_csma_ca_without_sending() {
    let _s = sim::session();
    reset_radio_state();
    sim::with(|c| c.poke8(RF_RSSI, 0x40));
    assert_eq!(transmit(&[0x41, 0x88, 0x01, 0x62, 0x1a, 0xff, 0xff, 0x00, 0x00], &mut || 7), TxStatus::ChannelBusy);
    assert_eq!(sim::with(|c| c.tx_kicks), 0);
    assert_eq!(CCA_BUSY.get(), MAX_CCA_ATTEMPTS);
    assert!(receiving());
}

#[test]
fn frames_too_short_or_too_long_are_not_sent() {
    let _s = sim::session();
    reset_radio_state();
    assert_eq!(transmit(&[1, 2], &mut no_backoff), TxStatus::Ok);
    assert_eq!(transmit(&[0; 126], &mut no_backoff), TxStatus::Ok);
    assert_eq!(sim::with(|c| c.tx_kicks), 0);
    assert_eq!(transmit(&[0x01, 0x08, 0x01], &mut no_backoff), TxStatus::Ok);
    assert_eq!(sim::with(|c| c.tx_kicks), 1);
}

fn no_backoff() -> u32 {
    0
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

fn frame(mpdu: &[u8], stamp: u32, rssi: u8, status: u8) -> Vec<u8> {
    let phr = mpdu.len() + 2;
    let mut v = vec![(phr + 9) as u8, 0, 0, 0, phr as u8];
    v.extend_from_slice(mpdu);
    v.extend_from_slice(&[0x04, 0x04]);
    v.extend_from_slice(&stamp.to_le_bytes());
    v.extend_from_slice(&[0, 0, rssi, status]);
    v
}

const OUR_PAN: u16 = 0x1a62;
const OUR_SHORT: u16 = 0x2b3c;

fn ours() {
    set_filter(OUR_PAN, OUR_SHORT, OUR_EXT);
    #[cfg(feature = "sense")]
    set_probe_pan(0x5e55);
}

fn data_to_short(fcf0: u8, pan: u16, dst: u16) -> Vec<u8> {
    let [p0, p1] = pan.to_le_bytes();
    let [d0, d1] = dst.to_le_bytes();
    vec![fcf0, 0x88, 0x42, p0, p1, d0, d1, 0x00, 0x00, 0xaa]
}

fn data_to_ext(fcf0: u8, pan: u16, dst: u64) -> Vec<u8> {
    let [p0, p1] = pan.to_le_bytes();
    let mut v = vec![fcf0, 0x8c, 0x42, p0, p1];
    v.extend_from_slice(&dst.to_le_bytes());
    v.extend_from_slice(&[0x00, 0x00]);
    v
}

fn heard(mpdu: &[u8]) -> (bool, usize) {
    let end_now = sim::with(|c| c.tick).wrapping_add(1_000 * 16);
    let stamp = end_now.wrapping_sub((mpdu.len() as u32 + 3) * 32 * 16);
    let kicks = sim::with(|c| c.tx_kicks);
    deliver(&frame(mpdu, stamp, 60, 0x10));
    let mut buf = [0u8; 127];
    let got = receive(&mut buf).is_some();
    (got, sim::with(|c| c.tx_kicks) - kicks)
}

#[test]
fn the_frame_filter_takes_what_is_for_us_and_acknowledges_what_asks_for_it() {
    let cases: Vec<(&str, Vec<u8>, bool, usize)> = vec![
        ("beacon", vec![0x00, 0x80, 0x01, 0x34, 0x12, 0x00, 0x00, 0xff, 0xcf, 0, 0], true, 0),
        ("data to us, ack", data_to_short(0x61, OUR_PAN, OUR_SHORT), true, 1),
        ("data to us, no ack", data_to_short(0x41, OUR_PAN, OUR_SHORT), true, 0),
        ("broadcast", data_to_short(0x61, OUR_PAN, 0xffff), true, 0),
        ("broadcast PAN", data_to_short(0x61, 0xffff, OUR_SHORT), true, 1),
        ("another short", data_to_short(0x61, OUR_PAN, OUR_SHORT ^ 1), false, 0),
        ("another short, high octet", data_to_short(0x61, OUR_PAN, OUR_SHORT ^ 0x100), false, 0),
        ("another PAN", data_to_short(0x61, OUR_PAN ^ 1, OUR_SHORT), false, 0),
        ("another PAN, high octet", data_to_short(0x61, OUR_PAN ^ 0x100, OUR_SHORT), false, 0),
        ("our long address", data_to_ext(0x61, OUR_PAN, OUR_EXT), true, 1),
        ("command to us", { let mut v = data_to_short(0x63, OUR_PAN, OUR_SHORT); v[1] = 0x88; v }, true, 1),
        ("ack frame type", vec![0x02, 0x00, 0x42], false, 0),
        ("reserved frame type 4", data_to_short(0x64, OUR_PAN, OUR_SHORT), false, 0),
        ("reserved frame type 7", data_to_short(0x67, OUR_PAN, OUR_SHORT), false, 0),
        ("no destination", vec![0x41, 0xc0, 0x42, 0x62, 0x1a, 0x01, 0x02, 0x03, 0x04, 0x05], false, 0),
        ("reserved destination mode", { let mut v = data_to_short(0x61, OUR_PAN, OUR_SHORT); v[1] = 0x84; v }, false, 0),
    ];
    for b in 0..8 {
        let _s = sim::session();
        reset_radio_state();
        ours();
        let mut e = data_to_ext(0x61, OUR_PAN, OUR_EXT);
        e[5 + b] ^= 0x01;
        assert_eq!(heard(&e), (false, 0), "long address octet {b}");
    }
    for (what, mpdu, taken, acks) in cases {
        let _s = sim::session();
        reset_radio_state();
        ours();
        assert_eq!(heard(&mpdu), (taken, acks), "{what}");
    }
}

#[test]
fn a_frame_shorter_than_its_addressing_is_dropped() {
    let _s = sim::session();
    reset_radio_state();
    set_filter(OUR_PAN, 0x0404, OUR_EXT);
    assert!(heard(&[0x41, 0x88, 0x42, 0x62, 0x1a, 0x04, 0x04]).0);
    assert!(!heard(&[0x41, 0x88, 0x42, 0x62, 0x1a, 0x04]).0);
    drop(_s);
    for (mpdu, taken) in [
        (vec![0x41, 0x88, 0x42, 0x62, 0x1a, 0x3c, 0x2b], true),
        (vec![0x41, 0x88, 0x42, 0x62, 0x1a, 0x3c], false),
        (data_to_ext(0x41, OUR_PAN, OUR_EXT)[..13].to_vec(), true),
        (data_to_ext(0x41, OUR_PAN, OUR_EXT)[..12].to_vec(), false),
    ] {
        let _s = sim::session();
        reset_radio_state();
        ours();
        assert_eq!(heard(&mpdu).0, taken, "{mpdu:x?}");
    }
}

#[cfg(feature = "sense")]
#[test]
fn a_probe_broadcast_on_the_probe_pan_is_taken() {
    for (dst, mode, taken) in [(0xffffu16, 0x88u8, true), (0xfffe, 0x88, false), (0xfeff, 0x88, false), (0xffff, 0x8c, false)] {
        let _s = sim::session();
        reset_radio_state();
        ours();
        let mut v = data_to_short(0x41, 0x5e55, dst);
        v[1] = mode;
        if mode == 0x8c {
            v = data_to_ext(0x41, 0x5e55, u64::MAX);
        }
        assert_eq!(heard(&v).0, taken, "{dst:#x} {mode:#x}");
    }
    let _s = sim::session();
    reset_radio_state();
    ours();
    assert!(!heard(&data_to_short(0x41, 0x5e56, 0xffff)).0);
}

#[test]
fn the_phy_header_must_hold_a_frame_and_match_the_dma_length() {
    for (phr, taken) in [(4usize, false), (5, true), (127, true), (128, false)] {
        let _s = sim::session();
        reset_radio_state();
        ours();
        let mut mpdu = vec![0u8; phr - 2];
        mpdu[0] = 0x00;
        let v = frame(&mpdu, 0, 60, 0x10);
        deliver(&v);
        assert_eq!(has_frame(), taken, "phr {phr}");
    }
}

#[test]
fn only_a_crc_status_with_bit_4_set_and_bits_0_and_6_clear_is_good() {
    for status in 0u8..=255 {
        let _s = sim::session();
        reset_radio_state();
        deliver(&frame(&[0x00, 0x80, 0x01, 0x34, 0x12], 0, 60, status));
        assert_eq!(has_frame(), status & 0x51 == 0x10, "{status:#x}");
    }
}

#[test]
fn a_queued_frame_keeps_its_length_rssi_end_and_octets() {
    let _s = sim::session();
    reset_radio_state();
    ours();
    let mpdu = data_to_short(0x41, OUR_PAN, OUR_SHORT);
    deliver(&frame(&mpdu, 0x1234_5678, 0x9c, 0x10));
    let mut buf = [0u8; 127];
    let (len, rssi, end) = receive(&mut buf).expect("queued");
    assert_eq!((len, rssi), (mpdu.len(), 0x9c - 110));
    assert_eq!(end, 0x1234_5678u32.wrapping_add((mpdu.len() as u32 + 3) * 32 * 16));
    assert_eq!(buf[..len], mpdu[..]);
}

#[test]
fn the_queue_wraps_around_its_sixteen_slots() {
    let _s = sim::session();
    reset_radio_state();
    let mut buf = [0u8; 127];
    for i in 0..40u8 {
        deliver(&frame(&[0x00, 0x80, i, 0x34, 0x12], 0, 60 + i, 0x10));
        if i % 3 != 2 {
            continue;
        }
        let mut n = 0;
        while let Some((_, rssi, _)) = receive(&mut buf) {
            assert_eq!(buf[2] as i32, rssi + 110 - 60);
            n += 1;
        }
        assert_eq!(n, 3);
    }
    assert_eq!(RADIO_DROPPED.get(), 0);
    assert_eq!(counters().2, 3);
    assert_eq!(HEAD.get() as usize, 40 % SLOTS);
    assert_eq!(TAIL.get() as usize, 39 % SLOTS);
}

#[test]
fn the_peak_of_waiting_frames_is_counted_across_the_wrap() {
    let _s = sim::session();
    reset_radio_state();
    RX_PEAK.set(0);
    HEAD.set(14);
    TAIL.set(14);
    for i in 0..5u8 {
        deliver(&frame(&[0x00, 0x80, i, 0x34, 0x12], 0, 60, 0x10));
    }
    let mut buf = [0u8; 127];
    assert!(receive(&mut buf).is_some());
    assert_eq!(counters().2, 5);
    assert!(receive(&mut buf).is_some());
    assert_eq!(counters().2, 5);
    for _ in 0..2 {
        deliver(&frame(&[0x00, 0x80, 9, 0x34, 0x12], 0, 60, 0x10));
    }
    assert!(receive(&mut buf).is_some());
    assert_eq!(counters().2, 5);
    for _ in 0..4 {
        deliver(&frame(&[0x00, 0x80, 9, 0x34, 0x12], 0, 60, 0x10));
    }
    assert!(receive(&mut buf).is_some());
    assert_eq!(counters().2, 8);
}

fn ack_at(late_ticks: i32) -> usize {
    let _s = sim::session();
    reset_radio_state();
    ours();
    let mpdu = data_to_short(0x61, OUR_PAN, OUR_SHORT);
    let stamp = 0x4000_0000u32;
    let end = stamp.wrapping_add((mpdu.len() as u32 + 3) * 32 * 16);
    sim::with(|c| c.tick = end.wrapping_add(late_ticks as u32).wrapping_sub(32));
    deliver(&frame(&mpdu, stamp, 60, 0x10));
    sim::with(|c| c.tx_kicks)
}

#[test]
fn a_frame_is_acknowledged_up_to_600_us_after_its_end() {
    assert_eq!(ack_at(600 * 16), 1);
    assert_eq!(ack_at(600 * 16 + 1), 0);
    assert_eq!(ack_at(-1000 * 16), 1);
}

#[test]
fn the_ack_starts_160_us_after_the_frame_and_waits_at_most_1_ms_for_its_transmission() {
    for completes in [true, false] {
        let _s = sim::session();
        reset_radio_state();
        ours();
        let mpdu = data_to_short(0x61, OUR_PAN, OUR_SHORT);
        let stamp = 0x4000_0000u32;
        let end = stamp.wrapping_add((mpdu.len() as u32 + 3) * 32 * 16);
        sim::with(|c| {
            c.tick = end.wrapping_sub(320);
            c.tx_completes = completes;
        });
        deliver(&frame(&mpdu, stamp, 60, 0x10));
        let log = sim::with(|c| c.log.clone());
        let kick = log.iter().position(|o| *o == Op::W8(0xc43, 0x04)).expect("the ACK is sent");
        let before: Vec<u32> = log[..kick].iter().filter_map(|o| match *o {
            Op::R32(0x740, t) => Some(t),
            _ => None,
        }).collect();
        assert_eq!(before.last().copied(), Some(end.wrapping_add(ACK_TURNAROUND_US * 16)));
        let done = kick + log[kick..].iter().position(|o| *o == Op::W8(RF_IRQ_STATUS, RF_IRQ_TX)).unwrap_or(0);
        let ticks: Vec<u32> = log[kick..done].iter().filter_map(|o| match *o {
            Op::R32(0x740, t) => Some(t),
            _ => None,
        }).collect();
        let waited = ticks.last().copied().unwrap_or(0).wrapping_sub(ticks[0]);
        assert_eq!(waited, if completes { 0 } else { 1000 * 16 + 16 }, "{completes}");
        let w = w8(&log[done..].iter().copied().filter(Op::is_write).collect::<Vec<_>>());
        assert_eq!(w[..8].to_vec(), [vec![(0xf20, 0x02)], channel_writes(0x95, 0x25, 0x14), vec![(0xf02, 0x65), (0x428, 0x01)]].concat());
    }
}

#[test]
fn the_ack_goes_before_the_frame_is_copied_to_the_queue_so_its_time_does_not_grow_with_the_length() {
    for n in [0usize, 100] {
        let _s = sim::session();
        reset_radio_state();
        ours();
        let mut mpdu = data_to_short(0x61, OUR_PAN, OUR_SHORT);
        mpdu.extend(std::iter::repeat(0x5a).take(n));
        let stamp = 0x4000_0000u32;
        let end = stamp.wrapping_add((mpdu.len() as u32 + 3) * 32 * 16);
        let queued_at_kick = std::rc::Rc::new(std::cell::Cell::new(None));
        let seen = queued_at_kick.clone();
        sim::with(|c| {
            c.tick = end.wrapping_sub(320);
            c.on(move |_, op| {
                if op == Op::W8(0xc43, 0x04) {
                    seen.set(Some(has_frame()));
                }
            });
        });
        deliver(&frame(&mpdu, stamp, 60, 0x10));
        assert_eq!(queued_at_kick.get(), Some(false), "{n}");
        let mut buf = [0u8; 127];
        let (len, _, _) = receive(&mut buf).expect("queued after the ACK");
        assert_eq!(buf[..len], mpdu[..]);
    }
}

fn data_request_from(fcf0: u8, fcf1: u8, src: &[u8], cmd: u8) -> Vec<u8> {
    let [p0, p1] = OUR_PAN.to_le_bytes();
    let [d0, d1] = OUR_SHORT.to_le_bytes();
    let mut v = vec![fcf0, fcf1, 0x07, p0, p1, d0, d1];
    if fcf0 & 0x40 == 0 {
        v.extend_from_slice(&[0x11, 0x22]);
    }
    v.extend_from_slice(src);
    v.push(cmd);
    v
}

fn pending_bit(mpdu: &[u8]) -> Option<bool> {
    let end = 0x4000_0000u32.wrapping_add((mpdu.len() as u32 + 3) * 32 * 16);
    sim::with(|c| c.tick = end.wrapping_sub(16));
    let kicks = sim::with(|c| c.tx_kicks);
    ACK_BUF.write(5, 0xee);
    deliver(&frame(mpdu, 0x4000_0000, 60, 0x10));
    let mut buf = [0u8; 127];
    let _ = receive(&mut buf);
    (sim::with(|c| c.tx_kicks) > kicks).then(|| ACK_BUF.read(5) == 0x12)
}

#[test]
fn a_data_request_from_a_long_address_is_told_of_its_pending_data() {
    let _s = sim::session();
    reset_radio_state();
    ours();
    let other: u64 = 0x0011_2233_4455_6677;
    let mine: u64 = 0x8899_aabb_ccdd_eeff;
    set_pending(&[], &[other, mine]);
    assert_eq!(pending_bit(&data_request_from(0x63, 0xc8, &mine.to_le_bytes(), 0x04)), Some(true));
    assert_eq!(pending_bit(&data_request_from(0x63, 0xc8, &other.to_le_bytes(), 0x04)), Some(true));
    for i in 0..8 {
        let mut a = mine.to_le_bytes();
        a[i] ^= 0x10;
        assert_eq!(pending_bit(&data_request_from(0x63, 0xc8, &a, 0x04)), Some(false), "octet {i}");
    }
    set_pending(&[], &[other]);
    assert_eq!(pending_bit(&data_request_from(0x63, 0xc8, &mine.to_le_bytes(), 0x04)), Some(false));
    set_pending(&[], &[]);
    assert_eq!(pending_bit(&data_request_from(0x63, 0xc8, &other.to_le_bytes(), 0x04)), Some(false));
}

#[test]
fn the_pending_bit_needs_a_data_request_after_a_source_address() {
    let _s = sim::session();
    reset_radio_state();
    ours();
    set_children(&[]);
    set_pending(&[0x1234], &[]);
    let src = 0x1234u16.to_le_bytes();
    assert_eq!(pending_bit(&data_request_from(0x63, 0x88, &src, 0x04)), Some(true));
    assert_eq!(pending_bit(&data_request_from(0x23, 0x88, &src, 0x04)), Some(true));
    assert_eq!(pending_bit(&data_request_from(0x63, 0x88, &src, 0x05)), Some(false));
    assert_eq!(pending_bit(&data_request_from(0x63, 0x48, &src, 0x04)), Some(false));
    assert_eq!(pending_bit(&data_request_from(0x63, 0x08, &[], 0x04)), Some(false));
    assert_eq!(pending_bit(&data_request_from(0x61, 0x88, &src, 0x04)), Some(false));
    let mut cut = data_request_from(0x63, 0x88, &src, 0x04);
    cut.pop();
    assert_eq!(pending_bit(&cut), Some(false));
    let unknown = 0x7777u16.to_le_bytes();
    assert_eq!(pending_bit(&data_request_from(0x63, 0x88, &unknown, 0x04)), Some(true));
    set_children(&[0x1111, 0x7777]);
    assert_eq!(pending_bit(&data_request_from(0x63, 0x88, &unknown, 0x04)), Some(false));
    assert_eq!(pending_bit(&data_request_from(0x63, 0x88, &0x7776u16.to_le_bytes(), 0x04)), Some(true));
    assert_eq!(pending_bit(&data_request_from(0x63, 0x88, &0x7677u16.to_le_bytes(), 0x04)), Some(true));
}

#[test]
fn the_pending_and_child_tables_keep_what_fits() {
    let _s = sim::session();
    reset_radio_state();
    ours();
    let shorts: Vec<u16> = (0..PENDING_SHORT_SLOTS as u16 + 3).map(|i| 0x100 + i).collect();
    let exts: Vec<u64> = (0..PENDING_EXT_SLOTS as u64 + 2).map(|i| 0x1000 + i).collect();
    set_pending(&shorts, &exts);
    assert_eq!(N_PENDING_SHORT.get() as usize, PENDING_SHORT_SLOTS);
    assert_eq!(N_PENDING_EXT.get() as usize, PENDING_EXT_SLOTS);
    assert_eq!(PENDING_SHORT.get().to_vec(), shorts[..PENDING_SHORT_SLOTS].to_vec());
    let last = PENDING_EXT_SLOTS - 1;
    assert_eq!(PENDING_EXT.get()[8 * last..8 * last + 8], (0x1000 + last as u64).to_le_bytes());
    let src = (0x100 + PENDING_SHORT_SLOTS as u16 - 1).to_le_bytes();
    set_children(&[u16::from_le_bytes(src)]);
    assert_eq!(pending_bit(&data_request_from(0x63, 0x88, &src, 0x04)), Some(true));
    let kids: Vec<u16> = (0..CHILD_SLOTS as u16 + 2).collect();
    set_children(&kids);
    assert_eq!(N_CHILDREN.get() as usize, CHILD_SLOTS);
    assert_eq!(CHILDREN.get().to_vec(), kids[..CHILD_SLOTS].to_vec());
    set_pending(&[1, 2], &[3]);
    assert_eq!(PENDING_SHORT.get()[2], 0xffff);
    assert_eq!(PENDING_EXT.get()[8..16], [0; 8]);
    assert_eq!((N_PENDING_SHORT.get(), N_PENDING_EXT.get()), (2, 1));
    set_children(&[5]);
    assert_eq!(CHILDREN.get()[1], 0xffff);
}

#[test]
fn a_frame_seen_while_still_receiving_is_left_for_later() {
    let _s = sim::session();
    reset_radio_state();
    RX_BUF.copy_from(0, &BEACON);
    sim::with(|c| {
        c.poke8(RF_IRQ_STATUS, RF_IRQ_RX);
        c.poke8(RF_RX_STATUS, 0x20);
        c.poke8(DMA_CHANNEL_ENABLE, 0x0c);
        c.log.clear();
    });
    irq_handler();
    assert!(!has_frame());
    assert_eq!(sim::with(|c| c.writes()), vec![Op::W8(0xf20, 0x01), Op::W8(0xc26, 0xfc)]);
    sim::with(|c| c.log.clear());
    irq_handler();
    assert_eq!(sim::with(|c| c.writes()), vec![Op::W8(0xc26, 0xfc)]);
}

#[test]
fn suspending_the_radio_first_takes_a_frame_that_has_arrived() {
    let _s = sim::session();
    reset_radio_state();
    RX_BUF.copy_from(0, &BEACON);
    sim::with(|c| {
        c.poke8(RF_IRQ_STATUS, RF_IRQ_RX);
        c.poke8(reg::IRQ_ENABLE, 1);
    });
    let s = suspend();
    assert_eq!(s, 1);
    assert!(has_frame());
    let w = sim::with(|c| c.writes());
    assert_eq!(w[0], Op::W8(reg::IRQ_ENABLE, 0));
    assert_eq!(w.iter().filter(|o| **o == Op::W8(0xf02, 0x45)).count(), 4);
}

fn segments(log: &[Op]) -> Vec<Vec<u32>> {
    let mut out = vec![vec![]];
    for o in log {
        match *o {
            Op::W8(0x643, 0) => out.push(vec![]),
            Op::R32(0x740, t) => {
                if let Some(s) = out.last_mut() {
                    s.push(t)
                }
            }
            _ => {}
        }
    }
    out
}

#[test]
fn the_backoff_doubles_from_8_to_32_slots_of_320_us() {
    let _s = sim::session();
    reset_radio_state();
    sim::with(|c| {
        c.poke8(RF_RSSI, 0x40);
        c.poke8(reg::IRQ_ENABLE, 1);
    });
    assert!(!csma_transmit(&mut || 0x1f));
    let segs = segments(&sim::with(|c| c.log.clone()));
    let spans: Vec<u32> = segs[..5].iter().map(|s| s.last().copied().unwrap_or(0).wrapping_sub(s[0])).collect();
    assert_eq!(spans, [7u32, 15, 31, 31, 31].iter().map(|n| n * 320 * 16 + 48).collect::<Vec<_>>());
    assert_eq!(CCA_BUSY.get(), 5);
}

#[test]
fn a_transmission_waits_for_1200_us_after_the_last_frame_heard() {
    let _s = sim::session();
    reset_radio_state();
    sim::with(|c| {
        c.poke8(RF_RSSI, 0x40);
        c.poke8(reg::IRQ_ENABLE, 1);
    });
    let t = sim::with(|c| c.tick);
    LAST_RX.set(t);
    assert!(!csma_transmit(&mut || 0));
    let segs = segments(&sim::with(|c| c.log.clone()));
    assert_eq!(segs[0].last().copied(), Some(t.wrapping_add(RX_GAP_US * 16)));
}

fn cca_with(rssi: &'static [u8]) -> bool {
    let mut i = 0;
    sim::with(|c| {
        c.poke8(RF_RSSI, rssi[0]);
        c.on(move |c, op| {
            if let Op::R8(RF_RSSI, _) = op {
                i = (i + 1).min(rssi.len() - 1);
                c.poke8(RF_RSSI, rssi[i]);
            }
        })
    });
    let r = channel_clear();
    sim::with(|c| c.hook = None);
    r
}

#[test]
fn the_channel_is_clear_when_the_mean_rssi_is_at_most_minus_85_dbm() {
    let _s = sim::session();
    reset_radio_state();
    assert!(cca_with(&[25]));
    assert!(!cca_with(&[26]));
    assert!(!cca_with(&[25, 25, 30]));
    assert!(cca_with(&[25, 30, 15]));
    assert!(!cca_with(&[25, 15, 35]));
    sim::with(|c| c.poke8(RF_RX_STATUS, 0x20));
    assert!(!cca_with(&[0]));
}

#[test]
fn the_clear_channel_window_is_128_us_from_before_the_receiver_turns_on() {
    for at in [128u32 * 16, 128 * 16 + 16] {
        let _s = sim::session();
        reset_radio_state();
        let mut start = None;
        sim::with(|c| {
            c.poke8(RF_RSSI, 25);
            c.on(move |c, op| {
                if let Op::R32(0x740, t) = op {
                    let s = *start.get_or_insert(t);
                    if t.wrapping_sub(s) == at {
                        c.poke8(RF_RSSI, 0x7f);
                    }
                }
            })
        });
        let clear = channel_clear();
        sim::with(|c| c.hook = None);
        assert_eq!(clear, at != 128 * 16, "{at}");
        let w = w8(&sim::with(|c| c.writes()));
        assert_eq!(w[..8].to_vec(), [channel_writes(0x95, 0x25, 0x14), vec![(0xf16, 0x29), (0x428, 0xe0), (0xf02, 0x45)]].concat());
    }
}

#[test]
fn a_sent_frame_waits_120_us_in_transmit_mode_and_at_most_10_ms_for_the_end() {
    for completes in [true, false] {
        let _s = sim::session();
        reset_radio_state();
        sim::with(|c| c.tx_completes = completes);
        assert!(csma_transmit(&mut || 0));
        let log = sim::with(|c| c.log.clone());
        let tx_mode = log.iter().position(|o| *o == Op::W8(0xf02, 0x55)).unwrap_or(0);
        let kick = log.iter().position(|o| *o == Op::W8(0xc43, 0x04)).unwrap_or(0);
        let settle: Vec<u32> = log[tx_mode..kick].iter().filter_map(|o| match *o {
            Op::R32(0x740, t) => Some(t),
            _ => None,
        }).collect();
        assert_eq!(settle.last().copied().unwrap_or(0).wrapping_sub(settle[0]), TX_SETTLE_US * 16 + 16);
        let done = kick + log[kick + 3..].iter().position(|o| *o == Op::W8(RF_IRQ_STATUS, RF_IRQ_TX)).unwrap_or(0) + 3;
        let waited: Vec<u32> = log[kick..done].iter().filter_map(|o| match *o {
            Op::R32(0x740, t) => Some(t),
            _ => None,
        }).collect();
        assert_eq!(waited.last().copied().unwrap_or(0).wrapping_sub(waited[0]), if completes { 0 } else { 10_000 * 16 + 16 });
        let after = w8(&log[done..].iter().copied().filter(Op::is_write).collect::<Vec<_>>());
        assert_eq!(after[..8].to_vec(), [vec![(0xf20, 0x02)], channel_writes(0x95, 0x25, 0x14), vec![(0xf02, 0x65), (0x428, 0xe1)]].concat());
        let air = log[done..].iter().rev().find_map(|o| match *o {
            Op::R32(0x740, t) => Some(t),
            _ => None,
        });
        assert_eq!(Some(LAST_AIR.get()), air);
        assert_eq!(*log.last().unwrap_or(&Op::R8(0, 0)), Op::W8(0x643, 0));
    }
}

fn ack_after(wait: u32) -> TxStatus {
    let _s = sim::session();
    reset_radio_state();
    let mut kicked = false;
    let mut armed = false;
    let mut t0 = None;
    sim::with(|c| {
        c.on(move |_, op| match op {
            Op::W8(0xc24, _) => kicked = true,
            Op::W8(0x643, v) if kicked && v != 0xff => armed = true,
            Op::R32(0x740, t) if armed => {
                let s = *t0.get_or_insert(t);
                if t.wrapping_sub(s) == wait {
                    ACK_SEEN.set(1);
                }
            }
            _ => {}
        })
    });
    let r = transmit(&[0x61, 0x88, 0x33, 0x62, 0x1a, 0x00, 0x00, 0x01, 0x00], &mut || 0);
    sim::with(|c| c.hook = None);
    r
}

#[test]
fn an_ack_is_awaited_for_2_ms_after_the_transmission() {
    assert_eq!(ack_after(ACK_WAIT_US * 16), TxStatus::Ok);
    assert_eq!(ACK_WANTED.get(), 0);
    let _s = sim::session();
    assert_eq!(sim::with(|c| c.tx_kicks), 0);
    drop(_s);
    assert_eq!(ack_after(ACK_WAIT_US * 16 + 16), TxStatus::NoAck);
}

#[test]
fn the_frame_and_its_ack_request_are_set_up_once_before_the_attempts() {
    let _s = sim::session();
    reset_radio_state();
    ACK_WANTED.set(0);
    let mut seen = vec![];
    let mpdu = [0x61, 0x88, 0x5a, 0x62, 0x1a, 0x00, 0x00, 0x01, 0x00];
    assert_eq!(transmit(&mpdu, &mut || 0), TxStatus::NoAck);
    seen.push(ACK_SEQ.get());
    assert_eq!(seen, vec![0x5a]);
    assert_eq!(counters().3, 4);
    assert_eq!(transmit(&[0x41, 0x88, 0x5b, 0x62, 0x1a, 0x00, 0x00, 0x01, 0x00], &mut || 0), TxStatus::Ok);
    assert_eq!(counters().3, 1);
    for (len, sent) in [(2usize, false), (3, true), (125, true), (126, false)] {
        let kicks = sim::with(|c| c.tx_kicks);
        let mut f = vec![0x41u8; len];
        f[0] = 0x01;
        assert_eq!(transmit(&f, &mut || 0), TxStatus::Ok);
        assert_eq!(sim::with(|c| c.tx_kicks) - kicks, sent as usize, "{len}");
        if sent {
            assert_eq!((TX_BUF.read32(0), TX_BUF.read(4), TX_BUF.read(4 + len)), (len as u32 + 1, len as u8 + 2, 0x41));
        }
    }
}

#[test]
fn a_frame_busy_on_its_last_attempt_ends_busy_and_one_sent_at_last_without_ack_ends_without() {
    for (busy_from, status) in [(3usize, TxStatus::ChannelBusy), (0, TxStatus::ChannelBusy), (9, TxStatus::NoAck)] {
        let _s = sim::session();
        reset_radio_state();
        let mut attempts = 0;
        sim::with(|c| {
            c.on(move |c, op| {
                if let Op::W8(0x643, 0) = op {
                    attempts += 1;
                    c.poke8(RF_RSSI, if attempts > busy_from * 1 { 0x40 } else { 0 });
                }
            })
        });
        let r = transmit(&[0x61, 0x88, 0x33, 0x62, 0x1a, 0x00, 0x00, 0x01, 0x00], &mut || 0);
        sim::with(|c| c.hook = None);
        assert_eq!(r, status, "{busy_from}");
    }
}

#[test]
fn a_timed_frame_goes_to_transmit_mode_240_us_ahead_and_starts_on_its_tick() {
    let _s = sim::session();
    reset_radio_state();
    let at = sim::with(|c| c.tick).wrapping_add(1_000 * 16);
    transmit_at(&[0x01, 0x08, 0x01], at);
    let log = sim::with(|c| c.log.clone());
    let off = log.iter().position(|o| *o == Op::W8(0x643, 0)).unwrap_or(0);
    assert_eq!(last_tick(&log[..off]), Some(at.wrapping_sub(2 * TX_SETTLE_US * 16)));
    let kick = log.iter().position(|o| *o == Op::W8(RF_IRQ_STATUS, RF_IRQ_TX)).unwrap_or(0);
    assert_eq!(last_tick(&log[..kick]), Some(at));
    assert_eq!((TX_BUF.read32(0), TX_BUF.read(4), TX_BUF.read(7)), (4, 5, 0x01));
    for (len, sent) in [(2usize, false), (3, true), (125, true), (126, false)] {
        let kicks = sim::with(|c| c.tx_kicks);
        let at = sim::with(|c| c.tick).wrapping_add(500 * 16);
        transmit_at(&vec![0x01; len], at);
        assert_eq!(sim::with(|c| c.tx_kicks) - kicks, sent as usize, "{len}");
    }
    for (ahead, sent) in [(16u32, true), (15, false)] {
        let kicks = sim::with(|c| c.tx_kicks);
        let at = sim::with(|c| c.tick).wrapping_add(ahead);
        transmit_at(&[0x01, 0x08, 0x01], at);
        assert_eq!(sim::with(|c| c.tx_kicks) - kicks, sent as usize, "{ahead}");
    }
}

#[test]
fn a_timed_frame_waits_at_most_10_ms_for_its_end_and_returns_to_receive() {
    let _s = sim::session();
    reset_radio_state();
    sim::with(|c| {
        c.tx_completes = false;
        c.poke8(reg::IRQ_ENABLE, 1);
    });
    let at = sim::with(|c| c.tick).wrapping_add(300 * 16);
    transmit_at(&[0x01, 0x08, 0x01], at);
    let log = sim::with(|c| c.log.clone());
    let kick = log.iter().position(|o| *o == Op::W8(0xc43, 0x04)).unwrap_or(0);
    let done = kick + log[kick..].iter().position(|o| *o == Op::W8(RF_IRQ_STATUS, RF_IRQ_TX)).unwrap_or(0);
    let ticks: Vec<u32> = log[kick..done].iter().filter_map(|o| match *o {
        Op::R32(0x740, t) => Some(t),
        _ => None,
    }).collect();
    assert_eq!(ticks.last().copied().unwrap_or(0).wrapping_sub(ticks[0]), 10_000 * 16 + 16);
    let w = w8(&log[done..].iter().copied().filter(Op::is_write).collect::<Vec<_>>());
    assert_eq!(w, [vec![(0xf20, 0x02)], channel_writes(0x95, 0x25, 0x14), vec![(0xf02, 0x65), (0x428, 0x01), (0x643, 0x01)]].concat());
}

fn last_tick(log: &[Op]) -> Option<u32> {
    log.iter().rev().find_map(|o| match *o {
        Op::R32(0x740, t) => Some(t),
        _ => None,
    })
}

#[test]
fn the_noise_floor_is_measured_only_after_3_ms_without_our_transmission_or_a_frame() {
    let _s = sim::session();
    reset_radio_state();
    sim::with(|c| {
        c.poke8(RF_RSSI, 30);
        c.poke8(reg::IRQ_ENABLE, 1);
    });
    let t = sim::with(|c| c.tick);
    LAST_AIR.set(t.wrapping_add(16).wrapping_sub(3_000 * 16));
    assert_eq!(noise_dbm(), Some(-80));
    let w = sim::with(|c| c.writes());
    assert_eq!(w.first(), Some(&Op::W8(0x643, 0)));
    assert_eq!(w.last(), Some(&Op::W8(0x643, 1)));
    let t = sim::with(|c| c.tick);
    LAST_AIR.set(t.wrapping_add(17).wrapping_sub(3_000 * 16));
    sim::with(|c| c.log.clear());
    assert_eq!(noise_dbm(), None);
    assert!(sim::with(|c| c.writes()).is_empty());
    LAST_AIR.set(0);
    sim::with(|c| {
        c.poke8(RF_RX_STATUS, 0x20);
        c.tick = 10_000 * 16;
        c.log.clear();
    });
    assert_eq!(noise_dbm(), None);
    assert_eq!(sim::with(|c| c.writes()), vec![Op::W8(0x643, 0), Op::W8(0x643, 1)]);
}

#[test]
fn the_noise_floor_is_the_mean_of_the_samples_in_128_us() {
    let _s = sim::session();
    reset_radio_state();
    let mut first = None;
    sim::with(|c| {
        c.poke8(RF_RSSI, 10);
        c.on(move |c, op| match op {
            Op::R32(0x740, t) => {
                let s = *first.get_or_insert(t);
                c.poke8(RF_RSSI, if t.wrapping_sub(s) >= 300 * 16 { 50 } else { 10 });
            }
            _ => {}
        })
    });
    LAST_AIR.set(0);
    sim::with(|c| c.tick = 10_000 * 16);
    let n = noise_dbm();
    sim::with(|c| c.hook = None);
    let log = sim::with(|c| c.log.clone());
    let samples: Vec<i32> = log.iter().filter_map(|o| match *o {
        Op::R8(RF_RSSI, v) => Some(v as i8 as i32 - 110),
        _ => None,
    }).collect();
    assert_eq!(n, Some(samples.iter().sum::<i32>() / samples.len() as i32));
    assert!(samples.len() > 10);
    let w = w8(&sim::with(|c| c.writes()));
    assert_eq!(w[1..9].to_vec(), [channel_writes(0x95, 0x25, 0x14), vec![(0xf16, 0x29), (0x428, 0xe0), (0xf02, 0x45)]].concat());
}

#[test]
fn energy_detection_keeps_the_peak_over_its_milliseconds() {
    let _s = sim::session();
    reset_radio_state();
    let mut first = None;
    sim::with(|c| {
        c.poke8(RF_RSSI, 10);
        c.on(move |c, op| {
            if let Op::R32(0x740, t) = op {
                let s = *first.get_or_insert(t);
                c.poke8(RF_RSSI, if t.wrapping_sub(s) == (85 + 2_000) * 16 + 32 { 70 } else { 10 });
            }
        })
    });
    assert_eq!(energy_detect(20, 2), -40);
    sim::with(|c| c.hook = None);
    let w = w8(&sim::with(|c| c.writes()));
    assert!(w.contains(&(0x1244, 0x49)));
    let mut first = None;
    sim::with(|c| {
        c.log.clear();
        c.on(move |c, op| {
            if let Op::R32(0x740, t) = op {
                let s = *first.get_or_insert(t);
                c.poke8(RF_RSSI, if t.wrapping_sub(s) == (85 + 2_000) * 16 + 48 { 70 } else { 10 });
            }
        })
    });
    assert_eq!(energy_detect(20, 2), -100);
    sim::with(|c| c.hook = None);
}

pub fn tables() -> (Vec<u16>, Vec<u64>, Vec<u16>) {
    let ns = crate::radio::N_PENDING_SHORT.get() as usize;
    let ne = crate::radio::N_PENDING_EXT.get() as usize;
    let nc = crate::radio::N_CHILDREN.get() as usize;
    let ext = crate::radio::PENDING_EXT.get();
    (
        crate::radio::PENDING_SHORT.get()[..ns].to_vec(),
        ext.chunks(8).take(ne).map(|c| u64::from_le_bytes(c.try_into().unwrap_or([0; 8]))).collect(),
        crate::radio::CHILDREN.get()[..nc].to_vec(),
    )
}

pub fn drain() {
    crate::radio::TAIL.set(crate::radio::HEAD.get());
}

pub fn queue(mpdu: &[u8], rssi: u8, end: u32) {
    let head = crate::radio::HEAD.get() as usize;
    let slot = head * crate::radio::SLOT_LEN;
    crate::radio::QUEUE.write(slot, mpdu.len() as u8);
    crate::radio::QUEUE.write(slot + 1, rssi);
    crate::radio::QUEUE.copy_from(slot + 2, &end.to_le_bytes());
    crate::radio::QUEUE.copy_from(slot + 6, mpdu);
    crate::radio::HEAD.set(((head + 1) % crate::radio::SLOTS) as u8);
}

pub fn forget_rx() {
    crate::radio::LAST_RX.set(0);
}

pub fn heard_now() {
    crate::radio::LAST_RX.set(crate::reg::tick());
}

pub fn heard_at(t: u32) {
    crate::radio::LAST_RX.set(t);
}

pub fn sent_at(t: u32) {
    crate::radio::LAST_AIR.set(t);
}

#[cfg(feature = "sense")]
pub fn probe_pan() -> u16 {
    crate::radio::PROBE_PAN.get()
}

#[test]
fn resuming_802_15_4_restores_its_tables_dma_and_interrupts_and_turns_the_receiver_on() {
    let _s = sim::session();
    reset_radio_state();
    sim::with(|c| {
        c.poke32(reg::IRQ_MASK, 0x0010_0001);
        c.poke8(reg::IRQ_ENABLE, 1);
        c.poke8(RF_IRQ_MASK, 0xff);
        c.poke8(RF_IRQ_MASK + 1, 0xff);
        c.poke8(DMA_IRQ_ENABLE, 0xff);
        c.poke8(DMA_CHANNEL_ENABLE, 0x00);
        c.poke8(0x1225, 0xff);
        c.poke8(0x1226, 0x00);
        c.poke8(0x1227, 0xff);
    });
    let s = suspend();
    sim::with(|c| c.log.clear());
    resume(s);
    let w = sim::with(|c| c.writes());
    let table = w.iter().position(|o| *o == Op::W8(0x12d2, 0x9b)).unwrap_or(0);
    assert_eq!(w[table - 1], Op::W16(RF_IRQ_STATUS as usize, 0xffff));
    assert_eq!(w8(&w[table..table + 35]), VENDOR_INIT_TABLE.to_vec());
    let at = w.iter().rposition(|o| matches!(o, Op::W8(0x1225, _))).unwrap_or(0);
    let tail = &w[at..];
    assert_eq!(
        tail[..4].to_vec(),
        vec![Op::W8(0x1225, 0xbf), Op::W8(0x1226, 0x80), Op::W8(0x1227, 0xff), Op::W16(DMA_RX_ADDR, RX_BUF.addr() as u16)]
    );
    assert_eq!(
        tail[4..11].to_vec(),
        vec![
            Op::W8(DMA_RX_SIZE, 0x09),
            Op::W8(DMA_RX_MODE, 0x01),
            Op::W8(DMA_IRQ_ENABLE, 0xf3),
            Op::W16(RF_IRQ_MASK, 0xe001),
            Op::W32(reg::IRQ_MASK, 0x0010_2001),
            Op::W8(DMA_CHANNEL_ENABLE, 0x0c),
            Op::W8(0xf02, 0x45),
        ]
    );
    assert_eq!(*w.last().unwrap_or(&Op::W8(0, 0)), Op::W8(reg::IRQ_ENABLE, 1));
}

#[test]
fn init_keeps_the_other_interrupt_mask_bits() {
    let _s = sim::session();
    reset_radio_state();
    sim::with(|c| {
        c.poke32(reg::IRQ_MASK, 0x0010_0001);
        c.poke8(RF_IRQ_MASK, 0xff);
        c.poke8(RF_IRQ_MASK + 1, 0xff);
    });
    init();
    let w = sim::with(|c| c.writes());
    assert!(w.contains(&Op::W16(RF_IRQ_MASK, 0xe001)));
    assert!(w.contains(&Op::W32(reg::IRQ_MASK, 0x0010_2001)));
}

#[test]
fn only_an_ack_of_five_octets_with_our_sequence_number_while_we_wait_counts() {
    let ack = |phr: u8, seq: u8| {
        let mut v = vec![phr + 9, 0, 0, 0, phr, 0x02, 0x00, seq];
        v.resize(phr as usize + 5, 0);
        v.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0x46, 0x10]);
        v
    };
    for (wanted, phr, seq, seen) in [(1u8, 5u8, 0x33u8, 1u8), (0, 5, 0x33, 0), (1, 6, 0x33, 0), (1, 5, 0x34, 0)] {
        let _s = sim::session();
        reset_radio_state();
        ACK_WANTED.set(wanted);
        ACK_SEQ.set(0x33);
        ACK_SEEN.set(0);
        deliver(&ack(phr, seq));
        assert_eq!(ACK_SEEN.get(), seen, "{wanted} {phr} {seq:#x}");
        assert!(!has_frame());
    }
}

#[cfg(feature = "trace")]
#[test]
fn the_trace_of_a_transmission_counts_its_attempts_and_the_busy_clear_channel_checks() {
    let _s = sim::session();
    reset_radio_state();
    sim::with(|c| c.poke8(reg::IRQ_ENABLE, 1));
    let mpdu = [0x41, 0x88, 0x5c, 0x62, 0x1a, 0x34, 0x12, 0x00, 0x00];
    assert_eq!(transmit(&mpdu, &mut || 0), TxStatus::Ok);
    assert_eq!(LAST_TX.get()[..2], [1, 0]);
    let mut checks = 0;
    sim::with(|c| {
        c.on(move |c, op| {
            if let Op::W8(0x643, 0) = op {
                checks += 1;
                c.poke8(RF_RSSI, if checks <= 2 { 0x40 } else { 0 });
            }
        })
    });
    assert_eq!(transmit(&mpdu, &mut || 0), TxStatus::Ok);
    sim::with(|c| c.hook = None);
    assert_eq!(LAST_TX.get()[..2], [1, 2]);
    sim::with(|c| c.poke8(RF_RSSI, 0x40));
    crate::trace::tests::rewind();
    assert_eq!(transmit(&mpdu, &mut || 0), TxStatus::ChannelBusy);
    assert_eq!(LAST_TX.get()[..2], [1, MAX_CCA_ATTEMPTS]);
    let e = crate::trace::tests::entry(0);
    assert_eq!(e[1] & 0x00ff_ffff, 0x005c_0000 | (TxStatus::ChannelBusy as u32) << 8 | crate::trace::MAC as u32);
    assert_eq!(e[2], 0x1234_0000 | mpdu.len() as u32);
    assert_eq!(e[3] & 0xffff, 1 | 5 << 8);
}

pub fn filter() -> (u16, u16, u64) {
    (crate::radio::PAN.get(), crate::radio::SHORT.get(), u64::from_le_bytes(crate::radio::EXT.get()))
}

pub fn set_counters(busy: u32, dropped: u32, peak: u8, sent: u8) {
    crate::radio::CCA_BUSY.set(busy);
    crate::radio::RADIO_DROPPED.set(dropped);
    crate::radio::RX_PEAK.set(peak);
    crate::radio::TX_SENT.set(sent);
}

#[test]
fn traffic_every_millisecond_does_not_hold_a_transmission_past_the_watchdog() {
    let _s = sim::session();
    reset_radio_state();
    let start = sim::with(|c| c.tick);
    LAST_RX.set(start);
    let mut last = start;
    sim::with(|c| {
        c.tick_step = 50 * 16;
        c.poke8(RF_RSSI, 0x00);
        c.poke8(reg::IRQ_ENABLE, 1);
        c.on(move |_, op| {
            if let Op::R32(0x740, t) = op {
                if t.wrapping_sub(start) < 3_000_000 * 16 && t.wrapping_sub(last) >= 1_000 * 16 {
                    last = t;
                    LAST_RX.set(t);
                }
            }
        })
    });
    assert!(!csma_transmit(&mut || 0));
    let spent_ms = sim::with(|c| c.tick).wrapping_sub(start) / 16_000;
    assert!(spent_ms < 2_000, "csma_transmit spent {spent_ms} ms with one frame heard per ms");
    assert_eq!(CCA_BUSY.get(), MAX_CCA_ATTEMPTS);
    assert_eq!(TX_SENT.get(), 0);
}

#[test]
fn the_wait_for_a_quiet_gap_gives_up_after_4800_us() {
    let _s = sim::session();
    reset_radio_state();
    let start = sim::with(|c| c.tick);
    sim::with(|c| {
        c.on(move |_, op| {
            if let Op::R32(0x740, t) = op {
                LAST_RX.set(t);
            }
        })
    });
    assert!(!rx_gap_heard());
    let first = start.wrapping_add(16);
    assert_eq!(sim::with(|c| c.tick).wrapping_sub(first), RX_GAP_WAIT_US * 16 + 32);
}

#[test]
fn a_quiet_gap_reached_before_the_limit_is_heard() {
    let _s = sim::session();
    reset_radio_state();
    let start = sim::with(|c| c.tick);
    LAST_RX.set(start.wrapping_add(RX_GAP_WAIT_US * 16 - RX_GAP_US * 16));
    assert!(rx_gap_heard());
}
