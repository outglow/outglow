use bulb_core::staging::{self as rules, Ownership};

pub use bulb_core::staging::Owner;

use crate::board::{self, BoardFlash};
use crate::cell::Shared;

pub const BASE: usize = 0x6000;
pub const SIZE: u32 = board::BOARD.flash.staging;

static OWNER: Shared<Ownership> = Shared::new(Ownership::FREE);

pub fn claim(who: Owner) -> bool {
    let mut o = OWNER.get();
    let ok = o.claim(who);
    OWNER.set(o);
    ok
}

pub fn holds(who: Owner) -> bool {
    OWNER.get().holds(who)
}

pub fn release(who: Owner) {
    OWNER.update(|mut o| {
        o.release(who);
        o
    });
}

pub fn activate() -> bool {
    if !rules::mark_bootable(&mut BoardFlash, BASE, SIZE, board::watchdog_feed) {
        return false;
    }
    board::reboot()
}
