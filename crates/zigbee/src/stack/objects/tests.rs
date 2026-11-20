use super::super::address_tests::{Scripted, IDENTITY};
use super::{XReport, ANALOG_INPUT};
use crate::stack::{Identity, Stack};
extern crate std;
use std::vec::Vec;

const COLOUR: Identity = Identity { color: true, ..IDENTITY };

fn stack() -> Stack<Scripted> {
    Stack::new(Scripted { random: Vec::new(), now: 100_000, sent: Vec::new(), deaf: Vec::new(), busy: false, attempts: 0, bx: None }, COLOUR)
}

fn reported(s: &mut Stack<Scripted>, r: XReport) -> bool {
    s.xreports[0] = r;
    s.xreport_timers(100_000);
    s.xreports[0].last_sent == 100_000
}

#[test]
fn a_number_is_reported_from_the_minimum_interval_on_for_a_change_of_at_least_the_reportable_change() {
    let x = XReport { ep: 1, cluster: 0x0300, attr: 0x0003, mfr: false, min_s: 5, max_s: 0, change: 0x0100, last: 0x1000, last_sent: 95_000 };
    for (now_x, since, want) in [(0x1200u16, 5_000u32, true), (0x1200, 4_999, false), (0x1010, 5_000, false), (0x0F00, 5_000, true), (0x1000, 5_000, false)] {
        let mut s = stack();
        s.light.x = now_x;
        assert_eq!(reported(&mut s, XReport { last_sent: 100_000 - since, ..x }), want, "{now_x:04X} {since}");
    }
}

#[test]
fn a_single_value_is_reported_for_a_change_either_way_of_at_least_the_reportable_change() {
    let r = XReport { ep: 6, cluster: ANALOG_INPUT, attr: 0x0055, mfr: false, min_s: 0, max_s: 0, change: 0, last: 0, last_sent: 0 };
    for (children, last, step, want) in [(4usize, 5.0f32, 1.2f32, false), (4, 5.0, 1.0, true), (6, 5.0, 1.0, true), (4, 5.0, 3.0, false)] {
        let mut s = stack();
        for c in s.children[..children].iter_mut() {
            c.used = true;
            c.joined = true;
        }
        assert_eq!(reported(&mut s, XReport { last: last.to_bits(), change: step.to_bits(), ..r }), want, "{children} {last} {step}");
    }
}

#[test]
fn a_binding_whose_address_is_unknown_is_asked_for_again_however_long_after_the_last_question() {
    let mut s = stack();
    s.state = crate::stack::State::Up;
    s.nib.short = 0x0042;
    s.bindings[0] = crate::stack::Binding { used: true, src_ep: 1, cluster: 0x0006, group: false, dst: 0x00124B00_0000_7777, dst_ep: 1 };
    let t = s.hal.now;
    let sent = s.hal.sent.len();
    s.send_report(1, 0x0006, false, 0, 0x10, 1);
    assert!(s.hal.sent.len() > sent, "the first NWK_addr_req");
    s.hal.now = t + 9_999;
    let sent = s.hal.sent.len();
    s.send_report(1, 0x0006, false, 0, 0x10, 1);
    assert_eq!(s.hal.sent.len(), sent, "not again within ten seconds");
    s.hal.now = t + 10_000;
    s.send_report(1, 0x0006, false, 0, 0x10, 1);
    assert!(s.hal.sent.len() > sent, "again after ten seconds");
    let t = t + 10_000;
    s.hal.now = t + 10_000 + 0x8000_0000;
    let sent = s.hal.sent.len();
    s.send_report(1, 0x0006, false, 0, 0x10, 1);
    assert!(s.hal.sent.len() > sent, "24.9 days later the address is asked for again");
}

#[test]
fn the_remaining_time_of_a_transition_long_over_stays_zero_when_the_clock_wraps() {
    let mut s = stack();
    let t = s.hal.now;
    s.level_until = Some(t + 1_000);
    s.color_until = Some(t + 2_000);
    s.timers(t + 999);
    assert_eq!(s.level_until, Some(t + 1_000), "the transition is still running");
    s.hal.now = t;
    assert_eq!(s.light_settings().remaining_tenths, 10, "a second to go");
    s.hal.now = t + 999;
    assert_eq!(s.light_settings().remaining_tenths, 0);
    s.hal.now = t + 1_001;
    assert_eq!(s.light_settings().remaining_tenths, 0, "over before the timers tick");
    s.timers(t + 1_000);
    assert_eq!((s.level_until, s.color_until), (None, Some(t + 2_000)), "the timers end the transition that is over");
    s.timers(t + 2_000);
    assert_eq!(s.color_until, None);
    s.hal.now = t + 1_000 + 0x8000_0000 + 5_000;
    assert_eq!(s.light_settings().remaining_tenths, 0, "24.9 days after the transition ended");
}

#[test]
fn an_identify_over_before_the_timers_tick_is_no_longer_identifying() {
    let mut s = stack();
    let t = s.hal.now;
    s.identify_until = Some(t - 5);
    s.on_zcl(0x0000, 1, 1, 0x0004, &[0x01, 7, 0x05, 0x34, 0x12], true);
    assert!(!s.groups.contains(&0x1234), "Add Group If Identifying after the identify ended");
}
