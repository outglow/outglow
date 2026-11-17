use super::super::address_tests::{Scripted, IDENTITY};
use super::Candidate;
use crate::mac::Beacon;
use crate::stack::Stack;
extern crate std;
use std::vec::Vec;

fn stack() -> Stack<Scripted> {
    Stack::new(Scripted { random: Vec::new(), now: 1_000 }, IDENTITY)
}

fn candidate(short: u16, lqi: u8, router_capacity: bool, depth: u8) -> Candidate {
    Candidate {
        channel: 11,
        lqi,
        beacon: Beacon {
            pan: 0x1A62,
            short,
            permit_join: true,
            pan_coordinator: false,
            stack_profile: 2,
            protocol_version: 2,
            router_capacity,
            end_device_capacity: true,
            depth,
            ext_pan: 1,
            update_id: 0,
        },
    }
}

fn order(s: &Stack<Scripted>) -> Vec<u16> {
    s.scan_found.iter().flatten().map(|c| c.beacon.short).collect()
}

#[test]
fn candidates_are_ranked_by_room_for_routers_and_a_good_link_then_by_link_quality() {
    let mut s = stack();
    s.candidate_add(candidate(1, 200, false, 1));
    s.candidate_add(candidate(2, 100, true, 1));
    s.candidate_add(candidate(3, 60, true, 1));
    s.candidate_add(candidate(4, 160, true, 1));
    assert_eq!(order(&s), [4, 2, 1, 3], "a link of cost 3 at most and room first, then the LQI");
    s.candidate_add(candidate(5, 250, false, 1));
    assert_eq!(order(&s), [4, 2, 5, 1], "four are kept");
    s.candidate_add(candidate(3, 90, true, 1));
    assert_eq!(order(&s), [4, 2, 3, 5], "heard again, better: moved up");
    s.candidate_add(candidate(4, 120, true, 1));
    assert_eq!(order(&s), [4, 2, 3, 5], "heard again, worse: kept as it was");
    assert_eq!(s.scan_found[0].unwrap().lqi, 160);
}

#[test]
fn for_a_rejoin_the_shallowest_router_comes_first() {
    let mut s = stack();
    s.rejoin = Some(true);
    s.candidate_add(candidate(1, 250, true, 4));
    s.candidate_add(candidate(2, 120, true, 1));
    s.candidate_add(candidate(3, 200, true, 1));
    s.candidate_add(candidate(4, 250, false, 0));
    assert_eq!(order(&s), [3, 2, 1, 4]);
    let mut other = candidate(3, 200, true, 1);
    other.channel = 15;
    s.candidate_add(other);
    assert_eq!(order(&s), [3, 3, 2, 1], "the same router on another channel is another candidate");
    assert_eq!(s.candidate_take().map(|c| c.channel), Some(11));
    assert_eq!(order(&s), [3, 2, 1]);
}
