use std::cell::RefCell;
use std::rc::Rc;

use crate::{Flash, Machine, FLASH_MEM};

const SLOT: u32 = 24000;

fn machine(below: &str) -> Machine {
    let mut fl = Flash::new(FLASH_MEM);
    fl.mem[8] = 0x4B;
    let mut m = Machine::new(fl, -1, 1).unwrap();
    m.cfg.timer_capture_below = below.to_string();
    m
}

fn write(m: &mut Machine, o: usize, size: usize, val: u32) -> String {
    super::err_text(&m.reg_write(o, size, val))
}

fn start_t1(m: &mut Machine, capture: u32, tick: u32) -> String {
    let v = m.regs[0x620] as u32 & !0x38;
    write(m, 0x620, 1, v);
    write(m, 0x634, 4, tick);
    write(m, 0x628, 4, capture);
    let v = (m.regs[0x620] as u32 & !0x30) | 0x08;
    write(m, 0x620, 1, v)
}

fn start_t0(m: &mut Machine, capture: u32, tick: u32) -> String {
    let v = m.regs[0x620] as u32 & !0x07;
    write(m, 0x620, 1, v);
    write(m, 0x630, 4, tick);
    write(m, 0x624, 4, capture);
    let v = (m.regs[0x620] as u32 & !0x06) | 0x01;
    write(m, 0x620, 1, v)
}

fn run(m: &mut Machine, cycles: i64) {
    m.cycles += cycles;
    m.update_time().unwrap();
}

fn matched_n(m: &mut Machine, n: u32) -> bool {
    let hit = m.regs[0x623] & (1 << n) != 0;
    write(m, 0x623, 1, 1 << n);
    hit
}

fn matched(m: &mut Machine) -> bool {
    matched_n(m, 1)
}

#[test]
fn timer_capture() {
    let s = "stop";
    let mut m = machine(s);
    assert_eq!(start_t1(&mut m, SLOT, 0), "", "Timer1 started with capture 24000");
    run(&mut m, SLOT as i64 - 1);
    assert!(!matched(&mut m), "no match one cycle before the capture");
    run(&mut m, 1);
    assert!(matched(&mut m) && m.tmr_tick(1) == 0, "a match at the capture, and the count starts again from 0");
    run(&mut m, 3 * SLOT as i64);
    assert!(matched(&mut m) && m.tmr_tick(1) == 0, "periodic");

    let mut m = machine(s);
    start_t1(&mut m, SLOT, 0);
    run(&mut m, 10000);
    assert_eq!(write(&mut m, 0x628, 4, 30000), "", "a capture raised above the running count is accepted");
    run(&mut m, 19999);
    assert!(!matched(&mut m), "no match before the count reaches the raised capture");
    run(&mut m, 1);
    assert!(matched(&mut m), "a match at the raised capture");

    let mut m = machine(s);
    start_t1(&mut m, SLOT, 0);
    run(&mut m, 20000);
    let e = write(&mut m, 0x628, 4, 10000);
    assert!(e.contains("wrapped"), "a capture written below the running count stops the run: {}", e);

    let mut m = machine(s);
    start_t1(&mut m, SLOT, 0);
    run(&mut m, 10000);
    let e = write(&mut m, 0x634, 4, 30000);
    assert!(e.contains("count write"), "a count written past the capture while running stops the run");

    let mut m = machine(s);
    let e = start_t1(&mut m, SLOT, 30000);
    assert!(e.contains("a start"), "a start with the count past the capture stops the run");

    let mut m = machine(s);
    write(&mut m, 0x620, 1, 0);
    assert_eq!(write(&mut m, 0x628, 4, 5), "", "a capture write while the timer is stopped is not checked");

    let mut m = machine(s);
    start_t1(&mut m, SLOT, 0);
    run(&mut m, 10000);
    assert_eq!(write(&mut m, 0x628, 4, SLOT + 10000 + 960), "", "stretched");
    run(&mut m, SLOT as i64 + 960);
    assert!(matched(&mut m) && m.tmr_tick(1) == 0, "the stretched period matches at 34960");
    run(&mut m, 30000);
    assert_ne!(write(&mut m, 0x628, 4, SLOT), "", "set back to 24000 when the count is 30000");

    let mut m = machine(s);
    start_t1(&mut m, SLOT, 0);
    run(&mut m, SLOT as i64 - 5);
    m.cycles += 10;
    assert!(write(&mut m, 0x628, 4, SLOT).is_empty() && matched(&mut m) && m.tmr_tick(1) == 5, "capture write 5 after");
    let mut m = machine(s);
    start_t1(&mut m, SLOT, 0);
    run(&mut m, SLOT as i64 - 5);
    m.cycles += 10;
    let v = m.regs[0x620] as u32 & !0x38;
    write(&mut m, 0x620, 1, v);
    assert!(matched(&mut m) && m.tmr_tick(1) == 5, "a stop 5 cycles after a match");
    let v = (m.regs[0x620] as u32 & !0x30) | 0x08;
    assert_eq!(write(&mut m, 0x620, 1, v), "", "and a start from that count does not stop the run");
    let mut m = machine(s);
    start_t1(&mut m, SLOT, 0);
    run(&mut m, SLOT as i64 - 5);
    m.cycles += 10;
    assert!(write(&mut m, 0x634, 4, 1000).is_empty() && matched(&mut m) && m.tmr_tick(1) == 1000, "count write");
    let mut m = machine(s);
    start_t1(&mut m, SLOT, 0);
    run(&mut m, SLOT as i64 - 5);
    m.cycles += 10;
    assert!(write(&mut m, 0x628, 4, 30000).is_empty() && matched(&mut m) && m.tmr_tick(1) == 5, "capture raised");
    let mut m = machine(s);
    start_t1(&mut m, SLOT, 0);
    run(&mut m, SLOT as i64 - 5);
    m.cycles += 10;
    let v = m.regs[0x620] as u32 | (m.regs[0x621] as u32) << 8 | (m.regs[0x622] as u32) << 16 | 0x02 << 24;
    write(&mut m, 0x620, 4, v);
    run(&mut m, 0);
    assert!(!matched(&mut m) && m.tmr_tick(1) == 5, "a 32-bit 0x620 write clearing the status clears that match");

    let mut m = machine(s);
    start_t1(&mut m, SLOT, 0);
    run(&mut m, SLOT as i64 - 5);
    m.cycles += 10;
    assert_eq!(m.reg_read(0x634, 4).unwrap(), 5, "a count read 5 cycles after a match");
    let mut m = machine(s);
    start_t1(&mut m, SLOT, 0);
    run(&mut m, SLOT as i64 - 5);
    m.cycles += 10;
    assert!(m.reg_read(0x623, 1).unwrap() & 0x02 != 0, "a status read shows the match");

    let mut m = machine(s);
    assert_eq!(start_t0(&mut m, SLOT, 0), "", "Timer0 started");
    run(&mut m, SLOT as i64 - 1);
    assert!(!matched_n(&mut m, 0), "Timer0: no match one cycle before the capture");
    run(&mut m, 1);
    assert!(matched_n(&mut m, 0) && m.tmr_tick(0) == 0 && !matched(&mut m), "Timer0: a match at the capture");
    run(&mut m, 20000);
    let e = write(&mut m, 0x624, 4, 10000);
    assert!(e.contains("timer 0"), "Timer0: a capture written below the running count stops the run: {}", e);

    let mut m = machine(s);
    start_t1(&mut m, 0, 0);
    run(&mut m, (1 << 32) + 5);
    assert!(write(&mut m, 0x628, 4, 100).is_empty() && m.tmr_tick(1) == 5, "capture 0, 2^32 + 5 cycles on");
    run(&mut m, 94);
    assert!(!matched(&mut m), "no match before the count reaches 100");
    run(&mut m, 1);
    assert!(matched(&mut m) && m.tmr_tick(1) == 0, "the match at 100");

    let s = "wrap";
    let mut m = machine(s);
    let events = Rc::new(RefCell::new(Vec::<String>::new()));
    let e2 = events.clone();
    m.event = Some(Rc::new(move |_m: &mut Machine, t: &str| e2.borrow_mut().push(t.to_string())));
    start_t1(&mut m, SLOT, 0);
    run(&mut m, 20000);
    let ok = write(&mut m, 0x628, 4, 10000).is_empty();
    let logged = events.borrow().iter().any(|e| e.contains("wrapped"));
    assert!(ok && logged, "wrap: the write is taken and logged as an event");
    run(&mut m, (1 << 32) - 20000 + 10000 - 1);
    assert!(!matched(&mut m), "wrap: no match until the count has wrapped round");
    run(&mut m, 1);
    assert!(matched(&mut m) && m.tmr_tick(1) == 0, "wrap: the match after the wrap");
    run(&mut m, 10000);
    assert!(matched(&mut m), "wrap: periodic again after it");
    let mut m = machine(s);
    start_t1(&mut m, SLOT, 0);
    run(&mut m, 20000);
    write(&mut m, 0x628, 4, 10000);
    run(&mut m, (1 << 32) - 20000 + 5);
    assert!(write(&mut m, 0x628, 4, 100).is_empty() && m.tmr_tick(1) == 5, "wrap: a capture of 100 at count 5");
    run(&mut m, 94);
    assert!(!matched(&mut m), "wrap: no match before 100");
    run(&mut m, 1);
    assert!(matched(&mut m) && m.tmr_tick(1) == 0, "wrap: the match at 100");
}
