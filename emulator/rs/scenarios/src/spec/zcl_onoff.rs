use super::Test;
use crate::frames::{py_value, ZclValue};
use crate::session::{expect, CmdAnswer, Session, Stop, R};
use crate::station::Tx;

const S: &str = "zcl-onoff";
const ONOFF: u16 = 0x0006;
const LEVEL: u16 = 0x0008;
const SCENES: u16 = 0x0005;
const GROUPS: u16 = 0x0004;
const SETTLE: f64 = 1500.0;

fn t(id: &'static str, doc: &'static str, clause: &'static str, req: &'static str, run: fn(&mut Session) -> R<String>) -> Test {
    Test { id, doc, clause, req, session: S, run, ..Test::D }
}

pub fn tests() -> Vec<Test> {
    vec![
        t("ZCL-ONOFF-01", "ZCL8", "3.8.2.3.1-3.8.2.3.3",
          "Off puts the device in its off state, On in its on state, Toggle in the other state; the OnOff attribute \
           follows.", on_off_toggle),
        t("ZCL-ONOFF-02", "ZLO", "7.2.1 Table 5; ZCL8 3.8.2.2 Table 3-45",
          "A dimmable light implements OnOff (bool), GlobalSceneControl (bool), OnTime (uint16), OffWaitTime (uint16) \
           and StartUpOnOff (enum8).", attributes),
        t("ZCL-ONOFF-03", "ZCL8", "3.8.2.3.4.3",
          "Off With Effect switches the device off, sets GlobalSceneControl to FALSE and OnTime to 0.", off_with_effect),
        t("ZCL-ONOFF-04", "ZCL8", "3.8.2.3.5.1, 3.8.2.3.4.3",
          "On With Recall Global Scene with GlobalSceneControl FALSE recalls the settings stored by Off With Effect \
           (here: the level), switches on and sets GlobalSceneControl to TRUE.", recall_global_scene),
        t("ZCL-ONOFF-05", "ZCL8", "3.8.2.3.5.1",
          "On With Recall Global Scene is discarded while GlobalSceneControl is TRUE.", recall_global_scene_discarded),
        t("ZCL-ONOFF-06", "ZCL8", "3.8.2.2.2", "A standard On command sets GlobalSceneControl to TRUE.", gsc_after_on),
        t("ZCL-ONOFF-07", "ZCL8", "3.10.2.1.3, 3.8.2.2.2",
          "Move to Level (with On/Off) that switches the light on sets GlobalSceneControl to TRUE.", gsc_after_level),
        t("ZCL-ONOFF-08", "ZCL8", "3.8.2.2.2",
          "Recall Scene that switches the light on sets GlobalSceneControl to TRUE.", gsc_after_recall_scene),
        t("ZCL-ONOFF-09", "ZCL8", "3.8.2.3.6.4",
          "On With Timed Off switches the light on, OnTime counts down in 1/10 s and when it reaches 0 the light is \
           switched off with OnTime = OffWaitTime = 0.", on_with_timed_off),
        t("ZCL-ONOFF-10", "ZCL8", "3.8.2.3.6.1, 3.8.2.3.6.4",
          "On With Timed Off with Accept Only When On set is discarded while the light is off and processed while it \
           is on.", timed_off_accept_only_when_on),
        t("ZCL-ONOFF-11", "ZCL8", "3.8.2.3.6.4, 3.8.2.3.1.1",
          "After Off during a timed on, OnTime is 0 and OffWaitTime counts down; On With Timed Off received meanwhile \
           does not switch the light on; once OffWaitTime is 0 it does.", off_wait_time),
        t("ZCL-ONOFF-12", "ZCL8", "3.8.2.3.2.1", "On received while OnTime is 0 sets OffWaitTime to 0.",
          on_clears_off_wait),
        t("ZCL-ONOFF-13", "ZCL8", "3.8.2.3.3.1", "Toggle received while the light is on sets OnTime to 0.",
          toggle_clears_on_time),
        t("ZCL-ONOFF-14", "ZCL8", "3.8.2.1.1", "A Level Control command that sets OnOff to 0 sets OnTime to 0.",
          level_off_clears_on_time),
        t("ZCL-ONOFF-15", "ZCL8", "3.8.2.1.1",
          "A Level Control command that sets OnOff to 1 while OnTime is 0 sets OffWaitTime to 0.",
          level_on_clears_off_wait),
        t("ZCL-ONOFF-16", "ZCL8", "3.8.2.2.5 Table 3-46",
          "StartUpOnOff decides the OnOff state after power is applied: 0x00 off, 0x01 on, 0x02 the opposite of the \
           previous state, 0xff the previous state.", start_up_on_off),
    ]
}

pub fn oo(b: bool) -> &'static str {
    if b {
        "on"
    } else {
        "off"
    }
}

pub fn tf(b: bool) -> &'static str {
    if b {
        "True"
    } else {
        "False"
    }
}

pub fn py_f(x: f64) -> String {
    if x.fract() == 0.0 && x.abs() < 1e16 {
        format!("{x:.1}")
    } else {
        format!("{x}")
    }
}

pub fn round1(x: f64) -> f64 {
    format!("{x:.1}").parse().unwrap()
}

pub fn ans_py(a: &CmdAnswer) -> String {
    match a {
        CmdAnswer::None => "None".into(),
        CmdAnswer::Status(st) => st.to_string(),
        CmdAnswer::Frame(f) => f.brief(),
    }
}

pub fn status(a: &CmdAnswer) -> Option<u8> {
    match a {
        CmdAnswer::Status(st) => Some(*st),
        _ => None,
    }
}

pub fn attr(s: &mut Session, cluster: u16, aid: u16) -> R<i64> {
    let (st, _, v) = s.read1(cluster, aid)?;
    match st {
        None => Err(Stop::Inconclusive(format!("no Read Attributes Response for {cluster:#06x}/{aid:#06x}"))),
        Some(st) if st != 0 => Err(Stop::NotApplicable(format!(
            "attribute {aid:#06x} of cluster {cluster:#06x} is not implemented (status {st:#04x})"
        ))),
        _ => match v {
            Some(ZclValue::Int(i)) => Ok(i),
            v => Err(Stop::Error(format!("TypeError: attribute {aid:#06x} is not an integer: {}", py_value(&v)))),
        },
    }
}

pub fn opt_attr(s: &mut Session, cluster: u16, aid: u16, default: i64) -> R<i64> {
    let (st, _, v) = s.read1(cluster, aid)?;
    Ok(if st == Some(0) { v.and_then(|v| v.int()).unwrap_or(default) } else { default })
}

pub fn act(s: &mut Session, cluster: u16, cmd: u8, payload: &[u8], what: &str, settle: f64) -> R<()> {
    match s.cmd(cluster, cmd, payload)? {
        CmdAnswer::None => return Err(Stop::Fail(format!("{what}: no Default Response"))),
        CmdAnswer::Frame(f) => return Err(Stop::Fail(format!("{what}: answered with {}", f.brief()))),
        CmdAnswer::Status(st) if st != 0 => return Err(Stop::Fail(format!("{what}: Default Response status {st:#04x}"))),
        CmdAnswer::Status(_) => {}
    }
    if settle > 0.0 {
        s.run(settle)?;
    }
    Ok(())
}

pub fn write_attr(s: &mut Session, cluster: u16, aid: u16, dtype: u8, value: Vec<u8>) -> R<u8> {
    let f = s.write(cluster, &[(aid, dtype, value)])?;
    let Some(f) = f else {
        return Err(Stop::Inconclusive(format!("no Write Attributes Response for {cluster:#06x}/{aid:#06x}")));
    };
    match f.zcl().and_then(|z| z.payload.first().copied()) {
        Some(st) => Ok(st),
        None => Err(Stop::Error("IndexError: empty Write Attributes Response".into())),
    }
}

pub fn finally<T>(s: &mut Session, body: impl FnOnce(&mut Session) -> R<T>,
                         fin: impl FnOnce(&mut Session) -> R<()>) -> R<T> {
    let r = body(s);
    fin(s)?;
    r
}

pub fn check_types(r: &[(u16, u8, Option<u8>, Option<ZclValue>)], want: &[(u16, u8)]) -> Vec<String> {
    let mut bad = Vec::new();
    for &(a, t) in want {
        match r.iter().find(|x| x.0 == a) {
            None => bad.push(format!("{a:#06x}: missing from the response")),
            Some((_, st, _, _)) if *st != 0 => bad.push(format!("{a:#06x}: status {st:#04x}")),
            Some((_, _, typ, _)) if *typ != Some(t) => {
                bad.push(format!("{a:#06x}: type {:#04x}, {t:#04x} required", typ.unwrap_or(0)))
            }
            _ => {}
        }
    }
    bad
}

fn state(s: &mut Session) -> R<(bool, i64)> {
    let on = s.light().on;
    Ok((on, attr(s, ONOFF, 0x0000)?))
}

fn expect_state(s: &mut Session, on: bool, when: &str) -> R<()> {
    let (pwm, a) = state(s)?;
    expect(
        pwm == on && a == if on { 1 } else { 0 },
        format!("{when}: PWM output {}, OnOff attribute {a}; expected {}", oo(pwm), oo(on)),
    )
}

fn move_to_level(s: &mut Session, level: u8, settle: f64) -> R<()> {
    act(s, LEVEL, 0x04, &[level, 0, 0, 0, 0], &format!("Move to Level (with On/Off) {level}"), settle)
}

fn timed_payload(ctrl: u8, on_time: u16, off_wait: u16) -> Vec<u8> {
    let mut p = vec![ctrl];
    p.extend_from_slice(&on_time.to_le_bytes());
    p.extend_from_slice(&off_wait.to_le_bytes());
    p
}

fn timed_off(s: &mut Session, ctrl: u8, on_time: u16, off_wait: u16, settle: f64) -> R<()> {
    act(s, ONOFF, 0x42, &timed_payload(ctrl, on_time, off_wait), "On With Timed Off", settle)
}

fn power_cycle(s: &mut Session, settle: f64) -> R<bool> {
    s.reboot()?;
    s.run(settle)?;
    let on = s.light().on;
    if !s.alive()? {
        return Err(Stop::Inconclusive("the bulb did not answer after the power cycle".into()));
    }
    Ok(on)
}

fn on_off_toggle(s: &mut Session) -> R<String> {
    act(s, ONOFF, 0x00, &[], "Off", SETTLE)?;
    expect_state(s, false, "after Off")?;
    act(s, ONOFF, 0x01, &[], "On", SETTLE)?;
    expect_state(s, true, "after On")?;
    act(s, ONOFF, 0x02, &[], "Toggle", SETTLE)?;
    expect_state(s, false, "after Toggle from on")?;
    act(s, ONOFF, 0x02, &[], "Toggle", SETTLE)?;
    expect_state(s, true, "after Toggle from off")?;
    act(s, ONOFF, 0x01, &[], "On", SETTLE)?;
    expect_state(s, true, "after On while on")?;
    Ok(String::new())
}

fn attributes(s: &mut Session) -> R<String> {
    let Some(r) = s.read(ONOFF, &[0x0000, 0x4000, 0x4001, 0x4002, 0x4003], 1500.0)? else {
        return Err(Stop::Fail("no Read Attributes Response".into()));
    };
    let bad = check_types(&r, &[(0x0000, 0x10), (0x4000, 0x10), (0x4001, 0x21), (0x4002, 0x21), (0x4003, 0x30)]);
    expect(bad.is_empty(), bad.join("; "))?;
    Ok(String::new())
}

fn off_with_effect(s: &mut Session) -> R<String> {
    act(s, ONOFF, 0x01, &[], "On", 300.0)?;
    let g = attr(s, ONOFF, 0x4000)?;
    expect(g == 1, "GlobalSceneControl is not TRUE after On")?;
    act(s, ONOFF, 0x40, &[0x00, 0x00], "Off With Effect", SETTLE)?;
    expect_state(s, false, "after Off With Effect")?;
    let g = attr(s, ONOFF, 0x4000)?;
    expect(g == 0, "GlobalSceneControl is still TRUE after Off With Effect")?;
    let ot = attr(s, ONOFF, 0x4001)?;
    expect(ot == 0, "OnTime is not 0 after Off With Effect")?;
    act(s, ONOFF, 0x01, &[], "On", 300.0)?;
    Ok(String::new())
}

fn recall_global_scene(s: &mut Session) -> R<String> {
    move_to_level(s, 60, SETTLE)?;
    act(s, ONOFF, 0x40, &[0x00, 0x00], "Off With Effect", SETTLE)?;
    let g = attr(s, ONOFF, 0x4000)?;
    expect(g == 0, "GlobalSceneControl is still TRUE after Off With Effect")?;
    let st = s.cmd(LEVEL, 0x00, &[200, 0, 0, 1, 1])?;
    s.run(300.0)?;
    let changed = attr(s, LEVEL, 0x0000)? == 200;
    act(s, ONOFF, 0x41, &[], "On With Recall Global Scene", SETTLE)?;
    expect_state(s, true, "after On With Recall Global Scene")?;
    let lvl = attr(s, LEVEL, 0x0000)?;
    expect(
        lvl == 60,
        format!(
            "CurrentLevel {lvl} after the recall; 60 was stored by Off With Effect{}",
            if changed { " (the level had been changed to 200 while off)" } else { "" }
        ),
    )?;
    let g = attr(s, ONOFF, 0x4000)?;
    expect(g == 1, "GlobalSceneControl is not TRUE after On With Recall Global Scene")?;
    Ok(if changed {
        "level changed while off and restored".into()
    } else {
        format!(
            "the level could not be changed while off (Move to Level with ExecuteIfOff override: status {})",
            ans_py(&st)
        )
    })
}

fn recall_global_scene_discarded(s: &mut Session) -> R<String> {
    act(s, ONOFF, 0x01, &[], "On", 300.0)?;
    act(s, ONOFF, 0x00, &[], "Off", SETTLE)?;
    let g = attr(s, ONOFF, 0x4000)?;
    expect(
        g == 1,
        "GlobalSceneControl is FALSE after On followed by a plain Off (3.8.2.2.2: only Off With Effect sets it to \
         FALSE)",
    )?;
    s.cmd(ONOFF, 0x41, &[])?;
    s.run(SETTLE)?;
    let (pwm, a) = state(s)?;
    act(s, ONOFF, 0x01, &[], "On", 300.0)?;
    expect(
        !pwm && a == 0,
        format!("the command was executed with GlobalSceneControl TRUE: PWM {}, OnOff {a}", oo(pwm)),
    )?;
    Ok(String::new())
}

fn gsc_after_on(s: &mut Session) -> R<String> {
    act(s, ONOFF, 0x40, &[0x00, 0x00], "Off With Effect", SETTLE)?;
    let g = attr(s, ONOFF, 0x4000)?;
    expect(g == 0, "GlobalSceneControl is still TRUE after Off With Effect")?;
    act(s, ONOFF, 0x01, &[], "On", SETTLE)?;
    let g = attr(s, ONOFF, 0x4000)?;
    expect(g == 1, "GlobalSceneControl is still FALSE after On")?;
    Ok(String::new())
}

fn gsc_after_level(s: &mut Session) -> R<String> {
    act(s, ONOFF, 0x40, &[0x00, 0x00], "Off With Effect", SETTLE)?;
    let g = attr(s, ONOFF, 0x4000)?;
    expect(g == 0, "GlobalSceneControl is still TRUE after Off With Effect")?;
    move_to_level(s, 150, SETTLE)?;
    let (pwm, a) = state(s)?;
    if !(pwm && a == 1) {
        act(s, ONOFF, 0x01, &[], "On", 300.0)?;
        return Err(Stop::Inconclusive(format!(
            "Move to Level (with On/Off) did not switch the light on (PWM {}, OnOff {a})",
            tf(pwm)
        )));
    }
    let g = attr(s, ONOFF, 0x4000)?;
    act(s, ONOFF, 0x01, &[], "On", 300.0)?;
    expect(g == 1, "GlobalSceneControl is still FALSE after Move to Level (with On/Off) switched the light on")?;
    Ok(String::new())
}

fn gsc_after_recall_scene(s: &mut Session) -> R<String> {
    s.need_cluster(SCENES)?;
    let (grp, scene) = (0x5A01u16, 7u8);
    let gs = |extra: &[u8]| {
        let mut p = grp.to_le_bytes().to_vec();
        p.extend_from_slice(extra);
        p
    };
    act(s, ONOFF, 0x01, &[], "On", 300.0)?;
    let f = s.zcl(GROUPS, 0x00, &gs(&[0]), Tx::new())?;
    let ok = f.as_ref().is_some_and(|f| {
        let z = f.zcl().unwrap();
        z.specific && matches!(z.payload.first(), Some(0x00) | Some(0x8A))
    });
    if !ok {
        return Err(Stop::Inconclusive(format!(
            "Add Group was not accepted: {}",
            f.map(|f| f.brief()).unwrap_or_else(|| "no response".into())
        )));
    }
    let f = s.zcl(SCENES, 0x04, &gs(&[scene]), Tx::new())?;
    let ok = f.as_ref().is_some_and(|f| {
        let z = f.zcl().unwrap();
        z.specific && z.payload.first() == Some(&0)
    });
    if !ok {
        return Err(Stop::Inconclusive(format!(
            "Store Scene was not accepted: {}",
            f.map(|f| f.brief()).unwrap_or_else(|| "no response".into())
        )));
    }
    finally(
        s,
        |s| {
            act(s, ONOFF, 0x40, &[0x00, 0x00], "Off With Effect", SETTLE)?;
            let g = attr(s, ONOFF, 0x4000)?;
            expect(g == 0, "GlobalSceneControl is still TRUE after Off With Effect")?;
            act(s, SCENES, 0x05, &gs(&[scene]), "Recall Scene", SETTLE)?;
            let (pwm, a) = state(s)?;
            if !(pwm && a == 1) {
                return Err(Stop::Fail(format!(
                    "Recall Scene of a scene stored with the light on left it off (PWM {}, OnOff {a})",
                    tf(pwm)
                )));
            }
            let g = attr(s, ONOFF, 0x4000)?;
            expect(g == 1, "GlobalSceneControl is still FALSE after Recall Scene switched the light on")?;
            Ok(String::new())
        },
        |s| {
            s.zcl(GROUPS, 0x03, &gs(&[]), Tx::new())?;
            s.cmd(ONOFF, 0x01, &[])?;
            Ok(())
        },
    )
}

fn on_with_timed_off(s: &mut Session) -> R<String> {
    act(s, ONOFF, 0x00, &[], "Off", SETTLE)?;
    let t0 = s.now();
    timed_off(s, 0, 40, 0, 300.0)?;
    expect_state(s, true, "0.3 s after On With Timed Off (on time 4.0 s)")?;
    let mut obs = Vec::new();
    for _ in 0..2 {
        let v = attr(s, ONOFF, 0x4001)?;
        let want = 40.0 - (s.now() - t0) / 100.0;
        let el = (s.now() - t0) / 1000.0;
        obs.push(format!("{v} at {el:.2} s"));
        expect(
            (v as f64 - want).abs() <= 3.0,
            format!("OnTime {v} at {el:.2} s after the command; {want:.0} expected (4.0 s counting down in 1/10 s)"),
        )?;
        s.run(1200.0)?;
    }
    let el = (s.now() - t0) / 1000.0;
    expect_state(s, true, &format!("{el:.1} s after the command (before the on time is over)"))?;
    let w = (t0 + 4600.0 - s.now()).max(0.0);
    s.run(w)?;
    expect_state(s, false, "4.6 s after On With Timed Off with on time 4.0 s")?;
    let ok = attr(s, ONOFF, 0x4001)? == 0 && attr(s, ONOFF, 0x4002)? == 0;
    expect(ok, "OnTime/OffWaitTime not 0 after the timed off")?;
    act(s, ONOFF, 0x01, &[], "On", 300.0)?;
    Ok(format!("OnTime {}", obs.join(", ")))
}

fn timed_off_accept_only_when_on(s: &mut Session) -> R<String> {
    act(s, ONOFF, 0x00, &[], "Off", SETTLE)?;
    s.cmd(ONOFF, 0x42, &timed_payload(1, 30, 0))?;
    s.run(600.0)?;
    let (pwm, a) = state(s)?;
    expect(!pwm && a == 0, format!("executed while off: PWM {}, OnOff {a}", oo(pwm)))?;
    let ot = attr(s, ONOFF, 0x4001)?;
    expect(ot == 0, "OnTime was set by a command that had to be discarded")?;
    act(s, ONOFF, 0x01, &[], "On", SETTLE)?;
    timed_off(s, 1, 20, 0, 300.0)?;
    let v = attr(s, ONOFF, 0x4001)?;
    expect(0 < v && v <= 20, format!("OnTime {v} after the command while on (on time 2.0 s); it was not processed"))?;
    s.run(2600.0)?;
    expect_state(s, false, "2.9 s after On With Timed Off (accept only when on, on time 2.0 s) while on")?;
    act(s, ONOFF, 0x01, &[], "On", 300.0)?;
    Ok(String::new())
}

fn off_wait_time(s: &mut Session) -> R<String> {
    act(s, ONOFF, 0x01, &[], "On", 300.0)?;
    timed_off(s, 0, 100, 40, 300.0)?;
    act(s, ONOFF, 0x00, &[], "Off", 0.0)?;
    let t_off = s.now();
    s.run(SETTLE)?;
    expect_state(s, false, "after Off")?;
    let ot = attr(s, ONOFF, 0x4001)?;
    expect(ot == 0, "OnTime is not 0 after Off")?;
    let w = attr(s, ONOFF, 0x4002)?;
    let want = 40.0 - (s.now() - t_off) / 100.0;
    expect(
        (w as f64 - want).abs() <= 4.0,
        format!(
            "OffWaitTime {w} at {:.1} s after Off; {want:.0} expected (off wait time 4.0 s counting down)",
            (s.now() - t_off) / 1000.0
        ),
    )?;
    s.cmd(ONOFF, 0x42, &timed_payload(0, 100, 40))?;
    s.run(700.0)?;
    let (pwm, a) = state(s)?;
    expect(
        !pwm && a == 0,
        format!("On With Timed Off during the off wait time switched the light on (PWM {}, OnOff {a})", oo(pwm)),
    )?;
    let w = (t_off + 4700.0 - s.now()).max(0.0);
    s.run(w)?;
    let ow = attr(s, ONOFF, 0x4002)?;
    expect(ow == 0, "OffWaitTime is not 0 4.7 s after Off (off wait time 4.0 s)")?;
    timed_off(s, 0, 15, 0, 600.0)?;
    expect_state(s, true, "after On With Timed Off once the off wait time is over")?;
    act(s, ONOFF, 0x00, &[], "Off", 300.0)?;
    act(s, ONOFF, 0x01, &[], "On", 300.0)?;
    Ok(String::new())
}

fn on_clears_off_wait(s: &mut Session) -> R<String> {
    act(s, ONOFF, 0x01, &[], "On", 300.0)?;
    timed_off(s, 0, 100, 80, 300.0)?;
    act(s, ONOFF, 0x00, &[], "Off", 500.0)?;
    let w = attr(s, ONOFF, 0x4002)?;
    if w == 0 {
        return Err(Stop::Inconclusive(
            "OffWaitTime was already 0 after Off during a timed on with off wait time 8.0 s".into(),
        ));
    }
    act(s, ONOFF, 0x01, &[], "On", 300.0)?;
    let w2 = attr(s, ONOFF, 0x4002)?;
    expect_state(s, true, "after On")?;
    expect(w2 == 0, format!("OffWaitTime {w2} after On with OnTime 0 (it was {w} before)"))?;
    Ok(String::new())
}

fn toggle_clears_on_time(s: &mut Session) -> R<String> {
    act(s, ONOFF, 0x01, &[], "On", 300.0)?;
    timed_off(s, 0, 100, 0, 300.0)?;
    let t = attr(s, ONOFF, 0x4001)?;
    if t == 0 {
        return Err(Stop::Inconclusive("OnTime is 0 right after On With Timed Off with on time 10.0 s".into()));
    }
    act(s, ONOFF, 0x02, &[], "Toggle", SETTLE)?;
    expect_state(s, false, "after Toggle while on")?;
    let t2 = attr(s, ONOFF, 0x4001)?;
    act(s, ONOFF, 0x01, &[], "On", 300.0)?;
    expect(t2 == 0, format!("OnTime {t2} after Toggle switched the light off (it was {t} before)"))?;
    Ok(String::new())
}

fn level_off_clears_on_time(s: &mut Session) -> R<String> {
    act(s, ONOFF, 0x01, &[], "On", 300.0)?;
    timed_off(s, 0, 100, 0, 300.0)?;
    let t = attr(s, ONOFF, 0x4001)?;
    if t == 0 {
        return Err(Stop::Inconclusive("OnTime is 0 right after On With Timed Off with on time 10.0 s".into()));
    }
    let (st, _, mn) = s.read1(LEVEL, 0x0002)?;
    let first = if st == Some(0) { mn.and_then(|v| v.int()).unwrap_or(1) } else { 1 };
    let (mut a, mut how) = (1i64, String::new());
    for lvl in [first, 0] {
        s.cmd(LEVEL, 0x04, &[lvl as u8, 0, 0, 0, 0])?;
        s.run(SETTLE)?;
        a = attr(s, ONOFF, 0x0000)?;
        how = format!("Move to Level (with On/Off) {lvl}");
        if a == 0 {
            break;
        }
    }
    if a != 0 {
        s.cmd(LEVEL, 0x05, &[1, 250, 0, 0])?;
        s.run(SETTLE + 500.0)?;
        a = attr(s, ONOFF, 0x0000)?;
        how = "Move (with On/Off) down to the end".into();
    }
    if a != 0 {
        act(s, ONOFF, 0x00, &[], "Off", 300.0)?;
        act(s, ONOFF, 0x01, &[], "On", 300.0)?;
        move_to_level(s, 254, SETTLE)?;
        return Err(Stop::Inconclusive(
            "no Level Control command set OnOff to 0 (tried Move to Level (with On/Off) to the minimum level and to \
             level 0, and Move (with On/Off) down to the end)"
                .into(),
        ));
    }
    let t2 = attr(s, ONOFF, 0x4001)?;
    move_to_level(s, 254, SETTLE)?;
    expect(t2 == 0, format!("OnTime {t2} after {how} set OnOff to 0 (it was {t} before)"))?;
    Ok(how)
}

fn level_on_clears_off_wait(s: &mut Session) -> R<String> {
    act(s, ONOFF, 0x01, &[], "On", 300.0)?;
    timed_off(s, 0, 100, 80, 300.0)?;
    act(s, ONOFF, 0x00, &[], "Off", 500.0)?;
    let w = attr(s, ONOFF, 0x4002)?;
    if w == 0 {
        return Err(Stop::Inconclusive(
            "OffWaitTime was already 0 after Off during a timed on with off wait time 8.0 s".into(),
        ));
    }
    move_to_level(s, 200, 600.0)?;
    let (_pwm, a) = state(s)?;
    let w2 = attr(s, ONOFF, 0x4002)?;
    if a != 1 {
        s.run(8000.0)?;
        act(s, ONOFF, 0x01, &[], "On", 300.0)?;
        return Err(Stop::Inconclusive("Move to Level (with On/Off) did not set OnOff to 1".into()));
    }
    expect(
        w2 == 0,
        format!("OffWaitTime {w2} after a Level Control command set OnOff to 1 with OnTime 0 (it was {w} before)"),
    )?;
    Ok(String::new())
}

fn start_up_on_off(s: &mut Session) -> R<String> {
    let orig = attr(s, ONOFF, 0x4003)?;
    let mut obs: Vec<String> = Vec::new();
    let cases = [
        (0x00u8, true, false, "0x00 from on"),
        (0x01, false, true, "0x01 from off"),
        (0xFF, false, false, "0xff from off"),
        (0xFF, true, true, "0xff from on"),
        (0x02, true, false, "0x02 from on"),
        (0x02, false, true, "0x02 from off"),
    ];
    finally(
        s,
        |s| {
            for (value, before_on, want_on, label) in cases {
                let st = write_attr(s, ONOFF, 0x4003, 0x30, vec![value])?;
                expect(st == 0, format!("writing StartUpOnOff = {value:#04x}: status {st:#04x}"))?;
                act(s, ONOFF, if before_on { 0x01 } else { 0x00 }, &[], if before_on { "On" } else { "Off" }, 2500.0)?;
                let pwm = power_cycle(s, 2500.0)?;
                let a = attr(s, ONOFF, 0x0000)?;
                obs.push(format!("{label}: {}", oo(a != 0)));
                expect(
                    pwm == want_on && a == if want_on { 1 } else { 0 },
                    format!(
                        "StartUpOnOff {value:#04x}, {} before the power cycle: after it PWM {}, OnOff {a}; {} required",
                        oo(before_on),
                        oo(pwm),
                        oo(want_on)
                    ),
                )?;
            }
            Ok(())
        },
        |s| {
            s.write(ONOFF, &[(0x4003, 0x30, vec![orig as u8])])?;
            s.cmd(ONOFF, 0x01, &[])?;
            Ok(())
        },
    )?;
    Ok(obs.join("; "))
}
