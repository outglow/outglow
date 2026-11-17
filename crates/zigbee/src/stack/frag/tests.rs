use super::super::address_tests::{Scripted, IDENTITY};
use crate::stack::Stack;
extern crate std;
use std::vec::Vec;

#[test]
fn a_fragment_acknowledgement_counts_only_from_the_peer_of_the_transfer() {
    let mut s = Stack::new(Scripted { random: Vec::new(), now: 1_000, sent: Vec::new(), deaf: Vec::new(), busy: false, attempts: 0, bx: None }, IDENTITY);
    s.frag_out.active = true;
    s.frag_out.a.peer = 0x0000;
    s.frag_out.a.counter = 5;
    s.frag_out.len = 200;
    s.on_fragment_ack(0x1234, 5, 0, 1);
    assert_eq!(s.frag_out.block, 0, "another device");
    s.on_fragment_ack(0x0000, 5, 0, 1);
    assert_eq!(s.frag_out.block, 1);
}
