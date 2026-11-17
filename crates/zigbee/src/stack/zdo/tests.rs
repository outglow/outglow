use super::super::address_tests::{Scripted, IDENTITY};
use super::{MGMT_LQI_REQ, NODE_DESC_REQ, PARENT_ANNCE, RESPONSE};
use crate::util::Buf;
use crate::stack::Stack;
use crate::util::Reader;
extern crate std;
use std::vec::Vec;

fn stack() -> Stack<Scripted> {
    Stack::new(Scripted { random: Vec::new(), now: 1_000, sent: Vec::new(), deaf: Vec::new(), busy: false, attempts: 0, bx: None }, IDENTITY)
}

#[test]
fn a_node_descriptor_that_failed_or_is_not_the_coordinators_leaves_the_key_step_alone() {
    let desc = |status: u8, of: u16| {
        let o = of.to_le_bytes();
        let mut p = std::vec![status, o[0], o[1]];
        p.extend_from_slice(&[0; 8]);
        p.extend_from_slice(&(22u16 << 9).to_le_bytes());
        p
    };
    for (status, of, step) in [(0x80, 0x0000, 2), (0x00, 0x1234, 2), (0x00, 0x0000, 0)] {
        let mut s = stack();
        s.tclk_step = 2;
        let p = desc(status, of);
        let _ = s.on_zdo_response(0x0000, NODE_DESC_REQ | RESPONSE, Reader::new(&p));
        assert_eq!(s.tclk_step == 2, step == 2, "{status:02X} {of:04X}");
    }
}

#[test]
fn a_parent_announce_response_takes_away_only_children_not_heard_lately() {
    let mut s = stack();
    for (i, (ext, heard)) in [(0xA, 1_000), (0xB, 1_500)].into_iter().enumerate() {
        let c = &mut s.children[i];
        c.used = true;
        c.joined = true;
        c.ext = ext;
        c.heard = heard;
    }
    s.hal.now = 1_000;
    let mut p = std::vec![0x00, 3];
    for ext in [0xAu64, 0xB, 0xC] {
        p.extend_from_slice(&ext.to_le_bytes());
    }
    let _ = s.on_zdo_response(0x1000, PARENT_ANNCE | RESPONSE, Reader::new(&p));
    assert_eq!((s.children[0].used, s.children[1].used), (false, true));
}

#[test]
fn the_neighbour_table_lists_the_neighbours_and_the_joined_end_device_children() {
    let mut s = stack();
    s.neighbors[0].used = true;
    s.neighbors[0].short = 0x0000;
    for (i, (joined, capability, short)) in [(true, 0x80u8, 0x2000u16), (false, 0x80, 0x2001), (true, 0x8E, 0x2002), (true, 0x88, 0x2003)].into_iter().enumerate() {
        let c = &mut s.children[i];
        (c.used, c.joined, c.capability, c.short) = (true, joined, capability, short);
    }
    let mut rsp = Buf::new();
    let _ = s.zdo_request(MGMT_LQI_REQ, Reader::new(&[0]), true, &mut rsp);
    let r = rsp.as_slice();
    assert_eq!((r[0], r[1], r[2], r[3]), (0x00, 3, 0, 3));
    let short = |k: usize| u16::from_le_bytes([r[4 + k * 22 + 16], r[4 + k * 22 + 17]]);
    assert_eq!([short(0), short(1), short(2)], [0x0000, 0x2000, 0x2003]);
}
