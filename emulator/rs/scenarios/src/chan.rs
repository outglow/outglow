//! A seeded fault model of the 802.15.4 air (`air::Channel`): per-link RSSI
//! and frame loss (asymmetric links allowed), interference bursts that
//! corrupt frames in time windows (the receiver sees a bad FCS), and
//! collisions at the scripted receivers (two frames overlapping where both
//! arrive: neither is received; a node does not receive while it sends).
//! Emulated bulbs model collisions in their own radio (radio154).
//!
//! Every random draw comes from one Python `random.Random(seed)` stream
//! (`ble::pyrandom`), in the order frames go on the air, so the same seed and
//! the same images give the same run. Nothing here is on unless a scenario
//! installs it (`ChanRef::install`).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use ::ble::pyrandom::PyRandom;

use crate::air::{Air, AirFrame, Channel, Fate, NodeId};

/// Python's `random.random()` on top of the MT19937 stream.
pub struct Rng(PyRandom);

impl Rng {
    pub fn new(seed: u32) -> Rng {
        Rng(PyRandom::new(seed))
    }
    pub fn random(&mut self) -> f64 {
        let a = (self.0.genrand() >> 5) as f64;
        let b = (self.0.genrand() >> 6) as f64;
        (a * 67108864.0 + b) * (1.0 / 9007199254740992.0)
    }
    /// An integer in [0, n).
    pub fn below(&mut self, n: u32) -> u32 {
        self.0.randrange(n.max(1))
    }
}

/// One direction of a link.
#[derive(Clone, Copy, Debug)]
pub struct Link {
    pub rssi: i64,
    /// probability that a frame on this link is lost (on top of `base_loss`)
    pub loss: f64,
}

/// An interference window: frames overlapping it are corrupted with
/// probability `p` at the receivers in `rx` (None: everyone).
#[derive(Clone, Debug)]
pub struct Burst {
    pub t0: f64,
    pub t1: f64,
    pub p: f64,
    pub rx: Option<Vec<NodeId>>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RxStats {
    pub ok: u64,
    pub lost: u64,
    pub corrupt: u64,
    pub collided: u64,
}

/// What a receiver got of a frame: energy and data, energy only (corrupt or collided).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Got {
    Ok,
    Corrupt,
    Collided,
}

pub struct ChanModel {
    pub rng: Rng,
    pub links: BTreeMap<(NodeId, NodeId), Link>,
    /// loss probability of every frame at every receiver
    pub base_loss: f64,
    pub bursts: Vec<Burst>,
    /// model collisions at scripted receivers
    pub collisions: bool,
    /// per frame index: the receivers that got energy, and what they got
    rec: Vec<Vec<(NodeId, Got)>>,
    pub stats: BTreeMap<NodeId, RxStats>,
}

impl ChanModel {
    pub fn new(seed: u32) -> ChanModel {
        ChanModel {
            rng: Rng::new(seed),
            links: BTreeMap::new(),
            base_loss: 0.0,
            bursts: Vec::new(),
            collisions: true,
            rec: Vec::new(),
            stats: BTreeMap::new(),
        }
    }

    /// Whether `rx` received frame `idx` intact (as far as the air is concerned;
    /// an emulated bulb's radio may still drop it: collision, receiver off).
    pub fn got(&self, idx: usize, rx: NodeId) -> bool {
        self.rec.get(idx).is_some_and(|v| v.iter().any(|(n, g)| *n == rx && *g == Got::Ok))
    }

    /// What `rx` got of frame `idx` on the air: "ok", "corrupt", "collided"
    /// (a scripted receiver), None: nothing (lost, out of range, off).
    pub fn what(&self, idx: usize, rx: NodeId) -> Option<&'static str> {
        let g = self.rec.get(idx)?.iter().find(|(n, _)| *n == rx)?.1;
        Some(match g {
            Got::Ok => "ok",
            Got::Corrupt => "corrupt",
            Got::Collided => "collided",
        })
    }

    fn slot(&mut self, idx: usize) -> &mut Vec<(NodeId, Got)> {
        if self.rec.len() <= idx {
            self.rec.resize_with(idx + 1, Vec::new);
        }
        &mut self.rec[idx]
    }

    fn fate(&mut self, idx: usize, sender: NodeId, rx: NodeId, t0: f64, t1: f64, rssi: i64) -> Fate {
        let link = self.links.get(&(sender, rx)).copied();
        let rssi = link.map_or(rssi, |l| l.rssi);
        let p_loss = 1.0 - (1.0 - self.base_loss) * (1.0 - link.map_or(0.0, |l| l.loss));
        if p_loss > 0.0 && self.rng.random() < p_loss {
            self.stats.entry(rx).or_default().lost += 1;
            return Fate::Lost;
        }
        let mut corrupt = false;
        for i in 0..self.bursts.len() {
            let b = &self.bursts[i];
            if b.t0 < t1 && t0 < b.t1 && b.rx.as_ref().map_or(true, |v| v.contains(&rx)) {
                let p = b.p;
                if self.rng.random() < p {
                    corrupt = true;
                    break;
                }
            }
        }
        if corrupt {
            self.stats.entry(rx).or_default().corrupt += 1;
            self.slot(idx).push((rx, Got::Corrupt));
            return Fate::Corrupt(rssi);
        }
        self.slot(idx).push((rx, Got::Ok));
        Fate::Ok(rssi)
    }

    fn heard(&mut self, idx: usize, rx: NodeId, frames: &[AirFrame]) -> bool {
        let me = &frames[idx];
        let mut clash = false;
        if self.collisions {
            let lo = idx.saturating_sub(40);
            let hi = (idx + 40).min(frames.len());
            for j in lo..hi {
                if j == idx {
                    continue;
                }
                let f = &frames[j];
                if !(f.t0 < me.t1 && me.t0 < f.t1) {
                    continue;
                }
                // half duplex, or another frame's energy at this receiver
                if f.who == rx || self.rec.get(j).is_some_and(|v| v.iter().any(|(n, _)| *n == rx)) {
                    clash = true;
                    break;
                }
            }
        }
        let st = self.stats.entry(rx).or_default();
        if clash {
            st.collided += 1;
        } else {
            st.ok += 1;
        }
        if clash {
            if let Some(e) = self.slot(idx).iter_mut().find(|(n, _)| *n == rx) {
                e.1 = Got::Collided;
            }
        }
        !clash
    }
}

/// The shared handle a scenario keeps; `install` puts it on the air.
#[derive(Clone)]
pub struct ChanRef(pub Rc<RefCell<ChanModel>>);

impl ChanRef {
    pub fn new(seed: u32) -> ChanRef {
        ChanRef(Rc::new(RefCell::new(ChanModel::new(seed))))
    }
    pub fn install(&self, air: &mut Air) {
        air.channel = Some(Box::new(self.clone()));
    }
    pub fn m(&self) -> std::cell::RefMut<'_, ChanModel> {
        self.0.borrow_mut()
    }
    pub fn got(&self, idx: usize, rx: NodeId) -> bool {
        self.0.borrow().got(idx, rx)
    }
    pub fn what(&self, idx: usize, rx: NodeId) -> Option<&'static str> {
        self.0.borrow().what(idx, rx)
    }
}

impl Channel for ChanRef {
    fn fate(&mut self, idx: usize, sender: NodeId, rx: NodeId, t0: f64, t1: f64, _psdu: &[u8], rssi: i64) -> Fate {
        self.0.borrow_mut().fate(idx, sender, rx, t0, t1, rssi)
    }
    fn heard(&mut self, idx: usize, rx: NodeId, frames: &[AirFrame]) -> bool {
        self.0.borrow_mut().heard(idx, rx, frames)
    }
}
