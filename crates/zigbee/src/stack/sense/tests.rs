use super::super::address_tests::{Scripted, IDENTITY};
use super::{A_LINES, A_LINE_SENS, A_OCCUPANCY, OCCUPANCY, SENSE};
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

fn sensing(random: Vec<u32>, busy: bool) -> Stack<Scripted> {
    let mut s = Stack::new(Scripted { random: Vec::new(), now: 100_000, sent: Vec::new(), deaf: Vec::new(), busy, attempts: 0, bx: None }, SENSING);
    s.sense.rate = 5;
    s.sense.started = 0;
    s.sense.next_probe = 100_000;
    s.sense.drained = true;
    s.hal.random = random;
    s
}

#[test]
fn a_probe_sent_waits_a_period_less_an_eighth_plus_up_to_a_quarter_and_one_not_sent_tries_again_soon() {
    let mut s = sensing(std::vec![50], false);
    s.sense_probe(100_000);
    assert_eq!(s.sense.next_probe, 100_000 + 200 - 25 + 50);
    let mut s = sensing(std::vec![3], true);
    s.sense_probe(100_000);
    assert_eq!(s.sense.next_probe, 100_005);
}

#[test]
fn the_sensing_clusters_and_long_attributes_are_where_they_belong() {
    let s = sensing(Vec::new(), false);
    assert!(s.sense_cluster(1, OCCUPANCY) && s.sense_cluster(1, SENSE));
    assert!(!s.sense_cluster(2, OCCUPANCY) && !s.sense_cluster(1, 0x0006));
    assert!(s.sense_long(SENSE, A_LINES) && s.sense_long(SENSE, A_LINE_SENS));
    assert!(!s.sense_long(OCCUPANCY, A_LINES) && !s.sense_long(SENSE, A_OCCUPANCY));
    let plain = Stack::new(Scripted { random: Vec::new(), now: 0, sent: Vec::new(), deaf: Vec::new(), busy: false, attempts: 0, bx: None }, IDENTITY);
    assert!(!plain.sense_cluster(1, OCCUPANCY) && !plain.sense_long(SENSE, A_LINES));
}
