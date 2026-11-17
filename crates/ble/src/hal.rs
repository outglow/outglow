pub const TICKS_PER_US: u32 = 16;
pub const TICKS_PER_MS: u32 = 16_000;

pub const LL_PAYLOAD_MAX: usize = 27;
pub const ADV_PDU_MAX: usize = 39;
pub const CONNECT_LEN: usize = 34;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnEvent {
    pub channel: u8,
    pub access_address: u32,
    pub crc_init: u32,
    pub start: u32,
    pub window: u32,
    pub deadline: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EventResult {
    pub anchor: Option<u32>,
}

pub trait Hal {
    fn now(&mut self) -> u32;
    fn random(&mut self) -> u32;
    fn address(&mut self) -> [u8; 6];

    fn advertise(&mut self, adv: &[u8], scan_rsp: &[u8], connect: &mut [u8; CONNECT_LEN]) -> Option<u32>;

    fn connection_reset(&mut self);
    fn connection_event(&mut self, event: &ConnEvent) -> EventResult;
    fn tx_free(&mut self) -> usize;
    fn tx_idle(&mut self) -> bool;
    fn tx_push(&mut self, llid: u8, payload: &[u8]) -> bool;
    fn rx_pop(&mut self, payload: &mut [u8; LL_PAYLOAD_MAX]) -> Option<(u8, usize)>;

    fn nv_load(&mut self, buf: &mut [u8]) -> Option<usize>;
    fn nv_store(&mut self, data: &[u8]);

    fn ota_capacity(&mut self) -> u32;
    fn ota_erase(&mut self, offset: u32);
    fn ota_write(&mut self, offset: u32, data: &[u8]);
    fn ota_activate(&mut self) -> bool;

    fn radio_busy(&mut self) -> bool;
    fn neighbours_talking(&mut self) -> bool;
    fn mesh_listen(&mut self, channel: u8, access_code: u32, ticks: u32, pdu: &mut [u8; ADV_PDU_MAX]) -> Option<usize>;
    fn mesh_send(&mut self, channels: &[u8], access_code: u32, pdu: &[u8]) -> bool;
}
