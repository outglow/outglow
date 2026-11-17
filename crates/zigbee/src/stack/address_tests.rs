extern crate std;
use std::vec::Vec;

use super::{Identity, Stack, ROUTE_ACTIVE};
use crate::hal::{Hal, NvItem, RxMeta, TxStatus};

pub(crate) struct Scripted {
    pub random: Vec<u32>,
    pub now: u32,
    pub sent: Vec<Vec<u8>>,
    pub deaf: Vec<u16>,
    pub busy: bool,
}

impl Hal for Scripted {
    fn now_ms(&mut self) -> u32 {
        self.now
    }
    fn random(&mut self) -> u32 {
        if self.random.is_empty() {
            7
        } else {
            self.random.remove(0)
        }
    }
    fn ieee(&mut self) -> u64 {
        0x00124B0000000042
    }
    fn set_channel(&mut self, _channel: u8) {}
    fn set_filter(&mut self, _pan: u16, _short: u16) {}
    fn transmit(&mut self, mpdu: &[u8]) -> TxStatus {
        self.sent.push(mpdu.to_vec());
        match mpdu.get(5..7) {
            _ if self.busy => TxStatus::ChannelBusy,
            Some(&[lo, hi]) if self.deaf.contains(&u16::from_le_bytes([lo, hi])) => TxStatus::NoAck,
            _ => TxStatus::Ok,
        }
    }
    fn receive(&mut self, _buf: &mut [u8; 127]) -> Option<RxMeta> {
        None
    }
    fn nv_load(&mut self, _item: NvItem, _buf: &mut [u8]) -> Option<usize> {
        None
    }
    fn nv_store(&mut self, _item: NvItem, _data: &[u8]) {}
    fn nv_erase(&mut self) {}
}

pub(crate) const IDENTITY: Identity = Identity {
    manufacturer: "t",
    model: "t",
    sw_build: "t",
    manufacturer_code: 0x1141,
    app_version: 1,
    hw_version: 1,
    color_temperature: false,
    ct_min: 153,
    ct_max: 454,
    color: false,
    image_type: 0x0390,
    file_version: 1,
    touchlink_master_key: None,
    install_code: None,
    ble: false,
    sense: false,
};

#[test]
fn a_new_address_is_none_of_those_in_use_and_never_a_reserved_one() {
    let mut s = Stack::new(Scripted { random: Vec::new(), now: 0, sent: Vec::new(), deaf: Vec::new(), busy: false }, IDENTITY);
    s.nib.short = 0x0101;
    s.children[3].used = true;
    s.children[3].short = 0x0202;
    s.children[4].short = 0x0606;
    s.neighbors[2].used = true;
    s.neighbors[2].short = 0x0303;
    s.routes[1].status = ROUTE_ACTIVE;
    s.routes[1].dst = 0x0404;
    s.addr_map[0] = (0x00124B0000000099, 0x0505);
    s.addr_map[1] = (0, 0x0707);
    s.hal.random = [0xFFF7, 0x0101, 0x0202, 0x0303, 0x0404, 0x0505, 0xFFF7 + 0x0606].to_vec();
    assert_eq!(s.new_address(), 0x0606, "0, its own, a child's, a neighbour's, a route's and a known address are taken; a free slot's is not");
    s.hal.random = [0x0707].to_vec();
    assert_eq!(s.new_address(), 0x0707, "an address map entry without an extended address holds no address");
    s.hal.random = [0xFFFF + 0xFFF7].to_vec();
    assert!(s.new_address() < 0xFFF7);
}

#[test]
fn after_eight_taken_draws_the_lowest_free_address_is_given() {
    let mut s = Stack::new(Scripted { random: Vec::new(), now: 0, sent: Vec::new(), deaf: Vec::new(), busy: false }, IDENTITY);
    s.nib.short = 0x0001;
    s.children[0].used = true;
    s.children[0].short = 0x0002;
    s.hal.random = [0x0001, 0x0002, 0x0001, 0x0002, 0x0001, 0x0002, 0x0001, 0x0002, 0x0909].to_vec();
    assert_eq!(s.new_address(), 0x0003);
    assert_eq!(s.hal.random, [0x0909].to_vec(), "eight draws and no more");
}

#[test]
fn the_sequence_numbers_start_from_the_bytes_of_one_random_number() {
    let s = Stack::new(Scripted { random: [0x1234_5678].to_vec(), now: 0, sent: Vec::new(), deaf: Vec::new(), busy: false }, IDENTITY);
    assert_eq!((s.mac_seq, s.nwk_seq, s.aps_counter, s.zdo_seq), (0x78, 0x56, 0x34, 0x12));
}

#[test]
fn the_network_opens_after_a_join_only_once_the_device_announce_is_out() {
    let mut s = Stack::new(Scripted { random: Vec::new(), now: 1_000, sent: Vec::new(), deaf: Vec::new(), busy: false }, IDENTITY);
    s.state = super::State::Up;
    s.steering = super::join::STEERING_OPEN_DUE;
    s.announce_at = Some(50_000);
    s.run_timers(1_000);
    assert_eq!((s.steering, s.permit_until), (super::join::STEERING_OPEN_DUE, None));
    s.announce_at = None;
    s.run_timers(1_000);
    assert_eq!(s.steering, 0);
    assert!(s.permit_until.is_some());
    s.permit_until = None;
    s.run_timers(1_000);
    assert_eq!(s.permit_until, None, "once");
}
