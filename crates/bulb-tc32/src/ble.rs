use ble::hal::{ConnEvent, EventResult, Hal, ADV_PDU_MAX, CONNECT_LEN, LL_PAYLOAD_MAX};
use ble::Identity;

use crate::ble_radio;
use crate::board::BOARD;
use crate::cell::Shared;
use crate::radio;
use crate::flash;
use crate::reg;
use crate::staging::{self, Owner};

pub const IDENTITY: Identity = Identity {
    product: BOARD.ble_product,
    model: BOARD.model,
    firmware_revision: crate::zb::VERSION,
    hardware_revision: BOARD.hw_revision,
    tunable: BOARD.tunable(),
    ct_min: BOARD.ct_min,
    ct_max: BOARD.ct_max,
    colour: BOARD.rgb.is_some(),
};

pub(crate) const RADIO_QUIET: u32 = 3 * 16_000;

const MAC_SECTOR: usize = crate::board::BOARD.flash.mac;

pub const MESH_SECTORS: [usize; 2] = [crate::board::BOARD.flash.store + 0x3000, crate::board::BOARD.flash.mesh_spare];
const SECTOR_SIZE: usize = 0x1000;
const RECORD_MAX: usize = 64;
const RECORD_BUF: usize = 67;
const _: () = assert!(RECORD_BUF == RECORD_MAX + 3);
const MAIN: usize = MESH_SECTORS[0];
const SPARE: usize = MESH_SECTORS[1];
const BACKUP: [u8; 4] = [0x5a, b'M', b'S', 0x02];
const BACKUP_LEN: usize = 4;

fn checksum(data: &[u8]) -> u8 {
    data.iter().fold(0x5a, |c, b| c.rotate_left(1) ^ *b)
}

fn backed_up() -> bool {
    let mut h = [0u8; BACKUP_LEN];
    flash::read(SPARE, &mut h);
    h == BACKUP
}

fn current() -> Option<(usize, usize)> {
    scan(MAIN, 0).0.or_else(|| backed_up().then(|| scan(SPARE, BACKUP_LEN).0).flatten())
}

fn blank_at(addr: usize, len: usize) -> bool {
    let mut b = [0u8; RECORD_BUF];
    flash::read(addr, &mut b[..len]);
    b[..len].iter().all(|&x| x == 0xff)
}

fn scan(sector: usize, start: usize) -> (Option<(usize, usize)>, usize) {
    let mut last = None;
    let mut off = start;
    while off + 3 <= SECTOR_SIZE {
        let mut head = [0u8; 2];
        flash::read(sector + off, &mut head);
        let len = head[1] as usize;
        if head[0] != 0xa5 || len > RECORD_MAX || off + 3 + len > SECTOR_SIZE {
            break;
        }
        let mut rec = [0u8; RECORD_MAX + 1];
        flash::read(sector + off + 2, &mut rec[..len + 1]);
        if checksum(&rec[..len]) == rec[len] {
            last = Some((sector + off + 2, len));
        }
        off += 3 + len;
    }
    (last, off)
}

fn put(at: usize, rec: &[u8]) {
    flash::write(at + 1, &rec[1..]);
    flash::write(at, &[0xa5]);
}

fn drop_backup() {
    if backed_up() {
        flash::write(SPARE, &[0]);
    }
}

fn record(data: &[u8]) -> ([u8; RECORD_BUF], usize) {
    let len = data.len().min(RECORD_MAX);
    let mut rec = [0u8; RECORD_BUF];
    rec[1] = len as u8;
    rec[2..2 + len].copy_from_slice(&data[..len]);
    rec[2 + len] = checksum(&data[..len]);
    (rec, 3 + len)
}

fn rewrite(rec: &[u8], backup: bool) {
    if backup {
        if !flash::blank(SPARE) {
            flash::erase_sector(SPARE);
        }
        put(SPARE + BACKUP_LEN, rec);
        flash::write(SPARE, &BACKUP);
        flash::write(MAIN, &[0]);
    }
    flash::erase_sector(MAIN);
    put(MAIN, rec);
    drop_backup();
}

pub(crate) static TIDY: Shared<bool> = Shared::new(true);

pub fn tidy() -> bool {
    if !TIDY.get() {
        return false;
    }
    TIDY.set(false);
    let (last, free) = scan(MAIN, 0);
    let Some((at, len)) = last.filter(|_| free + RECORD_BUF > SECTOR_SIZE) else {
        return false;
    };
    let mut data = [0u8; RECORD_MAX];
    flash::read(at, &mut data[..len]);
    let (rec, n) = record(&data[..len]);
    rewrite(&rec[..n], true);
    true
}

pub fn erase_settings() {
    drop_backup();
    if !flash::blank(MAIN) {
        flash::write(MAIN, &[0]);
        flash::erase_sector(MAIN);
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
        let (at, len) = current()?;
        let n = len.min(buf.len());
        flash::read(at, &mut buf[..n]);
        Some(n)
    }

    fn nv_store(&mut self, data: &[u8]) {
        let (rec, n) = record(data);
        let rec = &rec[..n];
        let (last, free) = scan(MAIN, 0);
        TIDY.set(true);
        if free + rec.len() <= SECTOR_SIZE && blank_at(MAIN + free, rec.len()) {
            put(MAIN + free, rec);
            drop_backup();
            return;
        }
        rewrite(rec, last.is_some());
    }

    fn ota_capacity(&mut self) -> u32 {
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

#[cfg(test)]
#[path = "tests/ble.rs"]
mod tests;
