//! Getting onto a network: active scan, MAC association, waiting for the
//! network key, and the Trust Center link key exchange that follows.

use super::{Neighbor, Nib, Stack, State, DEFAULT_TCLK};
use crate::aes::Aes128;
use crate::hal::{Hal, TxStatus};
use crate::mac::{self, Addr};
use crate::nwk;
use crate::util::{due, Buf};

/// Channels in scan order: the BDB primary set first, then the rest.
const CHANNELS: [u8; 16] = [11, 15, 20, 25, 12, 13, 14, 16, 17, 18, 19, 21, 22, 23, 24, 26];
const PRIMARY_CHANNELS: u8 = 4;
/// Listening time per channel: aBaseSuperframeDuration * (2^3 + 1) symbols.
const SCAN_MS: u32 = 140;
/// macResponseWaitTime: 32 * aBaseSuperframeDuration.
const RESPONSE_WAIT_MS: u32 = 500;
/// How long to wait for the network key after associating.
const KEY_WAIT_MS: u32 = 5_000;
const RETRY_MS: u32 = 3_000;
const REJOIN_WAIT_MS: u32 = 2_000;
/// bdbcTCLinkKeyExchangeTimeout: how long each step of the Trust Center
/// link key exchange waits for its answer (BDB 5.1.4).
const TCLK_WAIT_MS: u32 = 5_000;
/// bdbTCLinkKeyExchangeAttemptsMax.
const TCLK_ATTEMPTS: u8 = 3;
/// bdbcMinCommissioningTime, in seconds: how long the network is opened
/// for after this node has joined it (BDB 8.2 steps 14 and 15).
const COMMISSIONING_S: u8 = 180;

/// How many routers heard in a scan are remembered as possible parents.
pub(crate) const CANDIDATES: usize = 4;

/// A router to join or rejoin through, from its beacon.
#[derive(Clone, Copy)]
pub(crate) struct Candidate {
    pub channel: u8,
    pub lqi: u8,
    pub beacon: mac::Beacon,
}

/// `Stack::steering`: the node is joining by association, and the network
/// is to be opened for others when that is complete.
pub(crate) const STEERING_JOINING: u8 = 1;
/// The join is complete; the opening is due once the node has announced itself.
pub(crate) const STEERING_OPEN_DUE: u8 = 2;

/// apsParentAnnounceBaseTimer and the random time added to it.
pub(crate) const PARENT_ANNCE_BASE_MS: u32 = 10_000;
pub(crate) const PARENT_ANNCE_JITTER_MS: u32 = 10_000;
const PAN_REPORT_QUIET_MS: u32 = 60_000;
/// After a power cycle, the first Link Status goes this long (plus up to a
/// second) after the start.
pub(crate) const RESUME_LINK_STATUS_MS: u32 = 3_000;

impl<H: Hal> Stack<H> {
    /// The network has gone silent: scan all channels for its beacons, then
    /// carry on where they are (or where we were).
    pub(crate) fn search_network(&mut self, now: u32) {
        self.searching = Some(self.nib.channel);
        self.state = State::Scan;
        self.scan_index = 0;
        self.scan_found = [None; CANDIDATES];
        self.scan_next(now);
    }

    /// Notes a router heard in the scan. The list is kept best first: those
    /// that say they have room for a router and are heard with a link cost
    /// of 3 at most come before the others; for a rejoin, among those, the
    /// ones nearer the coordinator first (R22 3.6.1.4.2; for a join the
    /// depth does not count in Zigbee PRO, 3.6.1.4.1); and beyond that the
    /// better heard before the worse. (The text would not consider a parent
    /// without room, or one heard worse than that, at all. Here such a one
    /// is tried last instead of never: the alternative is to stay off the
    /// network, and the cost threshold has not been checked against this
    /// radio.)
    fn candidate_add(&mut self, c: Candidate) {
        let rejoin = self.rejoin.is_some();
        let rank = |c: &Candidate| {
            let suitable = c.beacon.router_capacity && nwk::cost_from_lqi(c.lqi) <= 3;
            (suitable, if rejoin && suitable { 15 - c.beacon.depth.min(15) } else { 0 }, c.lqi)
        };
        let same = |o: &Candidate| o.beacon.pan == c.beacon.pan && o.beacon.short == c.beacon.short && o.channel == c.channel;
        let mut list = self.scan_found;
        if let Some(i) = list.iter().position(|o| o.as_ref().is_some_and(same)) {
            // one router, heard again: its better beacon counts
            if list[i].is_some_and(|o| rank(&o) >= rank(&c)) {
                return;
            }
            list[i..].rotate_left(1);
            list[CANDIDATES - 1] = None;
        }
        let at = list.iter().position(|o| o.is_none_or(|o| rank(&o) < rank(&c)));
        if let Some(at) = at {
            list[at..].rotate_right(1);
            list[at] = Some(c);
        }
        self.scan_found = list;
    }

    /// The best router of the scan not tried yet.
    fn candidate_take(&mut self) -> Option<Candidate> {
        let c = self.scan_found[0].take();
        self.scan_found.rotate_left(1);
        c
    }

    /// Rejoins the network this node belongs to: finds a router of it, then
    /// asks it for (re)admission. Secured with the network key when the node
    /// still has the current one; without it the Trust Center has to send
    /// the key again (encrypted with this node's link key).
    pub(crate) fn start_rejoin(&mut self, now: u32, secure: bool) {
        self.log_event(super::diag::ev::REJOIN, secure as u16, 0);
        self.rejoin = Some(secure);
        self.search_network(now);
    }

    /// Asks the next router found in the search for readmission; false
    /// when none is left to ask.
    pub(crate) fn rejoin_next(&mut self, now: u32, secure: bool) -> bool {
        let Some(c) = self.candidate_take() else {
            return false;
        };
        let parent = c.beacon.short;
        let mut h = self.new_header(nwk::FRAME_CMD, parent, 1);
        h.security = secure;
        // "allocate address" is 0: the node has its address (R22 3.6.1.4.2)
        self.transmit_nwk(parent, &h, &[nwk::CMD_REJOIN_REQUEST, mac::CAPABILITY_ROUTER & !mac::CAPABILITY_ALLOCATE_ADDRESS]);
        self.rejoin_wait = Some((secure, now.wrapping_add(REJOIN_WAIT_MS), parent, c.beacon.depth));
        true
    }

    /// Rejoin Response: `p` starts at the command identifier.
    pub(crate) fn on_rejoin_response(&mut self, h: &nwk::Header, mac_src: u16, p: &[u8]) {
        let Some((secure, _, parent, depth)) = self.rejoin_wait else {
            return;
        };
        if mac_src != parent || p.len() < 4 {
            return;
        }
        // It has to be for this device and from the router that was asked
        // (R22 3.6.1.4.2), as far as the addresses are there to compare.
        let from_parent = match (h.src_ext, self.ext_of(parent)) {
            (Some(from), Some(known)) => from == known,
            _ => true,
        };
        if h.dst_ext.is_some_and(|e| e != self.ext) || !from_parent {
            return;
        }
        self.rejoin_wait = None;
        let short = u16::from_le_bytes([p[1], p[2]]);
        let now = self.hal.now_ms();
        if p[3] != 0 || short >= 0xFFF8 {
            // refused: on to the next router that could be the parent
            self.rejoin_next(now, secure);
            return;
        }
        self.scan_found = [None; CANDIDATES];
        self.nib.short = short;
        self.nib.parent = parent;
        self.nib.depth = depth.saturating_add(1).min(15);
        self.link_proven(parent);
        self.hal.set_filter(self.nib.pan, short);
        self.store_network();
        if secure {
            self.announce_at = Some(now.wrapping_add(50));
        } else {
            // readmitted; now the Trust Center sends the network key
            self.state = State::WaitKey;
            self.timer = now.wrapping_add(KEY_WAIT_MS);
        }
    }

    /// Back to square one: no network, scan again after a pause.
    pub(crate) fn down(&mut self, now: u32) {
        self.state = State::Down;
        self.steering = 0;
        self.hal.set_filter(0xFFFF, 0xFFFF);
        // between scans: on the first touchlink channel, where an initiator looks first
        self.nib.channel = CHANNELS[0];
        self.hal.set_channel(CHANNELS[0]);
        self.timer = now.wrapping_add(RETRY_MS + self.hal.random() % 2000);
    }

    pub(crate) fn join_timers(&mut self, now: u32) {
        if !due(now, self.timer) {
            return;
        }
        // A touchlink initiator has this bulb's scan response and will speak to
        // it on this channel: the search for a network to join waits there
        // until the transaction is over.
        if matches!(self.state, State::Down | State::Scan) && self.searching.is_none() {
            if let Some(t) = self.touchlink.filter(|t| !due(now, t.until)) {
                self.timer = t.until;
                return;
            }
        }
        match self.state {
            State::Down => {
                self.state = State::Scan;
                self.scan_index = 0;
                self.scan_found = [None; CANDIDATES];
                self.scan_next(now);
            }
            State::Scan => {
                let found = self.scan_found[0];
                let all = self.scan_index as usize >= CHANNELS.len();
                if let Some(home) = self.searching {
                    // looking for our own network: the first channel it is heard on
                    if found.is_none() && !all {
                        self.scan_next(now);
                        return;
                    }
                    self.searching = None;
                    // back to work: on the channel the network was found on, or the old one
                    if let Some(c) = found {
                        self.nib.channel = c.channel;
                        self.nib.pan = c.beacon.pan;
                        self.nib.update_id = c.beacon.update_id;
                        self.store_network();
                    } else {
                        self.nib.channel = home;
                    }
                    self.hal.set_channel(self.nib.channel);
                    self.hal.set_filter(self.nib.pan, self.nib.short);
                    self.state = State::Up;
                    self.heard_at = now;
                    self.log_event(super::diag::ev::UP, self.nib.short, self.nib.parent);
                    match self.rejoin.take() {
                        // the routers heard are asked one after the other
                        Some(secure) => {
                            self.rejoin_next(now, secure);
                        }
                        // The network moved to another channel while this
                        // node was away (a channel change it missed): a
                        // rejoin, secured with the key it has, tells the
                        // network where it is again, and the announcement
                        // after the answer tells every device (R22 3.6.1.4.2;
                        // BDB 7.1 recommends a router to find out whether its
                        // network has moved and "take corrective action").
                        // Its neighbors and the coordinator would otherwise
                        // not learn that it is back.
                        None if found.is_some_and(|c| c.channel != home) => {
                            self.rejoin_next(now, true);
                        }
                        None => self.scan_found = [None; CANDIDATES],
                    }
                } else if all || (self.scan_index == PRIMARY_CHANNELS && found.is_some()) {
                    // The primary channels have been scanned and there is
                    // something to try, or all channels have (BDB 8.2 steps
                    // 5 to 7 and 13).
                    self.join_next(now);
                } else {
                    self.scan_next(now);
                }
            }
            State::AssocPoll => {
                // ask the parent for the pending association response
                let mut f = Buf::new();
                mac::Header {
                    frame_type: mac::FRAME_CMD,
                    security: false,
                    pending: false,
                    ack_request: true,
                    seq: self.next_mac_seq(),
                    dst_pan: self.nib.pan,
                    dst: Addr::Short(self.nib.parent),
                    src_pan: self.nib.pan,
                    src: Addr::Ext(self.ext),
                }
                .write(&mut f);
                f.u8(mac::CMD_DATA_REQ);
                if self.mac_transmit(f.as_slice()) == TxStatus::Ok {
                    self.state = State::AssocWait;
                    self.timer = now.wrapping_add(RESPONSE_WAIT_MS);
                } else {
                    self.join_next(now);
                }
            }
            State::AssocWait => self.join_next(now),
            // no key after an association: on to the next network (BDB 8.2
            // step 10); no key after a rejoin: nothing else to try
            State::WaitKey if self.steering == STEERING_JOINING => self.join_next(now),
            State::WaitKey => self.down(now),
            State::Up => {}
        }
    }

    /// Tries to join through the best router of the scan that has not been
    /// tried yet: a router that turns the node away, does not answer or
    /// whose network sends no key is followed by the next one (R22
    /// 3.6.1.4.1, BDB 8.2 steps 8 and 10). When none is left, the channels
    /// not scanned yet follow (step 13), and after them a pause and a new
    /// round (step 17). A round tries every router heard once, so that one
    /// network is not tried over and over while another is in range (step
    /// 8: at most bdbcMaxSameNetworkRetryAttempts times in succession).
    fn join_next(&mut self, now: u32) {
        match self.candidate_take() {
            Some(c) => self.associate(now, &c),
            None if (self.scan_index as usize) < CHANNELS.len() => {
                self.state = State::Scan;
                self.scan_next(now);
            }
            None => self.down(now),
        }
    }

    fn scan_next(&mut self, now: u32) {
        let channel = CHANNELS[self.scan_index as usize % CHANNELS.len()];
        self.scan_index += 1;
        self.nib.channel = channel;
        self.hal.set_channel(channel);
        if self.searching.is_none() {
            self.hal.set_filter(0xFFFF, 0xFFFF);
        }
        let mut f = Buf::new();
        mac::Header {
            frame_type: mac::FRAME_CMD,
            security: false,
            pending: false,
            ack_request: false,
            seq: self.next_mac_seq(),
            dst_pan: 0xFFFF,
            dst: Addr::Short(0xFFFF),
            src_pan: 0,
            src: Addr::None,
        }
        .write(&mut f);
        f.u8(mac::CMD_BEACON_REQ);
        self.mac_transmit(f.as_slice());
        self.timer = self.hal.now_ms().wrapping_add(SCAN_MS);
        let _ = now;
    }

    pub(crate) fn on_beacon(&mut self, h: &mac::Header, payload: &[u8], lqi: u8) {
        let Some(b) = mac::Beacon::parse(h, payload) else {
            return;
        };
        if self.state == State::Up {
            // another network under our PAN identifier: the network manager should know
            if b.pan == self.nib.pan && b.ext_pan != self.nib.ext_pan && b.ext_pan != 0 {
                self.report_pan_conflict();
            }
            return;
        }
        if self.state != State::Scan {
            return;
        }
        if self.searching.is_some() {
            // looking for our own network: only its extended PAN identifier counts
            if b.ext_pan != self.nib.ext_pan {
                return;
            }
        } else if b.stack_profile != 2 || !b.permit_join || !b.router_capacity || b.pan == 0xFFFF {
            return;
        }
        self.candidate_add(Candidate { channel: self.nib.channel, lqi, beacon: b });
    }

    fn associate(&mut self, now: u32, c: &Candidate) {
        let tclk = self.nib.tclk;
        self.nib = Nib::empty();
        // an installed (non-default) link key would be kept here; today the
        // key is always the well-known one
        let _ = tclk;
        self.nib.tclk = DEFAULT_TCLK;
        self.nib.channel = c.channel;
        self.nib.pan = c.beacon.pan;
        self.nib.parent = c.beacon.short;
        self.nib.ext_pan = c.beacon.ext_pan;
        self.nib.update_id = c.beacon.update_id;
        // one more than the parent's, as far as the beacon's four bits go (R22 3.6.1.4.1)
        self.nib.depth = c.beacon.depth.saturating_add(1).min(15);
        self.join_lqi = c.lqi;
        self.steering = STEERING_JOINING;
        self.hal.set_channel(c.channel);
        self.hal.set_filter(c.beacon.pan, 0xFFFF);
        let mut f = Buf::new();
        mac::Header {
            frame_type: mac::FRAME_CMD,
            security: false,
            pending: false,
            ack_request: true,
            seq: self.next_mac_seq(),
            dst_pan: c.beacon.pan,
            dst: Addr::Short(c.beacon.short),
            src_pan: 0xFFFF,
            src: Addr::Ext(self.ext),
        }
        .write(&mut f);
        f.u8(mac::CMD_ASSOC_REQ);
        f.u8(mac::CAPABILITY_ROUTER);
        if self.mac_transmit(f.as_slice()) == TxStatus::Ok {
            self.state = State::AssocPoll;
            self.timer = self.hal.now_ms().wrapping_add(RESPONSE_WAIT_MS);
        } else {
            self.join_next(now);
        }
    }

    pub(crate) fn on_mac_command(&mut self, h: &mac::Header, payload: &[u8]) {
        match payload.first() {
            Some(&mac::CMD_ASSOC_RSP) if self.state == State::AssocWait => {
                if h.dst != Addr::Ext(self.ext) || payload.len() < 4 {
                    return;
                }
                let short = u16::from_le_bytes([payload[1], payload[2]]);
                let now = self.hal.now_ms();
                if payload[3] != 0 || short >= 0xFFF8 {
                    self.join_next(now);
                    return;
                }
                self.nib.short = short;
                self.hal.set_filter(self.nib.pan, short);
                self.clear_tables();
                let parent_ext = match h.src {
                    Addr::Ext(e) => e,
                    _ => 0,
                };
                self.neighbors[0] =
                    Neighbor { used: true, short: self.nib.parent, ext: parent_ext, lqi: self.join_lqi, age: 0, confirmed: true, ..Neighbor::EMPTY };
                self.link_proven(self.nib.parent);
                self.tc_fc_next = 0;
                self.state = State::WaitKey;
                self.timer = now.wrapping_add(KEY_WAIT_MS);
            }
            Some(&mac::CMD_ASSOC_REQ) if self.state == State::Up => {
                if let (Addr::Short(dst), Addr::Ext(ext), Some(&capability)) = (h.dst, h.src, payload.get(1)) {
                    if dst == self.nib.short && h.dst_pan == self.nib.pan {
                        self.on_association_request(ext, capability);
                    }
                }
            }
            Some(&mac::CMD_DATA_REQ) if self.state == State::Up => {
                if h.dst == Addr::Short(self.nib.short) && h.dst_pan == self.nib.pan {
                    self.on_data_request(h.src);
                }
            }
            Some(&mac::CMD_ORPHAN) if self.state == State::Up => {
                // an end device that lost its parent asks whether anyone knows it
                if let Addr::Ext(ext) = h.src {
                    if let Some(i) = self.child_by_ext(ext) {
                        let short = self.children[i].short;
                        let mut f = Buf::new();
                        mac::Header {
                            frame_type: mac::FRAME_CMD,
                            security: false,
                            pending: false,
                            ack_request: true,
                            seq: self.next_mac_seq(),
                            dst_pan: 0xFFFF,
                            dst: Addr::Ext(ext),
                            src_pan: self.nib.pan,
                            src: Addr::Ext(self.ext),
                        }
                        .write(&mut f);
                        // Coordinator Realignment: our network, us, the channel, its address
                        f.u8(mac::CMD_REALIGNMENT);
                        f.u16(self.nib.pan);
                        f.u16(self.nib.short);
                        f.u8(self.nib.channel);
                        f.u16(short);
                        self.mac_transmit(f.as_slice());
                        self.children[i].heard = self.hal.now_ms();
                    }
                }
            }
            Some(&mac::CMD_BEACON_REQ) if self.state == State::Up => {
                // Every router in range hears the same request: answer after
                // a random pause, so that the beacons do not all collide.
                if self.beacon_at.is_none() {
                    let now = self.hal.now_ms();
                    self.beacon_at = Some(now.wrapping_add(4 + self.hal.random() % 28));
                }
            }
            _ => {}
        }
    }

    /// Network Report "PAN identifier conflict" to the network manager, which
    /// may move the network to another identifier (Network Update). At most
    /// once a minute.
    fn report_pan_conflict(&mut self) {
        let now = self.hal.now_ms();
        if !due(now, self.pan_report_at) || self.manager == self.nib.short {
            return;
        }
        self.pan_report_at = now.wrapping_add(PAN_REPORT_QUIET_MS);
        let h = self.new_header(nwk::FRAME_CMD, self.manager, nwk::DEFAULT_RADIUS);
        let mut p = Buf::new();
        // one PAN identifier in the report, report type 0 (PAN identifier conflict)
        p.u8(nwk::CMD_NETWORK_REPORT);
        p.u8(0x01);
        p.u64(self.nib.ext_pan);
        p.u16(self.nib.pan);
        self.nwk_send(&h, p.as_slice());
    }

    /// Sends this router's beacon.
    pub(crate) fn send_beacon(&mut self) {
        let permit_join = self.permit_open();
        let b = mac::Beacon {
            pan: self.nib.pan,
            short: self.nib.short,
            permit_join,
            pan_coordinator: false,
            stack_profile: 2,
            protocol_version: 2,
            router_capacity: self.has_child_room(),
            end_device_capacity: self.has_end_device_room(),
            depth: self.nib.depth,
            ext_pan: self.nib.ext_pan,
            update_id: self.nib.update_id,
        };
        let mut f = Buf::new();
        b.write(self.next_mac_seq(), &mut f);
        self.mac_transmit(f.as_slice());
    }

    /// Takes the network a touchlink initiator handed over: leaves the old
    /// one, if any, and starts operating on the new one.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn adopt_network(&mut self, now: u32, channel: u8, pan: u16, ext_pan: u64, update_id: u8, short: u16, key: [u8; 16]) {
        if self.state == State::Up {
            self.send_leave();
        }
        self.leave_network(now);
        self.nib.channel = channel;
        self.nib.pan = pan;
        self.nib.ext_pan = ext_pan;
        self.nib.update_id = update_id;
        self.nib.short = short;
        self.nib.key = key;
        self.nib.key_seq = 0;
        self.nib.tc_ext = super::NO_TRUST_CENTER;
        self.hal.set_channel(channel);
        self.hal.set_filter(pan, short);
        self.network_up(now);
    }

    /// The network key arrived: the node is authenticated.
    pub(crate) fn network_up(&mut self, now: u32) {
        if self.nib.tc_ext == super::NO_TRUST_CENTER {
            // no Trust Center, no link key to exchange with it
            self.nib.tclk_state = 3;
        }
        self.key = Aes128::with_engine(&self.nib.key, H::AES);
        self.state = State::Up;
        self.heard_at = now;
        self.stats.joins = self.stats.joins.wrapping_add(1);
        self.log_event(super::diag::ev::UP, self.nib.short, self.nib.parent);
        self.scan_found = [None; CANDIDATES];
        if self.steering == STEERING_JOINING && self.nib.tclk_state == 3 {
            // no link key to exchange first (BDB 8.2 step 11)
            self.steering = STEERING_OPEN_DUE;
        }
        self.store_network();
        self.announce_at = Some(now.wrapping_add(50));
        self.link_status_at = now.wrapping_add(1000 + self.hal.random() % 1000);
        self.age_at = now.wrapping_add(super::LINK_STATUS_MS);
        // the link key exchange is done once per link key, not after every rejoin
        self.tclk_step = if self.nib.tclk_state == 0 { 1 } else { 0 };
        self.tclk_tries = 0;
        self.tclk_at = now.wrapping_add(500);
        self.unknown_key_frames = 0;
    }

    /// The join by association is complete, link key exchange included:
    /// what remains is to open the network for others (see `open_after_join`).
    pub(crate) fn steering_done(&mut self) {
        if self.steering == STEERING_JOINING {
            self.steering = STEERING_OPEN_DUE;
        }
    }

    /// The last steps of joining a network by association (BDB 8.2 steps 14
    /// and 15): everyone is asked to permit joining for the commissioning
    /// time, and this router does so itself, so that what is being added in
    /// the same session can join through the newcomer too.
    pub(crate) fn open_after_join(&mut self, now: u32) {
        self.steering = 0;
        self.permit_until = Some(now.wrapping_add(COMMISSIONING_S as u32 * 1000));
        self.zdo_permit_joining_request(COMMISSIONING_S);
    }

    /// The Trust Center link key exchange has failed. When it was the last
    /// part of joining, the node leaves the network and forgets it (BDB 8.2
    /// step 12), and looks for a network anew. (An earlier version stayed
    /// on, to be kind to Trust Centers that are slow or never answer. But a
    /// node that stays keeps the well-known link key, under which the Trust
    /// Center goes on sending it every future network key, readable by
    /// anyone; and the text leaves no choice.) An exchange taken up again
    /// after a power cycle that cut it short is another matter: the node
    /// has been on the network since, the text has no such case, and a
    /// Trust Center that is out of reach when the power comes back must not
    /// cost the node its network. It stays, and tries again next time.
    fn tclk_failed(&mut self, now: u32) {
        self.tclk_step = 0;
        if self.steering == STEERING_JOINING {
            self.leave_rejoin = false;
            // (announced and done by the main timers, like any other leave)
            self.leave_at = Some(now);
        }
    }

    /// Picks the stored network up again after a power cycle.
    pub(crate) fn resume(&mut self, now: u32) {
        self.heard_at = now;
        self.hal.set_channel(self.nib.channel);
        self.hal.set_filter(self.nib.pan, self.nib.short);
        self.state = State::Up;
        self.announce_at = Some(now.wrapping_add(200 + self.hal.random() % 300));
        self.booted = now;
        if self.children.iter().any(|c| c.used && c.capability & super::parent::CAP_FFD == 0) {
            self.parent_annce_from = 0;
            self.parent_annce_at = Some(now.wrapping_add(PARENT_ANNCE_BASE_MS + self.hal.random() % PARENT_ANNCE_JITTER_MS));
        }
        // The first Link Status after a power cycle waits until the
        // neighbors have been heard (their early Link Status answers our
        // Device_annce): one that lists only some of them would make the
        // others take the link for one-way and turn our route requests away.
        self.link_status_at = now.wrapping_add(RESUME_LINK_STATUS_MS + self.hal.random() % 1000);
        self.age_at = now.wrapping_add(super::LINK_STATUS_MS);
        self.tclk_tries = 0;
        self.tclk_at = now.wrapping_add(1000);
        self.tclk_step = match self.nib.tclk_state {
            0 => 1,
            1 => 5,
            _ => 0,
        };
    }

    /// Forgets the network (after a Leave) and starts looking for one again.
    pub(crate) fn leave_network(&mut self, now: u32) {
        self.log_event(super::diag::ev::LEFT, self.nib.short, 0);
        self.hal.nv_erase();
        // out of the network BLE is the way to the bulb: back to running it always
        self.ble_mode = super::BleMode::On;
        self.ble_window = super::BLE_WINDOW_DEFAULT_MIN;
        // the sensing's configuration went with the store
        #[cfg(feature = "sense")]
        self.sense_forget();
        self.children_stored = [super::nv::NO_CHILDREN; super::nv::CHILDREN_PARTS];
        // the frame counters must outlive the network: neighbors remember them
        self.store_counters();
        // and so do the diagnostic counters: they are the device's (a
        // factory reset clears them, ZCL 3.15.2.2.1.1)
        self.diag_keep();
        self.xreports = [super::objects::XReport::EMPTY; super::objects::XREPORTS];
        self.allow_children = true;
        self.nib = Nib::empty();
        self.clear_tables();
        self.groups = [0; super::GROUPS];
        self.bindings = [super::Binding::EMPTY; super::BINDINGS];
        self.scenes = [super::Scene::EMPTY; super::SCENES];
        self.scene_valid = false;
        self.proxies = [super::gp::ProxyEntry::EMPTY; super::gp::PROXIES];
        self.gp_mode = None;
        self.gp_tx = None;
        self.gp_pending = [None; 2];
        self.gp_keys = super::gp::Keys::DEFAULT;
        self.children = [super::parent::Child::EMPTY; super::CHILDREN];
        self.indirect = [super::parent::Indirect::EMPTY; super::INDIRECT];
        self.permit_until = None;
        self.hal.indirect_pending(&[], &[]);
        // what was learned of the old network's devices means nothing in
        // the next one (R22 3.6.1.10.4), and what was configured over the
        // old network goes with it (BDB 9.3: "much the same state as it was
        // when it left the factory")
        self.addr_map = [(0, 0xFFFF); super::ADDR_MAP];
        self.manager = 0x0000;
        for (r, (min_s, max_s, change)) in self.reports.iter_mut().zip(super::REPORT_DEFAULTS) {
            (r.min_s, r.max_s, r.change) = (min_s, max_s, change);
        }
        self.tclk_step = 0;
        self.announce_at = None;
        self.beacon_at = None;
        self.searching = None;
        self.rejoin = None;
        self.rejoin_wait = None;
        self.leave_rejoin = false;
        self.leave_at = None;
        self.channel_at = None;
        self.pan_at = None;
        self.conflict_due = None;
        self.down(now);
    }

    fn clear_tables(&mut self) {
        self.neighbors = [Neighbor::EMPTY; super::NEIGHBORS];
        self.routes = [super::Route::EMPTY; super::ROUTES];
        self.discoveries = [super::Discovery::EMPTY; super::DISCOVERIES];
        self.seen = [super::Seen::EMPTY; super::BTT];
        self.aps_dups = [super::Seen::EMPTY; super::APS_DUPS];
        for h in self.held.iter_mut().chain(self.bcast.iter_mut()) {
            h.used = false;
        }
    }

    /// Trust Center link key exchange (BDB 10.2.5): find out whether the
    /// Trust Center does it at all (stack revision 21 or later in its node
    /// descriptor), request a key, prove possession of it.
    pub(crate) fn tclk_timers(&mut self, now: u32) {
        if self.tclk_step == 0 || !due(now, self.tclk_at) {
            return;
        }
        match self.tclk_step {
            1 | 3 | 5 => {
                if self.tclk_tries >= TCLK_ATTEMPTS {
                    // no answer to any of the attempts (BDB 10.2.5 steps 4, 8 and 12)
                    self.tclk_failed(now);
                    return;
                }
                self.tclk_tries += 1;
                match self.tclk_step {
                    1 => self.zdo_node_desc_request(),
                    3 => self.aps_request_key(),
                    _ => self.aps_verify_key(),
                }
                self.tclk_step += 1;
                self.tclk_at = now.wrapping_add(TCLK_WAIT_MS);
            }
            // a wait ran out: repeat the request
            _ => self.tclk_step -= 1,
        }
    }

    /// Advances the exchange when the awaited answer arrives.
    pub(crate) fn tclk_advance(&mut self, to: u8) {
        self.tclk_step = to;
        self.tclk_tries = 0;
        self.tclk_at = self.hal.now_ms();
    }

    /// The Trust Center answered a key request with something that is no
    /// new link key (BDB 10.2.5 step 9): the exchange has failed.
    pub(crate) fn tclk_refused(&mut self) {
        let now = self.hal.now_ms();
        self.tclk_failed(now);
    }
}
