//! Link layer: advertising and one connection as peripheral.
//!
//! The radio runs single events; here is when they happen, on which
//! channel, with which window, and what the control packets of the central
//! change (Bluetooth Core Specification Vol 6 Part B).

use crate::hal::{ConnEvent, Hal, ADV_PDU_MAX, CONNECT_LEN, LL_PAYLOAD_MAX, TICKS_PER_MS, TICKS_PER_US};
use crate::pdu::{self, ConnectInd};
use crate::{reached, Ble};

/// Advertising interval: 300 ms (as the stock) plus the specification's
/// random delay (advDelay, 0 to 10 ms). The delay is drawn from 0 to 9 ms:
/// an event starts from the main loop, which comes round a fraction of a
/// millisecond late when the Zigbee side has frames to handle, and that
/// lateness adds to the interval; the last millisecond is kept for it, so
/// that the intervals stay within advInterval + 10 ms (Core 5.4 Vol 6 Part
/// B 4.4.2.2.1).
const ADV_INTERVAL: u32 = 300 * TICKS_PER_MS;
const ADV_DELAY_MAX_MS: u32 = 9;
/// T_GAP(lim_adv_timeout) (Vol 3 Part C Appendix A): from power-up the bulb
/// advertises in the limited discoverable mode, as the stock does (Flags
/// 0x05), for no longer than this; then in the general discoverable mode
/// (Flags 0x06), so that it can still be found and connected to.
const LIM_ADV_TIMEOUT: u32 = 180_000 * TICKS_PER_MS;
/// Turnaround time between packets: the widening of the receive window
/// must stay below half an interval less this.
const T_IFS: u32 = 150 * TICKS_PER_US;
/// 1.25 ms, the unit of connection intervals and transmit windows.
const UNIT: u32 = 1250 * TICKS_PER_US;
/// How long before an event's start the radio is taken from Zigbee. A main
/// loop pass that takes longer than this just then costs that event.
const LEAD: u32 = 1500 * TICKS_PER_US;
/// This device's clock accuracy assumed for window widening, in ppm.
const OWN_PPM: u32 = 100;
/// Allowance for both sides' timing jitter in the receive window.
const JITTER: u32 = 32 * TICKS_PER_US;
/// The longest a connection event may run after its window, and how long
/// before the next one it must be over: the rest of the time is Zigbee's.
const EVENT_MAX: u32 = 4 * TICKS_PER_MS;
const EVENT_GAP: u32 = 2 * TICKS_PER_MS;
/// A connection that never heard the central is lost after this many
/// connection events (Vol 6 Part B 4.5.2), whether run or missed.
const ESTABLISH_EVENTS: u16 = 6;
/// Places kept free in the transmit queue before a received packet is
/// handled: its answer is at most three fragments or one control packet.
pub(crate) const TX_RESERVE: usize = 4;

/// Connection interval asked of the central: 50 ms keeps the app
/// responsive and leaves Zigbee most of the radio time.
const WANTED_INTERVAL: u16 = 40;
const WANTED_TIMEOUT: u16 = 400;
/// Not within the first five seconds of a connection (GAP's recommendation):
/// the central is busy discovering services then.
const PARAM_REQUEST_AFTER: u32 = 5000 * TICKS_PER_MS;

// control opcodes
const LL_CONNECTION_UPDATE_IND: u8 = 0x00;
const LL_CHANNEL_MAP_IND: u8 = 0x01;
const LL_TERMINATE_IND: u8 = 0x02;
const LL_ENC_REQ: u8 = 0x03;
const LL_UNKNOWN_RSP: u8 = 0x07;
const LL_FEATURE_REQ: u8 = 0x08;
const LL_FEATURE_RSP: u8 = 0x09;
const LL_VERSION_IND: u8 = 0x0c;
const LL_REJECT_IND: u8 = 0x0d;
const LL_PING_RSP: u8 = 0x13;
const LL_LENGTH_RSP: u8 = 0x15;

const ERR_REMOTE_USER_TERMINATED: u8 = 0x13;
const ERR_UNSUPPORTED_REMOTE_FEATURE: u8 = 0x1a;

/// Bluetooth 4.2; company 0x0211 (Telink, the chip's maker); our own subversion.
const VERSION: [u8; 6] = [LL_VERSION_IND, 0x08, 0x11, 0x02, 0x01, 0x00];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkState {
    Advertising,
    Connected,
}

#[derive(Debug, Clone, Copy)]
struct Update {
    instant: u16,
    win_size: u8,
    win_offset: u16,
    interval: u16,
    latency: u16,
    timeout: u16,
}

pub(crate) struct Link {
    pub state: LinkState,
    next_adv: u32,
    /// The latest moment for that event: until then it waits while the
    /// other radio user has traffic.
    adv_latest: u32,
    /// Still in the limited discoverable mode, entered at `start`.
    limited: bool,
    start: u32,
    p: ConnectInd,
    /// Counter of the next connection event.
    event: u16,
    unmapped: u8,
    /// Reference for event times: the last anchor point, or where the
    /// transmit window starts before one was seen.
    anchor: u32,
    /// Connection intervals from `anchor` to the next event.
    since: u32,
    /// When the central was last heard (the CONNECT_IND before that): the
    /// two clocks agreed then.
    sync: u32,
    /// Start of the supervision timeout: when the central was last heard,
    /// or the start of the transmit window of new connection parameters.
    alive: u32,
    /// Counter of the event that ran last: received packets belong to it.
    last_event: u16,
    /// Transmit window still open: no anchor seen since the connection or
    /// its last update.
    window: u32,
    established: bool,
    update: Option<Update>,
    map_update: Option<([u8; 5], u16)>,
    version_sent: bool,
    /// The central asked to end the connection; it ends after the event that acknowledged that.
    terminated: bool,
    /// We asked to end the connection: give up waiting for the acknowledgement then.
    closing: Option<u32>,
    param_request_at: Option<u32>,
    pub connected_at: u32,
}

impl Link {
    pub fn new(now: u32) -> Link {
        Link {
            state: LinkState::Advertising,
            // the first event a little after start: the light comes first
            next_adv: now.wrapping_add(20 * TICKS_PER_MS),
            adv_latest: now.wrapping_add(20 * TICKS_PER_MS),
            limited: true,
            start: now,
            p: ConnectInd {
                access_address: 0,
                crc_init: 0,
                win_size: 1,
                win_offset: 0,
                interval: 6,
                latency: 0,
                timeout: 10,
                channel_map: [0xff, 0xff, 0xff, 0xff, 0x1f],
                hop: 5,
                sca: 0,
            },
            event: 0,
            unmapped: 0,
            anchor: 0,
            since: 0,
            sync: 0,
            alive: 0,
            last_event: 0,
            window: 0,
            established: false,
            update: None,
            map_update: None,
            version_sent: false,
            terminated: false,
            closing: None,
            param_request_at: None,
            connected_at: 0,
        }
    }

    fn interval(&self) -> u32 {
        self.p.interval as u32 * UNIT
    }

    /// When the central's packet of the next event is expected.
    fn expected(&self) -> u32 {
        self.anchor.wrapping_add(self.since.wrapping_mul(self.interval()))
    }

    /// How far the two clocks may have drifted apart by `at`.
    fn drift(&self, at: u32) -> u32 {
        let ppm = pdu::sca_ppm(self.p.sca) + OWN_PPM;
        let elapsed_ms = at.wrapping_sub(self.sync) / TICKS_PER_MS;
        // ppm of a millisecond is a nanosecond: 16 ticks per 1000
        elapsed_ms * ppm * TICKS_PER_US / 1000
    }

    /// How much earlier than expected the receiver opens (and how much
    /// longer it stays open): the drift and both sides' jitter.
    fn widening(&self, at: u32) -> u32 {
        self.drift(at) + JITTER
    }

    /// Moves on to the next event: counters, and the changes whose instant it is.
    fn advance(&mut self) {
        self.since += 1;
        self.last_event = self.event;
        self.event = self.event.wrapping_add(1);
        self.apply_due();
    }

    /// Applies a stored change when the next event is its instant.
    fn apply_due(&mut self) {
        if let Some((map, instant)) = self.map_update {
            if instant == self.event {
                self.p.channel_map = map;
                self.map_update = None;
            }
        }
        if let Some(u) = self.update {
            if u.instant == self.event {
                // the new timing starts from where this event would have been
                self.anchor = self.expected().wrapping_add(u.win_offset as u32 * UNIT);
                self.since = 0;
                self.window = u.win_size as u32 * UNIT;
                // the supervision timeout starts over with the transmit window
                self.alive = self.anchor;
                self.p.interval = u.interval;
                self.p.latency = u.latency;
                self.p.timeout = u.timeout;
                self.update = None;
            }
        }
    }
}

impl<H: Hal> Ble<H> {
    pub(crate) fn link_poll(&mut self) {
        // polled far more often than the clock wraps (268 s)
        if self.link.limited && self.hal.now().wrapping_sub(self.link.start) >= LIM_ADV_TIMEOUT {
            self.link.limited = false;
        }
        if self.link.state == LinkState::Advertising {
            let now = self.hal.now();
            if !reached(now, self.link.next_adv) {
                return;
            }
            // a frame that has just come or gone is often followed by
            // another (an answer, a broadcast after a unicast): what comes
            // while BLE has the radio is lost
            if !reached(now, self.link.adv_latest) && self.hal.radio_busy() {
                return;
            }
            self.advertise(now);
        }
        if self.link.state == LinkState::Connected {
            self.connection_events();
        }
    }

    pub(crate) fn link_connected(&self) -> bool {
        self.link.state == LinkState::Connected
    }

    /// How long the radio is free before the link layer takes it for its next event.
    pub(crate) fn link_idle_time(&self, now: u32) -> u32 {
        let l = &self.link;
        let next = match l.state {
            LinkState::Advertising => l.next_adv,
            LinkState::Connected => {
                let expected = l.expected();
                expected.wrapping_sub(l.widening(expected) + LEAD)
            }
        };
        let left = next.wrapping_sub(now);
        if left as i32 > 0 {
            left
        } else {
            0
        }
    }

    /// Flags (LE only; limited, later general discoverable), the GAP
    /// service, the mesh name, and AwoX's manufacturer data: vendor 0x0160,
    /// product, low four bytes of the address.
    fn advertising_data(&self, out: &mut [u8; 31]) -> usize {
        let name = self.mesh.name();
        let shown = &name[..name.len().min(12)];
        let mut n = 0;
        let mut put = |bytes: &[u8]| {
            out[n..n + bytes.len()].copy_from_slice(bytes);
            n += bytes.len();
        };
        // the GAP service as an incomplete list (0x02), as the stock firmware has it
        put(&[0x02, 0x01, if self.link.limited { 0x05 } else { 0x06 }, 0x03, 0x02, 0x00, 0x18]);
        // Complete Local Name, or Shortened when it does not fit
        put(&[shown.len() as u8 + 1, if shown.len() == name.len() { 0x09 } else { 0x08 }]);
        put(shown);
        put(&[0x09, 0xff, 0x60, 0x01]);
        put(&self.identity.product.to_le_bytes());
        put(&self.addr[..4]);
        n
    }

    fn advertise(&mut self, now: u32) {
        let mut data = [0u8; 31];
        let n = self.advertising_data(&mut data);
        let mut adv = [0u8; ADV_PDU_MAX];
        let adv_len = pdu::advertising(pdu::ADV_IND, &self.addr, &data[..n], &mut adv);
        let mut data = [0u8; 31];
        let n = self.scan_response_data(&mut data);
        let mut rsp = [0u8; ADV_PDU_MAX];
        let rsp_len = pdu::advertising(pdu::SCAN_RSP, &self.addr, &data[..n], &mut rsp);

        // An event due while the other radio user has traffic
        // (`Hal::radio_busy`) waits for a quiet moment, but no longer than
        // the rest of the advDelay range: it still starts within
        // advInterval + advDelay of this one.
        let delay = self.hal.random() % (ADV_DELAY_MAX_MS * TICKS_PER_MS);
        self.link.next_adv = now.wrapping_add(ADV_INTERVAL + delay);
        self.link.adv_latest = now.wrapping_add(ADV_INTERVAL + ADV_DELAY_MAX_MS * TICKS_PER_MS);
        self.stats.adv_events += 1;
        self.air.listen_due = true;
        let mut connect = [0u8; CONNECT_LEN];
        let Some(end) = self.hal.advertise(&adv[..adv_len], &rsp[..rsp_len], &mut connect) else { return };
        let Some(p) = pdu::parse_connect(&connect) else { return };
        self.connected(p, end);
    }

    fn connected(&mut self, p: ConnectInd, end: u32) {
        let now = self.hal.now();
        let l = &mut self.link;
        l.state = LinkState::Connected;
        l.p = p;
        l.event = 0;
        l.unmapped = 0;
        // the transmit window opens 1.25 ms after the CONNECT_IND, plus its offset
        l.anchor = end.wrapping_add(UNIT + p.win_offset as u32 * UNIT);
        l.since = 0;
        l.sync = end;
        l.alive = end;
        l.last_event = 0;
        l.window = p.win_size as u32 * UNIT;
        l.established = false;
        l.update = None;
        l.map_update = None;
        l.version_sent = false;
        l.terminated = false;
        l.closing = None;
        l.param_request_at = Some(now.wrapping_add(PARAM_REQUEST_AFTER));
        l.connected_at = now;
        self.stats.connections += 1;
        self.hal.connection_reset();
        self.host_reset();
        self.mesh_session_end();
    }

    pub(crate) fn disconnected(&mut self) {
        self.link.state = LinkState::Advertising;
        self.link.next_adv = self.hal.now();
        self.link.adv_latest = self.link.next_adv;
        // nothing of this connection may be left in the radio's queue when it advertises
        self.hal.connection_reset();
        self.stats.disconnects += 1;
        self.host_reset();
        self.mesh_session_end();
        self.ota_link_lost();
    }

    /// Asks the central to end the connection. It ends when the central
    /// acknowledged that, or after the supervision timeout.
    pub(crate) fn terminate(&mut self) {
        if self.link.state == LinkState::Connected && self.link.closing.is_none() {
            self.hal.tx_push(pdu::LLID_CONTROL, &[LL_TERMINATE_IND, ERR_REMOTE_USER_TERMINATED]);
            let timeout = self.link.p.timeout as u32 * 10 * TICKS_PER_MS;
            self.link.closing = Some(self.hal.now().wrapping_add(timeout));
        }
    }

    fn connection_events(&mut self) {
        loop {
            let l = &self.link;
            let expected = l.expected();
            let widening = l.widening(expected);
            let start = expected.wrapping_sub(widening);
            let window = l.window + 2 * widening;
            let now = self.hal.now();

            // supervision
            let lost = if l.established {
                now.wrapping_sub(l.alive) > l.p.timeout as u32 * 10 * TICKS_PER_MS && reached(now, l.alive)
            } else {
                l.event >= ESTABLISH_EVENTS
            };
            if lost {
                self.disconnected();
                return;
            }
            // A central unheard for so long that the window would cover half
            // an interval is lost: beyond that, events could not be told apart
            // (and the radio would hardly leave BLE any more).
            if l.drift(expected) >= (l.interval() / 2).saturating_sub(T_IFS) {
                self.stats.links_lost += 1;
                self.disconnected();
                return;
            }
            if let Some(deadline) = l.closing {
                if self.hal.tx_idle() || reached(now, deadline) {
                    self.disconnected();
                    return;
                }
            }
            if !reached(now, start.wrapping_sub(LEAD)) {
                return;
            }
            let channel = pdu::next_channel(&mut self.link.unmapped, self.link.p.hop, &self.link.p.channel_map);
            if reached(now, start.wrapping_add(window)) {
                // the main loop was away for the whole event
                self.stats.events_skipped += 1;
                self.link.advance();
                continue;
            }
            let latest = start.wrapping_add(window);
            let room = self.link.interval().saturating_sub(window + widening + EVENT_GAP);
            let event = ConnEvent {
                channel,
                access_address: self.link.p.access_address,
                crc_init: self.link.p.crc_init,
                start,
                window,
                deadline: latest.wrapping_add(room.clamp(TICKS_PER_MS, EVENT_MAX)),
            };
            let result = self.hal.connection_event(&event);
            self.stats.events += 1;
            self.air.listen_due = true;
            if let Some(anchor) = result.anchor {
                self.stats.events_heard += 1;
                let l = &mut self.link;
                l.anchor = anchor;
                l.since = 0;
                l.sync = anchor;
                l.alive = anchor;
                l.window = 0;
                l.established = true;
            }
            self.link.advance();
            // received packets first: a control packet may change the next event
            self.link_receive();
            if self.link.terminated {
                self.disconnected();
            }
            return;
        }
    }

    /// Hands received packets on: control packets here, data to L2CAP.
    fn link_receive(&mut self) {
        let mut payload = [0u8; LL_PAYLOAD_MAX];
        while self.hal.tx_free() >= TX_RESERVE {
            let Some((llid, len)) = self.hal.rx_pop(&mut payload) else { return };
            let data = &payload[..len.min(LL_PAYLOAD_MAX)];
            match llid {
                pdu::LLID_CONTROL => self.on_control(data),
                pdu::LLID_START | pdu::LLID_CONTINUE => self.on_l2cap_fragment(llid == pdu::LLID_START, data),
                _ => {}
            }
        }
    }

    /// Takes packets left in the receive queue when the transmit queue was
    /// too full after the event, and runs the link's own timers.
    pub(crate) fn link_idle(&mut self) {
        self.link_receive();
        if self.link.terminated {
            self.disconnected();
            return;
        }
        let now = self.hal.now();
        if let Some(at) = self.link.param_request_at {
            if reached(now, at) && self.hal.tx_free() >= TX_RESERVE {
                self.link.param_request_at = None;
                if self.link.p.interval != WANTED_INTERVAL {
                    self.request_parameters();
                }
            }
        }
    }

    /// L2CAP Connection Parameter Update Request.
    fn request_parameters(&mut self) {
        let mut req = [0x12, 0x01, 0x08, 0x00, 0, 0, 0, 0, 0, 0, 0, 0];
        req[4..6].copy_from_slice(&WANTED_INTERVAL.to_le_bytes());
        req[6..8].copy_from_slice(&WANTED_INTERVAL.to_le_bytes());
        req[10..12].copy_from_slice(&WANTED_TIMEOUT.to_le_bytes());
        self.send_l2cap(crate::att::CID_SIGNALING, &req);
    }

    fn on_control(&mut self, p: &[u8]) {
        let Some(&opcode) = p.first() else { return };
        let u16_at = |i: usize| u16::from_le_bytes([p[i], p[i + 1]]);
        match opcode {
            LL_CONNECTION_UPDATE_IND if p.len() >= 12 => {
                let u = Update {
                    win_size: p[1],
                    win_offset: u16_at(2),
                    interval: u16_at(4),
                    latency: u16_at(6),
                    timeout: u16_at(8),
                    instant: u16_at(10),
                };
                // how far the instant lies after the event the packet came in
                let ahead = u.instant.wrapping_sub(self.link.last_event);
                if ahead >= 32767 || !(6..=3200).contains(&u.interval) {
                    // an instant in the past: the two sides disagree about the connection
                    self.link.terminated = true;
                } else if ahead == 0 {
                    // First heard on the instant itself: that packet was the
                    // new anchor already, without a transmit window.
                    self.link.p.interval = u.interval;
                    self.link.p.latency = u.latency;
                    self.link.p.timeout = u.timeout;
                } else {
                    self.link.update = Some(u);
                    self.link.apply_due();
                }
            }
            LL_CHANNEL_MAP_IND if p.len() >= 8 => {
                let map = [p[1], p[2], p[3], p[4], p[5] & 0x1f];
                let instant = u16_at(6);
                let ahead = instant.wrapping_sub(self.link.last_event);
                if ahead >= 32767 || pdu::used_channels(&map) < 2 {
                    self.link.terminated = true;
                } else if ahead == 0 {
                    self.link.p.channel_map = map;
                } else {
                    self.link.map_update = Some((map, instant));
                    self.link.apply_due();
                }
            }
            LL_TERMINATE_IND => self.link.terminated = true,
            LL_FEATURE_REQ => {
                // No optional link layer feature is implemented; requests of
                // such features (ping, data length, PHY...) get LL_UNKNOWN_RSP below.
                self.hal.tx_push(pdu::LLID_CONTROL, &[LL_FEATURE_RSP, 0, 0, 0, 0, 0, 0, 0, 0]);
            }
            LL_VERSION_IND => {
                if !self.link.version_sent {
                    self.link.version_sent = true;
                    self.hal.tx_push(pdu::LLID_CONTROL, &VERSION);
                }
            }
            LL_ENC_REQ => {
                self.hal.tx_push(pdu::LLID_CONTROL, &[LL_REJECT_IND, ERR_UNSUPPORTED_REMOTE_FEATURE]);
            }
            // answers to nothing we asked, and indications without an answer
            LL_UNKNOWN_RSP | LL_FEATURE_RSP | LL_REJECT_IND | LL_PING_RSP | LL_LENGTH_RSP => {}
            _ => {
                self.hal.tx_push(pdu::LLID_CONTROL, &[LL_UNKNOWN_RSP, opcode]);
            }
        }
    }
}
