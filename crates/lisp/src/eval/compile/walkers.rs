//! The shared `Node`-tree walkers: child iteration, the escape analysis a local's
//! linearity rests on, and the element-read rewrite. Used by the front-end, the
//! optimizer passes in `inline.rs` and the JIT planner.

use super::*;

/// Returns `true` if `node` (or any of its children) contains a
/// [`ConstVal::Handle`] or a [`Node::MakeClosure`] (whose `fn_rest` is always a
/// RUNTIME Pair handle). Used to set [`CompiledArm::has_runtime_handles`] at
/// compile time so `vm_apply` can skip `live_vm_arms` registration for pure
/// arithmetic / control-flow bodies that have nothing for `runtime_collect` to
/// rewrite.
pub(crate) fn node_has_rt_handles(node: &Node) -> bool {
    match node {
        Node::Const(cv) => matches!(cv, ConstVal::Handle { .. }),
        Node::MakeClosure {
            fn_rest, captures, ..
        } => {
            // fn_rest is always a RUNTIME Pair; captures may contain handles too.
            matches!(fn_rest, ConstVal::Handle { .. })
                || captures.iter().any(|(_, n)| node_has_rt_handles(n))
        }
        Node::If(a, b, c) => {
            node_has_rt_handles(a) || node_has_rt_handles(b) || node_has_rt_handles(c)
        }
        Node::Do(ns) => ns.iter().any(node_has_rt_handles),
        Node::Vector(ns) => ns.iter().any(node_has_rt_handles),
        Node::Map(pairs) => pairs
            .iter()
            .any(|(k, v)| node_has_rt_handles(k) || node_has_rt_handles(v)),
        Node::Call { callee, args, .. } => {
            node_has_rt_handles(callee) || args.iter().any(node_has_rt_handles)
        }
        Node::SelfCall { args, .. } => args.iter().any(node_has_rt_handles),
        Node::LetBind { binds, body } => {
            binds.iter().any(|(_, n)| node_has_rt_handles(n)) || node_has_rt_handles(body)
        }
        Node::Prim2 { a, b, .. } => node_has_rt_handles(a) || node_has_rt_handles(b),
        Node::Prim3 { a, b, c, .. } => {
            node_has_rt_handles(a) || node_has_rt_handles(b) || node_has_rt_handles(c)
        }
        Node::Prim1 { a, .. } => node_has_rt_handles(a),
        Node::TryCatch { body, handler, .. } => {
            node_has_rt_handles(body) || node_has_rt_handles(handler)
        }
        Node::Local(_) | Node::Global(_) | Node::GlobalIc { .. } => false,
    }
}

/// Is `(a b)` the operand pair of a safe element read `(nth slot K)` — `a` is `Local(slot)`
/// and `b` a constant index in `0..nelems`? Such a use consumes only an *element* of the
/// vector in `slot`, never the vector itself, so it doesn't make the vector escape.
pub(crate) fn is_elem_read(a: &Node, b: &Node, slot: usize, nelems: usize) -> Option<usize> {
    if let (Node::Local(k), Node::Const(cv)) = (a, b) {
        if *k == slot {
            if let ValueRef::Int(idx) = cv.load().unpack() {
                if idx >= 0 && (idx as usize) < nelems {
                    return Some(idx as usize);
                }
            }
        }
    }
    None
}

/// Call `f` on every child of `node` (not `node` itself). Used by the
/// EA analyses to avoid repeating structural recursion.
/// Can evaluating `n` run arbitrary user code — and therefore execute a `def`?
///
/// Only a call (or a `try`, whose body is one) can; reads, arithmetic, literals and control
/// flow over those cannot, however deeply nested. Decides whether a call's free-global head
/// must be resolved before its arguments (KI-19, see `Inst::Call::staged`). Conservative in
/// the safe direction: an unrecognised node counts as "runs code".
pub(crate) fn node_runs_user_code(n: &Node) -> bool {
    match n {
        Node::Call { .. } | Node::SelfCall { .. } | Node::TryCatch { .. } => true,
        Node::Const(_) | Node::Local(_) | Node::Global(_) | Node::GlobalIc { .. } => false,
        _ => {
            let mut found = false;
            walk_children(n, |c| found = found || node_runs_user_code(c));
            found
        }
    }
}

pub(crate) fn walk_children<F: FnMut(&Node)>(node: &Node, mut f: F) {
    match node {
        Node::If(a, b, c) => {
            f(a);
            f(b);
            f(c);
        }
        Node::Do(xs) => xs.iter().for_each(&mut f),
        Node::LetBind { binds, body } => {
            binds.iter().for_each(|(_, v)| f(v));
            f(body);
        }
        Node::Call { callee, args, .. } => {
            f(callee);
            args.iter().for_each(&mut f);
        }
        Node::SelfCall { args, .. } => args.iter().for_each(&mut f),
        Node::MakeClosure { captures, .. } => captures.iter().for_each(|(_, v)| f(v)),
        Node::Vector(xs) => xs.iter().for_each(&mut f),
        Node::Map(kvs) => kvs.iter().for_each(|(k, v)| {
            f(k);
            f(v);
        }),
        Node::Prim2 { a, b, .. } => {
            f(a);
            f(b);
        }
        Node::Prim3 { a, b, c, .. } => {
            f(a);
            f(b);
            f(c);
        }
        Node::Prim1 { a, .. } => f(a),
        Node::TryCatch { body, handler, .. } => {
            f(body);
            f(handler);
        }
        Node::Const(_) | Node::Local(_) | Node::Global(_) | Node::GlobalIc { .. } => {}
    }
}

/// Mutable variant for tree rewrites.
pub(crate) fn walk_children_mut<F: FnMut(&mut Node)>(node: &mut Node, mut f: F) {
    match node {
        Node::If(a, b, c) => {
            f(a);
            f(b);
            f(c);
        }
        Node::Do(xs) => xs.iter_mut().for_each(&mut f),
        Node::LetBind { binds, body } => {
            binds.iter_mut().for_each(|(_, v)| f(v));
            f(body);
        }
        Node::Call { callee, args, .. } => {
            f(callee);
            args.iter_mut().for_each(&mut f);
        }
        Node::SelfCall { args, .. } => args.iter_mut().for_each(&mut f),
        Node::MakeClosure { captures, .. } => captures.iter_mut().for_each(|(_, v)| f(v)),
        Node::Vector(xs) => xs.iter_mut().for_each(&mut f),
        Node::Map(kvs) => kvs.iter_mut().for_each(|(k, v)| {
            f(k);
            f(v);
        }),
        Node::Prim2 { a, b, .. } => {
            f(a);
            f(b);
        }
        Node::Prim3 { a, b, c, .. } => {
            f(a);
            f(b);
            f(c);
        }
        Node::Prim1 { a, .. } => f(a),
        Node::TryCatch { body, handler, .. } => {
            f(body);
            f(handler);
        }
        Node::Const(_) | Node::Local(_) | Node::Global(_) | Node::GlobalIc { .. } => {}
    }
}

/// Does the value in frame `slot` **escape** — appear anywhere other than as the vector
/// operand of an in-range `(nth slot K)`? Immutability makes this a pure reachability walk
/// (no alias analysis — BEAM does none): a value is only reachable through references the
/// code explicitly creates, so any `Local(slot)` outside an element read means it's returned,
/// passed to a call, captured, or stored — i.e. escapes. Used by EA scalar replacement.
pub(crate) fn local_escapes(node: &Node, slot: usize, nelems: usize) -> bool {
    if let Node::Prim2 {
        op: PrimOp::VectorRef,
        a,
        b,
        ..
    } = node
    {
        if is_elem_read(a, b, slot, nelems).is_some() {
            return local_escapes(b, slot, nelems); // `a` consumed safely; `b` is the const index
        }
    }
    if let Node::Local(k) = node {
        return *k == slot;
    }
    let mut found = false;
    walk_children(node, |child| {
        found = found || local_escapes(child, slot, nelems)
    });
    found
}

/// In-place: replace every safe element read `(nth slot K)` with a direct `Local(base + K)`
/// read (the scalar-replaced element slots). Paired with `local_escapes` having returned
/// false, so every `Local(slot)` is exactly such a read.
pub(crate) fn rewrite_elem_reads(node: &mut Node, slot: usize, base: usize, nelems: usize) {
    if let Node::Prim2 {
        op: PrimOp::VectorRef,
        a,
        b,
        ..
    } = node
    {
        if let Some(k) = is_elem_read(a, b, slot, nelems) {
            *node = Node::Local(base + k);
            return;
        }
    }
    walk_children_mut(node, |child| rewrite_elem_reads(child, slot, base, nelems));
}

/// Escape-analysis scalar replacement (lever 2 / `modern-perf-bets` #2). A non-escaping
/// `(let (p [e0 … eN]) …)` whose `p` is read only as `(nth p K)` is rewritten so each element
/// binds to its own frame slot and the reads become direct `Local` reads — the vector is
/// **never allocated**, and the arm gets *simpler* (so it JITs better, not worse). Immutability
/// makes the escape test a pure reachability walk; BEAM does no EA, so this is a structural
/// edge. Conservative: a single-binder `let` of a small vector literal, all uses in-range
/// constant `nth`. Bumps `next_slot` by the element count. Recurses (nested lets covered).
pub(crate) fn ea_scalar_replace(node: &mut Node, next_slot: &mut usize) -> bool {
    const MAX_ELEMS: usize = 8;
    let mut changed = false;
    walk_children_mut(node, |child| {
        changed |= ea_scalar_replace(child, next_slot);
    });
    if let Node::LetBind { binds, body } = node {
        if binds.len() == 1 {
            let slot = binds[0].0;
            let n = match &binds[0].1 {
                Node::Vector(e) => e.len(),
                _ => 0,
            };
            if (1..=MAX_ELEMS).contains(&n) && !local_escapes(body, slot, n) {
                let base = *next_slot;
                *next_slot += n;
                rewrite_elem_reads(body, slot, base, n);
                let elems = match &mut binds[0].1 {
                    Node::Vector(e) => std::mem::replace(e, Box::new([])),
                    _ => unreachable!(),
                };
                *binds = elems
                    .into_vec()
                    .into_iter()
                    .enumerate()
                    .map(|(k, e)| (base + k, e))
                    .collect();
                changed = true;
            }
        }
    }
    changed
}
