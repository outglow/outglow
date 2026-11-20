use std::cell::RefCell;
use std::collections::BTreeMap;
use std::process::Command;
use std::rc::Rc;

use zbnode::Node;

use super::{SimCtx, SimDef};
use crate::runner::repo_root;
use crate::session::{Stop, R};

pub fn def() -> SimDef {
    SimDef { name: "rust_fw", doc: "the Rust firmware alone: PWM, debug-link commands, flash, power cycles, watchdog", run }
}

const SRAM: usize = 0x840000;
pub const CONFIG_SECTORS: [usize; 2] = [0xF0000, 0xF4000];
const SECTOR_SIZE: usize = 0x1000;
pub const SLOT: usize = 16;
const NV_SECTORS: [usize; 2] = [0xF1000, 0xF2000];
const MAC_SECTOR: usize = 0xFF000;
const RESET_SECTOR: usize = 0xCF000;
const OP_HANG: u8 = 0xEE;
const PWM_PERIOD: u32 = 48000;
const CPI: i64 = 24;
const FACTORY_RESET_BOOT_MS: f64 = 1000.0;

pub const OP_ON: u8 = 1;
pub const OP_OFF: u8 = 2;
const OP_TOGGLE: u8 = 3;
const OP_MOVE: u8 = 4;
const OP_STARTUP: u8 = 5;
pub const OP_POWER_ON: u8 = 6;
const POB_OFF: u8 = 0;
const POB_ON: u8 = 1;
const POB_TOGGLE: u8 = 2;
pub const POB_PREVIOUS: u8 = 0xFF;

pub fn syms(elf: &[u8]) -> Result<(Vec<(String, u32)>, BTreeMap<String, u32>), String> {
    let u16_at = |o: usize| u16::from_le_bytes([elf[o], elf[o + 1]]) as usize;
    let u32_at = |o: usize| u32::from_le_bytes(elf[o..o + 4].try_into().unwrap());
    if elf.len() < 0x34 || &elf[..4] != b"\x7fELF" {
        return Err("not an ELF file".into());
    }
    let (shoff, shentsize, shnum) = (u32_at(0x20) as usize, u16_at(0x2E), u16_at(0x30));
    let sh = |i: usize| shoff + i * shentsize;
    let Some(st) = (0..shnum).find(|i| u32_at(sh(*i) + 4) == 2) else { return Err("no .symtab (build with symbols)".into()) };
    let (off, size, link, ent) = (u32_at(sh(st) + 0x10) as usize, u32_at(sh(st) + 0x14) as usize, u32_at(sh(st) + 0x18) as usize,
                                  u32_at(sh(st) + 0x24) as usize);
    let stroff = u32_at(sh(link) + 0x10) as usize;
    let mut addr: Vec<(String, u32)> = Vec::new();
    let mut sizes = BTreeMap::new();
    for k in 0..size / ent.max(1) {
        let e = off + k * ent;
        let name_off = stroff + u32_at(e) as usize;
        let end = elf[name_off..].iter().position(|b| *b == 0).unwrap_or(0);
        let name = String::from_utf8_lossy(&elf[name_off..name_off + end]).to_string();
        if name.is_empty() {
            continue;
        }
        let (v, sz, info) = (u32_at(e + 4), u32_at(e + 8), elf[e + 12]);
        let (ty, bind) = (info & 0xF, info >> 4);
        if ty == 2 || (v != 0 && (bind == 0 || bind == 1)) {
            match addr.iter_mut().find(|(n, _)| *n == name) {
                Some(x) => x.1 = v & !1,
                None => addr.push((name.clone(), v & !1)),
            }
            if sz != 0 {
                sizes.insert(name, sz);
            }
        }
    }
    Ok((addr, sizes))
}

pub fn host_duty_table() -> Result<BTreeMap<u32, u32>, String> {
    let out = Command::new("cargo")
        .args(["run", "-q", "-p", "bulb-sim", "--example", "duty_table", &PWM_PERIOD.to_string()])
        .current_dir(repo_root())
        .output()
        .map_err(|e| format!("cargo run bulb-sim: {e}"))?;
    if !out.status.success() {
        return Err(format!("bulb-sim duty_table: {}", String::from_utf8_lossy(&out.stderr)));
    }
    let mut t = BTreeMap::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut f = line.split_whitespace();
        if let (Some(a), Some(b)) = (f.next(), f.next()) {
            t.insert(a.parse().unwrap_or(0), b.parse().unwrap_or(0));
        }
    }
    Ok(t)
}

fn llvm_dir() -> String {
    std::env::var("TC32_LLVM").unwrap_or_else(|_| format!("{}/oss/llvm-tc32/build/bin", std::env::var("HOME").unwrap_or_default()))
}

fn irq_path_calls(elf: &str, rstored: u32) -> Option<(usize, Vec<String>)> {
    let out = Command::new(format!("{}/llvm-objdump", llvm_dir()))
        .args(["-d", "--mcpu=tc32", "--no-show-raw-insn", "--disassemble-symbols=irq_handler", elf])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() || !text.contains("<irq_handler>:") {
        return None;
    }
    let mut targets = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() >= 3 && ["bl", "blx", "tjl"].contains(&f[1]) {
            if let Ok(t) = u32::from_str_radix(f[2].trim_start_matches('#').trim_start_matches("0x"), 16) {
                targets.push(t);
            }
        }
    }
    let outside = targets.iter().filter(|t| **t >= rstored).map(|t| format!("'{t:#x}'")).collect();
    Some((targets.len(), outside))
}

struct Out<'a> {
    cx: &'a mut SimCtx,
    ev: Rc<RefCell<Vec<String>>>,
}

impl Out<'_> {
    fn flush(&mut self) {
        let lines: Vec<String> = self.ev.borrow_mut().drain(..).collect();
        for l in lines {
            self.cx.say(&l);
        }
    }
    fn say(&mut self, s: &str) {
        self.flush();
        self.cx.say(s);
    }
    fn check(&mut self, name: &str, ok: bool, detail: &str) -> bool {
        self.flush();
        self.cx.results.push((name.to_string(), ok, detail.to_string()));
        let d = if detail.is_empty() { String::new() } else { format!("  {detail}") };
        self.cx.say(&format!("  {}  {name}{d}", if ok { "PASS" } else { "FAIL" }));
        ok
    }
}

fn tool_check(o: &mut Out, name: &str, script: &str, args: &[&str]) {
    let root = std::env::var("TC32_DEVTOOLS").unwrap_or_else(|_| format!("{}/oss/tc32-devtools", std::env::var("HOME").unwrap_or_default()));
    if !std::path::Path::new(&format!("{root}/compiler/{script}")).exists() {
        o.say(&format!("  (not checked: {name}; no tc32-devtools in {root}, set TC32_DEVTOOLS)"));
        return;
    }
    let llvm = llvm_dir();
    let r = Command::new("python3")
        .arg(format!("{root}/compiler/{script}"))
        .args(args)
        .env("TC32_LLVM", &llvm)
        .env("TC32_LLD", &llvm)
        .output();
    let (ok, text, code) = match r {
        Ok(o) => (o.status.success(), format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)), o.status.code().unwrap_or(-1)),
        Err(e) => (false, e.to_string(), -1),
    };
    let tail: Vec<&str> = text.trim().lines().collect();
    let last = tail.last().map(|s| s.to_string()).unwrap_or_else(|| format!("exit {code}"));
    o.check(name, ok, &last);
    if !ok {
        let from = tail.len().saturating_sub(15);
        o.say(&tail[from..].iter().map(|l| format!("      {l}")).collect::<Vec<_>>().join("\n"));
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct St {
    done: u8,
    pub on: u8,
    pub level: u8,
    pub pob: u8,
    startup: u8,
    reports: u8,
    rejected: u8,
}

impl St {
    fn py(&self) -> String {
        format!(
            "{{'done': {}, 'on': {}, 'level': {}, 'pob': {}, 'startup': {}, 'reports': {}, 'rejected': {}}}",
            self.done, self.on, self.level, self.pob, self.startup, self.reports, self.rejected
        )
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
struct Net {
    state: u8,
    channel: u8,
    reset_step: u8,
    factory: u8,
    warm: u8,
}

impl Net {
    fn py(&self) -> String {
        format!(
            "{{'state': {}, 'channel': {}, 'reset_step': {}, 'factory': {}, 'warm': {}}}",
            self.state, self.channel, self.reset_step, self.factory, self.warm
        )
    }
}

pub struct Bulb {
    pub node: Node,
    pub cmd: usize,
    pub state: usize,
    pub net_block: usize,
    pub seq: u8,
    pub ev: Rc<RefCell<Vec<String>>>,
    pub bitten: Option<f64>,
    pub expect_wd: bool,
}

fn emu(e: String) -> Stop {
    Stop::Error(e)
}

impl Bulb {
    pub fn power_on(flash: &[u8], ev: &Rc<RefCell<Vec<String>>>, link_shift: i64) -> R<Node> {
        let mut node = Node::new(flash, CPI, link_shift).map_err(emu)?;
        let e = ev.clone();
        node.m.event = Some(Rc::new(move |m: &mut tc32emu::Machine, text: &str| {
            e.borrow_mut().push(format!("[{:9.3} ms] {text}", m.ms()));
        }));
        ev.borrow_mut().push(format!("[{:9.3} ms] reset #{}: boot slot 0x{:05x}", 0.0, node.m.reset_count, node.m.boot_slot));
        Ok(node)
    }

    fn cycle(&mut self) -> R<()> {
        let flash = self.node.m.flash.mem.clone();
        self.node = Bulb::power_on(&flash, &self.ev, 0)?;
        self.seq = 0;
        Ok(())
    }

    pub fn ms(&self) -> f64 {
        self.node.ms()
    }

    pub fn run(&mut self, ms: f64) -> R<()> {
        let target = self.node.m.ms() + ms;
        while self.node.m.ms() < target {
            let r = self.node.m.run_ms(target, &[]);
            self.node.radio.b().tx_pending.clear();
            match r {
                Ok(()) | Err(tc32emu::Error::Stop(_)) => {}
                Err(tc32emu::Error::WatchdogReset) if self.expect_wd => {
                    self.bitten = Some(self.node.m.ms());
                    return self.node.m.reset(-1, true).map_err(|e| Stop::Error(e.to_string()));
                }
                Err(e) => return Err(Stop::Error(format!("{e} pc={:#x}", self.node.m.r[15]))),
            }
        }
        Ok(())
    }

    pub fn boot(&mut self, timeout_ms: f64) -> R<f64> {
        let start = self.ms();
        while self.st().reports == 0 {
            if self.ms() - start >= timeout_ms {
                return Err(Stop::Error(format!(
                    "the bulb did not reach its main loop within {timeout_ms:.0} ms of power-on (no light report, pc={:#x})",
                    self.node.m.r[15]
                )));
            }
            self.run(1.0)?;
        }
        let took = self.ms() - start;
        self.run(1.0)?;
        Ok(took)
    }

    fn sram(&self, a: usize, n: usize) -> Vec<u8> {
        self.node.m.sram[a - SRAM..a - SRAM + n].to_vec()
    }

    pub fn st(&self) -> St {
        let b = self.sram(self.state, 8);
        St { done: b[0], on: b[1], level: b[2], pob: b[3], startup: b[4], reports: b[5], rejected: b[6] }
    }

    fn net(&self) -> Net {
        let b = self.sram(self.net_block, 8);
        Net { state: b[0], channel: b[1], reset_step: b[4], factory: b[5], warm: b[6] }
    }

    fn session(&mut self, seconds: f64) -> R<Net> {
        self.cycle()?;
        self.boot(400.0)?;
        let rest = seconds * 1000.0 - self.ms();
        self.run(rest)?;
        Ok(self.net())
    }

    pub fn send(&mut self, payload: &[u8], timeout_ms: f64) -> R<bool> {
        let mut p = payload.to_vec();
        p.resize(7, 0);
        self.seq = self.seq.wrapping_add(1);
        let o = self.cmd - SRAM;
        self.node.m.sram[o + 1..o + 8].copy_from_slice(&p);
        self.node.m.sram[o] = self.seq;
        let before = self.st();
        let end = self.ms() + timeout_ms;
        while self.ms() < end {
            self.run(0.25)?;
            if self.bitten.is_some() {
                return Ok(false);
            }
            let s = self.st();
            if s.done == self.seq && (s.reports, s.rejected) != (before.reports, before.rejected) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn cmd1(&mut self, payload: &[u8]) -> R<bool> {
        self.send(payload, 200.0)
    }

    pub fn mv(&mut self, level: u8, transition_ms: u32) -> R<bool> {
        let mut p = vec![OP_MOVE, level];
        p.extend_from_slice(&transition_ms.to_le_bytes());
        self.send(&p, 200.0)
    }

    fn pc1_channel(&self) -> Option<u8> {
        let g = &self.node.m.regs;
        if (g[0x586 + 16] >> 1) & 1 != 0 {
            return None;
        }
        let mux = (g[0x5A8 + 4] >> 2) & 3;
        if mux == 2 {
            Some(0)
        } else {
            None
        }
    }

    fn pwm_py(&self) -> (bool, Option<u8>, u32, u32, String) {
        let g = &self.node.m.regs;
        let en = g[0x781] & 1 != 0;
        let cmp = u16::from_le_bytes([g[0x794], g[0x795]]) as u32;
        let max = u16::from_le_bytes([g[0x796], g[0x797]]) as u32;
        let ch = self.pc1_channel();
        let s = format!(
            "{{'enabled': {}, 'channel': {}, 'cmp': {cmp}, 'max': {max}}}",
            if en { "True" } else { "False" },
            ch.map(|c| c.to_string()).unwrap_or_else(|| "None".into())
        );
        (en, ch, cmp, max, s)
    }

    pub fn duty(&self) -> u32 {
        self.pwm_py().2
    }

    fn records(&self) -> Vec<Vec<u8>> {
        config_log(&self.node.m.flash.mem).map(|(_, r)| r).unwrap_or_default()
    }
}

fn config_slot(flash: &[u8], at: usize) -> Option<&[u8]> {
    let (a, b) = flash[at..at + SLOT].split_at(SLOT / 2);
    a.iter().zip(b).all(|(x, y)| *x == !*y).then_some(a)
}

pub fn config_log(flash: &[u8]) -> Option<(usize, Vec<Vec<u8>>)> {
    let gen = |base: usize| config_slot(flash, base).filter(|h| h[..4] == [0xA5, b'L', b'T', 1] && h[5..] == [0, 0, 0x5A]).map(|h| h[4]);
    let base = match (gen(CONFIG_SECTORS[0]), gen(CONFIG_SECTORS[1])) {
        (Some(a), Some(b)) => CONFIG_SECTORS[(b.wrapping_sub(a) as i8 > 0) as usize],
        (Some(_), None) => CONFIG_SECTORS[0],
        (None, Some(_)) => CONFIG_SECTORS[1],
        (None, None) => return None,
    };
    let recs = (SLOT..SECTOR_SIZE).step_by(SLOT).filter_map(|off| config_slot(flash, base + off)).map(|r| r.to_vec()).collect();
    Some((base, recs))
}

fn nv_items(flash: &[u8]) -> BTreeMap<u8, Vec<u8>> {
    let mut best: Option<(usize, u8)> = None;
    for base in NV_SECTORS {
        let h = &flash[base..base + 4];
        if h[0] == 0xA5 && h[3] == 0x5A && h[1] == h[2] ^ 0xFF && best.map_or(true, |b| h[1].wrapping_sub(b.1) < 0x80) {
            best = Some((base, h[1]));
        }
    }
    let mut items = BTreeMap::new();
    if let Some((base, _)) = best {
        let mut off = base + 4;
        while off + 3 <= base + SECTOR_SIZE && flash[off] != 0xFF {
            let n = flash[off + 1] as usize;
            items.insert(flash[off], flash[off + 2..off + 2 + n].to_vec());
            off += 3 + n;
        }
    }
    items
}

fn hx(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
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
    let (addr, size) = syms(&elf).map_err(Stop::Error)?;
    let a = |n: &str| addr.iter().find(|(k, _)| k == n).map(|(_, v)| *v as usize).unwrap_or(0);
    let table = host_duty_table().map_err(Stop::Error)?;
    let tb = |l: u8| table.get(&(l as u32)).copied().unwrap_or(0);
    let ev = Rc::new(RefCell::new(Vec::new()));
    let mut o = Out { cx, ev: ev.clone() };

    o.say("image");
    let le32 = |i: usize| u32::from_le_bytes(image[i..i + 4].try_into().unwrap());
    let ram_copy = (le32(0x0C) & 0xFFFF) as usize * 16;
    let rstored = a("_rstored_");
    o.check(
        "header: KNLT, RAM copy covers .ram_code, size field = file size",
        &image[8..12] == b"KNLT" && ram_copy >= rstored && le32(0x18) as usize == image.len(),
        &format!("copy={ram_copy:#x} ram_code_end={rstored:#x} size={}", image.len()),
    );
    let canon = |p: &std::path::Path| std::fs::canonicalize(p).unwrap_or(p.to_path_buf()).display().to_string();
    let elf_s = canon(&elf_img.path);
    let bin_s = canon(&bin_img.path);
    tool_check(&mut o, "forms_check.py: only instruction forms the vendor code uses", "forms_check.py", &["check", &elf_s]);
    let _ = &bin_s;

    o.say("first power-on (blank configuration sector)");
    let node = Bulb::power_on(&image, &ev, a("FW_OFFSET") as i64)?;
    let mut b = Bulb { node, cmd: a("BULB_CMD"), state: a("BULB_STATE"), net_block: a("BULB_NET"), seq: 0, ev: ev.clone(), bitten: None, expect_wd: false };
    let booted = b.boot(400.0).err().map(|e| format!("{e:?}"));
    let detail = booted.clone().unwrap_or_else(|| format!("pc={:#x} reports={}", b.node.m.r[15], b.st().reports));
    if !o.check("boots into the main loop", booted.is_none() && b.st().reports >= 1, &detail) {
        return finish(&mut o);
    }
    let (en, ch, _, max, ps) = b.pwm_py();
    o.check("PC1 muxed to PWM0, channel enabled, period 48000", en && ch == Some(0) && max == PWM_PERIOD, &ps);
    let hz = b.node.m.cpu_hz;
    o.check("system clock 48 MHz", hz == 48_000_000, &format!("{hz} Hz"));
    let s = b.st();
    o.check(
        "default: on at level 254, power-on behaviour 'on' (as the stock bulb), startup level 'previous'",
        (s.on, s.level, s.pob, s.startup, b.duty()) == (1, 254, POB_ON, 0xFF, tb(254)),
        &format!("{} duty={}", s.py(), b.duty()),
    );
    let recs = b.records();
    o.check("a first record is in the configuration log, after its sector's header", recs.len() == 1,
            &recs.first().map(|r| hx(r)).unwrap_or("none".into()));

    o.say("commands");
    let ok = b.cmd1(&[OP_OFF])? && b.duty() == 0 && b.cmd1(&[OP_ON])?;
    o.check("Off, then On: full brightness", ok && b.st().on == 1 && b.duty() == tb(254), &format!("duty={} host={}", b.duty(), tb(254)));
    let mut bad: Vec<(u8, u32, u32)> = Vec::new();
    for level in 1..=254u8 {
        if !b.mv(level, 0)? || b.duty() != tb(level) || b.st().level != level {
            bad.push((level, b.duty(), tb(level)));
        }
    }
    let show = |v: &[(u8, u32, u32)]| format!("[{}]", v.iter().take(5).map(|(l, c, h)| format!("({l}, {c}, {h})")).collect::<Vec<_>>().join(", "));
    o.check(
        "Move to Level 1..254: duty equals the host build's for every level",
        bad.is_empty(),
        &if bad.is_empty() { format!("e.g. 1->{}, 127->{}, 254->{}", tb(1), tb(127), tb(254)) } else { format!("first differences (level, chip, host): {}", show(&bad)) },
    );
    for level in (1..=254u8).rev() {
        if !b.mv(level, 0)? || b.duty() != tb(level) {
            bad.push((level, b.duty(), tb(level)));
        }
    }
    o.check("the same going down 254..1", bad.is_empty(), &if bad.is_empty() { String::new() } else { show(&bad) });
    let ok = b.mv(0, 0)? && b.st().on == 0 && b.st().level == 1 && b.duty() == 0;
    o.check("Move to Level 0 turns off and keeps the level", ok, &format!("{} duty={}", b.st().py(), b.duty()));
    let ok = b.cmd1(&[OP_TOGGLE])? && b.duty() == tb(1) && b.cmd1(&[OP_TOGGLE])? && b.duty() == 0;
    o.check("Toggle: on at the kept level, then off", ok, "");

    b.mv(20, 0)?;
    b.mv(220, 200)?;
    let mut trace = Vec::new();
    let t_end = b.ms() + 260.0;
    while b.ms() < t_end {
        b.run(10.0)?;
        trace.push(b.duty());
    }
    let rising = trace.windows(2).all(|w| w[0] <= w[1]);
    let mut distinct = trace.clone();
    distinct.sort();
    distinct.dedup();
    let third = trace[trace.len() / 3];
    o.check(
        "fade 20 -> 220 over 200 ms: rises monotonically, passes through, ends exactly",
        rising && tb(20) < third && third < tb(220) && *trace.last().unwrap() == tb(220) && distinct.len() > 10,
        &format!("{} .. {third} .. {} ({} distinct)", trace[0], trace.last().unwrap(), distinct.len()),
    );
    let rej = b.st().rejected;
    let duty = b.duty();
    let ok = b.cmd1(&[0x77])? && b.st().rejected == rej.wrapping_add(1) && b.duty() == duty;
    o.check("an undecodable command is counted and changes nothing", ok, "");

    o.say("flash");
    let log = b.node.m.flash.log.clone();
    let sector = |a: i64| (a as usize) & !(SECTOR_SIZE - 1);
    let config: Vec<_> = log.iter().filter(|e| CONFIG_SECTORS.contains(&sector(e.addr))).collect();
    let allowed = [CONFIG_SECTORS[0], CONFIG_SECTORS[1], MAC_SECTOR, RESET_SECTOR, NV_SECTORS[0], NV_SECTORS[1]];
    let outside: Vec<_> = log.iter().filter(|e| !allowed.contains(&sector(e.addr))).collect();
    let erases = config.iter().filter(|e| e.op == "erase").count();
    let oshow = format!(
        "[{}]",
        outside
            .iter()
            .take(3)
            .map(|e| if e.op == "status" { format!("('status', {}, '{}')", e.addr, e.data) } else { format!("('{}', {}, {})", e.op, e.addr, e.len) })
            .collect::<Vec<_>>()
            .join(", ")
    );
    o.check(
        "programs and erases only in the configuration, network-store, address and power-on counter sectors",
        !log.is_empty() && outside.is_empty(),
        &format!("{} operations ({} configuration), outside: {oshow}", log.len(), config.len()),
    );
    let headers = config.iter().filter(|e| e.op == "program" && e.addr as usize % SECTOR_SIZE == 0).count();
    let written = config.len() - erases - headers;
    o.check(
        "the configuration log moved to the other sector when one filled and erased the older one at the next move",
        erases >= 1 && headers >= 3 && written > 2 * (SECTOR_SIZE / SLOT - 1),
        &format!("{erases} erase(s), {headers} header(s), {written} record(s) written, {} in the active sector now", b.records().len()),
    );
    let mut ram_code: Vec<&String> = addr
        .iter()
        .filter(|(n, v)| 0x20 <= *v as usize && (*v as usize) < rstored && size.get(n).copied().unwrap_or(0) > 8)
        .map(|(n, _)| n)
        .collect();
    let has_irq = ram_code.iter().any(|n| *n == "irq_handler");
    let n_rc = ram_code.len();
    ram_code.sort();
    let joined = ram_code.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ");
    let joined: String = joined.chars().take(160).collect();
    o.check("the flash driver and the interrupt path are linked into the RAM copy", has_irq && n_rc >= 4, &joined);
    let calls = irq_path_calls(&elf_s, rstored as u32);
    let d = match &calls {
        None => "could not disassemble".to_string(),
        Some((n, out)) => format!("{n} call(s), outside: [{}]", out.join(", ")),
    };
    o.check("the interrupt handler calls nothing outside the RAM copy", calls.as_ref().is_some_and(|c| c.1.is_empty()), &d);

    o.say("power cycles");
    b.mv(77, 0)?;
    b.cmd1(&[OP_OFF])?;
    b.cycle()?;
    b.boot(400.0)?;
    let s = b.st();
    o.check("out of the box: turned off, then power lost and back: on, at the level it had",
            s.on == 1 && s.level == 77 && b.duty() == tb(77), &format!("{} duty={}", s.py(), b.duty()));
    b.cmd1(&[OP_POWER_ON, POB_PREVIOUS])?;
    b.mv(77, 0)?;
    b.cycle()?;
    b.boot(400.0)?;
    let s = b.st();
    o.check("previous state restored: on at level 77", s.on == 1 && s.level == 77 && b.duty() == tb(77),
            &format!("{} duty={}", s.py(), b.duty()));
    b.cmd1(&[OP_OFF])?;
    b.cycle()?;
    b.boot(400.0)?;
    let s = b.st();
    o.check("previous state restored: off, level 77 remembered", s.on == 0 && s.level == 77 && b.duty() == 0, "");
    b.cmd1(&[OP_POWER_ON, POB_ON])?;
    b.cmd1(&[OP_STARTUP, 100])?;
    b.cycle()?;
    b.boot(400.0)?;
    let s = b.st();
    o.check("power-on behaviour On with startup level 100", s.on == 1 && b.duty() == tb(100) && s.startup == 100,
            &format!("{} duty={}", s.py(), b.duty()));
    b.cmd1(&[OP_POWER_ON, POB_TOGGLE])?;
    b.cycle()?;
    b.boot(400.0)?;
    let first = b.st().on;
    b.cycle()?;
    b.boot(400.0)?;
    let second = b.st().on;
    o.check("power-on behaviour Toggle alternates", (first, second) == (0, 1), &format!("{first} then {second}"));
    b.cmd1(&[OP_POWER_ON, POB_OFF])?;
    b.cycle()?;
    b.boot(400.0)?;
    o.check("power-on behaviour Off", b.st().on == 0 && b.duty() == 0, "");

    o.say("watchdog");
    b.run(50.0)?;
    let g = &b.node.m.regs;
    let ctrl = u32::from_le_bytes([g[0x620], g[0x621], g[0x622], g[0x623]]);
    o.check("watchdog on, 2 s", ctrl & (1 << 23) != 0 && ctrl & (1 << 6) != 0 && (ctrl >> 9) & 0x3FFF == (2000 * 48000) >> 18,
            &format!("0x620={ctrl:#010x}"));
    b.cmd1(&[OP_POWER_ON, POB_PREVIOUS])?;
    b.cmd1(&[OP_STARTUP, 0xFF])?;
    b.cmd1(&[OP_ON])?;
    b.mv(140, 0)?;
    let words = b.node.m.flash.mem[RESET_SECTOR..RESET_SECTOR + 64].to_vec();
    let t0 = b.ms();
    b.bitten = None;
    b.expect_wd = true;
    b.send(&[OP_HANG], 3000.0)?;
    b.expect_wd = false;
    let bitten = b.bitten.take().map(|t| t - t0);
    o.check("a main loop that stops is ended by the watchdog after 2 s", bitten.is_some_and(|x| (1900.0..=2200.0).contains(&x)),
            &bitten.map(|x| format!("after {x:.0} ms")).unwrap_or_else(|| "no watchdog reset".into()));
    if bitten.is_some() {
        b.seq = 0;
        b.run(300.0)?;
        let (s, n) = (b.st(), b.net());
        o.check(
            "after the watchdog reset the light is back as it was, and the start does not count as a power-on",
            s.on == 1 && s.level == 140 && b.duty() == tb(140) && n.warm == 1 && b.node.m.flash.mem[RESET_SECTOR..RESET_SECTOR + 64] == words[..],
            &format!("{} {}", s.py(), n.py()),
        );
    }

    o.say("factory reset by power cycling (1, 1, 1, 12, 12 s, then on)");
    b.session(31.0)?;
    b.cmd1(&[OP_POWER_ON, POB_OFF])?;
    b.cmd1(&[OP_STARTUP, 33])?;
    b.run(200.0)?;
    let fc_before = nv_items(&b.node.m.flash.mem).get(&2).cloned().unwrap_or_default();
    let mut steps = Vec::new();
    for s in [1.0, 1.0, 1.0, 12.0, 12.0] {
        steps.push(b.session(s)?.reset_step);
    }
    let sshow = format!("[{}]", steps.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(", "));
    o.check("the counter follows the sequence", steps == [1, 2, 3, 5, 7], &sshow);
    o.check("no reset before the sixth power-on", b.net().factory == 0 && b.st().startup == 33, "");
    b.cycle()?;
    let took = b.boot(FACTORY_RESET_BOOT_MS)?;
    let (n, s) = (b.net(), b.st());
    o.check(
        "the sixth power-on is a factory reset: settings back to the defaults",
        n.factory == 1 && (s.on, s.level, s.pob, s.startup) == (1, 254, POB_ON, 0xFF),
        &format!("{} {}, main loop after {took:.0} ms", n.py(), s.py()),
    );
    let mut seen = std::collections::BTreeSet::new();
    let t_end = b.ms() + 2800.0;
    while b.ms() < t_end {
        b.run(50.0)?;
        seen.insert(b.duty());
    }
    b.run(700.0)?;
    let seen_v: Vec<u32> = seen.iter().copied().collect();
    o.check(
        "three seconds of blinking confirm it, then the light is steady",
        seen_v == [0, PWM_PERIOD] && b.duty() == tb(254),
        &format!("duties seen [{}], then {}", seen_v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(", "), b.duty()),
    );
    let items = nv_items(&b.node.m.flash.mem);
    let ishow = format!("{{{}}}", items.iter().map(|(k, v)| format!("{k}: '{}'", hx(v))).collect::<Vec<_>>().join(", "));
    let ok = items.keys().copied().collect::<Vec<_>>() == [2, 16]
        && items[&2].get(..4).unwrap_or(&[]) >= fc_before.get(..4).unwrap_or(&[])
        && items[&2].len() == 8
        && items[&16] == [1, 1, 0, 0, 0];
    o.check("the network store is wiped except for the frame counters (and NumberOfResets is 1)", ok,
            &format!("items now: {ishow}, counters before: {}", hx(&fc_before)));
    let n = b.session(1.0)?;
    o.check("the next power-on is an ordinary one", n.factory == 0, "");

    o.say("factory reset with a full light log sector (the slowest start measured)");
    b.session(31.0)?;
    b.cmd1(&[OP_POWER_ON, POB_PREVIOUS])?;
    let full = SECTOR_SIZE / SLOT;
    let mut k = 0u32;
    while log_slots(&b.node.m.flash.mem).iter().all(|&n| n < full) && k < 2 * full as u32 {
        b.mv(20 + (k * 53 % 200) as u8, 0)?;
        k += 1;
    }
    for s in [1.0, 1.0, 1.0, 12.0, 12.0] {
        b.session(s)?;
    }
    let slots = log_slots(&b.node.m.flash.mem);
    b.cycle()?;
    let booted = b.boot(FACTORY_RESET_BOOT_MS);
    let n = b.net();
    o.check(
        &format!("a factory reset over a full light log sector starts within {FACTORY_RESET_BOOT_MS:.0} ms"),
        booted.is_ok() && slots.contains(&full) && n.factory == 1 && b.st().level == 254,
        &format!("{k} changes, log slots used {slots:?}, {}, {}", booted.map_or_else(|e| format!("{e:?}"), |t| format!("main loop after {t:.0} ms")), n.py()),
    );
    finish(&mut o)
}

fn log_slots(flash: &[u8]) -> Vec<usize> {
    CONFIG_SECTORS
        .iter()
        .map(|&base| flash[base..base + SECTOR_SIZE].chunks(SLOT).filter(|c| c.iter().any(|&x| x != 0xFF)).count())
        .collect()
}

fn finish(o: &mut Out) -> R<()> {
    o.flush();
    let n = o.cx.results.len();
    let ok = o.cx.results.iter().filter(|r| r.1).count();
    o.say(&format!("\n{ok}/{n} checks passed"));
    Ok(())
}
