use super::*;
use crate::sim;

#[test]
fn lqi_is_a_linear_map_of_rssi_from_minus_99_to_minus_15() {
    assert_eq!(lqi_from_rssi(-200), 0);
    assert_eq!(lqi_from_rssi(-99), 0);
    assert_eq!(lqi_from_rssi(-57), 127);
    assert_eq!(lqi_from_rssi(-15), 255);
    assert_eq!(lqi_from_rssi(20), 255);
}

#[test]
fn build_time_numbers_are_parsed_as_hexadecimal() {
    assert_eq!(parse_hex("0x01000001"), 0x0100_0001);
    assert_eq!(parse_hex("0XfF"), 0xff);
    assert_eq!(parse_hex("1aB"), 0x1ab);
    assert_eq!(parse_key("000102030405060708090a0B0c0D0e0F"), core::array::from_fn(|i| i as u8));
    assert_eq!(FILE_VERSION, 0x0100_0000);
    assert!(std::panic::catch_unwind(|| parse_hex("0x1g")).is_err());
    assert!(std::panic::catch_unwind(|| parse_key("00")).is_err());
    assert!(std::panic::catch_unwind(|| parse_key("0000000000000000000000000000000z")).is_err());
}

#[test]
fn a_blank_mac_sector_gets_a_random_telink_address() {
    let _s = sim::session();
    let mut hal = Tc32Hal::new();
    let ieee = hal.ieee();
    assert_eq!(ieee >> 40, 0xa4_c138);
    let mut stored = [0u8; 8];
    flash::read(MAC_SECTOR, &mut stored);
    assert_eq!(stored[3..6], TELINK_OUI);
    assert_eq!(Tc32Hal::new().ieee(), ieee);
}

#[test]
fn a_zeroed_mac_sector_is_erased_before_the_new_address() {
    let _s = sim::session();
    sim::with(|c| c.flash[MAC_SECTOR..MAC_SECTOR + 8].fill(0));
    let ieee = Tc32Hal::new().ieee();
    assert_eq!(ieee >> 40, 0xa4_c138);
    assert!(sim::with(|c| c.flash_ops.contains(&(0x20, MAC_SECTOR))));
}

#[test]
fn an_address_in_zigbee_order_is_kept_as_it_is() {
    let _s = sim::session();
    sim::with(|c| c.flash[MAC_SECTOR..MAC_SECTOR + 8].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]));
    assert_eq!(Tc32Hal::new().ieee(), u64::from_le_bytes([1, 2, 3, 4, 5, 6, 7, 8]));
}

#[test]
fn the_milliseconds_keep_the_remainder_of_the_ticks() {
    let _s = sim::session();
    let mut hal = Tc32Hal::new();
    sim::with(|c| c.tick_step = 0);
    let t = sim::with(|c| c.tick);
    hal.last_tick = t;
    sim::with(|c| c.tick = t.wrapping_add(TICKS_PER_MS * 3 / 2));
    assert_eq!(hal.now_ms(), 1);
    sim::with(|c| c.tick = t.wrapping_add(TICKS_PER_MS * 3));
    assert_eq!(hal.now_ms(), 3);
    assert_ne!(hal.random(), hal.random());
}

#[test]
fn retained_data_survives_once_and_only_intact() {
    let _s = sim::session();
    let mut hal = Tc32Hal::new();
    hal.retain(&[1, 2, 3]);
    let mut buf = [0u8; 8];
    assert_eq!(hal.retained(&mut buf), Some(3));
    assert_eq!(buf[..3], [1, 2, 3]);
    assert_eq!(hal.retained(&mut buf), None);
    hal.retain(&[1, 2, 3]);
    RETAINED.write(12, 9);
    assert_eq!(hal.retained(&mut buf), None);
    hal.retain(&[1, 2, 3]);
    assert_eq!(hal.retained(&mut [0u8; 2]), None);
    assert_ne!(retained_sum(3, &[1, 2, 3]), retained_sum(4, &[1, 2, 3]));
}

#[test]
fn an_ota_download_erases_each_staging_sector_once_before_writing() {
    let _s = sim::session();
    let mut hal = Tc32Hal::new();
    staging::release(Owner::Ble);
    assert!(!hal.ota_begin(staging::SIZE + 1));
    assert!(hal.ota_begin(0x2000));
    sim::with(|c| c.flash[staging::BASE + 0x1000] = 0);
    hal.ota_write(0, &[0xaa; 32]);
    hal.ota_write(0x1000, &[0xbb; 32]);
    hal.ota_write(staging::SIZE, &[0xcc; 1]);
    let erases: Vec<usize> = sim::with(|c| c.flash_ops.iter().filter(|o| o.0 == 0x20).map(|o| o.1).collect());
    assert_eq!(erases, vec![staging::BASE + 0x1000]);
    let mut b = [0u8; 1];
    flash::read(staging::BASE + 0x1000, &mut b);
    assert_eq!(b, [0xbb]);
    assert!(hal.ota_resume(0x1001));
    assert_eq!(hal.staging_erased, 0x2000);
    staging::release(Owner::Zigbee);
    assert!(!hal.ota_resume(staging::SIZE + 1));
}

#[test]
fn the_nv_items_are_stored_in_flash() {
    let _s = sim::session();
    let mut hal = Tc32Hal::new();
    hal.nv_store(NvItem::Network, &[1, 2, 3]);
    let mut buf = [0u8; 8];
    assert_eq!(Tc32Hal::new().nv_load(NvItem::Network, &mut buf), Some(3));
    hal.nv_erase();
    assert_eq!(Tc32Hal::new().nv_load(NvItem::Network, &mut buf), None);
    let c = hal.counters();
    assert_eq!((c.stack_free, c.irq_stack_free), (0, 0));
    assert_eq!(hal.reset_cause(), crate::board::reset_cause());
}
