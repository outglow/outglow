// analog_log_test.go
use crate::{AnalogAccess, Flash, Machine, FLASH_MEM, REG_BASE};

fn write(m: &mut Machine, addr: u8, val: u8) {
    let _ = m.write(REG_BASE + 0xB8, 1, addr as u32);
    let _ = m.write(REG_BASE + 0xB9, 1, val as u32);
    let _ = m.write(REG_BASE + 0xBA, 1, 0x60);
    let _ = m.write(REG_BASE + 0xBA, 1, 0x00);
}

fn read(m: &mut Machine, addr: u8) -> u8 {
    let _ = m.write(REG_BASE + 0xB8, 1, addr as u32);
    let _ = m.write(REG_BASE + 0xBA, 1, 0x40);
    let v = m.read(REG_BASE + 0xB9, 1).unwrap_or(0);
    let _ = m.write(REG_BASE + 0xBA, 1, 0x00);
    v as u8
}

#[test]
fn analog_log() {
    let mut fl = Flash::new(FLASH_MEM);
    fl.mem[8] = 0x4B;
    let mut m = Machine::new(fl, -1, 1).unwrap();
    write(&mut m, 0x12, 0x68);
    assert!(m.analog_log.is_empty() && m.analog_writes.is_empty(), "recorded with LogAnalog off");
    m.log_analog = true;
    m.set_symbols([("start", 0)]);
    write(&mut m, 0x12, 0x28);
    assert_eq!(read(&mut m, 0x12), 0x28, "read back");
    m.xtal_ready = true;
    let v88 = read(&mut m, 0x88);
    let want = vec![
        AnalogAccess { kind: b'w', addr: 0x12, val: 0x28 },
        AnalogAccess { kind: b'r', addr: 0x12, val: 0x28 },
        AnalogAccess { kind: b'r', addr: 0x88, val: v88 },
    ];
    assert_eq!(m.analog_log, want);
    assert!(v88 & 0x80 != 0, "0x88 without the crystal-ready bit");
    let who = m.symbolize(m.r[15]);
    assert!(
        m.analog_writes.len() == 1
            && m.analog_writes[0].addr == 0x12
            && m.analog_writes[0].val == 0x28
            && m.analog_writes[0].who == who,
        "AnalogWrites {:?}",
        m.analog_writes
    );
    m.reset(-1, false).unwrap();
    assert!(m.analog_log.is_empty() && m.analog_writes.is_empty(), "a reset keeps the record");
}
