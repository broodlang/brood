//! The shared mutable table (ADR-107), the ONE identity-mutable structure
//! the language has. Values are deep-copied in and out so no two processes alias stored
//! data; the store itself is `crate::core::table`.

use crate::core::heap::Heap;
use crate::core::keywords as kw;
use crate::core::value::{EnvId, Value};
use crate::error::{LispError, LispResult};

use super::args::{arg, expect_int};

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::Sig;
    // In-memory shared mutable table (ADR-107). A `Value::Table` handle into a
    // global registry of stores holding deep clones (Message form); sendable across
    // processes (every copy shares one store) but local to this runtime.
    primitives.def(
        kw::TABLE_NEW,
        Arity::exact(0),
        Sig::nullary(table_ty),
        &[],
        "Create a new empty in-memory table: a shared, mutable key→value store behind an opaque handle. Unlike a map it is mutated in place (`table/put`/`table/delete`) and shared by identity — the handle can be sent to other processes, which all see the same store. Stores deep clones (keys/values are copied in and out), so no two processes alias a stored value. Local to this runtime; not node-portable. The table dies with the process that created it, unless a global holds it (`def`). Returns the handle.",
        table_new);
    primitives.def(
        kw::TABLE_PUT,
        Arity::exact(3),
        Sig::new(vec![table_ty, any, any], table_ty),
        &["t", "k", "v"],
        "Store v under key k in table t, overwriting any existing entry. Keys use the same structural equality as map keys. Returns t (for threading). Both k and v are deep-copied into the store.",
        table_put);
    primitives.def(
        kw::TABLE_GET,
        Arity::range(2, 3),
        Sig::new(vec![table_ty, any], any),
        &["t", "k", "default"],
        "A fresh copy of the value stored under k in table t, or default (nil if omitted) when absent.",
        table_get);
    primitives.def(
        kw::TABLE_HAS,
        Arity::exact(2),
        Sig::new(vec![table_ty, any], bool_ty),
        &["t", "k"],
        "True if table t has an entry for key k.",
        table_has,
    );
    primitives.def(
        kw::TABLE_DELETE,
        Arity::exact(2),
        Sig::new(vec![table_ty, any], table_ty),
        &["t", "k"],
        "Remove key k from table t if present. Returns t.",
        table_delete,
    );
    primitives.def(
        kw::TABLE_INCR,
        Arity::range(2, 3),
        Sig::new(vec![table_ty, any], int),
        &["t", "k", "delta"],
        "Atomically add delta (default 1) to the integer at key k in table t, treating an absent key as 0, and return the new value. The read-modify-write is atomic under the table lock, so concurrent increments never lose an update — use this for counters. Errors if the existing value is not an integer.",
        table_incr);
    primitives.def(
        kw::TABLE_ADD,
        Arity::exact(3),
        Sig::new(vec![table_ty, any, any], num),
        &["t", "k", "v"],
        "Store (+ (get t k 0) v) at key k in table t and return the new value: the read-modify-write the linear-map rewrite fuses `(assoc m k (+ (get m k 0) v))` into. Adds exactly as `+` does (floats, ratios, decimals, bignums, and its errors); the integer case is table-incr's lock-free path. The generic case is a read then a write, not atomic — this serves a table only its loop holds; the concurrent counter is table-incr.",
        table_add,
    );
    primitives.def(
        kw::TABLE_SUB,
        Arity::exact(3),
        Sig::new(vec![table_ty, any, any], num),
        &["t", "k", "v"],
        "Store (- (get t k 0) v) at key k in table t and return the new value: table-add's subtracting sibling, which the linear-map rewrite fuses `(assoc m k (- (get m k 0) v))` and the `dec` spelling into. Subtracts exactly as `-` does; not atomic in the general case.",
        table_sub,
    );
    primitives.def(
        kw::TABLE_COUNT,
        Arity::exact(1),
        Sig::new(vec![table_ty], int),
        &["t"],
        "The number of entries in table t.",
        table_count,
    );
    primitives.def(
        kw::TABLE_SNAPSHOT,
        Arity::exact(1),
        Sig::new(vec![table_ty], map_ty),
        &["t"],
        "A consistent point-in-time copy of the whole table t as an immutable map. Atomic; the returned map is unaffected by later mutation of t. Use map ops (keys/vals/get/reduce) on it. O(n).",
        table_snapshot);
    primitives.def(
        kw::TABLE_DROP,
        Arity::exact(1),
        Sig::new(vec![table_ty], bool_ty),
        &["t"],
        "Remove table t from the registry, freeing its store. Idempotent; returns true if it existed. Other handles to t then error on use. A table is also dropped when the process that created it exits, unless it was stored in a global.",
        table_drop);
    primitives.def(
        kw::TABLE_FREE,
        Arity::exact(2),
        Sig::new(vec![table_ty, any], any),
        &["t", "v"],
        "Internal: drop the private table t the linear-map rewrite built, then return v. Not for user code — use table/drop.",
        table_free);
}

// ---------- in-memory shared mutable table (ADR-107) ----------
// A `Value::Table(id)` handle; the store lives in `crate::core::table`. These builtins are
// thin wrappers — all the storage / locking / clone-in-clone-out lives there.

pub(super) fn expect_table(heap: &Heap, who: &str, v: Value) -> Result<u64, LispError> {
    expect!(heap, who, v, "table",
        Value::Table(id) => id,
    )
}

/// The `(table key …)` prefix every keyed primitive opens with: the table handle in
/// argument 0, checked, and the key in argument 1 vetted by `check_key`.
fn keyed_table(heap: &Heap, who: &str, args: &[Value]) -> Result<u64, LispError> {
    let id = expect_table(heap, who, arg(args, 0))?;
    crate::core::table::check_key(who, arg(args, 1))?;
    Ok(id)
}

/// The calling process owns the new table and its exit drops it (`core::table`,
/// "Lifetime"); outside any process the table has no owner.
pub(super) fn table_new(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let owner = crate::process::current_pid().unwrap_or(0);
    let id = crate::core::table::create_owned(owner);
    if owner != 0 {
        heap.note_owned_table(owner, id);
    }
    Ok(Value::table(id))
}

pub(super) fn table_put(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = keyed_table(heap, "table/put", args)?;
    crate::core::table::put(heap, id, arg(args, 1), arg(args, 2))
}

pub(super) fn table_get(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = keyed_table(heap, "table/get", args)?;
    crate::core::table::get(heap, id, arg(args, 1), arg(args, 2))
}

pub(super) fn table_has(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = keyed_table(heap, "table/contains?", args)?;
    Ok(Value::boolean(crate::core::table::has(
        heap,
        id,
        arg(args, 1),
    )?))
}

pub(super) fn table_delete(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = keyed_table(heap, "table/delete", args)?;
    crate::core::table::delete(heap, id, arg(args, 1))
}

pub(super) fn table_incr(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = keyed_table(heap, "table/incr", args)?;
    let delta = match arg(args, 2) {
        Value::Nil => 1, // (table-incr t k) defaults the delta to 1
        v => expect_int(heap, "table/incr", v)?,
    };
    crate::core::table::incr(heap, id, arg(args, 1), delta)
}

pub(super) fn table_add(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = keyed_table(heap, "%table-add", args)?;
    crate::core::table::add(heap, id, arg(args, 1), arg(args, 2))
}

pub(super) fn table_sub(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = keyed_table(heap, "%table-sub", args)?;
    crate::core::table::sub(heap, id, arg(args, 1), arg(args, 2))
}

pub(super) fn table_count(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_table(heap, "count", arg(args, 0))?;
    Ok(Value::int(crate::core::table::count(id)?))
}

pub(super) fn table_snapshot(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_table(heap, "table/snapshot", arg(args, 0))?;
    crate::core::table::snapshot(heap, id)
}

pub(super) fn table_drop(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_table(heap, "table/drop", arg(args, 0))?;
    Ok(Value::boolean(crate::core::table::drop_table(id)))
}

pub(super) fn table_free(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_table(heap, "%table-free", arg(args, 0))?;
    crate::core::table::free_private(id);
    Ok(arg(args, 1))
}
