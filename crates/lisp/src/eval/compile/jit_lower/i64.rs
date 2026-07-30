//! The unboxed scalar (`i64`/`f64`) register worker — the JIT's fast tier for
//! arithmetic/comparison arms that stay in unboxed scalars (overflow-checked i64
//! with deopt-to-BigInt, or f64). Split out of `jit_lower.rs`: this cluster is a
//! self-contained set of functions used only by the `jit_lower_arm` dispatcher (it
//! never touches `jit_lower_arm_inner`), so it moves cleanly. Reaches the parent's
//! items (IR types, `jit_i64_enabled`, subset helpers, cranelift) via `use super::*`.
use super::*;

/// The scalar the unboxed register worker specializes to. `Int` (i64, overflow-checked → deopt
/// to BigInt) or `Float` (f64, no overflow — IEEE inf/NaN are valid float results).
#[cfg(feature = "jit")]
#[derive(Clone, Copy, PartialEq)]
enum Scalar {
    Int,
    Float,
}

#[cfg(feature = "jit")]
impl Scalar {
    fn clif(self) -> cranelift_codegen::ir::Type {
        match self {
            Scalar::Int => cranelift_codegen::ir::types::I64,
            Scalar::Float => cranelift_codegen::ir::types::F64,
        }
    }
    fn tag(self) -> u8 {
        match self {
            Scalar::Int => crate::core::value::jit_layout::TAG_INT,
            Scalar::Float => crate::core::value::jit_layout::TAG_FLOAT,
        }
    }
}

/// Which unboxed scalar (if any) this arm's register worker can specialize to: `Int` if the body
/// is an int-only recursive subset, `Float` if float-only, else `None` (use the boxed path). The
/// base-case threshold const (`(< x 2)` vs `(< x 2.0)`) pins the kind, so a terminating recursion
/// is never ambiguous; a mixed-type body matches neither and stays boxed. Gate + single-or-more
/// fixed args + no-capture + top-level (`dbg_name`) + recursive + not-previously-too-deep.
#[cfg(feature = "jit")]
fn arm_scalar_kind(arm: &CompiledArm) -> Option<Scalar> {
    if !jit_i64_enabled()
        || arm.nrequired < 1
        || arm.noptional != 0
        || arm.rest_slot.is_some()
        || !arm.capture_names.is_empty()
    {
        return None;
    }
    // Use `dbg_name` (every top-level defn has it) rather than `inline_name` (set only when the
    // arm ALSO qualifies for the depth-2 inliner — which excludes e.g. Ackermann, whose inlined
    // expansion is too big). The worker needs no inlining; it just needs the self symbol.
    let self_sym = arm.dbg_name?;
    // A prior depth-bail switched this fn to the boxed path (which drains deep recursion).
    if i64_too_deep(self_sym) || !i64_has_self_call(&arm.body) {
        return None;
    }
    // The worker lowers a *non-tail* `(f …)` whose head is `dbg_name` to a direct call to
    // itself. `dbg_name` is only the symbol this closure was first `def`'d under, so that
    // is sound only while the global still binds THIS arm — see `self_global_ok`, which is
    // re-observed at every tiering election. Without the check, aliasing the closure and
    // rebinding the name kept the old body calling itself: `(def f h)`, `(def h …)`,
    // `(f 12)` answered 12 where the VM and tree-walker both answered 1001.
    if !arm
        .self_global_ok
        .load(std::sync::atomic::Ordering::Relaxed)
    {
        return None;
    }
    let empty = std::collections::HashSet::new();
    [Scalar::Int, Scalar::Float]
        .into_iter()
        .find(|&kind| i64_value_ok(&arm.body, self_sym, arm.nrequired, &empty, kind))
}

/// Does this arm take an unboxed register worker? [`jit_tier`] consults this to **skip the
/// two-stage inline upgrade** for such arms — the worker already recurses in registers to full
/// depth, so the boxed depth-2 upgrade (which would swap out the worker) is inferior.
#[cfg(feature = "jit")]
pub(crate) fn arm_i64_eligible(arm: &CompiledArm) -> bool {
    arm_scalar_kind(arm).is_some()
}

/// Does `node` contain a self-`Call`? In the i64-validated subset every `Node::Call` is a
/// single-/multi-arg self-call, so scanning for one is enough to confirm the arm recurses.
#[cfg(feature = "jit")]
fn i64_has_self_call(node: &Node) -> bool {
    match node {
        // A *non-tail* self-call is the "genuinely recursive, wins from a register frame" signal.
        Node::Call { .. } => true,
        // A tail self-call is a loop, NOT the recursion signal — so `SelfCall` itself doesn't
        // count (a pure-tail-recursive int fn like `loop`/`collatz` must stay on the faster
        // self-tail-loop path, not divert to this recursive worker). But we DO recurse into its
        // args, because a genuine non-tail `Call` can be nested there — Ackermann's inner
        // `(ack m (- k 1))` sits inside the outer tail `(ack (- m 1) …)`'s arguments.
        Node::SelfCall { args, .. } => args.iter().any(i64_has_self_call),
        Node::If(a, b, c) => i64_has_self_call(a) || i64_has_self_call(b) || i64_has_self_call(c),
        Node::Prim2 { a, b, .. } => i64_has_self_call(a) || i64_has_self_call(b),
        Node::LetBind { binds, body } => {
            binds.iter().any(|(_, r)| i64_has_self_call(r)) || i64_has_self_call(body)
        }
        Node::Do(xs) => xs.iter().any(i64_has_self_call),
        _ => false,
    }
}

// The i64 worker's native-recursion **frame-count** cap (`I64_DEPTH_LIMIT`, last valued at
// 32 768) lived here and is gone as of 2026-07-27. It bounded the native stack the register
// recursion runs on by counting levels, which is only ever right for one frame size — the
// KI-14 abort was exactly a frame size it was wrong for. The worker's byte-based stack guard
// (`jit_lower_arm_i64`'s prologue, comparing the frame address against `Heap::jit_stack_limit`)
// measures the resource that actually runs out and therefore subsumes it, and dropping the
// second per-level compare recovered the ~18% the guard had cost `fib`. The depth-bail
// *sentinel* (`2`) it used to raise is unchanged — the byte guard raises the same one, so the
// wrapper still returns outcome 5 and `jit_tier` still switches the arm permanently to the
// boxed path (without that switch a deep non-tail recursion deopt-and-re-tiers per level, a
// ~100× thrash). The `depth` parameter is still threaded: it costs nothing measurable and the
// bail diagnostics read it.

/// Functions (by defining-`defn` name) that a depth-bail proved are too deeply recursive for the
/// register worker — they run the boxed path instead (which drains gracefully). Process-global
/// and monotonic (a name only ever gets added), so `arm_i64_eligible` reads it lock-free-ish and
/// the switch is stable. Keyed by `dbg_name` since that's the stable identity across an arm's
/// recompiles / the shared-JIT cache.
#[cfg(feature = "jit")]
static I64_TOO_DEEP: std::sync::Mutex<Option<std::collections::HashSet<Symbol>>> =
    std::sync::Mutex::new(None);

/// Record that `sym`'s recursion overflowed the i64 worker's depth cap — switch it to boxed.
#[cfg(feature = "jit")]
pub(crate) fn i64_mark_too_deep(sym: Symbol) {
    if let Ok(mut g) = I64_TOO_DEEP.lock() {
        g.get_or_insert_with(std::collections::HashSet::new)
            .insert(sym);
    }
}

/// Has `sym` been marked too-deep for the i64 worker?
#[cfg(feature = "jit")]
fn i64_too_deep(sym: Symbol) -> bool {
    match I64_TOO_DEEP.lock() {
        Ok(g) => g.as_ref().is_some_and(|s| s.contains(&sym)),
        Err(_) => false,
    }
}

/// Is this arm one the i64 worker gave up on (depth-bail)? Consulted by `jit_tier`'s shared-JIT
/// install so a stale shared i64 wrapper isn't re-installed for a too-deep function.
#[cfg(feature = "jit")]
pub(crate) fn arm_i64_too_deep(arm: &CompiledArm) -> bool {
    arm.dbg_name.is_some_and(i64_too_deep)
}

/// Is `op` an arithmetic op the worker lowers to a value, for scalar `kind`? Int: Add/Sub/Mul
/// (overflow-checked → deopt to BigInt), Min/Max, Rem/Quot/Div (all guarded — an inexact Div
/// deopts too, since the VM's `/` returns a Float in that case and the int worker can't produce
/// one), bitops. Float: Add/Sub/Mul (no overflow — IEEE) and Div; NOT Min/Max (Cranelift
/// `fmin`/`fmax` NaN semantics differ from the VM's) nor the int-only ops (rem/quot/bitwise are
/// int-domain).
#[cfg(feature = "jit")]
fn i64_arith_op(kind: Scalar, op: PrimOp) -> bool {
    match kind {
        Scalar::Int => matches!(
            op,
            PrimOp::Add
                | PrimOp::Sub
                | PrimOp::Mul
                | PrimOp::Min
                | PrimOp::Max
                | PrimOp::Rem
                | PrimOp::Quot
                | PrimOp::Div
                | PrimOp::BitAnd
                | PrimOp::BitOr
                | PrimOp::BitXor
        ),
        Scalar::Float => matches!(op, PrimOp::Add | PrimOp::Sub | PrimOp::Mul | PrimOp::Div),
    }
}

/// Is `op` an integer comparison the i64 worker lowers to a 0/1 condition?
#[cfg(feature = "jit")]
fn i64_cmp_op(op: PrimOp) -> bool {
    matches!(op, PrimOp::Lt | PrimOp::Le | PrimOp::Eq)
}

/// Is this `Call` a `(throw <expr>)` on the **global** `throw` (not shadowed by the arm's
/// own name)? The i64 worker lowers it as: evaluate the payload in registers, then a
/// `brood_rt_i64_throw` callback that parks the error (or deopts if `throw` was redefined —
/// late binding wins) + the sentinel unwind. This is what lets an error-raising deep
/// recursion (`errors-deep`'s `descend`) keep its register worker instead of falling back
/// to the interpreted frame-build path.
#[cfg(feature = "jit")]
fn i64_throw_call<'a>(callee: &Node, args: &'a [Node], self_sym: Symbol) -> Option<&'a Node> {
    match callee {
        Node::Global(s) | Node::GlobalIc { sym: s, .. }
            if *s != self_sym && *s == crate::core::value::intern("throw") && args.len() == 1 =>
        {
            Some(&args[0])
        }
        _ => None,
    }
}

/// Non-mutating check: is `node` a value-position expression in the i64 worker's subset?
/// (int `Const`, param `Local(0)`, int arith `Prim2`, a single-arg self-`Call`, or an `If`
/// whose cond is a comparison and whose branches are values.) Anything else bails the whole
/// i64 lowering (the arm then uses the general boxed path).
#[cfg(feature = "jit")]
fn i64_value_ok(
    node: &Node,
    self_sym: Symbol,
    nargs: usize,
    bound: &std::collections::HashSet<usize>,
    kind: Scalar,
) -> bool {
    match node {
        // A const must match the worker's scalar (`as_int`/`as_f64` are `Some` only for that
        // kind), so a mixed-type body matches neither kind and stays boxed.
        Node::Const(ConstVal::Atom(v)) => match kind {
            Scalar::Int => v.as_int().is_some(),
            Scalar::Float => v.as_f64().is_some(),
        },
        // A param slot, or a `let` binder already in scope (a forward/unbound read bails —
        // the worker carries binders in SSA vars that must be def'd before use).
        Node::Local(k) => *k < nargs || bound.contains(k),
        Node::Prim2 { op, a, b, .. } if i64_arith_op(kind, *op) => {
            i64_value_ok(a, self_sym, nargs, bound, kind)
                && i64_value_ok(b, self_sym, nargs, bound, kind)
        }
        Node::If(c, t, e) => {
            i64_cond_ok(c, self_sym, nargs, bound, kind)
                && i64_value_ok(t, self_sym, nargs, bound, kind)
                && i64_value_ok(e, self_sym, nargs, bound, kind)
        }
        // `(throw <payload>)` — accepted in ANY position (tail or not; it never returns).
        // The payload must be in-subset for this worker's scalar (an int payload for an Int
        // worker, float for Float), so the register value can box straight into the error.
        Node::Call { callee, args, .. }
            if i64_throw_call(callee, args, self_sym)
                .is_some_and(|p| i64_value_ok(p, self_sym, nargs, bound, kind)) =>
        {
            true
        }
        Node::Call {
            callee,
            args,
            tail: false,
            ..
        } => {
            args.len() == nargs
                && matches!(&**callee, Node::Global(s) | Node::GlobalIc { sym: s, .. } if *s == self_sym)
                && args
                    .iter()
                    .all(|a| i64_value_ok(a, self_sym, nargs, bound, kind))
        }
        // A tail self-call (`SelfCall`) — always to self with exactly `nargs` args (its `compile_arm`
        // gate rules out `&optional`/`&rest`). Lowered like a non-tail self-`Call`: the register
        // worker recurses natively (no tail-loop), so a mixed tail+non-tail recursion (Ackermann)
        // rides registers instead of falling to the boxed path. Each arg must be in-subset.
        Node::SelfCall { args, .. } => {
            args.len() == nargs
                && args
                    .iter()
                    .all(|a| i64_value_ok(a, self_sym, nargs, bound, kind))
        }
        // `let`/`let*`: each rhs must be in-subset in the scope built so far (so a `letrec`
        // forward-ref bails), then its slot joins the scope for later binds + the body.
        Node::LetBind { binds, body } => {
            let mut scope = bound.clone();
            for (slot, rhs) in binds.iter() {
                if !i64_value_ok(rhs, self_sym, nargs, &scope, kind) {
                    return false;
                }
                scope.insert(*slot);
            }
            i64_value_ok(body, self_sym, nargs, &scope, kind)
        }
        // `do`: pure in this subset, so only the last form's value matters (the worker lowers
        // just that) — but validate every form is in-subset (else the whole arm bails).
        Node::Do(xs) => {
            !xs.is_empty()
                && xs
                    .iter()
                    .all(|x| i64_value_ok(x, self_sym, nargs, bound, kind))
        }
        _ => false,
    }
}

/// Non-mutating check: is `node` a condition (comparison) in the worker's subset for `kind`?
#[cfg(feature = "jit")]
fn i64_cond_ok(
    node: &Node,
    self_sym: Symbol,
    nargs: usize,
    bound: &std::collections::HashSet<usize>,
    kind: Scalar,
) -> bool {
    matches!(node, Node::Prim2 { op, a, b, .. }
        if i64_cmp_op(*op) && i64_value_ok(a, self_sym, nargs, bound, kind) && i64_value_ok(b, self_sym, nargs, bound, kind))
}

/// Shared context threaded through the i64 worker's recursive lowering.
#[cfg(feature = "jit")]
struct I64Ctx {
    kind: Scalar,     // Int (i64) or Float (f64) — selects const/arith/cmp/box lowering
    self_sym: Symbol, // the arm's own defn name (distinguishes a self-call from a `throw` call)
    self_ref: cranelift_codegen::ir::FuncRef,
    throw_ref: cranelift_codegen::ir::FuncRef, // brood_rt_i64_throw (park error / deopt)
    params: Vec<cranelift_codegen::ir::Value>, // the arm's `nargs` params (`Local(k)`)
    // `let` binder slots → their SSA variable (index = frame slot; `None` for a param slot).
    // A `Local(k)` with `k >= nargs` reads `use_var(slot_vars[k])`; a `LetBind` `def_var`s it.
    slot_vars: Vec<Option<cranelift_frontend::Variable>>,
    depth: cranelift_codegen::ir::Value, // this activation's depth
    ovf: cranelift_codegen::ir::Value,   // *mut u8 overflow sentinel
    heap: cranelift_codegen::ir::Value,  // *mut Heap — only used by the throw callback
    poisoned: cranelift_codegen::ir::Block, // shared unwind target (returns 0)
}

/// On a signed-overflow flag `ov`: set the overflow sentinel and jump the shared `poisoned`
/// unwind block; otherwise fall through. Leaves `b` switched to the fall-through block.
#[cfg(feature = "jit")]
fn i64_guard_overflow(
    b: &mut cranelift_frontend::FunctionBuilder,
    cx: &I64Ctx,
    ov: cranelift_codegen::ir::Value,
) {
    use cranelift_codegen::ir::{types, InstBuilder, MemFlagsData};
    let ovset = b.create_block();
    let cont = b.create_block();
    b.ins().brif(ov, ovset, &[], cont, &[]);
    b.seal_block(ovset);
    b.seal_block(cont);
    b.switch_to_block(ovset);
    let one = b.ins().iconst(types::I8, 1);
    b.ins().store(MemFlagsData::trusted(), one, cx.ovf, 0);
    b.ins().jump(cx.poisoned, &[]);
    b.switch_to_block(cont);
}

/// Lower an integer arithmetic op on two `i64` SSA operands `(x, y)`. Add/Sub/Mul are
/// overflow-checked (→ set sentinel + unwind, so the wrapper deopts to the VM → BigInt);
/// Min/Max are exact selects. Leaves `b` at the post-check block; the result is live there.
#[cfg(feature = "jit")]
fn lower_i64_arith(
    b: &mut cranelift_frontend::FunctionBuilder,
    cx: &I64Ctx,
    op: PrimOp,
    x: cranelift_codegen::ir::Value,
    y: cranelift_codegen::ir::Value,
) -> cranelift_codegen::ir::Value {
    use cranelift_codegen::ir::{condcodes::IntCC, InstBuilder};
    // Float: plain IEEE ops, no *overflow* (inf/NaN are valid float results) — far simpler
    // than int. Division still needs the ÷0 guard: Brood's `/` **raises** "division by
    // zero" rather than yielding IEEE infinity (`prim_div` tests `b == 0.0` before
    // dividing), so a bare `fdiv` returned `inf` where the VM raised — a JIT-only wrong
    // answer. `fcmp Equal` against 0.0 is true for -0.0 too, matching `b == 0.0` exactly.
    if cx.kind == Scalar::Float {
        return match op {
            PrimOp::Add => b.ins().fadd(x, y),
            PrimOp::Sub => b.ins().fsub(x, y),
            PrimOp::Mul => b.ins().fmul(x, y),
            PrimOp::Div => {
                let zero = b.ins().f64const(0.0);
                let div0 = b
                    .ins()
                    .fcmp(cranelift_codegen::ir::condcodes::FloatCC::Equal, y, zero);
                i64_guard_overflow(b, cx, div0);
                b.ins().fdiv(x, y)
            }
            _ => unreachable!("float checker restricts arith ops to +,-,*,/"),
        };
    }
    match op {
        PrimOp::Add => {
            let (r, ov) = b.ins().sadd_overflow(x, y);
            i64_guard_overflow(b, cx, ov);
            r
        }
        PrimOp::Sub => {
            let (r, ov) = b.ins().ssub_overflow(x, y);
            i64_guard_overflow(b, cx, ov);
            r
        }
        PrimOp::Mul => {
            let (r, ov) = b.ins().smul_overflow(x, y);
            i64_guard_overflow(b, cx, ov);
            r
        }
        PrimOp::Max => {
            let c = b.ins().icmp(IntCC::SignedGreaterThanOrEqual, x, y);
            b.ins().select(c, x, y)
        }
        PrimOp::Min => {
            let c = b.ins().icmp(IntCC::SignedLessThanOrEqual, x, y);
            b.ins().select(c, x, y)
        }
        // `rem`/`quot`/`div`: `sdiv`/`srem` TRAP on a zero divisor and on `i64::MIN / -1`, so guard
        // both → sentinel + unwind (the wrapper deopts; the VM raises the ÷0 error / does the
        // edge, staying bit-identical). Reuses `i64_guard_overflow` with the bail condition.
        PrimOp::Rem | PrimOp::Quot | PrimOp::Div => {
            let zero = b.ins().iconst(cranelift_codegen::ir::types::I64, 0);
            let div0 = b.ins().icmp(IntCC::Equal, y, zero);
            i64_guard_overflow(b, cx, div0);
            let min = b.ins().iconst(cranelift_codegen::ir::types::I64, i64::MIN);
            let neg1 = b.ins().iconst(cranelift_codegen::ir::types::I64, -1);
            let x_min = b.ins().icmp(IntCC::Equal, x, min);
            let y_m1 = b.ins().icmp(IntCC::Equal, y, neg1);
            let ov = b.ins().band(x_min, y_m1);
            i64_guard_overflow(b, cx, ov);
            match op {
                PrimOp::Rem => b.ins().srem(x, y),
                PrimOp::Quot => b.ins().sdiv(x, y),
                // `/` on two ints is an Int result only when it divides evenly (matching
                // `prim_apply`'s inline fast path, `compile/mod.rs`); a nonzero remainder
                // means the VM would build a Float, which this worker can't return — guard
                // it as inexact and deopt (the VM recomputes with full generality).
                PrimOp::Div => {
                    let r = b.ins().srem(x, y);
                    let inexact = b.ins().icmp(IntCC::NotEqual, r, zero);
                    i64_guard_overflow(b, cx, inexact);
                    b.ins().sdiv(x, y)
                }
                _ => unreachable!(),
            }
        }
        PrimOp::BitAnd => b.ins().band(x, y),
        PrimOp::BitOr => b.ins().bor(x, y),
        PrimOp::BitXor => b.ins().bxor(x, y),
        _ => unreachable!("i64 checker restricts arith ops"),
    }
}

/// Lower a value-position node of the i64 subset to an `i64` SSA value. Leaves `b` switched
/// to the block where the returned value is live. Pre-validated by [`i64_value_ok`].
#[cfg(feature = "jit")]
fn lower_i64_value(
    b: &mut cranelift_frontend::FunctionBuilder,
    cx: &I64Ctx,
    node: &Node,
) -> cranelift_codegen::ir::Value {
    use cranelift_codegen::ir::{types, InstBuilder, MemFlagsData};
    match node {
        Node::Const(ConstVal::Atom(v)) => match cx.kind {
            Scalar::Int => b.ins().iconst(types::I64, v.as_int().expect("int const")),
            Scalar::Float => b.ins().f64const(v.as_f64().expect("float const")),
        },
        Node::Local(k) => match cx.slot_vars[*k] {
            Some(var) => b.use_var(var), // a `let` binder
            None => cx.params[*k],       // a param
        },
        Node::Prim2 {
            op, a, b: bn, map, ..
        } => {
            let va = lower_i64_value(b, cx, a);
            let vb = lower_i64_value(b, cx, bn);
            let (x, y) = if map[0] == 0 { (va, vb) } else { (vb, va) };
            lower_i64_arith(b, cx, *op, x, y)
        }
        Node::LetBind { binds, body } => {
            // Evaluate each rhs in order and write it to its binder's SSA var (sequential
            // let/let*; forward-refs were rejected by the checker), then lower the body.
            for (slot, rhs) in binds.iter() {
                let v = lower_i64_value(b, cx, rhs);
                b.def_var(cx.slot_vars[*slot].expect("let binder var"), v);
            }
            lower_i64_value(b, cx, body)
        }
        // `do`: lower EVERY form, not just the last — the subset is pure except `throw`,
        // whose side effect (raising) must fire from a non-final position too. The dead
        // pure values cost nothing (Cranelift DCEs them).
        Node::Do(xs) => {
            let mut last = None;
            for x in xs.iter() {
                last = Some(lower_i64_value(b, cx, x));
            }
            last.expect("non-empty do")
        }
        Node::If(c, t, e) => {
            let cond = lower_i64_cond(b, cx, c);
            let then_b = b.create_block();
            let else_b = b.create_block();
            let merge = b.create_block();
            let rv = b.declare_var(cx.kind.clif());
            b.ins().brif(cond, then_b, &[], else_b, &[]);
            b.seal_block(then_b);
            b.seal_block(else_b);
            b.switch_to_block(then_b);
            let tv = lower_i64_value(b, cx, t);
            b.def_var(rv, tv);
            b.ins().jump(merge, &[]);
            b.switch_to_block(else_b);
            let ev = lower_i64_value(b, cx, e);
            b.def_var(rv, ev);
            b.ins().jump(merge, &[]);
            b.seal_block(merge);
            b.switch_to_block(merge);
            b.use_var(rv)
        }
        // `(throw <payload>)` (checker-verified via `i64_throw_call`): evaluate the payload in
        // registers, then the callback parks the thrown error (returning sentinel 3) — or 1
        // (deopt) if `throw` was redefined, so late binding stays exact — and the worker unwinds
        // through `poisoned` like an overflow. The dead continuation block keeps the
        // value-position contract (this node never produces a value at runtime).
        Node::Call { callee, args, .. } if i64_throw_call(callee, args, cx.self_sym).is_some() => {
            let payload = i64_throw_call(callee, args, cx.self_sym).expect("checked throw");
            let x = lower_i64_value(b, cx, payload);
            let bits = match cx.kind {
                Scalar::Int => x,
                Scalar::Float => b.ins().bitcast(types::I64, MemFlagsData::new(), x),
            };
            let isf = b
                .ins()
                .iconst(types::I64, (cx.kind == Scalar::Float) as i64);
            let call = b.ins().call(cx.throw_ref, &[cx.heap, bits, isf]);
            let sentinel = b.inst_results(call)[0];
            let s8 = b.ins().ireduce(types::I8, sentinel);
            b.ins().store(MemFlagsData::trusted(), s8, cx.ovf, 0);
            b.ins().jump(cx.poisoned, &[]);
            let dead = b.create_block();
            b.switch_to_block(dead);
            b.seal_block(dead);
            match cx.kind {
                Scalar::Int => b.ins().iconst(types::I64, 0),
                Scalar::Float => b.ins().f64const(0.0),
            }
        }
        // Both a non-tail self-`Call` (fib's argument-position recursion) and a tail `SelfCall`
        // (Ackermann's cond-branch recursion) lower the same way here: the register worker has no
        // tail-loop, so a tail call recurses natively just like a non-tail one (bounded by the
        // depth cap → deopt). `SelfCall` always targets self with exactly `nargs` args.
        Node::Call { args, .. } | Node::SelfCall { args, .. } => {
            // A self-call (checker-verified: `nargs` args, head == self). Register calling
            // convention: pass the args + depth+1 + the sentinel; no boxing / roots-staging /
            // fast-link dispatch. Lower every arg BEFORE the call (they read `params`, which the
            // call can't disturb — no memory frame).
            let mut call_args: Vec<cranelift_codegen::ir::Value> =
                args.iter().map(|a| lower_i64_value(b, cx, a)).collect();
            call_args.push(b.ins().iadd_imm(cx.depth, 1));
            call_args.push(cx.ovf);
            call_args.push(cx.heap);
            let call = b.ins().call(cx.self_ref, &call_args);
            let r = b.inst_results(call)[0];
            // If the callee (or a deeper level, or a depth-cap bail) set the sentinel, unwind
            // now — bounds the post-overflow unwind to O(depth) instead of O(2^depth).
            let o = b.ins().load(types::I8, MemFlagsData::trusted(), cx.ovf, 0);
            let cont = b.create_block();
            b.ins().brif(o, cx.poisoned, &[], cont, &[]);
            b.seal_block(cont);
            b.switch_to_block(cont);
            r
        }
        _ => unreachable!("i64 checker guarantees the value subset"),
    }
}

/// Lower a condition node (a comparison) to an `i1`. Pre-validated by [`i64_cond_ok`].
#[cfg(feature = "jit")]
fn lower_i64_cond(
    b: &mut cranelift_frontend::FunctionBuilder,
    cx: &I64Ctx,
    node: &Node,
) -> cranelift_codegen::ir::Value {
    use cranelift_codegen::ir::{
        condcodes::{FloatCC, IntCC},
        InstBuilder,
    };
    match node {
        Node::Prim2 {
            op, a, b: bn, map, ..
        } => {
            let va = lower_i64_value(b, cx, a);
            let vb = lower_i64_value(b, cx, bn);
            let (x, y) = if map[0] == 0 { (va, vb) } else { (vb, va) };
            match cx.kind {
                Scalar::Int => {
                    let cc = match op {
                        PrimOp::Lt => IntCC::SignedLessThan,
                        PrimOp::Le => IntCC::SignedLessThanOrEqual,
                        PrimOp::Eq => IntCC::Equal,
                        _ => unreachable!("checker restricts cmp ops"),
                    };
                    b.ins().icmp(cc, x, y)
                }
                // Ordered float compares (NaN → false), matching the VM's Rust `<`/`<=`/`==`.
                Scalar::Float => {
                    let cc = match op {
                        PrimOp::Lt => FloatCC::LessThan,
                        PrimOp::Le => FloatCC::LessThanOrEqual,
                        PrimOp::Eq => FloatCC::Equal,
                        _ => unreachable!("checker restricts cmp ops"),
                    };
                    b.ins().fcmp(cc, x, y)
                }
            }
        }
        _ => unreachable!("checker guarantees a comparison cond"),
    }
}

/// Lower an int-only single-arg recursive arm (`fib`) to an unboxed-`i64` register worker +
/// a thin boxed wrapper (the arm's actual entry). Returns the wrapper pointer, or `None` if
/// the arm isn't eligible / not in the subset (fall back to the general boxed lowering).
///
/// The **worker** `fn(n: i64, depth: i64, ovf: *mut u8) -> i64` recurses with register args
/// (no heap, no roots, no GC — an i64 isn't a handle), overflow-checked; on overflow or the
/// depth cap it sets `*ovf` and unwinds. The **wrapper** `fn(heap, base) -> outcome` reads the
/// arg `Value` from `roots[base]`; if it isn't an `Int` → outcome 1 (VM handles); else clears
/// `*ovf`, calls the worker, and either deopts (outcome 1 → VM recomputes with BigInt) if the
/// sentinel is set, or boxes the `i64` result into `roots[base]` and returns 0 (Done).
#[cfg(feature = "jit")]
pub(super) fn jit_lower_i64_arm(jit: &mut crate::jit::Jit, arm: &CompiledArm) -> Option<*const u8> {
    use crate::core::value::jit_layout::PAYLOAD_OFFSET;
    use cranelift_codegen::ir::{condcodes::IntCC, types, AbiParam, InstBuilder, MemFlagsData};
    use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
    use cranelift_module::{Linkage, Module};
    use std::sync::atomic::Ordering;

    // Eligibility + which scalar (Int/Float) this arm's worker specializes to.
    let kind = arm_scalar_kind(arm)?;
    let sty = kind.clif(); // i64 or f64 — the worker's arg/result register type
    let body = &arm.body;
    let nargs = arm.nrequired;
    let self_sym = arm.dbg_name?; // present — arm_scalar_kind already required it

    const STRIDE: i64 = std::mem::size_of::<Value>() as i64;
    let m = jit.module();
    let ptr_ty = m.target_config().pointer_type();
    let seq = JIT_ARM_SEQ.fetch_add(1, Ordering::Relaxed);

    // Signatures. Worker: (a0..a_{nargs-1}: sty, depth: i64, ovf: *mut u8, heap: *mut Heap) -> sty.
    // `heap` rides along untouched except by a `throw` lowering (its callback parks the error).
    let mut wsig = m.make_signature();
    for _ in 0..nargs {
        wsig.params.push(AbiParam::new(sty)); // an arg (i64 or f64)
    }
    wsig.params.push(AbiParam::new(types::I64)); // depth (always i64)
    wsig.params.push(AbiParam::new(ptr_ty)); // ovf ptr
    wsig.params.push(AbiParam::new(ptr_ty)); // heap ptr (throw callback only)
    wsig.returns.push(AbiParam::new(sty));
    let worker_id = m
        .declare_function(&format!("brood_jit_i64w_{seq}"), Linkage::Export, &wsig)
        .ok()?;
    let mut xsig = m.make_signature();
    xsig.params.push(AbiParam::new(ptr_ty)); // heap
    xsig.params.push(AbiParam::new(types::I64)); // base
    xsig.returns.push(AbiParam::new(types::I64)); // outcome
    let wrap_id = m
        .declare_function(&format!("brood_jit_i64x_{seq}"), Linkage::Export, &xsig)
        .ok()?;
    // Wrapper imports.
    let mut ptr_sig = m.make_signature();
    ptr_sig.params.push(AbiParam::new(ptr_ty));
    ptr_sig.returns.push(AbiParam::new(ptr_ty));
    let rb_id = m
        .declare_function("brood_rt_roots_base", Linkage::Import, &ptr_sig)
        .ok()?;
    let ovp_id = m
        .declare_function("brood_rt_i64_overflow_ptr", Linkage::Import, &ptr_sig)
        .ok()?;
    // The throw callback: (heap, payload_bits, is_float) -> sentinel (3 = error parked, 1 = deopt).
    let mut throw_sig = m.make_signature();
    throw_sig.params.push(AbiParam::new(ptr_ty));
    throw_sig.params.push(AbiParam::new(types::I64));
    throw_sig.params.push(AbiParam::new(types::I64));
    throw_sig.returns.push(AbiParam::new(types::I64));
    let throw_id = m
        .declare_function("brood_rt_i64_throw", Linkage::Import, &throw_sig)
        .ok()?;

    // ---- Worker ----
    {
        let mut ctx = m.make_context();
        ctx.func.signature = wsig;
        let mut fbctx = FunctionBuilderContext::new();
        let mut b = FunctionBuilder::new(&mut ctx.func, &mut fbctx);
        let self_ref = m.declare_func_in_func(worker_id, b.func);
        let throw_ref = m.declare_func_in_func(throw_id, b.func);
        let entry = b.create_block();
        b.append_block_params_for_function_params(entry);
        b.switch_to_block(entry);
        b.seal_block(entry);
        let params: Vec<cranelift_codegen::ir::Value> =
            (0..nargs).map(|k| b.block_params(entry)[k]).collect();
        let depth = b.block_params(entry)[nargs];
        let ovf = b.block_params(entry)[nargs + 1];
        let heap = b.block_params(entry)[nargs + 2];
        let poisoned = b.create_block();
        // Stack guard → set sentinel + unwind.
        let deep = b.create_block();
        let go = b.create_block();
        // **Stack guard (KI-14), and it is the ONLY per-level test.** Bail on *bytes*:
        // compare this frame's address against the limit the native entry point stamped
        // (`Heap::jit_stack_limit`, an absolute address ~512 KiB above the stack bottom).
        // The sentinel is the same one the old frame-count cap set, so the arm switches to
        // the boxed path and drains through heap frames.
        //
        // A frame *count* cap (`I64_DEPTH_LIMIT`, 32 768) used to run alongside this, and
        // it was both wrong and expensive. Wrong: a count is only ever right for one frame
        // size, which is what KI-14 was — `n_structure_open_array_object.json` recurses
        // ~100 000 levels through this worker with frames far heavier than the ~55–200 B
        // the cap was sized for, so 32 768 frames exhausted the 16 MiB worker stack long
        // before the count tripped, and the process died on its guard page rather than
        // raising a catchable error. Expensive: measuring the bytes AND the frames put a
        // second compare on every level of the recursion, which the 2026-07-27 run priced
        // at ~18% of `fib` (89 → 73 ms with it gone). The byte test subsumes the count
        // test — it measures the thing that actually runs out — so the count is gone.
        //
        // What the count *did* still cover is the case where the byte limit is `0`, i.e.
        // the platform could not read the stack: an unsigned compare against 0 never trips,
        // so the guard fails open. With no count cap behind it that would be an unguarded
        // native recursion, so the **wrapper refuses to run this worker at all when the
        // limit is 0** (returns outcome 5 → `jit_tier` moves the fn to the boxed path,
        // which keeps its own dispatch-level depth caps). See the wrapper's `nostack` bail.
        let limit = b.ins().load(
            ptr_ty,
            MemFlagsData::trusted(),
            heap,
            std::mem::offset_of!(crate::core::heap::Heap, jit_stack_limit) as i32,
        );
        let probe = b.create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
            cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
            8,
            3,
        ));
        let here = b.ins().stack_addr(ptr_ty, probe, 0);
        let over = b.ins().icmp(IntCC::UnsignedLessThan, here, limit);
        b.ins().brif(over, deep, &[], go, &[]);
        b.seal_block(deep);
        b.seal_block(go);
        b.switch_to_block(deep);
        // Sentinel 2 = depth-bail (vs 1 = overflow): the wrapper returns outcome 5 so `jit_tier`
        // switches this fn to the boxed path instead of re-tiering to i64 per level.
        let two = b.ins().iconst(types::I8, 2);
        b.ins().store(MemFlagsData::trusted(), two, ovf, 0);
        b.ins().jump(poisoned, &[]);
        b.switch_to_block(go);
        // SSA vars for `let` binder slots (>= nargs); param slots (< nargs) read `cx.params`.
        // Init to 0 as a safety floor (let/let* always overwrite before use — the checker
        // rejected forward-refs — so this only guards against any undefined-var edge).
        let mut slot_vars: Vec<Option<cranelift_frontend::Variable>> =
            Vec::with_capacity(arm.nslots);
        for k in 0..arm.nslots {
            if k < nargs {
                slot_vars.push(None);
            } else {
                let v = b.declare_var(sty);
                let z = match kind {
                    Scalar::Int => b.ins().iconst(types::I64, 0),
                    Scalar::Float => b.ins().f64const(0.0),
                };
                b.def_var(v, z);
                slot_vars.push(Some(v));
            }
        }
        let cx = I64Ctx {
            kind,
            self_sym,
            self_ref,
            throw_ref,
            params,
            slot_vars,
            depth,
            ovf,
            heap,
            poisoned,
        };
        let result = lower_i64_value(&mut b, &cx, body);
        b.ins().return_(&[result]);
        // The shared unwind block: returns a kind-zero (garbage — the wrapper deopts on the sentinel).
        b.switch_to_block(poisoned);
        let zero = match kind {
            Scalar::Int => b.ins().iconst(types::I64, 0),
            Scalar::Float => b.ins().f64const(0.0),
        };
        b.ins().return_(&[zero]);
        b.seal_block(poisoned);
        b.finalize();
        m.define_function(worker_id, &mut ctx).ok()?;
        m.clear_context(&mut ctx);
    }

    // ---- Boxed wrapper (the arm's entry) ----
    {
        let mut ctx = m.make_context();
        ctx.func.signature = xsig;
        let mut fbctx = FunctionBuilderContext::new();
        let mut b = FunctionBuilder::new(&mut ctx.func, &mut fbctx);
        let worker_ref = m.declare_func_in_func(worker_id, b.func);
        let rb_ref = m.declare_func_in_func(rb_id, b.func);
        let ovp_ref = m.declare_func_in_func(ovp_id, b.func);
        let entry = b.create_block();
        b.append_block_params_for_function_params(entry);
        b.switch_to_block(entry);
        b.seal_block(entry);
        let heap = b.block_params(entry)[0];
        let base = b.block_params(entry)[1];
        // **No stack limit → do not run the register worker at all.** The worker's only
        // per-level guard is the byte compare against `Heap::jit_stack_limit`, and a `0` limit
        // (the platform could not read the remaining stack) makes that unsigned compare fail
        // open — which, with the frame-count cap gone, would leave the native recursion
        // completely unguarded. Outcome 5 is the same "this fn belongs on the boxed path"
        // signal the depth-bail raises, so `jit_tier` retires the register version for good and
        // the boxed path — which still has its dispatch-level depth caps — takes over. One
        // load + one predicted branch per *outermost* activation; the recursion never sees it.
        {
            let limit = b.ins().load(
                ptr_ty,
                MemFlagsData::trusted(),
                heap,
                std::mem::offset_of!(crate::core::heap::Heap, jit_stack_limit) as i32,
            );
            let nostack = b.create_block();
            let armed = b.create_block();
            b.ins().brif(limit, armed, &[], nostack, &[]);
            b.seal_block(nostack);
            b.seal_block(armed);
            b.switch_to_block(nostack);
            let o5 = b.ins().iconst(types::I64, 5);
            b.ins().return_(&[o5]);
            b.switch_to_block(armed);
        }
        // Frame base address: roots + base*STRIDE. Args are at slots base+0..base+nargs-1; the
        // result goes back to slot base+0 (the VM's Done convention). The worker never touches
        // roots (it takes no heap), so this address stays valid across the worker call.
        let rbc = b.ins().call(rb_ref, &[heap]);
        let rbase = b.inst_results(rbc)[0];
        let off = b.ins().imul_imm(base, STRIDE);
        let argbase = b.ins().iadd(rbase, off);
        // Every arg must match the worker's scalar (Int/Float), else deopt to the VM.
        let deopt = b.create_block();
        for k in 0..nargs {
            let slot_off = (k as i64) * STRIDE;
            let tag = b
                .ins()
                .load(types::I8, MemFlagsData::trusted(), argbase, slot_off as i32);
            let is_ok = b.ins().icmp_imm(IntCC::Equal, tag, kind.tag() as i64);
            let nxt = b.create_block();
            b.ins().brif(is_ok, nxt, &[], deopt, &[]);
            b.seal_block(nxt);
            b.switch_to_block(nxt);
        }
        b.seal_block(deopt);
        // All args match → load the payloads (a float's bits bitcast to f64), clear the sentinel,
        // run the worker in registers.
        let mut wargs: Vec<cranelift_codegen::ir::Value> = (0..nargs)
            .map(|k| {
                let slot_off = (k as i64) * STRIDE + PAYLOAD_OFFSET as i64;
                let bits = b.ins().load(
                    types::I64,
                    MemFlagsData::trusted(),
                    argbase,
                    slot_off as i32,
                );
                match kind {
                    Scalar::Int => bits,
                    Scalar::Float => b.ins().bitcast(types::F64, MemFlagsData::new(), bits),
                }
            })
            .collect();
        let ovc = b.ins().call(ovp_ref, &[heap]);
        let ovf = b.inst_results(ovc)[0];
        let z0 = b.ins().iconst(types::I8, 0);
        b.ins().store(MemFlagsData::trusted(), z0, ovf, 0);
        let d0 = b.ins().iconst(types::I64, 0);
        wargs.push(d0);
        wargs.push(ovf);
        wargs.push(heap);
        let wc = b.ins().call(worker_ref, &wargs);
        let r = b.inst_results(wc)[0];
        let o = b.ins().load(types::I8, MemFlagsData::trusted(), ovf, 0);
        let doneb = b.create_block();
        let bailb = b.create_block();
        b.ins().brif(o, bailb, &[], doneb, &[]);
        b.seal_block(doneb);
        b.seal_block(bailb);
        // Sentinel nonzero → clear it, then split: 2 = depth-bail (outcome 5, `jit_tier` switches
        // this fn to the boxed path), 3 = thrown error (outcome 3 — the error is already parked
        // in `jit_pending_error` by `brood_rt_i64_throw`), 1 = overflow (outcome 1, VM recomputes
        // with BigInt).
        b.switch_to_block(bailb);
        let z1 = b.ins().iconst(types::I8, 0);
        b.ins().store(MemFlagsData::trusted(), z1, ovf, 0);
        let is_depth = b.ins().icmp_imm(IntCC::Equal, o, 2);
        let depthb = b.create_block();
        let notdepthb = b.create_block();
        b.ins().brif(is_depth, depthb, &[], notdepthb, &[]);
        b.seal_block(depthb);
        b.seal_block(notdepthb);
        b.switch_to_block(depthb);
        let o5 = b.ins().iconst(types::I64, 5);
        b.ins().return_(&[o5]);
        b.switch_to_block(notdepthb);
        let is_err = b.ins().icmp_imm(IntCC::Equal, o, 3);
        let errb = b.create_block();
        let ovb = b.create_block();
        b.ins().brif(is_err, errb, &[], ovb, &[]);
        b.seal_block(errb);
        b.seal_block(ovb);
        b.switch_to_block(errb);
        let o3 = b.ins().iconst(types::I64, 3);
        b.ins().return_(&[o3]);
        b.switch_to_block(ovb);
        let o1b = b.ins().iconst(types::I64, 1);
        b.ins().return_(&[o1b]);
        // Done → box the i64 result as an Int into roots[base], outcome 0.
        b.switch_to_block(doneb);
        let rbc2 = b.ins().call(rb_ref, &[heap]);
        let rbase2 = b.inst_results(rbc2)[0];
        let off2 = b.ins().imul_imm(base, STRIDE);
        let addr2 = b.ins().iadd(rbase2, off2);
        let tagv = b.ins().iconst(types::I64, kind.tag() as i64);
        b.ins().store(MemFlagsData::trusted(), tagv, addr2, 0);
        // Payload: an int as-is; a float's f64 bitcast to its i64 bits ([TAG_FLOAT, bits, 0]).
        let payload = match kind {
            Scalar::Int => r,
            Scalar::Float => b.ins().bitcast(types::I64, MemFlagsData::new(), r),
        };
        b.ins().store(
            MemFlagsData::trusted(),
            payload,
            addr2,
            PAYLOAD_OFFSET as i32,
        );
        let z2 = b.ins().iconst(types::I64, 0);
        b.ins().store(
            MemFlagsData::trusted(),
            z2,
            addr2,
            PAYLOAD_OFFSET as i32 + 8,
        );
        let d = b.ins().iconst(types::I64, 0);
        b.ins().return_(&[d]);
        // Any non-Int arg landed here: outcome 1 (the VM runs the arm).
        b.switch_to_block(deopt);
        let od = b.ins().iconst(types::I64, 1);
        b.ins().return_(&[od]);
        b.finalize();
        m.define_function(wrap_id, &mut ctx).ok()?;
        m.clear_context(&mut ctx);
    }

    m.finalize_definitions().ok()?;
    Some(m.get_finalized_function(wrap_id))
}
