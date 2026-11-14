//! Telink mesh on the air between two (or three) bulbs with our firmware
//! (emulator/mesh_sim.py): the app connects to A, commands with dst != 0 go
//! over the radio (channels 2/12/23/34, network access code, LTK) to B; B's
//! responses (0xdb) return to the app through A. `--image relay=`: a third
//! node C hears only B, commands to C go A -> B -> C.

use super::ble_central::{
    discover, fmt_light, hx, ok, secure_pw, set_mesh_packets, tel_get, unhx, Bench, MeshClient, Status, Transport,
    OP_MESH_ADDR, OP_POWER, OP_STATUS_REQ, OP_STATUS_RSP, OP_WHITE_BRIGHTNESS,
};
use super::interop::node_heuristic;
use super::{SimCtx, SimDef};
use crate::air::NodeId;
use crate::session::{Light, Stop, R};

pub fn def() -> SimDef {
    SimDef { name: "mesh", doc: "Telink mesh on the air between bulbs with our firmware (BLE=1)", run }
}

const MESH_NAME: &[u8] = b"EgloMesh";
const MESH_PW: &[u8] = b"s3cret";
const MESH_LTK: &[u8] = b"0123456789abcdef";
const ADDR_B: u16 = 0x0022;
const ADDR_C: u16 = 0x0044;
const ADDR_NONE: u16 = 0x0055;

fn mac(hexs: &str) -> Vec<u8> {
    let mut v = unhx(hexs);
    v.reverse();
    v
}

struct Run<'a> {
    cx: &'a mut SimCtx,
    b: Bench,
    names: Vec<(NodeId, &'static str)>,
}

impl Run<'_> {
    fn check(&mut self, name: &str, okv: bool, detail: &str) {
        self.cx.results.push((name.to_string(), okv, detail.to_string()));
        self.cx.say(&format!("{}: {name} {detail}", if okv { "PASS" } else { "FAIL" }));
    }

    fn name(&self, id: NodeId) -> &'static str {
        self.names.iter().find(|(i, _)| *i == id).unwrap().1
    }

    fn connect(&mut self, id: NodeId) -> R<(Vec<u8>, Vec<(u8, u16)>, Transport, MeshClient)> {
        self.b.ble_connect(id, 24, None);
        let Some(adva) = self.b.ble_wait_connected(id, 15000.0)? else {
            return Err(Stop::Error(format!("AssertionError: no BLE connection to {}", self.name(id))));
        };
        let mac_le = unhx(&adva);
        let (_, tel, _) = discover(&mut self.b, id)?;
        let tr = Transport::new(id, &tel);
        let cl = MeshClient::new(&mac_le);
        Ok((mac_le, tel, tr, cl))
    }

    /// Like the app when adding a bulb: factory login, setMesh, then the address (0xe0).
    fn pair(&mut self, id: NodeId, expect_mac: &[u8], addr: Option<u16>) -> R<()> {
        let (mac_le, _, mut tr, mut cl) = self.connect(id)?;
        let mut em = expect_mac.to_vec();
        em.reverse();
        let mut ml = mac_le.clone();
        ml.reverse();
        let nm = self.name(id);
        self.check(&format!("{nm}: MAC from flash {}", hx(&em)), mac_le == expect_mac, &hx(&ml));
        ok(cl.login(&mut self.b, &mut tr, b"unpaired", &secure_pw(&ml))?)?;
        for pkt in set_mesh_packets(&cl.key.unwrap(), MESH_NAME, MESH_PW, MESH_LTK) {
            ok(tr.write(&mut self.b, 0x14, &pkt)?)?;
            tr.sleep(&mut self.b, 50.0)?;
        }
        let rsp = ok(tr.read(&mut self.b, 0x14)?)?;
        self.check(&format!("{nm}: setMesh -> 0x07"), rsp.first() == Some(&0x07), &hx(&rsp));
        self.b.ble_disconnect(id, 3000.0)?;
        self.b.run(id, 500.0)?;
        if let Some(addr) = addr {
            let (_, _, mut tr, mut cl) = self.connect(id)?;
            ok(cl.login(&mut self.b, &mut tr, MESH_NAME, MESH_PW)?)?;
            ok(cl.send(&mut self.b, &mut tr, OP_MESH_ADDR, &[addr as u8, (addr >> 8) as u8], 0)?)?;
            tr.sleep(&mut self.b, 200.0)?;
            self.b.ble_disconnect(id, 3000.0)?;
            self.b.run(id, 500.0)?;
        }
        Ok(())
    }
}

fn st_list(v: &[Status]) -> String {
    format!("[{}]", v.iter().map(|s| s.repr()).collect::<Vec<_>>().join(", "))
}

fn run(cx: &mut SimCtx) -> R<()> {
    let ia = cx.image("bin", "target/tc32/bulb-tc32.bin");
    let ib = cx.images.iter().find(|i| i.label == "bin-b").cloned().unwrap_or_else(|| ia.clone());
    let relay = cx.images.iter().any(|i| i.label == "relay");
    let (mac_a, mac_b, mac_c) = (mac("a4c138112233"), mac("a4c138445566"), mac("a4c138778899"));
    let addr_a = mac_a[0] as u16;
    let mut na = node_heuristic(&ia, cx.cpi)?;
    let mut nb = node_heuristic(&ib, cx.cpi)?;
    let with00 = |m: &[u8]| {
        let mut v = m.to_vec();
        v.extend_from_slice(&[0, 0]);
        v
    };
    na.write_flash(0xFF000, &with00(&mac_a));
    nb.write_flash(0xFF000, &with00(&mac_b));
    let mut b = Bench::new(na, "A", false, cx.log());
    let (a, bb) = (NodeId::Dut, b.w.add_bulb(nb, "B"));
    let mut names = vec![(a, "A"), (bb, "B")];
    let mut c = None;
    if relay {
        let mut nc = node_heuristic(&ia, cx.cpi)?;
        nc.write_flash(0xFF000, &with00(&mac_c));
        let id = b.w.add_bulb(nc, "C");
        names.push((id, "C"));
        c = Some(id);
    }
    b.cluster();
    if let Some(cid) = c {
        let ba = b.w.air.ble_air.as_mut().unwrap();
        ba.blocked.insert((a, cid));
        ba.blocked.insert((cid, a));
    }
    b.run_ms(a, 1500.0)?;
    let mut r = Run { cx, b, names };

    // --- pairing the bulbs into one network (A keeps the default address)
    r.pair(a, &mac_a, None)?;
    r.pair(bb, &mac_b, Some(ADDR_B))?;
    if let Some(cid) = c {
        r.pair(cid, &mac_c, Some(ADDR_C))?;
    }

    // --- app on A
    let (_, tel, mut tr, mut cl) = r.connect(a)?;
    ok(cl.login(&mut r.b, &mut tr, MESH_NAME, MESH_PW)?)?;
    let has_key = cl.key.is_some();
    r.check("A: login with the network name/password", has_key, "");
    let mut q = vec![0x12];
    q.extend_from_slice(&(tel_get(&tel, 0x11).unwrap() + 1).to_le_bytes());
    q.extend_from_slice(&[0x01, 0x00]);
    r.b.att_ok(a, &q)?;
    ok(tr.write(&mut r.b, 0x11, &[0x01])?)?;
    tr.sleep(&mut r.b, 400.0)?;

    let mut cmd = |r: &mut Run, op: u8, params: &[u8], dest: u16, wait: f64| -> R<(Light, Light, Vec<Status>)> {
        let n = r.b.ntfs_len(a);
        ok(cl.send(&mut r.b, &mut tr, op, params, dest)?)?;
        tr.sleep(&mut r.b, wait)?;
        let got = cl.statuses(&r.b.ntfs(a)[n..]);
        Ok((r.b.light(a), r.b.light(bb), got))
    };
    let lights = |r: &Run| {
        let s = format!("A {} | B {}", fmt_light(&r.b.light(a)), fmt_light(&r.b.light(bb)));
        match c {
            Some(cid) => format!("{s} | C {}", fmt_light(&r.b.light(cid))),
            None => s,
        }
    };
    let ab = |la: &Light, lb: &Light| format!("A {} | B {}", fmt_light(la), fmt_light(lb));

    let (la, lb) = (r.b.light(a), r.b.light(bb));
    let t = lights(&r);
    r.check("start: both bulbs lit at 100 %", la.on && lb.on, &t);

    let (la, lb, _) = cmd(&mut r, OP_POWER, &[0], 0xFFFF, 1500.0)?;
    r.check("0xd0 off to 0xffff -> A and B go off (B over the air)", !la.on && !lb.on, &ab(&la, &lb));

    let (la, lb, _) = cmd(&mut r, OP_POWER, &[1], ADDR_B, 1500.0)?;
    r.check(&format!("0xd0 on to {ADDR_B:#06x} -> only B"), !la.on && lb.on, &ab(&la, &lb));

    cmd(&mut r, OP_POWER, &[1], 0xFFFF, 1500.0)?;
    let (la, lb, _) = cmd(&mut r, OP_WHITE_BRIGHTNESS, &[64], 0xFFFF, 2500.0)?;
    r.check("0xf1 64 to 0xffff -> both ~20-25 % (stock curve)",
            17.0 < la.duty && la.duty < 30.0 && 17.0 < lb.duty && lb.duty < 30.0, &ab(&la, &lb));

    let (_, _, nt) = cmd(&mut r, OP_STATUS_REQ, &[0x10], ADDR_B, 2500.0)?;
    let rsp: Vec<&Status> = nt.iter().filter(|x| x.op == OP_STATUS_RSP && x.mesh_id == Some(ADDR_B)).collect();
    let shown = rsp.last().map(|x| x.repr()).unwrap_or_else(|| st_list(&nt));
    r.check(&format!("0xda to {ADDR_B:#06x} -> 0xdb response from B through A (brightness 64, on)"),
            rsp.last().is_some_and(|x| x.white_brightness == 64 && x.on), &shown);

    let (_, _, nt) = cmd(&mut r, OP_STATUS_REQ, &[0x10], 0xFFFF, 2500.0)?;
    let mut ids: Vec<i64> = nt.iter().filter(|x| x.op == OP_STATUS_RSP).filter_map(|x| x.mesh_id.map(|m| m as i64)).collect();
    ids.sort();
    ids.dedup();
    let mut exp = vec![addr_a as i64, ADDR_B as i64];
    if c.is_some() {
        exp.push(ADDR_C as i64);
    }
    exp.sort();
    exp.dedup();
    r.check("0xda to 0xffff -> responses from A (locally) and the others (air)", ids == exp,
            &format!("{} (expected {})", crate::py::ints(ids.clone()), crate::py::ints(exp.clone())));

    let (la0, lb0) = (r.b.light(a), r.b.light(bb));
    let (la, lb, _) = cmd(&mut r, OP_POWER, &[0], ADDR_NONE, 1500.0)?;
    r.check(&format!("0xd0 to nonexistent {ADDR_NONE:#06x} -> nobody reacts"), la.on == la0.on && lb.on == lb0.on, &ab(&la, &lb));

    let (la, lb, _) = cmd(&mut r, OP_POWER, &[0], addr_a, 1500.0)?;
    r.check(&format!("0xd0 off to {addr_a:#06x} (A's default address from the MAC) -> only A"), !la.on && lb.on, &ab(&la, &lb));
    cmd(&mut r, OP_POWER, &[1], addr_a, 1500.0)?;

    if let Some(cid) = c {
        let lc = r.b.light(cid);
        r.check("C is lit before the forwarding test", lc.on, &fmt_light(&lc));
        let (la, lb, _) = cmd(&mut r, OP_POWER, &[0], ADDR_C, 3000.0)?;
        let lc = r.b.light(cid);
        let t = lights(&r);
        r.check(&format!("0xd0 off to {ADDR_C:#06x} (C out of range of A) -> C goes off through B (relay, ttl)"),
                !lc.on && la.on && lb.on, &t);
        let (_, _, nt) = cmd(&mut r, OP_STATUS_REQ, &[0x10], ADDR_C, 4000.0)?;
        let rsp: Vec<&Status> = nt.iter().filter(|x| x.op == OP_STATUS_RSP && x.mesh_id == Some(ADDR_C)).collect();
        let shown = rsp.last().map(|x| x.repr()).unwrap_or_else(|| st_list(&nt));
        r.check(&format!("0xda to {ADDR_C:#06x} -> 0xdb response from C returns through B to A and the app (off)"),
                rsp.last().is_some_and(|x| !x.on), &shown);
        let (la, lb, _) = cmd(&mut r, OP_POWER, &[1], 0xFFFF, 3000.0)?;
        let lc = r.b.light(cid);
        let t = lights(&r);
        r.check("0xd0 on to 0xffff -> A, B and C (C through B)", la.on && lb.on && lc.on, &t);
    }

    let (la, lb, _) = cmd(&mut r, OP_POWER, &[0], 0, 1500.0)?;
    r.check("0xd0 off to 0 -> only A (locally), B stays lit", !la.on && lb.on, &ab(&la, &lb));

    let ba = r.b.w.air.ble_air.as_ref().unwrap();
    let n_mesh = ba.packets.iter().filter(|p| !(37..=39).contains(&p.2)).count();
    let n_adv = ba.packets.len() - n_mesh;
    let first = |s: String| s.split_whitespace().next().unwrap_or("").to_string();
    let mut line = format!(
        "   air: mesh packets {n_mesh} (advertisements {n_adv}); received: A {} B {}",
        first(r.b.ble_air_stats(a)),
        first(r.b.ble_air_stats(bb))
    );
    if let Some(cid) = c {
        line += &format!(" C {}", first(r.b.ble_air_stats(cid)));
    }
    r.cx.say(&line);
    let mut line = format!("   resets: A {} B {}", r.b.resets(a), r.b.resets(bb));
    if let Some(cid) = c {
        line += &format!(" C {}", r.b.resets(cid));
    }
    r.cx.say(&line);
    let cx = r.cx;
    let okn = cx.results.iter().filter(|x| x.1).count();
    let wall = cx.t_wall.elapsed().as_secs_f64();
    cx.say(&format!("\n== result: {okn}/{}; wall time {wall:.0} s", cx.results.len()));
    let rs = cx.results.clone();
    for (n, o, d) in rs {
        cx.say(&format!("  {}  {n}  {d}", if o { "PASS" } else { "FAIL" }));
    }
    Ok(())
}
