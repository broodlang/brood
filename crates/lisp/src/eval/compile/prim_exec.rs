//! The VM's primitive executors: the Int×Int fast table, the float and generic apply, and the
//! rooted dispatch the bytecode loop takes for a `Prim2`/`Prim3` it cannot finish inline.
use super::*;

/// The Int×Int semantics of every inline-able op, in one table: just the fixnum
/// arithmetic, no type dispatch, no allocation. `None` means **defer** to the real
/// `%`-primitive, which owns every edge: i64 overflow (it promotes to a bignum, ADR
/// bignums), ÷0 and the `i64::MIN / -1` overflow (it raises), a non-exact `%div` (it
/// answers a `Ratio`), and the ops that need the heap. Marked `#[inline(always)]`
/// because it is tiny — LLVM constant-folds `op` at each call site, emitting a single
/// checked op or compare per instruction variant.
#[inline(always)]
pub(crate) fn prim2_int_fast(op: PrimOp, a: i64, b: i64) -> Option<Value> {
    match op {
        PrimOp::Add => a.checked_add(b).map(Value::int),
        PrimOp::Sub => a.checked_sub(b).map(Value::int),
        PrimOp::Mul => a.checked_mul(b).map(Value::int),
        PrimOp::Lt => Some(Value::boolean(a < b)),
        PrimOp::Le => Some(Value::boolean(a <= b)),
        PrimOp::Eq => Some(Value::boolean(a == b)),
        PrimOp::Rem => a.checked_rem(b).map(Value::int),
        // `%div` on two ints is EXACT: an even division is the Int quotient; anything
        // else defers.
        PrimOp::Div => match (a.checked_rem(b), a.checked_div(b)) {
            (Some(0), Some(q)) => Some(Value::int(q)),
            _ => None,
        },
        PrimOp::Quot => a.checked_div(b).map(Value::int),
        PrimOp::Max => Some(Value::int(a.max(b))),
        PrimOp::Min => Some(Value::int(a.min(b))),
        PrimOp::BitAnd => Some(Value::int(a & b)),
        PrimOp::BitOr => Some(Value::int(a | b)),
        PrimOp::BitXor => Some(Value::int(a ^ b)),
        // Cons needs heap alloc; VectorRef needs the heap (slab index) and its operands
        // aren't (Int, Int); handled in the exec arm.
        PrimOp::Cons
        | PrimOp::VectorRef
        | PrimOp::TableHas
        | PrimOp::TableGet
        | PrimOp::MapGet
        | PrimOp::ContractAwait => None,
    }
}

/// The inline fast path for a [`Node::Prim2`] (perf #1): handle the `(Int, Int)`
/// case of `op` directly, returning `Ok(Some(v))` when done inline, or `Ok(None)`
/// to defer to the real `%`-primitive — for any non-`(Int, Int)` operands (float
/// coercion, structural `=`, bignum operands, the canonical type errors), the
/// division edges, **and the i64-overflow cases**, which the native now resolves
/// by promoting to a bignum (ADR bignums) rather than erroring. Needs no heap:
/// the inline result is a scalar, so nothing is allocated and no GC can intervene.
pub(crate) fn prim_apply(op: PrimOp, x: Value, y: Value) -> Result<Option<Value>, LispError> {
    match (x.unpack(), y.unpack()) {
        // The one Int×Int table; its `None` is this function's defer.
        (ValueRef::Int(a), ValueRef::Int(b)) => Ok(prim2_int_fast(op, a, b)),
        _ => Ok(prim_apply_float(op, x, y)),
    }
}

/// The float fast path of [`prim_apply`] (ADR-096): both operands `Int`/`Float`
/// with at least one `Float` — exactly the shapes `num_bin`/`prim_lt`'s float
/// arms handle with a plain `f64` op after an exact `i64 as f64` coercion.
/// Everything else (`BigInt` operands, structural `=` on floats, `rem`/`quot`'s
/// numeric edges, division by zero) returns `None` so the real native owns the
/// result and the error messages.
pub(crate) fn prim_apply_float(op: PrimOp, x: Value, y: Value) -> Option<Value> {
    let (a, b) = match (x.unpack(), y.unpack()) {
        (ValueRef::Float(a), ValueRef::Float(b)) => (a, b),
        (ValueRef::Int(a), ValueRef::Float(b)) => (a as f64, b),
        (ValueRef::Float(a), ValueRef::Int(b)) => (a, b as f64),
        _ => return None,
    };
    Some(match op {
        PrimOp::Add => Value::float(a + b),
        PrimOp::Sub => Value::float(a - b),
        PrimOp::Mul => Value::float(a * b),
        PrimOp::Lt => Value::boolean(a < b),
        PrimOp::Le => Value::boolean(a <= b),
        // `%div`: the native errors on a zero denominator — defer that edge
        // (a NaN/inf denominator is not zero, so it stays inline, matching the
        // native's plain `a / b`).
        PrimOp::Div if b != 0.0 => Value::float(a / b),
        // `max`/`min` are NOT inlined for floats: the native `prim_max`/`prim_min`
        // select via `>`/`<` and return the *original* operand, so they (a) keep a
        // NaN operand (`a > NaN` is false → the other is kept) and (b) preserve
        // int-ness when the int operand wins (`(math/max 5 3.0)` → Int `5`). Rust's
        // `f64::max`/`min` discard NaN and this path would force a `Value::float`,
        // both diverging from the tree-walker (the reference). Defer to the native.
        // Bitwise ops are int-only; any float operand defers to the native (which errors).
        PrimOp::BitAnd | PrimOp::BitOr | PrimOp::BitXor => return None,
        // `=` is structural (the native owns float equality), `max`/`min` defer (above),
        // `rem`/`quot` take the numeric-tower path, and zero-denominator `%div` errors — defer.
        _ => return None,
    })
}

/// Guard-checked inline path shared by all three `Prim2` bytecode handlers.
/// Returns `Ok(Some(v))` when the operation completed inline (caller pushes `v`),
/// `Ok(None)` when the guard is stale or the operand shape needs the native
/// (overflow, BigInt, float-not-matched), and `Err` on a type/arithmetic error.
/// Handles `Cons` inline here (it allocates, so it needs `&mut Heap`).
#[inline(always)]
pub(crate) fn prim2_inline_exec(
    heap: &mut Heap,
    op: PrimOp,
    map: [u8; 2],
    swapped: bool,
    head: Symbol,
    guard: &AtomicU64,
    x: Value,
    y: Value,
) -> Result<Option<Value>, LispError> {
    let cur = heap.global_epoch();
    // The map the *head* itself resolves to (`resolve_prim`'s natural arg-map). For a
    // `(op Const Local)` fusion (`swapped`), the instruction's `map` was inverted so the
    // inline operand pick stays correct (`emit_node`), so un-invert it before comparing —
    // otherwise revalidation never matches and the arm silently slow-paths forever after
    // the first epoch bump (a `def`). Non-swapped instructions compare `map` directly.
    let head_map = if swapped {
        [1 - map[0] as usize, 1 - map[1] as usize]
    } else {
        [map[0] as usize, map[1] as usize]
    };
    let inlinable = guard.load(Ordering::Relaxed) == cur || {
        match resolve_prim(heap, head) {
            Some((op2, m2)) if op2 == op && m2 == head_map => {
                guard.store(cur, Ordering::Relaxed);
                true
            }
            _ => false,
        }
    };
    if !inlinable {
        return Ok(None);
    }
    // Int×Int fast path: `prim2_int_fast` is tiny and #[inline(always)] — LLVM
    // constant-folds `op` here, emitting one checked op or compare per handler,
    // with no function call and without bloating exec_chunk via full prim_apply.
    // (`VectorRef`/`Cons` never have `(Int, Int)` operands, so they skip this and
    // are handled on the cold path below — keeping this hot path branch-free of
    // them.)
    if let (ValueRef::Int(a), ValueRef::Int(b)) = (x.unpack(), y.unpack())
        && let Some(v) = prim2_int_fast(op, a, b)
    {
        crate::perf_bump!(prim2_inline);
        return Ok(Some(v));
    }
    // Int overflow, an inexact or ÷0 `%div`, or Cons with Int operands → the
    // native owns it (prim_apply's Int×Int arm is this same table).
    // Every other `=` answers inline too. The guard above proved the head still resolves
    // (through `=`'s passthrough arm) to the `%eq` native, and `%eq` (`prim_eq`) is
    // exactly `heap.equal` unless an operand is a lazy seq-view — the one case it needs an
    // evaluator for, which still defers. The commonest such compare is `(%eq (type-of x)
    // :kw)` from every type predicate (keyword identity inside `equal`); strings, floats
    // and structural values used to take the whole `prim2_dispatch_rooted` path (`=`
    // looked up, dispatched, `%eq` looked up, the native called) for the same answer.
    // The JIT's residual `=` (`brood_rt_equal`, KI-132) is this same rule.
    if op == PrimOp::Eq && !matches!(x, Value::SeqView(_)) && !matches!(y, Value::SeqView(_)) {
        crate::perf_bump!(prim2_inline);
        return Ok(Some(Value::boolean(heap.equal(x, y))));
    }
    // Float, BigInt, overflow, Cons, Div, VectorRef — the cold, type-coercing
    // path (not inlined, so it stays out of exec_chunk's instruction footprint).
    match prim_apply(op, x, y)? {
        Some(v) => {
            crate::perf_bump!(prim2_inline);
            Ok(Some(v))
        }
        None if op == PrimOp::Cons => {
            crate::perf_bump!(prim2_inline);
            Ok(Some(heap.alloc_pair(x, y)))
        }
        // `vector-ref`: a dense O(1) slab read. Inline only the in-bounds
        // `(Vector, Int)` case; non-vector / non-int / out-of-range defer
        // (`Ok(None)`) to the native, which owns the exact bounds + type errors.
        None if op == PrimOp::VectorRef => {
            if let (ValueRef::Vector(id), ValueRef::Int(n)) = (x.unpack(), y.unpack())
                && n >= 0
                && (n as usize) < heap.vector(id).len()
            {
                crate::perf_bump!(prim2_inline);
                return Ok(Some(heap.vector(id)[n as usize]));
            }
            Ok(None)
        }
        // `(get m k)` on a map: one CHAMP probe, no call — `Heap::map_get_inline` is the
        // rule (a hit, or a plain map's miss; a record's miss declines to `get`, whose
        // `%lookup-miss` resolves it through the `Lookup` ability). A non-map receiver
        // defers too, so the set / string / integer-index branches and every type error
        // stay in Brood, and this stays a fast path rather than a second implementation.
        // The shim's mark (KI-216): the value of the call it wraps, untouched. It is a prim
        // only so a frame suspended inside that call can be recognised by its resume point.
        None if op == PrimOp::ContractAwait => Ok(Some(y)),
        None if op == PrimOp::MapGet => {
            if let Some(v) = heap.map_get_inline(x, y) {
                crate::perf_bump!(prim2_inline);
                return Ok(Some(v));
            }
            Ok(None)
        }
        // `table-has?` / 2-arg `table-get`: run the table op directly, skipping the
        // whole native-call protocol. Same key guard as the natives (`check_key`), so a
        // closure/NaN key raises the identical error; a non-Table first operand defers
        // to the native for its exact type error. Errors (dropped table, bad key)
        // propagate — bit-identical to the dispatched native.
        None if op == PrimOp::TableHas => {
            if let ValueRef::Table(tid) = x.unpack() {
                crate::core::table::check_key("table/has?", y)?;
                crate::perf_bump!(prim2_inline);
                return Ok(Some(Value::boolean(crate::core::table::has(heap, tid, y)?)));
            }
            Ok(None)
        }
        None if op == PrimOp::TableGet => {
            if let ValueRef::Table(tid) = x.unpack() {
                crate::core::table::check_key("table/get", y)?;
                crate::perf_bump!(prim2_inline);
                return Ok(Some(crate::core::table::get(heap, tid, y, Value::Nil)?));
            }
            Ok(None)
        }
        None => Ok(None), // overflow or other deferred edge → fallback
    }
}

/// Slow-path dispatch shared by all three `Prim2` bytecode handlers.
/// Operands are already rooted at `[save]` and `[save+1]`; this function looks
/// up `head`, dispatches, truncates back to `save`, and returns the result.
/// Marked `inline(never)` to keep the cold path out of the hot dispatch loop.
#[inline(never)]
pub(crate) fn prim2_dispatch_rooted(
    heap: &mut Heap,
    head: Symbol,
    save: usize,
    pos: Option<Pos>,
    genv: EnvRoot,
) -> Result<Value, LispError> {
    crate::perf_bump!(prim2_fallback);
    let cur_env = heap.read_root_env(genv);
    let callee = match heap.env_get(cur_env, head) {
        Some(c) => c,
        // Autoload (ADR-335): operands are rooted at `save..`; `cur_env` is re-read.
        None => match crate::eval::derive::global_miss(heap, cur_env, head) {
            Ok(c) => c,
            Err(e) => {
                heap.truncate_roots(save);
                return Err(tag_pos(e, pos));
            }
        },
    };
    let cur_env = heap.read_root_env(genv);
    let sa = heap.root_at(save);
    let sb = heap.root_at(save + 1);
    let argv: SmallVec<[Value; 4]> = SmallVec::from_slice(&[sa, sb]);
    let result = dispatch(heap, callee, argv, false, cur_env).and_then(|s| force(heap, s));
    heap.truncate_roots(save);
    result.map_err(|e| tag_pos(e, pos))
}

/// [`prim2_dispatch_rooted`] for an operand that is a record (ADR-542) — where the
/// builtin may hand its call back: a record's `num/add` under `+`, its `compare-to` under
/// `<` (ADR-505). Dispatched as a call, so a closure comes back un-run as `Step::Tail` and
/// the caller pushes its frame where the result goes; a `receive` inside then parks.
#[inline(never)]
pub(crate) fn prim2_dispatch_step(
    heap: &mut Heap,
    head: Symbol,
    save: usize,
    pos: Option<Pos>,
    genv: EnvRoot,
) -> Result<Step, LispError> {
    crate::perf_bump!(prim2_fallback);
    let cur_env = heap.read_root_env(genv);
    let callee = match heap.env_get(cur_env, head) {
        Some(c) => c,
        // Autoload (ADR-335): operands are rooted at `save..`; `cur_env` is re-read.
        None => match crate::eval::derive::global_miss(heap, cur_env, head) {
            Ok(c) => c,
            Err(e) => {
                heap.truncate_roots(save);
                return Err(tag_pos(e, pos));
            }
        },
    };
    let cur_env = heap.read_root_env(genv);
    let sa = heap.root_at(save);
    let sb = heap.root_at(save + 1);
    let argv: SmallVec<[Value; 4]> = SmallVec::from_slice(&[sa, sb]);
    let step = dispatch(heap, callee, argv, true, cur_env);
    heap.truncate_roots(save);
    step.map_err(|e| tag_pos(e, pos))
}

/// Whether a binary primitive's fallback may run Brood under the builtin — an operand is a
/// record (ADR-542), whose arithmetic, ordering and `Lookup` are Brood — and so goes through
/// [`prim2_dispatch_step`] (ADR-505).
#[inline]
pub(crate) fn prim2_may_redirect(a: Value, b: Value) -> bool {
    matches!(a.unpack(), ValueRef::Record { .. }) || matches!(b.unpack(), ValueRef::Record { .. })
}

/// [`prim2_dispatch_rooted`]'s 3-ary sibling: operands already rooted at
/// `[save..save+3)`; look up `head`, dispatch, truncate, return.
#[inline(never)]
pub(crate) fn prim3_dispatch_rooted(
    heap: &mut Heap,
    head: Symbol,
    save: usize,
    pos: Option<Pos>,
    genv: EnvRoot,
) -> Result<Value, LispError> {
    crate::perf_bump!(prim2_fallback);
    let cur_env = heap.read_root_env(genv);
    let callee = match heap.env_get(cur_env, head) {
        Some(c) => c,
        // Autoload (ADR-335): operands are rooted at `save..`; `cur_env` is re-read.
        None => match crate::eval::derive::global_miss(heap, cur_env, head) {
            Ok(c) => c,
            Err(e) => {
                heap.truncate_roots(save);
                return Err(tag_pos(e, pos));
            }
        },
    };
    let cur_env = heap.read_root_env(genv);
    let argv: SmallVec<[Value; 4]> = SmallVec::from_slice(&[
        heap.root_at(save),
        heap.root_at(save + 1),
        heap.root_at(save + 2),
    ]);
    let result = dispatch(heap, callee, argv, false, cur_env).and_then(|s| force(heap, s));
    heap.truncate_roots(save);
    result.map_err(|e| tag_pos(e, pos))
}
