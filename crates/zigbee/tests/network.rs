use bulb_core::command::Command;
use crate::sim::*;
use zigbee::hal::TxStatus;
use zigbee::nwk;
use zigbee::stack::State;

const ON_OFF: u16 = 0x0006;
const LEVEL: u16 = 0x0008;

#[test]
fn joins_announces_and_exchanges_the_link_key() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    assert_eq!(short, 0x7A21);
    assert_eq!(net.nodes[n].stack.pan_id(), PAN);
    assert_eq!(net.nodes[n].stack.channel(), 11);
    let d = net.coord.device(short).unwrap();
    assert!(d.announced && d.verified);
    assert_eq!(net.coord.mic_failures, 0);
    net.run(17_000);
    assert!(net.coord.device(short).unwrap().link_status);
}

#[test]
fn key_exchange_also_works_with_the_key_transport_key() {
    let mut net = Net::new();
    net.coord.link_key_transport_id = 2;
    let n = net.join_node();
    assert!(net.coord.device(net.nodes[n].short()).unwrap().verified);
}

#[test]
fn old_trust_center_gets_no_key_request() {
    let mut net = Net::new();
    net.coord.stack_revision = 20;
    let n = net.join_node();
    net.run(10_000);
    let short = net.nodes[n].short();
    assert!(!net.coord.device(short).unwrap().verified);
    assert_eq!(net.nodes[n].stack.state(), State::Up);
}

#[test]
fn does_not_join_a_closed_network() {
    let mut net = Net::new();
    net.coord.permit_join = false;
    let n = net.add_node();
    net.run(30_000);
    assert_ne!(net.nodes[n].stack.state(), State::Up);
    assert!(net.coord.devices.is_empty());
    net.coord.permit_join = true;
    net.run_until(30_000, "join after permit", |net| net.nodes[n].stack.state() == State::Up);
}

#[test]
fn scans_all_channels_to_find_the_network() {
    let mut net = Net::new();
    net.air.borrow_mut().ports[0].channel = 24;
    let n = net.join_node();
    assert_eq!(net.nodes[n].stack.channel(), 24);
}

#[test]
fn on_off_and_level_commands_reach_the_light_and_are_answered() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();

    let t = net.now();
    let seq = net.coord.zcl(short, ON_OFF, 0x01, 0x00, &[]);
    net.run(50);
    assert_eq!(net.nodes[n].commands.last(), Some(&Command::Off));
    assert_eq!(net.coord.last_zcl(short, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x00, 0x00]);
    assert_eq!(net.coord.aps_acks, 1, "the request asked for an APS acknowledgement");
    let id = net.nodes[n].id;
    let sent: Vec<usize> = {
        let air = net.air.borrow();
        air.log.iter().enumerate().filter(|(_, (at, from, f))| *at >= t && *from == id && f[0] & 0x20 != 0 && f[5..7] == [0, 0]).map(|(i, _)| i).collect()
    };
    let took = *net.nodes[n].command_frames.last().unwrap();
    assert_eq!(sent.iter().filter(|&&i| i < took).count(), 1, "only the APS acknowledgement before the light changed: {sent:?}, {took}");

    net.coord.zcl(short, LEVEL, 0x01, 0x04, &[127, 5, 0, 0, 0]);
    net.run(50);
    assert_eq!(net.nodes[n].commands.last(), Some(&Command::MoveToLevel { level: 127, transition_ms: 500 }));
    assert!(net.nodes[n].on);

    net.coord.zcl(short, ON_OFF, 0x01, 0x02, &[]);
    net.run(50);
    assert!(!net.nodes[n].on);
    let before = net.nodes[n].commands.len();
    net.coord.zcl(short, LEVEL, 0x01, 0x00, &[200, 0, 0, 0, 0]);
    net.run(50);
    assert_eq!(net.nodes[n].commands.len(), before);

    let answers = net.coord.zcl_rx.len();
    net.coord.zcl(short, ON_OFF, 0x11, 0x01, &[]);
    net.run(50);
    assert!(net.nodes[n].on);
    assert_eq!(net.coord.zcl_rx.len(), answers);

    let seq = net.coord.zcl(short, ON_OFF, 0x11, 0x77, &[]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x77, 0x81]);
}

#[test]
fn level_move_step_and_stop() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    net.coord.zcl(short, LEVEL, 0x01, 0x04, &[100, 0, 0, 0, 0]);
    net.run(50);
    net.coord.zcl(short, LEVEL, 0x01, 0x02, &[0, 20, 10, 0, 0, 0]);
    net.run(50);
    assert_eq!(net.nodes[n].commands.last(), Some(&Command::MoveToLevel { level: 120, transition_ms: 1000 }));
    net.coord.zcl(short, LEVEL, 0x01, 0x01, &[1, 10, 0, 0]);
    net.run(50);
    assert_eq!(net.nodes[n].commands.last(), Some(&Command::MoveToLevel { level: 1, transition_ms: 11_900 }));
    net.coord.zcl(short, LEVEL, 0x01, 0x03, &[0, 0]);
    net.run(50);
    assert_eq!(net.nodes[n].commands.last(), Some(&Command::StopTransition));
}

#[test]
fn reads_basic_and_light_attributes() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let seq = net.coord.zcl(short, 0x0000, 0x00, 0x00, &[0x04, 0x00, 0x05, 0x00, 0x34, 0x12]);
    net.run(50);
    let rsp = net.coord.last_zcl(short, 0x0000, 0x01).unwrap();
    let mut want = vec![0x18, seq, 0x01, 0x04, 0x00, 0x00, 0x42, 4];
    want.extend_from_slice(b"AwoX");
    want.extend_from_slice(&[0x05, 0x00, 0x00, 0x42, 15]);
    want.extend_from_slice(b"ESMLFzm_w6_Dimm");
    want.extend_from_slice(&[0x34, 0x12, 0x86]);
    assert_eq!(rsp, want);

    net.coord.zcl(short, LEVEL, 0x01, 0x04, &[42, 0, 0, 0, 0]);
    net.run(50);
    let seq = net.coord.zcl(short, LEVEL, 0x00, 0x00, &[0x00, 0x00]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, LEVEL, 0x01).unwrap(), vec![0x18, seq, 0x01, 0x00, 0x00, 0x00, 0x20, 42]);
    let seq = net.coord.zcl(short, ON_OFF, 0x00, 0x00, &[0x00, 0x00]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, ON_OFF, 0x01).unwrap(), vec![0x18, seq, 0x01, 0x00, 0x00, 0x00, 0x10, 1]);
}

#[test]
fn writes_startup_attributes_and_rejects_read_only_ones() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let seq = net.coord.zcl(short, ON_OFF, 0x00, 0x02, &[0x03, 0x40, 0x30, 0x01]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, ON_OFF, 0x04).unwrap(), vec![0x18, seq, 0x04, 0x00]);
    assert!(net.nodes[n].commands.contains(&Command::SetPowerOnBehavior(bulb_core::PowerOnBehavior::On)));
    let seq = net.coord.zcl(short, ON_OFF, 0x00, 0x02, &[0x00, 0x00, 0x10, 0x01, 0x03, 0x40, 0x20, 0x01]);
    net.run(50);
    assert_eq!(
        net.coord.last_zcl(short, ON_OFF, 0x04).unwrap(),
        vec![0x18, seq, 0x04, 0x88, 0x00, 0x00, 0x8D, 0x03, 0x40]
    );
}

#[test]
fn interview_descriptors() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let s = short.to_le_bytes();
    let seq = net.coord.zdo(short, 0x0005, &s);
    net.run(50);
    assert_eq!(net.coord.last_zdo(short, 0x8005).unwrap(), vec![seq, 0x00, s[0], s[1], 9, 1, 2, 3, 4, 5, 6, 7, 8, 242]);
    for (ep, device, cluster) in [(2u8, 0x000Cu16, 0x000Cu16), (7, 0x0005, 0x0010), (8, 0x0005, 0x000D)] {
        let seq = net.coord.zdo(short, 0x0004, &[s[0], s[1], ep]);
        net.run(50);
        let d = device.to_le_bytes();
        let c = cluster.to_le_bytes();
        assert_eq!(
            net.coord.last_zdo(short, 0x8004).unwrap(),
            vec![seq, 0x00, s[0], s[1], 10, ep, 0x04, 0x01, d[0], d[1], 1, 1, c[0], c[1], 0]
        );
    }
    let seq = net.coord.zdo(short, 0x0004, &[s[0], s[1], 1]);
    net.run(50);
    assert_eq!(
        net.coord.last_zdo(short, 0x8004).unwrap(),
        vec![
            seq, 0x00, s[0], s[1], 26, 1, 0x04, 0x01, 0x01, 0x01, 1, 8, 0x00, 0x00, 0x03, 0x00, 0x04, 0x00, 0x05, 0x00,
            0x06, 0x00, 0x08, 0x00, 0x05, 0x0B, 0x00, 0x10, 1, 0x19, 0x00
        ]
    );
    let seq = net.coord.zdo(short, 0x0002, &s);
    net.run(50);
    let nd = net.coord.last_zdo(short, 0x8002).unwrap();
    assert_eq!(&nd[..7], &[seq, 0x00, s[0], s[1], 0x01, 0x40, 0x8E]);
    assert_eq!(&nd[7..9], &[0x41, 0x11]);
    for (endpoint, status) in [(9, 0x83), (0, 0x82), (255, 0x82)] {
        let seq = net.coord.zdo(short, 0x0004, &[s[0], s[1], endpoint]);
        net.run(50);
        assert_eq!(net.coord.last_zdo(short, 0x8004).unwrap(), vec![seq, status, s[0], s[1], 0]);
    }
    net.coord.zdo_rx.clear();
    net.coord.zdo(0xFFFD, 0x0004, &[s[0], s[1], 9]);
    net.run(200);
    assert!(net.coord.last_zdo(short, 0x8004).is_none());
    net.run(16_000);
    let seq = net.coord.zdo(short, 0x0031, &[0]);
    net.run(50);
    let lqi = net.coord.last_zdo(short, 0x8031).unwrap();
    assert_eq!(&lqi[..5], &[seq, 0x00, 1, 0, 1]);
    assert_eq!(&lqi[5..13], &EXT_PAN.to_le_bytes());
    assert_eq!(&lqi[13..21], &COORD_EXT.to_le_bytes());
    assert_eq!(&lqi[21..24], &[0x00, 0x00, 0x04]);
}

fn bind(net: &mut Net, n: usize, cluster: u16) -> Vec<u8> {
    let short = net.nodes[n].short();
    let ext = net.air.borrow().ports[net.nodes[n].id].ext;
    let mut p = ext.to_le_bytes().to_vec();
    p.push(1);
    p.extend_from_slice(&cluster.to_le_bytes());
    p.push(3);
    p.extend_from_slice(&COORD_EXT.to_le_bytes());
    p.push(1);
    net.coord.zdo(short, 0x0021, &p);
    net.run(50);
    net.coord.last_zdo(short, 0x8021).unwrap()
}

#[test]
fn reports_bound_attributes_on_change_and_periodically() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    assert_eq!(bind(&mut net, n, ON_OFF)[1], 0x00);
    let seq = net.coord.zcl(short, ON_OFF, 0x00, 0x06, &[0x00, 0x00, 0x00, 0x10, 0, 0, 30, 0]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, ON_OFF, 0x07).unwrap(), vec![0x18, seq, 0x07, 0x00]);
    net.run(200);
    let before = net.coord.zcl_rx.iter().filter(|e| e.2.get(2) == Some(&0x0A)).count();
    net.coord.zcl(short, ON_OFF, 0x11, 0x00, &[]);
    net.run(200);
    let report = net.coord.last_zcl(short, ON_OFF, 0x0A).unwrap();
    assert_eq!(&report[2..], &[0x0A, 0x00, 0x00, 0x10, 0x00]);
    let after = net.coord.zcl_rx.iter().filter(|e| e.2.get(2) == Some(&0x0A)).count();
    assert_eq!(after, before + 1);
    net.run(29_000);
    assert_eq!(net.coord.zcl_rx.iter().filter(|e| e.2.get(2) == Some(&0x0A)).count(), after);
    net.run(2_000);
    assert_eq!(net.coord.zcl_rx.iter().filter(|e| e.2.get(2) == Some(&0x0A)).count(), after + 1);
    let seq = net.coord.zcl(short, ON_OFF, 0x00, 0x08, &[0x00, 0x00, 0x00]);
    net.run(50);
    assert_eq!(
        net.coord.last_zcl(short, ON_OFF, 0x09).unwrap(),
        vec![0x18, seq, 0x09, 0x00, 0x00, 0x00, 0x00, 0x10, 0, 0, 30, 0]
    );
    let seq = net.coord.zcl(short, ON_OFF, 0x00, 0x06, &[0x00, 0x03, 0x40, 0x30, 0, 0, 30, 0]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, ON_OFF, 0x07).unwrap(), vec![0x18, seq, 0x07, 0x8C, 0x00, 0x03, 0x40]);
}

#[test]
fn groups_receive_group_casts() {
    let mut net = Net::new();
    let a = net.join_node();
    let b = net.join_node();
    let short = net.nodes[a].short();
    let seq = net.coord.zcl(short, 0x0004, 0x01, 0x00, &[0x34, 0x12, 0]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, 0x0004, 0x00).unwrap(), vec![0x19, seq, 0x00, 0x00, 0x34, 0x12]);
    net.coord.zcl_group(0x1234, ON_OFF, 0x00, &[]);
    net.run(300);
    assert!(!net.nodes[a].on, "the group member switches off");
    assert!(net.nodes[b].on, "the other node ignores the group");
    assert_eq!(net.nodes[a].commands.iter().filter(|c| **c == Command::Off).count(), 1, "relayed copies are dropped");
    let seq = net.coord.zcl(short, 0x0004, 0x01, 0x02, &[0]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, 0x0004, 0x02).unwrap(), vec![0x19, seq, 0x02, 7, 1, 0x34, 0x12]);
    net.coord.zcl(short, 0x0004, 0x01, 0x03, &[0x34, 0x12]);
    net.run(50);
    net.coord.zcl_group(0x1234, ON_OFF, 0x01, &[]);
    net.run(300);
    assert!(!net.nodes[a].on);
}

#[test]
fn state_survives_a_power_cycle() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    assert_eq!(bind(&mut net, n, ON_OFF)[1], 0x00);
    net.coord.zcl(short, 0x0004, 0x01, 0x00, &[0x34, 0x12, 0]);
    net.run(100);
    let joins = net.coord.devices.len();
    net.power_cycle(n);
    assert_eq!(net.nodes[n].stack.state(), State::Up, "resumes without rejoining");
    assert_eq!(net.nodes[n].short(), short);
    net.run(2_000);
    assert_eq!(net.coord.devices.len(), joins);
    let seq = net.coord.zcl(short, ON_OFF, 0x01, 0x00, &[]);
    net.run(100);
    assert_eq!(net.coord.last_zcl(short, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x00, 0x00]);
    let seq = net.coord.zcl(short, 0x0004, 0x01, 0x02, &[0]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, 0x0004, 0x02).unwrap(), vec![0x19, seq, 0x02, 7, 1, 0x34, 0x12]);
    let seq = net.coord.zdo(short, 0x0033, &[0]);
    net.run(50);
    assert_eq!(&net.coord.last_zdo(short, 0x8033).unwrap()[..5], &[seq, 0x00, 1, 0, 1]);
}

#[test]
fn frame_counter_writes_are_rare() {
    let mut net = Net::new();
    let n = net.join_node();
    let id = net.nodes[n].id;
    let short = net.nodes[n].short();
    let writes = net.air.borrow().ports[id].nv_writes;
    for _ in 0..50 {
        net.coord.zcl(short, ON_OFF, 0x01, 0x02, &[]);
        net.run(20);
    }
    net.run(60_000);
    assert_eq!(net.air.borrow().ports[id].nv_writes, writes, "ordinary traffic must not wear the flash");
}

#[test]
fn replayed_and_forged_frames_are_dropped() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    net.run(100);
    let t = net.now();
    net.coord.zcl(short, ON_OFF, 0x11, 0x00, &[]);
    net.run(50);
    assert!(!net.nodes[n].on);
    net.coord.zcl(short, ON_OFF, 0x11, 0x01, &[]);
    net.run(50);
    assert!(net.nodes[n].on);
    net.run(4_000);
    let off = net.sent_by(0, t).into_iter().next().unwrap();
    let replays = net.nodes[n].stack.stats.replays;
    net.air.borrow_mut().transmit(0, &off);
    net.run(50);
    assert!(net.nodes[n].on, "a replayed frame must not switch the light");
    assert_eq!(net.nodes[n].stack.stats.replays, replays + 1);
    let mut forged = off.clone();
    let last = forged.len() - 1;
    forged[last] ^= 0x55;
    forged[9 + 8 + 1] = forged[9 + 8 + 1].wrapping_add(10);
    let bad = net.nodes[n].stack.stats.bad_mic;
    net.air.borrow_mut().transmit(0, &forged);
    net.run(50);
    assert!(net.nodes[n].on);
    assert_eq!(net.nodes[n].stack.stats.bad_mic, bad + 1);
    let commands = net.nodes[n].commands.len();
    let mut h = net.coord.header(zigbee::nwk::FRAME_DATA, short, 30);
    h.security = false;
    let aps = net.coord.aps_frame(0x00, 1, ON_OFF, 0x0104, 1, &[0x11, 1, 0x00]);
    net.coord.nwk_send(h, &aps, None);
    net.run(50);
    assert_eq!(net.nodes[n].commands.len(), commands);
}

#[test]
fn network_key_update_and_switch() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let ext = net.air.borrow().ports[net.nodes[n].id].ext;
    let new_key = [0x5A; 16];
    net.coord.transport_network_key(short, ext, &new_key, 1, true);
    net.run(100);
    net.coord.switch_key(1);
    net.coord.key = new_key;
    net.coord.key_seq = 1;
    net.run(100);
    let seq = net.coord.zcl(short, ON_OFF, 0x01, 0x00, &[]);
    net.run(100);
    assert_eq!(net.coord.last_zcl(short, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x00, 0x00]);
    assert_eq!(net.coord.mic_failures, 0);
    net.power_cycle(n);
    net.run(1_000);
    let seq = net.coord.zcl(short, ON_OFF, 0x01, 0x01, &[]);
    net.run(100);
    assert_eq!(net.coord.last_zcl(short, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x01, 0x00]);
}

#[test]
fn leaves_on_request_and_can_join_again() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let ext = net.air.borrow().ports[net.nodes[n].id].ext;
    let mut p = ext.to_le_bytes().to_vec();
    p.push(0);
    let seq = net.coord.zdo(short, 0x0034, &p);
    net.run(100);
    assert_eq!(net.coord.last_zdo(short, 0x8034).unwrap(), vec![seq, 0x00]);
    net.run(1_000);
    assert_ne!(net.nodes[n].stack.state(), State::Up);
    assert!(net.coord.nwk_cmd_rx.iter().any(|(s, c)| *s == short && c[0] == zigbee::nwk::CMD_LEAVE));
    net.coord.permit_join = false;
    net.power_cycle(n);
    assert_eq!(net.nodes[n].stack.state(), State::Down);
    net.coord.permit_join = true;
    net.run_until(30_000, "second join", |net| {
        net.nodes[n].stack.state() == State::Up && net.coord.devices.iter().any(|d| d.ext == ext && d.verified)
    });
}

#[test]
fn nwk_leave_request_is_obeyed() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    net.coord.permit_join = false;
    let before = net.coord.nwk_cmd_rx.len();
    net.coord.nwk_command(short, 1, &[zigbee::nwk::CMD_LEAVE, 0x40]);
    net.run(1_000);
    assert_ne!(net.nodes[n].stack.state(), State::Up);
    assert!(
        net.coord.nwk_cmd_rx[before..].iter().any(|(s, c)| *s == short && c[0] == zigbee::nwk::CMD_LEAVE && c[1] & 0x40 == 0),
        "{:?}",
        &net.coord.nwk_cmd_rx[before..]
    );
    net.power_cycle(n);
    assert_eq!(net.nodes[n].stack.state(), State::Down);
}

#[test]
fn follows_a_channel_change() {
    let mut net = Net::new();
    let n = net.join_node();
    let mut p = (1u32 << 20).to_le_bytes().to_vec();
    p.extend_from_slice(&[0xFE, 1]);
    net.coord.zdo(0xFFFD, 0x0038, &p);
    net.run(10_000);
    net.air.borrow_mut().ports[0].channel = 20;
    assert_eq!(net.nodes[n].stack.channel(), 20);
    let short = net.nodes[n].short();
    let seq = net.coord.zcl(short, ON_OFF, 0x01, 0x00, &[]);
    net.run(100);
    assert_eq!(net.coord.last_zcl(short, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x00, 0x00]);
    net.power_cycle(n);
    assert_eq!(net.nodes[n].stack.channel(), 20);
}

fn line() -> (Net, usize, usize) {
    let mut net = Net::new();
    let a = net.join_node();
    let b = net.join_node();
    let (pa, pb) = (net.nodes[a].id, net.nodes[b].id);
    assert_ne!(pa, pb);
    net.link(0, pb, 0);
    net.run(70_000);
    (net, a, b)
}

#[test]
fn relays_broadcasts_to_nodes_out_of_range() {
    let (mut net, a, b) = line();
    net.coord.zcl(0xFFFD, ON_OFF, 0x01, 0x00, &[]);
    net.run(2_000);
    assert!(!net.nodes[a].on);
    assert!(!net.nodes[b].on, "the far node gets the broadcast through the near one");
    assert_eq!(net.nodes[b].commands.iter().filter(|c| **c == Command::Off).count(), 1);
    assert_eq!(net.nodes[a].commands.iter().filter(|c| **c == Command::Off).count(), 1);
}

#[test]
fn discovers_routes_in_both_directions() {
    let (mut net, a, b) = line();
    let (sa, sb) = (net.nodes[a].short(), net.nodes[b].short());
    net.coord.via.insert(sb, sa);
    let seq = net.coord.zcl(sb, ON_OFF, 0x01, 0x00, &[]);
    net.run(3_000);
    assert!(!net.nodes[b].on, "the command arrived over two hops");
    assert_eq!(net.coord.last_zcl(sb, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x00, 0x00]);
    assert!(net.nodes[a].stack.stats.relayed >= 2);
    let seq = net.coord.zcl(sb, ON_OFF, 0x01, 0x01, &[]);
    net.run(100);
    assert_eq!(net.coord.last_zcl(sb, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x01, 0x00]);
    let seq = net.coord.zdo(sa, 0x0032, &[0]);
    net.run(100);
    let rtg = net.coord.last_zdo(sa, 0x8032).unwrap();
    assert_eq!(&rtg[..2], &[seq, 0x00]);
    let entries: Vec<&[u8]> = rtg[5..].chunks(5).collect();
    assert!(entries.iter().any(|e| e[..2] == sb.to_le_bytes() && e[2] & 7 == 0 && e[3..] == sb.to_le_bytes()));
}

#[test]
fn many_to_one_route_and_route_record_then_source_routing() {
    let (mut net, a, b) = line();
    let (sa, sb) = (net.nodes[a].short(), net.nodes[b].short());
    net.coord.many_to_one_route_request(false);
    net.run(2_000);
    assert_eq!(bind(&mut net, a, ON_OFF)[1], 0x00);
    net.coord.via.insert(sb, sa);
    net.coord.zcl(sb, ON_OFF, 0x01, 0x00, &[]);
    net.run(3_000);
    assert!(!net.nodes[b].on);
    let record = net.coord.route_records.iter().find(|(src, _)| *src == sb).expect("route record from the far node");
    assert_eq!(record.1, vec![sa], "the relay added itself to the record");
    assert!(net.coord.route_records.iter().any(|(src, relays)| *src == sa && relays.is_empty()));
    net.coord.via.clear();
    net.coord.source_route.insert(sb, vec![sa]);
    let seq = net.coord.zcl(sb, ON_OFF, 0x01, 0x01, &[]);
    net.run(200);
    assert!(net.nodes[b].on, "a source-routed frame is relayed");
    assert_eq!(net.coord.last_zcl(sb, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x01, 0x00]);
}

#[test]
fn route_repairs_after_the_relay_disappears() {
    let mut net = Net::new();
    let a = net.join_node();
    let b = net.join_node();
    let c = net.join_node();
    let (pb, pc) = (net.nodes[b].id, net.nodes[c].id);
    net.link(0, pc, 0);
    net.run(70_000);
    let sc = net.nodes[c].short();
    let (sa, sb) = (net.nodes[a].short(), net.nodes[b].short());
    net.coord.via.insert(sc, sa);
    let seq = net.coord.zcl(sc, ON_OFF, 0x01, 0x00, &[]);
    net.run(3_000);
    assert_eq!(net.coord.last_zcl(sc, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x00, 0x00]);
    net.set_power(a, false);
    let _ = pb;
    net.coord.via.insert(sc, sb);
    let seq = net.coord.zcl(sc, ON_OFF, 0x01, 0x01, &[]);
    net.run(6_000);
    assert!(net.nodes[c].on);
    assert_eq!(net.coord.last_zcl(sc, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x01, 0x00]);
}

#[test]
fn answers_beacon_requests_after_a_random_pause() {
    let mut net = Net::new();
    let n = net.join_node();
    let id = net.nodes[n].id;
    net.coord.permit_join = false;
    net.run(181_000);
    let t = net.now();
    let scanner = net.air.borrow_mut().add_port(0x1111);
    net.air.borrow_mut().transmit(scanner, &[0x03, 0x08, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0x07]);
    net.run(3);
    assert!(net.sent_by(id, t).iter().all(|f| f[0] & 7 != 0), "the beacon waits a random moment");
    net.run(40);
    let beacons: Vec<Vec<u8>> = net.sent_by(id, t).into_iter().filter(|f| f[0] & 7 == 0).collect();
    assert_eq!(beacons.len(), 1);
    let (h, off) = zigbee::mac::Header::parse(&beacons[0]).unwrap();
    let b = zigbee::mac::Beacon::parse(&h, &beacons[0][off..]).unwrap();
    assert_eq!((b.pan, b.short, b.ext_pan), (PAN, net.nodes[n].short(), EXT_PAN));
    assert!(!b.permit_join && b.router_capacity && b.end_device_capacity);
    assert_eq!(b.depth, 1);
}

#[test]
fn the_stack_fits_comfortably_in_the_chips_ram() {
    let size = std::mem::size_of::<zigbee::Stack<SimHal>>();
    assert!(size < 14 * 1024, "Stack is {size} bytes");
}

#[test]
fn a_network_key_sent_in_the_clear_is_refused() {
    let mut net = Net::new();
    net.coord.send_key_in_the_clear = true;
    let n = net.add_node();
    net.run(12_000);
    assert_ne!(net.nodes[n].stack.state(), State::Up);
    assert!(!net.sent_by(net.nodes[n].id, 0).iter().any(|f| f[0] & 7 == 1), "no NWK frame was ever sent with that key");
}

#[test]
fn three_hops_route_discovery_and_a_two_relay_source_route() {
    let mut net = Net::new();
    let a = net.join_node();
    let b = net.join_node();
    let c = net.join_node();
    let (pa, pb, pc) = (net.nodes[a].id, net.nodes[b].id, net.nodes[c].id);
    net.link(0, pb, 0);
    net.link(0, pc, 0);
    net.link(pa, pc, 0);
    net.run(70_000);
    let (sa, sb, sc) = (net.nodes[a].short(), net.nodes[b].short(), net.nodes[c].short());
    net.coord.source_route.insert(sc, vec![sb, sa]);
    let seq = net.coord.zcl(sc, ON_OFF, 0x01, 0x00, &[]);
    net.run(4_000);
    assert!(!net.nodes[c].on, "the frame went a -> b -> c");
    assert_eq!(net.coord.last_zcl(sc, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x00, 0x00]);
    assert!(net.nodes[a].stack.stats.relayed >= 2 && net.nodes[b].stack.stats.relayed >= 2);
    net.coord.source_route.clear();
    net.coord.via.insert(sc, sa);
    let seq = net.coord.zcl(sc, ON_OFF, 0x01, 0x01, &[]);
    net.run(4_000);
    assert!(net.nodes[c].on);
    assert_eq!(net.coord.last_zcl(sc, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x01, 0x00]);
}

#[test]
fn link_status_lists_live_neighbors_with_both_costs() {
    let mut net = Net::new();
    let a = net.join_node();
    let b = net.join_node();
    let (sa, sb) = (net.nodes[a].short(), net.nodes[b].short());
    net.run(40_000);
    let last = |net: &Net, src: u16| net.coord.nwk_cmd_rx.iter().rev().find(|(s, c)| *s == src && c[0] == nwk::CMD_LINK_STATUS).unwrap().1.clone();
    let (lo, hi) = (sa.min(sb), sa.max(sb));
    let other = if sa == lo { hi } else { lo };
    let o = other.to_le_bytes();
    assert_eq!(last(&net, sa), vec![nwk::CMD_LINK_STATUS, 0x62, 0x00, 0x00, 0x11, o[0], o[1], 0x11]);
    net.set_power(b, false);
    net.run(75_000);
    assert_eq!(last(&net, sa), vec![nwk::CMD_LINK_STATUS, 0x61, 0x00, 0x00, 0x11]);
}

#[test]
fn finds_its_network_again_after_a_channel_change_it_missed() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    net.set_power(n, false);
    net.air.borrow_mut().ports[0].channel = 23;
    net.run(5_000);
    net.power_cycle(n);
    net.set_power(n, true);
    assert_eq!(net.nodes[n].stack.channel(), 11);
    net.run_until(200_000, "back on the network's channel", |net| net.nodes[n].stack.channel() == 23 && net.nodes[n].stack.state() == State::Up);
    let t = net.now();
    net.run(3_000);
    assert!(net.coord.rejoins.iter().any(|(s, secure)| *s == short && *secure), "rejoins {:x?}", net.coord.rejoins);
    let announced = net.nwk_sent_by(net.nodes[n].id, t).iter().any(|(_, h, p)| h.dst == 0xFFFD && h.frame_type == nwk::FRAME_DATA && p.len() > 8 && p[2..4] == [0x13, 0x00]);
    assert!(announced, "Device_annce on the new channel");
    let seq = net.coord.zcl(short, ON_OFF, 0x01, 0x00, &[]);
    net.run(3_000);
    assert_eq!(net.coord.last_zcl(short, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x00, 0x00]);
    assert_eq!(net.coord.devices.len(), 1, "no new association was needed");
    net.power_cycle(n);
    assert_eq!(net.nodes[n].stack.channel(), 23);
}

#[test]
fn a_search_that_finds_nothing_leaves_the_network_settings_alone() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    net.air.borrow_mut().ports[0].powered = false;
    net.run(400_000);
    assert_eq!(net.nodes[n].stack.state(), State::Up);
    assert_eq!((net.nodes[n].stack.channel(), net.nodes[n].short()), (11, short));
    net.air.borrow_mut().ports[0].powered = true;
    net.run(1_000);
    let seq = net.coord.zcl(short, ON_OFF, 0x01, 0x00, &[]);
    net.run(3_000);
    assert_eq!(net.coord.last_zcl(short, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x00, 0x00]);
}

#[test]
fn energy_scan_reports_the_level_of_every_requested_channel() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    net.air.borrow_mut().energy[4] = 200;
    net.air.borrow_mut().energy[9] = 37;
    let mut p = ((1u32 << 15) | (1 << 20) | (1 << 26)).to_le_bytes().to_vec();
    p.extend_from_slice(&[2, 1]);
    let seq = net.coord.zdo(short, 0x0038, &p);
    net.run(100);
    let rsp = net.coord.last_zdo(short, 0x8038).unwrap();
    assert_eq!(&rsp[..2], &[seq, 0x00]);
    assert_eq!(&rsp[2..6], &((1u32 << 15) | (1 << 20) | (1 << 26)).to_le_bytes());
    assert_eq!(&rsp[10..], &[3, 200, 37, 0]);
    let seq = net.coord.zcl(short, ON_OFF, 0x01, 0x00, &[]);
    net.run(100);
    assert_eq!(net.coord.last_zcl(short, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x00, 0x00]);
}

#[test]
fn leave_with_rejoin_brings_the_bulb_back_under_its_key() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    net.coord.permit_join = false;
    net.coord.nwk_command(short, 1, &[nwk::CMD_LEAVE, 0x60]);
    net.run_until(10_000, "rejoin", |net| !net.coord.rejoins.is_empty());
    assert_eq!(net.coord.rejoins, vec![(short, true)], "one rejoin request, secured with the network key");
    let leave = net.coord.nwk_cmd_rx.iter().find(|(s, c)| *s == short && c[0] == nwk::CMD_LEAVE).expect("leave indication");
    assert_eq!(leave.1[1], 0x20, "it said it would be back");
    net.run(1_000);
    assert_eq!(net.nodes[n].stack.state(), State::Up);
    assert_eq!(net.coord.devices.len(), 1, "no new association");
    let seq = net.coord.zcl(short, ON_OFF, 0x01, 0x00, &[]);
    net.run(3_000);
    assert_eq!(net.coord.last_zcl(short, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x00, 0x00]);
    net.power_cycle(n);
    assert_eq!(net.nodes[n].stack.state(), State::Up);
}

#[test]
fn a_network_key_change_missed_while_off_is_recovered_through_the_trust_center() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    net.coord.permit_join = false;
    net.set_power(n, false);
    net.coord.key = [0x77; 16];
    net.coord.key_seq = 1;
    net.run(5_000);
    net.power_cycle(n);
    net.set_power(n, true);
    net.run_until(200_000, "Trust Center rejoin", |net| !net.coord.rejoins.is_empty());
    assert_eq!(net.coord.rejoins, vec![(short, false)], "a rejoin request in the clear");
    net.run(2_000);
    assert_eq!(net.nodes[n].stack.state(), State::Up);
    assert!(!beacon_of(&mut net, n).permit_join);
    let unreadable = net.coord.mic_failures;
    let seq = net.coord.zcl(short, ON_OFF, 0x01, 0x00, &[]);
    net.run(20_000);
    assert_eq!(net.coord.last_zcl(short, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x00, 0x00]);
    assert_eq!(net.coord.mic_failures, unreadable, "everything since is under the new key");
    assert_eq!(net.coord.devices.len(), 1);
    net.power_cycle(n);
    net.run(1_000);
    let seq = net.coord.zcl(short, ON_OFF, 0x01, 0x01, &[]);
    net.run(3_000);
    assert_eq!(net.coord.last_zcl(short, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x01, 0x00]);
}

#[test]
fn data_encrypted_with_the_link_key_is_read_and_answered_in_kind() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let seq = net.coord.zcl_encrypted(short, ON_OFF, 0x01, 0x00, &[]);
    net.run(100);
    assert_eq!(net.nodes[n].commands.last(), Some(&Command::Off));
    assert_eq!(net.coord.last_zcl(short, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x00, 0x00]);
    assert_eq!(net.coord.aps_encrypted_rx, 2, "the acknowledgement and the default response came encrypted");
    assert_eq!(net.coord.mic_failures, 0);
    net.coord.zcl(short, ON_OFF, 0x01, 0x01, &[]);
    net.run(100);
    assert_eq!(net.coord.aps_encrypted_rx, 2);
    net.coord.devices[0].tclk = [9; 16];
    let commands = net.nodes[n].commands.len();
    net.coord.zcl_encrypted(short, ON_OFF, 0x01, 0x00, &[]);
    net.run(100);
    assert_eq!(net.nodes[n].commands.len(), commands);
}

fn conflicts(net: &Net) -> Vec<(u16, u16)> {
    net.coord
        .nwk_cmd_rx
        .iter()
        .filter(|(s, p)| *s != 0x0000 && p.len() >= 4 && p[0] == 0x03 && p[1] == 0x0D)
        .map(|(s, p)| (*s, u16::from_le_bytes([p[2], p[3]])))
        .collect()
}

#[test]
fn another_device_under_the_bulbs_address_makes_it_take_a_new_one() {
    let mut net = Net::new();
    let n = net.join_node();
    let old = net.nodes[n].short();
    let ext = net.air.borrow().ports[net.nodes[n].id].ext;
    let mut other = Zed::new(&net.air, false);
    assert!(other.join(&mut net, 0x0000));
    other.short = old;
    other.parent = old;
    other.announce();
    net.run(1_000);
    assert_eq!(conflicts(&net), vec![(old, old)]);
    let new = net.nodes[n].short();
    assert_ne!(new, old);
    assert!(new != 0 && new < 0xFFF8);
    assert_eq!(net.coord.devices.iter().find(|d| d.ext == ext).unwrap().short, new, "the coordinator heard the announcement");
    assert_eq!(net.air.borrow().ports[net.nodes[n].id].short, new, "the radio answers to the new address");
    let seq = net.coord.zcl(new, ON_OFF, 0x01, 0x00, &[]);
    net.run(100);
    assert_eq!(net.coord.last_zcl(new, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x00, 0x00]);
    net.power_cycle(n);
    net.run(1_000);
    assert_eq!(net.nodes[n].short(), new);
}

#[test]
fn a_reported_conflict_about_its_own_address_is_obeyed() {
    let mut net = Net::new();
    let n = net.join_node();
    let old = net.nodes[n].short();
    let o = old.to_le_bytes();
    net.coord.nwk_command(0xFFFD, 30, &[0x03, 0x0D, o[0], o[1]]);
    net.run(1_000);
    let new = net.nodes[n].short();
    assert_ne!(new, old);
    assert!(conflicts(&net).is_empty(), "the report is already out; the bulb only acts on it");
    let seq = net.coord.zcl(new, ON_OFF, 0x01, 0x01, &[]);
    net.run(100);
    assert_eq!(net.coord.last_zcl(new, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x01, 0x00]);
    net.coord.nwk_command(0xFFFD, 30, &[0x03, 0x0D, 0x34, 0x12]);
    net.run(1_000);
    assert_eq!(net.nodes[n].short(), new);
}

#[test]
fn two_other_devices_under_one_address_are_reported_once() {
    let mut net = Net::new();
    let n = net.join_node();
    let me = net.nodes[n].short();
    let mut first = Zed::new(&net.air, false);
    assert!(first.join(&mut net, 0x0000));
    let mut second = Zed::new(&net.air, false);
    assert!(second.join(&mut net, 0x0000));
    first.parent = me;
    second.parent = me;
    first.announce();
    second.announce();
    net.run(500);
    assert!(conflicts(&net).is_empty());
    second.short = first.short;
    second.announce();
    net.run(500);
    assert_eq!(conflicts(&net), vec![(me, first.short)]);
    second.announce();
    first.announce();
    net.run(500);
    assert_eq!(conflicts(&net).len(), 1, "not again for a while");
    assert_eq!(net.nodes[n].short(), me);
}

#[test]
fn a_foreign_network_under_the_same_pan_identifier_is_reported_to_the_manager() {
    let mut net = Net::new();
    let n = net.join_node();
    let me = net.nodes[n].short();
    let reports = |net: &Net| net.coord.nwk_cmd_rx.iter().filter(|(_, p)| p.first() == Some(&0x09)).cloned().collect::<Vec<_>>();
    let stranger = net.air.borrow_mut().add_port(0x00AA_0000_0000_0001);
    let beacon = |ext_pan: u64| {
        let mut f = zigbee::util::Buf::new();
        zigbee::mac::Beacon {
            pan: PAN,
            short: 0x0000,
            permit_join: false,
            pan_coordinator: true,
            stack_profile: 2,
            protocol_version: 2,
            router_capacity: true,
            end_device_capacity: true,
            depth: 0,
            ext_pan,
            update_id: 0,
        }
        .write(7, &mut f);
        f.as_slice().to_vec()
    };
    net.air.borrow_mut().transmit(stranger, &beacon(EXT_PAN));
    net.run(500);
    assert!(reports(&net).is_empty());
    net.air.borrow_mut().transmit(stranger, &beacon(0x1122_3344_5566_7788));
    net.run(500);
    let got = reports(&net);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].0, me);
    let mut want = vec![0x09, 0x01];
    want.extend_from_slice(&EXT_PAN.to_le_bytes());
    want.extend_from_slice(&PAN.to_le_bytes());
    assert_eq!(got[0].1, want);
    net.air.borrow_mut().transmit(stranger, &beacon(0x1122_3344_5566_7788));
    net.run(500);
    assert_eq!(reports(&net).len(), 1);
}

#[test]
fn a_trust_center_that_knows_the_install_code_is_joined_under_its_key() {
    let code: [u8; 16] = [0x83, 0xFE, 0xD3, 0x40, 0x7A, 0x93, 0x97, 0x23, 0xA5, 0xC6, 0x39, 0xB2, 0x69, 0x16, 0xD5, 0x05];
    let key = zigbee::crypto::install_code_key(&code);
    let mut net = Net::new();
    net.identity = zigbee::stack::Identity { install_code: Some(code), ..IDENTITY };
    net.coord.join_key = key;
    let n = net.join_node();
    let short = net.nodes[n].short();
    let seq = net.coord.zcl(short, ON_OFF, 0x01, 0x00, &[]);
    net.run(100);
    assert_eq!(net.coord.last_zcl(short, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x00, 0x00]);
    assert!(net.coord.devices.iter().any(|d| d.verified), "the link key exchange ran under the install code's key");
    let ext = net.air.borrow().ports[net.nodes[n].id].ext;
    let current = net.coord.devices.iter().find(|d| d.ext == ext).unwrap().tclk;
    net.coord.devices.iter_mut().find(|d| d.ext == ext).unwrap().tclk = key;
    net.coord.transport_network_key(short, ext, &[0x99; 16], 1, true);
    net.run(200);
    net.coord.devices.iter_mut().find(|d| d.ext == ext).unwrap().tclk = current;
    net.coord.switch_key(1);
    net.run(1_000);
    let seq = net.coord.zcl(short, ON_OFF, 0x01, 0x01, &[]);
    net.run(100);
    assert_eq!(net.coord.last_zcl(short, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x01, 0x00]);
    let mut net = Net::new();
    net.identity = zigbee::stack::Identity { install_code: Some(code), ..IDENTITY };
    let n = net.join_node();
    assert_eq!(net.nodes[n].stack.state(), State::Up);
    let mut net = Net::new();
    net.coord.join_key = key;
    let n = net.add_node();
    net.run(20_000);
    assert_ne!(net.nodes[n].stack.state(), State::Up);
}

#[test]
fn encrypted_data_is_taken_once_and_only_from_the_trust_center() {
    let mut net = Net::new();
    net.coord.stack_revision = 20;
    let n = net.join_node();
    let short = net.nodes[n].short();
    net.run(5_000);
    net.coord.zcl_encrypted(short, ON_OFF, 0x11, 0x00, &[]);
    net.run(100);
    assert_eq!(net.nodes[n].commands.iter().filter(|c| **c == Command::Off).count(), 1);
    let again = net.coord.last_encrypted.clone();
    for _ in 0..40 {
        net.coord.zcl(short, ON_OFF, 0x00, 0x00, &[0x00, 0x00]);
        net.run(300);
    }
    net.run(30_000);
    net.coord.nwk_data(short, &again);
    net.run(100);
    assert_eq!(net.nodes[n].commands.iter().filter(|c| **c == Command::Off).count(), 1);
    let mut other = Zed::new(&net.air, false);
    assert!(other.join(&mut net, 0x0000));
    other.parent = short;
    let zcl = [0x11, 0x55, 0x02];
    let mut f = vec![0x20, 1, 0x06, 0x00, 0x04, 0x01, 1, 0x66];
    let aux_at = f.len();
    f.push(0x25);
    f.extend_from_slice(&7u32.to_le_bytes());
    f.extend_from_slice(&other.ext.to_le_bytes());
    let body_at = f.len();
    f.extend_from_slice(&zcl);
    f.extend_from_slice(&[0; 4]);
    let mut nonce = [0u8; 13];
    nonce[..8].copy_from_slice(&other.ext.to_le_bytes());
    nonce[8..12].copy_from_slice(&7u32.to_le_bytes());
    nonce[12] = 0x25;
    let (head, body) = f.split_at_mut(body_at);
    zigbee::crypto::ccm_encrypt(&zigbee::aes::Aes128::new(&TCLK), &nonce, head, body, zcl.len()).unwrap();
    f[aux_at] = 0x20;
    let before = net.nodes[n].commands.len();
    let h = other.header(nwk::FRAME_DATA, short);
    other.nwk_send(h, &f);
    net.run(200);
    assert_eq!(net.nodes[n].commands.len(), before, "encrypted data from a device that is not the Trust Center is dropped");
}

#[test]
fn a_refused_rejoin_changes_nothing() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    net.coord.permit_join = false;
    net.coord.refuse_rejoin = true;
    net.set_power(n, false);
    net.coord.key = [0x77; 16];
    net.coord.key_seq = 1;
    net.run(5_000);
    net.power_cycle(n);
    net.set_power(n, true);
    net.run_until(200_000, "Trust Center rejoin attempt", |net| !net.coord.rejoins.is_empty());
    net.run(2_000);
    assert_eq!((net.nodes[n].stack.state(), net.nodes[n].short()), (State::Up, short));
    assert_eq!(net.air.borrow().ports[net.nodes[n].id].short, short);
    net.coord.refuse_rejoin = false;
    net.coord.rejoins.clear();
    net.run_until(300_000, "second attempt", |net| !net.coord.rejoins.is_empty());
    net.run(2_000);
    let seq = net.coord.zcl(short, ON_OFF, 0x01, 0x00, &[]);
    net.run(20_000);
    assert_eq!(net.coord.last_zcl(short, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x00, 0x00]);
}

#[test]
fn a_retransmitted_aps_frame_is_acknowledged_again_but_acted_on_once() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let aps = net.coord.aps_frame(0x40, 1, ON_OFF, 0x0104, 1, &[0x11, 0x42, 0x02]);
    let before = net.nodes[n].commands.len();
    let acks = |net: &Net| net.sent_by(net.nodes[n].id, 0).len();
    let sent = acks(&net);
    net.coord.nwk_data(short, &aps);
    net.run(200);
    let after_first = acks(&net);
    net.coord.nwk_data(short, &aps);
    net.run(200);
    assert_eq!(&net.nodes[n].commands[before..], &[Command::Off], "toggled once (the light was on)");
    assert!(after_first > sent && acks(&net) > after_first, "each copy is acknowledged");
}

#[test]
fn a_leave_request_must_be_addressed_to_the_bulb() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    net.coord.nwk_command(0xFFFD, 1, &[0x04, 0x40]);
    net.run(1_000);
    assert_eq!(net.nodes[n].stack.state(), State::Up);
    let seq = net.coord.zcl(short, ON_OFF, 0x01, 0x00, &[]);
    net.run(100);
    assert_eq!(net.coord.last_zcl(short, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x00, 0x00]);
}

#[test]
fn a_trust_center_that_counts_from_zero_under_the_new_link_key_is_still_heard() {
    let mut net = Net::new();
    net.coord.restart_link_key_counter = true;
    let n = net.join_node();
    let short = net.nodes[n].short();
    net.run(500);
    assert_eq!(net.coord.aps_fc, 1, "the Confirm Key went out with frame counter 0");
    let seq = net.coord.zcl_encrypted(short, ON_OFF, 0x01, 0x00, &[]);
    net.run(100);
    assert_eq!(net.nodes[n].commands.last(), Some(&Command::Off));
    assert_eq!(net.coord.last_zcl(short, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x00, 0x00]);
    let again = net.coord.last_encrypted.clone();
    net.run(5_000);
    let (commands, replays) = (net.nodes[n].commands.len(), net.nodes[n].stack.stats.replays);
    net.coord.nwk_data(short, &again);
    net.run(100);
    assert_eq!((net.nodes[n].commands.len(), net.nodes[n].stack.stats.replays), (commands, replays + 1));
    net.power_cycle(n);
    net.run(5_000);
    let seq = net.coord.zcl_encrypted(short, ON_OFF, 0x01, 0x01, &[]);
    net.run(100);
    assert_eq!(net.coord.last_zcl(short, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x01, 0x00]);
}

fn rotate_key(net: &mut Net, n: usize, key: &[u8; 16], seq: u8) {
    let short = net.nodes[n].short();
    let ext = net.air.borrow().ports[net.nodes[n].id].ext;
    net.coord.transport_network_key(short, ext, key, seq, true);
    net.run(100);
    net.coord.switch_key(seq);
    net.coord.use_key(key, seq, true);
    net.run(100);
}

#[test]
fn frame_counters_start_anew_under_a_new_network_key() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    net.run(100);
    let t = net.now();
    net.coord.zcl(short, ON_OFF, 0x11, 0x02, &[]);
    net.run(50);
    let under_old_key = net.sent_by(0, t).into_iter().next().unwrap();
    assert!(!net.nodes[n].on, "toggled");
    rotate_key(&mut net, n, &[0x5A; 16], 1);
    assert_eq!(net.coord.fc, 0);
    let t = net.now();
    let seq = net.coord.zcl(short, ON_OFF, 0x01, 0x00, &[]);
    net.run(100);
    assert_eq!(net.coord.last_zcl(short, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x00, 0x00]);
    assert_eq!(net.coord.mic_failures, 0);
    let under_new_key = net.sent_by(0, t).into_iter().next().unwrap();
    net.run(5_000);
    let (commands, replays) = (net.nodes[n].commands.len(), net.nodes[n].stack.stats.replays);
    net.air.borrow_mut().transmit(0, &under_new_key);
    net.run(50);
    assert_eq!((net.nodes[n].commands.len(), net.nodes[n].stack.stats.replays), (commands, replays + 1));
    net.air.borrow_mut().transmit(0, &under_old_key);
    net.run(50);
    assert_eq!((net.nodes[n].commands.len(), net.nodes[n].stack.stats.replays), (commands, replays + 2));
}

#[test]
fn a_frame_under_the_new_network_key_switches_to_it_without_a_switch_key() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let ext = net.air.borrow().ports[net.nodes[n].id].ext;
    let new_key = [0x5A; 16];
    net.coord.transport_network_key(short, ext, &new_key, 1, true);
    net.run(100);
    net.coord.use_key(&new_key, 1, true);
    let seq = net.coord.zcl(short, ON_OFF, 0x01, 0x00, &[]);
    net.run(100);
    assert_eq!(net.coord.last_zcl(short, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x00, 0x00], "answered under the new key");
    assert_eq!(net.coord.mic_failures, 0);
    net.power_cycle(n);
    net.run(1_000);
    let seq = net.coord.zcl(short, ON_OFF, 0x01, 0x01, &[]);
    net.run(100);
    assert_eq!(net.coord.last_zcl(short, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x01, 0x00]);
    assert_eq!(net.coord.mic_failures, 0);
}

fn device_behind(net: &mut Net, n: usize) -> Zed {
    let mut other = Zed::new(&net.air, false);
    assert!(other.join(net, 0x0000));
    other.parent = net.nodes[n].short();
    other
}

#[test]
fn frames_under_the_previous_network_key_are_still_read() {
    let mut net = Net::new();
    let n = net.join_node();
    let mut other = device_behind(&mut net, n);
    let report = [0x18, 7, 0x0A, 0x00, 0x00, 0x10, 0x01];
    other.zcl_to_coordinator(ON_OFF, &report);
    net.run(100);
    assert_eq!(net.coord.zcl_rx.iter().filter(|(s, _, _)| *s == other.short).count(), 1);
    rotate_key(&mut net, n, &[0x5A; 16], 1);
    other.zcl_to_coordinator(ON_OFF, &report);
    net.run(100);
    assert_eq!(net.coord.zcl_rx.iter().filter(|(s, _, _)| *s == other.short).count(), 2, "read under the old key, passed on under the new");
    assert_eq!(net.coord.mic_failures, 0, "the bulb itself sends under the new key only");
    net.coord.switch_key(0);
    net.run(100);
    let short = net.nodes[n].short();
    let seq = net.coord.zcl(short, ON_OFF, 0x01, 0x00, &[]);
    net.run(100);
    assert_eq!(net.coord.last_zcl(short, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x00, 0x00]);
    assert_eq!(net.coord.mic_failures, 0);
    net.power_cycle(n);
    net.run(1_000);
    other.zcl_to_coordinator(ON_OFF, &report);
    net.run(100);
    assert_eq!(net.coord.zcl_rx.iter().filter(|(s, _, _)| *s == other.short).count(), 3);
    rotate_key(&mut net, n, &[0x6B; 16], 2);
    other.zcl_to_coordinator(ON_OFF, &report);
    net.run(100);
    assert_eq!(net.coord.zcl_rx.iter().filter(|(s, _, _)| *s == other.short).count(), 3);
}

#[test]
fn a_key_sequence_number_that_comes_round_again_starts_its_counters_anew() {
    let mut net = Net::new();
    let n = net.join_node();
    let mut other = device_behind(&mut net, n);
    let report = [0x18, 7, 0x0A, 0x00, 0x00, 0x10, 0x01];
    other.zcl_to_coordinator(ON_OFF, &report);
    net.run(100);
    rotate_key(&mut net, n, &[0x5A; 16], 1);
    let third = [0x6B; 16];
    rotate_key(&mut net, n, &third, 0);
    other.key = Some(third);
    other.fc = 0;
    net.coord.zcl_rx.clear();
    other.zcl_to_coordinator(ON_OFF, &report);
    net.run(100);
    assert_eq!(net.coord.zcl_rx.len(), 1, "counter 0 under the new key 0 is no replay of the old key 0");
}

#[test]
fn a_network_key_broadcast_by_the_trust_center_is_taken_and_no_other() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let mut other = device_behind(&mut net, n);
    net.run(100);
    net.coord.network_key_update(short, &[0x71; 16], 1, COORD_EXT);
    net.coord.network_key_update(0xFFFF, &[0x72; 16], 1, other.ext);
    let mut aps = vec![0x09, 0x33, 0x05, 0x01];
    aps.extend_from_slice(&[0x73; 16]);
    aps.push(1);
    aps.extend_from_slice(&[0; 8]);
    aps.extend_from_slice(&COORD_EXT.to_le_bytes());
    let h = other.header(nwk::FRAME_DATA, 0xFFFF);
    other.nwk_send(h, &aps);
    net.run(1_000);
    net.coord.switch_key(1);
    net.run(1_000);
    let seq = net.coord.zcl(short, ON_OFF, 0x01, 0x00, &[]);
    net.run(20_000);
    assert_eq!(net.coord.last_zcl(short, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x00, 0x00]);
    assert_eq!(net.coord.mic_failures, 0, "the bulb still sends under the key it had");
    let new_key = [0x5A; 16];
    net.coord.network_key_update(0xFFFF, &new_key, 1, COORD_EXT);
    net.run(1_000);
    net.coord.switch_key(1);
    net.coord.use_key(&new_key, 1, true);
    net.run(1_000);
    let seq = net.coord.zcl(short, ON_OFF, 0x01, 0x01, &[]);
    net.run(20_000);
    assert_eq!(net.coord.last_zcl(short, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x01, 0x00]);
    assert_eq!(net.coord.mic_failures, 0);
    assert!(net.coord.rejoins.is_empty(), "without having to ask its way back in");
}

#[test]
fn a_replayed_transport_key_does_not_bring_an_old_key_back() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let ext = net.air.borrow().ports[net.nodes[n].id].ext;
    net.run(100);
    let t = net.now();
    net.coord.transport_network_key(short, ext, &[0x71; 16], 1, true);
    net.run(100);
    let new_key = [0x5A; 16];
    net.coord.transport_network_key(short, ext, &new_key, 1, true);
    net.run(100);
    let mpdu = net.sent_by(0, t).into_iter().next().unwrap();
    let (h, hl) = nwk::Header::parse(&mpdu[9..]).unwrap();
    let aux = nwk::parse_aux(&mpdu[9..], hl).unwrap();
    let mut frame = mpdu[9..].to_vec();
    let len = nwk::unsecure(&zigbee::aes::Aes128::new(&NWK_KEY), &aux, &mut frame, hl).unwrap();
    let transport_key = frame[hl + nwk::AUX_LEN..hl + nwk::AUX_LEN + len].to_vec();
    assert_eq!((h.dst, transport_key[0]), (short, 0x21));
    let replays = net.nodes[n].stack.stats.replays;
    net.coord.nwk_data(short, &transport_key);
    net.run(100);
    assert_eq!(net.nodes[n].stack.stats.replays, replays + 1);
    net.coord.switch_key(1);
    net.coord.use_key(&new_key, 1, true);
    net.run(100);
    let seq = net.coord.zcl(short, ON_OFF, 0x01, 0x00, &[]);
    net.run(100);
    assert_eq!(net.coord.last_zcl(short, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x00, 0x00]);
    assert_eq!(net.coord.mic_failures, 0);
}

fn pan_update(update_id: u8, pan: u16) -> Vec<u8> {
    let mut p = vec![nwk::CMD_NETWORK_UPDATE, 0x01];
    p.extend_from_slice(&EXT_PAN.to_le_bytes());
    p.push(update_id);
    p.extend_from_slice(&pan.to_le_bytes());
    p
}

fn beacon_of(net: &mut Net, n: usize) -> zigbee::mac::Beacon {
    let id = net.nodes[n].id;
    let t = net.now();
    let scanner = net.air.borrow_mut().add_port(0x1111);
    net.air.borrow_mut().transmit(scanner, &[0x03, 0x08, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0x07]);
    net.run(50);
    net.air.borrow_mut().ports[scanner].powered = false;
    let beacon = net.sent_by(id, t).into_iter().find(|f| f[0] & 7 == 0).expect("a beacon");
    let (h, off) = zigbee::mac::Header::parse(&beacon).unwrap();
    zigbee::mac::Beacon::parse(&h, &beacon[off..]).unwrap()
}

#[test]
fn a_new_pan_identifier_is_taken_once_the_update_has_been_everywhere() {
    let (mut net, a, b) = line();
    let (pa, sa, sb) = (net.nodes[a].id, net.nodes[a].short(), net.nodes[b].short());
    let new = 0x2B3C;
    let t = net.now();
    net.coord.nwk_command(0xFFFF, 30, &pan_update(7, new));
    net.run(8_800);
    assert_eq!((net.nodes[a].stack.pan_id(), net.nodes[b].stack.pan_id()), (PAN, PAN));
    let sent = net.sent_by(pa, t);
    assert!(sent.iter().any(|f| f[0] & 7 == 1 && f[5..7] == [0xFF, 0xFF]), "passed on");
    assert!(sent.iter().all(|f| f[3..5] == PAN.to_le_bytes()));
    net.run(400);
    assert_eq!((net.nodes[a].stack.pan_id(), net.nodes[b].stack.pan_id()), (new, new));
    assert_eq!(net.air.borrow().ports[pa].pan, new, "the radio listens on it");
    net.coord.set_pan(new);
    net.coord.via.insert(sb, sa);
    let seq = net.coord.zcl(sb, ON_OFF, 0x01, 0x00, &[]);
    net.run(3_000);
    assert_eq!(net.coord.last_zcl(sb, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x00, 0x00]);
    let beacon = beacon_of(&mut net, a);
    assert_eq!((beacon.pan, beacon.update_id), (new, 7));
    net.power_cycle(a);
    assert_eq!(net.nodes[a].stack.pan_id(), new);
    assert_eq!(beacon_of(&mut net, a).update_id, 7);
}

#[test]
fn the_update_identifier_is_the_network_managers_to_set() {
    let mut net = Net::new();
    let n = net.join_node();
    let mut p = 0u32.to_le_bytes().to_vec();
    p.extend_from_slice(&[0xFF, 3, 0x34, 0x12]);
    net.coord.zdo(0xFFFD, 0x0038, &p);
    net.run(500);
    assert_eq!(beacon_of(&mut net, n).update_id, 3);
    net.coord.nwk_command(0xFFFF, 30, &pan_update(9, 0x2B3C));
    net.run(10_000);
    net.coord.set_pan(0x2B3C);
    let beacon = beacon_of(&mut net, n);
    assert_eq!((beacon.pan, beacon.update_id), (0x2B3C, 3));
    let mut p = 0u32.to_le_bytes().to_vec();
    p.extend_from_slice(&[0xFF, 4, 0x00, 0x00]);
    net.coord.zdo(0xFFFD, 0x0038, &p);
    net.run(500);
    net.coord.nwk_command(0xFFFF, 30, &pan_update(10, 0x2B3D));
    net.run(10_000);
    net.coord.set_pan(0x2B3D);
    let beacon = beacon_of(&mut net, n);
    assert_eq!((beacon.pan, beacon.update_id), (0x2B3D, 10));
}

fn peer(net: &mut Net, n: usize, short: u16) -> Zed {
    let me = net.nodes[n].short();
    let z = Zed::router(&net.air, short, me);
    net.link(0, z.id, 0);
    z
}

fn neighbor(net: &mut Net, n: usize, short: u16) -> Zed {
    let me = net.nodes[n].short();
    let mut z = peer(net, n, short);
    z.link_status(&[(me, 1)]);
    net.run(50);
    z
}

fn broadcast_command(z: &mut Zed, payload: &[u8]) {
    let h = z.header(nwk::FRAME_CMD, 0xFFFC);
    z.mac_send(0xFFFF, h, payload);
}

fn route_request(z: &mut Zed, options: u8, id: u8, target: u16) {
    let t = target.to_le_bytes();
    broadcast_command(z, &[nwk::CMD_ROUTE_REQUEST, options, id, t[0], t[1], 0]);
}

fn relayed_requests(net: &Net, n: usize, id: u8, since: u32) -> Vec<(u32, u8)> {
    net.timed_nwk_sent_by(net.nodes[n].id, since)
        .iter()
        .filter(|(_, _, h, p)| h.frame_type == nwk::FRAME_CMD && p[0] == nwk::CMD_ROUTE_REQUEST && p[2] == id)
        .map(|(t, _, _, p)| (*t, p[5]))
        .collect()
}

fn routes(net: &mut Net, n: usize) -> Vec<(u16, u8, u16)> {
    let short = net.nodes[n].short();
    net.coord.zdo(short, 0x0032, &[0]);
    net.run(100);
    let rtg = net.coord.last_zdo(short, 0x8032).unwrap();
    rtg[5..].chunks(5).map(|e| (u16::from_le_bytes([e[0], e[1]]), e[2], u16::from_le_bytes([e[3], e[4]]))).collect()
}

fn statuses(net: &Net, n: usize, since: u32) -> Vec<(u16, u16, Vec<u8>)> {
    net.nwk_sent_by(net.nodes[n].id, since)
        .iter()
        .filter(|(_, h, p)| h.frame_type == nwk::FRAME_CMD && p[0] == nwk::CMD_NETWORK_STATUS)
        .map(|(m, h, p)| (if let zigbee::mac::Addr::Short(d) = m.dst { d } else { 0xFFFF }, h.dst, p[1..].to_vec()))
        .collect()
}

#[test]
fn a_route_request_is_taken_only_over_a_link_known_to_work_both_ways() {
    let mut net = Net::new();
    let n = net.join_node();
    let me = net.nodes[n].short();
    let mut x = peer(&mut net, n, 0x2B02);
    let t = net.now();
    route_request(&mut x, 0x08, 1, 0xFFFC);
    net.run(1_500);
    assert!(relayed_requests(&net, n, 1, t).is_empty());
    assert!(!routes(&mut net, n).iter().any(|r| r.0 == x.short));
    route_request(&mut x, 0x00, 2, me);
    net.run(500);
    x.drain();
    assert!(!x.rx.iter().any(|(_, p)| p[0] == nwk::CMD_ROUTE_REPLY));
    x.link_status(&[(me, 1)]);
    net.run(50);
    let t = net.now();
    route_request(&mut x, 0x08, 3, 0xFFFC);
    net.run(1_500);
    assert!(!relayed_requests(&net, n, 3, t).is_empty());
    assert!(routes(&mut net, n).iter().any(|r| *r == (x.short, 0x30, x.short)), "a route to the concentrator, with a route record due");
    route_request(&mut x, 0x00, 4, me);
    net.run(500);
    x.drain();
    assert!(x.rx.iter().any(|(_, p)| p[0] == nwk::CMD_ROUTE_REPLY && p[2] == 4));
    x.link_status(&[(0x0000, 1)]);
    net.run(50);
    let t = net.now();
    route_request(&mut x, 0x08, 5, 0xFFFC);
    net.run(1_500);
    assert!(relayed_requests(&net, n, 5, t).is_empty());
}

#[test]
fn route_requests_are_repeated_as_often_and_as_fast_as_the_constants_say() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let me = short;
    let mut x = neighbor(&mut net, n, 0x2B02);
    let t = net.now();
    route_request(&mut x, 0x00, 7, 0x5E01);
    net.run(2_000);
    let relays = relayed_requests(&net, n, 7, t);
    assert_eq!(relays.len(), 3, "the relay and nwkcRREQRetries (2) repeats");
    assert!((4..=256).contains(&(relays[0].0 - t)), "first relay after {} ms", relays[0].0 - t);
    assert_eq!((relays[1].0 - relays[0].0, relays[2].0 - relays[1].0), (254, 254));
    assert!(relays.iter().all(|r| r.1 == 1));
    let mut delays = vec![];
    for id in 20..36 {
        let t = net.now();
        route_request(&mut x, 0x00, id, 0x5E01);
        net.run(2_500);
        delays.push(relayed_requests(&net, n, id, t)[0].0 - t);
    }
    assert!(delays.iter().all(|d| (4..=256).contains(d)) && delays.iter().any(|d| *d > 64), "delays {delays:?}");
    net.run(10_000);
    x.link_status(&[(me, 1)]);
    net.run(50);
    let t = net.now();
    for id in 40..46 {
        route_request(&mut x, 0x00, id, 0x5E01);
        net.run(300);
    }
    route_request(&mut x, 0x00, 46, 0x5E01);
    net.run(1_000);
    assert!(relayed_requests(&net, n, 46, t).is_empty());
    let h = x.header(nwk::FRAME_CMD, 0xFFFC);
    x.mac_send(0xFFFF, h, &[nwk::CMD_ROUTE_REQUEST, 0x00, 40, 0x01, 0x5E, 2]);
    net.run(1_000);
    assert_eq!(relayed_requests(&net, n, 40, t).len(), 3);
    net.run(10_000);
    let t = net.now();
    let ask = |net: &mut Net| {
        let aps = net.coord.aps_frame(0x00, 1, ON_OFF, 0x0104, 1, &[0x00, 9, 0x00, 0x00, 0x00]);
        let mut h = net.coord.header(nwk::FRAME_DATA, short, 30);
        h.src = 0x5E02;
        net.coord.nwk_send(h, &aps, None);
    };
    ask(&mut net);
    net.run(3_000);
    let own: Vec<u32> = net
        .timed_nwk_sent_by(net.nodes[n].id, t)
        .iter()
        .filter(|(_, _, h, p)| h.src == short && h.frame_type == nwk::FRAME_CMD && p[0] == nwk::CMD_ROUTE_REQUEST && p[3..5] == [0x02, 0x5E])
        .map(|e| e.0)
        .collect();
    assert_eq!(own.len(), 4);
    assert!(own.windows(2).all(|w| w[1] - w[0] == 254), "at {own:?}");
    net.run(4_000);
    ask(&mut net);
    net.run(2_000);
    let requests = |net: &Net| {
        net.nwk_sent_by(net.nodes[n].id, t).iter().filter(|(_, h, p)| h.src == short && p[0] == nwk::CMD_ROUTE_REQUEST && h.frame_type == nwk::FRAME_CMD).count()
    };
    assert_eq!(requests(&net), 4);
    net.run(2_000);
    ask(&mut net);
    net.run(2_000);
    assert_eq!(requests(&net), 8);
}

#[test]
fn the_destination_of_a_route_request_answers_with_the_cost_of_the_link_and_both_addresses() {
    let mut net = Net::new();
    let n = net.join_node();
    let me = net.nodes[n].short();
    let my_ext = net.air.borrow().ports[net.nodes[n].id].ext;
    let mut x = neighbor(&mut net, n, 0x2B02);
    net.link(net.nodes[n].id, x.id, 80);
    for _ in 0..12 {
        x.link_status(&[(me, 1)]);
        net.run(20);
    }
    x.rx.clear();
    route_request(&mut x, 0x00, 0x53, me);
    net.run(300);
    x.drain();
    let (h, p) = x.rx.iter().find(|(_, p)| p[0] == nwk::CMD_ROUTE_REPLY).expect("a route reply");
    assert_eq!((h.src, h.dst, h.radius), (me, x.short, 30));
    assert_eq!((h.src_ext, h.dst_ext), (Some(my_ext), Some(x.ext)));
    let mut want = vec![nwk::CMD_ROUTE_REPLY, 0x30, 0x53];
    want.extend_from_slice(&x.short.to_le_bytes());
    want.extend_from_slice(&me.to_le_bytes());
    want.push(3);
    want.extend_from_slice(&x.ext.to_le_bytes());
    want.extend_from_slice(&my_ext.to_le_bytes());
    assert_eq!(p, &want);
}

#[test]
fn a_route_reply_is_passed_back_with_the_cost_of_the_next_link_and_a_smaller_radius() {
    let mut net = Net::new();
    let n = net.join_node();
    let me = net.nodes[n].short();
    let mut x = neighbor(&mut net, n, 0x2B02);
    let mut y = neighbor(&mut net, n, 0x3C03);
    net.link(x.id, y.id, 0);
    net.link(net.nodes[n].id, x.id, 80);
    for _ in 0..12 {
        x.link_status(&[(me, 1)]);
        net.run(20);
    }
    let t = net.now();
    route_request(&mut x, 0x00, 0x52, y.short);
    net.run(400);
    assert_eq!(relayed_requests(&net, n, 0x52, t)[0].1, 3);
    let reply = |id: u8, orig: u16, resp: u16, cost: u8| {
        let (o, r) = (orig.to_le_bytes(), resp.to_le_bytes());
        vec![nwk::CMD_ROUTE_REPLY, 0x00, id, o[0], o[1], r[0], r[1], cost]
    };
    x.rx.clear();
    let mut h = y.header(nwk::FRAME_CMD, me);
    h.radius = 30;
    y.nwk_send(h, &reply(0x52, x.short, y.short, 2));
    net.run(100);
    x.drain();
    let (h, p) = x.rx.iter().find(|(_, p)| p[0] == nwk::CMD_ROUTE_REPLY).expect("the reply passed on");
    assert_eq!(p, &reply(0x52, x.short, y.short, 5));
    assert_eq!((h.src, h.dst, h.radius, h.dst_ext), (me, x.short, 29, Some(x.ext)));
    let table = routes(&mut net, n);
    assert!(table.contains(&(y.short, 0x00, y.short)) && table.contains(&(x.short, 0x00, x.short)), "routes both ways: {table:x?}");
    x.rx.clear();
    let mut h = y.header(nwk::FRAME_CMD, me);
    h.radius = 1;
    y.nwk_send(h, &reply(0x52, x.short, y.short, 2));
    net.run(100);
    x.drain();
    assert_eq!(x.rx.iter().filter(|(_, p)| p[0] == nwk::CMD_ROUTE_REPLY).map(|(h, _)| h.radius).collect::<Vec<_>>(), vec![1]);
    x.rx.clear();
    let h = y.header(nwk::FRAME_CMD, me);
    y.nwk_send(h, &reply(0x52, x.short, y.short, 4));
    net.run(100);
    x.drain();
    assert!(!x.rx.iter().any(|(_, p)| p[0] == nwk::CMD_ROUTE_REPLY));
    let h = y.header(nwk::FRAME_CMD, me);
    y.nwk_send(h, &reply(0x77, x.short, 0x4D4D, 1));
    let h = y.header(nwk::FRAME_CMD, me);
    y.nwk_send(h, &reply(0x78, me, 0x4E4E, 1));
    net.run(100);
    x.drain();
    assert!(!x.rx.iter().any(|(_, p)| p[0] == nwk::CMD_ROUTE_REPLY));
    assert!(!routes(&mut net, n).iter().any(|r| r.0 == 0x4D4D || r.0 == 0x4E4E));
    let t = net.now();
    let aps = net.coord.aps_frame(0x00, 1, ON_OFF, 0x0104, 1, &[0x00, 9, 0x00, 0x00, 0x00]);
    let mut h = net.coord.header(nwk::FRAME_DATA, me, 30);
    h.src = 0x5E02;
    net.coord.nwk_send(h, &aps, None);
    net.run(100);
    let id = net.nwk_sent_by(net.nodes[n].id, t).iter().find(|(_, h, p)| h.src == me && p[0] == nwk::CMD_ROUTE_REQUEST).expect("the bulb's route request").2[2];
    let h = y.header(nwk::FRAME_CMD, me);
    y.nwk_send(h, &reply(id, me, 0x4E4F, 1));
    let h = y.header(nwk::FRAME_CMD, me);
    y.nwk_send(h, &reply(id, me, 0x5E02, 1));
    net.run(100);
    assert!(!routes(&mut net, n).iter().any(|r| r.0 == 0x4E4F || (r.0 == 0x5E02 && r.1 & 7 == 0)));
    let t = net.now();
    let h = y.header(nwk::FRAME_CMD, x.short);
    y.nwk_send(h, &reply(0x52, x.short, y.short, 1));
    net.run(100);
    assert!(!net.nwk_sent_by(net.nodes[n].id, t).iter().any(|(_, _, p)| p[0] == nwk::CMD_ROUTE_REPLY));
}

fn data_through(z: &mut Zed, dst: u16, discover: u8) -> TxStatus {
    let mut h = z.header(nwk::FRAME_DATA, dst);
    h.discover = discover;
    let seq = h.seq;
    z.nwk_send(h, &[0x00, 0x01, 0x06, 0x00, 0x04, 0x01, 0x01, seq, 0x18, seq, 0x0A, 0x00, 0x00, 0x10, 0x01])
}

#[test]
fn a_frame_that_cannot_be_passed_on_is_reported_to_its_source() {
    let mut net = Net::new();
    let n = net.join_node();
    let me = net.nodes[n].short();
    let mut x = neighbor(&mut net, n, 0x2B02);
    let mut y = neighbor(&mut net, n, 0x3C03);
    net.link(x.id, y.id, 0);
    assert_eq!(data_through(&mut x, y.short, 0), TxStatus::Ok);
    net.run(50);
    y.drain();
    assert_eq!(y.rx.iter().filter(|(h, _)| h.src == x.short && h.frame_type == nwk::FRAME_DATA).count(), 1);
    net.air.borrow_mut().ports[y.id].powered = false;
    let t = net.now();
    data_through(&mut x, y.short, 0);
    net.run(2_000);
    let y_bytes = y.short.to_le_bytes();
    assert_eq!(statuses(&net, n, t), vec![(x.short, x.short, vec![nwk::STATUS_LINK_FAILURE, y_bytes[0], y_bytes[1]])]);
    assert!(net.nwk_sent_by(net.nodes[n].id, t).iter().all(|(_, _, p)| p[0] != nwk::CMD_ROUTE_REQUEST));
    let t = net.now();
    data_through(&mut x, 0x4D4D, 0);
    net.run(2_000);
    assert_eq!(statuses(&net, n, t), vec![(x.short, x.short, vec![nwk::STATUS_NO_ROUTE, 0x4D, 0x4D])]);
    let t = net.now();
    data_through(&mut x, 0x4D4E, 1);
    net.run(4_000);
    assert!(statuses(&net, n, t).is_empty());
    assert_eq!(net.nwk_sent_by(net.nodes[n].id, t).iter().filter(|(_, _, p)| p[0] == nwk::CMD_ROUTE_REQUEST).count(), 4);
    net.run(2_000);
    assert_eq!(statuses(&net, n, t), vec![(x.short, x.short, vec![nwk::STATUS_NO_ROUTE, 0x4E, 0x4D])]);
    let report = net.nwk_sent_by(net.nodes[n].id, t).into_iter().find(|(_, _, p)| p[0] == nwk::CMD_NETWORK_STATUS).unwrap();
    assert_eq!(report.1.dst_ext, Some(x.ext));
    let t = net.now();
    net.set_power(n, true);
    net.air.borrow_mut().ports[x.id].powered = false;
    let _ = me;
    net.coord.zcl(net.nodes[n].short(), ON_OFF, 0x01, 0x00, &[]);
    net.run(200);
    assert!(statuses(&net, n, t).is_empty());
}

fn data_from(z: &mut Zed, src: u16) -> usize {
    z.drain();
    z.rx.iter().filter(|(h, _)| h.src == src && h.frame_type == nwk::FRAME_DATA).count()
}

#[test]
fn a_missed_acknowledgement_is_tried_again_and_does_not_cost_the_link() {
    let mut net = Net::new();
    let n = net.join_node();
    let mut x = neighbor(&mut net, n, 0x2B02);
    let mut y = neighbor(&mut net, n, 0x3C03);
    net.link(x.id, y.id, 0);
    let t = net.now();
    y.rx.clear();
    for k in 1..=4 {
        net.air.borrow_mut().ports[y.id].powered = false;
        let t = net.now();
        data_through(&mut x, y.short, 0);
        net.run(2);
        assert!(net.nwk_sent_by(net.nodes[n].id, t).iter().any(|(m, h, _)| h.src == x.short && m.dst == zigbee::mac::Addr::Short(y.short)));
        net.air.borrow_mut().ports[y.id].powered = true;
        net.run(200);
        assert_eq!(data_from(&mut y, x.short), k, "the frame got there on the next attempt");
    }
    assert!(statuses(&net, n, t).is_empty());
    y.rx.clear();
    net.air.borrow_mut().ports[y.id].powered = false;
    net.coord.source_route.insert(y.short, vec![net.nodes[n].short()]);
    net.coord.zcl(y.short, ON_OFF, 0x01, 0x00, &[]);
    net.run(2);
    net.air.borrow_mut().ports[y.id].powered = true;
    net.run(200);
    assert_eq!(data_from(&mut y, 0x0000), 1, "the source-routed frame got there on the next attempt");
    assert!(net.coord.statuses.is_empty());
    net.coord.source_route.clear();
    data_through(&mut x, y.short, 0);
    net.run(50);
    assert_eq!(data_from(&mut y, x.short), 1);
    let t = net.now();
    route_request(&mut y, 0x00, 0x41, 0x5E01);
    net.run(1_000);
    assert!(!relayed_requests(&net, n, 0x41, t).is_empty());
    net.air.borrow_mut().ports[y.id].powered = false;
    let t = net.now();
    data_through(&mut x, y.short, 0);
    net.run(500);
    let y_bytes = y.short.to_le_bytes();
    let failure = vec![(x.short, x.short, vec![nwk::STATUS_LINK_FAILURE, y_bytes[0], y_bytes[1]])];
    assert_eq!(statuses(&net, n, t), failure);
    let to_y = |net: &Net, t: u32| net.nwk_sent_by(net.nodes[n].id, t).iter().filter(|(m, _, _)| m.dst == zigbee::mac::Addr::Short(y.short)).count();
    assert_eq!(to_y(&net, t), 3, "one frame, three attempts");
    let t = net.now();
    data_through(&mut x, y.short, 0);
    net.run(500);
    assert_eq!(to_y(&net, t), 1);
    assert_eq!(statuses(&net, n, t), failure);
    net.air.borrow_mut().ports[y.id].powered = true;
    y.link_status(&[(net.nodes[n].short(), 1)]);
    net.run(50);
    net.air.borrow_mut().ports[y.id].powered = false;
    data_through(&mut x, y.short, 0);
    net.run(2);
    net.air.borrow_mut().ports[y.id].powered = true;
    net.run(200);
    assert_eq!(data_from(&mut y, x.short), 2);
}

#[test]
fn a_neighbor_that_does_not_know_the_link_gets_a_link_status_at_once() {
    let mut net = Net::new();
    let n = net.join_node();
    let me = net.nodes[n].short();
    let id = net.nodes[n].id;
    net.run(20_000);
    let sent = |net: &Net, t: u32| -> Vec<(u32, Vec<u16>)> {
        net.timed_nwk_sent_by(id, t)
            .into_iter()
            .filter(|(_, _, h, p)| h.frame_type == nwk::FRAME_CMD && p[0] == nwk::CMD_LINK_STATUS)
            .map(|(at, _, _, p)| (at, p[2..].chunks(3).map(|e| u16::from_le_bytes([e[0], e[1]])).collect()))
            .collect()
    };
    let mut x = peer(&mut net, n, 0x2B02);
    let t = net.now();
    x.link_status(&[(me, 1)]);
    net.run_until(1_500, "an early Link Status", |net| !sent(net, t).is_empty());
    let early = sent(&net, t);
    assert!(early[0].1.contains(&x.short));
    net.run(1);
    let t = net.now();
    x.link_status(&[]);
    net.run(1_500);
    let again = sent(&net, t);
    assert_eq!(again.len(), 1, "{again:?}");
    assert!(again[0].0 - early[0].0 >= 1_000, "{} ms apart", again[0].0 - early[0].0);
    let t = net.now();
    x.link_status(&[(me, 1)]);
    net.run(1_500);
    assert!(sent(&net, t).is_empty());
}

#[test]
fn after_a_power_cycle_the_links_are_known_both_ways_within_seconds() {
    let mut net = Net::new();
    let n = net.join_node();
    let id = net.nodes[n].id;
    let mut x = neighbor(&mut net, n, 0x2B02);
    net.run(20_000);
    let me = net.nodes[n].short();
    let ls = |net: &Net, t: u32| -> Vec<(u32, Vec<u16>)> {
        net.timed_nwk_sent_by(id, t)
            .into_iter()
            .filter(|(_, _, h, p)| h.frame_type == nwk::FRAME_CMD && p[0] == nwk::CMD_LINK_STATUS)
            .map(|(at, _, _, p)| (at, p[2..].chunks(3).map(|e| u16::from_le_bytes([e[0], e[1]])).collect()))
            .collect()
    };
    let t = net.now();
    let h = x.header(nwk::FRAME_DATA, 0xFFFD);
    let s = x.short.to_le_bytes();
    let mut annce = vec![0x08, 0x00, 0x13, 0x00, 0x00, 0x00, 0x00, 0x51, 0x01, s[0], s[1]];
    annce.extend_from_slice(&x.ext.to_le_bytes());
    annce.push(0x8E);
    x.mac_send(0xFFFF, h, &annce);
    net.run(600);
    let early = ls(&net, t);
    assert_eq!(early.len(), 1, "{early:?}");
    assert!(early[0].1.contains(&x.short));
    net.run(15_000);
    net.power_cycle(n);
    let t = net.now();
    net.run_until(2_000, "the Device_annce", |net| {
        net.nwk_sent_by(id, t).iter().any(|(_, h, _)| h.src == me && h.dst == 0xFFFD && h.frame_type == nwk::FRAME_DATA)
    });
    net.run(300);
    x.link_status(&[(me, 1)]);
    net.run(5_000);
    let first = ls(&net, t);
    assert!(!first.is_empty());
    assert!(first[0].0 - t >= 3_000, "first Link Status {} ms after the start", first[0].0 - t);
    assert!(first[0].1.contains(&x.short), "{first:?}");
}

#[test]
fn every_frame_waiting_for_a_route_is_accounted_for() {
    let mut net = Net::new();
    let n = net.join_node();
    let mut x = neighbor(&mut net, n, 0x2B02);
    let t = net.now();
    for k in 0..14u16 {
        data_through(&mut x, 0x4D00 + k, 1);
        net.run(30);
    }
    net.run(8_000);
    let mut told: Vec<u16> = statuses(&net, n, t)
        .iter()
        .filter(|(m, d, p)| *m == x.short && *d == x.short && p[0] == nwk::STATUS_NO_ROUTE)
        .map(|(_, _, p)| u16::from_le_bytes([p[1], p[2]]))
        .collect();
    told.sort();
    assert_eq!(told, (0..14u16).map(|k| 0x4D00 + k).collect::<Vec<_>>());
}

#[test]
fn a_failed_source_route_is_reported_with_the_destination_of_the_frame() {
    let mut net = Net::new();
    let n = net.join_node();
    let me = net.nodes[n].short();
    let mut y = neighbor(&mut net, n, 0x3C03);
    net.air.borrow_mut().ports[y.id].powered = false;
    let t = net.now();
    net.coord.source_route.insert(0x5E01, vec![y.short, me]);
    net.coord.zcl(0x5E01, ON_OFF, 0x01, 0x00, &[]);
    net.run(500);
    assert_eq!(net.coord.statuses, vec![(me, nwk::STATUS_SOURCE_ROUTE_FAILURE, vec![0x01, 0x5E])], "the frame's destination, not the relay that failed");
    let _ = (t, &mut y);
    net.coord.statuses.clear();
    let mut h = net.coord.header(nwk::FRAME_CMD, 0x5E01, 30);
    h.src_ext = Some(COORD_EXT);
    net.coord.nwk_send(h, &[nwk::CMD_NETWORK_STATUS, nwk::STATUS_NO_ROUTE, 0x34, 0x12], None);
    net.run(500);
    assert!(net.coord.statuses.is_empty());
}

#[test]
fn a_failed_route_to_a_concentrator_is_reported_towards_it_and_not_repaired() {
    let mut net = Net::new();
    let n = net.join_node();
    let me = net.nodes[n].short();
    let mut x = neighbor(&mut net, n, 0x2B02);
    let mut y = neighbor(&mut net, n, 0x3C03);
    net.link(x.id, y.id, 0);
    let mut h = y.header(nwk::FRAME_CMD, 0xFFFC);
    h.src = 0x5E01;
    h.src_ext = Some(0x00124B00_5E010000);
    y.mac_send(0xFFFF, h, &[nwk::CMD_ROUTE_REQUEST, 0x08, 9, 0xFC, 0xFF, 3]);
    net.run(1_000);
    assert!(routes(&mut net, n).contains(&(0x5E01, 0x30, y.short)));
    net.air.borrow_mut().ports[y.id].powered = false;
    let t = net.now();
    data_through(&mut x, 0x5E01, 1);
    net.run(3_000);
    let x_bytes = x.short.to_le_bytes();
    let sent = statuses(&net, n, t);
    assert_eq!(sent.len(), 1);
    assert!([0x0000, x.short].contains(&sent[0].0), "to a router neighbor, not {:#06x}", sent[0].0);
    assert_eq!((sent[0].1, &sent[0].2), (0x5E01, &vec![nwk::STATUS_MANY_TO_ONE_FAILURE, x_bytes[0], x_bytes[1]]));
    assert!(net.nwk_sent_by(net.nodes[n].id, t).iter().all(|(_, _, p)| p[0] != nwk::CMD_ROUTE_REQUEST));
    let rerouted = net.nwk_sent_by(net.nodes[n].id, t).into_iter().filter(|(m, h, _)| h.frame_type == nwk::FRAME_DATA && h.src == x.short && m.dst == zigbee::mac::Addr::Short(0x0000)).count();
    assert_eq!(rerouted, 1);
    assert!(routes(&mut net, n).contains(&(0x5E01, 0x30, y.short)));
    let t = net.now();
    let h = x.header(nwk::FRAME_CMD, 0x5E01);
    x.nwk_send(h, &[nwk::CMD_NETWORK_STATUS, nwk::STATUS_MANY_TO_ONE_FAILURE, 0x99, 0x77]);
    net.run(200);
    let passed: Vec<_> = net.nwk_sent_by(net.nodes[n].id, t).into_iter().filter(|(_, _, p)| p[0] == nwk::CMD_NETWORK_STATUS).collect();
    assert_eq!(passed.len(), 1);
    assert_eq!((passed[0].0.dst, passed[0].1.src, passed[0].1.dst, passed[0].1.radius), (zigbee::mac::Addr::Short(0x0000), x.short, 0x5E01, 29));
    let t = net.now();
    let h = x.header(nwk::FRAME_CMD, 0x5E01);
    x.nwk_send(h, &[nwk::CMD_NETWORK_STATUS, nwk::STATUS_LINK_FAILURE, 0x99, 0x77]);
    net.run(200);
    assert!(net.nwk_sent_by(net.nodes[n].id, t).is_empty());
    net.air.borrow_mut().ports[0].powered = false;
    let t = net.now();
    data_through(&mut x, 0x5E01, 1);
    net.run(3_000);
    assert!(net.nwk_sent_by(net.nodes[n].id, t).iter().all(|(_, _, p)| p[0] != nwk::CMD_ROUTE_REQUEST));
    let _ = me;
}

#[test]
fn what_a_concentrator_announced_outlives_an_ordinary_route_discovery() {
    let (mut net, a, b) = line();
    let (sa, sb) = (net.nodes[a].short(), net.nodes[b].short());
    net.coord.many_to_one_route_request(false);
    net.run(2_000);
    assert!(routes(&mut net, a).contains(&(0x0000, 0x30, 0x0000)), "a route to a concentrator, route record required");
    let records = |net: &Net, src: u16| net.coord.route_records.iter().filter(|(s, _)| *s == src).count();
    let before = records(&net, sa);
    for _ in 0..3 {
        net.coord.zcl(sa, ON_OFF, 0x01, 0x00, &[]);
        net.run(200);
    }
    assert_eq!(before, 1, "next door to the concentrator: one record, which lists no relays, with the first frame");
    assert_eq!(records(&net, sa), before, "and none before the acknowledgements and answers after it");
    assert!(routes(&mut net, a).contains(&(0x0000, 0x10, 0x0000)));
    net.coord.nwk_command(0xFFFC, 30, &[nwk::CMD_ROUTE_REQUEST, 0x00, 0x61, sa.to_le_bytes()[0], sa.to_le_bytes()[1], 0]);
    net.run(500);
    assert!(net.coord.nwk_cmd_rx.iter().any(|(s, p)| *s == sa && p[0] == nwk::CMD_ROUTE_REPLY && p[2] == 0x61));
    assert!(routes(&mut net, a).contains(&(0x0000, 0x10, 0x0000)), "the route request leaves the concentrator's route as it was");
    net.coord.source_route.insert(sb, vec![sa]);
    net.coord.zcl(sb, ON_OFF, 0x01, 0x00, &[]);
    net.run(1_000);
    assert!(routes(&mut net, a).contains(&(0x0000, 0x10, 0x0000)));
    let before = records(&net, sa);
    net.coord.zcl(sa, ON_OFF, 0x01, 0x01, &[]);
    net.run(200);
    assert_eq!(records(&net, sa), before);
    net.coord.many_to_one_route_request(false);
    net.run(2_000);
    assert!(routes(&mut net, a).contains(&(0x0000, 0x10, 0x0000)), "unchanged route: still no record required");
    net.coord.many_to_one_route_request(true);
    net.run(2_000);
    net.coord.zcl(sb, ON_OFF, 0x01, 0x00, &[]);
    net.run(1_000);
    let before = records(&net, sa);
    for _ in 0..2 {
        net.coord.zcl(sa, ON_OFF, 0x01, 0x00, &[]);
        net.run(200);
    }
    assert_eq!(records(&net, sa) - before, 2 * 2);
}

#[test]
fn broadcasts_are_repeated_remembered_and_dropped_as_the_text_says() {
    let mut net = Net::new();
    let n = net.join_node();
    let me = net.nodes[n].short();
    let id = net.nodes[n].id;
    net.run(181_000);
    let copies = |net: &Net, src: u16, seq: u8, since: u32| {
        net.timed_nwk_sent_by(id, since).into_iter().filter(|(_, _, h, _)| h.src == src && h.seq == seq).map(|e| e.0).collect::<Vec<u32>>()
    };
    let t = net.now();
    let h = net.coord.header(nwk::FRAME_DATA, 0xFFFD, 5);
    let seq = h.seq;
    let aps = net.coord.aps_frame(0x08, 1, 0x0003, 0x0104, 1, &[0x01, 1, 0x01]);
    net.coord.nwk_send(h, &aps, None);
    net.run(3_000);
    let sent = copies(&net, 0x0000, seq, t);
    assert_eq!(sent.len(), 2);
    assert!(sent[0] - t <= 64 && sent[1] - sent[0] == 150, "at {sent:?} after {t}");
    let own = net.nwk_sent_by(id, 0).into_iter().find(|(_, h, _)| h.src == me && h.dst == 0xFFFD && h.frame_type == nwk::FRAME_DATA).unwrap();
    assert_eq!(own.1.discover, 0);
    for dst in [0xFFFB, 0xFFFE, 0xFFF8] {
        let t = net.now();
        let h = net.coord.header(nwk::FRAME_DATA, dst, 5);
        let seq = h.seq;
        net.coord.nwk_send(h, &aps, Some(0xFFFF));
        net.run(1_500);
        assert!(copies(&net, 0x0000, seq, t).is_empty(), "{dst:#06x}");
    }
    let mut x = neighbor(&mut net, n, 0x2B02);
    for _ in 0..100 {
        x.link_status(&[(me, 1)]);
        net.run(10);
    }
    let before = net.nodes[n].commands.len();
    net.coord.zcl(0xFFFD, ON_OFF, 0x01, 0x02, &[]);
    net.run(1_500);
    assert_eq!(net.nodes[n].commands.len(), before + 1);
    net.run(9_000);
    let first = net.coord.header(nwk::FRAME_DATA, 0xFFFD, 5);
    let toggle = net.coord.aps_frame(0x08, 1, ON_OFF, 0x0104, 1, &[0x01, 1, 0x02]);
    net.coord.nwk_send(first, &toggle, None);
    net.run(20);
    for _ in 0..79 {
        let h = net.coord.header(nwk::FRAME_DATA, 0xFFFD, 1);
        net.coord.nwk_send(h, &aps, None);
        net.run(20);
    }
    let before = net.nodes[n].commands.len();
    let t = net.now();
    let h = net.coord.header(nwk::FRAME_DATA, 0xFFFD, 5);
    let seq = h.seq;
    net.coord.nwk_send(h, &toggle, None);
    net.run(100);
    assert_eq!(net.nodes[n].commands.len(), before, "no room to remember it: dropped");
    assert!(copies(&net, 0x0000, seq, t).is_empty());
    net.coord.nwk_send(first, &toggle, None);
    net.run(100);
    assert_eq!(net.nodes[n].commands.len(), before, "the first one is still remembered");
    net.run(300);
    let h = net.coord.header(nwk::FRAME_DATA, 0xFFFD, 5);
    net.coord.nwk_send(h, &toggle, None);
    net.run(100);
    assert_eq!(net.nodes[n].commands.len(), before + 1, "the oldest record made room");
    net.run(9_000);
    let mut relayed = net.coord.header(nwk::FRAME_DATA, 0xFFFD, 4);
    relayed.src = 0x5E07;
    x.mac_send(0xFFFF, relayed, &aps);
    net.run(1_500);
    let t = net.now();
    let h = net.coord.header(nwk::FRAME_DATA, 0xFFFD, 5);
    let seq = h.seq;
    net.coord.nwk_send(h, &aps, None);
    net.run(1_500);
    assert_eq!(copies(&net, 0x0000, seq, t).len(), 2, "x did not pass it on: retransmitted");
    let t = net.now();
    let h = net.coord.header(nwk::FRAME_DATA, 0xFFFD, 5);
    let seq = h.seq;
    net.coord.nwk_send(h, &aps, None);
    net.run(70);
    let mut copy = h;
    copy.radius = 4;
    x.mac_send(0xFFFF, copy, &aps);
    net.run(1_500);
    assert_eq!(copies(&net, 0x0000, seq, t).len(), 1, "x passed it on: no retransmission");
    let t = net.now();
    net.coord.nwk_command(0xFFFC, 30, &[nwk::CMD_ROUTE_REQUEST, 0x00, 0x33, 0x01, 0x5E, 0]);
    let aps = net.coord.aps_frame(0x00, 1, ON_OFF, 0x0104, 1, &[0x00, 9, 0x00, 0x00, 0x00]);
    let mut h = net.coord.header(nwk::FRAME_DATA, me, 30);
    h.src = 0x5E02;
    net.coord.nwk_send(h, &aps, None);
    net.run(100);
    let own = net.nwk_sent_by(id, t).into_iter().find(|(_, h, p)| h.src == me && p[0] == nwk::CMD_ROUTE_REQUEST).expect("its own route request");
    let mut echo = own.1;
    echo.radius -= 1;
    x.mac_send(0xFFFF, echo, &own.2);
    net.run(1_000);
    assert!(conflicts(&net).is_empty());
    assert_eq!(net.nodes[n].short(), me);
}

#[test]
fn two_broadcasts_of_two_devices_close_together_are_both_passed_on_and_then_left() {
    let mut net = Net::new();
    let n = net.join_node();
    let id = net.nodes[n].id;
    net.run(181_000);
    let mut x = neighbor(&mut net, n, 0x2B02);
    let aps = net.coord.aps_frame(0x08, 1, 0x0003, 0x0104, 1, &[0x01, 1, 0x01]);
    let copies = |net: &Net, src: u16, seq: u8, since: u32| {
        net.timed_nwk_sent_by(id, since).into_iter().filter(|(_, _, h, _)| h.src == src && h.seq == seq).map(|e| e.0).collect::<Vec<u32>>()
    };
    for gap in [0u32, 2, 5, 10, 15, 20] {
        let t = net.now();
        let a = net.coord.header(nwk::FRAME_DATA, 0xFFFD, 5);
        net.coord.nwk_send(a, &aps, None);
        net.run(gap);
        let t2 = net.now();
        let mut b = net.coord.header(nwk::FRAME_DATA, 0xFFFD, 5);
        b.src = 0x2B02;
        b.seq = a.seq.wrapping_add(100);
        x.mac_send(0xFFFF, b, &aps);
        net.run(1_500);
        let end = net.now();
        let first = copies(&net, 0x0000, a.seq, t);
        let second = copies(&net, 0x2B02, b.seq, t);
        for (what, sent, at) in [("the coordinator's", &first, t), ("x's", &second, t2)] {
            assert_eq!(sent.len(), 2, "{gap} ms apart: {what} passed on at {sent:?} (arrived at {at})");
            assert!(sent[0] - at <= 64, "{gap} ms apart: {what} first passed on {} ms after it arrived", sent[0] - at);
            assert_eq!(sent[1] - sent[0], 150, "{gap} ms apart: {what} again after nwkPassiveAckTimeout");
        }
        net.run(10_000);
        assert!(copies(&net, 0x0000, a.seq, end).is_empty() && copies(&net, 0x2B02, b.seq, end).is_empty(), "{gap} ms apart: sent again late");
    }
    let me = net.nodes[n].short();
    for gap in [0u32, 2, 5, 10, 15, 20] {
        x.link_status(&[(me, 1)]);
        net.run(50);
        let t = net.now();
        let a = net.coord.header(nwk::FRAME_DATA, 0xFFFD, 5);
        net.coord.nwk_send(a, &aps, None);
        net.run(gap);
        let t2 = net.now();
        let mut b = net.coord.header(nwk::FRAME_DATA, 0xFFFD, 4);
        b.src = 0x5E07;
        b.seq = a.seq.wrapping_add(100);
        x.mac_send(0xFFFF, b, &aps);
        net.run(1_500);
        let end = net.now();
        let first = copies(&net, 0x0000, a.seq, t);
        let second = copies(&net, 0x5E07, b.seq, t);
        assert!(first.len() == 2 && first[0] - t <= 64 && first[1] - first[0] == 150, "{gap} ms apart: the coordinator's at {first:?} (arrived at {t})");
        assert!(second.len() == 1 && second[0] - t2 <= 64, "{gap} ms apart: the relayed one at {second:?} (arrived at {t2})");
        net.run(10_000);
        assert!(copies(&net, 0x0000, a.seq, end).is_empty() && copies(&net, 0x5E07, b.seq, end).is_empty(), "{gap} ms apart: sent again late");
    }
}

#[test]
fn a_neighbor_seen_passing_on_a_route_request_or_our_own_broadcast_is_waited_for() {
    let mut net = Net::new();
    let n = net.join_node();
    let me = net.nodes[n].short();
    let id = net.nodes[n].id;
    net.run(181_000);
    let copies = |net: &Net, seq: u8, since: u32| {
        net.timed_nwk_sent_by(id, since).into_iter().filter(|(_, _, h, _)| h.src == 0x0000 && h.seq == seq).count()
    };
    let mut x = neighbor(&mut net, n, 0x2B02);
    let aps = net.coord.aps_frame(0x08, 1, 0x0003, 0x0104, 1, &[0x01, 1, 0x01]);
    let mut relayed = net.coord.header(nwk::FRAME_DATA, 0xFFFD, 4);
    relayed.src = 0x5E07;
    x.mac_send(0xFFFF, relayed, &aps);
    net.run(1_500);
    let mut y = neighbor(&mut net, n, 0x2B03);
    let mut request = net.coord.header(nwk::FRAME_CMD, 0xFFFC, 29);
    request.src = 0x0000;
    y.mac_send(0xFFFF, request, &[nwk::CMD_ROUTE_REQUEST, 0x08, 0x41, 0xFC, 0xFF, 0]);
    net.run(1_500);
    let mut z = neighbor(&mut net, n, 0x2B04);
    let own = net.nwk_sent_by(id, 0).into_iter().find(|(_, h, _)| h.src == me && h.dst == 0xFFFD && h.frame_type == nwk::FRAME_DATA).unwrap();
    let mut echo = own.1;
    echo.radius -= 1;
    z.mac_send(0xFFFF, echo, &own.2);
    net.run(1_500);
    for (y_quiet, what) in [(true, "a route request"), (false, "our own broadcast")] {
        let t = net.now();
        let h = net.coord.header(nwk::FRAME_DATA, 0xFFFD, 5);
        let seq = h.seq;
        net.coord.nwk_send(h, &aps, None);
        net.run(70);
        let mut copy = h;
        copy.radius = 4;
        x.mac_send(0xFFFF, copy, &aps);
        if y_quiet {
            z.mac_send(0xFFFF, copy, &aps);
        } else {
            y.mac_send(0xFFFF, copy, &aps);
        }
        net.run(1_500);
        assert_eq!(copies(&net, seq, t), 2, "a neighbour seen passing on {what} did not pass this on: retransmitted");
    }
    let t = net.now();
    let h = net.coord.header(nwk::FRAME_DATA, 0xFFFD, 5);
    let seq = h.seq;
    net.coord.nwk_send(h, &aps, None);
    net.run(70);
    let mut copy = h;
    copy.radius = 4;
    for r in [&mut x, &mut y, &mut z] {
        r.mac_send(0xFFFF, copy, &aps);
    }
    net.run(1_500);
    assert_eq!(copies(&net, seq, t), 1, "every neighbour that relays passed it on");
}

#[test]
fn a_late_copy_of_its_own_broadcast_is_not_a_conflict() {
    let mut net = Net::new();
    let n = net.join_node();
    let me = net.nodes[n].short();
    let id = net.nodes[n].id;
    net.run(20_000);
    let mut x = neighbor(&mut net, n, 0x2B02);
    let t = net.now();
    net.coord.nwk_command(0xFFFC, 30, &[nwk::CMD_ROUTE_REQUEST, 0x00, 0x34, 0x01, 0x5E, 0]);
    let aps = net.coord.aps_frame(0x00, 1, ON_OFF, 0x0104, 1, &[0x00, 9, 0x00, 0x00, 0x00]);
    let mut h = net.coord.header(nwk::FRAME_DATA, me, 30);
    h.src = 0x5E03;
    net.coord.nwk_send(h, &aps, None);
    net.run(100);
    let own = net.nwk_sent_by(id, t).into_iter().find(|(_, h, p)| h.src == me && p[0] == nwk::CMD_ROUTE_REQUEST).expect("its own route request");
    assert_eq!(own.1.src_ext, Some(net.air.borrow().ports[id].ext));
    for (late, with_ext) in [(10_000, true), (30_000, false)] {
        net.run(late);
        let mut echo = own.1;
        echo.radius -= 2;
        if !with_ext {
            echo.src_ext = None;
        }
        x.mac_send(0xFFFF, echo, &own.2);
        net.run(1_000);
        assert!(conflicts(&net).is_empty(), "a copy {late} ms late");
        assert_eq!(net.nodes[n].short(), me, "a copy {late} ms late");
    }
    let mut other = net.coord.header(nwk::FRAME_DATA, 0xFFFD, 5);
    other.src = me;
    other.seq = net.nwk_sent_by(id, 0).iter().rev().find(|(_, h, _)| h.src == me).map(|(_, h, _)| h.seq).unwrap().wrapping_add(128);
    x.mac_send(0xFFFF, other, &aps);
    net.run(1_000);
    assert_ne!(net.nodes[n].short(), me, "a broadcast of another device under its address is a conflict");
}

#[test]
fn addresses_in_the_header_are_checked_and_unknown_commands_answered() {
    let mut net = Net::new();
    let n = net.join_node();
    let me = net.nodes[n].short();
    let mut x = neighbor(&mut net, n, 0x2B02);
    x.rx.clear();
    let h = x.header(nwk::FRAME_CMD, me);
    x.nwk_send(h, &[0x3F, 1, 2]);
    net.run(100);
    x.drain();
    let commands = |x: &Zed| x.rx.iter().filter(|(h, _)| h.frame_type == nwk::FRAME_CMD && h.dst == x.short).map(|(h, p)| (h.dst, p.clone())).collect::<Vec<_>>();
    assert_eq!(commands(&x), vec![(x.short, vec![nwk::CMD_NETWORK_STATUS, 0x13, 0x3F])]);
    x.rx.clear();
    broadcast_command(&mut x, &[0x3F, 1, 2]);
    let h = x.header(nwk::FRAME_CMD, me);
    x.nwk_send(h, &[nwk::CMD_ROUTE_RECORD, 0]);
    net.run(100);
    x.drain();
    assert!(commands(&x).is_empty());
    let before = net.nodes[n].commands.len();
    let t = net.now();
    let aps = net.coord.aps_frame(0x00, 1, ON_OFF, 0x0104, 1, &[0x01, 1, 0x02]);
    let mut h = net.coord.header(nwk::FRAME_DATA, me, 30);
    h.dst_ext = Some(0x00124B00_DEAD0004);
    net.coord.nwk_send(h, &aps, None);
    net.run(1_000);
    assert_eq!(net.nodes[n].commands.len(), before);
    assert_eq!(conflicts(&net), vec![(me, me)]);
    assert_ne!(net.nodes[n].short(), me);
    assert!(net.nwk_sent_by(net.nodes[n].id, t).iter().all(|(_, _, p)| p[0] != nwk::CMD_ROUTE_REQUEST));
    let me = net.nodes[n].short();
    let ext = net.air.borrow().ports[net.nodes[n].id].ext;
    let mut h = net.coord.header(nwk::FRAME_DATA, me, 30);
    h.dst_ext = Some(ext);
    net.coord.nwk_send(h, &aps, None);
    net.run(1_000);
    assert_eq!(net.nodes[n].commands.len(), before + 1);
    assert_eq!(net.nodes[n].short(), me);
}

#[test]
fn a_broadcast_under_the_bulbs_address_that_it_never_sent_is_another_devices() {
    let mut net = Net::new();
    let n = net.join_node();
    let me = net.nodes[n].short();
    let toggle = net.coord.aps_frame(0x08, 1, ON_OFF, 0x0104, 1, &[0x01, 1, 0x02]);
    net.power_cycle(n);
    net.run(2_000);
    let before = net.nodes[n].commands.len();
    let mut h = net.coord.header(nwk::FRAME_DATA, 0xFFFD, 5);
    h.src = me;
    net.coord.nwk_send(h, &toggle, None);
    net.run(1_000);
    assert_eq!(net.nodes[n].commands.len(), before);
    assert!(conflicts(&net).is_empty());
    net.run(10_000);
    let mut h = net.coord.header(nwk::FRAME_DATA, 0xFFFD, 5);
    h.src = me;
    net.coord.nwk_send(h, &toggle, None);
    net.run(1_000);
    assert_eq!(net.nodes[n].commands.len(), before);
    assert_eq!(conflicts(&net), vec![(me, me)]);
    assert_ne!(net.nodes[n].short(), me);
}

fn other_network(net: &Net) -> Coord {
    let mut other = Coord::new(&net.air);
    other.set_pan(0x2222);
    other.ext_pan = 0xEEEE_EEEE_EEEE_EEEE;
    other.send_key_in_the_clear = true;
    other
}

fn run_both(net: &mut Net, other: &mut Coord, max_ms: u32, done: impl Fn(&Net) -> bool) -> bool {
    for _ in 0..max_ms {
        if done(net) {
            return true;
        }
        net.run(1);
        other.poll();
    }
    done(net)
}

#[test]
fn a_network_that_does_not_let_the_bulb_in_is_followed_by_the_next_one() {
    let mut net = Net::new();
    let mut other = other_network(&net);
    let n = net.add_node();
    let id = net.nodes[n].id;
    let ext = net.air.borrow().ports[id].ext;
    net.link(0, id, 150);
    assert!(run_both(&mut net, &mut other, 20_000, |net| net.nodes[n].stack.state() == State::Up), "joined");
    assert_eq!(net.nodes[n].stack.pan_id(), PAN);
    assert!(other.devices.iter().any(|d| d.ext == ext), "the stronger network was tried first");
    let scans = net.sent_by(id, 0).iter().filter(|f| f[0] & 7 == 3 && f.last() == Some(&0x07)).count();
    assert_eq!(scans, 4, "the four primary channels, once");
    assert_eq!(beacon_of(&mut net, n).depth, 1);
}

#[test]
fn a_network_that_does_not_answer_the_association_is_followed_by_the_next_one() {
    let mut net = Net::new();
    let mut other = other_network(&net);
    other.ignore_associations = true;
    net.coord.depth = 15;
    let n = net.add_node();
    net.link(0, net.nodes[n].id, 150);
    assert!(run_both(&mut net, &mut other, 20_000, |net| net.nodes[n].stack.state() == State::Up), "joined");
    assert_eq!(net.nodes[n].stack.pan_id(), PAN);
    assert_eq!(beacon_of(&mut net, n).depth, 15);
}

#[test]
fn a_network_on_a_primary_channel_does_not_hide_one_on_another_channel() {
    let mut net = Net::new();
    let mut other = other_network(&net);
    net.air.borrow_mut().ports[0].channel = 12;
    let n = net.add_node();
    assert!(run_both(&mut net, &mut other, 30_000, |net| net.nodes[n].stack.state() == State::Up), "joined");
    assert_eq!((net.nodes[n].stack.pan_id(), net.nodes[n].stack.channel()), (PAN, 12));
    assert!(!other.devices.is_empty());
}

#[test]
fn a_parent_that_does_not_answer_is_followed_by_the_next_router_of_the_network() {
    let mut net = Net::new();
    let a = net.join_node();
    net.coord.permit(180);
    net.run(500);
    let b = net.add_node();
    let (pa, pb) = (net.nodes[a].id, net.nodes[b].id);
    net.link(0, pb, 150);
    net.air.borrow_mut().lqi[pb][pa] = 0;
    net.run_until(20_000, "joined through the coordinator", |net| net.nodes[b].stack.state() == State::Up);
    assert_eq!(net.coord.devices.iter().find(|d| d.short == net.nodes[b].short()).unwrap().parent, None);
}

fn permit_requests(net: &Net, n: usize) -> Vec<(u16, Vec<u8>)> {
    let me = net.nodes[n].short();
    net.nwk_sent_by(net.nodes[n].id, 0)
        .into_iter()
        .filter(|(_, h, p)| h.src == me && h.frame_type == nwk::FRAME_DATA && p.len() > 8 && p[2..4] == [0x36, 0x00] && p[4..6] == [0, 0])
        .map(|(_, h, p)| (h.dst, p[9..].to_vec()))
        .collect()
}

#[test]
fn the_network_is_opened_for_others_once_the_bulb_has_joined() {
    let mut net = Net::new();
    let n = net.join_node();
    net.run(1_000);
    let sent = permit_requests(&net, n);
    assert_eq!(sent[0], (0xFFFC, vec![180, 0x01]));
    assert!(sent.iter().all(|s| *s == sent[0]) && sent.len() == 2, "one broadcast, repeated like any other");
    let order: Vec<u16> = net
        .nwk_sent_by(net.nodes[n].id, 0)
        .into_iter()
        .filter(|(_, h, p)| h.frame_type == nwk::FRAME_DATA && p.len() > 8 && p[0] & 3 == 0)
        .map(|(_, _, p)| u16::from_le_bytes([p[2], p[3]]))
        .collect();
    let at = |cluster: u16| order.iter().position(|c| *c == cluster).unwrap();
    assert!(at(0x0013) < at(0x0036) && at(0x0002) < at(0x0036), "clusters in order: {order:x?}");
    assert!(beacon_of(&mut net, n).permit_join);
    net.run(170_000);
    assert!(beacon_of(&mut net, n).permit_join);
    net.run(10_000);
    assert!(!beacon_of(&mut net, n).permit_join);
    net.power_cycle(n);
    net.run(5_000);
    assert!(!beacon_of(&mut net, n).permit_join);
    let short = net.nodes[n].short();
    net.coord.nwk_command(short, 1, &[nwk::CMD_LEAVE, 0x60]);
    net.run_until(20_000, "rejoin", |net| !net.coord.rejoins.is_empty());
    net.run(5_000);
    assert!(!beacon_of(&mut net, n).permit_join);
    assert_eq!(permit_requests(&net, n).len(), 2);
}

#[test]
fn a_trust_center_that_exchanges_no_link_keys_is_no_reason_to_wait_with_opening_the_network() {
    let mut net = Net::new();
    net.coord.stack_revision = 20;
    let n = net.join_node();
    net.run(2_000);
    assert_eq!(permit_requests(&net, n).len(), 2);
    assert!(beacon_of(&mut net, n).permit_join);
}

#[test]
fn a_failed_link_key_exchange_ends_in_leaving_the_network() {
    let mut net = Net::new();
    net.coord.mute_key_exchange = true;
    let n = net.add_node();
    let id = net.nodes[n].id;
    net.run_until(20_000, "associated", |net| net.nodes[n].stack.state() == State::Up);
    let short = net.nodes[n].short();
    net.coord.permit_join = false;
    net.run(20_000);
    let requests: Vec<u32> = net
        .timed_nwk_sent_by(id, 0)
        .into_iter()
        .filter(|(_, _, h, p)| h.frame_type == nwk::FRAME_DATA && h.dst == 0x0000 && p[0] == 0x21)
        .map(|e| e.0)
        .collect();
    assert_eq!(requests.len(), 3);
    assert!(requests.windows(2).all(|w| (5_000..5_010).contains(&(w[1] - w[0]))), "at {requests:?}");
    assert_ne!(net.nodes[n].stack.state(), State::Up);
    let leave = net.coord.nwk_cmd_rx.iter().find(|(s, p)| *s == short && p[0] == nwk::CMD_LEAVE).expect("a Leave");
    assert_eq!(leave.1, vec![nwk::CMD_LEAVE, 0x00]);
    assert!(permit_requests(&net, n).is_empty(), "the network is not opened by a node that is on its way out");
    net.power_cycle(n);
    assert_eq!(net.nodes[n].stack.state(), State::Down);
    net.coord.mute_key_exchange = false;
    net.coord.permit_join = true;
    net.run_until(30_000, "joined again", |net| net.nodes[n].stack.state() == State::Up && net.coord.devices.iter().any(|d| d.verified));
    net.run(30_000);
    assert_eq!(net.nodes[n].stack.state(), State::Up);
}

#[test]
fn a_new_link_key_that_is_the_old_one_fails_the_exchange() {
    let mut net = Net::new();
    net.coord.same_link_key = true;
    let n = net.add_node();
    net.run_until(20_000, "associated", |net| net.nodes[n].stack.state() == State::Up);
    net.coord.permit_join = false;
    net.run(5_000);
    assert_ne!(net.nodes[n].stack.state(), State::Up, "BDB 10.2.5 step 9");
}

#[test]
fn leave_requests_and_bindings_sent_to_everyone_are_not_taken() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let ext = net.air.borrow().ports[net.nodes[n].id].ext;
    for who in [ext, 0] {
        let mut p = who.to_le_bytes().to_vec();
        p.push(0);
        net.coord.zdo(0xFFFD, 0x0034, &p);
        net.run(2_000);
        assert_eq!(net.nodes[n].stack.state(), State::Up);
    }
    assert!(net.coord.last_zdo(short, 0x8034).is_none());
    let mut p = ext.to_le_bytes().to_vec();
    p.extend_from_slice(&[1, 0x06, 0x00, 3]);
    p.extend_from_slice(&COORD_EXT.to_le_bytes());
    p.push(1);
    net.coord.zdo(0xFFFD, 0x0021, &p);
    net.run(500);
    assert!(net.coord.last_zdo(short, 0x8021).is_none());
    let table = |net: &mut Net| {
        net.coord.zdo(short, 0x0033, &[0]);
        net.run(100);
        net.coord.last_zdo(short, 0x8033).unwrap()[2]
    };
    assert_eq!(table(&mut net), 0, "no binding was made");
    assert_eq!(bind(&mut net, n, ON_OFF)[1], 0x00);
    net.coord.zdo_rx.clear();
    net.coord.zdo(0xFFFD, 0x0022, &p);
    net.run(500);
    assert!(net.coord.last_zdo(short, 0x8022).is_none());
    assert_eq!(table(&mut net), 1, "the binding is still there");
}

#[test]
fn remove_device_naming_the_bulb_makes_it_leave() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let ext = net.air.borrow().ports[net.nodes[n].id].ext;
    let mut clear = vec![0x01, 0x55, 0x07];
    clear.extend_from_slice(&ext.to_le_bytes());
    net.coord.nwk_data(short, &clear);
    net.run(2_000);
    assert_eq!(net.nodes[n].stack.state(), State::Up);
    net.coord.remove_device(short, 0x00124B00_DEAD0001);
    net.run(2_000);
    assert_eq!(net.nodes[n].stack.state(), State::Up);
    net.coord.permit_join = false;
    net.coord.remove_device(short, ext);
    net.run(2_000);
    assert_ne!(net.nodes[n].stack.state(), State::Up);
    assert!(net.coord.nwk_cmd_rx.iter().any(|(s, p)| *s == short && p == &vec![nwk::CMD_LEAVE, 0x00]));
}

#[test]
fn what_was_learned_and_configured_in_a_network_is_gone_after_leaving_it() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let ext = net.air.borrow().ports[net.nodes[n].id].ext;
    let seq = net.coord.zcl(short, ON_OFF, 0x00, 0x06, &[0x00, 0x00, 0x00, 0x10, 5, 0, 20, 0]);
    net.run(200);
    assert_eq!(net.coord.last_zcl(short, ON_OFF, 0x07).unwrap(), vec![0x18, seq, 0x07, 0x00]);
    let read_config = |net: &mut Net, short: u16| {
        net.coord.zcl(short, ON_OFF, 0x00, 0x08, &[0x00, 0x00, 0x00]);
        net.run(200);
        net.coord.last_zcl(short, ON_OFF, 0x09).unwrap()[3..].to_vec()
    };
    assert_eq!(read_config(&mut net, short), vec![0x00, 0x00, 0x00, 0x00, 0x10, 5, 0, 20, 0]);
    let mut p = 0u32.to_le_bytes().to_vec();
    p.extend_from_slice(&[0xFF, 3, 0x34, 0x12]);
    net.coord.zdo(0xFFFD, 0x0038, &p);
    net.run(500);
    let mut p = ext.to_le_bytes().to_vec();
    p.push(0);
    net.coord.zdo(short, 0x0034, &p);
    net.run_until(60_000, "second join", |net| {
        net.nodes[n].stack.state() == State::Up && net.nodes[n].short() != short && net.coord.devices.iter().any(|d| d.ext == ext && d.verified)
    });
    let short = net.nodes[n].short();
    assert_eq!(read_config(&mut net, short), vec![0x00, 0x00, 0x00, 0x00, 0x10, 0, 0, 0x10, 0x0E], "the default: 0 to 3600 s");
    net.coord.nwk_command(0xFFFF, 30, &pan_update(9, 0x2B3C));
    net.run(10_000);
    net.coord.set_pan(0x2B3C);
    assert_eq!(beacon_of(&mut net, n).update_id, 9);
}

fn answers_under(net: &mut Net, n: usize, key_seq: u8) -> bool {
    let short = net.nodes[n].short();
    let t = net.now();
    let seq = net.coord.zcl(short, ON_OFF, 0x00, 0x00, &[0x00, 0x00]);
    net.run(300);
    let read = net.coord.last_zcl(short, ON_OFF, 0x01).is_some_and(|f| f[1] == seq);
    let seqs: Vec<u8> = net
        .sent_by(net.nodes[n].id, t)
        .iter()
        .filter(|f| f[0] & 7 == 1)
        .filter_map(|f| {
            let (h, hl) = nwk::Header::parse(&f[9..])?;
            if h.security { nwk::parse_aux(&f[9..], hl).map(|a| a.key_seq) } else { None }
        })
        .collect();
    read && !seqs.is_empty() && seqs.iter().all(|s| *s == key_seq)
}

#[test]
fn a_network_key_comes_from_the_trust_center_under_its_link_key_and_from_nobody_else() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let ext = net.air.borrow().ports[net.nodes[n].id].ext;
    let own = net.coord.device(short).unwrap().tclk;
    let link = |net: &mut Net, key: [u8; 16]| net.coord.devices.iter_mut().find(|d| d.short == short).unwrap().tclk = key;
    let try_key = |net: &mut Net, n: usize, key: &[u8; 16]| {
        net.coord.transport_network_key(short, ext, key, 1, true);
        net.run(200);
        net.coord.switch_key(1);
        net.run(500);
        assert!(answers_under(net, n, 0), "the key was not taken");
    };
    link(&mut net, DISTRIBUTED_LINK_KEY);
    try_key(&mut net, n, &[0x51; 16]);
    link(&mut net, own);
    net.coord.aps_ext = 0x00124B00_DEAD0005;
    try_key(&mut net, n, &[0x52; 16]);
    net.coord.aps_ext = COORD_EXT;
    net.coord.tc_ext = 0x00124B00_DEAD0006;
    try_key(&mut net, n, &[0x53; 16]);
    net.coord.tc_ext = COORD_EXT;
    let new_key = [0x54; 16];
    net.coord.transport_network_key(short, ext, &new_key, 1, true);
    net.run(200);
    let mut x = neighbor(&mut net, n, 0x2B02);
    let mut h = x.header(nwk::FRAME_DATA, 0xFFFD);
    h.discover = 0;
    x.mac_send(0xFFFF, h, &[0x09, 0x77, 0x09, 1]);
    net.run(500);
    assert!(answers_under(&mut net, n, 0));
    net.coord.switch_key(1);
    net.coord.use_key(&new_key, 1, false);
    net.run(500);
    assert!(answers_under(&mut net, n, 1));
}

#[test]
fn a_key_transport_recorded_earlier_is_not_taken_by_a_bulb_waiting_for_its_key_again() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    net.coord.permit_join = false;
    let away = |net: &mut Net, key: [u8; 16], seq: u8| {
        net.set_power(n, false);
        net.coord.key = key;
        net.coord.key_seq = seq;
        net.run(5_000);
        net.air.borrow_mut().ports[net.nodes[n].id].queue.clear();
        net.set_power(n, true);
        net.coord.rejoins.clear();
        net.run_until(200_000, "Trust Center rejoin", |net| !net.coord.rejoins.is_empty());
    };
    let t = net.now();
    away(&mut net, [0x77; 16], 1);
    net.run(2_000);
    assert!(answers_under(&mut net, n, 1));
    let recorded = net
        .sent_by(0, t)
        .into_iter()
        .find(|f| f[0] & 7 == 1 && f[5..7] == short.to_le_bytes() && nwk::Header::parse(&f[9..]).is_some_and(|(h, hl)| !h.security && f[9 + hl] == 0x21))
        .expect("the Transport Key of the first rejoin");
    net.coord.send_key_in_the_clear = true;
    away(&mut net, [0x78; 16], 2);
    net.run(100);
    assert_eq!(net.nodes[n].stack.state(), State::WaitKey);
    net.air.borrow_mut().transmit(0, &recorded);
    net.run(500);
    assert_eq!(net.nodes[n].stack.state(), State::WaitKey, "an old key transport is no key transport");
    net.coord.send_key_in_the_clear = false;
    net.coord.tc_ext = 0x00124B00_DEAD0006;
    let ext = net.air.borrow().ports[net.nodes[n].id].ext;
    net.coord.transport_network_key(short, ext, &[0x78; 16], 2, false);
    net.run(500);
    assert_eq!(net.nodes[n].stack.state(), State::WaitKey);
    net.coord.tc_ext = COORD_EXT;
    net.coord.transport_network_key(short, ext, &[0x78; 16], 2, false);
    net.run(2_000);
    assert!(answers_under(&mut net, n, 2));
}

#[test]
fn every_retransmission_of_an_aps_frame_is_known_for_one() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let aps = net.coord.aps_frame(0x40, 1, ON_OFF, 0x0104, 1, &[0x11, 0x42, 0x02]);
    let before = net.nodes[n].commands.len();
    net.coord.nwk_data(short, &aps);
    for _ in 0..3 {
        net.run(1_600);
        net.coord.nwk_data(short, &aps);
    }
    net.run(200);
    assert_eq!(net.nodes[n].commands.len(), before + 1);
    let aps = net.coord.aps_frame(0x40, 1, ON_OFF, 0x0104, 1, &[0x11, 0x43, 0x02]);
    net.coord.nwk_data(short, &aps);
    net.run(4_800);
    net.coord.nwk_data(short, &aps);
    net.run(200);
    assert_eq!(net.nodes[n].commands.len(), before + 2);
    net.run(2_000);
    net.coord.nwk_data(short, &aps);
    net.run(200);
    assert_eq!(net.nodes[n].commands.len(), before + 3);
    let aps = net.coord.aps_frame(0x40, 1, ON_OFF, 0x0104, 1, &[0x11, 0x44, 0x02]);
    net.coord.nwk_data(short, &aps);
    for _ in 0..15 {
        net.coord.zcl(short, ON_OFF, 0x00, 0x00, &[0x00, 0x00]);
        net.run(100);
    }
    net.coord.nwk_data(short, &aps);
    net.run(200);
    assert_eq!(net.nodes[n].commands.len(), before + 4);
}

#[test]
fn a_rejoin_that_one_router_does_not_grant_is_asked_of_the_next() {
    let mut net = Net::new();
    let a = net.join_node();
    let b = net.join_node();
    let (sa, sb) = (net.nodes[a].short(), net.nodes[b].short());
    let (pa, pb) = (net.nodes[a].id, net.nodes[b].id);
    let ext_b = net.air.borrow().ports[pb].ext;
    net.run(181_000);
    net.link(pa, pb, 180);
    net.coord.permit_join = false;
    let through_a = |net: &Net| net.coord.updates.iter().filter(|u| *u == &(sa, ext_b, 0x00)).count();
    net.coord.refuse_rejoin = true;
    net.coord.nwk_command(sb, 1, &[nwk::CMD_LEAVE, 0x60]);
    net.run(10_000);
    assert_eq!(net.coord.rejoins, vec![(sb, true)], "the coordinator was asked, once");
    assert!(net.coord.nwk_cmd_rx.iter().any(|(s, p)| *s == sb && p == &vec![nwk::CMD_REJOIN_REQUEST, 0x0E]));
    assert_eq!(through_a(&net), 1, "then the next router, which told the Trust Center of the rejoin");
    assert_eq!(net.nodes[b].stack.state(), State::Up);
    assert_eq!(beacon_of(&mut net, b).depth, 2, "one deeper than its new parent");
    net.coord.refuse_rejoin = false;
    net.coord.ignore_rejoin = true;
    net.coord.rejoins.clear();
    net.coord.via.clear();
    net.coord.nwk_command(sb, 1, &[nwk::CMD_LEAVE, 0x60]);
    net.run(10_000);
    assert_eq!(net.coord.rejoins, vec![(sb, true)]);
    assert_eq!(through_a(&net), 2);
    let _ = pb;
}

#[test]
fn a_rejoin_response_has_to_be_for_this_device() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let ext = net.air.borrow().ports[net.nodes[n].id].ext;
    net.coord.permit_join = false;
    net.coord.ignore_rejoin = true;
    net.set_power(n, false);
    net.coord.key = [0x77; 16];
    net.coord.key_seq = 1;
    net.run(5_000);
    net.air.borrow_mut().ports[net.nodes[n].id].queue.clear();
    net.set_power(n, true);
    net.run_until(200_000, "Trust Center rejoin", |net| !net.coord.rejoins.is_empty());
    let respond = |net: &mut Net, to: u64| {
        let mut h = net.coord.header(nwk::FRAME_CMD, short, 1);
        h.security = false;
        h.dst_ext = Some(to);
        let s = short.to_le_bytes();
        net.coord.nwk_send(h, &[nwk::CMD_REJOIN_RESPONSE, s[0], s[1], 0x00], None);
        net.run(50);
    };
    respond(&mut net, 0x00124B00_DEAD0007);
    assert_eq!(net.nodes[n].stack.state(), State::Up);
    respond(&mut net, ext);
    assert_eq!(net.nodes[n].stack.state(), State::WaitKey);
}

#[test]
fn a_reset_by_a_local_action_tells_the_network_before_it_forgets_it() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    net.coord.permit_join = false;
    net.nodes[n].stack.leave();
    net.run(100);
    assert!(!net.nodes[n].stack.is_joined());
    assert!(net.coord.nwk_cmd_rx.iter().any(|(s, p)| *s == short && p == &vec![nwk::CMD_LEAVE, 0x00]), "Leave: no request, no rejoin (BDB 9.5)");
    net.power_cycle(n);
    assert_eq!(net.nodes[n].stack.state(), State::Down, "nothing of the network is stored");
}

#[test]
fn permit_joining_for_255_seconds_means_254() {
    let mut net = Net::new();
    let n = net.join_node();
    net.run(181_000);
    net.coord.permit(255);
    net.run(253_500);
    assert!(beacon_of(&mut net, n).permit_join);
    net.run(700);
    assert!(!beacon_of(&mut net, n).permit_join, "R22 2.4.3.3.7");
}

#[test]
fn for_a_rejoin_the_router_nearest_the_coordinator_is_asked_first() {
    let mut net = Net::new();
    let a = net.join_node();
    let b = net.join_node();
    let (sa, sb) = (net.nodes[a].short(), net.nodes[b].short());
    let (pa, pb) = (net.nodes[a].id, net.nodes[b].id);
    let ext_b = net.air.borrow().ports[pb].ext;
    net.run(181_000);
    net.coord.permit_join = false;
    net.link(0, pb, 170);
    net.coord.nwk_command(sb, 1, &[nwk::CMD_LEAVE, 0x60]);
    net.run(10_000);
    assert_eq!(net.coord.rejoins, vec![(sb, true)], "the coordinator, although the bulb is heard better");
    net.coord.rejoins.clear();
    net.link(0, pb, 60);
    net.coord.nwk_command(sb, 1, &[nwk::CMD_LEAVE, 0x60]);
    net.run(10_000);
    assert!(net.coord.rejoins.is_empty());
    assert!(net.coord.updates.contains(&(sa, ext_b, 0x00)), "rejoined through the other bulb");
    let _ = pa;
}

#[test]
fn of_several_routers_that_notice_an_address_conflict_one_reports_it() {
    let mut net = Net::new();
    let a = net.join_node();
    let b = net.join_node();
    let (sa, sb) = (net.nodes[a].short(), net.nodes[b].short());
    let announce = |z: &mut Zed| {
        let mut aps = vec![0x08, 0x00, 0x13, 0x00, 0x00, 0x00, 0x00, 0x31, 0x32];
        aps.extend_from_slice(&z.short.to_le_bytes());
        aps.extend_from_slice(&z.ext.to_le_bytes());
        aps.push(0x8C);
        let h = z.header(nwk::FRAME_DATA, 0xFFFD);
        z.mac_send(0xFFFF, h, &aps);
    };
    let mut first = Zed::router(&net.air, 0x2B02, sa);
    let mut second = Zed::router(&net.air, 0x3C03, sa);
    announce(&mut first);
    announce(&mut second);
    net.run(2_000);
    assert!(conflicts(&net).is_empty());
    let t = net.now();
    second.short = first.short;
    announce(&mut second);
    net.run(2_000);
    let reports: Vec<(u32, u16)> = [a, b]
        .iter()
        .flat_map(|n| net.timed_nwk_sent_by(net.nodes[*n].id, t))
        .filter(|(_, _, h, p)| h.frame_type == nwk::FRAME_CMD && p[0] == nwk::CMD_NETWORK_STATUS && [sa, sb].contains(&h.src))
        .map(|(at, _, h, _)| (at, h.src))
        .collect();
    let first_at = reports.iter().map(|r| r.0).min().expect("a report");
    assert!(first_at > t && first_at - t <= 64, "after {} ms", first_at - t);
    let reporters: std::collections::HashSet<u16> = reports.iter().map(|r| r.1).collect();
    assert_eq!(reporters.len(), 1, "reports: {reports:?}");
    assert_eq!(conflicts(&net).len(), 1);
    announce(&mut first);
    announce(&mut second);
    net.run(2_000);
    assert_eq!(conflicts(&net).len(), 1);
    let mut waits = vec![];
    for k in 1..6u16 {
        let mut first = Zed::router(&net.air, 0x4000 + k, sa);
        let mut second = Zed::router(&net.air, 0x5000 + k, sa);
        announce(&mut first);
        net.run(1_000);
        let t = net.now();
        second.short = first.short;
        announce(&mut second);
        net.run(1_000);
        let sent = [a, b]
            .iter()
            .flat_map(|n| net.timed_nwk_sent_by(net.nodes[*n].id, t))
            .filter(|(_, _, h, p)| h.frame_type == nwk::FRAME_CMD && p[0] == nwk::CMD_NETWORK_STATUS)
            .map(|e| e.0 - t)
            .min()
            .expect("a report");
        waits.push(sent);
    }
    assert!(waits.iter().all(|w| *w <= 65) && waits.iter().any(|w| *w > 2), "waits {waits:?}");
    let before = conflicts(&net).len();
    let mut first = Zed::router(&net.air, 0x4100, sa);
    let mut second = Zed::router(&net.air, 0x5100, sa);
    let pb = net.nodes[b].id;
    net.link(pb, second.id, 0);
    announce(&mut first);
    net.run(1_000);
    second.short = first.short;
    announce(&mut second);
    net.run(500);
    assert_eq!(conflicts(&net).len(), before + 1, "the bulb that heard it");
    net.link(pb, second.id, 255);
    announce(&mut first);
    net.run(500);
    announce(&mut second);
    net.run(1_000);
    assert_eq!(conflicts(&net).len(), before + 1);
}

#[test]
fn a_link_key_exchange_cut_short_by_a_power_cycle_is_tried_again_and_costs_nothing_if_it_fails() {
    let mut net = Net::new();
    net.coord.mute_key_exchange = true;
    let n = net.add_node();
    net.run_until(20_000, "associated", |net| net.nodes[n].stack.state() == State::Up);
    let short = net.nodes[n].short();
    net.run(3_000);
    net.power_cycle(n);
    let t = net.now();
    net.run(30_000);
    let requests = net.nwk_sent_by(net.nodes[n].id, t).into_iter().filter(|(_, h, p)| h.frame_type == nwk::FRAME_DATA && h.dst == 0x0000 && p[0] == 0x21).count();
    assert_eq!(requests, 3, "Request Key, three times");
    assert_eq!(net.nodes[n].stack.state(), State::Up);
    assert!(!net.coord.nwk_cmd_rx.iter().any(|(s, p)| *s == short && p[0] == nwk::CMD_LEAVE));
    net.coord.mute_key_exchange = false;
    net.power_cycle(n);
    net.run_until(30_000, "link key exchanged", |net| net.coord.devices.iter().any(|d| d.verified));
}

#[test]
fn early_link_status_frames_do_not_age_the_neighbors() {
    let mut net = Net::new();
    let n = net.join_node();
    let id = net.nodes[n].id;
    net.run(20_000);
    let sent = |net: &Net, t: u32| -> Vec<(u32, Vec<u16>)> {
        net.timed_nwk_sent_by(id, t)
            .into_iter()
            .filter(|(_, _, h, p)| h.frame_type == nwk::FRAME_CMD && p[0] == nwk::CMD_LINK_STATUS)
            .map(|(at, _, _, p)| (at, p[2..].chunks(3).map(|e| u16::from_le_bytes([e[0], e[1]])).collect()))
            .collect()
    };
    let mut x = peer(&mut net, n, 0x2B02);
    let t = net.now();
    for _ in 0..45 {
        x.link_status(&[]);
        net.run(2_000);
    }
    let frames = sent(&net, t);
    assert!(frames.len() > 12, "early Link Status frames went out: {}", frames.len());
    let missing: Vec<u32> = frames.iter().filter(|f| !f.1.contains(&0x0000)).map(|f| f.0 - t).collect();
    assert!(missing.is_empty(), "the coordinator left out of the Link Status at {missing:?} ms of {} frames", frames.len());
}

#[test]
fn a_frame_for_a_neighbor_is_not_handed_back_to_the_router_it_came_from() {
    let (mut net, a, b) = line();
    let (sa, sb) = (net.nodes[a].short(), net.nodes[b].short());
    let (pa, pb) = (net.nodes[a].id, net.nodes[b].id);
    net.coord.many_to_one_route_request(false);
    net.run(2_000);
    net.coord.via.insert(sb, sa);
    assert!(routes(&mut net, b).contains(&(0x0000, 0x30, sa)));
    net.link(0, pb, 255);
    net.link(0, pa, 0);
    net.run(50_000);
    net.coord.via.clear();
    net.coord.via.insert(sa, sb);
    let t = net.now();
    let seq = net.coord.zcl(sa, ON_OFF, 0x01, 0x00, &[]);
    net.run(3_000);
    assert!(!net.nodes[a].on, "the command reached a through b");
    let bounced = net.nwk_sent_by(pb, t).into_iter().filter(|(m, h, _)| h.src == sa && h.dst == 0x0000 && m.dst == zigbee::mac::Addr::Short(sa)).count();
    assert_eq!(bounced, 0, "b passes a's frames for the coordinator on to it, not back to a");
    assert_eq!(net.coord.last_zcl(sa, ON_OFF, 0x0B), Some(vec![0x18, seq, 0x0B, 0x00, 0x00]), "the answer came up through b");
}

fn many_to_one_through(concentrator: &mut Zed, relay: &mut Zed, id: u8) {
    let h = concentrator.header(nwk::FRAME_CMD, 0xFFFC);
    relay.mac_send(0xFFFF, h, &[nwk::CMD_ROUTE_REQUEST, 0x08, id, 0xFC, 0xFF, 0]);
}

fn source_routed_through(concentrator: &mut Zed, relay: &mut Zed, me: u16) {
    let mut h = concentrator.header(nwk::FRAME_DATA, relay.short);
    let mut list = [0u16; nwk::MAX_RELAYS];
    list[0] = me;
    h.relays = Some((1, 0, list));
    relay.mac_send(me, h, &RELAYED_APS);
}

fn relayed_for(net: &Net, n: usize, dst: u16, since: u32) -> Vec<u16> {
    net.nwk_sent_by(net.nodes[n].id, since)
        .into_iter()
        .filter(|(_, h, _)| h.frame_type == nwk::FRAME_DATA && h.dst == dst)
        .map(|(m, _, _)| if let zigbee::mac::Addr::Short(d) = m.dst { d } else { 0xFFFF })
        .collect()
}

const RELAYED_APS: [u8; 11] = [0x00, 0x01, 0x06, 0x00, 0x04, 0x01, 0x01, 0x02, 0x01, 0x00, 0x01];

#[test]
fn a_frame_for_a_concentrator_next_door_goes_straight_to_it_and_asks_for_a_route_record() {
    let mut net = Net::new();
    let n = net.join_node();
    let me = net.nodes[n].short();
    let mut x = neighbor(&mut net, n, 0x2B02);
    let mut c = neighbor(&mut net, n, 0x3C03);
    many_to_one_through(&mut c, &mut x, 0x41);
    net.run(500);
    assert!(routes(&mut net, n).contains(&(c.short, 0x30, x.short)));
    source_routed_through(&mut c, &mut x, me);
    net.run(500);
    assert!(routes(&mut net, n).contains(&(c.short, 0x10, x.short)), "the concentrator has the path");
    let t = net.now();
    let h = x.header(nwk::FRAME_DATA, c.short);
    x.mac_send(me, h, &RELAYED_APS);
    net.run(500);
    assert_eq!(relayed_for(&net, n, c.short, t), vec![c.short], "straight to the concentrator, not back to x");
    assert!(routes(&mut net, n).contains(&(c.short, 0x30, c.short)), "the path changed, so a route record is due");
}

#[test]
fn a_frame_for_a_concentrator_that_is_no_fresh_neighbor_goes_where_the_route_says() {
    let mut net = Net::new();
    let n = net.join_node();
    let me = net.nodes[n].short();
    let mut x = neighbor(&mut net, n, 0x2B02);
    let mut c = Zed::router(&net.air, 0x3C03, me);
    net.link(0, c.id, 0);
    many_to_one_through(&mut c, &mut x, 0x41);
    net.run(500);
    let t = net.now();
    let h = x.header(nwk::FRAME_DATA, c.short);
    x.mac_send(me, h, &RELAYED_APS);
    net.run(500);
    assert_eq!(relayed_for(&net, n, c.short, t), vec![x.short], "not a neighbor: the route is kept");
    assert!(routes(&mut net, n).contains(&(c.short, 0x30, x.short)));
    c.link_status(&[(me, 1)]);
    for _ in 0..8 {
        x.link_status(&[(me, 1)]);
        net.run(10_000);
    }
    let t = net.now();
    let h = x.header(nwk::FRAME_DATA, c.short);
    x.mac_send(me, h, &RELAYED_APS);
    net.run(500);
    assert_eq!(relayed_for(&net, n, c.short, t), vec![x.short], "a neighbor not heard for long: the route is kept");
    assert!(routes(&mut net, n).contains(&(c.short, 0x30, x.short)));
}
