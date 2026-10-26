//! Heap accounting: a global allocator that counts what the program allocates.
//!
//! A program opts in by installing it:
//!
//! ```ignore
//! #[global_allocator]
//! static ALLOCATOR: ab_core::memory::CountingAllocator = ab_core::memory::CountingAllocator;
//! ```
//!
//! Then [`stats`] says how much is allocated now, at most so far, and how many allocations
//! and frees there have been. Counting is a few relaxed atomic additions per allocation, so
//! it can stay on in development builds. Without it installed, [`stats`] reports
//! `installed: false` and zeros.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static FREES: AtomicU64 = AtomicU64::new(0);
static ALLOCATED: AtomicU64 = AtomicU64::new(0);

/// The system allocator, counted (see the module docs).
#[derive(Debug, Default, Clone, Copy)]
pub struct CountingAllocator;

fn added(size: usize) {
    let live = LIVE.fetch_add(size, Ordering::Relaxed) + size;
    PEAK.fetch_max(live, Ordering::Relaxed);
    ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
    ALLOCATED.fetch_add(size as u64, Ordering::Relaxed);
}

fn removed(size: usize) {
    LIVE.fetch_sub(size, Ordering::Relaxed);
    FREES.fetch_add(1, Ordering::Relaxed);
}

// SAFETY: every call goes straight to `System`, which upholds `GlobalAlloc`'s contract; the
// counters only observe sizes.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller's layout is passed on unchanged.
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            added(layout.size());
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: as `alloc`.
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            added(layout.size());
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` was allocated by `System` with `layout` (every allocation goes there).
        unsafe { System.dealloc(ptr, layout) };
        removed(layout.size());
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: as `dealloc`, and `new_size` is the caller's.
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            removed(layout.size());
            added(new_size);
        }
        p
    }
}

/// What the [`CountingAllocator`] counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AllocStats {
    /// The allocator is installed (else everything is 0).
    pub installed: bool,
    /// Bytes allocated now.
    pub live: usize,
    /// The most bytes allocated at once so far.
    pub peak: usize,
    /// Allocations so far (a reallocation counts as one more).
    pub allocations: u64,
    pub frees: u64,
    /// Bytes allocated so far, freed or not.
    pub allocated: u64,
}

impl AllocStats {
    /// Allocations not freed yet.
    pub fn outstanding(&self) -> u64 {
        self.allocations.saturating_sub(self.frees)
    }
}

/// The allocator's counts now.
pub fn stats() -> AllocStats {
    let allocations = ALLOCATIONS.load(Ordering::Relaxed);
    AllocStats {
        installed: allocations > 0,
        live: LIVE.load(Ordering::Relaxed),
        peak: PEAK.load(Ordering::Relaxed),
        allocations,
        frees: FREES.load(Ordering::Relaxed),
        allocated: ALLOCATED.load(Ordering::Relaxed),
    }
}

/// `bytes` for people: `1.5 MB`.
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_follow_allocations() {
        // The test binary does not install it: count through it by hand.
        let a = CountingAllocator;
        let before = stats();
        let layout = Layout::from_size_align(4096, 8).unwrap();
        // SAFETY: a valid layout; the block is freed with the same one.
        unsafe {
            let p = a.alloc(layout);
            assert!(!p.is_null());
            let mid = stats();
            assert_eq!(mid.live, before.live + 4096);
            assert!(mid.peak >= mid.live);
            let p = a.realloc(p, layout, 8192);
            assert_eq!(stats().live, before.live + 8192);
            a.dealloc(p, Layout::from_size_align(8192, 8).unwrap());
        }
        let after = stats();
        assert_eq!(after.live, before.live);
        assert_eq!(after.allocations, before.allocations + 2);
        assert_eq!(after.frees, before.frees + 2);
        assert_eq!(human_bytes(1536), "1.5 KB");
        assert_eq!(human_bytes(12), "12 B");
    }
}
