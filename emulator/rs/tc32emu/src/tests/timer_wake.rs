use crate::{Flash, Machine, FLASH_MEM};

#[test]
fn timer_wake() {
    let mut fl = Flash::new(FLASH_MEM);
    fl.mem[8] = 0x4B;
    let mut m = Machine::new(fl, -1, 1).unwrap();
    let tick_ms = 1.0 / 32.768;
    for ticks in [1u32, 7, 33, 100, 491] {
        m.cycles += 1234;
        let wake = m.k32_now().wrapping_add(ticks);
        m.reg_write(0x74C, 4, wake).unwrap();
        m.reg_write(0x74B, 1, 0x08).unwrap();
        m.analog[0x26] = 0x20;
        let start = m.ms();
        m.reg_write(0x6F, 1, 0x81).unwrap();
        let end = m.cycles + (50.0 * m.cpu_hz as f64 / 1000.0) as i64;
        while m.asleep.is_some() && m.cycles < end {
            m.sleep_until(end);
        }
        assert!(m.asleep.is_none(), "{} ticks: no wake within 50 ms", ticks);
        let slept = m.ms() - start;
        assert!(
            m.k32_now().wrapping_sub(wake) < 0x80000000 && m.analog[0x44] & 0x02 != 0,
            "{} ticks: awake before the wake value",
            ticks
        );
        assert!(slept <= ticks as f64 * tick_ms + 1e-6, "{} ticks: slept {:.4} ms", ticks, slept);
        m.analog[0x44] = 0;
    }
}
