use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;

#[derive(clap::Args)]
pub struct Args {
    #[doc = " fail if the generated files are not up to date instead of writing them"]
    #[arg(long)]
    check: bool,
    #[doc = " the profiles (default analysis/profiles)"]
    #[arg(long)]
    profiles: Option<PathBuf>,
}

const PWM_MUX: [(&str, u32, u32, u32, &str); 25] = [
    ("PA0", 0x5a8, 0, 1, "PWM0_N"), ("PA2", 0x5a8, 4, 2, "PWM0"), ("PA3", 0x5a8, 6, 2, "PWM1"),
    ("PA4", 0x5a9, 0, 2, "PWM2"), ("PB0", 0x5aa, 0, 0, "PWM3"), ("PB1", 0x5aa, 2, 0, "PWM4"),
    ("PB2", 0x5aa, 4, 0, "PWM5"), ("PB3", 0x5aa, 6, 0, "PWM0_N"), ("PB4", 0x5ab, 0, 1, "PWM4"),
    ("PB5", 0x5ab, 2, 1, "PWM5"), ("PC0", 0x5ac, 0, 1, "PWM4_N"), ("PC1", 0x5ac, 2, 1, "PWM1_N"),
    ("PC1", 0x5ac, 2, 2, "PWM0"), ("PC2", 0x5ac, 4, 0, "PWM0"), ("PC3", 0x5ac, 6, 0, "PWM1"),
    ("PC4", 0x5ad, 0, 0, "PWM2"), ("PC4", 0x5ad, 0, 2, "PWM0_N"), ("PC5", 0x5ad, 2, 0, "PWM3_N"),
    ("PC6", 0x5ad, 4, 2, "PWM4_N"), ("PC7", 0x5ad, 6, 2, "PWM5_N"), ("PD2", 0x5ae, 4, 2, "PWM3"),
    ("PD3", 0x5ae, 6, 0, "PWM1_N"), ("PD4", 0x5af, 0, 2, "PWM2_N"), ("PD5", 0x5af, 2, 0, "PWM0"),
    ("PD5", 0x5af, 2, 2, "PWM0_N"),
];

fn gpio_func(port: char) -> Option<u32> {
    Some(match port {
        'A' => 0x586,
        'B' => 0x58e,
        'C' => 0x596,
        'D' => 0x59e,
        'E' => 0x5a6,
        _ => return None,
    })
}

const IMAGE_TYPES: [(&str, u16); 21] = [
    ("ESMLFzm_w6_Dimm-4.63", 0x0390),
    ("ESMLFzm_w6_TW-4.63", 0x0391),
    ("EGLO_ZM_Dimmable-4.60", 0x0392),
    ("EGLO_ZM_Dimmable-4.62", 0x0393),
    ("EGLO_ZM_TW-4.60", 0x0394),
    ("EGLO_ZM_TW-4.62", 0x0395),
    ("EGLO_ZM_TW-4.69", 0x0396),
    ("EGLO_ZM_TW_CLP-4.60", 0x0397),
    ("ESMLzm_TW_GU10-4.63", 0x0398),
    ("EGLO_ZM_RGB_TW-4.60", 0x0399),
    ("EGLO_ZM_RGB_TW-4.62", 0x039A),
    ("EGLO_ZM_RGB_TW-4.69", 0x039B),
    ("EGLO_ZM_RGBTW_CLP-4.60", 0x039C),
    ("ESMLzm_c5_E14-4.62", 0x039D),
    ("ESMLzm_c5_GU10-4.62", 0x039E),
    ("ESMLzm_c9-4.62", 0x039F),
    ("ESMLzm_c13g-4.62", 0x03A0),
    ("EGLO_ZM_RGB_W-4.62", 0x03A1),
    ("EBF_RGB_Zm-4.60", 0x03A2),
    ("EBF_RGB_Zm-4.69", 0x03A3),
    ("EBF_RGB_Zm_CLP-4.60", 0x03A4),
];

struct Out {
    pwm: u32,
    gpio: u32,
    bit: u32,
    mux: u32,
    shift: u32,
    value: u32,
}

struct Board {
    name: String,
    model: String,
    product: u16,
    hw_revision: String,
    hw_version: u32,
    warm: Out,
    cold: Option<Out>,
    rgb: Option<[Out; 3]>,
    beside: bool,
    colour_default: (u32, u32),
    kind: &'static str,
    period: u64,
    ct: (i64, i64),
    image_type: u16,
}

fn out_of(name: &str, pwm: u32, pins: &[(String, String)]) -> Result<Out> {
    let func = format!("PWM{pwm}");
    let pin = pins.iter().find(|(_, f)| *f == func).map(|(p, _)| p.clone())
        .ok_or_else(|| anyhow!("{name}: no pin carries {func}"))?;
    let &(_, mux, shift, value, _) = PWM_MUX.iter().find(|e| e.0 == pin && e.4 == func)
        .ok_or_else(|| anyhow!("{name}: {pin} cannot carry {func}"))?;
    let mut c = pin.chars().skip(1);
    let port = c.next().unwrap_or('?');
    let n = c.next().and_then(|d| d.to_digit(10)).ok_or_else(|| anyhow!("{name}: pin {pin}"))?;
    Ok(Out { pwm, gpio: gpio_func(port).ok_or_else(|| anyhow!("{name}: port of {pin}"))?, bit: 1 << n, mux, shift, value })
}

fn pwm(row: &Value, c: u32) -> f64 {
    row.get(format!("pwm{c}")).and_then(Value::as_f64).unwrap_or(0.0)
}

fn rows<'a>(p: &'a Value, key: &str) -> Result<&'a Vec<Value>> {
    p.get(key).and_then(Value::as_array).ok_or_else(|| anyhow!("no {key}"))
}

fn board(p: &Value, image_type: u16) -> Result<Board> {
    let s = |k: &str| p.get(k).and_then(Value::as_str).map(str::to_string).ok_or_else(|| anyhow!("no {k}"));
    let (model, hw) = (s("model")?, s("hw")?);
    let name = format!("{model}-{hw}");
    let pins: Vec<(String, String)> = p["outputs_on"]["pins"].as_array().ok_or_else(|| anyhow!("{name}: no pins"))?
        .iter().filter_map(|x| Some((x.get(0)?.as_str()?.to_string(), x.get(1)?.as_str()?.to_string()))).collect();
    if pins.iter().any(|(_, f)| f.ends_with("_N")) {
        bail!("{name}: an inverted PWM output, which the firmware does not drive");
    }
    let mut chans: Vec<u32> = rows(p, "level_sweep")?.iter()
        .flat_map(|r| r.as_object().into_iter().flat_map(|o| o.keys()))
        .filter_map(|k| k.strip_prefix("pwm").and_then(|d| d.parse().ok()))
        .collect::<BTreeSet<u32>>().into_iter().collect();
    let tunable = p.get("ct_sweep").is_some();
    let mut rgb: Option<Vec<u32>> = None;
    if let Some(hue) = p.get("hue_sweep").and_then(Value::as_array) {
        let mut picked = Vec::new();
        for h in [0, 84, 168] {
            let row = hue.iter().find(|r| r["v"].as_i64() == Some(h)).ok_or_else(|| anyhow!("{name}: no hue {h}"))?;
            let top: Vec<u32> = chans.iter().copied().filter(|&c| pwm(row, c) > 99.0 && !picked.contains(&c)).collect();
            let narrowed: Vec<u32> = top.iter().copied().filter(|&c| !hue.iter().all(|r| pwm(r, c) > 99.0)).collect();
            let t = if narrowed.is_empty() { top } else { narrowed };
            if t.len() != 1 {
                bail!("{name}: no single channel at full for hue {h}: {t:?}");
            }
            picked.push(t[0]);
        }
        chans.retain(|c| !picked.contains(c));
        rgb = Some(picked);
    }
    let color = |k: &str| p["color"].get(k).and_then(Value::as_i64).ok_or_else(|| anyhow!("{name}: no colour attribute {k}"));
    let (warm, cold, ct) = if rgb.is_some() && chans.len() == 1 {
        (chans[0], None, (color("0x400b")?, color("0x400c")?))
    } else if tunable {
        let sweep = rows(p, "ct_sweep")?;
        let (first, last) = (&sweep[0], &sweep[sweep.len() - 1]);
        let cold: Vec<u32> = chans.iter().copied().filter(|&c| pwm(first, c) > 99.0 && pwm(last, c) < 1.0).collect();
        let warm: Vec<u32> = chans.iter().copied().filter(|&c| pwm(last, c) > 99.0 && pwm(first, c) < 1.0).collect();
        if chans.len() != 2 || cold.len() != 1 || warm.len() != 1 {
            bail!("{name}: not a warm and a cold channel: {chans:?}");
        }
        (warm[0], Some(cold[0]), (color("0x400b")?, color("0x400c")?))
    } else {
        if chans.len() != 1 {
            bail!("{name}: not one channel: {chans:?}");
        }
        (chans[0], None, (153, 454))
    };
    let all: Vec<u32> = chans.iter().copied().chain(rgb.iter().flatten().copied()).collect();
    let periods: BTreeSet<u64> = all.iter()
        .map(|&c| p["outputs_on"]["chans"][c as usize]["max"].as_u64().unwrap_or(0)).collect();
    if periods.len() != 1 {
        bail!("{name}: channels with different periods {periods:?}");
    }
    let beside = model.starts_with("EBF");
    let kind = match (&rgb, cold) {
        (Some(_), _) if beside => "outglow-ebf",
        (Some(_), Some(_)) => "outglow-rgbtw",
        (Some(_), None) => "outglow-rgbw",
        (None, Some(_)) => "outglow-tw",
        (None, None) => "outglow-dimm",
    };
    let product = p["ble_product"][0].as_str().and_then(|s| u16::from_str_radix(s.trim_start_matches("0x"), 16).ok())
        .ok_or_else(|| anyhow!("{name}: no BLE product"))?;
    let hw_version = hw.split('.').nth(1).and_then(|v| v.parse().ok()).ok_or_else(|| anyhow!("{name}: hw {hw}"))?;
    let rgb = match rgb {
        Some(v) => Some([out_of(&name, v[0], &pins)?, out_of(&name, v[1], &pins)?, out_of(&name, v[2], &pins)?]),
        None => None,
    };
    Ok(Board {
        warm: out_of(&name, warm, &pins)?,
        cold: cold.map(|c| out_of(&name, c, &pins)).transpose()?,
        rgb,
        beside,
        colour_default: if beside { (160, 254) } else { (170, 254) },
        kind,
        period: *periods.iter().next().unwrap(),
        ct,
        image_type,
        product,
        hw_version,
        hw_revision: hw,
        model,
        name,
    })
}

fn rs_out(o: &Out) -> String {
    format!("Out {{ pwm: {}, gpio: {:#05x}, bit: {:#04x}, mux: {:#05x}, shift: {}, value: {} }}", o.pwm, o.gpio, o.bit, o.mux, o.shift, o.value)
}

fn firmware_table(boards: &[Board]) -> String {
    let mut s = String::from("use crate::board::{Board, Out};\n\n");
    let _ = writeln!(s, "pub const BOARDS: [Board; {}] = [", boards.len());
    for b in boards {
        let _ = writeln!(s, "    Board {{");
        let _ = writeln!(s, "        name: \"{}\",", b.name);
        let _ = writeln!(s, "        tag: \"outglow-board:{}\",", b.name);
        let _ = writeln!(s, "        model: \"{}\",", b.model);
        let _ = writeln!(s, "        ble_product: {:#06x},", b.product);
        let _ = writeln!(s, "        hw_revision: \"{}\",", b.hw_revision);
        let _ = writeln!(s, "        hw_version: {},", b.hw_version);
        let _ = writeln!(s, "        warm: {},", rs_out(&b.warm));
        let _ = writeln!(s, "        cold: {},", b.cold.as_ref().map(|c| format!("Some({})", rs_out(c))).unwrap_or("None".into()));
        let _ = writeln!(s, "        rgb: {},", b.rgb.as_ref().map(|r| format!("Some([{}])", r.iter().map(rs_out).collect::<Vec<_>>().join(", "))).unwrap_or("None".into()));
        let _ = writeln!(s, "        rgb_beside_white: {},", b.beside);
        let _ = writeln!(s, "        colour_default: ({}, {}),", b.colour_default.0, b.colour_default.1);
        let _ = writeln!(s, "        kind: \"{}\",", b.kind);
        let _ = writeln!(s, "        period: {},", b.period);
        let _ = writeln!(s, "        ct_min: {},", b.ct.0);
        let _ = writeln!(s, "        ct_max: {},", b.ct.1);
        let _ = writeln!(s, "        image_type: {:#06x},", b.image_type);
        let _ = writeln!(s, "    }},");
    }
    s.push_str("];\n");
    s
}

fn tsv(boards: &[Board]) -> String {
    let mut s = String::from("# name\ttunable\timage_type (generated by `cargo xtask boards`)\n");
    for b in boards {
        let tunable = b.cold.is_some() || b.rgb.is_some();
        let _ = writeln!(s, "{}\t{}\t{:#06x}", b.name, tunable as u8, b.image_type);
    }
    s
}

fn tools_table(boards: &[Board]) -> String {
    let mut s = String::from(
        "",
    );
    let _ = writeln!(s, "pub const BOARDS: [Known; {}] = [", boards.len());
    for b in boards {
        let _ = writeln!(s, "    Known {{ name: \"{}\", model: \"{}\", hw_revision: \"{}\", ble_product: {:#06x}, kind: \"{}\", image_type: {:#06x} }},",
                         b.name, b.model, b.hw_revision, b.product, b.kind, b.image_type);
    }
    s.push_str("];\n");
    s
}

fn site_json(boards: &[Board]) -> String {
    let v: Vec<Value> = boards.iter().map(|b| serde_json::json!({
        "name": b.name, "model": b.model, "ble_product": b.product, "hw_revision": b.hw_revision,
        "kind": b.kind, "image_type": b.image_type,
    })).collect();
    serde_json::to_string_pretty(&v).unwrap() + "\n"
}

pub fn run(a: Args) -> Result<()> {
    let root = crate::root();
    let dir = a.profiles.unwrap_or_else(|| root.join("analysis/profiles"));
    let mut boards = Vec::new();
    for (name, image_type) in IMAGE_TYPES {
        let (model, hw) = name.rsplit_once('-').unwrap();
        let path = dir.join(format!("{model}__{hw}.json"));
        let p: Value = serde_json::from_slice(&std::fs::read(&path).with_context(|| format!("no profile for {name}: {}", path.display()))?)?;
        if p.get("joined").and_then(Value::as_bool) != Some(true) {
            bail!("{}: the profile did not join a network", path.display());
        }
        boards.push(board(&p, image_type).with_context(|| path.display().to_string())?);
    }
    let targets = [
        (root.join("crates/bulb-tc32/src/boards.rs"), firmware_table(&boards)),
        (root.join("crates/bulb-tc32/boards.tsv"), tsv(&boards)),
        (root.join("crates/outglow-image/src/boards_table.rs"), tools_table(&boards)),
        (root.join("site/public/boards.json"), site_json(&boards)),
    ];
    if a.check {
        let stale: Vec<String> = targets.iter()
            .filter(|(p, t)| std::fs::read_to_string(p).ok().as_deref() != Some(t.as_str()))
            .map(|(p, _)| p.strip_prefix(&root).unwrap_or(p).display().to_string()).collect();
        if !stale.is_empty() {
            bail!("out of date (run `cargo xtask boards`): {}", stale.join(", "));
        }
        println!("{} boards, all files up to date", boards.len());
        return Ok(());
    }
    for (p, t) in &targets {
        std::fs::write(p, t)?;
        println!("written {}", p.strip_prefix(&root).unwrap_or(p).display());
    }
    Ok(())
}
