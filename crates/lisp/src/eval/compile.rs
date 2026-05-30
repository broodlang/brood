//! The compiling execution engine — ADR-076, [`docs/bytecode-vm.md`].
//!
//! A **closure-compiling VM over a lexically-addressed IR**: a form compiles once
//! into a [`Node`] tree run by a trampoline ([`vm_apply`]). The crux is GC: a
//! call's frame slots are a contiguous region of the **existing** `Heap::roots`
//! operand stack, so the moving collector relocates them in place (`arena_flip`'s
//! root walk) with **no new root set** — `Node::Local(i)` reads `root_at(base+i)`.
//!
//! **Stage 1 (the first milestone): a bounded slice.** Only **top-level closures**
//! (captured env `None` → the global env) with a **single exact-arity arm** whose
//! body is built entirely from the core forms below compile and VM-run; everything
//! else *defers to the tree-walker* (`eval::eval`). Macros are already expanded by
//! this point (`eval::macros::compile` ran), so the compiler only handles `if`/`do`
//! specially and treats any other non-special head as a function call. This proves
//! the frame-slots-as-roots mechanism and the `env_get`-name-scan elimination on
//! recursion-heavy code (`fib`, tail loops) before the larger compiler investment.
//!
//! Naming note: [`run`] runs **after** `eval::macros::compile` (macroexpand-all +
//! namespace-resolve), on the already-expanded, already-resolved form.

use smallvec::SmallVec;
use std::sync::Arc;

use crate::core::heap::Heap;
use crate::core::value::{self, ClosureId, EnvId, Symbol, Value};
use crate::error::{error_codes, LispError, LispResult};

/// Is the compiling VM enabled? `BROOD_VM` set in the environment turns it on.
/// **Off by default** — the tree-walker is the engine until the Stage 3 cutover
/// (ADR-076). Read once and cached; the flag can't change mid-run.
pub fn vm_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("BROOD_VM").is_some())
}

/// A compiled IR node (ADR-076). Stage 1 vocabulary — the core forms a top-level
/// arithmetic/recursive body is built from. Anything outside this set makes the
/// whole closure ineligible (it runs on the tree-walker instead), so there is no
/// `Defer` node: a VM-run body is *fully* compiled, which is what lets `exec_node`
/// never need an `EnvId` for locals.
pub enum Node {
    /// A self-evaluating literal (number, bool, nil, string, keyword). Holds an
    /// immovable value (atoms, or a RUNTIME/PRELUDE handle from a promoted body),
    /// so the cached `Node` tree needs no GC rooting.
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
    /// A combination. `tail` marks a tail call (the trampoline reuses the frame
    /// instead of recursing — proper TCO). Non-tail calls recurse via [`vm_apply`].
    Call {
        callee: Box<Node>,
        args: Box<[Node]>,
        tail: bool,
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
}

/// The compiled counterpart of a [`ClosureArm`](crate::core::value::ClosureArm):
/// the dense frame width (`nparams`) and the compiled body. Cached per closure on
/// the heap (`Heap::vm_cache_*`). Immutable and `Send + Sync` (its `Node`s hold
/// only immovable handles + symbols + indices), so it lives behind an `Arc`.
pub struct CompiledArm {
    /// Number of parameters — the call's argv fills slots `0..nparams`.
    pub nparams: usize,
    /// Total frame slots: params plus every `let`/`letrec` binder in the body
    /// (flattened lexical scope). `vm_apply` pushes `nparams` args then nil-fills
    /// to `nslots`.
    pub nslots: usize,
    pub body: Node,
}

/// A compiled closure: the VM-eligible **exact-arity** arms, each `Arc`'d so the
/// trampoline can hold one across a call (Stage 2b — multi-arity). `dispatch`
/// selects by argument count (`nparams`), matching eval's preference for an exact
/// arm. Arms that aren't VM-eligible (variadic — `&optional`/`&` rest — or a
/// non-core body) are simply absent, so a call to such an arity defers to the
/// tree-walker.
pub struct CompiledClosure {
    pub arms: Vec<Arc<CompiledArm>>,
}

impl CompiledClosure {
    /// The compiled arm for a call of `argc` args, if one was VM-compiled. Exact
    /// arms have distinct arities, so this is unambiguous.
    fn arm_for(&self, argc: usize) -> Option<&Arc<CompiledArm>> {
        self.arms.iter().find(|a| a.nparams == argc)
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
    },
}

// ===================== compiler (form → Node) =====================

/// Compile-time lexical scope: `let`/`letrec`/param binders flattened into one
/// activation frame (ADR-076 Stage 2a). Each in-scope name maps to a frame slot;
/// `next` is the next free slot and `max` is the high-water mark (= the arm's
/// `nslots`). Shadowing: `lookup` scans newest-first. `bind` claims a slot;
/// `restore` pops a scope's binders (reusing their slots — safe, the bindings are
/// dead once out of scope).
struct Scope {
    names: Vec<(Symbol, usize)>,
    next: usize,
    max: usize,
}

impl Scope {
    fn new() -> Self {
        Scope { names: Vec::new(), next: 0, max: 0 }
    }
    fn with_params(params: &[Symbol]) -> Self {
        let mut s = Scope::new();
        for &p in params {
            s.bind(p);
        }
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
        return Some(Node::Const(Value::Nil));
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
            for (pair, &slot) in elems.chunks_exact(2).zip(slots.iter()) {
                binds.push((slot, compile_node(heap, pair[1], scope, false)?));
            }
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
    result
}

/// Compile an already-expanded, already-resolved `form` against the lexical
/// `scope`. `tail` is whether this form is in tail position. Returns `None` when
/// the form uses anything outside the VM's vocabulary (the caller then defers the
/// whole closure to the tree-walker).
fn compile_node(heap: &Heap, form: Value, scope: &mut Scope, tail: bool) -> Option<Node> {
    match form {
        // Self-evaluating literals.
        Value::Int(_)
        | Value::Float(_)
        | Value::Bool(_)
        | Value::Nil
        | Value::Str(_)
        | Value::Keyword(_) => Some(Node::Const(form)),

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
                if value::symbol_is(h, "if") {
                    // (if cond then) or (if cond then else)
                    if items.len() != 3 && items.len() != 4 {
                        return None;
                    }
                    let cond = compile_node(heap, items[1], scope, false)?;
                    let then = compile_node(heap, items[2], scope, tail)?;
                    let els = match items.get(3) {
                        Some(&e) => compile_node(heap, e, scope, tail)?,
                        None => Node::Const(Value::Nil),
                    };
                    return Some(Node::If(Box::new(cond), Box::new(then), Box::new(els)));
                }
                if value::symbol_is(h, "do") {
                    return compile_body(heap, &items[1..], scope, tail);
                }
                // `let`/`let*` are sequential; `letrec` pre-allocates all slots.
                if value::symbol_is(h, "let") || value::symbol_is(h, "let*") {
                    return compile_let(heap, &items, scope, tail, false);
                }
                if value::symbol_is(h, "letrec") {
                    return compile_let(heap, &items, scope, tail, true);
                }
                // Any *other* special form (def/fn/quote/quasiquote/and/or/
                // binding/match*/…) is outside the VM's vocabulary — defer the
                // whole closure to the tree-walker.
                if crate::eval::is_special_form(h) {
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
            })
        }

        // Vector/map literals, opaque handles, etc. — not in the slice.
        _ => None,
    }
}

/// Compile a closure's body to a [`CompiledArm`], or `None` if it isn't
/// VM-eligible (multi-arm, has `&optional`/`&` rest, captures a *local* env, or its
/// body uses a non-core form). The slice handles single-arm, exact-arity,
/// global-capturing closures only.
fn compile_closure(heap: &Heap, id: ClosureId) -> Option<CompiledClosure> {
    let cl = heap.closure(id);
    // Only closures with no captured *local* frame — env `None` (the global
    // sentinel) or env == the process global env. A real local capture is out of
    // the slice: its free vars would need that frame, which isn't on the VM stack
    // (Stage 2c).
    if let Some(e) = cl.env {
        if !heap.is_global(e) {
            return None;
        }
    }
    // Snapshot the **exact-arity** arms (params + body); variadic arms
    // (`&optional`/`&` rest) are skipped — a call to that arity defers. Cloning
    // ends the `cl` borrow so `compile_body` can re-borrow the heap.
    let arms_src: Vec<(Vec<Symbol>, Vec<Value>)> = cl
        .arms
        .iter()
        .filter(|a| a.optionals.is_empty() && a.rest.is_none())
        .map(|a| (a.params.clone(), a.body.clone()))
        .collect();
    let mut arms: Vec<Arc<CompiledArm>> = Vec::with_capacity(arms_src.len());
    for (params, body_forms) in arms_src {
        let nparams = params.len();
        // Params occupy slots 0..nparams; `let`/`letrec` binders extend the frame.
        let mut scope = Scope::with_params(&params);
        // A non-core arm body just isn't added — that arity defers, others VM-run.
        if let Some(body) = compile_body(heap, &body_forms, &mut scope, true) {
            arms.push(Arc::new(CompiledArm {
                nparams,
                nslots: scope.max,
                body,
            }));
        }
    }
    if arms.is_empty() {
        None
    } else {
        Some(CompiledClosure { arms })
    }
}

/// The compiled body for closure `id`, compiling-and-caching on first use. Only
/// **RUNTIME** closures (top-level / promoted — stable handle bits) are cached and
/// run; LOCAL closures return `None` (deferred), since the slice doesn't handle
/// captured local frames. `None` is cached too, so an ineligible closure isn't
/// re-analysed on every call.
fn compiled_for(heap: &Heap, id: ClosureId) -> Option<Arc<CompiledClosure>> {
    if id.region() != value::RUNTIME {
        return None;
    }
    let key = id.0;
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
/// this also makes a stray tail safe rather than a panic.
fn force(heap: &mut Heap, step: Step, genv: EnvId) -> LispResult {
    match step {
        Step::Done(v) => Ok(v),
        Step::Tail { compiled, args } => vm_apply(heap, compiled, &args, genv),
    }
}

/// Execute one node. `frame_base` is the start of this activation's slot region on
/// `Heap::roots`; `genv` is the global env for free-name resolution. Returns a
/// [`Step`] so a tail call can bubble up to [`vm_apply`]'s trampoline.
fn exec_node(heap: &mut Heap, node: &Node, frame_base: usize, genv: EnvId) -> Result<Step, LispError> {
    match node {
        Node::Const(v) => Ok(Step::Done(*v)),
        // Slot read — depth 0: the callee's own frame. (Deeper depths arrive with
        // the full compiler; the slice only binds params.)
        Node::Local(i) => Ok(Step::Done(heap.root_at(frame_base + i))),
        Node::Global(s) => match heap.env_get(genv, *s) {
            Some(v) => Ok(Step::Done(v)),
            None => Err(crate::eval::unbound_error(heap, *s)),
        },
        Node::If(cond, then, els) => {
            let cs = exec_node(heap, cond, frame_base, genv)?;
            let c = force(heap, cs, genv)?;
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
                force(heap, s, genv)?;
            }
            exec_node(heap, &nodes[last], frame_base, genv)
        }
        Node::LetBind { binds, body } => {
            // Evaluate each rhs and write it into its (pre-allocated) frame slot,
            // in order. A binding's rhs eval can collect — the frame slots live on
            // `Heap::roots`, relocated in place, so `frame_base + slot` stays valid.
            for (slot, rhs) in binds.iter() {
                let s = exec_node(heap, rhs, frame_base, genv)?;
                let v = force(heap, s, genv)?;
                heap.set_root_at(frame_base + slot, v);
            }
            // Body is tail-propagated (its tail call bubbles up to the trampoline).
            exec_node(heap, body, frame_base, genv)
        }
        Node::Call { callee, args, tail } => {
            // Evaluate the callee, then each argument, keeping them on the operand
            // stack so a collection during a later argument's eval relocates them in
            // place (mirrors `eval::eval_arguments`). `save` is this call's region;
            // it is always truncated back, including on the error path.
            let cs = exec_node(heap, callee, frame_base, genv)?;
            let cv = force(heap, cs, genv)?;
            let save = heap.roots_len();
            heap.push_root(cv);
            for a in args.iter() {
                let step = match exec_node(heap, a, frame_base, genv) {
                    Ok(s) => s,
                    Err(e) => {
                        heap.truncate_roots(save);
                        return Err(e);
                    }
                };
                match force(heap, step, genv) {
                    Ok(v) => heap.push_root(v),
                    Err(e) => {
                        heap.truncate_roots(save);
                        return Err(e);
                    }
                }
            }
            // Re-read post-collection from the (relocated) operand stack.
            let callee_v = heap.root_at(save);
            let mut argv: SmallVec<[Value; 4]> = SmallVec::with_capacity(args.len());
            for k in 0..args.len() {
                argv.push(heap.root_at(save + 1 + k));
            }
            let result = dispatch(heap, callee_v, argv, *tail, genv);
            heap.truncate_roots(save);
            result
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
                if tail {
                    return Ok(Step::Tail { compiled: arm, args: cur_argv });
                }
                return Ok(Step::Done(vm_apply(heap, arm, &cur_argv, genv)?));
            }
        }
    }
    Ok(Step::Done(crate::eval::apply(heap, cur_callee, &cur_argv, genv)?))
}

/// Run a compiled closure body — the trampoline. `args` become the frame's dense
/// slots, pushed as a region of `Heap::roots` (so `arena_flip` relocates them); a
/// tail call truncates the frame and re-pushes the new args, **reusing the frame**
/// for O(1) stack (proper TCO). Mirrors `eval`'s per-iteration discipline: a GC
/// safepoint, the soft-memory backstop, reduction-counted preemption, the eval
/// deadline, and the non-tail-recursion stack guard.
fn vm_apply(heap: &mut Heap, compiled0: Arc<CompiledArm>, args: &[Value], genv: EnvId) -> LispResult {
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

    // Build the frame: `nparams` args fill slots 0..nparams, then nil-fill the
    // `let`/`letrec` binder slots up to `nslots`. The whole region lives on
    // `Heap::roots`, so `collect` relocates it in place.
    let base = heap.roots_len();
    for &a in args {
        heap.push_root(a);
    }
    for _ in args.len()..compiled0.nslots {
        heap.push_root(Value::Nil);
    }
    let mut compiled = compiled0;
    loop {
        // GC safepoint — the frame slots live on `Heap::roots`, so `collect`
        // relocates them in place; the compiled body holds only immovable handles
        // and `genv` is the (immovable) global env, so no extra roots are needed.
        if !crate::process::macro_block_active() && heap.gc_due() {
            heap.collect(&mut [], &mut []);
        }
        // Soft-memory backstop (ADR-043) — catchable, never frees/moves.
        if let Some(used) = crate::core::alloc::soft_limit_hit() {
            heap.truncate_roots(base);
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
            return Err(LispError::runtime(
                "evaluation exceeded its time limit (MCP tool watchdog)",
            ));
        }

        match exec_node(heap, &compiled.body, base, genv) {
            Ok(Step::Done(v)) => {
                heap.truncate_roots(base);
                return Ok(v);
            }
            Ok(Step::Tail { compiled: c2, args: a2 }) => {
                // Reuse the frame: drop the old slots, push the new args + nil-fill
                // to the new arm's slot count at `base`.
                heap.truncate_roots(base);
                for &a in &a2 {
                    heap.push_root(a);
                }
                for _ in a2.len()..c2.nslots {
                    heap.push_root(Value::Nil);
                }
                compiled = c2;
            }
            Err(e) => {
                heap.truncate_roots(base);
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
            let base = heap.roots_len();
            for _ in 0..scope.max {
                heap.push_root(Value::Nil);
            }
            let r = exec_node(heap, &node, base, env).and_then(|s| force(heap, s, env));
            heap.truncate_roots(base);
            r
        }
        None => crate::eval::eval(heap, form, env),
    }
}
