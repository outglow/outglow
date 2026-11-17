use std::collections::BTreeMap;
use std::fmt::Write;

use anyhow::{bail, Result};
use serde::Serialize;
use zigbee::stack::blackbox::{kind, HEADER};

pub const TOTAL_AT: usize = 20;

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Header {
    pub version: u8,
    pub neighbors: u8,
    pub link_entry: u8,
    pub ring_cap: u8,
    pub ring_len: u8,
    pub ring_entry: u8,
    pub rows_cap: u16,
    pub rows_len: u16,
    pub row_entry: u8,
    pub row_s: u16,
    pub now_ms: u32,
    pub ring_written: u32,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Link {
    pub address: String,
    pub frames: Vec<(u8, bool)>,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(untagged)]
pub enum Status {
    Name(&'static str),
    Code(u8),
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Frame {
    pub ago_ms: u32,
    pub mac_dst: String,
    pub dst: String,
    pub src: String,
    pub nwk_type: &'static str,
    pub secured: bool,
    pub broadcast: bool,
    pub relayed: bool,
    pub what: Option<&'static str>,
    pub id: u16,
    pub name: Option<&'static str>,
    pub seq: u8,
    pub attempts: u8,
    pub status: Status,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Row {
    pub minute: u16,
    pub tx: u16,
    pub rx: u16,
    pub retries: u8,
    pub fails: u8,
    pub coord_cost: u8,
    pub coord_measured: bool,
    pub coord_direct: bool,
    pub route_disc: u8,
    pub coord_delivery: u8,
    pub stale: u8,
    pub noise: i8,
}

impl Row {
    pub const FIELDS: [&'static str; 12] = [
        "minute", "tx", "rx", "retries", "fails", "coord_cost", "coord_measured", "coord_direct", "route_disc",
        "coord_delivery", "stale", "noise",
    ];

    pub fn cells(&self) -> Vec<String> {
        let b = |v: bool| if v { "True" } else { "False" }.to_string();
        vec![
            self.minute.to_string(),
            self.tx.to_string(),
            self.rx.to_string(),
            self.retries.to_string(),
            self.fails.to_string(),
            self.coord_cost.to_string(),
            b(self.coord_measured),
            b(self.coord_direct),
            self.route_disc.to_string(),
            self.coord_delivery.to_string(),
            self.stale.to_string(),
            self.noise.to_string(),
        ]
    }
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct BlackBox {
    pub header: Header,
    pub links: Vec<Link>,
    pub ring: Vec<Frame>,
    pub rows: Vec<Row>,
}

const NWK_COMMANDS: [&str; 12] = [
    "route request",
    "route reply",
    "network status",
    "leave",
    "route record",
    "rejoin request",
    "rejoin response",
    "link status",
    "network report",
    "network update",
    "end device timeout request",
    "end device timeout response",
];

const CLUSTERS: [(u16, &str); 21] = [
    (0x0000, "Basic"),
    (0x0003, "Identify"),
    (0x0004, "Groups"),
    (0x0005, "Scenes"),
    (0x0006, "On/Off"),
    (0x0008, "Level"),
    (0x0019, "OTA"),
    (0x0300, "Color"),
    (0x0B05, "Diagnostics"),
    (0x000C, "Analog Input"),
    (0x0010, "Binary Output"),
    (0x000D, "Analog Output"),
    (0x0013, "ZDO Device_annce"),
    (0x8031, "ZDO Mgmt_Lqi_rsp"),
    (0x8032, "ZDO Mgmt_Rtg_rsp"),
    (0x8000, "ZDO NWK_addr_rsp"),
    (0x8001, "ZDO IEEE_addr_rsp"),
    (0x8004, "ZDO Simple_Desc_rsp"),
    (0x8005, "ZDO Active_EP_rsp"),
    (0x0021, "Green Power"),
    (0xFC5F, "BLE settings"),
];

fn no_name(_: u16) -> Option<&'static str> {
    None
}

fn nwk_command(id: u16) -> Option<&'static str> {
    NWK_COMMANDS.get(usize::from(id.checked_sub(1)?)).copied()
}

fn cluster(id: u16) -> Option<&'static str> {
    CLUSTERS.iter().find(|(c, _)| *c == id).map(|(_, n)| *n)
}

const NAMES: [fn(u16) -> Option<&'static str>; 4] = [no_name, nwk_command, cluster, cluster];

const STATUS: [&str; 4] = ["ok", "no ack", "channel busy", "held for child"];

fn u16le(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32le(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn addr(v: u16) -> String {
    format!("0x{v:04x}")
}

pub fn total_len(page0: &[u8]) -> Option<usize> {
    (page0.len() >= TOTAL_AT + 2).then(|| u16le(page0, TOTAL_AT) as usize)
}

pub fn decode(b: &[u8]) -> Result<BlackBox> {
    if b.len() < HEADER {
        bail!("black box of {} bytes: shorter than its header", b.len());
    }
    let h = Header {
        version: b[0],
        neighbors: b[1],
        link_entry: b[2],
        ring_cap: b[3],
        ring_len: b[4],
        ring_entry: b[5],
        rows_cap: u16le(b, 6),
        rows_len: u16le(b, 8),
        row_entry: b[10],
        row_s: b[11] as u16 * 10,
        now_ms: u32le(b, 12),
        ring_written: u32le(b, 16),
    };
    let (le, re, we) = (h.link_entry as usize, h.ring_entry as usize, h.row_entry as usize);
    let need = HEADER + h.neighbors as usize * le + h.ring_cap as usize * re + h.rows_len as usize * we;
    let small = (h.neighbors > 0 && le < 4) || (h.ring_len > 0 && re < 15) || (h.rows_len > 0 && we < 12);
    if b.len() < need || small {
        bail!("black box of {} bytes does not hold what its header describes ({need} bytes)", b.len());
    }
    let mut at = HEADER;
    let mut links = Vec::new();
    for _ in 0..h.neighbors {
        let e = &b[at..at + le];
        at += le;
        let (short, n) = (u16le(e, 0), e[2] as usize);
        if short == 0xFFFF || n == 0 {
            continue;
        }
        if 4 + n.div_ceil(2) > le {
            bail!("neighbour 0x{short:04x}: {n} frames do not fit its entry");
        }
        let frames = (0..n)
            .map(|i| (e[4 + i / 2] >> (4 * (i % 2))) & 0x0F)
            .map(|f| (f & 7, f & 8 != 0))
            .collect();
        links.push(Link { address: addr(short), frames });
    }
    let mut ring = Vec::new();
    let id_shift = kind::ID_COMMAND.trailing_zeros();
    for i in 0..(h.ring_len.min(h.ring_cap)) as usize {
        let e = &b[at + i * re..at + (i + 1) * re];
        let k = e[10];
        let what = (k >> id_shift) & 3;
        let id = u16le(e, 11);
        let st = e[14] >> 4;
        ring.push(Frame {
            ago_ms: h.now_ms.wrapping_sub(u32le(e, 0)),
            mac_dst: addr(u16le(e, 4)),
            dst: addr(u16le(e, 6)),
            src: addr(u16le(e, 8)),
            nwk_type: ["data", "command", "?", "inter-PAN"][(k & 3) as usize],
            secured: k & kind::SECURED != 0,
            broadcast: k & kind::BROADCAST != 0,
            relayed: k & kind::RELAYED != 0,
            what: [None, Some("nwk_command"), Some("cluster"), Some("aps_ack")][what as usize],
            id,
            name: NAMES[what as usize](id),
            seq: e[13],
            attempts: e[14] & 15,
            status: STATUS.get(st as usize).map_or(Status::Code(st), |s| Status::Name(s)),
        });
    }
    at += h.ring_cap as usize * re;
    let rows = (0..h.rows_len as usize)
        .map(|i| {
            let r = &b[at + i * we..at + (i + 1) * we];
            Row {
                minute: u16le(r, 0),
                tx: u16le(r, 2),
                rx: u16le(r, 4),
                retries: r[6],
                fails: r[7],
                coord_cost: r[8] & 7,
                coord_measured: r[8] & 8 != 0,
                coord_direct: r[8] & 16 != 0,
                route_disc: r[8] >> 5,
                coord_delivery: (r[9] as f64 * 100.0 / 255.0).round() as u8,
                stale: r[10],
                noise: r[11] as i8,
            }
        })
        .collect();
    Ok(BlackBox { header: h, links, ring, rows })
}

fn dict_repr(m: &BTreeMap<usize, usize>) -> String {
    format!("{{{}}}", m.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join(", "))
}

pub fn report(bx: &BlackBox, missing: &[usize]) -> String {
    let h = &bx.header;
    let mut o = String::new();
    let w = &mut o;
    if !missing.is_empty() {
        let list = missing.iter().map(usize::to_string).collect::<Vec<_>>().join(", ");
        let _ = writeln!(w, "(pages not read: [{list}])");
    }
    let _ = writeln!(
        w,
        "== black box v{}: {} frames sent since the start, {} rows of {} s",
        h.version, h.ring_written, h.rows_len, h.row_s
    );
    let _ = writeln!(
        w,
        "== unicasts per neighbour, oldest first (digit = attempts, x = not acknowledged), and the runs of lost attempts"
    );
    for l in &bx.links {
        let seq: String = l.frames.iter().map(|&(a, ok)| if ok { char::from(b'0' + a) } else { 'x' }).collect();
        let mut att = Vec::new();
        for &(a, ok) in &l.frames {
            let lost = if ok { a.saturating_sub(1) } else { a };
            att.extend(std::iter::repeat_n(false, lost as usize));
            if ok {
                att.push(true);
            }
        }
        let mut runs = BTreeMap::new();
        let mut run = 0;
        for &ok in att.iter().chain([true].iter()) {
            if ok {
                if run > 0 {
                    *runs.entry(run).or_insert(0) += 1;
                }
                run = 0;
            } else {
                run += 1;
            }
        }
        let p = if att.is_empty() { 0.0 } else { att.iter().filter(|&&ok| ok).count() as f64 / att.len() as f64 };
        let runs = if runs.is_empty() { "-".into() } else { dict_repr(&runs) };
        let _ = writeln!(
            w,
            "   {}: {:>2} frames, {:>3} attempts, delivered {:.0}%, runs of losses {runs}",
            l.address,
            l.frames.len(),
            att.len(),
            100.0 * p
        );
        let _ = writeln!(w, "      {seq}");
    }
    let _ = writeln!(w, "== frames sent, newest first");
    for f in bx.ring.iter().rev() {
        let what = match (f.name, f.what) {
            (Some(n), _) => n.to_string(),
            (None, Some(wh)) => format!("{wh} 0x{:04x}", f.id),
            (None, None) => String::new(),
        };
        let flags: String = [(f.relayed, 'R'), (f.broadcast, 'B'), (f.secured, 'S')]
            .iter()
            .map(|&(on, c)| if on { c } else { '-' })
            .collect();
        let status = match f.status {
            Status::Name(s) => s.to_string(),
            Status::Code(c) => c.to_string(),
        };
        let _ = writeln!(
            w,
            "   -{:8.1}s  to {}  {}->{}  {flags} {:7} {what:28} #{:<3} {} attempts, {status}",
            f.ago_ms as f64 / 1000.0,
            f.mac_dst,
            f.src,
            f.dst,
            f.nwk_type,
            f.seq,
            f.attempts
        );
    }
    let _ = writeln!(
        w,
        "== rows (every {} s): minute, unicasts sent, heard, retries, failures, coordinator, stale, noise",
        h.row_s
    );
    for r in &bx.rows {
        let c = format!(
            "{} cost {}{} p {}%",
            if r.coord_direct { "direct" } else { "via" },
            r.coord_cost,
            if r.coord_measured { "m" } else { "" },
            r.coord_delivery
        );
        let _ = writeln!(
            w,
            "   {:>5} min  tx {:>4}  rx {:>5}  retry {:>3}  fail {:>2}  {c:28} disc {}  stale {:>2}  noise {}",
            r.minute, r.tx, r.rx, r.retries, r.fails, r.route_disc, r.stale, r.noise
        );
    }
    o
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn unhex(s: &str) -> Result<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 || !s.is_ascii() {
        bail!("not hex: odd length or not ASCII");
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| anyhow::anyhow!("not hex: {e}")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOB: &str = include_str!("testdata/box.hex");
    const DECODED: &str = include_str!("testdata/box.decoded.json");
    const REPORT: &str = include_str!("testdata/box.report.txt");

    #[test]
    fn decodes_as_the_python_tool() {
        let b = unhex(BLOB).unwrap();
        assert_eq!(b.len(), zigbee::stack::blackbox::BOX_LEN);
        assert_eq!(total_len(&b), Some(zigbee::stack::blackbox::BOX_LEN));
        let bx = decode(&b).unwrap();
        assert_eq!(bx.header.version, zigbee::stack::blackbox::VERSION);
        let ours = serde_json::to_value(&bx).unwrap();
        let python: serde_json::Value = serde_json::from_str(DECODED).unwrap();
        assert_eq!(ours, python);
        assert!(!bx.links.is_empty() && !bx.ring.is_empty() && !bx.rows.is_empty());
    }

    #[test]
    fn reports_as_the_python_tool() {
        let bx = decode(&unhex(BLOB).unwrap()).unwrap();
        assert_eq!(report(&bx, &[]), REPORT);
        assert!(report(&bx, &[3, 7]).starts_with("(pages not read: [3, 7])\n"));
    }

    #[test]
    fn frame_fields() {
        let mut b = vec![0u8; HEADER + 16];
        b[3] = 1;
        b[4] = 1;
        b[5] = 16;
        b[12..16].copy_from_slice(&5000u32.to_le_bytes());
        let e = &mut b[HEADER..];
        e[0..4].copy_from_slice(&1500u32.to_le_bytes());
        e[4..6].copy_from_slice(&0x1234u16.to_le_bytes());
        e[10] = 1 | kind::SECURED | kind::RELAYED | kind::ID_COMMAND;
        e[11] = 8;
        e[13] = 42;
        e[14] = 3 | 5 << 4;
        let f = &decode(&b).unwrap().ring[0];
        assert_eq!(f.ago_ms, 3500);
        assert_eq!(f.mac_dst, "0x1234");
        assert_eq!((f.nwk_type, f.secured, f.broadcast, f.relayed), ("command", true, false, true));
        assert_eq!((f.what, f.name), (Some("nwk_command"), Some("link status")));
        assert_eq!((f.seq, f.attempts, f.status.clone()), (42, 3, Status::Code(5)));
    }

    #[test]
    fn names_of_commands_and_clusters() {
        assert_eq!(nwk_command(0), None);
        assert_eq!(nwk_command(1), Some("route request"));
        assert_eq!(nwk_command(12), Some("end device timeout response"));
        assert_eq!(nwk_command(13), None);
        assert_eq!(cluster(0x0000), Some("Basic"));
        assert_eq!(cluster(0xFC5F), Some("BLE settings"));
        assert_eq!(cluster(0x0007), None);
        assert_eq!(no_name(1), None);
    }

    fn ring_of(k: u8, id: u16, status: u8) -> Frame {
        let mut b = vec![0u8; HEADER + 15];
        b[3] = 1;
        b[4] = 1;
        b[5] = 15;
        let e = &mut b[HEADER..];
        e[10] = k;
        e[11..13].copy_from_slice(&id.to_le_bytes());
        e[14] = status << 4 | 1;
        decode(&b).unwrap().ring.remove(0)
    }

    #[test]
    fn every_kind_of_frame_and_its_line() {
        let at = |k: u8| k << kind::ID_COMMAND.trailing_zeros();
        let f = ring_of(at(0) | 3, 8, 1);
        assert_eq!((f.what, f.name, f.nwk_type, f.status.clone()), (None, None, "inter-PAN", Status::Name("no ack")));
        let f = ring_of(at(1) | 2, 13, 2);
        assert_eq!((f.what, f.name, f.nwk_type, f.status.clone()), (Some("nwk_command"), None, "?", Status::Name("channel busy")));
        let f = ring_of(at(2) | kind::BROADCAST, 0x0006, 3);
        assert_eq!((f.what, f.name, f.nwk_type, f.broadcast), (Some("cluster"), Some("On/Off"), "data", true));
        assert_eq!(f.status, Status::Name("held for child"));
        let f = ring_of(at(3), 0x0300, 4);
        assert_eq!((f.what, f.name, f.status.clone()), (Some("aps_ack"), Some("Color"), Status::Code(4)));
        let f = ring_of(at(2), 0x1234, 0);
        let bx = BlackBox {
            header: decode(&[0; HEADER]).unwrap().header,
            links: vec![],
            ring: vec![ring_of(at(0), 8, 1), f, ring_of(at(1) | kind::SECURED | kind::RELAYED | kind::BROADCAST, 8, 7)],
            rows: vec![],
        };
        let r = report(&bx, &[]);
        let lines: Vec<&str> = r.lines().filter(|l| l.starts_with("   -")).collect();
        assert_eq!(lines[0], "   -     0.0s  to 0x0000  0x0000->0x0000  RBS data    link status                  #0   1 attempts, 7");
        assert_eq!(lines[1], "   -     0.0s  to 0x0000  0x0000->0x0000  --- data    cluster 0x1234               #0   1 attempts, ok");
        assert_eq!(lines[2], "   -     0.0s  to 0x0000  0x0000->0x0000  --- data                                 #0   1 attempts, no ack");
    }

    #[test]
    fn a_neighbour_with_more_frames_than_its_entry_holds() {
        let mut b = vec![0u8; HEADER + 5];
        b[1] = 1;
        b[2] = 5;
        let e = &mut b[HEADER..];
        e[0..2].copy_from_slice(&0x1001u16.to_le_bytes());
        e[2] = 2;
        assert_eq!(decode(&b).unwrap().links[0].frames.len(), 2);
        b[HEADER + 2] = 3;
        assert_eq!(decode(&b).unwrap_err().to_string(), "neighbour 0x1001: 3 frames do not fit its entry");
        b[HEADER + 2] = 0;
        assert!(decode(&b).unwrap().links.is_empty());
        b[HEADER + 2] = 1;
        b[HEADER..HEADER + 2].copy_from_slice(&0xFFFFu16.to_le_bytes());
        assert!(decode(&b).unwrap().links.is_empty());
    }

    #[test]
    fn link_nibbles_and_runs_of_losses() {
        let mut b = vec![0u8; HEADER + 36];
        b[1] = 1;
        b[2] = 36;
        let e = &mut b[HEADER..];
        e[0..2].copy_from_slice(&0x1001u16.to_le_bytes());
        e[2] = 3;
        e[4] = (2 | 8) | 3 << 4;
        e[5] = 1 | 8;
        let bx = decode(&b).unwrap();
        assert_eq!(bx.links, [Link { address: "0x1001".into(), frames: vec![(2, true), (3, false), (1, true)] }]);
        let r = report(&bx, &[]);
        assert!(r.contains("   0x1001:  3 frames,   6 attempts, delivered 33%, runs of losses {1: 1, 3: 1}\n      2x1\n"), "{r}");
    }

    #[test]
    fn short_blob_is_an_error() {
        assert_eq!(decode(&[0; HEADER - 1]).unwrap_err().to_string(), "black box of 23 bytes: shorter than its header");
        assert!(decode(&[0; HEADER]).is_ok());
        let mut b = vec![0u8; HEADER];
        b[1] = 16;
        b[2] = 36;
        assert_eq!(decode(&b).unwrap_err().to_string(), "black box of 24 bytes does not hold what its header describes (600 bytes)");
    }

    fn header(neighbors: u8, le: u8, ring: (u8, u8, u8), rows: (u16, u8)) -> Vec<u8> {
        let mut b = vec![0u8; HEADER];
        b[1] = neighbors;
        b[2] = le;
        (b[3], b[4], b[5]) = ring;
        b[8..10].copy_from_slice(&rows.0.to_le_bytes());
        b[10] = rows.1;
        b
    }

    fn fits(mut b: Vec<u8>, len: usize) -> Result<BlackBox> {
        b.resize(len, 0);
        decode(&b)
    }

    #[test]
    fn the_header_says_how_long_the_box_is() {
        let need = HEADER + 2 * 5 + 3 * 16 + 4 * 13;
        let h = header(2, 5, (3, 2, 16), (4, 13));
        assert!(fits(h.clone(), need).is_ok());
        assert_eq!(
            fits(h, need - 1).unwrap_err().to_string(),
            format!("black box of {} bytes does not hold what its header describes ({need} bytes)", need - 1)
        );
        for (h, n) in [(header(2, 5, (0, 0, 0), (0, 0)), 10), (header(0, 0, (3, 0, 16), (0, 0)), 48), (header(0, 0, (0, 0, 0), (4, 13)), 52)] {
            assert!(fits(h.clone(), HEADER + n).is_ok());
            assert!(fits(h, HEADER + n - 1).is_err());
        }
    }

    #[test]
    fn entries_too_small_for_what_they_hold() {
        let ok = |h: Vec<u8>| fits(h, 4096).is_ok();
        assert!(ok(header(1, 4, (0, 0, 0), (0, 0))));
        assert!(!ok(header(1, 3, (0, 0, 0), (0, 0))));
        assert!(ok(header(0, 3, (0, 0, 0), (0, 0))));
        assert!(ok(header(0, 0, (1, 1, 15), (0, 0))));
        assert!(!ok(header(0, 0, (1, 1, 14), (0, 0))));
        assert!(ok(header(0, 0, (1, 0, 14), (0, 0))));
        assert!(ok(header(0, 0, (0, 0, 0), (1, 12))));
        assert!(!ok(header(0, 0, (0, 0, 0), (1, 11))));
        assert!(ok(header(0, 0, (0, 0, 0), (0, 11))));
    }

    #[test]
    fn a_row_and_its_cells() {
        let mut b = header(0, 0, (0, 0, 0), (1, 12));
        b.extend_from_slice(&[7, 0, 0x34, 0x12, 9, 0, 3, 4, 3 | 8 | 16 | 5 << 5, 128, 6, 0xA5]);
        let r = decode(&b).unwrap().rows.remove(0);
        assert_eq!(
            r.cells(),
            ["7", "4660", "9", "3", "4", "3", "True", "True", "5", "50", "6", "-91"]
        );
        let mut b = header(0, 0, (0, 0, 0), (1, 12));
        b.extend_from_slice(&[0; 12]);
        assert_eq!(decode(&b).unwrap().rows[0].cells()[6..8], ["False", "False"]);
    }

    #[test]
    fn a_ring_shorter_than_its_count_and_a_link_without_attempts() {
        let mut b = header(1, 5, (1, 3, 15), (0, 0));
        b.extend_from_slice(&[0x02, 0x10, 1, 0, 0]);
        b.extend_from_slice(&[0; 15]);
        let bx = decode(&b).unwrap();
        assert_eq!(bx.ring.len(), 1);
        assert_eq!(bx.links[0].frames, [(0, false)]);
        assert!(report(&bx, &[]).contains("   0x1002:  1 frames,   0 attempts, delivered 0%, runs of losses -\n      x\n"));
        assert_eq!(unhex("éé").unwrap_err().to_string(), "not hex: odd length or not ASCII");
        assert_eq!(unhex("0g").unwrap_err().to_string(), "not hex: invalid digit found in string");
        assert_eq!(unhex(" 0a1B ").unwrap(), [0x0a, 0x1b]);
        assert_eq!(hex(&[0xab, 1]), "ab01");
    }

    #[test]
    fn the_length_from_page_0() {
        let mut p = vec![0u8; TOTAL_AT + 2];
        p[TOTAL_AT..].copy_from_slice(&1234u16.to_le_bytes());
        assert_eq!(total_len(&p), Some(1234));
        assert_eq!(total_len(&p[..TOTAL_AT + 1]), None);
    }
}
