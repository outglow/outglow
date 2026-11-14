// xip_slot_test.go
use crate::{Flash, Machine, FLASH_MEM, REG_BASE};

#[test]
fn xip_slot() {
    let addrs = [0x100usize, 0x20100, 0x30000, 0x40000, 0x77000];
    let machine = |slot: usize| {
        let mut fl = Flash::new(FLASH_MEM);
        fl.mem[slot + 8] = 0x4B;
        fl.mem[slot + 0x0C..slot + 0x10].copy_from_slice(&[0, 0, 0, 0]);
        for &a in &addrs {
            fl.mem[a..a + 4].copy_from_slice(&(a as u32).to_le_bytes());
        }
        Machine::new(fl, -1, 1).unwrap()
    };
    let mut m = machine(0);
    assert_eq!(m.boot_slot, 0);
    for &a in &addrs {
        assert_eq!(m.read(a as u32, 4).unwrap(), a as u32, "slot A");
    }
    let mut m = machine(0x20000);
    assert_eq!(m.boot_slot, 0x20000);
    assert_eq!(m.read(0x100, 4).unwrap(), 0x20100, "slot B: 0x100");
    for a in [0x20100u32, 0x30000] {
        let e = super::err_text(&m.read(a, 4));
        assert!(e.contains("not documented"), "slot B: {:#x}: {}", a, e);
    }
    for a in [0x40000u32, 0x77000] {
        assert_eq!(m.read(a, 4).unwrap(), a, "slot B");
    }
    m.write(REG_BASE + 0x608, 4, (1 << 24) | 0x100).unwrap();
    assert_eq!(u32::from_le_bytes(m.sram[0x100..0x104].try_into().unwrap()), 0x20100, "slot B: the loader");
}
