use crate::{Flash, Machine, FLASH_MEM};

#[test]
fn symbolize() {
    let mut fl = Flash::new(FLASH_MEM);
    fl.mem[8] = 0x4B;
    let mut m = Machine::new(fl, -1, 1).unwrap();
    assert_eq!(m.symbolize(0x1BAA), "0x1baa", "no symbols");
    m.set_symbols([("trim_analog_write", 0x1BA0), ("other", 0x1000), ("alias", 0x1000)]);
    assert_eq!(m.symbolize(0x1BAA), "trim_analog_write+0xa", "set after construction");
    assert_eq!(m.symbolize(0x1004), "other+0x4", "two names at one address: the later by name");
    assert_eq!(m.symbolize(0x10), "0x10", "below every symbol");
    m.set_symbols(std::iter::empty());
    assert_eq!(m.symbolize(0x1BAA), "0x1baa", "cleared");
}
