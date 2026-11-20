use memcap as _;
use std::cell::Cell;

use bulb_core::staging::{image_good, mark_bootable, Owner, Ownership, BOOT_MARK, MARK};
use bulb_core::store::Flash;
use bulb_sim::{FlashOp, NorFlash};
use outglow_image::telink;

const BASE: usize = 0x6000;
const AREA: u32 = 0x6_4000;
const DIMM: u16 = 0x0390;
const OURS: [u8; 4] = [0x41, 0x11, 0x90, 0x03];
const STOCK_HEAD: [u8; 0x18] = [
    0x51, 0x80, 0x00, 0x02, 0x00, 0x01, 0x00, 0x00, 0x4b, 0x4e, 0x4c, 0x54, 0x10, 0x03, 0x88, 0x00, 0xc6, 0x80, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00,
];

fn image_as(len: usize, seed: u8, id: [u8; 4]) -> Vec<u8> {
    let mut v: Vec<u8> = (0..len).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect();
    v[6] = 0;
    v[BOOT_MARK..BOOT_MARK + 4].copy_from_slice(b"KNLT");
    v[0x12..0x16].copy_from_slice(&id);
    telink::finalize(&v)
}

fn image(len: usize, seed: u8) -> Vec<u8> {
    image_as(len, seed, OURS)
}

fn stock(len: usize) -> Vec<u8> {
    let mut v: Vec<u8> = (0..len).map(|i| (i as u8).wrapping_mul(7)).collect();
    v[..STOCK_HEAD.len()].copy_from_slice(&STOCK_HEAD);
    telink::finalize(&v)
}

fn staged(img: &[u8]) -> NorFlash {
    assert!(telink::verify(img));
    let f = NorFlash::new(BASE, AREA as usize, 0x1000);
    let mut data = img.to_vec();
    data[BOOT_MARK] = 0xFF;
    f.poke(BASE, &data);
    f
}

fn no_feed() {}

fn mark(f: &NorFlash) -> (bool, Vec<FlashOp>) {
    f.clear_ops();
    let mut h = f.clone();
    let ok = mark_bootable(&mut h, BASE, AREA, DIMM, no_feed);
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
        let mut back = vec![0u8; img.len()];
        f.read(BASE, &mut back);
        assert_eq!(back, img);
    }
}

#[test]
fn any_damage_leaves_the_image_unmarked() {
    let img = image(0x3000, 9);
    let mut cases: Vec<(String, NorFlash)> = Vec::new();
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
    let f = staged(&img);
    f.poke(BASE + 0x2000, &vec![0xFF; img.len() - 0x2000]);
    cases.push(("the last sector missing".into(), f));
    let f = staged(&img);
    f.poke(BASE + img.len() - 1, &[0xFF]);
    cases.push(("the last byte missing".into(), f));
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
    cases.push(("nothing staged".into(), NorFlash::new(BASE, AREA as usize, 0x1000)));
    for (what, f) in &cases {
        let (ok, ops) = mark(f);
        assert!(!ok && ops.is_empty(), "{what}: marked");
    }
}

#[test]
fn this_boards_image_and_any_stock_image_are_marked() {
    for (what, img) in [("this board's image", image(0x2000, 4)), ("a stock image", stock(0x2000))] {
        let f = staged(&img);
        let (ok, ops) = mark(&f);
        assert!(ok, "{what}");
        assert_eq!(ops, vec![FlashOp::Program { addr: BASE + BOOT_MARK, data: vec![MARK] }], "{what}");
    }
    let tw = image_as(0x2000, 4, [0x41, 0x11, 0x91, 0x03]);
    let f = staged(&tw);
    f.clear_ops();
    let mut h = f.clone();
    assert!(mark_bootable(&mut h, BASE, AREA, 0x0391, no_feed), "a TW image on a TW board");
}

#[test]
fn an_image_for_another_board_or_link_address_is_not_marked() {
    let cases = [
        ("TW image on a dimm board", [0x41, 0x11, 0x91, 0x03]),
        ("LilyGO image linked for 0x0", [0x41, 0x11, 0xa5, 0x03]),
        ("Telink SDK image of the LilyGO module", [0x41, 0x11, 0x00, 0x02]),
        ("C firmware, dimm", [0x41, 0x11, 0x81, 0x03]),
        ("C bootloader linked for 0x0", [0x41, 0x11, 0xff, 0x03]),
        ("another manufacturer", [0x42, 0x11, 0x90, 0x03]),
        ("another manufacturer, high byte", [0x41, 0x12, 0x90, 0x03]),
        ("image type high byte", [0x41, 0x11, 0x90, 0x04]),
        ("stock manufacturer, our type", [0x00, 0x00, 0x90, 0x03]),
        ("our manufacturer, stock type", [0x41, 0x11, 0x00, 0x00]),
        ("stock with a type", [0x00, 0x00, 0x00, 0x01]),
        ("stock with a manufacturer", [0x01, 0x00, 0x00, 0x00]),
        ("erased", [0xff; 4]),
    ];
    for (what, id) in cases {
        let f = staged(&image_as(0x2000, 6, id));
        let (ok, ops) = mark(&f);
        assert!(!ok && ops.is_empty(), "{what}: marked");
        assert!(!image_good(&f, BASE, AREA, DIMM, no_feed), "{what}");
    }
}

#[test]
fn the_check_takes_the_size_from_the_header_not_from_the_area() {
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
    let img = image(AREA as usize - 4 - 15, 5);
    let f = staged(&img);
    FEEDS.with(|n| n.set(0));
    assert!(image_good(&f, BASE, AREA, DIMM, feed));
    let feeds = FEEDS.with(|n| n.get());
    assert_eq!(feeds as usize, (img.len() - 4) / 0x4000, "one feed every 16 KiB checked");
}

#[test]
fn one_update_path_at_a_time() {
    let mut o = Ownership::FREE;
    assert!(!o.holds(Owner::Zigbee) && !o.holds(Owner::Ble));
    assert!(o.claim(Owner::Zigbee));
    assert!(!o.claim(Owner::Ble));
    o.release(Owner::Ble);
    assert!(o.holds(Owner::Zigbee) && !o.holds(Owner::Ble));
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
