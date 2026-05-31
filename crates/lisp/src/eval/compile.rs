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
use std::sync::Arc;

use crate::core::heap::{EnvRoot, Heap, VmCacheKey};
use crate::core::keywords as kw;
use crate::core::value::{self, ClosureId, EnvId, Symbol, Value};
use crate::error::{error_codes, LispError, LispResult, Pos};

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
        !matches!(v.trim().to_ascii_lowercase().as_str(), "" | "0" | "false" | "off" | "no")
    }
    *ON.get_or_init(|| match std::env::var("BROOD_VM") {
        Ok(v) => truthy(&v), // explicit override (BROOD_VM=0 → tree-walker)
        Err(_) => true,      // VM is the default engine
    })
}

/// A compiled IR node (ADR-076). Stage 1 vocabulary — the core forms a top-level
/// arithmetic/recursive body is built from. Anything outside this set makes the
/// whole closure ineligible (it runs on the tree-walker instead), so there is no
/// `Defer` node: a VM-run body is *fully* compiled, which is what lets `exec_node`
/// never need an `EnvId` for locals.
pub enum Node {
    /// A self-evaluating literal (number, bool, nil, string, keyword). **Invariant:
    /// holds an immovable value** — an inline atom, an interned keyword, or a
    /// PRELUDE/RUNTIME handle — so the cached `Node` tree (an `Arc` off the GC root
    /// graph) needs no rooting and can't dangle. Construct only via [`const_node`],
    /// which `promote`s to enforce this; a bare `Const(local_handle)` is the
    /// use-after-GC bug fixed 2026-05-31.
    Const(Value),
    /// A lexically-addressed local read: frame-slot `index` (depth 0 in the
    /// slice — only the callee's own params). Reads `root_at(frame_base + index)`.
    Local(usize),
    /// A free reference — resolved at run time through the global env (`env_get`,
    /// which also consults the dynamic-binding stack), exactly as the tree-walker
    /// resolves a non-local symbol.
    Global(Symbol),
    /// `(if cond then else)` — `cond` in value position, the branches inheriting
    /// the enclosing tail position.
    If(Box<Node>, Box<Node>, Box<Node>),
    /// `(do a b … z)` — all but the last for effect, the last in tail position.
    Do(Box<[Node]>),
    /// A vector literal `[a b …]` — evaluate each element (value position), then
    /// build a fresh vector. (A *quoted* vector `'[…]` is immutable data and compiles
    /// to a single immovable `Const` via `quote`, not this.)
    Vector(Box<[Node]>),
    /// A map literal `{k v …}` — evaluate each key and value (value position), then
    /// build a fresh map. (A *quoted* map is a `Const`, not this.)
    Map(Box<[(Node, Node)]>),
    /// A combination. `tail` marks a tail call (the trampoline reuses the frame
    /// instead of recursing — proper TCO). Non-tail calls recurse via [`vm_apply`].
    /// `pos` is the source `line:col` of this combination, captured at compile time
    /// (when the form's reader-recorded position is still live — see
    /// [`Heap::form_pos`]); an error from this call is tagged with it (innermost
    /// wins, like the tree-walker's `or_form_pos`) so VM diagnostics keep line/col.
    /// `None` for a promoted RUNTIME body (whose forms carry no recorded position —
    /// neither engine tags those).
    Call {
        callee: Box<Node>,
        args: Box<[Node]>,
        tail: bool,
        pos: Option<Pos>,
    },
    /// `let`/`let*`/`letrec` (Stage 2a). Lexical scope is **flattened** into the
    /// single activation frame: each binder owns a frame slot (pre-allocated in
    /// `nslots`). Evaluate each `rhs` and write it into its `slot`
    /// (`set_root_at`), in order, then run `body` (tail-propagated). `let`/`let*`
    /// are sequential (a rhs sees earlier binders); `letrec` pre-allocates all
    /// slots (init `nil`) so a rhs can reference any binder.
    LetBind {
        binds: Box<[(usize, Node)]>,
        body: Box<Node>,
    },
    /// `(fn …)`/`(lambda …)` evaluated *inside* a compiled body (Stage 2c). Builds
    /// a closure value that closes over a **flat snapshot** of the enclosing lexical
    /// environment: a fresh env frame (parent = the process global) is filled from
    /// `captures` — each `(name, src)` evaluates `src` in the current frame and
    /// binds it under `name` — and the closure captures that frame. Free vars in the
    /// new closure's body then resolve by name through it (`env_get`), exactly as a
    /// tree-walker-built closure resolves through its captured env chain (Brood
    /// bindings are immutable, so a value snapshot is equivalent to an env
    /// reference). `fn_rest` is the `(fn …)` form's cdr — an immovable RUNTIME
    /// sub-form parsed by [`crate::eval::make_closure`] at run time (reusing all the
    /// arity/optional/doc parsing).
    MakeClosure {
        fn_rest: Value,
        captures: Box<[(Symbol, Node)]>,
    },
}

/// The compiled counterpart of a [`ClosureArm`](crate::core::value::ClosureArm):
/// the frame layout and the compiled body. Cached per closure on the heap
/// (`Heap::vm_cache_*`). Immutable and `Send + Sync` (its `Node`s hold only
/// immovable handles + symbols + indices), so it lives behind an `Arc`.
///
/// Slot layout: required params `0..nrequired`, then `&optional` params
/// `nrequired..nrequired+noptional`, then the `&` rest slot (if any), then the
/// `let`/`letrec` binders — up to `nslots`. A missing optional takes its default:
/// `nil` (no eval) for a nil-default param, or the compiled `optional_defaults`
/// node (evaluated against the partially-built frame, so it can reference earlier
/// params) for a real default.
pub struct CompiledArm {
    /// Required params — `argv[0..nrequired]` fill slots `0..nrequired`. Selection
    /// guarantees `argc >= nrequired`, so they're always present.
    pub nrequired: usize,
    /// Count of `&optional` params. A provided arg fills the slot; a missing one
    /// takes its default (see `optional_defaults`).
    pub noptional: usize,
    /// Per-optional default, indexed `0..noptional`: `None` = nil-default (just push
    /// `nil`), `Some(node)` = a real default form, compiled in a scope where the
    /// required params and *earlier* optionals are bound. Evaluated by `push_frame`
    /// only when the optional's arg is missing — left-to-right, so a later default
    /// sees earlier ones (matching the tree-walker).
    pub optional_defaults: Box<[Option<Node>]>,
    /// `&` rest param's slot, if any: collects `argv[nrequired+noptional..]` into a
    /// fresh list.
    pub rest_slot: Option<usize>,
    /// Total frame slots (params + optionals + rest + `let`/`letrec` binders).
    pub nslots: usize,
    pub body: Node,
}

/// One arm of a closure: its arity shape plus the compiled body **if** it was
/// VM-eligible. Every arm is recorded — even ones that defer — so [`arm_for`]
/// reproduces [`Closure::select_arm`](crate::core::value::Closure::select_arm)
/// *exactly* (picks the same arm) before checking whether that arm can run on the
/// VM. Without the full table a variadic arm (which accepts a *range* of arities)
/// could be picked where the tree-walker would pick an overlapping fixed arm — a
/// silent wrong-arm miscompile.
struct ArmSpec {
    nrequired: usize,
    noptional: usize,
    has_rest: bool,
    compiled: Option<Arc<CompiledArm>>,
}

impl ArmSpec {
    fn accepts(&self, argc: usize) -> bool {
        argc >= self.nrequired && (self.has_rest || argc <= self.nrequired + self.noptional)
    }
}

/// A compiled closure: every arm's arity shape + (if VM-eligible) its compiled body.
pub struct CompiledClosure {
    arms: Vec<ArmSpec>,
}

impl CompiledClosure {
    /// The compiled arm to run for `argc`, or `None` to defer to the tree-walker.
    /// Mirrors `Closure::select_arm`: among accepting arms, prefer a fixed (no-rest)
    /// arm, then the most required params; ties resolve to the later arm (Rust's
    /// `max_by_key`, same as eval). Returns the winner's compiled body iff it was
    /// VM-eligible — otherwise `None`, so the tree-walker runs the *same* arm.
    fn arm_for(&self, argc: usize) -> Option<&Arc<CompiledArm>> {
        let winner = self
            .arms
            .iter()
            .filter(|a| a.accepts(argc))
            .max_by_key(|a| (!a.has_rest, a.nrequired))?;
        winner.compiled.as_ref()
    }
}

/// The result of running a node: a finished value, or a *tail call* the trampoline
/// must continue (reusing the frame). `Tail` is only ever produced for a `Call`
/// node compiled with `tail == true`, which only appears in a closure body run by
/// [`vm_apply`] — so it never escapes to a value context.
enum Step {
    Done(Value),
    Tail {
        compiled: Arc<CompiledArm>,
        args: SmallVec<[Value; 4]>,
        /// The tail callee's own captured env — the trampoline switches `genv` to
        /// this so the next arm resolves its free vars in *its* scope (Stage 2c: a
        /// tail call can cross into a closure with a different captured env).
        genv: EnvId,
    },
}

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
}

impl Scope {
    fn new() -> Self {
        Scope {
            names: Vec::new(),
            next: 0,
            max: 0,
            enclosing: Vec::new(),
            unsafe_slots: Vec::new(),
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
        self.names.iter().rev().find(|(n, _)| *n == sym).map(|&(_, slot)| slot)
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
    match form {
        Value::Nil => Some(Vec::new()),
        Value::Vector(vid) => Some(heap.vector(vid).to_vec()),
        Value::Pair(_) => heap.list_to_vec(form).ok(),
        _ => None,
    }
}

/// Compile a body (a `do`-like sequence): all but the last for effect, the last
/// in `tail` position. Empty → `nil`. A single form returns that node directly.
fn compile_body(heap: &Heap, forms: &[Value], scope: &mut Scope, tail: bool) -> Option<Node> {
    if forms.is_empty() {
        return Some(const_node(heap, Value::Nil));
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
fn compile_let(heap: &Heap, items: &[Value], scope: &mut Scope, tail: bool, rec: bool) -> Option<Node> {
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
                match pair[0] {
                    Value::Sym(s) => slots.push(scope.bind(s)),
                    _ => return None,
                }
            }
            // While compiling the rhs, the letrec slots aren't yet filled — a
            // nested `(fn …)` capturing one would snapshot `nil` (a value snapshot
            // can't do letrec's recursive late-binding), so mark them unsafe to
            // capture; they become safe once we reach the body (all rhs done).
            scope.unsafe_slots.extend_from_slice(&slots);
            for (pair, &slot) in elems.chunks_exact(2).zip(slots.iter()) {
                binds.push((slot, compile_node(heap, pair[1], scope, false)?));
            }
            scope.unsafe_slots.truncate(unsafe_saved);
        } else {
            // let/let*: sequential — a rhs sees only earlier binders.
            for pair in elems.chunks_exact(2) {
                let name = match pair[0] {
                    Value::Sym(s) => s,
                    _ => return None,
                };
                let rhs = compile_node(heap, pair[1], scope, false)?;
                let slot = scope.bind(name);
                binds.push((slot, rhs));
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
    match v {
        Value::Pair(p) => p.region() != value::LOCAL,
        Value::Nil => true, // `(fn)` — degenerate, but stable
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
    Node::Const(frozen)
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
    match v {
        Value::Str(id) => id.region() != value::LOCAL,
        Value::Pair(id) => id.region() != value::LOCAL,
        Value::Vector(id) => id.region() != value::LOCAL,
        Value::Map(id) => id.region() != value::LOCAL,
        Value::Rope(id) => id.region() != value::LOCAL,
        Value::Fn(id) | Value::Macro(id) => id.region() != value::LOCAL,
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
/// frame parent. Returns `None` (defer) if a capture would read a not-yet-finalized
/// `letrec` slot, which a value snapshot can't express.
fn compile_captures(scope: &Scope) -> Option<Vec<(Symbol, Node)>> {
    let mut seen: Vec<Symbol> = Vec::new();
    let mut caps: Vec<(Symbol, Node)> = Vec::new();
    // Current-frame lexicals, innermost binding first (so shadowing wins).
    for &(sym, slot) in scope.names.iter().rev() {
        if seen.contains(&sym) {
            continue;
        }
        seen.push(sym);
        if scope.is_unsafe(slot) {
            return None; // capturing an in-progress letrec binder → defer
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
    Some(caps)
}

/// Compile a `(fn …)`/`(lambda …)` evaluated inside a compiled body to a
/// [`Node::MakeClosure`] (Stage 2c), or `None` (defer) if it can't be VM-built. The
/// closure's *body* is not compiled here — it's compiled lazily by [`compiled_for`]
/// when the closure is first called, keyed by its RUNTIME body handle.
fn compile_make_closure(heap: &Heap, form: Value, scope: &Scope) -> Option<Node> {
    // Post-macroexpand a pattern-param / multi-clause `fn` is already lowered to
    // `match*`; a `fn` reaching here should be plain. Defer defensively otherwise.
    if crate::eval::macros::fn_needs_lowering(heap, form) {
        return None;
    }
    let fn_rest = match form {
        Value::Pair(p) => heap.pair(p).1,
        _ => return None,
    };
    if !fn_rest_is_stable(fn_rest) {
        return None;
    }
    let captures = compile_captures(scope)?;
    Some(Node::MakeClosure {
        fn_rest,
        captures: captures.into_boxed_slice(),
    })
}

/// Compile an already-expanded, already-resolved `form` against the lexical
/// `scope`. `tail` is whether this form is in tail position. Returns `None` when
/// the form uses anything outside the VM's vocabulary (the caller then defers the
/// whole closure to the tree-walker).
fn compile_node(heap: &Heap, form: Value, scope: &mut Scope, tail: bool) -> Option<Node> {
    match form {
        // Self-evaluating literals. `const_node` freezes any embedded heap handle
        // into the immovable RUNTIME region — load-bearing for `Value::Str` (a LOCAL
        // string baked raw into the off-GC-graph AST is the use-after-GC class; see
        // `const_node`), a no-op for the inline/interned atoms.
        Value::Int(_)
        | Value::Float(_)
        | Value::Bool(_)
        | Value::Nil
        | Value::Str(_)
        | Value::Keyword(_) => Some(const_node(heap, form)),

        // A name: a local frame slot if bound, else a global reference.
        Value::Sym(s) => match scope.lookup(s) {
            Some(slot) => Some(Node::Local(slot)),
            None => Some(Node::Global(s)),
        },

        // A combination — a special form we handle (`if`/`do`) or a function call.
        Value::Pair(_) => {
            let items = heap.list_to_vec(form).ok()?;
            let head = *items.first()?;
            if let Value::Sym(h) = head {
                if value::symbol_is(h, kw::IF) {
                    // (if cond then) or (if cond then else)
                    if items.len() != 3 && items.len() != 4 {
                        return None;
                    }
                    let cond = compile_node(heap, items[1], scope, false)?;
                    let then = compile_node(heap, items[2], scope, tail)?;
                    let els = match items.get(3) {
                        Some(&e) => compile_node(heap, e, scope, tail)?,
                        None => const_node(heap, Value::Nil),
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
                    return Some(const_node(heap, items.get(1).copied().unwrap_or(Value::Nil)));
                }
                // `let`/`let*` are sequential; `letrec` pre-allocates all slots.
                if value::symbol_is(h, kw::LET) || value::symbol_is(h, kw::LET_STAR) {
                    return compile_let(heap, &items, scope, tail, false);
                }
                if value::symbol_is(h, kw::LETREC) {
                    return compile_let(heap, &items, scope, tail, true);
                }
                // `(fn …)`/`(lambda …)` inside a compiled body (Stage 2c): build a
                // closure capturing a flat snapshot of the enclosing lexicals.
                if value::symbol_is(h, kw::FN) || value::symbol_is(h, kw::LAMBDA) {
                    return compile_make_closure(heap, form, scope);
                }
                // Any *other* special form (`def`/`quasiquote`/`defmacro`/`binding`)
                // is outside the VM's vocabulary — defer the whole closure to the
                // tree-walker. (`if`/`do`/`let`/`letrec`/`fn`/`quote` are handled
                // above; `and`/`or`/`match`/`match*` aren't special forms — they're
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
            }
            // Function call: compile the callee and every argument (value position).
            let callee = compile_node(heap, head, scope, false)?;
            let mut args = Vec::with_capacity(items.len() - 1);
            for &a in &items[1..] {
                args.push(compile_node(heap, a, scope, false)?);
            }
            Some(Node::Call {
                callee: Box::new(callee),
                args: args.into_boxed_slice(),
                tail,
                // Capture the combination's source position now, while its
                // reader-recorded `form_pos` entry is live (a later collection moves
                // the LOCAL form, but `Pos` is plain data and stays valid).
                pos: heap.form_pos(form),
            })
        }

        // Vector literal — evaluate each element (value position), build fresh.
        Value::Vector(id) => {
            let items = heap.vector(id).to_vec();
            let mut nodes = Vec::with_capacity(items.len());
            for e in items {
                nodes.push(compile_node(heap, e, scope, false)?);
            }
            Some(Node::Vector(nodes.into_boxed_slice()))
        }
        // Map literal — evaluate each key and value (value position), build fresh.
        Value::Map(id) => {
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
fn compile_arm(
    heap: &Heap,
    required: &[Symbol],
    optionals: &[(Symbol, Value)],
    rest: Option<Symbol>,
    body: &[Value],
    enclosing: Vec<Symbol>,
) -> Option<CompiledArm> {
    let nrequired = required.len();
    let noptional = optionals.len();
    let mut scope = Scope::with_params_enclosing(&[], enclosing);
    for &p in required {
        scope.bind(p);
    }
    let mut optional_defaults: Vec<Option<Node>> = Vec::with_capacity(noptional);
    for (name, default) in optionals {
        // A nil default needs no eval (push_frame just leaves the slot nil); a real
        // default compiles in the current scope (required + earlier optionals bound).
        let node = match default {
            Value::Nil => None,
            d => Some(compile_node(heap, *d, &mut scope, false)?),
        };
        optional_defaults.push(node);
        scope.bind(*name);
    }
    if let Some(r) = rest {
        scope.bind(r);
    }
    let body = compile_body(heap, body, &mut scope, true)?;
    Some(CompiledArm {
        nrequired,
        noptional,
        optional_defaults: optional_defaults.into_boxed_slice(),
        rest_slot: rest.map(|_| nrequired + noptional),
        nslots: scope.max,
        body,
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
        let compiled =
            compile_arm(heap, &s.required, &s.optionals, s.rest, &s.body, enclosing.clone())
                .map(Arc::new);
        specs.push(ArmSpec { nrequired, noptional, has_rest, compiled });
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
            match first {
                Value::Pair(p) if p.region() != value::LOCAL => Some(VmCacheKey::LocalBody(p.0)),
                _ => None,
            }
        }
        _ => None, // PRELUDE closures stay on the tree-walker (as before).
    }
}

/// The compiled body for closure `id`, compiling-and-caching on first use. Keyed by
/// [`cache_key`] so a local-capturing closure is found by its RUNTIME body code,
/// not its recycled LOCAL handle. `None` (ineligible) is cached too — but only when
/// the closure *has* a stable key; an unkeyable closure simply defers each call
/// (cheap: a region check + a body-handle peek).
fn compiled_for(heap: &Heap, id: ClosureId) -> Option<Arc<CompiledClosure>> {
    let key = cache_key(heap, id)?;
    if let Some(entry) = heap.vm_cache_get(key) {
        return entry;
    }
    let compiled = compile_closure(heap, id).map(Arc::new);
    heap.vm_cache_put(key, compiled.clone());
    compiled
}

// ===================== executor (Node → value) =====================

/// Resolve a [`Step`] to a value, running a `Tail` to completion. In value
/// positions the step is always `Done` (sub-nodes compile with `tail = false`);
/// this also makes a stray tail safe rather than a panic. A `Tail` carries its own
/// callee env (Stage 2c), so `force` needs no ambient env.
fn force(heap: &mut Heap, step: Step) -> LispResult {
    match step {
        Step::Done(v) => Ok(v),
        Step::Tail { compiled, args, genv } => vm_apply(heap, compiled, &args, genv),
    }
}

/// Execute one node. `frame_base` is the start of this activation's slot region on
/// `Heap::roots`; `genv` is an [`EnvRoot`] for the *current* closure's captured env
/// — read fresh via [`Heap::read_root_env`] wherever it's needed, since a nested
/// call can collect and relocate a movable LOCAL captured env (Stage 2c, R1b).
/// Returns a [`Step`] so a tail call can bubble up to [`vm_apply`]'s trampoline.
fn exec_node(
    heap: &mut Heap,
    node: &Node,
    frame_base: usize,
    genv: EnvRoot,
) -> Result<Step, LispError> {
    match node {
        Node::Const(v) => Ok(Step::Done(*v)),
        // Slot read — depth 0: the callee's own frame. (Deeper depths arrive with
        // the full compiler; the slice only binds params.)
        Node::Local(i) => Ok(Step::Done(heap.root_at(frame_base + i))),
        Node::Global(s) => match heap.env_get(heap.read_root_env(genv), *s) {
            Some(v) => Ok(Step::Done(v)),
            None => Err(crate::eval::unbound_error(heap, *s)),
        },
        Node::If(cond, then, els) => {
            let cs = exec_node(heap, cond, frame_base, genv)?;
            let c = force(heap, cs)?;
            if crate::eval::truthy(c) {
                exec_node(heap, then, frame_base, genv)
            } else {
                exec_node(heap, els, frame_base, genv)
            }
        }
        Node::Do(nodes) => {
            if nodes.is_empty() {
                return Ok(Step::Done(Value::Nil));
            }
            let last = nodes.len() - 1;
            for n in &nodes[..last] {
                // for effect — must be a value (compiled tail=false)
                let s = exec_node(heap, n, frame_base, genv)?;
                force(heap, s)?;
            }
            exec_node(heap, &nodes[last], frame_base, genv)
        }
        Node::Vector(elems) => {
            // Evaluate each element, keeping the results on the operand stack so a
            // collection during a later element relocates them in place (mirrors the
            // `Call` arg loop); then build a fresh vector. `save` is truncated on
            // every path, including errors.
            let save = heap.roots_len();
            for e in elems.iter() {
                let step = match exec_node(heap, e, frame_base, genv) {
                    Ok(s) => s,
                    Err(err) => {
                        heap.truncate_roots(save);
                        return Err(err);
                    }
                };
                match force(heap, step) {
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
            Ok(Step::Done(heap.alloc_vector(vals)))
        }
        Node::Map(entries) => {
            // Same operand-stack discipline as `Vector`: each key then value is
            // pushed (so a collection mid-build relocates them), then a fresh map is
            // built from the relocated pairs.
            let save = heap.roots_len();
            for (kn, vn) in entries.iter() {
                for node in [kn, vn] {
                    let step = match exec_node(heap, node, frame_base, genv) {
                        Ok(s) => s,
                        Err(err) => {
                            heap.truncate_roots(save);
                            return Err(err);
                        }
                    };
                    match force(heap, step) {
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
            Ok(Step::Done(heap.map_from_pairs(pairs)))
        }
        Node::LetBind { binds, body } => {
            // Evaluate each rhs and write it into its (pre-allocated) frame slot,
            // in order. A binding's rhs eval can collect — the frame slots live on
            // `Heap::roots`, relocated in place, so `frame_base + slot` stays valid.
            for (slot, rhs) in binds.iter() {
                let s = exec_node(heap, rhs, frame_base, genv)?;
                let v = force(heap, s)?;
                heap.set_root_at(frame_base + slot, v);
            }
            // Body is tail-propagated (its tail call bubbles up to the trampoline).
            exec_node(heap, body, frame_base, genv)
        }
        Node::MakeClosure { fn_rest, captures } => {
            // Build the captured env: a flat snapshot of the enclosing lexicals
            // (parent = the process global, so true globals + dynamics still resolve
            // live and late-bound). No `captures` source is a call, so evaluating
            // them runs no safepoint — the fresh `frame` and the (immovable) node
            // fields stay valid until `make_closure` consumes them below. With no
            // captures the closure is global-capturing (`env == None`).
            let env = if captures.is_empty() {
                heap.global()
            } else {
                let frame = heap.new_env(Some(heap.global()));
                for (name, src) in captures.iter() {
                    let step = exec_node(heap, src, frame_base, genv)?;
                    let v = force(heap, step)?;
                    heap.env_define(frame, *name, v);
                }
                frame
            };
            let cl = crate::eval::make_closure(heap, None, *fn_rest, env)?;
            Ok(Step::Done(cl))
        }
        Node::Call { callee, args, tail, pos } => {
            // Tag an error with this combination's source position if it doesn't
            // already carry one — so the *innermost* failing call wins (mirrors the
            // tree-walker's `or_form_pos`); a sub-call that already tagged itself is
            // left untouched. `None` (a promoted RUNTIME body) is a no-op.
            let pos = *pos;
            let tag = |e: LispError| match pos {
                Some(p) => e.or_pos(p),
                None => e,
            };
            // Evaluate the callee, then each argument, keeping them on the operand
            // stack so a collection during a later argument's eval relocates them in
            // place (mirrors `eval::eval_arguments`). `save` is this call's region;
            // it is always truncated back, including on the error path.
            let cs = exec_node(heap, callee, frame_base, genv).map_err(|e| tag(e))?;
            let cv = force(heap, cs).map_err(|e| tag(e))?;
            let save = heap.roots_len();
            heap.push_root(cv);
            for a in args.iter() {
                let step = match exec_node(heap, a, frame_base, genv) {
                    Ok(s) => s,
                    Err(e) => {
                        heap.truncate_roots(save);
                        return Err(tag(e));
                    }
                };
                match force(heap, step) {
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
            // The *current* env (read fresh post-collection) is what a native callee
            // runs in; a VM-eligible closure callee instead runs in its own captured
            // env, which `dispatch` reads off the closure.
            let cur_env = heap.read_root_env(genv);
            let result = dispatch(heap, callee_v, argv, *tail, cur_env);
            heap.truncate_roots(save);
            result.map_err(|e| tag(e))
        }
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
    // Thin-wrapper passthrough redirect (ADR-069), mirroring `eval`'s `'dispatch`
    // loop: a pure pass-through prelude op (`(< n 2)` → `<` whose 2-arg arm is
    // `(%lt n 2)`, etc.) redirects straight to its inner `%native` on remapped
    // args — so the hot loop reaches `call_native` directly instead of re-entering
    // `apply_closure` (a frame alloc + param binds + a body eval) for every
    // arithmetic/comparison op. Late-binding safe: it reads the *live* closure and
    // re-resolves the inner head each call (a symbol lookup — no GC, so `cur_argv`
    // stays valid). Looped for chained passthroughs.
    loop {
        let id = match cur_callee {
            Value::Fn(id) => id,
            _ => break,
        };
        let Some((head, map)) = crate::eval::passthrough_arm(heap, id, cur_argv.len()) else {
            break;
        };
        let cl_env = heap.closure(id).env.unwrap_or_else(|| heap.global());
        let inner = match head {
            Value::Sym(s) => heap.env_get(cl_env, s),
            other => Some(other),
        };
        let Some(inner) = inner else { break };
        if !matches!(inner, Value::Fn(_) | Value::Native(_)) {
            break;
        }
        let mut next: SmallVec<[Value; 4]> = SmallVec::with_capacity(map.len());
        for &i in &map {
            next.push(cur_argv[i]);
        }
        // The elided inner call would have been its own reduction — count it (and
        // honour the deadline) so a passthrough-heavy / self-passthrough loop keeps
        // preemption fairness and can't escape the watchdog (the bug eval hit).
        crate::process::tick();
        if crate::process::deadline_exceeded() {
            return Err(LispError::runtime(
                "evaluation exceeded its time limit (MCP tool watchdog)",
            ));
        }
        cur_callee = inner;
        cur_argv = next;
    }
    // A VM-eligible closure of matching arity runs on the VM (or yields a tail
    // call for the trampoline); a native or non-passthrough/ineligible callee goes
    // to the tree-walker via `eval::apply` (which is just `call_native` for a
    // native — cheap).
    if let Value::Fn(id) = cur_callee {
        if let Some(cc) = compiled_for(heap, id) {
            if let Some(arm) = cc.arm_for(cur_argv.len()) {
                let arm = Arc::clone(arm);
                // Run the callee in *its own* captured env (Stage 2c): a
                // global-capturing closure (`env == None`) resolves to the process
                // global as before, while a local-capturing one resolves its free
                // vars in the env it closed over. `genv` (the caller's env) is only
                // for natives below.
                let callee_env = heap.closure(id).env.unwrap_or_else(|| heap.global());
                if tail {
                    return Ok(Step::Tail { compiled: arm, args: cur_argv, genv: callee_env });
                }
                return Ok(Step::Done(vm_apply(heap, arm, &cur_argv, callee_env)?));
            }
        }
    }
    Ok(Step::Done(crate::eval::apply(heap, cur_callee, &cur_argv, genv)?))
}

/// Push a fresh activation frame for `arm` onto `Heap::roots`: required args, then
/// `&optional` slots (the provided arg, or nil if missing), then the `&` rest list
/// (the trailing args conased into a fresh list), then nil for the `let`/`letrec`
/// binders — `nslots` values total. Selection guarantees `args.len() >= nrequired`.
/// No eval runs here (the rest is a plain `list_from_slice`), so no collection can
/// happen between reading `args` and rooting the slots.
fn push_frame(
    heap: &mut Heap,
    arm: &CompiledArm,
    args: &[Value],
    genv: EnvRoot,
) -> Result<(), LispError> {
    let base = heap.roots_len();
    // Pre-allocate the whole frame as nil: every slot (params, optionals, rest, and
    // the body's `let`/`letrec` binders) must exist before anything writes it via
    // `set_root_at` — including a real `&optional` default whose body may bind its
    // own `let` slots.
    for _ in 0..arm.nslots {
        heap.push_root(Value::Nil);
    }
    // Consume ALL provided args into their (now-rooted) slots FIRST, before any
    // default is evaluated: a default's eval can collect, which would strand the
    // still-live `args` slice (LOCAL handles) if it were read afterwards.
    for i in 0..arm.nrequired {
        heap.set_root_at(base + i, args.get(i).copied().unwrap_or(Value::Nil));
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
    // Missing optionals take their default, left-to-right (so a later default sees an
    // earlier one). `None` is a nil-default — the slot is already nil. A real default
    // evaluates against the frame: earlier params/optionals are filled and rooted;
    // its own slot and later slots are still nil (the compiler bound it after the
    // default, so the default can't name itself).
    for j in provided_opt..arm.noptional {
        if let Some(node) = &arm.optional_defaults[j] {
            let step = exec_node(heap, node, base, genv)?;
            let v = force(heap, step)?;
            heap.set_root_at(base + arm.nrequired + j, v);
        }
    }
    Ok(())
}

/// Run a compiled closure body — the trampoline. `args` become the frame's dense
/// slots (via [`push_frame`]), pushed as a region of `Heap::roots` (so `arena_flip`
/// relocates them); a tail call truncates the frame and rebuilds it, **reusing the
/// region** for O(1) stack (proper TCO). Mirrors `eval`'s per-iteration discipline:
/// a GC safepoint, the soft-memory backstop, reduction-counted preemption, the eval
/// deadline, and the non-tail-recursion stack guard.
fn vm_apply(heap: &mut Heap, compiled0: Arc<CompiledArm>, args: &[Value], genv0: EnvId) -> LispResult {
    // Match `eval`: a GC-block guard (feeds the stack-overflow base) + the stack
    // budget check, so deep *non-tail* VM recursion fails cleanly instead of a
    // SIGSEGV. Tail calls reuse the frame below and never grow the Rust stack.
    let _gc_block = crate::process::GcBlockGuard::enter();
    let probe = 0u8;
    if let Some(used) = crate::process::stack_overflow_check(&probe as *const u8 as usize) {
        return Err(LispError::runtime(format!(
            "recursion too deep: used {used} bytes of stack, over the \
             {}-byte budget (runaway non-tail recursion?)",
            crate::process::stack_budget()
        ))
        .with_code(error_codes::STACK_DEPTH_EXCEEDED)
        .with_hint(
            "rewrite as a tail-recursive loop (proper tail calls are O(1) stack), \
             or raise the budget with BROOD_STACK_BUDGET",
        ));
    }

    // Root the captured env on `env_roots` (Stage 2c): for a global-capturing
    // closure this is the immovable `EnvId::GLOBAL` (kept inline, free), but a
    // local-capturing closure's env is a movable LOCAL frame that a collection at
    // the safepoint — or inside any nested call — would relocate. `root_env` parks
    // it so `arena_flip` relocates it in place; we re-read the live handle after
    // every collection via the `EnvRoot`. A tail call into a *different* closure
    // re-roots that callee's env here.
    let env_base = heap.env_roots_len();
    let mut genv = heap.root_env(genv0);

    // Build the frame (required / optional / rest / nil-filled binders), evaluating
    // any real `&optional` default for a missing arg. The whole region lives on
    // `Heap::roots`, so `collect` relocates it in place.
    let base = heap.roots_len();
    if let Err(e) = push_frame(heap, &compiled0, args, genv) {
        heap.truncate_roots(base);
        heap.truncate_env_roots(env_base);
        return Err(e);
    }
    let mut compiled = compiled0;
    loop {
        // GC safepoint — the frame slots live on `Heap::roots` and the captured env
        // on `Heap::env_roots`, so `collect` relocates both in place; the compiled
        // body itself holds only immovable handles, so no further extra roots.
        if !crate::process::macro_block_active() && heap.gc_due() {
            heap.collect(&mut [], &mut []);
        }
        // Soft-memory backstop (ADR-043) — catchable, never frees/moves.
        if let Some(used) = crate::core::alloc::soft_limit_hit() {
            heap.truncate_roots(base);
            heap.truncate_env_roots(env_base);
            return Err(LispError::runtime(format!(
                "memory limit exceeded: {used} bytes allocated process-wide \
                 exceeds the {}-byte soft limit (raise or unset BROOD_MEM_LIMIT)",
                crate::core::alloc::soft_limit()
            ))
            .with_code(error_codes::MEMORY_LIMIT));
        }
        // Reduction-counted preemption + the eval deadline (the watchdog the
        // passthrough loop once escaped — checked every tail iteration here too).
        crate::process::tick();
        if crate::process::deadline_exceeded() {
            heap.truncate_roots(base);
            heap.truncate_env_roots(env_base);
            return Err(LispError::runtime(
                "evaluation exceeded its time limit (MCP tool watchdog)",
            ));
        }

        match exec_node(heap, &compiled.body, base, genv) {
            Ok(Step::Done(v)) => {
                heap.truncate_roots(base);
                heap.truncate_env_roots(env_base);
                return Ok(v);
            }
            Ok(Step::Tail { compiled: c2, args: a2, genv: g2 }) => {
                // Switch to the tail callee's env FIRST (`g2` is still valid — no
                // collection since `dispatch` read it off the callee closure), and
                // root it before rebuilding the frame, so a real `&optional` default
                // in `c2` both resolves its free vars through `g2` and survives any
                // collection its own eval triggers.
                heap.truncate_env_roots(env_base);
                genv = heap.root_env(g2);
                // Reuse the frame region: drop the old slots and rebuild at `base`
                // for the (possibly different, possibly variadic) tail arm.
                heap.truncate_roots(base);
                if let Err(e) = push_frame(heap, &c2, &a2, genv) {
                    heap.truncate_roots(base);
                    heap.truncate_env_roots(env_base);
                    return Err(e);
                }
                compiled = c2;
            }
            Err(e) => {
                heap.truncate_roots(base);
                heap.truncate_env_roots(env_base);
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
    match compile_node(heap, form, &mut scope, false) {
        Some(node) => {
            // A top-level `let` introduces frame slots too — give the form a frame
            // of `scope.max` nil slots (like a 0-param closure), then tear it down.
            // The top-level env is the (immovable) process global, so `root_env`
            // keeps it inline; rooting it uniformly keeps `exec_node`'s contract.
            let env_base = heap.env_roots_len();
            let genv = heap.root_env(env);
            let base = heap.roots_len();
            for _ in 0..scope.max {
                heap.push_root(Value::Nil);
            }
            let r = exec_node(heap, &node, base, genv).and_then(|s| force(heap, s));
            heap.truncate_roots(base);
            heap.truncate_env_roots(env_base);
            r
        }
        None => crate::eval::eval(heap, form, env),
    }
}
