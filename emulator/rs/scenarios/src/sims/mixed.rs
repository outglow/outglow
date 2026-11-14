//! Networks of the Rust firmware and the original (stock) firmware together:
//! the stock firmware is AwoX's own build of Telink's Zigbee stack, an
//! implementation independent of `crates/zigbee`, so each scenario holds the
//! Rust bulb against it in one role or the other (parent and child, relay for
//! each other, neighbours in a mesh, members of one group).
//!
//! Images by role: `--image rust=...` (default target/tc32/bulb-tc32.bin) and
//! `--image stock=...` (default stock/stock-dimm.bin; in `OUTGLOW_STOCK_DIR`,
//! a checkout of outglow/original-firmware, when it is not in stock/). The
//! tunable white pair: `--image rust=target/tc32-tw/bulb-tc32.bin --image
//! stock=stock/stock-tw.bin`. A scenario is skipped when an image is missing.
//!
//! The expectations come from the specification (R22, ZCL 8, 802.15.4) and,
//! where it leaves a choice, from what the stock firmware was measured to do
//! here; the details of each check say which.

use super::util::{after, command, hex_list, hexo, onoff, rel, times, verified};
use super::{node_of, Sim, SimCtx, SimDef};
use crate::air::NodeId;
use crate::coord::{Coord, HA, NWK_KEY};
use crate::frames::{fmt_ext, mac_parse};
use crate::image::Image;
use crate::session::R;

pub fn defs() -> Vec<SimDef> {
    vec![
        SimDef { name: "mixed_parent_rust", doc: "a stock bulb out of the coordinator's range joins through a Rust bulb", run: parent_rust },
        SimDef { name: "mixed_parent_stock", doc: "a Rust bulb out of the coordinator's range joins through a stock bulb", run: parent_stock },
        SimDef { name: "mixed_mesh", doc: "a Rust and a stock bulb relay for each other: routes, many-to-one, source routes, broadcasts", run: mesh },
    ]
}

pub const QUANTUM: f64 = 0.25;

/// Which firmware a bulb runs.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Fw {
    Rust,
    Stock,
}

impl Fw {
    pub fn label(self) -> &'static str {
        match self {
            Fw::Rust => "rust",
            Fw::Stock => "stock",
        }
    }
    pub fn default_image(self) -> &'static str {
        match self {
            Fw::Rust => "target/tc32/bulb-tc32.bin",
            Fw::Stock => "stock/stock-dimm.bin",
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Fw::Rust => "Rust",
            Fw::Stock => "stock",
        }
    }
}

/// The image of a role (`--image rust=` / `--image stock=`).
pub fn image(cx: &SimCtx, fw: Fw) -> Image {
    cx.image(fw.label(), fw.default_image())
}

/// The factory MAC record at 0xFF000 for a bulb whose varying bytes are `v`:
/// the BLE MAC little endian with the Telink OUI a4:c1:38 in bytes 3..6 (the
/// stock firmware keeps only such a record, else it writes its own, the same
/// in every emulated bulb) and two more bytes.
pub const fn mac_record(v: [u8; 5]) -> [u8; 8] {
    [v[0], v[1], v[2], 0x38, 0xC1, 0xA4, v[3], v[4] & 0x7F]
}

/// The IEEE address a bulb makes of its MAC record: bytes 6..8 in front of
/// the BLE MAC (measured on the stock firmware; the Rust firmware does the same).
pub const fn ieee_of(_fw: Fw, rec: &[u8; 8]) -> u64 {
    u64::from_le_bytes([rec[6], rec[7], rec[0], rec[1], rec[2], rec[3], rec[4], rec[5]])
}

/// One emulated bulb of a mixed network.
#[derive(Clone)]
pub struct Bulb {
    pub id: NodeId,
    pub fw: Fw,
    pub name: String,
    pub ext: u64,
    pub img: Image,
}

/// Bulbs to be: (firmware, name, varying MAC bytes).
pub type Spec = (Fw, &'static str, [u8; 5]);

/// The first bulb owns the schedule; the coordinator on channel 11 with the default key.
pub fn build(cx: &SimCtx, specs: &[Spec]) -> R<(Sim, Vec<Bulb>)> {
    build_on(cx, specs, Coord::new_plain(11, NWK_KEY))
}

pub fn build_on(cx: &SimCtx, specs: &[Spec], coord: Coord) -> R<(Sim, Vec<Bulb>)> {
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
        bulbs.push(Bulb { id, fw: *fw, name: name.to_string(), ext: ieee_of(*fw, &rec), img });
    }
    let s = sim.ok_or_else(|| crate::session::Stop::Error("no bulbs".into()))?;
    Ok((s, bulbs))
}

/// `[(id, name)]` for `finish`.
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

/// Short addresses in a node's neighbour table (ZDO Mgmt_Lqi, all pages).
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

/// Frames sent (MAC) by `by` whose NWK source is `of` (and destination `to`) since `since`.
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

/// How long a light command is given to take effect: the stock firmware
/// fades to the new level (about 1 s, measured), the Rust firmware switches at once.
pub const SETTLE_MS: f64 = 1500.0;

/// A light command (`command`) and the light once it has settled.
pub fn light_cmd(s: &mut Sim, id: NodeId, short: u16, cluster: u16, cmd: u8, payload: &[u8], max_ms: f64)
                 -> R<(Option<f64>, crate::session::Light)> {
    let dt = command(s, short, cluster, cmd, payload, max_ms)?;
    s.run(SETTLE_MS)?;
    Ok((dt, s.light(id)))
}

/// Many-to-one route request, twice (one broadcast can be lost).
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

/// An On/Off command to all routers (NWK broadcast 0xFFFD).
pub fn bcast(s: &mut Sim, cmd: u8) {
    s.w.coord.zcl_send_with(&mut s.w.air, 0xFFFD, 1, 0x0006, cmd, &[], HA, true, true, None, false);
}

/// Waits until every bulb has announced itself; their short addresses.
pub fn join_all(s: &mut Sim, b: &[Bulb], max_ms: f64) -> R<Option<Vec<u16>>> {
    let exts: Vec<u64> = b.iter().map(|x| x.ext).collect();
    let ok = s.run_until(max_ms, 10.0, |s| exts.iter().all(|e| s.announced(*e).is_some()))?;
    Ok(if ok { Some(exts.iter().map(|e| s.announced(*e).unwrap()).collect()) } else { None })
}

/// The bulbs' addresses as text, `A=0x1234 B=None`.
pub fn shorts_text(s: &Sim, b: &[Bulb]) -> String {
    b.iter().map(|x| format!("{}={}", x.name, hexo(s.announced(x.ext)))).collect::<Vec<_>>().join(" ")
}

/// The last coordinator events, for a failed join.
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

// =================================================================== parent

fn parent_rust(cx: &mut SimCtx) -> R<()> {
    parent(cx, Fw::Rust, Fw::Stock)
}

fn parent_stock(cx: &mut SimCtx) -> R<()> {
    parent(cx, Fw::Stock, Fw::Rust)
}

/// The joiner is out of the coordinator's range from the start, so the only
/// beacon it can answer is the parent's: beacon with joining allowed once the
/// network is open, MAC association with the response handed over on the
/// newcomer's poll (802.15.4 indirect transmission: frame-pending bit in the
/// ACK of the Data Request), APS Update-Device to the Trust Center (R22
/// 4.6.3.2), the network key passed on from an APS Tunnel command; then the
/// joiner's own traffic through its parent.
fn parent(cx: &mut SimCtx, pfw: Fw, jfw: Fw) -> R<()> {
    let (mut s, b) = build(cx, &[(pfw, "P", [0x01, 0x00, 0x52, 0x00, 0x00]), (jfw, "J", [0x02, 0x00, 0x54, 0x00, 0x00])])?;
    let (p, j) = (b[0].clone(), b[1].clone());
    s.w.set_t_off(j.id, 8000.0); // the joiner is powered later
    s.w.air.block(NodeId::Coord, j.id, true); // and never hears the coordinator
    s.w.coord.start(&mut s.w.air);
    cx.say(&format!("== parent P: {}; joiner J: {}; channel 11", pfw.title(), jfw.title()));
    cx.say(&format!("   {}", describe(&b)));

    let ok = s.run_until(15000.0, 10.0, |s| s.announced(p.ext).is_some())?;
    let ps = s.announced(p.ext);
    if !cx.check(s.now(), &format!("the {} bulb P joins at the coordinator", pfw.title()), ok, &format!("P={}", hexo(ps))) {
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
    if !cx.check(now, &format!("the {} bulb J, hearing only P, joins through it", jfw.title()), ok,
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

// =================================================================== mesh

/// Each bulb in turn out of the coordinator's range so that all its traffic
/// goes through the other: unicast routing both ways, Route Records after a
/// many-to-one route request (R22 3.6.4.5), source-routed frames relayed
/// (3.6.3.3.2), broadcasts relayed exactly once (3.6.5).
fn mesh(cx: &mut SimCtx) -> R<()> {
    let (mut s, b) = build(cx, &[(Fw::Rust, "R", [0x01, 0x00, 0x52, 0x00, 0x00]), (Fw::Stock, "S", [0x02, 0x00, 0x54, 0x00, 0x00])])?;
    let (rb, sb) = (b[0].clone(), b[1].clone());
    let (rid, tid) = (rb.id, sb.id);
    s.w.set_t_off(tid, 900.0); // not both scanning in the same instant
    // The stock bulb never hears the coordinator: it joins through the Rust
    // bulb. (A stock bulb whose parent is the coordinator and whose link to it
    // goes away keeps sending to the coordinator directly, measured for 4
    // minutes: it does not age its parent out of the neighbour table, so it
    // cannot be moved behind another router as the Rust bulb is below;
    // `mixed_link_loss`.)
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

    s.run(34000.0)?; // two link status periods
    let nr = neighbors(&mut s, r)?;
    let nt = neighbors(&mut s, t)?;
    cx.check(s.now(), "the Rust bulb (S's parent) lists the stock bulb and the coordinator as neighbours",
             nr.as_ref().is_some_and(|v| v.contains(&t) && v.contains(&0)), &format!("R's neighbours: {}", hex_list(nr.as_deref().unwrap_or(&[]))));
    cx.check(s.now(), "the stock bulb lists the Rust bulb as a neighbour", nt.as_ref().is_some_and(|v| v.contains(&r)),
             &format!("S's neighbours: {}", hex_list(nt.as_deref().unwrap_or(&[]))));

    for (far, near) in [(&sb, &rb), (&rb, &sb)] {
        let (fs, ns) = (sh[if far.id == rid { 0 } else { 1 }], sh[if near.id == rid { 0 } else { 1 }]);
        cx.say(&format!("-- the {} bulb out of the coordinator's range: everything goes through the {} bulb", far.fw.title(), near.fw.title()));
        if far.id == rid {
            s.w.air.block(NodeId::Coord, near.id, false);
            s.w.air.block(NodeId::Coord, far.id, true);
            // the Rust bulb forgets the coordinator as a neighbour when its link
            // status stays away (R22 3.6.1.5: nwkRouterAgeLimit 3 x nwkLinkStatusPeriod 15 s)
            s.run(50000.0)?;
        }
        s.w.coord.st.via.insert(fs, ns);
        let t0 = s.now();
        let (dt, l) = light_cmd(&mut s, far.id, fs, 0x0006, 0x00, &[], 20000.0)?;
        if std::env::var_os("MIXED_DEBUG").is_some() {
            debug_frames(cx, &s, &b, t0);
        }
        let off = !l.on;
        cx.check(s.now(), &format!("Off reaches the {} bulb through the {} bulb and the answer finds its way back", far.fw.title(), near.fw.title()),
                 dt.is_some() && off, &after(dt, "no default response"));
        let n = relayed(&s, ns, fs, t0, None);
        cx.check(s.now(), &format!("the {} bulb relayed the {} bulb's frames", near.fw.title(), far.fw.title()), n >= 1, &format!("{n} frame(s)"));
        let mut tt = Vec::new();
        for _ in 0..6 {
            tt.push(command(&mut s, fs, 0x0006, 0x02, &[], 3000.0)?);
        }
        cx.check(s.now(), "six more commands over the established route, each answered within 3 s", tt.iter().all(|x| x.is_some()), &times(&tt));
        let n0 = s.w.coord.route_records.len();
        let t0 = s.now();
        concentrator(&mut s)?;
        let n = relayed(&s, ns, 0x0000, t0, Some(0xFFFC));
        cx.check(s.now(), &format!("the {} bulb relays the coordinator's many-to-one route request", near.fw.title()), n >= 1,
                 &format!("{n} relayed frame(s)"));
        let dt = command(&mut s, fs, 0x0006, 0x01, &[], 3000.0)?;
        let rec = records(&s, n0, fs);
        cx.check(s.now(), &format!("the {} bulb's Route Record arrives with the {} bulb added as relay", far.fw.title(), near.fw.title()),
                 dt.is_some() && rec.iter().any(|x| *x == [ns]), &format!("records from {}: {}", far.name, records_text(&rec)));
        let dt = command(&mut s, ns, 0x0006, 0x02, &[], 3000.0)?;
        let own = s.w.coord.route_records[n0..].iter().any(|x| x.1 == ns && x.2.is_empty());
        cx.check(s.now(), &format!("the {} bulb sends its own Route Record (no relays) before its answer", near.fw.title()), dt.is_some() && own,
                 &records_text(&records(&s, n0, ns)));
        s.w.coord.st.via.remove(&fs);
        s.w.coord.source_routes.insert(fs, vec![ns]);
        let (dt, l) = light_cmd(&mut s, far.id, fs, 0x0006, 0x00, &[], 3000.0)?;
        let off = !l.on;
        cx.check(s.now(), &format!("a source-routed frame to the {} bulb is relayed by the {} bulb", far.fw.title(), near.fw.title()),
                 dt.is_some() && off, &dt.map(|d| format!("answered after {d:.0} ms")).unwrap_or_else(|| "no answer".into()));
        s.w.coord.source_routes.remove(&fs);
    }

    // ---- broadcasts (R is out of range now, behind S)
    s.w.coord.st.via.insert(r, t);
    command(&mut s, r, 0x0006, 0x00, &[], 3000.0)?;
    command(&mut s, t, 0x0006, 0x00, &[], 3000.0)?;
    s.run(SETTLE_MS)?;
    let was_off = !s.light(rid).on && !s.light(tid).on;
    s.run(300.0)?;
    bcast(&mut s, 0x01);
    s.run(3000.0)?;
    let (lr, lt) = (s.light(rid).on, s.light(tid).on);
    cx.check(s.now(), "a broadcast On is relayed by the stock bulb to the Rust bulb", was_off && lr && lt, &format!("R {}, S {}", onoff(lr), onoff(lt)));
    s.w.coord.st.via.remove(&r);
    s.w.air.block(NodeId::Coord, rid, false);
    s.w.air.block(NodeId::Coord, tid, true);
    s.run(34000.0)?;
    bcast(&mut s, 0x00);
    s.run(3000.0)?;
    let (lr, lt) = (s.light(rid).on, s.light(tid).on);
    cx.check(s.now(), "a broadcast Off is relayed by the Rust bulb to the stock bulb", !lr && !lt, &format!("R {}, S {}", onoff(lr), onoff(lt)));
    bcast(&mut s, 0x02);
    s.run(3000.0)?;
    let (lr, lt) = (s.light(rid).on, s.light(tid).on);
    cx.check(s.now(), "a broadcast Toggle switches both bulbs exactly once", lr && lt, &format!("R {}, S {}", onoff(lr), onoff(lt)));
    let (ok, rs) = resets_text(&s, &b);
    cx.check(s.now(), "no resets, nobody left the network", ok && s.events("leave").is_empty(), &format!("resets {rs}"));
    finish(cx, &s, &b);
    Ok(())
}

/// `MIXED_DEBUG=1`: the frames since `since`, one line each (MAC and NWK addresses).
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
        cx.say(&format!("   [{:9.3}] {who:>6} ty={} seq={} {:?}->{:?}{nwk}", f.t0 / 1000.0, m.ty, m.seq, m.src, m.dst));
    }
}
