use super::*;
use crate::sim::{self, Op};

fn mesh_sector() -> Vec<u8> {
    sim::with(|c| c.flash[MESH_SECTOR..MESH_SECTOR + SECTOR_SIZE].to_vec())
}

#[test]
fn a_settings_record_is_the_marker_its_length_the_data_and_a_rotating_checksum() {
    let _s = sim::session();
    let mut hal = BleHal::new();
    hal.nv_store(&[1, 2]);
    assert_eq!(mesh_sector()[..6], [0xa5, 2, 1, 2, 0x69, 0xff]);
    assert_eq!(checksum(&[]), 0x5a);
    assert_eq!(checksum(&[0x80]), 0x34);
    assert_eq!(checksum(&[0xff, 0x01]), 0x97);
    let mut buf = [0u8; 8];
    assert_eq!(hal.nv_load(&mut buf), Some(2));
    assert_eq!(buf[..2], [1, 2]);
}

#[test]
fn the_last_intact_record_is_loaded_and_new_ones_go_after_the_last() {
    let _s = sim::session();
    let mut hal = BleHal::new();
    let mut buf = [0u8; 70];
    assert_eq!(hal.nv_load(&mut buf), None);
    hal.nv_store(&[1, 2, 3]);
    hal.nv_store(&[4; 10]);
    assert_eq!(hal.nv_load(&mut buf), Some(10));
    assert_eq!(buf[..10], [4; 10]);
    sim::with(|c| c.flash[MESH_SECTOR + 6 + 2 + 10] ^= 0x01);
    assert_eq!(hal.nv_load(&mut buf), Some(3));
    assert_eq!(buf[..3], [1, 2, 3]);
    hal.nv_store(&[7]);
    assert_eq!(mesh_sector()[6 + 13..6 + 13 + 4], [0xa5, 1, 7, checksum(&[7])]);
    assert_eq!(hal.nv_load(&mut buf), Some(1));
    let mut small = [0u8; 2];
    hal.nv_store(&[9, 8, 7, 6]);
    assert_eq!(hal.nv_load(&mut small), Some(2));
    assert_eq!(small, [9, 8]);
}

#[test]
fn a_record_is_at_most_64_octets_and_a_longer_length_ends_the_records() {
    let _s = sim::session();
    let mut hal = BleHal::new();
    let data: Vec<u8> = (0..70).collect();
    hal.nv_store(&data);
    let mut buf = [0u8; 70];
    assert_eq!(hal.nv_load(&mut buf), Some(RECORD_MAX));
    assert_eq!(buf[..RECORD_MAX], data[..RECORD_MAX]);
    assert_eq!(mesh_sector()[1], RECORD_MAX as u8);
    hal.nv_store(&[1]);
    sim::with(|c| {
        c.flash[MESH_SECTOR + 3 + RECORD_MAX + 1] = RECORD_MAX as u8 + 1;
    });
    assert_eq!(hal.nv_load(&mut buf), Some(RECORD_MAX));
    assert_eq!(scan().1, 3 + RECORD_MAX);
    sim::with(|c| {
        c.flash[MESH_SECTOR + 3 + RECORD_MAX + 1] = 1;
        c.flash[MESH_SECTOR + 3 + RECORD_MAX] = 0xa4;
    });
    assert_eq!(hal.nv_load(&mut buf), Some(RECORD_MAX));
    assert_eq!(scan().1, 3 + RECORD_MAX);
}

fn fill_to(free: usize) {
    sim::with(|c| {
        let mut off = 0;
        while off < free {
            let rem = free - off;
            let mut len = (rem - 3).min(RECORD_MAX);
            if (1..3).contains(&(rem - 3 - len)) {
                len -= 3;
            }
            let s = MESH_SECTOR + off;
            c.flash[s] = 0xa5;
            c.flash[s + 1] = len as u8;
            c.flash[s + 2..s + 2 + len].fill(0x11);
            c.flash[s + 2 + len] = checksum(&vec![0x11; len]);
            off += 3 + len;
        }
    });
}

#[test]
fn a_record_that_would_pass_the_sector_end_starts_the_sector_over() {
    for (free, len, erased) in [(SECTOR_SIZE - 6, 3usize, false), (SECTOR_SIZE - 6, 4, true), (SECTOR_SIZE - 3, 0, false)] {
        let _s = sim::session();
        let mut hal = BleHal::new();
        fill_to(free);
        assert_eq!(scan().1, free);
        let data = vec![0x22; len];
        hal.nv_store(&data);
        let erases = sim::with(|c| c.flash_ops.iter().filter(|o| o.0 == 0x20).count());
        assert_eq!(erases, erased as usize, "{free} {len}");
        let mut buf = [0u8; 8];
        assert_eq!(hal.nv_load(&mut buf), Some(len));
        let at = if erased { 0 } else { free };
        assert_eq!(mesh_sector()[at], 0xa5);
        assert_eq!(mesh_sector()[at + 2..at + 2 + len], data[..]);
    }
}

#[test]
fn a_record_ending_exactly_at_the_sector_end_is_read() {
    let _s = sim::session();
    let mut hal = BleHal::new();
    fill_to(SECTOR_SIZE - 7);
    hal.nv_store(&[5, 6, 7, 8]);
    assert_eq!(scan(), (Some((SECTOR_SIZE - 5, 4)), SECTOR_SIZE));
    let mut buf = [0u8; 8];
    assert_eq!(hal.nv_load(&mut buf), Some(4));
    assert_eq!(buf[..4], [5, 6, 7, 8]);
    sim::with(|c| {
        c.flash[MESH_SECTOR + SECTOR_SIZE - 6] = 5;
    });
    assert_eq!(scan(), (Some((SECTOR_SIZE - 10, 2)), SECTOR_SIZE - 7));
}

#[test]
fn erasing_the_settings_erases_their_sector_only_when_it_holds_something() {
    let _s = sim::session();
    erase_settings();
    assert!(sim::with(|c| c.flash_ops.is_empty()));
    BleHal::new().nv_store(&[1]);
    erase_settings();
    assert_eq!(sim::with(|c| c.flash_ops.last().copied()), Some((0x20, MESH_SECTOR)));
    assert!(flash::blank(MESH_SECTOR));
}

#[test]
fn the_address_and_the_random_numbers_come_from_the_mac_sector_and_the_tick() {
    let _s = sim::session();
    sim::with(|c| {
        c.flash[MAC_SECTOR..MAC_SECTOR + 8].copy_from_slice(&[0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xdc, 0xfe]);
        c.tick = 0x0102_0300;
        c.tick_step = 16;
    });
    let mut hal = BleHal::new();
    let seed = (0x7654_3210u32 ^ 0x0102_0310) | 1;
    assert_eq!(hal.rng, seed);
    assert_eq!(hal.address(), [0x10, 0x32, 0x54, 0x76, 0x98, 0xba]);
    let mut x = seed;
    for k in 0..4u32 {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        assert_eq!(hal.random(), x ^ (0x0102_0310 + 16 * (k + 1)));
    }
    assert_eq!(hal.now(), 0x0102_0310 + 16 * 5);
    sim::with(|c| c.flash[MAC_SECTOR] = 0x11);
    assert_eq!(BleHal::new().rng, (0x7654_3211u32 ^ (0x0102_0310 + 16 * 6)) | 1);
}

#[test]
fn an_update_over_ble_writes_only_inside_the_staging_area_it_holds() {
    let _s = sim::session();
    let mut hal = BleHal::new();
    staging::release(Owner::Zigbee);
    staging::release(Owner::Ble);
    assert!(staging::claim(Owner::Zigbee));
    assert_eq!(hal.ota_capacity(), 0);
    hal.ota_write(0, &[1]);
    sim::with(|c| c.flash[staging::BASE] = 0);
    hal.ota_erase(0);
    assert!(sim::with(|c| c.flash_ops.is_empty()));
    staging::release(Owner::Zigbee);
    assert_eq!(hal.ota_capacity(), staging::SIZE);
    assert!(staging::holds(Owner::Ble));
    hal.ota_erase(0x0fff);
    assert_eq!(sim::with(|c| c.flash_ops.clone()), vec![(0x20, staging::BASE)]);
    hal.ota_erase(0x1000);
    assert_eq!(sim::with(|c| c.flash_ops.len()), 1);
    let last = staging::SIZE - 0x1000;
    sim::with(|c| {
        c.flash[staging::BASE + last as usize] = 0;
        c.flash[staging::BASE + staging::SIZE as usize] = 0;
    });
    hal.ota_erase(staging::SIZE);
    assert_eq!(sim::with(|c| c.flash_ops.len()), 1);
    hal.ota_erase(staging::SIZE - 1);
    assert_eq!(sim::with(|c| c.flash_ops.last().copied()), Some((0x20, staging::BASE + last as usize)));
    hal.ota_write(staging::SIZE - 2, &[1, 2]);
    hal.ota_write(staging::SIZE - 1, &[3, 4]);
    assert_eq!(sim::with(|c| c.flash[staging::BASE + staging::SIZE as usize - 2..staging::BASE + staging::SIZE as usize + 1].to_vec()), vec![1, 2, 0]);
    hal.ota_write(0x10, &[0x5a; 3]);
    assert_eq!(sim::with(|c| c.flash[staging::BASE + 0x10..staging::BASE + 0x14].to_vec()), vec![0x5a, 0x5a, 0x5a, 0xff]);
    staging::release(Owner::Ble);
}

#[test]
fn a_complete_image_is_marked_bootable_and_the_chip_restarts() {
    let _s = sim::session();
    let mut hal = BleHal::new();
    staging::release(Owner::Zigbee);
    assert_eq!(hal.ota_capacity(), staging::SIZE);
    let img = crate::tests::image(0x2000);
    hal.ota_write(0, &img);
    let mut broken = img.clone();
    broken[0x100] ^= 1;
    sim::with(|c| c.flash[staging::BASE + 0x100] = broken[0x100]);
    assert!(!hal.ota_activate());
    sim::with(|c| c.flash[staging::BASE..staging::BASE + 0x2000].copy_from_slice(&img));
    assert!(sim::reboots(move || {
        let _ = BleHal::new().ota_activate();
    }));
    assert_eq!(sim::flash8(staging::BASE + 8), 0x4b);
    staging::release(Owner::Ble);
}

#[test]
fn zigbee_air_counts_as_busy_for_3_ms_after_our_frame_and_neighbours_for_3_ms_after_theirs() {
    let _s = sim::session();
    let mut hal = BleHal::new();
    sim::with(|c| c.tick = 0x1000_0000);
    let now = 0x1000_0000u32 + 16;
    crate::radio::tests::heard_at(now.wrapping_sub(RADIO_QUIET));
    assert!(!hal.neighbours_talking());
    crate::radio::tests::heard_at(now.wrapping_add(16).wrapping_sub(RADIO_QUIET) + 1);
    assert!(hal.neighbours_talking());
    crate::radio::tests::sent_at(now.wrapping_add(32).wrapping_sub(RADIO_QUIET));
    assert!(!hal.radio_busy());
    crate::radio::tests::sent_at(now.wrapping_add(48).wrapping_sub(RADIO_QUIET) + 1);
    assert!(hal.radio_busy());
    crate::radio::tests::sent_at(0);
    crate::radio::tests::heard_at(0);
    sim::with(|c| c.poke8(0x448, 0x20));
    assert!(hal.radio_busy());
    assert!(hal.neighbours_talking());
    sim::with(|c| {
        c.poke8(0x448, 0x00);
        c.poke8(0x441, 0x1a);
    });
    assert!(hal.radio_busy());
    assert!(hal.neighbours_talking());
}

#[test]
fn the_hal_hands_the_radio_calls_to_the_ble_driver() {
    let _s = sim::session();
    let mut hal = BleHal::new();
    hal.connection_reset();
    assert!(hal.tx_idle());
    assert_eq!(hal.tx_free(), 8);
    assert!(hal.tx_push(2, &[1, 2]));
    assert_eq!(hal.tx_free(), 7);
    assert!(!hal.tx_idle());
    let mut p = [0u8; LL_PAYLOAD_MAX];
    assert_eq!(hal.rx_pop(&mut p), None);
    hal.connection_reset();
    assert_eq!(hal.tx_free(), 8);
    assert!(hal.tx_idle());
    let mut c = [0u8; CONNECT_LEN];
    sim::with(|c| c.log.clear());
    assert_eq!(hal.advertise(&[0], &[0, 0], &mut c), None);
    assert!(hal.mesh_send(&[37], 1, &[0]));
    assert!(sim::with(|c| c.writes()).is_empty());
    let mut pdu = [0u8; ADV_PDU_MAX];
    assert_eq!(hal.mesh_listen(37, 0x1234_5678, 16, &mut pdu), None);
    assert!(sim::with(|c| c.writes()).contains(&Op::W32(0x408, 0x1234_5678)));
    let start = sim::with(|c| c.tick).wrapping_sub(16);
    let r = hal.connection_event(&ConnEvent { channel: 3, access_address: 1, crc_init: 2, start, window: 0, deadline: start });
    assert_eq!(r.anchor, None);
    assert!(sim::with(|c| c.writes()).contains(&Op::W8(0xf00, 0x82)));
}
