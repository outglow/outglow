//! What the BLE side needs from the hardware.
//!
//! The radio half works in whole events. The implementation switches the
//! radio to BLE, runs one advertising or connection event to its end and
//! gives the radio back to whoever had it (the 802.15.4 receiver). Within a
//! connection event it does what has to happen inside the 150 us turnaround:
//! acknowledging, retransmitting, sending the next queued packet. Everything
//! slower is this crate's business.

/// Time unit of the radio clock: sixteenths of a microsecond (the chip's
/// system timer). Times are `u32` and wrap after 268 s.
pub const TICKS_PER_US: u32 = 16;
pub const TICKS_PER_MS: u32 = 16_000;

/// Longest payload of a data channel packet (no data length extension).
pub const LL_PAYLOAD_MAX: usize = 27;
/// Advertising channel packet: header and up to 37 bytes.
pub const ADV_PDU_MAX: usize = 39;
/// Payload of a CONNECT_IND: both addresses and the connection parameters.
pub const CONNECT_LEN: usize = 34;

/// One connection event to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnEvent {
    /// Data channel index, 0..=36.
    pub channel: u8,
    pub access_address: u32,
    pub crc_init: u32,
    /// When the receiver must be listening.
    pub start: u32,
    /// How long from `start` to wait for the central's first packet.
    pub window: u32,
    /// The event must be over by then, whatever the central still has to send.
    pub deadline: u32,
}

/// Outcome of a connection event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EventResult {
    /// When the central's first packet of the event started on air, if one
    /// arrived with a good CRC: the connection's new anchor point.
    pub anchor: Option<u32>,
}

pub trait Hal {
    /// The radio clock.
    fn now(&mut self) -> u32;
    fn random(&mut self) -> u32;
    /// The device's public address, as it goes on air (least significant byte first).
    fn address(&mut self) -> [u8; 6];

    /// One advertising event: `adv` on channels 37, 38 and 39. A SCAN_REQ
    /// for this device is answered with `scan_rsp`. A CONNECT_IND for this
    /// device ends the event: its payload goes to `connect` and the time its
    /// last bit left the air is returned.
    fn advertise(&mut self, adv: &[u8], scan_rsp: &[u8], connect: &mut [u8; CONNECT_LEN]) -> Option<u32>;

    /// A connection starts or has ended: sequence numbers start from zero,
    /// both queues are empty.
    fn connection_reset(&mut self);
    /// One connection event. Blocks from a little before `start` to the end
    /// of the event. Received data packets go to the receive queue, queued
    /// packets are sent and leave the queue when acknowledged.
    fn connection_event(&mut self, event: &ConnEvent) -> EventResult;
    /// Free places in the transmit queue.
    fn tx_free(&mut self) -> usize;
    /// Whether everything queued was sent and acknowledged.
    fn tx_idle(&mut self) -> bool;
    /// Queues a data channel packet: `llid` 1 (continuation), 2 (start of an
    /// L2CAP frame) or 3 (link layer control). False when the queue is full.
    fn tx_push(&mut self, llid: u8, payload: &[u8]) -> bool;
    /// Takes the next received packet with a payload: its LLID and length.
    fn rx_pop(&mut self, payload: &mut [u8; LL_PAYLOAD_MAX]) -> Option<(u8, usize)>;

    /// The mesh network's settings; `None` when never stored.
    fn nv_load(&mut self, buf: &mut [u8]) -> Option<usize>;
    fn nv_store(&mut self, data: &[u8]);

    /// Size of the firmware staging area in bytes.
    fn ota_capacity(&mut self) -> u32;
    /// Erases the 4 KB sector of the staging area that starts at `offset`.
    fn ota_erase(&mut self, offset: u32);
    /// Stores image bytes in the staging area, which was erased before.
    fn ota_write(&mut self, offset: u32, data: &[u8]);
    /// The staged image is complete and its checksum was good as it came
    /// in: marks it bootable (byte 8 = 0x4B) and restarts into the
    /// bootloader. False when the implementation found the staged image
    /// wanting and left it unmarked.
    fn ota_activate(&mut self) -> bool;

    /// Whether the radio's other user (802.15.4) has had traffic just now: a
    /// frame received or sent within the last few milliseconds. An
    /// advertising event that falls due then waits for a quiet moment, but
    /// no later than its advDelay allows: what comes in while BLE has the
    /// radio is lost, and a frame often follows another (an answer, a
    /// broadcast after a unicast). Nothing waits for it by default.
    fn radio_busy(&mut self) -> bool {
        false
    }

    /// Mesh between bulbs: listens on a data channel with the network's
    /// access code for up to `ticks`; a packet with a good CRC goes to `pdu`
    /// (header and payload) and its length is returned.
    fn mesh_listen(&mut self, _channel: u8, _access_code: u32, _ticks: u32, _pdu: &mut [u8; ADV_PDU_MAX]) -> Option<usize> {
        None
    }
    /// Mesh between bulbs: sends `pdu` (header and payload) once on each of
    /// `channels`. False when it could not be sent now (the radio is still
    /// holding packets of the connection); it is to be tried again.
    fn mesh_send(&mut self, _channels: &[u8], _access_code: u32, _pdu: &[u8]) -> bool {
        true
    }
}
