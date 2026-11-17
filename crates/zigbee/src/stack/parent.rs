use super::{Stack, CHILDREN, INDIRECT, INDIRECT_PER_CHILD, PENDING_SHORTS};
use crate::hal::{Hal, TxStatus};
use crate::mac::{self, Addr};
use crate::nwk::{self, Header};
use crate::util::{due, Buf};

pub(crate) const CAP_FFD: u8 = 0x02;
pub(crate) const CAP_RX_ON: u8 = 0x08;

const INDIRECT_MS: u32 = 7_680;
const DEFAULT_TIMEOUT: u8 = 8;
const UNAUTHENTICATED_MS: u32 = 30_000;

const KEY_DELAY_MS: u32 = 50;

const STATUS_AT_CAPACITY: u8 = 0x01;
const STATUS_ACCESS_DENIED: u8 = 0x02;

const UPDATE_DEVICE_SECURED_REJOIN: u8 = 0x00;
const UPDATE_DEVICE_JOINED: u8 = 0x01;
const UPDATE_DEVICE_TC_REJOIN: u8 = 0x03;
const UPDATE_DEVICE_LEFT: u8 = 0x02;

#[derive(Clone, Copy)]
pub(crate) struct Child {
    pub used: bool,
    pub ext: u64,
    pub short: u16,
    pub capability: u8,
    pub joined: bool,
    pub timeout: u8,
    pub fc_next: u32,
    pub fc_seq: u8,
    pub heard: u32,
    pub polled_at: u32,
    pub poll_ms: u32,
    pub more: bool,
}

impl Child {
    pub(crate) const EMPTY: Child =
        Child {
        used: false,
        ext: 0,
        short: 0xFFFF,
        capability: 0,
        joined: false,
        timeout: DEFAULT_TIMEOUT,
        fc_next: 0,
        fc_seq: 0,
        heard: 0,
        polled_at: 0,
        poll_ms: 0,
        more: false,
    };

    pub(crate) fn sleepy(&self) -> bool {
        self.capability & CAP_RX_ON == 0
    }

    fn timeout_ms(&self) -> u32 {
        match self.timeout {
            0 => 10_000,
            n => (1u32 << n.min(14)) * 60_000,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Indirect {
    pub used: bool,
    pub ext: bool,
    pub addr: u64,
    pub frame: Buf,
    pub expires: u32,
    pub association: bool,
    pub broadcast: bool,
    pub due: u32,
}

impl Indirect {
    pub(crate) const EMPTY: Indirect =
        Indirect { used: false, ext: false, addr: 0, frame: Buf::new(), expires: 0, association: false, broadcast: false, due: 0 };
}

const _: () = assert!(CHILDREN <= 32);

const MAC_SEQ: usize = 2;
const MAC_DST: usize = 5;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Owner {
    Child(usize),
    Other(bool, u64),
    Broadcasts,
}

impl<H: Hal> Stack<H> {
    pub(crate) fn permit_open(&mut self) -> bool {
        match self.permit_until {
            Some(until) => {
                let now = self.hal.now_ms();
                if due(now, until) {
                    self.permit_until = None;
                    false
                } else {
                    true
                }
            }
            None => false,
        }
    }

    pub(crate) fn child_index(&self, short: u16) -> Option<usize> {
        self.children.iter().position(|c| c.used && c.short == short)
    }

    pub(crate) fn child_by_ext(&self, ext: u64) -> Option<usize> {
        self.children.iter().position(|c| c.used && c.ext == ext)
    }

    pub(crate) fn end_device_child(&self, short: u16) -> Option<usize> {
        self.child_index(short).filter(|&i| self.children[i].capability & CAP_FFD == 0)
    }

    pub(crate) fn has_child_room(&self) -> bool {
        self.children.iter().any(|c| !c.used)
    }

    pub(crate) fn has_end_device_room(&self) -> bool {
        self.allow_children && self.has_child_room()
    }

    pub(crate) fn new_address(&mut self) -> u16 {
        loop {
            let a = (self.hal.random() % 0xFFF7) as u16;
            let taken = a == 0
                || a == self.nib.short
                || self.child_index(a).is_some()
                || self.neighbor_index(a).is_some()
                || self.route_index(a).is_some()
                || self.addr_map.iter().any(|e| e.1 == a && e.0 != 0);
            if !taken {
                return a;
            }
        }
    }

    pub(crate) fn on_association_request(&mut self, ext: u64, capability: u8) {
        let now = self.hal.now_ms();
        let known = self.child_by_ext(ext);
        let mut slot = known.or_else(|| self.children.iter().position(|c| !c.used));
        if known.is_none() && capability & CAP_FFD == 0 && !self.allow_children {
            slot = None;
        }
        let (short, status) = match (self.permit_open(), slot) {
            (true, Some(i)) => {
                let known = if self.children[i].used {
                    Some((self.children[i].short, self.children[i].capability))
                } else {
                    self.neighbor_by_ext(ext).map(|n| (self.neighbors[n].short, CAP_FFD))
                };
                let short = match known {
                    Some((short, was)) if (was ^ capability) & CAP_FFD == 0 => short,
                    _ => self.new_address(),
                };
                self.children[i] =
                    Child { used: true, ext, short, capability, heard: now, ..Child::EMPTY };
                self.diag.add(super::diag::k::JOIN_INDICATION);
                (short, 0x00)
            }
            (true, None) => (0xFFFF, STATUS_AT_CAPACITY),
            (false, _) => (0xFFFF, STATUS_ACCESS_DENIED),
        };
        let mut f = Buf::new();
        mac::Header {
            frame_type: mac::FRAME_CMD,
            security: false,
            pending: false,
            ack_request: true,
            seq: self.next_mac_seq(),
            dst_pan: self.nib.pan,
            dst: Addr::Ext(ext),
            src_pan: self.nib.pan,
            src: Addr::Ext(self.ext),
        }
        .write(&mut f);
        f.u8(mac::CMD_ASSOC_RSP);
        f.u16(short);
        f.u8(status);
        self.indirect_put(true, ext, &f, status == 0);
    }

    fn indirect_child(&self, ext: bool, addr: u64) -> Option<usize> {
        if ext {
            self.child_by_ext(addr)
        } else {
            self.child_index(addr as u16)
        }
    }

    fn owner(&self, e: &Indirect) -> Owner {
        if e.broadcast {
            return Owner::Broadcasts;
        }
        match self.indirect_child(e.ext, e.addr) {
            Some(i) => Owner::Child(i),
            None => Owner::Other(e.ext, e.addr),
        }
    }

    fn indirect_expire(&mut self, now: u32) {
        for e in self.indirect.iter_mut() {
            if e.used && due(now, e.expires) {
                e.used = false;
                self.stats.indirect_expired = self.stats.indirect_expired.wrapping_add(1);
                self.diag.add(super::diag::k::INDIRECT_EXPIRED);
            }
        }
    }

    fn too_late(&self, owner: Owner, expires: u32) -> Option<u32> {
        let Owner::Child(c) = owner else { return None };
        let c = &self.children[c];
        let next = c.polled_at.wrapping_add(c.poll_ms);
        let late = next.wrapping_sub(expires);
        (c.poll_ms != 0 && late != 0 && late < 0x8000_0000).then_some(late)
    }

    fn indirect_slot(&mut self, owner: Owner) -> Option<usize> {
        let now = self.hal.now_ms();
        self.indirect_expire(now);
        let mut count = [0u8; INDIRECT];
        let mut oldest = [0usize; INDIRECT];
        let mut owners = [Owner::Broadcasts; INDIRECT];
        for (i, e) in self.indirect.iter().enumerate().filter(|(_, e)| e.used) {
            owners[i] = self.owner(e);
        }
        for i in 0..INDIRECT {
            oldest[i] = i;
            if !self.indirect[i].used {
                continue;
            }
            for j in 0..INDIRECT {
                if self.indirect[j].used && owners[j] == owners[i] {
                    count[i] += 1;
                    if self.indirect[j].expires.wrapping_sub(now) < self.indirect[oldest[i]].expires.wrapping_sub(now) {
                        oldest[i] = j;
                    }
                }
            }
        }
        let mine = (0..INDIRECT).find(|&i| self.indirect[i].used && owners[i] == owner);
        let slot = match mine {
            Some(i) if count[i] as usize >= INDIRECT_PER_CHILD => Some(oldest[i]),
            _ => match self.indirect.iter().position(|e| !e.used) {
                Some(free) => return Some(free),
                None => {
                    let mut latest: Option<(usize, u32)> = None;
                    for (i, e) in self.indirect.iter().enumerate() {
                        if let Some(late) = self.too_late(owners[i], e.expires) {
                            if latest.is_none_or(|(_, l)| late > l) {
                                latest = Some((i, late));
                            }
                        }
                    }
                    let own = self.too_late(owner, now.wrapping_add(INDIRECT_MS));
                    match (latest, own) {
                        (Some((i, late)), own) if own.is_none_or(|o| late > o) => Some(i),
                        (_, Some(_)) => None,
                        _ => {
                            let fullest = (0..INDIRECT).fold(0, |b, i| if count[i] > count[b] { i } else { b });
                            Some(oldest[fullest])
                        }
                    }
                }
            },
        };
        self.stats.indirect_dropped = self.stats.indirect_dropped.wrapping_add(1);
        self.diag.add(super::diag::k::INDIRECT_DROPPED);
        self.diag.add(super::diag::k::BUFFER_ALLOCATE_FAILURES);
        slot
    }

    fn indirect_put(&mut self, ext: bool, addr: u64, frame: &Buf, association: bool) {
        let owner = match self.indirect_child(ext, addr) {
            Some(i) => Owner::Child(i),
            None => Owner::Other(ext, addr),
        };
        let Some(slot) = self.indirect_slot(owner) else { return };
        let now = self.hal.now_ms();
        self.indirect[slot] = Indirect {
            used: true,
            ext,
            addr,
            frame: *frame,
            expires: now.wrapping_add(INDIRECT_MS),
            association,
            broadcast: false,
            due: 0,
        };
        self.publish_pending();
    }

    fn publish_pending(&mut self) {
        let mut shorts = [0u16; PENDING_SHORTS];
        let mut exts = [0u64; INDIRECT];
        let (mut ns, mut ne) = (0, 0);
        let mut add_short = |s: u16| {
            if ns < PENDING_SHORTS && !shorts[..ns].contains(&s) {
                shorts[ns] = s;
                ns += 1;
            }
        };
        for e in self.indirect.iter().filter(|e| e.used) {
            if e.broadcast {
                for (i, c) in self.children.iter().enumerate() {
                    if e.due & 1 << i != 0 {
                        add_short(c.short);
                    }
                }
                continue;
            }
            let (short, ext) = match self.indirect_child(e.ext, e.addr) {
                Some(i) => (Some(self.children[i].short), Some(self.children[i].ext)),
                None if e.ext => (None, Some(e.addr)),
                None => (Some(e.addr as u16), None),
            };
            if let Some(s) = short {
                add_short(s);
            }
            if let Some(x) = ext.filter(|x| ne < INDIRECT && !exts[..ne].contains(x)) {
                exts[ne] = x;
                ne += 1;
            }
        }
        self.hal.indirect_pending(&shorts[..ns], &exts[..ne]);
        let held = self.indirect.iter().filter(|e| e.used).count() as u32;
        self.diag.peak(super::diag::k::INDIRECT_PEAK, held);
    }

    pub(crate) fn on_data_request(&mut self, src: Addr) {
        let now = self.hal.now_ms();
        let child = match src {
            Addr::Short(s) => self.child_index(s),
            Addr::Ext(e) => self.child_by_ext(e),
            Addr::None => None,
        };
        if let Some(i) = child {
            let c = &mut self.children[i];
            c.heard = now;
            if c.polled_at != 0 && !c.more {
                c.poll_ms = now.wrapping_sub(c.polled_at).max(1);
            }
            c.polled_at = now;
            c.more = false;
        } else if let Addr::Short(short) = src {
            let router = self.neighbor_index(short).is_some_and(|i| self.neighbors[i].age < super::AGE_LIMIT);
            if !router {
                self.tell_to_rejoin(short, true);
            }
        }
        let matches = |e: &Indirect| -> bool {
            if !e.used {
                return false;
            }
            if e.broadcast {
                return child.is_some_and(|i| e.due & 1 << i != 0);
            }
            let direct = match src {
                Addr::Short(s) => !e.ext && e.addr == s as u64,
                Addr::Ext(x) => e.ext && e.addr == x,
                Addr::None => false,
            };
            direct
                || child.is_some_and(|i| {
                    let c = &self.children[i];
                    if e.ext {
                        e.addr == c.ext
                    } else {
                        e.addr == c.short as u64
                    }
                })
        };
        let waiting = self.indirect.iter().filter(|e| matches(e) && !due(now, e.expires)).count();
        let mut first: Option<usize> = None;
        for (i, e) in self.indirect.iter().enumerate() {
            if matches(e) && !due(now, e.expires) {
                if first.is_none_or(|f| e.expires.wrapping_sub(now) < self.indirect[f].expires.wrapping_sub(now)) {
                    first = Some(i);
                }
            }
        }
        if let Some(i) = first {
            let mut frame = self.indirect[i].frame;
            let (broadcast, association, addr) = (self.indirect[i].broadcast, self.indirect[i].association, self.indirect[i].addr);
            if waiting > 1 {
                let fc0 = frame.get(0);
                frame.set(0, fc0 | 0x10);
            }
            let to = child.filter(|_| broadcast);
            if let Some(c) = to {
                let [lo, hi] = self.children[c].short.to_le_bytes();
                frame.set(MAC_SEQ, self.next_mac_seq());
                frame.set(MAC_DST, lo);
                frame.set(MAC_DST + 1, hi);
            }
            if self.mac_transmit(frame.as_slice()) == TxStatus::Ok {
                let e = &mut self.indirect[i];
                match to {
                    Some(c) => {
                        e.due &= !(1 << c);
                        e.used = e.due != 0;
                    }
                    None => e.used = false,
                }
                if association {
                    self.update_device(addr, UPDATE_DEVICE_JOINED);
                }
                if let (Some(c), true) = (child, waiting > 1) {
                    self.children[c].more = true;
                }
            }
        }
        self.indirect_expire(now);
        self.publish_pending();
    }

    pub(crate) fn tell_to_rejoin(&mut self, short: u16, polling: bool) {
        if short >= 0xFFF8 {
            return;
        }
        let mut h = self.new_header(nwk::FRAME_CMD, short, 1);
        h.dst_ext = None;
        let leave = [nwk::CMD_LEAVE, 0x60];
        if !polling {
            self.transmit_nwk(short, &h, &leave);
            return;
        }
        let now = self.hal.now_ms();
        let waiting = |e: &Indirect| e.used && !due(now, e.expires);
        if self.indirect.iter().any(|e| waiting(e) && !e.ext && self.child_index(e.addr as u16).is_none()) {
            return;
        }
        if self.indirect.iter().all(waiting) {
            return;
        }
        let f = self.nwk_frame(short, &h, &leave);
        if f.ok() {
            self.indirect_put(false, short as u64, &f, false);
        }
    }

    fn update_device(&mut self, ext: u64, status: u8) {
        let Some(i) = self.child_by_ext(ext) else {
            return;
        };
        if self.nib.tc_ext == super::NO_TRUST_CENTER {
            if matches!(status, UPDATE_DEVICE_JOINED | UPDATE_DEVICE_TC_REJOIN) {
                let now = self.hal.now_ms();
                self.key_due = Some((now.wrapping_add(KEY_DELAY_MS), ext));
            }
            return;
        }
        let mut p = Buf::new();
        p.u8(super::aps::CMD_UPDATE_DEVICE);
        p.u64(ext);
        p.u16(self.children[i].short);
        p.u8(status);
        self.aps_command(p.as_slice(), true);
        if self.nib.tclk == super::DEFAULT_TCLK {
            self.aps_command(p.as_slice(), false);
        }
    }

    pub(crate) fn on_tunnel(&mut self, ext: u64, inner: &[u8]) {
        let Some(i) = self.child_by_ext(ext) else {
            return;
        };
        let short = self.children[i].short;
        let seq = self.next_nwk_seq();
        let mut h = Header::new(nwk::FRAME_DATA, short, self.nib.short, 1, seq);
        h.security = false;
        self.transmit_nwk(short, &h, inner);
        self.children[i].joined = true;
        self.children[i].heard = self.hal.now_ms();
        self.store_children();
    }

    pub(crate) fn on_rejoin_request(&mut self, h: &Header, mac_src: u16, p: &[u8], secured: bool) {
        let (Some(ext), Some(&capability)) = (h.src_ext, p.get(1)) else {
            return;
        };
        if !secured && self.nib.tc_ext == super::NO_TRUST_CENTER {
            self.rejoin_refuse(h, mac_src, ext, capability, secured, STATUS_ACCESS_DENIED);
            return;
        }
        let known = self.child_by_ext(ext);
        let neighbor = self.neighbor_by_ext(ext);
        if !secured {
            let kind = CAP_FFD | CAP_RX_ON;
            let differs = match (known, neighbor) {
                (Some(i), _) => self.children[i].short != h.src || (self.children[i].capability ^ capability) & kind != 0,
                (None, Some(n)) => self.neighbors[n].short != h.src || capability & CAP_FFD == 0,
                (None, None) => false,
            };
            if differs {
                self.rejoin_refuse(h, mac_src, ext, capability, secured, STATUS_ACCESS_DENIED);
                return;
            }
        }
        let room = known.or_else(|| self.children.iter().position(|c| !c.used)).filter(|_| {
            known.is_some() || capability & CAP_FFD != 0 || self.allow_children
        });
        let Some(i) = room else {
            self.rejoin_refuse(h, mac_src, ext, capability, secured, STATUS_AT_CAPACITY);
            return;
        };
        self.diag.add(super::diag::k::JOIN_INDICATION);
        let now = self.hal.now_ms();
        let taken = h.src == self.nib.short
            || h.src >= 0xFFF8
            || self.child_index(h.src).is_some_and(|c| c != i)
            || self.neighbor_index(h.src).is_some_and(|n| ![0, ext].contains(&self.neighbors[n].ext));
        let short = if taken { self.new_address() } else { h.src };
        if !self.children[i].used {
            for e in self.indirect.iter_mut().filter(|e| !e.ext && e.addr == h.src as u64) {
                e.used = false;
            }
            let (fc_next, fc_seq) = neighbor.map_or((0, 0), |n| (self.neighbors[n].fc_next, self.neighbors[n].fc_seq));
            self.children[i] =
                Child { used: true, ext, short, capability, joined: secured, timeout: DEFAULT_TIMEOUT, fc_next, fc_seq, heard: now, ..Child::EMPTY };
        } else if secured {
            let c = &mut self.children[i];
            (c.short, c.capability, c.joined, c.timeout, c.heard) = (short, capability, true, DEFAULT_TIMEOUT, now);
        }
        if secured && capability & CAP_FFD == 0 {
            if let Some(n) = neighbor {
                self.neighbors[n].used = false;
                self.diag.add(super::diag::k::NEIGHBOR_REMOVED);
            }
        }
        let mut rh = self.new_header(nwk::FRAME_CMD, h.src, 1);
        rh.dst_ext = Some(ext);
        rh.security = secured;
        let s = short.to_le_bytes();
        self.transmit_nwk(mac_src, &rh, &[nwk::CMD_REJOIN_RESPONSE, s[0], s[1], 0x00]);
        self.update_device(ext, if secured { UPDATE_DEVICE_SECURED_REJOIN } else { UPDATE_DEVICE_TC_REJOIN });
        if secured {
            self.store_children();
        }
    }

    fn rejoin_refuse(&mut self, h: &Header, mac_src: u16, ext: u64, capability: u8, secured: bool, status: u8) {
        let mut rh = self.new_header(nwk::FRAME_CMD, h.src, 1);
        rh.dst_ext = Some(ext);
        rh.security = secured;
        let s = h.src.to_le_bytes();
        let response = [nwk::CMD_REJOIN_RESPONSE, s[0], s[1], status];
        if capability & CAP_RX_ON != 0 || self.child_index(mac_src).is_some() {
            self.transmit_nwk(mac_src, &rh, &response);
            return;
        }
        let now = self.hal.now_ms();
        if self.indirect.iter().any(|e| !e.used || due(now, e.expires)) {
            let f = self.nwk_frame(mac_src, &rh, &response);
            if f.ok() {
                self.indirect_put(false, mac_src as u64, &f, false);
            }
        }
    }

    pub(crate) fn on_timeout_request(&mut self, h: &Header, p: &[u8]) {
        let (Some(&timeout), Some(&configuration)) = (p.get(1), p.get(2)) else {
            return;
        };
        let child = self.end_device_child(h.src);
        let status = match child {
            _ if timeout > 14 => 0x01,
            None => return,
            Some(_) if configuration != 0 => 0x01,
            Some(i) => {
                self.children[i].timeout = timeout;
                self.store_children();
                0x00
            }
        };
        let mut rh = self.new_header(nwk::FRAME_CMD, h.src, 1);
        if rh.dst_ext.is_none() {
            rh.dst_ext = h.src_ext;
        }
        self.transmit_nwk(h.src, &rh, &[nwk::CMD_TIMEOUT_RESPONSE, status, 0x01]);
    }

    pub(crate) fn child_left(&mut self, i: usize) {
        let ext = self.children[i].ext;
        if self.children[i].joined {
            self.update_device(ext, UPDATE_DEVICE_LEFT);
        }
        self.remove_child(i);
    }

    pub(crate) fn remove_child(&mut self, i: usize) {
        let c = self.children[i];
        if c.used {
            self.diag.add(super::diag::k::CHILD_MOVED);
        }
        self.children[i].used = false;
        for e in self.indirect.iter_mut().filter(|e| e.used) {
            if e.broadcast {
                e.due &= !(1 << i);
                e.used = e.due != 0;
            } else if (e.ext && e.addr == c.ext) || (!e.ext && e.addr == c.short as u64) {
                e.used = false;
            }
        }
        self.publish_pending();
        if c.joined {
            self.store_children();
        }
    }

    pub(crate) fn on_remove_device(&mut self, ext: u64) {
        if ext == self.ext {
            let now = self.hal.now_ms();
            self.leave_at = Some(now.wrapping_add(100));
            self.leave_rejoin = false;
        } else if let Some(i) = self.child_by_ext(ext) {
            self.dismiss_child(i, 0x00);
        }
    }

    pub(crate) fn dismiss_child(&mut self, i: usize, options: u8) {
        let c = self.children[i];
        self.remove_child(i);
        if !c.joined {
            return;
        }
        let mut h = self.new_header(nwk::FRAME_CMD, c.short, 1);
        h.dst_ext = Some(c.ext);
        let leave = [nwk::CMD_LEAVE, 0x40 | options & 0xA0];
        if c.sleepy() {
            let f = self.nwk_frame(c.short, &h, &leave);
            if f.ok() {
                self.indirect_put(false, c.short as u64, &f, false);
            }
        } else {
            self.transmit_nwk(c.short, &h, &leave);
        }
    }

    pub(crate) fn broadcast_to_sleepy(&mut self, h: &Header, payload: &[u8]) {
        if h.dst != nwk::BROADCAST_ALL {
            return;
        }
        let mut due = 0u32;
        let mut first = None;
        for (i, c) in self.children.iter().enumerate() {
            if c.used && c.joined && c.sleepy() && c.short != h.src {
                due |= 1 << i;
                first = first.or(Some(c.short));
            }
        }
        let Some(first) = first else { return };
        let frame = self.nwk_frame(first, h, payload);
        if !frame.ok() {
            return;
        }
        let Some(slot) = self.indirect_slot(Owner::Broadcasts) else { return };
        let now = self.hal.now_ms();
        self.indirect[slot] =
            Indirect { used: true, ext: false, addr: 0, frame, expires: now.wrapping_add(INDIRECT_MS), association: false, broadcast: true, due };
        self.publish_pending();
    }

    pub(crate) fn hold_for_child(&mut self, short: u16, frame: &Buf) {
        self.indirect_put(false, short as u64, frame, false);
    }

    pub(crate) fn key_timer(&mut self, now: u32) {
        let Some((at, ext)) = self.key_due else {
            return;
        };
        if !due(now, at) {
            return;
        }
        self.key_due = None;
        if let Some(i) = self.child_by_ext(ext) {
            let short = self.children[i].short;
            self.distribute_key(short, ext);
            self.children[i].joined = true;
            self.children[i].heard = now;
            self.store_children();
        }
    }

    pub(crate) fn publish_children(&mut self) {
        let mut n = 0;
        let mut same = true;
        for c in self.children.iter().filter(|c| c.used && c.capability & CAP_FFD == 0) {
            if self.polled.get(n) != Some(&c.short) {
                same = false;
                break;
            }
            n += 1;
        }
        if same && self.polled.get(n).is_none_or(|s| *s == 0xFFFF) {
            return;
        }
        let mut list = [0xFFFFu16; CHILDREN];
        let mut n = 0;
        for c in self.children.iter().filter(|c| c.used && c.capability & CAP_FFD == 0) {
            list[n] = c.short;
            n += 1;
        }
        if list != self.polled {
            self.polled = list;
            self.hal.poll_children(&list[..n]);
        }
    }

    pub(crate) fn parent_timers(&mut self, now: u32) {
        for i in 0..CHILDREN {
            let c = self.children[i];
            if !c.used {
                continue;
            }
            let silent = now.wrapping_sub(c.heard);
            if !c.joined && silent > UNAUTHENTICATED_MS {
                self.remove_child(i);
            } else if c.joined && c.capability & CAP_FFD == 0 && silent > c.timeout_ms() {
                self.remove_child(i);
            }
        }
    }
}

#[cfg(test)]
mod tests;
