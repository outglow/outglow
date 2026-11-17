use serde_json::json;

use super::*;
use crate::sh::fake::Tmp;

fn row(v: i64, pwm: &[(u32, f64)]) -> Value {
    let mut r = json!({ "v": v });
    for (c, x) in pwm {
        r[format!("pwm{c}")] = json!(x);
    }
    r
}

fn profile(model: &str, pins: &[(&str, &str)], level: &[u32]) -> Value {
    let chans: Vec<Value> = (0..6).map(|c| json!({"ch": c, "max": 48000})).collect();
    json!({
        "model": model,
        "hw": "4.62",
        "joined": true,
        "ble_product": ["0x2096"],
        "outputs_on": {"pins": pins.iter().map(|(p, f)| json!([p, f])).collect::<Vec<_>>(), "chans": chans},
        "level_sweep": [row(1, &level.iter().map(|&c| (c, 1.0)).collect::<Vec<_>>()), {"other": 1}],
        "color": {"0x400b": 153, "0x400c": 370},
    })
}

fn tunable(mut p: Value, warm: u32, cold: u32) -> Value {
    p["ct_sweep"] = json!([row(153, &[(cold, 100.0), (warm, 0.0)]), row(300, &[(cold, 50.0), (warm, 50.0)]), row(370, &[(cold, 0.0), (warm, 100.0)])]);
    p
}

fn coloured(mut p: Value, rgb: [u32; 3], always: Option<u32>) -> Value {
    let hue: Vec<Value> = [0, 42, 84, 168]
        .iter()
        .map(|&h| {
            let full = match h {
                0 => Some(rgb[0]),
                84 => Some(rgb[1]),
                168 => Some(rgb[2]),
                _ => None,
            };
            let mut pwm: Vec<(u32, f64)> = rgb.iter().map(|&c| (c, if Some(c) == full { 100.0 } else { 0.0 })).collect();
            pwm.extend(always.map(|c| (c, 100.0)));
            row(h, &pwm)
        })
        .collect();
    p["hue_sweep"] = json!(hue);
    p
}

#[track_caller]
fn err(p: &Value) -> String {
    format!("{:#}", board(p, 0x0390).err().unwrap())
}

#[test]
fn a_dimmable_board() {
    let b = board(&profile("ESMLFzm_w6_Dimm", &[("PC1", "PWM0")], &[0]), 0x0390).unwrap();
    assert_eq!((b.name.as_str(), b.model.as_str(), b.hw_revision.as_str(), b.hw_version), ("ESMLFzm_w6_Dimm-4.62", "ESMLFzm_w6_Dimm", "4.62", 62));
    assert_eq!((b.product, b.kind, b.period, b.ct, b.image_type, b.beside, b.colour_default), (0x2096, "outglow-dimm", 48000, (153, 454), 0x0390, false, (170, 254)));
    assert_eq!(rs_out(&b.warm), "Out { pwm: 0, gpio: 0x596, bit: 0x02, mux: 0x5ac, shift: 2, value: 2 }");
    assert!(b.cold.is_none() && b.rgb.is_none());
}

#[test]
fn a_tunable_white_board() {
    let p = tunable(profile("T", &[("PC1", "PWM0"), ("PB5", "PWM5")], &[0, 5]), 0, 5);
    let b = board(&p, 0x0391).unwrap();
    assert_eq!((b.kind, b.ct), ("outglow-tw", (153, 370)));
    assert_eq!(rs_out(b.cold.as_ref().unwrap()), "Out { pwm: 5, gpio: 0x58e, bit: 0x20, mux: 0x5ab, shift: 2, value: 1 }");
}

#[test]
fn colour_boards() {
    let pins = [("PA2", "PWM0"), ("PA3", "PWM1"), ("PA4", "PWM2"), ("PB0", "PWM3"), ("PB1", "PWM4")];
    let p = coloured(tunable(profile("RGBTW", &pins, &[0, 1, 2, 3, 4]), 0, 1), [2, 3, 4], None);
    let b = board(&p, 1).unwrap();
    assert_eq!((b.kind, b.ct), ("outglow-rgbtw", (153, 370)));
    let rgb: Vec<String> = b.rgb.as_ref().unwrap().iter().map(rs_out).collect();
    assert_eq!(rgb[0], "Out { pwm: 2, gpio: 0x586, bit: 0x10, mux: 0x5a9, shift: 0, value: 2 }");
    assert_eq!(rgb[2], "Out { pwm: 4, gpio: 0x58e, bit: 0x02, mux: 0x5aa, shift: 2, value: 0 }");
    let p = coloured(profile("RGBW", &pins[..4], &[0, 1, 2, 3]), [1, 2, 3], Some(0));
    let b = board(&p, 1).unwrap();
    assert_eq!((b.kind, b.ct, rs_out(&b.warm).starts_with("Out { pwm: 0,")), ("outglow-rgbw", (153, 370), true));
    let p = coloured(tunable(profile("EBF_RGB", &pins, &[0, 1, 2, 3, 4]), 0, 1), [2, 3, 4], None);
    let b = board(&p, 1).unwrap();
    assert_eq!((b.kind, b.beside, b.colour_default), ("outglow-ebf", true, (160, 254)));
}

#[test]
fn profiles_the_firmware_cannot_drive() {
    let dimm = || profile("M", &[("PC1", "PWM0")], &[0]);
    let mut p = dimm();
    p.as_object_mut().unwrap().remove("hw");
    assert_eq!(err(&p), "no hw");
    let mut p = dimm();
    p["outputs_on"]["pins"] = json!(null);
    assert_eq!(err(&p), "M-4.62: no pins");
    let mut p = dimm();
    p["outputs_on"]["pins"] = json!([["PA0", "PWM0_N"], [1, 2], ["PC1"]]);
    assert_eq!(err(&p), "M-4.62: an inverted PWM output, which the firmware does not drive");
    let mut p = dimm();
    p.as_object_mut().unwrap().remove("level_sweep");
    assert_eq!(err(&p), "no level_sweep");
    assert_eq!(err(&profile("M", &[("PC1", "PWM0")], &[0, 1])), "M-4.62: not one channel: [0, 1]");
    let tw = |level: &[u32]| tunable(profile("M", &[("PC1", "PWM0"), ("PB5", "PWM5")], level), 0, 5);
    assert_eq!(err(&tw(&[0, 5, 1])), "M-4.62: not a warm and a cold channel: [0, 1, 5]");
    let mut p = tw(&[0, 5]);
    p["ct_sweep"][2] = row(370, &[(5, 0.0), (0, 99.0)]);
    assert_eq!(err(&p), "M-4.62: not a warm and a cold channel: [0, 5]");
    let mut p = tw(&[0, 5]);
    p["ct_sweep"][0] = row(153, &[(5, 99.0), (0, 0.0)]);
    assert_eq!(err(&p), "M-4.62: not a warm and a cold channel: [0, 5]");
    let mut p = tw(&[0, 5]);
    p["ct_sweep"][0] = row(153, &[(5, 100.0), (0, 1.0)]);
    assert_eq!(err(&p), "M-4.62: not a warm and a cold channel: [0, 5]");
    let mut p = tw(&[0, 5]);
    p["ct_sweep"][2] = row(370, &[(5, 1.0), (0, 100.0)]);
    assert_eq!(err(&p), "M-4.62: not a warm and a cold channel: [0, 5]");
    let mut p = tw(&[0, 5]);
    p["ct_sweep"] = json!({});
    assert_eq!(err(&p), "no ct_sweep");
    let mut p = tw(&[0, 5]);
    p["color"] = json!({"0x400b": 153});
    assert_eq!(err(&p), "M-4.62: no colour attribute 0x400c");
    let mut p = dimm();
    p["outputs_on"]["chans"][0]["max"] = json!(1000);
    p["outputs_on"]["pins"] = json!([["PC1", "PWM0"], ["PB5", "PWM5"]]);
    let p = tunable(Value::clone(&p), 0, 5);
    let mut p = p;
    p["level_sweep"] = json!([row(1, &[(0, 1.0), (5, 1.0)])]);
    assert_eq!(err(&p), "M-4.62: channels with different periods {1000, 48000}");
    let mut p = dimm();
    p["ble_product"] = json!(["zz"]);
    assert_eq!(err(&p), "M-4.62: no BLE product");
    let mut p = dimm();
    p["hw"] = json!("4");
    assert_eq!(err(&p), "M-4: hw 4");
    let p = profile("M", &[("PC2", "PWM0")], &[1]);
    assert_eq!(err(&p), "M-4.62: no pin carries PWM1");
    let p = profile("M", &[("PA0", "PWM1")], &[1]);
    assert_eq!(err(&p), "M-4.62: PA0 cannot carry PWM1");
    let p = profile("M", &[("PC1", "PWM1")], &[1]);
    assert_eq!(err(&p), "M-4.62: PC1 cannot carry PWM1");
}

#[test]
fn colour_profiles_the_firmware_cannot_drive() {
    let pins = [("PA2", "PWM0"), ("PA3", "PWM1"), ("PA4", "PWM2"), ("PB0", "PWM3")];
    let p = coloured(profile("C", &pins, &[0, 1, 2, 3]), [1, 2, 3], Some(0));
    let mut q = p.clone();
    q["hue_sweep"][2]["v"] = json!(85);
    assert_eq!(err(&q), "C-4.62: no hue 84");
    let mut q = p.clone();
    q["hue_sweep"][2]["pwm1"] = json!(100.0);
    assert!(board(&q, 1).is_ok());
    q["hue_sweep"][2]["pwm3"] = json!(100.0);
    assert_eq!(err(&q), "C-4.62: no single channel at full for hue 84: [2, 3]");
    let mut q = p.clone();
    q["hue_sweep"][3]["pwm2"] = json!(100.0);
    assert_eq!(board(&q, 1).unwrap().rgb.unwrap()[2].pwm, 3);
    q["hue_sweep"][3]["pwm3"] = json!(0.0);
    q["hue_sweep"][3]["pwm0"] = json!(0.0);
    assert_eq!(err(&q), "C-4.62: no single channel at full for hue 0: [0, 1]");
    let mut q = p.clone();
    q["hue_sweep"][0]["pwm1"] = json!(99.0);
    assert!(board(&q, 1).is_ok());
    let mut q = p.clone();
    q["color"] = json!({"0x400c": 1});
    assert_eq!(err(&q), "C-4.62: no colour attribute 0x400b");
    let narrow = coloured(profile("C", &pins, &[0, 1, 2, 3]), [1, 2, 3], Some(0));
    let all_full: Vec<bool> = narrow["hue_sweep"].as_array().unwrap().iter().map(|r| pwm(r, 0) > 99.0).collect();
    assert_eq!(all_full, [true; 4]);
    assert_eq!(pwm(&json!({}), 0), 0.0);
}

#[test]
fn the_generated_files_are_up_to_date() {
    run_in(&crate::root(), Args { check: true, profiles: None }).unwrap();
}

#[test]
fn generating_writes_the_four_files() {
    let t = Tmp::new("boards");
    for d in ["crates/bulb-tc32/src", "crates/outglow-image/src", "site/public"] {
        std::fs::create_dir_all(t.path(d)).unwrap();
    }
    let profiles = crate::root().join("analysis/profiles");
    let e = run_in(&t.0, Args { check: true, profiles: Some(profiles.clone()) }).unwrap_err().to_string();
    assert_eq!(
        e,
        "out of date (run `cargo xtask boards`): crates/bulb-tc32/src/boards.rs, crates/bulb-tc32/boards.tsv, crates/outglow-image/src/boards_table.rs, site/public/boards.json"
    );
    run_in(&t.0, Args { check: false, profiles: Some(profiles.clone()) }).unwrap();
    for f in ["crates/bulb-tc32/src/boards.rs", "crates/bulb-tc32/boards.tsv", "crates/outglow-image/src/boards_table.rs", "site/public/boards.json"] {
        assert_eq!(std::fs::read_to_string(t.path(f)).unwrap(), std::fs::read_to_string(crate::root().join(f)).unwrap(), "{f}");
    }
    run_in(&t.0, Args { check: true, profiles: Some(profiles) }).unwrap();
    std::fs::remove_dir_all(t.path("site")).unwrap();
    assert!(run_in(&t.0, Args { check: false, profiles: Some(crate::root().join("analysis/profiles")) }).is_err());
}

#[test]
fn a_missing_or_unjoined_profile() {
    let t = Tmp::new("profiles");
    let e = run_in(&t.0, Args { check: true, profiles: Some(t.0.clone()) }).unwrap_err().to_string();
    assert_eq!(e, format!("no profile for ESMLFzm_w6_Dimm-4.63: {}", t.path("ESMLFzm_w6_Dimm__4.63.json").display()));
    let p = t.put("ESMLFzm_w6_Dimm__4.63.json", r#"{"joined": false}"#);
    let e = run_in(&t.0, Args { check: true, profiles: Some(t.0.clone()) }).unwrap_err().to_string();
    assert_eq!(e, format!("{}: the profile did not join a network", p.display()));
    t.put("ESMLFzm_w6_Dimm__4.63.json", r#"{"joined": true}"#);
    let e = run_in(&t.0, Args { check: true, profiles: Some(t.0.clone()) }).unwrap_err();
    assert_eq!(format!("{e:#}"), format!("{}: no model", p.display()));
    t.put("ESMLFzm_w6_Dimm__4.63.json", "not json");
    assert!(run_in(&t.0, Args { check: true, profiles: Some(t.0.clone()) }).is_err());
}
