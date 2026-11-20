use crate::sim::*;
use std::collections::HashMap;
use zigbee::mac::{self, Addr};
use zigbee::nwk;
use zigbee::stack::State;
use zigbee::util::Buf;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn byte(&mut self) -> u8 {
        self.next() as u8
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
    fn bytes(&mut self, max: usize) -> Vec<u8> {
        let n = self.below(max as u64 + 1) as usize;
        (0..n).map(|_| self.interesting()).collect()
    }
    fn interesting(&mut self) -> u8 {
        match self.below(8) {
            0 => 0,
            1 => 0xFF,
            2 => 1,
            3 => 0x7F,
            4 => 0x80,
            _ => self.byte(),
        }
    }
    fn pick<T: Copy>(&mut self, v: &[T]) -> T {
        v[self.below(v.len() as u64) as usize]
    }
}

const CLUSTERS: [u16; 22] = [
    0x0000, 0x0003, 0x0004, 0x0005, 0x0006, 0x0008, 0x0019, 0x0300, 0x0B05, 0x1000, 0xFC5E, 0xFC5F, 0x0406, 0x000C, 0x0010, 0x0012, 0x0021, 0x000A, 0x0001,
    0x0702, 0xFFFF, 0x0020,
];

fn mutate(rng: &mut Rng, f: &mut Vec<u8>) {
    for _ in 0..1 + rng.below(4) {
        match rng.below(6) {
            0 if !f.is_empty() => {
                let i = rng.below(f.len() as u64) as usize;
                f[i] ^= 1 << rng.below(8);
            }
            1 if !f.is_empty() => {
                let i = rng.below(f.len() as u64) as usize;
                f[i] = rng.interesting();
            }
            2 if !f.is_empty() => {
                let n = rng.below(f.len() as u64) as usize;
                f.truncate(n);
            }
            3 => {
                let extra = rng.bytes(20);
                f.extend_from_slice(&extra);
            }
            4 if !f.is_empty() => {
                let i = rng.below(f.len() as u64 + 1) as usize;
                f.insert(i.min(f.len()), rng.interesting());
            }
            _ if f.len() > 1 => {
                let i = rng.below(f.len() as u64) as usize;
                f.remove(i);
            }
            _ => {}
        }
    }
}

fn disruptive_aps(aps: &[u8]) -> bool {
    let Some(&fc) = aps.first() else { return false };
    match fc & 3 {
        1 => true,
        0 | 2 => {
            let cluster = aps.get(2..4).map(|c| u16::from_le_bytes([c[0], c[1]]));
            let profile = aps.get(4..6).map(|c| u16::from_le_bytes([c[0], c[1]]));
            let ext = fc & 0x80 != 0;
            let hdr = if fc & 0x0C == 0x0C { 6 } else { 8 } + if ext { 2 } else { 0 };
            let body = aps.get(hdr..).unwrap_or(&[]);
            match (profile, cluster) {
                (Some(0x0000), Some(0x0034 | 0x0038 | 0x0036 | 0x0021 | 0x0022 | 0x0023)) => true,
                (Some(0x0000), _) => false,
                (_, Some(0x0000)) => body.first().is_none_or(|zfc| zfc & 3 == 1),
                (_, Some(0x1000)) => true,
                (_, Some(0xFC5F)) => true,
                (_, Some(0x0019)) => true,
                _ => fc & 0x0C == 0x0C,
            }
        }
        _ => false,
    }
}

struct World {
    net: Net,
    shorts: Vec<u16>,
    z: Zed,
    zport: usize,
    fc_seen: HashMap<u64, (u32, Vec<u8>)>,
    frames: u64,
    corpus: Vec<Vec<u8>>,
    changes: u32,
}

fn world() -> World {
    let air = Air::new();
    let coord = Coord::new(&air);
    let mut net = Net { air, coord, nodes: vec![], identity: IDENTITY };
    let a = net.join_node();
    let b = net.join_node();
    let c = net.join_node();
    let (pa, pb, pc) = (net.nodes[a].id, net.nodes[b].id, net.nodes[c].id);
    net.link(0, pb, 0);
    net.link(0, pc, 0);
    net.link(pa, pc, 0);
    net.run(70_000);
    let shorts: Vec<u16> = net.nodes.iter().map(|n| n.short()).collect();
    net.coord.via.insert(shorts[1], shorts[0]);
    net.coord.via.insert(shorts[2], shorts[0]);
    let z = Zed::router(&net.air, 0x4444, shorts[1]);
    let zport = z.id;
    for p in [0, pa, pc] {
        net.link(p, zport, 0);
    }
    net.coord.fc = 50_000;
    net.coord.tolerant = true;
    World { net, shorts, z, zport, fc_seen: HashMap::new(), frames: 0, corpus: vec![], changes: 0 }
}

fn check_counters(w: &mut World) {
    let log = std::mem::take(&mut w.net.air.borrow_mut().log);
    let bulbs: Vec<usize> = w.net.nodes.iter().map(|n| n.id).collect();
    for (_, from, mpdu) in log.iter() {
        if w.corpus.len() < 4000 && w.frames % 3 == 0 {
            w.corpus.push(mpdu.clone());
        } else if !w.corpus.is_empty() && w.frames % 7 == 0 {
            let i = (w.frames as usize / 7) % w.corpus.len();
            w.corpus[i] = mpdu.clone();
        }
        if !bulbs.contains(from) {
            continue;
        }
        let Some((mh, off)) = mac::Header::parse(mpdu) else { continue };
        if mh.frame_type != mac::FRAME_DATA {
            continue;
        }
        let p = &mpdu[off..];
        let Some((h, hl)) = nwk::Header::parse(p) else { continue };
        if !h.security {
            continue;
        }
        let Some(aux) = nwk::parse_aux(p, hl) else { continue };
        let body = p[hl..].to_vec();
        if let Some((last, last_body)) = w.fc_seen.get(&aux.ext) {
            assert!(aux.counter > *last || (aux.counter == *last && *last_body == body), "frame counter of {:016x} went from {} to {}", aux.ext, last, aux.counter);
        }
        w.fc_seen.insert(aux.ext, (aux.counter, body));
    }
}

fn mac_frame(rng: &mut Rng, dst: Addr, src: Addr, frame_type: u8, payload: &[u8]) -> Vec<u8> {
    let mut f = Buf::new();
    mac::Header {
        frame_type,
        security: rng.chance(3),
        pending: rng.chance(10),
        ack_request: rng.chance(50),
        seq: rng.byte(),
        dst_pan: if rng.chance(70) { PAN } else { { let r16 = rng.next() as u16; rng.pick(&[0xFFFF, 0x1234, r16]) } },
        dst,
        src_pan: if rng.chance(80) { PAN } else { rng.next() as u16 },
        src,
    }
    .write(&mut f);
    let mut v = f.as_slice().to_vec();
    v.extend_from_slice(payload);
    v.truncate(127);
    v
}

fn inject(w: &mut World, node: usize, f: Vec<u8>) {
    let id = w.net.nodes[node].id;
    w.net.air.borrow_mut().ports[id].queue.push_back((f, 200));
}

fn rand_addr(rng: &mut Rng, w: &World) -> Addr {
    match rng.below(5) {
        0 => Addr::None,
        1 => Addr::Short(0xFFFF),
        2 => Addr::Short(rng.pick(&w.shorts)),
        3 => Addr::Ext((0xA4_C138u64 << 40) + 1 + rng.below(3)),
        _ => {
            if rng.chance(50) {
                Addr::Short(rng.next() as u16)
            } else {
                Addr::Ext(rng.next())
            }
        }
    }
}

fn zcl_payload(rng: &mut Rng) -> (u16, u8, Vec<u8>) {
    let cluster = if rng.chance(90) { rng.pick(&CLUSTERS) } else { rng.next() as u16 };
    let mut fc = if rng.chance(70) { 0x00 } else { 0x01 };
    if rng.chance(10) {
        fc |= 0x04;
    }
    if rng.chance(10) {
        fc |= 0x08;
    }
    if rng.chance(20) {
        fc |= 0x10;
    }
    let mut z = vec![fc];
    if fc & 0x04 != 0 {
        z.extend_from_slice(&rng.pick(&[0x1141u16, 0x0000, 0xFFFF]).to_le_bytes());
    }
    z.push(rng.byte());
    let cmd = if fc & 3 == 0 { rng.below(0x18) as u8 } else { rng.below(0x50) as u8 };
    z.push(cmd);
    let body = match (fc & 3, cmd) {
        (0, 0x00 | 0x0C | 0x15) => {
            let mut b = vec![];
            for _ in 0..rng.below(30) {
                b.extend_from_slice(&(rng.pick(&[0x0000u16, 0x0001, 0x4000, 0xFFFD, 0x5000, 0x0003, 0x0004, 0x0006, 0x0010, 0x0400]).wrapping_add(rng.below(8) as u16)).to_le_bytes());
            }
            b
        }
        (0, 0x02 | 0x03 | 0x05) => {
            let mut b = vec![];
            for _ in 0..rng.below(8) {
                b.extend_from_slice(&(rng.below(0x20) as u16 + rng.pick(&[0u16, 0x4000, 0x5000])).to_le_bytes());
                let t = rng.pick(&[0x10u8, 0x20, 0x21, 0x23, 0x28, 0x30, 0x41, 0x42, 0x18, 0x29, 0x2B, 0x48, 0x4C, 0xF0, 0xE2, 0x00, 0xFF]);
                b.push(t);
                let v = rng.bytes(8);
                b.extend_from_slice(&v);
            }
            b
        }
        (0, 0x06) => {
            let mut b = vec![];
            for _ in 0..rng.below(6) {
                b.push(rng.below(2) as u8);
                b.extend_from_slice(&(rng.below(0x10) as u16 + rng.pick(&[0u16, 0x4000])).to_le_bytes());
                b.push(rng.pick(&[0x10u8, 0x20, 0x21, 0x29, 0x30, 0x42]));
                let v = rng.bytes(8);
                b.extend_from_slice(&v);
            }
            b
        }
        (0, 0x08) => {
            let mut b = vec![];
            for _ in 0..rng.below(8) {
                b.push(rng.below(2) as u8);
                b.extend_from_slice(&(rng.below(0x10) as u16).to_le_bytes());
            }
            b
        }
        _ => rng.bytes(40),
    };
    z.extend_from_slice(&body);
    if rng.chance(15) {
        mutate(rng, &mut z);
    }
    (cluster, rng.pick(&[1u8, 1, 1, 2, 3, 7, 8, 0xFF, 242, 0, 5]), z)
}

fn step(w: &mut World, rng: &mut Rng) {
    let target = rng.below(w.shorts.len() as u64) as usize;
    let dst = match rng.below(10) {
        0 => rng.pick(&[0xFFFFu16, 0xFFFD, 0xFFFC, 0xFFFB]),
        1 => rng.next() as u16,
        _ => w.shorts[target],
    };
    w.frames += 1;
    match rng.below(16) {
        0..=3 => {
            let (cluster, ep, z) = zcl_payload(rng);
            let mut fc = if nwk::is_broadcast(dst) { 0x08 } else if rng.chance(50) { 0x40 } else { 0x00 };
            if rng.chance(10) {
                fc = 0x0C;
            }
            let aps = if fc == 0x0C {
                let mut a = vec![fc];
                a.extend_from_slice(&rng.byte().to_le_bytes());
                a.push(0);
                a.extend_from_slice(&cluster.to_le_bytes());
                a.extend_from_slice(&0x0104u16.to_le_bytes());
                a.push(1);
                a.push(rng.byte());
                a.extend_from_slice(&z);
                a
            } else {
                let profile = rng.pick(&[0x0104u16, 0x0104, 0x0104, 0xC05E, 0xA1E0, 0x0000, 0xFFFF, 0x1234]);
                w.net.coord.aps_frame(fc, ep, cluster, profile, rng.pick(&[1u8, 2, 242, 0xFF]), &z)
            };
            if !disruptive_aps(&aps) {
                w.net.coord.nwk_data(dst, &aps);
            }
        }
        4 | 5 => {
            let cluster = if rng.chance(60) { rng.below(0x40) as u16 } else { 0x8000 + rng.below(0x40) as u16 };
            if [0x0034u16, 0x0038, 0x0036, 0x0021, 0x0022, 0x0023].contains(&cluster) {
                return;
            }
            let p = rng.bytes(50);
            w.net.coord.zdo(dst, cluster, &p);
        }
        6 => {
            let mut aps = vec![rng.byte() & !0x03 | if rng.chance(50) { 0 } else { 2 }];
            let more = rng.bytes(60);
            aps.extend_from_slice(&more);
            if rng.chance(30) {
                aps[0] |= 0x80;
            }
            if !disruptive_aps(&aps) {
                w.net.coord.nwk_data(dst, &aps);
            }
        }
        7 => {
            let cmd = rng.pick(&[1u8, 2, 3, 5, 7, 8, 9, 0x0B, 0x0C, 0x0D, 0x00, 0xFF]);
            let mut p = vec![cmd];
            let more = rng.bytes(40);
            p.extend_from_slice(&more);
            let mut p = p;
            let radius = rng.pick(&[0u8, 1, 2, 30, 255]);
            let mut h = w.net.coord.header(nwk::FRAME_CMD, dst, radius);
            if rng.chance(30) {
                h.src_ext = None;
            }
            if rng.chance(20) {
                h.dst_ext = Some(rng.next());
            }
            p.truncate(if h.dst_ext.is_some() { 30 } else { 60 });
            w.net.coord.nwk_send(h, &p, None);
        }
        8 | 9 => {
            let ft = if rng.chance(50) { nwk::FRAME_CMD } else { nwk::FRAME_DATA };
            let mut h = w.z.header(ft, dst);
            if rng.chance(30) {
                h.src = { let r16 = rng.next() as u16; rng.pick(&[0x0000u16, 0xFFFF, w.shorts[0], w.shorts[2], r16]) };
            }
            h.radius = rng.pick(&[0u8, 1, 2, 5, 30, 255]);
            h.discover = rng.below(4) as u8;
            if rng.chance(20) {
                h.multicast = Some(rng.byte());
                h.dst = rng.next() as u16;
            }
            if rng.chance(20) {
                let count = rng.below(nwk::MAX_RELAYS as u64 + 1) as u8;
                let index = if rng.chance(70) && count > 0 { count - 1 } else { rng.byte() };
                let mut list = [0u16; nwk::MAX_RELAYS];
                for r in list.iter_mut().take(count as usize) {
                    *r = { let r16 = rng.next() as u16; rng.pick(&[w.shorts[0], w.shorts[1], w.shorts[2], 0x4444, r16]) };
                }
                h.relays = Some((count, index, list));
            }
            if rng.chance(20) {
                h.dst_ext = Some(rng.next());
            }
            if rng.chance(30) {
                h.src_ext = if rng.chance(50) { None } else { Some(rng.next()) };
            }
            h.end_device_initiator = rng.chance(10);
            let payload = if ft == nwk::FRAME_CMD {
                let cmd = rng.pick(&[1u8, 2, 3, 5, 6, 7, 8, 9, 0x0B, 0x0C, 0x0D, 0x00, 0xFF]);
                let mut p = vec![cmd];
                let more = rng.bytes(40);
                p.extend_from_slice(&more);
                p
            } else {
                let (cluster, ep, z) = zcl_payload(rng);
                let a = vec![0x40, ep, cluster as u8, (cluster >> 8) as u8, 0x04, 0x01, 1, rng.byte()];
                let mut a = a;
                a.extend_from_slice(&z);
                if disruptive_aps(&a) {
                    return;
                }
                a
            };
            let mac_dst = { let r16 = rng.next() as u16; rng.pick(&[w.shorts[1], 0xFFFF, w.shorts[1], r16]) };
            w.z.mac_send(mac_dst, h, &payload);
        }
        10 | 11 => {
            if w.corpus.is_empty() {
                return;
            }
            let i = rng.below(w.corpus.len() as u64) as usize;
            let mut f = w.corpus[i].clone();
            mutate(rng, &mut f);
            f.truncate(127);
            inject(w, target, f);
        }
        12 => {
            let src = rand_addr(rng, w);
            let dst = rand_addr(rng, w);
            let cmd = rng.pick(&[1u8, 2, 3, 4, 5, 6, 7, 8, 9, 0x0A, 0x0B, 0x0C, 0x13, 0xFF]);
            let mut p = vec![cmd];
            let more = rng.bytes(20);
            p.extend_from_slice(&more);
            let ft = rng.pick(&[mac::FRAME_CMD, mac::FRAME_CMD, mac::FRAME_BEACON, mac::FRAME_ACK, 4, 5, 7]);
            let f = mac_frame(rng, dst, src, ft, &p);
            inject(w, target, f);
        }
        13 => {
            let dst = if rng.chance(50) { Addr::Short(0xFFFF) } else { Addr::Ext((0xA4_C138u64 << 40) + 1 + target as u64) };
            let src = Addr::Ext(0x00AA_BBCC_DDEE_FF01);
            let mut p = vec![0x0B, 0x00, if matches!(dst, Addr::Ext(_)) { 0x03 } else { 0x0B }, 0x00, 0x10, 0x5E, 0xC0, rng.pick(&[0x11u8, 0x19, 0x01])];
            p.push(rng.byte());
            p.push(rng.pick(&[0x00u8, 0x02, 0x06, 0x07, 0x10, 0x12, 0x14, 0x16, 0x40, 0x41, 0x42, 0x01, 0x03, 0x11, 0x13, 0x15, 0x17, 0xFF]));
            let more = rng.bytes(60);
            p.extend_from_slice(&more);
            if rng.chance(20) {
                mutate(rng, &mut p);
            }
            let mut f = Buf::new();
            mac::Header { frame_type: mac::FRAME_DATA, security: false, pending: false, ack_request: false, seq: rng.byte(), dst_pan: rng.pick(&[0xFFFF, PAN]), dst, src_pan: 0x1234, src }.write(&mut f);
            let mut v = f.as_slice().to_vec();
            v.extend_from_slice(&p);
            v.truncate(127);
            inject(w, target, v);
        }
        14 => {
            let mut nwkp = vec![rng.pick(&[0x0Cu8, 0x8C, 0x0D, 0x8D, 0x4C, 0xCC, 0x00])];
            let more = rng.bytes(40);
            nwkp.extend_from_slice(&more);
            let mut f = vec![0x01, 0x08, rng.byte(), 0xFF, 0xFF, 0xFF, 0xFF];
            if rng.chance(20) {
                f = vec![0x41, 0xC8, rng.byte(), 0xFF, 0xFF, 0xFF, 0xFF];
                f.extend_from_slice(&rng.next().to_le_bytes());
            }
            f.extend_from_slice(&nwkp);
            f.truncate(127);
            inject(w, target, f);
        }
        16 => {
            let (cluster, _, z) = zcl_payload(rng);
            if z.len() < 3 || [0x0000u16, 0x1000, 0xFC5F, 0x0019].contains(&cluster) {
                return;
            }
            let fc = z[0];
            let at = if fc & 0x04 != 0 { 5 } else { 3 };
            if z.len() < at {
                return;
            }
            w.net.coord.zcl_encrypted(dst, cluster, fc & !0x04, z[at - 1], &z[at..]);
        }
        17 => {
            let cmd = rng.pick(&[0x06u8, 0x08, 0x0E, 0x0F, 0x10, 0x03, 0x0A, 0xFF]);
            let mut body = vec![cmd];
            let more = rng.bytes(40);
            body.extend_from_slice(&more);
            let link = w.net.coord.device(dst).map_or(TCLK, |d| d.tclk);
            let key_id = rng.pick(&[0u8, 2, 3]);
            let aps = w.net.coord.aps_command(&link, key_id, &body);
            w.net.coord.nwk_data(dst, &aps);
        }
        18 => {
            let (cluster, _, z) = zcl_payload(rng);
            if z.len() < 3 || [0x0000u16, 0x1000, 0xFC5F, 0x0019].contains(&cluster) || z[0] & 0x04 != 0 {
                return;
            }
            let mut payload = z[3..].to_vec();
            let extra = rng.bytes(120);
            payload.extend_from_slice(&extra);
            let block = 8 + rng.below(60) as usize;
            w.net.coord.zcl_fragmented(dst, cluster, z[0], z[2], &payload, block);
            if rng.chance(30) {
                w.net.coord.repeat_fragments();
            }
        }
        _ => {
            let cmd = rng.pick(&[0x00u8, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B]);
            let p = rng.bytes(50);
            let mut zcl = vec![rng.pick(&[0x19u8, 0x11, 0x01, 0x09]), rng.byte(), cmd];
            zcl.extend_from_slice(&p);
            let fc = if nwk::is_broadcast(dst) { 0x08 } else { 0x00 };
            let aps = w.net.coord.aps_frame(fc, 242, 0x0021, 0xA1E0, 242, &zcl);
            w.net.coord.nwk_data(dst, &aps);
        }
    }
}

fn refresh(w: &mut World) {
    let now: Vec<u16> = w.net.nodes.iter().map(|n| n.short()).collect();
    if now != w.shorts {
        w.changes += 1;
        w.net.coord.via.clear();
        w.net.coord.via.insert(now[1], now[0]);
        w.net.coord.via.insert(now[2], now[0]);
        w.shorts = now;
        w.z.parent = w.shorts[1];
    }
}

fn answers(w: &mut World, n: usize) -> bool {
    for _ in 0..4 {
        refresh(w);
        let short = w.shorts[n];
        w.net.coord.zcl_rx.clear();
        let seq = w.net.coord.zcl(short, 0x0000, 0x00, 0x00, &[0x00, 0x00]);
        w.net.run(4_000);
        if w.net.coord.last_zcl(short, 0x0000, 0x01).is_some_and(|r| r[1] == seq) {
            return true;
        }
    }
    false
}

fn lqi_table(w: &mut World, n: usize) -> Option<Vec<u8>> {
    let short = w.shorts[n];
    let mut all = vec![];
    let mut start = 0u8;
    loop {
        let seq = w.net.coord.zdo(short, 0x0031, &[start]);
        w.net.run(3_000);
        let r = w.net.coord.last_zdo(short, 0x8031).filter(|r| r[0] == seq)?;
        assert_eq!(r[1], 0, "Mgmt_Lqi status");
        let (total, idx, count) = (r[2], r[3], r[4]);
        assert_eq!(idx, start);
        assert_eq!(r.len(), 5 + count as usize * 22, "Mgmt_Lqi length");
        assert!(total as usize <= 64, "neighbour table total {total}");
        for e in r[5..].chunks(22) {
            let (ext, short) = (u64::from_le_bytes(e[8..16].try_into().unwrap()), u16::from_le_bytes([e[16], e[17]]));
            let child = (e[18] >> 4) & 0x07 == 0x01;
            assert!(!child || (short != 0x0000 && ext != COORD_EXT), "node {n} lists the coordinator {short:04x} {ext:016x} as its child");
            all.push(short);
        }
        start += count;
        if count == 0 || start >= total {
            break;
        }
    }
    let mut d = all.clone();
    d.sort();
    d.dedup();
    assert_eq!(d.len(), all.len(), "duplicate neighbours in node {n}: {all:04x?}");
    Some(all.iter().flat_map(|s| s.to_le_bytes()).collect())
}

fn fuzz(seed: u64, steps: u64, jump_days: u64) {
    let mut rng = Rng(seed);
    let mut w = world();
    for n in 0..3 {
        assert!(answers(&mut w, n), "seed {seed}: node {n} before fuzzing");
    }
    let start = w.net.now();
    let mut jumped = 0u64;
    for i in 0..steps {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| step(&mut w, &mut rng)));
        let ms = 1 + rng.below(25) as u32;
        w.net.run(ms);
        refresh(&mut w);
        if jump_days > 0 && rng.chance(1) && rng.chance(5) {
            let j = rng.below(jump_days * 86_400_000 / 20) as u32;
            if (w.net.now() as u64 + j as u64) < 0xF000_0000 {
                let mut air = w.net.air.borrow_mut();
                air.now = air.now.wrapping_add(j);
                drop(air);
                w.net.coord.zcl_rx.clear();
                jumped += j as u64;
            }
        }
        if rng.chance(1) && rng.chance(2) {
            let n = rng.below(3) as usize;
            w.net.power_cycle(n);
        }
        if i % 500 == 0 {
            check_counters(&mut w);
            w.net.coord.zcl_rx.clear();
            w.net.coord.zdo_rx.clear();
            w.net.coord.nwk_cmd_rx.clear();
            w.z.rx.clear();
            w.z.mac_rx.clear();
            w.net.air.borrow_mut().ports[w.zport].queue.clear();
            for n in 0..3 {
                assert_eq!(w.net.nodes[n].stack.state(), State::Up, "seed {seed} step {i}: node {n} left the network");
            }
        }
    }
    check_counters(&mut w);
    let nports = w.net.air.borrow().ports.len();
    for p in 0..nports {
        w.net.link(p, w.zport, 0);
    }
    w.net.run(200_000);
    check_counters(&mut w);
    for n in 0..3 {
        assert_eq!(w.net.nodes[n].stack.state(), State::Up, "seed {seed}: node {n} after the stream");
        assert!(answers(&mut w, n), "seed {seed}: node {n} no longer answers after the stream (relayed for 1, 2)");
        lqi_table(&mut w, n);
    }
    for n in 0..3 {
        w.net.power_cycle(n);
        w.net.run(30_000);
        assert_eq!(w.net.nodes[n].stack.state(), State::Up, "seed {seed}: node {n} after reload");
        refresh(&mut w);
        assert!(answers(&mut w, n), "seed {seed}: node {n} no longer answers after reload");
    }
    check_counters(&mut w);
    std::eprintln!("seed {seed}: {} frames, {} ms run, {} ms jumped, {} address changes, {} frames the simulated coordinator could not parse", w.frames, w.net.now().wrapping_sub(start), jumped, w.changes, w.net.coord.harness_panics);
}

#[test]
fn a_stream_of_odd_frames_leaves_every_bulb_in_the_network_reachable_and_its_counters_rising() {
    for seed in [1u64, 2] {
        fuzz(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1, 5_000, 3);
    }
}

struct Long {
    net: Net,
    fc_seen: HashMap<u64, (u32, Vec<u8>)>,
    powered: Vec<bool>,
}

fn long_counters(l: &mut Long) {
    let log = std::mem::take(&mut l.net.air.borrow_mut().log);
    let bulbs: Vec<usize> = l.net.nodes.iter().map(|n| n.id).collect();
    for (_, from, mpdu) in log.iter() {
        if !bulbs.contains(from) {
            continue;
        }
        let Some((mh, off)) = mac::Header::parse(mpdu) else { continue };
        if mh.frame_type != mac::FRAME_DATA {
            continue;
        }
        let p = &mpdu[off..];
        let Some((h, hl)) = nwk::Header::parse(p) else { continue };
        if !h.security {
            continue;
        }
        let Some(aux) = nwk::parse_aux(p, hl) else { continue };
        let body = p[hl..].to_vec();
        if let Some((last, last_body)) = l.fc_seen.get(&aux.ext) {
            assert!(aux.counter > *last || (aux.counter == *last && *last_body == body), "frame counter of {:016x} went from {} to {}", aux.ext, last, aux.counter);
        }
        l.fc_seen.insert(aux.ext, (aux.counter, body));
    }
}

fn long_via(l: &mut Long) -> Vec<bool> {
    let ports: Vec<usize> = l.net.nodes.iter().map(|n| n.id).collect();
    let shorts: Vec<u16> = l.net.nodes.iter().map(|n| n.short()).collect();
    let lqi = l.net.air.borrow().lqi.clone();
    l.net.coord.via.clear();
    l.net.coord.source_route.clear();
    let mut prev: HashMap<usize, Option<usize>> = HashMap::new();
    let mut queue = std::collections::VecDeque::new();
    prev.insert(usize::MAX, None);
    queue.push_back(usize::MAX);
    while let Some(at) = queue.pop_front() {
        let from_port = if at == usize::MAX { 0 } else { ports[at] };
        for (k, &p) in ports.iter().enumerate() {
            if l.powered[k] && !prev.contains_key(&k) && lqi[from_port][p] > 0 {
                prev.insert(k, Some(at));
                queue.push_back(k);
            }
        }
    }
    for k in 0..ports.len() {
        let mut relays = vec![];
        let mut cur = match prev.get(&k) {
            Some(Some(p)) => *p,
            _ => continue,
        };
        while cur != usize::MAX {
            relays.push(shorts[cur]);
            cur = prev[&cur].unwrap();
        }
        if !relays.is_empty() {
            l.net.coord.source_route.insert(shorts[k], relays);
        }
    }
    (0..ports.len()).map(|k| prev.contains_key(&k)).collect()
}

fn long_answers(l: &mut Long, n: usize) -> bool {
    l.net.coord.seen.clear();
    for _ in 0..5 {
        let _ = long_via(l);
        let short = l.net.nodes[n].short();
        l.net.coord.zcl_rx.clear();
        let seq = l.net.coord.zcl(short, 0x0000, 0x00, 0x00, &[0x00, 0x00]);
        l.net.run(5_000);
        if l.net.coord.last_zcl(short, 0x0000, 0x01).is_some_and(|r| r[1] == seq) {
            return true;
        }
    }
    false
}

fn long_run(seed: u64, days: u32, start: u32) {
    let mut rng = Rng(seed);
    let air = Air::new();
    air.borrow_mut().now = start;
    let coord = Coord::new(&air);
    let net = Net { air, coord, nodes: vec![], identity: IDENTITY };
    let mut l = Long { net, fc_seen: HashMap::new(), powered: vec![] };
    for _ in 0..4 {
        l.net.join_node();
        l.powered.push(true);
    }
    let ports: Vec<usize> = l.net.nodes.iter().map(|n| n.id).collect();
    l.net.link(0, ports[2], 0);
    l.net.link(0, ports[3], 0);
    l.net.link(ports[0], ports[3], 0);
    l.net.lossy(ports[1], ports[2], 10);
    l.net.run(70_000);
    let mut zed = Zed::new(&l.net.air, true);
    let parent = l.net.nodes[1].short();
    for p in [0, ports[0], ports[2], ports[3]] {
        l.net.link(p, zed.id, 0);
    }
    let zed_joined = zed.join(&mut l.net, parent);
    assert!(zed_joined, "the end device joins through bulb 1");
    let mut hours = 0u64;
    let mut events = HashMap::<&str, u32>::new();
    let mut offs: Vec<(u32, usize)> = vec![];
    for day in 0..days {
        for _hour in 0..24 {
            hours += 1;
            let window = 20_000 + rng.below(40_000) as u32;
            let mut t = 0;
            let _ = long_via(&mut l);
            while t < window {
                let n = rng.below(l.net.nodes.len() as u64) as usize;
                let short = l.net.nodes[n].short();
                match rng.below(6) {
                    0 => {
                        l.net.coord.zcl(short, 0x0006, 0x01, rng.below(3) as u8, &[]);
                    }
                    1 => {
                        l.net.coord.zcl(short, 0x0008, 0x01, 0x04, &[rng.byte(), 0x0A, 0x00]);
                    }
                    2 => {
                        l.net.coord.zcl(short, 0x0000, 0x00, 0x00, &[0x04, 0x00, 0x05, 0x00]);
                    }
                    3 => {
                        l.net.coord.zcl(0xFFFD, 0x0006, 0x01, 0x01, &[]);
                    }
                    4 => {
                        zed.poll(&mut l.net, 20);
                    }
                    _ => {
                        zed.zcl_to_coordinator(0x0402, &[0x18, rng.byte(), 0x0A, 0x00, 0x00, 0x29, rng.byte(), rng.byte()]);
                    }
                }
                let step = 200 + rng.below(3_000) as u32;
                l.net.run(step);
                t += step;
            }
            if rng.chance(3) {
                let n = rng.below(l.net.nodes.len() as u64) as usize;
                if l.powered[n] {
                    l.net.power_cycle(n);
                    *events.entry("power_cycle").or_default() += 1;
                }
            }
            if rng.chance(2) {
                let n = rng.below(l.net.nodes.len() as u64) as usize;
                if l.powered[n] {
                    l.net.set_power(n, false);
                    l.powered[n] = false;
                    offs.push((1 + rng.below(30) as u32, n));
                    *events.entry("power_off").or_default() += 1;
                }
            }
            for e in offs.iter_mut() {
                e.0 = e.0.saturating_sub(1);
                if e.0 == 0 {
                    l.net.set_power(e.1, true);
                    l.net.power_cycle(e.1);
                    l.powered[e.1] = true;
                    *events.entry("power_on").or_default() += 1;
                }
            }
            offs.retain(|e| e.0 > 0);
            if rng.chance(1) && rng.chance(30) {
                let n = rng.below(l.net.nodes.len() as u64) as usize;
                let short = l.net.nodes[n].short();
                let version = 0x0002_0000 + hours as u32;
                let image = make_image(2000, rng.byte());
                l.net.coord.ota_file = Some(OtaFile { manufacturer: 0x1141, image_type: 0x0390, version, data: make_ota_file(&image, 0x1141, 0x0390, version, 0, false) });
                l.net.coord.ota_end = None;
                l.net.coord.ota_notify(short);
                l.net.run(90_000);
                *events.entry("ota").or_default() += 1;
                if l.net.coord.ota_end == Some(0) {
                    *events.entry("ota_done").or_default() += 1;
                }
                l.net.coord.ota_file = None;
            }
            if rng.chance(1) && rng.chance(20) {
                let n = rng.below(l.net.nodes.len() as u64) as usize;
                if l.powered[n] {
                    let short = l.net.nodes[n].short();
                    l.net.coord.zdo(short, 0x0034, &[0; 8].iter().copied().chain([0x80]).collect::<Vec<u8>>());
                    l.net.run(20_000);
                    *events.entry("leave_rejoin").or_default() += 1;
                }
            }
            long_counters(&mut l);
            let rest = 3_600_000u32 - window;
            {
                let mut air = l.net.air.borrow_mut();
                air.now = air.now.wrapping_add(rest);
            }
            l.net.run(50);
            l.net.coord.zcl_rx.clear();
            l.net.coord.zdo_rx.clear();
            l.net.coord.nwk_cmd_rx.clear();
            l.net.coord.seen.clear();
            zed.rx.clear();
            zed.mac_rx.clear();
        }
        l.net.run(30_000);
        for n in 0..l.net.nodes.len() {
            if !l.powered[n] || !long_via(&mut l)[n] {
                continue;
            }
            assert_eq!(l.net.nodes[n].stack.state(), State::Up, "seed {seed} day {day}: bulb {n} is up");
            assert!(long_answers(&mut l, n), "seed {seed} day {day}: bulb {n} at {:04x} answers (now {:#x})", l.net.nodes[n].short(), l.net.now());
        }
        long_counters(&mut l);
    }
    for (_, n) in offs.drain(..) {
        l.net.set_power(n, true);
        l.net.power_cycle(n);
        l.powered[n] = true;
    }
    l.net.run(60_000);
    for n in 0..l.net.nodes.len() {
        let before = (l.net.nodes[n].short(), l.net.nodes[n].stack.pan_id(), l.net.nodes[n].stack.channel());
        l.net.power_cycle(n);
        l.net.coord.seen.clear();
        l.net.run(30_000);
        let after = (l.net.nodes[n].short(), l.net.nodes[n].stack.pan_id(), l.net.nodes[n].stack.channel());
        assert_eq!(before, after, "seed {seed}: bulb {n} reloads the same network");
        assert!(long_answers(&mut l, n), "seed {seed}: bulb {n} answers after the last reload");
    }
    long_counters(&mut l);
    std::eprintln!("long seed {seed}: {days} days from {start:#x} to {:#x}, {hours} hours, events {events:?}", l.net.now());
}

#[test]
fn a_week_of_use_power_cuts_updates_and_rejoins_across_the_clock_wrap_keeps_every_bulb_reachable() {
    let days = 7;
    long_run(1u64.wrapping_mul(0x2545_F491_4F6C_DD1D) | 1, days, 0xFFFF_FFFFu32.wrapping_sub(days * 43_200_000));
}
