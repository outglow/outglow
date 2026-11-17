use std::cell::Cell;
use std::rc::Rc;

use super::thumb::machine;
use crate::{I2cDevice, Machine};

fn rd(m: &mut Machine, o: usize) -> u8 {
    m.reg_read(o, 1).unwrap() as u8
}

fn wr(m: &mut Machine, o: usize, v: u32) {
    m.reg_write(o, 1, v).unwrap();
}

#[test]
fn uart_transmit_buffer_captures_the_bytes_written() {
    let mut m = machine();
    for b in b"Hi!" {
        wr(&mut m, 0x90, *b as u32);
    }
    assert_eq!(m.uart_tx, b"Hi!");
    assert_ne!(rd(&mut m, 0x9D) & 0x01, 0, "transmit done");
}

#[test]
fn uart_receive_reports_count_and_returns_bytes_in_order() {
    let mut m = machine();
    assert_eq!(rd(&mut m, 0x9E) & 0x04, 0, "nothing received");
    m.uart_rx = vec![0x41, 0x42];
    assert_ne!(rd(&mut m, 0x9E) & 0x04, 0, "receive status");
    assert_eq!(rd(&mut m, 0x9C) & 0x07, 2, "count");
    assert_eq!((rd(&mut m, 0x90), rd(&mut m, 0x90)), (0x41, 0x42));
    assert_eq!(rd(&mut m, 0x9E) & 0x04, 0, "drained");
}

fn i2c() -> Machine {
    let mut m = machine();
    let mut dev = I2cDevice::new(0x76);
    dev.mem[0xD0] = 0x60;
    m.i2c_dev = Some(dev);
    wr(&mut m, 0x03, 0x02);
    m
}

#[test]
fn i2c_register_read_returns_the_device_byte_after_an_acked_address() {
    let mut m = i2c();
    wr(&mut m, 0x01, 0x76 << 1);
    wr(&mut m, 0x04, 0xD0);
    wr(&mut m, 0x07, 0x01 | 0x02 | 0x10);
    assert_eq!(rd(&mut m, 0x02) & 0x04, 0, "ACK");
    wr(&mut m, 0x07, 0x08 | 0x20);
    assert_eq!(rd(&mut m, 0x06), 0x60);
}

#[test]
fn i2c_register_write_is_read_back() {
    let mut m = i2c();
    wr(&mut m, 0x01, 0x76 << 1);
    wr(&mut m, 0x04, 0xF4);
    wr(&mut m, 0x05, 0x27);
    wr(&mut m, 0x07, 0x01 | 0x02 | 0x04 | 0x10 | 0x20);
    wr(&mut m, 0x04, 0xF4);
    wr(&mut m, 0x07, 0x01 | 0x02 | 0x10);
    wr(&mut m, 0x07, 0x08 | 0x20);
    assert_eq!(rd(&mut m, 0x06), 0x27);
    assert_eq!(m.i2c_dev.as_ref().unwrap().mem[0xF4], 0x27);
}

#[test]
fn i2c_absent_address_is_nacked_and_reads_ff() {
    let mut m = i2c();
    wr(&mut m, 0x01, 0x40 << 1);
    wr(&mut m, 0x07, 0x01 | 0x10);
    assert_ne!(rd(&mut m, 0x02) & 0x04, 0, "NAK");
    wr(&mut m, 0x07, 0x08 | 0x20);
    assert_eq!(rd(&mut m, 0x06), 0xFF);
}

#[test]
fn i2c_master_disabled_ignores_a_launch() {
    let mut m = i2c();
    wr(&mut m, 0x03, 0x00);
    m.regs[0x06] = 0x11;
    wr(&mut m, 0x01, 0x76 << 1);
    wr(&mut m, 0x04, 0xD0);
    wr(&mut m, 0x07, 0x01 | 0x02 | 0x08 | 0x10 | 0x20);
    assert_eq!(rd(&mut m, 0x06), 0x11);
}

fn qdec() -> (Machine, Rc<Cell<(bool, bool)>>) {
    let mut m = machine();
    m.regs[0x581] |= 0x0C;
    let ab = Rc::new(Cell::new((false, false)));
    let pins = ab.clone();
    m.pad_levels_hook = Some(Rc::new(move |_m: &mut Machine, port: usize, mut lvl: u8, mut flt: u8| {
        if port == 0 {
            let (a, b) = pins.get();
            flt &= !0x0C;
            lvl = (lvl & !0x0C) | if a { 0x04 } else { 0 } | if b { 0x08 } else { 0 };
        }
        (lvl, flt)
    }));
    (m, ab)
}

fn walk(m: &mut Machine, ab: &Rc<Cell<(bool, bool)>>, states: &[(u8, u8)]) {
    for &(a, b) in states {
        ab.set((a != 0, b != 0));
        m.update_time().unwrap();
    }
}

fn count(m: &mut Machine) -> i8 {
    m.reg_read(0xD0, 1).unwrap() as u8 as i8
}

#[test]
fn qdec_counts_a_full_gray_code_cycle_in_each_direction() {
    let (mut m, ab) = qdec();
    walk(&mut m, &ab, &[(0, 0), (0, 1), (1, 1), (1, 0), (0, 0)]);
    let one = count(&mut m);
    walk(&mut m, &ab, &[(0, 0), (1, 0), (1, 1), (0, 1), (0, 0)]);
    let other = count(&mut m);
    assert_eq!(one, -other, "the two directions are symmetric");
    assert_eq!(one.unsigned_abs(), 4, "double accuracy: one count per edge");
}

#[test]
fn qdec_count_clears_on_read_and_on_reset() {
    let (mut m, ab) = qdec();
    walk(&mut m, &ab, &[(0, 0), (0, 1), (1, 1)]);
    assert_ne!(count(&mut m), 0);
    assert_eq!(count(&mut m), 0, "a read clears it");
    walk(&mut m, &ab, &[(1, 0), (0, 0)]);
    wr(&mut m, 0xD6, 1);
    assert_eq!(count(&mut m), 0, "reset");
}

#[test]
fn qdec_ignores_a_reversal_back_to_the_start() {
    let (mut m, ab) = qdec();
    walk(&mut m, &ab, &[(0, 0), (0, 1), (0, 0), (0, 1), (0, 0)]);
    assert_eq!(count(&mut m), 0);
}

fn u16_at(m: &mut Machine, lo: usize) -> u32 {
    m.reg_read(lo, 1).unwrap() | m.reg_read(lo + 1, 1).unwrap() << 8
}

#[test]
fn audio_fifo_advances_at_16k_samples_per_second_only_while_enabled() {
    let mut m = machine();
    assert_eq!((u16_at(&mut m, 0xB16), u16_at(&mut m, 0xB20)), (0, 0));
    wr(&mut m, 0xB10, 0x01);
    let (w0, n0) = (u16_at(&mut m, 0xB16), u16_at(&mut m, 0xB20));
    m.cycles += m.cpu_hz / 1000;
    let (w1, n1) = (u16_at(&mut m, 0xB16), u16_at(&mut m, 0xB20));
    assert_eq!(n1 - n0, 16, "samples in 1 ms");
    assert_eq!(w1 - w0, 32, "write pointer in bytes");
    wr(&mut m, 0xB10, 0x00);
    let w2 = u16_at(&mut m, 0xB16);
    m.cycles += m.cpu_hz / 1000;
    assert_eq!(u16_at(&mut m, 0xB16), w2, "frozen when disabled");
}

#[test]
fn pga_current_gain_follows_the_limit_in_auto_mode_and_the_setting_in_manual_mode() {
    let mut m = machine();
    m.regs[0xB42] = 0x80;
    m.regs[0xB43] = 0x2A;
    assert_eq!((rd(&mut m, 0xB5E), rd(&mut m, 0xB5F)), (0x2A, 0x2A));
    m.regs[0xB42] = 0x00;
    m.regs[0xB41] = 0x15;
    assert_eq!((rd(&mut m, 0xB5E), rd(&mut m, 0xB5F)), (0x15, 0x15));
}
