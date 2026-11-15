//! The bulb's hardware behind `bulb-core`'s HAL traits: clock, LED PWM,
//! system timer and the configuration record in flash.
//!
//! Pins, PWM channels, PWM period and colour temperature range come from the
//! board the image is built for (`BOARD`, from `boards.rs`): what the stock
//! firmware of that product and hardware revision does (docs/stock-profiles.md).

use crate::cell::Shared;
use core::cell::Cell;

use bulb_core::hal::{Clock, ConfigStore, Pwm};
use bulb_core::store::LightLog;
use zigbee::hal::ResetCause;

use crate::flash;
use crate::reg;

const TICKS_PER_MS: u32 = 16_000;

/// A PWM output of the LED driver: the channel, and the pin that carries it
/// (its bit in the port's GPIO function register, cleared for the
/// multiplexed function, and its multiplexer field).
#[derive(Clone, Copy)]
pub struct Out {
    pub pwm: u8,
    pub gpio: usize,
    pub bit: u8,
    pub mux: usize,
    pub shift: u8,
    pub value: u8,
}

/// One product and hardware revision of the range (`boards.rs`).
#[derive(Clone, Copy)]
pub struct Board {
    /// `model-revision`, as `cargo xtask build --board` names it.
    pub name: &'static str,
    /// `outglow-board:` and the name, kept in the image (`BOARD_TAG`).
    pub tag: &'static str,
    /// modelId (Zigbee Basic, BLE Device Information).
    pub model: &'static str,
    /// AwoX product number in the BLE advertising data.
    pub ble_product: u16,
    /// Hardware revision: BLE Device Information ("4.63") and Zigbee Basic HWVersion (63).
    pub hw_revision: &'static str,
    pub hw_version: u8,
    /// The only channel of a dimmable bulb, the warm one of a tunable-white bulb.
    pub warm: Out,
    pub cold: Option<Out>,
    /// A colour bulb's red, green and blue channels.
    pub rgb: Option<[Out; 3]>,
    /// The colour channels are a light of their own beside the white one
    /// (EBF: a white ring and a colour strip, `bulb_core::hal::Pwm::rgb_beside_white`).
    pub rgb_beside_white: bool,
    /// Hue and saturation of a factory-new colour bulb.
    pub colour_default: (u8, u8),
    /// The Zigbee model id: ours, one per kind of light (`outglow-dimm`, `-tw`,
    /// `-rgbtw`, `-rgbw`, `-ebf`).
    pub kind: &'static str,
    /// PWM period in PWM clock ticks (the clock undivided): 48000, or 3000 on revision 4.69.
    pub period: u16,
    /// Colour temperature range in mireds: all cold at `ct_min`, all warm at `ct_max`.
    pub ct_min: u16,
    pub ct_max: u16,
    /// Zigbee OTA image type of this board's images: its own, so that no server
    /// hands it an image for another one.
    pub image_type: u16,
}

/// The board of this build: `BULB_BOARD` (`cargo xtask build` sets it), the dimmable
/// filament bulb without it.
pub const BOARD: Board = board_named(match option_env!("BULB_BOARD") {
    Some(name) => name,
    None => "ESMLFzm_w6_Dimm-4.63",
});

/// What the image says it was built for, for tools that read the image
/// (`outglow ble push` checks it against the bulb's own hardware revision).
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

/// Chip setup that has to precede everything else: clocks, the analog
/// supplies and oscillators, the system timer. The writes and their order
/// are those of Telink's start-up for this chip (docs/rust-radio.md); what
/// only serves the sleep modes is left out.
pub fn init() {
    reg::write8(reg::IRQ_ENABLE, 0);
    reg::write32(reg::IRQ_MASK, 0);
    // every block out of reset, every clock gate open
    reg::write8(0x60, 0x00);
    reg::write8(0x61, 0x00);
    reg::write8(0x62, 0x00);
    reg::write8(0x63, 0xff);
    reg::write8(0x64, 0xff);
    reg::write8(0x65, 0xff);
    reg::write8(reg::CLK_SEL, 0x06);
    analog_write(0x82, 0x64);
    analog_write(0x34, 0x80);
    analog_write(0x0b, (analog_read(0x0b) & 0x80) | 0x38);
    analog_write(0x8c, 0x02);
    analog_write(0x02, 0xa2);
    for addr in 0x27..=0x2a {
        analog_write(addr, 0x00);
    }
    // DMA buffers are in SRAM: high address byte of every channel
    reg::write32(0xc40, 0x0404_0404);
    reg::write32(0xc44, 0x0404_0404);
    reg::write8(0xc48, 0x04);
    reg::write16(0x750, 0);
    // depends on the chip revision
    analog_write(0x01, if reg::read8(0x7d) == 1 { 0x3c } else { 0x4c });
    // the "woke from sleep" status flag of a previous run
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
    // 48 MHz system clock from the crystal doubler; for it the flash supply is raised
    reg::write8(reg::CLK_SEL, reg::CLK_SEL_48M_CRYSTAL);
    analog_write(0x0c, 0xc6);
}

/// The system timer runs off the crystal: once it counts, the crystal is
/// stable. Reboots if it never does.
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

/// Calibrates the 24 MHz RC oscillator against the crystal.
fn calibrate_rc_24m() {
    analog_write(0xc8, 0x80);
    analog_write(0x30, analog_read(0x30) | 0x80);
    analog_write(0xc7, 0x0e);
    analog_write(0xc7, 0x0f);
    while analog_read(0xcf) & 0x80 == 0 {}
    analog_write(0x33, analog_read(0xcb));
    analog_write(0x30, analog_read(0x30) & 0x7f);
    analog_write(0xc7, 0x0e);
}

/// Calibrates the doubler that makes 48 MHz from the 24 MHz crystal.
fn calibrate_doubler() {
    analog_write(0x86, 0xbb);
    analog_write(0x82, analog_read(0x82) & 0x7f);
    analog_write(0x87, (analog_read(0x87) & !0x03) | 0x02);
    analog_write(0x87, analog_read(0x87) | 0x04);
    analog_write(0x87, ((analog_read(0x88) & 0x1f) << 3) | (analog_read(0x87) & 0x07));
    analog_write(0x82, analog_read(0x82) | 0x80);
    analog_write(0x87, analog_read(0x87) & 0xfd);
    analog_write(0x86, 0xfb);
}

/// Analog register that keeps its value across a watchdog or software
/// reset but not across a power loss; bit 0 marks "the chip has been running".
const RETAINED: u8 = 0x3a;
const RETAINED_ALIVE: u8 = 0x01;
/// Watchdog status: bit 0 set when the last reset was the watchdog's;
/// writing 1 clears it (the C firmware reads it the same way).
const WD_STATUS: usize = 0x72;

/// Why this start happened (`warm_reset` finds out).
static CAUSE: Shared<ResetCause> = Shared::new(ResetCause::PowerOn);

/// True when this start follows a watchdog or software reset rather than a
/// power-on. Call once, early.
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

/// Starts the watchdog: the chip resets unless `watchdog_feed` is called at
/// least every `ms` milliseconds.
pub fn watchdog_start(ms: u32) {
    // capture in units of 2^18 cycles of the 48 MHz system clock
    let capture = (ms * 48_000) >> 18;
    reg::write32(reg::TIMER2_TICK, 0);
    let ctrl = reg::read32(reg::TIMER_CTRL) & 0x00ff_ffff & !(0x3fff << 9);
    reg::write32(reg::TIMER_CTRL, ctrl | (capture & 0x3fff) << 9 | 1 << 6 | 1 << 23);
    watchdog_feed();
}

#[inline(always)]
pub fn watchdog_feed() {
    reg::write8(reg::TIMER_STATUS, 0x08);
}

/// Restarts the chip.
pub fn reboot() -> ! {
    reg::write8(reg::PWDN_CTRL, 0x20);
    loop {}
}

/// The power-on counter of the factory reset sequence: 16-bit words in the
/// sector the stock firmware keeps its own in, in the same format.
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

/// The LED driver's PWM inputs: the board's warm channel (the only one of a
/// dimmable bulb) and its cold channel.
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

/// A PWM channel at duty 0 on its pin: period, the pin's multiplexer
/// field, out of GPIO mode, the channel enabled.
fn start(o: Out) {
    reg::write16(reg::pwm_max(o.pwm), BOARD.period);
    reg::write16(reg::pwm_cmp(o.pwm), 0);
    let mux = reg::read8(o.mux);
    reg::write8(o.mux, (mux & !(3 << o.shift)) | o.value << o.shift);
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
        // above the period is the PWM held high, as the stock firmware drives it
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

/// Milliseconds since `SystemClock::new`, from the 16 MHz system timer.
///
/// The timer wraps every 268 s; `now_ms` must be called more often than that
/// (the main loop calls it on every pass).
pub struct SystemClock {
    last_tick: Cell<u32>,
    /// Ticks not yet counted as a whole millisecond.
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

/// The configuration record: `bulb_core::store::LightLog` in two 4 KB
/// sectors that take turns, so that a power cut at any moment leaves the last
/// record or the one before it (docs/rust-radio.md).
pub struct FlashStore(LightLog<BoardFlash>, Option<LightLog<BoardFlash>>);

/// The sectors of a colour bulb's colour (`bulb_core::colour::ColourRecord`),
/// a log of its own beside the configuration.
pub const COLOUR_SECTORS: [usize; 2] = [0xf_5000, 0xf_6000];

fn is_colour(r: &[u8]) -> bool {
    bulb_core::colour::ColourRecord::decode(r).is_some()
}

/// The sectors of the configuration log. They and the network store's two
/// sectors (and the mesh settings' one) lie above the range the stock
/// firmware keeps its settings in (0xCF000..0xEE000), so a bulb returned to
/// the stock firmware finds nothing of ours in its way. The firmware before
/// the second sector kept a single log in the first one; that log is read
/// until the first record is stored.
pub const CONFIG_SECTORS: [usize; 2] = [0xf_0000, 0xf_4000];
const SECTOR_SIZE: usize = 0x1000;

/// The chip's flash for the log.
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
    /// Forgets the stored configuration (factory reset). Call before `new`.
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
            // only a change: a colour picker sends the same colour again and again
            let mut now = [0u8; bulb_core::colour::RECORD_LEN];
            if c.load(&mut now).is_some() && now[..] == *data {
                return;
            }
            ConfigStore::store(c, data);
            flash::stored();
        }
    }
}
