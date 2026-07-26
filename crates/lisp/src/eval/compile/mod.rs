//! The compiling execution engine — ADR-076, [`docs/bytecode-vm.md`].
//!
//! A **closure-compiling VM over a lexically-addressed IR**: a form compiles once
//! into a [`Node`] tree run by a trampoline ([`vm_apply`]). The crux is GC: a
//! call's frame slots are a contiguous region of the **existing** `Heap::roots`
//! operand stack, so the moving collector relocates them in place (`arena_flip`'s
//! root walk) with **no new root set** — `Node::Local(i)` reads `root_at(base+i)`.
//!
//! **The VM is the default engine** (ADR-076 Stage 3); `BROOD_VM=0` forces the
//! tree-walker. A closure is VM-compiled when it's built from the core vocabulary
//! ([`Node`] below): `if`/`do`/`let`/`letrec`/`fn`/`quote` plus calls and vector/map
//! literals, with `&optional` (nil- *or* real-default) and any capture (global *or*
//! local — Stage 2c). Because `match`/`match*`/`and`/`or` are macros that expand to
//! exactly these forms, **pattern-matching `fn`s and `match` run on the VM too** (the
//! `quote`/literal in `match*`'s no-match arm used to force them to defer). Anything
//! still outside the set — `def`/`quasiquote`/`defmacro`/`binding`, or a body built
//! from movable (conased) forms — **defers to the tree-walker** (`eval::eval`)
//! per-form, so partial compilation is always safe and the language is unchanged.
//! Macros are already expanded by this point (`eval::macros::compile` ran), so the
//! compiler never sees a macro call.
//!
//! Naming note: [`run`] runs **after** `eval::macros::compile` (macroexpand-all +
//! namespace-resolve), on the already-expanded, already-resolved form.

use smallvec::SmallVec;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicPtr, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use crate::core::heap::{EnvRoot, Heap, VmCacheKey};
use crate::core::keywords as kw;
use crate::core::value::{
    self, BigIntId, ClosureId, EnvId, MapId, NativeId, PairId, RopeId, StrId, Symbol, Value,
    ValueRef, VecId,
};
use crate::error::{LispError, LispResult, Pos};

thread_local! {
    /// Per-thread engine override for the differential test harness (and any tool
    /// that wants to pin the engine): `Some(true)` forces the VM, `Some(false)` the
    /// tree-walker, `None` defers to the cached `BROOD_VM`/default choice. Checked
    /// before the cache so it wins; only a top-level form consults it, so the cost
    /// is negligible. See [`set_forced_engine`].
    static FORCED_ENGINE: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Force (or clear) the execution engine for the current thread, overriding
/// `BROOD_VM` and the build default — lets one process run a form through *both*
/// engines (the differential harness, `crates/lisp/tests/differential.rs`).
/// `Some(true)` = VM, `Some(false)` = tree-walker, `None` = default.
pub fn set_forced_engine(choice: Option<bool>) {
    FORCED_ENGINE.with(|c| c.set(choice));
}

/// Is the compiling VM enabled? A per-thread [`set_forced_engine`] override wins;
/// otherwise **the VM is the default engine** (ADR-076 Stage 3 cutover): every build
/// runs it unless `BROOD_VM` is set to a falsy value (`0`/`false`/`off`/`no`/empty),
/// which forces the tree-walker — the one-env-var escape hatch retained for at least
/// one release. Any other `BROOD_VM` value (or none) selects the VM. The env/default
/// choice is read once and cached; it can't change mid-run, but the override can.
pub fn vm_enabled() -> bool {
    if let Some(forced) = FORCED_ENGINE.with(|c| c.get()) {
        return forced;
    }
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    fn truthy(v: &str) -> bool {
        !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "" | "0" | "false" | "off" | "no"
        )
    }
    *ON.get_or_init(|| match std::env::var("BROOD_VM") {
        Ok(v) => truthy(&v), // explicit override (BROOD_VM=0 → tree-walker)
        Err(_) => true,      // VM is the default engine
    })
}

/// "This `Node::Call` has no call-site inline cache" — the callee isn't a free
/// global reference (ADR-096).
pub const NO_SITE: u32 = u32::MAX;

mod ir;
pub use ir::{rewrite_arm_handles, Inst};
pub use ir::{
    Chunk, CompiledArm, CompiledClosure, ConstVal, HandleKind, Node, PrimOp, PrimOp1, PrimOp3,
};
// pub(super) items from ir: explicitly imported so `use ir::*` (pub-only) doesn't miss them.
// pub items re-exported above; these are pub(super) items needed internally:
use ir::{ArmSpec, ChunkExit, Step};
// NodePtr is pub in ir, but not re-exported from mod.rs — import privately:
use ir::NodePtr;

mod emit;
pub(crate) use emit::*;
mod exec_value;
pub(crate) use exec_value::*;
mod dispatch;
pub(crate) use dispatch::*;
mod exec_chunk;
pub(crate) use exec_chunk::*;
mod vm_run_bc;
pub(crate) use vm_run_bc::*;
mod inline;
pub(crate) use inline::*;
#[cfg(feature = "jit")]
mod jit_runtime;
#[cfg(feature = "jit")]
pub(crate) use jit_runtime::*;
#[cfg(test)]
mod tests;

// ===================== compiler (form → Node) =====================

/// Compile-time lexical scope: `let`/`letrec`/param binders flattened into one
/// activation frame (ADR-076 Stage 2a). Each in-scope name maps to a frame slot;
/// `next` is the next free slot and `max` is the high-water mark (= the arm's
/// `nslots`). Shadowing: `lookup` scans newest-first. `bind` claims a slot;
/// `restore` pops a scope's binders (reusing their slots — safe, the bindings are
/// dead once out of scope).
///
/// `enclosing` (Stage 2c) holds the names lexically visible from *outer* closures —
/// derived once, by walking this closure's captured env, in [`compile_closure`].
/// They aren't frame slots (they live in the captured env, reached by name via
/// `Node::Global`), but a nested `(fn …)` must still snapshot them when it captures
/// the lexical environment, so the compiler has to know which free names are
/// enclosing *lexicals* (snapshot) vs true globals (resolved live, never snapshot).
///
/// `unsafe_slots` marks frame slots that are **not yet finalized** — the binders of
/// a `letrec` whose rhs are still being compiled. A `(fn …)` that would capture one
/// can't be VM-built (a value snapshot can't express letrec's recursive
/// late-binding), so it defers to the tree-walker.
struct Scope {
    names: Vec<(Symbol, usize)>,
    next: usize,
    max: usize,
    enclosing: Vec<Symbol>,
    unsafe_slots: Vec<usize>,
    /// While compiling a `letrec` binder whose RHS is *directly* a `(fn …)`, the
    /// slot of that binder — so a nested closure capturing it recognises the
    /// **direct self-recursion** case and binds its own name to itself at build
    /// time (see [`compile_captures`]) rather than deferring. `None` everywhere
    /// else, so an ordinary capture of an in-progress letrec binder (mutual
    /// recursion) still defers.
    letrec_self: Option<usize>,
    /// `(self-name, arity)` when this arm is a plain fixed-arity local recursive
    /// helper (a `letrec` binder bound to itself — see [`compile_closure`]). A
    /// **tail** call to `self-name` with exactly `arity` args compiles to a
    /// [`Node::SelfCall`] that re-invokes the current arm directly, skipping the
    /// env-resolve + dispatch the generic call path pays per iteration. `None`
    /// for an ordinary closure (and unset while compiling a nested `(fn …)`, which
    /// gets its own scope).
    self_call: Option<(Symbol, usize)>,
}

impl Scope {
    fn new() -> Self {
        Scope {
            names: Vec::new(),
            next: 0,
            max: 0,
            enclosing: Vec::new(),
            unsafe_slots: Vec::new(),
            letrec_self: None,
            self_call: None,
        }
    }
    fn with_params(params: &[Symbol]) -> Self {
        let mut s = Scope::new();
        for &p in params {
            s.bind(p);
        }
        s
    }
    /// As [`with_params`](Self::with_params) but seeded with the enclosing lexical
    /// names a nested closure closes over (Stage 2c).
    fn with_params_enclosing(params: &[Symbol], enclosing: Vec<Symbol>) -> Self {
        let mut s = Scope::with_params(params);
        s.enclosing = enclosing;
        s
    }
    fn lookup(&self, sym: Symbol) -> Option<usize> {
        self.names
            .iter()
            .rev()
            .find(|(n, _)| *n == sym)
            .map(|&(_, slot)| slot)
    }
    fn bind(&mut self, sym: Symbol) -> usize {
        let slot = self.next;
        self.next += 1;
        if self.next > self.max {
            self.max = self.next;
        }
        self.names.push((sym, slot));
        slot
    }
    fn is_unsafe(&self, slot: usize) -> bool {
        self.unsafe_slots.contains(&slot)
    }
    /// Snapshot for scope exit: `(names-len, next-slot)`.
    fn mark(&self) -> (usize, usize) {
        (self.names.len(), self.next)
    }
    fn restore(&mut self, (names_len, next): (usize, usize)) {
        self.names.truncate(names_len);
        self.next = next;
    }
}

/// Extract a binding form's elements (`[n1, v1, n2, v2, …]`) from either a list
/// `(n1 v1 …)` or a vector `[n1 v1 …]` (both accepted in Brood binding position),
/// or `None` if it isn't one.
fn binding_elems(heap: &Heap, form: Value) -> Option<Vec<Value>> {
    match form.unpack() {
        ValueRef::Nil => Some(Vec::new()),
        ValueRef::Vector(vid) => Some(heap.vector(vid).to_vec()),
        ValueRef::Pair(_) => heap.list_to_vec(form).ok(),
        _ => None,
    }
}

/// Compile a body (a `do`-like sequence): all but the last for effect, the last
/// in `tail` position. Empty → `nil`. A single form returns that node directly.
fn compile_body(heap: &Heap, forms: &[Value], scope: &mut Scope, tail: bool) -> Option<Node> {
    if forms.is_empty() {
        return Some(const_node(heap, Value::nil()));
    }
    let n = forms.len();
    let mut nodes = Vec::with_capacity(n);
    for (i, &f) in forms.iter().enumerate() {
        nodes.push(compile_node(heap, f, scope, tail && i + 1 == n)?);
    }
    Some(if nodes.len() == 1 {
        nodes.pop().unwrap()
    } else {
        Node::Do(nodes.into_boxed_slice())
    })
}

/// Compile a `let`/`let*` (sequential) or `letrec` form to a [`Node::LetBind`], or
/// `None` (defer) if a binder isn't a plain symbol or anything fails. Pushes the
/// binders into `scope` for the body, then restores on the way out.
fn compile_let(
    heap: &Heap,
    items: &[Value],
    scope: &mut Scope,
    tail: bool,
    rec: bool,
) -> Option<Node> {
    if items.len() < 2 {
        return None;
    }
    let elems = binding_elems(heap, items[1])?;
    if elems.len() % 2 != 0 {
        return None;
    }
    let saved = scope.mark();
    let unsafe_saved = scope.unsafe_slots.len();
    let result = (|| {
        let mut binds: Vec<(usize, Node)> = Vec::with_capacity(elems.len() / 2);
        if rec {
            // letrec: pre-allocate every binder's slot (init nil) so a rhs can
            // reference any binder; then compile the rhs in order.
            let mut slots = Vec::with_capacity(elems.len() / 2);
            for pair in elems.chunks_exact(2) {
                match pair[0].unpack() {
                    ValueRef::Sym(s) => slots.push(scope.bind(s)),
                    _ => return None,
                }
            }
            // While compiling the rhs, the letrec slots aren't yet filled — a
            // nested `(fn …)` capturing one would snapshot `nil` (a value snapshot
            // can't do letrec's recursive late-binding), so mark them unsafe to
            // capture; they become safe once we reach the body (all rhs done).
            scope.unsafe_slots.extend_from_slice(&slots);
            for (pair, &slot) in elems.chunks_exact(2).zip(slots.iter()) {
                // A binder whose RHS is *directly* a `(fn …)` enables the direct
                // self-recursion path: `compile_captures` may bind that name to the
                // built closure instead of deferring. Set it only for the fn-RHS
                // case (and only across this one `compile_node`, which consumes it
                // without recursing first) so a fn nested elsewhere in a non-fn RHS
                // — e.g. `(g (fn …))`, whose binder value is the *call* result, not
                // the fn — never misclaims self-recursion.
                let saved_self = scope.letrec_self;
                scope.letrec_self = is_fn_form(heap, pair[1]).then_some(slot);
                let rhs = compile_node(heap, pair[1], scope, false);
                scope.letrec_self = saved_self;
                binds.push((slot, rhs?));
            }
            scope.unsafe_slots.truncate(unsafe_saved);
        } else {
            // let/let*: sequential — a rhs sees only earlier binders.
            for pair in elems.chunks_exact(2) {
                let name = match pair[0].unpack() {
                    ValueRef::Sym(s) => s,
                    _ => return None,
                };
                if is_fn_form(heap, pair[1]) {
                    // A fn-valued binder: pre-allocate the slot before compiling
                    // the RHS so compile_captures can route a self-reference through
                    // self_name, producing a structural env cycle. The tree-walker's
                    // let captures the scope frame by reference — env_define adds f
                    // to it after the closure is built — so the TW closure IS
                    // structurally self-referential (send rejects it). Without this
                    // path the VM closure gets env=global (no frame, no cycle), send
                    // accepts it, and the two engines diverge.
                    let slot = scope.bind(name);
                    let unsafe_before = scope.unsafe_slots.len();
                    scope.unsafe_slots.push(slot);
                    let saved_self = scope.letrec_self;
                    scope.letrec_self = Some(slot);
                    let rhs = compile_node(heap, pair[1], scope, false);
                    scope.letrec_self = saved_self;
                    scope.unsafe_slots.truncate(unsafe_before);
                    binds.push((slot, rhs?));
                } else {
                    let rhs = compile_node(heap, pair[1], scope, false)?;
                    let slot = scope.bind(name);
                    binds.push((slot, rhs));
                }
            }
        }
        let body = compile_body(heap, &items[2..], scope, tail)?;
        Some(Node::LetBind {
            binds: binds.into_boxed_slice(),
            body: Box::new(body),
        })
    })();
    scope.restore(saved);
    scope.unsafe_slots.truncate(unsafe_saved); // also undo on the early-`None` paths
    result
}

/// Is `fn_rest` (a `(fn …)` form's cdr) safe to bake into a cached [`Node`]? It
/// must be an immovable handle: the body the closure will parse from it lives there
/// for the life of the compiled body, so a movable LOCAL form (e.g. a top-level
/// freshly-read or quasiquote-built `fn`) would dangle after a collection. Such a
/// form simply defers to the tree-walker.
fn fn_rest_is_stable(v: Value) -> bool {
    match v.unpack() {
        ValueRef::Pair(p) => p.region() != value::LOCAL,
        ValueRef::Nil => true, // `(fn)` — degenerate, but stable
        _ => false,
    }
}

/// Bake a self-evaluating literal into a [`Node::Const`], guaranteeing the embedded
/// value is **immovable**. A compiled `Node` tree lives in an `Arc` *off* the GC
/// root graph, so the collector neither traces nor relocates a handle inside it: a
/// LOCAL heap handle (e.g. a freshly-read `Value::Str` in a top-level form, which
/// `run()` never `promote`s) would dangle after a collection *during that form's own
/// evaluation* and be read as freed/moved memory by a later sub-form — a
/// use-after-GC (the bug fixed 2026-05-31; it's why `(do (doc-search …) "lit")`
/// crashed under GC stress). `promote` freezes a LOCAL string/heap literal into the
/// immovable RUNTIME code region (the same freeze a `def`/`defn` body's literals
/// get) and is a no-op for inline atoms, interned keywords, and already-shared
/// PRELUDE/RUNTIME handles. **Route every literal `Const` through here** — the
/// invariant is easy to bypass with a bare `Node::Const(form)` (which is exactly how
/// the `Value::Str` arm originally introduced the bug); the sibling `MakeClosure`
/// path guards the same hazard via [`fn_rest_is_stable`] (deferring instead of
/// freezing).
fn const_node(heap: &Heap, v: Value) -> Node {
    let frozen = heap.promote(v);
    debug_assert!(
        value_is_immovable(frozen),
        "Node::Const must hold an immovable handle (the Arc'd AST is off the GC root \
         graph and can't relocate it); promote left a movable {frozen:?}"
    );
    Node::Const(ConstVal::new(frozen))
}

/// A `Value` carrying no relocatable LOCAL heap handle — an inline scalar, an
/// interned symbol/keyword, or a PRELUDE/RUNTIME handle. The postcondition
/// [`const_node`] asserts; the handle kinds mirror those [`Heap::promote`] copies
/// out of LOCAL.
///
/// Not `#[cfg(debug_assertions)]`: `debug_assert!` still *compiles* its condition
/// in release (it expands to `if cfg!(debug_assertions) { assert!(…) }` — a dead
/// branch, but the call must resolve), so gating this out breaks the release
/// build. In release the optimizer drops the never-taken branch.
fn value_is_immovable(v: Value) -> bool {
    match v.unpack() {
        ValueRef::Str(id) => id.region() != value::LOCAL,
        ValueRef::BigInt(id) => id.region() != value::LOCAL,
        ValueRef::Pair(id) => id.region() != value::LOCAL,
        ValueRef::Vector(id) => id.region() != value::LOCAL,
        ValueRef::Map(id) => id.region() != value::LOCAL,
        // A set is a `MapId` — movable when LOCAL, so it must be checked (else this
        // tripwire would wrongly pass a movable LOCAL set baked into a Const).
        ValueRef::Set(id) => id.region() != value::LOCAL,
        ValueRef::Rope(id) => id.region() != value::LOCAL,
        ValueRef::Fn(id) | ValueRef::Macro(id) => id.region() != value::LOCAL,
        // A `Range` is a `VecId` and a `Transient` a `TransientId` — both movable when
        // LOCAL, so it must be checked too (else this tripwire would wrongly pass a
        // movable LOCAL `Range` baked into a Const).
        ValueRef::Range(id) => id.region() != value::LOCAL,
        // A `SeqView` is a `VecId` too — movable when LOCAL, so it must be checked
        // (else this tripwire would wrongly pass a movable LOCAL view in a Const).
        ValueRef::SeqView(id) => id.region() != value::LOCAL,
        // Inline scalars (Int/Float/Bool/Nil), interned Sym/Keyword, and the
        // remaining handle-free kinds carry nothing the GC relocates.
        _ => true,
    }
}

/// The capture list for a nested `(fn …)` — the enclosing lexical environment it
/// closes over, snapshotted by value (Brood bindings are immutable, so this is
/// equivalent to capturing the env by reference). Each current-frame lexical maps
/// to a `Node::Local` slot read; each name inherited from an *outer* closure maps
/// to a `Node::Global` read through the current captured env. True globals are
/// **not** captured — they resolve live (late-bound) through the new closure's
/// Compile `(%try (fn () body…) (fn (e) handler…))` to a `Node::TryCatch` that runs
/// body and handler inline in the current frame, without closure allocation.
fn compile_try_catch(heap: &Heap, items: &[Value], scope: &mut Scope) -> Option<Node> {
    if items.len() != 3 {
        return None;
    }
    let thunk_items = heap.list_to_vec(items[1]).ok()?;
    let handler_items = heap.list_to_vec(items[2]).ok()?;
    if thunk_items.len() < 2 || handler_items.len() < 2 {
        return None;
    }
    if !matches!(thunk_items[0].unpack(), ValueRef::Sym(s) if value::symbol_is(s, kw::FN)) {
        return None;
    }
    if !matches!(handler_items[0].unpack(), ValueRef::Sym(s) if value::symbol_is(s, kw::FN)) {
        return None;
    }
    let thunk_params = heap.list_to_vec(thunk_items[1]).ok()?;
    let handler_params = heap.list_to_vec(handler_items[1]).ok()?;
    if !thunk_params.is_empty() || handler_params.len() != 1 {
        return None;
    }
    let evar = match handler_params[0].unpack() {
        ValueRef::Sym(s) => s,
        _ => return None,
    };
    let body = compile_body(heap, &thunk_items[2..], scope, false)?;
    let saved = scope.mark();
    let bind_slot = scope.bind(evar);
    let handler = compile_body(heap, &handler_items[2..], scope, false);
    scope.restore(saved);
    Some(Node::TryCatch {
        body: Box::new(body),
        bind_slot,
        handler: Box::new(handler?),
    })
}

/// Every symbol that appears anywhere in `body` (an over-approximation of its free
/// variables — it also includes bound/quoted/param symbols, which is harmless: capturing
/// an enclosing lexical the body never actually reads only wastes a slot, never changes
/// behaviour). [`compile_captures`] uses it to capture **only** the enclosing lexicals the
/// body could reference, instead of snapshotting the *whole* scope. That's what lets a
/// closure like `(fn () (worker))` (which mentions only the global `worker`) come out
/// **capture-free** — the precondition for the constant-closure fast path
/// ([`crate::eval::make_closure_cached`]) that stops a `spawn` fan-out re-promoting an
/// identical thunk every call. Iterative (an explicit worklist) so a deep body can't
/// overflow the compiler's stack.
fn body_symbols(heap: &Heap, body: Value) -> std::collections::HashSet<Symbol> {
    let mut out = std::collections::HashSet::new();
    let mut work = vec![body];
    while let Some(v) = work.pop() {
        match v.unpack() {
            ValueRef::Sym(s) => {
                out.insert(s);
            }
            ValueRef::Pair(p) => {
                let (h, t) = heap.pair(p);
                work.push(h);
                work.push(t);
            }
            ValueRef::Vector(vid) => work.extend(heap.vector(vid).iter().copied()),
            ValueRef::Map(mid) => heap.fold_entries(mid, &mut |k, val| {
                work.push(k);
                work.push(val);
            }),
            ValueRef::Set(sid) => heap.fold_entries(sid, &mut |k, _v| work.push(k)),
            _ => {}
        }
    }
    out
}

/// frame parent. Returns `None` (defer) if a capture would read a not-yet-finalized
/// `letrec` slot, which a value snapshot can't express. `referenced` is the set of
/// symbols the closure body mentions (see [`body_symbols`]); an enclosing lexical is
/// captured only if it appears there — capturing the entire scope otherwise both wastes
/// slots and (fatally for the constant-closure fast path) makes an unused-capture closure
/// look non-constant.
fn compile_captures(
    scope: &Scope,
    referenced: &std::collections::HashSet<Symbol>,
) -> Option<(Vec<(Symbol, Node)>, Option<Symbol>)> {
    let mut seen: Vec<Symbol> = Vec::new();
    let mut caps: Vec<(Symbol, Node)> = Vec::new();
    let mut self_name: Option<Symbol> = None;
    // Current-frame lexicals, innermost binding first (so shadowing wins).
    for &(sym, slot) in scope.names.iter().rev() {
        if seen.contains(&sym) {
            continue;
        }
        seen.push(sym);
        // Capture only lexicals the body could reference. An unreferenced binder is
        // dropped here — no wasted slot, and (crucially) an *unsafe* `letrec` binder the
        // body never touches no longer forces the whole closure to defer.
        if !referenced.contains(&sym) {
            continue;
        }
        if scope.is_unsafe(slot) {
            // An in-progress `letrec` binder. If it's the very binder this `(fn …)`
            // is the RHS of (direct self-recursion — `scope.letrec_self`), the
            // closure references *itself*: don't snapshot the slot (still nil),
            // record the name for the exec arm to bind to the built closure (the
            // tree-walker's late-bind). Any *other* unsafe binder is mutual
            // recursion a value snapshot can't express — defer the whole closure.
            if Some(slot) == scope.letrec_self {
                self_name = Some(sym);
                continue;
            }
            return None;
        }
        caps.push((sym, Node::Local(slot)));
    }
    // Lexicals inherited from outer closures — read by name from the current env.
    for &sym in scope.enclosing.iter() {
        if seen.contains(&sym) {
            continue;
        }
        seen.push(sym);
        if !referenced.contains(&sym) {
            continue;
        }
        caps.push((sym, Node::Global(sym)));
    }
    Some((caps, self_name))
}

/// Is `form` *directly* a `(fn …)` combination? Used by `letrec` to
/// gate the direct self-recursion path (only a fn-valued binder can be its own
/// recursive callee).
fn is_fn_form(heap: &Heap, form: Value) -> bool {
    if let ValueRef::Pair(p) = form.unpack() {
        if let ValueRef::Sym(h) = heap.pair(p).0.unpack() {
            return value::symbol_is(h, kw::FN);
        }
    }
    false
}

/// Compile a `(fn …)` evaluated inside a compiled body to a
/// [`Node::MakeClosure`] (Stage 2c), or `None` (defer) if it can't be VM-built. The
/// closure's *body* is not compiled here — it's compiled lazily by [`compiled_for`]
/// when the closure is first called, keyed by its RUNTIME body handle.
fn compile_make_closure(heap: &Heap, form: Value, scope: &Scope) -> Option<Node> {
    // Post-macroexpand a pattern-param / multi-clause `fn` is already lowered to
    // `match*`; a `fn` reaching here should be plain. Defer defensively otherwise.
    if crate::eval::macros::fn_needs_lowering(heap, form) {
        return None;
    }
    let fn_rest = match form.unpack() {
        ValueRef::Pair(p) => heap.pair(p).1,
        _ => return None,
    };
    // A LOCAL `fn_rest` is a `(fn …)` literal on the movable data heap — a top-level
    // inline lambda (e.g. pipeline's `(map (fn (i) (* i i)) …)`); without help its
    // whole enclosing form defers to the tree-walker. Freeze it into the immovable
    // RUNTIME code region (as `const_node` does for a literal) so the form is VM-
    // compilable. ONLY on a runtime heap: during the prelude *build* (gc disabled) a
    // macro/defn closure's `fn_rest` is also LOCAL here but is promoted by its own
    // `def` — promoting it now corrupts it mid-construction (`defn`'s `& body` went
    // unbound) — so defer there exactly as before. The baked RUNTIME handle is
    // rewritten in place under a RUNTIME compaction, like every other MakeClosure.
    let fn_rest = if fn_rest_is_stable(fn_rest) {
        fn_rest
    } else if heap.gc_enabled() {
        let promoted = heap.promote(fn_rest);
        if !fn_rest_is_stable(promoted) {
            return None;
        }
        promoted
    } else {
        return None;
    };
    // Capture only the enclosing lexicals this closure's body could reference (over-
    // approximated by every symbol appearing in `fn_rest` — its params + body), not the
    // whole scope. `fn_rest` is the immovable RUNTIME `(params . body)`, so the walk is safe.
    let referenced = body_symbols(heap, fn_rest);
    let (captures, self_name) = compile_captures(scope, &referenced)?;
    Some(Node::MakeClosure {
        fn_rest: ConstVal::new(fn_rest),
        captures: captures.into_boxed_slice(),
        self_name,
    })
}

/// Resolve a 2-arg call head `h` to a core inlinable [`PrimOp`] plus the arg-map
/// that routes the call's operands to the underlying `%`-primitive (perf #1), or
/// `None` if `h` isn't (currently) one. `h` may bind the primitive **directly** (a
/// `Value::Native`, map `[0,1]`) or — the common case — be a prelude wrapper
/// (`+`/`<`/`>`…) whose 2-arg arm is a pure passthrough to the `%`-native; that one
/// hop is followed via [`crate::eval::passthrough_arm`], inheriting its arg-map so
/// the `>`/`>=` wrappers (which forward to `%lt`/`%le` with swapped args) inline
/// too. Read against the live global env, so a user who has redefined the operator
/// away from the builtin simply doesn't match (and the call compiles normally).
fn resolve_prim(heap: &Heap, h: Symbol) -> Option<(PrimOp, [usize; 2])> {
    let v = heap.env_get(heap.global(), h)?;
    // The canonical prelude `nth`: `(nth v i)` on a vector is a bounds-checked
    // slab read, so inline it as `VectorRef` — the call's own `head` (`nth`) drives
    // the deopt, so the list / out-of-range / explicit-default cases dispatch the
    // real `nth` unchanged. Guarded by region: a user `(def nth …)` rebinds `nth`
    // to a non-PRELUDE closure, which fails this check, so the inline cleanly
    // disables (and the same epoch guard that protects every other inlined prim
    // re-validates here on a redefinition).
    if value::symbol_is(h, "nth") {
        return match v.unpack() {
            ValueRef::Fn(id) if id.region() == crate::core::value::PRELUDE => {
                Some((PrimOp::VectorRef, [0, 1]))
            }
            _ => None,
        };
    }
    let (nid, map): (NativeId, [usize; 2]) = match v.unpack() {
        ValueRef::Native(id) => (id, [0, 1]),
        ValueRef::Fn(id) => {
            let (inner_head, m) = crate::eval::passthrough_arm(heap, id, 2)?;
            if m.len() != 2 {
                return None;
            }
            let inner = match inner_head.unpack() {
                ValueRef::Sym(s) => heap.env_get(heap.global(), s)?,
                _ => inner_head,
            };
            match inner.unpack() {
                ValueRef::Native(id) => (id, [m[0], m[1]]),
                _ => return None,
            }
        }
        _ => return None,
    };
    let op = PrimOp::from_native_name(&heap.native(nid).name)?;
    Some((op, map))
}

/// Resolve a fold *reducer value* `f` to an inlinable associative [`PrimOp`]
/// (`+`/`*` only — the cases a counted range fold can run without a per-element
/// `apply`). The sibling of [`resolve_prim`], but it starts from the reducer
/// value `reduce`/`fold` actually hold (a `Native`, or the prelude `+`/`*`
/// closure) rather than a head symbol, and accepts only the in-order arg-map
/// `[0, 1]` so a swapped wrapper (`>` → `%lt`) can never be misread as a fold.
/// Read against the live global env, so a redefined `+` simply doesn't match.
pub fn reduce_prim_op(heap: &Heap, f: Value) -> Option<PrimOp> {
    let nid = match f.unpack() {
        ValueRef::Native(id) => id,
        ValueRef::Fn(id) => {
            let (inner_head, m) = crate::eval::passthrough_arm(heap, id, 2)?;
            if m.len() != 2 || m[0] != 0 || m[1] != 1 {
                return None;
            }
            match inner_head.unpack() {
                ValueRef::Sym(s) => match heap.env_get(heap.global(), s)?.unpack() {
                    ValueRef::Native(id) => id,
                    _ => return None,
                },
                ValueRef::Native(id) => id,
                _ => return None,
            }
        }
        _ => return None,
    };
    let op = PrimOp::from_native_name(&heap.native(nid).name)?;
    matches!(op, PrimOp::Add | PrimOp::Mul).then_some(op)
}

/// Apply an inlinable 2-ary [`PrimOp`] to a single `(x, y)` pair from outside the
/// bytecode loop (the `range_reduce` fast path). `Ok(Some(v))` when handled inline;
/// `Ok(None)` to defer to the real reducer (i64 overflow → BigInt, or a
/// Float/BigInt operand the scalar path doesn't own) so results stay bit-identical.
pub fn prim_apply_step(op: PrimOp, x: Value, y: Value) -> Result<Option<Value>, LispError> {
    prim_apply(op, x, y)
}

/// Tighter variant of [`prim_apply_step`] for the range-reduce hot path: both
/// operands are already `i64` (range element + integer accumulator), no Value
/// boxing involved. Returns the next `i64` accumulator, or `None` on overflow
/// (caller must fall back to the full `prim_apply_step` / `eval_apply` path).
/// Only covers `Add` and `Mul` since those are the only ops [`reduce_prim_op`]
/// admits.
#[inline]
pub fn prim_apply_int_step(op: PrimOp, a: i64, b: i64) -> Option<i64> {
    match op {
        PrimOp::Add => a.checked_add(b),
        PrimOp::Mul => a.checked_mul(b),
        _ => None,
    }
}

/// Resolve a 1-arg call head `h` to a core inlinable [`PrimOp1`], or `None` if it
/// isn't one. Unlike [`resolve_prim`] there's no passthrough hop: `first`/`rest`
/// are bound directly to their natives. Read against the live global env, so a
/// redefinition simply doesn't match.
fn resolve_prim1(heap: &Heap, h: Symbol) -> Option<PrimOp1> {
    // The canonical prelude `sqrt` (same discipline as `nth` in `resolve_prim`): only
    // the untouched PRELUDE closure inlines — a user `(def sqrt …)` rebinds to a
    // non-PRELUDE value, so the inline cleanly disables (and the epoch guard
    // re-validates on any redefinition). The inline handles ONLY x > 0; zero,
    // negatives (the wrapper's error), NaN, and bignums dispatch the real wrapper.
    if value::symbol_is(h, "sqrt") {
        return match heap.env_get(heap.global(), h)?.unpack() {
            ValueRef::Fn(id) if id.region() == crate::core::value::PRELUDE => Some(PrimOp1::Sqrt),
            _ => None,
        };
    }
    match heap.env_get(heap.global(), h)?.unpack() {
        ValueRef::Native(id) => PrimOp1::from_native_name(&heap.native(id).name),
        _ => None,
    }
}

/// Compile an already-expanded, already-resolved `form` against the lexical
/// `scope`. `tail` is whether this form is in tail position. Returns `None` when
/// the form uses anything outside the VM's vocabulary (the caller then defers the
/// whole closure to the tree-walker).
fn compile_node(heap: &Heap, form: Value, scope: &mut Scope, tail: bool) -> Option<Node> {
    match form.unpack() {
        // Self-evaluating literals. `const_node` freezes any embedded heap handle
        // into the immovable RUNTIME region — load-bearing for `Value::Str` (a LOCAL
        // string baked raw into the off-GC-graph AST is the use-after-GC class; see
        // `const_node`), a no-op for the inline/interned atoms.
        ValueRef::Int(_)
        | ValueRef::BigInt(_)
        | ValueRef::Float(_)
        | ValueRef::Bool(_)
        | ValueRef::Nil
        | ValueRef::Str(_)
        | ValueRef::Keyword(_) => Some(const_node(heap, form)),

        // A name: a local frame slot if bound, else a global reference with a
        // read IC (ADR-096).
        ValueRef::Sym(s) => match scope.lookup(s) {
            Some(slot) => Some(Node::Local(slot)),
            None => Some(Node::GlobalIc {
                sym: s,
                site: heap.vm_gsite_alloc(),
            }),
        },

        // A combination — a special form we handle (`if`/`do`) or a function call.
        ValueRef::Pair(_) => {
            let items = heap.list_to_vec(form).ok()?;
            let head = *items.first()?;
            if let ValueRef::Sym(h) = head.unpack() {
                if value::symbol_is(h, kw::IF) {
                    // (if cond then) or (if cond then else)
                    if items.len() != 3 && items.len() != 4 {
                        return None;
                    }
                    let cond = compile_node(heap, items[1], scope, false)?;
                    let then = compile_node(heap, items[2], scope, tail)?;
                    let els = match items.get(3) {
                        Some(&e) => compile_node(heap, e, scope, tail)?,
                        None => const_node(heap, Value::nil()),
                    };
                    return Some(Node::If(Box::new(cond), Box::new(then), Box::new(els)));
                }
                if value::symbol_is(h, kw::DO) {
                    return compile_body(heap, &items[1..], scope, tail);
                }
                if value::symbol_is(h, kw::QUOTE) {
                    // Quoted data → one immovable `Const` (`const_node` promotes the
                    // datum into the shared RUNTIME region). Unblocks any body that
                    // quotes data — notably match*'s no-match arm,
                    // `(throw [:match-error (quote :ctx) m (quote pats)])`, which had
                    // been forcing every non-total `match` / pattern-dispatch `fn`
                    // onto the tree-walker.
                    //
                    // `(quote a b)` is malformed — the tree-walker rejects it with an
                    // arity error. Defer the whole closure so both engines agree;
                    // compiling only `a` here would silently drop the tail.
                    if items.len() != 2 {
                        return None;
                    }
                    return Some(const_node(heap, items[1]));
                }
                // `let` is sequential; `letrec` pre-allocates all slots.
                if value::symbol_is(h, kw::LET) {
                    return compile_let(heap, &items, scope, tail, false);
                }
                if value::symbol_is(h, kw::LETREC) {
                    return compile_let(heap, &items, scope, tail, true);
                }
                // `(fn …)` inside a compiled body (Stage 2c): build a closure
                // capturing a flat snapshot of the enclosing lexicals.
                if value::symbol_is(h, kw::FN) {
                    return compile_make_closure(heap, form, scope);
                }
                // `(%try (fn () body…) (fn (e) handler…))` — inline try/catch:
                // run body and handler in the current frame without closure allocation.
                if value::symbol_is(h, kw::TRY_PRIM) {
                    if let Some(node) = compile_try_catch(heap, &items, scope) {
                        return Some(node);
                    }
                    // Non-canonical shape: fall through to generic call (try_catch native handles it)
                }
                // Any *other* special form (`def`/`quasiquote`/`binding`) is outside
                // the VM's vocabulary — defer the whole closure to the tree-walker.
                // (`if`/`do`/`let`/`letrec`/`fn`/`quote` are handled above;
                // `defmacro`/`and`/`or`/`match`/`match*` aren't special forms — they're
                // macros, already expanded to these core forms by the compile pass.)
                if crate::eval::is_special_form(h) {
                    return None;
                }
                // A call whose head is an (as-yet-)**unexpanded macro**. The compile
                // pass (`macroexpand_all`) expands macros that are already defined,
                // but a macro **defined after** the closure — a forward reference, or
                // a prelude fn using a macro defined later in the prelude (e.g.
                // `sleep` calls `receive`) — can't be expanded then, so it survives
                // verbatim in the stored body. The VM only runs *expanded* forms (and
                // would otherwise compile the macro's argument syntax — pin patterns,
                // `~`-unquotes — as ordinary calls), so defer the whole closure to the
                // tree-walker, which expands macros lazily at eval time. Macros live
                // in the global table; a locally-bound head can't be one.
                if scope.lookup(h).is_none()
                    && crate::eval::macros::macro_head_id(heap, heap.global(), h).is_some()
                {
                    return None;
                }
                // Primitive inlining (perf #1): a 2-arg call whose head is a free
                // (non-shadowed) reference resolving — through at most one passthrough
                // hop — to a core numeric/comparison primitive compiles to a
                // `Node::Prim2`. The `(Int, Int)` case then runs inline in `exec_node`,
                // skipping the global lookup, passthrough redirect, `compiled_for`
                // cache hit, arity check, and native dispatch the generic call path
                // pays per operator per iteration. Guarded by the global epoch so a
                // redefinition of the operator cleanly falls back (see `Node::Prim2`).
                // 1-ary sequence primitives (`first`/`rest`) inline the same way
                // (ADR-096) — the list-iteration workhorses of every prelude
                // sequence fn.
                if items.len() == 2 && scope.lookup(h).is_none() {
                    if let Some(op) = resolve_prim1(heap, h) {
                        let a = compile_node(heap, items[1], scope, false)?;
                        return Some(Node::Prim1 {
                            op,
                            a: Box::new(a),
                            head: h,
                            guard: AtomicU64::new(heap.global_epoch()),
                            pos: heap.form_pos_only(form),
                        });
                    }
                }
                if items.len() == 3 && scope.lookup(h).is_none() {
                    if let Some((op, map)) = resolve_prim(heap, h) {
                        let a = compile_node(heap, items[1], scope, false)?;
                        let b = compile_node(heap, items[2], scope, false)?;
                        // `a`'s value needs a root slot across `b`'s eval only
                        // if `b` can reach a safepoint (see the field doc).
                        let broot = !matches!(
                            b,
                            Node::Const(_)
                                | Node::Local(_)
                                | Node::Global(_)
                                | Node::GlobalIc { .. }
                        );
                        return Some(Node::Prim2 {
                            op,
                            a: Box::new(a),
                            b: Box::new(b),
                            map: [map[0] as u8, map[1] as u8],
                            head: h,
                            guard: AtomicU64::new(heap.global_epoch()),
                            pos: heap.form_pos_only(form),
                            broot,
                        });
                    }
                }
                // 3-arg inlinable primitive (`table-put`): same guard discipline as the
                // 2-arg prims; only a direct-native head qualifies (no wrapper to follow).
                if items.len() == 4 && scope.lookup(h).is_none() {
                    if let Some(op3) = resolve_prim3(heap, h) {
                        let a = compile_node(heap, items[1], scope, false)?;
                        let b = compile_node(heap, items[2], scope, false)?;
                        let c = compile_node(heap, items[3], scope, false)?;
                        return Some(Node::Prim3 {
                            op: op3,
                            a: Box::new(a),
                            b: Box::new(b),
                            c: Box::new(c),
                            head: h,
                            guard: AtomicU64::new(heap.global_epoch()),
                            pos: heap.form_pos_only(form),
                        });
                    }
                }
                // N-ary associative arithmetic (`(+ a b c …)`, `(* …)`) whose head is a
                // free reference to the prelude operator: left-fold into nested 2-ary
                // `Prim2` so each step inlines to a native add/mul (and the whole arm can
                // tier), instead of dispatching the variadic prelude `fold` once per call
                // (e.g. bintree's `(+ 1 (check …) (check …))`). Left-fold matches the
                // prelude's own `fold`; each `Prim2(Add/Mul)` deopts on i64 overflow exactly
                // as `%add`/`%mul` promote to BigInt, so results stay identical. Restricted
                // to the associative reducers with the in-order map `[0,1]` — never a
                // comparison (`<`/`=` chain pairwise, not fold) or a swapped wrapper.
                if items.len() > 3 && scope.lookup(h).is_none() {
                    if let Some((op, [0, 1])) = resolve_prim(heap, h) {
                        if matches!(op, PrimOp::Add | PrimOp::Mul) {
                            let mut acc = compile_node(heap, items[1], scope, false)?;
                            for &arg in &items[2..] {
                                let b = compile_node(heap, arg, scope, false)?;
                                let broot = !matches!(
                                    b,
                                    Node::Const(_)
                                        | Node::Local(_)
                                        | Node::Global(_)
                                        | Node::GlobalIc { .. }
                                );
                                acc = Node::Prim2 {
                                    op,
                                    a: Box::new(acc),
                                    b: Box::new(b),
                                    map: [0, 1],
                                    head: h,
                                    guard: AtomicU64::new(heap.global_epoch()),
                                    pos: heap.form_pos_only(form),
                                    broot,
                                };
                            }
                            return Some(acc);
                        }
                    }
                }
            }
            // Direct `letrec` self-recursive tail call (the self-call optimization):
            // a tail call whose head is this closure's own self-name, not shadowed by
            // a local, with exactly the arm's arity. Re-runs the current arm via the
            // trampoline without resolving the callee or dispatching. A non-tail
            // self-call, a shadowed name, or a mismatched arity falls through to the
            // regular env-resolved path below (still correct).
            if tail {
                if let (ValueRef::Sym(h), Some((name, arity))) = (head.unpack(), scope.self_call) {
                    if h == name && scope.lookup(h).is_none() && items.len() - 1 == arity {
                        let mut args = Vec::with_capacity(arity);
                        for &a in &items[1..] {
                            args.push(compile_node(heap, a, scope, false)?);
                        }
                        return Some(Node::SelfCall {
                            args: args.into_boxed_slice(),
                            pos: heap.form_pos_only(form),
                        });
                    }
                }
            }
            // Function call: compile the callee and every argument (value position).
            // A free-symbol head compiles to a plain `Node::Global` (not a
            // `GlobalIc`): the call's own site IC below caches the head's full
            // resolution, so a read IC there would be redundant (and waste a site).
            let callee = match head.unpack() {
                ValueRef::Sym(h) if scope.lookup(h).is_none() => Node::Global(h),
                _ => compile_node(heap, head, scope, false)?,
            };
            let mut args = Vec::with_capacity(items.len() - 1);
            for &a in &items[1..] {
                args.push(compile_node(heap, a, scope, false)?);
            }
            // A free-global callee gets a call-site inline-cache id (ADR-096);
            // a local/computed callee can resolve to a different function per
            // call, so it keeps the generic path.
            let site = match callee {
                Node::Global(_) => heap.vm_site_alloc(),
                _ => NO_SITE,
            };
            let (pos, file) = match heap.form_pos(form) {
                Some((p, f)) => (Some(p), f),
                None => (None, None),
            };
            #[cfg(debug_assertions)]
            if site != NO_SITE {
                heap.dbg_set_site_pos(site, pos, file.clone());
            }
            Some(Node::Call {
                callee: Box::new(callee),
                args: args.into_boxed_slice(),
                tail,
                pos,
                file,
                site,
            })
        }

        // Vector literal — evaluate each element (value position), build fresh.
        ValueRef::Vector(id) => {
            let items = heap.vector(id).to_vec();
            let mut nodes = Vec::with_capacity(items.len());
            for e in items {
                nodes.push(compile_node(heap, e, scope, false)?);
            }
            Some(Node::Vector(nodes.into_boxed_slice()))
        }
        // Map literal — evaluate each key and value (value position), build fresh.
        ValueRef::Map(id) => {
            let entries = heap.map_entries(id);
            let mut pairs = Vec::with_capacity(entries.len());
            for (k, v) in entries {
                let kn = compile_node(heap, k, scope, false)?;
                let vn = compile_node(heap, v, scope, false)?;
                pairs.push((kn, vn));
            }
            Some(Node::Map(pairs.into_boxed_slice()))
        }

        // Opaque handles, etc. — outside the VM's vocabulary.
        _ => None,
    }
}

/// Compile a closure's body to a [`CompiledArm`], or `None` if it isn't
/// VM-eligible (multi-arm with no exact arity, every arm `&optional`/`&` rest, or
/// every arm body uses a non-core form). Single-arm, exact-arity arms compile;
/// **local-capturing closures are eligible** (Stage 2c) — a free var resolves by
/// name through the closure's captured env (`Node::Global` → `env_get(genv, …)`),
/// which `vm_apply` sets to the closure's own env, so the body compiles the same
/// way whether the capture is global or local.
/// Compile one arm to a [`CompiledArm`], or `None` (defer this arm to the
/// tree-walker) if its body or any real `&optional` default uses a form outside the
/// VM vocabulary. Binds frame slots in layout order — required params, then each
/// optional (its default compiled *before* the optional's own slot is bound, so a
/// default sees the required params and earlier optionals but never itself), then
/// the `&` rest param — then compiles the body. The default nodes ride along in
/// `optional_defaults` for `push_frame` to evaluate on a missing arg.
/// Returns `true` if `node` (or any of its children) contains a
/// [`ConstVal::Handle`] or a [`Node::MakeClosure`] (whose `fn_rest` is always a
/// RUNTIME Pair handle). Used to set [`CompiledArm::has_runtime_handles`] at
/// compile time so `vm_apply` can skip `live_vm_arms` registration for pure
/// arithmetic / control-flow bodies that have nothing for `runtime_collect` to
/// rewrite.
fn node_has_rt_handles(node: &Node) -> bool {
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
fn is_elem_read(a: &Node, b: &Node, slot: usize, nelems: usize) -> Option<usize> {
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
fn walk_children<F: FnMut(&Node)>(node: &Node, mut f: F) {
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
fn walk_children_mut<F: FnMut(&mut Node)>(node: &mut Node, mut f: F) {
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
fn local_escapes(node: &Node, slot: usize, nelems: usize) -> bool {
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
fn rewrite_elem_reads(node: &mut Node, slot: usize, base: usize, nelems: usize) {
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
fn ea_scalar_replace(node: &mut Node, next_slot: &mut usize) -> bool {
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

// ============ linear map-accumulator → Table rewrite (docs/linear-map-accumulator.md) ============
//
// A self-tail-recursive fold that threads an immutable-map accumulator one update
// at a time pays an O(depth) path-copy per update (~2.25M node allocations for the
// `wordcount` benchmark). When the accumulator is provably *linear* — never
// aliased, never escapes except as the function's return — we represent it
// internally as a private `Table` (already GC-safe, mutated in place) and snapshot
// it back to an immutable map at the return. Sound because (a) the entry copies the
// input map into a fresh table the function alone owns, so callers' maps are never
// mutated, and (b) the intra-procedural reachability check below proves the slot is
// only ever a whitelisted map op's first arg, the self-call threading arg, or the
// return — exactly the "no alias analysis needed; a value is only reachable through
// references the code creates" property `local_escapes` relies on. The observable
// result is an ordinary immutable map (ADR-026 holds: the only mutable thing is a
// `Table`, never surfaced). On by default; opt out with `BROOD_LINMAP=0`.

fn compile_arm(
    heap: &Heap,
    required: &[Symbol],
    optionals: &[(Symbol, Value)],
    rest: Option<Symbol>,
    body: &[Value],
    enclosing: Vec<Symbol>,
    self_name: Option<Symbol>,
    defn_name: Option<Symbol>,
    trace_name: Option<Symbol>,
) -> Option<CompiledArm> {
    // Grab the first body form before `body` is shadowed by the compiled Node —
    // its recorded reader position carries the defining source file (`src_file`).
    let body_first_form = body.first().copied();
    let nrequired = required.len();
    let noptional = optionals.len();
    let mut scope = Scope::with_params_enclosing(&[], enclosing);
    // The self-call optimization applies only to a plain fixed-arity closure (no
    // `&optional`/`&` rest), where a tail call passing exactly `nrequired` args
    // re-runs this arm verbatim. With optionals/rest the frame-fill differs per
    // call, so such calls fall back to the regular env-resolved path (correct,
    // just unoptimized).
    if let Some(name) = self_name {
        if noptional == 0 && rest.is_none() {
            scope.self_call = Some((name, nrequired));
        }
    }
    // `defn` tail self-calls get the same inline frame-reset via SelfCall. The
    // in-flight call holds an Arc to its own compiled arm, so it correctly runs
    // the current compiled version even if the global is redefined mid-call.
    if let Some(name) = defn_name {
        if noptional == 0 && rest.is_none() {
            scope.self_call = Some((name, nrequired));
        }
    }
    for &p in required {
        scope.bind(p);
    }
    let mut optional_defaults: Vec<Option<Node>> = Vec::with_capacity(noptional);
    for (name, default) in optionals {
        // A nil default needs no eval (push_frame just leaves the slot nil); a real
        // default compiles in the current scope (required + earlier optionals bound).
        let node = match default.unpack() {
            ValueRef::Nil => None,
            _ => Some(compile_node(heap, *default, &mut scope, false)?),
        };
        optional_defaults.push(node);
        scope.bind(*name);
    }
    if let Some(r) = rest {
        scope.bind(r);
    }
    // #3 lexical addressing: bind each captured enclosing lexical to a **capture slot**
    // (right after params/optionals/rest, so `capture_base = nrequired + noptional +
    // rest_count`), so a body reference resolves to a fast `Node::Local(slot)` instead of
    // an `env_get` symbol-scan through the captured env. `push_frame` fills these slots at
    // call setup. A name already bound (a param shadows the enclosing lexical) is skipped —
    // the param wins, and `push_frame`'s by-name fill stays correct for the misaligned rest.
    let mut capture_names: Vec<Symbol> = Vec::new();
    for &name in &scope.enclosing.clone() {
        if scope.lookup(name).is_none() {
            scope.bind(name);
            capture_names.push(name);
        }
    }
    let capture_names = capture_names.into_boxed_slice();
    let mut body = compile_body(heap, body, &mut scope, true)?;
    // Escape-analysis scalar replacement (lever 2): eliminate non-escaping `(let (p […]) …)`
    // vector allocations, binding their elements to fresh slots `[scope.max ..]` and rewriting
    // `(nth p K)` to direct reads. Bumps `scope.max` for the element slots; makes the arm
    // simpler (fewer allocs, no `nth`), so it JITs better. No-op for arms without the pattern.
    ea_scalar_replace(&mut body, &mut scope.max);
    // Recursive self-inlining (Phase B, §6b — two-stage tiering, devlog 2026-06-17):
    // PROBE depth-1 inlining of a top-level no-capture recursive `defn`'s body WITHOUT
    // mutating the original. The VM keeps the original small `body`/`chunk`/`nslots`;
    // the inlined body is re-derived fresh in `jit_lower_arm` and compiled as a deferred
    // upgrade. Here we only record whether the arm qualifies + the inlined frame
    // high-water mark (`inline_nslots`), by running the inliner on a CLONE (then
    // discarding it). Gated to a clean fixed-arity layout (no `&optional`/`&` rest —
    // `M = scope.max` must be the whole frame so shifted blocks don't collide), with a
    // `defn_name` (top-level recursive, set only when the closure doesn't capture). The
    // probe enforces the rest of the gate (no `SelfCall`/`MakeClosure`, body-size bound,
    // ≥1 qualifying call). Deterministic: same arm → same shifted IR.
    #[cfg(feature = "jit")]
    let (inline_name, inline_stride, inline_nslots, leaf): (
        Option<Symbol>,
        usize,
        usize,
        Option<Box<ir::LeafInline>>,
    ) = {
        let m = scope.max;
        match defn_name {
            Some(name) if noptional == 0 && rest.is_none() => {
                match self_inline_probe(&body, name, nrequired, m) {
                    Some(inline_max) => (Some(name), m, inline_max, None),
                    // Mutually exclusive with self-inlining: the leaf derivation is
                    // stored (not re-derived), stamped with the current epoch, and
                    // rides the same deferred-upgrade channel (`inline_name` set so
                    // the swap invalidates this caller's fast links; `inline_stride`
                    // unused — the lowerer branches on `leaf` first).
                    None => match leaf_inline_probe(heap, &body, m, Some(name)) {
                        Some((spliced, leaf_nslots)) => (
                            Some(name),
                            0,
                            leaf_nslots,
                            Some(Box::new(ir::LeafInline {
                                body: spliced,
                                epoch: heap.global_epoch(),
                            })),
                        ),
                        None => (None, 0, 0, None),
                    },
                }
            }
            _ => (None, 0, 0, None),
        }
    };
    let optional_defaults = optional_defaults.into_boxed_slice();
    let has_runtime_handles =
        node_has_rt_handles(&body) || optional_defaults.iter().flatten().any(node_has_rt_handles);
    // Stage 1: try to compile the body to flat bytecode (a call-free, handle-free
    // subset for now — `compile_chunk` returns `None` otherwise, and the arm runs
    // via `exec_node` exactly as before).
    let chunk = compile_chunk(&body);
    // Line coverage (ADR-148 tier 2): register this arm's instrumented lines as they
    // are compiled. This is the report's DENOMINATOR — see `coverage.rs` for why it
    // cannot be inferred from the source text instead.
    if crate::coverage::enabled() {
        if let (Some(chunk), Some(file)) = (
            chunk.as_ref(),
            body_first_form
                .and_then(|f| heap.form_pos(f))
                .and_then(|(_, file)| file),
        ) {
            crate::coverage::note_instrumented(
                &file,
                chunk.code.iter().filter_map(|inst| match inst {
                    Inst::RecordLine(line) => Some(*line),
                    _ => None,
                }),
            );
        }
    }
    // Reserve a few extra frame slots (above the compiler's `scope.max`) when the arm
    // has ≥2 non-tail calls, so a JIT-lowered version can spill call-result handles
    // that must survive a later call's safepoint (two-call recursion: `fib`, bintree
    // `check`). The VM never references these slots; `push_frame` nil-inits them like
    // any other. Computed identically here (to size the frame) and in `jit_lower_arm`
    // (to place spills) via `jit_spill_reserve`.
    let spill_reserve = chunk.as_ref().map_or(0, |c| jit_spill_reserve(&c.code));
    // Deopt-resume checkpoint slots (see `CompiledArm::ckpt_slot`): one packed
    // journal slot + room for the deepest post-call operand stack. Reserved above
    // the spill slots; zero cost for call-free arms (`ckpt_depth` is None).
    let ckpt_depth = chunk
        .as_ref()
        .and_then(|c| jit_ckpt_depth(&c.code, defn_name));
    let (ckpt_slot, ckpt_reserve) = match ckpt_depth {
        Some(d) => ((scope.max + spill_reserve) as u32, 1 + d),
        None => (u32::MAX, 0),
    };
    // Deopt-feedback watch (see the field doc): any non-loop arm with ≥1 non-tail
    // call. Vector-op arms are watched too — nbody's `advance-body` (calls +
    // `nth`s + a vector literal) deopted on ~100% of activations and only
    // feedback can catch that; a healthy vec arm (bintree's `check`) never
    // deopts, so it pays one relaxed load per native completion.
    let deopt_watch = chunk.as_ref().is_some_and(|c| {
        c.code
            .iter()
            .any(|i| matches!(i, Inst::Call { tail: false, .. }))
            && !c.code.iter().any(|i| matches!(i, Inst::SelfCall { .. }))
    });
    let nslots_total = scope.max + spill_reserve + ckpt_reserve;
    Some(CompiledArm {
        nrequired,
        noptional,
        optional_defaults,
        rest_slot: rest.map(|_| nrequired + noptional),
        nslots: nslots_total,
        body,
        chunk,
        has_runtime_handles,
        jit_code: AtomicPtr::new(std::ptr::null_mut()),
        jit_calls: AtomicU32::new(0),
        deopt_watch,
        jit_deopts: AtomicU32::new(0),
        ckpt_slot,
        compile_epoch: AtomicU64::new(0),
        share_key: None,
        shared_published: std::sync::atomic::AtomicBool::new(false),
        fn_name: trace_name,
        // The file the body was read from — trace entries name it as the call
        // site's file (a fn's calls are in its own source). Cold: once per arm
        // compile.
        src_file: body_first_form
            .and_then(|f| heap.form_pos(f))
            .and_then(|(_, file)| file),
        capture_names,
        #[cfg(feature = "jit")]
        inline_name,
        #[cfg(feature = "jit")]
        dbg_name: defn_name,
        #[cfg(feature = "jit")]
        inline_stride,
        // Floored at the SMALL frame size: the VM/small-native frame is already
        // `nslots_total` (locals + spill + ckpt reserves), and the per-engine sizing
        // hook grows a live frame to `inline_nslots` on a post-swap entry — a smaller
        // value would make that "grow" an underflowing shrink (hit by the leaf
        // inliner, whose spliced layout can be smaller than the small layout's
        // reserves; the spliced blocks overlap the small spill/ckpt area by design —
        // each engine owns its layout exclusively per activation).
        #[cfg(feature = "jit")]
        inline_nslots: if inline_name.is_some() {
            inline_nslots.max(nslots_total)
        } else {
            inline_nslots
        },
        #[cfg(feature = "jit")]
        inline_code: AtomicPtr::new(std::ptr::null_mut()),
        #[cfg(feature = "jit")]
        inline_queued: std::sync::atomic::AtomicBool::new(false),
        #[cfg(feature = "jit")]
        inline_installed: std::sync::atomic::AtomicBool::new(false),
        #[cfg(feature = "jit")]
        leaf,
    })
}

fn compile_closure(heap: &Heap, id: ClosureId) -> Option<CompiledClosure> {
    let cl = heap.closure(id);
    // The lexical names this closure inherits from outer closures (Stage 2c) —
    // empty for a global-capturing (top-level) closure. A nested `(fn …)` in the
    // body needs these to snapshot the enclosing environment it captures.
    let enclosing: Vec<Symbol> = match cl.env {
        Some(e) if !heap.is_global(e) => heap.env_chain_names(e),
        _ => Vec::new(),
    };
    // Direct `letrec` self-recursion (the self-call optimization): a closure whose
    // captured frame binds a name to *itself* (the `env_define` the `MakeClosure`
    // self-name path installs) is a local recursive helper — `defseq`'s `--loop`,
    // a hand-written named loop. A tail call to that name can re-invoke this very
    // arm without resolving the callee through the env or any dispatch (the binding
    // is an immutable letrec slot — no late-binding/epoch concern, unlike a global
    // `defn`, which is *not* self-bound in a captured frame and so never matches
    // here). `compile_arm` turns such calls into [`Node::SelfCall`].
    let self_name: Option<Symbol> = match cl.env {
        Some(e) if !heap.is_global(e) => heap.env_frame_self_name(e, id),
        _ => None,
    };
    // `defn` tail self-calls use the same `Inst::SelfCall` inline frame-reset path as
    // letrec. The in-flight call's Arc owns the compiled arm, so it runs the current
    // compiled version even if the global is redefined; new callers see the new version.
    let defn_name: Option<Symbol> = if cl.env.is_none() { cl.name } else { None };
    // Any closure's name (top-level or not), for error stack traces (`fn_name`).
    let trace_name: Option<Symbol> = cl.name;
    // Snapshot every arm's shape + body (cloning ends the `cl` borrow), then compile
    // each via [`compile_arm`]. An arm is VM-eligible when its body — and every real
    // `&optional` default form — is core vocabulary; otherwise that arm defers
    // (`compiled: None`). Ineligible arms are still recorded so `arm_for` selection
    // stays faithful to `select_arm` (variadic/exact overlap — see ArmSpec).
    struct Src {
        required: Vec<Symbol>,
        optionals: Vec<(Symbol, Value)>, // name + default form (`Nil` = nil-default)
        rest: Option<Symbol>,
        body: Vec<Value>,
    }
    let arms_src: Vec<Src> = cl
        .arms
        .iter()
        .map(|a| Src {
            required: a.params.clone(),
            optionals: a.optionals.clone(),
            rest: a.rest,
            body: a.body.clone(),
        })
        .collect();
    let mut specs: Vec<ArmSpec> = Vec::with_capacity(arms_src.len());
    for s in arms_src {
        let nrequired = s.required.len();
        let noptional = s.optionals.len();
        let has_rest = s.rest.is_some();
        let compiled = compile_arm(
            heap,
            &s.required,
            &s.optionals,
            s.rest,
            &s.body,
            enclosing.clone(),
            self_name,
            defn_name,
            trace_name,
        )
        .map(|mut arm| {
            // Shared-JIT identity (the spawn lever, ADR-101): a simple fixed-arity
            // RUNTIME/PRELUDE closure arm has a stable, process-independent `(id, argc)`
            // key (the same key `cache_key` uses), so its JIT'd native code can be
            // shared across all of the runtime's processes instead of being recompiled
            // per process. See `CompiledArm::share_key`.
            if noptional == 0 && !has_rest && matches!(id.region(), value::RUNTIME | value::PRELUDE)
            {
                arm.share_key = Some((id.0, nrequired as u16));
            }
            Arc::new(arm)
        });
        specs.push(ArmSpec {
            nrequired,
            noptional,
            has_rest,
            compiled,
        });
    }
    // Nothing to gain if no arm compiled (and a wholly-`None` entry would just mask
    // the tree-walker on every call) — defer the closure.
    if specs.iter().all(|s| s.compiled.is_none()) {
        None
    } else {
        Some(CompiledClosure { arms: specs })
    }
}

/// A stable cache key for closure `id`, or `None` if it can't be safely cached /
/// VM-run (ADR-076 §2c(a)). A **RUNTIME** closure (top-level / promoted) is keyed
/// by its own handle `.0`, which is stable for the closure's life. A **LOCAL**
/// closure's handle index is recycled by the collector, so it's keyed instead by
/// the handle of its first body form — but only when that form lives in the
/// immovable RUNTIME code region. A LOCAL closure whose body was built from movable
/// LOCAL forms (e.g. conased by `eval`/quasiquote) has no stable key *and* would
/// put movable handles in the cached `Node` tree, so it's left to the tree-walker.
fn cache_key(heap: &Heap, id: ClosureId) -> Option<VmCacheKey> {
    match id.region() {
        value::RUNTIME | value::PRELUDE => Some(VmCacheKey::Runtime(id.0)),
        value::LOCAL => {
            // Key on the first arm's first body form. Require an allocated RUNTIME
            // handle so the key is both stable and collision-free (immediates and
            // interned symbols are shared, so they'd alias unrelated closures).
            let first = heap.closure(id).arms.first()?.body.first().copied()?;
            match first.unpack() {
                ValueRef::Pair(p) if p.region() != value::LOCAL => Some(VmCacheKey::LocalBody(p.0)),
                _ => None,
            }
        }
        _ => None, // any other region (e.g. a blob/shared handle) — not VM-cached.
    }
}

/// The compiled body for closure `id`, compiling-and-caching on first use. Keyed by
/// [`cache_key`] so a local-capturing closure is found by its RUNTIME body code,
/// not its recycled LOCAL handle. `None` (ineligible) is cached too — but only when
/// the closure *has* a stable key; an unkeyable closure simply defers each call
/// (cheap: a region check + a body-handle peek).
/// The per-call hot path: resolve `id`'s `argc` arm, cloning **only** the
/// `Arc<CompiledArm>` (not the enclosing `CompiledClosure`). On a cache hit
/// (the overwhelmingly common case — a recursive or repeated callee) this is a
/// single `vm_cache_arm` lookup + one arm clone. A miss compiles + caches the
/// closure once, then resolves the arm. `None` = no VM arm for `argc` (defer to
/// the tree-walker), identical to `compiled_for(..).and_then(|c| c.arm_for(argc))`.
fn compiled_arm_for(heap: &Heap, id: ClosureId, argc: usize) -> Option<Arc<CompiledArm>> {
    let key = cache_key(heap, id)?;
    if let Some(hit) = heap.vm_cache_arm(key, argc) {
        return hit;
    }
    // Cold: compile + cache the closure once, then take the arm.
    let compiled = compile_closure(heap, id).map(Arc::new);
    heap.vm_cache_put(key, compiled.clone());
    compiled.and_then(|cc| cc.arm_for(argc).cloned())
}

/// Compile `f`'s body NOW, without calling it, and cache the result. Returns whether
/// anything was compiled.
///
/// Exists for line coverage's denominator (ADR-148 tier 2). Arms compile LAZILY — on
/// first call, via [`compiled_arm_for`] — so the set of instrumented lines otherwise
/// contains only lines that already ran, making the ratio a tautology: a fixture whose
/// every function had run reported 100% while a deliberately-uncalled function's lines
/// were absent from BOTH halves. Forcing the compile registers those lines, so a
/// never-called function correctly reports 0%.
///
/// Only the closure's own arms are reached. A nested `(fn …)` inside a body compiles
/// when the enclosing body runs, so an unexecuted body's inner closure stays
/// unmeasured — a known under-count, and a strictly smaller one than not forcing at all.
pub fn precompile(heap: &mut Heap, f: Value) -> bool {
    let ValueRef::Fn(id) = f.unpack() else {
        return false;
    };
    let compiled = compile_closure(heap, id).map(Arc::new);
    let did = compiled.is_some();
    // Cache it if the closure is keyable, so the forced compile isn't wasted work the
    // first real call redoes. An unkeyable closure just recompiles later, as always.
    if let Some(key) = cache_key(heap, id) {
        heap.vm_cache_put(key, compiled);
    }
    did
}

/// The higher-order-fn closure-call fast path (gated). A `reduce`/`fold`/… driver calls
/// the SAME step closure once per element; the general per-call path (`apply_value` → `dispatch`)
/// re-resolves the closure's arm (`vm_cache_arm`) and re-runs the passthrough/arity matching every
/// element — ~40–50% of `pipeline`/`nqueens` per the profile, and a user-closure fold is ~60× a
/// primitive one for identical work. This resolves the arm ONCE ([`hof_resolve`]); the driver then
/// calls [`hof_apply_step`] per element, which only re-reads the (rooted, GC-current) closure for
/// its captured env and calls the cached arm via `vm_apply` — skipping the re-resolution.
///
/// **Default ON**; `BROOD_NO_HOF` opts out (the A/B lever). A modest, broad win — ~8% on
/// `nqueens`, ~19% on a light-closure `range-reduce` — for any Rust HOF driver folding a user
/// closure. (It removes dispatch's self-overhead, not the per-call `push_frame`/`vm_run_bc`
/// protocol — that's the separate lean-native-call lever.)
#[cfg(feature = "jit")]
fn hof_fast_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("BROOD_NO_HOF").is_none())
}
#[cfg(not(feature = "jit"))]
fn hof_fast_enabled() -> bool {
    std::env::var_os("BROOD_NO_HOF").is_none()
}

/// A step closure resolved once for the HOF fast path: the closure identity (re-checked per call
/// so a late-rebind falls back) + its compiled arm (GC-stable `Arc`, off the heap graph).
pub(crate) struct HofArm {
    id: ClosureId,
    arm: Arc<CompiledArm>,
}

/// Resolve `f` to a cached [`HofArm`] if it's a **plain fixed-arity-`argc` VM closure** (not a
/// thin passthrough wrapper, no optional/rest) — else `None` (the driver uses its general path).
/// Returns `None` when the gate is off. Call once, before the per-element loop.
pub(crate) fn hof_resolve(heap: &Heap, f: Value, argc: usize) -> Option<HofArm> {
    if !hof_fast_enabled() {
        return None;
    }
    let id = match f.unpack() {
        ValueRef::Fn(id) => id,
        _ => return None,
    };
    // A thin-wrapper passthrough (`>` → `%lt`, …) redirects; leave those to `dispatch`.
    if crate::eval::passthrough_arm(heap, id, argc).is_some() {
        return None;
    }
    let arm = compiled_arm_for(heap, id, argc)?;
    if arm.nrequired != argc || arm.noptional != 0 || arm.rest_slot.is_some() {
        return None;
    }
    Some(HofArm { id, arm })
}

/// Call the cached step closure on `args`. `f` is the *current* (rooted, GC-relocated) closure
/// value — re-read by the caller each element; if it no longer names the cached closure (a
/// late-rebind), returns `None` so the caller falls back to its general per-call path. Otherwise
/// runs the cached arm in the closure's captured env — via the **native fast-frame** when the arm
/// has installed, epoch-current JIT code ([`hof_apply_native`]), else via `vm_apply`.
pub(crate) fn hof_apply_step(
    heap: &mut Heap,
    hof: &HofArm,
    f: Value,
    args: &[Value],
) -> Option<LispResult> {
    let id = match f.unpack() {
        ValueRef::Fn(id) => id,
        _ => return None,
    };
    if id != hof.id {
        return None;
    }
    let cenv = heap.closure(id).env.unwrap_or_else(|| heap.global());
    // Fast-frame straight into the step's native code when installed (`nqueens`/`pipeline`:
    // the per-element step is JIT-eligible, but `vm_apply` re-enters the `vm_run_bc`
    // trampoline + `jit_tier` every element — ~25%+ of both per the profile). Falls back to
    // `vm_apply` when the arm isn't natively callable (not tiered yet / over the native cap /
    // shape) or deopts.
    #[cfg(feature = "jit")]
    if hof_native_enabled() {
        if let Some(r) = hof_apply_native(heap, &hof.arm, args, cenv) {
            return Some(r);
        }
    }
    Some(vm_apply(heap, hof.arm.clone(), args, cenv))
}

/// Run the HOF step arm via the JIT **fast-frame** protocol — stage the args + captures and jump
/// the installed native entry directly, skipping the `vm_apply` → `vm_run_bc` trampoline (frame
/// save/restore, per-loop safepoints) and the per-call `jit_tier` re-entry. Returns `Some(result)`
/// when it ran the native call (following an outcome-4 tail chain and re-running a deopt on the VM
/// itself), or `None` when the arm can't be linked (not yet native / over the native-recursion cap
/// / non-trivial shape) so the caller falls back to `vm_apply`.
///
/// Mirrors the computed-head native-link block in [`jit_dispatch_call`]: same frame setup, the same
/// `capture_value` fill (the fast frame bypasses `push_frame`, so captured lexicals must be filled
/// here), and the same 0/3/4/deopt outcome handling. `hof_resolve` already proved the arm is
/// fixed-arity-`argc` with no optionals/rest, so `capture_base == argc`.
/// Default ON; `BROOD_NO_HOF_JIT` opts out (the A/B / correctness lever for the HOF native
/// fast-frame, independent of `BROOD_NO_HOF` which disables the whole cached-arm path).
#[cfg(feature = "jit")]
fn hof_native_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("BROOD_NO_HOF_JIT").is_none())
}

#[cfg(feature = "jit")]
fn hof_apply_native(
    heap: &mut Heap,
    arm: &Arc<CompiledArm>,
    args: &[Value],
    cenv: EnvId,
) -> Option<LispResult> {
    use std::sync::atomic::Ordering::Acquire;
    let argc = args.len();
    let code = arm.jit_code.load(Acquire);
    if code.is_null() || code == crate::jit::BAILED || code == crate::jit::QUEUED {
        return None;
    }
    // Over the native-recursion cap → don't link (would overflow the native stack); let the VM
    // drain the recursion. (`hof_resolve` guaranteed nslots>0 / noptional==0 / rest none / the
    // `argc` arm; re-check the epoch here since a `def` can recompile mid-fold.)
    if heap.jit_native_depth >= JIT_NATIVE_DEPTH_LIMIT
        || !crate::eval::compile::jit_runtime::jit_native_headroom_ok(heap.jit_native_depth)
        || arm.compile_epoch.load(Acquire) != heap.global_epoch()
    {
        return None;
    }
    let nslots = arm.active_nslots();
    // Diagnostic label for the debug staged-stale report / BROOD_JIT_VERIFY (the arm's defining
    // name if known, else leave the caller's — cosmetic only).
    let dbg_sym = arm.dbg_name.unwrap_or(heap.jit_dbg_fn);
    let base = heap.roots_len();
    for &a in args {
        heap.push_root(a);
    }
    // Runtime BROOD_JIT_VERIFY: scan the staged args for a stale handle (the fast frame bypasses
    // `jit_dispatch_call`'s scan), matching `jit_run_fast_link`. NO_SITE: no call site (computed).
    if jit_verify_active() {
        jit_verify_staged(heap, base, base + argc, dbg_sym, NO_SITE, argc);
    }
    heap.extend_roots_to_nil(base + nslots);
    // Root the callee's captured env so a tenure inside the arm forwards it (the deopt path
    // below re-reads the live id from this root).
    let env_base = heap.env_roots_len();
    let env_root = heap.root_env(cenv);
    // Fill the capture slots from the captured env — the fast frame placed only params + nil.
    // `capture_base == argc` (nrequired == argc, no optionals/rest). No alloc → no GC → the
    // nil-filled body slots stay valid.
    if !arm.capture_names.is_empty() {
        let cenv_live = heap.read_root_env(env_root);
        for (k, &name) in arm.capture_names.iter().enumerate() {
            let v = heap.capture_value(cenv_live, k, name);
            heap.set_root_at(base + argc + k, v);
        }
    }
    let depth = heap.jit_native_depth;
    let saved = std::mem::replace(&mut heap.jit_call_env, env_root);
    let saved_fn = std::mem::replace(&mut heap.jit_dbg_fn, dbg_sym);
    heap.jit_native_depth = depth + 1;
    // SAFETY: `code` is a finalized `extern "C" fn(*mut Heap, base)` from `jit_lower_arm`, kept
    // for the process in `GLOBAL_JIT`; the frame is at `roots[base..]`; validated current by the
    // epoch check above.
    let f: extern "C" fn(*mut Heap, i64) -> i64 = unsafe { std::mem::transmute(code) };
    let outcome = f(heap as *mut Heap, base as i64);
    heap.jit_native_depth = depth;
    heap.jit_call_env = saved;
    heap.jit_dbg_fn = saved_fn;
    // Deopt feedback (see `jit_deopt_feedback`): the HOF step arm is the canonical
    // watched shape (nqueens' reduce closure).
    if arm.deopt_watch {
        use std::sync::atomic::Ordering::Relaxed;
        if outcome == 1 {
            jit_deopt_feedback(arm);
        } else if arm.jit_deopts.load(Relaxed) != 0 {
            arm.jit_deopts.store(0, Relaxed);
        }
    }
    // `f()` may have collected + relocated the captured env; re-read the live id before dropping
    // its root (the deopt path hands it to `vm_apply`).
    let cenv_live = heap.read_root_env(env_root);
    heap.truncate_env_roots(env_base);
    match outcome {
        0 => {
            crate::perf_bump!(jit_link_done);
            let result = heap.root_at(base);
            heap.truncate_roots(base);
            Some(Ok(result))
        }
        3 => {
            heap.truncate_roots(base);
            Some(Err(jit_take_error(heap).unwrap_or_else(|| {
                LispError::type_err("jit step deopt without a parked error")
            })))
        }
        // Tail call (4): the callee JIT'd a tail — [callee, arg0..argN] staged above its frame at
        // `[base+nslots, roots_len)`. Follow the chain rather than re-running via `vm_apply`.
        4 => {
            let staged_start = base + nslots;
            let staged_end = heap.roots_len();
            if staged_end > staged_start {
                let staged_callee = heap.root_at(staged_start);
                let staged_argc = staged_end - staged_start - 1;
                let staged_args: SmallVec<[Value; 4]> = (1..=staged_argc)
                    .map(|k| heap.root_at(staged_start + k))
                    .collect();
                heap.truncate_roots(base);
                return Some(apply_value(
                    heap,
                    staged_callee,
                    &staged_args,
                    heap.global(),
                ));
            }
            heap.truncate_roots(base);
            Some(Err(LispError::type_err(
                "jit step tail with no staged call",
            )))
        }
        // deopt (1) / preempt (2): re-run the arm on the VM. The args survive in the param slots
        // `[base, base+argc)` (GC-updated); re-read, drop the frame, and `vm_apply`.
        _ => {
            crate::perf_bump!(jit_link_rerun);
            // Deopt-resume (see `CompiledArm::ckpt_slot`): resume AT the checkpoint,
            // frame intact — never re-running side effects.
            if outcome == 1 {
                if let Some((rip, depth)) = jit_ckpt_read(heap, arm, base) {
                    return Some(vm_resume_deopt(
                        heap,
                        arm.clone(),
                        base,
                        cenv_live,
                        rip,
                        depth,
                    ));
                }
            }
            let mut argv2: SmallVec<[Value; 4]> = SmallVec::with_capacity(argc);
            for k in 0..argc {
                argv2.push(heap.root_at(base + k));
            }
            heap.truncate_roots(base);
            Some(vm_apply(heap, arm.clone(), &argv2, cenv_live))
        }
    }
}

// ===================== executor (Node → value) =====================

/// Resolve a [`Step`] to a value, running a `Tail` to completion. In value
/// positions the step is always `Done` (sub-nodes compile with `tail = false`);
/// this also makes a stray tail safe rather than a panic. A `Tail` carries its own
/// callee env (Stage 2c), so `force` needs no ambient env.
fn force(heap: &mut Heap, step: Step) -> LispResult {
    match step {
        Step::Done(v) => Ok(v),
        Step::Tail {
            compiled,
            args,
            genv,
        } => vm_apply(heap, compiled, &args, genv),
    }
}

/// Resolve a 3-arg call head to an inlinable [`PrimOp3`]. Only a **direct** native
/// binding qualifies (its one member, `table-put`, has no prelude wrapper to follow).
/// Read against the live global env — a redefined head simply doesn't match.
fn resolve_prim3(heap: &Heap, h: Symbol) -> Option<PrimOp3> {
    match heap.env_get(heap.global(), h)?.unpack() {
        ValueRef::Native(id) => PrimOp3::from_native_name(&heap.native(id).name),
        _ => None,
    }
}

/// If `form` is `(def name rhs)` (name a symbol, exactly one value), return `(name, rhs)`
/// so the driver can run `rhs` capturably and bind after. `None` for anything else — a
/// `(def name)` with no value, a `def` with a bad shape (handled by the normal `def`
/// error), or any non-`def` form.
fn def_rhs(heap: &Heap, form: Value) -> Option<(Value, Value)> {
    if !matches!(form.unpack(), ValueRef::Pair(_)) {
        return None;
    }
    let parts = heap.list_to_vec(form).ok()?;
    if parts.len() == 3
        && matches!(parts[0].unpack(), ValueRef::Sym(s) if value::symbol_is(s, kw::DEF))
        && matches!(parts[1].unpack(), ValueRef::Sym(_))
    {
        Some((parts[1], parts[2]))
    } else {
        None
    }
}

/// Bind `name` to the already-computed value `v` with the full `def` semantics, by
/// re-evaluating `(def name (quote v))` on the tree-walker — a trivial form (the RHS is a
/// literal, so it neither compiles-to-VM nor `receive`s), which reuses `def`'s naming,
/// promote-into-shared-RUNTIME, and reload diagnostics rather than re-implementing them.
fn bind_def(heap: &mut Heap, name: Value, v: Value) -> Result<(), LispError> {
    let g = heap.global();
    let base = heap.roots_len();
    heap.push_root(name);
    heap.push_root(v);
    let v = heap.root_at(base + 1);
    let quote = heap.list(vec![value::sym(kw::QUOTE), v]);
    heap.push_root(quote);
    let name = heap.root_at(base);
    let quote = heap.root_at(base + 2);
    let form = heap.list(vec![value::sym(kw::DEF), name, quote]);
    heap.push_root(form);
    let form = heap.root_at(base + 3);
    let r = crate::eval::eval(heap, form, g);
    heap.truncate_roots(base);
    r.map(|_| ())
}

/// Runaway guard for the explicit frame stack: a clean `STACK_DEPTH_EXCEEDED` once
/// the bytecode call depth crosses this many frames, replacing the native-stack byte
/// guard the `Node` engine uses (the driver doesn't grow the native stack per Brood
/// call, so unbounded non-tail recursion grows `frames` + `Heap::roots` instead).
/// Generous — the soft-memory cap (ADR-043) is the real backstop; this just turns an
/// infinite non-tail recursion into a catchable error before it exhausts memory.
const MAX_BC_FRAMES: usize = 1 << 20;

/// One suspended bytecode activation: where to resume (`ip`) and how to tear its
/// frame down. Promoted out of [`vm_run_bc`]'s body (it was a local `struct Frame`)
/// so a captured [`Suspended`] continuation can hold the whole stack. The indices
/// (`base`/`env_base`/`arm_slot`) are positions into `Heap::roots`/`env_roots`/
/// `live_vm_arms`, which stay valid across a suspend because the driver does **not**
/// unwind them when it captures (a collection while parked relocates the *values* at
/// those positions in place, keeping the indices good — ADR-100 §8).
pub(crate) struct BcFrame {
    arm: Arc<CompiledArm>,
    ip: usize,
    base: usize,
    env: EnvRoot,
    env_base: usize,
    arm_slot: usize,
    /// Persisted back-edge counter for this frame — see `exec_chunk`'s `back_edges` param.
    #[cfg(feature = "jit")]
    back_edges: u32,
}

/// A captured VM continuation — the reified call stack of a green process parked at a
/// clean `receive` (ADR-100 §8, the corosensei-removal migration). It is plain `Send`
/// data: `frames` (the pending non-tail callers) + `cur` (the frame that was running)
/// + the driver's entry marks (for unwinding on a later error) + the `receive`
/// deadline (so the scheduler arms a timer). The operand stack and frame slots it
/// references stay live on the owning process's `Heap::roots`; this struct only holds
/// the *control* state. Hand it back to [`vm_run_bc`] as `resume` to replay from the
/// suspending `%receive` call. The scheduler cutover (§8.3) stores it in place of a
/// `Coroutine`; for now only the capture→resume unit test consumes it.
pub(crate) struct Suspended {
    frames: Vec<BcFrame>,
    cur: BcFrame,
    entry_roots: usize,
    entry_env: usize,
    entry_arms: usize,
    /// The `(receive … (after ms …))` absolute wake time, or `None` to wait forever —
    /// the scheduler arms a timer from this so a parked process still fires its
    /// `after` clause.
    pub(crate) deadline: Option<std::time::Instant>,
}

/// What a [`vm_run_bc`] call produced (ADR-100 §8). A real error is the `Err` of the
/// enclosing `Result`. A **nested** run (`vm_apply`, `top_level=false`) only ever
/// produces `Done` (it can't capture across the native boundary); the other three are
/// the scheduler outcomes the **top-level body driver** reifies at its loop-top
/// safepoint in place of a coroutine yield.
pub(crate) enum VmOutcome {
    /// The body finished with this value.
    Done(Value),
    /// A clean `receive` parked: the captured continuation to store + resume on a
    /// wake (§8.2). `run_one` parks it on the mailbox.
    Suspended(Suspended),
    /// The reduction budget was exhausted at a loop-top safepoint (the state-capture
    /// analogue of `Suspend::Preempt`): captured the continuation so `run_one` can
    /// **re-enqueue** it (possibly onto another worker — live migration, §7).
    Preempted(Suspended),
    /// A hard `:kill` was pending at a loop-top safepoint (the analogue of
    /// `Suspend::Kill`): stop now, no capture — `run_one` retires the process with the
    /// mailbox's kill reason. Untrappable by construction (fires below `%try`).
    Killed,
}

/// Compile-then-run a resolved top-level `form` — the VM entry the form loops use
/// when `vm_enabled()`. A form built from the core vocabulary runs on the VM (an
/// empty lexical scope: no locals at top level); anything else defers to the
/// tree-walker. `env` is the process's global/root env.
pub fn run(heap: &mut Heap, form: Value, env: EnvId) -> LispResult {
    let mut scope = Scope::new();
    // When invoked with a *non-global* env — a `def` RHS evaluated inside a `let`,
    // e.g. `(let (me …) (def f (fn () me)))` — the form's closures must be able to
    // capture the enclosing lexicals. Seed them as `enclosing` names so a VM-compiled
    // closure snapshots them (`compile_captures` reads each via `env_get` on the live
    // env at `MakeClosure` time); without this the closure resolves them as unbound
    // globals once the lexical frame is gone (e.g. when a `def`'d closure is later
    // called, or shipped to another node). The overwhelmingly common case is
    // `env == global` (top-level forms): no lexical frames, so this is a no-op.
    if !heap.is_global(env) {
        let mut e = env;
        while !heap.is_global(e) {
            let (parent, bindings) = heap.env_frame_ref(e);
            for &(sym, _) in bindings.iter() {
                scope.enclosing.push(sym);
            }
            match parent {
                Some(p) => e = p,
                None => break,
            }
        }
    }
    match compile_node(heap, form, &mut scope, false) {
        Some(node) => {
            // A top-level `let` introduces frame slots too — give the form a frame
            // of `scope.max` nil slots (like a 0-param closure), then tear it down.
            // The top-level env is the (immovable) process global, so `root_env`
            // keeps it inline; rooting it uniformly keeps `exec_node`'s contract.
            //
            // Wrap the transient top-level node in a throwaway arm and register it as
            // LIVE: like a `vm_apply` frame, its `Const` literals are promoted RUNTIME
            // handles that a nested compaction (a sub-call into `load`/`eval`) would
            // strand — registering it lets `runtime_collect` rewrite them in place.
            let has_runtime_handles = node_has_rt_handles(&node);
            let arm = Arc::new(CompiledArm {
                nrequired: 0,
                noptional: 0,
                optional_defaults: Box::new([]),
                rest_slot: None,
                nslots: scope.max,
                body: node,
                // Top-level forms run via `exec_value` below, not the bytecode loop
                // (Stage 1 bytecode is reached only through `vm_apply`); no chunk.
                chunk: None,
                has_runtime_handles,
                jit_code: AtomicPtr::new(std::ptr::null_mut()),
                jit_calls: AtomicU32::new(0),
                deopt_watch: false,
                jit_deopts: AtomicU32::new(0),
                ckpt_slot: u32::MAX,
                compile_epoch: AtomicU64::new(0),
                share_key: None,
                shared_published: std::sync::atomic::AtomicBool::new(false),
                fn_name: None,
                src_file: None,
                capture_names: Box::new([]),
                #[cfg(feature = "jit")]
                inline_name: None,
                #[cfg(feature = "jit")]
                dbg_name: None,
                #[cfg(feature = "jit")]
                inline_stride: 0,
                #[cfg(feature = "jit")]
                inline_nslots: 0,
                #[cfg(feature = "jit")]
                inline_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
                #[cfg(feature = "jit")]
                inline_queued: std::sync::atomic::AtomicBool::new(false),
                #[cfg(feature = "jit")]
                inline_installed: std::sync::atomic::AtomicBool::new(false),
                #[cfg(feature = "jit")]
                leaf: None,
            });
            let arm_slot = if arm.has_runtime_handles {
                heap.live_arm_push(arm.clone())
            } else {
                usize::MAX
            };
            let env_base = heap.env_roots_len();
            let genv = heap.root_env(env);
            let base = heap.roots_len();
            for _ in 0..scope.max {
                heap.push_root(Value::nil());
            }
            let r = exec_value(heap, &arm.body, base, genv);
            heap.truncate_roots(base);
            heap.truncate_env_roots(env_base);
            if arm_slot != usize::MAX {
                heap.live_arm_truncate(arm_slot);
            }
            r
        }
        None => crate::eval::eval(heap, form, env),
    }
}

/// Apply a closure *value* (not a source form) to `args` through the VM when it's
/// VM-eligible, falling back to the tree-walker (`eval::apply`) otherwise — the
/// entry point for callers that hold a [`Value::Fn`] and want VM execution. A
/// spawned process's body uses this so it runs on the VM (with inlined
/// primitives) like top-level code via [`run`], instead of the tree-walker:
/// before this, `eval::apply` ran every green process tree-walked even under
/// `BROOD_VM=1`, ~4–5× slower (most of `pfib`'s gap to Elixir). `genv` is the
/// env a *native* callee runs in; a VM closure runs in its own captured env
/// (read off the closure inside `dispatch`). `tail = false`: this is a value
/// context, so any tail call is forced to completion by `force`.
pub fn apply_value(heap: &mut Heap, callee: Value, args: &[Value], genv: EnvId) -> LispResult {
    let argv: SmallVec<[Value; 4]> = args.iter().copied().collect();
    let step = dispatch(heap, callee, argv, false, genv)?;
    force(heap, step)
}

/// Apply `callee` through the active engine: the VM when enabled (a VM-eligible
/// callback runs compiled), the tree-walker under `BROOD_VM=0` (keeps the
/// differential / escape-hatch mode honest). `eval::apply` must stay pure
/// tree-walker — it's `dispatch`'s fallback, so routing it back through
/// `apply_value` would recurse. Use for once-per-call thunks (`try`, `binding`,
/// `isolate`); NOT for the `apply` builtin itself — that needs the TW's inline
/// `apply`-unfolding trampoline for O(1)-stack `(apply f …)`-driven tail recursion.
pub fn apply_engine(heap: &mut Heap, callee: Value, args: &[Value], genv: EnvId) -> LispResult {
    if vm_enabled() {
        apply_value(heap, callee, args, genv)
    } else {
        crate::eval::apply(heap, callee, args, genv)
    }
}

#[cfg(feature = "jit")]
mod jit_lower;
#[cfg(feature = "jit")]
use jit_lower::jit_ckpt_depth;
#[cfg(feature = "jit")]
pub(crate) use jit_lower::{jit_lower_arm, jit_lower_inlined_arm, jit_spill_reserve};
// When JIT is disabled, provide zero stubs so callers don't need cfg guards.
#[cfg(not(feature = "jit"))]
fn jit_spill_reserve(_code: &[Inst]) -> usize {
    0
}
#[cfg(not(feature = "jit"))]
fn jit_ckpt_depth(_code: &[Inst], _self_name: Option<Symbol>) -> Option<usize> {
    None
}

#[test]
fn test_inst_size() {
    // Not an assertion — just surfaces the IR `Inst` size in test output (a
    // regression in it shows up here). A non-zero size is guaranteed by the type.
    eprintln!("Inst size: {}", std::mem::size_of::<Inst>());
}
