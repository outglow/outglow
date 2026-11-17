use crate::sim::*;
use zigbee::aes::Aes128;
use zigbee::nwk;

const GP: u16 = 0x0021;
const SWITCH: u32 = 0x0171_2345;
const KEY: [u8; 16] = [0xC0, 0xC1, 0xC2, 0xC3, 0xC4, 0xC5, 0xC6, 0xC7, 0xC8, 0xC9, 0xCA, 0xCB, 0xCC, 0xCD, 0xCE, 0xCF];

fn commissioning() -> Vec<u8> {
    let mut p = vec![0x02, 0x81, 0xB2];
    p.extend_from_slice(&KEY);
    p.extend_from_slice(&1u32.to_le_bytes());
    p
}

fn gp_frames(net: &Net, cmd: u8) -> Vec<(u16, Vec<u8>)> {
    net.coord.zcl_rx.iter().filter(|(_, c, f)| *c == GP && f[2] == cmd).map(|(s, _, f)| (*s, f[3..].to_vec())).collect()
}

#[test]
fn the_bulb_presents_a_green_power_proxy_endpoint() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let s = short.to_le_bytes();
    let seq = net.coord.zdo(short, 0x0004, &[s[0], s[1], 242]);
    net.run(50);
    assert_eq!(
        net.coord.last_zdo(short, 0x8004).unwrap(),
        vec![seq, 0x00, s[0], s[1], 10, 242, 0xE0, 0xA1, 0x61, 0x00, 0, 0, 1, 0x21, 0x00]
    );
}

#[test]
fn commissioning_frames_are_tunnelled_to_the_sink() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let mut gpd = Gpd::new(&net.air, SWITCH, None);
    gpd.send(0xE0, &commissioning());
    net.run(100);
    assert!(gp_frames(&net, 0x04).is_empty());
    net.coord.gp_commissioning_mode(true, true);
    net.run(100);
    gpd.send(0xE0, &commissioning());
    net.run(100);
    let got = gp_frames(&net, 0x04);
    assert_eq!(got.len(), 1, "the device's three copies make one notification");
    let (src, p) = &got[0];
    assert_eq!(*src, short);
    assert_eq!(&p[..2], &[0x00, 0x08]);
    assert_eq!(&p[2..6], &SWITCH.to_le_bytes());
    assert_eq!(&p[6..10], &(gpd.seq() as u32).to_le_bytes());
    assert_eq!((p[10], p[11] as usize), (0xE0, commissioning().len()));
    assert_eq!(&p[12..12 + commissioning().len()], &commissioning()[..]);
    let at = 12 + commissioning().len();
    assert_eq!(&p[at..at + 2], &short.to_le_bytes());
    net.coord.gp_commissioning_mode(true, false);
    net.run(100);
    gpd.send(0xE0, &commissioning());
    net.run(1_000);
    let got = gp_frames(&net, 0x04);
    assert_eq!(got.len(), 2);
    assert_eq!(got[1].0, 0x2345, "alias: the low half of the source identifier");
    net.coord.gp_commissioning_mode(false, false);
    net.run(100);
    gpd.send(0xE0, &commissioning());
    net.run(1_000);
    assert_eq!(gp_frames(&net, 0x04).len(), 2);
}

#[test]
fn a_paired_device_is_forwarded_decrypted_and_only_when_fresh() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let mut gpd = Gpd::new(&net.air, SWITCH, Some(KEY));
    net.coord.gp_commissioning_mode(true, true);
    net.run(100);
    net.coord.gp_pairing(SWITCH, true, 3, 0, 3, Some(KEY), 100);
    net.run(100);
    gpd.send(0x22, &[]);
    net.run(100);
    let got = gp_frames(&net, 0x00);
    assert_eq!(got.len(), 1);
    let (src, p) = &got[0];
    assert_eq!(*src, short);
    assert_eq!(u16::from_le_bytes([p[0], p[1]]), 1 << 3 | 3 << 6 | 4 << 8 | 1 << 12 | 1 << 14);
    assert_eq!(&p[2..6], &SWITCH.to_le_bytes());
    assert_eq!(&p[6..10], &101u32.to_le_bytes());
    assert_eq!(&p[10..12], &[0x22, 0], "the command in the clear, no payload");
    assert!(gp_frames(&net, 0x04).is_empty(), "the pairing ended commissioning mode: no commissioning notification");
    gpd.send(0x30, &[0x55]);
    net.run(100);
    assert_eq!(&gp_frames(&net, 0x00)[1].1[10..13], &[0x30, 1, 0x55]);
    net.run(5_000);
    gpd.counter = 100;
    gpd.send(0x22, &[]);
    net.run(100);
    assert_eq!(gp_frames(&net, 0x00).len(), 2);
    gpd.counter = 500;
    gpd.key = Some([0x11; 16]);
    gpd.send(0x22, &[]);
    net.run(100);
    assert_eq!(gp_frames(&net, 0x00).len(), 2);
    gpd.key = None;
    gpd.send(0x22, &[]);
    net.run(100);
    assert_eq!(gp_frames(&net, 0x00).len(), 2);
}

#[test]
fn group_notifications_of_two_proxies_collapse_into_one() {
    let mut net = Net::new();
    let a = net.join_node();
    let b = net.join_node();
    let mut gpd = Gpd::new(&net.air, SWITCH, Some(KEY));
    net.coord.gp_pairing(SWITCH, true, 2, 0x0B84, 3, Some(KEY), 100);
    net.run(500);
    gpd.send(0x22, &[]);
    net.run(1_500);
    let got = gp_frames(&net, 0x00);
    assert_eq!(got.len(), 1, "both bulbs heard the switch; the sink gets one notification");
    assert_eq!(got[0].0, 0x2345, "from the device's alias address");
    assert_eq!(net.coord.group_rx.last(), Some(&(0x2345, 0x0B84)));
    assert_eq!(u16::from_le_bytes([got[0].1[0], got[0].1[1]]) & (1 << 5), 1 << 5, "also commissioned group");
    let sent = |net: &Net, n: usize| net.sent_by(net.nodes[n].id, 0).iter().filter(|f| f[3..5] == PAN.to_le_bytes() && f[11..13] == [0xFD, 0xFF] && f[13..15] == [0x45, 0x23]).map(|f| f[16]).collect::<Vec<u8>>();
    let (sa, sb) = (sent(&net, a), sent(&net, b));
    assert!(!sa.is_empty() || !sb.is_empty());
    assert!(sa.iter().chain(sb.iter()).all(|s| *s == gpd.seq().wrapping_sub(9)));
    for n in [a, b] {
        for f in net.sent_by(net.nodes[n].id, 0).iter().filter(|f| f.len() > 30 && f[0] & 7 == 1 && f[11..13] == [0xFD, 0xFF] && f[13..15] == [0x45, 0x23] && f[15] == 30) {
            assert_eq!(&aps_of(f)[..9], &[0x0C, 0x84, 0x0B, 0x21, 0x00, 0xE0, 0xA1, 242, gpd.seq().wrapping_sub(9)]);
        }
    }
}

#[test]
fn pairings_survive_a_power_cycle_and_can_be_removed() {
    let mut net = Net::new();
    let n = net.join_node();
    let mut gpd = Gpd::new(&net.air, SWITCH, Some(KEY));
    net.coord.gp_pairing(SWITCH, true, 3, 0, 3, Some(KEY), 100);
    net.run(100);
    net.power_cycle(n);
    net.run(2_000);
    gpd.send(0x22, &[]);
    net.run(3_000);
    assert_eq!(gp_frames(&net, 0x00).len(), 1);
    net.coord.gp_pairing(SWITCH, false, 3, 0, 3, None, 0);
    net.run(100);
    gpd.send(0x22, &[]);
    net.run(500);
    assert_eq!(gp_frames(&net, 0x00).len(), 1);
}

#[test]
fn the_sinks_answer_is_sent_in_the_devices_receive_window() {
    let mut net = Net::new();
    let a = net.join_node();
    let b = net.join_node();
    let (sa, pa) = (net.nodes[a].short(), net.nodes[a].id);
    let mut gpd = Gpd::new(&net.air, SWITCH, None);
    net.coord.gp_commissioning_mode(true, true);
    net.run(100);
    gpd.send_and_listen(0xE0, &[0x02, 0x23]);
    net.run(100);
    let got = gp_frames(&net, 0x04);
    assert!(!got.is_empty());
    assert_eq!(got[0].1[0] & 0x08, 0x08, "rx after tx");
    assert!(net.air.borrow().timed.is_empty(), "nothing to send to the device yet");
    net.coord.gp_response(sa, 11, SWITCH, 0xF0, &[0x01, 0x62, 0x1A]);
    net.run(100);
    let t_us = (net.now() + 1).wrapping_mul(1000);
    gpd.send_and_listen(0xE0, &[0x02, 0x23]);
    net.run(100);
    let timed = net.air.borrow().timed.clone();
    assert_eq!(timed.len(), 1, "one answer, from the bulb the sink named (the other heard the device too)");
    let (port, at_us, channel, frame) = &timed[0];
    assert_eq!((*port, *channel), (pa, 11));
    assert_eq!(*at_us, t_us + 20_000, "at the start of the receive window");
    assert_eq!(&frame[..2], &[0x01, 0x08]);
    assert_eq!(&frame[3..7], &[0xFF; 4]);
    let mut want = vec![0x8C, 0x80];
    want.extend_from_slice(&SWITCH.to_le_bytes());
    want.extend_from_slice(&[0xF0, 0x01, 0x62, 0x1A]);
    assert_eq!(&frame[7..], &want[..]);
    gpd.send_and_listen(0xE0, &[0x02, 0x23]);
    net.run(100);
    assert_eq!(net.air.borrow().timed.len(), 1);
    let _ = b;
}

#[test]
fn a_channel_configuration_is_delivered_on_the_channel_the_device_listens_on() {
    let mut net = Net::new();
    let n = net.join_node();
    let (short, port) = (net.nodes[n].short(), net.nodes[n].id);
    let mut gpd = Gpd::new(&net.air, SWITCH, None);
    net.coord.gp_commissioning_mode(true, true);
    net.run(100);
    gpd.channel_request();
    net.run(100);
    let got = gp_frames(&net, 0x04);
    assert_eq!(&got[0].1[2..6], &[0, 0, 0, 0]);
    assert_eq!(&got[0].1[10..13], &[0xE3, 1, 0x85]);
    net.coord.gp_response(short, 15, 0, 0xF3, &[0x10]);
    net.run(100);
    assert_eq!(net.air.borrow().ports[port].channel, 15, "the bulb waits there");
    net.air.borrow_mut().ports[gpd.port].channel = 15;
    gpd.channel_request();
    net.run(100);
    let timed = net.air.borrow().timed.clone();
    assert_eq!(timed.len(), 1);
    assert_eq!(timed[0].2, 15);
    assert_eq!(&timed[0].3[7..], &[0x0D, 0xF3, 0x10], "a maintenance frame with the channel configuration");
    assert_eq!(net.air.borrow().ports[port].channel, 11, "and it is back on the network's channel");
    let seq = net.coord.zcl(short, 0x0006, 0x01, 0x00, &[]);
    net.run(100);
    assert_eq!(net.coord.last_zcl(short, 0x0006, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x00, 0x00]);
}

#[test]
fn an_answer_nobody_fetches_is_dropped_and_the_bulb_returns_to_its_channel() {
    let mut net = Net::new();
    let n = net.join_node();
    let (short, port) = (net.nodes[n].short(), net.nodes[n].id);
    net.coord.gp_response(short, 20, 0, 0xF3, &[0x10]);
    net.run(100);
    assert_eq!(net.air.borrow().ports[port].channel, 11);
    let mut gpd = Gpd::new(&net.air, SWITCH, None);
    gpd.channel_request();
    net.run(100);
    assert!(net.air.borrow().timed.is_empty());
    net.coord.gp_commissioning_mode(true, true);
    net.run(100);
    net.coord.gp_response(short, 20, 0, 0xF3, &[0x10]);
    net.run(100);
    assert_eq!(net.air.borrow().ports[port].channel, 20);
    net.run(6_000);
    assert_eq!(net.air.borrow().ports[port].channel, 11);
    net.air.borrow_mut().ports[gpd.port].channel = 20;
    gpd.channel_request();
    net.run(100);
    assert!(net.air.borrow().timed.is_empty());
}

fn alias_frames(net: &Net, n: usize, alias: u16) -> Vec<(u16, u8, u8)> {
    net.sent_by(net.nodes[n].id, 0)
        .iter()
        .filter(|f| f.len() > 17 && f[0] & 7 == 1 && f[3..5] == PAN.to_le_bytes() && f[13..15] == alias.to_le_bytes())
        .map(|f| (u16::from_le_bytes([f[11], f[12]]), f[15], f[16]))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[test]
fn a_device_can_have_several_sinks_and_lose_them_one_by_one() {
    let mut net = Net::new();
    let n = net.join_node();
    let mut gpd = Gpd::new(&net.air, SWITCH, Some(KEY));
    net.coord.gp_pair(&Pairing::new(SWITCH, 3).secured(4, Some(KEY), 100));
    let mut group = Pairing::new(SWITCH, 2).secured(4, Some(KEY), 100);
    group.group = 0x0B84;
    net.coord.gp_pair(&group);
    net.run(200);
    gpd.send(0x22, &[]);
    net.run(1_000);
    let got = gp_frames(&net, 0x00);
    assert_eq!(got.len(), 2, "one by unicast, one to the group");
    for (_, p) in &got {
        assert_eq!(u16::from_le_bytes([p[0], p[1]]) & 0x38, 1 << 3 | 1 << 5, "also unicast, also commissioned group");
    }
    assert_eq!(net.coord.group_rx, vec![(0x2345, 0x0B84)]);
    group.add_sink = false;
    net.coord.gp_pair(&group);
    net.run(200);
    gpd.send(0x22, &[]);
    net.run(1_000);
    assert_eq!(gp_frames(&net, 0x00).len(), 3);
    assert_eq!(net.coord.group_rx.len(), 1);
    let mut unicast = Pairing::new(SWITCH, 3);
    unicast.add_sink = false;
    net.coord.gp_pair(&unicast);
    net.run(200);
    gpd.send(0x22, &[]);
    net.run(1_000);
    assert_eq!(gp_frames(&net, 0x00).len(), 3);
    let short = net.nodes[n].short();
    let mut by_id = vec![0x00];
    by_id.extend_from_slice(&SWITCH.to_le_bytes());
    let seq = net.coord.gp_unicast(short, 0x19, 0x0B, &by_id);
    net.run(100);
    assert_eq!(net.coord.last_zcl(short, GP, 0x0B).unwrap(), vec![0x11, seq, 0x0B, 0x8B, 0, 0xFF, 0]);
    net.coord.gp_pair(&Pairing::new(SWITCH, 3).secured(4, Some(KEY), 200));
    net.run(200);
    gpd.counter = 300;
    gpd.send(0x22, &[]);
    net.run(1_000);
    assert_eq!(gp_frames(&net, 0x00).len(), 4);
    let mut remove = Pairing::new(SWITCH, 3);
    remove.remove_gpd = true;
    assert_eq!(remove.payload().len(), 7);
    net.coord.gp_pair(&remove);
    net.run(200);
    gpd.send(0x22, &[]);
    net.run(1_000);
    assert_eq!(gp_frames(&net, 0x00).len(), 4);
}

#[test]
fn groupcasts_go_out_under_the_alias_and_radius_the_sink_assigned() {
    let mut net = Net::new();
    let n = net.join_node();
    let mut gpd = Gpd::new(&net.air, SWITCH, None);
    let mut derived = Pairing::new(SWITCH, 1);
    derived.group = 0x2345;
    derived.alias = Some(0x1234);
    derived.radius = Some(3);
    net.coord.gp_pair(&derived);
    let mut chosen = Pairing::new(SWITCH, 2);
    chosen.group = 0x0B84;
    chosen.alias = Some(0x0777);
    net.coord.gp_pair(&chosen);
    net.run(200);
    gpd.send(0x22, &[]);
    net.run(1_000);
    assert_eq!(net.coord.group_rx, vec![(0x1234, 0x2345), (0x0777, 0x0B84)]);
    let got = gp_frames(&net, 0x00);
    assert_eq!(u16::from_le_bytes([got[0].1[0], got[0].1[1]]) & 0x38, 1 << 4 | 1 << 5, "also derived group, also commissioned group");
    assert_eq!(alias_frames(&net, n, 0x1234), vec![(0xFFFD, 3, gpd.seq())]);
    assert_eq!(alias_frames(&net, n, 0x0777), vec![(0xFFFD, 3, gpd.seq().wrapping_sub(9))]);
    let mut plain = Pairing::new(0x0000_FFFE, 2);
    plain.group = 0x0B84;
    net.coord.gp_pair(&plain);
    net.run(200);
    let mut odd = Gpd::new(&net.air, 0x0000_FFFE, None);
    odd.send(0x22, &[]);
    net.run(1_000);
    assert_eq!(net.coord.group_rx.last(), Some(&(0xFFF6, 0x0B84)));
}

#[test]
fn the_proxy_table_can_be_read_back() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    net.coord.gp_pair(&Pairing::new(SWITCH, 3).secured(4, Some(KEY), 100));
    let mut second = Pairing::new(0x0000_0042, 2);
    second.group = 0x0B84;
    second.seq_cap = true;
    net.coord.gp_pair(&second);
    net.run(200);
    let mut first = vec![0x58, 0x40];
    first.extend_from_slice(&SWITCH.to_le_bytes());
    first.push(0x03 | 4 << 2);
    first.extend_from_slice(&100u32.to_le_bytes());
    first.extend_from_slice(&KEY);
    first.push(1);
    first.extend_from_slice(&COORD_EXT.to_le_bytes());
    first.extend_from_slice(&[0x00, 0x00, 0xFF]);
    let second = vec![0x38, 0x01, 0x42, 0, 0, 0, 0, 0, 0, 0, 1, 0x84, 0x0B, 0xFF, 0xFF, 0xFF];
    let mut by_id = vec![0x00];
    by_id.extend_from_slice(&SWITCH.to_le_bytes());
    let seq = net.coord.gp_unicast(short, 0x19, 0x0B, &by_id);
    net.run(100);
    let mut want = vec![0x11, seq, 0x0B, 0x00, 2, 0xFF, 1];
    want.extend_from_slice(&first);
    assert_eq!(net.coord.last_zcl(short, GP, 0x0B).unwrap(), want);
    let seq = net.coord.gp_unicast(short, 0x19, 0x0B, &[0x08, 0]);
    net.run(100);
    let mut want = vec![0x11, seq, 0x0B, 0x00, 2, 0, 2];
    want.extend_from_slice(&first);
    want.extend_from_slice(&second);
    assert_eq!(net.coord.last_zcl(short, GP, 0x0B).unwrap(), want);
    let seq = net.coord.gp_unicast(short, 0x19, 0x0B, &[0x08, 1]);
    net.run(100);
    let mut want = vec![0x11, seq, 0x0B, 0x00, 2, 1, 1];
    want.extend_from_slice(&second);
    assert_eq!(net.coord.last_zcl(short, GP, 0x0B).unwrap(), want);
    let seq = net.coord.gp_unicast(short, 0x19, 0x0B, &[0x08, 2]);
    net.run(100);
    assert_eq!(net.coord.last_zcl(short, GP, 0x0B).unwrap(), vec![0x11, seq, 0x0B, 0x8B, 2, 2, 0]);
    let seq = net.coord.gp_unicast(short, 0x08, 0x00, &[0x11, 0x00]);
    net.run(100);
    let mut want = vec![0x10, seq, 0x01, 0x11, 0x00, 0x00, 0x43, (first.len() + second.len()) as u8, 0];
    want.extend_from_slice(&first);
    want.extend_from_slice(&second);
    assert_eq!(net.coord.last_zcl(short, GP, 0x01).unwrap(), want);
    let seq = net.coord.gp_unicast(short, 0x08, 0x00, &[0x10, 0x00, 0x16, 0x00, 0x17, 0x00, 0x20, 0x00, 0x22, 0x00, 0xFD, 0xFF, 0x05, 0x00]);
    net.run(100);
    let mut want = vec![0x10, seq, 0x01];
    want.extend_from_slice(&[0x10, 0x00, 0x00, 0x20, 5]);
    want.extend_from_slice(&[0x16, 0x00, 0x00, 0x1A, 0x2F, 0xAC, 0x09]);
    want.extend_from_slice(&[0x17, 0x00, 0x00, 0x1A, 0xFF, 0xFF, 0xFF]);
    want.extend_from_slice(&[0x20, 0x00, 0x00, 0x18, 0x00]);
    want.extend_from_slice(&[0x22, 0x00, 0x00, 0xF1]);
    want.extend_from_slice(&TCLK);
    want.extend_from_slice(&[0xFD, 0xFF, 0x00, 0x21, 0x02, 0x00]);
    want.extend_from_slice(&[0x05, 0x00, 0x86]);
    assert_eq!(net.coord.last_zcl(short, GP, 0x01).unwrap(), want);
    let seq = net.coord.gp_unicast(short, 0x08, 0x02, &[0x10, 0x00, 0x20, 9]);
    net.run(100);
    assert_eq!(net.coord.last_zcl(short, GP, 0x04).unwrap(), vec![0x10, seq, 0x04, 0x88, 0x10, 0x00]);
    let seq = net.coord.gp_unicast(short, 0x01, 0x0A, &[0x08, 0]);
    net.run(100);
    assert_eq!(net.coord.last_zcl(short, GP, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x0A, 0x81]);
    net.power_cycle(n);
    net.run(2_000);
    let seq = net.coord.gp_unicast(short, 0x19, 0x0B, &[0x08, 1]);
    net.run(100);
    let mut want = vec![0x11, seq, 0x0B, 0x00, 2, 1, 1];
    want.extend_from_slice(&second);
    assert_eq!(net.coord.last_zcl(short, GP, 0x0B).unwrap(), want);
}

#[test]
fn shared_keys_come_from_the_pairing_the_cluster_attribute_or_the_network() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    net.coord.gp_pair(&Pairing::new(SWITCH, 3).secured(1, None, 100));
    net.run(200);
    let mut gpd = Gpd::new(&net.air, SWITCH, Some(NWK_KEY));
    gpd.individual = false;
    gpd.send(0x22, &[]);
    net.run(200);
    let got = gp_frames(&net, 0x00);
    assert_eq!(got.len(), 1);
    assert_eq!((u16::from_le_bytes([got[0].1[0], got[0].1[1]]) >> 8) & 7, 1, "key type: network key");
    gpd.individual = true;
    gpd.send(0x22, &[]);
    net.run(200);
    assert_eq!(gp_frames(&net, 0x00).len(), 1);
    let other = 0x0000_0099;
    net.coord.gp_pair(&Pairing::new(other, 3).secured(2, None, 100));
    net.run(200);
    let mut gpd = Gpd::new(&net.air, other, Some(KEY));
    gpd.individual = false;
    gpd.send(0x22, &[]);
    net.run(200);
    assert_eq!(gp_frames(&net, 0x00).len(), 1);
    let mut write = vec![0x20, 0x00, 0x18, 0x02, 0x21, 0x00, 0xF1];
    write.extend_from_slice(&KEY);
    let seq = net.coord.gp_unicast(short, 0x08, 0x02, &write);
    net.run(100);
    assert_eq!(net.coord.last_zcl(short, GP, 0x04).unwrap(), vec![0x10, seq, 0x04, 0x00]);
    gpd.send(0x22, &[]);
    net.run(200);
    assert_eq!(gp_frames(&net, 0x00).len(), 2);
    net.power_cycle(n);
    net.run(2_000);
    let seq = net.coord.gp_unicast(short, 0x08, 0x00, &[0x20, 0x00, 0x21, 0x00]);
    net.run(100);
    let mut want = vec![0x10, seq, 0x01, 0x20, 0x00, 0x00, 0x18, 0x02, 0x21, 0x00, 0x00, 0xF1];
    want.extend_from_slice(&KEY);
    assert_eq!(net.coord.last_zcl(short, GP, 0x01).unwrap(), want);
    let mut new = Gpd::new(&net.air, 0x0000_0777, Some(KEY));
    new.individual = false;
    net.coord.gp_commissioning_mode(true, true);
    net.run(100);
    new.send(0x22, &[]);
    net.run(200);
    let got = gp_frames(&net, 0x04);
    let p = &got.last().unwrap().1;
    assert_eq!(u16::from_le_bytes([p[0], p[1]]), 3 << 4 | 2 << 6 | 1 << 11);
    assert_eq!(&p[10..12], &[0x22, 0]);
    let mut private = Gpd::new(&net.air, 0x0000_0888, Some([0x5A; 16]));
    private.send(0x22, &[]);
    net.run(200);
    let got = gp_frames(&net, 0x04);
    let p = &got.last().unwrap().1;
    assert_eq!(u16::from_le_bytes([p[0], p[1]]), 3 << 4 | 4 << 6 | 1 << 9 | 1 << 11);
    assert_eq!(p[11], 0, "one protected byte: the command, no payload");
    assert_ne!(p[10], 0x22);
    assert_eq!(p.len(), 12 + 3 + 4);
}

#[test]
fn frames_that_are_only_authenticated_are_checked_too() {
    let mut net = Net::new();
    net.join_node();
    let mut pairing = Pairing::new(SWITCH, 3).secured(4, Some(KEY), 100);
    pairing.security = 2;
    net.coord.gp_pair(&pairing);
    net.run(200);
    let mut gpd = Gpd::new(&net.air, SWITCH, Some(KEY));
    gpd.level = 2;
    gpd.send(0x30, &[0x55]);
    net.run(200);
    let got = gp_frames(&net, 0x00);
    assert_eq!(got.len(), 1);
    assert_eq!((u16::from_le_bytes([got[0].1[0], got[0].1[1]]) >> 6) & 3, 2);
    assert_eq!(&got[0].1[10..13], &[0x30, 1, 0x55]);
    let sent = net.sent_by(gpd.port, 0).last().unwrap().clone();
    let mut forged = sent[7..].to_vec();
    let at = forged.len() - 5;
    forged[at] = 0x56;
    forged[6] = forged[6].wrapping_add(1);
    gpd.send_raw(&forged);
    net.run(200);
    assert_eq!(gp_frames(&net, 0x00).len(), 1);
    gpd.level = 3;
    gpd.send(0x30, &[0x55]);
    net.run(200);
    assert_eq!(gp_frames(&net, 0x00).len(), 1);
}

#[test]
fn unsecured_devices_that_number_their_frames_cannot_be_replayed_at_once() {
    let mut net = Net::new();
    net.join_node();
    let mut pairing = Pairing::new(SWITCH, 3);
    pairing.seq_cap = true;
    net.coord.gp_pair(&pairing);
    net.run(200);
    let mut gpd = Gpd::new(&net.air, SWITCH, None);
    gpd.send(0x22, &[]);
    net.run(200);
    assert_eq!(gp_frames(&net, 0x00).len(), 1);
    net.run(5_000);
    let again = net.sent_by(gpd.port, 0).last().unwrap().clone();
    net.air.borrow_mut().transmit(gpd.port, &again);
    net.run(200);
    assert_eq!(gp_frames(&net, 0x00).len(), 1);
    gpd.send(0x22, &[]);
    net.run(200);
    assert_eq!(gp_frames(&net, 0x00).len(), 2);
}

const SENSOR: u64 = 0x0011_2233_4455_FFFE;

#[test]
fn a_device_known_by_its_ieee_address_is_served_like_the_others() {
    let mut net = Net::new();
    let n = net.join_node();
    let (short, port) = (net.nodes[n].short(), net.nodes[n].id);
    let mut gpd = Gpd::with_ieee(&net.air, SENSOR, 5, None);
    net.coord.gp_commissioning_mode(true, true);
    net.run(100);
    gpd.send(0xE0, &commissioning());
    net.run(200);
    let got = gp_frames(&net, 0x04);
    let p = &got[0].1;
    assert_eq!(u16::from_le_bytes([p[0], p[1]]), 0x02 | 1 << 11);
    assert_eq!(&p[2..10], &SENSOR.to_le_bytes());
    assert_eq!(p[10], 5);
    assert_eq!(p[15], 0xE0);
    let mut pairing = Pairing::new(0, 3).secured(4, Some(KEY), 100);
    pairing.ieee = Some((SENSOR, 5));
    net.coord.gp_pair(&pairing);
    net.run(200);
    gpd.key = Some(KEY);
    gpd.send(0x22, &[]);
    net.run(200);
    let got = gp_frames(&net, 0x00);
    assert_eq!(got.len(), 1);
    let p = &got[0].1;
    assert_eq!(u16::from_le_bytes([p[0], p[1]]), 0x02 | 1 << 3 | 3 << 6 | 4 << 8 | 1 << 12 | 1 << 14);
    assert_eq!(&p[2..10], &SENSOR.to_le_bytes());
    assert_eq!(p[10], 5);
    assert_eq!(&p[11..15], &gpd.counter.to_le_bytes());
    assert_eq!(&p[15..17], &[0x22, 0]);
    gpd.ieee = Some((SENSOR, 6));
    gpd.send(0x22, &[]);
    net.run(200);
    assert_eq!(gp_frames(&net, 0x00).len(), 1);
    gpd.ieee = Some((SENSOR, 5));
    let mut by_id = vec![0x02];
    by_id.extend_from_slice(&SENSOR.to_le_bytes());
    by_id.push(5);
    net.coord.gp_unicast(short, 0x19, 0x0B, &by_id);
    net.run(100);
    let entry = net.coord.last_zcl(short, GP, 0x0B).unwrap();
    assert_eq!(&entry[3..7], &[0x00, 1, 0xFF, 1]);
    assert_eq!(&entry[7..9], &[0x5A, 0x44]);
    assert_eq!(&entry[9..17], &SENSOR.to_le_bytes());
    assert_eq!(entry[17], 5);
    net.coord.gp_commissioning_mode(true, true);
    net.run(100);
    net.coord.gp_response_to(short, 11, Some((SENSOR, 5)), 0, 0xF0, &[0x00]);
    net.run(100);
    gpd.key = None;
    gpd.send_and_listen(0xE0, &[0x02, 0x23]);
    net.run(100);
    let timed = net.air.borrow().timed.clone();
    assert_eq!(timed.len(), 1);
    assert_eq!(timed[0].0, port);
    let frame = &timed[0].3;
    assert_eq!(&frame[..2], &[0x01, 0x0C], "to an extended address, no source");
    assert_eq!(&frame[3..5], &[0xFF, 0xFF]);
    assert_eq!(&frame[5..13], &SENSOR.to_le_bytes());
    assert_eq!(&frame[13..], &[0x8C, 0x82, 5, 0xF0, 0x00]);
    net.coord.gp_commissioning_mode(false, true);
    net.run(100);
    let mut group = Pairing::new(0, 2).secured(4, Some(KEY), gpd.counter);
    group.ieee = Some((SENSOR, 5));
    group.group = 0x0B84;
    net.coord.gp_pair(&group);
    net.run(200);
    gpd.key = Some(KEY);
    gpd.send(0x22, &[]);
    net.run(1_000);
    assert_eq!(net.coord.group_rx.last(), Some(&(0xFFFE ^ 0x4455, 0x0B84)));
}

#[test]
fn commissioning_mode_tunnels_what_a_sink_can_use_and_nothing_else() {
    let mut net = Net::new();
    net.join_node();
    let mut gpd = Gpd::new(&net.air, SWITCH, None);
    net.coord.gp_commissioning_mode(true, true);
    net.run(100);
    let count = |net: &Net| gp_frames(net, 0x04).len();
    gpd.auto_commissioning = true;
    gpd.send(0x22, &[]);
    net.run(200);
    assert_eq!(count(&net), 1);
    gpd.send(0xE0, &commissioning());
    net.run(200);
    assert_eq!(count(&net), 1);
    gpd.auto_commissioning = false;
    gpd.send_raw(&[0x0D, 0xF3, 0x10]);
    net.run(200);
    gpd.send(0xF0, &[0x00]);
    net.run(200);
    gpd.send(0xE3, &[0x85]);
    net.run(200);
    assert_eq!(count(&net), 1);
    let closing = [0x19, 0x77, 0x02, 0x00];
    let aps = net.coord.aps_frame(0x08, 242, 0x0021, 0xA1E0, 242, &closing);
    let mut h = net.coord.header(0, 0xFFFD, 30);
    h.src = 0x4444;
    h.seq = 0x99;
    net.coord.nwk_send(h, &aps, None);
    net.run(200);
    gpd.send(0x22, &[]);
    net.run(200);
    assert_eq!(count(&net), 2);
    net.coord.gp_commissioning_mode(false, true);
    net.run(200);
    gpd.send(0x22, &[]);
    net.run(200);
    assert_eq!(count(&net), 2);
}

fn sent_after(net: &Net, n: usize, since: u32, alias: Option<(u16, u8)>) -> Option<u32> {
    let air = net.air.borrow();
    let port = net.nodes[n].id;
    let own = net.nodes[n].short().to_le_bytes();
    air.log
        .iter()
        .find(|(at, from, f)| {
            *at >= since
                && *from == port
                && f.len() > 30
                && f[0] & 7 == 1
                && f[9] & 3 == 0
                && match alias {
                    None => f[11..13] == [0, 0] && f[13..15] == own,
                    Some((alias, seq)) => f[11..13] == [0xFD, 0xFF] && f[13..15] == alias.to_le_bytes() && f[16] == seq,
                }
        })
        .map(|e| e.0 - since)
}

fn aps_of(frame: &[u8]) -> Vec<u8> {
    let p = &frame[9..];
    let (h, hl) = nwk::Header::parse(p).unwrap();
    assert!(h.security);
    let aux = nwk::parse_aux(p, hl).unwrap();
    let mut copy = p.to_vec();
    let len = nwk::unsecure(&Aes128::new(&NWK_KEY), &aux, &mut copy, hl).expect("a frame under the network key");
    copy[hl + nwk::AUX_LEN..hl + nwk::AUX_LEN + len].to_vec()
}

const ALIAS: u16 = 0x2345;

#[test]
fn commissioning_notifications_wait_as_the_specification_says() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let mut gpd = Gpd::new(&net.air, SWITCH, None);
    let (pg, pn) = (gpd.port, net.nodes[n].id);
    net.coord.gp_commissioning_mode(true, true);
    net.run(100);
    for (lqi, listens, want) in [(255u8, false, 5), (255, true, 32), (40, false, 5), (40, true, 32)] {
        net.air.borrow_mut().link(pg, pn, lqi);
        let t = net.now() + 1;
        if listens {
            gpd.send_and_listen(0xE0, &[0x02, 0x23]);
        } else {
            gpd.send(0x22, &[]);
        }
        net.run(300);
        assert_eq!(sent_after(&net, n, t, None), Some(want), "unicast, link quality {lqi}, listens: {listens}");
    }
    net.coord.gp_commissioning_mode(true, false);
    net.run(100);
    for (lqi, listens, want) in [(255u8, false, 5), (40, false, 5), (255, true, 32), (150, true, 64), (100, true, 96), (40, true, 128)] {
        net.air.borrow_mut().link(pg, pn, lqi);
        let t = net.now() + 1;
        if listens {
            gpd.send_and_listen(0xE0, &[0x02, 0x23]);
        } else {
            gpd.send(0x22, &[]);
        }
        net.run(300);
        assert_eq!(sent_after(&net, n, t, Some((ALIAS, gpd.seq().wrapping_sub(12)))), Some(want), "broadcast, link quality {lqi}, listens: {listens}");
    }
    let t = net.now() + 1;
    gpd.channel_request();
    net.run(300);
    assert_eq!(sent_after(&net, n, t, Some((0x0007, gpd.seq().wrapping_sub(12)))), Some(128));
    net.coord.gp_response(short, 11, SWITCH, 0xF0, &[0x00]);
    net.run(100);
    let t = net.now() + 1;
    gpd.send_and_listen(0xE0, &[0x02, 0x23]);
    net.run(300);
    assert_eq!(sent_after(&net, n, t, Some((ALIAS, gpd.seq().wrapping_sub(12)))), Some(32));
    net.coord.gp_commissioning_mode(true, false);
    net.run(100);
    let t = net.now() + 1;
    gpd.send_and_listen(0xE0, &[0x02, 0x23]);
    net.run(300);
    assert_eq!(sent_after(&net, n, t, Some((ALIAS, gpd.seq().wrapping_sub(12)))), Some(128));
    let _ = short;
}

#[test]
fn notifications_of_a_paired_device_wait_as_the_specification_says() {
    let mut net = Net::new();
    let n = net.join_node();
    let mut gpd = Gpd::new(&net.air, SWITCH, Some(KEY));
    net.coord.gp_pair(&Pairing::new(SWITCH, 3).secured(4, Some(KEY), 100));
    let mut group = Pairing::new(SWITCH, 2).secured(4, Some(KEY), 100);
    group.group = 0x0B84;
    net.coord.gp_pair(&group);
    net.run(200);
    let t = net.now() + 1;
    gpd.send(0x22, &[]);
    net.run(300);
    let group_seq = |gpd: &Gpd| Some((ALIAS, gpd.seq().wrapping_sub(9)));
    assert_eq!((sent_after(&net, n, t, None), sent_after(&net, n, t, group_seq(&gpd))), (Some(5), Some(5)));
    for (lqi, groupcast) in [(255u8, 32), (150, 64), (40, 128)] {
        net.air.borrow_mut().link(gpd.port, net.nodes[n].id, lqi);
        let t = net.now() + 1;
        gpd.send_secured_and_listen(0x22, &[]);
        net.run(300);
        assert_eq!((sent_after(&net, n, t, None), sent_after(&net, n, t, group_seq(&gpd))), (Some(32), Some(groupcast)), "link quality {lqi}");
    }
    assert_eq!(gp_frames(&net, 0x00).len(), 8, "each frame once to the sink and once to its group");
    let t = net.now() + 1;
    gpd.send_secured_and_listen(0xE0, &[0x02, 0x00]);
    net.run(300);
    assert_eq!((sent_after(&net, n, t, None), sent_after(&net, n, t, group_seq(&gpd))), (Some(5), Some(5)));
    let before = gp_frames(&net, 0x00).len();
    gpd.send(0x22, &[]);
    net.run(2);
    let mut remove = Pairing::new(SWITCH, 3);
    remove.remove_gpd = true;
    net.coord.gp_pair(&remove);
    net.coord.gp_pair(&Pairing::new(0x0000_0042, 3));
    net.run(300);
    assert_eq!(gp_frames(&net, 0x00).len(), before);
    let short = net.nodes[n].short();
    let mut response = vec![0x00];
    response.extend_from_slice(&short.to_le_bytes());
    response.push(0);
    response.extend_from_slice(&SWITCH.to_le_bytes());
    response.extend_from_slice(&[0xF0, 1, 0x00]);
    let seq = net.coord.gp_unicast(short, 0x09, 0x06, &response);
    net.run(200);
    assert!(net.coord.zcl_rx.iter().all(|(_, c, f)| *c != GP || f[1] != seq || f[2] != 0x0B), "no default response");
}

#[test]
fn of_two_proxies_the_better_placed_one_tunnels_a_broadcast_notification() {
    let mut net = Net::new();
    let a = net.join_node();
    let b = net.join_node();
    let mut gpd = Gpd::new(&net.air, SWITCH, None);
    net.air.borrow_mut().link(gpd.port, net.nodes[a].id, 100);
    net.coord.gp_commissioning_mode(true, false);
    net.run(1_000);
    gpd.send_and_listen(0xE0, &[0x02, 0x23]);
    net.run(2_000);
    assert_eq!(gp_frames(&net, 0x04).len(), 1);
    let alias = |net: &Net, n: usize| alias_frames(net, n, ALIAS);
    assert_eq!(alias(&net, b).first(), Some(&(0xFFFD, 30, gpd.seq().wrapping_sub(12))));
    assert!(alias(&net, a).iter().all(|f| f.1 == 29), "the first bulb only relays: {:?}", alias(&net, a));
    let p = &gp_frames(&net, 0x04)[0].1;
    assert_eq!(p[p.len() - 1], ((-15 + 110) / 2) as u8 | 3 << 6);
    let sent = net.sent_by(net.nodes[b].id, 0);
    let frame = sent.iter().find(|f| f.len() > 30 && f[0] & 7 == 1 && f[13..15] == ALIAS.to_le_bytes() && f[15] == 30).unwrap();
    let aps = aps_of(frame);
    assert_eq!(&aps[..8], &[0x08, 242, 0x21, 0x00, 0xE0, 0xA1, 242, gpd.seq().wrapping_sub(12)]);
    gpd.send(0x22, &[]);
    net.run(2_000);
    assert_eq!(gp_frames(&net, 0x04).len(), 2);
}

#[test]
fn the_fullest_table_entry_survives_a_power_cut() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let base = || {
        let mut p = Pairing::new(0, 3).secured(4, Some(KEY), 100);
        p.ieee = Some((SENSOR, 5));
        p
    };
    net.coord.gp_pair(&base());
    let mut second = base();
    second.sink = (0x0011_2233_4455_6677, 0x4321);
    net.coord.gp_pair(&second);
    let mut derived = base();
    derived.mode = 1;
    derived.alias = Some(0x1234);
    derived.radius = Some(7);
    net.coord.gp_pair(&derived);
    for group in [0x0B84u16, 0x0B85] {
        let mut chosen = base();
        chosen.mode = 2;
        chosen.group = group;
        chosen.alias = Some(group + 0x1000);
        net.coord.gp_pair(&chosen);
    }
    net.run(500);
    let mut by_id = vec![0x02];
    by_id.extend_from_slice(&SENSOR.to_le_bytes());
    by_id.push(5);
    net.coord.gp_unicast(short, 0x19, 0x0B, &by_id);
    net.run(100);
    let before = net.coord.last_zcl(short, GP, 0x0B).unwrap()[3..].to_vec();
    assert_eq!(&before[..6], &[0x00, 1, 0xFF, 1, 0xDA, 0x61]);
    assert_eq!(before.len(), 4 + 2 + 9 + 2 + 1 + 4 + 16 + 1 + 20 + 1 + 8 + 1);
    let mut third = base();
    third.sink = (0x0099_8877_6655_4433, 0x5555);
    net.coord.gp_pair(&third);
    net.run(200);
    net.power_cycle(n);
    net.run(2_000);
    let seq = net.coord.gp_unicast(short, 0x19, 0x0B, &by_id);
    net.run(100);
    let after = net.coord.last_zcl(short, GP, 0x0B).unwrap();
    assert_eq!(after[1], seq);
    assert_eq!(&after[3..], &before[..]);
}

#[test]
fn of_channel_requests_sent_in_a_row_only_the_one_the_device_listens_after_is_answered() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let mut gpd = Gpd::new(&net.air, SWITCH, None);
    net.coord.gp_commissioning_mode(true, true);
    net.run(100);
    net.coord.gp_response(short, 11, 0, 0xF3, &[0x10]);
    net.run(100);
    gpd.send_raw(&[0x4D, 0xE3, 0x85]);
    net.run(100);
    gpd.send_raw(&[0x4D, 0xE3, 0x85]);
    net.run(100);
    assert!(net.air.borrow().timed.is_empty());
    let got = gp_frames(&net, 0x04);
    assert_eq!(got.len(), 2);
    assert_eq!(got[1].1[0] & 0x08, 0x08);
    let t_us = (net.now() + 1).wrapping_mul(1000);
    gpd.channel_request();
    net.run(100);
    let timed = net.air.borrow().timed.clone();
    assert_eq!(timed.len(), 1);
    assert_eq!(timed[0].1, t_us + 20_000);
    assert_eq!(&timed[0].3[7..], &[0x0D, 0xF3, 0x10]);
}

#[test]
fn frames_that_are_not_a_devices_to_send_open_no_receive_window() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let mut gpd = Gpd::new(&net.air, SWITCH, Some(KEY));
    net.coord.gp_pair(&Pairing::new(SWITCH, 3).secured(4, Some(KEY), 100));
    net.run(200);
    net.coord.gp_commissioning_mode(true, true);
    net.run(100);
    net.coord.gp_response(short, 11, SWITCH, 0xF0, &[0x00]);
    net.run(100);
    let answers = |net: &Net| net.air.borrow().timed.len();
    let tunnelled = |net: &Net| gp_frames(net, 0x04).len();
    gpd.send_and_listen(0xF0, &[0x00]);
    net.run(200);
    gpd.send_and_listen(0xF3, &[0x10]);
    net.run(200);
    assert_eq!((answers(&net), tunnelled(&net)), (0, 0));
    gpd.send_secured_and_listen(0xF0, &[0x00]);
    net.run(200);
    assert_eq!((answers(&net), tunnelled(&net)), (0, 0));
    let key = gpd.key.replace([0x11; 16]);
    gpd.send_secured_and_listen(0x22, &[]);
    net.run(200);
    gpd.key = key;
    assert_eq!((answers(&net), tunnelled(&net)), (0, 1));
    gpd.auto_commissioning = true;
    gpd.send_and_listen(0x22, &[]);
    net.run(200);
    gpd.auto_commissioning = false;
    assert_eq!((answers(&net), tunnelled(&net)), (0, 1));
    gpd.send_secured_and_listen(0x22, &[]);
    net.run(200);
    assert_eq!((answers(&net), tunnelled(&net)), (1, 2));
    for id in [0xFFFF_FFF9u32, 0xFFFF_FFFE, 0xFFFF_FFFF] {
        let mut odd = Gpd::new(&net.air, id, None);
        odd.send(0x22, &[]);
        net.run(200);
    }
    assert_eq!(tunnelled(&net), 2);
    let mut last = Gpd::new(&net.air, 0xFFFF_FFF8, None);
    last.send(0x22, &[]);
    net.run(200);
    assert_eq!(tunnelled(&net), 3);
}

#[test]
fn a_frame_that_could_not_be_checked_is_reported_with_the_key_class_the_frame_names() {
    let mut net = Net::new();
    net.join_node();
    let mut gpd = Gpd::new(&net.air, SWITCH, Some([0x11; 16]));
    net.coord.gp_pair(&Pairing::new(SWITCH, 3).secured(4, Some(KEY), 100));
    net.run(200);
    net.coord.gp_commissioning_mode(true, true);
    net.run(100);
    let options = |net: &Net| {
        let got = gp_frames(net, 0x04);
        let p = &got.last().unwrap().1;
        u16::from_le_bytes([p[0], p[1]])
    };
    gpd.individual = false;
    gpd.send(0xE2, &[]);
    net.run(200);
    assert_eq!(options(&net), 3 << 4 | 0 << 6 | 1 << 9 | 1 << 11);
    gpd.individual = true;
    gpd.send(0xE2, &[]);
    net.run(200);
    assert_eq!(options(&net), 3 << 4 | 4 << 6 | 1 << 9 | 1 << 11);
    gpd.key = Some(KEY);
    gpd.send(0xE2, &[]);
    net.run(200);
    assert_eq!(options(&net), 3 << 4 | 4 << 6 | 1 << 11);
}

#[test]
fn a_sink_that_announces_a_new_address_gets_the_frames_there() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let sink_ext = 0x0011_2233_4455_6677u64;
    let mut pairing = Pairing::new(SWITCH, 3);
    pairing.sink = (sink_ext, 0x4321);
    net.coord.gp_pair(&pairing);
    net.run(200);
    let sinks = |net: &mut Net| {
        let mut by_id = vec![0x00];
        by_id.extend_from_slice(&SWITCH.to_le_bytes());
        net.coord.gp_unicast(short, 0x19, 0x0B, &by_id);
        net.run(100);
        net.coord.last_zcl(short, GP, 0x0B).unwrap()[7 + 6..].to_vec()
    };
    let mut want = vec![1];
    want.extend_from_slice(&sink_ext.to_le_bytes());
    want.extend_from_slice(&[0x21, 0x43, 0xFF]);
    assert_eq!(sinks(&mut net), want);
    let mut annce = vec![0x22, 0x43];
    annce.extend_from_slice(&sink_ext.to_le_bytes());
    annce.push(0x8E);
    net.coord.zdo(0xFFFD, 0x0013, &annce);
    net.run(200);
    want[9..11].copy_from_slice(&[0x22, 0x43]);
    assert_eq!(sinks(&mut net), want);
    net.power_cycle(n);
    net.run(2_000);
    assert_eq!(sinks(&mut net), want);
    pairing.sink = (sink_ext, 0x4323);
    net.coord.gp_pair(&pairing);
    net.run(200);
    want[9..11].copy_from_slice(&[0x23, 0x43]);
    assert_eq!(sinks(&mut net), want);
    let mut annce = vec![0x99, 0x43];
    annce.extend_from_slice(&0x0011_2233_4455_0000u64.to_le_bytes());
    annce.push(0x8E);
    net.coord.zdo(0xFFFD, 0x0013, &annce);
    net.run(200);
    assert_eq!(sinks(&mut net), want);
}

#[test]
fn a_zigbee_device_with_the_address_of_an_alias_is_told_to_take_another() {
    let mut net = Net::new();
    let n = net.join_node();
    let port = net.nodes[n].id;
    let mut group = Pairing::new(SWITCH, 2);
    group.group = 0x0B84;
    net.coord.gp_pair(&group);
    net.run(200);
    let announce = |net: &mut Net, short: u16| {
        let mut annce = short.to_le_bytes().to_vec();
        annce.extend_from_slice(&0x0015_8D00_0123_4567u64.to_le_bytes());
        annce.push(0x80);
        net.coord.zdo(0xFFFD, 0x0013, &annce);
    };
    let from_alias = |net: &Net, since: u32| -> Vec<(u32, u8, Vec<u8>)> {
        let air = net.air.borrow();
        air.log
            .iter()
            .filter(|(at, from, f)| *at >= since && *from == port && f.len() > 30 && f[0] & 7 == 1 && f[13..15] == ALIAS.to_le_bytes() && f[15] == 30)
            .map(|(at, _, f)| (at - since, f[16], aps_of(f)))
            .collect()
    };
    let t = net.now();
    announce(&mut net, 0x2346);
    net.run(500);
    assert!(from_alias(&net, t).is_empty());
    net.coord.gp_commissioning_mode(true, true);
    net.run(100);
    let t = net.now();
    announce(&mut net, ALIAS);
    net.run(500);
    assert!(from_alias(&net, t).is_empty());
    net.coord.gp_commissioning_mode(false, true);
    net.run(100);
    let t = net.now() + 1;
    announce(&mut net, ALIAS);
    net.run(500);
    let sent = from_alias(&net, t);
    assert!(!sent.is_empty());
    let (after, seq, aps) = &sent[0];
    assert!((5..=101).contains(after), "{after} ms after the announcement");
    assert_eq!(*seq, 0);
    assert_eq!(&aps[..8], &[0x08, 0x00, 0x13, 0x00, 0x00, 0x00, 0x00, 0x00]);
    assert_eq!(&aps[9..], &[0x45, 0x23, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
    assert!(sent.iter().all(|s| s.2[8] == aps[8]), "one announcement (and its repeats)");
    let mut gpd = Gpd::new(&net.air, SWITCH, None);
    gpd.send(0x22, &[]);
    net.run(500);
    assert_eq!(net.coord.group_rx.last(), Some(&(ALIAS, 0x0B84)));
}

#[test]
fn a_bulb_whose_address_is_announced_for_an_alias_takes_another() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let ext = net.air.borrow().ports[net.nodes[n].id].ext;
    let mut annce = short.to_le_bytes().to_vec();
    annce.extend_from_slice(&[0xFF; 8]);
    annce.push(0x00);
    net.coord.zdo(0xFFFD, 0x0013, &annce);
    net.run(2_000);
    let now = net.nodes[n].short();
    assert_ne!(now, short);
    assert_eq!(net.coord.devices.iter().find(|d| d.ext == ext).unwrap().short, now, "announced under the new address");
    let seq = net.coord.zcl(now, 0x0006, 0x01, 0x00, &[]);
    net.run(200);
    assert_eq!(net.coord.last_zcl(now, 0x0006, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x00, 0x00]);
    let mut annce = (now + 1).to_le_bytes().to_vec();
    annce.extend_from_slice(&[0xFF; 8]);
    annce.push(0x00);
    net.coord.zdo(0xFFFD, 0x0013, &annce);
    net.run(2_000);
    assert_eq!(net.nodes[n].short(), now);
}

#[test]
fn a_proxy_table_too_long_for_a_frame_is_read_entry_by_entry_and_in_order() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    net.coord.gp_pair(&Pairing::new(SWITCH, 3).secured(4, Some(KEY), 100));
    net.coord.gp_pair(&Pairing::new(SWITCH + 1, 3).secured(4, Some(KEY), 200));
    let mut third = Pairing::new(0x0000_0042, 2);
    third.group = 0x0B84;
    third.seq_cap = true;
    net.coord.gp_pair(&third);
    net.run(300);
    let ids = |rsp: &[u8]| -> Vec<u32> {
        let mut out = vec![];
        let mut at = 7;
        while at + 6 <= rsp.len() {
            let options = u16::from_le_bytes([rsp[at], rsp[at + 1]]);
            out.push(u32::from_le_bytes(rsp[at + 2..at + 6].try_into().unwrap()));
            at += if options & 0x4000 != 0 { 39 } else { 16 };
        }
        out
    };
    let mut seen = vec![];
    let mut index = 0u8;
    for _ in 0..3 {
        let seq = net.coord.gp_unicast(short, 0x19, 0x0B, &[0x08, index]);
        net.run(100);
        let rsp = net.coord.last_zcl(short, GP, 0x0B).unwrap();
        assert_eq!(&rsp[..6], &[0x11, seq, 0x0B, 0x00, 3, index]);
        let got = ids(&rsp);
        assert_eq!(got.len(), rsp[6] as usize);
        assert!(rsp.len() <= 80, "one frame");
        index += rsp[6];
        seen.extend(got);
        if index == 3 {
            break;
        }
    }
    assert_eq!(seen, vec![SWITCH, SWITCH + 1, 0x42], "a reader that goes on where the last answer ended gets every entry");
    let seq = net.coord.gp_unicast(short, 0x08, 0x00, &[0x11, 0x00, 0x10, 0x00]);
    net.run(100);
    assert_eq!(net.coord.last_zcl(short, GP, 0x01).unwrap(), vec![0x10, seq, 0x01, 0x11, 0x00, 0x89, 0x10, 0x00, 0x00, 0x20, 5]);
    let from_bulb = |net: &Net| net.coord.zcl_rx.iter().filter(|(s, c, f)| *s == short && *c == GP && f[2] == 0x0B).count();
    let before = from_bulb(&net);
    let mut by_id = vec![0x00];
    by_id.extend_from_slice(&(SWITCH + 1).to_le_bytes());
    net.coord.gp_command(0x0B, &by_id);
    net.run(300);
    assert_eq!(from_bulb(&net), before + 1);
    let rsp = net.coord.last_zcl(short, GP, 0x0B).unwrap();
    assert_eq!((&rsp[3..7], ids(&rsp)), (&[0x00, 3, 0xFF, 1][..], vec![SWITCH + 1]));
    let mut by_id = vec![0x00];
    by_id.extend_from_slice(&0x0BAD_F00Du32.to_le_bytes());
    net.coord.gp_command(0x0B, &by_id);
    net.coord.gp_command(0x0B, &[0x08, 7]);
    net.run(300);
    assert_eq!(from_bulb(&net), before + 1);
    let _ = n;
}

fn gp_keys(net: &mut Net, short: u16) -> Vec<u8> {
    let seq = net.coord.gp_unicast(short, 0x08, 0x00, &[0x20, 0x00, 0x21, 0x00, 0x22, 0x00]);
    net.run(100);
    let rsp = net.coord.last_zcl(short, GP, 0x01).unwrap();
    assert_eq!(rsp[..2], [0x10, seq]);
    rsp[3..].to_vec()
}

#[test]
fn proxy_attribute_writes_are_checked_and_undivided_ones_write_all_or_nothing() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let before = gp_keys(&mut net, short);
    let write = |net: &mut Net, cmd: u8, p: &[u8]| -> Option<Vec<u8>> {
        net.coord.zcl_rx.clear();
        let seq = net.coord.gp_unicast(short, 0x08, cmd, p);
        net.run(100);
        let r = net.coord.last_zcl(short, GP, 0x04)?;
        assert_eq!(r[1], seq);
        Some(r[3..].to_vec())
    };
    assert_eq!(write(&mut net, 0x02, &[0x20, 0x00, 0x18, 4, 0x20, 0x00, 0x18, 3]), Some(vec![0x87, 0x20, 0x00]), "an invalid value, then the next record");
    assert_eq!(gp_keys(&mut net, short)[4], 3);
    assert_eq!(write(&mut net, 0x02, &[0x21, 0x00, 0x20, 1, 0x20, 0x00, 0x18, 1]), Some(vec![0x8D, 0x21, 0x00]), "a wrong type stops the records");
    assert_eq!(gp_keys(&mut net, short)[4], 3);
    assert_eq!(write(&mut net, 0x02, &[0x99, 0x00, 0x20, 1]), Some(vec![0x86, 0x99, 0x00]));
    assert_eq!(write(&mut net, 0x02, &[0x22, 0x00, 0xF1, 1, 2, 3]), Some(vec![0x8D, 0x22, 0x00]), "a key cut short ends the records");
    assert_eq!(write(&mut net, 0x02, &[0x20, 0x00, 0x18]), Some(vec![0x8D, 0x20, 0x00]), "so does a key type cut short");
    assert_eq!(write(&mut net, 0x03, &[0x20, 0x00, 0x18, 1, 0x10, 0x00, 0x20, 1]), Some(vec![0x88, 0x10, 0x00]));
    assert_eq!(gp_keys(&mut net, short)[4], 3, "undivided: nothing written");
    let mut link = vec![0x22, 0x00, 0xF1];
    link.extend_from_slice(&[0x77; 16]);
    assert_eq!(write(&mut net, 0x05, &link), None, "no response");
    let after = gp_keys(&mut net, short);
    assert_eq!(after[after.len() - 16..], [0x77; 16]);
    assert_ne!(after, before);
    net.coord.zcl_rx.clear();
    net.coord.gp_unicast(short, 0x08, 0x0B, &[0x00, 0x00]);
    net.run(100);
    assert!(net.coord.zcl_rx.iter().all(|(s, _, _)| *s != short), "a default response is not answered");
    let seq = net.coord.gp_unicast(short, 0x08, 0x7F, &[]);
    net.run(100);
    assert_eq!(net.coord.last_zcl(short, GP, 0x0B).unwrap(), vec![0x10, seq, 0x0B, 0x7F, 0x81]);
}

#[test]
fn only_data_frames_with_an_application_id_the_proxy_knows_are_forwarded() {
    let mut net = Net::new();
    net.join_node();
    net.coord.gp_pair(&Pairing::new(SWITCH, 3));
    net.run(200);
    let mut gpd = Gpd::new(&net.air, SWITCH, None);
    let id = SWITCH.to_le_bytes();
    for nwk in [
        [&[0x0E][..], &id, &[0x22]].concat(),
        [&[0x0F][..], &id, &[0x22]].concat(),
        [&[0x8C, 0x01][..], &id, &[0x22]].concat(),
        [&[0x8C, 0x02, 1][..], &[0x22]].concat(),
        [&[0x8C, 0x03][..], &id, &[0x22]].concat(),
        [&[0x8C, 0x08][..], &id, &[0x22]].concat(),
        [&[0x8C, 0x80][..], &id, &[0x22]].concat(),
    ] {
        gpd.send_raw(&nwk);
        net.run(100);
        assert!(gp_frames(&net, 0x00).is_empty(), "{nwk:02x?}");
    }
    gpd.send_raw(&[&[0x0C][..], &id, &[0x22]].concat());
    net.run(100);
    assert_eq!(gp_frames(&net, 0x00).len(), 1);
}

#[test]
fn pairings_the_proxy_cannot_serve_and_gp_frames_it_does_not_take_are_ignored() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let mut full_unicast = Pairing::new(SWITCH, 0);
    full_unicast.sink = (COORD_EXT, 0x0000);
    net.coord.gp_pair(&full_unicast);
    let mut secured_level_one = Pairing::new(0x0000_0202, 3);
    secured_level_one.security = 1;
    net.coord.gp_pair(&secured_level_one);
    net.run(200);
    let seq = net.coord.gp_unicast(short, 0x19, 0x0B, &[0x08, 0]);
    net.run(100);
    assert_eq!(net.coord.last_zcl(short, GP, 0x0B).unwrap()[..4], [0x11, seq, 0x0B, 0x8B], "no entries");
    net.coord.zcl_rx.clear();
    net.coord.gp_unicast(short, 0x1D, 0x0B, &[0x41, 0x11, 0x08, 0]);
    net.run(100);
    assert!(net.coord.zcl_rx.iter().all(|(s, _, _)| *s != short), "manufacturer-specific: dropped");
    let seq = net.coord.gp_unicast(short, 0x11, 0x0B, &[0x08, 0]);
    net.run(100);
    assert_eq!(net.coord.last_zcl(short, GP, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x0B, 0x81], "a client command from a client");
    let seq = net.coord.gp_unicast(short, 0x19, 0x7E, &[]);
    net.run(100);
    assert_eq!(net.coord.last_zcl(short, GP, 0x0B).unwrap(), vec![0x10, seq, 0x0B, 0x7E, 0x81]);
}

#[test]
fn a_response_another_proxy_is_to_deliver_takes_back_the_one_queued_here() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    net.coord.gp_commissioning_mode(true, true);
    net.run(100);
    net.coord.gp_response(short, 11, SWITCH, 0xF0, &[0x00]);
    net.run(100);
    net.coord.gp_response(0x4444, 11, 0x0000_0999, 0xF0, &[0x00]);
    net.run(100);
    net.coord.gp_response(0x4444, 11, SWITCH, 0xF0, &[0x00]);
    net.run(100);
    let mut gpd = Gpd::new(&net.air, SWITCH, None);
    gpd.send_and_listen(0xE0, &[0x02, 0x23]);
    net.run(100);
    assert!(net.air.borrow().timed.is_empty(), "another proxy answers the device");
}

#[test]
fn a_response_without_a_payload_is_delivered_as_the_bare_command() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    net.coord.gp_commissioning_mode(true, true);
    net.run(100);
    let mut p = vec![0x00];
    p.extend_from_slice(&short.to_le_bytes());
    p.push(0);
    p.extend_from_slice(&SWITCH.to_le_bytes());
    p.extend_from_slice(&[0xF1, 0xFF]);
    net.coord.gp_command(0x06, &p);
    net.run(100);
    let mut gpd = Gpd::new(&net.air, SWITCH, None);
    gpd.send_and_listen(0xE0, &[0x02, 0x23]);
    net.run(100);
    let timed = net.air.borrow().timed.clone();
    assert_eq!(timed.len(), 1);
    let mut want = vec![0x8C, 0x80];
    want.extend_from_slice(&SWITCH.to_le_bytes());
    want.push(0xF1);
    assert_eq!(&timed[0].3[7..], &want[..]);
}

fn proxy_entries(net: &mut Net, short: u16) -> (u8, Vec<u8>) {
    let seq = net.coord.gp_unicast(short, 0x19, 0x0B, &[0x08, 0]);
    net.run(100);
    let r = net.coord.last_zcl(short, GP, 0x0B).unwrap();
    assert_eq!(r[1], seq);
    (r[3], r[4..].to_vec())
}

#[test]
fn a_derived_group_pairing_is_removed_and_a_group_paired_twice_keeps_one_entry_with_the_new_alias() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    net.coord.gp_pair(&Pairing::new(SWITCH, 1));
    net.run(100);
    assert_eq!(proxy_entries(&mut net, short).0, 0x00);
    let mut remove = Pairing::new(SWITCH, 1);
    remove.add_sink = false;
    net.coord.gp_pair(&remove);
    net.run(100);
    assert_eq!(proxy_entries(&mut net, short).0, 0x8B, "gone with its only destination");
    for alias in [0x1111u16, 0x2222] {
        let mut p = Pairing::new(SWITCH, 2);
        p.group = 0x4321;
        p.alias = Some(alias);
        net.coord.gp_pair(&p);
        net.run(100);
    }
    let (status, entries) = proxy_entries(&mut net, short);
    assert_eq!((status, entries[0], entries[2]), (0x00, 1, 1));
    assert!(entries.windows(4).any(|w| w == [0x21, 0x43, 0x22, 0x22]), "{entries:02x?}");
    assert!(!entries.windows(2).any(|w| w == [0x11, 0x11]));
}

#[test]
fn a_pairing_that_gives_the_device_the_bulbs_own_address_as_alias_makes_the_bulb_move() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let mut p = Pairing::new(SWITCH, 1);
    p.alias = Some(short);
    net.coord.gp_pair(&p);
    net.run_until(20_000, "a new address", |net| net.nodes[n].stack.short_address() != short);
}

#[test]
fn the_alias_of_a_device_whose_low_half_is_no_address_is_derived_from_both_halves() {
    let mut net = Net::new();
    net.join_node();
    net.coord.gp_commissioning_mode(true, false);
    net.run(100);
    for (id, alias) in [(0x1234_FFFFu32, 0xEDCBu16), (0x0012_0000, 0x0012), (0xFFF8_FFF8, 0x0007), (0x0000_FFFF, 0xFFF7)] {
        let mut gpd = Gpd::new(&net.air, id, None);
        net.coord.zcl_rx.clear();
        gpd.send(0xE0, &commissioning());
        net.run(1_000);
        let got = gp_frames(&net, 0x04);
        assert_eq!(got.first().map(|g| g.0), Some(alias), "device {id:#010x}");
    }
}

#[test]
fn a_frame_from_an_ieee_device_with_no_address_is_not_taken() {
    let mut net = Net::new();
    net.join_node();
    net.coord.gp_commissioning_mode(true, true);
    net.run(100);
    for ieee in [0u64, u64::MAX] {
        let mut gpd = Gpd::with_ieee(&net.air, ieee, 1, None);
        net.coord.zcl_rx.clear();
        gpd.send(0xE0, &commissioning());
        net.run(500);
        assert!(gp_frames(&net, 0x04).is_empty(), "{ieee:#x}");
    }    let mut gpd = Gpd::with_ieee(&net.air, 0x00124B0000001122, 1, None);
    gpd.send(0xE0, &commissioning());
    net.run(500);
    assert_eq!(gp_frames(&net, 0x04).len(), 1, "one with an address is");
}

#[test]
fn an_entry_keeps_its_group_when_its_sink_goes_and_shows_that_it_is_fixed() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let mut group = Pairing::new(SWITCH, 2);
    group.group = 0x4321;
    group.fixed = true;
    net.coord.gp_pair(&group);
    let mut sink = Pairing::new(SWITCH, 3);
    sink.sink = (COORD_EXT, 0x0000);
    net.coord.gp_pair(&sink);
    net.run(100);
    sink.add_sink = false;
    net.coord.gp_pair(&sink);
    net.run(100);
    let (status, entries) = proxy_entries(&mut net, short);
    assert_eq!((status, entries[0], entries[2]), (0x00, 1, 1), "the entry is still there");
    let options = u16::from_le_bytes([entries[3], entries[4]]);
    assert_eq!(options & (1 << 6 | 1 << 8 | 1 << 11), 1 << 8 | 1 << 11, "no sink, a group, fixed: {options:#06x}");
}

#[test]
fn commissioning_mode_ends_by_itself_after_the_default_window_or_the_one_given() {
    let mut net = Net::new();
    net.join_node();
    let mut gpd = Gpd::new(&net.air, SWITCH, None);
    net.coord.gp_commissioning_mode(true, true);
    net.run(179_000);
    gpd.send(0xE0, &commissioning());
    net.run(500);
    assert_eq!(gp_frames(&net, 0x04).len(), 1, "still open after 179 s");
    net.coord.gp_commissioning_mode(true, true);
    net.run(181_000);
    gpd.send(0xE0, &commissioning());
    net.run(500);
    assert_eq!(gp_frames(&net, 0x04).len(), 1, "closed after 180 s");
    net.coord.gp_command(0x02, &[0x01 | 0x02 | 0x20, 10, 0]);
    net.run(11_000);
    gpd.send(0xE0, &commissioning());
    net.run(500);
    assert_eq!(gp_frames(&net, 0x04).len(), 1, "closed after the 10 s given");
}

#[test]
fn a_secured_frame_is_taken_once_and_a_command_for_the_device_from_it_is_dropped() {
    let mut net = Net::new();
    net.join_node();
    net.coord.gp_pair(&Pairing::new(SWITCH, 3).secured(4, Some(KEY), 100));
    net.run(200);
    let mut gpd = Gpd::new(&net.air, SWITCH, Some(KEY));
    gpd.send(0x22, &[]);
    net.run(200);
    assert_eq!(gp_frames(&net, 0x00).len(), 1);
    let frame = net.sent_by(gpd.port, 0).last().unwrap().clone();
    net.run(3_000);
    net.air.borrow_mut().transmit(gpd.port, &frame);
    net.run(200);
    assert_eq!(gp_frames(&net, 0x00).len(), 1, "the same frame counter again: a replay");
    gpd.send(0xF0, &[0x00]);
    net.run(200);
    assert_eq!(gp_frames(&net, 0x00).len(), 1, "0xF0 is a command to the device");
    gpd.send(0xEF, &[]);
    net.run(200);
    assert_eq!(gp_frames(&net, 0x00).len(), 2);
}

#[test]
fn an_ieee_device_is_matched_to_its_entry_by_endpoint_unless_either_is_all() {
    let mut net = Net::new();
    net.join_node();
    let mut pairing = Pairing::new(0, 3);
    pairing.ieee = Some((SENSOR, 5));
    net.coord.gp_pair(&pairing);
    net.run(200);
    for (endpoint, forwarded) in [(6u8, 0usize), (5, 1), (0xFF, 2), (0, 3)] {
        let mut gpd = Gpd::with_ieee(&net.air, SENSOR, endpoint, None);
        gpd.send(0x22, &[]);
        net.run(2_500);
        assert_eq!(gp_frames(&net, 0x00).len(), forwarded, "endpoint {endpoint}");
    }
}

#[test]
fn a_notification_says_when_the_device_listens_after_sending() {
    let mut net = Net::new();
    net.join_node();
    net.coord.gp_pair(&Pairing::new(SWITCH, 3));
    net.run(200);
    let mut gpd = Gpd::new(&net.air, SWITCH, None);
    gpd.send(0x22, &[]);
    net.run(2_500);
    gpd.send_and_listen(0x22, &[]);
    net.run(500);
    let got = gp_frames(&net, 0x00);
    let options: Vec<u16> = got.iter().map(|g| u16::from_le_bytes([g.1[0], g.1[1]]) & (1 << 11)).collect();
    assert_eq!(options, vec![0, 1 << 11]);
}

fn device_announce(net: &mut Net, short: u16) {
    let mut annce = short.to_le_bytes().to_vec();
    annce.extend_from_slice(&0x0015_8D00_0123_4567u64.to_le_bytes());
    annce.push(0x80);
    net.coord.zdo(0xFFFD, 0x0013, &annce);
}

fn announced_for(net: &Net, n: usize, since: u32, alias: u16) -> std::collections::BTreeSet<u8> {
    net.nwk_sent_by(net.nodes[n].id, since)
        .into_iter()
        .filter(|(_, h, p)| h.src == alias && p.len() > 8 && p[2..4] == [0x13, 0x00])
        .map(|(_, _, p)| p[7])
        .collect()
}

#[test]
fn an_announced_address_clashing_with_an_assigned_alias_is_answered_once() {
    let mut net = Net::new();
    let n = net.join_node();
    let mut derived = Pairing::new(SWITCH, 1);
    derived.alias = Some(0x1111);
    net.coord.gp_pair(&derived);
    let mut group = Pairing::new(0x0000_0303, 2);
    group.group = 0x0B84;
    group.alias = Some(0x2222);
    net.coord.gp_pair(&group);
    net.run(200);
    for alias in [0x1111u16, 0x2222] {
        let t = net.now();
        device_announce(&mut net, alias);
        net.run(20);
        device_announce(&mut net, alias);
        net.run(500);
        assert_eq!(announced_for(&net, n, t, alias).len(), 1, "one announcement for {alias:#06x}");
    }
    let t = net.now();
    device_announce(&mut net, 0x3333);
    net.run(500);
    assert!(announced_for(&net, n, t, 0x3333).is_empty());
}
