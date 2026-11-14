//! Telink light-mesh on air: commands and answers between bulbs, outside the
//! connection with the app. Format from Telink's light-mesh SDK and its open
//! reimplementation (retsimx/tlsr8266_mesh):
//!
//! - data channels 2, 12, 23 and 34; the access code comes from the network
//!   name and password, the CRC starts as on advertising channels
//! - a 39-byte packet: header `82 25`, L2CAP length `21 00`, channel
//!   `03 ff`, the sender of this transmission, a zero, sequence number (3),
//!   source, destination, opcode, vendor `60 01`, ten parameter bytes, five
//!   more of bookkeeping, the hop limit, four bytes of checksum; opcode to
//!   hop limit is encrypted with the network's long-term key
//! - a receiver drops what it saw before (sequence number, source, opcode),
//!   carries out commands for it, answers a status request to its sender,
//!   and passes on what is not for it alone, with the hop limit one lower
//! - the bulb the app is connected to puts the app's commands for other
//!   bulbs on air and hands their answers to the app as notifications
//!
//! Radio time: a short listen after every advertising event (in a
//! connection only while an answer is awaited), and every packet repeated
//! on all four channels for longer than a listener's period. All of it only
//! once an app has paired the bulb into a network: in the factory network
//! ("unpaired") there is no listening and no sending, and Zigbee keeps the
//! 12 ms in every 300 that the listening costs.

use crate::crypto::{mesh_access_code, mesh_crypt, mesh_mic};
use crate::hal::{Hal, ADV_PDU_MAX, TICKS_PER_MS, TICKS_PER_US};
use crate::{reached, Ble};

pub const CHANNELS: [u8; 4] = [2, 12, 23, 34];
const LISTEN: u32 = 12 * TICKS_PER_MS;
/// Bursts per packet and the time between them: together longer than the
/// 300 ms between a listener's windows.
const REPEATS: u8 = 64;
const PERIOD: u32 = 5 * TICKS_PER_MS;
/// A burst the radio could not send yet is tried again this soon, at most
/// this many times in a row (a connection that keeps the radio's queue full
/// does not hold a train up for good).
const SKIPPED_RETRY: u32 = TICKS_PER_MS;
const SKIPPED_MAX: u8 = 100;
const HOPS: u8 = 2;
/// The radio is left alone this long before the link layer needs it.
const MARGIN: u32 = 1500 * TICKS_PER_US;
const MIN_BUDGET: u32 = 4 * TICKS_PER_MS;
/// How long answers are listened for in a connection after a request went out.
const ANSWER_WAIT: u32 = 1500 * TICKS_PER_MS;
const SLOTS: usize = 4;
const SEEN: usize = 8;

// offsets in the packet (header included)
const LEN: usize = 39;
const O_SENDER: usize = 6;
const O_NONCE: usize = 8;
const O_SN: usize = 9;
const O_SRC: usize = 12;
const O_DST: usize = 14;
const O_OP: usize = 16;
const O_VENDOR: usize = 17;
const O_PARAMS: usize = 19;
const O_HOPS: usize = 34;
const O_MIC: usize = 35;

const OP_STATUS_REQUEST: u8 = 0x1a;
const OP_STATUS_RESPONSE: u8 = 0x1b;

fn is_request(op: u8) -> bool {
    matches!(op, 0x1a | 0x1d | 0x17 | 0x2a)
}

fn is_response(op: u8) -> bool {
    matches!(op, 0x1b | 0x14 | 0x15 | 0x16 | 0x21 | 0x2b)
}

#[derive(Clone, Copy)]
struct Slot {
    pdu: [u8; LEN],
    due: u32,
    left: u8,
    /// bursts left out in a row (`SKIPPED_MAX`)
    skipped: u8,
}

pub(crate) struct Air {
    access_code: u32,
    channel: usize,
    tx: [Option<Slot>; SLOTS],
    seen: [([u8; 3], u16, u8); SEEN],
    seen_next: usize,
    /// A link layer event just ended: time for a listen window.
    pub listen_due: bool,
    answers_until: Option<u32>,
    pub stats: AirStats,
}

/// Counters for tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AirStats {
    pub bursts: u32,
    pub received: u32,
    pub rejected: u32,
    pub relayed: u32,
    pub to_app: u32,
}

impl Air {
    pub fn new() -> Air {
        Air {
            access_code: 0,
            channel: 0,
            tx: [None; SLOTS],
            seen: [([0; 3], 0, 0); SEEN],
            seen_next: 0,
            listen_due: false,
            answers_until: None,
            stats: AirStats::default(),
        }
    }

    fn seen(&self, sn: &[u8], src: u16, op: u8) -> bool {
        self.seen.iter().any(|s| s.0 == *sn && s.1 == src && s.2 == op)
    }

    fn remember(&mut self, sn: &[u8], src: u16, op: u8) {
        self.seen[self.seen_next] = ([sn[0], sn[1], sn[2]], src, op);
        self.seen_next = (self.seen_next + 1) % SEEN;
    }
}

fn u16_at(p: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([p[i], p[i + 1]])
}

fn build(sn: &[u8], src: u16, dst: u16, op: u8, params: &[u8; 10], hops: u8) -> [u8; LEN] {
    let mut p = [0u8; LEN];
    p[0] = 0x02;
    p[1] = 0x25;
    p[2] = 0x21;
    p[4..6].copy_from_slice(&[0x03, 0xff]);
    p[O_SN..O_SN + 3].copy_from_slice(&sn[..3]);
    p[O_SRC..O_SRC + 2].copy_from_slice(&src.to_le_bytes());
    p[O_DST..O_DST + 2].copy_from_slice(&dst.to_le_bytes());
    p[O_OP] = op;
    p[O_VENDOR..O_VENDOR + 2].copy_from_slice(&[0x60, 0x01]);
    p[O_PARAMS..O_PARAMS + 10].copy_from_slice(params);
    // retransmit count, informational (as Telink)
    p[O_PARAMS + 12] = 2;
    p[O_HOPS] = hops;
    p
}

fn nonce(p: &[u8; LEN]) -> [u8; 8] {
    let mut n = [0u8; 8];
    n.copy_from_slice(&p[O_NONCE..O_NONCE + 8]);
    n
}

fn encrypt(ltk: &[u8; 16], p: &mut [u8; LEN]) {
    p[0] |= 0x80;
    let n = nonce(p);
    let mic = mesh_mic(ltk, &n, &p[O_OP..O_MIC]);
    p[O_MIC..].copy_from_slice(&mic[..4]);
    mesh_crypt(ltk, &n, &mut p[O_OP..O_MIC]);
}

fn decrypt(ltk: &[u8; 16], p: &mut [u8; LEN]) -> bool {
    if p[0] & 0x80 == 0 {
        return false;
    }
    let n = nonce(p);
    mesh_crypt(ltk, &n, &mut p[O_OP..O_MIC]);
    mesh_mic(ltk, &n, &p[O_OP..O_MIC])[..4] == p[O_MIC..]
}

impl<H: Hal> Ble<H> {
    /// The network changed (start, pairing, reset): its access code on air does too.
    pub(crate) fn air_configure(&mut self) {
        self.air.access_code = mesh_access_code(self.mesh.name_padded(), self.mesh.password_padded());
    }

    pub fn air_stats(&self) -> AirStats {
        self.air.stats
    }

    /// Queues a packet (not yet encrypted) for its bursts.
    fn air_queue(&mut self, mut p: [u8; LEN]) {
        let me = self.mesh.device_address();
        p[O_SENDER..O_SENDER + 2].copy_from_slice(&me.to_le_bytes());
        let (src, op) = (u16_at(&p, O_SRC), p[O_OP] & 0x3f);
        let sn = [p[O_SN], p[O_SN + 1], p[O_SN + 2]];
        self.air.remember(&sn, src, op);
        encrypt(&self.mesh.ltk, &mut p);
        let slot = Slot { pdu: p, due: self.hal.now(), left: REPEATS, skipped: 0 };
        // with every slot taken the oldest packet gives way
        let i = self.air.tx.iter().position(|s| s.is_none()).unwrap_or(0);
        self.air.tx[i] = Some(slot);
    }

    /// The app's command for other bulbs goes on air under its sequence
    /// number, with this bulb as the source.
    pub(crate) fn air_bridge(&mut self, sn: &[u8], dst: u16, op: u8, params: &[u8; 10]) {
        let me = self.mesh.device_address();
        self.air_queue(build(sn, me, dst, op, params, HOPS));
        if is_request(op & 0x3f) {
            self.air.answers_until = Some(self.hal.now().wrapping_add(ANSWER_WAIT));
        }
    }

    fn air_received(&mut self, pdu: &[u8; ADV_PDU_MAX], len: usize) {
        let mut p = [0u8; LEN];
        p.copy_from_slice(&pdu[..LEN]);
        if len != LEN || p[1] != 0x25 || p[0] & 3 != 2 || p[2..4] != [0x21, 0x00] {
            self.air.stats.rejected += 1;
            return;
        }
        // the nodes' online status broadcasts are not kept
        if matches!(u16_at(&p, 4), 0xeeff | 0xffff) {
            return;
        }
        if !decrypt(&self.mesh.ltk, &mut p) {
            self.air.stats.rejected += 1;
            return;
        }
        if p[O_OP] & 0xc0 != 0xc0 || p[O_VENDOR..O_VENDOR + 2] != [0x60, 0x01] {
            return;
        }
        let op = p[O_OP] & 0x3f;
        let (src, dst) = (u16_at(&p, O_SRC), u16_at(&p, O_DST));
        let sn = [p[O_SN], p[O_SN + 1], p[O_SN + 2]];
        if self.air.seen(&sn, src, op) {
            return;
        }
        self.air.remember(&sn, src, op);
        self.air.stats.received += 1;

        let me = self.mesh.device_address();
        let to_everyone = dst == 0xffff;
        let to_me = dst & 0x8000 == 0 && (dst == 0 || dst == me);
        if is_response(op) {
            if dst == me {
                // an answer to what the app asked through this bulb
                self.air.stats.to_app += 1;
                let mut payload = [0u8; 13];
                payload.copy_from_slice(&p[O_OP..O_OP + 13]);
                self.mesh_notify_from_air(sn, src, &payload);
                return;
            }
        } else if to_everyone || to_me {
            let mut params = [0u8; 10];
            params.copy_from_slice(&p[O_PARAMS..O_PARAMS + 10]);
            self.mesh_command(p[O_OP], &params, true);
            if op == OP_STATUS_REQUEST {
                let status = self.mesh_status_params();
                self.air_queue(build(&sn, me, src, 0xc0 | OP_STATUS_RESPONSE, &status, HOPS));
            }
        }
        if !to_me && p[O_HOPS] > 0 {
            p[O_HOPS] -= 1;
            p[0] &= 0x7f;
            self.air.stats.relayed += 1;
            self.air_queue(p);
        }
    }

    /// Sends the bursts that are due and listens, in the time the link layer leaves.
    pub(crate) fn air_poll(&mut self) {
        let listen = core::mem::take(&mut self.air.listen_due);
        if !self.mesh.is_paired() {
            // A bulb no app has put into a mesh network yet has nobody to
            // talk to: its radio time stays with Zigbee.
            self.air.tx = [None; SLOTS];
            return;
        }
        let now = self.hal.now();
        if let Some(until) = self.air.answers_until {
            if reached(now, until) {
                self.air.answers_until = None;
            }
        }
        for i in 0..SLOTS {
            let Some(mut slot) = self.air.tx[i] else { continue };
            let now = self.hal.now();
            if !reached(now, slot.due) || self.link_idle_time(now) < MIN_BUDGET {
                continue;
            }
            // While the radio holds a packet for the app (until the next
            // connection event) a burst is left out: it is not one of the
            // bursts, or a train could lose a connection interval's worth
            // of them, 30 ms and more, and with it a listener's whole window.
            let held = self.link_connected() && !self.hal.tx_idle();
            let sent = !held && self.hal.mesh_send(&CHANNELS, self.air.access_code, &slot.pdu);
            if !sent && slot.skipped < SKIPPED_MAX {
                slot.skipped += 1;
                slot.due = self.hal.now().wrapping_add(SKIPPED_RETRY);
                self.air.tx[i] = Some(slot);
                continue;
            }
            slot.skipped = 0;
            self.air.stats.bursts += sent as u32;
            slot.left -= 1;
            // a little jitter, so that two senders do not stay in step
            let jitter = (self.hal.random() & 3) * 250 * TICKS_PER_US;
            slot.due = self.hal.now().wrapping_add(PERIOD - 500 * TICKS_PER_US + jitter);
            self.air.tx[i] = (slot.left > 0).then_some(slot);
        }
        // in a connection only while an answer is on its way
        let wanted = !self.link_connected() || self.air.answers_until.is_some();
        if !listen || !wanted {
            return;
        }
        let now = self.hal.now();
        let budget = self.link_idle_time(now);
        if budget < MIN_BUDGET {
            return;
        }
        let end = now.wrapping_add(LISTEN.min(budget - MARGIN));
        let channel = CHANNELS[self.air.channel % CHANNELS.len()];
        self.air.channel = self.air.channel.wrapping_add(1);
        loop {
            let now = self.hal.now();
            let left = end.wrapping_sub(now);
            if left as i32 <= TICKS_PER_MS as i32 / 2 {
                return;
            }
            let mut pdu = [0u8; ADV_PDU_MAX];
            match self.hal.mesh_listen(channel, self.air.access_code, left, &mut pdu) {
                Some(len) => self.air_received(&pdu, len),
                None => return,
            }
        }
    }
}
