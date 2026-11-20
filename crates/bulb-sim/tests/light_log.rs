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

fn program(addr: usize, data: &[u8]) -> FlashOp {
    FlashOp::Program { addr, data: data.to_vec() }
}

#[test]
fn erasing_starts_an_empty_newer_sector_before_it_spoils_and_erases_the_old_one() {
    let f = medium();
    open(&f).store(&rec(10));
    f.clear_ops();
    let mut log = open(&f);
    log.erase();
    assert_eq!(f.ops(), [program(B, &slot(&header(1, [0, 0, 0x5A]))), program(A, &[0]), FlashOp::Erase { addr: A }]);
    assert_eq!(log.current(), None);
    assert_eq!(open(&f).current(), None);
    f.clear_ops();
    log.store(&rec(30));
    assert_eq!(f.ops(), [program(B + SLOT, &slot(&rec(30)))]);
    assert_eq!(open(&f).current(), Some(rec(30)));
    f.clear_ops();
    let mut log = open(&f);
    log.erase();
    assert_eq!(f.ops(), [program(A, &slot(&header(2, [0, 0, 0x5A]))), program(B, &[0]), FlashOp::Erase { addr: B }]);
    assert_eq!(open(&f).current(), None);
}

#[test]
fn erasing_a_blank_log_only_writes_a_header() {
    let f = medium();
    let mut log = open(&f);
    log.erase();
    assert_eq!(f.ops(), [program(B, &slot(&header(0, [0, 0, 0x5A])))]);
    assert_eq!(open(&f).current(), None);
    f.clear_ops();
    log.store(&rec(40));
    assert_eq!(f.ops(), [program(B + SLOT, &slot(&rec(40)))]);
}

#[test]
fn erasing_a_legacy_log_starts_the_second_sector_and_erases_the_first() {
    let f = medium();
    f.poke(A + 3 * BLOCK_LEN, &rec(50));
    assert_eq!(open(&f).current(), Some(rec(50)));
    let mut log = open(&f);
    log.erase();
    assert_eq!(f.ops(), [program(B, &slot(&header(0, [0, 0, 0x5A]))), program(A, &[0]), FlashOp::Erase { addr: A }]);
    assert_eq!(open(&f).current(), None);
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


fn churn(log: &mut LightLog<NorFlash>, n: usize) {
    for _ in 0..n {
        let r = if log.current() == Some(rec(1)) { rec(2) } else { rec(1) };
        log.store(&r);
    }
}

fn filled(f: &NorFlash, records: usize) -> LightLog<NorFlash> {
    let mut log = open(f);
    churn(&mut log, records);
    f.clear_ops();
    log
}

const THREE_QUARTERS: usize = SIZE * 3 / 4 / SLOT;

#[test]
fn with_the_idle_hook_run_after_each_change_no_store_erases() {
    let (kept, left) = (medium(), medium());
    let (mut log, mut plain) = (open(&kept), open(&left));
    let (mut in_store, mut in_prepare) = (0, 0);
    for k in 0..511usize {
        let r = rec((k % 250) as u8 + 1);
        kept.clear_ops();
        log.store(&r);
        in_store += kept.erases();
        kept.clear_ops();
        log.prepare();
        in_prepare += kept.erases();
        plain.store(&r);
        assert_eq!(open(&kept).current(), Some(r));
    }
    assert_eq!((in_store, in_prepare), (0, 1));
    assert_eq!(left.erases(), 1, "without the idle hook the second move erases in the store");
    assert_eq!(open(&left).current(), open(&kept).current());
}

#[test]
fn the_spare_sector_is_erased_once_the_active_one_is_three_quarters_full() {
    let f = medium();
    f.poke(B + 5, &[0]);
    let mut log = filled(&f, THREE_QUARTERS - 2);
    assert!(!log.prepare());
    assert_eq!((f.erases(), f.reads()), (0, 0));
    log.store(&rec(250));
    f.clear_ops();
    assert!(log.prepare());
    assert_eq!(f.ops(), [FlashOp::Erase { addr: B }]);
    f.clear_ops();
    assert!(!log.prepare());
    assert_eq!((f.erases(), f.reads()), (0, 0), "a second call looks at nothing");
    assert_eq!(open(&f).current(), Some(rec(250)));
}

#[test]
fn a_blank_spare_is_only_checked_and_a_move_makes_the_next_spare_unknown() {
    let f = medium();
    let mut log = filled(&f, THREE_QUARTERS - 1);
    assert!(!log.prepare());
    assert_eq!(f.erases(), 0);
    assert!(f.reads() > 0);
    churn(&mut log, SIZE / SLOT - THREE_QUARTERS + 1);
    assert_eq!(f.erases(), 0);
    assert_eq!(open(&f).current(), log.current());
    churn(&mut log, THREE_QUARTERS - 2);
    f.clear_ops();
    assert!(log.prepare());
    assert_eq!(f.ops(), [FlashOp::Erase { addr: A }]);
}

#[test]
fn a_log_started_on_blank_flash_does_not_trust_the_spare_it_checked_before() {
    let f = medium();
    f.poke(B + 5, &[0]);
    let mut log = open(&f);
    assert!(!log.prepare());
    assert_eq!(f.erases(), 0);
    churn(&mut log, THREE_QUARTERS - 1);
    f.clear_ops();
    assert!(log.prepare());
    assert_eq!(f.ops(), [FlashOp::Erase { addr: B }]);
}
#[test]
fn before_the_first_record_the_first_sector_is_the_spare() {
    let f = medium();
    f.poke(A + 5, &[0]);
    f.poke(B + 5, &[0]);
    let mut log = open(&f);
    assert!(log.prepare());
    assert_eq!(f.ops(), [FlashOp::Erase { addr: A }]);
    f.clear_ops();
    log.store(&rec(10));
    assert_eq!(f.erases(), 0);
    assert_eq!(open(&f).current(), Some(rec(10)));
}

#[test]
fn a_legacy_log_has_the_second_sector_as_its_spare() {
    let f = medium();
    f.poke(A + 3 * BLOCK_LEN, &rec(50));
    f.poke(B + 5, &[0]);
    let mut log = open(&f);
    assert!(log.prepare());
    assert_eq!(f.ops(), [FlashOp::Erase { addr: B }]);
    assert_eq!(open(&f).current(), Some(rec(50)));
    f.clear_ops();
    log.store(&rec(60));
    assert_eq!(f.erases(), 0);
    assert_eq!(open(&f).current(), Some(rec(60)));
}
