use std::collections::BTreeMap;

use scenarios::image::Image;
use scenarios::session::{Session, SessionOpts};
use scenarios::station::Tx;

fn count(log: &[(String, i64, i64)], from: usize) -> (BTreeMap<i64, (u64, u64, u64)>, u64, u64) {
    let mut by: BTreeMap<i64, (u64, u64, u64)> = BTreeMap::new();
    let (mut er, mut pr) = (0, 0);
    for (op, addr, len) in &log[from..] {
        let e = by.entry(addr & !0xFFF).or_default();
        match op.as_str() {
            "erase" | "chip erase" => {
                e.0 += 1;
                er += 1
            }
            "program" => {
                e.1 += 1;
                e.2 += *len as u64;
                pr += 1
            }
            _ => {}
        }
    }
    (by, er, pr)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let img = Image::parse(&args[0]).unwrap();
    let cpi: i64 = args.get(1).map(|s| s.parse().unwrap()).unwrap_or(24);
    let mut s = Session::new(&img, SessionOpts { cpi, ..Default::default() }).expect("session");
    let ops = |s: &Session| -> Vec<(String, i64, i64)> { s.w.air.dut.m.flash.log.iter().map(|o| (o.op.clone(), o.addr, o.len)).collect() };
    let join_ops = s.w.air.dut.m.flash.log.len();
    let (by, er, pr) = count(&ops(&s), 0);
    println!("== {} (cpi {cpi})", img.label);
    println!("boot + join ({:.1} s): {er} erases, {pr} page programs", s.now() / 1000.0);
    for (a, (e, p, b)) in &by {
        println!("   {a:#07x}: {e} erases, {p} programs, {b} B");
    }
    let mut cmds = 0;
    let mut cycles = 0;
    let t0 = s.now();
    let mut k = 0u32;
    while s.now() - t0 < 3_600_000.0 {
        k += 1;
        if k % 30 == 0 {
            s.reboot().unwrap();
            cycles += 1;
            s.run(5000.0).unwrap();
            continue;
        }
        match k % 3 {
            0 => {
                s.tx_zcl(0x0006, 0x02, &[], Tx::new());
            }
            1 => {
                let lvl = 20 + (k * 37 % 230) as u8;
                s.tx_zcl(0x0008, 0x04, &[lvl, 5, 0], Tx::new());
            }
            _ => {
                s.tx_zcl(0x0006, 0x01, &[], Tx::new());
            }
        }
        cmds += 1;
        s.run(30_000.0).unwrap();
    }
    let (by, er, pr) = count(&ops(&s), join_ops);
    println!("one hour of use ({cmds} light commands, {cycles} power cycles): {er} erases, {pr} page programs");
    for (a, (e, p, b)) in &by {
        println!("   {a:#07x}: {e} erases, {p} programs, {b} B");
    }
}
