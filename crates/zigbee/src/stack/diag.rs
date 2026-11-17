use super::blackbox;
use super::Stack;
use crate::hal::{Hal, HalCounters, NvItem, ResetCause, TxStatus};
use crate::util::{due, pack, Reader};

pub(crate) mod k {
    pub const PERSISTENT_WRITES: usize = 0;
    pub const MAC_RX_BCAST: usize = 1;
    pub const MAC_TX_BCAST: usize = 2;
    pub const MAC_RX_UCAST: usize = 3;
    pub const MAC_TX_UCAST: usize = 4;
    pub const MAC_TX_UCAST_RETRY: usize = 5;
    pub const MAC_TX_UCAST_FAIL: usize = 6;
    pub const APS_RX_BCAST: usize = 7;
    pub const APS_TX_BCAST: usize = 8;
    pub const APS_RX_UCAST: usize = 9;
    pub const APS_TX_UCAST_SUCCESS: usize = 10;
    pub const APS_TX_UCAST_RETRY: usize = 11;
    pub const APS_TX_UCAST_FAIL: usize = 12;
    pub const ROUTE_DISC_INITIATED: usize = 13;
    pub const NEIGHBOR_ADDED: usize = 14;
    pub const NEIGHBOR_REMOVED: usize = 15;
    pub const NEIGHBOR_STALE: usize = 16;
    pub const JOIN_INDICATION: usize = 17;
    pub const CHILD_MOVED: usize = 18;
    pub const NWK_FC_FAILURE: usize = 19;
    pub const APS_FC_FAILURE: usize = 20;
    pub const APS_UNAUTHORIZED_KEY: usize = 21;
    pub const NWK_DECRYPT_FAILURES: usize = 22;
    pub const APS_DECRYPT_FAILURES: usize = 23;
    pub const BUFFER_ALLOCATE_FAILURES: usize = 24;
    pub const RELAYED_UCAST: usize = 25;
    pub const PHY_TO_MAC_QUEUE_LIMIT: usize = 26;
    pub const PACKET_VALIDATE_DROPS: usize = 27;
    pub const APS_MAC_RETRIES: usize = 28;
    pub const APS_SENT: usize = 29;
    pub const RELAY_DROP_QUEUE_FULL: usize = 30;
    pub const RELAY_DROP_NO_ROUTE: usize = 31;
    pub const RELAY_DROP_MAC_FAIL: usize = 32;
    pub const RELAY_DROP_DUPLICATE: usize = 33;
    pub const NWK_STATUS_SENT: usize = 34;
    pub const BCAST_DROP_TABLE_FULL: usize = 35;
    pub const BCAST_QUEUE_FULL: usize = 36;
    pub const CCA_BUSY: usize = 37;
    pub const CHANNEL_ACCESS_FAILURES: usize = 38;
    pub const RX_QUEUE_PEAK: usize = 39;
    pub const INDIRECT_PEAK: usize = 40;
    pub const INDIRECT_DROPPED: usize = 41;
    pub const INDIRECT_EXPIRED: usize = 42;
    pub const BLE_LENDS: usize = 43;
    pub const BLE_MS: usize = 44;
    pub const FLASH_ERASES: usize = 45;
    pub const COUNT: usize = 46;
}

const WIDTH: [u8; k::COUNT] = {
    let mut w = [2u8; k::COUNT];
    w[k::MAC_RX_BCAST] = 4;
    w[k::MAC_TX_BCAST] = 4;
    w[k::MAC_RX_UCAST] = 4;
    w[k::MAC_TX_UCAST] = 4;
    w[k::APS_MAC_RETRIES] = 4;
    w[k::APS_SENT] = 4;
    w[k::CCA_BUSY] = 4;
    w[k::BLE_LENDS] = 4;
    w[k::BLE_MS] = 4;
    w[k::RX_QUEUE_PEAK] = 1;
    w[k::INDIRECT_PEAK] = 1;
    w
};

const SNAPSHOT_VERSION: u8 = 1;
const BOOT_VERSION: u8 = 1;
pub(crate) const SNAPSHOT_MS: u32 = 3_600_000;
pub(crate) const SNAPSHOT_FIRST_MS: u32 = 1_800_000;
const RETAIN_MS: u32 = 1_000;
pub(crate) const SNAPSHOT_LEN: usize = 1 + {
    let mut n = 0;
    let mut i = 0;
    while i < k::COUNT {
        n += WIDTH[i] as usize;
        i += 1;
    }
    n
};

pub(crate) const CMD_CLEAR: u8 = 0x00;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Src {
    Counter(usize),
    Resets,
    AverageRetries,
    Lqi,
    Rssi,
    Uptime,
    Cause,
    WatchdogResets,
    Neighbors,
    Children,
    RssiBucket(u8),
    NoiseNow,
    NoiseMax,
    CoordNextHop,
    CoordCost,
    CoordManyToOne,
    RoutesUsed,
    NwkStatusRx,
    LastStatusRx,
    LastStatusTx,
    Channel,
    Pan,
    UpdateId,
    KeySeq,
    FrameCounter,
    Depth,
    Parent,
    LoopMax,
    StackFree,
    IrqStackFree,
    BleLendMax,
    FlashBusyMax,
    Ble(u8),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Octets {
    ExtPan,
    NeighborPage(u8),
    BoxPage(u16),
    EventPage(u8),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Attr {
    Number(u8, Src),
    Octets(Octets),
}

impl Attr {
    pub(crate) fn ty(self) -> u8 {
        match self {
            Attr::Number(ty, _) => ty,
            Attr::Octets(_) => T_OCTETS,
        }
    }

    pub(crate) fn number(self) -> Option<(u8, Src)> {
        match self {
            Attr::Number(ty, src) => Some((ty, src)),
            Attr::Octets(_) => None,
        }
    }
}

pub(crate) const RSSI_EDGES: [i8; 4] = [-90, -80, -70, -60];
pub(crate) const RSSI_BUCKETS: usize = 5;
pub(crate) const NOISE_MS: u32 = 60_000;
pub(crate) const NEIGHBORS_PER_PAGE: usize = 4;
pub(crate) const NEIGHBOR_ENTRY: usize = 15;
pub(crate) const NEIGHBOR_PAGES: usize = super::NEIGHBORS / NEIGHBORS_PER_PAGE;
pub(crate) const EVENTS: usize = 30;
pub(crate) const EVENTS_PER_PAGE: usize = 5;
pub(crate) const EVENT_ENTRY: usize = 12;
pub(crate) const EVENT_PAGES: usize = EVENTS / EVENTS_PER_PAGE;
pub const EVENTS_LEN: usize = 2 + EVENTS * EVENT_ENTRY;

pub mod ev {
    pub const START: u8 = 1;
    pub const UP: u8 = 2;
    pub const REJOIN: u8 = 3;
    pub const LEFT: u8 = 4;
    pub const NEIGHBOR_ADDED: u8 = 5;
    pub const NEIGHBOR_STALE: u8 = 6;
    pub const NEIGHBOR_REMOVED: u8 = 7;
    pub const ROUTE_DISCOVERY: u8 = 8;
    pub const STATUS_RX: u8 = 9;
    pub const STATUS_TX: u8 = 10;
    pub const KEY_SWITCH: u8 = 11;
    pub const CHANNEL: u8 = 12;
    pub const PAN: u8 = 13;
    pub const CONFLICT: u8 = 14;
    pub const BLE_CONNECT: u8 = 15;
    pub const BLE_DISCONNECT: u8 = 16;
    pub const BLE_LOST: u8 = 17;
    pub const TX_FAIL: u8 = 18;
    pub const IDENTIFY: u8 = 19;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoggedEvent {
    pub seq: u16,
    pub kind: u8,
    pub a: u16,
    pub b: u16,
    pub t: u32,
    pub this_start: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MacCounters {
    pub rx_ucast: u32,
    pub tx_ucast: u32,
    pub tx_ucast_retries: u32,
    pub tx_ucast_failures: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NeighborCounts {
    pub routers: u8,
    pub end_devices: u8,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Event {
    pub seq: u16,
    pub epoch: u8,
    pub kind: u8,
    pub t: u32,
    pub a: u16,
    pub b: u16,
}

impl Event {
    pub(crate) fn encode(&self, o: &mut [u8]) {
        o[0..2].copy_from_slice(&self.seq.to_le_bytes());
        o[2] = self.epoch;
        o[3] = self.kind;
        o[4..8].copy_from_slice(&self.t.to_le_bytes());
        o[8..10].copy_from_slice(&self.a.to_le_bytes());
        o[10..12].copy_from_slice(&self.b.to_le_bytes());
    }
}

pub(crate) const T_U8: u8 = 0x20;
pub(crate) const T_U16: u8 = 0x21;
pub(crate) const T_U32: u8 = 0x23;
pub(crate) const T_I8: u8 = 0x28;
pub(crate) const T_ENUM8: u8 = 0x30;
pub(crate) const T_OCTETS: u8 = 0x41;

pub(crate) const STANDARD: [(u16, Attr); 32] = [
    (0x0000, Attr::Number(T_U16, Src::Resets)),
    (0x0001, Attr::Number(T_U16, Src::Counter(k::PERSISTENT_WRITES))),
    (0x0100, Attr::Number(T_U32, Src::Counter(k::MAC_RX_BCAST))),
    (0x0101, Attr::Number(T_U32, Src::Counter(k::MAC_TX_BCAST))),
    (0x0102, Attr::Number(T_U32, Src::Counter(k::MAC_RX_UCAST))),
    (0x0103, Attr::Number(T_U32, Src::Counter(k::MAC_TX_UCAST))),
    (0x0104, Attr::Number(T_U16, Src::Counter(k::MAC_TX_UCAST_RETRY))),
    (0x0105, Attr::Number(T_U16, Src::Counter(k::MAC_TX_UCAST_FAIL))),
    (0x0106, Attr::Number(T_U16, Src::Counter(k::APS_RX_BCAST))),
    (0x0107, Attr::Number(T_U16, Src::Counter(k::APS_TX_BCAST))),
    (0x0108, Attr::Number(T_U16, Src::Counter(k::APS_RX_UCAST))),
    (0x0109, Attr::Number(T_U16, Src::Counter(k::APS_TX_UCAST_SUCCESS))),
    (0x010A, Attr::Number(T_U16, Src::Counter(k::APS_TX_UCAST_RETRY))),
    (0x010B, Attr::Number(T_U16, Src::Counter(k::APS_TX_UCAST_FAIL))),
    (0x010C, Attr::Number(T_U16, Src::Counter(k::ROUTE_DISC_INITIATED))),
    (0x010D, Attr::Number(T_U16, Src::Counter(k::NEIGHBOR_ADDED))),
    (0x010E, Attr::Number(T_U16, Src::Counter(k::NEIGHBOR_REMOVED))),
    (0x010F, Attr::Number(T_U16, Src::Counter(k::NEIGHBOR_STALE))),
    (0x0110, Attr::Number(T_U16, Src::Counter(k::JOIN_INDICATION))),
    (0x0111, Attr::Number(T_U16, Src::Counter(k::CHILD_MOVED))),
    (0x0112, Attr::Number(T_U16, Src::Counter(k::NWK_FC_FAILURE))),
    (0x0113, Attr::Number(T_U16, Src::Counter(k::APS_FC_FAILURE))),
    (0x0114, Attr::Number(T_U16, Src::Counter(k::APS_UNAUTHORIZED_KEY))),
    (0x0115, Attr::Number(T_U16, Src::Counter(k::NWK_DECRYPT_FAILURES))),
    (0x0116, Attr::Number(T_U16, Src::Counter(k::APS_DECRYPT_FAILURES))),
    (0x0117, Attr::Number(T_U16, Src::Counter(k::BUFFER_ALLOCATE_FAILURES))),
    (0x0118, Attr::Number(T_U16, Src::Counter(k::RELAYED_UCAST))),
    (0x0119, Attr::Number(T_U16, Src::Counter(k::PHY_TO_MAC_QUEUE_LIMIT))),
    (0x011A, Attr::Number(T_U16, Src::Counter(k::PACKET_VALIDATE_DROPS))),
    (0x011B, Attr::Number(T_U16, Src::AverageRetries)),
    (0x011C, Attr::Number(T_U8, Src::Lqi)),
    (0x011D, Attr::Number(T_I8, Src::Rssi)),
];

pub(crate) const MANUFACTURER: [(u16, Attr); 63] = [
    (0x5000, Attr::Number(T_U32, Src::Uptime)),
    (0x5001, Attr::Number(T_ENUM8, Src::Cause)),
    (0x5002, Attr::Number(T_U16, Src::WatchdogResets)),
    (0x5003, Attr::Number(T_U8, Src::Neighbors)),
    (0x5004, Attr::Number(T_U8, Src::Children)),
    (0x5005, Attr::Number(T_U16, Src::Counter(k::RELAY_DROP_QUEUE_FULL))),
    (0x5006, Attr::Number(T_U16, Src::Counter(k::RELAY_DROP_NO_ROUTE))),
    (0x5007, Attr::Number(T_U16, Src::Counter(k::RELAY_DROP_MAC_FAIL))),
    (0x5008, Attr::Number(T_U16, Src::Counter(k::RELAY_DROP_DUPLICATE))),
    (0x5009, Attr::Number(T_U16, Src::Counter(k::NWK_STATUS_SENT))),
    (0x500A, Attr::Number(T_U16, Src::Counter(k::BCAST_DROP_TABLE_FULL))),
    (0x500B, Attr::Number(T_U16, Src::Counter(k::BCAST_QUEUE_FULL))),
    (0x500C, Attr::Number(T_U32, Src::Counter(k::CCA_BUSY))),
    (0x500D, Attr::Number(T_U16, Src::Counter(k::CHANNEL_ACCESS_FAILURES))),
    (0x500E, Attr::Number(T_U8, Src::Counter(k::RX_QUEUE_PEAK))),
    (0x500F, Attr::Number(T_U8, Src::Counter(k::INDIRECT_PEAK))),
    (0x5010, Attr::Number(T_U16, Src::Counter(k::INDIRECT_DROPPED))),
    (0x5011, Attr::Number(T_U16, Src::Counter(k::INDIRECT_EXPIRED))),
    (0x5012, Attr::Number(T_U32, Src::Counter(k::BLE_LENDS))),
    (0x5013, Attr::Number(T_U32, Src::Counter(k::BLE_MS))),
    (0x5014, Attr::Number(T_U16, Src::Counter(k::FLASH_ERASES))),
    (0x5015, Attr::Number(T_U32, Src::RssiBucket(0))),
    (0x5016, Attr::Number(T_U32, Src::RssiBucket(1))),
    (0x5017, Attr::Number(T_U32, Src::RssiBucket(2))),
    (0x5018, Attr::Number(T_U32, Src::RssiBucket(3))),
    (0x5019, Attr::Number(T_U32, Src::RssiBucket(4))),
    (0x501A, Attr::Number(T_I8, Src::NoiseNow)),
    (0x501B, Attr::Number(T_I8, Src::NoiseMax)),
    (0x5020, Attr::Number(T_U16, Src::CoordNextHop)),
    (0x5021, Attr::Number(T_U8, Src::CoordCost)),
    (0x5022, Attr::Number(T_U8, Src::CoordManyToOne)),
    (0x5023, Attr::Number(T_U8, Src::RoutesUsed)),
    (0x5024, Attr::Number(T_U16, Src::NwkStatusRx)),
    (0x5025, Attr::Number(T_U8, Src::LastStatusRx)),
    (0x5026, Attr::Number(T_U8, Src::LastStatusTx)),
    (0x5030, Attr::Number(T_U8, Src::Channel)),
    (0x5031, Attr::Number(T_U16, Src::Pan)),
    (0x5032, Attr::Octets(Octets::ExtPan)),
    (0x5033, Attr::Number(T_U8, Src::UpdateId)),
    (0x5034, Attr::Number(T_U8, Src::KeySeq)),
    (0x5035, Attr::Number(T_U32, Src::FrameCounter)),
    (0x5036, Attr::Number(T_U8, Src::Depth)),
    (0x5037, Attr::Number(T_U16, Src::Parent)),
    (0x5040, Attr::Number(T_U32, Src::LoopMax)),
    (0x5041, Attr::Number(T_U32, Src::BleLendMax)),
    (0x5042, Attr::Number(T_U32, Src::FlashBusyMax)),
    (0x5043, Attr::Number(T_U32, Src::StackFree)),
    (0x5044, Attr::Number(T_U16, Src::IrqStackFree)),
    (0x5048, Attr::Number(T_U32, Src::Ble(0))),
    (0x5049, Attr::Number(T_U32, Src::Ble(1))),
    (0x504A, Attr::Number(T_U32, Src::Ble(2))),
    (0x504B, Attr::Number(T_U32, Src::Ble(3))),
    (0x504C, Attr::Number(T_U32, Src::Ble(4))),
    (0x5050, Attr::Octets(Octets::NeighborPage(0))),
    (0x5051, Attr::Octets(Octets::NeighborPage(1))),
    (0x5052, Attr::Octets(Octets::NeighborPage(2))),
    (0x5053, Attr::Octets(Octets::NeighborPage(3))),
    (0x5058, Attr::Octets(Octets::EventPage(0))),
    (0x5059, Attr::Octets(Octets::EventPage(1))),
    (0x505A, Attr::Octets(Octets::EventPage(2))),
    (0x505B, Attr::Octets(Octets::EventPage(3))),
    (0x505C, Attr::Octets(Octets::EventPage(4))),
    (0x505D, Attr::Octets(Octets::EventPage(5))),
];
const _: () = assert!(NEIGHBOR_PAGES == 4 && EVENT_PAGES == 6, "the attribute table lists every page");

const COUNTERS_MFR: usize = 21;
const COUNTERS_VERSION: u8 = 1;
const _: () = assert!(MANUFACTURER[COUNTERS_MFR - 1].0 == 0x5014, "the counters end at FlashErases");

const fn width(ty: u8) -> usize {
    match ty {
        T_U8 | T_I8 | T_ENUM8 => 1,
        T_U16 => 2,
        _ => 4,
    }
}

const fn counter_width(a: Attr) -> usize {
    match a {
        Attr::Number(ty, _) => {
            assert!(matches!(ty, T_U8 | T_I8 | T_ENUM8 | T_U16 | T_U32), "a number");
            width(ty)
        }
        Attr::Octets(_) => panic!("a number"),
    }
}

pub const DIAG_COUNTERS_MAX: usize = {
    let mut n = 1;
    let mut i = 0;
    while i < STANDARD.len() {
        n += 3 + counter_width(STANDARD[i].1);
        i += 1;
    }
    let mut i = 0;
    while i < COUNTERS_MFR {
        n += 3 + counter_width(MANUFACTURER[i].1);
        i += 1;
    }
    n
};

pub(crate) fn attribute(manufacturer: bool, id: u16) -> Option<Attr> {
    let table: &[(u16, Attr)] = if manufacturer { &MANUFACTURER } else { &STANDARD };
    if manufacturer && id.wrapping_sub(blackbox::BOX_FIRST) < blackbox::BOX_PAGES as u16 {
        return Some(Attr::Octets(Octets::BoxPage(id - blackbox::BOX_FIRST)));
    }
    table.iter().find(|a| a.0 == id).map(|a| a.1)
}

pub(crate) fn attributes(manufacturer: bool) -> &'static [(u16, Attr)] {
    if manufacturer {
        &MANUFACTURER
    } else {
        &STANDARD
    }
}

pub(crate) struct Diag {
    pub n: [u32; k::COUNT],
    pub resets: u16,
    pub watchdog_resets: u16,
    pub cause: ResetCause,
    pub last_attempts: u8,
    hal_seen: HalCounters,
    started: u32,
    snapshot_at: u32,
    retain_at: u32,
    pub rssi: [u32; RSSI_BUCKETS],
    pub noise_now: i8,
    pub noise_max: i8,
    pub noise_at: u32,
    pub status_rx: u32,
    pub last_status_rx: u8,
    pub last_status_tx: u8,
    pub events: [Event; EVENTS],
    pub ev_head: u8,
    pub ev_len: u8,
    pub ev_seq: u16,
}

impl Diag {
    pub(crate) const fn new(now: u32) -> Diag {
        Diag {
            n: [0; k::COUNT],
            resets: 0,
            watchdog_resets: 0,
            cause: ResetCause::PowerOn,
            last_attempts: 0,
            hal_seen: HalCounters {
                cca_busy: 0,
                rx_queue_full: 0,
                rx_queue_peak: 0,
                ble_lends: 0,
                ble_ms: 0,
                flash_erases: 0,
                flash_stores: 0,
                loop_max_us: 0,
                ble_lend_max_us: 0,
                flash_busy_max_us: 0,
                ble_connections: 0,
                ble_disconnects: 0,
                ble_links_lost: 0,
                ble_events: 0,
                ble_events_skipped: 0,
                stack_free: 0,
                irq_stack_free: 0,
            },
            started: now,
            snapshot_at: now.wrapping_add(SNAPSHOT_FIRST_MS),
            retain_at: now.wrapping_add(RETAIN_MS),
            rssi: [0; RSSI_BUCKETS],
            noise_now: i8::MIN,
            noise_max: i8::MIN,
            noise_at: now.wrapping_add(NOISE_MS),
            status_rx: 0,
            last_status_rx: 0,
            last_status_tx: 0,
            events: [Event { seq: 0, epoch: 0, kind: 0, t: 0, a: 0, b: 0 }; EVENTS],
            ev_head: 0,
            ev_len: 0,
            ev_seq: 0,
        }
    }

    pub(crate) fn rssi_add(&mut self, rssi: i8) {
        let i = RSSI_EDGES.iter().position(|&e| rssi < e).unwrap_or(RSSI_BUCKETS - 1);
        self.rssi[i] = self.rssi[i].wrapping_add(1);
    }

    pub(crate) fn event(&mut self, now: u32, kind: u8, a: u16, b: u16) {
        let e = Event { seq: self.ev_seq, epoch: self.resets as u8, kind, t: self.uptime_s(now), a, b };
        self.ev_seq = self.ev_seq.wrapping_add(1);
        self.events[self.ev_head as usize] = e;
        self.ev_head = ((self.ev_head as usize + 1) % EVENTS) as u8;
        self.ev_len = (self.ev_len as usize + 1).min(EVENTS) as u8;
    }

    pub(crate) fn event_newest(&self, i: usize) -> Option<Event> {
        if i >= self.ev_len as usize {
            return None;
        }
        Some(self.events[(self.ev_head as usize + EVENTS - 1 - i) % EVENTS])
    }

    pub(crate) fn encode_events(&self, out: &mut [u8; EVENTS_LEN]) {
        out[0] = self.ev_head;
        out[1] = self.ev_len;
        for (i, e) in self.events.iter().enumerate() {
            e.encode(&mut out[2 + i * EVENT_ENTRY..2 + (i + 1) * EVENT_ENTRY]);
        }
    }

    pub(crate) fn decode_events(&mut self, b: &[u8]) {
        if b.len() < EVENTS_LEN || b[0] as usize >= EVENTS || b[1] as usize > EVENTS {
            return;
        }
        self.ev_head = b[0];
        self.ev_len = b[1];
        for (i, e) in self.events.iter_mut().enumerate() {
            let o = &b[2 + i * EVENT_ENTRY..2 + (i + 1) * EVENT_ENTRY];
            *e = Event {
                seq: u16::from_le_bytes([o[0], o[1]]),
                epoch: o[2],
                kind: o[3],
                t: u32::from_le_bytes([o[4], o[5], o[6], o[7]]),
                a: u16::from_le_bytes([o[8], o[9]]),
                b: u16::from_le_bytes([o[10], o[11]]),
            };
        }
        if let Some(newest) = self.event_newest(0) {
            self.ev_seq = newest.seq.wrapping_add(1);
        }
    }

    #[inline]
    pub(crate) fn add(&mut self, i: usize) {
        self.n[i] = self.n[i].wrapping_add(1);
    }

    #[inline]
    pub(crate) fn add_n(&mut self, i: usize, v: u32) {
        self.n[i] = self.n[i].wrapping_add(v);
    }

    pub(crate) fn peak(&mut self, i: usize, v: u32) {
        if v > self.n[i] {
            self.n[i] = v;
        }
    }

    pub(crate) fn take_hal(&mut self, c: HalCounters) {
        let s = self.hal_seen;
        self.add_n(k::CCA_BUSY, c.cca_busy.wrapping_sub(s.cca_busy));
        self.add_n(k::PHY_TO_MAC_QUEUE_LIMIT, c.rx_queue_full.wrapping_sub(s.rx_queue_full));
        self.add_n(k::BLE_LENDS, c.ble_lends.wrapping_sub(s.ble_lends));
        self.add_n(k::BLE_MS, c.ble_ms.wrapping_sub(s.ble_ms));
        self.add_n(k::FLASH_ERASES, c.flash_erases.wrapping_sub(s.flash_erases));
        self.add_n(k::PERSISTENT_WRITES, c.flash_stores.wrapping_sub(s.flash_stores));
        self.peak(k::RX_QUEUE_PEAK, c.rx_queue_peak.min(255));
        self.hal_seen = c;
    }

    pub(crate) fn ble_events(&mut self, now: u32, before: HalCounters) {
        let c = self.hal_seen;
        if c.ble_connections != before.ble_connections {
            self.event(now, ev::BLE_CONNECT, c.ble_connections as u16, 0);
        }
        if c.ble_links_lost != before.ble_links_lost {
            self.event(now, ev::BLE_LOST, c.ble_links_lost as u16, 0);
        } else if c.ble_disconnects != before.ble_disconnects {
            self.event(now, ev::BLE_DISCONNECT, c.ble_disconnects as u16, 0);
        }
    }

    pub(crate) fn average_retries(&self) -> u16 {
        let sent = self.n[k::APS_SENT];
        if sent == 0 {
            return 0;
        }
        let r = self.n[k::APS_MAC_RETRIES] as u64;
        ((r * 2 + sent as u64) / (sent as u64 * 2)).min(0xFFFF) as u16
    }

    pub(crate) fn average_retries_x100(&self) -> u32 {
        let sent = self.n[k::APS_SENT];
        if sent == 0 {
            return 0;
        }
        ((self.n[k::APS_MAC_RETRIES] as u64 * 100 / sent as u64).min(u32::MAX as u64)) as u32
    }

    pub(crate) fn ble_ms_since_start(&self) -> u32 {
        self.hal_seen.ble_ms
    }

    pub(crate) fn uptime_s(&self, now: u32) -> u32 {
        now.wrapping_sub(self.started) / 1000
    }

    pub(crate) fn relays_lost(&self) -> u32 {
        self.n[k::RELAY_DROP_QUEUE_FULL]
            .wrapping_add(self.n[k::RELAY_DROP_NO_ROUTE])
            .wrapping_add(self.n[k::RELAY_DROP_MAC_FAIL])
    }

    fn encode(&self, out: &mut [u8; SNAPSHOT_LEN]) {
        out[0] = SNAPSHOT_VERSION;
        let mut at = 1;
        for (i, w) in WIDTH.iter().enumerate() {
            let b = self.n[i].to_le_bytes();
            out[at..at + *w as usize].copy_from_slice(&b[..*w as usize]);
            at += *w as usize;
        }
    }

    fn decode(&mut self, b: &[u8]) -> bool {
        let mut r = Reader::new(b);
        if r.u8() != Some(SNAPSHOT_VERSION) {
            return false;
        }
        let mut n = [0u32; k::COUNT];
        for (i, w) in WIDTH.iter().enumerate() {
            let Some(s) = r.take(*w as usize) else {
                return false;
            };
            let mut v = [0u8; 4];
            v[..s.len()].copy_from_slice(s);
            n[i] = u32::from_le_bytes(v);
        }
        self.n = n;
        true
    }
}

impl<H: Hal> Stack<H> {
    pub(crate) fn mac_transmit(&mut self, mpdu: &[u8]) -> TxStatus {
        let status = self.hal.transmit(mpdu);
        let attempts = self.hal.tx_attempts();
        self.diag.last_attempts = attempts;
        let unicast = mpdu.first().is_some_and(|fc| fc & 0x20 != 0);
        let sent = attempts > 0 || status != TxStatus::ChannelBusy;
        if unicast {
            if sent {
                self.diag.add(k::MAC_TX_UCAST);
            }
            self.diag.add_n(k::MAC_TX_UCAST_RETRY, attempts.saturating_sub(1) as u32);
            if status != TxStatus::Ok {
                self.diag.add(k::MAC_TX_UCAST_FAIL);
            }
            if mpdu.len() >= 7 && (mpdu[1] >> 2) & 0x03 == 2 {
                let dst = u16::from_le_bytes([mpdu[5], mpdu[6]]);
                if let Some(i) = self.neighbor_index(dst) {
                    let n = &mut self.neighbors[i];
                    n.tx = n.tx.saturating_add(sent as u16);
                    n.retries = n.retries.saturating_add(attempts.saturating_sub(1) as u16);
                    n.delivery_sample(attempts, status == TxStatus::Ok);
                    let now = self.hal.now_ms();
                    if let Some(b) = self.hal.black_box() {
                        b.link_frame(now, i, attempts, status == TxStatus::Ok);
                    }
                    if status != TxStatus::Ok {
                        n.fails = n.fails.saturating_add(1);
                        let now = self.hal.now_ms();
                        self.diag.event(now, ev::TX_FAIL, dst, status as u16);
                    }
                }
            }
        } else if sent {
            self.diag.add(k::MAC_TX_BCAST);
        }
        if status == TxStatus::ChannelBusy {
            self.diag.add(k::CHANNEL_ACCESS_FAILURES);
        }
        status
    }

    pub(crate) fn diag_start(&mut self) -> [u8; 5] {
        let cause = self.hal.reset_cause();
        self.diag.cause = cause;
        let mut buf = [0u8; super::RETAIN_MAX];
        let mut restored = false;
        if cause != ResetCause::PowerOn {
            if let Some(n) = self.hal.retained(&mut buf) {
                restored = self.diag.decode(&buf[..n.min(SNAPSHOT_LEN)]);
                if restored && n >= super::RETAIN_MAX {
                    self.diag.decode_events(&buf[SNAPSHOT_LEN..super::RETAIN_MAX]);
                }
            }
        }
        let mut buf = [0u8; SNAPSHOT_LEN];
        if !restored {
            if let Some(n) = self.hal.nv_load(NvItem::Diagnostics, &mut buf) {
                self.diag.decode(&buf[..n.min(SNAPSHOT_LEN)]);
            }
        }
        let mut boot = [0u8; 5];
        if self.hal.nv_load(NvItem::Boot, &mut boot) == Some(5) && boot[0] == BOOT_VERSION {
            self.diag.resets = u16::from_le_bytes([boot[1], boot[2]]);
            self.diag.watchdog_resets = u16::from_le_bytes([boot[3], boot[4]]);
        }
        self.diag.resets = self.diag.resets.wrapping_add(1);
        if cause == ResetCause::Watchdog {
            self.diag.watchdog_resets = self.diag.watchdog_resets.wrapping_add(1);
        }
        self.diag.hal_seen = HalCounters::default();
        let now = self.hal.now_ms();
        self.diag.event(now, ev::START, cause as u16, self.diag.resets);
        self.boot_record()
    }

    fn boot_record(&self) -> [u8; 5] {
        let r = self.diag.resets.to_le_bytes();
        let w = self.diag.watchdog_resets.to_le_bytes();
        [BOOT_VERSION, r[0], r[1], w[0], w[1]]
    }

    pub(crate) fn diag_started(&mut self) {
        if self.diag.cause != ResetCause::PowerOn {
            self.store_diagnostics();
        }
    }

    pub(crate) fn store_diagnostics(&mut self) {
        self.diag_sync();
        let mut b = [0u8; SNAPSHOT_LEN];
        self.diag.encode(&mut b);
        self.hal.nv_store(NvItem::Diagnostics, &b);
        self.diag.add(k::PERSISTENT_WRITES);
    }

    pub(crate) fn nv_written(&mut self) {
        self.diag.add(k::PERSISTENT_WRITES);
    }

    pub(crate) fn diag_sync(&mut self) {
        let c = self.hal.counters();
        let before = self.diag.hal_seen;
        self.diag.take_hal(c);
        let now = self.hal.now_ms();
        self.diag.ble_events(now, before);
    }

    pub(crate) fn log_event(&mut self, kind: u8, a: u16, b: u16) {
        let now = self.hal.now_ms();
        self.diag.event(now, kind, a, b);
    }

    pub(crate) fn diag_noise(&mut self, now: u32) {
        if !due(now, self.diag.noise_at) {
            return;
        }
        let Some(dbm) = self.hal.noise_dbm() else {
            self.diag.noise_at = now.wrapping_add(1_000);
            return;
        };
        self.diag.noise_at = now.wrapping_add(NOISE_MS);
        self.diag.noise_now = dbm;
        if self.diag.noise_max == i8::MIN || dbm > self.diag.noise_max {
            self.diag.noise_max = dbm;
        }
    }

    pub(crate) fn coordinator_route(&self) -> (u16, u8, bool) {
        if let Some(i) = self.neighbor_index(0x0000) {
            let n = &self.neighbors[i];
            if n.used && n.age <= super::AGE_LIMIT {
                return (0x0000, n.link_cost(), false);
            }
        }
        match self.routes.iter().find(|r| r.status == super::ROUTE_ACTIVE && r.dst == 0x0000) {
            Some(r) => {
                let cost = self
                    .neighbor_index(r.next)
                    .map(|i| self.neighbors[i].link_cost())
                    .unwrap_or(0);
                (r.next, cost, r.many_to_one)
            }
            None => (0xFFFF, 0, false),
        }
    }

    pub(crate) fn box_timers(&mut self, now: u32) {
        let Some(b) = self.hal.black_box() else { return };
        if !due(now, b.row_at) || b.held(now) {
            return;
        }
        b.row_at = now.wrapping_add(blackbox::ROW_MS);
        self.diag_sync();
        let n = &self.diag.n;
        let totals = blackbox::Totals {
            tx: n[k::MAC_TX_UCAST],
            rx: n[k::MAC_RX_UCAST].wrapping_add(n[k::MAC_RX_BCAST]),
            retries: n[k::MAC_TX_UCAST_RETRY],
            fails: n[k::MAC_TX_UCAST_FAIL],
            route_disc: n[k::ROUTE_DISC_INITIATED],
        };
        let (hop, cost, _) = self.coordinator_route();
        let (measured, delivery) = match self.neighbor_index(hop) {
            Some(i) if hop != 0xFFFF => (self.neighbors[i].measured(), (self.neighbors[i].delivery >> 8) as u8),
            _ => (false, 0),
        };
        let stale = self.neighbors.iter().filter(|n| n.used && n.confirmed && n.age > super::AGE_LIMIT).count() as u8;
        let row = blackbox::RowNow {
            now,
            totals,
            coord_hop: hop,
            coord_cost: cost,
            coord_measured: measured,
            coord_delivery: delivery,
            stale,
            noise: self.diag.noise_now,
        };
        if let Some(b) = self.hal.black_box() {
            b.row(row);
        }
    }

    pub(crate) fn diag_bytes(&mut self, src: Octets, out: &mut [u8; 64]) -> usize {
        match src {
            Octets::ExtPan => {
                out[..8].copy_from_slice(&self.nib.ext_pan.to_le_bytes());
                8
            }
            Octets::BoxPage(p) => {
                let mut shorts = [0xFFFFu16; super::NEIGHBORS];
                for (s, n) in shorts.iter_mut().zip(self.neighbors.iter()) {
                    if n.used {
                        *s = n.short;
                    }
                }
                let now = self.hal.now_ms();
                match self.hal.black_box() {
                    Some(b) => b.page(p, now, &shorts, out),
                    None => 0,
                }
            }
            Octets::NeighborPage(p) => {
                let mut n = 0;
                let from = p as usize * NEIGHBORS_PER_PAGE;
                for nb in self.neighbors[from..from + NEIGHBORS_PER_PAGE].iter() {
                    let o = &mut out[n..n + NEIGHBOR_ENTRY];
                    let child = nb.ext != 0 && self.child_by_ext(nb.ext).is_some();
                    o[0..2].copy_from_slice(&nb.short.to_le_bytes());
                    o[2] = nb.lqi;
                    o[3] = nb.out_cost;
                    o[4] = nb.age;
                    o[5] = pack(&[
                        (nb.used as u16, 0),
                        (nb.relays as u16, 1),
                        (nb.listed as u16, 2),
                        (child as u16, 3),
                        ((nb.link_cost() & 7) as u16, 4),
                        (nb.measured() as u16, 7),
                    ]) as u8;
                    o[6] = nb.stales;
                    o[7..9].copy_from_slice(&nb.rx.to_le_bytes());
                    o[9] = nb.rssi_avg as u8;
                    o[10] = nb.rssi_min as u8;
                    o[11] = nb.tx.min(255) as u8;
                    o[12] = nb.retries.min(255) as u8;
                    o[13] = nb.fails.min(255) as u8;
                    o[14] = (nb.delivery >> 8) as u8;
                    n += NEIGHBOR_ENTRY;
                }
                n
            }
            Octets::EventPage(p) => {
                let mut n = 0;
                for i in 0..EVENTS_PER_PAGE {
                    let Some(e) = self.diag.event_newest(p as usize * EVENTS_PER_PAGE + i) else { break };
                    e.encode(&mut out[n..n + EVENT_ENTRY]);
                    n += EVENT_ENTRY;
                }
                n
            }
        }
    }

    pub(crate) fn diag_timers(&mut self, now: u32) {
        if due(now, self.diag.retain_at) {
            self.diag.retain_at = now.wrapping_add(RETAIN_MS);
            self.diag_retain_now();
        }
        if due(now, self.diag.snapshot_at) {
            self.diag.snapshot_at = now.wrapping_add(SNAPSHOT_MS);
            self.store_diagnostics();
        }
    }

    pub(crate) fn diag_retain_now(&mut self) {
        self.diag_sync();
        let mut b = [0u8; super::RETAIN_MAX];
        let mut snap = [0u8; SNAPSHOT_LEN];
        self.diag.encode(&mut snap);
        b[..SNAPSHOT_LEN].copy_from_slice(&snap);
        let mut evs = [0u8; EVENTS_LEN];
        self.diag.encode_events(&mut evs);
        b[SNAPSHOT_LEN..].copy_from_slice(&evs);
        self.hal.retain(&b);
    }

    pub(crate) fn diag_keep(&mut self) {
        let boot = self.boot_record();
        self.nv_put(NvItem::Boot, &boot);
        self.store_diagnostics();
    }

    pub(crate) fn diag_clear(&mut self) {
        self.diag_sync();
        self.diag.n = [0; k::COUNT];
        self.diag.resets = 0;
        self.diag.watchdog_resets = 0;
        let boot = self.boot_record();
        self.nv_put(NvItem::Boot, &boot);
        self.store_diagnostics();
    }

    pub fn uptime_s(&mut self) -> u32 {
        let now = self.hal.now_ms();
        self.diag.uptime_s(now)
    }

    pub fn reset_cause(&self) -> ResetCause {
        self.diag.cause
    }

    pub fn resets(&self) -> (u16, u16) {
        (self.diag.resets, self.diag.watchdog_resets)
    }

    pub fn neighbor_counts(&self) -> NeighborCounts {
        let end_devices = self.children.iter().filter(|c| c.used && c.joined && c.capability & 0x02 == 0).count();
        NeighborCounts { routers: self.live_neighbors().min(255) as u8, end_devices: end_devices.min(255) as u8 }
    }

    pub fn last_signal(&self) -> (u8, i8) {
        self.rx_signal
    }

    pub fn mac_counters(&self) -> MacCounters {
        let n = &self.diag.n;
        MacCounters {
            rx_ucast: n[k::MAC_RX_UCAST],
            tx_ucast: n[k::MAC_TX_UCAST],
            tx_ucast_retries: n[k::MAC_TX_UCAST_RETRY],
            tx_ucast_failures: n[k::MAC_TX_UCAST_FAIL],
        }
    }

    pub fn event(&self, i: usize) -> Option<LoggedEvent> {
        let e = self.diag.event_newest(i)?;
        Some(LoggedEvent { seq: e.seq, kind: e.kind, a: e.a, b: e.b, t: e.t, this_start: e.epoch == self.diag.resets as u8 })
    }

    pub fn event_seq(&self) -> u16 {
        self.diag.ev_seq
    }

    pub fn log_identify(&mut self) {
        self.log_event(ev::IDENTIFY, 0, 0);
    }

    pub fn keep_for_restart(&mut self) {
        self.diag_retain_now();
    }

    pub fn sync_hardware_counters(&mut self) {
        self.diag_sync();
    }

    pub(crate) fn live_neighbors(&self) -> usize {
        self.neighbors.iter().filter(|n| n.used && n.age <= super::AGE_LIMIT).count()
    }

    pub(crate) fn diag_value(&mut self, src: Src) -> u32 {
        self.diag_sync();
        self.diag_value_synced(src)
    }

    pub fn diag_counters(&mut self, out: &mut [u8]) -> usize {
        if out.len() < DIAG_COUNTERS_MAX {
            return 0;
        }
        self.diag_sync();
        out[0] = COUNTERS_VERSION;
        let mut at = 1;
        for (id, ty, src) in STANDARD.iter().chain(MANUFACTURER[..COUNTERS_MFR].iter()).filter_map(|&(id, a)| a.number().map(|(ty, src)| (id, ty, src))) {
            let w = width(ty);
            let v = self.diag_value_synced(src).to_le_bytes();
            out[at..at + 2].copy_from_slice(&id.to_le_bytes());
            out[at + 2] = ty;
            out[at + 3..at + 3 + w].copy_from_slice(&v[..w]);
            at += 3 + w;
        }
        at
    }

    fn diag_value_synced(&mut self, src: Src) -> u32 {
        let now = self.hal.now_ms();
        match src {
            Src::Counter(i) => self.diag.n[i],
            Src::Resets => self.diag.resets as u32,
            Src::AverageRetries => self.diag.average_retries() as u32,
            Src::Lqi => self.rx_signal.0 as u32,
            Src::Rssi => self.rx_signal.1 as u8 as u32,
            Src::Uptime => self.diag.uptime_s(now),
            Src::Cause => self.diag.cause as u32,
            Src::WatchdogResets => self.diag.watchdog_resets as u32,
            Src::Neighbors => self.live_neighbors() as u32,
            Src::Children => self.child_count() as u32,
            Src::RssiBucket(i) => self.diag.rssi[i as usize],
            Src::NoiseNow => self.diag.noise_now as u8 as u32,
            Src::NoiseMax => self.diag.noise_max as u8 as u32,
            Src::CoordNextHop => self.coordinator_route().0 as u32,
            Src::CoordCost => self.coordinator_route().1 as u32,
            Src::CoordManyToOne => self.coordinator_route().2 as u32,
            Src::RoutesUsed => self.routes.iter().filter(|r| r.status != super::ROUTE_FREE).count() as u32,
            Src::NwkStatusRx => self.diag.status_rx,
            Src::LastStatusRx => self.diag.last_status_rx as u32,
            Src::LastStatusTx => self.diag.last_status_tx as u32,
            Src::Channel => self.nib.channel as u32,
            Src::Pan => self.nib.pan as u32,
            Src::UpdateId => self.nib.update_id as u32,
            Src::KeySeq => self.nib.key_seq as u32,
            Src::FrameCounter => self.fc_out,
            Src::Depth => self.nib.depth as u32,
            Src::Parent => self.nib.parent as u32,
            Src::LoopMax => self.diag.hal_seen.loop_max_us,
            Src::StackFree => self.diag.hal_seen.stack_free,
            Src::IrqStackFree => self.diag.hal_seen.irq_stack_free,
            Src::BleLendMax => self.diag.hal_seen.ble_lend_max_us,
            Src::FlashBusyMax => self.diag.hal_seen.flash_busy_max_us,
            Src::Ble(i) => {
                let c = &self.diag.hal_seen;
                [c.ble_connections, c.ble_disconnects, c.ble_links_lost, c.ble_events, c.ble_events_skipped][i as usize]
            }
        }
    }
}

#[cfg(test)]
mod tests;
