use super::{hex, rbg_machine, sha256};
use crate::{Machine, SRAM_BASE};

const SPI_DIGEST: &str = "f42f855764c2c592b83c8f7e062ddf52850259c7116b35c7ec5f5a38f22d7ae4";

fn spi_scenario(m: &mut Machine) -> Vec<u32> {
    let mut trace = vec![];
    fn w(m: &mut Machine, o: usize, size: usize, v: u32) {
        m.reg_write(o, size, v).unwrap();
    }
    fn setbits(m: &mut Machine, o: usize, mask: u8, v: u8) {
        let old = m.reg_read(o, 1).unwrap() as u8;
        w(m, o, 1, ((old & !mask) | (v & mask)) as u32);
    }
    let rd = |m: &mut Machine, o: usize, size: usize, trace: &mut Vec<u32>| {
        let v = m.reg_read(o, size).unwrap();
        trace.push(v);
        v
    };
    w(m, 0x60, 1, 0x00);
    w(m, 0x63, 1, 0x01);
    w(m, 0xFFD8, 4, 1 << 24 | 0x1D << 16);
    setbits(m, 0x59E, 0x20, 0x20);
    setbits(m, 0x59A, 0x20, 0x00);
    setbits(m, 0x59B, 0x20, 0x00);
    setbits(m, 0x5A8, 0xF0, 0x00);
    setbits(m, 0x5A9, 0x03, 0x00);
    setbits(m, 0x5B6, 0x20, 0x20);
    setbits(m, 0x5B7, 0x11, 0x01);
    setbits(m, 0x581, 0x08, 0x08);
    setbits(m, 0x586, 0x1C, 0x00);
    w(m, 0x0A, 1, 0x85);
    w(m, 0x09, 1, 0x02);
    setbits(m, 0x59B, 0x20, 0x20);
    w(m, 0x08, 1, 0x20);
    rd(m, 0x09, 1, &mut trace);
    m.cycles += 95;
    rd(m, 0x09, 1, &mut trace);
    m.cycles += 1;
    rd(m, 0x09, 1, &mut trace);
    rd(m, 0x08, 1, &mut trace);
    w(m, 0x08, 1, 0x05);
    let busy_error = if m.reg_write(0x08, 1, 0x06).is_err() { 1 } else { 0 };
    trace.push(busy_error);
    m.cycles += 96;
    rd(m, 0x08, 1, &mut trace);
    setbits(m, 0x59B, 0x20, 0x00);
    setbits(m, 0x59B, 0x20, 0x20);
    w(m, 0x08, 1, 0xA5);
    m.cycles += 96;
    rd(m, 0x08, 1, &mut trace);
    w(m, 0x09, 1, 0x02 | 0x08 | 0x04);
    rd(m, 0x08, 1, &mut trace);
    m.cycles += 96;
    rd(m, 0x08, 1, &mut trace);
    m.cycles += 96;
    w(m, 0x09, 1, 0x02);
    setbits(m, 0x586, 0x10, 0x10);
    w(m, 0x08, 1, 0x77);
    m.cycles += 96;
    rd(m, 0x08, 1, &mut trace);
    rd(m, 0xFFD8, 4, &mut trace);
    for _ in 0..4 {
        rd(m, 0xFFDC, 1, &mut trace);
    }
    w(m, 0xFFE4, 4, 1 << 24 | 4 << 16 | 0x1234);
    m.analog[0xEB] = 0x4F;
    m.analog[0xFC] = 0x00;
    const BUF: u32 = 0x848000;
    m.regs[0xB08] = (BUF & 0xFF) as u8;
    m.regs[0xB09] = ((BUF >> 8) & 0xFF) as u8;
    m.regs[0xB0A] = 1;
    m.regs[0xB10] |= 0x04;
    let slot = |m: &mut Machine| {
        m.update_time().unwrap();
        let o = (BUF - SRAM_BASE) as usize;
        u32::from_le_bytes(m.sram[o..o + 4].try_into().unwrap())
    };
    trace.push(slot(m));
    m.analog[0xEB] = 0x5F;
    trace.push(slot(m));
    w(m, 0xFFE4, 4, 0);
    m.analog[0xEB] = 0x4F;
    trace.push(slot(m));
    let bad = if m.reg_write(0xFFE4, 4, 1 << 24 | 11 << 16).is_err() { 1 } else { 0 };
    trace.push(bad);
    trace
}

#[test]
fn spi_model() {
    let mut m = rbg_machine();
    let tr = spi_scenario(&mut m);
    let text = tr.iter().map(|v| format!("{:x}", v)).collect::<Vec<_>>().join(" ");
    assert_eq!(hex(&sha256(text.as_bytes())), SPI_DIGEST, "trace {}", text);
    assert!(
        tr[10] == (2 << 16 | 3) && tr[11] == 0x20 && tr[12] == 0x05 && tr[13] == 0xA5 && tr[15] == 0x1234,
        "trace {}",
        text
    );
}
