#![forbid(unsafe_code)]
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

#[derive(Debug, Clone, Copy)]
pub struct Identity {
    pub product: u16,
    pub model: &'static str,
    pub firmware_revision: &'static str,
    pub hardware_revision: &'static str,
    pub tunable: bool,
    pub ct_min: u16,
    pub ct_max: u16,
    pub colour: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub adv_events: u32,
    pub connections: u32,
    pub events: u32,
    pub events_heard: u32,
    pub events_skipped: u32,
    pub disconnects: u32,
    pub links_lost: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Light {
    pub on: bool,
    pub level: u8,
    pub mireds: u16,
    pub hue: u8,
    pub sat: u8,
    pub mode: bulb_core::command::ColorMode,
    pub moving: bool,
    pub candle: bool,
    pub shown: bulb_core::command::Shown,
}

impl Light {
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
            self.mesh.status_due = true;
            self.mesh.next_poll = self.hal.now();
        }
    }

    pub fn link_state(&self) -> LinkState {
        self.link.state
    }

    pub fn ota_active(&self) -> bool {
        self.ota.active()
    }

    pub fn idle_time(&mut self) -> u32 {
        let now = self.hal.now();
        self.link_idle_time(now)
    }

    pub fn poll(&mut self) -> Option<Command> {
        self.link_poll();
        if self.link.state == LinkState::Connected {
            self.host_poll();
        }
        let now = self.hal.now();
        self.mesh_timers(now);
        self.ota_timers(now);
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

pub(crate) fn reached(now: u32, t: u32) -> bool {
    now.wrapping_sub(t) as i32 >= 0
}
