use super::*;
use crate::sim;
use bulb_core::hal::ConfigStore;
use zigbee::hal::NvItem;

#[test]
fn idling_arms_the_timer_and_waits_for_the_tick() {
    let _s = sim::session();
    let until = sim::with(|c| c.tick).wrapping_add(500 * reg::TICKS_PER_US);
    bulb_idle(until);
    assert!(sim::with(|c| c.writes()).contains(&sim::Op::W32(reg::SYSTEM_TICK_IRQ, until)));
    assert!((sim::with(|c| c.tick).wrapping_sub(until) as i32) >= 0);
}

#[test]
fn a_factory_reset_keeps_only_the_frame_counters() {
    let _s = sim::session();
    let mut nv = nv::open();
    nv.store(NvItem::Network as u8, &[1, 2, 3]);
    nv.store(NvItem::Counters as u8, &[4, 5]);
    board::FlashStore::new().store(&bulb_core::config::Config::default().encode());
    #[cfg(feature = "ble")]
    crate::flash::write(crate::ble::MESH_SECTOR, &[0]);
    factory_reset();
    let nv = nv::open();
    let mut buf = [0u8; 8];
    assert_eq!(nv.load(NvItem::Network as u8, &mut buf), None);
    assert_eq!(nv.load(NvItem::Counters as u8, &mut buf), Some(2));
    assert_eq!(board::FlashStore::new().load(&mut buf), None);
    #[cfg(feature = "ble")]
    assert!(crate::flash::blank(crate::ble::MESH_SECTOR));
}

#[test]
fn a_driver_loop_that_never_ends_stops_at_the_register_log_limit() {
    let _s = sim::session();
    sim::with(|c| c.log.resize(sim::LOG_MAX, sim::Op::R8(0, 0)));
    let r = std::panic::catch_unwind(|| reg::read8(0));
    assert!(r.is_err());
}
