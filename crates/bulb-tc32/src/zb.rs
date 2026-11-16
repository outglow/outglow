//! The Zigbee stack on this chip: `zigbee::Hal` on the radio, the system
//! timer and the flash, and the stack as `bulb-core`'s radio link.


use bulb_core::command::{Command, StateReport};
use bulb_core::hal::Radio;
use zigbee::hal::{HalCounters, NvItem, ResetCause};
use zigbee::stack::OtaState;
use zigbee::stack::Identity;
#[cfg(feature = "ble")]
use zigbee::stack::BleMode;
use zigbee::{Hal, RxMeta, Stack, TxStatus};

use crate::board::BOARD;
use crate::cell::{Shared, SharedBuf, TakeOnce};
use crate::flash;
use crate::link::DebugLink;
use crate::nv::Nv;
use crate::radio;
use crate::reg;
use crate::staging::{self, Owner};

/// The longest round of the main loop since the start, microseconds (`lib.rs`).
pub static LOOP_MAX_US: Shared<u32> = Shared::new(0);
/// The system timer tick at which BLE needs the radio next, as of the last
/// round (None: BLE off): the main loop's wait ends before it (`lib.rs`).
pub static BLE_NEXT: Shared<Option<u32>> = Shared::new(None);
/// The BLE stack's counters as of the last round (for the Diagnostics cluster).
#[cfg(feature = "ble")]
static BLE_STATS: Shared<ble::Stats> = Shared::new(ble::Stats { adv_events: 0, connections: 0, events: 0, events_heard: 0, events_skipped: 0, disconnects: 0, links_lost: 0 });

/// What the bulb says it is. Its own model identifiers, not the stock
/// bulb's ("ESMLFzm_w6_Dimm", "ESMLFzm_w6_TW"): a coordinator must not take
/// it for the stock firmware and use a device definition written for that
/// (Zigbee2MQTT builds one from what the bulb shows instead:
/// docs/diagnostics.md). One per kind of light, not per board; the hardware
/// version is the board's revision (63 for 4.63). Over BLE it is still the
/// stock bulb of its board (`ble.rs`), for the AwoX app.
const IDENTITY: Identity = Identity {
    manufacturer: "Outglow",
    model: BOARD.kind,
    sw_build: "outglow",
    manufacturer_code: 0x1141,
    app_version: 1,
    hw_version: BOARD.hw_version,
    color_temperature: BOARD.cold.is_some() || BOARD.rgb.is_some(),
    ct_min: BOARD.ct_min,
    ct_max: BOARD.ct_max,
    color: BOARD.rgb.is_some(),
    image_type: IMAGE_TYPE,
    file_version: FILE_VERSION,
    touchlink_master_key: MASTER_KEY,
    // these bulbs carry no install code
    install_code: None,
    ble: cfg!(feature = "ble"),
    #[cfg(feature = "sense")]
    sense: true,
};

/// The touchlink master key, if the build was given it: 32 hexadecimal
/// digits in the environment variable TOUCHLINK_MASTER_KEY. See
/// `docs/rust-radio.md`; the repository does not contain the key.
const MASTER_KEY: Option<[u8; 16]> = match option_env!("TOUCHLINK_MASTER_KEY") {
    Some(s) => Some(parse_key(s)),
    None => None,
};

const fn parse_key(s: &str) -> [u8; 16] {
    let b = s.as_bytes();
    assert!(b.len() == 32, "TOUCHLINK_MASTER_KEY: 32 hexadecimal digits");
    let mut key = [0u8; 16];
    let mut i = 0;
    while i < 32 {
        let d = match b[i] {
            c @ b'0'..=b'9' => c - b'0',
            c @ b'a'..=b'f' => c - b'a' + 10,
            c @ b'A'..=b'F' => c - b'A' + 10,
            _ => panic!("TOUCHLINK_MASTER_KEY: not a hexadecimal digit"),
        };
        key[i / 2] = key[i / 2] << 4 | d;
        i += 1;
    }
    key
}

/// OTA image type: this firmware's own, one per board, so that no server
/// hands it an image built for something else. `boot/start.S` puts the same
/// value into the image header (`cargo xtask build` takes it from the board table).
pub const IMAGE_TYPE: u16 = BOARD.image_type;

/// Version of this build as the OTA cluster sees it: hexadecimal in the
/// environment variable BULB_FILE_VERSION at build time (`cargo xtask build` sets it).
pub const FILE_VERSION: u32 = match option_env!("BULB_FILE_VERSION") {
    Some(s) => parse_hex(s),
    None => 0x0100_0000,
};

const fn parse_hex(s: &str) -> u32 {
    let b = s.as_bytes();
    let mut i = if b.len() > 2 && b[0] == b'0' && (b[1] == b'x' || b[1] == b'X') { 2 } else { 0 };
    let mut v = 0u32;
    while i < b.len() {
        let d = match b[i] {
            c @ b'0'..=b'9' => c - b'0',
            c @ b'a'..=b'f' => c - b'a' + 10,
            c @ b'A'..=b'F' => c - b'A' + 10,
            _ => panic!("BULB_FILE_VERSION is not hexadecimal"),
        };
        v = v << 4 | d as u32;
        i += 1;
    }
    v
}

/// Flash sector with the device's EUI-64, written at the factory.
const MAC_SECTOR: usize = 0xf_f000;
/// Telink's address prefix for devices without a factory address.
const TELINK_OUI: [u8; 3] = [0x38, 0xc1, 0xa4];
/// Address prefixes Telink stores in Bluetooth order (6-byte MAC first).
const BLE_ORDER_OUIS: [[u8; 3]; 7] = [
    [0x38, 0xc1, 0xa4],
    [0xd1, 0x19, 0xc4],
    [0xcb, 0x0b, 0xd8],
    [0xb4, 0xcf, 0x3c],
    [0xc7, 0xa3, 0xc0],
    [0x77, 0x5f, 0xd8],
    [0x28, 0x22, 0x38],
];

const TICKS_PER_MS: u32 = 16_000;

/// The diagnostic counters kept over a watchdog or software reset: in RAM
/// that the start-up code leaves alone (`.noinit`, `RETAINED`), behind a
/// header and a checksum, since after a power-on it holds whatever the chip
/// woke up with.
/// The black box's records (`zigbee::stack::blackbox`), outside `Stack`:
/// all zero, in .bss; the hal takes it.
static BLACK_BOX: TakeOnce<zigbee::BlackBox> = TakeOnce::new();

/// The `Retained` block: magic, length, checksum (a word each), then the
/// data. Any bit pattern is a valid value of it, which is what memory the
/// start-up code leaves alone holds after a power-on (the initialiser here
/// is not loaded); every access is volatile.
#[allow(unsafe_code)] // a linker attribute
#[link_section = ".noinit"]
static RETAINED: SharedBuf<{ 12 + zigbee::stack::RETAIN_MAX }> = SharedBuf::new([0; 12 + zigbee::stack::RETAIN_MAX]);
const RETAINED_MAGIC: u32 = 0x4449_4147;

fn retained_sum(len: u32, data: &[u8]) -> u32 {
    data.iter().fold(0x811C_9DC5u32 ^ len, |h, b| (h ^ *b as u32).wrapping_mul(0x0100_0193))
}

/// Link quality (and energy level) from a signal strength in dBm, on the
/// scale Telink's stack uses: -99 dBm and below is 0, -15 dBm and above 255.
fn lqi_from_rssi(rssi: i32) -> u8 {
    (255 * (rssi.clamp(-99, -15) + 99) / 84) as u8
}

pub struct Tc32Hal {
    /// Staging area erased up to this offset during the current download.
    staging_erased: u32,
    nv: Nv,
    ext: u64,
    last_tick: u32,
    rest: u32,
    ms: u32,
    rng: [u32; 2],
    black_box: Option<&'static mut zigbee::BlackBox>,
}

impl Tc32Hal {
    pub fn new() -> Self {
        let mut hal = Tc32Hal { staging_erased: 0, nv: Nv::new(), ext: 0, last_tick: reg::tick(), rest: 0, ms: 0, rng: [0; 2], black_box: BLACK_BOX.take_with(|| zigbee::BlackBox::EMPTY) };
        // Nothing here needs unpredictable numbers (keys come from the Trust
        // Center, nonces are counters): the generator only spreads backoffs
        // and jitter, so the timer and whatever the flash holds seed it.
        let mut seed = [0u8; 8];
        flash::read(MAC_SECTOR, &mut seed);
        hal.rng = [
            u32::from_le_bytes([seed[0], seed[1], seed[2], seed[3]]) ^ reg::tick() | 1,
            u32::from_le_bytes([seed[4], seed[5], seed[6], seed[7]]).wrapping_add(reg::tick()) | 1,
        ];
        hal.ext = hal.read_address();
        hal
    }

    /// The EUI-64 from the factory sector, in the layouts Telink's stack
    /// reads; a blank sector gets a random address under Telink's prefix,
    /// which is stored so that it stays the same.
    fn read_address(&mut self) -> u64 {
        let mut a = [0u8; 8];
        flash::read(MAC_SECTOR, &mut a);
        let blank = a.iter().all(|&b| b == 0xff) || a.iter().all(|&b| b == 0);
        if blank {
            if a[0] == 0 {
                flash::erase_sector(MAC_SECTOR);
            }
            let r = self.random().to_le_bytes();
            let r4 = self.random() as u8;
            let addr = [r[0], r[1], r[2], r[3], r4, TELINK_OUI[0], TELINK_OUI[1], TELINK_OUI[2]];
            flash::write(MAC_SECTOR, &addr[2..8]);
            flash::write(MAC_SECTOR + 6, &addr[0..2]);
            flash::read(MAC_SECTOR, &mut a);
        }
        if BLE_ORDER_OUIS.iter().any(|oui| a[3..6] == *oui) {
            a = [a[6], a[7], a[0], a[1], a[2], a[3], a[4], a[5]];
        }
        u64::from_le_bytes(a)
    }
}

impl Hal for Tc32Hal {
    /// The chip's AES block; it checks itself before its first use and
    /// the stack computes in software if it does not work.
    const AES: Option<zigbee::aes::Engine> = Some(crate::aes::encrypt);

    fn now_ms(&mut self) -> u32 {
        // the timer wraps every 268 s; the main loop calls this far more often
        let tick = reg::tick();
        let ticks = self.rest + tick.wrapping_sub(self.last_tick);
        self.last_tick = tick;
        self.rest = ticks % TICKS_PER_MS;
        self.ms = self.ms.wrapping_add(ticks / TICKS_PER_MS);
        self.ms
    }

    fn random(&mut self) -> u32 {
        // multiply-with-carry pair, stirred with the free-running timer
        self.rng[0] = 18000 * (self.rng[0] & 0xffff) + (self.rng[0] >> 16);
        self.rng[1] = 36969 * (self.rng[1] & 0xffff) + (self.rng[1] >> 16);
        ((self.rng[1] << 16).wrapping_add(self.rng[0])) ^ reg::tick()
    }

    fn ieee(&mut self) -> u64 {
        self.ext
    }

    fn set_channel(&mut self, channel: u8) {
        radio::set_channel(channel);
    }

    fn set_filter(&mut self, pan: u16, short: u16) {
        radio::set_filter(pan, short, self.ext);
    }

    fn transmit(&mut self, mpdu: &[u8]) -> TxStatus {
        radio::transmit(mpdu, || self.random())
    }

    fn transmit_at(&mut self, mpdu: &[u8], at_us: u32) {
        radio::transmit_at(mpdu, at_us.wrapping_mul(reg::TICKS_PER_US));
    }

    fn indirect_pending(&mut self, short: &[u16], ext: &[u64]) {
        radio::set_pending(short, ext);
    }

    fn poll_children(&mut self, short: &[u16]) {
        radio::set_children(short);
    }

    fn energy_detect(&mut self, channel: u8) -> u8 {
        lqi_from_rssi(radio::energy_detect(channel, 8))
    }

    fn receive(&mut self, buf: &mut [u8; 127]) -> Option<RxMeta> {
        let (len, rssi, end_tick) = radio::receive(buf)?;
        Some(RxMeta { len, lqi: lqi_from_rssi(rssi), rssi: rssi.clamp(-128, 127) as i8, end_us: end_tick / reg::TICKS_PER_US })
    }

    fn nv_load(&mut self, item: NvItem, buf: &mut [u8]) -> Option<usize> {
        self.nv.load(item as u8, buf)
    }

    fn nv_store(&mut self, item: NvItem, data: &[u8]) {
        self.nv.store(item as u8, data);
    }

    fn nv_erase(&mut self) {
        self.nv.erase();
    }

    fn ota_begin(&mut self, size: u32) -> bool {
        // one update at a time: not while an image is coming in over BLE
        if size > staging::SIZE || !staging::claim(Owner::Zigbee) {
            return false;
        }
        self.staging_erased = 0;
        true
    }

    fn ota_resume(&mut self, image_len: u32) -> bool {
        if image_len > staging::SIZE || !staging::claim(Owner::Zigbee) {
            return false;
        }
        // sectors below the checkpoint hold the image so far; the rest is
        // erased again as the download reaches it
        self.staging_erased = (image_len + 0xfff) & !0xfff;
        true
    }

    fn ota_write(&mut self, offset: u32, data: &[u8]) {
        let end = offset + data.len() as u32;
        if end > staging::SIZE || !staging::holds(Owner::Zigbee) {
            return;
        }
        // sectors are erased as the download reaches them: erasing the whole
        // area first would keep the radio deaf for seconds
        while self.staging_erased < end {
            let sector = staging::BASE + self.staging_erased as usize;
            if !flash::blank(sector) {
                flash::erase_sector(sector);
            }
            self.staging_erased += 0x1000;
        }
        flash::write(staging::BASE + offset as usize, data);
    }

    #[cfg(feature = "trace")]
    fn trace(&mut self, mut ev: zigbee::hal::Trace) {
        if ev.kind == zigbee::hal::trace::TX && ev.detail <= zigbee::hal::trace::TX_BUSY {
            let [attempts, busy, t0] = radio::LAST_TX.get();
            let us = reg::tick().wrapping_sub(t0) / reg::TICKS_PER_US;
            ev.aux = attempts.min(255) | busy.min(255) << 8 | us.min(0xffff) << 16;
        }
        crate::trace::record(ev);
    }

    fn nv_store_pair(&mut self, a: NvItem, a_data: &[u8], b: NvItem, b_data: &[u8]) {
        self.nv.store_pair((a as u8, a_data), (b as u8, b_data));
    }

    fn tx_attempts(&mut self) -> u8 {
        radio::counters().3
    }

    fn noise_dbm(&mut self) -> Option<i8> {
        radio::noise_dbm().map(|d| d.clamp(-128, 127) as i8)
    }

    fn counters(&mut self) -> HalCounters {
        let (cca_busy, rx_queue_full, rx_queue_peak, _) = radio::counters();
        #[cfg(feature = "ble")]
        let (ble_lends, ble_ms, ble_lend_max_us) = {
            let (l, m) = crate::ble_radio::lent();
            (l, m, crate::ble_radio::lent_max_us())
        };
        #[cfg(not(feature = "ble"))]
        let (ble_lends, ble_ms, ble_lend_max_us) = (0, 0, 0);
        let (flash_erases, flash_stores, flash_busy_max_us, loop_max_us) =
            (flash::ERASES.get(), flash::STORES.get(), flash::ERASE_MAX_US.get(), LOOP_MAX_US.get());
        #[cfg(feature = "ble")]
        let b = BLE_STATS.get();
        let (stack_free, irq_stack_free) = crate::stack::free();
        HalCounters {
            cca_busy,
            rx_queue_full,
            rx_queue_peak: rx_queue_peak as u32,
            ble_lends,
            ble_ms,
            flash_erases,
            flash_stores,
            loop_max_us,
            ble_lend_max_us,
            flash_busy_max_us,
            stack_free,
            irq_stack_free,
            #[cfg(feature = "ble")]
            ble_connections: b.connections,
            #[cfg(feature = "ble")]
            ble_disconnects: b.disconnects,
            #[cfg(feature = "ble")]
            ble_links_lost: b.links_lost,
            #[cfg(feature = "ble")]
            ble_events: b.events,
            #[cfg(feature = "ble")]
            ble_events_skipped: b.events_skipped,
            ..HalCounters::default()
        }
    }

    fn reset_cause(&mut self) -> ResetCause {
        crate::board::reset_cause()
    }

    fn black_box(&mut self) -> Option<&mut zigbee::BlackBox> {
        self.black_box.as_deref_mut()
    }

    fn retain(&mut self, data: &[u8]) {
        let n = data.len().min(zigbee::stack::RETAIN_MAX);
        let r = &RETAINED;
        r.write32(0, 0);
        r.copy_from(12, &data[..n]);
        r.write32(4, n as u32);
        r.write32(8, retained_sum(n as u32, &data[..n]));
        r.write32(0, RETAINED_MAGIC);
    }

    fn retained(&mut self, buf: &mut [u8]) -> Option<usize> {
        let r = &RETAINED;
        let n = r.read32(4) as usize;
        if r.read32(0) != RETAINED_MAGIC || n > zigbee::stack::RETAIN_MAX || n > buf.len() {
            return None;
        }
        r.copy_to(12, &mut buf[..n]);
        if retained_sum(n as u32, &buf[..n]) != r.read32(8) {
            return None;
        }
        // taken once
        r.write32(0, 0);
        Some(n)
    }

    fn ota_activate(&mut self) {
        // the stack checked the image as it arrived; what the flash holds is checked again here
        staging::activate();
        staging::release(Owner::Zigbee);
    }

    #[cfg(feature = "ble")]
    fn zcl_received(&mut self, src: u16, cluster: u16, cmd: u8, frame_control: u8, lqi: u8, rssi: i8) {
        let t = self.now_ms() / 1000;
        crate::diag::frame(t, src, cluster, cmd, frame_control, lqi, rssi);
    }

    #[cfg(feature = "sense")]
    fn sense_listen(&mut self, pan: u16) {
        radio::set_probe_pan(pan);
    }
}

/// The bulb's link to the outside: the Zigbee network, and the debug link
/// (`link.rs`) for tests and debuggers.
///
/// The radio and the stack are started on the first poll, not at
/// construction: the light must come on first after power-on, and reading
/// the stores takes a moment.
pub struct Link {
    stack: Option<Stack<Tc32Hal>>,
    /// BLE while it runs: the settings cluster says when (`BleMode`), and
    /// it is started and stopped between its events, which hand the radio
    /// back each time.
    #[cfg(feature = "ble")]
    ble: Option<ble::Ble<crate::ble::BleHal>>,
    /// The light as last reported, for a BLE side started later.
    #[cfg(feature = "ble")]
    last_light: Option<StateReport>,
    /// The window after power-on (`BleMode::Window`) has passed; it does
    /// not open again until the next power-on.
    #[cfg(feature = "ble")]
    window_over: bool,
    /// The diagnostic service's values and commands (`diag.rs`).
    #[cfg(feature = "ble")]
    diag: crate::diag::Glue,
    light: Option<StateReport>,
    debug: DebugLink,
}

/// Where the link (and so both stacks) lives: in .bss, where the linker
/// counts it against the SRAM, not on the main stack. As a value of `main`
/// it was there twice (`Bulb::boot` takes it by value), some 20 KB.
static LINK: TakeOnce<Link> = TakeOnce::new();

impl Link {
    /// The link, built in its place. There is one; a second call panics.
    #[inline(never)]
    pub fn take() -> &'static mut Link {
        LINK.take_with(Link::new).expect("the radio link is taken once")
    }

    pub fn new() -> Self {
        Link {
            stack: None,
            #[cfg(feature = "ble")]
            ble: None,
            #[cfg(feature = "ble")]
            last_light: None,
            #[cfg(feature = "ble")]
            window_over: false,
            #[cfg(feature = "ble")]
            diag: crate::diag::Glue::new(),
            light: None,
            debug: DebugLink,
        }
    }

    /// Starts the radio and the stacks. Apart from `poll`: what the stacks
    /// are built in on the way into `self` takes as much room again on the
    /// main stack, and here that room is given back before `poll` goes on
    /// (in `poll`'s own frame it stayed taken under every call it makes).
    #[inline(never)]
    #[cold]
    fn start(&mut self) {
        radio::init();
        Stack::new_in(&mut self.stack, Tc32Hal::new(), IDENTITY);
        // BLE comes after the stack (`ble_switch`): a blank factory sector
        // has its address by then, and the stack has read the BLE setting
    }

    /// (Its own frame: what BLE is built in is not added to the stack's.)
    #[cfg(feature = "ble")]
    #[inline(never)]
    #[cold]
    fn start_ble(&mut self) {
        let ble = self.ble.insert(ble::Ble::new(crate::ble::BleHal::new(), crate::ble::IDENTITY));
        if let Some(light) = &self.last_light {
            ble.set_light(light);
        }
        if let Some(stack) = self.stack.as_mut() {
            self.diag.start(ble, stack);
        }
    }

    /// Whether BLE is to run now, by the setting the stack keeps.
    #[cfg(feature = "ble")]
    fn ble_wanted(&mut self) -> bool {
        let Some(stack) = self.stack.as_mut() else {
            return false;
        };
        match stack.ble_mode() {
            BleMode::On => true,
            BleMode::Off => false,
            BleMode::Window => {
                // the stack's clock starts with the stack, right after power-on
                if !self.window_over {
                    let window_ms = stack.ble_window_minutes() as u32 * 60_000;
                    self.window_over = stack.hal.now_ms() >= window_ms;
                }
                !self.window_over
            }
        }
    }

    /// Starts or stops BLE as the setting says. A firmware update over BLE
    /// is let finish first (`ble::Ble::ota_busy`: the transfer, the restart
    /// into a good image, the connection whose central reads the final
    /// status). When the window after power-on closes, a connection under
    /// way goes on until it ends, as on the stock bulb; when BLE is switched
    /// off, any other connection is dropped and the central sees the link
    /// time out.
    #[cfg(feature = "ble")]
    fn ble_switch(&mut self) {
        let wanted = self.ble_wanted();
        let window = self.stack.as_ref().is_some_and(|s| s.ble_mode() == BleMode::Window);
        match self.ble.as_mut() {
            None if wanted => self.start_ble(),
            Some(ble) if !wanted && !ble.ota_busy() && !(window && ble.link_state() == ble::LinkState::Connected) => {
                use ble::Hal as _;
                ble.hal().connection_reset();
                self.ble = None;
                staging::release(Owner::Ble);
            }
            _ => {}
        }
        if let Some(stack) = self.stack.as_mut() {
            stack.set_ble_active(self.ble.is_some());
        }
    }
}

/// The link as `main` holds it: by reference (see `LINK`).
impl Radio for &mut Link {
    fn poll(&mut self) -> Option<Command> {
        (**self).poll()
    }

    fn report(&mut self, state: &StateReport) {
        (**self).report(state)
    }
}

impl Radio for Link {
    fn poll(&mut self) -> Option<Command> {
        if let Some(c) = self.debug.poll() {
            return Some(c);
        }
        if self.stack.is_none() {
            self.start();
        }
        #[cfg(feature = "ble")]
        self.ble_switch();
        let stack = self.stack.as_mut()?;
        if let Some(light) = self.light.take() {
            stack.set_light(&light);
            #[cfg(feature = "ble")]
            {
                if let Some(ble) = self.ble.as_mut() {
                    ble.set_light(&light);
                }
                self.last_light = Some(light);
            }
        }
        // BLE first: its events have their moments, and it gives the radio
        // back before the stack looks at it (`ble_switch` has decided whether
        // it runs at all)
        // (no BLE: the main loop's wait does not end for it; a round that
        // returned before this point keeps the last moment, which is safe)
        #[cfg(feature = "ble")]
        if self.ble.is_none() {
            BLE_NEXT.set(None);
        }
        #[cfg(feature = "ble")]
        if let Some(ble) = self.ble.as_mut() {
            BLE_STATS.set(ble.stats);
            if !ble.ota_active() {
                staging::release(Owner::Ble);
            }
            self.diag.refresh(ble, stack);
            if let Some(d) = ble.take_diag_command() {
                if let Some(c) = self.diag.execute(d, stack) {
                    return Some(c);
                }
            }
            let c = ble.poll();
            BLE_NEXT.set(Some(reg::tick().wrapping_add(ble.idle_time())));
            if let Some(c) = c {
                return Some(c);
            }
        }
        #[cfg(feature = "ble")]
        self.diag.restart_due(stack);
        if matches!(stack.ota_state(), OtaState::Idle | OtaState::Querying) {
            staging::release(Owner::Zigbee);
        }
        // no probes of the motion sensing while an image comes in over BLE
        #[cfg(all(feature = "sense", feature = "ble"))]
        stack.sense_pause(self.ble.as_ref().is_some_and(|b| b.ota_busy()));
        let c = stack.poll();
        crate::link::publish_network(stack.state() as u8, stack.short_address(), stack.channel());
        c
    }

    fn report(&mut self, state: &StateReport) {
        self.light = Some(*state);
        self.debug.report(state);
    }
}
