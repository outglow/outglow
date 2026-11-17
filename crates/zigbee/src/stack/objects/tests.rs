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
