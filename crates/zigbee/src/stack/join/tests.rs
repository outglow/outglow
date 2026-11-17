use super::super::address_tests::{Scripted, IDENTITY};
use super::Candidate;
use crate::mac::Beacon;
use crate::stack::Stack;
extern crate std;
use std::vec::Vec;

fn stack() -> Stack<Scripted> {
    Stack::new(Scripted { random: Vec::new(), now: 1_000, sent: Vec::new() }, IDENTITY)
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
