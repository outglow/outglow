use crate::{Flash, Machine, FLASH_MEM, SRAM_BASE};

fn cost(m: &mut Machine, addr: u32, size: usize) -> [i64; 2] {
    let (c, n) = (m.cycles, m.icache_misses);
    m.read(addr, size).unwrap();
    [m.cycles - c, m.icache_misses - n]
}

#[test]
fn icache() {
    let mut fl = Flash::new(FLASH_MEM);
    fl.mem[8] = 0x4B;
    fl.mem[0x0C..0x10].copy_from_slice(&[0x10, 0, 0, 0]);
    let mut m = Machine::new(fl, -1, 1).unwrap();
    assert_eq!(m.sram_code_end(), 0x100, "RAM mirror");
    let want = |m: &mut Machine, a: u32, s: usize, cycles: i64, misses: i64, text: &str| {
        assert_eq!(cost(m, a, s), [cycles, misses], "{}", text);
    };
    m.icache_miss = 0;
    want(&mut m, 0x4000, 2, 0, 0, "model off");
    m.icache_miss = 288;
    want(&mut m, 0x4000, 2, 288, 1, "first access to a line");
    want(&mut m, 0x4002, 2, 0, 0, "the same line again");
    want(&mut m, 0x401E, 2, 0, 0, "the same line's end");
    want(&mut m, 0x4020, 2, 288, 1, "the next line");
    want(&mut m, 0x4000 + 64 * 32, 2, 288, 1, "a line 64 lines further");
    want(&mut m, 0x4000, 2, 288, 1, "the first line again");
    want(&mut m, 0x403E, 4, 288, 1, "a read across two lines, the first held");
    want(&mut m, 0x80, 2, 0, 0, "the RAM mirror");
    m.write(SRAM_BASE + 0x1000, 4, 1).unwrap();
    want(&mut m, SRAM_BASE + 0x1000, 4, 0, 0, "an SRAM address");
    m.reset(-1, false).unwrap();
    m.icache_miss = 288;
    want(&mut m, 0x4020, 2, 288, 1, "after a reset");
}
