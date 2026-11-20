extern crate std;

use super::State;
use crate::crypto::crc16_modbus;
use crate::hal::*;
use crate::{Ble, Identity};
use std::vec::Vec;

struct Flash {
    writes: Vec<u32>,
    clean: Vec<bool>,
    dirty_writes: u32,
}

impl Hal for Flash {
    fn now(&mut self) -> u32 {
        0
    }
    fn random(&mut self) -> u32 {
        7
    }
    fn address(&mut self) -> [u8; 6] {
        [1, 2, 3, 4, 5, 6]
    }
    fn advertise(&mut self, _: &[u8], _: &[u8], _: &mut [u8; CONNECT_LEN]) -> Option<u32> {
        None
    }
    fn connection_reset(&mut self) {}
    fn connection_event(&mut self, _: &ConnEvent) -> EventResult {
        EventResult::default()
    }
    fn tx_free(&mut self) -> usize {
        8
    }
    fn tx_idle(&mut self) -> bool {
        true
    }
    fn tx_push(&mut self, _: u8, _: &[u8]) -> bool {
        true
    }
    fn rx_pop(&mut self, _: &mut [u8; LL_PAYLOAD_MAX]) -> Option<(u8, usize)> {
        None
    }
    fn nv_load(&mut self, _: &mut [u8]) -> Option<usize> {
        None
    }
    fn nv_store(&mut self, _: &[u8]) {}
    fn ota_capacity(&mut self) -> u32 {
        0x64000
    }
    fn ota_erase(&mut self, offset: u32) {
        let s = (offset / 0x1000) as usize * 0x1000;
        self.clean[s..s + 0x1000].iter_mut().for_each(|c| *c = true);
    }
    fn ota_write(&mut self, offset: u32, data: &[u8]) {
        self.writes.push(offset);
        let r = offset as usize..offset as usize + data.len();
        if !self.clean[r.clone()].iter().all(|c| *c) {
            self.dirty_writes += 1;
        }
        self.clean[r].iter_mut().for_each(|c| *c = false);
    }
    fn ota_activate(&mut self) -> bool {
        true
    }
    fn radio_busy(&mut self) -> bool {
        false
    }
    fn neighbours_talking(&mut self) -> bool {
        false
    }
    fn mesh_listen(&mut self, _: u8, _: u32, _: u32, _: &mut [u8; ADV_PDU_MAX]) -> Option<usize> {
        None
    }
    fn mesh_send(&mut self, _: &[u8], _: u32, _: &[u8]) -> bool {
        true
    }
}

fn flash() -> Flash {
    Flash { writes: Vec::new(), clean: std::vec![false; 0x64000], dirty_writes: 0 }
}

const ID: Identity = Identity {
    product: 0x20a7,
    model: "m",
    firmware_revision: "3.0.2",
    hardware_revision: "4",
    tunable: false,
    ct_min: 153,
    ct_max: 454,
    colour: false,
};

fn packet(index: u16, block: &[u8]) -> Vec<u8> {
    let mut p = index.to_le_bytes().to_vec();
    p.extend_from_slice(block);
    let crc = crc16_modbus(&p);
    p.extend_from_slice(&crc.to_le_bytes());
    p
}

#[test]
fn a_transfer_needing_more_blocks_than_a_block_index_counts_fails_rather_than_wrapping_to_block_zero() {
    let mut ble = Ble::new(flash(), ID);
    let mut image = std::vec![0u8; 0x10040];
    image[8..12].copy_from_slice(b"KNLT");
    let declared = image.len() as u32;
    image[0x18..0x1c].copy_from_slice(&declared.to_le_bytes());
    image.iter().take(0xffff).enumerate().for_each(|(i, b)| ble.ota_write(&packet(i as u16, &[*b])));
    assert_eq!((ble.ota.state, ble.ota.failed, ble.ota.next), (State::Receiving, false, 0xffff));
    ble.ota_write(&packet(0xffff, &[image[0xffff]]));
    assert_eq!((ble.ota.state, ble.ota.failed), (State::Done, true));
    ble.ota_write(&packet(0, &[image[0]]));
    assert_eq!(ble.hal.writes.len(), 0xffff + 1, "a fresh transfer from block zero is the only write after it");
    assert_eq!(ble.hal.writes.last(), Some(&0));
}

fn image(len: usize) -> Vec<u8> {
    let mut image: Vec<u8> = (0..len).map(|i| (i * 7 + 3) as u8).collect();
    image[8..12].copy_from_slice(b"KNLT");
    image[0x18..0x1c].copy_from_slice(&(len as u32).to_le_bytes());
    let mut crc = 0xffff_ffffu32;
    for b in &image[..len - 4] {
        crc = crate::crypto::crc32_update(crc, *b);
    }
    image[len - 4..].copy_from_slice(&crc.to_le_bytes());
    image
}

fn send(ble: &mut Ble<Flash>, image: &[u8], d: usize) {
    let n = image.len().div_ceil(d);
    for i in 0..n {
        let mut b = image[i * d..(i * d + d).min(image.len())].to_vec();
        b.resize(d, 0xff);
        ble.ota_write(&packet(i as u16, &b));
    }
    let mut end = (n as u16).to_le_bytes().to_vec();
    let c = crc16_modbus(&end);
    end.extend_from_slice(&c.to_le_bytes());
    ble.ota_write(&end);
}

#[test]
fn every_block_lands_in_erased_flash_whatever_the_erase_timer_got_done() {
    for (sectors_by_timer, second_erase) in [(0usize, false), (1, false), (3, false), (12, false), (3, true)] {
        let mut ble = Ble::new(flash(), ID);
        let img = image(0x5000 + 123);
        let d = 16;
        let mut erase = std::vec![0u8; d + 4];
        erase[2] = (img.len() / 4096 + 2) as u8;
        ble.ota_write(&erase);
        for _ in 0..sectors_by_timer {
            ble.ota_timers(10 * TICKS_PER_MS);
        }
        if second_erase {
            ble.ota_write(&erase);
        }
        send(&mut ble, &img, d);
        assert_eq!((ble.ota.state, ble.ota.failed, ble.hal.dirty_writes), (State::Done, false, 0), "{sectors_by_timer} {second_erase}");
    }
}

#[test]
fn a_second_transfer_after_a_failed_one_writes_only_erased_flash() {
    let mut ble = Ble::new(flash(), ID);
    let img = image(0x3000 + 5);
    send(&mut ble, &img[..0x2000], 16);
    ble.ota_write(&packet(7, &[0; 16]));
    assert!(ble.ota.failed);
    send(&mut ble, &img, 16);
    assert_eq!((ble.ota.state, ble.ota.failed, ble.hal.dirty_writes), (State::Done, false, 0));
    let mut ble2 = Ble::new(flash(), ID);
    send(&mut ble2, &img[..0x2000], 16);
    ble2.ota_write(&packet(7, &[0; 16]));
    let mut erase = std::vec![0u8; 20];
    erase[2] = 5;
    ble2.ota_write(&erase);
    ble2.ota_timers(10 * TICKS_PER_MS);
    send(&mut ble2, &img, 16);
    assert_eq!((ble2.ota.state, ble2.ota.failed, ble2.hal.dirty_writes), (State::Done, false, 0));
}
