use memcap as _;

use bulb_core::nv::{NvLog, MAX_ITEM};
use bulb_sim::{FlashOp, NorFlash};

const SECTORS: [usize; 2] = [0xF_1000, 0xF_2000];
const SIZE: usize = 0x1000;
type Nv = NvLog<NorFlash, 32>;

fn medium() -> NorFlash {
    NorFlash::new(SECTORS[0], 2 * SIZE, SIZE)
}

fn open(f: &NorFlash) -> Nv {
    NvLog::new(f.clone(), SECTORS, SIZE, || {})
}

fn bytes(f: &NorFlash, at: usize, n: usize) -> Vec<u8> {
    (at..at + n).map(|a| f.byte(a)).collect()
}

fn load(nv: &Nv, id: u8) -> Option<Vec<u8>> {
    let mut buf = [0u8; MAX_ITEM];
    nv.load(id, &mut buf).map(|n| buf[..n].to_vec())
}

fn programs_at(f: &NorFlash) -> Vec<(usize, usize)> {
    f.ops()
        .iter()
        .filter_map(|o| match o {
            FlashOp::Program { addr, data } => Some((*addr, data.len())),
            FlashOp::Erase { .. } => None,
        })
        .collect()
}

fn filled_to(f: &NorFlash, free: usize) -> Nv {
    let mut nv = open(f);
    let mut at = 4;
    while at + 163 <= free {
        nv.store(1, &[0x11; MAX_ITEM]);
        at += 163;
    }
    if at < free {
        nv.store(2, &vec![0x22; free - at - 3]);
    }
    f.clear_ops();
    nv
}

#[test]
fn a_record_is_id_length_data_and_a_crc8_from_0xff() {
    let f = medium();
    let mut nv = open(&f);
    nv.store(1, &[0xAA]);
    nv.store(3, &[1, 2, 3, 4]);
    nv.store(0x80, &[0x80]);
    let s = SECTORS[1];
    assert_eq!(bytes(&f, s, 4), [0xA5, 0x00, 0xFF, 0x5A]);
    assert_eq!(bytes(&f, s + 4, 4), [1, 1, 0xAA, 0x0A]);
    assert_eq!(bytes(&f, s + 8, 7), [3, 4, 1, 2, 3, 4, 0xB8]);
    assert_eq!(bytes(&f, s + 15, 4), [0x80, 1, 0x80, 0xBC]);
    assert_eq!(f.erases(), 0, "both sectors were blank");
    let nv = open(&f);
    assert_eq!((load(&nv, 1), load(&nv, 3)), (Some(vec![0xAA]), Some(vec![1, 2, 3, 4])));
}

#[test]
fn a_record_with_a_wrong_crc_ends_the_log() {
    let f = medium();
    let mut nv = open(&f);
    nv.store(1, &[0xAA]);
    nv.store(3, &[1, 2, 3, 4]);
    f.poke(SECTORS[1] + 7, &[0x0B]);
    let nv = open(&f);
    assert_eq!((load(&nv, 1), load(&nv, 3)), (None, None));
}

#[test]
fn a_switch_erases_only_a_sector_that_is_not_blank() {
    let f = medium();
    let mut nv = filled_to(&f, SIZE);
    nv.store(3, &[1]);
    assert_eq!((f.erases(), f.byte(SECTORS[0] + 1)), (0, 1), "into the blank sector 0, generation 1");
    for _ in 0..30 {
        nv.store(1, &[0x33; MAX_ITEM]);
    }
    assert_eq!(f.erases(), 1, "back into sector 1, which held the old log");
    assert_eq!((load(&nv, 3), f.byte(SECTORS[1] + 1)), (Some(vec![1]), 2));
}

#[test]
fn an_item_that_exactly_fills_the_sector_stays_in_it() {
    let f = medium();
    let mut nv = filled_to(&f, SIZE - 3 - MAX_ITEM);
    nv.store(5, &[0x55; MAX_ITEM]);
    assert!(programs_at(&f).iter().all(|&(a, _)| a >= SECTORS[1]), "{:?}", programs_at(&f));
    assert_eq!(f.erases(), 0);
    assert_eq!(load(&open(&f), 5), Some(vec![0x55; MAX_ITEM]));
}

#[test]
fn empty_or_oversized_items_are_not_stored() {
    let f = medium();
    let mut nv = open(&f);
    nv.store(5, &[9]);
    nv.store(5, &[]);
    nv.store(5, &[7; MAX_ITEM + 1]);
    assert_eq!(load(&nv, 5), Some(vec![9]));
    nv.store_pair((4, &[1]), (5, &[]));
    nv.store_pair((5, &[]), (6, &[2]));
    nv.store_pair((7, &[7; MAX_ITEM + 1]), (8, &[3]));
    nv.store_pair((9, &[4]), (10, &[7; MAX_ITEM + 1]));
    let nv = open(&f);
    assert_eq!([4, 5, 6, 7, 8, 9, 10].map(|id| load(&nv, id)), [Some(vec![1]), Some(vec![9]), Some(vec![2]), None, Some(vec![3]), Some(vec![4]), None]);
}

#[test]
fn a_pair_is_written_in_one_run() {
    let pair = |a: &[u8], b: &[u8]| {
        let f = medium();
        let mut nv = filled_to(&f, 8);
        nv.store_pair((3, a), (4, b));
        let runs = programs_at(&f);
        let nv = open(&f);
        assert_eq!((load(&nv, 3).as_deref(), load(&nv, 4).as_deref()), (Some(a), Some(b)));
        runs
    };
    let runs = pair(&[0x33; MAX_ITEM], &[0x44; 8]);
    assert_eq!(runs.iter().map(|r| r.1).sum::<usize>(), 174);
    assert_eq!(runs.len(), 6, "174 bytes in 32-byte programs: {runs:?}");
    assert_eq!(pair(&[0x33; 8], &[0x44; MAX_ITEM]).len(), 6);
    assert_eq!(pair(&[0x33; MAX_ITEM], &[0x44; MAX_ITEM]).iter().map(|r| r.1).sum::<usize>(), 326);
}

#[test]
fn a_pair_that_exactly_fills_the_sector_is_one_program() {
    let f = medium();
    let mut nv = filled_to(&f, SIZE - 22);
    nv.store_pair((3, &[3; 8]), (4, &[4; 8]));
    assert_eq!(programs_at(&f), [(SECTORS[1] + SIZE - 22, 22)]);
    let f = medium();
    let mut nv = filled_to(&f, SIZE - 21);
    nv.store_pair((3, &[3; 8]), (4, &[4; 8]));
    assert!(programs_at(&f).iter().any(|&(a, _)| a < SECTORS[1]), "a switch into sector 0: {:?}", f.ops());
    let nv = open(&f);
    assert_eq!((load(&nv, 3), load(&nv, 4)), (Some(vec![3; 8]), Some(vec![4; 8])));
}

#[test]
fn forgetting_erases_the_old_sector() {
    let f = medium();
    let mut nv = open(&f);
    nv.store(1, &[1]);
    nv.store(2, &[2]);
    f.clear_ops();
    nv.forget_all_but(2);
    assert_eq!(f.erases(), 1);
    assert!(bytes(&f, SECTORS[1], SIZE).iter().all(|&b| b == 0xFF));
    let nv = open(&f);
    assert_eq!((load(&nv, 1), load(&nv, 2)), (None, Some(vec![2])));
}

#[test]
fn a_record_running_past_the_sector_ends_the_log() {
    for len in [100, 4] {
        let f = medium();
        filled_to(&f, SIZE - 6);
        f.poke(SECTORS[1] + SIZE - 6, &[1, len]);
        let nv = open(&f);
        assert_eq!((load(&nv, 1), load(&nv, 2)), (Some(vec![0x11; MAX_ITEM]), Some(vec![0x22; 8])), "length {len}");
    }
}

#[test]
fn reopening_continues_the_log_in_place() {
    let f = medium();
    open(&f).store(1, &[1]);
    let mut nv = open(&f);
    f.clear_ops();
    nv.store(2, &[2]);
    assert_eq!(programs_at(&f), [(SECTORS[1] + 8, 4)]);
    assert_eq!(f.erases(), 0);
}

#[test]
fn bytes_after_the_last_record_move_the_next_store_to_the_other_sector() {
    let f = medium();
    open(&f).store(1, &[1]);
    f.poke(SECTORS[1] + 48, &[0x00]);
    let mut nv = open(&f);
    f.clear_ops();
    nv.store(2, &[2]);
    assert!(programs_at(&f).iter().all(|&(a, _)| a < SECTORS[1]), "{:?}", f.ops());
    let nv = open(&f);
    assert_eq!((load(&nv, 1), load(&nv, 2)), (Some(vec![1]), Some(vec![2])));
}

#[test]
fn a_record_of_no_bytes_is_no_item() {
    let f = medium();
    open(&f).store(5, &[9]);
    f.poke(SECTORS[1] + 8, &[6, 0, 0xA9]);
    let mut nv = open(&f);
    nv.store(7, &[1, 2, 3, 4]);
    let nv = open(&f);
    assert_eq!(load(&nv, 6), None);
    let mut ids = vec![];
    nv.each(|id, d| ids.push((id, d.len())));
    assert_eq!(ids, [(5, 1), (7, 4)]);
    assert_eq!(nv.load(7, &mut [0u8; 3]), None, "a buffer too small");
    assert_eq!(nv.load(7, &mut [0u8; 4]), Some(4));
}

