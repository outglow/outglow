extern crate std;

use super::State;
use crate::crypto::crc16_modbus;
use crate::hal::*;
use crate::{Ble, Identity};
use std::vec::Vec;

struct Flash {
    writes: Vec<u32>,
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
    fn ota_erase(&mut self, _: u32) {}
    fn ota_write(&mut self, offset: u32, _: &[u8]) {
        self.writes.push(offset);
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
    let mut ble = Ble::new(Flash { writes: Vec::new() }, ID);
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
