use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Instant;

use crate::image::Image;
use crate::py;
use crate::session::{Session, SessionOpts, Stop, R};
use crate::sims::json::{dump_str, J};
use crate::station::Tx;

pub const PWM_MUX: [(&str, usize, u32, u8, &str); 25] = [
    ("PA0", 0x5a8, 0, 1, "PWM0_N"), ("PA2", 0x5a8, 4, 2, "PWM0"), ("PA3", 0x5a8, 6, 2, "PWM1"),
    ("PA4", 0x5a9, 0, 2, "PWM2"), ("PB0", 0x5aa, 0, 0, "PWM3"), ("PB1", 0x5aa, 2, 0, "PWM4"),
    ("PB2", 0x5aa, 4, 0, "PWM5"), ("PB3", 0x5aa, 6, 0, "PWM0_N"), ("PB4", 0x5ab, 0, 1, "PWM4"),
    ("PB5", 0x5ab, 2, 1, "PWM5"), ("PC0", 0x5ac, 0, 1, "PWM4_N"), ("PC1", 0x5ac, 2, 1, "PWM1_N"),
    ("PC1", 0x5ac, 2, 2, "PWM0"), ("PC2", 0x5ac, 4, 0, "PWM0"), ("PC3", 0x5ac, 6, 0, "PWM1"),
    ("PC4", 0x5ad, 0, 0, "PWM2"), ("PC4", 0x5ad, 0, 2, "PWM0_N"), ("PC5", 0x5ad, 2, 0, "PWM3_N"),
    ("PC6", 0x5ad, 4, 2, "PWM4_N"), ("PC7", 0x5ad, 6, 2, "PWM5_N"), ("PD2", 0x5ae, 4, 2, "PWM3"),
    ("PD3", 0x5ae, 6, 0, "PWM1_N"), ("PD4", 0x5af, 0, 2, "PWM2_N"), ("PD5", 0x5af, 2, 0, "PWM0"),
    ("PD5", 0x5af, 2, 2, "PWM0_N"),
];

pub fn gpio_func(port: u8) -> usize {
    match port {
        b'A' => 0x586,
        b'B' => 0x58e,
        b'C' => 0x596,
        b'D' => 0x59e,
        _ => 0x5a6,
    }
}

pub const ON_OFF: u16 = 0x0006;
pub const LEVEL: u16 = 0x0008;
pub const COLOR: u16 = 0x0300;
pub const BASIC: u16 = 0x0000;

#[derive(Clone, Debug, PartialEq)]
pub struct Outputs {
    pub pins: Vec<(&'static str, &'static str)>,
    pub chans: [(bool, u16, u16); 6],
    pub raw: [u8; 8],
}

impl Outputs {
    pub fn json(&self) -> J {
        let pins = self.pins.iter().map(|(p, f)| J::Arr(vec![J::Str(p.to_string()), J::Str(f.to_string())])).collect();
        let chans = self
            .chans
            .iter()
            .enumerate()
            .map(|(i, &(en, cmp, max))| {
                obj(vec![("ch", J::Int(i as i64)), ("en", J::Bool(en)), ("cmp", J::Int(cmp as i64)), ("max", J::Int(max as i64))])
            })
            .collect();
        let raw = self.raw.iter().map(|b| format!("{b:02x}")).collect();
        obj(vec![("pins", J::Arr(pins)), ("chans", J::Arr(chans)), ("raw_pwm", J::Str(raw))])
    }
}

pub type Duties = Vec<(u8, f64)>;

pub fn duties_json(d: &Duties) -> J {
    J::Obj(d.iter().map(|(c, v)| (format!("pwm{c}"), J::Float(*v))).collect())
}

pub fn obj(v: Vec<(&str, J)>) -> J {
    J::Obj(v.into_iter().map(|(k, j)| (k.to_string(), j)).collect())
}

fn merge(mut a: Vec<(String, J)>, b: Vec<(String, J)>) -> Vec<(String, J)> {
    for (k, v) in b {
        match a.iter_mut().find(|(x, _)| *x == k) {
            Some(e) => e.1 = v,
            None => a.push((k, v)),
        }
    }
    a
}

pub struct Bulb {
    pub s: Session,
    pub ep: u8,
}

impl Bulb {
    pub fn new(path: &Path, cpi: i64) -> R<Bulb> {
        let img = Image { label: file_stem(path), path: path.to_path_buf(), link_shift: None };
        let s = Session::new(&img, SessionOpts { cpi, join: false, ..Default::default() })?;
        Ok(Bulb { s, ep: 1 })
    }

    pub fn ms(&self) -> f64 {
        self.s.now()
    }

    pub fn run(&mut self, ms: f64) -> R<()> {
        self.s.run(ms)
    }

    pub fn join(&mut self) -> R<bool> {
        match self.s.join(60.0, 0.0) {
            Ok(()) => {}
            Err(Stop::SessionLost(_)) => return Ok(false),
            Err(e) => return Err(e),
        }
        self.s.run(5700.0)?;
        Ok(true)
    }

    pub fn outputs(&self) -> Outputs {
        let g = &self.s.w.air.dut.m.regs;
        let mut pins = Vec::new();
        for &(pin, mreg, shift, val, func) in PWM_MUX.iter() {
            let b = pin.as_bytes();
            let bit = b[2] - b'0';
            let special = g[gpio_func(b[1])] & (1 << bit) == 0;
            if special && (g[mreg] >> shift) & 3 == val {
                pins.push((pin, func));
            }
        }
        let p = &g[0x780..0x7c0];
        let mut chans = [(false, 0u16, 0u16); 6];
        for (i, c) in chans.iter_mut().enumerate() {
            let en = if i == 0 { p[1] & 1 } else { (p[0] >> i) & 1 };
            let at = 0x14 + 4 * i;
            *c = (en != 0, u16::from_le_bytes([p[at], p[at + 1]]), u16::from_le_bytes([p[at + 2], p[at + 3]]));
        }
        Outputs { pins, chans, raw: p[..8].try_into().unwrap() }
    }

    pub fn duties(&self) -> Duties {
        let o = self.outputs();
        let used: BTreeSet<u8> = o.pins.iter().map(|(_, f)| f.as_bytes()[3] - b'0').collect();
        used.into_iter()
            .map(|i| {
                let (en, cmp, max) = o.chans[i as usize];
                (i, if en && max != 0 { py::round(100.0 * cmp as f64 / max as f64, 3) } else { 0.0 })
            })
            .collect()
    }

    pub fn settle(&mut self) -> R<Duties> {
        let t0 = self.ms();
        let mut last: Option<Duties> = None;
        let mut same = 0;
        while self.ms() - t0 < 3000.0 {
            self.run(100.0)?;
            let d = self.duties();
            same = if last.as_ref() == Some(&d) { same + 1 } else { 0 };
            last = Some(d);
            if same >= 2 && self.ms() - t0 >= 300.0 {
                break;
            }
        }
        Ok(last.unwrap_or_default())
    }

    pub fn cmd(&mut self, cluster: u16, cmd: u8, payload: &[u8]) -> R<()> {
        self.s.zcl(cluster, cmd, payload, Tx::new().ep(self.ep))?;
        Ok(())
    }

    pub fn read(&mut self, cluster: u16, attrs: &[u16]) -> R<Vec<(String, J)>> {
        let pl: Vec<u8> = attrs.iter().flat_map(|a| a.to_le_bytes()).collect();
        let f = self.s.zcl(cluster, 0x00, &pl, Tx::new().ep(self.ep).general())?;
        Ok(match f.and_then(|f| f.zcl()) {
            Some(z) if !z.specific && z.cmd == 0x01 => decode_attrs(&z.payload),
            _ => Vec::new(),
        })
    }

    pub fn move_level(&mut self, v: u8) -> R<()> {
        self.cmd(LEVEL, 0x04, &[v, 0, 0, 0, 0])
    }

    pub fn hs(&mut self, h: u8, s: u8, t: u16) -> R<()> {
        let t = t.to_le_bytes();
        self.cmd(COLOR, 0x06, &[h, s, t[0], t[1], 0, 0])
    }

    pub fn xy(&mut self, x: u16, y: u16, t: u16) -> R<()> {
        let mut p = Vec::new();
        for v in [x, y, t] {
            p.extend_from_slice(&v.to_le_bytes());
        }
        p.extend_from_slice(&[0, 0]);
        self.cmd(COLOR, 0x07, &p)
    }

    pub fn ct(&mut self, mireds: u16, t: u16) -> R<()> {
        let (m, t) = (mireds.to_le_bytes(), t.to_le_bytes());
        self.cmd(COLOR, 0x0A, &[m[0], m[1], t[0], t[1], 0, 0])
    }

    pub fn sweep<V: Copy>(&mut self, values: &[V], v_json: impl Fn(V) -> J, mut send: impl FnMut(&mut Bulb, V) -> R<()>)
                          -> R<J> {
        let mut rows = Vec::new();
        for &v in values {
            send(self, v)?;
            let d = self.settle()?;
            let mut row = vec![("v".to_string(), v_json(v))];
            row.extend(d.iter().map(|(c, x)| (format!("pwm{c}"), J::Float(*x))));
            rows.push(J::Obj(row));
        }
        Ok(J::Arr(rows))
    }
}

pub fn file_stem(p: &Path) -> String {
    p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

fn hex16(v: u16) -> String {
    format!("{v:#06x}")
}

pub fn decode_attrs(b: &[u8]) -> Vec<(String, J)> {
    let mut out: Vec<(String, J)> = Vec::new();
    let put = |k: String, v: J, out: &mut Vec<(String, J)>| match out.iter_mut().find(|(x, _)| *x == k) {
        Some(e) => e.1 = v,
        None => out.push((k, v)),
    };
    let mut i = 0;
    while i + 3 <= b.len() {
        let attr = u16::from_le_bytes([b[i], b[i + 1]]);
        let st = b[i + 2];
        i += 3;
        if st != 0 {
            put(hex16(attr), J::Str(format!("status {st:#04x}")), &mut out);
            continue;
        }
        let Some(&t) = b.get(i) else { break };
        i += 1;
        match t {
            0x42 => {
                let n = b.get(i).copied().unwrap_or(0) as usize;
                let s = &b[(i + 1).min(b.len())..(i + 1 + n).min(b.len())];
                put(hex16(attr), J::Str(String::from_utf8_lossy(s).into_owned()), &mut out);
                i += 1 + n;
            }
            0x10 | 0x18 | 0x20 | 0x30 | 0x08 if i < b.len() => {
                put(hex16(attr), J::Int(b[i] as i64), &mut out);
                i += 1;
            }
            0x09 | 0x19 | 0x21 | 0x31 if i + 2 <= b.len() => {
                put(hex16(attr), J::Int(u16::from_le_bytes([b[i], b[i + 1]]) as i64), &mut out);
                i += 2;
            }
            _ => {
                let rest: String = b[i.min(b.len())..].iter().map(|x| format!("{x:02x}")).collect();
                put(hex16(attr), J::Str(format!("type {t:#04x} {rest}")), &mut out);
                break;
            }
        }
    }
    out
}

pub fn identity(img: &[u8], name: &str) -> Vec<(String, J)> {
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut model = None;
    'scan: for i in 0..img.len() {
        for pre in [&b"ESML"[..], b"EGLO_ZM", b"EBF", b"EPIR", b"ERCU"] {
            if img[i..].starts_with(pre) {
                let st = i + pre.len();
                let n = img[st..].iter().take(24).take_while(|&&c| word(c)).count();
                if n >= 2 {
                    model = Some(String::from_utf8_lossy(&img[i..st + n]).into_owned());
                    break 'scan;
                }
            }
        }
    }
    let mut product = BTreeSet::new();
    let mut i = 0;
    while i + 4 <= img.len() {
        if img[i] == 0x60 && img[i + 1] == 0x01 && img[i + 2] != b'\n' && img[i + 3] == 0x20 {
            product.insert(0x2000u16 | img[i + 2] as u16);
            i += 4;
        } else {
            i += 1;
        }
    }
    let d = |c: u8| c.is_ascii_digit();
    let sw = img.windows(10).find(|w| {
        d(w[0]) && w[1] == b'.' && d(w[2]) && w[3] == b'.' && d(w[4]) && w[5] == b'_' && w[6..].iter().all(|&c| d(c))
    });
    let hw = hw_of(name);
    let opt = |s: Option<String>| s.map(J::Str).unwrap_or(J::Null);
    vec![
        ("model".into(), opt(model)),
        ("ble_product".into(), J::Arr(product.into_iter().map(|p| J::Str(hex16(p))).collect())),
        ("sw".into(), opt(sw.map(|w| String::from_utf8_lossy(w).into_owned()))),
        ("hw".into(), opt(hw)),
        ("size".into(), J::Int(img.len() as i64)),
    ]
}

fn rev_len(s: &[u8]) -> Option<usize> {
    if s.len() >= 3 && s[0].is_ascii_digit() && s[1] == b'.' && s[2].is_ascii_digit() {
        Some(2 + s[2..].iter().take_while(|c| c.is_ascii_digit()).count())
    } else {
        None
    }
}

fn hw_of(name: &str) -> Option<String> {
    let b = name.as_bytes();
    let key = b"jing_xun_";
    for i in 0..b.len().saturating_sub(key.len() - 1) {
        if b[i..].starts_with(key) {
            let r = &b[i + key.len()..];
            if let Some(n) = rev_len(r) {
                if r.get(n) == Some(&b'_') {
                    return Some(String::from_utf8_lossy(&r[..n]).into_owned());
                }
            }
        }
    }
    for i in 0..b.len().saturating_sub(1) {
        if b[i..].starts_with(b"__") {
            let r = &b[i + 2..];
            if rev_len(r) == Some(r.len()) {
                return Some(String::from_utf8_lossy(r).into_owned());
            }
        }
    }
    None
}

#[derive(Clone, Debug)]
pub struct Opts {
    pub cpi: i64,
    pub level_step: usize,
    pub ct_step: usize,
    pub hue_step: usize,
    pub sat_step: usize,
}

impl Default for Opts {
    fn default() -> Self {
        Opts { cpi: 3, level_step: 1, ct_step: 5, hue_step: 6, sat_step: 16 }
    }
}

fn int_j(v: usize) -> J {
    J::Int(v as i64)
}

pub fn profile(path: &Path, name: &str, a: &Opts) -> R<J> {
    let img = std::fs::read(path).map_err(|e| Stop::Error(format!("{}: {e}", path.display())))?;
    let mut prof: Vec<(String, J)> = vec![("image".into(), J::Str(name.into()))];
    prof.extend(identity(&img, name));
    let t_wall = Instant::now();
    let wall = |p: &mut Vec<(String, J)>| p.push(("wall_s".into(), J::Float(py::round(t_wall.elapsed().as_secs_f64(), 1))));
    let mut n = Bulb::new(path, a.cpi)?;
    n.run(1500.0)?;
    prof.push(("boot".into(), n.outputs().json()));
    let joined = n.join()?;
    prof.push(("joined".into(), J::Bool(joined)));
    if !joined {
        wall(&mut prof);
        return Ok(J::Obj(prof));
    }
    prof.push(("joined_after_s".into(), J::Float(py::round(n.s.t_joined / 1000.0, 1))));

    let eps = match n.s.endpoints() {
        Ok(e) => e,
        Err(Stop::Inconclusive(_)) => Vec::new(),
        Err(e) => return Err(e),
    };
    let mut endpoints = Vec::new();
    let mut light_ep = None;
    let mut has_color = false;
    for ep in eps {
        let d = match n.s.simple_desc(ep) {
            Ok(Some(d)) => d,
            Ok(None) | Err(Stop::Inconclusive(_)) => continue,
            Err(e) => return Err(e),
        };
        let list = |v: &[u16]| J::Arr(v.iter().map(|&x| J::Str(hex16(x))).collect());
        endpoints.push((
            ep.to_string(),
            obj(vec![("profile", J::Str(hex16(d.profile))), ("device", J::Str(hex16(d.device))), ("inputs", list(&d.inc)),
                     ("outputs", list(&d.outc))]),
        ));
        if light_ep.is_none() && d.profile == 0x0104 && d.inc.contains(&ON_OFF) {
            light_ep = Some(ep);
            has_color = d.inc.contains(&COLOR);
        }
    }
    prof.push(("endpoints".into(), J::Obj(endpoints)));
    let Some(ep) = light_ep else {
        wall(&mut prof);
        return Ok(J::Obj(prof));
    };
    n.ep = ep;
    prof.push(("light_endpoint".into(), J::Int(ep as i64)));
    let b1 = n.read(BASIC, &[0x0000, 0x0001, 0x0002, 0x0003, 0x0004, 0x0005, 0x0006, 0x0007])?;
    let b2 = n.read(BASIC, &[0x4000])?;
    prof.push(("basic".into(), J::Obj(merge(b1, b2))));
    let mut caps = 0;
    let mut color = J::Null;
    if has_color {
        let c1 = n.read(COLOR, &[0x400A, 0x400B, 0x400C, 0x0008, 0x4001])?;
        let c2 = n.read(COLOR, &[0x0010, 0x0011, 0x0012, 0x0013, 0x0015, 0x0016])?;
        color = J::Obj(merge(c1, c2));
        if let J::Int(c) = color.get("0x400a") {
            caps = *c;
        }
        prof.push(("color".into(), color.clone()));
    }

    n.cmd(ON_OFF, 0x01, &[])?;
    let on = n.settle()?;
    prof.push(("on".into(), duties_json(&on)));
    prof.push(("outputs_on".into(), n.outputs().json()));

    let mut levels: Vec<usize> = (1..255).step_by(a.level_step).collect();
    if 253 % a.level_step != 0 {
        levels.push(254);
    }
    let lv = n.sweep(&levels, int_j, |n, v| n.move_level(v as u8))?;
    prof.push(("level_sweep".into(), lv));
    n.move_level(254)?;
    n.settle()?;
    if caps & 0x10 != 0 {
        let lim = |k: &str| match color.get(k) {
            J::Int(v) if *v != 0 => Some(*v as usize),
            _ => None,
        };
        let lo = lim("0x400b").unwrap_or(153);
        let hi = lim("0x400c").unwrap_or(454);
        let mut mireds: BTreeSet<usize> = (lo..hi + 1).step_by(a.ct_step).collect();
        mireds.insert(hi);
        let mireds: Vec<usize> = mireds.into_iter().collect();
        let r = n.sweep(&mireds, int_j, |n, m| n.ct(m as u16, 0))?;
        prof.push(("ct_sweep".into(), r));
    }
    if caps & 0x01 != 0 {
        let hues: Vec<usize> = (0..255).step_by(a.hue_step).collect();
        let r = n.sweep(&hues, int_j, |n, h| n.hs(h as u8, 254, 0))?;
        prof.push(("hue_sweep".into(), r));
        let mut sats: Vec<usize> = (0..255).step_by(a.sat_step).collect();
        sats.push(254);
        let r = n.sweep(&sats, int_j, |n, s| n.hs(0, s as u8, 0))?;
        prof.push(("sat_sweep".into(), r));
        n.hs(0, 254, 0)?;
        n.settle()?;
        let mut lv: Vec<usize> = (1..255).step_by(16).collect();
        lv.push(254);
        let r = n.sweep(&lv, int_j, |n, v| n.move_level(v as u8))?;
        prof.push(("level_sweep_red".into(), r));
    }
    if caps & 0x08 != 0 {
        const XY: [(&str, f64, f64); 4] = [("red", 0.64, 0.33), ("green", 0.30, 0.60), ("blue", 0.15, 0.06), ("d65", 0.3127, 0.3290)];
        n.move_level(254)?;
        n.settle()?;
        let idx: Vec<usize> = (0..XY.len()).collect();
        let q = |f: f64| (f * 65535.0).round_ties_even() as u16;
        let r = n.sweep(&idx, |i| J::Str(XY[i].0.into()), |n, i| n.xy(q(XY[i].1), q(XY[i].2), 0))?;
        prof.push(("xy".into(), r));
    }
    n.cmd(ON_OFF, 0x00, &[])?;
    let off = n.settle()?;
    prof.push(("off".into(), duties_json(&off)));
    prof.push(("resets".into(), J::Int(n.s.w.resets as i64)));
    prof.push(("sim_s".into(), J::Float(py::round(n.ms() / 1000.0, 1))));
    wall(&mut prof);
    Ok(J::Obj(prof))
}

pub fn summary(p: &J) -> String {
    let o = if p.get("outputs_on").truthy() { p.get("outputs_on") } else { p.get("boot") };
    let pins: Vec<String> = o.get("pins").arr().iter().map(|x| format!("{}={}", x.idx(0).py_str(), x.idx(1).py_str())).collect();
    let pins = if pins.is_empty() { "-".to_string() } else { pins.join(", ") };
    let ep = p.get("light_endpoint");
    let dev = if ep.truthy() {
        let d = p.get("endpoints").get(&ep.py_str()).get("device");
        if d.is_null() { "-".into() } else { d.py_str() }
    } else {
        "-".into()
    };
    let c = p.get("color");
    let caps = if c.get("0x400a").is_null() { "-".into() } else { c.get("0x400a").py_str() };
    let ctr = if c.get("0x400b").is_null() { "-".into() } else { format!("{}..{}", c.get("0x400b").py_str(), c.get("0x400c").py_str()) };
    let prods: Vec<String> = p.get("ble_product").arr().iter().map(|x| x.py_str()).collect();
    format!(
        "| {} | {} | {} | {} | {} | {dev} | {caps} | {ctr} | {pins} |",
        p.get("image").py_str(),
        p.get("model").py_str(),
        prods.join(","),
        p.get("hw").py_str(),
        if p.get("joined").truthy() { "yes" } else { "NO" }
    )
}

pub fn dump(j: &J, indent: usize) -> String {
    let mut s = String::new();
    dump_into(j, indent, 0, &mut s);
    s
}

fn dump_into(j: &J, indent: usize, level: usize, s: &mut String) {
    let pad = |n: usize| " ".repeat(indent * n);
    match j {
        J::Arr(v) if !v.is_empty() => {
            s.push_str("[\n");
            for (k, x) in v.iter().enumerate() {
                s.push_str(&pad(level + 1));
                dump_into(x, indent, level + 1, s);
                s.push_str(if k + 1 < v.len() { ",\n" } else { "\n" });
            }
            s.push_str(&pad(level));
            s.push(']');
        }
        J::Obj(v) if !v.is_empty() => {
            s.push_str("{\n");
            for (k, (key, x)) in v.iter().enumerate() {
                s.push_str(&pad(level + 1));
                s.push_str(&dump_str(key));
                s.push_str(": ");
                dump_into(x, indent, level + 1, s);
                s.push_str(if k + 1 < v.len() { ",\n" } else { "\n" });
            }
            s.push_str(&pad(level));
            s.push('}');
        }
        _ => s.push_str(&j.dumps()),
    }
}

pub fn repo_root() -> PathBuf {
    crate::runner::repo_root()
}

pub fn stock_images() -> Result<Vec<PathBuf>, String> {
    let dir = repo_root().join("stock/apk/smartlightz");
    let mut v: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e} (run tools/extract-stock-images.sh)", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "bin"))
        .collect();
    if v.is_empty() {
        return Err(format!("no images in {} (run tools/extract-stock-images.sh)", dir.display()));
    }
    v.sort();
    Ok(v)
}

pub fn pool<T: Sync, U: Send>(items: &[T], jobs: usize, f: impl Fn(&T) -> U + Sync, mut done: impl FnMut(usize, &T, &U))
                              -> Vec<U> {
    let q = Arc::new(Mutex::new((0..items.len()).collect::<VecDeque<usize>>()));
    let (tx, rx) = mpsc::channel();
    let mut out: Vec<Option<U>> = (0..items.len()).map(|_| None).collect();
    std::thread::scope(|sc| {
        for _ in 0..jobs.max(1).min(items.len().max(1)) {
            let (q, tx, f) = (q.clone(), tx.clone(), &f);
            sc.spawn(move || loop {
                let Some(i) = q.lock().unwrap().pop_front() else { break };
                let _ = tx.send((i, f(&items[i])));
            });
        }
        drop(tx);
        for (i, u) in rx.iter() {
            done(i, &items[i], &u);
            out[i] = Some(u);
        }
    });
    out.into_iter().map(|u| u.unwrap()).collect()
}

pub fn default_jobs() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
}

pub fn boards() -> Result<Vec<(String, bool, String)>, String> {
    let p = repo_root().join("crates/bulb-tc32/boards.tsv");
    let t = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(t.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            (f[0].to_string(), f.get(1) == Some(&"1"), f.get(2).unwrap_or(&"").to_string())
        })
        .collect())
}

pub fn image_dir(name: &str) -> PathBuf {
    let d = match name {
        "ESMLFzm_w6_Dimm-4.63" => "tc32".to_string(),
        "ESMLFzm_w6_TW-4.63" => "tc32-tw".to_string(),
        _ => format!("tc32-{}", name.to_lowercase()),
    };
    repo_root().join("target").join(d)
}

pub fn stock_of(name: &str) -> Result<J, String> {
    let (model, rev) = name.rsplit_once('-').ok_or(format!("board name {name}: model-rev expected"))?;
    let p = repo_root().join(format!("analysis/profiles/{model}__{rev}.json"));
    let t = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    crate::sims::json::parse(&t)
}

pub type Row = (String, bool, String);

fn rows_close(ours: &[&J], stock: &[&J], period: f64, key: &str) -> (f64, Option<String>) {
    let mut worst = (0.0, None);
    for r in ours {
        let v = r.get("v");
        let Some(t) = stock.iter().rev().find(|t| t.get("v") == v) else {
            return (f64::INFINITY, Some(format!("{key} {} not in the stock sweep", v.py_str())));
        };
        let mut keys: BTreeSet<&str> = BTreeSet::new();
        for x in [*r, *t] {
            if let J::Obj(o) = x {
                keys.extend(o.iter().map(|(k, _)| k.as_str()).filter(|k| k.starts_with("pwm")));
            }
        }
        for k in keys {
            let d = (r.get(k).num().unwrap_or(0.0) - t.get(k).num().unwrap_or(0.0)).abs() * period / 100.0;
            if d > worst.0 {
                worst = (d, Some(format!("{key} {}: {k} ours {} stock {}", v.py_str(), r.get(k).py_str(), t.get(k).py_str())));
            }
        }
    }
    worst
}

fn diff_text(d: f64, where_: Option<String>) -> String {
    format!("largest difference {d:.2} PWM steps") + &where_.map(|w| format!(" ({w})")).unwrap_or_default()
}

fn pins_of(p: &J) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> =
        p.get("outputs_on").get("pins").arr().iter().map(|x| (x.idx(0).py_str(), x.idx(1).py_str())).collect();
    v.sort();
    v
}

fn pins_repr(v: &[(String, String)]) -> String {
    py::list(v.iter().map(|(a, b)| format!("({}, {})", py::str_repr(a), py::str_repr(b))))
}

pub fn compare(ours: &J, stock: &J) -> Vec<Row> {
    let mut res: Vec<Row> = Vec::new();
    let mut add = |what: &str, ok: bool, detail: String| res.push((what.to_string(), ok, detail));
    if !ours.get("joined").truthy() {
        add("joins", false, String::new());
        return res;
    }
    let (pins, spins) = (pins_of(ours), pins_of(stock));
    add("pins and channels", pins == spins, format!("ours {} stock {}", pins_repr(&pins), pins_repr(&spins)));
    let used: BTreeSet<usize> = spins.iter().map(|(_, f)| (f.as_bytes()[3] - b'0') as usize).collect();
    let max_of = |p: &J| -> BTreeSet<i64> {
        used.iter().filter_map(|&c| p.get("outputs_on").get("chans").idx(c).get("max").num().map(|x| x as i64)).collect()
    };
    let (per, sper) = (max_of(ours), max_of(stock));
    add("PWM period", per == sper, format!("ours {} stock {}", py::ints(per.iter().copied()), py::ints(sper.iter().copied())));
    let period = sper.iter().max().copied().unwrap_or(0) as f64;
    let tol = 1.0 + period * 0.001 / 100.0 * 2.0;
    let rows = |p: &J, k: &str| -> Vec<J> { p.get(k).arr().to_vec() };
    let (ol, sl) = (rows(ours, "level_sweep"), rows(stock, "level_sweep"));
    let (d, w) = rows_close(&ol.iter().skip(1).collect::<Vec<_>>(), &sl.iter().skip(1).collect::<Vec<_>>(), period, "level");
    add("level 2..254", d <= tol, diff_text(d, w));
    add(
        "(shown) level 1 with On/Off: off by ZCL 8",
        true,
        format!("ours {} stock {}", ol.first().map(|r| r.py_repr()).unwrap_or("None".into()),
                sl.first().map(|r| r.py_repr()).unwrap_or("None".into())),
    );
    if !stock.get("ct_sweep").is_null() {
        let (o, s) = (rows(ours, "ct_sweep"), rows(stock, "ct_sweep"));
        let (d, w) = rows_close(&o.iter().collect::<Vec<_>>(), &s.iter().collect::<Vec<_>>(), period, "mireds");
        add("colour temperature sweep", !ours.get("ct_sweep").is_null() && d <= tol, diff_text(d, w));
        let rng = |p: &J| (p.get("color").get("0x400b").clone(), p.get("color").get("0x400c").clone());
        let (a, b) = (rng(ours), rng(stock));
        let t = |r: &(J, J)| format!("({}, {})", r.0.py_repr(), r.1.py_repr());
        add("colour temperature range", a == b, format!("ours {} stock {}", t(&a), t(&b)));
    }
    for (key, what) in [("hue_sweep", "hue at full saturation"), ("sat_sweep", "saturation at hue 0"),
                        ("level_sweep_red", "level at red"), ("xy", "the four xy points")] {
        if stock.get(key).is_null() {
            continue;
        }
        let keep = |r: &&J| !(key == "level_sweep_red" && r.get("v") == &J::Int(1));
        let (o, s) = (rows(ours, key), rows(stock, key));
        let (d, w) = rows_close(&o.iter().filter(keep).collect::<Vec<_>>(), &s.iter().filter(keep).collect::<Vec<_>>(), period, key);
        add(what, !ours.get(key).is_null() && d <= tol, diff_text(d, w));
    }
    let (h0, h1) = (ours.get("basic").get("0x0003"), stock.get("basic").get("0x0003"));
    add("HWVersion", h0 == h1, format!("ours {} stock {}", h0.py_str(), h1.py_str()));
    let son = stock.get("on");
    let keys: Vec<String> = match son {
        J::Obj(o) if !o.is_empty() => o.iter().map(|(k, _)| k.clone()).collect(),
        _ => vec![String::new()],
    };
    let on_d = keys
        .iter()
        .map(|k| (ours.get("on").get(k).num().unwrap_or(0.0) - son.get(k).num().unwrap_or(0.0)).abs())
        .fold(0.0, f64::max)
        * period
        / 100.0;
    let show = |j: &J| if j.is_null() { "None".to_string() } else { j.py_repr() };
    add("factory-new light after On", on_d <= tol, format!("ours {} stock {}", show(ours.get("on")), show(son)));
    res
}

pub fn check_board(name: &str, a: &Opts) -> (Vec<Row>, Option<J>) {
    let path = image_dir(name).join("bulb-tc32.bin");
    if !path.exists() {
        return (vec![("image".into(), false, format!("no {}", path.display()))], None);
    }
    let stock = match stock_of(name) {
        Ok(s) => s,
        Err(e) => return (vec![("stock profile".into(), false, e)], None),
    };
    match profile(&path, name, a) {
        Ok(ours) => (compare(&ours, &stock), Some(ours)),
        Err(e) => (vec![("profile".into(), false, format!("{e:?}"))], None),
    }
}

pub fn build_board(name: &str) -> Result<(), String> {
    let r = std::process::Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .current_dir(repo_root())
        .args(["xtask", "build", "--board", name])
        .output()
        .map_err(|e| format!("cargo xtask build: {e}"))?;
    if !r.status.success() {
        let tail = |b: &[u8]| {
            let s = String::from_utf8_lossy(b);
            let st = (s.len().saturating_sub(2000)..s.len()).find(|&i| s.is_char_boundary(i)).unwrap_or(s.len());
            s[st..].to_string()
        };
        return Err(format!("build of {name} failed\n{}\n{}", tail(&r.stdout), tail(&r.stderr)));
    }
    Ok(())
}

pub fn board_check(only: Option<&str>, a: &Opts, jobs: usize, out: Option<&Path>, mut say: impl FnMut(&str))
                   -> Result<Vec<(String, Vec<Row>)>, String> {
    let todo: Vec<String> = boards()?.into_iter().map(|b| b.0).filter(|n| only.map_or(true, |o| n.contains(o))).collect();
    let mut pending: Vec<Option<Vec<Row>>> = vec![None; todo.len()];
    let mut next = 0;
    let res = pool(&todo, jobs, |name| check_board(name, a), |i, name, (rows, ours)| {
        if let (Some(dir), Some(j)) = (out, ours) {
            let _ = std::fs::create_dir_all(dir);
            let _ = std::fs::write(dir.join(format!("{name}.json")), dump(j, 1));
        }
        pending[i] = Some(rows.clone());
        while next < todo.len() {
            let Some(rows) = pending[next].take() else { break };
            say(&format!("== {}", todo[next]));
            for (what, ok, detail) in &rows {
                say(&format!("  {}  {what}  {detail}", if *ok { "PASS" } else { "FAIL" }));
            }
            next += 1;
        }
    });
    Ok(todo.into_iter().zip(res.into_iter().map(|r| r.0)).collect())
}
