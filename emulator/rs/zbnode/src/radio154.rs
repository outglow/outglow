use std::cell::{RefCell, RefMut};
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;

use tc32emu::{stop_err, Error, Machine, RegReadHook, RegWriteHook};

const US_PER_BYTE: i64 = 32;
const SHR_PHR: i64 = 6;
const TICKS_PER_US: f64 = 16.0;
const STATUS_RX: u8 = 0x01;
const STATUS_TX: u8 = 0x02;
const IRQ_SOURCE_RF: u32 = 1 << 13;

pub fn crc16_kermit(b: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for &x in b {
        crc ^= x as u16;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = crc >> 1 ^ 0x8408;
            } else {
                crc >>= 1;
            }
        }
    }
    crc
}

pub fn airtime_ms(n: usize) -> f64 {
    ((SHR_PHR + n as i64) * US_PER_BYTE) as f64 / 1000.0
}

pub const INSTALL_8258_WRITES: [usize; 2] = [0x74F, 0x74C];

pub fn install_8258(next: Option<RegWriteHook>) -> RegWriteHook {
    Rc::new(move |m: &mut Machine, o: usize, size: usize, val: u32| {
        if o <= 0x74F && 0x74F < o + size {
            let b = (val >> (8 * (0x74F - o) as u32)) as u8;
            let mut cur = m.regs[0x74A] as u32;
            if b & 1 != 0 {
                cur |= 2;
            } else if b & 2 != 0 {
                cur &= !2;
            }
            m.reg_write(0x74A, 1, cur)?;
            if size == 1 {
                m.regs[0x74F] = b;
                return Ok(true);
            }
        }
        if o <= 0x74C && 0x74C < o + size {
            let b = (val >> (8 * (0x74C - o) as u32)) as u8;
            if b & 0x02 != 0 {
                m.regs[0x748] |= 0x04;
            } else {
                m.regs[0x748] &= !0x04;
            }
            if size == 1 {
                m.regs[0x74C] = b;
                return Ok(true);
            }
        }
        match &next {
            Some(n) => n(m, o, size, val),
            None => Ok(false),
        }
    })
}

type EvFn = Box<dyn FnOnce(&mut Machine)>;

struct Event {
    t: f64,
    #[allow(dead_code)]
    seq: i64,
    f: EvFn,
}

struct RxAir {
    t0: f64,
    t1: f64,
    ok: bool,
    collided: bool,
    off: Option<&'static str>,
}

#[derive(Clone, Debug)]
pub struct RxFate {
    pub t0: f64,
    pub t1: f64,
    pub fcs: u16,
    pub len: usize,
    pub fate: &'static str,
    pub cleared: Option<(f64, u32)>,
}

fn off_reason(m: &Machine, tx_busy: bool) -> Option<&'static str> {
    let g = &m.regs;
    if !zigbee_mode(m) {
        Some("BLE mode")
    } else if tx_busy {
        Some("transmitting")
    } else if g[0xF02] & 0x10 != 0 {
        Some("TX mode")
    } else if g[0xF02] & 0x20 == 0 {
        Some("RX state off")
    } else if g[0x428] & 0x01 == 0 {
        Some("RX disabled")
    } else {
        None
    }
}

fn fcs_of(p: &[u8]) -> u16 {
    let n = p.len();
    if n >= 2 {
        u16::from_le_bytes([p[n - 2], p[n - 1]])
    } else {
        0
    }
}

#[derive(Clone, Debug)]
pub struct Frame {
    pub t0: f64,
    pub t1: f64,
    pub channel: i64,
    pub psdu: Vec<u8>,
}

pub struct Radio154State {
    sched: VecDeque<Event>,
    seq: i64,
    tx_busy: bool,
    rx: BTreeMap<i64, RxAir>,
    rx_id: i64,
    rssi_now: u8,
    pub tx_pending: VecDeque<Frame>,
    pub sent: i64,
    pub received: i64,
    pub dropped: BTreeMap<String, i64>,
    pub collisions: i64,
    pub bad_fcs: i64,
    pub rx_log: Option<Vec<RxFate>>,
    rx_pending: Option<usize>,
    pub stop_uncommitted: bool,
    track_leads: bool,
    tx_mode_since: Option<f64>,
    pub tx_leads: Vec<Option<f32>>,
}

impl Drop for Radio154State {
    fn drop(&mut self) {
        if !self.track_leads || self.tx_leads.is_empty() {
            return;
        }
        const EDGES: [f32; 9] = [25.0, 50.0, 100.0, 110.0, 120.0, 130.0, 150.0, 250.0, 500.0];
        let mut n = [0usize; EDGES.len() + 2];
        let mut min = f32::INFINITY;
        for l in &self.tx_leads {
            match l {
                None => n[0] += 1,
                Some(us) => {
                    min = min.min(*us);
                    n[1 + EDGES.iter().position(|e| us < e).unwrap_or(EDGES.len())] += 1;
                }
            }
        }
        let mut s = format!("tx leads: {} frames, min {min:.1} us; unseen {}", self.tx_leads.len(), n[0]);
        for (i, e) in EDGES.iter().enumerate() {
            s += &format!(", <{e} {}", n[1 + i]);
        }
        eprintln!("{s}, >= {} {}", EDGES[EDGES.len() - 1], n[EDGES.len() + 1]);
    }
}

fn committed(m: &Machine, s: &Radio154State) -> bool {
    zigbee_mode(m) && m.regs[0xF02] & 0x10 != 0 && !s.tx_busy && s.tx_pending.is_empty()
}

#[derive(Clone)]
pub struct Radio154(pub Rc<RefCell<Radio154State>>);

fn zigbee_mode(m: &Machine) -> bool {
    m.regs[0x404] & 0x03 == 0
}

fn rx_on(m: &Machine) -> bool {
    let g = &m.regs;
    zigbee_mode(m) && g[0xF02] & 0x20 != 0 && g[0x428] & 0x01 != 0 && g[0xF02] & 0x10 == 0
}

fn channel(m: &Machine) -> i64 {
    let g = &m.regs;
    let mhz = ((g[0x1245] & 0x3F) as i64) << 6 | (g[0x1244] >> 2) as i64;
    if mhz >= 2405 && (mhz - 2405) % 5 == 0 {
        return (mhz - 2405) / 5 + 11;
    }
    -1
}

fn irq_level(m: &mut Machine) -> u32 {
    let g = &m.regs;
    if (g[0xF20] as u16 | (g[0xF21] as u16) << 8) & (g[0xF1C] as u16 | (g[0xF1D] as u16) << 8) != 0 {
        return IRQ_SOURCE_RF;
    }
    0
}

impl Radio154 {
    pub fn b(&self) -> RefMut<'_, Radio154State> {
        self.0.borrow_mut()
    }

    pub fn new(m: &mut Machine) -> Radio154 {
        let r = Radio154(Rc::new(RefCell::new(Radio154State {
            sched: VecDeque::new(),
            seq: 0,
            tx_busy: false,
            rx: BTreeMap::new(),
            rx_id: 0,
            rssi_now: 0,
            tx_pending: VecDeque::new(),
            sent: 0,
            received: 0,
            dropped: BTreeMap::new(),
            collisions: 0,
            bad_fcs: 0,
            rx_log: None,
            rx_pending: None,
            stop_uncommitted: false,
            track_leads: std::env::var_os("ZB_TX_LEADS").is_some_and(|v| !v.is_empty()),
            tx_mode_since: None,
            tx_leads: Vec::new(),
        })));
        let prev_w = m.reg_write_hook.clone();
        let r1 = r.clone();
        m.reg_write_hook = Some(Rc::new(move |m: &mut Machine, o, size, val| r1.reg_write(m, o, size, val, &prev_w)));
        let prev_r = m.reg_read_hook.clone();
        let r2 = r.clone();
        m.reg_read_hook = Some(Rc::new(move |m: &mut Machine, o, size| r2.reg_read(m, o, size, &prev_r)));
        let prev_t = m.update_time_hook.clone();
        let r3 = r.clone();
        m.update_time_hook = Some(Rc::new(move |m: &mut Machine| {
            if let Some(p) = &prev_t {
                p(m)?;
            }
            r3.tick(m);
            let mut s = r3.b();
            if !s.tx_pending.is_empty() {
                return Err(stop_err("tx"));
            }
            if s.stop_uncommitted || s.track_leads {
                let c = committed(m, &s);
                if s.track_leads {
                    if !c {
                        s.tx_mode_since = None;
                    } else if s.tx_mode_since.is_none() {
                        s.tx_mode_since = Some(m.ms());
                    }
                }
                if s.stop_uncommitted && !c {
                    return Err(stop_err("tx mode left"));
                }
            }
            Ok(())
        }));
        let prev_i = m.idle_skip_hook.clone();
        let r4 = r.clone();
        m.idle_skip_hook = Some(Rc::new(move |m: &mut Machine, mut n: i64| {
            if let Some(p) = &prev_i {
                n = p(m, n);
            }
            let first = r4.b().sched.front().map(|e| e.t);
            if let Some(t) = first {
                let mut c = ((t - m.ms()) * m.cpu_hz as f64 / 1000.0) as i64;
                if c < 0 {
                    c = 0;
                }
                if c < n {
                    n = c;
                }
            }
            n
        }));
        m.level_sources.push(Rc::new(irq_level));
        let r5 = r.clone();
        m.reset_hooks.push(Rc::new(move |_m: &mut Machine| {
            let mut s = r5.b();
            s.tx_busy = false;
            s.rx.clear();
            s.rssi_now = 0;
            s.sched.clear();
        }));
        r
    }

    pub fn at(&self, t: f64, f: EvFn) {
        let mut s = self.b();
        s.seq += 1;
        let seq = s.seq;
        let pos = s.sched.partition_point(|e| !(t < e.t));
        s.sched.insert(pos, Event { t, seq, f });
    }

    fn tick(&self, m: &mut Machine) {
        let now = m.ms();
        loop {
            let f = {
                let mut s = self.b();
                match s.sched.front() {
                    Some(e) if e.t <= now => s.sched.pop_front().unwrap().f,
                    _ => break,
                }
            };
            f(m);
        }
    }

    pub fn committed(&self, m: &Machine) -> bool {
        committed(m, &self.0.borrow())
    }

    #[allow(dead_code)]
    pub fn next_event(&self) -> f64 {
        self.b().sched.front().map(|e| e.t).unwrap_or(-1.0)
    }

    pub fn hooked_regs() -> (&'static [usize], &'static [usize]) {
        (&[0x441, 0x448], &[0xF20, 0xF21, 0xC24])
    }

    fn reg_read(&self, m: &mut Machine, o: usize, size: usize, next: &Option<RegReadHook>) -> Result<Option<u32>, Error> {
        if size == 1 && o == 0x441 {
            return Ok(Some(self.b().rssi_now as u32));
        }
        if o <= 0x448 && 0x448 < o + size {
            let mut v = 0u32;
            let mut handled = false;
            if let Some(n) = next {
                if let Some(x) = n(m, o, size)? {
                    v = x;
                    handled = true;
                }
            }
            if !handled {
                for i in 0..size {
                    v |= (m.regs[o + i] as u32) << (8 * i);
                }
            }
            v &= !(0x20u32 << (8 * (0x448 - o)));
            return Ok(Some(v));
        }
        match next {
            Some(n) => n(m, o, size),
            None => Ok(None),
        }
    }

    fn reg_write(&self, m: &mut Machine, o: usize, size: usize, val: u32, next: &Option<RegWriteHook>) -> Result<bool, Error> {
        if o <= 0xF21 && o + size > 0xF20 {
            for i in 0..size {
                let (a, b) = (o + i, (val >> (8 * i as u32)) as u8);
                if a == 0xF20 || a == 0xF21 {
                    if a == 0xF20 && b & m.regs[a] & STATUS_RX != 0 {
                        let mut s = self.b();
                        if let (Some(i), Some(l)) = (s.rx_pending.take(), s.rx_log.as_mut()) {
                            l[i].cleared = Some((m.ms(), m.r[15]));
                        }
                    }
                    m.regs[a] &= !b;
                } else {
                    m.reg_write(a, 1, b as u32)?;
                }
            }
            return Ok(true);
        }
        let mut handled = false;
        if let Some(n) = next {
            handled = n(m, o, size, val)?;
        }
        if !handled {
            let hook = m.reg_write_hook.take();
            let res = m.reg_write(o, size, val);
            m.reg_write_hook = hook;
            res?;
        }
        if o <= 0xC24 && 0xC24 < o + size && (val >> (8 * (0xC24 - o) as u32)) & 0x08 != 0 {
            self.tx_start(m);
        }
        Ok(true)
    }

    fn tx_start(&self, m: &mut Machine) {
        let now = m.ms();
        let off = m.regs[0xC0C] as usize | (m.regs[0xC0D] as usize) << 8;
        let phr = m.sram[off + 4] as usize;
        if !zigbee_mode(m) {
            return;
        }
        if m.regs[0xF02] & 0x10 == 0 {
            *self.b().dropped.entry("TX outside TX mode".into()).or_insert(0) += 1;
            return;
        }
        if !(2..=127).contains(&phr) {
            *self.b().dropped.entry("bad PHR".into()).or_insert(0) += 1;
            return;
        }
        let mut psdu = m.sram[off + 5..off + 5 + phr - 2].to_vec();
        let fcs = crc16_kermit(&psdu);
        psdu.extend_from_slice(&fcs.to_le_bytes());
        let t1 = now + airtime_ms(psdu.len());
        let ch = channel(m);
        {
            let mut s = self.b();
            if s.track_leads {
                let lead = s.tx_mode_since.take().map(|t| ((now - t) * 1000.0) as f32);
                s.tx_leads.push(lead);
            }
            s.tx_busy = true;
            s.sent += 1;
            s.tx_pending.push_back(Frame { t0: now, t1, channel: ch, psdu });
        }
        let me = self.clone();
        self.at(
            t1,
            Box::new(move |m| {
                m.regs[0xC24] &= !0x08;
                me.b().tx_busy = false;
                m.regs[0xF20] |= STATUS_TX;
            }),
        );
    }

    pub fn inject(&self, f: Frame, rssi: i64) {
        self.inject_with(f, rssi, false)
    }

    pub fn inject_bad_fcs(&self, f: Frame, rssi: i64) {
        self.inject_with(f, rssi, true)
    }

    fn inject_with(&self, f: Frame, rssi: i64, bad_fcs: bool) {
        let id = {
            let mut s = self.b();
            s.rx_id += 1;
            s.rx_id
        };
        let (me, f1) = (self.clone(), f.clone());
        self.at(f.t0, Box::new(move |m| me.frame_start(m, id, &f1, rssi)));
        let me = self.clone();
        let t1 = f.t1;
        self.at(t1, Box::new(move |m| me.frame_end(m, id, &f, rssi, bad_fcs)));
    }

    fn frame_start(&self, m: &mut Machine, id: i64, f: &Frame, rssi: i64) {
        if f.channel != channel(m) {
            let mut s = self.b();
            if let Some(log) = &mut s.rx_log {
                let fate = if zigbee_mode(m) { "other channel" } else { "BLE mode" };
                log.push(RxFate { t0: f.t0, t1: f.t1, fcs: fcs_of(&f.psdu), len: f.psdu.len(), fate, cleared: None });
            }
            return;
        }
        let on = rx_on(m);
        let mut s = self.b();
        let mut ok = true;
        for a in s.rx.values_mut() {
            if a.t0 < f.t1 && f.t0 < a.t1 {
                a.ok = false;
                a.collided = true;
                ok = false;
            }
        }
        if !ok {
            s.collisions += 1;
        }
        let air_ok = ok && on && !s.tx_busy;
        let off = if s.rx_log.is_some() { off_reason(m, s.tx_busy) } else { None };
        s.rx.insert(id, RxAir { t0: f.t0, t1: f.t1, ok: air_ok, collided: !ok, off });
        let v = (rssi + 110).max(0);
        s.rssi_now = v as u8;
    }

    fn frame_end(&self, m: &mut Machine, id: i64, f: &Frame, rssi: i64, bad_fcs: bool) {
        let st = {
            let mut s = self.b();
            let st = s.rx.remove(&id);
            if s.rx.is_empty() {
                s.rssi_now = 0;
            }
            st
        };
        let Some(st) = st else {
            return;
        };
        let drop_why = if !st.ok {
            Some("collision or receiver off at start")
        } else if !rx_on(m) {
            Some("receiver off")
        } else if m.regs[0xC20] & 0x04 == 0 {
            Some("RX DMA off")
        } else {
            None
        };
        let logging = self.b().rx_log.is_some();
        let log = |me: &Self, fate: &'static str| {
            if let Some(l) = &mut me.b().rx_log {
                l.push(RxFate { t0: f.t0, t1: f.t1, fcs: fcs_of(&f.psdu), len: f.psdu.len(), fate, cleared: None });
            }
        };
        if let Some(w) = drop_why {
            *self.b().dropped.entry(w.into()).or_insert(0) += 1;
            if logging {
                let tx_busy = self.b().tx_busy;
                let fate = if st.collided {
                    "collision"
                } else if let Some(why) = st.off {
                    match why {
                        "BLE mode" => "off at start: BLE mode",
                        "transmitting" => "off at start: transmitting",
                        "TX mode" => "off at start: TX mode",
                        "RX state off" => "off at start: RX state off",
                        _ => "off at start: RX disabled",
                    }
                } else if w == "RX DMA off" {
                    "RX DMA off"
                } else {
                    match off_reason(m, tx_busy) {
                        Some("BLE mode") => "off at end: BLE mode",
                        Some("transmitting") => "off at end: transmitting",
                        Some("TX mode") => "off at end: TX mode",
                        Some("RX state off") => "off at end: RX state off",
                        _ => "off at end: RX disabled",
                    }
                };
                log(self, fate);
            }
            return;
        }
        let off = m.regs[0xC08] as usize | (m.regs[0xC09] as usize) << 8;
        let capb = m.regs[0xC0A] as usize * 16;
        let l = f.psdu.len();
        if capb != 0 && l + 13 > capb {
            *self.b().dropped.entry("too long for the buffer".into()).or_insert(0) += 1;
            log(self, "too long for the buffer");
            return;
        }
        log(self, if bad_fcs { "bad FCS" } else { "received" });
        {
            let mut s = self.b();
            if s.rx_log.is_some() {
                if m.regs[0xF20] & STATUS_RX != 0 {
                    if let (Some(i), Some(l)) = (s.rx_pending, s.rx_log.as_mut()) {
                        if l[i].cleared.is_none() {
                            l[i].fate = "overwritten";
                        }
                    }
                }
                s.rx_pending = s.rx_log.as_ref().map(|l| l.len() - 1);
            }
        }
        let since_sfd_us = (f.t1 - f.t0).mul_add(1000.0, -((5 * US_PER_BYTE) as f64));
        let ts = m.stimer_now().wrapping_sub((since_sfd_us * TICKS_PER_US) as u32);
        let mut pkt = Vec::with_capacity(l + 13);
        pkt.extend_from_slice(&((l + 9) as u32).to_le_bytes());
        pkt.push(l as u8);
        pkt.extend_from_slice(&f.psdu);
        pkt.extend_from_slice(&ts.to_le_bytes());
        let v = (rssi + 110).max(0);
        pkt.extend_from_slice(&[0, 0, v as u8, if bad_fcs { 0x00 } else { 0x10 }]);
        let n = pkt.len().min(m.sram.len().saturating_sub(off));
        m.sram[off..off + n].copy_from_slice(&pkt[..n]);
        if bad_fcs {
            self.b().bad_fcs += 1;
        } else {
            self.b().received += 1;
        }
        m.regs[0xF20] |= STATUS_RX;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tc32emu::{Config, Flash};

    #[test]
    fn reset_mid_frame_forgets_the_frame() {
        let mut fl = Flash::new(Config::env().flash_size);
        fl.mem[8] = 0x4B;
        let mut m = Machine::new(fl, -1, 1).unwrap();
        let r = Radio154::new(&mut m);
        m.regs[0x1245] = 37;
        m.regs[0x1244] = 37 << 2;
        assert_eq!(channel(&m), 11);
        r.inject(Frame { t0: 0.0, t1: 1.0, channel: 11, psdu: vec![0x02, 0x00, 0x01, 0x00, 0x00] }, -40);
        m.update_time().unwrap();
        assert_eq!(m.reg_read(0x441, 1).unwrap(), 70, "the frame's signal while it arrives");
        m.reset(-1, false).unwrap();
        assert_eq!(m.reg_read(0x441, 1).unwrap(), 0, "the signal after the reset");
        assert!(r.b().sched.is_empty(), "the frame's end is still scheduled");
    }
}
