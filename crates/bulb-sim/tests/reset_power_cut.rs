//! Power cuts during the writes of the factory reset counter
//! (`bulb_core::reset`, the words the firmware keeps at 0xCF000), on the
//! NOR flash model and with the cut model of `power_cut.rs` (`bulb_sim::cut`).
//!
//! A bulb is powered on and off along a walk that goes through every step of
//! the reset sequence, broken sequences, long sessions and the reuse of the
//! sector. Every flash operation of every power-on is cut in every way the
//! model knows. For each state a cut leaves, the fewest power-ons that still
//! end in a factory reset (whatever they last) are counted: a cut must never
//! bring the reset nearer than the operation it interrupted would have, with
//! the power cut just before it or just after it. Otherwise a badly timed
//! power cut, and a few ordinary power-ons after it, would take the bulb out
//! of its network.

use std::collections::HashSet;

use bulb_core::reset::{PowerCycles, WordStore};
use bulb_core::store::Flash;
use bulb_sim::cut::{cuts, Rng};
use bulb_sim::{FlashOp, NorFlash};

const BASE: usize = 0xC_F000;
/// Power-ons of the shortest sequence from a fresh counter: five, then the
/// one that finds it complete.
const STOCK_PATTERN: usize = 6;
/// One of each kind of power-on as the counter sees them: shorter than 6 s,
/// 6 to 30 s, longer than 30 s.
const KINDS: [u64; 3] = [1, 12, 40];

/// The counter's words on the flash model.
#[derive(Clone)]
struct NorWords(NorFlash);

impl WordStore for NorWords {
    fn slots(&self) -> usize {
        self.0.sector_size() / 2
    }
    fn read(&self, slot: usize) -> u16 {
        let mut b = [0u8; 2];
        self.0.read(BASE + slot * 2, &mut b);
        u16::from_le_bytes(b)
    }
    fn write(&mut self, slot: usize, word: u16) {
        self.0.program(BASE + slot * 2, &word.to_le_bytes());
    }
    fn erase(&mut self) {
        self.0.erase(BASE);
    }
}

fn medium(sector: usize) -> NorFlash {
    NorFlash::new(BASE, sector, sector)
}

fn contents(f: &NorFlash) -> Vec<u8> {
    let mut b = vec![0u8; f.sector_size()];
    f.read(BASE, &mut b);
    b
}

/// Powers the bulb on `f` (changed in place) for `seconds`; whether this
/// power-on was a factory reset.
fn power(f: &NorFlash, seconds: u64) -> bool {
    let (mut c, factory) = PowerCycles::power_on(NorWords(f.clone()));
    for ms in (0..=seconds * 1000).step_by(500) {
        c.tick(ms);
    }
    factory
}

/// The fewest power-ons from the flash `f` of which the last is a factory
/// reset; `STOCK_PATTERN + 1` when it takes more than the stock pattern.
fn need(f: &NorFlash) -> usize {
    let mut level = vec![f.snapshot()];
    let mut seen = HashSet::new();
    for n in 1..=STOCK_PATTERN {
        let mut next = Vec::new();
        for s in &level {
            for secs in KINDS {
                let t = s.snapshot();
                if power(&t, secs) {
                    return n;
                }
                if seen.insert(contents(&t)) {
                    next.push(t);
                }
            }
        }
        level = next;
    }
    STOCK_PATTERN + 1
}

#[derive(Default)]
struct Report {
    cuts: usize,
    early: Vec<String>,
}

/// Walks `walk` (power-on lengths in seconds) on a counter of `sector`
/// bytes, cutting every operation of every `cut_every`-th power-on and of
/// every power-on that erases the sector.
fn run(sector: usize, walk: &[u64], cut_every: usize) -> Report {
    let f = medium(sector);
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    let mut rep = Report::default();
    for (i, &secs) in walk.iter().enumerate() {
        let pre = f.snapshot();
        f.clear_ops();
        power(&f, secs);
        let ops = f.ops();
        if i % cut_every != 0 && !ops.iter().any(|o| matches!(o, FlashOp::Erase { .. })) {
            continue;
        }
        let replay = pre.snapshot();
        for (j, op) in ops.iter().enumerate() {
            let before = need(&replay);
            let done = replay.snapshot();
            done.apply(op);
            let after = need(&done);
            let fair = before.min(after);
            for (label, cut) in cuts(&replay, op, &mut rng) {
                rep.cuts += 1;
                let n = need(&cut);
                if n < fair {
                    let words = NorWords(cut.clone());
                    let active = (0..words.slots()).map(|k| (k, words.read(k))).find(|&(_, w)| w != 0);
                    rep.early.push(format!(
                        "power-on #{i} ({secs} s), operation {j} ({op:?}), cut {label}: a reset after {n} power-on(s), not {fair}; first word in use {active:04X?}"
                    ));
                }
            }
            replay.apply(op);
        }
    }
    rep
}

/// Power-ons that go through every step of the sequence, broken sequences,
/// complete ones (each a reset), long sessions and short blips.
fn walk(len: usize) -> Vec<u64> {
    let patterns: [&[u64]; 8] = [
        &[1, 1, 1, 12, 12, 1],
        &[1, 1, 1, 2],
        &[1, 1, 1, 12, 2],
        &[1, 1, 1, 12, 12, 40],
        &[40],
        &[12, 12],
        &[1, 1, 1, 1, 1],
        &[1, 1, 1, 31],
    ];
    let mut rng = Rng(77);
    let mut out = Vec::new();
    while out.len() < len {
        out.extend_from_slice(patterns[(rng.next() % patterns.len() as u64) as usize]);
    }
    out
}

fn assert_never_early(rep: &Report, min_cuts: usize) {
    eprintln!("{} cuts, {} bring the reset nearer", rep.cuts, rep.early.len());
    assert!(
        rep.early.is_empty(),
        "{} of {} cuts bring the factory reset nearer, e.g.:\n{}",
        rep.early.len(),
        rep.cuts,
        rep.early[..rep.early.len().min(12)].join("\n")
    );
    assert!(rep.cuts >= min_cuts, "only {} cuts", rep.cuts);
}

#[test]
fn the_stock_pattern_on_the_flash_model() {
    let f = medium(0x1000);
    let resets: Vec<bool> = [1, 1, 1, 12, 12, 1].iter().map(|&s| power(&f, s)).collect();
    assert_eq!(resets, [false, false, false, false, false, true]);
    assert_eq!(need(&medium(0x1000)), STOCK_PATTERN);
}

#[test]
fn no_cut_brings_the_reset_nearer_small_sector() {
    // 16 words: the sector is erased every few sequences
    let rep = run(32, &walk(400), 1);
    assert_never_early(&rep, 10_000);
}

#[test]
fn no_cut_brings_the_reset_nearer_real_sector() {
    // 4 KB as on the bulb: 2048 words, the walk wraps once
    let rep = run(0x1000, &walk(2100), 23);
    assert_never_early(&rep, 2_000);
}
