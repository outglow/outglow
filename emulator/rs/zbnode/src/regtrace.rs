use std::cell::{Cell, RefCell};
use std::fs::File;
use std::io::Write;
use std::rc::Rc;

use tc32emu::Machine;

use crate::gostr::float0;
use crate::must;

const RANGES: [(usize, usize); 7] = [(0x060, 0x070), (0x400, 0x500), (0x640, 0x650), (0x740, 0x750), (0xC00, 0xC50), (0xF00, 0xF30), (0x1200, 0x1300)];

struct Trace {
    f: RefCell<File>,
    t0: f64,
    t1: f64,
}

impl Trace {
    fn on(&self, m: &Machine) -> bool {
        let t = m.ms();
        t >= self.t0 && t <= self.t1
    }
    fn stamp(m: &Machine) -> String {
        format!("{:12.4} {:08x} pc={:06x} lr={:06x}", m.ms(), m.stimer_now(), m.r[15], m.r[14] & !1)
    }
    fn line(&self, s: String) {
        let _ = self.f.borrow_mut().write_all(s.as_bytes());
    }
    fn dump(&self, m: &Machine, tag: &str, lo: u16, n: i64) {
        let a = lo as usize;
        if n < 0 || a + n as usize > m.sram.len() {
            return;
        }
        let n = n as usize;
        self.line(format!("{}   {} 0x84{:04x}: {}\n", Self::stamp(m), tag, a, ble::gofmt::hex(&m.sram[a..a + n])));
    }
}

fn in_range(o: usize, size: usize) -> bool {
    if o == 0x643 && size == 1 {
        return false;
    }
    RANGES.iter().any(|&(lo, hi)| o < hi && o + size > lo) || (pwm_too() && o < 0x7c0 && o + size > 0x780)
}

fn pwm_too() -> bool {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("ZBNODE_REG_TRACE_PWM").is_some())
}

fn ble_len(m: &Machine, lo: u16, extra: i64) -> i64 {
    let a = lo as usize;
    if a + 6 > m.sram.len() {
        return -1;
    }
    6 + m.sram[a + 5] as i64 + extra
}

pub fn install(m: &mut Machine, spec: &str) {
    let parts: Vec<&str> = spec.split(',').collect();
    let f = must(File::create(parts[0]).map_err(|e| format!("open {}: {}", parts[0], go_io_err(&e))));
    let (mut t0, mut t1) = (0.0, f64::INFINITY);
    if parts.len() >= 3 {
        t0 = float0(parts[1]);
        t1 = float0(parts[2]);
    }
    let tr = Rc::new(Trace { f: RefCell::new(f), t0, t1 });
    let prev_w = m.reg_write_hook.clone();
    let t = tr.clone();
    m.reg_write_hook = Some(Rc::new(move |m: &mut Machine, o, size, val| {
        let res = match &prev_w {
            Some(p) => p(m, o, size, val),
            None => Ok(false),
        };
        if !t.on(m) {
            return res;
        }
        if in_range(o, size) {
            t.line(format!("{} W{:<2} 0x{:03x} = 0x{:x}\n", Trace::stamp(m), size * 8, o, val));
        }
        if o <= 0xBA && 0xBA < o + size {
            let ctrl = (val >> (8 * (0xBA - o) as u32)) as u8;
            match ctrl & 0x60 {
                0x60 => t.line(format!("{} A[0x{:02x}] = 0x{:02x}\n", Trace::stamp(m), m.regs[0xB8], m.regs[0xB9])),
                0x40 => t.line(format!("{} AR[0x{:02x}] -> 0x{:02x}\n", Trace::stamp(m), m.regs[0xB8], m.analog[m.regs[0xB8] as usize])),
                _ => {}
            }
        }
        if o == 0xC0C && size == 2 {
            t.dump(m, "TXBUF", val as u16, ble_len(m, val as u16, 0));
        } else if o == 0xC2C && size == 2 {
            t.dump(m, "FIFO ", val as u16, ble_len(m, val as u16, 0));
        }
        res
    }));
    let prev_t = m.update_time_hook.clone();
    let last = Rc::new(Cell::new(0u16));
    let t = tr;
    m.update_time_hook = Some(Rc::new(move |m: &mut Machine| {
        let res = match &prev_t {
            Some(p) => p(m),
            None => Ok(()),
        };
        let st = m.regs[0xF20] as u16 | (m.regs[0xF21] as u16) << 8;
        let set = st & !last.get();
        if set != 0 && t.on(m) {
            t.line(format!("{:12.4} {:08x} {:<19} S   0xf20 |= 0x{:04x} -> 0x{:04x}\n", m.ms(), m.stimer_now(), "(radio model)", set, st));
            if set & 1 != 0 {
                let lo = m.regs[0xC08] as u16 | (m.regs[0xC09] as u16) << 8;
                let mut n = 64;
                if m.regs[0x404] & 3 == 1 {
                    n = ble_len(m, lo, 11);
                }
                t.dump(m, "RXBUF", lo, n);
            }
        }
        last.set(st);
        res
    }));
}

fn go_io_err(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => "no such file or directory".into(),
        std::io::ErrorKind::PermissionDenied => "permission denied".into(),
        _ => e.to_string(),
    }
}
