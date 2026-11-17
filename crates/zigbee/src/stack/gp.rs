use super::frag::SINGLE_MAX;
use super::Stack;
use crate::aes::Aes128;
use crate::crypto;
use crate::hal::{Hal, NvItem};
use crate::mac::Addr;
use crate::nwk::{self, Header};
use crate::util::{due, le, pack, Buf, Buffer, Long, Reader};

pub(crate) const GP_ENDPOINT: u8 = 242;
pub(crate) const PROFILE_GP: u16 = 0xA1E0;
pub(crate) const CLUSTER_GP: u16 = 0x0021;
pub(crate) const DEVICE_PROXY_BASIC: u16 = 0x0061;
pub(crate) const PROXIES: usize = 5;
const SINKS: usize = 2;
const SINK_GROUPS: usize = 2;

const GP_NOTIFICATION: u8 = 0x00;
const GP_COMMISSIONING_NOTIFICATION: u8 = 0x04;
const GP_PROXY_TABLE_RESPONSE: u8 = 0x0B;
const GP_PAIRING: u8 = 0x01;
const GP_PROXY_COMMISSIONING_MODE: u8 = 0x02;
const GP_RESPONSE: u8 = 0x06;
const GP_PROXY_TABLE_REQUEST: u8 = 0x0B;

const ATTR_MAX_PROXY_TABLE_ENTRIES: u16 = 0x0010;
const ATTR_PROXY_TABLE: u16 = 0x0011;
const ATTR_FUNCTIONALITY: u16 = 0x0016;
const ATTR_ACTIVE_FUNCTIONALITY: u16 = 0x0017;
const ATTR_SHARED_KEY_TYPE: u16 = 0x0020;
const ATTR_SHARED_KEY: u16 = 0x0021;
const ATTR_LINK_KEY: u16 = 0x0022;
const ATTR_CLUSTER_REVISION: u16 = 0xFFFD;
const FUNCTIONALITY: u32 = 0x09_AC2F;
const ACTIVE_FUNCTIONALITY: u32 = 0xFF_FFFF;
const CLUSTER_REVISION: u16 = 2;

const GPD_COMMISSIONING: u8 = 0xE0;
const GPD_DECOMMISSIONING: u8 = 0xE1;
const GPD_SUCCESS: u8 = 0xE2;
const GPD_CHANNEL_REQUEST: u8 = 0xE3;
const GPD_TO_DEVICE: u8 = 0xF0;
const SRC_ID_RESERVED: u32 = 0xFFFF_FFF9;
const NO_IEEE: u64 = 0xFFFF_FFFF_FFFF_FFFF;

const DMIN_U: u32 = 5;
const DMIN_B: u32 = 32;
const DMAX: u32 = 100;

const RX_OFFSET_US: u32 = 20_000;
const TX_QUEUE_MS: u32 = 5_000;
const MAX_TX_PAYLOAD: usize = 48;

const KEY_NONE: u8 = 0;
const KEY_NWK: u8 = 1;
const KEY_GROUP: u8 = 2;
const KEY_NWK_DERIVED_GROUP: u8 = 3;
const KEY_INDIVIDUAL: u8 = 4;
const KEY_DERIVED_INDIVIDUAL: u8 = 7;

const SUCCESS: u8 = 0x00;
const MALFORMED_COMMAND: u8 = 0x80;
const UNSUP_COMMAND: u8 = 0x81;
const UNSUPPORTED_ATTRIBUTE: u8 = 0x86;
const INVALID_VALUE: u8 = 0x87;
const READ_ONLY: u8 = 0x88;
const INSUFFICIENT_SPACE: u8 = 0x89;
const INVALID_DATA_TYPE: u8 = 0x8D;
const NOT_FOUND: u8 = 0x8B;
const T_BITMAP8: u8 = 0x18;
const T_BITMAP24: u8 = 0x1A;
const T_U8: u8 = 0x20;
const T_U16: u8 = 0x21;
const T_LONG_OCTETS: u8 = 0x43;
const T_KEY: u8 = 0xF1;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Gpd {
    Src(u32),
    Ieee(u64),
}

impl Gpd {
    fn app(self) -> u8 {
        match self {
            Gpd::Src(_) => 0,
            Gpd::Ieee(_) => 2,
        }
    }

    fn read(app: u8, r: &mut Reader) -> Option<(Gpd, u8)> {
        match app {
            0 => Some((Gpd::Src(r.u32()?), 0)),
            2 => {
                let ieee = r.u64()?;
                Some((Gpd::Ieee(ieee), r.u8()?))
            }
            _ => None,
        }
    }

    fn write<const N: usize>(self, out: &mut Buffer<N>, endpoint: u8) {
        match self {
            Gpd::Src(id) => out.u32(id),
            Gpd::Ieee(ieee) => {
                out.u64(ieee);
                out.u8(endpoint);
            }
        }
    }

    fn is_valid(self) -> bool {
        match self {
            Gpd::Src(id) => id != 0 && id < SRC_ID_RESERVED,
            Gpd::Ieee(ieee) => ieee != 0 && ieee != NO_IEEE,
        }
    }

    pub(crate) fn alias(self) -> u16 {
        let (low, high) = match self {
            Gpd::Src(id) => (id as u16, (id >> 16) as u16),
            Gpd::Ieee(ieee) => (ieee as u16, (ieee >> 16) as u16),
        };
        let valid = |a: u16| a != 0 && a <= 0xFFF7;
        if valid(low) {
            return low;
        }
        let mixed = low ^ high;
        if valid(mixed) {
            mixed
        } else if mixed == 0 {
            0x0007
        } else {
            mixed - 0x0008
        }
    }

    fn nonce(self, counter: u32) -> [u8; 13] {
        let mut n = [0u8; 13];
        match self {
            Gpd::Src(id) => {
                n[..4].copy_from_slice(&id.to_le_bytes());
                n[4..8].copy_from_slice(&id.to_le_bytes());
            }
            Gpd::Ieee(ieee) => n[..8].copy_from_slice(&ieee.to_le_bytes()),
        }
        n[8..12].copy_from_slice(&counter.to_le_bytes());
        n[12] = 0x05;
        n
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Queued {
    pub gpd: Gpd,
    pub endpoint: u8,
    pub channel: u8,
    pub cmd: u8,
    pub len: u8,
    pub payload: [u8; MAX_TX_PAYLOAD],
    pub expires: u32,
}

const DEFAULT_WINDOW_S: u32 = 180;
const DUPLICATE_MS: u32 = 2_000;
const ENTRY_VERSION: u8 = 2;
const ENTRY_MAX: usize = 38 + 10 * SINKS + 4 * SINK_GROUPS;
const KEYS_VERSION: u8 = 1;

const MODE_FULL_UNICAST: u8 = 0;
const MODE_COMMISSIONED_GROUP: u8 = 2;
const MODE_LIGHTWEIGHT_UNICAST: u8 = 3;

#[derive(Clone, Copy)]
pub(crate) struct ProxyEntry {
    pub used: bool,
    pub gpd: Gpd,
    pub endpoint: u8,
    pub sinks: [(u64, u16); SINKS],
    pub sink_count: u8,
    pub groups: [(u16, u16); SINK_GROUPS],
    pub group_count: u8,
    pub derived_group: bool,
    pub alias: Option<u16>,
    pub radius: u8,
    pub seq_cap: bool,
    pub fixed: bool,
    pub in_range: bool,
    pub security: u8,
    pub key_type: u8,
    pub key: [u8; 16],
    pub counter: u32,
}

impl ProxyEntry {
    pub(crate) const EMPTY: ProxyEntry = ProxyEntry {
        used: false,
        gpd: Gpd::Src(0),
        endpoint: 0,
        sinks: [(0, 0); SINKS],
        sink_count: 0,
        groups: [(0, 0); SINK_GROUPS],
        group_count: 0,
        derived_group: false,
        alias: None,
        radius: 0xFF,
        seq_cap: false,
        fixed: false,
        in_range: false,
        security: 0,
        key_type: 0,
        key: [0; 16],
        counter: 0,
    };

    fn has_destination(&self) -> bool {
        self.sink_count != 0 || self.group_count != 0 || self.derived_group
    }

    fn write<const N: usize>(&self, out: &mut Buffer<N>) {
        let secure = self.security >= 2;
        let options = pack(&[
            (self.gpd.app() as u16, 0),
            (1, 3),
            (1, 4),
            (self.seq_cap as u16, 5),
            ((self.sink_count != 0) as u16, 6),
            (self.derived_group as u16, 7),
            ((self.group_count != 0) as u16, 8),
            (self.in_range as u16, 10),
            (self.fixed as u16, 11),
            (self.alias.is_some() as u16, 13),
            (secure as u16, 14),
        ]);
        out.u16(options);
        self.gpd.write(out, self.endpoint);
        if let Some(alias) = self.alias {
            out.u16(alias);
        }
        if secure {
            out.u8(pack(&[(self.security as u16, 0), (self.key_type as u16, 2)]) as u8);
        }
        if secure || self.seq_cap {
            out.u32(self.counter);
        }
        if secure {
            out.bytes(&self.key);
        }
        if self.sink_count != 0 {
            out.u8(self.sink_count);
            for (ext, short) in &self.sinks[..self.sink_count as usize] {
                out.u64(*ext);
                out.u16(*short);
            }
        }
        if self.group_count != 0 {
            out.u8(self.group_count);
            for (group, alias) in &self.groups[..self.group_count as usize] {
                out.u16(*group);
                out.u16(*alias);
            }
        }
        out.u8(self.radius);
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Keys {
    pub shared_type: u8,
    pub shared: [u8; 16],
    pub link: [u8; 16],
}

impl Keys {
    pub(crate) const DEFAULT: Keys = Keys { shared_type: KEY_NONE, shared: [0; 16], link: super::DEFAULT_TCLK };
}

#[derive(Clone, Copy)]
pub(crate) struct Commissioning {
    pub until: u32,
    pub sink: u16,
    pub unicast: bool,
    pub exit_on_pairing: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Forward {
    Sink(u16),
    Broadcast,
    Sinks(u8),
    Groups(u8),
    Announce,
}

#[derive(Clone, Copy)]
pub(crate) struct Pending {
    pub at: u32,
    pub to: Forward,
    pub alias: u16,
    pub seq: u8,
    pub then: u8,
    pub len: u8,
    pub zcl: [u8; 96],
}

struct Gpdf<'a> {
    gpd: Gpd,
    endpoint: u8,
    maintenance: bool,
    auto_commissioning: bool,
    security: u8,
    key_bit: u8,
    rx_after_tx: bool,
    listens: bool,
    counter: u32,
    header: &'a [u8],
    body: &'a [u8],
}

fn parse<'a>(mac_seq: u8, mac_src: Addr, p: &'a [u8]) -> Option<Gpdf<'a>> {
    let mut r = Reader::new(p);
    let fc = r.u8()?;
    let auto_commissioning = fc & 0x40 != 0;
    if fc & 3 == 1 && fc & 0x80 == 0 {
        return Some(Gpdf {
            gpd: Gpd::Src(0),
            endpoint: 0,
            maintenance: true,
            auto_commissioning,
            security: 0,
            key_bit: 0,
            rx_after_tx: true,
            listens: !auto_commissioning,
            counter: mac_seq as u32,
            header: &p[..1],
            body: r.rest(),
        });
    }
    if fc & 3 != 0 {
        return None;
    }
    let (mut app, mut security, mut key_bit, mut rx_after_tx, mut to_gpd) = (0, 0, 0, false, false);
    if fc & 0x80 != 0 {
        let ext = r.u8()?;
        app = ext & 7;
        security = (ext >> 3) & 3;
        key_bit = (ext >> 5) & 1;
        rx_after_tx = ext & 0x40 != 0;
        to_gpd = ext & 0x80 != 0;
    }
    if to_gpd || security == 1 || (rx_after_tx && auto_commissioning) {
        return None;
    }
    let (gpd, endpoint) = match (app, mac_src) {
        (0, _) => (Gpd::Src(r.u32()?), 0),
        (2, Addr::Ext(ieee)) => (Gpd::Ieee(ieee), r.u8()?),
        _ => return None,
    };
    if !gpd.is_valid() {
        return None;
    }
    let counter = if security >= 2 { r.u32()? } else { mac_seq as u32 };
    let header = &p[..r.pos()];
    Some(Gpdf {
        gpd,
        endpoint,
        maintenance: false,
        auto_commissioning,
        security,
        key_bit,
        rx_after_tx,
        listens: rx_after_tx,
        counter,
        header,
        body: r.rest(),
    })
}

fn unsecure(key: &[u8; 16], engine: Option<crate::aes::Engine>, f: &Gpdf, out: &mut [u8; 96]) -> Option<usize> {
    let len = f.body.len().checked_sub(crypto::MIC_LEN)?;
    let body = out.get_mut(..f.body.len())?;
    body.copy_from_slice(f.body);
    let nonce = f.gpd.nonce(f.counter);
    let aes = Aes128::with_engine(key, engine);
    if f.security == 2 {
        let mut aad = [0u8; 110];
        let n = f.header.len() + len;
        let a = aad.get_mut(..n)?;
        a[..f.header.len()].copy_from_slice(f.header);
        a[f.header.len()..].copy_from_slice(&f.body[..len]);
        crypto::ccm_decrypt(&aes, &nonce, &aad[..n], &mut body[len..])?;
        Some(len)
    } else {
        crypto::ccm_decrypt(&aes, &nonce, f.header, body)?;
        Some(len)
    }
}

fn link(lqi: u8, rssi: i8) -> u8 {
    let rssi = (rssi as i32).clamp(-109, 8);
    pack(&[(((rssi + 110) / 2) as u16, 0), ((lqi >> 6) as u16, 6)]) as u8
}

fn dmin(rx_after_tx: bool) -> u32 {
    if rx_after_tx {
        DMIN_B
    } else {
        DMIN_U
    }
}

fn quality_delay(lqi: u8) -> u32 {
    match lqi >> 6 {
        3 => 0,
        2 => 32,
        1 => 64,
        _ => 96,
    }
}

fn is_null(key: &[u8; 16]) -> bool {
    key.iter().all(|b| *b == 0)
}

impl<H: Hal> Stack<H> {
    fn proxy_index(&self, gpd: Gpd) -> Option<usize> {
        self.proxies.iter().position(|e| e.used && e.gpd == gpd)
    }

    fn gp_commissioning(&mut self, now: u32) -> Option<Commissioning> {
        match self.gp_mode {
            Some(c) if !due(now, c.until) => Some(c),
            _ => {
                self.gp_mode = None;
                None
            }
        }
    }

    fn gp_entry_key(&self, e: &ProxyEntry, key_bit: u8) -> Option<[u8; 16]> {
        let individual = matches!(e.key_type, KEY_INDIVIDUAL | KEY_DERIVED_INDIVIDUAL);
        let shared = matches!(e.key_type, KEY_NWK | KEY_GROUP | KEY_NWK_DERIVED_GROUP);
        if (key_bit != 0) != individual || !(individual || shared) {
            return None;
        }
        if !is_null(&e.key) {
            Some(e.key)
        } else if shared && self.gp_keys.shared_type == e.key_type && !is_null(&self.gp_keys.shared) {
            Some(self.gp_keys.shared)
        } else if e.key_type == KEY_NWK {
            Some(self.nib.key)
        } else {
            None
        }
    }

    pub(crate) fn on_gpdf(&mut self, mac_seq: u8, mac_src: Addr, p: &[u8], lqi: u8, end_us: u32) -> Option<()> {
        let f = parse(mac_seq, mac_src, p)?;
        let now = self.hal.now_ms();
        if f.security != 3 && f.body.first().is_some_and(|cmd| *cmd >= GPD_TO_DEVICE) {
            return None;
        }
        if self.gp_seen.iter().any(|s| s.0 == f.gpd && s.1 == f.counter && !due(now, s.2)) {
            return None;
        }
        let commissioning = self.gp_commissioning(now);
        let entry = self.proxy_index(f.gpd).filter(|i| {
            let e = &self.proxies[*i];
            matches!(f.gpd, Gpd::Src(_)) || f.endpoint == e.endpoint || matches!(f.endpoint, 0 | 0xFF) || matches!(e.endpoint, 0 | 0xFF)
        });
        let mut plain = [0u8; 96];
        let mut key_type = KEY_NONE;
        let processed = if f.security == 0 {
            plain.get_mut(..f.body.len())?.copy_from_slice(f.body);
            Some(f.body.len())
        } else if let Some(i) = entry {
            let e = self.proxies[i];
            key_type = e.key_type;
            let fits = f.security == e.security && f.counter > e.counter;
            self.gp_entry_key(&e, f.key_bit).filter(|_| fits).and_then(|key| unsecure(&key, H::AES, &f, &mut plain))
        } else if f.key_bit == 0 {
            key_type = self.gp_keys.shared_type;
            let key = self.gp_keys.shared;
            if is_null(&key) {
                None
            } else {
                unsecure(&key, H::AES, &f, &mut plain)
            }
        } else {
            key_type = KEY_INDIVIDUAL;
            None
        };
        if f.security == 3 && processed.is_some_and(|n| n > 0 && plain[0] >= GPD_TO_DEVICE) {
            return None;
        }
        if f.listens && processed.is_some() {
            self.gp_answer(&f, end_us);
        }
        if let Some(c) = commissioning {
            self.gp_remember(f.gpd, f.counter, now);
            self.gp_commissioning_notification(&c, &f, processed.map(|n| &plain[..n]), key_type, mac_seq, lqi, now);
            return Some(());
        }
        let i = entry?;
        let e = self.proxies[i];
        let len = processed?;
        if f.maintenance || f.security != e.security || len == 0 {
            return None;
        }
        let cmd = plain[0];
        if matches!(cmd, GPD_CHANNEL_REQUEST | GPD_SUCCESS) {
            return None;
        }
        if f.security == 0 && e.seq_cap && f.counter == e.counter {
            return None;
        }
        self.proxies[i].counter = f.counter;
        self.proxies[i].in_range = true;
        self.store_proxy(i);
        self.gp_remember(f.gpd, f.counter, now);

        let mut z: Buffer<96> = Buffer::new();
        self.zcl_seq = self.zcl_seq.wrapping_add(1);
        z.bytes(&[0x11, self.zcl_seq, GP_NOTIFICATION]);
        let rx_after_tx = f.rx_after_tx && !matches!(cmd, GPD_COMMISSIONING | GPD_DECOMMISSIONING);
        let options = pack(&[
            (f.gpd.app() as u16, 0),
            ((e.sink_count != 0) as u16, 3),
            (e.derived_group as u16, 4),
            ((e.group_count != 0) as u16, 5),
            (e.security as u16, 6),
            (e.key_type as u16, 8),
            (rx_after_tx as u16, 11),
            (1, 12),
            (1, 14),
        ]);
        z.u16(options);
        f.gpd.write(&mut z, f.endpoint);
        z.u32(f.counter);
        z.u8(cmd);
        z.u8(len as u8 - 1);
        z.bytes(&plain[1..len]);
        z.u16(self.nib.short);
        z.u8(link(lqi, self.rx_signal.1));
        if !z.ok() {
            return None;
        }
        let mut zcl = [0u8; 96];
        zcl[..z.len()].copy_from_slice(z.as_slice());
        let pending = Pending {
            at: now.wrapping_add(dmin(rx_after_tx)),
            to: Forward::Sinks(i as u8),
            alias: f.gpd.alias(),
            seq: mac_seq,
            then: if rx_after_tx { quality_delay(lqi) as u8 } else { 0 },
            len: z.len() as u8,
            zcl,
        };
        match self.gp_pending.iter().position(|p| p.is_none()) {
            Some(slot) => self.gp_pending[slot] = Some(pending),
            None => {
                self.gp_notify(&pending, i as u8, true);
                self.gp_notify(&pending, i as u8, false);
            }
        }
        Some(())
    }

    fn gp_notify(&mut self, p: &Pending, i: u8, sinks: bool) {
        let e = self.proxies[i as usize];
        let zcl = &p.zcl[..p.len as usize];
        let mut id: Buffer<9> = Buffer::new();
        e.gpd.write(&mut id, 0);
        let n = id.len().min(8);
        if !e.used || zcl.get(5..5 + n) != Some(&id.as_slice()[..n]) {
            return;
        }
        if sinks {
            for (_, sink) in &e.sinks[..e.sink_count as usize] {
                self.aps_data(*sink, GP_ENDPOINT, CLUSTER_GP, PROFILE_GP, GP_ENDPOINT, zcl);
            }
            return;
        }
        let radius = if matches!(e.radius, 0 | 0xFF) { nwk::DEFAULT_RADIUS } else { e.radius };
        let derived = e.gpd.alias();
        if e.derived_group {
            self.gp_group_notification(derived, e.alias.unwrap_or(derived), p.seq, radius, zcl);
        }
        for (group, alias) in &e.groups[..e.group_count as usize] {
            let alias = if *alias == 0xFFFF { derived } else { *alias };
            self.gp_group_notification(*group, alias, p.seq.wrapping_sub(9), radius, zcl);
        }
    }

    fn gp_group_notification(&mut self, group: u16, alias: u16, seq: u8, radius: u8, zcl: &[u8]) {
        let mut aps = Buf::new();
        aps.u8(0x0C);
        aps.u16(group);
        aps.u16(CLUSTER_GP);
        aps.u16(PROFILE_GP);
        aps.u8(GP_ENDPOINT);
        aps.u8(seq);
        aps.bytes(zcl);
        self.gp_alias_broadcast(alias, seq, radius, aps.as_slice());
    }

    fn gp_answer(&mut self, f: &Gpdf, end_us: u32) {
        let Some(q) = self.gp_tx else {
            return;
        };
        if q.gpd != f.gpd {
            return;
        }
        self.gp_tx = None;
        let mut m = Buf::new();
        match q.gpd {
            Gpd::Ieee(ieee) => {
                m.u16(0x0C01);
                m.u8(self.next_mac_seq());
                m.u16(0xFFFF);
                m.u64(ieee);
                m.u8(0x8C);
                m.u8(0x82);
                m.u8(q.endpoint);
            }
            Gpd::Src(id) => {
                m.u16(0x0801);
                m.u8(self.next_mac_seq());
                m.u16(0xFFFF);
                m.u16(0xFFFF);
                if id == 0 {
                    m.u8(0x0D);
                } else {
                    m.u8(0x8C);
                    m.u8(0x80);
                    m.u32(id);
                }
            }
        }
        m.u8(q.cmd);
        m.bytes(&q.payload[..q.len as usize]);
        self.hal.transmit_at(m.as_slice(), end_us.wrapping_add(RX_OFFSET_US));
        if q.channel != self.nib.channel {
            self.hal.set_channel(self.nib.channel);
        }
    }

    pub(crate) fn gp_timers(&mut self, now: u32) {
        if let Some(q) = self.gp_tx {
            if due(now, q.expires) {
                self.gp_tx = None;
                if q.channel != self.nib.channel {
                    self.hal.set_channel(self.nib.channel);
                }
            }
        }
        for i in 0..self.gp_pending.len() {
            let Some(p) = self.gp_pending[i] else {
                continue;
            };
            if !due(now, p.at) {
                continue;
            }
            self.gp_pending[i] = None;
            let zcl = &p.zcl[..p.len as usize];
            match p.to {
                Forward::Sink(sink) => self.aps_data(sink, GP_ENDPOINT, CLUSTER_GP, PROFILE_GP, GP_ENDPOINT, zcl),
                Forward::Broadcast => {
                    let mut aps = Buf::new();
                    aps.u8(0x08);
                    aps.u8(GP_ENDPOINT);
                    aps.u16(CLUSTER_GP);
                    aps.u16(PROFILE_GP);
                    aps.u8(GP_ENDPOINT);
                    aps.u8(p.seq);
                    aps.bytes(zcl);
                    self.gp_alias_broadcast(p.alias, p.seq, nwk::DEFAULT_RADIUS, aps.as_slice());
                }
                Forward::Sinks(entry) => {
                    self.gp_notify(&p, entry, true);
                    if p.then == 0 {
                        self.gp_notify(&p, entry, false);
                    } else {
                        self.gp_pending[i] = Some(Pending { at: now.wrapping_add(p.then as u32), to: Forward::Groups(entry), ..p });
                    }
                }
                Forward::Groups(entry) => self.gp_notify(&p, entry, false),
                Forward::Announce => {
                    self.zdo_seq = self.zdo_seq.wrapping_add(1);
                    let mut aps = Buf::new();
                    aps.bytes(&[0x08, 0x00, 0x13, 0x00, 0x00, 0x00, 0x00, 0x00, self.zdo_seq]);
                    aps.u16(p.alias);
                    aps.u64(NO_IEEE);
                    aps.u8(0x00);
                    self.gp_alias_broadcast(p.alias, 0, nwk::DEFAULT_RADIUS, aps.as_slice());
                }
            }
        }
    }

    pub(crate) fn gp_device_announced(&mut self, short: u16, ext: u64) {
        if ext == NO_IEEE {
            if short == self.nib.short {
                self.address_conflict(short);
            }
            return;
        }
        let now = self.hal.now_ms();
        let mut conflict = false;
        for i in 0..PROXIES {
            let mut e = self.proxies[i];
            if !e.used {
                continue;
            }
            let mut moved = false;
            for sink in e.sinks[..e.sink_count as usize].iter_mut().filter(|s| s.0 == ext && s.1 != short) {
                sink.1 = short;
                moved = true;
            }
            if moved {
                self.proxies[i] = e;
                self.store_proxy(i);
            }
            let derived = e.gpd.alias();
            conflict |= derived == short
                || (e.derived_group && e.alias == Some(short))
                || e.groups[..e.group_count as usize].iter().any(|g| g.1 == short);
        }
        if !conflict || self.gp_commissioning(now).is_some() || self.gp_pending.iter().flatten().any(|p| p.to == Forward::Announce && p.alias == short) {
            return;
        }
        if let Some(slot) = self.gp_pending.iter().position(|p| p.is_none()) {
            let at = now.wrapping_add(DMIN_U + self.hal.random() % (DMAX - DMIN_U + 1));
            self.gp_pending[slot] = Some(Pending { at, to: Forward::Announce, alias: short, seq: 0, then: 0, len: 0, zcl: [0; 96] });
        }
    }

    fn gp_remember(&mut self, gpd: Gpd, counter: u32, now: u32) {
        self.gp_seen.rotate_right(1);
        self.gp_seen[0] = (gpd, counter, now.wrapping_add(DUPLICATE_MS));
    }

    #[allow(clippy::too_many_arguments)]
    fn gp_commissioning_notification(&mut self, c: &Commissioning, f: &Gpdf, plain: Option<&[u8]>, key_type: u8, mac_seq: u8, lqi: u8, now: u32) {
        let cmd = plain.and_then(|p| p.first().copied());
        if f.maintenance && cmd != Some(GPD_CHANNEL_REQUEST) {
            return;
        }
        if f.auto_commissioning && matches!(cmd, Some(GPD_COMMISSIONING | GPD_SUCCESS)) {
            return;
        }
        if !f.maintenance && cmd == Some(GPD_CHANNEL_REQUEST) {
            return;
        }
        let mut z: Buffer<96> = Buffer::new();
        self.zcl_seq = self.zcl_seq.wrapping_add(1);
        z.bytes(&[0x11, self.zcl_seq, GP_COMMISSIONING_NOTIFICATION]);
        let failed = plain.is_none();
        let key_type = match (f.security, failed) {
            (_, true) if f.key_bit != 0 => KEY_INDIVIDUAL,
            (_, true) => KEY_NONE,
            _ => key_type,
        };
        let options = pack(&[
            (f.gpd.app() as u16, 0),
            (f.rx_after_tx as u16, 3),
            (f.security as u16, 4),
            (key_type as u16, 6),
            (failed as u16, 9),
            (1, 11),
        ]);
        z.u16(options);
        f.gpd.write(&mut z, f.endpoint);
        z.u32(f.counter);
        match plain {
            Some(p) if !p.is_empty() => {
                z.u8(p[0]);
                z.u8(p.len() as u8 - 1);
                z.bytes(&p[1..]);
            }
            _ => {
                let n = f.body.len().saturating_sub(crypto::MIC_LEN);
                z.u8(f.body.first().copied().unwrap_or(0));
                z.u8(n.saturating_sub(1) as u8);
                z.bytes(f.body.get(1..n).unwrap_or(&[]));
            }
        }
        z.u16(self.nib.short);
        z.u8(link(lqi, self.rx_signal.1));
        if failed {
            z.bytes(&f.body[f.body.len().saturating_sub(crypto::MIC_LEN)..]);
        }
        if !z.ok() {
            return;
        }
        let Some(slot) = self.gp_pending.iter().position(|p| p.is_none()) else {
            return;
        };
        let mut zcl = [0u8; 96];
        zcl[..z.len()].copy_from_slice(z.as_slice());
        let delay = if f.rx_after_tx && !c.unicast && !self.gp_first { DMIN_B + quality_delay(lqi) } else { dmin(f.rx_after_tx) };
        self.gp_pending[slot] = Some(Pending {
            at: now.wrapping_add(delay),
            to: if c.unicast { Forward::Sink(c.sink) } else { Forward::Broadcast },
            alias: f.gpd.alias(),
            seq: mac_seq.wrapping_sub(12),
            then: 0,
            len: z.len() as u8,
            zcl,
        });
    }

    fn gp_alias_broadcast(&mut self, alias: u16, seq: u8, radius: u8, aps: &[u8]) {
        let now = self.hal.now_ms();
        if self.broadcast_seen(alias, seq, now) {
            return;
        }
        let h = Header::new(nwk::FRAME_DATA, nwk::BROADCAST_RX_ON, alias, radius, seq);
        self.nwk_send(&h, aps);
    }

    pub(crate) fn on_gp(&mut self, src: u16, src_ep: u8, p: &[u8], unicast: bool) -> Option<()> {
        let mut r = Reader::new(p);
        let fc = r.u8()?;
        if fc & 0x04 != 0 {
            return None;
        }
        let seq = r.u8()?;
        let cmd = r.u8()?;
        let mut rsp = Long::new();
        let rsp_fc = pack(&[(1, 4), ((fc & 0x08 == 0) as u16, 3)]) as u8;
        let status = if fc & 3 == 0 {
            self.gp_global(cmd, r, seq, rsp_fc, &mut rsp)
        } else if fc & 0x08 == 0 {
            Some(UNSUP_COMMAND)
        } else {
            match self.gp_command(src, cmd, r, seq, &mut rsp) {
                Some(status) => status,
                None => Some(MALFORMED_COMMAND),
            }
        };
        let found = fc & 0x0B == 0x09 && cmd == GP_PROXY_TABLE_REQUEST && rsp.as_slice().get(3) == Some(&SUCCESS);
        if !unicast && !found {
            return Some(());
        }
        if rsp.is_empty() {
            match status {
                Some(status) if (status != SUCCESS || fc & 0x10 == 0) && !(fc & 3 == 0 && cmd == 0x0B) => {
                    rsp.bytes(&[rsp_fc, seq, 0x0B, cmd, status])
                }
                _ => return Some(()),
            }
        }
        if rsp.ok() {
            self.aps_data(src, src_ep, CLUSTER_GP, PROFILE_GP, GP_ENDPOINT, rsp.as_slice());
        }
        Some(())
    }

    fn gp_command(&mut self, src: u16, cmd: u8, mut r: Reader, seq: u8, rsp: &mut Long) -> Option<Option<u8>> {
        let now = self.hal.now_ms();
        match cmd {
            GP_PROXY_COMMISSIONING_MODE => {
                let options = r.u8()?;
                if self.gp_commissioning(now).is_some_and(|c| c.sink != src) {
                    return Some(Some(SUCCESS));
                }
                if options & 1 == 0 {
                    self.gp_mode = None;
                    return Some(Some(SUCCESS));
                }
                self.gp_first = false;
                let window = if options & 0x02 != 0 { r.u16()? as u32 } else { DEFAULT_WINDOW_S };
                self.gp_mode = Some(Commissioning {
                    until: now.wrapping_add(window * 1000),
                    sink: src,
                    unicast: options & 0x20 != 0,
                    exit_on_pairing: options & 0x04 != 0,
                });
                Some(Some(SUCCESS))
            }
            GP_RESPONSE => {
                if self.gp_commissioning(now).is_none() {
                    return Some(None);
                }
                let options = r.u8()?;
                let temp_master = r.u16()?;
                let channel = 11 + (r.u8()? & 0x0F);
                let (gpd, endpoint) = Gpd::read(options & 7, &mut r)?;
                let gpd_cmd = r.u8()?;
                let len = match r.u8()? {
                    0xFF => 0,
                    n => n as usize,
                };
                let data = r.take(len)?;
                self.gp_first = temp_master == self.nib.short;
                if !self.gp_first {
                    if self.gp_tx.is_some_and(|q| q.gpd == gpd) {
                        self.gp_tx = None;
                    }
                    return Some(Some(SUCCESS));
                }
                if len > MAX_TX_PAYLOAD {
                    return None;
                }
                let mut payload = [0u8; MAX_TX_PAYLOAD];
                payload[..len].copy_from_slice(data);
                self.gp_tx = Some(Queued { gpd, endpoint, channel, cmd: gpd_cmd, len: len as u8, payload, expires: now.wrapping_add(TX_QUEUE_MS) });
                if channel != self.nib.channel {
                    self.hal.set_channel(channel);
                }
                Some(Some(SUCCESS))
            }
            GP_PAIRING => Some(Some(self.gp_pairing(r)?)),
            GP_PROXY_TABLE_REQUEST => {
                let options = r.u8()?;
                let used = self.proxies.iter().filter(|e| e.used).count() as u8;
                rsp.bytes(&[0x11, seq, GP_PROXY_TABLE_RESPONSE]);
                let by_index = (options >> 3) & 3 == 1;
                let (first, one) = if by_index {
                    (r.u8()?, None)
                } else {
                    (0xFF, Some(Gpd::read(options & 7, &mut r)?.0))
                };
                let status_at = rsp.len();
                rsp.bytes(&[NOT_FOUND, used, first, 0]);
                let mut count = 0;
                for (n, e) in self.proxies.iter().filter(|e| e.used).enumerate() {
                    let wanted = match one {
                        Some(gpd) => e.gpd == gpd,
                        None => n >= first as usize,
                    };
                    if !wanted {
                        continue;
                    }
                    let mut entry: Buffer<72> = Buffer::new();
                    e.write(&mut entry);
                    if rsp.len() + entry.len() > SINGLE_MAX {
                        break;
                    }
                    rsp.bytes(entry.as_slice());
                    count += 1;
                }
                if count != 0 {
                    rsp.set(status_at, SUCCESS);
                    rsp.set(status_at + 3, count);
                }
                Some(None)
            }
            _ => Some(Some(UNSUP_COMMAND)),
        }
    }

    fn gp_pairing(&mut self, mut r: Reader) -> Option<u8> {
        let options = le(r.take(3)?);
        let (gpd, endpoint) = Gpd::read((options & 7) as u8, &mut r)?;
        let found = self.proxy_index(gpd);
        if options & 0x10 != 0 {
            let i = found?;
            self.proxies[i] = ProxyEntry::EMPTY;
            self.store_proxy(i);
            return Some(SUCCESS);
        }
        let mode = ((options >> 5) & 3) as u8;
        let security = ((options >> 9) & 3) as u8;
        if mode == MODE_FULL_UNICAST || security == 1 {
            return None;
        }
        let (mut sink, mut group) = ((0u64, 0u16), 0u16);
        if mode == MODE_LIGHTWEIGHT_UNICAST {
            sink = (r.u64()?, r.u16()?);
        } else {
            group = r.u16()?;
        }
        let add = options & 0x08 != 0;
        if !add {
            let i = found?;
            let e = &mut self.proxies[i];
            match mode {
                MODE_LIGHTWEIGHT_UNICAST => {
                    let n = e.sink_count as usize;
                    let at = e.sinks[..n].iter().position(|s| s.0 == sink.0)?;
                    e.sinks.copy_within(at + 1..n, at);
                    e.sink_count -= 1;
                }
                MODE_COMMISSIONED_GROUP => {
                    let n = e.group_count as usize;
                    let at = e.groups[..n].iter().position(|g| g.0 == group)?;
                    e.groups.copy_within(at + 1..n, at);
                    e.group_count -= 1;
                }
                _ => e.derived_group = false,
            }
            if !e.has_destination() {
                *e = ProxyEntry::EMPTY;
            }
            self.store_proxy(i);
            return Some(SUCCESS);
        }
        r.skip(1)?;
        let counter = if options & (1 << 14) != 0 { Some(r.u32()?) } else { None };
        let key = if options & (1 << 15) != 0 { Some(r.key()?) } else { None };
        let alias = if options & (1 << 16) != 0 { Some(r.u16()?) } else { None };
        let radius = if options & (1 << 17) != 0 { Some(r.u8()?) } else { None };
        let i = found.or_else(|| self.proxies.iter().position(|e| !e.used))?;
        let mut e = self.proxies[i];
        if !e.used {
            e = ProxyEntry { used: true, gpd, ..ProxyEntry::EMPTY };
        }
        match mode {
            MODE_LIGHTWEIGHT_UNICAST => {
                let n = e.sink_count as usize;
                match e.sinks[..n].iter().position(|s| s.0 == sink.0) {
                    Some(at) => e.sinks[at] = sink,
                    None => {
                        *e.sinks.get_mut(n)? = sink;
                        e.sink_count += 1;
                    }
                }
            }
            MODE_COMMISSIONED_GROUP => {
                let n = e.group_count as usize;
                let entry = (group, alias.unwrap_or(0xFFFF));
                match e.groups[..n].iter().position(|g| g.0 == group) {
                    Some(at) => e.groups[at] = entry,
                    None => {
                        *e.groups.get_mut(n)? = entry;
                        e.group_count += 1;
                    }
                }
            }
            _ => {
                e.derived_group = true;
                e.alias = alias;
            }
        }
        e.endpoint = endpoint;
        e.fixed |= options & (1 << 7) != 0;
        e.seq_cap |= options & (1 << 8) != 0;
        e.security = security;
        e.key_type = ((options >> 11) & 7) as u8;
        e.counter = counter.unwrap_or(e.counter);
        e.key = key.unwrap_or(e.key);
        e.radius = radius.unwrap_or(e.radius);
        self.proxies[i] = e;
        self.store_proxy(i);
        if self.gp_mode.is_some_and(|c| c.exit_on_pairing) {
            self.gp_mode = None;
        }
        let aliases = [Some(gpd.alias()), alias];
        if aliases.contains(&Some(self.nib.short)) {
            self.address_conflict(self.nib.short);
        }
        Some(SUCCESS)
    }

    fn gp_global(&mut self, cmd: u8, mut r: Reader, seq: u8, rsp_fc: u8, rsp: &mut Long) -> Option<u8> {
        match cmd {
            0x00 => {
                rsp.bytes(&[rsp_fc, seq, 0x01]);
                while let Some(id) = r.u16() {
                    rsp.u16(id);
                    let at = rsp.len();
                    rsp.u8(SUCCESS);
                    match id {
                        ATTR_MAX_PROXY_TABLE_ENTRIES => rsp.bytes(&[T_U8, PROXIES as u8]),
                        ATTR_PROXY_TABLE => {
                            let mut table = Long::new();
                            for e in self.proxies.iter().filter(|e| e.used) {
                                e.write(&mut table);
                            }
                            if table.ok() && rsp.len() + 3 + table.len() <= SINGLE_MAX {
                                rsp.u8(T_LONG_OCTETS);
                                rsp.u16(table.len() as u16);
                                rsp.bytes(table.as_slice());
                            } else {
                                rsp.set(at, INSUFFICIENT_SPACE);
                            }
                        }
                        ATTR_FUNCTIONALITY | ATTR_ACTIVE_FUNCTIONALITY => {
                            let v = if id == ATTR_FUNCTIONALITY { FUNCTIONALITY } else { ACTIVE_FUNCTIONALITY };
                            rsp.u8(T_BITMAP24);
                            rsp.bytes(&v.to_le_bytes()[..3]);
                        }
                        ATTR_SHARED_KEY_TYPE => rsp.bytes(&[T_BITMAP8, self.gp_keys.shared_type]),
                        ATTR_SHARED_KEY | ATTR_LINK_KEY => {
                            rsp.u8(T_KEY);
                            rsp.bytes(if id == ATTR_SHARED_KEY { &self.gp_keys.shared } else { &self.gp_keys.link });
                        }
                        ATTR_CLUSTER_REVISION => {
                            rsp.u8(T_U16);
                            rsp.u16(CLUSTER_REVISION);
                        }
                        _ => rsp.set(at, UNSUPPORTED_ATTRIBUTE),
                    }
                }
                None
            }
            0x02 | 0x03 | 0x05 => {
                rsp.bytes(&[rsp_fc, seq, 0x04]);
                let mut keys = self.gp_keys;
                let mut failed = false;
                while let Some(id) = r.u16() {
                    let Some(ty) = r.u8() else { break };
                    let status = match (id, ty) {
                        (ATTR_SHARED_KEY_TYPE, T_BITMAP8) => match r.u8() {
                            Some(v) if v <= KEY_NWK_DERIVED_GROUP => {
                                keys.shared_type = v;
                                SUCCESS
                            }
                            Some(_) => INVALID_VALUE,
                            None => INVALID_DATA_TYPE,
                        },
                        (ATTR_SHARED_KEY | ATTR_LINK_KEY, T_KEY) => match r.key() {
                            Some(key) => {
                                if id == ATTR_SHARED_KEY {
                                    keys.shared = key;
                                } else {
                                    keys.link = key;
                                }
                                SUCCESS
                            }
                            None => INVALID_DATA_TYPE,
                        },
                        (ATTR_SHARED_KEY_TYPE | ATTR_SHARED_KEY | ATTR_LINK_KEY, _) => INVALID_DATA_TYPE,
                        (
                            ATTR_MAX_PROXY_TABLE_ENTRIES | ATTR_PROXY_TABLE | ATTR_FUNCTIONALITY | ATTR_ACTIVE_FUNCTIONALITY
                            | ATTR_CLUSTER_REVISION,
                            _,
                        ) => READ_ONLY,
                        _ => UNSUPPORTED_ATTRIBUTE,
                    };
                    if status != SUCCESS {
                        failed = true;
                        rsp.u8(status);
                        rsp.u16(id);
                        if status != INVALID_VALUE {
                            break;
                        }
                    }
                }
                if !failed {
                    rsp.u8(SUCCESS);
                }
                if !(failed && cmd == 0x03) {
                    self.gp_keys = keys;
                    self.store_gp_keys();
                }
                if cmd == 0x05 {
                    rsp.clear();
                }
                None
            }
            _ => Some(UNSUP_COMMAND),
        }
    }

    pub(crate) fn gp_simple_descriptor(&self, rsp: &mut Buf) {
        rsp.u8(10);
        rsp.u8(GP_ENDPOINT);
        rsp.u16(PROFILE_GP);
        rsp.u16(DEVICE_PROXY_BASIC);
        rsp.u8(0);
        rsp.u8(0);
        rsp.u8(1);
        rsp.u16(CLUSTER_GP);
    }

    pub(crate) fn store_proxy(&mut self, i: usize) {
        let e = self.proxies[i];
        let mut b: Buffer<ENTRY_MAX> = Buffer::new();
        b.u8(ENTRY_VERSION);
        if e.used {
            let (ieee, id) = match e.gpd {
                Gpd::Src(id) => (false, id as u64),
                Gpd::Ieee(ieee) => (true, ieee),
            };
            b.u8(pack(&[
                (e.derived_group as u16, 0),
                (e.alias.is_some() as u16, 1),
                (e.seq_cap as u16, 2),
                (e.fixed as u16, 3),
                (ieee as u16, 4),
                (e.in_range as u16, 5),
            ]) as u8);
            b.u64(id);
            b.u8(e.endpoint);
            b.u16(e.alias.unwrap_or(0));
            b.u8(e.radius);
            b.u8(e.security);
            b.u8(e.key_type);
            b.bytes(&e.key);
            b.u32(e.counter);
            b.u8(e.sink_count);
            for (ext, short) in &e.sinks[..e.sink_count as usize] {
                b.u64(*ext);
                b.u16(*short);
            }
            b.u8(e.group_count);
            for (group, alias) in &e.groups[..e.group_count as usize] {
                b.u16(*group);
                b.u16(*alias);
            }
        }
        self.nv_put(NvItem::proxy(i), b.as_slice());
    }

    pub(crate) fn store_gp_keys(&mut self) {
        let mut b: Buffer<40> = Buffer::new();
        b.u8(KEYS_VERSION);
        b.u8(self.gp_keys.shared_type);
        b.bytes(&self.gp_keys.shared);
        b.bytes(&self.gp_keys.link);
        self.nv_put(NvItem::GpKeys, b.as_slice());
    }

    pub(crate) fn load_proxies(&mut self) {
        let mut raw = [0u8; ENTRY_MAX];
        for i in 0..PROXIES {
            let Some(len) = self.hal.nv_load(NvItem::proxy(i), &mut raw) else {
                continue;
            };
            let mut r = Reader::new(&raw[..len.min(raw.len())]);
            let entry = (|| {
                if r.u8()? != ENTRY_VERSION {
                    return None;
                }
                let flags = r.u8()?;
                let id = r.u64()?;
                let mut e = ProxyEntry {
                    used: true,
                    gpd: if flags & 0x10 != 0 { Gpd::Ieee(id) } else { Gpd::Src(id as u32) },
                    endpoint: r.u8()?,
                    derived_group: flags & 0x01 != 0,
                    seq_cap: flags & 0x04 != 0,
                    fixed: flags & 0x08 != 0,
                    in_range: flags & 0x20 != 0,
                    ..ProxyEntry::EMPTY
                };
                let alias = r.u16()?;
                e.alias = (flags & 0x02 != 0).then_some(alias);
                e.radius = r.u8()?;
                e.security = r.u8()?;
                e.key_type = r.u8()?;
                e.key = r.key()?;
                e.counter = r.u32()?;
                e.sink_count = r.u8()?.min(SINKS as u8);
                for s in e.sinks.iter_mut().take(e.sink_count as usize) {
                    *s = (r.u64()?, r.u16()?);
                }
                e.group_count = r.u8()?.min(SINK_GROUPS as u8);
                for g in e.groups.iter_mut().take(e.group_count as usize) {
                    *g = (r.u16()?, r.u16()?);
                }
                Some(e)
            })();
            if let Some(e) = entry {
                self.proxies[i] = e;
            }
        }
        if let Some(len) = self.hal.nv_load(NvItem::GpKeys, &mut raw) {
            let mut r = Reader::new(&raw[..len.min(raw.len())]);
            if let (Some(KEYS_VERSION), Some(shared_type), Some(shared), Some(link)) = (r.u8(), r.u8(), r.key(), r.key()) {
                self.gp_keys = Keys { shared_type, shared, link };
            }
        }
    }
}

#[cfg(test)]
mod tests;
