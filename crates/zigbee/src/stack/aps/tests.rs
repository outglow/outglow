use super::super::address_tests::{Scripted, IDENTITY};
use crate::stack::{Seen, Stack, APS_DUPS};
extern crate std;
use std::vec::Vec;

#[test]
fn a_full_duplicate_table_gives_up_the_first_of_the_entries_that_end_soonest() {
    let mut s = Stack::new(Scripted { random: Vec::new(), now: 1_000, sent: Vec::new(), deaf: Vec::new(), busy: false, attempts: 0, bx: None }, IDENTITY);
    for (i, e) in s.aps_dups.iter_mut().enumerate() {
        *e = Seen { used: true, src: 0x2000 + i as u16, seq: 1, expires: 5_000 };
    }
    assert!(!s.aps_duplicate(0x3000, 9));
    assert!(s.aps_duplicate(0x2000 + APS_DUPS as u16 - 1, 1), "the last of them kept");
    assert!(!s.aps_duplicate(0x2000, 1), "the first given up");
}

#[test]
fn a_frame_remembered_long_ago_does_not_make_a_new_one_with_the_same_counter_a_duplicate() {
    let mut s = Stack::new(Scripted { random: Vec::new(), now: 1_000, sent: Vec::new(), deaf: Vec::new(), busy: false, attempts: 0, bx: None }, IDENTITY);
    assert!(!s.aps_duplicate(0x0000, 7));
    s.net_timers(1_000 + 6_399);
    assert!(s.aps_dups.iter().any(|e| e.used), "the timers keep a frame still remembered");
    s.net_timers(1_000 + 6_400);
    assert!(s.aps_dups.iter().all(|e| !e.used), "the timers forget an expired frame");
    s.hal.now = 1_000 + 6_400 + 0x8000_0000;
    assert!(!s.aps_duplicate(0x0000, 7), "24.9 days later it is a new frame");
}
