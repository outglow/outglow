// steptrace: the Rust twin of emulator/go/cmd/steptrace (same flags, same
// output lines), for the differential check of the tc32emu port.
use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

use tc32emu::{Error, Flash, Machine, Range};

fn fnv(mut h: u64, b: &[u8]) -> u64 {
    for &x in b {
        h ^= x as u64;
        h = h.wrapping_mul(1099511628211);
    }
    h
}

const FNV_BASIS: u64 = 14695981039346656037;

struct Opts {
    cpi: i64,
    link: String,
    mode: String,
    n: i64,
    ms: f64,
    every: i64,
    from: i64,
    idle: String,
    events: bool,
    max_resets: i32,
    image: String,
}

fn parse_args() -> Opts {
    let mut o = Opts {
        cpi: 3,
        link: "0".into(),
        mode: "step".into(),
        n: 1000000,
        ms: 1000.0,
        every: 10000,
        from: 0,
        idle: String::new(),
        events: false,
        max_resets: 20,
        image: String::new(),
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].trim_start_matches('-').to_string();
        if !args[i].starts_with('-') {
            o.image = args[i].clone();
            i += 1;
            continue;
        }
        let (name, inline) = match a.find('=') {
            Some(p) => (a[..p].to_string(), Some(a[p + 1..].to_string())),
            None => (a.clone(), None),
        };
        if name == "events" {
            o.events = inline.map(|v| v == "true" || v == "1").unwrap_or(true);
            i += 1;
            continue;
        }
        let val = match inline {
            Some(v) => v,
            None => {
                i += 1;
                args[i].clone()
            }
        };
        match name.as_str() {
            "cpi" => o.cpi = val.parse().unwrap(),
            "link" => o.link = val,
            "mode" => o.mode = val,
            "n" => o.n = val.parse().unwrap(),
            "ms" => o.ms = val.parse().unwrap(),
            "every" => o.every = val.parse().unwrap(),
            "from" => o.from = val.parse().unwrap(),
            "idle" => o.idle = val,
            "resets" => o.max_resets = val.parse().unwrap(),
            _ => panic!("unknown flag {}", name),
        }
        i += 1;
    }
    o
}

fn digest(m: &Machine, tag: &str, count: i64, from: i64) {
    if count < from {
        return;
    }
    let h = fnv(FNV_BASIS, &m.sram);
    let g = fnv(FNV_BASIS, &m.regs);
    let a = fnv(FNV_BASIS, &m.analog);
    let mut s = format!("{} {} pc={:08x}", tag, count, m.r[15]);
    for i in 0..15 {
        s.push_str(&format!(" {:08x}", m.r[i]));
    }
    s.push_str(&format!(
        " nzcv={}{}{}{} mode={:02x} i={} cyc={} ms={:.6} sram={:016x} regs={:016x} ana={:016x} skip={}",
        m.n,
        m.z,
        m.c,
        m.v,
        m.mode,
        m.ibit,
        m.cycles,
        m.ms(),
        h,
        g,
        a,
        m.idle_skipped
    ));
    println!("{}", s);
}

fn main() {
    let o = parse_args();
    let img = std::fs::read(&o.image).unwrap_or_else(|e| {
        eprintln!("{}", e);
        std::process::exit(2)
    });
    let mut fl = Flash::new(1 << 20);
    fl.mem[..img.len()].copy_from_slice(&img);
    let sh = i64::from_str_radix(o.link.trim_start_matches("0x"), 16).unwrap_or(0);
    if sh > 0 {
        let sh = sh as usize;
        let n = img.len().min(fl.mem.len() - sh);
        fl.mem[sh..sh + n].copy_from_slice(&img[..n]);
    }
    let mut m = match Machine::new(fl, 0, o.cpi) {
        Ok(m) => m,
        Err(e) => {
            println!("ERROR {}", e);
            std::process::exit(1)
        }
    };
    if o.events {
        m.event = Some(Rc::new(|m: &mut Machine, s: &str| println!("E {:.6} {}", m.ms(), s)));
    }
    m.on_flash_write = Some(Rc::new(|m: &mut Machine, op: &str, addr: usize, l: usize| {
        println!("W {:.6} {} {:#x} {} pc={:#x}", m.ms(), op, addr, l, m.r[15]);
    }));
    let mut ranges = Vec::new();
    if !o.idle.is_empty() {
        let (lo, hi) = o.idle.split_once(':').unwrap();
        let a = u32::from_str_radix(lo.trim_start_matches("0x"), 16).unwrap();
        let b = u32::from_str_radix(hi.trim_start_matches("0x"), 16).unwrap();
        ranges.push(Range { lo: a, hi: b });
    }
    let count = Rc::new(Cell::new(0i64));
    let mut resets = 0;
    let max_resets = o.max_resets;
    let on_err = |m: &mut Machine, err: Error, count: i64, resets: &mut i32| -> bool {
        if matches!(err, Error::WatchdogReset | Error::SoftwareReset) {
            *resets += 1;
            if *resets > max_resets {
                println!("STOP resets");
                return false;
            }
            println!("RESET {} {:.6} {} pc={:#x}", count, m.ms(), err.to_string().replace(' ', "_"), m.r[15]);
            if let Err(e) = m.reset(-1, matches!(err, Error::WatchdogReset)) {
                println!("ERROR {}", e);
                return false;
            }
            return true;
        }
        println!("ERROR {} {:.6} {} pc={:#x}", count, m.ms(), err, m.r[15]);
        false
    };
    let start = Instant::now();
    match o.mode.as_str() {
        "step" => {
            while count.get() < o.n {
                if m.asleep.is_some() {
                    let t = m.cycles + m.cpu_hz / 1000;
                    m.sleep_until(t);
                    continue;
                }
                match m.step() {
                    Err(e) => {
                        if !on_err(&mut m, e, count.get(), &mut resets) {
                            break;
                        }
                        continue;
                    }
                    Ok(s) => {
                        if s.ran {
                            count.set(count.get() + 1);
                            if count.get() % o.every == 0 {
                                digest(&m, "D", count.get(), o.from);
                            }
                        }
                    }
                }
            }
        }
        "run" => {
            let c = count.clone();
            let (every, from) = (o.every, o.from);
            m.trace_step = Some(Rc::new(move |m: &mut Machine, _pc: u32, _hw: u16| {
                c.set(c.get() + 1);
                if c.get() % every == 0 {
                    digest(m, "D", c.get(), from);
                }
            }));
            while m.ms() < o.ms {
                if let Err(e) = m.run_ms(o.ms, &ranges) {
                    if !on_err(&mut m, e, count.get(), &mut resets) {
                        break;
                    }
                }
            }
        }
        _ => panic!("mode"),
    }
    digest(&m, "END", count.get(), o.from);
    let el = start.elapsed().as_secs_f64();
    eprintln!(
        "steptrace(rust): {} instructions, {:.3} ms simulated, {:.3} s wall, {:.2} Minstr/s, {:.1} sim-ms/s, resets {}",
        count.get(),
        m.ms(),
        el,
        count.get() as f64 / el / 1e6,
        m.ms() / el,
        resets
    );
}
