use super::super::address_tests::{Scripted, IDENTITY};
use super::super::diag::k;
use super::NEIGHBOR_FORGET;
use crate::hal::trace;
use crate::nwk;
use crate::stack::{Stack, State, AGE_LIMIT, MAX_NWK_PAYLOAD, NEIGHBORS, ROUTES};
extern crate std;
use std::vec::Vec;

fn stack() -> Stack<Scripted> {
    Stack::new(Scripted { random: Vec::new(), now: 1_000, sent: Vec::new() }, IDENTITY)
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
