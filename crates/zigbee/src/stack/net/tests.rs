use super::super::address_tests::{Scripted, IDENTITY};
use super::super::diag::k;
use super::NEIGHBOR_FORGET;
use crate::nwk;
use crate::stack::{Stack, State, AGE_LIMIT, NEIGHBORS};
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
