use bulb_core::command::Command;
use crate::sim::*;
use zigbee::hal::TxStatus;
use zigbee::nwk;
use zigbee::stack::{State, CHILDREN, INDIRECT, INDIRECT_PER_CHILD};

const ON_OFF: u16 = 0x0006;

fn more_sleepy_children(net: &mut Net, a: usize, n: usize) -> Vec<Zed> {
    let sa = net.nodes[a].short();
    net.coord.permit(254);
    net.run(600);
    (0..n)
        .map(|_| {
            let mut zed = Zed::new(&net.air, true);
            net.link(0, zed.id, 0);
            assert!(zed.join(net, sa), "the end device gets its address and the network key");
            zed
        })
        .collect()
}

fn open_network() -> (Net, usize) {
    let mut net = Net::new();
    let a = net.join_node();
    net.coord.permit(120);
    net.run(600);
    (net, a)
}

#[test]
fn a_router_out_of_the_coordinators_range_joins_through_the_bulb() {
    let (mut net, a) = open_network();
    let sa = net.nodes[a].short();
    let b = net.add_node();
    let pb = net.nodes[b].id;
    net.link(0, pb, 0);
    let ext = net.air.borrow().ports[pb].ext;
    net.run_until(30_000, "second bulb joined through the first", |net| {
        net.nodes[b].stack.state() == State::Up && net.coord.devices.iter().any(|d| d.ext == ext && d.announced && d.verified)
    });
    let d = net.coord.devices.iter().find(|d| d.ext == ext).unwrap();
    assert_eq!(d.parent, Some(sa), "the Trust Center heard about it from the first bulb");
    let sb = net.nodes[b].short();
    assert_eq!(d.short, sb);
    assert_ne!(sb, sa);
    assert_eq!(net.nodes[a].stack.child_count(), 1);
    assert_eq!(net.air.borrow().ports[net.nodes[a].id].poll_children, Some(vec![]));
    let seq = net.coord.zcl(sb, ON_OFF, 0x01, 0x00, &[]);
    net.run(500);
    assert!(!net.nodes[b].on);
    assert_eq!(net.coord.last_zcl(sb, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x00, 0x00]);
    net.run(20_000);
    assert_eq!(net.nodes[a].stack.child_count(), 0);
    let seq = net.coord.zcl(sb, ON_OFF, 0x01, 0x01, &[]);
    net.run(500);
    assert_eq!(net.coord.last_zcl(sb, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x01, 0x00]);
}

#[test]
fn a_router_that_has_just_joined_is_a_neighbor_with_a_working_link_at_once() {
    let (mut net, a) = open_network();
    let ida = net.nodes[a].id;
    let b = net.add_node();
    let pb = net.nodes[b].id;
    net.link(0, pb, 0);
    let ext = net.air.borrow().ports[pb].ext;
    net.run_until(30_000, "second bulb announced", |net| net.coord.devices.iter().any(|d| d.ext == ext && d.announced));
    let sb = net.nodes[b].short();
    let t = net.now();
    net.run(600);
    let ls: Vec<Vec<u8>> = net
        .nwk_sent_by(ida, t)
        .into_iter()
        .filter(|(_, h, p)| h.frame_type == nwk::FRAME_CMD && p[0] == nwk::CMD_LINK_STATUS)
        .map(|(_, _, p)| p)
        .collect();
    assert_eq!(ls.len(), 1);
    let entry = ls[0][2..].chunks(3).find(|e| u16::from_le_bytes([e[0], e[1]]) == sb).map(|e| e[2]);
    assert!(entry.is_some_and(|c| c >> 4 != 0), "the newcomer listed with an outgoing cost: {:x?}", ls[0]);
    net.run_until(20_000, "link key exchange through the first bulb", |net| net.coord.devices.iter().any(|d| d.ext == ext && d.verified));
}

#[test]
fn nobody_joins_through_the_bulb_while_the_network_is_closed() {
    let mut net = Net::new();
    let a = net.join_node();
    net.run(181_000);
    let b = net.add_node();
    let pb = net.nodes[b].id;
    net.link(0, pb, 0);
    net.run(20_000);
    assert_ne!(net.nodes[b].stack.state(), State::Up);
    assert_eq!(net.nodes[a].stack.child_count(), 0);
    let mut zed = Zed::new(&net.air, false);
    let pz = zed.id;
    net.link(0, pz, 0);
    let sa = net.nodes[a].short();
    assert!(!zed.join(&mut net, sa));
    assert_eq!(zed.mac_rx.iter().filter(|(_, p)| p[0] == 0x02).map(|(_, p)| p[3]).next(), Some(0x02), "access denied");
    net.coord.permit(2);
    net.run(3_000);
    net.coord.permit_join = false;
    let mut late = Zed::new(&net.air, false);
    let pl = late.id;
    net.link(0, pl, 0);
    assert!(!late.join(&mut net, sa));
}

fn with_sleepy_child() -> (Net, usize, Zed) {
    let (mut net, a) = open_network();
    let mut zed = Zed::new(&net.air, true);
    let pz = zed.id;
    net.link(0, pz, 0);
    let sa = net.nodes[a].short();
    assert!(zed.join(&mut net, sa), "the end device gets its address and the network key");
    net.run(200);
    (net, a, zed)
}

#[test]
fn a_sleepy_end_device_joins_and_is_reported_to_the_trust_center() {
    let (net, a, zed) = with_sleepy_child();
    let d = net.coord.devices.iter().find(|d| d.ext == zed.ext).expect("Update-Device reached the Trust Center");
    assert_eq!(d.short, zed.short);
    assert_eq!(d.parent, Some(net.nodes[a].short()));
    assert!(d.announced, "the child's Device Announce was relayed");
    assert_eq!(zed.key, Some(NWK_KEY));
    assert_eq!(net.nodes[a].stack.child_count(), 1);
}

#[test]
fn frames_for_a_sleeping_child_wait_for_its_poll() {
    let (mut net, a, mut zed) = with_sleepy_child();
    let sa = net.nodes[a].short();
    let t = net.now();
    net.coord.zcl(zed.short, ON_OFF, 0x11, 0x02, &[]);
    net.run(1_000);
    let child = zed.short.to_le_bytes();
    let to_child = move |net: &Net, since: u32| {
        net.sent_by(net.nodes[a].id, since).iter().filter(|f| f[0] & 7 == 1 && f[5..7] == child).count()
    };
    assert_eq!(to_child(&net, t), 0, "nothing is sent while the child sleeps");
    zed.poll(&mut net, 20);
    assert_eq!(to_child(&net, t), 1);
    let (h, aps) = zed.rx.last().expect("the frame arrives with the poll");
    assert_eq!((h.src, h.dst), (0x0000, zed.short));
    assert_eq!(&aps[2..4], &ON_OFF.to_le_bytes());
    zed.rx.clear();
    zed.poll(&mut net, 20);
    assert!(zed.rx.is_empty());
    let t = net.now();
    net.coord.zcl(zed.short, ON_OFF, 0x11, 0x02, &[]);
    net.run(9_000);
    zed.poll(&mut net, 20);
    assert!(zed.rx.is_empty());
    assert_eq!(to_child(&net, t), 0);
    let _ = sa;
}

#[test]
fn a_childs_frames_are_routed_and_route_requests_for_it_are_answered() {
    let (mut net, a, mut zed) = with_sleepy_child();
    let sa = net.nodes[a].short();
    assert_eq!(zed.zcl_to_coordinator(ON_OFF, &[0x18, 7, 0x0A, 0x00, 0x00, 0x10, 0x01]), TxStatus::Ok);
    net.run(100);
    assert_eq!(net.coord.last_zcl(zed.short, ON_OFF, 0x0A).unwrap(), vec![0x18, 7, 0x0A, 0x00, 0x00, 0x10, 0x01]);
    let z = zed.short.to_le_bytes();
    net.coord.nwk_command(0xFFFC, 30, &[nwk::CMD_ROUTE_REQUEST, 0x00, 0x77, z[0], z[1], 0]);
    net.run(200);
    let reply = net.coord.nwk_cmd_rx.iter().find(|(_, c)| c[0] == nwk::CMD_ROUTE_REPLY && c[2] == 0x77).expect("route reply");
    assert_eq!(reply.0, sa, "the reply comes from the parent");
    assert_eq!(&reply.1[5..7], &z, "and names the child as responder");
}

#[test]
fn broadcasts_to_all_devices_are_kept_for_sleeping_children() {
    let (mut net, _a, mut zed) = with_sleepy_child();
    net.coord.zcl(0xFFFF, ON_OFF, 0x11, 0x02, &[]);
    net.run(300);
    zed.poll(&mut net, 20);
    assert_eq!(zed.rx.len(), 1);
    assert_eq!(zed.rx[0].0.dst, 0xFFFF);
    zed.rx.clear();
    net.coord.zcl(0xFFFD, ON_OFF, 0x11, 0x02, &[]);
    net.run(300);
    zed.poll(&mut net, 20);
    assert!(zed.rx.is_empty());
}

#[test]
fn a_broadcast_for_sleeping_children_is_held_once_and_reaches_each_of_them_once() {
    let (mut net, a, zed) = with_sleepy_child();
    let mut kids = vec![zed];
    kids.extend(more_sleepy_children(&mut net, a, 5));
    let id = net.nodes[a].id;
    let t = net.now();
    net.coord.zcl(0xFFFF, ON_OFF, 0x11, 0x02, &[]);
    net.run(300);
    let shorts = net.air.borrow().ports[id].pending.0.clone();
    for k in &kids {
        assert!(shorts.contains(&k.short), "{:#06x} not among the addresses with frames waiting: {shorts:x?}", k.short);
    }
    for k in kids.iter_mut() {
        k.poll(&mut net, 20);
        k.poll(&mut net, 20);
        assert!(k.rx.iter().any(|(h, _)| (h.src, h.dst) == (0x0000, 0xFFFF)), "{:#06x} got the broadcast", k.short);
        assert_eq!(sent_to(&net, a, k.short, t).len(), 1, "{:#06x}: once, and nothing on the second poll", k.short);
    }
    let copies: Vec<Vec<u8>> = kids.iter().flat_map(|k| sent_to(&net, a, k.short, t)).collect();
    assert_eq!(copies.len(), kids.len());
    assert!(copies.iter().all(|f| f[9..] == copies[0][9..]), "the same NWK frame (one frame counter) for every child");
    let mut seqs: Vec<u8> = copies.iter().map(|f| f[2]).collect();
    seqs.dedup();
    assert_eq!(seqs.len(), kids.len(), "a MAC sequence number of its own for each copy");
    assert!(net.air.borrow().ports[id].pending.0.is_empty(), "nothing waits any more");
}

#[test]
fn a_broadcast_waits_for_a_sleeping_child_only_for_the_persistence_time() {
    let (mut net, a, mut zed) = with_sleepy_child();
    let late = more_sleepy_children(&mut net, a, 1);
    let mut late = late.into_iter().next().unwrap();
    net.coord.zcl(0xFFFF, ON_OFF, 0x11, 0x02, &[]);
    net.run(3_000);
    zed.poll(&mut net, 20);
    assert_eq!(zed.rx.len(), 1);
    net.run(6_000);
    late.poll(&mut net, 20);
    assert!(late.rx.is_empty(), "a broadcast 9 s old is not handed over (persistence time 7.68 s)");
    assert!(net.air.borrow().ports[net.nodes[a].id].pending.0.is_empty());
}

#[test]
fn one_child_holds_at_most_its_allowance_and_keeps_its_newest_frames() {
    let (mut net, a, mut zed) = with_sleepy_child();
    let mut other = more_sleepy_children(&mut net, a, 1).into_iter().next().unwrap();
    let dropped = net.nodes[a].stack.stats.indirect_dropped;
    let seqs: Vec<u8> = (0..INDIRECT_PER_CHILD + 2)
        .map(|_| {
            let s = net.coord.zcl(zed.short, ON_OFF, 0x11, 0x02, &[]);
            net.run(50);
            s
        })
        .collect();
    let theirs = net.coord.zcl(other.short, ON_OFF, 0x11, 0x02, &[]);
    net.run(50);
    for _ in 0..INDIRECT_PER_CHILD + 2 {
        zed.poll(&mut net, 20);
    }
    let got: Vec<u8> = zed.rx.iter().filter(|(h, _)| h.src == 0x0000).map(|(_, aps)| aps[9]).collect();
    assert_eq!(got, seqs[2..], "the newest frames, oldest first");
    other.poll(&mut net, 20);
    let got: Vec<u8> = other.rx.iter().filter(|(h, _)| h.src == 0x0000).map(|(_, aps)| aps[9]).collect();
    assert_eq!(got, vec![theirs], "another child's frame keeps its place");
    assert_eq!(net.nodes[a].stack.stats.indirect_dropped - dropped, 2);
}

#[test]
fn a_full_pool_takes_room_from_the_child_that_holds_most() {
    let (mut net, a, zed) = with_sleepy_child();
    let full = INDIRECT / INDIRECT_PER_CHILD - 1;
    let mut kids = vec![zed];
    kids.extend(more_sleepy_children(&mut net, a, full + 1));
    let rest = INDIRECT - full * INDIRECT_PER_CHILD;
    let mut want = vec![INDIRECT_PER_CHILD; full];
    want.extend([rest / 2, rest - rest / 2]);
    let mut seqs: Vec<Vec<u8>> = vec![vec![]; kids.len()];
    for (i, n) in want.iter().enumerate() {
        for _ in 0..*n {
            seqs[i].push(net.coord.zcl(kids[i].short, ON_OFF, 0x11, 0x02, &[]));
            net.run(20);
        }
    }
    let last = kids.len() - 1;
    seqs[last].push(net.coord.zcl(kids[last].short, ON_OFF, 0x11, 0x02, &[]));
    net.run(20);
    for (i, k) in kids.iter_mut().enumerate() {
        for _ in 0..INDIRECT_PER_CHILD {
            k.poll(&mut net, 20);
        }
        let got: Vec<u8> = k.rx.iter().filter(|(h, _)| h.src == 0x0000).map(|(_, aps)| aps[9]).collect();
        let expect = if i == 0 { seqs[0][1..].to_vec() } else { seqs[i].clone() };
        assert_eq!(got, expect, "child {i}");
    }
}

#[test]
fn a_full_pool_gives_up_first_the_frames_of_children_not_expected_in_time() {
    let (mut net, a, fast) = with_sleepy_child();
    let slow_n = INDIRECT / INDIRECT_PER_CHILD - 1;
    let mut kids = vec![fast];
    kids.extend(more_sleepy_children(&mut net, a, slow_n + 1));
    let (f, f2) = (0, slow_n + 1);
    for k in kids.iter_mut() {
        k.poll(&mut net, 20);
    }
    for _ in 0..20 {
        net.run(980);
        for i in [f, f2] {
            kids[i].poll(&mut net, 20);
        }
    }
    for k in kids[1..=slow_n].iter_mut() {
        k.poll(&mut net, 20);
    }
    let mut seqs: Vec<Vec<u8>> = vec![vec![]; kids.len()];
    for i in std::iter::once(f).chain(1..=slow_n) {
        for _ in 0..INDIRECT_PER_CHILD {
            seqs[i].push(net.coord.zcl(kids[i].short, ON_OFF, 0x11, 0x02, &[]));
            net.run(20);
        }
    }
    seqs[f2].push(net.coord.zcl(kids[f2].short, ON_OFF, 0x11, 0x02, &[]));
    net.run(20);
    for i in [f, f2] {
        for _ in 0..INDIRECT_PER_CHILD {
            kids[i].poll(&mut net, 20);
        }
        let got: Vec<u8> = kids[i].rx.iter().filter(|(h, _)| h.src == 0x0000).map(|(_, aps)| aps[9]).collect();
        assert_eq!(got, seqs[i], "child {i} polling every second has all its frames");
    }
}

#[test]
fn a_frame_its_child_would_not_be_there_for_does_not_take_the_place_of_one_it_would() {
    let (mut net, a, slow) = with_sleepy_child();
    let fast_n = INDIRECT / INDIRECT_PER_CHILD;
    let mut kids = vec![slow];
    kids.extend(more_sleepy_children(&mut net, a, fast_n));
    for k in kids.iter_mut() {
        k.poll(&mut net, 20);
    }
    for _ in 0..20 {
        net.run(900);
        for k in kids[1..].iter_mut() {
            k.poll(&mut net, 10);
        }
    }
    kids[0].poll(&mut net, 20);
    let mut seqs: Vec<Vec<u8>> = vec![vec![]; kids.len()];
    for i in 1..kids.len() {
        for _ in 0..INDIRECT_PER_CHILD {
            seqs[i].push(net.coord.zcl(kids[i].short, ON_OFF, 0x11, 0x02, &[]));
            net.run(20);
        }
    }
    let dropped = net.nodes[a].stack.stats.indirect_dropped;
    net.coord.zcl(kids[0].short, ON_OFF, 0x11, 0x02, &[]);
    net.run(20);
    assert_eq!(net.nodes[a].stack.stats.indirect_dropped - dropped, 1, "the frame for the slow child is not kept");
    for i in 1..kids.len() {
        for _ in 0..INDIRECT_PER_CHILD {
            kids[i].poll(&mut net, 20);
        }
        let got: Vec<u8> = kids[i].rx.iter().filter(|(h, _)| h.src == 0x0000).map(|(_, aps)| aps[9]).collect();
        assert_eq!(got, seqs[i], "child {i}");
    }
}

#[test]
fn the_child_table_shows_in_the_neighbor_list_and_survives_a_power_cycle() {
    let (mut net, a, mut zed) = with_sleepy_child();
    let sa = net.nodes[a].short();
    net.power_cycle(a);
    assert_eq!(net.nodes[a].stack.child_count(), 1);
    net.run(2_000);
    assert_eq!(zed.zcl_to_coordinator(ON_OFF, &[0x18, 9, 0x0A, 0x00, 0x00, 0x10, 0x00]), TxStatus::Ok);
    net.run(3_000);
    assert!(net.coord.last_zcl(zed.short, ON_OFF, 0x0A).is_some());
    let seq = net.coord.zdo(sa, 0x0031, &[0]);
    net.run(200);
    let lqi = net.coord.last_zdo(sa, 0x8031).unwrap();
    assert_eq!(&lqi[..2], &[seq, 0x00]);
    let entries: Vec<&[u8]> = lqi[5..].chunks(22).collect();
    let child = entries.iter().find(|e| e[8..16] == zed.ext.to_le_bytes()).expect("the child is listed");
    assert_eq!(&child[16..18], &zed.short.to_le_bytes());
    assert_eq!(child[18], 0x02 | 0x00 << 2 | 0x01 << 4);
}

fn left_reports(net: &Net) -> Vec<(u16, u64, u8)> {
    net.coord.updates.iter().filter(|u| u.2 == 0x02).cloned().collect()
}

#[test]
fn a_child_that_stops_polling_is_forgotten_and_told_to_rejoin_when_it_polls_again() {
    let (mut net, a, mut zed) = with_sleepy_child();
    let h = zed.header(nwk::FRAME_CMD, net.nodes[a].short());
    zed.nwk_send(h, &[nwk::CMD_TIMEOUT_REQUEST, 0x00, 0x00]);
    net.run(100);
    zed.poll(&mut net, 20);
    let (_, rsp) = zed.rx.last().expect("timeout response");
    assert_eq!(rsp, &vec![nwk::CMD_TIMEOUT_RESPONSE, 0x00, 0x01]);
    for _ in 0..5 {
        net.run(4_000);
        zed.poll(&mut net, 20);
    }
    assert_eq!(net.nodes[a].stack.child_count(), 1);
    let told = |net: &Net| net.air.borrow().ports[net.nodes[a].id].poll_children.clone();
    assert_eq!(told(&net), Some(vec![zed.short]));
    net.run(20_000);
    assert_eq!(net.nodes[a].stack.child_count(), 0);
    assert_eq!(told(&net), Some(vec![]));
    assert!(left_reports(&net).is_empty());
    assert!(!net.coord.devices.iter().any(|d| d.ext == zed.ext && d.left));
    zed.rx.clear();
    zed.poll(&mut net, 20);
    assert_eq!(zed.rx.len(), 1);
    let (h, p) = &zed.rx[0];
    assert_eq!(p, &vec![nwk::CMD_LEAVE, 0x60], "Leave: request, rejoin");
    assert_eq!((h.src, h.dst, h.radius), (net.nodes[a].short(), zed.short, 1));
    assert_eq!((h.dst_ext, h.security), (None, true), "no destination IEEE address");
    assert_eq!(net.nodes[a].stack.child_count(), 0);
}

fn forget_child(net: &mut Net, a: usize, zed: &mut Zed) {
    let h = zed.header(nwk::FRAME_CMD, net.nodes[a].short());
    zed.nwk_send(h, &[nwk::CMD_TIMEOUT_REQUEST, 0x00, 0x00]);
    net.run(20_000);
    assert_eq!(net.nodes[a].stack.child_count(), 0);
}

fn sent_to(net: &Net, a: usize, short: u16, since: u32) -> Vec<Vec<u8>> {
    net.sent_by(net.nodes[a].id, since).into_iter().filter(|f| f[0] & 7 == 1 && f[5..7] == short.to_le_bytes()).collect()
}

#[test]
fn a_leave_for_an_unknown_end_device_waits_for_a_poll_that_takes_it() {
    let (mut net, a, mut zed) = with_sleepy_child();
    let sa = net.nodes[a].short();
    forget_child(&mut net, a, &mut zed);
    let t = net.now();
    net.air.borrow_mut().ports[zed.id].short = 0xFFFE;
    zed.poll(&mut net, 20);
    assert_eq!(sent_to(&net, a, zed.short, t).len(), 1, "sent, not acknowledged");
    net.air.borrow_mut().ports[zed.id].short = zed.short;
    zed.rx.clear();
    zed.poll(&mut net, 20);
    let sent = sent_to(&net, a, zed.short, t);
    assert_eq!(sent.len(), 2);
    assert!(sent.iter().all(|f| f[0] & 0x10 == 0), "nothing more is waiting behind it");
    assert_eq!(zed.rx.iter().map(|(_, p)| p.clone()).collect::<Vec<_>>(), vec![vec![nwk::CMD_LEAVE, 0x60]]);
    let mut ghost = Zed::new(&net.air, true);
    ghost.parent = sa;
    ghost.short = 0x0000;
    net.coord.nwk_cmd_rx.clear();
    ghost.poll(&mut net, 50);
    assert!(!net.coord.nwk_cmd_rx.iter().any(|(s, p)| *s == sa && p[0] == nwk::CMD_LEAVE));
    let t = net.now();
    let mut stranger = Zed::new(&net.air, true);
    stranger.parent = sa;
    stranger.poll(&mut net, 50);
    stranger.short = 0xFFFC;
    stranger.poll(&mut net, 50);
    assert!(net.sent_by(net.nodes[a].id, t).iter().all(|f| f[0] & 7 != 1));
}

#[test]
fn a_leave_for_a_stranger_never_takes_the_place_of_a_childs_frame() {
    let (mut net, a, zed) = with_sleepy_child();
    let sa = net.nodes[a].short();
    let mut kids = vec![zed];
    kids.extend(more_sleepy_children(&mut net, a, INDIRECT / INDIRECT_PER_CHILD - 1));
    for k in &kids {
        for _ in 0..INDIRECT_PER_CHILD {
            net.coord.zcl(k.short, ON_OFF, 0x11, 0x02, &[]);
            net.run(50);
        }
    }
    let t = net.now();
    let mut stranger = Zed::new(&net.air, true);
    stranger.parent = sa;
    stranger.short = 0x4455;
    stranger.poll(&mut net, 20);
    assert!(sent_to(&net, a, 0x4455, t).is_empty());
    for k in kids.iter_mut() {
        for _ in 0..INDIRECT_PER_CHILD {
            k.poll(&mut net, 20);
        }
        assert_eq!(k.rx.iter().filter(|(h, _)| h.src == 0x0000 && h.dst == k.short).count(), INDIRECT_PER_CHILD);
    }
    let t = net.now();
    stranger.poll(&mut net, 20);
    assert_eq!(sent_to(&net, a, 0x4455, t).len(), 1, "sent; nobody has that address, so it stays queued");
    let mut another = Zed::new(&net.air, true);
    another.parent = sa;
    another.short = 0x4466;
    another.poll(&mut net, 20);
    assert!(sent_to(&net, a, 0x4466, t).is_empty());
    net.run(8_000);
    let t = net.now();
    another.poll(&mut net, 20);
    assert_eq!(sent_to(&net, a, 0x4466, t).len(), 1);
}

#[test]
fn a_device_that_rejoins_is_not_handed_the_leave_that_waited_for_it() {
    let (mut net, a, mut zed) = with_sleepy_child();
    let sa = net.nodes[a].short();
    forget_child(&mut net, a, &mut zed);
    net.air.borrow_mut().ports[zed.id].short = 0xFFFE;
    zed.poll(&mut net, 20);
    net.air.borrow_mut().ports[zed.id].short = zed.short;
    let mut h = zed.header(nwk::FRAME_CMD, sa);
    h.radius = 1;
    let cap = zed.capability();
    assert_eq!(zed.nwk_send(h, &[nwk::CMD_REJOIN_REQUEST, cap]), TxStatus::Ok);
    net.run(100);
    zed.rx.clear();
    zed.poll(&mut net, 20);
    zed.poll(&mut net, 20);
    let z = zed.short.to_le_bytes();
    assert_eq!(zed.rx.iter().map(|(_, p)| p.clone()).collect::<Vec<_>>(), vec![vec![nwk::CMD_REJOIN_RESPONSE, z[0], z[1], 0x00]]);
    assert_eq!(net.nodes[a].stack.child_count(), 1);
}

#[test]
fn an_end_device_the_bulb_does_not_know_is_not_served_and_told_to_rejoin() {
    let (mut net, a, mut zed) = with_sleepy_child();
    let sa = net.nodes[a].short();
    zed.end_device_initiator = true;
    let report = [0x18, 7, 0x0A, 0x00, 0x00, 0x10, 0x01];
    assert_eq!(zed.zcl_to_coordinator(ON_OFF, &report), TxStatus::Ok);
    net.run(100);
    assert!(net.coord.last_zcl(zed.short, ON_OFF, 0x0A).is_some(), "the frames of a child are passed on");
    let relayed = sent_to(&net, a, 0x0000, 0).pop().unwrap();
    assert_eq!((relayed[9] & 3, &relayed[13..15], relayed[10] & 0x20), (0, &zed.short.to_le_bytes()[..], 0), "without the mark, which is for the parent only");
    forget_child(&mut net, a, &mut zed);
    net.coord.zcl_rx.clear();
    let t = net.now();
    assert_eq!(zed.zcl_to_coordinator(ON_OFF, &report), TxStatus::Ok);
    net.run(100);
    assert!(net.coord.zcl_rx.is_empty());
    assert_eq!(sent_to(&net, a, zed.short, t).len(), 1);
    zed.rx.clear();
    zed.poll(&mut net, 20);
    assert_eq!(zed.rx.iter().map(|(_, p)| p.clone()).collect::<Vec<_>>(), vec![vec![nwk::CMD_LEAVE, 0x60]]);
    zed.end_device_initiator = false;
    assert_eq!(zed.zcl_to_coordinator(ON_OFF, &report), TxStatus::Ok);
    net.run(100);
    assert_eq!(net.coord.zcl_rx.len(), 1);
    zed.end_device_initiator = true;
    let mut h = zed.header(nwk::FRAME_CMD, sa);
    h.radius = 1;
    let cap = zed.capability();
    assert_eq!(zed.nwk_send(h, &[nwk::CMD_REJOIN_REQUEST, cap]), TxStatus::Ok);
    net.run(100);
    zed.rx.clear();
    zed.poll(&mut net, 20);
    assert!(zed.rx.iter().any(|(_, p)| p[0] == nwk::CMD_REJOIN_RESPONSE && p[3] == 0));
    assert_eq!(net.nodes[a].stack.child_count(), 1);

    let mut other = Zed::new(&net.air, false);
    assert!(other.join(&mut net, 0x0000));
    other.parent = sa;
    other.end_device_initiator = true;
    other.rx.clear();
    net.coord.zcl_rx.clear();
    assert_eq!(other.zcl_to_coordinator(ON_OFF, &report), TxStatus::Ok);
    net.run(100);
    other.drain();
    assert!(net.coord.zcl_rx.is_empty());
    let (h, p) = other.rx.last().expect("a Leave without a poll");
    assert_eq!(p, &vec![nwk::CMD_LEAVE, 0x60]);
    assert_eq!((h.src, h.dst, h.radius, h.dst_ext), (sa, other.short, 1, None));
}

#[test]
fn a_child_that_leaves_is_reported_only_if_it_does_not_mean_to_come_back() {
    let (mut net, a, mut zed) = with_sleepy_child();
    let sa = net.nodes[a].short();
    let mut h = zed.header(nwk::FRAME_CMD, 0xFFFD);
    h.radius = 1;
    zed.nwk_send(h, &[nwk::CMD_LEAVE, 0x20]);
    net.run(500);
    assert_eq!(net.nodes[a].stack.child_count(), 0);
    assert!(left_reports(&net).is_empty());
    let mut h = zed.header(nwk::FRAME_CMD, sa);
    h.radius = 1;
    let cap = zed.capability();
    zed.nwk_send(h, &[nwk::CMD_REJOIN_REQUEST, cap]);
    net.run(100);
    zed.poll(&mut net, 20);
    assert_eq!(net.nodes[a].stack.child_count(), 1);
    let mut h = zed.header(nwk::FRAME_CMD, 0xFFFD);
    h.radius = 1;
    zed.nwk_send(h, &[nwk::CMD_LEAVE, 0x00]);
    net.run(500);
    assert_eq!(net.nodes[a].stack.child_count(), 0);
    assert_eq!(left_reports(&net), vec![(sa, zed.ext, 0x02)]);
}

#[test]
fn a_device_with_the_key_rejoins_without_permit_joining() {
    let (mut net, a, mut zed) = with_sleepy_child();
    let sa = net.nodes[a].short();
    let h = zed.header(nwk::FRAME_CMD, sa);
    zed.nwk_send(h, &[nwk::CMD_TIMEOUT_REQUEST, 0x00, 0x00]);
    net.coord.permit(0);
    net.run(20_000);
    assert_eq!(net.nodes[a].stack.child_count(), 0);
    let mut h = zed.header(nwk::FRAME_CMD, sa);
    h.radius = 1;
    let cap = zed.capability();
    assert_eq!(zed.nwk_send(h, &[nwk::CMD_REJOIN_REQUEST, cap]), TxStatus::Ok);
    net.run(100);
    zed.rx.clear();
    zed.poll(&mut net, 20);
    let (rh, rsp) = zed.rx.last().expect("rejoin response on the poll");
    assert_eq!(rh.dst_ext, Some(zed.ext));
    let z = zed.short.to_le_bytes();
    assert_eq!(rsp, &vec![nwk::CMD_REJOIN_RESPONSE, z[0], z[1], 0x00]);
    assert_eq!(net.nodes[a].stack.child_count(), 1);
}

#[test]
fn a_child_that_listens_when_idle_gets_its_frames_at_once() {
    let (mut net, a, _) = with_sleepy_child();
    let mut zed = Zed::new(&net.air, false);
    let pz = zed.id;
    net.link(0, pz, 0);
    let sa = net.nodes[a].short();
    assert!(zed.join(&mut net, sa));
    net.run(200);
    assert_eq!(net.nodes[a].stack.child_count(), 2);
    net.coord.zcl(zed.short, ON_OFF, 0x11, 0x02, &[]);
    net.run(100);
    zed.drain();
    assert_eq!(zed.rx.iter().filter(|(h, _)| h.dst == zed.short && h.src == 0).count(), 1, "no poll needed");
    let seq = net.coord.zcl(sa, ON_OFF, 0x01, 0x00, &[]);
    net.run(100);
    assert_eq!(net.nodes[a].commands.last(), Some(&Command::Off));
    assert_eq!(net.coord.last_zcl(sa, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x00, 0x00]);
}

#[test]
fn a_child_that_lost_the_network_key_gets_it_back_through_its_parent() {
    let (mut net, a, mut zed) = with_sleepy_child();
    let sa = net.nodes[a].short();
    net.coord.permit(0);
    net.run(500);
    net.coord.updates.clear();
    assert!(zed.rejoin_without_key(&mut net, sa), "readmitted and handed the key, with the network closed");
    assert_eq!(zed.key, Some(NWK_KEY));
    assert_eq!(net.coord.updates, vec![(sa, zed.ext, 0x03)], "the parent reported a Trust Center rejoin");
    assert_eq!(net.nodes[a].stack.child_count(), 1);
    net.coord.zcl(zed.short, ON_OFF, 0x11, 0x02, &[]);
    net.run(300);
    zed.poll(&mut net, 20);
    assert!(zed.rx.iter().any(|(h, _)| h.dst == zed.short && h.src == 0));
}

#[test]
fn a_secured_rejoin_of_a_child_is_reported_to_the_trust_center() {
    let (mut net, a, mut zed) = with_sleepy_child();
    let sa = net.nodes[a].short();
    net.coord.updates.clear();
    let mut h = zed.header(nwk::FRAME_CMD, sa);
    h.radius = 1;
    let cap = zed.capability();
    zed.nwk_send(h, &[nwk::CMD_REJOIN_REQUEST, cap]);
    net.run(200);
    assert_eq!(net.coord.updates, vec![(sa, zed.ext, 0x00)]);
}

#[test]
fn a_child_whose_address_another_device_claims_is_given_a_new_one() {
    let (mut net, a, mut zed) = with_sleepy_child();
    let old = zed.short;
    let o = old.to_le_bytes();
    net.coord.nwk_command(0xFFFD, 30, &[0x03, 0x0D, o[0], o[1]]);
    net.run(500);
    zed.rx.clear();
    zed.poll(&mut net, 50);
    let (h, p) = zed.rx.iter().find(|(_, p)| p.first() == Some(&nwk::CMD_REJOIN_RESPONSE)).expect("an unsolicited Rejoin Response");
    assert_eq!(h.dst_ext, Some(zed.ext));
    let new = u16::from_le_bytes([p[1], p[2]]);
    assert_eq!(p[3], 0x00);
    assert_ne!(new, old);
    assert!(new != 0 && new < 0xFFF8 && new != net.nodes[a].short());
    zed.short = new;
    net.air.borrow_mut().ports[zed.id].short = new;
    assert_eq!(zed.zcl_to_coordinator(ON_OFF, &[0x18, 9, 0x0A, 0x00, 0x00, 0x10, 0x01]), TxStatus::Ok);
    net.run(3_000);
    assert!(net.coord.last_zcl(new, ON_OFF, 0x0A).is_some());
    assert_eq!(net.nodes[a].stack.child_count(), 1);
    let sa = net.nodes[a].short();
    net.coord.zdo(sa, 0x0031, &[0]);
    net.run(200);
    let lqi = net.coord.last_zdo(sa, 0x8031).unwrap();
    let entries: Vec<&[u8]> = lqi[5..].chunks(22).collect();
    let child = entries.iter().find(|e| e[8..16] == zed.ext.to_le_bytes()).expect("the child is listed");
    assert_eq!(&child[16..18], &new.to_le_bytes());
}

#[test]
fn an_orphaned_child_is_told_where_its_network_is() {
    let (mut net, a, mut zed) = with_sleepy_child();
    let sa = net.nodes[a].short();
    assert_eq!(zed.orphan_scan(&mut net), Some((PAN, sa, 11, zed.short)));
    let mut stranger = Zed::new(&net.air, true);
    assert_eq!(stranger.orphan_scan(&mut net), None);
}

#[test]
fn after_a_restart_children_that_moved_are_given_up() {
    let (mut net, a, mut zed) = with_sleepy_child();
    net.coord.permit(120);
    let b = net.join_node();
    let (sa, sb) = (net.nodes[a].short(), net.nodes[b].short());
    net.run(1_000);
    net.set_power(a, false);
    zed.parent = sb;
    let mut h = zed.header(nwk::FRAME_CMD, sb);
    h.radius = 1;
    let cap = zed.capability();
    assert_eq!(zed.nwk_send(h, &[nwk::CMD_REJOIN_REQUEST, cap]), TxStatus::Ok);
    net.run(100);
    zed.poll(&mut net, 20);
    net.run(1_000);
    assert_eq!(net.nodes[b].stack.child_count(), 1);
    net.power_cycle(a);
    net.set_power(a, true);
    assert_eq!(net.nodes[a].stack.child_count(), 1);
    zed.poll(&mut net, 50);
    let before = net.now();
    net.run(21_000);
    let annce: Vec<Vec<u8>> = net.sent_by(net.nodes[a].id, before);
    assert!(!annce.is_empty());
    assert_eq!(net.nodes[a].stack.child_count(), 0, "the first bulb gave the child up");
    assert_eq!(net.nodes[b].stack.child_count(), 1);
    let _ = sa;
}

#[test]
fn after_a_restart_more_children_than_fit_are_announced_in_several_parent_annces() {
    let (mut net, a, zed) = with_sleepy_child();
    let mut kids = vec![zed];
    kids.extend(more_sleepy_children(&mut net, a, 11));
    let sa = net.nodes[a].short();
    net.power_cycle(a);
    let annces = |net: &Net| -> Vec<Vec<u8>> {
        net.coord.zdo_rx.iter().filter(|(s, c, _)| *s == sa && *c == 0x001F).map(|e| e.2.clone()).collect()
    };
    let listed = |p: &[u8]| -> Vec<u64> { p[2..].chunks(8).map(|c| u64::from_le_bytes(c.try_into().unwrap())).collect() };
    net.run(21_000);
    let first = annces(&net);
    assert_eq!(first.len(), 1);
    assert_eq!((first[0][1], listed(&first[0]).len()), (9, 9), "as many as fit in one message without fragmentation");
    let rest: Vec<usize> = (0..kids.len()).filter(|i| !listed(&first[0]).contains(&kids[*i].ext)).collect();
    assert_eq!(rest.len(), 3);
    kids[rest[0]].poll(&mut net, 20);
    net.run(21_000);
    let all = annces(&net);
    assert_eq!(all.len(), 2, "the rest after another wait");
    assert_eq!(listed(&all[1]), vec![kids[rest[1]].ext, kids[rest[2]].ext]);
    assert_eq!(all[1][1], 2);
    net.run(21_000);
    assert_eq!(annces(&net).len(), 2, "and that is all");
    assert_eq!(net.nodes[a].stack.child_count(), 12, "nobody claimed them");
}

#[test]
fn more_children_claimed_than_fit_in_one_parent_annce_rsp_are_claimed_in_several() {
    let (mut net, a, zed) = with_sleepy_child();
    let mut kids = vec![zed];
    kids.extend(more_sleepy_children(&mut net, a, 9));
    let sa = net.nodes[a].short();
    let mut annce = vec![kids.len() as u8];
    for k in &kids {
        annce.extend_from_slice(&k.ext.to_le_bytes());
    }
    net.coord.zdo(0xFFFC, 0x001F, &annce);
    net.run(1_000);
    let rsps: Vec<Vec<u8>> = net.coord.zdo_rx.iter().filter(|(s, c, _)| *s == sa && *c == 0x801F).map(|e| e.2.clone()).collect();
    assert_eq!(rsps.iter().map(|r| (r[1], r[2])).collect::<Vec<_>>(), vec![(0x00, 9), (0x00, 1)]);
    let mut claimed: Vec<u64> = rsps.iter().flat_map(|r| r[3..].chunks(8).map(|c| u64::from_le_bytes(c.try_into().unwrap()))).collect();
    claimed.sort();
    let mut all: Vec<u64> = kids.iter().map(|k| k.ext).collect();
    all.sort();
    assert_eq!(claimed, all);
}

#[test]
fn after_a_power_cut_for_everyone_no_router_gives_up_a_child_it_has_not_heard_yet() {
    let (mut net, a, zed) = with_sleepy_child();
    net.power_cycle(a);
    net.run(1_000);
    let mut annce = vec![1];
    annce.extend_from_slice(&zed.ext.to_le_bytes());
    net.coord.zdo(0xFFFC, 0x001F, &annce);
    net.run(1_000);
    assert_eq!(net.nodes[a].stack.child_count(), 1);
    assert!(net.coord.last_zdo(net.nodes[a].short(), 0x801F).is_none(), "nothing to claim either");
}

fn route_repliers(net: &Net, id: u8) -> Vec<u16> {
    net.coord.nwk_cmd_rx.iter().filter(|(_, c)| c[0] == nwk::CMD_ROUTE_REPLY && c[2] == id).map(|(s, _)| *s).collect()
}

fn move_to(net: &mut Net, zed: &mut Zed, parent: u16) {
    zed.parent = parent;
    let mut h = zed.header(nwk::FRAME_CMD, parent);
    h.radius = 1;
    let cap = zed.capability();
    assert_eq!(zed.nwk_send(h, &[nwk::CMD_REJOIN_REQUEST, cap]), TxStatus::Ok);
    net.run(100);
    zed.rx.clear();
    zed.poll(net, 20);
    assert!(zed.rx.iter().any(|(_, p)| p[0] == nwk::CMD_REJOIN_RESPONSE && p[3] == 0));
}

#[test]
fn a_child_that_announces_itself_through_another_router_is_given_up() {
    let (mut net, a, mut zed) = with_sleepy_child();
    net.coord.permit(120);
    let b = net.join_node();
    let (sa, sb) = (net.nodes[a].short(), net.nodes[b].short());
    net.run(1_000);
    move_to(&mut net, &mut zed, sb);
    assert_eq!((net.nodes[a].stack.child_count(), net.nodes[b].stack.child_count()), (1, 1), "the first bulb has heard nothing of it yet");
    zed.announce();
    net.run(1_000);
    assert_eq!((net.nodes[a].stack.child_count(), net.nodes[b].stack.child_count()), (0, 1));
    assert!(left_reports(&net).is_empty(), "and that is no leaving");
    let z = zed.short.to_le_bytes();
    net.coord.nwk_command(0xFFFC, 30, &[nwk::CMD_ROUTE_REQUEST, 0x00, 0x55, z[0], z[1], 0]);
    net.run(500);
    assert_eq!(route_repliers(&net, 0x55), vec![sb]);
    let _ = sa;
}

#[test]
fn a_child_whose_frames_come_through_another_router_is_given_up() {
    let (mut net, a, mut zed) = with_sleepy_child();
    net.coord.permit(120);
    let b = net.join_node();
    let (pb, sb) = (net.nodes[b].id, net.nodes[b].short());
    net.link(0, pb, 0);
    net.run(70_000);
    move_to(&mut net, &mut zed, sb);
    net.run(3_000);
    assert_eq!(net.nodes[a].stack.child_count(), 1);
    assert_eq!(zed.zcl_to_coordinator(ON_OFF, &[0x18, 7, 0x0A, 0x00, 0x00, 0x10, 0x01]), TxStatus::Ok);
    net.run(3_000);
    assert!(net.coord.last_zcl(zed.short, ON_OFF, 0x0A).is_some(), "passed on");
    assert_eq!((net.nodes[a].stack.child_count(), net.nodes[b].stack.child_count()), (0, 1));
    assert!(left_reports(&net).is_empty());
}

#[test]
fn the_echo_of_a_childs_broadcast_does_not_make_it_a_stranger() {
    let (mut net, a, mut zed) = with_sleepy_child();
    net.coord.permit(120);
    let b = net.join_node();
    net.run(1_000);
    let t = net.now();
    zed.announce();
    net.run(2_000);
    assert!(net.sent_by(net.nodes[b].id, t).iter().any(|f| f[0] & 7 == 1 && f[5..7] == [0xFF, 0xFF] && f[13..15] == zed.short.to_le_bytes()));
    assert_eq!(net.nodes[a].stack.child_count(), 1);
}

#[test]
fn a_child_announced_under_another_address_is_given_up() {
    let (mut net, a, zed) = with_sleepy_child();
    let mut annce = vec![0x34, 0x12];
    annce.extend_from_slice(&zed.ext.to_le_bytes());
    annce.push(zed.capability());
    net.coord.zdo(0xFFFD, 0x0013, &annce);
    net.run(500);
    assert_eq!(net.nodes[a].stack.child_count(), 0);
    assert!(left_reports(&net).is_empty());
}

#[test]
fn address_requests_are_answered_for_end_device_children() {
    let (mut net, a, zed) = with_sleepy_child();
    let sa = net.nodes[a].short();
    let ea = net.air.borrow().ports[net.nodes[a].id].ext;
    let pair = |ext: u64, short: u16| [&ext.to_le_bytes()[..], &short.to_le_bytes()[..]].concat();
    let rsp = |status: u8, seq: u8, rest: &[u8]| [&[seq, status][..], rest].concat();
    let seq = net.coord.zdo(0xFFFD, 0x0000, &[&zed.ext.to_le_bytes()[..], &[0, 0]].concat());
    net.run(300);
    assert_eq!(net.coord.last_zdo(sa, 0x8000).unwrap(), rsp(0x00, seq, &pair(zed.ext, zed.short)));
    let seq = net.coord.zdo(sa, 0x0001, &[&zed.short.to_le_bytes()[..], &[0, 0]].concat());
    net.run(300);
    assert_eq!(net.coord.last_zdo(sa, 0x8001).unwrap(), rsp(0x00, seq, &pair(zed.ext, zed.short)));
    let seq = net.coord.zdo(sa, 0x0000, &[&ea.to_le_bytes()[..], &[0, 0]].concat());
    net.run(300);
    assert_eq!(net.coord.last_zdo(sa, 0x8000).unwrap(), rsp(0x00, seq, &pair(ea, sa)));
    let seq = net.coord.zdo(sa, 0x0001, &[&sa.to_le_bytes()[..], &[0, 0]].concat());
    net.run(300);
    assert_eq!(net.coord.last_zdo(sa, 0x8001).unwrap(), rsp(0x00, seq, &pair(ea, sa)));
    let z = zed.short.to_le_bytes();
    let seq = net.coord.zdo(sa, 0x0000, &[&zed.ext.to_le_bytes()[..], &[1, 0]].concat());
    net.run(300);
    assert_eq!(net.coord.last_zdo(sa, 0x8000).unwrap(), rsp(0x00, seq, &[&pair(ea, sa)[..], &[1, 0, z[0], z[1]]].concat()));
    let seq = net.coord.zdo(sa, 0x0001, &[&sa.to_le_bytes()[..], &[1, 1]].concat());
    net.run(300);
    assert_eq!(net.coord.last_zdo(sa, 0x8001).unwrap(), rsp(0x00, seq, &[&pair(ea, sa)[..], &[0, 1]].concat()));
    let seq = net.coord.zdo(sa, 0x0000, &[&zed.ext.to_le_bytes()[..], &[2, 0]].concat());
    net.run(300);
    assert_eq!(net.coord.last_zdo(sa, 0x8000).unwrap(), rsp(0x80, seq, &pair(zed.ext, zed.short)));
    let seq = net.coord.zdo(sa, 0x0001, &[&zed.short.to_le_bytes()[..], &[2, 0]].concat());
    net.run(300);
    assert_eq!(net.coord.last_zdo(sa, 0x8001).unwrap(), rsp(0x80, seq, &pair(ea, sa)));
    let seq = net.coord.zdo(sa, 0x0000, &[&0x1122_3344_5566_7788u64.to_le_bytes()[..], &[0, 0]].concat());
    net.run(300);
    assert_eq!(net.coord.last_zdo(sa, 0x8000).unwrap(), rsp(0x81, seq, &pair(0x1122_3344_5566_7788, 0xFFFF)));
    let seq = net.coord.zdo(sa, 0x0001, &[0x34, 0x12, 0, 0]);
    net.run(300);
    assert_eq!(net.coord.last_zdo(sa, 0x8001).unwrap(), rsp(0x81, seq, &pair(u64::MAX, 0x1234)));
    net.coord.zdo_rx.clear();
    net.coord.zdo(0xFFFD, 0x0000, &[&0x1122_3344_5566_7788u64.to_le_bytes()[..], &[0, 0]].concat());
    net.coord.zdo(0xFFFD, 0x0001, &[0x34, 0x12, 0, 0]);
    net.coord.zdo(0xFFFD, 0x0000, &[&zed.ext.to_le_bytes()[..], &[2, 0]].concat());
    net.run(500);
    assert!(net.coord.zdo_rx.iter().all(|(_, cluster, _)| *cluster != 0x8000 && *cluster != 0x8001));
}

#[test]
fn the_extended_address_response_of_a_bulb_without_children_has_no_list() {
    let mut net = Net::new();
    let a = net.join_node();
    let sa = net.nodes[a].short();
    let ea = net.air.borrow().ports[net.nodes[a].id].ext;
    let seq = net.coord.zdo(sa, 0x0001, &[&sa.to_le_bytes()[..], &[1, 0]].concat());
    net.run(300);
    let want = [&[seq, 0x00][..], &ea.to_le_bytes()[..], &sa.to_le_bytes()[..], &[0]].concat();
    assert_eq!(net.coord.last_zdo(sa, 0x8001).unwrap(), want);
}

#[test]
fn a_childs_frames_to_a_concentrator_are_preceded_by_a_route_record() {
    let (mut net, a, mut zed) = with_sleepy_child();
    let (pa, sa) = (net.nodes[a].id, net.nodes[a].short());
    let report = [0x18, 7, 0x0A, 0x00, 0x00, 0x10, 0x01];
    let to_coordinator = |net: &Net, since: u32| -> Vec<nwk::Header> {
        net.sent_by(pa, since).iter().filter(|f| f[0] & 7 == 1 && f[5..7] == [0, 0]).map(|f| nwk::Header::parse(&f[9..]).unwrap().0).collect()
    };
    for with_route in [false, true] {
        if with_route {
            let z = zed.short.to_le_bytes();
            net.coord.nwk_command(0xFFFC, 30, &[nwk::CMD_ROUTE_REQUEST, 0x00, 0x77, z[0], z[1], 0]);
            net.run(200);
            assert_eq!(route_repliers(&net, 0x77), vec![sa]);
        }
        let t = net.now();
        zed.zcl_to_coordinator(ON_OFF, &report);
        net.run(200);
        assert!(net.coord.route_records.is_empty());
        assert_eq!(to_coordinator(&net, t).iter().filter(|h| h.src == zed.short).count(), 1);
    }
    net.coord.many_to_one_route_request(false);
    net.run(2_000);
    net.coord.route_records.clear();
    let t = net.now();
    zed.zcl_to_coordinator(ON_OFF, &report);
    net.run(200);
    assert_eq!(net.coord.route_records, vec![(zed.short, vec![sa])], "in the child's name, the parent as first relay");
    let sent: Vec<nwk::Header> = to_coordinator(&net, t).into_iter().filter(|h| h.src == zed.short).collect();
    assert_eq!(sent.len(), 2);
    assert_eq!((sent[0].frame_type, sent[0].dst, sent[0].src_ext, sent[0].relays), (nwk::FRAME_CMD, 0x0000, Some(zed.ext), None));
    assert_eq!(sent[1].frame_type, nwk::FRAME_DATA, "the record goes first");
    net.coord.source_route.insert(zed.short, vec![sa]);
    net.coord.zcl(zed.short, ON_OFF, 0x11, 0x02, &[]);
    net.run(300);
    zed.rx.clear();
    zed.poll(&mut net, 20);
    assert!(zed.rx.iter().any(|(h, _)| h.src == 0x0000 && h.dst == zed.short && h.frame_type == nwk::FRAME_DATA));
}

#[test]
fn a_childs_counter_starts_anew_under_a_key_whose_number_was_used_before() {
    let (mut net, a, mut zed) = with_sleepy_child();
    let sa = net.nodes[a].short();
    let ea = net.air.borrow().ports[net.nodes[a].id].ext;
    let report = [0x18, 7, 0x0A, 0x00, 0x00, 0x10, 0x01];
    zed.zcl_to_coordinator(ON_OFF, &report);
    net.run(100);
    let third = [0x6B; 16];
    for (key, seq) in [([0x5A; 16], 1), (third, 0)] {
        net.coord.transport_network_key(sa, ea, &key, seq, true);
        net.run(100);
        net.coord.switch_key(seq);
        net.coord.use_key(&key, seq, true);
        net.run(100);
    }
    zed.key = Some(third);
    zed.fc = 0;
    net.coord.zcl_rx.clear();
    zed.zcl_to_coordinator(ON_OFF, &report);
    net.run(100);
    assert_eq!(net.coord.zcl_rx.len(), 1);
    assert_eq!(net.nodes[a].stack.child_count(), 1);
}

#[test]
fn a_child_not_heard_since_a_power_cycle_is_still_read_under_the_previous_key() {
    let (mut net, a, mut zed) = with_sleepy_child();
    let sa = net.nodes[a].short();
    let ea = net.air.borrow().ports[net.nodes[a].id].ext;
    let report = [0x18, 7, 0x0A, 0x00, 0x00, 0x10, 0x01];
    let second = [0x5A; 16];
    for (key, seq) in [(second, 1), ([0x6B; 16], 0)] {
        net.coord.transport_network_key(sa, ea, &key, seq, true);
        net.run(100);
        net.coord.switch_key(seq);
        net.coord.use_key(&key, seq, true);
        net.run(100);
    }
    zed.key = Some(second);
    zed.key_seq = 1;
    net.power_cycle(a);
    net.run(1_000);
    net.coord.zcl_rx.clear();
    zed.zcl_to_coordinator(ON_OFF, &report);
    net.run(100);
    assert_eq!(net.coord.zcl_rx.len(), 1);
}

#[test]
fn update_device_goes_out_in_both_forms_while_the_link_key_is_the_global_one() {
    let (net, a, zed) = with_sleepy_child();
    let sa = net.nodes[a].short();
    assert_eq!(net.coord.update_copies, vec![(sa, zed.ext, 0x01, true)]);
    let mut net = Net::new();
    net.coord.stack_revision = 20;
    net.coord.legacy_update_device = true;
    let a = net.join_node();
    let sa = net.nodes[a].short();
    net.coord.permit(120);
    net.run(600);
    let mut zed = Zed::new(&net.air, true);
    net.link(0, zed.id, 0);
    assert!(zed.join(&mut net, sa));
    assert_eq!(net.coord.update_copies, vec![(sa, zed.ext, 0x01, true), (sa, zed.ext, 0x01, false)]);
}

fn leaves_to(net: &Net, a: usize, short: u16, since: u32) -> Vec<(Option<u64>, u8)> {
    net.nwk_sent_by(net.nodes[a].id, since)
        .into_iter()
        .filter(|(_, h, p)| h.frame_type == nwk::FRAME_CMD && h.dst == short && p[0] == nwk::CMD_LEAVE)
        .map(|(_, h, p)| (h.dst_ext, p[1]))
        .collect()
}

#[test]
fn a_child_the_trust_center_removes_is_told_to_leave() {
    let (mut net, a, mut zed) = with_sleepy_child();
    let sa = net.nodes[a].short();
    let mut awake = Zed::new(&net.air, false);
    net.link(0, awake.id, 0);
    assert!(awake.join(&mut net, sa));
    net.run(200);
    assert_eq!(net.nodes[a].stack.child_count(), 2);
    let t = net.now();
    net.coord.remove_device(sa, zed.ext);
    net.run(300);
    assert_eq!(net.nodes[a].stack.child_count(), 1);
    assert!(leaves_to(&net, a, zed.short, t).is_empty());
    zed.rx.clear();
    zed.poll(&mut net, 20);
    let mine = |zed: &Zed| zed.rx.iter().filter(|(h, _)| h.dst == zed.short).map(|(h, p)| (h.dst_ext, p.clone())).collect::<Vec<_>>();
    assert_eq!(mine(&zed), vec![(Some(zed.ext), vec![nwk::CMD_LEAVE, 0x40])]);
    let t = net.now();
    net.coord.remove_device(sa, awake.ext);
    net.run(300);
    assert_eq!(leaves_to(&net, a, awake.short, t), vec![(Some(awake.ext), 0x40)]);
    assert_eq!(net.nodes[a].stack.child_count(), 0);
    assert!(left_reports(&net).is_empty(), "the Trust Center asked for it; it is not told that the device left");
}

#[test]
fn a_child_that_is_not_authenticated_yet_is_removed_without_a_leave() {
    let (mut net, a) = open_network();
    let sa = net.nodes[a].short();
    net.coord.legacy_update_device = true;
    let mut zed = Zed::new(&net.air, false);
    net.link(0, zed.id, 0);
    assert!(!zed.join(&mut net, sa));
    assert_ne!(zed.short, 0xFFFF, "associated");
    net.run(18_000);
    assert_eq!(net.nodes[a].stack.child_count(), 0, "not a child that counts yet");
    let t = net.now();
    net.coord.via.insert(zed.short, sa);
    net.coord.zcl(zed.short, ON_OFF, 0x11, 0x02, &[]);
    let z = zed.short.to_le_bytes();
    net.coord.nwk_command(0xFFFC, 30, &[nwk::CMD_ROUTE_REQUEST, 0x00, 0x78, z[0], z[1], 0]);
    net.run(1_000);
    let sent = net.nwk_sent_by(net.nodes[a].id, t);
    assert!(!sent.iter().any(|(m, h, _)| m.dst == zigbee::mac::Addr::Short(zed.short) && h.security));
    assert!(!sent.iter().any(|(_, h, p)| h.frame_type == nwk::FRAME_CMD && p[0] == nwk::CMD_ROUTE_REPLY));
    let t = net.now();
    net.coord.remove_device(sa, zed.ext);
    net.run(300);
    assert!(leaves_to(&net, a, zed.short, t).is_empty(), "it could not read one (R22 3.6.1.10.2)");
    net.coord.zcl(zed.short, ON_OFF, 0x11, 0x02, &[]);
    net.run(300);
    zed.drain();
    assert!(!zed.rx.iter().any(|(h, _)| h.security));
}

#[test]
fn mgmt_leave_req_about_a_child_sends_the_child_a_leave() {
    let (mut net, a, mut zed) = with_sleepy_child();
    let sa = net.nodes[a].short();
    let mut p = 0x00124B00_DEAD0003u64.to_le_bytes().to_vec();
    p.push(0);
    let seq = net.coord.zdo(sa, 0x0034, &p);
    net.run(300);
    assert_eq!(net.coord.last_zdo(sa, 0x8034).unwrap(), vec![seq, 0xC8], "UNKNOWN_DEVICE");
    assert_eq!((net.nodes[a].stack.state(), net.nodes[a].stack.child_count()), (State::Up, 1));
    let mut p = zed.ext.to_le_bytes().to_vec();
    p.push(0x80);
    let seq = net.coord.zdo(sa, 0x0034, &p);
    net.run(300);
    assert_eq!(net.coord.last_zdo(sa, 0x8034).unwrap(), vec![seq, 0x00]);
    zed.rx.clear();
    zed.poll(&mut net, 20);
    let mine = zed.rx.iter().filter(|(h, _)| h.dst == zed.short).map(|(h, p)| (h.dst_ext, p.clone())).collect::<Vec<_>>();
    assert_eq!(mine, vec![(Some(zed.ext), vec![nwk::CMD_LEAVE, 0x60])]);
    assert_eq!((net.nodes[a].stack.state(), net.nodes[a].stack.child_count()), (State::Up, 0));
}

#[test]
fn a_bad_timeout_request_is_answered_with_incorrect_value() {
    let (mut net, a, mut zed) = with_sleepy_child();
    let sa = net.nodes[a].short();
    let my_ext = net.air.borrow().ports[net.nodes[a].id].ext;
    let ask = |net: &mut Net, zed: &mut Zed, timeout: u8, configuration: u8| {
        let h = zed.header(nwk::FRAME_CMD, sa);
        zed.nwk_send(h, &[nwk::CMD_TIMEOUT_REQUEST, timeout, configuration]);
        net.run(100);
        zed.rx.clear();
        zed.poll(net, 20);
        zed.rx.last().cloned()
    };
    let (h, p) = ask(&mut net, &mut zed, 4, 0).expect("a response");
    assert_eq!(p, vec![nwk::CMD_TIMEOUT_RESPONSE, 0x00, 0x01]);
    assert_eq!((h.src_ext, h.dst_ext, h.radius), (Some(my_ext), Some(zed.ext), 1));
    assert_eq!(ask(&mut net, &mut zed, 15, 0).unwrap().1, vec![nwk::CMD_TIMEOUT_RESPONSE, 0x01, 0x01]);
    assert_eq!(ask(&mut net, &mut zed, 3, 0x01).unwrap().1, vec![nwk::CMD_TIMEOUT_RESPONSE, 0x01, 0x01]);
    net.run(15 * 60_000);
    assert_eq!(net.nodes[a].stack.child_count(), 1);
    net.run(70_000);
    assert_eq!(net.nodes[a].stack.child_count(), 0);
    let mut other = Zed::router(&net.air, 0x2B02, sa);
    let h = other.header(nwk::FRAME_CMD, sa);
    other.rx.clear();
    other.nwk_send(h, &[nwk::CMD_TIMEOUT_REQUEST, 4, 0]);
    net.run(100);
    other.drain();
    assert!(!other.rx.iter().any(|(_, p)| p[0] == nwk::CMD_TIMEOUT_RESPONSE));
}

fn ask_to_rejoin(net: &mut Net, zed: &mut Zed, short: u16, capability: u8, secured: bool) -> Option<(nwk::Header, u16, u8)> {
    let was = (zed.short, net.air.borrow().ports[zed.id].short);
    send_rejoin_request(net, zed, short, capability, secured);
    net.run(100);
    zed.poll(net, 20);
    zed.short = was.0;
    net.air.borrow_mut().ports[zed.id].short = was.1;
    zed.rx.iter().find(|(_, p)| p[0] == nwk::CMD_REJOIN_RESPONSE).map(|(h, p)| (*h, u16::from_le_bytes([p[1], p[2]]), p[3]))
}

fn send_rejoin_request(net: &mut Net, zed: &mut Zed, short: u16, capability: u8, secured: bool) {
    let parent = zed.parent;
    let mut h = zed.header(nwk::FRAME_CMD, parent);
    h.radius = 1;
    zed.rx.clear();
    zed.short = short;
    net.air.borrow_mut().ports[zed.id].short = short;
    net.air.borrow_mut().ports[zed.id].pan = PAN;
    if secured {
        zed.nwk_send(h, &[nwk::CMD_REJOIN_REQUEST, capability]);
    } else {
        h.security = false;
        h.src = short;
        let mut f = zigbee::util::Buf::new();
        zigbee::mac::Header {
            frame_type: zigbee::mac::FRAME_DATA,
            security: false,
            pending: false,
            ack_request: true,
            seq: h.seq,
            dst_pan: PAN,
            dst: zigbee::mac::Addr::Short(parent),
            src_pan: PAN,
            src: zigbee::mac::Addr::Short(short),
        }
        .write(&mut f);
        h.write(&mut f);
        f.bytes(&[nwk::CMD_REJOIN_REQUEST, capability]);
        net.air.borrow_mut().transmit(zed.id, f.as_slice());
    }
}

#[test]
fn a_rejoin_request_in_the_clear_changes_nothing_that_is_known_of_a_device() {
    let (mut net, a, mut zed) = with_sleepy_child();
    let sa = net.nodes[a].short();
    net.coord.updates.clear();
    let own_short = zed.short;
    let other_short = zed.short ^ 0x0101;
    let (h, short, status) = ask_to_rejoin(&mut net, &mut zed, other_short, 0x80, false).expect("an answer");
    assert_eq!((short, status, h.security, h.dst_ext), (other_short, 0x02, false, Some(zed.ext)));
    let (_, _, status) = ask_to_rejoin(&mut net, &mut zed, own_short, 0x88, false).expect("an answer");
    assert_eq!(status, 0x02);
    assert!(net.coord.updates.is_empty(), "the Trust Center is not asked for a key");
    assert_eq!(net.nodes[a].stack.child_count(), 1);
    net.coord.zcl(zed.short, ON_OFF, 0x11, 0x02, &[]);
    net.run(300);
    zed.rx.clear();
    zed.poll(&mut net, 20);
    assert!(zed.rx.iter().any(|(h, _)| h.dst == zed.short && h.src == 0), "frames for the child still wait for its poll");
    net.coord.legacy_update_device = true;
    net.coord.update_copies.clear();
    let (_, short, status) = ask_to_rejoin(&mut net, &mut zed, own_short, 0x80, false).expect("an answer");
    assert_eq!((short, status), (zed.short, 0x00));
    assert_eq!(net.coord.update_copies, vec![(sa, zed.ext, 0x03, true)]);
    assert_eq!(net.nodes[a].stack.child_count(), 1);
    zed.poll(&mut net, 20);
    let h = zed.header(nwk::FRAME_CMD, sa);
    zed.nwk_send(h, &[nwk::CMD_TIMEOUT_REQUEST, 0x00, 0x00]);
    net.run(8_000);
    assert_eq!(net.nodes[a].stack.child_count(), 1);
    let mut ghost = Zed::new(&net.air, true);
    ghost.ext = zed.ext;
    ghost.parent = sa;
    net.link(0, ghost.id, 0);
    send_rejoin_request(&mut net, &mut ghost, own_short, 0x80, false);
    net.run(9_000);
    assert_eq!(net.nodes[a].stack.child_count(), 0);
}

#[test]
fn a_rejoin_request_in_the_clear_does_not_make_the_bulb_forget_a_router() {
    let (mut net, a) = open_network();
    let sa = net.nodes[a].short();
    let mut x = Zed::router(&net.air, 0x2B02, sa);
    net.link(0, x.id, 0);
    x.link_status(&[(sa, 1)]);
    net.run(50);
    let toggle = |x: &mut Zed| {
        let h = x.header(nwk::FRAME_DATA, sa);
        let seq = h.seq;
        x.nwk_send(h, &[0x00, 0x01, 0x06, 0x00, 0x04, 0x01, 0x01, seq, 0x01, seq, 0x02]);
    };
    let before = net.nodes[a].commands.len();
    toggle(&mut x);
    net.run(100);
    assert_eq!(net.nodes[a].commands.len(), before + 1);
    let recorded = net.sent_by(x.id, 0).pop().unwrap();
    let mut ghost = Zed::new(&net.air, false);
    ghost.ext = x.ext;
    ghost.parent = sa;
    net.link(0, ghost.id, 0);
    let (_, _, status) = ask_to_rejoin(&mut net, &mut ghost, x.short, 0x80, false).expect("an answer");
    assert_eq!(status, 0x02);
    let (_, _, status) = ask_to_rejoin(&mut net, &mut ghost, x.short, 0x8E, false).expect("an answer");
    assert_eq!(status, 0x00);
    net.run(7_000);
    let replays = net.nodes[a].stack.stats.replays;
    net.air.borrow_mut().transmit(x.id, &recorded);
    net.run(100);
    assert_eq!(net.nodes[a].commands.len(), before + 1);
    assert_eq!(net.nodes[a].stack.stats.replays, replays + 1);
    toggle(&mut x);
    net.run(100);
    assert_eq!(net.nodes[a].commands.len(), before + 2);
}

#[test]
fn a_rejoin_is_refused_when_there_is_no_room_for_another_child() {
    let (mut net, a) = open_network();
    let sa = net.nodes[a].short();
    let mut children = more_sleepy_children(&mut net, a, CHILDREN);
    assert_eq!(net.nodes[a].stack.child_count(), CHILDREN);
    net.power_cycle(a);
    assert_eq!(net.nodes[a].stack.child_count(), CHILDREN);
    net.run(1_000);
    let mut late = Zed::router(&net.air, 0x2B02, sa);
    net.link(0, late.id, 0);
    let (_, short, status) = ask_to_rejoin(&mut net, &mut late, 0x2B02, 0x88, true).expect("an answer");
    assert_eq!((short, status), (0x2B02, 0x01), "PAN at capacity (R22 3.6.1.4.2)");
    let known = children[0].short;
    let (_, short, status) = ask_to_rejoin(&mut net, &mut children[0], known, 0x80, true).expect("an answer");
    assert_eq!((short, status), (known, 0x00));
}

fn distributed_network() -> (Net, usize) {
    let mut net = Net::new();
    net.coord.join_key = DISTRIBUTED_LINK_KEY;
    net.coord.tc_ext = u64::MAX;
    let a = net.add_node();
    net.run_until(20_000, "joined", |net| net.nodes[a].stack.state() == State::Up);
    net.run(3_000);
    (net, a)
}

#[test]
fn nobody_gets_the_network_key_by_a_rejoin_in_the_clear_where_there_is_no_trust_center() {
    let (mut net, a) = distributed_network();
    let sa = net.nodes[a].short();
    assert!(!net.coord.zdo_rx.iter().any(|(_, c, _)| *c == 0x0002), "no Trust Center to exchange a link key with");
    net.run(181_000);
    let mut zed = Zed::new(&net.air, false);
    zed.parent = sa;
    net.link(0, zed.id, 0);
    let (h, _, status) = ask_to_rejoin(&mut net, &mut zed, 0x4E0E, 0x88, false).expect("an answer");
    assert_eq!((status, h.security), (0x02, false), "PAN access denied (R22 4.6.3.3.2)");
    net.run(1_000);
    zed.drain();
    assert!(!zed.rx.iter().any(|(_, p)| p.first() == Some(&0x21)), "no Transport Key");
    assert_eq!(net.nodes[a].stack.child_count(), 0);
    zed.key = Some(NWK_KEY);
    let (_, short, status) = ask_to_rejoin(&mut net, &mut zed, 0x4E0E, 0x88, true).expect("an answer");
    assert_eq!((short, status), (0x4E0E, 0x00));
    assert_eq!(net.nodes[a].stack.child_count(), 1);
    assert!(net.coord.update_copies.is_empty());
}

#[test]
fn a_network_without_a_trust_center_has_no_key_commands() {
    let (mut net, a) = distributed_network();
    let sa = net.nodes[a].short();
    let ext = net.air.borrow().ports[net.nodes[a].id].ext;
    net.coord.devices.iter_mut().find(|d| d.short == sa).unwrap().tclk = TCLK;
    net.coord.aps_ext = u64::MAX;
    let replays = net.nodes[a].stack.stats.replays + net.nodes[a].stack.stats.bad_mic;
    net.coord.transport_network_key(sa, ext, &[0x66; 16], 1, true);
    net.run(300);
    let own = net.coord.key;
    net.coord.use_key(&[0x66; 16], 1, false);
    net.coord.zcl(sa, ON_OFF, 0x01, 0x00, &[]);
    net.run(300);
    net.coord.use_key(&own, 0, false);
    let seq = net.coord.zcl(sa, ON_OFF, 0x01, 0x01, &[]);
    net.run(300);
    assert_eq!(net.coord.last_zcl(sa, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x01, 0x00]);
    assert_eq!(net.coord.mic_failures, 0, "the bulb sends under the key it had");
    net.coord.network_key_update(0xFFFF, &[0x67; 16], 1, u64::MAX);
    net.run(300);
    net.coord.use_key(&[0x67; 16], 1, false);
    net.coord.zcl(sa, ON_OFF, 0x01, 0x00, &[]);
    net.run(300);
    net.coord.use_key(&own, 0, false);
    let unreadable = net.coord.mic_failures;
    let seq = net.coord.zcl(sa, ON_OFF, 0x01, 0x01, &[]);
    net.run(300);
    assert_eq!(net.coord.last_zcl(sa, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x01, 0x00]);
    assert_eq!(net.coord.mic_failures, unreadable);
    net.coord.remove_device(sa, ext);
    net.run(2_000);
    assert_eq!(net.nodes[a].stack.state(), State::Up);
    let _ = replays;
    net.coord.aps_ext = COORD_EXT;
    net.coord.permit(60);
    net.run(500);
    let mut zed = Zed::new(&net.air, false);
    net.link(0, zed.id, 0);
    assert!(zed.join(&mut net, sa), "the bulb itself sent the key");
    zed.rx.clear();
    let mut tunnel = vec![0x01, 0x55, 0x0E];
    tunnel.extend_from_slice(&zed.ext.to_le_bytes());
    tunnel.extend_from_slice(&[0x21, 0x01, 0x02, 0x03]);
    net.coord.nwk_data(sa, &tunnel);
    net.run(300);
    zed.drain();
    assert!(!zed.rx.iter().any(|(h, _)| !h.security));
}

#[test]
fn a_device_that_associates_again_keeps_its_address_if_it_is_the_same_kind_of_device() {
    let (mut net, a, mut zed) = with_sleepy_child();
    let sa = net.nodes[a].short();
    let was = zed.short;
    zed.mac_rx.clear();
    assert!(zed.join(&mut net, sa));
    assert_eq!(zed.short, was);
    let mut router = Zed::new(&net.air, false);
    router.ext = zed.ext;
    net.air.borrow_mut().ports[router.id].ext = zed.ext;
    net.air.borrow_mut().ports[zed.id].ext = 0;
    net.link(0, router.id, 0);
    router.parent = sa;
    net.air.borrow_mut().ports[router.id].pan = PAN;
    let mut f = zigbee::util::Buf::new();
    zigbee::mac::Header {
        frame_type: zigbee::mac::FRAME_CMD,
        security: false,
        pending: false,
        ack_request: true,
        seq: 9,
        dst_pan: PAN,
        dst: zigbee::mac::Addr::Short(sa),
        src_pan: 0xFFFF,
        src: zigbee::mac::Addr::Ext(zed.ext),
    }
    .write(&mut f);
    f.bytes(&[zigbee::mac::CMD_ASSOC_REQ, 0x8E]);
    net.air.borrow_mut().transmit(router.id, f.as_slice());
    net.run(500);
    router.poll(&mut net, 20);
    let rsp = router.mac_rx.iter().find(|(_, p)| p.first() == Some(&zigbee::mac::CMD_ASSOC_RSP)).expect("an association response").1.clone();
    assert_eq!(rsp[3], 0x00);
    assert_ne!(u16::from_le_bytes([rsp[1], rsp[2]]), was, "a new device, a new address");
}

#[test]
fn a_key_change_missed_where_there_is_no_trust_center_is_not_asked_for_in_the_clear() {
    let (mut net, a) = distributed_network();
    let id = net.nodes[a].id;
    let t = net.now();
    net.coord.use_key(&[0x77; 16], 1, false);
    net.run(200_000);
    let in_the_clear = net.sent_by(id, t).iter().filter(|f| f[0] & 7 == 1 && nwk::Header::parse(&f[9..]).is_some_and(|(h, _)| !h.security)).count();
    assert_eq!(in_the_clear, 0);
    assert_eq!(net.nodes[a].stack.state(), State::Up);
}

#[test]
fn a_broadcast_held_for_many_sleeping_children_lists_each_of_them_as_waiting() {
    let (mut net, a, zed) = with_sleepy_child();
    let mut kids = vec![zed];
    kids.extend(more_sleepy_children(&mut net, a, 11));
    let id = net.nodes[a].id;
    net.coord.zcl(0xFFFF, ON_OFF, 0x11, 0x02, &[]);
    net.run(300);
    let mut shorts = net.air.borrow().ports[id].pending.0.clone();
    shorts.sort();
    let mut want: Vec<u16> = kids.iter().map(|k| k.short).collect();
    want.sort();
    assert_eq!(shorts, want);
}

#[test]
fn a_parent_annce_cut_short_claims_nothing_even_when_it_names_many_children() {
    let (mut net, a, zed) = with_sleepy_child();
    let mut kids = vec![zed];
    kids.extend(more_sleepy_children(&mut net, a, 9));
    let sa = net.nodes[a].short();
    let mut annce = vec![kids.len() as u8 + 1];
    for k in &kids {
        annce.extend_from_slice(&k.ext.to_le_bytes());
    }
    net.coord.zdo(0xFFFC, 0x001F, &annce);
    net.run(1_000);
    assert!(net.coord.zdo_rx.iter().all(|(s, c, _)| *s != sa || *c != 0x801F), "not even the first nine");
}

#[test]
fn a_tunnel_for_a_device_that_is_not_a_child_goes_nowhere() {
    let mut net = Net::new();
    let a = net.join_node();
    let sa = net.nodes[a].short();
    let mut tunnel = vec![0x01, 0x66, 0x0E];
    tunnel.extend_from_slice(&0x00124B00000000C1u64.to_le_bytes());
    tunnel.extend_from_slice(&[0x21, 0x67, 0x30, 1, 2, 3]);
    let t = net.now();
    net.coord.nwk_data(sa, &tunnel);
    net.run(500);
    assert!(net.nwk_sent_by(net.nodes[a].id, t).iter().all(|(_, h, _)| h.security), "nothing passed on in the clear");
}

#[test]
fn a_rejoin_request_without_the_extended_source_or_capability_is_not_answered() {
    let (mut net, a, mut zed) = with_sleepy_child();
    let sa = net.nodes[a].short();
    zed.parent = sa;
    let mut h = zed.header(nwk::FRAME_CMD, sa);
    h.radius = 1;
    h.src_ext = None;
    let cap = zed.capability();
    zed.nwk_send(h, &[nwk::CMD_REJOIN_REQUEST, cap]);
    let mut h = zed.header(nwk::FRAME_CMD, sa);
    h.radius = 1;
    zed.nwk_send(h, &[nwk::CMD_REJOIN_REQUEST]);
    net.run(100);
    zed.rx.clear();
    zed.poll(&mut net, 20);
    assert!(!zed.rx.iter().any(|(_, p)| p[0] == nwk::CMD_REJOIN_RESPONSE));
}

#[test]
fn a_child_removed_while_a_broadcast_waits_for_it_leaves_the_broadcast_to_the_others() {
    let (mut net, a, zed) = with_sleepy_child();
    let mut kids = vec![zed];
    kids.extend(more_sleepy_children(&mut net, a, 1));
    let sa = net.nodes[a].short();
    let id = net.nodes[a].id;
    net.coord.zcl(0xFFFF, ON_OFF, 0x11, 0x02, &[]);
    net.run(300);
    net.coord.remove_device(sa, kids[0].ext);
    net.run(300);
    kids[0].poll(&mut net, 20);
    kids[0].poll(&mut net, 20);
    assert!(kids[0].rx.iter().all(|(h, _)| h.dst != 0xFFFF), "the removed child gets its leave, not the broadcast");
    kids[1].poll(&mut net, 20);
    assert!(kids[1].rx.iter().any(|(h, _)| h.dst == 0xFFFF), "the other still gets it");
    net.run(100);
    assert!(net.air.borrow().ports[id].pending.0.is_empty());
}

#[test]
fn a_timeout_request_cut_short_is_not_answered() {
    let (mut net, a, mut zed) = with_sleepy_child();
    let sa = net.nodes[a].short();
    let mut h = zed.header(nwk::FRAME_CMD, sa);
    h.radius = 1;
    zed.nwk_send(h, &[nwk::CMD_TIMEOUT_REQUEST, 0x03]);
    net.run(100);
    zed.rx.clear();
    zed.poll(&mut net, 20);
    assert!(!zed.rx.iter().any(|(_, p)| p[0] == nwk::CMD_TIMEOUT_RESPONSE));
}

#[test]
fn mgmt_lqi_lists_the_router_neighbours_and_then_the_end_device_children() {
    let (mut net, a, zed) = with_sleepy_child();
    let sa = net.nodes[a].short();
    let seq = net.coord.zdo(sa, 0x0031, &[0]);
    net.run(300);
    let rsp = net.coord.last_zdo(sa, 0x8031).unwrap();
    assert_eq!(rsp[..5], [seq, 0x00, 2, 0, 2], "the coordinator and the child");
    let child = &rsp[5 + 22..5 + 44];
    assert_eq!(child[8..16], zed.ext.to_le_bytes());
    assert_eq!(child[16..18], zed.short.to_le_bytes());
    assert_eq!(child[18], 0x02 | 0 << 2 | 1 << 4, "an end device, receiver off when idle, a child");
    assert_eq!(child[19..22], [0x02, 2, 255], "permit joining unknown, one below the bulb, its LQI not measured");
    assert_eq!(rsp[5 + 18..5 + 22], [0x00 | 1 << 2 | 0 << 4, 0x02, 0, rsp[5 + 21]], "the coordinator: the parent, at depth 0");
    let seq = net.coord.zdo(sa, 0x0031, &[1]);
    net.run(300);
    let rsp = net.coord.last_zdo(sa, 0x8031).unwrap();
    assert_eq!(rsp[..5], [seq, 0x00, 2, 1, 1]);
    assert_eq!(rsp[5 + 16..5 + 18], zed.short.to_le_bytes());
}

#[test]
fn mgmt_leave_for_a_child_passes_rejoin_and_remove_children_on_in_the_leave() {
    for (flags, options) in [(0x80u8, 0x60u8), (0x40, 0xC0), (0x00, 0x40)] {
        let (mut net, a, mut zed) = with_sleepy_child();
        let sa = net.nodes[a].short();
        let mut p = zed.ext.to_le_bytes().to_vec();
        p.push(flags);
        let seq = net.coord.zdo(sa, 0x0034, &p);
        net.run(300);
        assert_eq!(net.coord.last_zdo(sa, 0x8034).unwrap(), vec![seq, 0x00]);
        zed.rx.clear();
        zed.poll(&mut net, 20);
        assert!(zed.rx.iter().any(|(_, p)| p[..] == [nwk::CMD_LEAVE, options]), "flags {flags:#04x}: {:02x?}", zed.rx);
    }
}

#[test]
fn a_failed_parent_annce_rsp_takes_no_child_away() {
    let (mut net, a, zed) = with_sleepy_child();
    net.power_cycle(a);
    net.run(500);
    let mut rsp = vec![0x55, 0x8A, 1];
    rsp.extend_from_slice(&zed.ext.to_le_bytes());
    let sa = net.nodes[a].short();
    let aps = net.coord.aps_frame(0x00, 0, 0x801F, 0x0000, 0, &rsp);
    net.coord.nwk_data(sa, &aps);
    net.run(300);
    assert_eq!(net.nodes[a].stack.child_count(), 1);
    rsp[1] = 0x00;
    let aps = net.coord.aps_frame(0x00, 0, 0x801F, 0x0000, 0, &rsp);
    net.coord.nwk_data(sa, &aps);
    net.run(300);
    assert_eq!(net.nodes[a].stack.child_count(), 0, "a successful one does");
}

#[test]
fn a_child_that_was_sent_away_is_no_longer_listed_as_associated() {
    let (mut net, a, zed) = with_sleepy_child();
    let sa = net.nodes[a].short();
    let mut p = zed.ext.to_le_bytes().to_vec();
    p.push(0);
    net.coord.zdo(sa, 0x0034, &p);
    net.run(300);
    let seq = net.coord.zdo(sa, 0x0001, &[&sa.to_le_bytes()[..], &[1, 0]].concat());
    net.run(300);
    let rsp = net.coord.last_zdo(sa, 0x8001).unwrap();
    assert_eq!((rsp[0], rsp[1], rsp[12]), (seq, 0x00, 0), "no associated devices");
}

#[test]
fn a_full_child_table_takes_the_capacity_out_of_the_beacon() {
    let (mut net, a, _zed) = with_sleepy_child();
    let _kids = more_sleepy_children(&mut net, a, CHILDREN - 1);
    let id = net.nodes[a].id;
    let beacon = |net: &mut Net| {
        let t = net.now();
        net.air.borrow_mut().ports[id].queue.push_back((vec![0x03, 0x08, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0x07], 255));
        net.run(100);
        let f = net.sent_by(id, t).into_iter().find(|f| f[0] & 7 == 0).expect("a beacon");
        let (h, off) = zigbee::mac::Header::parse(&f).unwrap();
        zigbee::mac::Beacon::parse(&h, &f[off..]).unwrap()
    };
    let b = beacon(&mut net);
    assert!(!b.router_capacity && !b.end_device_capacity);
}

#[test]
fn an_association_response_waiting_for_a_device_is_listed_under_its_extended_address() {
    let (mut net, a) = open_network();
    let sa = net.nodes[a].short();
    let id = net.nodes[a].id;
    let s = sa.to_le_bytes();
    let ext = 0x00124B00000000E7u64;
    let mut assoc = vec![0x23, 0xC8, 0x04, 0x62, 0x1A, s[0], s[1], 0xFF, 0xFF];
    assoc.extend_from_slice(&ext.to_le_bytes());
    assoc.extend_from_slice(&[0x01, 0x80]);
    net.air.borrow_mut().ports[id].queue.push_back((assoc, 255));
    net.run(50);
    let pending = net.air.borrow().ports[id].pending.clone();
    assert_eq!(pending.1, vec![ext]);
    assert_eq!(pending.0.len(), 1, "and under the address it was given");
}

#[test]
fn of_two_frames_waiting_for_a_child_the_first_goes_with_frame_pending_and_the_second_without() {
    let (mut net, a, mut zed) = with_sleepy_child();
    let id = net.nodes[a].id;
    net.coord.zcl(zed.short, ON_OFF, 0x11, 0x01, &[]);
    net.run(100);
    net.coord.zcl(zed.short, ON_OFF, 0x11, 0x00, &[]);
    net.run(100);
    let t = net.now();
    zed.poll(&mut net, 20);
    zed.poll(&mut net, 20);
    let frames: Vec<u8> = net
        .sent_by(id, t)
        .into_iter()
        .filter(|f| f[0] & 7 == 1 && f[5..7] == zed.short.to_le_bytes())
        .map(|f| f[0] & 0x10)
        .collect();
    assert_eq!(frames, vec![0x10, 0x00]);
}

fn secured_rejoin(net: &mut Net, zed: &mut Zed, parent: u16, src: u16) -> Option<(u16, u8)> {
    zed.parent = parent;
    let mut h = zed.header(nwk::FRAME_CMD, parent);
    h.radius = 1;
    h.src = src;
    let cap = zed.capability();
    zed.nwk_send(h, &[nwk::CMD_REJOIN_REQUEST, cap]);
    net.run(100);
    zed.rx.clear();
    zed.poll(net, 20);
    zed.rx.iter().find(|(_, p)| p[0] == nwk::CMD_REJOIN_RESPONSE).map(|(_, p)| (u16::from_le_bytes([p[1], p[2]]), p[3]))
}

#[test]
fn a_rejoining_child_keeps_its_address_unless_it_is_taken_or_reserved() {
    let (mut net, a, mut zed) = with_sleepy_child();
    let other = more_sleepy_children(&mut net, a, 1).remove(0);
    let sa = net.nodes[a].short();
    let mine = zed.short;
    assert_eq!(secured_rejoin(&mut net, &mut zed, sa, mine), Some((mine, 0x00)));
    for taken in [sa, 0xFFF8, other.short, 0x0000] {
        let (short, status) = secured_rejoin(&mut net, &mut zed, sa, taken).expect("a rejoin response");
        assert_eq!(status, 0x00);
        assert!(![sa, 0xFFF8, other.short, 0x0000].contains(&short), "{taken:#06x} -> {short:#06x}");
        zed.short = short;
    }
}

#[test]
fn with_end_devices_not_allowed_a_new_one_cannot_rejoin_but_a_known_one_can() {
    let (mut net, a, mut zed) = with_sleepy_child();
    let sa = net.nodes[a].short();
    let seq = net.coord.zcl_ep(sa, 7, 0x0010, 0x00, 0x02, &[0x55, 0x00, 0x10, 0]);
    net.run(100);
    assert_eq!(net.coord.last_zcl_ep(sa, 7, 0x0010, 0x04).unwrap(), vec![0x18, seq, 0x04, 0x00]);
    let mine = zed.short;
    assert_eq!(secured_rejoin(&mut net, &mut zed, sa, mine), Some((mine, 0x00)), "a child it has");
    let mut stranger = Zed::new(&net.air, true);
    net.link(0, stranger.id, 0);
    stranger.key = zed.key;
    stranger.short = 0x5A5A;
    net.air.borrow_mut().ports[stranger.id].short = 0x5A5A;
    net.air.borrow_mut().ports[stranger.id].pan = PAN;
    stranger.pan = PAN;
    assert_eq!(secured_rejoin(&mut net, &mut stranger, sa, 0x5A5A).map(|r| r.1), Some(0x01), "at capacity for end devices");
}

#[test]
fn a_child_given_up_takes_its_waiting_frames_with_it() {
    let (mut net, a, zed) = with_sleepy_child();
    let id = net.nodes[a].id;
    net.coord.zcl(zed.short, ON_OFF, 0x11, 0x01, &[]);
    net.run(200);
    assert_eq!(net.air.borrow().ports[id].pending.0, vec![zed.short]);
    let mut annce = vec![0x34, 0x12];
    annce.extend_from_slice(&zed.ext.to_le_bytes());
    annce.push(zed.capability());
    net.coord.zdo(0xFFFD, 0x0013, &annce);
    net.run(500);
    assert_eq!(net.nodes[a].stack.child_count(), 0);
    assert!(net.air.borrow().ports[id].pending.0.is_empty());
}

#[test]
fn a_child_timeout_of_fourteen_is_taken_and_fifteen_refused() {
    let (mut net, a, mut zed) = with_sleepy_child();
    let sa = net.nodes[a].short();
    for (timeout, status) in [(14u8, 0x00u8), (15, 0x01)] {
        let mut h = zed.header(nwk::FRAME_CMD, sa);
        h.radius = 1;
        zed.nwk_send(h, &[nwk::CMD_TIMEOUT_REQUEST, timeout, 0x00]);
        net.run(100);
        zed.rx.clear();
        zed.poll(&mut net, 20);
        let rsp = zed.rx.iter().find(|(_, p)| p[0] == nwk::CMD_TIMEOUT_RESPONSE).expect("a timeout response").1.clone();
        assert_eq!(rsp[1], status, "timeout {timeout}");
    }
}

#[test]
fn a_childs_own_broadcast_is_not_held_for_it() {
    let (mut net, a, mut zed) = with_sleepy_child();
    let id = net.nodes[a].id;
    let mut h = zed.header(nwk::FRAME_DATA, 0xFFFF);
    h.radius = 5;
    let aps = vec![0x08, 1, 0x06, 0x00, 0x04, 0x01, 1, 0x31, 0x01, 0x32, 0x02];
    zed.nwk_send(h, &aps);
    net.run(300);
    assert!(net.air.borrow().ports[id].pending.0.is_empty(), "nothing waits for the child that sent it");
}

#[test]
fn a_refused_rejoin_reaches_a_device_that_listens_at_once() {
    let (mut net, a, _zed) = with_sleepy_child();
    let sa = net.nodes[a].short();
    net.coord.zcl_ep(sa, 7, 0x0010, 0x00, 0x02, &[0x55, 0x00, 0x10, 0]);
    net.run(100);
    let mut stranger = Zed::new(&net.air, false);
    net.link(0, stranger.id, 0);
    stranger.key = Some(NWK_KEY);
    stranger.short = 0x5B5B;
    stranger.pan = PAN;
    net.air.borrow_mut().ports[stranger.id].short = 0x5B5B;
    net.air.borrow_mut().ports[stranger.id].pan = PAN;
    stranger.parent = sa;
    let mut h = stranger.header(nwk::FRAME_CMD, sa);
    h.radius = 1;
    let cap = stranger.capability();
    stranger.nwk_send(h, &[nwk::CMD_REJOIN_REQUEST, cap]);
    net.run(50);
    stranger.drain();
    assert!(stranger.rx.iter().any(|(_, p)| p[0] == nwk::CMD_REJOIN_RESPONSE && p[3] == 0x01), "{:02x?}", stranger.rx);
}

#[test]
fn after_a_restart_link_status_goes_within_three_to_four_seconds_and_the_parent_annce_within_ten_to_twenty() {
    let (mut net, a, _zed) = with_sleepy_child();
    let id = net.nodes[a].id;
    net.power_cycle(a);
    let t = net.now();
    net.run(25_000);
    let sent = net.timed_nwk_sent_by(id, t);
    let link_status = sent.iter().find(|(_, _, h, p)| h.frame_type == nwk::FRAME_CMD && p[0] == nwk::CMD_LINK_STATUS).expect("a link status").0 - t;
    assert!((3_000..4_000).contains(&link_status), "link status {link_status} ms after the restart");
    let annce = sent.iter().find(|(_, _, h, p)| h.frame_type == nwk::FRAME_DATA && p.len() > 6 && p[2..4] == [0x1F, 0x00]).expect("a Parent_annce").0 - t;
    assert!((10_000..20_000).contains(&annce), "Parent_annce {annce} ms after the restart");
}

#[test]
fn an_unknown_device_rejoining_without_a_key_is_admitted_and_a_timeout_request_out_of_range_is_refused() {
    let (mut net, a) = open_network();
    let sa = net.nodes[a].short();
    let mut zed = Zed::new(&net.air, false);
    net.link(0, zed.id, 0);
    zed.short = 0x3456;
    zed.pan = PAN;
    net.air.borrow_mut().ports[zed.id].pan = PAN;
    net.air.borrow_mut().ports[zed.id].short = 0x3456;
    assert!(zed.rejoin_without_key(&mut net, sa), "an unknown router-capable device in the clear");
    let mut h = net.coord.header(nwk::FRAME_CMD, sa, 5);
    h.src = 0x7001;
    h.src_ext = Some(0x00124B00_0000_7001);
    let port = net.nodes[a].id;
    net.air.borrow_mut().ports[port].traces.clear();
    net.coord.nwk_send(h, &[nwk::CMD_TIMEOUT_REQUEST, 15, 0], Some(sa));
    net.run(200);
    let traces = net.air.borrow().ports[port].traces.clone();
    assert!(traces.iter().any(|t| t.kind == zigbee::hal::trace::TX && t.peer == 0x7001), "the refusal is sent to the device: {traces:?}");
}
