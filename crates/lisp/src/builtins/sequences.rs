use crate::core::heap::{Heap, SlabRef};
use crate::core::value::{self, EnvId, StrId, Value};
use crate::error::{LispError, LispResult};
use crate::syntax::printer;

use super::numeric::{
    arg, expect_int, expect_rope, expect_rope_ref, expect_string, expect_string_ref, num_to_f64,
    two,
};
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

/// A string argument to a **char-indexed** builtin: its bytes, its cached char count, and
/// whether a char index is also a byte offset. All three come from one slot resolution
/// because every one of these builtins needs all three, and the text is **borrowed** —
/// an owned copy of the haystack per call is what made incremental search quadratic once
/// already (`expect_string` still does that at ~113 other sites).
struct StrArg<'h> {
    id: StrId,
    s: SlabRef<'h, str>,
    chars: usize,
    ascii: bool,
}

impl StrArg<'_> {
    /// Byte offset of char `ci`, clamped to the end. Arithmetic when a char index *is* a
    /// byte offset; otherwise through the slot's sparse char→byte index (ADR-213), which
    /// is a lookup plus a bounded walk rather than a walk from the start.
    #[inline]
    fn char_to_byte(&self, heap: &Heap, ci: usize) -> usize {
        if self.ascii {
            ci.min(self.s.len())
        } else {
            heap.str_char_to_byte(self.id, ci)
        }
    }

    /// The return direction: a byte-level `find`/`match_indices` result as the char index
    /// the language speaks. `b` must be a char boundary.
    #[inline]
    fn byte_to_char(&self, heap: &Heap, b: usize) -> usize {
        if self.ascii {
            b
        } else {
            heap.str_byte_to_char(self.id, b)
        }
    }
}

/// Require a string, as the [`StrArg`] the char-indexed builtins work through.
#[inline]
fn expect_str_arg<'h>(heap: &'h Heap, who: &str, v: Value) -> Result<StrArg<'h>, LispError> {
    match v {
        Value::Str(id) => {
            let (chars, ascii) = heap.str_metrics(id);
            Ok(StrArg {
                id,
                s: heap.string(id),
                chars,
                ascii,
            })
        }
        other => Err(LispError::wrong_type(heap, who, "string", other)),
    }
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
pub(super) fn realize_seqviews(
    heap: &mut Heap,
    env: EnvId,
    args: &[Value],
) -> Result<Vec<Value>, LispError> {
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

/// If `v` is a RECORD (a `Value::Map` carrying `:__id__`), return its `Seqable` view — the
/// list its `->seq` ability op yields (its fields id-free by default, or a custom
/// collection's own sequence). Lets `first`/`rest`/`empty?` treat a record AS its sequence
/// (ADR-172 §7). Only reached on the builtin fallback: `first`/`rest` are `PrimOp1`s the JIT
/// inlines for lists, so the hot `fold--loop` never calls these. Returns `None` for a
/// non-record, so the caller keeps its normal path (a plain map stays a map).
pub(super) fn record_seq(heap: &mut Heap, v: Value) -> Result<Option<Value>, LispError> {
    let m = match v {
        Value::Map(m) => m,
        _ => return Ok(None),
    };
    if heap
        .map_get(m, Value::Keyword(crate::core::value::intern("__id__")))
        .is_none()
    {
        return Ok(None);
    }
    let genv = heap.global();
    let callee = heap
        .env_get(genv, crate::core::value::intern("->seq"))
        .ok_or_else(|| LispError::runtime("->seq: the Seqable protocol is unavailable"))?;
    Ok(Some(crate::eval::compile::apply_value(
        heap,
        callee,
        &[v],
        genv,
    )?))
}

pub(super) fn first(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let v0 = arg(args, 0);
    // a record dispatches to its `Seqable` view first (custom collection or fields).
    let v = match record_seq(heap, v0)? {
        Some(s) => s,
        None => v0,
    };
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
        // A set is a sequence of its elements (CHAMP order): its head, or nil if
        // empty — so `first`/`map`/`fold`/… treat a set as a seq (Clojure-like).
        Value::Set(id) => Ok(heap.set_elems(id).first().copied().unwrap_or(Value::nil())),
        // A map seqs as its `[k v]` pairs — the same view `seq`/`map`/`fold`/`last`
        // already take, so `first`/`rest` no longer erred on the one collection
        // every other seq op accepted.
        Value::Map(id) => match heap.map_first_entry(id) {
            Some((k, val)) => Ok(heap.alloc_vector(vec![k, val])),
            None => Ok(Value::nil()),
        },
        Value::Nil => Ok(Value::nil()),
        _ => Err(LispError::wrong_type(
            heap,
            "first",
            "list, vector, set, map or bytes",
            v,
        )),
    }
}

pub(super) fn rest(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let v0 = arg(args, 0);
    let v = match record_seq(heap, v0)? {
        Some(s) => s,
        None => v0,
    };
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
            // The next start can leave i64 near MIN/MAX — `(rest (%range 1 2 i64::MAX))`
            // — and the range is exhausted at exactly that point, so yield the empty
            // range instead of a wrapped `lo`. Matches `range_to_vec`/`range_eq_list`,
            // which end their walk on the same `checked_add` miss. (An unchecked `+`
            // here panicked under debug-assertions and silently produced a garbage
            // range in release.)
            match lo.checked_add(step) {
                Some(next) => Ok(heap.alloc_range(next, hi, step)),
                None => Ok(Value::nil()),
            }
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
        // The tail of a set is a plain list of its remaining elements (CHAMP order):
        // a set seqs as its elements, and after the first `rest` the fold walks a
        // list — so a `(fold f init a-set)` materialises the set at most once (O(n)).
        Value::Set(id) => {
            let items: Vec<Value> = heap.set_elems(id).into_iter().skip(1).collect();
            Ok(heap.list(items))
        }
        // The tail of a map is a plain list of its remaining `[k v]` pairs — the
        // set arm's reasoning, over the map's entry view (see `first`).
        Value::Map(id) => {
            let entries: Vec<(Value, Value)> = heap.map_entries(id).into_iter().skip(1).collect();
            let pairs: Vec<Value> = entries
                .into_iter()
                .map(|(k, val)| heap.alloc_vector(vec![k, val]))
                .collect();
            Ok(heap.list(pairs))
        }
        Value::Nil => Ok(Value::nil()),
        _ => Err(LispError::wrong_type(
            heap,
            "rest",
            "list, vector, set, map or bytes",
            v,
        )),
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
    let x0 = arg(args, 0);
    // a record is empty iff its `Seqable` view is (a custom empty queue, a field-less
    // record) — not iff the raw map is (which always carries `:__id__`).
    let x = match record_seq(heap, x0)? {
        Some(s) => s,
        None => x0,
    };
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
        Value::Set(id) => Ok(Value::boolean(heap.map_size(id) == 0)),
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
            // Fallible since the pre-size `capacity overflow` panic fix: a range wider
            // than `MAX_REALISED_RANGE` is refused as a catchable error, not a panic.
            let items = heap.range_to_vec(id)?;
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
/// delegated to the prelude `%seqview-realize` (`(reverse (fold %flip-cons nil
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
    // Hoisted out of the per-element loop deliberately: the choice is read once here and the
    // loop below branches on the resulting bool, so the ladder costs nothing per element.
    let use_vm = crate::eval::compile::tier_ceiling() >= crate::eval::compile::Tier::Bytecode;
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
            i = match i.checked_add(step) {
                Some(v) => v,
                None => break,
            };
        }
        return Ok(Value::int(int_acc));
    }

    range_reduce_slow(f, init, lo, hi, step, use_vm, env, heap)
}

/// `(%vector-reduce f acc v)` — left-fold a vector **by index** in a native loop.
///
/// The vector counterpart of [`range_reduce`], and it exists for the same reason: a
/// Brood-level fold pays a per-element `apply` that a native loop does not. The prelude's
/// `fold-vec` already dropped the `first`/`rest` list materialisation, but each element
/// still round-trips through the evaluator to call `f`, and — the part that costs the most
/// here — a reducer like `+` is a thin **passthrough wrapper**, so every element pays the
/// wrapper's redirect. [`reduce_prim_op`] resolves that wrapper ONCE (it is what makes
/// `(fold + 0 (range n))` fast today), but nothing on the vector path consulted it.
///
/// Measured on the `spawn-live` shape (100k fresh processes each folding a 16-cell payload,
/// the published row's exact `(fold + 0 p)`): the fold step cost **13.6 µs/unit** through
/// `fold-vec` against **8.8 µs** for the same fold written as `(fold %add 0 p)` — i.e. ~4.8 µs
/// of every unit was the passthrough redirect alone, on a row whose total is ~34 µs.
///
/// Ordering and semantics match `fold-vec` exactly: left-to-right, `(f acc item)`, and the
/// element is re-read from the (rooted) vector each step because every `apply` is a GC
/// safepoint that can relocate it.
pub(super) fn vector_reduce(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let f = arg(args, 0);
    let init = arg(args, 1);
    let vid = match arg(args, 2) {
        Value::Vector(id) => id,
        Value::Nil => return Ok(init),
        v => return Err(LispError::wrong_type(heap, "%vector-reduce", "vector", v)),
    };
    let n = heap.vector(vid).len();
    // Hoisted out of the per-element loop deliberately: the choice is read once here and the
    // loop below branches on the resulting bool, so the ladder costs nothing per element.
    let use_vm = crate::eval::compile::tier_ceiling() >= crate::eval::compile::Tier::Bytecode;
    // Primitive-reducer fast path: `+`/`*` directly, or through the prelude wrapper's
    // passthrough arm. This is the resolution the vector path never did.
    let prim = crate::eval::compile::reduce_prim_op(heap, f);

    // Tight i64 loop — no Value boxing, no root slot per element (integers are inline).
    // Mirrors `range_reduce`'s, including handing the remainder to the general loop on
    // overflow so a BigInt promotion stays bit-identical to the Brood fold.
    if let (Some(op), Some(mut int_acc)) = (prim, init.as_int()) {
        let mut i = 0usize;
        while i < n {
            let Some(x) = heap.vector(vid)[i].as_int() else {
                break; // non-int element — finish on the general path from here
            };
            match crate::eval::compile::prim_apply_int_step(op, int_acc, x) {
                Some(v) => int_acc = v,
                None => break, // overflow → general path from the current state
            }
            i += 1;
        }
        if i == n {
            return Ok(Value::int(int_acc));
        }
        return vector_reduce_general(f, Value::int(int_acc), vid, i, n, prim, use_vm, env, heap);
    }
    vector_reduce_general(f, init, vid, 0, n, prim, use_vm, env, heap)
}

/// The boxed/general half of [`vector_reduce`]: a non-int accumulator, a non-prim reducer,
/// or the tail of a fold that overflowed out of the i64 loop. Same three-tier step as
/// [`range_reduce_slow`] — inlined prim, else the resolved-once HOF arm, else a full apply.
#[allow(clippy::too_many_arguments)]
fn vector_reduce_general(
    f: Value,
    init: Value,
    vid: crate::core::value::VecId,
    start: usize,
    n: usize,
    prim: Option<crate::eval::compile::PrimOp>,
    use_vm: bool,
    env: EnvId,
    heap: &mut Heap,
) -> LispResult {
    let hof = if prim.is_none() && use_vm {
        crate::eval::compile::hof_resolve(heap, f, 2)
    } else {
        None
    };
    heap.root_scope(|heap| {
        let f_r = heap.root(f);
        let v_r = heap.root(Value::Vector(vid));
        let mut acc_r = heap.root(init);
        let mut i = start;
        while i < n {
            let f = heap.read_root(f_r);
            let acc = heap.read_root(acc_r);
            // Re-read the vector through its root: an `apply` below may have collected.
            let x = match heap.read_root(v_r) {
                Value::Vector(id) => heap.vector(id)[i],
                _ => break,
            };
            let step_call = |heap: &mut Heap, acc: Value| -> LispResult {
                if let Some(h) = &hof {
                    if let Some(r) = crate::eval::compile::hof_apply_step(heap, h, f, &[acc, x]) {
                        return r;
                    }
                }
                if use_vm {
                    crate::eval::compile::apply_value(heap, f, &[acc, x], env)
                } else {
                    apply(heap, f, &[acc, x], env)
                }
            };
            let next = match prim {
                Some(op) => match crate::eval::compile::prim_apply_step(op, acc, x)? {
                    Some(v) => v,
                    None => step_call(heap, acc)?,
                },
                None => step_call(heap, acc)?,
            };
            acc_r = heap.advance_root(acc_r, next);
            i += 1;
        }
        Ok(heap.read_root(acc_r))
    })
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
    // HOF fast path (gated): resolve the step closure's arm ONCE so the per-element call skips
    // arm re-resolution + passthrough/arity matching. Only for a non-prim reducer on the VM path.
    let hof = if prim.is_none() && use_vm {
        crate::eval::compile::hof_resolve(heap, f, 2)
    } else {
        None
    };
    heap.root_scope(|heap| {
        let f_r = heap.root(f);
        let mut acc_r = heap.root(init);
        let mut i = lo;
        while if step > 0 { i < hi } else { i > hi } {
            let f = heap.read_root(f_r);
            let acc = heap.read_root(acc_r);
            // Non-prim step: try the cached-arm fast path (falls back if `f` late-rebound or
            // the gate is off, i.e. `hof` is `None`).
            let step_call = |heap: &mut Heap, acc: Value| -> LispResult {
                if let Some(h) = &hof {
                    if let Some(r) =
                        crate::eval::compile::hof_apply_step(heap, h, f, &[acc, Value::int(i)])
                    {
                        return r;
                    }
                }
                if use_vm {
                    crate::eval::compile::apply_value(heap, f, &[acc, Value::int(i)], env)
                } else {
                    apply(heap, f, &[acc, Value::int(i)], env)
                }
            };
            let next = match prim {
                Some(op) => match crate::eval::compile::prim_apply_step(op, acc, Value::int(i))? {
                    Some(v) => v,
                    None => step_call(heap, acc)?,
                },
                None => step_call(heap, acc)?,
            };
            acc_r = heap.advance_root(acc_r, next);
            i = match i.checked_add(step) {
                Some(v) => v,
                None => break,
            };
        }
        Ok(heap.read_root(acc_r))
    })
}

/// `(%sort-asc coll)` — stable ascending sort of a numeric collection by `<`.
/// The fast path behind `(sort coll)` when no custom comparator is given;
/// the all-Brood `%merge-sort` in `std/prelude.blsp` still handles
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
    // error rather than an indeterminate-order partial sort. The same pass
    // unboxes the all-`Int` case into a plain `Vec<i64>` — see below for why
    // that is worth a second buffer.
    let mut ints: Vec<i64> = Vec::with_capacity(items.len());
    let mut all_int = true;
    for &v in &items {
        match v {
            Value::Int(n) => {
                // Once a Float has been seen the i64 buffer is dead, but the
                // loop still has to run to validate the remaining items.
                if all_int {
                    ints.push(n);
                }
            }
            // Any other number (Float/BigInt/Ratio/Decimal) drops the i64 fast
            // path; the general `value_cmp` sort below orders the full tower.
            Value::Float(_) | Value::BigInt(_) | Value::Ratio(_) | Value::Decimal(_) => {
                if all_int {
                    all_int = false;
                    ints = Vec::new(); // release the partial buffer
                }
            }
            _ => return Err(LispError::wrong_type(heap, "sort", "number", v)),
        }
    }

    // All-`Int` fast path: sort raw i64s instead of `Value`s. The general
    // `sort_by` below is a stable merge sort whose comparator re-`match`es a
    // 24-byte enum on every one of the ~n log n comparisons; on a slice of
    // i64 the compiler gets an unboxed, branch-predictable compare it can
    // vectorise.
    //
    // Measured A/B on one binary, benchmark suite's `sort` row (375k ints):
    // the sort call itself 106 -> 79 ms, the whole row 225 -> 196 ms. Note
    // what that does NOT say: comparison was only ~27 ms of the original
    // 106 ms. The rest is `seq_items` walking the cons spine in and
    // `heap.list` allocating a fresh 375k-cell list out, and this fast path
    // does not touch either. Anyone chasing the remaining ~79 ms should go
    // after the traversal/rebuild (i.e. allocation), not the comparator —
    // sorting is no longer the expensive part of `sort`.
    //
    // `sort_unstable` is safe to use here even though the general path is
    // stable: two equal `Int`s are the same value, so no observable ordering
    // distinguishes them. That does NOT hold for the mixed path, where an
    // Int and a Float can compare equal while remaining distinguishable
    // (`1` vs `1.0`), which is why only this branch drops stability.
    if all_int {
        ints.sort_unstable();
        // Reuse the `items` allocation rather than building a second Vec.
        for (slot, n) in items.iter_mut().zip(ints) {
            *slot = Value::Int(n);
        }
        return Ok(heap.list(items));
    }

    // Stable sort over the full numeric tower via the canonical `value_cmp`
    // (exact for Int/BigInt/Ratio/Decimal; Int-vs-Float compares precisely in
    // base 10). Only reached once a non-`Int` number appeared, so the common
    // all-int case above never pays for it.
    items.sort_by(|a, b| heap.value_cmp(*a, *b));

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
/// The vector counterpart of `%map-assoc`; O(n) copy (vectors are flat), one
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
    let pairs: Vec<(Value, Value)> = args
        .as_chunks::<2>()
        .0
        .iter()
        .map(|kv| (kv[0], kv[1]))
        .collect();
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

/// `(%dispatch impls op-key id)` — ability dispatch through the per-op inline cache
/// (ADR-172 §7). `impls` is the `*impls*` registry (passed by the op so the kernel stays
/// decoupled from the global's name), `op-key` the constant `[ability op]` vector, `id`
/// the first argument's dispatch keyword. Returns the impl `fn` (or nil). A pure,
/// cache-transparent memo of `impl-for`; see [`Heap::vm_dispatch`].
pub(super) fn dispatch(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Ok(heap.vm_dispatch(arg(args, 0), arg(args, 1), arg(args, 2)))
}

/// `(%registry-update! name op path value)` — atomically read-modify-write a global that
/// holds a registry (KI-22). `name` is the global's symbol, `op` one of `:assoc`,
/// `:assoc-new`, `:dissoc`, `:cons-new`, `path` a vector of one or two keys (nil for
/// `:cons-new`). Returns true if the registry was written, false if the op declined.
///
/// The whole sequence runs inside [`Heap::registry_update`] under the runtime's registry
/// lock — see there for why `(def *X* (assoc *X* …))` in Brood cannot be made safe.
pub(super) fn registry_update(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    use crate::core::heap::RegistryOp;
    let sym = match arg(args, 0) {
        Value::Sym(s) => s,
        v => {
            return Err(LispError::wrong_type(
                heap,
                "%registry-update!",
                "symbol",
                v,
            ))
        }
    };
    // Keywords are interned, so compare the interned ids rather than the Values.
    let op_sym = match arg(args, 1) {
        Value::Keyword(k) => k,
        v => {
            return Err(LispError::wrong_type(
                heap,
                "%registry-update!",
                "keyword",
                v,
            ))
        }
    };
    let op = if op_sym == value::intern("assoc") {
        RegistryOp::Assoc
    } else if op_sym == value::intern("assoc-new") {
        RegistryOp::AssocNew
    } else if op_sym == value::intern("dissoc") {
        RegistryOp::Dissoc
    } else if op_sym == value::intern("cons-new") {
        RegistryOp::ConsNew
    } else {
        return Err(LispError::type_err(
            "%registry-update!: op must be :assoc, :assoc-new, :dissoc or :cons-new",
        ));
    };
    let path = match arg(args, 2).unpack() {
        crate::core::value::ValueRef::Vector(id) => heap.vector(id).to_vec(),
        _ => Vec::new(),
    };
    Ok(Value::boolean(heap.registry_update(
        env,
        sym,
        op,
        &path,
        arg(args, 3),
    )))
}

/// `(%registry-member? name key)` — is registry global `name` a map containing `key`, read
/// from the shared globals table bypassing the per-process inline cache (ADR-225)? For a
/// load-once guard that must not miss a racing `provide`; see [`Heap::registry_member`].
pub(super) fn registry_member(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let sym = match arg(args, 0) {
        Value::Sym(s) => s,
        v => {
            return Err(LispError::wrong_type(
                heap,
                "%registry-member?",
                "symbol",
                v,
            ))
        }
    };
    Ok(Value::boolean(heap.registry_member(sym, arg(args, 1))))
}

/// `(%registry-cas! name old new)` — compare-and-swap a registry global (KI-23). Rebinds
/// `name` to `new` and returns true only if its current value still equals `old`; returns
/// false otherwise, leaving it untouched, so the caller can recompute and retry.
///
/// The general form of `%registry-update!`, for a registry whose update is not one map/list
/// op — `face-set`'s merge into the existing entry, `attach`'s strip-then-cons, the REPL's
/// filter-then-append. The transform stays an ordinary Brood function (the prelude's
/// `registry-swap!` retries around this); only the read-decide-write is indivisible. See
/// [`Heap::registry_cas`].
pub(super) fn registry_cas(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let sym = match arg(args, 0) {
        Value::Sym(s) => s,
        v => return Err(LispError::wrong_type(heap, "%registry-cas!", "symbol", v)),
    };
    Ok(Value::boolean(heap.registry_cas(
        env,
        sym,
        arg(args, 1),
        arg(args, 2),
    )))
}

/// `(%registry-names)` — the symbols of every global a registry update has written in this
/// runtime, as a list. See [`Heap::registry_names`]: it is what lets `std/tool/project.blsp`
/// derive the startup image's registry set instead of naming it (ADR-218).
pub(super) fn registry_names(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let names: Vec<Value> = heap.registry_names().into_iter().map(Value::Sym).collect();
    Ok(heap.list(names))
}

/// `(%map-get m k [default])` — the value `k` maps to, or `default` (nil if
/// omitted) when absent.
pub(super) fn map_get(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_map(heap, "%map-get", arg(args, 0))?;
    Ok(heap
        .map_get(id, arg(args, 1))
        .unwrap_or_else(|| arg(args, 2)))
}

/// `(%map-assoc m k v)` — a fresh map with `k` bound to `v`.
pub(super) fn map_assoc(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_map(heap, "%map-assoc", arg(args, 0))?;
    Ok(heap.map_assoc(id, arg(args, 1), arg(args, 2)))
}

/// `(%map-int-add m k delta)` — a fresh map with `k`'s integer value incremented
/// by `delta` (inserts `delta` when `k` is absent). Single trie traversal. Raises
/// past the i64 range, like `table-incr` — which the linear-map optimizer rewrites
/// this into, so the two agree (see [`Heap::map_int_add`]).
pub(super) fn map_int_add(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_map(heap, "%map-int-add", arg(args, 0))?;
    let delta = expect_int(heap, "%map-int-add", arg(args, 2))?;
    heap.map_int_add(id, arg(args, 1), delta)
}

/// `(%map-dissoc m k)` — a fresh map with `k` removed.
pub(super) fn map_dissoc(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_map(heap, "%map-dissoc", arg(args, 0))?;
    Ok(heap.map_dissoc(id, arg(args, 1)))
}

/// `(%map-pairs m)` — the entries as a list of `[k v]` vectors, in insertion
/// order, in one O(n) pass. The *single* map enumerator: `keys`/`vals`/
/// `contains?`/`reduce-kv` are all Brood over it (std/prelude.blsp).
pub(super) fn map_pairs(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_map(heap, "%map-pairs", arg(args, 0))?;
    let entries = heap.map_entries(id); // copy out, releasing the borrow before we alloc
    let pairs: Vec<Value> = entries
        .into_iter()
        .map(|(k, v)| heap.alloc_vector(vec![k, v]))
        .collect();
    Ok(heap.list(pairs))
}

/// `(%map-count m)` — the number of entries, O(1). The CHAMP root node tracks
/// its subtree size, so this never walks (or allocates) the entries; it's what
/// `count`/`empty?` on a map use instead of materialising `%map-pairs`.
pub(super) fn map_count(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_map(heap, "%map-count", arg(args, 0))?;
    Ok(Value::int(heap.map_size(id) as i64))
}

pub(super) fn expect_set(heap: &Heap, who: &str, v: Value) -> Result<value::MapId, LispError> {
    expect!(heap, who, v, "set",
        Value::Set(id) => id,
    )
}

/// Re-wrap the `MapId` a map op just produced as a **set** (both share the CHAMP
/// store; the set-op natives keep the backing values all `true`).
fn as_set(v: Value) -> Value {
    match v {
        Value::Map(id) => Value::set(id),
        _ => unreachable!("map op returns Value::Map"),
    }
}

/// `(%set a b c …)` — build a set from element args (the programmatic form of the
/// `#{ }` literal). Dedups by structural equality; every op returns a fresh set.
pub(super) fn set_construct(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Ok(heap.set_from_elems(args.to_vec()))
}

/// `(%set-add s x)` — a fresh set with `x` added (a set already holding `x` is
/// returned structurally unchanged — the CHAMP `assoc` is a no-op).
pub(super) fn set_add(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_set(heap, "%set-add", arg(args, 0))?;
    Ok(as_set(heap.map_assoc(id, arg(args, 1), Value::Bool(true))))
}

/// `(%set-remove s x)` — a fresh set with `x` removed (absent → unchanged).
pub(super) fn set_remove(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_set(heap, "%set-remove", arg(args, 0))?;
    Ok(as_set(heap.map_dissoc(id, arg(args, 1))))
}

/// `(%set-has? s x)` — is `x` an element of set `s`? O(log n) trie lookup.
pub(super) fn set_has(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_set(heap, "%set-has?", arg(args, 0))?;
    Ok(Value::boolean(heap.map_get(id, arg(args, 1)).is_some()))
}

/// `(%set-count s)` — the number of elements, O(1) (the CHAMP root tracks size).
pub(super) fn set_count(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_set(heap, "%set-count", arg(args, 0))?;
    Ok(Value::int(heap.map_size(id) as i64))
}

pub(super) fn string_length(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    match v {
        // O(1): the char count is cached on the slot. It used to be `chars().count()`
        // here — a full scan on every call, which is why a loop bounded by
        // `(string/length s)` was quadratic before it did anything else.
        Value::Str(id) => Ok(Value::int(heap.str_metrics(id).0 as i64)),
        _ => Err(LispError::wrong_type(heap, "string/length", "string", v)),
    }
}

/// `(string/display-width s)` — how many terminal/grid *cells* `s` occupies, counting
/// grapheme clusters (an emoji / flag / CJK char is 2, a combining mark 0). The
/// width-aware counterpart to `string-length` (which counts codepoints) — the
/// editor's column / cursor math uses it so a wide glyph advances two columns. The
/// GUI renderer advances the cell grid by the same measure (`crate::text_width`).
pub(super) fn display_width(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    match v {
        Value::Str(id) => Ok(Value::int(
            crate::text_width::display_width(&heap.string(id)) as i64,
        )),
        _ => Err(LispError::wrong_type(
            heap,
            "string/display-width",
            "string",
            v,
        )),
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
    // Streaming fast path for a lazy int range (`(string/join "," (range n))`): format
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
            i = match i.checked_add(step) {
                Some(v) => v,
                None => break,
            };
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

/// `(string/substring s start [end])` — the characters of `s` in `[start, end)`,
/// char-indexed (consistent with `string-length`). `end` defaults to the
/// string's length, so `(string/substring s start)` is "from `start` to the end".
/// Errors if out of range.

pub(super) fn substring(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    // The hot one: `char-at`, `starts-with?` and `ends-with?` are all Brood over this
    // (`std/prelude.blsp`), so its per-call cost is the floor for most string code. It had
    // three separate O(whole string) steps for what is usually a tiny result — an owned
    // `expect_string` copy, a `chars().count()` length, and a `chars().skip()` walk. With
    // a 216 KB haystack, `(string/char-at s 3)` cost ~11.5 µs and did not care that it was reading
    // the 4th character: measured with the CALL COUNT FIXED, the cost tracked the string's
    // size (1/6/23 ms as it grew 13.5k → 54k → 216k chars).
    let v = arg(args, 0);
    let start = expect_int(heap, "string/substring", arg(args, 1))?;
    let sub: String = {
        let h: &Heap = heap;
        let a = expect_str_arg(h, "string/substring", v)?;
        // The cached char count, O(1) — it used to be a `chars().count()` per call.
        let len = a.chars as i64;
        let end = match args.get(2) {
            Some(_) => expect_int(h, "string/substring", arg(args, 2))?,
            None => len,
        };
        if start < 0 || end < start || end > len {
            return Err(LispError::runtime(format!(
                "string/substring: range [{}, {}) out of bounds for length {}",
                start, end, len
            ))
            .with_code(crate::error::error_codes::INDEX_OUT_OF_RANGE));
        }
        // Both ends converted, so this is a direct slice — O(result) rather than O(end),
        // on multi-byte text as well. `chars().skip(start)` used to walk from byte 0 on
        // every call, which is what made a per-character scan quadratic off the ASCII path.
        let lo = a.char_to_byte(h, start as usize);
        let hi = a.char_to_byte(h, end as usize);
        a.s[lo..hi].to_string()
    };
    Ok(heap.alloc_string(&sub))
}

/// Shared body of `string-span` / `string-span-until`: from char `start`, count the
/// maximal run of chars whose membership in the set `chars` equals `in_set`, and
/// return the char index just past it. Char-indexed, like `substring`/`char-at`. The
/// forward char-class scan a tokenizer runs its inner loops on (skip a whitespace /
/// digit / delimiter run) — O(run) native instead of O(run) interpreted recursion.
pub(super) fn string_span_impl(
    args: &[Value],
    heap: &mut Heap,
    who: &str,
    in_set: bool,
) -> LispResult {
    // A tokenizer calls this once per token over one document, so an O(whole document)
    // step here is O(tokens x document) overall. It had three: the owned `expect_string`
    // copy, `chars().count()` for the length, and `chars().skip(start)`. Borrow, read the
    // cached count, and start from a byte offset the slot converts in O(1) (ASCII) or a
    // one-stride walk (multi-byte).
    let v = arg(args, 0);
    let start = expect_int(heap, who, arg(args, 1))?;
    let h: &Heap = heap;
    let a = expect_str_arg(h, who, v)?;
    let set = expect_string_ref(h, who, arg(args, 2))?;
    let len = a.chars as i64;
    if start < 0 || start > len {
        return Err(LispError::runtime(format!(
            "{}: start {} out of bounds for length {}",
            who, start, len
        ))
        .with_code(crate::error::error_codes::INDEX_OUT_OF_RANGE));
    }
    let byte_start = a.char_to_byte(h, start as usize);
    let mut idx = start as usize;
    for c in a.s[byte_start..].chars() {
        if set.contains(c) == in_set {
            idx += 1;
        } else {
            break;
        }
    }
    Ok(Value::int(idx as i64))
}

/// `(string/span s start chars)` — the char index just past the maximal run of chars
/// drawn from the set `chars`, beginning at `start` (so `start` itself when the char
/// there isn't in the set). For skipping a run *of* a class — whitespace, digits.
pub(super) fn string_span(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    string_span_impl(args, heap, "string/span", true)
}

/// `(string/span-until s start chars)` — the char index of the first char in the set
/// `chars` at or after `start` (or the length if none): the maximal run of chars
/// *not* in the set. For scanning up to a delimiter — comment-to-newline,
/// atom-to-delimiter, string-body-to-quote.
pub(super) fn string_span_until(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    string_span_impl(args, heap, "string/span-until", false)
}

/// Lexical category of an atom token (a maximal run of non-delimiter chars), matching
/// `std/editor/highlight`'s `hl--atom-face` shape: a `:`-prefixed or `nil`/`true`/`false`
/// constant is a `keyword`; one that parses as an int/float (like `string/->number`) is a
/// `number`; anything else is a plain `symbol`. The head-position special-form vs call
/// distinction is left to the consumer (it needs the surrounding `(`).

/// Scan a `|…|` bar body from `from` (just past the opening `|`) to just past the
/// closing `|` — honouring `\|`/`\\` escapes — or to `n` if unterminated. Shared by
/// the two `scan-tokens` bar arms (symbol and keyword).
pub(super) fn scan_bar(chars: &[char], n: usize, from: usize) -> usize {
    let mut j = from;
    while j < n {
        match chars[j] {
            '\\' => j += 2,
            '|' => {
                j += 1;
                break;
            }
            _ => j += 1,
        }
    }
    j.min(n)
}

/// `(%str-index-of s needle)` — the 0-based **char** index of the first
/// occurrence of `needle` in `s`, or -1 if absent. Linear: Rust's byte-level
/// `str::find`, then a one-pass byte→char-index conversion of the prefix. The
/// empty needle matches at 0 (matching `index-of`'s contract). The search
/// primitive the Brood `index-of`/`includes?` ride on; see the

pub(super) fn str_index_of(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    // Borrowed, not owned: `expect_string` would copy the whole haystack per call, which
    // is the difference between a linear incremental search and a quadratic one.
    let h: &Heap = heap;
    let a = expect_str_arg(h, "%str-index-of", arg(args, 0))?;
    let needle = expect_string_ref(h, "%str-index-of", arg(args, 1))?;
    // Optional 3rd arg: the CHAR index to start searching at. It exists so `index-of`'s
    // `from` does not have to build `(string/substring coll from n)` first — that copy is what
    // made "incremental search" over one string quadratic, the same trap the comment
    // above `string-split`'s registration describes for splitting. Searching a suffix
    // must not allocate one.
    let start = match args.get(2) {
        None | Some(Value::Nil) => 0usize,
        Some(&v) => match v {
            Value::Int(n) => n.max(0) as usize,
            other => return Err(LispError::wrong_type(heap, "%str-index-of", "int", other)),
        },
    };
    // Char index → byte offset and back, both through the slot: O(1) for a pure-ASCII
    // string (where the two numbers are equal) and an indexed lookup plus a bounded walk
    // otherwise. That is what makes an incremental search over one string linear rather
    // than O(position) per call **in both encoding regimes** — a char-count cache alone
    // could only do it for ASCII, because its mechanism is the ASCII test itself.
    //
    // A start past the end converts to the end, so an out-of-range start simply finds
    // nothing (matching the clamp the Brood side used to do).
    let byte_start = if start == 0 {
        0
    } else {
        a.char_to_byte(h, start)
    };
    let idx = match a.s[byte_start..].find(&*needle) {
        Some(rel) => a.byte_to_char(h, byte_start + rel) as i64,
        None => -1,
    };
    Ok(Value::int(idx))
}

/// `(%str-last-index-of s needle before)` — the char index of the **last** occurrence of
/// `needle` in `s` starting strictly before char index `before`, or -1.
///
/// Genuinely needs Rust, for the same reason as `string-split` and the `from` offset above:
/// the Brood version walked forward calling `(index-of s needle i)` per match, and every one
/// of those re-derives a char offset (and, before that offset existed, allocated a copy of
/// the suffix) — so a reverse search was O(matches x length). Measured 16.5x then 16.4x per
/// 4x of input, where linear is 4x. This is one forward pass with an advancing cursor.
///
/// It is on an editor hot path in both directions: `buffer.blsp`'s reverse search runs over
/// whole buffer text, and `lineedit.blsp` finds the current line's start (`last-index-of
/// text "\n" p`) on every keystroke.
pub(super) fn str_last_index_of(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    // Borrowed, not owned — see `%str-index-of`.
    let h: &Heap = heap;
    let a = expect_str_arg(h, "%str-last-index-of", arg(args, 0))?;
    let needle = expect_string_ref(h, "%str-last-index-of", arg(args, 1))?;
    // Cached char count, O(1); `char_len` used to be a full scan.
    let char_len = a.chars;
    let before = match args.get(2) {
        None | Some(Value::Nil) => char_len as i64,
        Some(&v) => match v {
            Value::Int(n) => n,
            other => {
                return Err(LispError::wrong_type(
                    heap,
                    "%str-last-index-of",
                    "int",
                    other,
                ))
            }
        },
    };
    // The empty needle matches at every position 0..=len, so the last start strictly before
    // `before` is `before - 1` (clamped). Kept as an explicit branch, exactly as the Brood
    // version had it: the general scan below would loop forever on a zero-width match.
    if needle.is_empty() {
        return Ok(Value::int(if before <= 0 {
            -1
        } else if before > char_len as i64 {
            char_len as i64
        } else {
            before - 1
        }));
    }
    if before <= 0 {
        return Ok(Value::int(-1));
    }
    // Byte limit for `before` (clamped past-the-end to the whole string). A match may START
    // before the limit and extend past it — that is still a match, so the bound is on the
    // match's start, not on the slice searched.
    let limit = if before as usize >= char_len {
        a.s.len()
    } else {
        a.char_to_byte(h, before as usize)
    };
    let mut best: Option<usize> = None;
    for (b, _) in a.s.match_indices(&*needle) {
        if b >= limit {
            break;
        }
        best = Some(b);
    }
    Ok(Value::int(match best {
        Some(b) => a.byte_to_char(h, b) as i64,
        None => -1,
    }))
}

/// `(%str-splice-diff old new)` — the minimal single splice `[lo hi repl]` that
/// turns `old` into `new`: replace `old[lo, hi)` (0-based CHAR indices) with the
/// string `repl`. The common prefix and suffix are trimmed off (the suffix never
/// overlaps the prefix), so the span is minimal; equal strings give `[n n ""]`.
/// One native byte-level pass snapped to char boundaries. Genuinely needs Rust:
/// this runs per keystroke on every process-hosted editor buffer (the myedit
/// flip), where the pure-Brood per-char scan (fn call + `char-at` per char) cost
/// ~40 ms/keystroke on a 300-line buffer — ~100× this pass.
pub(super) fn str_splice_diff(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    // Borrowed, not owned: this runs per keystroke over the WHOLE buffer text (twice),
    // and `expect_string` copied both. The result is three small values, so the borrows
    // end before the allocation below — the pattern every convertible `expect_string`
    // site takes (`seam`: the ones that allocate per piece *while* scanning, like
    // `string-split` and `scan-tokens`, cannot use it).
    let h: &Heap = heap;
    let old = expect_string_ref(h, "%str-splice-diff", arg(args, 0))?;
    let new = expect_string_ref(h, "%str-splice-diff", arg(args, 1))?;
    let ob = old.as_bytes();
    let nb = new.as_bytes();
    // Common byte prefix, snapped BACK to a char boundary in both (a boundary in
    // one is a boundary in the other: the prefixes are byte-identical).
    let mut p = ob.iter().zip(nb.iter()).take_while(|(a, b)| a == b).count();
    while p > 0 && !old.is_char_boundary(p) {
        p -= 1;
    }
    // Common byte suffix over the remainders (capped so it can't overlap the
    // prefix), snapped FORWARD (shrunk) to a char boundary in both.
    let max_suf = (ob.len() - p).min(nb.len() - p);
    let mut s = ob
        .iter()
        .rev()
        .zip(nb.iter().rev())
        .take(max_suf)
        .take_while(|(a, b)| a == b)
        .count();
    while s > 0 && !(old.is_char_boundary(ob.len() - s) && new.is_char_boundary(nb.len() - s)) {
        s -= 1;
    }
    let lo = old[..p].chars().count() as i64;
    let hi = lo + old[p..ob.len() - s].chars().count() as i64;
    let repl_str = new[p..nb.len() - s].to_string();
    drop((old, new));
    let repl = heap.alloc_string(&repl_str);
    Ok(heap.alloc_vector(vec![Value::int(lo), Value::int(hi), repl]))
}

/// `(string/split s sep)` — split `s` into a list of substrings on each occurrence
/// of `sep`, in one O(n) pass. An empty separator splits `s` into its individual
/// characters (1-char strings). Mirrors the semantics of the former pure-Brood
/// `string-split`/`string->list`, but without the O(n²) tail-substring rebuild.
pub(super) fn string_split(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "string/split", arg(args, 0))?;
    let sep = expect_string(heap, "string/split", arg(args, 1))?;
    let out: Vec<Value> = if sep.is_empty() {
        s.chars()
            .map(|c| heap.alloc_string(&c.to_string()))
            .collect()
    } else {
        s.split(sep.as_str())
            .map(|part| heap.alloc_string(part))
            .collect()
    };
    Ok(heap.list_from_slice(&out))
}

/// `(string/->codepoints s)` — the characters of `s` as a **vector of integer Unicode
/// codepoints**, one O(n) pass. The random-access text-scanning primitive:
/// parsers (std/regex, std/json, std/encoding) index code points with O(1)
/// `nth` and compare them as ints. Building the same vector in Brood —
/// `(apply vector (map string/char->int (string/->list s)))` — costs a 1-char string
/// allocation per char plus a closure call per char, and measured ~40 % of the
/// whole regex benchmark. Like `string-split`/`string-span`, this is text-access
/// *mechanism*; the parsers themselves stay in Brood.
pub(super) fn string_to_codepoints(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    // Borrowed: the codepoints are ints, so nothing is allocated while the borrow is
    // live — one copy of the string saved per call, on the parsers' hot path.
    let codes: Vec<Value> = {
        let s = expect_string_ref(heap, "string/->codepoints", arg(args, 0))?;
        s.chars().map(|c| Value::int(c as i64)).collect()
    };
    Ok(heap.alloc_vector(codes))
}

/// `(%codepoints->string codes)` — a string from a sequence of integer Unicode code
/// points: the **inverse of `string/->codepoints`**, which until now had none.
///
/// Its absence was a real gap, not a convenience. `string/->codepoints` is a native that
/// every text parser in `std/` uses to get an indexable code vector — and every one of them
/// then rebuilt its result with `(apply str (map int->char cs))`, which allocates a seq
/// view, calls a closure per code point to make a **one-character string**, and then
/// concatenates N of those variadically. That is what `std/string.blsp`'s
/// `codepoints->` was, so `json`'s per-string assembly, the regex matcher's and the hex
/// encoder's all paid it. One O(n) pass into a `String` replaces the whole shape.
///
/// Accepts a vector or list (and a `bytes` value, where a byte *is* its code point), so it
/// mirrors what the parsers actually hold. A value that is not an integer, or is not a
/// Unicode scalar (negative, above U+10FFFF, or a surrogate in D800–DFFF), is a clean
/// error naming the offender — a surrogate cannot be a `char`, and letting one through
/// would either panic or silently produce U+FFFD.
pub(super) fn codepoints_to_string(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let arg0 = arg(args, 0);
    // Collect the ints first, so the string is built without a live heap borrow.
    let codes: Vec<i64> = match arg0 {
        Value::Bytes(id) => heap
            .bytes(id)
            .as_bytes()
            .iter()
            .map(|b| *b as i64)
            .collect(),
        Value::Vector(id) => heap
            .vector(id)
            .to_vec()
            .iter()
            .map(int_code)
            .collect::<Result<_, _>>()
            .map_err(|v| bad_codepoint(heap, v))?,
        Value::Pair(_) | Value::Nil => {
            let mut out = Vec::new();
            let mut cur = arg0;
            while let Value::Pair(id) = cur {
                let (h, t) = heap.pair(id);
                out.push(int_code(&h).map_err(|v| bad_codepoint(heap, v))?);
                cur = t;
            }
            out
        }
        other => {
            return Err(LispError::wrong_type(
                heap,
                "%codepoints->string",
                "vector, list or bytes of codepoint ints",
                other,
            ))
        }
    };
    let mut out = String::with_capacity(codes.len());
    for c in codes {
        match u32::try_from(c).ok().and_then(char::from_u32) {
            Some(ch) => out.push(ch),
            None => {
                return Err(LispError::runtime(format!(
                    "%codepoints->string: {c} is not a Unicode scalar value (0..=0x10FFFF, \
                     excluding the surrogates 0xD800..=0xDFFF)"
                )))
            }
        }
    }
    Ok(heap.alloc_string(&out))
}

/// The int in `v`, or `v` itself when it is not one — the error carries the offender so
/// [`codepoints_to_string`] can name it.
fn int_code(v: &Value) -> Result<i64, Value> {
    match v {
        Value::Int(n) => Ok(*n),
        other => Err(*other),
    }
}

fn bad_codepoint(heap: &Heap, v: Value) -> LispError {
    LispError::wrong_type(heap, "%codepoints->string", "codepoint int", v)
}

/// `(string/->graphemes s)` — the **extended grapheme clusters** of `s` as a vector
/// of strings, one O(n) pass. The sibling of `string/->codepoints`, and the unit a
/// human means by "character": `"é"` written as `e` + U+0301 is two code points but
/// one grapheme, and a flag emoji is four code points and one grapheme. Cursor
/// motion, column arithmetic and truncation in `std/editor/*` all want this unit —
/// stepping a cursor by code point splits a cluster and corrupts the text. Not
/// bootstrappable: the boundary rules are UAX #29 tables, not a rule Brood can
/// express. `string/display-width` already segments the same way internally.
pub(super) fn string_to_graphemes(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use unicode_segmentation::UnicodeSegmentation;
    let s = expect_string(heap, "string/->graphemes", arg(args, 0))?;
    // `true` = *extended* grapheme clusters (UAX #29's recommended default, and
    // what the renderer and `string/display-width` use).
    let parts: Vec<String> = s.graphemes(true).map(|g| g.to_string()).collect();
    let vals: Vec<Value> = parts.iter().map(|g| heap.alloc_string(g)).collect();
    Ok(heap.alloc_vector(vals))
}

/// `(string/grapheme-count s)` — how many **extended grapheme clusters** `s` has: the
/// length a human means, and the exclusive upper bound for `grapheme-at`. One O(n)
/// segmentation pass that allocates nothing (`string->graphemes` had to build a
/// vector of n strings just to be counted).
pub(super) fn grapheme_count(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use unicode_segmentation::UnicodeSegmentation;
    let s = expect_string_ref(heap, "string/grapheme-count", arg(args, 0))?;
    Ok(Value::int(s.graphemes(true).count() as i64))
}

/// `(string/grapheme-at s i)` / `(string/grapheme-at s i default)` — the `i`-th grapheme cluster
/// of `s` as a string, or `default`/`nil` when `i` is out of range (never an error,
/// matching `nth`/`get`).
///
/// Why this is a primitive and not `(nth (string/->graphemes s) i)`: the docs require
/// a cursor to step by *cluster*, so that spelling was the only correct way to read
/// one character — and it builds a vector of every cluster in the string on **every
/// keystroke**. This walks to `i` and stops, allocating one string. The editor's
/// hottest path stops being O(n) in the buffer line's length.
pub(super) fn grapheme_at(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use unicode_segmentation::UnicodeSegmentation;
    let i = expect_int(heap, "string/grapheme-at", arg(args, 1))?;
    let default = args.get(2).copied().unwrap_or(Value::nil());
    if i < 0 {
        return Ok(default);
    }
    // Borrowed — the editor reads a cluster per keystroke, so a copy of the line (or the
    // buffer) per call is exactly what this path cannot afford.
    let found = {
        let s = expect_string_ref(heap, "string/grapheme-at", arg(args, 0))?;
        s.graphemes(true).nth(i as usize).map(|g| g.to_string())
    };
    match found {
        Some(g) => Ok(heap.alloc_string(&g)),
        None => Ok(default),
    }
}

/// `(string/substring-graphemes s start)` / `(… s start end)` — the half-open cluster range
/// `[start, end)` of `s` as a string, clamped to the ends (so it never errors, like
/// `take`/`drop`). The grapheme-indexed counterpart of `substring`, which is
/// codepoint-indexed and will happily slice a cluster in half — splitting `"é"`
/// (e + U+0301) into a bare `e` and an orphan combining mark.
pub(super) fn substring_graphemes(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use unicode_segmentation::UnicodeSegmentation;
    let start = expect_int(heap, "string/substring-graphemes", arg(args, 1))?.max(0) as usize;
    let end = match args.get(2) {
        None | Some(Value::Nil) => None,
        Some(_) => {
            Some(expect_int(heap, "string/substring-graphemes", arg(args, 2))?.max(0) as usize)
        }
    };
    let out: String = {
        let s = expect_string_ref(heap, "string/substring-graphemes", arg(args, 0))?;
        match end {
            Some(e) if e <= start => String::new(),
            Some(e) => s.graphemes(true).skip(start).take(e - start).collect(),
            None => s.graphemes(true).skip(start).collect(),
        }
    };
    Ok(heap.alloc_string(&out))
}

/// `(string/normalize s form)` — `s` in Unicode normalisation `form`, one of the
/// keywords `:nfc` `:nfd` `:nfkc` `:nfkd`. Text that a human reads as identical can
/// be several different strings — "é" is U+00E9 *or* U+0065 U+0301 — and Brood's `=`
/// is byte-structural, so only normalisation makes those compare equal. Canonical
/// (`nfc`/`nfd`) preserves meaning; compatibility (`nfkc`/`nfkd`) also folds
/// presentation differences (the ligature "ﬁ" → "fi", superscript "²" → "2"), which
/// is right for search and identifier matching and wrong for round-tripping text.
/// One primitive with a form keyword rather than four functions (ADR-011).
pub(super) fn string_normalize(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use unicode_normalization::UnicodeNormalization;
    let s = expect_string(heap, "string/normalize", arg(args, 0))?;
    let form = arg(args, 1);
    let name = match form {
        Value::Keyword(k) => crate::core::value::symbol_name(k),
        _ => {
            return Err(LispError::wrong_type(
                heap,
                "string/normalize",
                "keyword",
                form,
            ))
        }
    };
    let out: String = match name.as_str() {
        "nfc" => s.nfc().collect(),
        "nfd" => s.nfd().collect(),
        "nfkc" => s.nfkc().collect(),
        "nfkd" => s.nfkd().collect(),
        other => {
            return Err(LispError::runtime(format!(
                "string/normalize: unknown form :{other} (expected :nfc, :nfd, :nfkc or :nfkd)"
            )))
        }
    };
    Ok(heap.alloc_string(&out))
}

/// `(->fixed x n)` — x rendered with exactly `n` digits after the decimal point
/// (rounded). The one float→text op the language can't bootstrap: `str`/`pr-str`
/// print the shortest round-tripping form (full f64 precision, e.g.
/// `0.015873015873015872`), which is wrong for tabular/console output. An int `x`
/// is promoted, so `(->fixed 3 2)` is `"3.00"`. `n` must be non-negative.
pub(super) fn to_fixed(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let x = num_to_f64(heap, "->fixed", arg(args, 0))?;
    let n = expect_int(heap, "->fixed", arg(args, 1))?;
    if n < 0 {
        return Err(LispError::runtime(format!(
            "->fixed: decimal places must be non-negative, got {}",
            n
        ))
        .with_code(crate::error::error_codes::INDEX_OUT_OF_RANGE));
    }
    // Bound the width: `format!("{:.*}", n, x)` materialises an `n`-digit string,
    // so an unbounded `n` (e.g. `(->fixed 1.0 1000000000)`) allocates ~1 GB on the
    // Rust side, bypassing the GC/soft-memory cap. An f64 carries ~17 significant
    // digits; past that the tail is just zeros, so 1000 is far beyond any real use
    // while keeping the worst-case alloc to ~1 KB.
    const MAX_DECIMALS: i64 = 1000;
    if n > MAX_DECIMALS {
        return Err(LispError::runtime(format!(
            "->fixed: decimal places {n} too large (max {MAX_DECIMALS}); an f64 has \
             ~17 significant digits, so a larger count only pads zeros"
        ))
        .with_code(crate::error::error_codes::INDEX_OUT_OF_RANGE));
    }
    let s = format!("{:.*}", n as usize, x);
    Ok(heap.alloc_string(&s))
}

/// `(string/upper s)` — `s` with every character upper-cased. Case folding is
/// Unicode-aware (e.g. `ß` → `SS`), so it leans on the standard library's tables
/// rather than being expressible in Brood.
pub(super) fn upper(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string_ref(heap, "string/upper", arg(args, 0))?;
    Ok(heap.alloc_string(&s.to_uppercase()))
}

/// `(string/lower s)` — `s` with every character lower-cased (Unicode-aware, like `upper`).
pub(super) fn lower(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string_ref(heap, "string/lower", arg(args, 0))?;
    Ok(heap.alloc_string(&s.to_lowercase()))
}

pub(super) fn char_to_int(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "string/char->int", arg(args, 0))?;
    match s.chars().next() {
        Some(c) => Ok(Value::int(c as i64)),
        None => Err(LispError::runtime("string/char->int: empty string")),
    }
}

pub(super) fn int_to_char(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let n = expect_int(heap, "string/int->char", arg(args, 0))?;
    // Guard the u32 range *before* the cast: `n as u32` would silently truncate a
    // value outside [0, u32::MAX] and could alias a valid codepoint (returning the
    // wrong char) instead of erroring.
    let c = u32::try_from(n)
        .ok()
        .and_then(char::from_u32)
        .ok_or_else(|| {
            LispError::runtime(format!(
                "string/int->char: {} is not a valid Unicode codepoint",
                n
            ))
        })?;
    let mut buf = [0u8; 4];
    Ok(heap.alloc_string(c.encode_utf8(&mut buf)))
}

pub(super) fn string_to_utf8_bytes(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "string->utf8-bytes", arg(args, 0))?;
    let bytes = s.as_bytes().to_vec();
    Ok(super::io::bytes_to_value(&bytes, heap))
}

pub(super) fn utf8_bytes_to_string(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    // Accepts a `bytes` value, or (leniently) a vector or list of byte ints.
    let bytes = super::io::collect_bytes("utf8-bytes->string", arg(args, 0), heap)?;
    match String::from_utf8(bytes) {
        Ok(s) => Ok(heap.alloc_string(&s)),
        Err(e) => Err(LispError::runtime(format!(
            "utf8-bytes->string: invalid UTF-8: {}",
            e
        ))),
    }
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
    let s = expect_string(heap, "%string->rope", arg(args, 0))?;
    Ok(heap.alloc_rope(ropey::Rope::from_str(&s)))
}

/// `(rope->string r)` — the full text of rope `r` as a string.
pub(super) fn rope_to_string(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let r = expect_rope_ref(heap, "%rope->string", arg(args, 0))?;
    Ok(heap.alloc_string(&r.to_string()))
}

/// `(rope-length r)` — the number of characters in `r`.
pub(super) fn rope_length(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let r = expect_rope_ref(heap, "%rope-length", arg(args, 0))?;
    Ok(Value::int(r.len_chars() as i64))
}

/// `(rope-line-count r)` — the number of lines in `r` (ropey counts a trailing
/// newline as ending a line, so `"a\n"` is 2 lines and `""` is 1).
pub(super) fn rope_line_count(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let r = expect_rope_ref(heap, "%rope-line-count", arg(args, 0))?;
    Ok(Value::int(r.len_lines() as i64))
}

/// `(rope-insert r idx s)` — a fresh rope with string `s` inserted at character
/// index `idx` (0..=length).
pub(super) fn rope_insert(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let mut r = expect_rope(heap, "%rope-insert", arg(args, 0))?;
    let idx = expect_int(heap, "%rope-insert", arg(args, 1))?;
    let s = expect_string(heap, "%rope-insert", arg(args, 2))?;
    let len = r.len_chars();
    if idx < 0 || idx as usize > len {
        return Err(rope_oob("%rope-insert", "index", idx, len));
    }
    r.insert(idx as usize, &s);
    Ok(heap.alloc_rope(r))
}

/// `(rope-delete r start end)` — a fresh rope with characters `[start, end)`
/// removed.
pub(super) fn rope_delete(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let mut r = expect_rope(heap, "%rope-delete", arg(args, 0))?;
    let start = expect_int(heap, "%rope-delete", arg(args, 1))?;
    let end = expect_int(heap, "%rope-delete", arg(args, 2))?;
    let len = r.len_chars();
    if start < 0 || end < start || end as usize > len {
        return Err(rope_oob("%rope-delete", "range end", end, len));
    }
    r.remove(start as usize..end as usize);
    Ok(heap.alloc_rope(r))
}

/// `(rope-slice r start end)` — the text of characters `[start, end)` as a string.
pub(super) fn rope_slice(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let r = expect_rope_ref(heap, "%rope-slice", arg(args, 0))?;
    let start = expect_int(heap, "%rope-slice", arg(args, 1))?;
    let end = expect_int(heap, "%rope-slice", arg(args, 2))?;
    let len = r.len_chars();
    if start < 0 || end < start || end as usize > len {
        return Err(rope_oob("%rope-slice", "range end", end, len));
    }
    let s = r.slice(start as usize..end as usize).to_string();
    Ok(heap.alloc_string(&s))
}

/// `(rope-line r n)` — the text of line `n` (0-based), including its trailing
/// newline if present. The viewport-rendering primitive.
pub(super) fn rope_line(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let r = expect_rope_ref(heap, "%rope-line", arg(args, 0))?;
    let n = expect_int(heap, "%rope-line", arg(args, 1))?;
    let lines = r.len_lines();
    if n < 0 || n as usize >= lines {
        return Err(rope_oob("%rope-line", "line", n, lines.saturating_sub(1)));
    }
    let s = r.line(n as usize).to_string();
    Ok(heap.alloc_string(&s))
}

/// `(rope-char->line r idx)` — the 0-based line index containing character `idx`.
pub(super) fn rope_char_to_line(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let r = expect_rope_ref(heap, "%rope-char->line", arg(args, 0))?;
    let idx = expect_int(heap, "%rope-char->line", arg(args, 1))?;
    let len = r.len_chars();
    if idx < 0 || idx as usize > len {
        return Err(rope_oob("%rope-char->line", "index", idx, len));
    }
    Ok(Value::int(r.char_to_line(idx as usize) as i64))
}

/// `(rope-line->char r n)` — the character index where line `n` (0-based) begins.
pub(super) fn rope_line_to_char(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let r = expect_rope_ref(heap, "%rope-line->char", arg(args, 0))?;
    let n = expect_int(heap, "%rope-line->char", arg(args, 1))?;
    let lines = r.len_lines();
    if n < 0 || n as usize > lines {
        return Err(rope_oob("%rope-line->char", "line", n, lines));
    }
    Ok(Value::int(r.line_to_char(n as usize) as i64))
}
