use ble::crypto::{crc8_6c, tl_aes_encrypt};
use ble::LinkState;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

static STEPS: AtomicUsize = AtomicUsize::new(0);
static EARLY: AtomicUsize = AtomicUsize::new(0);
static LOGINS: AtomicUsize = AtomicUsize::new(0);
static CMDS: AtomicUsize = AtomicUsize::new(0);
static RECEIVED: AtomicUsize = AtomicUsize::new(0);

use crate::sim::{make_image, ota_packet, Rig, COLOUR, DIMMABLE, STAGING, TUNABLE};

struct R(u64);

impl R {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
    fn byte(&mut self) -> u8 {
        self.next() as u8
    }
    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.byte()).collect()
    }
}

fn crc32(d: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for b in d {
        crc ^= *b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    crc
}

fn staged_good(staging: &[u8]) -> bool {
    let size = u32::from_le_bytes([staging[0x18], staging[0x19], staging[0x1a], staging[0x1b]]) as usize;
    if staging[9..12] != *b"NLT" || !(0x40..=STAGING).contains(&size) || staging[8] != 0x4b {
        return false;
    }
    let crc = crc32(&staging[..size - 4]);
    staging[size - 4..size] == crc.to_le_bytes()
}

fn att_write(rig: &mut Rig, handle: u16, value: &[u8], cmd: bool) {
    let mut req = vec![if cmd { 0x52 } else { 0x12 }];
    req.extend_from_slice(&handle.to_le_bytes());
    req.extend_from_slice(value);
    rig.central().send_att(&req);
}

fn stock_packet(key: &[u8; 16], format: u8, payload: &[u8]) -> Vec<u8> {
    let len = payload.len();
    let mut whole = vec![len as u8];
    whole.extend_from_slice(payload);
    let mut plain = [0u8; 16];
    plain[0] = crc8_6c(&whole);
    plain[1] = len as u8;
    let head = len.min(14);
    plain[2..2 + head].copy_from_slice(&payload[..head]);
    let mut v = vec![format];
    v.extend_from_slice(&tl_aes_encrypt(key, &plain));
    if len > 14 {
        v.extend_from_slice(&payload[14..]);
    }
    v
}

struct Case {
    rig: Rig,
    r: R,
    image: Vec<u8>,
    renamed: bool,
    block: usize,
    next: u16,
}

impl Case {
    fn ensure_connected(&mut self) {
        if !self.rig.central().connected || self.rig.state() != LinkState::Connected {
            self.rig.central().silent = false;
            self.rig.central().connected = false;
            self.rig.central().connect = true;
            self.rig.run(1500);
            assert!(self.rig.central().connected && self.rig.state() == LinkState::Connected, "the bulb takes a new connection within 1.5 s of the central asking");
        }
    }

    fn settle(&mut self) {
        self.rig.run(400);
        self.rig.central().att.clear();
    }

    fn step(&mut self) {
        let r = &mut self.r;
        let choice = [0u64, 1, 2, 3, 4, 5, 6, 7, 8, 9, 9, 9, 10, 10, 11, 12, 13][r.below(17) as usize];
        match choice {
            0 => {
                let op = r.below(0x22) as u8;
                let n = 1 + r.below(26) as usize;
                let mut p = r.bytes(n);
                p[0] = op;
                if op == 0x02 && r.below(4) != 0 {
                    p[0] = 0x0c;
                }
                self.rig.central().send_control(&p);
            }
            1 => {
                let llid = 1 + r.below(2) as u8;
                let n = r.below(28) as usize;
                let p = r.bytes(n);
                self.rig.central().send_pdu(llid, &p);
            }
            2 => {
                let ops = [0x02, 0x04, 0x06, 0x08, 0x0a, 0x0c, 0x10, 0x12, 0x52, 0x1e, 0x16, 0x18, 0x20, 0xd2, r.byte()];
                let op = ops[r.below(ops.len() as u64) as usize];
                let n = r.below(64) as usize;
                let mut p = vec![op];
                p.extend_from_slice(&r.bytes(n));
                if p.len() >= 3 && r.below(2) == 0 {
                    let h = r.below(0x32) as u16;
                    p[1..3].copy_from_slice(&h.to_le_bytes());
                }
                let cid = [4u16, 4, 4, 5, 6, r.below(0x10000) as u16][r.below(6) as usize];
                self.rig.central().send_l2cap(cid, &p);
            }
            3 => {
                if r.below(3) == 0 {
                    let mut p = vec![0x0c];
                    p.extend_from_slice(&r.bytes(16));
                    att_write(&mut self.rig, 0x1e, &p, false);
                } else {
                    self.ensure_connected();
                    self.settle();
                    if self.rig.state() == LinkState::Connected {
                        if self.rig.login(b"unpaired", b"1234") {
                            LOGINS.fetch_add(1, Relaxed);
                        }
                    }
                }
            }
            4 | 5 => {
                let Some(key) = self.rig.key else { return };
                let fmt = r.below(4) as u8;
                let n = 1 + r.below(18) as usize;
                let mut payload = r.bytes(n);
                if fmt == 2 && n >= 3 && r.below(2) == 0 {
                    payload[1] = 0x60;
                    payload[2] = 0x01;
                    let ops = [0xd0, 0xf1, 0xf0, 0xda, 0xe0];
                    payload[0] = ops[r.below(ops.len() as u64) as usize];
                }
                if fmt < 2 && n >= 6 {
                    payload[2] = 0x01;
                    payload[0] = 0xff;
                    payload[1] = 0xff;
                    let cl = [0x0006u16, 0x0008, 0x0300];
                    payload[3..5].copy_from_slice(&cl[r.below(3) as usize].to_le_bytes());
                }
                let mut v = stock_packet(&key, fmt, &payload);
                if r.below(8) == 0 {
                    let i = r.below(v.len() as u64) as usize;
                    v[i] ^= 1 << r.below(8);
                }
                att_write(&mut self.rig, 0x15, &v, r.below(2) == 0);
            }
            6 => {
                if self.rig.key.is_none() {
                    return;
                }
                let ops = [0xd0, 0xf1, 0xf0, 0xda, 0xe0, r.byte()];
                let op = ops[r.below(ops.len() as u64) as usize];
                let params = r.bytes(10);
                let dst = [0xffffu16, 0, 1, 0x33, r.below(0x10000) as u16][r.below(5) as usize];
                let sn = r.below(1 << 24) as u32;
                let mut v = self.rig.numbered_command_packet(sn, dst, op, &params);
                if r.below(6) == 0 {
                    let i = r.below(v.len() as u64) as usize;
                    v[i] ^= 1 << r.below(8);
                }
                att_write(&mut self.rig, 0x15, &v, r.below(2) == 0);
            }
            7 => {
                let h = [0x18u16, 0x1b][r.below(2) as usize];
                let mut p = [0u8; 20];
                p[2] = r.byte();
                let n = 3 + r.below(18) as usize;
                att_write(&mut self.rig, h, &p[..n], h == 0x1b);
            }
            8 => {
                let h = [0x18u16, 0x1b][r.below(2) as usize];
                let block = 1 + r.below(20) as usize;
                let index = match r.below(5) {
                    0 => r.below(0x10000) as u16,
                    1 => 0xffff,
                    _ => r.below((self.image.len() / block + 2) as u64) as u16,
                };
                let at = index as usize * block;
                let data: Vec<u8> = if at < self.image.len() && r.below(3) != 0 {
                    let mut d = self.image[at..(at + block).min(self.image.len())].to_vec();
                    d.resize(block, 0xff);
                    d
                } else {
                    r.bytes(block)
                };
                let mut p = ota_packet(index, &data);
                if r.below(10) == 0 {
                    let i = r.below(p.len() as u64) as usize;
                    p[i] ^= 1 << r.below(8);
                }
                att_write(&mut self.rig, h, &p, h == 0x1b);
            }
            9 => {
                if r.below(8) == 0 {
                    self.next = 0;
                    if r.below(2) == 0 {
                        self.block = [16usize, 16, 8, 1, 20, 3][r.below(6) as usize];
                    }
                }
                let burst = 1 + r.below(60) as usize;
                for _ in 0..burst {
                    let r = &mut self.r;
                    let block = self.block;
                    let mut index = self.next;
                    let mut adv = true;
                    match r.below(40) {
                        0 => index = index.wrapping_add(1),
                        1 => {
                            index = index.wrapping_sub(1);
                            adv = false;
                        }
                        2 => index = 0,
                        _ => {}
                    }
                    let at = index as usize * block;
                    let mut data: Vec<u8> = if at < self.image.len() {
                        let mut d = self.image[at..(at + block).min(self.image.len())].to_vec();
                        d.resize(block, 0xff);
                        d
                    } else {
                        vec![0xff; block]
                    };
                    match r.below(60) {
                        0 => {
                            let i = r.below(block as u64) as usize;
                            data[i] ^= 1 << r.below(8);
                        }
                        1 if block > 1 => data.truncate(block - 1),
                        2 => data.push(0),
                        _ => {}
                    }
                    let mut p = ota_packet(index, &data);
                    if r.below(80) == 0 {
                        let i = r.below(p.len() as u64) as usize;
                        p[i] ^= 1 << r.below(8);
                    }
                    let cmd = r.below(8) != 0;
                    att_write(&mut self.rig, 0x1b, &p, cmd);
                    if adv {
                        self.next = index.wrapping_add(1);
                    }
                    if at >= self.image.len() {
                        break;
                    }
                    let ms = self.r.below(30);
                    self.rig.run(ms);
                }
            }
            10 => {
                let h = [0x18u16, 0x1b][r.below(2) as usize];
                let index = if r.below(2) == 0 { self.next } else { r.below(0x2000) as u16 };
                let mut p = index.to_le_bytes().to_vec();
                let crc = ble::crypto::crc16_modbus(&p);
                p.extend_from_slice(&crc.to_le_bytes());
                if r.below(4) == 0 {
                    p = vec![0xff, 0xff, r.byte(), r.byte()];
                }
                att_write(&mut self.rig, h, &p, h == 0x1b);
            }
            11 => {
                let ms = r.below(3000);
                self.rig.central().silent = true;
                self.rig.run(ms);
                self.rig.central().silent = false;
            }
            12 => {
                if r.below(4) == 0 {
                    self.rig.central().terminate();
                }
                self.ensure_connected();
            }
            _ => {
                let h = r.below(0x32) as u16;
                let n = r.below(30) as usize;
                let v = r.bytes(n);
                if h == 0x1e && v.first().is_some_and(|op| (4..=6).contains(op)) {
                    return;
                }
                att_write(&mut self.rig, h, &v, r.below(2) == 0);
            }
        }
        let ms = 5 + self.r.below(200);
        self.rig.run(ms);
    }

    fn full_ota(&mut self) {
        let block = 16;
        let mut packet = [0u8; 20];
        packet[2] = (self.image.len() / 4096 + 2) as u8;
        self.rig.write(0x1b, &packet);
        self.rig.run(600);
        let n = self.image.len().div_ceil(block);
        for i in 0..n {
            let mut b = self.image[i * block..((i + 1) * block).min(self.image.len())].to_vec();
            b.resize(block, 0xff);
            self.rig.write(0x1b, &ota_packet(i as u16, &b));
        }
        self.rig.write(0x1b, &ota_packet(n as u16, &[]));
    }
}

fn run_case(seed: u64, steps: usize) -> Result<(), String> {
    let identity = [DIMMABLE, TUNABLE, COLOUR][(seed % 3) as usize];
    let mut case = Case { rig: Rig::new(identity), r: R(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1), image: Vec::new(), renamed: false, block: 16, next: 0 };
    let len = 0x40 + case.r.below(5000) as usize;
    case.image = make_image(len, case.r.byte());
    case.rig.connect();
    for _ in 0..steps {
        STEPS.fetch_add(1, Relaxed);
        case.step();
        if case.rig.world.borrow().activated {
            let st = case.rig.world.borrow().staging.clone();
            if !staged_good(&st) {
                return Err(format!("seed {seed}: activated a staged image whose CRC is wrong"));
            }
            EARLY.fetch_add(1, Relaxed);
            return Ok(());
        }
    }
    let _ = case.renamed;
    CMDS.fetch_add(case.rig.world.borrow().commands.len(), Relaxed);
    case.rig.central().silent = false;
    case.rig.run(8000);
    case.ensure_connected();
    case.rig.run(500);
    if case.rig.state() != LinkState::Connected {
        case.rig.run(3000);
        case.rig.central().connected = false;
        case.rig.central().connect = true;
        case.rig.run(3000);
        if case.rig.state() != LinkState::Connected {
            let c = case.rig.central();
            return Err(format!("seed {seed}: the bulb does not take a connection afterwards: connect {} connected {} adv {} now {}", c.connect, c.connected, c.advertisements.len(), 0));
        }
    }
    case.settle();
    let name = case.rig.read(0x03);
    let st = case.rig.read(0x1b);
    RECEIVED.fetch_add(u16::from_le_bytes([st[2], st[3]]) as usize, Relaxed);
    if name.is_empty() {
        return Err(format!("seed {seed}: no name"));
    }
    if !case.rig.login(b"unpaired", b"1234") {
        return Err(format!("seed {seed}: the default login is refused afterwards"));
    }
    case.rig.enable_status();
    let mut before = case.rig.read(0x1b);
    if before[0] == 3 {
        case.rig.central().terminate();
        case.rig.run(2000);
        case.ensure_connected();
        case.settle();
        if !case.rig.login(b"unpaired", b"1234") {
            return Err(format!("seed {seed}: the default login is refused after a reconnect"));
        }
        before = case.rig.read(0x1b);
    }
    case.full_ota();
    let after = case.rig.read(0x1b);
    case.rig.run(1500);
    let w = case.rig.world.borrow();
    if !w.activated {
        return Err(format!("seed {seed}: a clean update afterwards did not activate: status before {:02x?} after {:02x?} image {} B", &before[..8], &after[..8], case.image.len()));
    }
    if w.staging[..case.image.len()] != case.image[..] {
        return Err(format!("seed {seed}: the clean update afterwards staged other bytes"));
    }
    Ok(())
}

#[test]
fn fuzz_link_att_mesh_ota() {
    let cases: u64 = std::env::var("FUZZ_CASES").ok().and_then(|s| s.parse().ok()).unwrap_or(20);
    let start: u64 = std::env::var("FUZZ_START").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
    let mut fails = Vec::new();
    for seed in start..start + cases {
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_case(seed, 120)));
        match res {
            Ok(Ok(())) => {}
            Ok(Err(e)) => fails.push(e),
            Err(p) => {
                let msg = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
                fails.push(format!("seed {seed}: panic {msg}"));
            }
        }
    }
    eprintln!("{} cases, {} failed; {} steps, {} activated mid-run, {} logins, {} commands to the light, {} OTA blocks still counted at the end", cases, fails.len(), STEPS.load(Relaxed), EARLY.load(Relaxed), LOGINS.load(Relaxed), CMDS.load(Relaxed), RECEIVED.load(Relaxed));
    for f in fails.iter().take(30) {
        eprintln!("  {f}");
    }
    assert!(fails.is_empty());
}
