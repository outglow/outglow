//! The staging area's rules (`bulb_core::staging`): one update path at a
//! time, and the bootable mark (0x4B at offset 8) written only over an image
//! whose flash contents check out. The images are finalised by the build's
//! own `outglow_image::telink::finalize` (an independent CRC-32), staged as
//! both update paths stage them (the mark left erased), on the flash model.

use std::cell::Cell;

use bulb_core::staging::{image_good, mark_bootable, Owner, Ownership, BOOT_MARK, MARK};
use bulb_core::store::Flash;
use bulb_sim::{FlashOp, NorFlash};
use outglow_image::telink;

const BASE: usize = 0x6000;
const AREA: u32 = 0x6_4000;

fn image(len: usize, seed: u8) -> Vec<u8> {
    let mut v: Vec<u8> = (0..len).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect();
    v[6] = 0;
    v[BOOT_MARK..BOOT_MARK + 4].copy_from_slice(b"KNLT");
    telink::finalize(&v)
}

/// The staging area holding `img` as an update leaves it: the mark erased.
fn staged(img: &[u8]) -> NorFlash {
    assert!(telink::verify(img));
    let f = NorFlash::new(BASE, AREA as usize, 0x1000);
    let mut data = img.to_vec();
    data[BOOT_MARK] = 0xFF;
    f.poke(BASE, &data);
    f
}

fn no_feed() {}

/// Marks and returns whether it did, the bytes written and what the flash holds then.
fn mark(f: &NorFlash) -> (bool, Vec<FlashOp>) {
    f.clear_ops();
    let mut h = f.clone();
    let ok = mark_bootable(&mut h, BASE, AREA, no_feed);
    (ok, f.ops())
}

#[test]
fn a_good_image_is_marked_and_nothing_else_is_written() {
    for len in [0x40, 0x1234, 84 * 1024, 228 * 1024, AREA as usize - 4 - 15] {
        let img = image(len, 3);
        let f = staged(&img);
        let (ok, ops) = mark(&f);
        assert!(ok, "{len:#x}");
        assert_eq!(ops, vec![FlashOp::Program { addr: BASE + BOOT_MARK, data: vec![MARK] }]);
        // what the bootloader then finds is the image byte for byte
        let mut back = vec![0u8; img.len()];
        f.read(BASE, &mut back);
        assert_eq!(back, img);
    }
}

#[test]
fn any_damage_leaves_the_image_unmarked() {
    let img = image(0x3000, 9);
    let mut cases: Vec<(String, NorFlash)> = Vec::new();
    // a bit flipped at many places, the CRC itself included
    for at in (0..img.len()).step_by(97).chain([0x18, 0x19, img.len() - 4, img.len() - 1]) {
        if at == BOOT_MARK {
            continue;
        }
        let mut d = img.clone();
        d[at] ^= 0x10;
        let f = staged(&img);
        f.poke(BASE + at, &[d[at]]);
        cases.push((format!("bit flipped at {at:#x}"), f));
    }
    // the last sector never written, the last byte still erased
    let f = staged(&img);
    f.poke(BASE + 0x2000, &vec![0xFF; img.len() - 0x2000]);
    cases.push(("the last sector missing".into(), f));
    let f = staged(&img);
    f.poke(BASE + img.len() - 1, &[0xFF]);
    cases.push(("the last byte missing".into(), f));
    // the header: no "NLT", the mark already set, sizes out of range
    let f = staged(&img);
    f.poke(BASE + 9, b"NLX");
    cases.push(("no KNLT".into(), f));
    let f = staged(&img);
    f.poke(BASE + BOOT_MARK, &[MARK]);
    cases.push(("marked already".into(), f));
    for size in [0u32, 0x3F, AREA + 1, 0xFFFF_FFFF] {
        let f = staged(&img);
        f.poke(BASE + 0x18, &size.to_le_bytes());
        cases.push((format!("size {size:#x}"), f));
    }
    // an empty area
    cases.push(("nothing staged".into(), NorFlash::new(BASE, AREA as usize, 0x1000)));
    for (what, f) in &cases {
        let (ok, ops) = mark(f);
        assert!(!ok && ops.is_empty(), "{what}: marked");
    }
}

#[test]
fn the_check_takes_the_size_from_the_header_not_from_the_area() {
    // a good image followed by leftovers of a longer earlier one
    let old = image(0x9000, 1);
    let new = image(0x4000, 2);
    let f = staged(&old);
    let mut d = new.clone();
    d[BOOT_MARK] = 0xFF;
    f.poke(BASE, &d);
    assert!(mark(&f).0);
}

thread_local! {
    static FEEDS: Cell<u32> = const { Cell::new(0) };
}

fn feed() {
    FEEDS.with(|n| n.set(n.get() + 1));
}

#[test]
fn the_watchdog_is_fed_as_a_large_image_is_checked() {
    // the whole area: 400 KB of CRC on the chip takes longer than 2 s unfed
    let img = image(AREA as usize - 4 - 15, 5);
    let f = staged(&img);
    FEEDS.with(|n| n.set(0));
    assert!(image_good(&f, BASE, AREA, feed));
    let feeds = FEEDS.with(|n| n.get());
    assert!(feeds as usize >= img.len() / 0x4000 - 1, "{feeds} feeds");
}

#[test]
fn one_update_path_at_a_time() {
    let mut o = Ownership::FREE;
    assert!(!o.holds(Owner::Zigbee) && !o.holds(Owner::Ble));
    assert!(o.claim(Owner::Zigbee));
    // a BLE update cannot start, nor end the Zigbee one
    assert!(!o.claim(Owner::Ble));
    o.release(Owner::Ble);
    assert!(o.holds(Owner::Zigbee) && !o.holds(Owner::Ble));
    // the same path again (an OTA download resumed)
    assert!(o.claim(Owner::Zigbee));
    o.release(Owner::Zigbee);
    assert_eq!(o, Ownership::FREE);
    assert!(o.claim(Owner::Ble));
    assert!(!o.claim(Owner::Zigbee));
    o.release(Owner::Zigbee);
    assert!(o.holds(Owner::Ble));
    o.release(Owner::Ble);
    assert!(o.claim(Owner::Zigbee));
}
