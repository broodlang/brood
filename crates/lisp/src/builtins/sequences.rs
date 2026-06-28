use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::error::{LispError, LispResult};
use crate::syntax::printer;

use super::numeric::{arg, two, expect_number, expect_string, expect_rope, expect_int};
use super::realize_seqview;
use crate::eval::apply;
macro_rules! expect {
    ($heap:expr, $who:expr, $v:expr, $expected:literal, $($pat:pat => $extract:expr),+ $(,)?) => {
        match $v {
            $($pat => Ok($extract),)+
            __other => Err(LispError::wrong_type($heap, $who, $expected, __other)),
        }
    };
}

// ---------- pair / sequence ----------

pub(super) fn cons(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (a, b) = two(args, "cons")?;
    Ok(heap.alloc_pair(a, b))
}

/// Realise any lazy seq-view among `args`, returning a fresh vec with views
/// replaced by their realised lists (non-view args untouched). For the
/// stringifiers/printers, whose `&Heap` printer can't run a transducer. Fast path:
/// no view ⇒ a plain copy, no eval. Rooting: each `realize_seqview` can collect,
/// so every input and every already-realised result is kept on the root stack.
pub(super) fn realize_seqviews(heap: &mut Heap, env: EnvId, args: &[Value]) -> Result<Vec<Value>, LispError> {
    if !args.iter().any(|a| matches!(a, Value::SeqView(_))) {
        return Ok(args.to_vec());
    }
    heap.root_scope(|heap| {
        let in_roots: Vec<_> = args.iter().map(|&a| heap.root(a)).collect();
        let mut out_roots: Vec<_> = Vec::with_capacity(args.len());
        for r in &in_roots {
            let v = heap.read_root(*r);
            let v = if matches!(v, Value::SeqView(_)) {
                realize_seqview(heap, env, v)?
            } else {
                v
            };
            out_roots.push(heap.root(v));
        }
        Ok(out_roots.iter().map(|r| heap.read_root(*r)).collect())
    })
}

pub(super) fn first(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    match v {
        Value::Pair(p) => Ok(heap.car(p)),
        Value::Vector(id) => Ok(heap.vector(id).first().copied().unwrap_or(Value::nil())),
        // Bytes are a sequence of ints 0–255; the head byte, or nil if empty.
        Value::Bytes(id) => Ok(heap
            .bytes(id)
            .as_bytes()
            .first()
            .map(|&b| Value::int(b as i64))
            .unwrap_or(Value::nil())),
        // A range is non-empty by construction, so its head is `lo`.
        Value::Range(id) => Ok(Value::int(heap.range_parts(id).0)),
        // A lazy seq-view realises (running its transducer) then yields the head
        // of the resulting list. Rare — the prelude routes most consumers through
        // `seq`/`fold`; this serves a direct `(first (map f xs))`.
        Value::SeqView(_) => match realize_seqview(heap, env, v)? {
            Value::Pair(p) => Ok(heap.car(p)),
            _ => Ok(Value::nil()),
        },
        Value::Nil => Ok(Value::nil()),
        _ => Err(LispError::wrong_type(heap, "first", "list or vector", v)),
    }
}

pub(super) fn rest(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    match v {
        Value::Pair(p) => Ok(heap.cdr(p)),
        Value::Vector(id) => {
            let items: Vec<Value> = heap.vector(id).iter().skip(1).copied().collect();
            Ok(heap.list(items))
        }
        // The tail of a range is another range, one step in — no materialisation
        // (`alloc_range` returns `Nil` once it's empty).
        Value::Range(id) => {
            let (lo, hi, step) = heap.range_parts(id);
            Ok(heap.alloc_range(lo + step, hi, step))
        }
        // The tail of a bytes value is a fresh bytes value (all but the first byte).
        Value::Bytes(id) => {
            let tail: Vec<u8> = heap.bytes(id).as_bytes().iter().skip(1).copied().collect();
            Ok(heap.alloc_bytes(crate::core::blob::SharedBlob::new(&tail)))
        }
        // A lazy seq-view realises then yields the tail of the resulting list.
        Value::SeqView(_) => match realize_seqview(heap, env, v)? {
            Value::Pair(p) => Ok(heap.cdr(p)),
            _ => Ok(Value::nil()),
        },
        Value::Nil => Ok(Value::nil()),
        _ => Err(LispError::wrong_type(heap, "rest", "list or vector", v)),
    }
}

pub(super) fn is_nil(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::boolean(matches!(arg(args, 0), Value::Nil)))
}

pub(super) fn is_pair(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::boolean(matches!(
        arg(args, 0),
        Value::Pair(_) | Value::Range(_) | Value::SeqView(_)
    )))
}

pub(super) fn is_empty(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let x = arg(args, 0);
    match x {
        Value::Nil => Ok(Value::boolean(true)),
        Value::Pair(_) | Value::Range(_) => Ok(Value::boolean(false)),
        Value::SeqView(_) => {
            let realized = realize_seqview(heap, env, x)?;
            Ok(Value::boolean(matches!(realized, Value::Nil)))
        }
        Value::Str(id) => Ok(Value::boolean(heap.string(id).is_empty())),
        Value::Vector(id) => Ok(Value::boolean(heap.vector(id).is_empty())),
        Value::Bytes(id) => Ok(Value::boolean(heap.bytes(id).as_bytes().is_empty())),
        Value::Map(id) => Ok(Value::boolean(heap.map_size(id) == 0)),
        _ => Err(LispError::wrong_type(heap, "empty?", "collection", x)),
    }
}

/// `(%range lo hi step)` — construct a lazy integer range. Returns `Nil` for an
/// empty range; errors on a zero step. The arg-parsing arities live in the
/// Brood `range`, which calls this with all three resolved.
pub(super) fn range_make(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let lo = expect_int(heap, "%range", arg(args, 0))?;
    let hi = expect_int(heap, "%range", arg(args, 1))?;
    let step = expect_int(heap, "%range", arg(args, 2))?;
    if step == 0 {
        return Err(LispError::runtime("range: step must be non-zero")
            .with_hint("use a positive or negative step, e.g. (range 0 10 2)"));
    }
    Ok(heap.alloc_range(lo, hi, step))
}

/// `(range? x)` — true iff `x` is a lazy range handle. (Empty ranges are `Nil`,
/// so this is false for them — the empty case takes the ordinary list path.)
pub(super) fn range_pred(args: &[Value], _: EnvId, _heap: &mut Heap) -> LispResult {
    Ok(Value::boolean(matches!(arg(args, 0), Value::Range(_))))
}

/// `(%range-count rng)` — the element count of a range, O(1).
pub(super) fn range_count(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Range(id) => Ok(Value::int(heap.range_len(id))),
        Value::Nil => Ok(Value::int(0)),
        v => Err(LispError::wrong_type(heap, "%range-count", "range", v)),
    }
}

/// `(%range->list rng)` — realise a range to a concrete list (the slow path
/// behind `seq`/`reverse`/`nth` on a range).
pub(super) fn range_to_list(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Range(id) => {
            let items = heap.range_to_vec(id);
            Ok(heap.list(items))
        }
        Value::Nil => Ok(Value::nil()),
        v => Err(LispError::wrong_type(heap, "%range->list", "range", v)),
    }
}

/// `(%seqview source xform)` — construct a lazy seq-view over `source` carrying
/// the transducer `xform`. The prelude `map`/`filter`/`keep`/`remove` build these
/// (composing `xform` when `source` is already a view); `fold`/`seq` fuse or
/// realise them.
pub(super) fn seqview_make(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let source = arg(args, 0);
    let xform = arg(args, 1);
    Ok(heap.alloc_seqview(source, xform))
}

/// `(%seqview-parts sv)` — the view's `[source xform]` as a 2-element vector, for
/// the prelude to fuse `fold` over the source or realise via the transducer.
pub(super) fn seqview_parts(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::SeqView(id) => {
            let (source, xform) = heap.seqview_parts(id);
            Ok(heap.alloc_vector(vec![source, xform]))
        }
        v => Err(LispError::wrong_type(heap, "%seqview-parts", "seq-view", v)),
    }
}

/// `(seqview? x)` — is `x` a lazy seq-view (a `map`/`filter`/… result not yet
/// realised)? The fold-family fast-path predicate, mirroring `range?`.
pub(super) fn seqview_pred(args: &[Value], _: EnvId, _heap: &mut Heap) -> LispResult {
    Ok(Value::boolean(matches!(arg(args, 0), Value::SeqView(_))))
}

/// Realise a lazy seq-view to a concrete list. The realisation runs the view's
/// transducer over its source, which means applying a Brood closure — so it is
/// delegated to the prelude `%seqview-realize` (`(reverse (fold flip-cons nil
/// sv))`, which fuses through `fold`'s seq-view branch). Resolved against the
/// live global env so a user redefinition is honoured. The kernel uses this from
/// the hot `first`/`rest` builtins; every other consumer realises in the prelude
/// (via `seq`) or fuses (via `fold`).

/// `(%range-reduce f acc rng)` — left-fold a range with `f` in a native counted
/// loop, **without materialising** it: the whole point of the reducible range.
/// `acc` and `f` are rooted across the loop because each `apply` is a safepoint
/// that can relocate them.
pub(super) fn range_reduce(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let f = arg(args, 0);
    let init = arg(args, 1);
    let (lo, hi, step) = match arg(args, 2) {
        Value::Range(id) => heap.range_parts(id),
        Value::Nil => return Ok(init), // empty range — acc unchanged
        v => return Err(LispError::wrong_type(heap, "%range-reduce", "range", v)),
    };
    // Route the per-element callback through the VM when it's the active engine.
    let use_vm = crate::eval::compile::vm_enabled();
    // Primitive-reducer fast path: when `f` is `+`/`*` (directly, or via the
    // prelude wrapper's passthrough arm), fold with the inlined scalar op and
    // never call back into `apply` per element.
    let prim = crate::eval::compile::reduce_prim_op(heap, f);

    // Tight i64 loop: when both prim resolves AND the accumulator is a plain i64,
    // operate on raw integers with no Value boxing per iteration. This avoids the
    // 24-byte-by-pointer passing overhead of `prim_apply_step` and the root
    // machinery (integers are inline — no GC slot needed). On overflow (rare),
    // fall through to the general path starting from the current position.
    if let (Some(op), Some(mut int_acc)) = (prim, init.as_int()) {
        let mut i = lo;
        while if step > 0 { i < hi } else { i > hi } {
            match crate::eval::compile::prim_apply_int_step(op, int_acc, i) {
                Some(v) => int_acc = v,
                None => {
                    // Overflow or unsupported op — hand off the remainder to the
                    // slow path starting from the current (i, acc) state.
                    return range_reduce_slow(
                        f,
                        Value::int(int_acc),
                        i,
                        hi,
                        step,
                        use_vm,
                        env,
                        heap,
                    );
                }
            }
            i += step;
        }
        return Ok(Value::int(int_acc));
    }

    range_reduce_slow(f, init, lo, hi, step, use_vm, env, heap)
}

pub(super) fn range_reduce_slow(
    f: Value,
    init: Value,
    lo: i64,
    hi: i64,
    step: i64,
    use_vm: bool,
    env: EnvId,
    heap: &mut Heap,
) -> LispResult {
    let prim = crate::eval::compile::reduce_prim_op(heap, f);
    heap.root_scope(|heap| {
        let f_r = heap.root(f);
        let mut acc_r = heap.root(init);
        let mut i = lo;
        while if step > 0 { i < hi } else { i > hi } {
            let f = heap.read_root(f_r);
            let acc = heap.read_root(acc_r);
            let next = match prim {
                Some(op) => match crate::eval::compile::prim_apply_step(op, acc, Value::int(i))? {
                    Some(v) => v,
                    None if use_vm => {
                        crate::eval::compile::apply_value(heap, f, &[acc, Value::int(i)], env)?
                    }
                    None => apply(heap, f, &[acc, Value::int(i)], env)?,
                },
                None if use_vm => {
                    crate::eval::compile::apply_value(heap, f, &[acc, Value::int(i)], env)?
                }
                None => apply(heap, f, &[acc, Value::int(i)], env)?,
            };
            acc_r = heap.advance_root(acc_r, next);
            i += step;
        }
        Ok(heap.read_root(acc_r))
    })
}

/// `(%sort-asc coll)` — stable ascending sort of a numeric collection by `<`.
/// The fast path behind `(sort coll)` when no custom comparator is given;
/// the all-Brood `merge-sort` in `std/prelude.blsp` still handles
/// `(sort less? coll)`. ~50× faster than the in-Brood mergesort on 10 000
/// items because every comparison is a Rust `match` instead of an
/// `eval::apply` round-trip.
///
/// Items must be `Int` / `Float` / mixed (the same shape `<` accepts).
/// Mixed Int+Float promote to float for the compare (matching `prim_lt`).
/// Any non-numeric item is a `wrong_type` error against the offending value.
pub(super) fn sort_asc(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    // Collect into a Vec. `seq_items` walks the cons spine (or copies a
    // vector) once. Values are `Copy` so the Vec holds plain handles — no
    // GC root machinery needed because `sort_by` does no eval and can't
    // trigger a safepoint.
    let mut items = heap.seq_items(arg(args, 0))?;

    // Validate before sorting so a non-numeric item produces one clear
    // error rather than an indeterminate-order partial sort.
    for &v in &items {
        match v {
            Value::Int(_) | Value::Float(_) => {}
            _ => return Err(LispError::wrong_type(heap, "sort", "number", v)),
        }
    }

    // Stable sort. The int-int branch keeps full precision; mixed pairs
    // promote to f64 (same compromise as `prim_lt`'s mixed case — past
    // 2^53 the float compare can collapse two distinct ints, but that
    // matches what `<` itself would do).
    items.sort_by(|a, b| match (*a, *b) {
        (Value::Int(x), Value::Int(y)) => x.cmp(&y),
        _ => {
            let xf = match *a {
                Value::Int(n) => n as f64,
                Value::Float(f) => f,
                _ => unreachable!(),
            };
            let yf = match *b {
                Value::Int(n) => n as f64,
                Value::Float(f) => f,
                _ => unreachable!(),
            };
            // NaN sorts as Equal (would otherwise break `sort_by`'s total
            // ordering). Real Brood `<` doesn't admit NaN past `(nan? x)`
            // anyway, so this is the lesser evil.
            xf.partial_cmp(&yf).unwrap_or(std::cmp::Ordering::Equal)
        }
    });

    Ok(heap.list(items))
}

/// `(%sort-cmp coll)` — stable ascending sort by the structural total order
/// (`Heap::value_cmp`). The Brood `sort` (prelude) routes here when items
/// aren't all numeric, so `(sort [[1 0] [2 1]])` and similar work without a
/// custom comparator. Cross-kind items get a defined tag-rank order rather
/// than the old "expected number" trap.
pub(super) fn sort_cmp(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let mut items = heap.seq_items(arg(args, 0))?;
    // `value_cmp` reads heap data through `&Heap` only; the items are `Copy`
    // handles, so no GC root machinery is needed.
    items.sort_by(|a, b| heap.value_cmp(*a, *b));
    Ok(heap.list(items))
}

/// `(compare a b)` — the structural total order as a binary comparison: `-1` if
/// `a` sorts before `b`, `0` if equal, `1` if after. Numbers compare
/// numerically; strings/keywords/symbols by text; vectors/lists
/// lexicographically; cross-kind values by a stable tag rank. The binary form of
/// the order `sort` uses, so `sort-by` and custom comparators work over any
/// orderable value, not just numbers.
pub(super) fn compare(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use std::cmp::Ordering;
    let ord = match heap.value_cmp(arg(args, 0), arg(args, 1)) {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    };
    Ok(Value::int(ord))
}

// ---------- vector ----------

pub(super) fn vector(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Ok(heap.alloc_vector(args.to_vec()))
}

pub(super) fn vector_ref(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    let n = expect_int(heap, "vector-ref", arg(args, 1))?;
    match v {
        Value::Vector(id) if n >= 0 && (n as usize) < heap.vector(id).len() => {
            Ok(heap.vector(id)[n as usize])
        }
        Value::Vector(id) => Err(LispError::runtime(format!(
            "vector-ref: index {} out of range [0, {})",
            n,
            heap.vector(id).len()
        ))
        .with_code(crate::error::error_codes::INDEX_OUT_OF_RANGE)),
        _ => Err(LispError::wrong_type(heap, "vector-ref", "vector", v)),
    }
}

pub(super) fn vector_length(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    match v {
        Value::Vector(id) => Ok(Value::int(heap.vector(id).len() as i64)),
        _ => Err(LispError::wrong_type(heap, "vector-length", "vector", v)),
    }
}

/// `(vector-assoc v i x)` — a fresh vector like `v` with index `i` set to `x`.
/// The vector counterpart of `map-assoc`; O(n) copy (vectors are flat), one
/// allocation, no cons churn. `i` must be in `[0, len)` (append-at-end is a
/// deferred power feature, ADR-011). No GC safepoint runs inside a builtin, so
/// the cloned handles stay valid across `alloc_vector`.
pub(super) fn vector_assoc(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    let i = expect_int(heap, "vector-assoc", arg(args, 1))?;
    let x = arg(args, 2);
    match v {
        Value::Vector(id) if i >= 0 && (i as usize) < heap.vector(id).len() => {
            let mut items = heap.vector(id).to_vec();
            items[i as usize] = x;
            Ok(heap.alloc_vector(items))
        }
        Value::Vector(id) => Err(LispError::runtime(format!(
            "vector-assoc: index {} out of range [0, {})",
            i,
            heap.vector(id).len()
        ))
        .with_code(crate::error::error_codes::INDEX_OUT_OF_RANGE)),
        _ => Err(LispError::wrong_type(heap, "vector-assoc", "vector", v)),
    }
}

/// `(subvec v start)` / `(subvec v start end)` — a fresh vector of the elements
/// of `v` in `[start, end)` (`end` defaults to the length). `0 <= start <= end
/// <= len`; out of range is an error. The slice counterpart of `substring`, and
/// the vector-preserving slice the list-returning `take`/`drop` don't give.
pub(super) fn subvec(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    let id = match v {
        Value::Vector(id) => id,
        _ => return Err(LispError::wrong_type(heap, "subvec", "vector", v)),
    };
    let len = heap.vector(id).len() as i64;
    let start = expect_int(heap, "subvec", arg(args, 1))?;
    let end = if args.len() > 2 {
        expect_int(heap, "subvec", arg(args, 2))?
    } else {
        len
    };
    if start < 0 || end > len || start > end {
        return Err(LispError::runtime(format!(
            "subvec: range [{start}, {end}) out of bounds for vector of length {len}"
        ))
        .with_code(crate::error::error_codes::INDEX_OUT_OF_RANGE));
    }
    let items = heap.vector(id)[start as usize..end as usize].to_vec();
    Ok(heap.alloc_vector(items))
}

// ---------- map ----------

/// Require a map; otherwise a self-identifying type error attributed to `who`.
pub(super) fn expect_map(heap: &Heap, who: &str, v: Value) -> Result<value::MapId, LispError> {
    expect!(heap, who, v, "map",
        Value::Map(id) => id,
    )
}

/// `(hash-map k v k v …)` — build a map from alternating key/value args (the
/// programmatic form of the `{ }` literal). Errors on an odd count; last-wins on
/// duplicate keys.
pub(super) fn hash_map(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    if !args.len().is_multiple_of(2) {
        return Err(LispError::arity(
            "hash-map: expected an even number of arguments (key/value pairs)",
        ));
    }
    let pairs: Vec<(Value, Value)> = args.chunks_exact(2).map(|kv| (kv[0], kv[1])).collect();
    Ok(heap.map_from_pairs(pairs))
}

/// The `[k v]` of a pair item — a `[k v]` vector or a `(k v)` list — with
/// `first`/`second` semantics (missing slots read as `nil`). Used by
/// [`map_into`] to read the items of an `into`/`zipmap` sequence.
pub(super) fn pair_kv(heap: &Heap, who: &str, p: Value) -> Result<(Value, Value), LispError> {
    match p {
        Value::Vector(id) => {
            let v = heap.vector(id);
            Ok((
                v.first().copied().unwrap_or(Value::nil()),
                v.get(1).copied().unwrap_or(Value::nil()),
            ))
        }
        Value::Pair(id) => {
            let (k, rest) = heap.pair(id);
            let val = match rest {
                Value::Pair(rid) => heap.pair(rid).0,
                _ => Value::nil(),
            };
            Ok((k, val))
        }
        _ => Err(LispError::wrong_type(heap, who, "pair or vector", p)),
    }
}

/// `(%map-into m seq)` — pour each `[k v]` item of `seq` into map `m`, returning
/// a fresh map, via the transient builder (`Heap::map_from_pairs_into`, see
/// `docs/transients.md`). The kernel hook behind the prelude's `into` (map
/// branch), `zipmap`, and `select-keys`; equals `(reduce assoc m seq)` but
/// mutates only build-local trie nodes, so it allocates O(result-nodes) rather
/// than O(n·depth).
pub(super) fn map_into(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let into = expect_map(heap, "%map-into", arg(args, 0))?;
    let items = heap.seq_items(arg(args, 1))?;
    let mut pairs = Vec::with_capacity(items.len());
    for it in items {
        pairs.push(pair_kv(heap, "%map-into", it)?);
    }
    Ok(heap.map_from_pairs_into(into, pairs))
}

/// `(map-get m k [default])` — the value `k` maps to, or `default` (nil if
/// omitted) when absent.
pub(super) fn map_get(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_map(heap, "map-get", arg(args, 0))?;
    Ok(heap
        .map_get(id, arg(args, 1))
        .unwrap_or_else(|| arg(args, 2)))
}

/// `(map-assoc m k v)` — a fresh map with `k` bound to `v`.
pub(super) fn map_assoc(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_map(heap, "map-assoc", arg(args, 0))?;
    Ok(heap.map_assoc(id, arg(args, 1), arg(args, 2)))
}

/// `(map-int-add m k delta)` — a fresh map with `k`'s integer value incremented
/// by `delta` (inserts `delta` when `k` is absent). Single trie traversal.
pub(super) fn map_int_add(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_map(heap, "map-int-add", arg(args, 0))?;
    let delta = expect_int(heap, "map-int-add", arg(args, 2))?;
    Ok(heap.map_int_add(id, arg(args, 1), delta))
}

/// `(map-dissoc m k)` — a fresh map with `k` removed.
pub(super) fn map_dissoc(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_map(heap, "map-dissoc", arg(args, 0))?;
    Ok(heap.map_dissoc(id, arg(args, 1)))
}

/// `(map-pairs m)` — the entries as a list of `[k v]` vectors, in insertion
/// order, in one O(n) pass. The *single* map enumerator: `keys`/`vals`/
/// `contains?`/`reduce-kv` are all Brood over it (std/prelude.blsp).
pub(super) fn map_pairs(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_map(heap, "map-pairs", arg(args, 0))?;
    let entries = heap.map_entries(id); // copy out, releasing the borrow before we alloc
    let pairs: Vec<Value> = entries
        .into_iter()
        .map(|(k, v)| heap.alloc_vector(vec![k, v]))
        .collect();
    Ok(heap.list(pairs))
}

/// `(map-count m)` — the number of entries, O(1). The CHAMP root node tracks
/// its subtree size, so this never walks (or allocates) the entries; it's what
/// `count`/`empty?` on a map use instead of materialising `map-pairs`.
pub(super) fn map_count(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_map(heap, "map-count", arg(args, 0))?;
    Ok(Value::int(heap.map_size(id) as i64))
}

pub(super) fn string_length(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    match v {
        Value::Str(id) => Ok(Value::int(heap.string(id).chars().count() as i64)),
        _ => Err(LispError::wrong_type(heap, "string-length", "string", v)),
    }
}

/// `(display-width s)` — how many terminal/grid *cells* `s` occupies, counting
/// grapheme clusters (an emoji / flag / CJK char is 2, a combining mark 0). The
/// width-aware counterpart to `string-length` (which counts codepoints) — the
/// editor's column / cursor math uses it so a wide glyph advances two columns. The
/// GUI renderer advances the cell grid by the same measure (`crate::text_width`).
pub(super) fn display_width(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    match v {
        Value::Str(id) => Ok(Value::int(
            crate::text_width::display_width(heap.string(id)) as i64,
        )),
        _ => Err(LispError::wrong_type(heap, "display-width", "string", v)),
    }
}

// ---------- type reflection ----------

/// `(type-of x)` — the runtime type tag of `x` as a keyword: `:int` `:float`
/// `:string` `:symbol` `:keyword` `:bool` `:nil` `:pair` `:vector` `:fn`
/// `:macro` `:native`. The single irreducible reflective primitive: the tag
/// predicates (`int?`/`string?`/…) are Brood wrappers over it (`std/prelude.blsp`),
/// and the in-language type checks build on it too.
pub(super) fn type_of(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    // Cached keyword id per tag — `type-of` is hit per element by the seq
    // predicates, so re-interning the tag name here dominated intern cost.
    Ok(Value::keyword(value::tag(arg(args, 0)).keyword()))
}

// ---------- value <-> text and I/O ----------

pub(super) fn str_concat(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let args = realize_seqviews(heap, env, args)?;
    let mut s = String::new();
    for &a in &args {
        s.push_str(&printer::display(heap, a));
    }
    Ok(heap.alloc_string(&s))
}

/// `(%string-join sep coll)` — the native fast path behind `join` for a string
/// separator. Walks `coll` once, appending each element's display form (the same
/// `str`/`join` use) with `sep` between adjacent elements into one pre-sized
/// buffer — no intermediate cons list and no `reverse` pass, which is what the
/// all-Brood `join` paid (≈2N cons cells built then reversed). `coll` is realised
/// via `seq_items` (list / vector / range; empty → `""`). Semantics match the
/// prelude `join`: display form per element, separator only between adjacent
/// elements, so a single-element collection has no trailing separator.
pub(super) fn string_join(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let sep = match arg(args, 0) {
        s @ Value::Str(_) => printer::display(heap, s),
        v => return Err(LispError::wrong_type(heap, "%string-join", "string", v)),
    };
    // Streaming fast path for a lazy int range (`(join "," (range n))`): format
    // each integer straight into the buffer in one pass — no intermediate Vec of
    // `Value`s, no per-element string allocation. The range stays immutable; this
    // only changes how its joined string is *constructed*.
    if let Value::Range(id) = arg(args, 1) {
        use std::fmt::Write;
        let (lo, hi, step) = heap.range_parts(id);
        let mut s = String::new();
        let mut first = true;
        let mut i = lo;
        while if step > 0 { i < hi } else { i > hi } {
            if !first {
                s.push_str(&sep);
            }
            first = false;
            let _ = write!(s, "{i}");
            i += step;
        }
        return Ok(heap.alloc_string(&s));
    }
    let items = heap.seq_items(arg(args, 1))?;
    // Rough pre-size (separators + a small per-element allowance) to avoid most
    // re-grows without a second display pass just to compute the exact length.
    let mut s = String::with_capacity(sep.len() * items.len().saturating_sub(1) + items.len() * 8);
    for (i, &item) in items.iter().enumerate() {
        if i > 0 {
            s.push_str(&sep);
        }
        s.push_str(&printer::display(heap, item));
    }
    Ok(heap.alloc_string(&s))
}

pub(super) fn pr_str(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let v = match arg(args, 0) {
        sv @ Value::SeqView(_) => realize_seqview(heap, env, sv)?,
        other => other,
    };
    let s = printer::print(heap, v);
    Ok(heap.alloc_string(&s))
}

pub(super) fn name_of(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    match v {
        Value::Sym(s) | Value::Keyword(s) => Ok(heap.alloc_string(&value::symbol_name(s))),
        Value::Str(_) => Ok(v),
        _ => Err(LispError::wrong_type(
            heap,
            "name",
            "symbol, keyword, or string",
            v,
        )),
    }
}

/// `(symbol x)` — the symbol whose spelling is `x`. Accepts a string (intern as
/// a fresh-or-existing symbol), a symbol (identity), or a keyword (same spelling,
/// retagged as a symbol). The lenient inverse of `name`; pairs with `keyword`.
pub(super) fn to_symbol(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    match v {
        Value::Sym(_) => Ok(v),
        Value::Keyword(s) => Ok(Value::symbol(s)),
        Value::Str(id) => {
            let name = heap.string(id).to_string();
            Ok(Value::symbol(value::intern(&name)))
        }
        _ => Err(LispError::wrong_type(
            heap,
            "symbol",
            "string, symbol, or keyword",
            v,
        )),
    }
}

/// `(keyword x)` — the keyword whose spelling is `x`. Accepts a string (intern),
/// a keyword (identity), or a symbol (same spelling, retagged as a keyword).
/// Mirrors `symbol`; the two share an interner so a keyword and a symbol with the
/// same spelling carry equal `Symbol` ids (the tag is the only distinction).
pub(super) fn to_keyword(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    match v {
        Value::Keyword(_) => Ok(v),
        Value::Sym(s) => Ok(Value::keyword(s)),
        Value::Str(id) => {
            let name = heap.string(id).to_string();
            Ok(Value::keyword(value::intern(&name)))
        }
        _ => Err(LispError::wrong_type(
            heap,
            "keyword",
            "string, symbol, or keyword",
            v,
        )),
    }
}

/// `(substring s start [end])` — the characters of `s` in `[start, end)`,
/// char-indexed (consistent with `string-length`). `end` defaults to the
/// string's length, so `(substring s start)` is "from `start` to the end".
/// Errors if out of range.

pub(super) fn substring(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "substring", arg(args, 0))?;
    let start = expect_int(heap, "substring", arg(args, 1))?;
    let len = s.chars().count() as i64;
    let end = match args.get(2) {
        Some(_) => expect_int(heap, "substring", arg(args, 2))?,
        None => len,
    };
    if start < 0 || end < start || end > len {
        return Err(LispError::runtime(format!(
            "substring: range [{}, {}) out of bounds for length {}",
            start, end, len
        ))
        .with_code(crate::error::error_codes::INDEX_OUT_OF_RANGE));
    }
    let sub: String = s
        .chars()
        .skip(start as usize)
        .take((end - start) as usize)
        .collect();
    Ok(heap.alloc_string(&sub))
}

/// Shared body of `string-span` / `string-span-until`: from char `start`, count the
/// maximal run of chars whose membership in the set `chars` equals `in_set`, and
/// return the char index just past it. Char-indexed, like `substring`/`char-at`. The
/// forward char-class scan a tokenizer runs its inner loops on (skip a whitespace /
/// digit / delimiter run) — O(run) native instead of O(run) interpreted recursion.
pub(super) fn string_span_impl(args: &[Value], heap: &mut Heap, who: &str, in_set: bool) -> LispResult {
    let s = expect_string(heap, who, arg(args, 0))?;
    let start = expect_int(heap, who, arg(args, 1))?;
    let set = expect_string(heap, who, arg(args, 2))?;
    let len = s.chars().count() as i64;
    if start < 0 || start > len {
        return Err(LispError::runtime(format!(
            "{}: start {} out of bounds for length {}",
            who, start, len
        ))
        .with_code(crate::error::error_codes::INDEX_OUT_OF_RANGE));
    }
    let mut idx = start as usize;
    for c in s.chars().skip(start as usize) {
        if set.contains(c) == in_set {
            idx += 1;
        } else {
            break;
        }
    }
    Ok(Value::int(idx as i64))
}

/// `(string-span s start chars)` — the char index just past the maximal run of chars
/// drawn from the set `chars`, beginning at `start` (so `start` itself when the char
/// there isn't in the set). For skipping a run *of* a class — whitespace, digits.
pub(super) fn string_span(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    string_span_impl(args, heap, "string-span", true)
}

/// `(string-span-until s start chars)` — the char index of the first char in the set
/// `chars` at or after `start` (or the length if none): the maximal run of chars
/// *not* in the set. For scanning up to a delimiter — comment-to-newline,
/// atom-to-delimiter, string-body-to-quote.
pub(super) fn string_span_until(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    string_span_impl(args, heap, "string-span-until", false)
}

/// Lexical category of an atom token (a maximal run of non-delimiter chars), matching
/// `std/editor/highlight`'s `hl--atom-face` shape: a `:`-prefixed or `nil`/`true`/`false`
/// constant is a `keyword`; one that parses as an int/float (like `string->number`) is a
/// `number`; anything else is a plain `symbol`. The head-position special-form vs call
/// distinction is left to the consumer (it needs the surrounding `(`).

pub(super) fn scan_atom_kind(t: &str) -> &'static str {
    if t.starts_with(':') || t == "nil" || t == "true" || t == "false" {
        "keyword"
    } else if t.parse::<i64>().is_ok() || t.parse::<f64>().is_ok() {
        "number"
    } else {
        "symbol"
    }
}

/// `(scan-tokens s)` — lexically tokenize Brood source `s` into a vector of
/// `[start end kind text]` tokens (char offsets, end-exclusive; whitespace and commas
/// skipped between tokens). `kind` is `:comment`, `:string`, `:number`, `:keyword`,
/// `:symbol`, `:open`, or `:close`. The lossless token stream a fontifier / structural
/// tool walks — the per-character scanning (a render hot path in interpreted Brood) runs
/// here in Rust, leaving the consumer to apply policy (faces, head-position) over
/// O(tokens), not O(chars). Strings honour `\\` escapes; a comment runs to end-of-line.
pub(super) fn scan_tokens(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "scan-tokens", arg(args, 0))?;
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let kw = |k: &'static str| Value::keyword(value::intern(k));
    let is_ws = |c: char| matches!(c, ' ' | '\t' | '\n' | '\r' | ',');
    let is_delim = |c: char| is_ws(c) || matches!(c, '(' | ')' | '[' | ']' | '{' | '}' | '"' | ';');
    let mut out: Vec<Value> = Vec::new();
    let mut i = 0usize;
    while i < n {
        if is_ws(chars[i]) {
            i += 1;
            continue;
        }
        let start = i;
        let (end, kind): (usize, &'static str) = match chars[i] {
            ';' => {
                let mut j = i + 1;
                while j < n && chars[j] != '\n' {
                    j += 1;
                }
                (j, "comment")
            }
            '"' => {
                let mut j = i + 1;
                loop {
                    if j >= n {
                        break;
                    }
                    match chars[j] {
                        '\\' => j += 2, // escape: skip the backslash and the next char
                        '"' => {
                            j += 1;
                            break;
                        }
                        _ => j += 1,
                    }
                }
                (j.min(n), "string")
            }
            '(' | '[' | '{' => (start + 1, "open"),
            ')' | ']' | '}' => (start + 1, "close"),
            _ => {
                let mut j = i;
                while j < n && !is_delim(chars[j]) {
                    j += 1;
                }
                let text: String = chars[start..j].iter().collect();
                (j, scan_atom_kind(&text))
            }
        };
        let text: String = chars[start..end].iter().collect();
        let tv = heap.alloc_string(&text);
        let tok = heap.alloc_vector(vec![
            Value::int(start as i64),
            Value::int(end as i64),
            kw(kind),
            tv,
        ]);
        out.push(tok);
        i = end;
    }
    Ok(heap.alloc_vector(out))
}

/// Append the run `[lo, hi)` (absolute offsets; `base` is the text's first char) in
/// `face` to `runs`, coalescing into the previous run when the faces are `equal` — the
/// runs partition the line contiguously, so coalescing just extends the last run's end.

pub(super) fn span_runs_push(
    runs: &mut Vec<(usize, usize, Value)>,
    base: i64,
    lo: i64,
    hi: i64,
    face: Value,
    heap: &Heap,
) {
    if hi <= lo {
        return;
    }
    // `lo`/`hi` are absolute offsets >= `base` by construction; `saturating_sub`
    // keeps the relative index non-negative even if a caller ever violated that,
    // so the host can't panic on an underflow.
    let lhi = hi.saturating_sub(base) as usize;
    if let Some(last) = runs.last_mut() {
        if heap.equal(last.2, face) {
            last.1 = lhi;
            return;
        }
    }
    runs.push((lo.saturating_sub(base) as usize, lhi, face));
}

/// Merge face `b` over face `a` (`b` wins on key conflict), as Brood's `(into a b)` —
/// the overlay-merge the fontifier does to paint a region/isearch face on top of a
/// syntax face. A nil face is the identity; two maps merge `b`'s entries into `a`.
pub(super) fn merge_faces(heap: &mut Heap, a: Value, b: Value) -> Value {
    match (a, b) {
        (Value::Nil, _) => b,
        (_, Value::Nil) => a,
        (Value::Map(ai), Value::Map(bi)) => {
            let entries = heap.map_entries(bi);
            heap.map_from_pairs_into(ai, entries)
        }
        _ => b,
    }
}

/// Read a `[start end face]` span/range list into `(start, end, face)` tuples (handles
/// at offsets outside the window are kept; the tilers clip them).
pub(super) fn read_spans(heap: &Heap, who: &str, v: Value) -> Result<Vec<(i64, i64, Value)>, LispError> {
    let items = heap.seq_items(v)?;
    let mut out = Vec::with_capacity(items.len());
    for sv in &items {
        let parts = match sv {
            Value::Vector(id) => heap.vector(*id).to_vec(),
            _ => {
                return Err(LispError::runtime(format!(
                    "{}: each span must be a [start end face] vector",
                    who
                )))
            }
        };
        match (parts.first(), parts.get(1), parts.get(2)) {
            (Some(Value::Int(s)), Some(Value::Int(e)), Some(f)) => out.push((*s, *e, *f)),
            _ => {
                return Err(LispError::runtime(format!(
                    "{}: each span must be [int int face]",
                    who
                )))
            }
        }
    }
    Ok(out)
}

/// `(span-runs text base spans [ranges])` — tile `text` (its first char at offset
/// `base`) into a list of `[substring face]` runs. From ascending, non-overlapping
/// `[start end face]` `spans`: each gap is a nil-faced run, each span its text in its
/// face. With an optional overlay `ranges` channel (`[lo hi face]`, may overlap /
/// be unordered), each char's face is its span face with every covering range face
/// merged on top (later ranges win) — the region / isearch / bracket overlays. Adjacent
/// equal-face runs coalesce. This is the fontifier's span→runs tiler (`std/editor/
/// highlight`'s `fontify-runs`) in Rust — it runs per visible line every frame. Faces
/// are opaque maps, merged via `into` semantics and compared with `equal` to coalesce.
pub(super) fn span_runs(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let text = expect_string(heap, "span-runs", arg(args, 0))?;
    let base = expect_int(heap, "span-runs", arg(args, 1))?;
    let spans = read_spans(heap, "span-runs", arg(args, 2))?;
    let ranges = match args.get(3) {
        Some(r) => read_spans(heap, "span-runs", *r)?,
        None => Vec::new(),
    };
    let chars: Vec<char> = text.chars().collect();
    // `base` is caller-controlled (any i64); guard the absolute end against i64
    // overflow so a Lisp program can't panic the host. With a valid `end`, every
    // `lo`/`hi` handed to `span_runs_push` is provably in `[base, end]`.
    let end = base.checked_add(chars.len() as i64).ok_or_else(|| {
        LispError::runtime(format!(
            "span-runs: base {base} plus text length {} overflows i64",
            chars.len()
        ))
        .with_code(crate::error::error_codes::INDEX_OUT_OF_RANGE)
    })?;
    let mut runs: Vec<(usize, usize, Value)> = Vec::new();

    if ranges.is_empty() {
        // fast path: no overlay merge — emit gaps + spans left-to-right.
        let mut cur = base;
        for (s, e, f) in spans {
            if e <= base {
                continue;
            }
            if s >= end {
                break; // ascending spans: the rest are past the window
            }
            let lo = s.max(cur);
            let hi = e.min(end);
            if lo > cur {
                span_runs_push(&mut runs, base, cur, lo, Value::Nil, heap);
            }
            span_runs_push(&mut runs, base, lo, hi, f, heap);
            cur = hi;
        }
        if cur < end {
            span_runs_push(&mut runs, base, cur, end, Value::Nil, heap);
        }
    } else {
        // overlay path: tile by the union of span + range edges, merging faces per
        // segment. O(segments) — segments, not chars — so a region over the viewport is
        // as cheap as plain syntax, not a per-character merge.
        let mut bounds: Vec<i64> = vec![base, end];
        for (s, e, _) in spans.iter().chain(ranges.iter()) {
            if *e > base && *s < end {
                bounds.push((*s).max(base));
                bounds.push((*e).min(end));
            }
        }
        bounds.sort_unstable();
        bounds.dedup();
        let mut si = 0usize; // monotonic span cursor (spans are ascending)
        for w in bounds.windows(2) {
            let (a, b) = (w[0], w[1]);
            if b <= a {
                continue;
            }
            while si < spans.len() && spans[si].1 <= a {
                si += 1;
            }
            let span_face = if si < spans.len() && spans[si].0 <= a && a < spans[si].1 {
                spans[si].2
            } else {
                Value::nil()
            };
            let mut rf = Value::nil();
            for (lo, hi, f) in &ranges {
                if *lo <= a && a < *hi {
                    rf = merge_faces(heap, rf, *f);
                }
            }
            let face = merge_faces(heap, span_face, rf);
            span_runs_push(&mut runs, base, a, b, face, heap);
        }
    }

    let n = chars.len();
    let out: Vec<Value> = runs
        .iter()
        .map(|&(lo, hi, f)| {
            // Clamp defensively: the run bounds are in-range by construction, but a
            // slice past `chars.len()` would panic the host — never let it.
            let seg: String = chars[lo.min(n)..hi.min(n)].iter().collect();
            let sv = heap.alloc_string(&seg);
            heap.alloc_vector(vec![sv, f])
        })
        .collect();
    Ok(heap.list_from_slice(&out))
}

/// OS clipboard access (the `clipboard` feature, via `arboard`). The handle lives in a
/// `OnceLock` for the whole process: on X11/Wayland the selection *owner* must stay
/// alive to answer paste requests, so a fresh handle per call would lose the copied text
/// the moment it dropped. Init failure (no display server) is cached as `None`, so the
/// builtins degrade to no-ops rather than retrying.
#[cfg(feature = "clipboard")]
mod clipboard {
    use arboard::Clipboard;
    use std::sync::{Mutex, OnceLock};
    static CB: OnceLock<Option<Mutex<Clipboard>>> = OnceLock::new();
    fn handle() -> Option<&'static Mutex<Clipboard>> {
        CB.get_or_init(|| Clipboard::new().ok().map(Mutex::new))
            .as_ref()
    }
    pub fn get_text() -> Option<String> {
        handle()?.lock().ok()?.get_text().ok()
    }
    pub fn set_text(s: &str) {
        if let Some(m) = handle() {
            if let Ok(mut cb) = m.lock() {
                let _ = cb.set_text(s.to_owned());
            }
        }
    }
}

/// `(clipboard-get)` — the OS clipboard's text, or nil when it's empty / non-text /
/// unavailable (no display server, or a build without the `clipboard` feature). The
/// editor's yank consults this so text copied in another app pastes in.
pub(super) fn clipboard_get(_args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    #[cfg(feature = "clipboard")]
    if let Some(s) = clipboard::get_text() {
        return Ok(heap.alloc_string(&s));
    }
    #[cfg(not(feature = "clipboard"))]
    let _ = &heap;
    Ok(Value::nil())
}

/// `(clipboard-set! s)` — copy string `s` to the OS clipboard so other apps can paste
/// it; returns `s` (so it threads). A no-op (still returns `s`) when no clipboard is
/// available or the `clipboard` feature is off, so callers needn't special-case headless
/// builds. The editor's kill/copy commands call this so a kill is system-wide.
pub(super) fn clipboard_set(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "clipboard-set!", arg(args, 0))?;
    #[cfg(feature = "clipboard")]
    clipboard::set_text(&s);
    #[cfg(not(feature = "clipboard"))]
    let _ = &s;
    Ok(arg(args, 0))
}

/// `(%str-index-of s needle)` — the 0-based **char** index of the first
/// occurrence of `needle` in `s`, or -1 if absent. Linear: Rust's byte-level
/// `str::find`, then a one-pass byte→char-index conversion of the prefix. The
/// empty needle matches at 0 (matching `index-of`'s contract). The search
/// primitive the Brood `index-of`/`string-contains?` ride on; see the

pub(super) fn str_index_of(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "%str-index-of", arg(args, 0))?;
    let needle = expect_string(heap, "%str-index-of", arg(args, 1))?;
    let idx = match s.find(needle.as_str()) {
        Some(byte) => s[..byte].chars().count() as i64,
        None => -1,
    };
    Ok(Value::int(idx))
}

/// `(string-split s sep)` — split `s` into a list of substrings on each occurrence
/// of `sep`, in one O(n) pass. An empty separator splits `s` into its individual
/// characters (1-char strings). Mirrors the semantics of the former pure-Brood
/// `string-split`/`string->list`, but without the O(n²) tail-substring rebuild.
pub(super) fn string_split(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "string-split", arg(args, 0))?;
    let sep = expect_string(heap, "string-split", arg(args, 1))?;
    let out: Vec<Value> = if sep.is_empty() {
        s.chars().map(|c| heap.alloc_string(&c.to_string())).collect()
    } else {
        s.split(sep.as_str()).map(|part| heap.alloc_string(part)).collect()
    };
    Ok(heap.list_from_slice(&out))
}

/// `(to-fixed x n)` — x rendered with exactly `n` digits after the decimal point
/// (rounded). The one float→text op the language can't bootstrap: `str`/`pr-str`
/// print the shortest round-tripping form (full f64 precision, e.g.
/// `0.015873015873015872`), which is wrong for tabular/console output. An int `x`
/// is promoted, so `(to-fixed 3 2)` is `"3.00"`. `n` must be non-negative.
pub(super) fn to_fixed(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let x = expect_number(heap, "to-fixed", arg(args, 0))?;
    let n = expect_int(heap, "to-fixed", arg(args, 1))?;
    if n < 0 {
        return Err(LispError::runtime(format!(
            "to-fixed: decimal places must be non-negative, got {}",
            n
        ))
        .with_code(crate::error::error_codes::INDEX_OUT_OF_RANGE));
    }
    // Bound the width: `format!("{:.*}", n, x)` materialises an `n`-digit string,
    // so an unbounded `n` (e.g. `(to-fixed 1.0 1000000000)`) allocates ~1 GB on the
    // Rust side, bypassing the GC/soft-memory cap. An f64 carries ~17 significant
    // digits; past that the tail is just zeros, so 1000 is far beyond any real use
    // while keeping the worst-case alloc to ~1 KB.
    const MAX_DECIMALS: i64 = 1000;
    if n > MAX_DECIMALS {
        return Err(LispError::runtime(format!(
            "to-fixed: decimal places {n} too large (max {MAX_DECIMALS}); an f64 has \
             ~17 significant digits, so a larger count only pads zeros"
        ))
        .with_code(crate::error::error_codes::INDEX_OUT_OF_RANGE));
    }
    let s = format!("{:.*}", n as usize, x);
    Ok(heap.alloc_string(&s))
}

/// `(upper s)` — `s` with every character upper-cased. Case folding is
/// Unicode-aware (e.g. `ß` → `SS`), so it leans on the standard library's tables
/// rather than being expressible in Brood.
pub(super) fn upper(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "upper", arg(args, 0))?;
    Ok(heap.alloc_string(&s.to_uppercase()))
}

/// `(lower s)` — `s` with every character lower-cased (Unicode-aware, like `upper`).
pub(super) fn lower(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "lower", arg(args, 0))?;
    Ok(heap.alloc_string(&s.to_lowercase()))
}

pub(super) fn char_to_int(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "char->int", arg(args, 0))?;
    match s.chars().next() {
        Some(c) => Ok(Value::int(c as i64)),
        None => Err(LispError::runtime("char->int: empty string")),
    }
}

pub(super) fn int_to_char(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let n = expect_int(heap, "int->char", arg(args, 0))?;
    let c = char::from_u32(n as u32).ok_or_else(|| {
        LispError::runtime(format!("int->char: {} is not a valid Unicode codepoint", n))
    })?;
    let mut buf = [0u8; 4];
    Ok(heap.alloc_string(c.encode_utf8(&mut buf)))
}

pub(super) fn string_to_utf8_bytes(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "string->utf8-bytes", arg(args, 0))?;
    let items: Vec<Value> = s.as_bytes().iter().map(|&b| Value::int(b as i64)).collect();
    Ok(heap.alloc_vector(items))
}

pub(super) fn utf8_bytes_to_string(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    // Accepts a vector *or* a proper list of byte integers (0–255).
    let v = arg(args, 0);
    let items: Vec<Value> = match v {
        Value::Vector(id) => heap.vector(id).to_vec(),
        Value::Nil => vec![],
        Value::Pair(_) => {
            let mut out = Vec::new();
            let mut cur = v;
            loop {
                match cur {
                    Value::Pair(id) => {
                        let (head, tail) = heap.pair(id);
                        out.push(head);
                        cur = tail;
                    }
                    Value::Nil => break,
                    other => {
                        return Err(LispError::wrong_type(
                            heap,
                            "utf8-bytes->string",
                            "proper list",
                            other,
                        ))
                    }
                }
            }
            out
        }
        other => {
            return Err(LispError::wrong_type(
                heap,
                "utf8-bytes->string",
                "vector or list",
                other,
            ))
        }
    };
    let mut bytes = Vec::with_capacity(items.len());
    for (i, val) in items.iter().enumerate() {
        match val {
            Value::Int(n) if *n >= 0 && *n <= 255 => bytes.push(*n as u8),
            Value::Int(n) => {
                return Err(LispError::runtime(format!(
                    "utf8-bytes->string: byte at index {} is out of range: {}",
                    i, n
                )))
            }
            other => {
                return Err(LispError::wrong_type(
                    heap,
                    "utf8-bytes->string",
                    "int",
                    *other,
                ))
            }
        }
    }
    match String::from_utf8(bytes) {
        Ok(s) => Ok(heap.alloc_string(&s)),
        Err(e) => Err(LispError::runtime(format!(
            "utf8-bytes->string: invalid UTF-8: {}",
            e
        ))),
    }
}

// ---------- transcendental math ----------

macro_rules! math1_unrestricted {
    ($name:ident, $brood:literal, $method:ident) => {
        pub(super) fn $name(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
            let x = expect_number(heap, $brood, arg(args, 0))?;
            Ok(Value::float(x.$method()))
        }
    };
}

macro_rules! math1_bounded {
    ($name:ident, $brood:literal, $method:ident) => {
        pub(super) fn $name(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
            let x = expect_number(heap, $brood, arg(args, 0))?;
            if x < -1.0 || x > 1.0 {
                return Err(LispError::runtime(format!(
                    "{}: argument {} is out of domain [-1, 1]",
                    $brood, x
                )));
            }
            Ok(Value::float(x.$method()))
        }
    };
}

macro_rules! math1_positive {
    ($name:ident, $brood:literal, $method:ident) => {
        pub(super) fn $name(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
            let x = expect_number(heap, $brood, arg(args, 0))?;
            if x <= 0.0 {
                return Err(LispError::runtime(format!(
                    "{}: argument {} must be positive",
                    $brood, x
                )));
            }
            Ok(Value::float(x.$method()))
        }
    };
}

math1_unrestricted!(math_sin, "sin", sin);
math1_unrestricted!(math_cos, "cos", cos);
math1_unrestricted!(math_tan, "tan", tan);
math1_unrestricted!(math_atan, "atan", atan);
math1_unrestricted!(math_exp, "exp", exp);
math1_bounded!(math_asin, "asin", asin);
math1_bounded!(math_acos, "acos", acos);
math1_positive!(math_ln, "ln", ln);
math1_positive!(math_log2, "log2", log2);
math1_positive!(math_log10, "log10", log10);

pub(super) fn math_f64_sqrt(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let x = expect_number(heap, "%f64-sqrt", arg(args, 0))?;
    if x < 0.0 {
        return Err(LispError::runtime(format!(
            "%f64-sqrt: argument {} must be non-negative",
            x
        )));
    }
    Ok(Value::float(x.sqrt()))
}

pub(super) fn math_atan2(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let y = expect_number(heap, "atan2", arg(args, 0))?;
    let x = expect_number(heap, "atan2", arg(args, 1))?;
    Ok(Value::float(y.atan2(x)))
}

// ---------- rope (editor buffer text — ADR-045) ----------
//
// All indices are **character** indices (matching the language's char-based
// string indexing), not bytes. Edits return a *fresh* rope (immutability):
// ropey clones share structure, so `clone()`-then-edit only copies touched
// B-tree nodes. Out-of-range indices raise a clean E-code error rather than
// letting ropey panic.

/// Raise a uniform out-of-range error attributed to `who`.
pub(super) fn rope_oob(who: &str, what: &str, got: i64, max: usize) -> LispError {
    LispError::runtime(format!(
        "{}: {} {} out of bounds (valid 0..={})",
        who, what, got, max
    ))
    .with_code(crate::error::error_codes::INDEX_OUT_OF_RANGE)
}

/// `(string->rope s)` — a rope holding the text of string `s`.
pub(super) fn string_to_rope(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "string->rope", arg(args, 0))?;
    Ok(heap.alloc_rope(ropey::Rope::from_str(&s)))
}

/// `(rope->string r)` — the full text of rope `r` as a string.
pub(super) fn rope_to_string(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let r = expect_rope(heap, "rope->string", arg(args, 0))?;
    Ok(heap.alloc_string(&r.to_string()))
}

/// `(rope-length r)` — the number of characters in `r`.
pub(super) fn rope_length(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let r = expect_rope(heap, "rope-length", arg(args, 0))?;
    Ok(Value::int(r.len_chars() as i64))
}

/// `(rope-line-count r)` — the number of lines in `r` (ropey counts a trailing
/// newline as ending a line, so `"a\n"` is 2 lines and `""` is 1).
pub(super) fn rope_line_count(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let r = expect_rope(heap, "rope-line-count", arg(args, 0))?;
    Ok(Value::int(r.len_lines() as i64))
}

/// `(rope-insert r idx s)` — a fresh rope with string `s` inserted at character
/// index `idx` (0..=length).
pub(super) fn rope_insert(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let mut r = expect_rope(heap, "rope-insert", arg(args, 0))?;
    let idx = expect_int(heap, "rope-insert", arg(args, 1))?;
    let s = expect_string(heap, "rope-insert", arg(args, 2))?;
    let len = r.len_chars();
    if idx < 0 || idx as usize > len {
        return Err(rope_oob("rope-insert", "index", idx, len));
    }
    r.insert(idx as usize, &s);
    Ok(heap.alloc_rope(r))
}

/// `(rope-delete r start end)` — a fresh rope with characters `[start, end)`
/// removed.
pub(super) fn rope_delete(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let mut r = expect_rope(heap, "rope-delete", arg(args, 0))?;
    let start = expect_int(heap, "rope-delete", arg(args, 1))?;
    let end = expect_int(heap, "rope-delete", arg(args, 2))?;
    let len = r.len_chars();
    if start < 0 || end < start || end as usize > len {
        return Err(rope_oob("rope-delete", "range end", end, len));
    }
    r.remove(start as usize..end as usize);
    Ok(heap.alloc_rope(r))
}

/// `(rope-slice r start end)` — the text of characters `[start, end)` as a string.
pub(super) fn rope_slice(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let r = expect_rope(heap, "rope-slice", arg(args, 0))?;
    let start = expect_int(heap, "rope-slice", arg(args, 1))?;
    let end = expect_int(heap, "rope-slice", arg(args, 2))?;
    let len = r.len_chars();
    if start < 0 || end < start || end as usize > len {
        return Err(rope_oob("rope-slice", "range end", end, len));
    }
    let s = r.slice(start as usize..end as usize).to_string();
    Ok(heap.alloc_string(&s))
}

/// `(rope-line r n)` — the text of line `n` (0-based), including its trailing
/// newline if present. The viewport-rendering primitive.
pub(super) fn rope_line(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let r = expect_rope(heap, "rope-line", arg(args, 0))?;
    let n = expect_int(heap, "rope-line", arg(args, 1))?;
    let lines = r.len_lines();
    if n < 0 || n as usize >= lines {
        return Err(rope_oob("rope-line", "line", n, lines.saturating_sub(1)));
    }
    let s = r.line(n as usize).to_string();
    Ok(heap.alloc_string(&s))
}

/// `(rope-char->line r idx)` — the 0-based line index containing character `idx`.
pub(super) fn rope_char_to_line(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let r = expect_rope(heap, "rope-char->line", arg(args, 0))?;
    let idx = expect_int(heap, "rope-char->line", arg(args, 1))?;
    let len = r.len_chars();
    if idx < 0 || idx as usize > len {
        return Err(rope_oob("rope-char->line", "index", idx, len));
    }
    Ok(Value::int(r.char_to_line(idx as usize) as i64))
}

/// `(rope-line->char r n)` — the character index where line `n` (0-based) begins.
pub(super) fn rope_line_to_char(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let r = expect_rope(heap, "rope-line->char", arg(args, 0))?;
    let n = expect_int(heap, "rope-line->char", arg(args, 1))?;
    let lines = r.len_lines();
    if n < 0 || n as usize > lines {
        return Err(rope_oob("rope-line->char", "line", n, lines));
    }
    Ok(Value::int(r.line_to_char(n as usize) as i64))
}


