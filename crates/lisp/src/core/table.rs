//! In-memory shared mutable table (ADR-107).
//!
//! A [`Value::Table`](crate::core::value::Value::Table) is a scalar `u64` handle
//! into a global registry of shared stores. Unlike a `Rope` (a per-process heap
//! object) it is **sendable across processes**: the handle copies by value and every
//! copy indexes the *same* store — the way a `Pid` names one shared process. This is
//! genuine mutable state, expressed the blessed way (CLAUDE.md): a Rust-backed
//! opaque resource behind primitives, never a mutable `Value`.
//!
//! ## Why this can't corrupt
//!
//! The store holds **deep clones in heap-independent [`Message`] form** (hashed
//! representation) or immediate scalars packed into atomic words (dense
//! representation) — nothing in a store is ever a live GC handle, so the moving
//! collector never traces or moves into it. `get` reconstructs a **fresh** value in
//! the *caller's* heap, so two processes never alias a stored value
//! (copy-in/copy-out). Key equality is **borrowed from the heap** (`hash_value` to
//! bucket, `equal` on a reconstructed key to resolve collisions), so table keys
//! behave identically to immutable-map keys — no parallel equality code.
//!
//! ## Locking discipline
//!
//! The registry is **lock-free** (an append-only `boxcar::Vec` — see `REGISTRY`): a
//! handle resolves to its store with a single indexed read and no lock or `Arc`
//! clone. The **dense** representation is lock-free per op too — one atomic
//! load/swap/CAS on the key's slot plus one flag load, the shape of
//! `:atomics`/`bytearray` that every hot Table workload (sieve marks, counters,
//! memo sets) actually is; the store `Mutex` is taken only for the hashed
//! representation and the one-time dense→hashed migration (see "Migration
//! protocol" on [`Store`]).
//!
//! ## Lifetime
//!
//! A table belongs to the process that created it and is dropped when that process
//! exits (an heir or ownership hand-off is deferred). The exception is
//! a table that becomes reachable from a **global**: `def` (and every other write to the
//! shared table of globals) promotes its value, and a table that promotion meets is
//! *pinned* — it now lives as long as the runtime, because any process may read that
//! global. So `(def- cache (table/new))` in a module body is app-lifetime whichever
//! process happened to load the module, and a table a short-lived worker built and sent
//! away dies with the worker. `table-drop` still ends a table early.
//! Operating on a dropped/unknown handle is a clean error, never UB.
//!
//! A table created on a thread with no process context (an embedder's `Interp`, the boot
//! image restoring a module's tables) has no owner and lives until `table-drop`; so does
//! a table whose owner died on the scheduler's panic-recovery path, which has no heap
//! left to list what the process owned.
//!
//! **What a drop frees.** The hashed map always; the dense slots' committed pages always
//! (`madvise(MADV_DONTNEED)` — the mapping stays, so a racing op on an escaped handle
//! still touches valid memory, and reads back `EMPTY`). The **registry slot** — the store
//! shell and its reserved chunks — is recycled for the next `table/new` only when no other
//! process can hold the handle: the table never left its creator (`escaped`: sent, or
//! captured by a `spawn`ed closure or a global), and no JIT'd loop of the dropping process
//! can still hold its slot base (`jit_shared`), or the drop is the owner's death, which
//! ends every such loop. A recycled slot's generation is bumped and is part of the
//! handle, so a stale handle in the dropping process errors instead of reaching the next
//! table. Without recycling, every short-lived table kept its shell and a 512 KB chunk
//! reservation for good: a function the linear-map rewrite builds on a table exhausted a
//! 16 GB address-space cap after ~25 000 calls.

use crate::core::heap::Heap;
use crate::core::sync::lock;
use crate::core::value::Value;
use crate::error::{LispError, LispResult};
use crate::process::{Message, from_message, to_message};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex, MutexGuard};

/// One structural-hash bucket: the (key-clone, value-clone) pairs sharing a hash. Almost
/// always length 1 — a genuine hash collision is rare — so the single entry is stored
/// **inline** (`SmallVec<[_; 1]>`), saving a per-entry heap allocation. For a table with a
/// million distinct scalar keys (a `sieve`) that is a million `Vec` allocations avoided:
/// less RSS, less allocator churn, one fewer pointer-chase per `get`/`put`.
type Bucket = smallvec::SmallVec<[(Message, Message); 1]>;

/// The store map is keyed by `heap.hash_value(key)` — an already-well-distributed
/// 64-bit **structural** hash. Re-hashing that u64 with the default SipHash on every
/// `get`/`put`/`has?` is pure waste, so key the map with an **identity** hasher: the
/// u64 passes straight through. Low-bit collisions are resolved exactly by the bucket's
/// `from_message`+`equal` walk (as before), so correctness is unchanged — this only
/// removes a SipHash round from the hot path of every table op.
#[derive(Default, Clone, Copy)]
struct IdentityHasher(u64);
impl std::hash::Hasher for IdentityHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        // The key is always a single `write_u64`; this fallback keeps the impl total.
        for &b in bytes {
            self.0 = self.0.rotate_left(8) ^ b as u64;
        }
    }
    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.0 = i;
    }
}
type StoreMap = HashMap<u64, Bucket, std::hash::BuildHasherDefault<IdentityHasher>>;

// ---- Dense slot encoding -----------------------------------------------------
//
// A dense slot is one atomic u64, so every dense op is a single atomic
// load/swap/CAS — no lock, no 16-byte enum. Tag in the low 3 bits:
//
//   0             EMPTY  (key absent — distinct from a stored nil)
//   1             NIL
//   2             TRUE
//   3             FALSE
//   5             MOVED  (migration sentinel — see the protocol on `Store`)
//   ..​.100 (bit 2) an int: value = (word as i64) >> 3  (61-bit two's complement)
//
// Ints outside ±2^60 don't fit the tagged word and migrate the table to the
// hashed representation (they round-trip fine there); every hot dense workload
// (marks, flags, counters, ids) lives comfortably in 61 bits.

pub(crate) const SLOT_EMPTY: u64 = 0;
pub(crate) const SLOT_NIL: u64 = 1;
pub(crate) const SLOT_TRUE: u64 = 2;
pub(crate) const SLOT_FALSE: u64 = 3;
pub(crate) const SLOT_MOVED: u64 = 5;
pub(crate) const INT_TAG: u64 = 0b100;

/// Encode a value into a dense slot word, or `None` when it needs the hashed
/// representation (non-scalar, or an int outside the 61-bit tagged range).
#[inline]
fn slot_enc(v: Value) -> Option<u64> {
    match v {
        Value::Nil => Some(SLOT_NIL),
        Value::Bool(true) => Some(SLOT_TRUE),
        Value::Bool(false) => Some(SLOT_FALSE),
        Value::Int(n) if (-(1i64 << 60)..(1i64 << 60)).contains(&n) => {
            Some(((n as u64) << 3) | INT_TAG)
        }
        _ => None,
    }
}

/// Decode a dense slot word to a `Value` — scalars are immediate, so no heap is
/// needed. `None` for `EMPTY`. Must not be called on `MOVED` (callers route those
/// to the hashed path first).
#[inline]
fn slot_dec(s: u64) -> Option<Value> {
    match s {
        SLOT_EMPTY => None,
        SLOT_NIL => Some(Value::Nil),
        SLOT_TRUE => Some(Value::Bool(true)),
        SLOT_FALSE => Some(Value::Bool(false)),
        _ => {
            debug_assert!(s & INT_TAG != 0, "slot_dec on a MOVED/invalid word");
            Some(Value::int((s as i64) >> 3))
        }
    }
}

/// The `Message` form of a dense slot word (for migration / snapshot).
fn slot_to_message(s: u64) -> Option<Message> {
    match s {
        SLOT_EMPTY => None,
        SLOT_NIL => Some(Message::Nil),
        SLOT_TRUE => Some(Message::Bool(true)),
        SLOT_FALSE => Some(Message::Bool(false)),
        _ => Some(Message::Int((s as i64) >> 3)),
    }
}

/// Largest int key the dense representation will hold. Beyond it (or for a negative /
/// non-int key, or a value outside the tagged-scalar shapes) the store migrates to
/// the hashed map. The dense region is `DENSE_KEY_MAX / CHUNK_SLOTS` chunks of
/// `CHUNK_SLOTS` atomic words, each an anonymous mapping reserved the first time a key
/// in its range is WRITTEN (the OS then commits 4 KB pages as slots are first touched),
/// so both address space and RSS track the keys actually used: one far-out key costs
/// one chunk and one page, not a resize.
pub(crate) const DENSE_KEY_MAX: i64 = 1 << 23;

/// Slots per dense chunk: 2^16 × 8 B = 512 KB of address space per chunk a table
/// touches — never one large reservation per table, which made many small tables
/// exhaust a capped address space (KI-142). A chunk is small enough that a table of a few
/// keys costs one, and large enough that a sieve over millions of keys maps at most 128.
pub(crate) const CHUNK_SHIFT: u32 = 16;
pub(crate) const CHUNK_SLOTS: usize = 1 << CHUNK_SHIFT;
/// The chunk count — `DENSE_KEY_MAX` is a multiple of `CHUNK_SLOTS` by construction.
pub(crate) const CHUNKS: usize = (DENSE_KEY_MAX as usize) >> CHUNK_SHIFT;

/// How many dense-slot chunks the process has installed. A chunk is never unmapped and a
/// dropped or exited table's slot is reused with its chunks, so this is the table store's
/// address-space footprint in chunks, and a steady churn of tables leaves it flat. The
/// `table_address_space` tests read it: the process's `VmSize` also moves with allocator
/// arenas and worker stacks, which is how a run that reclaimed every table still read 192 MB.
static CHUNKS_MAPPED: AtomicUsize = AtomicUsize::new(0);

/// [`CHUNKS_MAPPED`]: dense-slot chunks installed so far, process-wide.
pub fn chunks_mapped() -> usize {
    CHUNKS_MAPPED.load(Ordering::Relaxed)
}

/// The dense slot region: `CHUNKS` chunk pointers, each null until a write reaches
/// its key range, then a `CHUNK_SLOTS`-word mapping that is all-zero (= `EMPTY`) at
/// birth. On unix a chunk is one anonymous `mmap` — a virtual reservation whose pages
/// the OS commits on first touch, so a sparse chunk costs pages, not the full span.
/// (It also deliberately bypasses the ADR-043 counting allocator: an untouched
/// reservation against the soft cap would be pure fiction; the hashed side's real
/// allocations are counted as always.) A chunk, once installed, never moves and is
/// never unmapped — see "Lifetime" above — which is what lets JIT'd code hold the
/// chunk directory's address (`jit_dense_base`) and read a chunk pointer per op.
struct DenseSlots {
    chunks: Box<[AtomicPtr<AtomicU64>; CHUNKS]>,
}

impl DenseSlots {
    fn new() -> Self {
        DenseSlots {
            chunks: Box::new([const { AtomicPtr::new(std::ptr::null_mut()) }; CHUNKS]),
        }
    }

    /// The chunk directory's address, for JIT'd code (see [`jit_dense_base`]). Stable
    /// for the process lifetime: the box is never reallocated.
    #[cfg(feature = "jit")]
    fn directory(&self) -> *const AtomicPtr<AtomicU64> {
        self.chunks.as_ptr()
    }

    /// The slot for dense index `i` if its chunk has been mapped — `None` reads as
    /// `EMPTY` (no key in that range was ever written). Callers only produce `i` via
    /// [`dense_idx`], which bounds it below `DENSE_KEY_MAX`.
    #[inline]
    fn slot(&self, i: usize) -> Option<&AtomicU64> {
        debug_assert!(i < DENSE_KEY_MAX as usize);
        let chunk = self.chunks[i >> CHUNK_SHIFT].load(Ordering::Acquire);
        if chunk.is_null() {
            return None;
        }
        // SAFETY: an installed chunk is a live `CHUNK_SLOTS`-word mapping and
        // `i & (CHUNK_SLOTS - 1)` is inside it.
        Some(unsafe { &*chunk.add(i & (CHUNK_SLOTS - 1)) })
    }

    /// The slot for dense index `i`, mapping its chunk on first use. Fallible — the
    /// failure is a Brood error, not a panic: a chunk reservation is the first sizeable
    /// mapping a *small* program makes, so it is the one that hits an address-space cap
    /// that something else filled (on a 28-core box the allocator's per-thread arenas
    /// (20 × 128 MB, `PROT_NONE`) and the worker stacks (28 × 16 MB) reserve ~3 GB
    /// before any table exists). As an `assert!` this aborted the process from inside a
    /// JIT callback that cannot unwind, with a message blaming the table. As an error it
    /// names the cause, is catchable, and the test that hit it fails alone.
    #[inline]
    fn slot_or_map(&self, i: usize) -> Result<&AtomicU64, LispError> {
        if let Some(slot) = self.slot(i) {
            return Ok(slot);
        }
        self.map_chunk(i >> CHUNK_SHIFT)?;
        Ok(self.slot(i).expect("mapped above"))
    }

    /// Install chunk `c`, unless a racing writer installed it first — the loser
    /// releases its own mapping, so two first-users cannot leak one.
    fn map_chunk(&self, c: usize) -> Result<(), LispError> {
        let fresh = map_chunk_region()?;
        if self.chunks[c]
            .compare_exchange(
                std::ptr::null_mut(),
                fresh,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            unmap_chunk_region(fresh);
        } else {
            CHUNKS_MAPPED.fetch_add(1, Ordering::Relaxed);
        }
        Ok(())
    }

    /// Give every mapped chunk's committed pages back and leave all of its slots `EMPTY`.
    /// The mappings stay (see "Lifetime"): an op racing the drop of an escaped table
    /// touches valid memory, and the chunks are ready for the slot's next table.
    fn release_pages(&self) {
        for ptr in self.chunks.iter() {
            let chunk = ptr.load(Ordering::Acquire);
            if !chunk.is_null() {
                release_chunk_pages(chunk);
            }
        }
    }

    /// Every mapped slot below index `max`, in index order, with its index — a whole
    /// unmapped chunk is skipped without a read, so a scan (count / migrate / snapshot
    /// / drop) over a sparse table costs its touched chunks, never the region.
    fn slots_below(&self, max: usize) -> impl Iterator<Item = (usize, &AtomicU64)> + '_ {
        let max = max.min(DENSE_KEY_MAX as usize);
        self.chunks
            .iter()
            .enumerate()
            .take_while(move |(c, _)| c << CHUNK_SHIFT < max)
            .filter_map(move |(c, ptr)| {
                let chunk = ptr.load(Ordering::Acquire);
                if chunk.is_null() {
                    return None;
                }
                let base = c << CHUNK_SHIFT;
                let len = (max - base).min(CHUNK_SLOTS);
                // SAFETY: an installed chunk is a live `CHUNK_SLOTS`-word mapping and
                // `len <= CHUNK_SLOTS`.
                let words = unsafe { std::slice::from_raw_parts(chunk as *const AtomicU64, len) };
                Some(words.iter().enumerate().map(move |(k, s)| (base + k, s)))
            })
            .flatten()
    }
}

/// Reserve one chunk: `CHUNK_SLOTS` zeroed atomic words. Returns the mapping's address.
#[cfg(unix)]
fn map_chunk_region() -> Result<*mut AtomicU64, LispError> {
    let bytes = CHUNK_SLOTS * std::mem::size_of::<AtomicU64>();
    // SAFETY: a fresh private anonymous mapping; MAP_ANONYMOUS pages read as zero
    // (= every slot EMPTY) and commit lazily on first write.
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            bytes,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    if ptr == libc::MAP_FAILED {
        let os_error = std::io::Error::last_os_error();
        return Err(LispError::runtime(format!(
            "table: cannot reserve a {} KB dense slot chunk: {os_error}. The chunk is virtual \
             (committed a page at a time), so this is an address-space limit — an `ulimit -v` \
             below what the runtime reserves (allocator arenas + worker stacks, ~3 GB on a \
             28-core box) or a `BROOD_MEM_LIMIT`; raise it or run with fewer cores",
            bytes >> 10
        )));
    }
    Ok(ptr as *mut AtomicU64)
}

#[cfg(unix)]
fn unmap_chunk_region(ptr: *mut AtomicU64) {
    let bytes = CHUNK_SLOTS * std::mem::size_of::<AtomicU64>();
    // SAFETY: `ptr` is a mapping `map_chunk_region` made, never published (the CAS lost).
    unsafe {
        libc::munmap(ptr as *mut libc::c_void, bytes);
    }
}

/// Discard a chunk's pages: the next touch reads zero (= `EMPTY`) and commits afresh.
/// Miri does not model `MADV_DONTNEED`, so under it the portable version below runs: it
/// writes the same zeroes the kernel would fill in.
#[cfg(all(unix, not(miri)))]
fn release_chunk_pages(ptr: *mut AtomicU64) {
    let bytes = CHUNK_SLOTS * std::mem::size_of::<AtomicU64>();
    // SAFETY: `ptr` is an installed chunk — a live private anonymous mapping of `bytes`.
    // MADV_DONTNEED on such a mapping zero-fills on the next access; it never unmaps.
    unsafe {
        libc::madvise(ptr as *mut libc::c_void, bytes, libc::MADV_DONTNEED);
    }
}

#[cfg(any(not(unix), miri))]
fn release_chunk_pages(ptr: *mut AtomicU64) {
    // SAFETY: an installed chunk is a live `CHUNK_SLOTS`-word allocation.
    let words = unsafe { std::slice::from_raw_parts(ptr as *const AtomicU64, CHUNK_SLOTS) };
    for slot in words {
        if slot.load(Ordering::Relaxed) != SLOT_EMPTY {
            slot.store(SLOT_EMPTY, Ordering::Relaxed);
        }
    }
}

#[cfg(not(unix))]
fn map_chunk_region() -> Result<*mut AtomicU64, LispError> {
    // Fallback: one zeroed allocation (committed up front — the unix path's
    // lazy-commit is an optimization, not a semantic requirement).
    let layout = std::alloc::Layout::array::<AtomicU64>(CHUNK_SLOTS).expect("layout");
    // SAFETY: AtomicU64 is repr(transparent) over u64 and all-zero is a valid
    // (EMPTY) value; an installed chunk is leaked (never freed), matching the unix path.
    let ptr = unsafe { std::alloc::alloc_zeroed(layout) };
    if ptr.is_null() {
        return Err(LispError::runtime(format!(
            "table: cannot allocate a {} KB dense slot chunk",
            layout.size() >> 10
        )));
    }
    Ok(ptr as *mut AtomicU64)
}

#[cfg(not(unix))]
fn unmap_chunk_region(ptr: *mut AtomicU64) {
    let layout = std::alloc::Layout::array::<AtomicU64>(CHUNK_SLOTS).expect("layout");
    // SAFETY: `ptr` came from `alloc_zeroed` with this layout and was never published.
    unsafe { std::alloc::dealloc(ptr as *mut u8, layout) }
}

/// One shared store.
///
/// **Dense** (the birth representation): int keys in `[0, DENSE_KEY_MAX)` with
/// tagged-scalar values live in `slots` (see [`DenseSlots`]), indexed directly by
/// key. A `put`/`get`/`has?`/`incr` is ONE atomic op on the key's slot plus one
/// flag load — no structural hash, no bucket probe, no mutex (the per-op lock
/// round-trip and the old 16-byte-enum second cache line were most of `sieve`'s
/// per-op cost). `dense_count` tracks non-EMPTY slots exactly (swap-based, so
/// concurrent puts/deletes never drift) for `table-count`; `dense_max` is a
/// watermark (1 + highest index ever written) bounding migration/snapshot/drop
/// scans so they never touch — and so never commit — the untouched tail.
///
/// **Hashed**: everything else — the original `hash → bucket` map, under `hashed`'s
/// `Mutex` (`Some` once migrated). A dense store migrates here (one-time, O(max))
/// the first time an op doesn't fit the dense shape; it never migrates back.
///
/// **Migration protocol** (why lock-free dense ops can't race it wrong). The
/// migrator (holding the mutex) stores `dense = false` (SeqCst), reads the
/// watermark, then captures each non-EMPTY slot in `[0, max]` with
/// `swap(MOVED)`. Every dense op performs its slot op and then **re-checks the
/// flag** (a plain load on x86):
///
///   - Flag still true ⟹ the op's slot access is SeqCst-ordered *before* the
///     migrator's flag store, hence before every capture read — the migrator
///     sees its effect. (A writer beyond the old watermark updates `dense_max`
///     *before* its slot op, so the capture scan covers it by the same order.)
///   - Flag false (or the slot op observed `MOVED`) ⟹ ambiguity: re-route
///     through the hashed path, which blocks on the mutex the migrator holds.
///     `put`/`delete` re-apply (last-writer-wins set semantics — idempotent);
///     `get`/`has` re-read; `incr` resolves its one ambiguous case (CAS
///     succeeded, flag now false) under the lock by inspecting the slot: MOVED
///     means the migrator captured the incremented word (done — return it),
///     anything else means it skipped this slot as EMPTY before the CAS landed
///     (re-execute the increment on the map).
///
/// The protocol is model-checked in `tests/loom_table_protocol.rs` (a faithful
/// miniature — the real slots live in an mmap loom can't instrument). NOTE:
/// the model expresses the store→load orderings as explicit SeqCst FENCES
/// because loom 0.7 does not model the C11 SC total order for plain SeqCst
/// accesses (the litmus in that file demonstrates it); the real code needs no
/// fences — its SeqCst RMWs/stores/loads already carry that order per C11.
struct Store {
    slots: DenseSlots,
    dense_count: AtomicUsize,
    /// Watermark: 1 + the highest dense index ever written.
    dense_max: AtomicUsize,
    /// Latched once [`jit_dense_base`] hands this store's slot region to JIT'd
    /// code. Inline ops are a bare atomic on the key's slot — no watermark or
    /// count upkeep — so from that point `dense_count`/`dense_max` are lower
    /// bounds only: `table-count` tallies by scan and migration/snapshot/drop
    /// scan the FULL region (reads of untouched pages skip without committing
    /// them). Set (SeqCst) *before* the pointer escapes, so any scan that could
    /// observe an inline write also observes the latch.
    jit_shared: AtomicBool,
    /// Fast-path filter + the migration fence (see the protocol above): true
    /// until the store migrates.
    dense: AtomicBool,
    /// The hashed representation (`Some` once migrated) — also the lock that
    /// serializes migration, snapshots, and every hashed op.
    hashed: Mutex<Option<StoreMap>>,
    /// Tombstone for `table-drop`. The registry is append-only + lock-free, so a
    /// dropped table is flagged in place rather than removed; `lookup` treats a
    /// tombstoned store as gone.
    dropped: AtomicBool,
    /// This registry slot's generation, bumped each time [`recycle`] hands the slot to a
    /// new table. It is the handle's high half, so a handle to the slot's previous table
    /// fails `lookup` rather than reaching the new one.
    generation: AtomicU32,
    /// The pid of the process this table dies with, or 0: no owner (created outside a
    /// process) or pinned by a global (see "Lifetime").
    owner: AtomicU64,
    /// Latched when the handle leaves its creating process — sent, or promoted with a
    /// closure or a global. From then on another process may hold the handle, so the
    /// registry slot is never recycled.
    escaped: AtomicBool,
}

impl Store {
    /// The slot range every scan (count/migrate/snapshot/drop) must cover:
    /// the exact watermark normally, the whole region once JIT'd code holds the
    /// slot pointer (see `jit_shared`).
    #[inline]
    fn scan_max(&self) -> usize {
        if self.jit_shared.load(Ordering::SeqCst) {
            DENSE_KEY_MAX as usize
        } else {
            self.dense_max.load(Ordering::SeqCst)
        }
    }

    /// Raise the watermark to cover index `i` — BEFORE the slot op (load-bearing
    /// for the migration protocol above).
    #[inline]
    fn cover(&self, i: usize) {
        if i >= self.dense_max.load(Ordering::Relaxed) {
            self.dense_max.fetch_max(i + 1, Ordering::SeqCst);
        }
    }

    /// Migrate to the hashed representation, preserving every entry. MUST be
    /// called with `guard` = the held `hashed` lock and `guard.is_none()`.
    fn migrate_to_hashed(&self, guard: &mut MutexGuard<'_, Option<StoreMap>>) {
        self.dense.store(false, Ordering::SeqCst);
        let mut map = StoreMap::default();
        for (k, slot) in self.slots.slots_below(self.scan_max()) {
            // Skip EMPTY without writing (an untouched page stays untouched);
            // capture the rest with swap(MOVED) — the swap's return value is
            // authoritative even against a concurrent last-instant write.
            if slot.load(Ordering::SeqCst) == SLOT_EMPTY {
                continue;
            }
            let s = slot.swap(SLOT_MOVED, Ordering::SeqCst);
            if let Some(vm) = slot_to_message(s) {
                // Int keys hash exactly as the hashed ops hash them (the
                // heap's int fast path is heap-independent).
                let hash = Heap::hash_int(k as i64);
                map.entry(hash)
                    .or_default()
                    .push((Message::Int(k as i64), vm));
            }
        }
        **guard = Some(map);
    }

    /// The lock-free dense read both `get` and `has` start with. `Some(slot)` is the
    /// answer: the encoded slot (`SLOT_EMPTY` when `key` is absent), or `SLOT_EMPTY` for a
    /// key that can't be dense at all. `None` means read through the hashed path — the
    /// store is not dense, or a migration is moving the slot under us (checked after the
    /// slot load, so a value read before the move is never returned once the dense flag
    /// has dropped).
    fn dense_read(&self, key: Value) -> Option<u64> {
        if !self.dense.load(Ordering::Acquire) {
            return None;
        }
        // An out-of-shape key can't be present in a dense store.
        let Some(i) = dense_idx(key) else {
            return Some(SLOT_EMPTY);
        };
        // An unmapped chunk: no dense write ever reached this key's range.
        let s = self
            .slots
            .slot(i)
            .map_or(SLOT_EMPTY, |slot| slot.load(Ordering::SeqCst));
        (s != SLOT_MOVED && self.dense.load(Ordering::SeqCst)).then_some(s)
    }

    /// The held-lock hashed map, migrating first if this store is still dense — or
    /// an error if the table was dropped. `drop_table` tombstones and empties the
    /// store under this same lock, so an op that passed `lookup` before the drop and
    /// reaches the lock after it sees the tombstone here; without the check it
    /// migrated the cleared store and resurrected a hashed map on a dropped table
    /// (written to, and never freed).
    fn hashed_or_migrate<'g>(
        &self,
        guard: &'g mut MutexGuard<'_, Option<StoreMap>>,
    ) -> Result<&'g mut StoreMap, LispError> {
        if self.dropped.load(Ordering::SeqCst) {
            return Err(LispError::runtime(
                "table: no such table — it was dropped, or the process that created it has exited",
            ));
        }
        if guard.is_none() {
            self.migrate_to_hashed(guard);
        }
        Ok(guard.as_mut().expect("migrated above"))
    }
}

/// The table registry is **lock-free**: an append-only `boxcar::Vec` of stores. A
/// `table-put`/`get`/`has?` resolves its store with a single lock-free `get` + a borrow —
/// no registry mutex and no `Arc` clone per op, which is what makes a hot `Table` loop
/// (`sieve`, the regex DFA memo, a process's state map) cheap. Entries are never removed
/// (a drop tombstones in place, and [`recycle`] reuses the slot), so a `&Store` is stable
/// for the whole process lifetime and safe to hand out as `'static`.
static REGISTRY: LazyLock<boxcar::Vec<Store>> = LazyLock::new(boxcar::Vec::new);

/// Registry slots whose table was dropped while no other process could hold its handle —
/// reset and generation-bumped by [`recycle`], waiting for the next [`create_owned`].
static FREE_SLOTS: Mutex<Vec<usize>> = Mutex::new(Vec::new());

/// A table handle: the registry slot's generation in the high half, `index + 1` in the low
/// half (0 is reserved as "no table"). A slot's first table has generation 0, so its
/// handle is the plain `index + 1`.
fn handle(index: usize, generation: u32) -> u64 {
    ((generation as u64) << 32) | (index as u64 + 1)
}

/// The store `id` names and that handle's generation, if the slot exists — whether the
/// generation is still current is the caller's question.
fn slot_of(id: u64) -> Option<(&'static Store, u32)> {
    let index = (id & 0xFFFF_FFFF).checked_sub(1)? as usize;
    Some((REGISTRY.get(index)?, (id >> 32) as u32))
}

/// The store `id` names while that table is the slot's current one — dropped or not.
fn current_store(id: u64) -> Option<&'static Store> {
    slot_of(id).and_then(|(store, generation)| {
        (store.generation.load(Ordering::Relaxed) == generation).then_some(store)
    })
}

/// The live table holding the most memory — its id and an estimate of the bytes — for the
/// soft memory limit's report when no process holds an attributable share
/// (`process/pressure.rs`). A table belongs to no process, so stopping one frees nothing it
/// stores; naming it is what the limit can do. Heavy: it walks every hashed entry of every
/// table under that table's lock, so it runs only at such a crossing.
pub(crate) fn largest_table() -> Option<(u64, usize)> {
    REGISTRY
        .iter()
        .filter(|(_, store)| !store.dropped.load(Ordering::SeqCst))
        .map(|(index, store)| {
            let dense = store.dense_count.load(Ordering::Relaxed) * std::mem::size_of::<u64>();
            let hashed: usize = lock(&store.hashed).as_ref().map_or(0, |map| {
                map.values()
                    .flat_map(|bucket| bucket.iter())
                    .map(|(key, value)| {
                        crate::process::message::message_bytes(key)
                            + crate::process::message::message_bytes(value)
                    })
                    .sum()
            });
            (
                handle(index, store.generation.load(Ordering::Relaxed)),
                dense + hashed,
            )
        })
        .max_by_key(|&(_, bytes)| bytes)
}

/// Resolve a handle to its store, or a clean error if it was dropped / never existed.
/// Lock-free: one `boxcar::Vec::get` (a stable ref) plus the tombstone and generation
/// checks.
fn lookup(id: u64) -> Result<&'static Store, LispError> {
    match current_store(id) {
        Some(store) if !store.dropped.load(Ordering::Relaxed) => Ok(store),
        _ => Err(LispError::runtime(format!(
            "table {id}: no such table — it was dropped, or the process that created it has \
             exited"
        ))
        .with_hint(
            "A table lives as long as the process that made it, unless a global holds it \
             (ADR-418): `def` it, or create it in the process that outlives its readers.",
        )),
    }
}

/// Reject a key that can't reliably be looked up again — i.e. one for which the
/// store's lookup (`hash_value` to a bucket, `from_message`+`equal` to resolve) could
/// never match it back. Two classes:
///   - **identity values** (`Fn`/`Macro`/`Native`): a closure compares by handle
///     identity, which a stored deep-copy can't preserve — put would succeed but every
///     get miss. (Macros/builtins also can't even be serialized.)
///   - **NaN**: `NaN != NaN`, so a NaN key never equals itself — it would be
///     unretrievable, and each put would append a new (dead) entry.
/// Plain data and the id-stable handles (`Pid`/`Ref`/`Socket`/`Subprocess`/`Table`)
/// round-trip fine and are allowed.
///
/// This guards the *top-level* key only. A bad value *nested inside* a compound key
/// (e.g. a closure or NaN inside a vector key) has the identical hazard — but that is
/// exactly how such values behave as immutable-**map** keys too (table keys reuse map
/// equality), so it's a documented property, not walked, to keep the hot path cheap.
pub fn check_key(who: &str, key: Value) -> Result<(), LispError> {
    let reason = match key {
        Value::Fn(_) | Value::Macro(_) | Value::Native(_) => format!(
            "a {} cannot be a table key — it compares by identity, which a stored copy can't preserve",
            crate::core::value::tag(key).name()
        ),
        Value::Float(f) if f.is_nan() => {
            "NaN cannot be a table key — it never equals itself, so it could never be looked up"
                .to_string()
        }
        _ => return Ok(()),
    };
    Err(LispError::type_err(format!("{}: {}", who, reason)))
}

/// `(%table)` from outside any process — a table no process owns (see "Lifetime").
pub fn create() -> u64 {
    create_owned(0)
}

/// `(%table)` — create a new empty table owned by process `owner` (0 = none), reusing a
/// recycled registry slot when one is free. The dense slot region is reserved lazily on
/// the first dense write, so an unused (or immediately-hashed) table costs only its shell.
pub fn create_owned(owner: u64) -> u64 {
    if let Some(index) = lock(&FREE_SLOTS).pop() {
        let store = &REGISTRY[index];
        store.owner.store(owner, Ordering::Relaxed);
        store.escaped.store(false, Ordering::Relaxed);
        // Last: the slot is live from here.
        store.dropped.store(false, Ordering::SeqCst);
        return handle(index, store.generation.load(Ordering::Relaxed));
    }
    let index = REGISTRY.push(Store {
        slots: DenseSlots::new(),
        dense_count: AtomicUsize::new(0),
        dense_max: AtomicUsize::new(0),
        jit_shared: AtomicBool::new(false),
        dense: AtomicBool::new(true),
        hashed: Mutex::new(None),
        dropped: AtomicBool::new(false),
        generation: AtomicU32::new(0),
        owner: AtomicU64::new(owner),
        escaped: AtomicBool::new(false),
    });
    handle(index, 0)
}

/// `(%table-drop t)` — end a table. Idempotent; returns whether it was still live. Frees
/// the hashed map and the dense pages, and recycles the registry slot when no other
/// process can hold the handle and no JIT'd loop of this one can hold its slot base (see
/// "Lifetime"). A handle to an earlier table of a recycled slot drops nothing.
pub fn drop_table(id: u64) -> bool {
    let Some(store) = current_store(id) else {
        return false;
    };
    let was_live = tombstone(store);
    if was_live && !store.jit_shared.load(Ordering::SeqCst) {
        recycle_if_private(id, store);
    }
    was_live
}

/// `(%table-free t)` — drop a table the compiler built privately (the linear-map
/// rewrite's accumulator) and recycle its slot even though JIT'd code used it: the rewrite
/// frees the table after the loop that hoisted its slot base has returned, so no frame
/// holds it. Unlike `table-drop` this is not for user code — a user's loop may hold the
/// base across a `table-drop` it calls.
pub fn free_private(id: u64) {
    if let Some(store) = current_store(id)
        && tombstone(store)
    {
        recycle_if_private(id, store);
    }
}

/// Tombstone `store` and release its memory, returning whether it was still live. The whole
/// drop runs under the store lock — the lock that serializes migration — and
/// `hashed_or_migrate` re-checks the tombstone under it, so no op that raced past `lookup`
/// can migrate the emptied store back into a live hashed map afterwards.
fn tombstone(store: &Store) -> bool {
    let mut guard = lock(&store.hashed);
    let was_live = !store.dropped.swap(true, Ordering::SeqCst);
    if was_live {
        *guard = None;
        // Retire the dense fast paths too: any JIT'd inline op re-checks this flag and
        // re-routes to the FFI, whose `lookup` then reports the drop — so a dropped table
        // errors instead of silently writing into the retained region.
        store.dense.store(false, Ordering::SeqCst);
        store.slots.release_pages();
        store.dense_count.store(0, Ordering::Relaxed);
    }
    was_live
}

/// Hand a just-tombstoned table's slot to the next `create_owned`, unless its handle
/// escaped this process. The caller vouches that no JIT'd loop still holds the slot base.
fn recycle_if_private(id: u64, store: &Store) {
    if store.escaped.load(Ordering::SeqCst) {
        return;
    }
    let Some(index) = (id & 0xFFFF_FFFF).checked_sub(1) else {
        return;
    };
    // The pages were released by the tombstone, so every slot reads EMPTY again and the
    // watermark can start over. `dropped` stays set until `create_owned` hands it out.
    store.dense_max.store(0, Ordering::Relaxed);
    store.jit_shared.store(false, Ordering::Relaxed);
    store.dense.store(true, Ordering::SeqCst);
    store.owner.store(0, Ordering::Relaxed);
    store.generation.fetch_add(1, Ordering::SeqCst);
    lock(&FREE_SLOTS).push(index as usize);
}

/// A table handle is leaving its creating process — sent in a message, or promoted into
/// the shared region with a `spawn`ed closure or a global. Latches `escaped`; `pin` (a
/// global's value) also clears the owner, so the table outlives its creator.
pub(crate) fn note_shared(id: u64, pin: bool) {
    if let Some(store) = current_store(id) {
        store.escaped.store(true, Ordering::SeqCst);
        if pin {
            store.owner.store(0, Ordering::SeqCst);
        }
    }
}

/// Whether `pid` still owns table `id` — live, not pinned, not handed on.
pub(crate) fn owned_by(id: u64, pid: u64) -> bool {
    current_store(id).is_some_and(|store| {
        !store.dropped.load(Ordering::Relaxed) && store.owner.load(Ordering::SeqCst) == pid
    })
}

/// Process `pid` has exited: drop every table in `ids` it still owns. Its loops have all
/// ended, so a table that never escaped is recycled whether or not JIT'd code used it.
/// The owner is claimed with a CAS, so a `def` pinning the table concurrently either wins
/// (the table lives) or finds it already dropped.
pub(crate) fn release_owned(pid: u64, ids: &[u64]) {
    for &id in ids {
        let Some(store) = current_store(id) else {
            continue;
        };
        if store
            .owner
            .compare_exchange(pid, 0, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
            && tombstone(store)
        {
            recycle_if_private(id, store);
        }
    }
}

/// `(%table-count t)` — number of entries. Once JIT'd code holds the dense slot
/// region (`jit_shared` — inline ops don't maintain the exact counter), the
/// dense count is a full-region tally instead: O(region), still exact.
pub fn count(id: u64) -> Result<i64, LispError> {
    let store = lookup(id)?;
    if store.dense.load(Ordering::Acquire) {
        if !store.jit_shared.load(Ordering::SeqCst) {
            return Ok(store.dense_count.load(Ordering::Relaxed) as i64);
        }
        let mut n = 0i64;
        for (_, slot) in store.slots.slots_below(store.scan_max()) {
            let s = slot.load(Ordering::Relaxed);
            if s != SLOT_EMPTY && s != SLOT_MOVED {
                n += 1;
            }
        }
        if store.dense.load(Ordering::SeqCst) {
            return Ok(n);
        }
        // A migration raced the tally — fall through to the hashed count.
    }
    let data = lock(&store.hashed);
    match &*data {
        Some(map) => Ok(map.values().map(|b| b.len()).sum::<usize>() as i64),
        // Raced a migration that hadn't published the map when we read the flag.
        None => Ok(store.dense_count.load(Ordering::Relaxed) as i64),
    }
}

/// The dense-array index for `key`, when `key` is an int the dense shape can hold.
#[inline]
fn dense_idx(key: Value) -> Option<usize> {
    match key {
        Value::Int(n) if (0..DENSE_KEY_MAX).contains(&n) => Some(n as usize),
        _ => None,
    }
}

/// Index in `bucket` whose stored key equals `key` — reconstructing each candidate
/// into `heap` and comparing with Brood structural equality (so collisions resolve
/// exactly as map keys do). Buckets are size 0–1 except on a genuine hash collision.
fn find_idx(heap: &mut Heap, bucket: &[(Message, Message)], key: Value) -> Option<usize> {
    bucket.iter().position(|(km, _)| {
        // A scalar or string key compares without being rebuilt in the heap first — the
        // common table key (an index, an id, a keyword, a name), on every get/put/has?.
        if let Some(eq) = direct_key_eq(heap, km, key) {
            return eq;
        }
        let k = from_message(heap, km);
        heap.equal(key, k)
    })
}

/// Whether the stored key `km` equals `key`, decided without reconstructing it when
/// both are immediate scalars or either is a string; `None` otherwise (a compound key
/// resolves through `from_message` + `equal`, exactly as before). Structural equality's
/// rules: an int and a float are never equal, NaN is refused as a key upstream, and a
/// string equals only a string with the same text (`from_message` rebuilds a stored
/// `Str` / `StrShared` as a `Value::Str`, and `equal` has no other arm for one).
fn direct_key_eq(heap: &Heap, km: &Message, key: Value) -> Option<bool> {
    Some(match (km, key) {
        (Message::Int(a), Value::Int(b)) => *a == b,
        (Message::Float(a), Value::Float(b)) => *a == b,
        (Message::Keyword(a), Value::Keyword(b)) => *a == b,
        (Message::Sym(a), Value::Sym(b)) => *a == b,
        (Message::Bool(a), Value::Bool(b)) => *a == b,
        (Message::Nil, Value::Nil) => true,
        (Message::Str(text), Value::Str(id)) => text.as_str() == heap.string(id),
        (Message::StrShared(blob), Value::Str(id)) => blob.as_str() == Some(heap.string(id)),
        (Message::Str(_) | Message::StrShared(_), _) => false,
        (
            Message::Int(_)
            | Message::Float(_)
            | Message::Keyword(_)
            | Message::Sym(_)
            | Message::Bool(_)
            | Message::Nil,
            Value::Int(_)
            | Value::Float(_)
            | Value::Keyword(_)
            | Value::Sym(_)
            | Value::Bool(_)
            | Value::Nil
            | Value::Str(_),
        ) => false,
        _ => return None,
    })
}

/// `(%table-put t k v)` — store a clone of `v` under a clone of `k`, overwriting any
/// existing entry for `k`. Returns the table handle (for threading).
pub fn put(heap: &mut Heap, id: u64, key: Value, val: Value) -> LispResult {
    let store = lookup(id)?;
    // Dense fast path: int key in range + tagged-scalar value → ONE atomic swap
    // on the key's slot. Lock-free; see the migration protocol on `Store`.
    if store.dense.load(Ordering::Acquire)
        && let (Some(i), Some(word)) = (dense_idx(key), slot_enc(val))
    {
        store.cover(i);
        let old = store.slots.slot_or_map(i)?.swap(word, Ordering::SeqCst);
        if old != SLOT_MOVED && store.dense.load(Ordering::SeqCst) {
            if old == SLOT_EMPTY {
                store.dense_count.fetch_add(1, Ordering::Relaxed);
            }
            return Ok(Value::table(id));
        }
        // A migration raced us — re-apply on the map (idempotent overwrite).
    }
    // Hashed path (migrating first if still dense — an out-of-shape key/value
    // leaves the dense world for good).
    let mut guard = lock(&store.hashed);
    let hash = heap.hash_value(key);
    // An overwrite keeps the stored key, so search an already-hashed, live table first
    // and copy the key out of the heap only for an insert. A found key equals a stored
    // one, so it is sendable and its `to_message` could not have failed — skipping it
    // changes no error. A dense table (no map yet) or a dropped one is not searched here:
    // the original order below runs — the key, then the value, then the dropped check.
    let searched = match guard.as_mut() {
        Some(map) if !store.dropped.load(Ordering::SeqCst) => {
            let found = map
                .get(&hash)
                .and_then(|bucket| find_idx(heap, bucket, key));
            if let Some(i) = found {
                let vm = to_message(heap, val)?;
                if let Some(bucket) = map.get_mut(&hash) {
                    bucket[i].1 = vm;
                }
                return Ok(Value::table(id));
            }
            true
        }
        _ => false,
    };
    // Clone both out of the GC heap (also rejects non-sendable values).
    let km = to_message(heap, key)?;
    let vm = to_message(heap, val)?;
    let map = store.hashed_or_migrate(&mut guard)?;
    let bucket = map.entry(hash).or_default();
    // A map that existed was searched above and has no such key; one migrated just now
    // from dense may hold it (an int key whose new value is not a dense scalar).
    match if searched {
        None
    } else {
        find_idx(heap, bucket, key)
    } {
        Some(i) => bucket[i].1 = vm,
        None => bucket.push((km, vm)),
    }
    Ok(Value::table(id))
}

/// `(%table-get t k [default])` — a fresh copy of the value under `k`, or `default`.
pub fn get(heap: &mut Heap, id: u64, key: Value, default: Value) -> LispResult {
    let store = lookup(id)?;
    if let Some(s) = store.dense_read(key) {
        return Ok(slot_dec(s).unwrap_or(default));
    }
    let found = {
        let mut guard = lock(&store.hashed);
        let map = store.hashed_or_migrate(&mut guard)?;
        let hash = heap.hash_value(key);
        // Rebuilt in the heap under the lock, straight from the stored form: cloning
        // first to release the lock sooner would cost an allocation and a deep copy per
        // read of a compound value, for a lock no other process holds in the common case.
        match map.get(&hash) {
            Some(bucket) => find_idx(heap, bucket, key).map(|i| from_message(heap, &bucket[i].1)),
            None => None,
        }
    };
    Ok(found.unwrap_or(default))
}

/// `(%table-has? t k)` — whether `k` is present.
pub fn has(heap: &mut Heap, id: u64, key: Value) -> Result<bool, LispError> {
    let store = lookup(id)?;
    if let Some(s) = store.dense_read(key) {
        return Ok(s != SLOT_EMPTY);
    }
    let mut guard = lock(&store.hashed);
    let map = store.hashed_or_migrate(&mut guard)?;
    let hash = heap.hash_value(key);
    Ok(map
        .get(&hash)
        .is_some_and(|bucket| find_idx(heap, bucket, key).is_some()))
}

/// `(%table-delete t k)` — remove `k` if present. Returns the table handle.
pub fn delete(heap: &mut Heap, id: u64, key: Value) -> LispResult {
    let store = lookup(id)?;
    if store.dense.load(Ordering::Acquire) {
        match dense_idx(key) {
            Some(i) => {
                // An unmapped chunk: nothing was ever stored densely in this key's range.
                let Some(slot) = store.slots.slot(i) else {
                    return Ok(Value::table(id));
                };
                store.cover(i);
                let old = slot.swap(SLOT_EMPTY, Ordering::SeqCst);
                if old != SLOT_MOVED && store.dense.load(Ordering::SeqCst) {
                    if old != SLOT_EMPTY {
                        store.dense_count.fetch_sub(1, Ordering::Relaxed);
                    }
                    return Ok(Value::table(id));
                }
                // Migration raced us — re-apply on the map (idempotent).
            }
            // An out-of-shape key can't be present in a dense store — no-op.
            None => return Ok(Value::table(id)),
        }
    }
    let mut guard = lock(&store.hashed);
    let map = store.hashed_or_migrate(&mut guard)?;
    let hash = heap.hash_value(key);
    let now_empty = if let Some(bucket) = map.get_mut(&hash) {
        if let Some(i) = find_idx(heap, bucket, key) {
            bucket.swap_remove(i);
        }
        bucket.is_empty()
    } else {
        false
    };
    if now_empty {
        map.remove(&hash);
    }
    Ok(Value::table(id))
}

/// Why an integer increment could not be applied as one. `incr` turns each into its
/// error; `add` turns each into the generic `+` path.
enum IncrMiss {
    /// The stored value is not an `i64` (a float, a string, a bignum, …).
    NotInt,
    /// The stored value is an integer outside the ±2^63 range (a bignum).
    BigInt,
    /// `cur + delta` leaves the i64 range.
    Overflow,
}

/// `(%table-incr t k [delta])` — **atomically** add `delta` (default 1) to the integer
/// at `k` (treating an absent key as 0) and return the new value. On the dense path
/// this is a lock-free CAS loop on the key's slot (concurrent increments never lose
/// an update, and a racing migration can neither lose nor double-apply one — see the
/// protocol on `Store`); on the hashed path the whole read-modify-write happens under
/// the store lock. Errors if the existing value is not a plain integer.
pub fn incr(heap: &mut Heap, id: u64, key: Value, delta: i64) -> LispResult {
    match incr_int(heap, id, key, delta)? {
        Ok(next) => Ok(Value::int(next)),
        Err(IncrMiss::NotInt) => Err(LispError::type_err(
            "table/incr: the value at this key is not an integer",
        )),
        // A bignum *is* an integer in Brood, but table/incr deliberately works only
        // in the i64 range (a counter primitive) — say so precisely.
        Err(IncrMiss::BigInt) => Err(LispError::type_err(
            "table/incr: the value at this key is an integer outside the ±2^63 range that table/incr supports",
        )),
        Err(IncrMiss::Overflow) => Err(LispError::runtime(
            "table/incr: incrementing would exceed the ±2^63 range",
        )),
    }
}

/// `(%table-add t k v)` — store `(+ (get t k 0) v)` at `k` and return the new value:
/// the read-modify-write `(assoc m k (+ (get m k 0) v))` spelled on a table, which is
/// what the linear-map rewrite turns that idiom into (`eval/compile/inline.rs`). The
/// integer case is `incr`'s lock-free path; anything else — a float or bignum on
/// either side, a non-number (the error `+` raises, naming `+`), an i64 overflow (a
/// bignum result, as `+` promotes) — is exactly what `+` does with the stored value,
/// then a `put`. The generic half is a read then a write, NOT atomic against a
/// concurrent writer on the same key: this op serves a table no one else holds; the
/// concurrent counter is `incr`.
pub fn add(heap: &mut Heap, id: u64, key: Value, delta: Value) -> LispResult {
    if let Value::Int(d) = delta
        && let Ok(next) = incr_int(heap, id, key, d)?
    {
        return Ok(Value::int(next));
    }
    let old = get(heap, id, key, Value::int(0))?;
    let next = crate::builtins::numeric::add_values(heap, old, delta)?;
    put(heap, id, key, next)?;
    Ok(next)
}

/// `(%table-sub t k v)` — store `(- (get t k 0) v)` at `k` and return the new value: the
/// subtracting sibling of [`add`], for the tally written `(assoc m k (- (get m k 0) v))`
/// (or with `dec`). Same paths, same non-atomic general half; `-` rather than a negated
/// `+` so a non-number raises `-`'s own error, as the source did.
pub fn sub(heap: &mut Heap, id: u64, key: Value, delta: Value) -> LispResult {
    if let Value::Int(d) = delta
        && let Some(neg) = d.checked_neg()
        && let Ok(next) = incr_int(heap, id, key, neg)?
    {
        return Ok(Value::int(next));
    }
    let old = get(heap, id, key, Value::int(0))?;
    let next = crate::builtins::numeric::sub_values(heap, old, delta)?;
    put(heap, id, key, next)?;
    Ok(next)
}

/// The shared read-modify-write behind `incr` and `add`: `Ok(Ok(next))` when the
/// stored value was an `i64` (or absent) and the sum fits, `Ok(Err(miss))` when it
/// was not — leaving the table untouched — and `Err` only for a dead table.
fn incr_int(
    heap: &mut Heap,
    id: u64,
    key: Value,
    delta: i64,
) -> Result<Result<i64, IncrMiss>, LispError> {
    let store = lookup(id)?;
    if store.dense.load(Ordering::Acquire)
        && let Some(i) = dense_idx(key)
    {
        store.cover(i);
        let slot = store.slots.slot_or_map(i)?;
        let mut cur = slot.load(Ordering::SeqCst);
        loop {
            let (cur_int, was_empty) = match cur {
                SLOT_EMPTY => (0i64, true),
                SLOT_MOVED => break, // migration in flight → hashed path
                s if s & INT_TAG != 0 => ((s as i64) >> 3, false),
                _ => return Ok(Err(IncrMiss::NotInt)),
            };
            let Some(next) = cur_int.checked_add(delta) else {
                return Ok(Err(IncrMiss::Overflow));
            };
            let Some(word) = slot_enc(Value::int(next)) else {
                break; // leaves the 61-bit tagged range → migrate below
            };
            match slot.compare_exchange_weak(cur, word, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => {
                    if store.dense.load(Ordering::SeqCst) {
                        if was_empty {
                            store.dense_count.fetch_add(1, Ordering::Relaxed);
                        }
                        return Ok(Ok(next));
                    }
                    // CAS landed but a migration started: resolve under its
                    // lock. MOVED in the slot ⟹ the migrator captured our
                    // incremented word (done); anything else ⟹ it skipped
                    // this slot as EMPTY before our CAS ⟹ re-execute on the
                    // map (`incr` commutes, so re-executing linearizes).
                    let mut guard = lock(&store.hashed);
                    if slot.load(Ordering::SeqCst) == SLOT_MOVED {
                        return Ok(Ok(next));
                    }
                    // (A DROP also flips `dense` and clears the slot to EMPTY:
                    // `hashed_or_migrate` reports it rather than migrating.)
                    let map = store.hashed_or_migrate(&mut guard)?;
                    return incr_hashed(heap, map, key, delta);
                }
                Err(actual) => cur = actual,
            }
        }
    }
    let mut guard = lock(&store.hashed);
    let map = store.hashed_or_migrate(&mut guard)?;
    incr_hashed(heap, map, key, delta)
}

fn incr_hashed(
    heap: &mut Heap,
    map: &mut StoreMap,
    key: Value,
    delta: i64,
) -> Result<Result<i64, IncrMiss>, LispError> {
    let km = to_message(heap, key)?;
    let hash = heap.hash_value(key);
    let bucket = map.entry(hash).or_default();
    let idx = find_idx(heap, bucket, key);
    let cur = match idx {
        Some(i) => match &bucket[i].1 {
            Message::Int(n) => *n,
            Message::BigInt(_) => return Ok(Err(IncrMiss::BigInt)),
            _ => return Ok(Err(IncrMiss::NotInt)),
        },
        None => 0,
    };
    let Some(next) = cur.checked_add(delta) else {
        return Ok(Err(IncrMiss::Overflow));
    };
    match idx {
        Some(i) => bucket[i].1 = Message::Int(next),
        None => bucket.push((km, Message::Int(next))),
    }
    Ok(Ok(next))
}

/// `(%table-snapshot t)` — a point-in-time copy of the whole table as an immutable
/// Brood map. Because the entries are immutable clones, the returned map is
/// unaffected by later mutation — an MVCC snapshot, not a dirty read. O(n) copy.
/// Atomic per entry; on a **dense** table concurrent lock-free writes to *other*
/// keys may land before or after the copy independently (the hashed path snapshots
/// under the store lock, as before).
pub fn snapshot(heap: &mut Heap, id: u64) -> LispResult {
    let store = lookup(id)?;
    // Snapshot the raw clones first; build the Brood map after (outside any lock).
    let raw: Vec<(Message, Message)> = 'raw: {
        if store.dense.load(Ordering::Acquire) {
            let mut raw = Vec::new();
            for (k, slot) in store.slots.slots_below(store.scan_max()) {
                let s = slot.load(Ordering::SeqCst);
                if s == SLOT_MOVED {
                    break 'raw snapshot_hashed(store)?; // migration in flight
                }
                if let Some(vm) = slot_to_message(s) {
                    raw.push((Message::Int(k as i64), vm));
                }
            }
            if store.dense.load(Ordering::SeqCst) {
                break 'raw raw;
            }
            snapshot_hashed(store)?
        } else {
            snapshot_hashed(store)?
        }
    };
    let mut pairs = Vec::with_capacity(raw.len());
    for (km, vm) in &raw {
        let k = from_message(heap, km);
        let v = from_message(heap, vm);
        pairs.push((k, v));
    }
    // Bulk-build via the transient map builder (rooting-safe, O(result-nodes)).
    let into = match heap.alloc_empty_map() {
        Value::Map(mid) => mid,
        _ => unreachable!("alloc_empty_map returns a Map"),
    };
    Ok(heap.map_from_pairs_into(into, pairs))
}

fn snapshot_hashed(store: &Store) -> Result<Vec<(Message, Message)>, LispError> {
    let mut guard = lock(&store.hashed);
    let map = store.hashed_or_migrate(&mut guard)?;
    Ok(map.values().flat_map(|b| b.iter().cloned()).collect())
}

/// Hand the dense slot region of table `id` to JIT'd code: `(chunk_directory,
/// dense_flag)` raw pointers, or `None` when the table is missing/dropped/
/// hashed. The directory is `CHUNKS` chunk pointers (see [`DenseSlots`]); an inline
/// op loads the pointer for its key's chunk (`key >> CHUNK_SHIFT`), routes to the FFI
/// when it is null (an unmapped chunk — the FFI put maps it, the FFI has? answers
/// absent under the same flag protocol), and otherwise indexes the chunk by
/// `key & (CHUNK_SLOTS - 1)`. Directory and chunks are process-lifetime allocations
/// that never move (stable across GC, compaction, and even `table-drop` — see
/// "Lifetime"), so baked pointers cannot dangle; every inline op re-checks the
/// `dense` flag after its slot access and re-routes to the FFI path when it flipped
/// (migration or drop) — the exact per-op protocol the Rust ops use. Latches
/// `jit_shared` BEFORE the pointer escapes, so scans switch to full-region
/// coverage no later than any inline write they could observe.
#[cfg(feature = "jit")]
pub(crate) fn jit_dense_base(id: u64) -> Option<(*const AtomicPtr<AtomicU64>, *const AtomicBool)> {
    let store = lookup(id).ok()?;
    if !store.dense.load(Ordering::SeqCst) {
        return None;
    }
    store.jit_shared.store(true, Ordering::SeqCst);
    Some((store.slots.directory(), &store.dense as *const AtomicBool))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An op that resolved its store BEFORE a `table-drop` and takes the store lock
    /// AFTER it — the window every hashed op has between `lookup` and `lock` — must
    /// not migrate the emptied store into a live hashed map. The interleaving is
    /// reproduced exactly (resolve, drop, then the op's own locked step), for a store
    /// still dense at the drop and for one already hashed.
    #[test]
    fn an_op_racing_a_drop_cannot_resurrect_the_hashed_map() {
        let mut heap = Heap::new();
        let dense_id = create();
        put(&mut heap, dense_id, Value::int(3), Value::int(4)).expect("dense put");
        let hashed_id = create();
        put(&mut heap, hashed_id, Value::int(-1), Value::int(4)).expect("hashed put");
        for id in [dense_id, hashed_id] {
            // Another process can only be mid-op if the handle reached it — and a table
            // whose handle never escaped has its slot recycled by the drop instead.
            note_shared(id, false);
            let store = lookup(id).expect("live table");
            assert!(drop_table(id));
            let mut guard = lock(&store.hashed);
            assert!(
                store.hashed_or_migrate(&mut guard).is_err(),
                "table {id}: a locked op after the drop was handed a map"
            );
            assert!(
                guard.is_none(),
                "table {id}: the dropped store holds a map again"
            );
            assert!(!store.dense.load(Ordering::SeqCst));
        }
    }
}
