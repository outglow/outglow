use super::super::address_tests::{Scripted, IDENTITY};
use super::{attribute, k, Attr, Diag, Octets, Src, T_OCTETS, T_U32};
use crate::stack::Stack;
extern crate std;
use std::vec::Vec;

#[test]
fn a_diagnostics_attribute_is_a_number_of_its_type_or_octets() {
    assert_eq!(attribute(false, 0x0100).map(Attr::ty), Some(T_U32));
    assert!(attribute(false, 0x0100).and_then(Attr::number).is_some_and(|(ty, src)| ty == T_U32 && src == Src::Counter(k::MAC_RX_BCAST)));
    assert!(attribute(true, 0x5032) == Some(Attr::Octets(Octets::ExtPan)));
    assert_eq!(attribute(true, 0x5032).map(Attr::ty), Some(T_OCTETS));
    assert!(attribute(true, 0x5032).and_then(Attr::number).is_none());
}

#[test]
fn the_average_retries_of_nothing_sent_is_zero() {
    let mut d = Diag::new(0);
    assert_eq!((d.average_retries(), d.average_retries_x100()), (0, 0));
    d.n[k::APS_SENT] = 4;
    d.n[k::APS_MAC_RETRIES] = 6;
    assert_eq!((d.average_retries(), d.average_retries_x100()), (2, 150));
}

#[test]
fn without_a_noise_measurement_the_next_try_is_a_second_later() {
    let mut s = Stack::new(Scripted { random: Vec::new(), now: 1_000, sent: Vec::new(), deaf: Vec::new(), busy: false }, IDENTITY);
    s.diag.noise_at = 1_000;
    s.diag_noise(1_000);
    assert_eq!(s.diag.noise_at, 2_000);
}
