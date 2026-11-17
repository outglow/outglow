use memcap as _;

use bulb_core::config::BLOCK_LEN;
use bulb_core::store::{LightLog, SLOT};
use bulb_core::{Config, StartupLevel};
use bulb_sim::{FlashOp, NorFlash};

const A: usize = 0xF_0000;
const B: usize = 0xF_4000;
const SIZE: usize = 0x1000;

fn medium() -> NorFlash {
    NorFlash::new(A, B + SIZE - A, SIZE)
}

fn open(f: &NorFlash) -> LightLog<NorFlash> {
    LightLog::new(f.clone(), [A, B], SIZE)
}

fn rec(level: u8) -> [u8; BLOCK_LEN] {
    Config { startup_level: StartupLevel::Fixed(level), last_level: level, ..Config::default() }.encode()
}

fn slot(data: &[u8; BLOCK_LEN]) -> Vec<u8> {
    data.iter().copied().chain(data.iter().map(|b| !b)).collect()
}

fn header(gen: u8, end: [u8; 3]) -> [u8; BLOCK_LEN] {
    [0xA5, b'L', b'T', 0x01, gen, end[0], end[1], end[2]]
}

#[test]
fn after_reopening_a_record_goes_to_the_next_slot() {
    let f = medium();
    open(&f).store(&rec(10));
    f.clear_ops();
    let mut log = open(&f);
    assert_eq!(log.current(), Some(rec(10)));
    log.store(&rec(20));
    match f.ops().as_slice() {
        [FlashOp::Program { addr, data }] => assert_eq!((*addr, data.as_slice()), (A + 2 * SLOT, slot(&rec(20)).as_slice())),
        ops => panic!("one program: {ops:?}"),
    }
    let mut log = open(&f);
    f.clear_ops();
    log.store(&rec(30));
    assert!(matches!(f.ops().as_slice(), [FlashOp::Program { addr, .. }] if *addr == A + 3 * SLOT), "{:?}", f.ops());
}

#[test]
fn of_two_sectors_with_the_same_generation_the_first_counts() {
    let f = medium();
    open(&f).store(&rec(10));
    f.poke(B, &slot(&header(0, [0, 0, 0x5A])));
    f.poke(B + SLOT, &slot(&rec(20)));
    assert_eq!(open(&f).current(), Some(rec(10)));
    f.poke(B, &slot(&header(1, [0, 0, 0x5A])));
    assert_eq!(open(&f).current(), Some(rec(20)), "a newer generation counts");
}

#[test]
fn a_header_needs_its_start_and_its_end() {
    for bad in [header(1, [0, 0, 0x00]), [0xA5, b'L', b'X', 0x01, 1, 0, 0, 0x5A]] {
        let f = medium();
        open(&f).store(&rec(10));
        f.poke(B, &slot(&bad));
        f.poke(B + SLOT, &slot(&rec(20)));
        assert_eq!(open(&f).current(), Some(rec(10)), "{bad:02X?}");
    }
}

#[test]
fn erasing_clears_the_record_and_skips_a_blank_sector() {
    let f = medium();
    open(&f).store(&rec(10));
    f.clear_ops();
    let mut log = open(&f);
    log.erase();
    assert_eq!(f.erases(), 1);
    assert!(matches!(f.ops().as_slice(), [FlashOp::Erase { addr }] if *addr == A));
    assert_eq!(log.current(), None);
    assert_eq!(open(&f).current(), None);
    log.store(&rec(30));
    assert_eq!(open(&f).current(), Some(rec(30)));
}

#[test]
fn a_newer_sector_without_a_whole_record_falls_back_to_the_older() {
    let f = medium();
    open(&f).store(&rec(10));
    f.poke(B, &slot(&header(1, [0, 0, 0x5A])));
    let mut torn = slot(&rec(20));
    torn[15] ^= 0x01;
    f.poke(B + SLOT, &torn);
    assert_eq!(open(&f).current(), Some(rec(10)));
}

