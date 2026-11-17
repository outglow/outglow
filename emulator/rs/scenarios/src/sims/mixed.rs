use super::util::{after, command, hex_list, hexo, key_confirmed, onoff, rel, times, verified};
use super::{node_of, Sim, SimCtx, SimDef};
use crate::air::NodeId;
use crate::coord::{Coord, HA, NWK_KEY};
use crate::frames::{fmt_ext, mac_parse};
use crate::image::Image;
use crate::session::R;

pub fn defs() -> Vec<SimDef> {
    vec![
        SimDef { name: "mixed_parent_rust", doc: "a bulb out of the coordinator's range joins through a Rust bulb (a stock one with --firmware stock)", run: parent_rust },
        SimDef { name: "mixed_parent_stock", doc: "a Rust bulb out of the coordinator's range joins through a stock bulb", run: parent_stock },
        SimDef { name: "mixed_mesh", doc: "two bulbs relay for each other: routes, many-to-one, source routes, broadcasts (Rust and stock with --firmware stock)", run: mesh },
        SimDef { name: "mixed_chain", doc: "a chain coordinator - S1 - R - S2 (S stock with --firmware stock): joins hop by hop, traffic both ways, route records", run: chain },
        SimDef { name: "mixed_rejoin", doc: "in the chain S1 - R - S2 the first bulb, then the last, lose power and come back", run: rejoin },
        SimDef { name: "mixed_group", doc: "groups and scenes on a group of two bulbs, groupcast (Rust and stock with --firmware stock)", run: group },
        SimDef { name: "mixed_ota_relay", doc: "Zigbee OTA of a Rust bulb whose frames all go through a stock router", run: ota_relay },
        SimDef { name: "mixed_conflict", doc: "two bulbs with the same short address, the conflict is resolved (Rust and stock with --firmware stock)", run: conflict },
    ]
}

pub const QUANTUM: f64 = 0.25;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Fw {
    Rust,
    Stock,
    C,
}

impl Fw {
    pub fn label(self) -> &'static str {
        match self {
            Fw::Rust => "rust",
            Fw::Stock => "stock",
            Fw::C => "c",
        }
    }
    pub fn default_image(self) -> &'static str {
        match self {
            Fw::Rust => "target/tc32/bulb-tc32.bin",
            Fw::Stock => "stock/stock-dimm.bin",
            Fw::C => "firmware/build/open-eglo.bin",
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Fw::Rust => "Rust",
            Fw::Stock => "stock",
            Fw::C => "C",
        }
    }
    pub fn of(img: &Image) -> Fw {
        match super::Firmware::of(img) {
            "Rust" => Fw::Rust,
            "C" => Fw::C,
            _ => Fw::Stock,
        }
    }
}

pub fn image(cx: &SimCtx, fw: Fw) -> Image {
    cx.image(fw.label(), fw.default_image())
}

pub const fn mac_record(v: [u8; 5]) -> [u8; 8] {
    [v[0], v[1], v[2], 0x38, 0xC1, 0xA4, v[3], v[4] & 0x7F]
}

pub const fn ieee_of(_fw: Fw, rec: &[u8; 8]) -> u64 {
    u64::from_le_bytes([rec[6], rec[7], rec[0], rec[1], rec[2], rec[3], rec[4], rec[5]])
}

#[derive(Clone)]
pub struct Bulb {
    pub id: NodeId,
    pub fw: Fw,
    pub name: String,
    pub ext: u64,
    pub img: Image,
}

impl Bulb {
    pub fn who(&self) -> String {
        format!("{} ({})", self.name, self.fw.title())
    }
}

pub type Spec = (Fw, &'static str, [u8; 5]);

pub fn build(cx: &SimCtx, specs: &[Spec]) -> R<(Sim, Vec<Bulb>)> {
    build_on(cx, Coord::new_plain(11, NWK_KEY), specs)
}

pub fn build_on(cx: &SimCtx, coord: Coord, specs: &[Spec]) -> R<(Sim, Vec<Bulb>)> {
    let mut sim: Option<Sim> = None;
    let mut coord = Some(coord);
    let mut bulbs = Vec::new();
    for (fw, name, v) in specs {
        let img = image(cx, *fw);
        let mut node = node_of(&img, cx.cpi)?;
        let rec = mac_record(*v);
        node.write_flash(0xFF000, &rec);
        let id = match sim.as_mut() {
            None => {
                sim = Some(Sim::new(node, name, coord.take().unwrap(), cx.log(), QUANTUM));
                NodeId::Dut
            }
            Some(s) => s.w.add_bulb(node, name),
        };
        bulbs.push(Bulb { id, fw: Fw::of(&img), name: name.to_string(), ext: ieee_of(*fw, &rec), img });
    }
    let s = sim.ok_or_else(|| crate::session::Stop::Error("no bulbs".into()))?;
    Ok((s, bulbs))
}

pub fn names(b: &[Bulb]) -> Vec<(NodeId, String)> {
    b.iter().map(|x| (x.id, x.name.clone())).collect()
}

pub fn finish(cx: &mut SimCtx, s: &Sim, b: &[Bulb]) {
    let n = names(b);
    let refs: Vec<(NodeId, &str)> = n.iter().map(|(i, s)| (*i, s.as_str())).collect();
    cx.finish(s, &refs);
}

pub fn describe(b: &[Bulb]) -> String {
    b.iter().map(|x| format!("{} = {} ({})", x.name, x.fw.title(), rel(&x.img))).collect::<Vec<_>>().join("; ")
}

pub fn neighbors(s: &mut Sim, short: u16) -> R<Option<Vec<u16>>> {
    let (mut out, mut start) = (Vec::new(), 0u8);
    loop {
        let n0 = s.w.coord.zcl_rsp.len();
        s.w.coord.zdo_send(&mut s.w.air, short, 0x0031, &[start]);
        let got = s.run_until(3000.0, 10.0, |s| s.w.coord.zcl_rsp[n0..].iter().any(|r| r.src == short && r.cluster == 0x8031))?;
        if !got {
            return Ok(None);
        }
        let p = s.w.coord.zcl_rsp[n0..].iter().find(|r| r.src == short && r.cluster == 0x8031).unwrap().body.clone();
        if p.len() < 5 || p[1] != 0 {
            return Ok(None);
        }
        let (total, count) = (p[2], p[4]);
        for i in 0..count as usize {
            let o = 5 + 22 * i + 16;
            if p.len() < o + 2 {
                break;
            }
            out.push(u16::from_le_bytes([p[o], p[o + 1]]));
        }
        start = start.wrapping_add(count);
        if start >= total || count == 0 {
            return Ok(Some(out));
        }
    }
}

pub fn relayed(s: &Sim, by: u16, of: u16, since: f64, to: Option<u16>) -> usize {
    let mut n = 0;
    for f in &s.w.air.frames {
        if f.t0 < since || f.psdu.len() < 14 {
            continue;
        }
        let Some(m) = mac_parse(&f.psdu) else { continue };
        if m.ty == 1 && m.src == Some(by as u64) && m.payload.len() >= 8 {
            let dst = u16::from_le_bytes([m.payload[2], m.payload[3]]);
            let src = u16::from_le_bytes([m.payload[4], m.payload[5]]);
            if src == of && to.map_or(true, |t| t == dst) {
                n += 1;
            }
        }
    }
    n
}

pub fn command2(s: &mut Sim, short: u16, cluster: u16, cmd: u8, payload: &[u8], max_ms: f64) -> R<Option<f64>> {
    match command(s, short, cluster, cmd, payload, max_ms)? {
        Some(d) => Ok(Some(d)),
        None => command(s, short, cluster, cmd, payload, max_ms),
    }
}

pub const SETTLE_MS: f64 = 1500.0;

pub fn light_cmd(s: &mut Sim, id: NodeId, short: u16, cluster: u16, cmd: u8, payload: &[u8], max_ms: f64)
                 -> R<(Option<f64>, crate::session::Light)> {
    let dt = command(s, short, cluster, cmd, payload, max_ms)?;
    s.run(SETTLE_MS)?;
    Ok((dt, s.light(id)))
}

pub fn concentrator(s: &mut Sim) -> R<()> {
    for _ in 0..2 {
        s.run(400.0)?;
        s.w.coord.many_to_one_route_request(&mut s.w.air, false);
    }
    s.run(2500.0)
}

pub fn records(s: &Sim, n0: usize, who: u16) -> Vec<Vec<u16>> {
    s.w.coord.route_records[n0..].iter().filter(|x| x.1 == who).map(|x| x.2.clone()).collect()
}

pub fn records_text(v: &[Vec<u16>]) -> String {
    format!("[{}]", v.iter().map(|x| hex_list(x)).collect::<Vec<_>>().join(", "))
}

pub fn bcast(s: &mut Sim, cmd: u8) {
    s.w.coord.zcl_send_with(&mut s.w.air, 0xFFFD, 1, 0x0006, cmd, &[], HA, true, true, None, false);
}

pub fn bcast_acked(s: &mut Sim, cmd: u8, relay: NodeId) -> R<()> {
    let st = &s.w.coord.st;
    let counters = (st.nwk_seq, st.aps_ctr, st.zcl_seq);
    let n0 = s.w.air.frames.len();
    bcast(s, cmd);
    let st = &s.w.coord.st;
    let (nwk_seq, zcl_seq) = (st.nwk_seq, st.zcl_seq);
    s.run(10.0)?;
    let id = s.w.air.frames[n0..].iter().filter(|f| f.who == NodeId::Coord).find_map(|f| {
        let m = mac_parse(&f.psdu)?;
        (m.dst == Some(0xFFFF) && m.ty == 1 && m.payload.len() >= 8).then(|| (u16::from_le_bytes([m.payload[4], m.payload[5]]), m.payload[7]))
    });
    let Some(id) = id else { return Ok(()) };
    for _ in 0..2 {
        let since = s.w.air.frames.len();
        s.run(500.0)?;
        let relayed = s.w.air.frames[since..].iter().any(|f| {
            f.who == relay
                && mac_parse(&f.psdu).is_some_and(|m| m.ty == 1 && m.payload.len() >= 8
                    && (u16::from_le_bytes([m.payload[4], m.payload[5]]), m.payload[7]) == id)
        });
        if relayed {
            break;
        }
        let st = &mut s.w.coord.st;
        let now = (st.nwk_seq, st.aps_ctr, st.zcl_seq);
        (st.nwk_seq, st.aps_ctr, st.zcl_seq) = counters;
        s.w.coord.zcl_send_with(&mut s.w.air, 0xFFFD, 1, 0x0006, cmd, &[], HA, true, true, None, false);
        let st = &mut s.w.coord.st;
        debug_assert_eq!((st.nwk_seq, st.zcl_seq), (nwk_seq, zcl_seq));
        (st.nwk_seq, st.aps_ctr, st.zcl_seq) = now;
    }
    Ok(())
}

pub fn join_all(s: &mut Sim, b: &[Bulb], max_ms: f64) -> R<Option<Vec<u16>>> {
    let exts: Vec<u64> = b.iter().map(|x| x.ext).collect();
    let ok = s.run_until(max_ms, 10.0, |s| exts.iter().all(|e| s.announced(*e).is_some()))?;
    Ok(if ok { Some(exts.iter().map(|e| s.announced(*e).unwrap()).collect()) } else { None })
}

pub fn shorts_text(s: &Sim, b: &[Bulb]) -> String {
    b.iter().map(|x| format!("{}={}", x.name, hexo(s.announced(x.ext)))).collect::<Vec<_>>().join(" ")
}

pub fn tail_events(s: &Sim, n: usize) -> String {
    let ev: Vec<String> = s.w.coord.events.iter().map(|e| format!("'{}'", e.kind)).collect();
    let devs: Vec<String> =
        s.w.coord.devices.iter().map(|d| format!("{} {:#06x}{}", fmt_ext(d.ext), d.short, if d.announced { "" } else { " (not announced)" })).collect();
    format!("[{}]; devices: [{}]", ev[ev.len().saturating_sub(n)..].join(", "), devs.join(", "))
}

pub fn resets_text(s: &Sim, b: &[Bulb]) -> (bool, String) {
    let r: Vec<u32> = b.iter().map(|x| s.w.bulb_resets(x.id)).collect();
    (r.iter().all(|x| *x == 0), b.iter().zip(&r).map(|(x, n)| format!("{}={n}", x.name)).collect::<Vec<_>>().join(" "))
}

fn parent_rust(cx: &mut SimCtx) -> R<()> {
    parent(cx, Fw::Rust, Fw::Stock)
}

fn parent_stock(cx: &mut SimCtx) -> R<()> {
    parent(cx, Fw::Stock, Fw::Rust)
}

fn parent(cx: &mut SimCtx, pfw: Fw, jfw: Fw) -> R<()> {
    let (mut s, b) = build(cx, &[(pfw, "P", [0x01, 0x00, 0x52, 0x00, 0x00]), (jfw, "J", [0x02, 0x00, 0x54, 0x00, 0x00])])?;
    let (p, j) = (b[0].clone(), b[1].clone());
    s.w.set_t_off(j.id, 8000.0);
    s.w.air.block(NodeId::Coord, j.id, true);
    s.w.coord.aps_retry = true;
    s.w.coord.start(&mut s.w.air);
    cx.say(&format!("== parent P: {}; joiner J: {}; channel 11", p.fw.title(), j.fw.title()));
    cx.say(&format!("   {}", describe(&b)));

    let ok = s.run_until(15000.0, 10.0, |s| s.announced(p.ext).is_some())?;
    let ps = s.announced(p.ext);
    if !cx.check(s.now(), &format!("the {} bulb P joins at the coordinator", p.fw.title()), ok, &format!("P={}", hexo(ps))) {
        cx.say(&format!("   coordinator events: {}", tail_events(&s, 12)));
        finish(cx, &s, &b);
        return Ok(());
    }
    let ps = ps.unwrap();
    s.run_until(8000.0, 10.0, |s| verified(s) > 0)?;
    s.run(200.0)?;
    s.w.coord.permit_joining(&mut s.w.air, 180);
    let ok = s.run_until(60000.0, 10.0, |s| s.announced(j.ext).is_some())?;
    let js = s.announced(j.ext);
    let now = s.now();
    if !cx.check(now, &format!("the {} bulb J, hearing only P, joins through it", j.fw.title()), ok,
                 &format!("J={}, {:.1} s after its power-on", hexo(js), (now - 8000.0) / 1000.0)) {
        cx.say(&format!("   coordinator events: {}", tail_events(&s, 12)));
        finish(cx, &s, &b);
        return Ok(());
    }
    let js = js.unwrap();
    let upd: Vec<_> = s.events("update-device").into_iter().filter(|e| e.get("ext") == Some(fmt_ext(j.ext).as_str())).collect();
    let shown = upd.first().map(|e| e.details.clone()).unwrap_or_else(|| "none".into());
    let good = upd.first().is_some_and(|e| e.get("router") == Some(format!("{ps:#06x}").as_str()) && e.get("status") == Some("1"));
    cx.check(s.now(), "P reported the newcomer to the Trust Center (APS Update-Device, status 1: standard unsecured join)", good, &shown);
    let frames = &s.w.air.frames;
    let mut polls: Vec<Option<bool>> = Vec::new();
    for (i, f) in frames.iter().enumerate() {
        if f.who != j.id || f.psdu.len() < 5 {
            continue;
        }
        let Some(m) = mac_parse(&f.psdu) else { continue };
        if m.ty == 3 && m.payload.first() == Some(&0x04) && m.smode == 3 {
            let ack = frames[i + 1..(i + 3).min(frames.len())].iter().find(|x| x.who == p.id && x.psdu.len() == 5 && x.psdu[2] == m.seq);
            polls.push(ack.map(|a| a.psdu[0] & 0x10 != 0));
        }
    }
    let pshow = format!(
        "[{}]",
        polls.iter().map(|p| match p { None => "None", Some(true) => "True", Some(false) => "False" }).collect::<Vec<_>>().join(", ")
    );
    cx.check(s.now(), "J's poll for the association response is acknowledged with the frame-pending bit (802.15.4 6.3.2)",
             polls.contains(&Some(true)), &format!("pending bits of the ACKs: {pshow}"));
    let v0 = verified(&s);
    s.run_until(12000.0, 10.0, |s| verified(s) >= 2)?;
    cx.check(s.now(), "J's link key exchange with the Trust Center (BDB 10.2.5) works through P", verified(&s) >= 2,
             &format!("verified link keys: {} (before J's join {v0})", verified(&s)));
    let (dt, l) = light_cmd(&mut s, j.id, js, 0x0006, 0x00, &[], 8000.0)?;
    let off = !l.on;
    cx.check(s.now(), "Off reaches J through its parent, and the answer comes back", dt.is_some() && off, &after(dt, "no default response"));
    let (dt, l) = light_cmd(&mut s, j.id, js, 0x0006, 0x01, &[], 3000.0)?;
    let on = l.on;
    cx.check(s.now(), "On as well", dt.is_some() && on, &after(dt, "no default response"));
    s.run(34000.0)?;
    let mut tt = Vec::new();
    for _ in 0..4 {
        tt.push(command(&mut s, js, 0x0006, 0x02, &[], 3000.0)?);
    }
    cx.check(s.now(), "still reachable after the neighbor tables have settled (four more commands)", tt.iter().all(|x| x.is_some()), &times(&tt));
    let lq = neighbors(&mut s, ps)?;
    cx.check(s.now(), "P lists J among its neighbours (Mgmt_Lqi)", lq.as_ref().is_some_and(|v| v.contains(&js)),
             &format!("P's neighbours: {}", hex_list(lq.as_deref().unwrap_or(&[]))));
    let (dt, l) = light_cmd(&mut s, p.id, ps, 0x0006, 0x00, &[], 3000.0)?;
    let off = !l.on;
    cx.check(s.now(), "P itself still answers", dt.is_some() && off, "");
    let (ok, rs) = resets_text(&s, &b);
    cx.check(s.now(), "no resets, nobody left the network", ok && s.events("leave").is_empty(), &format!("resets {rs}"));
    finish(cx, &s, &b);
    Ok(())
}

fn mesh(cx: &mut SimCtx) -> R<()> {
    let (mut s, b) = build(cx, &[(Fw::Rust, "R", [0x01, 0x00, 0x52, 0x00, 0x00]), (Fw::Stock, "S", [0x02, 0x00, 0x54, 0x00, 0x00])])?;
    let (rb, sb) = (b[0].clone(), b[1].clone());
    let (rid, tid) = (rb.id, sb.id);
    s.w.set_t_off(tid, 900.0);
    s.w.air.block(NodeId::Coord, tid, true);
    s.w.coord.start(&mut s.w.air);
    cx.say(&format!("== {}; channel 11; S out of the coordinator's range from the start", describe(&b)));

    let Some(sh) = join_all(&mut s, &b, 60000.0)? else {
        cx.check(s.now(), "both bulbs join the network", false, &shorts_text(&s, &b));
        finish(cx, &s, &b);
        return Ok(());
    };
    cx.check(s.now(), "both bulbs join the network", true, &shorts_text(&s, &b));
    let (r, t) = (sh[0], sh[1]);
    s.run(8000.0)?;
    cx.check(s.now(), "both complete the Trust Center link key exchange", verified(&s) >= 2, "");

    s.run(34000.0)?;
    let nr = neighbors(&mut s, r)?;
    let nt = neighbors(&mut s, t)?;
    cx.check(s.now(), &format!("{}, S's parent, lists {} and the coordinator as neighbours", rb.who(), sb.who()),
             nr.as_ref().is_some_and(|v| v.contains(&t) && v.contains(&0)), &format!("R's neighbours: {}", hex_list(nr.as_deref().unwrap_or(&[]))));
    cx.check(s.now(), &format!("{} lists {} as a neighbour", sb.who(), rb.who()), nt.as_ref().is_some_and(|v| v.contains(&r)),
             &format!("S's neighbours: {}", hex_list(nt.as_deref().unwrap_or(&[]))));

    for (far, near) in [(&sb, &rb), (&rb, &sb)] {
        let (fs, ns) = (sh[if far.id == rid { 0 } else { 1 }], sh[if near.id == rid { 0 } else { 1 }]);
        cx.say(&format!("-- {} out of the coordinator's range: everything goes through {}", far.who(), near.who()));
        if far.id == rid {
            s.w.air.block(NodeId::Coord, near.id, false);
            s.w.air.block(NodeId::Coord, far.id, true);
            s.run(50000.0)?;
        }
        s.w.coord.st.via.insert(fs, ns);
        let t0 = s.now();
        let (dt, l) = light_cmd(&mut s, far.id, fs, 0x0006, 0x00, &[], 20000.0)?;
        if std::env::var_os("MIXED_DEBUG").is_some() {
            debug_frames(cx, &s, &b, t0);
        }
        let off = !l.on;
        cx.check(s.now(), &format!("Off reaches {} through {} and the answer finds its way back", far.who(), near.who()),
                 dt.is_some() && off, &after(dt, "no default response"));
        let n = relayed(&s, ns, fs, t0, None);
        cx.check(s.now(), &format!("{} relayed {}'s frames", near.who(), far.who()), n >= 1, &format!("{n} frame(s)"));
        let mut tt = Vec::new();
        for _ in 0..6 {
            tt.push(command2(&mut s, fs, 0x0006, 0x02, &[], 3000.0)?);
        }
        cx.check(s.now(), "six more commands over the established route, each answered within 3 s (one APS-like retry)", tt.iter().all(|x| x.is_some()), &times(&tt));
        let n0 = s.w.coord.route_records.len();
        let t0 = s.now();
        concentrator(&mut s)?;
        let n = relayed(&s, ns, 0x0000, t0, Some(0xFFFC));
        cx.check(s.now(), &format!("{} relays the coordinator's many-to-one route request", near.who()), n >= 1,
                 &format!("{n} relayed frame(s)"));
        let dt = command(&mut s, fs, 0x0006, 0x01, &[], 3000.0)?;
        let rec = records(&s, n0, fs);
        cx.check(s.now(), &format!("{}'s Route Record arrives with {} added as relay", far.who(), near.who()),
                 dt.is_some() && rec.iter().any(|x| *x == [ns]), &format!("records from {}: {}", far.name, records_text(&rec)));
        let dt = command(&mut s, ns, 0x0006, 0x02, &[], 3000.0)?;
        let own = s.w.coord.route_records[n0..].iter().any(|x| x.1 == ns && x.2.is_empty());
        cx.check(s.now(), &format!("{} sends its own Route Record (no relays) before its answer", near.who()), dt.is_some() && own,
                 &records_text(&records(&s, n0, ns)));
        s.w.coord.st.via.remove(&fs);
        s.w.coord.source_routes.insert(fs, vec![ns]);
        let (dt, l) = light_cmd(&mut s, far.id, fs, 0x0006, 0x00, &[], 3000.0)?;
        let off = !l.on;
        cx.check(s.now(), &format!("a source-routed frame to {} is relayed by {}", far.who(), near.who()),
                 dt.is_some() && off, &dt.map(|d| format!("answered after {d:.0} ms")).unwrap_or_else(|| "no answer".into()));
        s.w.coord.source_routes.remove(&fs);
    }

    s.w.coord.st.via.insert(r, t);
    command2(&mut s, r, 0x0006, 0x00, &[], 3000.0)?;
    command2(&mut s, t, 0x0006, 0x00, &[], 3000.0)?;
    s.run(SETTLE_MS)?;
    let was_off = !s.light(rid).on && !s.light(tid).on;
    s.run(300.0)?;
    bcast_acked(&mut s, 0x01, tid)?;
    s.run(3000.0)?;
    let (lr, lt) = (s.light(rid).on, s.light(tid).on);
    cx.check(s.now(), &format!("a broadcast On is relayed by {} to {}", sb.who(), rb.who()), was_off && lr && lt, &format!("R {}, S {}", onoff(lr), onoff(lt)));
    s.w.coord.st.via.remove(&r);
    s.w.air.block(NodeId::Coord, rid, false);
    s.w.air.block(NodeId::Coord, tid, true);
    s.run(34000.0)?;
    bcast_acked(&mut s, 0x00, rid)?;
    s.run(3000.0)?;
    let (lr, lt) = (s.light(rid).on, s.light(tid).on);
    cx.check(s.now(), &format!("a broadcast Off is relayed by {} to {}", rb.who(), sb.who()), !lr && !lt, &format!("R {}, S {}", onoff(lr), onoff(lt)));
    bcast_acked(&mut s, 0x02, rid)?;
    s.run(3000.0)?;
    let (lr, lt) = (s.light(rid).on, s.light(tid).on);
    cx.check(s.now(), "a broadcast Toggle switches both bulbs exactly once", lr && lt, &format!("R {}, S {}", onoff(lr), onoff(lt)));
    let (ok, rs) = resets_text(&s, &b);
    cx.check(s.now(), "no resets, nobody left the network", ok && s.events("leave").is_empty(), &format!("resets {rs}"));
    finish(cx, &s, &b);
    Ok(())
}

pub fn debug_frames(cx: &mut SimCtx, s: &Sim, b: &[Bulb], since: f64) {
    for f in &s.w.air.frames {
        if f.t0 < since {
            continue;
        }
        let who = b.iter().find(|x| x.id == f.who).map(|x| x.name.clone()).unwrap_or_else(|| s.w.air.name_of(f.who));
        let Some(m) = mac_parse(&f.psdu) else { continue };
        let nwk = if m.ty == 1 && m.payload.len() >= 8 {
            let p = &m.payload;
            format!(" nwk fc={:#06x} {:#06x}->{:#06x} r={} seq={}", u16::from_le_bytes([p[0], p[1]]), u16::from_le_bytes([p[4], p[5]]),
                    u16::from_le_bytes([p[2], p[3]]), p[6], p[7])
        } else {
            String::new()
        };
        cx.say(&format!("   [{:9.4}-{:9.4}] {who:>6} ty={} seq={} {:?}->{:?}{nwk}", f.t0 / 1000.0, f.t1 / 1000.0, m.ty, m.seq, m.src, m.dst));
    }
}

fn dropped_by_driver(s: &Sim, x: &Bulb) -> Option<Vec<String>> {
    let (irq, size) = x.img.symbol_sized("irq_handler")?;
    let irq = irq & !1;
    let off = match x.id {
        NodeId::Bulb(i) => s.w.air.bulbs[i as usize].t_off,
        _ => s.w.air.t_off,
    };
    let log = s.w.node(x.id).radio.b().rx_log.clone().unwrap_or_default();
    Some(log.iter().filter_map(|e| {
        let at = (e.t1 + off) / 1000.0;
        match (e.fate, e.cleared) {
            ("overwritten", _) => Some(format!("{at:.4} s: {} bytes overwritten", e.len)),
            ("received", Some((_, pc))) if !(irq..irq + size.max(2)).contains(&pc.wrapping_sub(2)) => {
                Some(format!("{at:.4} s: {} bytes cleared at {pc:#x}", e.len))
            }
            _ => None,
        }
    }).collect())
}

type PcLog = std::rc::Rc<std::cell::RefCell<Vec<(f64, String)>>>;
thread_local! {
    static WATCH: std::cell::RefCell<Option<PcLog>> = const { std::cell::RefCell::new(None) };
}

fn fates_watch(s: &mut Sim, who: NodeId) {
    let log = fates_watch_on(s, who);
    WATCH.with(|w| *w.borrow_mut() = log);
}
fn fates_watch_on(s: &mut Sim, who: NodeId) -> Option<PcLog> {
    std::env::var_os("MIXED_FATES")?;
    for id in s.w.air.nodes.clone() {
        if matches!(id, NodeId::Dut | NodeId::Bulb(_)) {
            s.w.node(id).radio.b().rx_log = Some(Vec::new());
        }
    }
    let log: PcLog = Default::default();
    for item in std::env::var("MIXED_HOOKS").unwrap_or_default().split(',').filter(|x| !x.is_empty()) {
        let Some((name, addr)) = item.split_once('=') else { continue };
        let (addr, ram) = match addr.split_once(':') {
            Some((a, r)) => (a, u32::from_str_radix(r.trim_start_matches("0x"), 16).ok()),
            None => (addr, None),
        };
        let Ok(a) = u32::from_str_radix(addr.trim_start_matches("0x"), 16) else { continue };
        let (l, name) = (log.clone(), name.to_string());
        s.w.node_mut(who).m.hooks.insert(a & !1, std::rc::Rc::new(move |m: &mut tc32emu::Machine| {
            let what = match ram {
                Some(r) => {
                    let o = (r - 0x840000) as usize;
                    let v = u32::from_le_bytes(m.sram[o..o + 4].try_into().unwrap());
                    format!("{name}[{v:#x} t={:#x}]", m.stimer_now())
                }
                None => name.clone(),
            };
            l.borrow_mut().push((m.ms(), what));
            Ok(())
        }));
    }
    Some(log)
}

fn fates_dump(cx: &mut SimCtx, s: &Sim, b: &[Bulb], who: NodeId) {
    let Some(pcs) = WATCH.with(|w| w.borrow().clone()) else { return };
    let window = std::env::var("MIXED_AIR").ok().and_then(|v| v.split_once(',').and_then(|(a, z)| Some((a.parse::<f64>().ok()?, z.parse::<f64>().ok()?))));
    if let Some((a, z)) = window {
        for f in s.w.air.frames.iter().filter(|f| f.t0 >= a * 1000.0 && f.t0 <= z * 1000.0) {
            let who = b.iter().find(|x| x.id == f.who).map(|x| x.name.clone()).unwrap_or_else(|| s.w.air.name_of(f.who));
            let Some(m) = mac_parse(&f.psdu) else { continue };
            let nwk = if m.ty == 1 && m.payload.len() >= 8 {
                let p = &m.payload;
                format!(" nwk fc={:#06x} {:#06x}->{:#06x} r={} seq={}", u16::from_le_bytes([p[0], p[1]]), u16::from_le_bytes([p[4], p[5]]),
                        u16::from_le_bytes([p[2], p[3]]), p[6], p[7])
            } else {
                String::new()
            };
            cx.say(&format!("   AIR [{:9.4}-{:9.4}] {who:>6} ty={} seq={} {:?}->{:?}{nwk}", f.t0 / 1000.0, f.t1 / 1000.0, m.ty, m.seq, m.src, m.dst));
        }
        for e in s.w.coord.events.iter().filter(|e| e.ms >= a * 1000.0 && e.ms <= z * 1000.0) {
            cx.say(&format!("   COORD [{:9.4}] {} {}", e.ms / 1000.0, e.kind, e.details));
        }
    }
    let off = match who {
        NodeId::Bulb(i) => s.w.air.bulbs[i as usize].t_off,
        _ => s.w.air.t_off,
    };
    for x in b.iter().filter(|x| x.id != who) {
        let off = match x.id {
            NodeId::Bulb(i) => s.w.air.bulbs[i as usize].t_off,
            _ => s.w.air.t_off,
        };
        for e in s.w.node(x.id).radio.b().rx_log.clone().unwrap_or_default() {
            let t0 = (e.t0 + off) / 1000.0;
            let shown = window.is_some_and(|(a, z)| t0 >= a && t0 <= z) && e.fate != "other channel";
            if (e.fate != "received" && e.fate != "other channel") || shown {
                let cleared = e.cleared.map(|(t, pc)| format!(" cleared {:.4} at {pc:#x}", (t + off) / 1000.0)).unwrap_or_default();
                cx.say(&format!("   FATE {} [{:9.4}-{:9.4}] len {:3} {}{cleared}", x.name, t0, (e.t1 + off) / 1000.0, e.len, e.fate));
            }
        }
    }
    let irq_pc = std::env::var("MIXED_IRQ_PC").ok().and_then(|x| u32::from_str_radix(x.trim_start_matches("0x"), 16).ok());
    let log = s.w.node(who).radio.b().rx_log.clone().unwrap_or_default();
    let pcs = pcs.borrow();
    for e in &log {
        let taken = e.fate == "received" && e.cleared.is_some_and(|(_, pc)| Some(pc) == irq_pc);
        if taken {
            continue;
        }
        let t0 = e.t0 + off;
        let f = s.w.air.frames.iter().find(|f| (f.t0 - t0).abs() < 0.01 && f.psdu.len() == e.len);
        let what = f.map(|f| {
            let name = b.iter().find(|x| x.id == f.who).map(|x| x.name.clone()).unwrap_or_else(|| s.w.air.name_of(f.who));
            match mac_parse(&f.psdu) {
                Some(m) => {
                    let nwk = if m.ty == 1 && m.payload.len() >= 8 {
                        let p = &m.payload;
                        format!(" nwk {:#06x}->{:#06x} r={} seq={}", u16::from_le_bytes([p[4], p[5]]), u16::from_le_bytes([p[2], p[3]]), p[6], p[7])
                    } else {
                        String::new()
                    };
                    format!("{name} ty={} seq={} {:?}->{:?}{nwk}", m.ty, m.seq, m.src, m.dst)
                }
                None => name,
            }
        }).unwrap_or_default();
        let cleared = e.cleared.map(|(t, pc)| format!(" cleared {:.4} at {pc:#x}", (t + off) / 1000.0)).unwrap_or_default();
        cx.say(&format!("   FATE [{:9.4}-{:9.4}] len {:3} {}{cleared}: {what}", t0 / 1000.0, (e.t1 + off) / 1000.0, e.len, e.fate));
        let around: Vec<String> = pcs.iter().filter(|(t, _)| *t > e.t0 - 15.0 && *t < e.t1 + 3.0)
            .map(|(t, n)| format!("{:.4} {n}", (t + off) / 1000.0)).collect();
        if !around.is_empty() {
            cx.say(&format!("        {}", around.join(", ")));
        }
    }
}

fn chain_setup(cx: &mut SimCtx) -> R<(Sim, Vec<Bulb>, Option<Vec<u16>>)> {
    let mut coord = Coord::new_plain(11, NWK_KEY);
    coord.nwk_discover = 1;
    let (mut s, b) = build_on(cx, coord, &[
        (Fw::Stock, "S1", [0x11, 0x00, 0x60, 0x00, 0x00]),
        (Fw::Rust, "R", [0x12, 0x00, 0x61, 0x00, 0x00]),
        (Fw::Stock, "S2", [0x13, 0x00, 0x62, 0x00, 0x00]),
    ])?;
    let (s1, r, s2) = (b[0].id, b[1].id, b[2].id);
    s.w.air.block(NodeId::Coord, r, true);
    s.w.air.block(NodeId::Coord, s2, true);
    s.w.air.block(s1, s2, true);
    s.w.set_t_off(r, 12000.0);
    s.w.set_t_off(s2, std::env::var("MIXED_S2_ON").ok().and_then(|v| v.parse().ok()).unwrap_or(20000.0));
    if std::env::var_os("MIXED_DEBUG").is_some() {
        for x in &b {
            s.w.node(x.id).radio.b().rx_log = Some(Vec::new());
        }
    }
    fates_watch(&mut s, r);
    s.w.node(r).radio.b().rx_log.get_or_insert_with(Vec::new);
    s.w.coord.start(&mut s.w.air);
    cx.say(&format!("== coordinator - S1 - R - S2: {}; channel 11", describe(&b)));
    let ok = s.run_until(12000.0, 10.0, |s| s.announced(b[0].ext).is_some())?;
    if !cx.check(s.now(), &format!("{} joins at the coordinator", b[0].who()), ok, &shorts_text(&s, &b)) {
        return Ok((s, b, None));
    }
    s.run(300.0)?;
    s.w.coord.permit_joining(&mut s.w.air, 180);
    let s1_short = s.announced(b[0].ext).unwrap();
    let end = s.now() + 60000.0;
    while s.now() < end && !b.iter().all(|x| s.announced(x.ext).is_some()) {
        s.run(20.0)?;
        if let Some(d) = s.w.coord.devices.iter().find(|d| d.ext == b[2].ext).map(|d| d.short) {
            s.w.coord.st.via.insert(d, s1_short);
        }
        if std::env::var_os("MIXED_DEBUG").is_some() && (29300.0..29320.0).contains(&s.now()) {
            let t_off = s.w.air.bulbs.get(0).map(|x| x.t_off).unwrap_or(0.0);
            for r in super::stress::account::ring_records(s.w.node(b[1].id), &b[1].img, t_off) {
                if r.t > 29000.0 {
                    cx.say(&format!("   R trace {:.4}: {} seq={} src={:#06x} peer={:#06x} aux={}", r.t / 1000.0, super::stress::account::describe(&r), r.seq, r.src, r.peer, r.aux));
                }
            }
        }
    }
    let Some(sh) = join_all(&mut s, &b, 0.0)? else {
        cx.check(s.now(), &format!("{} and {} join, each through its neighbour in the chain", b[1].who(), b[2].who()), false,
                 &format!("{}; coordinator events: {}", shorts_text(&s, &b), tail_events(&s, 10)));
        fates_dump(cx, &s, &b, r);
        if std::env::var_os("MIXED_DEBUG").is_some() {
            debug_frames(cx, &s, &b, 24000.0);
            if let Some(a) = b[1].img.symbol("RADIO_DROPPED") {
                let m = &s.w.node(b[1].id).m.sram;
                let o = (a - 0x840000) as usize;
                cx.say(&format!("   R: frames lost to a full receive queue: {}", u32::from_le_bytes(m[o..o + 4].try_into().unwrap())));
            }
            for x in &b {
                let log = s.w.node(x.id).radio.b().rx_log.clone().unwrap_or_default();
                for e in log.iter().filter(|e| e.fate != "received" && e.t0 > 24000.0) {
                    cx.say(&format!("   fate {}: {:.4}-{:.4} len {} {}", x.name, e.t0 / 1000.0, e.t1 / 1000.0, e.len, e.fate));
                }
            }
        }
        return Ok((s, b, None));
    };
    let upd = |s: &Sim, ext: u64| -> Option<u16> {
        s.events("update-device").into_iter().find(|e| e.get("ext") == Some(fmt_ext(ext).as_str()))
            .and_then(|e| e.get("router").and_then(|x| u16::from_str_radix(x.trim_start_matches("0x"), 16).ok()))
    };
    let (ur, us2) = (upd(&s, b[1].ext), upd(&s, b[2].ext));
    cx.check(s.now(), &format!("{} joins through S1 and {} through R: the Update-Device comes from the parent (R22 4.6.3.2)", b[1].who(), b[2].who()),
             ur == Some(sh[0]) && us2 == Some(sh[1]),
             &format!("{}; R reported by {}, S2 by {}", shorts_text(&s, &b), hexo(ur), hexo(us2)));
    let confirmed = |s: &Sim| -> Vec<bool> { sh.iter().map(|&x| key_confirmed(s, x)).collect() };
    s.run_until(15000.0, 10.0, |s| verified(s) >= 3 && confirmed(s).iter().all(|x| *x))?;
    let got = confirmed(&s);
    cx.check(s.now(), "the three complete the link key exchange with the Trust Center, R and S2 over the chain: \
              each has its Confirm Key (BDB 3.0.1 10.2.5)", verified(&s) >= 3 && got.iter().all(|x| *x),
             &format!("verified link keys: {}; Confirm Key received by {}", verified(&s),
                      b.iter().zip(&got).filter(|x| *x.1).map(|x| x.0.name.as_str()).collect::<Vec<_>>().join(", ")));
    s.w.coord.st.via.insert(sh[1], sh[0]);
    s.w.coord.st.via.insert(sh[2], sh[0]);
    Ok((s, b, Some(sh)))
}

fn chain(cx: &mut SimCtx) -> R<()> {
    let (mut s, b, sh) = chain_setup(cx)?;
    let Some(sh) = sh else {
        finish(cx, &s, &b);
        return Ok(());
    };
    let (s1, r, s2) = (sh[0], sh[1], sh[2]);
    for (k, what) in [(2usize, format!("S2 ({}, three hops: S1, R)", b[2].fw.title())), (1, format!("R ({}, two hops: S1)", b[1].fw.title()))] {
        let dt = command2(&mut s, sh[k], 0x0006, 0x00, &[], 5000.0)?;
        s.run(SETTLE_MS)?;
        let l = s.light(b[k].id);
        cx.check(s.now(), &format!("Off reaches {what} and its answer comes back up the chain"), dt.is_some() && !l.on,
                 &after(dt, "no default response"));
        let dt = command2(&mut s, sh[k], 0x0006, 0x01, &[], 5000.0)?;
        s.run(SETTLE_MS)?;
        let l = s.light(b[k].id);
        cx.check(s.now(), "and On", dt.is_some() && l.on, &after(dt, "no default response"));
    }
    let t0 = s.now();
    let mut tt = Vec::new();
    for _ in 0..6 {
        let t1 = s.now();
        tt.push(command2(&mut s, s2, 0x0006, 0x02, &[], 3000.0)?);
        if tt.last() == Some(&None) && std::env::var_os("MIXED_DEBUG").is_some() {
            debug_frames(cx, &s, &b, t1);
        }
    }
    cx.check(s.now(), "six Toggles to S2, each answered within 3 s (sent once more if not)", tt.iter().all(|x| x.is_some()), &times(&tt));
    let (down, up) = (relayed(&s, r, 0x0000, t0, Some(s2)), relayed(&s, r, s2, t0, Some(0x0000)));
    let (down1, up1) = (relayed(&s, s1, 0x0000, t0, Some(s2)), relayed(&s, s1, s2, t0, Some(0x0000)));
    cx.check(s.now(), &format!("both relays carried them both ways (R: {}, S1: {})", b[1].fw.title(), b[0].fw.title()), down >= 6 && up >= 6 && down1 >= 6 && up1 >= 6,
             &format!("R relayed {down} down, {up} up; S1 {down1} down, {up1} up"));
    let n0 = s.w.coord.route_records.len();
    concentrator(&mut s)?;
    let dt2 = command2(&mut s, s2, 0x0006, 0x02, &[], 3000.0)?;
    let dtr = command2(&mut s, r, 0x0006, 0x02, &[], 3000.0)?;
    let (rec2, recr) = (records(&s, n0, s2), records(&s, n0, r));
    cx.check(s.now(), "after a many-to-one route request S2's Route Record lists R then S1, R's lists S1",
             dt2.is_some() && dtr.is_some() && rec2.iter().any(|x| *x == [r, s1]) && recr.iter().any(|x| *x == [s1]),
             &format!("records from S2: {}, from R: {}", records_text(&rec2), records_text(&recr)));
    s.w.coord.source_routes.insert(s2, vec![r, s1]);
    let dt = command2(&mut s, s2, 0x0006, 0x00, &[], 5000.0)?;
    s.run(SETTLE_MS)?;
    let l = s.light(b[2].id);
    cx.check(s.now(), "a frame source-routed over S1 and R reaches S2 (each relay takes its turn in the relay list)", dt.is_some() && !l.on,
             &after(dt, "no answer"));
    s.w.coord.source_routes.remove(&s2);
    s.run(300.0)?;
    bcast_acked(&mut s, 0x01, b[0].id)?;
    s.run(3000.0)?;
    let on: Vec<bool> = b.iter().map(|x| s.light(x.id).on).collect();
    cx.check(s.now(), "a broadcast On reaches the whole chain", on.iter().all(|x| *x),
             &b.iter().zip(&on).map(|(x, o)| format!("{} {}", x.name, onoff(*o))).collect::<Vec<_>>().join(", "));
    if let Some(lost) = dropped_by_driver(&s, &b[1]) {
        cx.check(s.now(), "R's driver (Rust) takes every frame its radio received: none cleared unread before a \
                  transmission or a BLE event, none overwritten by the next one before its interrupt ran",
                 lost.is_empty(), &if lost.is_empty() { "none lost".to_string() } else { lost.join("; ") });
    }
    let (ok, rs) = resets_text(&s, &b);
    cx.check(s.now(), "no resets, nobody left the network", ok && s.events("leave").is_empty(), &format!("resets {rs}"));
    fates_dump(cx, &s, &b, b[1].id);
    finish(cx, &s, &b);
    Ok(())
}

fn rejoin(cx: &mut SimCtx) -> R<()> {
    let (mut s, b, sh) = chain_setup(cx)?;
    let Some(sh) = sh else {
        finish(cx, &s, &b);
        return Ok(());
    };
    let assoc = |s: &Sim| s.events("association-request").len();
    let reach = |s: &mut Sim, short: u16, max_ms: f64| -> R<Option<f64>> {
        let t0 = s.now();
        while s.now() < t0 + max_ms {
            if command(s, short, 0x0006, 0x02, &[], 3000.0)?.is_some() {
                return Ok(Some((s.now() - t0) / 1000.0));
            }
        }
        Ok(None)
    };
    let took = |s: &Sim, x: Option<f64>, j: usize| match x {
        Some(t) => format!("after {t:.1} s"),
        None => match s.announced(b[j].ext).filter(|&a| a != sh[j]) {
            Some(a) => format!("no answer; it announced itself again as {a:#06x}"),
            None => "no answer".into(),
        },
    };
    let updates = |s: &Sim| s.events("update-device").len();
    let u0 = updates(&s);
    for (k, off_ms, behind) in [(0usize, 60000.0, vec![1usize, 2]), (2, 30000.0, vec![])] {
        let who = format!("{} ({})", b[k].name, b[k].fw.title());
        cx.say(&format!("-- {who} loses power for {:.0} s", off_ms / 1000.0));
        s.w.power_off(b[k].id);
        s.run(off_ms / 2.0)?;
        for &j in &behind {
            let dt = command(&mut s, sh[j], 0x0006, 0x02, &[], 3000.0)?;
            cx.check(s.now(), &format!("meanwhile {} behind it is out of reach (no other path)", b[j].name), dt.is_none(), &after(dt, "no answer"));
        }
        s.run(off_ms / 2.0)?;
        let a0 = assoc(&s);
        s.w.power_on(b[k].id).map_err(super::emu)?;
        s.run(3000.0)?;
        let l = s.light(b[k].id);
        cx.check(s.now(), &format!("{who} back: its light comes on (power-on behaviour of both firmwares)"), l.on, &format!("duty {:.1} %", l.duty));
        let dt = reach(&mut s, sh[k], 30000.0)?;
        cx.check(s.now(), &format!("{who} answers again with its address from before"), dt.is_some(), &took(&s, dt, k));
        cx.check(s.now(), &format!("{who} resumed from its NV: no association"), assoc(&s) == a0, &format!("associations {}", assoc(&s) - a0));
        for &j in &behind {
            let dt = reach(&mut s, sh[j], 30000.0)?;
            cx.check(s.now(), &format!("{} behind it is reachable again", b[j].name), dt.is_some(), &took(&s, dt, j));
        }
    }
    let leaves = s.events("leave").len();
    cx.check(s.now(), "nobody left the network", leaves == 0, &format!("{leaves} Leave"));
    let again = updates(&s) - u0;
    cx.check(s.now(), "nobody joined again (no Update-Device since the setup)", again == 0, &format!("{again} Update-Device"));
    finish(cx, &s, &b);
    Ok(())
}

pub fn groupcast(s: &mut Sim, group: u16, cluster: u16, cmd: u8, payload: &[u8]) {
    let c = &mut s.w.coord;
    let seq = crate::station::nx(&mut c.st.zcl_seq);
    let mut aps = vec![0x0C];
    aps.extend_from_slice(&group.to_le_bytes());
    aps.extend_from_slice(&cluster.to_le_bytes());
    aps.extend_from_slice(&HA.to_le_bytes());
    aps.push(1);
    aps.push(crate::station::nx(&mut c.st.aps_ctr));
    aps.extend_from_slice(&[0x01 | 0x10, seq, cmd]);
    aps.extend_from_slice(payload);
    c.nwk_send_data(&mut s.w.air, 0xFFFD, &aps);
}

fn ask(s: &mut Sim, short: u16, cluster: u16, cmd: u8, payload: &[u8], rsp: u8) -> R<Option<Vec<u8>>> {
    let n0 = s.w.coord.zcl_rsp.len();
    s.w.coord.zcl_cmd(&mut s.w.air, short, 1, cluster, cmd, payload);
    let hit = |s: &Sim| s.w.coord.zcl_rsp[n0..].iter().find(|r| r.src == short && r.cluster == cluster && r.cmd == Some(rsp)).map(|r| r.body.clone());
    s.run_until(3000.0, 10.0, |s| hit(s).is_some())?;
    Ok(hit(s))
}

fn group(cx: &mut SimCtx) -> R<()> {
    const G: u16 = 0x0B01;
    let (mut s, b) = build(cx, &[(Fw::Rust, "R", [0x21, 0x00, 0x63, 0x00, 0x00]), (Fw::Stock, "S", [0x22, 0x00, 0x64, 0x00, 0x00])])?;
    s.w.set_t_off(b[1].id, 900.0);
    s.w.coord.start(&mut s.w.air);
    cx.say(&format!("== {}; group {G:#06x}", describe(&b)));
    let Some(sh) = join_all(&mut s, &b, 60000.0)? else {
        cx.check(s.now(), "both bulbs join", false, &shorts_text(&s, &b));
        finish(cx, &s, &b);
        return Ok(());
    };
    cx.check(s.now(), "both bulbs join", true, &shorts_text(&s, &b));
    s.run(6000.0)?;
    let hx = super::zigbee::hex;
    let both = |s: &Sim| -> (crate::session::Light, crate::session::Light) { (s.light(b[0].id), s.light(b[1].id)) };
    let lt = |l: &crate::session::Light| format!("{} {:.1} %", onoff(l.on), l.duty);
    let mut add = Vec::new();
    for k in 0..2 {
        add.push(ask(&mut s, sh[k], 0x0004, 0x00, &[G as u8, (G >> 8) as u8, 0x00], 0x00)?);
    }
    let want = vec![0x00, G as u8, (G >> 8) as u8];
    cx.check(s.now(), "Add Group: both answer SUCCESS with the group (ZCL 8 3.6.2.3.2)", add.iter().all(|r| r.as_deref() == Some(&want[..])),
             &add.iter().map(|r| r.as_deref().map(hx).unwrap_or_else(|| "none".into())).collect::<Vec<_>>().join(", "));
    let mut mem = Vec::new();
    for k in 0..2 {
        mem.push(ask(&mut s, sh[k], 0x0004, 0x02, &[0x00], 0x02)?);
    }
    let lists = |r: &Option<Vec<u8>>| r.as_ref().is_some_and(|v| v.len() >= 4 && v[1] >= 1 && v[2..].chunks(2).any(|c| c == [G as u8, (G >> 8) as u8]));
    cx.check(s.now(), "Get Group Membership lists it on both", mem.iter().all(lists),
             &mem.iter().map(|r| r.as_deref().map(hx).unwrap_or_else(|| "none".into())).collect::<Vec<_>>().join(", "));
    groupcast(&mut s, G, 0x0006, 0x00, &[]);
    s.run(3000.0)?;
    let (lr, ls) = both(&s);
    cx.check(s.now(), "Off to the group: both off", !lr.on && !ls.on, &format!("R {}, S {}", lt(&lr), lt(&ls)));
    groupcast(&mut s, G, 0x0008, 0x04, &[127, 0, 0]);
    s.run(3000.0)?;
    let (lr, ls) = both(&s);
    cx.check(s.now(), "Move to Level (with On/Off) 127 to the group: both on, at the same duty (the same curve)",
             lr.on && ls.on && (lr.duty - ls.duty).abs() < 1.0, &format!("R {}, S {}", lt(&lr), lt(&ls)));
    let lvl127 = (lr.duty, ls.duty);
    let mut st = Vec::new();
    for k in 0..2 {
        st.push(ask(&mut s, sh[k], 0x0005, 0x04, &[G as u8, (G >> 8) as u8, 0x01], 0x04)?);
    }
    let want = vec![0x00, G as u8, (G >> 8) as u8, 0x01];
    cx.check(s.now(), "Store Scene 1 of the group: both SUCCESS (ZCL 8 3.7.2.4.4)", st.iter().all(|r| r.as_deref() == Some(&want[..])),
             &st.iter().map(|r| r.as_deref().map(hx).unwrap_or_else(|| "none".into())).collect::<Vec<_>>().join(", "));
    groupcast(&mut s, G, 0x0008, 0x04, &[254, 0, 0]);
    let mut seen_off = false;
    for _ in 0..30 {
        s.run(200.0)?;
        seen_off |= !s.light(b[1].id).on;
    }
    s.run(1000.0)?;
    cx.say(&format!("   {} went off for a moment after Store Scene (the stock firmware's own confirmation): {seen_off}", b[1].who()));
    let (lr, ls) = both(&s);
    cx.check(s.now(), "Move to Level 254 to the group: both at full", lr.duty > 99.0 && ls.duty > 99.0, &format!("R {}, S {}", lt(&lr), lt(&ls)));
    groupcast(&mut s, G, 0x0005, 0x05, &[G as u8, (G >> 8) as u8, 0x01]);
    s.run(4000.0)?;
    let (lr, ls) = both(&s);
    cx.check(s.now(), "Recall Scene 1 to the group: both back at the stored level (ZCL 8 3.7.2.4.5)",
             (lr.duty - lvl127.0).abs() < 1.0 && (ls.duty - lvl127.1).abs() < 1.0,
             &format!("R {}, S {} (stored {:.1} %, {:.1} %)", lt(&lr), lt(&ls), lvl127.0, lvl127.1));
    let rm = ask(&mut s, sh[1], 0x0004, 0x03, &[G as u8, (G >> 8) as u8], 0x03)?;
    cx.check(s.now(), &format!("Remove Group from {}: SUCCESS", b[1].who()), rm.as_deref() == Some(&[0x00, G as u8, (G >> 8) as u8][..]),
             &rm.as_deref().map(hx).unwrap_or_else(|| "none".into()));
    groupcast(&mut s, G, 0x0006, 0x00, &[]);
    s.run(3000.0)?;
    let (lr, ls) = both(&s);
    cx.check(s.now(), &format!("Off to the group: {} (still a member) off, {} (no longer) stays on", b[0].who(), b[1].who()), !lr.on && ls.on,
             &format!("R {}, S {}", lt(&lr), lt(&ls)));
    let (ok, rs) = resets_text(&s, &b);
    cx.check(s.now(), "no resets", ok, &format!("resets {rs}"));
    finish(cx, &s, &b);
    Ok(())
}

fn ota_relay(cx: &mut SimCtx) -> R<()> {
    let stock_img = image(cx, Fw::Stock);
    let sbytes = stock_img.bytes().map_err(crate::session::Stop::Error)?;
    let (mut s, b) = build(cx, &[(Fw::Stock, "S", [0x31, 0x00, 0x65, 0x00, 0x00]), (Fw::Rust, "R", [0x32, 0x00, 0x66, 0x00, 0x00])])?;
    let (sid, rid) = (b[0].id, b[1].id);
    s.w.air.block(NodeId::Coord, rid, true);
    s.w.set_t_off(rid, 10000.0);
    s.w.coord.start(&mut s.w.air);
    let rust_ota = cx.image("rust-ota-file", &b[1].img.path.to_string_lossy().replace(".bin", ".ota"));
    let rb = rust_ota.bytes().map_err(crate::session::Stop::Error)?;
    let (mfg, itype) = (u16::from_le_bytes([rb[10], rb[11]]), u16::from_le_bytes([rb[12], rb[13]]));
    let file = super::transfer::ota_file(mfg, itype, 0x0000_0001, "stock firmware (revert)", &sbytes);
    let size = file.len();
    s.w.coord.add_ota_file(file).map_err(crate::session::Stop::Error)?;
    cx.say(&format!("== {}; R behind S; OTA file {size} B (mfg {mfg:#06x}, type {itype:#06x})", describe(&b)));
    let ok = s.run_until(10000.0, 10.0, |s| s.announced(b[0].ext).is_some())?;
    if !cx.check(s.now(), "the stock bulb joins at the coordinator", ok, &shorts_text(&s, &b)) {
        finish(cx, &s, &b);
        return Ok(());
    }
    s.run(300.0)?;
    s.w.coord.permit_joining(&mut s.w.air, 180);
    let Some(sh) = join_all(&mut s, &b, 60000.0)? else {
        cx.check(s.now(), "the Rust bulb joins through it", false, &shorts_text(&s, &b));
        finish(cx, &s, &b);
        return Ok(());
    };
    cx.check(s.now(), "the Rust bulb joins through it", true, &shorts_text(&s, &b));
    let (ss, rs) = (sh[0], sh[1]);
    s.w.coord.st.via.insert(rs, ss);
    s.run(8000.0)?;
    let t0 = s.now();
    let resets0 = s.w.bulb_resets(rid);
    s.w.coord.image_notify(&mut s.w.air, rs, 1);
    let wall = std::time::Instant::now();
    let done = s.run_until(1500.0 * 1000.0, 200.0, |s| s.events("ota-upgrade-end").len() > 0 || s.w.bulb_resets(rid) > resets0)?;
    let q = s.events("ota-query");
    cx.check(s.now(), "Query Next Image from the Rust bulb, through the stock router", q.iter().any(|e| e.get("src") == Some(format!("{rs:#06x}").as_str())),
             &q.last().map(|e| e.details.clone()).unwrap_or_default());
    let blocks: Vec<(usize, usize)> = s.w.coord.ota_log.iter().filter(|(_, k, _)| k == "ota-block").map(|(_, _, d)| {
        let f = |k: &str| -> usize { d.split(", ").find_map(|kv| kv.strip_prefix(k).and_then(|r| r.strip_prefix('='))).and_then(|v| v.parse().ok()).unwrap_or(0) };
        (f("offset"), f("size"))
    }).collect();
    let got = blocks.iter().map(|(o, n)| o + n).max().unwrap_or(0);
    cx.check(s.now(), "the whole file downloaded through the stock router", done && got >= size,
             &format!("{got}/{size} B in {} blocks, {:.0} s simulated, {:.0} s wall", blocks.len(), (s.now() - t0) / 1000.0, wall.elapsed().as_secs_f64()));
    let up = relayed(&s, ss, rs, t0, Some(0x0000));
    let down = relayed(&s, ss, 0x0000, t0, Some(rs));
    cx.check(s.now(), "the stock router relayed every request up and every block down", up >= blocks.len() && down >= blocks.len(),
             &format!("{up} up, {down} down for {} blocks", blocks.len()));
    let end = s.events("ota-upgrade-end");
    cx.check(s.now(), "Upgrade End: status SUCCESS", end.last().is_some_and(|e| e.get("status") == Some("0x00")),
             &end.last().map(|e| e.details.clone()).unwrap_or_default());
    s.run_until(20000.0, 20.0, |s| s.w.bulb_resets(rid) > resets0)?;
    let f = &s.w.node(rid).m.flash.mem;
    let staged = &f[0x6000..0x6000 + sbytes.len()];
    let nd = (0..sbytes.len()).filter(|i| staged[*i] != sbytes[*i]).count();
    cx.check(s.now(), "staged at 0x6000 byte for byte and marked bootable; the bulb restarted", nd == 0 && staged[8] == 0x4B && s.w.bulb_resets(rid) > resets0,
             &format!("differing bytes {nd}, byte 8 {:#04x}, resets {}", staged[8], s.w.bulb_resets(rid) - resets0));
    s.run(8000.0)?;
    let dt = command(&mut s, rs, 0x0006, 0x02, &[], 5000.0)?;
    cx.check(s.now(), "after the restart (the old image: no bootloader in the emulator) the Rust bulb answers through the stock router", dt.is_some(),
             &after(dt, "no answer"));
    let sr = s.w.bulb_resets(sid);
    cx.check(s.now(), "the stock router: no reset under the load", sr == 0, &format!("resets {sr}"));
    finish(cx, &s, &b);
    Ok(())
}

fn own_shorts(s: &Sim, b: &[Bulb], since: f64) -> Vec<Option<u16>> {
    let mut last: Vec<Option<u16>> = vec![None; b.len()];
    for f in &s.w.air.frames {
        if f.t0 < since {
            continue;
        }
        let Some(k) = b.iter().position(|x| x.id == f.who) else { continue };
        let Some(m) = mac_parse(&f.psdu) else { continue };
        let p = &m.payload;
        if m.ty != 1 || p.len() < 8 + 14 {
            continue;
        }
        let fcf = u16::from_le_bytes([p[0], p[1]]);
        if fcf & 0x0200 == 0 {
            continue;
        }
        let src = u16::from_le_bytes([p[4], p[5]]);
        if m.src != Some(src as u64) {
            continue;
        }
        let i = 8 + if fcf & 0x0800 != 0 { 8 } else { 0 } + if fcf & 0x1000 != 0 { 8 } else { 0 };
        let Some(ext) = p.get(i + 5..i + 13).map(|x| u64::from_le_bytes(x.try_into().unwrap())) else { continue };
        if ext == b[k].ext {
            last[k] = Some(src);
        }
    }
    last
}

fn conflict(cx: &mut SimCtx) -> R<()> {
    let (mut s, b) = build(cx, &[(Fw::Rust, "R", [0x41, 0x00, 0x67, 0x00, 0x00]), (Fw::Stock, "S", [0x42, 0x00, 0x68, 0x00, 0x00])])?;
    let (rid, sid) = (b[0].id, b[1].id);
    s.w.set_t_off(sid, 40000.0);
    s.w.coord.start(&mut s.w.air);
    cx.say(&format!("== {}", describe(&b)));
    let ok = s.run_until(15000.0, 10.0, |s| s.announced(b[0].ext).is_some())?;
    let r0 = s.announced(b[0].ext);
    if !cx.check(s.now(), &format!("{} joins", b[0].who()), ok, &format!("R={}", hexo(r0))) {
        finish(cx, &s, &b);
        return Ok(());
    }
    let r0 = r0.unwrap();
    s.run(8000.0)?;
    s.w.power_off(rid);
    s.w.coord.set_next_short(r0);
    let ok = s.run_until(60000.0, 10.0, |s| s.announced(b[1].ext).is_some())?;
    let s0 = s.announced(b[1].ext);
    if !cx.check(s.now(), &format!("while it is off, {} joins and is given the same address", b[1].who()), ok && s0 == Some(r0),
                 &format!("S={}", hexo(s0))) {
        finish(cx, &s, &b);
        return Ok(());
    }
    s.run(8000.0)?;
    let ann0 = s.events("device-announce").len();
    s.w.power_on(rid).map_err(super::emu)?;
    s.run(5000.0)?;
    let back = s.events("device-announce")[ann0..].iter().any(|e| e.get("ext") == Some(fmt_ext(b[0].ext).as_str())
        && e.get("short") == Some(format!("{r0:#06x}").as_str()));
    cx.check(s.now(), &format!("{} is back with the address from its NV and announces it: two devices on one address", b[0].who()), back,
             &s.events("device-announce")[ann0..].iter().map(|e| e.details.clone()).collect::<Vec<_>>().join("; "));
    s.run(60000.0)?;
    let now = own_shorts(&s, &b, s.now() - 20000.0);
    let anns: Vec<String> = s.events("device-announce")[ann0..].iter().map(|e| e.details.clone()).collect();
    let distinct = now[0].is_some() && now[1].is_some() && now[0] != now[1];
    cx.check(s.now(), "a minute later each has an address of its own (R22 3.6.1.9.3)", distinct,
             &format!("R sends from {}, S from {}; Device_annce since: [{}]", hexo(now[0]), hexo(now[1]), anns.join("; ")));
    let announced_new = |k: usize| -> bool {
        now[k].is_some_and(|a| a != r0)
            && s.events("device-announce")[ann0..].iter().any(|e| e.get("ext") == Some(fmt_ext(b[k].ext).as_str())
                && e.get("short") == Some(format!("{:#06x}", now[k].unwrap()).as_str()))
    };
    let movers: Vec<&str> = (0..2).filter(|k| now[*k].is_some_and(|a| a != r0)).map(|k| b[k].name.as_str()).collect();
    cx.check(s.now(), "the bulb that moved announced its new address (Device_annce)", !movers.is_empty() && (0..2).filter(|k| now[*k] != Some(r0)).all(announced_new),
             &format!("moved: [{}]", movers.join(", ")));
    let mut ok = distinct;
    let mut detail = Vec::new();
    for k in 0..2 {
        let Some(a) = now[k] else { continue };
        s.w.coord.st.via.remove(&a);
        let (dt, l) = light_cmd(&mut s, b[k].id, a, 0x0006, 0x00, &[], 5000.0)?;
        ok &= dt.is_some() && !l.on;
        detail.push(format!("{} at {a:#06x}: {}", b[k].name, after(dt, "no answer")));
    }
    cx.check(s.now(), "both are reachable at their addresses", ok, &detail.join("; "));
    let (okr, rs) = resets_text(&s, &b);
    cx.check(s.now(), "no resets", okr, &format!("resets {rs}"));
    let _ = sid;
    finish(cx, &s, &b);
    Ok(())
}
