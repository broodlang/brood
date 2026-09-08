//! The slab substrate — child of heap.
//!
//! Where LOCAL and shared values physically live: [`VecStore`] (a vector's elements, inline in
//! the slab slot up to [`INLINE_VEC_CAP`] and spilled to a `Vec` past it), [`Slabs`] (one `Vec`
//! per kind — the LOCAL nursery/old generations and the frozen PRELUDE are all this shape) with
//! the live-count / capacity / park-trim helpers the collector's sizing reads, [`CodeSlabs`]
//! (the append-only `boxcar` slabs of one RUNTIME code generation), and [`SlabRef`], the borrow
//! shim every accessor returns — a direct reference into a LOCAL/PRELUDE slab, or a pinned one
//! into a RUNTIME generation. Split out of `heap.rs` on 2026-09-08 (handoff item 1, move e).
//! Fields and constructors are `pub(super)` because `heap.rs`, `gc.rs` and `gc_runtime.rs`
//! address the slabs directly; `heap.rs` re-exports `VecStore`, `INLINE_VEC_CAP` and `SlabRef`
//! at their previous visibility so paths outside the module are unchanged.

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
}

// SAFETY: `Spill::ptr` always points into `Spill::vec`'s own buffer (established
// by the one constructor and re-derived on clone), so it is exactly as sendable/
// sharable as the `Vec` it caches.
unsafe impl Send for VecStore {}
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
        }
    }
}

impl VecStore {
    /// The one `Spill` constructor: caches the buffer pointer + length.
    #[inline]
    pub(super) fn spill(vec: Vec<Value>) -> Self {
        VecStore::Spill {
            ptr: vec.as_ptr(),
            len: vec.len() as u64,
            vec,
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
            VecStore::spill((0..len).map(producer).collect())
        }
    }

    #[inline]
    pub(super) fn as_slice(&self) -> &[Value] {
        match self {
            VecStore::Inline { len, items } => &items[..*len as usize],
            VecStore::Spill { vec, .. } => vec,
        }
    }

    #[inline]
    pub(super) fn as_mut_slice(&mut self) -> &mut [Value] {
        match self {
            VecStore::Inline { len, items } => &mut items[..*len as usize],
            VecStore::Spill { vec, .. } => vec,
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
    /// `Spill.ptr` byte offset within a slot (u8 tag, padded to the pointer's align).
    #[cfg(feature = "jit")]
    pub(crate) const JIT_SPILL_PTR_OFF: i32 = 8;
    /// `Spill.len` byte offset within a slot.
    #[cfg(feature = "jit")]
    pub(crate) const JIT_SPILL_LEN_OFF: i32 = 16;
}

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
    pub(super) bigints: Vec<num_bigint::BigInt>,
    /// Arbitrary-precision base-10 decimals (mirrors `bigints` exactly). One
    /// `bigdecimal::BigDecimal` per live `Value::Decimal`; immutable, holds no
    /// `Value` children. Unlike `bigints` there is no normalize-into-`Int`
    /// invariant — a decimal is its own type and any value is stored as-is.
    pub(super) decimals: Vec<bigdecimal::BigDecimal>,
    /// Exact rationals (mirrors `decimals`). One `num_rational::BigRational` per live
    /// `Value::Ratio`; immutable, holds no `Value` children. Always reduced with a
    /// positive denominator; a denominator of 1 is demoted to `Int` at construction
    /// (`Heap::alloc_ratio`), so no entry here is ever integer-valued.
    pub(super) ratios: Vec<num_rational::BigRational>,
    /// **Raw bytes** — byte-clean immutable leaves, one `Arc<SharedBlob>` per live
    /// value (arbitrary bytes, never UTF-8, own slab + handle). The `Arc` is the unit
    /// of cross-process sharing (a refcount bump, not a byte copy).
    pub(super) bytes: Vec<Arc<SharedBlob>>,
    /// Text ropes (ADR-045). A `ropey::Rope` is itself `Arc`-shared internally,
    /// so this slab owns one cheap handle per live rope; cloning for an edit
    /// bumps refcounts, not bytes. Always inline (no SharedBlob split — ropes
    /// don't cross processes, so there's no cross-heap aliasing to optimise).
    pub(super) ropes: Vec<ropey::Rope>,
    pub(super) closures: Vec<Closure>,
    pub(super) natives: Vec<NativeFn>,
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
    /// A fresh, empty `Slabs` whose per-slab `Vec`s carry the **capacity of
    /// `like`'s lengths** — the flip-side nursery allocator. A minor collection
    /// used to install `Slabs::default()` (zero capacity), so every cycle
    /// re-paid the full Vec-doubling ladder up to the nursery threshold — each
    /// doubling memmoves everything allocated so far, ~12 % of an
    /// allocation-bound run (bintree) went to those copies. The previous
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
            bigints: Vec::with_capacity(like.bigints.len()),
            decimals: Vec::with_capacity(like.decimals.len()),
            ratios: Vec::with_capacity(like.ratios.len()),
            bytes: Vec::with_capacity(like.bytes.len()),
            ropes: Vec::with_capacity(like.ropes.len()),
            closures: Vec::with_capacity(like.closures.len()),
            natives: Vec::new(), // never GC'd, never grows here
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

/// Byte-weighted footprint of a [`Slabs`] (`Σ slab.len() * size_of::<elem>`) —
/// the slab arrays themselves, not nested/shared content (inner spilled vectors,
/// string bytes, `Arc`-shared ropes/blobs). A comparative figure, not exact RSS.
/// Counts `natives` too (unlike [`slab_live_count`]). Backs [`Heap::local_bytes`].
///
/// **O(1)** — every term is `slab.len() * size_of::<elem>`, no per-element walk.
/// This matters: [`Heap::local_bytes`] is republished on every `receive` park, and
/// an earlier `Σ vectors.spilled_bytes()` term (walking every VecStore) made it
/// O(heap) — ~50% of a tight message-passing loop. Excluding spilled buffers also
/// squares the code with this doc's "not nested/shared content" contract; a
/// process with large spilled vectors under-reports `:memory` by their buffer
/// bytes, acceptable for a comparative observability figure (the hard memory cap
/// uses the global allocator counter, not this).
/// How much retained capacity must have accumulated **since the last trim** before
/// [`Heap::trim_parked`] does anything.
///
/// Note the "since the last trim": an absolute size threshold is the obvious design and it
/// is wrong in both directions, which the 2026-07-28 measurements showed plainly.
///
/// * A **high** absolute gate (32 KiB) is latency-safe but makes memory non-monotonic in
///   allocation: a process that consed 1,000 pairs crossed it and got trimmed to 8.5 KB,
///   while one that consed *100* stayed under and kept **14.5 KB** — allocating more used
///   less, which is indefensible to explain to a user.
/// * A **low** absolute gate (4 KiB) fixes that (5.4 → 8.5 → 8.5 KB, monotonic) and costs
///   `pingpong` **+193%** (213 → 624 ms), because that row parks 200k times and each park
///   now pays a collection.
///
/// Growth-since-last-trim gets both. A responder that parks constantly reaches a steady
/// working set, so after one trim its capacity stops growing and it never trims again — it
/// pays one subtraction per park. A process that actually accumulated something trims once
/// per accumulation, regardless of how large its heap is in absolute terms.
///
/// Measured in [`park_trim_probe`] slots rather than bytes so the gate stays a few loads:
/// 64 slots is roughly a few KiB of pairs, i.e. the same intent as the 4 KiB it replaces.
pub(super) const PARK_TRIM_GROWTH_SLOTS: usize = 64;

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
}

/// A borrow into a slab, valid for as long as the wrapper is held. It is either a
/// **direct** `&self`-borrow (LOCAL / PRELUDE, or a compaction-time RUNTIME read) or
/// a **pinned** borrow into an ArcSwap-managed RUNTIME generation, where the held
/// `Arc<CodeSlabs>` keeps that generation's slab alive so a concurrent Stage-4 free
/// ([`Heap::free_runtime_gen`], ADR-091) can swap the slab out without invalidating
/// an in-flight read. `Deref`s to `T`, so call sites use it exactly like `&T`.
///
/// The RUNTIME pin is a plain `Arc` clone obtained from a per-process **version-gated
/// cache** ([`Heap::code_gen_pinned`]) rather than a fresh `ArcSwap::load` guard per
/// deref: the latter's hybrid-strategy load dominated global-data-heavy hot loops (a
/// read of a `def`'d matrix element in `matmul` derefs a RUNTIME handle, ~16 M times).
pub struct SlabRef<'a, T: ?Sized> {
    /// Keeps the RUNTIME generation's `Arc<CodeSlabs>` alive while borrowed; `None`
    /// for a direct borrow. Never read directly — held purely so its `Drop` (the
    /// Arc release) runs no earlier than the pointer's last use.
    _pin: Option<Arc<CodeSlabs>>,
    /// Points into the borrowed slot — a direct `&'a T`, or into the slab the pin
    /// keeps alive. Valid for the wrapper's whole lifetime either way.
    ptr: *const T,
    _life: std::marker::PhantomData<&'a T>,
}

// SAFETY: `SlabRef` is a plain shared borrow (a `&T` plus, optionally, the `Arc`
// that keeps `T` alive). It is `Send`/`Sync` exactly when `&T` is — the pin is an
// `Arc` clone (already `Send`+`Sync` for our `CodeSlabs`), and the raw pointer only
// ever yields shared `&T` access.
unsafe impl<T: ?Sized + Sync> Sync for SlabRef<'_, T> {}
unsafe impl<T: ?Sized + Sync> Send for SlabRef<'_, T> {}

impl<'a, T: ?Sized> SlabRef<'a, T> {
    /// A direct `&self`-borrow (LOCAL / PRELUDE, or a compaction-time RUNTIME read).
    #[inline]
    pub(super) fn direct(r: &'a T) -> Self {
        SlabRef {
            _pin: None,
            ptr: r as *const T,
            _life: std::marker::PhantomData,
        }
    }
    /// A pinned borrow into a RUNTIME generation the `pin` `Arc` keeps alive.
    ///
    /// SAFETY: `ptr` must point into the `CodeSlabs` held alive by `pin` (obtained
    /// from `&*pin`), so it stays valid for the wrapper's whole lifetime.
    #[inline]
    pub(super) unsafe fn pinned(pin: Arc<CodeSlabs>, ptr: *const T) -> Self {
        SlabRef {
            _pin: Some(pin),
            ptr,
            _life: std::marker::PhantomData,
        }
    }
    /// Re-project the borrow to a part of `T` (e.g. a field), carrying the same pin
    /// so the underlying slab stays alive. Like `Ref::map`.
    #[inline]
    pub(crate) fn map<U: ?Sized>(self, f: impl FnOnce(&T) -> &U) -> SlabRef<'a, U> {
        // SAFETY: `self.ptr` is valid (invariant of `SlabRef`); the projected `&U`
        // points within the same slab the pin (moved below) keeps alive.
        let ptr = f(unsafe { &*self.ptr }) as *const U;
        SlabRef {
            _pin: self._pin,
            ptr,
            _life: std::marker::PhantomData,
        }
    }
}

impl<T: ?Sized> std::ops::Deref for SlabRef<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        // SAFETY: `ptr` is either a direct `&'a T` or points into the slab the held
        // `_guard` keeps alive; both outlive `&self`.
        unsafe { &*self.ptr }
    }
}

// `&T`-like ergonomics so a `SlabRef` drops into most call sites unchanged.
impl<T: ?Sized> AsRef<T> for SlabRef<'_, T> {
    #[inline]
    fn as_ref(&self) -> &T {
        self
    }
}
impl<T: ?Sized + std::fmt::Debug> std::fmt::Debug for SlabRef<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        (**self).fmt(f)
    }
}
impl<T: ?Sized + std::fmt::Display> std::fmt::Display for SlabRef<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        (**self).fmt(f)
    }
}
impl<T: ?Sized + PartialEq> PartialEq for SlabRef<'_, T> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}
impl<T: ?Sized + PartialEq> PartialEq<T> for SlabRef<'_, T> {
    #[inline]
    fn eq(&self, other: &T) -> bool {
        **self == *other
    }
}
// Comparing a `SlabRef<str>` against a string literal / `&str` (`sr == "foo"`).
impl PartialEq<&str> for SlabRef<'_, str> {
    #[inline]
    fn eq(&self, other: &&str) -> bool {
        &**self == *other
    }
}

impl CodeSlabs {
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
