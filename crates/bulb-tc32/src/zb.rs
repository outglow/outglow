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
use crate::nv::{self, Nv};
use crate::radio;
use crate::reg;
use crate::staging::{self, Owner};

pub static LOOP_MAX_US: Shared<u32> = Shared::new(0);
pub static BLE_NEXT: Shared<Option<u32>> = Shared::new(None);
#[cfg(feature = "ble")]
const ADV_GUARD: u32 = 4_000 * reg::TICKS_PER_US;
#[cfg(feature = "ble")]
const DIAG_ROOM: u32 = 6_000 * reg::TICKS_PER_US;
#[cfg(feature = "ble")]
static BLE_STATS: Shared<ble::Stats> = Shared::new(ble::Stats { adv_events: 0, connections: 0, events: 0, events_heard: 0, events_skipped: 0, disconnects: 0, links_lost: 0 });

pub(crate) const IDENTITY: Identity = Identity {
    manufacturer: "Outglow",
    model: BOARD.kind,
    sw_build: VERSION,
    manufacturer_code: 0x1141,
    app_version: 1,
    hw_version: BOARD.hw_version,
    color_temperature: BOARD.tunable(),
    ct_min: BOARD.ct_min,
    ct_max: BOARD.ct_max,
    color: BOARD.rgb.is_some(),
    image_type: IMAGE_TYPE,
    file_version: FILE_VERSION,
    touchlink_master_key: MASTER_KEY,
    install_code: None,
    ble: cfg!(feature = "ble"),
    #[cfg(feature = "sense")]
    sense: true,
};

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

pub const IMAGE_TYPE: u16 = BOARD.image_type;

pub const VERSION: &str = match option_env!("BULB_VERSION") {
    Some(v) => v,
    None => "1.0.0",
};

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

const MAC_SECTOR: usize = 0xf_f000;
const TELINK_OUI: [u8; 3] = [0x38, 0xc1, 0xa4];
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

static BLACK_BOX: TakeOnce<zigbee::BlackBox> = TakeOnce::new();

#[allow(unsafe_code)]
#[cfg_attr(target_os = "none", link_section = ".noinit")]
static RETAINED: SharedBuf<{ 12 + zigbee::stack::RETAIN_MAX }> = SharedBuf::new([0; 12 + zigbee::stack::RETAIN_MAX]);
const RETAINED_MAGIC: u32 = 0x4449_4147;

fn retained_sum(len: u32, data: &[u8]) -> u32 {
    data.iter().fold(0x811C_9DC5u32 ^ len, |h, b| (h ^ *b as u32).wrapping_mul(0x0100_0193))
}

fn lqi_from_rssi(rssi: i32) -> u8 {
    (255 * (rssi.clamp(-99, -15) + 99) / 84) as u8
}

pub struct Tc32Hal {
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
        let mut hal = Tc32Hal { staging_erased: 0, nv: nv::open(), ext: 0, last_tick: reg::tick(), rest: 0, ms: 0, rng: [0; 2], black_box: BLACK_BOX.take_with(|| zigbee::BlackBox::EMPTY) };
        let mut seed = [0u8; 8];
        flash::read(MAC_SECTOR, &mut seed);
        hal.rng = [
            u32::from_le_bytes([seed[0], seed[1], seed[2], seed[3]]) ^ reg::tick() | 1,
            u32::from_le_bytes([seed[4], seed[5], seed[6], seed[7]]).wrapping_add(reg::tick()) | 1,
        ];
        hal.ext = hal.read_address();
        hal
    }

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
    const AES: Option<zigbee::aes::Engine> = Some(crate::aes::encrypt);

    fn now_ms(&mut self) -> u32 {
        let tick = reg::tick();
        let ticks = self.rest + tick.wrapping_sub(self.last_tick);
        self.last_tick = tick;
        self.rest = ticks % TICKS_PER_MS;
        self.ms = self.ms.wrapping_add(ticks / TICKS_PER_MS);
        self.ms
    }

    fn random(&mut self) -> u32 {
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
        self.nv.forget_all_but(NvItem::Counters as u8);
    }

    fn ota_begin(&mut self, size: u32) -> bool {
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
        self.staging_erased = (image_len + 0xfff) & !0xfff;
        true
    }

    fn ota_write(&mut self, offset: u32, data: &[u8]) {
        let end = offset + data.len() as u32;
        if end > staging::SIZE || !staging::holds(Owner::Zigbee) {
            return;
        }
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
            ev.aux = crate::trace::tx_aux(radio::LAST_TX.get());
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
        r.write32(0, 0);
        Some(n)
    }

    fn ota_activate(&mut self) {
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

pub struct Link {
    stack: Option<Stack<Tc32Hal>>,
    #[cfg(feature = "ble")]
    ble: Option<ble::Ble<crate::ble::BleHal>>,
    #[cfg(feature = "ble")]
    last_light: Option<StateReport>,
    #[cfg(feature = "ble")]
    window_over: bool,
    #[cfg(feature = "ble")]
    diag: crate::diag::Glue,
    light: Option<StateReport>,
    debug: DebugLink,
}

pub(crate) static LINK: TakeOnce<Link> = TakeOnce::new();

impl Link {
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

    #[inline(never)]
    #[cold]
    fn start(&mut self) {
        radio::init();
        Stack::new_in(&mut self.stack, Tc32Hal::new(), IDENTITY);
    }
}

#[cfg(feature = "ble")]
#[inline(never)]
#[cold]
fn start_ble(stack: &mut Stack<Tc32Hal>, slot: &mut Option<ble::Ble<crate::ble::BleHal>>, diag: &mut crate::diag::Glue, light: &Option<StateReport>) {
    let ble = slot.insert(ble::Ble::new(crate::ble::BleHal::new(), crate::ble::IDENTITY));
    if let Some(light) = light {
        ble.set_light(light);
    }
    diag.start(ble, stack);
}

#[cfg(feature = "ble")]
fn ble_wanted(stack: &mut Stack<Tc32Hal>, window_over: &mut bool) -> bool {
    match stack.ble_mode() {
        BleMode::On => true,
        BleMode::Off => false,
        BleMode::Window => {
            if !*window_over {
                let window_ms = stack.ble_window_minutes() as u32 * 60_000;
                *window_over = stack.hal.now_ms() >= window_ms;
            }
            !*window_over
        }
    }
}

#[cfg(feature = "ble")]
fn ble_switch(link: &mut Link) -> Option<()> {
    let stack = link.stack.as_mut()?;
    let wanted = ble_wanted(stack, &mut link.window_over);
    match link.ble.as_mut() {
        None if wanted => start_ble(stack, &mut link.ble, &mut link.diag, &link.last_light),
        Some(ble) if !wanted && !ble.ota_busy() && ble.link_state() != ble::LinkState::Connected => {
            use ble::Hal as _;
            ble.hal().connection_reset();
            link.ble = None;
            staging::release(Owner::Ble);
        }
        _ => {}
    }
    stack.set_ble_active(link.ble.is_some());
    Some(())
}

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
        if let Some(light) = self.light.take() {
            let stack = self.stack.as_mut()?;
            stack.set_light(&light);
            #[cfg(feature = "ble")]
            {
                if let Some(ble) = self.ble.as_mut() {
                    ble.set_light(&light);
                }
                self.last_light = Some(light);
            }
        }
        #[cfg(feature = "ble")]
        ble_switch(self);
        let stack = self.stack.as_mut()?;
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
            if ble.idle_time() >= DIAG_ROOM {
                self.diag.refresh(ble, stack);
            }
            if let Some(d) = ble.take_diag_command() {
                if let Some(c) = self.diag.execute(d, stack) {
                    return Some(c);
                }
            }
            let c = ble.poll();
            let idle = ble.idle_time();
            BLE_NEXT.set(Some(reg::tick().wrapping_add(idle)));
            if let Some(c) = c {
                return Some(c);
            }
            if idle < ADV_GUARD && ble.link_state() == ble::LinkState::Advertising {
                return None;
            }
        }
        #[cfg(feature = "ble")]
        self.diag.restart_due(stack);
        if matches!(stack.ota_state(), OtaState::Idle | OtaState::Querying) {
            staging::release(Owner::Zigbee);
        }
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

#[cfg(test)]
#[path = "tests/zb.rs"]
mod tests;
