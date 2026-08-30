//! The GC root stack's storage: a `Vec<Value>` work-alike whose `(ptr, len, cap)` header
//! sits at **fixed offsets** (`#[repr(C)]`) — compute-frontier §7.5 increment 1. A
//! `Vec`'s internal layout is unspecified, so JIT-emitted code could never read or adjust
//! the frame extent directly; this buffer is the substrate that lets the fast-frame call
//! ceremony (`jit_run_fast_link`) eventually move into emitted CLIF. Until that lands,
//! nothing outside this module depends on the layout — the swap must be behaviorally
//! invisible (validated under `BROOD_GC_STRESS`/`BROOD_GC_VERIFY` + the full suite).
//!
//! Deliberately NOT Box- or Vec-backed with a cached raw pointer: a self-referential
//! pointer derived from a `Box` goes stale under stacked borrows the moment the struct
//! moves (a `Heap` lives inline in `Box<Process>` and moves). The allocation is owned
//! raw, its provenance from `std::alloc` directly, which survives moves of the header.
//! `Value` is `Copy` (no drop glue), so grow/drop are plain byte copies + one `dealloc`.

use crate::core::value::Value;
use std::alloc::{alloc, dealloc, handle_alloc_error, Layout};

/// Field order is load-bearing: `ptr` at 0, `len` at 8, `cap` at 16 (see module doc).
/// [`Self::header_offsets`] asserts it, so a reorder cannot ship silently.
#[repr(C)]
pub(crate) struct RootsBuf {
    ptr: *mut Value,
    len: usize,
    cap: usize,
}

// SAFETY: the buffer is uniquely owned (no aliasing handles escape long-term — `as_mut_ptr`
// callers re-fetch after any growth, the same contract `Vec` had), and `Value` is `Send`.
// A `Heap` moves between scheduler workers inside its `Box<Process>`, which is exactly the
// auto-impl the `Vec<Value>` field used to provide.
unsafe impl Send for RootsBuf {}

impl RootsBuf {
    pub(crate) const fn new() -> Self {
        RootsBuf {
            ptr: std::ptr::NonNull::<Value>::dangling().as_ptr(),
            len: 0,
            cap: 0,
        }
    }

    /// The `(ptr, len, cap)` byte offsets within the struct, for the JIT lowering.
    /// Compile-time constant; the `const` assertions pin the `#[repr(C)]` layout.
    #[allow(dead_code)] // consumed by §7.5 increment 2 (the inline fast-frame emission)
    pub(crate) const fn header_offsets() -> (usize, usize, usize) {
        let o = (
            std::mem::offset_of!(RootsBuf, ptr),
            std::mem::offset_of!(RootsBuf, len),
            std::mem::offset_of!(RootsBuf, cap),
        );
        assert!(o.0 == 0 && o.1 == 8 && o.2 == 16);
        o
    }

    #[inline(always)]
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// Set the length directly. Same contract as `Vec::set_len`.
    ///
    /// # Safety
    /// `n <= self.cap`, and elements `..n` must be initialized.
    #[inline(always)]
    pub(crate) unsafe fn set_len(&mut self, n: usize) {
        debug_assert!(n <= self.cap);
        self.len = n;
    }

    #[inline(always)]
    pub(crate) fn as_mut_ptr(&mut self) -> *mut Value {
        self.ptr
    }

    #[inline(always)]
    pub(crate) fn truncate(&mut self, n: usize) {
        // `Value` has no drop glue, so truncation is a length store (like `Vec`'s would
        // optimize to, minus the drop loop the optimizer had to prove away).
        if n < self.len {
            self.len = n;
        }
    }

    #[inline(always)]
    pub(crate) fn push(&mut self, v: Value) {
        if self.len == self.cap {
            self.grow(self.len + 1);
        }
        // SAFETY: len < cap after the grow; the slot is in-bounds.
        unsafe { self.ptr.add(self.len).write(v) };
        self.len += 1;
    }

    /// Ensure room for `additional` more elements past `len` (amortized doubling,
    /// `Vec::reserve` semantics — including the panic on overflow, which must not
    /// wrap into a too-small allocation).
    #[inline]
    pub(crate) fn reserve(&mut self, additional: usize) {
        let needed = self
            .len
            .checked_add(additional)
            .expect("roots capacity overflow");
        if needed > self.cap {
            self.grow(needed);
        }
    }

    /// Release memory past `len`. Called at quiesce points only (process teardown /
    /// post-collect shrink), so a plain realloc-down is fine.
    pub(crate) fn shrink_to_fit(&mut self) {
        if self.cap > self.len {
            self.realloc_to(self.len);
        }
    }

    #[cold]
    fn grow(&mut self, needed: usize) {
        // Amortized doubling with `Vec`'s small-first policy for a 24-byte element.
        let new_cap = needed.max(self.cap * 2).max(4);
        self.realloc_to(new_cap);
    }

    fn realloc_to(&mut self, new_cap: usize) {
        debug_assert!(new_cap >= self.len);
        let old_ptr = self.ptr;
        let old_cap = self.cap;
        if new_cap == 0 {
            if old_cap > 0 {
                // SAFETY: `old_ptr` was allocated with exactly this layout.
                unsafe { dealloc(old_ptr as *mut u8, Self::layout(old_cap)) };
            }
            self.ptr = std::ptr::NonNull::<Value>::dangling().as_ptr();
            self.cap = 0;
            return;
        }
        let layout = Self::layout(new_cap);
        let new_ptr = if old_cap > 0 {
            // SAFETY: `old_ptr` was allocated with `layout(old_cap)`; `realloc` copies
            // `min(old, new)` bytes and can extend in place — the same call `Vec` makes.
            unsafe { std::alloc::realloc(old_ptr as *mut u8, Self::layout(old_cap), layout.size()) }
        } else {
            // SAFETY: `layout` is non-zero-size (new_cap > 0, Value is 24 bytes).
            unsafe { alloc(layout) }
        } as *mut Value;
        if new_ptr.is_null() {
            handle_alloc_error(layout);
        }
        self.ptr = new_ptr;
        self.cap = new_cap;
    }

    fn layout(cap: usize) -> Layout {
        Layout::array::<Value>(cap).expect("roots capacity overflows a Layout")
    }
}

impl Drop for RootsBuf {
    fn drop(&mut self) {
        if self.cap > 0 {
            // SAFETY: allocated with exactly this layout; `Value` needs no drop glue.
            unsafe { dealloc(self.ptr as *mut u8, Self::layout(self.cap)) };
        }
    }
}

impl std::ops::Deref for RootsBuf {
    type Target = [Value];
    #[inline(always)]
    fn deref(&self) -> &[Value] {
        // SAFETY: `..len` is initialized (every write path initializes before or as it
        // raises `len`); `ptr` is valid for `cap >= len` elements (dangling only at len 0,
        // where an empty slice from a dangling-but-aligned pointer is allowed).
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl std::ops::DerefMut for RootsBuf {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut [Value] {
        // SAFETY: as in `Deref`, plus unique access via `&mut self`.
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_offsets_are_pinned() {
        assert_eq!(RootsBuf::header_offsets(), (0, 8, 16));
    }

    #[test]
    fn push_grow_truncate_roundtrip() {
        let mut b = RootsBuf::new();
        assert_eq!(b.len(), 0);
        for i in 0..100 {
            b.push(Value::Int(i));
        }
        assert_eq!(b.len(), 100);
        assert!(matches!(b[42], Value::Int(42)));
        b[42] = Value::Int(-1);
        assert!(matches!(b[42], Value::Int(-1)));
        b.truncate(10);
        assert_eq!(b.len(), 10);
        b.truncate(50); // no-op past len
        assert_eq!(b.len(), 10);
        b.shrink_to_fit();
        assert_eq!(b.len(), 10);
        assert!(matches!(b[9], Value::Int(9)));
        // reserve + set_len mirrors extend_roots_to_nil's raw pattern
        b.reserve(20);
        unsafe {
            std::ptr::write_bytes(b.as_mut_ptr().add(10), 0, 20);
            b.set_len(30);
        }
        assert_eq!(b.len(), 30);
        let s: &[Value] = &b;
        assert_eq!(s.len(), 30);
    }

    #[test]
    fn iteration_matches_slice_semantics() {
        let mut b = RootsBuf::new();
        for i in 0..5 {
            b.push(Value::Int(i));
        }
        let sum: i64 = b
            .iter()
            .map(|v| if let Value::Int(i) = v { *i } else { 0 })
            .sum();
        assert_eq!(sum, 10);
        for v in b.iter_mut() {
            if let Value::Int(i) = v {
                *i += 1;
            }
        }
        assert!(matches!(b[0], Value::Int(1)));
        // `for &v in &b` — the pattern gc_runtime uses
        let mut count = 0;
        for &v in b.iter() {
            let _ = v;
            count += 1;
        }
        assert_eq!(count, 5);
    }
}
