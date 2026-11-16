//! The mesh between bulbs: the app's commands reach bulbs it is not
//! connected to, and their answers come back.

mod sim;

use std::cell::RefCell;
use std::rc::Rc;

use sim::{run_all, Ether, Rig, DIMMABLE, MS};

const COMMAND: u16 = 0x15;
const OP_POWER: u8 = 0xd0;
const OP_STATUS: u8 = 0xda;
const OP_BRIGHTNESS: u8 = 0xf1;

const A: [u8; 6] = [0x0a, 0x22, 0x11, 0x38, 0xc1, 0xa4];
const B: [u8; 6] = [0x0b, 0x22, 0x11, 0x38, 0xc1, 0xa4];
const C: [u8; 6] = [0x0c, 0x22, 0x11, 0x38, 0xc1, 0xa4];

const NAME: &[u8; 16] = b"EgloMesh\0\0\0\0\0\0\0\0";
const PASSWORD: &[u8; 16] = b"s3cret\0\0\0\0\0\0\0\0\0\0";
const LTK: [u8; 16] = *b"0123456789abcdef";

/// A bulb that an app paired into a network before (settings as the firmware stores them).
fn paired(ether: &Rc<RefCell<Ether>>, address: [u8; 6], name: &[u8; 16], password: &[u8; 16], ltk: [u8; 16]) -> Rig {
    let rig = Rig::in_ether(ether, address, DIMMABLE);
    let mut nv = vec![0xa7];
    nv.extend_from_slice(name);
    nv.extend_from_slice(password);
    nv.extend_from_slice(&ltk);
    nv.extend_from_slice(&[0, 0, 1]);
    rig.world.borrow_mut().nv = Some(nv);
    rig.restart()
}

/// Bulb A with the app connected and logged in, and its neighbours, all in one mesh network.
fn bulbs(addresses: &[[u8; 6]]) -> (Rc<RefCell<Ether>>, Vec<Rig>) {
    let ether = Rc::new(RefCell::new(Ether::default()));
    let mut rigs: Vec<Rig> = addresses.iter().map(|a| paired(&ether, *a, NAME, PASSWORD, LTK)).collect();
    rigs[0].connect();
    assert!(rigs[0].login(b"EgloMesh", b"s3cret"));
    rigs[0].enable_status();
    let now = rigs[0].world.borrow().now;
    for rig in rigs.iter_mut() {
        rig.run_until(now);
    }
    (ether, rigs)
}

fn send(rigs: &mut [Rig], dst: u16, op: u8, params: &[u8], ms: u64) {
    let packet = rigs[0].command_packet(dst, op, params);
    rigs[0].central().send_att(&[&[0x52, COMMAND as u8, 0][..], &packet].concat());
    let mut all: Vec<&mut Rig> = rigs.iter_mut().collect();
    run_all(&mut all, ms);
}

#[test]
fn a_command_for_everyone_reaches_the_neighbour() {
    let (_ether, mut rigs) = bulbs(&[A, B]);
    send(&mut rigs, 0xffff, OP_POWER, &[0], 800);
    assert!(!rigs[0].is_on() && !rigs[1].is_on());
    send(&mut rigs, 0xffff, OP_BRIGHTNESS, &[50], 800);
    send(&mut rigs, 0xffff, OP_POWER, &[1], 800);
    assert_eq!((rigs[1].is_on(), rigs[1].level()), (true, 100));
    // every packet arrives many times; it is carried out once
    assert_eq!(rigs[1].ble.borrow().air_stats().received, 3);
}

#[test]
fn a_command_goes_on_air_as_often_as_every_other_while_the_app_is_answered() {
    // The connected bulb tells the app its new state; until the central has
    // acknowledged that packet the radio cannot send a burst. The bursts
    // left out are not counted, and after such a gap the train goes on for
    // a listener's period at least: a listener's window could have fallen
    // into it, and its next one comes up to 310 ms later (emulator,
    // sims::mesh).
    let (ether, mut rigs) = bulbs(&[A, B]);
    let before = ether.borrow().sent_by(0, |_| true);
    send(&mut rigs, 0xffff, OP_BRIGHTNESS, &[50], 1500);
    assert_eq!(rigs[1].level(), rigs[0].level());
    let sent = ether.borrow().sent_by(0, |_| true) - before;
    assert!(sent >= 64 * 4, "every burst of the train on all four channels: {sent}");
    let bursts = ether.borrow().bursts_of(0);
    let gap_end = bursts.windows(2).filter(|w| w[1] - w[0] > 30 * MS).map(|w| w[1]).last();
    let gap_end = gap_end.expect("the answer to the app held the radio");
    let last = *bursts.last().unwrap();
    assert!(last - gap_end >= 310 * MS, "the train ended {} ms after the gap", (last - gap_end) / MS);
}

#[test]
fn a_command_for_one_bulb_is_carried_out_by_that_bulb_only() {
    let (_ether, mut rigs) = bulbs(&[A, B]);
    send(&mut rigs, 0x000b, OP_POWER, &[0], 800);
    assert!(rigs[0].is_on(), "the connected bulb passes it on and stays as it is");
    assert!(!rigs[1].is_on());
    // nobody has this address
    send(&mut rigs, 0x0055, OP_POWER, &[0], 800);
    assert!(rigs[0].is_on());
    // 0: the connected bulb only, nothing goes on air
    let bursts = rigs[0].ble.borrow().air_stats().bursts;
    send(&mut rigs, 0x0000, OP_POWER, &[0], 800);
    assert!(!rigs[0].is_on());
    assert_eq!(rigs[0].ble.borrow().air_stats().bursts, bursts);
}

#[test]
fn a_status_request_is_answered_through_the_connected_bulb() {
    let (_ether, mut rigs) = bulbs(&[A, B]);
    send(&mut rigs, 0x000b, OP_BRIGHTNESS, &[30], 800);
    rigs[0].statuses();
    send(&mut rigs, 0x000b, OP_STATUS, &[0x10], 1500);
    let answers = rigs[0].statuses_from();
    assert_eq!(answers.len(), 1, "one answer, though it was on air many times");
    let (op, params, src) = &answers[0];
    assert_eq!((*op, *src), (0xdb, 0x000b));
    assert_eq!(params[..3], [1, 30, 64], "B's state: on, brightness 30");
    assert_eq!(rigs[0].ble.borrow().air_stats().to_app, 1);
}

#[test]
fn a_bulb_in_between_passes_packets_on() {
    let (ether, mut rigs) = bulbs(&[A, B, C]);
    // A and C do not hear each other
    ether.borrow_mut().blocked.push((0, 2));
    send(&mut rigs, 0x000c, OP_POWER, &[0], 1500);
    assert!(rigs[0].is_on() && rigs[1].is_on());
    assert!(!rigs[2].is_on(), "the command reached C through B");
    assert!(rigs[1].ble.borrow().air_stats().relayed >= 1);
    rigs[0].statuses();
    send(&mut rigs, 0x000c, OP_STATUS, &[0x10], 2500);
    let answers = rigs[0].statuses_from();
    assert_eq!(answers.len(), 1);
    assert_eq!((answers[0].0, answers[0].2, answers[0].1[0]), (0xdb, 0x000c, 0), "C's answer came back: off");
}

/// Bulb A with the app connected, next to bulb B whose stored network differs.
fn with_a_stranger(name: &[u8; 16], password: &[u8; 16], ltk: [u8; 16]) -> Vec<Rig> {
    let ether = Rc::new(RefCell::new(Ether::default()));
    let mut a = paired(&ether, A, NAME, PASSWORD, LTK);
    let mut b = paired(&ether, B, name, password, ltk);
    a.connect();
    assert!(a.login(b"EgloMesh", b"s3cret"));
    let now = a.world.borrow().now;
    b.run_until(now);
    vec![a, b]
}

#[test]
fn a_bulb_of_another_network_does_not_listen() {
    let mut rigs = with_a_stranger(b"elsewhere\0\0\0\0\0\0\0", b"password\0\0\0\0\0\0\0\0", LTK);
    send(&mut rigs, 0xffff, OP_POWER, &[0], 1000);
    assert!(!rigs[0].is_on());
    assert!(rigs[1].is_on(), "another name and password: another access code on air");
    let stats = rigs[1].ble.borrow().air_stats();
    assert_eq!((stats.received, stats.rejected), (0, 0));
}

#[test]
fn a_packet_under_another_key_is_rejected() {
    // the same name and password (so the same access code), another long-term key
    let mut rigs = with_a_stranger(NAME, PASSWORD, [0x11; 16]);
    send(&mut rigs, 0xffff, OP_POWER, &[0], 1000);
    assert!(!rigs[0].is_on());
    assert!(rigs[1].is_on());
    let stats = rigs[1].ble.borrow().air_stats();
    assert_eq!(stats.received, 0);
    assert!(stats.rejected > 0, "B heard the packets and their checksum did not match");
}

#[test]
fn bulbs_in_the_factory_network_leave_the_radio_to_zigbee() {
    // two bulbs fresh from the factory: the same name, password and key
    let ether = Rc::new(RefCell::new(Ether::default()));
    let mut rigs = vec![Rig::in_ether(&ether, A, DIMMABLE), Rig::in_ether(&ether, B, DIMMABLE)];
    rigs[0].connect();
    assert!(rigs[0].login(b"unpaired", b"1234"));
    let now = rigs[0].world.borrow().now;
    rigs[1].run_until(now);
    send(&mut rigs, 0xffff, OP_POWER, &[0], 1000);
    assert!(!rigs[0].is_on(), "the connected bulb obeys");
    assert!(rigs[1].is_on());
    for rig in &rigs {
        let stats = rig.ble.borrow().air_stats();
        assert_eq!((stats.bursts, stats.received, stats.rejected), (0, 0, 0));
        assert_eq!(rig.world.borrow().mesh_listens, 0, "no listening either");
    }
}

#[test]
fn a_neighbour_whose_window_falls_into_a_gap_of_the_train_hears_it_at_its_next() {
    // While the radio holds an answer for the app (until the central has
    // acknowledged it, at the next connection event but one) the connected
    // bulb sends no burst. A neighbour listens 12 ms after each of its
    // advertising events, 300 to 309 ms apart; when its window falls into
    // that gap, the train must still be on air at its next window. Here the
    // app reads a characteristic during the train, and the neighbour's
    // windows are moved round the whole period: before, the train ended
    // before the next window when the gap came late in it (emulator,
    // sims::mesh on CI).
    const READ_AT: u64 = 200;
    let mut missed = vec![];
    for offset in (0..320).step_by(4) {
        let (_ether, mut rigs) = bulbs(&[A, B]);
        // one of B's advertising events, then its next one (and its window)
        // about `offset` ms after the command
        let seen = rigs[1].world.borrow().advertised_at.len();
        while rigs[1].world.borrow().advertised_at.len() == seen {
            step(&mut rigs, 1);
        }
        step(&mut rigs, 300 - offset.min(300));
        let packet = rigs[0].command_packet(0xffff, OP_POWER, &[0]);
        rigs[0].central().send_att(&[&[0x52, COMMAND as u8, 0][..], &packet].concat());
        step(&mut rigs, READ_AT);
        rigs[0].central().send_att(&[0x0a, 0x03, 0x00]);
        step(&mut rigs, 1500 - READ_AT);
        assert!(!rigs[0].is_on());
        if rigs[1].is_on() {
            missed.push(offset);
        }
    }
    assert!(missed.is_empty(), "B missed the command with its window this many ms after it: {missed:?}");
}

#[test]
fn a_neighbour_that_leaves_out_an_advertising_event_still_listens() {
    // An advertising event that falls due while the Zigbee neighbours are
    // talking is left out (`ll.rs`), and the next one comes 300 ms later. Its
    // listening window is only put off until they are quiet: without it the
    // bulb would not listen for 600 ms, longer than a train (emulator,
    // sims::mesh with the bulbs' Zigbee network steering).
    for after in [20, 50, 80] {
        let (_ether, mut rigs) = bulbs(&[A, B]);
        let seen = rigs[1].world.borrow().advertised_at.len();
        while rigs[1].world.borrow().advertised_at.len() == seen {
            step(&mut rigs, 1);
        }
        let last = *rigs[1].world.borrow().advertised_at.last().unwrap();
        // B's next event, 300 to 309 ms after its last, while its neighbours talk
        rigs[1].world.borrow_mut().zigbee_busy.push((last + 295 * MS, last + 325 * MS, true));
        step(&mut rigs, after);
        send(&mut rigs, 0xffff, OP_POWER, &[0], 1500);
        let next = rigs[1].world.borrow().advertised_at.iter().copied().find(|t| *t > last).unwrap();
        assert!(next > last + 400 * MS, "the event was left out");
        assert!(!rigs[1].is_on(), "B heard the command sent {after} ms after its window");
    }
}

/// Runs the bulbs side by side for `ms`, in steps of a millisecond.
fn step(rigs: &mut [Rig], ms: u64) {
    for _ in 0..ms {
        let now = rigs[0].world.borrow().now + MS;
        for rig in rigs.iter_mut() {
            rig.run_until(now);
        }
    }
}
