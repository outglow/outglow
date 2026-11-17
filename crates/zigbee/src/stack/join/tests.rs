use super::super::address_tests::{Scripted, IDENTITY};
use super::Candidate;
use crate::mac::Beacon;
use crate::stack::Stack;
extern crate std;
use std::vec::Vec;

fn stack() -> Stack<Scripted> {
    Stack::new(Scripted { random: Vec::new(), now: 1_000, sent: Vec::new(), deaf: Vec::new(), busy: false, attempts: 0, bx: None }, IDENTITY)
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

fn header(src: crate::mac::Addr, dst: crate::mac::Addr) -> crate::mac::Header {
    crate::mac::Header { frame_type: crate::mac::FRAME_CMD, security: false, pending: false, ack_request: false, seq: 1, dst_pan: 0x1A62, dst, src_pan: 0x1A62, src }
}

#[test]
fn a_bulb_that_is_not_up_answers_no_mac_command() {
    use crate::mac::Addr;
    let mut s = stack();
    s.nib.short = 0x1234;
    s.nib.pan = 0x1A62;
    let me = Addr::Short(0x1234);
    s.on_mac_command(&header(Addr::Ext(7), me), &[crate::mac::CMD_ASSOC_REQ, 0x80]);
    s.on_mac_command(&header(Addr::Short(0x0A00), me), &[crate::mac::CMD_DATA_REQ]);
    s.on_mac_command(&header(Addr::Ext(7), Addr::Short(0xFFFF)), &[crate::mac::CMD_ORPHAN]);
    s.on_mac_command(&header(Addr::None, Addr::Short(0xFFFF)), &[crate::mac::CMD_BEACON_REQ]);
    s.on_mac_command(&header(Addr::Ext(9), Addr::Ext(s.ext)), &[crate::mac::CMD_ASSOC_RSP, 0x34, 0x12, 0x00]);
    assert!(s.hal.sent.is_empty());
    assert!(s.beacon_at.is_none());
    assert_eq!(s.nib.short, 0x1234, "an association response while not waiting for one is no address");
}

fn beacon(permit_join: bool, router_capacity: bool, stack_profile: u8, pan: u16) -> Candidate {
    let mut c = candidate(0x0001, 200, router_capacity, 1);
    c.beacon.permit_join = permit_join;
    c.beacon.stack_profile = stack_profile;
    c.beacon.pan = pan;
    c
}

#[test]
fn a_scan_takes_only_beacons_of_zigbee_pro_routers_that_let_it_in() {
    let mut s = stack();
    s.state = crate::stack::State::Scan;
    for (c, taken) in [
        (beacon(true, true, 2, 0x1A62), true),
        (beacon(false, true, 2, 0x1A63), false),
        (beacon(true, false, 2, 0x1A64), false),
        (beacon(true, true, 1, 0x1A65), false),
        (beacon(true, true, 2, 0xFFFF), false),
    ] {
        let mut f = crate::util::Buf::new();
        c.beacon.write(1, &mut f);
        let (h, off) = crate::mac::Header::parse(f.as_slice()).unwrap();
        s.scan_found = [None; super::CANDIDATES];
        s.on_beacon(&h, &f.as_slice()[off..], 200);
        assert_eq!(s.scan_found[0].is_some(), taken, "{:?}", c.beacon);
    }
}

fn assoc_rsp(s: &mut Stack<Scripted>, src: crate::mac::Addr, short: u16) {
    let h = crate::mac::Header {
        frame_type: crate::mac::FRAME_CMD,
        security: false,
        pending: false,
        ack_request: true,
        seq: 1,
        dst_pan: 0x1A62,
        dst: crate::mac::Addr::Ext(s.ext),
        src_pan: 0x1A62,
        src,
    };
    let a = short.to_le_bytes();
    s.on_mac_command(&h, &[crate::mac::CMD_ASSOC_RSP, a[0], a[1], 0x00]);
}

#[test]
fn an_association_starts_the_tables_anew_with_the_parent_as_the_one_confirmed_neighbour() {
    let mut s = stack();
    s.state = crate::stack::State::AssocWait;
    s.nib.parent = 0x0000;
    s.join_lqi = 180;
    s.neighbors[3].used = true;
    s.routes[2].status = crate::stack::ROUTE_ACTIVE;
    s.discoveries[1].used = true;
    s.seen[4].used = true;
    s.aps_dups[2].used = true;
    s.held[1].used = true;
    s.bcast[2].used = true;
    assoc_rsp(&mut s, crate::mac::Addr::Ext(0x00124B00_0000_0001), 0x1234);
    assert_eq!((s.state, s.nib.short), (crate::stack::State::WaitKey, 0x1234));
    let n = s.neighbors[0];
    assert_eq!((n.used, n.short, n.ext, n.lqi, n.age, n.confirmed), (true, 0x0000, 0x00124B00_0000_0001, 180, 0, true));
    assert!(!s.neighbors[3].used && s.routes[2].status == crate::stack::ROUTE_FREE && !s.discoveries[1].used);
    assert!(!s.seen[4].used && !s.aps_dups[2].used && !s.held[1].used && !s.bcast[2].used);
    let mut s = stack();
    s.state = crate::stack::State::AssocWait;
    s.nib.parent = 0x0000;
    assoc_rsp(&mut s, crate::mac::Addr::Short(0x0000), 0x1234);
    assert_eq!(s.neighbors[0].ext, 0, "a parent known only by its short address");
}

use crate::mac::{self, Addr};
use crate::stack::State;

fn mac_cmd(s: &mut Stack<Scripted>, dst: Addr, dst_pan: u16, src: Addr, payload: &[u8]) {
    let h = mac::Header { frame_type: mac::FRAME_CMD, security: false, pending: false, ack_request: true, seq: 1, dst_pan, dst, src_pan: dst_pan, src };
    s.on_mac_command(&h, payload);
}

#[test]
fn a_candidate_heard_again_with_a_better_link_moves_up() {
    let mut s = stack();
    s.candidate_add(candidate(1, 120, true, 1));
    s.candidate_add(candidate(2, 150, true, 1));
    s.candidate_add(candidate(1, 200, true, 1));
    assert_eq!(order(&s), [1, 2]);
    assert_eq!(s.scan_found[0].unwrap().lqi, 200);
}

#[test]
fn a_bulb_waiting_for_a_key_outside_a_join_goes_down_when_the_wait_is_over() {
    let mut s = stack();
    s.state = State::WaitKey;
    s.steering = 0;
    s.timer = 1_001;
    s.timers(1_000);
    assert_eq!(s.state, State::WaitKey);
    s.timers(1_001);
    assert_eq!(s.state, State::Down);
}

#[test]
fn a_rejoin_response_without_a_rejoin_is_ignored() {
    let mut s = stack();
    s.state = State::Up;
    s.nib.short = 0x0042;
    let h = crate::nwk::Header::new(crate::nwk::FRAME_CMD, 0x0042, 0x0000, 1, 1);
    s.on_rejoin_response(&h, 0x0000, &[crate::nwk::CMD_REJOIN_RESPONSE, 0x34, 0x12, 0x00]);
    assert_eq!(s.nib.short, 0x0042);
}

#[test]
fn beacons_are_taken_only_while_scanning_and_when_searching_only_those_of_the_network() {
    let beacon = |s: &mut Stack<Scripted>, ext_pan: u64| {
        let mut c = candidate(1, 200, true, 1).beacon;
        c.ext_pan = ext_pan;
        let mut f = crate::util::Buf::new();
        c.write(7, &mut f);
        let (h, off) = mac::Header::parse(f.as_slice()).unwrap();
        s.on_beacon(&h, &f.as_slice()[off..], 200);
    };
    let mut s = stack();
    s.state = State::Scan;
    beacon(&mut s, 1);
    assert_eq!(order(&s), [1]);
    let mut s = stack();
    s.state = State::AssocPoll;
    beacon(&mut s, 1);
    assert!(order(&s).is_empty(), "not while associating");
    let mut s = stack();
    s.state = State::Scan;
    s.searching = Some(0);
    s.nib.ext_pan = 9;
    beacon(&mut s, 1);
    assert!(order(&s).is_empty(), "another network");
    beacon(&mut s, 9);
    assert_eq!(order(&s), [1]);
    let mut s = stack();
    s.state = State::Scan;
    let h = mac::Header { frame_type: mac::FRAME_BEACON, security: false, pending: false, ack_request: false, seq: 1, dst_pan: 0, dst: Addr::None, src_pan: 0x1A62, src: Addr::Short(1) };
    s.on_beacon(&h, &[0x00], 200);
    assert!(order(&s).is_empty(), "not a beacon");
}

#[test]
fn an_association_request_that_is_not_sent_moves_on_to_the_next_candidate() {
    let mut s = stack();
    s.state = State::Scan;
    s.scan_found[0] = Some(candidate(0x0001, 200, true, 1));
    s.scan_found[1] = Some(candidate(0x0002, 200, true, 1));
    s.hal.deaf = std::vec![0x0001];
    s.join_next(1_000);
    let to: Vec<u16> = s.hal.sent.iter().map(|f| u16::from_le_bytes([f[5], f[6]])).collect();
    assert_eq!(to, [0x0001, 0x0002]);
    assert_eq!(s.state, State::AssocPoll);
}

#[test]
fn mac_commands_of_a_bulb_that_is_up_answer_only_its_own_address_and_its_children() {
    let mut s = stack();
    s.state = State::Up;
    s.nib.short = 0x0042;
    s.nib.pan = 0x1A62;
    s.allow_children = true;
    s.permit_until = Some(100_000);
    mac_cmd(&mut s, Addr::Short(0x0042), 0x1A62, Addr::Short(0x0005), &[mac::CMD_ASSOC_REQ, 0x8E]);
    mac_cmd(&mut s, Addr::Short(0x0042), 0x1A62, Addr::Ext(0x77), &[mac::CMD_ASSOC_REQ]);
    mac_cmd(&mut s, Addr::Short(0x0043), 0x1A62, Addr::Ext(0x77), &[mac::CMD_ASSOC_REQ, 0x8E]);
    assert!(s.children.iter().all(|c| !c.used));
    mac_cmd(&mut s, Addr::Short(0xFFFF), 0xFFFF, Addr::Short(0x0005), &[mac::CMD_ORPHAN]);
    mac_cmd(&mut s, Addr::Short(0xFFFF), 0xFFFF, Addr::Ext(0x77), &[mac::CMD_ORPHAN]);
    assert!(s.hal.sent.is_empty(), "an orphan that is no child is not realigned");
}

#[test]
fn a_bulb_resumed_with_its_trust_center_key_requested_but_not_confirmed_asks_to_verify_it() {
    for (state, step) in [(0u8, 1u8), (1, 5), (2, 0), (3, 0)] {
        let mut s = stack();
        s.nib.tclk_state = state;
        s.resume(1_000);
        assert_eq!(s.tclk_step, step, "{state}");
    }
}

#[test]
fn outside_a_rejoin_the_depth_of_a_router_does_not_rank_it() {
    let mut s = stack();
    s.candidate_add(candidate(1, 250, true, 4));
    s.candidate_add(candidate(2, 200, true, 1));
    assert_eq!(order(&s), [1, 2]);
}

#[test]
fn a_bulb_that_goes_down_tries_again_after_up_to_two_more_seconds() {
    let mut s = stack();
    s.hal.random = std::vec![1_999];
    s.down(1_000);
    assert_eq!((s.state, s.timer), (State::Down, 1_000 + super::RETRY_MS + 1_999));
}

#[test]
fn association_answers_requests_and_polls_count_only_in_the_state_they_belong_to() {
    let mut s = stack();
    s.state = State::Up;
    s.nib.short = 0x0042;
    s.nib.pan = 0x1A62;
    let me = s.ext;
    mac_cmd(&mut s, Addr::Ext(me), 0x1A62, Addr::Ext(0x00124B00_0000_0001), &[mac::CMD_ASSOC_RSP, 0x34, 0x12, 0x00]);
    assert_eq!((s.state, s.nib.short), (State::Up, 0x0042), "an answer nobody waits for");
    for state in [State::Scan, State::WaitKey] {
        let mut s = stack();
        s.state = state;
        s.nib.short = 0x0042;
        s.nib.pan = 0x1A62;
        s.allow_children = true;
        s.permit_until = Some(100_000);
        mac_cmd(&mut s, Addr::Short(0x0042), 0x1A62, Addr::Ext(0x77), &[mac::CMD_ASSOC_REQ, 0x8E]);
        mac_cmd(&mut s, Addr::Short(0x0042), 0x1A62, Addr::Short(0x1234), &[mac::CMD_DATA_REQ]);
        assert!(s.children.iter().all(|c| !c.used), "{state:?}");
        assert!(s.hal.sent.is_empty(), "{state:?}");
    }
}

#[test]
fn a_network_up_without_a_trust_center_opens_after_a_join_only() {
    for (steering, after) in [(super::STEERING_JOINING, super::STEERING_OPEN_DUE), (0, 0)] {
        let mut s = stack();
        s.nib.tc_ext = crate::stack::NO_TRUST_CENTER;
        s.steering = steering;
        s.network_up(1_000);
        assert_eq!(s.steering, after, "{steering}");
    }
}

#[test]
fn an_association_through_a_router_makes_it_the_neighbour_and_a_request_for_another_address_is_not_taken() {
    let mut s = stack();
    s.state = State::AssocWait;
    s.nib.parent = 0x1234;
    let me = s.ext;
    mac_cmd(&mut s, Addr::Ext(me), 0x1A62, Addr::Ext(0x00124B00_0000_1234), &[mac::CMD_ASSOC_RSP, 0x78, 0x56, 0x00]);
    assert_eq!((s.neighbors[0].short, s.neighbors[0].used), (0x1234, true));
    let mut s = stack();
    s.state = State::Up;
    s.nib.short = 0x0042;
    s.nib.pan = 0x1A62;
    s.allow_children = true;
    s.permit_until = Some(100_000);
    mac_cmd(&mut s, Addr::Short(0x0043), 0x1A62, Addr::Ext(0x77), &[mac::CMD_ASSOC_REQ, 0x8E]);
    mac_cmd(&mut s, Addr::Short(0x0042), 0x1B00, Addr::Ext(0x77), &[mac::CMD_ASSOC_REQ, 0x8E]);
    assert!(s.children.iter().all(|c| !c.used));
    mac_cmd(&mut s, Addr::Short(0x0042), 0x1A62, Addr::Ext(0x77), &[mac::CMD_ASSOC_REQ, 0x8E]);
    assert!(s.children[0].used, "its own address and PAN");
}

#[test]
fn a_resumed_bulb_announces_its_children_only_when_it_has_end_devices() {
    let mut s = stack();
    s.resume(1_000);
    assert_eq!(s.parent_annce_at, None);
    s.children[3].used = true;
    s.children[3].capability = 0x8E;
    s.resume(1_000);
    assert_eq!(s.parent_annce_at, None, "a router child");
    s.children[4].used = true;
    s.children[4].capability = 0x80;
    s.resume(1_000);
    assert!(s.parent_annce_at.is_some());
}
