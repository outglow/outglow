use std::cell::RefCell;
use std::rc::Rc;

use super::rust_fw::{host_duty_table, syms, Bulb, CONFIG_SECTORS, OP_OFF, OP_POWER_ON, POB_PREVIOUS};
use super::{SimCtx, SimDef};
use crate::session::{Stop, R};

pub fn def() -> SimDef {
    SimDef { name: "power_cut", doc: "the Rust firmware's light state across power cuts in its flash writes and sector switches", run }
}

const SECTOR: usize = 0x1000;

#[derive(Clone, Copy, Debug)]
enum Cut {
    Before,
    After,
    Bytes(usize),
    Random(u64),
    Half,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Op {
    Erase,
    Append,
    FirstRecord,
    Header,
}

impl Op {
    fn matches(self, op: &str, addr: usize, n: usize) -> bool {
        let off = addr % SECTOR;
        match self {
            Op::Erase => op == "erase",
            Op::Append => op == "program" && off >= 0x20,
            Op::FirstRecord => op == "program" && ((off == 0x10 && n == 16) || (off == 0 && n == 8)),
            Op::Header => op == "program" && off == 0 && n == 16,
        }
    }
}

struct Case {
    name: &'static str,
    op: Op,
    cut: Cut,
}

#[derive(Default)]
struct Tap {
    shadow: Vec<u8>,
    armed: Option<(Op, Cut)>,
    fired: Option<(Vec<u8>, String)>,
}

fn rng(seed: &mut u64) -> u8 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed as u8
}

fn install(b: &mut Bulb, tap: &Rc<RefCell<Tap>>) {
    {
        let mut t = tap.borrow_mut();
        let m = &b.node.m.flash.mem;
        t.shadow = CONFIG_SECTORS.iter().flat_map(|s| m[*s..*s + SECTOR].to_vec()).collect();
    }
    let tap = tap.clone();
    b.node.m.on_flash_write = Some(Rc::new(move |m: &mut tc32emu::Machine, op: &str, addr: usize, n: usize| {
        let Some(si) = CONFIG_SECTORS.iter().position(|s| *s == addr & !(SECTOR - 1)) else { return };
        let base = CONFIG_SECTORS[si];
        let mut t = tap.borrow_mut();
        if let (None, Some((want, cut))) = (&t.fired, t.armed) {
            if want.matches(op, addr, n) {
                let mut img = m.flash.mem.clone();
                let (from, len) = if op == "erase" { (base, SECTOR) } else { (addr, n) };
                let mut seed = match cut {
                    Cut::Random(s) => s | 1,
                    _ => 1,
                };
                for i in 0..len {
                    let pre = t.shadow[si * SECTOR + from - base + i];
                    let post = img[from + i];
                    let undone = match cut {
                        Cut::Before => 0xFF,
                        Cut::After => 0,
                        Cut::Bytes(k) => if i < k { 0 } else { 0xFF },
                        Cut::Random(_) => rng(&mut seed),
                        Cut::Half => if i < len / 2 { 0 } else { 0xFF },
                    };
                    img[from + i] = post ^ ((pre ^ post) & undone);
                }
                t.fired = Some((img, format!("{op} {addr:#07x} len {n}, cut {cut:?}")));
            }
        }
        t.shadow[si * SECTOR..(si + 1) * SECTOR].copy_from_slice(&m.flash.mem[base..base + SECTOR]);
    }));
}

fn light(b: &Bulb) -> (u8, u8, u32) {
    let s = b.st();
    (s.on, s.level, crate::session::light_of(&b.node).cmp)
}

struct Run<'a> {
    cx: &'a mut SimCtx,
    b: Bulb,
    tap: Rc<RefCell<Tap>>,
    tb: std::collections::BTreeMap<u32, u32>,
    k: u32,
    cur: (u8, u8),
}

impl Run<'_> {
    fn check(&mut self, name: &str, ok: bool, detail: &str) -> bool {
        self.b.ev.borrow_mut().clear();
        self.cx.results.push((name.to_string(), ok, detail.to_string()));
        let d = if detail.is_empty() { String::new() } else { format!("  {detail}") };
        self.cx.say(&format!("  {}  {name}{d}", if ok { "PASS" } else { "FAIL" }));
        ok
    }

    fn duty_of(&self, on: u8, level: u8) -> u32 {
        if on == 0 {
            0
        } else {
            self.tb.get(&(level as u32)).copied().unwrap_or(u32::MAX)
        }
    }

    fn power_on(&mut self, flash: &[u8]) -> R<()> {
        self.b.node = Bulb::power_on(flash, &self.b.ev, 0)?;
        self.b.seq = 0;
        install(&mut self.b, &self.tap);
        self.b.boot(400.0)?;
        let s = self.b.st();
        self.cur = (s.on, s.level);
        Ok(())
    }

    fn change(&mut self, may_off: bool) -> R<bool> {
        self.k += 1;
        let ok = if may_off && self.k % 7 == 0 {
            self.b.cmd1(&[OP_OFF])?
        } else {
            let mut level = 10 + (self.k * 37 % 231) as u8;
            if level == self.cur.1 {
                level += 1;
            }
            self.b.mv(level, 0)?
        };
        let s = self.b.st();
        self.cur = (s.on, s.level);
        Ok(ok)
    }

    fn case(&mut self, c: &Case, max: usize) -> R<Option<String>> {
        if self.cur == (1, 254) {
            self.change(false)?;
        }
        self.tap.borrow_mut().armed = Some((c.op, c.cut));
        self.tap.borrow_mut().fired = None;
        let mut n = 0;
        let (before, after) = loop {
            let before = self.cur;
            self.change(false)?;
            n += 1;
            if self.tap.borrow().fired.is_some() {
                break (before, self.cur);
            }
            if n >= max {
                self.tap.borrow_mut().armed = None;
                return Ok(None);
            }
        };
        let (img, what) = {
            let mut t = self.tap.borrow_mut();
            t.armed = None;
            t.fired.take().unwrap()
        };
        self.power_on(&img)?;
        let (on, level, cmp) = light(&self.b);
        let is = |s: (u8, u8)| (on, level) == s && cmp == self.duty_of(s.0, s.1);
        let verdict = if is(after) {
            "the state being stored"
        } else if is(before) {
            "the state before it"
        } else if (on, level) == (1, 254) {
            "THE DEFAULTS"
        } else {
            "ANOTHER STATE"
        };
        let ok = is(after) || is(before);
        let detail = format!(
            "after {n} change(s): {what}; came back on={on} level={level} duty={cmp}: {verdict} (before {before:?}, being stored {after:?})"
        );
        self.check(c.name, ok, &detail);
        Ok(Some(detail))
    }
}

fn legacy_record(level: u8) -> [u8; 8] {
    let mut r = [0x45, 0x02, 0x83, 0xFF, level, 185, 0xFF, 0];
    let mut c = 0u8;
    for &x in &r[..7] {
        c ^= x;
        for _ in 0..8 {
            c = if c & 0x80 != 0 { (c << 1) ^ 0x07 } else { c << 1 };
        }
    }
    r[7] = c;
    r
}

fn run(cx: &mut SimCtx) -> R<()> {
    let elf_img = cx.image("elf", "target/tc32/bulb-tc32.elf");
    let bin_img = cx.image("bin", "target/tc32/bulb-tc32.bin");
    for p in [&elf_img.path, &bin_img.path] {
        if !p.exists() {
            return Err(Stop::Error(format!("{} not found: build the firmware with `cargo xtask build`", p.display())));
        }
    }
    let image = bin_img.bytes().map_err(Stop::Error)?;
    let elf = std::fs::read(&elf_img.path).map_err(|e| Stop::Error(e.to_string()))?;
    let (addr, _) = syms(&elf).map_err(Stop::Error)?;
    let a = |n: &str| addr.iter().find(|(k, _)| k == n).map(|(_, v)| *v as usize).unwrap_or(0);
    let tb = host_duty_table().map_err(Stop::Error)?;
    let ev = Rc::new(RefCell::new(Vec::new()));
    let node = Bulb::power_on(&image, &ev, a("FW_OFFSET") as i64)?;
    let b = Bulb { node, cmd: a("BULB_CMD"), state: a("BULB_STATE"), net_block: a("BULB_NET"), seq: 0, ev, bitten: None, expect_wd: false, tagged: true };
    let tap = Rc::new(RefCell::new(Tap::default()));
    let mut r = Run { cx, b, tap, tb, k: 0, cur: (0, 0) };
    install(&mut r.b, &r.tap);
    r.b.boot(400.0)?;
    r.cx.say(&format!("image {}", bin_img.path.display()));
    let ok = r.b.cmd1(&[OP_POWER_ON, POB_PREVIOUS])?;
    let s = r.b.st();
    r.cur = (s.on, s.level);
    r.check("boots; power-on behaviour set to 'previous'", ok && s.pob == POB_PREVIOUS, "");
    r.change(false)?;

    r.cx.say("cuts in an ordinary change");
    let cases = [
        Case { name: "a record half programmed (the first half of its bytes)", op: Op::Append, cut: Cut::Half },
        Case { name: "a record with random bits programmed", op: Op::Append, cut: Cut::Random(0x1234_5678) },
        Case { name: "a record with 3 of its bytes programmed", op: Op::Append, cut: Cut::Bytes(3) },
        Case { name: "right after a record", op: Op::Append, cut: Cut::After },
    ];
    let mut lost = 0;
    for c in &cases {
        match r.case(c, 4)? {
            Some(d) if d.contains("THE DEFAULTS") || d.contains("ANOTHER") => lost += 1,
            Some(_) => {}
            None => {
                r.check(c.name, false, "the operation never came");
                lost += 1;
            }
        }
    }

    r.cx.say("cuts in a sector switch (the log is filled up to it with brightness changes)");
    let cases = [
        Case { name: "just before the sector erase", op: Op::Erase, cut: Cut::Before },
        Case { name: "during the erase: half of the sector erased", op: Op::Erase, cut: Cut::Half },
        Case { name: "during the erase: random bits erased", op: Op::Erase, cut: Cut::Random(0xC0FFEE) },
        Case { name: "between the erase and the first record", op: Op::Erase, cut: Cut::After },
        Case { name: "the first record of the new sector half programmed", op: Op::FirstRecord, cut: Cut::Bytes(4) },
        Case { name: "the new sector's header with random bits programmed", op: Op::Header, cut: Cut::Random(0xBEEF) },
        Case { name: "after the first record, before the header", op: Op::Header, cut: Cut::Before },
        Case { name: "right after the header", op: Op::Header, cut: Cut::After },
    ];
    for c in &cases {
        match r.case(c, 1100)? {
            Some(d) if d.contains("THE DEFAULTS") || d.contains("ANOTHER") => lost += 1,
            Some(_) => {}
            None => {
                r.check(c.name, false, "the operation never came: this image has no such write");
                lost += 1;
            }
        }
    }

    r.cx.say("afterwards");
    for _ in 0..600 {
        r.change(true)?;
    }
    let last = r.cur;
    let flash = r.b.node.m.flash.mem.clone();
    r.power_on(&flash)?;
    let (on, level, cmp) = light(&r.b);
    let ok = (on, level) == last && cmp == r.duty_of(last.0, last.1);
    r.check("600 more changes (more switches), then a power cycle: the last state", ok,
            &format!("on={on} level={level} duty={cmp}, last {last:?}"));
    r.check("no cut brought the bulb back with the defaults or another state", lost == 0, &format!("{lost} case(s) lost the state"));

    r.cx.say("update from the firmware with the single-sector log");
    migrate(&mut r, &image, a("FW_OFFSET"))?;
    let n = r.cx.results.len();
    let ok = r.cx.results.iter().filter(|x| x.1).count();
    r.cx.say(&format!("\n{ok}/{n} checks passed"));
    Ok(())
}

fn migrate(r: &mut Run, image: &[u8], fw_offset: usize) -> R<()> {
    let from = r.cx.image("from", "-");
    let from_elf = r.cx.image("from_elf", "-");
    let mut flash;
    let last;
    if from.path.exists() && from_elf.path.exists() {
        let old = from.bytes().map_err(Stop::Error)?;
        let elf = std::fs::read(&from_elf.path).map_err(|e| Stop::Error(e.to_string()))?;
        let (addr, _) = syms(&elf).map_err(Stop::Error)?;
        let a = |n: &str| addr.iter().find(|(k, _)| k == n).map(|(_, v)| *v as usize).unwrap_or(0);
        let node = Bulb::power_on(&old, &r.b.ev, a("FW_OFFSET") as i64)?;
        let mut ob = Bulb { node, cmd: a("BULB_CMD"), state: a("BULB_STATE"), net_block: a("BULB_NET"), seq: 0, ev: r.b.ev.clone(), bitten: None, expect_wd: false, tagged: false };
        ob.boot(400.0)?;
        ob.cmd1(&[OP_POWER_ON, POB_PREVIOUS])?;
        for k in 0..40u32 {
            ob.mv(20 + (k * 53 % 200) as u8, 0)?;
        }
        let s = ob.st();
        last = (s.on, s.level);
        flash = ob.node.m.flash.mem.clone();
        r.cx.say(&format!("  the earlier firmware ({}) stored 40 changes, the last {last:?}", from.path.display()));
    } else {
        flash = vec![0xFF; 1 << 20];
        for (k, level) in [30u8, 90, 150, 77].iter().enumerate() {
            flash[CONFIG_SECTORS[0] + k * 8..CONFIG_SECTORS[0] + k * 8 + 8].copy_from_slice(&legacy_record(*level));
        }
        last = (1, 77);
        r.cx.say("  records in the earlier firmware's format in 0xF0000, the last: on at level 77");
    }
    flash[..image.len()].copy_from_slice(image);
    flash[fw_offset..fw_offset + image.len()].copy_from_slice(image);
    r.power_on(&flash)?;
    let (on, level, cmp) = light(&r.b);
    r.check("the first power-on after the update: the earlier firmware's last state",
            (on, level) == last && cmp == r.duty_of(last.0, last.1), &format!("on={on} level={level} duty={cmp}"));
    let cases = [
        Case { name: "update, first store: its record half programmed", op: Op::FirstRecord, cut: Cut::Half },
        Case { name: "update, first store: before the header", op: Op::Header, cut: Cut::Before },
        Case { name: "update, first store: right after the header", op: Op::Header, cut: Cut::After },
    ];
    for c in &cases {
        if r.case(c, 3)?.is_none() {
            r.check(c.name, false, "the operation never came: this image has no such write");
        }
    }
    let flash = r.b.node.m.flash.mem.clone();
    let moved = super::rust_fw::config_log(&flash).is_some_and(|(base, recs)| base == CONFIG_SECTORS[1] && !recs.is_empty())
        && flash[CONFIG_SECTORS[0]..CONFIG_SECTORS[0] + SECTOR].iter().any(|x| *x != 0xFF);
    r.check("the log is now in 0xF4000, the earlier one left in 0xF0000 until the next switch", moved, "");
    Ok(())
}
