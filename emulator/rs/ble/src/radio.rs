// SPDX-License-Identifier: Apache-2.0
//! Radio (radio.go): the TLSR8278 RF block as a BLE peripheral's link layer
//! drives it.
//!
//! The radio is shared (`RadioRef`, an `Rc<RefCell<Radio>>`): the machine's
//! hooks, the scheduled steps and the centrals all hold it. No borrow of it is
//! held across a call into the machine or into a peer, since both can come
//! back into the radio (a register write re-enters the hook chain, a central
//! schedules its next event).

use std::cell::{RefCell, RefMut};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::rc::Rc;

use tc32emu::aes128;
use tc32emu::{Error, Machine, RegReadHook, RegWriteHook};

use crate::air::{Air, TO_HOST, TO_KEYBOARD};
use crate::gofmt::hex;
use crate::round3;
use crate::settings;

pub const M32: u32 = 0xFFFFFFFF;
pub const TICKS_PER_US: f64 = 16.0;
pub const RX: u32 = 0x1;
pub const TX: u32 = 0x2;
pub const RXTO: u32 = 0x4;
pub const CRC2: u32 = 0x10;
pub const DONE: u32 = 0x20;
pub const FSMTO: u32 = 0x40;
pub const FIRSTTO: u32 = 0x400;
/// TPLL: from the end of the chip's packet to the start of the peer's reply.
pub const TURNAROUND_MS: f64 = 0.150;

/// The advertising access address as a link layer writes 0x408..0x40b (MSB first).
pub const ADV_AA: [u8; 4] = [0x8e, 0x89, 0xbe, 0xd6];

pub fn ll_name_of(op: u8) -> Option<&'static str> {
    Some(match op {
        0x00 => "LL_CONNECTION_UPDATE_IND",
        0x01 => "LL_CHANNEL_MAP_IND",
        0x02 => "LL_TERMINATE_IND",
        0x03 => "LL_ENC_REQ",
        0x04 => "LL_ENC_RSP",
        0x05 => "LL_START_ENC_REQ",
        0x06 => "LL_START_ENC_RSP",
        0x07 => "LL_UNKNOWN_RSP",
        0x08 => "LL_FEATURE_REQ",
        0x09 => "LL_FEATURE_RSP",
        0x0a => "LL_PAUSE_ENC_REQ",
        0x0b => "LL_PAUSE_ENC_RSP",
        0x0c => "LL_VERSION_IND",
        0x0d => "LL_REJECT_IND",
        0x0e => "LL_PERIPHERAL_FEATURE_REQ",
        0x0f => "LL_CONNECTION_PARAM_REQ",
        0x10 => "LL_CONNECTION_PARAM_RSP",
        0x11 => "LL_REJECT_EXT_IND",
        0x12 => "LL_PING_REQ",
        0x13 => "LL_PING_RSP",
        0x14 => "LL_LENGTH_REQ",
        0x15 => "LL_LENGTH_RSP",
        0x16 => "LL_PHY_REQ",
        0x17 => "LL_PHY_RSP",
        0x18 => "LL_PHY_UPDATE_IND",
        0x19 => "LL_MIN_USED_CHANNELS_IND",
        _ => return None,
    })
}

pub fn att_name_of(op: u8) -> Option<&'static str> {
    Some(match op {
        0x01 => "Error Rsp",
        0x02 => "Exchange MTU Req",
        0x03 => "Exchange MTU Rsp",
        0x04 => "Find Info Req",
        0x05 => "Find Info Rsp",
        0x08 => "Read By Type Req",
        0x09 => "Read By Type Rsp",
        0x0a => "Read Req",
        0x0b => "Read Rsp",
        0x0c => "Read Blob Req",
        0x0d => "Read Blob Rsp",
        0x10 => "Read By Group Type Req",
        0x11 => "Read By Group Type Rsp",
        0x12 => "Write Req",
        0x13 => "Write Rsp",
        0x52 => "Write Cmd",
        0x1b => "Handle Value Ntf",
        0x1d => "Handle Value Ind",
        0x1e => "Handle Value Cfm",
        _ => return None,
    })
}

/// 1M PHY: preamble 1, access address 4, header 2, payload, CRC 3 bytes.
pub fn airtime_us(pdu: &[u8]) -> i64 {
    (1 + 4 + 2 + pdu[1] as i64 + 3) * 8
}

/// What the radio needs of a central (Go's Peer, plus the optional
/// AirLostPeer, AirLostTxPeer and Scanner methods with do-nothing defaults:
/// Go skips a peer without them, which is what the defaults do).
pub trait Peer {
    /// The reply to an ADV_IND (CONNECT_IND), or None.
    fn on_adv(&self, m: &mut Machine, pdu: &[u8], ch: u8) -> Option<Vec<u8>>;
    fn on_connect_sent(&self, m: &mut Machine, pdu: &[u8], t_end: f64);
    fn drop_pdu(&self, m: &mut Machine, pdu: &[u8]) -> bool;
    /// The next PDU in this event, or None.
    fn on_air(&self, m: &mut Machine, pdu: &[u8], ch: u8, tx_addr: u32) -> Option<Vec<u8>>;
    fn air_lost(&self, _m: &mut Machine, _pdu: &[u8]) {}
    fn air_lost_tx(&self, _m: &mut Machine, _pdu: &[u8]) {}
    fn scan_request(&self, _m: &mut Machine, _pdu: &[u8], _ch: u8) -> Option<Vec<u8>> {
        None
    }
}

pub type PeerRc = Rc<dyn Peer>;

/// Go's interface equality of two peers: the same object.
pub fn same_peer(a: &PeerRc, b: &PeerRc) -> bool {
    Rc::as_ptr(a) as *const () == Rc::as_ptr(b) as *const ()
}

/// A peer in Telink's private format (a 2.4G dongle model): the reply payload, or None.
pub trait TpllPeer {
    fn on_tpll(&self, m: &mut Machine, payload: &[u8], mhz: i64, access_code: &[u8]) -> Option<Vec<u8>>;
}

/// An open receive window in Telink's private format.
#[derive(Clone, Debug)]
pub struct Srx {
    pub t: f64,
    pub start: u32,
    pub mhz: i64,
    pub ac: Vec<u8>,
    pub rx: i64,
}
pub type SrxRc = Rc<RefCell<Srx>>;

#[derive(Clone, Debug)]
pub struct TpllTx {
    pub ms: f64,
    pub mhz: i64,
    pub ac: Vec<u8>,
    pub payload: Vec<u8>,
}

pub struct ScanReq {
    pub ms: f64,
    pub ch: u8,
    pub central: PeerRc,
    pub pdu: Vec<u8>,
}

/// A peripheral connection event the link layer scheduled.
#[derive(Clone, Debug)]
pub struct Brx {
    pub t: f64,
    pub start: u32,
    pub ch: u8,
    pub aa: Vec<u8>,
    pub crc_init: u32,
    pub last_sn: i64,
    pub nesn: i64,
    pub rx: i64,
    pub f03: u8,
    /// The last central packet's start and end (ms).
    pub on_air: Option<[f64; 2]>,
}
pub type BrxRc = Rc<RefCell<Brx>>;

pub type SchedFn = Box<dyn FnOnce(&mut Machine)>;

pub(crate) struct Sched {
    pub(crate) ms: f64,
    #[allow(dead_code)]
    pub(crate) seq: i64,
    pub(crate) f: SchedFn,
    pub(crate) central: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AdvTx {
    pub ms: f64,
    pub ch: u8,
    pub pdu: Vec<u8>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Cmd {
    pub ms: f64,
    pub cmd: u8,
    pub ch: u8,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Miss {
    pub ms: f64,
    /// The BLE channel, or the TPLL frequency in MHz.
    pub ch: i64,
    pub why: String,
}
#[derive(Clone, Debug, PartialEq)]
pub struct FifoEntry {
    pub ms: f64,
    pub wptr: i64,
    pub addr: u16,
}
impl FifoEntry {
    /// Go's %+v of the struct.
    pub fn go_plus_v(&self) -> String {
        format!("{{Ms:{} WPtr:{} Addr:{}}}", crate::gofmt::g(self.ms), self.wptr, self.addr)
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct AdvFifo {
    pub ms: f64,
    pub ch: u8,
    pub rptr: i64,
    pub wptr: i64,
}
/// A command the radio did not start because a suspend lost its setup.
#[derive(Clone, Debug, PartialEq)]
pub struct RfRefusal {
    pub ms: f64,
    pub cmd: u8,
    pub loss: i64,
}

pub type OnAirTxFn = Rc<dyn Fn(f64, u8, u32, Vec<u8>)>;

/// The TLSR8278 RF block model.
pub struct Radio {
    pub log: Option<Rc<dyn Fn(&str)>>,
    pub central: Option<PeerRc>,
    pub centrals: Vec<PeerRc>,
    pub conn_central: Option<PeerRc>,
    pub scan_reqs: Vec<ScanReq>,
    pub(crate) sched: VecDeque<Sched>,
    pub(crate) seq: i64,
    pub fifo: [u16; 16],
    pub wptr: i64,
    pub rptr: i64,
    pub brx: Option<BrxRc>,
    pub(crate) sn_rb: i64,
    pub(crate) nesn_rb: i64,
    pub(crate) ts: u32,
    pub adv_tx: Vec<AdvTx>,
    pub cmds: Vec<Cmd>,
    pub misses: Vec<Miss>,
    pub adv_fifo: Vec<AdvFifo>,
    pub fifo_log: Vec<FifoEntry>,
    pub(crate) aes_in: Vec<u8>,
    pub(crate) aes_out: Vec<u8>,
    pub aes_ops: i64,
    pub leads: Vec<f64>,
    pub rx_on: Vec<f64>,
    pub tpll: Option<Rc<dyn TpllPeer>>,
    pub srx: Option<SrxRc>,
    pub tpll_tx: Vec<TpllTx>,
    pub rf_written: BTreeSet<usize>,
    pub rf_uninit: BTreeSet<usize>,
    pub rf_losses: i64,
    pub rf_refused: Vec<RfRefusal>,
    pub(crate) rf_power_on: HashMap<usize, u8>,
    pub(crate) prev_read: Option<RegReadHook>,
    pub(crate) prev_write: Option<RegWriteHook>,
    pub(crate) pending: Option<Error>,
    pub air: Option<Air>,
    pub tx_lost: bool,
    pub late_starts: Vec<[f64; 2]>,
    pub on_air_tx: Option<OnAirTxFn>,
    pub(crate) ble_srx: bool,
    pub air_rx_ok: i64,
    pub air_rx_miss: Vec<Miss>,
}

/// The registers a link layer's radio initialisation writes.
pub const RF_INIT_REGS: [usize; 26] = [0x12D2, 0x12D3, 0x127B, 0x1279, 0x124A, 0x124B, 0x1254, 0x1255, 0x1256, 0x1257, 0x1276,
    0x134E, 0x134C, 0x401, 0x402, 0x430, 0x460, 0x461, 0x462, 0x463, 0x464, 0x465, 0xF06, 0xF0C, 0xF0E, 0xF10];
/// The PA power registers.
pub const RF_POWER_REGS: [usize; 3] = [0x1225, 0x1226, 0x1227];

pub fn rf_suspend_regs() -> Vec<usize> {
    let mut v = RF_INIT_REGS.to_vec();
    v.extend_from_slice(&RF_POWER_REGS);
    v
}

fn rf_suspend_set(a: usize) -> bool {
    RF_INIT_REGS.contains(&a) || RF_POWER_REGS.contains(&a)
}

fn le16(b: &[u8], o: usize) -> u16 {
    b[o] as u16 | (b[o + 1] as u16) << 8
}
fn le32(b: &[u8], o: usize) -> u32 {
    b[o] as u32 | (b[o + 1] as u32) << 8 | (b[o + 2] as u32) << 16 | (b[o + 3] as u32) << 24
}

fn irq_level(m: &mut Machine) -> u32 {
    let rg = &m.regs;
    if (rg[0xF20] as u32 | (rg[0xF21] as u32) << 8) & (rg[0xF1C] as u32 | (rg[0xF1D] as u32) << 8) != 0 {
        return 1 << 13;
    }
    0
}

/// The shared radio.
#[derive(Clone)]
pub struct RadioRef(pub Rc<RefCell<Radio>>);

impl RadioRef {
    /// A borrow of the radio's state, for a moment.
    pub fn b(&self) -> RefMut<'_, Radio> {
        self.0.borrow_mut()
    }

    pub(crate) fn log(&self, f: impl FnOnce() -> String) {
        let l = self.b().log.clone();
        if let Some(l) = l {
            l(&f());
        }
    }

    /// NewRadio attaches the model to m.
    pub fn new(m: &mut Machine, log: Option<Rc<dyn Fn(&str)>>) -> RadioRef {
        settings::init();
        let mut rf_power_on = HashMap::new();
        for a in rf_suspend_regs() {
            rf_power_on.insert(a, m.regs[a]);
        }
        let r = Radio {
            log,
            central: None,
            centrals: Vec::new(),
            conn_central: None,
            scan_reqs: Vec::new(),
            sched: VecDeque::new(),
            seq: 0,
            fifo: [0; 16],
            wptr: 0,
            rptr: 0,
            brx: None,
            sn_rb: 0,
            nesn_rb: 0,
            ts: 0,
            adv_tx: Vec::new(),
            cmds: Vec::new(),
            misses: Vec::new(),
            adv_fifo: Vec::new(),
            fifo_log: Vec::new(),
            aes_in: Vec::new(),
            aes_out: Vec::new(),
            aes_ops: 0,
            leads: Vec::new(),
            rx_on: Vec::new(),
            tpll: None,
            srx: None,
            tpll_tx: Vec::new(),
            rf_written: BTreeSet::new(),
            rf_uninit: BTreeSet::new(),
            rf_losses: 0,
            rf_refused: Vec::new(),
            rf_power_on,
            prev_read: m.reg_read_hook.clone(),
            prev_write: m.reg_write_hook.clone(),
            pending: None,
            air: None,
            tx_lost: false,
            late_starts: Vec::new(),
            on_air_tx: None,
            ble_srx: false,
            air_rx_ok: 0,
            air_rx_miss: Vec::new(),
        };
        let rr = RadioRef(Rc::new(RefCell::new(r)));
        let r1 = rr.clone();
        m.reg_read_hook = Some(Rc::new(move |m: &mut Machine, o, size| r1.reg_read(m, o, size)));
        let r2 = rr.clone();
        m.reg_write_hook = Some(Rc::new(move |m: &mut Machine, o, size, val| r2.reg_write(m, o, size, val)));
        let prev_ut = m.update_time_hook.clone();
        let r3 = rr.clone();
        m.update_time_hook = Some(Rc::new(move |m: &mut Machine| {
            if let Some(p) = &prev_ut {
                p(m)?;
            }
            r3.tick(m)
        }));
        m.level_sources.push(Rc::new(irq_level));
        let r4 = rr.clone();
        m.reset_hooks.push(Rc::new(move |_m: &mut Machine| r4.on_reset()));
        // An idle loop jumps to the next timer compare: not past the radio's next step.
        let prev_idle = m.idle_skip_hook.clone();
        let r5 = rr.clone();
        m.idle_skip_hook = Some(Rc::new(move |m: &mut Machine, mut n: i64| {
            if let Some(p) = &prev_idle {
                n = p(m, n);
            }
            let first = r5.b().sched.front().map(|e| e.ms);
            if let Some(ms) = first {
                let mut lim = ((ms - m.ms()) * m.cpu_hz as f64 / 1000.0) as i64;
                if lim < 0 {
                    lim = 0;
                }
                if lim < n {
                    n = lim;
                }
            }
            n
        }));
        rr
    }

    // ------------------------------------------------------------ time

    /// At runs f at simulated time ms; owner "central" marks the central's
    /// own steps, which a reset of the machine does not cancel.
    pub fn at(&self, ms: f64, f: SchedFn, owner: &str) {
        let mut r = self.b();
        r.seq += 1;
        let seq = r.seq;
        // sort.SliceStable by (ms, seq) after an append: the new step goes after
        // every step with ms <= its own (seq is the largest).
        let pos = r.sched.partition_point(|e| !(ms < e.ms));
        r.sched.insert(pos, Sched { ms, seq, f, central: owner == "central" });
    }

    /// Runs the steps whose time has come.
    pub fn tick(&self, m: &mut Machine) -> Result<(), Error> {
        let now = m.ms();
        loop {
            let f = {
                let mut r = self.b();
                match r.sched.front() {
                    Some(e) if e.ms <= now => r.sched.pop_front().unwrap().f,
                    _ => break,
                }
            };
            f(m);
            let p = self.b().pending.take();
            if let Some(e) = p {
                return Err(e);
            }
        }
        Ok(())
    }

    /// The time of the next step, if any.
    pub fn next_step_ms(&self) -> Option<f64> {
        self.b().sched.front().map(|e| e.ms)
    }

    /// The machine was reset: the radio's state and its pending steps go; the central goes on.
    pub fn on_reset(&self) {
        let mut r = self.b();
        r.sched.retain(|e| e.central);
        r.fifo = [0; 16];
        r.wptr = 0;
        r.rptr = 0;
        r.brx = None;
        r.sn_rb = 0;
        r.nesn_rb = 0;
        r.ts = 0;
        r.aes_in.clear();
        r.aes_out.clear();
        r.srx = None;
        r.rf_uninit.clear();
        r.rf_written.clear();
    }

    /// The system timer value us microseconds ago.
    pub fn ticks_ago(&self, m: &Machine, us: f64) -> u32 {
        m.stimer_now().wrapping_sub((us * TICKS_PER_US) as i64 as u32)
    }

    // ------------------------------------------------------------ registers

    /// Sets bits of the RF IRQ status (0xf20/0xf21).
    pub fn set_status(m: &mut Machine, bits: u32) {
        m.regs[0xF20] |= (bits & 0xFF) as u8;
        m.regs[0xF21] |= (bits >> 8) as u8;
    }

    pub(crate) fn rx_busy(&self, m: &Machine) -> bool {
        match settings::rx_busy() {
            "stuck" => true,
            "air" => {
                let r = self.b();
                if let Some(b) = &r.brx {
                    if let Some(oa) = b.borrow().on_air {
                        let t = m.ms();
                        return oa[0] <= t && t < oa[1];
                    }
                }
                false
            }
            _ => false,
        }
    }

    /// The register bytes reg_read and reg_write may treat other than by
    /// passing the access on (for tc32emu::RegHookMask; keep in step with
    /// them): reads, and writes (None: every write, TC32EMU_SUSPEND_RF=lost).
    /// Any other write ends in base_write's detached machine write.
    pub fn hooked_regs() -> (Vec<usize>, Option<Vec<usize>>) {
        let read = vec![0x448, 0xC2A, 0xC2B, 0xF22, 0xF23, 0x450, 0x548];
        if settings::suspend_rf() == "lost" {
            return (read, None);
        }
        (read, Some(vec![0xF20, 0xF21, 0xC2A, 0x540, 0x548, 0xC2C, 0x61, 0xF00]))
    }

    pub(crate) fn reg_read(&self, m: &mut Machine, o: usize, size: usize) -> Result<Option<u32>, Error> {
        if o <= 0x448 && 0x448 < o + size && settings::rx_busy() != "off" {
            let prev = self.b().prev_read.clone();
            let mut v = 0u32;
            let mut ok = false;
            if let Some(p) = prev {
                if let Some(x) = p(m, o, size)? {
                    v = x;
                    ok = true;
                }
            }
            if !ok {
                for i in 0..size {
                    v |= (m.regs[o + i] as u32) << (8 * i);
                }
            }
            if self.rx_busy(m) {
                v |= 0x20u32 << (8 * (0x448 - o));
            }
            return Ok(Some(v));
        }
        if size == 1 {
            let r = self.b();
            match o {
                0xC2A => return Ok(Some(r.rptr as u32)),
                0xC2B => return Ok(Some(r.wptr as u32)),
                0xF22 => return Ok(Some(r.sn_rb as u32)),
                0xF23 => return Ok(Some((r.nesn_rb << 4) as u32)),
                _ => {}
            }
        }
        if o == 0x450 && size == 4 {
            return Ok(Some(self.b().ts));
        }
        if o == 0x548 && size == 4 {
            let mut r = self.b();
            let mut w = [0u8; 4];
            let n = r.aes_out.len().min(4);
            w[..n].copy_from_slice(&r.aes_out[..n]);
            if r.aes_out.len() > 4 {
                r.aes_out.drain(..4);
            } else {
                r.aes_out.clear();
            }
            return Ok(Some(u32::from_le_bytes(w)));
        }
        let prev = self.b().prev_read.clone();
        match prev {
            Some(p) => p(m, o, size),
            None => Ok(None),
        }
    }

    pub(crate) fn aes_write(&self, m: &mut Machine, o: usize, val: u32) {
        let mut r = self.b();
        let rg = &mut m.regs;
        if o == 0x540 {
            rg[0x540] = (val & 0x01) as u8 | 0x02;
            r.aes_in.clear();
            r.aes_out.clear();
            return;
        }
        r.aes_in.extend_from_slice(&val.to_le_bytes());
        if r.aes_in.len() == 16 {
            let key: Vec<u8> = rg[0x550..0x560].to_vec();
            r.aes_out = if rg[0x540] & 1 != 0 {
                aes128::decrypt(&key, &r.aes_in).to_vec()
            } else {
                aes128::encrypt(&key, &r.aes_in).to_vec()
            };
            if settings::aes_log() {
                let op = if rg[0x540] & 1 != 0 { "dec" } else { "enc" };
                eprintln!("AES {} key={} in={} out={} pc={:#x}", op, hex(&key), hex(&r.aes_in), hex(&r.aes_out), m.r[15]);
            }
            r.aes_in.clear();
            r.aes_ops += 1;
            m.regs[0x540] = (m.regs[0x540] & 0x01) | 0x04;
        }
    }

    /// Writes through the hooks below the radio, or the machine's own register write.
    pub(crate) fn base_write(&self, m: &mut Machine, o: usize, size: usize, val: u32) -> Result<(), Error> {
        let prev = self.b().prev_write.clone();
        if let Some(p) = prev {
            if p(m, o, size, val)? {
                return Ok(());
            }
        }
        let hook = m.reg_write_hook.take();
        let res = m.reg_write(o, size, val);
        m.reg_write_hook = hook;
        res
    }

    pub(crate) fn reg_write(&self, m: &mut Machine, o: usize, size: usize, val: u32) -> Result<bool, Error> {
        let lost = settings::suspend_rf() == "lost";
        if lost {
            let mut r = self.b();
            for a in o..o + size {
                if rf_suspend_set(a) {
                    r.rf_written.insert(a);
                    r.rf_uninit.remove(&a);
                }
            }
        }
        if o <= 0xF21 && o + size > 0xF20 {
            for i in 0..size {
                let (a, b) = (o + i, (val >> (8 * i as u32)) as u8);
                if a == 0xF20 || a == 0xF21 {
                    m.regs[a] &= !b;
                } else {
                    self.base_write(m, a, 1, b as u32)?;
                }
            }
            return Ok(true);
        }
        if o == 0xC2A && size == 1 {
            let mut r = self.b();
            if val & 0x10 != 0 {
                if settings::txfifo_clear() == "position" {
                    r.rptr = r.wptr;
                } else {
                    r.rptr = 0;
                    r.wptr = 0;
                }
            }
            if val & 0x40 != 0 {
                r.rptr = (val & 0xF) as i64;
            }
            if val & 0x20 != 0 {
                r.rptr = (r.rptr + 1) & 0xF;
            }
            m.regs[0xC2A] = val as u8;
            return Ok(true);
        }
        if (o == 0x540 && size == 1) || (o == 0x548 && size == 4) {
            self.aes_write(m, o, val);
            return Ok(true);
        }
        if o == 0xC2C && size == 2 {
            let ms = m.ms();
            let mut r = self.b();
            let wp = r.wptr;
            r.fifo[wp as usize] = val as u16;
            r.fifo_log.push(FifoEntry { ms, wptr: wp, addr: val as u16 });
            r.wptr = (wp + 1) & 0xF;
            return Ok(true);
        }
        self.base_write(m, o, size, val)?;
        if lost && o <= 0x6F && 0x6F < o + size && m.asleep.is_some() {
            let mut r = self.b();
            for a in rf_suspend_regs() {
                m.regs[a] = r.rf_power_on[&a];
            }
            r.rf_uninit = r.rf_written.clone();
            r.rf_losses += 1;
        }
        if o == 0x61 && size == 1 && val & 1 != 0 {
            self.b().brx = None; // RF reset
        }
        if o <= 0xF00 && 0xF00 < o + size {
            let cmd = m.regs[0xF00];
            self.on_cmd(m, cmd);
        }
        Ok(true)
    }

    // ------------------------------------------------------------ commands

    /// The delay from now to the armed RF start tick (0xf16 bit 2 + 0xf18), or 0.006 ms.
    pub(crate) fn start_delay_ms(m: &Machine) -> f64 {
        let rg = &m.regs;
        if rg[0xF16] & 4 == 0 {
            return 0.006;
        }
        let start = le32(rg, 0xF18);
        let d = start.wrapping_sub(m.stimer_now());
        if d >= 0x80000000 {
            return 0.006;
        }
        d as f64 / TICKS_PER_US / 1000.0 + 0.006
    }

    pub(crate) fn on_cmd(&self, m: &mut Machine, cmd: u8) {
        let t = m.ms();
        {
            let mut r = self.b();
            if !r.rf_uninit.is_empty() && cmd != 0x80 {
                let tell = (r.rf_refused.is_empty() || r.rf_refused.last().unwrap().loss != r.rf_losses) && m.event.is_some();
                let msg = if tell {
                    let first = *r.rf_uninit.iter().next().unwrap();
                    Some(format!(
                        "radio: command 0x{:02x} after a suspend, with {} of the radio registers it had set up not written again (first 0x{:x}): nothing sent or heard until they are",
                        cmd,
                        r.rf_uninit.len(),
                        first
                    ))
                } else {
                    None
                };
                let losses = r.rf_losses;
                r.rf_refused.push(RfRefusal { ms: round3(t), cmd, loss: losses });
                drop(r);
                if let (Some(msg), Some(ev)) = (msg, m.event.clone()) {
                    ev(m, &msg);
                }
                return;
            }
        }
        let has_tpll = self.b().tpll.is_some();
        if has_tpll && tpll_format(m) {
            let ch = m.regs[0x40D];
            match cmd {
                0x85 | 0x87 => {
                    self.b().cmds.push(Cmd { ms: t, cmd, ch });
                    let me = self.clone();
                    let d = Self::start_delay_ms(m);
                    self.at(t + d, Box::new(move |m| me.tpll_tx_start(m, cmd == 0x87)), "radio");
                    return;
                }
                0x86 => {
                    self.b().cmds.push(Cmd { ms: t, cmd, ch });
                    let mut start = m.stimer_now();
                    if m.regs[0xF16] & 4 != 0 {
                        start = le32(&m.regs, 0xF18);
                    }
                    let s = Rc::new(RefCell::new(Srx { t, start, mhz: tpll_mhz(m), ac: tpll_access_code(m), rx: 0 }));
                    self.b().srx = Some(s.clone());
                    if m.regs[0xF03] & 2 != 0 {
                        let us = le32(&m.regs, 0xF28).wrapping_add(1);
                        let me = self.clone();
                        self.at(
                            t + us as f64 / 1000.0,
                            Box::new(move |m| {
                                if s.borrow().rx == 0 {
                                    me.srx_timeout(m, &s);
                                }
                            }),
                            "radio",
                        );
                    }
                    return;
                }
                0x80 => {
                    self.b().srx = None;
                }
                _ => {}
            }
        }
        let rg_ch = m.regs[0x40D];
        self.b().cmds.push(Cmd { ms: t, cmd, ch: rg_ch });
        match cmd {
            0x80 => {
                let mut r = self.b();
                r.brx = None;
                r.ble_srx = false;
            }
            0x86 => {
                self.b().ble_srx = true;
                if m.regs[0xF03] & 2 != 0 {
                    let us = le32(&m.regs, 0xF28).wrapping_add(1);
                    let me = self.clone();
                    self.at(
                        t + us as f64 / 1000.0,
                        Box::new(move |m| {
                            let was = {
                                let mut r = me.b();
                                let w = r.ble_srx;
                                r.ble_srx = false;
                                w
                            };
                            if was {
                                Self::set_status(m, FIRSTTO);
                            }
                        }),
                        "radio",
                    );
                }
            }
            0x82 => {
                let mut start = m.stimer_now();
                if m.regs[0xF16] & 4 != 0 {
                    start = le32(&m.regs, 0xF18);
                }
                let late = m.stimer_now().wrapping_sub(start);
                if late > 0 && late < 0x80000000 {
                    self.b().late_starts.push([t, late as f64 / 16.0]);
                    self.log(|| format!("[{:9.3}] radio: BRX command {:.0} us after its start tick", t, late as f64 / 16.0));
                }
                let rg = &m.regs;
                let b = Rc::new(RefCell::new(Brx {
                    t,
                    start,
                    ch: rg[0x40D],
                    aa: rg[0x408..0x40C].to_vec(),
                    crc_init: rg[0x424] as u32 | (rg[0x425] as u32) << 8 | (rg[0x426] as u32) << 16,
                    last_sn: ((rg[0xF03] >> 4) & 1) as i64,
                    nesn: ((rg[0xF03] >> 5) & 1) as i64,
                    rx: 0,
                    f03: rg[0xF03],
                    on_air: None,
                }));
                self.b().brx = Some(b.clone());
                if m.regs[0xF03] & 1 != 0 {
                    let us = le32(&m.regs, 0xF2C);
                    let me = self.clone();
                    let bb = b.clone();
                    self.at(t + us as f64 / 1000.0, Box::new(move |m| me.timeout(m, &bb, FSMTO)), "radio");
                }
                if m.regs[0xF03] & 2 != 0 {
                    let us = le32(&m.regs, 0xF28);
                    if us < 0x0FFFFFFF {
                        let me = self.clone();
                        let bb = b.clone();
                        self.at(
                            t + us as f64 / 1000.0,
                            Box::new(move |m| {
                                if bb.borrow().rx == 0 {
                                    me.timeout(m, &bb, FIRSTTO);
                                }
                            }),
                            "radio",
                        );
                    }
                }
            }
            0x85 | 0x87 => {
                let me = self.clone();
                let d = Self::start_delay_ms(m);
                self.at(t + d, Box::new(move |m| me.adv_tx_start(m, cmd, t)), "radio");
            }
            _ => {}
        }
    }

    pub(crate) fn read_bytes(&self, m: &mut Machine, a: u32, n: usize) -> Vec<u8> {
        let mut out = vec![0u8; n];
        for (i, o) in out.iter_mut().enumerate() {
            match m.read(a.wrapping_add(i as u32), 1) {
                Ok(v) => *o = v as u8,
                Err(e) => {
                    let mut r = self.b();
                    if r.pending.is_none() {
                        r.pending = Some(e);
                    }
                    *o = 0;
                }
            }
        }
        out
    }

    pub(crate) fn adv_tx_start(&self, m: &mut Machine, cmd: u8, _t0: f64) {
        let t = m.ms();
        let a = 0x840000 | le16(&m.regs, 0xC0C) as u32;
        let raw = self.read_bytes(m, a, 6 + 40);
        let pdu = raw[4..6 + raw[5] as usize].to_vec();
        let ch = m.regs[0x40D];
        self.b().adv_tx.push(AdvTx { ms: t, ch, pdu: pdu.clone() });
        let cb = self.b().on_air_tx.clone();
        if let Some(cb) = cb {
            cb(t, ch, le32(&m.regs, 0x408), pdu.clone());
        }
        // What the chip sends with packets still in the TX FIFO is not documented.
        let tf = tpll_format(m);
        let first_adv_fifo = {
            let mut r = self.b();
            if r.rptr != r.wptr && !tf {
                let (rp, wp) = (r.rptr, r.wptr);
                r.adv_fifo.push(AdvFifo { ms: t, ch, rptr: rp, wptr: wp });
                let why = format!("advertising with {} packet(s) in the TX FIFO (0xc2a {}, 0xc2b {})", (wp - rp) & 0xF, rp, wp);
                r.misses.push(Miss { ms: t, ch: ch as i64, why: why.clone() });
                if r.adv_fifo.len() == 1 {
                    Some(why)
                } else {
                    None
                }
            } else {
                None
            }
        };
        if let (Some(why), Some(ev)) = (first_adv_fifo, m.event.clone()) {
            ev(m, &format!("radio: {}; what the chip sends then is not documented", why));
        }
        Self::set_status(m, TX);
        let mut centrals = self.offered();
        if !centrals.is_empty() {
            let lost = {
                let mut r = self.b();
                match r.air.as_mut() {
                    Some(air) => air.lost(TO_HOST, ch, t),
                    None => false,
                }
            };
            if lost {
                self.log(|| format!("[{:9.3}] radio: advertising packet on ch {} lost on air", t, ch));
                centrals.clear();
            }
        }
        if cmd == 0x87 && !centrals.is_empty() && m.regs[0x408..0x40C] == ADV_AA {
            let mut who: Option<PeerRc> = None;
            let mut rsp: Option<Vec<u8>> = None;
            for c in &centrals {
                rsp = c.on_adv(m, &pdu, ch);
                if rsp.is_some() {
                    who = Some(c.clone());
                    break;
                }
            }
            if rsp.is_none() {
                for c in &centrals {
                    rsp = c.scan_request(m, &pdu, ch);
                    if let Some(s) = &rsp {
                        who = Some(c.clone());
                        self.b().scan_reqs.push(ScanReq { ms: t, ch, central: c.clone(), pdu: s.clone() });
                        break;
                    }
                }
            }
            if let Some(rsp) = rsp {
                let t_rx = t + (airtime_us(&pdu) + 150) as f64 / 1000.0;
                let lost = {
                    let mut r = self.b();
                    match r.air.as_mut() {
                        Some(air) => air.lost(TO_KEYBOARD, ch, t_rx),
                        None => false,
                    }
                };
                if lost {
                    self.log(|| format!("[{:9.3}] radio: answer to the advertising packet on ch {} lost on air", t_rx, ch));
                    if rsp[0] & 0xF == 5 {
                        let who = who.unwrap();
                        let at = t_rx + airtime_us(&rsp) as f64 / 1000.0;
                        self.at(at, Box::new(move |m| who.on_connect_sent(m, &rsp, m.ms())), "radio");
                    }
                    return;
                }
                // The DMA streams the header in as the packet arrives; RX status at its end.
                let me = self.clone();
                let rsp1 = rsp.clone();
                self.at(
                    t_rx + 0.056,
                    Box::new(move |m| {
                        me.dma_rx(m, &rsp1, t_rx, false);
                    }),
                    "radio",
                );
                let me = self.clone();
                let at = t_rx + airtime_us(&rsp) as f64 / 1000.0;
                self.at(at, Box::new(move |m| me.adv_rx_end(m, &rsp, t_rx, who)), "radio");
            }
        }
    }

    /// The centrals that hear advertising, in the order they answer: Central, then Centrals.
    pub fn offered(&self) -> Vec<PeerRc> {
        let r = self.b();
        let mut out = Vec::new();
        if let Some(c) = &r.central {
            out.push(c.clone());
        }
        for c in &r.centrals {
            if !r.central.as_ref().is_some_and(|x| same_peer(x, c)) {
                out.push(c.clone());
            }
        }
        out
    }

    /// The central of the connection: the one whose CONNECT_IND was sent, while it is still attached, else Central.
    pub fn link_central(&self) -> Option<PeerRc> {
        let cc = self.b().conn_central.clone();
        if let Some(c) = cc {
            for x in self.offered() {
                if same_peer(&x, &c) {
                    return Some(c);
                }
            }
        }
        self.b().central.clone()
    }

    pub(crate) fn timeout(&self, m: &mut Machine, b: &BrxRc, bit: u32) {
        let hit = {
            let mut r = self.b();
            if r.brx.as_ref().is_some_and(|x| Rc::ptr_eq(x, b)) {
                r.brx = None;
                true
            } else {
                false
            }
        };
        if hit {
            Self::set_status(m, bit);
            let t = m.ms();
            self.log(|| format!("[{:9.3}] radio: BRX timeout 0x{:x}", t, bit));
        }
    }

    pub(crate) fn dma_rx(&self, m: &mut Machine, pdu: &[u8], t_start: f64, status: bool) -> u32 {
        let a = 0x840000 | le16(&m.regs, 0xC08) as u32;
        let ln = pdu[1] as usize;
        let anchor = m.stimer_now().wrapping_sub(((m.ms() - t_start) * 1000.0 * TICKS_PER_US) as i64 as u32);
        let ts = anchor.wrapping_add(0x500);
        let mut buf = Vec::with_capacity(ln + 17);
        buf.extend_from_slice(&((ln + 13) as u32).to_le_bytes());
        buf.extend_from_slice(pdu);
        buf.extend_from_slice(&[0, 0, 0]);
        buf.extend_from_slice(&ts.to_le_bytes());
        buf.extend_from_slice(&[0, 0, 0xC8, 0]);
        for (i, &v) in buf.iter().enumerate() {
            if let Err(e) = m.write(a.wrapping_add(i as u32), 1, v as u32) {
                let mut r = self.b();
                if r.pending.is_none() {
                    r.pending = Some(e);
                }
            }
        }
        self.b().ts = ts;
        if status {
            Self::set_status(m, RX);
        }
        a
    }

    pub(crate) fn adv_rx_end(&self, m: &mut Machine, pdu: &[u8], _t_rx: f64, who: Option<PeerRc>) {
        Self::set_status(m, RX);
        if let Some(who) = who {
            if pdu[0] & 0xF == 5 {
                self.b().conn_central = Some(who.clone());
                let t = m.ms();
                who.on_connect_sent(m, pdu, t);
            }
        }
    }

    // ------------------------------------------------------------ connection events

    /// The central starts sending pdu now; false if the peripheral misses it.
    pub fn central_tx(&self, m: &mut Machine, pdu: &[u8], ch: u8, aa: &[u8]) -> bool {
        let t = m.ms();
        let b = {
            let mut r = self.b();
            r.tx_lost = false;
            r.brx.clone()
        };
        let why = match &b {
            // (in 802.15.4 mode: a combined chip's other radio user had the
            // radio, or its main loop was away for the whole event)
            None if m.regs[0x404] & 0x03 == 0 => "peripheral not listening (radio in 802.15.4 mode)".to_string(),
            None => "peripheral not listening".to_string(),
            Some(b) => {
                let bb = b.borrow();
                if bb.ch != ch {
                    format!("peripheral on channel {}, central on {}", bb.ch, ch)
                } else if bb.aa != aa {
                    format!("access address {} != {}", hex(&bb.aa), hex(aa))
                } else if m.stimer_now().wrapping_sub(bb.start) >= 0x80000000 {
                    "BRX scheduled later".to_string()
                } else {
                    String::new()
                }
            }
        };
        if !why.is_empty() {
            self.log(|| format!("[{:9.3}] radio: central packet on ch {} missed: {}", t, ch, why));
            self.b().misses.push(Miss { ms: t, ch: ch as i64, why });
            return false;
        }
        let b = b.unwrap();
        let lost = {
            let mut r = self.b();
            match r.air.as_mut() {
                Some(air) => air.lost(TO_KEYBOARD, ch, t),
                None => false,
            }
        };
        if lost {
            self.b().tx_lost = true;
            self.log(|| format!("[{:9.3}] radio: central packet on ch {} lost on air", t, ch));
            return false;
        }
        let end = t + airtime_us(pdu) as f64 / 1000.0;
        {
            let mut r = self.b();
            let bt = b.borrow().t;
            r.leads.push(t - bt);
            let start = b.borrow().start;
            r.rx_on.push(m.stimer_now().wrapping_sub(start) as f64 / TICKS_PER_US);
        }
        b.borrow_mut().on_air = Some([t, end]);
        let me = self.clone();
        let pdu = pdu.to_vec();
        self.at(end, Box::new(move |m| me.rx_done(m, &b, &pdu, t)), "radio");
        true
    }

    pub(crate) fn is_brx(&self, b: &BrxRc) -> bool {
        self.b().brx.as_ref().is_some_and(|x| Rc::ptr_eq(x, b))
    }

    pub(crate) fn rx_done(&self, m: &mut Machine, b: &BrxRc, pdu: &[u8], t_start: f64) {
        if !self.is_brx(b) {
            let ms = m.ms();
            let ch = b.borrow().ch as i64;
            self.b().misses.push(Miss { ms, ch, why: "BRX stopped during the packet".into() });
            return;
        }
        let hdr0 = pdu[0];
        let (rx_sn, rx_nesn) = (((hdr0 >> 3) & 1) as i64, ((hdr0 >> 2) & 1) as i64);
        let (tx_addr, have_tx, more, tx_sn) = {
            let mut r = self.b();
            let mut bb = b.borrow_mut();
            if rx_nesn != bb.last_sn {
                if r.rptr != r.wptr {
                    r.rptr = (r.rptr + 1) & 0xF;
                }
                bb.last_sn ^= 1;
            }
            let mut tx_addr = 0u32;
            let have_tx = r.rptr != r.wptr;
            if have_tx {
                tx_addr = 0x840000 | r.fifo[r.rptr as usize] as u32;
            }
            let more = have_tx && ((r.rptr + 1) & 0xF) != r.wptr;
            if rx_sn == bb.nesn {
                bb.nesn ^= 1;
            }
            let tx_sn = bb.last_sn;
            r.sn_rb = tx_sn;
            r.nesn_rb = bb.nesn;
            bb.rx += 1;
            (tx_addr, have_tx, more, tx_sn)
        };
        self.dma_rx(m, pdu, t_start, true);
        let me = self.clone();
        let bb = b.clone();
        self.at(m.ms() + 0.150, Box::new(move |m| me.tx_start(m, &bb, tx_addr, have_tx, tx_sn, more)), "radio");
    }

    pub(crate) fn tx_start(&self, m: &mut Machine, b: &BrxRc, mut tx_addr: u32, have_tx: bool, tx_sn: i64, more: bool) {
        if !self.is_brx(b) {
            return;
        }
        if !have_tx {
            tx_addr = 0x840000 | le16(&m.regs, 0xC0C) as u32;
        }
        let hl = self.read_bytes(m, tx_addr.wrapping_add(4), 2);
        let (hdr0, ln) = (hl[0], hl[1] as usize);
        let payload = self.read_bytes(m, tx_addr.wrapping_add(6), ln);
        let md = if more || hdr0 & 0x10 != 0 { 1u8 } else { 0 };
        let nesn = b.borrow().nesn as u8;
        let mut pdu = vec![(hdr0 & 0xE3) | (nesn << 2) | ((tx_sn as u8) << 3) | (md << 4), ln as u8];
        pdu.extend_from_slice(&payload);
        let me = self.clone();
        let bb = b.clone();
        let at = m.ms() + airtime_us(&pdu) as f64 / 1000.0;
        self.at(at, Box::new(move |m| me.tx_done(m, &bb, &pdu, tx_addr)), "radio");
    }

    pub(crate) fn tx_done(&self, m: &mut Machine, b: &BrxRc, pdu: &[u8], tx_addr: u32) {
        if !self.is_brx(b) {
            return;
        }
        Self::set_status(m, TX);
        let c = self.link_central();
        if let Some(c) = &c {
            if c.drop_pdu(m, pdu) {
                self.end_event_at(m, b);
                return;
            }
        }
        if let Some(c) = &c {
            let bch = b.borrow().ch;
            let now = m.ms();
            let lost = {
                let mut r = self.b();
                match r.air.as_mut() {
                    Some(air) => air.lost(TO_HOST, bch, now),
                    None => false,
                }
            };
            if lost {
                self.log(|| format!("[{:9.3}] radio: reply on ch {} lost on air", now, bch));
                c.air_lost(m, pdu);
                self.end_event_at(m, b);
                return;
            }
        }
        let mut nxt = None;
        if let Some(c) = &c {
            let bch = b.borrow().ch;
            nxt = c.on_air(m, pdu, bch, tx_addr);
        }
        match nxt {
            Some(nxt) => {
                let me = self.clone();
                let bb = b.clone();
                let c = c.unwrap();
                self.at(m.ms() + 0.150, Box::new(move |m| me.central_next(m, &c, &nxt, &bb)), "radio");
            }
            None => self.end_event_at(m, b),
        }
    }

    pub(crate) fn end_event_at(&self, m: &Machine, b: &BrxRc) {
        let me = self.clone();
        let bb = b.clone();
        self.at(m.ms() + 0.010, Box::new(move |m| me.end_event(m, &bb)), "radio");
    }

    /// The central's next packet in the event; one lost on air is told to it.
    pub(crate) fn central_next(&self, m: &mut Machine, c: &PeerRc, pdu: &[u8], b: &BrxRc) {
        let (ch, aa) = {
            let bb = b.borrow();
            (bb.ch, bb.aa.clone())
        };
        if !self.central_tx(m, pdu, ch, &aa) && self.b().tx_lost {
            c.air_lost_tx(m, pdu);
        }
    }

    pub(crate) fn end_event(&self, m: &mut Machine, b: &BrxRc) {
        let hit = {
            let mut r = self.b();
            if r.brx.as_ref().is_some_and(|x| Rc::ptr_eq(x, b)) {
                r.brx = None;
                true
            } else {
                false
            }
        };
        if hit {
            Self::set_status(m, DONE);
        }
    }

    // ------------------------------------------------------------ TPLL

    /// The air time of a packet with n payload bytes.
    pub fn tpll_airtime_us(m: &Machine, n: usize) -> f64 {
        let rg = &m.regs;
        let mbps = if rg[0x1220] == 0x04 { 2.0 } else { 1.0 };
        ((((rg[0x402] & 0x1F) as i64 + (rg[0x405] & 7) as i64 + 1 + n as i64 + 2) * 8) as f64) / mbps
    }

    pub(crate) fn srx_timeout(&self, m: &mut Machine, s: &SrxRc) {
        let hit = {
            let mut r = self.b();
            if r.srx.as_ref().is_some_and(|x| Rc::ptr_eq(x, s)) {
                r.srx = None;
                true
            } else {
                false
            }
        };
        if hit {
            Self::set_status(m, FIRSTTO);
        }
    }

    pub(crate) fn stx2rx_timeout(&self, m: &mut Machine, s: &SrxRc) {
        let hit = {
            let mut r = self.b();
            if r.srx.as_ref().is_some_and(|x| Rc::ptr_eq(x, s)) {
                r.srx = None;
                true
            } else {
                false
            }
        };
        if hit {
            Self::set_status(m, RXTO);
        }
    }

    pub(crate) fn tpll_tx_start(&self, m: &mut Machine, stx2rx: bool) {
        let a = 0x840000 | le16(&m.regs, 0xC0C) as u32;
        let n = (self.read_bytes(m, a.wrapping_add(4), 1)[0] & 0x3F) as usize;
        let payload = self.read_bytes(m, a.wrapping_add(5), n);
        let (mhz, ac) = (tpll_mhz(m), tpll_access_code(m));
        self.b().tpll_tx.push(TpllTx { ms: m.ms(), mhz, ac: ac.clone(), payload: payload.clone() });
        let me = self.clone();
        let at = m.ms() + Self::tpll_airtime_us(m, n) / 1000.0;
        self.at(at, Box::new(move |m| me.tpll_tx_done(m, &payload, mhz, &ac, stx2rx)), "radio");
    }

    pub(crate) fn tpll_tx_done(&self, m: &mut Machine, payload: &[u8], mhz: i64, ac: &[u8], stx2rx: bool) {
        Self::set_status(m, TX);
        if stx2rx {
            let s = Rc::new(RefCell::new(Srx { t: m.ms(), start: m.stimer_now(), mhz, ac: ac.to_vec(), rx: 0 }));
            self.b().srx = Some(s.clone());
            if m.regs[0xF03] & 4 != 0 {
                let us = le16(&m.regs, 0xF0A) as u32 + 1;
                let me = self.clone();
                self.at(
                    m.ms() + us as f64 / 1000.0,
                    Box::new(move |m| {
                        if s.borrow().rx == 0 {
                            me.stx2rx_timeout(m, &s);
                        }
                    }),
                    "radio",
                );
            }
        }
        let peer = self.b().tpll.clone();
        let reply = match peer {
            Some(p) => p.on_tpll(m, payload, mhz, ac),
            None => None,
        };
        if let Some(reply) = reply {
            let me = self.clone();
            let ac = ac.to_vec();
            self.at(m.ms() + TURNAROUND_MS, Box::new(move |m| me.tpll_rx_start(m, &reply, mhz, &ac)), "radio");
        }
    }

    pub(crate) fn tpll_rx_start(&self, m: &mut Machine, reply: &[u8], mhz: i64, ac: &[u8]) {
        let t = m.ms();
        let s = self.b().srx.clone();
        let why = match &s {
            None => "not listening".to_string(),
            Some(s) => {
                let ss = s.borrow();
                if ss.mhz != mhz {
                    format!("listening on {} MHz", ss.mhz)
                } else if ss.ac != ac {
                    format!("access code {} != {}", hex(&ss.ac), hex(ac))
                } else if m.stimer_now().wrapping_sub(ss.start) >= 0x80000000 {
                    "SRX scheduled later".to_string()
                } else {
                    String::new()
                }
            }
        };
        if !why.is_empty() {
            self.log(|| format!("[{:9.3}] radio: reply on {} MHz missed: {}", t, mhz, why));
            self.b().misses.push(Miss { ms: t, ch: mhz, why });
            return;
        }
        let s = s.unwrap();
        s.borrow_mut().rx += 1;
        let me = self.clone();
        let reply = reply.to_vec();
        let at = t + Self::tpll_airtime_us(m, reply.len()) / 1000.0;
        self.at(at, Box::new(move |m| me.tpll_rx_done(m, &s, &reply, t)), "radio");
    }

    pub(crate) fn tpll_rx_done(&self, m: &mut Machine, s: &SrxRc, reply: &[u8], t_start: f64) {
        let cur = self.b().srx.as_ref().is_some_and(|x| Rc::ptr_eq(x, s));
        if !cur {
            let ms = m.ms();
            let mhz = s.borrow().mhz;
            self.b().misses.push(Miss { ms, ch: mhz, why: "SRX stopped during the packet".into() });
            return;
        }
        let a = 0x840000 | le16(&m.regs, 0xC08) as u32;
        let n = reply.len();
        let anchor = m.stimer_now().wrapping_sub(((m.ms() - t_start) * 1000.0 * TICKS_PER_US) as i64 as u32);
        let mut buf = Vec::new();
        buf.extend_from_slice(&((n + 11) as u32).to_le_bytes());
        buf.push(n as u8);
        buf.extend_from_slice(reply);
        buf.extend_from_slice(&[0, 0]);
        buf.extend_from_slice(&anchor.to_le_bytes());
        buf.extend_from_slice(&[0, 0, 0xC8, 0]);
        for (i, &v) in buf.iter().enumerate() {
            if let Err(e) = m.write(a.wrapping_add(i as u32), 1, v as u32) {
                let mut r = self.b();
                if r.pending.is_none() {
                    r.pending = Some(e);
                }
            }
        }
        self.b().srx = None;
        Self::set_status(m, RX);
    }

    // ------------------------------------------------------------ air between nodes

    /// The chip hears a packet on channel ch with access address aa now.
    pub fn air_rx_open(&self, m: &Machine, ch: u8, aa: u32) -> bool {
        let rg = &m.regs;
        let r = self.b();
        if rg[0x404] & 3 != 1 || r.brx.is_some() {
            return false;
        }
        let manual = rg[0xF02] & 0x20 != 0 && rg[0x428] & 1 != 0 && rg[0xF02] & 0x10 == 0;
        if !r.ble_srx && !manual {
            return false;
        }
        rg[0x40D] == ch && le32(rg, 0x408) == aa
    }

    /// Another node's packet is on air from t0 to t1 (ms).
    pub fn air_rx(&self, m: &Machine, pdu: Vec<u8>, ch: u8, aa: u32, mut t0: f64, mut t1: f64) {
        let now = m.ms();
        if t0 < now {
            t1 += now - t0;
            t0 = now;
        }
        let me = self.clone();
        self.at(
            t0,
            Box::new(move |m| {
                if !me.air_rx_open(m, ch, aa) {
                    if aa != 0x8E89BED6 {
                        me.b().air_rx_miss.push(Miss {
                            ms: round3(t0),
                            ch: ch as i64,
                            why: "not listening (channel, access address or RX off)".into(),
                        });
                    }
                    return;
                }
                let me2 = me.clone();
                me.at(
                    t1,
                    Box::new(move |m| {
                        if !me2.air_rx_open(m, ch, aa) {
                            me2.b().air_rx_miss.push(Miss { ms: round3(t1), ch: ch as i64, why: "RX closed during the packet".into() });
                            return;
                        }
                        me2.dma_rx(m, &pdu, t0, true);
                        let mut r = me2.b();
                        r.ble_srx = false;
                        r.air_rx_ok += 1;
                    }),
                    "radio",
                );
            }),
            "radio",
        );
    }
}

pub(crate) fn tpll_format(m: &Machine) -> bool {
    m.regs[0x404] & 3 == 2
}

pub(crate) fn tpll_mhz(m: &Machine) -> i64 {
    let rg = &m.regs;
    ((rg[0x1245] & 0x3F) as i64) << 6 | (rg[0x1244] >> 2) as i64
}

pub(crate) fn tpll_access_code(m: &Machine) -> Vec<u8> {
    let rg = &m.regs;
    rg[0x408..0x408 + (rg[0x405] & 7) as usize].to_vec()
}
