#[path = "../../sense/tests/model/mod.rs"]
mod model;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use model::{Channel, Crossing, Rng};
use crate::sim::*;
use zigbee::hal::NvItem;
use zigbee::mac;
use zigbee::stack::Identity;
use zigbee::stack::{SENSE_DEFAULT_OFF_S, SENSE_DEFAULT_S, SENSE_ON_RATE};

const OCC: u16 = 0x0406;
const SENSE: u16 = 0xFC5E;
const CODE: u16 = 0x1141;
const PROBE_PAN: u16 = 0x5E55;
const T_U8: u8 = 0x20;
const T_U16: u8 = 0x21;

struct Field {
    links: HashMap<(u16, usize), Channel>,
    g: Rng,
}

type Shared = Rc<RefCell<Field>>;

fn mac_src_short(mpdu: &[u8]) -> Option<u16> {
    let (h, _) = mac::Header::parse(mpdu)?;
    match h.src {
        mac::Addr::Short(s) => Some(s),
        _ => None,
    }
}

fn field(net: &mut Net) -> Shared {
    let f = Rc::new(RefCell::new(Field { links: HashMap::new(), g: Rng(7) }));
    let ff = f.clone();
    net.air.borrow_mut().rssi_model = Some(Box::new(move |port, mpdu, now| {
        let mut f = ff.borrow_mut();
        let Some(src) = mac_src_short(mpdu) else {
            return Some(-50);
        };
        let Field { links, g } = &mut *f;
        match links.get_mut(&(src, port)) {
            Some(ch) => ch.rssi(g, now),
            None => Some(-50),
        }
    }));
    f
}

fn two() -> (Net, Shared, [usize; 2], [u16; 2]) {
    let (mut net, f, n, s) = two_off();
    for a in s {
        switch(&mut net, a, true);
    }
    (net, f, n, s)
}

fn switch(net: &mut Net, short: u16, on: bool) {
    assert_eq!(write(net, short, SENSE, 0x0001, T_U8, &[if on { SENSE_ON_RATE } else { 0 }]), 0);
}

fn two_off() -> (Net, Shared, [usize; 2], [u16; 2]) {
    let mut net = Net::new();
    net.identity = IDENTITY_SENSE;
    let a = net.join_node();
    let b = net.join_node();
    let f = field(&mut net);
    let k = model::link("K1", "K2");
    let (sa, sb) = (net.nodes[a].short(), net.nodes[b].short());
    let (pa, pb) = (net.nodes[a].id, net.nodes[b].id);
    f.borrow_mut().links.insert((sb, pa), Channel::of(&k, k.sigma_lo));
    f.borrow_mut().links.insert((sa, pb), Channel::of(&k, k.sigma_lo));
    (net, f, [a, b], [sa, sb])
}

fn both(f: &Shared, shorts: [u16; 2], ports: [usize; 2], mut each: impl FnMut(&mut Channel)) {
    let mut f = f.borrow_mut();
    each(f.links.get_mut(&(shorts[1], ports[0])).unwrap());
    each(f.links.get_mut(&(shorts[0], ports[1])).unwrap());
}

fn ports(net: &Net, n: [usize; 2]) -> [usize; 2] {
    [net.nodes[n[0]].id, net.nodes[n[1]].id]
}

fn read(net: &mut Net, short: u16, cluster: u16, id: u16) -> Option<Vec<u8>> {
    let i = id.to_le_bytes();
    let n0 = net.coord.zcl_rx.len();
    if cluster == SENSE {
        net.coord.zcl_mfr(short, cluster, 0x00, CODE, 0x00, &i);
    } else {
        net.coord.zcl(short, cluster, 0x00, 0x00, &i);
    }
    net.run(60);
    let hdr = if cluster == SENSE { 5 } else { 3 };
    let rsp = net.coord.zcl_rx[n0..].iter().rev().find(|(s, c, f)| *s == short && *c == cluster && f.get(hdr - 1) == Some(&0x01))?.2.clone();
    let rec = &rsp[hdr..];
    if rec.len() < 4 || rec[..2] != i || rec[2] != 0 {
        return None;
    }
    if rec[3] == 0x41 {
        assert_eq!(rec[4] as usize, rec.len() - 5, "an octet string of its length");
        return Some(rec[5..].to_vec());
    }
    Some(rec[4..].to_vec())
}

fn write(net: &mut Net, short: u16, cluster: u16, id: u16, ty: u8, value: &[u8]) -> u8 {
    let mut p = id.to_le_bytes().to_vec();
    p.push(ty);
    p.extend_from_slice(value);
    let n0 = net.coord.zcl_rx.len();
    if cluster == SENSE {
        net.coord.zcl_mfr(short, cluster, 0x00, CODE, 0x02, &p);
    } else {
        net.coord.zcl(short, cluster, 0x00, 0x02, &p);
    }
    net.run(60);
    let hdr = if cluster == SENSE { 5 } else { 3 };
    let rsp = net.coord.zcl_rx[n0..].iter().rev().find(|(s, c, f)| *s == short && *c == cluster && f.get(hdr - 1) == Some(&0x04)).expect("a write response");
    rsp.2[hdr]
}

fn reports(net: &Net, short: u16, cluster: u16, attr: u16, since: usize) -> Vec<u32> {
    let mut out = vec![];
    for (s, c, f) in &net.coord.zcl_rx[since..] {
        if *s != short || *c != cluster {
            continue;
        }
        let hdr = if f[0] & 0x04 != 0 { 5 } else { 3 };
        if f.get(hdr - 1) != Some(&0x0A) {
            continue;
        }
        let mut i = hdr;
        while i + 3 <= f.len() {
            let id = u16::from_le_bytes([f[i], f[i + 1]]);
            let n = match f[i + 2] {
                0x18 | 0x20 => 1,
                0x19 | 0x21 => 2,
                _ => break,
            };
            if id == attr {
                out.push(f[i + 3..i + 3 + n].iter().rev().fold(0u32, |v, b| v << 8 | *b as u32));
            }
            i += 3 + n;
        }
    }
    out
}

fn probes(net: &Net, port: usize, since: u32) -> Vec<Vec<u8>> {
    net.sent_by(port, since).into_iter().filter(|m| m.len() >= 9 && u16::from_le_bytes([m[3], m[4]]) == PROBE_PAN).collect()
}

fn past_holdoff(net: &mut Net) {
    let t = net.now();
    if t < 62_000 {
        net.run(62_000 - t);
    }
}

#[test]
fn the_clusters_are_on_the_endpoint_with_their_defaults() {
    let (mut net, _, n, [sa, _]) = two_off();
    let s = sa.to_le_bytes();
    net.coord.zdo(sa, 0x0004, &[s[0], s[1], 1]);
    net.run(50);
    let desc = net.coord.last_zdo(sa, 0x8004).unwrap();
    assert_eq!(desc[11], 10);
    assert_eq!(&desc[12 + 16..12 + 20], &[0x06, 0x04, 0x5E, 0xFC]);

    assert_eq!(read(&mut net, sa, OCC, 0x0000), Some(vec![0]), "Occupancy");
    assert_eq!(read(&mut net, sa, OCC, 0x0001), Some(vec![0]), "OccupancySensorType (PIR, as the C firmware)");
    assert_eq!(read(&mut net, sa, OCC, 0x0002), Some(vec![1]), "OccupancySensorTypeBitmap");
    assert_eq!(read(&mut net, sa, OCC, 0x0010), Some(SENSE_DEFAULT_OFF_S.to_le_bytes().to_vec()), "OFF delay 30 s");
    assert_eq!(read(&mut net, sa, OCC, 0xFFFD), Some(vec![2, 0]));
    assert_eq!(read(&mut net, sa, SENSE, 0x0000), Some(vec![SENSE_DEFAULT_S]), "sensitivity 40");
    assert_eq!(read(&mut net, sa, SENSE, 0x0001), Some(vec![0]), "no probes: the sensing is off");
    assert_eq!(read(&mut net, sa, SENSE, 0x0006), Some(vec![1]), "the automatic calibration ready for when it is on");
    assert_eq!(read(&mut net, sa, SENSE, 0x0003), Some(vec![0]), "no calibration running");
    assert_eq!(read(&mut net, sa, SENSE, 0x0004), Some(vec![0]), "self-tuning off");
    assert_eq!(read(&mut net, sa, SENSE, 0x0002), Some(vec![]), "no lines before the first probe");
    assert_eq!(read(&mut net, sa, SENSE, 0xFF43), Some(vec![0]));
    assert_eq!(read(&mut net, sa, SENSE, 0xFF46), Some(vec![0, 0]));
    assert_eq!(net.air.borrow().ports[net.nodes[n[0]].id].probe_pan, 0xFFFF, "off: the radio takes no probes");

    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let seq = net.coord.zcl(short, OCC, 0x00, 0x00, &[0, 0]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, OCC, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x00, 0xC3], "UNSUPPORTED_CLUSTER");
    assert_eq!(net.air.borrow().ports[net.nodes[n].id].probe_pan, 0xFFFF, "its radio does not take probes");
}

#[test]
fn probes_go_five_a_second_after_the_first_minute_unacknowledged() {
    let (mut net, _, n, [sa, _]) = two();
    let [pa, pb] = ports(&net, n);
    assert_eq!(net.air.borrow().ports[pa].probe_pan, PROBE_PAN, "the radio takes probes");
    net.run_until(70_000, "the first probe", |net| !probes(net, pa, 0).is_empty());
    let first = net.air.borrow().log.iter().find(|e| e.1 == pa && e.2.len() >= 5 && u16::from_le_bytes([e.2[3], e.2[4]]) == PROBE_PAN).unwrap().0;
    assert!(first >= 60_000, "no probe in the first 60 s: the first at {first} ms");
    let t0 = net.now();
    net.run(12_000);
    let (na, nb) = (probes(&net, pa, t0).len(), probes(&net, pb, t0).len());
    assert!((50..=70).contains(&na) && (50..=70).contains(&nb), "~5/s: {na} and {nb} in 12 s");
    let p = probes(&net, pa, t0).remove(0);
    assert_eq!(p.len(), 22, "13 bytes of payload");
    assert_eq!(&p[..2], &[0x41, 0x88], "data, no acknowledgement, PAN compression, short addresses");
    assert_eq!(&p[5..9], &[0xFF, 0xFF, sa as u8, (sa >> 8) as u8]);
    assert_eq!(&p[9..12], b"EG\x01");
    assert_eq!(u16::from_le_bytes([p[12], p[13]]), PAN, "the network's PAN inside");
    assert_eq!(p[18], 5, "the rate");
    let log = net.air.borrow().log.clone();
    for w in log.windows(2) {
        if w[0].2.len() >= 5 && u16::from_le_bytes([w[0].2[3], w[0].2[4]]) == PROBE_PAN {
            assert!(!(w[1].2.len() == 3 && w[1].2[0] & 7 == 2 && w[1].0 == w[0].0), "an ACK after a probe");
        }
    }
}

#[test]
fn a_line_for_the_neighbour_shows_the_measured_level() {
    let (mut net, _, _, [sa, sb]) = two();
    past_holdoff(&mut net);
    net.run(8000);
    let lines = read(&mut net, sa, SENSE, 0x0002).unwrap();
    assert_eq!(lines.len(), 6, "one line: {lines:?}");
    assert_eq!(u16::from_le_bytes([lines[0], lines[1]]), sb);
    let rssi = lines[2] as i8;
    assert!((-60..=-54).contains(&rssi), "the K1-K2 level -56.75 dBm: {rssi}");
    assert_eq!(lines[5] & 0x01, 0, "not ON");
    let sens = read(&mut net, sa, SENSE, 0x0005).unwrap();
    assert_eq!(sens, vec![sb as u8, (sb >> 8) as u8, SENSE_DEFAULT_S, SENSE_DEFAULT_S], "s and anchor of the line");
}

#[test]
fn a_crossing_is_reported_to_the_coordinator_and_cleared_after_the_delay() {
    let (mut net, f, n, [sa, sb]) = two();
    let p = ports(&net, n);
    assert_eq!(write(&mut net, sa, OCC, 0x0010, T_U16, &5u16.to_le_bytes()), 0);
    assert_eq!(write(&mut net, sb, OCC, 0x0010, T_U16, &5u16.to_le_bytes()), 0);
    past_holdoff(&mut net);
    net.run(10_000);
    let n0 = net.coord.zcl_rx.len();
    net.run(30_000);
    assert!(reports(&net, sa, OCC, 0, n0).is_empty() && reports(&net, sb, OCC, 0, n0).is_empty(), "the empty room: no report");

    let t = net.now() + 500;
    both(&f, [sa, sb], p, |ch| ch.crossings.push(Crossing { t0: t, t1: t + 1500, depth: 8.0 }));
    let n0 = net.coord.zcl_rx.len();
    net.run_until(4000, "occupancy from both", |net| {
        reports(net, sa, OCC, 0, n0).contains(&1) && reports(net, sb, OCC, 0, n0).contains(&1)
    });
    assert!(net.now() - t <= 3000, "within 3 s of the crossing: {} ms", net.now() - t);
    net.run(100);
    assert_eq!(reports(&net, sa, SENSE, 0xFF43, n0), vec![1], "zone state");
    assert_eq!(reports(&net, sa, SENSE, 0xFF46, n0), vec![1], "the bit of the line");
    assert_eq!(read(&mut net, sa, OCC, 0x0000), Some(vec![1]));
    let n1 = net.coord.zcl_rx.len();
    net.run_until(15_000, "unoccupied", |net| reports(net, sa, OCC, 0, n1).contains(&0));
    let after = net.now() - t;
    assert!((5000..=12_000).contains(&after), "OFF 5 s after the last hit: {after} ms after the crossing began");
    assert_eq!(read(&mut net, sa, SENSE, 0xFF46), Some(vec![0, 0]));
}

#[test]
fn light_changes_are_not_motion() {
    let (mut net, f, n, [sa, sb]) = two();
    let p = ports(&net, n);
    for a in [sa, sb] {
        assert_eq!(write(&mut net, a, SENSE, 0x0000, T_U8, &[45]), 0);
    }
    past_holdoff(&mut net);
    net.run(10_000);
    let n0 = net.coord.zcl_rx.len();
    both(&f, [sa, sb], p, |ch| ch.offset = -8.0);
    net.coord.zcl(sb, 0x0006, 0x01, 0x00, &[]);
    net.run(12_000);
    assert!(!net.nodes[n[1]].on);
    assert!(!reports(&net, sa, OCC, 0, n0).contains(&1) && !reports(&net, sb, OCC, 0, n0).contains(&1), "no occupancy");
    both(&f, [sa, sb], p, |ch| ch.offset = 0.0);
    net.run(4000);
    assert!(reports(&net, sa, OCC, 0, n0).contains(&1), "the jump alone is seen");
}

#[test]
fn the_rate_is_set_from_zcl_and_0_stops_the_probes() {
    let (mut net, _, n, [sa, _]) = two();
    let [pa, _] = ports(&net, n);
    past_holdoff(&mut net);
    assert_eq!(write(&mut net, sa, SENSE, 0x0001, T_U8, &[0]), 0);
    let t0 = net.now();
    net.run(5000);
    assert!(probes(&net, pa, t0).is_empty());
    assert_eq!(write(&mut net, sa, SENSE, 0x0001, T_U8, &[10]), 0);
    let t0 = net.now();
    net.run(5000);
    let k = probes(&net, pa, t0).len();
    assert!((42..=58).contains(&k), "~10/s: {k} in 5 s");
    assert!(probes(&net, pa, t0).iter().all(|p| p[18] == 10), "and the probes say so");
}

#[test]
fn calibration_from_the_attribute_counts_down_and_sets_the_line() {
    let (mut net, f, n, [sa, sb]) = two();
    let p = ports(&net, n);
    let k = model::link("K1", "K3");
    both(&f, [sa, sb], p, |ch| *ch = Channel::of(&k, k.sigma_lo));
    past_holdoff(&mut net);
    net.run(8000);
    assert_eq!(write(&mut net, sa, SENSE, 0x0003, T_U8, &[30]), 0);
    net.run(2000);
    let left = read(&mut net, sa, SENSE, 0x0003).unwrap()[0];
    assert!((26..=29).contains(&left), "counting down: {left} s");
    net.run(30_000);
    assert_eq!(read(&mut net, sa, SENSE, 0x0003), Some(vec![0]));
    let sens = read(&mut net, sa, SENSE, 0x0005).unwrap();
    assert_eq!(u16::from_le_bytes([sens[0], sens[1]]), sb);
    let (s, anchor) = (sens[2], sens[3]);
    assert!((20..45).contains(&s) && s == anchor, "less sensitive than 45, the anchor with it: {s}/{anchor}");
    let n0 = net.coord.zcl_rx.len();
    net.run(60_000);
    assert!(!reports(&net, sa, OCC, 0, n0).contains(&1), "a minute of the empty room without a report");
    net.power_cycle(n[0]);
    net.run(1000);
    assert_eq!(read(&mut net, sa, SENSE, 0x0005).unwrap(), sens);
}

#[test]
fn the_configuration_survives_a_power_cycle() {
    let (mut net, _, n, [sa, _]) = two();
    assert_eq!(write(&mut net, sa, OCC, 0x0010, T_U16, &[7, 0]), 0);
    assert_eq!(write(&mut net, sa, SENSE, 0x0001, T_U8, &[3]), 0);
    assert_eq!(write(&mut net, sa, SENSE, 0x0004, T_U8, &[1]), 0);
    assert_eq!(write(&mut net, sa, SENSE, 0x0000, T_U8, &[40]), 0);
    net.power_cycle(n[0]);
    net.run(1000);
    assert_eq!(read(&mut net, sa, OCC, 0x0010), Some(vec![7, 0]));
    assert_eq!(read(&mut net, sa, SENSE, 0x0001), Some(vec![3]));
    assert_eq!(read(&mut net, sa, SENSE, 0x0004), Some(vec![1]));
    assert_eq!(read(&mut net, sa, SENSE, 0x0000), Some(vec![40]));
}

#[test]
fn writes_out_of_range_of_the_wrong_type_or_to_the_state_are_refused() {
    let (mut net, _, _, [sa, _]) = two();
    assert_eq!(write(&mut net, sa, SENSE, 0x0000, T_U8, &[101]), 0x87, "s 0..=100");
    assert_eq!(write(&mut net, sa, SENSE, 0x0001, T_U8, &[11]), 0x87, "0..=10 probes/s");
    assert_eq!(write(&mut net, sa, SENSE, 0x0004, T_U8, &[2]), 0x87);
    assert_eq!(write(&mut net, sa, OCC, 0x0010, T_U16, &[0, 0]), 0x87, "an OFF delay of 1 s at least");
    assert_eq!(write(&mut net, sa, SENSE, 0x0000, T_U16, &[40, 0]), 0x8D, "INVALID_DATA_TYPE");
    assert_eq!(write(&mut net, sa, SENSE, 0xFF43, T_U8, &[1]), 0x88, "READ_ONLY");
    assert_eq!(write(&mut net, sa, OCC, 0x0000, 0x18, &[1]), 0x88);
    assert_eq!(read(&mut net, sa, SENSE, 0x0000), Some(vec![SENSE_DEFAULT_S]), "nothing written");
    let seq = net.coord.zcl_mfr(sa, SENSE, 0x00, 0x1234, 0x00, &[0, 0]);
    net.run(50);
    assert_eq!(net.coord.last_zcl_mfr(sa, SENSE, 0x0B).unwrap(), vec![0x1C, 0x34, 0x12, seq, 0x0B, 0x00, 0x81]);
}

#[test]
fn a_report_goes_every_half_hour_without_a_change() {
    let (mut net, f, n, [sa, sb]) = two();
    let p = ports(&net, n);
    both(&f, [sa, sb], p, |ch| ch.sigma = 0.3);
    let n0 = net.coord.zcl_rx.len();
    net.run(29 * 60_000);
    assert!(reports(&net, sa, OCC, 0, n0).is_empty(), "nothing to report");
    net.run(2 * 60_000);
    assert_eq!(reports(&net, sa, OCC, 0, n0), vec![0], "the half-hourly report");
    assert_eq!(reports(&net, sa, SENSE, 0xFF46, n0), vec![0]);
}

#[test]
fn leaving_and_reset_to_factory_defaults_bring_the_defaults_back() {
    let (mut net, _, n, [sa, _]) = two();
    write(&mut net, sa, SENSE, 0x0000, T_U8, &[30]);
    write(&mut net, sa, SENSE, 0x0001, T_U8, &[2]);
    net.coord.zcl(sa, 0x0000, 0x01, 0x00, &[]);
    net.run(50);
    assert_eq!(read(&mut net, sa, SENSE, 0x0000), Some(vec![SENSE_DEFAULT_S]));
    assert_eq!(read(&mut net, sa, SENSE, 0x0001), Some(vec![0]), "the sensing off again");
    assert_eq!(net.air.borrow().ports[net.nodes[n[0]].id].probe_pan, 0xFFFF);
    assert_eq!(read(&mut net, sa, SENSE, 0x0006), Some(vec![1]), "the automatic calibration on again");
    net.power_cycle(n[0]);
    net.run(500);
    assert_eq!(read(&mut net, sa, SENSE, 0x0000), Some(vec![SENSE_DEFAULT_S]), "and stored so");

    write(&mut net, sa, SENSE, 0x0000, T_U8, &[30]);
    net.coord.permit_join = false;
    net.nodes[n[0]].stack.leave();
    net.run(100);
    assert!(!net.nodes[n[0]].stack.is_joined());
    assert_eq!(net.nodes[n[0]].stack.sense_core().s, SENSE_DEFAULT_S);
    net.power_cycle(n[0]);
    assert_eq!(net.nodes[n[0]].stack.sense_core().s, SENSE_DEFAULT_S, "nothing stored");
}

#[test]
fn no_probes_while_an_update_over_ble_runs() {
    let (mut net, _, n, _) = two();
    let [pa, _] = ports(&net, n);
    past_holdoff(&mut net);
    net.nodes[n[0]].stack.sense_pause(true);
    net.run(300);
    let t0 = net.now();
    net.run(5000);
    assert!(probes(&net, pa, t0).is_empty());
    net.nodes[n[0]].stack.sense_pause(false);
    let t0 = net.now();
    net.run(2000);
    assert!(!probes(&net, pa, t0).is_empty());
}

#[test]
fn the_corridor_after_calibration_reports_a_crossing_and_nothing_else() {
    let mut net = Net::new();
    net.identity = IDENTITY_SENSE;
    let nodes = [net.join_node(), net.join_node(), net.join_node()];
    let f = field(&mut net);
    let names = ["K1", "K2", "K3"];
    let shorts: Vec<u16> = nodes.iter().map(|&n| net.nodes[n].short()).collect();
    for &s in &shorts {
        switch(&mut net, s, true);
    }
    let ports: Vec<usize> = nodes.iter().map(|&n| net.nodes[n].id).collect();
    for i in 0..3 {
        for j in 0..3 {
            if i != j {
                let l = model::link(names[i], names[j]);
                let mut ch = Channel::of(&l, l.sigma_lo);
                ch.drift_per_h = model::DRIFT_DB / 3.2;
                f.borrow_mut().links.insert((shorts[j], ports[i]), ch);
            }
        }
    }
    past_holdoff(&mut net);
    net.run(5000);
    for &s in &shorts {
        assert_eq!(write(&mut net, s, SENSE, 0x0003, T_U8, &[60]), 0);
        assert_eq!(write(&mut net, s, OCC, 0x0010, T_U16, &[5, 0]), 0);
    }
    net.run(62_000);
    for &s in &shorts {
        let sens = read(&mut net, s, SENSE, 0x0005).unwrap();
        assert_eq!(sens.len(), 8, "two lines at every bulb");
    }
    let n0 = net.coord.zcl_rx.len();
    net.run(20 * 60_000);
    for &s in &shorts {
        assert!(!reports(&net, s, OCC, 0, n0).contains(&1), "{s:#06x}: occupancy in the empty corridor");
    }
    let t = net.now() + 200;
    for (i, j) in [(0, 1), (1, 0)] {
        f.borrow_mut().links.get_mut(&(shorts[j], ports[i])).unwrap().crossings.push(Crossing { t0: t, t1: t + 1500, depth: 8.0 });
    }
    let n1 = net.coord.zcl_rx.len();
    net.run(4000);
    assert!(reports(&net, shorts[0], OCC, 0, n1).contains(&1) && reports(&net, shorts[1], OCC, 0, n1).contains(&1), "K1 and K2 saw it");
    assert!(!reports(&net, shorts[2], OCC, 0, n1).contains(&1), "K3 was not on that line");
}

#[test]
fn the_automatic_calibration_is_on_until_a_sensitivity_is_given_by_hand() {
    let (mut net, _, n, [sa, _]) = two();
    assert_eq!(read(&mut net, sa, SENSE, 0x0006), Some(vec![1]), "on out of the box");
    assert!(net.nodes[n[0]].stack.sense_core().auto_cal);
    assert_eq!(write(&mut net, sa, SENSE, 0x0000, T_U8, &[35]), 0);
    assert_eq!(read(&mut net, sa, SENSE, 0x0006), Some(vec![0]));
    assert_eq!(write(&mut net, sa, SENSE, 0x0006, T_U8, &[1]), 0);
    assert_eq!(write(&mut net, sa, SENSE, 0x0006, T_U8, &[2]), 0x87, "0 or 1");
    net.power_cycle(n[0]);
    net.run(500);
    assert_eq!(read(&mut net, sa, SENSE, 0x0006), Some(vec![1]));
    assert_eq!(read(&mut net, sa, SENSE, 0x0000), Some(vec![35]));
}

#[test]
fn out_of_the_box_the_sensing_is_off_and_the_air_is_as_without_it() {
    let run = |identity: Identity| {
        let mut net = Net::new();
        net.identity = identity;
        let n = [net.join_node(), net.join_node()];
        net.run(35 * 60_000);
        let ports = [net.nodes[n[0]].id, net.nodes[n[1]].id];
        let log = net.air.borrow().log.clone();
        let sent: Vec<(u32, usize, Vec<u8>)> = log.into_iter().filter(|e| ports.contains(&e.1)).collect();
        (net, n, sent)
    };
    let (_, _, without) = run(IDENTITY);
    let (mut net, n, with) = run(IDENTITY_SENSE);
    assert!(without.len() > 20, "the bulbs sent something: {}", without.len());
    assert_eq!(with, without, "an image with the sensing off sends what one without it does");
    let sa = net.nodes[n[0]].short();
    assert!(probes(&net, net.nodes[n[0]].id, 0).is_empty());
    let none = |net: &Net| net.coord.zcl_rx.iter().filter(|(_, c, _)| *c == OCC || *c == SENSE).count();
    assert_eq!(none(&net), 0, "no report");
    assert_eq!(read(&mut net, sa, OCC, 0x0000), Some(vec![0]), "Occupancy reads 0");
    assert_eq!(read(&mut net, sa, SENSE, 0x0002), Some(vec![]), "no lines");
}

#[test]
fn the_rate_switches_the_sensing_on_and_off() {
    let (mut net, _, n, [sa, sb]) = two_off();
    let [pa, pb] = ports(&net, n);
    past_holdoff(&mut net);
    switch(&mut net, sa, true);
    assert_eq!(net.air.borrow().ports[pa].probe_pan, PROBE_PAN);
    assert_eq!(net.air.borrow().ports[pb].probe_pan, 0xFFFF);
    let t0 = net.now();
    net.run(5000);
    let k = probes(&net, pa, t0).len();
    assert!((20..=30).contains(&k), "~5/s: {k} in 5 s");
    assert!(probes(&net, pb, t0).is_empty());
    assert_eq!(read(&mut net, sb, SENSE, 0x0002), Some(vec![]), "B takes no samples");
    assert_eq!(read(&mut net, sa, SENSE, 0x0002), Some(vec![]), "nobody probes for A");
    switch(&mut net, sb, true);
    net.run(5000);
    assert_eq!(read(&mut net, sa, SENSE, 0x0002).unwrap().len(), 6);
    assert_eq!(read(&mut net, sb, SENSE, 0x0002).unwrap().len(), 6);
    switch(&mut net, sa, false);
    assert_eq!(net.air.borrow().ports[pa].probe_pan, 0xFFFF);
    let (t0, n0) = (net.now(), net.coord.zcl_rx.len());
    net.run(31 * 60_000);
    assert!(probes(&net, pa, t0).is_empty());
    assert!(reports(&net, sa, OCC, 0, n0).is_empty() && reports(&net, sa, SENSE, 0xFF46, n0).is_empty(), "nothing from A");
    assert_eq!(reports(&net, sb, OCC, 0, n0), vec![0], "B, on, still reports every half hour");
    assert_eq!(read(&mut net, sa, SENSE, 0x0005).unwrap().len(), 4);
}

#[test]
fn switching_off_ends_the_occupancy() {
    let (mut net, f, n, [sa, sb]) = two();
    let p = ports(&net, n);
    past_holdoff(&mut net);
    net.run(10_000);
    let t = net.now() + 500;
    both(&f, [sa, sb], p, |ch| ch.crossings.push(Crossing { t0: t, t1: t + 1500, depth: 8.0 }));
    let n0 = net.coord.zcl_rx.len();
    net.run_until(4000, "occupancy", |net| reports(net, sa, OCC, 0, n0).contains(&1));
    let n1 = net.coord.zcl_rx.len();
    switch(&mut net, sa, false);
    assert_eq!(reports(&net, sa, OCC, 0, n1), vec![0], "told the occupancy is over");
    assert_eq!(read(&mut net, sa, OCC, 0x0000), Some(vec![0]));
    assert_eq!(read(&mut net, sa, SENSE, 0xFF46), Some(vec![0, 0]));
}

#[test]
fn the_detectors_day_starts_when_the_sensing_is_switched_on() {
    let (mut net, _, n, [sa, _]) = two_off();
    let at_start = net.nodes[n[0]].stack.sense_core().t_hour;
    net.run(2 * 3_600_000);
    let c = *net.nodes[n[0]].stack.sense_core();
    assert_eq!((c.hour, c.t_hour), (0, at_start), "off: the detector stood still");
    let t = net.now();
    switch(&mut net, sa, true);
    let c = *net.nodes[n[0]].stack.sense_core();
    assert_eq!(c.hour, 0);
    assert!((t..=net.now()).contains(&c.t_hour), "the hours from the switch-on: {} ({t})", c.t_hour);
}

#[test]
fn a_calibration_needs_the_sensing_on() {
    let (mut net, _, _, [sa, _]) = two_off();
    assert_eq!(write(&mut net, sa, SENSE, 0x0003, T_U8, &[60]), 0x87, "off: nothing to record");
    assert_eq!(write(&mut net, sa, SENSE, 0x0003, T_U8, &[0]), 0, "an abort is fine");
    assert_eq!(read(&mut net, sa, SENSE, 0x0003), Some(vec![0]));
    switch(&mut net, sa, true);
    assert_eq!(write(&mut net, sa, SENSE, 0x0003, T_U8, &[60]), 0);
    net.run(1000);
    assert!(read(&mut net, sa, SENSE, 0x0003).unwrap()[0] > 50);
    switch(&mut net, sa, false);
    assert_eq!(read(&mut net, sa, SENSE, 0x0003), Some(vec![0]));
}

#[test]
fn the_switch_is_stored_and_an_update_from_an_image_without_sensing_comes_up_off() {
    let (mut net, _, n, [sa, sb]) = two_off();
    switch(&mut net, sa, true);
    net.power_cycle(n[0]);
    net.power_cycle(n[1]);
    net.run(1000);
    assert_eq!(read(&mut net, sa, SENSE, 0x0001), Some(vec![SENSE_ON_RATE]), "on stays on");
    assert_eq!(read(&mut net, sb, SENSE, 0x0001), Some(vec![0]), "off stays off");
    assert_eq!(net.air.borrow().ports[net.nodes[n[0]].id].probe_pan, PROBE_PAN);
    switch(&mut net, sa, false);
    net.power_cycle(n[0]);
    net.run(1000);
    assert_eq!(read(&mut net, sa, SENSE, 0x0001), Some(vec![0]), "and off stays off");

    let mut net = Net::new();
    let k = net.join_node();
    net.identity = IDENTITY_SENSE;
    net.power_cycle(k);
    net.run(1000);
    let s = net.nodes[k].short();
    assert_eq!(read(&mut net, s, SENSE, 0x0001), Some(vec![0]), "updated: off");
    assert_eq!(net.air.borrow().ports[net.nodes[k].id].probe_pan, 0xFFFF);
    net.run(2 * 60_000);
    assert!(probes(&net, net.nodes[k].id, 0).is_empty());
}

#[test]
fn occupancy_reports_can_be_configured() {
    let (mut net, f, n, [sa, sb]) = two_off();
    let ext = net.air.borrow().ports[net.nodes[n[0]].id].ext;
    let mut bind = ext.to_le_bytes().to_vec();
    bind.push(1);
    bind.extend_from_slice(&OCC.to_le_bytes());
    bind.push(3);
    bind.extend_from_slice(&COORD_EXT.to_le_bytes());
    bind.push(1);
    net.coord.zdo(sa, 0x0021, &bind);
    net.run(2000);
    let seq = net.coord.zcl(sa, OCC, 0x00, 0x06, &[0x00, 0x00, 0x00, 0x18, 0, 0, 60, 0]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(sa, OCC, 0x07).unwrap(), vec![0x18, seq, 0x07, 0x00], "configured");
    let n0 = net.coord.zcl_rx.len();
    net.run(61_000);
    assert_eq!(reports(&net, sa, OCC, 0, n0), vec![0], "the periodic report, the sensing off");
    assert!(probes(&net, net.nodes[n[0]].id, 0).is_empty(), "and still no probes");
    switch(&mut net, sa, true);
    switch(&mut net, sb, true);
    past_holdoff(&mut net);
    net.run(10_000);
    let p = ports(&net, n);
    let t = net.now() + 500;
    both(&f, [sa, sb], p, |ch| ch.crossings.push(Crossing { t0: t, t1: t + 1500, depth: 8.0 }));
    let n1 = net.coord.zcl_rx.len();
    net.run(4000);
    assert!(reports(&net, sa, OCC, 0, n1).iter().filter(|v| **v == 1).count() >= 2, "both reports of the change");
}

#[test]
fn a_configuration_stored_by_the_previous_version_is_read_with_the_automatic_calibration_on() {
    let (mut net, _, n, [sa, _]) = two();
    past_holdoff(&mut net);
    net.run(30_000);
    assert_eq!(write(&mut net, sa, OCC, 0x0010, T_U16, &[9, 0]), 0);
    assert_eq!(write(&mut net, sa, SENSE, 0x0000, T_U8, &[55]), 0);
    assert_eq!(write(&mut net, sa, SENSE, 0x0006, T_U8, &[0]), 0);
    let lines = read(&mut net, sa, SENSE, 0x0005).unwrap();
    assert!(!lines.is_empty(), "a line to the other bulb");
    let id = net.nodes[n[0]].id;
    let v3 = net.air.borrow().ports[id].nv[&(NvItem::Sense as u8)].clone();
    assert_eq!((v3[0], v3[6]), (3, 0));
    let mut v2 = v3.clone();
    v2.remove(6);
    v2[0] = 2;
    net.air.borrow_mut().ports[id].nv.insert(NvItem::Sense as u8, v2);
    net.power_cycle(n[0]);
    net.run(1000);
    assert_eq!(read(&mut net, sa, OCC, 0x0010), Some(vec![9, 0]));
    assert_eq!(read(&mut net, sa, SENSE, 0x0000), Some(vec![55]));
    assert_eq!(read(&mut net, sa, SENSE, 0x0001), Some(vec![SENSE_ON_RATE]));
    assert_eq!(read(&mut net, sa, SENSE, 0x0006), Some(vec![1]));
    assert_eq!(read(&mut net, sa, SENSE, 0x0005), Some(lines.clone()), "the lines come back");
    let mut short = v3.clone();
    short.pop();
    net.air.borrow_mut().ports[id].nv.insert(NvItem::Sense as u8, short);
    net.power_cycle(n[0]);
    net.run(1000);
    assert_eq!(read(&mut net, sa, SENSE, 0x0000), Some(vec![SENSE_DEFAULT_S]), "a record of the wrong length is not read");
    assert_eq!(write(&mut net, sa, SENSE, 0x0077, T_U8, &[1]), 0x86);
}

#[test]
fn the_manufacturer_cluster_has_revision_one() {
    let (mut net, _, _, [sa, _]) = two_off();
    assert_eq!(read(&mut net, sa, SENSE, 0xFFFD), Some(vec![1, 0]));
}

#[test]
fn a_busy_channel_holds_the_probes_back() {
    let (mut net, _, n, _) = two();
    let [pa, _] = ports(&net, n);
    past_holdoff(&mut net);
    net.run(2_000);
    net.air.borrow_mut().ports[pa].busy = 1_000_000;
    let t = net.now();
    net.run(2_000);
    assert!(probes(&net, pa, t).is_empty());
    net.air.borrow_mut().ports[pa].busy = 0;
    let t = net.now();
    net.run(2_000);
    assert!(probes(&net, pa, t).len() >= 5, "and they go again once it is free");
}

#[test]
fn a_probe_waits_while_frames_are_still_coming_in() {
    let (mut net, _, n, [sa, _]) = two();
    let [pa, _] = ports(&net, n);
    past_holdoff(&mut net);
    let t = net.now();
    for _ in 0..400 {
        for _ in 0..6 {
            net.coord.nwk_data(sa, &[0x01, 0x55, 0x2A]);
        }
        net.run(1);
    }
    assert!(probes(&net, pa, t).len() <= 1, "{} probes in a flood", probes(&net, pa, t).len());
    let t = net.now();
    net.run(2_000);
    assert!(probes(&net, pa, t).len() >= 5);
}

#[test]
fn a_reset_to_factory_defaults_while_occupied_reports_the_room_empty() {
    let (mut net, f, n, [sa, sb]) = two();
    let p = ports(&net, n);
    past_holdoff(&mut net);
    net.run(10_000);
    let t = net.now() + 500;
    both(&f, [sa, sb], p, |ch| ch.crossings.push(Crossing { t0: t, t1: t + 1500, depth: 8.0 }));
    let n0 = net.coord.zcl_rx.len();
    net.run_until(4000, "occupancy", |net| reports(net, sa, OCC, 0, n0).contains(&1));
    let n1 = net.coord.zcl_rx.len();
    net.coord.zcl(sa, 0x0000, 0x01, 0x00, &[]);
    net.run(50);
    assert_eq!(reports(&net, sa, OCC, 0, n1), vec![0]);
}

#[test]
fn a_probe_of_another_network_is_taken_by_the_sensing_and_other_frames_are_left_to_the_stack() {
    let (mut net, _, n, [sa, _]) = two();
    let port = net.nodes[n[0]].id;
    past_holdoff(&mut net);
    net.run(1_000);
    let mut probe = probes(&net, net.nodes[n[1]].id, 0).last().cloned().expect("a probe of the other bulb");
    probe[9 + 3] ^= 0xFF;
    let before = read(&mut net, sa, 0x0B05, 0x0100);
    net.air.borrow_mut().transmit(port, &probe);
    net.run(20);
    assert_eq!(read(&mut net, sa, 0x0B05, 0x0100), before, "a foreign probe is not counted as received");
    let other = [0x03, 0x08, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0x07];
    net.air.borrow_mut().transmit(port, &other);
    net.run(20);
    assert_ne!(read(&mut net, sa, 0x0B05, 0x0100), before, "a beacon request goes to the stack");
}

#[test]
fn the_automatic_calibration_turned_off_stays_off_over_a_power_cycle_and_a_record_of_another_version_is_not_taken() {
    let (mut net, _, n, [sa, _]) = two();
    assert_eq!(write(&mut net, sa, SENSE, 0x0000, T_U8, &[40]), 0);
    assert_eq!(read(&mut net, sa, SENSE, 0x0006), Some(vec![0]));
    net.power_cycle(n[0]);
    net.run(1000);
    assert_eq!(read(&mut net, sa, SENSE, 0x0006), Some(vec![0]), "still off");
    let id = net.nodes[n[0]].id;
    let item = NvItem::Sense as u8;
    let mut stored = net.air.borrow().ports[id].nv[&item].clone();
    stored[0] ^= 0x40;
    net.air.borrow_mut().ports[id].nv.insert(item, stored);
    net.power_cycle(n[0]);
    net.run(1000);
    assert_eq!(read(&mut net, sa, SENSE, 0x0000), Some(vec![SENSE_DEFAULT_S]), "the defaults");
}

#[test]
fn the_lines_of_two_neighbours_are_listed_with_their_lengths() {
    let mut net = Net::new();
    net.identity = IDENTITY_SENSE;
    let nodes: Vec<usize> = (0..3).map(|_| net.join_node()).collect();
    let shorts: Vec<u16> = nodes.iter().map(|&n| net.nodes[n].short()).collect();
    for &s in &shorts {
        switch(&mut net, s, true);
    }
    past_holdoff(&mut net);
    net.run(8_000);
    assert_eq!(read(&mut net, shorts[0], SENSE, 0x0002).map(|l| l.len()), Some(12));
    assert_eq!(read(&mut net, shorts[0], SENSE, 0x0005).map(|l| l.len()), Some(8));
}
