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
//! Two lock levels, never nested: the registry `Mutex` is taken, the `Arc<Store>`
//! cloned out, the registry lock dropped — *then* the store's own `Mutex`. So no
//! deadlock, and per-table operations only contend with operations on the *same*
//! table.
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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

/// One shared store: `hash → bucket of (key-clone, value-clone)`. A bucket holds the
/// (rare) structural-hash collisions; equality within it is resolved against the
/// caller's heap so it matches Brood's `=` exactly.
struct Store {
    data: Mutex<HashMap<u64, Vec<(Message, Message)>>>,
}

static REGISTRY: LazyLock<Mutex<HashMap<u64, Arc<Store>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn registry() -> std::sync::MutexGuard<'static, HashMap<u64, Arc<Store>>> {
    REGISTRY.lock().expect("table registry mutex")
}

/// Resolve a handle to its store, or a clean error if it was dropped / never existed.
fn lookup(id: u64) -> Result<Arc<Store>, LispError> {
    registry()
        .get(&id)
        .cloned()
        .ok_or_else(|| LispError::runtime(format!("table {}: no such table (dropped?)", id)))
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

/// `(table)` — create a new empty table; returns its handle id.
pub fn create() -> u64 {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    registry().insert(
        id,
        Arc::new(Store {
            data: Mutex::new(HashMap::new()),
        }),
    );
    id
}

/// `(table-drop t)` — remove a table from the registry. Idempotent; returns whether
/// it existed. Other handles to it then error on use.
pub fn drop_table(id: u64) -> bool {
    registry().remove(&id).is_some()
}

/// `(table-count t)` — number of entries.
pub fn count(id: u64) -> Result<i64, LispError> {
    let store = lookup(id)?;
    let data = store.data.lock().expect("table store mutex");
    Ok(data.values().map(|b| b.len()).sum::<usize>() as i64)
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
    // Clone both out of the GC heap first (also rejects non-sendable values cleanly).
    let km = to_message(heap, key)?;
    let vm = to_message(heap, val)?;
    let store = lookup(id)?;
    let hash = heap.hash_value(key);
    let mut data = store.data.lock().expect("table store mutex");
    let bucket = data.entry(hash).or_default();
    match find_idx(heap, bucket, key) {
        Some(i) => bucket[i].1 = vm,
        None => bucket.push((km, vm)),
    }
    Ok(Value::table(id))
}

/// `(table-get t k [default])` — a fresh copy of the value under `k`, or `default`.
pub fn get(heap: &mut Heap, id: u64, key: Value, default: Value) -> LispResult {
    let store = lookup(id)?;
    let hash = heap.hash_value(key);
    let found = {
        let data = store.data.lock().expect("table store mutex");
        match data.get(&hash) {
            Some(bucket) => find_idx(heap, bucket, key).map(|i| bucket[i].1.clone()),
            None => None,
        }
    };
    // Reconstruct after releasing the store lock (keeps the lock hold minimal).
    Ok(found.map_or(default, |vm| from_message(heap, &vm)))
}

/// `(table-has? t k)` — whether `k` is present.
pub fn has(heap: &mut Heap, id: u64, key: Value) -> Result<bool, LispError> {
    let store = lookup(id)?;
    let hash = heap.hash_value(key);
    let data = store.data.lock().expect("table store mutex");
    Ok(data
        .get(&hash)
        .is_some_and(|bucket| find_idx(heap, bucket, key).is_some()))
}

/// `(table-delete t k)` — remove `k` if present. Returns the table handle.
pub fn delete(heap: &mut Heap, id: u64, key: Value) -> LispResult {
    let store = lookup(id)?;
    let hash = heap.hash_value(key);
    let mut data = store.data.lock().expect("table store mutex");
    let now_empty = if let Some(bucket) = data.get_mut(&hash) {
        if let Some(i) = find_idx(heap, bucket, key) {
            bucket.swap_remove(i);
        }
        bucket.is_empty()
    } else {
        false
    };
    if now_empty {
        data.remove(&hash);
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
    let hash = heap.hash_value(key);
    let mut data = store.data.lock().expect("table store mutex");
    let bucket = data.entry(hash).or_default();
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
        data.values().flat_map(|b| b.iter().cloned()).collect()
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
