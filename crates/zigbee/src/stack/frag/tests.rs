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

fn addressing(counter: u8) -> super::Addressing {
    super::Addressing { peer: 0x0000, dst_ep: 1, src_ep: 1, cluster: 0x0000, profile: 0x0104, counter }
}

#[test]
fn a_long_answer_waits_for_the_one_being_sent_and_a_third_is_counted_as_failed() {
    let mut s = Stack::new(Scripted { random: Vec::new(), now: 1_000, sent: Vec::new(), deaf: Vec::new(), busy: false, attempts: 0, bx: None }, IDENTITY);
    let fail = crate::stack::diag::k::APS_TX_UCAST_FAIL;
    s.send_fragmented(addressing(1), &[1; 100], false);
    s.send_fragmented(addressing(2), &[2; 100], false);
    assert_eq!(s.diag.n[fail], 0);
    s.send_fragmented(addressing(3), &[3; 100], false);
    assert_eq!(s.diag.n[fail], 1, "no room for a third");
    assert_eq!((s.frag_out.a.counter, s.frag_next.a.counter), (1, 2));
    s.on_fragment_ack(0x0000, 1, 0, 1);
    s.on_fragment_ack(0x0000, 1, 1, 1);
    assert!(s.frag_out.active && !s.frag_next.active);
    assert_eq!((s.frag_out.a.counter, s.frag_out.block), (2, 0), "the second starts when the first is done");
    s.send_fragmented(addressing(4), &[4; 100], false);
    for at in [2_600, 4_200, 5_800, 7_400] {
        s.frag_timers(at);
    }
    assert_eq!((s.frag_out.a.counter, s.diag.n[fail]), (4, 2), "the next starts when the second fails");
}

#[test]
fn a_transfer_received_long_ago_does_not_hold_back_a_new_one_from_the_same_peer() {
    let mut s = Stack::new(Scripted { random: Vec::new(), now: 1_000, sent: Vec::new(), deaf: Vec::new(), busy: false, attempts: 0, bx: None }, IDENTITY);
    assert_eq!(s.on_fragment(addressing(9), super::FIRST, 2, &[1; 64]), Some(()));
    s.frag_timers(1_000 + 6_399);
    assert!(s.frag_in.active, "the transfer is still waited for");
    s.frag_timers(1_000 + 6_400);
    assert!(!s.frag_in.active && !s.frag_in.done, "the timers give up an expired transfer");
    s.frag_in.done = true;
    s.frag_timers(1_000 + 6_400);
    assert!(!s.frag_in.done, "and forget a finished one");
    s.hal.now = 1_000 + 6_400 + 0x8000_0000;
    assert_eq!(s.on_fragment(addressing(9), super::FIRST, 2, &[2; 64]), Some(()));
    assert!(s.frag_in.active && s.frag_in.got == 1, "24.9 days later it is a new transfer");
}
