//! A process-wide allocator that counts bytes, so Brood can report how much
//! memory a piece of work used. It wraps the system allocator and keeps two
//! running totals — current live bytes and the high-water mark — updated on
//! every (de)allocation with relaxed atomics (cheap; ordering between the two
//! counters doesn't matter, only their individual values).
//!
//! Installed as `#[global_allocator]` in `lib.rs`, so it covers *every* Rust
//! allocation in the process (the interpreter included), not just Brood values.
//! For "how much memory did this run use," that whole-process number is the one
//! you want. The `(mem-bytes)` / `(mem-peak)` primitives in `builtins.rs` read
//! these counters. Dependency-free (std only), per ADR-005.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

static LIVE: AtomicUsize = AtomicUsize::new(0); // bytes currently allocated
static PEAK: AtomicUsize = AtomicUsize::new(0); // high-water mark of LIVE

/// System allocator wrapper that tallies bytes in/out.
pub struct Counting;

fn record_alloc(size: usize) {
    let live = LIVE.fetch_add(size, Ordering::Relaxed) + size;
    PEAK.fetch_max(live, Ordering::Relaxed);
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc(layout);
        if !ptr.is_null() {
            record_alloc(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout);
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_ptr = System.realloc(ptr, layout, new_size);
        if !new_ptr.is_null() {
            let old = layout.size();
            if new_size >= old {
                record_alloc(new_size - old);
            } else {
                LIVE.fetch_sub(old - new_size, Ordering::Relaxed);
            }
        }
        new_ptr
    }
}

/// Bytes currently allocated across the whole process.
pub fn live_bytes() -> usize {
    LIVE.load(Ordering::Relaxed)
}

/// The largest [`live_bytes`] has ever been (since process start).
pub fn peak_bytes() -> usize {
    PEAK.load(Ordering::Relaxed)
}
