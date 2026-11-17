use crate::sim::*;
use zigbee::nwk;
use zigbee::stack::State;

const ON_OFF: u16 = 0x0006;

fn neighbor(net: &mut Net, n: usize, short: u16) -> Zed {
    let me = net.nodes[n].short();
    let mut z = Zed::router(&net.air, short, me);
    net.link(0, z.id, 0);
    z.link_status(&[(me, 1)]);
    net.run(50);
    z
}

fn readdress(z: &mut Zed, short: u16) {
    z.short = short;
    z.air.borrow_mut().ports[z.id].short = short;
}

fn conflicts(net: &Net) -> Vec<(u16, u16)> {
    net.coord
        .nwk_cmd_rx
        .iter()
        .filter(|(s, p)| *s != 0x0000 && p.len() >= 4 && p[0] == nwk::CMD_NETWORK_STATUS && p[1] == 0x0D)
        .map(|(s, p)| (*s, u16::from_le_bytes([p[2], p[3]])))
        .collect()
}

fn neighbor_table(net: &mut Net, n: usize) -> Vec<(u16, u64)> {
    let short = net.nodes[n].short();
    let mut out = Vec::new();
    let mut start = 0u8;
    loop {
        net.coord.zdo(short, 0x0031, &[start]);
        net.run(200);
        let p = net.coord.last_zdo(short, 0x8031).expect("Mgmt_Lqi_rsp");
        assert_eq!(p[1], 0, "Mgmt_Lqi_rsp status");
        let (total, count) = (p[2], p[4]);
        for e in p[5..].chunks(22).take(count as usize) {
            out.push((u16::from_le_bytes([e[16], e[17]]), u64::from_le_bytes(e[8..16].try_into().unwrap())));
        }
        start += count;
        if start >= total || count == 0 {
            return out;
        }
    }
}

fn toggle_through(net: &mut Net, via: u16, z: &mut Zed, dst: u16) -> bool {
    z.drain();
    z.rx.clear();
    let aps = net.coord.aps_frame(0x00, 1, ON_OFF, 0x0104, 1, &[0x01, 0x42, 0x02]);
    let h = net.coord.header(nwk::FRAME_DATA, dst, 30);
    net.coord.nwk_send(h, &aps, Some(via));
    net.run(500);
    z.drain();
    z.rx.iter().any(|(h, p)| h.dst == dst && h.src == 0x0000 && p.ends_with(&[0x01, 0x42, 0x02]))
}

#[test]
fn a_neighbouring_router_that_takes_a_new_address_is_reached_under_it() {
    let mut net = Net::new();
    let n = net.join_node();
    let me = net.nodes[n].short();
    let (old, new) = (0x1C3B, 0x7B77);
    let mut r = neighbor(&mut net, n, old);
    r.announce();
    for _ in 0..3 {
        net.run(15_000);
        r.link_status(&[(me, 1)]);
    }
    assert!(neighbor_table(&mut net, n).contains(&(old, r.ext)), "{:x?}", neighbor_table(&mut net, n));
    assert!(toggle_through(&mut net, me, &mut r, old), "through the bulb under the old address");
    let o: [u8; 2] = u16::to_le_bytes(old);
    net.coord.nwk_command(0xFFFD, 30, &[nwk::CMD_NETWORK_STATUS, 0x0D, o[0], o[1]]);
    net.run(1_000);
    readdress(&mut r, new);
    r.announce();
    net.run(50);
    r.link_status(&[(me, 1)]);
    let t_move = net.now();
    net.run(1_000);
    let entries: Vec<u16> = neighbor_table(&mut net, n).iter().filter(|e| e.1 == r.ext).map(|e| e.0).collect();
    assert_eq!(entries, vec![new], "right after the move the bulb lists the router as {entries:x?}");
    assert!(toggle_through(&mut net, me, &mut r, new), "right after the move");
    for at in [70_000u32, 180_000, 300_000, 600_000] {
        while net.now() < t_move + at {
            net.run(15_000);
            r.link_status(&[(me, 1)]);
        }
        let table = neighbor_table(&mut net, n);
        let entries: Vec<u16> = table.iter().filter(|e| e.1 == r.ext).map(|e| e.0).collect();
        assert_eq!(entries, vec![new], "{} s after the move the bulb lists the router as {entries:x?}", at / 1000);
        for k in 0..3 {
            assert!(toggle_through(&mut net, me, &mut r, new), "{} s after the move, command {k} did not reach the router", at / 1000);
        }
    }
    assert!(conflicts(&net).iter().all(|c| c.1 != new), "no conflict reported on the new address: {:x?}", conflicts(&net));
}

#[test]
fn late_copies_of_its_own_announcement_are_no_conflict() {
    let mut net = Net::new();
    let n = net.join_node();
    let id = net.nodes[n].id;
    let ext = net.air.borrow().ports[id].ext;
    net.run(20_000);
    let mut x = neighbor(&mut net, n, 0x2B02);
    for with_ext in [false, true] {
        for late in [2_000u32, 5_000, 10_000, 20_000, 60_000] {
            net.run(12_000);
            let me0 = net.nodes[n].short();
            let o = me0.to_le_bytes();
            let t = net.now();
            net.coord.nwk_command(0xFFFD, 30, &[nwk::CMD_NETWORK_STATUS, 0x0D, o[0], o[1]]);
            net.run(1_500);
            let me = net.nodes[n].short();
            assert_ne!(me, me0, "the report on its address is obeyed");
            let (_, own, payload) = net
                .timed_nwk_sent_by(id, t)
                .into_iter()
                .map(|(t, _, h, p)| (t, h, p))
                .find(|(_, h, p)| h.src == me && h.dst == 0xFFFD && p.len() > 4 && p[2..4] == [0x13, 0x00])
                .expect("its Device_annce under the new address");
            let sent = net.timed_nwk_sent_by(id, t).into_iter().find(|(_, _, h, _)| h.seq == own.seq && h.src == me).unwrap().0;
            net.run(sent + late - net.now());
            let mut echo = own;
            echo.radius = echo.radius.saturating_sub(2).max(1);
            echo.src_ext = if with_ext { Some(ext) } else { None };
            let reports = conflicts(&net).len();
            x.mac_send(0xFFFF, echo, &payload);
            net.run(3_000);
            assert_eq!(net.nodes[n].short(), me, "a copy {late} ms late, IEEE address in it: {with_ext}");
            assert_eq!(conflicts(&net).len(), reports, "a copy {late} ms late, IEEE address in it: {with_ext}: {:x?}", conflicts(&net));
            assert_eq!(net.nodes[n].stack.state(), State::Up);
        }
    }
}

#[test]
fn a_bulb_off_during_a_channel_change_rejoins_on_the_new_channel_while_the_network_is_closed() {
    let mut net = Net::new();
    let a = net.join_node();
    let b = net.join_node();
    let (sa, sb) = (net.nodes[a].short(), net.nodes[b].short());
    net.coord.permit(0);
    net.run(1_000);
    net.set_power(b, false);
    let mut p = (1u32 << 25).to_le_bytes().to_vec();
    p.extend_from_slice(&[0xFE, 1]);
    net.coord.zdo(0xFFFD, 0x0038, &p);
    net.run(10_000);
    net.air.borrow_mut().ports[0].channel = 25;
    assert_eq!(net.nodes[a].stack.channel(), 25, "A, on, follows");
    assert_eq!(net.nodes[a].short(), sa);
    let devices = net.coord.devices.len();
    net.power_cycle(b);
    net.set_power(b, true);
    assert_eq!(net.nodes[b].stack.channel(), 11);
    net.run_until(240_000, "B back on channel 25", |net| net.nodes[b].stack.channel() == 25 && net.nodes[b].stack.state() == State::Up);
    net.run(3_000);
    assert!(net.coord.rejoins.iter().any(|(s, secure)| *s == sb && *secure), "rejoins {:x?}", net.coord.rejoins);
    assert_eq!(net.nodes[b].short(), sb, "B keeps its address");
    assert_eq!(net.coord.devices.len(), devices, "no new association");
    for (n, s) in [(a, sa), (b, sb)] {
        let seq = net.coord.zcl(s, ON_OFF, 0x01, 0x00, &[]);
        net.run(200);
        assert_eq!(net.coord.last_zcl(s, ON_OFF, 0x0B), Some(vec![0x18, seq, 0x0B, 0x00, 0x00]), "bulb {n} answers on channel 25");
    }
}
