use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle, Thread};

use crate::air::{bulb_next, Bulb, BulbOut};

pub(crate) struct BulbPtr(*mut Bulb);

unsafe impl Send for BulbPtr {}

impl BulbPtr {
    pub(crate) fn new(b: &mut Bulb) -> BulbPtr {
        BulbPtr(b)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Phase {
    Commit,
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
    busy: std::time::Duration,
}

struct Shared {
    gen: AtomicU64,
    done: AtomicUsize,
    quit: AtomicBool,
    slots: Vec<Mutex<Slot>>,
    main: Mutex<Thread>,
    spin: u32,
}

pub(crate) struct Pool {
    sh: Arc<Shared>,
    handles: Vec<JoinHandle<()>>,
}

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

    pub(crate) fn busy(&self) -> Vec<std::time::Duration> {
        self.sh.slots.iter().map(|s| s.lock().unwrap().busy).collect()
    }

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
