//! Motion sensing from the signal strength (RSSI) of frames between bulbs
//! (device-free sensing, as Philips Hue MotionAware): the detector core and
//! the probe frame. Portable, `no_std`, integer arithmetic, no allocation.
//!
//! The algorithm is the C firmware's (`firmware/sense/eglo_sense_core.c`,
//! reconstructed from the README of the Ripple project; `docs/rf-sensing.md`),
//! ported line by line; a comparison on random input feeds both the same
//! frames and finds their state the same after every step.
//!
//! ```text
//!   line      = one sender heard by this receiver (up to LINES)
//!   sample    = RSSI of a kept frame; at most 12.5 samples/s per line
//!   reference = median of the 5 previous samples
//!   weight    = min(|x - median| / threshold, 3)
//!   rule      = sum of the weights of the last 16 samples >= M continuously for
//!               D = 10 ms * (100 - s)  ->  line ON
//!   OFF       = off_s seconds after the last moment at which the rule was met
//!   s (0..100) selects the threshold and M (`params`); 0 = detection disabled.
//!               Every line has its own s.
//!
//!   calibration  = a recording of the empty room: for every line the most sensitive s at
//!                  which the M-of-16 rule was never met, minus a margin
//!   self-tuning  = once a day s by one level (5), at most 2 levels from the
//!                  anchor (s from calibration or set manually), based on the
//!                  line noise measured only when nothing detects motion
//!   automatic    = (not in the C firmware; `Core::auto_cal`) the calibration's
//!   calibration    search every hour; after a full day a line nobody
//!                  calibrated takes the s of its quietest hour (the highest
//!                  s that hour left), and that hour's noise as its anchor
//! ```
#![forbid(unsafe_code)]
#![no_std]

pub mod probe;

pub const LINES: usize = 10;
/// Reference samples (median).
pub const HIST: usize = 5;
/// Window of the M-of-N rule.
pub const WIN: usize = 16;
/// 12.5 kept samples/s.
pub const MIN_GAP_MS: u32 = 80;
/// A longer gap restarts the line's detection.
pub const RESET_GAP_MS: u32 = 4000;
/// Weaker frames do not count.
pub const MIN_RSSI: i8 = -90;
/// After a light change (own or the sender's).
pub const BLANK_MS: u32 = 1500;
/// A line without frames for this long may yield to a new sender.
pub const EVICT_MS: u32 = 60_000;
pub const NO_SRC: u16 = 0xFFFF;
/// Deviation in dB, clipped.
pub const DEV_MAX: i16 = 63;

/// Calibration margin: a short recording does not see rare noise
/// coincidences, so we look for the s at which the window sum did not even
/// reach M - CAL_HEADROOM, and subtract a further CAL_MARGIN units of s.
/// Values tuned on the synthetic channel model of the C firmware
/// (emulator/sense_core_test.py: 60 s recording, then an hour without a
/// false ON); `crates/sense/tests/measured.rs` checks them on the channel
/// measured between the owner's bulbs. As in the C core they can be set at
/// build time (environment `SENSE_CAL_HEADROOM`, `SENSE_CAL_MARGIN`), to
/// try others (`tests/explore.rs`).
pub const CAL_HEADROOM: u8 = env_u8(option_env!("SENSE_CAL_HEADROOM"), 4);
pub const CAL_MARGIN: u8 = env_u8(option_env!("SENSE_CAL_MARGIN"), 0);

const fn env_u8(v: Option<&str>, default: u8) -> u8 {
    let Some(s) = v else {
        return default;
    };
    let b = s.as_bytes();
    assert!(!b.is_empty() && b.len() <= 3, "a number 0..=255");
    let mut n = 0u32;
    let mut i = 0;
    while i < b.len() {
        assert!(b[i].is_ascii_digit(), "a number 0..=255");
        n = n * 10 + (b[i] - b'0') as u32;
        i += 1;
    }
    assert!(n <= 255, "a number 0..=255");
    n as u8
}
/// Fewer samples in the recording: the line stays unchanged.
pub const CAL_MIN_FRAMES: u16 = 40;
/// One self-tuning level.
pub const TUNE_STEP: u8 = 5;
/// At most this many levels from the anchor.
pub const TUNE_LEVELS: u8 = 2;
pub const HOUR_MS: u32 = 3_600_000;
/// An hour with fewer samples does not count.
pub const HOUR_MIN_FRAMES: u16 = 1000;
/// A day with fewer valid hours does not count.
pub const DAY_MIN_HOURS: u8 = 12;

/// Line detects motion.
pub const F_ON: u8 = 0x01;
/// Rule met at the last sample.
pub const F_RULE: u8 = 0x02;
/// We know the sender's light change counter.
pub const F_CHG: u8 = 0x04;

/// Threshold (in 1/4 dB) and M for a sensitivity: s 100..50: threshold
/// 1 dB, M grows 1..8; s 49..1: M = 8, threshold 1.25..10 dB.
pub fn params(s: u8) -> (u8, u8) {
    let s = s.min(100) as u32;
    if s >= 50 {
        (4, (1 + ((100 - s) * 7 + 25) / 50) as u8)
    } else {
        ((5 + ((49 - s) * 35 + 24) / 48) as u8, 8)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Line {
    /// Sender's short address; `NO_SRC` = free.
    pub src: u16,
    pub flags: u8,
    /// Valid samples in `hist` (0..=HIST).
    pub nhist: u8,
    pub hpos: u8,
    /// Sender's light change counter from the last probe.
    pub peer_chg: u8,
    pub hist: [i8; HIST],
    /// Last kept sample.
    pub rssi: i8,
    /// Sum of the window weights (diagnostics).
    pub sum: u8,
    /// Smoothed |x - median|, 1/8 dB (diagnostics).
    pub noise: u8,
    /// Sensitivity of this line.
    pub s: u8,
    /// Self-tuning anchor.
    pub anchor_s: u8,
    /// Noise of the anchor day, 1/32 dB; 0 = not known yet.
    pub anchor_noise: u16,
    pub dpos: u8,
    /// Deviations of the last 16 samples [dB].
    pub dev: [u8; WIN],
    /// Calibration: the most sensitive s that has not triggered so far.
    pub cal_s: u8,
    /// Calibration: number of samples and sum of their deviations.
    pub cal_n: u16,
    pub cal_sum: u32,
    /// Self-tuning: samples of the current hour (only when nothing is ON)...
    pub hour_cnt: u16,
    pub hour_sum: u32,
    /// ...and of the valid hours of the current day.
    pub day_cnt: u32,
    pub day_sum: u32,
    /// Quietest hour of the day, 1/32 dB.
    pub day_min: u16,
    pub day_hours: u8,
    /// Automatic calibration: the calibration's search over this hour (all
    /// samples, motion or not), and the highest of the day's hours (the
    /// quietest hour). Its noise is the self-tuning's `day_min`.
    pub hour_cal_s: u8,
    pub day_cal_s: u8,
    /// Time of the last kept sample.
    pub t_kept: u32,
    /// Time of the last frame from the sender.
    pub t_seen: u32,
    /// Since when the rule has been met.
    pub t_rule: u32,
    /// When the rule was last met.
    pub t_hit: u32,
    /// End of blanking; valid when `blank`.
    pub t_blank: u32,
    pub blank: bool,
}

impl Line {
    pub const EMPTY: Line = Line {
        src: NO_SRC,
        flags: 0,
        nhist: 0,
        hpos: 0,
        peer_chg: 0,
        hist: [0; HIST],
        rssi: 0,
        sum: 0,
        noise: 0,
        s: 0,
        anchor_s: 0,
        anchor_noise: 0,
        dpos: 0,
        dev: [0; WIN],
        cal_s: 100,
        cal_n: 0,
        cal_sum: 0,
        hour_cnt: 0,
        hour_sum: 0,
        day_cnt: 0,
        day_sum: 0,
        day_min: 0xFFFF,
        day_hours: 0,
        hour_cal_s: 100,
        day_cal_s: 0,
        t_kept: 0,
        t_seen: 0,
        t_rule: 0,
        t_hit: 0,
        t_blank: 0,
        blank: false,
    };

    pub fn used(&self) -> bool {
        self.src != NO_SRC
    }

    pub fn on(&self) -> bool {
        self.flags & F_ON != 0
    }

    /// Sum of the window weights for threshold `thr_q4` [1/4 dB].
    fn win_sum(&self, thr_q4: u8) -> u8 {
        let mut sum = 0u8;
        for &d in &self.dev {
            let wq = (d as u16 * 4) / thr_q4 as u16;
            sum += if wq > 3 { 3 } else { wq as u8 };
        }
        sum
    }

    fn restart(&mut self) {
        self.nhist = 0;
        self.hpos = 0;
        self.dpos = 0;
        self.dev = [0; WIN];
        self.sum = 0;
        self.flags &= !F_RULE;
    }

    fn day_reset(&mut self) {
        self.hour_cnt = 0;
        self.hour_sum = 0;
        self.day_cnt = 0;
        self.day_sum = 0;
        self.day_min = 0xFFFF;
        self.day_hours = 0;
    }

    fn set_s(&mut self, s: u8, anchor_noise: u16) {
        self.s = s;
        self.anchor_s = s;
        self.anchor_noise = anchor_noise;
        self.day_reset();
    }

    fn free(&mut self, s: u8) {
        self.src = NO_SRC;
        self.flags = 0;
        self.blank = false;
        self.noise = 0;
        self.rssi = 0;
        self.cal_s = 100;
        self.cal_n = 0;
        self.cal_sum = 0;
        self.set_s(s, 0);
        self.restart();
    }

    fn on_if_due(&mut self, now: u32) {
        if self.flags & F_RULE != 0 && now.wrapping_sub(self.t_rule) >= (100 - self.s as u32) * 10 {
            self.flags |= F_ON;
        }
    }
}

fn median5(h: &[i8; HIST]) -> i8 {
    let mut v = [0i8; HIST];
    for i in 0..HIST {
        let x = h[i];
        let mut j = i;
        while j > 0 && v[j - 1] > x {
            v[j] = v[j - 1];
            j -= 1;
        }
        v[j] = x;
    }
    v[HIST / 2]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Core {
    /// Default sensitivity (new lines).
    pub s: u8,
    /// Seconds until OFF.
    pub off_s: u16,
    /// Self-tuning enabled.
    pub selftune: bool,
    /// Automatic calibration from the quietest hour of the first full day
    /// (lines without an anchor noise: never calibrated, no anchor day yet).
    /// Off here, as in the C firmware; the stack turns it on.
    pub auto_cal: bool,
    /// The s/anchor of some line changed: save to NV and clear.
    pub dirty: bool,
    /// Calibration in progress.
    pub cal: bool,
    /// Hour of the current self-tuning day.
    pub hour: u8,
    /// Mask of the ON lines from the last tick.
    pub on_mask: u16,
    pub t_cal_end: u32,
    /// Start of the current hour.
    pub t_hour: u32,
    pub line: [Line; LINES],
}

impl Core {
    /// Every line free with sensitivity `s` (what `sense_core_init` makes of a
    /// zeroed `sense_t`). A constant expression, so that it is built where it
    /// goes: inside the stack, which is built in its place, not copied.
    pub const fn new(now: u32, s: u8, off_s: u16) -> Core {
        let s = if s > 100 { 100 } else { s };
        Core {
            s,
            off_s: if off_s != 0 { off_s } else { 1 },
            selftune: false,
            auto_cal: false,
            dirty: false,
            cal: false,
            hour: 0,
            on_mask: 0,
            t_cal_end: now,
            t_hour: now,
            line: [Line { s, anchor_s: s, ..Line::EMPTY }; LINES],
        }
    }

    /// Changing the default s sets it (and the anchor) in all lines.
    pub fn cfg(&mut self, s: u8, off_s: u16) {
        let s = s.min(100);
        self.off_s = if off_s != 0 { off_s } else { 1 };
        if s != self.s {
            self.s = s;
            for l in self.line.iter_mut() {
                l.set_s(s, 0);
                l.flags &= !F_RULE;
            }
        }
    }

    fn find(&self, src: u16) -> Option<usize> {
        self.line.iter().position(|l| l.src == src)
    }

    /// A free line, or the longest-silent one (over EVICT_MS) that is not ON.
    fn alloc(&mut self, now: u32, src: u16) -> Option<usize> {
        let mut best = None;
        let mut best_age = EVICT_MS;
        for (i, l) in self.line.iter().enumerate() {
            if l.src == NO_SRC {
                best = Some(i);
                break;
            }
            let age = now.wrapping_sub(l.t_seen);
            if l.flags & F_ON == 0 && age > best_age {
                best_age = age;
                best = Some(i);
            }
        }
        let i = best?;
        if self.line[i].src != NO_SRC {
            // a remembered line gave way
            self.dirty = true;
        }
        let s = self.s;
        let l = &mut self.line[i];
        l.free(s);
        l.src = src;
        l.t_seen = now;
        l.t_kept = now.wrapping_sub(MIN_GAP_MS);
        Some(i)
    }

    /// A line remembered in NV; false when there is no room (or it is there already).
    pub fn line_restore(&mut self, now: u32, src: u16, s: u8, anchor_s: u8, anchor_noise: u16) -> bool {
        if src == NO_SRC || self.find(src).is_some() {
            return false;
        }
        let Some(i) = self.alloc(now, src) else {
            return false;
        };
        let l = &mut self.line[i];
        l.s = s.min(100);
        l.anchor_s = anchor_s.min(100);
        l.anchor_noise = anchor_noise;
        true
    }

    /// A frame from `src` at level `rssi` [dBm]. `probe`: it was a probe
    /// (which may create the line), carrying the sender's light change counter.
    pub fn frame(&mut self, now: u32, src: u16, rssi: i8, probe: Option<u8>) {
        let i = match self.find(src) {
            Some(i) => i,
            None => {
                if probe.is_none() || rssi < MIN_RSSI {
                    return;
                }
                match self.alloc(now, src) {
                    Some(i) => i,
                    None => return,
                }
            }
        };
        let (cal, nothing_on, auto_cal) = (self.cal, self.on_mask == 0, self.auto_cal);
        let l = &mut self.line[i];
        l.t_seen = now;

        if let Some(chg) = probe {
            // the sender switched or dimmed its light: that is not motion
            if l.flags & F_CHG != 0 && chg != l.peer_chg {
                l.blank = true;
                l.t_blank = now.wrapping_add(BLANK_MS);
                l.restart();
            }
            l.peer_chg = chg;
            l.flags |= F_CHG;
        }
        if l.blank {
            if (now.wrapping_sub(l.t_blank) as i32) < 0 {
                return;
            }
            l.blank = false;
        }
        if rssi < MIN_RSSI {
            return;
        }
        if now.wrapping_sub(l.t_kept) < MIN_GAP_MS {
            // at most 12.5 samples/s
            return;
        }
        if now.wrapping_sub(l.t_kept) > RESET_GAP_MS {
            l.restart();
        }
        l.t_kept = now;
        l.rssi = rssi;

        if l.nhist as usize == HIST {
            let d = rssi as i16 - median5(&l.hist) as i16;
            let dev = d.abs().min(DEV_MAX) as u8;
            l.dev[l.dpos as usize] = dev;
            l.dpos = ((l.dpos as usize + 1) % WIN) as u8;
            let target: i16 = if dev > 31 { 255 } else { dev as i16 * 8 };
            l.noise = (l.noise as i16 + ((target - l.noise as i16) >> 3)) as u8;

            if cal {
                // the most sensitive s at which the window sum never reached M - margin; the sum
                // decreases with the threshold and M does not decrease, so it is enough to step
                // down as the recording goes
                l.cal_n = l.cal_n.wrapping_add(1);
                l.cal_sum = l.cal_sum.wrapping_add(dev as u32);
                while l.cal_s > 1 {
                    let (thr, m) = params(l.cal_s);
                    if (l.win_sum(thr) as u16 + CAL_HEADROOM as u16) < m as u16 {
                        break;
                    }
                    l.cal_s -= 1;
                }
            }
            if auto_cal {
                // the same search over the hour, whatever happens in it: the quietest hour of
                // the day is the one that leaves the highest s
                while l.hour_cal_s > 1 {
                    let (thr, m) = params(l.hour_cal_s);
                    if (l.win_sum(thr) as u16 + CAL_HEADROOM as u16) < m as u16 {
                        break;
                    }
                    l.hour_cal_s -= 1;
                }
            }
            if nothing_on && l.flags & F_ON == 0 && l.hour_cnt < 0xFFFF {
                // background noise: only when nothing detects motion
                l.hour_cnt += 1;
                l.hour_sum = l.hour_sum.wrapping_add(dev as u32);
            }

            if l.s != 0 {
                let (thr, m) = params(l.s);
                l.sum = l.win_sum(thr);
                if l.sum >= m {
                    if l.flags & F_RULE == 0 {
                        l.flags |= F_RULE;
                        l.t_rule = now;
                    }
                    l.t_hit = now;
                    l.on_if_due(now);
                } else {
                    l.flags &= !F_RULE;
                }
            } else {
                l.sum = 0;
                l.flags &= !F_RULE;
            }
        }
        l.hist[l.hpos as usize] = rssi;
        l.hpos = ((l.hpos as usize + 1) % HIST) as u8;
        if (l.nhist as usize) < HIST {
            l.nhist += 1;
        }
    }

    /// Own light changed: blank all lines for BLANK_MS.
    pub fn blank(&mut self, now: u32) {
        for l in self.line.iter_mut().filter(|l| l.src != NO_SRC) {
            l.blank = true;
            l.t_blank = now.wrapping_add(BLANK_MS);
            l.restart();
        }
    }

    /// Calibration: record the empty room for `seconds` (0 = abort without changes).
    pub fn cal_start(&mut self, now: u32, seconds: u16) {
        self.cal = seconds != 0;
        self.t_cal_end = now.wrapping_add(seconds as u32 * 1000);
        for l in self.line.iter_mut() {
            l.cal_s = 100;
            l.cal_n = 0;
            l.cal_sum = 0;
        }
    }

    /// Seconds until the end of the calibration; 0 = not running.
    pub fn cal_left(&self, now: u32) -> u16 {
        let left = self.t_cal_end.wrapping_sub(now) as i32;
        if !self.cal || left <= 0 {
            return 0;
        }
        ((left + 999) / 1000) as u16
    }

    fn cal_finish(&mut self) {
        self.cal = false;
        for l in self.line.iter_mut() {
            if l.src == NO_SRC || l.cal_n < CAL_MIN_FRAMES {
                continue;
            }
            let noise = ((l.cal_sum.wrapping_mul(32)) / l.cal_n as u32) as u16;
            let s = if l.cal_s > CAL_MARGIN { l.cal_s - CAL_MARGIN } else { 1 };
            l.set_s(s, if noise != 0 { noise } else { 1 });
            l.flags &= !F_RULE;
            self.dirty = true;
        }
    }

    /// End of the hour: close the statistics; every 24 hours move s by at most one level.
    fn hour_finish(&mut self) {
        self.hour += 1;
        let day_end = self.hour >= 24;
        if day_end {
            self.hour = 0;
        }
        let (selftune, auto_cal) = (self.selftune, self.auto_cal);
        let mut dirty = false;
        for l in self.line.iter_mut() {
            if l.src == NO_SRC {
                continue;
            }
            if l.hour_cnt >= HOUR_MIN_FRAMES {
                let mean = ((l.hour_sum.wrapping_mul(32)) / l.hour_cnt as u32) as u16;
                if mean < l.day_min {
                    l.day_min = mean;
                }
                l.day_sum = l.day_sum.wrapping_add(l.hour_sum);
                l.day_cnt = l.day_cnt.wrapping_add(l.hour_cnt as u32);
                l.day_hours += 1;
                // an hour counts for the automatic calibration as for the self-tuning
                if auto_cal && l.hour_cal_s > l.day_cal_s {
                    l.day_cal_s = l.hour_cal_s;
                }
            }
            l.hour_cnt = 0;
            l.hour_sum = 0;
            l.hour_cal_s = 100;
            if !day_end {
                continue;
            }
            if auto_cal && l.anchor_noise == 0 && l.day_hours >= DAY_MIN_HOURS && l.day_cal_s != 0 && l.s != 0 {
                // calibrated by its quietest hour: that s, and the quietest hour's noise as the anchor
                let s = if l.day_cal_s > CAL_MARGIN { l.day_cal_s - CAL_MARGIN } else { 1 };
                l.set_s(s, l.day_min.max(1));
                l.flags &= !F_RULE;
                dirty = true;
            } else if l.day_hours >= DAY_MIN_HOURS && l.s != 0 {
                let day_mean = ((l.day_sum.wrapping_mul(32)) / l.day_cnt) as u16;
                let span = TUNE_STEP * TUNE_LEVELS;
                let lo = if l.anchor_s > span { l.anchor_s - span } else { 1 };
                let hi = if l.anchor_s < 100 - span { l.anchor_s + span } else { 100 };
                if l.anchor_noise == 0 {
                    // first full day = the anchor day
                    l.anchor_noise = if day_mean != 0 { day_mean } else { 1 };
                    dirty = true;
                } else if selftune {
                    if l.day_min as u32 > 2 * l.anchor_noise as u32 && l.s > lo {
                        // even the quietest hour is more than 2x louder than on the anchor day
                        l.s = if l.s - lo > TUNE_STEP { l.s - TUNE_STEP } else { lo };
                        dirty = true;
                    } else if (day_mean as u32) * 2 < l.anchor_noise as u32 && l.s < hi {
                        // the whole day more than 2x quieter
                        l.s = if hi - l.s > TUNE_STEP { l.s + TUNE_STEP } else { hi };
                        dirty = true;
                    }
                }
            }
            l.day_cnt = 0;
            l.day_sum = 0;
            l.day_min = 0xFFFF;
            l.day_hours = 0;
            l.day_cal_s = 0;
        }
        if dirty {
            self.dirty = true;
        }
    }

    /// Passage of time (OFF, delay D, end of calibration, self-tuning);
    /// returns the mask of the ON lines.
    pub fn tick(&mut self, now: u32) -> u16 {
        if self.cal && (now.wrapping_sub(self.t_cal_end) as i32) >= 0 {
            self.cal_finish();
        }
        if now.wrapping_sub(self.t_hour) >= HOUR_MS {
            self.t_hour = self.t_hour.wrapping_add(HOUR_MS);
            self.hour_finish();
        }
        let mut on = 0u16;
        let off_ms = self.off_s as u32 * 1000;
        for (i, l) in self.line.iter_mut().enumerate() {
            if l.src == NO_SRC {
                continue;
            }
            if l.flags & F_RULE != 0 && now.wrapping_sub(l.t_kept) > RESET_GAP_MS {
                // the sender went silent midway
                l.restart();
            }
            if l.flags & F_RULE != 0 {
                l.t_hit = now;
                l.on_if_due(now);
            }
            if l.flags & F_ON != 0 && l.flags & F_RULE == 0 && now.wrapping_sub(l.t_hit) >= off_ms {
                l.flags &= !F_ON;
            }
            if l.flags & F_ON != 0 {
                on |= 1 << i;
            }
        }
        self.on_mask = on;
        on
    }
}
