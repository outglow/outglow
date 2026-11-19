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
