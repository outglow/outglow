use crate::cell::Shared;
use core::cell::Cell;

use bulb_core::hal::{Clock, ConfigStore, Pwm};
use bulb_core::store::LightLog;
use zigbee::hal::ResetCause;

use crate::flash;
use crate::reg;

const TICKS_PER_MS: u32 = 16_000;

#[derive(Clone, Copy)]
pub struct Out {
    pub pwm: u8,
    pub gpio: usize,
    pub bit: u8,
    pub mux: usize,
    pub shift: u8,
    pub value: u8,
}

#[derive(Clone, Copy)]
pub struct Board {
    pub name: &'static str,
    pub tag: &'static str,
    pub model: &'static str,
    pub ble_product: u16,
    pub hw_revision: &'static str,
    pub hw_version: u8,
    pub warm: Out,
    pub cold: Option<Out>,
    pub rgb: Option<[Out; 3]>,
    pub rgb_beside_white: bool,
    pub colour_default: (u8, u8),
    pub kind: &'static str,
    pub period: u16,
    pub ct_min: u16,
    pub ct_max: u16,
    pub image_type: u16,
}

pub const BOARD: Board = board_named(match option_env!("BULB_BOARD") {
    Some(name) => name,
    None => "ESMLFzm_w6_Dimm-4.63",
});

#[used]
static BOARD_TAG: &str = BOARD.tag;

const fn board_named(name: &str) -> Board {
    let boards = &crate::boards::BOARDS;
    let mut i = 0;
    while i < boards.len() {
        if str_eq(boards[i].name, name) {
            return boards[i];
        }
        i += 1;
    }
    panic!("BULB_BOARD names no board of boards.rs")
}

const fn str_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

pub fn init() {
    reg::write8(reg::IRQ_ENABLE, 0);
    reg::write32(reg::IRQ_MASK, 0);
    reg::write8(0x60, 0x00);
    reg::write8(0x61, 0x00);
    reg::write8(0x62, 0x00);
    reg::write8(0x63, 0xff);
    reg::write8(0x64, 0xff);
    reg::write8(0x65, 0xff);
    reg::write8(reg::CLK_SEL, 0x06);
    analog_write(0x82, 0x64);
    analog_write(0x34, 0x80);
    reg::analog_modify(0x0b, 0x80, 0x38);
    analog_write(0x8c, 0x02);
    analog_write(0x02, 0xa2);
    for addr in 0x27..=0x2a {
        analog_write(addr, 0x00);
    }
    reg::write32(0xc40, 0x0404_0404);
    reg::write32(0xc44, 0x0404_0404);
    reg::write8(0xc48, 0x04);
    reg::write16(0x750, 0);
    analog_write(0x01, if reg::read8(0x7d) == 1 { 0x3c } else { 0x4c });
    if analog_read(0x7f) & 0x01 != 0 {
        let status = analog_read(0x3c);
        if status & 0x02 != 0 {
            analog_write(0x3c, status & !0x02);
        }
    }
    reg::write8(reg::SYSTEM_TICK_CTRL, 0x01);
    wait_for_crystal();
    calibrate_rc_24m();
    calibrate_doubler();
    reg::write8(0xc20, 0x00);
    reg::write8(0xc21, 0x00);
    reg::write8(reg::CLK_SEL, reg::CLK_SEL_48M_CRYSTAL);
    analog_write(0x0c, 0xc6);
}

fn wait_for_crystal() {
    for _ in 0..10 {
        let t0 = reg::tick();
        let mut spins = 0u32;
        while spins < 2000 {
            if reg::tick().wrapping_sub(t0) > 320 {
                return;
            }
            spins += 1;
        }
    }
    reg::write8(reg::PWDN_CTRL, 0x20);
    loop {}
}

fn calibrate_rc_24m() {
    analog_write(0xc8, 0x80);
    reg::analog_modify(0x30, 0xff, 0x80);
    analog_write(0xc7, 0x0e);
    analog_write(0xc7, 0x0f);
    while analog_read(0xcf) & 0x80 == 0 {}
    analog_write(0x33, analog_read(0xcb));
    analog_write(0x30, analog_read(0x30) & 0x7f);
    analog_write(0xc7, 0x0e);
}

fn calibrate_doubler() {
    analog_write(0x86, 0xbb);
    analog_write(0x82, analog_read(0x82) & 0x7f);
    reg::analog_modify(0x87, !0x03, 0x02);
    reg::analog_modify(0x87, 0xff, 0x04);
    reg::analog_modify(0x87, 0x07, (analog_read(0x88) & 0x1f) << 3);
    reg::analog_modify(0x82, 0xff, 0x80);
    analog_write(0x87, analog_read(0x87) & 0xfd);
    analog_write(0x86, 0xfb);
}

const RETAINED: u8 = 0x3a;
const RETAINED_ALIVE: u8 = 0x01;
const WD_STATUS: usize = 0x72;
const WATCHDOG_ENABLE: u32 = 0x0080_0040;

static CAUSE: Shared<ResetCause> = Shared::new(ResetCause::PowerOn);

pub fn warm_reset() -> bool {
    let v = analog_read(RETAINED);
    let wd = reg::read8(WD_STATUS) & 0x01 != 0;
    if wd {
        reg::write8(WD_STATUS, 0x01);
    }
    let cause = if v & RETAINED_ALIVE == 0 {
        ResetCause::PowerOn
    } else if wd {
        ResetCause::Watchdog
    } else {
        ResetCause::Software
    };
    analog_write(RETAINED, v | RETAINED_ALIVE);
    CAUSE.set(cause);
    cause != ResetCause::PowerOn
}

pub fn reset_cause() -> ResetCause {
    CAUSE.get()
}

pub fn watchdog_start(ms: u32) {
    let capture = (ms * 48_000) >> 18;
    reg::write32(reg::TIMER2_TICK, 0);
    reg::modify32(reg::TIMER_CTRL, 0x0080_01ff, ((capture & 0x3fff) << 9) + WATCHDOG_ENABLE);
    watchdog_feed();
}

#[inline(always)]
pub fn watchdog_feed() {
    reg::write8(reg::TIMER_STATUS, 0x08);
}

pub fn reboot() -> ! {
    reg::write8(reg::PWDN_CTRL, 0x20);
    loop {}
}

pub struct FlashWords;

const RESET_SECTOR: usize = 0xc_f000;

impl bulb_core::reset::WordStore for FlashWords {
    fn slots(&self) -> usize {
        SECTOR_SIZE / 2
    }
    fn read(&self, slot: usize) -> u16 {
        let mut b = [0u8; 2];
        flash::read(RESET_SECTOR + slot * 2, &mut b);
        u16::from_le_bytes(b)
    }
    fn write(&mut self, slot: usize, word: u16) {
        flash::write(RESET_SECTOR + slot * 2, &word.to_le_bytes());
        flash::stored();
    }
    fn erase(&mut self) {
        flash::erase_sector(RESET_SECTOR);
    }
}

fn analog_write(addr: u8, value: u8) {
    reg::analog_write(addr, value);
}

fn analog_read(addr: u8) -> u8 {
    reg::analog_read(addr)
}

pub struct LedPwm;

impl LedPwm {
    pub fn new() -> Self {
        reg::write8(reg::PWM_CLK_DIV, 0);
        start(BOARD.warm);
        if let Some(cold) = BOARD.cold {
            start(cold);
        }
        if let Some(rgb) = BOARD.rgb {
            for o in rgb {
                start(o);
            }
        }
        LedPwm
    }
}

fn start(o: Out) {
    reg::write16(reg::pwm_max(o.pwm), BOARD.period);
    reg::write16(reg::pwm_cmp(o.pwm), 0);
    reg::modify8(o.mux, !(3 << o.shift), o.value << o.shift);
    reg::clear8(o.gpio, o.bit);
    match o.pwm {
        0 => reg::set8(reg::PWM0_ENABLE, 0x01),
        n => reg::set8(reg::PWM_ENABLE, 1 << n),
    }
}

impl Pwm for LedPwm {
    fn max_duty(&self) -> u16 {
        BOARD.period
    }

    fn set_duty(&mut self, duty: u16) {
        reg::write16(reg::pwm_cmp(BOARD.warm.pwm), duty.min(BOARD.period));
    }

    fn has_cold_channel(&self) -> bool {
        BOARD.cold.is_some()
    }

    fn set_cold_duty(&mut self, duty: u16) {
        if let Some(cold) = BOARD.cold {
            reg::write16(reg::pwm_cmp(cold.pwm), duty.min(BOARD.period));
        }
    }

    fn ct_range(&self) -> (u16, u16) {
        (BOARD.ct_min, BOARD.ct_max)
    }

    fn has_rgb(&self) -> bool {
        BOARD.rgb.is_some()
    }

    fn set_rgb(&mut self, r: u16, g: u16, b: u16) {
        if let Some([or, og, ob]) = BOARD.rgb {
            reg::write16(reg::pwm_cmp(or.pwm), r);
            reg::write16(reg::pwm_cmp(og.pwm), g);
            reg::write16(reg::pwm_cmp(ob.pwm), b);
        }
    }

    fn white_tw_curve(&self) -> bool {
        BOARD.cold.is_some() || BOARD.rgb.is_some()
    }

    fn rgb_beside_white(&self) -> bool {
        BOARD.rgb_beside_white
    }

    fn colour_default(&self) -> (u8, u8) {
        BOARD.colour_default
    }
}

pub struct SystemClock {
    last_tick: Cell<u32>,
    rest: Cell<u32>,
    ms: Cell<u64>,
}

impl SystemClock {
    pub fn new() -> Self {
        SystemClock { last_tick: Cell::new(reg::read32(reg::SYSTEM_TICK)), rest: Cell::new(0), ms: Cell::new(0) }
    }
}

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        let tick = reg::read32(reg::SYSTEM_TICK);
        let ticks = self.rest.get() + tick.wrapping_sub(self.last_tick.get());
        self.last_tick.set(tick);
        self.rest.set(ticks % TICKS_PER_MS);
        self.ms.set(self.ms.get() + (ticks / TICKS_PER_MS) as u64);
        self.ms.get()
    }
}

pub struct FlashStore(LightLog<BoardFlash>, Option<LightLog<BoardFlash>>);

pub const COLOUR_SECTORS: [usize; 2] = [0xf_5000, 0xf_6000];

fn is_colour(r: &[u8]) -> bool {
    bulb_core::colour::ColourRecord::decode(r).is_some()
}

pub const CONFIG_SECTORS: [usize; 2] = [0xf_0000, 0xf_4000];
const SECTOR_SIZE: usize = 0x1000;

pub struct BoardFlash;

impl bulb_core::store::Flash for BoardFlash {
    fn read(&self, addr: usize, buf: &mut [u8]) {
        flash::read(addr, buf);
    }
    fn program(&mut self, addr: usize, data: &[u8]) {
        flash::write(addr, data);
    }
    fn erase(&mut self, addr: usize) {
        flash::erase_sector(addr);
    }
}

impl FlashStore {
    pub fn erase() {
        let mut s = Self::new();
        s.0.erase();
        if let Some(c) = s.1.as_mut() {
            c.erase();
        }
    }

    pub fn new() -> Self {
        let colour = BOARD.rgb.map(|_| LightLog::of_records(BoardFlash, COLOUR_SECTORS, SECTOR_SIZE, is_colour));
        FlashStore(LightLog::new(BoardFlash, CONFIG_SECTORS, SECTOR_SIZE), colour)
    }
}

impl ConfigStore for FlashStore {
    fn load(&self, buf: &mut [u8]) -> Option<usize> {
        self.0.load(buf)
    }

    fn store(&mut self, data: &[u8]) {
        ConfigStore::store(&mut self.0, data);
        flash::stored();
    }

    fn load_colour(&self, buf: &mut [u8]) -> Option<usize> {
        self.1.as_ref()?.load(buf)
    }

    fn store_colour(&mut self, data: &[u8]) {
        if let Some(c) = self.1.as_mut() {
            let mut now = [0u8; bulb_core::colour::RECORD_LEN];
            if c.load(&mut now).is_some() && now[..] == *data {
                return;
            }
            ConfigStore::store(c, data);
            flash::stored();
        }
    }
}

#[cfg(test)]
#[path = "tests/board.rs"]
mod tests;
