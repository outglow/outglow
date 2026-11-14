//! The Telink light-mesh profile as AwoX's app uses it with one bulb.
//!
//! - pair characteristic: `0x0c rnd8 enc8` logs in and gives `0x0d rnd8`
//!   and a session key, or `0x0e` for a wrong password; `0x04/0x05/0x06`
//!   set a new network name, password and long-term key (`0x07` when all
//!   three arrived)
//! - command characteristic, three formats: 20 bytes `sn3 chk2 payload15`
//!   in counter mode with the session key, payload `dst op 60 01 params`
//!   (python-awox-mesh-light); a type byte and one AES block
//!   `crc8 len payload` (the stock firmware 3.0.2 and the AwoX HomeControl
//!   app), whose payload is either `op 60 01 params` or a ZCL frame the app
//!   tunnels: `addr 01 cluster command...`
//! - status characteristic: after `0x01` is written, a notification `0xdc`
//!   on every change of the light and `0xdb` in answer to a status request;
//!   to a central that speaks the app's format (the type byte and one AES
//!   block) instead the app's own status, `0x80` and one AES block, in
//!   answer to its status request and after every command (`app_status`,
//!   docs/stock-colour.md: what the stock firmware sends and the app reads)
//!
//! The factory network is "unpaired" with the password
//! sha256(address, secret, 00 00)[..16] (stock firmware 3.0 and later) or
//! "1234" (older apps); both are accepted until an app sets its own.

use bulb_core::command::{ColorMode, Command, HueDirection};

use crate::crypto::{crc8_6c, mesh_crypt, mesh_mic, tl_aes_decrypt, tl_aes_encrypt};
use crate::hal::{Hal, TICKS_PER_MS};
use crate::sha256::sha256_short;
use crate::{reached, Ble, Light};

const AWOX_SECRET: [u8; 8] = [0x56, 0xe8, 0x50, 0xed, 0xcf, 0xdd, 0x8b, 0x99];
const DEFAULT_NAME: &[u8] = b"unpaired";
const DEFAULT_PASSWORD: &[u8] = b"1234";
/// The network key before pairing: the default of Telink's light-mesh SDK.
const DEFAULT_LTK: [u8; 16] = [0xc0, 0xc1, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7, 0xd8, 0xd9, 0xda, 0xdb, 0xdc, 0xdd, 0xde, 0xdf];
const VENDOR: [u8; 2] = [0x60, 0x01];

const OP_POWER: u8 = 0xd0;
const OP_STATUS_REQUEST: u8 = 0xda;
const OP_STATUS_RESPONSE: u8 = 0xdb;
const OP_NOTIFY: u8 = 0xdc;
const OP_ADDRESS: u8 = 0xe0;
const OP_RESET: u8 = 0xe3;
const OP_TEMPERATURE: u8 = 0xf0;
const OP_BRIGHTNESS: u8 = 0xf1;

const NV_MAGIC: u8 = 0xa7;
const NV_LEN: usize = 52;
const POLL: u32 = 250 * TICKS_PER_MS;
const RESET_DISCONNECT: u32 = 300 * TICKS_PER_MS;
const PENDING: usize = 4;

fn padded(s: &[u8]) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[..s.len().min(16)].copy_from_slice(&s[..s.len().min(16)]);
    out
}

pub(crate) struct Mesh {
    name: [u8; 16],
    password: [u8; 16],
    pub ltk: [u8; 16],
    address: u16,
    paired: bool,
    secure_password: [u8; 16],
    address_low: u8,
    session: Option<[u8; 16]>,
    status_on: bool,
    pending_fields: [[u8; 16]; 3],
    pending_mask: u8,
    notify_sn: u32,
    last_sn: [u8; 3],
    status_due: bool,
    /// The central speaks the AwoX HomeControl app's format: it is told the
    /// state in the app's status (`app_status`), not in the mesh's own.
    app_format: bool,
    reported: Option<Light>,
    /// Notifications the transmit queue had no room for.
    unsent: [Option<[u8; 20]>; PENDING],
    next_poll: u32,
    disconnect_at: Option<u32>,
    pub att_status: [u8; 20],
    pub att_command: [u8; 20],
    pub att_pair: [u8; 20],
}

impl Mesh {
    pub fn new(addr: &[u8; 6], now: u32) -> Mesh {
        // the factory password: SHA-256 of the address as it is printed, the secret and two zeros
        let mut m = [0u8; 16];
        for (i, b) in m[..6].iter_mut().enumerate() {
            *b = addr[5 - i];
        }
        m[6..14].copy_from_slice(&AWOX_SECRET);
        let hash = sha256_short(&m);
        let mut mesh = Mesh {
            name: [0; 16],
            password: [0; 16],
            ltk: [0; 16],
            address: 0,
            paired: false,
            secure_password: padded(&hash[..16]),
            address_low: addr[0],
            session: None,
            status_on: false,
            pending_fields: [[0; 16]; 3],
            pending_mask: 0,
            notify_sn: 0x100,
            last_sn: [0; 3],
            status_due: false,
            app_format: false,
            reported: None,
            unsent: [None; PENDING],
            next_poll: now.wrapping_add(POLL),
            disconnect_at: None,
            att_status: [0; 20],
            att_command: [0; 20],
            att_pair: [0; 20],
        };
        mesh.defaults();
        mesh
    }

    fn defaults(&mut self) {
        self.name = padded(DEFAULT_NAME);
        self.password = padded(DEFAULT_PASSWORD);
        self.ltk = DEFAULT_LTK;
        self.address = 0;
        self.paired = false;
    }

    /// The network name without its padding.
    pub fn name(&self) -> &[u8] {
        let n = self.name.iter().position(|b| *b == 0).unwrap_or(16);
        &self.name[..n]
    }

    pub fn name_padded(&self) -> &[u8; 16] {
        &self.name
    }

    pub fn password_padded(&self) -> &[u8; 16] {
        &self.password
    }

    /// The bulb's mesh address: what the app assigned, else the low byte of its address (never 0).
    pub fn device_address(&self) -> u16 {
        match (self.address, self.address_low) {
            (0, 0) => 1,
            (0, low) => low as u16,
            (a, _) => a,
        }
    }

    /// Whether an app has set this bulb's network (it is not the factory one).
    pub fn is_paired(&self) -> bool {
        self.paired
    }

    pub fn logged_in(&self) -> bool {
        self.session.is_some()
    }

    fn encode(&self) -> [u8; NV_LEN] {
        let mut b = [0u8; NV_LEN];
        b[0] = NV_MAGIC;
        b[1..17].copy_from_slice(&self.name);
        b[17..33].copy_from_slice(&self.password);
        b[33..49].copy_from_slice(&self.ltk);
        b[49..51].copy_from_slice(&self.address.to_le_bytes());
        b[51] = self.paired as u8;
        b
    }

    fn decode(&mut self, b: &[u8]) -> bool {
        if b.len() != NV_LEN || b[0] != NV_MAGIC {
            return false;
        }
        self.name.copy_from_slice(&b[1..17]);
        self.password.copy_from_slice(&b[17..33]);
        self.ltk.copy_from_slice(&b[33..49]);
        self.address = u16::from_le_bytes([b[49], b[50]]);
        self.paired = b[51] != 0;
        true
    }

    /// Checks a login attempt against one password; the name-and-password
    /// block on success.
    fn login_block(&self, password: &[u8; 16], random: &[u8], proof: &[u8]) -> Option<[u8; 16]> {
        let mut block = [0u8; 16];
        for (i, b) in block.iter_mut().enumerate() {
            *b = self.name[i] ^ password[i];
        }
        let enc = tl_aes_encrypt(&padded(random), &block);
        (enc[..8] == *proof).then_some(block)
    }
}

/// Brightness on the mesh scale: level 1..=254 as 1..=127.
fn white_brightness(level: u8) -> u8 {
    ((level as u16 + 1) / 2).max(1) as u8
}

impl<H: Hal> Ble<H> {
    pub(crate) fn mesh_load(&mut self) {
        let mut b = [0u8; NV_LEN];
        let n = self.hal.nv_load(&mut b).unwrap_or(0);
        if !self.mesh.decode(&b[..n.min(NV_LEN)]) {
            self.mesh.defaults();
        }
        self.air_configure();
    }

    fn mesh_save(&mut self) {
        let b = self.mesh.encode();
        self.hal.nv_store(&b);
        self.air_configure();
    }

    pub(crate) fn mesh_session_end(&mut self) {
        let m = &mut self.mesh;
        m.session = None;
        m.status_on = false;
        m.status_due = false;
        m.app_format = false;
        m.pending_mask = 0;
        m.unsent = [None; PENDING];
        m.disconnect_at = None;
    }

    /// Colour temperature on the mesh scale: 0 (cool) ..= 127 (warm).
    fn white_temperature(&self) -> u8 {
        if !self.identity.tunable {
            return 64;
        }
        let (lo, hi) = (self.identity.ct_min, self.identity.ct_max);
        let m = self.light.mireds.clamp(lo, hi);
        let span = (hi - lo).max(1) as u32;
        (((m - lo) as u32 * 127 + span / 2) / span) as u8
    }

    /// Mode, brightness and temperature, as status packets carry them.
    fn status_triple(&self) -> [u8; 3] {
        [self.light.on as u8, white_brightness(self.light.level), self.white_temperature()]
    }

    /// The parameters of a status answer on air.
    pub(crate) fn mesh_status_params(&self) -> [u8; 10] {
        let mut p = [0u8; 10];
        p[..3].copy_from_slice(&self.status_triple());
        p
    }

    /// Another bulb's answer from the air, for the app.
    pub(crate) fn mesh_notify_from_air(&mut self, sn: [u8; 3], src: u16, payload: &[u8; 13]) {
        // the app takes the first octet of a notification for its type: a
        // mesh packet would read as anything (0x83: "Zigbee provisioning done")
        if self.mesh.app_format {
            return;
        }
        if !self.mesh_notify(sn, src, payload) {
            let packet = self.mesh.att_status;
            if let Some(slot) = self.mesh.unsent.iter_mut().find(|s| s.is_none()) {
                *slot = Some(packet);
            }
        }
    }

    /// Encrypts and sends a status packet: `sn3 src2 chk2` and 13 bytes of
    /// payload under the session key. False when it has to be sent again.
    fn mesh_notify(&mut self, sn: [u8; 3], src: u16, payload: &[u8; 13]) -> bool {
        let Some(key) = self.mesh.session else { return true };
        // the app's switch (a write of 1 to the status value, as on the
        // stock) or the standard one (the client configuration descriptor)
        if !self.mesh_status_enabled() {
            return true;
        }
        let mut packet = [0u8; 20];
        packet[..3].copy_from_slice(&sn);
        packet[3..5].copy_from_slice(&src.to_le_bytes());
        packet[7..].copy_from_slice(payload);
        let mut nonce = [0u8; 8];
        nonce[..3].copy_from_slice(&self.addr[..3]);
        nonce[3..].copy_from_slice(&packet[..5]);
        let mic = mesh_mic(&key, &nonce, &packet[7..]);
        packet[5..7].copy_from_slice(&mic[..2]);
        mesh_crypt(&key, &nonce, &mut packet[7..]);
        self.mesh.att_status = packet;
        self.notify_status(&packet)
    }

    /// The status the AwoX HomeControl app reads (its transport for these
    /// bulbs, g9.j / n7.k in the app), as the stock firmware of every
    /// TLSR8258 lamp sends it: `0x80`, then 16 octets in one AES block of the
    /// session key, then 3 in the clear; the 19 are
    /// `address(le) 01 00 00 product(le) mac[0..4] 01 flags level
    /// mireds(le) hue saturation 20`. Flags: bit 0 on, bit 1 the hue and
    /// saturation show, bit 2 the hue is turning. Mireds read 0xFFFF with no
    /// colour temperature showing (and on a bulb without one), hue and
    /// saturation 0xFF without them.
    pub(crate) fn app_status(&self, key: &[u8; 16]) -> [u8; 20] {
        let l = &self.light;
        let hs = self.identity.colour && l.mode == ColorMode::HueSaturation;
        let ct = self.identity.tunable && (!self.identity.colour || l.mode == ColorMode::Temperature);
        let mut d = [0u8; 19];
        // the address the app gave it (0 before pairing, as the stock), not the one the mesh falls back on
        d[..2].copy_from_slice(&self.mesh.address.to_le_bytes());
        d[2] = 0x01;
        d[5..7].copy_from_slice(&self.identity.product.to_le_bytes());
        d[7..11].copy_from_slice(&self.addr[..4]);
        d[11] = 0x01;
        d[12] = l.on as u8 | (hs as u8) << 1 | ((hs && l.moving) as u8) << 2;
        d[13] = l.level;
        d[14..16].copy_from_slice(&if ct { l.mireds } else { 0xFFFF }.to_le_bytes());
        (d[16], d[17]) = if hs { (l.hue, l.sat) } else { (0xFF, 0xFF) };
        // 0x20 in every stock image (its meaning not known)
        d[18] = 0x20;
        let mut v = [0u8; 20];
        v[0] = 0x80;
        let mut block = [0u8; 16];
        block.copy_from_slice(&d[..16]);
        v[1..17].copy_from_slice(&tl_aes_encrypt(key, &block));
        v[17..].copy_from_slice(&d[16..]);
        v
    }

    fn mesh_send_status(&mut self, op: u8) -> bool {
        if self.mesh.app_format {
            let Some(key) = self.mesh.session else { return true };
            if !self.mesh_status_enabled() {
                return true;
            }
            let v = self.app_status(&key);
            self.mesh.att_status = v;
            return self.notify_status(&v);
        }
        let address = self.mesh.device_address();
        self.mesh.notify_sn = self.mesh.notify_sn.wrapping_add(1);
        let sn = self.mesh.notify_sn.to_le_bytes();
        let status = self.status_triple();
        let mut p = [0u8; 13];
        p[0] = op;
        p[1..3].copy_from_slice(&VENDOR);
        if op == OP_NOTIFY {
            p[3] = address as u8;
            p[5..8].copy_from_slice(&status);
            p[12] = (address >> 8) as u8;
        } else {
            p[3..6].copy_from_slice(&status);
        }
        self.mesh_notify([sn[0], sn[1], sn[2]], address, &p)
    }

    /// Sends a status packet now, or keeps it for later when the queue is full.
    fn mesh_status_or_keep(&mut self, op: u8) {
        if !self.mesh_send_status(op) {
            let packet = self.mesh.att_status;
            if let Some(slot) = self.mesh.unsent.iter_mut().find(|s| s.is_none()) {
                *slot = Some(packet);
            }
        }
    }

    pub(crate) fn mesh_timers(&mut self, now: u32) {
        if let Some(at) = self.mesh.disconnect_at {
            if reached(now, at) {
                self.mesh.disconnect_at = None;
                self.terminate();
            }
        }
        if !reached(now, self.mesh.next_poll) {
            return;
        }
        self.mesh.next_poll = now.wrapping_add(POLL);
        self.mesh_light_changed();
    }

    /// Reports the light's state when it changed, and sends what could not be sent before.
    fn mesh_light_changed(&mut self) {
        if !self.mesh.logged_in() {
            self.mesh.reported = Some(self.light);
            return;
        }
        for i in 0..PENDING {
            if let Some(packet) = self.mesh.unsent[i] {
                if self.notify_status(&packet) {
                    self.mesh.unsent[i] = None;
                }
            }
        }
        // a turning hue is not news: told when it starts and when it stops (as the stock firmware)
        let news = match self.mesh.reported {
            Some(r) if r.moving && self.light.moving => Light { hue: r.hue, ..self.light } != r,
            r => r != Some(self.light),
        };
        if news || self.mesh.status_due {
            self.mesh.reported = Some(self.light);
            self.mesh.status_due = !self.mesh_send_status(OP_NOTIFY);
        }
    }

    /// Whether status notifications are wanted: the app's switch (a write
    /// of 1 to the status value, as on the stock) or the standard one (the
    /// client configuration descriptor).
    pub(crate) fn mesh_status_enabled(&self) -> bool {
        self.mesh.status_on || self.host.notifications_enabled()
    }

    /// One of the two switches was written; `was`: the state before.
    pub(crate) fn mesh_status_switched(&mut self, was: bool) {
        let now = self.mesh_status_enabled();
        if now != was {
            // the first notification goes out with the next poll
            self.mesh.status_due = now;
        }
    }

    pub(crate) fn mesh_status_write(&mut self, value: &[u8]) {
        if let Some(&v) = value.first() {
            let was = self.mesh_status_enabled();
            self.mesh.status_on = v != 0;
            self.mesh_status_switched(was);
        }
    }

    fn set_level(&mut self, level: u8, transition_ms: u32) {
        let level = level.clamp(1, 254);
        // a brightness command does not turn the light on
        self.push_command(if self.light.on { Command::MoveToLevel { level, transition_ms } } else { Command::SetLevelWhileOff(level) });
    }

    fn factory_mesh(&mut self, from_air: bool) {
        self.mesh.defaults();
        self.mesh_save();
        if !from_air {
            self.mesh.disconnect_at = Some(self.hal.now().wrapping_add(RESET_DISCONNECT));
        }
    }

    /// A command of the mesh's own vocabulary (vendor 0x0160), from the app
    /// or from another bulb.
    pub(crate) fn mesh_command(&mut self, op: u8, p: &[u8], from_air: bool) {
        match (op, p.first()) {
            (OP_POWER, Some(&v)) if v != 0xee => self.push_command(if v != 0 { Command::On } else { Command::Off }),
            (OP_BRIGHTNESS, Some(&v)) => self.set_level((v as u16 * 2).min(254) as u8, 0),
            (OP_TEMPERATURE, Some(&v)) if self.identity.tunable => {
                let (lo, hi) = (self.identity.ct_min, self.identity.ct_max);
                let mireds = lo + ((hi - lo) as u32 * v.min(127) as u32 / 127) as u16;
                self.push_command(Command::MoveToColorTemperature { mireds, transition_ms: 0 });
            }
            // on air the answer goes back to the asking bulb instead
            (OP_STATUS_REQUEST, _) if !from_air => self.mesh_status_or_keep(OP_STATUS_RESPONSE),
            (OP_ADDRESS, _) if p.len() >= 2 => {
                self.mesh.address = u16::from_le_bytes([p[0], p[1]]);
                self.mesh_save();
            }
            (OP_RESET, _) => self.factory_mesh(from_air),
            // modes, groups, time, alarms, colour: nothing here to map them to
            _ => {}
        }
    }

    /// A ZCL frame tunnelled by the AwoX HomeControl app:
    /// `addr (big-endian) 01 cluster (little-endian) command...`.
    fn zcl_tunnel(&mut self, kind: u8, p: &[u8]) {
        if p.len() < 5 || p[2] != 0x01 {
            return;
        }
        let address = u16::from_be_bytes([p[0], p[1]]);
        let me = self.mesh.device_address();
        let for_us = match kind {
            // groups other than "everyone" are not kept
            1 => address == 0xffff,
            _ => address == 0 || address == 0xffff || address & 0x7fff == me,
        };
        if !for_us {
            return;
        }
        let cluster = u16::from_le_bytes([p[3], p[4]]);
        let a = &p[5..];
        let tenths = |i: usize| if a.len() >= i + 2 { u16::from_le_bytes([a[i], a[i + 1]]) as u32 * 100 } else { 0 };
        match cluster {
            0x0006 if !a.is_empty() => match a[0] {
                0x00 => self.push_command(Command::Off),
                0x01 => self.push_command(Command::On),
                0x02 => self.push_command(Command::Toggle),
                _ => {}
            },
            // Move to Level (with On/Off); the app's candle mode (0x10) is not here
            0x0008 if a.len() >= 2 && (a[0] == 0x00 || a[0] == 0x04) => self.set_level(a[1], tenths(2)),
            0x0300 if a.len() >= 3 && a[0] == 0x0a && self.identity.tunable => {
                let mireds = u16::from_le_bytes([a[1], a[2]]);
                self.push_command(Command::MoveToColorTemperature { mireds, transition_ms: tenths(3) });
            }
            // a colour bulb: the colour commands of Color Control, as the stock
            // firmware hands the tunnel to its ZCL
            0x0300 if self.identity.colour && !a.is_empty() => {
                let hs = |hue: u8, sat: u8, direction: HueDirection, transition_ms: u32| {
                    Command::MoveToHueSaturation { hue: hue.min(254), sat: sat.min(254), direction, transition_ms }
                };
                let c = match a[0] {
                    // Move to Hue: hue, direction, time
                    0x00 if a.len() >= 3 => HueDirection::from_u8(a[2]).map(|d| hs(a[1], self.light.sat, d, tenths(3))),
                    // Move to Saturation: saturation, time
                    0x03 if a.len() >= 2 => Some(hs(self.light.hue, a[1], HueDirection::Shortest, tenths(2))),
                    // Move to Hue and Saturation: hue, saturation, time
                    0x06 if a.len() >= 3 => Some(hs(a[1], a[2], HueDirection::Shortest, tenths(3))),
                    // Move to Color: x, y, time
                    0x07 if a.len() >= 5 => Some(Command::MoveToXy {
                        x: u16::from_le_bytes([a[1], a[2]]),
                        y: u16::from_le_bytes([a[3], a[4]]),
                        transition_ms: tenths(5),
                    }),
                    // Enhanced Move Hue: mode (0 stop, 1 up, 3 down), rate in enhanced hue
                    // steps a second; the app's colour animation is up at 0x0C00
                    0x41 if a.len() >= 4 => match a[1] {
                        0 => Some(Command::StopColorTransition),
                        1 | 3 => Some(Command::MoveHue { up: a[1] == 1, rate: u16::from_le_bytes([a[2], a[3]]) }),
                        _ => None,
                    },
                    // Stop Move Step: the app ends the animation with it
                    0x47 => Some(Command::StopColorTransition),
                    _ => None,
                };
                if let Some(c) = c {
                    self.push_command(c);
                }
            }
            _ => {}
        }
    }

    /// The stock firmware's command format: a type byte and one AES block
    /// `crc8 len payload` under the session key. True when `value` was one.
    fn stock_command(&mut self, key: &[u8; 16], value: &[u8]) -> bool {
        if value.len() < 17 {
            return false;
        }
        let mut block = [0u8; 16];
        block.copy_from_slice(&value[1..17]);
        let plain = tl_aes_decrypt(key, &block);
        let len = plain[1] as usize;
        if !(1..=14).contains(&len) || crc8_6c(&plain[1..2 + len]) != plain[0] {
            return false;
        }
        self.mesh.att_command = [0; 20];
        self.mesh.att_command[..16].copy_from_slice(&plain);
        let payload = &plain[2..2 + len];
        match value[0] {
            0 | 1 => {
                // the stock firmware answers every command with its status
                self.mesh.app_format = true;
                self.mesh.status_due = true;
                self.zcl_tunnel(value[0], payload)
            }
            2 if len >= 3 && payload[1..3] == VENDOR => self.mesh_command(payload[0], &payload[3..], false),
            // the app's own status request: answered with the state as it is
            // now (a turning hue where it has got to) once the light reports it
            2 if payload[0] == 0x0a => {
                self.mesh.app_format = true;
                self.mesh.status_due = true;
                self.push_command(Command::ReadState);
            }
            // and its "kick" (leave the network)
            2 if payload[0] == 0x01 => self.factory_mesh(false),
            2 => {}
            _ => return false,
        }
        true
    }

    pub(crate) fn mesh_command_write(&mut self, value: &[u8]) {
        let Some(key) = self.mesh.session else { return };
        if self.stock_command(&key, value) || value.len() < 20 {
            return;
        }
        let mut packet = [0u8; 20];
        packet.copy_from_slice(&value[..20]);
        let mut nonce = [0u8; 8];
        nonce[..4].copy_from_slice(&self.addr[..4]);
        nonce[4] = 0x01;
        nonce[5..].copy_from_slice(&packet[..3]);
        mesh_crypt(&key, &nonce, &mut packet[5..]);
        let mic = mesh_mic(&key, &nonce, &packet[5..]);
        if mic[..2] != packet[3..5] {
            // wrong key or a damaged packet
            return;
        }
        if packet[..3] == self.mesh.last_sn {
            return;
        }
        self.mesh.last_sn.copy_from_slice(&packet[..3]);
        let payload = &packet[5..];
        self.mesh.att_command = [0; 20];
        self.mesh.att_command[..15].copy_from_slice(payload);
        // bit 15 of the destination: a group (0xffff everyone); otherwise a
        // device, 0 being the bulb the app is connected to
        let dst = u16::from_le_bytes([payload[0], payload[1]]);
        let to_me = dst & 0x8000 == 0 && (dst == 0 || dst == self.mesh.device_address());
        let op = payload[2];
        let mut params = [0u8; 10];
        params.copy_from_slice(&payload[5..]);
        let sn = self.mesh.last_sn;
        if to_me || dst == 0xffff {
            self.mesh_command(op, &params, false);
        }
        // what is not for this bulb alone goes on to the others
        if dst != 0 && !to_me {
            self.air_bridge(&sn, dst, op, &params);
        }
    }

    pub(crate) fn mesh_pair_write(&mut self, value: &[u8]) {
        let Some(&op) = value.first() else { return };
        if op == 0x0c && value.len() >= 17 {
            let (random, proof) = (&value[1..9], &value[9..17]);
            let m = &self.mesh;
            let block = if m.paired {
                m.login_block(&m.password, random, proof)
            } else {
                m.login_block(&m.secure_password, random, proof).or_else(|| m.login_block(&padded(DEFAULT_PASSWORD), random, proof))
            };
            self.mesh.att_pair = [0; 20];
            let Some(block) = block else {
                self.mesh.session = None;
                self.mesh.att_pair[0] = 0x0e;
                return;
            };
            let mut ours = [0u8; 8];
            ours[..4].copy_from_slice(&self.hal.random().to_le_bytes());
            ours[4..].copy_from_slice(&self.hal.random().to_le_bytes());
            let mut both = [0u8; 16];
            both[..8].copy_from_slice(random);
            both[8..].copy_from_slice(&ours);
            self.mesh.session = Some(tl_aes_encrypt(&block, &both));
            self.mesh.pending_mask = 0;
            self.mesh.att_pair[0] = 0x0d;
            self.mesh.att_pair[1..9].copy_from_slice(&ours);
            // bytes 9..17: the two random numbers the other way round,
            // encrypted; clients ignore them, the stock fills them too
            both[..8].copy_from_slice(&ours);
            both[8..].copy_from_slice(random);
            let e = tl_aes_encrypt(&block, &both);
            self.mesh.att_pair[9..17].copy_from_slice(&e[..8]);
            return;
        }
        if (0x04..=0x06).contains(&op) && value.len() >= 17 {
            let Some(key) = self.mesh.session else { return };
            let mut block = [0u8; 16];
            block.copy_from_slice(&value[1..17]);
            let field = (op - 0x04) as usize;
            self.mesh.pending_fields[field] = tl_aes_decrypt(&key, &block);
            self.mesh.pending_mask |= 1 << field;
            if self.mesh.pending_mask == 0x07 {
                let m = &mut self.mesh;
                m.name = m.pending_fields[0];
                m.password = m.pending_fields[1];
                m.ltk = m.pending_fields[2];
                m.paired = true;
                m.pending_mask = 0;
                m.att_pair = [0; 20];
                m.att_pair[0] = 0x07;
                self.mesh_save();
            }
        }
    }
}
