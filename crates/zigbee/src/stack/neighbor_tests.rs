use super::Neighbor;

fn fresh(lqi: u8) -> Neighbor {
    let mut n = Neighbor::EMPTY;
    n.used = true;
    n.lqi = lqi;
    n
}

#[test]
fn the_first_sixteen_attempts_give_the_measured_delivery_and_until_then_the_lqi_decides() {
    let mut n = fresh(80);
    for k in 0..15 {
        n.delivery_sample(1, k % 4 != 0);
        assert!(!n.measured());
        assert_eq!(n.link_cost(), 3, "from the LQI");
    }
    n.delivery_sample(1, true);
    assert!(n.measured());
    assert_eq!((n.attempts, n.delivered), (16, 12));
    assert_eq!(n.delivery, 49151, "12 of 16");
    assert_eq!(n.link_cost(), 3);
    let mut out = fresh(200);
    out.out_cost = 5;
    assert_eq!(out.link_cost(), 5, "the cost the neighbour reported when it is the higher");
}

#[test]
fn a_frame_sent_in_several_attempts_counts_each_of_them() {
    let mut n = fresh(200);
    n.delivery_sample(3, true);
    assert_eq!((n.attempts, n.delivered), (3, 1));
    n.delivery_sample(4, false);
    assert_eq!((n.attempts, n.delivered), (7, 1));
    n.delivery_sample(12, true);
    assert_eq!((n.attempts, n.delivered), (16, 1), "no more than sixteen are counted");
    assert_eq!(n.delivery, 4512, "1 of 16, then two failures and a success a hundred and twenty-eighth each");
    assert_eq!(n.link_cost(), 7);
}

#[test]
fn after_that_each_attempt_moves_the_delivery_a_hundred_and_twenty_eighth_of_the_way_rounded_and_never_past_the_ends() {
    let mut n = fresh(200);
    for k in 0..16 {
        n.delivery_sample(1, k % 4 != 0);
    }
    let mut up = n;
    up.delivery_sample(1, true);
    assert_eq!(up.delivery, 49279);
    let mut down = n;
    down.delivery_sample(1, false);
    assert_eq!(down.delivery, 48767);
    let mut full = n;
    for _ in 0..2000 {
        full.delivery_sample(1, true);
    }
    assert_eq!(full.delivery, 65472, "steps under half a hundred and twenty-eighth round to nothing");
    for _ in 0..2000 {
        full.delivery_sample(1, false);
    }
    assert_eq!(full.delivery, 63);
    assert_eq!(full.link_cost(), 7);
}

#[test]
fn the_cost_moves_only_once_the_delivery_is_past_the_margin() {
    let mut n = fresh(200);
    for _ in 0..16 {
        n.delivery_sample(1, true);
    }
    assert_eq!((n.delivery, n.cost), (65535, 1));
    let mut changes = std::vec::Vec::new();
    for k in 1..=60 {
        n.delivery_sample(1, false);
        if changes.last().map_or(1, |c: &(u32, u16, u8)| c.2) != n.cost {
            changes.push((k, n.delivery, n.cost));
        }
    }
    assert_eq!(changes, [(22, 55149, 2), (40, 47889, 4), (60, 40937, 7)], "lost attempts in a row, each cost 6 % past its own band");
    changes.clear();
    for k in 1..=90 {
        n.delivery_sample(1, true);
        if changes.last().map_or(7, |c: &(u32, u16, u8)| c.2) != n.cost {
            changes.push((k, n.delivery, n.cost));
        }
    }
    assert_eq!(changes, [(23, 44998, 4), (75, 51877, 3)], "and back up only as far as 6 % inside the better band allows");
}

fn heard_every(n: &mut Neighbor, gaps: &[u32], idle: bool) {
    let mut now = n.ls_at;
    for &g in gaps {
        now = now.wrapping_add(g);
        n.link_status_heard(now, idle);
    }
}

#[test]
fn the_first_link_status_only_starts_the_count_and_each_period_after_it_is_one_heard() {
    let mut n = fresh(200);
    n.ls_at = 5_000;
    n.link_status_heard(5_000, false);
    assert_eq!((n.ls_seen, n.heard_n), (true, 0));
    heard_every(&mut n, &[15_000; 7], false);
    assert!(!n.heard_known());
    assert_eq!(n.heard_percent(), super::UNKNOWN_HEARD);
    heard_every(&mut n, &[15_000], false);
    assert_eq!((n.heard_n, n.heard_hits, n.heard), (8, 8, 65535));
    assert_eq!(n.heard_percent(), 100);
}

#[test]
fn a_gap_of_several_periods_counts_the_missing_ones_and_a_short_one_counts_nothing() {
    let mut n = fresh(200);
    n.link_status_heard(0, false);
    heard_every(&mut n, &[45_000, 3_000, 15_000, 30_000, 15_000, 15_000], false);
    assert_eq!((n.heard_n, n.heard_hits, n.heard), (8, 5, 40959), "two lost, one early ignored, one lost");
    assert_eq!(n.heard_percent(), 62, "62.5 % rounded to the nearest");
    heard_every(&mut n, &[15_000], false);
    let up = toward_n(40959, true, 1);
    assert_eq!(n.heard, up, "a sixteenth of the way to all");
    heard_every(&mut n, &[30_000], false);
    assert_eq!(n.heard, toward_n(toward_n(up, false, 1), true, 1), "one lost, then one heard");
}

#[test]
fn no_more_than_sixteen_periods_are_counted_lost_at_once() {
    for periods in [17u32, 40] {
        let mut n = fresh(200);
        n.link_status_heard(0, false);
        heard_every(&mut n, &[15_000 * periods], false);
        assert_eq!((n.heard_n, n.heard_hits, n.heard), (8, 0, toward_n(0, true, 1)), "fifteen lost and one heard after {periods}");
    }
}

fn toward_n(p: u16, got: bool, n: u32) -> u16 {
    (0..n).fold(p, |p, _| super::toward(p, got, 4))
}

#[test]
fn the_period_is_the_shortest_gap_in_the_last_two_windows_of_sixteen_and_never_longer_than_ours() {
    let mut n = fresh(200);
    assert_eq!(n.link_status_period(), crate::stack::LINK_STATUS_MS, "the period of the specification until a gap is seen");
    n.link_status_heard(0, false);
    heard_every(&mut n, &[30_000; 4], false);
    assert_eq!(n.link_status_period(), crate::stack::LINK_STATUS_MS, "a longer gap is lost link status");
    assert_eq!((n.heard_n, n.heard_hits), (8, 4));
    heard_every(&mut n, &[super::LS_MIN_GAP_MS - 1], false);
    assert_eq!(n.link_status_period(), crate::stack::LINK_STATUS_MS, "an early one teaches nothing");
    heard_every(&mut n, &[super::LS_MIN_GAP_MS], false);
    assert_eq!(n.link_status_period(), super::LS_MIN_GAP_MS);
    let mut m = fresh(200);
    m.link_status_heard(0, false);
    heard_every(&mut m, &[12_000; 16], false);
    assert_eq!((m.ls_cycles, m.ls_gaps), (0, [0, 12_000]), "a window ends after sixteen");
    heard_every(&mut m, &[14_000; 16], false);
    assert_eq!((m.ls_gaps, m.link_status_period()), ([0, 14_000], 14_000), "the shorter gap of a past window is forgotten");
    heard_every(&mut m, &[15_000], false);
    assert_eq!(m.link_status_period(), 14_000);
    assert_eq!((m.heard_n, m.heard_hits), (8, 8), "none of them lost");
}

#[test]
fn an_idle_measured_link_takes_its_cost_from_the_link_status_heard() {
    let measured = || {
        let mut n = fresh(200);
        n.delivery_sample(16, false);
        n.link_status_heard(0, false);
        n
    };
    let mut n = measured();
    assert_eq!(n.link_cost(), 7);
    heard_every(&mut n, &[15_000; 8], true);
    assert_eq!((n.delivery, n.link_cost()), (65535, 1), "eight of eight heard");
    let mut busy = measured();
    heard_every(&mut busy, &[15_000; 8], false);
    assert_eq!((busy.delivery, busy.link_cost()), (0, 7), "a link in use keeps its acknowledgements");
    let mut early = measured();
    heard_every(&mut early, &[15_000; 7], true);
    assert_eq!(early.link_cost(), 7, "not before eight link status");
    let mut new = fresh(200);
    new.link_status_heard(0, true);
    heard_every(&mut new, &[15_000; 8], true);
    assert!(!new.measured() && new.delivery == 0, "a link never used keeps its first guess");
}

#[test]
fn a_gap_is_rounded_to_the_nearest_number_of_periods() {
    let mut n = fresh(200);
    n.link_status_heard(0, false);
    heard_every(&mut n, &[29_000, 8_000, 21_000], false);
    assert_eq!((n.heard_n, n.heard_hits), (4, 3), "29 s is one lost and one heard, 8 s one heard, 21 s one heard");
    heard_every(&mut n, &[7_000], false);
    assert_eq!(n.heard_n, 4, "under half a period counts nothing");
}
