use super::super::address_tests::{Scripted, IDENTITY};
use super::{Commissioning, Forward, Gpd, Pending, ProxyEntry, DMAX, DMIN_U, KEY_NWK};
use crate::mac::Addr;
use crate::stack::{Stack, State};
use crate::util::Reader;
extern crate std;
use std::vec::Vec;

#[test]
fn a_device_address_is_read_for_the_source_id_and_ieee_application_ids_only() {
    let b = [0x45, 0x23, 0x71, 0x01, 0x11, 0x22, 0x33, 0x44, 0x07];
    assert!(Gpd::read(0, &mut Reader::new(&b)) == Some((Gpd::Src(0x0171_2345), 0)));
    assert!(Gpd::read(2, &mut Reader::new(&b)) == Some((Gpd::Ieee(0x4433_2211_0171_2345), 0x07)));
    for app in [1, 3, 7] {
        assert!(Gpd::read(app, &mut Reader::new(&b)).is_none(), "{app}");
    }
}

#[test]
fn a_commissioning_frame_finds_no_room_while_two_notifications_wait() {
    let mut s = Stack::new(Scripted { random: Vec::new(), now: 1_000, sent: Vec::new(), deaf: Vec::new(), busy: false, attempts: 0, bx: None }, IDENTITY);
    s.state = State::Up;
    s.gp_mode = Some(Commissioning { until: 60_000, sink: 0x0000, unicast: true, exit_on_pairing: false });
    let waiting = Pending { at: 50_000, to: Forward::Sink(0x0000), alias: 0, seq: 0, then: 0, len: 3, zcl: [0; 96] };
    s.gp_pending = [Some(waiting), Some(waiting)];
    let _ = s.on_gpdf(0x31, Addr::None, &[0x0C, 0x45, 0x23, 0x71, 0x01, 0xE0, 0x02, 0x00, 0x00], 200, 0);
    assert!(s.gp_pending.iter().all(|p| p.is_some_and(|p| p.at == 50_000)));
    s.gp_pending = [Some(waiting), None];
    let _ = s.on_gpdf(0x32, Addr::None, &[0x0C, 0x45, 0x23, 0x71, 0x01, 0xE0, 0x02, 0x00, 0x00], 200, 0);
    assert!(s.gp_pending[1].is_some(), "with room it waits");
}

#[test]
fn a_pairing_under_the_network_key_without_its_own_or_a_shared_one_uses_the_network_key() {
    let mut s = Stack::new(Scripted { random: Vec::new(), now: 1_000, sent: Vec::new(), deaf: Vec::new(), busy: false, attempts: 0, bx: None }, IDENTITY);
    s.nib.key = [0x42; 16];
    s.gp_keys.shared_type = KEY_NWK;
    s.gp_keys.shared = [0; 16];
    let e = ProxyEntry { key_type: KEY_NWK, ..ProxyEntry::EMPTY };
    assert_eq!(s.gp_entry_key(&e, 0), Some([0x42; 16]));
    s.gp_keys.shared = [0x17; 16];
    assert_eq!(s.gp_entry_key(&e, 0), Some([0x17; 16]), "a shared key of its type first");
}

#[test]
fn a_device_announced_at_an_alias_in_use_is_answered_once_after_a_jitter() {
    let fresh = || {
        let mut s = Stack::new(Scripted { random: Vec::new(), now: 1_000, sent: Vec::new(), deaf: Vec::new(), busy: false, attempts: 0, bx: None }, IDENTITY);
        s.state = State::Up;
        s.proxies[0] = ProxyEntry { used: true, gpd: Gpd::Src(0x0171_2345), ..ProxyEntry::EMPTY };
        s
    };
    let mut s = fresh();
    s.hal.random = std::vec![DMAX - DMIN_U + 1];
    s.gp_device_announced(0x2345, 0x99);
    let p = s.gp_pending[0].unwrap();
    assert!(p.to == Forward::Announce && p.alias == 0x2345 && p.at == 1_000 + DMIN_U);
    s.gp_device_announced(0x2345, 0x99);
    assert!(s.gp_pending[1].is_none(), "one answer waits already");
    let other = Pending { at: 9_000, to: Forward::Announce, alias: 0x1111, seq: 0, then: 0, len: 0, zcl: [0; 96] };
    for waiting in [other, Pending { to: Forward::Sink(0x0000), alias: 0x2345, ..other }] {
        let mut s = fresh();
        s.gp_pending[0] = Some(waiting);
        s.gp_device_announced(0x2345, 0x99);
        assert!(s.gp_pending[1].is_some_and(|p| p.to == Forward::Announce && p.alias == 0x2345));
    }
}

#[test]
fn a_commissioning_window_that_ran_out_does_not_open_again_when_the_clock_wraps() {
    let mut s = Stack::new(Scripted { random: Vec::new(), now: 1_000, sent: Vec::new(), deaf: Vec::new(), busy: false, attempts: 0, bx: None }, IDENTITY);
    s.state = State::Up;
    s.gp_mode = Some(Commissioning { until: 61_000, sink: 0x0000, unicast: true, exit_on_pairing: false });
    s.gp_timers(60_999);
    assert!(s.gp_mode.is_some(), "the window is still open");
    s.gp_timers(61_000);
    assert!(s.gp_mode.is_none(), "the timers close the window");
    s.gp_timers(61_000 + 3_600_000);
    assert!(s.gp_commissioning(61_000 + 0x8000_0000 + 1).is_none(), "24.9 days after the window closed");
}

#[test]
fn a_device_frame_remembered_long_ago_is_taken_again_when_the_clock_wraps() {
    let mut s = Stack::new(Scripted { random: Vec::new(), now: 1_000, sent: Vec::new(), deaf: Vec::new(), busy: false, attempts: 0, bx: None }, IDENTITY);
    s.state = State::Up;
    s.gp_seen[0] = Some((Gpd::Src(0x0171_2345), 0x31, 3_000));
    s.gp_timers(2_999);
    assert!(s.gp_seen[0].is_some(), "still remembered");
    s.gp_timers(3_000);
    assert!(s.gp_seen[0].is_none(), "the timers forget an expired frame");
    let later = 3_000 + 0x8000_0000;
    s.hal.now = later;
    s.gp_mode = Some(Commissioning { until: later + 60_000, sink: 0x0000, unicast: true, exit_on_pairing: false });
    let _ = s.on_gpdf(0x31, Addr::None, &[0x0C, 0x45, 0x23, 0x71, 0x01, 0xE0, 0x02, 0x00, 0x00], 200, 0);
    assert!(s.gp_pending.iter().any(|p| p.is_some()), "24.9 days later the same counter is a new frame");
}
