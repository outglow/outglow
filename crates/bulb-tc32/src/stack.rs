//! How deep the stacks have gone: `boot/start.S` fills the RAM from the end
//! of `.bss` to the top with a pattern before `main`; whatever was never
//! written still holds it. The main stack grows down from `_stack_end_`
//! towards `.bss`, the interrupt stack from `_irq_stack_top_` (1 KB).

use core::ptr::{addr_of, read_volatile};

const PATTERN: u32 = 0xa5a5_a5a5;

extern "C" {
    static _end_bss_: u8;
    static _stack_end_: u8;
    static _irq_stack_top_: u8;
}

/// Bytes at the bottom of [from, to) that still hold the pattern.
fn untouched(from: usize, to: usize) -> u32 {
    let mut a = from;
    // SAFETY: word reads of SRAM inside the stack area the linker script
    // sets aside; nothing else lives there.
    while a < to && unsafe { read_volatile(a as *const u32) } == PATTERN {
        a += 4;
    }
    (a - from) as u32
}

/// Bytes of the main stack (with the free RAM below it) and of the
/// interrupt stack that have never been used since the start.
pub fn free() -> (u32, u32) {
    // SAFETY: only the addresses of linker symbols are taken.
    let (bss, main_top, irq_top) =
        unsafe { (addr_of!(_end_bss_) as usize, addr_of!(_stack_end_) as usize, addr_of!(_irq_stack_top_) as usize) };
    (untouched(bss, main_top), untouched(main_top, irq_top))
}
