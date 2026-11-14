// flash_size_test.go: FlashSize (the address-map bound) is the machine's
// cfg.flash_size here.
use super::err_text;
use crate::{Config, Error, Flash, Machine, FLASH_MEM, REG_BASE};

fn spi(fl: &mut Flash, data: &[u8], read: usize) -> (Vec<u8>, Result<(), Error>) {
    let mut got = vec![];
    if let Err(e) = fl.select(true) {
        return (got, Err(e));
    }
    for &b in data {
        if let Err(e) = fl.clock(b) {
            return (got, Err(e));
        }
    }
    for _ in 0..read {
        match fl.clock(0) {
            Ok(v) => got.push(v),
            Err(e) => return (got, Err(e)),
        }
    }
    let r = fl.select(false);
    (got, r)
}

fn mread(m: &mut Machine, addr: usize, n: usize) -> (Vec<u8>, Result<(), Error>) {
    let mut got = vec![];
    let _ = m.write(REG_BASE + 0x0D, 1, 0x00);
    for b in [0x03u8, (addr >> 16) as u8, (addr >> 8) as u8, addr as u8, 0x00] {
        let _ = m.write(REG_BASE + 0x0C, 1, b as u32);
    }
    let _ = m.write(REG_BASE + 0x0D, 1, 0x0A);
    let mut err = Ok(());
    for _ in 0..n {
        match m.read(REG_BASE + 0x0C, 1) {
            Ok(v) => got.push(v as u8),
            Err(e) => {
                err = Err(e);
                break;
            }
        }
    }
    let _ = m.write(REG_BASE + 0x0D, 1, 0x01);
    (got, err)
}

#[test]
fn flash_size() {
    for size in [0x80000usize, FLASH_MEM] {
        flash_size_half(size);
    }
}

fn flash_size_half(size: usize) {
    let small = size < FLASH_MEM;
    let mut cfg = Config::env();
    cfg.flash_size = size;
    let refused = |r: &Result<(), Error>| err_text(r).contains(&format!("the part has 0x{:x} bytes", size));
    let flash = |slot: usize| {
        let mut fl = Flash::new(size);
        fl.mem[slot + 8] = 0x4B;
        fl
    };
    let machine = |fl: Flash| Machine::with_config(fl, -1, 1, cfg.clone()).unwrap();

    let mut fl = flash(0);
    assert!(fl.size == size && fl.mem.len() == FLASH_MEM, "the part's size and the array's");
    let (got, _) = spi(&mut fl, &[0x9F], 3);
    let want_cap = if small { 0x13 } else { 0x14 };
    assert_eq!(got, vec![0x85, 0x60, want_cap], "JEDEC ID");
    fl.mem[0x7FFFE..0x80002].copy_from_slice(&[0x11, 0x22, 0x33, 0x44]);
    let top = if small { 0x80000 } else { 0x100000 };
    let mut m = machine(fl);
    let (got, r) = mread(&mut m, 0x7FFFE, 2);
    assert!(got == b"\x11\x22" && r.is_ok(), "read 0x7fffe-0x7ffff");
    let (got, r) = mread(&mut m, 0x80000, 1);
    if small {
        assert!(refused(&r), "read 0x80000: refused {}", err_text(&r));
    } else {
        assert!(got == b"\x33" && r.is_ok(), "read 0x80000: 33");
    }
    let (got, r) = mread(&mut m, 0x7FFFE, 3);
    if small {
        assert!(refused(&r) && got == b"\x11\x22", "read of 3 bytes from 0x7fffe: the third refused");
    } else {
        assert_eq!(got, b"\x11\x22\x33", "read of 3 bytes from 0x7fffe");
    }
    if small {
        let n0 = m.flash.log.len();
        m.flash.beyond_mode = "ff".into();
        let (got, r) = mread(&mut m, 0x7FFFE, 4);
        assert!(got == b"\x11\x22\xff\xff" && r.is_ok(), "beyond = ff");
        m.flash.mem[0] = 0xA1;
        m.flash.mem[1] = 0xA2;
        m.flash.beyond_mode = "wrap".into();
        let (got, r) = mread(&mut m, 0x7FFFE, 4);
        assert!(got == b"\x11\x22\xa1\xa2" && r.is_ok(), "beyond = wrap");
        let (got, r) = mread(&mut m, 0xFFFFFB, 4);
        assert!(got == m.flash.mem[0x7FFFB..0x7FFFF].to_vec() && r.is_ok(), "beyond = wrap: 0xfffffb reads 0x7fffb");
        let addrs: Vec<i64> = m.flash.log[n0..].iter().filter(|o| o.op == "read beyond").map(|o| o.addr).collect();
        assert_eq!(addrs, vec![0x80000, 0x80001, 0x80000, 0x80001, 0xFFFFFB, 0xFFFFFC, 0xFFFFFD, 0xFFFFFE], "logged");
        m.flash.log.truncate(n0);
        m.flash.mem[0] = 0xFF;
        m.flash.mem[1] = 0xFF;
        m.flash.beyond_mode = "stop".into();
    }
    let fl = &mut m.flash;
    let _ = spi(fl, &[0x06], 0);
    let (_, r) = spi(fl, &[0x02, 0x07, 0xF0, 0x00, 0xA5], 0);
    assert!(r.is_ok() && fl.mem[0x7F000] == 0xA5, "program 0x7f000: written");
    let before = fl.mem[0x80000..0x80100].to_vec();
    let _ = spi(fl, &[0x06], 0);
    let (_, r) = spi(fl, &[0x02, 0x08, 0x00, 0x10, 0x00], 0);
    if small {
        assert!(refused(&r) && fl.mem[0x80000..0x80100] == before[..], "program 0x80010: refused");
    } else {
        assert!(r.is_ok() && fl.mem[0x80010] == 0x00, "program 0x80010: written");
    }
    fl.wel = false;
    let _ = spi(fl, &[0x06], 0);
    let (_, r) = spi(fl, &[0x20, 0x07, 0xF0, 0x00], 0);
    assert!(r.is_ok() && fl.mem[0x7F000] == 0xFF, "erase 0x7f000: erased");
    fl.mem[0x80000] = 0x5A;
    let _ = spi(fl, &[0x06], 0);
    let (_, r) = spi(fl, &[0x20, 0x08, 0x00, 0x00], 0);
    if small {
        assert!(refused(&r) && fl.mem[0x80000] == 0x5A, "erase 0x80000: refused");
    } else {
        assert!(r.is_ok() && fl.mem[0x80000] == 0xFF, "erase 0x80000: erased");
    }
    for op in &fl.log {
        assert!(op.addr < top, "the flash's log holds no write at or above the top: {} {:#x}", op.op, op.addr);
    }
    for slot in [0x0usize, 0x20000] {
        let mut fl = flash(slot);
        fl.mem[0x7FFFC..0x80004].copy_from_slice(&[0, 1, 2, 3, 4, 5, 6, 7]);
        let mut m = machine(fl);
        let last = 0x80000u32 - 4;
        assert_eq!(m.read(last, 4).unwrap(), 0x03020100, "boot slot {:#x}: XIP read of flash 0x7fffc", slot);
        let r = m.read(last + 4, 4);
        if small {
            assert!(r.is_err(), "boot slot {:#x}: XIP read of flash 0x80000: refused", slot);
        } else {
            assert_eq!(r.unwrap(), 0x07060504, "boot slot {:#x}: XIP read of flash 0x80000", slot);
        }
        for i in 0..16 {
            m.flash.mem[slot + 0x1000 + i] = 0x40 + i as u8;
        }
        let r = m.write(REG_BASE + 0x608, 4, (1 << 24) | 0x1000);
        assert!(r.is_ok() && (0..16).all(|i| m.sram[0x1000 + i] == 0x40 + i as u8), "loader load of offset 0x1000");
        let r = m.write(REG_BASE + 0x608, 4, (1 << 24) | m.cfg.sram_size as u32);
        assert!(r.is_err() && m.sram.len() == m.cfg.sram_size, "loader load of offset SRAM_SIZE: refused");
    }
}
