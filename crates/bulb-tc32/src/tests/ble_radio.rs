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
    fresh_link();
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
    crate::radio::tests::heard_now();
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

const AA: u32 = 0x7176_4129;
const CRC_INIT: u32 = 0x33_4455;

const TRACED_VERSION_IND: [u8; 23] =
    [0x13, 0, 0, 0, 0x13, 0x06, 0x0c, 0x0b, 0x0f, 0x00, 0x34, 0x12, 0x73, 0xbb, 0x66, 0xf8, 0x0f, 0x19, 0x08, 0, 0, 0xc8, 0];
const TRACED_FEATURE_REQ: [u8; 26] = [
    0x16, 0, 0, 0, 0x0f, 0x09, 0x08, 0x01, 0, 0, 0, 0, 0, 0, 0, 0xd7, 0x05, 0x7b, 0xb8, 0x2f, 0x19, 0x08, 0, 0, 0xc8, 0,
];
const TRACED_READ_REQ: [u8; 24] =
    [0x14, 0, 0, 0, 0x0e, 0x07, 0x03, 0x00, 0x04, 0x00, 0x0a, 0x03, 0x00, 0x68, 0xe5, 0xc4, 0xf8, 0xec, 0x86, 0x08, 0, 0, 0xc8, 0];
const TRACED_EMPTY: [u8; 17] = [0x0d, 0, 0, 0, 0x01, 0x00, 0xbb, 0x1c, 0x82, 0xf9, 0x3f, 0x8e, 0x08, 0, 0, 0xc8, 0];

fn bit_reverse24(v: u32) -> u32 {
    (0..24).fold(0, |r, i| r | ((v >> i) & 1) << (23 - i))
}

fn bit_crc(init: u32, data: &[u8]) -> u32 {
    let mut c = bit_reverse24(init);
    for &d in data {
        let mut d = d;
        for _ in 0..8 {
            let feedback = (c ^ d as u32) & 1;
            c >>= 1;
            d >>= 1;
            if feedback != 0 {
                c ^= 0xda_6000;
            }
        }
    }
    c
}

fn rx_packet(header: u8, payload: &[u8]) -> Vec<u8> {
    let n = payload.len();
    let mut v = vec![(n + 13) as u8, 0, 0, 0, header, n as u8];
    v.extend_from_slice(payload);
    let crc = bit_crc(CRC_INIT, &v[4..6 + n]);
    v.extend_from_slice(&crc.to_le_bytes()[..3]);
    v.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0xc8, 0]);
    v
}

#[test]
fn the_reference_crc_gives_the_traced_packets_crc() {
    for p in [&TRACED_VERSION_IND[..], &TRACED_FEATURE_REQ[..], &TRACED_READ_REQ[..], &TRACED_EMPTY[..]] {
        let n = p[5] as usize;
        assert_eq!(rx_packet(p[4], &p[6..6 + n])[..9 + n], p[..9 + n]);
    }
    for v in [0x33_4455, 0x55_5555, 0x12_3456, 0x80_0001] {
        assert_eq!(reverse24(v), bit_reverse24(v));
    }
}

#[derive(Clone)]
enum Step {
    Rx { bytes: Vec<u8>, sn: u8, nesn: u8, stamp_ago: u32, rptr: Option<u8> },
    Status(u16),
}

fn rx(bytes: &[u8], sn: u8, nesn: u8) -> Step {
    Step::Rx { bytes: bytes.to_vec(), sn, nesn, stamp_ago: 80 * TICKS_PER_US, rptr: None }
}

fn buffer_at(dma: u16) -> &'static SharedBuf<RX_LEN> {
    RX.iter().find(|b| b.addr() as u16 == dma).expect("the DMA points at an RX buffer")
}

fn central(steps: Vec<Step>) -> impl FnMut(&mut sim::Chip, Op) {
    let mut steps = std::collections::VecDeque::from(steps);
    let mut armed = false;
    let mut dma = 0u16;
    move |c, op| match op {
        Op::W16(DMA_RX_ADDR, a) => dma = a,
        Op::W8(RF_CMD, 0x82) => armed = true,
        Op::R16(RF_IRQ_STATUS, s) if armed && s & (STATUS_RX | STATUS_TX) == 0 => match steps.pop_front() {
            Some(Step::Rx { bytes, sn, nesn, stamp_ago, rptr }) => {
                buffer_at(dma).copy_from(0, &bytes);
                c.poke8(RF_SN, sn);
                c.poke8(RF_NESN, nesn << 4);
                c.poke32(RF_TIMESTAMP, c.tick.wrapping_sub(stamp_ago));
                if let Some(r) = rptr {
                    c.poke8(TX_FIFO_RPTR, r);
                }
                c.raise16(RF_IRQ_STATUS, STATUS_RX);
            }
            Some(Step::Status(bits)) => c.raise16(RF_IRQ_STATUS, bits),
            None => {}
        },
        _ => {}
    }
}

pub(crate) fn reset_statics() {
    connection_reset();
    RX_CURRENT.set(0);
    FIFO_STARTED.set(0);
    FIFO_RPTR.set(0);
    FIFO_FREED.set(0);
    FIFO_WORD.set(0);
    FIFO_KIND.set([0; 16]);
    TX_QUEUE.copy_from(0, &[0; TX_SLOTS * TX_SLOT]);
    RX_QUEUE.copy_from(0, &[0; RX_SLOTS * RX_SLOT]);
}

pub(crate) fn fresh_link() {
    crate::radio::tests::forget_rx();
    reset_statics();
    sim::with(|c| c.tick = 0x2000_0000);
}

fn event_now(channel: u8, window_us: u32, deadline_us: u32) -> ConnEvent {
    let start = sim::with(|c| c.tick).wrapping_add(800 * TICKS_PER_US);
    ConnEvent { channel, access_address: AA, crc_init: CRC_INIT, start, window: window_us * TICKS_PER_US, deadline: start.wrapping_add(deadline_us * TICKS_PER_US) }
}

fn run_event(ev: &ConnEvent, steps: Vec<Step>) -> EventResult {
    sim::with(|c| {
        c.log.clear();
        c.on(central(steps));
    });
    let r = connection_event(ev);
    sim::with(|c| c.hook = None);
    r
}

fn writes_after(log: &[Op], first: Op) -> Vec<Op> {
    let at = log.iter().position(|o| *o == first).unwrap_or_else(|| panic!("{first:?} not written"));
    log[at..].iter().copied().filter(Op::is_write).collect()
}

fn rx_dma() -> Vec<u16> {
    let ours = [RX[0].addr() as u16, RX[1].addr() as u16];
    sim::with(|c| c.writes())
        .into_iter()
        .filter_map(|o| match o {
            Op::W16(DMA_RX_ADDR, a) if ours.contains(&a) => Some(a),
            _ => None,
        })
        .collect()
}

fn pop_all() -> Vec<(u8, Vec<u8>)> {
    let mut out = vec![];
    let mut p = [0u8; LL_PAYLOAD_MAX];
    while let Some((llid, n)) = rx_pop(&mut p) {
        out.push((llid, p[..n].to_vec()));
    }
    out
}

#[test]
fn a_connection_event_programs_the_radio_as_the_vendor_trace() {
    let _s = sim::session();
    fresh_link();
    let ev = event_now(7, 2_500, 30_000);
    run_event(&ev, vec![Step::Status(0x0020)]);
    let log = sim::with(|c| c.log.clone());
    let w = writes_after(&log, Op::W8(RF_SN_RESET, 0x01));
    let at = log.iter().position(|o| *o == Op::W8(RF_SN_RESET, 0x01)).unwrap_or(0);
    let before: Vec<Op> = log[..at].iter().copied().filter(Op::is_write).collect();
    assert_eq!(before[before.len() - 4..].to_vec(), vec![Op::W8(0xf16, 0x29), Op::W8(0x428, 0x80), Op::W8(0xf02, 0x45), Op::W8(0xf00, 0x80)]);
    let now = log[..at].iter().rev().find_map(|o| match *o {
        Op::R32(0x740, t) => Some(t),
        _ => None,
    });
    let now = now.unwrap_or(0);
    let fsm = ev.deadline.wrapping_sub(now) / TICKS_PER_US + 1000;
    assert_eq!(
        w[..31].to_vec(),
        vec![
            Op::W8(0xf01, 0x01),
            Op::W16(0xf1c, 0x0477),
            Op::W16(0xf20, 0xffff),
            Op::W8(0x61, 0x01),
            Op::W8(0x61, 0x00),
            Op::W8(0x40d, 0x07),
            Op::W8(0x1244, 0x20),
            Op::W8(0x1245, 0x26),
            Op::W8(0x1228, 0x08),
            Op::W8(0x1229, 0x14),
            Op::W32(0x408, 0x2941_7671),
            Op::W8(0x405, 0x84),
            Op::W32(0x424, 0x0033_4455),
            Op::W8(0xf03, 0x00),
            Op::W8(0xf03, 0x10),
            Op::W8(0xc2a, 0x40),
            Op::W16(0xf04, 0x004b),
            Op::W16(0xf0a, 0x00fa),
            Op::W8(0xf03, 0x11),
            Op::W32(0xf2c, fsm),
            Op::W32(0xf28, 0x0fff_ffff),
            Op::W16(0xc08, RX[0].addr() as u16),
            Op::W32(0xf18, ev.start.wrapping_sub(RX_LEAD)),
            Op::W8(0xf16, 0x2d),
            Op::W8(0xf00, 0x82),
            Op::W16(0xc0c, EMPTY.addr() as u16),
            Op::W8(0xf00, 0x80),
            Op::W16(0xf20, 0xffff),
            Op::W8(0xf16, 0x29),
            Op::W8(0x428, 0x80),
            Op::W8(0xf02, 0x45),
        ]
    );
    assert_eq!(fsm, ev.deadline.wrapping_sub(now) / 16 + 1000);
    assert!(fsm > 30_000 && fsm < 30_000 + 1000 + 800);
}

fn counter(i: usize) -> u32 {
    BLE_RADIO.at(i).unwrap_or(0)
}

#[test]
fn the_traced_first_event_queues_both_packets_saves_the_sequence_bits_and_finds_the_anchor() {
    let _s = sim::session();
    fresh_link();
    let (events, heard) = (counter(CONN_EVENTS), counter(HEARD));
    let ev = event_now(7, 2_500, 30_000);
    let stamp = std::rc::Rc::new(std::cell::Cell::new(0u32));
    let seen = stamp.clone();
    sim::with(|c| c.log.clear());
    let mut air = central(vec![rx(&TRACED_VERSION_IND, 1, 1), rx(&TRACED_FEATURE_REQ, 0, 0), Step::Status(0x0020)]);
    sim::with(|c| {
        c.on(move |c, op| {
            air(c, op);
            if seen.get() == 0 && c.peek8(RF_TIMESTAMP + 3) != 0 {
                seen.set(u32::from_le_bytes([c.peek8(0x450), c.peek8(0x451), c.peek8(0x452), c.peek8(0x453)]));
            }
        })
    });
    let r = connection_event(&ev);
    sim::with(|c| c.hook = None);
    assert_eq!(r.anchor, Some(stamp.get().wrapping_sub(0x500)));
    assert_eq!(pop_all(), vec![(3, TRACED_VERSION_IND[6..12].to_vec()), (3, TRACED_FEATURE_REQ[6..15].to_vec())]);
    assert_eq!(SN_NESN.get(), 0x00);
    assert_eq!(RX_CURRENT.get(), 0);
    assert_eq!((counter(CONN_EVENTS), counter(HEARD)), (events + 1, heard + 1));
    let (a, b) = (RX[0].addr() as u16, RX[1].addr() as u16);
    assert_eq!(rx_dma(), vec![a, b, a]);
}

#[test]
fn an_event_starting_on_the_odd_buffer_receives_into_it_first() {
    let _s = sim::session();
    fresh_link();
    RX_CURRENT.set(1);
    let ev = event_now(7, 2_500, 30_000);
    run_event(&ev, vec![rx(&TRACED_VERSION_IND, 1, 1), Step::Status(0x0020)]);
    assert_eq!(RX_CURRENT.get(), 0);
    assert_eq!(pop_all().len(), 1);
    assert_eq!(rx_dma(), vec![RX[1].addr() as u16, RX[0].addr() as u16]);
    assert_eq!(sim::with(|c| c.writes()).iter().filter(|o| **o == Op::W16(0xf20, STATUS_RX)).count(), 1);
}

#[test]
fn the_sequence_bits_are_read_from_their_own_register_bits() {
    for (sn, nesn, saved) in [(1u8, 0u8, 0x10u8), (0, 1, 0x20), (1, 1, 0x30), (0xfe, 0x0e, 0x00), (0xff, 0xef, 0x30)] {
        let _s = sim::session();
        fresh_link();
        let ev = event_now(7, 2_500, 30_000);
        sim::with(|c| c.log.clear());
        let mut air = central(vec![Step::Rx { bytes: TRACED_EMPTY.to_vec(), sn: 0, nesn: 0, stamp_ago: 80 * 16, rptr: None }, Step::Status(0x0020)]);
        sim::with(|c| {
            c.on(move |c, op| {
                air(c, op);
                if let Op::W16(RF_IRQ_STATUS, STATUS_RX) = op {
                    c.poke8(RF_SN, sn);
                    c.poke8(RF_NESN, nesn << 4 | nesn >> 4);
                }
            })
        });
        connection_event(&ev);
        sim::with(|c| c.hook = None);
        assert_eq!(SN_NESN.get(), saved, "sn {sn:#x} nesn {nesn:#x}");
    }
}

fn corrupt(p: &[u8], at: usize) -> Vec<u8> {
    let mut v = p.to_vec();
    v[at] ^= 0x01;
    v
}

#[test]
fn a_packet_whose_software_crc_fails_ends_the_event_unanswered() {
    let n = TRACED_VERSION_IND[5] as usize;
    for at in [4, 5 + n, 6 + n, 7 + n, 8 + n] {
        let _s = sim::session();
        fresh_link();
        SN_NESN.set(0x10);
        let errors = counter(CRC_ERRORS);
        let bad = if at == 5 + n { corrupt(&TRACED_VERSION_IND, 11) } else { corrupt(&TRACED_VERSION_IND, at) };
        let ev = event_now(7, 2_500, 30_000);
        let r = run_event(&ev, vec![rx(&bad, 0, 1), rx(&TRACED_FEATURE_REQ, 0, 0), Step::Status(0x0020)]);
        assert_eq!(r.anchor, None, "byte {at}");
        assert_eq!(counter(CRC_ERRORS), errors + 1);
        assert!(pop_all().is_empty());
        assert_eq!(SN_NESN.get(), 0x10);
        let w = writes_after(&sim::with(|c| c.log.clone()), Op::W16(0xf20, STATUS_RX));
        assert_eq!(w[..4].to_vec(), vec![Op::W16(0xf20, STATUS_RX), Op::W8(0xf00, 0x80), Op::W8(0xf00, 0x80), Op::W16(0xf20, 0xffff)]);
    }
}

#[test]
fn a_malformed_packet_is_skipped_and_takes_the_first_place() {
    let mut long = rx_packet(0x02, &[0; 48]);
    long.truncate(64);
    let mut short_dma = TRACED_VERSION_IND.to_vec();
    short_dma[0] = 0x12;
    let mut crc_flag = TRACED_VERSION_IND.to_vec();
    crc_flag[22] = 0x01;
    let mut long_dma = TRACED_VERSION_IND.to_vec();
    long_dma[0] = 0x14;
    for bad in [short_dma, long_dma, crc_flag, long] {
        let _s = sim::session();
        fresh_link();
        let errors = counter(CRC_ERRORS);
        let ev = event_now(7, 2_500, 30_000);
        let r = run_event(&ev, vec![rx(&bad, 0, 0), rx(&TRACED_VERSION_IND, 1, 1), Step::Status(0x0020)]);
        assert_eq!(r.anchor, None);
        assert_eq!(counter(CRC_ERRORS), errors);
        assert_eq!(pop_all(), vec![(3, TRACED_VERSION_IND[6..12].to_vec())]);
        assert_eq!(SN_NESN.get(), 0x30);
    }
}

#[test]
fn the_longest_packet_the_buffer_holds_is_checked_and_one_beyond_the_ll_payload_counts_as_an_overflow() {
    let _s = sim::session();
    fresh_link();
    let overflows = counter(RX_OVERFLOWS);
    let p = rx_packet(0x02, &[0x5a; LL_PAYLOAD_MAX]);
    let ev = event_now(7, 2_500, 30_000);
    run_event(&ev, vec![rx(&p, 1, 1), Step::Status(0x0020)]);
    assert_eq!(pop_all(), vec![(2, vec![0x5a; LL_PAYLOAD_MAX])]);
    let longer = rx_packet(0x02, &[0x5a; LL_PAYLOAD_MAX + 1]);
    let ev = event_now(7, 2_500, 30_000);
    let r = run_event(&ev, vec![rx(&longer, 0, 0), Step::Status(0x0020)]);
    assert_eq!(r.anchor, None);
    assert_eq!(counter(RX_OVERFLOWS), overflows + 1);
    assert!(pop_all().is_empty());
    let edge = rx_packet(0x02, &[0x11; RX_LEN - 17]);
    let errors = counter(CRC_ERRORS);
    let ev = event_now(7, 2_500, 30_000);
    run_event(&ev, vec![rx(&edge, 0, 0), Step::Status(0x0020)]);
    assert_eq!((counter(RX_OVERFLOWS), counter(CRC_ERRORS)), (overflows + 2, errors));
}

#[test]
fn a_full_receive_queue_refuses_a_new_packet_but_takes_repeats_and_empty_ones() {
    let _s = sim::session();
    fresh_link();
    RX_HEAD.set(RX_SLOTS as u8);
    let overflows = counter(RX_OVERFLOWS);
    let ev = event_now(7, 2_500, 30_000);
    run_event(&ev, vec![rx(&TRACED_VERSION_IND, 1, 1), Step::Status(0x0020)]);
    assert_eq!(counter(RX_OVERFLOWS), overflows + 1);
    assert_eq!(SN_NESN.get(), 0x10);
    let ev = event_now(7, 2_500, 30_000);
    run_event(&ev, vec![rx(&TRACED_EMPTY, 1, 1), Step::Status(0x0020)]);
    assert_eq!(SN_NESN.get(), 0x30);
    let ev = event_now(7, 2_500, 30_000);
    run_event(&ev, vec![rx(&TRACED_VERSION_IND, 0, 1), Step::Status(0x0020)]);
    assert_eq!(SN_NESN.get(), 0x20);
    assert_eq!(counter(RX_OVERFLOWS), overflows + 1);
    assert_eq!(RX_HEAD.get(), RX_SLOTS as u8);
    RX_TAIL.set(1);
    let ev = event_now(7, 2_500, 30_000);
    run_event(&ev, vec![rx(&TRACED_VERSION_IND, 0, 0), Step::Status(0x0020)]);
    assert_eq!(RX_HEAD.get(), RX_SLOTS as u8 + 1);
    assert_eq!(counter(RX_OVERFLOWS), overflows + 1);
}

#[test]
fn a_repeated_packet_is_acknowledged_but_not_queued_again() {
    let _s = sim::session();
    fresh_link();
    let ev = event_now(7, 2_500, 30_000);
    run_event(&ev, vec![rx(&TRACED_VERSION_IND, 1, 1), rx(&TRACED_VERSION_IND, 1, 1), Step::Status(0x0020)]);
    assert_eq!(pop_all().len(), 1);
    let ev = event_now(7, 2_500, 30_000);
    run_event(&ev, vec![rx(&TRACED_EMPTY, 0, 0), Step::Status(0x0020)]);
    assert!(pop_all().is_empty());
    assert_eq!(SN_NESN.get(), 0x00);
}

#[test]
fn only_a_first_packet_stamped_within_2_ms_gives_the_anchor() {
    for (ago, anchored) in [(2000 * TICKS_PER_US - 33, true), (2000 * TICKS_PER_US - 32, false)] {
        let _s = sim::session();
        fresh_link();
        let ev = event_now(7, 2_500, 30_000);
        let r = run_event(&ev, vec![Step::Rx { bytes: TRACED_EMPTY.to_vec(), sn: 1, nesn: 1, stamp_ago: ago, rptr: None }, Step::Status(0x0020)]);
        assert_eq!(r.anchor.is_some(), anchored, "{ago}");
    }
}

fn brx_reads() -> (Vec<u32>, Vec<u16>) {
    let log = sim::with(|c| c.log.clone());
    let from = log.iter().position(|o| *o == Op::W8(RF_CMD, 0x82)).unwrap_or(0);
    let to = from + log[from..].iter().position(|o| *o == Op::W8(RF_CMD, 0x80)).unwrap_or(0);
    let ticks = log[from..to].iter().filter_map(|o| match *o {
        Op::R32(0x740, t) => Some(t),
        _ => None,
    });
    let status = log[from..to].iter().filter_map(|o| match *o {
        Op::R16(RF_IRQ_STATUS, s) => Some(s),
        _ => None,
    });
    (ticks.collect(), status.collect())
}

#[test]
fn an_event_without_a_packet_listens_until_the_window_and_400_us_more() {
    let _s = sim::session();
    fresh_link();
    let ev = event_now(7, 2_500, 30_000);
    let r = run_event(&ev, vec![]);
    assert_eq!(r.anchor, None);
    let first_by = ev.start.wrapping_add(ev.window).wrapping_add(400 * TICKS_PER_US);
    assert_eq!(brx_reads().0.last().copied(), Some(first_by));
}

#[test]
fn an_event_ends_at_its_deadline_when_that_comes_first() {
    let _s = sim::session();
    fresh_link();
    let ev = event_now(7, 2_500, 1_000);
    run_event(&ev, vec![]);
    assert_eq!(brx_reads().0.last().copied(), Some(ev.deadline));
    let ev = event_now(7, 2_500, 30_000);
    run_event(&ev, vec![rx(&TRACED_EMPTY, 1, 1)]);
    let t = brx_reads().0;
    let heard = sim::with(|c| {
        let log = &c.log;
        let at = log.iter().position(|o| *o == Op::W16(RF_IRQ_STATUS, STATUS_RX)).unwrap_or(0);
        log[at..].iter().find_map(|o| match *o {
            Op::R32(0x740, t) => Some(t),
            _ => None,
        })
    });
    let heard = heard.unwrap_or(0);
    assert_eq!(t.last().copied(), Some(heard.wrapping_add(RX_GUARD)));
    let ev = event_now(7, 2_500, 500);
    run_event(&ev, vec![rx(&TRACED_EMPTY, 1, 1)]);
    assert_eq!(brx_reads().0.last().copied(), Some(ev.deadline));
}

#[test]
fn the_end_bits_of_the_status_close_the_event_and_the_others_do_not() {
    for bit in [0x0004u16, 0x0010, 0x0020, 0x0040, 0x0400] {
        let _s = sim::session();
        fresh_link();
        let ev = event_now(7, 2_500, 30_000);
        run_event(&ev, vec![Step::Status(bit)]);
        let (ticks, status) = brx_reads();
        assert_eq!(status.len(), 2, "{bit:#x}");
        assert_eq!(status[1] & bit, bit);
        assert_eq!(ticks.len(), 1);
    }
    for bit in [0x0008u16, 0x0080, 0x0100, 0x0200, 0x0800, 0x8000] {
        let _s = sim::session();
        fresh_link();
        let ev = event_now(7, 2_500, 30_000);
        run_event(&ev, vec![Step::Status(bit)]);
        let first_by = ev.start.wrapping_add(ev.window).wrapping_add(400 * TICKS_PER_US);
        assert_eq!(brx_reads().0.last().copied(), Some(first_by), "{bit:#x}");
    }
}

#[test]
fn the_tx_status_is_cleared_each_time_it_is_raised() {
    let _s = sim::session();
    fresh_link();
    let ev = event_now(7, 2_500, 30_000);
    run_event(&ev, vec![Step::Status(STATUS_TX), Step::Status(STATUS_TX), Step::Status(0x0020)]);
    let w = writes_after(&sim::with(|c| c.log.clone()), Op::W8(RF_CMD, 0x82));
    assert_eq!(w.iter().filter(|o| **o == Op::W16(RF_IRQ_STATUS, STATUS_TX)).count(), 2);
    assert_eq!(brx_reads().1.len(), 6);
}

fn pushes() -> Vec<u16> {
    sim::with(|c| c.writes())
        .into_iter()
        .filter_map(|o| match o {
            Op::W16(TX_FIFO_PUSH, a) => Some(a),
            _ => None,
        })
        .collect()
}

fn slot(i: usize) -> u16 {
    (TX_QUEUE.addr() + i % TX_SLOTS * TX_SLOT) as u16
}

#[test]
fn queued_packets_follow_a_sacrificial_empty_packet_into_an_empty_fifo_and_leave_when_acknowledged() {
    let _s = sim::session();
    fresh_link();
    assert!(tx_push(2, &[1, 2, 3]));
    assert!(tx_push(3, &[4]));
    FIFO_KIND.set([9; 16]);
    let ev = event_now(7, 2_500, 30_000);
    let ack = Step::Rx { bytes: TRACED_EMPTY.to_vec(), sn: 1, nesn: 1, stamp_ago: 80 * 16, rptr: Some(2) };
    run_event(&ev, vec![ack, Step::Status(0x0020)]);
    assert_eq!(pushes(), vec![EMPTY.addr() as u16, slot(0), slot(1)]);
    assert_eq!(FIFO_KIND.get(), [0, 1, 1, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9]);
    assert_eq!((TX_HEAD.get(), TX_HANDED.get(), TX_TAIL.get()), (1, 2, 2));
    assert_eq!(tx_free(), TX_SLOTS - 1);
    assert!(!tx_idle());
    let ev = event_now(7, 2_500, 30_000);
    let ack = Step::Rx { bytes: TRACED_EMPTY.to_vec(), sn: 0, nesn: 0, stamp_ago: 80 * 16, rptr: Some(3) };
    run_event(&ev, vec![ack, Step::Status(0x0020)]);
    assert!(pushes().is_empty());
    assert!(sim::with(|c| c.writes()).contains(&Op::W8(TX_FIFO_RPTR, 0x42)));
    assert!(tx_idle());
    let mut buf = [0u8; 10];
    TX_QUEUE.copy_to(0, &mut buf);
    assert_eq!(buf, [5, 0, 0, 0, 2, 3, 1, 2, 3, 0]);
}

#[test]
fn the_fifo_is_topped_up_to_seven_and_the_rest_waits() {
    let _s = sim::session();
    fresh_link();
    for i in 0..TX_SLOTS {
        assert!(tx_push(2, &[i as u8]));
    }
    let ev = event_now(7, 2_500, 30_000);
    run_event(&ev, vec![]);
    assert_eq!(pushes(), [vec![EMPTY.addr() as u16], (0..6).map(slot).collect()].concat());
    assert_eq!(TX_HANDED.get(), 6);
    assert_eq!(TX_HEAD.get(), 0);
    let ev = event_now(7, 2_500, 30_000);
    let ack = Step::Rx { bytes: TRACED_EMPTY.to_vec(), sn: 1, nesn: 1, stamp_ago: 80 * 16, rptr: Some(3) };
    run_event(&ev, vec![ack, Step::Status(0x0020)]);
    assert!(pushes().is_empty());
    assert_eq!(TX_HEAD.get(), 2);
    let ev = event_now(7, 2_500, 30_000);
    run_event(&ev, vec![]);
    assert_eq!(pushes(), vec![slot(6), slot(7)]);
    assert_eq!(sim::with(|c| c.peek8(TX_FIFO_WPTR)), 9);
}

#[test]
fn a_fifo_that_still_holds_packets_gets_no_empty_packet_and_is_freed_across_its_wrap() {
    let _s = sim::session();
    fresh_link();
    sim::with(|c| c.poke8(TX_FIFO_WPTR, 14));
    FIFO_KIND.set([9; 16]);
    assert!(tx_push(2, &[1]));
    let ev = event_now(7, 2_500, 30_000);
    run_event(&ev, vec![]);
    assert_eq!(pushes(), vec![EMPTY.addr() as u16, slot(0)]);
    assert_eq!(FIFO_RPTR.get(), 14);
    assert!(tx_push(2, &[2]));
    let ev = event_now(7, 2_500, 30_000);
    let ack = Step::Rx { bytes: TRACED_EMPTY.to_vec(), sn: 1, nesn: 1, stamp_ago: 80 * 16, rptr: Some(1) };
    run_event(&ev, vec![ack, Step::Status(0x0020)]);
    assert_eq!(pushes(), vec![slot(1)]);
    assert!(sim::with(|c| c.writes()).contains(&Op::W8(TX_FIFO_RPTR, 0x4e)));
    assert_eq!(FIFO_KIND.get(), [1, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 0, 1]);
    assert_eq!((FIFO_FREED.get(), TX_HEAD.get()), (1, 2));
    assert!(tx_idle());
}

fn first_write_at(log: &[Op]) -> usize {
    log.iter().position(Op::is_write).unwrap_or(0)
}

fn waited_until(log: &[Op]) -> Option<u32> {
    let w = first_write_at(log);
    log[..w].windows(2).rev().find_map(|p| match p {
        [Op::R8(0x448, _), Op::R32(0x740, t)] => Some(*t),
        _ => None,
    })
}

fn patience_from(log: &[Op]) -> Option<u32> {
    let first = log.iter().position(|o| matches!(o, Op::R8(0x448, _))).unwrap_or(0);
    last_tick_before(log, first)
}

fn last_tick_before(log: &[Op], at: usize) -> Option<u32> {
    log[..at].iter().rev().find_map(|o| match *o {
        Op::R32(0x740, t) => Some(t),
        _ => None,
    })
}

#[test]
fn the_radio_is_taken_700_us_before_the_event_and_zigbee_may_finish_until_400_us_before() {
    let _s = sim::session();
    fresh_link();
    let ev = event_now(7, 2_500, 30_000);
    run_event(&ev, vec![]);
    let log = sim::with(|c| c.log.clone());
    assert_eq!(patience_from(&log), Some(ev.start.wrapping_sub(700 * TICKS_PER_US)));
    fresh_link();
    sim::with(|c| c.poke8(0x448, 0x20));
    let ev = event_now(7, 2_500, 30_000);
    run_event(&ev, vec![]);
    let log = sim::with(|c| c.log.clone());
    assert_eq!(waited_until(&log), Some(ev.start.wrapping_sub(400 * TICKS_PER_US)));
}

#[test]
fn a_late_event_opens_its_receiver_ten_ticks_from_now() {
    for late_us in [0u32, 299, 301, 2_000] {
        let _s = sim::session();
        fresh_link();
        let mut ev = event_now(7, 2_500, 30_000);
        ev.start = sim::with(|c| c.tick).wrapping_sub(late_us * TICKS_PER_US).wrapping_add(300 * TICKS_PER_US);
        run_event(&ev, vec![]);
        let log = sim::with(|c| c.log.clone());
        let cmd = log.iter().position(|o| matches!(o, Op::W32(RF_START_TICK, _))).unwrap_or(0);
        let now = last_tick_before(&log, cmd).unwrap_or(0);
        let open = ev.start.wrapping_sub(RX_LEAD);
        assert!((open.wrapping_sub(now) as i32) < 10);
        assert_eq!(log[cmd], Op::W32(RF_START_TICK, now.wrapping_add(10)), "{late_us}");
    }
}

#[test]
fn the_receiver_opens_ten_ticks_ahead_at_the_least() {
    for ahead in [9u32, 10, 11, 12] {
        let _s = sim::session();
        fresh_link();
        let mut ev = event_now(7, 2_500, 30_000);
        sim::with(|c| c.log.clear());
        let target = std::rc::Rc::new(std::cell::Cell::new(0u32));
        let t = target.clone();
        sim::with(|c| {
            c.on(move |c, op| {
                if let Op::W32(RF_FIRST_TIMEOUT, _) = op {
                    c.tick = t.get().wrapping_sub(ahead).wrapping_sub(16);
                }
            })
        });
        ev.start = sim::with(|c| c.tick).wrapping_add(2_000 * TICKS_PER_US);
        target.set(ev.start.wrapping_sub(RX_LEAD));
        connection_event(&ev);
        sim::with(|c| c.hook = None);
        let log = sim::with(|c| c.log.clone());
        let cmd = log.iter().position(|o| matches!(o, Op::W32(RF_START_TICK, _))).unwrap_or(0);
        let now = last_tick_before(&log, cmd).unwrap_or(0);
        assert_eq!(now, target.get().wrapping_sub(ahead));
        assert_eq!(log[cmd], Op::W32(RF_START_TICK, now.wrapping_add(ahead.max(10))), "{ahead}");
    }
}

fn busy_fifo() {
    FIFO_STARTED.set(1);
    RESET_PENDING.set(0);
    FIFO_WORD.set(0x0100_0000);
    sim::with(|c| {
        c.poke8(TX_FIFO_WPTR, 1);
        c.poke8(TX_FIFO_RPTR, 0);
    });
}

#[test]
fn borrowing_the_radio_switches_it_to_ble_1m_as_the_vendor_does() {
    let _s = sim::session();
    fresh_link();
    busy_fifo();
    sim::with(|c| {
        c.poke8(DMA_CHANNEL_ENABLE, 0x0c);
        c.poke32(reg::IRQ_MASK, 0x0010_2001);
        c.poke8(0x1225, 0x01);
        c.poke8(0x1226, 0x7f);
        c.poke8(0x1227, 0xe0);
        c.log.clear();
    });
    FIFO_WORD.set(0x0140_0003);
    RX_CURRENT.set(1);
    assert!(!mesh_send(&[37], 0x1234_5678, &[0x02, 0x00]));
    let w = sim::with(|c| c.writes());
    let from = w.iter().position(|o| *o == Op::W8(DMA_CHANNEL_ENABLE, 0x04)).expect("TX DMA off first");
    let mut expected = vec![
        Op::W8(0xc20, 0x04),
        Op::W8(0xc20, 0x00),
        Op::W8(0xf16, 0x29),
        Op::W8(0x428, 0x80),
        Op::W8(0xf02, 0x45),
        Op::W8(0x61, 0x01),
        Op::W8(0x61, 0x00),
        Op::W8(0x61, 0x01),
        Op::W8(0x61, 0x00),
        Op::W32(0xc08, RX[1].addr() as u16 as u32 | 0x0104_0000),
        Op::W32(0xc28, 0x0140_0003),
        Op::W32(0x640, 0x0010_0001),
        Op::W16(0xf20, 0xffff),
    ];
    expected.extend(VENDOR_BLE_1M_INIT);
    expected.extend([Op::W8(0xc20, 0x0c), Op::W8(0x1225, 0x41), Op::W8(0x1226, 0xff), Op::W8(0x1227, 0xff)]);
    assert_eq!(w[from..from + expected.len()].to_vec(), expected);
    let back = &w[from + expected.len()..];
    assert_eq!(back[..3].to_vec(), vec![Op::W8(0xf16, 0x29), Op::W8(0x428, 0x80), Op::W8(0xf02, 0x45)]);
    assert_eq!(back[3], Op::W8(0xc20, 0x08));
    assert_eq!(back[4..8].to_vec(), vec![Op::W8(0x61, 0x01), Op::W8(0x61, 0x00), Op::W8(0x61, 0x01), Op::W8(0x61, 0x00)]);
}

#[test]
fn a_reset_connection_empties_the_hardware_fifo_on_the_next_borrow() {
    let _s = sim::session();
    fresh_link();
    busy_fifo();
    FIFO_WORD.set(0x0500_0000);
    RESET_PENDING.set(1);
    FIFO_RPTR.set(9);
    FIFO_FREED.set(9);
    assert!(mesh_send(&[37], 0x1234_5678, &[0x02, 0x00]));
    let w = sim::with(|c| c.writes());
    assert!(w.contains(&Op::W8(TX_FIFO_RPTR, 0x10)));
    assert!(w.contains(&Op::W32(DMA_FIFO, 0x0500_0000)));
    assert_eq!((FIFO_RPTR.get(), FIFO_FREED.get(), RESET_PENDING.get(), FIFO_STARTED.get()), (5, 5, 0, 1));
    sim::with(|c| c.log.clear());
    busy_fifo();
    let _ = mesh_send(&[37], 0x1234_5678, &[0x02, 0x00]);
    let w = sim::with(|c| c.writes());
    assert!(!w.iter().any(|o| matches!(o, Op::W8(TX_FIFO_RPTR, _))));
    assert!(w.iter().any(|o| matches!(o, Op::W32(DMA_FIFO, _))));
    sim::with(|c| c.log.clear());
    FIFO_STARTED.set(0);
    let _ = mesh_send(&[37], 0x1234_5678, &[0x02, 0x00]);
    let w = sim::with(|c| c.writes());
    assert!(w.contains(&Op::W8(TX_FIFO_RPTR, 0x10)));
    assert!(!w.iter().any(|o| matches!(o, Op::W32(DMA_FIFO, _))));
}

#[test]
fn giving_the_radio_back_saves_the_fifo_word() {
    let _s = sim::session();
    fresh_link();
    busy_fifo();
    sim::with(|c| {
        c.on(|c, op| {
            if op == Op::R8(TX_FIFO_WPTR, 1) {
                c.poke32(DMA_FIFO, 0x0040_0007);
            }
        })
    });
    assert!(!mesh_send(&[37], 0x1234_5678, &[0x02, 0x00]));
    sim::with(|c| c.hook = None);
    assert_eq!(FIFO_WORD.get(), 0x0040_0007);
}

fn lend_for(extra: u32) -> (u32, u32, u32) {
    busy_fifo();
    sim::with(|c| {
        c.log.clear();
        c.on(move |c, op| {
            if let Op::R8(TX_FIFO_WPTR, _) = op {
                c.tick = c.tick.wrapping_add(extra);
            }
        })
    });
    assert!(!mesh_send(&[37], 0x1234_5678, &[0x02, 0x00]));
    sim::with(|c| c.hook = None);
    let t = sim::with(|c| c.ticks_read());
    let n = t.len();
    (t[n - 3], t[n - 2], t[n - 1])
}

#[test]
fn the_time_lent_to_ble_is_counted_in_milliseconds_with_the_rest_carried_and_the_longest_lend_kept() {
    let _s = sim::session();
    fresh_link();
    LENT_MS.set(0);
    LENT_REST.set(0);
    LENT_MAX_US.set(0);
    let lends = LENDS.get();
    let (at, this, end) = lend_for(3 * 16_000 - 32 - 1);
    assert_eq!((this.wrapping_sub(at), end.wrapping_sub(at)), (3 * 16_000 - 17, 3 * 16_000 - 1));
    assert_eq!((LENT_MS.get(), LENT_REST.get()), (2, 15_999));
    assert_eq!(lent_max_us(), (3 * 16_000 - 17) / 16);
    let _ = lend_for(1);
    assert_eq!((LENT_MS.get(), LENT_REST.get()), (3, 32));
    assert_eq!(lent_max_us(), (3 * 16_000 - 17) / 16);
    let _ = lend_for(5 * 16_000);
    assert_eq!((LENT_MS.get(), LENT_REST.get()), (8, 64));
    assert_eq!(lent_max_us(), 5 * 1_000 + 1);
    assert_eq!(lent(), (lends.wrapping_add(3), 8));
}

const ADV_A: [u8; 6] = [0xee, 0xe7, 0x78, 0x38, 0xc1, 0xa4];

fn adv_ind(data: &[u8]) -> Vec<u8> {
    [vec![0x00, (6 + data.len()) as u8], ADV_A.to_vec(), data.to_vec()].concat()
}

fn scan_rsp() -> Vec<u8> {
    [vec![0x04, 16], ADV_A.to_vec(), b"\x09\x09unpaired".to_vec()].concat()
}

fn answer(header: u8, payload: &[u8]) -> Vec<u8> {
    let n = payload.len();
    let mut v = vec![(n + 13) as u8, 0, 0, 0, header, n as u8];
    v.extend_from_slice(payload);
    v.extend_from_slice(&[0; 11]);
    v
}

fn scan_req(adv_a: [u8; 6]) -> Vec<u8> {
    answer(0x43, &[[0x66, 0x55, 0x44, 0x33, 0x22, 0x11], adv_a].concat())
}

fn connect_ind(adv_a: [u8; 6], ll_data: usize) -> Vec<u8> {
    let ll: Vec<u8> = (0..ll_data as u8).map(|i| 0x80 | i).collect();
    answer(0x25, &[vec![0x66, 0x55, 0x44, 0x33, 0x22, 0x11], adv_a.to_vec(), ll].concat())
}

struct Scanner {
    answers: Vec<Option<Vec<u8>>>,
    tx_done: bool,
    rx_done: bool,
}

fn scanner(s: Scanner) -> impl FnMut(&mut sim::Chip, Op) {
    let mut channel = 0usize;
    move |c, op| match op {
        Op::W8(RF_CMD, 0x87) => {
            if s.tx_done {
                c.raise16(RF_IRQ_STATUS, STATUS_TX);
            }
        }
        Op::W16(RF_IRQ_STATUS, STATUS_TX) => {
            if let Some(Some(p)) = s.answers.get(channel) {
                rx_buffer(RX_CURRENT.get()).copy_from(0, p);
                if s.rx_done {
                    c.raise16(RF_IRQ_STATUS, STATUS_RX);
                }
            }
            channel += 1;
        }
        _ => {}
    }
}

fn advertise_to(s: Scanner, adv: &[u8]) -> (Option<u32>, [u8; CONNECT_LEN], Vec<Op>) {
    sim::with(|c| {
        c.log.clear();
        c.on(scanner(s));
    });
    let mut connect = [0u8; CONNECT_LEN];
    let r = advertise(adv, &scan_rsp(), &mut connect);
    sim::with(|c| c.hook = None);
    (r, connect, sim::with(|c| c.log.clone()))
}

fn channels_visited(log: &[Op]) -> Vec<u8> {
    let w: Vec<Op> = log.iter().copied().filter(Op::is_write).collect();
    w.windows(2)
        .filter_map(|p| match p {
            [Op::W8(RF_CHANNEL, ch), Op::W8(0x1244, 0x20)] => Some(*ch),
            _ => None,
        })
        .collect()
}

fn scanner_answering(answers: Vec<Option<Vec<u8>>>) -> Scanner {
    Scanner { answers, tx_done: true, rx_done: true }
}

#[test]
fn a_scan_request_for_us_is_answered_and_ends_the_event() {
    let _s = sim::session();
    quiet_air();
    let rsp = scan_rsp();
    let (r, _, log) = advertise_to(scanner_answering(vec![Some(scan_req(ADV_A))]), &adv_ind(&[2, 1, 5]));
    assert_eq!(r, None);
    assert_eq!(channels_visited(&log), vec![37]);
    let w = writes_after(&log, Op::W16(DMA_TX_ADDR, SCAN_RSP.addr() as u16));
    let armed = log.iter().position(|o| *o == Op::W8(RF_CMD, 0x85)).unwrap_or(0) - 1;
    assert_eq!(log[armed], Op::W32(RF_START_TICK, last_tick_before(&log, armed).unwrap_or(0).wrapping_add(48)));
    assert_eq!(w[1..4].to_vec(), vec![w[1], Op::W8(RF_CMD, 0x85), Op::W16(RF_IRQ_STATUS, STATUS_RX)]);
    let cleared = log.iter().position(|o| *o == Op::W16(RF_IRQ_STATUS, STATUS_RX)).unwrap_or(0);
    let stop = cleared + log[cleared..].iter().position(|o| *o == Op::W8(RF_CMD, 0x80)).unwrap_or(0);
    let ticks: Vec<u32> = log[cleared..stop].iter().filter_map(|o| match *o {
        Op::R32(0x740, t) => Some(t),
        _ => None,
    }).collect();
    assert_eq!(ticks.last().copied(), Some(ticks[0].wrapping_add((rsp[1] as u32 * 8 + 204) * TICKS_PER_US)));
    let mut buf = [0u8; 22];
    SCAN_RSP.copy_to(0, &mut buf);
    assert_eq!(buf[..], [&[rsp.len() as u8, 0, 0, 0][..], &rsp[..]].concat()[..]);
}

#[test]
fn a_scan_request_for_another_advertiser_is_not_answered() {
    for i in 0..6 {
        let _s = sim::session();
        quiet_air();
        let mut other = ADV_A;
        other[i] ^= 0x01;
        let (r, _, log) = advertise_to(scanner_answering(vec![Some(scan_req(other)), None, None]), &adv_ind(&[2, 1, 5]));
        assert_eq!(r, None);
        assert_eq!(channels_visited(&log), vec![37, 38, 39], "AdvA byte {i}");
        let cleared = log.iter().position(|o| *o == Op::W16(RF_IRQ_STATUS, STATUS_RX)).unwrap_or(0);
        assert_eq!(log[cleared..].iter().find(|o| o.is_write() && **o != Op::W16(RF_IRQ_STATUS, STATUS_RX)), Some(&Op::W8(RF_CMD, 0x80)));
    }
}

#[test]
fn an_answer_that_is_bad_or_not_a_request_is_not_answered() {
    let mut rx_add = scan_req(ADV_A);
    rx_add[4] |= 0x80;
    let mut crc = scan_req(ADV_A);
    let at = crc[0] as usize + 3;
    crc[at] = 0x01;
    let mut dma = scan_req(ADV_A);
    dma[0] += 1;
    let mut dma_short = scan_req(ADV_A);
    dma_short[0] -= 1;
    let mut adv = scan_req(ADV_A);
    adv[4] = 0x40;
    let long_req = answer(0x43, &[[0x66; 6], ADV_A, [0; 6]].concat());
    let short_req = answer(0x43, &[[0x66; 5].to_vec(), ADV_A.to_vec()].concat());
    for (n, bad) in [rx_add, crc, dma, dma_short, adv, long_req, short_req].into_iter().enumerate() {
        let _s = sim::session();
        quiet_air();
        let (r, _, log) = advertise_to(scanner_answering(vec![Some(bad), None, None]), &adv_ind(&[2, 1, 5]));
        assert_eq!(r, None);
        assert_eq!(channels_visited(&log), vec![37, 38, 39], "answer {n}");
    }
}

#[test]
fn a_connect_indication_for_us_ends_advertising_with_its_tick_and_data() {
    let _s = sim::session();
    quiet_air();
    let ind = connect_ind(ADV_A, 22);
    let (r, connect, log) = advertise_to(scanner_answering(vec![None, Some(ind.clone())]), &adv_ind(&[2, 1, 5]));
    assert_eq!(channels_visited(&log), vec![37, 38]);
    assert_eq!(connect.to_vec(), ind[6..6 + CONNECT_LEN].to_vec());
    let cleared = log.iter().position(|o| *o == Op::W16(RF_IRQ_STATUS, STATUS_RX)).unwrap_or(0);
    let stop = cleared + log[cleared..].iter().position(|o| *o == Op::W8(RF_CMD, 0x80)).unwrap_or(0);
    let tick = log[stop..].iter().find_map(|o| match *o {
        Op::R32(0x740, t) => Some(t),
        _ => None,
    });
    assert_eq!(r, tick);
    assert!(r.is_some());
    let after: Vec<Op> = log[stop..].iter().copied().filter(Op::is_write).collect();
    assert_eq!(after[1..3].to_vec(), vec![Op::W8(RF_CMD, 0x80), Op::W16(RF_IRQ_STATUS, 0xffff)]);
}

#[test]
fn a_connect_indication_of_another_length_or_for_another_advertiser_is_ignored() {
    let mut other = ADV_A;
    other[5] ^= 0x80;
    for (n, bad) in [connect_ind(ADV_A, 21), connect_ind(ADV_A, 23), connect_ind(other, 22)].into_iter().enumerate() {
        let _s = sim::session();
        quiet_air();
        let (r, _, log) = advertise_to(scanner_answering(vec![Some(bad), None, None]), &adv_ind(&[2, 1, 5]));
        assert_eq!(r, None, "{n}");
        assert_eq!(channels_visited(&log), vec![37, 38, 39], "{n}");
    }
}

fn channel_ticks(log: &[Op], from: Op, to: Op) -> Vec<u32> {
    let a = log.iter().position(|o| *o == from).unwrap_or(0);
    let b = a + log[a..].iter().position(|o| *o == to).unwrap_or(0);
    log[a..b]
        .iter()
        .filter_map(|o| match *o {
            Op::R32(0x740, t) => Some(t),
            _ => None,
        })
        .collect()
}

#[test]
fn each_channel_listens_for_an_answer_for_the_packet_and_60_octets_more() {
    for data in [vec![2, 1, 5], vec![0; 31]] {
        let _s = sim::session();
        quiet_air();
        let adv = adv_ind(&data);
        let (_, _, log) = advertise_to(scanner_answering(vec![]), &adv);
        let ticks = channel_ticks(&log, Op::W16(DMA_TX_ADDR, ADV.addr() as u16), Op::W8(RF_CMD, 0x80));
        let window = (adv[1] as u32 + 60) * 8 * TICKS_PER_US;
        assert_eq!(ticks.last().copied(), Some(ticks[0].wrapping_add(window)));
    }
}

#[test]
fn a_transmission_that_never_completes_is_given_up_after_2_ms() {
    let _s = sim::session();
    quiet_air();
    let (_, _, log) = advertise_to(Scanner { answers: vec![], tx_done: false, rx_done: true }, &adv_ind(&[2, 1, 5]));
    let ticks = channel_ticks(&log, Op::W16(DMA_TX_ADDR, ADV.addr() as u16), Op::W16(RF_IRQ_STATUS, STATUS_TX));
    assert_eq!(ticks.last().copied(), Some(ticks[0].wrapping_add(2000 * TICKS_PER_US)));
    assert_eq!(channels_visited(&log), vec![37, 38, 39]);
}

#[test]
fn an_answer_whose_reception_never_ends_arms_the_response_after_400_us() {
    let _s = sim::session();
    quiet_air();
    let (r, _, log) = advertise_to(Scanner { answers: vec![Some(scan_req(ADV_A))], tx_done: true, rx_done: false }, &adv_ind(&[2, 1, 5]));
    assert_eq!(r, None);
    let seen = log.iter().position(|o| *o == Op::W16(DMA_TX_ADDR, SCAN_RSP.addr() as u16)).unwrap_or(0);
    let t1 = last_tick_before(&log, seen).unwrap_or(0);
    let armed = seen + log[seen..].iter().position(|o| matches!(o, Op::W32(RF_START_TICK, _))).unwrap_or(0);
    assert_eq!(log[armed], Op::W32(RF_START_TICK, t1.wrapping_add(6400 + 16 + 48)));
    assert_eq!(last_tick_before(&log, armed), Some(t1.wrapping_add(6400 + 16)));
}

#[test]
fn every_ble_channel_is_tuned_to_its_frequency_and_band() {
    for chn in 0u8..=40 {
        let mhz: u32 = match chn {
            0..=10 => 2404 + 2 * chn as u32,
            11..=36 => 2428 + 2 * (chn as u32 - 11),
            37 => 2402,
            38 => 2426,
            _ => 2480,
        };
        let band = match mhz {
            2465.. => 0x0c,
            2435..=2464 => 0x10,
            2405..=2434 => 0x14,
            _ => 0x18,
        };
        let _s = sim::session();
        sim::with(|c| {
            c.poke8(0x1228, 0xff);
            c.poke8(0x1229, 0xff);
        });
        set_channel(chn);
        let idx = ((mhz - 2402) / 2) as u8;
        assert_eq!(
            sim::with(|c| c.writes()),
            vec![Op::W8(0x40d, chn), Op::W8(0x1244, 0x20), Op::W8(0x1245, 0x26), Op::W8(0x1228, 0xc0 | idx), Op::W8(0x1229, 0xc3 | band)],
            "channel {chn}"
        );
    }
}

#[test]
fn advertising_and_scan_response_data_must_fit_an_advertising_pdu() {
    let _s = sim::session();
    let mut connect = [0u8; CONNECT_LEN];
    for (adv, rsp, ok) in [(1, 2, false), (2, 1, false), (2, 2, true), (ADV_PDU_MAX, ADV_PDU_MAX, true), (ADV_PDU_MAX + 1, 2, false), (2, ADV_PDU_MAX + 1, false)] {
        quiet_air();
        sim::with(|c| c.log.clear());
        let n = BLE_RADIO.at(ADV_EVENTS).unwrap_or(0);
        let _ = advertise(&vec![0; adv], &vec![0; rsp], &mut connect);
        assert_eq!(BLE_RADIO.at(ADV_EVENTS), Some(if ok { n + 1 } else { n }), "{adv} {rsp}");
        if ok {
            assert_eq!((ADV.read(0), SCAN_RSP.read(0)), (adv as u8, rsp as u8));
        }
    }
}

#[test]
fn zigbee_traffic_heard_in_the_last_64_ms_keeps_advertising_on_channel_37() {
    for (ago, channels) in [(64_000 * TICKS_PER_US - 1, vec![37]), (64_000 * TICKS_PER_US, vec![37, 38, 39])] {
        let _s = sim::session();
        quiet_air();
        sim::with(|c| {
            c.log.clear();
            c.on(move |c, op| {
                if op == Op::W16(RF_IRQ_MASK, 0) {
                    crate::radio::tests::heard_at(c.tick.wrapping_add(c.tick_step).wrapping_sub(ago));
                }
            })
        });
        let mut connect = [0u8; CONNECT_LEN];
        let _ = advertise(&adv_ind(&[2, 1, 5]), &scan_rsp(), &mut connect);
        sim::with(|c| c.hook = None);
        assert_eq!(channels_visited(&sim::with(|c| c.log.clone())), channels, "{ago}");
    }
}

#[test]
fn advertising_waits_up_to_6_ms_for_3_ms_of_quiet_zigbee_air() {
    let _s = sim::session();
    quiet_air();
    let mut heard = None;
    sim::with(|c| {
        c.log.clear();
        c.on(move |c, op| {
            if let (Op::R8(0x448, _), None) = (op, heard) {
                heard = Some(c.tick.wrapping_add(c.tick_step));
                crate::radio::tests::heard_at(c.tick.wrapping_add(c.tick_step));
            }
        })
    });
    let mut connect = [0u8; CONNECT_LEN];
    let _ = advertise(&adv_ind(&[2, 1, 5]), &scan_rsp(), &mut connect);
    sim::with(|c| c.hook = None);
    let log = sim::with(|c| c.log.clone());
    let first = log.iter().position(|o| matches!(o, Op::R8(0x448, _))).unwrap_or(0);
    let t0 = log[first..].iter().find_map(|o| match *o {
        Op::R32(0x740, t) => Some(t),
        _ => None,
    });
    let taken = last_tick_before(&log, first_write_at(&log)).unwrap_or(0);
    let quiet: Vec<u32> = log[first..first_write_at(&log)].windows(3).filter_map(|p| match p {
        [Op::R8(0x441, _), Op::R32(0x740, t), _] => Some(*t),
        _ => None,
    }).collect();
    assert_eq!(quiet.last().copied(), t0.map(|t| t.wrapping_add(3_000 * TICKS_PER_US)));
    assert!(taken.wrapping_sub(quiet.last().copied().unwrap_or(0)) <= 16);
    quiet_air();
    sim::with(|c| {
        c.poke8(0x448, 0x20);
        c.log.clear();
    });
    let _ = advertise(&adv_ind(&[2, 1, 5]), &scan_rsp(), &mut connect);
    let log = sim::with(|c| c.log.clone());
    assert_eq!(last_tick_before(&log, first_write_at(&log)), patience_from(&log).map(|t| t.wrapping_add(ADV_PATIENCE)));
    quiet_air();
    sim::with(|c| {
        c.poke8(0x441, 0x1a);
        c.log.clear();
    });
    let _ = advertise(&adv_ind(&[2, 1, 5]), &scan_rsp(), &mut connect);
    let log = sim::with(|c| c.log.clone());
    assert_eq!(last_tick_before(&log, first_write_at(&log)), patience_from(&log).map(|t| t.wrapping_add(ADV_PATIENCE)));
}

fn burst(channels: &[u8], pdu: &[u8], tx_done: bool) -> (bool, Vec<Op>) {
    sim::with(|c| {
        c.log.clear();
        let mut shifted = false;
        c.on(move |c, op| match op {
            Op::W8(RF_CMD, 0x85) => {
                shifted = false;
                if tx_done {
                    c.raise16(RF_IRQ_STATUS, STATUS_TX);
                }
            }
            Op::R16(RF_IRQ_STATUS, _) if !shifted => {
                shifted = true;
                c.tick = c.tick.wrapping_add(16);
            }
            _ => {}
        })
    });
    let r = mesh_send(channels, 0x1234_5678, pdu);
    sim::with(|c| c.hook = None);
    (r, sim::with(|c| c.log.clone()))
}

#[test]
fn a_mesh_pdu_must_fit_an_advertising_pdu() {
    for (len, sent) in [(1usize, false), (2, true), (ADV_PDU_MAX, true), (ADV_PDU_MAX + 1, false)] {
        let _s = sim::session();
        quiet_air();
        let bursts = counter(MESH_BURSTS);
        let pdu: Vec<u8> = (0..len as u8).collect();
        let (r, log) = burst(&[37], &pdu, true);
        assert!(r);
        assert_eq!(counter(MESH_BURSTS), if sent { bursts + 1 } else { bursts }, "{len}");
        assert_eq!(log.iter().any(Op::is_write), sent);
        if sent {
            let mut buf = vec![0u8; len + 4];
            ADV.copy_to(0, &mut buf);
            assert_eq!(buf, [vec![len as u8, 0, 0, 0], pdu].concat());
        }
    }
}

#[test]
fn a_mesh_burst_sends_on_each_channel_with_the_mesh_access_code_for_its_air_time() {
    let _s = sim::session();
    quiet_air();
    let pdu = [0x02, 0x05, 1, 2, 3, 4, 5];
    let (r, log) = burst(&[2, 12, 23], &pdu, true);
    assert!(r);
    assert_eq!(channels_visited(&log), vec![2, 12, 23]);
    let w: Vec<Op> = log.iter().copied().filter(Op::is_write).collect();
    let setup = w.iter().position(|o| *o == Op::W32(RF_ACCESS, 0x1234_5678)).expect("the access code");
    assert_eq!(
        w[setup - 4..setup + 5].to_vec(),
        vec![
            Op::W8(0xf16, 0x29),
            Op::W8(0x428, 0x80),
            Op::W8(0xf02, 0x45),
            Op::W8(0xf00, 0x80),
            Op::W32(0x408, 0x1234_5678),
            Op::W32(0x424, 0x0055_5555),
            Op::W16(0xf04, 74),
            Op::W16(0xf1c, 0),
            Op::W8(0xf00, 0x80),
        ]
    );
    let air = ((pdu.len() as u32 + 8) * 8 + 100) * TICKS_PER_US;
    let mut from = 0;
    for _ in 0..3 {
        let kick = from + log[from..].iter().position(|o| *o == Op::W8(RF_CMD, 0x85)).unwrap_or(0);
        let start = &log[kick - 3];
        assert_eq!(log[kick - 5], Op::W16(RF_IRQ_STATUS, STATUS_TX));
        let at = last_tick_before(&log, kick - 3).unwrap_or(0);
        assert_eq!(*start, Op::W32(RF_START_TICK, at.wrapping_add(30 * TICKS_PER_US)));
        assert_eq!(log[kick + 1], Op::W16(DMA_TX_ADDR, ADV.addr() as u16));
        let done = kick + log[kick..].iter().position(|o| *o == Op::W16(RF_IRQ_STATUS, STATUS_TX)).unwrap_or(0);
        let ticks: Vec<u32> = log[kick..done].iter().filter_map(|o| match *o {
            Op::R32(0x740, t) => Some(t),
            _ => None,
        }).collect();
        assert_eq!(ticks.last().copied(), Some(ticks[0].wrapping_add(air)));
        from = done;
    }
}

#[test]
fn a_mesh_transmission_that_never_completes_is_given_up_after_1500_us() {
    let _s = sim::session();
    quiet_air();
    let (_, log) = burst(&[2], &[0x02, 0x00], false);
    let kick = log.iter().position(|o| *o == Op::W8(RF_CMD, 0x85)).unwrap_or(0);
    let done = kick + log[kick..].iter().position(|o| *o == Op::W16(RF_IRQ_STATUS, STATUS_TX)).unwrap_or(0);
    let ticks: Vec<u32> = log[kick..done].iter().filter_map(|o| match *o {
        Op::R32(0x740, t) => Some(t),
        _ => None,
    }).collect();
    assert_eq!(ticks.last().copied(), Some(ticks[0].wrapping_add(1500 * TICKS_PER_US)));
}

#[test]
fn a_mesh_burst_waits_up_to_6_ms_for_zigbee_to_finish_receiving() {
    let _s = sim::session();
    quiet_air();
    sim::with(|c| c.poke8(0x448, 0x20));
    let (_, log) = burst(&[2], &[0x02, 0x00], true);
    assert_eq!(waited_until(&log), patience_from(&log).map(|t| t.wrapping_add(ADV_PATIENCE)));
    sim::with(|c| c.poke8(0x448, 0x00));
    let (_, log) = burst(&[2], &[0x02, 0x00], true);
    assert_eq!(waited_until(&log), None);
    assert_eq!(log.iter().filter(|o| matches!(o, Op::R8(0x448, _))).count(), 1);
}

fn listen(packets: Vec<Vec<u8>>, ticks: u32) -> (Option<Vec<u8>>, Vec<Op>) {
    let mut q = std::collections::VecDeque::from(packets);
    sim::with(|c| {
        c.log.clear();
        let mut on = false;
        c.on(move |c, op| match op {
            Op::W8(RF_MODE_CTRL, 0x65) => on = true,
            Op::W8(0xf16, 0x29) => on = false,
            Op::R16(RF_IRQ_STATUS, s) if on && s & STATUS_RX == 0 => {
                if let Some(p) = q.pop_front() {
                    rx_buffer(RX_CURRENT.get()).copy_from(0, &p);
                    c.raise16(RF_IRQ_STATUS, STATUS_RX);
                }
            }
            _ => {}
        })
    });
    let mut pdu = [0u8; ADV_PDU_MAX];
    let r = mesh_listen(23, 0x1234_5678, ticks, &mut pdu);
    sim::with(|c| c.hook = None);
    (r.map(|n| pdu[..n].to_vec()), sim::with(|c| c.log.clone()))
}

#[test]
fn a_mesh_listen_hears_the_first_good_pdu() {
    let _s = sim::session();
    quiet_air();
    RX_CURRENT.set(1);
    let received = counter(MESH_RECEIVED);
    let good = answer(0x02, &[7; 20]);
    let mut crc = good.clone();
    let at = crc[0] as usize + 3;
    crc[at] = 0x01;
    let mut dma = good.clone();
    dma[0] += 1;
    let mut dma_short = good.clone();
    dma_short[0] -= 1;
    let (r, log) = listen(vec![crc, dma, dma_short, good.clone()], 10_000 * TICKS_PER_US);
    assert_eq!(r, Some(good[4..26].to_vec()));
    assert_eq!(counter(MESH_RECEIVED), received + 1);
    let w: Vec<Op> = log.iter().copied().filter(Op::is_write).collect();
    let tuned = w.iter().position(|o| *o == Op::W8(0x40d, 23)).unwrap_or(0);
    let p = RX[1].addr() as u16;
    assert_eq!(
        w[tuned + 5..tuned + 9].to_vec(),
        vec![Op::W16(DMA_RX_ADDR, p), Op::W16(RF_IRQ_STATUS, STATUS_RX), Op::W8(0x428, 0x81), Op::W8(0xf02, 0x65)]
    );
    assert_eq!(w.iter().filter(|o| **o == Op::W16(DMA_RX_ADDR, p)).count(), 4);
    assert_eq!(w.iter().filter(|o| **o == Op::W16(RF_IRQ_STATUS, STATUS_RX)).count(), 5);
    let off = w.iter().rposition(|o| *o == Op::W16(RF_IRQ_STATUS, STATUS_RX)).unwrap_or(0);
    assert_eq!(w[off + 1..off + 5].to_vec(), vec![Op::W8(0xf16, 0x29), Op::W8(0x428, 0x80), Op::W8(0xf02, 0x45), Op::W16(0xf20, 0xffff)]);
}

#[test]
fn a_mesh_pdu_longer_than_an_advertising_pdu_is_not_taken() {
    let _s = sim::session();
    quiet_air();
    let received = counter(MESH_RECEIVED);
    let (r, _) = listen(vec![answer(0x02, &[1; ADV_PDU_MAX - 1])], 2_000 * TICKS_PER_US);
    assert_eq!(r, None);
    assert_eq!(counter(MESH_RECEIVED), received);
    let (r, _) = listen(vec![answer(0x02, &[1; ADV_PDU_MAX - 2])], 2_000 * TICKS_PER_US);
    assert_eq!(r.map(|p| p.len()), Some(ADV_PDU_MAX));
    assert_eq!(counter(MESH_RECEIVED), received + 1);
}

#[test]
fn a_silent_mesh_listen_lasts_exactly_its_ticks() {
    let _s = sim::session();
    quiet_air();
    let (r, log) = listen(vec![], 1_000 * TICKS_PER_US);
    assert_eq!(r, None);
    let on = log.iter().position(|o| *o == Op::W8(RF_MODE_CTRL, 0x65)).unwrap_or(0);
    let off = on + log[on..].iter().position(|o| *o == Op::W8(0xf16, 0x29)).unwrap_or(0);
    let ticks: Vec<u32> = log[on..off].iter().filter_map(|o| match *o {
        Op::R32(0x740, t) => Some(t),
        _ => None,
    }).collect();
    assert_eq!(ticks.last().copied(), Some(ticks[0].wrapping_add(1_000 * TICKS_PER_US)));
}

#[derive(Default)]
pub(crate) struct CentralState {
    pub connect: bool,
    pub connected: bool,
    pub silent: bool,
    pub out: std::collections::VecDeque<Vec<u8>>,
    pub received: Vec<(u8, Vec<u8>)>,
    pub events: usize,
    fifo: [u16; 16],
    sn: u8,
    nesn: u8,
    dma: u16,
    armed: bool,
    sent: usize,
    pending_tx: bool,
}

pub(crate) type Central = std::rc::Rc<std::cell::RefCell<CentralState>>;

const CONNECT_INTERVAL: u16 = 24;
const CONNECT_TIMEOUT: u16 = 500;

fn connect_payload(adv_a: [u8; 6]) -> Vec<u8> {
    let mut p = vec![0x66, 0x55, 0x44, 0x33, 0x22, 0x11];
    p.extend_from_slice(&adv_a);
    p.extend_from_slice(&AA.to_le_bytes());
    p.extend_from_slice(&CRC_INIT.to_le_bytes()[..3]);
    p.push(2);
    p.extend_from_slice(&0u16.to_le_bytes());
    p.extend_from_slice(&CONNECT_INTERVAL.to_le_bytes());
    p.extend_from_slice(&0u16.to_le_bytes());
    p.extend_from_slice(&CONNECT_TIMEOUT.to_le_bytes());
    p.extend_from_slice(&[0xff, 0xff, 0xff, 0xff, 0x1f]);
    p.push(0x27);
    p
}

fn slot_packet(addr: u16) -> Option<(u8, Vec<u8>)> {
    (0..TX_SLOTS).find(|i| slot(*i) == addr).map(|i| {
        let at = i * TX_SLOT;
        let n = TX_QUEUE.read(at + 5) as usize;
        let mut p = vec![0u8; n];
        TX_QUEUE.copy_to(at + 6, &mut p);
        (TX_QUEUE.read(at + 4), p)
    })
}

pub(crate) fn central_link() -> Central {
    let st: Central = Default::default();
    let s = st.clone();
    sim::with(|c| {
        c.on(move |c, op| {
            let mut s = s.borrow_mut();
            match op {
                Op::W8(RF_CMD, 0x87) => c.raise16(RF_IRQ_STATUS, STATUS_TX),
                Op::W16(RF_IRQ_STATUS, STATUS_TX) if s.connect && !s.connected && !s.armed => {
                    let mut adv_a = [0u8; 6];
                    ADV.copy_to(6, &mut adv_a);
                    rx_buffer(RX_CURRENT.get()).copy_from(0, &answer(0x05, &connect_payload(adv_a)));
                    c.raise16(RF_IRQ_STATUS, STATUS_RX);
                    s.connected = true;
                }
                Op::W16(TX_FIFO_PUSH, a) => {
                    let w = c.peek8(TX_FIFO_WPTR).wrapping_sub(1) & 0x0f;
                    s.fifo[w as usize] = a;
                }
                Op::W16(DMA_RX_ADDR, a) => s.dma = a,
                Op::W8(RF_CMD, 0x82) => {
                    s.armed = !s.silent;
                    s.sent = 0;
                    s.events += 1;
                }
                Op::W8(RF_CMD, 0x80) => s.armed = false,
                Op::R16(RF_IRQ_STATUS, st) if s.armed && st & (STATUS_RX | STATUS_TX) == 0 => {
                    if s.pending_tx {
                        s.pending_tx = false;
                        let r = c.peek8(TX_FIFO_RPTR) & 0x0f;
                        if r != c.peek8(TX_FIFO_WPTR) & 0x0f {
                            if let Some(p) = slot_packet(s.fifo[r as usize]) {
                                s.received.push(p);
                            }
                            c.poke8(TX_FIFO_RPTR, (r + 1) & 0x0f);
                        }
                    }
                    if s.sent >= 4 {
                        c.raise16(RF_IRQ_STATUS, 0x0020);
                        s.armed = false;
                        return;
                    }
                    let (llid, payload) = match s.out.pop_front() {
                        Some(p) => (2u8, p),
                        None => (1u8, vec![]),
                    };
                    s.sent += 1;
                    let pkt = rx_packet(llid, &payload);
                    if let Some(b) = RX.iter().find(|b| b.addr() as u16 == s.dma) {
                        b.copy_from(0, &pkt);
                    }
                    s.nesn ^= 1;
                    s.sn ^= 1;
                    c.poke8(RF_SN, s.sn);
                    c.poke8(RF_NESN, s.nesn << 4);
                    c.poke32(RF_TIMESTAMP, c.tick.wrapping_sub(80 * TICKS_PER_US));
                    c.raise16(RF_IRQ_STATUS, STATUS_RX);
                    s.pending_tx = true;
                }
                _ => {}
            }
        })
    });
    st
}

pub(crate) fn att(pdu: &[u8]) -> Vec<u8> {
    [(pdu.len() as u16).to_le_bytes().to_vec(), vec![0x04, 0x00], pdu.to_vec()].concat()
}

impl CentralState {
    pub(crate) fn att_received(&mut self) -> Vec<Vec<u8>> {
        let r = self.received.drain(..).filter(|(llid, p)| *llid == 2 && p.len() >= 4 && p[2] == 4).map(|(_, p)| p[4..].to_vec());
        r.collect()
    }
}

#[test]
fn a_packet_of_the_longest_ll_payload_is_queued_for_sending() {
    let _s = sim::session();
    connection_reset();
    assert!(tx_push(2, &[0x77; LL_PAYLOAD_MAX]));
    assert!(!tx_push(2, &[0x77; LL_PAYLOAD_MAX + 1]));
    assert_eq!(tx_free(), TX_SLOTS - 1);
    let mut buf = [0u8; 6 + LL_PAYLOAD_MAX];
    TX_QUEUE.copy_to(0, &mut buf);
    assert_eq!(buf[..6], [LL_PAYLOAD_MAX as u8 + 2, 0, 0, 0, 2, LL_PAYLOAD_MAX as u8]);
    assert_eq!(buf[6..], [0x77; LL_PAYLOAD_MAX]);
    assert!(tx_push(0x07, &[1]));
    assert_eq!(TX_QUEUE.read(TX_SLOT + 4), 3);
}

#[test]
fn a_full_length_packet_does_not_spill_into_the_next_slot() {
    let _s = sim::session();
    fresh_link();
    for i in 1..RX_SLOTS {
        RX_QUEUE.write(i * RX_SLOT, 3);
        RX_QUEUE.write(i * RX_SLOT + 1, 1);
        RX_QUEUE.write(i * RX_SLOT + 2, i as u8);
    }
    RX_TAIL.set(1);
    RX_HEAD.set(RX_SLOTS as u8);
    let p = rx_packet(0x02, &[0x5a; LL_PAYLOAD_MAX]);
    let ev = event_now(7, 2_500, 30_000);
    run_event(&ev, vec![rx(&p, 1, 1), Step::Status(0x0020)]);
    let got = pop_all();
    assert_eq!(got.len(), RX_SLOTS);
    for (i, (llid, data)) in got[..RX_SLOTS - 1].iter().enumerate() {
        assert_eq!((*llid, data.clone()), (3, vec![i as u8 + 1]));
    }
    assert_eq!(got[RX_SLOTS - 1], (2, vec![0x5a; LL_PAYLOAD_MAX]));
}
