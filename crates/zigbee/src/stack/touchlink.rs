//! Touchlink, target side: answer a scan from close by, blink on Identify,
//! leave the network on Reset to Factory New, take the network an
//! initiator hands over (Network Join Router) or start one for it (Network
//! Start), follow it to another channel (Network Update). Such networks
//! have no Trust Center (distributed security).
//!
//! A bulb that is on a network with a Trust Center is not for the taking:
//! there it answers scans and identifies itself, and refuses the rest. (The
//! Base Device Behavior specification closed touchlink "stealing" this way;
//! Telink's target, which `emulator/rust_touchlink_sim.py` runs beside this
//! one, does the same.)
//!
//! The frames are inter-PAN: MAC data frames with extended addresses, a
//! two-byte NWK stub (frame type 3), an APS stub without endpoints, then ZCL
//! of the Touchlink Commissioning cluster. They go to the broadcast PAN
//! (an initiator that has no network listens on no other), from the PAN of
//! the bulb's network, or from a made-up one while it has none.
//!
//! The network key travels encrypted with a key derived from the
//! transaction. The development key (index 0) and the certification key
//! (index 15) are public. The master key (index 4) that commercial
//! initiators use is issued under licence and is not in this code; a build
//! that is given it (`Identity::touchlink_master_key`) offers and accepts
//! index 4 as well, by the same derivation as the certification key.
//! Without it a request under index 4 is answered with a failure.

use super::{Stack, State};
use crate::aes::Aes128;
use crate::hal::Hal;
use crate::mac::{self, Addr};
use crate::util::{due, Buf, Reader};

const PROFILE_ZLL: u16 = 0xC05E;
const CLUSTER_TOUCHLINK: u16 = 0x1000;

const SCAN_REQUEST: u8 = 0x00;
const SCAN_RESPONSE: u8 = 0x01;
const DEVICE_INFORMATION_REQUEST: u8 = 0x02;
const DEVICE_INFORMATION_RESPONSE: u8 = 0x03;
const IDENTIFY_REQUEST: u8 = 0x06;
const RESET_TO_FACTORY_NEW_REQUEST: u8 = 0x07;
const NETWORK_START_REQUEST: u8 = 0x10;
const NETWORK_START_RESPONSE: u8 = 0x11;
const NETWORK_JOIN_ROUTER_REQUEST: u8 = 0x12;
const NETWORK_JOIN_ROUTER_RESPONSE: u8 = 0x13;
const NETWORK_JOIN_END_DEVICE_REQUEST: u8 = 0x14;
const NETWORK_JOIN_END_DEVICE_RESPONSE: u8 = 0x15;
const NETWORK_UPDATE_REQUEST: u8 = 0x16;

const KEY_DEVELOPMENT: u8 = 0;
const KEY_MASTER: u8 = 4;
const KEY_CERTIFICATION: u8 = 15;
/// The certification key of the Zigbee Light Link specification.
const CERTIFICATION_KEY: [u8; 16] =
    [0xC0, 0xC1, 0xC2, 0xC3, 0xC4, 0xC5, 0xC6, 0xC7, 0xC8, 0xC9, 0xCA, 0xCB, 0xCC, 0xCD, 0xCE, 0xCF];

/// An open touchlink transaction.
#[derive(Clone, Copy)]
pub(crate) struct Transaction {
    pub id: u32,
    pub initiator: u64,
    pub until: u32,
    /// The response identifier we chose in the scan response.
    pub response: u32,
    /// Source PAN identifier of our frames while the bulb has no network of
    /// its own: any value that is a PAN identifier.
    pub pan: u16,
}

/// Decrypts the network key of a start or join request. `None` for a key
/// index this firmware has no key for.
fn network_key(index: u8, encrypted: &[u8; 16], transaction: u32, response: u32, master: Option<&[u8; 16]>) -> Option<[u8; 16]> {
    let (t, r) = (transaction.to_be_bytes(), response.to_be_bytes());
    let mut input = [0u8; 16];
    input[4..8].copy_from_slice(&t);
    input[12..16].copy_from_slice(&r);
    let transport = match index {
        KEY_DEVELOPMENT => {
            input[..4].copy_from_slice(b"PhLi");
            input[8..12].copy_from_slice(b"CLSN");
            input
        }
        KEY_CERTIFICATION | KEY_MASTER => {
            let key = if index == KEY_MASTER { master? } else { &CERTIFICATION_KEY };
            input[..4].copy_from_slice(&t);
            input[8..12].copy_from_slice(&r);
            Aes128::new(key).encrypt(&mut input);
            input
        }
        _ => return None,
    };
    let mut key = *encrypted;
    Aes128::new(&transport).decrypt(&mut key);
    Some(key)
}

/// aplcInterPANTransIdLifetime: how long a scan's transaction stays open.
const TRANSACTION_MS: u32 = 8_000;
/// Touchlink is for devices held next to each other: scans that arrive
/// weaker than this link quality (about -45 dBm) are not answered.
const MIN_LQI: u8 = 164;
/// Identify time when the request leaves it to the target.
const DEFAULT_IDENTIFY_S: u16 = 3;
/// Scan request, touchlink information field: the sender is a link initiator.
const LINK_INITIATOR: u8 = 0x10;
/// bdbcTLPrimaryChannelSet: where a network started by touchlink goes when
/// the initiator leaves the channel to the target.
const PRIMARY_CHANNELS: [u8; 4] = [11, 15, 20, 25];

impl<H: Hal> Stack<H> {
    /// An inter-PAN frame: `p` starts at the NWK stub.
    pub(crate) fn on_interpan(&mut self, h: &mac::Header, p: &[u8], lqi: u8) -> Option<()> {
        let Addr::Ext(initiator) = h.src else {
            return None;
        };
        let mut r = Reader::new(p);
        r.skip(2)?;
        // APS stub: frame type 3; a group address only with group delivery
        let aps = r.u8()?;
        if aps & 3 != 3 {
            return None;
        }
        if (aps >> 2) & 3 == 3 {
            r.skip(2)?;
        }
        if r.u16()? != CLUSTER_TOUCHLINK || r.u16()? != PROFILE_ZLL {
            return None;
        }
        let fc = r.u8()?;
        let seq = r.u8()?;
        let cmd = r.u8()?;
        // cluster commands from the initiator (client) only
        if fc & 0x0F != 0x01 {
            return None;
        }
        let transaction = r.u32()?;
        let now = self.hal.now_ms();
        if cmd == SCAN_REQUEST {
            // Zigbee information, touchlink information: only a link initiator is answered
            let (_, information) = (r.u8()?, r.u8()?);
            if lqi < MIN_LQI || transaction == 0 || information & LINK_INITIATOR == 0 {
                return None;
            }
            // A repeat of the scan we answered: an initiator broadcasts five
            // scan requests of one transaction on its first channel (BDB 3.0.1
            // 8.6 step 3). Discarded (8.7: a target "MAY restart the procedure
            // again from the beginning or discard the frame"): restarting would
            // pick a new response identifier, and an initiator that kept an
            // earlier response would derive the transport key from one this
            // bulb no longer has (ZCL 8 13.3.4.11), so the network key it hands
            // over would not decrypt. A new transaction starts afresh.
            if let Some(t) = self.touchlink {
                if t.id == transaction && t.initiator == initiator && !due(now, t.until) {
                    return Some(());
                }
            }
            let response_id = self.hal.random() | 1;
            let pan = (self.hal.random() % 0xFFFE) as u16 + 1;
            self.touchlink =
                Some(Transaction { id: transaction, initiator, until: now.wrapping_add(TRANSACTION_MS), response: response_id, pan });
            let on_network = self.state == State::Up;
            // An initiator may answer to the PAN our frames name instead of
            // the broadcast PAN. A bulb that waits for a network to join
            // listens for that too until it scans again (`join_timers`).
            if matches!(self.state, State::Down | State::Scan) && self.searching.is_none() {
                self.hal.set_filter(pan, 0xFFFF);
            }
            let mut f = self.interpan_frame(initiator, seq, SCAN_RESPONSE, transaction);
            f.u8(0); // RSSI correction
            f.u8(0x05); // router, receiver on when idle
            // factory new?; cannot assign addresses, not an initiator; Zigbee 3.0 ("profile interop")
            f.u8(!on_network as u8 | 0x80);
            let master = self.identity.touchlink_master_key.is_some() as u16;
            f.u16(1 << KEY_DEVELOPMENT | master << KEY_MASTER | 1 << KEY_CERTIFICATION);
            f.u32(response_id);
            f.u64(self.nib.ext_pan);
            f.u8(self.nib.update_id);
            f.u8(self.nib.channel);
            f.u16(self.nib.pan);
            f.u16(self.nib.short);
            f.u8(1); // one sub-device
            f.u8(0); // no group identifiers
            f.u8(super::aps::ENDPOINT);
            f.u16(super::aps::PROFILE_HA);
            f.u16(self.device_id());
            f.u8(1);
            f.u8(0);
            self.mac_transmit(f.as_slice());
            return Some(());
        }
        // everything else belongs to the transaction a scan opened
        let t = self.touchlink?;
        if t.id != transaction || t.initiator != initiator || due(now, t.until) {
            return None;
        }
        // on a network with a Trust Center the bulb stays where it is
        let centralized = self.state == State::Up && self.nib.tc_ext != super::NO_TRUST_CENTER;
        match cmd {
            DEVICE_INFORMATION_REQUEST => {
                let start = r.u8()?;
                let mut f = self.interpan_frame(initiator, seq, DEVICE_INFORMATION_RESPONSE, transaction);
                f.u8(1);
                f.u8(start);
                f.u8((start == 0) as u8);
                if start == 0 {
                    f.u64(self.ext);
                    f.u8(super::aps::ENDPOINT);
                    f.u16(super::aps::PROFILE_HA);
                    f.u16(self.device_id());
                    f.u8(1);
                    f.u8(0);
                    f.u8(0);
                }
                self.mac_transmit(f.as_slice());
            }
            IDENTIFY_REQUEST => {
                let seconds = match r.u16()? {
                    0xFFFF => DEFAULT_IDENTIFY_S,
                    s => s,
                };
                self.push_command(bulb_core::Command::Identify { times: seconds.min(255) as u8 });
            }
            RESET_TO_FACTORY_NEW_REQUEST => {
                self.touchlink = None;
                if self.state == State::Up && !centralized {
                    self.leave_at = Some(now.wrapping_add(100));
                }
            }
            // the network moved while this bulb was away: follow, if the news is newer than what it knows
            NETWORK_UPDATE_REQUEST => {
                let ext_pan = r.u64()?;
                let update_id = r.u8()?;
                let channel = r.u8()?;
                let pan = r.u16()?;
                let newer = update_id != self.nib.update_id && update_id.wrapping_sub(self.nib.update_id) < 0x80;
                if self.state == State::Up && !centralized && ext_pan == self.nib.ext_pan && pan == self.nib.pan && newer && (11..=26).contains(&channel) {
                    self.nib.update_id = update_id;
                    self.nib.channel = channel;
                    self.hal.set_channel(channel);
                    self.store_network();
                }
            }
            // this is a router
            NETWORK_JOIN_END_DEVICE_REQUEST => {
                let mut f = self.interpan_frame(initiator, seq, NETWORK_JOIN_END_DEVICE_RESPONSE, transaction);
                f.u8(0x01);
                self.mac_transmit(f.as_slice());
            }
            NETWORK_JOIN_ROUTER_REQUEST | NETWORK_START_REQUEST => {
                let mut ext_pan = r.u64()?;
                let key_index = r.u8()?;
                let encrypted = r.key()?;
                let update_id = if cmd == NETWORK_JOIN_ROUTER_REQUEST { r.u8()? } else { 0 };
                let mut channel = r.u8()?;
                let mut pan = r.u16()?;
                let short = r.u16()?;
                let key = network_key(key_index, &encrypted, transaction, t.response, self.identity.touchlink_master_key.as_ref());
                let valid = key.is_some() && short != 0 && short < 0xFFF8 && !centralized;
                if cmd == NETWORK_START_REQUEST {
                    // what the initiator leaves open is ours to choose
                    if ext_pan == 0 {
                        ext_pan = self.ext ^ ((self.hal.random() as u64) << 32 | self.hal.random() as u64) | 1;
                    }
                    if pan == 0 {
                        pan = (self.hal.random() % 0xFFF0) as u16 + 1;
                    }
                    if channel == 0 && valid {
                        channel = self.quietest_primary_channel();
                    }
                }
                let valid = valid && (11..=26).contains(&channel) && pan < 0xFFFF;
                let response = if cmd == NETWORK_START_REQUEST { NETWORK_START_RESPONSE } else { NETWORK_JOIN_ROUTER_RESPONSE };
                let mut f = self.interpan_frame(initiator, seq, response, transaction);
                f.u8(!valid as u8);
                if cmd == NETWORK_START_REQUEST {
                    f.u64(ext_pan);
                    f.u8(update_id);
                    f.u8(channel);
                    f.u16(pan);
                }
                self.mac_transmit(f.as_slice());
                self.touchlink = None;
                if valid {
                    self.adopt_network(now, channel, pan, ext_pan, update_id, short, key?);
                }
            }
            _ => {}
        }
        Some(())
    }

    /// The channel for a network of our own that the initiator left to us:
    /// the one of the touchlink primary channels with the least energy on
    /// it now (the first of equals). Whether another network uses the PAN
    /// identifier we then pick is not looked into.
    fn quietest_primary_channel(&mut self) -> u8 {
        let mut best = (PRIMARY_CHANNELS[0], u8::MAX);
        for channel in PRIMARY_CHANNELS {
            let energy = self.hal.energy_detect(channel);
            if energy < best.1 {
                best = (channel, energy);
            }
        }
        // back to where the initiator waits for the answer
        self.hal.set_channel(self.nib.channel);
        best.0
    }

    /// Starts an inter-PAN response to `initiator`: MAC header, NWK and APS
    /// stubs, ZCL header and the transaction identifier.
    fn interpan_frame(&mut self, initiator: u64, seq: u8, cmd: u8, transaction: u32) -> Buf {
        // from the bulb's PAN; without a network, from the one made up for the transaction
        let src_pan = match self.touchlink {
            Some(t) if self.state != State::Up => t.pan,
            _ => self.nib.pan,
        };
        let mut f = Buf::new();
        mac::Header {
            frame_type: mac::FRAME_DATA,
            security: false,
            pending: false,
            ack_request: true,
            seq: self.next_mac_seq(),
            // The broadcast PAN, whatever PAN the request names as its source:
            // an initiator without a network puts a made-up one there, and
            // its radio accepts no other than this.
            dst_pan: 0xFFFF,
            dst: Addr::Ext(initiator),
            src_pan,
            src: Addr::Ext(self.ext),
        }
        .write(&mut f);
        f.u16(0x000B); // NWK stub: inter-PAN, protocol version 2
        f.u8(0x03); // APS stub: inter-PAN, unicast
        f.u16(CLUSTER_TOUCHLINK);
        f.u16(PROFILE_ZLL);
        f.u8(0x19); // cluster command, server to client, no default response
        f.u8(seq);
        f.u8(cmd);
        f.u32(transaction);
        f
    }
}
