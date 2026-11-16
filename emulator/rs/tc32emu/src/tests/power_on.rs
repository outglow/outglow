use crate::{Flash, Machine, FLASH_MEM};

#[test]
fn power_on_analog() {
    let mut fl = Flash::new(FLASH_MEM);
    fl.mem[8] = 0x4B;
    let mut m = Machine::new(fl, -1, 1).unwrap();
    assert_eq!(&m.analog[0x3A..0x3D], &[0, 0, 0x0F], "power-on");
    let defaults = [0x20u8, 0, 0, 0, 0xFF];
    assert_eq!(&m.analog[0x35..0x3A], &defaults, "power-on: 0x35-0x39");
    m.analog[0x3A..0x3D].copy_from_slice(&[0x12, 0x34, 0x56]);
    m.analog[0x35..0x3A].copy_from_slice(&[0x71, 0x72, 0x73, 0x74, 0x75]);
    m.reset(-1, false).unwrap();
    assert_eq!(&m.analog[0x3A..0x3D], &[0x12, 0x34, 0x56], "after a reset");
    assert_eq!(&m.analog[0x35..0x3A], &defaults, "after a reset: 0x35-0x39");
    let fl = std::mem::replace(&mut m.flash, Flash::new(0x1000));
    let m = Machine::new(fl, -1, 1).unwrap();
    assert!(m.analog[0x3C] == 0x0F && m.analog[0x3A] == 0, "a new power-on");
}
