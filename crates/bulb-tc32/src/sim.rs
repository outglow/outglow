use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

use crate::reg;

pub const FLASH_SIZE: usize = 0x10_0000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    R8(usize, u8),
    R16(usize, u16),
    R32(usize, u32),
    W8(usize, u8),
    W16(usize, u16),
    W32(usize, u32),
    AnaW(u8, u8),
    AnaR(u8, u8),
}

impl Op {
    pub fn is_write(&self) -> bool {
        matches!(self, Op::W8(..) | Op::W16(..) | Op::W32(..))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum Spi {
    #[default]
    Idle,
    Command,
    Program { addr: usize, n: u8 },
    ProgramData { addr: usize },
    Erase { addr: usize, n: u8 },
    Status,
    Other,
}

pub struct Chip {
    regs: HashMap<usize, u8>,
    analog: [u8; 256],
    pub log: Vec<Op>,
    pub tick: u32,
    pub tick_step: u32,
    pub flash: Vec<u8>,
    pub flash_ops: Vec<(u8, usize)>,
    spi: Spi,
    write_enabled: bool,
    pub aes_ready: bool,
    aes_key: [u8; 16],
    aes_in: Vec<u8>,
    aes_out: Vec<u8>,
    pub tx_completes: bool,
    pub tx_kicks: usize,
    reset_requested: bool,
    erased: bool,
    pub hook: Option<Hook>,
}

pub type Hook = Box<dyn FnMut(&mut Chip, Op)>;

impl Chip {
    fn new() -> Self {
        let mut analog = [0u8; 256];
        analog[0xcf] = 0x80;
        Chip {
            regs: HashMap::new(),
            analog,
            log: Vec::new(),
            tick: 0,
            tick_step: 16,
            flash: vec![0xff; FLASH_SIZE],
            flash_ops: Vec::new(),
            spi: Spi::Idle,
            write_enabled: false,
            aes_ready: true,
            aes_key: [0; 16],
            aes_in: Vec::new(),
            aes_out: Vec::new(),
            tx_completes: true,
            tx_kicks: 0,
            reset_requested: false,
            erased: false,
            hook: None,
        }
    }

    pub fn peek8(&self, off: usize) -> u8 {
        self.regs.get(&off).copied().unwrap_or(0)
    }

    pub fn poke8(&mut self, off: usize, v: u8) {
        self.regs.insert(off, v);
    }

    pub fn poke32(&mut self, off: usize, v: u32) {
        for (i, b) in v.to_le_bytes().iter().enumerate() {
            self.regs.insert(off + i, *b);
        }
    }

    pub fn raise16(&mut self, off: usize, bits: u16) {
        let [lo, hi] = bits.to_le_bytes();
        let (a, b) = (self.peek8(off), self.peek8(off + 1));
        self.regs.insert(off, a | lo);
        self.regs.insert(off + 1, b | hi);
    }

    pub fn ticks_read(&self) -> Vec<u32> {
        self.log.iter().filter_map(|o| match *o {
            Op::R32(reg::SYSTEM_TICK, t) => Some(t),
            _ => None,
        }).collect()
    }

    pub fn analog(&self, addr: u8) -> u8 {
        self.analog[addr as usize]
    }

    pub fn set_analog(&mut self, addr: u8, v: u8) {
        self.analog[addr as usize] = v;
    }

    pub fn on(&mut self, hook: impl FnMut(&mut Chip, Op) + 'static) {
        self.hook = Some(Box::new(hook));
    }

    fn run_hook(&mut self, op: Op) {
        if let Some(mut h) = self.hook.take() {
            h(self, op);
            if self.hook.is_none() {
                self.hook = Some(h);
            }
        }
    }

    pub fn writes(&self) -> Vec<Op> {
        self.log.iter().copied().filter(Op::is_write).collect()
    }

    pub fn analog_writes(&self) -> Vec<(u8, u8)> {
        self.log
            .iter()
            .filter_map(|o| match *o {
                Op::AnaW(a, v) => Some((a, v)),
                _ => None,
            })
            .collect()
    }

    fn read(&mut self, off: usize, width: usize) -> u32 {
        if (reg::SYSTEM_TICK..reg::SYSTEM_TICK + 4).contains(&off) {
            self.tick = self.tick.wrapping_add(self.tick_step);
            let v = self.tick.to_le_bytes();
            for (i, b) in v.iter().enumerate() {
                self.regs.insert(reg::SYSTEM_TICK + i, *b);
            }
        }
        if off == AES_CTRL {
            let mut c = self.peek8(AES_CTRL) & !0x06;
            if self.aes_in.len() < 16 {
                c |= 0x02;
            }
            if self.aes_ready && self.aes_out.len() == 16 {
                c |= 0x04;
            }
            self.regs.insert(AES_CTRL, c);
        }
        if off == AES_DATA && width == 4 && self.aes_out.len() >= 4 {
            let w: Vec<u8> = self.aes_out.drain(..4).collect();
            for (i, b) in w.iter().enumerate() {
                self.regs.insert(AES_DATA + i, *b);
            }
        }
        let mut v = 0u32;
        for i in 0..width {
            v |= (self.peek8(off + i) as u32) << (8 * i);
        }
        v
    }

    fn write(&mut self, off: usize, width: usize, v: u32) {
        let bytes = v.to_le_bytes();
        if W1C.contains(&off) {
            for i in 0..width {
                let cur = self.peek8(off + i);
                self.regs.insert(off + i, cur & !bytes[i]);
            }
            return;
        }
        for i in 0..width {
            self.regs.insert(off + i, bytes[i]);
        }
        match off {
            reg::ANA_CTRL => self.analog_strobe(bytes[0]),
            reg::MSPI_CTRL => self.spi_cs(bytes[0] & reg::MSPI_CS_HIGH != 0),
            reg::MSPI_DATA => self.spi_byte(bytes[0]),
            reg::PWDN_CTRL if bytes[0] & 0x20 != 0 => self.reset_requested = true,
            AES_DATA if width == 4 => self.aes_feed(bytes),
            TX_FIFO_RPTR => {
                let w = self.peek8(TX_FIFO_WPTR) & 0x0f;
                let r = if bytes[0] & 0x10 != 0 { w } else { bytes[0] & 0x0f };
                self.regs.insert(TX_FIFO_RPTR, r);
            }
            TX_FIFO_PUSH => {
                let w = self.peek8(TX_FIFO_WPTR);
                self.regs.insert(TX_FIFO_WPTR, (w + 1) & 0x0f);
            }
            DMA_TX_READY if bytes[0] & 0x08 != 0 => {
                self.tx_kicks += 1;
                if self.tx_completes {
                    let s = self.peek8(RF_IRQ_STATUS);
                    self.regs.insert(RF_IRQ_STATUS, s | 0x02);
                }
            }
            _ => {}
        }
        if (AES_KEY..AES_KEY + 16).contains(&off) {
            self.aes_key[off - AES_KEY] = bytes[0];
        }
        if off == AES_CTRL && bytes[0] & 0x01 == 0 {
            self.aes_in.clear();
            self.aes_out.clear();
        }
    }

    fn analog_strobe(&mut self, ctrl: u8) {
        let addr = self.peek8(reg::ANA_ADDR);
        if ctrl == 0x60 {
            let v = self.peek8(reg::ANA_DATA);
            self.analog[addr as usize] = v;
            self.log.push(Op::AnaW(addr, v));
        } else if ctrl == 0x40 {
            let v = self.analog[addr as usize];
            self.regs.insert(reg::ANA_DATA, v);
            self.log.push(Op::AnaR(addr, v));
        }
    }

    fn spi_cs(&mut self, high: bool) {
        if matches!(self.spi, Spi::ProgramData { .. } | Spi::Erase { .. }) || self.erased {
            self.write_enabled = false;
            self.erased = false;
        }
        self.spi = if high { Spi::Idle } else { Spi::Command };
    }

    fn spi_byte(&mut self, b: u8) {
        self.spi = match self.spi {
            Spi::Idle => Spi::Idle,
            Spi::Command => match b {
                0x06 => {
                    self.write_enabled = true;
                    Spi::Other
                }
                0x02 => Spi::Program { addr: 0, n: 0 },
                0x20 => Spi::Erase { addr: 0, n: 0 },
                0x05 => Spi::Status,
                _ => Spi::Other,
            },
            Spi::Program { addr, n } => {
                let addr = addr << 8 | b as usize;
                if n == 2 {
                    self.flash_ops.push((0x02, addr));
                    Spi::ProgramData { addr }
                } else {
                    Spi::Program { addr, n: n + 1 }
                }
            }
            Spi::ProgramData { addr } => {
                if self.write_enabled {
                    if let Some(cell) = self.flash.get_mut(addr) {
                        *cell &= b;
                    }
                }
                Spi::ProgramData { addr: (addr & !0xff) | ((addr + 1) & 0xff) }
            }
            Spi::Erase { addr, n } => {
                let addr = addr << 8 | b as usize;
                if n == 2 {
                    self.flash_ops.push((0x20, addr));
                    if self.write_enabled {
                        let base = addr & !0xfff;
                        if base + 0x1000 <= self.flash.len() {
                            self.flash[base..base + 0x1000].fill(0xff);
                        }
                    }
                    self.erased = true;
                    Spi::Other
                } else {
                    Spi::Erase { addr, n: n + 1 }
                }
            }
            Spi::Status => {
                self.regs.insert(reg::MSPI_DATA, 0);
                Spi::Status
            }
            Spi::Other => Spi::Other,
        };
    }

    fn aes_feed(&mut self, bytes: [u8; 4]) {
        if self.aes_in.len() >= 16 {
            return;
        }
        self.aes_in.extend_from_slice(&bytes);
        if self.aes_in.len() == 16 {
            let mut block: [u8; 16] = self.aes_in[..].try_into().unwrap_or([0; 16]);
            zigbee::aes::Aes128::new(&self.aes_key).encrypt(&mut block);
            self.aes_out = block.to_vec();
        }
    }
}

const AES_CTRL: usize = 0x540;
const AES_DATA: usize = 0x548;
const AES_KEY: usize = 0x550;
const RF_IRQ_STATUS: usize = 0xf20;
const TX_FIFO_RPTR: usize = 0xc2a;
const TX_FIFO_WPTR: usize = 0xc2b;
const TX_FIFO_PUSH: usize = 0xc2c;
const DMA_TX_READY: usize = 0xc24;
const W1C: [usize; 2] = [0xf20, 0xf21];

thread_local! {
    static CHIP: RefCell<Option<Chip>> = const { RefCell::new(None) };
}

static LOCK: Mutex<()> = Mutex::new(());

pub struct Session {
    _guard: MutexGuard<'static, ()>,
}

pub const SESSION_MAX: std::time::Duration = std::time::Duration::from_secs(10);

static STARTED: Mutex<Option<std::time::Instant>> = Mutex::new(None);
static WATCHDOG: std::sync::Once = std::sync::Once::new();

fn watch() {
    loop {
        std::thread::sleep(std::time::Duration::from_millis(100));
        let started = *STARTED.lock().unwrap_or_else(|e| e.into_inner());
        if started.is_some_and(|t| t.elapsed() > SESSION_MAX) {
            eprintln!("a test ran {SESSION_MAX:?} on the simulated chip: a loop that never ends without touching a register");
            std::process::abort();
        }
    }
}

pub fn session() -> Session {
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    WATCHDOG.call_once(|| {
        std::thread::spawn(watch);
    });
    *STARTED.lock().unwrap_or_else(|e| e.into_inner()) = Some(std::time::Instant::now());
    CHIP.with(|c| *c.borrow_mut() = Some(Chip::new()));
    Session { _guard: guard }
}

impl Drop for Session {
    fn drop(&mut self) {
        *STARTED.lock().unwrap_or_else(|e| e.into_inner()) = None;
        CHIP.with(|c| *c.borrow_mut() = None);
    }
}

pub fn with<R>(f: impl FnOnce(&mut Chip) -> R) -> R {
    let (r, reset) = CHIP.with(|c| {
        let mut c = c.borrow_mut();
        let chip = c.get_or_insert_with(Chip::new);
        let r = f(chip);
        let reset = std::mem::take(&mut chip.reset_requested);
        (r, reset)
    });
    if reset {
        panic!("{}", REBOOT);
    }
    r
}

pub const REBOOT: &str = "the chip was reset (0x6f = 0x20)";

pub const LOG_MAX: usize = 10_000_000;

fn bounded() {
    if with(|c| c.log.len() >= LOG_MAX) {
        panic!("more than {LOG_MAX} register accesses in one test: a driver loop that never ends");
    }
}

pub fn read(off: usize, width: usize) -> u32 {
    bounded();
    with(|c| {
        let v = c.read(off, width);
        let op = match width {
            1 => Op::R8(off, v as u8),
            2 => Op::R16(off, v as u16),
            _ => Op::R32(off, v),
        };
        c.log.push(op);
        c.run_hook(op);
        v
    })
}

pub fn write(off: usize, width: usize, v: u32) {
    bounded();
    with(|c| {
        let op = match width {
            1 => Op::W8(off, v as u8),
            2 => Op::W16(off, v as u16),
            _ => Op::W32(off, v),
        };
        c.log.push(op);
        c.write(off, width, v);
        c.run_hook(op);
    })
}

pub fn flash8(addr: usize) -> u8 {
    with(|c| c.flash.get(addr).copied().unwrap_or(0xff))
}

pub fn reboots<F: FnOnce()>(f: F) -> bool {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    std::panic::set_hook(hook);
    match r {
        Ok(()) => false,
        Err(e) => {
            let msg = e.downcast_ref::<String>().cloned().unwrap_or_default();
            if msg == REBOOT {
                true
            } else {
                std::panic::resume_unwind(e)
            }
        }
    }
}
