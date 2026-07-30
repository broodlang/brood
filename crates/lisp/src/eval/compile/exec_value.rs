//! Node tree-walk executor + prim exec helpers (extracted from mod.rs).
use super::*;

/// Int×Int-only fast path for `prim2_inline_exec`: just the fixnum arithmetic,
/// no type dispatch, no allocation.  Marked `#[inline(always)]` because it is
/// tiny (one `match` arm per op) — LLVM constant-folds `op` at each call site
/// in `prim2_inline_exec` (itself always-inlined), emitting a single checked op
/// or compare per instruction variant.  Float, BigInt, overflow, Cons, and Div
/// all return `None`; the caller falls through to `prim_apply`.
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
        PrimOp::Quot => a.checked_div(b).map(Value::int),
        PrimOp::Max => Some(Value::int(a.max(b))),
        PrimOp::Min => Some(Value::int(a.min(b))),
        PrimOp::BitAnd => Some(Value::int(a & b)),
        PrimOp::BitOr => Some(Value::int(a | b)),
        PrimOp::BitXor => Some(Value::int(a ^ b)),
        // Cons needs heap alloc; Div may return Float — both handled by prim_apply.
        // VectorRef needs the heap (slab index) and its operands aren't (Int, Int);
        // handled directly in prim2_inline_exec.
        PrimOp::Cons | PrimOp::Div | PrimOp::VectorRef | PrimOp::TableHas | PrimOp::TableGet => {
            None
        }
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
    let (a, b) = match (x.unpack(), y.unpack()) {
        (ValueRef::Int(a), ValueRef::Int(b)) => (a, b),
        _ => return Ok(prim_apply_float(op, x, y)),
    };
    let v = match op {
        // On i64 overflow, defer (`Ok(None)`): the native `prim_add`/etc. redo
        // the op in BigInt and demote, so a too-big result becomes a `BigInt`
        // instead of an `E0041`.
        PrimOp::Add => match a.checked_add(b) {
            Some(r) => Value::int(r),
            None => return Ok(None),
        },
        PrimOp::Sub => match a.checked_sub(b) {
            Some(r) => Value::int(r),
            None => return Ok(None),
        },
        PrimOp::Mul => match a.checked_mul(b) {
            Some(r) => Value::int(r),
            None => return Ok(None),
        },
        PrimOp::Lt => Value::boolean(a < b),
        PrimOp::Le => Value::boolean(a <= b),
        PrimOp::Eq => Value::boolean(a == b),
        // Division family: handle the clean integer case inline, and **defer**
        // (`Ok(None)`) every edge — div-by-zero, the `i64::MIN / -1` overflow,
        // and (`%div` only) a non-exact quotient that the native returns as a
        // Float — so the native owns those exact results and error messages.
        PrimOp::Rem => match a.checked_rem(b) {
            Some(r) => Value::int(r),
            None => return Ok(None),
        },
        // `%div`: an even division yields the exact Int; a remainder yields the
        // float `a as f64 / b as f64` — exactly `prim_div`'s int arm (this covers
        // `i64::MIN / -1` too: its `checked_rem` is None and the native also takes
        // the float path). Only ÷0 defers, for the native's exact error. Inlining
        // the non-exact case matters: `(/ px n)` per pixel was 582k full dispatches
        // in mandelbrot alone.
        PrimOp::Div => match (a.checked_rem(b), a.checked_div(b)) {
            (Some(0), Some(q)) => Value::int(q),
            _ if b != 0 => Value::Float(a as f64 / b as f64),
            _ => return Ok(None),
        },
        PrimOp::Quot => match a.checked_div(b) {
            Some(q) => Value::int(q),
            None => return Ok(None),
        },
        PrimOp::Max => Value::int(a.max(b)),
        PrimOp::Min => Value::int(a.min(b)),
        PrimOp::BitAnd => Value::int(a & b),
        PrimOp::BitOr => Value::int(a | b),
        PrimOp::BitXor => Value::int(a ^ b),
        // Handled in the exec arm (they need `&mut Heap` / the heap); never reach here.
        PrimOp::Cons | PrimOp::VectorRef | PrimOp::TableHas | PrimOp::TableGet => return Ok(None),
    };
    Ok(Some(v))
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
        // int-ness when the int operand wins (`(max 5 3.0)` → Int `5`). Rust's
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
    if let (ValueRef::Int(a), ValueRef::Int(b)) = (x.unpack(), y.unpack()) {
        if let Some(v) = prim2_int_fast(op, a, b) {
            crate::perf_bump!(prim2_inline);
            return Ok(Some(v));
        }
        // Int overflow, Div, or Cons with Int operands → fall through to prim_apply.
    }
    // Interned-immediate `=` fast path: `(%eq (type-of x) :kw)` is the single most
    // common non-int comparison in Brood — every type predicate (`empty?`/`nil?`/
    // `cond`/…) runs it, and `type-of` yields a `Keyword`. Comparing two keywords
    // (or two symbols) is interned-id equality, exactly what `heap.equal` returns
    // for them, with no heap touch and no native call. Without this, each one missed
    // both `prim2_int_fast` and `prim_apply` (numeric-only) and took the full
    // `prim2_dispatch_rooted` slow path (measured: 28% of nqueens' prim2 ops).
    if op == PrimOp::Eq {
        let eq = match (x.unpack(), y.unpack()) {
            (ValueRef::Keyword(a), ValueRef::Keyword(b)) => Some(a == b),
            (ValueRef::Sym(a), ValueRef::Sym(b)) => Some(a == b),
            _ => None,
        };
        if let Some(r) = eq {
            crate::perf_bump!(prim2_inline);
            return Ok(Some(Value::boolean(r)));
        }
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
            if let (ValueRef::Vector(id), ValueRef::Int(n)) = (x.unpack(), y.unpack()) {
                if n >= 0 && (n as usize) < heap.vector(id).len() {
                    crate::perf_bump!(prim2_inline);
                    return Ok(Some(heap.vector(id)[n as usize]));
                }
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
                crate::core::table::check_key("table-has?", y)?;
                crate::perf_bump!(prim2_inline);
                return Ok(Some(Value::boolean(crate::core::table::has(heap, tid, y)?)));
            }
            Ok(None)
        }
        None if op == PrimOp::TableGet => {
            if let ValueRef::Table(tid) = x.unpack() {
                crate::core::table::check_key("table-get", y)?;
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
        None => {
            heap.truncate_roots(save);
            return Err(tag_pos(crate::eval::unbound_error(heap, head), pos));
        }
    };
    let sa = heap.root_at(save);
    let sb = heap.root_at(save + 1);
    let argv: SmallVec<[Value; 4]> = SmallVec::from_slice(&[sa, sb]);
    let result = dispatch(heap, callee, argv, false, cur_env).and_then(|s| force(heap, s));
    heap.truncate_roots(save);
    result.map_err(|e| tag_pos(e, pos))
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
        None => {
            heap.truncate_roots(save);
            return Err(tag_pos(crate::eval::unbound_error(heap, head), pos));
        }
    };
    let argv: SmallVec<[Value; 4]> = SmallVec::from_slice(&[
        heap.root_at(save),
        heap.root_at(save + 1),
        heap.root_at(save + 2),
    ]);
    let result = dispatch(heap, callee, argv, false, cur_env).and_then(|s| force(heap, s));
    heap.truncate_roots(save);
    result.map_err(|e| tag_pos(e, pos))
}

/// Execute one node in **value position** — operands, call arguments, literal
/// elements, binding right-hand sides: the overwhelmingly common case. Returns
/// the value directly — no [`Step`] is built and no [`force`] unwrap runs. A
/// `Call` reached here was compiled `tail = false`, so [`exec_call`]'s step is
/// always `Done` (and a stray `Tail` is still resolved safely by [`force`]).
pub(crate) fn exec_value(
    heap: &mut Heap,
    node: &Node,
    frame_base: usize,
    genv: EnvRoot,
) -> LispResult {
    match node {
        Node::Const(cv) => Ok(cv.load()),
        // Slot read — depth 0: the callee's own frame. (Deeper depths arrive with
        // the full compiler; the slice only binds params.)
        Node::Local(i) => Ok(heap.root_at(frame_base + i)),
        Node::Global(s) => match heap.env_get(heap.read_root_env(genv), *s) {
            Some(v) => Ok(v),
            None => Err(crate::eval::unbound_error(heap, *s)),
        },
        Node::GlobalIc { sym, site } => {
            let env = heap.read_root_env(genv);
            // The IC engages only when free names resolve through the process
            // global (same gate as the call-site IC): a captured-env frame can
            // shadow the symbol, and differs per closure instance.
            if heap.is_global(env) {
                let epoch = heap.global_epoch();
                if let Some(v) = heap.vm_global_ic_probe(*site, *sym, epoch) {
                    crate::perf_bump!(global_ic_hit);
                    return Ok(v);
                }
                crate::perf_bump!(global_ic_miss);
                return match heap.env_get(env, *sym) {
                    Some(v) => {
                        // Never cache a dynamic symbol — `binding` rebinds it
                        // without bumping the epoch (a later `defdyn` of a cached
                        // symbol bumps it, so the stale entry self-invalidates).
                        if !value::is_dynamic(*sym) {
                            heap.vm_global_ic_put(*site, *sym, epoch, v);
                        }
                        Ok(v)
                    }
                    None => Err(crate::eval::unbound_error(heap, *sym)),
                };
            }
            match heap.env_get(env, *sym) {
                Some(v) => Ok(v),
                None => Err(crate::eval::unbound_error(heap, *sym)),
            }
        }
        Node::If(cond, then, els) => {
            let c = exec_value(heap, cond, frame_base, genv)?;
            if crate::eval::truthy(c) {
                exec_value(heap, then, frame_base, genv)
            } else {
                exec_value(heap, els, frame_base, genv)
            }
        }
        Node::Do(nodes) => {
            if nodes.is_empty() {
                return Ok(Value::nil());
            }
            let last = nodes.len() - 1;
            for n in &nodes[..last] {
                exec_value(heap, n, frame_base, genv)?; // for effect
            }
            exec_value(heap, &nodes[last], frame_base, genv)
        }
        Node::Vector(elems) => {
            // Evaluate each element, keeping the results on the operand stack so a
            // collection during a later element relocates them in place (mirrors the
            // `Call` arg loop); then build a fresh vector. `save` is truncated on
            // every path, including errors.
            let save = heap.roots_len();
            for e in elems.iter() {
                match exec_value(heap, e, frame_base, genv) {
                    Ok(v) => heap.push_root(v),
                    Err(err) => {
                        heap.truncate_roots(save);
                        return Err(err);
                    }
                }
            }
            let n = elems.len();
            let mut vals = Vec::with_capacity(n);
            for k in 0..n {
                vals.push(heap.root_at(save + k));
            }
            heap.truncate_roots(save);
            Ok(heap.alloc_vector(vals))
        }
        Node::Map(entries) => {
            // Same operand-stack discipline as `Vector`: each key then value is
            // pushed (so a collection mid-build relocates them), then a fresh map is
            // built from the relocated pairs.
            let save = heap.roots_len();
            for (kn, vn) in entries.iter() {
                for node in [kn, vn] {
                    match exec_value(heap, node, frame_base, genv) {
                        Ok(v) => heap.push_root(v),
                        Err(err) => {
                            heap.truncate_roots(save);
                            return Err(err);
                        }
                    }
                }
            }
            let n = entries.len();
            let mut pairs = Vec::with_capacity(n);
            for i in 0..n {
                pairs.push((heap.root_at(save + 2 * i), heap.root_at(save + 2 * i + 1)));
            }
            heap.truncate_roots(save);
            Ok(heap.map_from_pairs(pairs))
        }
        Node::LetBind { binds, body } => {
            // Value-position `let` (an argument/operand): same slot discipline as
            // the tail flavor in `exec_node`, body in value position.
            for (slot, rhs) in binds.iter() {
                let v = exec_value(heap, rhs, frame_base, genv)?;
                heap.set_root_at(frame_base + slot, v);
            }
            exec_value(heap, body, frame_base, genv)
        }
        Node::MakeClosure {
            fn_rest,
            captures,
            self_name,
        } => {
            // Build the captured env: a flat snapshot of the enclosing lexicals
            // (parent = the process global, so true globals + dynamics still resolve
            // live and late-bound). No `captures` source is a call, so evaluating
            // them runs no safepoint — the fresh `frame` and the (immovable) node
            // fields stay valid until `make_closure` consumes them below. With no
            // captures *and* no self-name the closure is global-capturing
            // (`env == None`); a self-name needs a frame to bind into.
            let env = if captures.is_empty() && self_name.is_none() {
                heap.global()
            } else {
                let frame = heap.new_env(Some(heap.global()));
                for (name, src) in captures.iter() {
                    let v = exec_value(heap, src, frame_base, genv)?;
                    heap.env_define(frame, *name, v);
                }
                frame
            };
            let closure = crate::eval::make_closure_cached(heap, fn_rest.load(), env)?;
            // Direct `letrec` self-recursion: bind the binder name to the closure
            // we just built, in the closure's own captured env. The recursive call
            // then resolves through that env (uncached — a local-capturing frame
            // isn't `is_global`, so neither inline cache engages). This makes the
            // env contain the closure while the closure captures the env — the same
            // cycle the tree-walker's `letrec` builds, handled by the tracing GC.
            if let Some(name) = self_name {
                heap.env_define(env, *name, closure);
            }
            Ok(closure)
        }
        Node::SelfCall { .. } => {
            // Emitted only in tail position (`compile_node`'s `if tail` guard), so it
            // is always handled by `exec_node`, never reached here in value position.
            unreachable!("Node::SelfCall is tail-only — exec_node handles it");
        }
        Node::Call {
            staged: _,
            callee,
            args,
            tail,
            pos,
            file,
            site,
        } => {
            let step = exec_call(
                heap,
                callee,
                args,
                *tail,
                *pos,
                file.as_deref(),
                *site,
                frame_base,
                genv,
            )?;
            force(heap, step)
        }
        Node::Prim1 {
            op,
            a,
            head,
            guard,
            pos,
        } => {
            let pos = *pos;
            let tag = |e: LispError| match pos {
                Some(p) => e.or_pos(p),
                None => e,
            };
            let sa = exec_value(heap, a, frame_base, genv).map_err(tag)?;
            // Inline only while `head` still resolves to `op` (epoch-guarded, as
            // in `Prim2`). The inline cases read a slab cell and run no further
            // eval, so the operand needs no rooting here.
            let cur = heap.global_epoch();
            let inlinable = if guard.load(Ordering::Relaxed) == cur {
                true
            } else {
                match resolve_prim1(heap, *head) {
                    Some(op2) if op2 == *op => {
                        guard.store(cur, Ordering::Relaxed);
                        true
                    }
                    _ => false,
                }
            };
            if inlinable {
                match (op, sa.unpack()) {
                    (PrimOp1::First, ValueRef::Pair(p)) => {
                        crate::perf_bump!(prim1_inline);
                        return Ok(heap.pair(p).0);
                    }
                    (PrimOp1::Rest, ValueRef::Pair(p)) => {
                        crate::perf_bump!(prim1_inline);
                        return Ok(heap.pair(p).1);
                    }
                    (PrimOp1::First | PrimOp1::Rest, ValueRef::Nil) => {
                        crate::perf_bump!(prim1_inline);
                        return Ok(Value::nil());
                    }
                    (PrimOp1::IsEmpty, ValueRef::Nil) => {
                        crate::perf_bump!(prim1_inline);
                        return Ok(Value::boolean(true));
                    }
                    (PrimOp1::IsEmpty, ValueRef::Pair(_) | ValueRef::Range(_)) => {
                        crate::perf_bump!(prim1_inline);
                        return Ok(Value::boolean(false));
                    }
                    // `type-of` is total: tag → cached keyword, every operand shape.
                    (PrimOp1::TypeOf, _) => {
                        crate::perf_bump!(prim1_inline);
                        return Ok(Value::keyword(crate::core::value::tag(sa).keyword()));
                    }
                    _ => {} // vectors/ranges/type errors → the native owns them
                }
            }
            crate::perf_bump!(prim1_fallback);
            // Fallback: a general call on the surface operator (rooted across
            // the dispatch, which can collect).
            let save = heap.roots_len();
            heap.push_root(sa);
            let cur_env = heap.read_root_env(genv);
            let callee = match heap.env_get(cur_env, *head) {
                Some(c) => c,
                None => {
                    heap.truncate_roots(save);
                    return Err(tag(crate::eval::unbound_error(heap, *head)));
                }
            };
            let sa = heap.root_at(save);
            let argv: SmallVec<[Value; 4]> = SmallVec::from_slice(&[sa]);
            let result = dispatch(heap, callee, argv, false, cur_env).and_then(|s| force(heap, s));
            heap.truncate_roots(save);
            result.map_err(tag)
        }
        Node::Prim2 {
            op,
            a,
            b,
            map,
            head,
            guard,
            pos,
            broot,
        } => {
            let pos = *pos;
            let tag = |e: LispError| match pos {
                Some(p) => e.or_pos(p),
                None => e,
            };
            // Evaluate operands in source order. `a`'s value is rooted across
            // `b`'s eval only when `b` can reach a safepoint (`broot` — see the
            // field doc); the common pure-leaf shape runs root-free, since the
            // inline path below touches no safepoint either. The fallback
            // dispatch roots both regardless. `save` is always truncated back.
            let save = heap.roots_len();
            let sa = match exec_value(heap, a, frame_base, genv) {
                Ok(v) => v,
                Err(e) => return Err(tag(e)),
            };
            if *broot {
                heap.push_root(sa);
            }
            let sb = match exec_value(heap, b, frame_base, genv) {
                Ok(v) => v,
                Err(e) => {
                    heap.truncate_roots(save);
                    return Err(tag(e));
                }
            };
            // Re-read `a` post-collection (a no-op unless it was rooted), then
            // route to the primitive's argument order. `b` ran no further eval,
            // so its value is current as-is.
            let sa = if *broot { heap.root_at(save) } else { sa };
            let src = [sa, sb];
            let x = src[map[0] as usize];
            let y = src[map[1] as usize];
            // Inline only while `head` still resolves to `op` (epoch-guarded). A
            // redefinition bumps `global_epoch`, forcing one re-validate; if it no
            // longer maps to the primitive we drop to the general fallback below.
            let cur = heap.global_epoch();
            let inlinable = if guard.load(Ordering::Relaxed) == cur {
                true
            } else {
                match resolve_prim(heap, *head) {
                    Some((op2, m2)) if op2 == *op && m2 == [map[0] as usize, map[1] as usize] => {
                        guard.store(cur, Ordering::Relaxed);
                        true
                    }
                    _ => false,
                }
            };
            if inlinable {
                match prim_apply(*op, x, y) {
                    Ok(Some(v)) => {
                        crate::perf_bump!(prim2_inline);
                        heap.truncate_roots(save);
                        return Ok(v);
                    }
                    // `prim_apply` is heap-less, so it always defers `cons`
                    // (which allocates) — inline it here, off the numeric ops'
                    // hot path. It accepts any operands: never defers on shape.
                    Ok(None) if *op == PrimOp::Cons => {
                        crate::perf_bump!(prim2_inline);
                        let v = heap.alloc_pair(x, y);
                        heap.truncate_roots(save);
                        return Ok(v);
                    }
                    Ok(None) => {} // non-inline operand shape → defer to the real primitive
                    Err(e) => {
                        heap.truncate_roots(save);
                        return Err(tag(e));
                    }
                }
            }
            crate::perf_bump!(prim2_fallback);
            // Fallback: call the surface operator on the source-order operands,
            // exactly as the generic call path would — covers a redefined
            // operator and every non-inline operand shape, with identical
            // semantics. Root both operands first (the dispatch can collect);
            // `sa` may already hold the slot at `save`.
            if !*broot {
                heap.push_root(sa);
            }
            heap.push_root(sb);
            let cur_env = heap.read_root_env(genv);
            let callee = match heap.env_get(cur_env, *head) {
                Some(c) => c,
                None => {
                    heap.truncate_roots(save);
                    return Err(tag(crate::eval::unbound_error(heap, *head)));
                }
            };
            let argv: SmallVec<[Value; 4]> = SmallVec::from_slice(&[sa, sb]);
            let result = dispatch(heap, callee, argv, false, cur_env).and_then(|s| force(heap, s));
            heap.truncate_roots(save);
            result.map_err(tag)
        }
        Node::Prim3 {
            a, b, c, head, pos, ..
        } => {
            let pos = *pos;
            let tag = |e: LispError| match pos {
                Some(p) => e.or_pos(p),
                None => e,
            };
            // Cold path (optional defaults & co.): evaluate the three operands in
            // source order, rooting each across the later evals (which can reach a
            // safepoint), then dispatch `head` exactly like the generic call path —
            // identical semantics for every operand shape and for a redefined head.
            let save = heap.roots_len();
            let sa = match exec_value(heap, a, frame_base, genv) {
                Ok(v) => v,
                Err(e) => return Err(tag(e)),
            };
            heap.push_root(sa);
            let sb = match exec_value(heap, b, frame_base, genv) {
                Ok(v) => v,
                Err(e) => {
                    heap.truncate_roots(save);
                    return Err(tag(e));
                }
            };
            heap.push_root(sb);
            let sc = match exec_value(heap, c, frame_base, genv) {
                Ok(v) => v,
                Err(e) => {
                    heap.truncate_roots(save);
                    return Err(tag(e));
                }
            };
            heap.push_root(sc);
            let cur_env = heap.read_root_env(genv);
            let callee = match heap.env_get(cur_env, *head) {
                Some(cv) => cv,
                None => {
                    heap.truncate_roots(save);
                    return Err(tag(crate::eval::unbound_error(heap, *head)));
                }
            };
            let argv: SmallVec<[Value; 4]> = SmallVec::from_slice(&[
                heap.root_at(save),
                heap.root_at(save + 1),
                heap.root_at(save + 2),
            ]);
            let result = dispatch(heap, callee, argv, false, cur_env).and_then(|s| force(heap, s));
            heap.truncate_roots(save);
            result.map_err(tag)
        }
        Node::TryCatch {
            body,
            bind_slot,
            handler,
        } => match exec_value(heap, body, frame_base, genv) {
            Ok(v) => Ok(v),
            Err(e) if e.is_control() => Err(e),
            Err(e) => {
                let caught = match e.payload {
                    Some(v) => v,
                    None => e.to_value_map(heap),
                };
                heap.set_root_at(frame_base + bind_slot, caught);
                exec_value(heap, handler, frame_base, genv)
            }
        },
    }
}
