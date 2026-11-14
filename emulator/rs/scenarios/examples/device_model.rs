//! What a coordinator learns of a bulb when it interviews it, as JSON: the
//! node descriptor's manufacturer code, the Basic attributes, the active
//! endpoints with their simple descriptors, and every attribute of every
//! server cluster (Discover Attributes, then Read Attributes). The input of
//! tools/z2m-generated-definition.mjs, which hands it to Zigbee2MQTT's
//! definition generator.
//!
//!   cargo run --release -p scenarios --example device_model -- rust=../../target/tc32/bulb-tc32.bin [cpi] > model.json

use scenarios::frames::ZclValue;
use scenarios::image::Image;
use scenarios::session::{Session, SessionOpts};
use scenarios::station::Tx;

fn json_str(b: &[u8]) -> String {
    let mut s = String::from("\"");
    for &c in b {
        match c {
            b'"' => s.push_str("\\\""),
            b'\\' => s.push_str("\\\\"),
            0x20..=0x7e => s.push(c as char),
            c => s.push_str(&format!("\\u{c:04x}")),
        }
    }
    s.push('"');
    s
}

/// A value as JSON: numbers as numbers (single precision decoded), strings as strings.
fn json_value(ty: u8, v: &ZclValue) -> String {
    match v {
        ZclValue::Int(i) if ty == 0x39 => {
            let f = f32::from_bits(*i as u32);
            if f.is_finite() {
                format!("{f}")
            } else {
                "null".into()
            }
        }
        ZclValue::Int(i) => i.to_string(),
        ZclValue::Bytes(b) if ty == 0x42 => json_str(b),
        ZclValue::Bytes(b) => format!("[{}]", b.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(",")),
        ZclValue::None => "null".into(),
    }
}

/// Every attribute of a server cluster of an endpoint: (id, type, value as JSON).
fn attributes(s: &mut Session, ep: u8, cluster: u16) -> Vec<(u16, u8, String)> {
    let mut ids = Vec::new();
    let mut start = 0u16;
    for _ in 0..10 {
        let st = start.to_le_bytes();
        let Ok(Some(f)) = s.zcl(cluster, 0x0C, &[st[0], st[1], 16], Tx::new().general().ep(ep)) else { break };
        let z = f.zcl().unwrap();
        if z.cmd != 0x0D || z.payload.is_empty() {
            break;
        }
        let recs: Vec<(u16, u8)> = z.payload[1..].chunks(3).filter(|c| c.len() == 3).map(|c| (u16::from_le_bytes([c[0], c[1]]), c[2])).collect();
        let last = recs.last().map(|r| r.0);
        ids.extend(recs);
        match (z.payload[0], last) {
            (0, Some(l)) if l < 0xFFFF => start = l + 1,
            _ => break,
        }
    }
    let mut out = Vec::new();
    for chunk in ids.chunks(4) {
        let want: Vec<u16> = chunk.iter().map(|a| a.0).collect();
        if let Ok(Some(r)) = s.read_with(cluster, &want, Tx::new().ep(ep)) {
            for (id, st, ty, v) in r {
                if let (0, Some(ty), Some(v)) = (st, ty, v) {
                    out.push((id, ty, json_value(ty, &v)));
                }
            }
        }
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let img = Image::parse(&args[0]).unwrap();
    let cpi: i64 = args.get(1).map(|s| s.parse().unwrap()).unwrap_or(24);
    let mut s = Session::new(&img, SessionOpts { cpi, ..Default::default() }).expect("session");
    s.run(3000.0).unwrap();
    let dst = s.dut_short.to_le_bytes();
    // node descriptor: logical type, manufacturer code
    let nd = s.zdo(0x0002, &dst, 2000.0).unwrap().map(|f| f.zdo().unwrap().payload).unwrap_or_default();
    let (kind, mfr) = match nd.get(3..8) {
        Some(d) => (d[0] & 7, u16::from_le_bytes([d[3], d[4]])),
        None => (0xFF, 0),
    };
    let eps = s.endpoints().unwrap();
    let mut ep_json = Vec::new();
    let mut basic = Vec::new();
    for ep in eps {
        let Ok(Some(d)) = s.simple_desc(ep) else { continue };
        let mut clusters = Vec::new();
        // the Green Power endpoint has no server clusters to read
        for c in d.inc.iter().copied() {
            let a = attributes(&mut s, ep, c);
            if ep == 1 && c == 0x0000 {
                basic = a.clone();
            }
            let body: Vec<String> = a.iter().map(|(id, ty, v)| format!("\"{id}\": {{\"type\": {ty}, \"value\": {v}}}")).collect();
            clusters.push(format!("\"{c}\": {{{}}}", body.join(", ")));
        }
        let list = |v: &[u16]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(", ");
        ep_json.push(format!(
            "    {{\"ID\": {}, \"profileID\": {}, \"deviceID\": {}, \"inputClusters\": [{}], \"outputClusters\": [{}],\n     \"attributes\": {{{}}}}}",
            d.ep, d.profile, d.device, list(&d.inc), list(&d.outc), clusters.join(", ")
        ));
    }
    let b = |id: u16| basic.iter().find(|a| a.0 == id).map(|a| a.2.clone()).unwrap_or_else(|| "null".into());
    println!("{{");
    println!("  \"image\": {},", json_str(img.label.as_bytes()));
    println!("  \"ieeeAddr\": \"0x{:016x}\",", s.dut_ext);
    println!("  \"networkAddress\": {},", s.dut_short);
    println!("  \"type\": \"{}\",", match kind { 0 => "Coordinator", 1 => "Router", 2 => "EndDevice", _ => "Unknown" });
    println!("  \"manufacturerID\": {mfr},");
    println!("  \"manufacturerName\": {},", b(0x0004));
    println!("  \"modelID\": {},", b(0x0005));
    println!("  \"softwareBuildID\": {},", b(0x4000));
    println!("  \"dateCode\": {},", b(0x0006));
    println!("  \"zclVersion\": {},", b(0x0000));
    println!("  \"applicationVersion\": {},", b(0x0001));
    println!("  \"stackVersion\": {},", b(0x0002));
    println!("  \"hardwareVersion\": {},", b(0x0003));
    println!("  \"powerSource\": {},", b(0x0007));
    println!("  \"endpoints\": [\n{}\n  ]", ep_json.join(",\n"));
    println!("}}");
}
