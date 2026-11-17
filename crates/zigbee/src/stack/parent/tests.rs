use super::super::address_tests::{Scripted, IDENTITY};
use super::{Owner, INDIRECT_MS};
use crate::stack::{Stack, INDIRECT};
extern crate std;
use std::vec::Vec;

fn stack() -> Stack<Scripted> {
    Stack::new(Scripted { random: Vec::new(), now: 1_000 }, IDENTITY)
}

fn child(s: &mut Stack<Scripted>, i: usize, short: u16, polled_at: u32, poll_ms: u32) {
    let c = &mut s.children[i];
    c.used = true;
    c.joined = true;
    c.short = short;
    c.polled_at = polled_at;
    c.poll_ms = poll_ms;
}

fn put(s: &mut Stack<Scripted>, slot: usize, short: u16, expires: u32) {
    let e = &mut s.indirect[slot];
    e.used = true;
    e.ext = false;
    e.addr = short as u64;
    e.expires = expires;
    e.broadcast = false;
}

#[test]
fn a_child_with_its_four_frames_waiting_gives_up_the_one_that_expires_first_the_earliest_placed_of_a_tie() {
    let mut s = stack();
    child(&mut s, 0, 0x0A00, 0, 0);
    for (slot, expires) in [(3, 5_000), (5, 2_000), (7, 2_000), (9, 6_000)] {
        put(&mut s, slot, 0x0A00, expires);
    }
    assert_eq!(s.indirect_slot(Owner::Child(0)), Some(5));
    child(&mut s, 1, 0x0B00, 0, 0);
    assert_eq!(s.indirect_slot(Owner::Child(1)), Some(0), "another child takes a free slot");
}

#[test]
fn a_full_pool_gives_up_the_frame_whose_child_polls_latest_after_it_expires() {
    let mut s = stack();
    child(&mut s, 0, 0x0A00, 1_000, 10_000);
    child(&mut s, 1, 0x0B00, 1_000, 12_000);
    child(&mut s, 2, 0x0C00, 1_000, 100);
    for slot in 0..INDIRECT {
        let owner = [0x0A00u16, 0x0B00, 0x0C00][slot % 3];
        put(&mut s, slot, owner, 1_000 + 5_000 + slot as u32);
    }
    assert_eq!(s.indirect_slot(Owner::Other(false, 0x0D00)), Some(1), "child 1's frame expires 8 s before its poll, the most");
    child(&mut s, 3, 0x0E00, 1_000, 1_000_000);
    assert_eq!(s.indirect_slot(Owner::Child(3)), None, "a frame for a child that polls later still is not worth one");
}

#[test]
fn a_full_pool_without_late_frames_gives_up_the_oldest_frame_of_whoever_holds_most() {
    let mut s = stack();
    for slot in 0..INDIRECT {
        let short = if slot < 10 { 0x0A00 } else { 0x0B00 + slot as u16 };
        put(&mut s, slot, short, 1_000 + INDIRECT_MS - slot as u32);
    }
    assert_eq!(s.indirect_slot(Owner::Other(false, 0x0D00)), Some(9));
}

#[test]
fn a_poll_that_leaves_frames_waiting_does_not_count_as_the_childs_poll_interval() {
    let mut s = stack();
    child(&mut s, 0, 0x0A00, 0, 0);
    put(&mut s, 0, 0x0A00, 1_000 + INDIRECT_MS);
    put(&mut s, 1, 0x0A00, 1_000 + INDIRECT_MS + 1);
    s.on_data_request(crate::mac::Addr::Short(0x0A00));
    assert!(s.children[0].more, "another frame waits");
    s.hal.now = 1_050;
    s.on_data_request(crate::mac::Addr::Short(0x0A00));
    assert!(!s.children[0].more);
    assert_eq!(s.children[0].poll_ms, 0, "50 ms after a poll that had more is no interval");
    s.hal.now = 8_050;
    s.on_data_request(crate::mac::Addr::Short(0x0A00));
    assert_eq!(s.children[0].poll_ms, 7_000);
}

#[test]
fn an_unauthenticated_child_goes_after_thirty_seconds_and_a_joined_end_device_after_its_timeout() {
    let mut s = stack();
    child(&mut s, 0, 0x0A00, 0, 0);
    s.children[0].joined = false;
    s.children[0].heard = 500;
    s.parent_timers(500 + 30_000);
    assert!(s.children[0].used);
    s.parent_timers(500 + 30_001);
    assert!(!s.children[0].used);
    child(&mut s, 1, 0x0B00, 0, 0);
    s.children[1].heard = 500;
    s.children[1].timeout = 0;
    let limit = s.children[1].timeout_ms();
    s.parent_timers(500 + limit);
    assert!(s.children[1].used);
    s.parent_timers(500 + limit + 1);
    assert!(!s.children[1].used);
}
