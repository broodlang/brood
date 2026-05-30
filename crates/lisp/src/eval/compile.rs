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
}

/// The compiled counterpart of a [`ClosureArm`](crate::core::value::ClosureArm):
/// the dense frame width (`nparams`) and the compiled body. Cached per closure on
/// the heap (`Heap::vm_cache_*`). Immutable and `Send + Sync` (its `Node`s hold
/// only immovable handles + symbols + indices), so it lives behind an `Arc`.
pub struct CompiledArm {
    pub nparams: usize,
    pub body: Node,
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

/// Compile an already-expanded, already-resolved `form` against the lexical
/// `scope` (the in-scope local names, innermost last — depth 0 only in the slice).
/// `tail` is whether this form is in tail position. Returns `None` when the form
/// uses anything outside the Stage-1 core vocabulary (the caller then defers the
/// whole closure to the tree-walker).
fn compile_node(heap: &Heap, form: Value, scope: &[Symbol], tail: bool) -> Option<Node> {
    match form {
        // Self-evaluating literals.
        Value::Int(_)
        | Value::Float(_)
        | Value::Bool(_)
        | Value::Nil
        | Value::Str(_)
        | Value::Keyword(_) => Some(Node::Const(form)),

        // A name: a local frame slot if bound, else a global reference.
        Value::Sym(s) => match scope.iter().position(|&p| p == s) {
            Some(idx) => Some(Node::Local(idx)),
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
                    let forms = &items[1..];
                    if forms.is_empty() {
                        return Some(Node::Const(Value::Nil));
                    }
                    let n = forms.len();
                    let mut nodes = Vec::with_capacity(n);
                    for (i, &f) in forms.iter().enumerate() {
                        nodes.push(compile_node(heap, f, scope, tail && i + 1 == n)?);
                    }
                    return Some(Node::Do(nodes.into_boxed_slice()));
                }
                // Any *other* special form (def/let/fn/letrec/quote/quasiquote/
                // and/or/binding/…) is outside the slice — defer the whole closure.
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
fn compile_closure(heap: &Heap, id: ClosureId) -> Option<CompiledArm> {
    let cl = heap.closure(id);
    if cl.arms.len() != 1 {
        return None;
    }
    // Only closures with no captured *local* frame — env `None` (the global
    // sentinel) or env == the process global env. A real local capture is out of
    // the slice: its free vars would need that frame, which isn't on the VM stack.
    if let Some(e) = cl.env {
        if !heap.is_global(e) {
            return None;
        }
    }
    let arm = &cl.arms[0];
    if !arm.optionals.is_empty() || arm.rest.is_some() {
        return None;
    }
    let params = arm.params.clone();
    let body_forms = arm.body.clone();
    // `cl`/`arm` borrows end here; `compile_node` re-borrows the heap immutably.
    let nparams = params.len();
    if body_forms.is_empty() {
        return Some(CompiledArm {
            nparams,
            body: Node::Const(Value::Nil),
        });
    }
    let n = body_forms.len();
    let mut nodes = Vec::with_capacity(n);
    for (i, &f) in body_forms.iter().enumerate() {
        nodes.push(compile_node(heap, f, &params, i + 1 == n)?);
    }
    let body = if nodes.len() == 1 {
        nodes.pop().unwrap()
    } else {
        Node::Do(nodes.into_boxed_slice())
    };
    Some(CompiledArm { nparams, body })
}

/// The compiled body for closure `id`, compiling-and-caching on first use. Only
/// **RUNTIME** closures (top-level / promoted — stable handle bits) are cached and
/// run; LOCAL closures return `None` (deferred), since the slice doesn't handle
/// captured local frames. `None` is cached too, so an ineligible closure isn't
/// re-analysed on every call.
fn compiled_for(heap: &Heap, id: ClosureId) -> Option<Arc<CompiledArm>> {
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
        Node::Global(s) => heap
            .env_get(genv, *s)
            .map(Step::Done)
            .ok_or_else(|| crate::eval::unbound_error(*s)),
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
        if let Some(compiled) = compiled_for(heap, id) {
            if compiled.nparams == cur_argv.len() {
                if tail {
                    return Ok(Step::Tail { compiled, args: cur_argv });
                }
                return Ok(Step::Done(vm_apply(heap, compiled, &cur_argv, genv)?));
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

    let base = heap.roots_len();
    for &a in args {
        heap.push_root(a);
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
                // Reuse the frame: drop the old slots, push the new args at `base`.
                heap.truncate_roots(base);
                for &a in &a2 {
                    heap.push_root(a);
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
    match compile_node(heap, form, &[], false) {
        Some(node) => {
            let base = heap.roots_len();
            let step = exec_node(heap, &node, base, env)?;
            force(heap, step, env)
        }
        None => crate::eval::eval(heap, form, env),
    }
}
