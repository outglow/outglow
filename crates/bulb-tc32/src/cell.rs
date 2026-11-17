use core::cell::UnsafeCell;
use core::mem::MaybeUninit;

#[repr(transparent)]
pub struct Shared<T>(UnsafeCell<T>);

unsafe impl<T: Copy> Sync for Shared<T> {}

impl<T: Copy> Shared<T> {
    pub const fn new(v: T) -> Self {
        Shared(UnsafeCell::new(v))
    }

    #[inline(always)]
    pub fn get(&self) -> T {
        unsafe { self.0.get().read_volatile() }
    }

    #[inline(always)]
    pub fn set(&self, v: T) {
        unsafe { self.0.get().write_volatile(v) }
    }

    #[inline(always)]
    pub fn update(&self, f: impl FnOnce(T) -> T) {
        self.set(f(self.get()));
    }
}

impl<T: Copy, const N: usize> Shared<[T; N]> {
    #[inline(always)]
    pub fn at(&self, i: usize) -> Option<T> {
        if i < N {
            Some(unsafe { (self.0.get() as *const T).add(i).read_volatile() })
        } else {
            None
        }
    }

    #[inline(always)]
    #[cfg_attr(not(feature = "ble"), allow(dead_code))]
    pub fn set_at(&self, i: usize, v: T) {
        if i < N {
            unsafe { (self.0.get() as *mut T).add(i).write_volatile(v) }
        }
    }
}

#[repr(C, align(4))]
pub struct SharedBuf<const N: usize>(UnsafeCell<[u8; N]>);

unsafe impl<const N: usize> Sync for SharedBuf<N> {}

impl<const N: usize> SharedBuf<N> {
    pub const fn new(init: [u8; N]) -> Self {
        SharedBuf(UnsafeCell::new(init))
    }

    #[inline(always)]
    pub fn read(&self, i: usize) -> u8 {
        if i < N {
            unsafe { (self.0.get() as *const u8).add(i).read_volatile() }
        } else {
            0
        }
    }

    #[inline(always)]
    pub fn write(&self, i: usize, v: u8) {
        if i < N {
            unsafe { (self.0.get() as *mut u8).add(i).write_volatile(v) }
        }
    }

    #[inline(always)]
    pub fn read32(&self, i: usize) -> u32 {
        if i % 4 == 0 && i + 4 <= N {
            unsafe { ((self.0.get() as *const u8).add(i) as *const u32).read_volatile() }
        } else {
            0
        }
    }

    #[inline(always)]
    pub fn write32(&self, i: usize, v: u32) {
        if i % 4 == 0 && i + 4 <= N {
            unsafe { ((self.0.get() as *mut u8).add(i) as *mut u32).write_volatile(v) }
        }
    }

    #[inline(always)]
    pub fn copy_from(&self, at: usize, src: &[u8]) {
        for (i, b) in src.iter().enumerate() {
            self.write(at + i, *b);
        }
    }

    #[inline(always)]
    pub fn copy_to(&self, at: usize, dst: &mut [u8]) {
        for (i, b) in dst.iter_mut().enumerate() {
            *b = self.read(at + i);
        }
    }

    #[inline(always)]
    pub fn addr(&self) -> usize {
        self.0.get() as usize
    }
}

pub struct TakeOnce<T> {
    taken: Shared<bool>,
    value: UnsafeCell<MaybeUninit<T>>,
}

unsafe impl<T> Sync for TakeOnce<T> {}

impl<T> TakeOnce<T> {
    pub const fn new() -> Self {
        TakeOnce { taken: Shared::new(false), value: UnsafeCell::new(MaybeUninit::uninit()) }
    }

    #[inline(always)]
    pub fn take_with(&'static self, f: impl FnOnce() -> T) -> Option<&'static mut T> {
        if self.taken.get() {
            return None;
        }
        self.taken.set(true);
        Some(unsafe { (*self.value.get()).write(f()) })
    }

    #[cfg(test)]
    pub fn release(&self) {
        self.taken.set(false);
    }
}

#[cfg(test)]
#[path = "tests/cell.rs"]
mod tests;
