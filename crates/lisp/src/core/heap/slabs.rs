//! The slab substrate — child of heap.
//!
//! Where LOCAL and shared values physically live: [`VecStore`] (a vector's elements, inline in
//! the slab slot up to [`INLINE_VEC_CAP`] and spilled to a `Vec` past it), [`Slabs`] (one `Vec`
//! per kind — the LOCAL nursery/old generations and the frozen PRELUDE are all this shape) with
//! the live-count / capacity / park-trim helpers the collector's sizing reads, [`CodeSlabs`]
//! (the append-only `boxcar` slabs of one RUNTIME code generation). An accessor's borrow into
//! a RUNTIME generation is a plain `&T`: `Heap::code_gen_ref`'s per-process cache keeps every
//! generation it has handed out alive until `&mut self`. Fields and constructors are
//! `pub(super)` because `heap.rs`, `gc.rs` and `gc_runtime.rs` address the slabs directly;
//! `heap.rs` re-exports `VecStore` and `INLINE_VEC_CAP`.

use super::*;

/// How many elements a vector stores **inline in its slab slot** before it
/// spills to a heap `Vec` (see [`VecStore`]). Set to 2 — the hot small-vector
/// case (2-element tuples like `bintree` nodes, 2-element `SeqView` backings) —
/// kept small so an inline slot (~64 B) stays *below* the old `Vec<Value>`
/// handle-plus-`malloc` footprint (a 24 B slot + a ≥48 B heap block). A larger
/// cap would inline 3-element `Range` backings too, but at a bigger slot for
/// *every* vector, which added GC-copy traffic on the copy-bound `bintree`
/// (whose whole tree is live at once) — net CPU-negative in measurement.
pub(crate) const INLINE_VEC_CAP: usize = 2;

/// Element storage for one heap vector: **inline** in the slab slot for the
/// common small case, or **spilled** to a heap `Vec` for larger vectors. This
/// replaced a bare `Vec<Value>` per slot (`vectors: Vec<Vec<Value>>`), which
/// paid a `malloc` on *every* vector allocation and forced element reads through
/// a double indirection the JIT couldn't inline. A small vector is now a plain
/// bump-push (like a pair), and its elements sit at a fixed offset in the slot
/// so the JIT can inline the read the way it inlines a pair car/cdr.
///
/// An **enum** (not a struct with an always-present spill field), so the inline
/// and spill forms share storage and a small vector costs no more than its two
/// `Value`s plus a length. Both present as `&[Value]` through [`Deref`]/
/// [`DerefMut`], so every reader (accessors, GC, message copy, builtins) is
/// oblivious to which form backs a given vector. `#[repr(u8)]` pins the layout
/// (tag byte at 0; the `Inline` variant's `len` at 8, `items` at 16) for the
/// JIT's inline element read — see `jit_lower.rs`.
#[repr(u8)]
pub(crate) enum VecStore {
    Inline {
        len: u8,
        items: [Value; INLINE_VEC_CAP],
    },
    // Rust never reads `ptr`/`len` by name — JIT-lowered native code loads them
    // through the `#[repr(u8)]`-pinned byte offsets (see jit_lower.rs), which
    // dead_code analysis can't see.
    #[allow(dead_code)]
    Spill {
        /// Cached `vec.as_ptr()`, so the JIT reads spilled elements through one
        /// raw load instead of an FFI slab call (the ~20 ns/element that gated
        /// nbody's field reads and the json/regex code-vector scans). Sound
        /// because a spilled buffer never moves: vector contents are immutable
        /// (never pushed/resized after construction; `DerefMut` element writes
        /// don't reallocate), and moving the `VecStore` struct itself (slab
        /// growth, GC copy) moves three words — not the heap buffer they point
        /// to. A GC relocation builds a NEW store via [`VecStore::spill`], which
        /// re-derives the pointer.
        ptr: *const Value,
        /// Cached element count for the JIT's bounds check.
        len: u64,
        vec: Vec<Value>,
    },
    /// The first `len` elements of an [`AppendBuf`] this vector may share with the vectors
    /// appended from it (ADR-511), so a `conj` onto the newest one writes one element
    /// instead of copying them all. `ptr` and `len` sit where `Spill`'s do, so the JIT
    /// reads both the same way.
    #[allow(dead_code)]
    Shared {
        ptr: *const Value,
        len: u64,
        buf: Arc<AppendBuf>,
    },
}

/// The buffer a [`VecStore::Shared`] vector reads its elements from (ADR-511): a `Vec`
/// whose length is how far it is CLAIMED — the longest vector built over it — and whose
/// spare capacity is room for the next. Every vector over it reads only its own first `len`
/// elements, which never change once written. An append writes past the claimed length,
/// where no vector over the buffer reads, and only when the vector appended to ends exactly
/// there: two `conj`s onto one vector cannot both claim a slot, because the second finds it
/// claimed and copies. No value is ever changed — the buffer only grows into space nothing
/// can see.
///
/// One heap holds it, and so one thread: the buffer is LOCAL-only. A promotion, a message
/// and a [`VecStore`] clone all copy the elements out, and the shared regions build their
/// own stores, so no other heap ever reaches it.
pub(crate) struct AppendBuf {
    items: std::cell::UnsafeCell<Vec<Value>>,
}

// SAFETY: see the type's doc — one heap holds the buffer, and a heap runs on one thread at a
// time; moving the heap to another worker moves the buffer with it, never shares it.
unsafe impl Send for AppendBuf {}
// SAFETY: as for `Send` — no two threads ever hold a reference to the same buffer.
unsafe impl Sync for AppendBuf {}

impl AppendBuf {
    pub(super) fn new(items: Vec<Value>) -> Arc<Self> {
        Arc::new(AppendBuf {
            items: std::cell::UnsafeCell::new(items),
        })
    }

    fn items(&self) -> &Vec<Value> {
        // SAFETY: one thread (the type's doc), and no `&mut` to the `Vec` outlives
        // `append_in_place`.
        unsafe { &*self.items.get() }
    }

    pub(super) fn as_ptr(&self) -> *const Value {
        self.items().as_ptr()
    }

    pub(super) fn capacity(&self) -> usize {
        self.items().capacity()
    }

    /// How far the buffer is claimed: the length of the longest vector built over it.
    pub(super) fn claimed(&self) -> usize {
        self.items().len()
    }

    /// Append `tail` after the first `len` elements, in place: only when nothing is claimed
    /// past `len` and `tail` fits without moving the buffer. `false` means copy instead.
    pub(super) fn append_in_place(&self, len: usize, tail: &[Value]) -> bool {
        // SAFETY: one thread (the type's doc). Readers go through a vector's cached `ptr`
        // and `len`, never this `Vec`, and the capacity check rules out the reallocation
        // that would move what they read.
        let items = unsafe { &mut *self.items.get() };
        if items.len() != len || items.capacity() - len < tail.len() {
            return false;
        }
        items.extend_from_slice(tail);
        true
    }

    /// The elements, cut to `len`, when `this` is the buffer's only holder — the collector
    /// relocating the last vector over it, which then rewrites them in place.
    pub(super) fn into_unique(this: Arc<Self>, len: usize) -> Result<Vec<Value>, Arc<Self>> {
        Arc::try_unwrap(this).map(|buf| {
            let mut items = buf.items.into_inner();
            items.truncate(len);
            items
        })
    }
}

// SAFETY: `Spill::ptr` always points into `Spill::vec`'s own buffer (established
// by the one constructor and re-derived on clone), so it is exactly as sendable/
// sharable as the `Vec` it caches.
unsafe impl Send for VecStore {}
// SAFETY: as for `Send` — the cached pointer is no more shareable than the `Vec` it points into.
unsafe impl Sync for VecStore {}

impl Clone for VecStore {
    fn clone(&self) -> Self {
        match self {
            VecStore::Inline { len, items } => VecStore::Inline {
                len: *len,
                items: *items,
            },
            // NOT derived: a derived clone would copy `ptr` — pointing the clone
            // at the ORIGINAL buffer. Re-derive from the cloned Vec.
            VecStore::Spill { vec, .. } => VecStore::spill(vec.clone()),
            // A copy of its own elements: a clone may land in another heap, and an append
            // buffer never leaves the one it was made in (`AppendBuf`).
            VecStore::Shared { .. } => VecStore::spill(self.as_slice().to_vec()),
        }
    }
}

impl VecStore {
    /// The bytes of this vector's own buffer: a spilled vector's elements, nothing for an
    /// inline one (its elements live in the slab entry). What the collector's byte trigger
    /// counts for a vector, and [`owned_bytes`] sums.
    #[inline]
    pub(super) fn buffer_bytes(&self) -> usize {
        match self {
            VecStore::Spill { vec, .. } => vec.capacity() * std::mem::size_of::<Value>(),
            VecStore::Shared { buf, .. } => buf.capacity() * std::mem::size_of::<Value>(),
            VecStore::Inline { .. } => 0,
        }
    }

    /// The one `Spill` constructor: caches the buffer pointer + length.
    #[inline]
    pub(super) fn spill(vec: Vec<Value>) -> Self {
        VecStore::Spill {
            ptr: vec.as_ptr(),
            len: vec.len() as u64,
            vec,
        }
    }

    /// A vector of the first `len` elements of `buf`, which must hold at least that many.
    #[inline]
    pub(super) fn shared(buf: Arc<AppendBuf>, len: usize) -> Self {
        VecStore::Shared {
            ptr: buf.as_ptr(),
            len: len as u64,
            buf,
        }
    }

    /// Wrap owned elements, inlining when they fit (no heap allocation) and
    /// spilling otherwise. Consumes `items` so the spill path is a move, not a copy.
    #[inline]
    pub(super) fn from_vec(items: Vec<Value>) -> Self {
        if items.len() <= INLINE_VEC_CAP {
            let mut inline = [Value::nil(); INLINE_VEC_CAP];
            inline[..items.len()].copy_from_slice(&items);
            VecStore::Inline {
                len: items.len() as u8,
                items: inline,
            }
        } else {
            VecStore::spill(items)
        }
    }

    /// Build from a known element count + a per-index producer, inlining without
    /// a temporary `Vec` when it fits. The GC copy path ([`flush_vector`]) uses
    /// this so relocating a small survivor allocates nothing.
    #[inline]
    pub(super) fn from_flushed(len: usize, mut producer: impl FnMut(usize) -> Value) -> Self {
        if len <= INLINE_VEC_CAP {
            let mut inline = [Value::nil(); INLINE_VEC_CAP];
            for (i, slot) in inline[..len].iter_mut().enumerate() {
                *slot = producer(i);
            }
            VecStore::Inline {
                len: len as u8,
                items: inline,
            }
        } else {
            VecStore::spill(Vec::from_fn(len, producer))
        }
    }

    #[inline]
    pub(super) fn as_slice(&self) -> &[Value] {
        match self {
            VecStore::Inline { len, items } => &items[..*len as usize],
            VecStore::Spill { vec, .. } => vec,
            // SAFETY: `ptr` is the buffer's start and its first `len` elements are written
            // and never change (`AppendBuf`); the buffer lives as long as `buf` does.
            VecStore::Shared { ptr, len, .. } => unsafe {
                std::slice::from_raw_parts(*ptr, *len as usize)
            },
        }
    }

    /// The elements, writable — by a builder before the vector is visible, or the collector
    /// rewriting handles it relocated. A `Shared` vector is first given a buffer of its own
    /// unless it already holds its buffer alone: a write must never reach a vector that
    /// shares it.
    #[inline]
    pub(super) fn as_mut_slice(&mut self) -> &mut [Value] {
        if let VecStore::Shared { buf, .. } = self
            && Arc::get_mut(buf).is_none()
        {
            *self = VecStore::spill(self.as_slice().to_vec());
        }
        match self {
            VecStore::Inline { len, items } => &mut items[..*len as usize],
            VecStore::Spill { vec, .. } => vec,
            // SAFETY: as in `as_slice`, and `Arc::get_mut` above found no other holder, so
            // nothing else reads these elements.
            VecStore::Shared { ptr, len, .. } => unsafe {
                std::slice::from_raw_parts_mut(*ptr as *mut Value, *len as usize)
            },
        }
    }

    // ---- Byte layout for the JIT's inline element read (jit_lower.rs) ----
    // A `#[repr(u8)]` enum is laid out per RFC 2195 as a union of repr(C)
    // variant-structs, each prefixed by the u8 discriminant. So for the `Inline`
    // variant: discriminant @0, `len` @1, `items` @8 (8-aligned). The
    // discriminant of the first variant (`Inline`) is 0. `JIT_STRIDE` is the
    // slab stride. These are asserted against reality by `vecstore_jit_layout`.

    /// Slab stride: bytes per `VecStore` slot.
    #[cfg(feature = "jit")]
    pub(crate) const JIT_STRIDE: i64 = std::mem::size_of::<VecStore>() as i64;
    /// Discriminant byte offset within a slot.
    #[cfg(feature = "jit")]
    pub(crate) const JIT_TAG_OFF: i32 = 0;
    /// Discriminant value that means `Inline` (inline-readable).
    #[cfg(feature = "jit")]
    pub(crate) const JIT_INLINE_TAG: i64 = 0;
    /// `Inline.len` (u8) byte offset within a slot.
    #[cfg(feature = "jit")]
    pub(crate) const JIT_LEN_OFF: i32 = 1;
    /// `Inline.items[0]` byte offset within a slot.
    #[cfg(feature = "jit")]
    pub(crate) const JIT_ITEMS_OFF: i32 = 8;
    /// Discriminant value that means `Spill` (pointer-readable).
    #[cfg(feature = "jit")]
    pub(crate) const JIT_SPILL_TAG: i64 = 1;
    /// Discriminant value that means `Shared`, pointer-readable exactly like `Spill`: its
    /// `ptr` and `len` sit at the same offsets (ADR-511).
    #[cfg(feature = "jit")]
    pub(crate) const JIT_SHARED_TAG: i64 = 2;
    /// `Spill.ptr` byte offset within a slot (u8 tag, padded to the pointer's align).
    #[cfg(feature = "jit")]
    pub(crate) const JIT_SPILL_PTR_OFF: i32 = 8;
    /// `Spill.len` byte offset within a slot.
    #[cfg(feature = "jit")]
    pub(crate) const JIT_SPILL_LEN_OFF: i32 = 16;
}

// The JIT reads a vector through its cached `ptr`/`len` when the tag is at least `Spill`'s
// (`jit_lower::emit`), which is right only while `Inline` sorts first and `Spill` and `Shared`
// after it.
#[cfg(feature = "jit")]
const _: () = assert!(
    VecStore::JIT_INLINE_TAG < VecStore::JIT_SPILL_TAG
        && VecStore::JIT_SPILL_TAG < VecStore::JIT_SHARED_TAG
);

impl std::ops::Deref for VecStore {
    type Target = [Value];
    #[inline]
    fn deref(&self) -> &[Value] {
        self.as_slice()
    }
}

impl std::ops::DerefMut for VecStore {
    #[inline]
    fn deref_mut(&mut self) -> &mut [Value] {
        self.as_mut_slice()
    }
}

/// A slab for a kind most processes never allocate — bigints, decimals, ratios, ropes,
/// local natives: one pointer until the first push, where a `Vec` is three words. Every
/// process carries two sets of slabs (nursery and old generation), so five of these are
/// 80 bytes a set off the floor of a process that holds none of them (`process_floor.rs`).
/// Reads go through the slice it dereferences to, empty while unallocated.
#[allow(clippy::box_collection)] // the box is the point: one word until the first push
pub(super) struct LazyVec<T>(Option<Box<Vec<T>>>);

impl<T> Default for LazyVec<T> {
    fn default() -> Self {
        LazyVec(None)
    }
}

impl<T> LazyVec<T> {
    pub(super) fn with_capacity(n: usize) -> Self {
        LazyVec((n > 0).then(|| Box::new(Vec::with_capacity(n))))
    }

    #[inline]
    pub(super) fn push(&mut self, value: T) {
        self.0.get_or_insert_with(Default::default).push(value);
    }

    pub(super) fn capacity(&self) -> usize {
        self.0.as_ref().map_or(0, |v| v.capacity())
    }

    pub(super) fn truncate(&mut self, len: usize) {
        if let Some(v) = self.0.as_mut() {
            v.truncate(len);
        }
    }

    /// Releases the box too once empty, so a process that allocated one bigint long ago
    /// goes back to costing one pointer.
    pub(super) fn shrink_to_fit(&mut self) {
        match self.0.as_mut() {
            Some(v) if v.is_empty() => self.0 = None,
            Some(v) => v.shrink_to_fit(),
            None => {}
        }
    }
}

impl<T> std::ops::Deref for LazyVec<T> {
    type Target = [T];
    #[inline]
    fn deref(&self) -> &[T] {
        self.0.as_deref().map_or(&[], |v| v.as_slice())
    }
}

impl<T> std::ops::DerefMut for LazyVec<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut [T] {
        match self.0.as_deref_mut() {
            Some(v) => v.as_mut_slice(),
            None => &mut [],
        }
    }
}

/// The slabs holding heap objects in the LOCAL data heap and the PRELUDE region.
#[derive(Default)]
pub(super) struct Slabs {
    pub(super) pairs: Vec<(Value, Value)>,
    pub(super) vectors: Vec<VecStore>,
    /// Maps as a flat slab of CHAMP nodes (ADR-040). Each [`MapNode`] is
    /// either a branch (two bitmaps + packed data/children arrays) or a
    /// max-depth collision leaf. The handle in `Value::Map(MapId)` points
    /// at the trie's *root* node; child sub-nodes live in the same slab,
    /// referenced by `MapId`. The root is the only entry-point — internal
    /// nodes are reachable only through the trie itself.
    pub(super) maps: Vec<MapNode>,
    pub(super) strings: Vec<LocalString>,
    /// Arbitrary-precision integers (the bignum leaf, mirrors `strings`). One
    /// `num_bigint::BigInt` per live value that overflowed i64; immutable, holds
    /// no `Value` children. Every entry satisfies the normalize invariant
    /// (strictly outside the i64 range) — `Heap::int_from_bigint` enforces it.
    pub(super) bigints: LazyVec<num_bigint::BigInt>,
    /// Arbitrary-precision base-10 decimals (mirrors `bigints` exactly). One
    /// `bigdecimal::BigDecimal` per live `Value::Decimal`; immutable, holds no
    /// `Value` children. Unlike `bigints` there is no normalize-into-`Int`
    /// invariant — a decimal is its own type and any value is stored as-is.
    pub(super) decimals: LazyVec<bigdecimal::BigDecimal>,
    /// Exact rationals (mirrors `decimals`). One `num_rational::BigRational` per live
    /// `Value::Ratio`; immutable, holds no `Value` children. Always reduced with a
    /// positive denominator; a denominator of 1 is demoted to `Int` at construction
    /// (`Heap::alloc_ratio`), so no entry here is ever integer-valued.
    pub(super) ratios: LazyVec<num_rational::BigRational>,
    /// **Raw bytes** — byte-clean immutable leaves, one `Arc<SharedBlob>` per live
    /// value (arbitrary bytes, never UTF-8, own slab + handle). The `Arc` is the unit
    /// of cross-process sharing (a refcount bump, not a byte copy).
    pub(super) bytes: Vec<Arc<SharedBlob>>,
    /// Text ropes (ADR-045). A `ropey::Rope` is itself `Arc`-shared internally,
    /// so this slab owns one cheap handle per live rope; cloning for an edit
    /// bumps refcounts, not bytes. Always inline (no SharedBlob split — ropes
    /// don't cross processes, so there's no cross-heap aliasing to optimise).
    pub(super) ropes: LazyVec<ropey::Rope>,
    pub(super) closures: Vec<Closure>,
    pub(super) natives: LazyVec<NativeFn>,
    pub(super) envs: Vec<EnvFrame>,
}

/// Live object count of a [`Slabs`] (`Σ slab.len()`). The collector is a moving
/// copy collector that never reuses a slot in place — survivors are relocated
/// into fresh slabs and the dead dropped wholesale — so there is no free list to
/// subtract and the slab lengths *are* the live count. Shared by both
/// [`Heap::local_live_count`] (the nursery) and [`Heap::old_live_count`] (the old
/// gen), which were identical sums. `natives` is excluded (it's never GC'd — see
/// the byte-weighted [`slab_bytes`], which does count it for footprint).
impl Slabs {
    /// How many objects the slab holding `kind` has — the bound a valid handle's index
    /// stays under.
    pub(super) fn len_of(&self, kind: crate::core::value::HandleKind) -> usize {
        use crate::core::value::HandleKind;
        match kind.slab() {
            HandleKind::Pair => self.pairs.len(),
            HandleKind::Vector => self.vectors.len(),
            HandleKind::Map => self.maps.len(),
            HandleKind::Str => self.strings.len(),
            HandleKind::BigInt => self.bigints.len(),
            HandleKind::Decimal => self.decimals.len(),
            HandleKind::Ratio => self.ratios.len(),
            HandleKind::Bytes => self.bytes.len(),
            HandleKind::Rope => self.ropes.len(),
            HandleKind::Fn => self.closures.len(),
            other => unreachable!("{} is not a slab of its own", other.name()),
        }
    }

    /// A fresh, empty `Slabs` whose per-slab `Vec`s carry the **capacity of
    /// `like`'s lengths** — the flip-side nursery allocator. A zero-capacity
    /// nursery would re-pay the full Vec-doubling ladder every cycle (each
    /// doubling memmoves everything allocated so far). The previous
    /// nursery's *lengths* are the steady-state high-water mark (each cycle
    /// allocates about as much as the last), so reserving them up front makes
    /// the next cycle's pushes copy-free while releasing the memory of any
    /// one-off spike (capacity follows the last cycle's actual use, not max).
    pub(super) fn with_capacity_like(like: &Slabs) -> Slabs {
        Slabs {
            pairs: Vec::with_capacity(like.pairs.len()),
            vectors: Vec::with_capacity(like.vectors.len()),
            maps: Vec::with_capacity(like.maps.len()),
            strings: Vec::with_capacity(like.strings.len()),
            bigints: LazyVec::with_capacity(like.bigints.len()),
            decimals: LazyVec::with_capacity(like.decimals.len()),
            ratios: LazyVec::with_capacity(like.ratios.len()),
            bytes: Vec::with_capacity(like.bytes.len()),
            ropes: LazyVec::with_capacity(like.ropes.len()),
            closures: Vec::with_capacity(like.closures.len()),
            natives: LazyVec::default(), // never GC'd, never grows here
            envs: Vec::with_capacity(like.envs.len()),
        }
    }
}

pub(super) fn slab_live_count(s: &Slabs) -> usize {
    s.pairs.len()
        + s.vectors.len()
        + s.maps.len()
        + s.strings.len()
        + s.bigints.len()
        + s.decimals.len()
        + s.ratios.len()
        + s.bytes.len()
        + s.ropes.len()
        + s.closures.len()
        + s.envs.len()
}

/// How much retained capacity must have accumulated **since the last trim** before
/// [`Heap::trim_parked`] does anything.
///
/// An absolute size threshold is wrong in both directions: a high one makes memory
/// non-monotonic in allocation (a process that allocated less can keep more), and a low
/// one makes a process that parks constantly pay a collection per park.
///
/// Growth-since-last-trim gets both. A responder that parks constantly reaches a steady
/// working set, so after one trim its capacity stops growing and it never trims again — it
/// pays one subtraction per park. A process that actually accumulated something trims once
/// per accumulation, regardless of how large its heap is in absolute terms.
///
/// Counted in [`park_trim_probe`] slots rather than bytes so the gate stays a few loads:
/// 64 slots is roughly a few KiB of pairs.
///
/// **And proportional above that** ([`park_trim_due`]): the growth must also reach half
/// the last trim's high-water. A long-lived process that keeps ACCUMULATING — an editor's
/// UI loop, whose caches, buffers and history grow a little with every edit, and which
/// parks after every event — crossed a fresh 64-slot mark every few turns, and each time
/// paid a whole-heap collection at the park and then the slab regrowth after the shrink:
/// 28 trims in a 20-second session, which made its turns cost twice the CPU they cost
/// with collection off. Proportional, a growing heap trims a logarithmic number of times,
/// and an idle one still gives back everything beyond 1.5× what it last settled at.
pub(super) const PARK_TRIM_GROWTH_SLOTS: usize = 64;

/// Whether a parked process whose slab capacity is now `probe` should trim, having last
/// trimmed at high-water `mark` — grown by at least [`PARK_TRIM_GROWTH_SLOTS`] AND by half
/// of `mark` since.
pub(super) fn park_trim_due(probe: usize, mark: usize) -> bool {
    probe.saturating_sub(mark) >= PARK_TRIM_GROWTH_SLOTS.max(mark / 2)
}

/// Retained *capacity* of a slab set, in bytes — what the process is holding from the
/// allocator, as opposed to [`slab_bytes`]'s live contents. The two diverge sharply for a
/// process that allocated and then dropped: the `Vec`s keep their high-water capacity, and a
/// nursery flip deliberately preserves it (`Slabs::with_capacity_like`) so the next cycle
/// does not re-pay the doubling ladder. That is right for a *running* process and wrong for
/// a parked one, which may hold it for the rest of the program.
pub(super) fn slab_capacity_bytes(s: &Slabs) -> usize {
    use std::mem::size_of;
    s.pairs.capacity() * size_of::<(Value, Value)>()
        + s.vectors.capacity() * size_of::<VecStore>()
        + s.maps.capacity() * size_of::<MapNode>()
        + s.strings.capacity() * size_of::<LocalString>()
        + s.bigints.capacity() * size_of::<num_bigint::BigInt>()
        + s.decimals.capacity() * size_of::<bigdecimal::BigDecimal>()
        + s.ratios.capacity() * size_of::<num_rational::BigRational>()
        + s.bytes.capacity() * size_of::<Arc<SharedBlob>>()
        + s.ropes.capacity() * size_of::<ropey::Rope>()
        + s.closures.capacity() * size_of::<Closure>()
        + s.natives.capacity() * size_of::<NativeFn>()
        + s.envs.capacity() * size_of::<EnvFrame>()
}

/// Where a slab set's buffer-owning slabs end: their lengths at one moment. The
/// collector takes one before a tenure and measures [`buffer_bytes_since`] after it, so
/// counting the bytes a tenure added costs what the tenure copied, never the whole old
/// generation. Every generation is append-only between collections, so the entries past
/// the mark are exactly the ones added since.
#[derive(Clone, Copy, Default)]
pub(super) struct BufferMark {
    vectors: usize,
    strings: usize,
    bytes: usize,
}

pub(super) fn buffer_mark(s: &Slabs) -> BufferMark {
    BufferMark {
        vectors: s.vectors.len(),
        strings: s.strings.len(),
        bytes: s.bytes.len(),
    }
}

/// The bytes owned by the buffers of `s`'s entries past `mark`: spilled vectors, string
/// text and byte blobs, the three kinds whose size one object does not bound. With the
/// default mark it is the whole slab set's. O(entries past the mark).
pub(super) fn buffer_bytes_since(s: &Slabs, mark: BufferMark) -> usize {
    let vectors: usize = s.vectors[mark.vectors..]
        .iter()
        .map(VecStore::buffer_bytes)
        .sum();
    let strings: usize = s.strings[mark.strings..]
        .iter()
        .map(LocalString::buffer_bytes)
        .sum();
    let bytes: usize = s.bytes[mark.bytes..].iter().map(|blob| blob.len()).sum();
    vectors + strings + bytes
}

/// Everything a slab set holds, in bytes: [`slab_capacity_bytes`] plus what the entries
/// own outside the slabs — spilled vector buffers, map and frame arrays, string text and
/// shared blobs, ropes. A shared blob counts in full in every heap that holds it, since
/// each holder keeps it alive. **O(entries)**, so it is for a rare question — which
/// process to stop under memory pressure ([`crate::process::pressure`]) — never a hot path.
pub(super) fn owned_bytes(s: &Slabs) -> usize {
    use std::mem::size_of;
    let vectors: usize = s.vectors.iter().map(VecStore::buffer_bytes).sum();
    let maps: usize = s
        .maps
        .iter()
        .map(|node| {
            let data = if node.data.spilled() {
                node.data.capacity() * size_of::<(Value, Value)>()
            } else {
                0
            };
            let children = if node.children.spilled() {
                node.children.capacity() * size_of::<MapId>()
            } else {
                0
            };
            data + children
        })
        .sum();
    let strings: usize = s.strings.iter().map(LocalString::buffer_bytes).sum();
    let bytes: usize = s.bytes.iter().map(|blob| blob.len()).sum();
    let ropes: usize = s.ropes.iter().map(|rope| rope.len_bytes()).sum();
    let envs: usize = s
        .envs
        .iter()
        .map(|frame| {
            if frame.vars.spilled() {
                frame.vars.capacity() * size_of::<(Symbol, Value)>()
            } else {
                0
            }
        })
        .sum();
    slab_capacity_bytes(s) + vectors + maps + strings + bytes + ropes + envs
}

/// A **cheap** stand-in for retained capacity, in slab elements, for the park-time gate.
///
/// [`slab_capacity_bytes`] sums eleven `capacity()` fields per generation and multiplies each
/// by a size — ~25 ns, which is nothing once, and everything on a path that runs on every
/// park. `ring` parks a million times: the full sum cost it **+4.6%** while the trims it
/// gated cost nothing measurable. Three element counts from the nursery track growth just as
/// well for a heuristic (pairs, vectors and env frames are what a working set is made of),
/// and the trim itself still measures real bytes.
#[inline]
pub(super) fn park_trim_probe(s: &Slabs) -> usize {
    s.pairs.capacity() + s.vectors.capacity() + s.envs.capacity()
}

/// Hand every slab's unused capacity back to the allocator.
pub(super) fn shrink_slabs(s: &mut Slabs) {
    s.pairs.shrink_to_fit();
    s.vectors.shrink_to_fit();
    s.maps.shrink_to_fit();
    s.strings.shrink_to_fit();
    s.bigints.shrink_to_fit();
    s.decimals.shrink_to_fit();
    s.ratios.shrink_to_fit();
    s.bytes.shrink_to_fit();
    s.ropes.shrink_to_fit();
    s.closures.shrink_to_fit();
    s.natives.shrink_to_fit();
    s.envs.shrink_to_fit();
}

/// Byte-weighted footprint of a [`Slabs`] (`Σ slab.len() * size_of::<elem>`) —
/// the slab arrays themselves, not nested/shared content (inner spilled vectors,
/// string bytes, `Arc`-shared ropes/blobs). A comparative figure, not exact RSS.
/// Counts `natives` too (unlike [`slab_live_count`]). Backs [`Heap::local_bytes`].
///
/// **O(1)** — no per-element walk, because [`Heap::local_bytes`] is republished on
/// every `receive` park. A process with large spilled vectors therefore under-reports
/// `:memory` by their buffer bytes, acceptable for a comparative figure (the hard
/// memory cap uses the global allocator counter, not this).
pub(super) fn slab_bytes(s: &Slabs) -> usize {
    use std::mem::size_of;
    s.pairs.len() * size_of::<(Value, Value)>()
        + s.vectors.len() * size_of::<VecStore>()
        + s.maps.len() * size_of::<MapNode>()
        + s.strings.len() * size_of::<LocalString>()
        + s.bigints.len() * size_of::<num_bigint::BigInt>()
        + s.decimals.len() * size_of::<bigdecimal::BigDecimal>()
        + s.ratios.len() * size_of::<num_rational::BigRational>()
        + s.bytes.len() * size_of::<Arc<SharedBlob>>()
        + s.ropes.len() * size_of::<ropey::Rope>()
        + s.closures.len() * size_of::<Closure>()
        + s.natives.len() * size_of::<NativeFn>()
        + s.envs.len() * size_of::<EnvFrame>()
}

/// Append-only code slabs for the shared RUNTIME region. `boxcar::Vec` gives
/// lock-free reads that return stable references (existing elements never move
/// or free as the vector grows), so process threads read closure bodies without
/// locking while another process `def`s new code.
#[derive(Default)]
pub(super) struct CodeSlabs {
    pub(super) pairs: boxcar::Vec<(Value, Value)>,
    pub(super) vectors: boxcar::Vec<VecStore>,
    pub(super) maps: boxcar::Vec<MapNode>,
    pub(super) strings: boxcar::Vec<LocalString>,
    /// Bignums `def`'d into a global / baked as a literal into shared RUNTIME
    /// code (mirrors `strings`). Immutable, holds no handles; append-only.
    pub(super) bigints: boxcar::Vec<num_bigint::BigInt>,
    /// Decimals `def`'d into a global / baked as a literal into shared RUNTIME
    /// code (mirrors `bigints`). Immutable, holds no handles; append-only.
    pub(super) decimals: boxcar::Vec<bigdecimal::BigDecimal>,
    /// Rationals `def`'d into a global / baked as a literal into shared RUNTIME
    /// code (mirrors `decimals`). Immutable, holds no handles; append-only.
    pub(super) ratios: boxcar::Vec<num_rational::BigRational>,
    /// Raw bytes `def`'d into a global / captured by a promoted closure (mirrors
    /// `bigints`). Byte-clean `Arc<SharedBlob>`, never read as UTF-8.
    /// Append-only; the Arc is shared, not copied.
    pub(super) bytes: boxcar::Vec<Arc<SharedBlob>>,
    /// Ropes `def`'d into a global (shared read-only across this runtime's
    /// processes). A `ropey::Rope` is `Send + Sync` and immutable-by-construction
    /// here (every edit makes a fresh LOCAL rope), so sharing one by handle is
    /// sound. Append-only like the rest of this region.
    pub(super) ropes: boxcar::Vec<ropey::Rope>,
    /// `OnceLock`-wrapped so `promote` can **reserve a slot, then fill it** — the
    /// append-only `boxcar` can't write-back the way the GC's mutable slabs do, so
    /// a *cyclic* promote (a closure whose captured scope binds the closure itself,
    /// e.g. `(let (g (fn () g)) g)` or mutually-recursive `letrec` closures) would
    /// otherwise recurse forever → SIGSEGV. Reserve-then-fill lets the recursion
    /// resolve the back-edge to the reserved handle. Each cell is set exactly once
    /// before the handle is ever published, so reads (`get().unwrap()`) never race.
    pub(super) closures: boxcar::Vec<OnceLock<Closure>>,
    /// Captured environments of promoted closures. A closure defined *inside a
    /// function call* (not at top level) closes over a local scope; promoting it
    /// for sharing copies that scope here so it resolves in any process. Frozen
    /// once promoted (read-only), so append-only is sound. `OnceLock`-wrapped for
    /// the same reserve-then-fill cycle break as `closures` above.
    pub(super) envs: boxcar::Vec<OnceLock<EnvFrame>>,
    /// Where each `boxcar` bucket of [`Self::vectors`] keeps its `VecStore`s, so native
    /// code can index a RUNTIME vector without the `brood_rt_vector_ref` callback (the
    /// `matmul` lever, `docs/compute-frontier.md` 2026-10-25). Entry `b` is the address the
    /// bucket's first `VecStore` would have, or 0 until the callback has read a vector in
    /// that bucket and noted it ([`Self::note_vector`]). Written once per bucket and never
    /// changed: a bucket never moves, and a generation that is replaced is a fresh
    /// `CodeSlabs` with its own table (every replacement bumps `gen_version`).
    #[cfg(feature = "jit")]
    pub(super) vec_buckets: [std::sync::atomic::AtomicUsize; RT_VEC_BUCKETS],
}

/// Buckets a RUNTIME vector index can fall in: handle indices are below `2^32`, and
/// `⌊log2(2^32 + 32)⌋ − 5` is 27.
#[cfg(feature = "jit")]
pub(crate) const RT_VEC_BUCKETS: usize = 28;

/// `boxcar` 0.2.14's location of index `i` (`buckets.rs`, `SKIPPED_BUCKETS = 5`): bucket
/// `⌊log2(i + 32)⌋ − 5`, at offset `i + 32 − 2^⌊log2(i + 32)⌋`. The JIT computes the same
/// thing inline (`jit_lower/emit.rs`, `shared_vec_slot`). [`rt_vec_entry_stride`] checks
/// it against a real `boxcar::Vec` before native code may rely on it.
#[cfg(feature = "jit")]
#[inline]
pub(crate) fn rt_vec_location(i: usize) -> (usize, usize) {
    let inner = i + 32;
    let log = (usize::BITS - 1 - inner.leading_zeros()) as usize;
    (log - 5, inner - (1 << log))
}

/// The distance between two consecutive `VecStore`s in a `boxcar` bucket, or `None` when
/// [`rt_vec_location`] does not describe this build's `boxcar`, in which case native code
/// keeps the callback for every RUNTIME vector. Asked once per process: the answer is a
/// fact about the binary.
#[cfg(feature = "jit")]
pub(crate) fn rt_vec_entry_stride() -> Option<usize> {
    static STRIDE: OnceLock<Option<usize>> = OnceLock::new();
    *STRIDE.get_or_init(|| {
        // Enough entries to span the first four buckets (32 + 64 + 128 + some of 256).
        const N: usize = 300;
        let probe: boxcar::Vec<VecStore> = boxcar::Vec::new();
        for _ in 0..N {
            probe.push(VecStore::from_vec(Vec::new()));
        }
        let addr = |i: usize| &probe[i] as *const VecStore as usize;
        let stride = addr(1).checked_sub(addr(0))?;
        if stride < std::mem::size_of::<VecStore>() {
            return None;
        }
        // The first index of each bucket, by the same formula, is its base.
        let mut first = [usize::MAX; RT_VEC_BUCKETS];
        for i in 0..N {
            let (b, off) = rt_vec_location(i);
            if off == 0 {
                first[b] = i;
            }
        }
        for i in 0..N {
            let (b, off) = rt_vec_location(i);
            if first[b] == usize::MAX || addr(i) != addr(first[b]) + off * stride {
                return None;
            }
        }
        Some(stride)
    })
}

impl CodeSlabs {
    /// Record where vector `i`'s bucket keeps its `VecStore`s, if that is not known yet —
    /// called by the callback after it has read vector `i`, so the next read of any vector
    /// in the same bucket stays in native code.
    #[cfg(feature = "jit")]
    pub(super) fn note_vector(&self, i: usize) {
        use std::sync::atomic::Ordering;
        let Some(stride) = rt_vec_entry_stride() else {
            return;
        };
        let (b, off) = rt_vec_location(i);
        let Some(cell) = self.vec_buckets.get(b) else {
            return;
        };
        if cell.load(Ordering::Relaxed) != 0 {
            return;
        }
        if let Some(store) = self.vectors.get(i) {
            let base = store as *const VecStore as usize - off * stride;
            cell.store(base, Ordering::Release);
        }
    }

    /// True if this generation holds no code — every slab empty. Aging may only
    /// start a new generation in a slot that is empty (its previous generation
    /// fully reclaimed), so a fresh gen's handle indices can't collide with a
    /// stale generation's still-live handles (the 2-versions-max rule, ADR-091).
    pub(super) fn is_empty(&self) -> bool {
        self.pairs.count() == 0
            && self.vectors.count() == 0
            && self.maps.count() == 0
            && self.strings.count() == 0
            && self.bigints.count() == 0
            && self.decimals.count() == 0
            && self.ratios.count() == 0
            && self.bytes.count() == 0
            && self.ropes.count() == 0
            && self.closures.count() == 0
            && self.envs.count() == 0
    }
}

#[cfg(all(test, feature = "jit"))]
mod vecstore_layout_tests {
    use super::*;

    /// Native code finds a RUNTIME vector's `VecStore` by `rt_vec_location` and a bucket
    /// base (`shared_vec_slot`), so the formula must place every index exactly where
    /// `boxcar` does — far past the 300-entry startup probe, across a dozen buckets. A
    /// `boxcar` upgrade that changes its layout fails here and in `rt_vec_entry_stride`,
    /// which then keeps every RUNTIME read on the callback.
    ///
    /// The vector holds `VecStore`s and steps by the stride the startup check measured, as
    /// native code does: a `boxcar` entry carries its own `active` flag beside the value, so
    /// the step is not `size_of` the element — a `u64` vector stepping by 8 failed at index 1,
    /// where the entry is 16 bytes on.
    #[test]
    fn the_shared_vector_location_is_where_boxcar_puts_it() {
        let stride =
            rt_vec_entry_stride().expect("the startup self-check must accept this build's boxcar");
        const N: usize = 100_000;
        let v: boxcar::Vec<VecStore> = boxcar::Vec::new();
        for _ in 0..N {
            v.push(VecStore::from_vec(Vec::new()));
        }
        let addr = |i: usize| &v[i] as *const VecStore as usize;
        let mut first = [usize::MAX; RT_VEC_BUCKETS];
        for i in 0..N {
            let (b, off) = rt_vec_location(i);
            if off == 0 {
                first[b] = i;
            }
        }
        for i in 0..N {
            let (b, off) = rt_vec_location(i);
            assert_eq!(
                addr(i),
                addr(first[b]) + off * stride,
                "index {i} (bucket {b})"
            );
        }
        // The largest handle index lands in the last bucket the table has room for.
        assert_eq!(rt_vec_location(u32::MAX as usize).0, RT_VEC_BUCKETS - 1);
    }
    use crate::core::value::Value;

    /// Pin the byte layout the JIT hardcodes for its inline small-vector read
    /// (`jit_lower.rs`): the `Inline` discriminant is 0, and `len`/`items` sit at
    /// the advertised offsets within a slot. A `#[repr(u8)]` layout drift (e.g.
    /// bumping `INLINE_VEC_CAP` or reordering fields) fails here rather than
    /// silently miscompiling every `nth`.
    #[test]
    fn vecstore_jit_layout() {
        let v = VecStore::Inline {
            len: 2,
            items: [Value::int(7), Value::int(9)],
        };
        let base = &v as *const VecStore as usize;
        // Discriminant byte at offset 0 (repr(u8), RFC 2195).
        // SAFETY: `v` is a live local; under `repr(u8)` its first byte is the initialised
        // discriminant, and only that byte is read.
        let tag = unsafe { *(base as *const u8) };
        assert_eq!(tag as i64, VecStore::JIT_INLINE_TAG, "Inline discriminant");
        if let VecStore::Inline { len, items } = &v {
            assert_eq!(
                len as *const u8 as usize - base,
                VecStore::JIT_LEN_OFF as usize,
                "Inline.len offset"
            );
            assert_eq!(
                items.as_ptr() as usize - base,
                VecStore::JIT_ITEMS_OFF as usize,
                "Inline.items offset"
            );
        }
        assert_eq!(
            std::mem::size_of::<VecStore>() as i64,
            VecStore::JIT_STRIDE,
            "slab stride"
        );
        // The JIT reads a Value element as 3 i64 words; the slab stride between
        // elements is `size_of::<Value>()`.
        assert_eq!(std::mem::size_of::<Value>(), 24, "Value stride");
        // Spill layout: discriminant 1 @0, cached ptr @8, cached len @16 — the
        // JIT's pointer-read path loads exactly these.
        let sp = VecStore::spill(vec![Value::int(1), Value::int(2), Value::int(3)]);
        let sbase = &sp as *const VecStore as usize;
        // SAFETY: as for `tag` — `sp` is a live local whose first byte is the discriminant.
        let stag = unsafe { *(sbase as *const u8) };
        assert_eq!(stag as i64, VecStore::JIT_SPILL_TAG, "Spill discriminant");
        if let VecStore::Spill { ptr, len, vec } = &sp {
            assert_eq!(
                ptr as *const *const Value as usize - sbase,
                VecStore::JIT_SPILL_PTR_OFF as usize,
                "Spill.ptr offset"
            );
            assert_eq!(
                len as *const u64 as usize - sbase,
                VecStore::JIT_SPILL_LEN_OFF as usize,
                "Spill.len offset"
            );
            assert_eq!(*ptr, vec.as_ptr(), "cached ptr matches the buffer");
            assert_eq!(*len as usize, vec.len(), "cached len matches");
        }
        // Shared layout (ADR-511): discriminant 2, and `ptr`/`len` exactly where Spill's are,
        // so the JIT's one pointer-read path serves both.
        let buf = AppendBuf::new(vec![Value::int(1), Value::int(2), Value::int(3)]);
        let sh = VecStore::shared(buf, 3);
        let hbase = &sh as *const VecStore as usize;
        // SAFETY: as for `tag` — `sh` is a live local whose first byte is the discriminant.
        let htag = unsafe { *(hbase as *const u8) };
        assert_eq!(htag as i64, VecStore::JIT_SHARED_TAG, "Shared discriminant");
        if let VecStore::Shared { ptr, len, buf } = &sh {
            assert_eq!(
                ptr as *const *const Value as usize - hbase,
                VecStore::JIT_SPILL_PTR_OFF as usize,
                "Shared.ptr offset"
            );
            assert_eq!(
                len as *const u64 as usize - hbase,
                VecStore::JIT_SPILL_LEN_OFF as usize,
                "Shared.len offset"
            );
            assert_eq!(*ptr, buf.as_ptr(), "cached ptr matches the buffer");
            assert_eq!(*len, 3, "cached len");
        }
    }
}

#[cfg(test)]
mod lazy_vec_tests {
    use super::LazyVec;

    /// One pointer until used, a `Vec` while used, and one pointer again once emptied and
    /// shrunk — the property the per-process floor counts on.
    #[test]
    fn a_lazy_slab_costs_a_pointer_until_used_and_after_it_empties() {
        assert_eq!(std::mem::size_of::<LazyVec<u64>>(), 8);
        let mut v: LazyVec<u64> = LazyVec::default();
        assert!(v.is_empty() && v.0.is_none() && v.capacity() == 0);
        assert!(
            LazyVec::<u64>::with_capacity(0).0.is_none(),
            "no box for a zero reserve"
        );
        v.push(7);
        v.push(8);
        assert_eq!((v.len(), v[1]), (2, 8));
        v[0] = 9;
        assert_eq!(&v[..], &[9, 8]);
        v.truncate(0);
        v.shrink_to_fit();
        assert!(v.0.is_none(), "an emptied slab gives its box back");
    }
}

#[cfg(test)]
mod park_trim_tests {
    use super::*;

    #[test]
    fn a_small_heap_trims_after_the_fixed_minimum() {
        assert!(!park_trim_due(63, 0));
        assert!(park_trim_due(64, 0));
        assert!(park_trim_due(100 + 64, 100));
    }

    #[test]
    fn a_large_growing_heap_trims_only_after_growing_by_half() {
        // the editor's case: 1M slots of capacity, growing a little per turn
        let mark = 1_000_000;
        assert!(!park_trim_due(mark + 64, mark));
        assert!(!park_trim_due(mark + 499_999, mark));
        assert!(park_trim_due(mark + 500_000, mark));
    }

    #[test]
    fn a_steadily_growing_heap_trims_logarithmically_often() {
        // grow by 1000 slots per park from 10K to 10M: count the trims
        let (mut probe, mut mark, mut trims) = (10_000usize, 10_000usize, 0);
        while probe < 10_000_000 {
            probe += 1000;
            if park_trim_due(probe, mark) {
                trims += 1;
                mark = probe;
            }
        }
        assert!(trims < 20, "{trims} trims growing 10K → 10M");
    }
}
