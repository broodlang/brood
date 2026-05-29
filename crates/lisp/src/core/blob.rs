//! Shared, refcounted heap for large immutable byte blobs.
//!
//! Strings allocated in a process LOCAL heap above [`SHARED_BLOB_THRESHOLD`]
//! bytes are routed here instead of being copied inline. Send between
//! processes then bumps an atomic refcount instead of deep-copying the bytes;
//! handle death (process exit, a collection dropping a non-surviving slot) drops
//! the `Arc` and frees the blob when the last reference goes.
//!
//! ADR-026 makes data immutable, so there are no cycles — a plain `Arc` is
//! sound (no `Weak`, no cycle collector). ADR-033's closure-as-data already
//! proved cross-process handle retag works for immutable code; this is the
//! same pattern for bulk byte data.

use std::sync::Arc;

#[cfg(debug_assertions)]
use std::sync::atomic::{AtomicUsize, Ordering};

/// Strings of this size or larger route through [`BlobHeap`] instead of being
/// stored inline in the per-process slab. Below the threshold, inline storage
/// is cheaper than the atomic refcount traffic + extra indirection. Mid-range
/// starting value; tunable from one place.
pub const SHARED_BLOB_THRESHOLD: usize = 256;

/// An immutable byte blob shared across processes within one runtime.
///
/// The bytes are always valid UTF-8 by construction — every allocation site
/// goes through [`crate::core::heap::Heap::alloc_string`] which accepts only
/// `&str`. Readers may rely on this invariant.
pub struct SharedBlob {
    pub(crate) bytes: Box<[u8]>,
}

impl SharedBlob {
    pub fn new(bytes: &[u8]) -> Arc<Self> {
        Arc::new(SharedBlob {
            bytes: bytes.into(),
        })
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }
}

/// Per-runtime registry for shared blobs. Phase 1 holds only a debug-only
/// liveness counter; future phases may add interning, content-addressed
/// dedup, or cross-runtime serialization hooks here without churning
/// callers.
///
/// Lives behind `Arc<BlobHeap>` alongside `Arc<RuntimeCode>` and
/// `Arc<SharedCode>`, so every spawned process in the runtime sees the same
/// heap. Crossing a runtime boundary (dist wire) deep-copies the bytes;
/// `Arc<SharedBlob>` never escapes its originating runtime.
#[derive(Default)]
pub struct BlobHeap {
    #[cfg(debug_assertions)]
    live_count: AtomicUsize,
}

impl BlobHeap {
    pub fn new() -> Arc<Self> {
        Arc::new(BlobHeap::default())
    }

    #[cfg(debug_assertions)]
    pub fn note_alloc(&self) {
        self.live_count.fetch_add(1, Ordering::Relaxed);
    }

    #[cfg(debug_assertions)]
    pub fn note_free(&self) {
        self.live_count.fetch_sub(1, Ordering::Relaxed);
    }

    #[cfg(debug_assertions)]
    pub fn live_count(&self) -> usize {
        self.live_count.load(Ordering::Relaxed)
    }
}
