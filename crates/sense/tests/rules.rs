use memcap as _;

use sense::{Core, Line, F_CHG, F_ON, F_RULE, HOUR_MS, NO_SRC};

struct Bench {
    c: Core,
    t: u32,
}

impl Bench {
    fn new(s: u8) -> Bench {
        let mut c = Core::new(0, s, 30);
        c.start(0);
        Bench { c, t: 0 }
    }

    fn after(&mut self, ms: u32, rssi: i8) {
        self.t += ms;
        self.c.frame(self.t, 1, rssi, Some(0));
    }

    fn samples(&mut self, rssi: &[i8]) {
        for &r in rssi {
            self.after(100, r);
        }
    }

    fn line(&self) -> Line {
        self.c.line[0]
    }

    fn tick(&mut self) -> u16 {
        self.c.tick(self.t)
    }

    fn on_line(s: u8) -> Bench {
        let mut b = Bench::new(s);
        b.samples(&[-60; 5]);
        b.samples(&[-50, -60, -60, -50, -60, -60, -50]);
        b.after(500, -60);
        b.after(100, -60);
        assert!(b.line().on(), "{:?}", b.line());
        b
    }
}

fn day_line(hours: u8) -> Core {
    let mut c = Core::new(0, 50, 30);
    c.start(0);
    assert!(c.line_restore(0, 1, 50, 50, 0));
    c.line[0].day_hours = hours;
    c.line[0].day_cnt = 1000 * hours as u32;
    c.line[0].day_sum = 1000 * hours as u32;
    c.line[0].day_min = 32;
    c.hour = 23;
    c
}

fn end_day(c: &mut Core) {
    let t = c.t_hour.wrapping_add(HOUR_MS);
    c.tick(t);
    assert_eq!(c.hour, 0);
}

#[test]
fn noise_moves_an_eighth_of_the_way_to_eight_times_the_deviation() {
    let mut b = Bench::new(50);
    b.samples(&[-60; 5]);
    let mut trace = vec![];
    for r in [-50, -60, -60, -29, -60, -60, -28, -60, -60, 10, -60, -60, -59] {
        b.after(100, r);
        trace.push(b.line().noise);
    }
    assert_eq!(trace, [10, 8, 7, 37, 32, 28, 56, 49, 42, 68, 59, 51, 45]);
    assert_eq!(&b.line().dev[..13], &[10, 0, 0, 31, 0, 0, 32, 0, 0, 63, 0, 0, 1]);
}

#[test]
fn a_line_turns_on_when_the_rule_holds_for_its_delay_and_reports_it() {
    let mut b = Bench::new(50);
    b.samples(&[-60; 5]);
    b.samples(&[-50, -60, -60, -50, -60, -60]);
    assert_eq!(b.line().flags & F_RULE, 0);
    b.after(100, -50);
    let rule = b.t;
    assert_eq!((b.line().flags & F_RULE, b.line().t_rule), (F_RULE, rule));
    assert!(!b.line().on());
    b.after(499, -60);
    assert!(!b.line().on());
    assert_eq!(b.c.tick(rule + 499), 0);
    assert_eq!(b.c.tick(rule + 500), 1);
    assert!(b.line().on());
}

#[test]
fn the_bit_of_a_line_in_the_on_mask_is_its_index() {
    let mut b = Bench::on_line(50);
    b.c.line.swap(0, 3);
    assert_eq!(b.tick(), 1 << 3);
    assert_eq!(b.c.on_mask, 1 << 3);
}

#[test]
fn blanking_restarts_the_reference_and_keeps_a_line_on() {
    let mut b = Bench::on_line(50);
    let t = b.t;
    b.c.blank(t);
    let l = b.line();
    assert!(l.on());
    assert_eq!(l.flags, F_ON | F_CHG);
    assert_eq!((l.nhist, l.sum, l.blank), (0, 0, true));
}

#[test]
fn start_turns_every_line_off_and_resets_the_day() {
    let mut b = Bench::on_line(50);
    b.c.line[0].day_hours = 5;
    b.c.line[0].day_sum = 7;
    b.c.line[0].day_cnt = 9;
    b.c.line[0].day_min = 3;
    b.c.line[0].hour_cnt = 11;
    b.c.line[0].hour_sum = 13;
    let t = b.t;
    b.c.start(t);
    let l = b.line();
    assert_eq!(l.flags, F_CHG);
    assert_eq!((l.day_hours, l.day_sum, l.day_cnt, l.day_min, l.hour_cnt, l.hour_sum), (0, 0, 0, 0xFFFF, 0, 0));
}

#[test]
fn a_new_sensitivity_drops_the_rule_and_the_day_but_keeps_a_line_on() {
    let mut b = Bench::on_line(50);
    b.c.line[0].day_hours = 5;
    b.c.line[0].hour_cnt = 11;
    b.c.cfg(50, 10);
    assert_eq!(b.line().flags, F_ON | F_CHG | F_RULE);
    assert_eq!((b.line().day_hours, b.c.off_s), (5, 10));
    b.c.cfg(60, 0);
    let l = b.line();
    assert_eq!(l.flags, F_ON | F_CHG);
    assert_eq!((l.s, l.anchor_s, l.anchor_noise, l.day_hours, l.hour_cnt, b.c.off_s), (60, 60, 0, 0, 0, 1));
}

#[test]
fn a_line_is_evicted_only_when_silent_for_more_than_a_minute() {
    let mut c = Core::new(0, 50, 30);
    c.start(0);
    for src in 1..=10u16 {
        for k in 0..6 {
            c.frame(100 * k, src, -60 + k as i8, Some(2));
        }
    }
    assert!(!c.dirty);
    let busy = c.line[0];
    assert_eq!((busy.nhist, busy.flags, busy.peer_chg), (5, F_CHG, 2));
    c.frame(60_500, 11, -60, Some(0));
    assert_eq!(c.line.iter().position(|l| l.src == 11), None);
    c.frame(60_501, 11, -60, Some(0));
    assert!(c.dirty);
    let l = c.line[0];
    assert_eq!(l.src, 11);
    assert_eq!((l.nhist, l.noise, l.flags, l.peer_chg, l.cal_s, l.rssi), (1, 0, F_CHG, 0, 100, -60));
}

#[test]
fn a_line_that_is_on_is_never_evicted() {
    let mut b = Bench::on_line(50);
    for src in 2..=10u16 {
        b.c.frame(b.t, src, -60, Some(0));
    }
    let t = b.t + 600_000;
    b.c.frame(t, 11, -60, Some(0));
    assert_eq!(b.c.line.iter().position(|l| l.src == 11), Some(1));
    b.c.line[1].flags |= F_ON;
    b.c.frame(t + 600_000, 12, -60, Some(0));
    assert_eq!(b.c.line.iter().position(|l| l.src == 12), Some(2));
}

#[test]
fn a_probe_at_minus_90_dbm_counts_one_below_does_not() {
    let mut c = Core::new(0, 50, 30);
    c.start(0);
    c.frame(100, 1, -91, Some(0));
    assert!(!c.line[0].used());
    c.frame(100, 1, -90, Some(0));
    assert_eq!((c.line[0].src, c.line[0].nhist), (1, 1));
    c.frame(200, 1, -91, Some(0));
    assert_eq!(c.line[0].nhist, 1);
    c.frame(300, 1, -90, Some(0));
    assert_eq!((c.line[0].nhist, c.line[0].hist[1]), (2, -90));
}

#[test]
fn the_first_probe_sets_the_change_count_a_later_change_blanks_for_1_5_s() {
    let mut b = Bench::new(50);
    b.c.frame(100, 1, -60, Some(3));
    assert_eq!((b.line().nhist, b.line().blank, b.line().peer_chg), (1, false, 3));
    b.c.frame(200, 1, -60, Some(3));
    assert_eq!(b.line().nhist, 2);
    b.c.frame(300, 1, -60, Some(4));
    assert_eq!((b.line().nhist, b.line().blank, b.line().t_blank), (0, true, 1800));
    b.c.frame(1799, 1, -60, Some(4));
    assert_eq!((b.line().nhist, b.line().blank), (0, true));
    b.c.frame(1800, 1, -60, Some(4));
    assert_eq!((b.line().nhist, b.line().blank), (1, false));
}

#[test]
fn frames_without_a_probe_are_samples_of_a_known_line_and_never_blank() {
    let mut c = Core::new(0, 50, 30);
    c.start(0);
    c.frame(100, 1, -60, None);
    assert!(!c.line[0].used());
    assert!(c.line_restore(100, 1, 50, 50, 0));
    c.frame(200, 1, -60, None);
    c.frame(300, 1, -60, None);
    assert_eq!((c.line[0].nhist, c.line[0].flags), (2, 0));
}

#[test]
fn a_gap_of_exactly_4_s_keeps_the_reference() {
    let mut b = Bench::new(50);
    b.samples(&[-60; 5]);
    b.after(4000, -60);
    assert_eq!(b.line().nhist, 5);
    assert_eq!(b.line().dev[0], 0);
    b.after(4001, -60);
    assert_eq!(b.line().nhist, 1);
}

#[test]
fn the_hourly_count_stops_at_its_maximum() {
    let mut c = Core::new(0, 50, 30);
    c.start(0);
    let mut t = 0u32;
    for _ in 0..0x1_0005u32 {
        t += 100;
        c.frame(t, 1, -60, Some(0));
    }
    assert_eq!(c.line[0].hour_cnt, 0xFFFF);
}

#[test]
fn samples_count_for_the_hour_only_while_nothing_is_on() {
    let mut b = Bench::on_line(50);
    let n = b.line().hour_cnt;
    b.after(100, -60);
    assert_eq!(b.line().hour_cnt, n);
    let t0 = b.t;
    for k in 0..=6 {
        b.c.frame(t0 + 100 * k, 2, -60, Some(0));
    }
    assert_eq!(b.c.line[1].hour_cnt, 2);
    assert_eq!(b.c.tick(t0 + 600), 1);
    for k in 7..=9 {
        b.c.frame(t0 + 100 * k, 2, -60, Some(0));
    }
    assert_eq!(b.c.line[1].hour_cnt, 2);
}

#[test]
fn sensitivity_0_keeps_the_rule_off() {
    let mut b = Bench::on_line(50);
    b.c.line[0].flags |= F_RULE;
    b.c.line[0].s = 0;
    b.after(100, -50);
    assert_eq!((b.line().flags, b.line().sum), (F_ON | F_CHG, 0));
}

#[test]
fn a_calm_window_drops_the_rule() {
    let mut b = Bench::new(50);
    b.samples(&[-60; 5]);
    b.samples(&[-50, -60, -60, -50, -60, -60, -50]);
    assert_eq!(b.line().flags, F_RULE | F_CHG);
    for _ in 0..16 {
        b.after(100, -60);
    }
    assert_eq!(b.line().flags, F_ON | F_CHG);
}

#[test]
fn calibration_stops_where_the_window_with_headroom_is_below_m() {
    let mut b = Bench::new(50);
    b.samples(&[-60; 5]);
    let t = b.t;
    b.c.cal_start(t, 100);
    b.c.auto_cal = true;
    b.samples(&[-59, -60, -60, -59, -60, -60, -59, -60, -60, -59]);
    let l = b.line();
    assert_eq!((l.cal_s, l.hour_cal_s), (49, 49));
}

#[test]
fn calibration_on_a_wild_link_stops_at_sensitivity_1() {
    let mut b = Bench::new(50);
    b.samples(&[-60; 5]);
    let t = b.t;
    b.c.cal_start(t, 100);
    b.c.auto_cal = true;
    for _ in 0..8 {
        b.samples(&[0, -60, -60]);
    }
    let l = b.line();
    assert_eq!((l.cal_s, l.hour_cal_s), (1, 1));
}

#[test]
fn seconds_of_calibration_left_round_up() {
    let mut c = Core::new(0, 50, 30);
    assert_eq!(c.cal_left(0), 0);
    c.cal_start(1000, 10);
    assert_eq!(c.cal_left(1000), 10);
    assert_eq!(c.cal_left(1001), 10);
    assert_eq!(c.cal_left(1999), 10);
    assert_eq!(c.cal_left(2000), 9);
    assert_eq!(c.cal_left(10_999), 1);
    assert_eq!(c.cal_left(11_000), 0);
    assert_eq!(c.cal_left(12_000), 0);
    c.cal = false;
    assert_eq!(c.cal_left(1000), 0);
}

fn calibrate(devs: usize, rssi: i8) -> Core {
    let mut b = Bench::new(50);
    b.samples(&[-60; 5]);
    let t = b.t;
    b.c.cal_start(t, 10);
    for k in 0..devs {
        b.after(100, if k % 3 == 0 { rssi } else { -60 });
    }
    b.c.tick(t + 10_000);
    assert!(!b.c.cal);
    b.c
}

#[test]
fn calibration_needs_40_samples() {
    let c = calibrate(39, -58);
    assert_eq!((c.line[0].s, c.line[0].anchor_noise, c.dirty), (50, 0, false));
    let c = calibrate(40, -58);
    let l = c.line[0];
    assert_eq!((l.cal_n, l.cal_sum), (40, 28));
    assert_eq!((l.s, l.anchor_s, l.anchor_noise, c.dirty), (l.cal_s, l.cal_s, 28 * 32 / 40, true));
    assert_eq!(l.flags & F_RULE, 0);
}

#[test]
fn a_silent_calibration_anchors_noise_1() {
    let c = calibrate(45, -60);
    assert_eq!((c.line[0].cal_sum, c.line[0].anchor_noise), (0, 1));
}

#[test]
fn calibration_keeps_the_light_on_and_drops_the_rule() {
    let mut b = Bench::on_line(50);
    let t = b.t;
    b.c.cal_start(t, 5);
    for _ in 0..45 {
        b.after(100, -60);
    }
    b.c.line[0].flags |= F_RULE;
    b.c.tick(t + 5000);
    assert_eq!((b.c.cal, b.line().flags), (false, F_ON | F_CHG));
}

#[test]
fn an_hour_with_fewer_than_1000_samples_is_not_counted() {
    let mut c = day_line(0);
    c.hour = 0;
    c.line[0].hour_cnt = 999;
    c.line[0].hour_sum = 999;
    end_hour(&mut c);
    assert_eq!((c.line[0].day_hours, c.line[0].day_cnt, c.line[0].hour_cnt), (0, 0, 0));
    c.line[0].hour_cnt = 1000;
    c.line[0].hour_sum = 500;
    end_hour(&mut c);
    let l = c.line[0];
    assert_eq!((l.day_hours, l.day_cnt, l.day_sum, l.day_min, l.hour_cnt, l.hour_sum), (1, 1000, 500, 16, 0, 0));
}

fn end_hour(c: &mut Core) {
    let t = c.t_hour.wrapping_add(HOUR_MS);
    c.tick(t);
}

#[test]
fn the_day_keeps_its_quietest_hour_and_the_highest_hourly_calibration() {
    let mut c = day_line(0);
    c.hour = 0;
    c.auto_cal = true;
    for (sum, cal) in [(600u32, 40u8), (300, 30), (900, 60), (300, 20)] {
        c.line[0].hour_cnt = 1000;
        c.line[0].hour_sum = sum;
        c.line[0].hour_cal_s = cal;
        end_hour(&mut c);
        assert_eq!(c.line[0].hour_cal_s, 100);
    }
    assert_eq!((c.line[0].day_min, c.line[0].day_cal_s, c.line[0].day_hours), (9, 60, 4));
}

#[test]
fn hours_without_auto_calibration_leave_the_days_calibration() {
    let mut c = day_line(0);
    c.hour = 0;
    c.line[0].hour_cnt = 1000;
    c.line[0].hour_cal_s = 40;
    end_hour(&mut c);
    assert_eq!(c.line[0].day_cal_s, 0);
}

#[test]
fn auto_calibration_takes_the_days_sensitivity_and_quietest_hour() {
    let mut c = day_line(12);
    c.auto_cal = true;
    c.line[0].day_cal_s = 37;
    c.line[0].flags = F_ON | F_RULE | F_CHG;
    c.line[0].t_hit = HOUR_MS;
    end_day(&mut c);
    let l = c.line[0];
    assert_eq!((l.s, l.anchor_s, l.anchor_noise, l.flags, c.dirty), (37, 37, 32, F_ON | F_CHG, true));
    assert_eq!((l.day_hours, l.day_cal_s, l.day_min), (0, 0, 0xFFFF));
}

#[test]
fn auto_calibration_of_a_day_without_noise_anchors_noise_1() {
    let mut c = day_line(12);
    c.auto_cal = true;
    c.line[0].day_cal_s = 37;
    c.line[0].day_min = 0;
    end_day(&mut c);
    assert_eq!((c.line[0].s, c.line[0].anchor_noise), (37, 1));
}

#[test]
fn auto_calibration_needs_every_condition() {
    let base = || {
        let mut c = day_line(12);
        c.auto_cal = true;
        c.line[0].day_cal_s = 37;
        c
    };
    let mut c = base();
    c.auto_cal = false;
    end_day(&mut c);
    assert_eq!((c.line[0].s, c.line[0].anchor_noise), (50, 32));
    let mut c = base();
    c.line[0].anchor_noise = 20;
    end_day(&mut c);
    assert_eq!((c.line[0].s, c.line[0].anchor_noise, c.dirty), (50, 20, false));
    let mut c = base();
    c.line[0].day_hours = 11;
    end_day(&mut c);
    assert_eq!((c.line[0].s, c.line[0].anchor_noise, c.dirty), (50, 0, false));
    let mut c = base();
    c.line[0].day_cal_s = 0;
    end_day(&mut c);
    assert_eq!((c.line[0].s, c.line[0].anchor_noise), (50, 32));
    let mut c = base();
    c.line[0].s = 0;
    end_day(&mut c);
    assert_eq!((c.line[0].s, c.line[0].anchor_noise, c.dirty), (0, 0, false));
}

#[test]
fn the_first_full_day_anchors_the_mean_noise() {
    let mut c = day_line(12);
    c.line[0].day_sum = 3000;
    end_day(&mut c);
    assert_eq!((c.line[0].anchor_noise, c.line[0].s, c.dirty), (8, 50, true));
    let mut c = day_line(12);
    c.line[0].day_sum = 0;
    end_day(&mut c);
    assert_eq!(c.line[0].anchor_noise, 1);
}

fn tune(s: u8, anchor_s: u8, anchor_noise: u16, day_min: u16, day_mean: u16) -> (u8, bool) {
    let mut c = day_line(12);
    c.selftune = true;
    c.line[0].s = s;
    c.line[0].anchor_s = anchor_s;
    c.line[0].anchor_noise = anchor_noise;
    c.line[0].day_min = day_min;
    c.line[0].day_cnt = 32_000;
    c.line[0].day_sum = day_mean as u32 * 1000;
    end_day(&mut c);
    (c.line[0].s, c.dirty)
}

#[test]
fn a_day_louder_than_twice_the_anchor_lowers_s_by_a_step() {
    assert_eq!(tune(50, 50, 10, 20, 10), (50, false));
    assert_eq!(tune(50, 50, 10, 21, 10), (45, true));
    assert_eq!(tune(50, 50, 10, 15, 10), (50, false));
    assert_eq!(tune(43, 50, 10, 21, 10), (40, true));
    assert_eq!(tune(40, 50, 10, 21, 10), (40, false));
    assert_eq!(tune(3, 10, 10, 21, 10), (1, true));
    assert_eq!(tune(1, 10, 10, 21, 10), (1, false));
    assert_eq!(tune(5, 5, 10, 21, 10), (1, true));
    assert_eq!(tune(14, 12, 10, 21, 10), (9, true));
}

#[test]
fn a_day_quieter_than_half_the_anchor_raises_s_by_a_step() {
    assert_eq!(tune(50, 50, 20, 30, 10), (50, false));
    assert_eq!(tune(50, 50, 20, 30, 9), (55, true));
    assert_eq!(tune(50, 50, 12, 20, 6), (50, false));
    assert_eq!(tune(57, 50, 20, 30, 9), (60, true));
    assert_eq!(tune(60, 50, 20, 30, 9), (60, false));
    assert_eq!(tune(99, 95, 20, 30, 9), (100, true));
    assert_eq!(tune(100, 95, 20, 30, 9), (100, false));
    assert_eq!(tune(88, 90, 20, 30, 9), (93, true));
}

#[test]
fn without_self_tuning_a_day_moves_nothing() {
    let mut c = day_line(12);
    c.line[0].anchor_noise = 10;
    c.line[0].day_min = 100;
    end_day(&mut c);
    assert_eq!((c.line[0].s, c.dirty), (50, false));
}

#[test]
fn a_rule_without_kept_samples_for_4_s_restarts_at_the_tick() {
    let mut b = Bench::new(50);
    b.samples(&[-60; 5]);
    b.samples(&[-50, -60, -60, -50, -60, -60, -50]);
    let kept = b.line().t_kept;
    b.c.tick(kept + 4000);
    assert_eq!((b.line().flags & F_RULE, b.line().nhist, b.line().t_hit), (F_RULE, 5, kept + 4000));
    b.c.tick(kept + 4001);
    assert_eq!((b.line().flags & F_RULE, b.line().nhist), (0, 0));
}

#[test]
fn a_quiet_line_keeps_its_reference_at_the_tick() {
    let mut b = Bench::new(50);
    b.samples(&[-60; 5]);
    let kept = b.line().t_kept;
    b.c.tick(kept + 10_000);
    assert_eq!(b.line().nhist, 5);
    let mut c = Core::new(0, 50, 30);
    c.start(0);
    assert!(c.line_restore(0, 1, 50, 50, 0));
    for k in 1..=5 {
        c.frame(100 * k, 1, -60, None);
    }
    c.tick(20_000);
    assert_eq!((c.line[0].nhist, c.line[0].flags), (5, 0));
}

#[test]
fn a_line_turns_off_after_off_seconds_without_a_hit() {
    let mut c = Core::new(0, 50, 2);
    c.start(0);
    assert!(c.line_restore(0, 1, 50, 50, 0));
    let mut t = 0;
    for r in [-60, -60, -60, -60, -60, -50, -60, -60, -50, -60, -60, -50, -60, -60, -60, -60, -60, -60] {
        t += 100;
        c.frame(t, 1, r, None);
    }
    assert_eq!(c.tick(t), 1);
    for _ in 0..16 {
        t += 100;
        c.frame(t, 1, -60, None);
    }
    assert_eq!(c.line[0].flags, F_ON);
    let hit = c.line[0].t_hit;
    assert_eq!(c.tick(hit + 1999), 1);
    assert_eq!(c.tick(hit + 2000), 0);
    assert_eq!(c.line[0].flags, 0);
}

#[test]
fn unused_lines_are_skipped_at_the_tick() {
    let mut c = Core::new(0, 50, 30);
    c.start(0);
    c.line[4].flags = F_ON;
    assert_eq!(c.tick(100), 0);
    assert_eq!(c.line[4].src, NO_SRC);
}
