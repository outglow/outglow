use crate::{Flash, Machine};

#[test]
fn gpio_inputs_and_alias() {
    let cfg = crate::Config::env();
    let mut fl = Flash::new(cfg.flash_size);
    fl.mem[8] = 0x4B;
    let mut m = Machine::new(fl, -1, 1).unwrap();
    let rd = |m: &mut Machine, o: usize| m.reg_read(o, 1).unwrap();
    m.analog[0x0E] = 0x01 | 0x02 << 2 | 0x03 << 4;
    m.regs[0x586] |= 0x10;
    m.regs[0x582] &= !0x10;
    m.regs[0x583] &= !0x10;
    m.regs[0x581] = 0xFF;
    assert_eq!(rd(&mut m, 0x580), 0xED, "port A");
    m.regs[0x581] = 0x0F;
    assert_eq!(rd(&mut m, 0x580), 0x0D, "port A through the input enable");
    m.regs[0x599] = 0xFF;
    let mut seq = vec![];
    for pull in [2u8, 0, 1, 0] {
        m.analog[0x14] = pull;
        seq.push(rd(&mut m, 0x598) & 1);
    }
    assert_eq!(seq, vec![0, 0, 1, 1], "port D pin 0 hold");
    m.reset(0, false).unwrap();
    m.analog[0x14] = 0;
    m.regs[0x599] = 0xFF;
    assert_eq!(rd(&mut m, 0x598) & 1, 1, "after a reset a floating pad must read 1");
    m.analog[0x12] = 0xFF;
    m.analog[0x13] = 0xFF;
    m.analog[0xC0] = 0x03;
    assert_eq!(rd(&mut m, 0x590), 0x03, "port C through analog 0xc0");
    m.analog[0x11] = 0x03 << 6;
    m.regs[0x589] = 0x80;
    assert_eq!(rd(&mut m, 0x588), 0x80, "port B");
    m.reset(0, false).unwrap();
    m.regs[0x599] = 0xFF;
    let v = m.regs[0x59E] | 0x06;
    m.reg_write(0x59E, 1, v as u32).unwrap();
    let v = m.regs[0x59B] & !0x02;
    m.reg_write(0x59B, 1, v as u32).unwrap();
    let v = m.regs[0x59A] & !0x02;
    m.reg_write(0x59A, 1, v as u32).unwrap();
    let v = m.regs[0x59A] | 0x02;
    m.reg_write(0x59A, 1, v as u32).unwrap();
    for val in [0x02u32 << 4, 0x00] {
        m.reg_write(0xB8, 1, 0x14).unwrap();
        m.reg_write(0xB9, 1, val).unwrap();
        m.reg_write(0xBA, 1, 0x60).unwrap();
    }
    assert_eq!(rd(&mut m, 0x598) & 0x06, 0, "port D after driving/pulling then floating with no read");
    m.write(0x1000ABC, 1, 0x5A).unwrap();
    let a = m.read(0x800ABC, 1).unwrap();
    let b = m.read(0x1000ABC, 1).unwrap();
    assert!(m.regs[0xABC] == 0x5A && a == 0x5A && b == 0x5A, "alias write/read");
    m.write(0x1000664, 4, 100).unwrap();
    m.write(0x1000668, 4, 7).unwrap();
    m.write(0x1000660, 1, 0).unwrap();
    assert_eq!(m.read(0x1000664, 4).unwrap(), 14, "divider through the alias");
}

fn port_a_bit3() -> Machine {
    let mut fl = Flash::new(crate::FLASH_MEM);
    fl.mem[8] = 0x4B;
    let mut m = Machine::new(fl, -1, 1).unwrap();
    m.regs[0x586] |= 0x08;
    m.regs[0x581] |= 0x08;
    m
}

#[test]
fn a_driven_output_reads_back_its_level_through_the_input_register() {
    let mut m = port_a_bit3();
    m.regs[0x582] &= !0x08;
    m.regs[0x583] |= 0x08;
    assert_ne!(m.reg_read(0x580, 1).unwrap() & 0x08, 0, "high");
    m.regs[0x583] &= !0x08;
    assert_eq!(m.reg_read(0x580, 1).unwrap() & 0x08, 0, "low");
}

#[test]
fn input_enable_off_reads_zero_even_when_driven_high() {
    let mut m = port_a_bit3();
    m.regs[0x582] &= !0x08;
    m.regs[0x583] |= 0x08;
    m.regs[0x581] &= !0x08;
    assert_eq!(m.reg_read(0x580, 1).unwrap() & 0x08, 0);
}

#[test]
fn an_undriven_input_follows_its_pull_up_or_pull_down() {
    let mut m = port_a_bit3();
    m.regs[0x582] |= 0x08;
    m.analog[0x0E] = (m.analog[0x0E] & !(3 << 6)) | (1 << 6);
    assert_ne!(m.reg_read(0x580, 1).unwrap() & 0x08, 0, "pull-up");
    m.analog[0x0E] = (m.analog[0x0E] & !(3 << 6)) | (2 << 6);
    assert_eq!(m.reg_read(0x580, 1).unwrap() & 0x08, 0, "pull-down");
}

#[test]
fn pull_masks_bit_by_bit() {
    for pulls in 0..=0xFFFFu32 {
        let (mut up, mut none) = (0u8, 0u8);
        for bit in 0..8 {
            let pull = (pulls >> (2 * bit)) & 3;
            up |= ((pull & 1) as u8) << bit;
            none |= ((pull == 0) as u8) << bit;
        }
        let (lo, hi) = (pulls & 0x5555, (pulls >> 1) & 0x5555);
        assert_eq!((crate::machine::even_bits(lo), crate::machine::even_bits(!(lo | hi) & 0x5555)), (up, none), "{pulls:#06x}");
    }
}
