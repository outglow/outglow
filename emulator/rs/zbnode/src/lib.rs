// SPDX-License-Identifier: Apache-2.0
//! zbnode as a library: a TLSR8258 firmware image in the tc32emu port with the
//! 8258 layer, the 802.15.4 radio and the BLE model.
//!
//! A [`Node`] holds one emulated bulb. The `zbnode` binary drives it over the
//! stdin/stdout protocol of the scenario scripts (see `main.rs` and
//! `emulator/go/cmd/zbnode/main.go`); in-process users (the Rust scenario
//! harness) call it directly. Both see the same behaviour: the setup below is
//! the one the binary always did, in the same order, so the hooks chain the
//! same way.

pub mod bleatt;
pub mod blespec;
pub mod gostr;
pub mod radio154;
pub mod regtrace;

static PROF_SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::env;
use std::process;
use std::rc::Rc;

use ble::gofmt::{hex, unhex};
use ble::{airtime_us, BrxRc, RadioRef};
use tc32emu::{stop_err, Error, Flash, Machine};

use crate::gostr::{atoi, hex_i64, parse_uint_lossy};
pub use crate::radio154::{Frame, Radio154};
use crate::radio154::install_8258;

/// must: print the error and exit 1.
pub fn must<T, E: std::fmt::Display>(r: Result<T, E>) -> T {
    match r {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{}", e);
            process::exit(1);
        }
    }
}

/// BLE CRC24 in reflected form (polynomial 0x00065B -> 0xDA6000), start =
/// CRCInit from 0x424..0x426 with the order of all 24 bits reversed.
fn ble24_crc(init: u32, data: &[u8]) -> u32 {
    let rev8 = |b: u8| -> u32 { b.reverse_bits() as u32 };
    let mut c = rev8(init as u8) << 16 | rev8((init >> 8) as u8) << 8 | rev8((init >> 16) as u8);
    for &d in data {
        c ^= d as u32;
        for _ in 0..8 {
            if c & 1 != 0 {
                c = c >> 1 ^ 0xDA6000;
            } else {
                c >>= 1;
            }
        }
    }
    c & 0xFFFFFF
}

/// A BLE packet the node put on the air.
pub struct AirTx {
    pub t0: f64,
    pub t1: f64,
    pub ch: u8,
    pub aa: u32,
    pub pdu: Vec<u8>,
}

/// `fmt.Sscanf(v, "%f,%f", ...)`: the values read before the first failure.
fn scan_two_f(v: &str) -> (f64, f64) {
    let mut it = v.splitn(2, ',');
    let a = it.next().and_then(|s| s.trim().parse::<f64>().ok());
    let Some(a) = a else { return (0.0, 0.0) };
    let b = it.next().and_then(|s| s.trim().parse::<f64>().ok()).unwrap_or(0.0);
    (a, b)
}

/// `fmt.Sscanf(v, "%x,%x", ...)`.
fn scan_two_x(v: &str) -> (u32, u32) {
    let p = |s: &str| -> Option<u32> {
        let s = s.trim();
        let s = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).unwrap_or(s);
        u32::from_str_radix(s, 16).ok()
    };
    let mut it = v.splitn(2, ',');
    let Some(a) = it.next().and_then(p) else { return (0, 0) };
    let b = it.next().and_then(p).unwrap_or(0);
    (a, b)
}

/// True when the environment variable is set to something non-empty.
pub fn env_set(name: &str) -> bool {
    env::var(name).map(|v| !v.is_empty()).unwrap_or(false)
}

/// What one step of [`Node::step_run`] produced.
pub enum RunEvent {
    /// The node transmitted an 802.15.4 frame (a run ends here).
    Tx(Frame),
    /// The node put a BLE packet on the air (a run ends here).
    BleTx(AirTx),
    /// The node reached the target time (a run ends here).
    At,
    /// The emulator ran; nothing to report yet, call again.
    Ran,
    /// The image rebooted into code it copied to SRAM.
    SramBoot { pc: u32, ram_code: usize },
    /// A watchdog or software reset; the node restarted.
    Reset { cause: String, pc: u32 },
    /// An emulation error: the node cannot continue.
    Error(String),
    /// Restarting the machine failed (what `must` reports).
    Fatal(String),
}

/// One emulated bulb: the machine, its 802.15.4 radio and its BLE radio.
pub struct Node {
    pub m: Machine,
    pub radio: Radio154,
    pub ble_radio: RadioRef,
    /// BLE packets put on the air, not yet handed out.
    pub air_pending: Rc<RefCell<VecDeque<AirTx>>>,
    /// Size of the SRAM code area after an SRAM boot (0: none).
    pub sram_boot: Rc<Cell<usize>>,
    sram_boot_pending: Rc<Cell<bool>>,
    /// Watchdog and software resets so far.
    pub resets: i64,
    /// Corrupt the CRC24 of every n-th received BLE packet (0: never).
    pub crc_err_every: Rc<Cell<i64>>,
    pub crc_errs: Rc<Cell<i64>>,
    /// ZBNODE_TRACE is on: the read hook may print for any address.
    read_hook_traces: bool,
}

impl Node {
    /// Loads `img` at 0 of a 1 MB flash (and again at `link_shift` when it is
    /// positive), with the debug options of the ZBNODE_* environment.
    pub fn new(img: &[u8], cpi: i64, link_shift: i64) -> Result<Node, String> {
        let mut fl = Flash::new(1 << 20);
        let n = img.len().min(fl.mem.len());
        fl.mem[..n].copy_from_slice(&img[..n]);
        if let Ok(u) = env::var("ZBNODE_FLASH_UID") {
            if !u.is_empty() {
                let (b, ok) = unhex(&u);
                if ok {
                    for i in 0..fl.uid.len().min(b.len()) {
                        fl.uid[i] = b[i];
                    }
                }
            }
        }
        if link_shift > 0 {
            let sh = link_shift as usize;
            let n = img.len().min(fl.mem.len().saturating_sub(sh));
            fl.mem[sh..sh + n].copy_from_slice(&img[..n]);
        }
        let mut mm = Machine::new(fl, 0, cpi).map_err(|e| e.to_string())?;
        let m = &mut mm;
        m.event = Some(Rc::new(|_: &mut Machine, _: &str| {}));
        m.reg_write_hook = Some(install_8258(None));

        let sram_boot = Rc::new(Cell::new(0usize));
        let sram_boot_pending = Rc::new(Cell::new(false));
        let prev_w = m.reg_write_hook.clone().unwrap();
        let rf_log = env_set("ZBNODE_RF_LOG");
        if env_set("ZBNODE_FLASH_LOG") {
            m.on_flash_write = Some(Rc::new(|m: &mut Machine, op: &str, addr: usize, n: usize| {
                eprintln!("[{:10.3}] flash {:<9} 0x{:07x} len {:<5} pc={:#x} lr={:#x}", m.ms(), op, addr, n, m.r[15], m.r[14]);
            }));
        }
        let (tr_t0, tr_t1) = match env::var("ZBNODE_TRACE") {
            Ok(v) if !v.is_empty() => scan_two_f(&v),
            _ => (0.0, 0.0),
        };
        let in_trace = move |m: &Machine| tr_t1 > 0.0 && m.ms() >= tr_t0 && m.ms() <= tr_t1;
        let (tr_m0, tr_m1) = match env::var("ZBNODE_TRACE_MEM") {
            Ok(v) if !v.is_empty() => scan_two_x(&v),
            _ => (0, 0),
        };
        let ana_log = env_set("ZBNODE_ANA_LOG");
        {
            let sbp = sram_boot_pending.clone();
            m.reg_write_hook = Some(Rc::new(move |m: &mut Machine, o: usize, size: usize, val: u32| {
                if ana_log && (o == 0xb8 || o == 0xb9 || o == 0xba || o == 0x6f) {
                    eprintln!("[{:10.3}] ana/rst w 0x{:04x} = {:#x} pc={:#x}", m.ms(), o, val, m.r[15]);
                }
                if rf_log
                    && (o == 0xF00
                        || o == 0xF16
                        || o == 0x404
                        || o == 0xF02
                        || o == 0x74C
                        || o == 0x744
                        || o == 0x748
                        || o == 0x640
                        || o == 0x641
                        || o == 0x642
                        || (o == 0xC24 && val & 8 != 0))
                {
                    eprintln!("[{:10.3}] rf w 0x{:05x} = {:#x} pc={:#x}", m.ms(), o, val, m.r[15]);
                }
                if in_trace(m) && (0x400..0x1300).contains(&o) {
                    eprintln!("[{:10.4}] W 0x{:05x}/{} = {:#x} pc={:#x}", m.ms(), o, size, val, m.r[15]);
                }
                if o == 0x602 && size == 1 && val == 0x88 {
                    sbp.set(true);
                    return Err(stop_err("sram boot"));
                }
                prev_w(m, o, size, val)
            }));
        }
        // A write to a mapped flash address does nothing on a real TC32: ignored.
        let flash_w_log = env_set("ZBNODE_FLASHW_LOG");
        let prev_mw = m.mem_write_hook.clone();
        let mem_watch = match env::var("ZBNODE_MEMW") {
            Ok(v) if !v.is_empty() => parse_uint_lossy(v.strip_prefix("0x").unwrap_or(&v), 16, 32) as u32,
            _ => 0,
        };
        let flash_len = m.flash.mem.len() as u32;
        // SRAM lies above the flash: there the hook below does nothing unless
        // it watches an address or chains to an earlier one.
        if mem_watch == 0 && prev_mw.is_none() {
            m.mem_write_hook_below = flash_len;
        }
        m.mem_write_hook = Some(Rc::new(move |m: &mut Machine, a: u32, size: usize, val: u32| {
            if mem_watch != 0 && a <= mem_watch && mem_watch < a.wrapping_add(size as u32) {
                eprintln!("[{:10.3}] MEMW {:#x} = {:#x} (sz {}) pc={:#x} lr={:#x}", m.ms(), a, val, size, m.r[15], m.r[14]);
            }
            if a.wrapping_add(size as u32) <= flash_len {
                if flash_w_log {
                    eprintln!("[{:10.3}] write{} to flash 0x{:07x} = {:#x} pc={:#x} (ignored)", m.ms(), size * 8, a, val, m.r[15]);
                }
                return Ok(true);
            }
            match &prev_mw {
                Some(p) => p(m, a, size, val),
                None => Ok(false),
            }
        }));
        {
            let sb = sram_boot.clone();
            m.mem_read_hook = Some(Rc::new(move |m: &mut Machine, a: u32, size: usize| {
                let pc = m.r[15];
                let traced = in_trace(m) && ((0x800400..0x801300).contains(&a) || (a >= tr_m0 && a < tr_m1));
                let res = if sb.get() > 0 && (a as usize) + size <= sb.get() {
                    let mut v = 0u32;
                    for i in 0..size {
                        v |= (m.sram[a as usize + i] as u32) << (8 * i);
                    }
                    Some(v)
                } else {
                    None
                };
                if traced {
                    eprintln!("[{:10.4}] R 0x{:05x}/{} pc={:#x}", m.ms(), a.wrapping_sub(0x800000), size, pc);
                }
                Ok(res)
            }));
        }
        // ZBNODE_PCTRACE_AT=0xADDR [ZBNODE_PCTRACE_N=count]: a dispatcher trace.
        if let Ok(at) = env::var("ZBNODE_PCTRACE_AT") {
            if !at.is_empty() {
                let a = hex_i64(&at);
                let mut budget = 4000i64;
                if let Ok(n) = env::var("ZBNODE_PCTRACE_N") {
                    if !n.is_empty() {
                        budget = atoi(&n);
                    }
                }
                let left = Rc::new(Cell::new(0i64));
                let l1 = left.clone();
                m.hooks.insert((a & !1) as u32, Rc::new(move |_m: &mut Machine| {
                    l1.set(budget);
                    Ok(())
                }));
                m.trace_step = Some(Rc::new(move |m: &mut Machine, pc: u32, hw: u16| {
                    if left.get() > 0 {
                        left.set(left.get() - 1);
                        eprintln!(
                            "PCT {:#x} hw={:04x} lr={:#x} r0={:#x} r1={:#x} r2={:#x} r3={:#x}",
                            pc, hw, m.r[14], m.r[0], m.r[1], m.r[2], m.r[3]
                        );
                    }
                }));
            }
        }
        // ZBNODE_CALLERS=0xADDR,...: on entry to each, counts the return
        // addresses (lr) and prints them when the node goes.
        if let Ok(list) = env::var("ZBNODE_CALLERS") {
            for at in list.split(',').filter(|x| !x.is_empty()) {
                struct Callers(u32, RefCell<std::collections::HashMap<u32, u64>>);
                impl Drop for Callers {
                    fn drop(&mut self) {
                        let mut v: Vec<_> = self.1.borrow().iter().map(|(a, b)| (*b, *a)).collect();
                        v.sort_by(|a, b| b.cmp(a));
                        let top: Vec<String> = v.iter().take(12).map(|(n, a)| format!("{a:#x}:{n}")).collect();
                        eprintln!("CALLERS of {:#x}: {}", self.0, top.join(" "));
                    }
                }
                let a = (hex_i64(at) & !1) as u32;
                let c = Rc::new(Callers(a, RefCell::new(std::collections::HashMap::new())));
                m.hooks.insert(a, Rc::new(move |m: &mut Machine| {
                    *c.1.borrow_mut().entry((m.r[14] & !1) ^ (m.r[2] << 20)).or_insert(0) += 1;
                    Ok(())
                }));
            }
        }
        // ZBNODE_PCPROF=file: instructions executed per address, written as
        // "pc count" lines when the node goes (profiling the firmware's time).
        if let Ok(path) = env::var("ZBNODE_PCPROF") {
            if !path.is_empty() && m.trace_step.is_none() {
                struct Prof {
                    path: String,
                    counts: RefCell<std::collections::HashMap<u32, u64>>,
                }
                impl Drop for Prof {
                    fn drop(&mut self) {
                        use std::io::Write;
                        let mut v: Vec<_> = self.counts.borrow().iter().map(|(a, b)| (*a, *b)).collect();
                        v.sort();
                        let id = std::process::id();
                        let n = PROF_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if let Ok(mut f) = std::fs::File::create(format!("{}.{id}.{n}", self.path)) {
                            for (a, c) in v {
                                let _ = writeln!(f, "{a:#x} {c}");
                            }
                        }
                    }
                }
                // the application's size from its header (0x6A000 + 0x18): which image this is
                let hdr = |i: usize| m.flash.mem.get(0x6A018 + i).copied().unwrap_or(0) as u32;
                let size = hdr(0) | hdr(1) << 8 | hdr(2) << 16 | hdr(3) << 24;
                let prof = Rc::new(Prof { path: format!("{path}.size{size}"), counts: RefCell::new(std::collections::HashMap::new()) });
                m.trace_step = Some(Rc::new(move |_m: &mut Machine, pc: u32, _hw: u16| {
                    *prof.counts.borrow_mut().entry(pc).or_insert(0) += 1;
                }));
            }
        }
        let ble_log: Option<Rc<dyn Fn(&str)>> = if env_set("ZBNODE_BLE_LOG") { Some(Rc::new(|s: &str| eprintln!("{}", s))) } else { None };
        let ble_radio = RadioRef::new(m, ble_log);
        // BLE air between nodes: every STX/STX2RX goes out as an AirTx.
        let air_pending: Rc<RefCell<VecDeque<AirTx>>> = Rc::new(RefCell::new(VecDeque::new()));
        {
            let ap = air_pending.clone();
            ble_radio.b().on_air_tx = Some(Rc::new(move |ms: f64, ch: u8, aa: u32, pdu: Vec<u8>| {
                let t1 = ms + airtime_us(&pdu) as f64 / 1000.0;
                ap.borrow_mut().push_back(AirTx { t0: ms, t1, ch, aa, pdu });
            }));
        }
        let radio = Radio154::new(m);
        // The CRC24 field of a received BLE packet, computed as it arrived on the air.
        let crc_err_every = Rc::new(Cell::new(0i64));
        let crc_errs = Rc::new(Cell::new(0i64));
        {
            let inner_t = m.update_time_hook.clone();
            let br = ble_radio.clone();
            let last_b: RefCell<Option<BrxRc>> = RefCell::new(None);
            let last_rx = Cell::new(0i64);
            let crc_count = Cell::new(0i64);
            let (every, errs) = (crc_err_every.clone(), crc_errs.clone());
            m.update_time_hook = Some(Rc::new(move |m: &mut Machine| {
                let res = match &inner_t {
                    Some(p) => p(m),
                    None => Ok(()),
                };
                let b = br.b().brx.clone();
                if let Some(b) = b {
                    let (rx, crc_init) = {
                        let bb = b.borrow();
                        (bb.rx, bb.crc_init)
                    };
                    let same = last_b.borrow().as_ref().is_some_and(|x| Rc::ptr_eq(x, &b));
                    if rx > 0 && (!same || rx != last_rx.get()) {
                        *last_b.borrow_mut() = Some(b.clone());
                        last_rx.set(rx);
                        let a = m.regs[0xC08] as usize | (m.regs[0xC09] as usize) << 8;
                        if a + 0x120 <= m.sram.len() {
                            let ln = m.sram[a + 5] as usize;
                            let mut crc = ble24_crc(crc_init, &m.sram[a + 4..a + 6 + ln]);
                            crc_count.set(crc_count.get() + 1);
                            if every.get() > 0 && crc_count.get() % every.get() == 0 {
                                crc ^= 1;
                                errs.set(errs.get() + 1);
                            }
                            m.sram[a + 6 + ln] = crc as u8;
                            m.sram[a + 7 + ln] = (crc >> 8) as u8;
                            m.sram[a + 8 + ln] = (crc >> 16) as u8;
                            if in_trace(m) {
                                eprintln!(
                                    "[{:10.4}] CRC buf={:#x} init={:x} crc={:06x} pkt={}",
                                    m.ms(),
                                    a,
                                    crc_init,
                                    crc,
                                    hex(&m.sram[a..a + 12 + ln])
                                );
                            }
                        }
                    }
                }
                res
            }));
        }
        // ZBNODE_REG_TRACE: installed last, so that it sees what the radio models handle themselves.
        let mut reg_trace = false;
        if let Ok(v) = env::var("ZBNODE_REG_TRACE") {
            if !v.is_empty() {
                regtrace::install(m, &v);
                reg_trace = true;
            }
        }
        // The register hooks above (802.15.4 radio -> BLE radio -> the write
        // hook in this function -> install_8258) pass every other register
        // access on: the machine may skip them there (tc32emu::RegHookMask).
        // Not with the register trace or the logs, which see every access.
        if !reg_trace && !ana_log && !rf_log && tr_t1 <= 0.0 && env::var("ZBNODE_REG_MASK").as_deref() != Ok("0") {
            let mut k = tc32emu::RegHookMask::default();
            let (r154_r, r154_w) = Radio154::hooked_regs();
            let (ble_r, ble_w) = RadioRef::hooked_regs();
            for &o in r154_r.iter().chain(ble_r.iter()) {
                k.mark_read(o);
            }
            for &o in r154_w.iter().chain(radio154::INSTALL_8258_WRITES.iter()).chain([0x602usize].iter()) {
                k.mark_write(o);
            }
            match ble_w {
                Some(w) => w.iter().for_each(|&o| k.mark_write(o)),
                None => k.mark_all_writes(),
            }
            m.reg_hook_mask = Some(Box::new(k));
        }
        let mut node = Node {
            m: mm,
            radio,
            ble_radio,
            air_pending,
            sram_boot,
            sram_boot_pending,
            resets: 0,
            crc_err_every,
            crc_errs,
            read_hook_traces: tr_t1 > 0.0,
        };
        node.sync_read_hook_bound();
        Ok(node)
    }

    /// The read hook above answers only below the SRAM code area's end and
    /// otherwise does nothing unless ZBNODE_TRACE is on: tell the machine, so
    /// that it skips the hook for the other reads (instruction fetches from
    /// flash above all). Called whenever `sram_boot` changes.
    fn sync_read_hook_bound(&mut self) {
        self.m.mem_read_hook_below = if self.read_hook_traces { u32::MAX } else { self.sram_boot.get() as u32 };
    }

    /// Simulated time of the node in ms.
    pub fn ms(&self) -> f64 {
        self.m.ms()
    }

    /// One step towards `target` ms. A run is a loop of steps that ends at
    /// [`RunEvent::Tx`], [`RunEvent::BleTx`] or [`RunEvent::At`]; the binary
    /// interleaves its BLE output between the steps.
    pub fn step_run(&mut self, target: f64) -> RunEvent {
        if let Some(fr) = self.radio.b().tx_pending.pop_front() {
            return RunEvent::Tx(fr);
        }
        if let Some(a) = self.air_pending.borrow_mut().pop_front() {
            return RunEvent::BleTx(a);
        }
        let m = &mut self.m;
        if m.ms() >= target {
            return RunEvent::At;
        }
        let err = m.run_ms(target, &[]);
        if self.sram_boot_pending.get() {
            self.sram_boot_pending.set(false);
            let mut n = (m.sram[0x0C] as usize | (m.sram[0x0D] as usize) << 8) * 16;
            if n > 0x4000 || n == 0 {
                n = 0x4000;
            }
            let saved = m.sram.clone();
            if let Err(e) = m.reset(0, false) {
                return RunEvent::Fatal(e.to_string());
            }
            let k = saved.len().min(m.sram.len());
            m.sram[..k].copy_from_slice(&saved[..k]);
            self.sram_boot.set(n);
            m.mem_read_hook_below = if self.read_hook_traces { u32::MAX } else { n as u32 };
            return RunEvent::SramBoot { pc: m.r[15], ram_code: n };
        }
        match err {
            Ok(()) | Err(Error::Stop(_)) => RunEvent::Ran,
            Err(e @ (Error::WatchdogReset | Error::SoftwareReset)) => {
                self.resets += 1;
                self.sram_boot.set(0);
                m.mem_read_hook_below = if self.read_hook_traces { u32::MAX } else { 0 };
                let pc = m.r[15];
                if let Err(e2) = m.reset(-1, matches!(e, Error::WatchdogReset)) {
                    return RunEvent::Fatal(e2.to_string());
                }
                RunEvent::Reset { cause: e.to_string(), pc }
            }
            Err(e) => RunEvent::Error(e.to_string()),
        }
    }

    /// An 802.15.4 frame arriving on the node's channel.
    pub fn inject_rx(&self, f: Frame, rssi: i64) {
        self.radio.inject(f, rssi);
    }

    /// A frame that reaches the radio with a bad FCS (`Radio154::inject_bad_fcs`).
    pub fn inject_rx_bad_fcs(&self, f: Frame, rssi: i64) {
        self.radio.inject_bad_fcs(f, rssi);
    }

    /// Power off and on: the retention registers lose their content.
    pub fn reboot(&mut self) -> Result<(), String> {
        self.sram_boot.set(0);
        self.sync_read_hook_bound();
        self.m.analog[0x3A] = 0;
        self.m.analog[0x3B] = 0;
        self.m.analog[0x3C] = 0x0F;
        self.m.reset(-1, false).map_err(|e| e.to_string())
    }

    /// A software reset (a watchdog reset with `wd`).
    pub fn soft_reset(&mut self, wd: bool) -> Result<(), String> {
        self.sram_boot.set(0);
        self.sync_read_hook_bound();
        self.m.reset(-1, wd).map_err(|e| e.to_string())
    }

    /// WRITEFLASH: erases the 4 KB sectors the data touches, then writes it.
    pub fn write_flash(&mut self, a: usize, data: &[u8]) {
        let m = &mut self.m;
        let end = a + data.len();
        let mut sec = a & !0xFFF;
        while sec < end {
            for i in sec..sec + 0x1000 {
                m.flash.mem[i] = 0xFF;
            }
            sec += 0x1000;
        }
        let n = data.len().min(m.flash.mem.len().saturating_sub(a));
        m.flash.mem[a..a + n].copy_from_slice(&data[..n]);
    }

    /// ERASE: n bytes from a set to 0xFF.
    pub fn erase(&mut self, a: usize, n: usize) {
        for i in a..a + n {
            self.m.flash.mem[i] = 0xFF;
        }
    }

    /// CALL: a firmware function called directly: r0-r3 from `args`, LR at a
    /// return trap, PC at `addr`, stepped until it returns (or `budget`
    /// instructions); the registers and flags are restored afterwards.
    pub fn call(&mut self, addr: u32, args: &[u32], budget: i64) -> CallOutcome {
        let m = &mut self.m;
        const TRAP: u32 = 0xFFFFFFFE;
        for (i, a) in args.iter().take(4).enumerate() {
            m.r[i] = *a;
        }
        let saved_r = m.r;
        let (sn, sz, sc, sv) = (m.n, m.z, m.c, m.v);
        m.r[14] = TRAP;
        m.r[15] = addr & !1;
        let mut cerr: Option<Error> = None;
        let mut steps = 0i64;
        while steps < budget {
            if m.r[15] == (TRAP & !1) {
                break;
            }
            if let Err(e) = m.step() {
                cerr = Some(e);
                break;
            }
            steps += 1;
        }
        let (r0, r1, r2, r3, rpc) = (m.r[0], m.r[1], m.r[2], m.r[3], m.r[15]);
        m.r = saved_r;
        m.n = sn;
        m.z = sz;
        m.c = sc;
        m.v = sv;
        match cerr {
            Some(e) => CallOutcome::Err { r0, steps, pc: rpc, err: e.to_string() },
            None if steps >= budget => CallOutcome::Timeout { r0, steps, pc: rpc },
            None => CallOutcome::Done { r: [r0, r1, r2, r3], pc: rpc, steps },
        }
    }
}

/// What `Node::call` ended with.
#[derive(Clone, Debug)]
pub enum CallOutcome {
    Done { r: [u32; 4], pc: u32, steps: i64 },
    Timeout { r0: u32, steps: i64, pc: u32 },
    Err { r0: u32, steps: i64, pc: u32, err: String },
}
