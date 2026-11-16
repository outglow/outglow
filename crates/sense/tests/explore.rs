mod harness;
mod model;

use harness::{Room, Sender};
use model::*;

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
    on_ms as f64 / (room.t - t0) as f64
}

#[test]
#[ignore]
fn explore() {
    for r in READINGS.iter().chain(WEAK.iter()) {
        println!("{:?} sigma {:.2} k {:.2}", r, r.sigma(), expected_min_sigmas(r.frames));
    }
    for l in links() {
        println!("{} - {}: level {:.1} sigma {:.2}..{:.2} loss {:.2}", l.a, l.b, l.level, l.sigma_lo, l.sigma_hi, l.loss);
    }
    println!("headroom {} margin {}", sense::CAL_HEADROOM, sense::CAL_MARGIN);
    let weak = link_from(&WEAK, "K2", "W1");
    for l in links().iter().chain([weak].iter()) {
        for (name, sigma) in [("lo", l.sigma_lo), ("hi", l.sigma_hi)] {
            let (mut fo, mut fh) = (0, 0.0);
            let mut det = [0u32; 4];
            let mut ss = vec![];
            let n = 10u64;
            for k in 0..n {
                let mut ch = Channel::of(l, sigma);
                ch.drift_per_h = DRIFT_DB / 3.2;
                ch.burst_p = 0.01;
                ch.burst_dbm = -71.0;
                let mut sd = Sender::new(1, ch);
                sd.frames_per_s = FRAMES_PER_S;
                let mut room = Room::new(500 + k, 45, 5, vec![sd]);
                room.run(5000);
                room.calibrate(60);
                ss.push(room.core.line[0].s);
                room.clear_log();
                let t0 = room.t;
                room.run(3_600_000);
                fo += room.ons().len();
                fh += on_share(&room, t0);
                for (j, depth) in [6.0, 8.0, 10.0, 14.0].iter().enumerate() {
                    room.run(20_000);
                    if room.mask != 0 {
                        continue;
                    }
                    let t = room.t;
                    room.senders[0].ch.crossings.push(Crossing { t0: t, t1: t + 1500, depth: *depth });
                    room.clear_log();
                    room.run(2500);
                    det[j] += !room.ons().is_empty() as u32;
                }
            }
            println!(
                "{}-{} {:.1} dBm sigma_{name} {sigma:.2} loss {:.2}: s {ss:?} false ON {fo} in {n} h ({:.3} %), detected 6/8/10/14 dB {det:?} of {n}",
                l.a, l.b, l.level, l.loss, 100.0 * fh / n as f64
            );
        }
    }
    for sigma in [0.4, 0.8, 1.1, 1.4, 2.0, 2.5, 3.1] {
        for s in [45u8, 40, 35] {
            let mut room = Room::new(1, s, 30, vec![Sender::new(1, Channel::new(-57.0, sigma, 0.05))]);
            room.run(3_600_000);
            print!("sigma {sigma}: s={s} uncalibrated ON {:.1} %/h ({} ONs)  ", 100.0 * on_share(&room, 1000), room.ons().len());
        }
        println!();
        let (mut fo, mut fh) = (0, 0.0);
        let mut ss = vec![];
        let mut det = [0u32; 5];
        let mut tot = 0;
        for k in 0..20u64 {
            let mut room = Room::new(100 + k, 45, 5, vec![Sender::new(1, Channel::new(-57.0, sigma, 0.05))]);
            room.run(5000);
            room.calibrate(60);
            ss.push(room.core.line[0].s);
            room.clear_log();
            let t0 = room.t;
            room.run(3_600_000);
            fo += room.ons().len();
            fh += on_share(&room, t0);
            for (j, depth) in [4.0, 6.0, 8.0, 10.0, 14.0].iter().enumerate() {
                room.run(20_000);
                if room.mask != 0 {
                    continue;
                }
                let t = room.t;
                room.senders[0].ch.crossings.push(Crossing { t0: t, t1: t + 1500, depth: *depth });
                room.clear_log();
                room.run(2500);
                det[j] += !room.ons().is_empty() as u32;
            }
            tot += 1;
        }
        println!("   calibrated (60 s) s {ss:?}\n   false ON in {tot} h: {fo} ({:.2} % of the time); detected 4/6/8/10/14 dB {det:?} of {tot}", 100.0 * fh / tot as f64);
    }
}
