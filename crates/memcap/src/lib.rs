use std::alloc::{GlobalAlloc, Layout, System};
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};

pub const DEFAULT_LIMIT_MB: usize = 4096;
pub const ENV: &str = "OUTGLOW_TEST_MEM_LIMIT_MB";

pub struct Capped;

static USED: AtomicUsize = AtomicUsize::new(0);
static LIMIT: AtomicUsize = AtomicUsize::new(0);

fn limit() -> usize {
    let l = LIMIT.load(Ordering::Relaxed);
    if l != 0 {
        return l;
    }
    LIMIT.store(usize::MAX, Ordering::Relaxed);
    let mb = std::env::var(ENV).ok().and_then(|v| v.parse::<usize>().ok()).unwrap_or(DEFAULT_LIMIT_MB);
    let bytes = if mb == 0 { usize::MAX } else { mb.saturating_mul(1 << 20) };
    LIMIT.store(bytes, Ordering::Relaxed);
    bytes
}

pub fn used() -> usize {
    USED.load(Ordering::Relaxed)
}

fn over(size: usize) {
    let total = USED.fetch_add(size, Ordering::Relaxed) + size;
    let l = limit();
    if total > l {
        let mut e = std::io::stderr();
        let _ = e.write_all(b"\nmemcap: the test process allocated more than ");
        let mut digits = [0u8; 20];
        let mut n = l >> 20;
        let mut at = digits.len();
        loop {
            at -= 1;
            digits[at] = b'0' + (n % 10) as u8;
            n /= 10;
            if n == 0 {
                break;
            }
        }
        let _ = e.write_all(&digits[at..]);
        let _ = e.write_all(b" MB (a runaway loop?); stopped. Raise it with OUTGLOW_TEST_MEM_LIMIT_MB (0 = no limit).\n");
        std::process::abort();
    }
}

unsafe impl GlobalAlloc for Capped {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        over(layout.size());
        System.alloc(layout)
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        over(layout.size());
        System.alloc_zeroed(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        USED.fetch_sub(layout.size(), Ordering::Relaxed);
        System.dealloc(ptr, layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if new_size > layout.size() {
            over(new_size - layout.size());
        } else {
            USED.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
        }
        System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static GLOBAL: Capped = Capped;
