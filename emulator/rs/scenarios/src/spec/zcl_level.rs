//! Level Control cluster (ZCL 8, 3.10) and Level Control for Lighting (3.19)
//! as a ZLO dimmable light must implement them (ZLO 7.2), ported from
//! emulator/spec/zcl_level.py.
//!
//! Transitions are checked on the PWM output while they run: the duty cycle is
//! sampled between steps of simulated time. The mapping of level to duty is the
//! device's own (the text leaves it open), so the tests only use that it is
//! monotonic: reference duties are measured on the same image with a
//! transition time of 0.

use std::collections::HashSet;

use super::zcl_onoff::{act, attr, check_types, finally, oo, opt_attr, write_attr};
use super::Test;
use crate::frames::{ext_bytes, py_value};
use crate::session::{expect, CmdAnswer, Session, Stop, R};
use crate::station::Tx;

const S: &str = "zcl-level";
const ONOFF: u16 = 0x0006;
const LEVEL: u16 = 0x0008;
const SETTLE: f64 = 1500.0;
const MIN_AMBIGUOUS: &str = "for Move to Level (with On/Off) \"the minimum level allowed by the device\" is 1 for \
    lighting (3.19.2.2.1) and the MinLevel attribute where implemented, but the base cluster's MinLevel defaults to 0 \
    when the attribute is absent (Table 3-56); the test uses MinLevel if the image has it, else 1. The Move and Step \
    variants do not depend on this: they run down to whatever the device's minimum is";

fn t(id: &'static str, doc: &'static str, clause: &'static str, req: &'static str, run: fn(&mut Session) -> R<String>) -> Test {
    Test { id, doc, clause, req, session: S, run, ..Test::D }
}

pub fn tests() -> Vec<Test> {
    vec![
        t("ZCL-LEVEL-01", "ZLO", "7.2.1 Table 5; ZCL8 3.19.2.2 Table 3-143",
          "A dimmable light implements CurrentLevel (uint8), RemainingTime (uint16), Options (map8) and \
           StartUpCurrentLevel (uint8).", attributes),
        t("ZCL-LEVEL-02", "ZCL8", "3.10.2.3.1.2",
          "Move to Level moves to the level continuously (not as a step) and takes the transition time given; \
           CurrentLevel ends at the level.", move_to_level_transition),
        Test {
            mandatory: false,
            ..t("ZCL-LEVEL-03", "ZCL8", "3.10.2.3.1.2",
                "Move to Level with transition time 0xffff takes OnOffTransitionTime (if that optional attribute is \
                 implemented; otherwise the device moves as fast as it can) and reaches the level.",
                move_to_level_default_time)
        },
        t("ZCL-LEVEL-04", "ZCL8", "3.10.2.3.2.4 Table 3-61",
          "Move changes the level at the given rate and stops at the maximum (up) or minimum (down) level; it does \
           not affect OnOff.", move_cmd),
        t("ZCL-LEVEL-05", "ZCL8", "3.10.2.3.4.2",
          "Stop (0x03 and 0x07) terminates a Move to Level in progress: CurrentLevel stays where it was and \
           RemainingTime becomes 0.", stop_cmd),
        t("ZCL-LEVEL-06", "ZCL8", "3.10.2.3.3.2 Table 3-63",
          "Step changes CurrentLevel by the step size, stopping at the maximum or minimum level; it does not affect \
           OnOff.", step_cmd),
        t("ZCL-LEVEL-07", "ZCL8", "3.10.2.3.6",
          "Move to Level (with On/Off) and Move (with On/Off) to a level above the minimum set OnOff to On before \
           starting: a light that is off comes on.", with_onoff_turns_on),
        Test {
            ambiguous: Some(MIN_AMBIGUOUS),
            ..t("ZCL-LEVEL-08", "ZCL8", "3.10.2.3.6, 3.19.2.2.1",
                "A 'with On/Off' command that takes CurrentLevel to the minimum level allowed by the device sets \
                 OnOff to Off.", with_onoff_turns_off)
        },
        t("ZCL-LEVEL-09", "ZCL8", "3.10.2.1.2",
          "Move to Level (without On/Off) to the minimum level leaves OnOff on.", without_onoff_keeps_on),
        t("ZCL-LEVEL-10", "ZCL8", "3.10.2.2.8.1",
          "With ExecuteIfOff = 0, Move to Level, Move and Step (without On/Off) are not executed while OnOff is off.",
          not_executed_when_off),
        t("ZCL-LEVEL-11", "ZCL8", "3.10.2.3.1.2, 3.10.2.1.2",
          "OptionsMask/OptionsOverride with ExecuteIfOff = 1 make Move to Level execute while off: CurrentLevel \
           changes, OnOff stays off.", options_override),
        t("ZCL-LEVEL-12", "ZCL8", "3.10.2.2.8, 3.10.2.3.1.2",
          "With the Options attribute's ExecuteIfOff = 1 commands execute while off; an OptionsMask/OptionsOverride \
           that clears the bit for one command prevents that command.", options_attribute),
        t("ZCL-LEVEL-13", "ZCL8", "3.10.2.1.1 Table 3-55",
          "With OnLevel undefined, Off and then On restore the CurrentLevel that was in effect before: the attribute \
           holds it after Off and after On.", off_on_restores_level),
        Test {
            ambiguous: Some(
                "the text restores the CurrentLevel attribute; that the light output for the same CurrentLevel is \
                 the same before Off and after On follows from 'level' being the brightness of a light, but is not \
                 stated",
            ),
            ..t("ZCL-LEVEL-19", "ZCL8", "3.10.2.1.1 Table 3-55",
                "After Off and On the light output is again the output of the restored CurrentLevel.",
                off_on_restores_output)
        },
        Test {
            mandatory: false,
            ..t("ZCL-LEVEL-14", "ZCL8", "3.10.2.2.10, 3.10.2.1.1",
                "With OnLevel defined, On sets CurrentLevel to OnLevel.", on_level)
        },
        t("ZCL-LEVEL-15", "ZCL8", "3.10.2.2.2",
          "RemainingTime is the time left, in 1/10 s, until the command in progress completes, and 0 afterwards.",
          remaining_time),
        t("ZCL-LEVEL-16", "ZCL8", "3.19.2.2.1",
          "For lighting CurrentLevel stays within 1..0xfe: 0 is never used and 0xff is the undefined value, whatever \
           level a command asks for.", level_range),
        t("ZCL-LEVEL-17", "ZCL8", "2.5.7, 2.5.11; ZLO 7.2.1 Table 5",
          "CurrentLevel is reportable: after Configure Reporting and a binding, a change of the level is reported \
           with the new value.", level_reporting),
        t("ZCL-LEVEL-18", "ZCL8", "3.10.2.2.14 Table 3-58",
          "StartUpCurrentLevel decides CurrentLevel after power is applied: a level value is used as is, 0xff \
           restores the previous level, 0x00 selects the minimum level.", start_up_current_level),
    ]
}

// ---- local helpers (as in zcl_level.py)
fn level(s: &mut Session) -> R<i64> {
    attr(s, LEVEL, 0x0000)
}

fn onoff(s: &mut Session) -> R<i64> {
    attr(s, ONOFF, 0x0000)
}

fn min_level(s: &mut Session) -> R<i64> {
    opt_attr(s, LEVEL, 0x0002, 1)
}

fn max_level(s: &mut Session) -> R<i64> {
    opt_attr(s, LEVEL, 0x0003, 0xFE)
}

fn wo(with_onoff: bool) -> &'static str {
    if with_onoff {
        " (with On/Off)"
    } else {
        ""
    }
}

fn mtl_payload(lvl: u8, tt: u16, mask: u8, ovr: u8) -> Vec<u8> {
    let mut p = vec![lvl];
    p.extend_from_slice(&tt.to_le_bytes());
    p.extend_from_slice(&[mask, ovr]);
    p
}

fn move_to(s: &mut Session, lvl: u8, tt: u16, with_onoff: bool, settle: f64) -> R<()> {
    act(
        s,
        LEVEL,
        if with_onoff { 0x04 } else { 0x00 },
        &mtl_payload(lvl, tt, 0, 0),
        &format!("Move to Level{} {lvl} in {:.1} s", wo(with_onoff), tt as f64 / 10.0),
        settle,
    )
}

fn move_(s: &mut Session, up: bool, rate: u8, with_onoff: bool) -> R<()> {
    act(
        s,
        LEVEL,
        if with_onoff { 0x05 } else { 0x01 },
        &[if up { 0 } else { 1 }, rate, 0, 0],
        &format!("Move{} {} at {rate}/s", wo(with_onoff), if up { "up" } else { "down" }),
        0.0,
    )
}

fn step(s: &mut Session, up: bool, size: u8, tt: u16, with_onoff: bool) -> R<()> {
    let mut p = vec![if up { 0 } else { 1 }, size];
    p.extend_from_slice(&tt.to_le_bytes());
    p.extend_from_slice(&[0, 0]);
    act(
        s,
        LEVEL,
        if with_onoff { 0x06 } else { 0x02 },
        &p,
        &format!("Step{} {} {size}", wo(with_onoff), if up { "up" } else { "down" }),
        tt as f64 * 100.0 + 700.0,
    )
}

fn stop(s: &mut Session, cmd: u8) -> R<()> {
    act(s, LEVEL, cmd, &[0, 0], if cmd == 0x03 { "Stop" } else { "Stop (0x07)" }, 0.0)
}

fn on(s: &mut Session) -> R<()> {
    act(s, ONOFF, 0x01, &[], "On", SETTLE)
}

fn off(s: &mut Session) -> R<()> {
    act(s, ONOFF, 0x00, &[], "Off", SETTLE)
}

/// Light on at a level, no transition; returns the settled duty.
fn start(s: &mut Session, lvl: u8) -> R<f64> {
    act(s, ONOFF, 0x01, &[], "On", 200.0)?;
    move_to(s, lvl, 0, true, 1200.0)?;
    Ok(s.duty())
}

fn set_options(s: &mut Session, value: u8) -> R<i64> {
    let cur = attr(s, LEVEL, 0x000F)?;
    if cur != value as i64 {
        let st = write_attr(s, LEVEL, 0x000F, 0x18, vec![value])?;
        if st != 0 {
            return Err(Stop::Inconclusive(format!(
                "the Options attribute could not be written ({value:#04x}: status {st:#04x})"
            )));
        }
    }
    Ok(cur)
}

fn stable(s: &mut Session, ms: f64) -> R<(bool, f64, f64)> {
    let a = s.duty();
    s.run(ms)?;
    let b = s.duty();
    Ok(((a - b).abs() < 0.05, a, b))
}

// ---- tests
fn attributes(s: &mut Session) -> R<String> {
    let Some(r) = s.read(LEVEL, &[0x0000, 0x0001, 0x000F, 0x4000], 1500.0)? else {
        return Err(Stop::Fail("no Read Attributes Response".into()));
    };
    let bad = check_types(&r, &[(0x0000, 0x20), (0x0001, 0x21), (0x000F, 0x18), (0x4000, 0x20)]);
    expect(bad.is_empty(), bad.join("; "))?;
    Ok(String::new())
}

fn samples_txt(samples: &[(f64, f64)]) -> String {
    samples.iter().map(|(t, d)| format!("{t:.1}s {d:.1}%")).collect::<Vec<_>>().join(", ")
}

fn move_to_level_transition(s: &mut Session) -> R<String> {
    let d_lo = start(s, 40)?;
    let d_hi = start(s, 254)?;
    if !(d_hi > d_lo + 5.0) {
        return Err(Stop::Inconclusive(format!(
            "duty at level 254 ({d_hi:.1}%) is not above duty at level 40 ({d_lo:.1}%)"
        )));
    }
    move_to(s, 40, 30, false, 0.0)?;
    let t0 = s.now();
    let mut samples = Vec::new();
    while s.now() - t0 < 3900.0 {
        s.run(300.0)?;
        samples.push(((s.now() - t0) / 1000.0, s.duty()));
    }
    let txt = samples_txt(&samples);
    let eps = (d_hi - d_lo) * 0.01;
    let mid_ok = samples.iter().filter(|(t, _)| (1.0..=2.0).contains(t)).all(|(_, d)| d_lo + eps < *d && *d < d_hi - eps);
    expect(
        mid_ok,
        format!(
            "between 1.0 and 2.0 s of a 3.0 s transition the duty is not strictly between {d_hi:.1}% and {d_lo:.1}%: \
             {txt}"
        ),
    )?;
    expect(samples.windows(2).all(|w| w[1].1 <= w[0].1 + 0.05), format!("the duty does not fall monotonically: {txt}"))?;
    let distinct: HashSet<String> = samples.iter().map(|(_, d)| format!("{d:.1}")).collect();
    expect(distinct.len() >= 5, format!("fewer than 5 distinct duty values during the transition: {txt}"))?;
    let end_ok = samples.iter().filter(|(t, _)| *t >= 3.6).all(|(_, d)| (d - d_lo).abs() < 0.3);
    expect(
        end_ok,
        format!("the duty is not at the level-40 value {d_lo:.1}% 3.6 s after the start of a 3.0 s transition: {txt}"),
    )?;
    let lv = level(s)?;
    expect(lv == 40, format!("CurrentLevel {lv} after Move to Level 40"))?;
    Ok(txt)
}

fn move_to_level_default_time(s: &mut Session) -> R<String> {
    let (st, _, oott) = s.read1(LEVEL, 0x0010)?;
    let d_lo = start(s, 40)?;
    let d_hi = start(s, 254)?;
    let has = st == Some(0);
    if has && write_attr(s, LEVEL, 0x0010, 0x21, 20u16.to_le_bytes().to_vec())? != 0 {
        return Err(Stop::Inconclusive("OnOffTransitionTime could not be written".into()));
    }
    finally(
        s,
        |s| {
            move_to(s, 40, 0xFFFF, false, 1000.0)?;
            let mid = s.duty();
            s.run(2000.0)?;
            let d = s.duty();
            let lv = level(s)?;
            expect(
                lv == 40 && (d - d_lo).abs() < 0.3,
                format!("3 s after the command: CurrentLevel {lv}, duty {d:.1}% (level 40 is {d_lo:.1}%)"),
            )?;
            if has {
                let eps = (d_hi - d_lo) * 0.01;
                expect(
                    d_lo + eps < mid && mid < d_hi - eps,
                    format!(
                        "1.0 s into a transition that OnOffTransitionTime sets to 2.0 s the duty is {mid:.1}% (start \
                         {d_hi:.1}%, end {d_lo:.1}%)"
                    ),
                )?;
                return Ok("OnOffTransitionTime implemented and used".into());
            }
            Ok("OnOffTransitionTime not implemented; the level was reached".into())
        },
        |s| {
            if has {
                let v = oott.and_then(|v| v.int()).unwrap_or(0) as u16;
                s.write(LEVEL, &[(0x0010, 0x21, v.to_le_bytes().to_vec())])?;
            }
            Ok(())
        },
    )
}

fn move_cmd(s: &mut Session) -> R<String> {
    start(s, 10)?;
    move_(s, true, 50, false)?;
    let t0 = s.now();
    s.run(2000.0)?;
    stop(s, 0x03)?;
    let dt = (s.now() - t0) / 1000.0;
    let lv = level(s)?;
    let want = 10.0 + 50.0 * dt;
    expect(
        (lv as f64 - want).abs() <= 0.25 * 50.0 * dt + 3.0,
        format!("CurrentLevel {lv} after moving up from 10 at 50/s for {dt:.2} s (about {want:.0} expected)"),
    )?;
    let (ok, a, b) = stable(s, 500.0)?;
    expect(ok, format!("the PWM duty still changes after Stop ({a:.2}% -> {b:.2}%)"))?;
    let obs = format!("up 50/s for {dt:.2} s: level {lv}");
    let mx = max_level(s)?;
    let mn = min_level(s)?;
    move_(s, true, 120, false)?;
    s.run(2500.0)?;
    let lv = level(s)?;
    expect(lv == mx, format!("CurrentLevel {lv} 2.5 s after Move up at 120/s; it must stop at the maximum {mx}"))?;
    move_(s, false, 200, false)?;
    s.run(2200.0)?;
    let lv = level(s)?;
    let o = onoff(s)?;
    expect(
        lv == mn,
        format!("CurrentLevel {lv} 2.2 s after Move down at 200/s from {mx}; it must stop at the minimum {mn}"),
    )?;
    let pwm = s.light().on;
    expect(
        o == 1 && pwm,
        format!("Move (without On/Off) down to the minimum changed OnOff to {o} / PWM {}", oo(pwm)),
    )?;
    Ok(obs)
}

fn stop_cmd(s: &mut Session) -> R<String> {
    let mut obs = Vec::new();
    for cmd in [0x03u8, 0x07] {
        start(s, 254)?;
        move_to(s, 20, 40, false, 2000.0)?;
        stop(s, cmd)?;
        let (ok, a, b) = stable(s, 800.0)?;
        let lv = level(s)?;
        obs.push(format!("Stop {cmd:#04x}: level {lv}"));
        expect(ok, format!("Stop {cmd:#04x}: the PWM duty still changes after Stop ({a:.2}% -> {b:.2}%)"))?;
        expect(
            20 < lv && lv < 254,
            format!("Stop {cmd:#04x} 2.0 s into a 4.0 s move from 254 to 20: CurrentLevel {lv}"),
        )?;
        let (st, _, rt) = s.read1(LEVEL, 0x0001)?;
        if st == Some(0) {
            expect(
                rt.as_ref().and_then(|v| v.int()) == Some(0),
                format!("Stop {cmd:#04x}: RemainingTime {} after Stop", py_value(&rt)),
            )?;
        }
        s.run(2500.0)?;
        let lv2 = level(s)?;
        expect(lv2 == lv, format!("Stop {cmd:#04x}: CurrentLevel went on from {lv} to {lv2} after Stop"))?;
    }
    Ok(obs.join("; "))
}

fn step_cmd(s: &mut Session) -> R<String> {
    start(s, 100)?;
    step(s, true, 40, 10, false)?;
    let lv = level(s)?;
    expect(lv == 140, format!("CurrentLevel {lv} after Step up 40 from 100"))?;
    step(s, false, 15, 5, false)?;
    let lv = level(s)?;
    expect(lv == 125, format!("CurrentLevel {lv} after Step down 15 from 140"))?;
    let mx = max_level(s)?;
    let mn = min_level(s)?;
    step(s, true, 200, 10, false)?;
    let lv = level(s)?;
    expect(lv == mx, format!("CurrentLevel {lv} after Step up 200 from 125; the maximum is {mx}"))?;
    step(s, false, 253, 10, false)?;
    let lv = level(s)?;
    let o = onoff(s)?;
    expect(lv == mn, format!("CurrentLevel {lv} after Step down 253 from {mx}; the minimum is {mn}"))?;
    let pwm = s.light().on;
    expect(o == 1 && pwm, format!("Step (without On/Off) down to the minimum changed OnOff to {o}"))?;
    Ok(String::new())
}

fn with_onoff_turns_on(s: &mut Session) -> R<String> {
    start(s, 254)?;
    off(s)?;
    move_to(s, 128, 0, true, SETTLE)?;
    let o = onoff(s)?;
    let lv = level(s)?;
    let pwm = s.light().on;
    expect(o == 1 && pwm, format!("after Move to Level (with On/Off) 128 from off: OnOff {o}, PWM {}", oo(pwm)))?;
    expect(lv == 128, format!("CurrentLevel {lv} after Move to Level (with On/Off) 128"))?;
    off(s)?;
    move_(s, true, 100, true)?;
    s.run(1500.0)?;
    stop(s, 0x03)?;
    let o = onoff(s)?;
    let pwm = s.light().on;
    expect(o == 1 && pwm, format!("after Move (with On/Off) up from off: OnOff {o}, PWM {}", oo(pwm)))?;
    off(s)?;
    step(s, true, 60, 5, true)?;
    let o = onoff(s)?;
    let pwm = s.light().on;
    expect(o == 1 && pwm, format!("after Step (with On/Off) up from off: OnOff {o}, PWM {}", oo(pwm)))?;
    Ok(String::new())
}

fn with_onoff_turns_off(s: &mut Session) -> R<String> {
    let (st, _, mn_attr) = s.read1(LEVEL, 0x0002)?;
    let has = st == Some(0);
    let mn = if has { mn_attr.and_then(|v| v.int()).unwrap_or(1) } else { 1 };
    let src = if has {
        format!("MinLevel attribute = {mn}")
    } else {
        "MinLevel not implemented, minimum for lighting = 1".to_string()
    };
    let mut bad = Vec::new();
    start(s, 100)?;
    move_to(s, mn as u8, 0, true, SETTLE)?;
    let (lv, o, pwm) = (level(s)?, onoff(s)?, s.light().on);
    if o != 0 || pwm {
        bad.push(format!("Move to Level (with On/Off) {mn}: CurrentLevel {lv}, OnOff {o}, PWM {}", oo(pwm)));
    }
    start(s, 100)?;
    move_(s, false, 200, true)?;
    s.run(1800.0)?;
    let (lv, o, pwm) = (level(s)?, onoff(s)?, s.light().on);
    if o != 0 || pwm {
        bad.push(format!("Move (with On/Off) down to the end: CurrentLevel {lv}, OnOff {o}, PWM {}", oo(pwm)));
    }
    start(s, 100)?;
    step(s, false, 200, 5, true)?;
    let (lv, o, pwm) = (level(s)?, onoff(s)?, s.light().on);
    if o != 0 || pwm {
        bad.push(format!("Step (with On/Off) down 200 from 100: CurrentLevel {lv}, OnOff {o}, PWM {}", oo(pwm)));
    }
    start(s, 254)?;
    expect(bad.is_empty(), format!("{src}; OnOff not set to Off: {}", bad.join("; ")))?;
    Ok(src)
}

fn without_onoff_keeps_on(s: &mut Session) -> R<String> {
    start(s, 100)?;
    let mn = min_level(s)?;
    move_to(s, mn as u8, 0, false, SETTLE)?;
    let lv = level(s)?;
    let o = onoff(s)?;
    start(s, 254)?;
    expect(lv == mn, format!("CurrentLevel {lv} after Move to Level {mn}"))?;
    expect(o == 1, format!("OnOff {o} after Move to Level (without On/Off) to the minimum level"))?;
    Ok(String::new())
}

fn not_executed_when_off(s: &mut Session) -> R<String> {
    start(s, 200)?;
    let orig = set_options(s, 0x00)?;
    finally(
        s,
        |s| {
            off(s)?;
            let l0 = level(s)?;
            s.cmd(LEVEL, 0x00, &mtl_payload(77, 0, 0, 0))?;
            s.run(600.0)?;
            let l1 = level(s)?;
            s.cmd(LEVEL, 0x02, &[1, 30, 0, 0, 0, 0])?;
            s.run(600.0)?;
            let l2 = level(s)?;
            s.cmd(LEVEL, 0x01, &[1, 100, 0, 0])?;
            s.run(800.0)?;
            let (l3, o, pwm) = (level(s)?, onoff(s)?, s.light().on);
            s.cmd(LEVEL, 0x03, &[0, 0])?;
            expect(
                (l1, l2, l3) == (l0, l0, l0),
                format!(
                    "CurrentLevel {l0} when switched off; after Move to Level 77: {l1}, after Step down 30: {l2}, \
                     after Move down: {l3}"
                ),
            )?;
            expect(o == 0 && !pwm, format!("OnOff {o}, PWM {} after the commands", oo(pwm)))?;
            Ok(String::new())
        },
        |s| {
            on(s)?;
            if orig != 0 {
                s.write(LEVEL, &[(0x000F, 0x18, vec![orig as u8])])?;
            }
            Ok(())
        },
    )
}

fn options_override(s: &mut Session) -> R<String> {
    start(s, 200)?;
    let orig = set_options(s, 0x00)?;
    finally(
        s,
        |s| {
            off(s)?;
            s.cmd(LEVEL, 0x00, &mtl_payload(77, 0, 0x01, 0x01))?;
            s.run(700.0)?;
            let (lv, o, pwm) = (level(s)?, onoff(s)?, s.light().on);
            expect(lv == 77, format!("CurrentLevel {lv} after Move to Level 77 with ExecuteIfOff overridden to 1 while off"))?;
            expect(
                o == 0 && !pwm,
                format!("Move to Level (without On/Off) changed OnOff to {o} / PWM {}", oo(pwm)),
            )?;
            Ok(String::new())
        },
        |s| {
            on(s)?;
            if orig != 0 {
                s.write(LEVEL, &[(0x000F, 0x18, vec![orig as u8])])?;
            }
            Ok(())
        },
    )
}

fn options_attribute(s: &mut Session) -> R<String> {
    start(s, 200)?;
    let orig = set_options(s, 0x01)?;
    finally(
        s,
        |s| {
            off(s)?;
            s.cmd(LEVEL, 0x00, &mtl_payload(90, 0, 0, 0))?;
            s.run(700.0)?;
            let lv = level(s)?;
            expect(lv == 90, format!("CurrentLevel {lv} after Move to Level 90 while off with Options.ExecuteIfOff = 1"))?;
            s.cmd(LEVEL, 0x00, &mtl_payload(120, 0, 0x01, 0x00))?;
            s.run(700.0)?;
            let lv = level(s)?;
            let o = onoff(s)?;
            expect(
                lv == 90,
                format!("CurrentLevel {lv} after Move to Level 120 whose OptionsMask/Override clear ExecuteIfOff"),
            )?;
            expect(o == 0, format!("OnOff {o} after the commands"))?;
            Ok(String::new())
        },
        |s| {
            s.write(LEVEL, &[(0x000F, 0x18, vec![orig as u8])])?;
            on(s)
        },
    )
}

/// Level 123, Off, On: (duty before, CurrentLevel after Off, CurrentLevel after On, duty after On).
fn off_on(s: &mut Session) -> R<(f64, i64, i64, f64)> {
    let (st, _, onlevel) = s.read1(LEVEL, 0x0011)?;
    if st == Some(0)
        && onlevel.as_ref().and_then(|v| v.int()) != Some(0xFF)
        && write_attr(s, LEVEL, 0x0011, 0x20, vec![0xFF])? != 0
    {
        return Err(Stop::Inconclusive("OnLevel is defined and could not be set to 0xff".into()));
    }
    let d = start(s, 123)?;
    off(s)?;
    s.run(1000.0)?;
    let lv_off = level(s)?;
    on(s)?;
    let mut d2 = 0.0;
    for _ in 0..8 {
        // no attribute tells how long the fade is: wait until the output is steady
        let (ok, _, b) = stable(s, 500.0)?;
        d2 = b;
        if ok {
            break;
        }
    }
    let lv_on = level(s)?;
    start(s, 254)?;
    Ok((d, lv_off, lv_on, d2))
}

fn off_on_restores_level(s: &mut Session) -> R<String> {
    let (_, lv_off, lv_on, _) = off_on(s)?;
    expect(lv_off == 123, format!("CurrentLevel {lv_off} after Off completed; 123 was in effect before"))?;
    expect(lv_on == 123, format!("CurrentLevel {lv_on} after On; 123 was in effect before Off"))?;
    Ok(String::new())
}

fn off_on_restores_output(s: &mut Session) -> R<String> {
    let (d, _, lv_on, d2) = off_on(s)?;
    if lv_on != 123 {
        return Err(Stop::Inconclusive(format!("CurrentLevel is {lv_on} after On, not the 123 in effect before Off")));
    }
    expect(
        (d2 - d).abs() < 0.3,
        format!("CurrentLevel is 123 before Off and after On, but the PWM duty is {d:.1}% before and {d2:.1}% after"),
    )?;
    Ok(String::new())
}

fn on_level(s: &mut Session) -> R<String> {
    let orig = attr(s, LEVEL, 0x0011)?;
    let st = write_attr(s, LEVEL, 0x0011, 0x20, vec![80])?;
    expect(st == 0, format!("writing OnLevel = 80: status {st:#04x}"))?;
    finally(
        s,
        |s| {
            let d80 = start(s, 80)?;
            start(s, 200)?;
            off(s)?;
            on(s)?;
            s.run(500.0)?;
            let lv = level(s)?;
            let d = s.duty();
            expect(
                lv == 80 && (d - d80).abs() < 0.3,
                format!("after On with OnLevel 80: CurrentLevel {lv}, duty {d:.1}% (level 80 is {d80:.1}%)"),
            )?;
            Ok(String::new())
        },
        |s| {
            s.write(LEVEL, &[(0x0011, 0x20, vec![orig as u8])])?;
            start(s, 254)?;
            Ok(())
        },
    )
}

fn remaining_time(s: &mut Session) -> R<String> {
    attr(s, LEVEL, 0x0001)?;
    start(s, 254)?;
    move_to(s, 30, 50, false, 0.0)?;
    let t0 = s.now();
    let mut obs = Vec::new();
    for wait in [1000.0, 1500.0] {
        s.run(wait)?;
        let rt = attr(s, LEVEL, 0x0001)?;
        let el = (s.now() - t0) / 1000.0;
        obs.push(format!("{rt} at {el:.2} s"));
        expect(
            (rt as f64 - (50.0 - el * 10.0)).abs() <= 4.0,
            format!("RemainingTime {rt} at {el:.2} s of a 5.0 s transition ({:.0} expected)", 50.0 - el * 10.0),
        )?;
    }
    let w = (t0 + 5600.0 - s.now()).max(0.0);
    s.run(w)?;
    let rt = attr(s, LEVEL, 0x0001)?;
    start(s, 254)?;
    expect(rt == 0, format!("RemainingTime {rt} 5.6 s after the start of a 5.0 s transition"))?;
    Ok(format!("RemainingTime {}", obs.join(", ")))
}

fn level_range(s: &mut Session) -> R<String> {
    start(s, 100)?;
    let st0 = s.cmd(LEVEL, 0x00, &mtl_payload(0, 0, 0, 0))?;
    s.run(800.0)?;
    let lv0 = level(s)?;
    let st1 = s.cmd(LEVEL, 0x00, &mtl_payload(255, 0, 0, 0))?;
    s.run(800.0)?;
    let lv1 = level(s)?;
    start(s, 254)?;
    let fmt = |a: &CmdAnswer| match a {
        CmdAnswer::None => "none".to_string(),
        CmdAnswer::Status(st) => format!("{st:#04x}"),
        CmdAnswer::Frame(_) => "response".to_string(),
    };
    expect(
        (1..=0xFE).contains(&lv0),
        format!("CurrentLevel {lv0} after Move to Level 0 (default response {})", fmt(&st0)),
    )?;
    expect(
        (1..=0xFE).contains(&lv1),
        format!(
            "CurrentLevel {lv1} after Move to Level 255 (default response {}); 0xff is the undefined value in Level \
             Control for Lighting",
            fmt(&st1)
        ),
    )?;
    Ok(format!(
        "Move to Level 0 -> status {}, level {lv0}; Move to Level 255 -> status {}, level {lv1}",
        fmt(&st0),
        fmt(&st1)
    ))
}

fn bind_payload(s: &Session) -> Vec<u8> {
    let mut p = ext_bytes(s.dut_ext).to_vec();
    p.push(1);
    p.extend_from_slice(&LEVEL.to_le_bytes());
    p.push(0x03);
    p.extend_from_slice(&ext_bytes(s.w.coord.ext));
    p.push(1);
    p
}

fn cfg_payload(min: u16, max: u16, change: u8) -> Vec<u8> {
    let mut p = vec![0x00];
    p.extend_from_slice(&0x0000u16.to_le_bytes());
    p.push(0x20);
    p.extend_from_slice(&min.to_le_bytes());
    p.extend_from_slice(&max.to_le_bytes());
    p.push(change);
    p
}

/// Python's `repr()` of a list of str.
fn py_str_list(v: &[String]) -> String {
    format!("[{}]", v.iter().map(|x| format!("'{x}'")).collect::<Vec<_>>().join(", "))
}

fn level_reporting(s: &mut Session) -> R<String> {
    start(s, 254)?;
    let bp = bind_payload(s);
    let f = s.zdo(0x0021, &bp, 2000.0)?;
    if !f.is_some_and(|f| f.zdo().unwrap().payload.first() == Some(&0)) {
        return Err(Stop::Inconclusive("Bind_req for the Level Control cluster was not accepted".into()));
    }
    finally(
        s,
        |s| {
            let f = s.zcl(LEVEL, 0x06, &cfg_payload(1, 60, 1), Tx::new().general())?;
            let ok = f.as_ref().is_some_and(|f| {
                let z = f.zcl().unwrap();
                !z.specific && z.cmd == 0x07
            });
            expect(
                ok,
                format!(
                    "no Configure Reporting Response: {}",
                    f.as_ref().map(|f| f.brief()).unwrap_or_else(|| "nothing".into())
                ),
            )?;
            let st = f.unwrap().zcl().unwrap().payload.first().copied();
            let Some(st) = st else { return Err(Stop::Error("IndexError: empty Configure Reporting Response".into())) };
            expect(st == 0, format!("Configure Reporting Response status {st:#04x} for CurrentLevel"))?;
            s.run(1500.0)?;
            let i = s.mark();
            s.tx_zcl(LEVEL, 0x00, &mtl_payload(99, 0, 0, 0), Tx::new().ddr());
            let reps = s.collect(
                |f| {
                    f.zcl().is_some_and(|z| !z.specific && z.cmd == 0x0A)
                        && f.aps.as_ref().and_then(|a| a.cluster) == Some(LEVEL)
                },
                4000.0,
                Some(i),
            )?;
            let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
            let vals: Vec<String> = reps.iter().map(|r| hex(&r.zcl().unwrap().payload)).collect();
            expect(!reps.is_empty(), "no Report Attributes within 4 s of a level change (min interval 1 s, reportable change 1)")?;
            expect(
                reps.iter().any(|r| r.zcl().unwrap().payload.get(..4) == Some(&[0x00, 0x00, 0x20, 99][..])),
                format!("no report carries CurrentLevel (uint8) = 99: {}", py_str_list(&vals)),
            )?;
            Ok(format!("{} report(s): {}", reps.len(), py_str_list(&vals)))
        },
        |s| {
            s.zcl(LEVEL, 0x06, &cfg_payload(0, 0xFFFF, 0), Tx::new().general())?;
            let bp = bind_payload(s);
            s.zdo(0x0022, &bp, 2000.0)?;
            start(s, 254)?;
            Ok(())
        },
    )
}

fn start_up_current_level(s: &mut Session) -> R<String> {
    let orig = attr(s, LEVEL, 0x4000)?;
    let mut obs: Vec<String> = Vec::new();
    let mut case = |s: &mut Session, value: u8, before: u8, want: i64, label: &str| -> R<()> {
        let st = write_attr(s, LEVEL, 0x4000, 0x20, vec![value])?;
        expect(st == 0, format!("writing StartUpCurrentLevel = {value:#04x}: status {st:#04x}"))?;
        start(s, before)?;
        s.run(2500.0)?;
        s.reboot()?;
        s.run(2500.0)?;
        if !s.alive()? {
            return Err(Stop::Inconclusive("the bulb did not answer after the power cycle".into()));
        }
        let lv = level(s)?;
        obs.push(format!("{label}: {lv}"));
        expect(
            lv == want,
            format!(
                "StartUpCurrentLevel {value:#04x}, level {before} before the power cycle: CurrentLevel {lv} after it, \
                 {want} required"
            ),
        )
    };
    finally(
        s,
        |s| {
            case(s, 100, 200, 100, "100")?;
            case(s, 0xFF, 60, 60, "0xff from 60")?;
            let mn = min_level(s)?;
            case(s, 0x00, 200, mn, "0x00")
        },
        |s| {
            s.write(LEVEL, &[(0x4000, 0x20, vec![orig as u8])])?;
            s.cmd(ONOFF, 0x01, &[])?;
            Ok(())
        },
    )?;
    Ok(obs.join("; "))
}
