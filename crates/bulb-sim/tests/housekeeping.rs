use memcap as _;

use std::cell::Cell;
use std::rc::Rc;

use bulb_core::command::StateReport;
use bulb_core::hal::{ConfigStore, Radio};
use bulb_core::{Bulb, Command};
use bulb_sim::{MockClock, MockFlash, MockPwm, MockRadio};

#[derive(Clone, Default)]
struct Store {
    config: MockFlash,
    prepared: Rc<Cell<u32>>,
}

impl ConfigStore for Store {
    fn load(&self, buf: &mut [u8]) -> Option<usize> {
        self.config.load(buf)
    }
    fn store(&mut self, data: &[u8]) {
        self.config.store(data)
    }
    fn prepare(&mut self) {
        self.prepared.set(self.prepared.get() + 1);
    }
}

#[derive(Clone, Default)]
struct Link {
    inner: MockRadio,
    quiet: Rc<Cell<bool>>,
    asked: Rc<Cell<u32>>,
}

impl Radio for Link {
    fn poll(&mut self) -> Option<Command> {
        self.inner.poll()
    }
    fn report(&mut self, state: &StateReport) {
        self.inner.report(state)
    }
    fn housekeep(&mut self) -> bool {
        self.asked.set(self.asked.get() + 1);
        self.quiet.get()
    }
}

struct Rig {
    bulb: Bulb<MockPwm, MockClock, Store, Link>,
    clock: MockClock,
    store: Store,
    link: Link,
}

impl Rig {
    fn new() -> Self {
        let (clock, store, link) = (MockClock::new(), Store::default(), Link::default());
        link.quiet.set(true);
        let bulb = Bulb::boot(MockPwm::new(1000), clock.handle(), store.clone(), link.clone());
        Rig { bulb, clock, store, link }
    }

    fn ticks(&mut self, n: u32, step_ms: u64) -> (u32, u32) {
        let (asked, prepared) = (self.link.asked.get(), self.store.prepared.get());
        for _ in 0..n {
            self.clock.advance(step_ms);
            self.bulb.tick();
        }
        (self.link.asked.get() - asked, self.store.prepared.get() - prepared)
    }
}

#[test]
fn an_idle_bulb_on_a_quiet_link_prepares_its_store_every_tick() {
    let mut r = Rig::new();
    assert_eq!(r.ticks(5, 10), (5, 5));
}

#[test]
fn a_busy_link_keeps_the_store_from_preparing() {
    let mut r = Rig::new();
    r.link.quiet.set(false);
    assert_eq!(r.ticks(5, 10), (5, 0));
}

#[test]
fn nothing_is_prepared_while_a_transition_runs() {
    let mut r = Rig::new();
    r.bulb.handle(Command::MoveToLevel { level: 20, transition_ms: 1000 });
    assert_eq!(r.ticks(9, 100), (0, 0));
    assert_eq!(r.ticks(3, 100), (3, 3));
}

#[test]
fn nothing_is_prepared_while_identifying() {
    let mut r = Rig::new();
    r.bulb.handle(Command::Identify { times: 1 });
    assert_eq!(r.ticks(9, 100), (0, 0));
    assert_eq!(r.ticks(3, 100), (3, 3));
}

#[test]
fn a_link_without_housekeeping_never_lets_the_store_prepare() {
    let (clock, store) = (MockClock::new(), Store::default());
    let mut bulb = Bulb::boot(MockPwm::new(1000), clock.handle(), store.clone(), MockRadio::new());
    for _ in 0..5 {
        clock.advance(10);
        bulb.tick();
    }
    assert_eq!(store.prepared.get(), 0);
}
