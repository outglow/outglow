use std::cell::Cell;
use std::rc::Rc;

use crate::{Flash, Machine, FLASH_MEM, REG_BASE};

const PORT: usize = 2;
const BIT: u32 = 2;

struct T {
    outside: Rc<Cell<i64>>,
}

impl T {
    fn machine(&self, pull: u8, cpf: i64, rpct: i64) -> Machine {
        let mut fl = Flash::new(FLASH_MEM);
        fl.mem[8] = 0x4B;
        fl.mem[0x0C..0x10].copy_from_slice(&[0, 0, 0, 0]);
        let mut m = Machine::new(fl, -1, 1).unwrap();
        m.pad_c_pf = cpf;
        m.pad_r_pct = rpct;
        m.analog[0x12] = pull << 4;
        m.analog[0xC0] = 0xFF;
        m.regs[0x590 + 6] = 0xFF;
        m.regs[0x590 + 2] = 0xFF;
        self.outside.set(-1);
        let o = self.outside.clone();
        m.pad_levels_hook = Some(Rc::new(move |_m: &mut Machine, p: usize, mut lvl: u8, mut flt: u8| {
            if p == PORT && o.get() >= 0 {
                lvl = (lvl & !(1 << BIT)) | ((o.get() as u8) << BIT);
                flt &= !(1 << BIT);
            }
            (lvl, flt)
        }));
        m
    }
    fn release(&self, m: &mut Machine, level: i64, cycles: i64) -> i64 {
        self.outside.set(level);
        m.hold_pads(PORT);
        self.outside.set(-1);
        m.hold_pads(PORT);
        m.cycles += cycles;
        read(m)
    }
}

fn read(m: &mut Machine) -> i64 {
    ((m.read(REG_BASE + 0x590, 1).unwrap() >> BIT) & 1) as i64
}

#[test]
fn pad_rc() {
    let t = T { outside: Rc::new(Cell::new(-1)) };
    let hz = t.machine(2, 0, 100).cpu_hz;
    let cycles_for = |ns: i64| {
        let mut c = 0i64;
        loop {
            if c * 1_000_000_000 / hz >= ns {
                return c;
            }
            c += 1;
        }
    };
    let n = cycles_for(1204 * 100_000 * 40 / 1_000_000);
    assert_eq!(t.release(&mut t.machine(2, 0, 100), 1, 0), 0, "model off: low at once");
    assert_eq!(t.release(&mut t.machine(2, 40, 100), 1, 0), 1, "still high right after the release");
    assert_eq!(t.release(&mut t.machine(2, 40, 100), 1, n - 1), 1, "still high one cycle before the time");
    let mut m = t.machine(2, 40, 100);
    assert!(t.release(&mut m, 1, n) == 0 && read(&mut m) == 0, "low at the time, and it stays low");
    let mut m = t.machine(2, 40, 100);
    t.release(&mut m, 1, 10);
    m.cycles += n - 10;
    assert_eq!(read(&mut m), 0, "the time runs from the release");
    let n2 = cycles_for(1204 * 100_000 * 60 * 150 / 100 / 1_000_000);
    let mut m = t.machine(2, 60, 150);
    assert_eq!(t.release(&mut m, 1, n2 - 1), 1, "60 pF at 150 %: high before the time");
    m.cycles += 1;
    assert_eq!(read(&mut m), 0, "60 pF at 150 %: low at the time");
    let mut m = t.machine(2, 40, 100);
    t.release(&mut m, 1, 5);
    t.outside.set(1);
    assert_eq!(read(&mut m), 1, "driven high again: high");
    t.outside.set(-1);
    m.hold_pads(PORT);
    m.cycles += n - 1;
    assert_eq!(read(&mut m), 1, "the time starts again");
    m.cycles += 1;
    assert_eq!(read(&mut m), 0, "low once it has passed");
    let n3 = cycles_for(1204 * 10_000 * 40 / 1_000_000);
    assert_eq!(t.release(&mut t.machine(3, 40, 100), 0, n3 - 1), 0, "10 kOhm up: still low before the time");
    assert_eq!(t.release(&mut t.machine(3, 40, 100), 0, n3), 1, "10 kOhm up: high at the time");
    let mut m = t.machine(2, 40, 100);
    t.release(&mut m, 1, n);
    assert_eq!(t.release(&mut m, 0, 0), 0, "at the pull's own level already");
    let mut m = t.machine(2, 40, 100);
    t.outside.set(1);
    m.hold_pads(PORT);
    t.outside.set(0);
    m.hold_pads(PORT);
    assert_eq!(read(&mut m), 1, "driven from outside to the pull's own level: taken as released");
    let mut m = t.machine(2, 40, 100);
    t.outside.set(1);
    m.hold_pads(PORT);
    t.outside.set(-1);
    m.write(REG_BASE + 0x590 + 3, 1, 0x00).unwrap();
    m.write(REG_BASE + 0x590 + 2, 1, 0xFF & !(1 << BIT)).unwrap();
    assert_eq!(read(&mut m), 0, "the pad's own output low: low at once");
    m.write(REG_BASE + 0x590 + 2, 1, 0xFF).unwrap();
    assert_eq!(read(&mut m), 0, "released from its own output: already at the pull's level");

    let mut m = t.machine(3, 40, 100);
    m.regs[0x5B5] |= 0x08;
    m.regs[0x590 + 7] = 1 << BIT;
    m.regs[0x590 + 4] = 0;
    t.outside.set(0);
    m.hold_pads(PORT);
    m.gpio_irq_update();
    let l = m.irq_latch() & !(1 << 18);
    m.set_irq_latch(l);
    t.outside.set(-1);
    m.gpio_irq_update();
    assert!(read(&mut m) == 0 && m.irq_latch() & (1 << 18) == 0, "contact open, pad still low: no interrupt yet");
    m.cycles += n3 - 1;
    m.pad_rc_service();
    assert_eq!(m.irq_latch() & (1 << 18), 0, "one cycle before the time: none yet");
    let due = m.pad_rc_due();
    assert!(matches!(due, Some(d) if (0..=1).contains(&d)), "the pad is due: {:?}", due);
    m.cycles += 1;
    m.pad_rc_service();
    assert!(read(&mut m) == 1 && m.irq_latch() & (1 << 18) != 0, "at the time the pad reads high and the interrupt");
    assert!(m.pad_rc_due().is_none(), "no pad on its way any more");
}
