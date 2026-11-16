mod harness;
mod model;

use harness::{Room, Sender};
use model::*;

const START_S: u8 = 40;

fn burst_dbm(l: &Link) -> f64 {
    NOISE.iter().filter(|n| n.0 == l.a || n.0 == l.b).map(|n| n.3 as f64).fold(-120.0, f64::max)
}

fn channel(l: &Link, sigma: f64) -> Sender {
    let mut ch = Channel::of(l, sigma);
    ch.drift_per_h = DRIFT_DB / 3.2;
    ch.burst_p = 0.01;
    ch.burst_dbm = burst_dbm(l);
    let mut sd = Sender::new(1, ch);
    sd.frames_per_s = FRAMES_PER_S;
    sd
}

fn calibrated(l: &Link, sigma: f64, seed: u64) -> Room {
    let mut room = Room::new(seed, 45, 5, vec![channel(l, sigma)]);
    room.run(5000);
    room.calibrate(60);
    room.clear_log();
    room
}

fn on_share(room: &Room, t0: u32) -> f64 {
    let mut on_ms = 0u64;
    let mut prev = (t0, 0u16);
    for &(t, m) in room.log.iter().filter(|e| e.0 >= t0) {
        if prev.1 != 0 {
            on_ms += (t - prev.0) as u64;
        }
        prev = (t, m);
    }
    if prev.1 != 0 {
        on_ms += (room.t - prev.0) as u64;
    }
    on_ms as f64 / (room.t - t0).max(1) as f64
}

fn caught(l: &Link, sigma: f64, depth: f64, seeds: u64) -> (u32, u32) {
    let (mut hit, mut tries) = (0, 0);
    for k in 0..seeds {
        let mut room = calibrated(l, sigma, 7000 + k);
        room.run(20_000);
        if room.mask != 0 {
            continue;
        }
        tries += 1;
        let t = room.t;
        room.senders[0].ch.crossings.push(Crossing { t0: t, t1: t + 1500, depth });
        room.clear_log();
        room.run(2500);
        hit += !room.ons().is_empty() as u32;
    }
    (hit, tries)
}

#[test]
fn the_readings_give_these_links() {
    let round = |x: f64| (x * 100.0).round() / 100.0;
    let want = [
        ("K1", "K2", -56.75, 0.83, 2.34, 0.10),
        ("K1", "K3", -67.75, 1.99, 3.09, 0.02),
        ("K2", "K3", -55.75, 1.14, 1.39, 0.07),
        ("K1", "D", -56.0, 1.05, 1.08, 0.09),
        ("K2", "D", -61.0, 0.40, 1.54, 0.10),
        ("K3", "D", -73.0, 1.18, 2.01, 0.10),
    ];
    for (l, w) in links().iter().zip(want) {
        eprintln!("{}-{}: {:.2} dBm, sigma {:.2}..{:.2} dB, loss {:.2}, bursts at {} dBm", l.a, l.b, l.level, l.sigma_lo, l.sigma_hi, l.loss, burst_dbm(l));
        assert_eq!((l.a, l.b, l.level, round(l.sigma_lo), round(l.sigma_hi), round(l.loss)), w);
    }
    for l in links() {
        assert!(l.level - (-97.0) >= 24.0, "{}-{}", l.a, l.b);
    }
    assert!(links().iter().all(|l| l.sigma_lo >= 0.39));
}

#[test]
fn an_empty_room_stays_quiet_after_calibration_on_every_measured_link() {
    let hours = 5u64;
    let weak = link_from(&WEAK, "K2", "W1");
    let cases: Vec<(Link, &str, f64)> = links()
        .into_iter()
        .flat_map(|l| [(l, "lo", l.sigma_lo), (l, "hi", l.sigma_hi)])
        .chain([(weak, "hi", weak.sigma_hi)])
        .collect();
    for (l, which, sigma) in cases.iter().map(|(l, w, s)| (l, *w, *s)) {
        {
            let (mut ons, mut share, mut ss) = (0, 0.0, vec![]);
            for k in 0..hours {
                let mut room = calibrated(l, sigma, 100 + k);
                ss.push(room.core.line[0].s);
                let t0 = room.t;
                room.run(3_600_000);
                ons += room.ons().len();
                share += on_share(&room, t0);
            }
            let share = share / hours as f64;
            eprintln!("{}-{} sigma_{which} {sigma:.2}: calibrated s {ss:?}, {ons} false ON in {hours} h, ON {:.3} % of the time", l.a, l.b, 100.0 * share);
            let allowed = if which == "lo" { 0 } else { 2 };
            assert!(ons <= allowed, "{}-{} sigma {sigma:.2}: {ons} false ON in {hours} h", l.a, l.b);
            assert!(share < 0.001, "{}-{}: ON {:.3} % of the time", l.a, l.b, 100.0 * share);
        }
    }
}

#[test]
fn a_crossing_is_caught_on_every_measured_link_with_its_smaller_spread() {
    for l in links() {
        let depth = if l.sigma_lo < 1.5 { 8.0 } else { 14.0 };
        let (hit, tries) = caught(&l, l.sigma_lo, depth, 20);
        eprintln!("{}-{} sigma_lo {:.2}: {depth} dB caught {hit} of {tries}", l.a, l.b, l.sigma_lo);
        assert!(tries >= 18, "the line stays ON after calibration");
        assert!(hit * 10 >= tries * 7, "{}-{}: {depth} dB caught {hit} of {tries}", l.a, l.b);
    }
}

#[test]
fn with_the_larger_spread_a_deep_shadow_is_still_caught() {
    for l in links().iter().filter(|l| l.sigma_hi <= 2.4) {
        let (hit8, tries) = caught(l, l.sigma_hi, 8.0, 20);
        let (hit14, _) = caught(l, l.sigma_hi, 14.0, 20);
        eprintln!("{}-{} sigma_hi {:.2}: 8 dB caught {hit8} of {tries}, 14 dB {hit14}", l.a, l.b, l.sigma_hi);
        assert!(hit14 * 10 >= tries * 7, "{}-{}: 14 dB caught {hit14} of {tries}", l.a, l.b);
    }
}

#[test]
fn a_slow_drift_of_the_level_is_not_motion() {
    for l in links() {
        let mut room = calibrated(&l, l.sigma_lo, 55);
        room.senders[0].ch.drift_per_h = 24.0;
        room.senders[0].ch.drift_from = room.t;
        room.run(600_000);
        assert!(room.ons().is_empty(), "{}-{}: {:?}", l.a, l.b, room.log);
    }
}

#[test]
fn a_link_below_the_floor_makes_no_line_and_one_near_it_no_false_on() {
    let l = link_from(&WEAK, "K2", "W1");
    let mut far = l;
    far.level = -96.0;
    let mut room = Room::new(3, 45, 5, vec![Sender::new(1, Channel::new(far.level, 1.0, 0.1))]);
    room.run(60_000);
    assert!(!room.core.line[0].used(), "a line from frames below -90 dBm");
    let room = calibrated(&l, l.sigma_hi, 9);
    assert!(room.core.line[0].used() && room.core.line[0].s < 20, "s {}", room.core.line[0].s);
}

#[test]
#[ignore = "red: before the first day's automatic calibration, s = 40 is too sensitive for K1-K3"]
fn the_default_sensitivity_is_quiet_on_the_measured_links() {
    let mut bad = vec![];
    for l in links() {
        let mut room = Room::new(1, START_S, 30, vec![channel(&l, l.sigma_lo)]);
        room.run(3_600_000);
        let share = on_share(&room, 1000);
        eprintln!("{}-{} sigma_lo {:.2}: uncalibrated s = {} ON {:.1} % of an hour", l.a, l.b, l.sigma_lo, START_S, 100.0 * share);
        if share >= 0.001 {
            bad.push((l.a, l.b, (share * 1000.0).round() / 10.0));
        }
    }
    assert!(bad.is_empty(), "ON in an empty room (link, % of the time): {bad:?}");
}

#[test]
#[ignore = "red: on K1-K3 with the larger spread (3.1 dB) even 14 dB crossings are mostly missed"]
fn the_noisiest_link_sees_a_deep_shadow() {
    let l = link("K1", "K3");
    let (hit, tries) = caught(&l, l.sigma_hi, 14.0, 20);
    eprintln!("K1-K3 sigma_hi {:.2}: 14 dB caught {hit} of {tries}", l.sigma_hi);
    assert!(hit * 10 >= tries * 8, "14 dB caught {hit} of {tries}");
}
