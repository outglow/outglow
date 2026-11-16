pub mod account;
pub mod diag;
pub mod net;
pub mod report;
pub mod scen;

use super::{SimCtx, SimDef};
use crate::session::R;
use net::{pct, AirCount, Cfg, Flow, Mesh};

pub fn defs() -> Vec<SimDef> {
    vec![
        SimDef { name: "stress_noise", doc: "the same traffic over a lossy and noisy channel", run: |cx| run_one(cx, "stress_noise", scen::noise) },
        SimDef { name: "stress_churn", doc: "routers vanish and return, rooms switched off, the bulb power-cycled", run: |cx| run_one(cx, "stress_churn", scen::churn) },
        SimDef { name: "stress_broadcast", doc: "group commands to all bulbs, broadcast storms", run: |cx| run_one(cx, "stress_broadcast", scen::broadcast) },
        SimDef {
            name: "stress_diag",
            doc: "traffic to and through the bulbs at rising rates, with the Diagnostics counters read over ZCL against the air",
            run: |cx| run_one(cx, "stress_diag", diag::run),
        },
    ]
}

#[derive(Clone, Debug, Default)]
pub struct Row {
    pub phase: String,
    pub sent: usize,
    pub ok: usize,
    pub p50: Option<f64>,
    pub p95: Option<f64>,
    pub max: Option<f64>,
    pub flows: Vec<(String, usize, usize)>,
    pub dup: usize,
    pub air: AirCount,
    pub resets: u32,
    pub extra: Vec<(String, String)>,
}

impl Row {
    pub fn ok_pct(&self) -> Option<f64> {
        if self.sent == 0 {
            None
        } else {
            Some(100.0 * self.ok as f64 / self.sent as f64)
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct StressResult {
    pub scenario: String,
    pub variant: String,
    pub images: String,
    pub seed: u32,
    pub cpi: i64,
    pub rows: Vec<Row>,
    pub checks: Vec<(String, bool, String)>,
    pub notes: Vec<String>,
    pub sim_ms: f64,
    pub wall_s: f64,
    pub error: Option<String>,
}

impl StressResult {
    pub fn new(name: &str, cfg: &Cfg) -> StressResult {
        let images = match &cfg.mix {
            Some(m) => format!("{}+{}", cfg.dut.label, m.label),
            None => cfg.dut.label.clone(),
        };
        StressResult { scenario: name.into(), variant: cfg.variant().into(), images, seed: cfg.seed, cpi: cfg.cpi, ..Default::default() }
    }
    pub fn check(&mut self, name: &str, ok: bool, detail: String) {
        self.checks.push((name.to_string(), ok, detail));
    }
    pub fn passed(&self) -> bool {
        self.error.is_none() && self.checks.iter().all(|c| c.1)
    }
}

pub type Scenario = fn(&mut Mesh, &mut StressResult) -> R<()>;

pub fn execute(cfg: Cfg, name: &str, f: Scenario, log: Option<crate::air::Log>) -> StressResult {
    let t0 = std::time::Instant::now();
    let mut res = StressResult::new(name, &cfg);
    match Mesh::build(cfg, log) {
        Ok(mut m) => {
            res.notes.push(format!(
                "joined: {}",
                m.bulbs
                    .iter()
                    .zip(&m.join_ms)
                    .map(|(b, j)| format!("{}={:#06x}@{:.1}s", b.name, b.short, j.unwrap_or(-1.0) / 1000.0))
                    .collect::<Vec<_>>()
                    .join(" ")
            ));
            if let Err(e) = f(&mut m, &mut res) {
                res.error = Some(stop_text(&e));
            }
            for b in &m.bulbs {
                let r = m.s.w.node(b.id).radio.b();
                let drops: Vec<String> = r.dropped.iter().map(|(k, v)| format!("{k} {v}")).collect();
                res.notes.push(format!("radio {}: sent {}, received {}, bad FCS {}, dropped: {}", b.name, r.sent, r.received, r.bad_fcs,
                                       if drops.is_empty() { "none".into() } else { drops.join(", ") }));
            }
            if std::env::var("STRESS_LIGHT").is_ok() {
                for (k, b) in m.bulbs.iter().enumerate() {
                    let v: Vec<String> = m.light_log[k].iter().map(|(t, on)| format!("{:.0}{}", t, if *on { "+" } else { "-" })).collect();
                    println!("light {}: {}", b.name, v.join(" "));
                }
                let g: Vec<String> = m.msgs.iter().filter(|x| x.flow == net::Flow::Group).map(|x| format!("{:.0}:{}", x.t, x.cmd)).collect();
                println!("group cmds: {}", g.join(" "));
            }
            if std::env::var("STRESS_DEBUG").is_ok() {
                for x in m.msgs.iter().filter(|x| x.answer_ctrs.len() > 1) {
                    println!("-- answered twice: {:?} {:#06x}->{:#06x} seq={} at {:.3}, APS counters {:?}", x.flow, x.src, x.dst, x.seq, x.t,
                             x.answer_ctrs);
                    for d in m.dec.iter().filter(|d| d.t0 >= x.t - 1.0 && d.t0 < x.t + 4000.0) {
                        let rel = d.zcl.as_ref().is_some_and(|z| z.seq == x.seq)
                            && d.nwk.as_ref().is_some_and(|n| [n.src, n.dst].contains(&x.dst));
                        if rel {
                            println!("   {}", m.brief(d));
                        }
                    }
                }
                for (i, x) in m.relay_lost_list().into_iter().take(4) {
                    println!("-- relay lost by {}", m.s.w.air.name_of(x));
                    let t = m.dec[i].t0;
                    for d in m.dec.iter().filter(|d| d.t0 >= t - 30.0 && d.t0 < t + 400.0) {
                        if d.who == Some(x) || d.mac_dst == Some(m.short_of(x)) || d.idx == m.dec[i].idx {
                            println!("   {}", m.brief(d));
                        }
                    }
                }
            }
            if let Ok(v) = std::env::var("STRESS_DEBUG") {
                let n: usize = v.parse().unwrap_or(3);
                for p in 0..m.phases.len() {
                    for l in m.debug_failed(p, n) {
                        println!("{l}");
                    }
                }
            }
            if let Some(a) = &m.acct {
                for p in &m.phases {
                    let t1 = if p.t1 < 0.0 { m.now() } else { p.t1 };
                    for l in account::report(&m, a, p.t0, t1, &format!("{name} {} {}", res.images, p.name)) {
                        println!("{l}");
                    }
                }
            }
            let extra: Vec<String> = m.notes.iter().filter(|n| !res.notes.contains(n)).cloned().collect();
            res.notes.extend(extra);
            res.sim_ms = m.now();
        }
        Err(e) => res.error = Some(format!("mesh: {}", stop_text(&e))),
    }
    res.wall_s = t0.elapsed().as_secs_f64();
    res
}

pub fn stop_text(e: &crate::session::Stop) -> String {
    use crate::session::Stop::*;
    match e {
        Fail(m) | Inconclusive(m) | NotApplicable(m) | SessionLost(m) | Error(m) => m.clone(),
    }
}

fn run_one(cx: &mut SimCtx, name: &str, f: Scenario) -> R<()> {
    let cfg = Cfg::from_ctx(cx);
    let res = execute(cfg, name, f, cx.log());
    for line in report::text(&res).lines() {
        cx.say(line);
    }
    for (n, ok, d) in &res.checks {
        cx.results.push((n.clone(), *ok, d.clone()));
    }
    if let Some(e) = &res.error {
        cx.results.push(("scenario ran to the end".into(), false, e.clone()));
    }
    Ok(())
}

pub fn row(m: &Mesh, p: usize) -> Row {
    let ph = &m.phases[p];
    let msgs: Vec<_> = m.msgs.iter().filter(|x| x.phase == p).collect();
    let lat: Vec<f64> = msgs.iter().filter_map(|x| x.latency()).collect();
    let mut flows = Vec::new();
    for f in [Flow::CmdBulb, Flow::ReadBulb, Flow::Report, Flow::PlugReadBulb, Flow::CmdPlug, Flow::Group] {
        let v: Vec<_> = msgs.iter().filter(|x| x.flow == f).collect();
        if !v.is_empty() {
            flows.push((f.tag().to_string(), v.iter().filter(|x| x.ok()).count(), v.len()));
        }
    }
    Row {
        phase: ph.name.clone(),
        sent: msgs.len(),
        ok: msgs.iter().filter(|x| x.ok()).count(),
        p50: pct(&lat, 50.0),
        p95: pct(&lat, 95.0),
        max: lat.iter().cloned().fold(None, |a: Option<f64>, b| Some(a.map_or(b, |a| a.max(b)))),
        flows,
        dup: msgs.iter().filter(|x| x.answer_ctrs.len() > 1).count(),
        air: m.air_count(ph.t0, if ph.t1 < 0.0 { f64::MAX } else { ph.t1 }, None),
        resets: ph.r1.saturating_sub(ph.r0),
        extra: Vec::new(),
    }
}
