use super::ble_central::{hx, login_packet, unhx, Bench};
use super::json;
use super::{node_of, SimCtx, SimDef};
use crate::air::NodeId;
use crate::session::{Stop, R};

pub fn def() -> SimDef {
    SimDef { name: "ble_diag", doc: "the GATT diagnostic service of the device page", run }
}

const D: NodeId = NodeId::Dut;
const DIAG: u16 = 0x0B05;
const MFR: u16 = 0x1141;
const STOCK_HANDLES: [(u8, u16); 5] = [(0x11, 0x12), (0x12, 0x15), (0x13, 0x18), (0x15, 0x1B), (0x14, 0x1E)];
const IEEE: u64 = 0xa4c138_0000_00d1a6;
const SERVICE: &str = "e6100000-5a4b-4c3d-8e2f-1a2b3c4d5e6f";
const CHARS: [(&str, &str); 7] =
    [("0001", "info"), ("0002", "state"), ("0003", "log"), ("0004", "cmd"), ("0005", "frames"), ("0006", "config"), ("0007", "counters")];

fn u16le(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn u32le(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

#[derive(Debug)]
struct Info {
    tunable: bool,
    flags: u8,
    release: u8,
    ieee: String,
    model: String,
    firmware: String,
}

fn decode_info(b: &[u8]) -> Option<Info> {
    if b.len() < 14 || b[0] != 1 {
        return None;
    }
    let ieee = b[4..12].iter().rev().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(":");
    let mlen = b[12] as usize;
    let model = String::from_utf8_lossy(b.get(13..13 + mlen)?).into_owned();
    let flen = *b.get(13 + mlen)? as usize;
    let firmware = String::from_utf8_lossy(b.get(14 + mlen..14 + mlen + flen)?).into_owned();
    Some(Info { tunable: b[1] == 1, flags: b[2], release: b[3], ieee, model, firmware })
}

#[derive(Debug)]
struct State {
    uptime: u32,
    reset_cause: u8,
    safe_mode: bool,
    joined: bool,
    short: u16,
    pan: u16,
    channel: u8,
    lqi: u8,
    rssi: i8,
    mac_rx: u32,
    mac_tx: u32,
    on: bool,
    level: u8,
    mireds: Option<u16>,
    logged_in: bool,
    result: u8,
}

fn decode_state(b: &[u8]) -> Option<State> {
    if b.len() < 40 || b[0] != 1 {
        return None;
    }
    let ct = u16le(b, 37);
    Some(State {
        uptime: u32le(b, 1),
        reset_cause: b[5],
        safe_mode: b[8] == 1,
        joined: b[12] == 1,
        short: u16le(b, 13),
        pan: u16le(b, 15),
        channel: b[17],
        lqi: b[21],
        rssi: b[22] as i8,
        mac_rx: u32le(b, 23),
        mac_tx: u32le(b, 27),
        on: b[35] == 1,
        level: b[36],
        mireds: (ct != 0xFFFF).then_some(ct),
        logged_in: b[39] & 0x80 != 0,
        result: b[39] & 0x0F,
    })
}

fn decode_log(b: &[u8]) -> Vec<(u32, u8, u8)> {
    let n = (*b.first().unwrap_or(&0) as usize).min(b.len().saturating_sub(1) / 6);
    (0..n).map(|i| 1 + i * 6).map(|o| (u32le(b, o), b[o + 4], b[o + 5])).collect()
}

fn c_codes(log: &[(u32, u8, u8)]) -> Vec<u8> {
    log.iter().map(|e| e.1).filter(|c| (1..32).contains(c)).collect()
}

fn log_text(log: &[(u32, u8, u8)], n: usize) -> String {
    log.iter()
        .take(n)
        .map(|(t, c, a)| if *t == 0xFFFF_FFFF { format!("before: {c} ({a})") } else { format!("{t} s: {c} ({a})") })
        .collect::<Vec<_>>()
        .join("; ")
}

#[derive(Debug)]
struct Frame {
    time: u32,
    src: u16,
    cluster: u16,
    cmd: u8,
    specific: bool,
    lqi: u8,
}

fn decode_frames(b: &[u8]) -> Vec<Frame> {
    let n = (*b.first().unwrap_or(&0) as usize / 12).min(b.len().saturating_sub(1) / 12);
    (0..n)
        .map(|i| 1 + i * 12)
        .map(|o| Frame { time: u32le(b, o), src: u16le(b, o + 4), cluster: u16le(b, o + 6), cmd: b[o + 8], specific: b[o + 9] & 0x03 == 1, lqi: b[o + 10] })
        .collect()
}

#[derive(Debug)]
struct Config {
    start_up_on_off: u8,
    start_up_level: u8,
    level_options: u8,
    color_options: Option<u8>,
    start_up_mireds: u16,
    ct_min: u16,
    ct_max: u16,
}

fn decode_config(b: &[u8]) -> Option<Config> {
    if b.len() < 20 || b[0] != 1 {
        return None;
    }
    Some(Config {
        start_up_on_off: b[1],
        start_up_level: b[2],
        level_options: b[3],
        color_options: (b[4] != 0xFF).then_some(b[4]),
        start_up_mireds: u16le(b, 5),
        ct_min: u16le(b, 7),
        ct_max: u16le(b, 9),
    })
}

fn zcl_width(t: u8) -> Option<(usize, bool)> {
    match t {
        0x08 | 0x10 | 0x18 | 0x20 | 0x30 => Some((1, false)),
        0x28 => Some((1, true)),
        0x21 | 0x31 => Some((2, false)),
        0x29 => Some((2, true)),
        0x22 => Some((3, false)),
        0x23 => Some((4, false)),
        0x2b => Some((4, true)),
        _ => None,
    }
}

fn number(v: &[u8], signed: bool) -> i64 {
    let mut x = v.iter().rev().fold(0i64, |a, b| a << 8 | *b as i64);
    let bits = 8 * v.len();
    if signed && x >= 1 << (bits - 1) {
        x -= 1 << bits;
    }
    x
}

fn decode_counters(b: &[u8]) -> Option<Vec<(u16, u8, i64)>> {
    if b.first() != Some(&1) {
        return None;
    }
    let mut out = Vec::new();
    let mut o = 1;
    while o + 3 <= b.len() {
        let (id, t) = (u16le(b, o), b[o + 2]);
        let (w, signed) = zcl_width(t)?;
        let v = b.get(o + 3..o + 3 + w)?;
        out.push((id, t, number(v, signed)));
        o += 3 + w;
    }
    (o == b.len()).then_some(out)
}

mod cmd {
    pub fn identify() -> Vec<u8> {
        vec![0x01]
    }
    pub fn on_off(mode: u8) -> Vec<u8> {
        vec![0x10, mode]
    }
    pub fn level(level: u8, tenths: u16) -> Vec<u8> {
        let t = tenths.to_le_bytes();
        vec![0x11, level, t[0], t[1]]
    }
    pub fn color_temp(mireds: u16, tenths: u16) -> Vec<u8> {
        let (m, t) = (mireds.to_le_bytes(), tenths.to_le_bytes());
        vec![0x12, m[0], m[1], t[0], t[1]]
    }
    pub fn start_up_on_off(v: u8) -> Vec<u8> {
        vec![0x20, v]
    }
    pub fn start_up_level(v: u8) -> Vec<u8> {
        vec![0x21, v]
    }
    pub fn level_options(v: u8) -> Vec<u8> {
        vec![0x22, v]
    }
    pub fn color_options(v: u8) -> Vec<u8> {
        vec![0x23, v]
    }
    pub fn start_up_color_temp(mireds: u16) -> Vec<u8> {
        let m = mireds.to_le_bytes();
        vec![0x24, m[0], m[1]]
    }
}

struct S<'a> {
    cx: &'a mut SimCtx,
    b: Bench,
    mtu: usize,
    bad: Vec<String>,
    names: Vec<(u16, &'static str)>,
    raw: Vec<(&'static str, Vec<u8>)>,
}

fn none(what: &str) -> Stop {
    Stop::Error(format!("{what}: no value or not in the page's format"))
}

impl S<'_> {
    fn check(&mut self, name: &str, okv: bool, detail: &str) -> bool {
        if !okv {
            self.bad.push(name.to_string());
        }
        let t = self.b.t(D);
        self.cx.check(t, name, okv, detail);
        okv
    }

    fn run_until(&mut self, max_ms: f64, mut cond: impl FnMut(&Bench) -> bool) -> R<bool> {
        let end = self.b.t(D) + max_ms;
        while self.b.t(D) < end {
            if cond(&self.b) {
                return Ok(true);
            }
            let t = end.min(self.b.t(D) + 20.0);
            self.b.run_ms(D, t)?;
        }
        Ok(cond(&self.b))
    }

    fn run(&mut self, ms: f64) -> R<()> {
        self.run_until(ms, |_| false).map(|_| ())
    }

    fn zcl_and_wait(&mut self, send: impl FnOnce(&mut Bench), cluster: u16, kinds: &[u8], max_ms: f64) -> R<Vec<Vec<u8>>> {
        let n0 = self.b.w.coord.zcl_rsp.len();
        send(&mut self.b);
        let kinds = kinds.to_vec();
        let got = move |b: &Bench| -> Vec<Vec<u8>> {
            b.w.coord.zcl_rsp[n0..]
                .iter()
                .filter(|r| r.cluster == cluster && r.cmd.is_some_and(|c| kinds.contains(&c)))
                .map(|r| r.body.clone())
                .collect()
        };
        let g2 = got.clone();
        self.run_until(max_ms, move |b| !g2(b).is_empty())?;
        Ok(got(&self.b))
    }

    fn short(&self) -> u16 {
        self.b.w.coord.devices.iter().find(|d| d.announced).unwrap().short
    }

    fn zcl(&mut self, cluster: u16, cmd: u8, payload: &[u8]) -> R<()> {
        let short = self.short();
        let p = payload.to_vec();
        self.zcl_and_wait(move |b| { b.w.coord.zcl_cmd(&mut b.w.air, short, 1, cluster, cmd, &p); }, cluster, &[0x0B], 2000.0)?;
        Ok(())
    }

    fn connect(&mut self) -> R<Option<String>> {
        self.b.ble_connect(D, 40, None);
        let c = self.b.ble_wait_connected(D, 15000.0)?;
        self.mtu = 23;
        if c.is_some() {
            let rsp = self.att(&[0x02, 64, 0])?;
            if rsp[0] == 0x03 {
                self.mtu = 64.min(u16::from_le_bytes([rsp[1], rsp[2]]) as usize);
            }
        }
        Ok(c)
    }

    fn att(&mut self, pdu: &[u8]) -> R<Vec<u8>> {
        self.b.att_ok(D, pdu)
    }

    fn read_long(&mut self, handle: u16) -> R<Option<Vec<u8>>> {
        let mut q = vec![0x0a];
        q.extend_from_slice(&handle.to_le_bytes());
        let rsp = self.att(&q)?;
        if rsp[0] != 0x0B {
            return Ok(None);
        }
        let mut val = rsp[1..].to_vec();
        let mut chunk = rsp.len() - 1;
        while chunk == self.mtu - 1 {
            let mut q = vec![0x0c];
            q.extend_from_slice(&handle.to_le_bytes());
            q.extend_from_slice(&(val.len() as u16).to_le_bytes());
            let rsp = self.att(&q)?;
            if rsp[0] != 0x0D {
                break;
            }
            chunk = rsp.len() - 1;
            val.extend_from_slice(&rsp[1..]);
        }
        if let Some(&(_, name)) = self.names.iter().find(|(h, _)| *h == handle) {
            match self.raw.iter_mut().find(|(n, _)| *n == name) {
                Some(r) => r.1 = val.clone(),
                None => self.raw.push((name, val.clone())),
            }
        }
        Ok(Some(val))
    }

    fn read(&mut self, handle: u16, what: &str) -> R<Vec<u8>> {
        self.read_long(handle)?.ok_or_else(|| none(what))
    }

    fn state(&mut self, h: u16) -> R<State> {
        let v = self.read(h, "state")?;
        decode_state(&v).ok_or_else(|| none("state"))
    }

    fn write(&mut self, handle: u16, data: &[u8]) -> R<Vec<u8>> {
        let mut q = vec![0x12];
        q.extend_from_slice(&handle.to_le_bytes());
        q.extend_from_slice(data);
        self.att(&q)
    }

    fn command(&mut self, cmd_h: u16, state_h: u16, data: &[u8]) -> R<State> {
        self.write(cmd_h, data)?;
        self.run(300.0)?;
        self.state(state_h)
    }
}

fn read_records(body: &[u8]) -> Vec<(u16, u8, i64)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 3 <= body.len() {
        let id = u16::from_le_bytes([body[i], body[i + 1]]);
        if body[i + 2] != 0 {
            i += 3;
            continue;
        }
        let Some(&t) = body.get(i + 3) else { break };
        let Some((w, signed)) = zcl_width(t) else { break };
        let Some(v) = body.get(i + 4..i + 4 + w) else { break };
        out.push((id, t, number(v, signed)));
        i += 4 + w;
    }
    out
}

fn zigbee_diag(s: &mut S, ids: &[u16]) -> R<Vec<(u16, u8, i64)>> {
    let short = s.short();
    let mut out = Vec::new();
    for mfr in [false, true] {
        let part: Vec<u16> = ids.iter().copied().filter(|i| (*i >= 0x5000) == mfr).collect();
        for chunk in part.chunks(8) {
            let c = chunk.to_vec();
            let rsp = s.zcl_and_wait(
                move |b| {
                    if mfr {
                        b.w.coord.read_attrs_mfr(&mut b.w.air, short, 1, DIAG, MFR, &c);
                    } else {
                        b.w.coord.read_attrs(&mut b.w.air, short, 1, DIAG, &c);
                    }
                },
                DIAG,
                &[0x01],
                3000.0,
            )?;
            out.extend(rsp.iter().flat_map(|r| read_records(r)));
        }
    }
    Ok(out)
}

fn counters_check(s: &mut S, h: u16) -> R<()> {
    let first = s.read(h, "counters")?;
    let first = decode_counters(&first).ok_or_else(|| none("counters"))?;
    let ids: Vec<u16> = first.iter().map(|c| c.0).collect();
    let before = zigbee_diag(s, &ids)?;
    s.run(1200.0)?;
    let raw = s.read(h, "counters")?;
    let after = zigbee_diag(s, &ids)?;
    let list = decode_counters(&raw).ok_or_else(|| none("counters"))?;
    let (mut bad, mut same) = (Vec::new(), 0);
    for &(id, t, v) in &list {
        let a = before.iter().find(|r| r.0 == id);
        let b = after.iter().find(|r| r.0 == id);
        match (a, b) {
            (Some(a), Some(b)) if a.1 == t && b.1 == t => {
                let (lo, hi) = (a.2.min(b.2), a.2.max(b.2));
                if !(lo..=hi).contains(&v) {
                    bad.push(format!("{id:#06x} {v} not in {lo}..={hi}"));
                } else if a.2 == b.2 {
                    same += 1;
                }
            }
            _ => bad.push(format!("{id:#06x}: over Zigbee {a:?} / {b:?}, type {t:#04x} over BLE")),
        }
    }
    let n_std = list.iter().filter(|c| c.0 < 0x5000).count();
    let n_mfr = list.len() - n_std;
    let val = |id: u16| list.iter().find(|c| c.0 == id).map_or(0, |c| c.2);
    let tx = val(0x0103);
    let rate = |n: i64| if tx > 0 { n as f64 / tx as f64 } else { 0.0 };
    let ok = bad.is_empty() && n_std == 32 && n_mfr == 21 && list.len() == ids.len();
    s.check(
        "counters: every attribute of the Diagnostics cluster (32 standard, 21 manufacturer) as Zigbee reads it",
        ok,
        &format!(
            "{} B, {n_std}+{n_mfr} attributes, {same} equal to both reads, the rest between them; retry rate {:.3}, fail rate {:.3}{}",
            raw.len(),
            rate(val(0x0104)),
            rate(val(0x0105)),
            if bad.is_empty() { String::new() } else { format!("; {}", bad.join("; ")) }
        ),
    );
    Ok(())
}

fn run(cx: &mut SimCtx) -> R<()> {
    let img = cx.image("bin", "target/tc32/bulb-tc32.bin");
    let tw = cx.images.iter().any(|i| i.label == "tw");
    let mut node = node_of(&img, cx.cpi)?;
    let e = IEEE.to_le_bytes();
    node.write_flash(0xFF000, &[e[0], e[1], e[2], 0x38, 0xC1, 0xA4, e[3], e[4] & 0x7F]);
    let b = Bench::new(node, "dut", true, None);
    let mut s = S { cx, b, mtu: 23, bad: Vec::new(), names: Vec::new(), raw: Vec::new() };
    let r = body(&mut s, tw, &img);
    dump(&s, &img.path, tw);
    r?;
    if let Some(l) = super::util::stack_report(s.b.w.node(D), &img) {
        s.cx.say(&l);
    }
    finish(s)
}

fn body(s: &mut S, tw: bool, img: &crate::image::Image) -> R<()> {
    let svc: Vec<u8> = {
        let mut v = unhx(&SERVICE.replace('-', ""));
        v.reverse();
        v
    };

    s.run(1000.0)?;
    let n0 = s.b.w.coord.events.len();
    let okv = s.run_until(60000.0, move |b| b.w.coord.events[n0..].iter().any(|e| e.kind == "device-announce"))?;
    s.check("joining the Zigbee network", okv, "");
    if !okv {
        return Ok(());
    }
    let dev = s.b.w.coord.devices.iter().find(|d| d.announced).unwrap().clone();
    if let Some(l) = super::util::stack_rearm(s.b.w.node_mut(D), img) {
        s.cx.say(&l);
    }
    s.run(6000.0)?;
    s.zcl(0x0006, 0x01, &[])?;
    s.zcl(0x0008, 0x04, &[127, 0, 0])?;
    if tw {
        s.zcl(0x0300, 0x0A, &[250, 0, 0, 0])?;
    }
    s.run(1500.0)?;

    let c = s.connect()?;
    s.check("BLE connection", c.is_some(), "");

    let mut q = vec![0x06, 1, 0, 0xFF, 0xFF, 0x00, 0x28];
    q.extend_from_slice(&svc);
    let rsp = s.att(&q)?;
    let found = rsp[0] == 0x07;
    let (h0, h1) = if found { (u16::from_le_bytes([rsp[1], rsp[2]]), u16::from_le_bytes([rsp[3], rsp[4]])) } else { (0, 0) };
    s.check(
        "diagnostic service found by UUID, after the stock handles",
        found && h0 == 0x20 && h1 == 0x2e,
        &format!("handles {h0:#x}..{h1:#x}"),
    );

    let mut chars: Vec<(String, (u16, u8))> = Vec::new();
    let mut start = h0 as u32;
    while found && start <= h1 as u32 {
        let mut q = vec![0x08];
        q.extend_from_slice(&(start as u16).to_le_bytes());
        q.extend_from_slice(&h1.to_le_bytes());
        q.extend_from_slice(&0x2803u16.to_le_bytes());
        let rsp = s.att(&q)?;
        if rsp[0] != 0x09 {
            break;
        }
        let ln = rsp[1] as usize;
        let mut i = 2;
        while i < rsp.len() {
            let e = &rsp[i..(i + ln).min(rsp.len())];
            let (h, props, vh) = (u16::from_le_bytes([e[0], e[1]]), e[2], u16::from_le_bytes([e[3], e[4]]));
            let mut u = e[5..].to_vec();
            u.reverse();
            let full = hx(&u);
            let k = if full.len() == 32 && full[..4] == SERVICE[..4] && full[8..] == SERVICE.replace('-', "")[8..] {
                full[4..8].to_string()
            } else {
                full
            };
            match chars.iter_mut().find(|(x, _)| *x == k) {
                Some(x) => x.1 = (vh, props),
                None => chars.push((k, (vh, props))),
            }
            start = h as u32 + 1;
            i += ln;
        }
    }
    let want = [("0001", 0x02u8), ("0002", 0x02), ("0003", 0x02), ("0004", 0x08), ("0005", 0x02), ("0006", 0x02), ("0007", 0x02)];
    let mut got: Vec<(String, u8)> = chars.iter().map(|(k, v)| (k.clone(), v.1)).collect();
    got.sort();
    let w: Vec<(String, u8)> = want.iter().map(|(k, v)| (k.to_string(), *v)).collect();
    let shown = chars.iter().map(|(k, v)| format!("{k}: {:#x} props {:#04x}", v.0, v.1)).collect::<Vec<_>>().join(", ");
    s.check("characteristics info/state/log/frames/config/counters (read) and cmd (write)", got == w, &shown);
    let ch = |k: &str| chars.iter().find(|(x, _)| x == k).map(|(_, v)| v.0).ok_or_else(|| Stop::Error(format!("no characteristic {k}")));
    for (k, name) in CHARS {
        if let Ok(h) = ch(k) {
            s.names.push((h, name));
        }
    }

    let mut tel: Vec<(u8, u16)> = Vec::new();
    let mut start: u32 = 1;
    while start < h0 as u32 {
        let mut q = vec![0x08];
        q.extend_from_slice(&(start as u16).to_le_bytes());
        q.extend_from_slice(&(h0 - 1).to_le_bytes());
        q.extend_from_slice(&0x2803u16.to_le_bytes());
        let rsp = s.att(&q)?;
        if rsp[0] != 0x09 {
            break;
        }
        let ln = rsp[1] as usize;
        let mut i = 2;
        while i < rsp.len() {
            let e = &rsp[i..(i + ln).min(rsp.len())];
            let (h, vh) = (u16::from_le_bytes([e[0], e[1]]), u16::from_le_bytes([e[3], e[4]]));
            if e.len() - 5 == 16 {
                match tel.iter_mut().find(|(k, _)| *k == e[5]) {
                    Some(x) => x.1 = vh,
                    None => tel.push((e[5], vh)),
                }
            }
            start = h as u32 + 1;
            i += ln;
        }
    }
    let tget = |k: u8| tel.iter().find(|(x, _)| *x == k).map(|(_, v)| *v);
    let mut ts = tel.clone();
    ts.sort();
    s.check(
        "Telink mesh handles still as in stock",
        STOCK_HANDLES.iter().all(|(k, v)| tget(*k) == Some(*v)),
        &ts.iter().map(|(k, v)| format!("{k:#x}->{v:#06x}")).collect::<Vec<_>>().join(", "),
    );

    let raw = s.read(ch("0001")?, "info")?;
    let info = decode_info(&raw).ok_or_else(|| none("info"))?;
    let ieee = crate::py::hex_sep(&dev.ext.to_be_bytes(), ":");
    s.check(
        "info: model, firmware, variant, IEEE as in Zigbee",
        info.model == if tw { "ESMLFzm_w6_TW" } else { "ESMLFzm_w6_Dimm" }
            && super::util::firmware_revision_ok(&img, info.firmware.as_bytes())
            && info.tunable == tw
            && info.flags == 0x03
            && info.ieee == ieee,
        &format!(
            "model {} firmware {} tunable {} flags {:#04x} release {} IEEE {} (Zigbee: {ieee})",
            info.model, info.firmware, info.tunable, info.flags, info.release, info.ieee
        ),
    );

    let state_h = ch("0002")?;
    let raw_state = s.read(state_h, "state")?;
    let mtu = s.mtu;
    s.check("state: length 40 B read in full", raw_state.len() == 40, &format!("{} B at MTU {mtu}", raw_state.len()));
    let st = decode_state(&raw_state).ok_or_else(|| none("state"))?;
    let light = s.b.light(D);
    s.check(
        "state: Zigbee network (address, PAN, channel) matches the coordinator",
        st.joined && st.short == dev.short && st.pan == s.b.w.coord.st.pan && st.channel == 11,
        &format!("joined {} address {:#06x} PAN {:#06x} channel {}", st.joined, st.short, st.pan, st.channel),
    );
    s.check(
        "state: light matches the clusters and PWM",
        st.on == light.on && st.level == 127 && st.mireds == if tw { Some(250) } else { None },
        &format!("on {} level {} mireds {:?}", st.on, st.level, st.mireds),
    );
    let ms = s.b.t(D);
    s.check(
        "state: uptime, start cause, MAC counters",
        (st.uptime as f64 - ms / 1000.0).abs() <= 3.0
            && st.reset_cause == 0
            && !st.safe_mode
            && st.mac_rx > 0
            && st.mac_tx > 0
            && st.lqi > 0
            && st.rssi < 0,
        &format!("uptime={} s at {:.1} s, mac rx {} tx {}, lqi={} rssi={}", st.uptime, ms / 1000.0, st.mac_rx, st.mac_tx, st.lqi, st.rssi),
    );

    let log = decode_log(&s.read(ch("0003")?, "log")?);
    let codes = c_codes(&log);
    s.check(
        "log: start, joining Zigbee, BLE connection (newest first)",
        codes.len() >= 3 && codes[..3] == [8, 5, 1] && log.iter().any(|e| e.1 == 5 && e.2 == 11),
        &log_text(&log, log.len()),
    );

    let frames = decode_frames(&s.read(ch("0005")?, "frames")?);
    let mut want: Vec<(u16, u8)> = if tw { vec![(0x0300, 0x0A)] } else { vec![] };
    want.extend([(0x0008, 0x04), (0x0006, 0x01)]);
    let good = frames.len() >= want.len()
        && frames.iter().zip(&want).all(|(f, w)| (f.cluster, f.cmd) == *w && f.specific && f.src == 0 && f.lqi > 0);
    s.check(
        "frames: Zigbee commands from before the connection, newest first, with signal",
        good,
        &frames
            .iter()
            .map(|f| format!("{} s {:#06x}/{:#04x}{} from {:#06x} LQI {}", f.time, f.cluster, f.cmd, if f.specific { "" } else { " (general)" }, f.src, f.lqi))
            .collect::<Vec<_>>()
            .join("; "),
    );

    let cfg_h = ch("0006")?;
    let cfg = decode_config(&s.read(cfg_h, "config")?).ok_or_else(|| none("config"))?;
    let tw_ok = if tw { cfg.ct_min == 153 && cfg.ct_max == 454 && cfg.color_options == Some(1) } else { cfg.color_options.is_none() };
    let short = dev.short;
    let mut startup = Vec::new();
    for (cluster, attr) in [(0x0006u16, 0x4003u16), (0x0008, 0x4000)] {
        let r = s.zcl_and_wait(move |b| { b.w.coord.read_attrs(&mut b.w.air, short, 1, cluster, &[attr]); }, cluster, &[0x01], 3000.0)?;
        startup.push(r.first().and_then(|z| z.last().copied()).map_or(-1, |v| v as i32));
    }
    s.check(
        "config: settings as in the clusters",
        cfg.start_up_on_off as i32 == startup[0] && cfg.start_up_level as i32 == startup[1] && tw_ok,
        &format!("{cfg:?} (over Zigbee: StartUpOnOff {}, StartUpCurrentLevel {})", startup[0], startup[1]),
    );

    let cmd_h = ch("0004")?;
    let st1 = s.command(cmd_h, state_h, &cmd::on_off(0))?;
    let on = s.b.light(D).on;
    s.check("control without login rejected", st1.result == 1 && !st1.logged_in && st1.on && on, &format!("result {}", st1.result));
    let h14 = tget(0x14).ok_or_else(|| Stop::Error("no stock characteristic 0x14".into()))?;
    let rd14 = [0x0a, h14 as u8, (h14 >> 8) as u8];
    let rnd = [1, 2, 3, 4, 5, 6, 7, 8];
    s.write(h14, &login_packet(b"unpaired", b"bad", &rnd))?;
    let bad = s.att(&rd14)?;
    s.write(h14, &login_packet(b"unpaired", b"1234", &rnd))?;
    let good = s.att(&rd14)?;
    let st1 = s.state(state_h)?;
    s.check(
        "mesh login (wrong password rejected, good one accepted)",
        bad[1] == 0x0E && good[1] == 0x0D && st1.logged_in,
        &format!("wrong -> {:#04x}, good -> {:#04x}", bad[1], good[1]),
    );
    let st1 = s.command(cmd_h, state_h, &cmd::on_off(0))?;
    let on = s.b.light(D).on;
    s.check("after login: turn off", st1.result == 0 && !st1.on && !on, "");
    s.command(cmd_h, state_h, &cmd::level(60, 0))?;
    s.run(500.0)?;
    let st1 = s.state(state_h)?;
    let on = s.b.light(D).on;
    s.check("level 60 turns the light on", st1.on && st1.level == 60 && on, &format!("on {} level {}", st1.on, st1.level));
    if tw {
        s.command(cmd_h, state_h, &cmd::color_temp(400, 0))?;
        s.run(500.0)?;
        let st1 = s.state(state_h)?;
        let lt = s.b.light(D);
        s.check(
            "colour temperature 400 mireds: more of the warm channel",
            st1.mireds == Some(400) && lt.duty0 > 3.0 * lt.duty5,
            &format!("mireds {:?} PWM0 {:.2}% PWM5 {:.2}%", st1.mireds, lt.duty0, lt.duty5),
        );
        let bad_ct = s.command(cmd_h, state_h, &cmd::color_temp(100, 0))?;
        s.check(
            "colour temperature out of range rejected",
            bad_ct.result == 2 && bad_ct.mireds == Some(400),
            &format!("result {} mireds {:?}", bad_ct.result, bad_ct.mireds),
        );
    }
    s.command(cmd_h, state_h, &cmd::start_up_on_off(255))?;
    s.command(cmd_h, state_h, &cmd::start_up_level(80))?;
    s.command(cmd_h, state_h, &cmd::level_options(1))?;
    if tw {
        s.command(cmd_h, state_h, &cmd::color_options(0))?;
        s.command(cmd_h, state_h, &cmd::start_up_color_temp(300))?;
    }
    let cfg = decode_config(&s.read(cfg_h, "config")?).ok_or_else(|| none("config"))?;
    let tw_ok = if tw { cfg.color_options == Some(0) && cfg.start_up_mireds == 300 } else { true };
    s.check(
        "settings saved and visible in config",
        cfg.start_up_on_off == 255 && cfg.start_up_level == 80 && cfg.level_options & 0x01 != 0 && tw_ok,
        &format!("{cfg:?}"),
    );
    let zrd = s.zcl_and_wait(move |b| { b.w.coord.read_attrs(&mut b.w.air, short, 1, 0x0006, &[0x4003]); }, 0x0006, &[0x01], 3000.0)?;
    let mut good = !zrd.is_empty() && zrd[0].last() == Some(&0xFF);
    if good {
        let f = decode_frames(&s.read(ch("0005")?, "frames")?);
        good = f.first().is_some_and(|f| (f.cluster, f.cmd, f.specific) == (0x0006, 0x00, false));
    }
    s.check(
        "the same setting is visible through Zigbee (StartUpOnOff = 0xFF), and the Read Attributes frame lands in the table",
        good,
        &zrd.first().map(|z| hx(z)).unwrap_or_else(|| "no Zigbee response".into()),
    );

    let st = s.state(state_h)?;
    let light = s.b.light(D);
    s.run(2500.0)?;
    let st2 = s.state(state_h)?;
    let d = st2.uptime as i64 - st.uptime as i64;
    s.check("state refreshes every second", (2..=4).contains(&d), &format!("{} -> {} s", st.uptime, st2.uptime));

    let rsp = s.write(cmd_h, &cmd::identify())?;
    let mut seq = Vec::new();
    for _ in 0..34 {
        s.run(100.0)?;
        seq.push(s.b.light(D).on);
    }
    let toggles = (1..seq.len()).filter(|i| seq[*i] != seq[i - 1]).count();
    let now_on = s.b.light(D).on;
    s.check(
        "cmd 0x01: the bulb blinks and returns to its state",
        rsp[0] == 0x13 && toggles >= 5 && now_on == light.on,
        &format!("state changes: {toggles}"),
    );

    s.b.ble_disconnect(D, 3000.0)?;
    s.run(1500.0)?;
    let c = s.connect()?;
    s.check("BLE reconnection", c.is_some(), "");
    let log = decode_log(&s.read(ch("0003")?, "log")?);
    let codes = c_codes(&log);
    s.check(
        "log: new connection, disconnection, blink (newest first)",
        codes.len() >= 3 && codes[..3] == [8, 9, 10],
        &log_text(&log, 4),
    );
    counters_check(s, ch("0007")?)?;
    Ok(())
}

fn dump(s: &S, image: &std::path::Path, tw: bool) {
    let Some(path) = std::env::var_os("BLE_DIAG_DUMP") else { return };
    let root = crate::runner::repo_root();
    let image = image.strip_prefix(&root).unwrap_or(image.file_name().map(std::path::Path::new).unwrap_or(image));
    let dev = s.b.w.coord.devices.iter().find(|d| d.announced);
    let mut out = String::from("{\n");
    out += &format!("  \"scenario\": \"ble_diag\",\n  \"image\": {},\n  \"variant\": \"{}\",\n", json::dump_str(&image.display().to_string()), if tw { "tw" } else { "dimm" });
    if let Some(d) = dev {
        out += &format!(
            "  \"zigbee\": {{\"ieee\": \"{}\", \"short\": {}, \"pan\": {}, \"channel\": 11}},\n",
            crate::py::hex_sep(&d.ext.to_be_bytes(), ":"),
            d.short,
            s.b.w.coord.st.pan
        );
    }
    let values: Vec<String> = CHARS
        .iter()
        .filter_map(|(_, name)| s.raw.iter().find(|(n, _)| n == name))
        .map(|(n, v)| format!("    \"{n}\": \"{}\"", hx(v)))
        .collect();
    out += &format!("  \"values\": {{\n{}\n  }}\n}}\n", values.join(",\n"));
    match std::fs::write(&path, out) {
        Ok(()) => println!("ble_diag: values written to {}", std::path::Path::new(&path).display()),
        Err(e) => println!("ble_diag: BLE_DIAG_DUMP {}: {e}", std::path::Path::new(&path).display()),
    }
}

fn finish(s: S) -> R<()> {
    let n = s.cx.results.len();
    let extra = if s.bad.is_empty() {
        String::new()
    } else {
        format!("; failed: [{}]", s.bad.iter().map(|x| crate::py::str_repr(x)).collect::<Vec<_>>().join(", "))
    };
    s.cx.say(&format!("\n== {}/{n} passed{extra}", n - s.bad.len()));
    Ok(())
}
