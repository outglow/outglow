use crate::ble::harness::{att_error, hx, BleSession, LinkOpts, CID_SMP};
use crate::ble::util::{describe, error_problem, link, read_all, some, table, value};
use crate::ble::{BleTest, Level};
use crate::py;
use crate::session::{expect, Stop, R};

const DOC: &str = "Core 5.4 Vol 3 Part G";
const PRIMARY: [u8; 2] = [0x00, 0x28];
const SECONDARY: [u8; 2] = [0x01, 0x28];
const INCLUDE: [u8; 2] = [0x02, 0x28];
const CHAR: [u8; 2] = [0x03, 0x28];

pub fn tests() -> Vec<BleTest> {
    let t = |id, clause, req, run| BleTest { id, doc: DOC, clause, req, run, session: "gatt", ..BleTest::D };
    vec![
        t(
            "gatt_primary_services",
            "4.4.1, 3.1",
            "Discover All Primary Services returns every service declaration once, in handle order, with the service \
             UUID as its value and the last attribute of the service definition as End Group Handle; the groups do \
             not overlap and the table begins with a service declaration.",
            primary_services as fn(&mut BleSession) -> R<String>,
        ),
        t(
            "gatt_service_by_uuid",
            "4.4.2; Part F 3.4.3.4",
            "Discover Primary Service by Service UUID returns, for every service UUID of the server, the same handle \
             ranges as Discover All Primary Services, and ends with Attribute Not Found.",
            service_by_uuid,
        ),
        t(
            "gatt_char_declaration_value",
            "3.3.1 (Table 3.4)",
            "The value of a characteristic declaration, read with a Read Request, is the Characteristic Properties, \
             the Characteristic Value Handle and the Characteristic UUID: 5 or 19 octets, the same that \
             characteristic discovery returns.",
            char_declaration_value,
        ),
        t(
            "gatt_char_definition",
            "3.3, 3.3.1, 3.3.2",
            "Every characteristic declaration is found by characteristic discovery, lies inside a service, and is \
             followed immediately by its value attribute, whose handle and type are the ones the declaration names.",
            char_definition,
        ),
        t(
            "gatt_chars_of_service",
            "4.6.1",
            "Discover All Characteristics of a Service, run on the handle range of each service, returns exactly the \
             declarations inside that range and ends with Attribute Not Found.",
            chars_of_service,
        ),
        t(
            "gatt_descriptors",
            "4.7.1, 3.3.3, 3.3.1.1",
            "Discover All Characteristic Descriptors on the range after each characteristic value returns the \
             descriptors of that characteristic; none of the descriptors 0x2900-0x2903 occurs twice in a \
             characteristic, and Extended Properties / Broadcast have their descriptor.",
            descriptors,
        ),
        BleTest {
            mandatory: false,
            level: Level::Should,
            ..t(
                "gatt_uuid_grouping",
                "3.1, 3.3.1",
                "Service definitions with 16-bit UUIDs are listed together and those with 128-bit UUIDs together; \
                 likewise the characteristic definitions inside a service.",
                uuid_grouping,
            )
        },
        t(
            "gatt_included_services",
            "4.5.1, 3.2",
            "Find Included Services on each service range is answered: with the include declarations of the service, \
             or with Attribute Not Found naming the Starting Handle where there are none.",
            included_services,
        ),
        BleTest {
            mandatory: false,
            ambiguous: Some(
                "7.1: 'If the list of GATT based services and the service definitions cannot change for the lifetime \
                 of the device then this characteristic shall not exist, otherwise this characteristic shall exist.' \
                 Whether the definitions can change (for example with a firmware update, which replaces the table on \
                 these bulbs) is not visible over the air, so a server without the characteristic is neither passed \
                 nor failed, unless it does not pair: then no bonded client keeps an attribute cache across \
                 connections (Part G 2.5.2) and the test gives N/A. 'Only one instance of the GATT service shall be \
                 exposed' is read as: not more than one",
            ),
            ..t(
                "gatt_service_gatt",
                "7, 7.1",
                "The GATT service (0x1801) is exposed at most once and holds the characteristics of Part G 7; the \
                 Service Changed characteristic, which a server whose service definitions can change must have, has \
                 properties 0x20, a CCCD and a value that is neither readable nor writable.",
                service_gatt,
            )
        },
    ]
}

fn rev(b: &[u8]) -> Vec<u8> {
    b.iter().rev().copied().collect()
}

fn le16(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn req(op: u8, start: u32, end: u16, tail: &[u8]) -> Vec<u8> {
    let mut q = vec![op];
    q.extend_from_slice(&(start as u16).to_le_bytes());
    q.extend_from_slice(&end.to_le_bytes());
    q.extend_from_slice(tail);
    q
}

fn vget(vals: &[(u16, Option<Vec<u8>>)], h: u16) -> Option<&[u8]> {
    vals.iter().find(|(x, _)| *x == h).and_then(|(_, r)| r.as_deref())
}

fn hexes(it: impl IntoIterator<Item = u16>) -> String {
    py::list(it.into_iter().map(|h| py::hexq(h as u64)))
}

#[allow(clippy::type_complexity)]
fn primary(s: &mut BleSession) -> R<(Vec<(u16, u16, Vec<u8>)>, Vec<String>)> {
    let (mut out, mut bad, mut start): (Vec<(u16, u16, Vec<u8>)>, Vec<String>, u32) = (Vec::new(), Vec::new(), 1);
    for _ in 0..100 {
        let rsp = s.att(&req(0x10, start, 0xFFFF, &PRIMARY))?;
        if att_error(rsp.as_deref()).is_some() {
            if let Some(p) = error_problem(rsp.as_deref(), 0x10, &[0x0A], Some(start as u16)) {
                bad.push(format!("request from {start:#06x}: {p}"));
            }
            break;
        }
        let ok = rsp.as_ref().is_some_and(|r| {
            r[0] == 0x11 && r.len() >= 8 && (r[1] == 6 || r[1] == 20) && (r.len() - 2) % r[1] as usize == 0
        });
        if !ok {
            bad.push(format!(
                "request from {start:#06x}: {}; not Attribute Data of 6 or 20 octets",
                describe(rsp.as_deref())
            ));
            break;
        }
        let r = rsp.unwrap();
        let w = r[1] as usize;
        for i in (2..r.len()).step_by(w) {
            out.push((le16(&r, i), le16(&r, i + 2), r[i + 4..i + w].to_vec()));
        }
        let last = out.last().unwrap().1;
        if last == 0xFFFF || (last as u32) < start {
            break;
        }
        start = last as u32 + 1;
    }
    Ok((out, bad))
}

fn primary_services(s: &mut BleSession) -> R<String> {
    link(s, LinkOpts::new())?;
    let g = table(s)?;
    let vals = read_all(s)?;
    let (groups, mut bad) = primary(s)?;
    let hs: Vec<u16> = g.attrs.keys().copied().collect();
    let decls: Vec<u16> = hs.iter().copied().filter(|h| g.attrs[h] == PRIMARY || g.attrs[h] == SECONDARY).collect();
    let prim: Vec<u16> = hs.iter().copied().filter(|h| g.attrs[h] == PRIMARY).collect();
    let t0 = &g.attrs[&hs[0]];
    if *t0 != PRIMARY && *t0 != SECONDARY {
        bad.push(format!("the first attribute ({:#06x}) has type {}: it is in no service", hs[0], hx(&rev(t0))));
    }
    let starts: Vec<u16> = groups.iter().map(|x| x.0).collect();
    if starts != prim {
        bad.push(format!(
            "services returned at {}; attributes of type «Primary Service» are at {}",
            hexes(starts.iter().copied()),
            hexes(prim.iter().copied())
        ));
    }
    for (i, (a, b, u)) in groups.iter().enumerate() {
        let (a, b) = (*a, *b);
        let nxt = decls.iter().copied().find(|h| *h > a);
        let lim: u32 = nxt.map(|n| n as u32).unwrap_or(0x10000);
        let last = if g.attrs.contains_key(&a) {
            hs.iter().copied().filter(|h| a <= *h && (*h as u32) < lim).max()
        } else {
            None
        };
        let ok_end = Some(b) == last || (nxt.is_none() && b == 0xFFFF);
        if u.len() != 2 && u.len() != 16 {
            bad.push(format!("service at {a:#06x}: UUID of {} octets", u.len()));
        }
        if value(vget(&vals, a)).as_deref() != Some(&u[..]) {
            bad.push(format!(
                "service at {a:#06x}: UUID {} by Read By Group Type, {} by Read Request",
                hx(&rev(u)),
                describe(vget(&vals, a))
            ));
        }
        if !ok_end {
            bad.push(match last {
                Some(l) => format!(
                    "service at {a:#06x}: End Group Handle {b:#06x}; the last attribute before the next service \
                     declaration is {l:#06x}"
                ),
                None => format!("service at {a:#06x} is no attribute"),
            });
        }
        if i > 0 && a <= groups[i - 1].1 {
            bad.push(format!(
                "service at {a:#06x} begins inside the group {:#06x}-{:#06x}",
                groups[i - 1].0,
                groups[i - 1].1
            ));
        }
    }
    expect(bad.is_empty(), some(&bad, 5))?;
    expect(!groups.is_empty(), "no primary service")?;
    Ok(groups.iter().map(|(a, b, u)| format!("{} {a:#06x}-{b:#06x}", hx(&rev(u)))).collect::<Vec<_>>().join("; "))
}

fn service_by_uuid(s: &mut BleSession) -> R<String> {
    link(s, LinkOpts::new())?;
    let g = table(s)?;
    let (groups, mut bad) = primary(s)?;
    if !bad.is_empty() || groups.is_empty() {
        return Err(Stop::Inconclusive(format!("Discover All Primary Services did not complete: {}", some(&bad, 2))));
    }
    let last_start = groups.last().unwrap().0;
    let last_handle = g.last_handle();
    let norm = |r: &[(u16, u16)]| -> Vec<(u16, u16)> {
        r.iter().map(|(a, b)| (*a, if *b == 0xFFFF && *a == last_start { last_handle } else { *b })).collect()
    };
    let mut uuids: Vec<Vec<u8>> = Vec::new();
    for (_, _, u) in &groups {
        if !uuids.contains(u) {
            uuids.push(u.clone());
        }
    }
    let pairs = |r: &[(u16, u16)]| py::list(r.iter().map(|(a, b)| py::tuple([py::hexq(*a as u64), py::hexq(*b as u64)])));
    let mut notes = Vec::new();
    for u in uuids {
        let want: Vec<(u16, u16)> = groups.iter().filter(|x| x.2 == u).map(|x| (x.0, x.1)).collect();
        let (mut got, mut start): (Vec<(u16, u16)>, u32) = (Vec::new(), 1);
        let mut tail = PRIMARY.to_vec();
        tail.extend_from_slice(&u);
        for _ in 0..20 {
            let rsp = s.att(&req(0x06, start, 0xFFFF, &tail))?;
            if att_error(rsp.as_deref()).is_some() {
                if let Some(p) = error_problem(rsp.as_deref(), 0x06, &[0x0A], Some(start as u16)) {
                    bad.push(format!("service {}, request from {start:#06x}: {p}", hx(&rev(&u))));
                }
                break;
            }
            let ok = rsp.as_ref().is_some_and(|r| r[0] == 0x07 && r.len() >= 5 && (r.len() - 1) % 4 == 0);
            if !ok {
                bad.push(format!("service {}, request from {start:#06x}: {}", hx(&rev(&u)), describe(rsp.as_deref())));
                break;
            }
            let r = rsp.unwrap();
            for i in (1..r.len()).step_by(4) {
                got.push((le16(&r, i), le16(&r, i + 2)));
            }
            let e = got.last().unwrap().1;
            if e == 0xFFFF || (e as u32) < start {
                break;
            }
            start = e as u32 + 1;
        }
        if norm(&got) != norm(&want) {
            bad.push(format!(
                "service {}: ranges {}; Discover All Primary Services gave {}",
                hx(&rev(&u)),
                pairs(&got),
                pairs(&want)
            ));
        }
        notes.push(format!(
            "{} {}",
            hx(&rev(&u)),
            got.iter().map(|(a, b)| format!("{a:#06x}-{b:#06x}")).collect::<Vec<_>>().join(", ")
        ));
    }
    expect(bad.is_empty(), some(&bad, 3))?;
    Ok(notes.join("; "))
}

fn char_declaration_value(s: &mut BleSession) -> R<String> {
    link(s, LinkOpts::new())?;
    let g = table(s)?;
    let vals = read_all(s)?;
    let mut bad = Vec::new();
    for c in &g.chars {
        let mut want = vec![c.props];
        want.extend_from_slice(&c.value.to_le_bytes());
        want.extend_from_slice(&c.uuid);
        if value(vget(&vals, c.decl)).as_deref() != Some(&want[..]) {
            bad.push(format!(
                "declaration {:#06x}: Read Request gives {}; Read By Type gives {}",
                c.decl,
                describe(vget(&vals, c.decl)),
                hx(&want)
            ));
        }
    }
    expect(bad.is_empty(), format!("{} of {} declarations: {}", bad.len(), g.chars.len(), some(&bad, 3)))?;
    Ok(format!("{} declarations: Read Request and Read By Type give the same 5 or 19 octets", g.chars.len()))
}

fn char_definition(s: &mut BleSession) -> R<String> {
    link(s, LinkOpts::new())?;
    let g = table(s)?;
    let hs: Vec<u16> = g.attrs.keys().copied().collect();
    let mut bad = Vec::new();
    let decl: Vec<u16> = hs.iter().copied().filter(|h| g.attrs[h] == CHAR).collect();
    let found: Vec<u16> = g.chars.iter().map(|c| c.decl).collect();
    if decl != found {
        bad.push(format!(
            "attributes of type «Characteristic» at {}; discovery by Read By Type found {}",
            hexes(decl.iter().copied()),
            hexes(found.iter().copied())
        ));
    }
    for c in &g.chars {
        let nxt = hs.iter().copied().find(|h| *h > c.decl);
        let sv = g.services.iter().find(|x| x.start < c.decl && c.decl <= x.end);
        if c.uuid.len() != 2 && c.uuid.len() != 16 {
            bad.push(format!("declaration {:#06x}: UUID of {} octets", c.decl, c.uuid.len()));
        }
        if Some(c.value) != nxt {
            bad.push(match nxt {
                Some(n) => format!("declaration {:#06x} names value handle {:#06x}; the next attribute is {n:#06x}", c.decl, c.value),
                None => format!("declaration {:#06x} is the last attribute", c.decl),
            });
        } else if g.attrs[&c.value] != c.uuid {
            bad.push(format!(
                "declaration {:#06x} names UUID {}; the attribute {:#06x} has type {}",
                c.decl,
                hx(&rev(&c.uuid)),
                c.value,
                hx(&rev(&g.attrs[&c.value]))
            ));
        }
        if sv.map_or(true, |sv| c.value > sv.end) {
            bad.push(format!("characteristic {:#06x}-{:#06x} is not inside one service definition", c.decl, c.value));
        }
    }
    expect(bad.is_empty(), some(&bad, 5))?;
    expect(!g.chars.is_empty(), "no characteristic")?;
    Ok(format!(
        "{} characteristics, each declaration followed by its value attribute of the declared type",
        g.chars.len()
    ))
}

fn chars_of_service(s: &mut BleSession) -> R<String> {
    link(s, LinkOpts::new())?;
    let g = table(s)?;
    let (mut bad, mut notes) = (Vec::new(), Vec::new());
    for sv in &g.services {
        let want: Vec<(u16, Vec<u8>)> = g
            .chars
            .iter()
            .filter(|c| sv.start <= c.decl && c.decl <= sv.end)
            .map(|c| {
                let mut d = vec![c.props];
                d.extend_from_slice(&c.value.to_le_bytes());
                d.extend_from_slice(&c.uuid);
                (c.decl, d)
            })
            .collect();
        let (mut got, mut start): (Vec<(u16, Vec<u8>)>, u32) = (Vec::new(), sv.start as u32);
        for _ in 0..60 {
            let rsp = s.att(&req(0x08, start, sv.end, &CHAR))?;
            if att_error(rsp.as_deref()).is_some() {
                if let Some(p) = error_problem(rsp.as_deref(), 0x08, &[0x0A], Some(start as u16)) {
                    bad.push(format!("service {:#06x}-{:#06x}, request from {start:#06x}: {p}", sv.start, sv.end));
                }
                break;
            }
            let ok = rsp.as_ref().is_some_and(|r| {
                r[0] == 0x09 && r.len() >= 9 && (r[1] == 7 || r[1] == 21) && (r.len() - 2) % r[1] as usize == 0
            });
            if !ok {
                bad.push(format!(
                    "service {:#06x}-{:#06x}, request from {start:#06x}: {}",
                    sv.start,
                    sv.end,
                    describe(rsp.as_deref())
                ));
                break;
            }
            let r = rsp.unwrap();
            let w = r[1] as usize;
            for i in (2..r.len()).step_by(w) {
                got.push((le16(&r, i), r[i + 2..i + w].to_vec()));
            }
            let l = got.last().unwrap().0;
            if l >= sv.end || (l as u32) < start {
                break;
            }
            start = l as u32 + 1;
        }
        if got != want {
            bad.push(format!(
                "service {:#06x}-{:#06x}: declarations at {}; the range holds {}",
                sv.start,
                sv.end,
                hexes(got.iter().map(|x| x.0)),
                hexes(want.iter().map(|x| x.0))
            ));
        }
        let u = hx(&rev(&sv.uuid));
        notes.push(format!("{}: {}", &u[..u.len().min(8)], got.len()));
    }
    expect(bad.is_empty(), some(&bad, 3))?;
    Ok(format!("characteristics per service: {}", notes.join(", ")))
}

fn descriptors(s: &mut BleSession) -> R<String> {
    link(s, LinkOpts::new())?;
    let g = table(s)?;
    let (mut bad, mut n) = (Vec::new(), 0usize);
    for c in &g.chars {
        let types: Vec<&Vec<u8>> = c.descs.iter().map(|(_, u)| u).collect();
        for t in [0x2900u16, 0x2901, 0x2902, 0x2903] {
            let k = types.iter().filter(|u| u[..] == t.to_le_bytes()).count();
            if k > 1 {
                bad.push(format!("characteristic {:#06x}: {k} descriptors {t:#06x}", c.decl));
            }
        }
        let has = |t: u16| types.iter().any(|u| u[..] == t.to_le_bytes());
        if c.props & 0x80 != 0 && !has(0x2900) {
            bad.push(format!("characteristic {:#06x}: Extended Properties bit without descriptor 0x2900", c.decl));
        }
        if c.props & 0x01 != 0 && !has(0x2903) {
            bad.push(format!("characteristic {:#06x}: Broadcast bit without descriptor 0x2903", c.decl));
        }
        if c.end <= c.value {
            continue;
        }
        let (mut got, mut start): (Vec<(u16, Vec<u8>)>, u32) = (Vec::new(), c.value as u32 + 1);
        for _ in 0..40 {
            let rsp = s.att(&req(0x04, start, c.end, &[]))?;
            if att_error(rsp.as_deref()).is_some() {
                if let Some(p) = error_problem(rsp.as_deref(), 0x04, &[0x0A], Some(start as u16)) {
                    bad.push(format!("characteristic {:#06x}, request {start:#06x}-{:#06x}: {p}", c.decl, c.end));
                }
                break;
            }
            let w = if rsp.as_ref().is_some_and(|r| r.get(1) == Some(&1)) { 4 } else { 18 };
            let ok = rsp.as_ref().is_some_and(|r| {
                r[0] == 0x05 && r.len() >= 6 && (r[1] == 1 || r[1] == 2) && (r.len() - 2) % w == 0
            });
            if !ok {
                bad.push(format!(
                    "characteristic {:#06x}, request {start:#06x}-{:#06x}: {}",
                    c.decl,
                    c.end,
                    describe(rsp.as_deref())
                ));
                break;
            }
            let r = rsp.unwrap();
            for i in (2..r.len()).step_by(w) {
                got.push((le16(&r, i), r[i + 2..i + w].to_vec()));
            }
            let l = got.last().unwrap().0;
            if l >= c.end || (l as u32) < start {
                break;
            }
            start = l as u32 + 1;
        }
        if got != c.descs {
            let show = |v: &[(u16, Vec<u8>)]| {
                py::list(v.iter().map(|(h, u)| py::tuple([py::hexq(*h as u64), py::str_repr(&hx(&rev(u)))])))
            };
            bad.push(format!(
                "characteristic {:#06x}: descriptors {} in {:#06x}-{:#06x}; the table has {}",
                c.decl,
                show(&got),
                c.value as u32 + 1,
                c.end,
                show(&c.descs)
            ));
        }
        n += got.len();
    }
    expect(bad.is_empty(), some(&bad, 3))?;
    let mut kinds: Vec<String> = g.chars.iter().flat_map(|c| c.descs.iter().map(|(_, u)| hx(&rev(u)))).collect();
    kinds.sort();
    kinds.dedup();
    let with = g.chars.iter().filter(|c| !c.descs.is_empty()).count();
    Ok(format!(
        "{n} descriptors in {with} of {} characteristics; types {}",
        g.chars.len(),
        if kinds.is_empty() { "none".to_string() } else { kinds.join(", ") }
    ))
}

fn uuid_grouping(s: &mut BleSession) -> R<String> {
    link(s, LinkOpts::new())?;
    let g = table(s)?;
    let changes = |sizes: &[usize]| sizes.windows(2).filter(|w| w[0] != w[1]).count();
    let ints = |v: &[usize]| py::ints(v.iter().map(|x| *x as i64));
    let mut bad = Vec::new();
    let sv: Vec<usize> = g.services.iter().map(|x| x.uuid.len()).collect();
    if changes(&sv) > 1 {
        bad.push(format!("service UUID sizes in handle order: {}", ints(&sv)));
    }
    for x in &g.services {
        let cs: Vec<usize> = g.chars.iter().filter(|c| x.start < c.decl && c.decl <= x.end).map(|c| c.uuid.len()).collect();
        if changes(&cs) > 1 {
            bad.push(format!(
                "service {:#06x}-{:#06x}: characteristic UUID sizes in handle order: {}",
                x.start,
                x.end,
                ints(&cs)
            ));
        }
    }
    expect(bad.is_empty(), some(&bad, 5))?;
    Ok(format!("service UUID sizes {}; inside each service the characteristic UUID sizes change at most once", ints(&sv)))
}

fn included_services(s: &mut BleSession) -> R<String> {
    link(s, LinkOpts::new())?;
    let g = table(s)?;
    let (mut bad, mut n) = (Vec::new(), 0usize);
    for sv in &g.services {
        let inc: Vec<u16> =
            g.attrs.iter().filter(|(h, t)| sv.start <= **h && **h <= sv.end && t[..] == INCLUDE).map(|(h, _)| *h).collect();
        let rsp = s.att(&req(0x08, sv.start as u32, sv.end, &INCLUDE))?;
        if inc.is_empty() {
            if let Some(p) = error_problem(rsp.as_deref(), 0x08, &[0x0A], Some(sv.start)) {
                bad.push(format!("service {:#06x}-{:#06x}, no include declaration: {p}", sv.start, sv.end));
            }
            continue;
        }
        n += inc.len();
        let ok = rsp.as_ref().is_some_and(|r| {
            r[0] == 0x09 && r.len() >= 8 && (r[1] == 6 || r[1] == 8) && r[2..4] == inc[0].to_le_bytes()
        });
        if !ok {
            bad.push(format!(
                "service {:#06x}-{:#06x}, include declaration at {:#06x}: {}",
                sv.start,
                sv.end,
                inc[0],
                describe(rsp.as_deref())
            ));
        }
    }
    expect(bad.is_empty(), some(&bad, 3))?;
    Ok(if n > 0 {
        format!("{n} include declarations returned")
    } else {
        format!("no include declarations: Attribute Not Found for each of {} service ranges", g.services.len())
    })
}

fn service_gatt(s: &mut BleSession) -> R<String> {
    link(s, LinkOpts::new())?;
    let g = table(s)?;
    let vals = read_all(s)?;
    let gs: Vec<_> = g.services.iter().filter(|x| x.uuid[..] == 0x1801u16.to_le_bytes()).collect();
    expect(gs.len() <= 1, format!("{} GATT services, at {}", gs.len(), hexes(gs.iter().map(|x| x.start))))?;
    let sec7: Vec<_> = g
        .chars
        .iter()
        .filter(|c| [0x2A05u16, 0x2B29, 0x2B2A, 0x2B3A].iter().any(|u| c.uuid[..] == u.to_le_bytes()))
        .collect();
    let out: Vec<_> = sec7
        .iter()
        .filter(|c| gs.is_empty() || !(gs[0].start < c.decl && c.decl <= gs[0].end) || !gs[0].primary)
        .collect();
    expect(
        out.is_empty(),
        format!(
            "characteristics of Part G 7 outside a primary GATT service: {}",
            out.iter().map(|c| format!("{} at {:#06x}", hx(&rev(&c.uuid)), c.decl)).collect::<Vec<_>>().join(", ")
        ),
    )?;
    let sc: Vec<_> = g.chars.iter().filter(|c| c.uuid[..] == 0x2A05u16.to_le_bytes()).collect();
    if sc.is_empty() {
        let what = if gs.is_empty() { "no GATT service and no" } else { "a GATT service without" };
        s.poll()?;
        let n0 = s.lk().frames.len();
        s.l2cap(CID_SMP, &[0x01, 0x03, 0x00, 0x01, 0x10, 0x00, 0x01])?;
        let rsp = s.frame_wait(CID_SMP, |_| true, 4000.0, Some(n0))?.map(|f| f.data);
        if rsp.as_deref() == Some(&[0x05, 0x05][..]) {
            return Err(Stop::NotApplicable(format!(
                "{what} Service Changed characteristic, and the server does not pair (Pairing Request answered \
                 Pairing Not Supported): no bonded client keeps an attribute cache across connections (Part G \
                 2.5.2), so a change of the definitions with a firmware update leaves nothing stale"
            )));
        }
        return Err(Stop::Inconclusive(format!(
            "{what} Service Changed characteristic: right only if the service definitions cannot change for the \
             lifetime of the device, which the test cannot see (Pairing Request answered {})",
            rsp.as_deref().map(hx).unwrap_or_else(|| "with nothing".into())
        )));
    }
    expect(sc.len() == 1, format!("{} Service Changed characteristics", sc.len()))?;
    let c = sc[0];
    let mut bad = Vec::new();
    if c.props != 0x20 {
        bad.push(format!("properties {:#04x}; required 0x20", c.props));
    }
    if c.descs.iter().filter(|(_, u)| u[..] == [0x02, 0x29]).count() != 1 {
        bad.push("no Client Characteristic Configuration descriptor".to_string());
    }
    if value(vget(&vals, c.value)).is_some() {
        bad.push(format!("the value is readable ({}); required not readable", describe(vget(&vals, c.value))));
    }
    expect(bad.is_empty(), format!("Service Changed: {}", bad.join("; ")))?;
    Ok(format!(
        "GATT service at {:#06x} with Service Changed (indicate only, CCCD, value not readable)",
        gs[0].start
    ))
}
