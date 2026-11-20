use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;

use bulb_core::command::{HueDirection, StateReport};
use bulb_core::hal::{Clock, ConfigStore, Pwm, Radio};
use bulb_core::{Bulb, Command};

use crate::sim::*;

const IDENTIFY: u16 = 0x0003;
const GROUPS: u16 = 0x0004;
const SCENES: u16 = 0x0005;
const ON_OFF: u16 = 0x0006;
const LEVEL: u16 = 0x0008;
const COLOR: u16 = 0x0300;
const GROUP: u16 = 0x0042;

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
    fn poll(&mut self) -> Option<Command> {
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
    ble: VecDeque<Command>,
    log: Vec<(u32, bool, Command)>,
    attrs: Vec<(u16, u16)>,
}

fn identity(kind: u8) -> zigbee::stack::Identity {
    match kind {
        0 => IDENTITY,
        1 => IDENTITY_TW,
        _ => IDENTITY_COLOR,
    }
}

impl Rig {
    fn new(kind: u8) -> Rig {
        let mut net = Net::new();
        net.identity = identity(kind);
        let n = net.join_node();
        let short = net.nodes[n].short();
        let pwm = P { cold: kind >= 1, rgb: kind == 2 };
        let port = Port::default();
        let store = Store::default();
        let bulb = Bulb::boot(pwm, Clk(net.air.clone()), store.clone(), port.clone());
        let mut r = Rig { net, n, short, bulb, port, store, pwm, ble: VecDeque::new(), log: vec![], attrs: vec![] };
        r.run(100);
        r.setup(kind);
        r
    }

    fn setup(&mut self, kind: u8) {
        self.net.coord.zcl(self.short, GROUPS, 0x01, 0x00, &[GROUP as u8, (GROUP >> 8) as u8, 0]);
        self.run(60);
        let ext = self.net.air.borrow().ports[self.net.nodes[self.n].id].ext;
        let mut clusters = vec![ON_OFF, LEVEL];
        if kind >= 1 {
            clusters.push(COLOR);
        }
        for c in clusters {
            let mut bind = ext.to_le_bytes().to_vec();
            bind.push(1);
            bind.extend_from_slice(&c.to_le_bytes());
            bind.push(3);
            bind.extend_from_slice(&COORD_EXT.to_le_bytes());
            bind.push(1);
            self.net.coord.zdo(self.short, 0x0021, &bind);
            self.run(60);
        }
        let mut want: Vec<(u16, u16, u8)> = vec![(ON_OFF, 0x0000, 0x10), (LEVEL, 0x0000, 0x20)];
        if kind >= 1 {
            want.push((COLOR, 0x0007, 0x21));
            want.push((COLOR, 0x0008, 0x30));
        }
        if kind == 2 {
            want.extend_from_slice(&[(COLOR, 0x0000, 0x20), (COLOR, 0x0001, 0x20), (COLOR, 0x0003, 0x21), (COLOR, 0x0004, 0x21)]);
        }
        for (c, id, ty) in want {
            let mut p = vec![0x00];
            p.extend_from_slice(&id.to_le_bytes());
            p.push(ty);
            p.extend_from_slice(&[1, 0, 10, 0]);
            match ty {
                0x20 => p.push(1),
                0x21 => p.extend_from_slice(&[1, 0]),
                _ => {}
            }
            self.net.coord.zcl(self.short, c, 0x00, 0x06, &p);
            self.run(60);
            let rsp = self.net.coord.last_zcl(self.short, c, 0x07).unwrap();
            if rsp[3] == 0 {
                self.attrs.push((c, id));
            }
        }
    }

    fn poll_once(&mut self) -> bool {
        if let Some(l) = self.port.0.borrow_mut().take() {
            self.net.nodes[self.n].stack.set_light(&l);
        }
        let now = self.net.air.borrow().now;
        if let Some(c) = self.ble.pop_front() {
            self.log.push((now, true, c));
            self.bulb.handle(c);
            return true;
        }
        match self.net.nodes[self.n].stack.poll() {
            Some(c) => {
                self.log.push((now, false, c));
                self.bulb.handle(c);
                true
            }
            None => false,
        }
    }

    fn drain(&mut self) {
        for _ in 0..4 {
            while self.poll_once() {}
        }
    }

    fn step_by(&mut self, ms: u32) {
        self.net.air.borrow_mut().now += ms;
        self.net.coord.poll();
        self.drain();
        self.bulb.tick();
    }

    fn run(&mut self, ms: u32) {
        for _ in 0..ms {
            self.step_by(1);
        }
    }

    fn settle(&mut self, ms: u32) {
        for _ in 0..ms / 20 {
            self.step_by(20);
        }
        self.run(100);
    }

    fn read_attrs(&mut self, cluster: u16, ids: &[u16]) -> BTreeMap<(u16, u16), Vec<u8>> {
        let p: Vec<u8> = ids.iter().flat_map(|i| i.to_le_bytes()).collect();
        let seq = self.net.coord.zcl(self.short, cluster, 0x00, 0x00, &p);
        self.run(50);
        let rsp = self.net.coord.last_zcl(self.short, cluster, 0x01).unwrap();
        assert_eq!(&rsp[..3], &[0x18, seq, 0x01]);
        let mut out = BTreeMap::new();
        let mut i = 3;
        while i + 3 <= rsp.len() {
            let id = u16::from_le_bytes([rsp[i], rsp[i + 1]]);
            if rsp[i + 2] != 0 {
                i += 3;
                continue;
            }
            let ty = rsp[i + 3];
            let n = size(ty);
            out.insert((cluster, id), rsp[i + 4..i + 4 + n].to_vec());
            i += 4 + n;
        }
        out
    }

    fn z2m(&mut self, kind: u8) -> BTreeMap<(u16, u16), Vec<u8>> {
        let mut v = self.read_attrs(ON_OFF, &[0x0000, 0x4003]);
        v.extend(self.read_attrs(LEVEL, &[0x0000, 0x4000]));
        if kind >= 1 {
            v.extend(self.read_attrs(COLOR, &[0x0007, 0x0008, 0x4010]));
        }
        if kind == 2 {
            v.extend(self.read_attrs(COLOR, &[0x0000, 0x0001, 0x0003, 0x0004]));
        }
        v
    }

    fn last_reports(&self) -> BTreeMap<(u16, u16), Vec<u8>> {
        let mut out = BTreeMap::new();
        for (src, cluster, f) in &self.net.coord.zcl_rx {
            if *src != self.short || f.get(2) != Some(&0x0A) {
                continue;
            }
            let mut i = 3;
            while i + 3 <= f.len() {
                let id = u16::from_le_bytes([f[i], f[i + 1]]);
                let n = size(f[i + 2]);
                out.insert((*cluster, id), f[i + 3..i + 3 + n].to_vec());
                i += 3 + n;
            }
        }
        out
    }

    fn responses(&self) -> Vec<Vec<u8>> {
        let mut v: Vec<Vec<u8>> = self
            .net
            .coord
            .zcl_rx
            .iter()
            .filter(|(s, _, f)| *s == self.short && f.get(2) != Some(&0x0A))
            .map(|(_, c, f)| {
                let mut x = c.to_le_bytes().to_vec();
                x.extend_from_slice(f);
                x
            })
            .collect();
        v.sort();
        v
    }

    fn power_cut(&mut self) {
        *self.port.0.borrow_mut() = None;
        self.ble.clear();
        self.bulb = Bulb::boot(self.pwm, Clk(self.net.air.clone()), self.store.clone(), self.port.clone());
        self.net.power_cycle(self.n);
        self.run(200);
    }
}

fn size(ty: u8) -> usize {
    match ty {
        0x10 | 0x18 | 0x20 | 0x30 => 1,
        0x21 | 0x19 => 2,
        _ => panic!("type {ty:#x}"),
    }
}

fn expected(s: &StateReport, kind: u8) -> BTreeMap<(u16, u16), Vec<u8>> {
    let mut m = BTreeMap::new();
    m.insert((ON_OFF, 0x0000), vec![s.on as u8]);
    m.insert((ON_OFF, 0x4003), vec![s.power_on_behavior.to_u8()]);
    m.insert((LEVEL, 0x0000), vec![s.level]);
    m.insert((LEVEL, 0x4000), vec![s.startup_level]);
    if kind >= 1 {
        m.insert((COLOR, 0x0007), s.color_temp.to_le_bytes().to_vec());
        m.insert((COLOR, 0x0008), vec![s.color_mode as u8]);
        m.insert((COLOR, 0x4010), s.startup_color_temp.to_le_bytes().to_vec());
    }
    if kind == 2 {
        if !s.hue_moving {
            m.insert((COLOR, 0x0000), vec![s.hue]);
        }
        m.insert((COLOR, 0x0001), vec![s.sat.min(254)]);
        m.insert((COLOR, 0x0003), s.x.to_le_bytes().to_vec());
        m.insert((COLOR, 0x0004), s.y.to_le_bytes().to_vec());
    }
    m
}

#[derive(Clone, Debug)]
enum Ev {
    Zcl { group: bool, cluster: u16, cmd: u8, p: Vec<u8> },
    Write(u16, u16, u8, Vec<u8>),
    Read(u16, Vec<u16>),
    Ble(Command),
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
    fn pick<T: Copy>(&mut self, v: &[T]) -> T {
        v[self.below(v.len() as u64) as usize]
    }
    fn tenths(&mut self) -> [u8; 2] {
        let t: u16 = self.pick(&[0, 0, 1, 3, 7, 15, 30, 0xFFFF]);
        t.to_le_bytes()
    }
}

fn opts(r: &mut Rng, p: &mut Vec<u8>) {
    if r.below(3) == 0 {
        p.push(r.below(2) as u8);
        p.push(r.below(2) as u8);
    }
}

fn gen_ev(r: &mut Rng, kind: u8) -> Ev {
    loop {
        let e = gen_ev_any(r, kind);
        if std::env::var("LIGHT_MODEL_NO_STOP").is_err() || !is_stop(&e) {
            return e;
        }
    }
}

fn is_stop(e: &Ev) -> bool {
    match e {
        Ev::Zcl { cluster: LEVEL, cmd: 0x03 | 0x07, .. } | Ev::Zcl { cluster: COLOR, cmd: 0x47, .. } => true,
        Ev::Zcl { cluster: COLOR, cmd: 0x4B | 0x01 | 0x04, p, .. } => p[0] == 0,
        Ev::Ble(Command::StopColorTransition) | Ev::Ble(Command::MoveHue { rate: 0, .. }) => true,
        Ev::Ble(Command::MoveToLevel { transition_ms, .. }) | Ev::Ble(Command::MoveToColorTemperature { transition_ms, .. }) | Ev::Ble(Command::MoveToHueSaturation { transition_ms, .. }) => *transition_ms != 0,
        _ => false,
    }
}

fn gen_ev_any(r: &mut Rng, kind: u8) -> Ev {
    let lv = 1 + r.below(254) as u8;
    let zcl = |group: bool, cluster: u16, cmd: u8, p: Vec<u8>| Ev::Zcl { group, cluster, cmd, p };
    let g = r.below(4) == 0;
    let classes = if kind == 0 { 9 } else if kind == 1 { 11 } else { 13 };
    match r.below(classes) {
        0 => {
            let cmd = r.pick(&[0x00, 0x01, 0x02, 0x02, 0x40, 0x41, 0x42]);
            let p = match cmd {
                0x40 => vec![r.below(2) as u8, r.below(3) as u8],
                0x42 => {
                    let on = (r.below(40) as u16).to_le_bytes();
                    let off = (r.below(40) as u16).to_le_bytes();
                    vec![r.below(2) as u8, on[0], on[1], off[0], off[1]]
                }
                _ => vec![],
            };
            zcl(g, ON_OFF, cmd, p)
        }
        1 | 2 => {
            let cmd = r.below(8) as u8;
            let t = r.tenths();
            let mut p = match cmd & 3 {
                0 => vec![if r.below(4) == 0 { 0 } else { lv }, t[0], t[1]],
                1 => vec![r.pick(&[0, 1]), r.pick(&[10, 40, 100, 255, 0xFF])],
                2 => vec![r.below(2) as u8, r.byte(), t[0], t[1]],
                _ => vec![],
            };
            opts(r, &mut p);
            zcl(g, LEVEL, cmd, p)
        }
        3 => {
            let id = 1 + r.below(3) as u8;
            let grp: u16 = if r.below(2) == 0 { 0 } else { GROUP };
            let gb = grp.to_le_bytes();
            match r.below(5) {
                0 | 1 => zcl(g, SCENES, 0x05, if r.below(2) == 0 { vec![gb[0], gb[1], id] } else { let t = r.tenths(); vec![gb[0], gb[1], id, t[0], t[1]] }),
                2 => zcl(false, SCENES, 0x04, vec![gb[0], gb[1], id]),
                3 => {
                    let enhanced = r.below(2) == 0;
                    let t = (r.below(25) as u16).to_le_bytes();
                    let mut p = vec![gb[0], gb[1], id, t[0], t[1], 0];
                    if r.below(4) != 0 {
                        p.extend_from_slice(&[0x06, 0x00, 1, r.below(2) as u8]);
                    }
                    if r.below(4) != 0 {
                        p.extend_from_slice(&[0x08, 0x00, 1, lv]);
                    }
                    if kind >= 1 && r.below(3) != 0 {
                        let x = (r.next() as u16 % 0xF000).to_le_bytes();
                        let y = (r.next() as u16 % 0xF000).to_le_bytes();
                        let ct = if r.below(2) == 0 { 0 } else { 153 + r.below(300) as u16 }.to_le_bytes();
                        let mut c = vec![x[0], x[1], y[0], y[1], 0, r.below(255) as u8, r.below(255) as u8, 0, 0, 0, 0, ct[0], ct[1]];
                        if r.below(2) == 0 {
                            c.push(r.below(3) as u8);
                        }
                        p.extend_from_slice(&[0x00, 0x03, c.len() as u8]);
                        p.extend_from_slice(&c);
                    }
                    zcl(false, SCENES, if enhanced { 0x40 } else { 0x00 }, p)
                }
                _ => zcl(false, SCENES, 0x02, vec![gb[0], gb[1], id]),
            }
        }
        4 => {
            if r.below(2) == 0 {
                zcl(g, IDENTIFY, 0x00, vec![r.below(3) as u8, 0])
            } else {
                zcl(g, IDENTIFY, 0x40, vec![r.pick(&[0, 1, 2, 0x0B, 0xFE, 0xFF]), 0])
            }
        }
        5 => match r.below(6) {
            0 => Ev::Write(ON_OFF, 0x4003, 0x30, vec![r.pick(&[0, 1, 2, 0xFF])]),
            1 => Ev::Write(ON_OFF, 0x4001, 0x21, (r.below(30) as u16).to_le_bytes().to_vec()),
            2 => Ev::Write(ON_OFF, 0x4002, 0x21, (r.below(30) as u16).to_le_bytes().to_vec()),
            3 => Ev::Write(LEVEL, 0x4000, 0x20, vec![r.pick(&[0, 1, lv, 0xFF])]),
            4 => Ev::Write(LEVEL, 0x000F, 0x18, vec![r.below(4) as u8]),
            _ => Ev::Write(LEVEL, 0x0011, 0x20, vec![r.pick(&[0xFF, lv])]),
        },
        6 => {
            let c = r.pick(&[ON_OFF, LEVEL, COLOR]);
            let ids = match c {
                ON_OFF => vec![0x0000, 0x4001, 0x4002, 0x4003],
                LEVEL => vec![0x0000, 0x4000, 0x0011],
                _ if kind == 0 => vec![0x0000],
                _ => vec![0x0000, 0x0001, 0x0003, 0x0004, 0x0007, 0x0008, 0x4010],
            };
            if c == COLOR && kind == 0 {
                Ev::Read(ON_OFF, vec![0x0000])
            } else {
                Ev::Read(c, ids)
            }
        }
        7 | 8 => Ev::Ble(match r.below(if kind == 0 { 6 } else if kind == 1 { 8 } else { 11 }) {
            0 => Command::On,
            1 => Command::Off,
            2 => Command::Toggle,
            3 => Command::MoveToLevel { level: lv, transition_ms: r.pick(&[0, 0, 300, 2000]) },
            4 => Command::Candle(r.below(2) == 0),
            5 => Command::SetLevelWhileOff(lv),
            6 | 7 => Command::MoveToColorTemperature { mireds: 153 + r.below(300) as u16, transition_ms: r.pick(&[0, 500]) },
            8 => Command::MoveToHueSaturation { hue: r.below(255) as u8, sat: r.byte(), direction: HueDirection::Shortest, transition_ms: r.pick(&[0, 500]) },
            9 => Command::MoveHue { up: r.below(2) == 0, rate: r.pick(&[0, 20, 100]) },
            _ => Command::StopColorTransition,
        }),
        9 | 10 => {
            let t = r.tenths();
            let m = (153 + r.below(300) as u16).to_le_bytes();
            let (cmd, mut p) = match r.below(5) {
                0 | 1 => (0x0A, vec![m[0], m[1], t[0], t[1]]),
                2 => (0x4B, vec![r.pick(&[0, 1, 3]), r.pick(&[10, 50, 200]), 0, 0, 0, 0, 0]),
                3 => (0x4C, vec![r.pick(&[1, 3]), r.byte(), 0, t[0], t[1], 0, 0, 0, 0]),
                _ => (0x47, vec![]),
            };
            opts(r, &mut p);
            zcl(g, COLOR, cmd, p)
        }
        _ => {
            let t = r.tenths();
            let (cmd, mut p) = match r.below(10) {
                0 => (0x00, vec![r.below(255) as u8, r.below(4) as u8, t[0], t[1]]),
                1 => (0x01, vec![r.pick(&[0, 1, 3]), r.pick(&[10, 50, 200])]),
                2 => (0x03, vec![r.below(255) as u8, t[0], t[1]]),
                3 => (0x04, vec![r.pick(&[0, 1, 3]), r.pick(&[10, 50, 200])]),
                4 => (0x05, vec![r.pick(&[1, 3]), r.byte(), t[0]]),
                5 => (0x06, vec![r.below(255) as u8, r.below(255) as u8, t[0], t[1]]),
                6 | 7 => {
                    let x = (r.next() as u16 % 0xF000).to_le_bytes();
                    let y = (r.next() as u16 % 0xF000).to_le_bytes();
                    (0x07, vec![x[0], x[1], y[0], y[1], t[0], t[1]])
                }
                8 => (0x02, vec![r.pick(&[1, 3]), r.byte(), t[0]]),
                _ => (0x09, vec![r.byte(), r.byte(), r.byte(), r.byte(), t[0], t[1]]),
            };
            opts(r, &mut p);
            zcl(g, COLOR, cmd, p)
        }
    }
}

fn send(rig: &mut Rig, e: &Ev) {
    match e {
        Ev::Zcl { group: true, cluster, cmd, p } => rig.net.coord.zcl_group(GROUP, *cluster, *cmd, p),
        Ev::Zcl { group: false, cluster, cmd, p } => {
            rig.net.coord.zcl(rig.short, *cluster, 0x01, *cmd, p);
        }
        Ev::Write(c, id, ty, v) => {
            let mut p = id.to_le_bytes().to_vec();
            p.push(*ty);
            p.extend_from_slice(v);
            rig.net.coord.zcl(rig.short, *c, 0x00, 0x02, &p);
        }
        Ev::Read(c, ids) => {
            let p: Vec<u8> = ids.iter().flat_map(|i| i.to_le_bytes()).collect();
            rig.net.coord.zcl(rig.short, *c, 0x00, 0x00, &p);
        }
        Ev::Ble(c) => rig.ble.push_back(*c),
    }
}

struct Chunk {
    evs: Vec<Ev>,
    polls: u64,
    wait: u32,
    cut: bool,
}

fn consistent(rig: &mut Rig, kind: u8, what: &str) -> Option<String> {
    let s = rig.bulb.state();
    let want = expected(&s, kind);
    let got = rig.z2m(kind);
    let mut bad = vec![];
    for (k, v) in &want {
        if got.get(k) != Some(v) {
            bad.push(format!("{what} attr {:04x}/{:04x}: zigbee {:?} bulb {:?}", k.0, k.1, got.get(k), v));
        }
    }
    let reps = rig.last_reports();
    for k in rig.attrs.clone() {
        if let (Some(w), r) = (want.get(&k), reps.get(&k)) {
            if r != Some(w) {
                bad.push(format!("{what} report {:04x}/{:04x}: last {:?} bulb {:?}", k.0, k.1, r, w));
            }
        }
    }
    (!bad.is_empty()).then(|| bad.join("\n"))
}

fn one(seed: u64, kind: u8) -> Option<String> {
    let mut r = Rng(seed);
    let nchunks = 1 + r.below(6);
    let chunks: Vec<Chunk> = (0..nchunks)
        .map(|_| {
            let n = 1 + r.below(7) as usize;
            let mut evs: Vec<Ev> = (0..n).map(|_| gen_ev(&mut r, kind)).collect();
            evs.sort_by_key(|e| !matches!(e, Ev::Ble(_)));
            Chunk { evs, polls: r.below(3), wait: r.pick(&[0, 0, 1, 7, 60, 250, 900, 2500]), cut: r.below(12) == 0 }
        })
        .collect();
    let mut a = Rig::new(kind);
    let mut b = Rig::new(kind);
    let mut problems = vec![];
    for (ci, c) in chunks.iter().enumerate() {
        for e in &c.evs {
            send(&mut a, e);
            a.drain();
        }
        if c.evs.iter().any(|e| matches!(e, Ev::Ble(_))) {
            b.drain();
        }
        for e in &c.evs {
            send(&mut b, e);
        }
        for _ in 0..c.polls {
            b.poll_once();
        }
        b.drain();
        for _ in 0..c.wait.max(1) {
            if c.wait == 0 {
                break;
            }
            a.step_by(1);
            b.step_by(1);
        }
        if c.wait > 0 && a.bulb.state() != b.bulb.state() {
            problems.push(format!("chunk {ci}: state one-by-one {:?}\n  together {:?}", a.bulb.state(), b.bulb.state()));
            break;
        }
        if c.cut {
            a.drain();
            b.drain();
            a.power_cut();
            b.power_cut();
        }
    }
    if problems.is_empty() {
        a.settle(40_000);
        b.settle(40_000);
        if a.bulb.state() != b.bulb.state() {
            problems.push(format!("settled: one-by-one {:?}\n  together {:?}", a.bulb.state(), b.bulb.state()));
        }
        if a.responses() != b.responses() {
            let (ra, rb) = (a.responses(), b.responses());
            let d: Vec<_> = ra.iter().zip(rb.iter()).filter(|(x, y)| x != y).take(2).collect();
            problems.push(format!("responses differ: {d:?} lens {} {}", ra.len(), rb.len()));
        }
        problems.extend(consistent(&mut a, kind, "one-by-one"));
        problems.extend(consistent(&mut b, kind, "together"));
        a.net.coord.zcl_rx.clear();
        a.power_cut();
        a.settle(25_000);
        problems.extend(consistent(&mut a, kind, "after power cut"));
    }
    (!problems.is_empty()).then(|| {
        let evs: Vec<_> = chunks.iter().map(|c| (c.evs.clone(), c.polls, c.wait, c.cut)).collect();
        format!("seed {seed:#x} kind {kind}\n{}\nchunks {evs:?}\ncmds A {:?}\ncmds B {:?}", problems.join("\n"), a.log, b.log)
    })
}

fn count() -> u64 {
    std::env::var("LIGHT_MODEL_N").ok().and_then(|v| v.parse().ok()).unwrap_or(40)
}

fn many(kind: u8, base: u64) {
    if let Some(seed) = std::env::var("LIGHT_MODEL_SEED").ok().and_then(|v| u64::from_str_radix(v.trim_start_matches("0x"), 16).ok()) {
        if std::env::var("LIGHT_MODEL_KIND").ok().and_then(|v| v.parse::<u8>().ok()) == Some(kind) {
            if let Some(f) = one(seed, kind) {
                panic!("{f}");
            }
        }
        return;
    }
    let n = count();
    let mut fails = vec![];
    for k in 0..n {
        if let Some(f) = one(base.wrapping_add(k.wrapping_mul(0x9E37_79B9_7F4A_7C15)) | 1, kind) {
            if std::env::var("LIGHT_MODEL_LIST").is_ok() {
                let mut l = f.lines();
                let _ = l.next();
                eprintln!("FAIL {f}\nEND");
            }
            fails.push(f);
        }
    }
    assert!(fails.is_empty(), "{} of {n} failed; first:\n{}", fails.len(), fails.iter().take(3).cloned().collect::<Vec<_>>().join("\n\n"));
}

#[test]
fn model_dimm() {
    many(0, 0xA1A1);
}

#[test]
fn model_tw() {
    many(1, 0xB2B2);
}

#[test]
fn model_colour() {
    many(2, 0xC3C3);
}

fn stop_then_step(together: bool) -> u8 {
    let mut rig = Rig::new(0);
    rig.net.coord.zcl(rig.short, LEVEL, 0x01, 0x00, &[20, 30, 0]);
    rig.run(1500);
    rig.net.coord.zcl(rig.short, ON_OFF, 0x00, 0x02, &[0x03, 0x40, 0x30, 1]);
    rig.net.coord.zcl(rig.short, LEVEL, 0x01, 0x03, &[]);
    if together {
        rig.poll_once();
    } else {
        rig.drain();
    }
    rig.net.coord.zcl(rig.short, LEVEL, 0x01, 0x02, &[0, 10, 0, 0]);
    rig.run(100);
    rig.bulb.state().level
}

#[test]
fn a_step_heard_while_a_stop_waits_starts_from_the_stopped_level() {
    let (together, one_by_one) = (stop_then_step(true), stop_then_step(false));
    assert!(together.abs_diff(one_by_one) <= 1, "together {together} one by one {one_by_one}");
}

fn ct_stop_then_step(together: bool) -> u16 {
    let mut rig = Rig::new(1);
    rig.net.coord.zcl(rig.short, COLOR, 0x01, 0x0A, &[153, 0, 0, 0]);
    rig.run(100);
    rig.net.coord.zcl(rig.short, COLOR, 0x01, 0x0A, &[0xC6, 0x01, 30, 0]);
    rig.run(1500);
    rig.net.coord.zcl(rig.short, ON_OFF, 0x00, 0x02, &[0x03, 0x40, 0x30, 1]);
    rig.net.coord.zcl(rig.short, COLOR, 0x01, 0x47, &[]);
    if together {
        rig.poll_once();
    } else {
        rig.drain();
    }
    rig.net.coord.zcl(rig.short, COLOR, 0x01, 0x4C, &[1, 10, 0, 0, 0, 0, 0, 0, 0]);
    rig.run(100);
    rig.bulb.state().color_temp
}

#[test]
fn a_colour_step_heard_while_a_stop_waits_starts_from_the_stopped_temperature() {
    assert_eq!(ct_stop_then_step(true), ct_stop_then_step(false));
}

fn ct_step_during_fade(with_on: bool) -> u16 {
    let mut rig = Rig::new(1);
    rig.net.coord.zcl(rig.short, COLOR, 0x01, 0x0A, &[153, 0, 0, 0]);
    rig.run(100);
    rig.net.coord.zcl(rig.short, COLOR, 0x01, 0x0A, &[0xC6, 0x01, 30, 0]);
    rig.run(1500);
    if with_on {
        rig.net.coord.zcl(rig.short, ON_OFF, 0x01, 0x01, &[]);
    }
    rig.run(10);
    rig.net.coord.zcl(rig.short, COLOR, 0x01, 0x4C, &[3, 10, 0, 0, 0, 0, 0, 0, 0]);
    rig.run(100);
    rig.bulb.state().color_temp
}

#[test]
fn an_on_for_a_light_that_is_on_keeps_the_colour_fade_a_step_counts_from() {
    assert_eq!(ct_step_during_fade(true), ct_step_during_fade(false));
}

fn read_now(rig: &mut Rig, cluster: u16, ids: &[u16]) -> Vec<u8> {
    let p: Vec<u8> = ids.iter().flat_map(|i| i.to_le_bytes()).collect();
    let seq = rig.net.coord.zcl(rig.short, cluster, 0x00, 0x00, &p);
    rig.poll_once();
    rig.run(50);
    let rsp = rig.net.coord.last_zcl(rig.short, cluster, 0x01).unwrap();
    assert_eq!(&rsp[..3], &[0x18, seq, 0x01]);
    rsp[3..].to_vec()
}

#[test]
fn a_temperature_heard_after_xy_reads_x_and_y_as_0_like_the_bulb() {
    let mut rig = Rig::new(2);
    rig.net.coord.zcl(rig.short, COLOR, 0x01, 0x07, &[0x00, 0x60, 0x00, 0x50, 0, 0]);
    rig.run(100);
    rig.net.coord.zcl(rig.short, COLOR, 0x01, 0x0A, &[0x2C, 0x01, 0, 0]);
    let read = read_now(&mut rig, COLOR, &[0x0003, 0x0004]);
    assert_eq!(read, [0x03, 0x00, 0x00, 0x21, 0, 0, 0x04, 0x00, 0x00, 0x21, 0, 0]);
    let s = rig.bulb.state();
    assert_eq!((s.x, s.y), (0, 0));
}

fn hue_move_during_saturation_fade(together: bool) -> u8 {
    let mut rig = Rig::new(2);
    rig.net.coord.zcl(rig.short, COLOR, 0x01, 0x06, &[163, 26, 0, 0]);
    rig.run(100);
    rig.net.coord.zcl(rig.short, COLOR, 0x01, 0x06, &[163, 233, 7, 0]);
    if !together {
        rig.drain();
    }
    rig.net.coord.zcl(rig.short, COLOR, 0x01, 0x01, &[3, 200]);
    if !together {
        rig.drain();
    }
    rig.net.coord.zcl(rig.short, COLOR, 0x01, 0x00, &[49, 3, 0, 0]);
    rig.drain();
    rig.run(1000);
    rig.bulb.state().sat
}

#[test]
fn a_hue_move_keeps_the_saturation_the_fade_has_reached() {
    assert_eq!((hue_move_during_saturation_fade(true), hue_move_during_saturation_fade(false)), (26, 26));
}

fn stop_right_after_a_fade_from_off(together: bool) -> (bool, u8) {
    let mut rig = Rig::new(0);
    rig.net.coord.zcl(rig.short, ON_OFF, 0x01, 0x00, &[]);
    rig.run(100);
    rig.net.coord.zcl(rig.short, LEVEL, 0x01, 0x04, &[254, 30, 0]);
    rig.net.coord.zcl(rig.short, LEVEL, 0x01, 0x03, &[]);
    if together {
        rig.poll_once();
    } else {
        rig.drain();
    }
    let s = rig.bulb.state();
    assert!(s.on);
    rig.net.coord.zcl(rig.short, ON_OFF, 0x01, 0x02, &[]);
    rig.run(100);
    let s = rig.bulb.state();
    (s.on, s.level)
}

#[test]
fn a_stop_in_the_millisecond_a_fade_from_off_starts_leaves_the_light_on() {
    assert_eq!(stop_right_after_a_fade_from_off(true), stop_right_after_a_fade_from_off(false));
}

#[test]
fn a_startup_level_written_behind_a_queued_command_reads_back_before_the_bulb_takes_it() {
    let mut rig = Rig::new(0);
    rig.net.coord.zcl(rig.short, LEVEL, 0x01, 0x00, &[100, 0, 0]);
    rig.net.coord.zcl(rig.short, LEVEL, 0x00, 0x02, &[0x00, 0x40, 0x20, 160]);
    rig.poll_once();
    let read = read_now(&mut rig, LEVEL, &[0x4000]);
    assert_eq!(read, [0x00, 0x40, 0x00, 0x20, 160]);
    assert_eq!(rig.bulb.state().startup_level, 160);
}

#[test]
fn a_startup_temperature_written_behind_a_queued_command_reads_back_before_the_bulb_takes_it() {
    let mut rig = Rig::new(1);
    rig.net.coord.zcl(rig.short, LEVEL, 0x01, 0x00, &[100, 0, 0]);
    rig.net.coord.zcl(rig.short, COLOR, 0x00, 0x02, &[0x10, 0x40, 0x21, 0x2C, 0x01]);
    rig.poll_once();
    let read = read_now(&mut rig, COLOR, &[0x4010]);
    assert_eq!(read, [0x10, 0x40, 0x00, 0x21, 0x2C, 0x01]);
    assert_eq!(rig.bulb.state().startup_color_temp, 300);
}

fn after_app_jump(app: bool) -> (u8, u16) {
    let mut rig = Rig::new(1);
    rig.net.coord.zcl(rig.short, COLOR, 0x01, 0x0A, &[153, 0, 0, 0]);
    rig.run(100);
    rig.net.coord.zcl(rig.short, LEVEL, 0x01, 0x00, &[20, 30, 0]);
    rig.net.coord.zcl(rig.short, COLOR, 0x01, 0x0A, &[0xC6, 0x01, 30, 0]);
    rig.run(1500);
    if app {
        rig.ble.push_back(Command::MoveToLevel { level: 200, transition_ms: 0 });
        rig.ble.push_back(Command::MoveToColorTemperature { mireds: 200, transition_ms: 0 });
    } else {
        rig.net.coord.zcl(rig.short, LEVEL, 0x01, 0x00, &[200, 0, 0]);
        rig.net.coord.zcl(rig.short, COLOR, 0x01, 0x0A, &[200, 0, 0, 0]);
    }
    rig.run(10);
    rig.net.coord.zcl(rig.short, LEVEL, 0x01, 0x02, &[0, 10, 0, 0]);
    rig.net.coord.zcl(rig.short, COLOR, 0x01, 0x4C, &[1, 10, 0, 0, 0, 0, 0, 0, 0]);
    rig.run(100);
    (rig.bulb.state().level, rig.bulb.state().color_temp)
}

#[test]
fn a_step_after_the_app_set_the_light_during_a_zigbee_fade_counts_from_the_app_value() {
    assert_eq!(after_app_jump(true), after_app_jump(false));
}

fn app_back_on_during_fade_to_off(app: Option<Command>) -> (bool, u8) {
    let mut rig = Rig::new(0);
    rig.net.coord.zcl(rig.short, LEVEL, 0x01, 0x04, &[200, 0, 0]);
    rig.run(100);
    rig.net.coord.zcl(rig.short, LEVEL, 0x01, 0x04, &[0, 40, 0]);
    rig.run(1000);
    match app {
        Some(c) => rig.ble.push_back(c),
        None => {
            rig.net.coord.zcl(rig.short, LEVEL, 0x01, 0x04, &[200, 0, 0]);
        }
    }
    rig.run(2000);
    rig.net.coord.zcl(rig.short, LEVEL, 0x01, 0x02, &[0, 10, 0, 0]);
    rig.run(100);
    let s = rig.bulb.state();
    (s.on, s.level)
}

#[test]
fn a_step_after_the_app_relit_the_light_during_a_zigbee_fade_to_off_counts_from_the_app_value() {
    let zigbee = app_back_on_during_fade_to_off(None);
    assert_eq!(app_back_on_during_fade_to_off(Some(Command::MoveToLevel { level: 200, transition_ms: 0 })), zigbee);
}

#[test]
fn a_step_after_the_app_switched_on_during_a_zigbee_fade_to_off_counts_from_the_app_value() {
    let zigbee = app_back_on_during_fade_to_off(None);
    assert_eq!(app_back_on_during_fade_to_off(Some(Command::On)), zigbee);
}

fn replay(kind: u8, chunks: &[(Vec<Ev>, u64, u32)]) -> (StateReport, StateReport) {
    let mut a = Rig::new(kind);
    let mut b = Rig::new(kind);
    for (evs, polls, wait) in chunks {
        for e in evs {
            send(&mut a, e);
            a.drain();
        }
        for e in evs {
            send(&mut b, e);
        }
        for _ in 0..*polls {
            b.poll_once();
        }
        b.drain();
        for _ in 0..*wait {
            a.step_by(1);
            b.step_by(1);
        }
    }
    (a.bulb.state(), b.bulb.state())
}

#[test]
fn a_temperature_step_after_xy_and_a_temperature_move_heard_together_counts_from_the_same_temperature() {
    let z = |group: bool, cmd: u8, p: &[u8]| Ev::Zcl { group, cluster: COLOR, cmd, p: p.to_vec() };
    let chunks = vec![
        (vec![z(false, 0x09, &[223, 44, 252, 113, 3, 0]), z(true, 0x01, &[0, 200, 1, 1]), z(false, 0x4B, &[3, 10, 0, 0, 0, 0, 0, 1, 1])], 1, 0),
        (vec![z(false, 0x4C, &[3, 108, 0, 15, 0, 0, 0, 0, 0])], 1, 1),
    ];
    let (one_by_one, together) = replay(2, &chunks);
    assert_eq!(together.color_temp, one_by_one.color_temp);
}

#[test]
fn a_stop_heard_the_millisecond_a_fade_to_off_ends_before_the_bulb_ticks_leaves_the_light_off() {
    let mut rig = Rig::new(0);
    rig.net.coord.zcl(rig.short, LEVEL, 0x01, 0x04, &[200, 0, 0]);
    rig.run(100);
    rig.net.coord.zcl(rig.short, LEVEL, 0x01, 0x04, &[0, 10, 0]);
    rig.drain();
    rig.run(999);
    rig.net.air.borrow_mut().now += 1;
    rig.net.coord.zcl(rig.short, LEVEL, 0x01, 0x03, &[]);
    rig.net.coord.poll();
    rig.poll_once();
    let seq = rig.net.coord.zcl(rig.short, ON_OFF, 0x00, 0x00, &[0x00, 0x00]);
    for _ in 0..50 {
        rig.net.air.borrow_mut().now += 1;
        rig.net.coord.poll();
        assert_eq!(rig.net.nodes[rig.n].stack.poll(), None);
    }
    assert_eq!(rig.net.coord.last_zcl(rig.short, ON_OFF, 0x01).unwrap(), [0x18, seq, 0x01, 0x00, 0x00, 0x00, 0x10, 0], "before the bulb's report");
    rig.run(100);
    let s = rig.bulb.state();
    assert_eq!((s.on, s.level), (false, 200));
    assert_eq!(consistent(&mut rig, 0, "after the stop"), None);
}

#[test]
fn a_step_down_while_brightening_out_of_an_off_effect_counts_from_the_level_shown() {
    let mut rig = Rig::new(0);
    rig.run(500);
    rig.net.coord.zcl(rig.short, ON_OFF, 0x01, 0x40, &[0, 0]);
    rig.run(300);
    rig.net.coord.zcl(rig.short, LEVEL, 0x01, 0x05, &[0, 40]);
    rig.run(100);
    let shown = rig.bulb.state().shown.level;
    rig.net.coord.zcl(rig.short, LEVEL, 0x01, 0x06, &[1, 39, 1, 0]);
    rig.run(1000);
    let s = rig.bulb.state();
    assert!(s.on && s.level.abs_diff(shown - 39) <= 2, "shown {shown} before the step, then on {} level {} ({:?})", s.on, s.level, rig.log);
}

fn off_effect_then_together(first: &[(u16, u8, &[u8])], wait: u32, then: &[(u16, u8, &[u8])]) -> (u8, bool, u8) {
    let mut rig = Rig::new(0);
    rig.run(500);
    let before = rig.bulb.state().level;
    for (cluster, cmd, p) in first {
        rig.net.coord.zcl(rig.short, *cluster, 0x01, *cmd, p);
    }
    rig.run(wait);
    for (cluster, cmd, p) in then {
        rig.net.coord.zcl(rig.short, *cluster, 0x01, *cmd, p);
    }
    rig.drain();
    rig.run(1000);
    let s = rig.bulb.state();
    (before, s.on, s.level)
}

const BRIGHTEN: (u16, u8, &[u8]) = (LEVEL, 0x05, &[0, 40]);
const DIM_39: (u16, u8, &[u8]) = (LEVEL, 0x06, &[1, 39, 1, 0]);

#[test]
fn a_step_down_heard_with_an_off_effect_and_a_move_up_counts_from_the_level_shown() {
    let (before, on, level) = off_effect_then_together(&[], 0, &[(ON_OFF, 0x40, &[0, 0]), BRIGHTEN, DIM_39]);
    assert!(on && level.abs_diff(before - 39) <= 1, "{before} then on {on} level {level}");
}

#[test]
fn a_step_down_after_an_off_that_cut_an_off_effect_short_counts_from_off() {
    let (_, on, _) = off_effect_then_together(&[(ON_OFF, 0x40, &[0, 2])], 300, &[(ON_OFF, 0x00, &[]), BRIGHTEN, DIM_39]);
    assert!(!on);
}

#[test]
fn a_step_down_after_the_app_cut_an_off_effect_short_counts_from_off() {
    let mut rig = Rig::new(0);
    rig.run(500);
    rig.net.coord.zcl(rig.short, ON_OFF, 0x01, 0x40, &[0, 2]);
    rig.run(300);
    rig.ble.push_back(Command::Off);
    rig.run(100);
    rig.net.coord.zcl(rig.short, LEVEL, 0x01, 0x05, &[0, 40]);
    rig.net.coord.zcl(rig.short, LEVEL, 0x01, 0x06, &[1, 39, 1, 0]);
    rig.drain();
    rig.run(1000);
    assert!(!rig.bulb.state().on, "{:?}", rig.log);
}

#[test]
fn a_step_down_while_brightening_out_of_an_app_fade_to_off_counts_from_the_level_shown() {
    let mut rig = Rig::new(0);
    rig.run(500);
    rig.ble.push_back(Command::FadeOff { transition_ms: 12_800 });
    rig.run(300);
    let shown = rig.bulb.state().shown.level;
    rig.net.coord.zcl(rig.short, LEVEL, 0x01, 0x05, &[0, 40]);
    rig.net.coord.zcl(rig.short, LEVEL, 0x01, 0x06, &[1, 39, 1, 0]);
    rig.drain();
    rig.run(1000);
    let s = rig.bulb.state();
    assert!(s.on && s.level.abs_diff(shown - 39) <= 2, "shown {shown} before the step, then on {} level {} ({:?})", s.on, s.level, rig.log);
}
