use super::thumb::*;
use crate::{Machine, MODE_IRQ, MODE_SVC};

fn armed() -> Machine {
    let mut m = machine();
    m.regs[0x643] = 1;
    m.ibit = 0;
    m.regs[0x640] = 0x10;
    m
}

#[test]
fn a_latched_unmasked_source_is_pending() {
    let mut m = armed();
    m.latch_irq(4);
    assert!(m.irq_pending());
}

#[test]
fn a_masked_source_is_not_pending() {
    let mut m = armed();
    m.regs[0x640] = 0;
    m.latch_irq(4);
    assert!(!m.irq_pending());
}

#[test]
fn the_cpsr_i_bit_blocks_interrupts() {
    let mut m = armed();
    m.ibit = 1;
    m.latch_irq(4);
    assert!(!m.irq_pending());
}

#[test]
fn the_master_enable_blocks_interrupts() {
    let mut m = armed();
    m.regs[0x643] = 0;
    m.latch_irq(4);
    assert!(!m.irq_pending());
}

#[test]
fn a_source_in_the_upper_mask_bytes_is_pending() {
    let mut m = armed();
    m.regs[0x640] = 0;
    m.regs[0x642] = 0x10;
    m.latch_irq(20);
    assert!(m.irq_pending());
}

#[test]
fn irq_entry_jumps_to_0x10_in_irq_mode_with_interrupts_off() {
    let mut m = armed();
    m.r[15] = CODE + 0x40;
    m.mode = MODE_SVC;
    m.c = 1;
    let cpsr = m.cpsr();
    m.latch_irq(4);
    m.take_irq();
    assert_eq!((m.r[15], m.mode, m.ibit, m.r[14]), (0x10, MODE_IRQ, 1, CODE + 0x40));
    assert_eq!(m.spsr[0], cpsr);
}

#[test]
fn treti_pops_the_pc_and_restores_the_saved_mode() {
    let mut m = armed();
    m.r[15] = CODE + 0x40;
    m.latch_irq(4);
    m.take_irq();
    m.r[13] = STACK;
    set_sram_u32(&mut m, STACK, 0x0001_2340);
    let off = (CODE + 0x100 - crate::SRAM_BASE) as usize;
    m.sram[off..off + 2].copy_from_slice(&0x6900u16.to_le_bytes());
    m.r[15] = CODE + 0x100;
    m.step().unwrap();
    assert_eq!((m.mode, m.r[15], m.ibit), (MODE_SVC, 0x0001_2340, 0));
    assert_eq!(m.banked(MODE_IRQ)[0], STACK + 4);
}

#[test]
fn the_timer_status_is_a_level_interrupt_source() {
    let mut m = armed();
    m.regs[0x640] = 0x01;
    assert!(!m.irq_pending());
    m.regs[0x623] = 0x01;
    assert!(m.irq_pending());
    m.regs[0x623] = 0;
    assert!(!m.irq_pending());
}
