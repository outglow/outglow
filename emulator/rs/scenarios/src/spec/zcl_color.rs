use super::zcl_onoff::{act, ans_py, attr, check_types, finally, py_f, round1, status, write_attr};
use super::Test;
use crate::frames::py_value;
use crate::session::{expect, CmdAnswer, Session, Stop, R};

const S: &str = "zcl-color";
const ONOFF: u16 = 0x0006;
const LEVEL: u16 = 0x0008;
const COLOR: u16 = 0x0300;
const CT: u16 = 0x0007;
const MODE: u16 = 0x0008;
const OPTIONS: u16 = 0x000F;
const EMODE: u16 = 0x4001;
const CAPS: u16 = 0x400A;
const PMIN: u16 = 0x400B;
const PMAX: u16 = 0x400C;
const CMIN: u16 = 0x400D;
const STARTUP: u16 = 0x4010;
const REMAINING: u16 = 0x0002;
const SETTLE: f64 = 1000.0;

fn t(id: &'static str, doc: &'static str, clause: &'static str, req: &'static str, run: fn(&mut Session) -> R<String>) -> Test {
    Test { id, doc, clause, req, session: S, run, ..Test::D }
}

pub fn tests() -> Vec<Test> {
    vec![
        t("ZCL-COLOR-01", "ZCL8", "5.2.2.2.1 Table 5.3",
          "A Color Control server implements ColorMode, Options, EnhancedColorMode and ColorCapabilities, whatever \
           its capabilities.", base_attributes),
        t("ZCL-COLOR-02", "ZLO", "18.2.1 Table 38; ZCL8 5.2.2.2.1 Table 5.3",
          "A colour temperature light implements ColorTemperatureMireds, ColorTempPhysicalMinMireds, \
           ColorTempPhysicalMaxMireds, CoupleColorTempToLevelMinMireds, StartUpColorTemperatureMireds (uint16) and \
           RemainingTime (uint16).", ct_attributes),
        t("ZCL-COLOR-03", "ZCL8", "5.2.2.2.1.19-5.2.2.2.1.21",
          "ColorTempPhysicalMinMireds <= CoupleColorTempToLevelMinMireds <= ColorTemperatureMireds <= \
           ColorTempPhysicalMaxMireds.", ct_invariants),
        t("ZCL-COLOR-04", "ZCL8", "5.2.2.3.14.3",
          "Move to Color Temperature sets ColorMode (and EnhancedColorMode) to 0x02 and moves to the colour \
           temperature given; the light output follows.", move_to_color_temperature),
        t("ZCL-COLOR-05", "ZCL8", "5.2.2.3.1",
          "Move to Color Temperature to a target the hardware cannot reach is clipped at the physical minimum or \
           maximum and still answered with SUCCESS.", move_to_ct_clipped),
        t("ZCL-COLOR-06", "ZCL8", "5.2.2.3.14.3",
          "Move to Color Temperature is a continuous movement that takes the transition time given.",
          move_to_ct_transition),
        t("ZCL-COLOR-07", "ZCL8", "5.2.2.2.1.3",
          "RemainingTime is the time left, in 1/10 s, until the active command completes, and 0 afterwards.",
          remaining_time),
        t("ZCL-COLOR-08", "ZCL8", "5.2.2.3.21.6 Table 5.27",
          "Move Color Temperature changes ColorTemperatureMireds at the rate given, stops on move mode Stop, and is \
           clipped at the Minimum/Maximum Mireds fields or the physical limits.", move_color_temperature),
        t("ZCL-COLOR-09", "ZCL8", "5.2.2.3.20.2",
          "Stop Move Step terminates a Move to Color Temperature and a Move Color Temperature in progress and sets \
           RemainingTime to 0.", stop_move_step),
        t("ZCL-COLOR-10", "ZCL8", "5.2.2.3.22.7 Table 5.28",
          "Step Color Temperature changes ColorTemperatureMireds by the step size, clipped at the Minimum/Maximum \
           Mireds fields or the physical limits.", step_color_temperature),
        t("ZCL-COLOR-11", "ZCL8", "5.2.2.3.21.2",
          "Move Color Temperature up or down with rate 0 has no effect and is answered with Default Response \
           INVALID_FIELD.", move_ct_rate_zero),
        t("ZCL-COLOR-12", "ZCL8", "5.2.2.2.1.10, 5.2.2.3.1",
          "With ExecuteIfOff = 0 a colour command received while OnOff is off is not executed.",
          not_executed_when_off),
        Test {
            ambiguous: Some(
                "5.2.2.3.1 still carries the older sentence that a colour command received in the off state SHALL be \
                 ignored; the test follows the Options text (5.2.2.2.1.10, 5.2.2.3.3), which is the newer and more \
                 specific one",
            ),
            ..t("ZCL-COLOR-13", "ZCL8", "5.2.2.2.1.10, 5.2.2.3.3",
                "With ExecuteIfOff = 1, from the Options attribute or from OptionsMask/OptionsOverride, a colour \
                 command is executed while OnOff is off; an override that clears the bit prevents it.",
                options_execute_if_off)
        },
        t("ZCL-COLOR-14", "ZCL8", "5.2.2.1.1, 5.2.2.2.1.21",
          "With CoupleColorTempToLevel = 1 in the Level Control Options and colour mode 0x02, a change of \
           CurrentLevel changes ColorTemperatureMireds: the maximum level gives CoupleColorTempToLevelMinMireds, a \
           low level a higher mired value; with the bit 0 the level has no influence.", couple_to_level),
        t("ZCL-COLOR-15", "ZCL8", "5.2.2.2.1.18",
          "A unicast Color Control command the device does not support is answered with a Default Response carrying \
           the unsupported-command status.", unsupported_command),
        t("ZCL-COLOR-16", "ZCL8", "5.2.2.2.1.22 Table 5.9",
          "StartUpColorTemperatureMireds decides ColorTemperatureMireds after power is applied (a value is used as \
           is, 0xffff restores the previous value) and the colour mode is 0x02.", start_up_color_temperature),
    ]
}

type Mix = (f64, f64);

fn mx(m: Mix) -> String {
    format!("({}, {})", py_f(m.0), py_f(m.1))
}

fn need_ct(s: &mut Session) -> R<(i64, i64)> {
    if !s.has_note("ct") {
        s.need_cluster(COLOR)?;
        let (st, _, caps) = s.read1(COLOR, CAPS)?;
        let (ok, why) = if st == Some(0) {
            let c = caps.as_ref().and_then(|v| v.int()).unwrap_or(0);
            (c & 0x10 != 0, format!("ColorCapabilities {c:#06x} has no colour temperature bit"))
        } else {
            (
                s.read1(COLOR, CT)?.0 == Some(0),
                "the simple descriptor lists Color Control, but neither ColorCapabilities nor ColorTemperatureMireds \
                 is implemented"
                    .to_string(),
            )
        };
        if ok {
            let lo = s.read1(COLOR, PMIN)?;
            let hi = s.read1(COLOR, PMAX)?;
            if lo.0 != Some(0) || hi.0 != Some(0) {
                return Err(Stop::Inconclusive("ColorTempPhysicalMinMireds/MaxMireds cannot be read".into()));
            }
            let v = |x: &Option<crate::frames::ZclValue>| x.as_ref().and_then(|v| v.int()).unwrap_or(0);
            s.set_note::<Result<(i64, i64), String>>("ct", Ok((v(&lo.2), v(&hi.2))));
        } else {
            s.set_note::<Result<(i64, i64), String>>("ct", Err(why));
        }
    }
    match s.note::<Result<(i64, i64), String>>("ct").unwrap() {
        Ok(r) => Ok(r),
        Err(why) => Err(Stop::NotApplicable(why)),
    }
}

fn ct(s: &mut Session) -> R<i64> {
    attr(s, COLOR, CT)
}

fn mtc_payload(mireds: i64, tt: u16, mask: u8, ovr: u8) -> Vec<u8> {
    let mut p = (mireds as u16).to_le_bytes().to_vec();
    p.extend_from_slice(&tt.to_le_bytes());
    p.extend_from_slice(&[mask, ovr]);
    p
}

fn move_to_ct(s: &mut Session, mireds: i64, tt: u16, settle: f64) -> R<()> {
    act(
        s,
        COLOR,
        0x0A,
        &mtc_payload(mireds, tt, 0, 0),
        &format!("Move to Color Temperature {mireds} in {:.1} s", tt as f64 / 10.0),
        settle,
    )
}

fn mct_payload(mode: u8, rate: u16, lo: i64, hi: i64) -> Vec<u8> {
    let mut p = vec![mode];
    p.extend_from_slice(&rate.to_le_bytes());
    p.extend_from_slice(&(lo as u16).to_le_bytes());
    p.extend_from_slice(&(hi as u16).to_le_bytes());
    p.extend_from_slice(&[0, 0]);
    p
}

fn move_ct(s: &mut Session, mode: u8, rate: u16, lo: i64, hi: i64) -> R<()> {
    act(s, COLOR, 0x4B, &mct_payload(mode, rate, lo, hi), &format!("Move Color Temperature mode {mode} rate {rate}"), 0.0)
}

fn sct_payload(mode: u8, size: u16, tt: u16, lo: i64, hi: i64) -> Vec<u8> {
    let mut p = vec![mode];
    p.extend_from_slice(&size.to_le_bytes());
    p.extend_from_slice(&tt.to_le_bytes());
    p.extend_from_slice(&(lo as u16).to_le_bytes());
    p.extend_from_slice(&(hi as u16).to_le_bytes());
    p.extend_from_slice(&[0, 0]);
    p
}

fn step_ct(s: &mut Session, mode: u8, size: u16, tt: u16, lo: i64, hi: i64) -> R<()> {
    act(
        s,
        COLOR,
        0x4C,
        &sct_payload(mode, size, tt, lo, hi),
        &format!("Step Color Temperature mode {mode} size {size}"),
        tt as f64 * 100.0 + 700.0,
    )
}

fn mix(s: &Session) -> Mix {
    let l = s.light();
    (round1(l.duty0), round1(l.duty5))
}

fn light_on(s: &mut Session) -> R<()> {
    act(s, ONOFF, 0x01, &[], "On", 200.0)?;
    act(s, LEVEL, 0x04, &[254, 0, 0, 0, 0], "Move to Level (with On/Off) 254", 800.0)
}

fn set_options(s: &mut Session, value: u8) -> R<i64> {
    let cur = attr(s, COLOR, OPTIONS)?;
    if cur != value as i64 {
        let st = write_attr(s, COLOR, OPTIONS, 0x18, vec![value])?;
        if st != 0 {
            return Err(Stop::Inconclusive(format!(
                "the Options attribute could not be written ({value:#04x}: status {st:#04x})"
            )));
        }
    }
    Ok(cur)
}

fn stable(s: &mut Session, ms: f64) -> R<(bool, Mix, Mix)> {
    let a = mix(s);
    s.run(ms)?;
    let b = mix(s);
    Ok((a == b, a, b))
}

fn dr_text(a: &CmdAnswer, other: &str) -> String {
    match a {
        CmdAnswer::None => "missing".into(),
        CmdAnswer::Status(st) => format!("status {st:#04x}"),
        CmdAnswer::Frame(_) => other.into(),
    }
}

fn base_attributes(s: &mut Session) -> R<String> {
    s.need_cluster(COLOR)?;
    let Some(r) = s.read(COLOR, &[MODE, OPTIONS, EMODE, CAPS], 1500.0)? else {
        return Err(Stop::Fail(
            "the simple descriptor lists Color Control, but Read Attributes on it is not answered with a Read \
             Attributes Response"
                .into(),
        ));
    };
    let bad = check_types(&r, &[(MODE, 0x30), (OPTIONS, 0x18), (EMODE, 0x30), (CAPS, 0x19)]);
    expect(bad.is_empty(), format!("the simple descriptor lists Color Control; {}", bad.join("; ")))?;
    Ok(String::new())
}

fn ct_attributes(s: &mut Session) -> R<String> {
    need_ct(s)?;
    let mut bad = Vec::new();
    for ids in [[CT, PMIN, PMAX], [CMIN, STARTUP, REMAINING]] {
        let r = s.read(COLOR, &ids, 1500.0)?.unwrap_or_default();
        let want: Vec<(u16, u8)> = ids.iter().map(|a| (*a, 0x21)).collect();
        bad.extend(check_types(&r, &want));
    }
    expect(bad.is_empty(), bad.join("; "))?;
    Ok(String::new())
}

fn ct_invariants(s: &mut Session) -> R<String> {
    let (lo, hi) = need_ct(s)?;
    light_on(s)?;
    move_to_ct(s, (lo + hi).div_euclid(2), 0, SETTLE)?;
    let cur = ct(s)?;
    let cmin = attr(s, COLOR, CMIN)?;
    expect(
        lo <= cmin && cmin <= cur && cur <= hi,
        format!("physical min {lo}, couple-to-level min {cmin}, current {cur}, physical max {hi}"),
    )?;
    Ok(format!("{lo} <= {cmin} <= {cur} <= {hi}"))
}

fn move_to_color_temperature(s: &mut Session) -> R<String> {
    let (lo, hi) = need_ct(s)?;
    light_on(s)?;
    let (a, b) = (lo + (hi - lo).div_euclid(4), hi - (hi - lo).div_euclid(4));
    move_to_ct(s, a, 0, SETTLE)?;
    let va = ct(s)?;
    let ma = mix(s);
    move_to_ct(s, b, 0, SETTLE)?;
    let vb = ct(s)?;
    let mb = mix(s);
    expect((va, vb) == (a, b), format!("ColorTemperatureMireds {va} after Move to {a}, {vb} after Move to {b}"))?;
    expect(ma != mb, format!("the PWM outputs are the same at {a} and {b} mireds: {}", mx(ma)))?;
    let mode = attr(s, COLOR, MODE)?;
    let emode = attr(s, COLOR, EMODE)?;
    expect(mode == 2 && emode == 2, format!("ColorMode {mode}, EnhancedColorMode {emode} after Move to Color Temperature"))?;
    Ok(format!("{a} mireds -> PWM0/PWM5 {}; {b} mireds -> {}", mx(ma), mx(mb)))
}

fn move_to_ct_clipped(s: &mut Session) -> R<String> {
    let (lo, hi) = need_ct(s)?;
    light_on(s)?;
    move_to_ct(s, (lo + hi).div_euclid(2), 0, SETTLE)?;
    let st = s.cmd(COLOR, 0x0A, &mtc_payload(hi + 150, 0, 0, 0))?;
    s.run(SETTLE)?;
    let v_hi = ct(s)?;
    let low = 1.max(lo - 100);
    let st2 = s.cmd(COLOR, 0x0A, &mtc_payload(low, 0, 0, 0))?;
    s.run(SETTLE)?;
    let v_lo = ct(s)?;
    expect(
        status(&st) == Some(0) && status(&st2) == Some(0),
        format!(
            "Default Response status {} / {} (SUCCESS required) for targets {} / {low}",
            ans_py(&st),
            ans_py(&st2),
            hi + 150
        ),
    )?;
    expect(v_hi == hi, format!("ColorTemperatureMireds {v_hi} after Move to {}; physical maximum is {hi}", hi + 150))?;
    expect(v_lo == lo, format!("ColorTemperatureMireds {v_lo} after Move to {low}; physical minimum is {lo}"))?;
    Ok(String::new())
}

fn move_to_ct_transition(s: &mut Session) -> R<String> {
    let (lo, hi) = need_ct(s)?;
    light_on(s)?;
    let (a, b) = (lo + (hi - lo).div_euclid(5), hi - (hi - lo).div_euclid(5));
    move_to_ct(s, b, 0, SETTLE)?;
    let end = mix(s);
    move_to_ct(s, a, 0, SETTLE)?;
    let begin = mix(s);
    if begin == end {
        return Err(Stop::Inconclusive(format!("the PWM outputs are the same at {a} and {b} mireds")));
    }
    let k = if begin.0 != end.0 { 0 } else { 1 };
    let pick = |m: Mix| if k == 0 { m.0 } else { m.1 };
    let (bk, ek) = (pick(begin), pick(end));
    move_to_ct(s, b, 30, 0.0)?;
    let t0 = s.now();
    let mut samples: Vec<(f64, f64)> = Vec::new();
    while s.now() - t0 < 3900.0 {
        s.run(300.0)?;
        samples.push(((s.now() - t0) / 1000.0, pick(mix(s))));
    }
    let txt = samples.iter().map(|(t, d)| format!("{t:.1}s {d:.1}%")).collect::<Vec<_>>().join(", ");
    let (lo_d, hi_d) = if bk <= ek { (bk, ek) } else { (ek, bk) };
    let eps = (hi_d - lo_d) * 0.01;
    let pwm = if k == 0 { "0" } else { "5" };
    expect(
        samples.iter().filter(|(t, _)| (1.0..=2.0).contains(t)).all(|(_, d)| lo_d + eps < *d && *d < hi_d - eps),
        format!(
            "between 1.0 and 2.0 s of a 3.0 s transition PWM{pwm} is not strictly between {}% and {}%: {txt}",
            py_f(bk),
            py_f(ek)
        ),
    )?;
    let sign = if ek > bk { 1.0 } else { -1.0 };
    expect(samples.windows(2).all(|w| (w[1].1 - w[0].1) * sign >= -0.05), format!("not monotonic: {txt}"))?;
    let mut distinct: Vec<u64> = samples.iter().map(|(_, d)| d.to_bits()).collect();
    distinct.sort();
    distinct.dedup();
    expect(distinct.len() >= 5, format!("fewer than 5 distinct outputs during the transition: {txt}"))?;
    expect(
        samples.iter().filter(|(t, _)| *t >= 3.6).all(|(_, d)| (d - ek).abs() < 0.3),
        format!(
            "the output is not at the {b}-mired value {}% 3.6 s after the start of a 3.0 s transition: {txt}",
            py_f(ek)
        ),
    )?;
    let v = ct(s)?;
    expect(v == b, format!("ColorTemperatureMireds {v} after the transition to {b}"))?;
    Ok(format!("PWM{pwm}: {txt}"))
}

fn remaining_time(s: &mut Session) -> R<String> {
    let (lo, hi) = need_ct(s)?;
    attr(s, COLOR, REMAINING)?;
    light_on(s)?;
    move_to_ct(s, lo, 0, SETTLE)?;
    move_to_ct(s, hi, 50, 0.0)?;
    let t0 = s.now();
    let mut obs = Vec::new();
    for wait in [1000.0, 1500.0] {
        s.run(wait)?;
        let rt = attr(s, COLOR, REMAINING)?;
        let el = (s.now() - t0) / 1000.0;
        obs.push(format!("{rt} at {el:.2} s"));
        expect(
            (rt as f64 - (50.0 - el * 10.0)).abs() <= 4.0,
            format!("RemainingTime {rt} at {el:.2} s of a 5.0 s transition ({:.0} expected)", 50.0 - el * 10.0),
        )?;
    }
    let w = (t0 + 5600.0 - s.now()).max(0.0);
    s.run(w)?;
    let rt = attr(s, COLOR, REMAINING)?;
    expect(rt == 0, format!("RemainingTime {rt} 5.6 s after the start of a 5.0 s transition"))?;
    Ok(format!("RemainingTime {}", obs.join(", ")))
}

fn move_color_temperature(s: &mut Session) -> R<String> {
    let (lo, hi) = need_ct(s)?;
    light_on(s)?;
    let a = lo + 20;
    move_to_ct(s, a, 0, SETTLE)?;
    move_ct(s, 0x01, 50, 0, 0)?;
    let t0 = s.now();
    s.run(2000.0)?;
    move_ct(s, 0x00, 0, 0, 0)?;
    let dt = (s.now() - t0) / 1000.0;
    let before = mix(s);
    let (ok, m1, m2) = stable(s, 500.0)?;
    let v = ct(s)?;
    let want = a as f64 + 50.0 * dt;
    expect(
        ok,
        format!("the PWM outputs still change after move mode Stop ({} -> {}); ColorTemperatureMireds {v}", mx(m1), mx(m2)),
    )?;
    expect(
        (v as f64 - want).abs() <= 0.25 * 50.0 * dt + 3.0,
        format!(
            "ColorTemperatureMireds {v} after moving up from {a} at 50/s for {dt:.2} s and move mode Stop (about \
             {want:.0} expected); PWM0/PWM5 right after Stop {}, 0.5 s later {}",
            mx(before),
            mx(m2)
        ),
    )?;
    let mode = attr(s, COLOR, MODE)?;
    let emode = attr(s, COLOR, EMODE)?;
    expect(mode == 2 && emode == 2, format!("ColorMode {mode}, EnhancedColorMode {emode} after Move Color Temperature"))?;
    let obs = format!("up 50/s for {dt:.2} s from {a}: {v}");
    let cap = (hi - 20).min(v + 60);
    move_ct(s, 0x01, 100, 0, cap)?;
    s.run(2000.0)?;
    let v2 = ct(s)?;
    expect(
        v2 == cap,
        format!("ColorTemperatureMireds {v2} 2 s after Move up at 100/s with Maximum Mireds {cap} (from {v})"),
    )?;
    move_ct(s, 0x03, 250, 0, 0)?;
    s.run(2200.0)?;
    let v3 = ct(s)?;
    expect(
        v3 == lo,
        format!(
            "ColorTemperatureMireds {v3} 2.2 s after Move down at 250/s with Minimum Mireds 0; the physical minimum \
             {lo} is the bound"
        ),
    )?;
    Ok(obs)
}

fn stop_move_step(s: &mut Session) -> R<String> {
    let (lo, hi) = need_ct(s)?;
    light_on(s)?;
    move_to_ct(s, lo, 0, SETTLE)?;
    move_to_ct(s, hi, 40, 2000.0)?;
    act(s, COLOR, 0x47, &[0, 0], "Stop Move Step", 0.0)?;
    let (ok, m1, m2) = stable(s, 800.0)?;
    let v = ct(s)?;
    let (st, _, rt) = s.read1(COLOR, REMAINING)?;
    s.run(2500.0)?;
    let v_later = ct(s)?;
    let m3 = mix(s);
    expect(
        ok && m3 == m2,
        format!(
            "Move to Color Temperature: the PWM outputs go on changing after Stop Move Step ({} -> {} -> {}); \
             ColorTemperatureMireds {v} then {v_later}",
            mx(m1),
            mx(m2),
            mx(m3)
        ),
    )?;
    expect(
        lo < v && v < hi && v_later == v,
        format!(
            "Move to Color Temperature stopped 2.0 s into a 4.0 s move from {lo} to {hi}: ColorTemperatureMireds {v}, \
             later {v_later}"
        ),
    )?;
    if st == Some(0) {
        expect(
            rt.as_ref().and_then(|x| x.int()) == Some(0),
            format!("RemainingTime {} after Stop Move Step", py_value(&rt)),
        )?;
    }
    let obs = format!("Move to stopped at {v}");
    move_to_ct(s, lo, 0, SETTLE)?;
    move_ct(s, 0x01, 40, 0, 0)?;
    s.run(1500.0)?;
    act(s, COLOR, 0x47, &[0, 0], "Stop Move Step", 0.0)?;
    let (ok, m1, m2) = stable(s, 800.0)?;
    let v = ct(s)?;
    s.run(1500.0)?;
    let v_later = ct(s)?;
    let m3 = mix(s);
    expect(
        ok && m3 == m2 && v_later == v && lo < v && v < hi,
        format!(
            "Move Color Temperature: after Stop Move Step the outputs are {} -> {} -> {}, ColorTemperatureMireds {v} \
             then {v_later}",
            mx(m1),
            mx(m2),
            mx(m3)
        ),
    )?;
    Ok(format!("{obs}; Move stopped at {v}"))
}

fn step_color_temperature(s: &mut Session) -> R<String> {
    let (lo, hi) = need_ct(s)?;
    light_on(s)?;
    let a = lo + 60;
    move_to_ct(s, a, 0, SETTLE)?;
    step_ct(s, 0x01, 60, 10, 0, 0)?;
    let v = ct(s)?;
    expect(v == a + 60, format!("ColorTemperatureMireds {v} after Step up 60 from {a}"))?;
    step_ct(s, 0x03, 25, 0, 0, 0)?;
    let v = ct(s)?;
    expect(v == a + 35, format!("ColorTemperatureMireds {v} after Step down 25 from {}", a + 60))?;
    let cap = (hi - 10).min(a + 80);
    step_ct(s, 0x01, 500, 10, 0, cap)?;
    let v = ct(s)?;
    expect(v == cap, format!("ColorTemperatureMireds {v} after Step up 500 with Maximum Mireds {cap}"))?;
    step_ct(s, 0x03, 1000, 10, 0, 0)?;
    let v = ct(s)?;
    expect(
        v == lo,
        format!("ColorTemperatureMireds {v} after Step down 1000 with Minimum Mireds 0; physical minimum {lo}"),
    )?;
    let mode = attr(s, COLOR, MODE)?;
    let emode = attr(s, COLOR, EMODE)?;
    expect(mode == 2 && emode == 2, format!("ColorMode {mode}, EnhancedColorMode {emode} after Step Color Temperature"))?;
    Ok(String::new())
}

fn move_ct_rate_zero(s: &mut Session) -> R<String> {
    let (lo, hi) = need_ct(s)?;
    light_on(s)?;
    let a = (lo + hi).div_euclid(2);
    move_to_ct(s, a, 0, SETTLE)?;
    let st = s.cmd(COLOR, 0x4B, &mct_payload(0x01, 0, 0, 0))?;
    s.run(1000.0)?;
    let v = ct(s)?;
    expect(
        status(&st) == Some(0x85),
        format!("Default Response {}; INVALID_FIELD is 0x85", dr_text(&st, "replaced by another response")),
    )?;
    expect(v == a, format!("ColorTemperatureMireds moved from {a} to {v}"))?;
    Ok(String::new())
}

fn not_executed_when_off(s: &mut Session) -> R<String> {
    let (lo, hi) = need_ct(s)?;
    light_on(s)?;
    let orig = set_options(s, 0x00)?;
    finally(
        s,
        |s| {
            move_to_ct(s, lo + 30, 0, SETTLE)?;
            act(s, ONOFF, 0x00, &[], "Off", 1500.0)?;
            s.cmd(COLOR, 0x0A, &mtc_payload(hi - 30, 0, 0, 0))?;
            s.run(700.0)?;
            let v1 = ct(s)?;
            s.cmd(COLOR, 0x4C, &sct_payload(0x01, 40, 0, 0, 0))?;
            s.run(700.0)?;
            let v2 = ct(s)?;
            expect(
                (v1, v2) == (lo + 30, lo + 30),
                format!(
                    "ColorTemperatureMireds {} when switched off; {v1} after Move to Color Temperature {}, {v2} after \
                     Step up 40",
                    lo + 30,
                    hi - 30
                ),
            )?;
            Ok(String::new())
        },
        |s| {
            s.cmd(ONOFF, 0x01, &[])?;
            s.run(500.0)?;
            if orig != 0 {
                s.write(COLOR, &[(OPTIONS, 0x18, vec![orig as u8])])?;
            }
            Ok(())
        },
    )
}

fn options_execute_if_off(s: &mut Session) -> R<String> {
    let (lo, _hi) = need_ct(s)?;
    light_on(s)?;
    let orig = set_options(s, 0x00)?;
    finally(
        s,
        |s| {
            move_to_ct(s, lo + 30, 0, SETTLE)?;
            act(s, ONOFF, 0x00, &[], "Off", 1500.0)?;
            s.cmd(COLOR, 0x0A, &mtc_payload(lo + 80, 0, 0x01, 0x01))?;
            s.run(700.0)?;
            let v1 = ct(s)?;
            expect(
                v1 == lo + 80,
                format!("ColorTemperatureMireds {v1} after Move to {} with ExecuteIfOff overridden to 1 while off", lo + 80),
            )?;
            let st = write_attr(s, COLOR, OPTIONS, 0x18, vec![0x01])?;
            expect(st == 0, format!("writing Options = 0x01: status {st:#04x}"))?;
            s.cmd(COLOR, 0x0A, &mtc_payload(lo + 120, 0, 0, 0))?;
            s.run(700.0)?;
            let v2 = ct(s)?;
            expect(
                v2 == lo + 120,
                format!("ColorTemperatureMireds {v2} after Move to {} while off with Options.ExecuteIfOff = 1", lo + 120),
            )?;
            s.cmd(COLOR, 0x0A, &mtc_payload(lo + 160, 0, 0x01, 0x00))?;
            s.run(700.0)?;
            let v3 = ct(s)?;
            let o = attr(s, ONOFF, 0x0000)?;
            expect(
                v3 == lo + 120,
                format!(
                    "ColorTemperatureMireds {v3} after Move to {} whose OptionsMask/Override clear ExecuteIfOff",
                    lo + 160
                ),
            )?;
            let pwm = s.light().on;
            expect(o == 0 && !pwm, format!("a colour command switched the light on (OnOff {o})"))?;
            Ok(String::new())
        },
        |s| {
            s.write(COLOR, &[(OPTIONS, 0x18, vec![orig as u8])])?;
            s.cmd(ONOFF, 0x01, &[])?;
            s.run(500.0)?;
            Ok(())
        },
    )
}

fn set_level(s: &mut Session, v: u8) -> R<()> {
    act(s, LEVEL, 0x04, &[v, 0, 0, 0, 0], &format!("Move to Level (with On/Off) {v}"), 1000.0)
}

fn couple_to_level(s: &mut Session) -> R<String> {
    let (lo, hi) = need_ct(s)?;
    let cmin = attr(s, COLOR, CMIN)?;
    let lopt = attr(s, LEVEL, 0x000F)?;
    light_on(s)?;
    finally(
        s,
        |s| {
            if lopt & 0x02 != 0 && write_attr(s, LEVEL, 0x000F, 0x18, vec![(lopt & !0x02) as u8])? != 0 {
                return Err(Stop::Inconclusive("CoupleColorTempToLevel could not be cleared in the Level Control Options".into()));
            }
            let mid = (lo + hi).div_euclid(2);
            move_to_ct(s, mid, 0, SETTLE)?;
            set_level(s, 40)?;
            let v_a = ct(s)?;
            set_level(s, 254)?;
            let v_b = ct(s)?;
            expect(
                (v_a, v_b) == (mid, mid),
                format!(
                    "with CoupleColorTempToLevel = 0, ColorTemperatureMireds {mid} became {v_a} at level 40 and {v_b} \
                     at level 254"
                ),
            )?;
            let st = write_attr(s, LEVEL, 0x000F, 0x18, vec![((lopt & !0x02) | 0x02) as u8])?;
            if st != 0 || attr(s, LEVEL, 0x000F)? & 0x02 == 0 {
                return Err(Stop::Inconclusive(format!(
                    "CoupleColorTempToLevel could not be set in the Level Control Options (status {st:#04x})"
                )));
            }
            move_to_ct(s, mid, 0, SETTLE)?;
            set_level(s, 40)?;
            let v_low = ct(s)?;
            set_level(s, 254)?;
            let v_max = ct(s)?;
            expect(
                v_max == cmin,
                format!(
                    "ColorTemperatureMireds {v_max} at the maximum level with coupling on; CoupleColorTempToLevelMinMireds \
                     is {cmin}"
                ),
            )?;
            expect(
                v_low > v_max,
                format!("ColorTemperatureMireds {v_low} at level 40 is not above the value at the maximum level ({v_max})"),
            )?;
            Ok(format!("coupled: level 40 -> {v_low} mireds, level 254 -> {v_max} mireds"))
        },
        |s| {
            s.write(LEVEL, &[(0x000F, 0x18, vec![lopt as u8])])?;
            set_level(s, 254)
        },
    )
}

fn unsupported_command(s: &mut Session) -> R<String> {
    need_ct(s)?;
    let caps = attr(s, COLOR, CAPS)?;
    if caps & 0x01 != 0 {
        return Err(Stop::NotApplicable(
            "the device supports hue/saturation; no unsupported Color Control command to send".into(),
        ));
    }
    light_on(s)?;
    let st = s.cmd(COLOR, 0x00, &[100, 0, 0, 0, 0, 0])?;
    expect(
        status(&st) == Some(0x81),
        format!(
            "Move to Hue on a light without hue/saturation: Default Response {}; UNSUP_COMMAND is 0x81 in ZCL8 \
             (UNSUP_CLUSTER_COMMAND, the same value, before)",
            dr_text(&st, "replaced by a response")
        ),
    )?;
    Ok(String::new())
}

fn start_up_color_temperature(s: &mut Session) -> R<String> {
    let (lo, hi) = need_ct(s)?;
    let orig = attr(s, COLOR, STARTUP)?;
    let mut obs: Vec<String> = Vec::new();
    let mut case = |s: &mut Session, value: u16, before: i64, want: i64, label: String| -> R<()> {
        let st = write_attr(s, COLOR, STARTUP, 0x21, value.to_le_bytes().to_vec())?;
        expect(st == 0, format!("writing StartUpColorTemperatureMireds = {value:#06x}: status {st:#04x}"))?;
        light_on(s)?;
        move_to_ct(s, before, 0, SETTLE)?;
        s.run(2500.0)?;
        s.reboot()?;
        s.run(2500.0)?;
        if !s.alive()? {
            return Err(Stop::Inconclusive("the bulb did not answer after the power cycle".into()));
        }
        let v = ct(s)?;
        let mode = attr(s, COLOR, MODE)?;
        let emode = attr(s, COLOR, EMODE)?;
        obs.push(format!("{label}: {v}"));
        expect(
            v == want,
            format!(
                "StartUpColorTemperatureMireds {value:#06x}, {before} mireds before the power cycle: \
                 ColorTemperatureMireds {v} after it, {want} required"
            ),
        )?;
        expect(mode == 2 && emode == 2, format!("ColorMode {mode}, EnhancedColorMode {emode} after the power cycle"))
    };
    finally(
        s,
        |s| {
            case(s, (lo + 40) as u16, hi - 40, lo + 40, format!("{}", lo + 40))?;
            case(s, 0xFFFF, hi - 70, hi - 70, format!("0xffff from {}", hi - 70))
        },
        |s| {
            s.write(COLOR, &[(STARTUP, 0x21, (orig as u16).to_le_bytes().to_vec())])?;
            s.cmd(ONOFF, 0x01, &[])?;
            Ok(())
        },
    )?;
    Ok(obs.join("; "))
}
