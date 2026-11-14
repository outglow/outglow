//! What the stack needs from the hardware.

/// Outcome of a MAC transmission (after CSMA-CA and all retries).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxStatus {
    /// Sent; acknowledged if the frame asked for an acknowledgement.
    Ok,
    /// Sent, but no acknowledgement arrived after all retries.
    NoAck,
    /// The channel was never clear.
    ChannelBusy,
}

/// Metadata of a received frame.
#[derive(Debug, Clone, Copy)]
pub struct RxMeta {
    /// MPDU length in bytes, without the FCS.
    pub len: usize,
    /// Link quality 0..=255.
    pub lqi: u8,
    /// Received signal strength in dBm.
    pub rssi: i8,
    /// When the frame ended, in microseconds on the radio's clock (wraps).
    pub end_us: u32,
}

/// What became of a frame, for diagnostics (`Hal::trace`): the kinds and
/// their details. A frame is named by the source and sequence number of its
/// NWK header.
pub mod trace {
    /// A unicast for someone else, taken for relaying. `peer`: MAC source; `aux`: radius.
    pub const RELAY: u8 = 1;
    /// A NWK frame handed to the MAC. `peer`: MAC destination; `detail`:
    /// `TX_*`; `aux`: filled in by the hardware layer (attempts, busy
    /// channel assessments, time taken).
    pub const TX: u8 = 2;
    pub const TX_OK: u8 = 0;
    pub const TX_NO_ACK: u8 = 1;
    pub const TX_BUSY: u8 = 2;
    /// kept for a sleepy child's poll
    pub const TX_FOR_CHILD: u8 = 3;
    /// longer than a frame can be
    pub const TX_TOO_LONG: u8 = 4;
    /// A frame for someone else not passed on. `detail`: `DROP_*`.
    pub const DROP: u8 = 3;
    pub const DROP_RADIUS: u8 = 1;
    pub const DROP_SOURCE_ROUTE: u8 = 2;
    pub const DROP_ROUTE_REPLY: u8 = 3;
    pub const DROP_UNAUTHENTICATED_CHILD: u8 = 4;
    pub const DROP_TOO_LONG: u8 = 5;
    /// a failed frame towards a concentrator with no other router to take it
    pub const DROP_NO_OTHER_ROUTER: u8 = 6;
    /// a command with no route and no route discovery allowed
    pub const DROP_NO_ROUTE: u8 = 7;
    /// a route record that cannot go on
    pub const DROP_ROUTE_RECORD: u8 = 8;
    /// a frame whose MAC failed and that is reported to nobody
    pub const DROP_UNREPORTED: u8 = 9;
    /// A frame kept for later. `detail`: 0 waiting for a route, else the
    /// attempts still to come after a failed one; `peer`: next hop.
    pub const HOLD: u8 = 4;
    /// A frame that was kept and has failed. `detail`: `FAIL_*`; `peer`: destination.
    pub const HELD_FAIL: u8 = 5;
    pub const FAIL_EXPIRED: u8 = 1;
    pub const FAIL_EVICTED: u8 = 2;
    pub const FAIL_NO_ROOM: u8 = 3;
    /// A Network Status sent. `src`: to whom; `seq`: status code; `peer`: about.
    pub const NWK_STATUS: u8 = 6;
    /// A received NWK frame dropped before routing. `detail`: `RX_*`; `peer`: MAC source.
    pub const RX_DROP: u8 = 7;
    /// counter not above the last one heard: a copy of a frame taken before
    pub const RX_REPLAY: u8 = 1;
    pub const RX_BAD_MIC: u8 = 2;
    pub const RX_UNKNOWN_KEY: u8 = 3;
    pub const RX_ECHO: u8 = 4;
    pub const RX_OWN_ADDRESS: u8 = 5;
    pub const RX_NOT_SECURED: u8 = 6;
    pub const RX_NOT_UP: u8 = 7;
    pub const RX_NOT_A_CHILD: u8 = 8;
    /// A broadcast queued (`detail` 0) or not remembered (`detail` 1: the
    /// broadcast transaction table is full; 2: no room in the queue).
    pub const BROADCAST: u8 = 8;
}

/// One diagnostic record (`Hal::trace`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Trace {
    pub kind: u8,
    pub detail: u8,
    pub seq: u8,
    pub src: u16,
    pub peer: u16,
    pub aux: u32,
}

/// What the hardware layer counts on its own since the chip started, for the
/// Diagnostics cluster (`Hal::counters`). Every field only grows (wrapping);
/// 0 where the hardware layer does not count it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HalCounters {
    /// Clear channel assessments that found the channel busy.
    pub cca_busy: u32,
    /// Received frames lost because the radio's receive queue was full.
    pub rx_queue_full: u32,
    /// The most frames that were waiting in the receive queue at once.
    pub rx_queue_peak: u32,
    /// Times the radio was lent to Bluetooth LE (advertising, connection
    /// and mesh events) and for how long, in milliseconds.
    pub ble_lends: u32,
    pub ble_ms: u32,
    /// Sector erases of the settings area (not of the OTA staging area).
    pub flash_erases: u32,
    /// Records written to stores the stack does not see: the light's
    /// settings and the power-on counter.
    pub flash_stores: u32,
    /// The longest round of the main loop, the longest loan of the radio
    /// to BLE and the longest flash erase since the start, microseconds.
    pub loop_max_us: u32,
    pub ble_lend_max_us: u32,
    pub flash_busy_max_us: u32,
    /// Bluetooth LE: connections, disconnections (all), links lost (the
    /// central went silent), connection events run and skipped.
    pub ble_connections: u32,
    pub ble_disconnects: u32,
    pub ble_links_lost: u32,
    pub ble_events: u32,
    pub ble_events_skipped: u32,
    /// Bytes of the main stack (with the free RAM below it) and of the
    /// interrupt stack never used since the start; 0 when not measured.
    pub stack_free: u32,
    pub irq_stack_free: u32,
}

/// Why the chip last started (`Hal::reset_cause`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetCause {
    PowerOn = 0,
    Watchdog = 1,
    /// A restart the firmware asked for (after an update, after a panic).
    Software = 2,
}

/// Non-volatile items the stack keeps. Each is one small record that is
/// rewritten as a whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NvItem {
    /// Network membership: channel, addresses, keys.
    Network = 1,
    /// Outgoing frame counters, stored ahead of use.
    Counters = 2,
    /// Group table, binding table and reporting configuration.
    Tables = 3,
    /// End-device children.
    Children = 4,
    /// Scene table.
    Scenes = 5,
    /// Green Power proxy table, one item per entry (see `NvItem::proxy`).
    Proxy0 = 6,
    /// Progress of an OTA download, for resuming it after a power cut.
    OtaProgress = 7,
    Proxy1 = 8,
    Proxy2 = 9,
    Proxy3 = 10,
    Proxy4 = 11,
    /// Keys of the Green Power cluster set by a sink.
    GpKeys = 12,
    /// The end-device children after the first `CHILDREN_PER_ITEM` (`NvItem::children`).
    Children1 = 13,
    Children2 = 14,
    /// The Diagnostics counters and the reporting configuration of the
    /// diagnostic attributes: written about once an hour, once after a
    /// watchdog or software reset, and when that configuration changes.
    Diagnostics = 15,
    /// The reset counters (NumberOfResets, watchdog resets): written at
    /// every start, together with `Counters` (`Hal::nv_store_pair`).
    Boot = 16,
    /// Reporting configurations of the attributes outside the light's
    /// three (diagnostics, the object endpoints).
    Reports = 17,
    /// The bulb's own settings (manufacturer cluster `0xFC5F`): when BLE runs.
    Settings = 18,
}

impl NvItem {
    /// The highest item id: a store that keeps the items by id needs room
    /// for 1..=MAX_ID. A new item goes last and moves this along.
    pub const MAX_ID: u8 = NvItem::Settings as u8;

    /// The item of entry `i` of the Green Power proxy table.
    pub fn proxy(i: usize) -> NvItem {
        [NvItem::Proxy0, NvItem::Proxy1, NvItem::Proxy2, NvItem::Proxy3, NvItem::Proxy4][i % 5]
    }

    /// The item of part `k` of the child table.
    pub fn children(k: usize) -> NvItem {
        [NvItem::Children, NvItem::Children1, NvItem::Children2][k % 3]
    }
}

/// Hardware services. The radio half is the lower MAC: the implementation
/// does CSMA-CA, waits for the acknowledgement and retries on its own, and it
/// acknowledges received frames addressed to this node (set with
/// `set_filter`) within the 802.15.4 turnaround time.
pub trait Hal {
    /// The hardware's AES-128 block encryption, when it has one: frame
    /// security then costs microseconds per block instead of the best part
    /// of a millisecond. `None`: everything is computed in software.
    const AES: Option<crate::aes::Engine> = None;

    /// Monotonic time in milliseconds (wraps after 49 days).
    fn now_ms(&mut self) -> u32;
    fn random(&mut self) -> u32;
    /// The node's EUI-64.
    fn ieee(&mut self) -> u64;

    /// Tunes to an 802.15.4 channel, 11..=26, and leaves the receiver on.
    fn set_channel(&mut self, channel: u8);
    /// Addresses the receiver accepts and acknowledges: frames to this PAN
    /// (or the broadcast PAN) and to this short address, the node's EUI-64 or
    /// the broadcast address. Beacons always pass.
    fn set_filter(&mut self, pan: u16, short: u16);
    /// Transmits an MPDU (without FCS). Blocks until the outcome is known.
    fn transmit(&mut self, mpdu: &[u8]) -> TxStatus;
    /// Addresses of children with frames waiting for their poll. The radio
    /// sets the frame-pending bit in its acknowledgement of a Data Request
    /// from one of them (and clears it for everyone else).
    fn indirect_pending(&mut self, _short: &[u16], _ext: &[u64]) {}
    /// Short addresses of this node's end-device children. A Data Request
    /// from any other short address is acknowledged with the frame-pending
    /// bit set: its sender takes this node for a parent it is not, and must
    /// stay awake for the Leave the stack answers such a poll with.
    fn poll_children(&mut self, _short: &[u16]) {}
    /// The noise floor of the channel the radio listens on, dBm, measured
    /// without leaving it; `None` when it cannot be measured now (a frame
    /// is arriving, or one went by a moment ago).
    fn noise_dbm(&mut self) -> Option<i8> {
        None
    }
    /// Energy detection: the strongest signal on a channel over a few
    /// milliseconds, 0 (nothing) ..= 255. Leaves the radio on that channel.
    fn energy_detect(&mut self, _channel: u8) -> u8 {
        0
    }
    /// OTA: prepares the staging area for an image file of `size` bytes.
    /// False when there is no staging area or the file cannot fit.
    fn ota_begin(&mut self, _size: u32) -> bool {
        false
    }
    /// OTA: continues a download that was interrupted with `image_len` bytes
    /// of the image already in the staging area. False if that is not possible.
    fn ota_resume(&mut self, _image_len: u32) -> bool {
        false
    }
    /// OTA: stores image bytes at `offset` of the staging area. Offsets only grow.
    fn ota_write(&mut self, _offset: u32, _data: &[u8]) {}
    /// OTA: the staged image is complete and verified. Marks it bootable
    /// (byte 8 = 0x4B) and restarts into the bootloader.
    fn ota_activate(&mut self) {}
    /// Transmits an MPDU at a given moment (on the clock of
    /// `RxMeta::end_us`), without channel assessment, acknowledgement or
    /// retry: for the answer to a Green Power device, which listens for a
    /// short window at a fixed time after its own frame.
    fn transmit_at(&mut self, _mpdu: &[u8], _at_us: u32) {}
    /// Takes the next received MPDU (FCS checked and removed), if any.
    fn receive(&mut self, buf: &mut [u8; 127]) -> Option<RxMeta>;

    /// Reads an item; `None` when it was never stored.
    fn nv_load(&mut self, item: NvItem, buf: &mut [u8]) -> Option<usize>;
    fn nv_store(&mut self, item: NvItem, data: &[u8]);
    /// Forgets every item (leaving the network, factory reset).
    fn nv_erase(&mut self);

    /// Stores two items; where the store can, in one write (the flash
    /// programmed once instead of twice: the reset counters go with the
    /// frame counters, which every start writes anyway).
    fn nv_store_pair(&mut self, a: NvItem, a_data: &[u8], b: NvItem, b_data: &[u8]) {
        self.nv_store(a, a_data);
        self.nv_store(b, b_data);
    }

    /// Diagnostics: what became of a frame (`trace`). Does nothing unless
    /// the hardware layer is built to record it; the stack behaves the same
    /// either way.
    fn trace(&mut self, _ev: Trace) {}

    /// How often the last `transmit` put the frame on the air (1: it went
    /// once; more: MAC retransmissions; 0: not known, or never sent).
    fn tx_attempts(&mut self) -> u8 {
        0
    }
    /// The counters the hardware layer keeps (`HalCounters`).
    fn counters(&mut self) -> HalCounters {
        HalCounters::default()
    }
    /// The black box's storage (`stack::blackbox`), if the hardware layer
    /// keeps one: it lives outside `Stack`.
    fn black_box(&mut self) -> Option<&mut crate::BlackBox> {
        None
    }
    fn reset_cause(&mut self) -> ResetCause {
        ResetCause::PowerOn
    }
    /// Keeps a few bytes where a watchdog or software reset does not clear
    /// them (RAM that the start-up code leaves alone), for `retained` after
    /// the restart. The diagnostic counters go there every second.
    fn retain(&mut self, _data: &[u8]) {}
    /// What `retain` kept before a warm restart, if it is intact.
    fn retained(&mut self, _buf: &mut [u8]) -> Option<usize> {
        None
    }
}
