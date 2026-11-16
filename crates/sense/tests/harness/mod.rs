#![allow(dead_code)]

use super::model::{Channel, Rng};
use sense::Core;

pub struct Sender {
    pub src: u16,
    pub ch: Channel,
    pub chg: u8,
    pub probe_ms: u32,
    next_probe: u32,
    pub frames_per_s: f64,
}

impl Sender {
    pub fn new(src: u16, ch: Channel) -> Sender {
        Sender { src, ch, chg: 0, probe_ms: 200, next_probe: 0, frames_per_s: 0.0 }
    }
}

pub struct Room {
    pub core: Core,
    pub t: u32,
    pub g: Rng,
    pub senders: Vec<Sender>,
    pub log: Vec<(u32, u16)>,
    pub mask: u16,
    pub step: u32,
}

impl Room {
    pub fn new(seed: u64, s: u8, off_s: u16, senders: Vec<Sender>) -> Room {
        let t = 1000;
        let mut r = Room { core: Core::new(t, s, off_s), t, g: Rng(seed), senders, log: vec![], mask: 0, step: 10 };
        for (i, sd) in r.senders.iter_mut().enumerate() {
            sd.next_probe = t + i as u32 * 7;
        }
        r
    }

    pub fn run(&mut self, ms: u32) {
        let end = self.t + ms;
        while self.t < end {
            for i in 0..self.senders.len() {
                let t = self.t;
                let sd = &mut self.senders[i];
                if t >= sd.next_probe {
                    if let Some(r) = sd.ch.rssi(&mut self.g, t) {
                        self.core.frame(t, sd.src, r, Some(sd.chg));
                    }
                    let p = sd.probe_ms;
                    sd.next_probe = t + p - p / 8 + self.g.below((p / 4 + 1) as u64) as u32;
                }
                if sd.frames_per_s > 0.0 && self.g.unit() < sd.frames_per_s * self.step as f64 / 1000.0 {
                    if let Some(r) = sd.ch.rssi(&mut self.g, t) {
                        self.core.frame(t, sd.src, r, None);
                    }
                }
            }
            self.tick();
            self.t += self.step;
        }
    }

    pub fn tick(&mut self) -> u16 {
        let m = self.core.tick(self.t);
        if m != self.mask {
            self.log.push((self.t, m));
            self.mask = m;
        }
        m
    }

    pub fn calibrate(&mut self, seconds: u16) {
        self.core.cal_start(self.t, seconds);
        self.run(seconds as u32 * 1000 + 100);
    }

    pub fn ons(&self) -> Vec<u32> {
        let mut prev = 0u16;
        let mut out = vec![];
        for &(t, m) in &self.log {
            if m & !prev != 0 {
                out.push(t);
            }
            prev = m;
        }
        out
    }

    pub fn clear_log(&mut self) {
        self.log.clear();
        self.mask = self.core.tick(self.t);
    }
}
