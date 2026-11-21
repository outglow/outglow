use ble::LinkState;
use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::sim::{Params, Rig, COLOUR, DIMMABLE, TUNABLE, UNIT};

struct R(u64);

impl R {
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

fn channel_map(r: &mut R) -> [u8; 5] {
    loop {
        let mut m = [0u8; 5];
        let density = r.below(4);
        for c in 0..37 {
            let on = match density {
                0 => r.below(8) == 0,
                1 => r.below(2) == 0,
                _ => r.below(8) != 0,
            };
            if on {
                m[c / 8] |= 1 << (c % 8);
            }
        }
        if (0..37).filter(|c| m[c / 8] & (1 << (c % 8)) != 0).count() >= 2 {
            return m;
        }
    }
}

fn params(r: &mut R) -> Params {
    let interval = [6u16, 6, 7, 8, 12, 16, 24, 40, 80, 160][r.below(10) as usize];
    let win_size = 1 + r.below((interval.min(9) - 1) as u64) as u8;
    Params {
        access_address: r.next() as u32 | 1,
        crc_init: (r.next() as u32) & 0xff_ffff,
        win_size,
        win_offset: r.below(interval as u64 + 1) as u16,
        interval,
        latency: 0,
        timeout: (interval / 4 + 20).max(50 + r.below(400) as u16),
        map: channel_map(r),
        hop: 5 + r.below(12) as u8,
    }
}

fn alive(rig: &mut Rig) -> Result<(), String> {
    let got = catch_unwind(AssertUnwindSafe(|| rig.read(0x03)));
    match got {
        Ok(name) if !name.is_empty() => Ok(()),
        Ok(_) => Err("empty name".into()),
        Err(p) => Err(p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default()),
    }
}

fn run_centrals(seed: u64) -> Result<(usize, usize, usize), String> {
    let mut r = R(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let identity = [DIMMABLE, TUNABLE, COLOUR][(seed % 3) as usize];
    let mut rig = Rig::new(identity);
    let (mut conns, mut updates, mut maps) = (0, 0, 0);
    let rounds = 3 + r.below(5) as usize;
    for round in 0..rounds {
        let p = params(&mut r);
        {
            let mut c = rig.central();
            c.params = p;
            c.place_in_window = r.below(p.win_size as u64 * UNIT);
            c.ppm = r.below(101) as i64 - 50;
            c.silent = false;
            c.connected = false;
            c.connect = true;
        }
        rig.run(1500);
        if rig.state() != LinkState::Connected || !rig.central().connected {
            return Err(format!("seed {seed} round {round}: no connection with {p:?}"));
        }
        conns += 1;
        alive(&mut rig).map_err(|e| format!("seed {seed} round {round} {p:?}: no answer after connecting: {e}"))?;
        let steps = 2 + r.below(10) as usize;
        for step in 0..steps {
            match r.below(6) {
                0 | 1 => {
                    let cur = rig.central().params;
                    let interval = [6u16, 7, 8, 12, 24, 40, 80][r.below(7) as usize];
                    let win_size = 1 + r.below((interval.min(9) - 1) as u64) as u8;
                    let win_offset = r.below(interval as u64 + 1) as u16;
                    let timeout = (interval / 4 + 20).max(50 + r.below(400) as u16);
                    let lead = [6u16, 6, 7, 10, 20, 100][r.below(6) as usize];
                    let place = r.below(win_size as u64 * UNIT);
                    rig.central().place_in_window = place;
                    rig.central().update_connection(win_size, win_offset, interval, timeout, lead);
                    let wait = (lead as u64 + 4) * cur.interval.max(interval) as u64 * 5 / 4 + 200;
                    rig.run(wait);
                    updates += 1;
                    alive(&mut rig).map_err(|e| format!("seed {seed} round {round} step {step}: update to interval {interval} offset {win_offset} size {win_size} place {place} lead {lead} from {cur:?}: {e}"))?;
                }
                2 => {
                    let cur = rig.central().params;
                    let m = channel_map(&mut r);
                    let lead = [6u16, 6, 7, 10, 20, 100][r.below(6) as usize];
                    rig.central().update_channel_map(m, lead);
                    rig.run((lead as u64 + 4) * cur.interval as u64 * 5 / 4 + 200);
                    maps += 1;
                    alive(&mut rig).map_err(|e| format!("seed {seed} round {round} step {step}: map {m:02x?} lead {lead}: {e}"))?;
                }
                3 => {
                    let cur = rig.central().params;
                    let budget = (cur.timeout as u64 * 10 / 3).min(2500);
                    let ms = r.below(budget);
                    rig.central().silent = true;
                    rig.run(ms);
                    rig.central().silent = false;
                    rig.run(300);
                    alive(&mut rig).map_err(|e| format!("seed {seed} round {round} step {step}: after {ms} ms silent ({cur:?}): {e}"))?;
                }
                4 => {
                    if rig.login(b"unpaired", b"1234") {
                        let level = 1 + r.below(100) as u8;
                        rig.command(0xf1, &[level]);
                        rig.command(0xd0, &[1]);
                        if !rig.is_on() {
                            return Err(format!("seed {seed} round {round} step {step}: light not on after command"));
                        }
                    } else {
                        return Err(format!("seed {seed} round {round} step {step}: default login refused"));
                    }
                }
                _ => {
                    let ms = r.below(5000);
                    rig.run(ms);
                    alive(&mut rig).map_err(|e| format!("seed {seed} round {round} step {step}: after {ms} ms: {e}"))?;
                }
            }
        }
        match r.below(3) {
            0 => {
                rig.central().terminate();
                rig.run(1000);
            }
            1 => {
                let t = rig.central().params.timeout as u64 * 10;
                rig.central().connected = false;
                rig.run(t + 1000);
            }
            _ => {
                let t = rig.central().params.timeout as u64 * 10;
                rig.central().silent = true;
                rig.run(t + 1000);
                rig.central().connected = false;
                rig.central().silent = false;
            }
        }
        if rig.state() != LinkState::Advertising {
            return Err(format!("seed {seed} round {round}: still connected after the central left"));
        }
    }
    Ok((conns, updates, maps))
}

#[test]
fn fuzz_centrals_in_turn() {
    let cases: u64 = std::env::var("FUZZ_CASES").ok().and_then(|s| s.parse().ok()).unwrap_or(20);
    let start: u64 = std::env::var("FUZZ_START").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
    let (mut c, mut u, mut m, mut fails) = (0, 0, 0, Vec::new());
    for seed in start..start + cases {
        match catch_unwind(AssertUnwindSafe(|| run_centrals(seed))) {
            Ok(Ok((a, b, d))) => {
                c += a;
                u += b;
                m += d;
            }
            Ok(Err(e)) => fails.push(e),
            Err(p) => fails.push(format!("seed {seed}: panic {:?}", p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())))),
        }
    }
    eprintln!("{cases} cases, {} failed; {c} connections, {u} parameter updates, {m} channel maps", fails.len());
    for f in fails.iter().take(20) {
        eprintln!("  {f}");
    }
    assert!(fails.is_empty());
}

fn long_case(seed: u64) -> Result<usize, String> {
    let mut r = R(seed.wrapping_mul(0x2545_F491_4F6C_DD1D) | 1);
    let mut rig = Rig::new(DIMMABLE);
    let mut p = Params::default();
    p.interval = 6;
    p.timeout = 100;
    p.map = channel_map(&mut r);
    rig.central().params = p;
    rig.central().connect = true;
    rig.run(1500);
    if rig.state() != LinkState::Connected {
        return Err(format!("seed {seed}: no connection"));
    }
    let target = 0xffffu16.wrapping_sub(r.below(40) as u16);
    let mut checks = 0;
    loop {
        let ev = rig.central().event;
        if ev >= 0xe000 {
            break;
        }
        rig.run(20_000);
        alive(&mut rig).map_err(|e| format!("seed {seed}: at event {ev}: {e}"))?;
        checks += 1;
    }
    while rig.central().event < target && rig.central().event > 0x100 {
        if rig.central().event < 0xff00 { rig.run(1000); } else { rig.run(5); }
    }
    let ev = rig.central().event;
    let lead = 6 + r.below(60) as u16;
    match r.below(3) {
        0 => {
            let m = channel_map(&mut r);
            rig.central().update_channel_map(m, lead);
        }
        1 => {
            let interval = [6u16, 8, 24][r.below(3) as usize];
            rig.central().update_connection(1, 0, interval, 100, lead);
        }
        _ => {
            let m = channel_map(&mut r);
            rig.central().update_channel_map(m, lead);
            rig.run(3);
            let interval = [6u16, 8, 24][r.below(3) as usize];
            rig.central().update_connection(1, 0, interval, 100, lead + 3);
        }
    }
    rig.run(3000);
    alive(&mut rig).map_err(|e| format!("seed {seed}: update at event {ev:#x} lead {lead}: {e}"))?;
    for _ in 0..5 {
        rig.run(10_000);
        alive(&mut rig).map_err(|e| format!("seed {seed}: after wrap: {e}"))?;
        checks += 1;
    }
    Ok(checks)
}

#[test]
fn fuzz_long_connections_across_the_event_counter_wrap() {
    let cases: u64 = std::env::var("FUZZ_CASES").ok().and_then(|s| s.parse().ok()).unwrap_or(2);
    let start: u64 = std::env::var("FUZZ_START").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
    let mut fails = Vec::new();
    let mut checks = 0;
    for seed in start..start + cases {
        match catch_unwind(AssertUnwindSafe(|| long_case(seed))) {
            Ok(Ok(n)) => checks += n,
            Ok(Err(e)) => fails.push(e),
            Err(p) => fails.push(format!("seed {seed}: panic {:?}", p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())))),
        }
    }
    eprintln!("{cases} long connections, {} failed, {checks} checks", fails.len());
    for f in &fails {
        eprintln!("  {f}");
    }
    assert!(fails.is_empty());
}

mod air {
    use super::R;
    use crate::sim::{run_all, Ether, Rig, DIMMABLE};
    use ble::crypto::{mesh_crypt, mesh_mic};
    use std::cell::RefCell;
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::rc::Rc;

    const NAME: &[u8; 16] = b"EgloMesh\0\0\0\0\0\0\0\0";
    const PASSWORD: &[u8; 16] = b"s3cret\0\0\0\0\0\0\0\0\0\0";
    const LTK: [u8; 16] = *b"0123456789abcdef";
    const ADDRS: [[u8; 6]; 3] = [[0x0a, 0x22, 0x11, 0x38, 0xc1, 0xa4], [0x0b, 0x22, 0x11, 0x38, 0xc1, 0xa4], [0x0c, 0x22, 0x11, 0x38, 0xc1, 0xa4]];

    fn paired(ether: &Rc<RefCell<Ether>>, address: [u8; 6]) -> Rig {
        let rig = Rig::in_ether(ether, address, DIMMABLE);
        let mut nv = vec![0xa7];
        nv.extend_from_slice(NAME);
        nv.extend_from_slice(PASSWORD);
        nv.extend_from_slice(&LTK);
        nv.extend_from_slice(&[0, 0, 1]);
        rig.world.borrow_mut().nv = Some(nv);
        rig.restart()
    }

    fn air_pdu(sn: u32, src: u16, dst: u16, op: u8, params: &[u8], hops: u8, ltk: &[u8; 16]) -> Vec<u8> {
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
        p[31] = 2;
        p[34] = hops;
        let nonce: [u8; 8] = p[8..16].try_into().unwrap();
        let mic = mesh_mic(ltk, &nonce, &p[16..35]);
        p[35..].copy_from_slice(&mic[..4]);
        mesh_crypt(ltk, &nonce, &mut p[16..35]);
        p
    }

    fn send(rigs: &mut [Rig], dst: u16, op: u8, params: &[u8], ms: u64) {
        let packet = rigs[0].command_packet(dst, op, params);
        rigs[0].central().send_att(&[&[0x52, 0x15, 0][..], &packet].concat());
        let mut all: Vec<&mut Rig> = rigs.iter_mut().collect();
        run_all(&mut all, ms);
    }

    fn sync(rigs: &mut [Rig]) {
        let now = rigs.iter().map(|r| r.world.borrow().now).max().unwrap();
        for r in rigs.iter_mut() {
            r.run_until(now);
        }
    }

    fn case(seed: u64) -> Result<usize, String> {
        let mut r = R(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let ether = Rc::new(RefCell::new(Ether::default()));
        let n = 2 + r.below(2) as usize;
        let mut rigs: Vec<Rig> = ADDRS[..n].iter().map(|a| paired(&ether, *a)).collect();
        rigs[0].connect();
        if !rigs[0].login(b"EgloMesh", b"s3cret") {
            return Err(format!("seed {seed}: login"));
        }
        rigs[0].enable_status();
        sync(&mut rigs);
        let mut sn = 0x10u32;
        let steps = 10 + r.below(40) as usize;
        for _ in 0..steps {
            match r.below(8) {
                0 | 1 => {
                    let dst = [0xffffu16, 0x000b, 0x000c, 0x000a, r.below(0x10000) as u16][r.below(5) as usize];
                    let op = [0xd0u8, 0xf1, 0xf0, 0xda, 0xe0, r.next() as u8][r.below(6) as usize];
                    let op = if op & 0x3f == 0x23 { 0xf1 } else { op };
                    let params: Vec<u8> = (0..1 + r.below(8)).map(|_| r.next() as u8).collect();
                    let ms = 50 + r.below(1500);
                    send(&mut rigs, dst, op, &params, ms);
                }
                2 | 3 => {
                    let to = r.below(n as u64) as usize;
                    sn = sn.wrapping_add(1 + r.below(3) as u32);
                    let s = if r.below(4) == 0 { r.next() as u32 } else { sn };
                    let dst = [0xffffu16, 0x000b, 0x000c, r.below(0x10000) as u16][r.below(4) as usize];
                    let op = [0xd0u8 | 0xc0, 0xf1 | 0xc0, 0xda | 0xc0, 0xdb | 0xc0, r.next() as u8][r.below(5) as usize];
                    let op = if op & 0x3f == 0x23 { 0xf1 } else { op };
                    let params: Vec<u8> = (0..r.below(10)).map(|_| r.next() as u8).collect();
                    let hops = [0u8, 1, 2, 5, 0xff, r.next() as u8][r.below(6) as usize];
                    let src = [0x21u16, 0x0a, 0x0b, r.below(0x10000) as u16][r.below(4) as usize];
                    let key = if r.below(10) == 0 { [7u8; 16] } else { LTK };
                    let mut p = air_pdu(s, src, dst, op, &params, hops, &key);
                    match r.below(6) {
                        0 => {
                            let i = r.below(p.len() as u64) as usize;
                            p[i] ^= 1 << r.below(8);
                        }
                        1 => p.truncate(r.below(p.len() as u64) as usize),
                        2 => p = (0..r.below(40)).map(|_| r.next() as u8).collect(),
                        _ => {}
                    }
                    if !p.is_empty() {
                        ether.borrow_mut().deliver(to, &p);
                    }
                    let ms = r.below(300);
                    let mut all: Vec<&mut Rig> = rigs.iter_mut().collect();
                    run_all(&mut all, ms);
                }
                4 => {
                    let a = r.below(n as u64) as usize;
                    let b = r.below(n as u64) as usize;
                    let mut e = ether.borrow_mut();
                    if r.below(2) == 0 {
                        e.blocked.push((a, b));
                    } else {
                        e.blocked.clear();
                    }
                }
                5 => {
                    rigs[0].central().terminate();
                    let mut all: Vec<&mut Rig> = rigs.iter_mut().collect();
                    run_all(&mut all, 1000 + r.below(2000));
                    drop(all);
                    rigs[0].central().connected = false;
                    rigs[0].central().connect = true;
                    let mut all: Vec<&mut Rig> = rigs.iter_mut().collect();
                    run_all(&mut all, 1500);
                    drop(all);
                    if !rigs[0].central().connected {
                        return Err(format!("seed {seed}: no reconnection"));
                    }
                    if !rigs[0].login(b"EgloMesh", b"s3cret") {
                        return Err(format!("seed {seed}: relogin"));
                    }
                    rigs[0].enable_status();
                    sync(&mut rigs);
                }
                _ => {
                    let ms = r.below(3000);
                    let mut all: Vec<&mut Rig> = rigs.iter_mut().collect();
                    run_all(&mut all, ms);
                }
            }
        }
        ether.borrow_mut().blocked.clear();
        let mut all: Vec<&mut Rig> = rigs.iter_mut().collect();
        run_all(&mut all, 3000);
        drop(all);
        sync(&mut rigs);
        let before: Vec<_> = rigs.iter().map(|x| x.ble.borrow().air_stats()).collect();
        let level = 10 + r.below(80) as u8;
        send(&mut rigs, 0xffff, 0xf1, &[level], 1500);
        send(&mut rigs, 0xffff, 0xd0, &[1], 1500);
        let want = (rigs[0].is_on(), rigs[0].level());
        for (i, rig) in rigs.iter().enumerate() {
            if (rig.is_on(), rig.level()) != want || !rig.is_on() {
                if std::env::var("AIR_DEBUG").is_ok() { for (k, x) in rigs.iter().enumerate() { eprintln!("bulb {k}: before {:?} after {:?} state {:?}", before[k], x.ble.borrow().air_stats(), x.state()); } }
                return Err(format!("seed {seed}: bulb {i} on {} level {} after a clean broadcast, the connected bulb {want:?}", rig.is_on(), rig.level()));
            }
        }
        Ok(steps)
    }

    #[test]
    fn fuzz_mesh_air_between_bulbs() {
        let cases: u64 = std::env::var("FUZZ_CASES").ok().and_then(|s| s.parse().ok()).unwrap_or(10);
        let start: u64 = std::env::var("FUZZ_START").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
        let mut fails = Vec::new();
        let mut steps = 0;
        for seed in start..start + cases {
            match catch_unwind(AssertUnwindSafe(|| case(seed))) {
                Ok(Ok(n)) => steps += n,
                Ok(Err(e)) => fails.push(e),
                Err(p) => fails.push(format!("seed {seed}: panic {:?}", p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())))),
            }
        }
        eprintln!("{cases} mesh cases, {} failed, {steps} steps", fails.len());
        for f in fails.iter().take(20) {
            eprintln!("  {f}");
        }
        assert!(fails.is_empty());
    }
}
