use memcap as _;
mod sim;

use std::cell::RefCell;
use std::rc::Rc;

use ble::crypto::{mesh_crypt, mesh_mic};
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
    assert_eq!(rigs[1].ble.borrow().air_stats().received, 3);
}

#[test]
fn a_command_goes_on_air_as_often_as_every_other_while_the_app_is_answered() {
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
    send(&mut rigs, 0x0055, OP_POWER, &[0], 800);
    assert!(rigs[0].is_on());
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
    const READ_AT: u64 = 200;
    let mut missed = vec![];
    for offset in (0..320).step_by(4) {
        let (_ether, mut rigs) = bulbs(&[A, B]);
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
    for after in [20, 50, 80] {
        let (_ether, mut rigs) = bulbs(&[A, B]);
        let seen = rigs[1].world.borrow().advertised_at.len();
        while rigs[1].world.borrow().advertised_at.len() == seen {
            step(&mut rigs, 1);
        }
        let last = *rigs[1].world.borrow().advertised_at.last().unwrap();
        rigs[1].world.borrow_mut().zigbee_busy.push((last + 295 * MS, last + 325 * MS, true));
        step(&mut rigs, after);
        send(&mut rigs, 0xffff, OP_POWER, &[0], 1500);
        let next = rigs[1].world.borrow().advertised_at.iter().copied().find(|t| *t > last).unwrap();
        assert!(next > last + 400 * MS, "the event was left out");
        assert!(!rigs[1].is_on(), "B heard the command sent {after} ms after its window");
    }
}

fn step(rigs: &mut [Rig], ms: u64) {
    for _ in 0..ms {
        let now = rigs[0].world.borrow().now + MS;
        for rig in rigs.iter_mut() {
            rig.run_until(now);
        }
    }
}

fn air_pdu(sn: u32, src: u16, dst: u16, op: u8, params: &[u8], ltk: &[u8; 16]) -> Vec<u8> {
    let mut p = vec![0u8; 39];
    p[0] = 0x82;
    p[1] = 0x25;
    p[2] = 0x21;
    p[4..6].copy_from_slice(&[0x03, 0xff]);
    p[6..8].copy_from_slice(&src.to_le_bytes());
    p[9..12].copy_from_slice(&sn.to_le_bytes()[..3]);
    p[12..14].copy_from_slice(&src.to_le_bytes());
    p[14..16].copy_from_slice(&dst.to_le_bytes());
    p[16] = op;
    p[17..19].copy_from_slice(&[0x60, 0x01]);
    p[19..19 + params.len()].copy_from_slice(params);
    let nonce: [u8; 8] = p[8..16].try_into().unwrap();
    let mic = mesh_mic(ltk, &nonce, &p[16..35]);
    p[35..].copy_from_slice(&mic[..4]);
    mesh_crypt(ltk, &nonce, &mut p[16..35]);
    p
}

fn alone() -> (Rc<RefCell<Ether>>, Rig) {
    let ether = Rc::new(RefCell::new(Ether::default()));
    let rig = paired(&ether, B, NAME, PASSWORD, LTK);
    (ether, rig)
}

#[test]
fn a_packet_from_the_air_is_carried_out() {
    let (ether, mut rig) = alone();
    ether.borrow_mut().deliver(0, &air_pdu(7, 0x0021, 0xffff, 0xc0 | 0x10, &[0], &LTK));
    rig.run(1000);
    assert!(!rig.is_on());
    assert_eq!(rig.ble.borrow().air_stats().received, 1);
}

#[test]
fn packets_that_are_not_the_mesh_format_are_rejected() {
    let good = air_pdu(7, 0x0021, 0xffff, 0xc0 | 0x10, &[0], &LTK);
    let mut wrong: Vec<(Vec<u8>, &str)> = vec![(good[..30].to_vec(), "short")];
    for (i, v, what) in [(1, 0x24, "length"), (0, 0x83, "advertising type"), (2, 0x20, "AD type"), (3, 0x01, "second octet of the AD")] {
        let mut p = good.clone();
        p[i] = v;
        wrong.push((p, what));
    }
    let mut plain = good.clone();
    plain[0] = 0x02;
    wrong.push((plain, "not encrypted"));
    let mut damaged = good.clone();
    damaged[20] ^= 1;
    wrong.push((damaged, "MIC"));
    for (pdu, what) in wrong {
        let (ether, mut rig) = alone();
        ether.borrow_mut().deliver(0, &pdu);
        rig.run(1000);
        assert!(rig.is_on(), "{what}");
        let stats = rig.ble.borrow().air_stats();
        assert_eq!((stats.received, stats.rejected), (0, 1), "{what}");
    }
}

#[test]
fn packets_of_other_kinds_are_dropped_without_a_word() {
    let good = air_pdu(7, 0x0021, 0xffff, 0xc0 | 0x10, &[0], &LTK);
    let mut quiet: Vec<(Vec<u8>, &str)> = vec![];
    for kind in [[0xff, 0xee], [0xff, 0xff]] {
        let mut p = good.clone();
        p[4..6].copy_from_slice(&kind);
        quiet.push((p, "a kind the stock leaves alone"));
    }
    for op in [0x10, 0x40 | 0x10, 0x80 | 0x10] {
        quiet.push((air_pdu(7, 0x0021, 0xffff, op, &[0], &LTK), "an opcode without both top bits"));
    }
    let mut vendor = good.clone();
    vendor[17] = 0x61;
    let nonce: [u8; 8] = good[8..16].try_into().unwrap();
    let mut body = good[16..35].to_vec();
    mesh_crypt(&LTK, &nonce, &mut body);
    body[1] = 0x61;
    let mic = mesh_mic(&LTK, &nonce, &body);
    mesh_crypt(&LTK, &nonce, &mut body);
    vendor[16..35].copy_from_slice(&body);
    vendor[35..].copy_from_slice(&mic[..4]);
    quiet.push((vendor, "another vendor"));
    for (pdu, what) in quiet {
        let (ether, mut rig) = alone();
        ether.borrow_mut().deliver(0, &pdu);
        rig.run(1000);
        assert!(rig.is_on(), "{what}");
        let stats = rig.ble.borrow().air_stats();
        assert_eq!((stats.received, stats.rejected), (0, 0), "{what}");
    }
}

fn ask_b(rig: &mut Rig) {
    let packet = rig.command_packet(0x000b, OP_STATUS, &[0x10]);
    rig.central().send_att(&[&[0x52, COMMAND as u8, 0][..], &packet].concat());
    rig.run(100);
}

fn answer_from_b(rig: &mut Rig, sn: u32) {
    let ether = rig.world.borrow().ether.clone().unwrap();
    ether.borrow_mut().deliver(0, &air_pdu(sn, 0x000b, 0x000a, 0xc0 | 0x1b, &[1, 30, 64], &LTK));
}

fn from_b(rig: &mut Rig) -> usize {
    rig.statuses_from().iter().filter(|s| (s.0, s.2) == (0xdb, 0x000b)).count()
}

#[test]
fn an_answer_from_the_air_goes_to_the_app_when_it_has_room() {
    let (_ether, mut rigs) = bulbs(&[A]);
    let rig = &mut rigs[0];
    ask_b(rig);
    answer_from_b(rig, 0x40);
    rig.run(400);
    assert_eq!(from_b(rig), 1);
    ask_b(rig);
    rig.central().silent = true;
    for _ in 0..5 {
        rig.bulb.handle(bulb_core::Command::Toggle);
        rig.run(260);
    }
    answer_from_b(rig, 0x41);
    rig.run(50);
    assert_eq!(rig.ble.borrow().air_stats().to_app, 2);
    rig.central().silent = false;
    rig.run(800);
    assert_eq!(from_b(rig), 1, "the answer that found the queue full came later");
}

#[test]
fn the_app_in_its_own_format_gets_no_mesh_answers() {
    let (_ether, mut rigs) = bulbs(&[A]);
    let key = rigs[0].key.unwrap();
    let mut plain = [0u8; 16];
    plain[1..4].copy_from_slice(&[2, 0x0a, 0]);
    plain[0] = ble::crypto::crc8_6c(&plain[1..4]);
    let packet = [&[2u8][..], &ble::crypto::tl_aes_encrypt(&key, &plain)].concat();
    rigs[0].write(COMMAND, &packet);
    rigs[0].run(400);
    ask_b(&mut rigs[0]);
    rigs[0].central().notifications.clear();
    answer_from_b(&mut rigs[0], 0x40);
    rigs[0].run(400);
    assert_eq!(from_b(&mut rigs[0]), 0);
    assert_eq!(rigs[0].ble.borrow().air_stats().to_app, 1, "heard, not passed on");
}

#[test]
fn the_last_eight_packets_are_remembered_and_not_carried_out_twice() {
    let (ether, mut rig) = alone();
    let pdu = |sn: u32| air_pdu(sn, 0x0021, 0xffff, 0xc0 | 0x10, &[0], &LTK);
    let received = |rig: &Rig| rig.ble.borrow().air_stats().received;
    for sn in 1..=8 {
        ether.borrow_mut().deliver(0, &pdu(sn));
        rig.run(400);
    }
    assert_eq!(received(&rig), 8);
    for sn in [1, 8, 4] {
        ether.borrow_mut().deliver(0, &pdu(sn));
        rig.run(400);
    }
    assert_eq!(received(&rig), 8, "all eight still remembered");
    ether.borrow_mut().deliver(0, &air_pdu(1, 0x0022, 0xffff, 0xc0 | 0x10, &[0], &LTK));
    ether.borrow_mut().deliver(0, &air_pdu(1, 0x0021, 0xffff, 0xc0 | 0x11, &[0], &LTK));
    rig.run(400);
    assert_eq!(received(&rig), 10, "another source, another opcode: other packets");
    ether.borrow_mut().deliver(0, &pdu(3));
    rig.run(400);
    assert_eq!(received(&rig), 10, "the eighth newest");
    ether.borrow_mut().deliver(0, &pdu(2));
    rig.run(400);
    assert_eq!(received(&rig), 11, "the ninth newest was forgotten");
}

#[test]
fn a_connected_bulb_waiting_for_answers_stops_listening_3_ms_before_its_next_event() {
    let ether = Rc::new(RefCell::new(Ether::default()));
    let mut rig = paired(&ether, A, NAME, PASSWORD, LTK);
    rig.central().params.interval = 8;
    rig.connect();
    assert!(rig.login(b"EgloMesh", b"s3cret"));
    let from = rig.world.borrow().now;
    ask_b(&mut rig);
    rig.run(1000);
    let w = rig.world.borrow();
    let gaps: Vec<i64> = w
        .listened
        .iter()
        .filter(|l| l.0 >= from)
        .filter_map(|&(_, end, _)| w.event_starts.iter().find(|&&s| s > end).map(|&s| s as i64 - end as i64))
        .collect();
    assert!(gaps.len() > 50, "{} windows", gaps.len());
    let least = *gaps.iter().min().unwrap();
    assert_eq!(least, 3 * MS as i64, "1.5 ms of margin before the 1.5 ms the event is prepared in");
}

#[test]
fn a_bulb_alone_listens_12_ms_after_each_advertising_event_on_the_next_channel() {
    let (_ether, mut rig) = alone();
    rig.run(2000);
    let w = rig.world.borrow();
    assert!(w.listened.len() >= 6);
    for (i, (&(from, to, channel), adv)) in w.listened.iter().zip(&w.advertised_at).enumerate() {
        assert_eq!(to - from, 12 * MS);
        assert_eq!(channel, [2, 12, 23, 34][i % 4]);
        assert_eq!(from, adv + 3 * MS, "right after the advertising event, which takes 3 ms here");
    }
}
