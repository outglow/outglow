use std::cell::RefCell;
use std::rc::Rc;

use bulb_core::command::StateReport;
use bulb_core::hal::{Clock, ConfigStore, Pwm, Radio};
use bulb_core::Bulb;

use crate::sim::*;

const ON_OFF: u16 = 0x0006;
const LEVEL: u16 = 0x0008;
const COLOR: u16 = 0x0300;
const SCENES: u16 = 0x0005;

#[derive(Clone, Copy)]
struct P {
    cold: bool,
    rgb: bool,
}

impl Pwm for P {
    fn max_duty(&self) -> u16 {
        1000
    }
    fn set_duty(&mut self, _d: u16) {}
    fn has_cold_channel(&self) -> bool {
        self.cold
    }
    fn has_rgb(&self) -> bool {
        self.rgb
    }
    fn ct_range(&self) -> (u16, u16) {
        if self.rgb {
            (153, 370)
        } else {
            (bulb_core::CT_MIN, bulb_core::CT_MAX)
        }
    }
}

#[derive(Clone)]
struct Clk(Shared);

impl Clock for Clk {
    fn now_ms(&self) -> u64 {
        self.0.borrow().now as u64
    }
}

#[derive(Clone, Default)]
struct Store(Rc<RefCell<(Option<Vec<u8>>, Option<Vec<u8>>)>>);

impl ConfigStore for Store {
    fn load(&self, buf: &mut [u8]) -> Option<usize> {
        let d = self.0.borrow();
        let b = d.0.as_ref()?;
        let n = b.len().min(buf.len());
        buf[..n].copy_from_slice(&b[..n]);
        Some(n)
    }
    fn store(&mut self, data: &[u8]) {
        self.0.borrow_mut().0 = Some(data.to_vec());
    }
    fn load_colour(&self, buf: &mut [u8]) -> Option<usize> {
        let d = self.0.borrow();
        let b = d.1.as_ref()?;
        let n = b.len().min(buf.len());
        buf[..n].copy_from_slice(&b[..n]);
        Some(n)
    }
    fn store_colour(&mut self, data: &[u8]) {
        self.0.borrow_mut().1 = Some(data.to_vec());
    }
}

#[derive(Clone, Default)]
struct Port(Rc<RefCell<Option<StateReport>>>);

impl Radio for Port {
    fn poll(&mut self) -> Option<bulb_core::Command> {
        None
    }
    fn report(&mut self, s: &StateReport) {
        *self.0.borrow_mut() = Some(*s);
    }
}

type B = Bulb<P, Clk, Store, Port>;

struct Rig {
    net: Net,
    n: usize,
    short: u16,
    bulb: B,
    port: Port,
    store: Store,
    pwm: P,
    log: Vec<bulb_core::Command>,
}

impl Rig {
    fn new(kind: u8) -> Rig {
        let mut net = Net::new();
        net.identity = match kind {
            0 => IDENTITY,
            1 => IDENTITY_TW,
            _ => IDENTITY_COLOR,
        };
        let n = net.join_node();
        let short = net.nodes[n].short();
        let pwm = P { cold: kind >= 1, rgb: kind == 2 };
        let port = Port::default();
        let store = Store::default();
        let bulb = Bulb::boot(pwm, Clk(net.air.clone()), store.clone(), port.clone());
        let mut r = Rig { net, n, short, bulb, port, store, pwm, log: vec![] };
        r.run(100);
        r
    }

    fn poll_once(&mut self) -> bool {
        if let Some(l) = self.port.0.borrow_mut().take() {
            self.net.nodes[self.n].stack.set_light(&l);
        }
        match self.net.nodes[self.n].stack.poll() {
            Some(c) => {
                self.log.push(c);
                self.bulb.handle(c);
                true
            }
            None => false,
        }
    }

    fn step(&mut self) {
        self.net.air.borrow_mut().now += 1;
        self.net.coord.poll();
        while self.poll_once() {}
        self.bulb.tick();
    }

    fn run(&mut self, ms: u32) {
        for _ in 0..ms {
            self.step();
        }
    }

    fn read(&mut self, cluster: u16, ids: &[u16]) -> Vec<u8> {
        let p: Vec<u8> = ids.iter().flat_map(|i| i.to_le_bytes()).collect();
        let seq = self.net.coord.zcl(self.short, cluster, 0x00, 0x00, &p);
        self.run(50);
        let rsp = self.net.coord.last_zcl(self.short, cluster, 0x01).unwrap();
        assert_eq!(&rsp[..3], &[0x18, seq, 0x01]);
        rsp[3..].to_vec()
    }

    fn z2m(&mut self, kind: u8) -> Vec<Vec<u8>> {
        let mut v = vec![self.read(ON_OFF, &[0x0000]), self.read(LEVEL, &[0x0000])];
        if kind >= 1 {
            v.push(self.read(COLOR, &[0x0007, 0x0008]));
        }
        if kind == 2 {
            v.push(self.read(COLOR, &[0x0000, 0x0001, 0x0003, 0x0004]));
        }
        v
    }

    fn reboot(&mut self) -> StateReport {
        *self.port.0.borrow_mut() = None;
        let b = Bulb::boot(self.pwm, Clk(self.net.air.clone()), self.store.clone(), Port::default());
        b.state()
    }
}

#[derive(Clone, Debug)]
enum Op {
    Zcl(u16, u8, Vec<u8>),
    Write(u16, u16, u8, Vec<u8>),
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn byte(&mut self) -> u8 {
        self.next() as u8
    }
}

fn gen_op(r: &mut Rng, kind: u8) -> Op {
    let lv = 1 + r.below(254) as u8;
    let n = if kind == 0 { 14 } else if kind == 1 { 19 } else { 25 };
    match r.below(n) {
        0 => Op::Zcl(ON_OFF, 0x00, vec![]),
        1 => Op::Zcl(ON_OFF, 0x01, vec![]),
        2 | 3 => Op::Zcl(ON_OFF, 0x02, vec![]),
        4 => Op::Zcl(ON_OFF, 0x40, vec![0, 1]),
        5 => Op::Zcl(ON_OFF, 0x41, vec![]),
        6 => Op::Zcl(LEVEL, 0x00, vec![lv, 0, 0, 0, 0]),
        7 => Op::Zcl(LEVEL, 0x04, vec![if r.below(4) == 0 { 0 } else { lv }, 0, 0]),
        8 => Op::Zcl(LEVEL, if r.below(2) == 0 { 0x02 } else { 0x06 }, vec![r.below(2) as u8, r.byte(), 0, 0, 0, 0]),
        9 => Op::Zcl(SCENES, 0x04, vec![0, 0, r.below(3) as u8]),
        10 => Op::Zcl(SCENES, 0x05, vec![0, 0, r.below(3) as u8]),
        11 => Op::Zcl(ON_OFF, 0x42, vec![r.below(2) as u8, 0x10, 0x27, 0, 0]),
        12 => Op::Write(LEVEL, 0x000F, 0x18, vec![r.below(2) as u8]),
        13 => Op::Write(LEVEL, 0x0011, 0x20, vec![if r.below(2) == 0 { 0xFF } else { lv }]),
        14 | 15 => {
            let m = 153 + r.below(300) as u16;
            let m = m.to_le_bytes();
            Op::Zcl(COLOR, 0x0A, vec![m[0], m[1], 0, 0, 0, 0])
        }
        16 => Op::Zcl(COLOR, 0x4C, vec![1 + 2 * r.below(2) as u8, r.byte(), 0, 0, 0, 0, 0, 0, 0, 0, 0]),
        17 => Op::Write(COLOR, 0x000F, 0x18, vec![r.below(2) as u8]),
        18 => Op::Write(LEVEL, 0x000F, 0x18, vec![2 * r.below(2) as u8 | r.below(2) as u8]),
        19 => Op::Zcl(COLOR, 0x06, vec![r.below(255) as u8, r.below(255) as u8, 0, 0, 0, 0]),
        20 => Op::Zcl(COLOR, 0x00, vec![r.below(255) as u8, 0, 0, 0, 0, 0]),
        21 => Op::Zcl(COLOR, 0x03, vec![r.below(255) as u8, 0, 0, 0, 0]),
        22 => {
            let x = (r.next() as u16 % 0xF000).to_le_bytes();
            let y = (r.next() as u16 % 0xF000).to_le_bytes();
            Op::Zcl(COLOR, 0x07, vec![x[0], x[1], y[0], y[1], 0, 0, 0, 0])
        }
        23 => Op::Zcl(COLOR, 0x02, vec![1 + 2 * r.below(2) as u8, r.byte(), 0, 0, 0]),
        _ => Op::Zcl(COLOR, 0x05, vec![1 + 2 * r.below(2) as u8, r.byte(), 0, 0, 0]),
    }
}

fn send(rig: &mut Rig, op: &Op) {
    match op {
        Op::Zcl(c, cmd, p) => {
            rig.net.coord.zcl(rig.short, *c, 0x01, *cmd, p);
        }
        Op::Write(c, id, ty, v) => {
            let mut p = id.to_le_bytes().to_vec();
            p.push(*ty);
            p.extend_from_slice(v);
            rig.net.coord.zcl(rig.short, *c, 0x00, 0x02, &p);
        }
    }
}

type Seen = (bool, u8, u16, u8, u8, u8, u16, u16, Vec<Vec<u8>>, (bool, u8, u16));

fn observe(rig: &mut Rig, kind: u8) -> Seen {
    rig.run(200);
    let s = rig.bulb.state();
    let z = rig.z2m(kind);
    let p = rig.reboot();
    (s.on, s.level, s.color_temp, s.color_mode as u8, s.hue, s.sat, s.x, s.y, z, (p.on, p.level, p.color_temp))
}

fn prelude(rig: &mut Rig, r: &mut Rng, kind: u8) -> Vec<Op> {
    let ops: Vec<Op> = (0..r.below(4)).map(|_| gen_op(r, kind)).collect();
    for o in &ops {
        send(rig, o);
        rig.run(60);
    }
    ops
}

fn one(seed: u64, kind: u8) -> Option<String> {
    let mut r = Rng(seed);
    let len = 1 + r.below(7) as usize;
    let mut pre_rng = Rng(r.next() | 1);
    let ops: Vec<Op> = (0..len).map(|_| gen_op(&mut r, kind)).collect();
    let mut a = Rig::new(kind);
    let pre = prelude(&mut a, &mut Rng(pre_rng.0), kind);
    for o in &ops {
        send(&mut a, o);
        a.run(60);
    }
    let seq = observe(&mut a, kind);
    let mut b = Rig::new(kind);
    prelude(&mut b, &mut pre_rng, kind);
    let mut i = 0;
    let mut sched = vec![];
    while i < ops.len() {
        let k = 1 + r.below(8) as usize;
        for o in ops.iter().skip(i).take(k) {
            send(&mut b, o);
        }
        i += k;
        let polls = r.below(3);
        for _ in 0..polls {
            b.poll_once();
        }
        let t = r.below(3);
        sched.push((k, polls, t));
        for _ in 0..t {
            b.step();
        }
    }
    let burst = observe(&mut b, kind);
    (seq != burst).then(|| format!("seed {seed:#x} kind {kind}\npre {pre:?}\nops {ops:?}\nsched {sched:?}\nseq   {seq:?}\nburst {burst:?}\nseq cmds {:?}\nburst cmds {:?}", a.log, b.log))
}

fn many(kind: u8, count: u64, base: u64) {
    let mut fails = vec![];
    for k in 0..count {
        if let Some(f) = one(base.wrapping_add(k.wrapping_mul(0x9E37_79B9_7F4A_7C15)) | 1, kind) {
            fails.push(f);
        }
    }
    assert!(fails.is_empty(), "{} of {count} failed; first:\n{}", fails.len(), fails.iter().take(3).cloned().collect::<Vec<_>>().join("\n\n"));
}

#[test]
fn dimm_bursts_match_one_by_one() {
    many(0, 300, 0x1111);
}

#[test]
fn tw_bursts_match_one_by_one() {
    many(1, 300, 0x2222);
}

#[test]
fn colour_bursts_match_one_by_one() {
    many(2, 300, 0x3333);
}

#[test]
fn eight_brightness_commands_heard_together_end_at_the_last() {
    let mut rig = Rig::new(0);
    for l in [10u8, 20, 30, 40, 50, 60, 70, 80] {
        rig.net.coord.zcl(rig.short, LEVEL, 0x01, 0x00, &[l, 0, 0, 0, 0]);
    }
    rig.run(200);
    let got = rig.bulb.state().level;
    let z2m = rig.read(LEVEL, &[0x0000]);
    assert_eq!((got, z2m[4]), (80, 80), "commands {:?}", rig.log);
}

#[test]
fn a_saturation_heard_with_the_xy_before_it_keeps_the_xy_hue() {
    let mut rig = Rig::new(2);
    rig.net.coord.zcl(rig.short, COLOR, 0x01, 0x07, &[83, 9, 97, 164, 0, 0]);
    rig.net.coord.zcl(rig.short, COLOR, 0x01, 0x03, &[137, 0, 0]);
    rig.run(200);
    let mut one = Rig::new(2);
    one.net.coord.zcl(one.short, COLOR, 0x01, 0x07, &[83, 9, 97, 164, 0, 0]);
    one.run(60);
    one.net.coord.zcl(one.short, COLOR, 0x01, 0x03, &[137, 0, 0]);
    one.run(200);
    assert_eq!(rig.bulb.state().hue, one.bulb.state().hue, "together {:?} one by one {:?}", rig.log, one.log);
}

#[test]
fn off_by_level_then_store_scene_heard_together_keeps_the_brightness() {
    let mut rig = Rig::new(0);
    rig.net.coord.zcl(rig.short, LEVEL, 0x01, 0x04, &[200, 0, 0]);
    rig.run(60);
    rig.net.coord.zcl(rig.short, LEVEL, 0x01, 0x04, &[0, 0, 0]);
    rig.net.coord.zcl(rig.short, SCENES, 0x01, 0x04, &[0, 0, 1]);
    rig.run(60);
    rig.net.coord.zcl(rig.short, SCENES, 0x01, 0x05, &[0, 0, 1]);
    rig.run(60);
    rig.net.coord.zcl(rig.short, ON_OFF, 0x01, 0x01, &[]);
    rig.run(60);
    assert_eq!(rig.bulb.state().level, 200, "commands {:?}", rig.log);
}

#[test]
fn current_saturation_after_a_move_to_colour_stays_in_range() {
    let mut rig = Rig::new(2);
    let mut bad = vec![];
    for k in 0..200u32 {
        let x = (k * 7919 % 0xF000) as u16;
        let y = (k * 104729 % 0xF000) as u16;
        if x == 0 && y == 0 {
            continue;
        }
        let (a, b) = (x.to_le_bytes(), y.to_le_bytes());
        rig.net.coord.zcl(rig.short, COLOR, 0x01, 0x07, &[a[0], a[1], b[0], b[1], 0, 0]);
        rig.run(60);
        let sat = rig.read(COLOR, &[0x0001])[4];
        if sat > 254 {
            bad.push((x, y, sat));
        }
    }
    assert!(bad.is_empty(), "{bad:?}");
}

#[test]
fn repeated_startup_settings_in_one_write_end_at_the_last() {
    let mut rig = Rig::new(1);
    let (mut level, mut power) = (vec![], vec![]);
    for v in 1..=9u8 {
        level.extend_from_slice(&[0x00, 0x40, 0x20, v * 10]);
        power.extend_from_slice(&[0x03, 0x40, 0x30, v % 3]);
    }
    rig.net.coord.zcl(rig.short, LEVEL, 0x00, 0x02, &level);
    rig.net.coord.zcl(rig.short, ON_OFF, 0x00, 0x02, &power);
    rig.run(200);
    let s = rig.bulb.state();
    assert_eq!((s.startup_level, s.power_on_behavior.to_u8()), (90, 0), "commands {:?}", rig.log);
}

const FULL_XY: [u8; 6] = [0xD7, 0xA3, 0x7B, 0x54, 0, 0];

fn after_full_saturation(cmd: u8, payload: &[u8]) -> bulb_core::Command {
    let mut rig = Rig::new(2);
    rig.net.coord.zcl(rig.short, COLOR, 0x01, 0x07, &FULL_XY);
    rig.run(60);
    assert_eq!(rig.bulb.state().sat, 255);
    rig.net.coord.zcl(rig.short, COLOR, 0x01, cmd, payload);
    rig.run(60);
    *rig.log.last().unwrap()
}

#[test]
fn saturation_steps_and_moves_start_from_the_highest_zcl_value() {
    use bulb_core::command::HueDirection::Shortest;
    let hs = |sat, transition_ms| bulb_core::Command::MoveToHueSaturation { hue: 0, sat, direction: Shortest, transition_ms };
    assert_eq!(after_full_saturation(0x05, &[3, 4, 0]), hs(250, 0));
    assert_eq!(after_full_saturation(0x04, &[3, 2]), hs(0, 127_000));
    assert_eq!(after_full_saturation(0x04, &[1, 2]), hs(254, 0));
}

#[test]
fn a_viewed_scene_and_a_report_keep_the_saturation_in_range() {
    let mut rig = Rig::new(2);
    let ext = rig.net.air.borrow().ports[rig.net.nodes[rig.n].id].ext;
    let mut bind = ext.to_le_bytes().to_vec();
    bind.push(1);
    bind.extend_from_slice(&COLOR.to_le_bytes());
    bind.push(3);
    bind.extend_from_slice(&COORD_EXT.to_le_bytes());
    bind.push(1);
    rig.net.coord.zdo(rig.short, 0x0021, &bind);
    rig.net.coord.zcl(rig.short, COLOR, 0x01, 0x06, &[10, 100, 0, 0]);
    rig.run(2_000);
    rig.net.coord.zcl(rig.short, COLOR, 0x00, 0x06, &[0x00, 0x01, 0x00, 0x20, 1, 0, 0x10, 0x0E, 1]);
    rig.run(60);
    rig.net.coord.zcl(rig.short, COLOR, 0x01, 0x07, &FULL_XY);
    rig.run(3_000);
    let reported: Vec<u8> = rig.net.coord.zcl_rx.iter().filter(|e| e.1 == COLOR && e.2[2] == 0x0A && e.2[3..6] == [0x01, 0x00, 0x20]).map(|e| e.2[6]).collect();
    assert_eq!(reported.last(), Some(&254), "{reported:?}");
    rig.net.coord.zcl(rig.short, SCENES, 0x01, 0x04, &[0, 0, 1]);
    rig.run(60);
    rig.net.coord.zcl(rig.short, SCENES, 0x01, 0x01, &[0, 0, 1]);
    rig.run(60);
    let view = rig.net.coord.last_zcl(rig.short, SCENES, 0x01).unwrap();
    let at = view.windows(3).position(|w| w == [0x00, 0x03, 14]).unwrap();
    assert_eq!(view[at + 9], 254, "{view:?}");
}
