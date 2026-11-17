use super::{BlackBox, RowNow, Totals, BOX_LEN, BOX_PAGES, LINKS_AT, LINK_ENTRY, ROWS_AT};
use crate::stack::NEIGHBORS;
extern crate std;
use std::vec::Vec;

fn read(b: &mut BlackBox, now: u32, shorts: &[u16; NEIGHBORS]) -> Vec<u8> {
    let mut all = Vec::new();
    for p in 0..BOX_PAGES as u16 {
        let mut out = [0u8; 64];
        let n = b.page(p, now, shorts, &mut out);
        all.extend_from_slice(&out[..n]);
    }
    assert_eq!(all.len(), BOX_LEN);
    all
}

#[test]
fn the_box_is_held_until_the_hold_ends() {
    let mut b = BlackBox::EMPTY;
    b.hold_until = Some(1_000);
    assert!(b.held(999));
    assert!(!b.held(1_000));
    assert_eq!(b.hold_until, None);
}

#[test]
fn a_links_frames_are_read_back_oldest_first_two_to_a_byte_after_its_address_and_count() {
    let mut b = BlackBox::EMPTY;
    for (attempts, ok) in [(1, true), (2, false), (3, true), (4, false), (5, true)] {
        b.link_frame(0, 2, attempts, ok);
    }
    let mut shorts = [0u16; NEIGHBORS];
    shorts[2] = 0x1234;
    let all = read(&mut b, 0, &shorts);
    let e = &all[LINKS_AT + 2 * LINK_ENTRY..LINKS_AT + 3 * LINK_ENTRY];
    assert_eq!(e[..7], [0x34, 0x12, 5, 0, 0x29, 0x4B, 0x0D]);
    assert!(e[7..].iter().all(|x| *x == 0));
    let mut b = BlackBox::EMPTY;
    for k in 0..66u8 {
        b.link_frame(0, 1, k % 8, k % 3 == 0);
    }
    let all = read(&mut b, 0, &shorts);
    let e = &all[LINKS_AT + LINK_ENTRY..LINKS_AT + 2 * LINK_ENTRY];
    assert_eq!(e[2], 64, "the last 64");
    let v = |k: u8| (k % 8).min(7) | ((k % 3 == 0) as u8) << 3;
    assert_eq!(e[4], v(2) | v(3) << 4, "from the third");
    assert_eq!(e[35], v(64) | v(65) << 4);
    b.clear_link(1);
    let all = read(&mut b, 0, &shorts);
    assert_eq!(all[LINKS_AT + LINK_ENTRY + 2], 0, "cleared");
}

#[test]
fn a_row_keeps_the_cost_to_the_coordinator_in_its_low_three_bits() {
    let mut b = BlackBox::EMPTY;
    let row = |now: u32, tx: u32| RowNow {
        now,
        totals: Totals { tx, rx: 0, retries: 0, fails: 0, route_disc: 0 },
        coord_hop: 0x1000,
        coord_cost: 2,
        coord_measured: true,
        coord_delivery: 200,
        stale: 1,
        noise: -90,
    };
    b.row(row(0, 10));
    b.row(row(120_000, 17));
    let all = read(&mut b, 120_000, &[0; NEIGHBORS]);
    let r = &all[ROWS_AT..ROWS_AT + 12];
    assert_eq!(r[..4], [2, 0, 7, 0]);
    assert_eq!(r[8], 2 | 1 << 3);
    assert_eq!((r[9], r[10], r[11]), (200, 1, -90i8 as u8));
}

#[test]
fn reading_the_first_page_holds_the_box_until_the_last_page_is_read() {
    let mut b = BlackBox::EMPTY;
    let mut out = [0u8; 64];
    b.page(0, 1_000, &[0; NEIGHBORS], &mut out);
    assert_eq!(b.hold_until, Some(1_000 + super::HOLD_MS));
    b.page(1, 1_000, &[0; NEIGHBORS], &mut out);
    assert!(b.hold_until.is_some());
    let n = b.page(BOX_PAGES as u16 - 1, 1_000, &[0; NEIGHBORS], &mut out);
    assert_eq!(n, BOX_LEN - (BOX_PAGES - 1) * 64);
    assert_eq!(b.hold_until, None);
}

#[test]
fn a_link_history_one_past_full_reads_back_the_last_64_with_the_oldest_kept_intact() {
    let mut b = BlackBox::EMPTY;
    for k in 0..65u8 {
        b.link_frame(0, 0, k % 8, k % 2 == 0);
    }
    let all = read(&mut b, 0, &[0; NEIGHBORS]);
    let e = &all[LINKS_AT..LINKS_AT + LINK_ENTRY];
    let v = |k: u8| (k % 8).min(7) | ((k % 2 == 0) as u8) << 3;
    assert_eq!(e[2], 64);
    assert_eq!(e[4], v(1) | v(2) << 4, "from the second frame");
    assert_eq!(e[35], v(63) | v(64) << 4);
}

#[test]
fn frames_are_read_back_oldest_first() {
    let mut b = BlackBox::EMPTY;
    for k in 0..3u8 {
        b.frame(1_000 + k as u32, 0x1000, 0x2000, 0x0042, k, 1, 0, 1, 0);
    }
    let all = read(&mut b, 2_000, &[0; NEIGHBORS]);
    let ring = &all[super::RING_AT..super::RING_AT + 3 * super::RING_ENTRY];
    let seqs: Vec<u8> = ring.chunks(super::RING_ENTRY).map(|e| e[13]).collect();
    assert_eq!(seqs, [0, 1, 2]);
    assert_eq!(all[4], 3, "three in the ring");
}
