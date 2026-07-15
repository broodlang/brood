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
//! The store holds **deep clones in heap-independent [`Message`] form** — the same
//! serialization a cross-process `send` uses. Nothing in the store is ever a live GC
//! handle, so the moving collector never traces or moves into it. `get` reconstructs
//! a **fresh** value in the *caller's* heap, so two processes never alias a stored
//! value (Erlang's ETS copy-in/copy-out). Key equality is **borrowed from the heap**
//! (`hash_value` to bucket, `equal` on a reconstructed key to resolve collisions), so
//! table keys behave identically to immutable-map keys — no parallel equality code.
//!
//! ## Locking discipline
//!
//! The registry is **lock-free** (an append-only `boxcar::Vec` — see `REGISTRY`): a
//! handle resolves to its store with a single indexed read and no lock or `Arc` clone,
//! so per-table ops never contend on a global. Only the store's own `Mutex` is taken,
//! and only per-op — so operations contend solely with operations on the *same* table.
//!
//! ## Lifetime
//!
//! A table lives until `table-drop` or runtime exit (no owner-death GC in v1 — an
//! app-lifetime store created at startup is the model; owner/`heir` semantics are a
//! deferred follow-on). Operating on a dropped/unknown handle is a clean error, never
//! UB.

use crate::core::heap::Heap;
use crate::core::value::Value;
use crate::error::{LispError, LispResult};
use crate::process::{from_message, to_message, Message};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};

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

/// A value in the dense (int-keyed array) representation: the scalar shapes that
/// round-trip without a heap (`nil`/bool/int). `Empty` = the key is absent — distinct
/// from a stored `Nil`, so `table-has?` stays exact. 16 bytes/slot.
#[derive(Clone, Copy, PartialEq)]
enum DenseVal {
    Empty,
    Nil,
    Bool(bool),
    Int(i64),
}

impl DenseVal {
    /// The dense encoding of `v`, or `None` when `v` needs the hashed representation.
    fn encode(v: Value) -> Option<DenseVal> {
        match v {
            Value::Nil => Some(DenseVal::Nil),
            Value::Bool(b) => Some(DenseVal::Bool(b)),
            Value::Int(n) => Some(DenseVal::Int(n)),
            _ => None,
        }
    }
    /// Decode to a `Value` — scalars are immediate, so no heap is needed.
    fn decode(self) -> Option<Value> {
        match self {
            DenseVal::Empty => None,
            DenseVal::Nil => Some(Value::Nil),
            DenseVal::Bool(b) => Some(Value::Bool(b)),
            DenseVal::Int(n) => Some(Value::int(n)),
        }
    }
    fn to_message(self) -> Option<Message> {
        match self {
            DenseVal::Empty => None,
            DenseVal::Nil => Some(Message::Nil),
            DenseVal::Bool(b) => Some(Message::Bool(b)),
            DenseVal::Int(n) => Some(Message::Int(n)),
        }
    }
}

/// Largest int key the dense representation will hold. Beyond it (or for a negative /
/// non-int key, or a non-scalar value) the store migrates to the hashed map. 2^23 slots
/// × 16 B = 128 MB worst-case per table, bounded in practice by the sparsity guard.
const DENSE_KEY_MAX: i64 = 1 << 23;

/// Sparsity guard: growing the dense array to `new_len` is allowed only while the
/// array stays reasonably full (`new_len ≤ 64·count + 4096`), so one far-out key
/// can't balloon RSS — it migrates to the hashed map instead.
fn dense_grow_ok(new_len: usize, count: usize) -> bool {
    new_len <= 64 * (count + 1) + 4096
}

/// A store's physical representation.
///
/// **Dense**: int keys in `[0, DENSE_KEY_MAX)` with scalar (`nil`/bool/int) values live
/// in a flat `Vec` indexed by key — one bounds-check + direct slot access per op: no
/// structural hash, no bucket probe (the 3M-entry `HashMap` probe was ~183 ns of cache
/// misses per op in `sieve`), no `Message` clone of the key. This is the shape of the
/// hot Table workloads (`sieve`'s composite marks, counters, id sets).
///
/// **Hashed**: everything else — the original `hash → bucket` map. A dense store
/// migrates here (one-time O(n)) the first time an op doesn't fit the dense shape;
/// it never migrates back.
enum Storage {
    Dense { vals: Vec<DenseVal>, count: usize },
    Hashed(StoreMap),
}

impl Storage {
    /// Migrate a dense store to the hashed representation, preserving every entry.
    /// Int keys hash through the same `hash_value` fast path the hashed ops use.
    fn migrate_to_hashed(&mut self, heap: &Heap) {
        if let Storage::Dense { vals, .. } = self {
            let mut map = StoreMap::default();
            for (k, dv) in vals.iter().enumerate() {
                if let Some(vm) = dv.to_message() {
                    let hash = heap.hash_value(Value::int(k as i64));
                    map.entry(hash)
                        .or_default()
                        .push((Message::Int(k as i64), vm));
                }
            }
            *self = Storage::Hashed(map);
        }
    }
}

/// One shared store: dense int-keyed array or `hash → bucket of (key-clone,
/// value-clone)` (see [`Storage`]). Bucket equality is resolved against the
/// caller's heap so it matches Brood's `=` exactly.
struct Store {
    data: Mutex<Storage>,
    /// Tombstone for `table-drop`. The registry is append-only + lock-free, so a
    /// dropped table is flagged in place (its data cleared to free memory) rather
    /// than removed; `lookup` treats a tombstoned store as gone.
    dropped: AtomicBool,
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

/// `(table)` — create a new empty table; returns its handle id. `push` hands out the
/// next dense index atomically, so `id = idx + 1` (0 is reserved as "no table").
pub fn create() -> u64 {
    let idx = REGISTRY.push(Store {
        // Every table starts dense (an empty Vec — free); the first op that doesn't
        // fit the dense shape migrates it to the hashed map.
        data: Mutex::new(Storage::Dense {
            vals: Vec::new(),
            count: 0,
        }),
        dropped: AtomicBool::new(false),
    });
    idx as u64 + 1
}

/// `(table-drop t)` — tombstone a table (the lock-free registry can't remove entries).
/// Idempotent; returns whether it was still live. Clears the data to free memory; other
/// handles to it then error on use. Only the small store shell lingers.
pub fn drop_table(id: u64) -> bool {
    let Some(idx) = id.checked_sub(1) else {
        return false;
    };
    match REGISTRY.get(idx as usize) {
        Some(store) => {
            let was_live = !store.dropped.swap(true, Ordering::Relaxed);
            if was_live {
                *store.data.lock().expect("table store mutex") = Storage::Dense {
                    vals: Vec::new(),
                    count: 0,
                };
            }
            was_live
        }
        None => false,
    }
}

/// `(table-count t)` — number of entries.
pub fn count(id: u64) -> Result<i64, LispError> {
    let store = lookup(id)?;
    let data = store.data.lock().expect("table store mutex");
    Ok(match &*data {
        Storage::Dense { count, .. } => *count as i64,
        Storage::Hashed(map) => map.values().map(|b| b.len()).sum::<usize>() as i64,
    })
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

/// `(table-put t k v)` — store a clone of `v` under a clone of `k`, overwriting any
/// existing entry for `k`. Returns the table handle (for threading).
pub fn put(heap: &mut Heap, id: u64, key: Value, val: Value) -> LispResult {
    let store = lookup(id)?;
    let mut data = store.data.lock().expect("table store mutex");
    // Dense fast path: int key in range + scalar value → a bounds-check and a direct
    // slot store. No structural hash, no Message clone, no bucket probe.
    let mut needs_migrate = false;
    if let Storage::Dense { vals, count } = &mut *data {
        match (dense_idx(key), DenseVal::encode(val)) {
            (Some(i), Some(dv)) if i < vals.len() || dense_grow_ok(i + 1, *count) => {
                if i >= vals.len() {
                    vals.resize(i + 1, DenseVal::Empty);
                }
                if vals[i] == DenseVal::Empty {
                    *count += 1;
                }
                vals[i] = dv;
                return Ok(Value::table(id));
            }
            // Out-of-shape key/value (or a too-sparse grow): this table leaves the
            // dense world for good.
            _ => needs_migrate = true,
        }
    }
    if needs_migrate {
        data.migrate_to_hashed(heap);
    }
    // Hashed path — clone both out of the GC heap (also rejects non-sendable values).
    let km = to_message(heap, key)?;
    let vm = to_message(heap, val)?;
    let hash = heap.hash_value(key);
    let Storage::Hashed(map) = &mut *data else {
        unreachable!("dense either stored or migrated above");
    };
    let bucket = map.entry(hash).or_default();
    match find_idx(heap, bucket, key) {
        Some(i) => bucket[i].1 = vm,
        None => bucket.push((km, vm)),
    }
    Ok(Value::table(id))
}

/// `(table-get t k [default])` — a fresh copy of the value under `k`, or `default`.
/// A read never migrates: an out-of-shape key simply can't be present in a dense store.
pub fn get(heap: &mut Heap, id: u64, key: Value, default: Value) -> LispResult {
    let store = lookup(id)?;
    let found = {
        let data = store.data.lock().expect("table store mutex");
        match &*data {
            Storage::Dense { vals, .. } => {
                return Ok(dense_idx(key)
                    .and_then(|i| vals.get(i))
                    .and_then(|dv| dv.decode())
                    .unwrap_or(default));
            }
            Storage::Hashed(map) => {
                let hash = heap.hash_value(key);
                match map.get(&hash) {
                    Some(bucket) => find_idx(heap, bucket, key).map(|i| bucket[i].1.clone()),
                    None => None,
                }
            }
        }
    };
    // Reconstruct after releasing the store lock (keeps the lock hold minimal).
    Ok(found.map_or(default, |vm| from_message(heap, &vm)))
}

/// `(table-has? t k)` — whether `k` is present.
pub fn has(heap: &mut Heap, id: u64, key: Value) -> Result<bool, LispError> {
    let store = lookup(id)?;
    let data = store.data.lock().expect("table store mutex");
    Ok(match &*data {
        Storage::Dense { vals, .. } => dense_idx(key)
            .and_then(|i| vals.get(i))
            .is_some_and(|dv| *dv != DenseVal::Empty),
        Storage::Hashed(map) => {
            let hash = heap.hash_value(key);
            map.get(&hash)
                .is_some_and(|bucket| find_idx(heap, bucket, key).is_some())
        }
    })
}

/// `(table-delete t k)` — remove `k` if present. Returns the table handle.
pub fn delete(heap: &mut Heap, id: u64, key: Value) -> LispResult {
    let store = lookup(id)?;
    let mut data = store.data.lock().expect("table store mutex");
    match &mut *data {
        Storage::Dense { vals, count } => {
            // An out-of-shape key can't be present in a dense store — no-op.
            if let Some(slot) = dense_idx(key).and_then(|i| vals.get_mut(i)) {
                if *slot != DenseVal::Empty {
                    *slot = DenseVal::Empty;
                    *count -= 1;
                }
            }
        }
        Storage::Hashed(map) => {
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
        }
    }
    Ok(Value::table(id))
}

/// `(table-incr t k [delta])` — **atomically** add `delta` (default 1) to the integer
/// at `k` (treating an absent key as 0) and return the new value. The whole
/// read-modify-write happens under the store lock, so concurrent increments never
/// lose an update — the one safe atomic mutator (no user closure can run under the
/// lock). Errors if the existing value is not a plain integer.
pub fn incr(heap: &mut Heap, id: u64, key: Value, delta: i64) -> LispResult {
    let km = to_message(heap, key)?;
    let store = lookup(id)?;
    let mut data = store.data.lock().expect("table store mutex");
    // Dense fast path: an int slot RMW in place. A non-Int stored value gets the same
    // errors as the hashed path; an out-of-shape key migrates (incr *inserts* on absent,
    // so the key must land somewhere the store can hold it).
    let mut needs_migrate = false;
    if let Storage::Dense { vals, count } = &mut *data {
        match dense_idx(key) {
            Some(i) if i < vals.len() || dense_grow_ok(i + 1, *count) => {
                if i >= vals.len() {
                    vals.resize(i + 1, DenseVal::Empty);
                }
                let cur = match vals[i] {
                    DenseVal::Int(n) => n,
                    DenseVal::Empty => {
                        *count += 1;
                        0
                    }
                    _ => {
                        return Err(LispError::type_err(
                            "table-incr: the value at this key is not an integer",
                        ))
                    }
                };
                let next = cur.checked_add(delta).ok_or_else(|| {
                    LispError::runtime("table-incr: incrementing would exceed the ±2^63 range")
                })?;
                vals[i] = DenseVal::Int(next);
                return Ok(Value::int(next));
            }
            _ => needs_migrate = true,
        }
    }
    if needs_migrate {
        data.migrate_to_hashed(heap);
    }
    let hash = heap.hash_value(key);
    let Storage::Hashed(map) = &mut *data else {
        unreachable!("dense either returned or migrated above");
    };
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

/// `(table-snapshot t)` — a consistent point-in-time copy of the whole table as an
/// immutable Brood map. Atomic (taken under one lock); because the entries are
/// immutable clones, the returned map is unaffected by later mutation — the MVCC win
/// over ETS's dirty reads. O(n) copy.
pub fn snapshot(heap: &mut Heap, id: u64) -> LispResult {
    let store = lookup(id)?;
    // Snapshot the raw clones under the lock; build the Brood map after releasing it.
    let raw: Vec<(Message, Message)> = {
        let data = store.data.lock().expect("table store mutex");
        match &*data {
            Storage::Dense { vals, .. } => vals
                .iter()
                .enumerate()
                .filter_map(|(k, dv)| dv.to_message().map(|vm| (Message::Int(k as i64), vm)))
                .collect(),
            Storage::Hashed(map) => map.values().flat_map(|b| b.iter().cloned()).collect(),
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
