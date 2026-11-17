use super::super::address_tests::{Scripted, IDENTITY};
use crate::stack::{Seen, Stack, APS_DUPS};
extern crate std;
use std::vec::Vec;

#[test]
fn a_full_duplicate_table_gives_up_the_first_of_the_entries_that_end_soonest() {
    let mut s = Stack::new(Scripted { random: Vec::new(), now: 1_000, sent: Vec::new(), deaf: Vec::new(), busy: false, attempts: 0 }, IDENTITY);
    for (i, e) in s.aps_dups.iter_mut().enumerate() {
        *e = Seen { used: true, src: 0x2000 + i as u16, seq: 1, expires: 5_000 };
    }
    assert!(!s.aps_duplicate(0x3000, 9));
    assert!(s.aps_duplicate(0x2000 + APS_DUPS as u16 - 1, 1), "the last of them kept");
    assert!(!s.aps_duplicate(0x2000, 1), "the first given up");
}
