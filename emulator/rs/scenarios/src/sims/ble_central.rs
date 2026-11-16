use std::rc::Rc;

use zbnode::Node;

use crate::air::{AttTap, BleAir, Log, NodeId, World};
use crate::ble::mesh::{command_packet, secure_password};
use crate::coord::{Coord, NWK_KEY};
use crate::crypto::telink_aes;
use crate::frames::Keys;
use crate::session::{light_of, Light, Stop, R};

pub fn hx(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn unhx(s: &str) -> Vec<u8> {
    (0..s.len() / 2).filter_map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()).collect()
}

pub fn hex_or_empty(b: &[u8]) -> String {
    if b.is_empty() {
        "b''".into()
    } else {
        hx(b)
    }
}

fn r3(x: f64) -> f64 {
    format!("{x:.3}").parse().unwrap()
}

pub struct Bench {
    pub w: World,
    pub cluster: bool,
}

pub const QUANTUM: f64 = 0.5;

pub fn connect_central(w: &mut World, id: NodeId, interval: i64, scan_ms: Option<f64>) {
    let mut op = ble::default_options();
    op.log = None;
    op.pair = false;
    op.interval = interval;
    op.timeout = 400;
    op.max_exch = 8;
    let node = w.node_mut(id);
    op.connect_after_ms = node.m.ms();
    if let Some(s) = scan_ms {
        op.active_scan = true;
        op.connect_after_ms = node.m.ms() + s;
    }
    let q = zbnode::bleatt::new_queue();
    op.att_script = Some(q.clone());
    let c = ble::Central::new(&mut node.m, &node.ble_radio, op);
    let tap = w.tap_mut(id);
    match tap {
        Some(t) => t.attach(q, c),
        None => *tap = Some(AttTap::new(q, c)),
    }
}

impl Bench {
    pub fn new(first: Node, name: &str, coord: bool, log: Option<Log>) -> Bench {
        let c = Coord::new_plain(11, NWK_KEY);
        let mut w = World::new(first, c, Keys::default(), log);
        w.air.dut_name = name.to_string();
        if coord {
            w.coord.start(&mut w.air);
        } else {
            w.air.nodes.retain(|n| *n != NodeId::Coord);
        }
        Bench { w, cluster: false }
    }

    pub fn cluster(&mut self) {
        self.w.air.ble_air = Some(BleAir::default());
        self.cluster = true;
    }

    pub fn t(&self, id: NodeId) -> f64 {
        self.w.bulb_t(id)
    }

    pub fn run_ms(&mut self, id: NodeId, target: f64) -> R<()> {
        let r = if self.cluster {
            self.w.lockstep_from(id, target, QUANTUM)
        } else {
            match id {
                NodeId::Dut => self.w.run_ms(target),
                NodeId::Bulb(i) => self.w.run_bulb(i as usize, target),
                _ => unreachable!(),
            }
        };
        r.map_err(Stop::Error)
    }

    pub fn run(&mut self, id: NodeId, ms: f64) -> R<()> {
        let t = self.t(id) + ms;
        self.run_ms(id, t)
    }

    pub fn node(&self, id: NodeId) -> &Node {
        self.w.node(id)
    }

    pub fn light(&self, id: NodeId) -> Light {
        light_of(self.w.node(id))
    }

    pub fn resets(&self, id: NodeId) -> u32 {
        self.w.bulb_resets(id)
    }

    pub fn flash(&self, id: NodeId, a: usize, n: usize) -> Vec<u8> {
        self.w.node(id).m.flash.mem[a..a + n].to_vec()
    }

    pub fn mem(&self, id: NodeId, a: usize, n: usize) -> Vec<u8> {
        let m = &self.w.node(id).m;
        if a >= 0x840000 && a + n <= 0x840000 + m.sram.len() {
            m.sram[a - 0x840000..a - 0x840000 + n].to_vec()
        } else if a >= 0x800000 && a + n <= 0x800000 + m.regs.len() {
            m.regs[a - 0x800000..a - 0x800000 + n].to_vec()
        } else {
            m.flash.mem[a..a + n].to_vec()
        }
    }

    pub fn write_flash(&mut self, id: NodeId, a: usize, data: &[u8]) {
        self.w.node_mut(id).write_flash(a, data);
    }

    pub fn ble_connect(&mut self, id: NodeId, interval: i64, scan_ms: Option<f64>) {
        connect_central(&mut self.w, id, interval, scan_ms);
    }

    pub fn ble_state(&self, id: NodeId) -> (String, String, bool) {
        match self.w.tap(id) {
            None => ("none".into(), "-".into(), false),
            Some(t) => {
                let c = t.c.borrow();
                let adv = if c.adv_a.is_empty() { "-".to_string() } else { hx(&c.adv_a) };
                (c.state.to_string(), adv, zbnode::bleatt::started(&t.q))
            }
        }
    }

    pub fn ble_wait_connected(&mut self, id: NodeId, timeout_ms: f64) -> R<Option<String>> {
        let end = self.t(id) + timeout_ms;
        while self.t(id) < end {
            let (_, adva, started) = self.ble_state(id);
            if started {
                return Ok(Some(adva));
            }
            let t = self.t(id) + 20.0;
            self.run_ms(id, t)?;
        }
        Ok(None)
    }

    pub fn ble_disconnect(&mut self, id: NodeId, wait_ms: f64) -> R<String> {
        if let Some(t) = self.w.tap(id) {
            t.c.borrow_mut().terminate(0x13);
        }
        let end = self.t(id) + wait_ms;
        while self.t(id) < end {
            let t = self.t(id) + 20.0;
            self.run_ms(id, t)?;
            let st = self.ble_state(id).0;
            if st.starts_with("terminat") || st == "failed" || st == "none" {
                return Ok(st);
            }
        }
        Ok(self.ble_state(id).0)
    }

    pub fn ble_adv(&self, id: NodeId) -> Vec<u8> {
        match self.w.tap(id) {
            Some(t) if t.c.borrow().adv_pdu.len() >= 8 => t.c.borrow().adv_pdu[8..].to_vec(),
            _ => Vec::new(),
        }
    }

    pub fn ble_stats(&self, id: NodeId) -> BleStats {
        let st = self.w.tap(id).map(|t| t.c.borrow().stats).unwrap_or_default();
        let br = self.w.node(id).ble_radio.b();
        BleStats {
            events: st.events,
            replies: st.replies,
            missed: st.missed,
            retx: st.retx,
            late: br.late_starts.len() as i64,
            misses: br.misses.len() as i64,
            advfifo: br.adv_fifo.len() as i64,
            refused: br.rf_refused.len() as i64,
            cmds: br.cmds.len() as i64,
            scanreqs: br.scan_reqs.len() as i64,
        }
    }

    pub fn ble_rf(&self, id: NodeId, since_ms: f64) -> (Vec<(f64, u8, u8)>, Vec<(f64, u8, Vec<u8>)>) {
        let br = self.w.node(id).ble_radio.b();
        (
            br.cmds.iter().filter(|c| c.ms >= since_ms).map(|c| (r3(c.ms), c.cmd, c.ch)).collect(),
            br.adv_tx.iter().filter(|a| a.ms >= since_ms).map(|a| (r3(a.ms), a.ch, a.pdu.clone())).collect(),
        )
    }

    pub fn ble_crc_errors(&mut self, id: NodeId, every: i64) -> i64 {
        let n = self.w.node(id);
        n.crc_err_every.set(every);
        n.crc_errs.get()
    }

    pub fn ble_misses(&self, id: NodeId) -> Vec<String> {
        let br = self.w.node(id).ble_radio.b();
        br.misses[br.misses.len().saturating_sub(8)..]
            .iter()
            .map(|x| {
                let line = format!("{} ch{} {}", ble::gofmt::f(x.ms, 3), x.ch, x.why);
                line.split_whitespace().collect::<Vec<_>>().join(" ")
            })
            .collect()
    }

    pub fn ble_air_stats(&self, id: NodeId) -> String {
        let br = self.w.node(id).ble_radio.b();
        let mut s = format!("ok={} miss={}", br.air_rx_ok, br.air_rx_miss.len());
        for x in &br.air_rx_miss[br.air_rx_miss.len().saturating_sub(6)..] {
            s += &format!(" [{} ch{} {}]", ble::gofmt::f(x.ms, 3), x.ch, x.why.replace(' ', "_"));
        }
        s
    }

    pub fn att(&mut self, id: NodeId, pdu: &[u8], timeout_ms: f64, step_ms: f64) -> R<Result<Vec<u8>, String>> {
        let n0 = self.w.tap(id).map(|t| t.rsps.len()).unwrap_or(0);
        {
            let tap = self.w.tap(id).expect("ATT without BLECONNECT");
            let (q, c) = (tap.q.clone(), tap.c.clone());
            let node = self.w.node_mut(id);
            zbnode::bleatt::push(&q, &c, &mut node.m, pdu.to_vec());
        }
        let end = self.t(id) + timeout_ms;
        while self.w.tap(id).unwrap().rsps.len() == n0 && self.t(id) < end {
            let t = self.t(id) + step_ms;
            self.run_ms(id, t)?;
        }
        let tap = self.w.tap(id).unwrap();
        if tap.rsps.len() == n0 {
            return Ok(Err(format!("no ATT response to {}", hx(pdu))));
        }
        Ok(Ok(tap.rsps[n0].1.clone()))
    }

    pub fn att_ok(&mut self, id: NodeId, pdu: &[u8]) -> R<Vec<u8>> {
        self.att(id, pdu, 3000.0, 2.0)?.map_err(|e| Stop::Error(format!("TimeoutError: {e}")))
    }

    pub fn ntfs_len(&self, id: NodeId) -> usize {
        self.w.tap(id).map(|t| t.ntfs.len()).unwrap_or(0)
    }

    pub fn ntfs(&self, id: NodeId) -> Vec<(f64, i64, String)> {
        self.w.tap(id).map(|t| t.ntfs.clone()).unwrap_or_default()
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct BleStats {
    pub events: i64,
    pub replies: i64,
    pub missed: i64,
    pub retx: i64,
    pub late: i64,
    pub misses: i64,
    pub advfifo: i64,
    pub refused: i64,
    pub cmds: i64,
    pub scanreqs: i64,
}

#[derive(Clone, Debug)]
pub enum TrErr {
    Ota(String),
    Timeout(String),
}

impl TrErr {
    pub fn text(&self) -> String {
        match self {
            TrErr::Ota(s) | TrErr::Timeout(s) => s.clone(),
        }
    }
}

pub type TrR<T> = R<Result<T, TrErr>>;

pub struct Transport {
    pub id: NodeId,
    pub h: Vec<(u8, u16)>,
    pub writes: usize,
    pub reads: usize,
}

impl Transport {
    pub fn new(id: NodeId, h: &[(u8, u16)]) -> Transport {
        Transport { id, h: h.to_vec(), writes: 0, reads: 0 }
    }

    fn handle(&self, ch: u8) -> u16 {
        self.h.iter().find(|(c, _)| *c == ch).map(|(_, v)| *v).expect("characteristic not discovered")
    }

    pub fn write(&mut self, b: &mut Bench, ch: u8, data: &[u8]) -> TrR<()> {
        let mut q = vec![0x12];
        q.extend_from_slice(&self.handle(ch).to_le_bytes());
        q.extend_from_slice(data);
        let rsp = match b.att(self.id, &q, 3000.0, 2.0)? {
            Ok(r) => r,
            Err(e) => return Ok(Err(TrErr::Timeout(e))),
        };
        self.writes += 1;
        if rsp[0] != 0x13 {
            return Ok(Err(TrErr::Ota(format!("write {ch:#x}: {}", hx(&rsp)))));
        }
        Ok(Ok(()))
    }

    pub fn read(&mut self, b: &mut Bench, ch: u8) -> TrR<Vec<u8>> {
        let mut q = vec![0x0a];
        q.extend_from_slice(&self.handle(ch).to_le_bytes());
        let rsp = match b.att(self.id, &q, 3000.0, 2.0)? {
            Ok(r) => r,
            Err(e) => return Ok(Err(TrErr::Timeout(e))),
        };
        self.reads += 1;
        if rsp[0] != 0x0B {
            return Ok(Err(TrErr::Ota(format!("read {ch:#x}: {}", hx(&rsp)))));
        }
        Ok(Ok(rsp[1..].to_vec()))
    }

    pub fn sleep(&mut self, b: &mut Bench, ms: f64) -> R<()> {
        b.run(self.id, ms)
    }
}

pub fn ok<T>(r: Result<T, TrErr>) -> R<T> {
    r.map_err(|e| match e {
        TrErr::Ota(s) => Stop::Error(format!("OtaError: {s}")),
        TrErr::Timeout(s) => Stop::Error(format!("TimeoutError: {s}")),
    })
}

#[derive(Clone, Debug)]
pub struct Chr {
    pub vh: u16,
    pub props: u8,
    pub uuid: Vec<u8>,
}

pub fn discover(b: &mut Bench, id: NodeId) -> R<(Vec<Chr>, Vec<(u8, u16)>, Vec<String>)> {
    let mut chars: Vec<Chr> = Vec::new();
    let mut start: u32 = 1;
    while start <= 0xFFFF {
        let mut q = vec![0x08];
        q.extend_from_slice(&(start as u16).to_le_bytes());
        q.extend_from_slice(&0xFFFFu16.to_le_bytes());
        q.extend_from_slice(&0x2803u16.to_le_bytes());
        let rsp = b.att_ok(id, &q)?;
        if rsp[0] != 0x09 {
            break;
        }
        let ln = rsp[1] as usize;
        let mut last = start;
        let mut i = 2;
        while i < rsp.len() {
            let e = &rsp[i..(i + ln).min(rsp.len())];
            let (h, props, vh) = (u16::from_le_bytes([e[0], e[1]]), e[2], u16::from_le_bytes([e[3], e[4]]));
            let u = e[5..].to_vec();
            match chars.iter_mut().find(|c| c.vh == vh) {
                Some(c) => {
                    c.props = props;
                    c.uuid = u;
                }
                None => chars.push(Chr { vh, props, uuid: u }),
            }
            last = h as u32;
            i += ln;
        }
        start = last + 1;
    }
    chars.sort_by_key(|c| c.vh);
    let base = [0x0d, 0x0c, 0x0b, 0x0a, 0x09, 0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01, 0x00];
    let mut tel: Vec<(u8, u16)> = Vec::new();
    let mut lines = Vec::new();
    for c in &chars {
        if c.uuid.len() == 16 && c.uuid[2..] == base {
            match tel.iter_mut().find(|(k, _)| *k == c.uuid[0]) {
                Some(e) => e.1 = c.vh,
                None => tel.push((c.uuid[0], c.vh)),
            }
        }
        let mut r = c.uuid.clone();
        r.reverse();
        lines.push(format!("   characteristic handle {:#06x} props {:#04x} uuid {}", c.vh, c.props, hx(&r)));
    }
    Ok((chars, tel, lines))
}

pub fn tel_get(tel: &[(u8, u16)], k: u8) -> Option<u16> {
    tel.iter().find(|(c, _)| *c == k).map(|(_, v)| *v)
}

pub fn tel_text(tel: &[(u8, u16)]) -> String {
    let mut t = tel.to_vec();
    t.sort();
    t.iter().map(|(k, v)| format!("{k:#x}->{v:#06x}")).collect::<Vec<_>>().join(", ")
}

pub const STATUS: u8 = 0x11;
pub const COMMAND: u8 = 0x12;
pub const OTA_OLD: u8 = 0x13;
pub const PAIR: u8 = 0x14;
pub const OTA: u8 = 0x15;

pub const RND8: [u8; 8] = [0x5a, 0x17, 0xc3, 0x8e, 0x21, 0x9b, 0x40, 0xf6];

pub fn pad16(b: &[u8]) -> [u8; 16] {
    let mut o = [0u8; 16];
    let n = b.len().min(16);
    o[..n].copy_from_slice(&b[..n]);
    o
}

pub fn crc16_modbus(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in data {
        crc ^= b as u16;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xA001 } else { crc >> 1 };
        }
    }
    crc
}

pub fn name_pass(name: &[u8], password: &[u8]) -> [u8; 16] {
    let (n, p) = (pad16(name), pad16(password));
    let mut o = [0u8; 16];
    for i in 0..16 {
        o[i] = n[i] ^ p[i];
    }
    o
}

pub fn login_packet(name: &[u8], password: &[u8], rnd8: &[u8; 8]) -> Vec<u8> {
    let enc = telink_aes(&pad16(rnd8), &name_pass(name, password));
    let mut v = vec![0x0c];
    v.extend_from_slice(rnd8);
    v.extend_from_slice(&enc[..8]);
    v
}

pub fn session_key(name: &[u8], password: &[u8], rnd8: &[u8; 8], resp8: &[u8]) -> [u8; 16] {
    let mut m = rnd8.to_vec();
    m.extend_from_slice(resp8);
    telink_aes(&name_pass(name, password), &pad16(&m))
}

pub fn secure_pw(mac6: &[u8]) -> Vec<u8> {
    secure_password(mac6).to_vec()
}

pub fn login(b: &mut Bench, t: &mut Transport, name: &[u8], password: &[u8], rnd8: &[u8; 8]) -> TrR<[u8; 16]> {
    if let Err(e) = t.write(b, PAIR, &login_packet(name, password, rnd8))? {
        return Ok(Err(e));
    }
    let rsp = match t.read(b, PAIR)? {
        Ok(r) => r,
        Err(e) => return Ok(Err(e)),
    };
    if rsp.first() != Some(&0x0D) {
        return Ok(Err(TrErr::Ota(format!("login refused: {}", hex_or_empty(&rsp)))));
    }
    Ok(Ok(session_key(name, password, rnd8, &rsp[1..9])))
}

pub fn login_line(rsp9: &[u8]) -> String {
    format!("logged in (response {})", hx(rsp9))
}

pub fn erase_packet(img_len: usize, mtu: usize) -> Vec<u8> {
    let mut b = vec![0u8; mtu - 3];
    b[2] = ((img_len / 4096 + 2) & 0xFF) as u8;
    b
}

pub fn data_packets(img: &[u8], mtu: usize) -> Vec<(usize, Vec<u8>)> {
    let d = mtu - 7;
    let n = img.len().div_ceil(d);
    (0..n)
        .map(|i| {
            let mut p = (i as u16).to_le_bytes().to_vec();
            let chunk = &img[i * d..((i + 1) * d).min(img.len())];
            p.extend_from_slice(chunk);
            p.resize(2 + d, 0xff);
            let c = crc16_modbus(&p);
            p.extend_from_slice(&c.to_le_bytes());
            (i, p)
        })
        .collect()
}

pub fn end_packet(img_len: usize, mtu: usize) -> Vec<u8> {
    let d = mtu - 7;
    let idx = img_len.div_ceil(d) as u16;
    let mut p = idx.to_le_bytes().to_vec();
    let c = crc16_modbus(&p);
    p.extend_from_slice(&c.to_le_bytes());
    p
}

pub const OP_POWER: u8 = 0xD0;
pub const OP_WHITE_BRIGHTNESS: u8 = 0xF1;
pub const OP_WHITE_TEMPERATURE: u8 = 0xF0;
pub const OP_STATUS_REQ: u8 = 0xDA;
pub const OP_STATUS_RSP: u8 = 0xDB;
pub const OP_NOTIFY: u8 = 0xDC;
pub const OP_MESH_ADDR: u8 = 0xE0;
pub const OP_MESH_RESET: u8 = 0xE3;

fn aes(key: &[u8; 16], data: &[u8]) -> [u8; 16] {
    telink_aes(key, &pad16(data))
}

fn checksum(key: &[u8; 16], nonce8: &[u8], payload: &[u8]) -> [u8; 16] {
    let mut n = nonce8.to_vec();
    n.push(payload.len() as u8);
    let mut chk = aes(key, &n);
    for blk in payload.chunks(16) {
        let b = pad16(blk);
        let x: Vec<u8> = chk.iter().zip(b.iter()).map(|(a, b)| a ^ b).collect();
        chk = aes(key, &x);
    }
    chk
}

fn crypt(key: &[u8; 16], nonce8: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut base = [0u8; 16];
    base[1..1 + nonce8.len()].copy_from_slice(nonce8);
    let mut out = Vec::new();
    for blk in payload.chunks(16) {
        let e = aes(key, &base);
        out.extend(e.iter().zip(blk.iter()).map(|(a, b)| a ^ b));
        base[0] = base[0].wrapping_add(1);
    }
    out
}

pub fn decrypt_notification(key: &[u8; 16], mac_le: &[u8], pkt: &[u8]) -> Option<Vec<u8>> {
    if pkt.len() < 20 {
        return None;
    }
    let mut nonce = mac_le[..3].to_vec();
    nonce.extend_from_slice(&pkt[..5]);
    let payload = crypt(key, &nonce, &pkt[7..]);
    let chk = checksum(key, &nonce, &payload);
    if chk[..2] == pkt[5..7] {
        let mut v = pkt[..7].to_vec();
        v.extend(payload);
        Some(v)
    } else {
        None
    }
}

#[derive(Clone, Debug)]
pub struct Status {
    pub op: u8,
    pub mesh_id: Option<u16>,
    pub mode: u8,
    pub on: bool,
    pub white_brightness: u8,
    pub white_temperature: u8,
    pub color_brightness: u8,
    pub rgb: (u8, u8, u8),
}

impl Status {
    pub fn repr(&self) -> String {
        if self.mesh_id.is_none() {
            return format!("{{'op': {}}}", self.op);
        }
        format!(
            "{{'op': {}, 'mesh_id': {}, 'mode': {}, 'on': {}, 'white_brightness': {}, 'white_temperature': {}, \
             'color_brightness': {}, 'rgb': ({}, {}, {})}}",
            self.op,
            self.mesh_id.unwrap(),
            self.mode,
            if self.on { "True" } else { "False" },
            self.white_brightness,
            self.white_temperature,
            self.color_brightness,
            self.rgb.0,
            self.rgb.1,
            self.rgb.2
        )
    }
}

pub fn parse_status(dec: &[u8]) -> Status {
    let op = dec[7];
    let mk = |mesh: u16, i: usize| Status {
        op,
        mesh_id: Some(mesh),
        mode: dec[i],
        on: dec[i] & 1 != 0,
        white_brightness: dec[i + 1],
        white_temperature: dec[i + 2],
        color_brightness: dec[i + 3],
        rgb: (dec[i + 4], dec[i + 5], dec[i + 6]),
    };
    match op {
        OP_STATUS_RSP => mk(dec[3] as u16 | (dec[4] as u16) << 8, 10),
        OP_NOTIFY => mk(dec[10] as u16 | (dec[19] as u16) << 8, 12),
        _ => Status { op, mesh_id: None, mode: 0, on: false, white_brightness: 0, white_temperature: 0, color_brightness: 0, rgb: (0, 0, 0) },
    }
}

pub fn set_mesh_packets(session_key: &[u8; 16], name: &[u8], password: &[u8], ltk: &[u8]) -> Vec<Vec<u8>> {
    let enc = |v: &[u8]| telink_aes(session_key, &pad16(v));
    let mut out = Vec::new();
    for (k, v) in [(0x04u8, name), (0x05, password), (0x06, ltk)] {
        let mut p = vec![k];
        p.extend_from_slice(&enc(v));
        out.push(p);
    }
    out
}

pub fn crc8_6c(data: &[u8]) -> u8 {
    let mut crc: u8 = 0;
    for &b in data {
        let mut r1 = b;
        for _ in 0..8 {
            let mix = (crc ^ r1) & 1;
            crc >>= 1;
            if mix != 0 {
                crc ^= 0x6c;
            }
            r1 >>= 1;
        }
    }
    crc
}

pub fn zcl_tunnel_packet(session_key: &[u8; 16], addr: u16, cluster: u16, zcl_args: &[u8]) -> Vec<u8> {
    let (hi, lo) = ((addr >> 8) as u8, addr as u8);
    let mut payload = vec![hi, lo, 0x01, cluster as u8, (cluster >> 8) as u8];
    payload.extend_from_slice(zcl_args);
    let mut body = vec![payload.len() as u8];
    body.extend_from_slice(&payload);
    let mut blk = vec![crc8_6c(&body)];
    blk.extend_from_slice(&body);
    let t = if hi == 0x80 { 1 } else { 0 };
    let mut v = vec![t];
    v.extend_from_slice(&telink_aes(session_key, &pad16(&blk)));
    v
}

pub fn zcl_onoff(key: &[u8; 16], addr: u16, on: bool) -> Vec<u8> {
    zcl_tunnel_packet(key, addr, 0x0006, &[if on { 1 } else { 0 }])
}

pub fn zcl_level(key: &[u8; 16], addr: u16, level: u8) -> Vec<u8> {
    zcl_tunnel_packet(key, addr, 0x0008, &[0x04, level, 2, 0, 0, 0, 0])
}

pub fn zcl_color_temp(key: &[u8; 16], addr: u16, mireds: u16) -> Vec<u8> {
    zcl_tunnel_packet(key, addr, 0x0300, &[0x0a, mireds as u8, (mireds >> 8) as u8, 2, 0])
}

pub struct MeshClient {
    pub mac_le: Vec<u8>,
    pub key: Option<[u8; 16]>,
    pub sn: u32,
}

impl MeshClient {
    pub fn new(mac_le: &[u8]) -> MeshClient {
        MeshClient { mac_le: mac_le.to_vec(), key: None, sn: 0x100 }
    }

    pub fn login(&mut self, b: &mut Bench, t: &mut Transport, name: &[u8], password: &[u8]) -> TrR<[u8; 16]> {
        let r = login(b, t, name, password, &RND8)?;
        if let Ok(k) = &r {
            self.key = Some(*k);
        }
        Ok(r)
    }

    pub fn send(&mut self, b: &mut Bench, t: &mut Transport, op: u8, params: &[u8], dest: u16) -> TrR<()> {
        self.sn += 1;
        let key = self.key.unwrap_or([0; 16]);
        let pkt = command_packet(&key, &self.mac_le, self.sn, dest, op, params);
        t.write(b, COMMAND, &pkt)
    }

    pub fn decrypt(&self, raw: &[u8]) -> Option<Vec<u8>> {
        decrypt_notification(&self.key.unwrap_or([0; 16]), &self.mac_le, raw)
    }

    pub fn statuses(&self, ntfs: &[(f64, i64, String)]) -> Vec<Status> {
        ntfs.iter().filter_map(|(_, _, v)| self.decrypt(&unhx(&v.replace('_', "")))).map(|d| parse_status(&d)).collect()
    }
}

pub fn fmt_light(s: &Light) -> String {
    let mut out = format!(
        "PWM0 {} {:5.1}% ({}/{})",
        if s.pwm_en { "ON " } else { "off" },
        s.duty0,
        s.cmp,
        s.max
    );
    if s.pwm5_en {
        out += &format!(" + PWM5 {:5.1}% ({}/{}) = {:5.1}%", s.duty5, s.cmp5, s.max5, s.duty);
    }
    out
}

pub fn log_none() -> Option<Log> {
    None
}

pub fn rc_log(f: impl Fn(&str) + 'static) -> Log {
    Rc::new(f)
}
