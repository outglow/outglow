#![forbid(unsafe_code)]
#![no_std]
#[cfg(test)]
use memcap as _;

pub mod probe;

pub const LINES: usize = 10;
pub const HIST: usize = 5;
pub const WIN: usize = 16;
pub const MIN_GAP_MS: u32 = 80;
pub const RESET_GAP_MS: u32 = 4000;
pub const MIN_RSSI: i8 = -90;
pub const BLANK_MS: u32 = 1500;
pub const EVICT_MS: u32 = 60_000;
pub const NO_SRC: u16 = 0xFFFF;
pub const DEV_MAX: i16 = 63;

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
pub const CAL_MIN_FRAMES: u16 = 40;
pub const TUNE_STEP: u8 = 5;
pub const TUNE_LEVELS: u8 = 2;
pub const HOUR_MS: u32 = 3_600_000;
pub const HOUR_MIN_FRAMES: u16 = 1000;
pub const DAY_MIN_HOURS: u8 = 12;

pub const F_ON: u8 = 0x01;
pub const F_RULE: u8 = 0x02;
pub const F_CHG: u8 = 0x04;

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
    pub src: u16,
    pub flags: u8,
    pub nhist: u8,
    pub hpos: u8,
    pub peer_chg: u8,
    pub hist: [i8; HIST],
    pub rssi: i8,
    pub sum: u8,
    pub noise: u8,
    pub s: u8,
    pub anchor_s: u8,
    pub anchor_noise: u16,
    pub dpos: u8,
    pub dev: [u8; WIN],
    pub cal_s: u8,
    pub cal_n: u16,
    pub cal_sum: u32,
    pub hour_cnt: u16,
    pub hour_sum: u32,
    pub day_cnt: u32,
    pub day_sum: u32,
    pub day_min: u16,
    pub day_hours: u8,
    pub hour_cal_s: u8,
    pub day_cal_s: u8,
    pub t_kept: u32,
    pub t_seen: u32,
    pub t_rule: u32,
    pub t_hit: u32,
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

    pub fn level(&self) -> u8 {
        self.dev.iter().map(|&d| d as u16).sum::<u16>().min(255) as u8
    }

    fn win_sum(&self, thr_q4: u8) -> u8 {
        let mut sum = 0u8;
        for &d in &self.dev {
            let wq = (d as u16 * 4) / thr_q4 as u16;
            sum += wq.min(3) as u8;
        }
        sum
    }

    fn quiet_s(&self, from: u8) -> u8 {
        (2..=from)
            .rev()
            .find(|&s| {
                let (thr, m) = params(s);
                (self.win_sum(thr) as u16 + CAL_HEADROOM as u16) < m as u16
            })
            .unwrap_or(1)
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
        self.hour_cal_s = 100;
        self.day_cal_s = 0;
        self.set_s(s, 0);
        self.restart();
    }

    fn on_if_due(&mut self, now: u32) {
        if now.wrapping_sub(self.t_rule) >= (100 - self.s as u32) * 10 {
            self.flags |= F_ON;
        }
    }
}

fn median5(h: &[i8; HIST]) -> i8 {
    let mut v = *h;
    for i in 1..HIST {
        for j in (1..=i).rev() {
            if v[j - 1] <= v[j] {
                break;
            }
            v.swap(j - 1, j);
        }
    }
    v[HIST / 2]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Core {
    pub s: u8,
    pub off_s: u16,
    pub selftune: bool,
    pub auto_cal: bool,
    pub dirty: bool,
    pub cal: bool,
    pub hour: u8,
    pub on_mask: u16,
    pub t_cal_end: u32,
    pub t_hour: u32,
    pub line: [Line; LINES],
}

impl Core {
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

    pub fn start(&mut self, now: u32) {
        self.cal = false;
        self.t_cal_end = now;
        self.hour = 0;
        self.t_hour = now;
        self.on_mask = 0;
        for l in self.line.iter_mut() {
            l.day_reset();
            l.hour_cal_s = 100;
            l.day_cal_s = 0;
            l.flags &= !F_ON;
            l.blank = false;
            l.restart();
            l.t_seen = now;
            l.t_kept = now.wrapping_sub(MIN_GAP_MS);
        }
    }

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
            if l.flags & F_CHG != 0 && chg != l.peer_chg {
                l.blank = true;
                l.t_blank = now.wrapping_add(BLANK_MS);
                l.restart();
            }
            l.peer_chg = chg;
            l.flags |= F_CHG;
        }
        if l.blank {
            if (1..=BLANK_MS).contains(&l.t_blank.wrapping_sub(now)) {
                return;
            }
            l.blank = false;
        }
        if rssi < MIN_RSSI {
            return;
        }
        if now.wrapping_sub(l.t_kept) < MIN_GAP_MS {
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
                l.cal_n = l.cal_n.wrapping_add(1);
                l.cal_sum = l.cal_sum.wrapping_add(dev as u32);
                l.cal_s = l.quiet_s(l.cal_s);
            }
            if auto_cal {
                l.hour_cal_s = l.quiet_s(l.hour_cal_s);
            }
            if nothing_on && l.flags & F_ON == 0 && l.hour_cnt < 0xFFFF {
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

    pub fn blank(&mut self, now: u32) {
        for l in self.line.iter_mut().filter(|l| l.src != NO_SRC) {
            l.blank = true;
            l.t_blank = now.wrapping_add(BLANK_MS);
            l.restart();
        }
    }

    pub fn cal_start(&mut self, now: u32, seconds: u16) {
        self.cal = seconds != 0;
        self.t_cal_end = now.wrapping_add(seconds as u32 * 1000);
        for l in self.line.iter_mut() {
            l.cal_s = 100;
            l.cal_n = 0;
            l.cal_sum = 0;
        }
    }

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
            l.set_s(l.cal_s.saturating_sub(CAL_MARGIN).max(1), if noise != 0 { noise } else { 1 });
            l.flags &= !F_RULE;
            self.dirty = true;
        }
    }

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
                l.day_min = l.day_min.min(mean);
                l.day_sum = l.day_sum.wrapping_add(l.hour_sum);
                l.day_cnt = l.day_cnt.wrapping_add(l.hour_cnt as u32);
                l.day_hours += 1;
                if auto_cal {
                    l.day_cal_s = l.day_cal_s.max(l.hour_cal_s);
                }
            }
            l.hour_cnt = 0;
            l.hour_sum = 0;
            l.hour_cal_s = 100;
            if !day_end {
                continue;
            }
            if auto_cal && l.anchor_noise == 0 && l.day_hours >= DAY_MIN_HOURS && l.day_cal_s != 0 && l.s != 0 {
                l.set_s(l.day_cal_s.saturating_sub(CAL_MARGIN).max(1), l.day_min.max(1));
                l.flags &= !F_RULE;
                dirty = true;
            } else if l.day_hours >= DAY_MIN_HOURS && l.s != 0 {
                let day_mean = ((l.day_sum.wrapping_mul(32)) / l.day_cnt) as u16;
                let span = TUNE_STEP * TUNE_LEVELS;
                let lo = if l.anchor_s > span { l.anchor_s - span } else { 1 };
                let hi = (l.anchor_s + span).min(100);
                if l.anchor_noise == 0 {
                    l.anchor_noise = if day_mean != 0 { day_mean } else { 1 };
                    dirty = true;
                } else if selftune {
                    if l.day_min as u32 > 2 * l.anchor_noise as u32 && l.s > lo {
                        l.s = l.s.saturating_sub(TUNE_STEP).max(lo);
                        dirty = true;
                    } else if (day_mean as u32) * 2 < l.anchor_noise as u32 && l.s < hi {
                        l.s = (l.s + TUNE_STEP).min(hi);
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
                l.restart();
            }
            if l.flags & F_RULE != 0 {
                l.t_hit = now;
                l.on_if_due(now);
            }
            if l.flags & F_RULE == 0 && now.wrapping_sub(l.t_hit) >= off_ms {
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

#[cfg(test)]
mod tests {
    use super::{env_u8, Line, WIN};

    #[test]
    fn the_level_of_a_line_is_the_sum_of_its_window_up_to_255() {
        let mut l = Line::EMPTY;
        assert_eq!(l.level(), 0);
        l.dev[0] = 3;
        l.dev[WIN - 1] = 4;
        assert_eq!(l.level(), 7);
        l.dev = [15; WIN];
        assert_eq!(l.level(), 240);
        l.dev = [16; WIN];
        assert_eq!(l.level(), 255);
        l.dev = [63; WIN];
        assert_eq!(l.level(), 255);
    }

    #[test]
    fn a_build_setting_is_a_decimal_byte() {
        assert_eq!(env_u8(None, 4), 4);
        assert_eq!(env_u8(Some("7"), 4), 7);
        assert_eq!(env_u8(Some("042"), 4), 42);
        assert_eq!(env_u8(Some("255"), 4), 255);
        assert_eq!(env_u8(Some("190"), 4), 190);
    }

    #[test]
    #[should_panic(expected = "a number 0..=255")]
    fn an_empty_build_setting_is_refused() {
        env_u8(Some(""), 4);
    }

    #[test]
    #[should_panic(expected = "a number 0..=255")]
    fn a_build_setting_of_four_digits_is_refused() {
        env_u8(Some("0001"), 4);
    }

    #[test]
    #[should_panic(expected = "a number 0..=255")]
    fn a_build_setting_above_255_is_refused() {
        env_u8(Some("256"), 4);
    }

    #[test]
    #[should_panic(expected = "a number 0..=255")]
    fn a_build_setting_with_a_letter_is_refused() {
        env_u8(Some("1x"), 4);
    }
}
