use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use super::{emu, node_of, SimCtx, SimDef};
use crate::air::{AirFrame, Log, World};
use crate::coord::{Coord, NWK_KEY};
use crate::frames::{fmt_ext, Keys};
use crate::session::{light_of, Light, R};

pub fn def() -> SimDef {
    SimDef { name: "zigbee", doc: "one bulb joins, is interviewed and controlled (zigbee_sim.py)", run }
}

const EP: u8 = 1;

pub fn write_pcap(path: &std::path::Path, frames: &[AirFrame]) -> std::io::Result<()> {
    let mut b = Vec::new();
    for v in [0xA1B2C3D4u32] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&4u16.to_le_bytes());
    for v in [0i32 as u32, 0, 256, 195] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    for f in frames {
        let us = (f.t0 * 1000.0) as u64;
        for v in [(us / 1_000_000) as u32, (us % 1_000_000) as u32, f.psdu.len() as u32, f.psdu.len() as u32] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&f.psdu);
    }
    std::fs::write(path, b)
}

pub fn fmt_light(s: &Light) -> String {
    let mut out = format!("PWM0 {} {:5.1}% ({}/{})", if s.pwm_en { "ON " } else { "off" }, s.duty0, s.cmp, s.max);
    if s.pwm5_en {
        out += &format!(" + PWM5 {:5.1}% ({}/{}) = {:5.1}%", s.duty5, s.cmp5, s.max5, s.duty);
    }
    out
}

pub fn decode_attrs(b: &[u8]) -> String {
    let (mut out, mut i) = (Vec::new(), 0usize);
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
                out.push(format!("{attr:#06x}=type{t:#04x}:{}", hex(&b[i..])));
                break;
            }
        }
    }
    out.join(", ")
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn decode_report(b: &[u8]) -> String {
    let attr = u16::from_le_bytes([b[0], b[1]]);
    format!("attr {attr:#06x} type {:#04x} value {}", b[2], hex(&b[3..]))
}

pub struct Z {
    pub w: World,
    pub out: Rc<RefCell<String>>,
    pub results: Vec<(String, bool, String)>,
    pub resets: u32,
    pub t_wall: Instant,
}

impl Z {
    pub fn new(cx: &SimCtx, img: &crate::image::Image) -> R<Z> {
        let node = node_of(img, cx.cpi)?;
        let out = Rc::new(RefCell::new(String::new()));
        let o2 = out.clone();
        let live = cx.verbose > 0;
        let log: Log = Rc::new(move |s: &str| {
            let mut o = o2.borrow_mut();
            o.push_str(s);
            o.push('\n');
            if live {
                println!("{s}");
            }
        });
        let coord = Coord::new_plain(11, NWK_KEY);
        let mut w = World::new(node, coord, Keys::default(), Some(log));
        w.coord.start(&mut w.air);
        Ok(Z { w, out, results: Vec::new(), resets: 0, t_wall: Instant::now() })
    }
    pub fn ms(&self) -> f64 {
        self.w.now()
    }
    pub fn say(&self, s: &str) {
        let mut o = self.out.borrow_mut();
        o.push_str(s);
        o.push('\n');
    }
    pub fn run_until(&mut self, max_ms: f64, step: f64, mut cond: impl FnMut(&Z) -> bool) -> R<bool> {
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
        self.results.push((name.into(), ok, detail.into()));
        self.say(&format!("[{:9.3} ms] {}: {name} {detail}", self.ms(), if ok { "PASS" } else { "FAIL" }));
        ok
    }
    pub fn has(&self, kind: &str) -> bool {
        self.w.coord.events.iter().any(|e| e.kind == kind)
    }
    pub fn zcl_and_wait(&mut self, send: impl FnOnce(&mut Z), cluster: u16, kinds: &[u8], max_ms: f64)
                    -> R<Vec<crate::coord::ZclRsp>> {
        let n0 = self.w.coord.zcl_rsp.len();
        send(self);
        let kinds = kinds.to_vec();
        let got = move |z: &Z| -> Vec<crate::coord::ZclRsp> {
            z.w.coord.zcl_rsp[n0..].iter().filter(|r| r.cluster == cluster && r.cmd.is_some_and(|c| kinds.contains(&c))).cloned().collect()
        };
        self.run_until(max_ms, 20.0, |z| !got(z).is_empty())?;
        Ok(got(self))
    }
    pub fn zdo_wait(&mut self, cluster: u16, payload: &[u8], rsp: u16) -> R<Option<Vec<u8>>> {
        let n0 = self.w.coord.zcl_rsp.len();
        let short = self.short();
        self.w.coord.zdo_send(&mut self.w.air, short, cluster, payload);
        self.run_until(2000.0, 20.0, |z| z.w.coord.zcl_rsp[n0..].iter().any(|r| r.cluster == rsp))?;
        Ok(self.w.coord.zcl_rsp[n0..].iter().find(|r| r.cluster == rsp).map(|r| r.body.clone()))
    }
    pub fn short(&self) -> u16 {
        self.w.coord.devices.iter().find(|d| d.announced).map(|d| d.short).unwrap_or(0)
    }
}

fn run(cx: &mut SimCtx) -> R<()> {
    let img = cx.image("bin", "target/tc32/bulb-tc32.bin");
    let mut z = Z::new(cx, &img)?;
    let pcap = std::env::var("SIM_PCAP").unwrap_or_else(|_| "zigbee-sim.pcap".into());
    let name = img.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    z.say(&format!("== firmware: {name}; coordinator PAN {:#06x} channel 11; pcap: {pcap}", z.w.coord.st.pan));
    let r = body(&mut z);
    finish(&mut z, cx, &pcap);
    r
}

pub fn finish(z: &mut Z, cx: &mut SimCtx, pcap: &str) {
    let n_ok = z.results.iter().filter(|r| r.1).count();
    let (sent, recv, dropped) = {
        let rb = z.w.air.dut.radio.b();
        (rb.sent, rb.received, rb.dropped.clone())
    };
    let ndrop: i64 = dropped.values().sum();
    z.say(&format!(
        "\n== result: {n_ok}/{} steps passed; simulation time {:.1} s, wall time {:.0} s; frames: firmware {sent}, received {recv}, dropped {ndrop}",
        z.results.len(),
        z.ms() / 1000.0,
        z.t_wall.elapsed().as_secs_f64()
    ));
    for (n, ok, d) in z.results.clone() {
        z.say(&format!("  {}  {n}  {d}", if ok { "PASS" } else { "FAIL" }));
    }
    if !dropped.is_empty() {
        let why = dropped.iter().map(|(k, v)| format!("'{k}': {v}")).collect::<Vec<_>>().join(", ");
        z.say(&format!("  dropped by the firmware radio: {{{why}}}"));
    }
    let secs = sector_ranges(&z.w.air.dut.m.flash.mem).iter().map(|[a, b]| format!("{a:#x}-{:#x}", b + 0xfff)).collect::<Vec<_>>().join(", ");
    z.say(&format!("  non-empty flash sectors at the end: {secs}"));
    let p = std::path::PathBuf::from(pcap);
    let p = if p.is_relative() { crate::runner::repo_root().join(p) } else { p };
    let _ = write_pcap(&p, &z.w.air.frames);
    cx.out.push_str(&z.out.borrow());
    cx.results = z.results.clone();
}

pub fn sector_ranges(flash: &[u8]) -> Vec<[usize; 2]> {
    let mut rng: Vec<[usize; 2]> = Vec::new();
    for x in (0..0x100000).step_by(0x1000) {
        if flash[x..x + 0x1000].iter().any(|b| *b != 0xFF) {
            match rng.last_mut() {
                Some(r) if x == r[1] + 0x1000 => r[1] = x,
                _ => rng.push([x, x]),
            }
        }
    }
    rng
}

fn body(z: &mut Z) -> R<()> {
    z.idle(1000.0)?;
    let st = light_of(&z.w.air.dut);
    z.check("light after power-on", st.on, &fmt_light(&st));

    let n0 = z.w.coord.events.len();
    let ok = z.run_until(60000.0, 20.0, |z| z.w.coord.events[n0..].iter().any(|e| e.kind == "device-announce"))?;
    let d = if ok { format!("after {:.1} s of simulation", z.ms() / 1000.0) } else { String::new() };
    z.check("joining the network (Device Announce)", ok, &d);
    if !ok {
        return Ok(());
    }
    let dev = z.w.coord.devices.iter().find(|d| d.announced).cloned().unwrap();
    let short = dev.short;
    z.say(&format!("   device {} -> {short:#06x}", fmt_ext(dev.ext)));
    z.idle(6000.0)?;
    let left = z.has("leave");
    z.check("no network leave after joining", !left, "");
    if z.has("request-key") {
        let ok = z.w.coord.events.iter().any(|e| e.kind == "verify-key" && e.get("ok") == Some("True"));
        z.check("TC key exchange (Verify Key ok)", ok, "");
    }

    let r = z.zdo_wait(0x0005, &short.to_le_bytes(), 0x8005)?;
    let eps: Vec<i64> = r.as_ref().map(|p| p[5..5 + p[4] as usize].iter().map(|x| *x as i64).collect()).unwrap_or_default();
    z.check("ZDO Active Endpoints", r.is_some(), &format!("endpoints={}", crate::py::ints(eps)));
    let mut q = short.to_le_bytes().to_vec();
    q.push(EP);
    let r = z.zdo_wait(0x0004, &q, 0x8004)?;
    match r {
        Some(p) => {
            let (prof, devid) = (u16::from_le_bytes([p[6], p[7]]), u16::from_le_bytes([p[8], p[9]]));
            let nin = p[11] as usize;
            let ins: Vec<String> = (0..nin).map(|i| format!("'{:#x}'", u16::from_le_bytes([p[12 + 2 * i], p[13 + 2 * i]]))).collect();
            z.check("ZDO Simple Descriptor", true, &format!("profile={prof:#06x} device={devid:#06x} in={}", crate::py::list(ins)));
        }
        None => {
            z.check("ZDO Simple Descriptor", false, "");
        }
    }
    let rsp = z.zcl_and_wait(|z| { z.w.coord.read_attrs(&mut z.w.air, short, EP, 0x0000, &[0x0004, 0x0005, 0x4000]); }, 0x0000, &[0x01], 1500.0)?;
    let d = rsp.first().map(|r| decode_attrs(&r.body)).unwrap_or_default();
    z.check("Basic: Read Attributes (manufacturer/model/sw)", !rsp.is_empty(), &d);

    let cmd = |z: &mut Z, name: &str, cluster: u16, cmd_id: u8, payload: &[u8], expect: &dyn Fn(&Light) -> bool| -> R<()> {
        let pl = payload.to_vec();
        let rsp = z.zcl_and_wait(|z| { z.w.coord.zcl_cmd(&mut z.w.air, short, EP, cluster, cmd_id, &pl); }, cluster, &[0x0B], 1500.0)?;
        z.idle(1200.0)?;
        let st = light_of(&z.w.air.dut);
        let status = rsp.first().and_then(|r| r.body.get(1).copied());
        let ok = !rsp.is_empty() && status == Some(0) && expect(&st);
        let s = status.map(|v| v.to_string()).unwrap_or_else(|| "None".into());
        z.check(name, ok, &format!("default-rsp status={s} -> {}", fmt_light(&st)));
        Ok(())
    };
    let lvl = |l: u8| [l, 0, 0, 0, 0];
    cmd(z, "On/Off: Off", 0x0006, 0x00, &[], &|st| !st.on)?;
    cmd(z, "On/Off: On", 0x0006, 0x01, &[], &|st| st.on)?;
    cmd(z, "Level: Move to Level 127 (50%)", 0x0008, 0x04, &lvl(127), &|st| st.on && (17.0..=80.0).contains(&st.duty))?;
    cmd(z, "Level: Move to Level 254 (100%)", 0x0008, 0x04, &lvl(254), &|st| st.on && st.duty >= 95.0)?;
    cmd(z, "Level: Move to Level 10 (min)", 0x0008, 0x04, &lvl(10), &|st| st.on && st.duty < 20.0)?;
    cmd(z, "On/Off: Toggle (-> off)", 0x0006, 0x02, &[], &|st| !st.on)?;
    cmd(z, "On/Off: Toggle (-> on)", 0x0006, 0x02, &[], &|st| st.on)?;

    for (name, cl) in [("Read Attributes OnOff", 0x0006u16), ("Read Attributes CurrentLevel", 0x0008)] {
        let rsp = z.zcl_and_wait(|z| { z.w.coord.read_attrs(&mut z.w.air, short, EP, cl, &[0x0000]); }, cl, &[0x01], 1500.0)?;
        let d = rsp.first().map(|r| decode_attrs(&r.body)).unwrap_or_default();
        z.check(name, !rsp.is_empty(), &d);
    }

    let mut b = dev.ext.to_le_bytes().to_vec();
    b.push(EP);
    b.extend_from_slice(&0x0006u16.to_le_bytes());
    b.push(0x03);
    b.extend_from_slice(&z.w.coord.ext.to_le_bytes());
    b.push(0x01);
    let r = z.zdo_wait(0x0021, &b, 0x8021)?;
    let ok = r.as_ref().is_some_and(|p| p.get(1) == Some(&0x00));
    z.check("ZDO Bind OnOff -> coordinator", ok, &r.map(|p| hex(&p)).unwrap_or_default());
    let pl = [0x00, 0x00, 0x00, 0x10, 0, 0, 60, 0];
    let rsp = z.zcl_and_wait(|z| {
        z.w.coord.zcl_send_with(&mut z.w.air, short, EP, 0x0006, 0x06, &pl, crate::coord::HA, false, false, None, false);
    }, 0x0006, &[0x07], 1500.0)?;
    let ok = rsp.first().is_some_and(|r| r.body.first() == Some(&0x00));
    z.check("Configure Reporting OnOff", ok, &rsp.first().map(|r| hex(&r.body)).unwrap_or_default());
    let n0 = z.w.coord.zcl_rsp.len();
    z.w.coord.zcl_send_with(&mut z.w.air, short, EP, 0x0006, 0x00, &[], crate::coord::HA, true, true, None, false);
    let rep_of = move |z: &Z| z.w.coord.zcl_rsp[n0..].iter().find(|r| r.cluster == 0x0006 && r.cmd == Some(0x0A)).cloned();
    z.run_until(5000.0, 20.0, |z| rep_of(z).is_some())?;
    let rep = rep_of(z);
    z.check("OnOff report after state change", rep.is_some(), &rep.map(|r| decode_report(&r.body)).unwrap_or_default());

    z.idle(30000.0)?;
    let (sent, ndrop) = {
        let rb = z.w.air.dut.radio.b();
        (rb.sent, rb.dropped.values().sum::<i64>())
    };
    let ok = z.resets == 0 && !z.has("leave");
    let d = format!("resets={}, frames from firmware={sent}, dropped={ndrop}", z.resets);
    z.check("30 s on the network: no resets and no Leave", ok, &d);
    Ok(())
}
