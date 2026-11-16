//! The firmware staging area: where the stock firmware's BLE update puts a
//! new image and the resident bootloader looks for one
//! (docs/ble-first-flash.md). Both update paths of this firmware, Zigbee
//! and BLE, write here, one at a time, and both hand over to the
//! bootloader through `activate`, which checks what the flash really holds.
//! The rules are `bulb_core::staging`'s (tested on the host:
//! crates/bulb-sim/tests/staging.rs).

use bulb_core::staging::{self as rules, Ownership};

pub use bulb_core::staging::Owner;

use crate::board::{self, BoardFlash};
use crate::cell::Shared;

pub const BASE: usize = 0x6000;
pub const SIZE: u32 = 0x6_4000;

static OWNER: Shared<Ownership> = Shared::new(Ownership::FREE);

/// Takes the staging area for one update path. False while the other has it.
pub fn claim(who: Owner) -> bool {
    let mut o = OWNER.get();
    let ok = o.claim(who);
    OWNER.set(o);
    ok
}

pub fn holds(who: Owner) -> bool {
    OWNER.get().holds(who)
}

/// Gives the staging area up (the transfer ended or was abandoned).
pub fn release(who: Owner) {
    OWNER.update(|mut o| {
        o.release(who);
        o
    });
}

/// Marks the staged image bootable and restarts into the bootloader, if
/// the flash holds a good image. Returns (false) only when it does not.
pub fn activate() -> bool {
    if !rules::mark_bootable(&mut BoardFlash, BASE, SIZE, board::watchdog_feed) {
        return false;
    }
    board::reboot()
}
