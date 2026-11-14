//! Network layer: receiving and securing frames, neighbors and link status,
//! mesh routing (route discovery, many-to-one routes, route records, source
//! routes) and broadcast relay.

use super::diag::k;
use super::join::RESUME_LINK_STATUS_MS;
use super::parent::CAP_FFD;
use super::{
    Discovery, Held, Neighbor, Route, Seen, Stack, State, AGE_LIMIT, MAX_NWK_PAYLOAD, ROUTED, ROUTE_ACTIVE,
    ROUTE_DISCOVERY, ROUTE_FREE,
};
use crate::aes::Aes128;
use crate::hal::{trace, Hal, Trace, TxStatus};
use crate::mac::{self, Addr};
use crate::nwk::{self, Header};
use crate::util::{due, Buf, Reader};

/// An address conflict is reported once in this time: every router that
/// notices it would report it, and the reports of the others arrive too.
const CONFLICT_QUIET_MS: u32 = 5_000;
/// nwkcMaxBroadcastJitter: the most a report of an address conflict waits.
const CONFLICT_JITTER_MS: u32 = 64;
/// nwkNetworkBroadcastDeliveryTime: how long a broadcast is remembered.
const BROADCAST_MEMORY_MS: u32 = 9_000;
/// A broadcast under our own address with one of the last this many NWK
/// sequence numbers we used is our own, however late it comes back (see
/// `own_late_copy`). At the few frames a minute a bulb sends this covers
/// minutes; a device that really uses our address hits it by chance one
/// time in four at most, and shows itself by its IEEE address anyway.
const OWN_SEQ_WINDOW: u8 = 64;
/// A broadcast record makes room for a new one when the table is full
/// once it is this old. R22 3.6.5 keeps a record for
/// nwkNetworkBroadcastDeliveryTime and has a broadcast dropped when every
/// record is still in use ("should"); in a storm of several broadcasts a
/// second that drops the commands among them (a group Toggle) as well. The
/// copies of a broadcast come within about a second (relay jitter and the
/// retransmissions every nwkPassiveAckTimeout); the record of one older
/// than this goes first.
const BTT_KEEP_MS: u32 = 2_000;
/// nwkPassiveAckTimeout: how long the relays of the neighbors are waited
/// for before a broadcast goes out again. (A NIB attribute whose value the
/// stack profile sets; the Zigbee PRO one is 500 ms.) A neighbor relays
/// within nwkcMaxBroadcastJitter (64 ms) and a CSMA backoff of the moment
/// it hears the broadcast; 150 ms is room enough for that. The shorter
/// time keeps every copy we send within a fifth of a second of the first:
/// the AwoX stock firmware forgets a broadcast within half a second in a
/// storm and executes a later copy again (a group Toggle twice), and in a
/// house of stock and Rust bulbs our retransmissions 500 and 1000 ms later
/// did exactly that (docs/stress-results.md).
const BROADCAST_RETRY_MS: u32 = 150;
/// nwkMaxBroadcastRetries: how often a broadcast is transmitted again after
/// its first transmission, whether it started here or is passed on, while
/// some neighbor has not been heard passing it on (R22 3.6.5, passive
/// acknowledgement: "at most" this often). The NIB allows 0 to 5; R22 Table
/// 3-58 has 3 and the Zigbee PRO stack profile 2. One: in the stress house
/// of nine bulbs a second retransmission delivered no more group commands,
/// and every copy is airtime in a storm and a chance for a router with a
/// short memory to take it for a new broadcast.
const BROADCAST_RETRIES: u8 = 1;
/// nwkcInitialRREQRetries, nwkcRREQRetries: repeats of a route request by
/// its originator and by a router passing it on (R22 3.6.3.5.1, 3.6.3.5.2).
const RREQ_INITIAL_RETRIES: u8 = 3;
const RREQ_RELAY_RETRIES: u8 = 2;
/// nwkcRREQRetryInterval.
const RREQ_RETRY_MS: u32 = 254;
/// nwkcMinRREQJitter, nwkcMaxRREQJitter in milliseconds: a relayed route
/// request waits twice a random time between the two.
const RREQ_JITTER_MIN_MS: u32 = 2;
const RREQ_JITTER_MAX_MS: u32 = 128;
/// How long a frame waits for a route.
const HOLD_MS: u32 = 5_000;
/// nwkcRouteDiscoveryTime: how long a route discovery table entry lives, and
/// with it the routing table entry of a route still being discovered (R22
/// 3.6.3.6).
const DISCOVERY_MS: u32 = 10_000;
/// Link status periods after which a silent neighbor is forgotten.
const NEIGHBOR_FORGET: u8 = 40;
/// An early Link Status (see `on_link_status`): after a random pause of
/// at least `EARLY_LS_MIN_MS`, plus up to `EARLY_LS_SPREAD_MS`, and never
/// sooner than `EARLY_LS_GAP_MS` after the last one.
const EARLY_LS_MIN_MS: u32 = 100;
const EARLY_LS_SPREAD_MS: u32 = 400;
const EARLY_LS_GAP_MS: u32 = 1_000;
/// Failed transmissions in a row after which the link to a neighbor counts
/// as failed (R22 3.6.3.7: a failure counter per neighbor, and "it is
/// important not to initiate repair too frequently"). In a mesh of routers
/// one transmission that gets no acknowledgement after all MAC retries is
/// common (two neighbors that send to each other at once, a hidden node, a
/// receiver busy with its own frame); a link is not given up for that.
const LINK_FAILURES: u8 = 3;
/// A unicast the MAC could not deliver is tried again this many times, each
/// after a random pause, before it counts as undeliverable: with
/// `LINK_FAILURES` attempts in all, one frame that never gets through
/// makes the link a failed one.
const NWK_RETRIES: u8 = LINK_FAILURES - 1;
/// The pause before another attempt: at least this, plus up to `NWK_RETRY_SPREAD_MS`.
const NWK_RETRY_MIN_MS: u32 = 8;
const NWK_RETRY_SPREAD_MS: u32 = 40;

fn is_route_request(h: &Header, payload: &[u8]) -> bool {
    h.frame_type == nwk::FRAME_CMD && payload.first() == Some(&nwk::CMD_ROUTE_REQUEST)
}

/// The address a Network Status about a routing failure or an address
/// conflict is about.
fn about_of(payload: &[u8]) -> Option<u16> {
    Some(u16::from_le_bytes([*payload.get(2)?, *payload.get(3)?]))
}

impl<H: Hal> Stack<H> {
    /// A diagnostic record about the frame with header `h` (`Hal::trace`).
    fn trace(&mut self, kind: u8, detail: u8, h: &Header, peer: u16, aux: u32) {
        self.hal.trace(Trace { kind, detail, seq: h.seq, src: h.src, peer, aux });
    }

    /// A received frame dropped before routing; traced for unicasts only
    /// (every broadcast comes many times).
    fn trace_rx(&mut self, detail: u8, h: &Header, mac_src: u16, aux: u32) {
        if !nwk::is_broadcast(h.dst) {
            self.trace(trace::RX_DROP, detail, h, mac_src, aux);
        }
    }

    pub(crate) fn on_mpdu(&mut self, mpdu: &[u8], lqi: u8, end_us: u32) {
        let Some((h, off)) = mac::Header::parse(mpdu) else {
            self.diag.add(k::PACKET_VALIDATE_DROPS);
            return;
        };
        let broadcast = matches!(h.dst, Addr::None | Addr::Short(mac::BROADCAST));
        self.diag.add(if broadcast { k::MAC_RX_BCAST } else { k::MAC_RX_UCAST });
        if h.security {
            return;
        }
        let payload = &mpdu[off..];
        match h.frame_type {
            mac::FRAME_BEACON => self.on_beacon(&h, payload, lqi),
            mac::FRAME_CMD => self.on_mac_command(&h, payload),
            mac::FRAME_DATA => {
                // Green Power frames (NWK protocol version 3) come from devices that are not on the network
                if self.state == State::Up && payload.first().is_some_and(|b| (b >> 2) & 0x0F == 3) {
                    let _ = self.on_gpdf(h.seq, h.src, payload, lqi, end_us);
                    return;
                }
                // inter-PAN frames (NWK frame type 3): touchlink
                if matches!(self.state, State::Up | State::Down | State::Scan) && payload.first().is_some_and(|b| b & 3 == 3) {
                    let _ = self.on_interpan(&h, payload, lqi);
                    return;
                }
                if !matches!(self.state, State::Up | State::WaitKey) || h.dst_pan != self.nib.pan {
                    return;
                }
                let to_us = matches!(h.dst, Addr::Short(d) if d == self.nib.short || d == mac::BROADCAST);
                if let (true, Addr::Short(src)) = (to_us, h.src) {
                    self.on_nwk(src, payload, lqi);
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------ transmit

    /// The MAC frame that carries one NWK frame to a neighbor, or to everyone
    /// in range with `mac_dst` 0xFFFF; secured when the header asks for it.
    pub(crate) fn nwk_frame(&mut self, mac_dst: u16, h: &Header, payload: &[u8]) -> Buf {
        let mut f = Buf::new();
        mac::Header {
            frame_type: mac::FRAME_DATA,
            security: false,
            pending: false,
            ack_request: mac_dst != mac::BROADCAST,
            seq: self.next_mac_seq(),
            dst_pan: self.nib.pan,
            dst: Addr::Short(mac_dst),
            src_pan: self.nib.pan,
            src: Addr::Short(self.nib.short),
        }
        .write(&mut f);
        let at = f.len();
        h.write(&mut f);
        if h.security {
            let counter = self.next_fc();
            nwk::secure(&self.key, self.nib.key_seq, self.ext, counter, &mut f, at, payload);
        } else {
            f.bytes(payload);
        }
        f
    }

    /// Secures (when the header asks for it) and transmits one NWK frame to
    /// a neighbor, or to everyone in range with `mac_dst` 0xFFFF.
    pub(crate) fn transmit_nwk(&mut self, mac_dst: u16, h: &Header, payload: &[u8]) -> TxStatus {
        let f = self.nwk_frame(mac_dst, h, payload);
        if !f.ok() {
            self.trace(trace::TX, trace::TX_TOO_LONG, h, mac_dst, 0);
            return TxStatus::Ok;
        }
        // a child that sleeps gets its frames when it polls
        if let Some(i) = self.child_index(mac_dst) {
            if self.children[i].sleepy() {
                self.hold_for_child(mac_dst, &f);
                self.box_frame(mac_dst, h, payload, 0, 3);
                self.trace(trace::TX, trace::TX_FOR_CHILD, h, mac_dst, 0);
                if h.src == self.nib.short && h.frame_type == nwk::FRAME_DATA {
                    self.diag.add(k::APS_TX_UCAST_SUCCESS);
                }
                return TxStatus::Ok;
            }
        }
        self.stats.tx = self.stats.tx.wrapping_add(1);
        let status = self.mac_transmit(f.as_slice());
        if mac_dst != mac::BROADCAST && h.src == self.nib.short && h.frame_type == nwk::FRAME_DATA {
            // an APS message of ours on its first hop
            let retries = self.diag.last_attempts.saturating_sub(1) as u32;
            self.diag.add_n(k::APS_MAC_RETRIES, retries);
            if status == TxStatus::Ok {
                self.diag.add(k::APS_TX_UCAST_SUCCESS);
            }
        }
        if status != TxStatus::Ok {
            self.stats.tx_failed = self.stats.tx_failed.wrapping_add(1);
        }
        let detail = match status {
            TxStatus::Ok => trace::TX_OK,
            TxStatus::NoAck => trace::TX_NO_ACK,
            TxStatus::ChannelBusy => trace::TX_BUSY,
        };
        self.trace(trace::TX, detail, h, mac_dst, 0);
        self.box_frame(mac_dst, h, payload, self.diag.last_attempts, status as u8);
        if mac_dst != mac::BROADCAST {
            if let Some(i) = self.neighbor_index(mac_dst) {
                let n = &mut self.neighbors[i];
                n.failures = if status == TxStatus::Ok { 0 } else { n.failures.saturating_add(1) };
            }
        }
        status
    }

    /// A frame sent, into the black box: its kind and, from the plain
    /// payload, the NWK command or the APS cluster. `status`: 0 delivered,
    /// 1 no acknowledgement, 2 channel busy, 3 held for a sleeping child.
    fn box_frame(&mut self, mac_dst: u16, h: &Header, payload: &[u8], attempts: u8, status: u8) {
        use super::blackbox::kind;
        let mut k = h.frame_type & 3;
        if h.security {
            k |= kind::SECURED;
        }
        if mac_dst == mac::BROADCAST {
            k |= kind::BROADCAST;
        }
        if h.src != self.nib.short {
            k |= kind::RELAYED;
        }
        let mut id = 0;
        if h.frame_type == nwk::FRAME_CMD {
            if let Some(&c) = payload.first() {
                k |= kind::ID_COMMAND;
                id = c as u16;
            }
        } else if h.frame_type == nwk::FRAME_DATA {
            if let Some(&fc) = payload.first() {
                // APS: data frames and acknowledgements (with the full format) name the cluster
                let at = match (fc & 3, (fc >> 2) & 3) {
                    (0, 0) | (0, 2) => Some((2, kind::ID_CLUSTER)),
                    (0, 3) => Some((3, kind::ID_CLUSTER)),
                    (0, _) => Some((1, kind::ID_CLUSTER)),
                    (2, _) if fc & 0x10 == 0 => Some((2, kind::ID_APS_ACK)),
                    _ => None,
                };
                if let Some((at, what)) = at {
                    if let (Some(&lo), Some(&hi)) = (payload.get(at), payload.get(at + 1)) {
                        k |= what;
                        id = u16::from_le_bytes([lo, hi]);
                    }
                }
            }
        }
        let now = self.hal.now_ms();
        if let Some(b) = self.hal.black_box() {
            b.frame(now, mac_dst, h.dst, h.src, h.seq, k, id, attempts, status);
        }
    }

    /// Header of a frame this node originates.
    pub(crate) fn new_header(&mut self, frame_type: u8, dst: u16, radius: u8) -> Header {
        let seq = self.next_nwk_seq();
        let mut h = Header::new(frame_type, dst, self.nib.short, radius, seq);
        if frame_type == nwk::FRAME_CMD {
            // every command names its sender, and whom it is for when that
            // is known (R22 3.6.1.9.1)
            h.src_ext = Some(self.ext);
            h.dst_ext = self.ext_of(dst);
        }
        h
    }

    /// The IEEE address of a device this node talks to directly, for the
    /// destination IEEE address of a command (R22 3.6.1.9.1: "if it is
    /// known"). Only what frames secured by the device itself have shown,
    /// that is a child or a neighbor: the receiver takes a wrong address
    /// here for an address conflict (3.6.1.9.2).
    pub(crate) fn ext_of(&self, short: u16) -> Option<u64> {
        if let Some(i) = self.child_index(short) {
            return Some(self.children[i].ext);
        }
        self.neighbor_index(short).map(|i| self.neighbors[i].ext).filter(|e| *e != 0)
    }

    /// Sends a frame this node originates to a unicast or broadcast address.
    pub(crate) fn nwk_send(&mut self, h: &Header, payload: &[u8]) {
        if nwk::is_broadcast(h.dst) {
            self.originate_broadcast(h, payload);
        } else if h.dst != self.nib.short {
            self.route_out(h, payload);
        }
    }

    /// Sends application data, preceded by a route record when the
    /// destination is a concentrator that still needs one.
    pub(crate) fn nwk_data(&mut self, dst: u16, payload: &[u8]) {
        // (only the APS sends data frames)
        self.diag.add(if nwk::is_broadcast(dst) { k::APS_TX_BCAST } else { k::APS_SENT });
        let mut h = self.new_header(nwk::FRAME_DATA, dst, nwk::DEFAULT_RADIUS);
        if !nwk::is_broadcast(dst) {
            // (a broadcast has "discover route" 0: R22 Table 3-45)
            h.discover = 1;
            if let Some(i) = self.route_index(dst) {
                let r = self.routes[i];
                // The record goes before every frame for as long as it is
                // "required" (R22 3.6.3.5.4): until a source-routed frame
                // of the concentrator shows that it has the route (3.6.3.3.2).
                if r.status == ROUTE_ACTIVE && r.many_to_one && (r.record_due || r.no_cache) {
                    let rh = self.new_header(nwk::FRAME_CMD, dst, nwk::DEFAULT_RADIUS);
                    self.transmit_nwk(r.next, &rh, &[nwk::CMD_ROUTE_RECORD, 0]);
                }
            }
        }
        self.nwk_send(&h, payload);
    }

    fn originate_broadcast(&mut self, h: &Header, payload: &[u8]) {
        let now = self.hal.now_ms();
        // (our own broadcasts are always remembered: their echo must be known for one)
        self.seen_before(h.src, h.seq, now, true);
        self.transmit_nwk(mac::BROADCAST, h, payload);
        self.broadcast_to_sleepy(h, payload);
        let (gap, retries) = if is_route_request(h, payload) {
            (RREQ_RETRY_MS, RREQ_INITIAL_RETRIES)
        } else {
            (BROADCAST_RETRY_MS, BROADCAST_RETRIES)
        };
        self.queue_broadcast(h, payload, now.wrapping_add(gap), retries);
    }

    fn queue_broadcast(&mut self, h: &Header, payload: &[u8], at: u32, times: u8) {
        if payload.len() > MAX_NWK_PAYLOAD {
            return;
        }
        match self.bcast.iter().position(|e| !e.used) {
            Some(i) => {
                let e = &mut self.bcast[i];
                e.used = true;
                e.header = *h;
                e.len = payload.len() as u8;
                e.payload[..payload.len()].copy_from_slice(payload);
                e.at = at;
                e.left = times;
                e.heard = 0;
            }
            // no room to wait: one transmission now is better than none
            None => {
                self.diag.add(k::BCAST_QUEUE_FULL);
                self.diag.add(k::BUFFER_ALLOCATE_FAILURES);
                self.trace(trace::BROADCAST, 2, h, mac::BROADCAST, 0);
                self.transmit_nwk(mac::BROADCAST, h, payload);
            }
        }
    }

    // ------------------------------------------------------------ receive

    pub(crate) fn on_nwk(&mut self, mac_src: u16, p: &[u8], lqi: u8) {
        let Some((mut h, hl)) = Header::parse(p) else {
            self.diag.add(k::PACKET_VALIDATE_DROPS);
            return;
        };
        let now = self.hal.now_ms();
        let mut frame = [0u8; 127];
        let n = p.len();
        let Some(copy) = frame.get_mut(..n) else {
            return;
        };
        copy.copy_from_slice(p);
        let (at, len) = if h.security {
            if self.state != State::Up {
                self.trace_rx(trace::RX_NOT_UP, &h, mac_src, 0);
                return;
            }
            let Some(aux) = nwk::parse_aux(p, hl) else {
                self.diag.add(k::PACKET_VALIDATE_DROPS);
                return;
            };
            // A frame counter belongs to the key it was used with (R22
            // 4.3.1.2 step 3): what is remembered of a sender is the lowest
            // counter it may use next under the key it was last heard with.
            // Under another key its counter may start anew (4.6.3.4.2),
            // with one exception: a sender that has used the active key
            // does not go back to the previous one (4.7.3.10.6), so such a
            // frame is an old one. (`fc_next` 0: nothing heard from it yet.)
            let (fc_seq, fc_next) = match (self.child_by_ext(aux.ext), self.neighbor_by_ext(aux.ext)) {
                (Some(i), _) => (self.children[i].fc_seq, self.children[i].fc_next),
                (None, Some(i)) => (self.neighbors[i].fc_seq, self.neighbors[i].fc_next),
                (None, None) => (aux.key_seq, 0),
            };
            let previous = self.nib.alt_valid && self.nib.alt_old && aux.key_seq == self.nib.alt_seq;
            let moved_on = previous && fc_seq == self.nib.key_seq && fc_next != 0;
            let floor = if aux.key_seq == fc_seq { fc_next } else { 0 };
            if moved_on || aux.counter < floor {
                self.stats.replays = self.stats.replays.wrapping_add(1);
                self.diag.add(k::NWK_FC_FAILURE);
                if !nwk::is_broadcast(h.dst) && h.dst != self.nib.short {
                    // a copy of a frame this node has taken for relaying already
                    self.diag.add(k::RELAY_DROP_DUPLICATE);
                }
                self.trace_rx(trace::RX_REPLAY, &h, mac_src, aux.counter);
                return;
            }
            let plain = if aux.key_seq == self.nib.key_seq {
                nwk::unsecure(&self.key, &aux, &mut frame[..n], hl)
            } else if self.nib.alt_valid && aux.key_seq == self.nib.alt_seq {
                nwk::unsecure(&Aes128::with_engine(&self.nib.alt_key, H::AES), &aux, &mut frame[..n], hl)
            } else {
                // a key this node was never given
                self.unknown_key_frames = self.unknown_key_frames.saturating_add(1);
                self.diag.add(k::NWK_DECRYPT_FAILURES);
                self.trace_rx(trace::RX_UNKNOWN_KEY, &h, mac_src, 0);
                return;
            };
            let Some(plen) = plain else {
                self.stats.bad_mic = self.stats.bad_mic.wrapping_add(1);
                self.diag.add(k::NWK_DECRYPT_FAILURES);
                self.trace_rx(trace::RX_BAD_MIC, &h, mac_src, 0);
                return;
            };
            // Someone already uses the key that was to come: from here on it
            // is the active one, whether the Switch Key was heard or not
            // (R22 4.3.1.2 step 7, 4.7.3.10.5).
            if aux.key_seq != self.nib.key_seq && !self.nib.alt_old {
                self.switch_network_key();
            }
            // A frame under our own address is the echo of something we sent,
            // unless another device uses the address: one that transmits
            // under it, one whose extended address the header shows, or the
            // sender of a broadcast we never originated.
            let me = self.nib.short;
            let echo = h.src == me
                && mac_src != me
                && h.src_ext.is_none_or(|e| e == self.ext)
                && (!nwk::is_broadcast(h.dst) || self.broadcast_seen(me, h.seq, now) || self.own_late_copy(&h));
            let mut clash = self.address_seen(mac_src, aux.ext);
            if let Some(ext) = h.src_ext {
                clash |= self.address_seen(h.src, ext);
            }
            if echo {
                self.trace_rx(trace::RX_ECHO, &h, mac_src, 0);
                return;
            }
            if h.src == me || mac_src == me {
                self.trace_rx(trace::RX_OWN_ADDRESS, &h, mac_src, 0);
                // Not our echo: the frame is another device's, and not to be
                // acted on. If it shows the other device's IEEE address, the
                // conflict has been dealt with above. A broadcast we never
                // sent shows it as well, but only once this node has been
                // up for as long as a broadcast is remembered: until then it
                // may be one of our own from before a restart (R22
                // 3.6.1.9.2, 3.6.5).
                if nwk::is_broadcast(h.dst) && now.wrapping_sub(self.booted) > BROADCAST_MEMORY_MS {
                    self.address_conflict(me);
                }
                return;
            }
            self.neighbor_heard(mac_src, aux.ext, lqi, aux.counter, aux.key_seq);
            self.heard_at = now;
            // A frame for our address that names another device as its
            // destination: that device uses our address (R22 3.6.1.9.2).
            if h.dst == me && h.dst_ext.is_some_and(|e| e != self.ext) {
                self.trace_rx(trace::RX_OWN_ADDRESS, &h, mac_src, 1);
                self.address_conflict(me);
                return;
            }
            // A broadcast data frame that brought an address conflict to
            // light is not passed on or acted on (R22 3.6.1.9).
            if clash && nwk::is_broadcast(h.dst) && h.frame_type == nwk::FRAME_DATA {
                return;
            }
            // (What follows changes the child table: only here, where the
            // frame is known to come from a node of the network.)
            if h.end_device_initiator {
                // An end device says it is talking to its parent (R22
                // 3.6.2.2). If it is not our child, it is not served, and is
                // told to rejoin: it has been forgotten here. The bit means
                // something on the first hop only (a parent clears it when
                // it passes the frame on, 3.3.1.1.9), so a frame that still
                // has it after a relay is left alone; and a device asking to
                // rejoin is on its way to becoming our child.
                h.end_device_initiator = false;
                let command = frame.get(hl + nwk::AUX_LEN).filter(|_| h.frame_type == nwk::FRAME_CMD && plen > 0);
                let rejoining = command == Some(&nwk::CMD_REJOIN_REQUEST);
                if mac_src == h.src && !rejoining && self.end_device_child(h.src).is_none() {
                    self.trace_rx(trace::RX_NOT_A_CHILD, &h, mac_src, 0);
                    self.tell_to_rejoin(h.src, false);
                    return;
                }
            }
            // A frame of one of our end-device children that another device
            // passed on: the child has a new parent (R22 3.6.2.3). The echo
            // of a broadcast we relayed for the child ourselves is known
            // from the broadcast transaction table.
            if mac_src != h.src && !(nwk::is_broadcast(h.dst) && self.broadcast_seen(h.src, h.seq, now)) {
                if let Some(i) = self.end_device_child(h.src) {
                    self.remove_child(i);
                }
            }
            (hl + nwk::AUX_LEN, plen)
        } else {
            // Rejoining without the network key is done in the clear: a
            // device asking us, or the answer to our own request.
            if self.state == State::Up && h.frame_type == nwk::FRAME_CMD && h.dst == self.nib.short {
                match p.get(hl) {
                    Some(&nwk::CMD_REJOIN_REQUEST) => self.on_rejoin_request(&h, mac_src, &p[hl..], false),
                    Some(&nwk::CMD_REJOIN_RESPONSE) if matches!(self.rejoin_wait, Some((false, ..))) => {
                        self.on_rejoin_response(&h, mac_src, &p[hl..])
                    }
                    _ => {}
                }
                return;
            }
            // Otherwise the one unsecured frame a node takes is the one that
            // brings it the network key, after association or such a rejoin.
            if self.state != State::WaitKey || h.dst != self.nib.short || h.frame_type != nwk::FRAME_DATA {
                self.trace_rx(trace::RX_NOT_SECURED, &h, mac_src, 0);
                return;
            }
            (hl, n - hl)
        };
        let payload = &frame[at..at + len];

        if is_route_request(&h, payload) {
            self.on_route_request(&h, mac_src, lqi, payload, now);
        } else if nwk::is_broadcast(h.dst) {
            // the broadcast addresses a router answers to (R22 Table 3-69)
            if !matches!(h.dst, nwk::BROADCAST_ALL | nwk::BROADCAST_RX_ON | nwk::BROADCAST_ROUTERS) {
                return;
            }
            if h.frame_type == nwk::FRAME_CMD && payload.first() == Some(&nwk::CMD_LINK_STATUS) {
                // A Link Status goes one hop, once (R22 3.6.3.4.1): it is
                // never passed on and needs no broadcast transaction record,
                // which every neighbor's would otherwise take up in turn.
                if mac_src == h.src {
                    self.on_link_status(h.src, payload);
                }
                return;
            }
            if mac_src != h.src {
                if let Some(i) = self.neighbor_index(mac_src) {
                    self.neighbors[i].relays = true;
                }
            }
            // already seen, or no room to remember it: dropped (R22 3.6.5);
            // a copy is the passive acknowledgement of its sender
            if self.seen_before(h.src, h.seq, now, false) != Some(false) {
                self.broadcast_heard(h.src, h.seq, mac_src);
                return;
            }
            self.neighbor_announced(&h, mac_src, payload);
            if h.radius > 1 {
                let mut fwd = h;
                fwd.radius -= 1;
                let jitter = self.hal.random() % 48;
                self.queue_broadcast(&fwd, payload, now.wrapping_add(jitter), 1 + BROADCAST_RETRIES);
                self.broadcast_heard(h.src, h.seq, mac_src);
                self.broadcast_to_sleepy(&fwd, payload);
            }
            self.deliver(&h, mac_src, payload);
        } else if h.dst == self.nib.short {
            if h.relays.is_some() {
                self.source_routed_by(h.src);
            }
            self.deliver(&h, mac_src, payload);
        } else if self.state == State::Up {
            self.relay(h, mac_src, payload);
        }
    }

    fn deliver(&mut self, h: &Header, mac_src: u16, payload: &[u8]) {
        match h.frame_type {
            nwk::FRAME_DATA => self.on_aps(h, payload),
            nwk::FRAME_CMD => self.on_nwk_command(h, mac_src, payload),
            _ => {}
        }
    }

    /// A broadcast under our own address that the broadcast transaction table
    /// no longer knows, but that is evidently one of ours relayed late: its
    /// header names our own IEEE address, or it carries one of the NWK
    /// sequence numbers we used last. R22 3.6.1.9.2 takes a new broadcast
    /// record with our own source address for a conflict once
    /// nwkNetworkBroadcastDeliveryTime has passed; but routers that forget a
    /// broadcast within a second and relay a copy many seconds later (the
    /// AwoX stock firmware, docs/address-conflicts.md) send our own frames
    /// back after that time, and a new address is then taken for nothing,
    /// again and again. A frame stating the pair (our address, our IEEE
    /// address) is consistent (3.6.1.9.2, first paragraphs); another device
    /// under our address shows its own IEEE address in its Device_annce and
    /// NWK commands, which the checks of `address_seen` catch.
    fn own_late_copy(&self, h: &Header) -> bool {
        h.src_ext == Some(self.ext) || self.nwk_seq.wrapping_sub(h.seq) < OWN_SEQ_WINDOW
    }

    /// `mac_src` was heard sending the broadcast (`src`, `seq`): a passive
    /// acknowledgement for our (re)transmissions of it.
    fn broadcast_heard(&mut self, src: u16, seq: u8, mac_src: u16) {
        let Some(n) = self.neighbor_index(mac_src) else {
            return;
        };
        for e in self.bcast.iter_mut().filter(|e| e.used && e.header.src == src && e.header.seq == seq) {
            e.heard |= 1 << n;
        }
    }

    /// Every neighbor that passes broadcasts on and is heard lately is in
    /// `heard`. (A neighbor never heard passing one on, an end device or a
    /// router that does not relay, is not waited for: it would make every
    /// broadcast go out three times, and every late copy may be taken for a
    /// new broadcast by a router that forgets them sooner, as the AwoX
    /// stock firmware does: docs/address-conflicts.md.) Without any
    /// neighbor that passes broadcasts on there is nothing to listen for,
    /// and the retransmission goes.
    fn passively_acknowledged(&self, heard: u16) -> bool {
        let relaying = |n: &Neighbor| n.used && n.relays && n.age <= AGE_LIMIT;
        self.neighbors.iter().any(relaying) && self.neighbors.iter().enumerate().all(|(i, n)| !relaying(n) || heard & (1 << i) != 0)
    }

    pub(crate) fn broadcast_seen(&mut self, src: u16, seq: u8, now: u32) -> bool {
        self.seen.iter().any(|e| e.used && e.src == src && e.seq == seq && !due(now, e.expires))
    }

    /// Remembers a broadcast in the broadcast transaction table. `Some(true)`:
    /// it was already there. `Some(false)`: it is new and now remembered.
    /// `None`: it is new, but every record is still in use; a broadcast that
    /// cannot be remembered is not processed (R22 3.6.5: "should be
    /// dropped"), or copies of it would be taken for new ones. With `force`
    /// (our own broadcasts) the record closest to its end makes room.
    fn seen_before(&mut self, src: u16, seq: u8, now: u32, force: bool) -> Option<bool> {
        let mut slot = None;
        let mut oldest = u32::MAX;
        for (i, e) in self.seen.iter_mut().enumerate() {
            if e.used && due(now, e.expires) {
                e.used = false;
            }
            if e.used && e.src == src && e.seq == seq {
                return Some(true);
            }
            let left = if e.used { e.expires.wrapping_sub(now) } else { 0 };
            if left < oldest {
                oldest = left;
                slot = Some(i);
            }
        }
        // a record still in use goes for our own broadcasts, or once it is
        // old enough that no copy is to be expected any more (`BTT_KEEP_MS`)
        let i = slot?;
        if self.seen[i].used && !force && oldest > BROADCAST_MEMORY_MS - BTT_KEEP_MS {
            self.diag.add(k::BCAST_DROP_TABLE_FULL);
            self.hal.trace(Trace { kind: trace::BROADCAST, detail: 1, seq, src, peer: 0, aux: 0 });
            return None;
        }
        self.seen[i] = Seen { used: true, src, seq, expires: now.wrapping_add(BROADCAST_MEMORY_MS) };
        Some(false)
    }

    fn relay(&mut self, mut h: Header, mac_src: u16, payload: &[u8]) {
        self.trace(trace::RELAY, 0, &h, mac_src, h.radius as u32);
        if h.radius <= 1 {
            self.trace(trace::DROP, trace::DROP_RADIUS, &h, mac_src, 0);
            return;
        }
        h.radius -= 1;
        self.stats.relayed = self.stats.relayed.wrapping_add(1);
        let command = if h.frame_type == nwk::FRAME_CMD { payload.first().copied() } else { None };
        if let Some((count, index, list)) = h.relays {
            // source routed: the list runs from the hop next to the
            // destination back towards the source, and `index` points at us
            let i = index as usize;
            if i >= count as usize || list[i] != self.nib.short {
                self.trace(trace::DROP, trace::DROP_SOURCE_ROUTE, &h, mac_src, index as u32);
                return;
            }
            self.source_routed_by(h.src);
            let next = if i == 0 { h.dst } else { list[i - 1] };
            h.relays = Some((count, index.saturating_sub(1), list));
            self.send_to(next, &h, payload, NWK_RETRIES);
            return;
        }
        match command {
            Some(nwk::CMD_ROUTE_RECORD) => {
                // A route record collects the relays it passes. If it cannot
                // go on it is dropped, and nobody is told (R22 3.6.3.5.4).
                let n = payload.len();
                let mut p = [0u8; MAX_NWK_PAYLOAD];
                if n < 2 || n + 2 > p.len() {
                    return;
                }
                p[..n].copy_from_slice(payload);
                p[1] = p[1].wrapping_add(1);
                p[n..n + 2].copy_from_slice(&self.nib.short.to_le_bytes());
                self.route_try(&h, &p[..n + 2], NWK_RETRIES);
                return;
            }
            // a Route Reply is made anew at every hop: one that is not for
            // us is nobody's (R22 3.6.2.2)
            Some(nwk::CMD_ROUTE_REPLY) => {
                self.trace(trace::DROP, trace::DROP_ROUTE_REPLY, &h, mac_src, 0);
                return;
            }
            Some(nwk::CMD_NETWORK_STATUS) => {
                // On its way to an end-device child, a report of a broken
                // link also tells us that the route is gone (R22 3.6.3.7.1).
                if let (Some(&code), Some(about), true) = (payload.get(1), about_of(payload), self.end_device_child(h.dst).is_some()) {
                    if matches!(code, nwk::STATUS_TREE_LINK_FAILURE | nwk::STATUS_LINK_FAILURE) {
                        self.route_drop(about);
                    }
                }
                // Handled like a data frame (3.6.2.2), except that its own
                // failure is reported to nobody (3.6.3.3); "many-to-one
                // route failure" goes on through some other router when
                // this one has no way to the concentrator (3.6.3.7.1).
                // (not over a link that has failed: see `route_try`)
                let (sent, mac_failed) = match self.next_hop(h.dst).filter(|&n| !self.link_failed(n)) {
                    Some(next) => {
                        let ok = self.transmit_nwk(next, &h, payload) == TxStatus::Ok;
                        if !ok {
                            self.link_lost(next, h.dst);
                        }
                        (ok, !ok)
                    }
                    None => (false, false),
                };
                let drop = if mac_failed { k::RELAY_DROP_MAC_FAIL } else { k::RELAY_DROP_NO_ROUTE };
                if sent {
                    self.diag.add(k::RELAYED_UCAST);
                } else if payload.get(1) == Some(&nwk::STATUS_MANY_TO_ONE_FAILURE) {
                    if let Some(via) = self.random_router(mac_src, mac_src) {
                        match self.transmit_nwk(via, &h, payload) {
                            TxStatus::Ok => self.diag.add(k::RELAYED_UCAST),
                            _ => self.diag.add(k::RELAY_DROP_MAC_FAIL),
                        }
                    } else {
                        self.diag.add(drop);
                        self.trace(trace::DROP, trace::DROP_UNREPORTED, &h, mac_src, 0);
                    }
                } else {
                    self.diag.add(drop);
                    self.trace(trace::DROP, trace::DROP_UNREPORTED, &h, mac_src, 0);
                }
                return;
            }
            _ => {}
        }
        if h.frame_type == nwk::FRAME_DATA {
            self.route_record_for_child(&h);
        }
        self.route_out(&h, payload);
    }

    /// The concentrator `src` sent a source-routed frame through or to this
    /// node: it has the route, and if it keeps routes, it needs no more
    /// route records (R22 3.6.3.3.2, last paragraph).
    fn source_routed_by(&mut self, src: u16) {
        if let Some(i) = self.route_index(src) {
            if !self.routes[i].no_cache {
                self.routes[i].record_due = false;
            }
        }
    }

    /// Before the data frame of an end-device child goes towards a
    /// concentrator, a route record goes there in the child's name: that is
    /// how the concentrator learns the way back to a device that cannot say
    /// it itself (R22 3.6.3.5.4, 3.4.5.2, 3.4.5.3). Sent every time; the
    /// bookkeeping that would spare some of them is optional.
    fn route_record_for_child(&mut self, h: &Header) {
        let (Some(c), Some(r)) = (self.end_device_child(h.src), self.route_index(h.dst)) else {
            return;
        };
        let (child, route) = (self.children[c], self.routes[r]);
        if route.status != ROUTE_ACTIVE || !route.many_to_one {
            return;
        }
        let seq = self.next_nwk_seq();
        let mut rh = Header::new(nwk::FRAME_CMD, h.dst, child.short, nwk::DEFAULT_RADIUS, seq);
        rh.src_ext = Some(child.ext);
        rh.dst_ext = self.ext_of(h.dst);
        // one relay so far: this router
        let me = self.nib.short.to_le_bytes();
        self.transmit_nwk(route.next, &rh, &[nwk::CMD_ROUTE_RECORD, 1, me[0], me[1]]);
    }

    // ------------------------------------------------------------ routing

    pub(crate) fn route_index(&self, dst: u16) -> Option<usize> {
        self.routes.iter().position(|r| r.status != ROUTE_FREE && r.dst == dst)
    }

    fn route_drop(&mut self, dst: u16) {
        if let Some(i) = self.route_index(dst) {
            self.routes[i].status = ROUTE_FREE;
        }
    }

    /// Next hop towards `dst`: a route if there is one, else the destination
    /// itself when it is a live neighbor.
    fn next_hop(&self, dst: u16) -> Option<u16> {
        if self.child_index(dst).is_some() {
            return Some(dst);
        }
        if let Some(i) = self.route_index(dst) {
            if self.routes[i].status == ROUTE_ACTIVE {
                return Some(self.routes[i].next);
            }
        }
        let i = self.neighbor_index(dst)?;
        (self.neighbors[i].age <= AGE_LIMIT).then_some(dst)
    }

    /// The link to the neighbor `short` has failed: its failure counter is
    /// up, and nothing has been heard from it since.
    fn link_failed(&self, short: u16) -> bool {
        self.neighbor_index(short).is_some_and(|i| self.neighbors[i].failures >= LINK_FAILURES)
    }

    fn route_set(&mut self, dst: u16, next: u16, status: u8, many_to_one: bool, no_cache: bool, now: u32) -> usize {
        let i = self.route_index(dst).or_else(|| self.routes.iter().position(|r| r.status == ROUTE_FREE)).unwrap_or_else(|| {
            // evict the oldest entry, sparing routes to concentrators
            let mut best = 0;
            let mut best_age = 0;
            for (i, r) in self.routes.iter().enumerate() {
                let age = now.wrapping_sub(r.at) / if r.many_to_one { 16 } else { 1 };
                if age >= best_age {
                    best_age = age;
                    best = i;
                }
            }
            best
        });
        self.routes[i] = Route { status, dst, next, many_to_one, no_cache, record_due: false, at: now };
        i
    }

    /// A route found by an ordinary route discovery: destination, next hop,
    /// active. That is all such a discovery sets (R22 3.6.3.5.2, 3.6.3.5.3);
    /// what a many-to-one request said of the destination stays, with a
    /// route record due again if the way to the concentrator changed.
    fn route_learned(&mut self, dst: u16, next: u16, now: u32) {
        match self.route_index(dst) {
            Some(i) if self.routes[i].status == ROUTE_ACTIVE => {
                let r = &mut self.routes[i];
                r.record_due |= r.many_to_one && r.next != next;
                r.next = next;
                r.at = now;
            }
            _ => {
                self.route_set(dst, next, ROUTE_ACTIVE, false, false, now);
            }
        }
    }

    /// Routes a unicast frame: originated here, or relayed for someone else.
    pub(crate) fn route_out(&mut self, h: &Header, payload: &[u8]) {
        self.route_try(h, payload, NWK_RETRIES);
    }

    /// One attempt at routing a unicast frame, `retries` more to come if
    /// the MAC cannot deliver it. Frames under our own source address are
    /// ours; the others are relayed.
    fn route_try(&mut self, h: &Header, payload: &[u8], retries: u8) {
        let originated = h.src == self.nib.short;
        // nothing is passed on to a child that is not authenticated yet (R22 4.6.3.2.1)
        if self.child_index(h.dst).is_some_and(|i| !self.children[i].joined) {
            self.trace(trace::DROP, trace::DROP_UNAUTHENTICATED_CHILD, h, h.dst, 0);
            return;
        }
        let data = h.frame_type == nwk::FRAME_DATA;
        // a route record that cannot go on is dropped, and nobody is told (R22 3.6.3.5.4)
        let record = h.frame_type == nwk::FRAME_CMD && payload.first() == Some(&nwk::CMD_ROUTE_RECORD);
        // what loses a relayed frame that goes no further: no route, unless the MAC failed first
        let mut cause = k::RELAY_DROP_NO_ROUTE;
        if let Some(mut next) = self.next_hop(h.dst) {
            let many_to_one =
                self.route_index(h.dst).is_some_and(|i| self.routes[i].status == ROUTE_ACTIVE && self.routes[i].many_to_one);
            // Towards a concentrator, a frame whose next hop has failed goes
            // by another router: every router has a route to it (R22
            // 3.6.3.7.1), and its next many-to-one request sets ours anew.
            if many_to_one && self.link_failed(next) {
                if let Some(via) = self.random_router(next, h.src) {
                    next = via;
                }
            }
            // over a link that has failed, one attempt per frame until the
            // neighbor is heard again (it may be gone: the air is not to be
            // filled with attempts that go nowhere)
            let retries = if self.link_failed(next) { 0 } else { retries };
            if self.transmit_nwk(next, h, payload) == TxStatus::Ok {
                if !originated {
                    self.diag.add(k::RELAYED_UCAST);
                }
                return;
            }
            if retries > 0 && self.hold_retry(h, payload, ROUTED, retries) {
                return;
            }
            // what lost the frame, if it goes no further: no room to try again, or the MAC
            let lost = if retries > 0 { k::RELAY_DROP_QUEUE_FULL } else { k::RELAY_DROP_MAC_FAIL };
            self.link_lost(next, h.dst);
            if record {
                if !originated {
                    self.diag.add(lost);
                }
                self.trace(trace::DROP, trace::DROP_ROUTE_RECORD, h, next, 0);
                return;
            }
            // What a failed link is followed by (R22 3.6.3.7.1). On a route
            // to a concentrator: "many-to-one route failure" towards the
            // concentrator, about the source of the frame, through some
            // other router; the route is not discovered anew for a frame of
            // someone else. On an ordinary route: "link failure" back to the
            // source of the frame.
            if data && many_to_one {
                if let Some(via) = self.random_router(next, h.src) {
                    let sh = self.new_header(nwk::FRAME_CMD, h.dst, nwk::DEFAULT_RADIUS);
                    let about = h.src.to_le_bytes();
                    self.transmit_nwk(via, &sh, &[nwk::CMD_NETWORK_STATUS, nwk::STATUS_MANY_TO_ONE_FAILURE, about[0], about[1]]);
                    // The frame itself is not discovered a route for ("many-to-one
                    // routes are not automatically rediscovered"): it goes on by
                    // that router, which has a route of its own to the concentrator.
                    if self.transmit_nwk(via, h, payload) == TxStatus::Ok {
                        if !originated {
                            self.diag.add(k::RELAYED_UCAST);
                        }
                        return;
                    }
                }
                if !originated {
                    self.diag.add(lost);
                    self.trace(trace::DROP, trace::DROP_NO_OTHER_ROUTER, h, next, 0);
                    return;
                }
            } else if data && !originated {
                self.send_network_status(h.src, nwk::STATUS_LINK_FAILURE, h.dst);
                if h.discover == 0 {
                    self.diag.add(lost);
                    return;
                }
            }
            cause = lost;
        }
        // No way to the destination. Our own frames wait for a route
        // discovery, and so do those of others that allow it: R22 3.6.3.3
        // says to start one for any frame with "discover route" set, while
        // 3.6.3.5.1 names only the frames of end-device children; the first
        // is followed, since a router that has lost a route which the source
        // still believes in would otherwise drop its frames for good.
        if originated || h.discover != 0 {
            self.hold(h, payload);
            self.discover(h.dst);
        } else if data {
            self.diag.add(k::RELAY_DROP_NO_ROUTE);
            self.send_network_status(h.src, nwk::STATUS_NO_ROUTE, h.dst);
        } else {
            if !originated {
                self.diag.add(cause);
            }
            self.trace(trace::DROP, if record { trace::DROP_ROUTE_RECORD } else { trace::DROP_NO_ROUTE }, h, h.dst, 0);
        }
    }

    /// A router neighbor picked at random, other than `avoid` and `avoid2`:
    /// one heard lately that hears us too, over a link that has not failed.
    fn random_router(&mut self, avoid: u16, avoid2: u16) -> Option<u16> {
        let live = |n: &&Neighbor| {
            n.used && n.age <= AGE_LIMIT && n.out_cost != 0 && n.failures < LINK_FAILURES && n.short != avoid && n.short != avoid2
        };
        let count = self.neighbors.iter().filter(live).count();
        if count == 0 {
            return None;
        }
        let pick = self.hal.random() as usize % count;
        self.neighbors.iter().filter(live).nth(pick).map(|n| n.short)
    }

    /// Drops every route through `next`, but those to concentrators (see
    /// `link_lost`) unless the neighbor is gone from the table.
    fn drop_routes_via(&mut self, next: u16) {
        let gone = self.neighbor_index(next).is_none();
        for r in self.routes.iter_mut() {
            if r.status == ROUTE_ACTIVE && r.next == next && (gone || !r.many_to_one) {
                r.status = ROUTE_FREE;
            }
        }
    }

    /// A frame for `dst` could not be delivered to the neighbor `next`,
    /// however often it was tried. The route that led there is repaired
    /// (dropped, to be found anew); when the link to `next` has failed (its
    /// failure counter is up, R22 3.6.3.7), so is every other route through
    /// it. The neighbor stays in the table with what its Link Status said:
    /// a link that missed a few acknowledgements is still one, and its own
    /// route requests are still taken (3.6.3.5.2 asks only for the outgoing
    /// cost it reported). Frames heard from it set the counter back.
    /// Routes to a concentrator stay: they are not discovered anew after a
    /// failure (R22 3.6.3.7.1) but set by its next many-to-one request, and
    /// until then its frames go by another router (`route_try`).
    fn link_lost(&mut self, next: u16, dst: u16) {
        if self.neighbor_index(next).is_none() || self.link_failed(next) {
            self.drop_routes_via(next);
        } else if let Some(i) = self.route_index(dst) {
            let r = &mut self.routes[i];
            if r.status == ROUTE_ACTIVE && r.next == next && !r.many_to_one {
                r.status = ROUTE_FREE;
            }
        }
    }

    /// Sends a frame to a neighbor the frame itself names (the next relay
    /// of a source route), trying again later if the MAC cannot deliver
    /// it. When it cannot be delivered at all, the source is told with the
    /// destination of the frame, not the relay that did not answer (R22
    /// 3.4.3.3.2); the failure of a Network Status is not reported (3.6.3.3).
    fn send_to(&mut self, next: u16, h: &Header, payload: &[u8], retries: u8) {
        let retries = if self.link_failed(next) { 0 } else { retries };
        let relayed = h.src != self.nib.short;
        if self.transmit_nwk(next, h, payload) == TxStatus::Ok {
            if relayed {
                self.diag.add(k::RELAYED_UCAST);
            }
            return;
        }
        if retries > 0 && self.hold_retry(h, payload, next, retries) {
            return;
        }
        if relayed {
            self.diag.add(if retries > 0 { k::RELAY_DROP_QUEUE_FULL } else { k::RELAY_DROP_MAC_FAIL });
        }
        self.link_lost(next, h.dst);
        let status = h.frame_type == nwk::FRAME_CMD && payload.first() == Some(&nwk::CMD_NETWORK_STATUS);
        if !status {
            self.send_network_status(h.src, nwk::STATUS_SOURCE_ROUTE_FAILURE, h.dst);
        }
    }

    /// Keeps a unicast the MAC could not deliver for another attempt after
    /// a random pause (two neighbors that sent to each other at the same
    /// moment, or a neighbor busy with its own transmissions, do not meet
    /// again). False when there is no room: the frame has failed now.
    fn hold_retry(&mut self, h: &Header, payload: &[u8], next: u16, retries: u8) -> bool {
        let Some(i) = self.held.iter().position(|e| !e.used) else {
            self.diag.add(k::BUFFER_ALLOCATE_FAILURES);
            self.trace(trace::HELD_FAIL, trace::FAIL_NO_ROOM, h, next, 1);
            return false;
        };
        if payload.len() > MAX_NWK_PAYLOAD {
            return false;
        }
        self.trace(trace::HOLD, retries, h, next, 0);
        let now = self.hal.now_ms();
        let pause = NWK_RETRY_MIN_MS + self.hal.random() % (NWK_RETRY_SPREAD_MS + 1);
        let e = &mut self.held[i];
        *e = Held::EMPTY;
        e.used = true;
        e.header = *h;
        e.len = payload.len() as u8;
        e.payload[..payload.len()].copy_from_slice(payload);
        e.at = now.wrapping_add(pause);
        e.left = retries;
        e.next = next;
        true
    }

    /// Keeps a unicast until a route to its destination is found. When
    /// every place is taken, the frame that has waited longest for its
    /// route makes room, and its source is told as if its time had run out
    /// (R22 3.6.3.3); with every place waiting for another attempt the new
    /// frame is the one that fails.
    fn hold(&mut self, h: &Header, payload: &[u8]) {
        if payload.len() > MAX_NWK_PAYLOAD {
            self.trace(trace::DROP, trace::DROP_TOO_LONG, h, h.dst, 0);
            return;
        }
        let now = self.hal.now_ms();
        let free = self.held.iter().position(|e| !e.used);
        let oldest = || {
            let waiting = self.held.iter().enumerate().filter(|(_, e)| e.used && e.left == 0);
            waiting.min_by_key(|(_, e)| e.at.wrapping_sub(now)).map(|(i, _)| i)
        };
        let i = match free.or_else(oldest) {
            Some(i) => i,
            None => {
                self.held_failed(h, trace::FAIL_NO_ROOM);
                return;
            }
        };
        if self.held[i].used {
            let old = self.held[i].header;
            self.held_failed(&old, trace::FAIL_EVICTED);
        }
        self.trace(trace::HOLD, 0, h, h.dst, 0);
        let e = &mut self.held[i];
        *e = Held::EMPTY;
        e.used = true;
        e.header = *h;
        e.len = payload.len() as u8;
        e.payload[..payload.len()].copy_from_slice(payload);
        e.at = now.wrapping_add(HOLD_MS);
    }

    /// No route was found for a frame that waited for one. The source of a
    /// frame that was being passed on is told (R22 3.6.3.3, last paragraph).
    fn held_failed(&mut self, h: &Header, why: u8) {
        self.trace(trace::HELD_FAIL, why, h, h.dst, 0);
        if why != trace::FAIL_EXPIRED {
            self.diag.add(k::BUFFER_ALLOCATE_FAILURES);
        }
        if h.src != self.nib.short {
            self.diag.add(if why == trace::FAIL_EXPIRED { k::RELAY_DROP_NO_ROUTE } else { k::RELAY_DROP_QUEUE_FULL });
        } else if h.frame_type == nwk::FRAME_DATA {
            self.diag.add(k::APS_TX_UCAST_FAIL);
        }
        if h.src != self.nib.short && h.frame_type == nwk::FRAME_DATA {
            self.send_network_status(h.src, nwk::STATUS_NO_ROUTE, h.dst);
        }
    }

    /// A route to `dst` has been found: the frames waiting for it go.
    fn flush_held(&mut self, dst: u16) {
        for i in 0..self.held.len() {
            if self.held[i].used && self.held[i].left == 0 && self.held[i].header.dst == dst {
                let e = self.held[i];
                self.held[i].used = false;
                self.route_try(&e.header, &e.payload[..e.len as usize], NWK_RETRIES);
            }
        }
    }

    /// The next attempt at a unicast the MAC could not deliver.
    fn retry_held(&mut self, e: &Held) {
        let payload = &e.payload[..e.len as usize];
        let retries = e.left.saturating_sub(1);
        if e.next == ROUTED {
            self.route_try(&e.header, payload, retries);
        } else {
            self.send_to(e.next, &e.header, payload, retries);
        }
    }

    /// A route discovery table entry to use: a free one; with `force` (our
    /// own discoveries) otherwise the one closest to its end. `None`: no
    /// room, which for the request of another node means it is not taken
    /// (no "routing capacity", R22 3.6.3.2, 3.6.3.5.2).
    fn discovery_slot(&mut self, now: u32, force: bool) -> Option<usize> {
        let mut best = None;
        let mut soonest = u32::MAX;
        for (i, d) in self.discoveries.iter().enumerate() {
            let live = d.used && !due(now, d.expires);
            let left = if live { d.expires.wrapping_sub(now) } else { 0 };
            if left < soonest && (force || !live) {
                soonest = left;
                best = Some(i);
            }
        }
        best
    }

    /// Starts a route discovery for `dst` unless one is running.
    fn discover(&mut self, dst: u16) {
        if let Some(i) = self.route_index(dst) {
            if self.routes[i].status == ROUTE_DISCOVERY {
                return;
            }
        }
        let now = self.hal.now_ms();
        self.route_set(dst, 0xFFFF, ROUTE_DISCOVERY, false, false, now);
        self.rreq_id = self.rreq_id.wrapping_add(1);
        let Some(slot) = self.discovery_slot(now, true) else {
            return;
        };
        self.discoveries[slot] = Discovery {
            used: true,
            id: self.rreq_id,
            src: self.nib.short,
            sender: self.nib.short,
            forward_cost: 0,
            residual_cost: 0xFF,
            expires: now.wrapping_add(DISCOVERY_MS),
        };
        let h = self.new_header(nwk::FRAME_CMD, nwk::BROADCAST_ROUTERS, nwk::DEFAULT_RADIUS);
        let d = dst.to_le_bytes();
        self.diag.add(k::ROUTE_DISC_INITIATED);
        self.log_event(super::diag::ev::ROUTE_DISCOVERY, dst, 0);
        self.originate_broadcast(&h, &[nwk::CMD_ROUTE_REQUEST, 0, self.rreq_id, d[0], d[1], 0]);
    }

    fn on_route_request(&mut self, h: &Header, mac_src: u16, lqi: u8, payload: &[u8], now: u32) {
        let mut r = Reader::new(payload);
        let (Some(_), Some(options), Some(id), Some(dst), Some(cost)) = (r.u8(), r.u8(), r.u8(), r.u16(), r.u8()) else {
            return;
        };
        if h.src == self.nib.short {
            return;
        }
        // A request is taken only over a link known to work both ways: from
        // a neighbor that has reported how it hears us (R22 3.6.3.5.2, for
        // many-to-one requests and, with nwkSymLink set as in Zigbee PRO,
        // for all others). A route over a one-way link would be no route.
        if !self.neighbor_index(mac_src).is_some_and(|i| self.neighbors[i].out_cost != 0) {
            return;
        }
        let cost_here = cost.saturating_add(self.link_cost(mac_src, lqi));
        let many_to_one = (options >> 3) & 3;
        let mut slot = None;
        for (i, d) in self.discoveries.iter().enumerate() {
            if d.used && d.src == h.src && d.id == id && !due(now, d.expires) {
                if d.forward_cost <= cost_here {
                    return;
                }
                slot = Some(i);
            }
        }
        let Some(slot) = slot.or_else(|| self.discovery_slot(now, false)) else {
            return;
        };
        self.discoveries[slot] = Discovery {
            used: true,
            id,
            src: h.src,
            sender: mac_src,
            forward_cost: cost_here,
            residual_cost: 0xFF,
            expires: now.wrapping_add(DISCOVERY_MS),
        };
        if many_to_one != 0 {
            // A concentrator announces itself: the request is the route. A
            // route record is required if the route is new or has changed,
            // or if the concentrator keeps no routes; otherwise that stays
            // as it was (R22 3.6.3.5.2).
            let no_cache = many_to_one == 2;
            let settled = self.route_index(h.src).is_some_and(|i| {
                let r = &self.routes[i];
                r.status == ROUTE_ACTIVE && r.many_to_one && r.next == mac_src && !r.record_due
            });
            let i = self.route_set(h.src, mac_src, ROUTE_ACTIVE, true, no_cache, now);
            self.routes[i].record_due = no_cache || !settled;
        } else {
            // We are the destination, or it is our end-device child, for
            // whom we answer once it is authenticated (R22 4.6.3.2.1):
            // reply, and keep the way back.
            let child = self.end_device_child(dst).filter(|&i| self.children[i].joined);
            let responder = match child {
                Some(i) => Some(self.children[i].ext),
                None if dst == self.nib.short => Some(self.ext),
                None => None,
            };
            if let Some(responder) = responder {
                self.route_learned(h.src, mac_src, now);
                // the path so far: the link the reply leaves on (R22 3.6.3.5.2)
                let cost = self.link_cost(mac_src, lqi);
                let mut p = Buf::new();
                p.u8(nwk::CMD_ROUTE_REPLY);
                // originator and responder IEEE address, where known (R22 3.4.2.3.1)
                p.u8(0x20 | if h.src_ext.is_some() { 0x10 } else { 0 });
                p.u8(id);
                p.u16(h.src);
                p.u16(dst);
                p.u8(cost);
                if let Some(ext) = h.src_ext {
                    p.u64(ext);
                }
                p.u64(responder);
                let rh = self.new_header(nwk::FRAME_CMD, mac_src, nwk::DEFAULT_RADIUS);
                self.transmit_nwk(mac_src, &rh, p.as_slice());
                return;
            }
        }
        if h.radius > 1 && payload.len() <= MAX_NWK_PAYLOAD {
            let mut fwd = *h;
            fwd.radius -= 1;
            let mut p = [0u8; MAX_NWK_PAYLOAD];
            p[..payload.len()].copy_from_slice(payload);
            p[5] = cost_here;
            // a copy of this request still waiting came by a worse way: this one takes its place
            for e in self.bcast.iter_mut() {
                if e.used && e.header.src == h.src && is_route_request(&e.header, &e.payload) && e.payload[2] == id {
                    e.used = false;
                }
            }
            let span = RREQ_JITTER_MAX_MS - RREQ_JITTER_MIN_MS + 1;
            let jitter = 2 * (RREQ_JITTER_MIN_MS + self.hal.random() % span);
            self.queue_broadcast(&fwd, &p[..payload.len()], now.wrapping_add(jitter), 1 + RREQ_RELAY_RETRIES);
        }
    }

    /// Route Reply addressed to this node: the answer to our own request,
    /// or one on its way back along the path the request took.
    fn on_route_reply(&mut self, h: &Header, mac_src: u16, payload: &[u8]) {
        let mut r = Reader::new(payload);
        let (Some(_), Some(_), Some(id), Some(orig), Some(resp), Some(cost)) =
            (r.u8(), r.u8(), r.u8(), r.u16(), r.u16(), r.u8())
        else {
            return;
        };
        let now = self.hal.now_ms();
        // a reply to a discovery this node knows nothing of is not taken (R22 3.6.3.5.3)
        let Some(i) = self.discoveries.iter().position(|d| d.used && d.src == orig && d.id == id && !due(now, d.expires)) else {
            return;
        };
        // The path cost is the sum up to the sender's own link to us, which
        // the sender has put in (R22 3.6.3.5.2, 3.6.3.5.3).
        if orig == self.nib.short {
            // The answer to our own request, if it is about the address we
            // asked for: the routing table has that from the start of the
            // discovery. If it is not, the reply is dropped, and the
            // discovery it claims to answer with it (R22 3.6.3.5.3).
            if self.route_index(resp).is_none() {
                self.discoveries[i].used = false;
                return;
            }
            // only a better way replaces the one a first reply brought
            if self.discoveries[i].residual_cost <= cost {
                return;
            }
            self.discoveries[i].residual_cost = cost;
            self.route_learned(resp, mac_src, now);
            // the request has been answered: no need to repeat it
            for e in self.bcast.iter_mut() {
                if e.used && e.header.src == orig && is_route_request(&e.header, &e.payload) && e.payload[2] == id {
                    e.used = false;
                }
            }
            self.flush_held(resp);
            return;
        }
        if self.discoveries[i].residual_cost < cost {
            return;
        }
        self.discoveries[i].residual_cost = cost;
        let back = self.discoveries[i].sender;
        self.route_learned(resp, mac_src, now);
        self.route_learned(orig, back, now);
        // The reply is made anew for the next hop back: the cost of that
        // link added, the radius one less but never 0 (R22 3.4.2.2).
        let mut p = [0u8; MAX_NWK_PAYLOAD];
        let n = payload.len().min(p.len());
        p[..n].copy_from_slice(&payload[..n]);
        p[7] = cost.saturating_add(self.link_cost(back, 0));
        let rh = self.new_header(nwk::FRAME_CMD, back, h.radius.saturating_sub(1).max(1));
        self.transmit_nwk(back, &rh, &p[..n]);
    }

    /// Network Status about a routing failure to the source of the frame
    /// that failed, with the destination that frame had (R22 3.4.3.3.2).
    fn send_network_status(&mut self, to: u16, status: u8, about: u16) {
        let a = about.to_le_bytes();
        self.send_status(to, &[nwk::CMD_NETWORK_STATUS, status, a[0], a[1]]);
    }

    fn send_status(&mut self, to: u16, payload: &[u8]) {
        if to == self.nib.short || nwk::is_broadcast(to) {
            return;
        }
        // best effort along an existing route: a failure report must not
        // start discoveries of its own
        if let Some(next) = self.next_hop(to) {
            let h = self.new_header(nwk::FRAME_CMD, to, nwk::DEFAULT_RADIUS);
            self.diag.add(k::NWK_STATUS_SENT);
            let status = payload.get(1).copied().unwrap_or(0);
            let about = u16::from_le_bytes([payload.get(2).copied().unwrap_or(0xFF), payload.get(3).copied().unwrap_or(0xFF)]);
            self.diag.last_status_tx = status;
            self.log_event(super::diag::ev::STATUS_TX, status as u16, about);
            self.transmit_nwk(next, &h, payload);
        } else {
            // no route back: the report is not sent
            let about = u16::from_le_bytes([payload.get(2).copied().unwrap_or(0xFF), payload.get(3).copied().unwrap_or(0xFF)]);
            let status = payload.get(1).copied().unwrap_or(0);
            self.hal.trace(Trace { kind: trace::NWK_STATUS, detail: 1, seq: status, src: to, peer: about, aux: 0 });
        }
    }

    /// Notes that `short` is the network address of the device `ext`, and
    /// acts when that contradicts what is known: one address, two devices.
    /// True when it did contradict.
    pub(crate) fn address_seen(&mut self, short: u16, ext: u64) -> bool {
        if ext == 0 || ext == u64::MAX || short >= 0xFFF8 || self.state != State::Up {
            return false;
        }
        let clash = if short == self.nib.short {
            ext != self.ext
        } else if let Some(i) = self.child_index(short) {
            self.children[i].ext != ext
        } else {
            self.neighbors.iter().any(|n| n.used && n.short == short && n.ext != ext && n.ext != 0)
                || self.addr_map.iter().any(|e| e.1 == short && e.0 != ext && e.0 != 0)
        };
        if clash {
            self.address_conflict(short);
        }
        clash
    }

    /// Two devices use one network address: tells the network (Network
    /// Status "address conflict", to all routers and devices that listen),
    /// and does its own part of the resolution. Every router that noticed
    /// the conflict would say so at once: each waits a random moment
    /// first, and keeps quiet if another one's report arrives meanwhile
    /// (R22 3.6.1.9.3).
    pub(crate) fn address_conflict(&mut self, short: u16) {
        let now = self.hal.now_ms();
        // once per address and a while: others report the same conflict
        if self.conflict.0 == short && !due(now, self.conflict.1) {
            return;
        }
        self.log_event(super::diag::ev::CONFLICT, short, 0);
        // (a report still waiting is about another address: out with it)
        self.address_conflict_report();
        self.conflict = (short, now.wrapping_add(CONFLICT_QUIET_MS));
        self.conflict_due = Some(now.wrapping_add(self.hal.random() % (CONFLICT_JITTER_MS + 1)));
    }

    /// Sends the report that was waiting, if any, and acts on it.
    fn address_conflict_report(&mut self) {
        if self.conflict_due.take().is_none() {
            return;
        }
        let short = self.conflict.0;
        let h = self.new_header(nwk::FRAME_CMD, nwk::BROADCAST_RX_ON, nwk::DEFAULT_RADIUS);
        let a = short.to_le_bytes();
        self.nwk_send(&h, &[nwk::CMD_NETWORK_STATUS, nwk::STATUS_ADDRESS_CONFLICT, a[0], a[1]]);
        self.address_conflict_resolve(short);
    }

    /// The holder of a conflicting address takes another: this node picks
    /// one for itself and announces it; a child that cannot hear the
    /// broadcast is given one by its parent in an unsolicited Rejoin Response.
    pub(crate) fn address_conflict_resolve(&mut self, short: u16) {
        let now = self.hal.now_ms();
        if short == self.nib.short {
            let new = self.new_address();
            self.nib.short = new;
            self.hal.set_filter(self.nib.pan, new);
            self.store_network();
            self.announce_at = Some(now.wrapping_add(50 + self.hal.random() % 200));
            return;
        }
        if let Some(i) = self.child_index(short) {
            let c = self.children[i];
            if c.capability & CAP_FFD == 0 {
                let new = self.new_address();
                let mut rh = self.new_header(nwk::FRAME_CMD, short, 1);
                rh.dst_ext = Some(c.ext);
                let s = new.to_le_bytes();
                self.transmit_nwk(short, &rh, &[nwk::CMD_REJOIN_RESPONSE, s[0], s[1], 0x00]);
                self.children[i].short = new;
                self.store_children();
            }
            return;
        }
        // what was known of the address belongs to one of the two; both will announce themselves anew
        for n in self.neighbors.iter_mut().filter(|n| n.used && n.short == short) {
            n.used = false;
            self.diag.n[k::NEIGHBOR_REMOVED] = self.diag.n[k::NEIGHBOR_REMOVED].wrapping_add(1);
        }
        for e in self.addr_map.iter_mut().filter(|e| e.1 == short) {
            e.1 = 0xFFFF;
        }
        if let Some(i) = self.route_index(short) {
            self.routes[i].status = ROUTE_FREE;
        }
    }

    fn on_nwk_command(&mut self, h: &Header, mac_src: u16, p: &[u8]) {
        match p.first() {
            Some(&nwk::CMD_ROUTE_REPLY) if h.dst == self.nib.short => self.on_route_reply(h, mac_src, p),
            Some(&nwk::CMD_LINK_STATUS) => self.on_link_status(h.src, p),
            Some(&nwk::CMD_NETWORK_STATUS) if p.len() >= 4 => {
                let about = u16::from_le_bytes([p[2], p[3]]);
                self.diag.status_rx = self.diag.status_rx.wrapping_add(1);
                self.diag.last_status_rx = p[1];
                self.log_event(super::diag::ev::STATUS_RX, p[1] as u16, about);
                match p[1] {
                    // some route from here failed further on: it is of no
                    // use any more (R22 3.6.3.7.1 says so of a link failure;
                    // "no route" from a router on the way means as much)
                    nwk::STATUS_NO_ROUTE | nwk::STATUS_TREE_LINK_FAILURE | nwk::STATUS_LINK_FAILURE => self.route_drop(about),
                    nwk::STATUS_ADDRESS_CONFLICT => {
                        // someone else has told the network: our own report
                        // of it, waiting or still to come, is not needed
                        if self.conflict.0 == about {
                            self.conflict_due = None;
                        }
                        let now = self.hal.now_ms();
                        self.conflict = (about, now.wrapping_add(CONFLICT_QUIET_MS));
                        self.route_drop(about);
                        self.address_conflict_resolve(about);
                    }
                    _ => {}
                }
            }
            Some(&nwk::CMD_REJOIN_REQUEST) => self.on_rejoin_request(h, mac_src, p, true),
            Some(&nwk::CMD_REJOIN_RESPONSE) => self.on_rejoin_response(h, mac_src, p),
            Some(&nwk::CMD_TIMEOUT_REQUEST) => self.on_timeout_request(h, p),
            Some(&nwk::CMD_LEAVE) if p.len() >= 2 => {
                if let (Some(i), true) = (self.child_index(h.src), p[1] & 0x40 == 0) {
                    // A child says goodbye. The Trust Center is told that it
                    // left only if it does not mean to come back (R22 4.6.3.6.2).
                    if p[1] & 0x20 != 0 {
                        self.remove_child(i);
                    } else {
                        self.child_left(i);
                    }
                    return;
                }
                if p[1] & 0x40 != 0 {
                    // a request for us to leave (the frame is NWK-secured and unicast)
                    if h.dst == self.nib.short {
                        let now = self.hal.now_ms();
                        self.leave_at = Some(now.wrapping_add(100));
                        self.leave_rejoin = p[1] & 0x20 != 0;
                    }
                } else if let Some(i) = self.neighbor_index(h.src) {
                    // a neighbor says goodbye
                    self.neighbors[i].used = false;
                    self.diag.add(k::NEIGHBOR_REMOVED);
                    self.drop_routes_via(h.src);
                }
            }
            Some(&nwk::CMD_NETWORK_UPDATE) if p.len() >= 13 => {
                // PAN identifier update. The command travels under the old
                // identifier, so the change waits until the broadcast has
                // been everywhere (R22 3.6.1.13.3): until then this node
                // passes the command on, and hears its neighbors, under the
                // old one.
                let mut r = Reader::new(&p[1..]);
                if let (Some(options), Some(ext_pan), Some(update_id), Some(pan)) = (r.u8(), r.u64(), r.u8(), r.u16()) {
                    if options >> 5 == 0 && ext_pan == self.nib.ext_pan {
                        let now = self.hal.now_ms();
                        self.pan_at = Some((now.wrapping_add(BROADCAST_MEMORY_MS), pan));
                        // the update id is the network manager's to set
                        if h.src == self.manager {
                            self.nib.update_id = update_id;
                        }
                    }
                }
            }
            // A command this node does not know, sent to it alone, is
            // answered with a Network Status "unknown command" that names it
            // (R22 3.6.13). (The commands of R22 that a router has no use
            // for, such as a Route Record that ends here, are known.)
            Some(&id) if !(nwk::CMD_ROUTE_REQUEST..=nwk::CMD_TIMEOUT_RESPONSE).contains(&id) && h.dst == self.nib.short => {
                self.send_status(h.src, &[nwk::CMD_NETWORK_STATUS, nwk::STATUS_UNKNOWN_COMMAND, id]);
            }
            _ => {}
        }
    }

    /// Tells the neighbors this node is leaving.
    pub(crate) fn send_leave(&mut self) {
        let h = self.new_header(nwk::FRAME_CMD, nwk::BROADCAST_RX_ON, 1);
        let options = if self.leave_rejoin { 0x20 } else { 0x00 };
        self.transmit_nwk(mac::BROADCAST, &h, &[nwk::CMD_LEAVE, options]);
    }

    // ------------------------------------------------------------ neighbors

    pub(crate) fn neighbor_index(&self, short: u16) -> Option<usize> {
        self.neighbors.iter().position(|n| n.used && n.short == short)
    }

    pub(crate) fn neighbor_by_ext(&self, ext: u64) -> Option<usize> {
        self.neighbors.iter().position(|n| n.used && n.ext == ext)
    }

    /// Records a frame heard directly from a neighbor.
    fn neighbor_heard(&mut self, short: u16, ext: u64, lqi: u8, counter: u32, key_seq: u8) {
        // What a router that has just joined through us is told of the link
        // before its first Link Status: the exchange that let it in showed
        // that it hears us (see `link_proven`).
        let mut out_cost = 0;
        if let Some(i) = self.child_by_ext(ext) {
            self.children[i].fc_next = counter.wrapping_add(1);
            self.children[i].fc_seq = key_seq;
            self.children[i].heard = self.hal.now_ms();
            // an end device is a child only; a router child is also a neighbor
            if self.children[i].capability & CAP_FFD == 0 {
                return;
            }
            if self.children[i].joined {
                out_cost = nwk::cost_from_lqi(lqi);
            }
        }
        let known = self.neighbor_by_ext(ext).or_else(|| self.neighbor_index(short));
        let i = match known {
            Some(i) => i,
            None => {
                let free = self.neighbors.iter().position(|n| !n.used);
                if free.is_none() {
                    self.diag.add(k::NEIGHBOR_REMOVED);
                }
                self.diag.add(k::NEIGHBOR_ADDED);
                let slot = free.unwrap_or_else(|| {
                    // evict the longest-silent neighbor
                    let mut best = 0;
                    for (i, n) in self.neighbors.iter().enumerate() {
                        if n.age > self.neighbors[best].age {
                            best = i;
                        }
                    }
                    best
                });
                // Usable as a next hop right away, but on probation: unless
                // its link status arrives, the next aging step makes it stale.
                // Not so a router that has just joined through us: the
                // exchange that let it in proved the link both ways, and
                // until its first Link Status (a second or two later) its
                // route requests are taken. (Made stale at our next Link
                // Status, it lost its outgoing cost, its discovery of the
                // Trust Center for the link key exchange went unanswered, and
                // it left the network to associate anew under another address.)
                let age = if out_cost != 0 { 0 } else { AGE_LIMIT };
                if let Some(b) = self.hal.black_box() {
                    b.clear_link(slot);
                }
                if self.neighbors[slot].used {
                    let old = self.neighbors[slot];
                    self.log_event(super::diag::ev::NEIGHBOR_REMOVED, old.short, old.lqi as u16);
                }
                let rssi = self.rx_signal.1;
                self.neighbors[slot] =
                    Neighbor {
                        used: true,
                        short,
                        ext,
                        lqi,
                        out_cost,
                        age,
                        confirmed: out_cost != 0,
                        fc_seq: key_seq,
                        rssi_avg: rssi,
                        rssi_min: rssi,
                        ..Neighbor::EMPTY
                    };
                self.log_event(super::diag::ev::NEIGHBOR_ADDED, short, lqi as u16);
                slot
            }
        };
        let n = &mut self.neighbors[i];
        n.short = short;
        n.ext = ext;
        n.lqi = ((n.lqi as u16 * 3 + lqi as u16) / 4) as u8;
        let rssi = self.rx_signal.1;
        n.rx = n.rx.saturating_add(1);
        n.rssi_avg = ((n.rssi_avg as i16 * 3 + rssi as i16) / 4) as i8;
        n.rssi_min = n.rssi_min.min(rssi);
        n.fc_next = counter.wrapping_add(1);
        n.fc_seq = key_seq;
        n.failures = 0;
    }

    /// The neighbor `short` has just answered frames of ours (it let us
    /// join or rejoin through it): until its Link Status says how it hears
    /// us, the link counts as being as good that way as it is this way. The
    /// outgoing cost would otherwise be unknown for up to a Link Status
    /// period, and for that long every route request coming through the
    /// parent would have to be turned away (R22 3.6.3.5.2), which is when a
    /// coordinator looks for the new device.
    pub(crate) fn link_proven(&mut self, short: u16) {
        if let Some(i) = self.neighbor_index(short) {
            let n = &mut self.neighbors[i];
            if n.out_cost == 0 {
                n.out_cost = nwk::cost_from_lqi(n.lqi);
            }
            n.confirmed = true;
        }
    }

    /// Cost of the link to a neighbor: the worse of what we measure and what
    /// it reports about our frames.
    fn link_cost(&self, short: u16, lqi: u8) -> u8 {
        match self.neighbor_index(short) {
            Some(i) => self.neighbors[i].link_cost(),
            None => nwk::cost_from_lqi(lqi),
        }
    }

    /// One link status period has passed: every neighbor is a period older
    /// (nwkRouterAgeLimit counts periods, R22 3.6.3.4.2, not the Link Status
    /// frames this router sends: early ones come as often as every second).
    pub(crate) fn age_neighbors(&mut self) {
        let now = self.hal.now_ms();
        for n in self.neighbors.iter_mut().filter(|n| n.used) {
            n.age = n.age.saturating_add(1);
            if n.age > AGE_LIMIT {
                n.out_cost = 0;
            }
            if n.age == AGE_LIMIT + 1 && n.confirmed {
                // not heard for nwkRouterAgeLimit link status periods (one on
                // probation, never confirmed by a Link Status, just lapses)
                self.diag.n[k::NEIGHBOR_STALE] = self.diag.n[k::NEIGHBOR_STALE].wrapping_add(1);
                n.stales = n.stales.saturating_add(1);
                self.diag.event(now, super::diag::ev::NEIGHBOR_STALE, n.short, n.lqi as u16);
            }
            if n.age > NEIGHBOR_FORGET {
                n.used = false;
                self.diag.n[k::NEIGHBOR_REMOVED] = self.diag.n[k::NEIGHBOR_REMOVED].wrapping_add(1);
                self.diag.event(now, super::diag::ev::NEIGHBOR_REMOVED, n.short, n.lqi as u16);
            }
        }
    }

    pub(crate) fn send_link_status(&mut self) {
        for n in self.neighbors.iter_mut() {
            n.listed = false;
        }
        self.link_status_sent = self.hal.now_ms();
        // entries in ascending address order, as the spec requires
        let mut p = Buf::new();
        p.u8(nwk::CMD_LINK_STATUS);
        p.u8(0);
        let mut count = 0u8;
        let mut after: i32 = -1;
        while count < 24 {
            let next = self
                .neighbors
                .iter()
                .filter(|n| n.used && n.age <= AGE_LIMIT && n.short as i32 > after)
                .min_by_key(|n| n.short);
            let Some(n) = next else {
                break;
            };
            p.u16(n.short);
            p.u8(nwk::cost_from_lqi(n.lqi) | n.out_cost << 4);
            after = n.short as i32;
            count += 1;
            if let Some(i) = self.neighbor_index(after as u16) {
                self.neighbors[i].listed = true;
            }
        }
        p.set(1, count | 0x60);
        let h = self.new_header(nwk::FRAME_CMD, nwk::BROADCAST_ROUTERS, 1);
        self.transmit_nwk(mac::BROADCAST, &h, p.as_slice());
    }

    fn on_link_status(&mut self, src: u16, p: &[u8]) {
        let Some(i) = self.neighbor_index(src) else {
            return;
        };
        let Some(&options) = p.get(1) else {
            return;
        };
        // a router that joined through us and now sends link status is a
        // neighbor like any other: it needs no parent any more
        if let Some(c) = self.child_index(src) {
            if self.children[c].joined {
                self.remove_child(c);
            }
        }
        self.neighbors[i].age = 0;
        self.neighbors[i].confirmed = true;
        let mut r = Reader::new(&p[2..]);
        let mut listed = false;
        for _ in 0..options & 0x1F {
            let (Some(addr), Some(status)) = (r.u16(), r.u8()) else {
                break;
            };
            if addr == self.nib.short {
                // its incoming cost is our outgoing cost
                self.neighbors[i].out_cost = status & 7;
                listed = true;
            }
        }
        let complete = options & 0x60 == 0x60;
        if !listed && complete {
            self.neighbors[i].out_cost = 0;
        }
        // What the neighbor knows of the link is out of date: it does not
        // list us although we hear it, or it lists us and was not in our
        // last Link Status (it has just come back, or we have). Our next
        // Link Status goes early, so that both ends know the link works both
        // ways within a second or two instead of a whole period: until then
        // neither takes the other's route requests (R22 3.6.3.5.2). The
        // period is a maximum ("more frequently if desired", 3.6.3.4.1).
        if (complete && !listed) || (listed && !self.neighbors[i].listed) {
            self.link_status_soon();
        }
    }

    /// Brings the next Link Status forward: after a short random pause, not
    /// sooner than `EARLY_LS_GAP_MS` after the last one, and not in the
    /// first seconds after a start, while the neighbors are still being
    /// heard (see `resume`).
    fn link_status_soon(&mut self) {
        let now = self.hal.now_ms();
        let later = |a: u32, b: u32| if (a.wrapping_sub(b) as i32) > 0 { a } else { b };
        let soon = now.wrapping_add(EARLY_LS_MIN_MS + self.hal.random() % EARLY_LS_SPREAD_MS);
        let allowed = later(self.link_status_sent.wrapping_add(EARLY_LS_GAP_MS), self.booted.wrapping_add(RESUME_LINK_STATUS_MS));
        let at = later(allowed, soon);
        if (self.link_status_at.wrapping_sub(at) as i32) > 0 {
            self.link_status_at = at;
        }
    }

    /// A neighbor announced itself (Device_annce, heard from it directly):
    /// it has just joined, rejoined or come back from a power cycle, and
    /// knows nothing of its links. Our Link Status goes early, so that it
    /// can take our route requests and we its (R22 3.6.3.5.2) without
    /// waiting for a whole period.
    fn neighbor_announced(&mut self, h: &Header, mac_src: u16, payload: &[u8]) {
        // APS data without APS security, to the ZDO endpoint, cluster Device_annce
        let annce = h.frame_type == nwk::FRAME_DATA
            && h.src == mac_src
            && payload.len() > 8
            && payload[0] & 0x23 == 0
            && payload[1] == 0
            && payload[2..6] == [0x13, 0x00, 0x00, 0x00];
        if annce && self.neighbor_index(mac_src).is_some() {
            self.link_status_soon();
        }
    }

    pub(crate) fn net_timers(&mut self, now: u32) {
        if self.conflict_due.is_some_and(|at| due(now, at)) {
            self.address_conflict_report();
        }
        for i in 0..self.bcast.len() {
            if self.bcast[i].used && due(now, self.bcast[i].at) {
                let e = self.bcast[i];
                let payload = &e.payload[..e.len as usize];
                // A retransmission only while some neighbor has not been heard
                // with the broadcast (passive acknowledgement, R22 3.6.5).
                // Route requests are repeated as often as the text says
                // (3.6.3.5.1, 3.6.3.5.2): their copies differ in path cost.
                let first = e.header.src != self.nib.short && e.left == 1 + BROADCAST_RETRIES;
                if !first && !is_route_request(&e.header, payload) && self.passively_acknowledged(e.heard) {
                    self.bcast[i].used = false;
                    continue;
                }
                self.transmit_nwk(mac::BROADCAST, &e.header, payload);
                if e.left <= 1 {
                    self.bcast[i].used = false;
                } else {
                    self.bcast[i].left -= 1;
                    let gap = if is_route_request(&e.header, payload) { RREQ_RETRY_MS } else { BROADCAST_RETRY_MS };
                    self.bcast[i].at = now.wrapping_add(gap);
                }
            }
        }
        for i in 0..self.held.len() {
            if self.held[i].used && due(now, self.held[i].at) {
                self.held[i].used = false;
                let e = self.held[i];
                if e.left > 0 {
                    self.retry_held(&e);
                } else {
                    self.held_failed(&e.header, trace::FAIL_EXPIRED);
                }
            }
        }
        for r in self.routes.iter_mut() {
            if r.status == ROUTE_DISCOVERY && due(now, r.at.wrapping_add(DISCOVERY_MS)) {
                r.status = ROUTE_FREE;
            }
        }
        for d in self.discoveries.iter_mut() {
            if d.used && due(now, d.expires) {
                d.used = false;
            }
        }
    }
}
