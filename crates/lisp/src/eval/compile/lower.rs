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
    /// The capture slot holding this closure's own self-name (a letrec loop's binding to
    /// itself), so the self-call rule can tell that slot from a shadowing local.
    pub(crate) self_slot: Option<usize>,
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
    /// The module the closure being compiled was written in (`Closure::module`, or the
    /// module of its own qualified name) — the contract boundary (ADR-383): a reference
    /// from this arm to a CONTRACTED sibling of that module compiles to the sibling's
    /// uncontracted binding ([`module_boundary_ref`]), and a nested `(fn …)` inherits
    /// it. `None` compiles every reference as written.
    pub(crate) module: Option<Symbol>,
    /// Stage every call's free-global head — resolve it before the arguments, as the
    /// tree-walker does — not only those whose arguments can run user code (KI-19). Set for
    /// a top-level form (`run`): it runs once, and a call whose operator AND argument are
    /// both unbound must report the operator, whose hint names the replacement — `(definterface
    /// Foo)` at a REPL is about `definterface`, not `Foo`. A nested `(fn …)` gets a fresh scope
    /// and keeps the elided head.
    pub(crate) stage_calls: bool,
    /// `(global-read site, call position)` for each staged head — moved into
    /// `CompiledArm::staged_head_pos`.
    pub(crate) staged_head_pos: Vec<(u32, crate::error::Pos)>,
    /// `(call position, head)` for each call whose head is a LOCAL — moved into
    /// `CompiledArm::local_call_heads`.
    pub(crate) local_call_heads: Vec<(crate::error::Pos, Symbol)>,
    /// The position of every positioned call, to tell a position one call owns from one
    /// several share (generated code is stamped with its origin's) — see
    /// [`Scope::take_local_call_heads`].
    pub(crate) call_positions: Vec<crate::error::Pos>,
    /// The innermost positioned form being compiled — where a global read below it is
    /// located (see `compile_node`).
    pub(crate) read_pos: Option<crate::error::Pos>,
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
            self_slot: None,
            sites: 0,
            gsites: 0,
            site_pos: Vec::new(),
            module: None,
            stage_calls: false,
            staged_head_pos: Vec::new(),
            local_call_heads: Vec::new(),
            call_positions: Vec::new(),
            read_pos: None,
        }
    }

    /// The arm's `local_call_heads`: each local-headed call keyed by its position — only
    /// where that position is the one call's. Generated code shares its origin's position, so
    /// `(do (~f 1) (~g 2))` puts a local `h` and a global at one place, and a lookup by
    /// position would blame `h` for the global's failure; such a position names nothing.
    pub(crate) fn take_local_call_heads(&mut self) -> Box<[(crate::error::Pos, Symbol)]> {
        let positions = std::mem::take(&mut self.call_positions);
        std::mem::take(&mut self.local_call_heads)
            .into_iter()
            .filter(|(at, _)| positions.iter().filter(|p| *p == at).count() == 1)
            .collect()
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
/// use-after-GC. `promote` freezes a LOCAL string/heap literal into the
/// immovable RUNTIME code region (the same freeze a `def`/`defn` body's literals
/// get) and is a no-op for inline atoms, interned keywords, and already-shared
/// PRELUDE/RUNTIME handles. **Route every literal `Const` through here** — the
/// invariant is easy to bypass with a bare `Node::Const(form)`; the sibling `MakeClosure`
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
        // The other boxed numbers and raw bytes — movable when LOCAL, like a BigInt.
        ValueRef::Decimal(id) => id.region() != value::LOCAL,
        ValueRef::Ratio(id) => id.region() != value::LOCAL,
        ValueRef::Bytes(id) => id.region() != value::LOCAL,
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
pub(crate) fn compile_binding(heap: &Heap, items: &[Value], scope: &mut Scope) -> Option<Node> {
    if items.len() != 4 {
        return None;
    }
    // `(quote (n…))` — the names, known now. Whether each is dynamic is checked when the
    // binding runs (a `defdyn` may come later), exactly as the native checks it.
    let quoted = heap.list_to_vec(items[1]).ok()?;
    if quoted.len() != 2
        || !matches!(quoted[0].unpack(), ValueRef::Sym(s) if value::symbol_is(s, kw::QUOTE))
    {
        return None;
    }
    let names: Box<[Symbol]> = heap
        .list_to_vec(quoted[1])
        .ok()?
        .into_iter()
        .map(|v| match v.unpack() {
            ValueRef::Sym(s) => Some(s),
            _ => None,
        })
        .collect::<Option<_>>()?;
    let thunk_items = heap.list_to_vec(items[3]).ok()?;
    if thunk_items.len() < 2
        || !matches!(thunk_items[0].unpack(), ValueRef::Sym(s) if value::symbol_is(s, kw::FN))
        || !heap.list_to_vec(thunk_items[1]).ok()?.is_empty()
    {
        return None;
    }
    let vals = compile_node(heap, items[2], scope, false)?;
    let body = compile_body(heap, &thunk_items[2..], scope, false)?;
    Some(Node::Binding {
        names,
        vals: Box::new(vals),
        body: Box::new(body),
    })
}

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
    if let ValueRef::Pair(p) = form.unpack()
        && let ValueRef::Sym(h) = heap.pair(p).0.unpack()
    {
        return value::symbol_is(h, kw::FN);
    }
    false
}

/// Compile a `(fn …)` evaluated inside a compiled body to a
/// [`Node::MakeClosure`] (Stage 2c), or `None` (defer) if it can't be VM-built. The
/// closure's *body* is not compiled here — it's compiled lazily by [`compiled_for`]
/// when the closure is first called, keyed by its RUNTIME body handle.
pub(crate) fn compile_make_closure(heap: &Heap, form: Value, scope: &Scope) -> Option<Node> {
    // Post-macroexpand a pattern-param / multi-clause `fn` is already lowered to
    // `%match`; a `fn` reaching here should be plain. Defer defensively otherwise.
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
        module: scope.module,
    })
}

/// The global a free symbol in this arm compiles to (ADR-383): the symbol itself, unless
/// it names a CONTRACTED function of the arm's own module, in which case the sibling's
/// uncontracted binding. A module's calls to its own functions are inside the boundary
/// the contract guards — the checker holds them to the declaration statically, and the
/// shim would charge every internal call for a check the module's author owes only to
/// callers outside it (`string/format` reached `string/char-at` per character: ×35).
///
/// Decided at COMPILE time, once per site, from the binding that exists then: the
/// uncontracted alias is bound exactly while a shim is (`contracts::contract_bind`), so
/// an arm compiled before its module's sweep keeps the public name and is merely
/// checked. A stale alias reference — the globals rolled back under an arm the shared
/// body cache kept — resolves to the public name on the miss path, so it can only ever
/// check more. Free of cost with no shim bound: one static load.
fn module_boundary_ref(heap: &Heap, scope: &Scope, sym: Symbol) -> Symbol {
    match scope.module {
        Some(module) if crate::builtins::contracts::boundary_active() => {
            crate::builtins::contracts::uncontracted_sibling(heap, module, sym).unwrap_or(sym)
        }
        _ => sym,
    }
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
        // `BROOD_NO_MAPGET` opts out.
        if !crate::diagnostics::debug_flags::map_get_lowering() {
            return None;
        }
        return match v.unpack() {
            ValueRef::Fn(id) if id.region() == crate::core::value::PRELUDE => {
                Some((PrimOp::MapGet, [0, 1]))
            }
            _ => None,
        };
    }
    // A compiled keyword call `(:k m)` (see `compile_keyword_call`): the same probe as `get`,
    // with the keyword's own fallback. Accepted while `%keyword-get` is the kernel native.
    if value::symbol_is(h, "%keyword-get") {
        if !crate::diagnostics::debug_flags::map_get_lowering() {
            return None;
        }
        return match v.unpack() {
            ValueRef::Native(_) => Some((PrimOp::MapGet, [0, 1])),
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

/// Tighter variant of [`prim_apply`] for the range-fold hot path: both
/// operands are already `i64` (range element + integer accumulator), no Value
/// boxing involved. Returns the next `i64` accumulator, or `None` on overflow
/// (caller must fall back to the full `prim_apply` / `eval_apply` path).
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
    // The canonical prelude `count`, by head and region exactly as `nth` is in
    // `resolve_prim`: a user `(def count …)` is not the PRELUDE closure, so the inline
    // drops, and the epoch guard on `Inst::Prim1` re-validates after any redefinition.
    if value::symbol_is(h, "count") {
        return match heap.env_get(heap.global(), h)?.unpack() {
            ValueRef::Fn(id) if id.region() == crate::core::value::PRELUDE => Some(PrimOp1::Count),
            _ => None,
        };
    }
    match heap.env_get(heap.global(), h)?.unpack() {
        ValueRef::Native(id) => PrimOp1::from_native_name(&heap.native(id).name),
        // A type predicate: `(defn vector? (x) (%eq (type-of x) :vector))` and its kin.
        // Recognised by shape (see [`type_predicate_tag`]), never by name — a user's own
        // `(defn even? …)` is not that shape and simply compiles as a call. The `?`
        // suffix only keeps the probe off every other 1-ary call head.
        ValueRef::Fn(id) if symbol_is_predicate(h) => {
            type_predicate_tag(heap, id).map(PrimOp1::TypeIs)
        }
        _ => None,
    }
}

/// A head whose name ends in `?` — the only heads worth running the type-predicate
/// probe on (`resolve_prim1` is asked about every 1-ary call head the compiler meets).
pub(crate) fn symbol_is_predicate(h: Symbol) -> bool {
    value::symbol_name_ref(h).ends_with('?')
}

/// The `type-of` keyword closure `id` tests for, iff it is a canonical type predicate: a
/// single 1-parameter arm whose one body form is exactly `(%eq (type-of p) :kw)` with
/// `%eq` and `type-of` resolving to their natives. That shape is what makes
/// [`PrimOp1::TypeIs`] sound: the wrapper's whole answer IS the tag compare, over every
/// operand, so nothing is left to deopt to. Any other closure — a predicate with a guard,
/// a rebound name, a user function that merely ends in `?` — is `None`, and the call
/// compiles as a call.
pub(crate) fn type_predicate_tag(heap: &Heap, id: ClosureId) -> Option<Symbol> {
    let closure = heap.closure(id);
    let arm = closure.select_arm(1)?;
    if !arm.optionals.is_empty()
        || arm.rest.is_some()
        || arm.params.len() != 1
        || arm.body.len() != 1
    {
        return None;
    }
    let p = arm.params[0];
    // `(%eq <type-of-call> <keyword>)` — the native `%eq`, by the name `PrimOp::Eq` keys on.
    let (h_eq, args) = call_parts(heap, arm.body[0])?;
    if args.len() != 2 || !head_is_native(heap, h_eq, kw::EQ_PRIM) {
        return None;
    }
    let kw = match args[1].unpack() {
        ValueRef::Keyword(k) => k,
        _ => return None,
    };
    // `(type-of p)` — the native, applied to the arm's own parameter.
    let (h_type, inner) = call_parts(heap, args[0])?;
    let is_type_of = inner.len() == 1
        && head_is_native(heap, h_type, "type-of")
        && matches!(inner[0].unpack(), ValueRef::Sym(s) if s == p);
    is_type_of.then_some(kw)
}

/// True iff symbol `h` is bound, in the global env, to the native called `name`.
fn head_is_native(heap: &Heap, h: Symbol, name: &str) -> bool {
    matches!(
        heap.env_get(heap.global(), h).map(|v| v.unpack()),
        Some(ValueRef::Native(id)) if heap.native(id).name == name
    )
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
    // A compound form recurses once per nesting level, and an expansion may be nested
    // to `macros::MAX_DEPTH` (1024) — one level per arm of a big dispatch — so a
    // compound step grows the native stack when it is near its end. An atom pays
    // nothing. Compile-time only: the compiled arm is cached and shared.
    if matches!(form.unpack(), ValueRef::Pair(_)) {
        // The innermost positioned form encloses every global read below it: an unbound
        // name in argument position reports there, not at whatever call reached this arm.
        let enclosing = scope.read_pos;
        if let Some(pos) = heap.form_pos_only(form) {
            scope.read_pos = Some(pos);
        }
        let node = crate::stack::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            compile_node_inner(heap, form, scope, tail)
        });
        scope.read_pos = enclosing;
        node
    } else {
        compile_node_inner(heap, form, scope, tail)
    }
}

fn compile_node_inner(heap: &Heap, form: Value, scope: &mut Scope, tail: bool) -> Option<Node> {
    match form.unpack() {
        // Self-evaluating literals. `const_node` freezes any embedded heap handle
        // into the immovable RUNTIME region — load-bearing for `Value::Str` (a LOCAL
        // string baked raw into the off-GC-graph AST is the use-after-GC class; see
        // `const_node`), a no-op for the inline/interned atoms.
        // A ratio (`1/3`) and a decimal (`1.5M`) are literals too: outside this list they were
        // outside the VM's vocabulary, so any function containing one ran on the tree-walker.
        // So was a bytes literal (`#b"\r\n"`) until 2026-10-20 — and a `receive` under the
        // tree-walker cannot park, so it blocks its OS worker: every `http/request` (`#b""`
        // in its body) dirty-blocked twice, and 500 concurrent fetches grew past 64 overflow
        // scheduler threads (found porting brood-benchmarks' `http` row).
        ValueRef::Int(_)
        | ValueRef::BigInt(_)
        | ValueRef::Ratio(_)
        | ValueRef::Decimal(_)
        | ValueRef::Bytes(_)
        | ValueRef::Float(_)
        | ValueRef::Bool(_)
        | ValueRef::Nil
        | ValueRef::Str(_)
        | ValueRef::Keyword(_) => Some(const_node(heap, form)),

        // A name: a local frame slot if bound, else a global reference with a
        // read IC (ADR-096) — through the module boundary (ADR-383).
        ValueRef::Sym(s) => match scope.lookup(s) {
            Some(slot) => Some(Node::Local(slot)),
            None => {
                let site = scope.gsite_alloc();
                // Where the read is written, for its miss (`staged_head_pos`, keyed by site).
                if let Some(pos) = scope.read_pos {
                    scope.staged_head_pos.push((site, pos));
                }
                Some(Node::GlobalIc {
                    sym: module_boundary_ref(heap, scope, s),
                    site,
                })
            }
        },

        // A combination — a special form we handle (`if`/`do`) or a function call.
        ValueRef::Pair(_) => {
            let items = heap.list_to_vec(form).ok()?;
            let head = *items.first()?;
            // `(:k m)` — a keyword called on one argument, the accessor shape (ADR-165).
            // Compiled as a call it took the generic call path, and the JIT refuses an arm
            // holding a call it cannot mediate (`call-mediated-boxed`): every function that
            // read a field this way stayed on the VM, ~2000 instructions a call more than the
            // same read spelled `(get m :k)`. It compiles to `get`'s own probe instead; the
            // fallback head is `%keyword-get`, so whatever the probe does not answer runs the
            // keyword's rule, its record and set answers and its error included.
            if let ValueRef::Keyword(_) = head.unpack()
                && items.len() == 2
            {
                let h = value::intern("%keyword-get");
                if scope.lookup(h).is_none()
                    && let Some((PrimOp::MapGet, [0, 1])) = resolve_prim(heap, h)
                {
                    let a = compile_node(heap, items[1], scope, false)?;
                    return Some(Node::Prim2 {
                        op: PrimOp::MapGet,
                        a: Box::new(a),
                        b: Box::new(const_node(heap, head)),
                        map: [0, 1],
                        head: h,
                        guard: AtomicU64::new(heap.global_epoch()),
                        pos: heap.form_pos_only(form),
                        broot: false,
                    });
                }
            }
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
                    // Without this fold a literal test emits a constant + a branch, which
                    // drops the whole arm out of the unboxed-i64 register worker's subset
                    // (the `(cond … :else x)` cliff, before ADR-518 made `cond` refuse a
                    // constant test). A hand-written `(if true a b)` and macro output that
                    // builds a literal test still reach here, so the fold stays general.
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
                    // quotes data — notably %match's no-match arm,
                    // `(%raise (%match-error (quote :ctx) m (quote pats)))`, which had
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
                if value::symbol_is(h, kw::TRY_PRIM)
                    && let Some(node) = compile_try_catch(heap, &items, scope)
                {
                    return Some(node);
                }
                // Non-canonical shape: fall through to generic call (try_catch native handles it)
                // `(%binding '(n…) [v…] (fn () body…))` — the `binding` macro's expansion —
                // inline (ADR-402): the body runs in this frame instead of as a thunk the
                // `%binding` native evaluates nested.
                if value::symbol_name_ref(h) == "%binding"
                    && let Some(node) = compile_binding(heap, &items, scope)
                {
                    return Some(node);
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
                // `defmacro`/`and`/`or`/`match`/`%match` aren't special forms — they're
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
                // `Node::Prim2`. The `(Int, Int)` case then runs inline in the bytecode loop,
                // skipping the global lookup, passthrough redirect, `compiled_for`
                // cache hit, arity check, and native dispatch the generic call path
                // pays per operator per iteration. Guarded by the global epoch so a
                // redefinition of the operator cleanly falls back (see `Node::Prim2`).
                // 1-ary sequence primitives (`first`/`rest`) inline the same way
                // (ADR-096) — the list-iteration workhorses of every prelude
                // sequence fn.
                // `(vector a …)` builds what the literal `[a …]` does, so it compiles to the
                // literal's `MakeVector` instead of a call to the variadic native: no call
                // frame, and an arm whose only vector is built this way gets the general
                // lowering's `MakeVector` admission rather than the call-mediated bail
                // (`nbody`'s `advance` ran on the VM for it). No guard, unlike the prims
                // above: a bare `vector` is a reserved name, so the native it names now is the
                // one it names for good, and a module that defines its own `vector` has every
                // bare use qualified to it (`m/vector`), which this does not match. Capped at
                // the JIT's `MakeVector` limit, past which a call keeps the arm admissible.
                if value::symbol_is(h, "vector")
                    && (2..=33).contains(&items.len())
                    && scope.lookup(h).is_none()
                    && head_is_native(heap, h, "vector")
                {
                    let elems = items[1..]
                        .iter()
                        .map(|&e| compile_node(heap, e, scope, false))
                        .collect::<Option<Vec<_>>>()?;
                    return Some(Node::Vector(elems.into_boxed_slice()));
                }
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
                    // Unary `(- x)` / `(/ x)`: the prelude's one-argument arms are exactly
                    // `(%sub 0 x)` and `(%div 1 x)`, so lower them as that `Prim2` with the
                    // identity as the left operand. As a generic `Call` to the variadic
                    // wrapper it would be a native→native link and, being a NON-tail call,
                    // a GC safepoint that keeps the arm from hoisting its pair-slab bases.
                    // Same guard discipline as the 2-arg case —
                    // `head` stays `-`, so a deopt dispatches the real wrapper with
                    // `(0 x)`, which is the same answer the 1-arg arm gives.
                    if let Some((op @ (PrimOp::Sub | PrimOp::Div), [0, 1])) = resolve_prim(heap, h)
                    {
                        let identity = if matches!(op, PrimOp::Sub) { 0 } else { 1 };
                        let b = compile_node(heap, items[1], scope, false)?;
                        let broot = !matches!(
                            b,
                            Node::Const(_)
                                | Node::Local(_)
                                | Node::Global(_)
                                | Node::GlobalIc { .. }
                        );
                        return Some(Node::Prim2 {
                            op,
                            a: Box::new(Node::Const(ConstVal::new(Value::int(identity)))),
                            b: Box::new(b),
                            map: [0, 1],
                            head: h,
                            guard: AtomicU64::new(heap.global_epoch()),
                            pos: heap.form_pos_only(form),
                            broot,
                        });
                    }
                }
                if items.len() == 3
                    && scope.lookup(h).is_none()
                    && let Some((op, map)) = resolve_prim(heap, h)
                {
                    let a = compile_node(heap, items[1], scope, false)?;
                    let b = compile_node(heap, items[2], scope, false)?;
                    // `a`'s value needs a root slot across `b`'s eval only
                    // if `b` can reach a safepoint (see the field doc).
                    let broot = !matches!(
                        b,
                        Node::Const(_) | Node::Local(_) | Node::Global(_) | Node::GlobalIc { .. }
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
                // 3-arg inlinable primitive (`table-put`): same guard discipline as the
                // 2-arg prims, and the same thin-wrapper following — `head` stays the
                // ORIGINAL head, so a deopt dispatches the real wrapper unchanged. The two
                // map ops (ADR-368) are NOT lowered here: the linear-map rewrite and its
                // probe (`linmap_probe`, run from the `def` expander) match `(get acc k 0)`
                // and `(assoc acc k …)` as CALL shapes, so they stay calls through every
                // Node pass and become `Prim3` last, in `inline::lower_map_prim3`.
                if items.len() == 4
                    && scope.lookup(h).is_none()
                    && let Some(op3 @ PrimOp3::TablePut) = resolve_prim3(heap, h)
                {
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
                // N-ary associative arithmetic (`(+ a b c …)`, `(* …)`) whose head is a
                // free reference to the prelude operator: left-fold into nested 2-ary
                // `Prim2` so each step inlines to a native add/mul (and the whole arm can
                // tier), instead of dispatching the variadic prelude `fold` once per call
                // (e.g. bintree's `(+ 1 (check …) (check …))`). Left-fold matches the
                // prelude's own `fold`; each `Prim2(Add/Mul)` deopts on i64 overflow exactly
                // as `%add`/`%mul` promote to BigInt, so results stay identical. Restricted
                // to the associative reducers with the in-order map `[0,1]` — never a
                // comparison (`<`/`=` chain pairwise, not fold) or a swapped wrapper.
                if items.len() > 3
                    && scope.lookup(h).is_none()
                    && let Some((op, [0, 1])) = resolve_prim(heap, h)
                    && matches!(op, PrimOp::Add | PrimOp::Mul)
                {
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
                // Multi-pair `(assoc coll k1 v1 k2 v2 …)` (ADR-368 follow-up, handoff item 2):
                // unrolled at compile time into nested single-pair calls, `(assoc (assoc coll
                // k1 v1) k2 v2)`, each of which `inline::lower_map_prim3` then makes a
                // `PrimOp3::MapAssoc`, instead of the wrapper's variadic arm building a rest
                // list and looping `%assoc-map-pairs` over it. Semantics are the wrapper's: a
                // map path-copies per pair, a vector goes through the single-pair arm's
                // `%vector-assoc` branch per pair, a duplicate key is last-wins, the
                // operands evaluate in source order and each pair's assoc happens before the
                // next pair evaluates — an intermediate map is unobservable, so the one
                // difference (the wrapper evaluates every operand first) cannot be seen. Only
                // while `assoc` is the PRELUDE closure (`resolve_prim3`'s rule), so a user
                // `(def assoc …)` gets its own semantics untouched.
                if items.len() >= 6
                    && items.len() % 2 == 0
                    && scope.lookup(h).is_none()
                    && resolve_prim3(heap, h) == Some(PrimOp3::MapAssoc)
                {
                    let pos = heap.form_pos_only(form);
                    let file = heap.form_pos(form).and_then(|(_, f)| f);
                    let mut acc = compile_node(heap, items[1], scope, false)?;
                    for pair in items[2..].chunks(2) {
                        let k = compile_node(heap, pair[0], scope, false)?;
                        let v = compile_node(heap, pair[1], scope, false)?;
                        // Each call gets a site of its own, like any named-head call: the
                        // emitter does not push a named head, so the call resolves it
                        // through its site. `lower_map_prim3` makes these primitives in an
                        // arm, but a top-level form never meets that pass, and there a
                        // site-less call left the head unresolved.
                        let site = scope.site_alloc();
                        let idx = site as usize;
                        if scope.site_pos.len() <= idx {
                            scope.site_pos.resize(idx + 1, None);
                        }
                        scope.site_pos[idx] = pos.map(|p| (p, file.clone()));
                        acc = Node::Call {
                            callee: Box::new(Node::Global(h)),
                            args: Box::new([acc, k, v]),
                            tail: false,
                            pos,
                            file: file.clone(),
                            site,
                            staged: false,
                        };
                    }
                    return Some(acc);
                }
            }
            // Direct `letrec` self-recursive tail call (the self-call optimization):
            // a tail call whose head is this closure's own self-name, not shadowed by
            // a local, with exactly the arm's arity. Re-runs the current arm via the
            // trampoline without resolving the callee or dispatching. A non-tail
            // self-call, a shadowed name, or a mismatched arity falls through to the
            // regular env-resolved path below (still correct).
            //
            // "Not shadowed by a local" must admit the self-name's OWN capture slot: a
            // letrec closure captures the frame that binds its name to itself, and `#3
            // lexical addressing` binds every capture to a frame slot — so `lookup` finds
            // the name bound, and treating that as shadowing would decline every letrec
            // loop (a `Call` through the captured closure, far slower than a `defn` loop).
            // A `let` that rebinds the name inside the body still
            // resolves to ITS slot, not `self_slot`, and still declines.
            if tail
                && let (ValueRef::Sym(h), Some((name, arity))) = (head.unpack(), scope.self_call)
            {
                let bound = scope.lookup(h);
                if h == name
                    && (bound.is_none() || bound == scope.self_slot)
                    && items.len() - 1 == arity
                {
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
            // Function call: compile the callee and every argument (value position).
            // A free-symbol head compiles to a plain `Node::Global` (not a
            // `GlobalIc`): the call's own site IC below caches the head's full
            // resolution, so a read IC there would be redundant (and waste a site).
            let local_head = match head.unpack() {
                ValueRef::Sym(h) if scope.lookup(h).is_some() => Some(h),
                _ => None,
            };
            let mut callee = match head.unpack() {
                ValueRef::Sym(h) if local_head.is_none() => {
                    Node::Global(module_boundary_ref(heap, scope, h))
                }
                _ => compile_node(heap, head, scope, false)?,
            };
            let mut args = Vec::with_capacity(items.len() - 1);
            for &a in &items[1..] {
                args.push(compile_node(heap, a, scope, false)?);
            }
            // ADR-182: an ability op call whose first argument's identity is provable skips
            // the op's `%identity-of`. The rewritten head is computed, so it takes the
            // NO_SITE path below.
            if let Node::Global(op) = callee
                && let Some(direct) = inline::mono_devirtualize(heap, scope, op, &args)
            {
                callee = direct;
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
            // prelude-heavy rows, whose calls to `first`/`rest`/`str`-class names can never
            // be rebound.
            //
            // (A module load is the one context allowed to define its own reserved surface,
            // and `is_reserved_global` already reports false while one is in progress, so
            // that path stages conservatively.)
            let staged = site != NO_SITE
                && (scope.stage_calls || args.iter().any(node_runs_user_code))
                && !matches!(callee, Node::Global(sym) if heap.is_reserved_global(sym));
            if staged && let Node::Global(sym) = callee {
                callee = Node::GlobalIc {
                    sym,
                    site: scope.gsite_alloc(),
                };
            }
            let (pos, file) = match heap.form_pos(form) {
                Some((p, f)) => (Some(p), f),
                None => (None, None),
            };
            if let (Node::GlobalIc { site: gsite, .. }, Some(p)) = (&callee, pos)
                && staged
            {
                scope.staged_head_pos.push((*gsite, p));
            }
            if let Some(p) = pos {
                scope.call_positions.push(p);
                if let Some(h) = local_head {
                    scope.local_call_heads.push((p, h));
                }
            }
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
            // literal like `[:a :b]` in a hot path (a `receive` tag-filter vector)
            // allocates on every evaluation.
            //
            // Folding to `form` is only valid when every element *evaluates to itself* —
            // i.e. its compiled constant is structurally what the source element already
            // was. That holds for the self-evaluating literals this is for (`[:a :b]`,
            // `[1 2]`, nested literals of them) and excludes `'go`, which compiles to the
            // symbol `go` while the source element is still the *list* `(quote go)` —
            // folding the raw form would produce `[:tag (quote go)]`. `compile_node` only holds `&Heap`, so building a
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

        // Set literal — like a vector literal: a constant when every element evaluates to
        // itself, else evaluate the elements and build the set with `%set` (the literal's
        // programmatic form: it dedups by structural equality, as the evaluator's
        // `set_from_elems` does). A set literal used to be outside the VM's vocabulary, so
        // any function containing one ran on the tree-walker for good — slower, never
        // preempted, and its `receive`s blocked their worker: the default crash reporter's
        // `#{}` kept it on a worker thread for the life of the program after its first report.
        ValueRef::Set(id) => {
            let items = heap.set_elems(id);
            let mut nodes = Vec::with_capacity(items.len());
            for &e in &items {
                nodes.push(compile_node(heap, e, scope, false)?);
            }
            let self_evaluating = nodes
                .iter()
                .zip(items.iter())
                .all(|(n, &src)| matches!(n, Node::Const(c) if heap.equal(c.load(), src)));
            if self_evaluating {
                return Some(const_node(heap, form));
            }
            // `%set` is a reserved kernel name — no `def` can rebind it — so the head needs
            // no staging (see the call path above).
            let site = scope.site_alloc();
            let (pos, file) = match heap.form_pos(form) {
                Some((p, f)) => (Some(p), f),
                None => (None, None),
            };
            let idx = site as usize;
            if scope.site_pos.len() <= idx {
                scope.site_pos.resize(idx + 1, None);
            }
            scope.site_pos[idx] = pos.map(|p| (p, file.clone()));
            Some(Node::Call {
                staged: false,
                callee: Box::new(Node::Global(value::intern("%set"))),
                args: nodes.into_boxed_slice(),
                tail: false,
                pos,
                file,
                site,
            })
        }

        // Opaque handles, etc. — outside the VM's vocabulary.
        _ => None,
    }
}
