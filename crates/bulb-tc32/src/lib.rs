#![cfg_attr(target_os = "none", no_std)]
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
#[cfg(not(target_os = "none"))]
pub mod sim;
#[allow(unsafe_code)]
mod stack;
mod staging;
mod trace;
mod zb;

use bulb_core::hal::Clock;
use bulb_core::reset::PowerCycles;
use bulb_core::{Bulb, Command};

const WATCHDOG_MS: u32 = 2_000;
const IDLE_MAX: u32 = 1_000 * reg::TICKS_PER_US;
const IDLE_BLE_MARGIN: u32 = 300 * reg::TICKS_PER_US;
const IDLE_MIN: u32 = 50 * reg::TICKS_PER_US;

#[cfg(not(test))]
#[allow(unsafe_code)]
#[cfg_attr(target_os = "none", no_mangle)]
pub extern "C" fn main() -> ! {
    run()
}

fn run() -> ! {
    board::init();
    let warm = board::warm_reset();
    let (mut cycles, factory_reset) = if warm {
        (PowerCycles::idle(board::FlashWords), false)
    } else {
        PowerCycles::power_on(board::FlashWords, self::factory_reset)
    };
    link::publish_start(cycles.step(), factory_reset, warm);
    let mut bulb = Bulb::boot(board::LedPwm::new(), board::SystemClock::new(), board::FlashStore::new(), zb::Link::take());
    if factory_reset {
        bulb.handle(Command::Identify { times: 3 });
    }
    let uptime = board::SystemClock::new();
    board::watchdog_start(WATCHDOG_MS);
    reg::set8(reg::SYSTEM_TICK_MODE, 0x02);
    loop {
        board::watchdog_feed();
        let start = reg::tick();
        bulb.tick();
        cycles.tick(uptime.now_ms());
        link::publish_reset_step(cycles.step());
        let end = reg::tick();
        zb::LOOP_MAX_US.update(|m| m.max(end.wrapping_sub(start) / reg::TICKS_PER_US));
        if let Some(until) = idle_until(end, zb::BLE_NEXT.get()) {
            bulb_idle(until);
        }
    }
}

fn idle_until(end: u32, ble_next: Option<u32>) -> Option<u32> {
    let wait = match ble_next {
        Some(next) => IDLE_MAX.min((next.wrapping_sub(end) as i32).max(0) as u32).saturating_sub(IDLE_BLE_MARGIN),
        None => IDLE_MAX,
    };
    (wait >= IDLE_MIN).then(|| end.wrapping_add(wait))
}

#[allow(unsafe_code)]
#[no_mangle]
#[inline(never)]
pub extern "C" fn bulb_idle(until: u32) {
    reg::write32(reg::SYSTEM_TICK_IRQ, until);
    while (until.wrapping_sub(reg::tick()) as i32) > 0 && !radio::has_frame() {}
}

pub(crate) fn factory_reset() {
    nv::open().forget_all_but(zigbee::hal::NvItem::Counters as u8);
    board::FlashStore::erase();
    #[cfg(feature = "ble")]
    ble::erase_settings();
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    board::reboot()
}

#[cfg(test)]
#[path = "tests/lib.rs"]
pub(crate) mod tests;
