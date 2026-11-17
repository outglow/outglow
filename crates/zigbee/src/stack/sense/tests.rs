use super::super::address_tests::{Scripted, IDENTITY};
use crate::stack::{Identity, Stack};
extern crate std;
use std::vec::Vec;

const SENSING: Identity = Identity { sense: true, ..IDENTITY };

#[test]
fn a_probe_waits_two_to_nine_milliseconds_for_the_radio_to_drain() {
    let mut s = Stack::new(Scripted { random: Vec::new(), now: 100_000, sent: Vec::new(), deaf: Vec::new(), busy: false, attempts: 0, bx: None }, SENSING);
    s.sense.rate = 5;
    s.sense.started = 0;
    s.sense.next_probe = 100_000;
    s.sense.drained = false;
    s.hal.random = std::vec![3];
    s.sense_probe(100_000);
    assert_eq!(s.sense.next_probe, 100_005);
}

#[test]
fn the_same_light_again_is_no_change() {
    let mut s = Stack::new(Scripted { random: Vec::new(), now: 100_000, sent: Vec::new(), deaf: Vec::new(), busy: false, attempts: 0, bx: None }, SENSING);
    s.sense_light(true, 100, 300);
    s.sense_light(true, 100, 300);
    assert_eq!(s.sense.light_changes, 0);
    s.sense_light(true, 101, 300);
    assert_eq!(s.sense.light_changes, 1);
}
