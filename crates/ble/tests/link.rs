//! Link layer: advertising, connection timing and the central's control procedures.

mod sim;

use ble::LinkState;
use sim::{Params, Rig, DIMMABLE, MS, UNIT};

fn fields(adv: &[u8]) -> Vec<(u8, Vec<u8>)> {
    let mut out = vec![];
    let mut d = &adv[8..];
    while d.len() >= 2 {
        let n = d[0] as usize;
        out.push((d[1], d[2..1 + n].to_vec()));
        d = &d[1 + n..];
    }
    out
}

#[test]
fn advertises_like_the_stock_bulb() {
    let mut rig = Rig::new(DIMMABLE);
    rig.central().scan = true;
    rig.run(3050);
    let c = rig.central();
    // every 300..305 ms
    assert!((10..=11).contains(&c.advertisements.len()), "{} advertising events in 3 s", c.advertisements.len());
    let _ = &c;
    let adv = &c.advertisements[0];
    assert_eq!(adv[0], 0x00, "ADV_IND from a public address");
    assert_eq!(adv[2..8], sim::ADDRESS);
    assert_eq!(
        fields(adv),
        [
            (0x01, vec![0x05]),
            // the GAP service as an incomplete list, as the stock bulb sends it (byte for byte in the emulator)
            (0x02, vec![0x00, 0x18]),
            (0x09, b"unpaired".to_vec()),
            // AwoX: vendor 0x0160, product, low four bytes of the address
            (0xff, vec![0x60, 0x01, 0xa7, 0x20, 0x33, 0x22, 0x11, 0x38]),
        ]
    );
    let rsp = &c.scan_responses[0];
    assert_eq!(rsp[0], 0x04, "SCAN_RSP");
    // the stock bulb's scan response: AwoX data of 29 octets, which the app
    // needs to list the bulb (product, MAC, firmware 3.0.2, mesh address 0,
    // on at full, no temperature, hue or saturation shown)
    let mut stock = vec![0x60, 0x01, 0xa7, 0x20, 0x33, 0x22, 0x11, 0x38, 0x03, 0x00, 0x02, 0x00, 0x00];
    stock.extend_from_slice(&[0x01, 0xfe, 0xff, 0xff, 0xff, 0xff, 0x04, 0x3f, 0x20, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(fields(rsp), [(0xff, stock)]);
}

#[test]
fn the_limited_discoverable_mode_ends_after_180_s() {
    let mut rig = Rig::new(DIMMABLE);
    rig.central().scan = true;
    rig.run(179_800);
    let flags = |rig: &Rig| fields(rig.central().advertisements.last().unwrap())[0].clone();
    assert_eq!(flags(&rig), (0x01, vec![0x05]), "limited discoverable from power-up, as the stock");
    // Vol 3 Part C 9.2.3.2: no longer than T_GAP(lim_adv_timeout) = 180 s; general discoverable after it
    // (the first event after 180 s comes within one advertising interval, at most 310 ms)
    rig.run(200 + 310);
    assert_eq!(flags(&rig), (0x01, vec![0x06]));
    let n = rig.central().advertisements.len();
    rig.run(3000);
    assert!(rig.central().advertisements.len() >= n + 9, "still advertising");
    assert_eq!(flags(&rig), (0x01, vec![0x06]));
    rig.connect();
    assert_eq!(rig.state(), LinkState::Connected);
}

#[test]
fn follows_the_central_through_the_connection() {
    let mut rig = Rig::new(DIMMABLE);
    rig.connect();
    rig.run(3000);
    let c = rig.central();
    assert!(c.served > 90, "{} events served in 3 s at 30 ms", c.served);
    assert_eq!(c.missed, 0, "no event of the central went unanswered");
    // hop 7 over all channels
    assert_eq!(c.channels[..6], [7, 14, 21, 28, 35, 5]);
    drop(c);
    assert_eq!(rig.ble.borrow().stats.events_skipped, 0);
    // advertising stops while connected
    let n = rig.central().advertisements.len();
    rig.run(1000);
    assert_eq!(rig.central().advertisements.len(), n);
}

#[test]
fn finds_the_first_packet_anywhere_in_the_transmit_window() {
    for (offset, size, place) in [(0u16, 1u8, 0u64), (0, 2, 2 * MS), (3, 4, 4 * MS + MS / 2), (10, 1, UNIT - MS / 10)] {
        let mut rig = Rig::new(DIMMABLE);
        rig.central().params = Params { win_offset: offset, win_size: size, ..Params::default() };
        rig.central().place_in_window = place;
        rig.connect();
        rig.run(1000);
        let c = rig.central();
        assert_eq!(c.missed, 0, "offset {offset} size {size} place {place}");
        assert!(c.served > 30);
    }
}

#[test]
fn keeps_up_with_a_central_whose_clock_drifts() {
    // the central claims 250 ppm (SCA 1); this side's own crystal may add 100
    for ppm in [-330, 330] {
        let mut rig = Rig::new(DIMMABLE);
        // 1 s interval: 330 us of drift between events
        rig.central().params = Params { interval: 800, timeout: 1000, ..Params::default() };
        rig.central().ppm = ppm;
        rig.connect();
        rig.run(20_000);
        let c = rig.central();
        assert_eq!(c.missed, 0, "{ppm} ppm");
        assert!(c.served >= 19);
    }
}

#[test]
fn a_connection_update_takes_effect_at_its_instant() {
    let mut rig = Rig::new(DIMMABLE);
    rig.connect();
    rig.central().update_connection(3, 5, 80, 300, 8);
    rig.central().place_in_window = 2 * MS;
    rig.run(3000);
    let served = rig.central().served;
    assert_eq!(rig.central().missed, 0, "the bulb moved to the new timing with the central");
    // 100 ms interval now
    rig.run(2000);
    let c = rig.central();
    assert_eq!(c.missed, 0);
    assert!((19..=21).contains(&(c.served - served)), "{} events in 2 s", c.served - served);
}

#[test]
fn a_channel_map_update_takes_effect_at_its_instant() {
    let mut rig = Rig::new(DIMMABLE);
    rig.connect();
    let map = [0x00, 0xfe, 0x03, 0x00, 0x00];
    rig.central().update_channel_map(map, 10);
    rig.run(3000);
    let c = rig.central();
    assert_eq!(c.missed, 0);
    assert!(c.channels[c.channels.len() - 50..].iter().all(|ch| (9..=17).contains(ch)), "only channels of the new map");
}

#[test]
fn the_central_ends_the_connection_and_can_come_back() {
    let mut rig = Rig::new(DIMMABLE);
    rig.connect();
    rig.central().terminate();
    rig.run(200);
    assert!(rig.central().ended, "the bulb acknowledged the LL_TERMINATE_IND");
    assert_eq!(rig.state(), LinkState::Advertising);
    let n = rig.central().advertisements.len();
    rig.run(700);
    assert!(rig.central().advertisements.len() >= n + 2, "advertising again");
    rig.connect();
    rig.run(500);
    assert_eq!(rig.central().missed, 0);
}

#[test]
fn a_central_that_went_away_is_dropped_after_the_supervision_timeout() {
    let mut rig = Rig::new(DIMMABLE);
    rig.central().params.timeout = 100;
    rig.connect();
    rig.central().silent = true;
    rig.run(900);
    assert_eq!(rig.state(), LinkState::Connected, "within the timeout the bulb keeps listening");
    rig.run(300);
    assert_eq!(rig.state(), LinkState::Advertising);
    assert_eq!(rig.ble.borrow().stats.disconnects, 1);
}

#[test]
fn a_connection_that_never_starts_is_given_up_after_six_intervals() {
    let mut rig = Rig::new(DIMMABLE);
    // the CONNECT_IND goes out, but nothing follows
    rig.central().connect = true;
    rig.central().silent = true;
    rig.run(100);
    assert_eq!(rig.state(), LinkState::Connected);
    rig.run(200);
    assert_eq!(rig.state(), LinkState::Advertising, "30 ms interval: gone well before the 5 s supervision timeout");
    // Vol 6 Part B 4.5.2: lost after six connection events, not in a seventh
    assert_eq!(rig.ble.borrow().stats.events, 6);
}

#[test]
fn answers_the_control_procedures_of_a_central() {
    let mut rig = Rig::new(DIMMABLE);
    rig.connect();
    for (request, response) in [
        // LL_FEATURE_REQ: no optional features
        (vec![0x08, 1, 0, 0, 0, 0, 0, 0, 0], vec![0x09, 0, 0, 0, 0, 0, 0, 0, 0]),
        // LL_VERSION_IND: Bluetooth 4.2, Telink's company identifier
        (vec![0x0c, 0x0b, 0x0f, 0x00, 0x34, 0x12], vec![0x0c, 0x08, 0x11, 0x02, 0x01, 0x00]),
        // procedures of features this device does not claim: LL_PING_REQ, LL_LENGTH_REQ, LL_PHY_REQ
        (vec![0x12], vec![0x07, 0x12]),
        (vec![0x14, 251, 0, 0x48, 0x08, 251, 0, 0x48, 0x08], vec![0x07, 0x14]),
        (vec![0x16, 2, 2], vec![0x07, 0x16]),
        // LL_ENC_REQ: no encryption
        ([&[0x03u8][..], &[0u8; 22][..]].concat(), vec![0x0d, 0x1a]),
    ] {
        rig.central().control.clear();
        rig.central().send_control(&request);
        rig.run(200);
        assert_eq!(rig.central().control, [response], "answer to {request:02x?}");
    }
    assert_eq!(rig.state(), LinkState::Connected);
}

#[test]
fn the_version_is_exchanged_once_and_odd_control_packets_are_refused() {
    let mut rig = Rig::new(DIMMABLE);
    rig.connect();
    for _ in 0..2 {
        rig.central().send_control(&[0x0c, 0x0b, 0x0f, 0x00, 0x34, 0x12]);
        rig.run(200);
    }
    assert_eq!(rig.central().control.len(), 1, "one LL_VERSION_IND per connection");
    // LL_CONNECTION_PARAM_REQ, here of a wrong length: a procedure this device does not have
    rig.central().control.clear();
    rig.central().send_control(&[0x0f; 20]);
    rig.run(200);
    assert_eq!(rig.central().control, [vec![0x07, 0x0f]]);
    assert_eq!(rig.state(), LinkState::Connected);
}

#[test]
fn asks_for_a_slower_connection_once() {
    let mut rig = Rig::new(DIMMABLE);
    rig.connect();
    rig.run(4000);
    assert!(rig.central().signaling.is_empty(), "not in the first five seconds of a connection");
    rig.run(1500);
    {
        let c = rig.central();
        assert_eq!(c.signaling.len(), 1);
        // Connection Parameter Update Request: 50 ms, no latency, 4 s timeout
        assert_eq!(c.signaling[0][..4], [0x12, 0x01, 0x08, 0x00]);
        assert_eq!(c.signaling[0][4..], [40, 0, 40, 0, 0, 0, 0x90, 0x01]);
    }
    rig.central().send_l2cap(5, &[0x13, 0x01, 0x02, 0x00, 0x00, 0x00]);
    rig.run(3000);
    assert_eq!(rig.central().signaling.len(), 1);

    // a central that already uses 50 ms is not asked
    let mut rig = Rig::new(DIMMABLE);
    rig.central().params.interval = 40;
    rig.connect();
    rig.run(6500);
    assert!(rig.central().signaling.is_empty());
}

#[test]
fn refuses_pairing_and_unknown_signaling() {
    let mut rig = Rig::new(DIMMABLE);
    rig.connect();
    rig.central().send_l2cap(6, &[0x01, 0x03, 0x00, 0x01, 0x10, 0x00, 0x00]);
    rig.central().send_l2cap(5, &[0x14, 0x07, 0x00, 0x00]);
    rig.run(300);
    {
        let c = rig.central();
        assert_eq!(c.smp, [vec![0x05, 0x05]], "Pairing Failed: pairing not supported");
        assert_eq!(c.signaling[0], [0x01, 0x07, 0x02, 0x00, 0x00, 0x00], "Command Reject with the request's identifier");
    }
    // any other security manager command gets the same answer, a Pairing Failed none
    rig.central().smp.clear();
    rig.central().send_l2cap(6, &[0x0b, 0x01]);
    rig.central().send_l2cap(6, &[0x05, 0x08]);
    // reserved codes are no commands: ignored
    for code in [0x00, 0x0f, 0x40, 0xff] {
        rig.central().send_l2cap(6, &[code, 0x01, 0x02]);
    }
    // a request with an odd code (credit based connection), and a response nobody asked for
    rig.central().signaling.clear();
    rig.central().send_l2cap(5, &[0x17, 0x09, 0x0a, 0x00, 0x80, 0x00, 0x40, 0x00, 0x40, 0x00, 0x01, 0x00, 0x40, 0x00]);
    rig.central().send_l2cap(5, &[0x15, 0x0a, 0x0a, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    rig.run(300);
    let c = rig.central();
    assert_eq!(c.smp, [vec![0x05, 0x05]]);
    assert_eq!(c.signaling, [vec![0x01, 0x09, 0x02, 0x00, 0x00, 0x00]]);
}

/// The central queues a change with its instant eight events ahead, and the
/// bulb hears nothing until `lead` events before that instant.
fn deaf_until(rig: &mut Rig, lead: u16, change: impl FnOnce(&mut sim::Central)) {
    rig.run(300);
    let instant = {
        let mut c = rig.central();
        change(&mut c);
        c.silent = true;
        c.event.wrapping_add(8)
    };
    while rig.central().event != instant.wrapping_sub(lead) {
        rig.run(2);
    }
    rig.central().silent = false;
}

#[test]
fn a_change_first_heard_just_before_or_on_its_instant_still_takes_effect() {
    for lead in [1u16, 0] {
        // new connection parameters: 100 ms interval
        let mut rig = Rig::new(DIMMABLE);
        rig.connect();
        deaf_until(&mut rig, lead, |c| c.update_connection(2, 0, 80, 300, 8));
        let served = rig.central().served;
        rig.run(3000);
        assert_eq!(rig.state(), LinkState::Connected, "lead {lead}");
        let c = rig.central();
        assert_eq!(c.missed, 0, "lead {lead}: the bulb is on the new timing");
        assert!((28..=32).contains(&(c.served - served)), "lead {lead}: {} events in 3 s", c.served - served);
        drop(c);

        // a new channel map
        let mut rig = Rig::new(DIMMABLE);
        rig.connect();
        deaf_until(&mut rig, lead, |c| c.update_channel_map([0x00, 0xfe, 0x03, 0x00, 0x00], 8));
        rig.run(3000);
        let c = rig.central();
        // the event the change was heard in may still have been on the old map
        assert!(c.missed <= 1, "lead {lead}: {} events missed after the change", c.missed);
        assert!(c.channels[c.channels.len() - 50..].iter().all(|ch| (9..=17).contains(ch)), "lead {lead}");
    }
}

#[test]
fn the_supervision_timeout_starts_over_with_new_parameters() {
    let mut rig = Rig::new(DIMMABLE);
    // 1 s interval; the new timeout (500 ms) is shorter than that
    rig.central().params = Params { interval: 800, timeout: 600, ..Params::default() };
    rig.connect();
    rig.run(1500);
    rig.central().update_connection(2, 0, 40, 50, 3);
    rig.run(6000);
    assert_eq!(rig.state(), LinkState::Connected);
    let c = rig.central();
    assert!(c.served > 40 && c.missed == 0, "{} served, {} missed", c.served, c.missed);
}

#[test]
fn a_central_unheard_until_the_window_would_be_half_an_interval_is_dropped() {
    let mut rig = Rig::new(DIMMABLE);
    // 7.5 ms interval, 20 s timeout: at 350 ppm the window reaches 3.6 ms after about ten seconds
    rig.central().params = Params { interval: 6, timeout: 2000, ..Params::default() };
    rig.connect();
    rig.central().silent = true;
    rig.run(8000);
    assert_eq!(rig.state(), LinkState::Connected);
    rig.run(4000);
    assert_eq!(rig.state(), LinkState::Advertising, "long before the supervision timeout");
}

#[test]
fn the_bulb_gives_up_its_own_termination_after_the_supervision_timeout() {
    let mut rig = Rig::new(DIMMABLE);
    rig.central().params.timeout = 200;
    rig.connect();
    assert!(rig.login(b"unpaired", b"1234"));
    // a mesh reset makes the bulb end the connection; the central is gone and never acknowledges
    let packet = rig.command_packet(0xffff, 0xe3, &[0]);
    rig.central().send_att(&[&[0x52, 0x15, 0x00][..], &packet].concat());
    rig.run(200);
    rig.central().silent = true;
    rig.run(1500);
    assert_eq!(rig.state(), LinkState::Connected, "still waiting for the acknowledgement");
    rig.run(1200);
    assert_eq!(rig.state(), LinkState::Advertising);
}

#[test]
fn a_long_name_is_advertised_as_shortened() {
    let mut rig = Rig::new(DIMMABLE);
    let mut nv = vec![0xa7];
    nv.extend_from_slice(b"SixteenByteName!");
    nv.extend_from_slice(b"password\0\0\0\0\0\0\0\0");
    nv.extend_from_slice(&[0x11; 16]);
    nv.extend_from_slice(&[0, 0, 1]);
    rig.world.borrow_mut().nv = Some(nv);
    let mut rig = rig.restart();
    rig.central().scan = true;
    rig.run(400);
    let c = rig.central();
    assert_eq!(fields(c.advertisements.last().unwrap())[2], (0x08, b"SixteenByteN".to_vec()), "Shortened Local Name");
    // the scan response carries the AwoX data, not the name
    assert_eq!(fields(c.scan_responses.last().unwrap())[0].0, 0xff);
}

#[test]
fn a_busy_main_loop_costs_events_but_not_the_connection() {
    let mut rig = Rig::new(DIMMABLE);
    rig.connect();
    rig.run(500);
    // a flash erase, a long Zigbee transmission
    rig.stall(200);
    rig.run(1000);
    let missed = rig.central().missed;
    assert!((5..=8).contains(&missed), "{missed} events lost in 200 ms at 30 ms");
    rig.run(2000);
    assert_eq!(rig.central().missed, missed, "back on the central's channel and time afterwards");
    assert_eq!(rig.state(), LinkState::Connected);
    assert!(rig.ble.borrow().stats.events_skipped >= 5);
    // and the attribute protocol still works
    assert_eq!(rig.read(0x0b), b"AwoX");
}

#[test]
fn a_change_whose_instant_has_passed_ends_the_connection() {
    let mut rig = Rig::new(DIMMABLE);
    rig.connect();
    rig.run(300);
    let event = rig.central().event;
    let mut c = vec![0x00, 2, 0, 0, 40, 0, 0, 0, 0x90, 0x01];
    c.extend_from_slice(&event.wrapping_sub(5).to_le_bytes());
    rig.central().send_control(&c);
    rig.run(300);
    assert_eq!(rig.state(), LinkState::Advertising);
}
