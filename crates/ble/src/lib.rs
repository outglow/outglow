//! Bluetooth LE for the bulb: a peripheral that looks like the stock
//! AwoX/EGLO Connect.Z bulb to the AwoX app and to our updater.
//!
//! - link layer (`ll`): advertising, one connection as peripheral, the
//!   control procedures a central starts; the radio runs whole events
//!   (`hal::Hal`), this crate decides when and on which channel
//! - L2CAP and the attribute server with the stock's handle table (`att`)
//! - the Telink mesh profile in AwoX's variant: login, pairing, light
//!   commands, status notifications (`mesh`)
//! - firmware update into the staging area (`ota`)
//! - the diagnostic service of the device page (`diag`), after the stock's
//!   handles; the firmware fills its values and carries out its commands
//!
//! The light itself is `bulb-core`: commands for it come out of `poll`,
//! its state comes in through `set_light`.
#![no_std]

pub mod crypto;
pub mod diag;
pub mod hal;
pub mod pdu;
pub mod sha256;

mod air;
mod att;
mod ll;
mod mesh;
mod ota;

use bulb_core::command::{Command, StateReport};

pub use air::AirStats;
pub use diag::DiagCommand;
pub use hal::Hal;
pub use ll::LinkState;

/// What the bulb says it is over BLE.
#[derive(Debug, Clone, Copy)]
pub struct Identity {
    /// AwoX product number in the advertising data (0x20A7 dimmable, 0x20A6 tunable white).
    pub product: u16,
    /// Model Number String of the Device Information service.
    pub model: &'static str,
    pub firmware_revision: &'static str,
    pub hardware_revision: &'static str,
    /// Whether the bulb has a colour temperature.
    pub tunable: bool,
    /// Its colour temperature range in mireds: the app's 0 (cool) ..= 127
    /// (warm) spans it.
    pub ct_min: u16,
    pub ct_max: u16,
    /// A colour bulb: the app's colour commands (Color Control in the ZCL
    /// tunnel) are taken.
    pub colour: bool,
}

/// Counters for tests and diagnostics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub adv_events: u32,
    pub connections: u32,
    /// Connection events run, and those of them in which the central was heard.
    pub events: u32,
    pub events_heard: u32,
    /// Connection events that passed before the radio could be taken.
    pub events_skipped: u32,
    pub disconnects: u32,
    /// Disconnections because the central was not heard any more (the link
    /// supervision ran out), not because either side ended the connection.
    pub links_lost: u32,
}

/// The light's state as the mesh protocol reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Light {
    pub on: bool,
    pub level: u8,
    pub mireds: u16,
    pub hue: u8,
    pub sat: u8,
    pub mode: bulb_core::command::ColorMode,
    /// The hue is turning (the app's colour animation).
    pub moving: bool,
    pub candle: bool,
    /// What the light shows now (in a transition, where it has got to).
    pub shown: bulb_core::command::Shown,
}

impl Light {
    /// The state without what only the app's status tells (where a
    /// transition has got to): what the mesh's own notifications follow.
    pub fn targets(&self) -> Light {
        Light { shown: Default::default(), ..*self }
    }
}

pub(crate) const CMDS: usize = 4;

pub struct Ble<H: Hal> {
    pub(crate) hal: H,
    pub(crate) identity: Identity,
    pub(crate) addr: [u8; 6],
    pub(crate) link: ll::Link,
    pub(crate) host: att::Host,
    pub(crate) mesh: mesh::Mesh,
    pub(crate) ota: ota::Ota,
    /// The central of this connection has written to the OTA channel.
    pub(crate) ota_this_link: bool,
    pub(crate) air: air::Air,
    pub(crate) light: Light,
    pub(crate) cmds: [Option<Command>; CMDS],
    pub(crate) diag: diag::Diag,
    pub stats: Stats,
}

impl<H: Hal> Ble<H> {
    pub fn new(mut hal: H, identity: Identity) -> Self {
        let addr = hal.address();
        let now = hal.now();
        let mut ble = Ble {
            hal,
            identity,
            addr,
            link: ll::Link::new(now),
            host: att::Host::new(),
            mesh: mesh::Mesh::new(&addr, now),
            ota: ota::Ota::new(),
            ota_this_link: false,
            air: air::Air::new(),
            light: Light {
                on: true,
                level: 254,
                mireds: 0,
                hue: 170,
                sat: 254,
                mode: bulb_core::command::ColorMode::Temperature,
                moving: false,
                candle: false,
                shown: bulb_core::command::Shown { level: 254, mireds: 0, hue: 170, sat: 254 },
            },
            cmds: [None; CMDS],
            diag: diag::Diag::new(),
            stats: Stats::default(),
        };
        ble.mesh_load();
        ble
    }

    pub fn hal(&mut self) -> &mut H {
        &mut self.hal
    }

    /// The light's state, for status notifications. Call after every change.
    pub fn set_light(&mut self, s: &StateReport) {
        self.light = Light {
            on: s.on,
            level: s.level,
            mireds: s.color_temp,
            hue: s.hue,
            sat: s.sat,
            mode: s.color_mode,
            moving: s.hue_moving,
            candle: s.candle,
            shown: s.shown,
        };
        if core::mem::take(&mut self.mesh.app_read) {
            // the state the app's status was waiting for: sent at once
            self.mesh.status_due = true;
            self.mesh.next_poll = self.hal.now();
        }
    }

    pub fn link_state(&self) -> LinkState {
        self.link.state
    }

    /// Whether a firmware transfer over BLE is in progress.
    pub fn ota_active(&self) -> bool {
        self.ota.active()
    }

    /// Runs the BLE side: an advertising or connection event when one is
    /// due, then received packets and timers. Returns a command for the
    /// light when the app sent one.
    pub fn poll(&mut self) -> Option<Command> {
        self.link_poll();
        if self.link.state == LinkState::Connected {
            self.host_poll();
        }
        let now = self.hal.now();
        self.mesh_timers(now);
        self.ota_timers(now);
        // nothing else on the radio while an image is coming in
        if !self.ota.active() {
            self.air_poll();
        }
        self.take_command()
    }

    pub(crate) fn push_command(&mut self, c: Command) {
        if let Some(slot) = self.cmds.iter_mut().find(|s| s.is_none()) {
            *slot = Some(c);
        }
    }

    fn take_command(&mut self) -> Option<Command> {
        let c = self.cmds[0].take();
        self.cmds.rotate_left(1);
        c
    }
}

/// Whether `now` has reached `t` on the wrapping radio clock.
pub(crate) fn reached(now: u32, t: u32) -> bool {
    now.wrapping_sub(t) as i32 >= 0
}
