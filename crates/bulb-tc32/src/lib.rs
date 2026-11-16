//! Bulb firmware for the Telink TLSR8258, in Rust, without vendor code.
//!
//! `boot/start.S` sets up the stacks, the flash cache, `.data` and `.bss` and
//! calls `main`. The hardware is driven through registers (`reg`, `board`,
//! `flash`, `radio`); the light's behaviour is `bulb-core` and the network
//! is the `zigbee` crate, the same code the host tests run. Commands arrive
//! over Zigbee, over Bluetooth LE (the `ble` crate on `ble_radio`, which
//! borrows the radio for single events), and for tests over the debug link
//! (`link`).
#![no_std]
// `unsafe` only where the hardware needs it: registers (`reg`), the cells
// shared with the interrupt handler and the DMA (`cell`), reading the flash
// (`flash`) and the stack's own memory (`stack`).
#![deny(unsafe_code)]

mod aes;
mod board;
mod boards;
#[allow(unsafe_code)]
mod cell;
#[cfg(feature = "ble")]
mod ble;
#[cfg(feature = "ble")]
mod ble_radio;
#[cfg(feature = "ble")]
mod diag;
#[allow(unsafe_code)]
mod flash;
mod link;
mod nv;
mod radio;
#[allow(unsafe_code)]
mod reg;
#[allow(unsafe_code)]
mod stack;
mod staging;
mod trace;
mod zb;

use bulb_core::hal::Clock;
use bulb_core::reset::PowerCycles;
use bulb_core::{Bulb, Command};

/// The main loop must come round at least this often, or the chip restarts.
const WATCHDOG_MS: u32 = 2_000;
/// The longest a round with nothing to do waits in `bulb_idle`: what is not
/// driven by an interrupt (the timers of the stack and of the light, the
/// debug link) is taken up that much later at most.
const IDLE_MAX: u32 = 1_000 * reg::TICKS_PER_US;
/// The main loop comes round this long before BLE needs the radio.
const IDLE_BLE_MARGIN: u32 = 300 * reg::TICKS_PER_US;
/// A wait shorter than this is not worth it.
const IDLE_MIN: u32 = 50 * reg::TICKS_PER_US;

#[allow(unsafe_code)] // a linker attribute

#[no_mangle]
pub extern "C" fn main() -> ! {
    board::init();
    // The factory reset sequence counts power-ons only.
    let warm = board::warm_reset();
    let (mut cycles, factory_reset) = if warm {
        (PowerCycles::idle(board::FlashWords), false)
    } else {
        PowerCycles::power_on(board::FlashWords)
    };
    if factory_reset {
        self::factory_reset();
    }
    link::publish_start(cycles.step(), factory_reset, warm);
    let mut bulb = Bulb::boot(board::LedPwm::new(), board::SystemClock::new(), board::FlashStore::new(), zb::Link::take());
    if factory_reset {
        // three blinks: the reset took
        bulb.handle(Command::Identify { times: 3 });
    }
    let uptime = board::SystemClock::new();
    board::watchdog_start(WATCHDOG_MS);
    // the compare's interrupt source on, for `bulb_idle`; it stays masked
    reg::set8(reg::SYSTEM_TICK_MODE, 0x02);
    loop {
        board::watchdog_feed();
        let start = reg::tick();
        bulb.tick();
        cycles.tick(uptime.now_ms());
        link::publish_reset_step(cycles.step());
        let end = reg::tick();
        zb::LOOP_MAX_US.update(|m| m.max(end.wrapping_sub(start) / reg::TICKS_PER_US));
        let mut wait = IDLE_MAX;
        if let Some(next) = zb::BLE_NEXT.get() {
            // (a moment already past wraps to a huge "left", hence the sign)
            let left = next.wrapping_sub(end) as i32;
            wait = wait.min(left.max(0) as u32).saturating_sub(IDLE_BLE_MARGIN);
        }
        if wait >= IDLE_MIN {
            bulb_idle(end.wrapping_add(wait));
        }
    }
}

/// A round of the main loop with nothing to do waits here, interrupts on,
/// until `until` or until a frame has come in. On the chip it is the busy
/// loop the main loop always was, in one place: the emulator, finding this
/// function by its name, jumps over the wait to the next event instead of
/// running it instruction by instruction (idle skipping). For that the
/// system timer's compare is set to `until`; its interrupt source is on but
/// masked (0x640), so it is never taken.
#[allow(unsafe_code)] // a linker attribute
#[no_mangle]
#[inline(never)]
pub extern "C" fn bulb_idle(until: u32) {
    reg::write32(reg::SYSTEM_TICK_IRQ, until);
    while (until.wrapping_sub(reg::tick()) as i32) > 0 && !radio::has_frame() {}
}

/// Factory reset: the network, the tables and the light's settings go, and
/// the BLE mesh network; the frame counters stay. (The power cycle sequence
/// at the start, and the device page's command before its restart.)
pub(crate) fn factory_reset() {
    nv::Nv::factory_reset(zigbee::hal::NvItem::Counters as u8);
    board::FlashStore::erase();
    #[cfg(feature = "ble")]
    ble::erase_settings();
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    board::reboot()
}
