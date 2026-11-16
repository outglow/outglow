use super::err_text;
use crate::{Error, Flash, Machine};

fn spi(m: &mut Machine, data: &[u8], read: usize) -> (Vec<u8>, Result<(), Error>) {
    let mut got = vec![];
    if let Err(e) = m.flash_select(true) {
        return (got, Err(e));
    }
    for &b in data {
        if let Err(e) = m.flash_clock(b) {
            return (got, Err(e));
        }
    }
    for _ in 0..read {
        match m.flash_clock(0) {
            Ok(v) => got.push(v),
            Err(e) => return (got, Err(e)),
        }
    }
    let r = m.flash_select(false);
    (got, r)
}

fn must(m: &mut Machine, data: &[u8], read: usize) -> Vec<u8> {
    let (mut got, r) = spi(m, data, read);
    if let Err(e) = r {
        panic!("FAIL unexpected: {}", e);
    }
    while got.len() < read {
        got.push(0xFF);
    }
    got
}

fn wip(m: &mut Machine) -> bool {
    must(m, &[0x05], 1)[0] & 0x01 != 0
}

fn program(m: &mut Machine, addr: usize, data: &[u8]) {
    must(m, &[0x06], 0);
    let mut c = vec![0x02, (addr >> 16) as u8, (addr >> 8) as u8, addr as u8];
    c.extend_from_slice(data);
    must(m, &c, 0);
}

fn erase(m: &mut Machine, addr: usize, cmd: u8) {
    must(m, &[0x06], 0);
    must(m, &[cmd, (addr >> 16) as u8, (addr >> 8) as u8, addr as u8], 0);
}

fn advance_us(m: &mut Machine, us: i64) {
    m.cycles += us * m.cpu_hz / 1_000_000;
}

fn xip_err(m: &mut Machine, addr: u32) -> String {
    err_text(&m.read(addr, 4))
}

#[test]
fn flash_busy() {
    let mut fl = Flash::new_jedec(0x80000, [0xC8, 0x60, 0x14]);
    fl.mem[8] = 0x4B;
    fl.tpp_us = 0;
    fl.tse_us = 0;
    fl.tbe32_us = 0;
    fl.tbe64_us = 0;
    let mut m = Machine::new(fl, -1, 1).unwrap();
    let m = &mut m;

    program(m, 0x40000, &[0x12]);
    assert!(m.flash.mem[0x40000] == 0x12 && !wip(m), "no busy times");

    m.flash.tpp_us = 1600;
    m.flash.tse_us = 150000;
    m.flash.tbe32_us = 500000;
    m.flash.tbe64_us = 800000;
    program(m, 0x40001, &[0x34]);
    assert!(wip(m), "TPP 1600 us: WIP set right after a page program");
    advance_us(m, 1590);
    assert!(wip(m), "WIP still set 1590 us on");
    advance_us(m, 20);
    assert!(!wip(m), "WIP clear 1610 us on");

    erase(m, 0x41000, 0x20);
    advance_us(m, 149000);
    assert!(wip(m), "TSE 150 ms: WIP set 149 ms after a sector erase");
    advance_us(m, 2000);
    assert!(!wip(m), "and clear at 151 ms");

    for (cmd, name, ms) in [(0xD8u8, "64 KB", 800i64), (0x52, "32 KB", 500)] {
        erase(m, 0x50000, cmd);
        advance_us(m, (ms - 1) * 1000);
        let busy_then = wip(m);
        advance_us(m, 2000);
        assert!(busy_then && !wip(m), "TBE {} ms: a {} block erase", ms, name);
    }

    must(m, &[0x06], 0);
    must(m, &[0x01, 0x1C], 0);
    program(m, 0x60000, &[0x00]);
    let last = m.flash.log.last().unwrap().clone();
    assert!(last.op == "protected" && last.addr == 0x60000 && !wip(m), "a program the protection ignores");
    must(m, &[0x06], 0);
    must(m, &[0x01, 0x00], 0);

    program(m, 0x40002, &[0x56]);
    let e = xip_err(m, 0x10000);
    assert!(!e.is_empty() && e.contains("busy"), "an XIP read while the part is busy stops the run: {}", e);
    m.flash.abort();
    assert!(wip(m), "a chip reset (abort) leaves the part busy");
    advance_us(m, 2000);
    assert!(xip_err(m, 0x10000).is_empty() && !wip(m), "the XIP read goes through once the part is done");

    program(m, 0x40003, &[0x78]);
    advance_us(m, 1000);
    m.reset(-1, false).unwrap();
    assert!(m.ms() == 0.0 && wip(m), "a reset 1000 us into a page program");
    advance_us(m, 590);
    assert!(wip(m), "WIP still set 590 us after the reset");
    advance_us(m, 20);
    assert!(!wip(m), "WIP clear 610 us after the reset");

    advance_us(m, 2000);
    program(m, 0x40005, &[0xBC]);
    advance_us(m, 800);
    m.reg_write(0x66, 1, 0x20).unwrap();
    assert_eq!(m.cpu_hz, 48_000_000, "the CPU clock at 48 MHz");
    advance_us(m, 790);
    assert!(wip(m), "WIP still set 790 us after the clock change");
    advance_us(m, 20);
    assert!(!wip(m), "WIP clear 810 us after the clock change");

    program(m, 0x40006, &[0xDE]);
    advance_us(m, 1000);
    m.reset(-1, false).unwrap();
    assert!(m.cpu_hz == 24_000_000 && wip(m), "a reset at 48 MHz 1000 us into a page program");
    advance_us(m, 590);
    assert!(wip(m), "WIP still set 590 us after the reset");
    advance_us(m, 20);
    assert!(!wip(m), "WIP clear 610 us after the reset");

    program(m, 0x40007, &[0xF0]);
    m.enter_suspend(0x81).unwrap();
    let t = m.cycles + 1590 * m.cpu_hz / 1_000_000;
    m.run(t, &[], 32).unwrap();
    assert!(m.asleep.is_some() && wip(m), "WIP still set after 1590 us of sleep");
    let t = m.cycles + 20 * m.cpu_hz / 1_000_000;
    m.run(t, &[], 32).unwrap();
    m.asleep = None;
    assert!(!wip(m), "WIP clear after 1610 us of sleep");

    program(m, 0x40008, &[0x11]);
    let (_, r) = spi(m, &[0x35], 1);
    assert!(wip(m) && r.is_ok(), "while busy: 0x05 and 0x35 are taken");
    let (_, r) = spi(m, &[0x02, 0x04, 0x00, 0x09, 0x22], 0);
    let e = err_text(&r);
    let _ = m.flash_select(false);
    assert!(e.contains("command 0x02") && e.contains("us left"), "while busy: a page program stops the run: {}", e);
    m.reg_write(0x0D, 1, 0x00).unwrap();
    m.reg_write(0x0C, 1, 0x03).unwrap();
    let e = err_text(&m.reg_write(0x0C, 1, 0x00));
    let _ = m.reg_write(0x0D, 1, 0x01);
    assert!(e.contains("command 0x03"), "while busy: a read (0x03) through the MSPI stops the run: {}", e);
    let (_, r) = spi(m, &[0x9F], 3);
    let e = err_text(&r);
    let _ = m.flash_select(false);
    assert!(e.contains("command 0x9f"), "while busy: a JEDEC ID read stops the run: {}", e);
    let (_, r) = spi(m, &[0x06], 0);
    let e = err_text(&r);
    assert!(e.contains("command 0x06"), "while busy: a write enable stops the run: {}", e);
    advance_us(m, 2000);
    let (_, r1) = spi(m, &[0x06], 0);
    let (_, r2) = spi(m, &[0x04], 0);
    assert!(r1.is_ok() && r2.is_ok(), "the write enable is taken once the part is done");

    program(m, 0x40004, &[0x9A]);
    let fl = std::mem::replace(&mut m.flash, Flash::new(0x1000));
    let mut m2 = Machine::new(fl, -1, 1).unwrap();
    assert!(!wip(&mut m2), "a power-on (a new Machine on the flash) finds the part idle");
}
