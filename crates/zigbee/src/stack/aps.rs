//! Application support sublayer: data frames with duplicate rejection and
//! acknowledgements, group addressing, and the security commands (Transport
//! Key, Switch Key, Request Key, Verify Key, Confirm Key).

use super::frag::{Addressing, SINGLE_MAX};
use super::gp::{CLUSTER_GP, GP_ENDPOINT, PROFILE_GP};
use super::diag::k;
use super::{Seen, Stack, State, APS_BROADCAST_RX_ON};
use crate::aes::Aes128;
use crate::crypto;
use crate::hal::Hal;
use crate::nwk;
use crate::util::{due, Buf, Reader};

pub(crate) const PROFILE_ZDO: u16 = 0x0000;
pub(crate) const PROFILE_HA: u16 = 0x0104;
pub(crate) const PROFILE_WILDCARD: u16 = 0xFFFF;
/// The light's endpoint.
pub(crate) const ENDPOINT: u8 = 1;

const CMD_TRANSPORT_KEY: u8 = 0x05;
pub(crate) const CMD_UPDATE_DEVICE: u8 = 0x06;
const CMD_REMOVE_DEVICE: u8 = 0x07;
const CMD_TUNNEL: u8 = 0x0E;
const CMD_REQUEST_KEY: u8 = 0x08;
const CMD_SWITCH_KEY: u8 = 0x09;
const CMD_VERIFY_KEY: u8 = 0x0F;
const CMD_CONFIRM_KEY: u8 = 0x10;

const KEY_NETWORK: u8 = 0x01;
const KEY_TC_LINK: u8 = 0x04;

/// What APS security adds to a frame: the auxiliary header (control, frame
/// counter, extended source) and the integrity code.
pub(crate) const SECURITY_OVERHEAD: usize = 13 + crypto::MIC_LEN;

/// How long a frame is remembered for the rejection of its duplicates: as
/// long as its sender may go on repeating it, which is apscMaxFrameRetries
/// (3) times, apscAckWaitDuration (1.6 s) apart (R22 2.2.8.4.2, 2.2.8.4.4),
/// and one such wait on top for the time the copies take to arrive.
const DUPLICATE_MS: u32 = 6_400;

/// The key an APS frame is secured with, from its key identifier: the link
/// key itself, or the key-transport or key-load key derived from it.
fn aps_key(link: &[u8; 16], key_id: u8) -> Option<[u8; 16]> {
    match key_id {
        0 => Some(*link),
        2 => Some(crypto::hmac_mmo(link, &[0x00])),
        3 => Some(crypto::hmac_mmo(link, &[0x02])),
        _ => None,
    }
}

impl<H: Hal> Stack<H> {
    pub(crate) fn on_aps(&mut self, h: &nwk::Header, p: &[u8]) {
        self.diag.add(if nwk::is_broadcast(h.dst) { k::APS_RX_BCAST } else { k::APS_RX_UCAST });
        match p.first().map(|fc| fc & 3) {
            Some(0) => {
                let _ = self.on_aps_data(h, p);
            }
            Some(1) => {
                let _ = self.on_aps_command(h, p);
            }
            Some(2) if self.state == State::Up => {
                let _ = self.on_aps_ack(h, p);
            }
            _ => {}
        }
    }

    fn on_aps_data(&mut self, h: &nwk::Header, p: &[u8]) -> Option<()> {
        if self.state != State::Up {
            return None;
        }
        let mut r = Reader::new(p);
        let fc = r.u8()?;
        let delivery = (fc >> 2) & 3;
        let dst_ep = match delivery {
            0 | 2 => r.u8()?,
            3 => {
                let group = r.u16()?;
                if group == 0 || !self.groups.contains(&group) {
                    return None;
                }
                ENDPOINT
            }
            _ => return None,
        };
        let cluster = r.u16()?;
        let profile = r.u16()?;
        let src_ep = r.u8()?;
        let counter = r.u8()?;
        let unicast = delivery == 0 && !nwk::is_broadcast(h.dst);
        if fc & 0x80 != 0 {
            // extended header: a block of a fragmented transfer
            let kind = r.u8()? & 3;
            let block = if kind != 0 { r.u8()? } else { 0 };
            if kind != 0 {
                // (fragments of an encrypted transfer are not supported)
                if !unicast || fc & 0x20 != 0 {
                    return None;
                }
                let a = Addressing { peer: h.src, dst_ep, src_ep, cluster, profile, counter };
                return self.on_fragment(a, kind, block, r.rest());
            }
        }
        // Data encrypted with a link key: the only link key this node has is
        // the Trust Center's, so only the Trust Center can send such frames.
        let mut plain = [0u8; 96];
        let secured = fc & 0x20 != 0;
        let payload = if secured {
            let header_len = r.pos();
            let len = self.aps_unsecure(h, p, header_len, &mut plain)?;
            &plain[..len]
        } else {
            r.rest()
        };
        if unicast && fc & 0x40 != 0 {
            let mut ack = Buf::new();
            ack.u8(if secured { 0x22 } else { 0x02 });
            ack.u8(src_ep);
            ack.u16(cluster);
            ack.u16(profile);
            ack.u8(dst_ep);
            ack.u8(counter);
            if secured {
                self.aps_secure(&mut ack, &[]);
            }
            self.nwk_data(h.src, ack.as_slice());
        }
        if unicast && self.aps_duplicate(h.src, counter) {
            return None;
        }
        // what was asked under encryption is answered under encryption
        self.reply_secured = secured;
        self.aps_deliver(h.src, dst_ep, src_ep, cluster, profile, payload, unicast);
        self.reply_secured = false;
        Some(())
    }

    /// Verifies and decrypts the payload of a data frame secured with the
    /// Trust Center link key. `p` is the whole APS frame, its header
    /// `header_len` bytes long. Returns the payload's length in `out`.
    fn aps_unsecure(&mut self, h: &nwk::Header, p: &[u8], header_len: usize, out: &mut [u8; 96]) -> Option<usize> {
        let mut a = Reader::new(p.get(header_len..)?);
        let sc = a.u8()?;
        let counter = a.u32()?;
        let ext = if sc & 0x20 != 0 { a.u64()? } else { self.nib.tc_ext };
        let aux_len = a.pos();
        let cipher = a.rest();
        // data key (key identifier 0) of the Trust Center
        if ext != self.nib.tc_ext || h.src != 0x0000 {
            // the only link key this node holds is the Trust Center's
            self.diag.add(k::APS_UNAUTHORIZED_KEY);
            return None;
        }
        if (sc >> 3) & 3 != 0 {
            return None;
        }
        if counter < self.tc_fc_next {
            self.stats.replays = self.stats.replays.wrapping_add(1);
            self.diag.add(k::APS_FC_FAILURE);
            return None;
        }
        let sc_calc = (sc & !7) | 5;
        let mut aad = [0u8; 12 + 13];
        let n = header_len + aux_len;
        aad.get_mut(..n)?.copy_from_slice(&p[..n]);
        aad[header_len] = sc_calc;
        let work = out.get_mut(..cipher.len())?;
        work.copy_from_slice(cipher);
        let len = crypto::ccm_decrypt(&Aes128::with_engine(&self.nib.tclk, H::AES), &nwk::nonce(ext, counter, sc_calc), &aad[..n], work);
        let Some(len) = len else {
            self.stats.bad_mic = self.stats.bad_mic.wrapping_add(1);
            self.diag.add(k::APS_DECRYPT_FAILURES);
            return None;
        };
        self.tc_fc_next = counter.wrapping_add(1);
        Some(len)
    }

    /// Appends the auxiliary header, the encrypted payload and the MIC to an
    /// APS frame whose header is in `f` (security bit set), using the Trust
    /// Center link key as data key.
    pub(crate) fn aps_secure(&mut self, f: &mut Buf, payload: &[u8]) {
        let counter = self.next_aps_fc();
        let aux_at = f.len();
        // data key (key id 0), extended nonce, level 5
        f.u8(0x25);
        f.u32(counter);
        f.u64(self.ext);
        let body_at = f.len();
        f.bytes(payload);
        f.bytes(&[0; crypto::MIC_LEN]);
        if !f.ok() {
            return;
        }
        let aes = Aes128::with_engine(&self.nib.tclk, H::AES);
        let nonce = nwk::nonce(self.ext, counter, 0x25);
        let (head, body) = f.as_mut_slice().split_at_mut(body_at);
        let _ = crypto::ccm_encrypt(&aes, &nonce, head, body, payload.len());
        f.set(aux_at, 0x20);
    }

    /// Hands a complete application payload to the endpoint it is for.
    pub(crate) fn aps_deliver(&mut self, src: u16, dst_ep: u8, src_ep: u8, cluster: u16, profile: u16, payload: &[u8], unicast: bool) {
        match (dst_ep, profile) {
            (0, PROFILE_ZDO) => self.on_zdo(src, cluster, payload, unicast),
            (ENDPOINT, PROFILE_HA | PROFILE_WILDCARD) => self.on_zcl(src, src_ep, ENDPOINT, cluster, payload, unicast),
            (0xFF, PROFILE_HA | PROFILE_WILDCARD) => {
                // every endpoint with the cluster; none has it: the light's answers
                let mut served = false;
                for ep in core::iter::once(ENDPOINT).chain(super::objects::ENDPOINTS) {
                    if self.clusters_of(ep).contains(&cluster) {
                        self.on_zcl(src, src_ep, ep, cluster, payload, unicast);
                        served = true;
                    }
                }
                if !served {
                    self.on_zcl(src, src_ep, ENDPOINT, cluster, payload, unicast);
                }
            }
            (ep, PROFILE_HA | PROFILE_WILDCARD) if super::objects::object(ep).is_some() => {
                self.on_zcl(src, src_ep, ep, cluster, payload, unicast)
            }
            (GP_ENDPOINT, PROFILE_GP) if cluster == CLUSTER_GP => {
                let _ = self.on_gp(src, src_ep, payload, unicast);
            }
            _ => {}
        }
    }

    /// An APS acknowledgement; only those of fragments matter (nothing else
    /// we send asks for one).
    fn on_aps_ack(&mut self, h: &nwk::Header, p: &[u8]) -> Option<()> {
        let mut r = Reader::new(p);
        let fc = r.u8()?;
        if fc & 0x80 == 0 {
            return None;
        }
        if fc & 0x10 == 0 {
            // data acknowledgement format: endpoints, cluster and profile
            r.skip(6)?;
        }
        let counter = r.u8()?;
        if r.u8()? & 3 == 0 {
            return None;
        }
        let block = r.u8()?;
        let bits = r.u8()?;
        self.on_fragment_ack(h.src, counter, block, bits);
        Some(())
    }

    /// True when this (source, counter) was seen a moment ago: a retry of a
    /// frame whose acknowledgement got lost.
    fn aps_duplicate(&mut self, src: u16, counter: u8) -> bool {
        let now = self.hal.now_ms();
        let mut slot = 0;
        let mut soonest = u32::MAX;
        for (i, e) in self.aps_dups.iter_mut().enumerate() {
            if e.used && due(now, e.expires) {
                e.used = false;
            }
            if e.used && e.src == src && e.seq == counter {
                return true;
            }
            let left = if e.used { e.expires.wrapping_sub(now) } else { 0 };
            if left < soonest {
                soonest = left;
                slot = i;
            }
        }
        self.aps_dups[slot] = Seen { used: true, src, seq: counter, expires: now.wrapping_add(DUPLICATE_MS) };
        false
    }

    /// Unicast or broadcast data from one of our endpoints.
    pub(crate) fn aps_data(&mut self, dst: u16, dst_ep: u8, cluster: u16, profile: u16, src_ep: u8, payload: &[u8]) {
        self.aps_counter = self.aps_counter.wrapping_add(1);
        // What was asked under the link key is answered under it, whatever
        // the length of the answer: security takes room, and what no longer
        // fits one frame goes in blocks like any long payload.
        let secure = self.reply_secured && dst == 0x0000;
        if payload.len() > if secure { SINGLE_MAX - SECURITY_OVERHEAD } else { SINGLE_MAX } {
            // too long for one frame: in blocks, to one node only
            if !nwk::is_broadcast(dst) {
                let a = Addressing { peer: dst, dst_ep, src_ep, cluster, profile, counter: self.aps_counter };
                self.send_fragmented(a, payload, secure);
            }
            return;
        }
        let mut f = Buf::new();
        f.u8(if nwk::is_broadcast(dst) { 0x08 } else if secure { 0x20 } else { 0x00 });
        f.u8(dst_ep);
        f.u16(cluster);
        f.u16(profile);
        f.u8(src_ep);
        f.u8(self.aps_counter);
        if secure {
            self.aps_secure(&mut f, payload);
        } else {
            f.bytes(payload);
        }
        if f.ok() {
            self.nwk_data(dst, f.as_slice());
        }
    }

    /// Data to a group.
    pub(crate) fn aps_group(&mut self, group: u16, cluster: u16, profile: u16, src_ep: u8, payload: &[u8]) {
        let mut f = Buf::new();
        f.u8(0x0C);
        f.u16(group);
        f.u16(cluster);
        f.u16(profile);
        f.u8(src_ep);
        self.aps_counter = self.aps_counter.wrapping_add(1);
        f.u8(self.aps_counter);
        f.bytes(payload);
        if f.ok() {
            self.nwk_data(APS_BROADCAST_RX_ON, f.as_slice());
        }
    }

    // ------------------------------------------------------------ commands

    fn on_aps_command(&mut self, h: &nwk::Header, p: &[u8]) -> Option<()> {
        let fc = *p.first()?;
        let rest = p.get(2..)?;
        let mut body = [0u8; 96];
        let (len, secured) = if fc & 0x20 != 0 {
            let mut a = Reader::new(rest);
            let sc = a.u8()?;
            let counter = a.u32()?;
            let ext = if sc & 0x20 != 0 { a.u64()? } else { self.nib.tc_ext };
            let aux_len = a.pos();
            let cipher = a.rest();
            let key_id = (sc >> 3) & 3;
            let sc_calc = (sc & !7) | 5;
            let mut aad = [0u8; 2 + 13];
            aad[..2].copy_from_slice(&p[..2]);
            aad[2..2 + aux_len].copy_from_slice(&rest[..aux_len]);
            aad[2] = sc_calc;
            let aad = &aad[..2 + aux_len];
            let nonce = nwk::nonce(ext, counter, sc_calc);
            let work = body.get_mut(..cipher.len())?;
            // Who may send this node a secured command (R22 4.4.1.3): in a
            // network with a Trust Center, the Trust Center alone, under the
            // link key shared with it; in a network without one, nobody
            // (there are no key updates there, 4.6.3.4). While this node is
            // joining by association it does not know yet which kind of
            // network it is (the address is 0), and its parent may be the
            // one to hand it the key.
            let tc = self.nib.tc_ext;
            let joining = self.state == State::WaitKey && tc == 0;
            if !joining && (tc == super::NO_TRUST_CENTER || ext != tc) {
                // no key of ours is authorised for this sender
                self.diag.add(k::APS_UNAUTHORIZED_KEY);
                return None;
            }
            // The key in use; while joining, the key the routers of a
            // network without a Trust Center use; the key received and not
            // yet confirmed; and while joining, the key of this device's
            // install code.
            let installed = self.identity.install_code.map(|code| crypto::install_code_key(&code));
            let links = [
                Some(self.nib.tclk),
                Some(super::DISTRIBUTED_LINK_KEY).filter(|_| joining),
                Some(self.nib.tclk_new).filter(|_| self.nib.tclk_state == 1),
                installed.filter(|_| self.state == State::WaitKey),
            ];
            let mut plain = None;
            let mut unconfirmed = false;
            for (i, link) in links.iter().enumerate() {
                let Some(link) = link else {
                    continue;
                };
                let key = aps_key(link, key_id)?;
                work.copy_from_slice(cipher);
                plain = crypto::ccm_decrypt(&Aes128::with_engine(&key, H::AES), &nonce, aad, work);
                if plain.is_some() {
                    if i == 3 {
                        // the Trust Center knows the install code: its key is the link key from here on
                        self.nib.tclk = *link;
                    }
                    unconfirmed = i == 2;
                    break;
                }
            }
            let Some(len) = plain else {
                self.stats.bad_mic = self.stats.bad_mic.wrapping_add(1);
                self.diag.add(k::APS_DECRYPT_FAILURES);
                return None;
            };
            // The incoming frame counter belongs to the link key and starts
            // at 0 with a new one (BDB 10.2.5 step 9): a frame under the key
            // just received is not held to the counter of the key in use,
            // which goes on guarding the frames still sent under that one.
            // (An association starts the count anew; a rejoin does not, so
            // a key transport recorded earlier cannot be played back to a
            // node waiting for the key again. The count is not kept through
            // a power cycle: a Trust Center that lost its own, as after a
            // coordinator is replaced, would otherwise never be heard again.)
            if !unconfirmed {
                if counter < self.tc_fc_next {
                    self.stats.replays = self.stats.replays.wrapping_add(1);
                    self.diag.add(k::APS_FC_FAILURE);
                    return None;
                }
                self.tc_fc_next = counter.wrapping_add(1);
            }
            (len, true)
        } else {
            body.get_mut(..rest.len())?.copy_from_slice(rest);
            (rest.len(), false)
        };
        let from_tc = h.src == 0x0000 && self.nib.tc_ext != super::NO_TRUST_CENTER;
        let mut r = Reader::new(&body[..len]);
        match r.u8()? {
            CMD_TRANSPORT_KEY => {
                let key_type = r.u8()?;
                // A key without APS encryption is taken in one case only: the
                // Trust Center's broadcast of the next network key to a node
                // that is on the network (R22 4.4.1.5, 4.6.3.4.1), which has
                // to name the Trust Center as its sender (NWK source 0x0000
                // here, its address in the key's source field below; 4.4.1.3).
                // Such a frame has the security of the network layer alone.
                // That shows that whoever sent or passed it on holds the
                // current network key, and nothing more: any node of the
                // network can write the Trust Center's addresses into it.
                // It is as much as the Switch Key that follows ever has, and
                // a node with the network key reads and forges all traffic
                // as it is. What must stay impossible is a key from someone
                // who is not on the network: while joining (no network
                // security yet) the key has to come under the link key. And
                // a network without a Trust Center has no key updates at all
                // (4.6.3.4).
                let broadcast_update = key_type == KEY_NETWORK
                    && self.state == State::Up
                    && h.security
                    && nwk::is_broadcast(h.dst)
                    && h.src == 0x0000
                    && self.nib.tc_ext != super::NO_TRUST_CENTER;
                if !secured && !broadcast_update {
                    return None;
                }
                match key_type {
                    KEY_NETWORK => {
                        let key = r.key()?;
                        let seq = r.u8()?;
                        let dst = r.u64()?;
                        let src = r.u64()?;
                        if dst != self.ext && dst != 0 {
                            return None;
                        }
                        if self.state == State::WaitKey {
                            // the key names the Trust Center; a node that
                            // comes back by a rejoin knows it already, and
                            // takes the key from no other (4.4.1.3)
                            if self.nib.tc_ext != 0 && src != self.nib.tc_ext {
                                return None;
                            }
                            self.nib.key = key;
                            self.nib.key_seq = seq;
                            self.nib.tc_ext = src;
                            let now = self.hal.now_ms();
                            self.network_up(now);
                        } else if self.state == State::Up && seq != self.nib.key_seq && src == self.nib.tc_ext {
                            // key update, from the Trust Center by what it
                            // says of its source too (4.4.1.3): keep it
                            // until the Switch Key, or until someone is
                            // heard using it
                            self.nib.alt_key = key;
                            self.nib.alt_seq = seq;
                            self.nib.alt_valid = true;
                            self.nib.alt_old = false;
                            // nobody has used this key yet, whatever was heard
                            // under its sequence number before (R22 4.6.3.4.2)
                            for n in self.neighbors.iter_mut().filter(|n| n.fc_seq == seq) {
                                n.fc_next = 0;
                            }
                            for c in self.children.iter_mut().filter(|c| c.fc_seq == seq) {
                                c.fc_next = 0;
                            }
                            self.store_network();
                        }
                    }
                    KEY_TC_LINK => {
                        let key = r.key()?;
                        if self.state == State::Up && matches!(self.tclk_step, 3 | 4) {
                            // the key in use again is no new key: the
                            // exchange has failed (BDB 10.2.5 step 9)
                            if key == self.nib.tclk {
                                self.tclk_refused();
                                return None;
                            }
                            self.nib.tclk_new = key;
                            self.nib.tclk_state = 1;
                            self.store_network();
                            self.tclk_advance(5);
                        }
                    }
                    _ => {}
                }
            }
            // The commands below name no sender and need no APS
            // encryption: they are the Trust Center's when they come from
            // its network address (R22 4.4.1.3), and a network without a
            // Trust Center has none of them.
            CMD_TUNNEL => {
                // only the Trust Center tunnels keys, and only inside the network's security
                let ext = r.u64()?;
                if self.state == State::Up && h.security && from_tc {
                    self.on_tunnel(ext, r.rest());
                }
            }
            CMD_REMOVE_DEVICE => {
                let ext = r.u64()?;
                if self.state == State::Up && secured && from_tc {
                    self.on_remove_device(ext);
                }
            }
            CMD_SWITCH_KEY => {
                let seq = r.u8()?;
                if self.state == State::Up && h.security && from_tc && self.nib.alt_valid && !self.nib.alt_old && self.nib.alt_seq == seq {
                    self.switch_network_key();
                }
            }
            CMD_CONFIRM_KEY => {
                let status = r.u8()?;
                let key_type = r.u8()?;
                if secured && matches!(self.tclk_step, 5 | 6) && status == 0 && key_type == KEY_TC_LINK {
                    self.nib.tclk = self.nib.tclk_new;
                    self.nib.tclk_state = 2;
                    // the new key's frame counter starts at 0 (R22 4.4.8.2.3 step 9)
                    self.tc_fc_next = 0;
                    self.store_network();
                    self.tclk_step = 0;
                    self.steering_done();
                }
            }
            _ => {}
        }
        Some(())
    }

    /// Makes the network key that was to come the active one. The key in use
    /// until now stays as the alternate: frames under it are still read
    /// (R22 4.7.3.10.6), nothing is sent under it any more.
    pub(crate) fn switch_network_key(&mut self) {
        core::mem::swap(&mut self.nib.key, &mut self.nib.alt_key);
        core::mem::swap(&mut self.nib.key_seq, &mut self.nib.alt_seq);
        self.nib.alt_old = true;
        self.key = Aes128::with_engine(&self.nib.key, H::AES);
        self.store_network();
        self.log_event(super::diag::ev::KEY_SWITCH, self.nib.key_seq as u16, 0);
    }

    /// Transport Key for a device that joined through this router on a
    /// network without a Trust Center: the routers hand out the network key
    /// themselves, encrypted with the key-transport key of the distributed
    /// link key, in an unsecured NWK frame.
    pub(crate) fn distribute_key(&mut self, short: u16, ext: u64) {
        let mut body = Buf::new();
        body.u8(CMD_TRANSPORT_KEY);
        body.u8(KEY_NETWORK);
        body.bytes(&self.nib.key);
        body.u8(self.nib.key_seq);
        body.u64(ext);
        body.u64(super::NO_TRUST_CENTER);
        let key = aps_key(&super::DISTRIBUTED_LINK_KEY, 2).unwrap_or([0; 16]);
        let counter = self.next_aps_fc();
        self.aps_counter = self.aps_counter.wrapping_add(1);
        let mut f = Buf::new();
        f.u8(0x21);
        f.u8(self.aps_counter);
        // key-transport key (key id 2), extended nonce, level 5
        let aux_at = f.len();
        f.u8(0x35);
        f.u32(counter);
        f.u64(self.ext);
        let body_at = f.len();
        f.bytes(body.as_slice());
        f.bytes(&[0; crypto::MIC_LEN]);
        let nonce = nwk::nonce(self.ext, counter, 0x35);
        let (head, tail) = f.as_mut_slice().split_at_mut(body_at);
        let _ = crypto::ccm_encrypt(&Aes128::with_engine(&key, H::AES), &nonce, head, tail, body.len());
        f.set(aux_at, 0x30);
        let seq = self.next_nwk_seq();
        let mut h = nwk::Header::new(nwk::FRAME_DATA, short, self.nib.short, 1, seq);
        h.security = false;
        self.transmit_nwk(short, &h, f.as_slice());
    }

    /// Sends an APS command to the Trust Center, encrypted with the link key
    /// when `secure`.
    pub(crate) fn aps_command(&mut self, payload: &[u8], secure: bool) {
        let mut f = Buf::new();
        self.aps_counter = self.aps_counter.wrapping_add(1);
        if secure {
            f.u8(0x21);
            f.u8(self.aps_counter);
            self.aps_secure(&mut f, payload);
            if !f.ok() {
                return;
            }
        } else {
            f.u8(0x01);
            f.u8(self.aps_counter);
            f.bytes(payload);
        }
        self.nwk_data(0x0000, f.as_slice());
    }

    pub(crate) fn aps_request_key(&mut self) {
        self.aps_command(&[CMD_REQUEST_KEY, KEY_TC_LINK], true);
    }

    /// Proves possession of the new link key: its hash, keyed with 0x03.
    pub(crate) fn aps_verify_key(&mut self) {
        let mut p = Buf::new();
        p.u8(CMD_VERIFY_KEY);
        p.u8(KEY_TC_LINK);
        p.u64(self.ext);
        p.bytes(&crypto::hmac_mmo(&self.nib.tclk_new, &[0x03]));
        self.aps_command(p.as_slice(), false);
    }
}
