use super::*;
use crate::sim;
use zigbee::hal::Trace;

pub(crate) fn entry(i: usize) -> [u32; 4] {
    let at = 8 + i * 16;
    [ring::ZB_TRACE.read32(at), ring::ZB_TRACE.read32(at + 4), ring::ZB_TRACE.read32(at + 8), ring::ZB_TRACE.read32(at + 12)]
}

#[test]
fn the_ring_starts_with_its_magic_and_keeps_each_event_with_its_tick() {
    let _s = sim::session();
    assert_eq!(ring::ZB_TRACE.read32(0), 0x5452_4345);
    ring::ZB_TRACE.write32(4, 0);
    sim::with(|c| c.tick = 0x1000);
    record(Trace { kind: 0x12, detail: 0x34, seq: 0x56, src: 0x789a, peer: 0xbcde, aux: 0xf012_3456 });
    assert_eq!(entry(0), [0x1010, 0x0056_3412, 0xbcde_789a, 0xf012_3456]);
    assert_eq!(ring::ZB_TRACE.read32(4), 1);
    let w = sim::with(|c| c.writes());
    assert_eq!(w.first(), Some(&sim::Op::W8(crate::reg::IRQ_ENABLE, 0)));
    span(BLE, 3, 0x4321, 0x1000);
    assert_eq!(entry(1), [0x1030, 0x0000_0311, 0x4321_0000, 0x20]);
}

#[test]
fn the_ring_wraps_after_512_events() {
    let _s = sim::session();
    ring::ZB_TRACE.write32(4, ring::ENTRIES as u32 - 1);
    span(FLASH, 1, 7, 0);
    span(MAC, 2, 8, 0);
    assert_eq!(entry(ring::ENTRIES - 1)[1], 0x0000_0110);
    assert_eq!(entry(0)[1], 0x0000_0212);
    assert_eq!(ring::ZB_TRACE.read32(4), ring::ENTRIES as u32 + 1);
    assert_eq!(ring::ZB_TRACE.read32(0), 0x5452_4345);
}

#[test]
fn a_transmission_is_summed_up_in_attempts_busy_attempts_and_microseconds() {
    let _s = sim::session();
    sim::with(|c| c.tick = 0x10_0000);
    assert_eq!(tx_aux([3, 1, 0x10_0000 - 160]), 0x000b_0103);
    assert_eq!(tx_aux([300, 256, 0]), 0xffff_ffff);
    assert_eq!(tx_aux([255, 255, 0x10_0000 + 16 - 0xffff * 16]), 0xffff_ffff);
    assert_eq!(tx_aux([1, 0, 0x10_0040 - 0xfffe * 16]), 0xfffe_0001);
}

#[test]
fn a_flash_program_and_erase_are_traced_with_their_sector() {
    let _s = sim::session();
    ring::ZB_TRACE.write32(4, 0);
    crate::flash::write(0xf_5010, &[1]);
    crate::flash::erase_sector(0xf_6000);
    assert_eq!(entry(0)[1], 0x0000_0010);
    assert_eq!(entry(0)[2], 0x00f5_0000);
    assert_eq!(entry(1)[1], 0x0000_0110);
    assert_eq!(entry(1)[2], 0x00f6_0000);
}

pub(crate) fn rewind() {
    ring::ZB_TRACE.write32(4, 0);
}
