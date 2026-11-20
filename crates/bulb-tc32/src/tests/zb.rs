use super::*;
use crate::sim::{self, Op};

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
    let _s = sim::session();
    assert_eq!(parse_hex("0x1"), 1);
    assert_eq!(parse_hex("10"), 0x10);
    assert_eq!(parse_hex("0"), 0);
    assert_eq!(parse_hex("ffffffff"), 0xffff_ffff);
    assert!(std::panic::catch_unwind(|| parse_hex("0x")).is_err());
    assert!(std::panic::catch_unwind(|| parse_hex("1x2")).is_err());
    assert_eq!(parse_key("ffffffffffffffffffffffffffffffff"), [0xff; 16]);
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
    assert_eq!((c.stack_free, c.irq_stack_free), crate::stack::free());
    assert_ne!(crate::stack::free(), (0, 0));
    assert_eq!(hal.reset_cause(), crate::board::reset_cause());
}

#[test]
fn a_bulb_on_the_simulated_chip_starts_zigbee_and_bluetooth() {
    let _s = sim::session();
    crate::board::warm_reset();
    let mut link = Link::new();
    let started = std::time::Instant::now();
    let mut polls = 0;
    while polls < 3_000 && started.elapsed() < std::time::Duration::from_secs(20) {
        let _ = link.poll();
        sim::with(|c| c.tick = c.tick.wrapping_add(16_000));
        polls += 1;
    }
    assert!(link.stack.is_some());
    assert!(sim::with(|c| c.writes()).contains(&sim::Op::W8(0x12d2, 0x9b)));
    assert!(sim::with(|c| c.tx_kicks) > 0, "the stack sent nothing in {polls} polls");
    #[cfg(feature = "ble")]
    {
        assert!(link.ble.is_some());
        assert!(crate::ble_radio::lent().0 > 0, "no BLE event in {polls} polls");
    }
    assert_eq!(crate::link::BULB_NET.read(1) >= 11, true);
}

#[test]
fn the_random_numbers_are_two_multiply_with_carry_generators_mixed_with_the_tick() {
    let _s = sim::session();
    sim::with(|c| {
        c.flash[MAC_SECTOR..MAC_SECTOR + 8].copy_from_slice(&[0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef]);
        c.tick = 0x1000;
    });
    let mut hal = Tc32Hal::new();
    let a = (0x6745_2301u32 ^ 0x1020) | 1;
    let b = 0xefcd_ab89u32.wrapping_add(0x1030) | 1;
    assert_eq!(hal.rng, [a, b]);
    let (mut x, mut y) = (a, b);
    let mut t = sim::with(|c| c.tick);
    for _ in 0..3 {
        x = 18000 * (x & 0xffff) + (x >> 16);
        y = 36969 * (y & 0xffff) + (y >> 16);
        t += 16;
        assert_eq!(hal.random(), (y << 16).wrapping_add(x) ^ t);
    }
}

#[test]
fn an_address_with_a_listed_ble_oui_is_turned_into_zigbee_order() {
    for oui in BLE_ORDER_OUIS {
        let _s = sim::session();
        let stored = [0x11, 0x22, 0x33, oui[0], oui[1], oui[2], 0x77, 0x88];
        sim::with(|c| c.flash[MAC_SECTOR..MAC_SECTOR + 8].copy_from_slice(&stored));
        assert_eq!(Tc32Hal::new().ieee(), u64::from_le_bytes([0x77, 0x88, 0x11, 0x22, 0x33, oui[0], oui[1], oui[2]]), "{oui:x?}");
    }
    for stored in [[0x11, 0x22, 0x33, 0x38, 0xc1, 0xa5, 0x77, 0x88], [0, 0, 0, 0, 0, 0, 0, 1], [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xfe]] {
        let _s = sim::session();
        sim::with(|c| c.flash[MAC_SECTOR..MAC_SECTOR + 8].copy_from_slice(&stored));
        assert_eq!(Tc32Hal::new().ieee(), u64::from_le_bytes(stored));
        assert!(sim::with(|c| c.flash_ops.is_empty()));
    }
}

#[test]
fn a_new_address_is_stored_in_ble_order_and_read_back_in_zigbee_order() {
    let _s = sim::session();
    let ieee = Tc32Hal::new().ieee().to_le_bytes();
    assert_eq!(ieee[5..], TELINK_OUI);
    let stored = sim::with(|c| c.flash[MAC_SECTOR..MAC_SECTOR + 8].to_vec());
    assert_eq!(stored, [&ieee[2..8], &ieee[..2]].concat());
    assert_ne!(ieee[..5], [0xff; 5]);
    assert_ne!(stored[6..8], [0xff, 0xff]);
    assert!(sim::with(|c| c.flash[MAC_SECTOR - 8..MAC_SECTOR].iter().chain(&c.flash[MAC_SECTOR + 8..MAC_SECTOR + 48]).all(|b| *b == 0xff)));
}

#[test]
fn the_hal_hands_frames_and_tables_to_the_radio_driver() {
    let _s = sim::session();
    let mut hal = Tc32Hal::new();
    sim::with(|c| c.log.clear());
    let at = sim::with(|c| c.tick) / 16 + 200;
    hal.transmit_at(&[0x01, 0x08, 0x01], at);
    assert_eq!(sim::with(|c| c.tx_kicks), 1);
    let log = sim::with(|c| c.log.clone());
    let kick = log.iter().position(|o| *o == Op::W8(0xf20, 0x02)).unwrap_or(0);
    let t = log[..kick].iter().rev().find_map(|o| match *o {
        Op::R32(0x740, t) => Some(t),
        _ => None,
    });
    assert_eq!(t, Some(at * 16));
    hal.indirect_pending(&[0x1234], &[0x99]);
    hal.poll_children(&[0x4321]);
    assert_eq!(crate::radio::tests::tables(), (vec![0x1234], vec![0x99], vec![0x4321]));
    assert_eq!(hal.transmit(&[0x41, 0x88, 0x01, 0xff, 0xff, 0xff, 0xff, 0x00, 0x00]), TxStatus::Ok);
    assert_eq!(hal.tx_attempts(), 1);
    hal.set_channel(20);
    assert!(sim::with(|c| c.writes()).contains(&Op::W8(0x1244, 0x49)));
    hal.set_filter(0x1a62, 0x0001);
    assert_eq!(crate::radio::tests::filter(), (0x1a62, 0x0001, hal.ieee()));
    #[cfg(feature = "sense")]
    {
        hal.sense_listen(0x5e55);
        assert_eq!(crate::radio::tests::probe_pan(), 0x5e55);
    }
}

#[test]
fn a_received_frame_carries_its_lqi_rssi_and_end_in_microseconds() {
    let _s = sim::session();
    let mut hal = Tc32Hal::new();
    let mut buf = [0u8; 127];
    assert!(hal.receive(&mut buf).is_none());
    for (rssi_byte, rssi, lqi) in [(0x9cu8, 46i8, 255u8), (53, -57, 127), (0, -110, 0), (0xff, 127, 255), (0x01, -109, 0)] {
        crate::radio::tests::queue(&[0x00, 0x80, 0x01, 0x34, 0x12], rssi_byte, 0x1234_5678);
        let m = hal.receive(&mut buf).expect("queued");
        assert_eq!((m.len, m.rssi, m.lqi, m.end_us), (5, rssi, lqi, 0x1234_5678 / 16), "{rssi_byte}");
    }
}

#[test]
fn energy_and_noise_are_read_in_dbm_and_lqi() {
    let _s = sim::session();
    let mut hal = Tc32Hal::new();
    sim::with(|c| c.poke8(0x441, 53));
    assert_eq!(hal.energy_detect(15), 127);
    sim::with(|c| c.tick = 10_000 * 16);
    crate::radio::tests::sent_at(0);
    assert_eq!(hal.noise_dbm(), Some(-57));
    sim::with(|c| c.poke8(0x441, 0x80));
    assert_eq!(hal.noise_dbm(), Some(-128));
    crate::radio::tests::sent_at(sim::with(|c| c.tick));
    assert_eq!(hal.noise_dbm(), None);
}

#[test]
fn only_the_first_hal_gets_the_black_box() {
    let _s = sim::session();
    BLACK_BOX.release();
    let mut a = Tc32Hal::new();
    let mut b = Tc32Hal::new();
    assert!(a.black_box().is_some());
    assert!(b.black_box().is_none());
}

#[test]
fn an_ota_resume_starts_erasing_at_the_next_whole_sector() {
    let _s = sim::session();
    let mut hal = Tc32Hal::new();
    staging::release(Owner::Ble);
    for (len, erased) in [(0u32, 0u32), (1, 0x1000), (0x1000, 0x1000), (0x1001, 0x2000), (staging::SIZE, staging::SIZE)] {
        assert!(hal.ota_resume(len));
        assert_eq!(hal.staging_erased, erased, "{len}");
    }
    assert!(hal.ota_begin(staging::SIZE));
    assert_eq!(hal.staging_erased, 0);
    staging::release(Owner::Zigbee);
    assert!(staging::claim(Owner::Ble));
    assert!(!hal.ota_begin(1));
    assert!(!hal.ota_resume(1));
    hal.ota_write(0, &[1]);
    assert_eq!(sim::flash8(staging::BASE), 0xff);
    staging::release(Owner::Ble);
}

#[test]
fn an_ota_write_reaching_the_end_of_the_staging_area_is_taken() {
    let _s = sim::session();
    let mut hal = Tc32Hal::new();
    staging::release(Owner::Ble);
    assert!(hal.ota_resume(staging::SIZE - 0x1000));
    sim::with(|c| c.flash[staging::BASE + staging::SIZE as usize - 1] = 0);
    hal.ota_write(staging::SIZE - 2, &[1, 2]);
    assert_eq!(sim::with(|c| c.flash_ops.iter().filter(|o| o.0 == 0x20).count()), 1);
    assert_eq!((sim::flash8(staging::BASE + staging::SIZE as usize - 2), sim::flash8(staging::BASE + staging::SIZE as usize - 1)), (1, 2));
    hal.ota_write(staging::SIZE - 1, &[3, 4]);
    assert_eq!(sim::flash8(staging::BASE + staging::SIZE as usize - 1), 2);
    assert_eq!(hal.staging_erased, staging::SIZE);
    staging::release(Owner::Zigbee);
}

#[test]
fn activating_a_zigbee_download_releases_the_staging_area_or_restarts_into_it() {
    let _s = sim::session();
    let mut hal = Tc32Hal::new();
    staging::release(Owner::Ble);
    assert!(hal.ota_begin(0x2000));
    hal.ota_activate();
    assert!(!staging::holds(Owner::Zigbee));
    assert!(hal.ota_begin(0x2000));
    let img = crate::tests::image(0x2000);
    hal.ota_write(0, &img);
    assert!(sim::reboots(move || Tc32Hal::new().ota_activate()));
    assert_eq!(sim::flash8(staging::BASE + 8), 0x4b);
    staging::release(Owner::Zigbee);
}

#[test]
fn retained_data_is_cut_to_its_maximum_and_refused_when_longer_than_the_buffer() {
    let _s = sim::session();
    let mut hal = Tc32Hal::new();
    let long: Vec<u8> = (0..zigbee::stack::RETAIN_MAX as u32 + 5).map(|i| i as u8).collect();
    hal.retain(&long);
    assert_eq!(RETAINED.read32(4), zigbee::stack::RETAIN_MAX as u32);
    assert_eq!(RETAINED.read32(0), RETAINED_MAGIC);
    let mut buf = vec![0u8; zigbee::stack::RETAIN_MAX];
    assert_eq!(hal.retained(&mut buf), Some(zigbee::stack::RETAIN_MAX));
    assert_eq!(buf, long[..zigbee::stack::RETAIN_MAX].to_vec());
    assert_eq!(RETAINED.read32(0), 0);
    hal.retain(&[1, 2]);
    RETAINED.write32(4, zigbee::stack::RETAIN_MAX as u32 + 1);
    let mut big = vec![0u8; zigbee::stack::RETAIN_MAX + 4];
    assert_eq!(hal.retained(&mut big), None);
    hal.retain(&[1, 2]);
    assert_eq!(hal.retained(&mut [0u8; 2]), Some(2));
    hal.retain(&[1, 2]);
    RETAINED.write32(0, RETAINED_MAGIC ^ 1);
    assert_eq!(hal.retained(&mut [0u8; 2]), None);
    hal.retain(&[]);
    assert_eq!(hal.retained(&mut []), Some(0));
    assert_eq!(retained_sum(0, &[]), 0x811c_9dc5);
    assert_eq!(retained_sum(1, &[0]), 0x811c_9dc4u32.wrapping_mul(0x0100_0193));
    assert_eq!(retained_sum(2, &[1, 2]), ((0x811c_9dc7u32 ^ 1).wrapping_mul(0x0100_0193) ^ 2).wrapping_mul(0x0100_0193));
}

#[test]
fn the_counters_gather_the_radio_flash_and_ble_figures() {
    let _s = sim::session();
    let mut hal = Tc32Hal::new();
    LOOP_MAX_US.set(77);
    crate::radio::tests::set_counters(11, 12, 13, 4);
    flash::ERASES.set(21);
    flash::STORES.set(22);
    flash::ERASE_MAX_US.set(23);
    #[cfg(feature = "ble")]
    crate::ble_radio::tests::set_lent(31, 32, 33);
    let c = hal.counters();
    assert_eq!((c.cca_busy, c.rx_queue_full, c.rx_queue_peak), (11, 12, 13));
    assert_eq!(hal.tx_attempts(), 4);
    assert_eq!((c.flash_erases, c.flash_stores, c.flash_busy_max_us, c.loop_max_us), (21, 22, 23, 77));
    #[cfg(feature = "ble")]
    {
        let (l, m) = crate::ble_radio::lent();
        assert_eq!((c.ble_lends, c.ble_ms, c.ble_lend_max_us), (l, m, crate::ble_radio::lent_max_us()));
        assert_eq!((l, m, crate::ble_radio::lent_max_us()), (31, 32, 33));
        BLE_STATS.set(ble::Stats { adv_events: 1, connections: 2, events: 3, events_heard: 4, events_skipped: 5, disconnects: 6, links_lost: 7 });
        let c = hal.counters();
        assert_eq!((c.ble_connections, c.ble_disconnects, c.ble_links_lost, c.ble_events, c.ble_events_skipped), (2, 6, 7, 3, 5));
    }
}

#[cfg(feature = "ble")]
mod over_ble {
    use super::*;
    use crate::ble_radio::tests::{att, central_link as central, Central};

    fn poll_until(link: &mut Link, mut done: impl FnMut(&mut Link, Option<Command>) -> bool, what: &str) {
        for _ in 0..20_000 {
            let c = link.poll();
            if done(link, c) {
                return;
            }
        }
        panic!("{what} did not happen");
    }

    fn connected() -> (Link, Central) {
        crate::board::warm_reset();
        let mut link = Link::new();
        let air = central();
        air.borrow_mut().connect = true;
        poll_until(&mut link, |l, _| {
            sim::with(|c| c.tick = c.tick.wrapping_add(1_000 * 16));
            l.ble.as_ref().is_some_and(|b| b.link_state() == ble::LinkState::Connected)
        }, "a connection");
        (link, air)
    }

    fn request(link: &mut Link, air: &Central, pdu: &[u8]) -> Vec<u8> {
        air.borrow_mut().att_received();
        air.borrow_mut().out.push_back(att(pdu));
        let mut got = None;
        poll_until(link, |_, _| {
            got = air.borrow_mut().att_received().into_iter().find(|r| r[0] == pdu[0] + 1 || r[0] == 0x01);
            got.is_some()
        }, "an ATT response");
        got.unwrap_or_default()
    }

    fn read_long(link: &mut Link, air: &Central, handle: u16) -> Vec<u8> {
        let h = handle.to_le_bytes();
        let mut v = request(link, air, &[0x0a, h[0], h[1]])[1..].to_vec();
        while v.len() % 22 == 0 {
            let o = (v.len() as u16).to_le_bytes();
            let more = request(link, air, &[0x0c, h[0], h[1], o[0], o[1]]);
            if more[0] != 0x0d || more.len() == 1 {
                break;
            }
            v.extend_from_slice(&more[1..]);
        }
        v
    }

    #[test]
    fn an_identify_from_the_device_page_makes_the_bulb_blink() {
        let _s = sim::session();
        let (mut link, air) = connected();
        air.borrow_mut().out.push_back(att(&[0x12, 0x28, 0x00, 0x01]));
        let mut seen = false;
        poll_until(&mut link, |_, c| {
            seen |= c == Some(Command::Identify { times: 3 });
            seen
        }, "the identify command");
        let state = read_long(&mut link, &air, 0x24);
        assert_eq!(state[0], 1);
    }

    fn login(link: &mut Link, air: &Central) -> [u8; 16] {
        let pad = |s: &[u8]| {
            let mut b = [0u8; 16];
            b[..s.len()].copy_from_slice(s);
            b
        };
        let block: [u8; 16] = std::array::from_fn(|i| pad(b"unpaired")[i] ^ pad(b"1234")[i]);
        let random = [1, 2, 3, 4, 5, 6, 7, 8];
        let proof = ble::crypto::tl_aes_encrypt(&pad(&random), &block);
        let mut packet = vec![0x12, 0x1e, 0x00, 0x0c];
        packet.extend_from_slice(&random);
        packet.extend_from_slice(&proof[..8]);
        assert_eq!(request(link, air, &packet), vec![0x13]);
        let rsp = request(link, air, &[0x0a, 0x1e, 0x00]);
        assert_eq!(rsp[1], 0x0d, "the login is accepted");
        let mut both = [0u8; 16];
        both[..8].copy_from_slice(&random);
        both[8..].copy_from_slice(&rsp[2..10]);
        ble::crypto::tl_aes_encrypt(&block, &both)
    }

    fn mesh_command(link: &mut Link, key: &[u8; 16], sn: u32, op: u8, params: &[u8]) -> Vec<u8> {
        let mut addr = [0u8; 6];
        flash::read(MAC_SECTOR, &mut addr);
        let sn = &sn.to_le_bytes()[..3];
        let mut nonce = [0u8; 8];
        nonce[..4].copy_from_slice(&addr[..4]);
        nonce[4] = 1;
        nonce[5..].copy_from_slice(sn);
        let mut payload = [0u8; 15];
        payload[..2].copy_from_slice(&0xffffu16.to_le_bytes());
        payload[2] = op;
        payload[3..5].copy_from_slice(&[0x60, 0x01]);
        payload[5..5 + params.len()].copy_from_slice(params);
        let mic = ble::crypto::mesh_mic(key, &nonce, &payload);
        ble::crypto::mesh_crypt(key, &nonce, &mut payload);
        let _ = link;
        [vec![0x52, 0x15, 0x00], sn.to_vec(), mic[..2].to_vec(), payload.to_vec()].concat()
    }

    #[test]
    fn a_command_from_the_app_reaches_the_bulb_and_the_page_can_restart_it() {
        let _s = sim::session();
        let (mut link, air) = connected();
        let key = login(&mut link, &air);
        air.borrow_mut().out.push_back(att(&mesh_command(&mut link, &key, 1, 0xd0, &[1])));
        let mut on = false;
        poll_until(&mut link, |_, c| {
            on |= c == Some(Command::On);
            on
        }, "the power command");
        nv::open().store(NvItem::Network as u8, &[1, 2, 3]);
        assert_eq!(request(&mut link, &air, &[0x12, 0x28, 0x00, 0x30]), vec![0x13]);
        let t0 = link.stack.as_mut().map(|s| s.hal.now_ms()).unwrap_or(0);
        let mut last = t0;
        assert!(sim::reboots(|| poll_until(&mut link, |l, _| {
            last = l.stack.as_mut().map(|s| s.hal.now_ms()).unwrap_or(0);
            sim::with(|c| c.tick = c.tick.wrapping_add(16_000));
            false
        }, "the restart")));
        assert!((400..=510).contains(&last.wrapping_sub(t0)), "{}", last.wrapping_sub(t0));
        let mut buf = [0u8; 4];
        assert_eq!(nv::open().load(NvItem::Network as u8, &mut buf), Some(3));
    }

    fn poll_with_room(link: &mut Link, room_ms: u32) -> bool {
        for _ in 0..20_000 {
            let now = sim::with(|c| c.tick);
            let next = BLE_NEXT.get().unwrap_or(now);
            let left = next.wrapping_sub(now) as i32;
            if left > (room_ms as i32 + 1) * 16_000 {
                sim::with(|c| c.tick = next.wrapping_sub(room_ms * 16_000));
                link.diag.refresh_at = link.stack.as_mut().map(|s| s.hal.now_ms()).unwrap_or(0).wrapping_sub(1);
                let before = link.diag.refresh_at;
                let _ = link.poll();
                return link.diag.refresh_at != before;
            }
            let _ = link.poll();
        }
        panic!("no room of {room_ms} ms before a BLE event");
    }

    #[test]
    fn the_device_page_is_refreshed_only_with_6_ms_to_spare_before_the_next_ble_event() {
        let _s = sim::session();
        let (mut link, _air) = connected();
        assert!(!poll_with_room(&mut link, 5));
        assert!(poll_with_room(&mut link, 8));
        assert!(!poll_with_room(&mut link, 3));
    }

    fn network_record() -> Vec<u8> {
        let mut v = vec![1, 15];
        v.extend_from_slice(&0x1a62u16.to_le_bytes());
        v.extend_from_slice(&0x2b3cu16.to_le_bytes());
        v.extend_from_slice(&0xdddd_dddd_dddd_ddddu64.to_le_bytes());
        v.extend_from_slice(&[0, 1, 0, 0]);
        v.extend_from_slice(&[0x11; 16]);
        v.extend_from_slice(&[0, 0, 0]);
        v.extend_from_slice(&[0; 16]);
        v.extend_from_slice(&0x00ab_cdef_0000_0001u64.to_le_bytes());
        v.extend_from_slice(&[0x5a; 16]);
        v.extend_from_slice(&[0; 16]);
        v.push(0);
        v
    }

    fn link_status_from(src: u16, src_ext: u64, seq: u8, us: u16) -> Vec<u8> {
        use zigbee::{aes::Aes128, nwk, util::Buf};
        let mut f = Buf::new();
        f.bytes(&[0x41, 0x88, seq, 0x62, 0x1a, 0xff, 0xff]);
        f.bytes(&src.to_le_bytes());
        let at = f.len();
        let mut h = nwk::Header::new(nwk::FRAME_CMD, 0xfffc, src, 1, seq);
        h.src_ext = Some(src_ext);
        h.write(&mut f);
        let mut p = vec![nwk::CMD_LINK_STATUS, 0x61];
        p.extend_from_slice(&us.to_le_bytes());
        p.push(0x11);
        nwk::secure(&Aes128::new(&[0x11; 16]), 0, src_ext, seq as u32 + 100, &mut f, at, &p);
        f.as_slice().to_vec()
    }

    #[test]
    fn the_device_page_shows_the_network_the_link_and_the_resets_of_a_joined_bulb() {
        let _s = sim::session();
        nv::open().store(NvItem::Network as u8, &network_record());
        let mut child = vec![1u8];
        child.extend_from_slice(&0x00cd_0000_0000_2222u64.to_le_bytes());
        child.extend_from_slice(&0x2222u16.to_le_bytes());
        child.extend_from_slice(&[0x80, 8]);
        nv::open().store(NvItem::Children as u8, &child);
        sim::with(|c| {
            c.set_analog(0x3a, 0x01);
            c.poke8(0x72, 0x01);
        });
        crate::link::BULB_NET.write(4, 3);
        crate::board::warm_reset();
        let mut link = Link::new();
        let _ = link.poll();
        let mpdu = [0x41, 0x88, 0x07, 0x62, 0x1a, 0x3c, 0x2b, 0x00, 0x00, 0x48, 0x00];
        crate::radio::tests::queue(&mpdu, 70, 0);
        let ext = 0x0012_4b00_0102_0304u64.to_le_bytes();
        let assoc = [&[0x23u8, 0xc8, 0x08, 0x62, 0x1a, 0x3c, 0x2b, 0xff, 0xff][..], &ext, &[0x01, 0x8e]].concat();
        let poll = [&[0x63u8, 0xc8, 0x09, 0x62, 0x1a, 0x3c, 0x2b][..], &ext, &[0x04]].concat();
        for i in 0..240 {
            if i == 10 {
                crate::radio::tests::queue(&assoc, 70, 0);
            }
            if i == 20 {
                crate::radio::tests::queue(&poll, 70, 0);
            }
            if i % 20 == 5 {
                crate::radio::tests::queue(&link_status_from(0x1111, 0x00ab_0000_0000_1111, i as u8, 0x2b3c), 90, 0);
            }
            sim::with(|c| c.tick = c.tick.wrapping_add(50 * 16_000));
            for _ in 0..5 {
                let _ = link.poll();
            }
        }
        let air = central();
        air.borrow_mut().connect = true;
        poll_until(&mut link, |l, _| {
            sim::with(|c| c.tick = c.tick.wrapping_add(1_000 * 16));
            l.ble.as_ref().is_some_and(|b| b.link_state() == ble::LinkState::Connected)
        }, "a connection");
        assert!(poll_with_room(&mut link, 8));
        let state = read_long(&mut link, &air, 0x24);
        let st = link.stack.as_mut().expect("the stack");
        let (starts, wd) = st.resets();
        let n = st.neighbor_counts();
        let (lqi, rssi) = st.last_signal();
        let m = st.mac_counters();
        assert!(st.is_joined());
        assert_eq!(state[5], st.reset_cause() as u8);
        assert_eq!(state[7], wd.min(255) as u8);
        assert_eq!(state[9], 3);
        assert_eq!(state[10..12], starts.to_le_bytes());
        assert_eq!(state[12], 1);
        assert_eq!(state[13..17], [0x3c, 0x2b, 0x62, 0x1a]);
        assert_eq!(state[17], 15);
        assert_eq!(state[18..21], [n.routers.saturating_add(n.end_devices), n.routers, n.end_devices]);
        assert_eq!(state[21..23], [lqi, rssi as u8]);
        assert_eq!(state[23..27], m.rx_ucast.to_le_bytes());
        assert_eq!(state[27..31], m.tx_ucast.to_le_bytes());
        assert_eq!(state[31..33], (m.tx_ucast_retries as u16).to_le_bytes());
        assert_eq!(state[33..35], (m.tx_ucast_failures as u16).to_le_bytes());
        assert!(n.routers > 0 && n.end_devices > 0, "{n:?}");
        assert!(wd > 0 && lqi > 0 && rssi != 0 && m.rx_ucast > 0 && m.tx_ucast > 0 && m.tx_ucast_retries > 0 && m.tx_ucast_failures > 0);
    }

    #[test]
    fn a_poll_frees_the_staging_area_no_update_is_using_and_publishes_the_link_figures() {
        let _s = sim::session();
        let (mut link, _air) = connected();
        assert!(staging::claim(Owner::Ble));
        let _ = link.poll();
        assert!(!staging::holds(Owner::Ble));
        assert!(staging::claim(Owner::Zigbee));
        let _ = link.poll();
        assert!(!staging::holds(Owner::Zigbee));
        let c = link.stack.as_mut().map(|s| s.hal.counters()).unwrap_or_default();
        assert_eq!(c.ble_connections, 1);
        assert!(c.ble_events > 0);
    }

    #[test]
    fn the_device_page_reads_the_bulb_identity_its_settings_and_its_counters() {
        let _s = sim::session();
        let (mut link, air) = connected();
        let info = read_long(&mut link, &air, 0x22);
        let ieee = link.stack.as_mut().map(|s| s.hal.ieee()).unwrap_or(0);
        let model = crate::ble::IDENTITY.model.as_bytes();
        assert_eq!(info[..4], [1, BOARD.tunable() as u8, 1 | (cfg!(feature = "sense") as u8) << 1, FILE_VERSION as u8]);
        assert_eq!(info[4..12], ieee.to_le_bytes());
        assert_eq!(info[12] as usize, model.len());
        assert_eq!(&info[13..13 + model.len()], model);
        let config = read_long(&mut link, &air, 0x2c);
        let ls = link.stack.as_mut().map(|s| s.light_settings());
        assert_eq!(config[..4], [1, ls.map_or(0, |l| l.start_up_on_off), ls.map_or(0, |l| l.start_up_level), ls.map_or(0, |l| l.level_options)]);
        let counters = read_long(&mut link, &air, 0x2e);
        let mut ours = [0u8; zigbee::stack::DIAG_COUNTERS_MAX];
        let n = link.stack.as_mut().map(|s| s.diag_counters(&mut ours)).unwrap_or(0);
        assert_eq!(counters.len(), n);
        let log = read_long(&mut link, &air, 0x26);
        assert!(log[0] >= 1);
        assert_eq!(log.len(), 1 + 6 * log[0] as usize);
    }

    #[test]
    fn the_device_page_sees_the_uptime_grow_each_second_and_the_frames_heard() {
        let _s = sim::session();
        let (mut link, air) = connected();
        let up = |s: &[u8]| u32::from_le_bytes([s[1], s[2], s[3], s[4]]);
        let first = up(&read_long(&mut link, &air, 0x24));
        for _ in 0..3 {
            sim::with(|c| c.tick = c.tick.wrapping_add(1_200 * 16_000));
            poll_until(&mut link, |_, _| true, "a poll");
        }
        if let Some(s) = link.stack.as_mut() {
            s.hal.zcl_received(0x1234, 0x0006, 0x01, 0x11, 200, -40);
        }
        let later = up(&read_long(&mut link, &air, 0x24));
        assert!(later >= first + 3, "{first} {later}");
        let frames = read_long(&mut link, &air, 0x2a);
        assert!(frames[0] >= 1);
    }
}

#[cfg(feature = "ble")]
mod ble_switch {
    use super::*;
    use crate::ble_radio::tests::{adv_events, central_link as central, scan_response};
    use bulb_core::command::{ColorMode, PowerOnBehavior, Shown};

    fn settings(mode: u8, minutes: u16) {
        let m = minutes.to_le_bytes();
        nv::open().store(NvItem::Settings as u8, &[1, mode, m[0], m[1]]);
    }

    fn polls(link: &mut Link, n: usize, step_ms: u32) {
        for _ in 0..n {
            let _ = link.poll();
            sim::with(|c| c.tick = c.tick.wrapping_add(step_ms * 16_000));
        }
    }

    fn report(on: bool, level: u8) -> StateReport {
        StateReport {
            on,
            level,
            power_on_behavior: PowerOnBehavior::Previous,
            startup_level: 0,
            color_temp: 0,
            startup_color_temp: 0,
            color_mode: ColorMode::Temperature,
            hue: 0,
            sat: 0,
            x: 0,
            y: 0,
            hue_moving: false,
            level_fade: None,
            ct_fade_ms: None,
            colour_fading: false,
            candle: false,
            shown: Shown::default(),
        }
    }

    #[test]
    fn with_ble_switched_off_the_bulb_never_advertises() {
        let _s = sim::session();
        settings(2, 20);
        let mut link = Link::new();
        let n = adv_events();
        polls(&mut link, 200, 50);
        assert!(link.ble.is_none());
        assert_eq!(adv_events(), n);
        assert_eq!(BLE_NEXT.get(), None);
    }

    #[test]
    fn a_ble_window_ends_after_its_minutes_and_stays_ended() {
        let _s = sim::session();
        settings(1, 2);
        let mut link = Link::new();
        polls(&mut link, 10, 1);
        assert!(link.ble.is_some());
        assert!(!link.window_over);
        let ms = link.stack.as_mut().map(|s| s.hal.now_ms()).unwrap_or(0);
        sim::with(|c| c.tick = c.tick.wrapping_add((119_990 - ms).wrapping_mul(16_000)));
        let _ = link.poll();
        assert!(link.ble.is_some());
        sim::with(|c| c.tick = c.tick.wrapping_add(20 * 16_000));
        let _ = link.poll();
        assert!(link.ble.is_none());
        assert!(link.window_over);
        assert_eq!(BLE_NEXT.get(), None);
        assert!(staging::claim(Owner::Ble));
        staging::release(Owner::Ble);
        polls(&mut link, 5, 10);
        assert!(link.ble.is_none());
    }

    #[test]
    fn a_connected_phone_keeps_ble_past_its_window_until_the_link_is_lost() {
        let _s = sim::session();
        settings(1, 1);
        crate::board::warm_reset();
        let mut link = Link::new();
        polls(&mut link, 3, 1);
        let ms = link.stack.as_mut().map(|s| s.hal.now_ms()).unwrap_or(0);
        sim::with(|c| c.tick = c.tick.wrapping_add((59_000 - ms).wrapping_mul(16_000)));
        let air = central();
        air.borrow_mut().connect = true;
        for _ in 0..20_000 {
            let _ = link.poll();
            if link.ble.as_ref().is_some_and(|b| b.link_state() == ble::LinkState::Connected) {
                break;
            }
        }
        assert_eq!(link.ble.as_ref().map(|b| b.link_state()), Some(ble::LinkState::Connected));
        for _ in 0..200 {
            let _ = link.poll();
            sim::with(|c| c.tick = c.tick.wrapping_add(10 * 16_000));
        }
        assert!(link.window_over);
        assert!(link.ble.is_some());
        air.borrow_mut().silent = true;
        for _ in 0..2_000 {
            let _ = link.poll();
            sim::with(|c| c.tick = c.tick.wrapping_add(10 * 16_000));
            if link.ble.is_none() {
                break;
            }
        }
        assert!(link.ble.is_none());
    }

    #[test]
    fn the_light_reported_before_ble_starts_is_in_its_first_scan_response() {
        let _s = sim::session();
        let mut link = Link::new();
        link.report(&report(false, 77));
        assert_eq!(crate::link::BULB_STATE.read(2), 77);
        let n = adv_events();
        for _ in 0..20_000 {
            let _ = link.poll();
            if adv_events() != n {
                break;
            }
        }
        let rsp = scan_response();
        assert_eq!((rsp[4 + 2 + 6 + 15], rsp[4 + 2 + 6 + 16]), (0, 77));
        link.report(&report(true, 200));
        let n = adv_events();
        for _ in 0..2_000 {
            let _ = link.poll();
            sim::with(|c| c.tick = c.tick.wrapping_add(5 * 16_000));
            if adv_events() != n {
                break;
            }
        }
        let rsp = scan_response();
        assert_eq!((rsp[4 + 2 + 6 + 15], rsp[4 + 2 + 6 + 16]), (1, 200));
        assert_eq!(link.last_light.map(|l| l.level), Some(200));
    }

    #[test]
    fn zigbee_waits_while_an_advertising_event_is_less_than_4_ms_away() {
        let _s = sim::session();
        let mut link = Link::new();
        let (mut guarded, mut free) = (0, 0);
        for _ in 0..3_000 {
            crate::link::BULB_NET.write(0, 0xee);
            let r = link.poll();
            let now = sim::with(|c| c.tick);
            let left = BLE_NEXT.get().map(|n| n.wrapping_sub(now) as i32);
            if crate::link::BULB_NET.read(0) == 0xee {
                guarded += 1;
                assert_eq!(r, None);
                assert!(left.is_some_and(|l| l < ADV_GUARD as i32), "{left:?}");
            } else {
                free += 1;
                assert!(left.is_some_and(|l| l > ADV_GUARD as i32 - 2_000 * 16), "{left:?}");
            }
        }
        assert!(guarded > 0 && free > 0, "{guarded} {free}");
    }

    fn zigbee_polled_with(link: &mut Link, left_us: u32) -> bool {
        for _ in 0..20_000 {
            let now = sim::with(|c| c.tick);
            let next = BLE_NEXT.get().unwrap_or(now);
            if next.wrapping_sub(now) as i32 > (left_us as i32 + 1_000) * 16 {
                sim::with(|c| c.tick = next.wrapping_sub(left_us * 16));
                crate::link::BULB_NET.write(0, 0xee);
                let _ = link.poll();
                return crate::link::BULB_NET.read(0) != 0xee;
            }
            let _ = link.poll();
        }
        panic!("no advertising event ahead");
    }

    #[test]
    fn zigbee_is_held_back_from_4_ms_before_an_advertising_event() {
        let _s = sim::session();
        let mut link = Link::new();
        assert!(zigbee_polled_with(&mut link, 4_200));
        assert!(!zigbee_polled_with(&mut link, 3_800));
        assert!(!zigbee_polled_with(&mut link, 500));
    }

    #[test]
    fn a_debug_command_is_taken_before_anything_else() {
        let _s = sim::session();
        let mut link = Link::new();
        crate::link::BULB_STATE.write(0, 0);
        crate::link::BULB_CMD.write(0, 1);
        crate::link::BULB_CMD.copy_from(1, &[1, 0, 0, 0, 0, 0, 0]);
        assert_eq!(link.poll(), Some(Command::On));
        assert!(link.stack.is_none());
        let mut r: &mut Link = &mut link;
        assert_eq!(Radio::poll(&mut r), None);
        assert!(r.stack.is_some());
        Radio::report(&mut r, &report(true, 9));
        assert_eq!(link.light.map(|l| l.level), Some(9));
    }

    fn erases() -> Vec<usize> {
        sim::with(|c| c.flash_ops.iter().filter(|o| o.0 == 0x20).map(|o| o.1).collect())
    }

    #[test]
    fn the_link_housekeeps_only_with_no_report_or_frame_waiting_and_one_erase_at_a_time() {
        let _s = sim::session();
        let mut nv = nv::open();
        for _ in 0..19 {
            nv.store(NvItem::Network as u8, &[0x5a; 160]);
        }
        sim::with(|c| c.flash[nv::SECTORS[0] + 8] = 0);
        let mut link = Link::new();
        assert!(!Radio::housekeep(&mut link));
        let _ = link.poll();
        assert!(link.stack.is_some());
        {
            use ble::Hal as _;
            let mut hal = crate::ble::BleHal::new();
            (0..61).for_each(|k| hal.nv_store(&[k as u8; 64]));
        }
        sim::with(|c| c.flash_ops.clear());
        let mut r: &mut Link = &mut link;
        Radio::report(&mut r, &report(true, 5));
        assert!(!Radio::housekeep(&mut r));
        assert!(erases().is_empty(), "a report waits");
        let _ = link.poll();
        crate::radio::tests::queue(&[0x00, 0x80, 0x01, 0x34, 0x12], 0x9c, 0);
        assert!(!Radio::housekeep(&mut link));
        assert!(erases().is_empty(), "a frame waits");
        crate::radio::tests::reset_radio_state();
        assert!(!Radio::housekeep(&mut link));
        assert_eq!(erases(), [crate::ble::MESH_SECTORS[0]]);
        sim::with(|c| c.flash_ops.clear());
        assert!(!Radio::housekeep(&mut link));
        assert_eq!(erases(), [nv::SECTORS[0]]);
        let mut r: &mut Link = &mut link;
        assert!(Radio::housekeep(&mut r));
        assert_eq!(erases(), [nv::SECTORS[0]]);
    }
}

#[cfg(feature = "ble")]
#[test]
fn a_zcl_frame_received_is_logged_for_the_device_page_with_its_second() {
    let _s = sim::session();
    let mut hal = Tc32Hal::new();
    let n = crate::diag::tests::frames_in();
    sim::with(|c| c.tick = c.tick.wrapping_add(16_000 * 2_500));
    hal.zcl_received(0x1234, 0x0006, 0x01, 0x11, 200, -40);
    assert_eq!(crate::diag::tests::frames_in(), n.wrapping_add(1));
    assert_eq!(crate::diag::tests::last_frame_time(), hal.ms / 1000);
}

#[cfg(feature = "trace")]
#[test]
fn a_zigbee_trace_of_a_transmission_gets_the_radio_figures() {
    use zigbee::hal::{trace, Trace};
    let _s = sim::session();
    let mut hal = Tc32Hal::new();
    crate::trace::tests::rewind();
    let t0 = sim::with(|c| c.tick);
    radio::LAST_TX.set([3, 2, t0]);
    for (kind, detail, filled) in [(trace::TX, trace::TX_BUSY, true), (trace::TX, 0, true), (trace::TX, trace::TX_BUSY + 1, false), (trace::TX + 1, 0, false)] {
        hal.trace(Trace { kind, detail, seq: 1, src: 2, peer: 3, aux: 0xdead_beef });
        let n = crate::trace::tests::count() - 1;
        let aux = crate::trace::tests::entry(n)[3];
        if filled {
            assert_eq!(aux & 0xffff, 0x0203, "{kind} {detail}");
        } else {
            assert_eq!(aux, 0xdead_beef, "{kind} {detail}");
        }
    }
}

struct FuzzRng(u64);

impl FuzzRng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

fn staged_marked_and_bad() -> Option<String> {
    let base = staging::BASE;
    let f = sim::with(|c| c.flash[base..base + staging::SIZE as usize].to_vec());
    if f[8] == 0xff {
        return None;
    }
    if f[8] != 0x4b {
        return Some(format!("a torn mark {:#x}", f[8]));
    }
    let size = u32::from_le_bytes([f[0x18], f[0x19], f[0x1a], f[0x1b]]) as usize;
    if &f[9..12] != b"NLT" || !(0x40..=staging::SIZE as usize).contains(&size) {
        return Some(format!("marked, header bad, size {size:#x}"));
    }
    let id = &f[0x12..0x16];
    let ours = [0x41, 0x11, crate::board::BOARD.image_type as u8, (crate::board::BOARD.image_type >> 8) as u8];
    if id != ours && id != [0, 0, 0, 0] {
        return Some(format!("marked, image of another board {id:02x?}"));
    }
    let crc = crate::tests::crc32(&f[..size - 4]);
    if f[size - 4..size] != crc.to_le_bytes() {
        return Some("marked, the CRC in flash is wrong".into());
    }
    None
}

fn cut_at(writes: usize) -> std::rc::Rc<std::cell::RefCell<Option<Vec<u8>>>> {
    let n = std::rc::Rc::new(std::cell::Cell::new(0usize));
    let shot: std::rc::Rc<std::cell::RefCell<Option<Vec<u8>>>> = Default::default();
    let s2 = shot.clone();
    sim::with(|c| {
        c.on(move |chip, op| {
            if matches!(op, Op::W8(a, _) if a == crate::reg::MSPI_DATA) {
                n.set(n.get() + 1);
                if n.get() == writes {
                    *s2.borrow_mut() = Some(chip.flash.clone());
                }
            }
        })
    });
    shot
}

fn staging_case(seed: u64) -> Result<(usize, usize, usize), String> {
    let mut r = FuzzRng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut zhal = Tc32Hal::new();
    let mut bhal = crate::ble::BleHal::new();
    use ble::Hal as _;
    let ty = crate::board::BOARD.image_type;
    let mut images: Vec<Vec<u8>> = (0..4)
        .map(|k| {
            let span = if r.below(4) == 0 { 0x3000 } else { 0x300 };
            let size = 0x40 + r.below(span) as usize;
            let mut v = crate::tests::image_of(size, if k == 3 { ty ^ 1 } else { ty });
            if k == 2 {
                v[0x12..0x16].fill(0);
                let mut sum = v[..size - 4].to_vec();
                sum[8] = 0x4b;
                let crc = crate::tests::crc32(&sum);
                v[size - 4..].copy_from_slice(&crc.to_le_bytes());
            }
            v
        })
        .collect();
    let bad = images[0].clone();
    let mut broken = bad.clone();
    let i = 0x20 + r.below((broken.len() - 0x24) as u64) as usize;
    broken[i] ^= 1;
    images.push(broken);
    let (mut zimg, mut zoff, mut bimg, mut boff) = (0usize, 0usize, 0usize, 0usize);
    let (mut cuts, mut marks, mut ops) = (0, 0, 0);
    for _ in 0..60 + r.below(200) {
        ops += 1;
        let cut = r.below(3) == 0;
        let shot = cut.then(|| cut_at(1 + r.below(400) as usize));
        let choice = r.below(12);
        let rebooted = sim::reboots(|| match choice {
            0 => {
                zimg = r.below(images.len() as u64) as usize;
                zoff = 0;
                let _ = zhal.ota_begin(images[zimg].len() as u32);
            }
            1 => {
                let _ = zhal.ota_resume(zoff as u32);
            }
            2 | 3 => {
                let img = &images[zimg];
                let n = (if r.below(3) == 0 { 0x10000 } else { 1 + r.below(200) as usize }).min(img.len() - zoff.min(img.len()));
                if n > 0 {
                    let mut chunk = img[zoff..zoff + n].to_vec();
                    if zoff <= 8 && zoff + n > 8 {
                        chunk[8 - zoff] = 0xff;
                    }
                    zhal.ota_write(zoff as u32, &chunk);
                    zoff += n;
                }
            }
            4 => zhal.ota_activate(),
            5 => {
                if r.below(2) == 0 {
                    bimg = r.below(images.len() as u64) as usize;
                    boff = 0;
                }
                let _ = bhal.ota_capacity();
            }
            6 => bhal.ota_erase((r.below(staging::SIZE as u64 + 0x2000) as u32) & !0xfff),
            7 | 8 => {
                let img = &images[bimg];
                let n = (if r.below(3) == 0 { 0x1000 - boff % 0x1000 } else { 1 + r.below(64) as usize }).min(img.len() - boff.min(img.len()));
                if n > 0 {
                    if boff % 0x1000 == 0 {
                        bhal.ota_erase(boff as u32);
                    }
                    let mut chunk = img[boff..boff + n].to_vec();
                    if boff <= 8 && boff + n > 8 {
                        chunk[8 - boff] = 0xff;
                    }
                    bhal.ota_write(boff as u32, &chunk);
                    boff += n;
                }
            }
            9 => {
                let _ = bhal.ota_activate();
            }
            10 => staging::release(Owner::Zigbee),
            _ => staging::release(Owner::Ble),
        });
        sim::with(|c| c.hook = None);
        let snap = shot.and_then(|s| s.borrow_mut().take());
        if let Some(fl) = snap {
            sim::with(|c| c.flash = fl);
            cuts += 1;
        }
        if let Some(e) = staged_marked_and_bad() {
            return Err(format!("seed {seed} op {ops} (choice {choice}, cut {cut}, rebooted {rebooted}): {e}"));
        }
        if rebooted || snap_taken(cut) {
            if sim::flash8(staging::BASE + 8) == 0x4b {
                marks += 1;
                sim::with(|c| c.flash[staging::BASE..staging::BASE + 0x1000].fill(0xff));
            }
            crate::tests::reset_statics();
            zhal = Tc32Hal::new();
            bhal = crate::ble::BleHal::new();
        }
    }
    Ok((ops, cuts, marks))
}

fn snap_taken(cut: bool) -> bool {
    cut
}

#[test]
fn fuzz_staging_owners_and_power_cuts() {
    let cases: u64 = std::env::var("FUZZ_CASES").ok().and_then(|s| s.parse().ok()).unwrap_or(20);
    let start: u64 = std::env::var("FUZZ_START").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
    let (mut o, mut c, mut m, mut fails) = (0, 0, 0, Vec::new());
    for seed in start..start + cases {
        let _s = sim::session();
        match staging_case(seed) {
            Ok((a, b, d)) => {
                o += a;
                c += b;
                m += d;
            }
            Err(e) => fails.push(e),
        }
    }
    eprintln!("{cases} staging cases, {} failed; {o} operations, {c} power cuts, {m} images marked", fails.len());
    for f in fails.iter().take(20) {
        eprintln!("  {f}");
    }
    assert!(fails.is_empty());
}
