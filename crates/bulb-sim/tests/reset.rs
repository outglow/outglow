//! Factory reset by power cycling (bulb-core's `reset` module).

use bulb_core::reset::{PowerCycles, WordStore};

#[derive(Clone)]
struct Words {
    w: Vec<u16>,
    erases: u32,
}

impl Words {
    fn new(n: usize) -> Self {
        Words { w: vec![0xFFFF; n], erases: 0 }
    }
}

impl WordStore for Words {
    fn slots(&self) -> usize {
        self.w.len()
    }
    fn read(&self, slot: usize) -> u16 {
        self.w[slot]
    }
    fn write(&mut self, slot: usize, word: u16) {
        // flash: bits only go from 1 to 0
        self.w[slot] &= word;
    }
    fn erase(&mut self) {
        self.w.iter_mut().for_each(|w| *w = 0xFFFF);
        self.erases += 1;
    }
}

/// Powers the bulb for `seconds`; returns the store as the power cut leaves it
/// and whether this power-on was a factory reset.
fn power(store: Words, seconds: u64) -> (Words, bool) {
    let (mut c, factory) = PowerCycles::power_on(store);
    for ms in (0..=seconds * 1000).step_by(100) {
        c.tick(ms);
    }
    (c.store().clone(), factory)
}

fn sequence(seconds: &[u64]) -> (Words, Vec<bool>) {
    let mut store = Words::new(64);
    let mut resets = vec![];
    for &s in seconds {
        let (next, factory) = power(store, s);
        store = next;
        resets.push(factory);
    }
    (store, resets)
}

#[test]
fn the_stock_sequence_resets_on_the_sixth_power_on() {
    let (_, resets) = sequence(&[1, 1, 1, 12, 12, 1]);
    assert_eq!(resets, vec![false, false, false, false, false, true]);
}

#[test]
fn ordinary_use_never_resets() {
    // long sessions, short blips, a mix
    for pattern in [vec![3600u64; 8], vec![1; 12], vec![1, 1, 1, 1, 1, 1], vec![1, 1, 1, 40, 12, 1], vec![12, 12, 12, 12, 12, 12]] {
        let (_, resets) = sequence(&pattern);
        assert!(resets.iter().all(|r| !r), "{pattern:?} -> {resets:?}");
    }
}

#[test]
fn a_long_power_on_cut_short_starts_the_count_over() {
    // the fourth power-on must last 6 s; here it is cut after 2 s
    let (_, resets) = sequence(&[1, 1, 1, 2, 12, 1]);
    assert!(resets.iter().all(|r| !r));
    // and the full sequence works right after
    let (_, resets) = sequence(&[1, 1, 1, 2, 1, 1, 1, 12, 12, 1]);
    assert_eq!(resets.last(), Some(&true));
    assert_eq!(resets.iter().filter(|r| **r).count(), 1);
}

#[test]
fn a_long_power_on_that_lasts_too_long_cancels() {
    let (_, resets) = sequence(&[1, 1, 1, 31, 12, 1]);
    assert!(resets.iter().all(|r| !r));
}

#[test]
fn the_reset_happens_once() {
    let (store, resets) = sequence(&[1, 1, 1, 12, 12, 1]);
    assert_eq!(resets.last(), Some(&true));
    let (_, again) = power(store, 1);
    assert!(!again);
}

#[test]
fn the_words_are_reused_after_an_erase_and_garbage_is_skipped() {
    let mut store = Words::new(8);
    for _ in 0..40 {
        let (next, factory) = power(store, 60);
        assert!(!factory);
        store = next;
    }
    assert!(store.erases >= 4, "{} erases", store.erases);
    // a word that is not a count
    let mut store = Words::new(8);
    store.w[0] = 0x1234;
    let (store, factory) = power(store, 1);
    assert!(!factory);
    assert_eq!(store.w[0], 0x0000);
    assert_eq!(store.w[1], 0xFFFE);
}

#[test]
fn a_reset_that_is_not_a_power_on_does_not_count() {
    let (store, _) = sequence(&[1, 1]);
    let before = store.w.clone();
    let mut c = PowerCycles::idle(store);
    c.tick(7_000);
    c.tick(31_000);
    assert_eq!(c.store().w, before);
    assert_eq!(c.step(), 0);
}

#[test]
fn a_count_the_stock_firmware_left_carries_on() {
    // the stock firmware walks the same words; past slot 0 its layout is this one's
    let mut store = Words::new(64);
    store.w[..3].fill(0x0000);
    store.w[3] = 0xFFF8; // three short power-ons
    let (store, _) = power(store, 12);
    assert_eq!(store.w[3], 0xFFE0);
    let (store, _) = power(store, 12);
    let (_, factory) = power(store, 1);
    assert!(factory);
}

#[test]
fn a_sector_that_is_not_as_this_counter_leaves_it_starts_over() {
    let starts_over = |store: Words| {
        let (store, factory) = power(store, 1);
        assert!(!factory, "{:04X?}", store.w);
        assert_eq!(store.erases, 1);
        assert_eq!(store.w[..3], [0x0000, 0xFFFE, 0xFFFF]);
    };
    // a count in slot 0, as the stock firmware starts a fresh sector
    let mut store = Words::new(16);
    store.w[0] = 0xFF80;
    starts_over(store);
    // stray bits after the active word, as an erase cut short leaves them
    let mut store = Words::new(16);
    store.w[0] = 0x0000;
    store.w[1] = 0xFF80;
    store.w[9] = 0xFFFE;
    starts_over(store);
}
