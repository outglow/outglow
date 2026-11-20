use super::*;
use crate::sim::{self, Op};

const MESH_SECTOR: usize = MAIN;

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
    assert_eq!(scan(MESH_SECTOR, 0).1, 3 + RECORD_MAX);
    sim::with(|c| {
        c.flash[MESH_SECTOR + 3 + RECORD_MAX + 1] = 1;
        c.flash[MESH_SECTOR + 3 + RECORD_MAX] = 0xa4;
    });
    assert_eq!(hal.nv_load(&mut buf), Some(RECORD_MAX));
    assert_eq!(scan(MESH_SECTOR, 0).1, 3 + RECORD_MAX);
}

fn fill_to(free: usize) {
    fill(MESH_SECTOR, 0, free);
}

fn fill(sector: usize, start: usize, free: usize) {
    sim::with(|c| {
        let mut off = start;
        while off < free {
            let rem = free - off;
            let mut len = (rem - 3).min(RECORD_MAX);
            if (1..3).contains(&(rem - 3 - len)) {
                len -= 3;
            }
            let s = sector + off;
            c.flash[s] = 0xa5;
            c.flash[s + 1] = len as u8;
            c.flash[s + 2..s + 2 + len].fill(0x11);
            c.flash[s + 2 + len] = checksum(&vec![0x11; len]);
            off += 3 + len;
        }
    });
}

#[test]
fn a_record_that_would_pass_the_sector_end_is_backed_up_in_the_spare_then_starts_the_sector_over() {
    for (free, len, moved) in [(SECTOR_SIZE - 6, 3usize, false), (SECTOR_SIZE - 6, 4, true), (SECTOR_SIZE - 3, 0, false)] {
        let _s = sim::session();
        let mut hal = BleHal::new();
        fill_to(free);
        assert_eq!(scan(MESH_SECTOR, 0).1, free);
        let data = vec![0x22; len];
        hal.nv_store(&data);
        let erases: Vec<usize> = sim::with(|c| c.flash_ops.iter().filter(|o| o.0 == 0x20).map(|o| o.1).collect());
        assert_eq!(erases, if moved { vec![MESH_SECTOR] } else { vec![] }, "{free} {len}");
        let mut buf = [0u8; 8];
        assert_eq!(hal.nv_load(&mut buf), Some(len));
        let at = if moved { 0 } else { free };
        assert_eq!(mesh_sector()[at], 0xa5);
        assert_eq!(mesh_sector()[at + 2..at + 2 + len], data[..]);
        assert!(!backed_up());
        let spare = sim::with(|c| c.flash[SPARE..SPARE + 12].to_vec());
        let expect: Vec<u8> = if moved { [&[0x00, b'M', b'S', 0x02, 0xa5, 4][..], &data[..], &[checksum(&data), 0xff]].concat() } else { vec![0xff; 12] };
        assert_eq!(spare, expect);
    }
}

fn flash_copy() -> Vec<u8> {
    sim::with(|c| c.flash.clone())
}

fn load_on(flash: &[u8]) -> Option<Vec<u8>> {
    sim::with(|c| c.flash.copy_from_slice(flash));
    let mut buf = [0u8; RECORD_MAX];
    BleHal::new().nv_load(&mut buf).map(|n| buf[..n].to_vec())
}

#[derive(Clone, Debug)]
enum Step {
    Erase(usize),
    Program(usize, Vec<u8>),
}

fn apply(f: &mut [u8], step: &Step) {
    match step {
        Step::Erase(a) => f[*a..*a + SECTOR_SIZE].fill(0xff),
        Step::Program(a, d) => d.iter().enumerate().for_each(|(i, b)| f[a + i] &= b),
    }
}

fn cut_states(f: &[u8], step: &Step) -> Vec<Vec<u8>> {
    let mut out = vec![f.to_vec()];
    match step {
        Step::Erase(a) => {
            for (from, to) in [(0, SECTOR_SIZE / 2), (SECTOR_SIZE / 2, SECTOR_SIZE), (0, 1), (1, SECTOR_SIZE), (0, 64)] {
                let mut c = f.to_vec();
                c[a + from..a + to].fill(0xff);
                out.push(c);
            }
        }
        Step::Program(a, d) => {
            for k in 0..d.len() {
                for mask in [0x00u8, 0x0f, 0xf0] {
                    let mut c = f.to_vec();
                    d[..k].iter().enumerate().for_each(|(i, b)| c[a + i] &= b);
                    c[a + k] &= d[k] | mask;
                    out.push(c);
                }
            }
        }
    }
    let mut done = f.to_vec();
    apply(&mut done, step);
    out.push(done);
    out
}

fn every_cut(pre: &[u8], steps: &[Step], post: &[u8], allowed: &[Option<Vec<u8>>]) -> usize {
    let mut state = pre.to_vec();
    let mut cuts = 0;
    for step in steps {
        for cut in cut_states(&state, step) {
            let got = load_on(&cut);
            assert!(allowed.contains(&got), "a cut in {step:x?} loads {got:02x?}");
            BleHal::new().nv_store(&[0x77; 3]);
            assert_eq!(load_on(&flash_copy()), Some(vec![0x77; 3]), "a store after a cut in {step:x?}");
            cuts += 1;
        }
        apply(&mut state, step);
    }
    assert_eq!(state, post, "the steps are what the code did");
    cuts
}

fn record_of(data: &[u8]) -> Vec<u8> {
    [&[0xa5, data.len() as u8][..], data, &[checksum(data)]].concat()
}

fn erases() -> Vec<usize> {
    sim::with(|c| c.flash_ops.iter().filter(|o| o.0 == 0x20).map(|o| o.1).collect())
}

#[test]
fn settings_in_the_earlier_single_sector_format_stay_in_it_and_an_earlier_image_reads_them() {
    let _s = sim::session();
    let mut hal = BleHal::new();
    fill_to(SECTOR_SIZE - 20);
    hal.nv_store(&[7; 12]);
    let mut buf = [0u8; RECORD_MAX];
    assert_eq!(hal.nv_load(&mut buf), Some(12));
    assert!(flash::blank(SPARE));
    hal.nv_store(&[8; 12]);
    assert_eq!(hal.nv_load(&mut buf), Some(12));
    assert_eq!(buf[..12], [8; 12]);
    assert_eq!(scan(MESH_SECTOR, 0), (Some((MESH_SECTOR + 2, 12)), 15));
    assert!(!backed_up());
    hal.nv_store(&[9; 3]);
    assert_eq!(scan(MESH_SECTOR, 0), (Some((MESH_SECTOR + 15 + 2, 3)), 21));
}

#[test]
fn settings_an_earlier_image_wrote_after_a_move_win_over_the_spare() {
    let _s = sim::session();
    let mut hal = BleHal::new();
    fill_to(SECTOR_SIZE - 20);
    hal.nv_store(&[1; 12]);
    hal.nv_store(&[2; 12]);
    flash::erase_sector(MESH_SECTOR);
    flash::write(MESH_SECTOR, &record_of(&[9; 3]));
    let mut buf = [0u8; RECORD_MAX];
    assert_eq!(hal.nv_load(&mut buf), Some(3));
    assert_eq!(buf[..3], [9; 3]);
    flash::erase_sector(MESH_SECTOR);
    assert_eq!(hal.nv_load(&mut buf), None, "a factory reset of an earlier image brings nothing back");
}

#[test]
fn a_power_cut_anywhere_in_starting_the_sector_over_keeps_the_earlier_or_the_new_settings() {
    let _s = sim::session();
    fill_to(SECTOR_SIZE - 7);
    sim::with(|c| c.flash[SPARE..SPARE + 40].fill(0x3c));
    let pre = flash_copy();
    let old = load_on(&pre);
    assert_eq!(old, Some(vec![0x11; 2]));
    sim::with(|c| c.flash_ops.clear());
    BleHal::new().nv_store(&[0x42; 5]);
    assert_eq!(erases(), [SPARE, MESH_SECTOR]);
    let post = flash_copy();
    let rec = record_of(&[0x42; 5]);
    let steps = [
        Step::Erase(SPARE),
        Step::Program(SPARE + BACKUP_LEN, rec.clone()),
        Step::Program(SPARE, BACKUP.to_vec()),
        Step::Program(MESH_SECTOR, vec![0]),
        Step::Erase(MESH_SECTOR),
        Step::Program(MESH_SECTOR, rec),
        Step::Program(SPARE, vec![0]),
    ];
    let cuts = every_cut(&pre, &steps, &post, &[old, Some(vec![0x42; 5])]);
    assert!(cuts > 60, "{cuts}");
}

#[test]
fn a_cut_left_behind_with_only_the_spare_valid_keeps_it_until_the_next_record_is_in_place() {
    let _s = sim::session();
    let rec = record_of(&[0x31; 4]);
    sim::with(|c| {
        c.flash[SPARE..SPARE + BACKUP_LEN].copy_from_slice(&BACKUP);
        c.flash[SPARE + BACKUP_LEN..SPARE + BACKUP_LEN + rec.len()].copy_from_slice(&rec);
        c.flash[MESH_SECTOR..MESH_SECTOR + 8].fill(0x00);
    });
    let pre = flash_copy();
    assert_eq!(load_on(&pre), Some(vec![0x31; 4]));
    sim::with(|c| c.flash_ops.clear());
    BleHal::new().nv_store(&[0x52; 2]);
    assert_eq!(erases(), [MESH_SECTOR]);
    let post = flash_copy();
    let steps = [Step::Erase(MESH_SECTOR), Step::Program(MESH_SECTOR, record_of(&[0x52; 2])), Step::Program(SPARE, vec![0])];
    every_cut(&pre, &steps, &post, &[Some(vec![0x31; 4]), Some(vec![0x52; 2])]);
    sim::with(|c| {
        c.flash[MESH_SECTOR..MESH_SECTOR + SECTOR_SIZE].fill(0xff);
        c.flash[SPARE..SPARE + BACKUP_LEN].copy_from_slice(&BACKUP);
    });
    BleHal::new().nv_store(&[0x34; 1]);
    assert_eq!(scan(MESH_SECTOR, 0).0, Some((MESH_SECTOR + 2, 1)));
    assert!(!backed_up());
}

#[test]
fn a_backup_is_its_magic_in_full() {
    let _s = sim::session();
    assert!(!backed_up());
    flash::write(SPARE, &BACKUP);
    assert!(backed_up());
    for k in 0..BACKUP_LEN {
        let mut h = BACKUP;
        h[k] ^= 0x01;
        sim::with(|c| c.flash[SPARE..SPARE + BACKUP_LEN].copy_from_slice(&h));
        assert!(!backed_up(), "{k}");
    }
}

#[test]
fn a_record_after_garbage_in_the_free_space_starts_the_sector_over() {
    let _s = sim::session();
    let mut hal = BleHal::new();
    hal.nv_store(&[1, 2]);
    sim::with(|c| c.flash[MESH_SECTOR + 5 + 4] = 0x00);
    hal.nv_store(&[3, 4]);
    assert_eq!(erases(), [MESH_SECTOR]);
    assert_eq!(scan(MESH_SECTOR, 0), (Some((MESH_SECTOR + 2, 2)), 5));
    sim::with(|c| c.flash[MESH_SECTOR + 5 + 4] = 0xff);
    sim::with(|c| c.flash[MESH_SECTOR + 5 + 3] = 0x00);
    sim::with(|c| c.flash_ops.clear());
    hal.nv_store(&[5, 6]);
    assert_eq!(erases(), [SPARE, MESH_SECTOR]);
}

#[test]
fn a_record_ending_exactly_at_the_sector_end_is_read() {
    let _s = sim::session();
    let mut hal = BleHal::new();
    fill_to(SECTOR_SIZE - 7);
    hal.nv_store(&[5, 6, 7, 8]);
    assert_eq!(scan(MESH_SECTOR, 0), (Some((MESH_SECTOR + SECTOR_SIZE - 5, 4)), SECTOR_SIZE));
    let mut buf = [0u8; 8];
    assert_eq!(hal.nv_load(&mut buf), Some(4));
    assert_eq!(buf[..4], [5, 6, 7, 8]);
    sim::with(|c| {
        c.flash[MESH_SECTOR + SECTOR_SIZE - 6] = 5;
    });
    assert_eq!(scan(MESH_SECTOR, 0), (Some((MESH_SECTOR + SECTOR_SIZE - 10, 2)), SECTOR_SIZE - 7));
}

#[test]
fn erasing_the_settings_invalidates_both_sectors_before_erasing_the_main_one() {
    let _s = sim::session();
    erase_settings();
    assert!(sim::with(|c| c.flash_ops.is_empty()));
    BleHal::new().nv_store(&[1]);
    sim::with(|c| c.flash_ops.clear());
    erase_settings();
    assert_eq!(sim::with(|c| c.flash_ops.clone()), [(0x02, MESH_SECTOR), (0x20, MESH_SECTOR)]);
    assert!(flash::blank(MESH_SECTOR));
    flash::write(SPARE + BACKUP_LEN, &[0xa5, 0, 0x5a]);
    sim::with(|c| c.flash_ops.clear());
    erase_settings();
    assert!(sim::with(|c| c.flash_ops.is_empty()), "records without a backup mark are never read and wait for the next move");
    flash::write(SPARE, &BACKUP);
    flash::write(MESH_SECTOR, &record_of(&[4]));
    sim::with(|c| c.flash_ops.clear());
    erase_settings();
    assert_eq!(sim::with(|c| c.flash_ops.clone()), [(0x02, SPARE), (0x02, MESH_SECTOR), (0x20, MESH_SECTOR)]);
    assert_eq!(load_on(&flash_copy()), None);
}

#[test]
fn a_power_cut_anywhere_in_erasing_the_settings_loads_the_newest_or_nothing() {
    let _s = sim::session();
    let rec = record_of(&[0x61; 3]);
    sim::with(|c| {
        c.flash[SPARE..SPARE + BACKUP_LEN].copy_from_slice(&BACKUP);
        c.flash[SPARE + BACKUP_LEN..SPARE + BACKUP_LEN + rec.len()].copy_from_slice(&record_of(&[0x60; 3]));
        c.flash[MESH_SECTOR..MESH_SECTOR + rec.len()].copy_from_slice(&rec);
    });
    let pre = flash_copy();
    assert_eq!(load_on(&pre), Some(vec![0x61; 3]));
    erase_settings();
    let post = flash_copy();
    let steps = [Step::Program(SPARE, vec![0]), Step::Program(MESH_SECTOR, vec![0]), Step::Erase(MESH_SECTOR)];
    every_cut(&pre, &steps, &post, &[Some(vec![0x61; 3]), None]);
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
    let other = crate::tests::image_of(0x2000, crate::board::BOARD.image_type ^ 1);
    sim::with(|c| c.flash[staging::BASE..staging::BASE + 0x2000].copy_from_slice(&other));
    assert!(!hal.ota_activate());
    assert_eq!(sim::flash8(staging::BASE + 8), 0xff);
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
    crate::ble_radio::tests::busy_fifo();
    assert!(!hal.mesh_send(&[37], 1, &[0x02, 0x00]));
    let mut pdu = [0u8; ADV_PDU_MAX];
    assert_eq!(hal.mesh_listen(37, 0x1234_5678, 16, &mut pdu), None);
    assert!(sim::with(|c| c.writes()).contains(&Op::W32(0x408, 0x1234_5678)));
    let start = sim::with(|c| c.tick).wrapping_sub(16);
    let r = hal.connection_event(&ConnEvent { channel: 3, access_address: 1, crc_init: 2, start, window: 0, deadline: start });
    assert_eq!(r.anchor, None);
    assert!(sim::with(|c| c.writes()).contains(&Op::W8(0xf00, 0x82)));
}

#[test]
fn the_second_settings_sector_is_used_by_nothing_else_and_lies_past_the_image_and_the_staging_area() {
    use crate::board::{AWOX_1M, BARE_512K, BOARD, COLOUR_SECTORS, CONFIG_SECTORS};
    let f = BOARD.flash;
    let mut used: Vec<usize> = [CONFIG_SECTORS, crate::nv::SECTORS, COLOUR_SECTORS].concat();
    used.extend([MESH_SECTOR, f.reset, f.mac, f.cap_trim]);
    assert_eq!(MESH_SECTORS, [0xf_3000, 0xf_7000]);
    assert!(!used.contains(&MESH_SECTORS[1]), "{used:x?}");
    for (layout, image) in [(AWOX_1M, 0x6_a000..0x6_a000 + 0x6_4000), (BARE_512K, 0..0x6_4000)] {
        let spare = layout.mesh_spare;
        let taken: Vec<usize> = (0..7).map(|k| layout.store + k * 0x1000).chain([layout.reset, layout.mac, layout.cap_trim]).collect();
        assert!(!taken.contains(&spare), "{spare:x} in {taken:x?}");
        assert!(!image.contains(&spare) && spare >= image.end, "{spare:x}");
        assert!(!(0x6000..0x6000 + layout.staging as usize).contains(&spare));
        assert_eq!(spare % SECTOR_SIZE, 0);
    }
    assert_eq!(BARE_512K.mesh_spare, 0x7_5000);
}

fn spi_writes_of(f: impl FnOnce()) -> usize {
    let n = std::rc::Rc::new(std::cell::Cell::new(0usize));
    let m = n.clone();
    sim::with(|c| c.on(move |_, op| if matches!(op, Op::W8(a, _) if a == crate::reg::MSPI_DATA) { m.set(m.get() + 1) }));
    f();
    sim::with(|c| c.hook = None);
    n.get()
}

fn cut_after(writes: usize, f: impl FnOnce()) -> bool {
    let n = std::rc::Rc::new(std::cell::Cell::new(0usize));
    let shot: std::rc::Rc<std::cell::RefCell<Option<Vec<u8>>>> = Default::default();
    let (m, s2) = (n.clone(), shot.clone());
    sim::with(|c| {
        c.on(move |chip, op| {
            if matches!(op, Op::W8(a, _) if a == crate::reg::MSPI_DATA) {
                m.set(m.get() + 1);
                if m.get() == writes {
                    *s2.borrow_mut() = Some(chip.flash.clone());
                }
            }
        })
    });
    f();
    sim::with(|c| c.hook = None);
    let taken = shot.borrow_mut().take();
    match taken {
        Some(fl) => {
            sim::with(|c| c.flash = fl);
            true
        }
        None => false,
    }
}

#[test]
fn mesh_settings_chained_cuts() {
    let _s = sim::session();
    let chains: u64 = std::env::var("CHAINS").ok().and_then(|s| s.parse().ok()).unwrap_or(16);
    let (mut cuts, mut stores, mut bad) = (0usize, 0usize, Vec::new());
    for chain in 0..chains {
        let mut rng = 0x1234_5678_9abc_def1u64 ^ chain.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let mut next = move || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng
        };
        sim::with(|c| {
            c.flash[MAIN..MAIN + SECTOR_SIZE].fill(0xff);
            c.flash[SPARE..SPARE + SECTOR_SIZE].fill(0xff);
        });
        let mut model: Option<Vec<u8>> = None;
        for i in 0..400 {
            let len = (next() % 70) as usize;
            let data: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            let want = data[..len.min(RECORD_MAX)].to_vec();
            sim::with(|c| { c.log.clear(); c.flash_ops.clear(); });
            let pre = flash_copy();
            let total = spi_writes_of(|| BleHal::new().nv_store(&data));
            stores += 1;
            if next() % 3 != 0 || total == 0 {
                let got = load_on(&flash_copy());
                if got.as_ref() != Some(&want) {
                    bad.push(format!("chain {chain} store {i}: without a cut loads {got:02x?}"));
                    break;
                }
                model = Some(want);
                continue;
            }
            sim::with(|c| c.flash.copy_from_slice(&pre));
            let at = 1 + (next() as usize) % total;
            let was_cut = cut_after(at, || BleHal::new().nv_store(&data));
            cuts += was_cut as usize;
            let after_cut = flash_copy();
            let got = load_on(&after_cut);
            if got != model && got.as_ref() != Some(&want) {
                bad.push(format!("chain {chain} store {i} ({len} B): cut after SPI write {at}/{total} loads {:02x?}, before {:02x?}", got.as_ref().map(|g| g.len()), model.as_ref().map(|m| m.len())));
                break;
            }
            model = got;
        }
    }
    eprintln!("mesh settings: {chains} chains, {stores} stores, {cuts} cuts carried on from, {} bad", bad.len());
    for b in bad.iter().take(10) {
        eprintln!("  {b}");
    }
    assert!(bad.is_empty());
}

#[test]
fn a_record_torn_after_its_name_is_not_loaded_as_the_settings() {
    let _s = sim::session();
    let mut old = vec![0u8; 52];
    old[0] = 0xa7;
    old[1..9].copy_from_slice(b"unpaired");
    old[17..21].copy_from_slice(b"1234");
    BleHal::new().nv_store(&old);
    let mut new = vec![0x5au8; 52];
    new[0] = 0xa7;
    new[1..9].copy_from_slice(b"EgloMesh");
    new[9..16].fill(0);
    new[16] = checksum(&new[..16]).rotate_left(1);
    assert_eq!(checksum(&[&new[..17], &[0xff; 35][..]].concat()), 0xff);
    assert!(cut_after(5 + 2 + 17, || BleHal::new().nv_store(&new)));
    let got = load_on(&flash_copy());
    assert!(got == Some(old.clone()) || got == Some(new.clone()), "a cut after the name loads {got:02x?}");
}

#[test]
fn the_idle_hook_starts_the_settings_over_once_another_record_may_not_fit() {
    let _s = sim::session();
    let mut hal = BleHal::new();
    fill_to(SECTOR_SIZE - RECORD_BUF);
    assert!(!tidy());
    assert!(erases().is_empty());
    hal.nv_store(&[0x33; 6]);
    assert!(erases().is_empty());
    let (last, free) = scan(MESH_SECTOR, 0);
    assert_eq!(free, SECTOR_SIZE - RECORD_BUF + 9);
    assert!(tidy());
    assert_eq!(erases(), [MESH_SECTOR]);
    assert_eq!(last.map(|(_, n)| n), Some(6));
    assert_eq!(scan(MESH_SECTOR, 0), (Some((MESH_SECTOR + 2, 6)), 9));
    assert!(!backed_up());
    let mut buf = [0u8; 8];
    assert_eq!(hal.nv_load(&mut buf), Some(6));
    assert_eq!(buf[..6], [0x33; 6]);
    assert!(!tidy());
    assert_eq!(erases(), [MESH_SECTOR], "nothing stored since: nothing to look at");
}

#[test]
fn the_idle_hook_looks_again_only_after_a_store() {
    let _s = sim::session();
    let mut hal = BleHal::new();
    assert!(!tidy());
    fill_to(SECTOR_SIZE - RECORD_BUF + 1);
    assert!(!tidy());
    assert!(erases().is_empty());
    hal.nv_store(&[]);
    assert!(tidy());
    assert_eq!(erases(), [MESH_SECTOR]);
    assert_eq!(scan(MESH_SECTOR, 0), (Some((MESH_SECTOR + 2, 0)), 3));
}

#[test]
fn a_second_start_over_from_the_idle_hook_erases_the_used_spare_first() {
    let _s = sim::session();
    let mut hal = BleHal::new();
    hal.nv_store(&[1; 4]);
    fill(MESH_SECTOR, 7, SECTOR_SIZE - 2);
    hal.nv_store(&[2; 4]);
    assert_eq!(erases(), [MESH_SECTOR]);
    sim::with(|c| c.flash_ops.clear());
    fill(MESH_SECTOR, 7, SECTOR_SIZE - 6);
    hal.nv_store(&[]);
    assert!(erases().is_empty());
    assert!(tidy());
    assert_eq!(erases(), [SPARE, MESH_SECTOR]);
}

#[test]
fn a_sector_full_of_broken_records_is_left_to_the_next_store() {
    let _s = sim::session();
    let mut hal = BleHal::new();
    fill_to(SECTOR_SIZE - 3);
    sim::with(|c| {
        let mut off = 0;
        while off + 3 <= SECTOR_SIZE && c.flash[MESH_SECTOR + off] == 0xa5 {
            let len = c.flash[MESH_SECTOR + off + 1] as usize;
            c.flash[MESH_SECTOR + off + 2 + len] ^= 0xff;
            off += 3 + len;
        }
    });
    assert_eq!(scan(MESH_SECTOR, 0), (None, SECTOR_SIZE - 3));
    TIDY.set(true);
    assert!(!tidy());
    assert!(erases().is_empty());
    let mut buf = [0u8; 8];
    assert_eq!(hal.nv_load(&mut buf), None);
}
