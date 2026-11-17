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
            data(0x00),
            cs(true),
        ]
    );
    let busy_reads = sim::with(|c| c.log.iter().filter(|o| matches!(o, Op::R8(0x0d, v) if v & 0x10 != 0)).count());
    assert_eq!(busy_reads, w.iter().filter(|o| matches!(o, Op::W8(0x0c, _))).count());
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

#[test]
fn a_long_erase_is_polled_until_the_flash_is_ready() {
    let _s = sim::session();
    sim::with(|c| c.flash_busy_polls = 5);
    erase_sector(0xf_5000);
    let w = sim::with(|c| c.writes());
    let status = w.iter().position(|o| *o == Op::W8(0x0c, 0x05)).unwrap_or(0);
    assert_eq!(w[status + 1..].iter().filter(|o| **o == Op::W8(0x0c, 0x00)).count(), 6);
}

#[test]
fn a_write_is_cut_into_programs_of_at_most_32_octets_within_a_page() {
    let _s = sim::session();
    let data: Vec<u8> = (0..100).collect();
    write(0xf_00e0, &data);
    assert_eq!(sim::with(|c| c.flash_ops.clone()), vec![(0x02, 0xf_00e0), (0x02, 0xf_0100), (0x02, 0xf_0120), (0x02, 0xf_0140)]);
    let mut back = [0u8; 100];
    read(0xf_00e0, &mut back);
    assert_eq!(back.to_vec(), data);
    let sizes: Vec<usize> = {
        let w = sim::with(|c| c.writes());
        let mut sizes = vec![];
        let mut n = None;
        for o in w {
            match o {
                Op::W8(0x0c, 0x02) if n.is_none() => n = Some(0usize),
                Op::W8(0x0c, _) => n = n.map(|k| k + 1),
                Op::W8(0x0d, 0x01) => {
                    if let Some(k) = n.take() {
                        sizes.push(k - 3);
                    }
                }
                _ => {}
            }
        }
        sizes
    };
    assert_eq!(sizes, vec![32, 32, 32, 4]);
    write(0xf_1000, &[]);
    assert_eq!(sim::with(|c| c.flash_ops.len()), 4);
}

#[test]
fn a_sector_is_blank_only_when_every_octet_of_it_is_erased() {
    let _s = sim::session();
    assert!(blank(0xf_7000));
    for at in [0usize, 31, 32, 0xfff] {
        sim::with(|c| c.flash[0xf_7000 + at] = 0xfe);
        assert!(!blank(0xf_7000), "{at}");
        sim::with(|c| c.flash[0xf_7000 + at] = 0xff);
    }
    sim::with(|c| c.flash[0xf_8000] = 0);
    assert!(blank(0xf_7000));
}

#[test]
fn the_longest_erase_is_kept_in_microseconds() {
    let _s = sim::session();
    ERASE_MAX_US.set(0);
    sim::with(|c| c.tick_step = 160);
    erase_sector(0xf_7000);
    let first = ERASE_MAX_US.get();
    assert_eq!(first, 10);
    sim::with(|c| c.tick_step = 16);
    erase_sector(0xf_7000);
    assert_eq!(ERASE_MAX_US.get(), first);
    sim::with(|c| c.tick_step = 800);
    erase_sector(0xf_7000);
    assert_eq!(ERASE_MAX_US.get(), 50);
}
