//! The shared mutable table — Brood's ETS (ADR-107), the ONE identity-mutable structure
//! the language has. Values are deep-copied in and out so no two processes alias stored
//! data; the store itself is `crate::core::table`.

use crate::core::heap::Heap;
use crate::core::keywords as kw;
use crate::core::value::{EnvId, Value};
use crate::error::{LispError, LispResult};

use super::numeric::{arg, expect_int};

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::Sig;
    // In-memory shared table — Brood's ETS (ADR-107). A `Value::Table` handle into a
    // global registry of stores holding deep clones (Message form); sendable across
    // processes (every copy shares one store) but local to this runtime.
    primitives.def(
        kw::TABLE_NEW,
        Arity::exact(0),
        Sig::nullary(table_ty),
        &[],
        "Create a new empty in-memory table (Brood's ETS): a shared, mutable key→value store behind an opaque handle. Unlike a map it is mutated in place (`table/put`/`table/delete`) and shared by identity — the handle can be sent to other processes, which all see the same store. Stores deep clones (keys/values are copied in and out), so no two processes alias a stored value. Local to this runtime; not node-portable. Returns the handle.",
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
        "Remove table t from the registry, freeing its store. Idempotent; returns true if it existed. Other handles to t then error on use.",
        table_drop);
}

// ---------- in-memory shared table (Brood's ETS, ADR-107) ----------
// A `Value::Table(id)` handle; the store lives in `crate::core::table`. These builtins are
// thin wrappers — all the storage / locking / clone-in-clone-out lives there.

pub(super) fn expect_table(heap: &Heap, who: &str, v: Value) -> Result<u64, LispError> {
    expect!(heap, who, v, "table",
        Value::Table(id) => id,
    )
}

pub(super) fn table_new(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::table(crate::core::table::create()))
}

pub(super) fn table_put(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_table(heap, "table-put", arg(args, 0))?;
    crate::core::table::check_key("table-put", arg(args, 1))?;
    crate::core::table::put(heap, id, arg(args, 1), arg(args, 2))
}

pub(super) fn table_get(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_table(heap, "table-get", arg(args, 0))?;
    crate::core::table::check_key("table-get", arg(args, 1))?;
    crate::core::table::get(heap, id, arg(args, 1), arg(args, 2))
}

pub(super) fn table_has(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_table(heap, "table-has?", arg(args, 0))?;
    crate::core::table::check_key("table-has?", arg(args, 1))?;
    Ok(Value::boolean(crate::core::table::has(
        heap,
        id,
        arg(args, 1),
    )?))
}

pub(super) fn table_delete(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_table(heap, "table-delete", arg(args, 0))?;
    crate::core::table::check_key("table-delete", arg(args, 1))?;
    crate::core::table::delete(heap, id, arg(args, 1))
}

pub(super) fn table_incr(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_table(heap, "table-incr", arg(args, 0))?;
    crate::core::table::check_key("table-incr", arg(args, 1))?;
    let delta = match arg(args, 2) {
        Value::Nil => 1, // (table-incr t k) defaults the delta to 1
        v => expect_int(heap, "table-incr", v)?,
    };
    crate::core::table::incr(heap, id, arg(args, 1), delta)
}

pub(super) fn table_count(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_table(heap, "table-count", arg(args, 0))?;
    Ok(Value::int(crate::core::table::count(id)?))
}

pub(super) fn table_snapshot(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_table(heap, "table-snapshot", arg(args, 0))?;
    crate::core::table::snapshot(heap, id)
}

pub(super) fn table_drop(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_table(heap, "table-drop", arg(args, 0))?;
    Ok(Value::boolean(crate::core::table::drop_table(id)))
}
