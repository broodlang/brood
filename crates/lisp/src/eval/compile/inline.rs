//! Node→Node optimizer passes: linmap rewrite + self/leaf inlining (extracted from mod.rs).
use super::*;

/// Whitelisted map READ ops (return a value — safe in any position) → Table op.
pub(crate) fn linmap_read_op(sym: Symbol) -> Option<&'static str> {
    match value::symbol_name_opt(sym)? {
        "map-get" => Some("table-get"),
        "map-count" => Some("table-count"),
        _ => None,
    }
}

/// Whitelisted map UPDATE ops (return the new map — must be consumed at a sink) → Table op.
/// Only ops that provably store serializable values (integers, removals) are whitelisted;
/// `map-assoc` stores arbitrary `Value`s including ropes, which `table-put`/`table-snapshot`
/// cannot serialize — so it is excluded until the Table can hold non-serializable values.
pub(crate) fn linmap_update_op(sym: Symbol) -> Option<&'static str> {
    match value::symbol_name_opt(sym)? {
        "map-int-add" => Some("table-incr"),
        "map-dissoc" => Some("table-delete"),
        _ => None,
    }
}

/// The global symbol a call head resolves to, if it is a free-global head.
pub(crate) fn call_head_sym(callee: &Node) -> Option<Symbol> {
    match callee {
        Node::Global(s) => Some(*s),
        Node::GlobalIc { sym, .. } => Some(*sym),
        _ => None,
    }
}

/// Where a value flows: a "sink" is a position whose value is the accumulator's
/// linear continuation — the self-call's own arg slot, or the function's return.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum LinSink {
    Return,
    SelfArg,
    No,
}

/// True iff `(args[0] == Local(s))`.
pub(crate) fn first_arg_is_local(args: &[Node], s: usize) -> bool {
    matches!(args.first(), Some(Node::Local(k)) if *k == s)
}

/// Reachability check: every read of slot `s` is a whitelisted map op's first arg,
/// the self-call's arg-`s` (threading), or a sink return — and update ops on `s`
/// occur only at a sink (so the new map is linearly consumed, never aliased).
/// Anything else → not linear (bail). `sink` is this position's flow role.
pub(crate) fn linmap_linear(node: &Node, s: usize, sink: LinSink) -> bool {
    match node {
        Node::Local(k) => *k != s || sink != LinSink::No,
        Node::Call { callee, args, .. } => {
            if let Some(h) = call_head_sym(callee) {
                if first_arg_is_local(args, s)
                    && (linmap_read_op(h).is_some()
                        || (linmap_update_op(h).is_some() && sink != LinSink::No))
                {
                    // args[0] (== Local(s)) is consumed by the op; the rest must
                    // not mention s (s is the map, not a key/value/default).
                    return args[1..].iter().all(|a| linmap_linear(a, s, LinSink::No));
                }
            }
            linmap_linear(callee, s, LinSink::No)
                && args.iter().all(|a| linmap_linear(a, s, LinSink::No))
        }
        Node::SelfCall { args, .. } => args.iter().enumerate().all(|(i, a)| {
            linmap_linear(
                a,
                s,
                if i == s {
                    LinSink::SelfArg
                } else {
                    LinSink::No
                },
            )
        }),
        Node::If(c, t, e) => {
            linmap_linear(c, s, LinSink::No)
                && linmap_linear(t, s, sink)
                && linmap_linear(e, s, sink)
        }
        Node::Do(xs) => {
            let last = xs.len().saturating_sub(1);
            xs.iter()
                .enumerate()
                .all(|(i, x)| linmap_linear(x, s, if i == last { sink } else { LinSink::No }))
        }
        Node::LetBind { binds, body } => {
            binds.iter().all(|(_, v)| linmap_linear(v, s, LinSink::No))
                && linmap_linear(body, s, sink)
        }
        Node::Const(_) | Node::Global(_) | Node::GlobalIc { .. } => true,
        // Vector / Map / Prim1 / Prim2 / MakeClosure / TryCatch: any read of s here
        // is a genuine escape (capture, store, arithmetic on a map, …).
        other => {
            let mut ok = true;
            walk_children(other, |c| ok = ok && linmap_linear(c, s, LinSink::No));
            ok
        }
    }
}

/// Does `s` appear as the first arg of an UPDATE op anywhere? (Only then is the
/// rewrite a win — a read-only accumulator gains nothing.)
pub(crate) fn linmap_has_update(node: &Node, s: usize) -> bool {
    if let Node::Call { callee, args, .. } = node {
        if first_arg_is_local(args, s) {
            if let Some(h) = call_head_sym(callee) {
                if linmap_update_op(h).is_some() {
                    return true;
                }
            }
        }
    }
    let mut found = false;
    walk_children(node, |c| found = found || linmap_has_update(c, s));
    found
}

/// True if `node`, or any descendant, satisfies `pred`. The one recursive
/// "does the tree contain a …?" walker — every `node_has_*` probe is a `pred`
/// over this (self-calls, `MakeClosure`, …), so the recursion lives in one place.
pub(crate) fn node_any<F: Fn(&Node) -> bool>(node: &Node, pred: &F) -> bool {
    pred(node) || {
        let mut found = false;
        walk_children(node, |c| found = found || node_any(c, pred));
        found
    }
}

/// Probe whether `(fn (params…) body…)` folds a **linear** immutable-map
/// accumulator through a self-tail loop, returning that accumulator's param index
/// if so. Backs the macroexpand-time wrapper-split (`eval/macros.rs`): it compiles
/// a throwaway `Node` (like `self_inline_probe`) and runs the reachability analysis
/// on it, so the soundness rule lives in exactly one place. Only a simple
/// fixed-arity, no-capture top-level `defn` qualifies; anything else → `None`.
pub(crate) fn linmap_probe(
    heap: &Heap,
    name: Symbol,
    params: &[Symbol],
    body: &[Value],
) -> Option<usize> {
    if std::env::var_os("BROOD_LINMAP").is_some_and(|v| v == "0") {
        return None; // opt-out escape hatch
    }
    let mut scope = Scope::with_params_enclosing(&[], Vec::new());
    scope.self_call = Some((name, params.len()));
    for &p in params {
        scope.bind(p);
    }
    let node = compile_body(heap, body, &mut scope, true)?;
    if !node_any(&node, &|n| matches!(n, Node::SelfCall { .. })) {
        return None;
    }
    (0..params.len())
        .find(|&s| linmap_has_update(&node, s) && linmap_linear(&node, s, LinSink::Return))
}

// ===================== recursive self-inlining (Phase B, §6b) =====================
//
// `docs/jit-optimizing-tier.md` §6b. A non-tail self-recursive call to a top-level
// `defn` is replaced by an *inlined block* — the callee's body spliced into the
// caller's frame at a shifted slot range — so the inlined level runs without the
// per-call protocol (no frame setup, no dispatch). Depth-1 only: the copied body's
// own self-calls stay as `Node::Call` (a real call at the leaf). Removes ~1 protocol
// entry per ~2 levels for `fib`-shaped two-call recursion.
//
// Gated conservatively (see `self_inline_arm`): top-level no-capture recursive defn,
// no `SelfCall` (its frame-reuse is incompatible with slot-shifting), no `MakeClosure`,
// a body-size bound, and ≥1 qualifying non-tail self-call.

/// Largest original-arm body (node count) we will inline. Inlining roughly doubles the
/// body, and an oversized arm both blows the i-cache and risks the JIT's lowering
/// limits; `fib`/`collatz`-shaped recursive kernels are tiny (well under this). Picked
/// conservatively to avoid 2^D blow-up while comfortably admitting the target shapes.
#[cfg(feature = "jit")]
pub(crate) const SELF_INLINE_MAX_BODY: usize = 64;

/// Total node count of `node` (every variant counted, children recursed).
#[cfg(feature = "jit")]
pub(crate) fn node_count(node: &Node) -> usize {
    let mut n = 1;
    walk_children(node, |child| n += node_count(child));
    n
}

/// Is `node` a non-tail self-recursive call to `defn_name` with exactly `nrequired`
/// args? The call head is a free-global reference — `compile_node` lowers a free symbol
/// in head position to `Node::Global(sym)` (never `GlobalIc`, since the call site's own
/// IC caches the resolution), so that's the only shape to match. A computed/local callee
/// (`NO_SITE`) can resolve to a different function per call and is never inlined.
#[cfg(feature = "jit")]
pub(crate) fn is_inlinable_self_call(node: &Node, defn_name: Symbol, nrequired: usize) -> bool {
    if let Node::Call {
        callee,
        args,
        tail: false,
        ..
    } = node
    {
        if args.len() == nrequired {
            return matches!(
                &**callee,
                Node::Global(s) | Node::GlobalIc { sym: s, .. } if *s == defn_name
            );
        }
    }
    false
}

/// Deep-copy `node`, adding `delta` to every frame-slot reference it contains
/// (`Local`, `SetLocal`/`LetBind` targets). `Node` is **not** `Clone` — `Const`/`Prim2`/
/// `Prim1` carry `AtomicU64`s reconstructed here with their current loaded value, and
/// `ConstVal`/`MakeClosure.fn_rest` handles are rebuilt via `ConstVal::new(cv.load())`
/// (an atom stays inline; a handle is re-split — its bits stay live for the next runtime
/// compaction). The copy's own `Call`/`GlobalIc` keep their `site` ids (all copies share
/// the same correct IC entry); `pos` is shared (diagnostics only). A missed slot shift is
/// a silent wrong result — every slot-bearing variant is enumerated.
#[cfg(feature = "jit")]
pub(crate) fn shift_slots(node: &Node, delta: usize) -> Node {
    match node {
        Node::Const(cv) => Node::Const(ConstVal::new(cv.load())),
        Node::Local(i) => Node::Local(i + delta),
        Node::Global(s) => Node::Global(*s),
        Node::GlobalIc { sym, site } => Node::GlobalIc {
            sym: *sym,
            site: *site,
        },
        Node::If(a, b, c) => Node::If(
            Box::new(shift_slots(a, delta)),
            Box::new(shift_slots(b, delta)),
            Box::new(shift_slots(c, delta)),
        ),
        Node::Do(xs) => Node::Do(xs.iter().map(|n| shift_slots(n, delta)).collect()),
        Node::Vector(xs) => Node::Vector(xs.iter().map(|n| shift_slots(n, delta)).collect()),
        Node::Map(kvs) => Node::Map(
            kvs.iter()
                .map(|(k, v)| (shift_slots(k, delta), shift_slots(v, delta)))
                .collect(),
        ),
        Node::Call {
            callee,
            args,
            tail: _,
            pos,
            file,
            site,
        } => Node::Call {
            callee: Box::new(shift_slots(callee, delta)),
            args: args.iter().map(|n| shift_slots(n, delta)).collect(),
            // **Demote to non-tail.** A spliced body always lands in the *operand*
            // (non-tail) position the inlined self-call occupied (the inliner only
            // inlines `tail: false` self-calls), so NOTHING in the copy is in the arm's
            // tail position any more. A call that was tail-of-the-original-fn (e.g. the
            // `else (helper …)` clause of a `cond` body) must NOT stay `tail: true`: a
            // tail call returns from the whole frame, which would discard the expression
            // wrapping the inlined block (`(/ 1 <block>)` returned `<block>` — the pow /
            // `s` 32-test regression). Leaf self-calls were already `tail: false`; forcing
            // false is a no-op for them. (`shift_slots` is used only by the inliner, and
            // only to splice into non-tail position, so the demotion is always correct.)
            tail: false,
            pos: *pos,
            file: file.clone(),
            site: *site,
        },
        Node::SelfCall { args, pos } => Node::SelfCall {
            args: args.iter().map(|n| shift_slots(n, delta)).collect(),
            pos: *pos,
        },
        Node::LetBind { binds, body } => Node::LetBind {
            binds: binds
                .iter()
                .map(|(slot, n)| (slot + delta, shift_slots(n, delta)))
                .collect(),
            body: Box::new(shift_slots(body, delta)),
        },
        Node::MakeClosure {
            fn_rest,
            captures,
            self_name,
        } => Node::MakeClosure {
            fn_rest: ConstVal::new(fn_rest.load()),
            captures: captures
                .iter()
                .map(|(sym, n)| (*sym, shift_slots(n, delta)))
                .collect(),
            self_name: *self_name,
        },
        Node::Prim2 {
            op,
            a,
            b,
            map,
            head,
            guard,
            pos,
            broot,
        } => Node::Prim2 {
            op: *op,
            a: Box::new(shift_slots(a, delta)),
            b: Box::new(shift_slots(b, delta)),
            map: *map,
            head: *head,
            guard: AtomicU64::new(guard.load(Ordering::Relaxed)),
            pos: *pos,
            broot: *broot,
        },
        Node::Prim3 {
            op,
            a,
            b,
            c,
            head,
            guard,
            pos,
        } => Node::Prim3 {
            op: *op,
            a: Box::new(shift_slots(a, delta)),
            b: Box::new(shift_slots(b, delta)),
            c: Box::new(shift_slots(c, delta)),
            head: *head,
            guard: AtomicU64::new(guard.load(Ordering::Relaxed)),
            pos: *pos,
        },
        Node::Prim1 {
            op,
            a,
            head,
            guard,
            pos,
        } => Node::Prim1 {
            op: *op,
            a: Box::new(shift_slots(a, delta)),
            head: *head,
            guard: AtomicU64::new(guard.load(Ordering::Relaxed)),
            pos: *pos,
        },
        Node::TryCatch {
            body,
            bind_slot,
            handler,
        } => Node::TryCatch {
            body: Box::new(shift_slots(body, delta)),
            bind_slot: bind_slot + delta,
            handler: Box::new(shift_slots(handler, delta)),
        },
    }
}

/// Replace, in place, each qualifying non-tail self-call in `node` with an inlined block:
/// `LetBind { binds: [(M*i + k, args[k])], body: shift_slots(orig_body, M*i) }`. Each
/// distinct call site gets the next inline-block index `i` (1, 2, …), so simultaneous
/// inlined results occupy disjoint shifted slot ranges. The args bind in the *outer*
/// scope (so they read the caller's unshifted slots); the shifted body reads the shifted
/// param slots. The copied body's own self-calls stay `Node::Call` (depth-1 bound).
/// Returns the number of sites inlined.
#[cfg(feature = "jit")]
pub(crate) fn inline_self_calls(
    node: &mut Node,
    orig_body: &Node,
    defn_name: Symbol,
    nrequired: usize,
    m: usize,
    next_block: &mut usize,
) -> usize {
    // Bottom-up: rewrite children first, so an inlined block's *args* (which stay in the
    // outer scope) are never themselves re-inlined — only the original-body calls are.
    let mut count = 0;
    match node {
        Node::Const(_) | Node::Local(_) | Node::Global(_) | Node::GlobalIc { .. } => {}
        Node::If(a, b, c) => {
            count += inline_self_calls(a, orig_body, defn_name, nrequired, m, next_block);
            count += inline_self_calls(b, orig_body, defn_name, nrequired, m, next_block);
            count += inline_self_calls(c, orig_body, defn_name, nrequired, m, next_block);
        }
        Node::Do(xs) | Node::Vector(xs) => {
            for n in xs.iter_mut() {
                count += inline_self_calls(n, orig_body, defn_name, nrequired, m, next_block);
            }
        }
        Node::Map(kvs) => {
            for (k, v) in kvs.iter_mut() {
                count += inline_self_calls(k, orig_body, defn_name, nrequired, m, next_block);
                count += inline_self_calls(v, orig_body, defn_name, nrequired, m, next_block);
            }
        }
        Node::Call { callee, args, .. } => {
            count += inline_self_calls(callee, orig_body, defn_name, nrequired, m, next_block);
            for n in args.iter_mut() {
                count += inline_self_calls(n, orig_body, defn_name, nrequired, m, next_block);
            }
        }
        Node::SelfCall { args, .. } => {
            for n in args.iter_mut() {
                count += inline_self_calls(n, orig_body, defn_name, nrequired, m, next_block);
            }
        }
        Node::LetBind { binds, body } => {
            for (_, n) in binds.iter_mut() {
                count += inline_self_calls(n, orig_body, defn_name, nrequired, m, next_block);
            }
            count += inline_self_calls(body, orig_body, defn_name, nrequired, m, next_block);
        }
        Node::MakeClosure { captures, .. } => {
            for (_, n) in captures.iter_mut() {
                count += inline_self_calls(n, orig_body, defn_name, nrequired, m, next_block);
            }
        }
        Node::Prim2 { a, b, .. } => {
            count += inline_self_calls(a, orig_body, defn_name, nrequired, m, next_block);
            count += inline_self_calls(b, orig_body, defn_name, nrequired, m, next_block);
        }
        Node::Prim3 { a, b, c, .. } => {
            count += inline_self_calls(a, orig_body, defn_name, nrequired, m, next_block);
            count += inline_self_calls(b, orig_body, defn_name, nrequired, m, next_block);
            count += inline_self_calls(c, orig_body, defn_name, nrequired, m, next_block);
        }
        Node::Prim1 { a, .. } => {
            count += inline_self_calls(a, orig_body, defn_name, nrequired, m, next_block);
        }
        Node::TryCatch { body, handler, .. } => {
            count += inline_self_calls(body, orig_body, defn_name, nrequired, m, next_block);
            count += inline_self_calls(handler, orig_body, defn_name, nrequired, m, next_block);
        }
    }
    // Now consider *this* node (children already inlined). The args we move out keep
    // their (already-recursed) form; the spliced body is a fresh copy of the *original*
    // body shifted into this block's slot range — so the copy's own calls are untouched.
    if is_inlinable_self_call(node, defn_name, nrequired) {
        let i = *next_block;
        *next_block += 1;
        let shift = m * i;
        // Take the call's args out of the node.
        let args = match node {
            Node::Call { args, .. } => std::mem::take(args),
            _ => unreachable!(),
        };
        let binds: Box<[(usize, Node)]> = args
            .into_vec()
            .into_iter()
            .enumerate()
            .map(|(k, a)| (shift + k, a))
            .collect();
        *node = Node::LetBind {
            binds,
            body: Box::new(shift_slots(orig_body, shift)),
        };
        count += 1;
    }
    count
}

/// Is the JIT self-inliner enabled? **Default ON** (the two-stage tiering build, devlog
/// 2026-06-17) — `BROOD_NO_INLINE=1` opts out (the A/B baseline lever). Replaces the old
/// `BROOD_JIT_INLINE` opt-in: the dual-body + per-engine frame sizing + deferred-upgrade
/// tiering removes the regressions that kept it shelved (the VM keeps the original small
/// body; the inlined arm tiers only as a low-priority background upgrade).
#[cfg(feature = "jit")]
pub(crate) fn self_inline_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("BROOD_NO_INLINE").is_none())
}

/// How many recursion levels to splice into a self-inlined arm. Each pass inlines one more
/// level of the recursion (removing that level's call protocol + its call-boundary boxing)
/// at the cost of a larger arm body / frame. **Default 2** (the shipped two-stage-tiering
/// behaviour); `BROOD_INLINE_DEPTH` overrides for A/B measurement. Read once (all processes
/// of a runtime must agree — the inlined native is shared across them and its frame size
/// must be deterministic). Clamped to ≥1.
#[cfg(feature = "jit")]
pub(crate) fn self_inline_depth() -> usize {
    static D: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *D.get_or_init(|| {
        std::env::var("BROOD_INLINE_DEPTH")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .map(|d| d.max(1))
            .unwrap_or(2)
    })
}

/// Per-pass body-size ceiling for expansion *beyond the first pass*: an extra level only
/// splices while the body so far is ≤ this, bounding the compiled-arm blow-up (a larger
/// arm costs more i-cache + compile time — the thing that can erase the inline win). The
/// first pass and the initial qualify-gate stay at [`SELF_INLINE_MAX_BODY`] (a large
/// *original* body never inlines). **Default `SELF_INLINE_MAX_BODY`** (= the shipped
/// Level-2 gate); `BROOD_INLINE_MAXBODY` overrides for A/B measurement.
#[cfg(feature = "jit")]
pub(crate) fn self_inline_expand_cap() -> usize {
    static C: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *C.get_or_init(|| {
        std::env::var("BROOD_INLINE_MAXBODY")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(SELF_INLINE_MAX_BODY)
    })
}

/// Build the inlined body: splice the recursion [`self_inline_depth`] levels deep. The
/// single source of truth shared by [`self_inline_probe`] (which measures the frame) and
/// [`rederive_inlined_body`] (which produces the body for lowering) — so the two can never
/// diverge (a mismatch would size the frame wrong → corruption). Returns the spliced node
/// plus `next_block` (= 1 + total inlined blocks; `m * next_block` is the slot high-water
/// mark). `None` when no site inlines.
#[cfg(feature = "jit")]
pub(crate) fn build_inlined_body(
    body: &Node,
    defn_name: Symbol,
    nrequired: usize,
    m: usize,
) -> Option<(Node, usize)> {
    let mut spliced = shift_slots(body, 0);
    let orig = shift_slots(body, 0);
    let mut next_block = 1usize;
    // Pass 1 (depth-1): the top-level self-calls become inlined blocks whose bodies still
    // hold `Node::Call` self-calls.
    let inlined = inline_self_calls(
        &mut spliced,
        &orig,
        defn_name,
        nrequired,
        m,
        &mut next_block,
    );
    if inlined == 0 {
        return None;
    }
    // Extra passes (depth-2, -3, …): each re-inlines the calls left in the previously
    // spliced bodies, one level deeper — while the body stays under the expansion cap.
    let cap = self_inline_expand_cap();
    for _ in 1..self_inline_depth() {
        if node_count(&spliced) > cap {
            break;
        }
        inline_self_calls(
            &mut spliced,
            &orig,
            defn_name,
            nrequired,
            m,
            &mut next_block,
        );
    }
    Some((spliced, next_block))
}

/// Runtime JIT off-switch. **Default OFF** (the JIT runs). `BROOD_NO_JIT=1` makes
/// [`jit_tier`] never compile or run native code — every arm interprets on the VM,
/// which is the correct reference engine. Today disabling the JIT otherwise needs a
/// no-`jit`-feature *build*; this is the runtime A/B lever that lets a suspected
/// JIT-only miscompile (e.g. a use-after-GC in a render arm) be ruled in/out and
/// worked around without a rebuild. Cached once, like the other JIT levers.
#[cfg(feature = "jit")]
pub(crate) fn no_jit_enabled() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| std::env::var_os("BROOD_NO_JIT").is_some())
}

/// Debug bisect: `BROOD_NO_JIT_COMPUTED=1` bails (runs on the VM) any arm whose chunk
/// contains a **computed-head** non-tail call `(f …)` — the shape fold--loop / assoc--pairs
/// share, suspected in the JIT+GC value→nil/stale bug. If the repro goes clean with this set,
/// the computed-head call lowering is the culprit.
#[cfg(feature = "jit")]
pub(crate) fn no_jit_computed() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| std::env::var_os("BROOD_NO_JIT_COMPUTED").is_some())
}

/// Runtime JIT self-verification (`BROOD_JIT_VERIFY=1`). Runs the staged-stale-handle scan
/// on every Brood→Brood call's staged args in **any** build (not just debug-assertions) —
/// so a JIT+GC use-after-GC (bug #2) can be caught at the staging site (naming the callee +
/// the stale handle kind) in a normal `--release` binary, without a debug-armed rebuild.
/// Off by default; one cached bool check + (when on) a short scan per call.
#[cfg(feature = "jit")]
pub(crate) fn jit_verify_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("BROOD_JIT_VERIFY").is_some())
}

/// `BROOD_JIT_VERIFY_FN=<fn>`: log every JIT'd Brood→Brood call to `<fn>` with each staged
/// arg's *type* — the targeted, low-noise way to see a value-level corruption (e.g. a `nil`
/// staged where a number belongs: pong's `badge-ops` getting `throb=nil`). Identifies whether
/// a bad value is staged *from JIT'd code* and which arg position, without a debug rebuild.
#[cfg(feature = "jit")]
pub(crate) fn jit_verify_fn() -> Option<&'static str> {
    static F: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    F.get_or_init(|| std::env::var("BROOD_JIT_VERIFY_FN").ok())
        .as_deref()
}

/// Either verification mode is on (the stale-handle scan, or the targeted per-fn arg log).
#[cfg(feature = "jit")]
pub(crate) fn jit_verify_active() -> bool {
    jit_verify_enabled() || jit_verify_fn().is_some()
}

/// A one-word type tag for a `Value`, for the `BROOD_JIT_VERIFY_FN` arg log.
#[cfg(feature = "jit")]
pub(crate) fn jit_describe_value(v: Value) -> &'static str {
    match v {
        Value::Nil => "NIL",
        Value::Bool(_) => "bool",
        Value::Int(_) => "int",
        Value::BigInt(_) => "bigint",
        Value::Float(_) => "float",
        Value::Sym(_) => "sym",
        Value::Keyword(_) => "keyword",
        Value::Str(_) => "str",
        Value::Rope(_) => "rope",
        Value::Pair(_) => "pair",
        Value::Vector(_) => "vector",
        Value::Range(_) => "range",
        Value::Map(_) => "map",
        Value::Set(_) => "set",
        _ => "other",
    }
}

/// Scan `roots[lo..hi]` for a stale LOCAL handle (`BROOD_JIT_VERIFY`) and, when
/// `BROOD_JIT_VERIFY_FN` names this call's callee, log every staged arg's type. `head`/`site`/
/// `argc` describe the call being staged.
#[cfg(feature = "jit")]
pub(crate) fn jit_verify_staged(
    heap: &Heap,
    lo: usize,
    hi: usize,
    head: Symbol,
    site: u32,
    argc: usize,
) {
    // Non-panicking: a computed-head call passes a `head` that isn't a real interned
    // symbol, so never `expect` it (that aborts the whole run from inside a diagnostic).
    let head_name = crate::core::value::symbol_name_opt(head).unwrap_or("<computed>");
    let log_args = jit_verify_fn() == Some(head_name);
    for k in lo..hi {
        let v = heap.root_at(k);
        if jit_verify_enabled() {
            if let Some((kind, g, e)) = heap.dbg_value_stale(v) {
                let raw = unsafe { std::mem::transmute::<Value, [i64; 3]>(v) };
                eprintln!(
                    "[jit-verify] STALE {kind} (gen {g} != live {e}) staged at roots[{k}] BY arm \
                     '{}' for call to '{head_name}' (site={site}, argc={argc}); raw=[{:#x},{:#x},{:#x}]",
                    crate::core::value::symbol_name_opt(heap.jit_dbg_fn).unwrap_or("<unknown>"),
                    raw[0], raw[1], raw[2],
                );
            }
        }
        if log_args {
            let raw = unsafe { std::mem::transmute::<Value, [i64; 3]>(v) };
            eprintln!(
                "[jit-verify-fn] BY arm '{}' call to '{head_name}' (site={site}) arg[{}] = {} raw=[{:#x},{:#x},{:#x}]",
                crate::core::value::symbol_name_opt(heap.jit_dbg_fn).unwrap_or("<unknown>"),
                k - lo,
                jit_describe_value(v),
                raw[0], raw[1], raw[2],
            );
        }
    }
}

/// Is the in-IR call-site fast-link (Track B / Technique A increment 1) enabled? **Default ON**
/// (shipped after the gate proved it — fib ~20% faster, JIT≡VM clean). When on, a JIT'd arm's
/// non-tail free-global call emits an epoch-guarded flat-table fast path (`brood_rt_fast_frame`)
/// ahead of the `brood_rt_call_slow` miss path, removing the per-call IC probe + `RefCell`
/// borrow. `BROOD_NO_JIT_ICALL=1` opts out (every call goes straight through
/// `brood_rt_call_slow`, the A/B baseline lever). Increment 2 (full in-IR frame setup) was
/// measured slower and reverted — see `docs/devlog.md` 2026-06-19; this is the sweet spot.
#[cfg(feature = "jit")]
pub(crate) fn icall_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("BROOD_NO_JIT_ICALL").is_none())
}

/// **Non-mutating** probe: does `body` qualify for depth-1 self-inlining, and if so what
/// is the inlined frame's slot high-water mark? Runs the inliner on a CLONE (discarded),
/// so the original `body` is never touched — the VM keeps the small layout. `m` is the
/// original arm's slot high-water mark (`scope.max`), the per-block slot stride. Returns
/// the inlined `scope.max` (= `m * (1 + blocks)`) when ≥1 site inlines, else `None`. The
/// gate (top-level no-capture recursive defn) is partly the caller's (`defn_name` +
/// fixed-arity); here we enforce no `SelfCall`/`MakeClosure`, the body-size bound, and
/// ≥1 qualifying call. The *spill reserve* for the inlined chunk is added by the caller's
/// re-derivation; this returns the slot count only.
/// True if `node` (or any descendant) does **heap work** — builds a vector/map literal
/// (`[..]`/`{..}`), `cons`es, or reads a structure (`nth`/`vector-ref`, `first`/`rest`).
/// Such recursive arms must NOT be inlined. **Measured (devlog 2026-06-17):** inlining
/// bintree's `make` (which builds `[l r]` per node) regresses bintree **~15×**
/// (0.45s → 6.4s) and inlining its `nth`-walking `check` ~5.6× — the bigger inlined arm +
/// its larger per-engine frame, hit on ~1.6M short heap-touching activations, lose far
/// more than the per-call dispatch they remove. The inline win only materialises for
/// **pure-arithmetic/control recursion** (fib/collatz/pfib keep their ~1.8×, no heap work).
#[cfg(feature = "jit")]
pub(crate) fn node_touches_heap(node: &Node) -> bool {
    match node {
        // Allocating literals: `[..]` (bintree's `make`), `{..}`.
        Node::Vector(_) | Node::Map(_) => true,
        // `cons` and `nth`/`vector-ref`; the table ops reconstruct/store values.
        Node::Prim2 {
            op: PrimOp::VectorRef | PrimOp::Cons | PrimOp::TableHas | PrimOp::TableGet,
            ..
        } => true,
        Node::Prim3 { .. } => true,
        // `first`/`rest` (car/cdr) dereference a pair handle — heap reads.
        // `nil?`/`pair?` are tag-only checks; `sqrt` is pure float math.
        Node::Prim1 {
            op: PrimOp1::First | PrimOp1::Rest,
            ..
        } => true,
        Node::Prim1 {
            op:
                PrimOp1::IsNil | PrimOp1::IsPair | PrimOp1::IsEmpty | PrimOp1::Sqrt | PrimOp1::TypeOf,
            ..
        } => false,
        Node::Const(_) | Node::Local(_) | Node::Global(_) | Node::GlobalIc { .. } => false,
        Node::If(a, b, c) => node_touches_heap(a) || node_touches_heap(b) || node_touches_heap(c),
        Node::Do(xs) => xs.iter().any(node_touches_heap),
        Node::Call { callee, args, .. } => {
            node_touches_heap(callee) || args.iter().any(node_touches_heap)
        }
        Node::SelfCall { args, .. } => args.iter().any(node_touches_heap),
        Node::LetBind { binds, body } => {
            binds.iter().any(|(_, n)| node_touches_heap(n)) || node_touches_heap(body)
        }
        Node::MakeClosure { captures, .. } => captures.iter().any(|(_, n)| node_touches_heap(n)),
        Node::Prim2 { a, b, .. } => node_touches_heap(a) || node_touches_heap(b),
        Node::TryCatch { body, handler, .. } => {
            node_touches_heap(body) || node_touches_heap(handler)
        }
    }
}

#[cfg(feature = "jit")]
pub(crate) fn self_inline_probe(
    body: &Node,
    defn_name: Symbol,
    nrequired: usize,
    m: usize,
) -> Option<usize> {
    if !self_inline_enabled() {
        return None;
    }
    // Frame-reuse self-calls and nested closures are incompatible with naive slot
    // shifting; skip an oversized body to avoid blow-up.
    if node_any(body, &|n| matches!(n, Node::SelfCall { .. }))
        || node_any(body, &|n| matches!(n, Node::MakeClosure { .. }))
        || node_count(body) > SELF_INLINE_MAX_BODY
    {
        return None;
    }
    // Inline ONLY pure-arithmetic/control recursion. A heap-touching body (builds `[..]`/
    // `{..}`, `cons`, `nth`, `first`/`rest`) regresses when inlined — the bigger arm + frame
    // on millions of alloc/walk activations costs more than the dispatch it removes
    // (bintree's `make` ~15×, `check` ~5.6×; see [`node_touches_heap`], devlog 2026-06-17).
    if node_touches_heap(body) {
        return None;
    }
    // Splice the body (shared with `rederive_inlined_body` so they can't diverge) to count
    // blocks / the new max. Discarded — the VM keeps the original small layout.
    let (clone, next_block) = build_inlined_body(body, defn_name, nrequired, m)?;
    let inline_max = m * next_block;
    // The inlined frame must also reserve the inlined chunk's call-result spill slots
    // (above `inline_max`) — exactly as `compile_arm` adds `jit_spill_reserve` to the
    // original `nslots`. The inlined body has MORE non-tail calls (the spliced leaf
    // calls), so it needs at least as much. Compile the spliced chunk to measure it; if
    // it doesn't lower to a chunk, the inliner can't help — bail (the small arm tiers).
    let spliced_chunk = compile_chunk(&clone)?;
    let inline_nslots = inline_max + jit_spill_reserve(&spliced_chunk.code);
    if std::env::var("BROOD_INLINE_DBG").is_ok() {
        eprintln!(
            "[inline-dbg] probe {} nrequired={} m={} depth={} blocks={} new_max={} inline_nslots={}",
            crate::core::value::symbol_name(defn_name),
            nrequired,
            m,
            self_inline_depth(),
            next_block - 1,
            inline_max,
            inline_nslots
        );
    }
    Some(inline_nslots)
}

/// Re-derive the inlined body fresh from `body` (the small original), for the JIT to
/// lower as the deferred upgrade. Mirrors `self_inline_probe`'s mutation on a fresh clone
/// of `body`, then `m * stride` placement — so the result is bit-identical to what the
/// probe measured. Returns the spliced `Node` (or `None` if it no longer qualifies, which
/// can't happen for an arm whose probe set `inline_name`). The caller compiles it to a
/// chunk and lowers against `inline_nslots`.
#[cfg(feature = "jit")]
pub(crate) fn rederive_inlined_body(
    body: &Node,
    defn_name: Symbol,
    nrequired: usize,
    m: usize,
) -> Option<Node> {
    // Same splice the probe measured (both go through `build_inlined_body`), so the
    // re-derived body's frame is exactly `inline_nslots`.
    Some(build_inlined_body(body, defn_name, nrequired, m)?.0)
}

// ===================== leaf-callee inlining (BROOD_NO_LEAF_INLINE opts out) ==============
//
// The self-inliner's sibling for *different* callees (`docs/jit-optimizing-tier.md`
// Phase 2): a non-tail call to a statically-known, small, calls-free top-level `defn`
// (`(add1 n)`, `(sq x)`, a scalar predicate) is replaced by the callee's body spliced
// above the caller's frame — removing that call's entire protocol (frame setup +
// dispatch + link trampoline). Derivation happens ONCE, at arm-compile time (the only
// point with `&Heap` access to resolve the callee symbols), and is stored on the arm
// ([`CompiledArm::leaf`]); it rides the existing two-stage deferred-upgrade channel.
// Hot-reload safety: the stored derivation carries its epoch, and lowering refuses any
// other epoch — a `def` between derivation and lowering (or after install, via the
// per-entry `compile_epoch` guard) always wins.

/// Is leaf-callee inlining enabled? **Default ON** since 2026-07-19 (measured: boot,
/// `require`-heavy loads, `nest check`, the in-language suite, and every benchmark row
/// flat; scalar-helper loops ~30%, type-predicate dispatch a further ~8% on top of
/// `PrimOp1::TypeOf`). `BROOD_NO_LEAF_INLINE=1` opts out — the A/B / bisect lever,
/// like `BROOD_NO_INLINE` for the self-inliner.
#[cfg(feature = "jit")]
pub(crate) fn leaf_inline_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("BROOD_NO_LEAF_INLINE").is_none())
}

/// Largest callee body (node count) worth splicing. Leaf helpers are tiny; a larger
/// body's call protocol is proportionally cheaper and the splice bloats the caller.
#[cfg(feature = "jit")]
pub(crate) const LEAF_INLINE_MAX_BODY: usize = 24;

/// Most call sites spliced per caller arm — bounds the frame + body growth.
#[cfg(feature = "jit")]
pub(crate) const LEAF_INLINE_MAX_BLOCKS: usize = 8;

/// Does `body` qualify as a spliceable **leaf**? No calls of any kind (so no tail
/// flags to demote and no recursion), no closure creation, no `try` (its handler
/// protocol is frame-relative), no global reads (their IC sites belong to the callee's
/// arm), no RUNTIME-handle consts (the stored derivation is never rewritten by
/// `runtime_collect` — epoch gating makes that safe, but excluding them keeps the
/// stored bits inert), and small. Prims, locals, consts, `if`/`do`/`let`, vector/map
/// literals are all fine.
#[cfg(feature = "jit")]
pub(crate) fn leaf_body_qualifies(body: &Node) -> bool {
    fn clean(n: &Node) -> bool {
        match n {
            Node::Call { .. }
            | Node::SelfCall { .. }
            | Node::MakeClosure { .. }
            | Node::TryCatch { .. }
            | Node::Global(_)
            | Node::GlobalIc { .. } => false,
            _ => {
                let mut ok = true;
                walk_children(n, |c| ok = ok && clean(c));
                ok
            }
        }
    }
    node_count(body) <= LEAF_INLINE_MAX_BODY && clean(body) && !node_has_rt_handles(body)
}

// Reentrancy guard for `leaf_resolve_callee`: resolving a callee may COMPILE it
// (`compiled_arm_for` → `compile_closure` → `compile_arm`), and that nested compile
// must not run its own leaf probe — unguarded, the probe walks the entire call graph
// (and never terminates on mutual recursion: `a`'s probe compiles `b`, whose probe
// compiles `a`, … → the boot stack overflow this guard fixed). With the guard, a
// nested compile skips probing: depth is bounded at 1, and since a *qualifying*
// callee is calls-free, suppressing its (empty) probe changes nothing. The one wart:
// a NON-qualifying callee compiled here is cached without its own leaf metadata, so
// it forgoes its own upgrade in this process — metadata only, body/frame identical.
#[cfg(feature = "jit")]
thread_local! {
    static LEAF_RESOLVING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Resolve a call head `sym`/`argc` to a spliceable callee arm: the global must be a
/// plain fixed-arity, non-capturing closure whose selected arm's body
/// [qualifies](leaf_body_qualifies). `None` = leave the call alone.
#[cfg(feature = "jit")]
pub(crate) fn leaf_resolve_callee(
    heap: &Heap,
    sym: Symbol,
    argc: usize,
) -> Option<Arc<CompiledArm>> {
    let v = heap.env_get(heap.global(), sym)?;
    let id = match v.unpack() {
        ValueRef::Fn(id) => id,
        _ => return None,
    };
    LEAF_RESOLVING.with(|g| g.set(true));
    let arm = compiled_arm_for(heap, id, argc);
    LEAF_RESOLVING.with(|g| g.set(false));
    let arm = arm?;
    if arm.nrequired != argc
        || arm.noptional != 0
        || arm.rest_slot.is_some()
        || !arm.capture_names.is_empty()
        || !leaf_body_qualifies(&arm.body)
    {
        return None;
    }
    Some(arm)
}

/// Replace, in place, each qualifying non-tail static-head call in `node` with the
/// callee's spliced body: `LetBind { binds: [(base + k, args[k])], body:
/// shift_slots(callee_body, base) }`, where `base` starts at the caller's frame
/// high-water mark and grows by each callee's `nslots` (unlike the self-inliner's
/// uniform stride — callees have different frame sizes). Bottom-up, so a spliced
/// argument expression is never re-scanned. Returns the number of sites spliced.
#[cfg(feature = "jit")]
pub(crate) fn leaf_inline_splice(
    heap: &Heap,
    node: &mut Node,
    next_base: &mut usize,
    blocks: &mut usize,
    self_name: Option<Symbol>,
) -> usize {
    let mut count = 0;
    match node {
        Node::Const(_) | Node::Local(_) | Node::Global(_) | Node::GlobalIc { .. } => {}
        Node::If(a, b, c) => {
            count += leaf_inline_splice(heap, a, next_base, blocks, self_name);
            count += leaf_inline_splice(heap, b, next_base, blocks, self_name);
            count += leaf_inline_splice(heap, c, next_base, blocks, self_name);
        }
        Node::Do(xs) | Node::Vector(xs) => {
            for n in xs.iter_mut() {
                count += leaf_inline_splice(heap, n, next_base, blocks, self_name);
            }
        }
        Node::Map(kvs) => {
            for (k, v) in kvs.iter_mut() {
                count += leaf_inline_splice(heap, k, next_base, blocks, self_name);
                count += leaf_inline_splice(heap, v, next_base, blocks, self_name);
            }
        }
        Node::Call { callee, args, .. } => {
            count += leaf_inline_splice(heap, callee, next_base, blocks, self_name);
            for n in args.iter_mut() {
                count += leaf_inline_splice(heap, n, next_base, blocks, self_name);
            }
        }
        Node::SelfCall { args, .. } => {
            for n in args.iter_mut() {
                count += leaf_inline_splice(heap, n, next_base, blocks, self_name);
            }
        }
        Node::LetBind { binds, body } => {
            for (_, n) in binds.iter_mut() {
                count += leaf_inline_splice(heap, n, next_base, blocks, self_name);
            }
            count += leaf_inline_splice(heap, body, next_base, blocks, self_name);
        }
        Node::MakeClosure { captures, .. } => {
            for (_, n) in captures.iter_mut() {
                count += leaf_inline_splice(heap, n, next_base, blocks, self_name);
            }
        }
        Node::Prim2 { a, b, .. } => {
            count += leaf_inline_splice(heap, a, next_base, blocks, self_name);
            count += leaf_inline_splice(heap, b, next_base, blocks, self_name);
        }
        Node::Prim3 { a, b, c, .. } => {
            count += leaf_inline_splice(heap, a, next_base, blocks, self_name);
            count += leaf_inline_splice(heap, b, next_base, blocks, self_name);
            count += leaf_inline_splice(heap, c, next_base, blocks, self_name);
        }
        Node::Prim1 { a, .. } => {
            count += leaf_inline_splice(heap, a, next_base, blocks, self_name);
        }
        Node::TryCatch { body, handler, .. } => {
            count += leaf_inline_splice(heap, body, next_base, blocks, self_name);
            count += leaf_inline_splice(heap, handler, next_base, blocks, self_name);
        }
    }
    // Children handled; now this node. A tail call is left alone (replacing it would
    // change what "the last thing in the frame" is; it's also already the cheap path).
    if *blocks >= LEAF_INLINE_MAX_BLOCKS {
        return count;
    }
    if let Node::Call {
        callee,
        args,
        tail: false,
        ..
    } = node
    {
        let sym = match &**callee {
            Node::Global(s) | Node::GlobalIc { sym: s, .. } => *s,
            _ => return count,
        };
        // A self-call is the self-inliner's job (and during the defining `def` the
        // name resolves to the PREVIOUS binding — never splice that).
        if Some(sym) == self_name {
            return count;
        }
        let Some(callee_arm) = leaf_resolve_callee(heap, sym, args.len()) else {
            return count;
        };
        let base = *next_base;
        *next_base += callee_arm.nslots;
        *blocks += 1;
        let args = match node {
            Node::Call { args, .. } => std::mem::take(args),
            _ => unreachable!(),
        };
        let binds: Box<[(usize, Node)]> = args
            .into_vec()
            .into_iter()
            .enumerate()
            .map(|(k, a)| (base + k, a))
            .collect();
        *node = Node::LetBind {
            binds,
            body: Box::new(shift_slots(&callee_arm.body, base)),
        };
        count += 1;
    }
    count
}

/// Probe + derive leaf-callee inlining for a caller arm: returns the spliced body and
/// its frame size (`inline_nslots`), or `None` if nothing qualifies. The spliced
/// chunk's ops are validated against the CURRENT globals here (`chunk_ops_native`) —
/// equivalent to the tier-time validation the small chunk gets, because lowering is
/// gated to this exact epoch (any intervening `def` bumps it and the derivation is
/// refused).
#[cfg(feature = "jit")]
pub(crate) fn leaf_inline_probe(
    heap: &Heap,
    body: &Node,
    m: usize,
    self_name: Option<Symbol>,
) -> Option<(Node, usize)> {
    if !leaf_inline_enabled() {
        return None;
    }
    // Nested compile during a resolution — don't probe (see [`LEAF_RESOLVING`]).
    if LEAF_RESOLVING.with(|g| g.get()) {
        return None;
    }
    // Don't splice into an oversized caller (i-cache + lowering limits — the same
    // reasoning as the self-inliner's bound).
    if node_count(body) > SELF_INLINE_MAX_BODY {
        return None;
    }
    let mut spliced = shift_slots(body, 0);
    let mut next_base = m;
    let mut blocks = 0usize;
    let n = leaf_inline_splice(heap, &mut spliced, &mut next_base, &mut blocks, self_name);
    if n == 0 {
        return None;
    }
    let chunk = compile_chunk(&spliced)?;
    // Foreign prims arrived with the callee bodies — validate them now (see doc above).
    if !chunk_ops_native(heap, &chunk) {
        return None;
    }
    // Every non-tail call must have been spliced away: the inlined native has no
    // deopt checkpoint (see `jit_ckpt_read`), so a deopt re-runs from ip 0 — only
    // effect-free when nothing before the deopt point completed a call. A residual
    // non-tail call (an unresolvable/large callee next to a spliced one) fails the
    // derivation; the arm keeps its small native + checkpointing.
    if chunk
        .code
        .iter()
        .any(|i| matches!(i, Inst::Call { tail: false, .. }))
    {
        return None;
    }
    let nslots = next_base + jit_spill_reserve(&chunk.code);
    if std::env::var("BROOD_INLINE_DBG").is_ok() {
        eprintln!(
            "[inline-dbg] leaf probe {} sites={} m={} leaf_nslots={}",
            self_name
                .map(crate::core::value::symbol_name)
                .unwrap_or_else(|| "<anon>".into()),
            n,
            m,
            nslots
        );
    }
    Some((spliced, nslots))
}
