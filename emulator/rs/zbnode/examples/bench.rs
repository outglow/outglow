use std::time::Instant;

use zbnode::{Node, RunEvent};

fn fnv(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &x in b {
        h ^= x as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let img = std::fs::read(&a[1]).expect("image");
    let shift = a.get(2).map(|s| i64::from_str_radix(s.trim_start_matches("0x"), 16).unwrap()).unwrap_or(0x6a000);
    let ms: f64 = a.get(3).map(|s| s.parse().unwrap()).unwrap_or(20000.0);
    let cpi: i64 = a.get(4).map(|s| s.parse().unwrap()).unwrap_or(3);
    let mut n = Node::new(&img, cpi, shift).expect("node");
    let t0 = Instant::now();
    let (mut tx, mut ble, mut resets) = (0, 0, 0);
    loop {
        match n.step_run(ms) {
            RunEvent::At => break,
            RunEvent::Tx(_) => tx += 1,
            RunEvent::BleTx(_) => ble += 1,
            RunEvent::Reset { .. } => resets += 1,
            RunEvent::Error(e) | RunEvent::Fatal(e) => panic!("{e}"),
            _ => {}
        }
    }
    let dt = t0.elapsed().as_secs_f64();
    let m = &n.m;
    println!(
        "{:.3} s wall for {:.0} ms: cycles={} pc={:#x} r={:x?} sram={:016x} regs={:016x} tx={} ble={} resets={}",
        dt,
        ms,
        m.cycles,
        m.r[15],
        &m.r[..15],
        fnv(&m.sram),
        fnv(&m.regs),
        tx,
        ble,
        resets
    );
}
