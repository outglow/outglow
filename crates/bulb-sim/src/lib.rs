#![forbid(unsafe_code)]
#[cfg(test)]
use memcap as _;

pub mod cut;

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use bulb_core::command::StateReport;
use bulb_core::hal::{Clock, ConfigStore, Pwm, Radio};
use bulb_core::{Bulb, Command};

#[derive(Debug, Clone)]
pub struct MockPwm {
    pub max: u16,
    pub duty: u16,
    pub history: Vec<u16>,
    pub cold: Option<u16>,
    pub rgb: Option<[u16; 3]>,
}

impl MockPwm {
    pub fn new(max: u16) -> Self {
        MockPwm { max, duty: 0, history: vec![], cold: None, rgb: None }
    }
    pub fn tunable(max: u16) -> Self {
        MockPwm { max, duty: 0, history: vec![], cold: Some(0), rgb: None }
    }
    pub fn colour(max: u16) -> Self {
        MockPwm { max, duty: 0, history: vec![], cold: Some(0), rgb: Some([0; 3]) }
    }
    pub fn fraction(&self) -> f32 {
        self.duty as f32 / self.max as f32
    }
}

impl Pwm for MockPwm {
    fn max_duty(&self) -> u16 {
        self.max
    }
    fn set_duty(&mut self, duty: u16) {
        self.duty = duty.min(self.max);
        self.history.push(self.duty);
    }
    fn has_cold_channel(&self) -> bool {
        self.cold.is_some()
    }
    fn set_cold_duty(&mut self, duty: u16) {
        if self.cold.is_some() {
            self.cold = Some(duty.min(self.max));
        }
    }
    fn has_rgb(&self) -> bool {
        self.rgb.is_some()
    }
    fn set_rgb(&mut self, r: u16, g: u16, b: u16) {
        if self.rgb.is_some() {
            self.rgb = Some([r, g, b]);
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct MockClock {
    ms: Rc<RefCell<u64>>,
}

impl MockClock {
    pub fn new() -> Self {
        MockClock { ms: Rc::new(RefCell::new(0)) }
    }
    pub fn advance(&self, ms: u64) {
        *self.ms.borrow_mut() += ms;
    }
    pub fn handle(&self) -> MockClock {
        self.clone()
    }
}

impl Clock for MockClock {
    fn now_ms(&self) -> u64 {
        *self.ms.borrow()
    }
}

#[derive(Debug, Clone, Default)]
pub struct MockFlash {
    data: Rc<RefCell<Option<Vec<u8>>>>,
    pub writes: Rc<RefCell<u32>>,
}

impl MockFlash {
    pub fn new() -> Self {
        MockFlash::default()
    }
    pub fn handle(&self) -> MockFlash {
        self.clone()
    }
    pub fn is_blank(&self) -> bool {
        self.data.borrow().is_none()
    }
    pub fn write_count(&self) -> u32 {
        *self.writes.borrow()
    }
}

impl ConfigStore for MockFlash {
    fn load(&self, buf: &mut [u8]) -> Option<usize> {
        let d = self.data.borrow();
        let bytes = d.as_ref()?;
        let n = bytes.len().min(buf.len());
        buf[..n].copy_from_slice(&bytes[..n]);
        Some(n)
    }
    fn store(&mut self, data: &[u8]) {
        *self.data.borrow_mut() = Some(data.to_vec());
        *self.writes.borrow_mut() += 1;
    }
}

#[derive(Debug, Clone, Default)]
pub struct MockRadio {
    inbox: Rc<RefCell<VecDeque<Command>>>,
    pub reports: Rc<RefCell<Vec<StateReport>>>,
}

impl MockRadio {
    pub fn new() -> Self {
        MockRadio::default()
    }
    pub fn handle(&self) -> MockRadio {
        self.clone()
    }
    pub fn send(&self, cmd: Command) {
        self.inbox.borrow_mut().push_back(cmd);
    }
    pub fn last_report(&self) -> Option<StateReport> {
        self.reports.borrow().last().copied()
    }
}

impl Radio for MockRadio {
    fn poll(&mut self) -> Option<Command> {
        self.inbox.borrow_mut().pop_front()
    }
    fn report(&mut self, state: &StateReport) {
        self.reports.borrow_mut().push(*state);
    }
}

pub struct Harness {
    pub bulb: Bulb<MockPwm, MockClock, MockFlash, MockRadio>,
    pub clock: MockClock,
    pub flash: MockFlash,
    pub radio: MockRadio,
}

impl Harness {
    pub fn new() -> Self {
        Self::with_flash(MockFlash::new())
    }

    pub fn new_tunable() -> Self {
        Self::with(MockFlash::new(), MockPwm::tunable(1000))
    }

    fn with_flash(flash: MockFlash) -> Self {
        Self::with(flash, MockPwm::new(1000))
    }

    fn with(flash: MockFlash, pwm: MockPwm) -> Self {
        let clock = MockClock::new();
        let radio = MockRadio::new();
        let bulb = Bulb::boot(pwm, clock.handle(), flash.handle(), radio.handle());
        Harness { bulb, clock, flash, radio }
    }

    pub fn run_for(&mut self, total_ms: u64, step_ms: u64) {
        let step = step_ms.max(1);
        let mut elapsed = 0;
        self.bulb.tick();
        while elapsed < total_ms {
            self.clock.advance(step);
            elapsed += step;
            self.bulb.tick();
        }
    }

    pub fn tick(&mut self) {
        self.bulb.tick();
    }

    pub fn power_cycle(&mut self) {
        let flash = self.flash.handle();
        let radio = MockRadio::new();
        let clock = MockClock::new();
        let pwm = if self.bulb.pwm().cold.is_some() { MockPwm::tunable(1000) } else { MockPwm::new(1000) };
        let bulb = Bulb::boot(pwm, clock.handle(), flash.handle(), radio.handle());
        self.bulb = bulb;
        self.clock = clock;
        self.flash = flash;
        self.radio = radio;
    }

    pub fn command(&mut self, cmd: Command) {
        self.radio.send(cmd);
        self.bulb.tick();
    }

    pub fn is_on(&self) -> bool {
        self.bulb.is_on()
    }
    pub fn level(&self) -> u8 {
        self.bulb.current_level()
    }
    pub fn state(&self) -> StateReport {
        self.bulb.state()
    }
}

impl Default for Harness {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlashOp {
    Program { addr: usize, data: Vec<u8> },
    Erase { addr: usize },
}

#[derive(Debug, Clone)]
struct NorInner {
    base: usize,
    sector: usize,
    mem: Vec<u8>,
    ops: Vec<FlashOp>,
}

pub const MAX_OPS: usize = 1_000_000;

#[derive(Debug, Clone)]
pub struct NorFlash {
    inner: Rc<RefCell<NorInner>>,
}

impl NorFlash {
    pub fn new(base: usize, len: usize, sector: usize) -> Self {
        NorFlash { inner: Rc::new(RefCell::new(NorInner { base, sector, mem: vec![0xFF; len], ops: Vec::new() })) }
    }
    pub fn snapshot(&self) -> NorFlash {
        let mut i = self.inner.borrow().clone();
        i.ops.clear();
        NorFlash { inner: Rc::new(RefCell::new(i)) }
    }
    pub fn ops(&self) -> Vec<FlashOp> {
        self.inner.borrow().ops.clone()
    }
    pub fn clear_ops(&self) {
        self.inner.borrow_mut().ops.clear();
    }
    pub fn programs(&self) -> usize {
        self.inner.borrow().ops.iter().filter(|o| matches!(o, FlashOp::Program { .. })).count()
    }
    pub fn erases(&self) -> usize {
        self.inner.borrow().ops.iter().filter(|o| matches!(o, FlashOp::Erase { .. })).count()
    }
    pub fn byte(&self, addr: usize) -> u8 {
        let i = self.inner.borrow();
        i.mem[addr - i.base]
    }
    pub fn poke(&self, addr: usize, data: &[u8]) {
        let mut i = self.inner.borrow_mut();
        let o = addr - i.base;
        i.mem[o..o + data.len()].copy_from_slice(data);
    }
    fn record(&self, op: FlashOp) {
        let mut i = self.inner.borrow_mut();
        assert!(i.ops.len() < MAX_OPS, "more than {MAX_OPS} flash operations without clear_ops: a runaway loop");
        i.ops.push(op);
    }
    pub fn sector_size(&self) -> usize {
        self.inner.borrow().sector
    }
    pub fn apply(&self, op: &FlashOp) {
        self.apply_part(op, |_, _| 0);
    }
    pub fn apply_part(&self, op: &FlashOp, mut undone: impl FnMut(usize, u8) -> u8) {
        let mut i = self.inner.borrow_mut();
        let base = i.base;
        match op {
            FlashOp::Program { addr, data } => {
                for (k, d) in data.iter().enumerate() {
                    let old = i.mem[addr + k - base];
                    let keep = undone(k, old) & old & !d;
                    i.mem[addr + k - base] = (old & d) | keep;
                }
            }
            FlashOp::Erase { addr } => {
                let s = i.sector;
                for k in 0..s {
                    let old = i.mem[addr + k - base];
                    let keep = undone(k, old) & !old;
                    i.mem[addr + k - base] = 0xFF & !keep;
                }
            }
        }
    }
}

impl bulb_core::store::Flash for NorFlash {
    fn read(&self, addr: usize, buf: &mut [u8]) {
        let i = self.inner.borrow();
        let o = addr - i.base;
        buf.copy_from_slice(&i.mem[o..o + buf.len()]);
    }
    fn program(&mut self, addr: usize, data: &[u8]) {
        assert!(!data.is_empty(), "an empty program at {addr:#x}");
        assert_eq!(addr / 256, (addr + data.len() - 1) / 256, "a program within one page");
        let op = FlashOp::Program { addr, data: data.to_vec() };
        self.apply(&op);
        self.record(op);
    }
    fn erase(&mut self, addr: usize) {
        assert_eq!(addr % self.sector_size(), 0, "a sector start");
        let op = FlashOp::Erase { addr };
        self.apply(&op);
        self.record(op);
    }
}
