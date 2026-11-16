//! The automatic calibration (`Core::auto_cal`) on the channel measured
//! between the owner's bulbs (`tests/model`): a bulb nobody calibrates
//! takes, after its first full day, the sensitivity of the quietest hour of
//! that day. The day has people in it: a crossing every 8 minutes or so
//! from 7:00 to 23:00, quiet nights. The expectations are those of a
//! calibration by hand (`measured.rs`): an empty corridor stays quiet
//! afterwards, a crossing is seen.
//!
//! `-- --nocapture` prints the numbers.

mod harness;
mod model;

use harness::{Room, Sender};
use model::*;

/// The sensitivity a bulb starts with (crates/zigbee DEFAULT_S).
const START_S: u8 = 40;
const DAY_MS: u32 = 24 * 3_600_000;

fn burst_dbm(l: &Link) -> f64 {
    NOISE.iter().filter(|n| n.0 == l.a || n.0 == l.b).map(|n| n.3 as f64).fold(-120.0, f64::max)
}

/// The link's channel as `measured.rs` has it, with a day of people crossing it.
fn day_with_people(l: &Link, sigma: f64, seed: u64) -> Sender {
    let mut ch = Channel::of(l, sigma);
    ch.drift_per_h = DRIFT_DB / 3.2;
    ch.burst_p = 0.01;
    ch.burst_dbm = burst_dbm(l);
    let mut g = Rng(seed ^ 0x5eed);
    let mut t = 7 * 3_600_000u32;
    while t < 23 * 3_600_000 {
        let depth = 6.0 + 8.0 * g.unit();
        ch.crossings.push(Crossing { t0: 1000 + t, t1: 1000 + t + 1500, depth });
        t += 4 * 60_000 + g.below(8 * 60_000) as u32;
    }
    let mut sd = Sender::new(1, ch);
    sd.frames_per_s = FRAMES_PER_S;
    sd
}

/// A line after its first full day with automatic calibration on.
fn after_a_day(l: &Link, sigma: f64, seed: u64) -> Room {
    let mut room = Room::new(seed, START_S, 5, vec![day_with_people(l, sigma, seed)]);
    room.core.auto_cal = true;
    room.run(DAY_MS + 60_000);
    // the day's crossings are over
    room.senders[0].ch.crossings.clear();
    room.clear_log();
    room
}

#[test]
fn a_day_calibrates_every_measured_link_by_its_quietest_hour() {
    for l in links() {
        let room = after_a_day(&l, l.sigma_lo, 300);
        let line = room.core.line[0];
        eprintln!("{}-{} sigma_lo {:.2}: s {START_S} -> {} (anchor noise {})", l.a, l.b, l.sigma_lo, line.s, line.anchor_noise);
        assert!(line.anchor_noise != 0, "{}-{}: calibrated", l.a, l.b);
        assert_eq!(line.anchor_s, line.s);
        assert!(room.core.dirty || line.s != START_S, "{}-{}: to be stored", l.a, l.b);
    }
}

#[test]
fn after_the_day_the_empty_corridor_stays_quiet() {
    for l in links() {
        let mut room = after_a_day(&l, l.sigma_lo, 400);
        let s = room.core.line[0].s;
        room.run(5 * 3_600_000);
        let ons = room.ons().len();
        eprintln!("{}-{} sigma_lo {:.2}: s {s}, {ons} false ON in 5 h", l.a, l.b, l.sigma_lo);
        assert_eq!(ons, 0, "{}-{}: {ons} false ON in 5 h", l.a, l.b);
    }
}

#[test]
fn after_the_day_a_crossing_is_seen() {
    for l in links() {
        // as with a calibration by hand: the noisiest link needs a deeper shadow
        let depth = if l.sigma_lo < 1.5 { 8.0 } else { 14.0 };
        let mut room = after_a_day(&l, l.sigma_lo, 500);
        let (mut hit, tries) = (0, 10);
        for _ in 0..tries {
            room.run(20_000);
            let t = room.t;
            room.senders[0].ch.crossings.push(Crossing { t0: t, t1: t + 1500, depth });
            room.clear_log();
            room.run(2500);
            hit += !room.ons().is_empty() as u32;
            room.run(10_000);
        }
        eprintln!("{}-{} sigma_lo {:.2}: s {}, {depth} dB caught {hit} of {tries}", l.a, l.b, l.sigma_lo, room.core.line[0].s);
        assert!(hit * 10 >= tries * 7, "{}-{}: {depth} dB caught {hit} of {tries}", l.a, l.b);
    }
}

#[test]
fn a_line_calibrated_by_hand_keeps_its_sensitivity() {
    let l = links()[0];
    let mut room = Room::new(600, START_S, 5, vec![day_with_people(&l, l.sigma_lo, 600)]);
    room.core.auto_cal = true;
    room.run(5000);
    room.calibrate(60);
    let s = room.core.line[0].s;
    room.run(DAY_MS + 60_000);
    assert_eq!(room.core.line[0].s, s, "the day does not overrule the recording");
}

#[test]
fn the_first_day_starts_when_the_sensing_is_switched_on() {
    // the bulb ran 20 hours with the sensing off (the stack neither feeds nor
    // ticks the core), then it is switched on: the day the calibration
    // waits for is the 24 hours from then, not the rest of a day from the
    // start (4 hours, too few: the calibration would have waited a second day)
    let l = links()[0];
    let mut room = Room::new(800, START_S, 5, vec![day_with_people(&l, l.sigma_lo, 800)]);
    room.core.auto_cal = true;
    room.t += 20 * 3_600_000;
    room.core.start(room.t);
    room.run(DAY_MS - 60_000);
    assert_eq!(room.core.line[0].anchor_noise, 0, "not before a full day of sensing");
    room.run(2 * 60_000);
    assert!(room.core.line[0].anchor_noise != 0, "calibrated a day after the switch-on");
    assert!(room.core.dirty, "to be stored");
}

#[test]
fn switching_on_again_keeps_what_the_lines_learnt() {
    let l = links()[0];
    let mut room = Room::new(900, START_S, 5, vec![day_with_people(&l, l.sigma_lo, 900)]);
    room.run(5000);
    room.calibrate(60);
    let line = room.core.line[0];
    assert!(line.anchor_noise != 0);
    // off for an hour, on again
    room.t += 3_600_000;
    room.core.start(room.t);
    let after = room.core.line[0];
    assert_eq!((after.src, after.s, after.anchor_s, after.anchor_noise), (line.src, line.s, line.anchor_s, line.anchor_noise));
    assert!(!after.on() && room.core.on_mask == 0 && !room.core.cal);
    assert_eq!((room.core.hour, room.core.t_hour), (0, room.t));
}

#[test]
fn without_it_the_first_day_only_sets_the_anchor_noise() {
    // what the C firmware does (and the core by default): s stays
    let l = links()[0];
    let mut room = Room::new(700, START_S, 5, vec![day_with_people(&l, l.sigma_lo, 700)]);
    room.run(DAY_MS + 60_000);
    assert_eq!(room.core.line[0].s, START_S);
    assert!(room.core.line[0].anchor_noise != 0);
}
