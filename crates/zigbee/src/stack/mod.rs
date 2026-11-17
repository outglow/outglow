mod aps;
pub mod blackbox;
mod diag;
mod frag;
mod gp;
mod join;
#[cfg(test)]
mod link_eval;
mod net;
mod nv;
mod objects;
mod ota;
mod parent;
#[cfg(feature = "sense")]
mod sense;
mod touchlink;
mod zcl;
mod zdo;

use bulb_core::command::{Command, StateReport};

pub use diag::{ev, LoggedEvent, MacCounters, NeighborCounts, DIAG_COUNTERS_MAX, EVENTS_LEN as DIAG_EVENTS_LEN};
pub use ota::OtaState;
pub use zcl::LightSettings;
#[cfg(feature = "sense")]
pub use sense::{DEFAULT_OFF_S as SENSE_DEFAULT_OFF_S, DEFAULT_RATE as SENSE_DEFAULT_RATE, DEFAULT_S as SENSE_DEFAULT_S, ON_RATE as SENSE_ON_RATE};

use crate::aes::Aes128;
use crate::hal::Hal;
use crate::nwk;
use crate::util::due;

pub(crate) const NEIGHBORS: usize = 16;
pub(crate) const ROUTES: usize = 16;
pub(crate) const DISCOVERIES: usize = 6;
pub(crate) const BTT: usize = 80;
pub(crate) const HELD: usize = 10;
pub(crate) const BCAST: usize = 8;
pub(crate) const APS_DUPS: usize = 16;
pub(crate) const GROUPS: usize = 8;
pub(crate) const BINDINGS: usize = 8;
pub(crate) const ADDR_MAP: usize = 4;
pub(crate) const CMDS: usize = 6;
pub const CHILDREN: usize = 32;
pub const INDIRECT: usize = 24;
pub const INDIRECT_PER_CHILD: usize = 4;
pub const PENDING_SHORTS: usize = CHILDREN + INDIRECT;
pub const RETAIN_MAX: usize = diag::SNAPSHOT_LEN + diag::EVENTS_LEN;
pub(crate) const MAX_NWK_PAYLOAD: usize = 90;

pub(crate) const LINK_STATUS_MS: u32 = 15_000;
pub(crate) const AGE_LIMIT: u8 = 3;
const SILENCE_MS: u32 = 180_000;
const KEY_MISSED_MS: u32 = 30_000;
const COUNTER_STEP: u32 = 1024;

#[derive(Clone, Copy)]
pub struct Identity {
    pub manufacturer: &'static str,
    pub model: &'static str,
    pub sw_build: &'static str,
    pub manufacturer_code: u16,
    pub app_version: u8,
    pub hw_version: u8,
    pub color_temperature: bool,
    pub ct_min: u16,
    pub ct_max: u16,
    pub color: bool,
    pub image_type: u16,
    pub file_version: u32,
    pub touchlink_master_key: Option<[u8; 16]>,
    pub install_code: Option<[u8; 16]>,
    pub ble: bool,
    #[cfg(feature = "sense")]
    pub sense: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BleMode {
    On,
    Window,
    Off,
}

impl BleMode {
    pub(crate) fn from_u8(v: u8) -> Option<BleMode> {
        match v {
            0 => Some(BleMode::On),
            1 => Some(BleMode::Window),
            2 => Some(BleMode::Off),
            _ => None,
        }
    }

    pub(crate) fn to_u8(self) -> u8 {
        match self {
            BleMode::On => 0,
            BleMode::Window => 1,
            BleMode::Off => 2,
        }
    }
}

pub const BLE_WINDOW_DEFAULT_MIN: u16 = 20;
pub const BLE_WINDOW_MAX_MIN: u16 = 1440;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Down,
    Scan,
    AssocPoll,
    AssocWait,
    WaitKey,
    Up,
}

#[derive(Clone, Copy)]
pub(crate) struct Nib {
    pub channel: u8,
    pub pan: u16,
    pub short: u16,
    pub ext_pan: u64,
    pub update_id: u8,
    pub depth: u8,
    pub parent: u16,
    pub key: [u8; 16],
    pub key_seq: u8,
    pub alt_valid: bool,
    pub alt_old: bool,
    pub alt_seq: u8,
    pub alt_key: [u8; 16],
    pub tc_ext: u64,
    pub tclk: [u8; 16],
    pub tclk_new: [u8; 16],
    pub tclk_state: u8,
}

pub(crate) const DEFAULT_TCLK: [u8; 16] = *b"ZigBeeAlliance09";
pub(crate) const DISTRIBUTED_LINK_KEY: [u8; 16] =
    [0xD0, 0xD1, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9, 0xDA, 0xDB, 0xDC, 0xDD, 0xDE, 0xDF];
pub(crate) const NO_TRUST_CENTER: u64 = 0xFFFF_FFFF_FFFF_FFFF;

impl Nib {
    pub(crate) const fn empty() -> Nib {
        Nib {
            channel: 11,
            pan: 0xFFFF,
            short: 0xFFFF,
            ext_pan: 0,
            update_id: 0,
            depth: 1,
            parent: 0xFFFF,
            key: [0; 16],
            key_seq: 0,
            alt_valid: false,
            alt_old: false,
            alt_seq: 0,
            alt_key: [0; 16],
            tc_ext: 0,
            tclk: DEFAULT_TCLK,
            tclk_new: [0; 16],
            tclk_state: 0,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Neighbor {
    pub used: bool,
    pub short: u16,
    pub ext: u64,
    pub lqi: u8,
    pub out_cost: u8,
    pub age: u8,
    pub fc_next: u32,
    pub fc_seq: u8,
    pub failures: u8,
    pub listed: bool,
    pub relays: bool,
    pub stales: u8,
    pub rx: u16,
    pub rssi_avg: i8,
    pub rssi_min: i8,
    pub tx: u16,
    pub retries: u16,
    pub fails: u16,
    pub delivery: u16,
    pub delivered: u8,
    pub attempts: u8,
    pub cost: u8,
    pub confirmed: bool,
}

pub(crate) const DELIVERY_SAMPLES: u8 = 16;
const DELIVERY_SHIFT: u32 = 4;
const DELIVERY_MARGIN: u16 = 7864;

impl Neighbor {
    pub(crate) fn delivery_sample(&mut self, attempts: u8, ok: bool) {
        for k in 0..attempts {
            let got = ok && k + 1 == attempts;
            if self.attempts < DELIVERY_SAMPLES {
                self.attempts += 1;
                self.delivered += got as u8;
                if self.attempts == DELIVERY_SAMPLES {
                    self.delivery = (self.delivered as u32 * 65535 / DELIVERY_SAMPLES as u32) as u16;
                }
            } else {
                let p = self.delivery as i32;
                let d = if got { 65535 } else { 0 } - p;
                let half = 1 << (DELIVERY_SHIFT - 1);
                let step = if d >= 0 { (d + half) >> DELIVERY_SHIFT } else { -((-d + half) >> DELIVERY_SHIFT) };
                self.delivery = (p + step).clamp(0, 65535) as u16;
            }
            if self.measured() {
                let p = self.delivery;
                let new = nwk::cost_from_delivery(p);
                let cur = self.cost;
                let moves = cur == 0
                    || (new < cur && nwk::cost_from_delivery(p.saturating_sub(DELIVERY_MARGIN)) < cur)
                    || (new > cur && nwk::cost_from_delivery(p.saturating_add(DELIVERY_MARGIN)) > cur);
                if moves {
                    self.cost = new;
                }
            }
        }
    }

    pub(crate) fn measured(&self) -> bool {
        self.attempts >= DELIVERY_SAMPLES
    }

    pub(crate) fn link_cost(&self) -> u8 {
        if self.measured() {
            self.cost
        } else {
            nwk::cost_from_lqi(self.lqi).max(self.out_cost)
        }
    }
}

impl Neighbor {
    pub(crate) const EMPTY: Neighbor =
        Neighbor {
            used: false,
            short: 0,
            ext: 0,
            lqi: 0,
            out_cost: 0,
            age: 0,
            fc_next: 0,
            fc_seq: 0,
            failures: 0,
            listed: false,
            relays: false,
            stales: 0,
            rx: 0,
            rssi_avg: 0,
            rssi_min: 0,
            tx: 0,
            retries: 0,
            fails: 0,
            delivery: 0,
            delivered: 0,
            attempts: 0,
            cost: 0,
            confirmed: false,
        };
}

pub(crate) const ROUTE_FREE: u8 = 0;
pub(crate) const ROUTE_ACTIVE: u8 = 1;
pub(crate) const ROUTE_DISCOVERY: u8 = 2;

#[derive(Clone, Copy)]
pub(crate) struct Route {
    pub status: u8,
    pub dst: u16,
    pub next: u16,
    pub many_to_one: bool,
    pub no_cache: bool,
    pub record_due: bool,
    pub at: u32,
}

impl Route {
    const EMPTY: Route =
        Route { status: ROUTE_FREE, dst: 0, next: 0, many_to_one: false, no_cache: false, record_due: false, at: 0 };
}

#[derive(Clone, Copy)]
pub(crate) struct Discovery {
    pub used: bool,
    pub id: u8,
    pub src: u16,
    pub sender: u16,
    pub forward_cost: u8,
    pub residual_cost: u8,
    pub expires: u32,
}

impl Discovery {
    const EMPTY: Discovery =
        Discovery { used: false, id: 0, src: 0, sender: 0, forward_cost: 0, residual_cost: 0xFF, expires: 0 };
}

#[derive(Clone, Copy)]
pub(crate) struct Seen {
    pub used: bool,
    pub src: u16,
    pub seq: u8,
    pub expires: u32,
}

impl Seen {
    const EMPTY: Seen = Seen { used: false, src: 0, seq: 0, expires: 0 };
}

#[derive(Clone, Copy)]
pub(crate) struct Held {
    pub used: bool,
    pub header: nwk::Header,
    pub len: u8,
    pub payload: [u8; MAX_NWK_PAYLOAD],
    pub at: u32,
    pub left: u8,
    pub next: u16,
    pub heard: u16,
}

pub(crate) const ROUTED: u16 = 0xFFFF;

impl Held {
    const EMPTY: Held = Held {
        used: false,
        header: nwk::Header {
            frame_type: 0,
            discover: 0,
            security: true,
            dst: 0,
            src: 0,
            radius: 0,
            seq: 0,
            dst_ext: None,
            src_ext: None,
            multicast: None,
            relays: None,
            end_device_initiator: false,
        },
        len: 0,
        payload: [0; MAX_NWK_PAYLOAD],
        at: 0,
        left: 0,
        next: ROUTED,
        heard: 0,
    };
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Binding {
    pub used: bool,
    pub src_ep: u8,
    pub cluster: u16,
    pub group: bool,
    pub dst: u64,
    pub dst_ep: u8,
}

impl Binding {
    const EMPTY: Binding = Binding { used: false, src_ep: aps::ENDPOINT, cluster: 0, group: false, dst: 0, dst_ep: 0 };
}

#[derive(Clone, Copy)]
pub(crate) struct Report {
    pub min_s: u16,
    pub max_s: u16,
    pub change: u16,
    pub last_value: u16,
    pub last_sent: u32,
}

pub(crate) const REPORT_ON_OFF: usize = 0;
pub(crate) const REPORT_LEVEL: usize = 1;
pub(crate) const REPORT_COLOR_TEMP: usize = 2;
pub(crate) const REPORTS: usize = 3;
pub(crate) const REPORT_DEFAULTS: [(u16, u16, u16); REPORTS] = [(0, 3600, 0), (1, 3600, 1), (1, 3600, 1)];
pub(crate) const SCENES: usize = 8;
pub(crate) const APS_BROADCAST_RX_ON: u16 = nwk::BROADCAST_RX_ON;

pub(crate) const DEFERRED_MAX: usize = 8;

#[derive(Clone, Copy)]
pub(crate) struct Deferred {
    pub dst: u16,
    pub dst_ep: u8,
    pub cluster: u16,
    pub secured: bool,
    pub len: u8,
    pub data: [u8; DEFERRED_MAX],
}

#[derive(Clone, Copy)]
pub(crate) struct LightState {
    pub on: bool,
    pub level: u8,
    pub power_on: u8,
    pub startup_level: u8,
    pub color_temp: u16,
    pub startup_color_temp: u16,
    pub color_mode: u8,
    pub hue: u8,
    pub sat: u8,
    pub x: u16,
    pub y: u16,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct SceneColour {
    pub mode: u8,
    pub hue: u8,
    pub sat: u8,
    pub x: u16,
    pub y: u16,
}

impl SceneColour {
    pub(crate) const TEMPERATURE: SceneColour = SceneColour { mode: 2, hue: 0, sat: 0, x: 0, y: 0 };
}

#[derive(Clone, Copy)]
pub(crate) struct Scene {
    pub used: bool,
    pub group: u16,
    pub id: u8,
    pub on: bool,
    pub level: u8,
    pub color_temp: u16,
    pub colour: SceneColour,
    pub transition: u16,
    pub tenths: u8,
    pub omitted: u8,
}

impl Scene {
    pub(crate) const EMPTY: Scene =
        Scene { used: false, group: 0, id: 0, on: false, level: 0, color_temp: 0, colour: SceneColour::TEMPERATURE, transition: 0, tenths: 0, omitted: 0 };
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Stats {
    pub rx: u32,
    pub tx: u32,
    pub tx_failed: u32,
    pub relayed: u32,
    pub bad_mic: u32,
    pub replays: u32,
    pub joins: u32,
    pub indirect_dropped: u32,
    pub indirect_expired: u32,
}

pub struct Stack<H: Hal> {
    pub hal: H,
    pub(crate) identity: Identity,
    pub(crate) ext: u64,
    pub(crate) state: State,
    pub(crate) timer: u32,
    pub(crate) scan_index: u8,
    pub(crate) scan_found: [Option<join::Candidate>; join::CANDIDATES],
    pub(crate) join_lqi: u8,
    pub(crate) steering: u8,
    pub(crate) nib: Nib,
    pub(crate) key: Aes128,
    pub(crate) fc_out: u32,
    pub(crate) fc_stored: u32,
    pub(crate) aps_fc_out: u32,
    pub(crate) aps_fc_stored: u32,
    pub(crate) tc_fc_next: u32,
    pub(crate) mac_seq: u8,
    pub(crate) nwk_seq: u8,
    pub(crate) aps_counter: u8,
    pub(crate) zdo_seq: u8,
    pub(crate) zcl_seq: u8,
    pub(crate) rreq_id: u8,
    pub(crate) neighbors: [Neighbor; NEIGHBORS],
    pub(crate) routes: [Route; ROUTES],
    pub(crate) discoveries: [Discovery; DISCOVERIES],
    pub(crate) seen: [Seen; BTT],
    pub(crate) held: [Held; HELD],
    pub(crate) bcast: [Held; BCAST],
    pub(crate) aps_dups: [Seen; APS_DUPS],
    pub(crate) groups: [u16; GROUPS],
    pub(crate) bindings: [Binding; BINDINGS],
    pub(crate) addr_map: [(u64, u16); ADDR_MAP],
    pub(crate) addr_next: u8,
    pub(crate) addr_query_at: u32,
    pub(crate) children: [parent::Child; CHILDREN],
    pub(crate) children_stored: [u32; nv::CHILDREN_PARTS],
    pub(crate) polled: [u16; CHILDREN],
    pub(crate) indirect: [parent::Indirect; INDIRECT],
    pub(crate) permit_until: Option<u32>,
    pub(crate) children_checked_at: u32,
    pub(crate) key_due: Option<(u32, u64)>,
    pub(crate) ota: ota::Ota,
    pub(crate) proxies: [gp::ProxyEntry; gp::PROXIES],
    pub(crate) gp_mode: Option<gp::Commissioning>,
    pub(crate) gp_tx: Option<gp::Queued>,
    pub(crate) gp_seen: [(gp::Gpd, u32, u32); 4],
    pub(crate) gp_keys: gp::Keys,
    pub(crate) gp_pending: [Option<gp::Pending>; 2],
    pub(crate) gp_first: bool,
    pub(crate) conflict: (u16, u32),
    pub(crate) conflict_due: Option<u32>,
    pub(crate) pan_report_at: u32,
    pub(crate) manager: u16,
    pub(crate) booted: u32,
    pub(crate) parent_annce_at: Option<u32>,
    pub(crate) parent_annce_from: usize,
    pub(crate) touchlink: Option<touchlink::Transaction>,
    pub(crate) frag_in: frag::Incoming,
    pub(crate) frag_out: frag::Outgoing,
    pub(crate) reports: [Report; REPORTS],
    pub(crate) scenes: [Scene; SCENES],
    pub(crate) scene_current: (u16, u8),
    pub(crate) scene_valid: bool,
    pub(crate) level_options: u8,
    pub(crate) color_options: u8,
    pub(crate) on_level: u8,
    pub(crate) global_scene_control: bool,
    pub(crate) on_time: u16,
    pub(crate) off_wait: u16,
    pub(crate) on_tick_at: u32,
    pub(crate) rx_signal: (u8, i8),
    pub(crate) level_until: u32,
    pub(crate) color_until: u32,
    pub(crate) scenes_dirty: bool,
    pub(crate) tables_dirty: bool,
    pub(crate) reply_secured: bool,
    pub(crate) light: LightState,
    pub(crate) cmds: [Option<Command>; CMDS],
    pub(crate) deferred: Option<Deferred>,
    pub(crate) link_status_at: u32,
    pub(crate) age_at: u32,
    pub(crate) link_status_sent: u32,
    pub(crate) announce_at: Option<u32>,
    pub(crate) beacon_at: Option<u32>,
    pub(crate) heard_at: u32,
    pub(crate) searching: Option<u8>,
    pub(crate) leave_at: Option<u32>,
    pub(crate) leave_rejoin: bool,
    pub(crate) rejoin: Option<bool>,
    pub(crate) rejoin_wait: Option<(bool, u32, u16, u8)>,
    pub(crate) unknown_key_frames: u8,
    pub(crate) channel_at: Option<(u32, u8)>,
    pub(crate) pan_at: Option<(u32, u16)>,
    pub(crate) identify_until: Option<u32>,
    pub(crate) tclk_step: u8,
    pub(crate) tclk_at: u32,
    pub(crate) tclk_tries: u8,
    pub(crate) ble_mode: BleMode,
    pub(crate) ble_window: u16,
    pub(crate) ble_active: bool,
    pub stats: Stats,
    pub(crate) diag: diag::Diag,
    pub(crate) xreports: [objects::XReport; objects::XREPORTS],
    pub(crate) xreports_dirty: bool,
    pub(crate) xreport_at: u32,
    pub(crate) allow_children: bool,
    #[cfg(feature = "sense")]
    pub(crate) sense: sense::Sense,
}

impl<H: Hal> Stack<H> {
    pub fn new(hal: H, identity: Identity) -> Self {
        let mut s = Self::blank(hal, identity);
        let now = s.timer;
        s.restore(now);
        s
    }

    #[inline(always)]
    fn blank(mut hal: H, identity: Identity) -> Self {
        let ext = hal.ieee();
        let now = hal.now_ms();
        let seed = hal.random();
        Stack {
            hal,
            identity,
            ext,
            state: State::Down,
            timer: now,
            scan_index: 0,
            scan_found: [None; join::CANDIDATES],
            join_lqi: 0,
            steering: 0,
            nib: Nib::empty(),
            key: Aes128::with_engine(&[0; 16], H::AES),
            fc_out: 0,
            fc_stored: 0,
            aps_fc_out: 0,
            aps_fc_stored: 0,
            tc_fc_next: 0,
            mac_seq: seed as u8,
            nwk_seq: (seed >> 8) as u8,
            aps_counter: (seed >> 16) as u8,
            zdo_seq: (seed >> 24) as u8,
            zcl_seq: 0,
            rreq_id: 0,
            neighbors: [Neighbor::EMPTY; NEIGHBORS],
            routes: [Route::EMPTY; ROUTES],
            discoveries: [Discovery::EMPTY; DISCOVERIES],
            seen: [Seen::EMPTY; BTT],
            held: [Held::EMPTY; HELD],
            bcast: [Held::EMPTY; BCAST],
            aps_dups: [Seen::EMPTY; APS_DUPS],
            groups: [0; GROUPS],
            bindings: [Binding::EMPTY; BINDINGS],
            addr_map: [(0, 0xFFFF); ADDR_MAP],
            addr_next: 0,
            addr_query_at: now,
            children: [parent::Child::EMPTY; CHILDREN],
            children_stored: [nv::NO_CHILDREN; nv::CHILDREN_PARTS],
            polled: [0; CHILDREN],
            indirect: [parent::Indirect::EMPTY; INDIRECT],
            permit_until: None,
            children_checked_at: now,
            key_due: None,
            ota: ota::Ota::new(now),
            proxies: [gp::ProxyEntry::EMPTY; gp::PROXIES],
            gp_mode: None,
            gp_tx: None,
            gp_seen: [(gp::Gpd::Src(0), 0, now); 4],
            gp_keys: gp::Keys::DEFAULT,
            gp_pending: [None; 2],
            gp_first: false,
            conflict: (0xFFFF, now),
            conflict_due: None,
            pan_report_at: now,
            manager: 0x0000,
            booted: now,
            parent_annce_at: None,
            parent_annce_from: 0,
            touchlink: None,
            frag_in: frag::Incoming::EMPTY,
            frag_out: frag::Outgoing::EMPTY,
            reports: REPORT_DEFAULTS.map(|(min_s, max_s, change)| Report { min_s, max_s, change, last_value: 0, last_sent: now }),
            scenes: [Scene::EMPTY; SCENES],
            scene_current: (0, 0),
            scene_valid: false,
            level_options: 0,
            color_options: zcl::COLOR_OPTIONS_DEFAULT,
            on_level: 0xFF,
            global_scene_control: true,
            on_time: 0,
            off_wait: 0,
            on_tick_at: now,
            rx_signal: (0, 0),
            level_until: now,
            color_until: now,
            scenes_dirty: false,
            tables_dirty: false,
            reply_secured: false,
            light: LightState {
                on: false,
                level: 254,
                power_on: 0xFF,
                startup_level: 0xFF,
                color_temp: bulb_core::CT_DEFAULT,
                startup_color_temp: 0xFFFF,
                color_mode: 2,
                hue: 170,
                sat: 254,
                x: 0,
                y: 0,
            },
            cmds: [None; CMDS],
            deferred: None,
            link_status_at: now,
            age_at: now.wrapping_add(LINK_STATUS_MS),
            link_status_sent: now,
            announce_at: None,
            beacon_at: None,
            heard_at: now,
            searching: None,
            leave_at: None,
            leave_rejoin: false,
            rejoin: None,
            rejoin_wait: None,
            unknown_key_frames: 0,
            channel_at: None,
            pan_at: None,
            identify_until: None,
            tclk_step: 0,
            tclk_at: now,
            tclk_tries: 0,
            ble_mode: BleMode::On,
            ble_window: BLE_WINDOW_DEFAULT_MIN,
            ble_active: false,
            stats: Stats::default(),
            diag: diag::Diag::new(now),
            xreports: [objects::XReport::EMPTY; objects::XREPORTS],
            xreports_dirty: false,
            xreport_at: now,
            allow_children: true,
            #[cfg(feature = "sense")]
            sense: sense::Sense::new(now),
        }
    }

    pub fn new_in(slot: &mut Option<Self>, hal: H, identity: Identity) -> &mut Self {
        let s = slot.insert(Self::blank(hal, identity));
        let now = s.timer;
        s.restore(now);
        s
    }

    fn restore(&mut self, now: u32) {
        self.load_counters();
        self.load_settings();
        #[cfg(feature = "sense")]
        self.sense_load(now);
        if self.load_network() {
            self.load_tables();
            self.load_xreports(now);
            self.load_scenes();
            self.load_proxies();
            self.load_children(now);
            self.resume(now);
        } else {
            self.hal.set_filter(0xFFFF, 0xFFFF);
        }
        self.diag_started();
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn is_joined(&self) -> bool {
        self.state == State::Up
    }

    pub fn short_address(&self) -> u16 {
        self.nib.short
    }

    pub fn pan_id(&self) -> u16 {
        self.nib.pan
    }

    pub fn channel(&self) -> u8 {
        self.nib.channel
    }

    pub fn leave(&mut self) {
        if self.state == State::Up {
            self.leave_rejoin = false;
            self.leave_at = Some(self.hal.now_ms());
        }
    }

    pub fn ble_mode(&self) -> BleMode {
        self.ble_mode
    }

    pub fn ble_window_minutes(&self) -> u16 {
        self.ble_window
    }

    pub fn set_ble_active(&mut self, active: bool) {
        self.ble_active = active;
    }

    pub fn child_count(&self) -> usize {
        self.children.iter().filter(|c| c.used && c.joined).count()
    }

    pub fn set_light(&mut self, s: &StateReport) {
        let l = LightState {
            on: s.on,
            level: s.level,
            power_on: s.power_on_behavior.to_u8(),
            startup_level: s.startup_level,
            color_temp: s.color_temp,
            startup_color_temp: s.startup_color_temp,
            color_mode: s.color_mode as u8,
            hue: s.hue,
            sat: s.sat,
            x: s.x,
            y: s.y,
        };
        self.light = l;
        #[cfg(feature = "sense")]
        self.sense_light(s.on, s.level, s.color_temp);
    }

    pub fn poll(&mut self) -> Option<Command> {
        if self.cmds[0].is_none() {
            self.send_deferred();
        }
        #[cfg(feature = "sense")]
        {
            self.sense.drained = false;
        }
        for _ in 0..4 {
            let mut buf = [0u8; 127];
            match self.hal.receive(&mut buf) {
                Some(meta) => {
                    #[cfg(feature = "sense")]
                    if self.sense_heard(buf.get(..meta.len).unwrap_or(&[]), meta.rssi) {
                        continue;
                    }
                    self.stats.rx = self.stats.rx.wrapping_add(1);
                    if let Some(mpdu) = buf.get(..meta.len) {
                        self.rx_signal = (meta.lqi, meta.rssi);
                        self.diag.rssi_add(meta.rssi);
                        self.on_mpdu(mpdu, meta.lqi, meta.end_us);
                    }
                }
                None => {
                    #[cfg(feature = "sense")]
                    {
                        self.sense.drained = true;
                    }
                    break;
                }
            }
        }
        let now = self.hal.now_ms();
        match self.state {
            State::Up => self.run_timers(now),
            _ => self.join_timers(now),
        }
        self.take_command()
    }

    pub(crate) fn push_command(&mut self, c: Command) {
        let now = self.hal.now_ms();
        match c {
            Command::MoveToLevel { transition_ms, .. } => self.level_until = now.wrapping_add(transition_ms),
            Command::MoveToColorTemperature { transition_ms, .. }
            | Command::MoveToHueSaturation { transition_ms, .. }
            | Command::MoveToXy { transition_ms, .. } => self.color_until = now.wrapping_add(transition_ms),
            Command::StopTransition => self.level_until = now,
            Command::StopColorTransition => self.color_until = now,
            Command::Off | Command::On | Command::Toggle => {
                self.level_until = now;
                self.color_until = now;
            }
            _ => {}
        }
        for slot in self.cmds.iter_mut() {
            if slot.is_none() {
                *slot = Some(c);
                return;
            }
        }
    }

    fn take_command(&mut self) -> Option<Command> {
        let c = self.cmds[0].take();
        if c.is_some() {
            self.cmds.rotate_left(1);
        }
        c
    }

    fn run_timers(&mut self, now: u32) {
        self.diag_timers(now);
        if self.state == State::Up {
            self.diag_noise(now);
        }
        self.box_timers(now);
        if let Some(at) = self.leave_at {
            if due(now, at) {
                self.leave_at = None;
                self.send_leave();
                if core::mem::take(&mut self.leave_rejoin) {
                    self.start_rejoin(now, true);
                } else {
                    self.leave_network(now);
                }
                return;
            }
        }
        if let Some((at, channel)) = self.channel_at {
            if due(now, at) {
                self.channel_at = None;
                self.nib.channel = channel;
                self.hal.set_channel(channel);
                self.log_event(diag::ev::CHANNEL, channel as u16, 0);
                self.store_network();
            }
        }
        if let Some((at, pan)) = self.pan_at {
            if due(now, at) {
                self.pan_at = None;
                self.nib.pan = pan;
                self.log_event(diag::ev::PAN, pan, 0);
                self.hal.set_filter(pan, self.nib.short);
                self.store_network();
            }
        }
        if let Some(at) = self.announce_at {
            if due(now, at) {
                self.announce_at = None;
                self.device_announce();
            }
        }
        if self.steering == join::STEERING_OPEN_DUE && self.announce_at.is_none() {
            self.open_after_join(now);
        }
        if let Some(at) = self.parent_annce_at {
            if due(now, at) {
                self.parent_annce_at = None;
                self.parent_announce();
            }
        }
        if let Some(at) = self.beacon_at {
            if due(now, at) {
                self.beacon_at = None;
                self.send_beacon();
            }
        }
        if let Some((secure, until, ..)) = self.rejoin_wait {
            if due(now, until) {
                self.rejoin_wait = None;
                self.rejoin_next(now, secure);
            }
        }
        if self.unknown_key_frames >= 5
            && now.wrapping_sub(self.heard_at) > KEY_MISSED_MS
            && self.rejoin_wait.is_none()
            && self.nib.tc_ext != NO_TRUST_CENTER
        {
            self.unknown_key_frames = 0;
            self.start_rejoin(now, false);
            return;
        }
        if now.wrapping_sub(self.heard_at) > SILENCE_MS {
            self.search_network(now);
            return;
        }
        if due(now, self.link_status_at) {
            self.link_status_at = now.wrapping_add(LINK_STATUS_MS - 1000 + self.hal.random() % 1000);
            self.send_link_status();
        }
        if due(now, self.age_at) {
            self.age_at = now.wrapping_add(LINK_STATUS_MS);
            self.age_neighbors();
        }
        self.net_timers(now);
        self.key_timer(now);
        self.publish_children();
        if due(now, self.children_checked_at) {
            self.children_checked_at = now.wrapping_add(5_000);
            self.parent_timers(now);
        }
        self.frag_timers(now);
        self.gp_timers(now);
        self.ota_timers(now);
        self.tclk_timers(now);
        self.report_timers(now);
        self.xreports_due(now);
        self.on_off_timers(now);
        if let Some(until) = self.identify_until {
            if due(now, until) {
                self.identify_until = None;
            }
        }
        if self.tables_dirty {
            self.tables_dirty = false;
            self.store_tables();
        }
        if self.scenes_dirty {
            self.scenes_dirty = false;
            self.store_scenes();
        }
        #[cfg(feature = "sense")]
        self.sense_timers(now);
    }

    pub(crate) fn next_fc(&mut self) -> u32 {
        let v = self.fc_out;
        self.fc_out = v.wrapping_add(1);
        if self.fc_out >= self.fc_stored {
            self.fc_stored = self.fc_out.wrapping_add(COUNTER_STEP);
            self.store_counters();
        }
        v
    }

    pub(crate) fn next_aps_fc(&mut self) -> u32 {
        let v = self.aps_fc_out;
        self.aps_fc_out = v.wrapping_add(1);
        if self.aps_fc_out >= self.aps_fc_stored {
            self.aps_fc_stored = self.aps_fc_out.wrapping_add(COUNTER_STEP);
            self.store_counters();
        }
        v
    }

    pub(crate) fn next_mac_seq(&mut self) -> u8 {
        self.mac_seq = self.mac_seq.wrapping_add(1);
        self.mac_seq
    }

    pub(crate) fn next_nwk_seq(&mut self) -> u8 {
        self.nwk_seq = self.nwk_seq.wrapping_add(1);
        self.nwk_seq
    }
}
