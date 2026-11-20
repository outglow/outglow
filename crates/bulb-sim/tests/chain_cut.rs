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
    Prepare,
}

impl Op {
    fn run(&self, nv: &mut Nv) {
        match self {
            Op::Store(id, d) => nv.store(*id, d),
            Op::Pair((a, da), (b, db)) => nv.store_pair((*a, da), (*b, db)),
            Op::Forget(keep) => nv.forget_all_but(*keep),
            Op::Prepare => {
                nv.prepare();
            }
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
            Op::Prepare => {}
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
    match rng.next() % 24 {
        0 => Op::Forget(COUNTERS),
        20..=23 => Op::Prepare,
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

#[test]
fn nv_log_chained_cuts() {
    let chains: u64 = std::env::var("CHAINS").ok().and_then(|s| s.parse().ok()).unwrap_or(200);
    let (mut cuts_done, mut ops_done, mut lost) = (0usize, 0usize, Vec::new());
    for chain in 0..chains {
        let mut rng = Rng(0xC0FF_EE00 + chain * 7919);
        let mut ctr = 1u32 << 20;
        let mut f = NorFlash::new(SECTORS[0], 2 * SIZE, SIZE);
        let mut model = Items::new();
        let ops = 200 + (rng.next() % 400) as usize;
        for i in 0..ops {
            let op = next_op(&mut rng, &mut ctr);
            let pre = f.snapshot();
            let mut nv: Nv = NvLog::new(f.clone(), SECTORS, SIZE, || {});
            f.clear_ops();
            op.run(&mut nv);
            ops_done += 1;
            let after = op.after(&model);
            let fops = f.ops();
            if fops.is_empty() || rng.next() % 3 != 0 {
                let got = all(&f);
                if got != after {
                    lost.push(format!("chain {chain} op {i}: without a cut the items differ"));
                    break;
                }
                model = after;
                continue;
            }
            let j = (rng.next() as usize) % fops.len();
            let replay = pre.snapshot();
            for o in &fops[..j] {
                replay.apply(o);
            }
            let variants = cuts(&replay, &fops[j], &mut rng);
            let k = (rng.next() as usize) % variants.len();
            let (label, cut) = variants.into_iter().nth(k).unwrap();
            cuts_done += 1;
            let got = all(&cut);
            if counter(&got) < counter(&model) {
                lost.push(format!("chain {chain} op {i}: frame counter went back {:?} < {:?} (cut {label} in flash op {j}/{})", counter(&got), counter(&model), fops.len()));
                break;
            }
            let mut bad = false;
            for id in 1..N as u8 {
                let (g, b, a) = (got.get(&id), model.get(&id), after.get(&id));
                if g == b || g == a {
                    continue;
                }
                bad = true;
                lost.push(format!(
                    "chain {chain} op {i} {}: item {id} = {:?} B, had {:?} B, stored {:?} B (cut {label} in flash op {j}/{} {:?})",
                    match &op { Op::Store(id, d) => format!("store {id} ({} B)", d.len()), Op::Pair((a, _), (b, _)) => format!("pair {a}+{b}"), Op::Forget(k) => format!("forget all but {k}"), Op::Prepare => "prepare".into() },
                    g.map(|v| v.len()), b.map(|v| v.len()), a.map(|v| v.len()), fops.len(), match &fops[j] { bulb_sim::FlashOp::Program { addr, data } => format!("program {addr:#x} {} B", data.len()), bulb_sim::FlashOp::Erase { addr } => format!("erase {addr:#x}") }
                ));
            }
            if bad {
                break;
            }
            f = cut;
            model = got;
        }
    }
    eprintln!("NvLog: {chains} chains, {ops_done} operations, {cuts_done} cuts carried on from, {} lost", lost.len());
    for l in lost.iter().take(10) {
        eprintln!("  {l}");
    }
    assert!(lost.is_empty());
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

#[test]
fn light_log_chained_cuts() {
    let chains: u64 = std::env::var("CHAINS").ok().and_then(|s| s.parse().ok()).unwrap_or(200);
    light_log_chains(0xBEEF_0000, chains, &[0x40, 0x80, 0x1000], 50, 3);
}

#[test]
fn light_log_cut_in_every_operation_of_a_small_log() {
    let chains: u64 = std::env::var("CHAINS").ok().and_then(|s| s.parse().ok()).unwrap_or(150);
    light_log_chains(0xD0B1_0000, chains, &[0x40], 4, 1);
}

fn light_log_chains(seed: u64, chains: u64, sizes: &[usize], erase_every: u64, cut_every: u64) {
    const A: usize = 0xF_0000;
    const B: usize = 0xF_4000;
    let (mut cuts_done, mut ops_done, mut lost) = (0usize, 0usize, Vec::new());
    for chain in 0..chains {
        let mut rng = Rng(seed + chain * 104_729);
        let size = sizes[chain as usize % sizes.len()];
        let mut f = NorFlash::new(A, B + size - A, size);
        let mut model: Option<[u8; BLOCK_LEN]> = None;
        let ops = 300 + (rng.next() % 900) as usize;
        for i in 0..ops {
            let erase = rng.next() % erase_every == 0;
            let prepare = !erase && rng.next() % 4 == 0;
            let r = rec(&mut rng);
            let pre = f.snapshot();
            let mut log = LightLog::new(f.clone(), [A, B], size);
            if log.current() != model {
                lost.push(format!("chain {chain} op {i}: reopened {:?}, expected {:?}", log.current(), model));
                break;
            }
            f.clear_ops();
            let after = match (erase, prepare) {
                (true, _) => {
                    log.erase();
                    None
                }
                (false, true) => {
                    log.prepare();
                    model
                }
                (false, false) => {
                    log.store(&r);
                    Some(r)
                }
            };
            ops_done += 1;
            let fops = f.ops();
            if fops.is_empty() || rng.next() % cut_every != 0 {
                model = after;
                continue;
            }
            let j = (rng.next() as usize) % fops.len();
            let replay = pre.snapshot();
            for o in &fops[..j] {
                replay.apply(o);
            }
            let variants = cuts(&replay, &fops[j], &mut rng);
            let k = (rng.next() as usize) % variants.len();
            let (label, cut) = variants.into_iter().nth(k).unwrap();
            cuts_done += 1;
            let got = LightLog::new(cut.clone(), [A, B], size).current();
            if got != model && got != after {
                lost.push(format!("chain {chain} op {i} ({}): got {:02x?}, before {:02x?}, after {:02x?} (cut {label} in flash op {j}/{} {:?})", match (erase, prepare) { (true, _) => "erase", (false, true) => "prepare", (false, false) => "store" }, got, model, after, fops.len(), fops[j]));
                break;
            }
            f = cut;
            model = got;
        }
    }
    eprintln!("LightLog: {chains} chains, {ops_done} operations, {cuts_done} cuts carried on from, {} lost", lost.len());
    for l in lost.iter().take(10) {
        eprintln!("  {l}");
    }
    assert!(lost.is_empty());
}

fn nv_crc8(id: u8, data: &[u8]) -> u8 {
    let mut crc = 0xffu8;
    for b in [id, data.len() as u8].iter().chain(data) {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 { (crc << 1) ^ 0x07 } else { crc << 1 };
        }
    }
    crc
}

#[test]
fn a_counters_record_torn_in_its_id_does_not_replace_the_tables() {
    let tables = NvItem::Tables as u8;
    let f = NorFlash::new(SECTORS[0], 2 * SIZE, SIZE);
    let mut nv: Nv = NvLog::new(f.clone(), SECTORS, SIZE, || {});
    let groups = vec![2u8, 1, 0x34, 0x12, 0, 0, 0];
    nv.store(tables, &groups);
    nv.store(COUNTERS, &[0x10, 0, 0, 0, 5, 0, 0, 0]);
    let counters = (0x20u32..)
        .map(|c| [c.to_le_bytes(), (c / 3).to_le_bytes()].concat())
        .find(|d| nv_crc8(tables, d) == 0xff)
        .unwrap();
    let pre = f.snapshot();
    f.clear_ops();
    nv.store(COUNTERS, &counters);
    let ops = f.ops();
    let start = ops.iter().filter_map(|o| match o { bulb_sim::FlashOp::Program { addr, .. } => Some(*addr), bulb_sim::FlashOp::Erase { .. } => None }).min().unwrap();
    for j in 0..ops.len() {
        let cut = pre.snapshot();
        ops[..j].iter().for_each(|o| cut.apply(o));
        let bulb_sim::FlashOp::Program { addr, .. } = ops[j] else { panic!("an erase in an append") };
        cut.apply_part(&ops[j], |i, _| match addr + i - start {
            0 => 0x01,
            10 => 0xff,
            _ => 0,
        });
        let after: Nv = NvLog::new(cut.clone(), SECTORS, SIZE, || {});
        let mut buf = [0u8; bulb_core::nv::MAX_ITEM];
        let n = after.load(tables, &mut buf);
        assert_eq!(n.map(|n| buf[..n].to_vec()), Some(groups.clone()), "a cut in flash op {j} of a frame counter store replaced the group table");
    }
}

#[test]
fn the_first_record_after_an_erase_torn_after_five_bytes_is_not_loaded() {
    const A: usize = 0xF_0000;
    const B: usize = 0xF_4000;
    let f = NorFlash::new(A, B + 0x1000 - A, 0x1000);
    let mut log = LightLog::new(f.clone(), [A, B], 0x1000);
    log.store(&Config::default().encode());
    log.erase();
    let wanted = (1u8..=254)
        .flat_map(|level| (0u8..=255).map(move |startup| (level, startup)))
        .map(|(level, startup)| {
            Config { last_level: level, startup_level: bulb_core::config::StartupLevel::from_u8(startup), last_color_temp: 250, startup_color_temp: 300, ..Config::default() }.encode()
        })
        .find(|r| {
            let torn = [r[0], r[1], r[2], r[3], r[4], 0xff, 0xff, 0xff];
            Config::decode(&torn).is_some() && torn != *r
        })
        .unwrap();
    let pre = f.snapshot();
    f.clear_ops();
    log.store(&wanted);
    let ops = f.ops();
    for j in 0..ops.len() {
        let cut = pre.snapshot();
        ops[..j].iter().for_each(|o| cut.apply(o));
        cut.apply_part(&ops[j], |i, _| if i < 5 { 0 } else { 0xff });
        let got = LightLog::new(cut, [A, B], 0x1000).current();
        assert!(got.is_none() || got == Some(wanted), "a cut 5 bytes into flash op {j} of the first record after an erase loads {got:02x?}, stored {wanted:02x?}");
    }
}
