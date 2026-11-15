//! The Diagnostics cluster (ZCL revision 8, clause 3.15): what the stack
//! counts while it works, how the counters survive a restart, and the
//! manufacturer-specific attributes for what the standard set does not say
//! about a router under load (docs/diagnostics.md).
//!
//! Counters live in RAM as `u32` and only grow, wrapping as the text
//! expects ("many of the counters ... will wrap quickly", 3.15.2.2.2); an
//! attribute of type uint16 shows the low 16 bits. They are kept across a
//! restart: a snapshot goes to the non-volatile store about once an hour
//! and once after a watchdog or software reset (3.15.1 recommends
//! persistent server attributes so that "what was causing the reset
//! behavior" can be analysed), the first time half an hour after the start,
//! and every second into RAM that a warm restart leaves alone
//! (`Hal::retain`), so a watchdog reset loses at most a second of counting.
//! A power cut loses what was counted since the last snapshot. NumberOfResets is exact: it is stored at every start, in the
//! same flash write as the frame counters (`Hal::nv_store_pair`). A factory
//! reset clears everything (3.15.2.2.1.1); so does the manufacturer-specific
//! command `CMD_CLEAR` (the server has no commands of its own, 3.15.2.3).

use super::blackbox;
use super::Stack;
use crate::hal::{Hal, HalCounters, NvItem, ResetCause, TxStatus};
use crate::util::{due, Reader};

/// Counter indices (`Diag::n`).
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
    /// MAC retransmissions of the APS messages this node originated, and
    /// how many such messages: AverageMACRetryPerAPSMessageSent.
    pub const APS_MAC_RETRIES: usize = 28;
    pub const APS_SENT: usize = 29;
    // manufacturer-specific
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

/// Bytes each counter takes in the stored snapshot: as wide as its
/// attribute (the low bits are what the attribute shows), 4 for the two
/// that make the average.
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
/// How often the snapshot goes to the non-volatile store, and when the
/// first one goes after a start: a bulb on a wall switch that is on for
/// half an hour keeps what it counted, and one switched every few minutes
/// costs no writes (its NumberOfResets is exact all the same).
pub(crate) const SNAPSHOT_MS: u32 = 3_600_000;
pub(crate) const SNAPSHOT_FIRST_MS: u32 = 1_800_000;
/// How often the snapshot goes to the RAM that survives a warm restart.
const RETAIN_MS: u32 = 1_000;
/// Longest stored snapshot: version, then the counters.
pub(crate) const SNAPSHOT_LEN: usize = 1 + {
    let mut n = 0;
    let mut i = 0;
    while i < k::COUNT {
        n += WIDTH[i] as usize;
        i += 1;
    }
    n
};

/// Manufacturer-specific command of the Diagnostics cluster (with our
/// manufacturer code): sets every counter back to 0, NumberOfResets too.
pub(crate) const CMD_CLEAR: u8 = 0x00;

/// What a Diagnostics attribute shows.
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
    /// Received frames by signal strength (`RSSI_EDGES`).
    RssiBucket(u8),
    /// The channel's noise floor, dBm: the last measurement, the highest since the start.
    NoiseNow,
    NoiseMax,
    /// The way to the coordinator: first hop (0xFFFF none), its link cost,
    /// whether the route came from a many-to-one request.
    CoordNextHop,
    CoordCost,
    CoordManyToOne,
    RoutesUsed,
    NwkStatusRx,
    LastStatusRx,
    LastStatusTx,
    // network parameters, to compare with the coordinator's
    Channel,
    Pan,
    ExtPan,
    UpdateId,
    KeySeq,
    FrameCounter,
    Depth,
    Parent,
    /// What the hardware layer measured: the longest main loop round, the
    /// longest loan of the radio to BLE, the longest flash erase (µs).
    LoopMax,
    StackFree,
    IrqStackFree,
    BleLendMax,
    FlashBusyMax,
    /// Bluetooth LE: connections, disconnections, links lost (the central
    /// went silent), connection events run and skipped.
    Ble(u8),
    /// Tables in pages (octet strings): the neighbor table and the event log.
    NeighborPage(u8),
    /// A page of the black box (`blackbox.rs`).
    BoxPage(u16),
    EventPage(u8),
}

/// RSSI bucket edges, dBm: below -90, -90..-80, -80..-70, -70..-60, -60 and above.
pub(crate) const RSSI_EDGES: [i8; 4] = [-90, -80, -70, -60];
pub(crate) const RSSI_BUCKETS: usize = 5;
/// How often the noise floor is measured (energy detection on the channel in a quiet moment).
pub(crate) const NOISE_MS: u32 = 60_000;
/// Neighbor table pages: entries a page, bytes an entry.
pub(crate) const NEIGHBORS_PER_PAGE: usize = 4;
pub(crate) const NEIGHBOR_ENTRY: usize = 15;
pub(crate) const NEIGHBOR_PAGES: usize = super::NEIGHBORS / NEIGHBORS_PER_PAGE;
/// The event log: entries, entries a page, bytes an entry.
pub(crate) const EVENTS: usize = 30;
pub(crate) const EVENTS_PER_PAGE: usize = 5;
pub(crate) const EVENT_ENTRY: usize = 12;
pub(crate) const EVENT_PAGES: usize = EVENTS / EVENTS_PER_PAGE;
/// Bytes of the event log in the RAM a warm restart keeps: head, count, entries.
pub(crate) const EVENTS_LEN: usize = 2 + EVENTS * EVENT_ENTRY;

/// What the event log records (`Event::kind`); `a` and `b` say more.
pub mod ev {
    /// The stack started: a = reset cause, b = NumberOfResets.
    pub const START: u8 = 1;
    /// On the network: a = our address, b = parent.
    pub const UP: u8 = 2;
    /// Rejoin started: a = secured (1/0).
    pub const REJOIN: u8 = 3;
    /// Left the network.
    pub const LEFT: u8 = 4;
    /// Neighbor added, gone stale, removed: a = its address, b = LQI.
    pub const NEIGHBOR_ADDED: u8 = 5;
    pub const NEIGHBOR_STALE: u8 = 6;
    pub const NEIGHBOR_REMOVED: u8 = 7;
    /// Route discovery started: a = destination.
    pub const ROUTE_DISCOVERY: u8 = 8;
    /// Network Status received / sent: a = status code, b = address it is about.
    pub const STATUS_RX: u8 = 9;
    pub const STATUS_TX: u8 = 10;
    /// Network key switched: a = key sequence number.
    pub const KEY_SWITCH: u8 = 11;
    /// Channel / PAN identifier changed: a = new value.
    pub const CHANNEL: u8 = 12;
    pub const PAN: u8 = 13;
    /// Address conflict: a = the address.
    pub const CONFLICT: u8 = 14;
    /// Bluetooth LE connection opened / closed / lost: a = how many so far.
    pub const BLE_CONNECT: u8 = 15;
    pub const BLE_DISCONNECT: u8 = 16;
    pub const BLE_LOST: u8 = 17;
    /// Our transmission to a neighbor failed after every retry: a = neighbor, b = status.
    pub const TX_FAIL: u8 = 18;
    /// The light blinked on request from outside the network (the BLE device page).
    pub const IDENTIFY: u8 = 19;
}

/// An entry of the event log as `Stack::event` shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoggedEvent {
    /// One more than the event before.
    pub seq: u16,
    /// What happened (`ev`), and two values that say more.
    pub kind: u8,
    pub a: u16,
    pub b: u16,
    /// Seconds since the start it happened in.
    pub t: u32,
    /// It happened in this start, not in one before a warm restart.
    pub this_start: bool,
}

/// The MAC unicast counters of the Diagnostics cluster (0x0102..=0x0105).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MacCounters {
    pub rx_ucast: u32,
    pub tx_ucast: u32,
    pub tx_ucast_retries: u32,
    pub tx_ucast_failures: u32,
}

/// The neighbors of this router: routers heard within the age limit, and
/// end-device children.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NeighborCounts {
    pub routers: u8,
    pub end_devices: u8,
}

/// One entry of the event log: its number (one more than the event
/// before; the log is read in pages, and between two of them new events
/// move the rest along: the reader keeps each number once), which start it
/// belongs to (low byte of NumberOfResets), what, when (seconds since that
/// start), and two values.
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
    /// The entry as the log stores and the page shows it: number, start,
    /// kind, seconds (u32), a, b.
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

/// The standard attributes this node counts honestly, in ascending order
/// (Tables 3-130 and 3-131). Left out: none of the table; see
/// docs/diagnostics.md for how each is counted.
pub(crate) const STANDARD: [(u16, u8, Src); 32] = [
    (0x0000, T_U16, Src::Resets),
    (0x0001, T_U16, Src::Counter(k::PERSISTENT_WRITES)),
    (0x0100, T_U32, Src::Counter(k::MAC_RX_BCAST)),
    (0x0101, T_U32, Src::Counter(k::MAC_TX_BCAST)),
    (0x0102, T_U32, Src::Counter(k::MAC_RX_UCAST)),
    (0x0103, T_U32, Src::Counter(k::MAC_TX_UCAST)),
    (0x0104, T_U16, Src::Counter(k::MAC_TX_UCAST_RETRY)),
    (0x0105, T_U16, Src::Counter(k::MAC_TX_UCAST_FAIL)),
    (0x0106, T_U16, Src::Counter(k::APS_RX_BCAST)),
    (0x0107, T_U16, Src::Counter(k::APS_TX_BCAST)),
    (0x0108, T_U16, Src::Counter(k::APS_RX_UCAST)),
    (0x0109, T_U16, Src::Counter(k::APS_TX_UCAST_SUCCESS)),
    (0x010A, T_U16, Src::Counter(k::APS_TX_UCAST_RETRY)),
    (0x010B, T_U16, Src::Counter(k::APS_TX_UCAST_FAIL)),
    (0x010C, T_U16, Src::Counter(k::ROUTE_DISC_INITIATED)),
    (0x010D, T_U16, Src::Counter(k::NEIGHBOR_ADDED)),
    (0x010E, T_U16, Src::Counter(k::NEIGHBOR_REMOVED)),
    (0x010F, T_U16, Src::Counter(k::NEIGHBOR_STALE)),
    (0x0110, T_U16, Src::Counter(k::JOIN_INDICATION)),
    (0x0111, T_U16, Src::Counter(k::CHILD_MOVED)),
    (0x0112, T_U16, Src::Counter(k::NWK_FC_FAILURE)),
    (0x0113, T_U16, Src::Counter(k::APS_FC_FAILURE)),
    (0x0114, T_U16, Src::Counter(k::APS_UNAUTHORIZED_KEY)),
    (0x0115, T_U16, Src::Counter(k::NWK_DECRYPT_FAILURES)),
    (0x0116, T_U16, Src::Counter(k::APS_DECRYPT_FAILURES)),
    (0x0117, T_U16, Src::Counter(k::BUFFER_ALLOCATE_FAILURES)),
    (0x0118, T_U16, Src::Counter(k::RELAYED_UCAST)),
    (0x0119, T_U16, Src::Counter(k::PHY_TO_MAC_QUEUE_LIMIT)),
    (0x011A, T_U16, Src::Counter(k::PACKET_VALIDATE_DROPS)),
    (0x011B, T_U16, Src::AverageRetries),
    (0x011C, T_U8, Src::Lqi),
    (0x011D, T_I8, Src::Rssi),
];

/// The manufacturer-specific attributes (manufacturer code of the
/// identity, 0x1141), in ascending order. Their identifiers lie apart from
/// the standard ones, so a client that ignores the manufacturer code cannot
/// mistake one for the other.
pub(crate) const MANUFACTURER: [(u16, u8, Src); 63] = [
    (0x5000, T_U32, Src::Uptime),
    (0x5001, T_ENUM8, Src::Cause),
    (0x5002, T_U16, Src::WatchdogResets),
    (0x5003, T_U8, Src::Neighbors),
    (0x5004, T_U8, Src::Children),
    (0x5005, T_U16, Src::Counter(k::RELAY_DROP_QUEUE_FULL)),
    (0x5006, T_U16, Src::Counter(k::RELAY_DROP_NO_ROUTE)),
    (0x5007, T_U16, Src::Counter(k::RELAY_DROP_MAC_FAIL)),
    (0x5008, T_U16, Src::Counter(k::RELAY_DROP_DUPLICATE)),
    (0x5009, T_U16, Src::Counter(k::NWK_STATUS_SENT)),
    (0x500A, T_U16, Src::Counter(k::BCAST_DROP_TABLE_FULL)),
    (0x500B, T_U16, Src::Counter(k::BCAST_QUEUE_FULL)),
    (0x500C, T_U32, Src::Counter(k::CCA_BUSY)),
    (0x500D, T_U16, Src::Counter(k::CHANNEL_ACCESS_FAILURES)),
    (0x500E, T_U8, Src::Counter(k::RX_QUEUE_PEAK)),
    (0x500F, T_U8, Src::Counter(k::INDIRECT_PEAK)),
    (0x5010, T_U16, Src::Counter(k::INDIRECT_DROPPED)),
    (0x5011, T_U16, Src::Counter(k::INDIRECT_EXPIRED)),
    (0x5012, T_U32, Src::Counter(k::BLE_LENDS)),
    (0x5013, T_U32, Src::Counter(k::BLE_MS)),
    (0x5014, T_U16, Src::Counter(k::FLASH_ERASES)),
    (0x5015, T_U32, Src::RssiBucket(0)),
    (0x5016, T_U32, Src::RssiBucket(1)),
    (0x5017, T_U32, Src::RssiBucket(2)),
    (0x5018, T_U32, Src::RssiBucket(3)),
    (0x5019, T_U32, Src::RssiBucket(4)),
    (0x501A, T_I8, Src::NoiseNow),
    (0x501B, T_I8, Src::NoiseMax),
    (0x5020, T_U16, Src::CoordNextHop),
    (0x5021, T_U8, Src::CoordCost),
    (0x5022, T_U8, Src::CoordManyToOne),
    (0x5023, T_U8, Src::RoutesUsed),
    (0x5024, T_U16, Src::NwkStatusRx),
    (0x5025, T_U8, Src::LastStatusRx),
    (0x5026, T_U8, Src::LastStatusTx),
    (0x5030, T_U8, Src::Channel),
    (0x5031, T_U16, Src::Pan),
    (0x5032, T_OCTETS, Src::ExtPan),
    (0x5033, T_U8, Src::UpdateId),
    (0x5034, T_U8, Src::KeySeq),
    (0x5035, T_U32, Src::FrameCounter),
    (0x5036, T_U8, Src::Depth),
    (0x5037, T_U16, Src::Parent),
    (0x5040, T_U32, Src::LoopMax),
    (0x5041, T_U32, Src::BleLendMax),
    (0x5042, T_U32, Src::FlashBusyMax),
    (0x5043, T_U32, Src::StackFree),
    (0x5044, T_U16, Src::IrqStackFree),
    (0x5048, T_U32, Src::Ble(0)),
    (0x5049, T_U32, Src::Ble(1)),
    (0x504A, T_U32, Src::Ble(2)),
    (0x504B, T_U32, Src::Ble(3)),
    (0x504C, T_U32, Src::Ble(4)),
    (0x5050, T_OCTETS, Src::NeighborPage(0)),
    (0x5051, T_OCTETS, Src::NeighborPage(1)),
    (0x5052, T_OCTETS, Src::NeighborPage(2)),
    (0x5053, T_OCTETS, Src::NeighborPage(3)),
    (0x5058, T_OCTETS, Src::EventPage(0)),
    (0x5059, T_OCTETS, Src::EventPage(1)),
    (0x505A, T_OCTETS, Src::EventPage(2)),
    (0x505B, T_OCTETS, Src::EventPage(3)),
    (0x505C, T_OCTETS, Src::EventPage(4)),
    (0x505D, T_OCTETS, Src::EventPage(5)),
];
const _: () = assert!(NEIGHBOR_PAGES == 4 && EVENT_PAGES == 6, "the attribute table lists every page");

/// The manufacturer's attributes in `Stack::diag_counters`: the numbers
/// from 0x5000 up to the first that is not a counter of the start (FlashErases).
const COUNTERS_MFR: usize = 21;
const COUNTERS_VERSION: u8 = 1;
const _: () = assert!(MANUFACTURER[COUNTERS_MFR - 1].0 == 0x5014, "the counters end at FlashErases");

/// Bytes of a numeric attribute value of type `ty`.
const fn width(ty: u8) -> usize {
    match ty {
        T_U8 | T_I8 | T_ENUM8 => 1,
        T_U16 => 2,
        T_U32 => 4,
        _ => 0,
    }
}

/// Longest value `Stack::diag_counters` writes.
pub const DIAG_COUNTERS_MAX: usize = {
    let mut n = 1;
    let mut i = 0;
    while i < STANDARD.len() {
        assert!(width(STANDARD[i].1) > 0, "a number");
        n += 3 + width(STANDARD[i].1);
        i += 1;
    }
    let mut i = 0;
    while i < COUNTERS_MFR {
        assert!(width(MANUFACTURER[i].1) > 0, "a number");
        n += 3 + width(MANUFACTURER[i].1);
        i += 1;
    }
    n
};

/// A diagnostic attribute: (type, what it shows), of the manufacturer's set or the standard one.
pub(crate) fn attribute(manufacturer: bool, id: u16) -> Option<(u8, Src)> {
    let table: &[(u16, u8, Src)] = if manufacturer { &MANUFACTURER } else { &STANDARD };
    if manufacturer && id.wrapping_sub(blackbox::BOX_FIRST) < blackbox::BOX_PAGES as u16 {
        // not in the table: read only, by number
        return Some((T_OCTETS, Src::BoxPage(id - blackbox::BOX_FIRST)));
    }
    table.iter().find(|a| a.0 == id).map(|a| (a.1, a.2))
}

pub(crate) fn attributes(manufacturer: bool) -> &'static [(u16, u8, Src)] {
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
    /// How often the last transmission went on the air (`Hal::tx_attempts`).
    pub last_attempts: u8,
    /// What the hardware layer had counted when `n` last took it over.
    hal_seen: HalCounters,
    /// When the stack started (for the uptime and the snapshot timer).
    started: u32,
    snapshot_at: u32,
    retain_at: u32,
    /// Received frames by signal strength since the start.
    pub rssi: [u32; RSSI_BUCKETS],
    /// Noise floor in dBm (i8::MIN: not measured yet), and when next.
    pub noise_now: i8,
    pub noise_max: i8,
    pub noise_at: u32,
    pub status_rx: u32,
    pub last_status_rx: u8,
    pub last_status_tx: u8,
    /// The event log: a ring, `head` the next place, `len` entries in it.
    pub events: [Event; EVENTS],
    pub ev_head: u8,
    pub ev_len: u8,
    /// The number the next event gets.
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

    /// Counts a received frame by its signal strength.
    pub(crate) fn rssi_add(&mut self, rssi: i8) {
        let i = RSSI_EDGES.iter().position(|&e| rssi < e).unwrap_or(RSSI_BUCKETS - 1);
        self.rssi[i] = self.rssi[i].wrapping_add(1);
    }

    /// Puts an event into the log, the oldest giving way.
    pub(crate) fn event(&mut self, now: u32, kind: u8, a: u16, b: u16) {
        let e = Event { seq: self.ev_seq, epoch: self.resets as u8, kind, t: self.uptime_s(now), a, b };
        self.ev_seq = self.ev_seq.wrapping_add(1);
        self.events[self.ev_head as usize] = e;
        self.ev_head = ((self.ev_head as usize + 1) % EVENTS) as u8;
        self.ev_len = (self.ev_len as usize + 1).min(EVENTS) as u8;
    }

    /// The `i`-th newest event.
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
        // numbering goes on after the newest
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

    /// Takes over what the hardware layer counted since the last time.
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

    /// The hardware layer's Bluetooth LE counts that moved since `before`, as events.
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

    /// AverageMACRetryPerAPSMessageSent, rounded to the nearest whole retry.
    pub(crate) fn average_retries(&self) -> u16 {
        let sent = self.n[k::APS_SENT];
        if sent == 0 {
            return 0;
        }
        let r = self.n[k::APS_MAC_RETRIES] as u64;
        ((r * 2 + sent as u64) / (sent as u64 * 2)).min(0xFFFF) as u16
    }

    /// The average in hundredths, for the Analog Input that shows it.
    pub(crate) fn average_retries_x100(&self) -> u32 {
        let sent = self.n[k::APS_SENT];
        if sent == 0 {
            return 0;
        }
        ((self.n[k::APS_MAC_RETRIES] as u64 * 100 / sent as u64).min(u32::MAX as u64)) as u32
    }

    /// Milliseconds the radio was lent to BLE since the start.
    pub(crate) fn ble_ms_since_start(&self) -> u32 {
        self.hal_seen.ble_ms
    }

    pub(crate) fn uptime_s(&self, now: u32) -> u32 {
        now.wrapping_sub(self.started) / 1000
    }

    /// Relayed unicasts lost, whatever the cause.
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
    /// Every MAC transmission of the stack goes through here: the MAC
    /// counters of Table 3-131 and the busy channel.
    pub(crate) fn mac_transmit(&mut self, mpdu: &[u8]) -> TxStatus {
        let status = self.hal.transmit(mpdu);
        let attempts = self.hal.tx_attempts();
        self.diag.last_attempts = attempts;
        let unicast = mpdu.first().is_some_and(|fc| fc & 0x20 != 0);
        // on the air at least once (a busy channel may have kept it off entirely)
        let sent = attempts > 0 || status != TxStatus::ChannelBusy;
        if unicast {
            if sent {
                self.diag.add(k::MAC_TX_UCAST);
            }
            self.diag.add_n(k::MAC_TX_UCAST_RETRY, attempts.saturating_sub(1) as u32);
            if status != TxStatus::Ok {
                self.diag.add(k::MAC_TX_UCAST_FAIL);
            }
            // per neighbor: a short destination address (destination mode bits 10-11 = 2)
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

    /// At the start: the counters as they were (from the RAM a warm restart
    /// keeps, else from the last stored snapshot), the reset counted.
    /// Returns the boot record, which goes to the store with the frame
    /// counters (`load_counters`).
    pub(crate) fn diag_start(&mut self) -> [u8; 5] {
        let cause = self.hal.reset_cause();
        self.diag.cause = cause;
        let mut buf = [0u8; super::RETAIN_MAX];
        let mut restored = false;
        if cause != ResetCause::PowerOn {
            if let Some(n) = self.hal.retained(&mut buf) {
                restored = self.diag.decode(&buf[..n.min(SNAPSHOT_LEN)]);
                if restored && n >= super::RETAIN_MAX {
                    // the event log of the starts before, for what led to this one
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
        // the hardware layer's counts begin anew with every start
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

    /// After a start: once the stores have been read, a warm restart's
    /// counters (which only the RAM had) go to the store too.
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

    /// Counts a record the stack writes to the non-volatile store.
    pub(crate) fn nv_written(&mut self) {
        self.diag.add(k::PERSISTENT_WRITES);
    }

    /// Takes over the hardware layer's counters and the pool's high-water mark.
    pub(crate) fn diag_sync(&mut self) {
        let c = self.hal.counters();
        let before = self.diag.hal_seen;
        self.diag.take_hal(c);
        let now = self.hal.now_ms();
        self.diag.ble_events(now, before);
    }

    /// Puts an event into the log.
    pub(crate) fn log_event(&mut self, kind: u8, a: u16, b: u16) {
        let now = self.hal.now_ms();
        self.diag.event(now, kind, a, b);
    }

    /// Once a minute, in a quiet moment: the noise floor of our channel.
    pub(crate) fn diag_noise(&mut self, now: u32) {
        if !due(now, self.diag.noise_at) {
            return;
        }
        // Not with energy detection: that retunes the radio, and a frame
        // arriving meanwhile would be acknowledged late. The hardware layer
        // reads the signal strength of the channel it listens on, when quiet.
        let Some(dbm) = self.hal.noise_dbm() else {
            // traffic: another try in a second
            self.diag.noise_at = now.wrapping_add(1_000);
            return;
        };
        self.diag.noise_at = now.wrapping_add(NOISE_MS);
        self.diag.noise_now = dbm;
        if self.diag.noise_max == i8::MIN || dbm > self.diag.noise_max {
            self.diag.noise_max = dbm;
        }
    }

    /// The route to the coordinator: (first hop, its link cost, from a many-to-one request).
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

    /// A row of the black box every `blackbox::ROW_MS`.
    pub(crate) fn box_timers(&mut self, now: u32) {
        // a row comes late rather than while the pages are being read
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

    /// An attribute of type octet string: the bytes after the length.
    pub(crate) fn diag_bytes(&mut self, src: Src, out: &mut [u8; 64]) -> usize {
        match src {
            Src::ExtPan => {
                out[..8].copy_from_slice(&self.nib.ext_pan.to_le_bytes());
                8
            }
            Src::BoxPage(p) => {
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
            Src::NeighborPage(p) => {
                // entry: address, LQI, outgoing cost, age, flags (bit 0 in use, bit 1 relays
                // broadcasts, bit 2 in our last Link Status, bit 3 also a child, bits 4-6 the
                // link cost routing uses, bit 7 it is measured), times stale, frames heard,
                // average and lowest RSSI, our transmissions to it, retries, failures, the
                // measured probability of delivery (0..=255, the high byte of 0..=65535)
                let mut n = 0;
                let from = p as usize * NEIGHBORS_PER_PAGE;
                for nb in self.neighbors[from..from + NEIGHBORS_PER_PAGE].iter() {
                    let o = &mut out[n..n + NEIGHBOR_ENTRY];
                    let child = nb.ext != 0 && self.child_by_ext(nb.ext).is_some();
                    o[0..2].copy_from_slice(&nb.short.to_le_bytes());
                    o[2] = nb.lqi;
                    o[3] = nb.out_cost;
                    o[4] = nb.age;
                    o[5] = nb.used as u8
                        | (nb.relays as u8) << 1
                        | (nb.listed as u8) << 2
                        | (child as u8) << 3
                        | (nb.link_cost() & 7) << 4
                        | (nb.measured() as u8) << 7;
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
            Src::EventPage(p) => {
                // newest first
                let mut n = 0;
                for i in 0..EVENTS_PER_PAGE {
                    let Some(e) = self.diag.event_newest(p as usize * EVENTS_PER_PAGE + i) else { break };
                    e.encode(&mut out[n..n + EVENT_ENTRY]);
                    n += EVENT_ENTRY;
                }
                n
            }
            _ => 0,
        }
    }

    /// Once a second into the RAM a warm restart keeps; once an hour to the store.
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

    /// Keeps the counters for the restart that is about to happen (an update).
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

    /// Stores the counters again after the store was wiped (leaving the network).
    pub(crate) fn diag_keep(&mut self) {
        let boot = self.boot_record();
        self.nv_put(NvItem::Boot, &boot);
        self.store_diagnostics();
    }

    /// The manufacturer's command `CMD_CLEAR`: every counter back to 0.
    pub(crate) fn diag_clear(&mut self) {
        self.diag_sync();
        self.diag.n = [0; k::COUNT];
        self.diag.resets = 0;
        self.diag.watchdog_resets = 0;
        let boot = self.boot_record();
        self.nv_put(NvItem::Boot, &boot);
        self.store_diagnostics();
    }

    /// Seconds since the stack started.
    pub fn uptime_s(&mut self) -> u32 {
        let now = self.hal.now_ms();
        self.diag.uptime_s(now)
    }

    /// Why the chip started this time.
    pub fn reset_cause(&self) -> ResetCause {
        self.diag.cause
    }

    /// NumberOfResets (every start counts) and the watchdog resets among them.
    pub fn resets(&self) -> (u16, u16) {
        (self.diag.resets, self.diag.watchdog_resets)
    }

    pub fn neighbor_counts(&self) -> NeighborCounts {
        let end_devices = self.children.iter().filter(|c| c.used && c.joined && c.capability & 0x02 == 0).count();
        NeighborCounts { routers: self.live_neighbors().min(255) as u8, end_devices: end_devices.min(255) as u8 }
    }

    /// Link quality and signal strength (dBm) of the last frame received
    /// (LastMessageLQI, LastMessageRSSI).
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

    /// The `i`-th newest entry of the event log (the log keeps `EVENTS`).
    pub fn event(&self, i: usize) -> Option<LoggedEvent> {
        let e = self.diag.event_newest(i)?;
        Some(LoggedEvent { seq: e.seq, kind: e.kind, a: e.a, b: e.b, t: e.t, this_start: e.epoch == self.diag.resets as u8 })
    }

    /// The number the next event will get: it changes with every new entry.
    pub fn event_seq(&self) -> u16 {
        self.diag.ev_seq
    }

    /// The firmware made the light blink on a request from outside the
    /// network (`ev::IDENTIFY`).
    pub fn log_identify(&mut self) {
        self.log_event(ev::IDENTIFY, 0, 0);
    }

    /// Keeps the diagnostic counters and the event log for a restart the
    /// firmware is about to make (`Hal::retain`), as before an update.
    pub fn keep_for_restart(&mut self) {
        self.diag_retain_now();
    }

    /// Takes over the hardware layer's counters now rather than at the next
    /// second, so that the BLE connections and disconnections they show
    /// are in the event log at once (the stack does it every second only
    /// while on a network).
    pub fn sync_hardware_counters(&mut self) {
        self.diag_sync();
    }

    /// Router neighbors heard within the age limit.
    pub(crate) fn live_neighbors(&self) -> usize {
        self.neighbors.iter().filter(|n| n.used && n.age <= super::AGE_LIMIT).count()
    }

    /// The value of a diagnostic attribute, as a number (the type says how wide).
    pub(crate) fn diag_value(&mut self, src: Src) -> u32 {
        self.diag_sync();
        self.diag_value_synced(src)
    }

    /// The Diagnostics cluster's numeric attributes in one go, from the
    /// values the cluster answers with: the standard ones and the
    /// manufacturer's first `COUNTERS_MFR` (0x5000..=0x5014). Format 1, then
    /// per attribute in ascending order its identifier (u16), ZCL type and
    /// value, little-endian, as wide as the type. `out` takes
    /// `DIAG_COUNTERS_MAX` bytes; returns how many were written.
    pub fn diag_counters(&mut self, out: &mut [u8]) -> usize {
        if out.len() < DIAG_COUNTERS_MAX {
            return 0;
        }
        self.diag_sync();
        out[0] = COUNTERS_VERSION;
        let mut at = 1;
        for &(id, ty, src) in STANDARD.iter().chain(MANUFACTURER[..COUNTERS_MFR].iter()) {
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
            Src::ExtPan | Src::NeighborPage(_) | Src::EventPage(_) | Src::BoxPage(_) => 0,
        }
    }
}
