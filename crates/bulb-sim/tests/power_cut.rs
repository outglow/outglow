use bulb_core::config::{Config, BLOCK_LEN};
use bulb_core::store::LightLog;
use bulb_core::{Bulb, Command, PowerOnBehavior};
use bulb_sim::cut::{cuts, Rng};
use bulb_sim::{FlashOp, MockClock, MockPwm, MockRadio, NorFlash};

const A: usize = 0xF_0000;
const B: usize = 0xF_4000;
type Rec = [u8; BLOCK_LEN];

fn medium(size: usize) -> NorFlash {
    NorFlash::new(A, B + size - A, size)
}

fn states(n: usize, seed: usize) -> Vec<Rec> {
    (0..n)
        .map(|i| {
            let k = i + seed;
            Config {
                last_level: 1 + (k * 37 % 254) as u8,
                last_on: k % 3 != 0,
                power_on_behavior: if k % 5 == 0 { PowerOnBehavior::Previous } else { PowerOnBehavior::On },
                ..Config::default()
            }
            .encode()
        })
        .collect()
}

trait Store {
    fn boot(f: &NorFlash) -> Self;
    fn store(&mut self, r: &Rec);
    fn load(&self) -> Option<Rec>;
}

impl Store for LightLog<NorFlash> {
    fn boot(f: &NorFlash) -> Self {
        LightLog::new(f.clone(), [A, B], f.sector_size())
    }
    fn store(&mut self, r: &Rec) {
        LightLog::store(self, r)
    }
    fn load(&self) -> Option<Rec> {
        self.current().filter(|r| Config::decode(r).is_some())
    }
}

struct OldStore {
    f: NorFlash,
    next: usize,
}

impl OldStore {
    fn slot(&self, off: usize) -> Rec {
        let mut b = [0u8; BLOCK_LEN];
        bulb_core::store::Flash::read(&self.f, A + off, &mut b);
        b
    }
}

impl Store for OldStore {
    fn boot(f: &NorFlash) -> Self {
        let mut s = OldStore { f: f.clone(), next: 0 };
        let mut off = 0;
        while off < f.sector_size() {
            if s.slot(off).iter().any(|&b| b != 0xFF) {
                s.next = off + BLOCK_LEN;
            }
            off += BLOCK_LEN;
        }
        s
    }
    fn store(&mut self, r: &Rec) {
        use bulb_core::store::Flash;
        if self.next != 0 && self.slot(self.next - BLOCK_LEN) == *r {
            return;
        }
        if self.next + BLOCK_LEN > self.f.sector_size() {
            self.f.erase(A);
            self.next = 0;
        }
        self.f.program(A + self.next, r);
        self.next += BLOCK_LEN;
    }
    fn load(&self) -> Option<Rec> {
        if self.next == 0 {
            return None;
        }
        Some(self.slot(self.next - BLOCK_LEN)).filter(|r| Config::decode(r).is_some())
    }
}

#[derive(Default, Debug)]
struct Report {
    cuts: usize,
    recoveries: usize,
    lost: Vec<String>,
}

fn run<S: Store>(f: &NorFlash, mut prev: Option<Rec>, recs: &[Rec], recover_every: usize, recover: usize) -> Report {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut rep = Report::default();
    let mut s = S::boot(f);
    assert_eq!(s.load(), prev, "the boot before the first store");
    let more = states(recover, 1000);
    for (i, r) in recs.iter().enumerate() {
        let pre = f.snapshot();
        f.clear_ops();
        s.store(r);
        let ops = f.ops();
        let erases = ops.iter().any(|o| matches!(o, FlashOp::Erase { .. }));
        let replay = pre.snapshot();
        for (j, op) in ops.iter().enumerate() {
            for (label, cut) in cuts(&replay, op, &mut rng) {
                rep.cuts += 1;
                let got = S::boot(&cut).load();
                if got != Some(*r) && got != prev {
                    rep.lost.push(format!("store #{i}, operation {j} ({op:?}), cut {label}: got {got:?}"));
                    continue;
                }
                if recover > 0 && (erases || i % recover_every == 0) {
                    rep.recoveries += 1;
                    let mut t = S::boot(&cut);
                    for m in &more {
                        t.store(m);
                    }
                    let back = S::boot(&cut).load();
                    if back != more.last().copied() {
                        rep.lost.push(format!("after store #{i} cut at operation {j} ({label}), {recover} more stores: got {back:?}"));
                    }
                }
            }
            replay.apply(op);
        }
        prev = Some(*r);
        let clean = S::boot(f).load();
        if clean != prev {
            rep.lost.push(format!("store #{i} without a cut: got {clean:?}"));
        }
    }
    rep
}

fn assert_safe(rep: &Report, min_cuts: usize) {
    eprintln!("{} cuts, {} recoveries, {} lost", rep.cuts, rep.recoveries, rep.lost.len());
    assert!(rep.lost.is_empty(), "{} of {} cuts lost the state, e.g.:\n{}", rep.lost.len(), rep.cuts, rep.lost[..rep.lost.len().min(10)].join("\n"));
    assert!(rep.cuts >= min_cuts, "only {} cuts", rep.cuts);
}

#[test]
fn small_sectors_every_cut_and_recovery() {
    let f = medium(0x80);
    let rep = run::<LightLog<NorFlash>>(&f, None, &states(80, 0), 1, 24);
    assert_safe(&rep, 5000);
    assert!(rep.recoveries > 4000);
}

#[test]
fn real_sectors_every_cut() {
    let f = medium(0x1000);
    let rep = run::<LightLog<NorFlash>>(&f, None, &states(600, 0), 50, 300);
    assert_safe(&rep, 20_000);
    assert!(rep.recoveries > 500);
}

fn legacy(f: &NorFlash, n: usize) -> Vec<Rec> {
    let recs = states(n, 500);
    for (k, r) in recs.iter().enumerate() {
        f.poke(A + k * BLOCK_LEN, r);
    }
    recs
}

#[test]
fn after_the_earlier_firmware_its_last_record_counts_until_the_first_store() {
    for n in [1, 37, 512] {
        let f = medium(0x1000);
        let old = legacy(&f, n);
        let rep = run::<LightLog<NorFlash>>(&f, old.last().copied(), &states(300, 0), 25, 300);
        assert_safe(&rep, 5000);
        assert_ne!(f.byte(B), 0xFF);
    }
}

#[test]
fn a_torn_last_record_of_the_earlier_firmware_is_skipped() {
    let f = medium(0x1000);
    let old = legacy(&f, 40);
    let torn = states(1, 9)[0];
    f.poke(A + 40 * BLOCK_LEN, &[torn[0], torn[1], torn[2], torn[3], 0xFF, 0xFF, 0xFF, 0xFF]);
    let s = <LightLog<NorFlash> as Store>::boot(&f);
    assert_eq!(s.load(), old.last().copied());
}

#[test]
fn the_earlier_single_sector_store_loses_the_state() {
    let f = medium(0x80);
    let rep = run::<OldStore>(&f, None, &states(60, 0), 1, 0);
    let erase = rep.lost.iter().filter(|l| l.contains("Erase")).count();
    let torn = rep.lost.iter().filter(|l| l.contains("Program") && l.contains("programmed")).count();
    eprintln!("earlier store: {} cuts, {} lost ({erase} in an erase or right after it, {torn} by a torn record), e.g.\n{}",
              rep.cuts, rep.lost.len(), rep.lost.iter().filter(|l| l.ends_with("got None")).take(3).cloned().collect::<Vec<_>>().join("\n"));
    assert!(erase > 0 && torn > 0, "{rep:?}");
    assert!(rep.lost.iter().any(|l| l.ends_with("got None")));
}

#[test]
fn one_program_per_change_and_an_erase_only_every_255_changes() {
    let f = medium(0x1000);
    let mut s = LightLog::new(f.clone(), [A, B], 0x1000);
    let recs = states(1000, 0);
    f.clear_ops();
    for r in &recs {
        s.store(r);
        s.store(r);
    }
    let headers = f.ops().iter().filter(|o| matches!(o, FlashOp::Program { addr, .. } if *addr == A || *addr == B)).count();
    assert_eq!(headers, 1 + (recs.len() - 1) / 255);
    assert_eq!(f.programs(), recs.len() + headers);
    assert_eq!(f.erases(), headers - 2);
}

#[test]
fn the_bulb_comes_back_after_cuts_during_commands() {
    let f = medium(0x1000);
    let mut rng = Rng(12345);
    let boot = |f: &NorFlash| Bulb::boot(MockPwm::new(1000), MockClock::new(), LightLog::new(f.clone(), [A, B], 0x1000), MockRadio::new());
    let mut bulb = boot(&f);
    bulb.handle(Command::SetPowerOnBehavior(PowerOnBehavior::Previous));
    let mut lost = Vec::new();
    let mut n_cuts = 0;
    for k in 0..1500u32 {
        let before = bulb.state();
        let cmd = match rng.next() % 4 {
            0 => Command::Toggle,
            1 => Command::Off,
            _ => Command::MoveToLevel { level: 1 + (rng.next() % 254) as u8, transition_ms: 0 },
        };
        let pre = f.snapshot();
        f.clear_ops();
        bulb.handle(cmd);
        let after = bulb.state();
        let ops = f.ops();
        if ops.is_empty() || k % 3 != 0 {
            continue;
        }
        let j = (rng.next() as usize) % ops.len();
        let replay = pre.snapshot();
        for op in &ops[..j] {
            replay.apply(op);
        }
        let variants = cuts(&replay, &ops[j], &mut rng);
        let (label, cut) = &variants[(rng.next() as usize) % variants.len()];
        n_cuts += 1;
        let back = boot(cut).state();
        let same = |s: &bulb_core::StateReport| (s.on, s.level) == (back.on, back.level) && back.power_on_behavior == PowerOnBehavior::Previous;
        if !same(&before) && !same(&after) {
            lost.push(format!("command #{k} {cmd:?}, cut {label}: {back:?}, before {before:?}, after {after:?}"));
        }
        f.poke(A, &read_all(cut, A, B + 0x1000 - A));
        bulb = boot(&f);
    }
    assert!(lost.is_empty(), "{}", lost.join("\n"));
    assert!(n_cuts > 400);
}

fn read_all(f: &NorFlash, from: usize, n: usize) -> Vec<u8> {
    let mut b = vec![0u8; n];
    bulb_core::store::Flash::read(f, from, &mut b);
    b
}
