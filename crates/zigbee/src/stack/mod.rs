//! The stack proper: one `Stack` owns the hardware, the network state and all
//! tables, and is driven by `poll()` from the firmware's main loop.
//!
//! Scope: a Zigbee PRO router on a centralized-security network. It joins by
//! MAC association, takes the network key from the Trust Center, exchanges
//! its Trust Center link key, routes for other nodes (mesh route discovery,
//! many-to-one routes with route records, source routes, broadcast relay,
//! link status), lets other devices join through it and looks after
//! end-device children (the parent role), and serves ZDO and the ZCL
//! clusters of a dimmable light.
//! Touchlink is answered as a target, and Green Power devices are proxied
//! (basic proxy). Networks without a Trust Center (distributed security, as
//! touchlink makes them) are joined and served too.

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

pub use diag::{ev, LoggedEvent, MacCounters, NeighborCounts, DIAG_COUNTERS_MAX};
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
/// Broadcast transaction records: enough for 2 s (`BTT_KEEP_MS` in net.rs)
/// of a storm of 40 broadcasts a second.
pub(crate) const BTT: usize = 80;
/// Unicasts waiting for a route or for another attempt: a few per
/// neighbor of a busy router.
pub(crate) const HELD: usize = 10;
/// Broadcasts waiting for their (re)transmission.
pub(crate) const BCAST: usize = 8;
pub(crate) const APS_DUPS: usize = 16;
pub(crate) const GROUPS: usize = 8;
pub(crate) const BINDINGS: usize = 8;
pub(crate) const ADDR_MAP: usize = 4;
pub(crate) const CMDS: usize = 6;
/// Devices that joined through this router and are its children (end
/// devices; a router child is a neighbor once it has the key).
pub const CHILDREN: usize = 32;
/// Frames held for children until they poll (indirect transmission): one
/// pool for all, at most `INDIRECT_PER_CHILD` for one child.
pub const INDIRECT: usize = 24;
pub const INDIRECT_PER_CHILD: usize = 4;
/// The most short addresses the radio is told have frames waiting
/// (`Hal::indirect_pending`): every child (a broadcast waits for all that
/// sleep) and the other addresses of the pool; the extended addresses are
/// at most `INDIRECT`.
pub const PENDING_SHORTS: usize = CHILDREN + INDIRECT;
/// The most bytes the stack hands to `Hal::retain` (the diagnostic counters).
pub const RETAIN_MAX: usize = diag::SNAPSHOT_LEN + diag::EVENTS_LEN;
/// Longest NWK payload: 125 - MAC header 9 - NWK header 8 - aux 14 - MIC 4.
pub(crate) const MAX_NWK_PAYLOAD: usize = 90;

/// nwkLinkStatusPeriod.
pub(crate) const LINK_STATUS_MS: u32 = 15_000;
/// nwkRouterAgeLimit: missed link status periods before a neighbor is stale.
pub(crate) const AGE_LIMIT: u8 = 3;
/// Silence after which the node looks for its network on other channels.
/// (Neighbors send Link Status every 15 s.)
const SILENCE_MS: u32 = 180_000;
/// Only undecryptable frames for this long: the network key has changed.
const KEY_MISSED_MS: u32 = 30_000;
/// How far ahead of use the outgoing frame counters are stored.
const COUNTER_STEP: u32 = 1024;

/// How the device presents itself (Basic cluster, node descriptor).
#[derive(Clone, Copy)]
pub struct Identity {
    pub manufacturer: &'static str,
    pub model: &'static str,
    pub sw_build: &'static str,
    pub manufacturer_code: u16,
    /// Application version (Basic 0x0001).
    pub app_version: u8,
    /// Hardware version (Basic 0x0003).
    pub hw_version: u8,
    /// Tunable-white bulb: a Color Temperature Light with the Color Control cluster.
    pub color_temperature: bool,
    /// Its colour temperature range in mireds (ColorTempPhysicalMin/Max).
    pub ct_min: u16,
    pub ct_max: u16,
    /// A colour bulb: an Extended Color Light, Color Control with hue and
    /// saturation and CIE xy beside the colour temperature (which it has too:
    /// `color_temperature` is set as well).
    pub color: bool,
    /// OTA image type and the version of the running firmware.
    pub image_type: u16,
    pub file_version: u32,
    /// The touchlink master key (key index 4), if the build was given it.
    /// It is what commercial initiators use; it is issued under licence and
    /// is not part of this code.
    pub touchlink_master_key: Option<[u8; 16]>,
    /// The device's install code (16 bytes, without its checksum), if it
    /// has one: a Trust Center that was told the code sends the network key
    /// under the link key derived from it instead of the well-known one.
    pub install_code: Option<[u8; 16]>,
    /// The firmware has Bluetooth LE next to Zigbee: the endpoint carries
    /// the settings cluster (`0xFC5F`) that says when BLE runs (`BleMode`).
    pub ble: bool,
    /// Motion sensing from RSSI (feature `sense`, docs/rf-sensing.md):
    /// probes, Occupancy Sensing and the manufacturer cluster `0xFC5E`.
    #[cfg(feature = "sense")]
    pub sense: bool,
}

/// When the firmware runs BLE (attribute `0x0000` of the settings cluster
/// `0xFC5F`). The firmware asks `Stack::ble_mode`; the stack only keeps
/// the setting. Leaving the network and a factory reset forget it, so BLE
/// comes back: a bulb that is in no network has only BLE to be reached by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BleMode {
    /// Always (out of the box).
    On,
    /// For `Stack::ble_window_minutes` after power-on, then not until the next.
    Window,
    /// Not at all.
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

/// The BLE window out of the box, in minutes: as long as the stock bulb
/// takes connections after power-on when it is not paired with the app.
pub const BLE_WINDOW_DEFAULT_MIN: u16 = 20;
/// The longest BLE window that can be set: a day.
pub const BLE_WINDOW_MAX_MIN: u16 = 1440;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Not on a network; a scan starts when the timer runs out.
    Down,
    /// Active scan: listening for beacons on one channel.
    Scan,
    /// Association request acknowledged; waiting to poll for the response.
    AssocPoll,
    /// Polled; waiting for the association response.
    AssocWait,
    /// Associated; waiting for the network key from the Trust Center.
    WaitKey,
    /// Authenticated and routing.
    Up,
}

/// Network information that survives a power cycle.
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
    /// The alternate network key: the one to come (sent by the Trust Center,
    /// not yet switched to) or, with `alt_old`, the one in use before the
    /// last switch, which is still good for incoming frames.
    pub alt_valid: bool,
    pub alt_old: bool,
    pub alt_seq: u8,
    pub alt_key: [u8; 16],
    pub tc_ext: u64,
    /// Trust Center link key in use.
    pub tclk: [u8; 16],
    /// Link key received from the Trust Center and not yet confirmed.
    pub tclk_new: [u8; 16],
    /// 0 = default key, exchange still to do; 1 = new key received;
    /// 2 = new key confirmed; 3 = the Trust Center does not exchange keys.
    pub tclk_state: u8,
}

pub(crate) const DEFAULT_TCLK: [u8; 16] = *b"ZigBeeAlliance09";
/// Link key under which the routers of a network without a Trust Center
/// hand the network key to a joining device (distributed security global
/// link key of the Base Device Behavior specification).
pub(crate) const DISTRIBUTED_LINK_KEY: [u8; 16] =
    [0xD0, 0xD1, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9, 0xDA, 0xDB, 0xDC, 0xDD, 0xDE, 0xDF];
/// "Trust Center address" of a network that has none.
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
    /// 0 when not yet known.
    pub ext: u64,
    /// Running average of the link quality of frames heard from it.
    pub lqi: u8,
    /// What it last reported as its incoming cost for our frames; 0 = unknown.
    pub out_cost: u8,
    /// Link status periods since it was last heard.
    pub age: u8,
    /// Lowest frame counter it may use next, under the network key with
    /// the sequence number `fc_seq`.
    pub fc_next: u32,
    pub fc_seq: u8,
    /// Transmissions to it in a row that the MAC could not deliver (the
    /// failure counter of R22 3.6.3.7); 0 again once one gets through or
    /// it is heard.
    pub failures: u8,
    /// It was in our last Link Status.
    pub listed: bool,
    /// It has been heard passing on a broadcast of another device: a
    /// router whose relay a passive acknowledgement waits for.
    pub relays: bool,
    // diagnostics (docs/diagnostics.md): times it went stale, frames heard
    // from it and their RSSI (running average, lowest), our transmissions to
    // it, their retries and failures
    pub stales: u8,
    pub rx: u16,
    pub rssi_avg: i8,
    pub rssi_min: i8,
    pub tx: u16,
    pub retries: u16,
    pub fails: u16,
    /// Probability of delivery measured on our unicasts to it (0..=65535
    /// for 0..1): the share of transmission attempts that got their
    /// acknowledgement, a frame and its ACK, so both ways. The first
    /// `DELIVERY_SAMPLES` attempts are counted (`delivered`, `attempts`),
    /// then it is a running average (1/16 a sample, rounded). Until then the
    /// link cost comes from the LQI.
    pub delivery: u16,
    pub delivered: u8,
    pub attempts: u8,
    /// The measured link cost, with hysteresis (`delivery_sample`).
    pub cost: u8,
    /// A Link Status of its own has been heard (or it joined through us):
    /// a two-way neighbor, not one on probation. Only such a one can go
    /// stale.
    pub confirmed: bool,
}

/// Attempts measured before the probability of delivery replaces the LQI estimate.
pub(crate) const DELIVERY_SAMPLES: u8 = 16;
/// The running average takes 1/2^this of each attempt.
const DELIVERY_SHIFT: u32 = 4;
/// Hysteresis of the measured cost, in probability of delivery (0.12): the
/// cost moves only when it would move even with the estimate this much
/// closer to where it was. All seven costs lie within p 0.63..1, steps
/// 0.03..0.11 apart, and the average's own spread at p 0.8 is about 0.07:
/// without it the cost changed every few frames on a middling link. Chosen
/// on the Pareto front of `link_eval_pareto` (src/stack/link_eval.rs).
const DELIVERY_MARGIN: u16 = 7864;

impl Neighbor {
    /// Takes the outcome of one transmission: `attempts` on the air, the
    /// last acknowledged if `ok`.
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
                // rounded half away from zero, so the average reaches 0 and 1
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

    /// The cost of the link for routing: measured once there are enough
    /// samples (both ways in one), else the worse of the LQI estimate and
    /// what the neighbor reports of our frames (R22 3.6.3.1).
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
    /// The destination is a concentrator (route from a many-to-one request).
    pub many_to_one: bool,
    /// The concentrator keeps no route cache: every data frame needs a route record.
    pub no_cache: bool,
    /// A route record still has to go to the concentrator.
    pub record_due: bool,
    /// When the entry was created or last confirmed.
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
    /// Neighbor the cheapest request came from (the way back to `src`).
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

/// A plaintext NWK frame kept for later: a broadcast waiting for its
/// (re)transmission, a unicast waiting for a route, or a unicast waiting to
/// be tried again after the MAC could not deliver it.
#[derive(Clone, Copy)]
pub(crate) struct Held {
    pub used: bool,
    pub header: nwk::Header,
    pub len: u8,
    pub payload: [u8; MAX_NWK_PAYLOAD],
    /// Broadcast: next transmission. Unicast waiting for a route: when to
    /// give up. Unicast to be tried again: when.
    pub at: u32,
    /// Broadcast: transmissions left. Unicast: 0 while it waits for a
    /// route, else the attempts left (this one included).
    pub left: u8,
    /// Unicast to be tried again: the neighbor it goes to (a source route
    /// names it), or `ROUTED` when the routing table picks it anew.
    pub next: u16,
    /// Broadcast: the neighbors (bits by index in the neighbor table) heard
    /// sending it, ours or theirs: its passive acknowledgement.
    pub heard: u16,
}

/// `Held::next` of a unicast that is routed anew when it is tried again.
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
    /// The endpoint of ours the binding is for (the light's, or an object's).
    pub src_ep: u8,
    pub cluster: u16,
    /// true: `dst` is a group address; false: `dst` is an EUI-64 with `dst_ep`.
    pub group: bool,
    pub dst: u64,
    pub dst_ep: u8,
}

impl Binding {
    const EMPTY: Binding = Binding { used: false, src_ep: aps::ENDPOINT, cluster: 0, group: false, dst: 0, dst_ep: 0 };
}

/// Reporting configuration and state of one attribute.
#[derive(Clone, Copy)]
pub(crate) struct Report {
    pub min_s: u16,
    /// 0xFFFF = reporting off.
    pub max_s: u16,
    pub change: u16,
    pub last_value: u16,
    pub last_sent: u32,
}

/// Index into `Stack::reports`.
pub(crate) const REPORT_ON_OFF: usize = 0;
pub(crate) const REPORT_LEVEL: usize = 1;
pub(crate) const REPORT_COLOR_TEMP: usize = 2;
pub(crate) const REPORTS: usize = 3;
/// Reporting configuration out of the box: minimum and maximum interval in seconds, reportable change.
pub(crate) const REPORT_DEFAULTS: [(u16, u16, u16); REPORTS] = [(0, 3600, 0), (1, 3600, 1), (1, 3600, 1)];
pub(crate) const SCENES: usize = 8;
/// Any node in range that keeps its receiver on.
pub(crate) const APS_BROADCAST_RX_ON: u16 = nwk::BROADCAST_RX_ON;

/// The longest default response kept for after the light has changed.
pub(crate) const DEFERRED_MAX: usize = 8;

/// A default response waiting until the firmware has taken the command it
/// answers (`Stack::defer_reply`).
#[derive(Clone, Copy)]
pub(crate) struct Deferred {
    pub dst: u16,
    pub dst_ep: u8,
    pub cluster: u16,
    /// The request came APS-encrypted.
    pub secured: bool,
    pub len: u8,
    pub data: [u8; DEFERRED_MAX],
}

/// What the stack knows of the light, for attribute reads and reports.
#[derive(Clone, Copy)]
pub(crate) struct LightState {
    pub on: bool,
    pub level: u8,
    pub power_on: u8,
    pub startup_level: u8,
    pub color_temp: u16,
    pub startup_color_temp: u16,
    /// Colour bulbs: ColorMode and the colour the light is set to.
    pub color_mode: u8,
    pub hue: u8,
    pub sat: u8,
    pub x: u16,
    pub y: u16,
}

/// The colour of a scene of a colour bulb: ColorMode (2: the colour
/// temperature of the scene), hue, saturation and CIE x, y.
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

/// A stored scene: what the light looks like in it.
#[derive(Clone, Copy)]
pub(crate) struct Scene {
    pub used: bool,
    pub group: u16,
    pub id: u8,
    pub on: bool,
    pub level: u8,
    pub color_temp: u16,
    /// Colour bulbs: the colour mode and colour of the scene.
    pub colour: SceneColour,
    /// Transition time: whole seconds, and tenths on top.
    pub transition: u16,
    pub tenths: u8,
    /// Extension field sets the scene was added without (`zcl::SCENE_NO_*`):
    /// recalling it leaves those clusters as they are.
    pub omitted: u8,
}

impl Scene {
    pub(crate) const EMPTY: Scene =
        Scene { used: false, group: 0, id: 0, on: false, level: 0, color_temp: 0, colour: SceneColour::TEMPERATURE, transition: 0, tenths: 0, omitted: 0 };
}

/// Counters for diagnostics and tests.
#[derive(Clone, Copy, Default, Debug)]
pub struct Stats {
    pub rx: u32,
    pub tx: u32,
    pub tx_failed: u32,
    pub relayed: u32,
    pub bad_mic: u32,
    pub replays: u32,
    pub joins: u32,
    /// Frames held for children that gave up their place (or found none)
    /// before they were polled for, and that waited out their persistence time.
    pub indirect_dropped: u32,
    pub indirect_expired: u32,
}

pub struct Stack<H: Hal> {
    pub hal: H,
    pub(crate) identity: Identity,
    pub(crate) ext: u64,
    pub(crate) state: State,
    /// Deadline of the current joining state.
    pub(crate) timer: u32,
    pub(crate) scan_index: u8,
    /// Routers heard in the scan and not tried yet, the best first.
    pub(crate) scan_found: [Option<join::Candidate>; join::CANDIDATES],
    /// Link quality of the beacon of the router being joined through.
    pub(crate) join_lqi: u8,
    /// Joining by association and what is left of it to do: 0 or one of `join::STEERING_*`.
    pub(crate) steering: u8,
    pub(crate) nib: Nib,
    pub(crate) key: Aes128,
    pub(crate) fc_out: u32,
    pub(crate) fc_stored: u32,
    pub(crate) aps_fc_out: u32,
    pub(crate) aps_fc_stored: u32,
    /// Lowest APS frame counter the Trust Center may use next.
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
    /// A digest of each part of the child table as last stored.
    pub(crate) children_stored: [u32; nv::CHILDREN_PARTS],
    /// The end-device children the radio was last told about.
    pub(crate) polled: [u16; CHILDREN],
    pub(crate) indirect: [parent::Indirect; INDIRECT],
    /// Joining through this router is allowed until then.
    pub(crate) permit_until: Option<u32>,
    pub(crate) children_checked_at: u32,
    /// A newcomer to send the network key to (no Trust Center): when, who.
    pub(crate) key_due: Option<(u32, u64)>,
    pub(crate) ota: ota::Ota,
    pub(crate) proxies: [gp::ProxyEntry; gp::PROXIES],
    pub(crate) gp_mode: Option<gp::Commissioning>,
    /// A frame from the sink waiting for a Green Power device's receive window.
    pub(crate) gp_tx: Option<gp::Queued>,
    /// Green Power frames just handled: device, counter, until when they count as repeats.
    pub(crate) gp_seen: [(gp::Gpd, u32, u32); 4],
    pub(crate) gp_keys: gp::Keys,
    /// Commissioning notifications waiting out their tunnelling delay.
    pub(crate) gp_pending: [Option<gp::Pending>; 2],
    /// The sink last named this proxy to answer a device.
    pub(crate) gp_first: bool,
    /// Address conflict last reported, and until when it is not reported again.
    pub(crate) conflict: (u16, u32),
    /// When the report of that conflict goes out, while it is waiting to.
    pub(crate) conflict_due: Option<u32>,
    /// When a PAN identifier conflict may next be reported.
    pub(crate) pan_report_at: u32,
    /// nwkManagerAddr.
    pub(crate) manager: u16,
    /// When this router last came up with a stored network; children heard
    /// since are known to be still ours.
    pub(crate) booted: u32,
    /// When to tell the other routers which end devices we take for ours.
    pub(crate) parent_annce_at: Option<u32>,
    /// The entry of the child table the next Parent_annce starts from.
    pub(crate) parent_annce_from: usize,
    pub(crate) touchlink: Option<touchlink::Transaction>,
    pub(crate) frag_in: frag::Incoming,
    pub(crate) frag_out: frag::Outgoing,
    pub(crate) reports: [Report; REPORTS],
    pub(crate) scenes: [Scene; SCENES],
    /// Scene last recalled (group, id), valid until the light is changed otherwise.
    pub(crate) scene_current: (u16, u8),
    pub(crate) scene_valid: bool,
    /// The Options attributes of Level Control and Color Control (bit 0: execute if off).
    pub(crate) level_options: u8,
    pub(crate) color_options: u8,
    /// OnLevel: the level an On command brings (0xFF: the previous one).
    pub(crate) on_level: u8,
    /// On/Off cluster: GlobalSceneControl, OnTime and OffWaitTime (in
    /// tenths of a second), and when they next count down.
    pub(crate) global_scene_control: bool,
    pub(crate) on_time: u16,
    pub(crate) off_wait: u16,
    pub(crate) on_tick_at: u32,
    /// Link quality and signal strength (dBm) of the frame being processed.
    pub(crate) rx_signal: (u8, i8),
    /// When the running transitions of level and colour end (RemainingTime).
    pub(crate) level_until: u32,
    pub(crate) color_until: u32,
    pub(crate) scenes_dirty: bool,
    pub(crate) tables_dirty: bool,
    /// The request being handled came APS-encrypted: so goes the answer.
    pub(crate) reply_secured: bool,
    pub(crate) light: LightState,
    pub(crate) cmds: [Option<Command>; CMDS],
    /// A default response to send once the commands above have been taken.
    pub(crate) deferred: Option<Deferred>,
    pub(crate) link_status_at: u32,
    /// When the neighbors age by one link status period next.
    pub(crate) age_at: u32,
    /// When the last Link Status went out.
    pub(crate) link_status_sent: u32,
    pub(crate) announce_at: Option<u32>,
    pub(crate) beacon_at: Option<u32>,
    /// When a frame of the network was last received.
    pub(crate) heard_at: u32,
    /// A scan is looking for the network this node belongs to (it went
    /// silent), not for one to join; the channel to return to if not found.
    pub(crate) searching: Option<u8>,
    pub(crate) leave_at: Option<u32>,
    /// The pending leave is to be followed by a rejoin.
    pub(crate) leave_rejoin: bool,
    /// A rejoin is wanted once the network has been found (secured or not).
    pub(crate) rejoin: Option<bool>,
    /// Rejoin request sent: secured?, deadline for the response, whom it was
    /// sent to and that router's depth.
    pub(crate) rejoin_wait: Option<(bool, u32, u16, u8)>,
    /// Frames of this PAN secured with a network key this node does not have.
    pub(crate) unknown_key_frames: u8,
    pub(crate) channel_at: Option<(u32, u8)>,
    /// A PAN identifier update on its way round the network: when to take it, what it is.
    pub(crate) pan_at: Option<(u32, u16)>,
    pub(crate) identify_until: Option<u32>,
    /// Trust Center link key exchange: 0 idle, 1 ask for the node descriptor,
    /// 2 wait for it, 3 request the key, 4 wait for it, 5 verify, 6 wait for
    /// the confirmation.
    pub(crate) tclk_step: u8,
    pub(crate) tclk_at: u32,
    pub(crate) tclk_tries: u8,
    /// Settings cluster: when BLE runs, the window for `BleMode::Window` in
    /// minutes, and whether the firmware says BLE is running now.
    pub(crate) ble_mode: BleMode,
    pub(crate) ble_window: u16,
    pub(crate) ble_active: bool,
    pub stats: Stats,
    /// The Diagnostics cluster's counters (`diag.rs`).
    pub(crate) diag: diag::Diag,
    /// Reporting of the attributes outside the light's three (`objects.rs`).
    pub(crate) xreports: [objects::XReport; objects::XREPORTS],
    pub(crate) xreports_dirty: bool,
    pub(crate) xreport_at: u32,
    /// Setting (object endpoint 7): end devices may join through this
    /// router. Endpoint 8 shows the BLE setting above as a window in
    /// minutes (0: no limit).
    pub(crate) allow_children: bool,
    /// Motion sensing (`sense.rs`).
    #[cfg(feature = "sense")]
    pub(crate) sense: sense::Sense,
}

impl<H: Hal> Stack<H> {
    /// Builds the stack and resumes the stored network, if there is one.
    pub fn new(hal: H, identity: Identity) -> Self {
        let mut s = Self::blank(hal, identity);
        let now = s.timer;
        s.restore(now);
        s
    }

    /// A stack that knows no network yet (`timer` is the time of its making).
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
            // not a valid list: the first pass of the timers publishes the real one
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
            // the stock bulb takes a colour temperature while it is off
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

    /// `new` into a place of the caller's (a static, say), with one copy of
    /// the stack on the way at most, where `new` may take two: on a small
    /// chip that is room the main stack does not have to keep free.
    pub fn new_in(slot: &mut Option<Self>, hal: H, identity: Identity) -> &mut Self {
        let s = slot.insert(Self::blank(hal, identity));
        let now = s.timer;
        s.restore(now);
        s
    }

    /// The stored state: counters, network, tables, children.
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

    /// Leaves the network for good: tells the neighbors (a Leave without
    /// "rejoin"), forgets the network and everything configured over it,
    /// and starts looking for a network anew. For a reset by a local action
    /// (BDB 9.5), which has to leave before it wipes the stored state; the
    /// stack has to be polled until it is no longer joined.
    pub fn leave(&mut self) {
        if self.state == State::Up {
            self.leave_rejoin = false;
            self.leave_at = Some(self.hal.now_ms());
        }
    }

    /// When the firmware is to run BLE (settings cluster `0xFC5F`).
    pub fn ble_mode(&self) -> BleMode {
        self.ble_mode
    }

    /// How long BLE runs after power-on in `BleMode::Window`, in minutes.
    pub fn ble_window_minutes(&self) -> u16 {
        self.ble_window
    }

    /// The firmware tells whether BLE is running now (attribute `0x0002`
    /// of the settings cluster).
    pub fn set_ble_active(&mut self, active: bool) {
        self.ble_active = active;
    }

    /// Number of devices in the child table.
    pub fn child_count(&self) -> usize {
        self.children.iter().filter(|c| c.used && c.joined).count()
    }

    /// The light's state, for attribute reads and reports. Call after every
    /// change.
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

    /// Runs the stack: handles received frames and expired timers. Returns a
    /// command for the light when the network sent one.
    pub fn poll(&mut self) -> Option<Command> {
        // the light has done what the last commands asked: now they are answered
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
                    // a probe of the motion sensing is a sample, not a frame for the stack
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
        // how long the light will be on its way, for the RemainingTime attributes
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
                    // told to leave and come back: a rejoin with the key we have
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
                // no answer: the next router that could be the parent is asked (R22 3.6.1.4.2)
                self.rejoin_wait = None;
                self.rejoin_next(now, secure);
            }
        }
        if self.unknown_key_frames >= 5
            && now.wrapping_sub(self.heard_at) > KEY_MISSED_MS
            && self.rejoin_wait.is_none()
            && self.nib.tc_ext != NO_TRUST_CENTER
        {
            // The neighbors talk under a network key we never got (it was
            // changed while this node was off): ask the Trust Center for it.
            // (Where there is none, there is nobody to ask: R22 4.6.3.3.2.)
            self.unknown_key_frames = 0;
            self.start_rejoin(now, false);
            return;
        }
        if now.wrapping_sub(self.heard_at) > SILENCE_MS {
            // Nothing heard for minutes: the network may have moved to another
            // channel while this node was off. Look for it.
            self.search_network(now);
            return;
        }
        if due(now, self.link_status_at) {
            // "every nwkLinkStatusPeriod seconds ... more frequently if
            // desired", with jitter (R22 3.6.3.4.1): the jitter only shortens it
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

    /// Next outgoing NWK frame counter. The stored value always runs ahead,
    /// so a power cut can never make the counter go back.
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
