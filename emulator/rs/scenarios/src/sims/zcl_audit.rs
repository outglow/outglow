use super::zbase::{Out, ZSim, EP};
use super::{SimCtx, SimDef};
use crate::air::NodeId;
use crate::coord::HA;
use crate::py;
use crate::session::{Stop, R};
use crate::station::nx;

pub fn def() -> SimDef {
    SimDef { name: "zcl_audit", doc: "~70 Zigbee behaviour steps of one image, printed as zcl_audit.py prints them", run }
}

#[derive(Clone, Debug, PartialEq)]
pub enum V {
    None,
    B(bool),
    I(i128),
    F(f64),
    S(String),
    L(Vec<V>),
    T(Vec<V>),
    D(Vec<(V, V)>),
}

impl V {
    pub fn s(x: impl Into<String>) -> V {
        V::S(x.into())
    }
    pub fn repr(&self) -> String {
        match self {
            V::S(s) => py::str_repr(s),
            _ => self.str(),
        }
    }
    pub fn str(&self) -> String {
        match self {
            V::None => "None".into(),
            V::B(b) => py::boolean(*b).into(),
            V::I(i) => i.to_string(),
            V::F(f) => py::float(*f),
            V::S(s) => s.clone(),
            V::L(v) => format!("[{}]", v.iter().map(|x| x.repr()).collect::<Vec<_>>().join(", ")),
            V::T(v) => py::tuple(v.iter().map(|x| x.repr())),
            V::D(v) => format!("{{{}}}", v.iter().map(|(k, x)| format!("{}: {}", k.repr(), x.repr())).collect::<Vec<_>>().join(", ")),
        }
    }
    fn get(&self, k: &str) -> V {
        match self {
            V::D(v) => v.iter().find(|(x, _)| *x == V::s(k)).map(|(_, x)| x.clone()).unwrap_or(V::None),
            _ => V::None,
        }
    }
}

fn d(items: Vec<(&str, V)>) -> V {
    V::D(items.into_iter().map(|(k, v)| (V::s(k), v)).collect())
}

fn hx(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn sizes(t: u8) -> Option<usize> {
    Some(match t {
        0x10 | 0x18 | 0x20 | 0x28 | 0x30 => 1,
        0x19 | 0x21 | 0x29 | 0x31 => 2,
        0x22 => 3,
        0x1B | 0x23 | 0x2B | 0xE2 => 4,
        0x25 => 6,
        0xF0 => 8,
        _ => return None,
    })
}

fn val(t: u8, b: &[u8], i: usize) -> (V, usize) {
    if t == 0x41 || t == 0x42 {
        let n = b[i] as usize;
        if n == 0xFF {
            return (V::None, i + 1);
        }
        let s = String::from_utf8_lossy(b.get(i + 1..(i + 1 + n).min(b.len())).unwrap_or(&[])).to_string();
        return (V::S(s), i + 1 + n);
    }
    match sizes(t) {
        None => (V::S(format!("type{t:#x}:{}", hx(b.get(i..).unwrap_or(&[])))), b.len()),
        Some(n) => {
            let mut v: i128 = 0;
            for (k, x) in b.get(i..(i + n).min(b.len())).unwrap_or(&[]).iter().enumerate() {
                v |= (*x as i128) << (8 * k);
            }
            (V::I(v), i + n)
        }
    }
}

struct A {
    s: ZSim,
    out: Out,
    short: u16,
}

impl A {
    fn out(&mut self, k: &str, v: V) {
        self.out.say(&format!("{k}: {}", v.str()));
    }
    fn run(&mut self, ms: f64) -> R<()> {
        self.s.idle(ms)
    }
    fn light(&self) -> V {
        let st = self.s.light();
        d(vec![
            ("on", V::B(st.on)),
            ("duty", V::F(py::round(st.duty, 2))),
            ("d0", V::F(py::round(st.duty0, 2))),
            ("d5", V::F(py::round(st.duty5, 2))),
        ])
    }
    fn duty(&self) -> f64 {
        py::round(self.s.light().duty, 2)
    }
    fn wait(&mut self, send: &dyn Fn(&mut ZSim, u16), cluster: u16, kinds: &[u8], ms: f64, tries: usize) -> R<Option<Vec<u8>>> {
        let short = self.short;
        for _ in 0..tries {
            let r = self.s.zcl_and_wait(|s| send(s, short), cluster, kinds, ms)?;
            if let Some(x) = r.first() {
                return Ok(Some(x.body.clone()));
            }
        }
        Ok(None)
    }
    fn read(&mut self, cluster: u16, attrs: &[u16]) -> R<V> {
        let mut res: Vec<(V, V)> = Vec::new();
        let set = |res: &mut Vec<(V, V)>, k: String, v: V, only_new: bool| {
            match res.iter_mut().find(|(x, _)| *x == V::S(k.clone())) {
                Some(e) => {
                    if !only_new {
                        e.1 = v
                    }
                }
                None => res.push((V::S(k), v)),
            }
        };
        for chunk in attrs.chunks(4) {
            let c = chunk.to_vec();
            let b = self.wait(&move |s, short| {
                s.w.coord.read_attrs(&mut s.w.air, short, EP, cluster, &c);
            }, cluster, &[0x01, 0x0B], 2000.0, 2)?;
            let Some(b) = b else {
                for x in chunk {
                    set(&mut res, format!("{x:#06x}"), V::s("no response"), false);
                }
                continue;
            };
            let mut i = 0usize;
            let mut got = Vec::new();
            while i + 3 <= b.len() {
                let at = u16::from_le_bytes([b[i], b[i + 1]]);
                let st = b[i + 2];
                i += 3;
                got.push(at);
                if st != 0 {
                    set(&mut res, format!("{at:#06x}"), V::S(format!("status {st:#04x}")), false);
                    continue;
                }
                let t = b[i];
                let (v, ni) = val(t, &b, i + 1);
                i = ni;
                set(&mut res, format!("{at:#06x}"), v, false);
            }
            for x in chunk {
                if !got.contains(x) {
                    set(&mut res, format!("{x:#06x}"), V::S(format!("rsp {}", hx(&b))), true);
                }
            }
        }
        Ok(V::D(res))
    }
    fn rd1(&mut self, cluster: u16, attr: u16) -> R<V> {
        Ok(self.read(cluster, &[attr])?.get(&format!("{attr:#06x}")))
    }
    fn write(&mut self, cluster: u16, attr: u16, t: u8, value: u64, n: usize) -> R<V> {
        let mut pl = attr.to_le_bytes().to_vec();
        pl.push(t);
        pl.extend_from_slice(&value.to_le_bytes()[..n]);
        let b = self.wait(&move |s, short| {
            s.w.coord.zcl_send_with(&mut s.w.air, short, EP, cluster, 0x02, &pl, HA, false, false, None, false);
        }, cluster, &[0x04, 0x0B], 2000.0, 2)?;
        Ok(V::S(match b {
            None => "no response".into(),
            Some(b) if b.first() == Some(&0) => "ok".into(),
            Some(b) => format!("status {:#04x}", b[0]),
        }))
    }
    fn discover(&mut self, cluster: u16) -> R<V> {
        let mut attrs: Vec<u16> = Vec::new();
        let mut start: u16 = 0;
        for _ in 0..8 {
            let mut pl = start.to_le_bytes().to_vec();
            pl.push(40);
            let b = self.wait(&move |s, short| {
                s.w.coord.zcl_send_with(&mut s.w.air, short, EP, cluster, 0x0C, &pl, HA, false, false, None, false);
            }, cluster, &[0x0D, 0x0B], 2000.0, 2)?;
            let Some(b) = b else { return Ok(V::s("no response")) };
            if b.len() == 2 && b[0] == 0x0C {
                return Ok(V::S(format!("default-rsp status {:#04x}", b[1])));
            }
            let got: Vec<u16> = (0..(b.len() - 1) / 3).map(|i| u16::from_le_bytes([b[1 + 3 * i], b[2 + 3 * i]])).collect();
            attrs.extend(&got);
            if b[0] != 0 || got.is_empty() {
                break;
            }
            start = got.last().unwrap() + 1;
        }
        Ok(V::L(attrs.iter().map(|x| V::S(format!("{x:#06x}"))).collect()))
    }
    fn cmd(&mut self, cluster: u16, cid: u8, payload: &[u8], kinds: &[u8]) -> R<String> {
        let pl = payload.to_vec();
        let b = self.wait(&move |s, short| {
            s.w.coord.zcl_cmd(&mut s.w.air, short, EP, cluster, cid, &pl);
        }, cluster, kinds, 2000.0, 1)?;
        Ok(b.map(|b| hx(&b)).unwrap_or_else(|| "no response".into()))
    }
    fn c(&mut self, cluster: u16, cid: u8, payload: &[u8]) -> R<String> {
        self.cmd(cluster, cid, payload, &[0x0B])
    }
    fn dstat(h: String) -> V {
        if h.len() != 4 {
            V::S(h)
        } else if &h[2..] == "00" {
            V::s("ok")
        } else {
            V::S(format!("status 0x{}", &h[2..]))
        }
    }
    fn dc(&mut self, cluster: u16, cid: u8, payload: &[u8]) -> R<V> {
        Ok(A::dstat(self.c(cluster, cid, payload)?))
    }
    fn zdo(&mut self, cluster: u16, payload: &[u8], ms: f64) -> R<Option<Vec<u8>>> {
        let n0 = self.s.w.coord.zcl_rsp.len();
        let short = self.short;
        self.s.w.coord.zdo_send(&mut self.s.w.air, short, cluster, payload);
        let want = cluster | 0x8000;
        self.s.run_until(ms, 20.0, |s| s.w.coord.zcl_rsp[n0..].iter().any(|r| r.cluster == want))?;
        Ok(self.s.w.coord.zcl_rsp[n0..].iter().find(|r| r.cluster == want).map(|r| r.body.clone()))
    }
    fn reports(&self, n0: usize, cluster: u16) -> V {
        V::L(self.s.w.coord.zcl_rsp[n0..]
            .iter()
            .filter(|r| r.cluster == cluster && r.cmd == Some(0x0A))
            .map(|r| V::S(hx(&r.body)))
            .collect())
    }
    fn n_reports(&self, n0: usize, cluster: u16) -> i128 {
        match self.reports(n0, cluster) {
            V::L(v) => v.len() as i128,
            _ => 0,
        }
    }
    fn configure_reporting(&mut self, cluster: u16, attr: u16, dtype: u8, min_s: u16, max_s: u16, change: &[u8]) -> R<Option<Vec<u8>>> {
        let mut pl = vec![0x00];
        pl.extend_from_slice(&attr.to_le_bytes());
        pl.push(dtype);
        pl.extend_from_slice(&min_s.to_le_bytes());
        pl.extend_from_slice(&max_s.to_le_bytes());
        pl.extend_from_slice(change);
        self.wait(&move |s, short| {
            s.w.coord.zcl_send_with(&mut s.w.air, short, EP, cluster, 0x06, &pl, HA, false, false, None, false);
        }, cluster, &[0x07, 0x0B], 2000.0, 2)
    }
    fn samp(&mut self, n: usize, step: f64) -> R<V> {
        let mut o = Vec::new();
        for _ in 0..n {
            self.run(step)?;
            let st = self.s.light();
            o.push(V::T(vec![V::I(st.on as i128), V::F(py::round(st.duty, 2))]));
        }
        Ok(V::L(o))
    }
    fn trace(&mut self, n: usize, step: f64) -> R<Vec<f64>> {
        let mut tr = Vec::new();
        for _ in 0..n {
            self.run(step)?;
            tr.push(self.duty());
        }
        Ok(tr)
    }
    fn power_cycle(&mut self, settle: f64) -> R<(V, V)> {
        self.s.w.reboot().map_err(Stop::Error)?;
        self.run(1500.0)?;
        let st = self.light();
        self.run(settle)?;
        let alive = self.rd1(0x0006, 0)?;
        Ok((st, alive))
    }
}

fn mtl(level: u8, t: u16) -> Vec<u8> {
    let mut p = vec![level];
    p.extend_from_slice(&t.to_le_bytes());
    p
}

fn fl(v: &[f64]) -> V {
    V::L(v.iter().map(|x| V::F(*x)).collect())
}

fn minf(v: &[f64]) -> f64 {
    v.iter().copied().fold(f64::INFINITY, f64::min)
}

fn maxf(v: &[f64]) -> f64 {
    v.iter().copied().fold(f64::NEG_INFINITY, f64::max)
}

fn run(cx: &mut SimCtx) -> R<()> {
    let img = cx.image("bin", "target/tc32/bulb-tc32.bin");
    let out = Out::new(cx.verbose > 0);
    let mut s = ZSim::new(&img, cx.cpi, out.clone())?;
    s.w.air.log = None;
    let mut a = A { s, out: out.clone(), short: 0 };
    let r = body(&mut a);
    out.into_ctx(cx);
    cx.results.push(("zcl_audit".into(), r.is_ok() && a.short != 0, String::new()));
    r
}

fn body(a: &mut A) -> R<()> {
    a.run(1000.0)?;
    let l = a.light();
    a.out("power_on_light", l);
    let ok = a.s.wait_event("device-announce", 60000.0)?;
    a.out("join", V::B(ok));
    if !ok {
        return Ok(());
    }
    let dev = a.s.w.coord.devices.iter().find(|d| d.announced).unwrap().clone();
    a.short = dev.short;
    let short = a.short;
    a.run(6000.0)?;
    let n_start = a.s.w.coord.zcl_rsp.len();

    if let Some(p) = a.zdo(0x0002, &short.to_le_bytes(), 2500.0)? {
        if p.get(1) == Some(&0) {
            let nd = &p[4..];
            let v = d(vec![
                ("logical_type", V::I((nd[0] & 7) as i128)),
                ("mac_cap", V::S(format!("{:#04x}", nd[2]))),
                ("mfg", V::S(format!("{:#06x}", u16::from_le_bytes([nd[3], nd[4]])))),
                ("server_mask", V::S(format!("{:#06x}", u16::from_le_bytes([nd[8], nd[9]])))),
            ]);
            a.out("node_desc", v);
        }
    }
    let p = a.zdo(0x0005, &short.to_le_bytes(), 2500.0)?;
    let eps: Vec<u8> = p.map(|p| p[5..5 + p[4] as usize].to_vec()).unwrap_or_default();
    a.out("endpoints", V::L(eps.iter().map(|e| V::I(*e as i128)).collect()));
    let mut sd: Vec<(V, V)> = Vec::new();
    let mut ins1: Vec<u16> = Vec::new();
    for ep in &eps {
        let mut q = short.to_le_bytes().to_vec();
        q.push(*ep);
        let p = a.zdo(0x0004, &q, 2500.0)?;
        let Some(p) = p.filter(|p| p.get(1) == Some(&0)) else {
            sd.push((V::I(*ep as i128), V::s("none")));
            continue;
        };
        let u = |i: usize| u16::from_le_bytes([p[i], p[i + 1]]);
        let (prof, devid) = (u(6), u(8));
        let nin = p[11] as usize;
        let ins: Vec<u16> = (0..nin).map(|i| u(12 + 2 * i)).collect();
        let o = 12 + 2 * nin;
        let nout = p[o] as usize;
        let outs: Vec<u16> = (0..nout).map(|i| u(o + 1 + 2 * i)).collect();
        if *ep == 1 {
            ins1 = ins.clone();
        }
        let hl = |v: &[u16]| V::L(v.iter().map(|x| V::S(format!("{x:#06x}"))).collect());
        sd.push((V::I(*ep as i128), d(vec![
            ("profile", V::S(format!("{prof:#06x}"))),
            ("device", V::S(format!("{devid:#06x}"))),
            ("in", hl(&ins)),
            ("out", hl(&outs)),
        ])));
    }
    a.out("simple_desc", V::D(sd));
    let has_color = ins1.contains(&0x0300);

    let mut disc = Vec::new();
    for cl in ins1.clone() {
        if cl < 0x8000 {
            let v = a.discover(cl)?;
            disc.push((V::S(format!("{cl:#06x}")), v));
        }
    }
    a.out("discover_attrs", V::D(disc));
    let v = a.read(0x0000, &[0, 1, 2, 3, 4, 5, 6, 7, 0x4000])?;
    a.out("basic", v);
    let v = a.read(0x0006, &[0x0000, 0x4000, 0x4001, 0x4002, 0x4003])?;
    a.out("onoff_attrs", v);
    let v = a.read(0x0008, &[0x0000, 0x0001, 0x0002, 0x0003, 0x000F, 0x0010, 0x0011, 0x0012, 0x0013, 0x0014, 0x4000])?;
    a.out("level_attrs", v);
    let v = a.read(0x0300, &[0x0003, 0x0004, 0x0007, 0x0008, 0x000F, 0x400A, 0x400B, 0x400C, 0x400D, 0x4010])?;
    a.out("color_attrs", v);
    let v = a.read(0x0003, &[0x0000])?;
    a.out("identify_attrs", v);
    let v = a.read(0x0004, &[0x0000])?;
    a.out("groups_attrs", v);
    let v = a.read(0x0005, &[0, 1, 2, 3, 4])?;
    a.out("scenes_attrs", v);
    let v = a.read(0x0B05, &[0x011C, 0x011D])?;
    a.out("diagnostics_0b05", v);
    let v = a.read(0x000A, &[0x0000, 0x0001])?;
    a.out("time_attrs", v);

    let v = a.dc(0x0006, 0x01, &[])?;
    a.out("on", v);
    a.run(1500.0)?;
    let v = a.dc(0x0008, 0x04, &mtl(254, 0))?;
    a.out("mtl_254_instant", v);
    a.run(1500.0)?;
    let l0 = a.duty();
    let r = a.dc(0x0008, 0x04, &mtl(20, 30))?;
    let tr = a.trace(8, 500.0)?;
    let la = a.rd1(0x0008, 0)?;
    a.out("transition_3s_254_to_20", d(vec![("rsp", r), ("start", V::F(l0)), ("duty_every_500ms", fl(&tr)), ("level_after", la)]));
    let v = a.dc(0x0008, 0x04, &mtl(254, 0))?;
    a.out("mtl_254_again", v);
    a.run(1200.0)?;
    let r = a.dc(0x0008, 0x01, &[1, 50])?;
    a.run(1000.0)?;
    let r2 = a.dc(0x0008, 0x03, &[])?;
    a.run(500.0)?;
    let lv = a.rd1(0x0008, 0)?;
    let l = a.light();
    a.out("move_down_then_stop", d(vec![("move", r), ("stop", r2), ("level", lv), ("light", l)]));
    let r = a.dc(0x0008, 0x02, &[0, 40, 0, 0])?;
    a.run(800.0)?;
    let lv = a.rd1(0x0008, 0)?;
    a.out("step_up_40", d(vec![("rsp", r), ("level", lv)]));

    a.c(0x0008, 0x04, &mtl(200, 0))?;
    a.run(1000.0)?;
    let v = a.dc(0x0006, 0x00, &[])?;
    a.out("off", v);
    a.run(1500.0)?;
    let r = a.dc(0x0008, 0x00, &mtl(60, 0))?;
    a.run(800.0)?;
    let a1 = d(vec![("rsp", r), ("level", a.rd1(0x0008, 0)?), ("light", a.light())]);
    let mut p = mtl(90, 0);
    p.extend_from_slice(&[1, 1]);
    let r = a.dc(0x0008, 0x00, &p)?;
    a.run(800.0)?;
    let a2 = d(vec![("rsp", r), ("level", a.rd1(0x0008, 0)?), ("light", a.light())]);
    let w = a.write(0x0008, 0x000F, 0x18, 0x01, 1)?;
    let r = a.dc(0x0008, 0x00, &mtl(120, 0))?;
    a.run(800.0)?;
    let a3 = d(vec![("write_options", w), ("rsp", r), ("level", a.rd1(0x0008, 0)?), ("light", a.light())]);
    a.write(0x0008, 0x000F, 0x18, 0x00, 1)?;
    a.c(0x0006, 0x01, &[])?;
    a.run(1500.0)?;
    let ao = d(vec![("level", a.rd1(0x0008, 0)?), ("light", a.light())]);
    a.out("execute_if_off", d(vec![("plain", a1), ("override_in_cmd", a2), ("options_attr", a3), ("after_on", ao)]));

    a.c(0x0008, 0x04, &mtl(254, 0))?;
    a.run(1000.0)?;
    let r = a.dc(0x0006, 0x40, &[0, 0])?;
    let tr = a.trace(5, 300.0)?;
    let oo = a.rd1(0x0006, 0)?;
    a.out("off_with_effect", d(vec![("rsp", r), ("duty_every_300ms", fl(&tr)), ("onoff", oo)]));
    let r = a.dc(0x0006, 0x41, &[])?;
    a.run(1200.0)?;
    let l = a.light();
    a.out("on_with_recall_global_scene", d(vec![("rsp", r), ("light", l)]));
    a.c(0x0006, 0x00, &[])?;
    a.run(1200.0)?;
    let mut p = vec![0];
    p.extend_from_slice(&20u16.to_le_bytes());
    p.extend_from_slice(&0u16.to_le_bytes());
    let r = a.dc(0x0006, 0x42, &p)?;
    a.run(800.0)?;
    let x1 = a.light();
    a.run(3000.0)?;
    let l = a.light();
    let oo = a.rd1(0x0006, 0)?;
    a.out("on_with_timed_off_2s", d(vec![("rsp", r), ("at_0.8s", x1), ("at_3.8s", l), ("onoff", oo)]));
    a.c(0x0006, 0x01, &[])?;
    a.run(1200.0)?;

    a.c(0x0008, 0x04, &mtl(254, 0))?;
    a.run(1000.0)?;
    let r = a.dc(0x0003, 0x00, &4u16.to_le_bytes())?;
    let tr = a.trace(20, 200.0)?;
    let changes = (1..tr.len()).filter(|i| tr[*i] != tr[i - 1]).count() as i128;
    a.out("identify_4s", d(vec![("rsp", r), ("duty_min", V::F(minf(&tr))), ("duty_max", V::F(maxf(&tr))), ("changes", V::I(changes))]));
    a.run(1500.0)?;
    let r = a.dc(0x0003, 0x40, &[0, 0])?;
    let tr = a.trace(15, 200.0)?;
    a.out("identify_trigger_effect_blink", d(vec![("rsp", r), ("duty_min", V::F(minf(&tr))), ("duty_max", V::F(maxf(&tr)))]));
    let r = a.dc(0x0003, 0x40, &[1, 0])?;
    let tr = a.trace(25, 200.0)?;
    a.out("identify_trigger_effect_breathe", d(vec![("rsp", r), ("duty_min", V::F(minf(&tr))), ("duty_max", V::F(maxf(&tr)))]));
    a.c(0x0003, 0x40, &[0xFF, 0])?;
    a.run(1500.0)?;

    const G: u16 = 0x1234;
    let mut p = G.to_le_bytes().to_vec();
    p.push(0);
    let v = a.cmd(0x0004, 0x00, &p, &[0x00, 0x0B])?;
    a.out("group_add", V::S(v));
    let v = a.cmd(0x0004, 0x02, &[0], &[0x02, 0x0B])?;
    a.out("group_membership", V::S(v));
    a.c(0x0008, 0x04, &mtl(200, 0))?;
    a.run(1000.0)?;
    let mut gs = G.to_le_bytes().to_vec();
    gs.push(1);
    let v = a.cmd(0x0005, 0x04, &gs, &[0x04, 0x0B])?;
    a.out("scene_store", V::S(v));
    a.c(0x0008, 0x04, &mtl(30, 0))?;
    a.run(1000.0)?;
    let lv_before = a.rd1(0x0008, 0)?;
    let r = a.dc(0x0005, 0x05, &gs)?;
    a.run(1500.0)?;
    let la = a.rd1(0x0008, 0)?;
    let l = a.light();
    a.out("scene_recall", d(vec![("rsp", r), ("level_before", lv_before), ("level_after", la), ("light", l)]));
    let v = a.cmd(0x0005, 0x06, &G.to_le_bytes(), &[0x06, 0x0B])?;
    a.out("scene_membership", V::S(v));
    a.c(0x0006, 0x01, &[])?;
    a.run(1000.0)?;
    {
        let w = &mut a.s.w;
        let mut aps = vec![0x0C];
        aps.extend_from_slice(&G.to_le_bytes());
        aps.extend_from_slice(&0x0006u16.to_le_bytes());
        aps.extend_from_slice(&0x0104u16.to_le_bytes());
        aps.push(0x01);
        aps.push(nx(&mut w.coord.st.aps_ctr));
        aps.extend_from_slice(&[0x11, 0x77, 0x00]);
        w.coord.nwk_send(&mut w.air, 0xFFFD, &aps, 0, true, 30, Some(0xFFFF), true);
    }
    a.run(2000.0)?;
    let l = a.light();
    let oo = a.rd1(0x0006, 0)?;
    a.out("group_addressed_off", d(vec![("light", l), ("onoff", oo)]));
    a.c(0x0006, 0x01, &[])?;
    a.run(1200.0)?;

    let v = d(vec![("onoff", V::I(a.n_reports(n_start, 0x0006))), ("level", V::I(a.n_reports(n_start, 0x0008)))]);
    a.out("reports_seen_without_config", v);
    let coord_ext = a.s.w.coord.ext;
    let bind_req = |cl: u16| {
        let mut q = dev.ext.to_le_bytes().to_vec();
        q.push(EP);
        q.extend_from_slice(&cl.to_le_bytes());
        q.push(0x03);
        q.extend_from_slice(&coord_ext.to_le_bytes());
        q.push(0x01);
        q
    };
    let mut bind = Vec::new();
    for cl in [0x0006u16, 0x0008] {
        let p = a.zdo(0x0021, &bind_req(cl), 2500.0)?;
        let v = match p {
            None => "no response".to_string(),
            Some(p) if p[1] == 0 => "ok".to_string(),
            Some(p) => format!("status {:#04x}", p[1]),
        };
        bind.push((V::S(format!("{cl:#06x}")), V::S(v)));
    }
    a.out("bind", V::D(bind));
    let b = a.configure_reporting(0x0006, 0x0000, 0x10, 0, 60, &[])?;
    let cr1 = b.map(|b| hx(&b)).unwrap_or_else(|| "no response".into());
    let b = a.configure_reporting(0x0008, 0x0000, 0x20, 1, 60, &[1])?;
    let cr2 = b.map(|b| hx(&b)).unwrap_or_else(|| "no response".into());
    a.out("configure_reporting", d(vec![("onoff", V::S(cr1)), ("level", V::S(cr2))]));
    let b = a.wait(&|s, short| {
        s.w.coord.zcl_send_with(&mut s.w.air, short, EP, 0x0008, 0x08, &[0, 0, 0], HA, false, false, None, false);
    }, 0x0008, &[0x09, 0x0B], 2000.0, 2)?;
    a.out("read_reporting_config_level", V::S(b.map(|b| hx(&b)).unwrap_or_else(|| "no response".into())));
    let n0 = a.s.w.coord.zcl_rsp.len();
    a.s.w.coord.zcl_send_with(&mut a.s.w.air, short, EP, 0x0006, 0x00, &[], HA, true, true, None, false);
    a.run(4000.0)?;
    let ra = a.reports(n0, 0x0006);
    let n0 = a.s.w.coord.zcl_rsp.len();
    a.s.w.coord.zcl_send_with(&mut a.s.w.air, short, EP, 0x0008, 0x04, &mtl(90, 10), HA, true, true, None, false);
    a.run(5000.0)?;
    let v = d(vec![("onoff_after_off", ra), ("level_after_move", a.reports(n0, 0x0008)), ("onoff_after_move_with_onoff", a.reports(n0, 0x0006))]);
    a.out("reports_after_change", v);

    let mut mg = Vec::new();
    for (name, cl, pl) in [("lqi", 0x0031u16, vec![0u8]), ("rtg", 0x0032, vec![0]), ("bind_table", 0x0033, vec![0]),
                           ("permit_join", 0x0036, vec![10, 1])] {
        let p = a.zdo(cl, &pl, 2500.0)?;
        let v = match p {
            None => "no response".to_string(),
            Some(p) if p[1] == 0 => format!("ok {}", hx(p.get(2..6.min(p.len())).unwrap_or(&[]))),
            Some(p) => format!("status {:#04x}", p[1]),
        };
        mg.push((V::s(name), V::S(v)));
    }
    a.out("zdo_mgmt", V::D(mg));

    if has_color {
        let ct16 = |m: u16, t: u16| {
            let mut p = m.to_le_bytes().to_vec();
            p.extend_from_slice(&t.to_le_bytes());
            p
        };
        let mut ct = Vec::new();
        a.c(0x0006, 0x01, &[])?;
        a.c(0x0008, 0x04, &mtl(254, 0))?;
        a.run(1200.0)?;
        ct.push(("mtct_200", a.dc(0x0300, 0x0A, &ct16(200, 0))?));
        a.run(1200.0)?;
        ct.push(("ct_200", d(vec![("attr", a.rd1(0x0300, 7)?), ("light", a.light())])));
        ct.push(("mtct_400", a.dc(0x0300, 0x0A, &ct16(400, 0))?));
        a.run(1200.0)?;
        ct.push(("ct_400", d(vec![("attr", a.rd1(0x0300, 7)?), ("light", a.light())])));
        a.c(0x0006, 0x00, &[])?;
        a.run(1200.0)?;
        let r = a.dc(0x0300, 0x0A, &ct16(250, 0))?;
        a.run(800.0)?;
        let e1 = d(vec![("rsp", r), ("attr", a.rd1(0x0300, 7)?)]);
        let mut p = ct16(300, 0);
        p.extend_from_slice(&[1, 1]);
        let r = a.dc(0x0300, 0x0A, &p)?;
        a.run(800.0)?;
        let e2 = d(vec![("rsp", r), ("attr", a.rd1(0x0300, 7)?)]);
        ct.push(("execute_if_off", d(vec![("plain", e1), ("override_in_cmd", e2)])));
        a.c(0x0006, 0x01, &[])?;
        a.run(1200.0)?;
        ct.push(("write_startup_ct", a.write(0x0300, 0x4010, 0x21, 350, 2)?));
        ct.push(("write_level_options_couple_ct", a.write(0x0008, 0x000F, 0x18, 0x02, 1)?));
        a.c(0x0300, 0x0A, &ct16(200, 0))?;
        a.run(1000.0)?;
        a.c(0x0008, 0x04, &mtl(30, 0))?;
        a.run(1200.0)?;
        ct.push(("couple_ct_to_level", d(vec![("ct_at_level_30", a.rd1(0x0300, 7)?), ("light", a.light())])));
        a.write(0x0008, 0x000F, 0x18, 0x00, 1)?;
        a.out("color_temp", d(ct));
    }

    let mut pc = Vec::new();
    a.c(0x0006, 0x01, &[])?;
    a.c(0x0008, 0x04, &mtl(77, 0))?;
    a.run(4000.0)?;
    let (st, alive) = a.power_cycle(3000.0)?;
    pc.push(("default_after_on_level77", d(vec![("light_1.5s", st), ("onoff", alive), ("level", a.rd1(0x0008, 0)?)])));
    a.c(0x0006, 0x00, &[])?;
    a.run(4000.0)?;
    let (st, alive) = a.power_cycle(3000.0)?;
    pc.push(("default_after_off", d(vec![("light_1.5s", st), ("onoff", alive)])));
    let orig = a.rd1(0x0006, 0x4003)?;
    pc.push(("startup_onoff_orig", orig.clone()));
    pc.push(("write_startup_onoff_off", a.write(0x0006, 0x4003, 0x30, 0x00, 1)?));
    a.c(0x0006, 0x01, &[])?;
    a.run(4000.0)?;
    let (st, alive) = a.power_cycle(3000.0)?;
    pc.push(("startup_off_after_on", d(vec![("light_1.5s", st), ("onoff", alive)])));
    pc.push(("write_startup_onoff_previous", a.write(0x0006, 0x4003, 0x30, 0xFF, 1)?));
    a.c(0x0006, 0x00, &[])?;
    a.run(4000.0)?;
    let (st, alive) = a.power_cycle(3000.0)?;
    pc.push(("startup_previous_after_off", d(vec![("light_1.5s", st), ("onoff", alive)])));
    pc.push(("write_startup_onoff_on", a.write(0x0006, 0x4003, 0x30, 0x01, 1)?));
    pc.push(("write_startup_level_40", a.write(0x0008, 0x4000, 0x20, 40, 1)?));
    a.c(0x0006, 0x01, &[])?;
    a.c(0x0008, 0x04, &mtl(200, 0))?;
    a.run(4000.0)?;
    let (st, alive) = a.power_cycle(3000.0)?;
    pc.push(("startup_on_level40", d(vec![("light_1.5s", st), ("onoff", alive), ("level", a.rd1(0x0008, 0)?)])));
    a.write(0x0008, 0x4000, 0x20, 0xFF, 1)?;
    if let V::I(o) = orig {
        a.write(0x0006, 0x4003, 0x30, o as u64, 1)?;
    }
    let n_ann = a.s.w.coord.events.iter().filter(|e| e.kind == "device-announce").count() as i128;
    pc.push(("rejoin_events", V::I(n_ann)));
    a.out("power_cycle", d(pc));

    for cl in [0x0006u16, 0x0008] {
        a.zdo(0x0021, &bind_req(cl), 2500.0)?;
    }
    a.configure_reporting(0x0006, 0x0000, 0x10, 0, 60, &[])?;
    a.c(0x0006, 0x01, &[])?;
    a.c(0x0008, 0x04, &mtl(200, 0))?;
    a.run(1500.0)?;
    a.c(0x0006, 0x00, &[])?;
    a.run(3000.0)?;
    let n0 = a.s.w.coord.zcl_rsp.len();
    let r = a.dc(0x0008, 0x04, &mtl(90, 10))?;
    a.run(4000.0)?;
    let v = d(vec![("rsp", r), ("onoff_reports", a.reports(n0, 0x0006)), ("onoff", a.rd1(0x0006, 0)?), ("light", a.light())]);
    a.out("mtl_with_onoff_from_off", v);
    a.c(0x0008, 0x04, &mtl(254, 0))?;
    a.run(1500.0)?;
    let g0 = a.rd1(0x0006, 0x4000)?;
    let r1 = a.dc(0x0006, 0x40, &[0, 0])?;
    let fade = a.samp(12, 100.0)?;
    let g1 = a.rd1(0x0006, 0x4000)?;
    let r2 = a.dc(0x0006, 0x41, &[])?;
    a.run(1500.0)?;
    let oo = a.rd1(0x0006, 0)?;
    let l = a.light();
    let ga = a.rd1(0x0006, 0x4000)?;
    a.out("off_with_effect_fade_and_global_scene", d(vec![("gsc_before", g0), ("off_effect", r1), ("fade_100ms(on,duty)", fade),
        ("gsc_after_off", g1), ("recall", r2), ("onoff", oo), ("light", l), ("gsc_after_on", ga)]));
    a.c(0x0006, 0x01, &[])?;
    a.run(1500.0)?;
    a.c(0x0008, 0x04, &mtl(120, 0))?;
    a.run(1500.0)?;
    let d_move = a.light();
    a.c(0x0006, 0x00, &[])?;
    a.run(2500.0)?;
    a.c(0x0006, 0x01, &[])?;
    let d_on = a.samp(10, 500.0)?;
    a.c(0x0008, 0x04, &mtl(120, 0))?;
    a.run(1500.0)?;
    let l = a.light();
    a.out("level120_after_move_vs_on", d(vec![("after_move", d_move), ("after_on_500ms(on,duty)", d_on), ("after_move_again", l)]));
    a.c(0x0008, 0x04, &mtl(254, 0))?;
    a.run(1200.0)?;
    let r = a.dc(0x0003, 0x00, &3u16.to_le_bytes())?;
    let tr = a.samp(34, 100.0)?;
    a.out("identify_3s_trace", d(vec![("rsp", r), ("(on,duty)_100ms", tr)]));

    let mut kinds: Vec<String> = a.s.w.coord.events.iter().map(|e| e.kind.clone()).collect();
    kinds.sort();
    kinds.dedup();
    a.out("events", V::L(kinds.into_iter().map(V::S).collect()));
    let resets = a.s.w.resets as i128;
    a.out("resets", V::I(resets));
    let _ = NodeId::Dut;
    Ok(())
}
