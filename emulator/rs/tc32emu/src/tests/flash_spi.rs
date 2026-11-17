use crate::{Flash, FLASH_MEM};

fn flash() -> Flash {
    let mut f = Flash::new(FLASH_MEM);
    f.tpp_us = 0;
    f.tse_us = 0;
    f.tbe32_us = 0;
    f.tbe64_us = 0;
    f
}

fn cmd(f: &mut Flash, bytes: &[u8]) -> Vec<u8> {
    f.select(true).unwrap();
    let out = bytes.iter().map(|&b| f.clock(b).unwrap()).collect();
    f.select(false).unwrap();
    out
}

fn status(f: &mut Flash) -> u8 {
    cmd(f, &[0x05, 0])[1]
}

fn read(f: &mut Flash, addr: usize, n: usize) -> Vec<u8> {
    let mut c = vec![0x03, (addr >> 16) as u8, (addr >> 8) as u8, addr as u8];
    c.resize(4 + n, 0);
    cmd(f, &c)[4..].to_vec()
}

fn program(f: &mut Flash, addr: usize, data: &[u8]) {
    let mut c = vec![0x02, (addr >> 16) as u8, (addr >> 8) as u8, addr as u8];
    c.extend_from_slice(data);
    cmd(f, &c);
}

fn sector_erase(f: &mut Flash, addr: usize) {
    cmd(f, &[0x20, (addr >> 16) as u8, (addr >> 8) as u8, addr as u8]);
}

#[test]
fn jedec_id_reads_manufacturer_type_and_capacity() {
    let mut f = flash();
    assert_eq!(&cmd(&mut f, &[0x9F, 0, 0, 0])[1..], &[0x85, 0x60, 0x14]);
}

#[test]
fn erased_flash_reads_ff() {
    let mut f = flash();
    assert_eq!(read(&mut f, 0x1000, 4), vec![0xFF; 4]);
}

#[test]
fn write_enable_sets_wel_and_write_disable_clears_it() {
    let mut f = flash();
    cmd(&mut f, &[0x06]);
    assert_ne!(status(&mut f) & 0x02, 0);
    cmd(&mut f, &[0x04]);
    assert_eq!(status(&mut f) & 0x02, 0);
}

#[test]
fn page_program_without_write_enable_is_ignored() {
    let mut f = flash();
    program(&mut f, 0x1000, &[0x12]);
    assert_eq!(read(&mut f, 0x1000, 1), vec![0xFF]);
}

#[test]
fn page_program_writes_and_then_clears_wel() {
    let mut f = flash();
    cmd(&mut f, &[0x06]);
    program(&mut f, 0x1000, &[0x12, 0x34, 0x56, 0x78]);
    assert_eq!(read(&mut f, 0x1000, 4), vec![0x12, 0x34, 0x56, 0x78]);
    assert_eq!(status(&mut f) & 0x02, 0);
}

#[test]
fn page_program_can_only_clear_bits() {
    let mut f = flash();
    cmd(&mut f, &[0x06]);
    program(&mut f, 0x1000, &[0x12]);
    cmd(&mut f, &[0x06]);
    program(&mut f, 0x1000, &[0x0F]);
    assert_eq!(read(&mut f, 0x1000, 1), vec![0x02]);
}

#[test]
fn page_program_wraps_inside_its_256_byte_page() {
    let mut f = flash();
    cmd(&mut f, &[0x06]);
    program(&mut f, 0x10FF, &[0xAA, 0xBB]);
    assert_eq!(read(&mut f, 0x10FF, 1), vec![0xAA]);
    assert_eq!(read(&mut f, 0x1000, 1), vec![0xBB]);
    assert_eq!(read(&mut f, 0x1100, 1), vec![0xFF]);
}

#[test]
fn sector_erase_clears_only_the_4k_sector_holding_the_address() {
    let mut f = flash();
    for a in [0x0FFF, 0x1000, 0x1FFF, 0x2000] {
        cmd(&mut f, &[0x06]);
        program(&mut f, a, &[0x55]);
    }
    cmd(&mut f, &[0x06]);
    sector_erase(&mut f, 0x1234);
    assert_eq!(read(&mut f, 0x1000, 1), vec![0xFF]);
    assert_eq!(read(&mut f, 0x1FFF, 1), vec![0xFF]);
    assert_eq!(read(&mut f, 0x0FFF, 1), vec![0x55]);
    assert_eq!(read(&mut f, 0x2000, 1), vec![0x55]);
}

#[test]
fn sector_erase_without_write_enable_is_ignored() {
    let mut f = flash();
    cmd(&mut f, &[0x06]);
    program(&mut f, 0x3000, &[0x00]);
    sector_erase(&mut f, 0x3000);
    assert_eq!(read(&mut f, 0x3000, 1), vec![0x00]);
}

#[test]
fn read_continues_across_page_and_sector_boundaries() {
    let mut f = flash();
    for (a, v) in [(0x10FF, 0x11), (0x1100, 0x22), (0x1FFF, 0x33), (0x2000, 0x44)] {
        cmd(&mut f, &[0x06]);
        program(&mut f, a, &[v]);
    }
    assert_eq!(read(&mut f, 0x10FF, 2), vec![0x11, 0x22]);
    assert_eq!(read(&mut f, 0x1FFF, 2), vec![0x33, 0x44]);
}

#[test]
fn deep_power_down_answers_nothing_until_release() {
    let mut f = flash();
    cmd(&mut f, &[0x06]);
    program(&mut f, 0x2000, &[0x55]);
    cmd(&mut f, &[0xB9]);
    assert_eq!(cmd(&mut f, &[0x05, 0])[1], 0xFF);
    assert_eq!(read(&mut f, 0x2000, 1), vec![0xFF]);
    cmd(&mut f, &[0x06]);
    program(&mut f, 0x2000, &[0x00]);
    cmd(&mut f, &[0xAB]);
    assert_eq!(read(&mut f, 0x2000, 1), vec![0x55]);
    assert_eq!(status(&mut f) & 0x02, 0);
}
