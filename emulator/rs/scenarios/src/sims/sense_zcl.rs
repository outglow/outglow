//! The ZCL side of the motion sensing scenarios: the clusters and attributes
//! of Occupancy Sensing (0x0406) and the manufacturer cluster 0xFC5E, reads,
//! writes and reports through the coordinator, checks in the Python
//! scripts' format.

use super::{Sim, SimCtx};
use crate::py;
use crate::session::R;
use crate::station::nx;

pub(super) const EP: u8 = 1;
pub(super) const SENSE_PAN: u16 = 0x5E55;
pub(super) const CL_OCC: u16 = 0x0406;
pub(super) const CL_SENSE: u16 = 0xFC5E;
pub(super) const MANU: u16 = 0x1141;
pub(super) const A_SENS: u16 = 0x0000;
pub(super) const A_RATE: u16 = 0x0001;
pub(super) const A_LINES: u16 = 0x0002;
pub(super) const A_CAL: u16 = 0x0003;
pub(super) const A_TUNE: u16 = 0x0004;
pub(super) const A_LSENS: u16 = 0x0005;
pub(super) const A_ZONE: u16 = 0xFF43;
pub(super) const A_LINE: u16 = 0xFF46;
pub(super) const QUANTUM: f64 = 0.25;

/// `x and x.hex()` of an optional bytes value inside an f-string.
pub(super) fn hexo(v: &Option<Vec<u8>>) -> String {
    match v {
        None => "None".into(),
        Some(b) if b.is_empty() => "b''".into(),
        Some(b) => b.iter().map(|x| format!("{x:02x}")).collect(),
    }
}

pub(super) fn reports_text(v: &[(f64, i64)]) -> String {
    py::list(v.iter().map(|(ms, x)| py::tuple([py::float(*ms), x.to_string()])))
}

pub(super) struct Sense {
    pub(super) s: Sim,
    pub(super) results: Vec<(String, bool, String)>,
    pub(super) retries: Vec<(f64, u16, u8)>,
}

impl Sense {
    pub(super) fn now(&self) -> f64 {
        self.s.now()
    }

    pub(super) fn check(&mut self, cx: &mut SimCtx, name: &str, ok: bool, detail: &str) -> bool {
        self.results.push((name.to_string(), ok, detail.to_string()));
        cx.say(&format!("[{:10.3} ms] {}: {name} {detail}", self.now(), if ok { "PASS" } else { "FAIL" }));
        ok
    }

    pub(super) fn run_until(&mut self, max_ms: f64, cond: impl FnMut(&Sim) -> bool) -> R<bool> {
        self.s.run_until(max_ms, 20.0, cond)
    }

    /// A ZCL frame (optionally with a manufacturer code); the first response's body or None; 3 attempts.
    pub(super) fn zcl(&mut self, short: u16, cluster: u16, cmd: u8, payload: &[u8], manu: Option<u16>, specific: bool) -> R<Option<Vec<u8>>> {
        let want = if specific {
            0x0B
        } else {
            match cmd {
                0x00 => 0x01,
                0x02 => 0x04,
                _ => 0x0B,
            }
        };
        for _ in 0..3 {
            let c = &mut self.s.w.coord;
            let n0 = c.zcl_rsp.len();
            let seq = nx(&mut c.st.zcl_seq);
            let fc = if specific { 1 } else { 0 } | if manu.is_some() { 0x04 } else { 0 };
            let mut aps = vec![0x40, EP];
            aps.extend_from_slice(&cluster.to_le_bytes());
            aps.extend_from_slice(&0x0104u16.to_le_bytes());
            aps.push(0x01);
            aps.push(nx(&mut c.st.aps_ctr));
            aps.push(fc);
            if let Some(m) = manu {
                aps.extend_from_slice(&m.to_le_bytes());
            }
            aps.extend_from_slice(&[seq, cmd]);
            aps.extend_from_slice(payload);
            c.nwk_send_data(&mut self.s.w.air, short, &aps);
            let got = |s: &Sim| s.w.coord.zcl_rsp[n0..].iter().find(|r| r.src == short && r.cluster == cluster && r.cmd == Some(want)).map(|r| r.body.clone());
            self.s.run_until(2000.0, 20.0, |s| got(s).is_some())?;
            if let Some(b) = got(&self.s) {
                return Ok(Some(b));
            }
            let t = self.now();
            self.retries.push((t, cluster, cmd));
        }
        Ok(None)
    }

    /// An attribute value as bytes (without the type), or None.
    pub(super) fn read(&mut self, short: u16, cluster: u16, attr: u16, manu: Option<u16>) -> R<Option<Vec<u8>>> {
        let b = self.zcl(short, cluster, 0x00, &attr.to_le_bytes(), manu, false)?;
        let Some(b) = b else { return Ok(None) };
        if b.len() < 4 || b[2] != 0 {
            return Ok(None);
        }
        let (t, v) = (b[3], &b[4..]);
        if t == 0x41 || t == 0x42 {
            let n = v.first().copied().unwrap_or(0) as usize;
            return Ok(Some(v.get(1..(1 + n).min(v.len())).unwrap_or(&[]).to_vec()));
        }
        Ok(Some(v.to_vec()))
    }

    pub(super) fn write(&mut self, short: u16, cluster: u16, attr: u16, dtype: u8, value: &[u8], manu: Option<u16>) -> R<bool> {
        let mut p = attr.to_le_bytes().to_vec();
        p.push(dtype);
        p.extend_from_slice(value);
        let b = self.zcl(short, cluster, 0x02, &p, manu, false)?;
        Ok(b.is_some_and(|b| b.first() == Some(&0x00)))
    }

    /// Attribute values from reports from `short` after `since`.
    pub(super) fn reports(&self, short: u16, cluster: u16, attr: u16, since: f64) -> Vec<(f64, i64)> {
        let mut out = Vec::new();
        for r in &self.s.w.coord.zcl_rsp {
            if r.ms < since || r.src != short || r.cluster != cluster || r.cmd != Some(0x0A) {
                continue;
            }
            let b = &r.body;
            let mut i = 0;
            while i + 3 <= b.len() {
                let aid = u16::from_le_bytes([b[i], b[i + 1]]);
                let t = b[i + 2];
                let n = match t {
                    0x18 | 0x20 | 0x30 | 0x10 => 1,
                    0x19 | 0x21 => 2,
                    _ => break,
                };
                if aid == attr {
                    let mut v: i64 = 0;
                    for (k, x) in b.get(i + 3..(i + 3 + n).min(b.len())).unwrap_or(&[]).iter().enumerate() {
                        v |= (*x as i64) << (8 * k);
                    }
                    out.push((r.ms, v));
                }
                i += 3 + n;
            }
        }
        out
    }

    pub(super) fn line_sens(&mut self, short: u16) -> R<Vec<(u16, u8, u8)>> {
        let b = self.read(short, CL_SENSE, A_LSENS, Some(MANU))?.unwrap_or_default();
        Ok((0..b.len() / 4 * 4).step_by(4).filter(|i| i + 4 <= b.len()).map(|i| (u16::from_le_bytes([b[i], b[i + 1]]), b[i + 2], b[i + 3])).collect())
    }

    pub(super) fn finish(&mut self, cx: &mut SimCtx) {
        let n_ok = self.results.iter().filter(|r| r.1).count();
        let wall = cx.t_wall.elapsed().as_secs_f64();
        cx.say(&format!(
            "\n== result: {n_ok}/{} steps passed; simulation time {:.1} s, wall time {wall:.0} s",
            self.results.len(),
            self.now() / 1000.0
        ));
        for (name, ok, d) in self.results.clone() {
            cx.say(&format!("  {}  {name}  {d}", if ok { "PASS" } else { "FAIL" }));
        }
        cx.results = self.results.clone();
    }
}

pub(super) fn sens_text(v: &[(u16, u8, u8)]) -> String {
    py::list(v.iter().map(|(a, b, c)| py::tuple([a.to_string(), b.to_string(), c.to_string()])))
}
