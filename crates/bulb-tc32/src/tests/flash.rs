use super::*;
use crate::sim::{self, Op};

#[test]
fn a_write_across_a_page_is_split_at_the_page_end() {
    let _s = sim::session();
    let data: Vec<u8> = (0..40).collect();
    write(0xf_00f0, &data);
    assert_eq!(sim::with(|c| c.flash_ops.clone()), vec![(0x02, 0xf_00f0), (0x02, 0xf_0100)]);
    let mut back = [0u8; 40];
    read(0xf_00f0, &mut back);
    assert_eq!(back.to_vec(), data);
}

#[test]
fn a_program_is_write_enable_page_program_address_data_then_status_polling() {
    let _s = sim::session();
    write(0x01_2345, &[0xa5, 0x5a]);
    let w: Vec<Op> = sim::with(|c| c.writes()).into_iter().filter(|o| !matches!(o, Op::W8(0x643, _))).collect();
    let cs = |high: bool| Op::W8(0x0d, if high { 0x01 } else { 0x00 });
    let data = |b: u8| Op::W8(0x0c, b);
    assert_eq!(
        w,
        vec![
            cs(true),
            cs(false),
            data(0x06),
            cs(true),
            cs(false),
            data(0x02),
            data(0x01),
            data(0x23),
            data(0x45),
            data(0xa5),
            data(0x5a),
            cs(true),
            cs(true),
            cs(false),
            data(0x05),
            data(0x00),
            cs(true),
        ]
    );
    assert_eq!(sim::with(|c| (c.flash[0x01_2345], c.flash[0x01_2346])), (0xa5, 0x5a));
}

#[test]
fn programming_only_clears_bits_and_an_erase_sets_the_sector_back() {
    let _s = sim::session();
    write(0xf_5000, &[0x0f]);
    write(0xf_5000, &[0xf0]);
    let mut b = [0u8; 1];
    read(0xf_5000, &mut b);
    assert_eq!(b, [0x00]);
    assert!(!blank(0xf_5000));
    let before = ERASES.get();
    erase_sector(0xf_5123);
    assert_eq!(sim::with(|c| c.flash_ops.last().copied()), Some((0x20, 0xf_5000)));
    assert!(blank(0xf_5000));
    assert_eq!(ERASES.get(), before.wrapping_add(1));
}

#[test]
fn erasing_the_staging_area_is_not_counted_as_wear() {
    let _s = sim::session();
    let before = ERASES.get();
    erase_sector(crate::staging::BASE);
    erase_sector(crate::staging::BASE + crate::staging::SIZE as usize - 0x1000);
    assert_eq!(ERASES.get(), before);
    erase_sector(crate::staging::BASE + crate::staging::SIZE as usize);
    assert_eq!(ERASES.get(), before.wrapping_add(1));
}

#[test]
fn the_irqs_are_off_while_the_flash_is_busy() {
    let _s = sim::session();
    sim::with(|c| c.poke8(crate::reg::IRQ_ENABLE, 1));
    erase_sector(0xf_6000);
    let w = sim::with(|c| c.writes());
    assert_eq!(w.first(), Some(&Op::W8(0x643, 0)));
    assert_eq!(w.last(), Some(&Op::W8(0x643, 1)));
    let n = STORES.get();
    stored();
    assert_eq!(STORES.get(), n.wrapping_add(1));
}
