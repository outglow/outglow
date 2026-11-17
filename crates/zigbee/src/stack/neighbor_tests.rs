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
    assert_eq!(n.delivery, 7470, "1 of 16, then two failures and a success a sixteenth each");
    assert_eq!(n.link_cost(), 7);
}

#[test]
fn after_that_each_attempt_moves_the_delivery_a_sixteenth_of_the_way_rounded_and_never_past_the_ends() {
    let mut n = fresh(200);
    for k in 0..16 {
        n.delivery_sample(1, k % 4 != 0);
    }
    let mut up = n;
    up.delivery_sample(1, true);
    assert_eq!(up.delivery, 50175);
    let mut down = n;
    down.delivery_sample(1, false);
    assert_eq!(down.delivery, 46079);
    let mut full = n;
    for _ in 0..400 {
        full.delivery_sample(1, true);
    }
    assert_eq!(full.delivery, 65528, "steps under half a sixteenth round to nothing");
    for _ in 0..400 {
        full.delivery_sample(1, false);
    }
    assert_eq!(full.delivery, 7);
    assert_eq!(full.link_cost(), 7);
}

#[test]
fn the_cost_moves_only_once_the_delivery_is_past_the_margin() {
    let mut n = fresh(200);
    for _ in 0..16 {
        n.delivery_sample(1, true);
    }
    assert_eq!((n.delivery, n.cost), (65535, 1));
    let falling = [
        (61439, 1), (57599, 1), (53999, 1), (50624, 3), (47460, 3), (44494, 3), (41713, 3), (39106, 7), (36662, 7), (34371, 7),
        (32223, 7), (30209, 7), (28321, 7), (26551, 7),
    ];
    for (p, cost) in falling {
        n.delivery_sample(1, false);
        assert_eq!((n.delivery, n.cost), (p, cost));
    }
    let rising = [
        (28988, 7), (31272, 7), (33413, 7), (35421, 7), (37303, 7), (39068, 7), (40722, 7), (42273, 7), (43727, 7), (45090, 7),
        (46368, 7), (47566, 7), (48689, 7), (49742, 3), (50729, 3), (51654, 3), (52522, 3), (53335, 3), (54098, 3), (54813, 3),
        (55483, 3), (56111, 3), (56700, 3), (57252, 3), (57770, 3), (58255, 3),
    ];
    for (p, cost) in rising {
        n.delivery_sample(1, true);
        assert_eq!((n.delivery, n.cost), (p, cost));
    }
}
