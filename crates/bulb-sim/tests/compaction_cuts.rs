use memcap as _;
use std::collections::BTreeMap;

use bulb_core::config::{Config, BLOCK_LEN};
use bulb_core::nv::NvLog;
use bulb_core::store::LightLog;
use bulb_core::PowerOnBehavior;
use bulb_sim::cut::{cuts, Rng};
use bulb_sim::NorFlash;
use zigbee::hal::NvItem;

const SECTORS: [usize; 2] = [0xF_1000, 0xF_2000];
const SIZE: usize = 0x1000;
const N: usize = NvItem::MAX_ID as usize + 1;
const COUNTERS: u8 = NvItem::Counters as u8;
type Nv = NvLog<NorFlash, N>;
type Items = BTreeMap<u8, Vec<u8>>;

fn all(f: &NorFlash) -> Items {
    let nv: Nv = NvLog::new(f.clone(), SECTORS, SIZE, || {});
    let mut out = Items::new();
    nv.each(|id, d| {
        out.insert(id, d.to_vec());
    });
    out
}

#[derive(Clone, Debug)]
enum Op {
    Store(u8, Vec<u8>),
    Pair((u8, Vec<u8>), (u8, Vec<u8>)),
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

fn counter(items: &Items) -> Option<u32> {
    items.get(&COUNTERS).map(|d| u32::from_le_bytes([d[0], d[1], d[2], d[3]]))
}

fn next_op(rng: &mut Rng, counter: &mut u32) -> Op {
    let mut item = |rng: &mut Rng, id: u8| -> Vec<u8> {
        if id == COUNTERS {
            *counter += 1 + (rng.next() % 1000) as u32;
            let mut d = counter.to_le_bytes().to_vec();
            d.extend_from_slice(&(*counter / 3).to_le_bytes());
            return d;
        }
        let len = 1 + (rng.next() as usize) % 160;
        (0..len).map(|_| rng.byte()).collect()
    };
    match rng.next() % 20 {
        0 => Op::Forget(COUNTERS),
        1..=5 => {
            let c = item(rng, COUNTERS);
            let id = 1 + (rng.next() % (N as u64 - 1)) as u8;
            let id = if id == COUNTERS { 1 } else { id };
            let b = item(rng, id);
            Op::Pair((COUNTERS, c), (id, b))
        }
        6..=8 => Op::Store(COUNTERS, item(rng, COUNTERS)),
        _ => {
            let id = 1 + (rng.next() % (N as u64 - 1)) as u8;
            let d = item(rng, id);
            Op::Store(id, d)
        }
    }
}

fn rec(rng: &mut Rng) -> [u8; BLOCK_LEN] {
    Config {
        last_level: 1 + (rng.next() % 254) as u8,
        last_on: rng.next() % 2 == 0,
        power_on_behavior: if rng.next() % 3 == 0 { PowerOnBehavior::Previous } else { PowerOnBehavior::On },
        ..Config::default()
    }
    .encode()
}

fn chains(default: u64) -> (u64, u64) {
    let n = std::env::var("CHAINS").ok().and_then(|s| s.parse().ok()).unwrap_or(default);
    let s = std::env::var("CHAIN_START").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
    (s, n)
}

#[test]
fn nv_log_every_cut_in_compaction_and_forget() {
    let (s, n) = chains(12);
    let (mut checked, mut compactions, mut lost) = (0usize, 0usize, Vec::new());
    for chain in s..s + n {
        let mut rng = Rng(0xA11C_0000 + chain * 6151);
        let mut ctr = 1u32 << 20;
        let f = NorFlash::new(SECTORS[0], 2 * SIZE, SIZE);
        let mut model = Items::new();
        let mut nv: Nv = NvLog::new(f.clone(), SECTORS, SIZE, || {});
        let ops = 100 + (rng.next() % 400) as usize;
        for i in 0..ops {
            let op = next_op(&mut rng, &mut ctr);
            let pre = f.snapshot();
            f.clear_ops();
            op.run(&mut nv);
            let after = op.after(&model);
            let fops = f.ops();
            if fops.iter().any(|o| matches!(o, bulb_sim::FlashOp::Erase { .. })) {
                compactions += 1;
                let replay = pre.snapshot();
                for (j, o) in fops.iter().enumerate() {
                    for (label, cut) in cuts(&replay, o, &mut rng) {
                        checked += 1;
                        let got = all(&cut);
                        if counter(&got) < counter(&model) {
                            lost.push(format!("chain {chain} op {i} {op:?}: counter back, cut {label} in {j}/{}", fops.len()));
                        }
                        for id in 1..N as u8 {
                            let g = got.get(&id);
                            if g != model.get(&id) && g != after.get(&id) {
                                lost.push(format!("chain {chain} op {i}: item {id} {:?} cut {label} in op {j}/{} {:?}", g.map(|v| v.len()), fops.len(), o));
                            }
                        }
                        let mut again: Nv = NvLog::new(cut.clone(), SECTORS, SIZE, || {});
                        let probe = vec![0x42u8; 7];
                        again.store(5, &probe);
                        let g2 = all(&cut);
                        if g2.get(&5) != Some(&probe) {
                            lost.push(format!("chain {chain} op {i}: a store after cut {label} in {j} is not read back"));
                        }
                        for id in 1..N as u8 {
                            if id != 5 && g2.get(&id) != got.get(&id) {
                                lost.push(format!("chain {chain} op {i}: a store after cut {label} in {j} changed item {id}"));
                            }
                        }
                    }
                    replay.apply(o);
                }
                if lost.len() > 10 {
                    break;
                }
            }
            let got = all(&f);
            assert_eq!(got, after, "chain {chain} op {i}");
            model = after;
        }
    }
    eprintln!("NvLog compaction: {n} chains, {compactions} compactions or forgets, {checked} cut states, {} lost", lost.len());
    for l in lost.iter().take(10) {
        eprintln!("  {l}");
    }
    assert!(lost.is_empty());
}

#[test]
fn light_log_every_cut_in_a_sector_switch_or_erase() {
    const A: usize = 0xF_0000;
    const B: usize = 0xF_4000;
    let (s, n) = chains(40);
    let (mut checked, mut switches, mut lost) = (0usize, 0usize, Vec::new());
    for chain in s..s + n {
        let mut rng = Rng(0xB0B0_0000 + chain * 7907);
        let size = [0x40usize, 0x80, 0x1000][(chain % 3) as usize];
        let f = NorFlash::new(A, B + size - A, size);
        let mut model: Option<[u8; BLOCK_LEN]> = None;
        let ops = 100 + (rng.next() % 600) as usize;
        for _ in 0..ops {
            let erase = rng.next() % 40 == 0;
            let r = rec(&mut rng);
            let pre = f.snapshot();
            let mut log = LightLog::new(f.clone(), [A, B], size);
            f.clear_ops();
            if erase { log.erase() } else { log.store(&r) }
            let after = if erase { None } else { Some(r) };
            let fops = f.ops();
            if erase || (fops.iter().any(|o| matches!(o, bulb_sim::FlashOp::Erase { .. })) || fops.len() > 1) {
                switches += 1;
                let replay = pre.snapshot();
                for (j, o) in fops.iter().enumerate() {
                    for (label, cut) in cuts(&replay, o, &mut rng) {
                        checked += 1;
                        let got = LightLog::new(cut.clone(), [A, B], size).current();
                        if got != model && got != after {
                            lost.push(format!("chain {chain}: got {got:02x?} before {model:02x?} after {after:02x?} cut {label} in {j}/{} {o:?}", fops.len()));
                        }
                        let mut again = LightLog::new(cut.clone(), [A, B], size);
                        let probe = rec(&mut rng);
                        again.store(&probe);
                        let back = LightLog::new(cut.clone(), [A, B], size).current();
                        if back != Some(probe) {
                            lost.push(format!("chain {chain}: a store after cut {label} in {j}/{} reads {back:02x?}", fops.len()));
                        }
                    }
                    replay.apply(o);
                }
            }
            model = after;
        }
    }
    eprintln!("LightLog switches: {n} chains, {switches} switches or erases, {checked} cut states, {} lost", lost.len());
    for l in lost.iter().take(10) {
        eprintln!("  {l}");
    }
    assert!(lost.is_empty());
}

#[test]
fn a_cut_anywhere_in_erasing_the_light_log_loads_the_newest_record_or_nothing() {
    const A: usize = 0xF_0000;
    const B: usize = 0xF_4000;
    let mut rng = Rng(0xE7A5E);
    for n in 1..=12u8 {
        let f = NorFlash::new(A, B + 0x40 - A, 0x40);
        let mut log = LightLog::new(f.clone(), [A, B], 0x40);
        let recs: Vec<[u8; BLOCK_LEN]> = (1..=n).map(|l| Config { last_level: l * 10, ..Config::default() }.encode()).collect();
        for r in &recs {
            log.store(r);
        }
        let newest = recs.last().copied();
        let pre = f.snapshot();
        f.clear_ops();
        log.erase();
        let ops = f.ops();
        let replay = pre.snapshot();
        for (j, o) in ops.iter().enumerate() {
            for (label, cut) in cuts(&replay, o, &mut rng) {
                let got = LightLog::new(cut, [A, B], 0x40).current();
                assert!(got.is_none() || got == newest, "{n} records, a cut {label} in flash op {j}/{} of LightLog::erase loads {got:02x?}, not the newest {newest:02x?} or nothing", ops.len());
            }
            replay.apply(o);
        }
        assert_eq!(LightLog::new(f.clone(), [A, B], 0x40).current(), None);
    }
}

#[test]
fn an_erase_cut_after_the_new_header_and_then_cut_again_keeps_the_newest_record_or_nothing() {
    const A: usize = 0xF_0000;
    const B: usize = 0xF_4000;
    let recs: Vec<[u8; BLOCK_LEN]> = (1..=3u8).map(|l| Config { last_level: l * 10, ..Config::default() }.encode()).collect();
    let f = NorFlash::new(A, B + 0x40 - A, 0x40);
    let mut log = LightLog::new(f.clone(), [A, B], 0x40);
    for r in &recs {
        log.store(r);
    }
    let pre = f.snapshot();
    f.clear_ops();
    log.erase();
    let ops = f.ops();
    pre.apply(&ops[0]);
    let mut log = LightLog::new(pre.clone(), [A, B], 0x40);
    assert_eq!(log.current(), Some(recs[2]), "after a cut once the new header is in, the newest record");
    let before = pre.snapshot();
    pre.clear_ops();
    log.erase();
    let ops = pre.ops();
    let mut rng = Rng(0x2C07);
    for (j, o) in ops.iter().enumerate() {
        for (label, cut) in cuts(&before, o, &mut rng) {
            let got = LightLog::new(cut, [A, B], 0x40).current();
            assert!(got.is_none() || got == Some(recs[2]), "a second erase cut {label} in flash op {j}/{} loads {got:02x?}, not the newest (level 30) or nothing", ops.len());
        }
        before.apply(o);
    }
}
