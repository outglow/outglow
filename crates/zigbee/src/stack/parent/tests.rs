use super::super::address_tests::{Scripted, IDENTITY};
use super::{Owner, INDIRECT_MS};
use crate::stack::{Stack, INDIRECT};
extern crate std;
use std::vec::Vec;

fn stack() -> Stack<Scripted> {
    Stack::new(Scripted { random: Vec::new(), now: 1_000, sent: Vec::new(), deaf: Vec::new(), busy: false, attempts: 0, bx: None }, IDENTITY)
}

fn child(s: &mut Stack<Scripted>, i: usize, short: u16, polled_at: u32, poll_ms: u32) {
    let c = &mut s.children[i];
    c.used = true;
    c.joined = true;
    c.short = short;
    c.polled_at = polled_at;
    c.poll_ms = poll_ms;
}

fn put(s: &mut Stack<Scripted>, slot: usize, short: u16, expires: u32) {
    let e = &mut s.indirect[slot];
    e.used = true;
    e.ext = false;
    e.addr = short as u64;
    e.expires = expires;
    e.broadcast = false;
}

#[test]
fn a_child_with_its_four_frames_waiting_gives_up_the_one_that_expires_first_the_earliest_placed_of_a_tie() {
    let mut s = stack();
    child(&mut s, 0, 0x0A00, 0, 0);
    for (slot, expires) in [(3, 5_000), (5, 2_000), (7, 2_000), (9, 6_000)] {
        put(&mut s, slot, 0x0A00, expires);
    }
    assert_eq!(s.indirect_slot(Owner::Child(0)), Some(5));
    child(&mut s, 1, 0x0B00, 0, 0);
    assert_eq!(s.indirect_slot(Owner::Child(1)), Some(0), "another child takes a free slot");
}

#[test]
fn a_full_pool_gives_up_the_frame_whose_child_polls_latest_after_it_expires() {
    let mut s = stack();
    child(&mut s, 0, 0x0A00, 1_000, 10_000);
    child(&mut s, 1, 0x0B00, 1_000, 12_000);
    child(&mut s, 2, 0x0C00, 1_000, 100);
    for slot in 0..INDIRECT {
        let owner = [0x0A00u16, 0x0B00, 0x0C00][slot % 3];
        put(&mut s, slot, owner, 1_000 + 5_000 + slot as u32);
    }
    assert_eq!(s.indirect_slot(Owner::Other(false, 0x0D00)), Some(1), "child 1's frame expires 8 s before its poll, the most");
    child(&mut s, 3, 0x0E00, 1_000, 1_000_000);
    assert_eq!(s.indirect_slot(Owner::Child(3)), None, "a frame for a child that polls later still is not worth one");
}

#[test]
fn a_full_pool_without_late_frames_gives_up_the_oldest_frame_of_whoever_holds_most() {
    let mut s = stack();
    for slot in 0..INDIRECT {
        let short = if slot < 10 { 0x0A00 } else { 0x0B00 + slot as u16 };
        put(&mut s, slot, short, 1_000 + INDIRECT_MS - slot as u32);
    }
    assert_eq!(s.indirect_slot(Owner::Other(false, 0x0D00)), Some(9));
}

#[test]
fn a_poll_that_leaves_frames_waiting_does_not_count_as_the_childs_poll_interval() {
    let mut s = stack();
    child(&mut s, 0, 0x0A00, 0, 0);
    put(&mut s, 0, 0x0A00, 1_000 + INDIRECT_MS);
    put(&mut s, 1, 0x0A00, 1_000 + INDIRECT_MS + 1);
    s.on_data_request(crate::mac::Addr::Short(0x0A00));
    assert!(s.children[0].more, "another frame waits");
    s.hal.now = 1_050;
    s.on_data_request(crate::mac::Addr::Short(0x0A00));
    assert!(!s.children[0].more);
    assert_eq!(s.children[0].poll_ms, 0, "50 ms after a poll that had more is no interval");
    s.hal.now = 8_050;
    s.on_data_request(crate::mac::Addr::Short(0x0A00));
    assert_eq!(s.children[0].poll_ms, 7_000);
}

#[test]
fn an_unauthenticated_child_goes_after_thirty_seconds_and_a_joined_end_device_after_its_timeout() {
    let mut s = stack();
    child(&mut s, 0, 0x0A00, 0, 0);
    s.children[0].joined = false;
    s.children[0].heard = 500;
    s.parent_timers(500 + 30_000);
    assert!(s.children[0].used);
    s.parent_timers(500 + 30_001);
    assert!(!s.children[0].used);
    child(&mut s, 1, 0x0B00, 0, 0);
    s.children[1].heard = 500;
    s.children[1].timeout = 0;
    let limit = s.children[1].timeout_ms();
    s.parent_timers(500 + limit);
    assert!(s.children[1].used);
    s.parent_timers(500 + limit + 1);
    assert!(!s.children[1].used);
}

#[test]
fn an_associating_device_is_heard_now_and_unauthenticated() {
    let mut s = stack();
    s.hal.now = 100_000;
    s.permit_until = Some(160_000);
    s.on_association_request(0x00124B00000000C3, 0x80);
    let c = s.children.iter().find(|c| c.used).copied().expect("a child");
    assert_eq!((c.ext, c.heard, c.heard_up, c.joined, c.capability), (0x00124B00000000C3, 100_000, true, false, 0x80));
    s.parent_timers(100_000 + 5_000);
    assert!(s.children.iter().any(|c| c.used), "kept while it gets its key");
}

const EXT: u64 = 0x00124B00_0000_7777;

fn rejoin(s: &mut Stack<Scripted>, src: u16, capability: u8, secured: bool) -> Option<(u16, u8)> {
    let mut h = crate::nwk::Header::new(crate::nwk::FRAME_CMD, 0x0042, src, 1, 1);
    h.src_ext = Some(EXT);
    s.hal.sent.clear();
    s.on_rejoin_request(&h, src, &[crate::nwk::CMD_REJOIN_REQUEST, capability], secured);
    let f = s.hal.sent.first()?;
    let r = &f[f.len() - 4..];
    (!secured).then(|| (u16::from_le_bytes([r[1], r[2]]), r[3]))
}

fn up_parent() -> Stack<Scripted> {
    let mut s = stack();
    s.state = crate::stack::State::Up;
    s.nib.short = 0x0042;
    s.nib.tc_ext = 0x00124B00_0000_0001;
    s
}

#[test]
fn an_unsecured_rejoin_is_taken_only_from_a_known_device_with_its_address_and_kind() {
    let cases: [(u16, u8, u16, u8, Option<(u16, u8)>); 5] = [
        (0x2000, 0x88, 0x2000, 0x88, Some((0x2000, 0x00))),
        (0x2000, 0x88, 0x2000, 0x8C, Some((0x2000, 0x00))),
        (0x2000, 0x88, 0x2000, 0x80, Some((0x2000, 0x02))),
        (0x2000, 0x8A, 0x2000, 0x88, Some((0x2000, 0x02))),
        (0x2000, 0x88, 0x2001, 0x88, Some((0x2001, 0x02))),
    ];
    for (short, capability, src, asked, answer) in cases {
        let mut s = up_parent();
        let c = &mut s.children[0];
        c.used = true;
        c.ext = EXT;
        c.short = short;
        c.capability = capability;
        assert_eq!(rejoin(&mut s, src, asked, false), answer, "{capability:02X} {src:04X} {asked:02X}");
    }
}

#[test]
fn an_unsecured_rejoin_of_a_neighbour_is_taken_only_for_a_router_with_its_address() {
    for (src, asked, answer) in [(0x3000, 0x8E, (0x3000, 0x00)), (0x3000, 0x88, (0x3000, 0x02)), (0x3001, 0x8E, (0x3001, 0x02))] {
        let mut s = up_parent();
        let n = &mut s.neighbors[0];
        n.used = true;
        n.short = 0x3000;
        n.ext = EXT;
        assert_eq!(rejoin(&mut s, src, asked, false), Some(answer), "{src:04X} {asked:02X}");
    }
}

#[test]
fn a_rejoining_device_keeps_its_address_unless_it_is_taken() {
    let cases: [(u16, Option<(u16, u64)>, bool); 6] = [
        (0x5000, None, true),
        (0x0042, None, false),
        (0xFFF8, None, false),
        (0xFFF7, None, true),
        (0x5000, Some((0x5000, 0x99)), false),
        (0x5000, Some((0x5000, 0)), true),
    ];
    for (src, neighbor, kept) in cases {
        let mut s = up_parent();
        if let Some((short, ext)) = neighbor {
            let n = &mut s.neighbors[0];
            n.used = true;
            n.short = short;
            n.ext = ext;
        }
        s.hal.random = [0x0777].to_vec();
        let (short, status) = rejoin(&mut s, src, 0x8E, false).unwrap();
        assert_eq!((short == src, status), (kept, 0x00), "{src:04X} {neighbor:?}");
    }
    let mut s = up_parent();
    s.children[3].used = true;
    s.children[3].short = 0x5000;
    s.children[3].capability = 0x8E;
    s.hal.random = [0x0777].to_vec();
    assert_eq!(rejoin(&mut s, 0x5000, 0x8E, false), Some((0x0777, 0x00)), "another child's");
}

#[test]
fn a_secured_rejoin_renews_a_known_child_and_an_end_device_is_no_longer_a_neighbour() {
    let mut s = up_parent();
    let c = &mut s.children[2];
    c.used = true;
    c.ext = EXT;
    c.short = 0x2000;
    c.capability = 0x80;
    c.timeout = 3;
    c.heard = 5;
    let n = &mut s.neighbors[0];
    n.used = true;
    n.short = 0x2000;
    n.ext = EXT;
    s.hal.now = 4_000;
    rejoin(&mut s, 0x2000, 0x88, true);
    let c = s.children[2];
    assert_eq!((c.short, c.capability, c.joined, c.timeout, c.heard, c.heard_up), (0x2000, 0x88, true, super::DEFAULT_TIMEOUT, 4_000, true));
    assert!(!s.neighbors[0].used);
    let mut s = up_parent();
    let n = &mut s.neighbors[0];
    n.used = true;
    n.short = 0x3000;
    n.ext = EXT;
    n.fc_next = 77;
    n.fc_seq = 2;
    s.hal.now = 4_000;
    rejoin(&mut s, 0x3000, 0x8E, true);
    let c = s.children[0];
    assert_eq!((c.used, c.ext, c.joined, c.fc_next, c.fc_seq, c.heard, c.heard_up), (true, EXT, true, 77, 2, 4_000, true));
    assert!(s.neighbors[0].used, "a router stays a neighbour");
}

#[test]
fn a_child_removed_takes_its_frames_and_its_share_of_broadcasts_with_it() {
    let mut s = up_parent();
    let c = &mut s.children[2];
    c.used = true;
    c.joined = true;
    c.ext = EXT;
    c.short = 0x2002;
    let entries: [(bool, u64, bool, u32); 6] = [
        (true, EXT, false, 0),
        (false, 0x2002, false, 0),
        (false, 0x2003, false, 0),
        (true, EXT + 1, false, 0),
        (false, 0, true, 1 << 2 | 1 << 5),
        (false, 0, true, 1 << 2),
    ];
    for (k, (ext, addr, broadcast, due)) in entries.into_iter().enumerate() {
        let e = &mut s.indirect[k];
        e.used = true;
        e.ext = ext;
        e.addr = addr;
        e.broadcast = broadcast;
        e.due = due;
        e.expires = 50_000;
    }
    s.remove_child(2);
    let used: Vec<bool> = s.indirect[..6].iter().map(|e| e.used).collect();
    assert_eq!(used, [false, false, true, true, true, false]);
    assert_eq!(s.indirect[4].due, 1 << 5);
    assert!(!s.children[2].used);
    assert_eq!(s.diag.n[crate::stack::diag::k::CHILD_MOVED], 1);
}

#[test]
fn a_timeout_request_of_up_to_fourteen_is_taken_from_an_end_device_child_without_configuration() {
    for (timeout, configuration, taken) in [(14, 0, true), (15, 0, false), (3, 1, false)] {
        let mut s = up_parent();
        let c = &mut s.children[0];
        c.used = true;
        c.joined = true;
        c.short = 0x2000;
        c.capability = 0x88;
        let h = crate::nwk::Header::new(crate::nwk::FRAME_CMD, 0x0042, 0x2000, 1, 1);
        s.on_timeout_request(&h, &[crate::nwk::CMD_TIMEOUT_REQUEST, timeout, configuration]);
        assert_eq!(s.children[0].timeout == timeout, taken, "{timeout} {configuration}");
        assert_eq!(s.hal.sent.len(), 1, "answered either way");
    }
}

#[test]
fn a_refused_rejoin_of_a_sleepy_device_waits_for_its_poll_when_there_is_room() {
    let mut s = up_parent();
    s.nib.tc_ext = crate::stack::NO_TRUST_CENTER;
    rejoin(&mut s, 0x5000, 0x80, false);
    assert!(s.hal.sent.is_empty());
    assert!(s.indirect.iter().any(|e| e.used && !e.ext && e.addr == 0x5000));
    let mut s = up_parent();
    s.nib.tc_ext = crate::stack::NO_TRUST_CENTER;
    for e in s.indirect.iter_mut() {
        e.used = true;
        e.addr = 0x7000;
        e.expires = 50_000;
    }
    rejoin(&mut s, 0x5000, 0x80, false);
    assert!(s.indirect.iter().all(|e| e.addr == 0x7000), "no room");
    s.indirect[3].expires = 900;
    rejoin(&mut s, 0x5000, 0x80, false);
    assert_eq!(s.indirect.iter().filter(|e| e.addr == 0x5000).count(), 1, "an expired one makes room");
}

fn frame(s: &mut Stack<Scripted>, slot: usize) {
    let mut f = crate::util::Buf::new();
    crate::mac::Header {
        frame_type: crate::mac::FRAME_DATA,
        security: false,
        pending: false,
        ack_request: true,
        seq: 1,
        dst_pan: 0x1A62,
        dst: crate::mac::Addr::Short(0x2000),
        src_pan: 0x1A62,
        src: crate::mac::Addr::Short(0x0042),
    }
    .write(&mut f);
    f.u8(0x55);
    s.indirect[slot].frame = f;
}

#[test]
fn a_poll_takes_a_frame_kept_for_the_childs_extended_address_and_one_without_a_source_takes_nothing() {
    let mut s = up_parent();
    child(&mut s, 0, 0x2000, 0, 7_500);
    s.children[0].ext = EXT;
    put(&mut s, 0, 0x2000, 50_000);
    s.indirect[0].ext = true;
    s.indirect[0].addr = EXT;
    frame(&mut s, 0);
    s.on_data_request(crate::mac::Addr::None);
    assert!(s.hal.sent.is_empty());
    s.on_data_request(crate::mac::Addr::Short(0x2000));
    assert_eq!(s.hal.sent.len(), 1);
    assert!(!s.indirect[0].used);
}

#[test]
fn an_association_response_delivered_after_its_child_is_gone_tells_the_trust_center_nothing() {
    let mut s = up_parent();
    put(&mut s, 0, 0, 50_000);
    s.indirect[0].ext = true;
    s.indirect[0].addr = EXT;
    s.indirect[0].association = true;
    frame(&mut s, 0);
    s.on_data_request(crate::mac::Addr::Ext(EXT));
    assert_eq!(s.hal.sent.len(), 1, "the response, no update device");
}

#[test]
fn a_timeout_request_out_of_range_from_a_device_that_is_no_child_is_refused_to_its_extended_address() {
    let mut s = up_parent();
    let mut h = crate::nwk::Header::new(crate::nwk::FRAME_CMD, 0x0042, 0x2000, 1, 1);
    h.src_ext = Some(EXT);
    s.on_timeout_request(&h, &[crate::nwk::CMD_TIMEOUT_REQUEST, 15, 0]);
    assert_eq!(s.hal.sent.len(), 1);
    let f = &s.hal.sent[0];
    let (nh, _) = crate::nwk::Header::parse(&f[9..]).unwrap();
    assert_eq!(nh.dst_ext, Some(EXT));
}

#[test]
fn a_broadcast_too_long_to_keep_for_sleeping_children_is_not_kept() {
    let mut s = up_parent();
    child(&mut s, 0, 0x2000, 0, 7_500);
    let h = crate::nwk::Header::new(crate::nwk::FRAME_DATA, 0xFFFF, 0x0000, 5, 1);
    s.broadcast_to_sleepy(&h, &[0; 120]);
    assert!(s.indirect.iter().all(|e| !e.used));
    s.broadcast_to_sleepy(&h, &[0; 10]);
    assert!(s.indirect.iter().any(|e| e.used && e.broadcast));
}

#[test]
fn a_router_already_a_neighbour_associating_keeps_its_address_and_an_end_device_in_its_place_gets_a_new_one() {
    for (capability, kept) in [(0x8Eu8, true), (0x80, false)] {
        let mut s = up_parent();
        s.permit_until = Some(100_000);
        s.allow_children = true;
        let n = &mut s.neighbors[0];
        n.used = true;
        n.short = 0x3000;
        n.ext = EXT;
        s.hal.random = std::vec![0x0777];
        s.on_association_request(EXT, capability);
        assert_eq!(s.children[0].short == 0x3000, kept, "{capability:02X}");
    }
}

#[test]
fn of_frames_equally_late_the_first_is_given_up_and_a_new_one_as_late_is_not_worth_its_place() {
    let mut s = stack();
    child(&mut s, 0, 0x0A00, 1_000, 10_000);
    child(&mut s, 1, 0x0B00, 1_000, 10_000);
    for slot in 0..INDIRECT {
        put(&mut s, slot, [0x0A00u16, 0x0B00][slot % 2], 6_000);
    }
    assert_eq!(s.indirect_slot(Owner::Other(false, 0x0D00)), Some(0), "the first of the latest");
    child(&mut s, 2, 0x0C00, 1_000, 10_000 + INDIRECT_MS - 5_000);
    assert_eq!(s.indirect_slot(Owner::Child(2)), None, "as late as the latest");
}

#[test]
fn a_frame_late_by_half_the_clock_or_more_is_not_late_but_early() {
    let mut s = stack();
    child(&mut s, 0, 0x0A00, 0, 1);
    assert_eq!(s.too_late(Owner::Child(0), 1u32.wrapping_sub(0x8000_0000)), None);
    assert_eq!(s.too_late(Owner::Child(0), 1u32.wrapping_sub(0x7FFF_FFFF)), Some(0x7FFF_FFFF));
}

fn marked(s: &mut Stack<Scripted>, slot: usize, ext: bool, addr: u64, expires: u32, mark: u8) {
    let e = &mut s.indirect[slot];
    e.used = true;
    e.ext = ext;
    e.addr = addr;
    e.expires = expires;
    e.broadcast = false;
    e.association = false;
    let mut f = crate::util::Buf::new();
    f.bytes(&[0x63, 0xCC, 0x01, 0x62, 0x1A, 0x00, 0x00, 0x00, 0x00, mark]);
    e.frame = f;
}

#[test]
fn a_poll_by_extended_address_takes_only_that_devices_frame_and_of_two_ending_together_the_first() {
    let mut s = up_parent();
    marked(&mut s, 0, true, 0xBB, 5_000, 0xB0);
    marked(&mut s, 1, true, 0xAA, 9_000, 0xA0);
    marked(&mut s, 2, false, 0xAA, 3_000, 0xC0);
    s.on_data_request(crate::mac::Addr::Ext(0xAA));
    assert_eq!(s.hal.sent.last().and_then(|f| f.last()), Some(&0xA0));
    let mut s = up_parent();
    child(&mut s, 0, 0x2000, 0, 0);
    marked(&mut s, 3, false, 0x2000, 5_000, 0x31);
    marked(&mut s, 4, false, 0x2000, 5_000, 0x32);
    s.on_data_request(crate::mac::Addr::Short(0x2000));
    assert_eq!(s.hal.sent.last().and_then(|f| f.last()), Some(&0x31));
}

#[test]
fn a_new_child_rejoining_takes_away_only_the_frames_kept_for_its_short_address() {
    let mut s = up_parent();
    marked(&mut s, 0, false, 0x5000, 50_000, 1);
    marked(&mut s, 1, true, 0x77, 50_000, 2);
    marked(&mut s, 2, false, 0x5001, 50_000, 3);
    rejoin(&mut s, 0x5000, 0x8E, true);
    assert_eq!([s.indirect[0].used, s.indirect[1].used, s.indirect[2].used], [false, true, true]);
}

#[test]
fn an_expired_permit_join_window_is_closed_by_the_timers_and_does_not_reopen_when_the_clock_wraps() {
    let mut s = stack();
    s.state = crate::stack::State::Up;
    s.permit_until = Some(1_000 + 180_000);
    s.parent_timers(1_000 + 179_999);
    assert!(s.permit_until.is_some());
    s.parent_timers(1_000 + 180_000);
    s.hal.now = 1_000 + 180_000 + 0x8000_0000;
    assert!(!s.permit_open());
}

#[test]
fn a_sleepy_device_rejoining_on_a_taken_address_gets_its_answer_at_its_poll() {
    let mut s = up_parent();
    let n = &mut s.neighbors[0];
    n.used = true;
    n.short = 0x5000;
    n.ext = 0x99;
    s.hal.random = [0x0777].to_vec();
    s.hal.sent.clear();
    let mut h = crate::nwk::Header::new(crate::nwk::FRAME_CMD, 0x0042, 0x5000, 1, 1);
    h.src_ext = Some(EXT);
    s.on_rejoin_request(&h, 0x5000, &[crate::nwk::CMD_REJOIN_REQUEST, 0x80], false);
    let c = s.children.iter().find(|c| c.used).copied().unwrap();
    assert_eq!(c.short, 0x0777);
    let answer = [crate::nwk::CMD_REJOIN_RESPONSE, 0x77, 0x07, 0x00];
    let on_air = |s: &Stack<Scripted>| s.hal.sent.iter().filter(|f| f.ends_with(&answer)).count();
    let held = s.indirect.iter().filter(|e| e.used).count();
    assert_eq!((on_air(&s), held), (0, 1), "the rejoin response to a sleepy child went straight on air instead of waiting for its poll");
    s.hal.sent.clear();
    s.on_data_request(crate::mac::Addr::Short(0x5000));
    assert_eq!(on_air(&s), 1, "the poll from the old address takes the held response");
    assert_eq!(&s.hal.sent[0][5..7], &[0x00, 0x50]);
}
