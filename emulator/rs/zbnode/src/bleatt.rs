use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use ble::{AttScript, Central, CentralRc, ScriptValue};
use tc32emu::Machine;

pub struct Rsp {
    pub ms: f64,
    pub data: Vec<u8>,
}

#[derive(Default)]
pub struct Queue {
    reqs: VecDeque<Vec<u8>>,
    pub rsps: Vec<Rsp>,
    idle: bool,
    started: bool,
}

pub struct QueueCell(RefCell<Queue>);

impl QueueCell {
    pub fn borrow(&self) -> std::cell::Ref<'_, Queue> {
        self.0.borrow()
    }
    fn borrow_mut(&self) -> std::cell::RefMut<'_, Queue> {
        self.0.borrow_mut()
    }
}

pub type QueueRc = Rc<QueueCell>;

pub fn new_queue() -> QueueRc {
    Rc::new(QueueCell(RefCell::new(Queue::default())))
}

impl AttScript for QueueCell {
    fn next(&self, _c: &mut Central, m: &mut Machine, value: &ScriptValue) -> (Option<Vec<u8>>, bool, bool) {
        let mut q = self.borrow_mut();
        q.started = true;
        if let ScriptValue::Bytes(b) = value {
            q.rsps.push(Rsp { ms: m.ms(), data: b.clone() });
        }
        match q.reqs.pop_front() {
            None => {
                q.idle = true;
                (None, false, true)
            }
            Some(r) => {
                q.idle = false;
                (Some(r), false, false)
            }
        }
    }
}

pub fn push(q: &QueueRc, c: &CentralRc, m: &mut Machine, req: Vec<u8>) {
    let kick = {
        let mut s = q.borrow_mut();
        s.reqs.push_back(req);
        if s.started && s.idle {
            s.idle = false;
            true
        } else {
            false
        }
    };
    if kick {
        c.borrow_mut().script_step(m, ScriptValue::Nil);
    }
}

pub fn started(q: &QueueRc) -> bool {
    q.borrow().started
}
