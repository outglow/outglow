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
use crate::util::{due, pack, Buf, Reader};

const CONFLICT_QUIET_MS: u32 = 5_000;
const CONFLICT_JITTER_MS: u32 = 64;
const BROADCAST_MEMORY_MS: u32 = 9_000;
const OWN_SEQ_WINDOW: u8 = 64;
const BTT_KEEP_MS: u32 = 2_000;
const BROADCAST_RETRY_MS: u32 = 150;
const BROADCAST_RETRIES: u8 = 1;
const RREQ_INITIAL_RETRIES: u8 = 3;
const RREQ_RELAY_RETRIES: u8 = 2;
const RREQ_RETRY_MS: u32 = 254;
const RREQ_JITTER_MIN_MS: u32 = 2;
const RREQ_JITTER_MAX_MS: u32 = 128;
const HOLD_MS: u32 = 5_000;
const DISCOVERY_MS: u32 = 10_000;
const NEIGHBOR_FORGET: u8 = 40;
const EARLY_LS_MIN_MS: u32 = 100;
const EARLY_LS_SPREAD_MS: u32 = 400;
const EARLY_LS_GAP_MS: u32 = 1_000;
const LINK_FAILURES: u8 = 3;
const NWK_RETRIES: u8 = LINK_FAILURES - 1;
const NWK_RETRY_MIN_MS: u32 = 8;
const NWK_RETRY_SPREAD_MS: u32 = 40;

fn is_route_request(h: &Header, payload: &[u8]) -> bool {
    h.frame_type == nwk::FRAME_CMD && payload.first() == Some(&nwk::CMD_ROUTE_REQUEST)
}

fn about_of(payload: &[u8]) -> Option<u16> {
    Some(u16::from_le_bytes([*payload.get(2)?, *payload.get(3)?]))
}

impl<H: Hal> Stack<H> {
    fn trace(&mut self, kind: u8, detail: u8, h: &Header, peer: u16, aux: u32) {
        self.hal.trace(Trace { kind, detail, seq: h.seq, src: h.src, peer, aux });
    }

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
                if self.state == State::Up && payload.first().is_some_and(|b| (b >> 2) & 0x0F == 3) {
                    let _ = self.on_gpdf(h.seq, h.src, payload, lqi, end_us);
                    return;
                }
                if matches!(self.state, State::Up | State::Down | State::Scan) && payload.first().is_some_and(|b| b & 3 == 3) {
                    let _ = self.on_interpan(&h, payload, lqi);
                    return;
                }
                if !matches!(self.state, State::Up | State::WaitKey) || h.dst_pan != self.nib.pan {
                    return;
                }
                let to_us = matches!(h.dst, Addr::Short(d) if d == self.nib.short || d == mac::BROADCAST);
                if let (true, Addr::Short(src)) = (to_us, h.src) {
                    self.on_nwk(src, !broadcast, payload, lqi);
                }
            }
            _ => {}
        }
    }

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

    pub(crate) fn transmit_nwk(&mut self, mac_dst: u16, h: &Header, payload: &[u8]) -> TxStatus {
        let f = self.nwk_frame(mac_dst, h, payload);
        if !f.ok() {
            self.trace(trace::TX, trace::TX_TOO_LONG, h, mac_dst, 0);
            return TxStatus::Ok;
        }
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
        let status = match status {
            TxStatus::ChannelBusy if self.diag.last_attempts > 0 => TxStatus::NoAck,
            other => other,
        };
        if mac_dst != mac::BROADCAST {
            if let Some(i) = self.neighbor_index(mac_dst) {
                let n = &mut self.neighbors[i];
                n.failures = match status {
                    TxStatus::Ok => 0,
                    TxStatus::NoAck => n.failures.saturating_add(1),
                    TxStatus::ChannelBusy => n.failures,
                };
            }
        }
        status
    }

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
        } else if let Some(&fc) = payload.first() {
            let at = match (fc & 3, (fc >> 2) & 3) {
                (0, 0) | (0, 2) => Some((2, kind::ID_CLUSTER)),
                (0, 3) => Some((3, kind::ID_CLUSTER)),
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
        let now = self.hal.now_ms();
        if let Some(b) = self.hal.black_box() {
            b.frame(now, mac_dst, h.dst, h.src, h.seq, k, id, attempts, status);
        }
    }

    pub(crate) fn new_header(&mut self, frame_type: u8, dst: u16, radius: u8) -> Header {
        let seq = self.next_nwk_seq();
        let mut h = Header::new(frame_type, dst, self.nib.short, radius, seq);
        if frame_type == nwk::FRAME_CMD {
            h.src_ext = Some(self.ext);
            h.dst_ext = self.ext_of(dst);
        }
        h
    }

    pub(crate) fn ext_of(&self, short: u16) -> Option<u64> {
        if let Some(i) = self.child_index(short) {
            return Some(self.children[i].ext);
        }
        self.neighbor_index(short).map(|i| self.neighbors[i].ext).filter(|e| *e != 0)
    }

    pub(crate) fn nwk_send(&mut self, h: &Header, payload: &[u8]) {
        if nwk::is_broadcast(h.dst) {
            self.originate_broadcast(h, payload);
        } else if h.dst != self.nib.short {
            self.route_out(h, payload);
        }
    }

    pub(crate) fn nwk_data(&mut self, dst: u16, payload: &[u8]) {
        self.diag.add(if nwk::is_broadcast(dst) { k::APS_TX_BCAST } else { k::APS_SENT });
        let mut h = self.new_header(nwk::FRAME_DATA, dst, nwk::DEFAULT_RADIUS);
        if !nwk::is_broadcast(dst) {
            h.discover = 1;
            if let Some(i) = self.route_index(dst) {
                let r = self.routes[i];
                if r.status == ROUTE_ACTIVE && r.many_to_one && (r.record_due || r.no_cache) {
                    let rh = self.new_header(nwk::FRAME_CMD, dst, nwk::DEFAULT_RADIUS);
                    self.transmit_nwk(r.next, &rh, &[nwk::CMD_ROUTE_RECORD, 0]);
                    self.routes[i].record_due &= r.next != dst;
                }
            }
        }
        self.nwk_send(&h, payload);
    }

    fn originate_broadcast(&mut self, h: &Header, payload: &[u8]) {
        let now = self.hal.now_ms();
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
            None => {
                self.diag.add(k::BCAST_QUEUE_FULL);
                self.diag.add(k::BUFFER_ALLOCATE_FAILURES);
                self.trace(trace::BROADCAST, 2, h, mac::BROADCAST, 0);
                self.transmit_nwk(mac::BROADCAST, h, payload);
            }
        }
    }

    pub(crate) fn on_nwk(&mut self, mac_src: u16, mac_unicast: bool, p: &[u8], lqi: u8) {
        let Some((mut h, hl)) = Header::parse(p) else {
            self.diag.add(k::PACKET_VALIDATE_DROPS);
            return;
        };
        let now = self.hal.now_ms();
        let mut frame = [0u8; 127];
        let n = p.len().min(frame.len());
        frame[..n].copy_from_slice(&p[..n]);
        let (at, len) = if h.security {
            if self.state != State::Up {
                self.trace_rx(trace::RX_NOT_UP, &h, mac_src, 0);
                return;
            }
            let Some(aux) = nwk::parse_aux(p, hl) else {
                self.diag.add(k::PACKET_VALIDATE_DROPS);
                return;
            };
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
            if aux.key_seq != self.nib.key_seq && !self.nib.alt_old {
                self.switch_network_key();
            }
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
                if nwk::is_broadcast(h.dst) {
                    self.relay_heard(h.src, h.seq, mac_src);
                }
                self.trace_rx(trace::RX_ECHO, &h, mac_src, 0);
                return;
            }
            if h.src == me || mac_src == me {
                self.trace_rx(trace::RX_OWN_ADDRESS, &h, mac_src, 0);
                if nwk::is_broadcast(h.dst) && now.wrapping_sub(self.booted) > BROADCAST_MEMORY_MS {
                    self.address_conflict(me);
                }
                return;
            }
            self.neighbor_heard(mac_src, aux.ext, lqi, aux.counter, aux.key_seq);
            self.heard_at = now;
            if h.dst == me && h.dst_ext.is_some_and(|e| e != self.ext) {
                self.trace_rx(trace::RX_OWN_ADDRESS, &h, mac_src, 1);
                self.address_conflict(me);
                return;
            }
            if clash && nwk::is_broadcast(h.dst) && h.frame_type == nwk::FRAME_DATA {
                return;
            }
            let command = frame.get(hl + nwk::AUX_LEN).filter(|_| h.frame_type == nwk::FRAME_CMD && plen > 0);
            let rejoining = command == Some(&nwk::CMD_REJOIN_REQUEST);
            if rejoining && !self.rejoin_request_sound(&h, mac_src, mac_unicast) {
                return;
            }
            if h.end_device_initiator {
                h.end_device_initiator = false;
                if mac_src == h.src && !rejoining && self.end_device_child(h.src).is_none() {
                    self.trace_rx(trace::RX_NOT_A_CHILD, &h, mac_src, 0);
                    self.tell_to_rejoin(h.src, false);
                    return;
                }
            }
            if mac_src != h.src && !(nwk::is_broadcast(h.dst) && self.broadcast_seen(h.src, h.seq, now)) {
                if let Some(i) = self.end_device_child(h.src) {
                    self.remove_child(i);
                }
            }
            (hl + nwk::AUX_LEN, plen)
        } else {
            if self.state == State::Up && h.frame_type == nwk::FRAME_CMD && h.dst == self.nib.short {
                match p.get(hl) {
                    Some(&nwk::CMD_REJOIN_REQUEST) if self.rejoin_request_sound(&h, mac_src, mac_unicast) => {
                        self.on_rejoin_request(&h, mac_src, &p[hl..], false)
                    }
                    Some(&nwk::CMD_REJOIN_RESPONSE) if matches!(self.rejoin_wait, Some((false, ..))) => {
                        self.on_rejoin_response(&h, mac_src, &p[hl..])
                    }
                    _ => {}
                }
                return;
            }
            if self.state != State::WaitKey || h.dst != self.nib.short || h.frame_type != nwk::FRAME_DATA {
                self.trace_rx(trace::RX_NOT_SECURED, &h, mac_src, 0);
                return;
            }
            (hl, n - hl)
        };
        let payload = &frame[at..at + len];

        if nwk::is_broadcast(h.dst) && mac_src != h.src {
            if let Some(i) = self.neighbor_index(mac_src) {
                self.neighbors[i].relays = true;
            }
        }
        if is_route_request(&h, payload) {
            self.on_route_request(&h, mac_src, lqi, payload, now);
        } else if nwk::is_broadcast(h.dst) {
            if !matches!(h.dst, nwk::BROADCAST_ALL | nwk::BROADCAST_RX_ON | nwk::BROADCAST_ROUTERS) {
                return;
            }
            if h.frame_type == nwk::FRAME_CMD && payload.first() == Some(&nwk::CMD_LINK_STATUS) {
                if mac_src == h.src {
                    self.on_link_status(h.src, payload);
                }
                return;
            }
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

    fn own_late_copy(&self, h: &Header) -> bool {
        h.src_ext == Some(self.ext) || self.nwk_seq.wrapping_sub(h.seq) < OWN_SEQ_WINDOW
    }

    fn relay_heard(&mut self, src: u16, seq: u8, mac_src: u16) {
        if let Some(i) = self.neighbor_index(mac_src) {
            self.neighbors[i].relays = true;
        }
        self.broadcast_heard(src, seq, mac_src);
    }

    fn broadcast_heard(&mut self, src: u16, seq: u8, mac_src: u16) {
        let Some(n) = self.neighbor_index(mac_src) else {
            return;
        };
        for e in self.bcast.iter_mut().filter(|e| e.used && e.header.src == src && e.header.seq == seq) {
            e.heard |= 1 << n;
        }
    }

    fn passively_acknowledged(&self, heard: u16) -> bool {
        let relaying = |n: &Neighbor| n.used && n.relays && n.age <= AGE_LIMIT;
        self.neighbors.iter().any(relaying) && self.neighbors.iter().enumerate().all(|(i, n)| !relaying(n) || heard & (1 << i) != 0)
    }

    pub(crate) fn broadcast_seen(&mut self, src: u16, seq: u8, now: u32) -> bool {
        self.seen.iter().any(|e| e.used && e.src == src && e.seq == seq && !due(now, e.expires))
    }

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
        self.not_back_to(h.dst, mac_src);
        match command {
            Some(nwk::CMD_ROUTE_RECORD) => {
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
            Some(nwk::CMD_ROUTE_REPLY) => {
                self.trace(trace::DROP, trace::DROP_ROUTE_REPLY, &h, mac_src, 0);
                return;
            }
            Some(nwk::CMD_NETWORK_STATUS) => {
                if let (Some(&code), Some(about), true) = (payload.get(1), about_of(payload), self.end_device_child(h.dst).is_some()) {
                    if matches!(code, nwk::STATUS_TREE_LINK_FAILURE | nwk::STATUS_LINK_FAILURE) {
                        self.route_drop(about);
                    }
                }
                let next = self.next_hop(h.dst).filter(|&n| !self.link_failed(n));
                match next.map(|next| (next, self.transmit_nwk(next, &h, payload))) {
                    Some((_, TxStatus::Ok)) => self.diag.add(k::RELAYED_UCAST),
                    Some((next, TxStatus::ChannelBusy)) => self.busy_drop(&h, next, k::RELAY_DROP_MAC_FAIL),
                    Some((next, TxStatus::NoAck)) => {
                        self.link_lost(next, h.dst);
                        self.status_undelivered(&h, mac_src, payload, k::RELAY_DROP_MAC_FAIL);
                    }
                    None => self.status_undelivered(&h, mac_src, payload, k::RELAY_DROP_NO_ROUTE),
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

    fn not_back_to(&mut self, dst: u16, mac_src: u16) {
        let Some(i) = self.route_index(dst) else { return };
        if self.routes[i].status != ROUTE_ACTIVE || self.routes[i].next != mac_src {
            return;
        }
        let direct = self.neighbor_index(dst).is_some_and(|n| self.neighbors[n].age <= AGE_LIMIT) && !self.link_failed(dst);
        if direct {
            let r = &mut self.routes[i];
            r.record_due |= r.many_to_one;
            r.next = dst;
        }
    }

    fn source_routed_by(&mut self, src: u16) {
        if let Some(i) = self.route_index(src) {
            if !self.routes[i].no_cache {
                self.routes[i].record_due = false;
            }
        }
    }

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
        let me = self.nib.short.to_le_bytes();
        self.transmit_nwk(route.next, &rh, &[nwk::CMD_ROUTE_RECORD, 1, me[0], me[1]]);
    }

    pub(crate) fn route_index(&self, dst: u16) -> Option<usize> {
        self.routes.iter().position(|r| r.status != ROUTE_FREE && r.dst == dst)
    }

    fn route_drop(&mut self, dst: u16) {
        if let Some(i) = self.route_index(dst) {
            self.routes[i].status = ROUTE_FREE;
        }
    }

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

    fn link_failed(&self, short: u16) -> bool {
        self.neighbor_index(short).is_some_and(|i| self.neighbors[i].failures >= LINK_FAILURES)
    }

    fn route_set(&mut self, dst: u16, next: u16, status: u8, many_to_one: bool, no_cache: bool, now: u32) -> usize {
        let i = self.route_index(dst).or_else(|| self.routes.iter().position(|r| r.status == ROUTE_FREE)).unwrap_or_else(|| {
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

    pub(crate) fn route_out(&mut self, h: &Header, payload: &[u8]) {
        self.route_try(h, payload, NWK_RETRIES);
    }

    fn route_try(&mut self, h: &Header, payload: &[u8], retries: u8) {
        let originated = h.src == self.nib.short;
        if self.child_index(h.dst).is_some_and(|i| !self.children[i].joined) {
            self.trace(trace::DROP, trace::DROP_UNAUTHENTICATED_CHILD, h, h.dst, 0);
            return;
        }
        let data = h.frame_type == nwk::FRAME_DATA;
        let record = h.frame_type == nwk::FRAME_CMD && payload.first() == Some(&nwk::CMD_ROUTE_RECORD);
        let mut cause = k::RELAY_DROP_NO_ROUTE;
        if let Some(mut next) = self.next_hop(h.dst) {
            let many_to_one =
                self.route_index(h.dst).is_some_and(|i| self.routes[i].status == ROUTE_ACTIVE && self.routes[i].many_to_one);
            if many_to_one && self.link_failed(next) {
                if let Some(via) = self.random_router(next, h.src) {
                    next = via;
                }
            }
            let retries = if self.link_failed(next) { 0 } else { retries };
            let status = self.transmit_nwk(next, h, payload);
            if status == TxStatus::Ok {
                if !originated {
                    self.diag.add(k::RELAYED_UCAST);
                }
                return;
            }
            if retries > 0 && self.hold_retry(h, payload, ROUTED, retries) {
                return;
            }
            let lost = if retries > 0 { k::RELAY_DROP_QUEUE_FULL } else { k::RELAY_DROP_MAC_FAIL };
            if status == TxStatus::ChannelBusy {
                self.busy_drop(h, next, lost);
                return;
            }
            self.link_lost(next, h.dst);
            if record {
                if !originated {
                    self.diag.add(lost);
                }
                self.trace(trace::DROP, trace::DROP_ROUTE_RECORD, h, next, 0);
                return;
            }
            if data && many_to_one {
                if let Some(via) = self.random_router(next, h.src) {
                    let sh = self.new_header(nwk::FRAME_CMD, h.dst, nwk::DEFAULT_RADIUS);
                    let about = h.src.to_le_bytes();
                    self.transmit_nwk(via, &sh, &[nwk::CMD_NETWORK_STATUS, nwk::STATUS_MANY_TO_ONE_FAILURE, about[0], about[1]]);
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

    fn drop_routes_via(&mut self, next: u16) {
        let gone = self.neighbor_index(next).is_none();
        for r in self.routes.iter_mut() {
            if r.status == ROUTE_ACTIVE && r.next == next && (gone || !r.many_to_one) {
                r.status = ROUTE_FREE;
            }
        }
    }

    fn link_lost(&mut self, next: u16, dst: u16) {
        if let Some(i) = self.child_index(next).filter(|&i| self.children[i].capability & CAP_FFD != 0) {
            self.remove_child(i);
        }
        if self.neighbor_index(next).is_none() || self.link_failed(next) {
            self.drop_routes_via(next);
        } else if let Some(i) = self.route_index(dst) {
            let r = &mut self.routes[i];
            if r.status == ROUTE_ACTIVE && r.next == next && !r.many_to_one {
                r.status = ROUTE_FREE;
            }
        }
    }

    fn send_to(&mut self, next: u16, h: &Header, payload: &[u8], retries: u8) {
        let retries = if self.link_failed(next) { 0 } else { retries };
        let relayed = h.src != self.nib.short;
        let status = self.transmit_nwk(next, h, payload);
        if status == TxStatus::Ok {
            if relayed {
                self.diag.add(k::RELAYED_UCAST);
            }
            return;
        }
        if retries > 0 && self.hold_retry(h, payload, next, retries) {
            return;
        }
        let lost = if retries > 0 { k::RELAY_DROP_QUEUE_FULL } else { k::RELAY_DROP_MAC_FAIL };
        if status == TxStatus::ChannelBusy {
            self.busy_drop(h, next, lost);
            return;
        }
        if relayed {
            self.diag.add(lost);
        }
        self.link_lost(next, h.dst);
        let status = h.frame_type == nwk::FRAME_CMD && payload.first() == Some(&nwk::CMD_NETWORK_STATUS);
        if !status {
            self.send_network_status(h.src, nwk::STATUS_SOURCE_ROUTE_FAILURE, h.dst);
        }
    }

    fn status_undelivered(&mut self, h: &Header, mac_src: u16, payload: &[u8], drop: usize) {
        let detour = match payload.get(1) == Some(&nwk::STATUS_MANY_TO_ONE_FAILURE) {
            true => self.random_router(mac_src, mac_src),
            false => None,
        };
        match detour.map(|via| self.transmit_nwk(via, h, payload)) {
            Some(TxStatus::Ok) => self.diag.add(k::RELAYED_UCAST),
            Some(TxStatus::NoAck | TxStatus::ChannelBusy) => self.diag.add(k::RELAY_DROP_MAC_FAIL),
            None => {
                self.diag.add(drop);
                self.trace(trace::DROP, trace::DROP_UNREPORTED, h, mac_src, 0);
            }
        }
    }

    fn busy_drop(&mut self, h: &Header, next: u16, lost: usize) {
        if h.src != self.nib.short {
            self.diag.add(lost);
        } else if h.frame_type == nwk::FRAME_DATA {
            self.diag.add(k::APS_TX_UCAST_FAIL);
        }
        self.trace(trace::DROP, trace::DROP_CHANNEL_BUSY, h, next, 0);
    }

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

    fn flush_held(&mut self, dst: u16) {
        for i in 0..self.held.len() {
            if self.held[i].used && self.held[i].left == 0 && self.held[i].header.dst == dst {
                let e = self.held[i];
                self.held[i].used = false;
                self.route_try(&e.header, &e.payload[..e.len as usize], NWK_RETRIES);
            }
        }
    }

    fn retry_held(&mut self, e: &Held) {
        let payload = &e.payload[..e.len as usize];
        let retries = e.left.saturating_sub(1);
        if e.next == ROUTED {
            self.route_try(&e.header, payload, retries);
        } else {
            self.send_to(e.next, &e.header, payload, retries);
        }
    }

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

    fn discover(&mut self, dst: u16) {
        if let Some(i) = self.route_index(dst) {
            if self.routes[i].status == ROUTE_DISCOVERY {
                return;
            }
        }
        let now = self.hal.now_ms();
        self.route_set(dst, 0xFFFF, ROUTE_DISCOVERY, false, false, now);
        self.rreq_id = self.rreq_id.wrapping_add(1);
        let slot = self.discovery_slot(now, true).unwrap_or(0);
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
            let no_cache = many_to_one == 2;
            let settled = self.route_index(h.src).is_some_and(|i| {
                let r = &self.routes[i];
                r.status == ROUTE_ACTIVE && r.many_to_one && r.next == mac_src && !r.record_due
            });
            let i = self.route_set(h.src, mac_src, ROUTE_ACTIVE, true, no_cache, now);
            self.routes[i].record_due = no_cache || !settled;
        } else {
            let child = self.end_device_child(dst).filter(|&i| self.children[i].joined);
            let responder = match child {
                Some(i) => Some(self.children[i].ext),
                None if dst == self.nib.short => Some(self.ext),
                None => None,
            };
            if let Some(responder) = responder {
                self.route_learned(h.src, mac_src, now);
                let cost = self.link_cost(mac_src, lqi);
                let mut p = Buf::new();
                p.u8(nwk::CMD_ROUTE_REPLY);
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

    fn on_route_reply(&mut self, h: &Header, mac_src: u16, payload: &[u8]) {
        let mut r = Reader::new(payload);
        let (Some(_), Some(_), Some(id), Some(orig), Some(resp), Some(cost)) =
            (r.u8(), r.u8(), r.u8(), r.u16(), r.u16(), r.u8())
        else {
            return;
        };
        let now = self.hal.now_ms();
        let Some(i) = self.discoveries.iter().position(|d| d.used && d.src == orig && d.id == id && !due(now, d.expires)) else {
            return;
        };
        if orig == self.nib.short {
            if self.route_index(resp).is_none() {
                self.discoveries[i].used = false;
                return;
            }
            if self.discoveries[i].residual_cost <= cost {
                return;
            }
            self.discoveries[i].residual_cost = cost;
            self.route_learned(resp, mac_src, now);
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
        let mut p = [0u8; MAX_NWK_PAYLOAD];
        let n = payload.len().min(p.len());
        p[..n].copy_from_slice(&payload[..n]);
        p[7] = cost.saturating_add(self.link_cost(back, 0));
        let rh = self.new_header(nwk::FRAME_CMD, back, h.radius.saturating_sub(1).max(1));
        self.transmit_nwk(back, &rh, &p[..n]);
    }

    fn send_network_status(&mut self, to: u16, status: u8, about: u16) {
        let a = about.to_le_bytes();
        self.send_status(to, &[nwk::CMD_NETWORK_STATUS, status, a[0], a[1]]);
    }

    fn send_status(&mut self, to: u16, payload: &[u8]) {
        if to == self.nib.short || nwk::is_broadcast(to) {
            return;
        }
        if let Some(next) = self.next_hop(to) {
            let h = self.new_header(nwk::FRAME_CMD, to, nwk::DEFAULT_RADIUS);
            self.diag.add(k::NWK_STATUS_SENT);
            let status = payload.get(1).copied().unwrap_or(0);
            let about = u16::from_le_bytes([payload.get(2).copied().unwrap_or(0xFF), payload.get(3).copied().unwrap_or(0xFF)]);
            self.diag.last_status_tx = status;
            self.log_event(super::diag::ev::STATUS_TX, status as u16, about);
            self.transmit_nwk(next, &h, payload);
        } else {
            let about = u16::from_le_bytes([payload.get(2).copied().unwrap_or(0xFF), payload.get(3).copied().unwrap_or(0xFF)]);
            let status = payload.get(1).copied().unwrap_or(0);
            self.hal.trace(Trace { kind: trace::NWK_STATUS, detail: 1, seq: status, src: to, peer: about, aux: 0 });
        }
    }

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

    pub(crate) fn address_conflict(&mut self, short: u16) {
        let now = self.hal.now_ms();
        if self.conflict.0 == short && !due(now, self.conflict.1) {
            return;
        }
        self.log_event(super::diag::ev::CONFLICT, short, 0);
        self.address_conflict_report();
        self.conflict = (short, now.wrapping_add(CONFLICT_QUIET_MS));
        self.conflict_due = Some(now.wrapping_add(self.hal.random() % (CONFLICT_JITTER_MS + 1)));
    }

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
                    nwk::STATUS_NO_ROUTE | nwk::STATUS_TREE_LINK_FAILURE | nwk::STATUS_LINK_FAILURE => self.route_drop(about),
                    nwk::STATUS_ADDRESS_CONFLICT => {
                        match self.conflict.0 == about {
                            true => self.conflict_due = None,
                            false => self.address_conflict_report(),
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
                    if p[1] & 0x20 != 0 {
                        self.remove_child(i);
                    } else {
                        self.child_left(i);
                    }
                    return;
                }
                if p[1] & 0x40 != 0 {
                    if h.dst == self.nib.short {
                        let now = self.hal.now_ms();
                        self.leave_at = Some(now.wrapping_add(100));
                        self.leave_rejoin = p[1] & 0x20 != 0;
                    }
                } else if let Some(i) = self.neighbor_index(h.src) {
                    self.neighbors[i].used = false;
                    self.diag.add(k::NEIGHBOR_REMOVED);
                    self.drop_routes_via(h.src);
                }
            }
            Some(&nwk::CMD_NETWORK_UPDATE) => {
                let mut r = Reader::new(&p[1..]);
                if let (Some(options), Some(ext_pan), Some(update_id), Some(pan)) = (r.u8(), r.u64(), r.u8(), r.u16()) {
                    if options >> 5 == 0 && ext_pan == self.nib.ext_pan {
                        let now = self.hal.now_ms();
                        self.pan_at = Some((now.wrapping_add(BROADCAST_MEMORY_MS), pan));
                        if h.src == self.manager {
                            self.nib.update_id = update_id;
                        }
                    }
                }
            }
            Some(&id) if !(nwk::CMD_ROUTE_REQUEST..=nwk::CMD_TIMEOUT_RESPONSE).contains(&id) && h.dst == self.nib.short => {
                self.send_status(h.src, &[nwk::CMD_NETWORK_STATUS, nwk::STATUS_UNKNOWN_COMMAND, id]);
            }
            _ => {}
        }
    }

    pub(crate) fn send_leave(&mut self) {
        let h = self.new_header(nwk::FRAME_CMD, nwk::BROADCAST_RX_ON, 1);
        let options = if self.leave_rejoin { 0x20 } else { 0x00 };
        self.transmit_nwk(mac::BROADCAST, &h, &[nwk::CMD_LEAVE, options]);
    }

    pub(crate) fn neighbor_index(&self, short: u16) -> Option<usize> {
        self.neighbors.iter().position(|n| n.used && n.short == short)
    }

    pub(crate) fn neighbor_by_ext(&self, ext: u64) -> Option<usize> {
        self.neighbors.iter().position(|n| n.used && n.ext == ext)
    }

    fn neighbor_heard(&mut self, short: u16, ext: u64, lqi: u8, counter: u32, key_seq: u8) {
        let mut out_cost = 0;
        if let Some(i) = self.child_by_ext(ext) {
            self.children[i].fc_next = counter.wrapping_add(1);
            self.children[i].fc_seq = key_seq;
            let now = self.hal.now_ms();
            self.children[i].hear(now);
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
                    self.neighbors
                        .iter()
                        .enumerate()
                        .rev()
                        .max_by_key(|(_, n)| (!self.routes_through(n.short), n.age))
                        .map_or(0, |(i, _)| i)
                });
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
                        lqi,
                        out_cost,
                        age,
                        confirmed: out_cost != 0,
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

    pub(crate) fn link_proven(&mut self, short: u16) {
        if let Some(i) = self.neighbor_index(short) {
            let n = &mut self.neighbors[i];
            if n.out_cost == 0 {
                n.out_cost = nwk::cost_from_lqi(n.lqi);
            }
            n.confirmed = true;
        }
    }

    fn link_cost(&self, short: u16, lqi: u8) -> u8 {
        match self.neighbor_index(short) {
            Some(i) => self.neighbors[i].link_cost(),
            None => nwk::cost_from_lqi(lqi),
        }
    }

    pub(crate) fn age_neighbors(&mut self) {
        let now = self.hal.now_ms();
        for n in self.neighbors.iter_mut().filter(|n| n.used) {
            n.age = n.age.saturating_add(1);
            if n.age > AGE_LIMIT {
                n.out_cost = 0;
            }
            if n.age == AGE_LIMIT + 1 && n.confirmed {
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
        let mut p = Buf::new();
        p.u8(nwk::CMD_LINK_STATUS);
        p.u8(0);
        let mut count = 0u8;
        let mut after: i32 = -1;
        for _ in 0..self.neighbors.len() {
            let next = self
                .neighbors
                .iter()
                .filter(|n| n.used && n.age <= AGE_LIMIT && n.short as i32 > after)
                .min_by_key(|n| n.short);
            let Some(n) = next else {
                break;
            };
            p.u16(n.short);
            p.u8(pack(&[(nwk::cost_from_lqi(n.lqi), 0), (n.out_cost, 4)]));
            after = n.short as i32;
            count += 1;
            if let Some(i) = self.neighbor_index(after as u16) {
                self.neighbors[i].listed = true;
            }
        }
        p.set(1, pack(&[(count, 0), (0x60, 0)]));
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
        if let Some(c) = self.child_index(src) {
            if self.children[c].joined {
                self.remove_child(c);
            }
        }
        self.neighbors[i].age = 0;
        self.neighbors[i].confirmed = true;
        if options & 0x40 != 0 {
            let now = self.hal.now_ms();
            let idle = src != 0x0000
                && !self.routes_through(src)
                && now.wrapping_sub(self.neighbors[i].acked_at) >= super::ACTIVE_MS;
            self.neighbors[i].link_status_heard(now, idle);
        }
        let mut r = Reader::new(&p[2..]);
        let mut listed = false;
        for _ in 0..options & 0x1F {
            let (Some(addr), Some(status)) = (r.u16(), r.u8()) else {
                break;
            };
            if addr == self.nib.short {
                self.neighbors[i].out_cost = status & 7;
                listed = true;
            }
        }
        let complete = options & 0x60 == 0x60;
        if !listed && complete {
            self.neighbors[i].out_cost = 0;
        }
        if (complete && !listed) || (listed && !self.neighbors[i].listed) {
            self.link_status_soon();
        }
    }

    pub(crate) fn routes_through(&self, short: u16) -> bool {
        self.routes.iter().any(|r| r.status == ROUTE_ACTIVE && r.next == short)
    }

    fn link_status_soon(&mut self) {
        let now = self.hal.now_ms();
        let later = |a: u32, b: u32| if (a.wrapping_sub(b) as i32) > 0 { a } else { b };
        let soon = now.wrapping_add(EARLY_LS_MIN_MS + self.hal.random() % EARLY_LS_SPREAD_MS);
        let resumed = now.wrapping_add(RESUME_LINK_STATUS_MS.saturating_sub(now.wrapping_sub(self.booted)));
        let allowed = later(self.link_status_sent.wrapping_add(EARLY_LS_GAP_MS), resumed);
        let at = later(allowed, soon);
        if (self.link_status_at.wrapping_sub(at) as i32) > 0 {
            self.link_status_at = at;
        }
    }

    fn neighbor_announced(&mut self, h: &Header, mac_src: u16, payload: &[u8]) {
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
        if due(now, self.conflict.1) {
            self.conflict.0 = nwk::BROADCAST_ALL;
        }
        if self.pan_report_at.is_some_and(|at| due(now, at)) {
            self.pan_report_at = None;
        }
        for i in 0..self.bcast.len() {
            if self.bcast[i].used && due(now, self.bcast[i].at) {
                let e = self.bcast[i];
                let payload = &e.payload[..e.len as usize];
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
        for e in self.seen.iter_mut().chain(self.aps_dups.iter_mut()) {
            if due(now, e.expires) {
                e.used = false;
            }
        }
    }
}

#[cfg(test)]
mod tests;
