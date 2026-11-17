#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;

use bulb_core::command::{Command, PowerOnBehavior, StateReport};
use zigbee::aes::Aes128;
use zigbee::crypto;
use zigbee::hal::{Hal, HalCounters, NvItem, ResetCause, RxMeta, TxStatus};
use zigbee::mac::{self, Addr};
use zigbee::nwk;
use zigbee::stack::{Identity, Stack, State};
use zigbee::util::{Buf, Reader};

pub const PAN: u16 = 0x1A62;
pub const COORD_EXT: u64 = 0x00124B0000000001;
pub const EXT_PAN: u64 = 0xDDDDDDDDDDDDDDDD;
pub const NWK_KEY: [u8; 16] = [1, 3, 5, 7, 9, 11, 13, 15, 0, 2, 4, 6, 8, 10, 12, 13];
pub const TCLK: [u8; 16] = *b"ZigBeeAlliance09";
pub const DISTRIBUTED_LINK_KEY: [u8; 16] = [0xD0, 0xD1, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9, 0xDA, 0xDB, 0xDC, 0xDD, 0xDE, 0xDF];

pub const STAGING_MAX: usize = 0x4000;

pub fn make_image(len: usize, seed: u8) -> Vec<u8> {
    let mut image: Vec<u8> = (0..len).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect();
    image[8..12].copy_from_slice(b"KNLT");
    image[0x18..0x1C].copy_from_slice(&(len as u32).to_le_bytes());
    seal_image(&mut image);
    image
}

pub fn seal_image(image: &mut [u8]) {
    let n = image.len() - 4;
    let mut crc = 0xFFFF_FFFFu32;
    for b in &image[..n] {
        crc ^= *b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    image[n..].copy_from_slice(&crc.to_le_bytes());
}

pub fn make_ota_file(image: &[u8], manufacturer: u16, image_type: u16, version: u32, extra: usize, other_element: bool) -> Vec<u8> {
    let header_len = 56 + extra;
    let mut body = vec![];
    if other_element {
        body.extend_from_slice(&[0x01, 0x00, 5, 0, 0, 0, 1, 2, 3, 4, 5]);
    }
    body.extend_from_slice(&[0x00, 0x00]);
    body.extend_from_slice(&(image.len() as u32).to_le_bytes());
    body.extend_from_slice(image);
    let mut f = vec![];
    f.extend_from_slice(&0x0BEE_F11Eu32.to_le_bytes());
    f.extend_from_slice(&0x0100u16.to_le_bytes());
    f.extend_from_slice(&(header_len as u16).to_le_bytes());
    f.extend_from_slice(&0u16.to_le_bytes());
    f.extend_from_slice(&manufacturer.to_le_bytes());
    f.extend_from_slice(&image_type.to_le_bytes());
    f.extend_from_slice(&version.to_le_bytes());
    f.extend_from_slice(&2u16.to_le_bytes());
    f.extend_from_slice(&[b'x'; 32]);
    f.extend_from_slice(&((header_len + body.len()) as u32).to_le_bytes());
    f.extend_from_slice(&vec![0xEE; extra]);
    f.extend_from_slice(&body);
    f
}

pub struct OtaFile {
    pub manufacturer: u16,
    pub image_type: u16,
    pub version: u32,
    pub data: Vec<u8>,
}

pub const IDENTITY: Identity = Identity {
    manufacturer: "AwoX",
    model: "ESMLFzm_w6_Dimm",
    sw_build: "test",
    manufacturer_code: 0x1141,
    app_version: 1,
    hw_version: 1,
    color_temperature: false,
    ct_min: bulb_core::CT_MIN,
    ct_max: bulb_core::CT_MAX,
    color: false,
    image_type: 0x0390,
    file_version: 0x0001_0000,
    touchlink_master_key: None,
    install_code: None,
    ble: false,
    sense: false,
};

pub const IDENTITY_TW: Identity = Identity { model: "ESMLFzm_w6_TW", color_temperature: true, ..IDENTITY };

pub const IDENTITY_COLOR: Identity =
    Identity { model: "outglow-rgbtw", color_temperature: true, color: true, ct_min: 153, ct_max: 370, image_type: 0x0399, ..IDENTITY };

pub const IDENTITY_BLE: Identity = Identity { ble: true, ..IDENTITY };

pub const IDENTITY_SENSE: Identity = Identity { sense: true, ..IDENTITY };

pub type RssiModel = Box<dyn FnMut(usize, &[u8], u32) -> Option<i8>>;

pub struct Port {
    pub channel: u8,
    pub pan: u16,
    pub short: u16,
    pub ext: u64,
    pub powered: bool,
    pub queue: VecDeque<(Vec<u8>, u8)>,
    pub nv: HashMap<u8, Vec<u8>>,
    pub nv_writes: u32,
    pub staging: Vec<u8>,
    pub activated: bool,
    pub poll_children: Option<Vec<u16>>,
    pub pending: (Vec<u16>, Vec<u64>),
    pub last_attempts: u8,
    pub next_attempts: u8,
    pub hal_counters: HalCounters,
    pub reset_cause: ResetCause,
    pub retained: Option<Vec<u8>>,
    pub zcl_received: Vec<(u16, u16, u8, u8, u8, i8)>,
    pub traces: Vec<zigbee::hal::Trace>,
    pub probe_pan: u16,
    pub busy: u32,
    pub no_black_box: bool,
    pub noise: Option<i8>,
}

pub struct Air {
    pub now: u32,
    pub ports: Vec<Port>,
    pub lqi: Vec<Vec<u8>>,
    pub loss: Vec<Vec<u8>>,
    pub log: Vec<(u32, usize, Vec<u8>)>,
    pub timed: Vec<(usize, u32, u8, Vec<u8>)>,
    pub energy: [u8; 16],
    pub rssi_model: Option<RssiModel>,
    rng: u32,
}

pub type Shared = Rc<RefCell<Air>>;

const BULB_OUI: u64 = 0xA4_C138;

impl Drop for Air {
    fn drop(&mut self) {
        let Ok(dir) = std::env::var("ZIGBEE_PCAP_DIR") else {
            return;
        };
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("air").replace("::", ".");
        let mut out = Vec::new();
        out.extend_from_slice(&0xA1B2_C3D4u32.to_le_bytes());
        out.extend_from_slice(&[2, 0, 4, 0]);
        out.extend_from_slice(&[0; 8]);
        out.extend_from_slice(&65535u32.to_le_bytes());
        out.extend_from_slice(&230u32.to_le_bytes());
        let mut senders = String::new();
        for (at, from, mpdu) in &self.log {
            senders.push_str(if self.ports[*from].ext >> 40 == BULB_OUI { "B\n" } else { "T\n" });
            out.extend_from_slice(&(at / 1000).to_le_bytes());
            out.extend_from_slice(&(at % 1000 * 1000).to_le_bytes());
            out.extend_from_slice(&(mpdu.len() as u32).to_le_bytes());
            out.extend_from_slice(&(mpdu.len() as u32).to_le_bytes());
            out.extend_from_slice(mpdu);
        }
        let mut n = 0;
        loop {
            let stem = if n == 0 { format!("{dir}/{name}") } else { format!("{dir}/{name}.{n}") };
            if !std::path::Path::new(&format!("{stem}.pcap")).exists() {
                let _ = std::fs::write(format!("{stem}.pcap"), &out);
                let _ = std::fs::write(format!("{stem}.senders"), senders);
                break;
            }
            n += 1;
        }
    }
}

impl Air {
    pub fn new() -> Shared {
        Rc::new(RefCell::new(Air {
            now: 1000,
            ports: vec![],
            lqi: vec![],
            loss: vec![],
            log: vec![],
            timed: vec![],
            energy: [0; 16],
            rssi_model: None,
            rng: 0x1234_5678,
        }))
    }

    pub fn add_port(&mut self, ext: u64) -> usize {
        self.ports.push(Port {
            channel: 11,
            pan: 0xFFFF,
            short: 0xFFFF,
            ext,
            powered: true,
            queue: VecDeque::new(),
            nv: HashMap::new(),
            nv_writes: 0,
            staging: vec![],
            activated: false,
            poll_children: None,
            pending: (vec![], vec![]),
            last_attempts: 0,
            next_attempts: 0,
            hal_counters: HalCounters::default(),
            reset_cause: ResetCause::PowerOn,
            retained: None,
            zcl_received: Vec::new(),
            traces: Vec::new(),
            probe_pan: 0xFFFF,
            busy: 0,
            no_black_box: false,
            noise: Some(-97),
        });
        let n = self.ports.len();
        for row in self.lqi.iter_mut() {
            row.push(255);
        }
        self.lqi.push(vec![255; n]);
        for row in self.loss.iter_mut() {
            row.push(0);
        }
        self.loss.push(vec![0; n]);
        n - 1
    }

    pub fn link(&mut self, a: usize, b: usize, lqi: u8) {
        self.lqi[a][b] = lqi;
        self.lqi[b][a] = lqi;
    }

    pub fn lossy(&mut self, a: usize, b: usize, percent: u8) {
        self.loss[a][b] = percent;
        self.loss[b][a] = percent;
    }

    fn lost(&mut self, from: usize, to: usize) -> bool {
        let l = self.loss[from][to];
        l != 0 && self.random() % 100 < l as u32
    }

    fn random(&mut self) -> u32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        self.rng
    }

    pub fn transmit(&mut self, from: usize, mpdu: &[u8]) -> TxStatus {
        self.log.push((self.now, from, mpdu.to_vec()));
        let Some((h, _)) = mac::Header::parse(mpdu) else {
            return TxStatus::Ok;
        };
        let channel = self.ports[from].channel;
        let mut acked = false;
        let mut lossy_attempts = 0u8;
        for j in 0..self.ports.len() {
            let lqi = self.lqi[from][j];
            let (powered, port_channel, pan, short, ext, probe_pan) = {
                let p = &self.ports[j];
                (p.powered, p.channel, p.pan, p.short, p.ext, p.probe_pan)
            };
            if j == from || lqi == 0 || !powered || port_channel != channel {
                continue;
            }
            let pan_ok = h.dst_pan == 0xFFFF || h.dst_pan == pan;
            let (accept, ack) = match h.dst {
                Addr::None => (true, false),
                Addr::Short(0xFFFF) => (pan_ok || (probe_pan != 0xFFFF && h.dst_pan == probe_pan), false),
                Addr::Short(s) => (pan_ok && s == short, true),
                Addr::Ext(e) => (pan_ok && e == ext, true),
            };
            if !accept {
                continue;
            }
            if self.loss[from][j] == 0 {
                self.ports[j].queue.push_back((mpdu.to_vec(), lqi));
                acked |= ack;
                continue;
            }
            if !(ack && h.ack_request) {
                if !self.lost(from, j) {
                    self.ports[j].queue.push_back((mpdu.to_vec(), lqi));
                }
                continue;
            }
            let mut received = false;
            for attempt in 1..=4u8 {
                lossy_attempts = attempt;
                if self.lost(from, j) {
                    continue;
                }
                received = true;
                if !self.lost(j, from) {
                    acked = true;
                    break;
                }
            }
            if received {
                self.ports[j].queue.push_back((mpdu.to_vec(), lqi));
            }
        }
        let p = &mut self.ports[from];
        if h.ack_request && !acked {
            p.last_attempts = 4;
            TxStatus::NoAck
        } else if lossy_attempts > 0 {
            p.last_attempts = lossy_attempts;
            TxStatus::Ok
        } else {
            p.last_attempts = if h.ack_request { core::mem::take(&mut p.next_attempts).max(1) } else { 1 };
            TxStatus::Ok
        }
    }
}

pub struct SimHal {
    pub air: Shared,
    pub id: usize,
    pub bx: std::boxed::Box<zigbee::BlackBox>,
}

impl Hal for SimHal {
    fn now_ms(&mut self) -> u32 {
        self.air.borrow().now
    }
    fn random(&mut self) -> u32 {
        self.air.borrow_mut().random()
    }
    fn ieee(&mut self) -> u64 {
        self.air.borrow().ports[self.id].ext
    }
    fn set_channel(&mut self, channel: u8) {
        self.air.borrow_mut().ports[self.id].channel = channel;
    }
    fn set_filter(&mut self, pan: u16, short: u16) {
        let mut air = self.air.borrow_mut();
        air.ports[self.id].pan = pan;
        air.ports[self.id].short = short;
    }
    fn transmit(&mut self, mpdu: &[u8]) -> TxStatus {
        let mut air = self.air.borrow_mut();
        if air.ports[self.id].busy > 0 {
            air.ports[self.id].busy -= 1;
            return TxStatus::ChannelBusy;
        }
        air.transmit(self.id, mpdu)
    }
    fn ota_begin(&mut self, size: u32) -> bool {
        if size as usize > STAGING_MAX {
            return false;
        }
        self.air.borrow_mut().ports[self.id].staging = vec![0xFF; STAGING_MAX];
        true
    }
    fn ota_resume(&mut self, image_len: u32) -> bool {
        let mut air = self.air.borrow_mut();
        let staging = &mut air.ports[self.id].staging;
        if staging.len() != STAGING_MAX {
            return false;
        }
        let from = (image_len as usize + 0xFFF) & !0xFFF;
        staging[from.min(STAGING_MAX)..].iter_mut().for_each(|b| *b = 0xFF);
        true
    }
    fn ota_write(&mut self, offset: u32, data: &[u8]) {
        let mut air = self.air.borrow_mut();
        let staging = &mut air.ports[self.id].staging;
        for (i, b) in data.iter().enumerate() {
            staging[offset as usize + i] &= *b;
        }
    }
    fn ota_activate(&mut self) {
        let mut air = self.air.borrow_mut();
        let p = &mut air.ports[self.id];
        p.staging[8] &= 0x4B;
        p.activated = true;
    }
    fn poll_children(&mut self, short: &[u16]) {
        self.air.borrow_mut().ports[self.id].poll_children = Some(short.to_vec());
    }
    fn indirect_pending(&mut self, short: &[u16], ext: &[u64]) {
        self.air.borrow_mut().ports[self.id].pending = (short.to_vec(), ext.to_vec());
    }
    fn energy_detect(&mut self, channel: u8) -> u8 {
        let mut air = self.air.borrow_mut();
        air.ports[self.id].channel = channel;
        air.energy[channel as usize - 11]
    }
    fn transmit_at(&mut self, mpdu: &[u8], at_us: u32) {
        let mut air = self.air.borrow_mut();
        let channel = air.ports[self.id].channel;
        air.timed.push((self.id, at_us, channel, mpdu.to_vec()));
        air.transmit(self.id, mpdu);
    }
    fn receive(&mut self, buf: &mut [u8; 127]) -> Option<RxMeta> {
        let mut air = self.air.borrow_mut();
        let now = air.now;
        loop {
            let (frame, lqi) = air.ports[self.id].queue.pop_front()?;
            let rssi = match air.rssi_model.as_mut() {
                None => (lqi as i32 * 84 / 255 - 99) as i8,
                Some(model) => match model(self.id, &frame, now) {
                    Some(r) => r,
                    None => continue,
                },
            };
            buf[..frame.len()].copy_from_slice(&frame);
            return Some(RxMeta { len: frame.len(), lqi, rssi, end_us: now.wrapping_mul(1000) });
        }
    }
    fn sense_listen(&mut self, pan: u16) {
        self.air.borrow_mut().ports[self.id].probe_pan = pan;
    }
    fn nv_load(&mut self, item: NvItem, buf: &mut [u8]) -> Option<usize> {
        let air = self.air.borrow();
        let data = air.ports[self.id].nv.get(&(item as u8))?;
        if data.len() > buf.len() {
            return None;
        }
        buf[..data.len()].copy_from_slice(data);
        Some(data.len())
    }
    fn nv_store(&mut self, item: NvItem, data: &[u8]) {
        let mut air = self.air.borrow_mut();
        air.ports[self.id].nv.insert(item as u8, data.to_vec());
        air.ports[self.id].nv_writes += 1;
    }
    fn nv_erase(&mut self) {
        self.air.borrow_mut().ports[self.id].nv.clear();
    }
    fn nv_store_pair(&mut self, a: NvItem, a_data: &[u8], b: NvItem, b_data: &[u8]) {
        let mut air = self.air.borrow_mut();
        let p = &mut air.ports[self.id];
        p.nv.insert(a as u8, a_data.to_vec());
        p.nv.insert(b as u8, b_data.to_vec());
        p.nv_writes += 1;
    }
    fn tx_attempts(&mut self) -> u8 {
        self.air.borrow().ports[self.id].last_attempts
    }
    fn counters(&mut self) -> HalCounters {
        self.air.borrow().ports[self.id].hal_counters
    }
    fn black_box(&mut self) -> Option<&mut zigbee::BlackBox> {
        if self.air.borrow().ports[self.id].no_black_box {
            return None;
        }
        Some(&mut self.bx)
    }
    fn noise_dbm(&mut self) -> Option<i8> {
        self.air.borrow().ports[self.id].noise
    }
    fn reset_cause(&mut self) -> ResetCause {
        self.air.borrow().ports[self.id].reset_cause
    }
    fn retain(&mut self, data: &[u8]) {
        self.air.borrow_mut().ports[self.id].retained = Some(data.to_vec());
    }
    fn trace(&mut self, ev: zigbee::hal::Trace) {
        self.air.borrow_mut().ports[self.id].traces.push(ev);
    }
    fn zcl_received(&mut self, src: u16, cluster: u16, cmd: u8, frame_control: u8, lqi: u8, rssi: i8) {
        self.air.borrow_mut().ports[self.id].zcl_received.push((src, cluster, cmd, frame_control, lqi, rssi));
    }
    fn retained(&mut self, buf: &mut [u8]) -> Option<usize> {
        let air = self.air.borrow();
        let r = air.ports[self.id].retained.as_ref()?;
        buf.get_mut(..r.len())?.copy_from_slice(r);
        Some(r.len())
    }
}

pub struct Node {
    pub id: usize,
    pub stack: Stack<SimHal>,
    pub on: bool,
    pub level: u8,
    pub color_temp: u16,
    pub color_mode: bulb_core::colour::ColorMode,
    pub hue: u8,
    pub sat: u8,
    pub x: u16,
    pub y: u16,
    pub commands: Vec<Command>,
    pub command_frames: Vec<usize>,
}

impl Node {
    fn apply(&mut self, c: Command) {
        match c {
            Command::On => self.on = true,
            Command::Off => self.on = false,
            Command::Toggle => self.on = !self.on,
            Command::MoveToLevel { level, .. } => {
                if level == 0 {
                    self.on = false;
                } else {
                    self.on = true;
                    self.level = level;
                }
            }
            Command::MoveToColorTemperature { mireds, .. } => {
                self.color_temp = mireds;
                self.color_mode = bulb_core::colour::ColorMode::Temperature;
            }
            Command::MoveToHueSaturation { hue, sat, .. } => {
                (self.hue, self.sat) = (hue, sat);
                self.color_mode = bulb_core::colour::ColorMode::HueSaturation;
            }
            Command::MoveToXy { x, y, .. } => {
                (self.x, self.y) = (x, y);
                self.color_mode = bulb_core::colour::ColorMode::Xy;
            }
            Command::SetLevelWhileOff(level) => self.level = level,
            _ => {}
        }
        self.commands.push(c);
        self.command_frames.push(self.stack.hal.air.borrow().log.len());
        self.report();
    }

    pub fn report(&mut self) {
        self.stack.set_light(&StateReport {
            on: self.on,
            level: self.level,
            power_on_behavior: PowerOnBehavior::Previous,
            startup_level: 0xFF,
            color_temp: self.color_temp,
            startup_color_temp: 0xFFFF,
            color_mode: self.color_mode,
            hue: self.hue,
            sat: self.sat,
            x: self.x,
            y: self.y,
            hue_moving: false,
            candle: false,
            shown: Default::default(),
        });
    }

    pub fn short(&self) -> u16 {
        self.stack.short_address()
    }
}

pub struct Device {
    pub ext: u64,
    pub short: u16,
    pub announced: bool,
    pub link_status: bool,
    pub tclk: [u8; 16],
    pub tclk_new: Option<[u8; 16]>,
    pub verified: bool,
    pub parent: Option<u16>,
    pub left: bool,
}

pub struct Coord {
    pub air: Shared,
    pub id: usize,
    pub pan: u16,
    pub key: [u8; 16],
    pub key_seq: u8,
    pub fc: u32,
    pub aps_fc: u32,
    seq: u8,
    pub permit_join: bool,
    pub stack_revision: u16,
    pub node_desc_status: u8,
    pub link_key_transport_id: u8,
    pub send_key_in_the_clear: bool,
    pub restart_link_key_counter: bool,
    pub devices: Vec<Device>,
    pending: Vec<u64>,
    key_due: Vec<(u32, u16, u64)>,
    next_short: u16,
    fc_in: HashMap<u64, u32>,
    seen: HashSet<(u16, u8)>,
    pub via: HashMap<u16, u16>,
    pub source_route: HashMap<u16, Vec<u16>>,
    pub join_key: [u8; 16],
    pub refuse_rejoin: bool,
    pub ext_pan: u64,
    pub tc_ext: u64,
    pub aps_ext: u64,
    pub depth: u8,
    pub ignore_associations: bool,
    pub ignore_rejoin: bool,
    pub legacy_update_device: bool,
    pub mute_key_exchange: bool,
    pub same_link_key: bool,
    pub update_copies: Vec<(u16, u64, u8, bool)>,
    pub statuses: Vec<(u16, u8, Vec<u8>)>,
    pub leaves: Vec<(u16, u16, u8)>,
    pub last_encrypted: Vec<u8>,
    pub zcl_rx: Vec<(u16, u16, Vec<u8>)>,
    pub zcl_rx_ep: Vec<u8>,
    pub group_rx: Vec<(u16, u16)>,
    pub zdo_rx: Vec<(u16, u16, Vec<u8>)>,
    pub nwk_cmd_rx: Vec<(u16, Vec<u8>)>,
    pub route_records: Vec<(u16, Vec<u16>)>,
    pub rejoins: Vec<(u16, bool)>,
    pub updates: Vec<(u16, u64, u8)>,
    pub ota_file: Option<OtaFile>,
    pub ota_offer_anyway: bool,
    pub ota_drop_blocks: u32,
    pub ota_block_requests: u32,
    pub ota_block_size: Option<usize>,
    pub ota_queries: u32,
    pub ota_blocks: u32,
    pub ota_end: Option<u8>,
    pub ota_ends: u32,
    pub ota_endpoint: u8,
    pub ota_hidden: bool,
    pub ota_asked_on: Option<u8>,
    pub match_requests: u32,
    pub ota_wait: (u32, u32),
    pub ota_upgrade_time: Option<u32>,
    pub aps_acks: u32,
    pub aps_encrypted_rx: u32,
    pub fragment_acks: u32,
    pub drop_fragments: u32,
    fragments: HashMap<(u16, u8), Vec<Option<Vec<u8>>>>,
    last_fragmented: Option<(u16, u16, Vec<u8>, u8, usize)>,
    pub mic_failures: u32,
    next_link_status: u32,
}

impl Coord {
    pub fn new(air: &Shared) -> Coord {
        let id = air.borrow_mut().add_port(COORD_EXT);
        {
            let mut a = air.borrow_mut();
            a.ports[id].pan = PAN;
            a.ports[id].short = 0;
        }
        let now = air.borrow().now;
        Coord {
            air: air.clone(),
            id,
            pan: PAN,
            key: NWK_KEY,
            key_seq: 0,
            fc: 1,
            aps_fc: 1,
            seq: 0x40,
            permit_join: true,
            stack_revision: 22,
            node_desc_status: 0x00,
            link_key_transport_id: 3,
            send_key_in_the_clear: false,
            restart_link_key_counter: false,
            devices: vec![],
            pending: vec![],
            key_due: vec![],
            next_short: 0x7A21,
            fc_in: HashMap::new(),
            seen: HashSet::new(),
            via: HashMap::new(),
            source_route: HashMap::new(),
            join_key: TCLK,
            refuse_rejoin: false,
            ext_pan: EXT_PAN,
            tc_ext: COORD_EXT,
            aps_ext: COORD_EXT,
            depth: 0,
            ignore_associations: false,
            ignore_rejoin: false,
            legacy_update_device: false,
            mute_key_exchange: false,
            same_link_key: false,
            update_copies: vec![],
            statuses: vec![],
            leaves: vec![],
            last_encrypted: vec![],
            zcl_rx: vec![],
            zcl_rx_ep: vec![],
            group_rx: vec![],
            zdo_rx: vec![],
            nwk_cmd_rx: vec![],
            route_records: vec![],
            rejoins: vec![],
            updates: vec![],
            ota_file: None,
            ota_offer_anyway: false,
            ota_drop_blocks: 0,
            ota_block_requests: 0,
            ota_block_size: None,
            ota_queries: 0,
            ota_blocks: 0,
            ota_end: None,
            ota_ends: 0,
            ota_endpoint: 1,
            ota_hidden: false,
            ota_asked_on: None,
            match_requests: 0,
            ota_wait: (0, 0),
            ota_upgrade_time: Some(0),
            aps_acks: 0,
            aps_encrypted_rx: 0,
            fragment_acks: 0,
            drop_fragments: 0,
            fragments: HashMap::new(),
            last_fragmented: None,
            mic_failures: 0,
            next_link_status: now + 15_000,
        }
    }

    fn next_seq(&mut self) -> u8 {
        self.seq = self.seq.wrapping_add(1);
        self.seq
    }

    pub fn device(&self, short: u16) -> Option<&Device> {
        self.devices.iter().find(|d| d.short == short)
    }

    fn device_mut(&mut self, short: u16) -> Option<&mut Device> {
        self.devices.iter_mut().find(|d| d.short == short)
    }

    fn tx(&mut self, mpdu: &[u8]) -> TxStatus {
        self.air.borrow_mut().transmit(self.id, mpdu)
    }

    fn mac_header(&mut self, frame_type: u8, dst: Addr, src: Addr) -> Buf {
        let mut f = Buf::new();
        mac::Header {
            frame_type,
            security: false,
            pending: false,
            ack_request: !matches!(dst, Addr::Short(0xFFFF) | Addr::None),
            seq: self.next_seq(),
            dst_pan: self.pan,
            dst,
            src_pan: self.pan,
            src,
        }
        .write(&mut f);
        f
    }

    pub fn set_pan(&mut self, pan: u16) {
        self.pan = pan;
        self.air.borrow_mut().ports[self.id].pan = pan;
    }

    pub fn nwk_send(&mut self, mut h: nwk::Header, payload: &[u8], mac_dst: Option<u16>) -> TxStatus {
        if h.relays.is_none() {
            if let Some(list) = self.source_route.get(&h.dst) {
                let mut relays = [0u16; nwk::MAX_RELAYS];
                relays[..list.len()].copy_from_slice(list);
                h.relays = Some((list.len() as u8, list.len() as u8 - 1, relays));
            }
        }
        let next = mac_dst.unwrap_or_else(|| match h.relays {
            Some((count, _, list)) if count > 0 => list[count as usize - 1],
            _ if nwk::is_broadcast(h.dst) => 0xFFFF,
            _ => *self.via.get(&h.dst).unwrap_or(&h.dst),
        });
        let mut f = self.mac_header(mac::FRAME_DATA, Addr::Short(next), Addr::Short(0));
        let at = f.len();
        h.write(&mut f);
        if h.security {
            let fc = self.fc;
            self.fc += 1;
            nwk::secure(&Aes128::new(&self.key), self.key_seq, COORD_EXT, fc, &mut f, at, payload);
        } else {
            f.bytes(payload);
        }
        assert!(f.ok(), "coordinator frame too long");
        self.tx(f.as_slice())
    }

    pub fn header(&mut self, frame_type: u8, dst: u16, radius: u8) -> nwk::Header {
        let seq = self.next_seq();
        let mut h = nwk::Header::new(frame_type, dst, 0x0000, radius, seq);
        h.discover = if frame_type == nwk::FRAME_DATA { 1 } else { 0 };
        if frame_type == nwk::FRAME_CMD {
            h.src_ext = Some(COORD_EXT);
        }
        h
    }

    pub fn nwk_data(&mut self, dst: u16, aps: &[u8]) -> TxStatus {
        let h = self.header(nwk::FRAME_DATA, dst, 30);
        self.nwk_send(h, aps, None)
    }

    pub fn nwk_command(&mut self, dst: u16, radius: u8, payload: &[u8]) -> TxStatus {
        let h = self.header(nwk::FRAME_CMD, dst, radius);
        self.nwk_send(h, payload, None)
    }

    pub fn many_to_one_route_request(&mut self, no_cache: bool) {
        let options = if no_cache { 2 << 3 } else { 1 << 3 };
        let id = self.next_seq();
        self.nwk_command(0xFFFC, 30, &[nwk::CMD_ROUTE_REQUEST, options, id, 0xFC, 0xFF, 0]);
    }

    fn link_status(&mut self) {
        let mut shorts: Vec<u16> = self.devices.iter().filter(|d| d.announced).map(|d| d.short).collect();
        shorts.sort();
        let mut p = vec![nwk::CMD_LINK_STATUS, shorts.len() as u8 | 0x60];
        for s in shorts {
            p.extend_from_slice(&s.to_le_bytes());
            p.push(0x11);
        }
        self.nwk_command(0xFFFC, 1, &p);
    }

    pub fn aps_frame(&mut self, fc: u8, dst_ep: u8, cluster: u16, profile: u16, src_ep: u8, payload: &[u8]) -> Vec<u8> {
        let mut f = vec![fc, dst_ep];
        f.extend_from_slice(&cluster.to_le_bytes());
        f.extend_from_slice(&profile.to_le_bytes());
        f.push(src_ep);
        f.push(self.next_seq());
        f.extend_from_slice(payload);
        f
    }

    pub fn aps_command(&mut self, link: &[u8; 16], key_id: u8, payload: &[u8]) -> Vec<u8> {
        let key = match key_id {
            0 => *link,
            2 => crypto::hmac_mmo(link, &[0x00]),
            _ => crypto::hmac_mmo(link, &[0x02]),
        };
        let counter = self.aps_fc;
        self.aps_fc += 1;
        let sc = key_id << 3 | 0x20;
        let mut f = vec![0x21, self.next_seq(), sc | 5];
        f.extend_from_slice(&counter.to_le_bytes());
        f.extend_from_slice(&self.aps_ext.to_le_bytes());
        let body_at = f.len();
        f.extend_from_slice(payload);
        f.extend_from_slice(&[0; 4]);
        let nonce = nonce(self.aps_ext, counter, sc | 5);
        let (head, body) = f.split_at_mut(body_at);
        crypto::ccm_encrypt(&Aes128::new(&key), &nonce, head, body, payload.len()).unwrap();
        f[2] = sc;
        f
    }

    pub fn transport_network_key(&mut self, short: u16, ext: u64, key: &[u8; 16], seq: u8, nwk_secured: bool) {
        let mut body = vec![0x05, 0x01];
        body.extend_from_slice(key);
        body.push(seq);
        body.extend_from_slice(&ext.to_le_bytes());
        body.extend_from_slice(&self.tc_ext.to_le_bytes());
        let link = self.device(short).map_or(self.join_key, |d| d.tclk);
        let aps = if self.send_key_in_the_clear {
            let mut clear = vec![0x01, self.next_seq()];
            clear.extend_from_slice(&body);
            clear
        } else {
            self.aps_command(&link, 2, &body)
        };
        let mut h = self.header(nwk::FRAME_DATA, short, if nwk_secured { 30 } else { 1 });
        h.security = nwk_secured;
        self.nwk_send(h, &aps, None);
    }

    pub fn switch_key(&mut self, seq: u8) {
        let ctr = self.next_seq();
        let aps = vec![0x09, ctr, 0x09, seq];
        self.nwk_data(0xFFFF, &aps);
    }

    pub fn network_key_update(&mut self, dst: u16, key: &[u8; 16], seq: u8, source: u64) {
        let mut aps = vec![if nwk::is_broadcast(dst) { 0x09 } else { 0x01 }, self.next_seq(), 0x05, 0x01];
        aps.extend_from_slice(key);
        aps.push(seq);
        aps.extend_from_slice(&[0; 8]);
        aps.extend_from_slice(&source.to_le_bytes());
        self.nwk_data(dst, &aps);
    }

    pub fn use_key(&mut self, key: &[u8; 16], seq: u8, restart: bool) {
        self.key = *key;
        self.key_seq = seq;
        if restart {
            self.fc = 0;
        }
    }

    pub fn zdo(&mut self, dst: u16, cluster: u16, payload: &[u8]) -> u8 {
        let seq = self.next_seq();
        let mut p = vec![seq];
        p.extend_from_slice(payload);
        let fc = if nwk::is_broadcast(dst) { 0x08 } else { 0x00 };
        let aps = self.aps_frame(fc, 0, cluster, 0x0000, 0, &p);
        self.nwk_data(dst, &aps);
        seq
    }

    pub fn zcl(&mut self, dst: u16, cluster: u16, frame_control: u8, cmd: u8, payload: &[u8]) -> u8 {
        let seq = self.next_seq();
        let mut p = vec![frame_control, seq, cmd];
        p.extend_from_slice(payload);
        let fc = if nwk::is_broadcast(dst) { 0x08 } else { 0x40 };
        let aps = self.aps_frame(fc, 1, cluster, 0x0104, 1, &p);
        self.nwk_data(dst, &aps);
        seq
    }

    pub fn zcl_ep(&mut self, dst: u16, ep: u8, cluster: u16, frame_control: u8, cmd: u8, payload: &[u8]) -> u8 {
        let seq = self.next_seq();
        let mut p = vec![frame_control, seq, cmd];
        p.extend_from_slice(payload);
        let fc = if nwk::is_broadcast(dst) { 0x08 } else { 0x40 };
        let aps = self.aps_frame(fc, ep, cluster, 0x0104, 1, &p);
        self.nwk_data(dst, &aps);
        seq
    }

    pub fn zcl_mfr(&mut self, dst: u16, cluster: u16, frame_control: u8, code: u16, cmd: u8, payload: &[u8]) -> u8 {
        let seq = self.next_seq();
        let mut p = vec![frame_control | 0x04];
        p.extend_from_slice(&code.to_le_bytes());
        p.extend_from_slice(&[seq, cmd]);
        p.extend_from_slice(payload);
        let aps = self.aps_frame(0x40, 1, cluster, 0x0104, 1, &p);
        self.nwk_data(dst, &aps);
        seq
    }

    pub fn last_zcl_mfr(&self, src: u16, cluster: u16, cmd: u8) -> Option<Vec<u8>> {
        self.zcl_rx
            .iter()
            .rev()
            .find(|(s, c, f)| *s == src && *c == cluster && f.first().is_some_and(|fc| fc & 0x04 != 0) && f.get(4) == Some(&cmd))
            .map(|e| e.2.clone())
    }

    pub fn last_zcl_ep(&self, src: u16, ep: u8, cluster: u16, cmd: u8) -> Option<Vec<u8>> {
        self.zcl_rx
            .iter()
            .zip(self.zcl_rx_ep.iter())
            .rev()
            .find(|((s, c, f), e)| *s == src && *c == cluster && **e == ep && f.get(2) == Some(&cmd))
            .map(|e| e.0 .2.clone())
    }

    pub fn zcl_encrypted(&mut self, dst: u16, cluster: u16, frame_control: u8, cmd: u8, payload: &[u8]) -> u8 {
        let seq = self.next_seq();
        let mut zcl = vec![frame_control, seq, cmd];
        zcl.extend_from_slice(payload);
        let link = self.device(dst).map_or(self.join_key, |d| d.tclk);
        let counter = self.aps_fc;
        self.aps_fc += 1;
        let mut f = vec![0x60, 1];
        f.extend_from_slice(&cluster.to_le_bytes());
        f.extend_from_slice(&0x0104u16.to_le_bytes());
        f.push(1);
        f.push(self.next_seq());
        let aux_at = f.len();
        f.push(0x25);
        f.extend_from_slice(&counter.to_le_bytes());
        f.extend_from_slice(&COORD_EXT.to_le_bytes());
        let body_at = f.len();
        f.extend_from_slice(&zcl);
        f.extend_from_slice(&[0; 4]);
        let (head, body) = f.split_at_mut(body_at);
        crypto::ccm_encrypt(&Aes128::new(&link), &nonce(COORD_EXT, counter, 0x25), head, body, zcl.len()).unwrap();
        f[aux_at] = 0x20;
        self.last_encrypted = f.clone();
        self.nwk_data(dst, &f);
        seq
    }

    pub fn zcl_fragmented(&mut self, dst: u16, cluster: u16, frame_control: u8, cmd: u8, payload: &[u8], block: usize) -> u8 {
        let seq = self.next_seq();
        let mut zcl = vec![frame_control, seq, cmd];
        zcl.extend_from_slice(payload);
        let counter = self.next_seq();
        self.last_fragmented = Some((dst, cluster, zcl, counter, block));
        self.repeat_fragments();
        seq
    }

    pub fn repeat_fragments(&mut self) {
        let (dst, cluster, zcl, counter, block) = self.last_fragmented.clone().expect("a fragmented transfer");
        let chunks: Vec<&[u8]> = zcl.chunks(block).collect();
        for (i, chunk) in chunks.iter().enumerate() {
            let mut aps = vec![0xC0, 1];
            aps.extend_from_slice(&cluster.to_le_bytes());
            aps.extend_from_slice(&0x0104u16.to_le_bytes());
            aps.push(1);
            aps.push(counter);
            aps.extend_from_slice(&if i == 0 { [1, chunks.len() as u8] } else { [2, i as u8] });
            aps.extend_from_slice(chunk);
            self.nwk_data(dst, &aps);
        }
    }

    pub fn zcl_group(&mut self, group: u16, cluster: u16, cmd: u8, payload: &[u8]) {
        let seq = self.next_seq();
        let mut aps = vec![0x0C];
        aps.extend_from_slice(&group.to_le_bytes());
        aps.extend_from_slice(&cluster.to_le_bytes());
        aps.extend_from_slice(&0x0104u16.to_le_bytes());
        aps.push(1);
        aps.push(self.next_seq());
        aps.extend_from_slice(&[0x01, seq, cmd]);
        aps.extend_from_slice(payload);
        self.nwk_data(0xFFFD, &aps);
    }

    pub fn last_zcl(&self, src: u16, cluster: u16, cmd: u8) -> Option<Vec<u8>> {
        self.zcl_rx.iter().rev().find(|(s, c, f)| *s == src && *c == cluster && f.get(2) == Some(&cmd)).map(|e| e.2.clone())
    }

    pub fn last_zdo(&self, src: u16, cluster: u16) -> Option<Vec<u8>> {
        self.zdo_rx.iter().rev().find(|(s, c, _)| *s == src && *c == cluster).map(|e| e.2.clone())
    }

    pub fn poll(&mut self) {
        loop {
            let next = self.air.borrow_mut().ports[self.id].queue.pop_front();
            let Some((frame, _)) = next else {
                break;
            };
            self.on_frame(&frame);
        }
        let now = self.air.borrow().now;
        while let Some(i) = self.key_due.iter().position(|e| now >= e.0) {
            let (_, short, ext) = self.key_due.remove(i);
            let key = self.key;
            let seq = self.key_seq;
            self.transport_network_key(short, ext, &key, seq, false);
        }
        if now >= self.next_link_status {
            self.next_link_status = now + 15_000;
            self.link_status();
        }
    }

    fn on_frame(&mut self, mpdu: &[u8]) {
        let Some((h, off)) = mac::Header::parse(mpdu) else {
            return;
        };
        let payload = &mpdu[off..];
        match (h.frame_type, payload.first()) {
            (mac::FRAME_CMD, Some(&mac::CMD_BEACON_REQ)) => {
                let b = mac::Beacon {
                    pan: self.pan,
                    short: 0,
                    permit_join: self.permit_join,
                    pan_coordinator: true,
                    stack_profile: 2,
                    protocol_version: 2,
                    router_capacity: true,
                    end_device_capacity: true,
                    depth: self.depth,
                    ext_pan: self.ext_pan,
                    update_id: 0,
                };
                let mut f = Buf::new();
                b.write(self.next_seq(), &mut f);
                self.tx(f.as_slice());
            }
            (mac::FRAME_CMD, Some(&mac::CMD_ASSOC_REQ)) => {
                if let (Addr::Ext(ext), true) = (h.src, self.permit_join && !self.ignore_associations) {
                    self.devices.retain(|d| d.ext != ext);
                    let short = self.next_short;
                    self.next_short += 0x1111;
                    self.devices.push(Device {
                        ext,
                        short,
                        announced: false,
                        link_status: false,
                        tclk: self.join_key,
                        tclk_new: None,
                        verified: false,
                        parent: None,
                        left: false,
                    });
                    self.fc_in.remove(&ext);
                    self.pending.push(ext);
                }
            }
            (mac::FRAME_CMD, Some(&mac::CMD_DATA_REQ)) => {
                if let Addr::Ext(ext) = h.src {
                    if let Some(i) = self.pending.iter().position(|e| *e == ext) {
                        self.pending.remove(i);
                        let short = self.devices.iter().find(|d| d.ext == ext).unwrap().short;
                        let mut f = self.mac_header(mac::FRAME_CMD, Addr::Ext(ext), Addr::Ext(COORD_EXT));
                        f.u8(mac::CMD_ASSOC_RSP);
                        f.u16(short);
                        f.u8(0);
                        self.tx(f.as_slice());
                        let now = self.air.borrow().now;
                        self.key_due.push((now + 30, short, ext));
                    }
                }
            }
            (mac::FRAME_DATA, _) => {
                if let Addr::Short(src) = h.src {
                    self.on_nwk(src, payload);
                }
            }
            _ => {}
        }
    }

    fn on_nwk(&mut self, mac_src: u16, p: &[u8]) {
        let Some((h, hl)) = nwk::Header::parse(p) else {
            return;
        };
        if !h.security {
            if h.frame_type == nwk::FRAME_CMD && h.dst == 0 && p.get(hl) == Some(&nwk::CMD_REJOIN_REQUEST) {
                if let Some(d) = h.src_ext.and_then(|e| self.devices.iter().find(|d| d.ext == e)) {
                    let (short, ext) = (d.short, d.ext);
                    self.rejoins.push((short, false));
                    if self.ignore_rejoin {
                        return;
                    }
                    let mut rh = self.header(nwk::FRAME_CMD, short, 1);
                    rh.security = false;
                    rh.dst_ext = Some(ext);
                    if self.refuse_rejoin {
                        self.nwk_send(rh, &[nwk::CMD_REJOIN_RESPONSE, 0x34, 0x12, 0x02], Some(mac_src));
                        return;
                    }
                    let s = short.to_le_bytes();
                    self.nwk_send(rh, &[nwk::CMD_REJOIN_RESPONSE, s[0], s[1], 0], Some(mac_src));
                    self.fc_in.remove(&ext);
                    let now = self.air.borrow().now;
                    self.key_due.push((now + 30, short, ext));
                }
            }
            return;
        }
        let Some(aux) = nwk::parse_aux(p, hl) else {
            return;
        };
        let mut frame = p.to_vec();
        let Some(len) = nwk::unsecure(&Aes128::new(&self.key), &aux, &mut frame, hl) else {
            self.mic_failures += 1;
            return;
        };
        if let Some(last) = self.fc_in.get(&aux.ext) {
            assert!(aux.counter > *last, "frame counter of {:#x} went back: {} after {}", aux.ext, aux.counter, last);
        }
        self.fc_in.insert(aux.ext, aux.counter);
        let payload = frame[hl + nwk::AUX_LEN..hl + nwk::AUX_LEN + len].to_vec();
        if nwk::is_broadcast(h.dst) && !self.seen.insert((h.src, h.seq)) {
            return;
        }
        if h.dst != 0 && !nwk::is_broadcast(h.dst) {
            if h.frame_type == nwk::FRAME_CMD && payload.len() >= 2 && payload[0] == nwk::CMD_LEAVE {
                self.leaves.push((h.src, h.dst, payload[1]));
            }
            return;
        }
        if h.frame_type == nwk::FRAME_CMD {
            match payload[0] {
                nwk::CMD_LINK_STATUS => {
                    if let Some(d) = self.device_mut(h.src) {
                        d.link_status = true;
                    }
                }
                nwk::CMD_ROUTE_REQUEST => {
                    let target = u16::from_le_bytes([payload[3], payload[4]]);
                    if target == 0 {
                        let o = h.src.to_le_bytes();
                        let rh = self.header(nwk::FRAME_CMD, mac_src, 30);
                        self.nwk_send(rh, &[nwk::CMD_ROUTE_REPLY, 0, payload[2], o[0], o[1], 0, 0, 1], Some(mac_src));
                    }
                }
                nwk::CMD_REJOIN_REQUEST if h.dst == 0 && self.ignore_rejoin => self.rejoins.push((h.src, true)),
                nwk::CMD_REJOIN_REQUEST if h.dst == 0 => {
                    self.rejoins.push((h.src, true));
                    let mut rh = self.header(nwk::FRAME_CMD, h.src, 1);
                    rh.dst_ext = h.src_ext;
                    let s = h.src.to_le_bytes();
                    let status = if self.refuse_rejoin { 0x01 } else { 0x00 };
                    self.nwk_send(rh, &[nwk::CMD_REJOIN_RESPONSE, s[0], s[1], status], Some(mac_src));
                }
                nwk::CMD_ROUTE_RECORD => {
                    let relays = payload[2..].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                    self.route_records.push((h.src, relays));
                }
                nwk::CMD_NETWORK_STATUS if payload.len() >= 2 => self.statuses.push((h.src, payload[1], payload[2..].to_vec())),
                _ => {}
            }
            self.nwk_cmd_rx.push((h.src, payload));
            return;
        }
        self.on_aps(h.src, &payload);
    }

    fn on_aps(&mut self, src: u16, p: &[u8]) {
        let fc = p[0];
        let decrypted;
        let p = if fc & 3 != 1 && fc & 0x20 != 0 {
            let header_len = if fc & 3 == 2 && fc & 0x10 != 0 { 2 } else if fc & 3 == 0 && fc & 0x80 != 0 { 10 } else { 8 };
            let mut r = Reader::new(&p[header_len..]);
            let sc = r.u8().unwrap();
            let counter = r.u32().unwrap();
            let ext = r.u64().unwrap();
            let aux_len = r.pos();
            let link = self.device(src).map_or(self.join_key, |d| d.tclk);
            let mut aad = p[..header_len + aux_len].to_vec();
            aad[header_len] = (sc & !7) | 5;
            let mut body = r.rest().to_vec();
            match crypto::ccm_decrypt(&Aes128::new(&link), &nonce(ext, counter, (sc & !7) | 5), &aad, &mut body) {
                Some(n) => {
                    self.aps_encrypted_rx += 1;
                    decrypted = [&p[..header_len], &body[..n]].concat();
                    &decrypted[..]
                }
                None => {
                    self.mic_failures += 1;
                    return;
                }
            }
        } else {
            p
        };
        match fc & 3 {
            2 if fc & 0x80 != 0 => self.fragment_acks += 1,
            2 => self.aps_acks += 1,
            1 => {
                let body = if fc & 0x20 != 0 {
                    let mut r = Reader::new(&p[2..]);
                    let sc = r.u8().unwrap();
                    let counter = r.u32().unwrap();
                    let ext = r.u64().unwrap();
                    let aux_len = r.pos();
                    let link = self.device(src).map_or(self.join_key, |d| d.tclk);
                    let mut aad = p[..2 + aux_len].to_vec();
                    aad[2] = (sc & !7) | 5;
                    let mut body = r.rest().to_vec();
                    let nonce = nonce(ext, counter, (sc & !7) | 5);
                    match crypto::ccm_decrypt(&Aes128::new(&link), &nonce, &aad, &mut body) {
                        Some(n) => body[..n].to_vec(),
                        None => {
                            self.mic_failures += 1;
                            return;
                        }
                    }
                } else {
                    p[2..].to_vec()
                };
                self.on_aps_command(src, fc & 0x20 != 0, &body);
            }
            _ => {
                let shifted;
                let p = if (fc >> 2) & 3 == 3 {
                    self.group_rx.push((src, u16::from_le_bytes([p[1], p[2]])));
                    shifted = [&p[..1], &p[2..]].concat();
                    &shifted[..]
                } else {
                    p
                };
                let cluster = u16::from_le_bytes([p[2], p[3]]);
                let profile = u16::from_le_bytes([p[4], p[5]]);
                let payload = if fc & 0x80 != 0 && p[8] & 3 != 0 {
                    if self.drop_fragments > 0 {
                        self.drop_fragments -= 1;
                        return;
                    }
                    let (kind, block, counter) = (p[8] & 3, p[9], p[7]);
                    let blocks = self.fragments.entry((src, counter)).or_default();
                    let index = if kind == 1 {
                        blocks.resize(block as usize, None);
                        0
                    } else {
                        block as usize
                    };
                    if index >= blocks.len() {
                        return;
                    }
                    blocks[index] = Some(p[10..].to_vec());
                    let whole = blocks.iter().all(|b| b.is_some()).then(|| blocks.iter().flatten().flatten().copied().collect::<Vec<u8>>());
                    let mut ack = vec![0x82, p[6], p[2], p[3], p[4], p[5], p[1], counter, kind, index as u8, 0xFF];
                    ack.truncate(11);
                    self.nwk_data(src, &ack);
                    match whole {
                        Some(w) => {
                            self.fragments.remove(&(src, counter));
                            w
                        }
                        None => return,
                    }
                } else {
                    p[8..].to_vec()
                };
                if profile == 0 {
                    self.on_zdo(src, cluster, &payload);
                    self.zdo_rx.push((src, cluster, payload));
                } else if cluster == 0x0019 {
                    self.ota_asked_on = Some(p[1]);
                    self.on_ota(src, &payload);
                } else {
                    self.zcl_rx.push((src, cluster, payload));
                    self.zcl_rx_ep.push(p[6]);
                }
            }
        }
    }

    pub fn remove_device(&mut self, router: u16, ext: u64) {
        let link = self.device(router).map_or(self.join_key, |d| d.tclk);
        let mut body = vec![0x07];
        body.extend_from_slice(&ext.to_le_bytes());
        let aps = self.aps_command(&link, 0, &body);
        self.nwk_data(router, &aps);
    }

    pub fn permit(&mut self, seconds: u8) {
        self.permit_join = seconds != 0;
        self.zdo(0xFFFC, 0x0036, &[seconds, 1]);
    }

    fn on_update_device(&mut self, router: u16, b: &[u8]) {
        let ext = u64::from_le_bytes(b[1..9].try_into().unwrap());
        let short = u16::from_le_bytes([b[9], b[10]]);
        self.updates.push((router, ext, b[11]));
        if b[11] == 0x02 {
            if let Some(d) = self.devices.iter_mut().find(|d| d.ext == ext) {
                d.left = true;
            }
            return;
        }
        let known = self.devices.iter().find(|d| d.ext == ext).map(|d| (d.tclk, d.verified));
        self.devices.retain(|d| d.ext != ext);
        let (link, verified) = known.unwrap_or((self.join_key, false));
        self.devices.push(Device {
            ext,
            short,
            announced: false,
            link_status: false,
            tclk: link,
            tclk_new: None,
            verified,
            parent: Some(router),
            left: false,
        });
        self.fc_in.remove(&ext);
        self.via.insert(short, router);
        if b[11] == 0x00 {
            return;
        }
        let mut body = vec![0x05, 0x01];
        body.extend_from_slice(&self.key.clone());
        body.push(self.key_seq);
        body.extend_from_slice(&ext.to_le_bytes());
        body.extend_from_slice(&COORD_EXT.to_le_bytes());
        let inner = self.aps_command(&link, 2, &body);
        let mut tunnel = vec![0x01, self.next_seq(), 0x0E];
        tunnel.extend_from_slice(&ext.to_le_bytes());
        tunnel.extend_from_slice(&inner);
        self.nwk_data(router, &tunnel);
    }

    fn on_aps_command(&mut self, src: u16, secured: bool, b: &[u8]) {
        if b[0] == 0x06 {
            self.update_copies.push((src, u64::from_le_bytes(b[1..9].try_into().unwrap()), b[11], secured));
            if secured != self.legacy_update_device {
                self.on_update_device(src, b);
            }
            return;
        }
        let Some(d) = self.device(src) else {
            return;
        };
        let (ext, link) = (d.ext, d.tclk);
        match b[0] {
            0x08 | 0x0F if self.mute_key_exchange => {}
            0x08 if secured && b[1] == 0x04 => {
                let new_key = if self.same_link_key { link } else { crypto::hmac_mmo(&TCLK, &ext.to_le_bytes()) };
                self.device_mut(src).unwrap().tclk_new = Some(new_key);
                let mut body = vec![0x05, 0x04];
                body.extend_from_slice(&new_key);
                body.extend_from_slice(&ext.to_le_bytes());
                body.extend_from_slice(&COORD_EXT.to_le_bytes());
                let key_id = self.link_key_transport_id;
                let aps = self.aps_command(&link, key_id, &body);
                self.nwk_data(src, &aps);
            }
            0x0F => {
                let Some(new_key) = self.device(src).unwrap().tclk_new else {
                    return;
                };
                let ok = b[10..26] == crypto::hmac_mmo(&new_key, &[0x03]);
                let mut body = vec![0x10, if ok { 0x00 } else { 0xAD }, b[1]];
                body.extend_from_slice(&ext.to_le_bytes());
                if ok {
                    let d = self.device_mut(src).unwrap();
                    d.tclk = new_key;
                    d.verified = true;
                    if self.restart_link_key_counter {
                        self.aps_fc = 0;
                    }
                }
                let aps = self.aps_command(&new_key, 0, &body);
                self.nwk_data(src, &aps);
            }
            _ => {}
        }
    }

    fn on_ota(&mut self, src: u16, p: &[u8]) {
        let (seq, cmd, b) = (p[1], p[2], &p[3..]);
        let mut rsp = vec![0x19, seq];
        match cmd {
            0x01 => {
                self.ota_queries += 1;
                assert!(self.ota_queries < 1_000, "a flood of OTA queries");
                let (manufacturer, image_type) = (u16::from_le_bytes([b[1], b[2]]), u16::from_le_bytes([b[3], b[4]]));
                rsp.push(0x02);
                match &self.ota_file {
                    Some(f) if self.ota_offer_anyway || (f.manufacturer == manufacturer && f.image_type == image_type) => {
                        rsp.push(0x00);
                        rsp.extend_from_slice(&f.manufacturer.to_le_bytes());
                        rsp.extend_from_slice(&f.image_type.to_le_bytes());
                        rsp.extend_from_slice(&f.version.to_le_bytes());
                        rsp.extend_from_slice(&(f.data.len() as u32).to_le_bytes());
                    }
                    _ => rsp.push(0x98),
                }
            }
            0x03 => {
                self.ota_block_requests += 1;
                if self.ota_drop_blocks > 0 {
                    self.ota_drop_blocks -= 1;
                    return;
                }
                self.ota_blocks += 1;
                let offset = u32::from_le_bytes([b[9], b[10], b[11], b[12]]) as usize;
                let max = self.ota_block_size.unwrap_or(b[13] as usize);
                rsp.push(0x05);
                if self.ota_wait.0 > 0 {
                    self.ota_wait.0 -= 1;
                    rsp.push(0x97);
                    rsp.extend_from_slice(&1000u32.to_le_bytes());
                    rsp.extend_from_slice(&(1000 + self.ota_wait.1).to_le_bytes());
                    let aps = self.aps_frame(0x00, 1, 0x0019, 0x0104, 1, &rsp);
                    self.nwk_data(src, &aps);
                    return;
                }
                match &self.ota_file {
                    Some(f) if offset < f.data.len() => {
                        let chunk = &f.data[offset..(offset + max).min(f.data.len())];
                        rsp.push(0x00);
                        rsp.extend_from_slice(&b[1..9]);
                        rsp.extend_from_slice(&(offset as u32).to_le_bytes());
                        rsp.push(chunk.len() as u8);
                        rsp.extend_from_slice(chunk);
                    }
                    _ => rsp.push(0x95),
                }
            }
            0x06 => {
                self.ota_end = Some(b[0]);
                if b[0] != 0 {
                    return;
                }
                self.ota_ends += 1;
                let Some(upgrade_time) = self.ota_upgrade_time else {
                    return;
                };
                rsp.push(0x07);
                rsp.extend_from_slice(&b[1..9]);
                rsp.extend_from_slice(&[0; 4]);
                rsp.extend_from_slice(&upgrade_time.to_le_bytes());
            }
            _ => return,
        }
        if rsp.len() > 70 {
            self.zcl_fragmented(src, 0x0019, 0x19, rsp[2], &rsp[3..], 50);
            return;
        }
        let aps = self.aps_frame(0x00, 1, 0x0019, 0x0104, 1, &rsp);
        self.nwk_data(src, &aps);
    }

    pub fn gp_command(&mut self, cmd: u8, payload: &[u8]) {
        let seq = self.next_seq();
        let mut zcl = vec![0x19, seq, cmd];
        zcl.extend_from_slice(payload);
        let aps = self.aps_frame(0x08, 242, 0x0021, 0xA1E0, 242, &zcl);
        self.nwk_data(0xFFFD, &aps);
    }

    pub fn gp_commissioning_mode(&mut self, enter: bool, unicast: bool) {
        let options = if enter { 0x01 | 0x04 | (unicast as u8) << 5 } else { 0x00 };
        self.gp_command(0x02, &[options]);
    }

    pub fn gp_unicast(&mut self, dst: u16, frame_control: u8, cmd: u8, payload: &[u8]) -> u8 {
        let seq = self.next_seq();
        let mut zcl = vec![frame_control, seq, cmd];
        zcl.extend_from_slice(payload);
        let aps = self.aps_frame(0x00, 242, 0x0021, 0xA1E0, 242, &zcl);
        self.nwk_data(dst, &aps);
        seq
    }

    pub fn gp_pairing(&mut self, gpd: u32, add: bool, mode: u8, group: u16, security: u8, key: Option<[u8; 16]>, counter: u32) {
        let mut pairing = Pairing::new(gpd, mode);
        pairing.remove_gpd = !add;
        pairing.group = group;
        pairing.security = security;
        pairing.key_type = if key.is_some() { 4 } else { 0 };
        pairing.key = key;
        pairing.counter = Some(counter);
        self.gp_pair(&pairing);
    }

    pub fn gp_pair(&mut self, pairing: &Pairing) {
        self.gp_command(0x01, &pairing.payload());
    }

    pub fn gp_response(&mut self, temp_master: u16, channel: u8, gpd: u32, cmd: u8, payload: &[u8]) {
        self.gp_response_to(temp_master, channel, None, gpd, cmd, payload);
    }

    pub fn gp_response_to(&mut self, temp_master: u16, channel: u8, ieee: Option<(u64, u8)>, gpd: u32, cmd: u8, payload: &[u8]) {
        let mut p = vec![if ieee.is_some() { 0x02 } else { 0x00 }];
        p.extend_from_slice(&temp_master.to_le_bytes());
        p.push(channel - 11);
        match ieee {
            Some((ieee, endpoint)) => {
                p.extend_from_slice(&ieee.to_le_bytes());
                p.push(endpoint);
            }
            None => p.extend_from_slice(&gpd.to_le_bytes()),
        }
        p.push(cmd);
        p.push(payload.len() as u8);
        p.extend_from_slice(payload);
        self.gp_command(0x06, &p);
    }

    pub fn ota_notify(&mut self, dst: u16) {
        let seq = self.next_seq();
        let aps = self.aps_frame(if nwk::is_broadcast(dst) { 0x08 } else { 0x00 }, 1, 0x0019, 0x0104, self.ota_endpoint, &[0x19, seq, 0x00, 0x00, 100]);
        self.nwk_data(dst, &aps);
    }

    pub fn ota_upgrade_end_response(&mut self, dst: u16, manufacturer: u16, image_type: u16, version: u32, upgrade_time: u32) {
        let seq = self.next_seq();
        let mut zcl = vec![0x19, seq, 0x07];
        zcl.extend_from_slice(&manufacturer.to_le_bytes());
        zcl.extend_from_slice(&image_type.to_le_bytes());
        zcl.extend_from_slice(&version.to_le_bytes());
        zcl.extend_from_slice(&[0; 4]);
        zcl.extend_from_slice(&upgrade_time.to_le_bytes());
        let aps = self.aps_frame(if nwk::is_broadcast(dst) { 0x08 } else { 0x00 }, 1, 0x0019, 0x0104, self.ota_endpoint, &zcl);
        self.nwk_data(dst, &aps);
    }

    fn on_zdo(&mut self, src: u16, cluster: u16, p: &[u8]) {
        match cluster {
            0x0006 => {
                self.match_requests += 1;
                assert!(self.match_requests < 1_000, "a flood of match descriptor requests");
                if self.ota_hidden {
                    return;
                }
                let rsp = vec![p[0], 0x00, 0x00, 0x00, 1, self.ota_endpoint];
                let aps = self.aps_frame(0x00, 0, 0x8006, 0x0000, 0, &rsp);
                self.nwk_data(src, &aps);
            }
            0x0013 => {
                let short = u16::from_le_bytes([p[1], p[2]]);
                if let Some(ext) = p.get(3..11) {
                    let ext = u64::from_le_bytes(ext.try_into().unwrap());
                    if let Some(d) = self.devices.iter_mut().find(|d| d.ext == ext) {
                        d.short = short;
                    }
                }
                if let Some(d) = self.device_mut(short) {
                    d.announced = true;
                }
            }
            0x0002 => {
                let mut rsp = vec![p[0], self.node_desc_status, 0x00, 0x00, 0x00, 0x40, 0x8F, 0x41, 0x11, 0x50, 0xA0, 0x00];
                rsp.extend_from_slice(&(0x0001 | self.stack_revision << 9).to_le_bytes());
                rsp.extend_from_slice(&[0xA0, 0x00, 0x00]);
                let aps = self.aps_frame(0x00, 0, 0x8002, 0x0000, 0, &rsp);
                self.nwk_data(src, &aps);
            }
            _ => {}
        }
    }
}

pub fn nonce(ext: u64, counter: u32, sc: u8) -> [u8; 13] {
    let mut n = [0u8; 13];
    n[..8].copy_from_slice(&ext.to_le_bytes());
    n[8..12].copy_from_slice(&counter.to_le_bytes());
    n[12] = sc;
    n
}

pub struct Zed {
    pub air: Shared,
    pub id: usize,
    pub ext: u64,
    pub short: u16,
    pub parent: u16,
    pub pan: u16,
    pub sleepy: bool,
    pub key: Option<[u8; 16]>,
    pub key_seq: u8,
    pub fc: u32,
    pub end_device_initiator: bool,
    seq: u8,
    pub rx: Vec<(nwk::Header, Vec<u8>)>,
    pub mac_rx: Vec<(mac::Header, Vec<u8>)>,
}

impl Zed {
    pub fn new(air: &Shared, sleepy: bool) -> Zed {
        let ext = 0x0015_8D00_0000_0000 + air.borrow().ports.len() as u64;
        let id = air.borrow_mut().add_port(ext);
        air.borrow_mut().ports[id].powered = !sleepy;
        Zed {
            air: air.clone(),
            id,
            ext,
            short: 0xFFFF,
            parent: 0xFFFF,
            pan: PAN,
            sleepy,
            key: None,
            key_seq: 0,
            fc: 100,
            end_device_initiator: false,
            seq: 0,
            rx: vec![],
            mac_rx: vec![],
        }
    }

    pub fn capability(&self) -> u8 {
        if self.sleepy {
            0x80
        } else {
            0x88
        }
    }

    fn next_seq(&mut self) -> u8 {
        self.seq = self.seq.wrapping_add(1);
        self.seq
    }

    fn listen(&self, on: bool) {
        if self.sleepy {
            self.air.borrow_mut().ports[self.id].powered = on;
        }
    }

    fn tx(&mut self, mpdu: &[u8]) -> TxStatus {
        self.air.borrow_mut().transmit(self.id, mpdu)
    }

    fn mac_command(&mut self, src: Addr, src_pan: u16, payload: &[u8]) -> TxStatus {
        let mut f = Buf::new();
        mac::Header {
            frame_type: mac::FRAME_CMD,
            security: false,
            pending: false,
            ack_request: true,
            seq: self.next_seq(),
            dst_pan: self.pan,
            dst: Addr::Short(self.parent),
            src_pan,
            src,
        }
        .write(&mut f);
        f.bytes(payload);
        self.tx(f.as_slice())
    }

    pub fn orphan_scan(&mut self, net: &mut Net) -> Option<(u16, u16, u8, u16)> {
        self.listen(true);
        let mut f = Buf::new();
        mac::Header {
            frame_type: mac::FRAME_CMD,
            security: false,
            pending: false,
            ack_request: false,
            seq: self.next_seq(),
            dst_pan: 0xFFFF,
            dst: Addr::Short(0xFFFF),
            src_pan: 0xFFFF,
            src: Addr::Ext(self.ext),
        }
        .write(&mut f);
        f.u8(mac::CMD_ORPHAN);
        self.tx(f.as_slice());
        net.run(50);
        self.drain();
        self.listen(false);
        let ext = self.ext;
        self.mac_rx
            .iter()
            .rev()
            .find(|(h, p)| h.frame_type == mac::FRAME_CMD && h.dst == Addr::Ext(ext) && p.first() == Some(&mac::CMD_REALIGNMENT) && p.len() >= 8)
            .map(|(_, p)| (u16::from_le_bytes([p[1], p[2]]), u16::from_le_bytes([p[3], p[4]]), p[5], u16::from_le_bytes([p[6], p[7]])))
    }

    pub fn drain(&mut self) {
        loop {
            let next = self.air.borrow_mut().ports[self.id].queue.pop_front();
            let Some((mpdu, _)) = next else {
                break;
            };
            let Some((mh, off)) = mac::Header::parse(&mpdu) else {
                continue;
            };
            let payload = &mpdu[off..];
            if mh.frame_type != mac::FRAME_DATA {
                self.mac_rx.push((mh, payload.to_vec()));
                continue;
            }
            let Some((h, hl)) = nwk::Header::parse(payload) else {
                continue;
            };
            if !h.security {
                self.rx.push((h, payload[hl..].to_vec()));
                continue;
            }
            let (Some(aux), Some(key)) = (nwk::parse_aux(payload, hl), self.key) else {
                continue;
            };
            let mut frame = payload.to_vec();
            if let Some(len) = nwk::unsecure(&Aes128::new(&key), &aux, &mut frame, hl) {
                self.rx.push((h, frame[hl + nwk::AUX_LEN..hl + nwk::AUX_LEN + len].to_vec()));
            }
        }
    }

    pub fn poll(&mut self, net: &mut Net, wait: u32) -> TxStatus {
        self.listen(true);
        let src = if self.short == 0xFFFF { Addr::Ext(self.ext) } else { Addr::Short(self.short) };
        let status = self.mac_command(src, self.pan, &[mac::CMD_DATA_REQ]);
        net.run(wait);
        self.drain();
        self.listen(false);
        status
    }

    pub fn join(&mut self, net: &mut Net, parent: u16) -> bool {
        self.parent = parent;
        self.air.borrow_mut().ports[self.id].pan = self.pan;
        self.listen(true);
        let cap = self.capability();
        let status = self.mac_command(Addr::Ext(self.ext), 0xFFFF, &[mac::CMD_ASSOC_REQ, cap]);
        self.listen(false);
        if status != TxStatus::Ok {
            return false;
        }
        net.run(500);
        self.listen(true);
        self.mac_command(Addr::Ext(self.ext), self.pan, &[mac::CMD_DATA_REQ]);
        let mut response = None;
        for _ in 0..20 {
            net.run(1);
            self.drain();
            response = self.mac_rx.iter().find(|(_, p)| p.first() == Some(&mac::CMD_ASSOC_RSP)).cloned();
            if response.is_some() {
                break;
            }
        }
        self.listen(false);
        let Some((_, rsp)) = response else {
            return false;
        };
        if rsp[3] != 0 {
            return false;
        }
        self.short = u16::from_le_bytes([rsp[1], rsp[2]]);
        self.air.borrow_mut().ports[self.id].short = self.short;
        for _ in 0..20 {
            net.run(100);
            self.poll(net, 20);
            if self.take_key() {
                self.announce();
                return true;
            }
        }
        false
    }

    fn take_key(&mut self) -> bool {
        let found = self.rx.iter().find(|(h, p)| !h.security && p.first() == Some(&0x21)).cloned();
        let Some((_, aps)) = found else {
            return false;
        };
        let mut r = Reader::new(&aps[2..]);
        let sc = r.u8().unwrap();
        let counter = r.u32().unwrap();
        let ext = r.u64().unwrap();
        let aux_len = r.pos();
        let mut aad = aps[..2 + aux_len].to_vec();
        aad[2] = (sc & !7) | 5;
        let mut body = r.rest().to_vec();
        let cipher = body.clone();
        let mut n = None;
        for link in [TCLK, DISTRIBUTED_LINK_KEY] {
            body = cipher.clone();
            let key = crypto::hmac_mmo(&link, &[0x00]);
            n = crypto::ccm_decrypt(&Aes128::new(&key), &nonce(ext, counter, (sc & !7) | 5), &aad, &mut body);
            if n.is_some() {
                break;
            }
        }
        let n = n.expect("a Transport Key that decrypts");
        assert_eq!(&body[..2], &[0x05, 0x01]);
        assert_eq!(n, 35);
        let mut k = [0u8; 16];
        k.copy_from_slice(&body[2..18]);
        self.key = Some(k);
        self.key_seq = body[18];
        self.rx.clear();
        true
    }

    pub fn rejoin_without_key(&mut self, net: &mut Net, parent: u16) -> bool {
        self.parent = parent;
        self.key = None;
        self.rx.clear();
        let mut h = self.header(nwk::FRAME_CMD, parent);
        h.radius = 1;
        h.security = false;
        let mut f = Buf::new();
        mac::Header {
            frame_type: mac::FRAME_DATA,
            security: false,
            pending: false,
            ack_request: true,
            seq: self.next_seq(),
            dst_pan: self.pan,
            dst: Addr::Short(parent),
            src_pan: self.pan,
            src: Addr::Short(self.short),
        }
        .write(&mut f);
        h.write(&mut f);
        f.bytes(&[nwk::CMD_REJOIN_REQUEST, self.capability()]);
        self.listen(true);
        let status = self.tx(f.as_slice());
        self.listen(false);
        if status != TxStatus::Ok {
            return false;
        }
        let mut readmitted = false;
        for _ in 0..30 {
            net.run(100);
            self.poll(net, 20);
            readmitted |= self.rx.iter().any(|(h, p)| !h.security && p.first() == Some(&nwk::CMD_REJOIN_RESPONSE) && p[3] == 0);
            if readmitted && self.take_key() {
                return true;
            }
        }
        false
    }

    pub fn router(air: &Shared, short: u16, via: u16) -> Zed {
        let mut z = Zed::new(air, false);
        z.short = short;
        z.parent = via;
        z.key = Some(NWK_KEY);
        let mut a = air.borrow_mut();
        a.ports[z.id].pan = PAN;
        a.ports[z.id].short = short;
        drop(a);
        z
    }

    pub fn link_status(&mut self, neighbors: &[(u16, u8)]) {
        let mut list = neighbors.to_vec();
        list.sort();
        let mut p = vec![nwk::CMD_LINK_STATUS, list.len() as u8 | 0x60];
        for (short, cost) in list {
            p.extend_from_slice(&short.to_le_bytes());
            p.push(cost | cost << 4);
        }
        let mut h = self.header(nwk::FRAME_CMD, 0xFFFC);
        h.radius = 1;
        self.mac_send(0xFFFF, h, &p);
    }

    pub fn nwk_send(&mut self, mut h: nwk::Header, payload: &[u8]) -> TxStatus {
        h.src = self.short;
        self.mac_send(self.parent, h, payload)
    }

    pub fn mac_send(&mut self, mac_dst: u16, h: nwk::Header, payload: &[u8]) -> TxStatus {
        let mut f = Buf::new();
        mac::Header {
            frame_type: mac::FRAME_DATA,
            security: false,
            pending: false,
            ack_request: mac_dst != 0xFFFF,
            seq: self.next_seq(),
            dst_pan: self.pan,
            dst: Addr::Short(mac_dst),
            src_pan: self.pan,
            src: Addr::Short(self.short),
        }
        .write(&mut f);
        let at = f.len();
        h.write(&mut f);
        let fc = self.fc;
        self.fc += 1;
        nwk::secure(&Aes128::new(&self.key.unwrap()), self.key_seq, self.ext, fc, &mut f, at, payload);
        self.listen(true);
        let status = self.tx(f.as_slice());
        self.listen(false);
        status
    }

    pub fn header(&mut self, frame_type: u8, dst: u16) -> nwk::Header {
        let seq = self.next_seq();
        let mut h = nwk::Header::new(frame_type, dst, self.short, 30, seq);
        h.end_device_initiator = self.end_device_initiator;
        if frame_type == nwk::FRAME_CMD {
            h.src_ext = Some(self.ext);
        } else {
            h.discover = 1;
        }
        h
    }

    pub fn announce(&mut self) {
        let mut p = vec![self.next_seq()];
        p.extend_from_slice(&self.short.to_le_bytes());
        p.extend_from_slice(&self.ext.to_le_bytes());
        p.push(self.capability());
        let mut aps = vec![0x08, 0x00, 0x13, 0x00, 0x00, 0x00, 0x00, self.next_seq()];
        aps.extend_from_slice(&p);
        let h = self.header(nwk::FRAME_DATA, 0xFFFD);
        self.nwk_send(h, &aps);
    }

    pub fn zcl_to_coordinator(&mut self, cluster: u16, zcl: &[u8]) -> TxStatus {
        let mut aps = vec![0x00, 0x01];
        aps.extend_from_slice(&cluster.to_le_bytes());
        aps.extend_from_slice(&0x0104u16.to_le_bytes());
        aps.push(1);
        aps.push(self.next_seq());
        aps.extend_from_slice(zcl);
        let h = self.header(nwk::FRAME_DATA, 0x0000);
        self.nwk_send(h, &aps)
    }
}

#[derive(Clone)]
pub struct Pairing {
    pub gpd: u32,
    pub ieee: Option<(u64, u8)>,
    pub add_sink: bool,
    pub remove_gpd: bool,
    pub mode: u8,
    pub sink: (u64, u16),
    pub group: u16,
    pub fixed: bool,
    pub seq_cap: bool,
    pub security: u8,
    pub key_type: u8,
    pub counter: Option<u32>,
    pub key: Option<[u8; 16]>,
    pub alias: Option<u16>,
    pub radius: Option<u8>,
}

impl Pairing {
    pub fn new(gpd: u32, mode: u8) -> Pairing {
        Pairing {
            gpd,
            ieee: None,
            add_sink: true,
            remove_gpd: false,
            mode,
            sink: (COORD_EXT, 0x0000),
            group: 0,
            fixed: false,
            seq_cap: false,
            security: 0,
            key_type: 0,
            counter: None,
            key: None,
            alias: None,
            radius: None,
        }
    }

    pub fn secured(mut self, key_type: u8, key: Option<[u8; 16]>, counter: u32) -> Pairing {
        self.security = 3;
        self.key_type = key_type;
        self.key = key;
        self.counter = Some(counter);
        self
    }

    pub fn payload(&self) -> Vec<u8> {
        let app: u32 = if self.ieee.is_some() { 2 } else { 0 };
        let add = self.add_sink && !self.remove_gpd;
        let options: u32 = app
            | (add as u32) << 3
            | (self.remove_gpd as u32) << 4
            | (self.mode as u32) << 5
            | (self.fixed as u32) << 7
            | (self.seq_cap as u32) << 8
            | (self.security as u32) << 9
            | (self.key_type as u32) << 11
            | ((add && self.counter.is_some()) as u32) << 14
            | ((add && self.key.is_some()) as u32) << 15
            | ((add && self.alias.is_some()) as u32) << 16
            | ((add && self.radius.is_some()) as u32) << 17;
        let mut p = options.to_le_bytes()[..3].to_vec();
        match self.ieee {
            Some((ieee, endpoint)) => {
                p.extend_from_slice(&ieee.to_le_bytes());
                p.push(endpoint);
            }
            None => p.extend_from_slice(&self.gpd.to_le_bytes()),
        }
        if self.remove_gpd {
            return p;
        }
        if self.mode == 3 {
            p.extend_from_slice(&self.sink.0.to_le_bytes());
            p.extend_from_slice(&self.sink.1.to_le_bytes());
        } else {
            p.extend_from_slice(&self.group.to_le_bytes());
        }
        if self.add_sink {
            p.push(0x02);
            if let Some(counter) = self.counter {
                p.extend_from_slice(&counter.to_le_bytes());
            }
            if let Some(key) = self.key {
                p.extend_from_slice(&key);
            }
            if let Some(alias) = self.alias {
                p.extend_from_slice(&alias.to_le_bytes());
            }
            if let Some(radius) = self.radius {
                p.push(radius);
            }
        }
        p
    }
}

pub struct Gpd {
    pub air: Shared,
    pub port: usize,
    pub id: u32,
    pub ieee: Option<(u64, u8)>,
    pub key: Option<[u8; 16]>,
    pub level: u8,
    pub individual: bool,
    pub auto_commissioning: bool,
    pub counter: u32,
    seq: u8,
}

impl Gpd {
    pub fn new(air: &Shared, id: u32, key: Option<[u8; 16]>) -> Gpd {
        let port = air.borrow_mut().add_port(0);
        air.borrow_mut().ports[port].powered = false;
        Gpd { air: air.clone(), port, id, ieee: None, key, level: 3, individual: true, auto_commissioning: false, counter: 100, seq: 0x30 }
    }

    pub fn with_ieee(air: &Shared, ieee: u64, endpoint: u8, key: Option<[u8; 16]>) -> Gpd {
        let mut gpd = Gpd::new(air, 0, key);
        gpd.ieee = Some((ieee, endpoint));
        gpd
    }

    fn headers(&self, rx_after_tx: bool) -> (Vec<u8>, Vec<u8>) {
        let secured = self.key.is_some();
        let auto = (self.auto_commissioning as u8) << 6;
        let ext = if secured { self.level << 3 | (self.individual as u8) << 5 } else { 0 } | (rx_after_tx as u8) << 6;
        match self.ieee {
            Some((ieee, endpoint)) => {
                let mut mac = vec![0x41, 0xC8, self.seq, 0xFF, 0xFF, 0xFF, 0xFF];
                mac.extend_from_slice(&ieee.to_le_bytes());
                let mut nwk = vec![0x8C | auto, 0x02 | ext, endpoint];
                if secured {
                    nwk.extend_from_slice(&self.counter.to_le_bytes());
                }
                (mac, nwk)
            }
            None => {
                let mac = vec![0x01, 0x08, self.seq, 0xFF, 0xFF, 0xFF, 0xFF];
                let mut nwk = if ext == 0 { vec![0x0C | auto] } else { vec![0x8C | auto, ext] };
                nwk.extend_from_slice(&self.id.to_le_bytes());
                if secured {
                    nwk.extend_from_slice(&self.counter.to_le_bytes());
                }
                (mac, nwk)
            }
        }
    }

    fn frame(&self, cmd: u8, payload: &[u8], rx_after_tx: bool) -> Vec<u8> {
        let (mut f, header) = self.headers(rx_after_tx);
        let mut body = vec![cmd];
        body.extend_from_slice(payload);
        f.extend_from_slice(&header);
        if let Some(key) = self.key {
            let mut nonce = [0u8; 13];
            match self.ieee {
                Some((ieee, _)) => nonce[..8].copy_from_slice(&ieee.to_le_bytes()),
                None => {
                    nonce[..4].copy_from_slice(&self.id.to_le_bytes());
                    nonce[4..8].copy_from_slice(&self.id.to_le_bytes());
                }
            }
            nonce[8..12].copy_from_slice(&self.counter.to_le_bytes());
            nonce[12] = 0x05;
            let aes = Aes128::new(&key);
            if self.level == 3 {
                let n = body.len();
                body.extend_from_slice(&[0; 4]);
                crypto::ccm_encrypt(&aes, &nonce, &header, &mut body, n).unwrap();
            } else {
                let mut aad = header.clone();
                aad.extend_from_slice(&body);
                let mut mic = [0u8; 4];
                crypto::ccm_encrypt(&aes, &nonce, &aad, &mut mic, 0).unwrap();
                body.extend_from_slice(&mic);
            }
        }
        f.extend_from_slice(&body);
        f
    }

    pub fn seq(&self) -> u8 {
        self.seq
    }

    pub fn send_and_listen(&mut self, cmd: u8, payload: &[u8]) {
        self.seq = self.seq.wrapping_add(1);
        let key = self.key.take();
        let f = self.frame(cmd, payload, true);
        self.key = key;
        self.air.borrow_mut().transmit(self.port, &f);
    }

    pub fn send_secured_and_listen(&mut self, cmd: u8, payload: &[u8]) {
        self.seq = self.seq.wrapping_add(1);
        self.counter += 1;
        let f = self.frame(cmd, payload, true);
        self.air.borrow_mut().transmit(self.port, &f);
    }

    pub fn send_raw(&mut self, nwk: &[u8]) {
        self.seq = self.seq.wrapping_add(1);
        let mut f = vec![0x01, 0x08, self.seq, 0xFF, 0xFF, 0xFF, 0xFF];
        f.extend_from_slice(nwk);
        self.air.borrow_mut().transmit(self.port, &f);
    }

    pub fn channel_request(&mut self) {
        self.seq = self.seq.wrapping_add(1);
        let f = vec![0x01, 0x08, self.seq, 0xFF, 0xFF, 0xFF, 0xFF, 0x0D, 0xE3, 0x85];
        self.air.borrow_mut().transmit(self.port, &f);
    }

    pub fn send(&mut self, cmd: u8, payload: &[u8]) {
        self.seq = self.seq.wrapping_add(1);
        self.counter += 1;
        let f = self.frame(cmd, payload, false);
        for _ in 0..3 {
            self.air.borrow_mut().transmit(self.port, &f);
        }
    }
}

pub struct Net {
    pub air: Shared,
    pub coord: Coord,
    pub nodes: Vec<Node>,
    pub identity: Identity,
}

impl Net {
    pub fn new() -> Net {
        let air = Air::new();
        let coord = Coord::new(&air);
        Net { air, coord, nodes: vec![], identity: IDENTITY }
    }

    pub fn add_node(&mut self) -> usize {
        let ext = (BULB_OUI << 40) + self.nodes.len() as u64 + 1;
        let id = self.air.borrow_mut().add_port(ext);
        let stack = Stack::new(SimHal { air: self.air.clone(), id, bx: std::boxed::Box::new(zigbee::BlackBox::EMPTY) }, self.identity);
        self.nodes.push(Node { id, stack, on: true, level: 254, color_temp: bulb_core::CT_DEFAULT, color_mode: bulb_core::colour::ColorMode::Temperature, hue: 170, sat: 254, x: 0, y: 0, commands: vec![], command_frames: vec![] });
        self.nodes.last_mut().unwrap().report();
        self.nodes.len() - 1
    }

    pub fn power_cycle(&mut self, n: usize) {
        let id = self.nodes[n].id;
        self.air.borrow_mut().ports[id].queue.clear();
        let stack = Stack::new(SimHal { air: self.air.clone(), id, bx: std::boxed::Box::new(zigbee::BlackBox::EMPTY) }, self.identity);
        self.nodes[n].stack = stack;
        self.nodes[n].report();
    }

    pub fn set_power(&mut self, n: usize, on: bool) {
        let id = self.nodes[n].id;
        self.air.borrow_mut().ports[id].powered = on;
    }

    pub fn link(&mut self, a: usize, b: usize, lqi: u8) {
        self.air.borrow_mut().link(a, b, lqi);
    }

    pub fn lossy(&mut self, a: usize, b: usize, percent: u8) {
        self.air.borrow_mut().lossy(a, b, percent);
    }

    pub fn now(&self) -> u32 {
        self.air.borrow().now
    }

    pub fn run(&mut self, ms: u32) {
        for _ in 0..ms {
            self.air.borrow_mut().now += 1;
            self.coord.poll();
            for node in self.nodes.iter_mut() {
                let powered = node.stack.hal.air.borrow().ports[node.id].powered;
                if !powered {
                    continue;
                }
                while let Some(c) = node.stack.poll() {
                    node.apply(c);
                }
            }
        }
    }

    pub fn run_until(&mut self, max_ms: u32, what: &str, mut done: impl FnMut(&Net) -> bool) {
        for _ in 0..max_ms {
            if done(self) {
                return;
            }
            self.run(1);
        }
        assert!(done(self), "timed out waiting for: {what}");
    }

    pub fn join_node(&mut self) -> usize {
        let n = self.add_node();
        let ext = self.air.borrow().ports[self.nodes[n].id].ext;
        self.run_until(20_000, "node joined", |net| {
            net.nodes[n].stack.state() == State::Up
                && net.coord.devices.iter().any(|d| d.ext == ext && d.announced && (d.verified || net.coord.stack_revision < 21))
        });
        n
    }

    pub fn sent_by(&self, port: usize, since: u32) -> Vec<Vec<u8>> {
        self.air.borrow().log.iter().filter(|e| e.1 == port && e.0 >= since).map(|e| e.2.clone()).collect()
    }

    pub fn nwk_sent_by(&self, port: usize, since: u32) -> Vec<(mac::Header, nwk::Header, Vec<u8>)> {
        self.timed_nwk_sent_by(port, since).into_iter().map(|e| (e.1, e.2, e.3)).collect()
    }

    pub fn timed_nwk_sent_by(&self, port: usize, since: u32) -> Vec<(u32, mac::Header, nwk::Header, Vec<u8>)> {
        let air = self.air.borrow();
        let mut out = vec![];
        for (at, from, mpdu) in air.log.iter().filter(|e| e.1 == port && e.0 >= since) {
            let _ = from;
            let Some((mh, off)) = mac::Header::parse(mpdu) else {
                continue;
            };
            if mh.frame_type != mac::FRAME_DATA {
                continue;
            }
            let payload = &mpdu[off..];
            let Some((h, hl)) = nwk::Header::parse(payload) else {
                continue;
            };
            if !h.security {
                out.push((*at, mh, h, payload[hl..].to_vec()));
                continue;
            }
            let Some(aux) = nwk::parse_aux(payload, hl) else {
                continue;
            };
            let mut frame = payload.to_vec();
            if let Some(len) = nwk::unsecure(&Aes128::new(&self.coord.key), &aux, &mut frame, hl) {
                out.push((*at, mh, h, frame[hl + nwk::AUX_LEN..hl + nwk::AUX_LEN + len].to_vec()));
            }
        }
        out
    }
}
