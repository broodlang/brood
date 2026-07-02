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
pub use ir::{Chunk, CompiledArm, CompiledClosure, ConstVal, HandleKind, Node, PrimOp, PrimOp1};
// pub(super) items from ir: explicitly imported so `use ir::*` (pub-only) doesn't miss them.
// pub items re-exported above; these are pub(super) items needed internally:
use ir::{ArmSpec, ChunkExit, Step};
// NodePtr is pub in ir, but not re-exported from mod.rs — import privately:
use ir::NodePtr;

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

/// frame parent. Returns `None` (defer) if a capture would read a not-yet-finalized
/// `letrec` slot, which a value snapshot can't express.
fn compile_captures(scope: &Scope) -> Option<(Vec<(Symbol, Node)>, Option<Symbol>)> {
    let mut seen: Vec<Symbol> = Vec::new();
    let mut caps: Vec<(Symbol, Node)> = Vec::new();
    let mut self_name: Option<Symbol> = None;
    // Current-frame lexicals, innermost binding first (so shadowing wins).
    for &(sym, slot) in scope.names.iter().rev() {
        if seen.contains(&sym) {
            continue;
        }
        seen.push(sym);
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
    let (captures, self_name) = compile_captures(scope)?;
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
// `Table`, never surfaced). Gated behind `BROOD_LINMAP` while it stabilises.

/// Whitelisted map READ ops (return a value — safe in any position) → Table op.
fn linmap_read_op(sym: Symbol) -> Option<&'static str> {
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
fn linmap_update_op(sym: Symbol) -> Option<&'static str> {
    match value::symbol_name_opt(sym)? {
        "map-int-add" => Some("table-incr"),
        "map-dissoc" => Some("table-delete"),
        _ => None,
    }
}

/// The global symbol a call head resolves to, if it is a free-global head.
fn call_head_sym(callee: &Node) -> Option<Symbol> {
    match callee {
        Node::Global(s) => Some(*s),
        Node::GlobalIc { sym, .. } => Some(*sym),
        _ => None,
    }
}

/// Where a value flows: a "sink" is a position whose value is the accumulator's
/// linear continuation — the self-call's own arg slot, or the function's return.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LinSink {
    Return,
    SelfArg,
    No,
}

/// True iff `(args[0] == Local(s))`.
fn first_arg_is_local(args: &[Node], s: usize) -> bool {
    matches!(args.first(), Some(Node::Local(k)) if *k == s)
}

/// Reachability check: every read of slot `s` is a whitelisted map op's first arg,
/// the self-call's arg-`s` (threading), or a sink return — and update ops on `s`
/// occur only at a sink (so the new map is linearly consumed, never aliased).
/// Anything else → not linear (bail). `sink` is this position's flow role.
fn linmap_linear(node: &Node, s: usize, sink: LinSink) -> bool {
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
fn linmap_has_update(node: &Node, s: usize) -> bool {
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

fn node_has_selfcall(node: &Node) -> bool {
    if matches!(node, Node::SelfCall { .. }) {
        return true;
    }
    let mut found = false;
    walk_children(node, |c| found = found || node_has_selfcall(c));
    found
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
    if !node_has_selfcall(&node) {
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
const SELF_INLINE_MAX_BODY: usize = 64;

/// Total node count of `node` (every variant counted, children recursed).
#[cfg(feature = "jit")]
fn node_count(node: &Node) -> usize {
    let mut n = 1;
    walk_children(node, |child| n += node_count(child));
    n
}

/// True if `node` (or any descendant) is a `Node::SelfCall`.
#[cfg(feature = "jit")]
fn node_has_self_call(node: &Node) -> bool {
    matches!(node, Node::SelfCall { .. }) || {
        let mut found = false;
        walk_children(node, |c| found = found || node_has_self_call(c));
        found
    }
}

/// True if `node` (or any descendant) is a `Node::MakeClosure`.
#[cfg(feature = "jit")]
fn node_has_make_closure(node: &Node) -> bool {
    matches!(node, Node::MakeClosure { .. }) || {
        let mut found = false;
        walk_children(node, |c| found = found || node_has_make_closure(c));
        found
    }
}

/// Is `node` a non-tail self-recursive call to `defn_name` with exactly `nrequired`
/// args? The call head is a free-global reference — `compile_node` lowers a free symbol
/// in head position to `Node::Global(sym)` (never `GlobalIc`, since the call site's own
/// IC caches the resolution), so that's the only shape to match. A computed/local callee
/// (`NO_SITE`) can resolve to a different function per call and is never inlined.
#[cfg(feature = "jit")]
fn is_inlinable_self_call(node: &Node, defn_name: Symbol, nrequired: usize) -> bool {
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
fn shift_slots(node: &Node, delta: usize) -> Node {
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
fn inline_self_calls(
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
fn self_inline_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("BROOD_NO_INLINE").is_none())
}

/// Runtime JIT off-switch. **Default OFF** (the JIT runs). `BROOD_NO_JIT=1` makes
/// [`jit_tier`] never compile or run native code — every arm interprets on the VM,
/// which is the correct reference engine. Today disabling the JIT otherwise needs a
/// no-`jit`-feature *build*; this is the runtime A/B lever that lets a suspected
/// JIT-only miscompile (e.g. a use-after-GC in a render arm) be ruled in/out and
/// worked around without a rebuild. Cached once, like the other JIT levers.
#[cfg(feature = "jit")]
fn no_jit_enabled() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| std::env::var_os("BROOD_NO_JIT").is_some())
}

/// Debug bisect: `BROOD_NO_JIT_COMPUTED=1` bails (runs on the VM) any arm whose chunk
/// contains a **computed-head** non-tail call `(f …)` — the shape fold--loop / assoc--pairs
/// share, suspected in the JIT+GC value→nil/stale bug. If the repro goes clean with this set,
/// the computed-head call lowering is the culprit.
#[cfg(feature = "jit")]
fn no_jit_computed() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| std::env::var_os("BROOD_NO_JIT_COMPUTED").is_some())
}

/// Runtime JIT self-verification (`BROOD_JIT_VERIFY=1`). Runs the staged-stale-handle scan
/// on every Brood→Brood call's staged args in **any** build (not just debug-assertions) —
/// so a JIT+GC use-after-GC (bug #2) can be caught at the staging site (naming the callee +
/// the stale handle kind) in a normal `--release` binary, without a debug-armed rebuild.
/// Off by default; one cached bool check + (when on) a short scan per call.
#[cfg(feature = "jit")]
fn jit_verify_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("BROOD_JIT_VERIFY").is_some())
}

/// `BROOD_JIT_VERIFY_FN=<fn>`: log every JIT'd Brood→Brood call to `<fn>` with each staged
/// arg's *type* — the targeted, low-noise way to see a value-level corruption (e.g. a `nil`
/// staged where a number belongs: pong's `badge-ops` getting `throb=nil`). Identifies whether
/// a bad value is staged *from JIT'd code* and which arg position, without a debug rebuild.
#[cfg(feature = "jit")]
fn jit_verify_fn() -> Option<&'static str> {
    static F: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    F.get_or_init(|| std::env::var("BROOD_JIT_VERIFY_FN").ok())
        .as_deref()
}

/// Either verification mode is on (the stale-handle scan, or the targeted per-fn arg log).
#[cfg(feature = "jit")]
fn jit_verify_active() -> bool {
    jit_verify_enabled() || jit_verify_fn().is_some()
}

/// A one-word type tag for a `Value`, for the `BROOD_JIT_VERIFY_FN` arg log.
#[cfg(feature = "jit")]
fn jit_describe_value(v: Value) -> &'static str {
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
        _ => "other",
    }
}

/// Scan `roots[lo..hi]` for a stale LOCAL handle (`BROOD_JIT_VERIFY`) and, when
/// `BROOD_JIT_VERIFY_FN` names this call's callee, log every staged arg's type. `head`/`site`/
/// `argc` describe the call being staged.
#[cfg(feature = "jit")]
fn jit_verify_staged(heap: &Heap, lo: usize, hi: usize, head: Symbol, site: u32, argc: usize) {
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
fn icall_enabled() -> bool {
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
fn node_touches_heap(node: &Node) -> bool {
    match node {
        // Allocating literals: `[..]` (bintree's `make`), `{..}`.
        Node::Vector(_) | Node::Map(_) => true,
        // `cons` and `nth`/`vector-ref`.
        Node::Prim2 {
            op: PrimOp::VectorRef | PrimOp::Cons,
            ..
        } => true,
        // `first`/`rest` (car/cdr) dereference a pair handle — heap reads.
        // `nil?`/`pair?` are tag-only checks — no heap dereference.
        Node::Prim1 {
            op: PrimOp1::First | PrimOp1::Rest,
            ..
        } => true,
        Node::Prim1 {
            op: PrimOp1::IsNil | PrimOp1::IsPair | PrimOp1::IsEmpty,
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
fn self_inline_probe(body: &Node, defn_name: Symbol, nrequired: usize, m: usize) -> Option<usize> {
    if !self_inline_enabled() {
        return None;
    }
    // Frame-reuse self-calls and nested closures are incompatible with naive slot
    // shifting; skip an oversized body to avoid blow-up.
    if node_has_self_call(body)
        || node_has_make_closure(body)
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
    // Clone the body and run the inliner on the copy to count blocks / the new max.
    let mut clone = shift_slots(body, 0);
    let orig = shift_slots(body, 0);
    let mut next_block = 1usize;
    let inlined = inline_self_calls(&mut clone, &orig, defn_name, nrequired, m, &mut next_block);
    if inlined == 0 {
        return None;
    }
    // Level-2: if the level-1 result is still compact, inline its external calls too.
    // Uses the same `orig` template and a continuing `next_block`, so slot ranges stay
    // disjoint. Must mirror rederive_inlined_body exactly.
    if node_count(&clone) <= SELF_INLINE_MAX_BODY {
        inline_self_calls(&mut clone, &orig, defn_name, nrequired, m, &mut next_block);
    }
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
            "[inline-dbg] probe {} nrequired={} m={} inlined={} new_max={} inline_nslots={}",
            crate::core::value::symbol_name(defn_name),
            nrequired,
            m,
            inlined,
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
fn rederive_inlined_body(
    body: &Node,
    defn_name: Symbol,
    nrequired: usize,
    m: usize,
) -> Option<Node> {
    let mut spliced = shift_slots(body, 0);
    let orig = shift_slots(body, 0);
    let mut next_block = 1usize;
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
    // Level-2: must mirror self_inline_probe exactly.
    if node_count(&spliced) <= SELF_INLINE_MAX_BODY {
        inline_self_calls(
            &mut spliced,
            &orig,
            defn_name,
            nrequired,
            m,
            &mut next_block,
        );
    }
    Some(spliced)
}

fn compile_arm(
    heap: &Heap,
    required: &[Symbol],
    optionals: &[(Symbol, Value)],
    rest: Option<Symbol>,
    body: &[Value],
    enclosing: Vec<Symbol>,
    self_name: Option<Symbol>,
    defn_name: Option<Symbol>,
) -> Option<CompiledArm> {
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
    let (inline_name, inline_stride, inline_nslots): (Option<Symbol>, usize, usize) = {
        let m = scope.max;
        match defn_name {
            Some(name) if noptional == 0 && rest.is_none() => {
                match self_inline_probe(&body, name, nrequired, m) {
                    Some(inline_max) => (Some(name), m, inline_max),
                    None => (None, 0, 0),
                }
            }
            _ => (None, 0, 0),
        }
    };
    let optional_defaults = optional_defaults.into_boxed_slice();
    let has_runtime_handles =
        node_has_rt_handles(&body) || optional_defaults.iter().flatten().any(node_has_rt_handles);
    // Stage 1: try to compile the body to flat bytecode (a call-free, handle-free
    // subset for now — `compile_chunk` returns `None` otherwise, and the arm runs
    // via `exec_node` exactly as before).
    let chunk = compile_chunk(&body);
    // Reserve a few extra frame slots (above the compiler's `scope.max`) when the arm
    // has ≥2 non-tail calls, so a JIT-lowered version can spill call-result handles
    // that must survive a later call's safepoint (two-call recursion: `fib`, bintree
    // `check`). The VM never references these slots; `push_frame` nil-inits them like
    // any other. Computed identically here (to size the frame) and in `jit_lower_arm`
    // (to place spills) via `jit_spill_reserve`.
    let spill_reserve = chunk.as_ref().map_or(0, |c| jit_spill_reserve(&c.code));
    Some(CompiledArm {
        nrequired,
        noptional,
        optional_defaults,
        rest_slot: rest.map(|_| nrequired + noptional),
        nslots: scope.max + spill_reserve,
        body,
        chunk,
        has_runtime_handles,
        jit_code: AtomicPtr::new(std::ptr::null_mut()),
        jit_calls: AtomicU32::new(0),
        compile_epoch: AtomicU64::new(0),
        share_key: None,
        shared_published: std::sync::atomic::AtomicBool::new(false),
        capture_names,
        #[cfg(feature = "jit")]
        inline_name,
        #[cfg(feature = "jit")]
        dbg_name: defn_name,
        #[cfg(feature = "jit")]
        inline_stride,
        #[cfg(feature = "jit")]
        inline_nslots,
        #[cfg(feature = "jit")]
        inline_code: AtomicPtr::new(std::ptr::null_mut()),
        #[cfg(feature = "jit")]
        inline_queued: std::sync::atomic::AtomicBool::new(false),
        #[cfg(feature = "jit")]
        inline_installed: std::sync::atomic::AtomicBool::new(false),
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

/// Int×Int-only fast path for `prim2_inline_exec`: just the fixnum arithmetic,
/// no type dispatch, no allocation.  Marked `#[inline(always)]` because it is
/// tiny (one `match` arm per op) — LLVM constant-folds `op` at each call site
/// in `prim2_inline_exec` (itself always-inlined), emitting a single checked op
/// or compare per instruction variant.  Float, BigInt, overflow, Cons, and Div
/// all return `None`; the caller falls through to `prim_apply`.
#[inline(always)]
fn prim2_int_fast(op: PrimOp, a: i64, b: i64) -> Option<Value> {
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
        PrimOp::Cons | PrimOp::Div | PrimOp::VectorRef => None,
    }
}

/// The inline fast path for a [`Node::Prim2`] (perf #1): handle the `(Int, Int)`
/// case of `op` directly, returning `Ok(Some(v))` when done inline, or `Ok(None)`
/// to defer to the real `%`-primitive — for any non-`(Int, Int)` operands (float
/// coercion, structural `=`, bignum operands, the canonical type errors), the
/// division edges, **and the i64-overflow cases**, which the native now resolves
/// by promoting to a bignum (ADR bignums) rather than erroring. Needs no heap:
/// the inline result is a scalar, so nothing is allocated and no GC can intervene.
fn prim_apply(op: PrimOp, x: Value, y: Value) -> Result<Option<Value>, LispError> {
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
        // `%div` returns an Int only when it divides evenly (matching `prim_div`);
        // a remainder means a Float result, which the native builds.
        PrimOp::Div => match (a.checked_rem(b), a.checked_div(b)) {
            (Some(0), Some(q)) => Value::int(q),
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
        PrimOp::Cons | PrimOp::VectorRef => return Ok(None),
    };
    Ok(Some(v))
}

/// The float fast path of [`prim_apply`] (ADR-096): both operands `Int`/`Float`
/// with at least one `Float` — exactly the shapes `num_bin`/`prim_lt`'s float
/// arms handle with a plain `f64` op after an exact `i64 as f64` coercion.
/// Everything else (`BigInt` operands, structural `=` on floats, `rem`/`quot`'s
/// numeric edges, division by zero) returns `None` so the real native owns the
/// result and the error messages.
fn prim_apply_float(op: PrimOp, x: Value, y: Value) -> Option<Value> {
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
fn prim2_inline_exec(
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
        None => Ok(None), // overflow or other deferred edge → fallback
    }
}

/// Slow-path dispatch shared by all three `Prim2` bytecode handlers.
/// Operands are already rooted at `[save]` and `[save+1]`; this function looks
/// up `head`, dispatches, truncates back to `save`, and returns the result.
/// Marked `inline(never)` to keep the cold path out of the hot dispatch loop.
#[inline(never)]
fn prim2_dispatch_rooted(
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

/// Execute one node in **value position** — operands, call arguments, literal
/// elements, binding right-hand sides: the overwhelmingly common case. Returns
/// the value directly — no [`Step`] is built and no [`force`] unwrap runs. A
/// `Call` reached here was compiled `tail = false`, so [`exec_call`]'s step is
/// always `Done` (and a stray `Tail` is still resolved safely by [`force`]).
fn exec_value(heap: &mut Heap, node: &Node, frame_base: usize, genv: EnvRoot) -> LispResult {
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
            let closure = crate::eval::make_closure(heap, None, fn_rest.load(), env)?;
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

/// The combination executor for the surviving `Node` path ([`exec_value`] — used by
/// `push_frame`'s `&optional` defaults and top-level `run`). Resolves the callee
/// through the call-site IC, evaluates the arguments onto the operand stack, and
/// dispatches; the returned [`Step`] is forced in value position. (The bytecode
/// engine uses its own `Inst::Call` path in [`exec_chunk`].)
#[allow(clippy::too_many_arguments)]
fn exec_call(
    heap: &mut Heap,
    callee: &Node,
    args: &[Node],
    tail: bool,
    pos: Option<Pos>,
    file: Option<&str>,
    site: u32,
    frame_base: usize,
    genv: EnvRoot,
) -> Result<Step, LispError> {
    // Tag an error with this combination's source position (and file when known)
    // if it doesn't already carry one — so the *innermost* failing call wins
    // (mirrors the tree-walker's `or_form_pos`); a sub-call that already tagged
    // itself is left untouched.
    let tag = |e: LispError| match pos {
        Some(p) => match file {
            Some(f) => e.or_pos(p).or_file(f),
            None => e.or_pos(p),
        },
        None => e,
    };
    // Resolve the callee — through this site's inline cache when it has one
    // (ADR-096). A hit skips the `env_get` walk entirely and may carry the
    // VM fast path (the callee's compiled arm + captured env); a miss
    // resolves normally and installs the entry, stamped with `probe_epoch`
    // (read *before* the resolution, so an arg-eval `def` below can't be
    // attributed to this resolution). Engages only when the body's free
    // names resolve through the process global (`is_global`): a
    // local-capturing closure's captured frames could shadow the symbol,
    // and they differ per closure *instance* while the site is shared.
    let probe_epoch = heap.global_epoch();
    let mut fast: Option<(Arc<CompiledArm>, EnvId)> = None;
    let cv: Value;
    'resolve: {
        if site != NO_SITE {
            if let Node::Global(sym) = callee {
                if heap.is_global(heap.read_root_env(genv)) {
                    let argc = args.len() as u32;
                    if let Some((v, payload)) = heap.vm_call_ic_probe(site, *sym, argc, probe_epoch)
                    {
                        crate::perf_bump!(call_ic_hit);
                        cv = v;
                        fast = payload;
                        break 'resolve;
                    }
                    crate::perf_bump!(call_ic_miss);
                    // Miss: resolve (exactly what `exec_value` on the callee
                    // would do), then install. A *dynamic* symbol is never
                    // cached — a `binding` re-binds it without bumping the
                    // epoch, so a cached resolution would bypass it. (A
                    // later `defdyn` of a cached symbol bumps the epoch, so
                    // the entry self-invalidates and the re-install refuses.)
                    let env = heap.read_root_env(genv);
                    let v = match heap.env_get(env, *sym) {
                        Some(v) => v,
                        None => return Err(tag(crate::eval::unbound_error(heap, *sym))),
                    };
                    if !value::is_dynamic(*sym) {
                        let arm = match v.unpack() {
                            // Cache the VM fast path only for a callee
                            // `dispatch` would run on the VM directly: a
                            // non-passthrough closure with a compiled arm
                            // for this argc. Everything else caches just
                            // the value (still skips the lookup walk).
                            ValueRef::Fn(id)
                                if crate::eval::passthrough_arm(heap, id, args.len()).is_none() =>
                            {
                                compiled_arm_for(heap, id, args.len()).map(|arm| {
                                    let cenv =
                                        heap.closure(id).env.unwrap_or_else(|| heap.global());
                                    (arm, cenv)
                                })
                            }
                            _ => None,
                        };
                        fast = arm.clone();
                        // Mirror vm_call_ic_put's LOCAL-env guard: arg eval below can
                        // trigger a LOCAL minor collect that moves the env without
                        // bumping the global epoch, so the epoch check at the fast-path
                        // use site wouldn't catch a stale LOCAL cenv.  Clear fast now.
                        if let Some((_, cenv)) = &fast {
                            if *cenv != EnvId::GLOBAL && cenv.region() == value::LOCAL {
                                fast = None;
                            }
                        }
                        heap.vm_call_ic_put(
                            site,
                            crate::core::heap::CallIcEntry {
                                sym: *sym,
                                argc,
                                epoch: probe_epoch,
                                callee: v,
                                arm,
                                fast: std::cell::Cell::new(None),
                            },
                        );
                    }
                    cv = v;
                    break 'resolve;
                }
            }
        }
        // No IC for this site/shape: evaluate the callee node as before.
        cv = exec_value(heap, callee, frame_base, genv).map_err(tag)?;
    }
    // Evaluate each argument, keeping the callee + results on the operand
    // stack so a collection during a later argument's eval relocates them in
    // place (mirrors `eval::eval_arguments`). `save` is this call's region;
    // it is always truncated back, including on the error path.
    let save = heap.roots_len();
    heap.push_root(cv);
    for a in args.iter() {
        match exec_value(heap, a, frame_base, genv) {
            Ok(v) => heap.push_root(v),
            Err(e) => {
                heap.truncate_roots(save);
                return Err(tag(e));
            }
        }
    }
    // Re-read post-collection from the (relocated) operand stack.
    let callee_v = heap.root_at(save);
    let mut argv: SmallVec<[Value; 4]> = SmallVec::with_capacity(args.len());
    for k in 0..args.len() {
        argv.push(heap.root_at(save + 1 + k));
    }
    // The IC fast path: run the cached compiled arm directly, skipping
    // `dispatch`'s passthrough probe + body-cache lookup + env read —
    // but only if the global epoch is *still* `probe_epoch`. An arg's
    // eval can `def` (new resolution next call — but THIS call correctly
    // uses the pre-args callee, which is `callee_v`, rooted) or fire a
    // RUNTIME compaction (which rewrites the rooted `callee_v` in place
    // but NOT the un-registered `fast` arm's node tree or its env
    // handle) — either bumps the epoch, so the stale fast path is
    // dropped and the rooted callee takes the generic path below.
    if let Some((arm, cenv)) = fast {
        if heap.global_epoch() == probe_epoch {
            let result = if tail {
                Ok(Step::Tail {
                    compiled: arm,
                    args: argv,
                    genv: cenv,
                })
            } else {
                vm_apply(heap, arm, &argv, cenv).map(Step::Done)
            };
            heap.truncate_roots(save);
            return result.map_err(tag);
        }
    }
    // The *current* env (read fresh post-collection) is what a native callee
    // runs in; a VM-eligible closure callee instead runs in its own captured
    // env, which `dispatch` reads off the closure.
    let cur_env = heap.read_root_env(genv);
    let result = dispatch(heap, callee_v, argv, tail, cur_env);
    heap.truncate_roots(save);
    result.map_err(tag)
}

/// Restores the `capture_top_level` flag on drop — so the gate is reset even if the
/// guarded tree-walker `apply` *panics* (caught by `run_one`'s `catch_unwind`). The
/// manual save/restore it replaces leaked a `false` flag on a panic until the next
/// `vm_run_bc` entry re-stamped it.
struct CaptureTopGuard(bool);
impl Drop for CaptureTopGuard {
    fn drop(&mut self) {
        crate::process::set_capture_top_level(self.0);
    }
}

/// Dispatch a call whose callee and `argv` are already evaluated. A VM-eligible
/// closure of matching arity runs on the VM (or, in tail position, returns a
/// `Tail` for the trampoline); everything else (natives, multi-arm / ineligible
/// closures, arity mismatches) defers to the tree-walker via `eval::apply`.
fn dispatch(
    heap: &mut Heap,
    callee: Value,
    argv: SmallVec<[Value; 4]>,
    tail: bool,
    genv: EnvId,
) -> Result<Step, LispError> {
    let mut cur_callee = callee;
    let mut cur_argv = argv;
    // Outer `'apply` loop: mirrors the TW's `'dispatch` loop (eval/mod.rs). Each
    // iteration runs the passthrough-redirect inner loop, then checks for `apply`
    // unfolding. On unfold, `cur_callee`/`cur_argv` are rewritten and the outer
    // loop continues so passthrough can redirect the unfolded callee (e.g.
    // `(apply + '(1 2))` unfolds to `(+ 1 2)`, then passthrough elides `+`).
    // On no-unfold, `break` falls through to the VM/TW dispatch below.
    //
    // O(1) stack: no new Rust frame per `apply` iteration — the unfolding and
    // re-dispatch all happen inside this single `dispatch` call, then `vm_apply`
    // (or a `Step::Tail` trampoline) handles the real callee.
    'apply: loop {
        // Thin-wrapper passthrough redirect (ADR-069), mirroring `eval`'s `'dispatch`
        // loop: a pure pass-through prelude op (`(< n 2)` → `<` whose 2-arg arm is
        // `(%lt n 2)`, etc.) redirects straight to its inner `%native` on remapped
        // args — so the hot loop reaches `call_native` directly instead of re-entering
        // `apply_closure` (a frame alloc + param binds + a body eval) for every
        // arithmetic/comparison op. Late-binding safe: it reads the *live* closure and
        // re-resolves the inner head each call (a symbol lookup — no GC, so `cur_argv`
        // stays valid). Looped for chained passthroughs.
        loop {
            let id = match cur_callee.unpack() {
                ValueRef::Fn(id) => id,
                _ => break,
            };
            let Some((head, map)) = crate::eval::passthrough_arm(heap, id, cur_argv.len()) else {
                break;
            };
            let cl_env = heap.closure(id).env.unwrap_or_else(|| heap.global());
            // VM inner-head resolution: a direct `env_get` for a symbol head (no GC, so
            // `cur_argv` stays valid), else the head value itself. The shared
            // `passthrough_redirect_ok` then gates the redirect (callable inner only),
            // counts the reduction, and honours the deadline.
            let inner = match head.unpack() {
                ValueRef::Sym(s) => heap.env_get(cl_env, s),
                _ => Some(head),
            };
            let Some(inner) = inner else { break };
            // A redirect back to the *same* closure is direct self-recursion
            // (`(defn hog () (hog))`), not a thin wrapper: looping it here would spin
            // un-preemptibly (this redirect path has no captureable safepoint). Break
            // so it dispatches as a normal call → its VM arm, whose loop-top reduction
            // check preempts it (ADR-100 §8.1).
            if matches!(inner.unpack(), ValueRef::Fn(iid) if iid.0 == id.0) {
                break;
            }
            if !crate::eval::passthrough_redirect_ok(inner)? {
                break;
            }
            let mut next: SmallVec<[Value; 4]> = SmallVec::with_capacity(map.len());
            for &i in &map {
                next.push(cur_argv[i]);
            }
            cur_callee = inner;
            cur_argv = next;
        }
        // `apply` unfolding: `(apply real arg... list)` → `(real arg... ...list)`.
        // Mirrors the TW's inline unfolding (eval/mod.rs `while let Native … "apply"`).
        // After unfolding, `continue 'apply` re-runs passthrough on the real callee.
        // If the callee is not `apply`, or arity < 2, break and dispatch normally.
        if let ValueRef::Native(id) = cur_callee.unpack() {
            if heap.native(id).name == "apply" && cur_argv.len() >= 2 {
                let list = cur_argv
                    .pop()
                    .expect("cur_argv non-empty (len >= 2, checked)");
                let mut real = cur_argv.remove(0);
                // A lazy seq-view as the spliced arg list must realise first —
                // `seq_items` can't run its transducer. The realise re-enters `eval`
                // (a GC safepoint that relocates LOCAL handles), so the callee `real`
                // and the remaining leading args must be rooted across it and re-read
                // after — never trusted as pre-safepoint copies (ADR-114 re-read
                // discipline; mirrors `realize_seqviews`/`prim_eq`). Without this,
                // `(apply <local-closure> … <seq-view>)` derefs a stale closure/arg
                // handle → use-after-GC.
                let list = if matches!(list.unpack(), ValueRef::SeqView(_)) {
                    heap.root_scope(|heap| -> Result<Value, LispError> {
                        let real_r = heap.root(real);
                        let arg_roots: SmallVec<[_; 4]> =
                            cur_argv.iter().map(|&v| heap.root(v)).collect();
                        let realized = crate::builtins::realize_seqview(heap, genv, list)?;
                        real = heap.read_root(real_r);
                        for (slot, &r) in cur_argv.iter_mut().zip(arg_roots.iter()) {
                            *slot = heap.read_root(r);
                        }
                        Ok(realized)
                    })?
                } else {
                    list
                };
                cur_argv.extend(heap.seq_items(list)?);
                cur_callee = real;
                continue 'apply;
            }
        }
        break;
    }
    // A VM-eligible closure of matching arity runs on the VM (or yields a tail
    // call for the trampoline); a native or non-passthrough/ineligible callee goes
    // to the tree-walker via `eval::apply` (which is just `call_native` for a
    // native — cheap).
    if let ValueRef::Fn(id) = cur_callee.unpack() {
        // Resolve the arm cloning only the `Arc<CompiledArm>` (not the enclosing
        // `CompiledClosure`) — one fewer Arc clone per call on the hot path.
        if let Some(arm) = compiled_arm_for(heap, id, cur_argv.len()) {
            // Run the callee in *its own* captured env (Stage 2c): a
            // global-capturing closure (`env == None`) resolves to the process
            // global as before, while a local-capturing one resolves its free
            // vars in the env it closed over. `genv` (the caller's env) is only
            // for natives below.
            let callee_env = heap.closure(id).env.unwrap_or_else(|| heap.global());
            if tail {
                return Ok(Step::Tail {
                    compiled: arm,
                    args: cur_argv,
                    genv: callee_env,
                });
            }
            // JIT fast path: call jit_tier directly, bypassing vm_run_bc's per-call
            // overhead (GcBlockGuard, TopLevelGuard thread-locals, BcFrame Vec alloc,
            // loop-top safepoint checks). Gated on: JIT code installed, no runtime GC
            // handles (which need live_arm_push registration), no installed inline
            // upgrade (which needs the larger inline_nslots frame vm_run_bc sets up).
            // The pre-check on `jit_code` avoids push_frame + vm_apply double-setup
            // for arms that haven't tiered yet (those fall straight through to vm_apply).
            #[cfg(feature = "jit")]
            {
                use std::sync::atomic::Ordering::Acquire;
                let code = arm.jit_code.load(Acquire);
                if !arm.has_runtime_handles
                    && !arm.inline_installed.load(Acquire)
                    && !code.is_null()
                    && code != crate::jit::BAILED
                    && code != crate::jit::QUEUED
                {
                    let env_base = heap.env_roots_len();
                    let env_root = heap.root_env(callee_env);
                    let base = heap.roots_len();
                    if let Err(e) = push_frame(heap, &arm, &cur_argv, env_root) {
                        heap.truncate_env_roots(env_base);
                        return Err(e);
                    }
                    match jit_tier(&arm, heap, base, env_root) {
                        Some(0) => {
                            crate::perf_bump!(jit_apply_fast);
                            let v = heap.root_at(base);
                            heap.truncate_roots(base);
                            heap.truncate_env_roots(env_base);
                            return Ok(Step::Done(v));
                        }
                        Some(3) => {
                            heap.truncate_roots(base);
                            heap.truncate_env_roots(env_base);
                            let e = heap
                                .jit_pending_error
                                .take()
                                .expect("JIT outcome 3 with no parked error");
                            return Err(e);
                        }
                        Some(4) => {
                            crate::perf_bump!(jit_fast_tail4);
                            // Tail call staged by the JIT: [callee, arg0..argN] sit
                            // above the frame at roots[base+active_nslots..].  These
                            // were pushed *after* any GC that fired inside
                            // jit_dispatch_call's safepoint (line ~8890), so they hold
                            // current-epoch handles — unlike cur_argv which was captured
                            // before jit_tier ran and may be stale.  Follow the staged
                            // call directly instead of re-running this arm on the VM.
                            let frame_top = base + arm.active_nslots();
                            let n = heap.roots_len();
                            let callee_env2 = heap.read_root_env(env_root);
                            if n > frame_top {
                                let staged_callee = heap.root_at(frame_top);
                                let staged_argc = n - frame_top - 1;
                                let staged_argv: SmallVec<[Value; 4]> = (0..staged_argc)
                                    .map(|k| heap.root_at(frame_top + 1 + k))
                                    .collect();
                                heap.truncate_roots(base);
                                heap.truncate_env_roots(env_base);
                                return Ok(Step::Done(apply_value(
                                    heap,
                                    staged_callee,
                                    &staged_argv,
                                    callee_env2,
                                )?));
                            }
                            // Staged area is empty (shouldn't happen for outcome 4,
                            // but fall back gracefully).  GC may have run, so read
                            // fresh args from the frame rather than stale cur_argv.
                            let argc = cur_argv.len();
                            let fresh_argv: SmallVec<[Value; 4]> = if arm.rest_slot.is_none() {
                                (0..argc).map(|k| heap.root_at(base + k)).collect()
                            } else {
                                // Rest arm: the rest elements were folded into a list at
                                // bind time, so they can't be reconstructed per-slot from
                                // `roots`; fall back to the pre-call `cur_argv`. Sound ONLY
                                // if no GC relocated those handles since capture — i.e. a
                                // rest arm never reaches here after a real safepoint (it's
                                // outside the JIT int-subset). A stale handle here is the
                                // bug #2 class (ADR-114) and would corrupt, so enforce the
                                // invariant in debug rather than leaving it as a silent
                                // assumption.
                                #[cfg(debug_assertions)]
                                for v in &cur_argv {
                                    debug_assert!(
                                        heap.dbg_value_stale(*v).is_none(),
                                        "dispatch: stale LOCAL handle in the rest-arm \
                                             cur_argv fallback after a JIT safepoint — the \
                                             'rest arms never JIT post-safepoint' invariant \
                                             broke (ADR-114; re-read from roots instead)"
                                    );
                                }
                                cur_argv
                            };
                            heap.truncate_roots(base);
                            heap.truncate_env_roots(env_base);
                            return Ok(Step::Done(vm_apply(heap, arm, &fresh_argv, callee_env2)?));
                        }
                        _ => {
                            crate::perf_bump!(jit_fast_deopt);
                            // Epoch reset (→ None), deopt (1), or preempt (2): re-run
                            // on the VM.  GC can fire during any jit_dispatch_call
                            // safepoint (line ~8903) triggered by a sub-call inside
                            // jit_tier — even for deopt (the sub-call returns a
                            // non-int, GC fires in its safepoint, then the JIT deopts
                            // on the non-int result).  After that GC, cur_argv holds
                            // stale LOCAL handles.  The frame at roots[base..base+nslots]
                            // is updated in place by every GC, so read fresh args from
                            // there.  Arms with a rest slot collect the rest elements
                            // into a list; they're unreachable in the JIT int-subset
                            // and fall through to cur_argv as an inert dead-code path.
                            let callee_env2 = heap.read_root_env(env_root);
                            let argc = cur_argv.len();
                            let fresh_argv: SmallVec<[Value; 4]> = if arm.rest_slot.is_none() {
                                (0..argc).map(|k| heap.root_at(base + k)).collect()
                            } else {
                                // Rest arm: the rest elements were folded into a list at
                                // bind time, so they can't be reconstructed per-slot from
                                // `roots`; fall back to the pre-call `cur_argv`. Sound ONLY
                                // if no GC relocated those handles since capture — i.e. a
                                // rest arm never reaches here after a real safepoint (it's
                                // outside the JIT int-subset). A stale handle here is the
                                // bug #2 class (ADR-114) and would corrupt, so enforce the
                                // invariant in debug rather than leaving it as a silent
                                // assumption.
                                #[cfg(debug_assertions)]
                                for v in &cur_argv {
                                    debug_assert!(
                                        heap.dbg_value_stale(*v).is_none(),
                                        "dispatch: stale LOCAL handle in the rest-arm \
                                             cur_argv fallback after a JIT safepoint — the \
                                             'rest arms never JIT post-safepoint' invariant \
                                             broke (ADR-114; re-read from roots instead)"
                                    );
                                }
                                cur_argv
                            };
                            heap.truncate_roots(base);
                            heap.truncate_env_roots(env_base);
                            return Ok(Step::Done(vm_apply(heap, arm, &fresh_argv, callee_env2)?));
                        }
                    }
                }
            }
            return Ok(Step::Done(vm_apply(heap, arm, &cur_argv, callee_env)?));
        }
        // A closure with no VM-eligible arm for this argc — a true defer to the
        // tree-walker. Native frames created by the tree-walker can't be captured
        // by the state-capture machinery; gate off so any `receive` inside blocks
        // the worker (§7.4 dirty-scheduler carve-out) instead of attempting a
        // state-capture that can't cross the native boundary.
        crate::perf_bump!(tw_defer);
        let _guard = CaptureTopGuard(crate::process::set_capture_top_level(false));
        let result = crate::eval::apply(heap, cur_callee, &cur_argv, genv);
        return Ok(Step::Done(result?));
    }
    Ok(Step::Done(crate::eval::apply(
        heap, cur_callee, &cur_argv, genv,
    )?))
}

/// Push a fresh activation frame for `arm` onto `Heap::roots`: required args, then
/// `&optional` slots (the provided arg, or nil if missing), then the `&` rest list
/// (the trailing args conased into a fresh list), then nil for the `let`/`letrec`
/// binders — `nslots` values total. Selection guarantees `args.len() >= nrequired`.
/// No eval runs here (the rest is a plain `list_from_slice`), so no collection can
/// happen between reading `args` and rooting the slots.
/// DEBUG ONLY: assert every value in `args` has a valid `Value` discriminant. A value
/// with an out-of-range tag byte is non-`Value` memory — a JIT frame-slot corruption
/// (the bug #2 family). Aborting here, at the earliest frame entry that sees it, makes
/// the backtrace point at the call chain that produced the garbage.
#[cfg(debug_assertions)]
fn dbg_check_args(args: &[Value], label: &str) {
    for (i, a) in args.iter().enumerate() {
        let tag = (unsafe { std::mem::transmute::<Value, [i64; 3]>(*a) }[0] as u64 & 0xff) as u8;
        // Value has ~22 variants (max discriminant ~21); well above that is garbage.
        if tag > 24 {
            panic!("[arg-origin] {label}: arg[{i}] has invalid Value tag {tag:#x} — corrupt (non-Value) memory passed into a frame");
        }
    }
}

fn push_frame(
    heap: &mut Heap,
    arm: &CompiledArm,
    args: &[Value],
    genv: EnvRoot,
) -> Result<(), LispError> {
    // DEBUG ONLY: catch a corrupt argument at the EARLIEST frame entry — the first call
    // that receives an invalid-tag `Value` is closest to the origin of the JIT GC bug.
    // Abort so the backtrace shows the call chain that produced it.
    #[cfg(debug_assertions)]
    dbg_check_args(args, "push_frame");

    let base = heap.roots_len();
    // Pre-allocate the whole frame as nil: every slot (params, optionals, rest, and
    // the body's `let`/`letrec` binders) must exist before anything writes it via
    // `set_root_at` — including a real `&optional` default whose body may bind its
    // own `let` slots. One `resize` rather than a per-slot push loop (call hot path).
    heap.extend_roots_to_nil(base + arm.nslots);
    // Consume ALL provided args into their (now-rooted) slots FIRST, before any
    // default is evaluated: a default's eval can collect, which would strand the
    // still-live `args` slice (LOCAL handles) if it were read afterwards.
    for i in 0..arm.nrequired {
        heap.set_root_at(base + i, args.get(i).copied().unwrap_or(Value::nil()));
    }
    // Provided optionals are a left-to-right prefix of `args`; the remainder are
    // missing and take their defaults below.
    let provided_opt = args.len().saturating_sub(arm.nrequired).min(arm.noptional);
    for j in 0..provided_opt {
        heap.set_root_at(base + arm.nrequired + j, args[arm.nrequired + j]);
    }
    if let Some(rslot) = arm.rest_slot {
        let start = (arm.nrequired + arm.noptional).min(args.len());
        let rest = heap.list_from_slice(&args[start..]);
        heap.set_root_at(base + rslot, rest);
    }
    // #3 lexical addressing: fill the capture slots from the closure's captured env, so
    // the body reads captured lexicals as fast `Node::Local` slots rather than `env_get`
    // symbol-scans. Each `capture_names[k]` occupies slot `capture_base + k`. `capture_value`
    // takes an index fast-path when the captured env is a flat frame (`vars[k]` is that name
    // — the VM-built common case) and falls back to a by-name `env_get` for a chained /
    // tree-walker env, so it's correct in both engines. Filled before optional defaults so a
    // default form may reference a capture. No GC between here and the body (no alloc).
    if !arm.capture_names.is_empty() {
        let cenv = heap.read_root_env(genv);
        let capture_base = arm.nrequired + arm.noptional + arm.rest_slot.is_some() as usize;
        for (k, &name) in arm.capture_names.iter().enumerate() {
            let v = heap.capture_value(cenv, k, name);
            heap.set_root_at(base + capture_base + k, v);
        }
    }
    // Missing optionals take their default, left-to-right (so a later default sees an
    // earlier one). `None` is a nil-default — the slot is already nil. A real default
    // evaluates against the frame: earlier params/optionals are filled and rooted;
    // its own slot and later slots are still nil (the compiler bound it after the
    // default, so the default can't name itself).
    for j in provided_opt..arm.noptional {
        if let Some(node) = &arm.optional_defaults[j] {
            let v = exec_value(heap, node, base, genv)?;
            heap.set_root_at(base + arm.nrequired + j, v);
        }
    }
    Ok(())
}

/// Run a chunked closure arm and its whole chain of chunked calls on the explicit
/// bytecode frame stack ([`vm_run_bc`]) — the sole VM executor since ADR-100 Stage 5
/// (the `Node`-walking trampoline was retired with the bytecode default). Every
/// `CompiledArm` from `compile_arm` has a chunk, so this always routes to the driver;
/// `vm_run_bc` does the per-frame live-arm registration + the runaway frame guard.
/// Callers: `dispatch` (non-tail VM-closure branch), `exec_call`'s IC fast path, and
/// `force` (a tail `Step`). The tree-walker (`BROOD_VM=0`) is the remaining fallback.
fn vm_apply(
    heap: &mut Heap,
    compiled0: Arc<CompiledArm>,
    args: &[Value],
    genv0: EnvId,
) -> LispResult {
    // `top_level = false`: this is a nested run (the process-body driver is
    // `run_process_body`), so it does no loop-top preempt/kill capture — only the
    // body driver does. A `receive` suspend that surfaces here re-raises (§8.1).
    match vm_run_bc(heap, compiled0, args, genv0, None, false)? {
        VmOutcome::Done(v) => Ok(v),
        // A `receive` suspended inside this VM run — but this run is **nested** under a
        // native (a `map`/`try`/`binding`/`%isolate` callback that re-entered the VM via
        // `dispatch`/`apply_value`), so its continuation can't be returned as a value.
        // This is the native-nested case (ADR-100 §8.1): discard the captured inner
        // frames (their roots were left intact for a top-level resume — unwind them to
        // the entry mark) and re-raise the `Control::Suspend` so the enclosing native
        // re-raises it untouched. The *outer* `vm_run_bc` then re-runs this native's
        // `Inst::Call` on resume — correct because the only shape that occurs has no
        // irreversible side effect before the `receive`. (A native-nested receive that
        // *would* repeat a side effect is gated off this path by `capture_top_level()`
        // and blocks its worker instead — the §7.4 dirty carve-out.)
        VmOutcome::Suspended(s) => {
            let deadline = s.deadline;
            heap.truncate_roots(s.entry_roots);
            heap.truncate_env_roots(s.entry_env);
            heap.live_arm_truncate(s.entry_arms);
            Err(LispError::suspend(deadline))
        }
        // `top_level = false` ⇒ no loop-top capture, so a nested run never preempts or
        // self-kills; these are produced only by the body driver (`run_process_body`).
        VmOutcome::Preempted(_) | VmOutcome::Killed => {
            unreachable!("a nested vm_apply run does no loop-top preempt/kill capture")
        }
    }
}

/// Run a green process's body thunk on the bytecode driver as the **top-level**
/// state-capture run (ADR-100 §8.3) — the entry `run_one` uses. A `None` `resume`
/// starts the body fresh (resolving `body`'s 0-arg compiled arm); a `Some` replays a
/// parked continuation. Unlike [`apply_value`]/[`vm_apply`], a
/// `Suspended`/`Preempted`/`Killed` outcome is **returned** (the scheduler parks /
/// re-enqueues / retires it) rather than re-raised — this is the body driver, so its
/// continuation is the process's continuation. A body with a compiled 0-arg arm runs on
/// the capture driver; one without (vanishingly rare) tree-walks on the worker thread
/// and its `receive`s block (the §7.4 dirty carve-out).
pub(crate) fn run_process_body(
    heap: &mut Heap,
    body: Value,
    resume: Option<Suspended>,
) -> Result<VmOutcome, LispError> {
    match resume {
        // Resume: the continuation's own `cur.arm` drives; `arm0`/`genv0` are ignored
        // by the resume branch, so pass a (cheap) clone of it as the placeholder.
        Some(s) => {
            let arm = s.cur.arm.clone();
            vm_run_bc(heap, arm, &[], EnvId::GLOBAL, Some(s), true)
        }
        // Fresh: run the 0-arg body. A VM-eligible body (the overwhelming case — every
        // body in the whole suite is) runs on the capture driver, so its `receive`s
        // capture + migrate. A body that defers to the tree-walker (no compiled 0-arg
        // arm — vanishingly rare; zero across the suite) has no reified frame stack to
        // capture, so it runs tree-walked **on this worker thread** and its `receive`s
        // **block** the worker (the dirty-scheduler carve-out, §7.4); it returns Done/
        // Err and never suspends. Either way: no coroutine.
        None => match body.unpack() {
            ValueRef::Fn(id) if compiled_arm_for(heap, id, 0).is_some() => {
                let arm = compiled_arm_for(heap, id, 0).expect("just checked is_some");
                let cenv = heap.closure(id).env.unwrap_or_else(|| heap.global());
                vm_run_bc(heap, arm, &[], cenv, None, true)
            }
            ValueRef::Fn(_) => {
                crate::eval::apply(heap, body, &[], EnvId::GLOBAL).map(VmOutcome::Done)
            }
            _ => Err(LispError::type_err("process body must be a function")),
        },
    }
}

/// Lower a compiled `Node` body to a [`Chunk`], or `None` if it uses any node
/// outside Stage 1's vocabulary (`Call`/`SelfCall`/`MakeClosure`, or a `Const` with
/// a movable RUNTIME handle). `None` is always safe — the arm runs on `exec_node`.
fn compile_chunk(body: &Node) -> Option<Chunk> {
    let mut code = Vec::new();
    emit_node(body, &mut code)?;
    Some(Chunk { code })
}

/// Recursively emit `node` into `code`, leaving its value on the operand stack.
/// Returns `None` (aborting the whole chunk) on any unsupported node.
fn emit_node(node: &Node, code: &mut Vec<Inst>) -> Option<()> {
    match node {
        // A fresh `ConstVal` cloned from the node's (atoms inline; a movable RUNTIME
        // handle is re-encoded). Chunk handles are rewritten in place under a RUNTIME
        // compaction by `rewrite_chunk` (registered via the arm's `has_runtime_handles`).
        Node::Const(cv) => code.push(Inst::Const(ConstVal::new(cv.load()))),
        Node::Local(i) => code.push(Inst::Local(*i)),
        Node::Global(s) => code.push(Inst::Global(*s)),
        Node::GlobalIc { sym, site } => code.push(Inst::GlobalIc {
            sym: *sym,
            site: *site,
        }),
        Node::If(cond, then, els) => {
            emit_node(cond, code)?;
            let j_else = code.len();
            code.push(Inst::JumpIfFalse(0)); // backpatched
            emit_node(then, code)?;
            let j_end = code.len();
            code.push(Inst::Jump(0)); // backpatched
            let else_ip = code.len();
            emit_node(els, code)?;
            let end_ip = code.len();
            code[j_else] = Inst::JumpIfFalse(else_ip);
            code[j_end] = Inst::Jump(end_ip);
        }
        Node::Do(nodes) => {
            if nodes.is_empty() {
                code.push(Inst::Const(ConstVal::Atom(Value::nil())));
            } else {
                let last = nodes.len() - 1;
                for n in &nodes[..last] {
                    emit_node(n, code)?;
                    code.push(Inst::Pop); // evaluated for effect
                }
                emit_node(&nodes[last], code)?;
            }
        }
        Node::LetBind { binds, body } => {
            for (slot, rhs) in binds.iter() {
                emit_node(rhs, code)?;
                code.push(Inst::SetLocal(*slot));
            }
            emit_node(body, code)?;
        }
        Node::Vector(elems) => {
            for e in elems.iter() {
                emit_node(e, code)?;
            }
            code.push(Inst::MakeVector(elems.len()));
        }
        Node::Map(entries) => {
            for (k, v) in entries.iter() {
                emit_node(k, code)?;
                emit_node(v, code)?;
            }
            code.push(Inst::MakeMap(entries.len()));
        }
        Node::Prim1 {
            op,
            a,
            head,
            guard,
            pos,
        } => {
            emit_node(a, code)?;
            code.push(Inst::Prim1 {
                op: *op,
                head: *head,
                guard: AtomicU64::new(guard.load(Ordering::Relaxed)),
                pos: *pos,
            });
        }
        Node::Prim2 {
            op,
            a,
            b,
            map,
            head,
            guard,
            pos,
            broot: _,
        } => {
            // Snapshot the guard epoch; each push site creates its own AtomicU64
            // (AtomicU64 is not Copy so we can't reuse a single binding).
            let gv = guard.load(Ordering::Relaxed);
            // Fuse when operands are frame locals or integer literals: avoids
            // the 2 intermediate root-stack pushes the generic path needs.
            // Only integer constants are fused (keeps Prim2SlotInt below
            // MakeClosure's size, so the Inst enum doesn't grow).
            let fused = match (&**a, &**b) {
                (Node::Local(sa), Node::Local(sb)) => {
                    code.push(Inst::Prim2SlotSlot {
                        op: *op,
                        map: *map,
                        slot_a: *sa,
                        slot_b: *sb,
                        head: *head,
                        guard: AtomicU64::new(gv),
                        pos: *pos,
                    });
                    true
                }
                (Node::Local(sa), Node::Const(cv)) => {
                    if let ValueRef::Int(n) = cv.load().unpack() {
                        code.push(Inst::Prim2SlotInt {
                            op: *op,
                            map: *map,
                            slot_a: *sa,
                            int_b: n,
                            swapped: false,
                            head: *head,
                            guard: AtomicU64::new(gv),
                            pos: *pos,
                        });
                        true
                    } else {
                        false
                    }
                }
                (Node::Const(cv), Node::Local(sb)) => {
                    if let ValueRef::Int(n) = cv.load().unpack() {
                        // Slot goes to src[0], const to src[1] — invert the map. `swapped`
                        // so the dispatch fallback restores the original `(op Const Local)`
                        // order when it calls the user `head` (the inline path uses `map`).
                        let new_map = [1u8 - map[0], 1u8 - map[1]];
                        code.push(Inst::Prim2SlotInt {
                            op: *op,
                            map: new_map,
                            slot_a: *sb,
                            int_b: n,
                            swapped: true,
                            head: *head,
                            guard: AtomicU64::new(gv),
                            pos: *pos,
                        });
                        true
                    } else {
                        false
                    }
                }
                _ => false,
            };
            if !fused {
                emit_node(a, code)?;
                emit_node(b, code)?;
                code.push(Inst::Prim2 {
                    op: *op,
                    map: *map,
                    head: *head,
                    guard: AtomicU64::new(gv),
                    pos: *pos,
                });
            }
        }
        Node::Call {
            callee,
            args,
            tail,
            pos,
            file: _,
            site,
        } => {
            // Callee first, then each arg (the order `exec_call` evaluates them). When
            // the head is a free global, carry its symbol + `site` so the call-site IC
            // can cache the resolved arm (Stage 5); the callee is still pushed and
            // resolved in-order, so the IC is a pure cache.
            let head = if let Node::Global(s) = &**callee {
                Some(*s)
            } else {
                None
            };
            // A free-global head is NOT staged: `Inst::Call` resolves it through the call IC
            // (or `env_get` on a miss), so there's no redundant head-`Global` push + per-call
            // `env_get`. A computed callee (head `None`) is staged below the args as before.
            if head.is_none() {
                emit_node(callee, code)?;
            }
            for a in args.iter() {
                emit_node(a, code)?;
            }
            code.push(Inst::Call {
                argc: args.len(),
                tail: *tail,
                pos: *pos,
                site: *site,
                head,
            });
        }
        Node::SelfCall { args, pos: _ } => {
            for a in args.iter() {
                emit_node(a, code)?;
            }
            code.push(Inst::SelfCall { argc: args.len() });
        }
        Node::MakeClosure {
            fn_rest,
            captures,
            self_name,
        } => {
            // Capture sources are leaf reads (an enclosing lexical → `Local`, or a
            // global → `Global`), so emitting them is safepoint-free; their values
            // land on the operand stack in `captures` order and `MakeClosure` binds
            // them to the matching names. A fresh `ConstVal` re-encodes `fn_rest`
            // (rewritten in place by `rewrite_chunk` under a compaction).
            for (_, src) in captures.iter() {
                emit_node(src, code)?;
            }
            let names: Box<[Symbol]> = captures.iter().map(|(name, _)| *name).collect();
            code.push(Inst::MakeClosure {
                fn_rest: ConstVal::new(fn_rest.load()),
                names,
                self_name: *self_name,
            });
        }
        Node::TryCatch {
            body,
            bind_slot,
            handler,
        } => {
            code.push(Inst::TryCatch {
                body: NodePtr(NonNull::from(body.as_ref())),
                bind_slot: *bind_slot,
                handler: NodePtr(NonNull::from(handler.as_ref())),
            });
        }
    }
    Some(())
}

/// Tag an error with `pos` if it doesn't already carry one (innermost wins),
/// matching the `Node` interpreter's `or_pos` discipline.
#[inline]
fn tag_pos(e: LispError, pos: Option<Pos>) -> LispError {
    match pos {
        Some(p) => e.or_pos(p),
        None => e,
    }
}

/// Run a [`Chunk`] frame from `*ip`, returning a [`ChunkExit`] to the driver
/// ([`vm_run_bc`]). `*ip` is **resumed and updated in place**, so after a non-tail
/// `Call` returns `ChunkExit::Call`, the driver re-enters here at the instruction
/// after the call once the callee frame completes. The operand stack (`Heap::roots`
/// above `base + nslots`) carries intermediate values; frame slots live at `base..`;
/// `genv` is the captured-env root. On error, returns `Err` without unwinding the
/// operand stack — the driver unwinds every frame's roots back to entry.
///
/// Stage 4: a **non-tail** `Call` to a chunked VM arm returns `ChunkExit::Call` so
/// the driver **pushes a frame** instead of recursing natively; a non-tail call to a
/// native or tree-walked arm is run here (via `dispatch`) and its value pushed. A
/// **tail** `Call`/`SelfCall` returns `Tail`/`SelfTail` so the driver reuses the
/// frame (TCO). A single pass to the next call/return is bounded by the chunk length.
fn exec_chunk(
    heap: &mut Heap,
    arm: &CompiledArm,
    ip: &mut usize,
    base: usize,
    genv: EnvRoot,
    capture: bool,
    // Back-edge tiering counter (jit only): persisted across exec_chunk re-entries for
    // the same frame so non-tail Brood calls (which exit and re-enter exec_chunk) don't
    // reset the SelfCall iteration count. Each SelfCall increments this; every 256th
    // iteration triggers a JIT tier check in the outer loop. Owned by vm_run_bc and
    // stored in BcFrame so it survives frame save/restore.
    #[cfg(feature = "jit")] back_edges: &mut u32,
) -> Result<ChunkExit, LispError> {
    let chunk = arm.chunk.as_ref().expect("exec_chunk: arm has no chunk");
    while *ip < chunk.code.len() {
        let inst = &chunk.code[*ip];
        *ip += 1;
        // Instruction-level trace for debugging (BROOD_VM_TRACE=1).
        // Gate on a process-global OnceLock so we check once, not per-instruction.
        #[cfg(debug_assertions)]
        {
            static VM_TRACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
            if *VM_TRACE.get_or_init(|| {
                std::env::var("BROOD_VM_TRACE").is_ok_and(|v| v != "0" && !v.is_empty())
            }) {
                eprintln!("[vm-trace ip={}] {}", *ip - 1, inst.trace_name());
            }
        }
        match inst {
            Inst::Const(cv) => {
                let v = cv.load();
                heap.push_root(v);
            }
            Inst::Local(i) => {
                let v = heap.root_at(base + i);
                heap.push_root(v);
            }
            Inst::Global(s) => {
                let env = heap.read_root_env(genv);
                match heap.env_get(env, *s) {
                    Some(v) => heap.push_root(v),
                    None => return Err(crate::eval::unbound_error(heap, *s)),
                }
            }
            Inst::GlobalIc { sym, site } => {
                let env = heap.read_root_env(genv);
                let v = if heap.is_global(env) {
                    let epoch = heap.global_epoch();
                    if let Some(v) = heap.vm_global_ic_probe(*site, *sym, epoch) {
                        crate::perf_bump!(global_ic_hit);
                        v
                    } else {
                        crate::perf_bump!(global_ic_miss);
                        match heap.env_get(env, *sym) {
                            Some(v) => {
                                if !value::is_dynamic(*sym) {
                                    heap.vm_global_ic_put(*site, *sym, epoch, v);
                                }
                                v
                            }
                            None => return Err(crate::eval::unbound_error(heap, *sym)),
                        }
                    }
                } else {
                    match heap.env_get(env, *sym) {
                        Some(v) => v,
                        None => return Err(crate::eval::unbound_error(heap, *sym)),
                    }
                };
                heap.push_root(v);
            }
            Inst::Pop => {
                let n = heap.roots_len();
                heap.truncate_roots(n - 1);
            }
            Inst::SetLocal(slot) => {
                let n = heap.roots_len();
                let v = heap.root_at(n - 1);
                heap.truncate_roots(n - 1);
                heap.set_root_at(base + slot, v);
            }
            Inst::Jump(t) => *ip = *t,
            Inst::JumpIfFalse(t) => {
                let n = heap.roots_len();
                let c = heap.root_at(n - 1);
                heap.truncate_roots(n - 1);
                if !crate::eval::truthy(c) {
                    *ip = *t;
                }
            }
            Inst::MakeVector(nelem) => {
                // Same discipline as `exec_value`'s `Node::Vector`: read the elements
                // (already on the operand stack), truncate, then build.
                let n = heap.roots_len();
                let start = n - nelem;
                let mut vals = Vec::with_capacity(*nelem);
                for k in 0..*nelem {
                    vals.push(heap.root_at(start + k));
                }
                heap.truncate_roots(start);
                let v = heap.alloc_vector(vals);
                heap.push_root(v);
            }
            Inst::MakeMap(npairs) => {
                let n = heap.roots_len();
                let start = n - 2 * npairs;
                let mut pairs = Vec::with_capacity(*npairs);
                for i in 0..*npairs {
                    pairs.push((heap.root_at(start + 2 * i), heap.root_at(start + 2 * i + 1)));
                }
                heap.truncate_roots(start);
                let v = heap.map_from_pairs(pairs);
                heap.push_root(v);
            }
            Inst::Prim1 {
                op,
                head,
                guard,
                pos,
            } => {
                let pos = *pos;
                let n = heap.roots_len();
                let sa = heap.root_at(n - 1);
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
                            let v = heap.pair(p).0;
                            heap.truncate_roots(n - 1);
                            heap.push_root(v);
                            continue;
                        }
                        (PrimOp1::Rest, ValueRef::Pair(p)) => {
                            crate::perf_bump!(prim1_inline);
                            let v = heap.pair(p).1;
                            heap.truncate_roots(n - 1);
                            heap.push_root(v);
                            continue;
                        }
                        (PrimOp1::First | PrimOp1::Rest, ValueRef::Nil) => {
                            crate::perf_bump!(prim1_inline);
                            heap.truncate_roots(n - 1);
                            heap.push_root(Value::nil());
                            continue;
                        }
                        (PrimOp1::IsNil, v) => {
                            crate::perf_bump!(prim1_inline);
                            let result = Value::boolean(matches!(v, ValueRef::Nil));
                            heap.truncate_roots(n - 1);
                            heap.push_root(result);
                            continue;
                        }
                        (PrimOp1::IsPair, v) => {
                            crate::perf_bump!(prim1_inline);
                            let result = Value::boolean(matches!(
                                v,
                                ValueRef::Pair(_) | ValueRef::Range(_) | ValueRef::SeqView(_)
                            ));
                            heap.truncate_roots(n - 1);
                            heap.push_root(result);
                            continue;
                        }
                        (PrimOp1::IsEmpty, ValueRef::Nil) => {
                            crate::perf_bump!(prim1_inline);
                            heap.truncate_roots(n - 1);
                            heap.push_root(Value::boolean(true));
                            continue;
                        }
                        (PrimOp1::IsEmpty, ValueRef::Pair(_) | ValueRef::Range(_)) => {
                            crate::perf_bump!(prim1_inline);
                            heap.truncate_roots(n - 1);
                            heap.push_root(Value::boolean(false));
                            continue;
                        }
                        _ => {}
                    }
                }
                crate::perf_bump!(prim1_fallback);
                let cur_env = heap.read_root_env(genv);
                let callee = match heap.env_get(cur_env, *head) {
                    Some(c) => c,
                    None => return Err(tag_pos(crate::eval::unbound_error(heap, *head), pos)),
                };
                let sa = heap.root_at(n - 1);
                let argv: SmallVec<[Value; 4]> = SmallVec::from_slice(&[sa]);
                let result =
                    dispatch(heap, callee, argv, false, cur_env).and_then(|s| force(heap, s));
                heap.truncate_roots(n - 1);
                match result {
                    Ok(v) => heap.push_root(v),
                    Err(e) => return Err(tag_pos(e, pos)),
                }
            }
            Inst::Prim2 {
                op,
                map,
                head,
                guard,
                pos,
            } => {
                let n = heap.roots_len();
                let sa = heap.root_at(n - 2);
                let sb = heap.root_at(n - 1);
                let x = [sa, sb][map[0] as usize];
                let y = [sa, sb][map[1] as usize];
                match prim2_inline_exec(heap, *op, *map, false, *head, guard, x, y)? {
                    Some(v) => {
                        heap.truncate_roots(n - 2);
                        heap.push_root(v);
                    }
                    None => {
                        // Operands already rooted at n-2 and n-1.
                        let v = prim2_dispatch_rooted(heap, *head, n - 2, *pos, genv)?;
                        heap.push_root(v);
                    }
                }
            }
            Inst::Prim2SlotSlot {
                op,
                map,
                slot_a,
                slot_b,
                head,
                guard,
                pos,
            } => {
                let sa = heap.root_at(base + slot_a);
                let sb = heap.root_at(base + slot_b);
                let x = [sa, sb][map[0] as usize];
                let y = [sa, sb][map[1] as usize];
                let v = match prim2_inline_exec(heap, *op, *map, false, *head, guard, x, y)? {
                    Some(v) => v,
                    None => {
                        let save = heap.roots_len();
                        heap.push_root(sa);
                        heap.push_root(sb);
                        prim2_dispatch_rooted(heap, *head, save, *pos, genv)?
                    }
                };
                heap.push_root(v);
            }
            Inst::Prim2SlotInt {
                op,
                map,
                slot_a,
                int_b,
                swapped,
                head,
                guard,
                pos,
            } => {
                let sa = heap.root_at(base + slot_a);
                let sb = Value::int(*int_b);
                let x = [sa, sb][map[0] as usize];
                let y = [sa, sb][map[1] as usize];
                let v = match prim2_inline_exec(heap, *op, *map, *swapped, *head, guard, x, y)? {
                    Some(v) => v,
                    None => {
                        // Dispatch to the user `head` in the ORIGINAL call order. For the
                        // `(op Const Local)` fusion (`swapped`) that's `[const, local]` =
                        // `[sb, sa]`; otherwise `[sa, sb]`. (The inline path above used the
                        // map; this slow path must reconstruct the source order — a
                        // mismatch silently mis-ordered non-commutative ops, e.g.
                        // `(/ 24 x)` ran as `(/ x 24)`.)
                        let save = heap.roots_len();
                        let (first, second) = if *swapped { (sb, sa) } else { (sa, sb) };
                        heap.push_root(first);
                        heap.push_root(second);
                        prim2_dispatch_rooted(heap, *head, save, *pos, genv)?
                    }
                };
                heap.push_root(v);
            }
            Inst::Call {
                argc,
                tail,
                pos,
                site,
                head,
            } => {
                let pos = *pos;
                let argc = *argc;
                let n = heap.roots_len();
                let cur_env = heap.read_root_env(genv);
                // The top `argc` operands are always the args. A **free-global** head
                // (`head = Some`) is NOT staged — no preceding `Global` inst pushed it — so
                // the operands are just `[args]` (`drop_base = n - argc`) and the callee is
                // resolved here: the call-site IC gives `(callee, arm)` on a hit with no
                // `env_get`, else `env_get` resolves it and fills the IC. A **computed**
                // head (`head = None`) is staged below the args (`callee` at `n - argc - 1`,
                // `drop_base = n - argc - 1`) and takes no IC. This unifies callee resolution
                // into the call IC — the head no longer has its own `Global`/`env_get`.
                let mut argv: SmallVec<[Value; 4]> = SmallVec::with_capacity(argc);
                for k in 0..argc {
                    argv.push(heap.root_at(n - argc + k));
                }
                let mut fast: Option<(Arc<CompiledArm>, EnvId)> = None;
                let (callee, drop_base) = if let Some(sym) = head {
                    let drop_base = n - argc;
                    if *site != NO_SITE && heap.is_global(cur_env) {
                        let epoch = heap.global_epoch();
                        if let Some((v, payload)) =
                            heap.vm_call_ic_probe(*site, *sym, argc as u32, epoch)
                        {
                            crate::perf_bump!(call_ic_hit);
                            fast = payload;
                            (v, drop_base)
                        } else {
                            crate::perf_bump!(call_ic_miss);
                            let v = match heap.env_get(cur_env, *sym) {
                                Some(v) => v,
                                None => {
                                    return Err(tag_pos(
                                        crate::eval::unbound_error(heap, *sym),
                                        pos,
                                    ))
                                }
                            };
                            // Cache the resolved callee + (for a non-passthrough VM closure)
                            // its arm. A dynamic var is never cached (it can shadow per call).
                            let arm = match v.unpack() {
                                ValueRef::Fn(id)
                                    if crate::eval::passthrough_arm(heap, id, argc).is_none() =>
                                {
                                    compiled_arm_for(heap, id, argc).map(|arm| {
                                        let cenv =
                                            heap.closure(id).env.unwrap_or_else(|| heap.global());
                                        (arm, cenv)
                                    })
                                }
                                _ => None,
                            };
                            fast = arm.clone();
                            if !value::is_dynamic(*sym) {
                                heap.vm_call_ic_put(
                                    *site,
                                    crate::core::heap::CallIcEntry {
                                        sym: *sym,
                                        argc: argc as u32,
                                        epoch,
                                        callee: v,
                                        arm,
                                        fast: std::cell::Cell::new(None),
                                    },
                                );
                            }
                            (v, drop_base)
                        }
                    } else {
                        // No IC (a local/dynamic binding shadows the head, or no site):
                        // resolve live each call.
                        let v = match heap.env_get(cur_env, *sym) {
                            Some(v) => v,
                            None => {
                                return Err(tag_pos(crate::eval::unbound_error(heap, *sym), pos))
                            }
                        };
                        (v, drop_base)
                    }
                } else {
                    (heap.root_at(n - argc - 1), n - argc - 1)
                };
                // Inline fast-path: IC hit for the exact same arm, same captured env, no
                // optional/rest params, and GC is not yet due. Covers the common
                // `(defn f (x) … (f …))` self-tail pattern (which uses `Inst::Call` via
                // a global, unlike `letrec` self-recursion which emits `Inst::SelfCall`).
                // This is the main speedup for loop/collatz/fib/reduce.
                //
                // We check the inline condition here — using borrows only — before the
                // `match fast` below consumes `argv` and `fast`. If the check passes we
                // reset the frame and `continue` the inner loop without ever returning to
                // `vm_run_bc`. If it doesn't, we fall through to the normal dispatch path.
                //
                // GC guard: `argv` was read from roots just above, with no allocation in
                // between, so the values are still valid. We skip the inline if GC is due
                // so the outer loop can collect (and can't have stale off-heap SmallVec).
                if *tail {
                    if let Some((ref compiled, cenv)) = fast {
                        if std::ptr::eq(compiled.as_ref(), arm)
                            && arm.noptional == 0
                            && arm.rest_slot.is_none()
                            && cur_env == cenv
                            && !heap.gc_due()
                        {
                            crate::perf_bump!(self_tail);
                            heap.truncate_roots(base + arm.nslots);
                            for i in 0..arm.nslots {
                                heap.set_root_at(base + i, Value::nil());
                            }
                            for i in 0..arm.nrequired {
                                heap.set_root_at(base + i, argv[i]);
                            }
                            *ip = 0;
                            if let Some(used) = crate::core::alloc::soft_limit_hit() {
                                return Err(crate::eval::memory_limit_error(used));
                            }
                            if capture {
                                if crate::process::capture_hard_kill_pending() {
                                    return Ok(ChunkExit::Killed);
                                }
                                if crate::process::tick_capture() {
                                    return Ok(ChunkExit::Preempt);
                                }
                            } else {
                                crate::process::tick();
                            }
                            if crate::process::deadline_exceeded() {
                                return Err(crate::eval::deadline_error());
                            }
                            continue;
                        }
                    }
                }
                // IC hit with a VM arm → skip `dispatch` entirely; else resolve with
                // `tail = true` so a VM-closure callee comes back as `Step::Tail` (the
                // resolved arm + args + env, **un-run**) and a native / tree-walked
                // callee comes back executed as `Step::Done(value)`.
                let step = match fast {
                    Some((arm, cenv)) => Step::Tail {
                        compiled: arm,
                        args: argv,
                        genv: cenv,
                    },
                    None => match dispatch(heap, callee, argv, true, cur_env) {
                        Ok(s) => s,
                        Err(e) if e.is_control() => {
                            // State-capture suspend (ADR-100 §8): a clean `receive`
                            // raised `Control::Suspend` through the `%receive` native.
                            // Rewind `ip` to re-run THIS call on resume (re-scan the
                            // mailbox); the callee + args are still on the operand stack
                            // (the `Err` path never truncated them), so the re-run reads
                            // them back. Hand the driver a `Suspend` to capture the
                            // continuation. Default-off builds never produce the signal.
                            *ip -= 1;
                            let deadline = match &e.control {
                                Some(crate::error::Control::Suspend { deadline }) => *deadline,
                                None => None,
                            };
                            return Ok(ChunkExit::Suspend { deadline });
                        }
                        Err(e) => return Err(tag_pos(e, pos)),
                    },
                };
                if *tail {
                    // Tail position: hand the call to the driver, which reuses this
                    // frame (TCO). Leftover operands are dropped by the driver
                    // (truncate to `base`).
                    return Ok(match step {
                        Step::Tail {
                            compiled,
                            args,
                            genv,
                        } => ChunkExit::Tail {
                            arm: compiled,
                            args,
                            genv,
                        },
                        Step::Done(v) => ChunkExit::Done(v),
                    });
                }
                match step {
                    // Non-tail call to a chunked VM arm: drop the operands (`[args]`, plus a
                    // computed callee) and hand the driver a frame to **push**.
                    Step::Tail {
                        compiled,
                        args,
                        genv,
                    } => {
                        heap.truncate_roots(drop_base);
                        return Ok(ChunkExit::Call {
                            arm: compiled,
                            args,
                            genv,
                        });
                    }
                    // Native / tree-walked callee already ran: push its value and continue.
                    Step::Done(v) => {
                        heap.truncate_roots(drop_base);
                        heap.push_root(v);
                        // GC safepoint: mirror the frequency the BcFrame path gets
                        // from vm_run_bc's outer loop. All live data is on heap.roots
                        // here (frame + result just pushed), so collection is safe.
                        if !crate::process::macro_block_active() && heap.gc_due() {
                            heap.collect(&mut [], &mut []);
                        }
                    }
                }
            }
            Inst::SelfCall { argc } => {
                crate::perf_bump!(self_tail);
                // Direct `letrec` self-tail-call: inline the frame reset and all
                // safepoints so we stay inside this `while` loop instead of
                // round-tripping through `vm_run_bc` on every iteration. Critical for
                // tight tail-recursive loops (loop/collatz/fib): eliminates one Rust
                // call-return and a `SmallVec` construction per iteration.
                //
                // Safety ordering: GC runs first (args still rooted on the operand
                // stack), then args are read (relocated values used), then the frame is
                // reset. No collection fires after the args leave the root stack.
                if !crate::process::macro_block_active() && heap.gc_due() {
                    heap.collect(&mut [], &mut []);
                }
                let n = heap.roots_len();
                let mut argv: SmallVec<[Value; 4]> = SmallVec::with_capacity(*argc);
                for k in 0..*argc {
                    argv.push(heap.root_at(n - argc + k));
                }
                // Reset frame in place (same as the old outer-loop SelfTail handler).
                heap.truncate_roots(base + arm.nslots);
                for i in 0..arm.nslots {
                    heap.set_root_at(base + i, Value::nil());
                }
                for i in 0..arm.nrequired {
                    heap.set_root_at(base + i, argv[i]);
                }
                *ip = 0;
                if let Some(used) = crate::core::alloc::soft_limit_hit() {
                    return Err(crate::eval::memory_limit_error(used));
                }
                if capture {
                    if crate::process::capture_hard_kill_pending() {
                        return Ok(ChunkExit::Killed);
                    }
                    if crate::process::tick_capture() {
                        // Frame already reset; driver captures the continuation as-is.
                        return Ok(ChunkExit::Preempt);
                    }
                } else {
                    crate::process::tick();
                }
                if crate::process::deadline_exceeded() {
                    return Err(crate::eval::deadline_error());
                }
                // Back-edge tiering: periodically hand a hot self-tail loop to the driver
                // so it can tier. The frame is already reset (ip=0, args in slots), so the
                // driver re-enters this same arm at ip 0. We exit only when there's a
                // reason to: native code is installed (run it — it loops internally), or
                // the arm is still untried (drive `jit_tier`'s counter toward the
                // threshold). While QUEUED (compile in flight) or BAILED we stay inline —
                // no round-trips — just an atomic load every `BACKEDGE_TIER_INTERVAL`.
                #[cfg(feature = "jit")]
                {
                    const BACKEDGE_TIER_INTERVAL: u32 = 256;
                    let edges = back_edges.wrapping_add(1);
                    *back_edges = edges;
                    if edges.is_multiple_of(BACKEDGE_TIER_INTERVAL) {
                        let code = arm.jit_code.load(std::sync::atomic::Ordering::Acquire);
                        let installed = !code.is_null()
                            && code != crate::jit::BAILED
                            && code != crate::jit::QUEUED;
                        if installed || code.is_null() {
                            return Ok(ChunkExit::SelfTail);
                        }
                    }
                }
                // Stay in the inner dispatch loop — no function-call round-trip.
                continue;
            }
            Inst::MakeClosure {
                fn_rest,
                names,
                self_name,
            } => {
                // Mirrors `exec_value`'s `Node::MakeClosure`. The capture values are on
                // the operand stack (pushed by preceding leaf insts — safepoint-free,
                // and alloc here never collects mid-pass), so building the env and the
                // closure is collection-free; `env` stays valid until `make_closure`
                // consumes it. With no captures *and* no self-name the closure is
                // global-capturing; a self-name needs a frame to late-bind into.
                let ncap = names.len();
                let n = heap.roots_len();
                let env = if ncap == 0 && self_name.is_none() {
                    heap.global()
                } else {
                    let frame = heap.new_env(Some(heap.global()));
                    for i in 0..ncap {
                        let v = heap.root_at(n - ncap + i);
                        heap.env_define(frame, names[i], v);
                    }
                    frame
                };
                heap.truncate_roots(n - ncap); // drop the capture values
                let closure = crate::eval::make_closure(heap, None, fn_rest.load(), env)?;
                // Direct `letrec` self-recursion: bind the binder name to the closure
                // in its own captured env (the env↔closure cycle the tracing GC owns).
                if let Some(name) = self_name {
                    heap.env_define(env, *name, closure);
                }
                heap.push_root(closure);
            }
            Inst::TryCatch {
                body,
                bind_slot,
                handler,
            } => {
                // SAFETY: NodePtrs reference nodes owned by arm.body (same CompiledArm),
                // which outlives exec_chunk via the Arc<CompiledArm> held by vm_run_bc.
                let body_node = unsafe { body.0.as_ref() };
                let handler_node = unsafe { handler.0.as_ref() };
                // `exec_value` runs the body through the tree-walker, which can't be
                // captured across the native frame boundary. Gate off `capture_top_level`
                // so a `receive` inside the body or handler blocks the worker (the §7.4
                // dirty-scheduler carve-out) rather than attempting a state-capture that
                // can't cross native frames — the same guard `dispatch` applies when it
                // defers a closure to the tree-walker.
                let _guard = CaptureTopGuard(crate::process::set_capture_top_level(false));
                match exec_value(heap, body_node, base, genv) {
                    Ok(v) => heap.push_root(v),
                    Err(e) if e.is_control() => return Err(e),
                    Err(e) => {
                        let caught = match e.payload {
                            Some(v) => v,
                            None => e.to_value_map(heap),
                        };
                        heap.set_root_at(base + bind_slot, caught);
                        match exec_value(heap, handler_node, base, genv) {
                            Ok(v) => heap.push_root(v),
                            Err(e) => return Err(e),
                        }
                    }
                }
            }
        }
    }
    // The body's value is the lone operand left above the frame.
    let n = heap.roots_len();
    Ok(ChunkExit::Done(heap.root_at(n - 1)))
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
struct BcFrame {
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

/// The bytecode driver (ADR-100 Stage 4): run a chunked arm and the **entire chain of
/// chunked calls it makes** on one explicit frame stack, with no native recursion per
/// Brood call. A non-tail call to a chunked arm pushes a frame; a tail call reuses the
/// current frame (TCO); a self-tail-call resets it in place; `Done` pops. Calls to
/// natives / tree-walked arms run inline via `dispatch` (leaves w.r.t. this stack).
/// Every frame's slots live on `Heap::roots` and its env on `Heap::env_roots`, so one
/// safepoint at the loop top relocates the whole stack in place; each frame registers
/// its arm in `live_vm_arms` (hot-reload compaction rewrites every in-flight chunk).
///
/// This is what makes a paused process's continuation **relocatable heap data** — the
/// prerequisite for migrating a running process (concurrency-v2.md §7). `resume` drives
/// state capture (§8, the engine that replaced corosensei): `None` starts `arm0` fresh;
/// `Some(s)` replays a previously [`VmOutcome::Suspended`] continuation from the
/// `%receive` call it parked at, re-entering the loop with `s`'s frame stack (and the
/// operand stack it left on the heap) intact — on **any** worker, no coroutine. A clean
/// `receive` suspend returns `Ok(VmOutcome::Suspended(..))` *without unwinding* (the roots
/// must survive for the resume). The driver runs directly on the worker thread; the
/// continuation lives entirely in `s`, no native stack involved.
fn vm_run_bc(
    heap: &mut Heap,
    arm0: Arc<CompiledArm>,
    args0: &[Value],
    genv0: EnvId,
    resume: Option<Suspended>,
    top_level: bool,
) -> Result<VmOutcome, LispError> {
    crate::perf_bump!(vm_apply);
    // Keep the GC-block depth consistent for any nested native / tree-walked sub-call
    // (their own `stack_overflow_check` reads it). The driver itself doesn't recurse
    // per Brood call — runaway non-tail recursion is caught by `MAX_BC_FRAMES` below,
    // not the native-stack byte guard.
    let _gc_block = crate::process::GcBlockGuard::enter();
    // Publish this driver's `top_level` to the `receive` gate (restored on exit, so the
    // innermost driver wins): a top-level receive captures, a native-nested one blocks.
    struct TopLevelGuard(bool);
    impl Drop for TopLevelGuard {
        fn drop(&mut self) {
            crate::process::set_capture_top_level(self.0);
        }
    }
    let _top_guard = TopLevelGuard(crate::process::set_capture_top_level(top_level));
    // Loop-top **preempt/kill capture** is done only by the *top-level* body driver
    // (`run_process_body`) of a capture-mode green process (ADR-100 §8). A nested
    // `vm_apply` run (a `map`/`try`/`binding` native callback) is NOT top-level: it
    // can't capture a `Preempted`/`Killed` across the native boundary, so it uses the
    // normal `tick`; a `receive` suspend that surfaces there blocks the worker instead
    // (the dirty-scheduler carve-out, §7.4) rather than re-running the native.
    let capture = top_level && crate::process::in_capture_run();

    // Entry marks for a one-shot unwind on error (truncate every frame's roots / env
    // roots / live-arm registrations back to where the driver started). Carried in the
    // `Suspended` so a resumed run still unwinds to the *original* entry on a later error.
    let entry_roots;
    let entry_env;
    let entry_arms;
    // The currently-executing frame is held in locals (not the Vec) so a tail/self
    // loop mutates registers, not the stack — only a non-tail call pushes a `BcFrame`.
    let mut frames: Vec<BcFrame>;
    let mut cur_arm;
    let mut cur_env_base;
    let mut cur_env;
    let mut cur_base;
    let mut cur_arm_slot;
    let mut cur_ip;
    // Persistent back-edge counter for the current frame. Passed as `&mut` into
    // exec_chunk so SelfCall iterations accumulate across exec_chunk re-entries caused
    // by non-tail Brood calls (which exit exec_chunk — a local counter would reset).
    #[cfg(feature = "jit")]
    let mut cur_back_edges: u32;
    // Fresh start (vs. resuming a parked continuation) — the JIT tiering hook fires only
    // on a fresh arm activation, never mid-receive resume.
    let fresh;
    match resume {
        // Resume a parked continuation: its frame stack + operand roots are still on
        // the heap (the suspend didn't unwind), so restore the registers and re-enter
        // the loop at the `%receive` `Inst::Call` it rewound to — no fresh frame push.
        Some(s) => {
            entry_roots = s.entry_roots;
            entry_env = s.entry_env;
            entry_arms = s.entry_arms;
            frames = s.frames;
            let cur = s.cur;
            cur_arm = cur.arm;
            cur_ip = cur.ip;
            cur_base = cur.base;
            cur_env = cur.env;
            cur_env_base = cur.env_base;
            cur_arm_slot = cur.arm_slot;
            #[cfg(feature = "jit")]
            {
                cur_back_edges = cur.back_edges;
            }
            fresh = false;
        }
        // Fresh start: push `arm0`'s activation frame.
        None => {
            entry_roots = heap.roots_len();
            entry_env = heap.env_roots_len();
            entry_arms = heap.live_arm_len();
            frames = Vec::new();
            cur_arm = arm0;
            cur_env_base = heap.env_roots_len();
            cur_env = heap.root_env(genv0);
            cur_base = heap.roots_len();
            cur_arm_slot = if cur_arm.has_runtime_handles {
                heap.live_arm_push(cur_arm.clone())
            } else {
                usize::MAX
            };
            if let Err(e) = push_frame(heap, &cur_arm, args0, cur_env) {
                heap.truncate_roots(entry_roots);
                heap.truncate_env_roots(entry_env);
                heap.live_arm_truncate(entry_arms);
                return Err(e);
            }
            cur_ip = 0usize;
            #[cfg(feature = "jit")]
            {
                cur_back_edges = 0;
            }
            fresh = true;
        }
    }
    let unwind = |heap: &mut Heap| {
        heap.truncate_roots(entry_roots);
        heap.truncate_env_roots(entry_env);
        heap.live_arm_truncate(entry_arms);
    };

    // JIT tiering hook (ADR-101 1b): on a fresh arm activation whose frame is now set up
    // at `roots[cur_base..]`, give the JIT a chance to run it natively. `Done` (0) → the
    // result is in `roots[cur_base]`; unwind the frame and return it. `deopt`/`preempt`
    // (1/2) or not-hot/out-of-subset (None) → fall through to the interpreter loop with
    // the frame intact (for a preempt the slots hold the partial loop state, so the VM —
    // which preempts at its own loop-top since the budget is already spent — resumes from
    // exactly there). Only the int subset is ever compiled; everything else stays here.
    // JIT tiering (ADR-101 1b): try the native code whenever an arm is (re)entered at
    // ip 0 — a fresh activation, a non-tail call's callee, or a tail call's reused frame.
    // `try_jit` flags such an entry; the check runs at the loop top and produces a
    // `ChunkExit` that flows through the *same* handling as the interpreter's output, so
    // a JIT `Done`/`Tail` retires/reuses the frame identically to the VM. A re-entry via
    // tail call thus re-tiers the callee, and an arm *ending* in a tail call tiers too.
    #[cfg(feature = "jit")]
    let mut try_jit = fresh;
    #[cfg(not(feature = "jit"))]
    let _ = fresh; // silence unused warning when the JIT is off

    loop {
        // Per-iteration safepoint / preemption / deadline — relocates every frame's
        // slots and env in place (all on `Heap::roots`/`env_roots`). Mirrors the
        // `Node` trampoline's loop top.
        if !crate::process::macro_block_active() && heap.gc_due() {
            heap.collect(&mut [], &mut []);
        }
        if let Some(used) = crate::core::alloc::soft_limit_hit() {
            unwind(heap);
            return Err(crate::eval::memory_limit_error(used));
        }
        if capture {
            // State-capture preemption/kill (ADR-100 §8.1), in place of the coroutine
            // yield: the frame boundary is the safepoint. A pending hard `:kill` stops
            // now (no capture — the process is retired and its heap dropped); a hit
            // reduction budget captures the continuation so `run_one` re-enqueues it
            // (on any worker — live migration). Both fire only at this clean loop top.
            if crate::process::capture_hard_kill_pending() {
                return Ok(VmOutcome::Killed);
            }
            if crate::process::tick_capture() {
                let cur = BcFrame {
                    arm: cur_arm,
                    ip: cur_ip,
                    base: cur_base,
                    env: cur_env,
                    env_base: cur_env_base,
                    arm_slot: cur_arm_slot,
                    #[cfg(feature = "jit")]
                    back_edges: cur_back_edges,
                };
                return Ok(VmOutcome::Preempted(Suspended {
                    frames,
                    cur,
                    entry_roots,
                    entry_env,
                    entry_arms,
                    deadline: None,
                }));
            }
        } else {
            crate::process::tick();
        }
        if crate::process::deadline_exceeded() {
            unwind(heap);
            return Err(crate::eval::deadline_error());
        }

        // Either run the arm natively (if it's flagged for a tier check) or interpret it.
        // Both yield a `Result<ChunkExit, _>` handled uniformly below.
        let exit =
            {
                #[cfg(feature = "jit")]
                {
                    if try_jit {
                        try_jit = false;
                        // Per-engine frame sizing (two-stage tiering, devlog 2026-06-17): the VM
                        // built the frame to the ORIGINAL `nslots` (small). ONLY when this arm's
                        // *installed* native version is the deferred inlined upgrade does the
                        // native entry need the larger `inline_nslots` frame (the spliced blocks'
                        // shifted slot ranges). `inline_installed` is false for every arm that
                        // doesn't inline (the overwhelming common case — fib is the exception),
                        // so the hot path pays nothing: it calls `jit_tier` exactly as before.
                        // Only the inlined arm grows `roots` and restores the small top on a
                        // non-`Done` outcome (deopt re-runs the ORIGINAL small body from params).
                        let inlined_active = cur_arm
                            .inline_installed
                            .load(std::sync::atomic::Ordering::Acquire);
                        let small_top = cur_base + cur_arm.nslots;
                        if inlined_active {
                            heap.extend_roots_to_nil(cur_base + cur_arm.inline_nslots);
                        }
                        // Clean frame state `jit_tier` runs against: slots set up, operand
                        // stack empty. A deopt/preempt re-run (`exec_chunk` from ip 0) below
                        // assumes roots return to exactly here.
                        let pre_roots = heap.roots_len();
                        let jit_outcome = jit_tier(&cur_arm, heap, cur_base, cur_env);
                        // Restore the small frame top on every non-Done path so the `exec_chunk`
                        // re-run sees the original layout (Done retires the whole frame anyway).
                        // The inlined native keeps operands in registers, so it leaves `roots`
                        // exactly at the frame top it was entered with (`cur_base+inline_nslots`).
                        // A Some(4) tail outcome stages callee+args ABOVE that top, read by
                        // `jit_dispatch_tail` relative to `active_nslots` — don't disturb those.
                        if inlined_active
                            && matches!(jit_outcome, Some(1) | Some(2) | None)
                            && heap.roots_len() == cur_base + cur_arm.inline_nslots
                        {
                            heap.truncate_roots(small_top);
                        }
                        // Work-attribution (perf-stats): native completion (0/4) vs a
                        // mid-run deopt (1) vs preemption (2). A hot arm with high
                        // `jit_deopt` vs `jit_native` compiles but keeps falling off the
                        // native path — the matmul-class signal.
                        match jit_outcome {
                            Some(0) | Some(4) => {
                                crate::perf_bump!(jit_native);
                            }
                            Some(1) => {
                                crate::perf_bump!(jit_deopt);
                            }
                            Some(2) => {
                                crate::perf_bump!(jit_preempt);
                            }
                            _ => {}
                        }
                        // Dirty-stack-on-deopt check: a native arm that deopts (1) or is
                        // preempted (2) must leave `roots` as `jit_tier` found them; if it
                        // grew, the `exec_chunk` re-run starts on a corrupt operand stack.
                        if matches!(jit_outcome, Some(1) | Some(2)) {
                            let now = heap.roots_len();
                            if now != pre_roots {
                                crate::perf_bump!(jit_deopt_dirty);
                                #[cfg(feature = "perf-stats")]
                                {
                                    static SHOWN: std::sync::atomic::AtomicBool =
                                        std::sync::atomic::AtomicBool::new(false);
                                    if !SHOWN.swap(true, std::sync::atomic::Ordering::Relaxed) {
                                        eprintln!(
                                            "[jit-dirty] deopt/preempt left roots_len={now} \
                                         (jit_tier found {pre_roots}) — dirty operand stack \
                                         before the VM re-run"
                                        );
                                    }
                                }
                            }
                        }
                        match jit_outcome {
                            // Done: result in `roots[cur_base]` → the `Done` arm retires it.
                            Some(0) => Ok(ChunkExit::Done(heap.root_at(cur_base))),
                            // A JIT'd call/global errored — propagate the parked error.
                            Some(3) => Err(jit_take_error(heap)
                                .expect("JIT error outcome without a parked error")),
                            // A JIT'd tail call: dispatch the staged callee+args → reuse the
                            // frame (`Tail`) or a finished native callee (`Done`).
                            Some(4) => jit_dispatch_tail(heap, cur_base, &cur_arm, cur_env),
                            // 1 (deopt) / 2 (preempt) / None (not hot / out of subset): run the
                            // arm on the VM with the frame intact (`cur_ip` is still 0).
                            _ => exec_chunk(
                                heap,
                                &cur_arm,
                                &mut cur_ip,
                                cur_base,
                                cur_env,
                                capture,
                                #[cfg(feature = "jit")]
                                &mut cur_back_edges,
                            ),
                        }
                    } else {
                        exec_chunk(
                            heap,
                            &cur_arm,
                            &mut cur_ip,
                            cur_base,
                            cur_env,
                            capture,
                            #[cfg(feature = "jit")]
                            &mut cur_back_edges,
                        )
                    }
                }
                #[cfg(not(feature = "jit"))]
                {
                    exec_chunk(heap, &cur_arm, &mut cur_ip, cur_base, cur_env, capture)
                }
            };
        match exit {
            Ok(ChunkExit::Done(v)) => {
                // Retire the current frame, then either finish or hand `v` to the caller.
                heap.truncate_roots(cur_base);
                heap.truncate_env_roots(cur_env_base);
                if cur_arm_slot != usize::MAX {
                    heap.live_arm_truncate(cur_arm_slot);
                }
                match frames.pop() {
                    None => return Ok(VmOutcome::Done(v)),
                    Some(caller) => {
                        cur_arm = caller.arm;
                        cur_ip = caller.ip;
                        cur_base = caller.base;
                        cur_env = caller.env;
                        cur_env_base = caller.env_base;
                        cur_arm_slot = caller.arm_slot;
                        #[cfg(feature = "jit")]
                        {
                            // Restore the caller's back-edge counter so SelfCall
                            // iterations accumulate correctly across non-tail calls.
                            cur_back_edges = caller.back_edges;
                        }
                        // The result lands where the caller pushed the callee — its
                        // operand stack continues seamlessly past the call site.
                        heap.push_root(v);
                    }
                }
            }
            Ok(ChunkExit::Call { arm, args, genv }) => {
                if frames.len() + 1 > MAX_BC_FRAMES {
                    unwind(heap);
                    return Err(crate::eval::bc_frame_depth_error(frames.len()));
                }
                // Suspend the caller (resume at the already-advanced `cur_ip`) and
                // switch the registers to the callee. `exec_chunk` already dropped the
                // callee+args operands, so the callee's frame starts at `roots_len()`.
                let caller_arm = std::mem::replace(&mut cur_arm, arm);
                frames.push(BcFrame {
                    arm: caller_arm,
                    ip: cur_ip,
                    base: cur_base,
                    env: cur_env,
                    env_base: cur_env_base,
                    arm_slot: cur_arm_slot,
                    #[cfg(feature = "jit")]
                    back_edges: cur_back_edges,
                });
                cur_env_base = heap.env_roots_len();
                cur_env = heap.root_env(genv);
                cur_base = heap.roots_len();
                cur_arm_slot = if cur_arm.has_runtime_handles {
                    heap.live_arm_push(cur_arm.clone())
                } else {
                    usize::MAX
                };
                if let Err(e) = push_frame(heap, &cur_arm, &args, cur_env) {
                    unwind(heap);
                    return Err(e);
                }
                // The callee frame is set up at `roots[cur_base..]` with `cur_ip = 0`; flag
                // it for a tier check at the loop top (the dominant Brood→Brood path). A
                // native `Done`/`Tail`/error is then handled by the shared arms above —
                // identical to the old inline call-site tiering, minus the duplication.
                cur_ip = 0;
                #[cfg(feature = "jit")]
                {
                    try_jit = true;
                    cur_back_edges = 0; // fresh counter for the callee's frame
                }
            }
            Ok(ChunkExit::Tail { arm, args, genv }) => {
                crate::perf_bump!(tail_call);
                // Reuse the current frame for the tail callee (TCO): re-root its env,
                // rebuild its slots in place. Same discipline as the `Node` trampoline.
                heap.truncate_env_roots(cur_env_base);
                cur_env = heap.root_env(genv);
                heap.truncate_roots(cur_base);
                if cur_arm_slot != usize::MAX {
                    heap.live_arm_set(cur_arm_slot, arm.clone());
                } else if arm.has_runtime_handles {
                    cur_arm_slot = heap.live_arm_push(arm.clone());
                }
                if let Err(e) = push_frame(heap, &arm, &args, cur_env) {
                    unwind(heap);
                    return Err(e);
                }
                cur_arm = arm;
                cur_ip = 0;
                // The tail callee occupies a fresh frame at ip 0 — give it a tier check
                // too (whether the tail call came from the VM or a JIT'd arm). This is what
                // lets mutually-recursive arms reached only via tail calls run natively.
                #[cfg(feature = "jit")]
                {
                    try_jit = true;
                    cur_back_edges = 0; // fresh arm, fresh counter
                }
            }
            Ok(ChunkExit::SelfTail) => {
                // Back-edge tiering: a hot self-tail loop handed itself back so we can
                // tier it. The frame is already reset in place (ip=0, the iteration's args
                // in slots) and the operand stack is at the frame top, so we simply
                // re-enter the *same* arm with a tier check — `jit_tier` counts toward the
                // threshold while untried, then runs the installed native code (which
                // loops internally). No frame rebuild; `cur_arm`/`cur_base`/`cur_env` hold.
                cur_ip = 0;
                #[cfg(feature = "jit")]
                {
                    try_jit = true;
                }
            }
            Ok(ChunkExit::Killed) => {
                // Hard kill fired at the inline SelfCall safepoint.
                return Ok(VmOutcome::Killed);
            }
            Ok(ChunkExit::Preempt) => {
                // Reduction budget exhausted at the inline SelfCall safepoint. The frame
                // is already reset (ip=0 inside exec_chunk); capture and re-enqueue.
                let cur = BcFrame {
                    arm: cur_arm,
                    ip: cur_ip,
                    base: cur_base,
                    env: cur_env,
                    env_base: cur_env_base,
                    arm_slot: cur_arm_slot,
                    #[cfg(feature = "jit")]
                    back_edges: cur_back_edges,
                };
                return Ok(VmOutcome::Preempted(Suspended {
                    frames,
                    cur,
                    entry_roots,
                    entry_env,
                    entry_arms,
                    deadline: None,
                }));
            }
            Ok(ChunkExit::Suspend { deadline }) => {
                // A clean `receive` parked (ADR-100 §8). `exec_chunk` rewound `cur_ip`
                // to the suspending `%receive` `Inst::Call` and left the callee + args
                // on the operand stack, so the captured continuation replays straight
                // from there. Capture the whole frame stack as `Suspended` and return
                // it WITHOUT unwinding — the operand stack and frame slots must survive
                // on the heap for the resume (a collection while parked relocates them
                // in place; the saved `base`/`env_base` indices stay valid).
                let cur = BcFrame {
                    arm: cur_arm,
                    ip: cur_ip,
                    base: cur_base,
                    env: cur_env,
                    env_base: cur_env_base,
                    arm_slot: cur_arm_slot,
                    #[cfg(feature = "jit")]
                    back_edges: cur_back_edges,
                };
                return Ok(VmOutcome::Suspended(Suspended {
                    frames,
                    cur,
                    entry_roots,
                    entry_env,
                    entry_arms,
                    deadline,
                }));
            }
            Err(e) => {
                unwind(heap);
                return Err(e);
            }
        }
    }
}

// ===================== entry =====================

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
            for &(sym, _) in bindings {
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
                compile_epoch: AtomicU64::new(0),
                share_key: None,
                shared_published: std::sync::atomic::AtomicBool::new(false),
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
pub(crate) use jit_lower::{jit_lower_arm, jit_lower_inlined_arm, jit_spill_reserve};
// When JIT is disabled, provide a zero stub so callers don't need cfg guards.
#[cfg(not(feature = "jit"))]
fn jit_spill_reserve(_code: &[Inst]) -> usize {
    0
}

/// The background JIT compiler (ADR-101 1b). A single dedicated OS thread, lazily spawned,
/// is the **only** place arms are lowered: it owns the sole mutable access to the JIT
/// module via [`GLOBAL_JIT`](crate::jit::GLOBAL_JIT), so that lock is otherwise
/// uncontended. Worker threads never compile — they hand a hot arm here and keep running
/// the VM until the native pointer is installed.
///
/// This is the fix for the scheduler-starvation flake: compiling Cranelift IR is
/// CPU-bound work of unbounded-ish duration, and doing it inline on a worker thread (while
/// holding `GLOBAL_JIT`) stalls that worker — during a compile burst the whole pool
/// serializes on the lock, and any process waiting on a tight timer (`(after ms …)`,
/// monitor `:down` delivery) can miss its deadline. Moving compilation off the workers
/// decouples scheduler responsiveness from codegen entirely.
///
/// The channel is bounded so a pathological burst can't grow it without limit; on a full
/// queue the enqueue is dropped and the arm reset to "untried" (it re-tiers later). The
/// thread is detached and lives for the process; sends after a (theoretical) hangup are
/// swallowed.
#[cfg(feature = "jit")]
// The work item carries a **slot-tag profile** (`Vec<u8>`, one `Tag as u8` per frame
// slot, snapshotted from a live frame at tier time) alongside the arm, so the
// background compiler can type-specialize float arms without a `CompiledArm` field.
// Empty means "no profile" (integer-only lowering, the pre-float behaviour).
struct JitCompiler {
    /// Primary (initial-tier) queue: the small ORIGINAL arm. Drained first, always.
    primary: std::sync::mpsc::SyncSender<(Arc<CompiledArm>, Vec<u8>)>,
    /// Deferred (lower-priority) queue: the re-derived **inlined** upgrade. The bg thread
    /// pulls from it only when `primary` is empty — so under a spawn-style initial-tier
    /// storm (thousands of short-lived processes tiering their small arms) the inlined
    /// upgrades sit behind the backlog and never compete; a long-lived workload (fib 35)
    /// drains its primary, then the deferred inlined compile lands and the swap fires.
    deferred: std::sync::mpsc::SyncSender<(Arc<CompiledArm>, Vec<u8>)>,
}

/// Permanent keep-alive for every `CompiledArm` whose native code was installed into the
/// process-lifetime `GLOBAL_JIT` module. The native code bakes raw pointers into the arm's
/// chunk `ConstVal`s (read by `brood_rt_const_load`), so the arm (chunk) must outlive the
/// code — i.e. forever. Without this, the arm's only other owners are the closure / call-IC,
/// which are dropped when a closure is rebound or a green process exits, freeing the chunk
/// out from under still-installed native code (bug #2: a dangling ConstVal → garbage const).
#[cfg(feature = "jit")]
static JIT_ARM_KEEPALIVE: std::sync::Mutex<Vec<Arc<CompiledArm>>> =
    std::sync::Mutex::new(Vec::new());

#[cfg(feature = "jit")]
static JIT_COMPILER: std::sync::LazyLock<JitCompiler> = std::sync::LazyLock::new(|| {
    use std::sync::atomic::Ordering::Release;
    use std::sync::mpsc::{sync_channel, TryRecvError};
    let (ptx, prx) = sync_channel::<(Arc<CompiledArm>, Vec<u8>)>(256);
    let (dtx, drx) = sync_channel::<(Arc<CompiledArm>, Vec<u8>)>(256);
    std::thread::Builder::new()
        .name("brood-jit".into())
        .spawn(move || {
            // If codegen ever *panics* (a Cranelift verifier/finalize failure, e.g. an
            // unregistered `brood_rt_*` symbol, or any future lowering bug), don't let
            // the panic kill this thread — that would abandon the receivers, fill the
            // bounded queues, and silently disable the JIT process-wide while the program
            // ran on none the wiser. Catch it, mark the offending arm BAILED, and stop
            // compiling further (the module may be left half-mutated, so subsequent
            // compiles can't be trusted): the process keeps running, correctly, on the
            // interpreter. A single panic still prints once via the default hook — a
            // loud, actionable signal — but doesn't spam or crash.
            let mut codegen_poisoned = false;
            // Lower one work item: `inlined=false` → the small original arm, store into
            // `jit_code`; `inlined=true` → the re-derived inlined body, store into
            // `inline_code` (jit_tier swaps it into `jit_code` later, epoch-bumped).
            let mut compile = |arm: &Arc<CompiledArm>, slot_tags: &[u8], inlined: bool| {
                let slot = if inlined {
                    &arm.inline_code
                } else {
                    &arm.jit_code
                };
                if codegen_poisoned {
                    slot.store(crate::jit::BAILED, Release);
                    return;
                }
                let mut jit = crate::jit::GLOBAL_JIT
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let lowered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    if inlined {
                        jit_lower_inlined_arm(&mut jit, arm, slot_tags)
                    } else {
                        jit_lower_arm(&mut jit, arm, slot_tags)
                    }
                }));
                drop(jit); // install the pointer outside the module lock
                match lowered {
                    Ok(Some(ptr)) => {
                        slot.store(ptr as *mut u8, Release);
                        // The installed native code lives forever in GLOBAL_JIT and bakes raw
                        // pointers into this arm's chunk `ConstVal`s. Keep the arm (hence its
                        // chunk) alive permanently so those pointers never dangle when the
                        // closure / call-IC that referenced it is dropped (e.g. a green process
                        // exits) — the bug-#2 use-after-free: a freed ConstVal chunk fed garbage
                        // consts (a garbage map_get key) into still-installed native code.
                        JIT_ARM_KEEPALIVE
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .push(arm.clone());
                    }
                    Ok(None) => slot.store(crate::jit::BAILED, Release),
                    Err(_) => {
                        codegen_poisoned = true;
                        slot.store(crate::jit::BAILED, Release);
                    }
                }
            };
            loop {
                // 1. Drain the entire primary queue before touching deferred — the
                //    initial-tier work always wins the compiler.
                match prx.try_recv() {
                    Ok((arm, tags)) => {
                        compile(&arm, &tags, false);
                        continue;
                    }
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => break,
                }
                // 2. Primary empty: take one deferred inlined upgrade if any.
                match drx.try_recv() {
                    Ok((arm, tags)) => {
                        compile(&arm, &tags, true);
                        continue;
                    }
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => {}
                }
                // 3. Both empty: block on the primary (initial tier latency matters), but
                //    only briefly — so a deferred item enqueued while we slept is picked up
                //    promptly once primary stays quiet. A 1ms idle poll is free (the thread
                //    is otherwise sleeping) and never delays a primary send (which wakes it).
                match prx.recv_timeout(std::time::Duration::from_millis(1)) {
                    Ok((arm, tags)) => compile(&arm, &tags, false),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        })
        .expect("spawn brood-jit compiler thread");
    JitCompiler {
        primary: ptx,
        deferred: dtx,
    }
});

/// Are all the arm chunk's inlined 2-ary primitives still bound to their native
/// implementations (ADR-096 §4.A epoch-guard, evaluated eagerly)? The JIT lowers
/// `+`/`<`/… to raw machine ops, which is sound only while the head symbol resolves to
/// the matching `%`-native (and arg-map). A `(def + …)` rebinds it; [`resolve_prim`]
/// reads the live global env, so this returns `false` for the redefined operator and
/// the arm must stay on the VM (which dispatches to the new definition). Non-prim
/// instructions can't be invalidated, so they pass. A chunkless arm passes here and is
/// bailed by [`jit_lower_arm`] instead.
#[cfg(feature = "jit")]
fn chunk_ops_all_native(heap: &Heap, arm: &CompiledArm) -> bool {
    let Some(chunk) = arm.chunk.as_ref() else {
        return true;
    };
    chunk.code.iter().all(|inst| match inst {
        Inst::Prim2 { op, map, head, .. } | Inst::Prim2SlotSlot { op, map, head, .. } => {
            // These store the head's *natural* arg-map (what `resolve_prim` returns).
            matches!(
                resolve_prim(heap, *head),
                Some((o, m)) if o == *op && m == [map[0] as usize, map[1] as usize]
            )
        }
        Inst::Prim2SlotInt {
            op,
            map,
            head,
            swapped,
            ..
        } => {
            // A `(Const, Local)` fusion inverts the map so the slot is operand 0 (and sets
            // `swapped`). Un-invert before comparing to `resolve_prim`'s natural map —
            // otherwise a commutative `(op const local)` like `(* 3 m)` spuriously fails
            // this check and the whole (valid) arm is wrongly marked BAILED, never JITs.
            // Mirrors the revalidation in `prim2_inline_exec`.
            let want = if *swapped {
                [1 - map[0] as usize, 1 - map[1] as usize]
            } else {
                [map[0] as usize, map[1] as usize]
            };
            matches!(resolve_prim(heap, *head), Some((o, m)) if o == *op && m == want)
        }
        _ => true,
    })
}

/// Take the error a JIT runtime callback parked (see [`Heap::jit_pending_error`]) — called
/// by [`vm_run_bc`] on the error outcome.
#[cfg(feature = "jit")]
pub(crate) fn jit_take_error(heap: &mut Heap) -> Option<LispError> {
    heap.jit_pending_error.take()
}

/// Resolve free global `sym` in the executing JIT'd arm's env — the callee-loading
/// `Inst::Global`/`GlobalIc` lowering (and a global read in value position). Returns the
/// value, or parks an unbound error and returns `None`. Reads the *live* env each call,
/// so a `def` rebind is seen immediately (the same late binding as `Inst::Global`).
#[cfg(feature = "jit")]
#[inline]
pub(crate) fn jit_resolve_global(heap: &mut Heap, sym: Symbol) -> Option<Value> {
    let env = heap.read_root_env(heap.jit_call_env);
    match heap.env_get(env, sym) {
        Some(v) => Some(v),
        None => {
            let e = crate::eval::unbound_error(heap, sym);
            heap.jit_pending_error = Some(e);
            None
        }
    }
}

/// Resolve free global `sym` through the per-`site` global inline cache — the JIT
/// equivalent of the VM's `Inst::GlobalIc`, sharing the same [`Heap::vm_global_ics`]
/// entries. On a process-global env, a cached value stamped at the current epoch is
/// returned without an `env_get` walk; a miss resolves once and fills the cache. This
/// is the difference between a hot recursive callee (`fib` resolving itself every call)
/// costing one cached read vs. a full name resolution per call — the cost that made
/// native-linked recursion regress `spawn` (millions of redundant `env_get`s). Late
/// binding holds via the epoch stamp (a `def` bumps the epoch → miss → re-resolve;
/// the JIT'd arm is invalidated by the same epoch). Dynamic vars are never cached.
#[cfg(feature = "jit")]
#[inline]
pub(crate) fn jit_resolve_global_ic(heap: &mut Heap, sym: Symbol, site: u32) -> Option<Value> {
    let env = heap.read_root_env(heap.jit_call_env);
    if heap.is_global(env) {
        let epoch = heap.global_epoch();
        if let Some(v) = heap.vm_global_ic_probe(site, sym, epoch) {
            crate::perf_bump!(global_ic_hit);
            return Some(v);
        }
        crate::perf_bump!(global_ic_miss);
        match heap.env_get(env, sym) {
            Some(v) => {
                if !value::is_dynamic(sym) {
                    heap.vm_global_ic_put(site, sym, epoch, v);
                }
                Some(v)
            }
            None => {
                let e = crate::eval::unbound_error(heap, sym);
                heap.jit_pending_error = Some(e);
                None
            }
        }
    } else {
        match heap.env_get(env, sym) {
            Some(v) => Some(v),
            None => {
                let e = crate::eval::unbound_error(heap, sym);
                heap.jit_pending_error = Some(e);
                None
            }
        }
    }
}

/// Cap on native-to-native recursion (see [`Heap::jit_native_depth`]). Past this many
/// native levels, drain the rest of the subtree on the VM (heap frames, bounded by
/// [`MAX_BC_FRAMES`]) so deep non-tail recursion keeps working instead of overflowing the
/// native stack. 1 500 levels (~a few MB of the 16 MB worker stack) dwarfs any real depth.
#[cfg(feature = "jit")]
pub(crate) const JIT_NATIVE_DEPTH_LIMIT: u32 = 1500;

/// The result of running a validated native fast-link ([`jit_run_fast_link`]): the call
/// completed (`Done`), raised an error parked for the arm to propagate (`Error`), or could
/// not be fast-linked after all (`Fallthrough` — the IC moved under us; the args have been
/// re-staged for the caller's slow path).
#[cfg(feature = "jit")]
pub(crate) enum FastLinkOutcome {
    Done(Value),
    Error,
    Fallthrough,
}

/// The shared body of a validated native fast-link: set up the callee frame at `stage_base`,
/// call its installed native `code`, and handle the outcome — `Done` (result boxed in
/// `roots[stage_base]`), the parked-error exit, or a deopt/preempt/tail that re-runs the
/// callee on the VM via the IC. Both [`jit_dispatch_call`] (after `vm_call_ic_fast_link`)
/// and [`jit_dispatch_fast_frame`] (the in-IR epoch-guarded path, which reads `code/nslots/
/// env` from the flat side table instead) funnel through here, so the two can never desync.
/// `epoch`/`stage_base` are the caller's already-computed values; `code` is a finalized
/// `extern "C" fn(*mut Heap, i64) -> i64`. On `Fallthrough` the `argc` args are re-staged at
/// `[stage_base, stage_base+argc)` for the caller's slow path.
#[cfg(feature = "jit")]
#[allow(clippy::too_many_arguments)]
fn jit_run_fast_link(
    heap: &mut Heap,
    argc: usize,
    site: u32,
    head: Symbol,
    epoch: u64,
    stage_base: usize,
    code: usize,
    nslots: usize,
    callee_env: EnvId,
) -> FastLinkOutcome {
    heap.truncate_roots(stage_base + argc);
    // DEBUG ONLY: the JIT fast path bypasses `push_frame`, so validate the staged args
    // here too — catch a corrupt arg at the earliest native frame entry (bug #2).
    #[cfg(debug_assertions)]
    {
        let args: Vec<Value> = (0..argc).map(|k| heap.root_at(stage_base + k)).collect();
        dbg_check_args(
            &args,
            &format!(
                "jit_run_fast_link site={site} loc={}",
                heap.dbg_site_loc(site)
            ),
        );
    }
    // Runtime BROOD_JIT_VERIFY: the fast-link path bypasses jit_dispatch_call's scan, so
    // scan the staged args here too (works in a plain --release build).
    if jit_verify_active() {
        jit_verify_staged(heap, stage_base, stage_base + argc, head, site, argc);
    }
    heap.extend_roots_to_nil(stage_base + nslots);
    let base = stage_base;
    // SAFETY: `code` is a finalized `extern "C" fn(*mut Heap, base)` from `jit_lower_arm`,
    // kept for the process in `GLOBAL_JIT`; the frame is at `roots[base..]`. Validated
    // current by the caller's epoch check (the IC fast-link, or the IR's flat-table guard).
    let f: extern "C" fn(*mut Heap, i64) -> i64 = unsafe { std::mem::transmute(code as *mut u8) };
    let depth = heap.jit_native_depth;
    // Root callee_env via env_roots so GC tenure inside the callee forwards it.
    let env_base = heap.env_roots_len();
    let env_root = heap.root_env(callee_env);
    let saved = std::mem::replace(&mut heap.jit_call_env, env_root);
    let saved_fn = std::mem::replace(&mut heap.jit_dbg_fn, head);
    heap.jit_native_depth = depth + 1;
    let outcome = f(heap as *mut Heap, base as i64);
    heap.jit_native_depth = depth;
    heap.jit_call_env = saved;
    heap.jit_dbg_fn = saved_fn;
    heap.truncate_env_roots(env_base);
    match outcome {
        0 => {
            crate::perf_bump!(jit_link_done);
            let result = heap.root_at(base);
            heap.truncate_roots(stage_base);
            FastLinkOutcome::Done(result)
        }
        3 => {
            heap.truncate_roots(stage_base);
            FastLinkOutcome::Error
        }
        // Tail call (outcome 4): the callee JIT'd a tail call — [callee, arg0..argN] are staged
        // in roots above the callee's frame at `[base+nslots, roots_len)`. Rather than discarding
        // the staged call and re-running the callee via `vm_apply` (which would pay both JIT and
        // VM overhead for every tail-calling callee), follow the chain: dispatch the staged call
        // as if the callee had returned that value. This makes JIT-compiled thin delegators
        // (e.g. `prime?` tail-calling `divides-none?`) called in non-tail position efficient.
        4 => {
            let staged_start = base + nslots;
            let staged_end = heap.roots_len();
            if staged_end > staged_start {
                let staged_callee = heap.root_at(staged_start);
                let staged_argc = staged_end - staged_start - 1;
                let staged_args: SmallVec<[Value; 4]> = (1..=staged_argc)
                    .map(|k| heap.root_at(staged_start + k))
                    .collect();
                heap.truncate_roots(stage_base);
                return match apply_value(heap, staged_callee, &staged_args, heap.global()) {
                    Ok(v) => FastLinkOutcome::Done(v),
                    Err(e) => {
                        heap.jit_pending_error = Some(e);
                        FastLinkOutcome::Error
                    }
                };
            }
            // No staged call staged (shouldn't happen): fall back.
            heap.truncate_roots(stage_base);
            FastLinkOutcome::Error
        }
        // deopt (1) / preempt (2): re-run on the VM. The args survive in the param
        // slots `[base, base+argc)`. Re-probe for the arm (clones — but only on this rare
        // path) and `vm_apply`.
        _ => {
            crate::perf_bump!(jit_link_rerun);
            let mut argv2: SmallVec<[Value; 4]> = SmallVec::with_capacity(argc);
            for k in 0..argc {
                argv2.push(heap.root_at(base + k));
            }
            heap.truncate_roots(stage_base);
            if let Some((_, Some((arm, cenv)))) =
                heap.vm_call_ic_probe(site, head, argc as u32, epoch)
            {
                return match vm_apply(heap, arm, &argv2, cenv) {
                    Ok(v) => FastLinkOutcome::Done(v),
                    Err(e) => {
                        heap.jit_pending_error = Some(e);
                        FastLinkOutcome::Error
                    }
                };
            }
            // IC changed under us: restage the args so the elided slow path finds them.
            for a in &argv2 {
                heap.push_root(*a);
            }
            FastLinkOutcome::Fallthrough
        }
    }
}

/// The JIT's **in-IR** fast call path (Track B / Technique A). The arm's IR has already
/// validated this elided call site's flat-table fast-link (`site < len` && `epoch ==
/// global_epoch` && the slot's `sym`/`argc` match this site's baked head/arity — the last
/// guards against a call-site id reused across a `runtime_collect` clear, ADR-096) and read
/// `(code, nslots, env)` out of [`Heap::vm_fast_links`] with raw
/// loads — so this skips the IC probe + `RefCell` borrow that [`jit_dispatch_call`]'s fast
/// path pays (the measured 40.9%-of-`fib` cost) and runs the same frame body via
/// [`jit_run_fast_link`]. The `argc` args are the top operands on `roots`. Returns a
/// [`FastLinkOutcome`] the caller maps to a status: `Done` (result), `Error` (parked), or
/// `Fallthrough` — over the native-recursion cap, or the IC moved — which sends the IR to
/// the `brood_rt_call_slow` miss path with the args left staged.
#[cfg(feature = "jit")]
#[allow(clippy::too_many_arguments)]
pub(crate) fn jit_dispatch_fast_frame(
    heap: &mut Heap,
    site: u32,
    head: Symbol,
    argc: usize,
    nslots: usize,
    code: usize,
    env: u64,
) -> FastLinkOutcome {
    let n = heap.roots_len();
    let epoch = heap.global_epoch();
    // Elided (free-global) head: the args are the top `argc` operands; the frame starts there.
    let stage_base = n - argc;
    // Over the native-recursion cap → don't link (would overflow the native stack); the args
    // stay staged at `[stage_base, n)` so the slow path drains the recursion on the VM.
    if heap.jit_native_depth >= JIT_NATIVE_DEPTH_LIMIT {
        return FastLinkOutcome::Fallthrough;
    }
    let callee_env = EnvId(env);
    // Cross-check (debug only, fires in the gate): the flat-table values the IR handed us
    // must equal what the authoritative IC fast-link resolves at this epoch — a mismatch is
    // a mirror desync and a silent-wrong-answer risk.
    #[cfg(debug_assertions)]
    {
        let auth = heap.vm_call_ic_fast_link(site, head, argc as u32, epoch);
        debug_assert!(
            matches!(auth, Some((c, ns, e)) if c as usize == code && ns == nslots && e == callee_env),
            "fast-link mirror desynced from the call IC (site {site}, head {head}): \
             mirror=(code={code:#x}, nslots={nslots}, env={:#x}) auth={auth:?} — the IR's \
             epoch+sym+argc guard should make this unreachable (see FastLink)",
            callee_env.0
        );
    }
    jit_run_fast_link(
        heap, argc, site, head, epoch, stage_base, code, nslots, callee_env,
    )
}

/// Run a JIT'd arm's **non-tail** Brood→Brood call. The `argc` args are the top operands
/// on `roots`. A **free-global** head (`site != NO_SITE`) is *not* staged — the callee is
/// resolved here via the call-site IC (`head` + `site`), so the args occupy `[n-argc, n)`
/// and the frame starts at `n-argc`. A **computed** head leaves the callee staged below the
/// args (`[n-argc-1]`). The fast path links straight to the callee's native code; otherwise
/// [`dispatch`] runs it (`tail = false` ⇒ to completion) as a **nested** (non-top-level)
/// run, so it never preempts/suspends across the native boundary (the §7.4 carve-out).
#[cfg(feature = "jit")]
pub(crate) fn jit_dispatch_call(
    heap: &mut Heap,
    argc: usize,
    site: u32,
    head: Symbol,
) -> Option<Value> {
    use std::sync::atomic::Ordering::Acquire;
    let n = heap.roots_len();
    let over_cap = heap.jit_native_depth >= JIT_NATIVE_DEPTH_LIMIT;
    let epoch = heap.global_epoch();
    // A free-global head isn't staged (`elided`): the callee is resolved via the call IC.
    // `stage_base` is where the callee frame starts — directly at the args for an elided
    // head, one slot lower (over the staged callee) for a computed one.
    let elided = site != NO_SITE;
    let stage_base = if elided { n - argc } else { n - argc - 1 };

    #[cfg(debug_assertions)]
    {
        for k in stage_base..n {
            let v = heap.root_at(k);
            if let Some((kind, g, e)) = heap.dbg_value_stale(v) {
                let raw = unsafe { std::mem::transmute::<Value, [i64; 3]>(v) };
                eprintln!(
                    "[jit-staged-stale] STALE {kind} (gen {g} != live {e}) staged at roots[{k}] \
                     BY arm '{}' for call to '{}' at {} (site={site}, argc={argc}); raw=[{:#x},{:#x},{:#x}]",
                    crate::core::value::symbol_name_opt(heap.jit_dbg_fn).unwrap_or("<unknown>"),
                    crate::core::value::symbol_name_opt(head).unwrap_or("<computed>"),
                    heap.dbg_site_loc(site),
                    raw[0], raw[1], raw[2],
                );
            }
        }
    }
    // Runtime BROOD_JIT_VERIFY: same scan in a plain --release build.
    if jit_verify_active() {
        jit_verify_staged(heap, stage_base, n, head, site, argc);
    }

    // ---- Fast native link (no per-call Arc clone) ----
    // The hot recursive case (`fib`, a free-global head). `vm_call_ic_fast_link` validates
    // the whole link (sym/argc/epoch + installed + simple arm) and returns Copy data — no
    // `Arc::clone` (the one atomic-RMW per call the older cloning path below pays ~30M
    // times). Args are already staged at `[stage_base, stage_base+argc)`. Mirrors the
    // cloning path's frame setup + outcome handling; deopt (rare) re-probes for the arm.
    if elided && !over_cap {
        if let Some((code, nslots, callee_env)) =
            heap.vm_call_ic_fast_link(site, head, argc as u32, epoch)
        {
            match jit_run_fast_link(
                heap,
                argc,
                site,
                head,
                epoch,
                stage_base,
                code as usize,
                nslots,
                callee_env,
            ) {
                FastLinkOutcome::Done(v) => return Some(v),
                FastLinkOutcome::Error => return None,
                // IC changed under us (astronomically rare): the args were re-staged at
                // `[stage_base, ..)` — fall through to the slow path below.
                FastLinkOutcome::Fallthrough => {}
            }
        }
    }

    // ---- Native-to-native call linking ----
    // Link straight to the callee's installed, epoch-current native code — set up its frame
    // at `stage_base` and call its entry — skipping `dispatch → vm_apply → vm_run_bc →
    // jit_tier`. The arm (and captured env) come from the call-site IC (reusing the VM's
    // `vm_call_ic`, epoch-stamped): a hit costs no `env_get` and no `compiled_arm_for`. The
    // frame is exactly where the VM puts a callee frame, so this holds no more roots than the
    // interpreter. These sites bypass `exec_chunk`, so the JIT self-populates the IC on a miss.
    {
        let resolved: Option<(Arc<CompiledArm>, EnvId)> = if elided {
            match heap.vm_call_ic_probe(site, head, argc as u32, epoch) {
                Some((_, Some((a, env)))) => Some((a, env)),
                _ => {
                    // Miss: resolve the callee global (the only `env_get` on the call path,
                    // and only while cold) and fill the IC.
                    let cenv = heap.read_root_env(heap.jit_call_env);
                    match heap.env_get(cenv, head).map(|v| v.unpack()) {
                        Some(ValueRef::Fn(id)) => compiled_arm_for(heap, id, argc).map(|a| {
                            let env = heap.closure(id).env.unwrap_or_else(|| heap.global());
                            if !value::is_dynamic(head) {
                                heap.vm_call_ic_put(
                                    site,
                                    crate::core::heap::CallIcEntry {
                                        sym: head,
                                        argc: argc as u32,
                                        epoch,
                                        callee: Value::func(id),
                                        arm: Some((a.clone(), env)),
                                        fast: std::cell::Cell::new(None),
                                    },
                                );
                            }
                            (a, env)
                        }),
                        _ => None,
                    }
                }
            }
        } else if let ValueRef::Fn(id) = heap.root_at(stage_base).unpack() {
            compiled_arm_for(heap, id, argc)
                .map(|a| (a, heap.closure(id).env.unwrap_or_else(|| heap.global())))
        } else {
            None
        };
        if let Some((arm, callee_env)) = resolved {
            let code = arm.jit_code.load(Acquire);
            let installed =
                !code.is_null() && code != crate::jit::BAILED && code != crate::jit::QUEUED;
            // `nslots > 0` mirrors `jit_lower_arm`'s return-via-`roots[base]` requirement;
            // no-optional/no-rest keeps the inline frame setup trivial and infallible. The
            // epoch guard mirrors `jit_tier`. Over the recursion cap → skip (the slow path
            // drains on the VM via `jit_force_vm`).
            if installed
                && arm.nslots > 0
                && arm.noptional == 0
                && arm.rest_slot.is_none()
                && !over_cap
                && arm.compile_epoch.load(Acquire) == epoch
            {
                let depth = heap.jit_native_depth;
                // Build the callee frame at `stage_base`. For an elided head the args are
                // already in place (`[stage_base, stage_base+argc)`); for a computed head the
                // dead callee slot sits below them, so shift the args down one (forward-safe:
                // each write is below its read). Then nil-fill the let/spill slots.
                if !elided {
                    for k in 0..argc {
                        let a = heap.root_at(stage_base + 1 + k);
                        heap.set_root_at(stage_base + k, a);
                    }
                }
                heap.truncate_roots(stage_base + argc);
                // Two-stage tiering: size the callee frame to its *installed* native version
                // (inlined upgrade → `inline_nslots`; small → `nslots`). Capture once and reuse
                // for both frame extension and the outcome-4 staged_start calculation — the two
                // must agree on the same frame boundary.
                let frame_nslots = arm.active_nslots();
                heap.extend_roots_to_nil(stage_base + frame_nslots);
                let base = stage_base;
                // SAFETY: `code` is a finalized `extern "C" fn(*mut Heap, base)` from
                // `jit_lower_arm`, living for the process in `GLOBAL_JIT`; the frame is set
                // up at `roots[base..]`.
                let f: extern "C" fn(*mut Heap, i64) -> i64 = unsafe { std::mem::transmute(code) };
                // Root callee_env via env_roots so GC tenure inside the callee forwards it.
                let env_base = heap.env_roots_len();
                let env_root = heap.root_env(callee_env);
                let saved = std::mem::replace(&mut heap.jit_call_env, env_root);
                let saved_fn = std::mem::replace(&mut heap.jit_dbg_fn, head);
                // Fill the closure's capture slots from its captured env. The fast frame
                // setup above placed only the params (and `extend_roots_to_nil` zeroed the
                // rest) — it bypasses `push_frame`, which is where captures are normally
                // filled. Without this, a callee WITH captures reads its captured lexicals
                // (e.g. a fold reducer's free `dir`) as nil, producing wrong results /
                // type errors far away (`path-join nil …` → `string-length: got nil`).
                // capture_base == argc here: noptional == 0 && rest_slot is none (guarded
                // above) and nrequired == argc (the arm was selected for this argc). Reads
                // are alloc-free (no GC), so the nil-filled body slots above stay valid.
                if !arm.capture_names.is_empty() {
                    let cenv = heap.read_root_env(env_root);
                    for (k, &name) in arm.capture_names.iter().enumerate() {
                        let v = heap.capture_value(cenv, k, name);
                        heap.set_root_at(stage_base + argc + k, v);
                    }
                }
                heap.jit_native_depth = depth + 1;
                let outcome = f(heap as *mut Heap, base as i64);
                heap.jit_native_depth = depth;
                heap.jit_call_env = saved;
                heap.jit_dbg_fn = saved_fn;
                // `f()` runs the callee, which allocates freely and so may have triggered a
                // collection that *relocated* the captured env. `minor_collect` forwarded the
                // rooted copy (`env_root`) but NOT the local `callee_env` EnvId — re-read the
                // live id from its root before dropping it. Without this the deopt path below
                // hands `vm_apply` a stale env handle → `push_frame`/`env_frame` use-after-GC
                // (the whole reason `callee_env` was env-rooted at all). The other outcomes
                // read their results from `roots` (already GC-updated), so this is the one
                // post-`f()` consumer of the locally-held handle.
                let callee_env = heap.read_root_env(env_root);
                heap.truncate_env_roots(env_base);
                match outcome {
                    // Done: result boxed in `roots[base]`. Take it, drop the frame.
                    0 => {
                        crate::perf_bump!(jit_link_done);
                        let result = heap.root_at(base);
                        heap.truncate_roots(stage_base);
                        return Some(result);
                    }
                    // Error: callee parked it. PROPAGATE — never re-run, or an already-failed
                    // subtree re-errors at every unwinding level (quadratic).
                    3 => {
                        heap.truncate_roots(stage_base);
                        return None;
                    }
                    // Tail call (4): the callee JIT'd a tail — [callee, arg0..argN] staged above
                    // its frame at `[base+frame_nslots, roots_len)`. Follow the chain rather than
                    // re-running the callee via `vm_apply` (which would pay both JIT and VM cost).
                    4 => {
                        let staged_start = base + frame_nslots;
                        let staged_end = heap.roots_len();
                        if staged_end > staged_start {
                            let staged_callee = heap.root_at(staged_start);
                            let staged_argc = staged_end - staged_start - 1;
                            let staged_args: SmallVec<[Value; 4]> = (1..=staged_argc)
                                .map(|k| heap.root_at(staged_start + k))
                                .collect();
                            heap.truncate_roots(stage_base);
                            return match apply_value(
                                heap,
                                staged_callee,
                                &staged_args,
                                heap.global(),
                            ) {
                                Ok(v) => Some(v),
                                Err(e) => {
                                    heap.jit_pending_error = Some(e);
                                    None
                                }
                            };
                        }
                        heap.truncate_roots(stage_base);
                        return None;
                    }
                    // deopt (1) / preempt (2): re-run the callee on the VM. The args
                    // survive in the frame's param slots `[base, base+argc)` (params aren't
                    // overwritten by the arm body), so re-read, drop the frame, and `vm_apply`.
                    _ => {
                        crate::perf_bump!(jit_link_rerun);
                        let mut argv2: SmallVec<[Value; 4]> = SmallVec::with_capacity(argc);
                        for k in 0..argc {
                            argv2.push(heap.root_at(base + k));
                        }
                        heap.truncate_roots(stage_base);
                        return match vm_apply(heap, arm, &argv2, callee_env) {
                            Ok(v) => Some(v),
                            Err(e) => {
                                heap.jit_pending_error = Some(e);
                                None
                            }
                        };
                    }
                }
            }
        }
    }

    // ---- Slow path ---- (not linkable: not yet native, over the cap, or a non-closure /
    // unbound callee). Resolve the callee (elided: via `env_get`; computed: the staged slot)
    // and run it on the VM. The args are the top `argc` operands either way.
    let callee = if elided {
        let cenv = heap.read_root_env(heap.jit_call_env);
        match heap.env_get(cenv, head) {
            Some(v) => v,
            None => {
                heap.jit_pending_error = Some(crate::eval::unbound_error(heap, head));
                return None;
            }
        }
    } else {
        heap.root_at(stage_base)
    };
    let mut argv: SmallVec<[Value; 4]> = SmallVec::with_capacity(argc);
    for k in 0..argc {
        argv.push(heap.root_at(n - argc + k));
    }
    let env = heap.read_root_env(heap.jit_call_env);
    // Over the native cap: force this dispatch (and all it recurses into) onto the VM, so the
    // remaining recursion drains through the bounded heap-frame loop. Restored after.
    let saved_force = if over_cap {
        Some(std::mem::replace(&mut heap.jit_force_vm, true))
    } else {
        None
    };
    let result = match dispatch(heap, callee, argv, false, env) {
        Ok(Step::Done(v)) => Ok(v),
        Ok(Step::Tail {
            compiled,
            args,
            genv,
        }) => vm_apply(heap, compiled, &args, genv),
        Err(e) => Err(e),
    };
    if let Some(prev) = saved_force {
        heap.jit_force_vm = prev;
    }
    match result {
        Ok(v) => {
            heap.truncate_roots(stage_base);
            // GC safepoint: mirrors vm_run_bc's outer-loop check so native
            // calls from the JIT get GC opportunities at the same cadence as
            // the BcFrame path. Root `v` first so it survives relocation.
            if !crate::process::macro_block_active() && heap.gc_due() {
                heap.push_root(v);
                heap.collect(&mut [], &mut []);
                let relocated = heap.root_at(heap.roots_len() - 1);
                heap.truncate_roots(heap.roots_len() - 1);
                Some(relocated)
            } else {
                Some(v)
            }
        }
        Err(e) => {
            // Symmetric with the `Ok` arm: drop the call's staged operands
            // (callee + args at `[stage_base, n)`) now that the call failed. Safe —
            // the thrown value rides in `e` (off the roots stack), and this arm does
            // no GC (only the `Ok` arm collects), so nothing can go stale; this just
            // frees the staged roots immediately instead of leaving them for the
            // `try` handler's `truncate_roots(entry_roots)` to reclaim later.
            heap.truncate_roots(stage_base);
            heap.jit_pending_error = Some(e);
            None
        }
    }
}

/// Run a JIT'd arm's **tail** Brood→Brood call (outcome 4). The callee + `argc` args were
/// staged on `roots` *above the frame top* (`base + nslots`) in the VM's `Inst::Call`
/// layout (`[.., callee, arg0 .. arg_{argc-1}]`) — `argc` is recovered from the root
/// length since the JIT keeps its own operands in registers (so the frame top is always
/// exactly `base + nslots`). Unlike the non-tail path, the call *is* the arm's result
/// (TCO), so this resolves it with `tail = true` and hands [`vm_run_bc`] a [`ChunkExit`]
/// to **reuse** the current frame with — `Tail` for a VM-closure callee (run on the main
/// driver loop, keeping full preempt/suspend support), `Done` for an already-run
/// native/tree-walked callee. The native stack never grows: the driver's loop is the
/// trampoline. Mirrors the tail branch of the VM's `Inst::Call`.
#[cfg(feature = "jit")]
fn jit_dispatch_tail(
    heap: &mut Heap,
    base: usize,
    arm: &CompiledArm,
    env: EnvRoot,
) -> Result<ChunkExit, LispError> {
    // Two-stage tiering: a tail call is staged by the native code ABOVE its own frame top,
    // which is `active_nslots` (the inlined upgrade runs with the bigger frame). Use the
    // active size so the staged `[callee, args…]` is read at the right offset.
    let top = base + arm.active_nslots();
    let n = heap.roots_len();
    let argc = n - top - 1;
    let callee = heap.root_at(top);
    // Verify the staged tail-call args too (BROOD_JIT_VERIFY / _FN) — the tail path is
    // separate from jit_dispatch_call, and a tail-called callee (e.g. pong's lambda
    // tail-calling `badge-ops`) stages its args here. The callee is a Value, so resolve
    // its closure name for the `_FN` match (u32::MAX = anonymous → "<computed>").
    if jit_verify_active() {
        let head = match callee.unpack() {
            crate::core::value::ValueRef::Fn(id) => heap.closure(id).name.unwrap_or(u32::MAX),
            _ => u32::MAX,
        };
        jit_verify_staged(heap, top + 1, n, head, NO_SITE, argc);
    }
    let mut argv: SmallVec<[Value; 4]> = SmallVec::with_capacity(argc);
    for k in 0..argc {
        argv.push(heap.root_at(top + 1 + k));
    }
    let env_id = heap.read_root_env(env);
    // `dispatch(.., tail = true, ..)` resolves a VM-closure callee to a `Step::Tail`
    // **without running it** (no native recursion) and runs a native/tree-walked callee
    // to a `Step::Done`. An error (incl. a control/suspend from a directly tail-called
    // suspending native — unreachable from surface `receive`, whose match closure puts
    // the arm out of subset) propagates; `vm_run_bc` unwinds the staged operands.
    let step = dispatch(heap, callee, argv, true, env_id)?;
    // Success: drop the staged operands. The driver next truncates to `base` and rebuilds
    // the frame for the callee (reuse), so leaving them would be harmless — but truncating
    // keeps the root stack tight if the callee turned out native (`Done`).
    heap.truncate_roots(top);
    Ok(match step {
        Step::Tail {
            compiled,
            args,
            genv,
        } => ChunkExit::Tail {
            arm: compiled,
            args,
            genv,
        },
        Step::Done(v) => ChunkExit::Done(v),
    })
}

/// Tiering entry (ADR-101 1b): on an arm invocation whose frame is already set up at
/// `roots[base..]`, decide whether to run the JIT'd code. Counts the call; once the arm
/// crosses the hotness threshold it is handed to the [background compiler](JIT_COMPILER)
/// **once** (a `null → QUEUED` CAS elects the single thread that enqueues it) and runs on
/// the VM meanwhile. When the native pointer is later installed, subsequent calls run it.
/// Returns `Some(outcome)` if JIT'd code ran (`0` = Done with the result in `roots[base]`,
/// `1` = deopt — an operand wasn't an `Int`, `2` = preempt — the back-edge budget was
/// spent), or `None` to run the arm on the VM (not hot yet, compile in flight, or out of
/// the JIT's subset). **Never blocks on compilation** — that's the whole point.
///
/// **Hot-reload safety (the epoch guard).** A JIT'd arm inlines its arithmetic operators
/// as raw machine ops, so it must be invalidated if a `def` rebinds one. The arm carries
/// the [`global_epoch`](Heap::global_epoch) it was compiled at; a `def` bumps that epoch.
/// Before each native entry we compare the two — on a mismatch the arm is reset to
/// untried, so the next call re-validates its operators ([`chunk_ops_all_native`]) and
/// either recompiles (the rebind was of some *other* global) or bails (the operator
/// itself was redefined, so it stays on the VM forever, dispatching to the new
/// definition). The check is per *activation*, not per loop iteration: a JIT'd arm
/// evaluates no Brood, so no `def` can land mid-run, and the redefinition therefore takes
/// effect at the next arm entry — the standard safepoint granularity for a JIT.
#[cfg(feature = "jit")]
pub(crate) fn jit_tier(
    arm: &Arc<CompiledArm>,
    heap: &mut Heap,
    base: usize,
    env: EnvRoot,
) -> Option<i64> {
    use std::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed, Release};
    const THRESHOLD: u32 = 8;

    // Draining an over-deep native-recursion subtree on the VM (see [`JIT_FORCE_VM`]):
    // interpret this arm so its recursion stays in the bounded heap-frame loop.
    if heap.jit_force_vm {
        return None;
    }
    // Runtime JIT off-switch (BROOD_NO_JIT): never compile or run native — interpret
    // on the (correct) VM. Returns before the hotness count + the background-compile
    // enqueue CAS, so no arm is ever handed to the compiler and no native pointer is
    // installed, so the fast-link / dispatch paths have nothing to call either.
    if no_jit_enabled() {
        return None;
    }
    if no_jit_computed() {
        if let Some(c) = arm.chunk.as_ref() {
            if c.code.iter().any(|i| {
                matches!(
                    i,
                    Inst::Call {
                        tail: false,
                        head: None,
                        ..
                    }
                )
            }) {
                return None;
            }
        }
    }
    let mut code = arm.jit_code.load(Acquire);
    if code == crate::jit::BAILED || code == crate::jit::QUEUED {
        return None; // out of subset, or compile in flight — run the VM
    }
    // Shared-JIT install (the spawn lever): before this process spends THRESHOLD
    // interpreted calls + a background compile on its OWN copy of a RUNTIME/PRELUDE
    // arm, check whether another process of this runtime already compiled it. If so,
    // and the code is epoch-current, install the shared pointer directly and run it
    // now — so a hot shared function (`fib` under `spawn`) compiles to native ONCE,
    // not once per process. Stale entries (a `def`/compaction bumped the epoch) skip.
    if code.is_null() {
        if let Some(key) = arm.share_key {
            if let Some((ptr, epoch)) = heap.jit_shared_lookup(key) {
                if epoch == heap.global_epoch()
                    && !ptr.is_null()
                    && ptr != crate::jit::BAILED
                    && ptr != crate::jit::QUEUED
                {
                    arm.compile_epoch.store(epoch, Release);
                    arm.jit_code.store(ptr, Release);
                    arm.shared_published.store(true, Relaxed); // already in the cache
                    code = ptr;
                }
            }
        }
    }
    if code.is_null() {
        // Count the invocation; only enqueue once the arm is hot.
        if arm.jit_calls.fetch_add(1, Relaxed) + 1 < THRESHOLD {
            return None;
        }
        // Hot. Refuse to JIT an arm whose inlined operators are no longer native (a `def`
        // redefined one): mark it BAILED so it stays on the VM, where the operator's
        // epoch guard dispatches to the new definition. Otherwise record the epoch the
        // arm is being compiled at (the hot-reload guard, read on each native entry below)
        // and elect a single enqueuer via CAS (others see QUEUED and run the VM). A full
        // queue → back off: reset to untried so a later hot call re-attempts.
        if !chunk_ops_all_native(heap, arm) {
            arm.jit_code.store(crate::jit::BAILED, Release);
            return None;
        }
        arm.compile_epoch.store(heap.global_epoch(), Release);
        if arm
            .jit_code
            .compare_exchange(std::ptr::null_mut(), crate::jit::QUEUED, AcqRel, Acquire)
            .is_ok()
        {
            // Snapshot the live frame's slot tags (this is the elected enqueuer; the frame
            // at `roots[base..base+nslots]` holds the hot activation's params). Used to
            // type-specialize float arms; let-binder slots read nil here and get their type
            // from the body's writes during lowering. Sent with the arm — empty Vec is fine
            // (the lowerer treats absent/non-float profiles as integer-only).
            let slot_tags: Vec<u8> = (0..arm.nslots)
                .map(|i| crate::core::value::tag(heap.root_at(base + i)) as u8)
                .collect();
            if JIT_COMPILER
                .primary
                .try_send((arm.clone(), slot_tags))
                .is_err()
            {
                // The background compile queue is full (a burst of distinct hot arms — e.g.
                // thousands of short-lived green processes each tiering their own arm copy,
                // overwhelming the bounded channel). Reset to untried AND back the hotness
                // counter all the way off, so the arm runs on the VM for another THRESHOLD
                // calls before re-attempting — instead of re-validating (`chunk_ops_all_native`,
                // an `env_get`/`resolve_prim` per op) on *every* call while the queue stays
                // full. Measured: ~36M redundant re-validations in `spawn` (20 000 procs)
                // collapse to ~1/THRESHOLD of that. The arm still compiles once the queue
                // drains (a long-lived process re-reaches the threshold and re-enqueues).
                arm.jit_code.store(std::ptr::null_mut(), Release);
                arm.jit_calls.store(0, Relaxed);
            }
        }
        return None;
    }
    // A real, installed code pointer. Hot-reload guard: if the global epoch moved since
    // the arm was compiled, some `def` happened — invalidate the arm (reset to untried)
    // and run the VM this activation. The next call re-tiers, re-validating operators and
    // recompiling at the new epoch, or bailing if one was genuinely redefined.
    if arm.compile_epoch.load(Acquire) != heap.global_epoch() {
        arm.jit_code.store(std::ptr::null_mut(), Release);
        arm.jit_calls.store(THRESHOLD, Release); // re-tier promptly (already proven hot)
        arm.shared_published.store(false, Relaxed); // recompiled code must re-publish
        arm.inline_installed.store(false, Relaxed); // re-decide the inline swap at the new epoch
        arm.inline_queued.store(false, Relaxed); // re-enqueue the inlined upgrade if still hot
        return None;
    }
    // ---- Two-stage tiering (devlog 2026-06-17): the deferred inlined upgrade ----
    // The small original native is installed and running (the spawn-friendly fast path).
    // For an arm that qualifies for recursive self-inlining, the *inlined* body is compiled
    // separately on the lower-priority deferred queue and swapped in here once ready:
    //
    //  (1) Enqueue once. The first time we run the small native, hand the inlined compile to
    //      the DEFERRED queue (drained only when the primary initial-tier queue is empty).
    //      Under spawn's storm the primary queue never empties, so this never compiles until
    //      the storm clears — spawn finishes on the small native, no regression. A long-lived
    //      workload (fib 35) drains its primary and the inlined upgrade lands.
    //
    //  (2) Swap once. When `inline_code` holds a real installed pointer, atomically swap it
    //      into `jit_code`, bump the global epoch (so every fast-linked call site re-validates
    //      and picks up the inlined code WITH its larger `inline_nslots` frame — the per-engine
    //      sizing key), set `inline_installed`, and run the VM this one activation. The next
    //      entry sizes the frame to `active_nslots()` (= `inline_nslots`) and runs the inlined
    //      native. One VM activation on the transition — negligible.
    if arm.inline_name.is_some() && !arm.inline_installed.load(Acquire) {
        let ic = arm.inline_code.load(Acquire);
        if ic.is_null() {
            // Not yet compiled/enqueued. Elect a single enqueuer via the queued flag.
            if !arm.inline_queued.swap(true, AcqRel) {
                let slot_tags: Vec<u8> = (0..arm.nslots)
                    .map(|i| crate::core::value::tag(heap.root_at(base + i)) as u8)
                    .collect();
                // Deferred (low-priority). On a full queue, un-set `inline_queued` so a
                // later call re-attempts — but DON'T disturb the running small native.
                if JIT_COMPILER
                    .deferred
                    .try_send((arm.clone(), slot_tags))
                    .is_err()
                {
                    arm.inline_queued.store(false, Relaxed);
                }
            }
        } else if ic != crate::jit::BAILED && ic != crate::jit::QUEUED {
            // The inlined upgrade is ready — swap it in. Store `inline_installed` BEFORE
            // `jit_code` so that any reader which Acquire-loads `jit_code = inline_code` is
            // guaranteed (by the Release-Acquire chain) to also see `inline_installed = true`
            // and therefore call `active_nslots()` → `inline_nslots`. The reversed order
            // (jit_code before inline_installed) created a race: a reader could observe the
            // inline code pointer but still see `inline_installed = false`, sizing the callee
            // frame to the small `nslots` — the inline code would then raw-read beyond the
            // frame, picking up stale Vec-capacity data as slot values and passing garbage
            // through the outcome-4 tail-call staging path.
            //
            // This arm is PER-PROCESS ([`compiled_arm_for`] caches it in the process's own
            // `vm_cache`), so the upgrade must only re-point THIS process's fast-links to
            // this callee — NOT bump the shared `global_epoch`. A global bump invalidated
            // every peer process's `compile_epoch` too, so under `pfib` all 100 processes
            // cascaded: each peer nuked its installed code, re-tiered, re-upgraded and
            // re-bumped in turn, permanently diverting calls off the in-IR fast-link onto
            // the slow IC-dispatch path (~2× instructions; the parallel-scaling gap). We keep
            // `compile_epoch` at the current epoch (the arm's inlined operators were just
            // re-validated at compile time) and invalidate only this process's fast-links to
            // this callee, which then re-probe and pick up `inline_code` + `inline_nslots`.
            arm.inline_installed.store(true, Release); // BEFORE jit_code — see comment above
            arm.jit_code.store(ic, Release);
            if let Some(sym) = arm.inline_name {
                heap.invalidate_fast_links_for(sym);
            }
            // Run the VM this activation; the next entry sizes the frame to inline_nslots
            // (the call site reads `active_nslots()`) and runs the inlined native.
            return None;
        }
        // `ic == BAILED`: the inlined body fell out of subset — leave the small native
        // installed forever (it's correct + fast). No retry.
    }
    // Publish freshly-compiled native code to the shared cache so the runtime's other
    // processes install it directly instead of recompiling (the spawn lever). The
    // `swap` guard makes this one lock acquire per arm-instance, not one per call; a
    // process that installed the code *from* the cache already has the flag set.
    // NEVER publish an INLINED arm to the shared `(id, argc)` cache: a peer process that
    // installed it would run the inlined code with its OWN small `nslots` frame (it has its
    // own `CompiledArm` with `inline_installed == false`) → frame undersize / corruption.
    // The inlined upgrade is per-process by design; only the small native is shared (which
    // is the spawn-friendly path anyway). Guard on `inline_installed`.
    if !arm.inline_installed.load(Acquire) {
        if let Some(key) = arm.share_key {
            if !arm.shared_published.swap(true, Relaxed) {
                heap.jit_shared_publish(key, code, arm.compile_epoch.load(Acquire));
            }
        }
    }
    // SAFETY: `code` is a finalized `extern "C" fn(*mut Heap, base) -> i64` produced by
    // `jit_lower_arm`, living in the process-lifetime GLOBAL_JIT module. The frame is set
    // up at `roots[base..]`; the JIT'd arm keeps its own operands in registers (the call
    // staging grows `roots` only transiently, popped before return), so `heap` stays
    // valid for the call.
    let f: extern "C" fn(*mut Heap, i64) -> i64 = unsafe { std::mem::transmute(code) };
    // Publish this arm's env for the call/global callbacks, save/restoring the previous
    // value so a JIT'd callee that re-enters another JIT'd arm nests correctly.
    let saved_env = std::mem::replace(&mut heap.jit_call_env, env);
    // Best-effort arm name for the staged-stale diagnostic (recursive defns carry
    // `inline_name`; others reset to MAX so the value is never misleadingly stale).
    let saved_fn = std::mem::replace(&mut heap.jit_dbg_fn, arm.dbg_name.unwrap_or(u32::MAX));
    let outcome = f(heap as *mut Heap, base as i64);
    heap.jit_call_env = saved_env;
    heap.jit_dbg_fn = saved_fn;
    Some(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Bump a movable handle's index by `by`; leave atoms alone. Stands in for the
    // `runtime_collect` flush that relocates a handle into the compacted region.
    fn bump(v: Value, by: usize) -> Value {
        match v.unpack() {
            ValueRef::Str(id) => Value::str_(StrId::runtime(id.index() + by)),
            ValueRef::Pair(id) => Value::pair(PairId::runtime(id.index() + by)),
            _ => v,
        }
    }

    // `Value` has no `PartialEq` (Brood equality is a structural function), so compare
    // a handle const by kind + index.
    fn str_idx(v: Value) -> usize {
        match v.unpack() {
            ValueRef::Str(id) => id.index(),
            other => panic!("expected a Str, got {:?}", std::mem::discriminant(&other)),
        }
    }
    fn pair_idx(v: Value) -> usize {
        match v.unpack() {
            ValueRef::Pair(id) => id.index(),
            other => panic!("expected a Pair, got {:?}", std::mem::discriminant(&other)),
        }
    }

    /// Regression: a swapped `(op Const Local)` `Prim2SlotInt` must keep inlining after an
    /// epoch bump. The fusion stores an *inverted* arg-map (so the inline operand pick is
    /// correct); `prim2_inline_exec` revalidates against the head's *natural* map, so the
    /// `swapped` call site must un-invert it. Before the fix it compared the inverted map,
    /// which never matched `resolve_prim`'s natural map — so every such prim silently fell
    /// to the slow path forever after the first `def` bumped the epoch.
    #[test]
    fn swapped_prim2slotint_reinlines_after_epoch_bump() {
        let mut interp = crate::Interp::new();
        let heap = &mut interp.heap;
        let minus = value::intern("-"); // natural map [0,1]; `(- 24 x)` fuses to [1,0] swapped
                                        // A stale guard (≠ current epoch) forces the revalidation path the bug lived on.
        let guard = AtomicU64::new(heap.global_epoch().wrapping_add(1));
        // Operands as the caller picks them for map=[1,0]: x = const 24, y = local 5.
        let out = prim2_inline_exec(
            heap,
            PrimOp::Sub,
            [1, 0],
            true,
            minus,
            &guard,
            Value::int(24),
            Value::int(5),
        )
        .expect("no arithmetic error");
        match out {
            Some(v) => match v.unpack() {
                ValueRef::Int(n) => assert_eq!(n, 19, "(- 24 5) must inline to 19"),
                _ => panic!("expected Int(19), got tag {:?}", value::tag(v)),
            },
            None => panic!("swapped Prim2SlotInt slow-pathed after an epoch bump (the bug)"),
        }
        // The guard was refreshed to the live epoch, so subsequent calls take the fast path.
        assert_eq!(guard.load(Ordering::Relaxed), heap.global_epoch());
    }

    #[test]
    fn const_handle_round_trips() {
        // A heap-handle const decodes back to the same handle, and `rewrite` moves it.
        let cv = ConstVal::new(Value::str_(StrId::runtime(5)));
        assert!(
            matches!(cv, ConstVal::Handle { .. }),
            "a Str must encode as a Handle"
        );
        assert_eq!(str_idx(cv.load()), 5);
        cv.rewrite(&mut |v| bump(v, 100));
        assert_eq!(str_idx(cv.load()), 105, "rewrite must relocate the handle");

        // An atom stays inline and is never touched by a rewrite.
        let atom = ConstVal::new(Value::int(42));
        assert!(
            matches!(atom, ConstVal::Atom(_)),
            "an Int must encode as an Atom"
        );
        atom.rewrite(&mut |_| panic!("an atom const must not be passed to the flush"));
        assert!(matches!(atom.load().unpack(), ValueRef::Int(42)));
    }

    #[test]
    fn rewrite_arm_handles_rewrites_every_embedded_handle() {
        // The regression guard: `runtime_collect` calls this on each LIVE arm, so it
        // must reach every movable handle a node tree embeds — a `Const` literal, a
        // `MakeClosure` `fn_rest`, an `&optional` default — through all the structural
        // node variants, while leaving atoms/symbols/indices alone.
        let body = Node::Do(Box::new([
            Node::Const(ConstVal::new(Value::str_(StrId::runtime(1)))),
            Node::If(
                Box::new(Node::Const(ConstVal::new(Value::int(7)))), // atom — untouched
                Box::new(Node::Const(ConstVal::new(Value::pair(PairId::runtime(2))))),
                Box::new(Node::MakeClosure {
                    fn_rest: ConstVal::new(Value::pair(PairId::runtime(3))),
                    captures: Box::new([]),
                    self_name: None,
                }),
            ),
        ]));
        let arm = CompiledArm {
            nrequired: 0,
            noptional: 1,
            optional_defaults: Box::new([Some(Node::Const(ConstVal::new(Value::str_(
                StrId::runtime(4),
            ))))]),
            rest_slot: None,
            nslots: 0,
            body,
            chunk: None,
            has_runtime_handles: true,
            jit_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
            jit_calls: std::sync::atomic::AtomicU32::new(0),
            compile_epoch: std::sync::atomic::AtomicU64::new(0),
            share_key: None,
            shared_published: std::sync::atomic::AtomicBool::new(false),
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
        };

        rewrite_arm_handles(&arm, &mut |v| bump(v, 100));

        // Destructure the (known) tree and assert each handle moved, the atom didn't.
        let Node::Do(top) = &arm.body else {
            panic!("body")
        };
        assert_eq!(str_idx(load_const(&top[0])), 101);
        let Node::If(cond, then, els) = &top[1] else {
            panic!("if")
        };
        assert!(
            matches!(load_const(cond).unpack(), ValueRef::Int(7)),
            "atom const must be untouched"
        );
        assert_eq!(pair_idx(load_const(then)), 102);
        let Node::MakeClosure { fn_rest, .. } = &**els else {
            panic!("makeclosure")
        };
        assert_eq!(pair_idx(fn_rest.load()), 103);
        let Some(def) = &arm.optional_defaults[0] else {
            panic!("optional default")
        };
        assert_eq!(str_idx(load_const(def)), 104);
    }

    fn load_const(node: &Node) -> Value {
        match node {
            Node::Const(cv) => cv.load(),
            other => panic!("expected a Const, got {:?}", std::mem::discriminant(other)),
        }
    }

    // ===================== state-capture (ADR-100 §8) =====================

    thread_local! {
        /// Drives the suspend-once test native: 0 → suspend, ≥1 → return the value.
        static SUSPEND_GATE: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    }

    /// A stand-in for the `%receive` native: the **first** call raises a
    /// `Control::Suspend` (as a clean `receive` on an empty mailbox would); the
    /// **second** returns a value (as it would once a message arrived). Lets the
    /// capture→resume round-trip be tested in isolation
    /// from the mailbox/scheduler plumbing — the machinery under test is the driver's
    /// capture + replay, identical for any native that suspends mid-call.
    fn suspend_once_native(_args: &[Value], _env: EnvId, _heap: &mut Heap) -> LispResult {
        let n = SUSPEND_GATE.with(|c| {
            let v = c.get();
            c.set(v + 1);
            v
        });
        if n == 0 {
            Err(LispError::suspend(None))
        } else {
            Ok(Value::int(42))
        }
    }

    #[test]
    fn vm_run_bc_captures_and_resumes_a_suspend() {
        use crate::core::value::{Arity, NativeFn};
        use crate::types::Sig;

        SUSPEND_GATE.with(|c| c.set(0));
        let mut heap = Heap::new();

        // The suspend-once native, held in the arm's one frame slot (slot 0). A 0-arg
        // `Inst::Call` against it is the suspending point — the shape a `(receive …)`
        // lowers to (`%receive` is the callee, here `slot 0`).
        let native = heap.alloc_native(NativeFn {
            name: "%test-suspend-once".to_string(),
            arity: Arity::exact(0),
            func: suspend_once_native,
            params: &[],
            doc: "",
            sig: Sig::any(),
        });

        // Body `(slot0)`: push the native from slot 0, then a non-tail 0-ary call.
        let chunk = Chunk {
            code: vec![
                Inst::Local(0),
                Inst::Call {
                    argc: 0,
                    tail: false,
                    pos: None,
                    site: NO_SITE,
                    head: None,
                },
            ],
        };
        let arm = Arc::new(CompiledArm {
            nrequired: 1, // slot 0 = the callee, passed as the sole arg
            noptional: 0,
            optional_defaults: Box::new([]),
            rest_slot: None,
            nslots: 1,
            body: Node::Const(ConstVal::new(Value::nil())), // unused at runtime (chunk drives)
            chunk: Some(chunk),
            has_runtime_handles: false,
            jit_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
            jit_calls: std::sync::atomic::AtomicU32::new(0),
            compile_epoch: std::sync::atomic::AtomicU64::new(0),
            share_key: None,
            shared_published: std::sync::atomic::AtomicBool::new(false),
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
        });

        // First run: the native suspends, so the driver captures the continuation
        // WITHOUT unwinding (the operand stack — the pushed callee — survives on the
        // heap for the resume).
        let roots_before = heap.roots_len();
        let outcome = vm_run_bc(&mut heap, arm.clone(), &[native], EnvId::GLOBAL, None, true)
            .expect("first run errored");
        let suspended = match outcome {
            VmOutcome::Suspended(s) => s,
            _ => panic!("expected a captured suspend"),
        };
        assert!(
            heap.roots_len() > roots_before,
            "the captured continuation's frame slots + operands must stay rooted"
        );

        // Resume: replay from the rewound `%receive` call; the native now returns 42.
        let resumed = vm_run_bc(
            &mut heap,
            arm,
            &[native],
            EnvId::GLOBAL,
            Some(suspended),
            true,
        )
        .expect("resume errored");
        match resumed {
            VmOutcome::Done(v) => match v.unpack() {
                ValueRef::Int(n) => assert_eq!(n, 42, "resumed to the wrong value"),
                other => panic!("resumed to a non-int: {:?}", value::tag(other)),
            },
            other => panic!(
                "expected Done(42), got {}",
                match other {
                    VmOutcome::Suspended(_) => "Suspended (the gate didn't advance)",
                    VmOutcome::Preempted(_) => "Preempted",
                    VmOutcome::Killed => "Killed",
                    VmOutcome::Done(_) => unreachable!(),
                }
            ),
        }
        // The driver retired its only frame on `Done`, unwinding the operand stack
        // back to where the first run started.
        assert_eq!(
            heap.roots_len(),
            roots_before,
            "a completed resume must tear its frame stack back down to entry"
        );
    }

    /// JIT Stage-1 Step A: lower a straight-line int arm `(+ x 1)` to native code and
    /// run it against a real heap frame — read the arg from `roots[base]`, compute in
    /// registers, box the result back, and match the VM's answer.
    #[cfg(feature = "jit")]
    #[test]
    fn jit_lowers_and_runs_a_straight_line_int_arm() {
        let mut heap = Heap::new();
        // Body `(+ x 1)`: [Local(0), Const(1), Prim2 Add].
        let chunk = Chunk {
            code: vec![
                Inst::Local(0),
                Inst::Const(ConstVal::new(Value::int(1))),
                Inst::Prim2 {
                    op: PrimOp::Add,
                    map: [0, 1],
                    head: value::intern("+"),
                    guard: AtomicU64::new(0),
                    pos: None,
                },
            ],
        };
        let arm = CompiledArm {
            nrequired: 1,
            noptional: 0,
            optional_defaults: Box::new([]),
            rest_slot: None,
            nslots: 1,
            body: Node::Const(ConstVal::new(Value::nil())),
            chunk: Some(chunk),
            has_runtime_handles: false,
            jit_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
            jit_calls: std::sync::atomic::AtomicU32::new(0),
            compile_epoch: std::sync::atomic::AtomicU64::new(0),
            share_key: None,
            shared_published: std::sync::atomic::AtomicBool::new(false),
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
        };

        let mut jit = crate::jit::Jit::new();
        let ptr = jit_lower_arm(&mut jit, &arm, &[]).expect("straight-line int arm should JIT");
        let f: extern "C" fn(*mut Heap, i64) -> i64 = unsafe { std::mem::transmute(ptr) };

        // Frame: x = 41 at roots[base].
        let base = heap.roots_len();
        heap.push_root(Value::int(41));
        let outcome = f(&mut heap as *mut Heap, base as i64);
        assert_eq!(outcome, 0, "Done (no deopt — arg is an Int)");
        match heap.root_at(base).unpack() {
            ValueRef::Int(n) => assert_eq!(n, 42, "JIT-compiled (+ x 1) on x=41"),
            other => panic!("expected Int(42), got tag {:?}", value::tag(other)),
        }
    }

    /// JIT Stage-1 Step B: control flow + comparisons. Lower `(if (< x 0) (- 0 x) x)`
    /// (an `abs`) — JumpIfFalse/Jump → CFG blocks, `<` → an `icmp` branch, the two arms
    /// merging at a Done block param — and check both arms against the math.
    #[cfg(feature = "jit")]
    #[test]
    fn jit_lowers_and_runs_an_if_with_comparison() {
        let prim2 = |op: PrimOp, head: &str| Inst::Prim2 {
            op,
            map: [0, 1],
            head: value::intern(head),
            guard: AtomicU64::new(0),
            pos: None,
        };
        // (if (< x 0) (- 0 x) x), x = slot 0.
        let chunk = Chunk {
            code: vec![
                Inst::Local(0),                            // 0: x
                Inst::Const(ConstVal::new(Value::int(0))), // 1: 0
                prim2(PrimOp::Lt, "<"),                    // 2: x < 0
                Inst::JumpIfFalse(8),                      // 3: false → else (ip 8)
                Inst::Const(ConstVal::new(Value::int(0))), // 4: then: 0
                Inst::Local(0),                            // 5: x
                prim2(PrimOp::Sub, "-"),                   // 6: 0 - x
                Inst::Jump(9),                             // 7: → done (ip 9 = len)
                Inst::Local(0),                            // 8: else: x
            ],
        };
        let arm = CompiledArm {
            nrequired: 1,
            noptional: 0,
            optional_defaults: Box::new([]),
            rest_slot: None,
            nslots: 1,
            body: Node::Const(ConstVal::new(Value::nil())),
            chunk: Some(chunk),
            has_runtime_handles: false,
            jit_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
            jit_calls: std::sync::atomic::AtomicU32::new(0),
            compile_epoch: std::sync::atomic::AtomicU64::new(0),
            share_key: None,
            shared_published: std::sync::atomic::AtomicBool::new(false),
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
        };

        let mut jit = crate::jit::Jit::new();
        let ptr = jit_lower_arm(&mut jit, &arm, &[]).expect("if/cmp arm should JIT");
        let f: extern "C" fn(*mut Heap, i64) -> i64 = unsafe { std::mem::transmute(ptr) };

        for (x, want) in [(-5i64, 5i64), (3, 3), (0, 0)] {
            let mut heap = Heap::new();
            let base = heap.roots_len();
            heap.push_root(Value::int(x));
            assert_eq!(f(&mut heap as *mut Heap, base as i64), 0, "Done for x={x}");
            match heap.root_at(base).unpack() {
                ValueRef::Int(n) => assert_eq!(n, want, "abs({x})"),
                other => panic!(
                    "x={x}: expected Int({want}), got tag {:?}",
                    value::tag(other)
                ),
            }
        }
    }

    /// JIT Stage-1 Step C: the self-recursive **loop**. Lower
    /// `(if (< i 1) acc (sumto (- i 1) (+ acc i)))` — `SelfCall` boxes the new args into
    /// the frame slots and branches the loop header; the frame slots in `roots` carry the
    /// loop state. A native int loop, no per-iteration dispatch. (No `tick` yet — tested
    /// in isolation, not wired into the scheduler.)
    #[cfg(feature = "jit")]
    #[test]
    fn jit_lowers_and_runs_a_self_recursive_int_loop() {
        let prim2 = |op: PrimOp, head: &str| Inst::Prim2 {
            op,
            map: [0, 1],
            head: value::intern(head),
            guard: AtomicU64::new(0),
            pos: None,
        };
        // (defn sumto (i acc) (if (< i 1) acc (sumto (- i 1) (+ acc i)))) — i=slot0, acc=slot1.
        let chunk = Chunk {
            code: vec![
                Inst::Local(0),                            // 0: i
                Inst::Const(ConstVal::new(Value::int(1))), // 1: 1
                prim2(PrimOp::Lt, "<"),                    // 2: i < 1
                Inst::JumpIfFalse(6),                      // 3: false → else (ip 6)
                Inst::Local(1),                            // 4: then: acc
                Inst::Jump(13),                            // 5: → done (len)
                Inst::Local(0),                            // 6: else: i
                Inst::Const(ConstVal::new(Value::int(1))), // 7: 1
                prim2(PrimOp::Sub, "-"),                   // 8: (- i 1)  = arg0
                Inst::Local(1),                            // 9: acc
                Inst::Local(0),                            // 10: i
                prim2(PrimOp::Add, "+"),                   // 11: (+ acc i) = arg1
                Inst::SelfCall { argc: 2 },                // 12: (sumto arg0 arg1)
            ],
        };
        let arm = CompiledArm {
            nrequired: 2,
            noptional: 0,
            optional_defaults: Box::new([]),
            rest_slot: None,
            nslots: 2,
            body: Node::Const(ConstVal::new(Value::nil())),
            chunk: Some(chunk),
            has_runtime_handles: false,
            jit_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
            jit_calls: std::sync::atomic::AtomicU32::new(0),
            compile_epoch: std::sync::atomic::AtomicU64::new(0),
            share_key: None,
            shared_published: std::sync::atomic::AtomicBool::new(false),
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
        };

        let mut jit = crate::jit::Jit::new();
        let ptr = jit_lower_arm(&mut jit, &arm, &[]).expect("self-recursive int loop should JIT");
        let f: extern "C" fn(*mut Heap, i64) -> i64 = unsafe { std::mem::transmute(ptr) };

        // Prime the reduction budget so these short loops run to completion (the
        // back-edge `brood_rt_tick` would otherwise yield at REDUCTIONS == 0).
        crate::process::yield_now();
        // sumto(n,0) = n+(n-1)+…+1; sumto(1,0)→sumto(0,1)→1; sumto(0,0)→0.
        for (n, want) in [(5i64, 15i64), (100, 5050), (1, 1), (0, 0)] {
            let mut heap = Heap::new();
            let base = heap.roots_len();
            heap.push_root(Value::int(n)); // i = slot 0
            heap.push_root(Value::int(0)); // acc = slot 1
            assert_eq!(f(&mut heap as *mut Heap, base as i64), 0, "Done for n={n}");
            match heap.root_at(base).unpack() {
                ValueRef::Int(r) => assert_eq!(r, want, "sumto({n}, 0)"),
                other => panic!(
                    "n={n}: expected Int({want}), got tag {:?}",
                    value::tag(other)
                ),
            }
        }

        // Preemption: a loop longer than the reduction budget yields at a back-edge —
        // the JIT'd arm returns 2 (preempt), with the frame slots left mid-computation
        // in `roots` for the driver to resume on the VM. `brood_rt_tick` only preempts in
        // a capture-mode green process, so simulate one (set/clear `capture_run`).
        crate::process::set_capture_run(true);
        crate::process::yield_now(); // budget = REDUCTION_BUDGET
        let mut heap = Heap::new();
        let base = heap.roots_len();
        heap.push_root(Value::int(1_000_000)); // far more iterations than the budget
        heap.push_root(Value::int(0));
        let outcome = f(&mut heap as *mut Heap, base as i64);
        crate::process::set_capture_run(false); // restore (cargo test shares threads)
        assert_eq!(
            outcome, 2,
            "a loop exceeding the budget must preempt (return 2) in a green process"
        );
    }

    /// An arm *ending* in a **tail call with a staged (computed) callee**
    /// (`Inst::Call { tail: true, head: None }`) must lower (return `Some`), not bail —
    /// the jit-tier2 §6.2 payoff. The body is deliberately past the body-weight gate
    /// (4 work ops: `=`, `-`, `*`, `*`), since a thinner tail-call arm is gated out.
    /// We can't run it in isolation (outcome 4 needs the driver to dispatch the staged
    /// callee), so this asserts the *lowering* succeeds; `tests/jit.rs` proves the result.
    ///
    /// Also pins the deliberate counter-case: a **free-global** tail call
    /// (`head: Some`, the head elided from the operand stack) *bails*. The tail path
    /// (`jit_dispatch_tail`, outcome 4) reads a *staged* callee, which an elided head
    /// doesn't leave behind — so such arms (the common mutual-recursion shape) stay on
    /// the correct VM path rather than lower into a stale-callee read.
    #[cfg(feature = "jit")]
    #[test]
    fn jit_lowers_an_arm_ending_in_a_tail_call() {
        let prim2 = |op: PrimOp, head: &str| Inst::Prim2 {
            op,
            map: [0, 1],
            head: value::intern(head),
            guard: AtomicU64::new(0),
            pos: None,
        };
        // (defn fa (n acc) (if (= n 0) acc (fb (- n 1) (* (* acc acc) acc)))) — n=slot0, acc=slot1.
        let fb = value::intern("fb");
        let chunk = Chunk {
            code: vec![
                Inst::Local(0),                            // 0: n
                Inst::Const(ConstVal::new(Value::int(0))), // 1: 0
                prim2(PrimOp::Eq, "="),                    // 2: n == 0    (work 1)
                Inst::JumpIfFalse(6),                      // 3: false → else (ip 6)
                Inst::Local(1),                            // 4: then: acc
                Inst::Jump(16),                            // 5: → done (len)
                Inst::Global(fb),                          // 6: else: callee `fb`
                Inst::Local(0),                            // 7: n
                Inst::Const(ConstVal::new(Value::int(1))), // 8: 1
                prim2(PrimOp::Sub, "-"),                   // 9: (- n 1) = arg0   (work 2)
                Inst::Local(1),                            // 10: acc
                Inst::Local(1),                            // 11: acc
                prim2(PrimOp::Mul, "*"),                   // 12: (* acc acc)     (work 3)
                Inst::Local(1),                            // 13: acc
                prim2(PrimOp::Mul, "*"),                   // 14: (* … acc) = arg1 (work 4)
                Inst::Call {
                    argc: 2,
                    tail: true,
                    pos: None,
                    site: NO_SITE,
                    // Computed callee: `fb` is staged on the operand stack (the `Global(fb)`
                    // at ip 6 above), so `head` is `None`. This is the shape that lowers — the
                    // staged callee is exactly what `jit_dispatch_tail` reads back.
                    head: None,
                }, // 15
            ],
        };
        let arm = CompiledArm {
            nrequired: 2,
            noptional: 0,
            optional_defaults: Box::new([]),
            rest_slot: None,
            nslots: 2,
            body: Node::Const(ConstVal::new(Value::nil())),
            chunk: Some(chunk),
            has_runtime_handles: false,
            jit_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
            jit_calls: std::sync::atomic::AtomicU32::new(0),
            compile_epoch: std::sync::atomic::AtomicU64::new(0),
            share_key: None,
            shared_published: std::sync::atomic::AtomicBool::new(false),
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
        };
        let mut jit = crate::jit::Jit::new();
        assert!(
            jit_lower_arm(&mut jit, &arm, &[]).is_some(),
            "an arm ending in a computed-callee tail call (past the body-weight gate) must lower"
        );

        // The *same* 4-work-op arm whose tail call is a **free-global** head (`head:
        // Some(fb)`, elided shape) now lowers successfully: the JIT stages the callee
        // via `globic_ref` before the args (the free-global tail call fix, c99f539).
        let elided = Chunk {
            code: vec![
                Inst::Local(0),                            // 0: n
                Inst::Const(ConstVal::new(Value::int(0))), // 1: 0
                prim2(PrimOp::Eq, "="),                    // 2: n == 0    (work 1)
                Inst::JumpIfFalse(6),                      // 3: false → else (ip 6)
                Inst::Local(1),                            // 4: then: acc
                Inst::Jump(15),                            // 5: → done (len)
                Inst::Local(0),                            // 6: else: n (no staged callee — elided)
                Inst::Const(ConstVal::new(Value::int(1))), // 7: 1
                prim2(PrimOp::Sub, "-"),                   // 8: (- n 1) = arg0   (work 2)
                Inst::Local(1),                            // 9: acc
                Inst::Local(1),                            // 10: acc
                prim2(PrimOp::Mul, "*"),                   // 11: (* acc acc)     (work 3)
                Inst::Local(1),                            // 12: acc
                prim2(PrimOp::Mul, "*"),                   // 13: (* … acc) = arg1 (work 4)
                Inst::Call {
                    argc: 2,
                    tail: true,
                    pos: None,
                    site: NO_SITE,
                    head: Some(fb), // free-global head, elided from the stack
                }, // 14
            ],
        };
        let elided_arm = CompiledArm {
            nrequired: 2,
            noptional: 0,
            optional_defaults: Box::new([]),
            rest_slot: None,
            nslots: 2,
            body: Node::Const(ConstVal::new(Value::nil())),
            chunk: Some(elided),
            has_runtime_handles: false,
            jit_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
            jit_calls: std::sync::atomic::AtomicU32::new(0),
            compile_epoch: std::sync::atomic::AtomicU64::new(0),
            share_key: None,
            shared_published: std::sync::atomic::AtomicBool::new(false),
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
        };
        assert!(
            jit_lower_arm(&mut jit, &elided_arm, &[]).is_some(),
            "an elided free-global tail call must lower (callee staged via globic_ref, c99f539)"
        );

        // ...and a *thin* tail-call arm (2 work ops: `=`, `-`) is gated out — stays on the
        // VM, where the per-hop round-trip would otherwise cost more than it saves.
        let thin = Chunk {
            code: vec![
                Inst::Local(0),
                Inst::Const(ConstVal::new(Value::int(0))),
                prim2(PrimOp::Eq, "="),
                Inst::JumpIfFalse(6),
                Inst::Local(1),
                Inst::Jump(10),
                Inst::Global(fb),
                Inst::Local(0),
                Inst::Const(ConstVal::new(Value::int(1))),
                prim2(PrimOp::Sub, "-"),
                Inst::Call {
                    argc: 1,
                    tail: true,
                    pos: None,
                    site: NO_SITE,
                    head: Some(fb),
                },
            ],
        };
        let thin_arm = CompiledArm {
            nrequired: 2,
            noptional: 0,
            optional_defaults: Box::new([]),
            rest_slot: None,
            nslots: 2,
            body: Node::Const(ConstVal::new(Value::nil())),
            chunk: Some(thin),
            has_runtime_handles: false,
            jit_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
            jit_calls: std::sync::atomic::AtomicU32::new(0),
            compile_epoch: std::sync::atomic::AtomicU64::new(0),
            share_key: None,
            shared_published: std::sync::atomic::AtomicBool::new(false),
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
        };
        assert!(
            jit_lower_arm(&mut jit, &thin_arm, &[]).is_none(),
            "a thin tail-call arm (2 work ops) must be gated out (stays on the VM)"
        );
    }

    /// JIT Stage-1.5: the **fused** `Prim2Slot*` variants — which `emit_node` actually
    /// produces for real loop bodies (`(- i 1)`, `(+ acc i)`, `(< i 1)`) — lower and run.
    /// Before this, the JIT bailed on every fused prim, so it never fired on real
    /// compiled code. Also pins the two correctness fixes that came with the coverage:
    /// `map` (the `>`/swapped-operand case) and overflow → deopt (so the JIT matches the
    /// VM's BigInt promotion instead of silently wrapping).
    #[cfg(feature = "jit")]
    #[test]
    fn jit_lowers_fused_prims_map_and_overflow() {
        // All uses here are the `(op Local Const)` form, so `swapped: false`.
        let slot_int =
            |op: PrimOp, map: [u8; 2], slot_a: usize, int_b: i64, head: &str| Inst::Prim2SlotInt {
                op,
                map,
                slot_a,
                int_b,
                swapped: false,
                head: value::intern(head),
                guard: AtomicU64::new(0),
                pos: None,
            };
        let slot_slot =
            |op: PrimOp, slot_a: usize, slot_b: usize, head: &str| Inst::Prim2SlotSlot {
                op,
                map: [0, 1],
                slot_a,
                slot_b,
                head: value::intern(head),
                guard: AtomicU64::new(0),
                pos: None,
            };
        let mk_arm = |chunk: Chunk, nreq: usize, nslots: usize| CompiledArm {
            nrequired: nreq,
            noptional: 0,
            optional_defaults: Box::new([]),
            rest_slot: None,
            nslots,
            body: Node::Const(ConstVal::new(Value::nil())),
            chunk: Some(chunk),
            has_runtime_handles: false,
            jit_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
            jit_calls: std::sync::atomic::AtomicU32::new(0),
            compile_epoch: std::sync::atomic::AtomicU64::new(0),
            share_key: None,
            shared_published: std::sync::atomic::AtomicBool::new(false),
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
        };
        let mut jit = crate::jit::Jit::new();

        // (a) sumto with the REAL fused shape: `(< i 1)`/`(- i 1)` → Prim2SlotInt,
        // `(+ acc i)` → Prim2SlotSlot. i = slot0, acc = slot1.
        let sumto = mk_arm(
            Chunk {
                code: vec![
                    slot_int(PrimOp::Lt, [0, 1], 0, 1, "<"),  // 0: (< i 1)
                    Inst::JumpIfFalse(4),                     // 1: false → else
                    Inst::Local(1),                           // 2: then: acc
                    Inst::Jump(7),                            // 3: → done
                    slot_int(PrimOp::Sub, [0, 1], 0, 1, "-"), // 4: (- i 1) = arg0
                    slot_slot(PrimOp::Add, 1, 0, "+"),        // 5: (+ acc i) = arg1
                    Inst::SelfCall { argc: 2 },               // 6: (sumto arg0 arg1)
                ],
            },
            2,
            2,
        );
        let f: extern "C" fn(*mut Heap, i64) -> i64 = unsafe {
            std::mem::transmute(jit_lower_arm(&mut jit, &sumto, &[]).expect("fused sumto JITs"))
        };
        crate::process::yield_now(); // prime the reduction budget so the loop completes
        for (n, want) in [(5i64, 15i64), (100, 5050), (1, 1), (0, 0)] {
            let mut heap = Heap::new();
            let base = heap.roots_len();
            heap.push_root(Value::int(n));
            heap.push_root(Value::int(0));
            assert_eq!(
                f(&mut heap as *mut Heap, base as i64),
                0,
                "Done for sumto({n})"
            );
            match heap.root_at(base).unpack() {
                ValueRef::Int(r) => assert_eq!(r, want, "fused sumto({n}, 0)"),
                other => panic!("expected Int, got tag {:?}", value::tag(other)),
            }
        }

        // (b) `map` — `>` lowers to `%lt` with `map = [1, 0]` (operands swapped). The JIT
        // must apply it: `(if (> x 5) 100 200)` is 100 for x=10 and 200 for x=3. Ignoring
        // `map` would compute `x < 5` and flip both answers.
        let gt = mk_arm(
            Chunk {
                code: vec![
                    slot_int(PrimOp::Lt, [1, 0], 0, 5, ">"), // 0: (> x 5)  [swapped]
                    Inst::JumpIfFalse(4),                    // 1
                    Inst::Const(ConstVal::new(Value::int(100))), // 2: then
                    Inst::Jump(5),                           // 3
                    Inst::Const(ConstVal::new(Value::int(200))), // 4: else
                ],
            },
            1,
            1,
        );
        let g: extern "C" fn(*mut Heap, i64) -> i64 = unsafe {
            std::mem::transmute(jit_lower_arm(&mut jit, &gt, &[]).expect("(> x 5) JITs"))
        };
        for (x, want) in [(10i64, 100i64), (3, 200)] {
            let mut heap = Heap::new();
            let base = heap.roots_len();
            heap.push_root(Value::int(x));
            assert_eq!(
                g(&mut heap as *mut Heap, base as i64),
                0,
                "Done for (> {x} 5)"
            );
            match heap.root_at(base).unpack() {
                ValueRef::Int(r) => {
                    assert_eq!(r, want, "(if (> {x} 5) 100 200) — map must be applied")
                }
                other => panic!("expected Int, got tag {:?}", value::tag(other)),
            }
        }

        // (c) overflow → deopt. `(* x x)` for a huge x overflows i64; the VM defers such
        // an op to the native, which promotes to a BigInt, so the JIT must deopt (return
        // 1) rather than store a wrapped i64. A non-overflowing x runs to Done (0).
        let sq = mk_arm(
            Chunk {
                code: vec![slot_slot(PrimOp::Mul, 0, 0, "*")],
            },
            1,
            1,
        );
        let s: extern "C" fn(*mut Heap, i64) -> i64 = unsafe {
            std::mem::transmute(jit_lower_arm(&mut jit, &sq, &[]).expect("(* x x) JITs"))
        };
        let mut heap = Heap::new();
        let base = heap.roots_len();
        heap.push_root(Value::int(3));
        assert_eq!(
            s(&mut heap as *mut Heap, base as i64),
            0,
            "(* 3 3) is in range"
        );
        assert!(
            matches!(heap.root_at(base).unpack(), ValueRef::Int(9)),
            "(* 3 3) = 9"
        );
        let mut heap = Heap::new();
        let base = heap.roots_len();
        heap.push_root(Value::int(4_000_000_000)); // 4e9 * 4e9 = 1.6e19 > i64::MAX
        assert_eq!(
            s(&mut heap as *mut Heap, base as i64),
            1,
            "an overflowing (* x x) must deopt to the VM (BigInt), not wrap"
        );
    }

    /// JIT Stage-1 1b: tiering. An arm invoked past the hotness threshold is compiled
    /// once and thereafter runs as native code (`jit_tier` returns `Some(0)` with the
    /// result in `roots[base]`); below the threshold it returns `None` (run on the VM).
    /// An arm out of the JIT subset is marked BAILED and always returns `None`.
    #[cfg(feature = "jit")]
    #[test]
    fn jit_tier_compiles_a_hot_arm_then_runs_native() {
        let prim2 = |op: PrimOp, head: &str| Inst::Prim2 {
            op,
            map: [0, 1],
            head: value::intern(head),
            guard: AtomicU64::new(0),
            pos: None,
        };
        // sumto(i acc) = (if (< i 1) acc (sumto (- i 1) (+ acc i))).
        let mk_arm = |chunk: Chunk, nreq: usize, nslots: usize| CompiledArm {
            nrequired: nreq,
            noptional: 0,
            optional_defaults: Box::new([]),
            rest_slot: None,
            nslots,
            body: Node::Const(ConstVal::new(Value::nil())),
            chunk: Some(chunk),
            has_runtime_handles: false,
            jit_code: AtomicPtr::new(std::ptr::null_mut()),
            jit_calls: AtomicU32::new(0),
            compile_epoch: AtomicU64::new(0),
            share_key: None,
            shared_published: std::sync::atomic::AtomicBool::new(false),
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
        };
        let sumto = Arc::new(mk_arm(
            Chunk {
                code: vec![
                    Inst::Local(0),
                    Inst::Const(ConstVal::new(Value::int(1))),
                    prim2(PrimOp::Lt, "<"),
                    Inst::JumpIfFalse(6),
                    Inst::Local(1),
                    Inst::Jump(13),
                    Inst::Local(0),
                    Inst::Const(ConstVal::new(Value::int(1))),
                    prim2(PrimOp::Sub, "-"),
                    Inst::Local(1),
                    Inst::Local(0),
                    prim2(PrimOp::Add, "+"),
                    Inst::SelfCall { argc: 2 },
                ],
            },
            2,
            2,
        ));

        // A prelude-loaded heap, so `jit_tier`'s operator-validation (`+`/`-`/`<` must
        // still resolve to their natives — the hot-reload guard) sees the live globals; a
        // bare `Heap::new()` has no global env. One heap, reused across poll iterations
        // (truncate the frame each time), keeps the epoch stable so the arm stays tiered.
        let mut interp = crate::Interp::new();
        // Compilation is async now (the background compiler thread), so a hot arm returns
        // None until the native pointer is installed. Poll past the threshold, giving the
        // compiler time to land the code, and assert it eventually runs native.
        crate::process::yield_now(); // prime the reduction budget (short loops)
        let mut ran_native = 0;
        for _ in 0..400 {
            crate::process::yield_now(); // keep the budget topped up across calls
            let base = interp.heap.roots_len();
            interp.heap.push_root(Value::int(5)); // i
            interp.heap.push_root(Value::int(0)); // acc
            let outcome = jit_tier(
                &sumto,
                &mut interp.heap,
                base,
                EnvRoot::Stable(EnvId::GLOBAL),
            );
            match outcome {
                None => {
                    interp.heap.truncate_roots(base);
                    std::thread::sleep(std::time::Duration::from_millis(2)); // not hot / compile in flight
                }
                Some(0) => {
                    ran_native += 1;
                    match interp.heap.root_at(base).unpack() {
                        ValueRef::Int(r) => assert_eq!(r, 15, "JIT'd sumto(5,0)"),
                        other => panic!("expected Int(15), got tag {:?}", value::tag(other)),
                    }
                    interp.heap.truncate_roots(base);
                    if ran_native >= 3 {
                        break;
                    }
                }
                Some(o) => panic!("unexpected JIT outcome {o}"),
            }
        }
        assert!(ran_native > 0, "the hot arm should tier up to native code");

        // An out-of-subset arm is marked BAILED and never runs native. `MakeMap` has no
        // JIT lowering path (there's no map-build codegen), so a map-building arm is
        // always out of subset. (Scalar `Const`s — `Int`/`Nil`/`Float`/`Bool` — and a
        // bare `Global` now *are* in subset, so neither is the bail example any more.)
        let bailing = Arc::new(mk_arm(
            Chunk {
                code: vec![Inst::MakeMap(0)],
            },
            0,
            1,
        ));
        for _ in 0..400 {
            let base = interp.heap.roots_len();
            interp.heap.push_root(Value::int(0));
            assert_eq!(
                jit_tier(
                    &bailing,
                    &mut interp.heap,
                    base,
                    EnvRoot::Stable(EnvId::GLOBAL)
                ),
                None,
                "out-of-subset arm bails"
            );
            interp.heap.truncate_roots(base);
            if bailing.jit_code.load(std::sync::atomic::Ordering::Acquire) == crate::jit::BAILED {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(
            bailing.jit_code.load(std::sync::atomic::Ordering::Acquire),
            crate::jit::BAILED,
            "out-of-subset arm must settle to BAILED"
        );
    }

    /// JIT Stage-1 end-to-end: `vm_run_bc`'s hot-path hook runs a tiered arm as native
    /// code and returns the same result the interpreter would. Warm the arm past the
    /// threshold so it compiles, then invoke it through `vm_run_bc` (fresh start) and
    /// check the `Done` value.
    #[cfg(feature = "jit")]
    #[test]
    fn vm_run_bc_runs_a_tiered_arm_via_the_hook() {
        let prim2 = |op: PrimOp, head: &str| Inst::Prim2 {
            op,
            map: [0, 1],
            head: value::intern(head),
            guard: AtomicU64::new(0),
            pos: None,
        };
        let chunk = Chunk {
            code: vec![
                Inst::Local(0),
                Inst::Const(ConstVal::new(Value::int(1))),
                prim2(PrimOp::Lt, "<"),
                Inst::JumpIfFalse(6),
                Inst::Local(1),
                Inst::Jump(13),
                Inst::Local(0),
                Inst::Const(ConstVal::new(Value::int(1))),
                prim2(PrimOp::Sub, "-"),
                Inst::Local(1),
                Inst::Local(0),
                prim2(PrimOp::Add, "+"),
                Inst::SelfCall { argc: 2 },
            ],
        };
        let arm = Arc::new(CompiledArm {
            nrequired: 2,
            noptional: 0,
            optional_defaults: Box::new([]),
            rest_slot: None,
            nslots: 2,
            body: Node::Const(ConstVal::new(Value::nil())),
            chunk: Some(chunk),
            has_runtime_handles: false,
            jit_code: AtomicPtr::new(std::ptr::null_mut()),
            jit_calls: AtomicU32::new(0),
            compile_epoch: AtomicU64::new(0),
            share_key: None,
            shared_published: std::sync::atomic::AtomicBool::new(false),
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
        });

        // Warm it past the threshold so jit_tier hands it to the background compiler;
        // poll until the native pointer is installed (compilation is async now). A
        // prelude-loaded heap, so the operator-validation in `jit_tier` resolves `+`/`-`/`<`.
        use std::sync::atomic::Ordering::Acquire;
        let mut interp = crate::Interp::new();
        crate::process::yield_now();
        let mut tiered = false;
        for _ in 0..400 {
            crate::process::yield_now();
            let base = interp.heap.roots_len();
            interp.heap.push_root(Value::int(5));
            interp.heap.push_root(Value::int(0));
            let _ = jit_tier(&arm, &mut interp.heap, base, EnvRoot::Stable(EnvId::GLOBAL));
            interp.heap.truncate_roots(base);
            let code = arm.jit_code.load(Acquire);
            if !code.is_null() && code != crate::jit::BAILED && code != crate::jit::QUEUED {
                tiered = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(tiered, "the arm should have tiered up to native code");

        // Now run it through vm_run_bc — its fresh-start hook should call the native code.
        crate::process::yield_now();
        let outcome = vm_run_bc(
            &mut interp.heap,
            arm,
            &[Value::int(5), Value::int(0)],
            EnvId::GLOBAL,
            None,
            true,
        )
        .expect("vm_run_bc errored");
        match outcome {
            VmOutcome::Done(v) => match v.unpack() {
                ValueRef::Int(n) => assert_eq!(n, 15, "tiered sumto(5,0) via the hook"),
                other => panic!("Done non-int: tag {:?}", value::tag(other)),
            },
            _ => panic!("expected Done(15) from the JIT hook"),
        }
    }

    /// JIT Stage-1.5: the actual speedup — JIT'd `sumto(N,0)` vs the interpreter, same
    /// arm, run through `vm_run_bc`. The VM baseline forces BAILED so its hook stays on
    /// the interpreter; the JIT arm is warmed so the hook runs native. Benchmark, not a
    /// pass/fail test — run with `--ignored --nocapture`.
    #[cfg(feature = "jit")]
    #[test]
    #[ignore = "benchmark — cargo test -p brood --features jit --lib jit_speedup -- --ignored --nocapture"]
    fn jit_speedup_vs_vm() {
        use std::time::Instant;
        let prim2 = |op: PrimOp, head: &str| Inst::Prim2 {
            op,
            map: [0, 1],
            head: value::intern(head),
            guard: AtomicU64::new(0),
            pos: None,
        };
        let mk = || CompiledArm {
            nrequired: 2,
            noptional: 0,
            optional_defaults: Box::new([]),
            rest_slot: None,
            nslots: 2,
            body: Node::Const(ConstVal::new(Value::nil())),
            chunk: Some(Chunk {
                code: vec![
                    Inst::Local(0),
                    Inst::Const(ConstVal::new(Value::int(1))),
                    prim2(PrimOp::Lt, "<"),
                    Inst::JumpIfFalse(6),
                    Inst::Local(1),
                    Inst::Jump(13),
                    Inst::Local(0),
                    Inst::Const(ConstVal::new(Value::int(1))),
                    prim2(PrimOp::Sub, "-"),
                    Inst::Local(1),
                    Inst::Local(0),
                    prim2(PrimOp::Add, "+"),
                    Inst::SelfCall { argc: 2 },
                ],
            }),
            has_runtime_handles: false,
            jit_code: AtomicPtr::new(std::ptr::null_mut()),
            jit_calls: AtomicU32::new(0),
            compile_epoch: AtomicU64::new(0),
            share_key: None,
            shared_published: std::sync::atomic::AtomicBool::new(false),
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
        };
        let n = 100_000i64; // iterations per sumto call
        let reps = 300;
        // A prelude-loaded heap, reused across reps (vm_run_bc unwinds to entry on Done, so
        // roots stay clean): needed so the JIT tiering hook's operator-validation resolves
        // `+`/`-`/`<`, and so the per-rep cost is the loop, not a prelude load.
        let mut interp = crate::Interp::new();
        let run = |h: &mut Heap, arm: &Arc<CompiledArm>| -> i64 {
            match vm_run_bc(
                h,
                arm.clone(),
                &[Value::int(n), Value::int(0)],
                EnvId::GLOBAL,
                None,
                true,
            )
            .expect("run")
            {
                VmOutcome::Done(v) => match v.unpack() {
                    ValueRef::Int(r) => r,
                    _ => panic!("bad outcome"),
                },
                _ => panic!("bad outcome"),
            }
        };

        // VM baseline: BAILED forces the hook to stay on the interpreter.
        let vm_arm = Arc::new(mk());
        vm_arm
            .jit_code
            .store(crate::jit::BAILED, std::sync::atomic::Ordering::Release);
        let r0 = run(&mut interp.heap, &vm_arm); // warm caches / verify
        let t = Instant::now();
        for _ in 0..reps {
            assert_eq!(run(&mut interp.heap, &vm_arm), r0);
        }
        let vm = t.elapsed();

        // JIT: warm the arm so the background compiler installs native code, then the
        // hook runs it. Poll until tiered (compilation is async).
        use std::sync::atomic::Ordering::Acquire;
        let jit_arm = Arc::new(mk());
        crate::process::yield_now();
        for _ in 0..1000 {
            let b = interp.heap.roots_len();
            interp.heap.push_root(Value::int(5));
            interp.heap.push_root(Value::int(0));
            let _ = jit_tier(
                &jit_arm,
                &mut interp.heap,
                b,
                EnvRoot::Stable(EnvId::GLOBAL),
            );
            interp.heap.truncate_roots(b);
            let c = jit_arm.jit_code.load(Acquire);
            if !c.is_null() && c != crate::jit::BAILED && c != crate::jit::QUEUED {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let r1 = run(&mut interp.heap, &jit_arm);
        assert_eq!(r1, r0, "JIT must match the VM");
        let t = Instant::now();
        for _ in 0..reps {
            assert_eq!(run(&mut interp.heap, &jit_arm), r1);
        }
        let jit = t.elapsed();

        eprintln!(
            "sumto({n},0) x{reps}: VM {vm:?}  JIT {jit:?}  speedup {:.1}x",
            vm.as_secs_f64() / jit.as_secs_f64().max(1e-9)
        );
    }
}

#[test]
fn test_inst_size() {
    // Not an assertion — just surfaces the IR `Inst` size in test output (a
    // regression in it shows up here). A non-zero size is guaranteed by the type.
    eprintln!("Inst size: {}", std::mem::size_of::<Inst>());
}
