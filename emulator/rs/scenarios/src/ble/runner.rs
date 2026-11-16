//! The image x BLE test runner (`ble_spec_suite.py`): one booted bulb per
//! (image, session); between the tests of a session the connection is dropped
//! and the bulb must advertise again; a test that loses the bulb ends the
//! session and the next test boots a new one.

use std::collections::VecDeque;
use std::sync::{mpsc, Arc, Mutex};

use super::harness::BleSession;
use super::{tests, BleTest, Level};
use crate::air::Log;
use crate::image::Image;
use crate::runner::{drive, parse_args_with, repo_root, selected, Args, Outcome, TestMeta, Verdict, NOT_STARTED};
use crate::session::Stop;

/// The stock firmware and the Rust firmware.
pub fn default_images() -> Vec<Image> {
    let r = repo_root();
    vec![
        Image { label: "stock".into(), path: crate::sims::image_path("stock/stock-dimm.bin"), link_shift: None },
        Image { label: "rust".into(), path: r.join("target/tc32/bulb-tc32.bin"), link_shift: None },
    ]
}

fn run_group(image: &Image, session: &str, tests: &[&BleTest], cpi: i64, verbose: u8, out: &mpsc::Sender<Outcome>) {
    let log: Option<Log> = if verbose > 1 {
        let label = image.label.clone();
        Some(std::rc::Rc::new(move |s: &str| println!("      [{label}] {s}")))
    } else {
        None
    };
    let send = |id: &'static str, verdict: Verdict, obs: String, sim_s: f64| {
        if verbose > 0 {
            println!("   [{} {session}] {:12} {id}  {obs}", image.label, verdict.name());
        }
        let _ = out.send(Outcome { image: image.label.clone(), id, verdict, obs, sim_s });
    };
    let mut s: Option<BleSession> = None;
    for (k, t) in tests.iter().enumerate() {
        if s.is_none() {
            match BleSession::new(image, cpi, log.clone(), 1500.0) {
                Ok(x) => s = Some(x),
                Err(e) => {
                    let m = match e {
                        Stop::Fail(m) | Stop::Inconclusive(m) | Stop::NotApplicable(m) | Stop::SessionLost(m) | Stop::Error(m) => m,
                    };
                    send(t.id, Verdict::Inconclusive, format!("{NOT_STARTED}{m}"), 0.0);
                    continue;
                }
            }
        }
        let ss = s.as_mut().unwrap();
        let sim0 = ss.now();
        let r = (t.run)(ss);
        let sim_s = ((ss.now() - sim0) / 100.0).round() / 10.0;
        let mut lost = false;
        match r {
            Ok(obs) => send(t.id, Verdict::Pass, obs, sim_s),
            Err(Stop::Fail(m)) => {
                let v = if t.level == Level::Shall { Verdict::Fail } else { Verdict::Note };
                send(t.id, v, m, sim_s)
            }
            Err(Stop::Inconclusive(m)) => send(t.id, Verdict::Inconclusive, m, sim_s),
            Err(Stop::NotApplicable(m)) => send(t.id, Verdict::NotApplicable, m, sim_s),
            Err(Stop::SessionLost(m)) => {
                send(t.id, Verdict::Inconclusive, m, sim_s);
                lost = true;
            }
            Err(Stop::Error(m)) => {
                send(t.id, Verdict::Error, m, sim_s);
                lost = true;
            }
        }
        if !lost && k + 1 < tests.len() {
            // the next test of the session starts from an advertising bulb
            let ss = s.as_mut().unwrap();
            if ss.drop_link(6000.0).and_then(|_| ss.adv_reply_clear()).is_err() {
                lost = true;
            }
        }
        if lost {
            s = None;
        }
    }
}

pub fn run(a: &Args, on: &mut dyn FnMut(&Outcome)) -> Vec<Outcome> {
    let all = tests();
    let mut groups: Vec<(Image, String, Vec<&BleTest>)> = Vec::new();
    for img in &a.images {
        let mut by_session: Vec<(String, Vec<&BleTest>)> = Vec::new();
        for t in &all {
            if !selected(a, &format!("{}::{}", img.label, t.id)) {
                continue;
            }
            let g = t.group();
            match by_session.iter_mut().find(|(s, _)| *s == g) {
                Some((_, v)) => v.push(t),
                None => by_session.push((g, vec![t])),
            }
        }
        for (s, v) in by_session {
            groups.push((img.clone(), s, v));
        }
    }
    groups.sort_by_key(|g| std::cmp::Reverse(g.2.len()));
    let (tx, rx) = mpsc::channel();
    let queue = Arc::new(Mutex::new(groups.into_iter().collect::<VecDeque<_>>()));
    let out = std::thread::scope(|sc| {
        for _ in 0..a.jobs.max(1) {
            let (q, tx) = (queue.clone(), tx.clone());
            sc.spawn(move || loop {
                let Some((img, session, tests)) = q.lock().unwrap().pop_front() else { break };
                if !img.path.exists() {
                    for t in &tests {
                        let _ = tx.send(Outcome {
                            image: img.label.clone(),
                            id: t.id,
                            verdict: Verdict::Error,
                            obs: format!("no image at {}", img.path.display()),
                            sim_s: 0.0,
                        });
                    }
                    continue;
                }
                run_group(&img, &session, &tests, a.cpi, a.verbose, &tx);
            });
        }
        drop(tx);
        rx.iter().map(|o| {
            on(&o);
            o
        }).collect::<Vec<_>>()
    });
    let mut out = out;
    let img_pos = |l: &str| a.images.iter().position(|i| i.label == l).unwrap_or(usize::MAX);
    let test_pos = |id: &str| all.iter().position(|t| t.id == id).unwrap_or(usize::MAX);
    out.sort_by_key(|o| (img_pos(&o.image), test_pos(o.id)));
    out
}

/// The `ble` test binary's main.
pub fn main_with(argv: &[String]) -> i32 {
    let a = match parse_args_with(argv, default_images) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("scenarios ble: {e}");
            return 2;
        }
    };
    let meta: Vec<TestMeta> = super::modules()
        .into_iter()
        .flat_map(|(module, v)| {
            v.into_iter().map(move |t| TestMeta {
                id: t.id,
                module,
                doc: t.doc,
                clause: t.clause,
                req: t.req,
                mandatory: t.mandatory,
                level: match t.level {
                    Level::Shall => "shall",
                    Level::Should => "should",
                    Level::Reading => "reading",
                },
                ambiguous: t.ambiguous,
            })
        })
        .collect();
    drive(&a, "ble", &meta, &mut |on| run(&a, on))
}
