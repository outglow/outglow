#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxStatus {
    Ok,
    NoAck,
    ChannelBusy,
}

#[derive(Debug, Clone, Copy)]
pub struct RxMeta {
    pub len: usize,
    pub lqi: u8,
    pub rssi: i8,
    pub end_us: u32,
}

pub mod trace {
    pub const RELAY: u8 = 1;
    pub const TX: u8 = 2;
    pub const TX_OK: u8 = 0;
    pub const TX_NO_ACK: u8 = 1;
    pub const TX_BUSY: u8 = 2;
    pub const TX_FOR_CHILD: u8 = 3;
    pub const TX_TOO_LONG: u8 = 4;
    pub const DROP: u8 = 3;
    pub const DROP_RADIUS: u8 = 1;
    pub const DROP_SOURCE_ROUTE: u8 = 2;
    pub const DROP_ROUTE_REPLY: u8 = 3;
    pub const DROP_UNAUTHENTICATED_CHILD: u8 = 4;
    pub const DROP_TOO_LONG: u8 = 5;
    pub const DROP_NO_OTHER_ROUTER: u8 = 6;
    pub const DROP_NO_ROUTE: u8 = 7;
    pub const DROP_ROUTE_RECORD: u8 = 8;
    pub const DROP_UNREPORTED: u8 = 9;
    pub const DROP_CHANNEL_BUSY: u8 = 10;
    pub const HOLD: u8 = 4;
    pub const HELD_FAIL: u8 = 5;
    pub const FAIL_EXPIRED: u8 = 1;
    pub const FAIL_EVICTED: u8 = 2;
    pub const FAIL_NO_ROOM: u8 = 3;
    pub const NWK_STATUS: u8 = 6;
    pub const RX_DROP: u8 = 7;
    pub const RX_REPLAY: u8 = 1;
    pub const RX_BAD_MIC: u8 = 2;
    pub const RX_UNKNOWN_KEY: u8 = 3;
    pub const RX_ECHO: u8 = 4;
    pub const RX_OWN_ADDRESS: u8 = 5;
    pub const RX_NOT_SECURED: u8 = 6;
    pub const RX_NOT_UP: u8 = 7;
    pub const RX_NOT_A_CHILD: u8 = 8;
    pub const BROADCAST: u8 = 8;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Trace {
    pub kind: u8,
    pub detail: u8,
    pub seq: u8,
    pub src: u16,
    pub peer: u16,
    pub aux: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HalCounters {
    pub cca_busy: u32,
    pub rx_queue_full: u32,
    pub rx_queue_peak: u32,
    pub ble_lends: u32,
    pub ble_ms: u32,
    pub flash_erases: u32,
    pub flash_stores: u32,
    pub loop_max_us: u32,
    pub ble_lend_max_us: u32,
    pub flash_busy_max_us: u32,
    pub ble_connections: u32,
    pub ble_disconnects: u32,
    pub ble_links_lost: u32,
    pub ble_events: u32,
    pub ble_events_skipped: u32,
    pub stack_free: u32,
    pub irq_stack_free: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetCause {
    PowerOn = 0,
    Watchdog = 1,
    Software = 2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NvItem {
    Network = 1,
    Counters = 2,
    Tables = 3,
    Children = 4,
    Scenes = 5,
    Proxy0 = 6,
    OtaProgress = 7,
    Proxy1 = 8,
    Proxy2 = 9,
    Proxy3 = 10,
    Proxy4 = 11,
    GpKeys = 12,
    Children1 = 13,
    Children2 = 14,
    Diagnostics = 15,
    Boot = 16,
    Reports = 17,
    Settings = 18,
    #[cfg(feature = "sense")]
    Sense = 19,
}

impl NvItem {
    #[cfg(not(feature = "sense"))]
    pub const MAX_ID: u8 = NvItem::Settings as u8;
    #[cfg(feature = "sense")]
    pub const MAX_ID: u8 = NvItem::Sense as u8;

    pub fn proxy(i: usize) -> NvItem {
        [NvItem::Proxy0, NvItem::Proxy1, NvItem::Proxy2, NvItem::Proxy3, NvItem::Proxy4][i % 5]
    }

    pub fn children(k: usize) -> NvItem {
        [NvItem::Children, NvItem::Children1, NvItem::Children2][k % 3]
    }
}

pub trait Hal {
    const AES: Option<crate::aes::Engine> = None;

    fn now_ms(&mut self) -> u32;
    fn random(&mut self) -> u32;
    fn ieee(&mut self) -> u64;

    fn set_channel(&mut self, channel: u8);
    fn set_filter(&mut self, pan: u16, short: u16);
    fn transmit(&mut self, mpdu: &[u8]) -> TxStatus;
    fn indirect_pending(&mut self, _short: &[u16], _ext: &[u64]) {}
    fn poll_children(&mut self, _short: &[u16]) {}
    fn noise_dbm(&mut self) -> Option<i8> {
        None
    }
    fn energy_detect(&mut self, _channel: u8) -> u8 {
        0
    }
    fn ota_begin(&mut self, _size: u32) -> bool {
        false
    }
    fn ota_resume(&mut self, _image_len: u32) -> bool {
        false
    }
    fn ota_write(&mut self, _offset: u32, _data: &[u8]) {}
    fn ota_activate(&mut self) {}
    fn transmit_at(&mut self, _mpdu: &[u8], _at_us: u32) {}
    fn receive(&mut self, buf: &mut [u8; 127]) -> Option<RxMeta>;

    fn nv_load(&mut self, item: NvItem, buf: &mut [u8]) -> Option<usize>;
    fn nv_store(&mut self, item: NvItem, data: &[u8]);
    fn nv_erase(&mut self);

    fn nv_store_pair(&mut self, a: NvItem, a_data: &[u8], b: NvItem, b_data: &[u8]) {
        self.nv_store(a, a_data);
        self.nv_store(b, b_data);
    }

    fn trace(&mut self, _ev: Trace) {}

    fn tx_attempts(&mut self) -> u8 {
        0
    }
    fn counters(&mut self) -> HalCounters {
        HalCounters::default()
    }
    fn black_box(&mut self) -> Option<&mut crate::BlackBox> {
        None
    }
    fn reset_cause(&mut self) -> ResetCause {
        ResetCause::PowerOn
    }
    fn retain(&mut self, _data: &[u8]) {}
    fn retained(&mut self, _buf: &mut [u8]) -> Option<usize> {
        None
    }
    fn zcl_received(&mut self, _src: u16, _cluster: u16, _cmd: u8, _frame_control: u8, _lqi: u8, _rssi: i8) {}

    #[cfg(feature = "sense")]
    fn sense_listen(&mut self, _pan: u16) {}
}
