#[cfg(target_os = "none")]
use core::ptr::{addr_of, read_volatile};

#[cfg(target_os = "none")]
const PATTERN: u32 = 0xa5a5_a5a5;

#[cfg(target_os = "none")]
extern "C" {
    static _end_bss_: u8;
    static _stack_end_: u8;
    static _irq_stack_top_: u8;
}

#[cfg(target_os = "none")]
fn untouched(from: usize, to: usize) -> u32 {
    let mut a = from;
    while a < to && unsafe { read_volatile(a as *const u32) } == PATTERN {
        a += 4;
    }
    (a - from) as u32
}

#[cfg(target_os = "none")]
pub fn free() -> (u32, u32) {
    let (bss, main_top, irq_top) = (addr_of!(_end_bss_) as usize, addr_of!(_stack_end_) as usize, addr_of!(_irq_stack_top_) as usize);
    (untouched(bss, main_top), untouched(main_top, irq_top))
}

#[cfg(not(target_os = "none"))]
pub fn free() -> (u32, u32) {
    (0, 0)
}
