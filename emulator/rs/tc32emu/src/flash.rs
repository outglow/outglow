// SPDX-License-Identifier: Apache-2.0
//! An SPI NOR flash behind the MSPI registers, and the XIP backing store
//! (flash.go).

use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::sync::OnceLock;

use crate::config::Config;
use crate::error::{emu_err, Error};
use crate::machine::FLASH_MEM;

/// Flash.OnWrite: (op, addr, n) as a page program or an erase is carried out
/// ("program", "erase") or refused by the block protection ("protected").
pub type OnWriteFn = Rc<dyn Fn(&str, usize, usize)>;

/// One entry of Flash.log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlashOp {
    pub op: String,
    pub addr: i64,
    pub len: i64,
    /// Status writes: the bytes after the command, hex.
    pub data: String,
}

impl FlashOp {
    fn new(op: &str, addr: i64, len: i64, data: String) -> FlashOp {
        FlashOp { op: op.to_string(), addr, len, data }
    }
}

/// A protected address range, first to last inclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ProtectedAddrs {
    pub first: i64,
    pub last: i64,
}

pub struct Flash {
    /// FlashMem bytes, whatever the part's size.
    pub mem: Vec<u8>,
    /// The part's size: what lies at or above it is refused.
    pub size: usize,
    /// The address of the byte last clocked out if the part does not have it, else -1.
    pub beyond: i64,
    /// "stop", "ff" or "wrap" (TC32EMU_FLASH_BEYOND).
    pub beyond_mode: String,
    pub jedec: [u8; 3],
    pub mid: i64,
    /// SFDP data; empty for a part without (Go nil).
    pub sfdp: Vec<u8>,
    pub part: i64,
    /// The 16-bit status register.
    pub status: i64,
    pub uid: Vec<u8>,
    pub(crate) wel: bool,
    pub(crate) cs_low: bool,
    pub(crate) tx: Vec<u8>,
    pub rx: u8,
    /// Command byte -> count.
    pub commands: HashMap<u8, i64>,
    pub dpd: bool,
    /// When WIP clears: the machine's cycle count, 0 when idle.
    pub busy_until: i64,
    /// Go's `machine != nil`: a Machine owns this flash (then `now`/`hz`
    /// are its Cycles and CPUHz, which the Machine keeps up to date before it
    /// calls in).
    pub(crate) attached: bool,
    pub(crate) now: i64,
    pub(crate) hz: i64,
    pub log: Vec<FlashOp>,
    /// Go OnWrite, called at once (it does not see the machine; a machine
    /// owner who needs it uses Machine::on_flash_write).
    pub on_write: Option<OnWriteFn>,
    /// FlashTppUs, FlashTseUs, FlashTbe32Us, FlashTbe64Us (package variables in Go).
    pub tpp_us: i64,
    pub tse_us: i64,
    pub tbe32_us: i64,
    pub tbe64_us: i64,
    /// The writes committed since the machine last looked (for Machine::on_flash_write).
    pub(crate) wrote_events: Vec<(&'static str, usize, usize)>,
}

impl Flash {
    /// NewFlash: an erased flash, a part of size bytes with the default IDs (a Puya P25Q80).
    pub fn new(size: usize) -> Flash {
        Flash::new_jedec(size, [0x85, 0x60, 0x14])
    }

    /// NewFlashJedec.
    pub fn new_jedec(size: usize, jedec: [u8; 3]) -> Flash {
        let cfg = Config::env();
        let mut f = Flash {
            mem: vec![0xFF; size.max(FLASH_MEM)],
            size,
            beyond: -1,
            beyond_mode: cfg.flash_beyond.clone(),
            jedec,
            mid: 0,
            sfdp: Vec::new(),
            part: 0,
            status: 0,
            uid: (0..16).map(|i| 0xA0 + i as u8).collect(),
            wel: false,
            cs_low: false,
            tx: Vec::new(),
            rx: 0xFF,
            commands: HashMap::new(),
            dpd: false,
            busy_until: 0,
            attached: false,
            now: 0,
            hz: 0,
            log: Vec::new(),
            on_write: None,
            tpp_us: cfg.flash_tpp_us,
            tse_us: cfg.flash_tse_us,
            tbe32_us: cfg.flash_tbe32_us,
            tbe64_us: cfg.flash_tbe64_us,
            wrote_events: Vec::new(),
        };
        if size < FLASH_MEM && jedec[2] == 0x14 {
            // bits.Len(uint(size)) - 1
            let len = (usize::BITS - size.leading_zeros()) as i64;
            f.jedec[2] = (len - 1) as u8;
        }
        f.mid = (f.jedec[2] as i64) << 16 | (f.jedec[1] as i64) << 8 | f.jedec[0] as i64;
        f.part = f.mid;
        f
    }

    /// Select drives chip select: low = selected. Deselecting commits the command.
    pub fn select(&mut self, low: bool) -> Result<(), Error> {
        let mut err = Ok(());
        if self.cs_low && !low {
            err = self.commit();
        }
        if low && !self.cs_low {
            self.tx.clear();
        }
        self.cs_low = low;
        err
    }

    /// SetSFDP gives the part SFDP data.
    pub fn set_sfdp(&mut self, data: Vec<u8>) {
        self.sfdp = data;
        self.part = self.mid;
        if (self.mid == 0x13325E || self.mid == 0x14325E) && !self.sfdp.is_empty() && self.sfdp[0] == 0x53 {
            self.part = 0x01000000 | self.mid;
        }
    }

    /// SPIBusy: a transaction that an XIP access would cut.
    pub fn spi_busy(&self) -> bool {
        if self.tx.len() > 1 {
            return true;
        }
        if self.tx.len() == 1 {
            return matches!(
                self.tx[0],
                0x01 | 0x02 | 0x03 | 0x05 | 0x0B | 0x11 | 0x20 | 0x31 | 0x35 | 0x4B | 0x52 | 0x5A | 0x9F | 0xD8
            );
        }
        false
    }

    /// Abort: a chip reset takes the chip select high without a commit.
    pub fn abort(&mut self) {
        self.cs_low = false;
        self.tx.clear();
        self.beyond = -1;
    }

    /// The bytes sent in the transaction under way (Go f.tx).
    pub fn tx(&self) -> &[u8] {
        &self.tx
    }

    pub fn cs_low(&self) -> bool {
        self.cs_low
    }

    /// Busy: a program or erase with a busy time still running (at the cycle
    /// count the owning machine last gave).
    pub fn busy(&mut self) -> bool {
        if self.busy_until == 0 {
            return false;
        }
        if self.now < self.busy_until {
            return true;
        }
        self.busy_until = 0;
        false
    }

    /// BusyLeftUs: the busy time left, in microseconds.
    pub fn busy_left_us(&self) -> f64 {
        (self.busy_until.wrapping_sub(self.now).wrapping_mul(1_000_000)) as f64 / self.hz as f64
    }

    fn start_busy(&mut self, us: i64) {
        if us != 0 && self.attached {
            self.busy_until = self.now + (us * self.hz + 999_999) / 1_000_000;
        }
    }

    fn refuse_busy(&mut self, cmd: u8) -> Result<(), Error> {
        if cmd != 0x05 && cmd != 0x35 && self.busy() {
            self.tx.clear();
            return Err(emu_err(format!(
                "flash command 0x{:02x} while the flash is busy with a program or erase (WIP, {:.0} us left): the part takes only status reads until WIP clears",
                cmd,
                self.busy_left_us()
            )));
        }
        Ok(())
    }

    /// Clock sends one byte and returns the byte clocked in.
    pub fn clock(&mut self, b: u8) -> Result<u8, Error> {
        if !self.cs_low {
            return Ok(0xFF);
        }
        self.tx.push(b);
        let cmd = self.tx[0];
        let n = self.tx.len();
        self.beyond = -1;
        if n > 1 && self.busy_until != 0 {
            self.refuse_busy(cmd)?;
        }
        if self.dpd {
            return Ok(0xFF);
        }
        Ok(self.clock_data(cmd, n))
    }

    fn clock_data(&mut self, cmd: u8, n: usize) -> u8 {
        if cmd == 0x03 || cmd == 0x0B {
            let skip = if cmd == 0x0B { 5 } else { 4 };
            if n > skip {
                let a = ((self.tx[1] as usize) << 16 | (self.tx[2] as usize) << 8 | self.tx[3] as usize) + (n - skip - 1);
                if self.size < FLASH_MEM && a >= self.size {
                    self.beyond = a as i64;
                    if self.beyond_mode == "wrap" {
                        return self.mem[a % self.size];
                    }
                    return 0xFF;
                }
                return self.mem[a % self.mem.len()];
            }
            return 0xFF;
        }
        if cmd == 0x05 {
            let mut v = (self.status & 0xFC) as u8;
            if self.wel {
                v |= 0x02;
            }
            if self.busy() {
                v |= 0x01;
            }
            return v;
        }
        if cmd == 0x35 {
            return (self.status >> 8) as u8;
        }
        if cmd == 0x9F && 1 < n && n <= 4 {
            return self.jedec[n - 2];
        }
        if cmd == 0x5A && n > 5 {
            let a = ((self.tx[1] as usize) << 16 | (self.tx[2] as usize) << 8 | self.tx[3] as usize) + (n - 6);
            if a < self.sfdp.len() {
                return self.sfdp[a];
            }
            return 0xFF;
        }
        if cmd == 0x4B && n > 5 {
            return self.uid[(n - 6) % self.uid.len()];
        }
        0xFF
    }

    fn commit(&mut self) -> Result<(), Error> {
        if self.tx.is_empty() {
            return Ok(());
        }
        let cmd = self.tx[0];
        let (mut addr, mut has_addr) = (0usize, false);
        if self.tx.len() >= 4 {
            addr = (self.tx[1] as usize) << 16 | (self.tx[2] as usize) << 8 | self.tx[3] as usize;
            has_addr = true;
        }
        *self.commands.entry(cmd).or_insert(0) += 1;
        if self.busy_until != 0 {
            self.refuse_busy(cmd)?;
        }
        if self.dpd {
            self.dpd = cmd != 0xAB;
            self.tx.clear();
            return Ok(());
        }
        if self.size < FLASH_MEM && has_addr && (cmd == 0x02 || cmd == 0x20 || cmd == 0x52 || cmd == 0xD8) {
            let n = if cmd == 0x02 { self.tx.len() - 4 } else { 1 };
            if addr + n > self.size {
                self.tx.clear();
                let op = if cmd == 0x02 { "program" } else { "erase" };
                return Err(emu_err(format!(
                    "flash {} (0x{:02x}) at 0x{:x}: the part has 0x{:x} bytes",
                    op, cmd, addr, self.size
                )));
            }
        }
        if cmd == 0xB9 {
            self.dpd = true;
        } else if (cmd == 0x01 || cmd == 0x31 || cmd == 0x11) && self.wel {
            let data: Vec<u8> = self.tx[1..].to_vec();
            let hexs: String = data.iter().map(|b| format!("{:02x}", b)).collect();
            self.log.push(FlashOp::new("status", cmd as i64, data.len() as i64, hexs));
            self.wel = false;
            if cmd == 0x01 && !data.is_empty() {
                self.status = (self.status & 0xFF00) | (data[0] & 0xFC) as i64;
                if data.len() >= 2 {
                    self.status = (self.status & 0x00FF) | (data[1] as i64) << 8;
                }
            } else if cmd == 0x31 && !data.is_empty() {
                self.status = (self.status & 0x00FF) | (data[0] as i64) << 8;
            }
            let (_, _, listed) = self.protected_range();
            if !listed {
                self.tx.clear();
                return Err(emu_err(format!(
                    "flash status 0x{:04x} (0x{:02x}): the SDK's table for part 0x{:06x} does not list its protection",
                    self.status, cmd, self.part
                )));
            }
        } else if (cmd == 0x60 || cmd == 0xC7) && self.wel {
            for b in self.mem.iter_mut() {
                *b = 0xFF;
            }
            let n = self.mem.len() as i64;
            self.log.push(FlashOp::new("chip erase", 0, n, String::new()));
            self.wel = false;
        } else if cmd == 0x06 {
            self.wel = true;
        } else if cmd == 0x04 {
            self.wel = false;
        } else if cmd == 0x02 && self.wel && has_addr {
            let data: Vec<u8> = self.tx[4..].to_vec();
            let page = addr & !0xFF;
            self.wel = false;
            if self.protected(addr as i64, data.len() as i64) {
                self.wrote("protected", addr, data.len());
            } else {
                for (i, &b) in data.iter().enumerate() {
                    let a = page | ((addr + i) & 0xFF);
                    self.mem[a] &= b;
                }
                self.wrote("program", addr, data.len());
                let us = self.tpp_us;
                self.start_busy(us);
            }
        } else if (cmd == 0x20 || cmd == 0x52 || cmd == 0xD8) && self.wel && has_addr {
            let size: usize = match cmd {
                0x20 => 0x1000,
                0x52 => 0x8000,
                _ => 0x10000,
            };
            let base = addr & !(size - 1);
            self.wel = false;
            if self.protected(base as i64, size as i64) {
                self.wrote("protected", base, size);
            } else {
                let end = (base + size).min(self.mem.len());
                for i in base..end {
                    self.mem[i] = 0xFF;
                }
                self.wrote("erase", base, size);
                let us = match cmd {
                    0x20 => self.tse_us,
                    0x52 => self.tbe32_us,
                    _ => self.tbe64_us,
                };
                self.start_busy(us);
            }
        }
        self.tx.clear();
        Ok(())
    }

    /// ProtectedRange: (range, protected, listed).
    pub fn protected_range(&self) -> (ProtectedAddrs, bool, bool) {
        let t = match flash_protection().get(&self.part) {
            Some(t) => t,
            None => return (ProtectedAddrs::default(), false, true),
        };
        match t.ranges.get(&(self.status & t.mask)) {
            None => (ProtectedAddrs::default(), false, false),
            Some(None) => (ProtectedAddrs::default(), false, true),
            Some(Some(p)) => (*p, true, true),
        }
    }

    /// Protected: any of the n bytes at addr lies in the protected range.
    pub fn protected(&self, addr: i64, n: i64) -> bool {
        let (r, protected, _) = self.protected_range();
        protected && addr <= r.last && addr + n - 1 >= r.first
    }

    fn wrote(&mut self, op: &'static str, addr: usize, n: usize) {
        self.log.push(FlashOp::new(op, addr as i64, n as i64, String::new()));
        if let Some(f) = self.on_write.clone() {
            f(op, addr, n);
        }
        if self.attached {
            self.wrote_events.push((op, addr, n));
        }
    }
}

struct ProtTable {
    mask: i64,
    ranges: BTreeMap<i64, Option<ProtectedAddrs>>,
}

fn pa(first: i64, last: i64) -> Option<ProtectedAddrs> {
    Some(ProtectedAddrs { first, last })
}

fn low_table(size: i64) -> ProtTable {
    let top = size - 1;
    let mut r = BTreeMap::new();
    r.insert(0x00, None);
    r.insert(0x04, pa(0, top - 0x2000));
    r.insert(0x08, pa(0, top - 0x4000));
    r.insert(0x0C, pa(0, top - 0x8000));
    r.insert(0x10, pa(0, top - 0x10000));
    r.insert(0x14, pa(0, top - 0x20000));
    r.insert(0x18, pa(0, top - 0x40000));
    r.insert(0x1C, pa(0, top));
    ProtTable { mask: 0x1C, ranges: r }
}

fn puya_table(size: i64) -> ProtTable {
    let (top, k) = (size - 1, 1024i64);
    let mut r = BTreeMap::new();
    for (v, p) in [
        (0x0000, None),
        (0x0020, None),
        (0x407C, None),
        (0x0004, pa(size - 64 * k, top)),
        (0x0008, pa(size - 128 * k, top)),
        (0x000C, pa(size - 256 * k, top)),
        (0x0024, pa(0, 64 * k - 1)),
        (0x0028, pa(0, 128 * k - 1)),
        (0x002C, pa(0, 256 * k - 1)),
        (0x0044, pa(size - 4 * k, top)),
        (0x0048, pa(size - 8 * k, top)),
        (0x004C, pa(size - 16 * k, top)),
        (0x0050, pa(size - 32 * k, top)),
        (0x0054, pa(size - 32 * k, top)),
        (0x0064, pa(0, 4 * k - 1)),
        (0x0068, pa(0, 8 * k - 1)),
        (0x006C, pa(0, 16 * k - 1)),
        (0x0070, pa(0, 32 * k - 1)),
        (0x0074, pa(0, 32 * k - 1)),
        (0x4044, pa(0, top - 4 * k)),
        (0x4048, pa(0, top - 8 * k)),
        (0x404C, pa(0, top - 16 * k)),
        (0x4050, pa(0, top - 32 * k)),
        (0x4054, pa(0, top - 32 * k)),
        (0x4064, pa(4 * k, top)),
        (0x4068, pa(8 * k, top)),
        (0x406C, pa(16 * k, top)),
        (0x4070, pa(32 * k, top)),
        (0x4074, pa(32 * k, top)),
        (0x007C, pa(0, top)),
        (0x4000, pa(0, top)),
        (0x4040, pa(0, top)),
        (0x4020, pa(0, top)),
        (0x4060, pa(0, top)),
    ] {
        r.insert(v, p);
    }
    let more: Vec<(i64, Option<ProtectedAddrs>)> = if size == 0x100000 {
        vec![
            (0x0010, pa(0x80000, top)),
            (0x4030, pa(0x80000, top)),
            (0x0030, pa(0, 0x7FFFF)),
            (0x4010, pa(0, 0x7FFFF)),
            (0x4004, pa(0, top - 64 * k)),
            (0x4008, pa(0, top - 128 * k)),
            (0x400C, pa(0, top - 256 * k)),
            (0x4024, pa(64 * k, top)),
            (0x4028, pa(128 * k, top)),
            (0x402C, pa(256 * k, top)),
        ]
    } else {
        vec![
            (0x4030, None),
            (0x4004, pa(0, top - 64 * k)),
            (0x4008, pa(0, top - 128 * k)),
            (0x4024, pa(64 * k, top)),
            (0x4028, pa(128 * k, top)),
        ]
    };
    for (v, p) in more {
        r.insert(v, p);
    }
    ProtTable { mask: 0x407C, ranges: r }
}

fn zbit_c_table(size: i64) -> ProtTable {
    let (top, k) = (size - 1, 1024i64);
    let mut r = BTreeMap::new();
    for (v, p) in [
        (0x0000, None),
        (0x0004, pa(size - 64 * k, top)),
        (0x0008, pa(size - 128 * k, top)),
        (0x000C, pa(size - 256 * k, top)),
        (0x0024, pa(0, 64 * k - 1)),
        (0x0028, pa(0, 128 * k - 1)),
        (0x002C, pa(0, 256 * k - 1)),
        (0x0044, pa(size - 4 * k, top)),
        (0x0048, pa(size - 8 * k, top)),
        (0x004C, pa(size - 16 * k, top)),
        (0x0050, pa(size - 32 * k, top)),
        (0x0064, pa(0, 4 * k - 1)),
        (0x0068, pa(0, 8 * k - 1)),
        (0x006C, pa(0, 16 * k - 1)),
        (0x0070, pa(0, 32 * k - 1)),
        (0x4004, pa(0, top - 64 * k)),
        (0x4008, pa(0, top - 128 * k)),
        (0x4024, pa(64 * k, top)),
        (0x4028, pa(128 * k, top)),
        (0x4044, pa(0, top - 4 * k)),
        (0x4048, pa(0, top - 8 * k)),
        (0x404C, pa(0, top - 16 * k)),
        (0x4050, pa(0, top - 32 * k)),
        (0x4064, pa(4 * k, top)),
        (0x4068, pa(8 * k, top)),
        (0x406C, pa(16 * k, top)),
    ] {
        r.insert(v, p);
    }
    let more: Vec<(i64, Option<ProtectedAddrs>)> = if size == 0x100000 {
        vec![
            (0x0010, pa(0x80000, top)),
            (0x001C, pa(0, top)),
            (0x003C, pa(0, top)),
            (0x005C, pa(0, top)),
            (0x007C, pa(0, top)),
            (0x0030, pa(0, 0x7FFFF)),
            (0x0054, pa(size - 32 * k, top)),
            (0x0074, pa(0, 32 * k - 1)),
            (0x400C, pa(0, top - 256 * k)),
            (0x402C, pa(256 * k, top)),
            (0x4054, pa(0, top - 32 * k)),
            (0x4070, pa(32 * k, top)),
            (0x4074, pa(32 * k, top)),
        ]
    } else {
        vec![
            (0x0010, pa(0, top)),
            (0x0030, pa(0, top)),
            (0x005C, pa(0, top)),
            (0x007C, pa(0, top)),
            (0x0058, pa(size - 32 * k, top)),
            (0x0078, pa(0, 32 * k - 1)),
            (0x4058, pa(0, top - 32 * k)),
            (0x4078, pa(32 * k, top)),
        ]
    };
    for (v, p) in more {
        r.insert(v, p);
    }
    ProtTable { mask: 0x407C, ranges: r }
}

fn flash_protection() -> &'static BTreeMap<i64, ProtTable> {
    static T: OnceLock<BTreeMap<i64, ProtTable>> = OnceLock::new();
    T.get_or_init(|| {
        let mut m = BTreeMap::new();
        m.insert(0x1360C8, low_table(0x80000));
        m.insert(0x13325E, low_table(0x80000));
        m.insert(0x1460C8, low_table(0x100000));
        m.insert(0x14325E, low_table(0x100000));
        m.insert(0x136085, puya_table(0x80000));
        m.insert(0x146085, puya_table(0x100000));
        m.insert(0x0113325E, zbit_c_table(0x80000));
        m.insert(0x0114325E, zbit_c_table(0x100000));
        m
    })
}
