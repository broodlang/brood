//! Reading a primitive's arguments: the positional accessors and the `expect_*` family,
//! each a self-identifying type error naming `who` (the primitive that needed the value)
//! when the argument has the wrong shape. Every domain file reads its arguments through
//! these, built on the `expect!` macro in `builtins.rs`, rather than writing its own match.

use crate::core::heap::Heap;
use crate::core::value::{self, MapId, Value};
use crate::error::LispError;

/// Argument `i`, or nil when the call passed fewer.
pub(crate) fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).copied().unwrap_or(Value::nil())
}

/// Destructure exactly two args. The declared `Arity` is the *primary* arity
/// check (enforced once in `eval::call_native` before any builtin runs); this
/// re-check is defense-in-depth for a direct Rust call that bypasses the gate
/// (e.g. a unit test) — it keeps such a call a clean error instead of a panic.
pub(super) fn two(args: &[Value], who: &str) -> Result<(Value, Value), LispError> {
    if args.len() != 2 {
        return Err(LispError::arity(format!(
            "{}: expected 2 arguments, got {}",
            who,
            args.len()
        )));
    }
    Ok((args[0], args[1]))
}

/// Require a number, coerced to `f64`.
pub(super) fn expect_number(heap: &Heap, who: &str, v: Value) -> Result<f64, LispError> {
    expect!(heap, who, v, "number",
        Value::Int(n) => n as f64,
        Value::Float(f) => f,
    )
}

/// Require an integer in the 64-bit range.
pub(super) fn expect_int(heap: &Heap, who: &str, v: Value) -> Result<i64, LispError> {
    // A bignum IS an int to the language (`int?` says so), so the generic message
    // read "expected int, got int (1180591620717411303424)" — true and useless. Name
    // the actual constraint: this primitive wants a machine-sized one.
    if let Value::BigInt(_) = v {
        return Err(LispError::type_err(format!(
            "{}: expected an int in the 64-bit range, got {}",
            who,
            crate::syntax::printer::print(heap, v),
        )));
    }
    expect!(heap, who, v, "int",
        Value::Int(n) => n,
    )
}

/// Require an integer (`Int` or `BigInt`), coerced to `num_bigint::BigInt`. The bignum
/// analogue of [`expect_int`] — `expect_int` rejects a `BigInt`, but the bitwise /
/// bignum-aware ops accept either, so they route through here instead of losing the
/// value to a bare `type_err`.
pub(super) fn expect_bigint(
    heap: &Heap,
    who: &str,
    v: Value,
) -> Result<num_bigint::BigInt, LispError> {
    heap.as_bigint(v)
        .ok_or_else(|| LispError::wrong_type(heap, who, "int", v))
}

/// Require a string, returned **owned** so the `heap` borrow is released before
/// the builtin reads or allocates further (most callers go on to touch
/// `&mut heap`). Prefer [`expect_string_ref`] when the string is only read.
pub(crate) fn expect_string(heap: &Heap, who: &str, v: Value) -> Result<String, LispError> {
    expect!(heap, who, v, "string",
        Value::Str(id) => heap.string(id).to_string(),
    )
}

/// Require a string, returned **borrowed** — no copy. [`expect_string`] hands back an
/// owned `String`, i.e. it **copies the whole string on every call**. That is invisible
/// on a short argument and quadratic on a long one — an incremental search over one big
/// string re-copied the haystack per probe, which dominated even after the char↔byte
/// conversion was made O(1). Use this for any builtin that only *reads* its string
/// argument; reach for `expect_string` only when an owned value is genuinely needed
/// (because the heap is mutated afterwards, or the value is moved into a fresh
/// allocation).
pub(super) fn expect_string_ref<'h>(
    heap: &'h Heap,
    who: &str,
    v: Value,
) -> Result<&'h str, LispError> {
    expect!(heap, who, v, "string",
        Value::Str(id) => heap.string(id),
    )
}

/// Require a rope, returned **owned** (a cheap `Arc`-node clone) so the `heap`
/// borrow is released before the builtin edits or allocates a fresh rope.
pub(super) fn expect_rope(heap: &Heap, who: &str, v: Value) -> Result<ropey::Rope, LispError> {
    expect!(heap, who, v, "rope",
        Value::Rope(id) => heap.rope(id).clone(),
    )
}

/// Require a rope, returned **borrowed** — no clone. For the read-only query
/// builtins (length/line/slice/…), all of ropey's queries take `&self`, so the
/// heap borrow can be held for the duration. Use this instead of `expect_rope`
/// whenever the rope isn't edited; it skips the per-call `Arc`-node clone on the
/// editor's hot path (viewport render calls `rope-line`/`rope-slice` per line per
/// frame). Reach for `expect_rope` only when allocating a fresh, edited rope.
pub(super) fn expect_rope_ref<'h>(
    heap: &'h Heap,
    who: &str,
    v: Value,
) -> Result<&'h ropey::Rope, LispError> {
    expect!(heap, who, v, "rope",
        Value::Rope(id) => heap.rope(id),
    )
}

/// Require a symbol.
pub(super) fn expect_symbol(heap: &Heap, who: &str, v: Value) -> Result<value::Symbol, LispError> {
    expect!(heap, who, v, "symbol",
        Value::Sym(s) => s,
    )
}

/// Require a keyword.
pub(super) fn expect_keyword(heap: &Heap, who: &str, v: Value) -> Result<value::Symbol, LispError> {
    expect!(heap, who, v, "keyword",
        Value::Keyword(k) => k,
    )
}

/// Require a map or a **record** (ADR-542), answering the field map and, for a record, its
/// identity — the map primitives work on a record's fields, which never include its id, so
/// a record passes through every field operation with nothing to hide. A primitive that
/// returns a modified map hands the id to [`rewrap`] so a record stays the same record kind.
pub(super) fn expect_fields(
    heap: &Heap,
    who: &str,
    v: Value,
) -> Result<(MapId, Option<value::Symbol>), LispError> {
    match v {
        Value::Map(id) => Ok((id, None)),
        Value::Record { id, fields } => Ok((fields, Some(id))),
        other => Err(LispError::wrong_type(heap, who, "map", other)),
    }
}

/// The map a field operation produced, as the kind its input was: a record of `id` when
/// the input was one ([`expect_fields`]), else the map unchanged.
pub(super) fn rewrap(result: Value, id: Option<value::Symbol>) -> Value {
    match (result, id) {
        (Value::Map(fields), Some(id)) => Value::record(id, fields),
        (other, _) => other,
    }
}

/// Require a set.
pub(super) fn expect_set(heap: &Heap, who: &str, v: Value) -> Result<MapId, LispError> {
    expect!(heap, who, v, "set",
        Value::Set(id) => id,
    )
}
