// flash_lock_test.go (a flash without a machine), TestFlashOnWrite,
// TestXIPWhileChipSelectLow and TestSRAMFill.
use std::cell::RefCell;
use std::rc::Rc;

use crate::{sram_fill, Error, Flash, FlashOp, Machine, FLASH_MEM, REG_BASE};

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

fn status(fl: &mut Flash) -> i64 {
    let (lo, _) = spi(fl, &[0x05], 1);
    let (hi, _) = spi(fl, &[0x35], 1);
    lo[0] as i64 | (hi[0] as i64) << 8
}

fn program(fl: &mut Flash, addr: usize, data: &[u8]) -> Result<(), Error> {
    let _ = spi(fl, &[0x06], 0);
    let mut c = vec![0x02, (addr >> 16) as u8, (addr >> 8) as u8, addr as u8];
    c.extend_from_slice(data);
    spi(fl, &c, 0).1
}

fn erase(fl: &mut Flash, addr: usize, cmd: u8) -> Result<(), Error> {
    let _ = spi(fl, &[0x06], 0);
    spi(fl, &[cmd, (addr >> 16) as u8, (addr >> 8) as u8, addr as u8], 0).1
}

fn write_status(fl: &mut Flash, cmd: u8, data: &[u8]) -> Result<(), Error> {
    let _ = spi(fl, &[0x06], 0);
    let mut c = vec![cmd];
    c.extend_from_slice(data);
    spi(fl, &c, 0).1
}

fn range_is(fl: &Flash, first: i64, last: i64) -> bool {
    let (r, p, l) = fl.protected_range();
    p && l && r.first == first && r.last == last
}

fn none(fl: &Flash) -> bool {
    let (_, p, l) = fl.protected_range();
    !p && l
}

fn last(fl: &Flash) -> FlashOp {
    fl.log.last().unwrap().clone()
}

fn op_is(op: &FlashOp, name: &str, addr: i64, n: i64) -> bool {
    op.op == name && op.addr == addr && op.len == n
}

#[test]
fn flash_lock() {
    let mut fl = Flash::new_jedec(0x80000, [0xC8, 0x60, 0x14]);
    assert_eq!(fl.mid, 0x1360C8, "512 KB GD mid");
    assert!(status(&mut fl) == 0 && none(&fl), "status 0 after power-on");
    let _ = spi(&mut fl, &[0x06], 0);
    let (v, _) = spi(&mut fl, &[0x05], 1);
    assert_eq!(v[0], 0x02, "WEL in bit 1");
    let _ = spi(&mut fl, &[0x04], 0);
    let r = write_status(&mut fl, 0x01, &[0x18]);
    assert!(r.is_ok() && status(&mut fl) == 0x18 && range_is(&fl, 0, 0x3FFFF), "0x18: the low 256 KB protected");
    let l = last(&fl);
    assert!(l.op == "status" && l.addr == 0x01 && l.data == "18", "status logged");
    let r = program(&mut fl, 0x3F000, &[0x00]);
    assert!(r.is_ok() && fl.mem[0x3F000] == 0xFF && op_is(&last(&fl), "protected", 0x3F000, 1), "program at 0x3f000 ignored");
    let r = program(&mut fl, 0x8, &[0xFF, 0x00]);
    assert!(r.is_ok() && fl.mem[0x9] == 0xFF && op_is(&last(&fl), "protected", 0x8, 2), "boot flag program ignored");
    let r = erase(&mut fl, 0x20000, 0x20);
    assert!(r.is_ok() && op_is(&last(&fl), "protected", 0x20000, 0x1000), "erase at 0x20000 ignored");
    let r = erase(&mut fl, 0x3F000, 0xD8);
    assert!(r.is_ok() && op_is(&last(&fl), "protected", 0x30000, 0x10000), "64 KB erase ignored");
    let r = program(&mut fl, 0x40000, &[0x5A]);
    assert!(r.is_ok() && fl.mem[0x40000] == 0x5A && op_is(&last(&fl), "program", 0x40000, 1), "program at 0x40000 written");
    let r = erase(&mut fl, 0x68000, 0x20);
    assert!(r.is_ok() && op_is(&last(&fl), "erase", 0x68000, 0x1000), "erase at 0x68000 erased");
    fl.mem[0x3F000] = 0x00;
    let r = erase(&mut fl, 0x3F000, 0x52);
    assert!(r.is_ok() && fl.mem[0x3F000] == 0x00 && op_is(&last(&fl), "protected", 0x38000, 0x8000), "32 KB erase ignored");
    let (v, _) = spi(&mut fl, &[0x05], 1);
    assert_eq!(v[0], 0x18, "WEL clear after an ignored program");
    let r = write_status(&mut fl, 0x01, &[0x1C]);
    assert!(r.is_ok() && range_is(&fl, 0, 0x7FFFF), "0x1c: all protected");
    let r = program(&mut fl, 0x40000, &[0x00]);
    assert!(r.is_ok() && fl.mem[0x40000] == 0x5A, "program at 0x40000 under 0x1c ignored");
    let r = write_status(&mut fl, 0x01, &[0x00]);
    assert!(r.is_ok() && status(&mut fl) == 0 && none(&fl), "0x00: the unlock");
    let r = program(&mut fl, 0x3F000, &[0x00]);
    assert!(r.is_ok() && fl.mem[0x3F000] == 0x00 && op_is(&last(&fl), "program", 0x3F000, 1), "program after the unlock");
    let _ = write_status(&mut fl, 0x01, &[0x18]);
    assert_eq!(status(&mut fl), 0x18, "locked again");
    let (_, r) = spi(&mut fl, &[0x01, 0x00], 0);
    assert!(r.is_ok() && status(&mut fl) == 0x18, "0x01 without write enable ignored");
    let _ = write_status(&mut fl, 0x01, &[0x18 | 0x02 | 0x01]);
    assert_eq!(status(&mut fl), 0x18, "WIP and WEL not stored");
    let r = write_status(&mut fl, 0x01, &[0x18, 0x02]);
    assert!(r.is_ok() && status(&mut fl) == 0x0218 && last(&fl).data == "1802", "two bytes with 0x01");
    let r = write_status(&mut fl, 0x31, &[0x00]);
    assert!(r.is_ok() && status(&mut fl) == 0x0018, "0x31 writes the high byte");

    let mut fl = Flash::new_jedec(FLASH_MEM, [0xC8, 0x60, 0x14]);
    assert_eq!(fl.mid, 0x1460C8, "1 MB GD mid");
    let _ = write_status(&mut fl, 0x01, &[0x18]);
    assert!(range_is(&fl, 0, 0xBFFFF), "0x18 on 1 MB: the low 768 KB");
    let _ = program(&mut fl, 0x40000, &[0x00]);
    assert!(fl.mem[0x40000] == 0xFF && op_is(&last(&fl), "protected", 0x40000, 1), "program at 0x40000 ignored on 1 MB");
    let _ = write_status(&mut fl, 0x01, &[0x14]);
    assert!(range_is(&fl, 0, 0xDFFFF), "0x14 on 1 MB: the low 896 KB");

    let mut fl = Flash::new_jedec(0x80000, [0x5E, 0x32, 0x14]);
    let _ = write_status(&mut fl, 0x01, &[0x18]);
    assert!(fl.mid == 0x13325E && range_is(&fl, 0, 0x3FFFF), "Zbit 512 KB: 0x18 the low 256 KB");

    let mut fl = Flash::new_jedec(0x80000, [0x5E, 0x32, 0x14]);
    let (v, _) = spi(&mut fl, &[0x5A, 0, 0, 0, 0], 4);
    assert!(v == b"\xff\xff\xff\xff" && fl.part == 0x13325E, "Zbit B: no SFDP");
    fl.set_sfdp(b"SFDP".to_vec());
    let (v, _) = spi(&mut fl, &[0x5A, 0, 0, 0, 0], 4);
    assert!(v == b"SFDP" && fl.part == 0x0113325E, "Zbit C: SFDP signature");
    let (v, _) = spi(&mut fl, &[0x5A, 0, 0, 2, 0], 3);
    assert_eq!(v, b"DP\xff", "SFDP from address 2");
    let r = write_status(&mut fl, 0x01, &[0x2C, 0x00]);
    assert!(r.is_ok() && status(&mut fl) == 0x002C && range_is(&fl, 0, 0x3FFFF), "C 0x002c");
    let r = write_status(&mut fl, 0x01, &[0x10]);
    assert!(r.is_ok() && range_is(&fl, 0, 0x7FFFF), "C 0x0010: all");
    let r = write_status(&mut fl, 0x01, &[0x18]);
    assert!(r.is_err() && r.unwrap_err().to_string().contains("113325e"), "C 0x0018: not listed");
    let r = write_status(&mut fl, 0x01, &[0x04, 0x40]);
    assert!(r.is_ok() && range_is(&fl, 0, 0x6FFFF), "C 0x4004: the low 448 KB");
    let mut fl = Flash::new_jedec(FLASH_MEM, [0x5E, 0x32, 0x14]);
    fl.set_sfdp(b"SFDP".to_vec());
    let _ = write_status(&mut fl, 0x01, &[0x1C]);
    assert!(fl.part == 0x0114325E && range_is(&fl, 0, 0xFFFFF), "ZB25WD80C 0x001c: all");
    let _ = write_status(&mut fl, 0x01, &[0x10]);
    assert!(range_is(&fl, 0x80000, 0xFFFFF), "ZB25WD80C 0x0010: the upper 512 KB");

    let mut fl = Flash::new(FLASH_MEM);
    assert_eq!(fl.mid, 0x146085, "default mid");
    let r = write_status(&mut fl, 0x01, &[0x2C]);
    assert!(r.is_ok() && range_is(&fl, 0, 0x3FFFF), "0x002c: the low 256 KB");
    let r = write_status(&mut fl, 0x01, &[0x0C, 0x40]);
    assert!(r.is_ok() && status(&mut fl) == 0x400C && range_is(&fl, 0, 0xBFFFF), "0x400c");
    let r = write_status(&mut fl, 0x01, &[0x18]);
    assert!(r.is_err() && r.unwrap_err().to_string().contains("does not list"), "0x18 on Puya refused");
    let r = write_status(&mut fl, 0x01, &[0x00]);
    assert!(r.is_ok() && status(&mut fl) == 0x4000 && range_is(&fl, 0, 0xFFFFF), "one byte 0x00 leaves 0x4000");
    let r = write_status(&mut fl, 0x01, &[0x00, 0x00]);
    assert!(r.is_ok() && none(&fl), "0x0000 two bytes: nothing protected");
    let mut fl = Flash::new(0x80000);
    let r = write_status(&mut fl, 0x01, &[0x04, 0x40]);
    assert!(fl.mid == 0x136085 && r.is_ok() && range_is(&fl, 0, 0x6FFFF), "P25Q40 0x4004");

    let mut fl = Flash::new_jedec(FLASH_MEM, [0xEF, 0x40, 0x14]);
    let r = write_status(&mut fl, 0x01, &[0x1C]);
    assert!(r.is_ok() && status(&mut fl) == 0x1C && none(&fl), "unknown mid: status kept, nothing protected");
    let _ = program(&mut fl, 0x1000, &[0x00]);
    assert_eq!(fl.mem[0x1000], 0x00, "program there written");
}

#[test]
fn flash_on_write() {
    let mut fl = Flash::new_jedec(0x80000, [0xC8, 0x60, 0x14]);
    let seen: Rc<RefCell<Vec<(String, usize, usize)>>> = Rc::new(RefCell::new(vec![]));
    let s2 = seen.clone();
    fl.on_write = Some(Rc::new(move |op: &str, addr: usize, n: usize| s2.borrow_mut().push((op.to_string(), addr, n))));
    let mut cmd = |b: &[u8]| {
        let _ = fl.select(true);
        for &x in b {
            fl.clock(x).unwrap();
        }
        fl.select(false).unwrap();
    };
    cmd(&[0x06]);
    cmd(&[0x02, 0x04, 0x00, 0x00, 0x12, 0x34]);
    cmd(&[0x06]);
    cmd(&[0x20, 0x04, 0x10, 0x00]);
    cmd(&[0x06]);
    cmd(&[0x01, 0x18]);
    cmd(&[0x06]);
    cmd(&[0x02, 0x00, 0x10, 0x00, 0x00]);
    cmd(&[0x06]);
    cmd(&[0xD8, 0x03, 0xF0, 0x00]);
    let want = vec![
        ("program".to_string(), 0x40000, 2),
        ("erase".to_string(), 0x41000, 0x1000),
        ("protected".to_string(), 0x1000, 1),
        ("protected".to_string(), 0x30000, 0x10000),
    ];
    assert_eq!(*seen.borrow(), want);
}

/// Machine::on_flash_write sees the same writes, with the machine.
#[test]
fn machine_on_flash_write() {
    let mut fl = Flash::new_jedec(0x80000, [0xC8, 0x60, 0x14]);
    fl.mem[8] = 0x4B;
    let mut m = Machine::new(fl, -1, 1).unwrap();
    let seen: Rc<RefCell<Vec<(String, usize, usize, u32)>>> = Rc::new(RefCell::new(vec![]));
    let s2 = seen.clone();
    m.on_flash_write = Some(Rc::new(move |m: &mut Machine, op: &str, addr: usize, n: usize| {
        s2.borrow_mut().push((op.to_string(), addr, n, m.r[15]))
    }));
    m.r[15] = 0x1234;
    for c in [&[0x06u8][..], &[0x02, 0x04, 0x00, 0x00, 0x12, 0x34]] {
        m.reg_write(0x0D, 1, 0).unwrap();
        for &b in c {
            m.reg_write(0x0C, 1, b as u32).unwrap();
        }
        m.reg_write(0x0D, 1, 1).unwrap();
    }
    assert_eq!(*seen.borrow(), vec![("program".to_string(), 0x40000, 2, 0x1234)]);
}

#[test]
fn xip_while_chip_select_low() {
    let mut fl = Flash::new_jedec(FLASH_MEM, [0xC8, 0x60, 0x14]);
    fl.mem[8] = 0x4B;
    fl.mem[0x10000..0x10004].copy_from_slice(&[0x11, 0x22, 0x33, 0x44]);
    let mut m = Machine::new(fl, -1, 1).unwrap();
    assert_eq!(m.read(0x10000, 4).unwrap(), 0x44332211, "chip select high");
    let _ = m.write(REG_BASE + 0x0D, 1, 0x00);
    assert_eq!(m.read(0x10000, 4).unwrap(), 0x44332211, "chip select low and nothing sent");
    let _ = m.write(REG_BASE + 0x0C, 1, 0x02);
    let e = super::err_text(&m.read(0x10000, 4));
    assert!(e.contains("chip select low") && e.contains("0x02"), "XIP read with the chip select low: {}", e);
    let _ = m.write(REG_BASE + 0x0D, 1, 0x01);
    assert_eq!(m.read(0x10000, 4).unwrap(), 0x44332211, "served again");
    let _ = m.write(REG_BASE + 0x0D, 1, 0x00);
    let _ = m.write(REG_BASE + 0x0C, 1, 0xAB);
    assert_eq!(m.read(0x10000, 4).unwrap(), 0x44332211, "after a bare one-byte command");
}

#[test]
fn sram_fill_known() {
    assert_eq!(sram_fill(1, 8), vec![0x21, 0x01, 0xc5, 0x4f, 0xd1, 0xd0, 0x1a, 0xb2]);
}
