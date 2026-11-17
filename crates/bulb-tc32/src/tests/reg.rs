use super::*;
use crate::sim::{self, Op};

#[test]
fn a_read_modify_write_keeps_the_masked_bits_and_sets_the_others() {
    let _s = sim::session();
    sim::with(|c| {
        c.poke8(0x100, 0xf5);
        c.poke32(0x104, 0xffff_ffff);
        c.poke32(0x108, 0x1234_5678);
    });
    modify8(0x100, 0x0f, 0x11);
    modify16(0x104, 0x00ff, 0x0101);
    modify32(0x108, 0xff00_ff00, 0x0211_0011);
    set8(0x10c, 0x81);
    set8(0x10c, 0x01);
    clear8(0x100, 0x10);
    assert_eq!(
        sim::with(|c| c.writes()),
        vec![Op::W8(0x100, 0x15), Op::W16(0x104, 0x01ff), Op::W32(0x108, 0x1211_5611), Op::W8(0x10c, 0x81), Op::W8(0x10c, 0x81), Op::W8(0x100, 0x05)]
    );
}

#[test]
fn an_analog_register_is_written_and_read_through_the_port_with_irqs_off() {
    let _s = sim::session();
    sim::with(|c| {
        c.poke8(IRQ_ENABLE, 1);
        c.set_analog(0x8a, 0xc3);
    });
    analog_modify(0x8a, 0x40, 0x15);
    assert_eq!(sim::with(|c| c.analog(0x8a)), 0x55);
    assert_eq!(sim::with(|c| c.analog_writes()), vec![(0x8a, 0x55)]);
    let w = sim::with(|c| c.writes());
    assert_eq!(
        w,
        vec![
            Op::W8(IRQ_ENABLE, 0),
            Op::W8(ANA_ADDR, 0x8a),
            Op::W8(ANA_CTRL, 0x40),
            Op::W8(ANA_CTRL, 0x00),
            Op::W8(IRQ_ENABLE, 1),
            Op::W8(IRQ_ENABLE, 0),
            Op::W8(ANA_ADDR, 0x8a),
            Op::W8(ANA_DATA, 0x55),
            Op::W8(ANA_CTRL, 0x60),
            Op::W8(ANA_CTRL, 0x00),
            Op::W8(IRQ_ENABLE, 1),
        ]
    );
}

fn busy_port(reads: u32) {
    let mut left = 0;
    sim::with(|c| {
        c.on(move |c, op| match op {
            Op::W8(ANA_CTRL, 0x40 | 0x60) => {
                left = reads;
                c.poke8(ANA_CTRL, c.peek8(ANA_CTRL) | 0x01);
            }
            Op::R8(ANA_CTRL, _) => {
                left = left.saturating_sub(1);
                if left == 0 {
                    c.poke8(ANA_CTRL, c.peek8(ANA_CTRL) & !0x01);
                }
            }
            _ => {}
        })
    });
}

#[test]
fn the_analog_port_is_waited_for_while_it_is_busy() {
    let _s = sim::session();
    sim::with(|c| c.set_analog(0x10, 0x42));
    busy_port(4);
    assert_eq!(analog_read(0x10), 0x42);
    assert_eq!(sim::with(|c| c.log.iter().filter(|o| matches!(o, Op::R8(ANA_CTRL, _))).count()), 5);
    sim::with(|c| c.log.clear());
    busy_port(3);
    analog_write(0x11, 0x99);
    assert_eq!(sim::with(|c| c.log.iter().filter(|o| matches!(o, Op::R8(ANA_CTRL, _))).count()), 4);
    assert_eq!(sim::with(|c| c.analog(0x11)), 0x99);
}

#[test]
fn a_delay_waits_more_than_its_microseconds_of_ticks() {
    let _s = sim::session();
    sim::with(|c| c.tick = 1000);
    delay_us(5);
    let t = sim::with(|c| c.ticks_read());
    assert_eq!(t.first().copied(), Some(1016));
    assert_eq!(t.last().copied(), Some(1016 + 5 * 16 + 16));
    assert_eq!(tick(), 1016 + 5 * 16 + 32);
}

#[test]
fn the_pwm_registers_of_a_channel_are_four_octets_apart() {
    assert_eq!((pwm_cmp(0), pwm_max(0)), (0x794, 0x796));
    assert_eq!((pwm_cmp(5), pwm_max(5)), (0x7a8, 0x7aa));
}
