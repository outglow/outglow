use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use crate::config::Config;
use crate::error::{emu_err, Error};
use crate::flash::Flash;
use crate::i2c::I2cDevice;
use crate::pke::PkeWrite;
use crate::spi::SpiDevice;

pub const FLASH_MEM: usize = 0x100000;
pub(crate) const ICACHE_LINES: usize = 64;
pub(crate) const ICACHE_LINE_SHIFT: usize = 5;

pub const SRAM_BASE: u32 = 0x840000;
pub const REG_BASE: u32 = 0x800000;
pub const REG_SIZE: usize = 0x10000;
pub const REG_ALIAS: u32 = 0x1000000;
pub const MODE_IRQ: u32 = 0x12;
pub const MODE_SVC: u32 = 0x13;
pub const CPU_HZ: i64 = 24_000_000;
pub const STIMER_HZ: i64 = 16_000_000;
pub const IRQ_STIMER: u32 = 20;
pub(crate) const STIMER_CTRL_RESET: u8 = 0xC1;
pub(crate) const STIMER_EN: u8 = 0x02;
pub(crate) const STIMER_IRQ: u8 = 0x04;
pub(crate) const M32: i64 = 0xFFFFFFFF;

pub fn sram_fill(seed: u32, size: usize) -> Vec<u8> {
    let mut out = vec![0u8; size];
    let mut x = seed;
    for b in out.iter_mut() {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        *b = x as u8;
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnalogAccess {
    pub kind: u8,
    pub addr: u8,
    pub val: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnalogWrite {
    pub addr: u8,
    pub val: u8,
    pub who: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wake {
    pub ms: f64,
    pub status: u8,
}

#[derive(Clone)]
pub struct RegHookMask {
    read: Vec<u64>,
    write: Vec<u64>,
}

impl Default for RegHookMask {
    fn default() -> Self {
        RegHookMask { read: vec![0; REG_SIZE / 64], write: vec![0; REG_SIZE / 64] }
    }
}

impl RegHookMask {
    pub fn mark_read(&mut self, o: usize) {
        self.read[o / 64] |= 1 << (o % 64);
    }
    pub fn mark_write(&mut self, o: usize) {
        self.write[o / 64] |= 1 << (o % 64);
    }
    pub fn mark_all_writes(&mut self) {
        self.write.iter_mut().for_each(|w| *w = u64::MAX);
    }
    #[inline]
    fn touches(bits: &[u64], o: usize, size: usize) -> bool {
        if o + size > REG_SIZE {
            return true;
        }
        (o..o + size).any(|b| bits[b / 64] & (1 << (b % 64)) != 0)
    }
    #[inline]
    pub fn read_hooked(&self, o: usize, size: usize) -> bool {
        Self::touches(&self.read, o, size)
    }
    #[inline]
    pub fn write_hooked(&self, o: usize, size: usize) -> bool {
        Self::touches(&self.write, o, size)
    }
}

pub type RegReadHook = Rc<dyn Fn(&mut Machine, usize, usize) -> Result<Option<u32>, Error>>;
pub type RegWriteHook = Rc<dyn Fn(&mut Machine, usize, usize, u32) -> Result<bool, Error>>;
pub type MemReadHook = Rc<dyn Fn(&mut Machine, u32, usize) -> Result<Option<u32>, Error>>;
pub type MemWriteHook = Rc<dyn Fn(&mut Machine, u32, usize, u32) -> Result<bool, Error>>;
pub type PadLevelsHook = Rc<dyn Fn(&mut Machine, usize, u8, u8) -> (u8, u8)>;
pub type MachineHook = Rc<dyn Fn(&mut Machine)>;
pub type TraceStepHook = Rc<dyn Fn(&mut Machine, u32, u16)>;
pub type OnWakeHook = Rc<dyn Fn(&mut Machine, u8)>;
pub type PcHook = Rc<dyn Fn(&mut Machine) -> Result<(), Error>>;
pub type EventHook = Rc<dyn Fn(&mut Machine, &str)>;
pub type ChkHook = Rc<dyn Fn(&mut Machine, u32, usize) -> Option<u32>>;
pub type UpdateTimeHook = Rc<dyn Fn(&mut Machine) -> Result<(), Error>>;
pub type IdleSkipHook = Rc<dyn Fn(&mut Machine, i64) -> i64>;
pub type SysclkHook = Rc<dyn Fn(&mut Machine) -> i64>;
pub type SramCodeEndHook = Rc<dyn Fn(&mut Machine) -> usize>;
pub type LevelSource = Rc<dyn Fn(&mut Machine) -> u32>;
pub type CoreWakeSource = Rc<dyn Fn(&mut Machine) -> bool>;
pub type FlashWriteHook = Rc<dyn Fn(&mut Machine, &str, usize, usize)>;

pub(crate) fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

pub(crate) fn le16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

pub(crate) fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

#[inline]
fn le_int(b: &[u8]) -> u32 {
    match b.len() {
        4 => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        2 => b[0] as u32 | (b[1] as u32) << 8,
        1 => b[0] as u32,
        _ => {
            let mut v = 0u32;
            for i in (0..b.len()).rev() {
                v = (v << 8) | b[i] as u32;
            }
            v
        }
    }
}

fn clip(b: &[u8], o: usize, size: usize) -> &[u8] {
    if o >= b.len() {
        return &[];
    }
    if o + size > b.len() {
        return &b[o..];
    }
    &b[o..o + size]
}

#[inline]
pub(crate) fn size_mask(size: usize) -> u32 {
    if size >= 4 {
        u32::MAX
    } else {
        (1u32 << (8 * size)) - 1
    }
}

fn bank_index(mode: u32) -> Option<usize> {
    match mode {
        MODE_IRQ => Some(0),
        MODE_SVC => Some(1),
        _ => None,
    }
}

pub(crate) const PAD_R_OHM: [i64; 4] = [0, 1_000_000, 100_000, 10_000];

pub struct Machine {
    pub cfg: Config,
    pub flash: Flash,
    pub cpi: i64,
    pub cycle_accurate: bool,

    pub r: [u32; 16],
    pub(crate) bank: [[u32; 2]; 2],
    pub spsr: [u32; 2],
    pub mode: u32,
    pub ibit: u32,
    pub n: u32,
    pub z: u32,
    pub c: u32,
    pub v: u32,
    pub sram: Vec<u8>,
    pub regs: Vec<u8>,
    pub analog: [u8; 256],
    pub pad_hold: [u8; 4],

    pub boot_slot: usize,
    pub boot_copy: usize,
    pub cycles: i64,
    pub cpu_hz: i64,
    pub(crate) ms_base: f64,
    pub(crate) ms_base_cyc: i64,
    pub(crate) tick_base: u32,
    pub(crate) tick_base_cy: i64,
    pub(crate) stimer_on: bool,
    pub stimer_cmp: u32,
    pub(crate) irq_latch: u32,
    pub(crate) gpio_req: bool,
    pub(crate) last_tick: u32,
    pub(crate) tick2_base: i64,
    pub(crate) tmr_base: [i64; 2],
    pub(crate) tmr_on: [bool; 2],
    pub(crate) tmr_wrap: [bool; 2],
    pub(crate) tmr2_on: bool,
    pub(crate) tmr2_last: u32,
    pub(crate) wd_fed: i64,
    pub(crate) mspi_auto: bool,
    pub(crate) rng: u32,
    pub(crate) rbg_mode: usize,
    pub(crate) rbg_param: u32,
    pub(crate) rbg_state: u64,
    pub(crate) rbg_index: usize,
    pub(crate) rbg_cycle: Vec<u32>,
    pub(crate) k32_sigma_ns: i64,
    pub(crate) k32_rng: u64,
    pub(crate) k32_count: i64,
    pub(crate) k32_next: f64,
    pub(crate) k32_stopped: Option<u32>,
    pub(crate) pke_us: i64,
    pub(crate) pke_done_at: i64,
    pub(crate) pke_rt: u32,
    pub(crate) pke_pending: Vec<PkeWrite>,
    pub(crate) adc_mode: usize,
    pub(crate) adc_param: u32,
    pub(crate) adc_state: u64,
    pub(crate) adc_index: i64,
    pub(crate) adc_on: bool,
    pub(crate) adc_t0: i64,
    pub(crate) adc_period: i64,
    pub(crate) adc_done: i64,
    pub(crate) dfifo2_on: bool,
    pub(crate) dfifo2_t0: i64,
    pub(crate) dfifo0_on: bool,
    pub(crate) dfifo1_on: bool,
    pub(crate) dfifo0_t0: i64,
    pub(crate) dfifo1_t0: i64,
    pub(crate) pwm0_clr_frame: i64,
    pub(crate) pwm_fifo: Vec<u32>,
    pub(crate) pwm_ir_done: bool,
    pub(crate) qdec_init: bool,
    pub(crate) qdec_last: i64,
    pub(crate) qdec_count: i32,
    pub(crate) qdec_sub: i64,
    pub i2c_dev: Option<I2cDevice>,
    pub uart_tx: Vec<u8>,
    pub uart_rx: Vec<u8>,

    pub(crate) adc_pin_codes: BTreeMap<i64, u32>,

    pub spi_dev: Option<SpiDevice>,
    pub(crate) spi_busy_until: i64,
    pub(crate) spi_rx: u8,
    pub(crate) aes_in: Vec<u8>,
    pub(crate) aes_out: Vec<u8>,
    pub(crate) aes_ccm: bool,
    pub(crate) aes_prev: [u8; 16],
    pub div_ops: i64,
    pub log_analog: bool,
    pub analog_log: Vec<AnalogAccess>,
    pub analog_writes: Vec<AnalogWrite>,
    pub pad_c_pf: i64,
    pub pad_r_pct: i64,
    pub(crate) pad_rc_since: BTreeMap<(usize, usize), i64>,
    pub icache_miss: i64,
    pub icache_misses: i64,
    pub(crate) icache_tags: [i64; ICACHE_LINES],
    pub reset_count: i64,

    pub asleep: Option<u32>,
    pub stalled: bool,
    pub(crate) stall_since: i64,
    pub stall_cycles: i64,
    pub(crate) sleep_since: f64,
    pub(crate) sleep_deep: bool,
    pub lpc_fired: bool,
    pub(crate) k32_wake: Option<u32>,

    pub xtal_ready: bool,
    pub stimer_stuck: bool,
    pub adc_code: u32,
    pub gpio_oen_reset: u8,
    pub level_sources: Vec<LevelSource>,
    pub core_wake_sources: Vec<CoreWakeSource>,
    pub wakes: Vec<Wake>,
    pub float_wakes: BTreeMap<(i64, i64), bool>,
    pub on_treti: Option<MachineHook>,

    pub reg_read_hook: Option<RegReadHook>,
    pub reg_write_hook: Option<RegWriteHook>,
    pub reg_hook_mask: Option<Box<RegHookMask>>,
    pub pad_levels_hook: Option<PadLevelsHook>,
    pub reset_hooks: Vec<MachineHook>,
    pub trace_step: Option<TraceStepHook>,
    pub on_irq: Option<MachineHook>,
    pub on_wake: Option<OnWakeHook>,
    pub hooks: HashMap<u32, PcHook>,
    pub event: Option<EventHook>,
    pub mem_read_hook: Option<MemReadHook>,
    pub mem_read_hook_below: u32,
    pub mem_write_hook: Option<MemWriteHook>,
    pub mem_write_hook_below: u32,
    pub unaligned_read_hook: Option<MemReadHook>,
    pub chk_hook: Option<ChkHook>,
    pub update_time_hook: Option<UpdateTimeHook>,
    pub idle_skip_hook: Option<IdleSkipHook>,
    pub sysclk_hook: Option<SysclkHook>,
    pub on_feed: Option<MachineHook>,
    pub sram_code_end_hook: Option<SramCodeEndHook>,
    pub on_flash_write: Option<FlashWriteHook>,
    pub(crate) sym_addrs: Vec<u32>,
    pub(crate) sym_names: Vec<String>,

    pub idle_skipped: i64,
}

impl Machine {
    pub fn new(flash: Flash, boot_slot: i64, cpi: i64) -> Result<Machine, Error> {
        Machine::with_config(flash, boot_slot, cpi, Config::env())
    }

    pub fn with_config(mut flash: Flash, boot_slot: i64, cpi: i64, cfg: Config) -> Result<Machine, Error> {
        flash.attached = true;
        flash.now = 0;
        flash.hz = CPU_HZ;
        let sram = if cfg.sram_seed != 0 { sram_fill(cfg.sram_seed, cfg.sram_size) } else { vec![0u8; cfg.sram_size] };
        let mut m = Machine {
            flash,
            cpi,
            cycle_accurate: cfg.cycle_accurate,
            r: [0; 16],
            bank: [[0; 2]; 2],
            spsr: [0; 2],
            mode: 0,
            ibit: 0,
            n: 0,
            z: 0,
            c: 0,
            v: 0,
            sram,
            regs: vec![0u8; REG_SIZE],
            analog: [0; 256],
            pad_hold: [0; 4],
            boot_slot: 0,
            boot_copy: 0,
            cycles: 0,
            cpu_hz: 0,
            ms_base: 0.0,
            ms_base_cyc: 0,
            tick_base: 0,
            tick_base_cy: 0,
            stimer_on: false,
            stimer_cmp: 0,
            irq_latch: 0,
            gpio_req: false,
            last_tick: 0,
            tick2_base: 0,
            tmr_base: [0; 2],
            tmr_on: [false; 2],
            tmr_wrap: [false; 2],
            tmr2_on: false,
            tmr2_last: 0,
            wd_fed: 0,
            mspi_auto: false,
            rng: 0x12345678,
            rbg_mode: 0,
            rbg_param: 0,
            rbg_state: 0,
            rbg_index: 0,
            rbg_cycle: Vec::new(),
            k32_sigma_ns: 0,
            k32_rng: 0,
            k32_count: 0,
            k32_next: 0.0,
            k32_stopped: None,
            pke_us: 0,
            pke_done_at: 0,
            pke_rt: 0,
            pke_pending: Vec::new(),
            adc_mode: 0,
            adc_param: 0,
            adc_state: 0,
            adc_index: 0,
            adc_on: false,
            adc_t0: 0,
            adc_period: 0,
            adc_done: 0,
            dfifo2_on: false,
            dfifo2_t0: 0,
            dfifo0_on: false,
            dfifo1_on: false,
            dfifo0_t0: 0,
            dfifo1_t0: 0,
            pwm0_clr_frame: 0,
            pwm_fifo: Vec::new(),
            pwm_ir_done: false,
            qdec_init: false,
            qdec_last: 0,
            qdec_count: 0,
            qdec_sub: 0,
            i2c_dev: None,
            uart_tx: Vec::new(),
            uart_rx: Vec::new(),
            adc_pin_codes: BTreeMap::new(),
            spi_dev: None,
            spi_busy_until: 0,
            spi_rx: 0,
            aes_in: Vec::new(),
            aes_out: Vec::new(),
            aes_ccm: false,
            aes_prev: [0; 16],
            div_ops: 0,
            log_analog: false,
            analog_log: Vec::new(),
            analog_writes: Vec::new(),
            pad_c_pf: cfg.pad_c_pf,
            pad_r_pct: cfg.pad_r_pct,
            pad_rc_since: BTreeMap::new(),
            icache_miss: cfg.icache_miss,
            icache_misses: 0,
            icache_tags: [0; ICACHE_LINES],
            reset_count: 0,
            asleep: None,
            stalled: false,
            stall_since: 0,
            stall_cycles: 0,
            sleep_since: 0.0,
            sleep_deep: false,
            lpc_fired: false,
            k32_wake: None,
            xtal_ready: true,
            stimer_stuck: false,
            adc_code: 0x0D47,
            gpio_oen_reset: 0xFF,
            level_sources: Vec::new(),
            core_wake_sources: Vec::new(),
            wakes: Vec::new(),
            float_wakes: BTreeMap::new(),
            on_treti: None,
            reg_read_hook: None,
            reg_write_hook: None,
            reg_hook_mask: None,
            pad_levels_hook: None,
            reset_hooks: Vec::new(),
            trace_step: None,
            on_irq: None,
            on_wake: None,
            hooks: HashMap::new(),
            event: None,
            mem_read_hook: None,
            mem_read_hook_below: u32::MAX,
            mem_write_hook: None,
            mem_write_hook_below: u32::MAX,
            unaligned_read_hook: None,
            chk_hook: None,
            update_time_hook: None,
            idle_skip_hook: None,
            sysclk_hook: None,
            on_feed: None,
            sram_code_end_hook: None,
            on_flash_write: None,
            sym_addrs: Vec::new(),
            sym_names: Vec::new(),
            idle_skipped: 0,
            cfg,
        };
        m.level_sources = vec![Rc::new(|m: &mut Machine| (m.regs[0x623] & 0x07) as u32)];
        m.analog[0x3C] = 0x0F;
        let (rm, rp, am, ap) = (m.cfg.rbg_mode, m.cfg.rbg_param, m.cfg.adc_mode, m.cfg.adc_param);
        m.rbg_set(rm, rp);
        m.adc_set(am, ap);
        m.pke_us = m.cfg.pke_us;
        m.k32_sigma_ns = m.cfg.k32_jitter_ns;
        m.k32_rng = m.cfg.k32_jitter_seed;
        m.reset(boot_slot, false)?;
        Ok(m)
    }

    pub fn banked(&self, mode: u32) -> [u32; 2] {
        match bank_index(mode) {
            Some(i) => self.bank[i],
            None => [0, 0],
        }
    }

    pub fn set_banked(&mut self, mode: u32, v: [u32; 2]) {
        if let Some(i) = bank_index(mode) {
            self.bank[i] = v;
        }
    }

    pub fn latch_irq(&mut self, src: u32) {
        self.irq_latch |= 1u32.checked_shl(src).unwrap_or(0);
    }

    pub fn irq_latch(&self) -> u32 {
        self.irq_latch
    }

    pub fn set_irq_latch(&mut self, v: u32) {
        self.irq_latch = v;
    }

    pub fn banked_sp(&self) -> Vec<u32> {
        vec![self.bank[0][0], self.bank[1][0]]
    }

    pub fn set_symbols<'a, I: IntoIterator<Item = (&'a str, u32)>>(&mut self, syms: I) {
        let mut ps: Vec<(u32, String)> = syms.into_iter().map(|(n, a)| (a, n.to_string())).collect();
        ps.sort();
        self.sym_addrs = ps.iter().map(|p| p.0).collect();
        self.sym_names = ps.into_iter().map(|p| p.1).collect();
    }

    pub fn symbolize(&self, addr: u32) -> String {
        let i = self.sym_addrs.partition_point(|&a| a <= addr);
        if i == 0 {
            return format!("0x{:x}", addr);
        }
        let i = i - 1;
        format!("{}+0x{:x}", self.sym_names[i], addr - self.sym_addrs[i])
    }

    #[inline]
    pub(crate) fn here(&self) -> String {
        self.symbolize(self.r[15])
    }

    pub(crate) fn event(&mut self, text: &str) {
        if let Some(h) = self.event.clone() {
            h(self, text);
        }
    }

    #[inline]
    pub(crate) fn has_event(&self) -> bool {
        self.event.is_some()
    }

    #[inline]
    pub(crate) fn flash_sync(&mut self) {
        self.flash.now = self.cycles;
        self.flash.hz = self.cpu_hz;
    }

    pub fn flash_busy(&mut self) -> bool {
        self.flash_sync();
        self.flash.busy()
    }

    pub fn flash_busy_left_us(&mut self) -> f64 {
        self.flash_sync();
        self.flash.busy_left_us()
    }

    pub fn flash_select(&mut self, low: bool) -> Result<(), Error> {
        self.flash_sync();
        let r = self.flash.select(low);
        self.flash_wrote();
        r
    }

    pub fn flash_clock(&mut self, b: u8) -> Result<u8, Error> {
        self.flash_sync();
        self.flash.clock(b)
    }

    fn flash_wrote(&mut self) {
        if self.flash.wrote_events.is_empty() {
            return;
        }
        let evs = std::mem::take(&mut self.flash.wrote_events);
        if let Some(h) = self.on_flash_write.clone() {
            for (op, addr, n) in evs {
                h(self, op, addr, n);
            }
        }
    }

    pub fn reset(&mut self, boot_slot: i64, cause_wd: bool) -> Result<(), Error> {
        let mut boot_slot = boot_slot;
        if boot_slot < 0 {
            boot_slot = -1;
            for s in [0x0usize, 0x20000, 0x40000, 0x80000] {
                if s < self.flash.size && self.flash.mem[s + 8] == 0x4B {
                    boot_slot = s as i64;
                    break;
                }
            }
            if boot_slot < 0 {
                return Err(emu_err("no bootable slot (byte 8 = 0x4b)"));
            }
        }
        let boot_slot = boot_slot as usize;
        self.boot_slot = boot_slot;
        self.flash.abort();
        let (mut busy_left, mut busy_hz) = (0i64, CPU_HZ);
        if self.reset_count > 0 {
            if self.flash_busy() {
                busy_left = self.flash.busy_until - self.cycles;
            }
            busy_hz = self.cpu_hz;
        }
        self.analog_log = Vec::new();
        self.analog_writes = Vec::new();
        for t in self.icache_tags.iter_mut() {
            *t = -1;
        }
        let mut copy_len = (le32(&self.flash.mem, boot_slot + 0x0C) & 0xFFFF) as usize * 16;
        if copy_len > self.cfg.sram_size {
            copy_len = self.cfg.sram_size;
        }
        self.sram[..copy_len].copy_from_slice(&self.flash.mem[boot_slot..boot_slot + copy_len]);
        let mut keep = [0u8; 3];
        keep.copy_from_slice(&self.analog[0x3A..0x3D]);
        self.regs = vec![0u8; REG_SIZE];
        self.analog = [0; 256];
        self.adc_on = false;
        self.dfifo2_on = false;
        self.dfifo0_on = false;
        self.dfifo1_on = false;
        self.pwm0_clr_frame = 0;
        self.qdec_init = false;
        self.qdec_count = 0;
        self.qdec_sub = 0;
        self.pwm_fifo = Vec::new();
        self.pwm_ir_done = false;
        self.regs[0xD3] = 0x01;
        self.regs[0xD7] = 0x01;
        self.pke_done_at = -1;
        self.pke_rt = 0;
        self.pke_pending = Vec::new();
        self.analog[0x35] = 0x20;
        self.analog[0x39] = 0xFF;
        self.analog[0x3A..0x3D].copy_from_slice(&keep);
        self.pad_hold = [0xFF; 4];
        self.analog[0x7F] = 0x01;
        if cause_wd {
            self.regs[0x72] = 1;
        }
        self.regs[0x74A] = STIMER_CTRL_RESET;
        for oen in [0x582usize, 0x58A, 0x592, 0x59A, 0x5A2] {
            self.regs[oen] = self.gpio_oen_reset;
        }
        self.regs[0x586] = 0x1F;
        self.regs[0x58E] = 0x3F;
        self.regs[0x596] = 0xFF;
        self.regs[0x59E] = 0x7B;
        if boot_slot != 0 {
            self.regs[0x63E] = 0x02;
        }
        self.r = [0; 16];
        self.bank = [[0; 2]; 2];
        self.spsr = [0; 2];
        self.mode = MODE_SVC;
        self.ibit = 1;
        self.n = 0;
        self.z = 0;
        self.c = 0;
        self.v = 0;
        self.cycles = 0;
        self.idle_skipped = 0;
        self.cpu_hz = CPU_HZ;
        self.ms_base = 0.0;
        self.ms_base_cyc = 0;
        self.flash.busy_until = self.flash_busy_until(busy_left, busy_hz);
        self.tick_base = self.cfg.tick_offset;
        self.tick_base_cy = 0;
        self.stimer_on = false;
        self.stimer_cmp = 0;
        self.irq_latch = 0;
        self.gpio_req = false;
        self.last_tick = self.cfg.tick_offset & !7;
        self.tick2_base = 0;
        self.tmr_on = [false; 2];
        self.tmr_base = [0; 2];
        self.tmr_wrap = [false; 2];
        self.tmr2_on = false;
        self.tmr2_last = 0;
        self.wd_fed = 0;
        self.mspi_auto = false;
        self.spi_busy_until = 0;
        self.spi_rx = 0;
        if let Some(d) = self.spi_dev.as_mut() {
            d.selected = false;
        }
        self.boot_copy = copy_len;
        self.asleep = None;
        self.stalled = false;
        self.stall_since = 0;
        self.stall_cycles = 0;
        self.sleep_since = 0.0;
        self.sleep_deep = false;
        self.k32_wake = None;
        self.k32_count = 0;
        self.k32_next = crate::rbg::K32_PERIOD_MS;
        self.aes_in = Vec::new();
        self.aes_out = Vec::new();
        self.aes_ccm = false;
        self.aes_prev = [0; 16];
        self.reset_count += 1;
        let n = self.reset_hooks.len();
        for i in 0..n {
            if i >= self.reset_hooks.len() {
                break;
            }
            let f = self.reset_hooks[i].clone();
            f(self);
        }
        if self.has_event() {
            let wd = if cause_wd { " (watchdog)" } else { "" };
            let text = format!("reset #{}: boot slot 0x{:05x}{}", self.reset_count, boot_slot, wd);
            self.event(&text);
        }
        Ok(())
    }

    #[inline]
    pub fn ms(&self) -> f64 {
        self.ms_base + (self.cycles.wrapping_sub(self.ms_base_cyc).wrapping_mul(1000)) as f64 / self.cpu_hz as f64
    }

    pub fn sysclk(&self) -> i64 {
        let sel = self.regs[0x66] as i64;
        let fhs_sel = ((self.regs[0x70] & 1) as i64) << 1 | (sel >> 7);
        let fhs: i64 = if fhs_sel != 0 { 24_000_000 } else { 48_000_000 };
        match (sel >> 5) & 3 {
            0 => 24_000_000,
            1 => fhs,
            2 => {
                let mut d = sel & 0x1F;
                if d < 2 {
                    d = 2;
                }
                fhs / d
            }
            _ => 32_000_000,
        }
    }

    pub(crate) fn set_cpu_hz(&mut self, hz: i64) {
        if hz == self.cpu_hz {
            return;
        }
        let now_tick = self.stimer_raw();
        self.ms_base = self.ms();
        self.ms_base_cyc = self.cycles;
        self.tick_base = now_tick;
        self.tick_base_cy = self.cycles;
        let (mut busy_left, old_hz) = (0i64, self.cpu_hz);
        if self.flash_busy() {
            busy_left = self.flash.busy_until - self.cycles;
        }
        self.cpu_hz = hz;
        self.flash.busy_until = self.flash_busy_until(busy_left, old_hz);
        if self.has_event() {
            let text = format!("CPU clock {} MHz", hz / 1_000_000);
            self.event(&text);
        }
    }

    fn flash_busy_until(&self, left: i64, hz: i64) -> i64 {
        if left <= 0 {
            return 0;
        }
        self.cycles + (left * self.cpu_hz + hz - 1) / hz
    }

    #[inline]
    pub(crate) fn stimer_raw(&self) -> u32 {
        if !self.stimer_on || self.stimer_stuck {
            return self.tick_base;
        }
        ((self.tick_base as i64).wrapping_add(
            self.cycles.wrapping_sub(self.tick_base_cy).wrapping_mul(STIMER_HZ) / self.cpu_hz,
        ) & M32) as u32
    }

    #[inline]
    pub fn stimer_now(&self) -> u32 {
        self.stimer_raw() & !7
    }

    pub(crate) fn stimer_start(&mut self, on: bool) {
        if on != self.stimer_on {
            self.tick_base = self.stimer_raw();
            self.tick_base_cy = self.cycles;
            self.stimer_on = on;
            self.last_tick = self.stimer_now();
            self.event(if on { "system timer started" } else { "system timer stopped" });
        }
        if on {
            self.regs[0x74A] |= STIMER_EN;
        } else {
            self.regs[0x74A] &= !STIMER_EN;
        }
    }

    pub fn stimer_on(&self) -> bool {
        self.stimer_on
    }

    pub(crate) fn k32_now(&mut self) -> u32 {
        if let Some(v) = self.k32_stopped {
            return v;
        }
        if self.k32_sigma_ns == 0 {
            return (((self.ms() * 32.768) as i64) & M32) as u32;
        }
        self.k32_jittered()
    }

    #[inline]
    pub(crate) fn tick2_now(&self) -> u32 {
        (self.cycles.wrapping_sub(self.tick2_base) & M32) as u32
    }

    pub fn update_time(&mut self) -> Result<(), Error> {
        if self.regs[0xB10] & 0x04 != 0 {
            self.dfifo2_fill();
        } else {
            self.adc_on = false;
        }
        let now = self.stimer_now();
        if self.stimer_on && self.regs[0x748] & STIMER_IRQ != 0 {
            if now.wrapping_sub(self.stimer_cmp) < 0x80000000 && self.last_tick.wrapping_sub(self.stimer_cmp) >= 0x80000000 {
                self.irq_latch |= 1 << IRQ_STIMER;
            }
        }
        self.last_tick = now;
        self.timers_update();
        self.qdec_sample();
        self.pwm_fifo_drain();
        let ctrl = le32(&self.regs, 0x620);
        if ctrl & (1 << 23) != 0 && ctrl & (1 << 6) != 0 {
            let capture = (ctrl >> 9) & 0x3FFF;
            if capture != 0 && (self.tick2_now() >> 18) >= capture {
                return Err(Error::WatchdogReset);
            }
        }
        if let Some(h) = self.update_time_hook.clone() {
            return h(self);
        }
        Ok(())
    }

    fn tmr_enabled(&self, n: usize) -> bool {
        let ctrl = self.regs[0x620];
        let (en_bit, sh) = if n == 1 { (0x08u8, 4) } else { (0x01u8, 1) };
        ctrl & en_bit != 0 && (ctrl >> sh) & 3 == 0
    }

    pub(crate) fn tmr_tick(&self, n: usize) -> u32 {
        if !self.tmr_on[n] {
            return le32(&self.regs, 0x630 + 4 * n);
        }
        (self.cycles.wrapping_sub(self.tmr_base[n]) & M32) as u32
    }

    pub(crate) fn timers_update(&mut self) {
        for n in 0..2 {
            if !self.tmr_on[n] {
                continue;
            }
            let cap = le32(&self.regs, 0x624 + 4 * n) as i64;
            if cap == 0 {
                continue;
            }
            let mut elapsed = self.cycles - self.tmr_base[n];
            if self.tmr_wrap[n] {
                if elapsed < 1 << 32 {
                    continue;
                }
                self.tmr_wrap[n] = false;
                self.tmr_base[n] += 1 << 32;
                elapsed -= 1 << 32;
                if self.has_event() {
                    let text = format!("timer {}: the count wrapped round to 0 (capture 0x{:x})", n, cap);
                    self.event(&text);
                }
            }
            if elapsed >= cap {
                self.regs[0x623] |= 1 << n;
                self.tmr_base[n] += (elapsed / cap) * cap;
            }
        }
        if self.tmr2_on {
            self.tmr2_update();
        }
    }

    pub(crate) fn tmr2_update(&mut self) {
        let now = self.tick2_now();
        let cap = le32(&self.regs, 0x62C);
        if cap != 0 && self.tmr2_mode0() {
            let d = now.wrapping_sub(self.tmr2_last);
            if d != 0 && cap.wrapping_sub(self.tmr2_last).wrapping_sub(1) < d {
                self.regs[0x623] |= 0x04;
            }
        }
        self.tmr2_last = now;
    }

    fn tmr2_mode0(&self) -> bool {
        self.regs[0x620] & 0x80 == 0 && self.regs[0x621] & 0x01 == 0
    }

    fn cycles_to_tmr2(&self) -> Option<i64> {
        let cap = le32(&self.regs, 0x62C);
        if !self.tmr2_on || cap == 0 || !self.tmr2_mode0() {
            return None;
        }
        match cap.wrapping_sub(self.tick2_now()) {
            0 => Some(1 << 32),
            d => Some(d as i64),
        }
    }

    fn tmr_check_past(&mut self, n: usize, what: &str) -> Result<(), Error> {
        let cap = le32(&self.regs, 0x624 + 4 * n);
        if self.tmr_on[n] {
            self.tmr_base[n] = self.cycles - self.tmr_tick(n) as i64;
        }
        if !self.tmr_on[n] || cap == 0 {
            self.tmr_wrap[n] = false;
            return Ok(());
        }
        let tick = self.tmr_tick(n);
        if tick < cap {
            self.tmr_wrap[n] = false;
            return Ok(());
        }
        let wait_s = ((1u64 << 32) - tick as u64 + cap as u64) as f64 / self.cpu_hz as f64;
        let text = format!(
            "timer {}: {} leaves the count 0x{:x} at or past the capture 0x{:x}: the chip matches on equality, so the next match comes when the count has wrapped, {:.1} s on, at {}",
            n,
            what,
            tick,
            cap,
            wait_s,
            self.here()
        );
        if self.cfg.timer_capture_below == "stop" {
            return Err(emu_err(text));
        }
        self.tmr_wrap[n] = true;
        self.event(&text);
        Ok(())
    }

    fn tmr_ctrl_written(&mut self) -> Result<(), Error> {
        for n in 0..2 {
            let on = self.tmr_enabled(n);
            if on && !self.tmr_on[n] {
                let start = le32(&self.regs, 0x630 + 4 * n) as i64;
                self.tmr_base[n] = self.cycles - start;
                self.tmr_on[n] = true;
                self.tmr_check_past(n, "a start")?;
            } else if !on && self.tmr_on[n] {
                let t = self.tmr_tick(n);
                put32(&mut self.regs, 0x630 + 4 * n, t);
                self.tmr_on[n] = false;
                self.tmr_wrap[n] = false;
            }
        }
        let on2 = self.regs[0x620] & 0x40 != 0;
        if on2 && !self.tmr2_on {
            self.tmr2_last = self.tick2_now();
        }
        self.tmr2_on = on2;
        Ok(())
    }

    pub fn cycles_to_stimer(&self, target: u32) -> i64 {
        if !self.stimer_on {
            return 0;
        }
        let delta = target.wrapping_sub(self.stimer_now());
        if delta >= 0x80000000 {
            return 0;
        }
        delta as i64 * self.cpu_hz / STIMER_HZ + 1
    }

    pub fn cycles_to_timer_match(&self) -> Option<i64> {
        let mut best = 0i64;
        let mut found = false;
        for n in 0..2 {
            if !self.tmr_on[n] || self.regs[0x640] & (1 << n) == 0 {
                continue;
            }
            let cap = le32(&self.regs, 0x624 + 4 * n) as i64;
            if cap == 0 {
                continue;
            }
            let mut left = self.tmr_base[n] + cap - self.cycles;
            if self.tmr_wrap[n] {
                left += 1 << 32;
            }
            if left < 0 {
                left = 0;
            }
            if !found || left < best {
                best = left;
                found = true;
            }
        }
        if found {
            Some(best)
        } else {
            None
        }
    }

    pub(crate) fn sram_code_end(&mut self) -> usize {
        if let Some(h) = self.sram_code_end_hook.clone() {
            return h(self);
        }
        let e = (self.regs[0x60C] as usize) << 8;
        if self.boot_copy > e {
            return self.boot_copy;
        }
        e
    }

    pub(crate) fn xip(&self, a: usize) -> Result<usize, Error> {
        let s = self.boot_slot;
        if a < s {
            return Ok(a + s);
        }
        if a < 2 * s {
            return Err(emu_err(format!(
                "XIP access 0x{:x} with boot slot 0x{:x}: what 0x{:x}-0x{:x} reads then is not documented, at {}",
                a,
                s,
                s,
                2 * s - 1,
                self.here()
            )));
        }
        Ok(a)
    }

    #[inline]
    pub fn read(&mut self, a: u32, size: usize) -> Result<u32, Error> {
        if (self.mem_read_hook.is_none() || a >= self.mem_read_hook_below)
            && a & (size as u32).wrapping_sub(1) == 0
            && SRAM_BASE <= a
            && a < REG_ALIAS
            && a as usize >= self.cfg.flash_size
        {
            let o = (a - SRAM_BASE) as usize;
            if o + size <= self.cfg.sram_size && o + size <= self.sram.len() {
                return Ok(le_int(&self.sram[o..o + size]));
            }
        }
        self.read_slow(a, size)
    }

    #[inline(never)]
    fn read_slow(&mut self, a: u32, size: usize) -> Result<u32, Error> {
        if a < self.mem_read_hook_below {
            if let Some(h) = self.mem_read_hook.clone() {
                if let Some(v) = h(self, a, size)? {
                    return Ok(v);
                }
            }
        }
        if a & (size as u32).wrapping_sub(1) != 0 {
            if let Some(h) = self.unaligned_read_hook.clone() {
                if let Some(v) = h(self, a, size)? {
                    return Ok(v);
                }
            }
        }
        if REG_ALIAS <= a && a < REG_ALIAS + REG_SIZE as u32 {
            return self.reg_read((a - REG_ALIAS) as usize, size);
        }
        if (a as usize) < self.cfg.flash_size {
            if self.flash.dpd && a as usize >= self.sram_code_end() {
                return Err(emu_err(format!(
                    "flash read 0x{:x} while the flash is in deep power-down (0xb9) at {}",
                    a,
                    self.here()
                )));
            }
            if self.flash.busy_until != 0 && a as usize >= self.sram_code_end() && self.flash_busy() {
                let left = self.flash_busy_left_us();
                return Err(emu_err(format!(
                    "flash read 0x{:x} while the flash is busy with a program or erase (WIP, {:.0} us left) at {}",
                    a,
                    left,
                    self.here()
                )));
            }
            if self.flash.cs_low && self.flash.spi_busy() && a as usize >= self.sram_code_end() {
                return Err(emu_err(format!(
                    "XIP read 0x{:x} while SPI command 0x{:02x} is underway ({} bytes sent, chip select low) at {}",
                    a,
                    self.flash.tx[0],
                    self.flash.tx.len(),
                    self.here()
                )));
            }
            let p = self.xip(a as usize)?;
            if p + size > self.flash.size {
                return Err(emu_err(format!(
                    "XIP read 0x{:x} past the end of flash (boot slot 0x{:x}) at {}",
                    a,
                    self.boot_slot,
                    self.here()
                )));
            }
            if self.icache_miss != 0 && a as usize >= self.sram_code_end() {
                let mut line = (a as i64) >> ICACHE_LINE_SHIFT;
                let last = (a as i64 + size as i64 - 1) >> ICACHE_LINE_SHIFT;
                loop {
                    let slot = (line % ICACHE_LINES as i64) as usize;
                    if self.icache_tags[slot] != line {
                        self.icache_tags[slot] = line;
                        self.icache_misses += 1;
                        self.cycles += self.icache_miss;
                    }
                    if line == last {
                        break;
                    }
                    line += 1;
                }
            }
            return Ok(le_int(&self.flash.mem[p..p + size]));
        }
        if SRAM_BASE <= a && a < SRAM_BASE + self.cfg.sram_size as u32 {
            let o = (a - SRAM_BASE) as usize;
            if o + size > self.cfg.sram_size {
                return Err(emu_err(format!(
                    "read{} 0x{:08x} crosses the end of SRAM at {}",
                    size * 8,
                    a,
                    self.here()
                )));
            }
            return Ok(le_int(&self.sram[o..o + size]));
        }
        if REG_BASE <= a && a < REG_BASE + REG_SIZE as u32 {
            return self.reg_read((a - REG_BASE) as usize, size);
        }
        Err(emu_err(format!("read{} 0x{:08x} at {}", size * 8, a, self.here())))
    }

    #[inline]
    pub fn write(&mut self, a: u32, size: usize, val: u32) -> Result<(), Error> {
        if (self.mem_write_hook.is_none() || a >= self.mem_write_hook_below) && SRAM_BASE <= a && a < REG_ALIAS {
            let o = (a - SRAM_BASE) as usize;
            if o + size <= self.cfg.sram_size && o + size <= self.sram.len() {
                for i in 0..size {
                    self.sram[o + i] = (val >> (8 * i)) as u8;
                }
                return Ok(());
            }
        }
        self.write_slow(a, size, val)
    }

    #[inline(never)]
    fn write_slow(&mut self, a: u32, size: usize, val: u32) -> Result<(), Error> {
        if let Some(h) = self.mem_write_hook.clone() {
            if h(self, a, size, val & size_mask(size))? {
                return Ok(());
            }
        }
        if REG_ALIAS <= a && a < REG_ALIAS + REG_SIZE as u32 {
            return self.reg_write((a - REG_ALIAS) as usize, size, val & size_mask(size));
        }
        if SRAM_BASE <= a && a < SRAM_BASE + self.cfg.sram_size as u32 {
            let o = (a - SRAM_BASE) as usize;
            if o + size > self.cfg.sram_size {
                return Err(emu_err(format!(
                    "write{} 0x{:08x} crosses the end of SRAM at {}",
                    size * 8,
                    a,
                    self.here()
                )));
            }
            for i in 0..size {
                self.sram[o + i] = (val >> (8 * i)) as u8;
            }
            return Ok(());
        }
        if REG_BASE <= a && a < REG_BASE + REG_SIZE as u32 {
            return self.reg_write((a - REG_BASE) as usize, size, val & size_mask(size));
        }
        Err(emu_err(format!("write{} 0x{:08x} = 0x{:x} at {}", size * 8, a, val, self.here())))
    }

    pub fn reg_read(&mut self, o: usize, size: usize) -> Result<u32, Error> {
        if self.reg_read_hook.is_some() && self.reg_hook_mask.as_ref().is_none_or(|k| k.read_hooked(o, size)) {
            let h = self.reg_read_hook.clone().unwrap();
            if let Some(v) = h(self, o, size)? {
                return Ok(v);
            }
        }
        if o < 0x638 && o + size > 0x620 {
            self.timers_update();
        }
        if o == 0x740 && size == 4 {
            return Ok(self.stimer_now());
        } else if o == 0x638 && size == 4 {
            return Ok(self.tick2_now());
        } else if (o == 0x630 || o == 0x634) && size == 4 {
            return Ok(self.tmr_tick((o - 0x630) / 4));
        } else if (0x648..=0x64B).contains(&o) {
            self.update_time()?;
            let val = (self.irq_sources() & 0xFFFFFF).to_le_bytes();
            return Ok(le_int(clip(&val, o - 0x648, size)));
        } else if o == 0x08 && size == 1 && self.spi_dev.is_some() {
            let v = self.spi_rx;
            if self.spi_running() && self.regs[0x09] & 0x08 != 0 {
                self.spi_octet(-1);
            }
            return Ok(v as u32);
        } else if o == 0x09 && size == 1 && self.spi_dev.is_some() {
            let mut v = self.regs[0x09] & !0x40;
            if self.cycles < self.spi_busy_until {
                v |= 0x40;
            }
            return Ok(v as u32);
        } else if o == 0xFFD8 && size == 4 {
            return Ok(match &self.spi_dev {
                None => 0,
                Some(d) => ((d.transactions & 0xFFFF) as u32) << 16 | (d.count & 0xFFFF) as u32,
            });
        } else if o == 0xFFDC && size == 1 {
            return Ok(match self.spi_dev.as_mut() {
                Some(d) if !d.fifo.is_empty() => d.fifo.remove(0) as u32,
                _ => 0,
            });
        } else if o == 0x0C && size == 1 {
            if self.flash.beyond >= 0 {
                if self.flash.beyond_mode == "stop" {
                    return Err(emu_err(format!(
                        "flash read of 0x{:x}: the part has 0x{:x} bytes at {}",
                        self.flash.beyond,
                        self.flash.size,
                        self.here()
                    )));
                }
                let b = self.flash.beyond;
                self.flash.log.push(crate::flash::FlashOp {
                    op: "read beyond".to_string(),
                    addr: b,
                    len: 1,
                    data: String::new(),
                });
            }
            if self.mspi_auto {
                let v = self.flash.rx;
                let rx = self.flash_clock(0x00)?;
                self.flash.rx = rx;
                return Ok(v as u32);
            }
            return Ok(self.flash.rx as u32);
        } else if o == 0x0D && size == 1 {
            return Ok((self.regs[0x0D] & !0x10) as u32);
        } else if o == 0xBA && size == 1 {
            return Ok((self.regs[0xBA] & !0x01) as u32);
        } else if o == crate::pke::PKE_STAT
            || o == crate::pke::PKE_RT_CODE
            || (crate::pke::PKE_RAM_A <= o && o < crate::pke::PKE_RAM_END)
        {
            if let Some(v) = self.pke_read(o, size) {
                return Ok(v);
            }
        } else if o == 0x4408 && size == 1 {
            if self.rbg_ready() {
                return Ok((self.regs[0x4408] | 0x01) as u32);
            }
            return Ok((self.regs[0x4408] & 0xFE) as u32);
        } else if o == 0x440C && size == 4 {
            return Ok(self.rbg_word());
        } else if o == 0x548 && size == 4 {
            return Ok(self.aes_read());
        } else if o == 0x74B && size == 1 {
            if self.cfg.k32_frozen {
                return Ok(0);
            }
            return Ok((self.k32_now() & 1) << 5);
        } else if o == 0x74F && size == 1 {
            return Ok(if self.stimer_on { 0x02 } else { 0 });
        } else if o == 0x750 && size == 4 {
            if self.cfg.k32_frozen {
                return Ok(0);
            }
            return Ok(self.k32_now());
        } else if o == 0x754 && size == 4 {
            return Ok((STIMER_HZ * 16 / 32768) as u32);
        } else if o == 0xD0 && size == 1 {
            let v = (self.qdec_count as u32) & 0xFF;
            self.qdec_count = 0;
            self.qdec_sub = 0;
            return Ok(v);
        } else if o == 0x90 && size == 1 {
            return Ok(self.uart_rx_pop() as u32);
        } else if o == 0x9C && size == 1 {
            return Ok(self.uart_buf_cnt());
        } else if o == 0x9D && size == 1 {
            return Ok(0x01);
        } else if o == 0x9E && size == 1 {
            return Ok(if !self.uart_rx.is_empty() { 0x04 } else { 0 });
        } else if o == 0x7B1 && size == 1 {
            let mut v = 0u32;
            if self.pwm_ir_done {
                v |= 0x02;
            }
            if self.pwm_frames() > self.pwm0_clr_frame {
                v |= 0x04;
            }
            return Ok(v);
        } else if o == 0x7B3 && size == 1 {
            return Ok(self.regs[0x7B3] as u32 & 0x01);
        } else if o == 0x7CD && size == 1 {
            return Ok(self.pwm_fifo_sr());
        } else if (o == 0x0B5E || o == 0x0B5F) && size == 1 {
            if self.regs[0x0B42] & 0x80 != 0 {
                return Ok((self.regs[0x0B43] & 0x7F) as u32);
            }
            return Ok((self.regs[0x0B41] & 0x3F) as u32);
        } else if o == 0x7B4 && size == 2 {
            return Ok(self.pwm_count(0));
        } else if o == 0x7BE && size == 2 {
            return Ok(self.pwm_count(5));
        } else if (o == 0x0B16 || o == 0x0B17 || o == 0x0B1A || o == 0x0B1B) && size == 1 {
            let (mut on, mut t0) = (self.dfifo0_on, self.dfifo0_t0);
            let mut hi = o == 0x0B17;
            if o == 0x0B1A || o == 0x0B1B {
                on = self.dfifo1_on;
                t0 = self.dfifo1_t0;
                hi = o == 0x0B1B;
            }
            let wptr = (self.dfifo_audio_samples(on, t0).wrapping_mul(2) as u32) & 0x0FFF;
            if hi {
                return Ok((wptr >> 8) & 0x0F);
            }
            return Ok(wptr & 0xFF);
        } else if (o == 0x0B20 || o == 0x0B21 || o == 0x0B24 || o == 0x0B25) && size == 1 {
            let (mut on, mut t0) = (self.dfifo0_on, self.dfifo0_t0);
            let mut hi = o == 0x0B21;
            if o == 0x0B24 || o == 0x0B25 {
                on = self.dfifo1_on;
                t0 = self.dfifo1_t0;
                hi = o == 0x0B25;
            }
            let mut n = self.dfifo_audio_samples(on, t0);
            if n > 0x7FF {
                n = 0x7FF;
            }
            if hi {
                return Ok(((n as u32) >> 8) & 0xFF);
            }
            return Ok((n as u32) & 0xFF);
        } else if (o == 0x0B1E && (size == 1 || size == 2)) || (o == 0x0B1F && size == 1) {
            let depth = self.dfifo2_depth();
            if self.dfifo2_on && depth > 0 {
                let wptr = (self.dfifo2_samples().wrapping_mul(4) as u32) % (depth as u32);
                if o == 0x0B1E && size == 2 {
                    return Ok(wptr & 0x0FFF);
                } else if o == 0x0B1E {
                    return Ok(wptr & 0xFF);
                }
                return Ok((wptr >> 8) & 0x0F);
            }
            return Ok(0);
        } else if (o == 0x0B28 && (size == 1 || size == 2)) || (o == 0x0B29 && size == 1) {
            let depth = self.dfifo2_depth();
            if self.dfifo2_on && depth > 0 {
                let mut num = self.dfifo2_samples();
                let cp = (depth / 4) as i64;
                if num > cp {
                    num = cp;
                }
                if o == 0x0B28 && size == 2 {
                    return Ok((num as u32) & 0xFFFF);
                } else if o == 0x0B28 {
                    return Ok((num as u32) & 0xFF);
                }
                return Ok(((num as u32) >> 8) & 0xFF);
            }
            return Ok(0);
        } else if (o == 0x580 || o == 0x588 || o == 0x590 || o == 0x598) && size == 1 {
            let port = (o - 0x580) / 8;
            let v = self.pad_inputs(port);
            let mut ie = self.regs[o + 1];
            if port == 2 {
                ie = self.analog[0xC0];
            }
            return Ok((v & ie) as u32);
        }
        Ok(le_int(clip(&self.regs, o, size)))
    }

    pub(crate) fn store(&mut self, o: usize, size: usize, val: u32) {
        let mut i = 0;
        while i < size && o + i < self.regs.len() {
            self.regs[o + i] = (val >> (8 * i)) as u8;
            i += 1;
        }
    }

    pub fn reg_write(&mut self, o: usize, size: usize, mut val: u32) -> Result<(), Error> {
        if self.reg_write_hook.is_some() {
            if self.reg_hook_mask.as_ref().is_some_and(|k| !k.write_hooked(o, size)) {
                let h = self.reg_write_hook.take();
                let res = self.reg_write(o, size, val);
                self.reg_write_hook = h;
                return res;
            }
            let h = self.reg_write_hook.clone().unwrap();
            if h(self, o, size, val)? {
                return Ok(());
            }
        }
        if (o == 0x540 && size == 1) || (o == 0x548 && size == 4) {
            self.aes_write(o, val);
            return Ok(());
        } else if o == 0x07 && size == 1 {
            self.regs[o] = val as u8;
            self.i2c_launch(val as u8);
            return Ok(());
        } else if o == 0x90 && size == 1 {
            self.uart_tx_byte(val as u8);
            return Ok(());
        } else if o == 0xD6 && size == 1 {
            self.qdec_count = 0;
            self.qdec_sub = 0;
            self.regs[o] = val as u8;
            return Ok(());
        } else if o == 0x7B1 && size == 1 {
            if val & 0x04 != 0 {
                self.pwm0_clr_frame = self.pwm_frames();
            }
            if val & 0x02 != 0 {
                self.pwm_ir_done = false;
            }
            self.regs[o] = val as u8;
            return Ok(());
        } else if o == 0x7B3 && size == 1 {
            if val & 0x01 != 0 {
                self.regs[0x7B3] &= !0x01;
            }
            return Ok(());
        } else if o == 0x7C8 && (size == 1 || size == 2 || size == 4) {
            self.pwm_fifo_push(val);
            return Ok(());
        } else if o == 0x7CE && size == 1 {
            if val & 0x01 != 0 {
                self.pwm_fifo.clear();
            }
            self.regs[o] = val as u8;
            return Ok(());
        } else if o == 0xFFE4 && size == 4 {
            return self.adc_pin_control(o, val);
        } else if o == 0xFFD8 && size == 4 {
            return self.spi_attach(val);
        } else if o == 0x08 && size == 1 && self.spi_running() {
            if self.cycles < self.spi_busy_until {
                return Err(emu_err(format!("SPI: 0x08 written while an octet is going out at {}", self.here())));
            }
            self.regs[0x08] = val as u8;
            let mut out = (val & 0xFF) as i64;
            if self.regs[0x09] & 0x04 != 0 {
                out = -1;
            }
            self.spi_octet(out);
            return Ok(());
        } else if o == 0x09 && size == 1 && self.spi_dev.is_some() {
            self.regs[0x09] = (val as u8) & !0x40;
            self.spi_cs_update();
            return Ok(());
        } else if o == crate::pke::PKE_CTRL || o == crate::pke::PKE_STAT || o == 0xFFEC {
            if let Some(r) = self.pke_write_reg(o, size, val) {
                return r;
            }
        } else if o == 0xFFE8 && size == 4 {
            return self.adc_control(o, val);
        } else if (o == 0xFFF4 || o == 0xFFF8 || o == 0xFFFC) && size == 4 {
            return self.rbg_control(o, val);
        } else if o == 0x740 && size == 4 {
            self.tick_base = val;
            self.tick_base_cy = self.cycles;
            self.last_tick = val & !7;
            return Ok(());
        } else if o == 0x744 && size == 4 {
            val &= !7;
            self.stimer_cmp = val;
            self.last_tick = self.stimer_now();
            put32(&mut self.regs, o, val);
            return Ok(());
        } else if o == 0x660 && size == 1 {
            let mut a = le32(&self.regs, 0x664) as i64;
            let mut b = le32(&self.regs, 0x668) as i64;
            if val & 1 != 0 {
                a = a as i32 as i64;
                b = b as i32 as i64;
            }
            let (q, rem);
            if b == 0 {
                q = -1;
                rem = a;
            } else {
                let (mut aa, mut bb) = (a, b);
                if aa < 0 {
                    aa = -aa;
                }
                if bb < 0 {
                    bb = -bb;
                }
                let mut qq = aa / bb;
                if (a < 0) != (b < 0) {
                    qq = -qq;
                }
                q = qq;
                rem = a - q * b;
            }
            put32(&mut self.regs, 0x664, (q & M32) as u32);
            put32(&mut self.regs, 0x668, (rem & M32) as u32);
            self.regs[0x660] = 0;
            self.div_ops += 1;
            return Ok(());
        } else if o == 0x608 && size == 4 {
            if val & (1 << 24) != 0 {
                let off = (val & 0xFFFFFF) as usize;
                let src = self.xip(off)?;
                if off + 16 > self.cfg.sram_size {
                    return Err(emu_err(format!(
                        "load of flash into SRAM offset 0x{:x}, past the end of SRAM at {}",
                        off,
                        self.here()
                    )));
                }
                if src + 16 > self.flash.size {
                    return Err(emu_err(format!(
                        "load of flash 0x{:x} into SRAM: the part has 0x{:x} bytes at {}",
                        src,
                        self.flash.size,
                        self.here()
                    )));
                }
                let (sram, mem) = (&mut self.sram, &self.flash.mem);
                sram[off..off + 16].copy_from_slice(&mem[src..src + 16]);
            }
            put32(&mut self.regs, o, val & 0x00FFFFFF);
            return Ok(());
        } else if o == 0x74A && size == 1 {
            self.stimer_start(val as u8 & STIMER_EN != 0);
            self.regs[o] = val as u8;
            return Ok(());
        } else if o == 0x74F && size == 1 {
            if val & 0x01 != 0 {
                self.stimer_start(true);
            } else if val & 0x02 != 0 {
                self.stimer_start(false);
            }
            self.regs[o] = val as u8;
            return Ok(());
        } else if o == 0x74C && size == 1 {
            self.regs[o] = val as u8;
            if val & 0x02 != 0 {
                self.regs[0x748] |= STIMER_IRQ;
            } else {
                self.regs[0x748] &= !STIMER_IRQ;
            }
            return Ok(());
        } else if o == 0xB10 && size == 1 {
            if val & 0x01 != 0 && !self.dfifo0_on {
                self.dfifo0_t0 = self.cycles;
            }
            self.dfifo0_on = val & 0x01 != 0;
            if val & 0x02 != 0 && !self.dfifo1_on {
                self.dfifo1_t0 = self.cycles;
            }
            self.dfifo1_on = val & 0x02 != 0;
            if val & 0x04 != 0 && !self.dfifo2_on {
                self.dfifo2_t0 = self.cycles;
            }
            self.dfifo2_on = val & 0x04 != 0;
            self.regs[o] = val as u8;
            return Ok(());
        } else if o == 0x638 && size == 4 {
            if self.tmr2_on {
                self.tmr2_update();
            }
            self.tick2_base = self.cycles.wrapping_sub(val as i64) & M32;
            self.tmr2_last = self.tick2_now();
            return Ok(());
        } else if (0x648..=0x64B).contains(&o) {
            self.irq_latch &= !(val << (8 * (o - 0x648)));
            return Ok(());
        } else if (o == 0x630 || o == 0x634) && size == 4 {
            let n = (o - 0x630) / 4;
            self.timers_update();
            put32(&mut self.regs, o, val);
            if self.tmr_on[n] {
                self.tmr_base[n] = self.cycles - val as i64;
                return self.tmr_check_past(n, "a count write");
            }
            return Ok(());
        } else if (0x624..0x62C).contains(&o) {
            self.timers_update();
            self.store(o, size, val);
            let (first, last) = ((o - 0x624) / 4, (o + size - 1 - 0x624) / 4);
            let mut n = first;
            while n <= last && n < 2 {
                self.tmr_check_past(n, "a capture write")?;
                n += 1;
            }
            return Ok(());
        } else if (0x62C..0x630).contains(&o) {
            if self.tmr2_on {
                self.tmr2_update();
            }
            self.store(o, size, val);
            self.tmr2_last = self.tick2_now();
            return Ok(());
        } else if o == 0x623 && size == 1 {
            self.regs[0x623] &= !((val & 0x07) as u8);
            if val & 0x08 != 0 {
                self.feed();
            }
            return Ok(());
        } else if o == 0x620 && (size == 1 || size == 2 || size == 4) {
            self.timers_update();
            if size == 4 {
                self.regs[0x620] = val as u8;
                self.regs[0x621] = (val >> 8) as u8;
                self.regs[0x622] = (val >> 16) as u8;
                self.regs[0x623] &= !(((val >> 24) & 0x07) as u8);
                if val & (1 << 27) != 0 {
                    self.feed();
                }
            } else {
                self.store(o, size, val);
            }
            return self.tmr_ctrl_written();
        } else if o == 0x72 && size == 1 {
            self.regs[0x72] &= !(val as u8);
            return Ok(());
        } else if o == 0x6F && size == 1 && val & 0x20 != 0 {
            return Err(Error::SoftwareReset);
        } else if o == 0x6F && size == 1 && val & 0x80 != 0 {
            self.regs[o] = val as u8;
            if val & 0x01 == 0 {
                return self.enter_stall(val);
            }
            return self.enter_suspend(val);
        }
        if o == 0x74B && size == 1 && val & 0x08 != 0 {
            let w = le32(&self.regs, 0x74C);
            self.k32_wake = Some(w);
        }
        if (o == 0x66 || o == 0x70) && size == 1 {
            self.regs[o] = val as u8;
            let hz = match self.sysclk_hook.clone() {
                Some(h) => h(self),
                None => self.sysclk(),
            };
            self.set_cpu_hz(hz);
            return Ok(());
        } else if o == 0x0D && size == 1 {
            self.flash_select(val & 0x01 == 0)?;
            self.mspi_auto = val & 0x08 != 0;
            self.regs[0x0D] = val as u8;
            return Ok(());
        } else if o == 0x0C && size == 1 {
            let rx = self.flash_clock(val as u8)?;
            self.flash.rx = rx;
            return Ok(());
        } else if o == 0xBA && size == 1 {
            if val & 0x40 != 0 {
                let addr = self.regs[0xB8] as usize;
                if val & 0x20 != 0 {
                    let old = self.analog[addr];
                    let b9 = self.regs[0xB9];
                    if addr == 0x44 {
                        self.analog[addr] = old & !b9;
                    } else {
                        self.analog[addr] = b9;
                    }
                    if addr == 0xC6 || addr == 0xC7 {
                        let done: u8 = if addr == 0xC7 { 0x80 } else { 0x40 };
                        if b9 & 1 != 0 {
                            self.analog[0xCF] |= done;
                        } else {
                            self.analog[0xCF] &= !done;
                        }
                    }
                    if (0x0E..=0x15).contains(&addr) {
                        self.hold_pads_internal((addr - 0x0E) / 2);
                        self.spi_cs_update();
                    }
                    if self.log_analog {
                        let who = self.here();
                        let b9 = self.regs[0xB9];
                        self.analog_writes.push(AnalogWrite { addr: addr as u8, val: b9, who });
                        self.analog_log.push(AnalogAccess { kind: b'w', addr: addr as u8, val: b9 });
                    }
                    if addr == 0x0B && (old ^ self.regs[0xB9]) & 0x80 != 0 && self.has_event() {
                        let text = format!("analog 0x0b bit 7 (USB DP pull-up) -> {}", self.regs[0xB9] >> 7);
                        self.event(&text);
                    }
                    if addr == 0xC0 {
                        self.gpio_irq_update();
                    }
                } else {
                    self.regs[0xB9] = self.analog[addr];
                    if addr == 0x88 {
                        if self.xtal_ready {
                            self.regs[0xB9] |= 0x80;
                        } else {
                            self.regs[0xB9] &= 0x7F;
                        }
                    }
                    if (addr == 0xF7 || addr == 0xF8) && self.analog[0xFC] & 0x20 == 0 {
                        let code = self.adc_polled_code();
                        if addr == 0xF7 {
                            self.regs[0xB9] = code as u8;
                        } else {
                            self.regs[0xB9] = 0x80 | ((code >> 8) & 0x7F) as u8;
                        }
                    }
                    if self.log_analog {
                        let b9 = self.regs[0xB9];
                        self.analog_log.push(AnalogAccess { kind: b'r', addr: addr as u8, val: b9 });
                    }
                }
            }
            self.regs[0xBA] = val as u8;
            return Ok(());
        }
        self.store(o, size, val);
        if o < 0x5A0 && o + size > 0x580 {
            let mut lo = (o as i64 - 0x580) / 8;
            let mut hi = (o as i64 + size as i64 - 1 - 0x580) / 8;
            if lo < 0 {
                lo = 0;
            }
            if hi > 3 {
                hi = 3;
            }
            let mut port = lo;
            while port <= hi {
                self.hold_pads_internal(port as usize);
                port += 1;
            }
        }
        if (o < 0x5A0 && o + size > 0x580) || (o <= 0x5B5 && 0x5B5 < o + size) {
            self.gpio_irq_update();
        }
        if self.spi_dev.is_some() && o < 0x5B8 && o + size > 0x580 {
            self.spi_cs_update();
        }
        Ok(())
    }

    fn hold_pads_internal(&mut self, port: usize) {
        if self.pad_c_pf != 0 {
            for p in 0..4 {
                self.pad_inputs(p);
            }
            return;
        }
        self.pad_inputs(port);
    }

    pub(crate) fn pad_inputs(&mut self, port: usize) -> u8 {
        let (lvl, flt) = self.pad_levels(port);
        let mut v = (lvl & !flt) | (self.pad_hold[port] & flt);
        if self.pad_c_pf != 0 {
            let b = 0x580 + 8 * port;
            for bit in 0..8usize {
                let key = (port, bit);
                let mask = 1u8 << bit;
                let pull = (self.analog[0x0E + 2 * port + (bit >> 2)] >> (2 * (bit & 3))) & 3;
                let own = (self.regs[b + 6] >> bit) & 1 != 0 && (self.regs[b + 2] >> bit) & 1 == 0;
                let rest: u8 = if pull == 1 || pull == 3 { mask } else { 0 };
                if flt & mask != 0 || own || pull == 0 || v & mask != rest || self.pad_hold[port] & mask == rest {
                    self.pad_rc_since.remove(&key);
                    continue;
                }
                let since = match self.pad_rc_since.get(&key) {
                    Some(&s) => s,
                    None => {
                        let s = self.cycles;
                        self.pad_rc_since.insert(key, s);
                        s
                    }
                };
                if self.cycles - since >= self.pad_rc_cycles(pull) {
                    self.pad_rc_since.remove(&key);
                } else {
                    v = (v & !mask) | (self.pad_hold[port] & mask);
                }
            }
        }
        self.pad_hold[port] = v;
        v
    }

    pub(crate) fn pad_rc_cycles(&self, pull: u8) -> i64 {
        let need_ns = 1204i64
            .wrapping_mul(PAD_R_OHM[pull as usize])
            .wrapping_mul(self.pad_c_pf)
            .wrapping_mul(self.pad_r_pct)
            / 100
            / 1_000_000;
        (need_ns.wrapping_mul(self.cpu_hz) + 999_999_999) / 1_000_000_000
    }

    pub(crate) fn pad_rc_service(&mut self) {
        let before = self.pad_hold;
        for port in 0..4 {
            self.pad_inputs(port);
        }
        if self.pad_hold != before {
            self.gpio_irq_update();
        }
    }

    pub(crate) fn pad_rc_due(&self) -> Option<i64> {
        let mut due = 0i64;
        let mut found = false;
        for (&(port, bit), &since) in self.pad_rc_since.iter() {
            let pull = (self.analog[0x0E + 2 * port + (bit >> 2)] >> (2 * (bit & 3))) & 3;
            let mut left = 0i64;
            if pull != 0 {
                left = since + self.pad_rc_cycles(pull) - self.cycles;
            }
            if !found || left < due {
                due = left;
                found = true;
            }
        }
        if found {
            Some(due)
        } else {
            None
        }
    }

    pub fn pad_rc_pending(&self) -> bool {
        !self.pad_rc_since.is_empty()
    }

    pub fn hold_pads(&mut self, port: usize) {
        self.hold_pads_internal(port);
    }

    pub fn gpio_irq_update(&mut self) {
        let mut req = 0u8;
        if self.regs[0x5B5] & 0x08 != 0 {
            for p in 0..4 {
                let b = 0x580 + 8 * p;
                let inp = self.reg_read(b, 1).unwrap_or(0);
                req |= ((inp as u8) ^ self.regs[b + 4]) & self.regs[b + 7];
            }
        }
        if req != 0 && !self.gpio_req {
            self.irq_latch |= 1 << 18;
        }
        self.gpio_req = req != 0;
    }

    pub(crate) fn feed(&mut self) {
        if let Some(h) = self.on_feed.clone() {
            h(self);
        }
        self.tick2_base = self.cycles;
        self.wd_fed = self.cycles;
    }

    pub fn irq_sources(&mut self) -> u32 {
        let mut level = 0u32;
        let n = self.level_sources.len();
        for i in 0..n {
            if i >= self.level_sources.len() {
                break;
            }
            let f = self.level_sources[i].clone();
            level |= f(self);
        }
        self.irq_latch | (level & 0xFFFF)
    }

    pub fn irq_pending(&mut self) -> bool {
        if self.regs[0x643] & 1 == 0 || self.ibit != 0 {
            return false;
        }
        let mask = self.regs[0x640] as u32 | (self.regs[0x641] as u32) << 8 | (self.regs[0x642] as u32) << 16;
        self.irq_sources() & mask != 0
    }

    pub fn take_irq(&mut self) {
        let ret = self.r[15];
        let cpsr = self.cpsr();
        let _ = self.set_cpsr((cpsr & !0x9F) | 0x80 | MODE_IRQ);
        self.spsr[0] = cpsr;
        self.r[14] = ret;
        self.r[15] = 0x10;
        if let Some(h) = self.on_irq.clone() {
            h(self);
        }
    }

    #[inline]
    pub fn cpsr(&self) -> u32 {
        self.n << 31 | self.z << 30 | self.c << 29 | self.v << 28 | self.ibit << 7 | self.mode
    }

    pub fn set_cpsr(&mut self, val: u32) -> Result<(), Error> {
        let new_mode = val & 0x1F;
        let ni = match bank_index(new_mode) {
            Some(i) => i,
            None => return Err(emu_err(format!("mode 0x{:x} at {}", new_mode, self.here()))),
        };
        if new_mode != self.mode {
            if let Some(oi) = bank_index(self.mode) {
                self.bank[oi] = [self.r[13], self.r[14]];
            }
            let b = self.bank[ni];
            self.r[13] = b[0];
            self.r[14] = b[1];
            self.mode = new_mode;
        }
        self.n = (val >> 31) & 1;
        self.z = (val >> 30) & 1;
        self.c = (val >> 29) & 1;
        self.v = (val >> 28) & 1;
        self.ibit = (val >> 7) & 1;
        Ok(())
    }

    #[inline]
    pub(crate) fn spsr_cur(&self) -> u32 {
        match bank_index(self.mode) {
            Some(i) => self.spsr[i],
            None => 0,
        }
    }

    #[inline]
    pub(crate) fn set_spsr_cur(&mut self, v: u32) {
        if let Some(i) = bank_index(self.mode) {
            self.spsr[i] = v;
        }
    }

    pub(crate) fn enter_suspend(&mut self, val: u32) -> Result<(), Error> {
        let v = self.stimer_raw();
        self.asleep = Some(v);
        self.sleep_since = self.ms();
        self.sleep_deep = self.analog[0x7E] != 0;
        if self.has_event() {
            let kind = if self.sleep_deep {
                format!("deep sleep (0x7e=0x{:02x})", self.analog[0x7E])
            } else {
                "suspend".to_string()
            };
            let text = format!("{} (0x6f = 0x{:02x}, wake sources analog 0x26 = 0x{:02x})", kind, val, self.analog[0x26]);
            self.event(&text);
        }
        Ok(())
    }

    fn enter_stall(&mut self, val: u32) -> Result<(), Error> {
        self.stalled = true;
        self.stall_since = self.cycles;
        if self.has_event() {
            let text = format!("stall (0x6f = 0x{:02x}, wake mask 0x78 = 0x{:08x})", val, le32(&self.regs, 0x78));
            self.event(&text);
        }
        Ok(())
    }

    fn stall_wake_sources(&mut self) -> u32 {
        let mask = le32(&self.regs, 0x78);
        if mask == 0 {
            return 0;
        }
        self.irq_sources() & mask
    }

    fn stall_skip(&mut self, left: i64) -> i64 {
        let mut skip = left.min(self.cpu_hz / 1000);
        for n in 0..2 {
            if !self.tmr_on[n] {
                continue;
            }
            let cap = le32(&self.regs, 0x624 + 4 * n) as i64;
            if cap == 0 {
                continue;
            }
            let mut d = self.tmr_base[n] + cap - self.cycles;
            if self.tmr_wrap[n] {
                d += 1 << 32;
            }
            skip = skip.min(d);
        }
        if let Some(d) = self.cycles_to_tmr2() {
            skip = skip.min(d);
        }
        if self.stimer_on && self.regs[0x748] & STIMER_IRQ != 0 {
            let d = self.cycles_to_stimer(self.stimer_cmp);
            if d > 0 {
                skip = skip.min(d);
            }
        }
        let ctrl = le32(&self.regs, 0x620);
        if ctrl & (1 << 23) != 0 && ctrl & (1 << 6) != 0 {
            let capture = ((ctrl >> 9) & 0x3FFF) as i64;
            if capture != 0 {
                skip = skip.min((capture << 18) - self.tick2_now() as i64);
            }
        }
        if let Some(h) = self.idle_skip_hook.clone() {
            skip = h(self, skip);
        }
        if let Some(due) = self.pad_rc_due() {
            skip = skip.min(due + 1);
        }
        skip.max(1)
    }

    pub fn stall_until(&mut self, max_cycles: i64) -> Result<(), Error> {
        loop {
            self.update_time()?;
            if !self.pad_rc_since.is_empty() {
                self.pad_rc_service();
            }
            let src = self.stall_wake_sources();
            if src != 0 {
                self.stalled = false;
                if self.has_event() {
                    let us = (self.cycles - self.stall_since) as f64 * 1e6 / self.cpu_hz as f64;
                    let text = format!("stall wake: sources 0x{:08x} after {:.1} us", src, us);
                    self.event(&text);
                }
                return Ok(());
            }
            if self.cycles >= max_cycles {
                return Ok(());
            }
            let skip = self.stall_skip(max_cycles - self.cycles);
            self.cycles += skip;
            self.stall_cycles += skip;
        }
    }

    pub fn pad_levels(&mut self, port: usize) -> (u8, u8) {
        let (lvl, flt) = self.pad_levels_base(port);
        if let Some(h) = self.pad_levels_hook.clone() {
            return h(self, port, lvl, flt);
        }
        (lvl, flt)
    }

    fn pad_levels_base(&self, port: usize) -> (u8, u8) {
        let b = 0x580 + 8 * port;
        let own = self.regs[b + 6] & !self.regs[b + 2];
        let mut lvl = self.regs[b + 3] & own;
        let mut rest = !own;
        let pwm_pin = match port {
            2 => Some(1usize),
            1 => Some(5usize),
            _ => None,
        };
        if let Some(bit) = pwm_pin {
            if rest & (1 << bit) != 0 {
                if let Some((ch, true)) = self.pwm_channel(port, bit) {
                    rest &= !(1 << bit);
                    if self.pwm_level(ch) {
                        lvl |= 1 << bit;
                    }
                }
            }
        }
        let pulls = self.analog[0x0E + 2 * port] as u32 | (self.analog[0x0F + 2 * port] as u32) << 8;
        let (mut up, mut none) = (0u8, 0u8);
        for bit in 0..8 {
            let pull = (pulls >> (2 * bit)) & 3;
            up |= ((pull & 1) as u8) << bit;
            none |= ((pull == 0) as u8) << bit;
        }
        (lvl | (up & rest), none & rest)
    }

    fn pwm_channel(&self, port: usize, pin: usize) -> Option<(usize, bool)> {
        if (self.regs[0x586 + 8 * port] >> pin) & 1 != 0 {
            return None;
        }
        let mux = ((self.regs[0x5A8 + 2 * port + (pin >> 2)] >> (2 * (pin & 3))) & 3) as usize;
        let ch = if port == 2 && pin == 1 && mux == 2 {
            0
        } else if port == 1 && pin == 5 && mux == 1 {
            5
        } else {
            return None;
        };
        Some((ch, self.pwm_enabled(ch)))
    }

    pub(crate) fn pwm_enabled(&self, ch: usize) -> bool {
        if ch == 0 {
            return self.regs[0x781] & 0x01 != 0;
        }
        self.regs[0x780] & (1 << ch) != 0
    }

    pub(crate) fn pwm_count(&self, ch: usize) -> u32 {
        let max = le16(&self.regs, 0x796 + 4 * ch) as u32;
        if max == 0 {
            return 0;
        }
        let div = self.regs[0x782] as i64 + 1;
        ((self.cycles / div) % max as i64) as u32
    }

    fn pwm_level(&self, ch: usize) -> bool {
        let cmp = le16(&self.regs, 0x794 + 4 * ch) as u32;
        let mut high = self.pwm_count(ch) < cmp;
        if self.regs[0x784] & (1 << ch) != 0 {
            high = !high;
        }
        high
    }

    pub(crate) fn pwm_frames(&self) -> i64 {
        let max = le16(&self.regs, 0x796) as i64;
        if max == 0 || !self.pwm_enabled(0) {
            return 0;
        }
        let div = self.regs[0x782] as i64 + 1;
        (self.cycles / div) / max
    }

    fn wake_status(&mut self) -> u8 {
        let src = self.analog[0x26];
        let mut st = 0u8;
        if src & 0x08 != 0 {
            for port in 0..4 {
                let en = self.analog[0x27 + port];
                if en == 0 {
                    continue;
                }
                let (lvl, flt) = self.pad_levels(port);
                if en & flt != 0 {
                    self.float_wakes.insert((port as i64, (en & flt) as i64), true);
                }
                let pol = self.analog[0x21 + port];
                if en & !flt & ((!lvl & pol) | (lvl & !pol)) != 0 {
                    st |= 0x08;
                }
            }
        }
        if src & 0x10 != 0 {
            let n = self.core_wake_sources.len();
            for i in 0..n {
                if i >= self.core_wake_sources.len() {
                    break;
                }
                let f = self.core_wake_sources[i].clone();
                if f(self) {
                    st |= 0x04;
                    break;
                }
            }
        }
        if src & 0x20 != 0 {
            if let Some(w) = self.k32_wake {
                if self.k32_now().wrapping_sub(w) < 0x80000000 {
                    st |= 0x02;
                }
            }
        }
        if src & 0x80 != 0 && self.lpc_fired {
            st |= 0x01;
        }
        st
    }

    pub fn sleep_until(&mut self, max_cycles: i64) {
        let st = self.wake_status();
        if st != 0 {
            self.analog[0x44] |= st;
            self.asleep = None;
            let ms = self.ms();
            self.wakes.push(Wake { ms, status: st });
            if self.has_event() {
                let text = format!("wake: analog 0x44 = 0x{:02x} after {:.1} ms", st, self.ms() - self.sleep_since);
                self.event(&text);
            }
            if let Some(h) = self.on_wake.clone() {
                h(self, st);
            }
            if self.sleep_deep {
                for i in 0x8000..self.sram.len() {
                    self.sram[i] = 0;
                }
                self.sleep_deep = false;
                let _ = self.reset(-1, false);
                self.analog[0x44] |= st;
            }
            return;
        }
        let mut chunk = max_cycles - self.cycles;
        let c = self.cpu_hz / 1000;
        if c < chunk {
            chunk = c;
        }
        if chunk < 1 {
            chunk = 1;
        }
        if self.analog[0x26] & 0x20 != 0 {
            if let Some(w) = self.k32_wake {
                let d = w.wrapping_sub(self.k32_now());
                if d < 0x80000000 {
                    let now = self.ms();
                    let need = ((((now * 32.768) as i64 + d as i64) as f64 / 32.768 - now) * self.cpu_hz as f64 / 1000.0)
                        .ceil() as i64;
                    if need < chunk {
                        chunk = need;
                    }
                    if chunk < 1 {
                        chunk = 1;
                    }
                }
            }
        }
        self.cycles += chunk;
        self.tick_base_cy += chunk;
        self.tick2_base += chunk;
        self.wd_fed += chunk;
        for n in 0..2 {
            if self.tmr_on[n] {
                self.tmr_base[n] += chunk;
            }
        }
    }
}
