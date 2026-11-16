use std::cell::Cell;
use std::rc::Rc;

use crate::{Flash, Machine, IRQ_STIMER, MODE_SVC};

#[test]
fn check_hooks() {
    let cfg = crate::Config::env();
    let mut fl = Flash::new(cfg.flash_size);
    fl.mem[8] = 0x4B;
    let mut m = Machine::new(fl, -1, 1).unwrap();
    m.sysclk_hook = Some(Rc::new(|_m: &mut Machine| 16_000_000));
    m.reg_write(0x66, 1, 0x20).unwrap();
    assert_eq!(m.cpu_hz, 16_000_000, "CPU clock with the hook");
    m.sysclk_hook = None;
    m.reg_write(0x66, 1, 0x20).unwrap();
    assert_eq!(m.cpu_hz, 48_000_000, "CPU clock without the hook");
    let feeds = Rc::new(Cell::new(0));
    let f2 = feeds.clone();
    m.on_feed = Some(Rc::new(move |_m: &mut Machine| f2.set(f2.get() + 1)));
    m.reg_write(0x623, 1, 0x08).unwrap();
    m.reg_write(0x620, 4, 1 << 27).unwrap();
    m.reg_write(0x623, 1, 0x01).unwrap();
    assert_eq!(feeds.get(), 2, "feeds seen");
    m.latch_irq(IRQ_STIMER);
    assert!(m.irq_sources() & (1 << IRQ_STIMER) != 0, "latched source not in the sources");
    m.set_banked(MODE_SVC, [0x848000, 0x1234]);
    assert_eq!(m.banked(MODE_SVC), [0x848000, 0x1234]);
    m.sram_code_end_hook = Some(Rc::new(|_m: &mut Machine| 0x400));
    assert_eq!(m.sram_code_end(), 0x400, "SRAM code end with the hook");
}

#[test]
fn reentrant_hooks() {
    let mut m = super::rbg_machine();
    let seen = Rc::new(Cell::new(0));
    let s2 = seen.clone();
    m.reg_write_hook = Some(Rc::new(move |m: &mut Machine, o: usize, _size: usize, val: u32| {
        s2.set(s2.get() + 1);
        if o == 0x74F {
            let cur = m.regs[0x74A] as u32 | if val & 1 != 0 { 2 } else { 0 };
            m.reg_write(0x74A, 1, cur)?;
            m.regs[0x74F] = val as u8;
            return Ok(true);
        }
        Ok(false)
    }));
    m.reg_write(0x74F, 1, 1).unwrap();
    assert_eq!(seen.get(), 2);
    assert!(m.stimer_on());
}
