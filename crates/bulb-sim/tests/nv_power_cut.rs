//! Power cuts at every point of the Zigbee stack's store
//! (`bulb_core::nv::NvLog`, the firmware's 0xF1000 and 0xF2000: network
//! keys, frame counters, child table, ...), with the NOR flash and cut models
//! of `power_cut.rs` (`bulb_sim::cut`).
//!
//! A workload like the stack's (the frame counters and the reset counters
//! together at every start, the frame counters alone, every other item now
//! and then, the three parts of the child table) runs through many sector
//! switches. Every flash operation of every store is cut in every way the
//! model knows; after each cut the store is opened again on that flash and
//! every item must be as it was before the store, or (only the items being
//! stored) as stored. The frame counters must never go back, and must never
//! be lost once stored. From many of the cut states the store then carries
//! on and must come back with what it stored after the cut.
//!
//! A record torn part way through can pass its CRC-8 (one in 256): the
//! format has nothing stronger. Such cuts are counted on their own and only
//! allowed for the item whose record was being written, at about that rate.

use std::collections::BTreeMap;

use bulb_core::nv::NvLog;
use bulb_sim::cut::{cuts, Rng};
use bulb_sim::{FlashOp, NorFlash};
use zigbee::hal::NvItem;

const SECTORS: [usize; 2] = [0xF_1000, 0xF_2000];
const SIZE: usize = 0x1000;
const N: usize = NvItem::MAX_ID as usize + 1;
const COUNTERS: u8 = NvItem::Counters as u8;
const BOOT: u8 = NvItem::Boot as u8;
type Nv = NvLog<NorFlash, N>;
type Items = BTreeMap<u8, Vec<u8>>;

fn medium() -> NorFlash {
    NorFlash::new(SECTORS[0], 2 * SIZE, SIZE)
}

fn open(f: &NorFlash) -> Nv {
    NvLog::new(f.clone(), SECTORS, SIZE, || {})
}

/// Every item, as `load` finds it (one pass: `each`; `each_is_load` checks
/// that the two agree).
fn all(nv: &Nv) -> Items {
    let mut out = Items::new();
    nv.each(|id, d| {
        out.insert(id, d.to_vec());
    });
    out
}

fn all_by_load(nv: &Nv) -> Items {
    let mut out = Items::new();
    for id in 1..N as u8 {
        let mut buf = [0u8; bulb_core::nv::MAX_ITEM];
        if let Some(n) = nv.load(id, &mut buf) {
            out.insert(id, buf[..n].to_vec());
        }
    }
    out
}

/// The longest record of each item, about as the stack writes them.
fn max_len(id: u8) -> usize {
    match id {
        COUNTERS | BOOT => 8,
        // network, tables, the child table's three parts, scenes
        1 | 3 | 4 | 13 | 14 | 5 => 160,
        7 => 16,
        _ => 48,
    }
}

#[derive(Clone, Debug)]
enum Op {
    Store(u8, Vec<u8>),
    Pair((u8, Vec<u8>), (u8, Vec<u8>)),
    /// Forget everything but this item (a Leave, a factory reset).
    Forget(u8),
}

impl Op {
    fn run(&self, nv: &mut Nv) {
        match self {
            Op::Store(id, d) => nv.store(*id, d),
            Op::Pair((a, da), (b, db)) => nv.store_pair((*a, da), (*b, db)),
            Op::Forget(keep) => nv.forget_all_but(*keep),
        }
    }
    /// The items as they are once the operation is complete.
    fn after(&self, before: &Items) -> Items {
        let mut m = before.clone();
        match self {
            Op::Store(id, d) => {
                m.insert(*id, d.clone());
            }
            Op::Pair((a, da), (b, db)) => {
                m.insert(*a, da.clone());
                m.insert(*b, db.clone());
            }
            Op::Forget(keep) => m.retain(|id, _| id == keep),
        }
        m
    }
}

struct Work {
    rng: Rng,
    counter: u32,
}

impl Work {
    fn counters(&mut self) -> Vec<u8> {
        // stored ahead of use, a step at a time
        self.counter += 1 + (self.rng.next() % 1000) as u32;
        let mut d = self.counter.to_le_bytes().to_vec();
        d.extend_from_slice(&(self.counter / 3).to_le_bytes());
        d
    }
    fn item(&mut self, id: u8) -> Vec<u8> {
        let max = max_len(id);
        let len = max / 2 + (self.rng.next() as usize) % (max - max / 2 + 1);
        (0..len).map(|_| self.rng.byte()).collect()
    }
    fn next(&mut self) -> Op {
        match self.rng.next() % 8 {
            0 | 1 => {
                let c = self.counters();
                let b = self.item(BOOT);
                Op::Pair((COUNTERS, c), (BOOT, b))
            }
            2 => Op::Store(COUNTERS, self.counters()),
            3 => {
                // a child joined: one part of the child table
                let id = NvItem::children((self.rng.next() % 3) as usize) as u8;
                Op::Store(id, self.item(id))
            }
            _ => {
                let id = 1 + (self.rng.next() % (N as u64 - 1)) as u8;
                let d = if id == COUNTERS { self.counters() } else { self.item(id) };
                Op::Store(id, d)
            }
        }
    }
}

fn counter(items: &Items) -> Option<u32> {
    items.get(&COUNTERS).map(|d| u32::from_le_bytes([d[0], d[1], d[2], d[3]]))
}

#[derive(Default)]
struct Report {
    cuts: usize,
    /// cuts in the middle of a program
    torn_cuts: usize,
    recoveries: usize,
    switches: usize,
    /// a torn record of the item being written passed its CRC
    crc_passes: Vec<String>,
    lost: Vec<String>,
}

/// What may be found after a cut during `op` on `before`: `None` when it is.
fn judge(op: &Op, before: &Items, got: &Items) -> Option<String> {
    let after = op.after(before);
    if let Op::Forget(keep) = op {
        // all or nothing, and the kept item as it was
        if *got == *before || *got == after {
            return None;
        }
        return Some(format!("forgetting all but {keep}: items {:?}, had {:?}", got.keys().collect::<Vec<_>>(), before.keys().collect::<Vec<_>>()));
    }
    for id in 1..N as u8 {
        let (g, b, a) = (got.get(&id), before.get(&id), after.get(&id));
        if g != b && g != a {
            return Some(format!("item {id}: {} bytes {:02X?}..., before {:?}, stored {:?}", g.map_or(0, |v| v.len()), g.map(|v| &v[..v.len().min(6)]), b.map(|v| v.len()), a.map(|v| v.len())));
        }
    }
    None
}

/// Runs `ops` on `f`, cutting every operation of every `cut_every`-th one
/// and of every one that switches sectors or forgets. From every
/// `recover_every`-th cut the store carries on with `recover` more operations.
fn run(f: &NorFlash, ops: &[Op], cut_every: usize, recover_every: usize, recover: usize) -> Report {
    let mut rng = Rng(0xA076_1D64_78BD_642F);
    let mut work = Work { rng: Rng(99), counter: 1 << 30 };
    let more: Vec<Op> = (0..recover).map(|_| work.next()).collect();
    let mut rep = Report::default();
    let mut nv = open(f);
    let mut model = all(&nv);
    for (i, op) in ops.iter().enumerate() {
        let pre = f.snapshot();
        f.clear_ops();
        op.run(&mut nv);
        let fops = f.ops();
        let erases = fops.iter().any(|o| matches!(o, FlashOp::Erase { .. }));
        let header = fops.iter().any(|o| matches!(o, FlashOp::Program { addr, data } if SECTORS.contains(addr) && data.len() == 4));
        rep.switches += header as usize;
        if i % cut_every == 0 || erases || header || matches!(op, Op::Forget(_)) {
            let replay = pre.snapshot();
            let last = fops.len() - 1;
            for (j, fop) in fops.iter().enumerate() {
                // the programs of a switch before its last record and header
                // leave a sector that is not active yet: a sample of their cuts
                let thin = header && j + 2 < last && matches!(fop, FlashOp::Program { .. });
                for (k, (label, cut)) in cuts(&replay, fop, &mut rng).into_iter().enumerate() {
                    if thin && k > 1 && k % 8 != 0 {
                        continue;
                    }
                    rep.cuts += 1;
                    // the item's record (of one or more programs) part way written
                    let torn = !(j == 0 && label == "before") && !(j == last && label == "after");
                    rep.torn_cuts += torn as usize;
                    let got = all(&open(&cut));
                    let where_ = format!("op #{i} {}, flash op {j}/{} ({}), cut {label}", name(op), fops.len(), short(fop));
                    if counter(&got) < counter(&model) {
                        rep.lost.push(format!("{where_}: the frame counters went back: {:?} < {:?}", counter(&got), counter(&model)));
                        continue;
                    }
                    if let Some(why) = judge(op, &model, &got) {
                        let writes_it = |id: u8| match op {
                            Op::Store(s, _) => *s == id,
                            Op::Pair((a, _), (b, _)) => *a == id || *b == id,
                            Op::Forget(_) => false,
                        };
                        let only_written = (1..N as u8).all(|id| got.get(&id) == model.get(&id) || got.get(&id) == op.after(&model).get(&id) || writes_it(id));
                        if torn && only_written {
                            rep.crc_passes.push(format!("{where_}: {why}"));
                        } else {
                            rep.lost.push(format!("{where_}: {why}"));
                        }
                        continue;
                    }
                    if recover > 0 && rep.cuts % recover_every == 0 {
                        rep.recoveries += 1;
                        let mut t = open(&cut);
                        let mut want = got.clone();
                        for m in &more {
                            m.run(&mut t);
                            want = m.after(&want);
                        }
                        let back = all(&open(&cut));
                        if back != want {
                            let diff: Vec<u8> = (1..N as u8).filter(|id| back.get(id) != want.get(id)).collect();
                            rep.lost.push(format!("{where_}, then {recover} more operations: items {diff:?} differ"));
                        }
                    }
                }
                replay.apply(fop);
            }
        }
        model = op.after(&model);
        let clean = all(&open(f));
        if clean != model {
            rep.lost.push(format!("op #{i} {} without a cut: items differ", name(op)));
        }
    }
    rep
}

fn name(op: &Op) -> String {
    match op {
        Op::Store(id, d) => format!("store {id} ({} B)", d.len()),
        Op::Pair((a, _), (b, _)) => format!("store {a}+{b}"),
        Op::Forget(k) => format!("forget all but {k}"),
    }
}

fn short(op: &FlashOp) -> String {
    match op {
        FlashOp::Program { addr, data } => format!("program {addr:#x} {} B", data.len()),
        FlashOp::Erase { addr } => format!("erase {addr:#x}"),
    }
}

fn assert_safe(rep: &Report, min_cuts: usize) {
    eprintln!(
        "{} cuts ({} part way through a program), {} recoveries, {} switches; {} lost, {} torn records passed their CRC",
        rep.cuts, rep.torn_cuts, rep.recoveries, rep.switches, rep.lost.len(), rep.crc_passes.len()
    );
    for c in rep.crc_passes.iter().take(3) {
        eprintln!("  {c}");
    }
    assert!(rep.lost.is_empty(), "{} of {} cuts lost the state, e.g.:\n{}", rep.lost.len(), rep.cuts, rep.lost[..rep.lost.len().min(10)].join("\n"));
    assert!(rep.cuts >= min_cuts, "only {} cuts", rep.cuts);
    // CRC-8: about one torn record in 256 passes
    assert!(rep.crc_passes.len() * 128 <= rep.torn_cuts.max(128), "{} of {} torn records passed", rep.crc_passes.len(), rep.torn_cuts);
}

fn workload(n: usize, seed: u64) -> Vec<Op> {
    let mut w = Work { rng: Rng(seed), counter: 1000 };
    (0..n).map(|_| w.next()).collect()
}

#[test]
fn every_cut_during_stores_and_switches() {
    let f = medium();
    let rep = run(&f, &workload(700, 7), 4, 7, 60);
    assert!(rep.switches >= 8, "{} switches", rep.switches);
    assert_safe(&rep, 40_000);
}

#[test]
fn every_cut_while_forgetting_keeps_the_frame_counters() {
    // Leave and factory reset: everything goes but the frame counters,
    // with the store fresh, part full and just switched
    for (n, seed) in [(5, 1), (40, 2), (120, 3), (333, 4)] {
        let f = medium();
        let mut ops = workload(n, seed);
        ops.push(Op::Forget(COUNTERS));
        ops.extend(workload(30, seed + 10));
        ops.push(Op::Forget(COUNTERS));
        let rep = run(&f, &ops, usize::MAX, 5, 30);
        assert_safe(&rep, 200);
    }
}

#[test]
fn the_child_table_and_every_item_survive_many_switches() {
    let f = medium();
    let mut nv = open(&f);
    let mut model = Items::new();
    let mut w = Work { rng: Rng(5), counter: 0 };
    for id in 1..N as u8 {
        let d = w.item(id);
        nv.store(id, &d);
        model.insert(id, d);
    }
    f.clear_ops();
    for _ in 0..6000 {
        let d = w.counters();
        nv.store(COUNTERS, &d);
        model.insert(COUNTERS, d);
    }
    let switches = f.ops().iter().filter(|o| matches!(o, FlashOp::Program { addr, .. } if SECTORS.contains(addr))).count();
    assert!(switches >= 20, "{switches} switches");
    assert_eq!(all_by_load(&open(&f)), model);
}

#[test]
fn each_is_load() {
    // the one-pass reader the cut tests use finds what `load` finds, torn
    // records and all
    let f = medium();
    let mut nv = open(&f);
    let mut rng = Rng(3);
    let mut checked = 0;
    for (i, op) in workload(400, 11).iter().enumerate() {
        let pre = f.snapshot();
        f.clear_ops();
        op.run(&mut nv);
        if i % 3 == 0 {
            let replay = pre.snapshot();
            for fop in f.ops() {
                for (_, cut) in cuts(&replay, &fop, &mut rng).into_iter().step_by(5) {
                    let c = open(&cut);
                    assert_eq!(all(&c), all_by_load(&c));
                    checked += 1;
                }
                replay.apply(&fop);
            }
        }
    }
    assert!(checked > 1000, "{checked}");
}
