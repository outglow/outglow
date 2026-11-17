#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use ble::crypto::{crc16_modbus, mesh_crypt, mesh_mic, tl_aes_encrypt};
use ble::hal::{ConnEvent, EventResult, Hal, ADV_PDU_MAX, CONNECT_LEN, LL_PAYLOAD_MAX, TICKS_PER_MS, TICKS_PER_US};
use ble::{Ble, Identity, LinkState};
use bulb_core::command::{Command, StateReport};
use bulb_core::hal::Radio;
use bulb_core::Bulb;
use bulb_sim::{MockClock, MockFlash, MockPwm};

pub const ADDRESS: [u8; 6] = [0x33, 0x22, 0x11, 0x38, 0xc1, 0xa4];
pub const CENTRAL_ADDRESS: [u8; 6] = [0x01, 0x02, 0x03, 0x04, 0x05, 0xc6];
pub const UNIT: u64 = 1250 * TICKS_PER_US as u64;
pub const MS: u64 = TICKS_PER_MS as u64;
pub const STAGING: usize = 0x8000;
const TX_SLOTS: usize = 8;
const RX_SLOTS: usize = 8;
const EXCHANGE: u64 = 700 * TICKS_PER_US as u64;

#[derive(Debug, Clone, Copy)]
pub struct Params {
    pub access_address: u32,
    pub crc_init: u32,
    pub win_size: u8,
    pub win_offset: u16,
    pub interval: u16,
    pub latency: u16,
    pub timeout: u16,
    pub map: [u8; 5],
    pub hop: u8,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            access_address: 0x7176_4129,
            crc_init: 0x33_4455,
            win_size: 2,
            win_offset: 0,
            interval: 24,
            latency: 0,
            timeout: 500,
            map: [0xff, 0xff, 0xff, 0xff, 0x1f],
            hop: 7,
        }
    }
}

pub struct Central {
    pub connect: bool,
    pub scan: bool,
    pub params: Params,
    pub connected: bool,
    pub place_in_window: u64,
    pub ppm: i64,
    pub silent: bool,
    next_event: u64,
    pub event: u16,
    unmapped: u8,
    sn: u8,
    nesn: u8,
    txq: VecDeque<(u8, Vec<u8>)>,
    update: Option<(Params, u16)>,
    map_update: Option<([u8; 5], u16)>,
    frame: Vec<u8>,
    pub att: VecDeque<Vec<u8>>,
    pub notifications: Vec<Vec<u8>>,
    pub signaling: Vec<Vec<u8>>,
    pub smp: Vec<Vec<u8>>,
    pub control: Vec<Vec<u8>>,
    pub scan_responses: Vec<Vec<u8>>,
    pub advertisements: Vec<Vec<u8>>,
    pub served: u32,
    pub missed: u32,
    pub channels: Vec<u8>,
    pub ended: bool,
    terminate_sent: bool,
}

impl Central {
    fn new() -> Central {
        Central {
            connect: false,
            scan: false,
            params: Params::default(),
            connected: false,
            place_in_window: 0,
            ppm: 0,
            silent: false,
            next_event: 0,
            event: 0,
            unmapped: 0,
            sn: 0,
            nesn: 0,
            txq: VecDeque::new(),
            update: None,
            map_update: None,
            frame: vec![],
            att: VecDeque::new(),
            notifications: vec![],
            signaling: vec![],
            smp: vec![],
            control: vec![],
            scan_responses: vec![],
            advertisements: vec![],
            served: 0,
            missed: 0,
            channels: vec![],
            ended: false,
            terminate_sent: false,
        }
    }

    fn interval(&self) -> u64 {
        let nominal = self.params.interval as u64 * UNIT;
        (nominal as i64 + nominal as i64 * self.ppm / 1_000_000) as u64
    }

    fn channel(&mut self) -> u8 {
        self.unmapped = (self.unmapped + self.params.hop) % 37;
        let used: Vec<u8> = (0..37).filter(|c| self.params.map[(*c / 8) as usize] & (1 << (*c % 8)) != 0).collect();
        if used.contains(&self.unmapped) {
            self.unmapped
        } else {
            used[self.unmapped as usize % used.len()]
        }
    }

    fn advance(&mut self) {
        self.event = self.event.wrapping_add(1);
        self.next_event += self.interval();
        if let Some((map, instant)) = self.map_update {
            if instant == self.event {
                self.params.map = map;
                self.map_update = None;
            }
        }
        if let Some((p, instant)) = self.update {
            if instant == self.event {
                self.next_event += p.win_offset as u64 * UNIT + self.place_in_window;
                self.params.interval = p.interval;
                self.params.latency = p.latency;
                self.params.timeout = p.timeout;
                self.update = None;
            }
        }
    }

    pub fn send_control(&mut self, payload: &[u8]) {
        self.txq.push_back((3, payload.to_vec()));
    }

    pub fn send_pdu(&mut self, llid: u8, payload: &[u8]) {
        self.txq.push_back((llid, payload.to_vec()));
    }

    pub fn send_l2cap(&mut self, cid: u16, data: &[u8]) {
        let mut frame = (data.len() as u16).to_le_bytes().to_vec();
        frame.extend_from_slice(&cid.to_le_bytes());
        frame.extend_from_slice(data);
        for (i, part) in frame.chunks(LL_PAYLOAD_MAX).enumerate() {
            self.txq.push_back((if i == 0 { 2 } else { 1 }, part.to_vec()));
        }
    }

    pub fn send_fragments(&mut self, parts: &[&[u8]]) {
        for (i, part) in parts.iter().enumerate() {
            self.txq.push_back((if i == 0 { 2 } else { 1 }, part.to_vec()));
        }
    }

    pub fn send_att(&mut self, pdu: &[u8]) {
        self.send_l2cap(4, pdu);
    }

    pub fn update_connection(&mut self, win_size: u8, win_offset: u16, interval: u16, timeout: u16, lead: u16) {
        let instant = self.event.wrapping_add(lead);
        let mut p = self.params;
        p.win_size = win_size;
        p.win_offset = win_offset;
        p.interval = interval;
        p.timeout = timeout;
        self.update = Some((p, instant));
        let mut c = vec![0x00, win_size];
        c.extend_from_slice(&win_offset.to_le_bytes());
        c.extend_from_slice(&interval.to_le_bytes());
        c.extend_from_slice(&0u16.to_le_bytes());
        c.extend_from_slice(&timeout.to_le_bytes());
        c.extend_from_slice(&instant.to_le_bytes());
        self.send_control(&c);
    }

    pub fn update_channel_map(&mut self, map: [u8; 5], lead: u16) {
        let instant = self.event.wrapping_add(lead);
        self.map_update = Some((map, instant));
        let mut c = vec![0x01];
        c.extend_from_slice(&map);
        c.extend_from_slice(&instant.to_le_bytes());
        self.send_control(&c);
    }

    pub fn terminate(&mut self) {
        self.send_control(&[0x02, 0x13]);
        self.terminate_sent = true;
    }

    fn received(&mut self, llid: u8, payload: &[u8]) {
        match llid {
            3 => {
                if payload.first() == Some(&0x02) {
                    self.ended = true;
                }
                self.control.push(payload.to_vec());
            }
            2 => {
                self.frame = payload.to_vec();
                self.frame_check();
            }
            1 if !payload.is_empty() => {
                self.frame.extend_from_slice(payload);
                self.frame_check();
            }
            _ => {}
        }
    }

    fn frame_check(&mut self) {
        if self.frame.len() < 4 {
            return;
        }
        let len = u16::from_le_bytes([self.frame[0], self.frame[1]]) as usize;
        if self.frame.len() < 4 + len {
            return;
        }
        let cid = u16::from_le_bytes([self.frame[2], self.frame[3]]);
        let data = self.frame[4..4 + len].to_vec();
        self.frame.clear();
        match cid {
            4 if data[0] == 0x1b => self.notifications.push(data),
            4 => self.att.push_back(data),
            5 => self.signaling.push(data),
            6 => self.smp.push(data),
            _ => {}
        }
    }
}

#[derive(Default)]
pub struct Ether {
    sends: Vec<(u64, u8, u32, Vec<u8>, usize, Vec<usize>)>,
    windows: Vec<(usize, u8, u32, u64, u64)>,
    inbox: Vec<VecDeque<Vec<u8>>>,
    pub blocked: Vec<(usize, usize)>,
}

impl Ether {
    pub fn sent_by(&self, node: usize, pdu_filter: impl Fn(&[u8]) -> bool) -> usize {
        self.sends.iter().filter(|s| s.4 == node && pdu_filter(&s.3)).count()
    }

    pub fn sent(&self, node: usize) -> Vec<Vec<u8>> {
        self.sends.iter().filter(|s| s.4 == node).map(|s| s.3.clone()).collect()
    }

    pub fn deliver(&mut self, node: usize, pdu: &[u8]) {
        self.inbox[node].push_back(pdu.to_vec());
    }

    pub fn bursts_of(&self, node: usize) -> Vec<u64> {
        let first = self.sends.iter().find(|s| s.4 == node).map(|s| s.1);
        self.sends.iter().filter(|s| s.4 == node && Some(s.1) == first).map(|s| s.0).collect()
    }

    fn hears(&self, a: usize, b: usize) -> bool {
        a != b && !self.blocked.contains(&(a, b)) && !self.blocked.contains(&(b, a))
    }
}

pub struct World {
    pub address: [u8; 6],
    pub ether: Option<Rc<RefCell<Ether>>>,
    pub node: usize,
    pub now: u64,
    pub central: Central,
    sn: u8,
    nesn: u8,
    in_flight: bool,
    txq: VecDeque<(u8, Vec<u8>)>,
    rxq: VecDeque<(u8, Vec<u8>)>,
    pub events: Vec<(u8, bool)>,
    pub nv: Option<Vec<u8>>,
    pub nv_writes: u32,
    pub staging: Vec<u8>,
    pub erased_sectors: Vec<u32>,
    pub activated: bool,
    pub mesh_listens: u32,
    pub listened: Vec<(u64, u64, u8)>,
    pub event_starts: Vec<u64>,
    pub flash_fault: bool,
    pub zigbee_busy: Vec<(u64, u64, bool)>,
    pub advertised_at: Vec<u64>,
    capture: Vec<(u64, u8, u32, u8, Vec<u8>)>,
    rng: u32,
}

impl World {
    fn pass_events(&mut self, until: u64) {
        let c = &mut self.central;
        while c.connected && c.next_event < until {
            if !c.silent {
                c.missed += 1;
            }
            c.channel();
            c.advance();
        }
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let Ok(dir) = std::env::var("BLE_PCAP_DIR") else {
            return;
        };
        if self.capture.is_empty() {
            return;
        }
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("ble").replace("::", ".");
        let mut out = Vec::new();
        out.extend_from_slice(&0xa1b2_c3d4u32.to_le_bytes());
        out.extend_from_slice(&[2, 0, 4, 0]);
        out.extend_from_slice(&[0; 8]);
        out.extend_from_slice(&65535u32.to_le_bytes());
        out.extend_from_slice(&256u32.to_le_bytes());
        for (at, channel, access, from, pdu) in &self.capture {
            let us = at / TICKS_PER_US as u64;
            let rf = match channel {
                37 => 0,
                38 => 12,
                39 => 39,
                c if *c <= 10 => c + 1,
                c => c + 2,
            };
            let flags: u16 = 0x0001 | 0x0010 | (*from as u16) << 7 | 0x0c00;
            let len = 10 + 4 + pdu.len() + 3;
            out.extend_from_slice(&((us / 1_000_000) as u32).to_le_bytes());
            out.extend_from_slice(&((us % 1_000_000) as u32).to_le_bytes());
            out.extend_from_slice(&(len as u32).to_le_bytes());
            out.extend_from_slice(&(len as u32).to_le_bytes());
            out.extend_from_slice(&[rf, 0, 0, 0]);
            out.extend_from_slice(&access.to_le_bytes());
            out.extend_from_slice(&flags.to_le_bytes());
            out.extend_from_slice(&access.to_le_bytes());
            out.extend_from_slice(pdu);
            out.extend_from_slice(&[0, 0, 0]);
        }
        let mut n = 0;
        loop {
            let path = if n == 0 { format!("{dir}/{name}.pcap") } else { format!("{dir}/{name}.{n}.pcap") };
            if !std::path::Path::new(&path).exists() {
                let _ = std::fs::write(path, &out);
                return;
            }
            n += 1;
        }
    }
}

#[derive(Clone)]
pub struct SimHal(pub Rc<RefCell<World>>);

impl Hal for SimHal {
    fn now(&mut self) -> u32 {
        self.0.borrow().now as u32
    }

    fn random(&mut self) -> u32 {
        let mut w = self.0.borrow_mut();
        w.rng = w.rng.wrapping_mul(1_103_515_245).wrapping_add(12345);
        w.rng >> 4
    }

    fn address(&mut self) -> [u8; 6] {
        self.0.borrow().address
    }

    fn radio_busy(&mut self) -> bool {
        let w = self.0.borrow();
        w.zigbee_busy.iter().any(|&(from, to, _)| (from..to).contains(&w.now))
    }

    fn neighbours_talking(&mut self) -> bool {
        let w = self.0.borrow();
        w.zigbee_busy.iter().any(|&(from, to, n)| n && (from..to).contains(&w.now))
    }

    fn advertise(&mut self, adv: &[u8], scan_rsp: &[u8], connect: &mut [u8; CONNECT_LEN]) -> Option<u32> {
        let mut w = self.0.borrow_mut();
        assert!(adv.len() <= ADV_PDU_MAX && adv[1] as usize == adv.len() - 2, "advertising PDU length field");
        w.central.advertisements.push(adv.to_vec());
        let now = w.now;
        w.advertised_at.push(now);
        w.capture.push((now, 37, 0x8e89_bed6, 0, adv.to_vec()));
        if w.central.scan {
            assert!(scan_rsp[1] as usize == scan_rsp.len() - 2, "scan response length field");
            w.central.scan_responses.push(scan_rsp.to_vec());
            let mut req = vec![0x03, 12];
            req.extend_from_slice(&CENTRAL_ADDRESS);
            req.extend_from_slice(&adv[2..8]);
            w.capture.push((now + MS / 2, 37, 0x8e89_bed6, 0, req));
            w.capture.push((now + MS, 37, 0x8e89_bed6, 0, scan_rsp.to_vec()));
        }
        if !w.central.connect || w.central.connected {
            w.now += 3 * MS;
            return None;
        }
        w.now += MS;
        let end = w.now;
        let p = w.central.params;
        connect[..6].copy_from_slice(&CENTRAL_ADDRESS);
        connect[6..12].copy_from_slice(&adv[2..8]);
        let d = &mut connect[12..];
        d[..4].copy_from_slice(&p.access_address.to_le_bytes());
        d[4..7].copy_from_slice(&p.crc_init.to_le_bytes()[..3]);
        d[7] = p.win_size;
        d[8..10].copy_from_slice(&p.win_offset.to_le_bytes());
        d[10..12].copy_from_slice(&p.interval.to_le_bytes());
        d[12..14].copy_from_slice(&p.latency.to_le_bytes());
        d[14..16].copy_from_slice(&p.timeout.to_le_bytes());
        d[16..21].copy_from_slice(&p.map);
        d[21] = p.hop | 1 << 5;
        let mut ind = vec![0x05, CONNECT_LEN as u8];
        ind.extend_from_slice(&connect[..]);
        w.capture.push((end, 37, 0x8e89_bed6, 0, ind));
        let c = &mut w.central;
        c.connect = false;
        c.connected = true;
        c.ended = false;
        c.terminate_sent = false;
        c.next_event = end + UNIT + p.win_offset as u64 * UNIT + c.place_in_window;
        c.event = 0;
        c.unmapped = 0;
        c.sn = 0;
        c.nesn = 0;
        c.txq.clear();
        c.frame.clear();
        c.update = None;
        c.map_update = None;
        Some(end as u32)
    }

    fn connection_reset(&mut self) {
        let mut w = self.0.borrow_mut();
        w.sn = 0;
        w.nesn = 0;
        w.in_flight = false;
        w.txq.clear();
        w.rxq.clear();
    }

    fn connection_event(&mut self, ev: &ConnEvent) -> EventResult {
        let mut w = self.0.borrow_mut();
        let w = &mut *w;
        let start = w.now + ev.start.wrapping_sub(w.now as u32) as i32 as i64 as u64;
        assert!(start + ev.window as u64 >= w.now, "event that is already over");
        w.event_starts.push(start);
        let room = ev.deadline.wrapping_sub(ev.start.wrapping_add(ev.window));
        assert!((16_000..=64_000).contains(&room), "1 to 4 ms for the event after its window: {room}");
        w.now = w.now.max(start);
        w.pass_events(w.now);
        let at = w.central.next_event;
        let heard = w.central.connected && !w.central.silent && at <= start + ev.window as u64;
        if !heard {
            w.now = start + ev.window as u64;
            w.pass_events(w.now);
            w.events.push((ev.channel, false));
            return EventResult { anchor: None };
        }
        let channel = w.central.channel();
        if channel != ev.channel || ev.access_address != w.central.params.access_address || ev.crc_init != w.central.params.crc_init {
            w.central.missed += 1;
            w.central.advance();
            w.now = start + ev.window as u64;
            w.events.push((ev.channel, false));
            return EventResult { anchor: None };
        }
        w.now = at;
        for _ in 0..8 {
            let c = &mut w.central;
            let (llid, payload) = c.txq.front().cloned().unwrap_or((1, vec![]));
            let central_more = c.txq.len() > 1;
            let (c_sn, c_nesn) = (c.sn, c.nesn);
            let mut on_air = vec![llid | c_nesn << 2 | c_sn << 3 | (central_more as u8) << 4, payload.len() as u8];
            on_air.extend_from_slice(&payload);
            w.capture.push((w.now, channel, ev.access_address, 2, on_air));
            if c_nesn != w.sn {
                w.sn ^= 1;
                if w.in_flight {
                    w.txq.pop_front();
                    w.in_flight = false;
                }
            }
            if c_sn == w.nesn && w.rxq.len() < RX_SLOTS {
                w.nesn ^= 1;
                if !payload.is_empty() {
                    w.rxq.push_back((llid, payload));
                }
            }
            let (llid, payload) = w.txq.front().cloned().unwrap_or((1, vec![]));
            w.in_flight = !w.txq.is_empty();
            let more = w.txq.len() > 1;
            let mut on_air = vec![llid | w.nesn << 2 | w.sn << 3 | (more as u8) << 4, payload.len() as u8];
            on_air.extend_from_slice(&payload);
            w.capture.push((w.now + EXCHANGE / 2, channel, ev.access_address, 3, on_air));
            let c = &mut w.central;
            if w.nesn != c.sn {
                c.sn ^= 1;
                if let Some((3, p)) = c.txq.pop_front() {
                    if p[0] == 0x02 {
                        c.ended = true;
                    }
                }
            }
            if w.sn == c.nesn {
                c.nesn ^= 1;
                c.received(llid, &payload);
                if llid == 3 && payload[0] == 0x02 {
                    w.sn ^= 1;
                    w.txq.pop_front();
                    w.in_flight = false;
                }
            }
            w.now += EXCHANGE;
            if !(more || central_more) || c.ended {
                break;
            }
        }
        let c = &mut w.central;
        c.served += 1;
        c.channels.push(channel);
        c.advance();
        if c.ended {
            c.connected = false;
        }
        w.events.push((ev.channel, true));
        EventResult { anchor: Some(at as u32) }
    }

    fn tx_free(&mut self) -> usize {
        TX_SLOTS - self.0.borrow().txq.len()
    }

    fn tx_idle(&mut self) -> bool {
        self.0.borrow().txq.is_empty()
    }

    fn tx_push(&mut self, llid: u8, payload: &[u8]) -> bool {
        let mut w = self.0.borrow_mut();
        assert!(payload.len() <= LL_PAYLOAD_MAX && (1..=3).contains(&llid));
        if w.txq.len() >= TX_SLOTS {
            return false;
        }
        w.txq.push_back((llid, payload.to_vec()));
        true
    }

    fn rx_pop(&mut self, payload: &mut [u8; LL_PAYLOAD_MAX]) -> Option<(u8, usize)> {
        let (llid, p) = self.0.borrow_mut().rxq.pop_front()?;
        payload[..p.len()].copy_from_slice(&p);
        Some((llid, p.len()))
    }

    fn nv_load(&mut self, buf: &mut [u8]) -> Option<usize> {
        let w = self.0.borrow();
        let v = w.nv.as_ref()?;
        let n = v.len().min(buf.len());
        buf[..n].copy_from_slice(&v[..n]);
        Some(n)
    }

    fn nv_store(&mut self, data: &[u8]) {
        let mut w = self.0.borrow_mut();
        w.nv = Some(data.to_vec());
        w.nv_writes += 1;
    }

    fn ota_capacity(&mut self) -> u32 {
        STAGING as u32
    }

    fn ota_erase(&mut self, offset: u32) {
        let mut w = self.0.borrow_mut();
        assert!(offset % 0x1000 == 0 && (offset as usize) < STAGING);
        w.staging[offset as usize..offset as usize + 0x1000].fill(0xff);
        w.erased_sectors.push(offset);
        w.now += 40 * MS;
    }

    fn ota_write(&mut self, offset: u32, data: &[u8]) {
        let mut w = self.0.borrow_mut();
        for (i, b) in data.iter().enumerate() {
            w.staging[offset as usize + i] &= *b;
        }
    }

    fn ota_activate(&mut self) -> bool {
        let mut w = self.0.borrow_mut();
        if w.flash_fault {
            return false;
        }
        w.staging[8] &= 0x4b;
        w.activated = true;
        true
    }

    fn mesh_send(&mut self, channels: &[u8], access_code: u32, pdu: &[u8]) -> bool {
        let mut w = self.0.borrow_mut();
        if w.central.connected && !w.txq.is_empty() {
            w.now += 100 * TICKS_PER_US as u64;
            return false;
        }
        let Some(ether) = w.ether.clone() else {
            w.now += 2 * MS;
            return true;
        };
        let mut e = ether.borrow_mut();
        for ch in channels {
            let t = w.now;
            let mut heard = vec![];
            for (node, wch, wac, t0, t1) in e.windows.clone() {
                if wch == *ch && wac == access_code && (t0..=t1).contains(&t) && e.hears(w.node, node) && !heard.contains(&node) {
                    heard.push(node);
                    e.inbox[node].push_back(pdu.to_vec());
                }
            }
            e.sends.push((t, *ch, access_code, pdu.to_vec(), w.node, heard));
            w.now += 500 * TICKS_PER_US as u64;
        }
        let keep = e.sends.len().saturating_sub(2000);
        e.sends.drain(..keep);
        true
    }

    fn mesh_listen(&mut self, channel: u8, access_code: u32, ticks: u32, pdu: &mut [u8; ADV_PDU_MAX]) -> Option<usize> {
        let mut w = self.0.borrow_mut();
        w.mesh_listens += 1;
        let at = w.now;
        w.listened.push((at, at + ticks as u64, channel));
        let Some(ether) = w.ether.clone() else {
            w.now += ticks as u64;
            return None;
        };
        let mut e = ether.borrow_mut();
        let (me, t0, t1) = (w.node, w.now, w.now + ticks as u64);
        let late = e.inbox[me].pop_front();
        let live = e.sends.iter().position(|(t, ch, ac, _, from, heard)| {
            *ch == channel && *ac == access_code && (t0..=t1).contains(t) && e.hears(me, *from) && !heard.contains(&me)
        });
        let (t, packet) = match (late, live) {
            (Some(p), _) => (t0, p),
            (None, Some(i)) => {
                e.sends[i].5.push(me);
                (e.sends[i].0, e.sends[i].3.clone())
            }
            (None, None) => {
                e.windows.push((me, channel, access_code, t0, t1));
                let keep = e.windows.len().saturating_sub(64);
                e.windows.drain(..keep);
                w.now = t1;
                return None;
            }
        };
        w.now = t + 400 * TICKS_PER_US as u64;
        pdu[..packet.len()].copy_from_slice(&packet);
        Some(packet.len())
    }
}

pub struct BleLink(pub Rc<RefCell<Ble<SimHal>>>);

impl Radio for BleLink {
    fn poll(&mut self) -> Option<Command> {
        self.0.borrow_mut().poll()
    }

    fn report(&mut self, state: &StateReport) {
        self.0.borrow_mut().set_light(state);
    }
}

pub const DIMMABLE: Identity =
    Identity { product: 0x20a7, model: "ESMLFzm_w6_Dimm", firmware_revision: "3.0.2_1610", hardware_revision: "4.63", tunable: false, ct_min: bulb_core::CT_MIN, ct_max: bulb_core::CT_MAX, colour: false };
pub const TUNABLE: Identity =
    Identity { product: 0x20a6, model: "ESMLFzm_w6_TW", firmware_revision: "3.0.2_1610", hardware_revision: "4.63", tunable: true, ct_min: bulb_core::CT_MIN, ct_max: bulb_core::CT_MAX, colour: false };
pub const COLOUR: Identity =
    Identity { product: 0x2096, model: "EGLO_ZM_RGB_TW", firmware_revision: "3.0.2_1610", hardware_revision: "4.60", tunable: true, ct_min: 153, ct_max: 370, colour: true };

pub struct Rig {
    pub world: Rc<RefCell<World>>,
    pub ble: Rc<RefCell<Ble<SimHal>>>,
    pub bulb: Bulb<MockPwm, MockClock, MockFlash, BleLink>,
    clock: MockClock,
    clock_ms: u64,
    identity: Identity,
    pub key: Option<[u8; 16]>,
    sn: u32,
}

impl Rig {
    pub fn new(identity: Identity) -> Rig {
        Rig::boot(Rig::world(ADDRESS, None, 0), identity)
    }

    pub fn in_ether(ether: &Rc<RefCell<Ether>>, address: [u8; 6], identity: Identity) -> Rig {
        let node = ether.borrow().inbox.len();
        ether.borrow_mut().inbox.push(VecDeque::new());
        Rig::boot(Rig::world(address, Some(ether.clone()), node), identity)
    }

    pub fn world(address: [u8; 6], ether: Option<Rc<RefCell<Ether>>>, node: usize) -> Rc<RefCell<World>> {
        Rc::new(RefCell::new(World {
            address,
            ether,
            node,
            now: 5 * MS,
            central: Central::new(),
            sn: 0,
            nesn: 0,
            in_flight: false,
            txq: VecDeque::new(),
            rxq: VecDeque::new(),
            events: vec![],
            nv: None,
            nv_writes: 0,
            staging: vec![0xa5; STAGING],
            erased_sectors: vec![],
            activated: false,
            flash_fault: false,
            zigbee_busy: vec![],
            advertised_at: vec![],
            mesh_listens: 0,
            listened: vec![],
            event_starts: vec![],
            capture: vec![],
            rng: 0x1234_5678 ^ u32::from_le_bytes([address[0], address[1], address[2], address[3]]),
        }))
    }

    pub fn boot(world: Rc<RefCell<World>>, identity: Identity) -> Rig {
        let ble = Rc::new(RefCell::new(Ble::new(SimHal(world.clone()), identity)));
        let clock = MockClock::new();
        let pwm = if identity.colour {
            MockPwm::colour(1000)
        } else if identity.tunable {
            MockPwm::tunable(1000)
        } else {
            MockPwm::new(1000)
        };
        let mut bulb = Bulb::boot(pwm, clock.handle(), MockFlash::new(), BleLink(ble.clone()));
        bulb.set_default_transition(0);
        let clock_ms = world.borrow().now / MS;
        Rig { world, ble, bulb, clock, clock_ms, identity, key: None, sn: 0x100 }
    }

    pub fn restart(self) -> Rig {
        let Rig { world, identity, .. } = self;
        world.borrow_mut().central.connected = false;
        Rig::boot(world, identity)
    }

    pub fn run(&mut self, ms: u64) {
        let end = self.world.borrow().now + ms * MS;
        self.run_until(end);
    }

    pub fn run_until(&mut self, end: u64) {
        while self.world.borrow().now < end {
            self.bulb.tick();
            self.world.borrow_mut().now += 100 * TICKS_PER_US as u64;
            let now_ms = self.world.borrow().now / MS;
            self.clock.advance(now_ms - self.clock_ms);
            self.clock_ms = now_ms;
        }
    }

    pub fn stall(&mut self, ms: u64) {
        self.world.borrow_mut().now += ms * MS;
    }

    pub fn central(&self) -> std::cell::RefMut<'_, Central> {
        std::cell::RefMut::map(self.world.borrow_mut(), |w| &mut w.central)
    }

    pub fn state(&self) -> LinkState {
        self.ble.borrow().link_state()
    }

    pub fn connect(&mut self) {
        self.central().connect = true;
        self.run(400);
        assert_eq!(self.state(), LinkState::Connected, "the bulb accepted the connection");
        self.run(200);
        assert!(self.central().served > 0, "the bulb answered in the connection");
    }

    pub fn att(&mut self, request: &[u8]) -> Vec<u8> {
        self.central().att.clear();
        self.central().send_att(request);
        for _ in 0..40 {
            self.run(50);
            if let Some(rsp) = self.central().att.pop_front() {
                return rsp;
            }
        }
        panic!("no response to {request:02x?}");
    }

    pub fn write(&mut self, handle: u16, value: &[u8]) {
        let mut req = vec![0x12];
        req.extend_from_slice(&handle.to_le_bytes());
        req.extend_from_slice(value);
        assert_eq!(self.att(&req), [0x13], "write to {handle:#x} accepted");
    }

    pub fn read(&mut self, handle: u16) -> Vec<u8> {
        let mut req = vec![0x0a];
        req.extend_from_slice(&handle.to_le_bytes());
        let rsp = self.att(&req);
        assert_eq!(rsp[0], 0x0b, "read of {handle:#x}: {rsp:02x?}");
        rsp[1..].to_vec()
    }

    pub fn secure_password() -> [u8; 16] {
        [0x82, 0xbf, 0x5e, 0x36, 0x56, 0xb2, 0xc0, 0xa6, 0xe4, 0x6f, 0xa4, 0x3c, 0xaa, 0x90, 0x4a, 0xcd]
    }

    pub fn login(&mut self, name: &[u8], password: &[u8]) -> bool {
        let pad = |s: &[u8]| {
            let mut b = [0u8; 16];
            b[..s.len()].copy_from_slice(s);
            b
        };
        let (name, password) = (pad(name), pad(password));
        let block: [u8; 16] = std::array::from_fn(|i| name[i] ^ password[i]);
        let random = [1, 2, 3, 4, 5, 6, 7, 8];
        let proof = tl_aes_encrypt(&pad(&random), &block);
        let mut packet = vec![0x0c];
        packet.extend_from_slice(&random);
        packet.extend_from_slice(&proof[..8]);
        self.write(0x1e, &packet);
        let rsp = self.read(0x1e);
        if rsp[0] != 0x0d {
            assert_eq!(rsp[0], 0x0e, "a refused login reads 0x0e");
            self.key = None;
            return false;
        }
        let mut both = [0u8; 16];
        both[..8].copy_from_slice(&random);
        both[8..].copy_from_slice(&rsp[1..9]);
        self.key = Some(tl_aes_encrypt(&block, &both));
        true
    }

    pub fn enable_status(&mut self) {
        self.write(0x13, &[1, 0]);
        self.write(0x12, &[1]);
    }

    pub fn command_packet(&mut self, dst: u16, op: u8, params: &[u8]) -> Vec<u8> {
        self.sn += 1;
        let key = self.key.expect("logged in");
        let sn = &self.sn.to_le_bytes()[..3];
        let mut nonce = [0u8; 8];
        nonce[..4].copy_from_slice(&self.world.borrow().address[..4]);
        nonce[4] = 1;
        nonce[5..].copy_from_slice(sn);
        let mut payload = [0u8; 15];
        payload[..2].copy_from_slice(&dst.to_le_bytes());
        payload[2] = op;
        payload[3..5].copy_from_slice(&[0x60, 0x01]);
        payload[5..5 + params.len()].copy_from_slice(params);
        let mic = mesh_mic(&key, &nonce, &payload);
        mesh_crypt(&key, &nonce, &mut payload);
        let mut packet = sn.to_vec();
        packet.extend_from_slice(&mic[..2]);
        packet.extend_from_slice(&payload);
        packet
    }

    pub fn command(&mut self, op: u8, params: &[u8]) {
        let packet = self.command_packet(0xffff, op, params);
        self.write(0x15, &packet);
        self.run(600);
    }

    pub fn statuses(&mut self) -> Vec<(u8, Vec<u8>)> {
        self.statuses_from().into_iter().map(|(op, params, _)| (op, params)).collect()
    }

    pub fn app_statuses(&mut self) -> (Vec<[u8; 19]>, usize) {
        let key = self.key.expect("logged in");
        let raw: Vec<Vec<u8>> = self.central().notifications.drain(..).collect();
        let mut out = Vec::new();
        let mut other = 0;
        for n in &raw {
            assert_eq!(n[..3], [0x1b, 0x12, 0x00], "notification of the status characteristic");
            let v = &n[3..];
            if v.len() == 20 && v[0] == 0x80 {
                let mut block = [0u8; 16];
                block.copy_from_slice(&v[1..17]);
                let mut d = [0u8; 19];
                d[..16].copy_from_slice(&ble::crypto::tl_aes_decrypt(&key, &block));
                d[16..].copy_from_slice(&v[17..20]);
                out.push(d);
            } else {
                other += 1;
            }
        }
        (out, other)
    }

    pub fn statuses_from(&mut self) -> Vec<(u8, Vec<u8>, u16)> {
        let key = self.key.expect("logged in");
        let raw: Vec<Vec<u8>> = self.central().notifications.drain(..).collect();
        let address = self.world.borrow().address;
        raw.iter()
            .filter_map(|n| {
                assert_eq!(n[..3], [0x1b, 0x12, 0x00], "notification of the status characteristic");
                let mut p = n[3..].to_vec();
                let mut nonce = [0u8; 8];
                nonce[..3].copy_from_slice(&address[..3]);
                nonce[3..].copy_from_slice(&p[..5]);
                mesh_crypt(&key, &nonce, &mut p[7..]);
                let mic = mesh_mic(&key, &nonce, &p[7..]);
                (mic[..2] == p[5..7]).then(|| (p[7], p[10..].to_vec(), u16::from_le_bytes([p[3], p[4]])))
            })
            .collect()
    }

    pub fn is_on(&self) -> bool {
        self.bulb.is_on()
    }

    pub fn level(&self) -> u8 {
        self.bulb.state().level
    }
}

pub fn run_all(rigs: &mut [&mut Rig], ms: u64) {
    let start = rigs.iter().map(|r| r.world.borrow().now).min().unwrap();
    for slice in 1..=ms / 5 {
        for rig in rigs.iter_mut() {
            rig.run_until(start + slice * 5 * MS);
        }
    }
}

pub fn ota_packet(index: u16, block: &[u8]) -> Vec<u8> {
    let mut p = index.to_le_bytes().to_vec();
    p.extend_from_slice(block);
    let crc = crc16_modbus(&p);
    p.extend_from_slice(&crc.to_le_bytes());
    p
}

pub fn make_image(len: usize, seed: u8) -> Vec<u8> {
    let mut image: Vec<u8> = (0..len).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect();
    image[8..12].copy_from_slice(b"KNLT");
    image[0x18..0x1c].copy_from_slice(&(len as u32).to_le_bytes());
    let n = len - 4;
    let mut crc = 0xffff_ffffu32;
    for b in &image[..n] {
        crc ^= *b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    image[n..].copy_from_slice(&crc.to_le_bytes());
    image
}
