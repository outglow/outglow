use crate::ble::harness::{hx, parse_ad, BleSession};
use crate::ble::{BleTest, Level};
use crate::frames::py_bytes;
use crate::py;
use crate::session::{expect, Stop, R};

const GAP: &str = "Core 5.4 Vol 3 Part C";
const CSS: &str = "CSS 11 Part A";
const FLAGS: u8 = 0x01;
const UUID16_INC: u8 = 0x02;
const UUID16_ALL: u8 = 0x03;
const UUID32_INC: u8 = 0x04;
const UUID32_ALL: u8 = 0x05;
const UUID128_INC: u8 = 0x06;
const UUID128_ALL: u8 = 0x07;
const NAME_SHORT: u8 = 0x08;
const NAME_FULL: u8 = 0x09;
const MFR: u8 = 0xFF;

pub fn tests() -> Vec<BleTest> {
    vec![
        BleTest {
            id: "gap_adv_data_format",
            doc: GAP,
            clause: "11",
            req: "Advertising data and scan response data are sequences of AD structures: a non-zero length octet, \
                  the AD type, length - 1 octets of data; nothing but zero octets may follow, and at most 31 octets \
                  in all.",
            run: adv_data_format,
            ..BleTest::D
        },
        BleTest {
            id: "gap_adv_flags",
            doc: GAP,
            clause: "9.2.3.2, 9.2.4.2; CSS 11 Part A 1.3",
            req: "A discoverable LE-only device advertises the Flags AD type with exactly one of the Limited and \
                  General Discoverable Mode bits, BR/EDR Not Supported set and the Simultaneous LE and BR/EDR bit \
                  clear; trailing zero octets of the flags are not sent.",
            run: adv_flags,
            ..BleTest::D
        },
        BleTest {
            id: "gap_limited_discoverable_timeout",
            doc: GAP,
            clause: "9.2.3.2, Appendix A",
            req: "A device does not stay in the limited discoverable mode (Limited Discoverable Mode flag set) longer \
                  than T_GAP(lim_adv_timeout) = 180 s.",
            run: limited_timeout,
            ..BleTest::D
        },
        BleTest {
            id: "gap_adv_service_uuids",
            doc: CSS,
            clause: "1.1",
            req: "At most one Service UUID structure per UUID size in a packet.",
            run: service_uuid_once,
            ..BleTest::D
        },
        BleTest {
            id: "gap_adv_no_gap_gatt_uuid",
            doc: CSS,
            clause: "1.1",
            req: "The GAP (0x1800) and GATT (0x1801) service UUIDs are not put into a Service UUID list.",
            run: no_gap_gatt_uuid,
            mandatory: false,
            level: Level::Should,
            ..BleTest::D
        },
        BleTest {
            id: "gap_adv_manufacturer_data",
            doc: CSS,
            clause: "1.4",
            req: "Manufacturer Specific Data starts with a two-octet company identifier.",
            run: manufacturer_data,
            ..BleTest::D
        },
        BleTest {
            id: "gap_adv_local_name_once",
            doc: CSS,
            clause: "1.2",
            req: "The Local Name appears with one type that says whether it is complete or shortened; the two \
                  packets of the device do not contradict each other.",
            run: local_name,
            ..BleTest::D
        },
    ]
}

fn data(s: &mut BleSession) -> R<(Vec<u8>, Option<Vec<u8>>)> {
    let pk = s.wait_adv(2, 5000.0, Some(0.0), 0)?;
    let Some(last) = pk.last() else { return Err(Stop::Inconclusive("no ADV_IND seen".into())) };
    let adv = last.data().to_vec();
    s.run(30.0)?;
    let t0 = s.now();
    let q = s.scan_req_default()?;
    s.adv_reply(&q, 2, -1)?;
    s.until(3000.0, 20.0, |s| Ok(if s.adv(t0, false).iter().any(|p| p.ty() == 4) { Some(()) } else { None }))?;
    s.adv_reply_clear()?;
    let rsp = s.adv(t0, false).iter().find(|p| p.ty() == 4).map(|p| p.data().to_vec());
    Ok((adv, rsp))
}

fn adv_data_format(s: &mut BleSession) -> R<String> {
    let (adv, rsp) = data(s)?;
    let mut notes = Vec::new();
    for (name, d) in [("AdvData", Some(adv)), ("ScanRspData", rsp)] {
        let Some(d) = d else {
            notes.push(format!("{name}: none"));
            continue;
        };
        let (ads, bad) = parse_ad(&d);
        expect(d.len() <= 31, format!("{name} is {} octets", d.len()))?;
        expect(bad.is_empty(), format!("{name} {}: {}", hx(&d), bad.join("; ")))?;
        let types = py::list(ads.iter().map(|(t, _)| format!("'{t:#04x}'")));
        notes.push(format!("{name}: {} octets, AD types {types}", d.len()));
    }
    Ok(notes.join("; "))
}

fn adv_flags(s: &mut BleSession) -> R<String> {
    let (adv, _) = data(s)?;
    let (ads, _) = parse_ad(&adv);
    let fl: Vec<&Vec<u8>> = ads.iter().filter(|(t, _)| *t == FLAGS).map(|(_, v)| v).collect();
    expect(fl.len() == 1, format!("{} Flags structures in connectable advertising data {}", fl.len(), hx(&adv)))?;
    let v = fl[0];
    expect(!v.is_empty(), "Flags without a value")?;
    expect(v.len() == 1 || *v.last().unwrap() != 0, format!("Flags value {} ends with a zero octet", hx(v)))?;
    let (lim, gen, nobredr, simul) = (v[0] & 1, v[0] >> 1 & 1, v[0] >> 2 & 1, v[0] >> 3 & 1);
    expect(
        !(lim != 0 && gen != 0),
        format!("Flags {:#04x}: both the Limited and the General Discoverable Mode bit are set", v[0]),
    )?;
    expect(
        nobredr == 1 && simul == 0,
        format!(
            "Flags {:#04x}: an LE-only device sets BR/EDR Not Supported and clears the Simultaneous LE and BR/EDR bit",
            v[0]
        ),
    )?;
    expect(v[0] & 0xE0 == 0, format!("Flags {:#04x}: reserved bits set", v[0]))?;
    let mode = if lim != 0 {
        "limited discoverable"
    } else if gen != 0 {
        "general discoverable"
    } else {
        "non-discoverable"
    };
    Ok(format!("Flags {:#04x}: {mode} mode, BR/EDR Not Supported", v[0]))
}

fn flags_of(data: &[u8]) -> Option<u8> {
    parse_ad(data).0.iter().find(|(t, v)| *t == FLAGS && !v.is_empty()).map(|(_, v)| v[0])
}

fn limited_timeout(s: &mut BleSession) -> R<String> {
    let first = s.wait_adv(1, 5000.0, Some(0.0), 0)?;
    let Some(f) = first.first() else { return Err(Stop::Inconclusive("no ADV_IND seen".into())) };
    let f0 = flags_of(f.data());
    if f0.map_or(true, |x| x & 1 == 0) {
        return Ok(format!("not in the limited discoverable mode (Flags {})", f0.map(|x| x.to_string()).unwrap_or("None".into())));
    }
    let t0 = f.ms;
    s.until(190000.0, 1000.0, |s| Ok(if s.now() > t0 + 185000.0 { Some(()) } else { None }))?;
    let late: Vec<_> = s.adv(t0 + 180000.0 + 1000.0, false).into_iter().filter(|p| p.ty() == 0).collect();
    let still: Vec<_> = late.iter().filter(|p| flags_of(p.data()).unwrap_or(0) & 1 != 0).collect();
    let lastf = still.last().map(|p| flags_of(p.data()).unwrap_or(0)).unwrap_or(0);
    expect(
        still.is_empty(),
        format!(
            "{} ADV_IND with the Limited Discoverable Mode flag (Flags {lastf:#04x}) between 181 and 185 s after the first one",
            still.len()
        ),
    )?;
    let tail = match late.last() {
        Some(p) => format!("Flags {:#04x} afterwards", flags_of(p.data()).unwrap_or(0)),
        None => "advertising stopped".into(),
    };
    Ok(format!("limited discoverable mode left within 180 s: {tail}"))
}

fn service_uuid_once(s: &mut BleSession) -> R<String> {
    let (adv, rsp) = data(s)?;
    let mut notes = Vec::new();
    for (name, d) in [("AdvData", Some(adv)), ("ScanRspData", rsp)] {
        let Some(d) = d else { continue };
        let (ads, _) = parse_ad(&d);
        for (size, types) in [(16usize, (UUID16_INC, UUID16_ALL)), (32, (UUID32_INC, UUID32_ALL)), (128, (UUID128_INC, UUID128_ALL))] {
            let got: Vec<&(u8, Vec<u8>)> = ads.iter().filter(|(t, _)| *t == types.0 || *t == types.1).collect();
            expect(got.len() <= 1, format!("{name}: {} structures with {size}-bit service UUIDs", got.len()))?;
            for (t, v) in got {
                expect(v.len() % (size / 8) == 0, format!("{name}: {size}-bit UUID list of {} octets", v.len()))?;
                let k = if *t == types.1 { "complete" } else { "incomplete" };
                notes.push(format!("{name}: {k} list of {size}-bit UUIDs {}", hx(v)));
            }
        }
    }
    if notes.is_empty() {
        return Ok("no Service UUID structures".into());
    }
    Ok(notes.join("; "))
}

fn no_gap_gatt_uuid(s: &mut BleSession) -> R<String> {
    let (adv, rsp) = data(s)?;
    let mut hit = Vec::new();
    for (name, d) in [("AdvData", Some(adv)), ("ScanRspData", rsp)] {
        for (t, v) in parse_ad(&d.unwrap_or_default()).0 {
            if t == UUID16_INC || t == UUID16_ALL {
                let mut i = 0;
                while i + 1 < v.len() {
                    let x = u16::from_le_bytes([v[i], v[i + 1]]);
                    if x == 0x1800 || x == 0x1801 {
                        let k = if t == UUID16_ALL { "complete" } else { "incomplete" };
                        hit.push(format!("{name}: {x:#06x} in the {k} list"));
                    }
                    i += 2;
                }
            }
        }
    }
    expect(hit.is_empty(), hit.join("; "))?;
    Ok("neither UUID advertised".into())
}

fn manufacturer_data(s: &mut BleSession) -> R<String> {
    let (adv, rsp) = data(s)?;
    let mut notes = Vec::new();
    for (name, d) in [("AdvData", Some(adv)), ("ScanRspData", rsp)] {
        for (t, v) in parse_ad(&d.unwrap_or_default()).0 {
            if t == MFR {
                expect(v.len() >= 2, format!("{name}: Manufacturer Specific Data of {} octet(s)", v.len()))?;
                notes.push(format!(
                    "{name}: company {:#06x}, {} octets of data",
                    u16::from_le_bytes([v[0], v[1]]),
                    v.len() - 2
                ));
            }
        }
    }
    if notes.is_empty() {
        return Ok("no Manufacturer Specific Data".into());
    }
    Ok(notes.join("; "))
}

fn local_name(s: &mut BleSession) -> R<String> {
    let (adv, rsp) = data(s)?;
    let mut names: Vec<(&str, u8, Vec<u8>)> = Vec::new();
    for (name, d) in [("AdvData", Some(adv)), ("ScanRspData", rsp)] {
        let got: Vec<(u8, Vec<u8>)> =
            parse_ad(&d.unwrap_or_default()).0.into_iter().filter(|(t, _)| *t == NAME_SHORT || *t == NAME_FULL).collect();
        expect(got.len() <= 1, format!("{name}: {} Local Name structures", got.len()))?;
        names.extend(got.into_iter().map(|(t, v)| (name, t, v)));
    }
    if names.is_empty() {
        return Ok("no Local Name advertised".into());
    }
    let full: Vec<&Vec<u8>> = names.iter().filter(|(_, t, _)| *t == NAME_FULL).map(|(_, _, v)| v).collect();
    let mut uniq = full.clone();
    uniq.sort();
    uniq.dedup();
    expect(uniq.len() <= 1, format!("two different complete names: {}", py::list(full.iter().map(|v| py_bytes(v)))))?;
    for (w, t, v) in &names {
        if *t == NAME_SHORT && !full.is_empty() {
            expect(
                full[0].starts_with(v),
                format!("{w}: shortened name {} is not the beginning of {}", py_bytes(v), py_bytes(full[0])),
            )?;
        }
    }
    Ok(names
        .iter()
        .map(|(w, t, v)| format!("{w}: {} {}", if *t == NAME_FULL { "complete" } else { "shortened" }, py_bytes(v)))
        .collect::<Vec<_>>()
        .join("; "))
}
