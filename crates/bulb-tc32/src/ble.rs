//! The BLE side on this chip: `ble::Hal` on the radio (`ble_radio`), the
//! system timer and the flash.

use ble::hal::{ConnEvent, EventResult, Hal, ADV_PDU_MAX, CONNECT_LEN, LL_PAYLOAD_MAX};
use ble::Identity;

use crate::ble_radio;
use crate::board::BOARD;
use crate::radio;
use crate::flash;
use crate::reg;
use crate::staging::{self, Owner};

/// What the bulb says it is over BLE: the stock bulb's product number and
/// Device Information strings, so that the AwoX app and our updater see the
/// device they know.
pub const IDENTITY: Identity = Identity {
    product: BOARD.ble_product,
    model: BOARD.model,
    firmware_revision: "3.0.2_1610",
    hardware_revision: BOARD.hw_revision,
    tunable: BOARD.cold.is_some() || BOARD.rgb.is_some(),
    ct_min: BOARD.ct_min,
    ct_max: BOARD.ct_max,
    colour: BOARD.rgb.is_some(),
};

/// 802.15.4 traffic this recent makes an advertising event wait
/// (`ble::Hal::radio_busy`, within its advDelay): a frame that follows
/// another (an acknowledgement, an answer, a broadcast after a unicast)
/// comes within a few milliseconds.
pub(crate) const RADIO_QUIET: u32 = 3 * 16_000;

/// Flash sector with the device's address, written at the factory: the BLE
/// address is its first six bytes (the start of the Zigbee EUI-64), as on
/// the stock firmware.
const MAC_SECTOR: usize = 0xf_f000;

/// The sector with the mesh network's settings, after the Zigbee store: a
/// log of records `A5, length, data, checksum`, the last valid one counts.
pub const MESH_SECTOR: usize = 0xf_3000;
const SECTOR_SIZE: usize = 0x1000;
const RECORD_MAX: usize = 64;

fn checksum(data: &[u8]) -> u8 {
    data.iter().fold(0x5a, |c, b| c.rotate_left(1) ^ *b)
}

/// Offset and length of the last valid record, and where the free space starts.
fn scan() -> (Option<(usize, usize)>, usize) {
    let mut last = None;
    let mut off = 0;
    while off + 3 <= SECTOR_SIZE {
        let mut head = [0u8; 2];
        flash::read(MESH_SECTOR + off, &mut head);
        let len = head[1] as usize;
        if head[0] != 0xa5 || len > RECORD_MAX || off + 3 + len > SECTOR_SIZE {
            break;
        }
        let mut rec = [0u8; RECORD_MAX + 1];
        flash::read(MESH_SECTOR + off + 2, &mut rec[..len + 1]);
        if checksum(&rec[..len]) == rec[len] {
            last = Some((off + 2, len));
        }
        off += 3 + len;
    }
    (last, off)
}

/// Forgets the mesh network (factory reset).
pub fn erase_settings() {
    if !flash::blank(MESH_SECTOR) {
        flash::erase_sector(MESH_SECTOR);
    }
}

pub struct BleHal {
    rng: u32,
}

impl BleHal {
    pub fn new() -> Self {
        let mut seed = [0u8; 4];
        flash::read(MAC_SECTOR, &mut seed);
        BleHal { rng: u32::from_le_bytes(seed) ^ reg::tick() | 1 }
    }
}

impl Hal for BleHal {
    fn now(&mut self) -> u32 {
        reg::tick()
    }

    fn random(&mut self) -> u32 {
        // xorshift, stirred with the free-running timer: session nonces and
        // advertising jitter, nothing that keeps a secret
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        self.rng ^ reg::tick()
    }

    fn address(&mut self) -> [u8; 6] {
        let mut a = [0u8; 6];
        flash::read(MAC_SECTOR, &mut a);
        a
    }

    fn advertise(&mut self, adv: &[u8], scan_rsp: &[u8], connect: &mut [u8; CONNECT_LEN]) -> Option<u32> {
        ble_radio::advertise(adv, scan_rsp, connect)
    }

    fn connection_reset(&mut self) {
        ble_radio::connection_reset();
    }

    fn connection_event(&mut self, event: &ConnEvent) -> EventResult {
        ble_radio::connection_event(event)
    }

    fn tx_free(&mut self) -> usize {
        ble_radio::tx_free()
    }

    fn tx_idle(&mut self) -> bool {
        ble_radio::tx_idle()
    }

    fn tx_push(&mut self, llid: u8, payload: &[u8]) -> bool {
        ble_radio::tx_push(llid, payload)
    }

    fn rx_pop(&mut self, payload: &mut [u8; LL_PAYLOAD_MAX]) -> Option<(u8, usize)> {
        ble_radio::rx_pop(payload)
    }

    fn nv_load(&mut self, buf: &mut [u8]) -> Option<usize> {
        let (off, len) = scan().0?;
        let n = len.min(buf.len());
        flash::read(MESH_SECTOR + off, &mut buf[..n]);
        Some(n)
    }

    fn nv_store(&mut self, data: &[u8]) {
        let len = data.len().min(RECORD_MAX);
        let mut free = scan().1;
        if free + 3 + len > SECTOR_SIZE {
            flash::erase_sector(MESH_SECTOR);
            free = 0;
        }
        let mut rec = [0u8; RECORD_MAX + 3];
        rec[0] = 0xa5;
        rec[1] = len as u8;
        rec[2..2 + len].copy_from_slice(&data[..len]);
        rec[2 + len] = checksum(&data[..len]);
        flash::write(MESH_SECTOR + free, &rec[..3 + len]);
    }

    fn ota_capacity(&mut self) -> u32 {
        // one update at a time: no room while Zigbee is downloading an image
        if staging::claim(Owner::Ble) {
            staging::SIZE
        } else {
            0
        }
    }

    fn ota_erase(&mut self, offset: u32) {
        let sector = staging::BASE + (offset as usize & !0xfff);
        if offset < staging::SIZE && staging::holds(Owner::Ble) && !flash::blank(sector) {
            flash::erase_sector(sector);
        }
    }

    fn ota_write(&mut self, offset: u32, data: &[u8]) {
        if offset + data.len() as u32 <= staging::SIZE && staging::holds(Owner::Ble) {
            flash::write(staging::BASE + offset as usize, data);
        }
    }

    fn ota_activate(&mut self) -> bool {
        staging::activate()
    }

    fn radio_busy(&mut self) -> bool {
        // (the time since the last frame counts from its end: one arriving now is traffic too)
        radio::receiving() || radio::quiet_ticks() < RADIO_QUIET
    }

    fn neighbours_talking(&mut self) -> bool {
        radio::receiving() || radio::rx_quiet_ticks() < RADIO_QUIET
    }

    fn mesh_listen(&mut self, channel: u8, access_code: u32, ticks: u32, pdu: &mut [u8; ADV_PDU_MAX]) -> Option<usize> {
        ble_radio::mesh_listen(channel, access_code, ticks, pdu)
    }

    fn mesh_send(&mut self, channels: &[u8], access_code: u32, pdu: &[u8]) -> bool {
        ble_radio::mesh_send(channels, access_code, pdu)
    }
}
