//! Statics the main loop shares with the interrupt handler, and the buffers
//! the radio's DMA reads and writes behind the program's back.
//!
//! The TLSR8258 has one core and no atomic instructions (`thumbv4t` has no
//! `target_has_atomic`), so `core::sync::atomic` and the usual mutexes are
//! not available. Instead every access here is one volatile load or store of
//! a `Copy` value or element, and no reference into a shared static is ever
//! handed out: the compiler can neither keep a value in a register across an
//! interrupt nor assume that memory did not change between two reads.
//!
//! What volatile cannot do is make a value wider than a word change at
//! once: an interrupt can come between the halves of `Shared<[u16; 2]>`.
//! Where both sides write such a value, the main loop turns the interrupt
//! off around the write (`reg::irq_disable`), as the callers say.
//!
//! With `reg.rs` (registers) and the flash routine running from RAM
//! (`flash.rs`), this is where the firmware's `unsafe` lives; the rest of
//! the crate denies it (`lib.rs`). Every method is `#[inline(always)]`: the
//! interrupt path runs from `.ram_code` and must not call into flash.

use core::cell::UnsafeCell;
use core::mem::MaybeUninit;

/// A value shared with the interrupt handler.
#[repr(transparent)]
pub struct Shared<T>(UnsafeCell<T>);

// SAFETY: one core, and every access is a volatile copy in or out (see the
// module comment); `T: Copy` rules out handing out a reference by value.
unsafe impl<T: Copy> Sync for Shared<T> {}

impl<T: Copy> Shared<T> {
    pub const fn new(v: T) -> Self {
        Shared(UnsafeCell::new(v))
    }

    #[inline(always)]
    pub fn get(&self) -> T {
        // SAFETY: the pointer is to this static, valid and aligned; volatile, no reference made.
        unsafe { self.0.get().read_volatile() }
    }

    #[inline(always)]
    pub fn set(&self, v: T) {
        // SAFETY: as in `get`.
        unsafe { self.0.get().write_volatile(v) }
    }

    /// `set(f(get()))`. Not atomic: an interrupt between the two can be lost,
    /// so only for values the interrupt does not write.
    #[inline(always)]
    pub fn update(&self, f: impl FnOnce(T) -> T) {
        self.set(f(self.get()));
    }
}

impl<T: Copy, const N: usize> Shared<[T; N]> {
    /// Element `i`, or `None` past the end (no panic: the interrupt path
    /// cannot reach the panic handler in flash).
    #[inline(always)]
    pub fn at(&self, i: usize) -> Option<T> {
        if i < N {
            // SAFETY: `i < N`, inside the array; volatile, no reference made.
            Some(unsafe { (self.0.get() as *const T).add(i).read_volatile() })
        } else {
            None
        }
    }

    /// Writes element `i`; past the end nothing is written.
    #[inline(always)]
    #[cfg_attr(not(feature = "ble"), allow(dead_code))] // only BLE writes single elements
    pub fn set_at(&self, i: usize, v: T) {
        if i < N {
            // SAFETY: as in `at`.
            unsafe { (self.0.get() as *mut T).add(i).write_volatile(v) }
        }
    }
}

/// A byte buffer the radio's DMA fills or sends from, word aligned as the
/// DMA needs. Its contents change while the program runs; reads past the
/// end give 0 and writes past it are dropped, so a bad length in a received
/// frame cannot reach other memory.
#[repr(C, align(4))]
pub struct SharedBuf<const N: usize>(UnsafeCell<[u8; N]>);

// SAFETY: as for `Shared`: one core, volatile byte and word accesses only.
unsafe impl<const N: usize> Sync for SharedBuf<N> {}

impl<const N: usize> SharedBuf<N> {
    pub const fn new(init: [u8; N]) -> Self {
        SharedBuf(UnsafeCell::new(init))
    }

    #[inline(always)]
    pub fn read(&self, i: usize) -> u8 {
        if i < N {
            // SAFETY: `i < N`, inside the buffer.
            unsafe { (self.0.get() as *const u8).add(i).read_volatile() }
        } else {
            0
        }
    }

    #[inline(always)]
    pub fn write(&self, i: usize, v: u8) {
        if i < N {
            // SAFETY: as in `read`.
            unsafe { (self.0.get() as *mut u8).add(i).write_volatile(v) }
        }
    }

    /// The little-endian word at byte `i`, which must be a multiple of 4
    /// (otherwise, or past the end, 0).
    #[inline(always)]
    pub fn read32(&self, i: usize) -> u32 {
        if i % 4 == 0 && i + 4 <= N {
            // SAFETY: inside the buffer, and aligned: the buffer is, and `i` is a multiple of 4.
            unsafe { ((self.0.get() as *const u8).add(i) as *const u32).read_volatile() }
        } else {
            0
        }
    }

    #[inline(always)]
    pub fn write32(&self, i: usize, v: u32) {
        if i % 4 == 0 && i + 4 <= N {
            // SAFETY: as in `read32`.
            unsafe { ((self.0.get() as *mut u8).add(i) as *mut u32).write_volatile(v) }
        }
    }

    /// Copies `src` in from byte `at`, as much as fits.
    #[inline(always)]
    pub fn copy_from(&self, at: usize, src: &[u8]) {
        let mut i = 0;
        while i < src.len() {
            self.write(at + i, src[i]);
            i += 1;
        }
    }

    /// Fills `dst` from byte `at`.
    #[inline(always)]
    pub fn copy_to(&self, at: usize, dst: &mut [u8]) {
        let mut i = 0;
        while i < dst.len() {
            dst[i] = self.read(at + i);
            i += 1;
        }
    }

    /// The address, for the DMA registers.
    #[inline(always)]
    pub fn addr(&self) -> usize {
        self.0.get() as usize
    }
}

/// A static that one owner takes as `&'static mut` and then uses as ordinary
/// memory: for state that no interrupt handler shares (the radio link, the
/// black box). The second `take` gets `None`, so there is never a second
/// reference.
pub struct TakeOnce<T> {
    taken: Shared<bool>,
    value: UnsafeCell<MaybeUninit<T>>,
}

// SAFETY: `take` hands out at most one reference, ever; nothing else reads
// `value`. (One core: the check and the set of `taken` cannot interleave
// with another `take`, which only the main loop calls.)
unsafe impl<T> Sync for TakeOnce<T> {}

impl<T> TakeOnce<T> {
    pub const fn new() -> Self {
        TakeOnce { taken: Shared::new(false), value: UnsafeCell::new(MaybeUninit::uninit()) }
    }

    /// Builds the value in place with `f` and returns the only reference to
    /// it; `None` if it was taken before. (A closure, not a value: a value of
    /// some kilobytes passed in would sit on the stack first.)
    #[inline(always)]
    pub fn take_with(&'static self, f: impl FnOnce() -> T) -> Option<&'static mut T> {
        if self.taken.get() {
            return None;
        }
        self.taken.set(true);
        // SAFETY: `taken` was false, so no reference to `value` exists, and
        // none will be made after this one.
        Some(unsafe { (*self.value.get()).write(f()) })
    }
}
