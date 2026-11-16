use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use super::{emu, node_of, SimCtx};
use crate::air::{Log, NodeId, World};
use crate::coord::{Coord, NWK_KEY};
use crate::frames::Keys;
use crate::image::Image;
use crate::session::{light_of, Light, R};

pub const EP: u8 = 1;

#[derive(Clone)]
pub struct Out {
    pub buf: Rc<RefCell<String>>,
    pub echo: bool,
}

impl Out {
    pub fn new(echo: bool) -> Out {
        Out { buf: Rc::new(RefCell::new(String::new())), echo }
    }
    pub fn say(&self, s: &str) {
        let mut b = self.buf.borrow_mut();
        b.push_str(s);
        b.push('\n');
        if self.echo {
            println!("{s}");
        }
    }
    pub fn log(&self) -> Log {
        let o = self.clone();
        Rc::new(move |s: &str| o.say(s))
    }
    pub fn into_ctx(&self, cx: &mut SimCtx) {
        cx.out.push_str(&self.buf.borrow());
    }
}

pub struct ZSim {
    pub w: World,
    pub out: Out,
    pub results: Vec<(String, bool, String)>,
    pub resets: u32,
    pub t_wall: Instant,
}

impl ZSim {
    pub fn new(img: &Image, cpi: i64, out: Out) -> R<ZSim> {
        let node = node_of(img, cpi)?;
        let coord = Coord::new_plain(11, NWK_KEY);
        let mut w = World::new(node, coord, Keys::default(), Some(out.log()));
        w.coord.start(&mut w.air);
        Ok(ZSim { w, out, results: Vec::new(), resets: 0, t_wall: Instant::now() })
    }

    pub fn ms(&self) -> f64 {
        self.w.air.t
    }

    pub fn run_until(&mut self, max_ms: f64, step: f64, mut cond: impl FnMut(&ZSim) -> bool) -> R<bool> {
        let end = self.ms() + max_ms;
        while self.ms() < end {
            if cond(self) {
                return Ok(true);
            }
            let t = end.min(self.ms() + step);
            self.w.run_ms(t).map_err(emu)?;
            self.resets = self.w.resets;
        }
        Ok(cond(self))
    }

    pub fn idle(&mut self, ms: f64) -> R<()> {
        self.run_until(ms, 20.0, |_| false).map(|_| ())
    }

    pub fn check(&mut self, name: &str, ok: bool, detail: &str) -> bool {
        self.results.push((name.to_string(), ok, detail.to_string()));
        self.out.say(&format!("[{:9.3} ms] {}: {name} {detail}", self.ms(), if ok { "PASS" } else { "FAIL" }));
        ok
    }

    pub fn wait_event(&mut self, kind: &str, max_ms: f64) -> R<bool> {
        let n0 = self.w.coord.events.len();
        let kind = kind.to_string();
        self.run_until(max_ms, 20.0, move |s| s.w.coord.events[n0..].iter().any(|e| e.kind == kind))
    }

    pub fn zcl_and_wait(&mut self, send: impl FnOnce(&mut ZSim), cluster: u16, kinds: &[u8], max_ms: f64)
                        -> R<Vec<crate::coord::ZclRsp>> {
        let n0 = self.w.coord.zcl_rsp.len();
        send(self);
        let k = kinds.to_vec();
        let got = move |s: &ZSim| -> Vec<crate::coord::ZclRsp> {
            s.w.coord.zcl_rsp[n0..]
                .iter()
                .filter(|r| r.cluster == cluster && r.cmd.is_some_and(|c| k.contains(&c)))
                .cloned()
                .collect()
        };
        let g2 = got.clone();
        self.run_until(max_ms, 20.0, move |s| !g2(s).is_empty())?;
        Ok(got(self))
    }

    pub fn light(&self) -> Light {
        light_of(self.w.node(NodeId::Dut))
    }
}

pub fn fmt_light(s: &Light) -> String {
    let mut out = format!("PWM0 {} {:5.1}% ({}/{})", if s.pwm_en { "ON " } else { "off" }, s.duty0, s.cmp, s.max);
    if s.pwm5_en {
        out += &format!(" + PWM5 {:5.1}% ({}/{}) = {:5.1}%", s.duty5, s.cmp5, s.max5, s.duty);
    }
    out
}

pub fn decode_attrs(b: &[u8]) -> String {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 3 <= b.len() {
        let attr = u16::from_le_bytes([b[i], b[i + 1]]);
        let st = b[i + 2];
        i += 3;
        if st != 0 {
            out.push(format!("{attr:#06x}=status{st:#04x}"));
            continue;
        }
        let Some(&t) = b.get(i) else { break };
        i += 1;
        match t {
            0x42 => {
                let n = b[i] as usize;
                let s = String::from_utf8_lossy(b.get(i + 1..(i + 1 + n).min(b.len())).unwrap_or(&[])).to_string();
                out.push(format!("{attr:#06x}={}", crate::py::str_repr(&s)));
                i += 1 + n;
            }
            0x10 | 0x20 | 0x30 | 0x18 | 0x08 => {
                out.push(format!("{attr:#06x}={}", b[i]));
                i += 1;
            }
            0x21 | 0x31 | 0x19 | 0x09 => {
                out.push(format!("{attr:#06x}={}", u16::from_le_bytes([b[i], b[i + 1]])));
                i += 2;
            }
            _ => {
                let rest: String = b[i..].iter().map(|x| format!("{x:02x}")).collect();
                out.push(format!("{attr:#06x}=type{t:#04x}:{rest}"));
                break;
            }
        }
    }
    out.join(", ")
}

pub fn finish(s: &ZSim) {
    let n_ok = s.results.iter().filter(|r| r.1).count();
    let node = s.w.node(NodeId::Dut);
    let (sent, recv, why) = {
        let r = node.radio.b();
        (r.sent, r.received, r.dropped.clone())
    };
    let total: i64 = why.values().sum();
    s.out.say(&format!(
        "\n== result: {n_ok}/{} steps passed; simulation time {:.1} s, wall time {:.0} s; frames: firmware {sent}, \
         received {recv}, dropped {total}",
        s.results.len(),
        s.ms() / 1000.0,
        s.t_wall.elapsed().as_secs_f64()
    ));
    for (name, ok, d) in &s.results {
        s.out.say(&format!("  {}  {name}  {d}", if *ok { "PASS" } else { "FAIL" }));
    }
    if !why.is_empty() {
        let d = why.iter().map(|(k, v)| format!("'{k}': {v}")).collect::<Vec<_>>().join(", ");
        s.out.say(&format!("  dropped by the firmware radio: {{{d}}}"));
    }
    let mem = &node.m.flash.mem;
    let n = mem.len().min(0x100000);
    let secs: Vec<usize> = (0..n).step_by(0x1000).filter(|i| mem[*i..*i + 0x1000].iter().any(|x| *x != 0xFF)).collect();
    let mut rng: Vec<[usize; 2]> = Vec::new();
    for x in secs {
        match rng.last_mut() {
            Some(r) if x == r[1] + 0x1000 => r[1] = x,
            _ => rng.push([x, x]),
        }
    }
    let t = rng.iter().map(|[a, b]| format!("{a:#x}-{:#x}", b + 0xfff)).collect::<Vec<_>>().join(", ");
    s.out.say(&format!("  non-empty flash sectors at the end: {t}"));
}
