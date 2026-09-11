//! The compiler front-end: an expanded form to a `Node` tree. `compile_node` is the
//! dispatch; the `compile_*` helpers each lower one special form against a `Scope`
//! (the lexical slot map), and `resolve_prim*` recognise the primitive calls the VM
//! executes inline.

use super::*;

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
pub(crate) struct Scope {
    pub(crate) names: Vec<(Symbol, usize)>,
    pub(crate) next: usize,
    pub(crate) max: usize,
    pub(crate) enclosing: Vec<Symbol>,
    pub(crate) unsafe_slots: Vec<usize>,
    /// While compiling a `letrec` binder whose RHS is *directly* a `(fn …)`, the
    /// slot of that binder — so a nested closure capturing it recognises the
    /// **direct self-recursion** case and binds its own name to itself at build
    /// time (see [`compile_captures`]) rather than deferring. `None` everywhere
    /// else, so an ordinary capture of an in-progress letrec binder (mutual
    /// recursion) still defers.
    pub(crate) letrec_self: Option<usize>,
    /// `(self-name, arity)` when this arm is a plain fixed-arity local recursive
    /// helper (a `letrec` binder bound to itself — see [`compile_closure`]). A
    /// **tail** call to `self-name` with exactly `arity` args compiles to a
    /// [`Node::SelfCall`] that re-invokes the current arm directly, skipping the
    /// env-resolve + dispatch the generic call path pays per iteration. `None`
    /// for an ordinary closure (and unset while compiling a nested `(fn …)`, which
    /// gets its own scope).
    pub(crate) self_call: Option<(Symbol, usize)>,
    /// Per-arm call-site IC counter (ADR-175 Phase A): sites number from 0 within the
    /// arm being compiled, so the compiled arm is position-independent — each process
    /// resolves its own IC block for the arm and indexes `base + site`. Replaces the
    /// per-process `Heap::vm_site_alloc` absolute numbering, under which a shared
    /// arm's sites only made sense in the process that compiled it.
    pub(crate) sites: u32,
    /// Per-arm global-read IC counter — the `Node::GlobalIc` counterpart of `sites`.
    pub(crate) gsites: u32,
    /// Source positions per arm-relative call site (indexed by site id). Moved into
    /// the [`CompiledArm`] at construction; see `CompiledArm::site_pos`. Unconditional
    /// (not debug-gated) so every construction site stays cfg-free; the cost is a few
    /// hundred bytes per compiled arm, shared per-runtime once arms are shared.
    pub(crate) site_pos: Vec<Option<(crate::error::Pos, Option<std::sync::Arc<str>>)>>,
}

impl Scope {
    pub(crate) fn new() -> Self {
        Scope {
            names: Vec::new(),
            next: 0,
            max: 0,
            enclosing: Vec::new(),
            unsafe_slots: Vec::new(),
            letrec_self: None,
            self_call: None,
            sites: 0,
            gsites: 0,
            site_pos: Vec::new(),
        }
    }
    /// Allocate the next arm-relative call-site id (see the `sites` field).
    pub(crate) fn site_alloc(&mut self) -> u32 {
        let s = self.sites;
        self.sites += 1;
        s
    }
    /// Allocate the next arm-relative global-read site id.
    pub(crate) fn gsite_alloc(&mut self) -> u32 {
        let s = self.gsites;
        self.gsites += 1;
        s
    }
    pub(crate) fn with_params(params: &[Symbol]) -> Self {
        let mut s = Scope::new();
        for &p in params {
            s.bind(p);
        }
        s
    }
    /// As [`with_params`](Self::with_params) but seeded with the enclosing lexical
    /// names a nested closure closes over (Stage 2c).
    pub(crate) fn with_params_enclosing(params: &[Symbol], enclosing: Vec<Symbol>) -> Self {
        let mut s = Scope::with_params(params);
        s.enclosing = enclosing;
        s
    }
    pub(crate) fn lookup(&self, sym: Symbol) -> Option<usize> {
        self.names
            .iter()
            .rev()
            .find(|(n, _)| *n == sym)
            .map(|&(_, slot)| slot)
    }
    pub(crate) fn bind(&mut self, sym: Symbol) -> usize {
        let slot = self.next;
        self.next += 1;
        if self.next > self.max {
            self.max = self.next;
        }
        self.names.push((sym, slot));
        slot
    }
    pub(crate) fn is_unsafe(&self, slot: usize) -> bool {
        self.unsafe_slots.contains(&slot)
    }
    /// Snapshot for scope exit: `(names-len, next-slot)`.
    pub(crate) fn mark(&self) -> (usize, usize) {
        (self.names.len(), self.next)
    }
    pub(crate) fn restore(&mut self, (names_len, next): (usize, usize)) {
        self.names.truncate(names_len);
        self.next = next;
    }
}

/// Extract a binding form's elements (`[n1, v1, n2, v2, …]`) from either a list
/// `(n1 v1 …)` or a vector `[n1 v1 …]` (both accepted in Brood binding position),
/// or `None` if it isn't one.
pub(crate) fn binding_elems(heap: &Heap, form: Value) -> Option<Vec<Value>> {
    match form.unpack() {
        ValueRef::Nil => Some(Vec::new()),
        ValueRef::Vector(vid) => Some(heap.vector(vid).to_vec()),
        ValueRef::Pair(_) => heap.list_to_vec(form).ok(),
        _ => None,
    }
}

/// Compile a body (a `do`-like sequence): all but the last for effect, the last
/// in `tail` position. Empty → `nil`. A single form returns that node directly.
pub(crate) fn compile_body(
    heap: &Heap,
    forms: &[Value],
    scope: &mut Scope,
    tail: bool,
) -> Option<Node> {
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
pub(crate) fn compile_let(
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
            for pair in elems.as_chunks::<2>().0 {
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
            for (pair, &slot) in elems.as_chunks::<2>().0.iter().zip(slots.iter()) {
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
            for pair in elems.as_chunks::<2>().0 {
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
pub(crate) fn fn_rest_is_stable(v: Value) -> bool {
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
/// The truthiness of a node whose value is known at compile time, or `None` when it
/// isn't a constant. Only `nil` and `false` are falsy (`eval::truthy`), and both are
/// [`ConstVal::Atom`]s — every `ConstVal::Handle` is an allocated heap object (string,
/// bignum, pair, vector, map, …), so a handle constant is unconditionally truthy.
pub(crate) fn const_truthiness(n: &Node) -> Option<bool> {
    match n {
        Node::Const(ConstVal::Atom(v)) => Some(crate::eval::truthy(*v)),
        Node::Const(ConstVal::Handle { .. }) => Some(true),
        _ => None,
    }
}

/// Compile `(%scope)` / `(%locals)` (ADR-174 path B): a fresh map of every in-scope
/// local, `{:name → Local(slot)}`, read from the compile-time lexical-scope table.
/// Shadowing follows `Scope::lookup` (newest binding of a name wins — the reversed scan
/// keeps the first-seen, i.e. innermost, slot per symbol). Slots still mid-`letrec`
/// (`unsafe`) are skipped — their value isn't finalized, so exposing it would read a
/// placeholder. Keyed by the name as a **keyword** (same interned `Symbol`, so `%eval-in`
/// binds it to the local's symbol) — matching the debugger's explicitly-named `:vals`, so
/// a named value cleanly overrides a captured local of the same name on `merge`.
#[cfg(feature = "dev-tools")]
pub(crate) fn compile_scope_map(heap: &Heap, scope: &Scope) -> Node {
    let mut seen: Vec<Symbol> = Vec::new();
    let mut pairs: Vec<(Node, Node)> = Vec::new();
    for &(sym, slot) in scope.names.iter().rev() {
        if seen.contains(&sym) || scope.is_unsafe(slot) {
            continue;
        }
        seen.push(sym);
        pairs.push((const_node(heap, Value::keyword(sym)), Node::Local(slot)));
    }
    Node::Map(pairs.into_boxed_slice())
}

pub(crate) fn const_node(heap: &Heap, v: Value) -> Node {
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
pub(crate) fn value_is_immovable(v: Value) -> bool {
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
pub(crate) fn compile_try_catch(heap: &Heap, items: &[Value], scope: &mut Scope) -> Option<Node> {
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
pub(crate) fn body_symbols(heap: &Heap, body: Value) -> std::collections::HashSet<Symbol> {
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
pub(crate) fn compile_captures(
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
pub(crate) fn is_fn_form(heap: &Heap, form: Value) -> bool {
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
pub(crate) fn compile_make_closure(heap: &Heap, form: Value, scope: &Scope) -> Option<Node> {
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
    // inline lambda (e.g. pipeline's `(map … (fn (i) (* i i)))`); without help its
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
pub(crate) fn resolve_prim(heap: &Heap, h: Symbol) -> Option<(PrimOp, [usize; 2])> {
    let v = heap.env_get(heap.global(), h)?;
    // The canonical prelude `nth`: `(nth v i)` on a vector is a bounds-checked
    // slab read, so inline it as `VectorRef` — the call's own `head` (`nth`) drives
    // the deopt, so the list / out-of-range / explicit-default cases dispatch the
    // real `nth` unchanged. Guarded by region: a user `(def nth …)` rebinds `nth`
    // to a non-PRELUDE closure, which fails this check, so the inline cleanly
    // disables (and the same epoch guard that protects every other inlined prim
    // re-validates here on a redefinition).
    // `(get m k)` on a map — the read that had no primitive. Same shape as `nth` below:
    // matched by head symbol, and accepted only when the global still resolves to the
    // PRELUDE closure, so a user `(def get …)` cleanly disables the inline (and the epoch
    // guard every `Prim2` carries re-validates on a redefinition). Two args only: the
    // 3-arity `(get m k default)` never reaches here, and the variadic fold path below
    // accepts `Add`/`Mul` alone, so it cannot be folded either.
    //
    // The op inlines only a present, non-nil value; everything else defers to the real
    // `get`, which keeps the set / string / integer-index branches and `%lookup-miss` in
    // Brood. See [`PrimOp::MapGet`].
    if value::symbol_is(h, "get") {
        if !inline::mapget_enabled() {
            return None;
        }
        return match v.unpack() {
            ValueRef::Fn(id) if id.region() == crate::core::value::PRELUDE => {
                Some((PrimOp::MapGet, [0, 1]))
            }
            _ => None,
        };
    }
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
pub(crate) fn resolve_prim1(heap: &Heap, h: Symbol) -> Option<PrimOp1> {
    // `sqrt` inlines to a single `f64::sqrt` for x > 0 (zero/negative/NaN/bignum deopt to the
    // live wrapper via the stored head). ADR-227 moved `sqrt` out of the prelude into
    // `std/math.blsp`, so the head is now the qualified `math/sqrt` bound to a RUNTIME closure —
    // where the old "is it the sealed PRELUDE `sqrt`?" identity no longer holds and a hot-reload
    // rebind is possible. So identify the canonical wrapper STRUCTURALLY instead: it must be the
    // exact `(if (< n 0) _ (if (<= n 0) _ (%f64-sqrt n)))` shape over its single parameter, with
    // `<`/`<=` the canonical PRELUDE comparisons and `%f64-sqrt` the native — which is precisely
    // what makes the "x > 0 ⇒ %f64-sqrt(x)" shortcut sound. Name-independent (any `…/sqrt` head,
    // or a bare `sqrt`), and rebind-safe for free: `Inst::Prim1` re-runs this on every
    // `global_epoch` change, so a rebind of the wrapper, `<`, `<=`, or `%f64-sqrt` fails the
    // check and cleanly falls back to a dispatch. It degrades to no-inline on ANY deviation from
    // the shape (a reworded wrapper just stops inlining, guarded by a test) — never a miscompile.
    if symbol_is_sqrt(h) {
        return match heap.env_get(heap.global(), h)?.unpack() {
            ValueRef::Fn(id) if is_canonical_sqrt_wrapper(heap, id) => Some(PrimOp1::Sqrt),
            _ => None,
        };
    }
    match heap.env_get(heap.global(), h)?.unpack() {
        ValueRef::Native(id) => PrimOp1::from_native_name(&heap.native(id).name),
        _ => None,
    }
}

/// A head whose name is `sqrt` or ends in `/sqrt` — the only heads for which the structural
/// sqrt-wrapper probe below is worth running. Keeps the probe off every other 1-ary call.
pub(crate) fn symbol_is_sqrt(h: Symbol) -> bool {
    value::symbol_is(h, "sqrt") || value::symbol_name_ref(h).ends_with("/sqrt")
}

/// Destructure a call form `(head a b …)` into `(head-symbol, [a b …])`, or `None` if it is not
/// a proper list headed by a symbol. Read-only over `&Heap`, for the structural sqrt probe.
pub(crate) fn call_parts(heap: &Heap, form: Value) -> Option<(Symbol, SmallVec<[Value; 4]>)> {
    let (head, mut rest) = match form.unpack() {
        ValueRef::Pair(p) => heap.pair(p),
        _ => return None,
    };
    let head_sym = match head.unpack() {
        ValueRef::Sym(s) => s,
        _ => return None,
    };
    let mut args: SmallVec<[Value; 4]> = SmallVec::new();
    loop {
        match rest.unpack() {
            ValueRef::Nil => break,
            ValueRef::Pair(p) => {
                let (a, next) = heap.pair(p);
                args.push(a);
                rest = next;
            }
            _ => return None,
        }
    }
    Some((head_sym, args))
}

/// True iff `form` is a guard call `(op p 0)` where `op` (`<` / `<=`) resolves to the canonical
/// PRELUDE comparison closure — so a rebind of `<`/`<=` (which bumps `global_epoch`) fails it and
/// the `Prim1` re-validation drops the inline. `p` is the wrapper's sole parameter symbol.
pub(crate) fn is_zero_guard(heap: &Heap, form: Value, p: Symbol, op: &str) -> bool {
    let Some((h, args)) = call_parts(heap, form) else {
        return false;
    };
    if !value::symbol_is(h, op) || args.len() != 2 {
        return false;
    }
    let canonical = matches!(
        heap.env_get(heap.global(), h).map(|v| v.unpack()),
        Some(ValueRef::Fn(id)) if id.region() == crate::core::value::PRELUDE
    );
    canonical
        && matches!(args[0].unpack(), ValueRef::Sym(s) if s == p)
        && matches!(args[1].unpack(), ValueRef::Int(0))
}

/// True iff `form` is `(%f64-sqrt p)` and `%f64-sqrt` resolves to the actual native — so a rebind
/// of `%f64-sqrt` fails it and the inline drops. `p` is the wrapper's sole parameter symbol.
pub(crate) fn is_f64_sqrt_call(heap: &Heap, form: Value, p: Symbol) -> bool {
    let Some((h, args)) = call_parts(heap, form) else {
        return false;
    };
    if !value::symbol_is(h, "%f64-sqrt") || args.len() != 1 {
        return false;
    }
    let is_native = matches!(
        heap.env_get(heap.global(), h).map(|v| v.unpack()),
        Some(ValueRef::Native(id)) if heap.native(id).name == "%f64-sqrt"
    );
    is_native && matches!(args[0].unpack(), ValueRef::Sym(s) if s == p)
}

/// True iff closure `id` is the canonical `sqrt` wrapper: a single 1-parameter arm whose one body
/// form is exactly `(if (< n 0) _ (if (<= n 0) _ (%f64-sqrt n)))` — the shape that guarantees a
/// positive argument returns `%f64-sqrt(n)`, which is all the `PrimOp1::Sqrt` x>0 shortcut needs
/// (every other argument deopts to the live wrapper via the stored head). Any other closure —
/// including a hot-reload rebind of `math/sqrt` to something else — fails to match, so the inline
/// never fires for a function that is not this exact sqrt.
pub(crate) fn is_canonical_sqrt_wrapper(heap: &Heap, id: ClosureId) -> bool {
    let closure = heap.closure(id);
    let Some(arm) = closure.select_arm(1) else {
        return false;
    };
    if !arm.optionals.is_empty()
        || arm.rest.is_some()
        || arm.params.len() != 1
        || arm.body.len() != 1
    {
        return false;
    }
    let p = arm.params[0];
    // Outer `(if (< p 0) <error> <inner-if>)`.
    let Some((h_outer, outer)) = call_parts(heap, arm.body[0]) else {
        return false;
    };
    if !value::symbol_is(h_outer, "if")
        || outer.len() != 3
        || !is_zero_guard(heap, outer[0], p, "<")
    {
        return false;
    }
    // Inner `(if (<= p 0) <zero> (%f64-sqrt p))`.
    let Some((h_inner, inner)) = call_parts(heap, outer[2]) else {
        return false;
    };
    value::symbol_is(h_inner, "if")
        && inner.len() == 3
        && is_zero_guard(heap, inner[0], p, "<=")
        && is_f64_sqrt_call(heap, inner[2], p)
}

/// Compile an already-expanded, already-resolved `form` against the lexical
/// `scope`. `tail` is whether this form is in tail position. Returns `None` when
/// the form uses anything outside the VM's vocabulary (the caller then defers the
/// whole closure to the tree-walker).
pub(crate) fn compile_node(
    heap: &Heap,
    form: Value,
    scope: &mut Scope,
    tail: bool,
) -> Option<Node> {
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
                site: scope.gsite_alloc(),
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
                    // A literal test picks its branch at compile time. Both branches are
                    // already compiled above, so this only discards the losing *Node* —
                    // every compile-time effect (slot allocation, `note_definition` for
                    // LSP nav) has already happened, and nothing about evaluation order
                    // changes because a constant test evaluates to itself.
                    //
                    // This is what makes a constant catch-all free. `cond` expands to
                    // nested `if`s, so `(cond … :else x)` ends in `(if :else x nil)`;
                    // without this fold that emits a keyword constant + a branch, which
                    // drops the whole arm out of the unboxed-i64 register worker's
                    // subset — measured at **12×** on `ackermann` and ~1.7× on
                    // `collatz`/`primes` once ADR-154 stopped special-casing `:else`.
                    // The fold is general (`(if true a b)`, `(cond … 42 x)` alike), so
                    // no spelling is privileged and no caller has to know the rule.
                    if let Some(t) = const_truthiness(&cond) {
                        return Some(if t { then } else { els });
                    }
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
                // `(%scope)` / `(%locals)` — the debugger locals intrinsic (ADR-174
                // path B). Compile it straight from the live lexical-scope table into a
                // fresh `{name → value}` map (name = the local's symbol, value read from
                // its frame slot), so `eval-at` sees EVERY in-scope local under the VM —
                // not just the values named at `break`. Only the 0-arg form is the
                // intrinsic; anything else falls through to the builtin (tree-walker path).
                #[cfg(feature = "dev-tools")]
                if items.len() == 1
                    && (value::symbol_is(h, kw::SCOPE_PRIM) || value::symbol_is(h, kw::LOCALS_PRIM))
                {
                    return Some(compile_scope_map(heap, scope));
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
                // 2-arg prims, and the same thin-wrapper following — `head` stays the
                // ORIGINAL head, so a deopt dispatches the real wrapper unchanged.
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
            let mut callee = match head.unpack() {
                ValueRef::Sym(h) if scope.lookup(h).is_none() => Node::Global(h),
                _ => compile_node(heap, head, scope, false)?,
            };
            let mut args = Vec::with_capacity(items.len() - 1);
            for &a in &items[1..] {
                args.push(compile_node(heap, a, scope, false)?);
            }
            // BROOD_MONO Tier 1 (ADR-182): devirtualize an ability op call with a literal
            // first argument to a direct call to the resolved impl. Off by default — the
            // `mono_enabled()` bool is the only cost then, so default builds are unchanged.
            // A Const callee falls through to the NO_SITE (computed-head) path below.
            if inline::mono_enabled() {
                if let Node::Global(op) = callee {
                    if let Some(direct) = inline::mono_devirtualize(heap, scope, op, &args) {
                        callee = direct;
                    }
                }
            }
            // A free-global callee gets a call-site inline-cache id (ADR-096);
            // a local/computed callee can resolve to a different function per
            // call, so it keeps the generic path.
            let site = match callee {
                Node::Global(_) => scope.site_alloc(),
                _ => NO_SITE,
            };
            // KI-19: the operator must be resolved BEFORE the arguments, matching the
            // tree-walker. Only an argument that can run user code can rebind it, so only
            // those calls pay anything: the head becomes a `GlobalIc` (resolved at IC speed
            // ahead of the args) and the call is marked `staged`. `head`/`site` are kept, so
            // the call-site IC still caches the arm — see `Inst::Call::staged`.
            // …but only for a head that can *actually* be rebound. A **reserved** name —
            // everything the language ships: prelude, builtins, embedded std modules — is
            // refused by `def` (ADR-166), so its resolution cannot change mid-call and the
            // elided head stays correct. That exemption is what keeps the cost off the
            // prelude-heavy rows: staging every call regressed `regex` 31%, `wordcount` 11%
            // and `sieve` 9%, almost all of it calls to `first`/`rest`/`str`-class names
            // that were never rebindable.
            //
            // (A module load is the one context allowed to define its own reserved surface,
            // and `is_reserved_global` already reports false while one is in progress, so
            // that path stages conservatively.)
            let staged = site != NO_SITE
                && args.iter().any(node_runs_user_code)
                && !matches!(callee, Node::Global(sym) if heap.is_reserved_global(sym));
            if staged {
                if let Node::Global(sym) = callee {
                    callee = Node::GlobalIc {
                        sym,
                        site: scope.gsite_alloc(),
                    };
                }
            }
            let (pos, file) = match heap.form_pos(form) {
                Some((p, f)) => (Some(p), f),
                None => (None, None),
            };
            if site != NO_SITE {
                // Arm-relative: accumulate on the scope; `compile_arm` moves the vec
                // into the CompiledArm, and `Heap::vm_arm_block` copies it into the
                // process's absolute table (debug builds) when the block is resolved.
                let idx = site as usize;
                if scope.site_pos.len() <= idx {
                    scope.site_pos.resize(idx + 1, None);
                }
                scope.site_pos[idx] = pos.map(|p| (p, file.clone()));
            }
            Some(Node::Call {
                staged,
                callee: Box::new(callee),
                args: args.into_boxed_slice(),
                tail,
                pos,
                file,
                site,
            })
        }

        // Vector literal — evaluate each element (value position), build fresh…
        ValueRef::Vector(id) => {
            let items = heap.vector(id).to_vec();
            let mut nodes = Vec::with_capacity(items.len());
            for e in items {
                nodes.push(compile_node(heap, e, scope, false)?);
            }
            // …unless every element is itself constant, in which case the whole
            // literal is a constant: build it once and share that instance. Sound
            // *because Brood data is immutable* — no one can tell a shared vector from
            // a freshly built one, since neither can be mutated. Without this, a
            // literal like `[:a :b]` in a hot path allocates on every evaluation
            // (measured 2026-07-29: the `receive` tag-filter vector cost `pingpong`
            // +3.6% / `ring` +2.4% purely in per-call allocation).
            //
            // Folding to `form` is only valid when every element *evaluates to itself* —
            // i.e. its compiled constant is structurally what the source element already
            // was. That holds for the self-evaluating literals this is for (`[:a :b]`,
            // `[1 2]`, nested literals of them) and excludes the case that made the first
            // version of this a real bug: `'go` compiles to the symbol `go` while the
            // source element is still the *list* `(quote go)`, so folding the raw form
            // produced `[:tag (quote go)]` — which broke quoted-symbol patterns and
            // `'foo` dependency names. `compile_node` only holds `&Heap`, so building a
            // fresh constant from the compiled values isn't available here; requiring
            // self-evaluation keeps the win without needing it.
            let self_evaluating = nodes
                .iter()
                .zip(heap.vector(id).iter())
                .all(|(n, &src)| matches!(n, Node::Const(c) if heap.equal(c.load(), src)));
            if self_evaluating {
                return Some(const_node(heap, form));
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
