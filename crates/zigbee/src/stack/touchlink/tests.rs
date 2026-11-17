use super::super::address_tests::{Scripted, IDENTITY};
use super::{Transaction, NETWORK_START_REQUEST, NETWORK_UPDATE_REQUEST, SCAN_REQUEST};
use crate::mac::{self, Addr};
use crate::stack::{Stack, State, NO_TRUST_CENTER};
extern crate std;
use std::vec::Vec;

const INITIATOR: u64 = 0x00124B00_0000_0077;
const TRANSACTION: u32 = 0x1234_5678;

fn stack(random: Vec<u32>) -> Stack<Scripted> {
    Stack::new(Scripted { random, now: 1_000, sent: Vec::new(), deaf: Vec::new(), busy: false, attempts: 0, bx: None }, IDENTITY)
}

fn interpan(s: &mut Stack<Scripted>, cmd: u8, fields: &[u8]) {
    let h = mac::Header {
        frame_type: mac::FRAME_DATA,
        security: false,
        pending: false,
        ack_request: false,
        seq: 1,
        dst_pan: 0xFFFF,
        dst: Addr::Short(0xFFFF),
        src_pan: 0x1A62,
        src: Addr::Ext(INITIATOR),
    };
    let mut p = std::vec![0x0B, 0x00, 0x0B, 0x00, 0x10, 0x5E, 0xC0, 0x11, 0x01, cmd];
    p.extend_from_slice(&TRANSACTION.to_le_bytes());
    p.extend_from_slice(fields);
    let _ = s.on_interpan(&h, &p, 200);
}

fn in_transaction(s: &mut Stack<Scripted>) {
    s.touchlink = Some(Transaction { id: TRANSACTION, initiator: INITIATOR, until: 9_000, response: 5, pan: 0x4321 });
}

#[test]
fn a_scan_makes_up_an_odd_response_identifier_and_a_pan_of_1_to_0xfffe() {
    let mut s = stack(std::vec![0, 4, 0xFFFE + 7]);
    interpan(&mut s, SCAN_REQUEST, &[0x05, 0x12]);
    let t = s.touchlink.unwrap();
    assert_eq!((t.response, t.pan), (5, 8));
}

#[test]
fn a_network_update_is_taken_only_when_newer_by_less_than_half_the_count() {
    for (update_id, taken) in [(0x7F, true), (0x80, false)] {
        let mut s = stack(Vec::new());
        s.state = State::Up;
        s.nib.tc_ext = NO_TRUST_CENTER;
        s.nib.ext_pan = 0xDD;
        s.nib.pan = 0x1A62;
        s.nib.channel = 11;
        in_transaction(&mut s);
        let mut f = 0xDDu64.to_le_bytes().to_vec();
        f.extend_from_slice(&[update_id, 20, 0x62, 0x1A]);
        interpan(&mut s, NETWORK_UPDATE_REQUEST, &f);
        assert_eq!(s.nib.channel == 20, taken, "{update_id:02X}");
    }
}

#[test]
fn a_network_start_makes_up_an_odd_extended_pan_from_the_bulbs_and_needs_a_short_address_below_0xfff8() {
    let start = |short: u16| {
        let mut f = 0u64.to_le_bytes().to_vec();
        f.push(0);
        f.extend_from_slice(&[0x11; 16]);
        f.push(15);
        f.extend_from_slice(&0x1234u16.to_le_bytes());
        f.extend_from_slice(&short.to_le_bytes());
        f
    };
    let mut s = stack(std::vec![0, 0x1111_2222, 0x3333_4444]);
    in_transaction(&mut s);
    interpan(&mut s, NETWORK_START_REQUEST, &start(0xFFF7));
    let f = s.hal.sent.last().unwrap().clone();
    let tail = &f[f.len() - 13..];
    let ext = (s.ext ^ 0x1111_2222_3333_4444) | 1;
    assert_eq!(tail[0], 0, "valid");
    assert_eq!(tail[1..9], ext.to_le_bytes());
    let mut s = stack(std::vec![0, 0x1111_2222, 0x3333_4444]);
    in_transaction(&mut s);
    interpan(&mut s, NETWORK_START_REQUEST, &start(0xFFF8));
    let f = s.hal.sent.last().unwrap().clone();
    assert_eq!(f[f.len() - 13], 1, "a reserved address");
}
