//! In-memory shared table — Brood's ETS (ADR-107).
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
//! the *caller's* heap, so two processes never alias a stored value (Erlang's ETS
//! copy-in/copy-out). Key equality is **borrowed from the heap** (`hash_value` to
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
//! A table lives until `table-drop` or runtime exit (no owner-death GC in v1 — an
//! app-lifetime store created at startup is the model; owner/`heir` semantics are a
//! deferred follow-on). Operating on a dropped/unknown handle is a clean error, never
//! UB. Like the registry's store shells, a dropped table's dense slot region is
//! retained (cleared to `EMPTY`, unusable) until process exit — the lock-free region
//! has no exclusive owner to unmap it; only the hashed map's memory is released.

use crate::core::heap::Heap;
use crate::core::value::Value;
use crate::error::{LispError, LispResult};
use crate::process::{from_message, to_message, Message};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex, MutexGuard, OnceLock};

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
#[derive(Default, Clone, Copy)]
struct BuildIdentityHasher;
impl std::hash::BuildHasher for BuildIdentityHasher {
    type Hasher = IdentityHasher;
    #[inline]
    fn build_hasher(&self) -> IdentityHasher {
        IdentityHasher(0)
    }
}
type StoreMap = HashMap<u64, Bucket, BuildIdentityHasher>;

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
/// the hashed map. The dense region is a single lazily-committed anonymous mapping
/// (2^23 slots × 8 B = 64 MB **virtual**); the OS commits 4 KB pages as slots are
/// first written, so RSS tracks the keys actually used — which is also why the old
/// sparsity guard is gone: one far-out key costs one page, not a 64 MB resize.
pub(crate) const DENSE_KEY_MAX: i64 = 1 << 23;

/// The dense slot region: `DENSE_KEY_MAX` atomic words, all-zero (= `EMPTY`) at
/// birth, reserved on the first dense write. On unix this is one anonymous
/// `mmap` — a virtual reservation whose pages the OS commits on first touch, so
/// an idle or sparse table costs pages, not the full span. (It also deliberately
/// bypasses the ADR-043 counting allocator: 64 MB of untouched reservation
/// against the soft cap would be pure fiction; the hashed side's real
/// allocations are counted as always.) Never unmapped — see "Lifetime" above.
struct DenseSlots(*const AtomicU64);
// SAFETY: the region is shared, immovable, and only ever accessed through
// `&AtomicU64` — exactly what atomics are for.
unsafe impl Send for DenseSlots {}
unsafe impl Sync for DenseSlots {}

impl DenseSlots {
    /// The failure is a Brood error, not a panic. This reservation is the first
    /// large mapping a *small* program makes, so it is the one that hits an
    /// address-space cap that something else filled: on a 28-core box the
    /// allocator's per-thread arenas (20 × 128 MB, `PROT_NONE`) and the worker
    /// stacks (28 × 16 MB) reserve ~3 GB before any table exists, and under
    /// `ulimit -v 4000000` the table's 64 MB is simply the mapping that lands on
    /// the wall. As an `assert!` this aborted the process from inside a JIT
    /// callback that cannot unwind, with a message blaming the table. As an error
    /// it names the cause, is catchable, and the test that hit it fails alone.
    #[cfg(unix)]
    fn try_new() -> Result<Self, LispError> {
        let bytes = DENSE_KEY_MAX as usize * std::mem::size_of::<AtomicU64>();
        // SAFETY: a fresh private anonymous mapping; MAP_ANONYMOUS pages read as
        // zero (= every slot EMPTY) and commit lazily on first write.
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
                "table: cannot reserve the {} MB dense slot region: {os_error}. The region is \
                 virtual (committed a page at a time), so this is an address-space limit — an \
                 `ulimit -v` below what the runtime reserves (allocator arenas + worker stacks, \
                 ~3 GB on a 28-core box) or a `BROOD_MEM_LIMIT`; raise it or run with fewer cores",
                bytes >> 20
            )));
        }
        Ok(DenseSlots(ptr as *const AtomicU64))
    }

    #[cfg(not(unix))]
    fn try_new() -> Result<Self, LispError> {
        // Fallback: one zeroed allocation (committed up front — the unix path's
        // lazy-commit is an optimization, not a semantic requirement).
        let layout =
            std::alloc::Layout::array::<AtomicU64>(DENSE_KEY_MAX as usize).expect("layout");
        // SAFETY: AtomicU64 is repr(transparent) over u64 and all-zero is a valid
        // (EMPTY) value; the region is leaked (never freed), matching the unix path.
        let ptr = unsafe { std::alloc::alloc_zeroed(layout) };
        if ptr.is_null() {
            return Err(LispError::runtime(format!(
                "table: cannot allocate the {} MB dense slot region",
                layout.size() >> 20
            )));
        }
        Ok(DenseSlots(ptr as *const AtomicU64))
    }

    /// The slot for dense index `i`. Callers only produce `i` via [`dense_idx`],
    /// which bounds it below `DENSE_KEY_MAX`.
    #[inline]
    fn slot(&self, i: usize) -> &AtomicU64 {
        debug_assert!(i < DENSE_KEY_MAX as usize);
        // SAFETY: in-bounds within the region mapped in `new`.
        unsafe { &*self.0.add(i) }
    }
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
    slots: OnceLock<DenseSlots>,
    /// Serialises the one-time reservation of `slots`, so two first-users cannot both
    /// `mmap` a region (the loser's would leak — 64 MB of address space, never freed).
    slots_init: Mutex<()>,
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
}

impl Store {
    /// The dense slot region, reserved on first use. Fallible: see
    /// [`DenseSlots::try_new`]. The fast path is one lock-free `get`; the reservation
    /// itself runs under `slots_init`, re-checking after the lock so a racing
    /// first-user finds the winner's region instead of mapping a second one.
    #[inline]
    fn dense_slots(&self) -> Result<&DenseSlots, LispError> {
        if let Some(slots) = self.slots.get() {
            return Ok(slots);
        }
        let _guard = self.slots_init.lock().unwrap_or_else(|e| e.into_inner());
        if self.slots.get().is_none() {
            let fresh = DenseSlots::try_new()?;
            let _ = self.slots.set(fresh);
        }
        Ok(self.slots.get().expect("set under the init lock"))
    }

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
        if let Some(slots) = self.slots.get() {
            let max = self.scan_max();
            for k in 0..max {
                let slot = slots.slot(k);
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
        }
        **guard = Some(map);
    }

    /// The held-lock hashed map, migrating first if this store is still dense.
    fn hashed_or_migrate<'g>(
        &self,
        guard: &'g mut MutexGuard<'_, Option<StoreMap>>,
    ) -> &'g mut StoreMap {
        if guard.is_none() {
            self.migrate_to_hashed(guard);
        }
        guard.as_mut().expect("migrated above")
    }
}

/// The table registry is **lock-free**: an append-only `boxcar::Vec` indexed by
/// `id - 1` (ids are handed out densely by `push` and never reused). A `table-put`/
/// `get`/`has?` resolves its store with a single lock-free `get` + a borrow — no
/// registry mutex and no `Arc` clone per op, which is what makes a hot `Table` loop
/// (`sieve`, the regex DFA memo, a process's state map) cheap. Entries are never
/// removed (drop tombstones in place), so a `&Store` is stable for the whole process
/// lifetime and safe to hand out as `'static`.
static REGISTRY: LazyLock<boxcar::Vec<Store>> = LazyLock::new(boxcar::Vec::new);

/// Resolve a handle to its store, or a clean error if it was dropped / never existed.
/// Lock-free: one `boxcar::Vec::get` (a stable ref) plus the tombstone check.
fn lookup(id: u64) -> Result<&'static Store, LispError> {
    let idx = id
        .checked_sub(1)
        .ok_or_else(|| LispError::runtime(format!("table {}: no such table", id)))?
        as usize;
    match REGISTRY.get(idx) {
        Some(store) if !store.dropped.load(Ordering::Relaxed) => Ok(store),
        _ => Err(LispError::runtime(format!(
            "table {}: no such table (dropped?)",
            id
        ))),
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
            "NaN cannot be a table key — it never equals itself, so it could never be looked up".to_string()
        }
        _ => return Ok(()),
    };
    Err(LispError::type_err(format!("{}: {}", who, reason)))
}

/// `(%table)` — create a new empty table; returns its handle id. `push` hands out the
/// next dense index atomically, so `id = idx + 1` (0 is reserved as "no table").
/// The dense slot region is reserved lazily on the first dense write, so an
/// unused (or immediately-hashed) table costs only this small shell.
pub fn create() -> u64 {
    let idx = REGISTRY.push(Store {
        slots: OnceLock::new(),
        slots_init: Mutex::new(()),
        dense_count: AtomicUsize::new(0),
        dense_max: AtomicUsize::new(0),
        jit_shared: AtomicBool::new(false),
        dense: AtomicBool::new(true),
        hashed: Mutex::new(None),
        dropped: AtomicBool::new(false),
    });
    idx as u64 + 1
}

/// `(%table-drop t)` — tombstone a table (the lock-free registry can't remove entries).
/// Idempotent; returns whether it was still live. Frees the hashed map and clears the
/// touched dense slots to `EMPTY`; the slot region itself (like the store shell) is
/// retained until process exit — the lock-free region has no exclusive owner to unmap.
pub fn drop_table(id: u64) -> bool {
    let Some(idx) = id.checked_sub(1) else {
        return false;
    };
    match REGISTRY.get(idx as usize) {
        Some(store) => {
            let was_live = !store.dropped.swap(true, Ordering::Relaxed);
            if was_live {
                *store.hashed.lock().expect("table store mutex") = None;
                // Retire the dense fast paths too: any JIT'd inline op re-checks
                // this flag and re-routes to the FFI, whose `lookup` then reports
                // the drop — so a dropped table errors instead of silently writing
                // into the retained region.
                store.dense.store(false, Ordering::SeqCst);
                if let Some(slots) = store.slots.get() {
                    let max = store.scan_max();
                    for k in 0..max {
                        // Clear only non-EMPTY slots: a blind store would commit
                        // every untouched page of the full-region scan.
                        let slot = slots.slot(k);
                        if slot.load(Ordering::Relaxed) != SLOT_EMPTY {
                            slot.store(SLOT_EMPTY, Ordering::Relaxed);
                        }
                    }
                }
                store.dense_count.store(0, Ordering::Relaxed);
            }
            was_live
        }
        None => false,
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
        if let Some(slots) = store.slots.get() {
            let mut n = 0i64;
            for k in 0..store.scan_max() {
                let s = slots.slot(k).load(Ordering::Relaxed);
                if s != SLOT_EMPTY && s != SLOT_MOVED {
                    n += 1;
                }
            }
            if store.dense.load(Ordering::SeqCst) {
                return Ok(n);
            }
            // A migration raced the tally — fall through to the hashed count.
        } else {
            return Ok(0);
        }
    }
    let data = store.hashed.lock().expect("table store mutex");
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
        let k = from_message(heap, km);
        heap.equal(key, k)
    })
}

/// `(%table-put t k v)` — store a clone of `v` under a clone of `k`, overwriting any
/// existing entry for `k`. Returns the table handle (for threading).
pub fn put(heap: &mut Heap, id: u64, key: Value, val: Value) -> LispResult {
    let store = lookup(id)?;
    // Dense fast path: int key in range + tagged-scalar value → ONE atomic swap
    // on the key's slot. Lock-free; see the migration protocol on `Store`.
    if store.dense.load(Ordering::Acquire) {
        if let (Some(i), Some(word)) = (dense_idx(key), slot_enc(val)) {
            store.cover(i);
            let old = store.dense_slots()?.slot(i).swap(word, Ordering::SeqCst);
            if old != SLOT_MOVED && store.dense.load(Ordering::SeqCst) {
                if old == SLOT_EMPTY {
                    store.dense_count.fetch_add(1, Ordering::Relaxed);
                }
                return Ok(Value::table(id));
            }
            // A migration raced us — re-apply on the map (idempotent overwrite).
        }
    }
    // Hashed path (migrating first if still dense — an out-of-shape key/value
    // leaves the dense world for good).
    let mut guard = store.hashed.lock().expect("table store mutex");
    // Clone both out of the GC heap (also rejects non-sendable values).
    let km = to_message(heap, key)?;
    let vm = to_message(heap, val)?;
    let hash = heap.hash_value(key);
    let map = store.hashed_or_migrate(&mut guard);
    let bucket = map.entry(hash).or_default();
    match find_idx(heap, bucket, key) {
        Some(i) => bucket[i].1 = vm,
        None => bucket.push((km, vm)),
    }
    Ok(Value::table(id))
}

/// `(%table-get t k [default])` — a fresh copy of the value under `k`, or `default`.
pub fn get(heap: &mut Heap, id: u64, key: Value, default: Value) -> LispResult {
    let store = lookup(id)?;
    if store.dense.load(Ordering::Acquire) {
        match dense_idx(key) {
            Some(i) => {
                let s = match store.slots.get() {
                    Some(slots) => slots.slot(i).load(Ordering::SeqCst),
                    None => SLOT_EMPTY, // no dense write ever happened
                };
                if s != SLOT_MOVED && store.dense.load(Ordering::SeqCst) {
                    return Ok(slot_dec(s).unwrap_or(default));
                }
                // Migration in flight: read through the hashed path below.
            }
            // An out-of-shape key can't be present in a dense store.
            None => return Ok(default),
        }
    }
    let found = {
        let mut guard = store.hashed.lock().expect("table store mutex");
        let map = store.hashed_or_migrate(&mut guard);
        let hash = heap.hash_value(key);
        match map.get(&hash) {
            Some(bucket) => find_idx(heap, bucket, key).map(|i| bucket[i].1.clone()),
            None => None,
        }
        // Reconstruct after releasing the store lock (keeps the lock hold minimal).
    };
    Ok(found.map_or(default, |vm| from_message(heap, &vm)))
}

/// `(%table-has? t k)` — whether `k` is present.
pub fn has(heap: &mut Heap, id: u64, key: Value) -> Result<bool, LispError> {
    let store = lookup(id)?;
    if store.dense.load(Ordering::Acquire) {
        match dense_idx(key) {
            Some(i) => {
                let s = match store.slots.get() {
                    Some(slots) => slots.slot(i).load(Ordering::SeqCst),
                    None => SLOT_EMPTY,
                };
                if s != SLOT_MOVED && store.dense.load(Ordering::SeqCst) {
                    return Ok(s != SLOT_EMPTY);
                }
            }
            None => return Ok(false),
        }
    }
    let mut guard = store.hashed.lock().expect("table store mutex");
    let map = store.hashed_or_migrate(&mut guard);
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
                if store.slots.get().is_none() {
                    return Ok(Value::table(id)); // nothing was ever stored densely
                }
                store.cover(i);
                let old = store
                    .dense_slots()?
                    .slot(i)
                    .swap(SLOT_EMPTY, Ordering::SeqCst);
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
    let mut guard = store.hashed.lock().expect("table store mutex");
    let map = store.hashed_or_migrate(&mut guard);
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

/// `(%table-incr t k [delta])` — **atomically** add `delta` (default 1) to the integer
/// at `k` (treating an absent key as 0) and return the new value. On the dense path
/// this is a lock-free CAS loop on the key's slot (concurrent increments never lose
/// an update, and a racing migration can neither lose nor double-apply one — see the
/// protocol on `Store`); on the hashed path the whole read-modify-write happens under
/// the store lock. Errors if the existing value is not a plain integer.
pub fn incr(heap: &mut Heap, id: u64, key: Value, delta: i64) -> LispResult {
    let store = lookup(id)?;
    if store.dense.load(Ordering::Acquire) {
        if let Some(i) = dense_idx(key) {
            store.cover(i);
            let slot = store.dense_slots()?.slot(i);
            let mut cur = slot.load(Ordering::SeqCst);
            loop {
                let (cur_int, was_empty) = match cur {
                    SLOT_EMPTY => (0i64, true),
                    SLOT_MOVED => break, // migration in flight → hashed path
                    s if s & INT_TAG != 0 => ((s as i64) >> 3, false),
                    _ => {
                        return Err(LispError::type_err(
                            "table-incr: the value at this key is not an integer",
                        ))
                    }
                };
                let next = cur_int.checked_add(delta).ok_or_else(|| {
                    LispError::runtime("table-incr: incrementing would exceed the ±2^63 range")
                })?;
                let Some(word) = slot_enc(Value::int(next)) else {
                    break; // leaves the 61-bit tagged range → migrate below
                };
                match slot.compare_exchange_weak(cur, word, Ordering::SeqCst, Ordering::SeqCst) {
                    Ok(_) => {
                        if store.dense.load(Ordering::SeqCst) {
                            if was_empty {
                                store.dense_count.fetch_add(1, Ordering::Relaxed);
                            }
                            return Ok(Value::int(next));
                        }
                        // CAS landed but a migration started: resolve under its
                        // lock. MOVED in the slot ⟹ the migrator captured our
                        // incremented word (done); anything else ⟹ it skipped
                        // this slot as EMPTY before our CAS ⟹ re-execute on the
                        // map (`incr` commutes, so re-executing linearizes).
                        let mut guard = store.hashed.lock().expect("table store mutex");
                        if slot.load(Ordering::SeqCst) == SLOT_MOVED {
                            return Ok(Value::int(next));
                        }
                        return incr_hashed(heap, store, guard.as_mut(), key, delta);
                    }
                    Err(actual) => cur = actual,
                }
            }
        }
    }
    let mut guard = store.hashed.lock().expect("table store mutex");
    if guard.is_none() {
        store.migrate_to_hashed(&mut guard);
    }
    incr_hashed(heap, store, guard.as_mut(), key, delta)
}

fn incr_hashed(
    heap: &mut Heap,
    _store: &Store,
    map: Option<&mut StoreMap>,
    key: Value,
    delta: i64,
) -> LispResult {
    let map = map.expect("hashed map exists after migration");
    let km = to_message(heap, key)?;
    let hash = heap.hash_value(key);
    let bucket = map.entry(hash).or_default();
    let idx = find_idx(heap, bucket, key);
    let cur = match idx {
        Some(i) => match &bucket[i].1 {
            Message::Int(n) => *n,
            // A bignum *is* an integer in Brood, but table-incr deliberately works only
            // in the i64 range (a counter primitive) — say so precisely.
            Message::BigInt(_) => {
                return Err(LispError::type_err(
                    "table-incr: the value at this key is an integer outside the ±2^63 range that table-incr supports",
                ))
            }
            _ => {
                return Err(LispError::type_err(
                    "table-incr: the value at this key is not an integer",
                ))
            }
        },
        None => 0,
    };
    let next = cur.checked_add(delta).ok_or_else(|| {
        LispError::runtime("table-incr: incrementing would exceed the ±2^63 range")
    })?;
    match idx {
        Some(i) => bucket[i].1 = Message::Int(next),
        None => bucket.push((km, Message::Int(next))),
    }
    Ok(Value::int(next))
}

/// `(%table-snapshot t)` — a point-in-time copy of the whole table as an immutable
/// Brood map. Because the entries are immutable clones, the returned map is
/// unaffected by later mutation — the MVCC win over ETS's dirty reads. O(n) copy.
/// Atomic per entry; on a **dense** table concurrent lock-free writes to *other*
/// keys may land before or after the copy independently (the hashed path snapshots
/// under the store lock, as before).
pub fn snapshot(heap: &mut Heap, id: u64) -> LispResult {
    let store = lookup(id)?;
    // Snapshot the raw clones first; build the Brood map after (outside any lock).
    let raw: Vec<(Message, Message)> = 'raw: {
        if store.dense.load(Ordering::Acquire) {
            let mut raw = Vec::new();
            if let Some(slots) = store.slots.get() {
                let max = store.scan_max();
                for k in 0..max {
                    let s = slots.slot(k).load(Ordering::SeqCst);
                    if s == SLOT_MOVED {
                        break 'raw snapshot_hashed(store); // migration in flight
                    }
                    if let Some(vm) = slot_to_message(s) {
                        raw.push((Message::Int(k as i64), vm));
                    }
                }
            }
            if store.dense.load(Ordering::SeqCst) {
                break 'raw raw;
            }
            snapshot_hashed(store)
        } else {
            snapshot_hashed(store)
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

fn snapshot_hashed(store: &Store) -> Vec<(Message, Message)> {
    let mut guard = store.hashed.lock().expect("table store mutex");
    let map = store.hashed_or_migrate(&mut guard);
    map.values().flat_map(|b| b.iter().cloned()).collect()
}

/// Hand the dense slot region of table `id` to JIT'd code: `(slots_base,
/// dense_flag)` raw pointers, or `None` when the table is missing/dropped/
/// hashed. The region is a process-lifetime anonymous mapping that never moves
/// (stable across GC, compaction, and even `table-drop` — see "Lifetime"), so
/// baked pointers cannot dangle; every inline op re-checks the `dense` flag
/// after its slot access and re-routes to the FFI path when it flipped
/// (migration or drop) — the exact per-op protocol the Rust ops use. Latches
/// `jit_shared` BEFORE the pointer escapes, so scans switch to full-region
/// coverage no later than any inline write they could observe.
#[cfg(feature = "jit")]
pub(crate) fn jit_dense_base(id: u64) -> Option<(*const AtomicU64, *const AtomicBool)> {
    let store = lookup(id).ok()?;
    if !store.dense.load(Ordering::SeqCst) {
        return None;
    }
    // Reserve BEFORE latching: a store whose region cannot be reserved stays on the
    // FFI path (which reports the error) rather than latched with no slots.
    let slots = store.dense_slots().ok()?;
    store.jit_shared.store(true, Ordering::SeqCst);
    Some((slots.0, &store.dense as *const AtomicBool))
}
