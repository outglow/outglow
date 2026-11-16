//! The worker threads of the parallel lockstep (`World::lockstep` with
//! `ZB_PARALLEL=1`): one thread per further bulb, kept for the life of the
//! `World`, each running its bulb for one phase at a time while the
//! scenario's thread runs the first bulb and the scripted nodes.
//!
//! A bulb is a `zbnode::Node`, full of `Rc` and `RefCell`, so it is not
//! `Send`. It is handed to a worker anyway ([`BulbPtr`]), under this
//! invariant: no `Rc` inside a bulb is shared with anything that another
//! thread touches while the bulb is out. The emulator's hooks only ever
//! capture the node's own models; the air (`Air`) copies every frame and BLE
//! packet it hands to a bulb; the scenario's own handles into a bulb (its BLE
//! central, debug logs a scenario hooks into one bulb) are touched only by
//! scenario code, which runs between the phases, while the workers wait.
//! The lockstep keeps its side: during a phase the further bulbs are taken
//! out of `Air::bulbs` and what the air sends them waits in `Air::lent`.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle, Thread};

use crate::air::{bulb_next, Bulb, BulbOut};

/// A bulb lent to a worker for one phase.
pub(crate) struct BulbPtr(*mut Bulb);

// SAFETY: see the module documentation. The pointer is used by one worker
// between `Pool::start` and the end of `Pool::wait`, and the bulb is touched
// by nobody else then.
unsafe impl Send for BulbPtr {}

impl BulbPtr {
    pub(crate) fn new(b: &mut Bulb) -> BulbPtr {
        BulbPtr(b)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Phase {
    /// The bulb waits in TX mode: run it until its frame starts or it
    /// leaves TX mode (or `target`).
    Commit,
    /// Run it to `target`.
    Run,
}

pub(crate) struct Job {
    pub bulb: BulbPtr,
    pub target: f64,
    pub powered: bool,
    pub phase: Phase,
}

#[derive(Default)]
struct Slot {
    job: Option<Job>,
    out: Vec<BulbOut>,
    err: Option<String>,
    /// wall time this worker spent running its bulb
    busy: std::time::Duration,
}

struct Shared {
    /// bumped by the scenario's thread to start a phase
    gen: AtomicU64,
    /// workers done with the current phase
    done: AtomicUsize,
    quit: AtomicBool,
    slots: Vec<Mutex<Slot>>,
    /// the thread that waits for the phase (the scenario's)
    main: Mutex<Thread>,
    spin: u32,
}

pub(crate) struct Pool {
    sh: Arc<Shared>,
    handles: Vec<JoinHandle<()>>,
}

/// Busy-wait this many rounds before sleeping (`ZB_PARALLEL_SPIN`): a phase
/// is short, and waking a parked thread costs more than most phases take.
fn spin_rounds() -> u32 {
    std::env::var("ZB_PARALLEL_SPIN").ok().and_then(|v| v.parse().ok()).unwrap_or(20_000)
}

impl Pool {
    pub(crate) fn new(n: usize) -> Pool {
        let sh = Arc::new(Shared {
            gen: AtomicU64::new(0),
            done: AtomicUsize::new(0),
            quit: AtomicBool::new(false),
            slots: (0..n).map(|_| Mutex::new(Slot::default())).collect(),
            main: Mutex::new(thread::current()),
            spin: spin_rounds(),
        });
        let handles = (0..n)
            .map(|k| {
                let sh = sh.clone();
                thread::Builder::new().name(format!("bulb{k}")).spawn(move || worker(&sh, k)).expect("spawn a bulb thread")
            })
            .collect();
        Pool { sh, handles }
    }

    pub(crate) fn len(&self) -> usize {
        self.sh.slots.len()
    }

    /// Starts a phase: worker k runs `jobs[k]` (None: nothing this time).
    ///
    /// # Safety
    /// Every bulb in `jobs` stays where it is and is touched by nobody else
    /// until `wait` returns.
    pub(crate) unsafe fn start(&self, jobs: Vec<Option<Job>>) {
        debug_assert_eq!(jobs.len(), self.len());
        *self.sh.main.lock().unwrap() = thread::current();
        for (slot, job) in self.sh.slots.iter().zip(jobs) {
            slot.lock().unwrap().job = job;
        }
        self.sh.done.store(0, Ordering::Release);
        self.sh.gen.fetch_add(1, Ordering::AcqRel);
        for h in &self.handles {
            h.thread().unpark();
        }
    }

    /// Waits for the phase `start` began to end on every worker.
    pub(crate) fn wait(&self) {
        let n = self.len();
        let mut spins = 0u32;
        while self.sh.done.load(Ordering::Acquire) < n {
            if spins < self.sh.spin {
                spins += 1;
                std::hint::spin_loop();
            } else {
                thread::park();
            }
        }
    }

    /// The wall time each worker spent running its bulb so far.
    pub(crate) fn busy(&self) -> Vec<std::time::Duration> {
        self.sh.slots.iter().map(|s| s.lock().unwrap().busy).collect()
    }

    /// What worker k's bulb put out in the last phase, and its error.
    pub(crate) fn take(&self, k: usize) -> (Vec<BulbOut>, Option<String>) {
        let mut s = self.sh.slots[k].lock().unwrap();
        (std::mem::take(&mut s.out), s.err.take())
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.sh.quit.store(true, Ordering::Release);
        self.sh.gen.fetch_add(1, Ordering::AcqRel);
        for h in self.handles.drain(..) {
            h.thread().unpark();
            let _ = h.join();
        }
    }
}

fn worker(sh: &Shared, k: usize) {
    let mut seen = 0u64;
    loop {
        let mut spins = 0u32;
        loop {
            let g = sh.gen.load(Ordering::Acquire);
            if g != seen {
                seen = g;
                break;
            }
            if spins < sh.spin {
                spins += 1;
                std::hint::spin_loop();
            } else {
                thread::park();
            }
        }
        if sh.quit.load(Ordering::Acquire) {
            return;
        }
        {
            let mut guard = sh.slots[k].lock().unwrap();
            let slot = &mut *guard;
            if let Some(job) = slot.job.take() {
                // SAFETY: `Pool::start`'s contract: the bulb is this worker's until `wait` returns.
                let b = unsafe { &mut *job.bulb.0 };
                let t0 = std::time::Instant::now();
                let out = &mut slot.out;
                let r = catch_unwind(AssertUnwindSafe(|| run_job(b, &job, out)));
                slot.err = match r {
                    Ok(Ok(())) => None,
                    Ok(Err(e)) => Some(e),
                    Err(p) => Some(format!(
                        "bulb thread panicked: {}",
                        p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default()
                    )),
                };
                slot.busy += t0.elapsed();
            }
        }
        if sh.done.fetch_add(1, Ordering::AcqRel) + 1 == sh.slots.len() {
            sh.main.lock().unwrap().unpark();
        }
    }
}

fn run_job(b: &mut Bulb, job: &Job, out: &mut Vec<BulbOut>) -> Result<(), String> {
    match job.phase {
        Phase::Run => {
            while let Some(o) = bulb_next(b, job.target, job.powered, false)? {
                out.push(o);
            }
        }
        Phase::Commit => {
            b.node.set_stop_uncommitted(true);
            let r = (|| -> Result<(), String> {
                while let Some(o) = bulb_next(b, job.target, job.powered, true)? {
                    let sent = matches!(o, BulbOut::Tx { .. });
                    out.push(o);
                    if sent {
                        break;
                    }
                }
                Ok(())
            })();
            b.node.set_stop_uncommitted(false);
            r?;
        }
    }
    Ok(())
}
