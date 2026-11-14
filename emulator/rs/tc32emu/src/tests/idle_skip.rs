// idle_skip_test.go
use std::cell::RefCell;
use std::rc::Rc;

use crate::isa::to_tc32;
use crate::{stop_err, Flash, Machine, Range, CPU_HZ, FLASH_MEM, IRQ_STIMER, MODE_SVC, STIMER_HZ};

const GPIO_SRC: u32 = 18;
const IDLE: [Range; 1] = [Range { lo: 0x100, hi: 0x120 }];

fn machine(code: &[u16], words: &[u32]) -> Machine {
    let mut fl = Flash::new(FLASH_MEM);
    fl.mem[8] = 0x4B;
    let mut a = 0x100;
    for &hw in code {
        fl.mem[a..a + 2].copy_from_slice(&to_tc32(hw).to_le_bytes());
        a += 2;
    }
    for &w in words {
        fl.mem[a..a + 4].copy_from_slice(&w.to_le_bytes());
        a += 4;
    }
    fl.mem[0x10..0x12].copy_from_slice(&to_tc32(0xE7FE).to_le_bytes());
    let mut m = Machine::new(fl, -1, 1).unwrap();
    m.r[15] = 0x100;
    m.set_cpsr(MODE_SVC).unwrap();
    m.reg_write(0x74A, 1, 0x02).unwrap();
    m.reg_write(0x748, 1, 0x04).unwrap();
    m
}

fn compare_in(m: &mut Machine, seconds: f64) {
    let v = m.stimer_now().wrapping_add((seconds * STIMER_HZ as f64) as u32);
    m.reg_write(0x744, 4, v).unwrap();
}

fn run_to_irq(m: &mut Machine, limit: i64) -> Option<i64> {
    m.hooks.insert(0x10, Rc::new(|_m: &mut Machine| Err(stop_err("irq"))));
    match m.run(limit, &IDLE, 32) {
        Err(e) if e.is_stop() => Some(m.cycles),
        Err(e) => panic!("{}", e),
        Ok(()) => None,
    }
}

#[test]
fn idle_skip() {
    let mut m = machine(&[0x20C8, 0x3801, 0xD1FD, 0x4902, 0x2201, 0x700A, 0xE7FE, 0x46C0], &[0x00800643]);
    m.reg_write(0x643, 1, 0).unwrap();
    m.reg_write(0x642, 1, 1 << (GPIO_SRC - 16)).unwrap();
    m.latch_irq(GPIO_SRC);
    compare_in(&mut m, 1.0);
    let at = run_to_irq(&mut m, 3 * CPU_HZ);
    assert!(matches!(at, Some(c) if c < 20_000), "0x800643 = 0, an interrupt pending: {:?}", at);

    let mut m = machine(&[0x20C8, 0x3801, 0xD1FD, 0xBBC3, 0xE7FE], &[]);
    m.set_cpsr(MODE_SVC | 0x80).unwrap();
    m.r[3] = MODE_SVC;
    m.reg_write(0x643, 1, 1).unwrap();
    m.reg_write(0x642, 1, 1 << (GPIO_SRC - 16)).unwrap();
    m.latch_irq(GPIO_SRC);
    compare_in(&mut m, 1.0);
    let at = run_to_irq(&mut m, 3 * CPU_HZ);
    assert!(matches!(at, Some(c) if c < 20_000), "CPSR I set, an interrupt pending: {:?}", at);

    let mut m = machine(&[0x3001, 0xE7FD], &[]);
    m.reg_write(0x643, 1, 1).unwrap();
    m.reg_write(0x642, 1, 1 << (IRQ_STIMER - 16)).unwrap();
    compare_in(&mut m, 0.010);
    let at = run_to_irq(&mut m, CPU_HZ);
    assert!(matches!(at, Some(c) if c >= CPU_HZ * 9 / 1000) && m.r[0] < 1000, "interrupts on, none pending: {:?}", at);
    let at = at.unwrap();
    assert!(m.idle_skipped * 10 >= at * 9 && m.idle_skipped <= at, "IdleSkipped {} of {}", m.idle_skipped, at);

    let cap = CPU_HZ / 2000;
    let timer1 = |m: &mut Machine, irq_mask: u32| {
        m.reg_write(0x643, 1, 1).unwrap();
        m.reg_write(0x640, 1, irq_mask).unwrap();
        m.reg_write(0x642, 1, 1 << (IRQ_STIMER - 16)).unwrap();
        m.reg_write(0x628, 4, cap as u32).unwrap();
        m.reg_write(0x634, 4, 0).unwrap();
        m.reg_write(0x620, 1, 0x08).unwrap();
    };
    let mut m = machine(&[0x3001, 0xE7FD], &[]);
    timer1(&mut m, 1 << 1);
    let start = m.cycles;
    compare_in(&mut m, 0.010);
    let at = run_to_irq(&mut m, CPU_HZ).expect("taken");
    assert!(at - start >= cap && at - start < cap + 100 && m.r[0] < 1000, "Timer1 unmasked: {} ", at - start);

    let mut m = machine(&[0x3001, 0xE7FD], &[]);
    timer1(&mut m, 1 << 1);
    let taken = Rc::new(RefCell::new(Vec::<i64>::new()));
    let t2 = taken.clone();
    m.hooks.insert(
        0x10,
        Rc::new(move |mm: &mut Machine| {
            t2.borrow_mut().push(mm.cycles);
            mm.reg_write(0x623, 1, 0x02)?;
            mm.set_cpsr(MODE_SVC)?;
            mm.r[15] = 0x100;
            Ok(())
        }),
    );
    compare_in(&mut m, 0.020);
    let t = m.cycles + 10 * cap + cap / 2;
    m.run(t, &IDLE, 32).unwrap();
    let taken = taken.borrow();
    let mut spaced = taken.len() == 10;
    for i in 1..taken.len() {
        let g = taken[i] - taken[i - 1];
        if g < cap - 100 || g > cap + 100 {
            spaced = false;
        }
    }
    assert!(spaced, "Timer1's interrupts through the idle loop: {:?}", taken);

    let mut m = machine(&[0x3001, 0xE7FD], &[]);
    timer1(&mut m, 0);
    let start = m.cycles;
    compare_in(&mut m, 0.010);
    let at = run_to_irq(&mut m, CPU_HZ).expect("taken");
    assert!(
        at - start >= CPU_HZ * 9 / 1000 && m.r[0] < 1000 && m.regs[0x623] & 0x02 != 0,
        "Timer1 masked: {}",
        at - start
    );
}
