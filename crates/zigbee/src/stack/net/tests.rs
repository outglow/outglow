use super::super::address_tests::{Scripted, IDENTITY};
use super::super::diag::k;
use super::NEIGHBOR_FORGET;
use crate::hal::trace;
use crate::nwk;
use crate::stack::{Stack, State, AGE_LIMIT, MAX_NWK_PAYLOAD, NEIGHBORS, ROUTES};
extern crate std;
use std::vec::Vec;

fn stack() -> Stack<Scripted> {
    Stack::new(Scripted { random: Vec::new(), now: 1_000, sent: Vec::new(), deaf: Vec::new(), busy: false, attempts: 0, bx: None }, IDENTITY)
}

fn fill(s: &mut Stack<Scripted>) {
    for (i, n) in s.neighbors.iter_mut().enumerate() {
        n.used = true;
        n.short = 0x1000 + i as u16;
        n.ext = 0x00124B00_0000_1000 + i as u64;
        n.age = 1;
    }
}

#[test]
fn a_new_neighbour_in_a_full_table_takes_the_place_of_the_first_of_the_oldest() {
    let mut s = stack();
    fill(&mut s);
    s.neighbors[5].age = 9;
    s.neighbors[9].age = 9;
    s.neighbors[12].age = 4;
    s.neighbor_heard(0x2222, 0x00124B00_0000_2222, 200, 7, 0);
    assert_eq!((s.neighbors[5].short, s.neighbors[5].ext), (0x2222, 0x00124B00_0000_2222));
    assert_eq!(s.neighbors.iter().filter(|n| n.short == 0x1009 || n.short == 0x100C).count(), 2);
    assert_eq!(s.diag.n[k::NEIGHBOR_REMOVED], 1);
}

#[test]
fn a_new_neighbour_starts_from_what_was_heard() {
    let mut s = stack();
    s.rx_signal = (200, 3);
    s.neighbor_heard(0x2222, 0x00124B00_0000_2222, 200, 7, 2);
    let n = s.neighbors[0];
    assert!(n.used);
    assert_eq!((n.lqi, n.rssi_avg, n.rssi_min, n.fc_next, n.fc_seq, n.rx), (200, 3, 3, 8, 2, 1));
    assert_eq!((n.out_cost, n.age, n.confirmed), (0, AGE_LIMIT, false), "a router not yet heard to hear us");
    s.rx_signal = (100, -61);
    s.neighbor_heard(0x2222, 0x00124B00_0000_2222, 100, 8, 2);
    let n = s.neighbors[0];
    assert_eq!((n.lqi, n.rssi_avg, n.rssi_min, n.rx), (175, -13, -61, 2), "a quarter of the new value to three quarters of the old");
}

#[test]
fn a_joined_router_child_is_a_confirmed_neighbour_with_its_link_cost() {
    let mut s = stack();
    let c = &mut s.children[0];
    c.used = true;
    c.joined = true;
    c.short = 0x2222;
    c.ext = 0x00124B00_0000_2222;
    c.capability = super::CAP_FFD;
    s.neighbor_heard(0x2222, 0x00124B00_0000_2222, 255, 7, 0);
    let n = s.neighbors[0];
    assert_eq!((n.out_cost, n.age, n.confirmed), (1, 0, true));
}

#[test]
fn a_neighbour_goes_stale_after_the_age_limit_and_is_forgotten_after_forty_link_status_periods() {
    let mut s = stack();
    fill(&mut s);
    for n in s.neighbors.iter_mut() {
        n.age = AGE_LIMIT - 1;
        n.out_cost = 3;
        n.confirmed = true;
    }
    s.neighbors[1].confirmed = false;
    s.age_neighbors();
    assert_eq!((s.neighbors[0].age, s.neighbors[0].out_cost), (AGE_LIMIT, 3));
    assert_eq!(s.diag.n[k::NEIGHBOR_STALE], 0);
    s.age_neighbors();
    assert_eq!((s.neighbors[0].age, s.neighbors[0].out_cost, s.neighbors[0].stales), (AGE_LIMIT + 1, 0, 1));
    assert_eq!(s.diag.n[k::NEIGHBOR_STALE], NEIGHBORS as u32 - 1, "only confirmed ones go stale");
    s.age_neighbors();
    assert_eq!(s.diag.n[k::NEIGHBOR_STALE], NEIGHBORS as u32 - 1, "once");
    for n in s.neighbors.iter_mut() {
        n.age = NEIGHBOR_FORGET - 1;
    }
    s.neighbors[2].age = NEIGHBOR_FORGET - 2;
    s.age_neighbors();
    assert!(s.neighbors.iter().all(|n| n.used));
    s.age_neighbors();
    assert_eq!(s.neighbors.iter().filter(|n| n.used).count(), 1);
    assert!(s.neighbors[2].used);
    assert_eq!(s.diag.n[k::NEIGHBOR_REMOVED], NEIGHBORS as u32 - 1);
}

fn announce(short: u16) -> [u8; 12] {
    let s = short.to_le_bytes();
    [0x08, 0x00, 0x13, 0x00, 0x00, 0x00, 0x01, 0x80, s[0], s[1], 0x42, 0x8E]
}

#[test]
fn a_device_announce_straight_from_a_neighbour_brings_the_link_status_forward() {
    let cases: [(u8, u16, u16, usize, bool, bool); 7] = [
        (nwk::FRAME_DATA, 0x1000, 0x1000, 12, true, true),
        (nwk::FRAME_DATA, 0x1000, 0x1000, 9, true, true),
        (nwk::FRAME_DATA, 0x1000, 0x1000, 8, true, false),
        (nwk::FRAME_CMD, 0x1000, 0x1000, 12, true, false),
        (nwk::FRAME_DATA, 0x1001, 0x1000, 12, true, false),
        (nwk::FRAME_DATA, 0x1000, 0x1000, 12, false, false),
        (nwk::FRAME_DATA, 0x2222, 0x2222, 12, false, false),
    ];
    for (i, (frame_type, src, mac_src, len, announce_frame, soon)) in cases.into_iter().enumerate() {
        let mut s = stack();
        fill(&mut s);
        s.link_status_at = 50_000;
        let mut p = announce(src);
        if !announce_frame {
            p[2] = 0x14;
        }
        s.neighbor_announced(&nwk::Header::new(frame_type, 0xFFFD, src, 30, 1), mac_src, &p[..len]);
        assert_eq!(s.link_status_at, if soon { 4_000 } else { 50_000 }, "case {i}");
    }
}

#[test]
fn a_link_status_lists_only_as_many_entries_as_its_count_says() {
    let mut s = stack();
    fill(&mut s);
    s.nib.short = 0x0042;
    s.neighbors[0].out_cost = 0;
    s.on_link_status(0x1000, &[nwk::CMD_LINK_STATUS, 0x61, 0x43, 0x00, 0x11, 0x42, 0x00, 0x33]);
    assert_eq!(s.neighbors[0].out_cost, 0, "the second entry is past the count");
    s.on_link_status(0x1000, &[nwk::CMD_LINK_STATUS, 0x62, 0x43, 0x00, 0x11, 0x42, 0x00, 0x33]);
    assert_eq!(s.neighbors[0].out_cost, 3);
}

fn up() -> Stack<Scripted> {
    let mut s = stack();
    s.state = State::Up;
    s.nib.short = 0x0042;
    s.ext = 0x00124B00_0000_0042;
    let c = &mut s.children[0];
    c.used = true;
    c.short = 0x2000;
    c.ext = 0xA;
    for (i, (short, ext, used)) in [(0x1000, 0xB, true), (0x1001, 0, true), (0x1002, 0xC, false)].into_iter().enumerate() {
        let n = &mut s.neighbors[i];
        n.used = used;
        n.short = short;
        n.ext = ext;
    }
    s.addr_map[0] = (0xD, 0x3000);
    s.addr_map[1] = (0, 0x3001);
    s.addr_map[2] = (0xE, 0xFFF8);
    s.addr_map[3] = (0xF, 0xFFF7);
    s
}

#[test]
fn an_address_seen_with_another_extended_address_than_the_one_known_for_it_is_a_conflict() {
    let cases: [(u16, u64, bool); 17] = [
        (0x0042, 0x00124B00_0000_0042, false),
        (0x0042, 0x99, true),
        (0x0042, 0, false),
        (0x0042, u64::MAX, false),
        (0x2000, 0xA, false),
        (0x2000, 0x99, true),
        (0x1000, 0xB, false),
        (0x1000, 0x99, true),
        (0x1001, 0x99, false),
        (0x1002, 0x99, false),
        (0x3000, 0xD, false),
        (0x3000, 0x99, true),
        (0x3001, 0x99, false),
        (0xFFF8, 0x99, false),
        (0xFFF7, 0x99, true),
        (0xFFF7, 0xF, false),
        (0x5555, 0x99, false),
    ];
    for (short, ext, clash) in cases {
        let mut s = up();
        assert_eq!(s.address_seen(short, ext), clash, "{short:04X} {ext:X}");
        assert_eq!(s.conflict.0 == short, clash, "{short:04X} {ext:X}");
    }
    let mut s = up();
    s.state = State::WaitKey;
    assert!(!s.address_seen(0x0042, 0x99), "not before the bulb is up");
}

#[test]
fn a_conflicting_address_of_a_neighbour_is_forgotten_everywhere() {
    let mut s = up();
    s.neighbors[3].used = true;
    s.neighbors[3].short = 0x1000;
    s.addr_map[1] = (0x77, 0x1000);
    s.routes[0].status = super::ROUTE_ACTIVE;
    s.routes[0].dst = 0x1000;
    s.address_conflict_resolve(0x1000);
    assert!(!s.neighbors[0].used && !s.neighbors[3].used && s.neighbors[1].used);
    assert_eq!(s.diag.n[k::NEIGHBOR_REMOVED], 2);
    assert_eq!((s.addr_map[1], s.addr_map[0]), ((0x77, 0xFFFF), (0xD, 0x3000)));
    assert_eq!(s.routes[0].status, super::ROUTE_FREE);
}

#[test]
fn a_network_status_about_a_conflict_is_taken_only_with_its_address_and_forgets_it() {
    let mut s = up();
    let h = nwk::Header::new(nwk::FRAME_CMD, 0xFFFD, 0x1001, 30, 1);
    s.on_nwk_command(&h, 0x1001, &[nwk::CMD_NETWORK_STATUS, nwk::STATUS_ADDRESS_CONFLICT, 0x00]);
    assert!(s.neighbors[0].used && s.diag.status_rx == 0, "too short");
    s.on_nwk_command(&h, 0x1001, &[nwk::CMD_NETWORK_STATUS, nwk::STATUS_ADDRESS_CONFLICT, 0x00, 0x10]);
    assert!(!s.neighbors[0].used);
    assert_eq!(s.diag.status_rx, 1);
    assert_eq!(s.conflict.0, 0x1000);
}

#[test]
fn an_unknown_nwk_command_is_answered_only_when_addressed_to_the_bulb() {
    for (dst, id, answered) in [(0x0042, 0x0D, true), (0x0042, 0x0C, false), (0x0042, 0x01, false), (0xFFFD, 0x0D, false)] {
        let mut s = up();
        let h = nwk::Header::new(nwk::FRAME_CMD, dst, 0x1000, 30, 1);
        s.on_nwk_command(&h, 0x1000, &[id]);
        assert_eq!(!s.hal.sent.is_empty(), answered, "{dst:04X} {id:02X}");
    }
}

fn route(s: &mut Stack<Scripted>, i: usize, dst: u16, next: u16, many_to_one: bool, at: u32) {
    s.routes[i] = super::Route { status: super::ROUTE_ACTIVE, dst, next, many_to_one, no_cache: false, record_due: false, at };
}

#[test]
fn a_full_route_table_gives_up_the_last_of_the_oldest_routes_with_many_to_one_ones_aging_sixteen_times_slower() {
    let mut s = stack();
    for i in 0..ROUTES {
        route(&mut s, i, 0x5000 + i as u16, 0x1000, false, 99_990);
    }
    assert_eq!(s.route_set(0x6000, 0x1000, super::ROUTE_ACTIVE, false, false, 100_000), ROUTES - 1);
    for i in 0..ROUTES {
        route(&mut s, i, 0x5000 + i as u16, 0x1000, false, 99_990);
    }
    route(&mut s, 5, 0x5005, 0x1000, true, 99_680);
    route(&mut s, 6, 0x5006, 0x1000, true, 99_000);
    assert_eq!(s.route_set(0x6000, 0x1001, super::ROUTE_ACTIVE, false, false, 100_000), 6);
    assert_eq!((s.routes[6].dst, s.routes[6].next, s.routes[6].at, s.routes[6].many_to_one), (0x6000, 0x1001, 100_000, false));
    route(&mut s, 6, 0x5006, 0x1000, true, 99_000);
    route(&mut s, 7, 0x5007, 0x1000, false, 99_900);
    assert_eq!(s.route_set(0x6001, 0x1001, super::ROUTE_ACTIVE, false, false, 100_000), 7);
}

#[test]
fn a_many_to_one_route_learned_through_another_hop_wants_a_route_record() {
    for (many_to_one, next, due) in [(true, 0x1001, true), (true, 0x1000, false), (false, 0x1001, false)] {
        let mut s = stack();
        route(&mut s, 0, 0x0000, 0x1000, many_to_one, 0);
        s.route_learned(0x0000, next, 500);
        assert_eq!((s.routes[0].record_due, s.routes[0].next, s.routes[0].at), (due, next, 500));
    }
    let mut s = stack();
    route(&mut s, 0, 0x0000, 0x1000, true, 0);
    s.routes[0].status = super::ROUTE_DISCOVERY;
    s.route_learned(0x0000, 0x1001, 500);
    assert_eq!((s.routes[0].status, s.routes[0].many_to_one, s.routes[0].record_due), (super::ROUTE_ACTIVE, false, false));
}

fn via_table(s: &mut Stack<Scripted>) {
    route(s, 0, 0x5000, 0x1000, true, 0);
    route(s, 1, 0x5001, 0x1000, false, 0);
    route(s, 2, 0x5002, 0x1001, false, 0);
    route(s, 3, 0x5003, 0x1000, false, 0);
    s.routes[3].status = super::ROUTE_DISCOVERY;
}

fn statuses(s: &Stack<Scripted>) -> [u8; 4] {
    [s.routes[0].status, s.routes[1].status, s.routes[2].status, s.routes[3].status]
}

#[test]
fn routes_through_a_lost_hop_are_dropped_and_many_to_one_ones_only_when_the_hop_is_gone() {
    let (a, f, d) = (super::ROUTE_ACTIVE, super::ROUTE_FREE, super::ROUTE_DISCOVERY);
    let mut s = stack();
    fill(&mut s);
    via_table(&mut s);
    s.drop_routes_via(0x1000);
    assert_eq!(statuses(&s), [a, f, a, d]);
    let mut s = stack();
    via_table(&mut s);
    s.drop_routes_via(0x1000);
    assert_eq!(statuses(&s), [f, f, a, d]);
}

#[test]
fn a_failed_frame_drops_its_own_route_only_while_the_hop_is_a_good_neighbour() {
    let (a, f, d) = (super::ROUTE_ACTIVE, super::ROUTE_FREE, super::ROUTE_DISCOVERY);
    let cases: [(u16, u16, [u8; 4]); 5] = [
        (0x1000, 0x5001, [a, f, a, d]),
        (0x1000, 0x5000, [a, a, a, d]),
        (0x1000, 0x5002, [a, a, a, d]),
        (0x1000, 0x5003, [a, a, a, d]),
        (0x1001, 0x5002, [a, a, f, d]),
    ];
    for (next, dst, after) in cases {
        let mut s = stack();
        fill(&mut s);
        via_table(&mut s);
        s.link_lost(next, dst);
        assert_eq!(statuses(&s), after, "{next:04X} {dst:04X}");
    }
    let mut s = stack();
    fill(&mut s);
    via_table(&mut s);
    s.neighbors[0].failures = super::LINK_FAILURES;
    s.link_lost(0x1000, 0x5002);
    assert_eq!(statuses(&s), [a, f, a, d], "a failed hop loses all its routes but many-to-one ones");
}

#[test]
fn another_router_is_one_that_is_live_and_neither_of_the_two_to_avoid() {
    let mut s = stack();
    fill(&mut s);
    for n in s.neighbors.iter_mut() {
        n.out_cost = 1;
    }
    for n in s.neighbors[4..].iter_mut() {
        n.used = false;
    }
    s.neighbors[2].failures = super::LINK_FAILURES;
    s.hal.random = [1, 0].to_vec();
    assert_eq!(s.random_router(0x1000, 0x1001), Some(0x1003));
    assert_eq!(s.random_router(0x1003, 0x1001), Some(0x1000));
    s.neighbors[3].age = AGE_LIMIT + 1;
    s.neighbors[0].out_cost = 0;
    assert_eq!(s.random_router(0x1002, 0x1001), None);
}

fn data_from(src: u16) -> nwk::Header {
    nwk::Header::new(nwk::FRAME_DATA, 0x5000, src, 30, 1)
}

#[test]
fn a_frame_for_a_retry_or_a_route_is_held_only_up_to_the_largest_nwk_payload() {
    let mut s = up();
    s.hal.random = [super::NWK_RETRY_SPREAD_MS].to_vec();
    assert!(s.hold_retry(&data_from(0x1000), &[0x55; MAX_NWK_PAYLOAD], 0x1000, 2));
    let e = s.held[0];
    assert_eq!((e.used, e.len, e.left, e.next, e.at), (true, MAX_NWK_PAYLOAD as u8, 2, 0x1000, 1_000 + super::NWK_RETRY_MIN_MS + super::NWK_RETRY_SPREAD_MS));
    assert!(!s.hold_retry(&data_from(0x1000), &[0x55; MAX_NWK_PAYLOAD + 1], 0x1000, 2));
    assert!(!s.held[1].used);
    s.hold(&data_from(0x0042), &[0x66; MAX_NWK_PAYLOAD]);
    assert_eq!((s.held[1].used, s.held[1].len, s.held[1].at), (true, MAX_NWK_PAYLOAD as u8, 1_000 + super::HOLD_MS));
    s.hold(&data_from(0x0042), &[0x66; MAX_NWK_PAYLOAD + 1]);
    assert!(!s.held[2].used);
}

#[test]
fn a_full_hold_gives_up_the_frame_waiting_for_a_route_that_expires_first_never_one_waiting_for_a_retry() {
    let mut s = up();
    for (i, e) in s.held.iter_mut().enumerate() {
        e.used = true;
        e.header = data_from(0x0042);
        e.header.dst = 0x5000 + i as u16;
        e.at = 3_000 + i as u32;
        e.left = 0;
    }
    s.held[0].left = 1;
    s.held[0].at = 1_500;
    s.hold(&data_from(0x0042), &[1]);
    assert_eq!(s.held[1].header.dst, 0x5000, "the second, the first waiting for a route");
    assert_eq!(s.held[0].header.dst, 0x5000);
    assert_eq!(s.diag.n[k::BUFFER_ALLOCATE_FAILURES], 1);
    for e in s.held.iter_mut() {
        e.left = 1;
    }
    s.hold(&data_from(0x0042), &[2]);
    assert_eq!(s.diag.n[k::BUFFER_ALLOCATE_FAILURES], 2, "no room");
    assert!(s.held.iter().all(|e| e.len != 1 || e.payload[0] != 2));
}

#[test]
fn a_held_frame_that_failed_is_counted_by_whose_it_was_and_why() {
    let b = k::BUFFER_ALLOCATE_FAILURES;
    let (no_route, full, aps) = (k::RELAY_DROP_NO_ROUTE, k::RELAY_DROP_QUEUE_FULL, k::APS_TX_UCAST_FAIL);
    let cases: [(u16, u8, u8, [u32; 4], bool); 6] = [
        (0x1000, nwk::FRAME_DATA, trace::FAIL_EXPIRED, [0, 1, 0, 0], true),
        (0x1000, nwk::FRAME_DATA, trace::FAIL_EVICTED, [1, 0, 1, 0], true),
        (0x1000, nwk::FRAME_CMD, trace::FAIL_NO_ROOM, [1, 0, 1, 0], false),
        (0x0042, nwk::FRAME_DATA, trace::FAIL_EXPIRED, [0, 0, 0, 1], false),
        (0x0042, nwk::FRAME_DATA, trace::FAIL_NO_ROOM, [1, 0, 0, 1], false),
        (0x0042, nwk::FRAME_CMD, trace::FAIL_EXPIRED, [0, 0, 0, 0], false),
    ];
    for (src, frame_type, why, counts, status) in cases {
        let mut s = up();
        fill(&mut s);
        let mut h = data_from(src);
        h.frame_type = frame_type;
        s.held_failed(&h, why);
        assert_eq!([s.diag.n[b], s.diag.n[no_route], s.diag.n[full], s.diag.n[aps]], counts, "{src:04X} {frame_type} {why}");
        assert_eq!(!s.hal.sent.is_empty(), status, "{src:04X} {frame_type} {why}");
    }
}

#[test]
fn a_network_status_is_never_sent_to_the_bulb_itself_or_to_a_broadcast_address() {
    for (to, sent) in [(0x1000, true), (0x0042, false), (0xFFFD, false)] {
        let mut s = up();
        fill(&mut s);
        s.neighbors[0].short = 0x0042;
        s.neighbors[1].short = 0xFFFD;
        s.neighbors[2].short = 0x1000;
        s.send_network_status(to, nwk::STATUS_NO_ROUTE, 0x5000);
        assert_eq!(!s.hal.sent.is_empty(), sent, "{to:04X}");
    }
}

#[test]
fn a_discovery_takes_the_first_free_slot_and_when_forced_the_live_one_that_ends_soonest() {
    let mut s = stack();
    for (i, d) in s.discoveries.iter_mut().enumerate() {
        d.used = true;
        d.expires = 5_000 + 100 * (i as u32 % 3);
    }
    assert_eq!(s.discovery_slot(1_000, false), None);
    assert_eq!(s.discovery_slot(1_000, true), Some(0));
    s.discoveries[0].expires = 5_100;
    assert_eq!(s.discovery_slot(1_000, true), Some(3));
    s.discoveries[2].expires = 900;
    s.discoveries[4].used = false;
    assert_eq!(s.discovery_slot(1_000, false), Some(2));
    assert_eq!(s.discovery_slot(1_000, true), Some(2));
}

#[test]
fn a_late_copy_of_the_bulbs_own_frame_is_one_with_its_extended_address_or_one_of_its_last_64_numbers() {
    let mut s = up();
    s.nwk_seq = 100;
    let h = |seq: u8, ext: Option<u64>| {
        let mut h = nwk::Header::new(nwk::FRAME_DATA, 0xFFFD, 0x0042, 30, seq);
        h.src_ext = ext;
        h
    };
    assert!(s.own_late_copy(&h(100, None)));
    assert!(s.own_late_copy(&h(37, None)));
    assert!(!s.own_late_copy(&h(36, None)));
    assert!(!s.own_late_copy(&h(101, None)));
    assert!(s.own_late_copy(&h(101, Some(0x00124B00_0000_0042))));
    assert!(!s.own_late_copy(&h(101, Some(0x99))));
}

#[test]
fn a_broadcast_heard_again_from_a_neighbour_marks_only_its_own_copies() {
    let mut s = up();
    fill(&mut s);
    for (i, (used, src, seq)) in [(true, 0x2000, 5), (true, 0x2000, 6), (true, 0x2001, 5), (false, 0x2000, 5)].into_iter().enumerate() {
        s.bcast[i].used = used;
        s.bcast[i].header = nwk::Header::new(nwk::FRAME_DATA, 0xFFFD, src, 30, seq);
    }
    s.broadcast_heard(0x2000, 5, 0x1003);
    assert_eq!([s.bcast[0].heard, s.bcast[1].heard, s.bcast[2].heard, s.bcast[3].heard], [1 << 3, 0, 0, 0]);
    s.broadcast_heard(0x2000, 5, 0x7777);
    assert_eq!(s.bcast[0].heard, 1 << 3, "not from a neighbour");
}

#[test]
fn a_full_broadcast_table_takes_a_new_broadcast_only_in_place_of_one_kept_at_most_seven_seconds_more() {
    let mut s = stack();
    let now = 10_000;
    for (i, e) in s.seen.iter_mut().enumerate() {
        *e = super::Seen { used: true, src: 0x3000 + i as u16, seq: 1, expires: now + super::BROADCAST_MEMORY_MS };
    }
    let keep = super::BROADCAST_MEMORY_MS - super::BTT_KEEP_MS;
    s.seen[2].expires = now + keep + 1;
    assert_eq!(s.seen_before(0x2000, 9, now, false), None);
    assert_eq!(s.seen_before(0x3002, 1, now, false), Some(true));
    s.seen[2].expires = now + keep;
    s.seen[4].expires = now + keep;
    assert_eq!(s.seen_before(0x2000, 9, now, false), Some(false));
    assert_eq!((s.seen[2].src, s.seen[2].seq, s.seen[2].expires), (0x2000, 9, now + super::BROADCAST_MEMORY_MS), "the first of the two");
    assert_eq!(s.seen[4].src, 0x3004);
    s.seen[4].expires = now + keep + 1;
    assert_eq!(s.seen_before(0x2001, 9, now, false), None);
    assert_eq!(s.seen_before(0x2001, 9, now, true), Some(false), "forced");
    assert_eq!(s.seen[4].src, 0x2001);
}

fn rreq(s: &mut Stack<Scripted>, i: usize, src: u16, id: u8) {
    let e = &mut s.bcast[i];
    e.used = true;
    e.header = nwk::Header::new(nwk::FRAME_CMD, 0xFFFC, src, 5, 1);
    e.payload[..6].copy_from_slice(&[nwk::CMD_ROUTE_REQUEST, 0, id, 0x55, 0x55, 0]);
    e.len = 6;
}

#[test]
fn a_route_request_passed_on_replaces_the_queued_copy_of_the_same_request_and_waits_a_jitter() {
    let mut s = up();
    fill(&mut s);
    s.neighbors[0].out_cost = 1;
    s.neighbors[0].lqi = 255;
    rreq(&mut s, 0, 0x2000, 7);
    rreq(&mut s, 1, 0x2001, 7);
    rreq(&mut s, 2, 0x2000, 8);
    s.hal.random = [130].to_vec();
    let h = nwk::Header::new(nwk::FRAME_CMD, 0xFFFC, 0x2000, 5, 2);
    s.on_route_request(&h, 0x1000, 255, &[nwk::CMD_ROUTE_REQUEST, 0, 7, 0x55, 0x55, 3], 1_000);
    let e = s.bcast[0];
    assert!(e.used && s.bcast[1].used && s.bcast[2].used && !s.bcast[3].used);
    assert_eq!((e.header.seq, e.header.radius, e.at, e.left), (2, 4, 1_010, 1 + super::RREQ_RELAY_RETRIES));
    assert_eq!(e.payload[..6], [nwk::CMD_ROUTE_REQUEST, 0, 7, 0x55, 0x55, 3 + s.neighbors[0].link_cost()]);
    let mut s = up();
    fill(&mut s);
    s.neighbors[0].out_cost = 1;
    let h = nwk::Header::new(nwk::FRAME_CMD, 0xFFFC, 0x2000, 1, 2);
    s.on_route_request(&h, 0x1000, 255, &[nwk::CMD_ROUTE_REQUEST, 0, 7, 0x55, 0x55, 3], 1_000);
    assert!(s.bcast.iter().all(|e| !e.used), "at the end of its radius");
    assert!(s.discoveries[0].used);
}

#[test]
fn a_broadcast_is_queued_only_up_to_the_largest_nwk_payload() {
    let mut s = up();
    let h = nwk::Header::new(nwk::FRAME_DATA, 0xFFFD, 0x2000, 5, 2);
    s.queue_broadcast(&h, &[1; MAX_NWK_PAYLOAD], 2_000, 3);
    assert_eq!((s.bcast[0].used, s.bcast[0].len, s.bcast[0].at, s.bcast[0].left), (true, MAX_NWK_PAYLOAD as u8, 2_000, 3));
    s.queue_broadcast(&h, &[1; MAX_NWK_PAYLOAD + 1], 2_000, 3);
    assert!(!s.bcast[1].used);
}

#[test]
fn a_route_reply_to_the_bulbs_own_request_stops_its_rebroadcasts() {
    let mut s = up();
    fill(&mut s);
    rreq(&mut s, 0, 0x0042, 7);
    rreq(&mut s, 1, 0x0042, 8);
    rreq(&mut s, 2, 0x2000, 7);
    s.bcast[3] = s.bcast[0];
    s.bcast[3].payload[0] = nwk::CMD_ROUTE_REPLY;
    s.discoveries[0] = super::Discovery { used: true, id: 7, src: 0x0042, sender: 0x0042, forward_cost: 0, residual_cost: 0xFF, expires: 9_000 };
    s.routes[0] = super::Route { status: super::ROUTE_DISCOVERY, dst: 0x5555, next: 0xFFFF, many_to_one: false, no_cache: false, record_due: false, at: 0 };
    let h = nwk::Header::new(nwk::FRAME_CMD, 0x0042, 0x1000, 5, 2);
    s.on_route_reply(&h, 0x1000, &[nwk::CMD_ROUTE_REPLY, 0, 7, 0x42, 0x00, 0x55, 0x55, 2]);
    assert_eq!([s.bcast[0].used, s.bcast[1].used, s.bcast[2].used, s.bcast[3].used], [false, true, true, true]);
    assert_eq!((s.routes[0].status, s.routes[0].next, s.discoveries[0].residual_cost), (super::ROUTE_ACTIVE, 0x1000, 2));
}

fn relaying() -> Stack<Scripted> {
    let mut s = up();
    fill(&mut s);
    for n in s.neighbors.iter_mut() {
        n.out_cost = 1;
    }
    s.children[0].capability = super::super::parent::CAP_RX_ON;
    s.children[0].joined = true;
    s
}

fn relay_status(s: &mut Stack<Scripted>, dst: u16, status: u8) {
    let h = nwk::Header::new(nwk::FRAME_CMD, dst, 0x3000, 5, 1);
    s.relay(h, 0x1000, &[nwk::CMD_NETWORK_STATUS, status, 0x00, 0x50]);
}

fn relay_counts(s: &Stack<Scripted>) -> [u32; 3] {
    [s.diag.n[k::RELAYED_UCAST], s.diag.n[k::RELAY_DROP_MAC_FAIL], s.diag.n[k::RELAY_DROP_NO_ROUTE]]
}

#[test]
fn a_network_status_passed_on_takes_its_route_or_another_router_and_a_link_failure_for_a_child_drops_the_route_it_tells_of() {
    let mut s = relaying();
    route(&mut s, 0, 0x5000, 0x1001, false, 0);
    relay_status(&mut s, 0x2000, nwk::STATUS_LINK_FAILURE);
    assert_eq!((relay_counts(&s), s.routes[0].status), ([1, 0, 0], super::ROUTE_FREE));
    let mut s = relaying();
    route(&mut s, 0, 0x5000, 0x1001, false, 0);
    relay_status(&mut s, 0x2000, nwk::STATUS_NO_ROUTE);
    assert_eq!(s.routes[0].status, super::ROUTE_ACTIVE, "not a link failure");
    relay_status(&mut s, 0x1002, nwk::STATUS_LINK_FAILURE);
    assert_eq!(s.routes[0].status, super::ROUTE_ACTIVE, "not for a child");
    let mut s = relaying();
    s.hal.deaf = [0x1002].to_vec();
    relay_status(&mut s, 0x1002, nwk::STATUS_NO_ROUTE);
    assert_eq!(relay_counts(&s), [0, 1, 0]);
    assert_eq!(s.neighbors[2].failures, 1);
    let mut s = relaying();
    s.neighbors[2].failures = super::LINK_FAILURES;
    relay_status(&mut s, 0x1002, nwk::STATUS_NO_ROUTE);
    assert_eq!((relay_counts(&s), s.hal.sent.len()), ([0, 0, 1], 0), "a failed link is no route");
    let mut s = relaying();
    s.hal.random = [0].to_vec();
    relay_status(&mut s, 0x7777, nwk::STATUS_MANY_TO_ONE_FAILURE);
    assert_eq!((relay_counts(&s), s.hal.sent.len()), ([1, 0, 0], 1));
    assert_eq!(s.hal.sent[0][5..7], [0x01, 0x10], "the first live router but the one it came from");
    let mut s = relaying();
    s.hal.random = [0].to_vec();
    s.hal.deaf = [0x1001].to_vec();
    relay_status(&mut s, 0x7777, nwk::STATUS_MANY_TO_ONE_FAILURE);
    assert_eq!(relay_counts(&s), [0, 1, 0]);
    let mut s = relaying();
    for n in s.neighbors[1..].iter_mut() {
        n.out_cost = 0;
    }
    relay_status(&mut s, 0x7777, nwk::STATUS_MANY_TO_ONE_FAILURE);
    assert_eq!(relay_counts(&s), [0, 0, 1]);
    let mut s = relaying();
    s.hal.deaf = [0x1002].to_vec();
    for n in s.neighbors[1..].iter_mut() {
        n.out_cost = 0;
    }
    s.neighbors[2].out_cost = 1;
    let h = nwk::Header::new(nwk::FRAME_CMD, 0x1002, 0x3000, 5, 1);
    s.relay(h, 0x1000, &[nwk::CMD_NETWORK_STATUS, nwk::STATUS_MANY_TO_ONE_FAILURE, 0x00, 0x50]);
    assert_eq!(relay_counts(&s), [0, 1, 0], "the route failed at the MAC and no other router is live");
    let mut s = relaying();
    relay_status(&mut s, 0x7777, nwk::STATUS_NO_ROUTE);
    assert_eq!(relay_counts(&s), [0, 0, 1]);
}

#[test]
fn a_source_routed_frame_is_passed_on_only_by_the_relay_its_index_points_at() {
    for (count, index, relayed) in [(2u8, 1u8, true), (2, 2, false), (2, 0, false)] {
        let mut s = relaying();
        let mut list = [0u16; nwk::MAX_RELAYS];
        list[..3].copy_from_slice(&[0x1005, 0x0042, 0x0042]);
        let mut h = nwk::Header::new(nwk::FRAME_DATA, 0x1001, 0x0000, 5, 1);
        h.relays = Some((count, index, list));
        s.relay(h, 0x1000, &[0x00]);
        assert_eq!(!s.hal.sent.is_empty(), relayed, "{count} {index}");
    }
}

const SENDER: u64 = 0x00124B00_0000_1000;

fn secured(h: nwk::Header, ext: u64, counter: u32, key_seq: u8, key: &[u8; 16], payload: &[u8]) -> Vec<u8> {
    let mut h = h;
    h.security = true;
    let mut f = crate::util::Buf::new();
    h.write(&mut f);
    nwk::secure(&crate::aes::Aes128::new(key), key_seq, ext, counter, &mut f, 0, payload);
    f.as_slice().to_vec()
}

fn heard(s: &mut Stack<Scripted>, h: nwk::Header, counter: u32, payload: &[u8]) {
    let f = secured(h, SENDER, counter, 0, &[0; 16], payload);
    s.on_nwk(0x1000, &f, 200);
}

#[test]
fn a_frame_from_another_pan_is_not_taken() {
    for (pan, taken) in [(0x1234, true), (0x4321, false)] {
        let mut s = up();
        s.nib.pan = 0x1234;
        let mut f = crate::util::Buf::new();
        crate::mac::Header {
            frame_type: crate::mac::FRAME_DATA,
            security: false,
            pending: false,
            ack_request: true,
            seq: 1,
            dst_pan: pan,
            dst: crate::mac::Addr::Short(0x0042),
            src_pan: pan,
            src: crate::mac::Addr::Short(0x7001),
        }
        .write(&mut f);
        f.bytes(&secured(nwk::Header::new(nwk::FRAME_DATA, 0x0042, 0x7001, 5, 1), 0x77, 1, 0, &[0; 16], &[0x40]));
        s.on_mpdu(f.as_slice(), 200, 0);
        assert_eq!(s.neighbor_index(0x7001).is_some(), taken, "{pan:04X}");
    }
}

#[test]
fn a_frame_under_a_key_number_neither_current_nor_kept_is_one_with_an_unknown_key() {
    for (alt_valid, alt_seq, key_seq, unknown) in [(true, 1, 2, true), (false, 2, 2, true), (true, 2, 2, false)] {
        let mut s = up();
        s.nib.alt_valid = alt_valid;
        s.nib.alt_seq = alt_seq;
        let f = secured(nwk::Header::new(nwk::FRAME_DATA, 0x0042, 0x1000, 5, 1), SENDER, 1, key_seq, &[0; 16], &[0x40]);
        s.on_nwk(0x1000, &f, 200);
        assert_eq!(s.unknown_key_frames == 1, unknown, "{alt_valid} {alt_seq} {key_seq}");
    }
}

#[test]
fn the_bulbs_own_address_from_another_device_is_a_conflict_nine_seconds_after_the_start_but_its_own_late_broadcast_is_not() {
    for (seq, since_start, conflict) in [(36, 9_000, false), (36, 9_001, true), (37, 9_001, false), (100, 9_001, false)] {
        let mut s = up();
        fill(&mut s);
        s.nwk_seq = 100;
        s.hal.now = s.booted + since_start;
        heard(&mut s, nwk::Header::new(nwk::FRAME_DATA, 0xFFFD, 0x0042, 5, seq), 1, &[0x08]);
        assert_eq!(s.conflict.0 == 0x0042, conflict, "{seq} {since_start}");
    }
}

#[test]
fn a_broadcast_data_frame_from_an_address_in_conflict_is_not_passed_on_but_a_command_is() {
    for (frame_type, src_ext, passed_on) in [
        (nwk::FRAME_DATA, Some(SENDER + 1), true),
        (nwk::FRAME_DATA, Some(0x99), false),
        (nwk::FRAME_CMD, Some(0x99), true),
    ] {
        let mut s = up();
        fill(&mut s);
        let mut h = nwk::Header::new(frame_type, 0xFFFD, 0x1001, 5, 9);
        h.src_ext = src_ext;
        heard(&mut s, h, 1, &[0x7F]);
        assert_eq!(s.bcast[0].used, passed_on, "{frame_type} {src_ext:?}");
    }
}

#[test]
fn a_frame_from_an_end_device_that_is_no_child_tells_it_to_rejoin_unless_it_is_a_rejoin_request() {
    for (frame_type, payload, told) in
        [(nwk::FRAME_DATA, &[nwk::CMD_REJOIN_REQUEST, 0x01, 0x06, 0x00][..], true), (nwk::FRAME_CMD, &[nwk::CMD_REJOIN_REQUEST, 0x80][..], false)]
    {
        let mut s = up();
        fill(&mut s);
        let mut h = nwk::Header::new(frame_type, 0x0042, 0x1000, 5, 9);
        h.end_device_initiator = true;
        heard(&mut s, h, 1, payload);
        let leave = s.hal.sent.iter().any(|f| f.len() > 9 && f[5..7] == [0x00, 0x10]);
        assert_eq!(leave, told, "{frame_type}");
    }
}

#[test]
fn a_frame_kept_for_a_sleepy_child_counts_as_sent_only_when_it_is_the_bulbs_own_data() {
    for (frame_type, src, counted) in [(nwk::FRAME_DATA, 0x0042, true), (nwk::FRAME_DATA, 0x3000, false), (nwk::FRAME_CMD, 0x0042, false)] {
        let mut s = up();
        s.children[0].joined = true;
        s.children[0].capability = 0;
        let status = s.transmit_nwk(0x2000, &nwk::Header::new(frame_type, 0x2000, src, 5, 1), &[0x40]);
        assert_eq!(status, crate::hal::TxStatus::Ok);
        assert!(s.hal.sent.is_empty(), "kept for the child's poll");
        assert_eq!(s.diag.n[k::APS_TX_UCAST_SUCCESS] == 1, counted, "{frame_type} {src:04X}");
    }
}

#[test]
fn a_source_routed_frame_that_fails_is_counted_by_why_and_reported_unless_it_is_a_status_itself() {
    let full = k::RELAY_DROP_QUEUE_FULL;
    let cases: [(u16, u8, u8, u8, bool, [u32; 3], bool); 6] = [
        (0x3000, nwk::FRAME_DATA, 0x00, 0, false, [0, 1, 0], true),
        (0x3000, nwk::FRAME_DATA, 0x00, 2, false, [0, 0, 0], false),
        (0x3000, nwk::FRAME_DATA, 0x00, 2, true, [0, 0, 1], true),
        (0x3000, nwk::FRAME_CMD, 0x7F, 0, false, [0, 1, 0], true),
        (0x3000, nwk::FRAME_CMD, nwk::CMD_NETWORK_STATUS, 0, false, [0, 1, 0], false),
        (0x0042, nwk::FRAME_DATA, 0x00, 0, false, [0, 0, 0], false),
    ];
    for (src, frame_type, first, retries, held_full, counts, reported) in cases {
        let mut s = relaying();
        s.hal.deaf = [0x1002].to_vec();
        route(&mut s, 0, 0x3000, 0x1001, false, 0);
        if held_full {
            for e in s.held.iter_mut() {
                e.used = true;
            }
        }
        let h = nwk::Header::new(frame_type, 0x5000, src, 5, 1);
        s.send_to(0x1002, &h, &[first, 0x00, 0x00, 0x50], retries);
        let c = [s.diag.n[k::RELAYED_UCAST], s.diag.n[k::RELAY_DROP_MAC_FAIL], s.diag.n[full]];
        assert_eq!(c, counts, "{src:04X} {frame_type} {first:02X} {retries} {held_full}");
        let status = s.hal.sent.iter().any(|f| f[5..7] == [0x01, 0x10]);
        assert_eq!(status, reported, "{src:04X} {frame_type} {first:02X} {retries} {held_full}");
    }
    let mut s = relaying();
    s.send_to(0x1002, &nwk::Header::new(nwk::FRAME_DATA, 0x5000, 0x3000, 5, 1), &[0x00], 2);
    assert_eq!(s.diag.n[k::RELAYED_UCAST], 1);
}

#[test]
fn a_network_status_of_a_missing_route_or_a_failed_link_drops_the_route_it_tells_of() {
    for (status, dropped) in [
        (nwk::STATUS_NO_ROUTE, true),
        (nwk::STATUS_TREE_LINK_FAILURE, true),
        (nwk::STATUS_LINK_FAILURE, true),
        (nwk::STATUS_MANY_TO_ONE_FAILURE, false),
    ] {
        let mut s = relaying();
        route(&mut s, 0, 0x5000, 0x1001, false, 0);
        let h = nwk::Header::new(nwk::FRAME_CMD, 0x0042, 0x1001, 5, 1);
        s.on_nwk_command(&h, 0x1001, &[nwk::CMD_NETWORK_STATUS, status, 0x00, 0x50]);
        assert_eq!(s.routes[0].status == super::ROUTE_FREE, dropped, "{status:02X}");
        assert_eq!(s.diag.last_status_rx, status);
    }
}

#[test]
fn a_leave_without_its_options_is_ignored_and_a_neighbours_own_leave_forgets_it() {
    let mut s = relaying();
    let h = nwk::Header::new(nwk::FRAME_CMD, 0x0042, 0x1001, 5, 1);
    s.on_nwk_command(&h, 0x1001, &[nwk::CMD_LEAVE]);
    assert!(s.neighbors[1].used);
    s.on_nwk_command(&h, 0x1001, &[nwk::CMD_LEAVE, 0x00]);
    assert!(!s.neighbors[1].used);
    assert_eq!(s.leave_at, None);
    s.on_nwk_command(&h, 0x1001, &[nwk::CMD_LEAVE, 0x40]);
    assert_eq!(s.leave_at, Some(1_100));
}

#[test]
fn a_network_update_is_taken_only_for_the_bulbs_extended_pan_and_with_all_its_fields() {
    let update = |ext_pan: u64| {
        let mut p = std::vec![nwk::CMD_NETWORK_UPDATE, 0x01];
        p.extend_from_slice(&ext_pan.to_le_bytes());
        p.extend_from_slice(&[7, 0x34, 0x12]);
        p
    };
    for (payload, taken) in [(update(0xDDDD), true), (update(0xEEEE), false), (update(0xDDDD)[..12].to_vec(), false)] {
        let mut s = relaying();
        s.nib.ext_pan = 0xDDDD;
        let h = nwk::Header::new(nwk::FRAME_CMD, 0xFFFF, 0x0000, 5, 1);
        s.on_nwk_command(&h, 0x1001, &payload);
        assert_eq!(s.pan_at.is_some(), taken, "{payload:02X?}");
    }
}

#[test]
fn a_conflict_is_reported_after_a_jitter_of_up_to_64_ms_and_the_same_one_is_quiet_for_five_seconds() {
    let mut s = up();
    s.hal.random = [super::CONFLICT_JITTER_MS].to_vec();
    s.address_conflict(0x1000);
    assert_eq!((s.conflict, s.conflict_due), ((0x1000, 1_000 + super::CONFLICT_QUIET_MS), Some(1_000 + super::CONFLICT_JITTER_MS)));
    s.conflict_due = None;
    s.hal.now = 1_000 + super::CONFLICT_QUIET_MS - 1;
    s.address_conflict(0x1000);
    assert_eq!(s.conflict_due, None);
    s.address_conflict(0x1001);
    assert!(s.conflict_due.is_some());
}

#[test]
fn a_conflict_over_the_bulbs_own_address_or_an_end_device_childs_gives_a_new_one() {
    let mut s = up();
    s.hal.random = [0x0777, 199].to_vec();
    s.address_conflict_resolve(0x0042);
    assert_eq!((s.nib.short, s.announce_at), (0x0777, Some(1_000 + 50 + 199)));
    let mut s = up();
    s.hal.random = [0x0888].to_vec();
    s.children[0].capability = super::super::parent::CAP_RX_ON;
    s.address_conflict_resolve(0x2000);
    assert_eq!(s.children[0].short, 0x0888);
    assert_eq!(s.hal.sent.len(), 1, "a rejoin response with it");
    let mut s = up();
    s.children[0].capability = super::CAP_FFD;
    s.address_conflict_resolve(0x2000);
    assert_eq!((s.children[0].short, s.hal.sent.len(), s.nib.short), (0x2000, 0, 0x0042), "a router child picks its own");
}

#[test]
fn a_route_reply_is_taken_only_when_addressed_to_the_bulb_and_a_link_status_also_when_unicast() {
    for (dst, taken) in [(0x0042, true), (0x7777, false)] {
        let mut s = relaying();
        s.discoveries[0] = super::Discovery { used: true, id: 7, src: 0x3000, sender: 0x1002, forward_cost: 3, residual_cost: 0xFF, expires: 9_000 };
        let h = nwk::Header::new(nwk::FRAME_CMD, dst, 0x1001, 5, 2);
        s.on_nwk_command(&h, 0x1001, &[nwk::CMD_ROUTE_REPLY, 0, 7, 0x00, 0x30, 0x55, 0x55, 2]);
        assert_eq!(s.discoveries[0].residual_cost == 2, taken, "{dst:04X}");
    }
    let mut s = relaying();
    let h = nwk::Header::new(nwk::FRAME_CMD, 0x0042, 0x1001, 1, 2);
    s.on_nwk_command(&h, 0x1001, &[nwk::CMD_LINK_STATUS, 0x60]);
    assert_eq!(s.neighbors[1].age, 0);
}

struct Tried {
    counts: [u32; 4],
    to: Vec<u16>,
    held: bool,
}

fn try_route(s: &mut Stack<Scripted>, frame_type: u8, src: u16, discover: u8, payload: &[u8], retries: u8) -> Tried {
    let mut h = nwk::Header::new(frame_type, 0x5000, src, 5, 1);
    h.discover = discover;
    s.route_try(&h, payload, retries);
    Tried {
        counts: [s.diag.n[k::RELAYED_UCAST], s.diag.n[k::RELAY_DROP_MAC_FAIL], s.diag.n[k::RELAY_DROP_QUEUE_FULL], s.diag.n[k::RELAY_DROP_NO_ROUTE]],
        to: s.hal.sent.iter().map(|f| u16::from_le_bytes([f[5], f[6]])).collect(),
        held: s.held.iter().any(|e| e.used),
    }
}

fn routed(many_to_one: bool, deaf: &[u16]) -> Stack<Scripted> {
    let mut s = relaying();
    route(&mut s, 0, 0x5000, 0x1002, many_to_one, 0);
    route(&mut s, 1, 0x3000, 0x1001, false, 0);
    s.hal.deaf = deaf.to_vec();
    s
}

#[test]
fn a_route_record_that_fails_is_dropped_and_another_command_waits_for_a_new_route() {
    let mut s = routed(false, &[0x1002]);
    let t = try_route(&mut s, nwk::FRAME_CMD, 0x3000, 1, &[nwk::CMD_ROUTE_RECORD, 0], 0);
    assert_eq!((t.counts, t.held), ([0, 1, 0, 0], false));
    let mut s = routed(false, &[0x1002]);
    let t = try_route(&mut s, nwk::FRAME_CMD, 0x0042, 1, &[nwk::CMD_ROUTE_RECORD, 0], 0);
    assert_eq!((t.counts, t.held), ([0, 0, 0, 0], false), "the bulb's own");
    let mut s = routed(false, &[0x1002]);
    let t = try_route(&mut s, nwk::FRAME_CMD, 0x3000, 1, &[0x7F], 0);
    assert_eq!((t.counts, t.held), ([0, 0, 0, 0], true));
    let mut s = routed(false, &[0x1002]);
    let t = try_route(&mut s, nwk::FRAME_CMD, 0x3000, 0, &[0x7F], 0);
    assert_eq!((t.counts, t.held), ([0, 1, 0, 0], false));
    let mut s = routed(false, &[0x1002]);
    let t = try_route(&mut s, nwk::FRAME_CMD, 0x0042, 0, &[0x7F], 0);
    assert_eq!(t.counts, [0, 0, 0, 0]);
}

#[test]
fn a_many_to_one_route_whose_hop_failed_goes_through_another_router() {
    let mut s = routed(true, &[]);
    s.neighbors[2].failures = super::LINK_FAILURES;
    s.hal.random = [0].to_vec();
    let t = try_route(&mut s, nwk::FRAME_DATA, 0x3000, 0, &[0x00], 2);
    assert_eq!((t.to, t.counts), (std::vec![0x1000], [1, 0, 0, 0]));
    let mut s = routed(false, &[]);
    s.neighbors[2].failures = super::LINK_FAILURES;
    s.hal.random = [0].to_vec();
    let t = try_route(&mut s, nwk::FRAME_DATA, 0x3000, 0, &[0x00], 2);
    assert_eq!(t.to, std::vec![0x1002], "not a many-to-one route");
}

#[test]
fn relayed_data_that_fails_on_a_many_to_one_route_is_reported_and_sent_through_another_router() {
    let mut s = routed(true, &[0x1002]);
    s.hal.random = [0, 0].to_vec();
    let t = try_route(&mut s, nwk::FRAME_DATA, 0x3000, 0, &[0x00], 0);
    assert_eq!((t.to, t.counts), (std::vec![0x1002, 0x1000, 0x1000], [1, 0, 0, 0]));
    let mut s = routed(true, &[0x1002]);
    s.hal.random = [0, 0].to_vec();
    let t = try_route(&mut s, nwk::FRAME_DATA, 0x0042, 0, &[0x00], 0);
    assert_eq!((t.to, t.counts), (std::vec![0x1002, 0x1000, 0x1000], [0, 0, 0, 0]));
    let mut s = routed(true, &[0x1002, 0x1000]);
    s.hal.random = [0, 0].to_vec();
    let t = try_route(&mut s, nwk::FRAME_DATA, 0x3000, 1, &[0x00], 0);
    assert_eq!((t.counts, t.held), ([0, 1, 0, 0], false), "no other router took it");
    let mut s = routed(true, &[0x1002, 0x1000]);
    s.hal.random = [0, 0].to_vec();
    let t = try_route(&mut s, nwk::FRAME_DATA, 0x0042, 1, &[0x00], 0);
    assert_eq!((t.counts, t.held), ([0, 0, 0, 0], true), "the bulb's own waits for a new route");
}

#[test]
fn relayed_data_that_fails_is_reported_to_its_source_and_counted_by_why() {
    let mut s = routed(false, &[0x1002]);
    let t = try_route(&mut s, nwk::FRAME_DATA, 0x3000, 0, &[0x00], 0);
    assert_eq!((t.to, t.counts), (std::vec![0x1002, 0x1001], [0, 1, 0, 0]));
    let mut s = routed(false, &[0x1002]);
    for e in s.held.iter_mut() {
        e.used = true;
    }
    let t = try_route(&mut s, nwk::FRAME_DATA, 0x3000, 0, &[0x00], 2);
    assert_eq!((t.to, t.counts), (std::vec![0x1002, 0x1001], [0, 0, 1, 0]));
    let mut s = routed(false, &[0x1002]);
    let t = try_route(&mut s, nwk::FRAME_DATA, 0x0042, 0, &[0x00], 0);
    assert_eq!((t.to, t.counts, t.held), (std::vec![0x1002, 0xFFFF], [0, 0, 0, 0], true), "the bulb's own waits for a route request");
}

#[test]
fn an_unsecured_rejoin_request_is_answered_only_by_a_bulb_that_is_up() {
    for (state, answered) in [(State::Up, true), (State::WaitKey, false)] {
        let mut s = up();
        s.state = state;
        s.nib.tc_ext = super::super::NO_TRUST_CENTER;
        let mut h = nwk::Header::new(nwk::FRAME_CMD, 0x0042, 0x7001, 5, 9);
        h.src_ext = Some(0x00124B00_0000_7777);
        h.security = false;
        let mut f = crate::util::Buf::new();
        h.write(&mut f);
        f.bytes(&[nwk::CMD_REJOIN_REQUEST, 0x8E]);
        s.on_nwk(0x7001, f.as_slice(), 200);
        assert_eq!(!s.hal.sent.is_empty(), answered, "{state:?}");
    }
}

#[test]
fn an_empty_command_from_an_end_device_that_is_no_child_is_no_rejoin_request_whatever_its_mic() {
    let mut s = up();
    fill(&mut s);
    let mut h = nwk::Header::new(nwk::FRAME_CMD, 0x0042, 0x1000, 5, 9);
    h.end_device_initiator = true;
    let counter = (1..).find(|&c| secured(h, SENDER, c, 0, &[0; 16], &[])[8 + nwk::AUX_LEN] == nwk::CMD_REJOIN_REQUEST).unwrap();
    heard(&mut s, h, counter, &[]);
    assert!(s.hal.sent.iter().any(|f| f.len() > 9 && f[5..7] == [0x00, 0x10]), "told to rejoin");
}

#[test]
fn frames_that_cannot_be_read_are_dropped() {
    let mut s = relaying();
    s.on_nwk(0x1000, &[0x08], 200);
    assert_eq!(s.diag.n[k::PACKET_VALIDATE_DROPS], 1, "no header");
    let mut f = crate::util::Buf::new();
    let mut h = nwk::Header::new(nwk::FRAME_DATA, 0x0042, 0x1000, 5, 1);
    h.security = true;
    h.write(&mut f);
    f.bytes(&[0x28, 0x01]);
    s.on_nwk(0x1000, f.as_slice(), 200);
    assert_eq!(s.diag.n[k::PACKET_VALIDATE_DROPS], 2, "no whole auxiliary header");
    let h = nwk::Header::new(3, 0x0042, 0x1000, 5, 1);
    heard(&mut s, h, 1, &[0x01, 0x02]);
    assert!(s.hal.sent.is_empty(), "a reserved frame type is delivered nowhere");
}

#[test]
fn a_frame_too_long_to_send_is_left_unsent() {
    let mut s = relaying();
    let h = nwk::Header::new(nwk::FRAME_DATA, 0x1001, 0x0042, 5, 1);
    assert_eq!(s.transmit_nwk(0x1001, &h, &[0; 120]), crate::hal::TxStatus::Ok);
    assert!(s.hal.sent.is_empty());
    assert_eq!(s.transmit_nwk(0x1001, &h, &[0x01]), crate::hal::TxStatus::Ok, "an APS command");
    assert_eq!(s.hal.sent.len(), 1);
}

#[test]
fn relayed_data_that_fails_and_may_discover_waits_for_a_new_route() {
    let mut s = routed(false, &[0x1002]);
    let t = try_route(&mut s, nwk::FRAME_DATA, 0x3000, 1, &[0x00], 0);
    assert_eq!((t.counts, t.held), ([0, 0, 0, 0], true));
}

#[test]
fn route_commands_cut_short_and_route_requests_no_better_than_one_heard_are_dropped() {
    let mut s = relaying();
    let h = nwk::Header::new(nwk::FRAME_CMD, 0xFFFC, 0x2000, 5, 2);
    s.on_route_request(&h, 0x1000, 255, &[nwk::CMD_ROUTE_REQUEST, 0, 7, 0x55, 0x55], 1_000);
    assert!(s.discoveries.iter().all(|d| !d.used));
    s.on_route_request(&h, 0x1000, 255, &[nwk::CMD_ROUTE_REQUEST, 0, 7, 0x55, 0x55, 3], 1_000);
    assert_eq!(s.discoveries[0].forward_cost, 3 + s.neighbors[0].link_cost());
    s.bcast[0].used = false;
    s.on_route_request(&h, 0x1001, 255, &[nwk::CMD_ROUTE_REQUEST, 0, 7, 0x55, 0x55, 3], 1_000);
    assert!(s.bcast.iter().all(|e| !e.used), "the same cost again is not passed on");
    assert!(!s.discoveries[1].used);
    let h = nwk::Header::new(nwk::FRAME_CMD, 0x0042, 0x1001, 5, 2);
    s.discoveries[1] = super::Discovery { used: true, id: 7, src: 0x3000, sender: 0x1002, forward_cost: 3, residual_cost: 0xFF, expires: 9_000 };
    s.on_route_reply(&h, 0x1001, &[nwk::CMD_ROUTE_REPLY, 0, 7, 0x00, 0x30, 0x55, 0x55]);
    assert_eq!(s.discoveries[1].residual_cost, 0xFF, "no cost");
}

#[test]
fn a_reply_to_the_bulbs_own_request_no_cheaper_than_one_taken_is_dropped_and_one_passed_on_costs_the_way_back() {
    let mut s = relaying();
    s.discoveries[0] = super::Discovery { used: true, id: 7, src: 0x0042, sender: 0x0042, forward_cost: 0, residual_cost: 2, expires: 9_000 };
    route(&mut s, 0, 0x5555, 0x1000, false, 0);
    s.routes[0].status = super::ROUTE_DISCOVERY;
    let h = nwk::Header::new(nwk::FRAME_CMD, 0x0042, 0x1001, 5, 2);
    s.on_route_reply(&h, 0x1001, &[nwk::CMD_ROUTE_REPLY, 0, 7, 0x42, 0x00, 0x55, 0x55, 2]);
    assert_eq!((s.routes[0].status, s.routes[0].next), (super::ROUTE_DISCOVERY, 0x1000));
    let mut s = relaying();
    s.discoveries[0] = super::Discovery { used: true, id: 7, src: 0x3000, sender: 0x7001, forward_cost: 3, residual_cost: 0xFF, expires: 9_000 };
    s.on_route_reply(&h, 0x1001, &[nwk::CMD_ROUTE_REPLY, 0, 7, 0x00, 0x30, 0x55, 0x55, 2]);
    assert_eq!(s.hal.sent.len(), 1, "back to the sender, no longer a neighbour");
    assert_eq!(s.hal.sent[0][5..7], [0x01, 0x70]);
}

#[test]
fn a_link_status_from_a_device_that_is_no_neighbour_or_without_options_is_ignored() {
    let mut s = relaying();
    s.neighbors[0].age = 2;
    s.on_link_status(0x7001, &[nwk::CMD_LINK_STATUS, 0x60]);
    s.on_link_status(0x1000, &[nwk::CMD_LINK_STATUS]);
    assert_eq!(s.neighbors[0].age, 2);
}

#[test]
fn unsecured_aps_data_while_waiting_for_the_key_is_not_taken() {
    let read_basic = [0x00, 0x01, 0x00, 0x00, 0x04, 0x01, 0x01, 0x05, 0x00, 0x07, 0x00, 0x05, 0x00];
    let mut s = relaying();
    s.state = State::WaitKey;
    let mut h = nwk::Header::new(nwk::FRAME_DATA, 0x0042, 0x1000, 5, 9);
    h.security = false;
    let mut f = crate::util::Buf::new();
    h.write(&mut f);
    f.bytes(&read_basic);
    s.on_nwk(0x1000, f.as_slice(), 200);
    assert!(s.hal.sent.is_empty());
}

#[test]
fn frames_of_other_types_or_without_a_payload_are_sent_as_they_are() {
    let mut s = relaying();
    for (frame_type, payload) in [(nwk::FRAME_DATA, &[][..]), (nwk::FRAME_CMD, &[][..]), (2, &[0x01][..]), (nwk::FRAME_DATA, &[0x00, 0x01][..])] {
        let h = nwk::Header::new(frame_type, 0x1001, 0x0042, 5, 1);
        assert_eq!(s.transmit_nwk(0x1001, &h, payload), crate::hal::TxStatus::Ok);
    }
    assert_eq!(s.hal.sent.len(), 4);
}

#[test]
fn a_cheaper_copy_of_a_route_request_heard_is_passed_on() {
    let mut s = relaying();
    let h = nwk::Header::new(nwk::FRAME_CMD, 0xFFFC, 0x2000, 5, 2);
    s.on_route_request(&h, 0x1000, 255, &[nwk::CMD_ROUTE_REQUEST, 0, 7, 0x55, 0x55, 9], 1_000);
    s.on_route_request(&h, 0x1001, 255, &[nwk::CMD_ROUTE_REQUEST, 0, 7, 0x55, 0x55, 1], 1_000);
    assert_eq!((s.discoveries[0].sender, s.discoveries[0].forward_cost), (0x1001, 1 + s.neighbors[1].link_cost()));
    assert!(!s.discoveries[1].used, "the same discovery");
    assert_eq!(s.bcast.iter().filter(|e| e.used).count(), 1);
}

#[test]
fn a_busy_channel_is_a_failed_transmission_but_not_a_failure_of_the_link() {
    let mut s = relaying();
    route(&mut s, 0, 0x5000, 0x1002, false, 0);
    s.hal.busy = true;
    s.neighbors[1].failures = 2;
    let h = nwk::Header::new(nwk::FRAME_DATA, 0x1001, 0x0042, 5, 1);
    for _ in 0..super::LINK_FAILURES + 1 {
        assert_eq!(s.transmit_nwk(0x1001, &h, &[0x00]), crate::hal::TxStatus::ChannelBusy);
    }
    assert_eq!(s.neighbors[1].failures, 2);
    for _ in 0..super::LINK_FAILURES {
        s.transmit_nwk(0x1002, &h, &[0x00]);
    }
    assert_eq!(s.neighbors[2].failures, 0);
    assert!(!s.link_failed(0x1002));
    assert_eq!(s.routes[0].status, super::ROUTE_ACTIVE);
    s.hal.busy = false;
    s.hal.deaf = [0x1001].to_vec();
    assert_eq!(s.transmit_nwk(0x1001, &h, &[0x00]), crate::hal::TxStatus::NoAck);
    assert_eq!(s.neighbors[1].failures, 3);
    assert!(s.link_failed(0x1001));
    s.hal.deaf.clear();
    assert_eq!(s.transmit_nwk(0x1001, &h, &[0x00]), crate::hal::TxStatus::Ok);
    assert_eq!(s.neighbors[1].failures, 0);
}

#[test]
fn a_broadcast_lost_to_a_busy_channel_goes_out_again_with_its_retry() {
    let mut s = relaying();
    s.hal.busy = true;
    let h = nwk::Header::new(nwk::FRAME_DATA, nwk::BROADCAST_ALL, 0x0042, 5, 1);
    s.originate_broadcast(&h, &[0x00]);
    assert_eq!(s.hal.sent.len(), 1);
    s.hal.busy = false;
    let at = s.bcast.iter().find(|e| e.used).map(|e| e.at).expect("a queued retry");
    s.net_timers(at);
    assert_eq!(s.hal.sent.len(), 2);
    assert_eq!(s.hal.sent[1][9..17], s.hal.sent[0][9..17], "the same NWK frame");
}

#[test]
fn a_frame_for_a_child_not_yet_joined_is_not_passed_on() {
    let mut s = relaying();
    s.children[1].used = true;
    s.children[1].short = 0x5000;
    s.children[1].joined = false;
    let t = try_route(&mut s, nwk::FRAME_DATA, 0x3000, 0, &[0x00], 2);
    assert_eq!((t.to, t.counts), (Vec::new(), [0, 0, 0, 0]));
}

#[test]
fn a_frame_that_fails_with_retries_left_is_held_for_a_retry() {
    let mut s = routed(false, &[0x1002]);
    let t = try_route(&mut s, nwk::FRAME_DATA, 0x3000, 0, &[0x00], 2);
    assert_eq!((t.to, t.counts, t.held), (std::vec![0x1002], [0, 0, 0, 0], true));
    assert_eq!((s.held[0].left, s.held[0].next), (2, super::super::ROUTED));
}

#[test]
fn relayed_data_on_a_many_to_one_route_with_no_other_router_is_dropped() {
    let mut s = routed(true, &[0x1002]);
    for n in s.neighbors.iter_mut() {
        n.out_cost = 0;
    }
    let t = try_route(&mut s, nwk::FRAME_DATA, 0x3000, 0, &[0x00], 0);
    assert_eq!((t.to, t.counts), (std::vec![0x1002], [0, 1, 0, 0]));
}

#[test]
fn relayed_data_with_no_route_and_no_discovery_is_reported_and_other_frames_are_dropped() {
    let mut s = relaying();
    route(&mut s, 1, 0x3000, 0x1001, false, 0);
    let t = try_route(&mut s, nwk::FRAME_DATA, 0x3000, 0, &[0x00], 2);
    assert_eq!((t.to, t.counts), (std::vec![0x1001], [0, 0, 0, 1]));
    let mut s = relaying();
    let t = try_route(&mut s, nwk::FRAME_CMD, 0x3000, 0, &[0x7F], 2);
    assert_eq!((t.to, t.counts, t.held), (Vec::new(), [0, 0, 0, 1], false));
}

#[test]
fn an_old_broadcast_of_the_bulbs_own_still_remembered_is_its_echo_not_a_conflict() {
    let mut s = up();
    fill(&mut s);
    s.nwk_seq = 100;
    s.hal.now = s.booted + 9_001;
    s.seen[0] = super::Seen { used: true, src: 0x0042, seq: 36, expires: s.hal.now + 5_000 };
    heard(&mut s, nwk::Header::new(nwk::FRAME_DATA, 0xFFFD, 0x0042, 5, 36), 1, &[0x08]);
    assert_ne!(s.conflict.0, 0x0042);
}

#[test]
fn a_network_status_passed_on_drops_its_route_only_when_the_hop_does_not_answer() {
    for (deaf, kept) in [(false, true), (true, false)] {
        let mut s = relaying();
        route(&mut s, 0, 0x5000, 0x1002, false, 0);
        if deaf {
            s.hal.deaf = std::vec![0x1002];
        }
        relay_status(&mut s, 0x5000, nwk::STATUS_NO_ROUTE);
        assert_eq!(s.routes[0].status == super::ROUTE_ACTIVE, kept, "deaf {deaf}");
    }
}

fn listened(s: &mut Stack<Scripted>, short: u16, options: u8, times: u32) {
    for _ in 0..times {
        s.hal.now += crate::stack::LINK_STATUS_MS;
        s.on_link_status(short, &[nwk::CMD_LINK_STATUS, options]);
    }
}

fn measured_neighbor(s: &mut Stack<Scripted>, i: usize, short: u16) {
    let n = &mut s.neighbors[i];
    (n.used, n.short, n.ext) = (true, short, 0x00124B00_0000_0000 + short as u64);
    n.delivery_sample(16, false);
}

#[test]
fn an_idle_link_to_a_router_takes_its_cost_from_the_last_frame_of_each_link_status_heard() {
    let mut s = stack();
    measured_neighbor(&mut s, 0, 0x1000);
    listened(&mut s, 0x1000, 0x20, 12);
    assert_eq!((s.neighbors[0].heard_n, s.neighbors[0].link_cost()), (0, 7), "a first frame of several is not a whole link status");
    listened(&mut s, 0x1000, 0x60, 9);
    assert_eq!((s.neighbors[0].heard_n, s.neighbors[0].link_cost()), (8, 1));
}

#[test]
fn the_coordinator_the_next_hop_of_a_route_and_a_link_acknowledged_lately_keep_their_measured_cost() {
    let mut s = stack();
    s.hal.now = 1_000_000;
    measured_neighbor(&mut s, 0, 0x0000);
    measured_neighbor(&mut s, 1, 0x1001);
    measured_neighbor(&mut s, 2, 0x1002);
    measured_neighbor(&mut s, 3, 0x1003);
    route(&mut s, 0, 0x0000, 0x1001, true, 0);
    route(&mut s, 1, 0x3000, 0x1003, false, 0);
    s.routes[1].status = super::ROUTE_DISCOVERY;
    for _ in 0..9 {
        s.neighbors[2].acked_at = s.hal.now;
        for short in [0x0000, 0x1001, 0x1002, 0x1003] {
            s.hal.now += 1;
            s.on_link_status(short, &[nwk::CMD_LINK_STATUS, 0x60]);
        }
        s.hal.now += crate::stack::LINK_STATUS_MS - 4;
    }
    let costs: Vec<u8> = s.neighbors[..4].iter().map(|n| n.link_cost()).collect();
    assert_eq!(costs, [7, 7, 7, 1], "only the link that is not a route's next hop and was not acknowledged within a minute");
    assert!(s.neighbors[..4].iter().all(|n| n.heard_n == 8), "all are counted");
    s.neighbors[2].acked_at = s.hal.now - super::super::ACTIVE_MS;
    s.on_link_status(0x1002, &[nwk::CMD_LINK_STATUS, 0x60]);
    assert_eq!(s.neighbors[2].link_cost(), 1, "a minute after its last acknowledgement");
}

#[test]
fn a_full_table_keeps_the_next_hop_of_a_route_even_when_it_is_the_oldest() {
    let mut s = stack();
    fill(&mut s);
    s.neighbors[5].age = 9;
    s.neighbors[9].age = 8;
    route(&mut s, 0, 0x0000, 0x1005, true, 0);
    s.neighbor_heard(0x2222, 0x00124B00_0000_2222, 200, 7, 0);
    assert_eq!((s.neighbors[5].short, s.neighbors[9].short), (0x1005, 0x2222));
    let mut all = stack();
    fill(&mut all);
    all.neighbors[7].age = 3;
    for i in 0..NEIGHBORS {
        route(&mut all, i, 0x3000 + i as u16, 0x1000 + i as u16, false, 0);
    }
    all.neighbor_heard(0x2222, 0x00124B00_0000_2222, 200, 7, 0);
    assert_eq!(all.neighbors[7].short, 0x2222, "the oldest when every one carries a route");
}

#[test]
fn a_conflict_about_the_same_address_waits_out_its_quiet_time_and_is_handled_again_however_long_after() {
    let mut s = up();
    let t = s.hal.now;
    s.address_conflict(0x1234);
    assert!(s.conflict_due.is_some());
    s.net_timers(t + 100);
    assert!(s.conflict_due.is_none(), "reported");
    s.hal.now = t + 4_999;
    s.net_timers(s.hal.now);
    s.address_conflict(0x1234);
    assert!(s.conflict_due.is_none(), "still quiet");
    s.hal.now = t + 5_000;
    s.net_timers(s.hal.now);
    s.hal.now = t + 5_000 + 0x8000_0000;
    s.address_conflict(0x1234);
    assert!(s.conflict_due.is_some(), "24.9 days later");
}

#[test]
fn a_broadcast_remembered_long_ago_does_not_make_a_new_one_with_the_same_number_a_duplicate() {
    let mut s = up();
    let t = s.hal.now;
    assert_eq!(s.seen_before(0x0000, 5, t, false), Some(false));
    assert_eq!(s.seen_before(0x0000, 5, t + 1_000, false), Some(true), "a copy");
    s.net_timers(t + 8_999);
    assert!(s.broadcast_seen(0x0000, 5, t + 8_999), "still remembered");
    assert!(s.seen.iter().any(|e| e.used), "the timers keep a transaction still remembered");
    s.net_timers(t + 9_000);
    assert!(s.seen.iter().all(|e| !e.used), "the timers forget an expired transaction");
    let later = t + 9_000 + 0x8000_0000;
    assert!(!s.broadcast_seen(0x0000, 5, later), "24.9 days later the transaction is long gone");
    assert_eq!(s.seen_before(0x0000, 5, later, false), Some(false), "24.9 days later it is a new broadcast");
}

#[test]
fn a_broadcast_table_filled_long_ago_takes_new_broadcasts() {
    let mut s = up();
    let t = s.hal.now;
    for i in 0..crate::stack::BTT as u16 {
        assert_eq!(s.seen_before(0x0100 + i, 1, t, false), Some(false));
    }
    s.net_timers(t + 9_000);
    let later = t + 9_000 + 0x8000_0000;
    assert_eq!(s.seen_before(0x7777, 1, later, false), Some(false), "24.9 days later the table is free");
}

#[test]
fn an_early_link_status_still_comes_early_after_25_days_of_uptime() {
    for uptime in [60_000u32, 0x8000_0000 + 60_000] {
        let mut s = stack();
        fill(&mut s);
        s.booted = 1_000;
        let now = 1_000u32.wrapping_add(uptime);
        s.hal.now = now;
        s.link_status_sent = now.wrapping_sub(10_000);
        s.link_status_at = now.wrapping_add(5_000);
        s.link_status_soon();
        assert_eq!(s.link_status_at, now.wrapping_add(107), "uptime {uptime:#X} ms");
    }
}

#[test]
fn a_conflict_heard_about_another_address_does_not_replace_the_own_one_waiting_for_its_jitter() {
    let mut s = up();
    s.hal.random = [super::CONFLICT_JITTER_MS, 0x0777, 199].to_vec();
    s.address_conflict(0x0042);
    s.hal.now = 1_010;
    let h = nwk::Header::new(nwk::FRAME_CMD, nwk::BROADCAST_RX_ON, 0x1000, 5, 9);
    s.on_nwk_command(&h, 0x1000, &[nwk::CMD_NETWORK_STATUS, nwk::STATUS_ADDRESS_CONFLICT, 0x34, 0x12]);
    s.hal.now = 1_000 + super::CONFLICT_JITTER_MS;
    s.net_timers(s.hal.now);
    assert_eq!(s.nib.short, 0x0777, "the bulb's own address, found in conflict first, is never given up");
}

#[test]
fn a_frame_relayed_on_a_busy_channel_keeps_the_route_and_reports_no_link_failure() {
    let mut s = relaying();
    route(&mut s, 0, 0x5000, 0x1002, false, 0);
    s.hal.busy = true;
    let h = nwk::Header::new(nwk::FRAME_DATA, 0x5000, 0x1000, 5, 1);
    s.relay(h, 0x1000, &[0x00, 0x01]);
    for _ in 0..4 {
        s.hal.now += 100;
        s.net_timers(s.hal.now);
    }
    let statuses = s.hal.sent.iter().filter(|f| f.len() > 9 && f[9] & 3 == nwk::FRAME_CMD).count();
    assert_eq!((s.routes[0].status, s.neighbors[2].failures), (super::ROUTE_ACTIVE, 0));
    assert_eq!(statuses, 0, "a network status (link failure) went back to the source");
}

#[test]
fn a_frame_lost_to_a_busy_channel_is_counted_by_whose_it_was_and_starts_no_route_discovery() {
    let cases: [(u8, u16, u8, [u32; 4], u32); 4] = [
        (nwk::FRAME_DATA, 0x3000, 0, [0, 1, 0, 0], 0),
        (nwk::FRAME_DATA, 0x3000, 1, [0, 1, 0, 0], 0),
        (nwk::FRAME_DATA, 0x0042, 1, [0, 0, 0, 0], 1),
        (nwk::FRAME_CMD, 0x0042, 1, [0, 0, 0, 0], 0),
    ];
    for (frame_type, src, discover, counts, aps_failed) in cases {
        let mut s = routed(true, &[]);
        s.hal.busy = true;
        let t = try_route(&mut s, frame_type, src, discover, &[0x7F], 0);
        let at = (frame_type, src, discover);
        assert_eq!((t.to, t.counts, t.held), (std::vec![0x1002], counts, false), "{at:?}");
        assert_eq!((s.diag.n[k::APS_TX_UCAST_FAIL], s.routes[0].status), (aps_failed, super::ROUTE_ACTIVE), "{at:?}");
    }
}

#[test]
fn a_source_routed_frame_lost_to_a_busy_channel_is_not_reported_as_a_failed_route() {
    let mut s = relaying();
    route(&mut s, 0, 0x5000, 0x1002, false, 0);
    s.hal.busy = true;
    s.send_to(0x1002, &nwk::Header::new(nwk::FRAME_DATA, 0x5000, 0x3000, 5, 1), &[0x00], 0);
    assert_eq!((s.hal.sent.len(), s.diag.n[k::RELAY_DROP_MAC_FAIL], s.routes[0].status), (1, 1, super::ROUTE_ACTIVE));
}

#[test]
fn a_network_status_lost_to_a_busy_channel_keeps_its_route_and_takes_no_other_router() {
    for status in [nwk::STATUS_NO_ROUTE, nwk::STATUS_MANY_TO_ONE_FAILURE] {
        let mut s = relaying();
        route(&mut s, 0, 0x5000, 0x1002, false, 0);
        s.hal.busy = true;
        relay_status(&mut s, 0x5000, status);
        let got = (relay_counts(&s), s.hal.sent.len(), s.routes[0].status);
        assert_eq!(got, ([0, 1, 0], 1, super::ROUTE_ACTIVE), "status {status:02X}");
    }
}

#[test]
fn a_next_hop_that_never_acked_before_the_channel_went_busy_still_loses_its_route() {
    let mut s = relaying();
    route(&mut s, 0, 0x5000, 0x1002, false, 0);
    s.hal.busy = true;
    s.hal.attempts = 3;
    let h = nwk::Header::new(nwk::FRAME_DATA, 0x5000, 0x1000, 5, 1);
    s.relay(h, 0x1000, &[0x00, 0x01]);
    for _ in 0..4 {
        s.hal.now += 100;
        s.net_timers(s.hal.now);
    }
    assert_eq!(s.routes[0].status, super::ROUTE_FREE, "three unacked frames then one CCA failure: the route is kept");
}

#[test]
fn a_frame_sent_unacknowledged_before_the_channel_went_busy_counts_against_its_link() {
    let mut s = relaying();
    s.hal.busy = true;
    s.hal.attempts = 1;
    let h = nwk::Header::new(nwk::FRAME_DATA, 0x5000, 0x0042, 5, 1);
    assert_eq!(s.transmit_nwk(0x1001, &h, &[0x00]), crate::hal::TxStatus::NoAck);
    assert_eq!(s.neighbors[1].failures, 1);
}

#[test]
fn a_frame_lost_to_a_busy_channel_with_no_room_to_wait_for_a_retry_counts_as_a_full_queue() {
    let full = |s: &mut Stack<Scripted>| {
        s.hal.busy = true;
        for e in s.held.iter_mut() {
            e.used = true;
        }
    };
    let mut s = routed(false, &[]);
    full(&mut s);
    let t = try_route(&mut s, nwk::FRAME_DATA, 0x3000, 0, &[0x00], 2);
    assert_eq!((t.counts, s.routes[0].status), ([0, 0, 1, 0], super::ROUTE_ACTIVE));
    let mut s = relaying();
    route(&mut s, 0, 0x5000, 0x1002, false, 0);
    full(&mut s);
    s.send_to(0x1002, &nwk::Header::new(nwk::FRAME_DATA, 0x5000, 0x3000, 5, 1), &[0x00], 2);
    let got = (s.hal.sent.len(), s.diag.n[k::RELAY_DROP_MAC_FAIL], s.diag.n[k::RELAY_DROP_QUEUE_FULL], s.routes[0].status);
    assert_eq!(got, (1, 0, 1, super::ROUTE_ACTIVE));
}
