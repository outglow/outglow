mod harness;
mod model;

use harness::{Room, Sender};
use model::{Channel, Crossing};
use sense::{params, Core, HIST, LINES, NO_SRC};

fn calm(src: u16) -> Sender {
    Sender::new(src, Channel::new(-60.0, 0.3, 0.0))
}

#[test]
fn the_sensitivity_table() {
    assert_eq!(params(100), (4, 1), "s 100: 1 dB, M = 1");
    assert_eq!(params(50), (4, 8), "s 50: 1 dB, M = 8");
    assert_eq!(params(49), (5, 8), "s 49: 1.25 dB, M = 8");
    assert_eq!(params(1), (40, 8), "s 1: 10 dB, M = 8");
    assert_eq!(params(45), (8, 8), "the default 45: 2 dB");
    assert_eq!(params(200), params(100), "above 100 is 100");
    for s in 1..100u8 {
        let (a, b) = (params(s), params(s + 1));
        assert!(a.0 as u32 * 16 + a.1 as u32 >= b.0 as u32 * 16 + b.1 as u32, "s {s}");
    }
}

#[test]
fn an_empty_room_stays_off_and_a_crossing_turns_the_line_on_then_off() {
    let mut r = Room::new(1, 50, 30, vec![calm(0x1234)]);
    r.run(600_000);
    assert!(r.log.is_empty(), "10 minutes of a calm link: no ON ({:?})", r.log);
    let l = r.core.line[0];
    assert_eq!((l.src, l.nhist as usize), (0x1234, HIST), "the probes made the line");

    let t = r.t + 5000;
    r.senders[0].ch.crossings.push(Crossing { t0: t, t1: t + 1500, depth: 6.0 });
    r.run(60_000);
    let ons = r.ons();
    let off = r.log.iter().find(|e| e.1 == 0).map(|e| e.0);
    assert!(ons.first().is_some_and(|on| on - t <= 2000), "ON within 2 s of the crossing: {ons:?}");
    let off = off.expect("OFF again");
    assert!((30_000..=36_000).contains(&(off - t)), "OFF 30 s after the last hit: {} ms", off - t);
}

#[test]
fn a_change_of_the_senders_light_is_not_motion() {
    let mut r = Room::new(1, 50, 30, vec![calm(7)]);
    r.run(20_000);
    r.senders[0].ch.offset = -5.0;
    r.senders[0].chg = 1;
    r.run(20_000);
    assert!(r.log.is_empty(), "{:?}", r.log);
    r.senders[0].ch.offset = 0.0;
    r.run(20_000);
    assert!(!r.ons().is_empty());
}

#[test]
fn a_change_of_the_own_light_blanks_every_line() {
    let mut r = Room::new(1, 50, 30, vec![calm(1), calm(2)]);
    r.run(20_000);
    for s in r.senders.iter_mut() {
        s.ch.offset = 4.0;
    }
    let t = r.t;
    r.core.blank(t);
    r.run(20_000);
    assert!(r.log.is_empty(), "{:?}", r.log);
}

#[test]
fn at_most_12_5_samples_a_second_are_kept() {
    let mut c = Core::new(1000, 50, 30);
    let (mut kept, mut last) = (0, None);
    for i in 0..1000u32 {
        c.frame(1000 + i * 10, 9, -60, Some(0));
        let k = c.line[0].t_kept;
        if Some(k) != last {
            kept += 1;
            last = Some(k);
        }
    }
    assert!((120..=126).contains(&kept), "100 frames/s for 10 s: {kept} kept");
}

#[test]
fn only_a_probe_above_the_floor_makes_a_line() {
    let mut c = Core::new(1000, 50, 30);
    c.frame(1000, 0x11, -95, Some(0));
    c.frame(1000, 0x22, -50, None);
    assert!(c.line.iter().all(|l| l.src == NO_SRC), "a probe below -90 dBm and an ordinary frame from a stranger");
    c.frame(1000, 0x22, -50, Some(0));
    assert_eq!(c.line[0].src, 0x22);
    c.frame(1100, 0x22, -51, None);
    assert_eq!((c.line[0].rssi, c.line[0].nhist), (-51, 2));
}

#[test]
fn an_eleventh_sender_takes_the_line_of_one_silent_for_a_minute() {
    let mut c = Core::new(1000, 50, 30);
    for i in 0..LINES as u16 {
        c.frame(1000, 0x100 + i, -60, Some(0));
    }
    c.frame(1000, 0x200, -60, Some(0));
    assert!((0..LINES).all(|i| c.line[i].src == 0x100 + i as u16), "all lines taken, the eleventh waits");
    let t = 62_000;
    for i in 1..LINES as u16 {
        c.frame(t, 0x100 + i, -60, Some(0));
    }
    c.frame(t, 0x200, -60, Some(0));
    assert_eq!(c.line[0].src, 0x200);
    assert!(c.dirty, "a remembered line gave way: stored again");
}

#[test]
fn a_gap_of_more_than_4_s_restarts_the_reference() {
    let mut r = Room::new(1, 50, 30, vec![calm(5)]);
    r.run(10_000);
    r.t += 6000;
    r.senders[0].ch.offset = -8.0;
    r.run(10_000);
    assert!(r.log.is_empty(), "back at another level after 6 s: {:?}", r.log);
}

#[test]
fn sensitivity_0_turns_detection_off() {
    let mut r = Room::new(1, 0, 30, vec![calm(5)]);
    let t = r.t;
    r.senders[0].ch.crossings.push(Crossing { t0: t + 5000, t1: t + 8000, depth: 10.0 });
    r.run(20_000);
    assert!(r.log.is_empty());
}

#[test]
fn calibration_finds_a_quiet_sensitivity_that_still_sees_a_crossing() {
    for sigma in [0.3, 0.6, 1.0, 1.5] {
        let mut r = Room::new(21, 50, 5, vec![Sender::new(1, Channel::new(-60.0, sigma, 0.0))]);
        r.run(5000);
        r.calibrate(60);
        let l = r.core.line[0];
        assert!(r.core.dirty, "the new s is to be stored");
        assert_eq!(l.s, l.anchor_s, "the result is the self-tuning anchor");
        assert!(l.anchor_noise > 0, "with the noise of the recording");
        r.clear_log();
        r.run(3_600_000);
        assert!(r.ons().is_empty(), "noise {sigma} dB, s {}: an hour without a false ON ({:?})", l.s, r.log);

        let need = if sigma >= 1.5 { 7 } else { 8 };
        let (mut caught, mut tries) = (0, 0);
        for k in 0..40 {
            let mut r = Room::new(300 + k, 50, 5, vec![Sender::new(1, Channel::new(-60.0, sigma, 0.0))]);
            r.run(5000);
            r.calibrate(60);
            r.run(5000);
            if r.mask != 0 {
                continue;
            }
            tries += 1;
            let t = r.t;
            r.senders[0].ch.crossings.push(Crossing { t0: t, t1: t + 1500, depth: 8.0 });
            r.run(2500);
            caught += (r.mask != 0) as u32;
        }
        assert!(tries >= 36 && caught * 10 >= tries * need, "noise {sigma} dB: {caught} of {tries} crossings");
    }
}

#[test]
fn calibration_leaves_a_line_without_samples_and_can_be_aborted() {
    let mut r = Room::new(1, 50, 30, vec![Sender::new(1, Channel::new(-60.0, 1.0, 0.0)), Sender::new(2, Channel::new(-60.0, 1.0, 0.0))]);
    r.run(5000);
    let quiet = r.senders.pop().unwrap();
    r.calibrate(60);
    assert!(r.core.line[0].s < 50);
    assert_eq!(r.core.line[1].s, 50, "a line without 40 samples keeps its s");
    r.senders.push(quiet);

    let mut r = Room::new(1, 50, 30, vec![Sender::new(1, Channel::new(-60.0, 1.0, 0.0))]);
    r.run(5000);
    let t = r.t;
    r.core.cal_start(t, 30);
    r.run(5000);
    let left = r.core.cal_left(r.t);
    assert!((24..=26).contains(&left), "counting down: {left} s");
    let t = r.t;
    r.core.cal_start(t, 0);
    assert_eq!(r.core.cal_left(t), 0);
    r.run(40_000);
    assert_eq!((r.core.line[0].s, r.core.cal), (50, false), "aborted: nothing changed");
}

fn days(r: &mut Room, n: usize, mut sigma_at: impl FnMut(usize, usize) -> Option<f64>) -> Vec<u8> {
    let mut out = vec![];
    r.step = 100;
    for d in 0..n {
        for h in 0..24 {
            if let Some(s) = sigma_at(d, h) {
                r.senders[0].ch.sigma = s;
            }
            r.run(3_600_000);
            r.tick();
        }
        out.push(r.core.line[0].s);
    }
    out
}

#[test]
fn self_tuning_moves_s_one_level_a_day_and_at_most_two_from_the_anchor() {
    let mut r = Room::new(31, 40, 5, vec![Sender::new(1, Channel::new(-60.0, 0.8, 0.0))]);
    r.core.selftune = true;
    assert_eq!(days(&mut r, 2, |_, _| None), [40, 40], "constant noise");
    assert!(r.core.line[0].anchor_noise > 0, "the first day set the anchor noise");
    assert_eq!(days(&mut r, 4, |_, _| Some(2.4)), [35, 30, 30, 30], "three times the noise: down by 5 a day, by 10 at most");
    assert_eq!(days(&mut r, 5, |_, _| Some(0.15)), [35, 40, 45, 50, 50], "under half: up by 5 a day, to the anchor + 10 at most");
}

#[test]
fn a_few_loud_hours_a_day_do_not_move_s() {
    let mut r = Room::new(32, 40, 5, vec![Sender::new(1, Channel::new(-60.0, 0.8, 0.0))]);
    r.core.selftune = true;
    days(&mut r, 1, |_, _| None);
    let busy = days(&mut r, 3, |_, h| Some(if (18..23).contains(&h) { 3.0 } else { 0.8 }));
    assert_eq!(busy, [40, 40, 40]);
}

#[test]
fn without_self_tuning_s_stays() {
    let mut r = Room::new(33, 40, 5, vec![Sender::new(1, Channel::new(-60.0, 0.8, 0.0))]);
    days(&mut r, 1, |_, _| None);
    assert_eq!(days(&mut r, 2, |_, _| Some(2.4)), [40, 40]);
}

#[test]
fn a_line_from_nv_keeps_its_s_and_a_new_default_overrides_every_line() {
    let mut r = Room::new(1, 45, 30, vec![calm(0x77)]);
    let t = r.t;
    assert!(r.core.line_restore(t, 0x77, 38, 40, 25));
    assert!(!r.core.line_restore(t, 0x77, 1, 1, 1), "once");
    r.run(5000);
    let l = r.core.line[0];
    assert_eq!((l.src, l.s, l.anchor_s, l.anchor_noise, l.nhist as usize), (0x77, 38, 40, 25, HIST));
    r.core.cfg(60, 30);
    let l = r.core.line[0];
    assert_eq!((l.s, l.anchor_s, l.anchor_noise), (60, 60, 0));
    r.core.cfg(60, 10);
    assert_eq!((r.core.line[0].s, r.core.off_s), (60, 10));
}
