//! Outer VM trampoline + program-run bodies (extracted from mod.rs).
use super::*;

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
            // Resolve ONCE. This was a guard calling `compiled_arm_for` and a body
            // re-calling it under an `expect` — two independent resolutions of the same
            // closure, which ran the cold compile path twice and, worse, could disagree:
            // a peer process advancing `free_epoch` between them clears this process's
            // `vm_cache`, so `is_some()` then `expect` was a live panic vector on a
            // process body that compiles perfectly well.
            ValueRef::Fn(id) => match compiled_arm_for(heap, id, 0) {
                Some(arm) => {
                    let cenv = heap.closure(id).env.unwrap_or_else(|| heap.global());
                    vm_run_bc(heap, arm, &[], cenv, None, true)
                }
                // Not VM-eligible: the tree-walker runs it (see the comment above).
                None => crate::eval::apply(heap, body, &[], EnvId::GLOBAL).map(VmOutcome::Done),
            },
            _ => Err(LispError::type_err("process body must be a function")),
        },
    }
}

// ---- the top-level program, run as a green process (ADR-135) --------------------

/// The one-shot slot the **root program process** publishes its terminal outcome to,
/// and the main (root) thread blocks on. `Ok(())` = every top-level form ran to
/// completion; `Err(e)` = a top-level form raised — the **structured** error (file/pos
/// already attached), so the CLI can render the full report (caret, hint, call trace)
/// instead of a pre-flattened string. The error's `payload` is stripped before
/// publishing: a `Value` is a handle into the program process's heap, which is dead by
/// the time the root thread reads the slot. A program that never returns (a top-level
/// server that suspends forever in `receive`) simply never publishes — the root thread
/// blocks indefinitely, exactly as it did when it ran the program itself.
pub struct ProgramExit {
    slot: std::sync::Mutex<Option<Result<(), LispError>>>,
    cv: std::sync::Condvar,
    /// The printed last-form value (wasm only). A `Value` can't cross the process-heap
    /// boundary (it dies with the program's heap), so the driver renders it to a string
    /// while the heap is alive; the playground reads it here. Native `run_program` discards
    /// the value, so this stays unset there.
    #[cfg(target_arch = "wasm32")]
    result: std::sync::Mutex<Option<String>>,
}

impl ProgramExit {
    pub fn new() -> Arc<Self> {
        Arc::new(ProgramExit {
            slot: std::sync::Mutex::new(None),
            cv: std::sync::Condvar::new(),
            #[cfg(target_arch = "wasm32")]
            result: std::sync::Mutex::new(None),
        })
    }

    fn publish(&self, r: Result<(), LispError>) {
        let mut g = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        if g.is_none() {
            *g = Some(r);
            self.cv.notify_all();
        }
    }

    /// Store the program's printed result (wasm; called by the driver at completion).
    #[cfg(target_arch = "wasm32")]
    pub fn set_result(&self, s: Option<String>) {
        *self.result.lock().unwrap_or_else(|e| e.into_inner()) = s;
    }

    /// Take the program's printed result (wasm; the playground reads it after `wait`).
    #[cfg(target_arch = "wasm32")]
    pub fn take_result(&self) -> Option<String> {
        self.result.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    /// Block the calling (root) thread until the program publishes its outcome. On wasm
    /// there are no worker threads, so instead of parking we drive the run queue on this
    /// thread (`pump_until_quiescent`) — which runs the program and everything it spawns to
    /// completion — then read the slot. If the pump goes quiescent without a publish the
    /// program deadlocked in a top-level `receive` (no sender will ever wake it); return
    /// `:normal` rather than hang forever, as there is no other thread to make progress.
    pub fn wait(&self) -> Result<(), LispError> {
        #[cfg(target_arch = "wasm32")]
        {
            crate::process::pump_until_quiescent();
            let g = self.slot.lock().unwrap_or_else(|e| e.into_inner());
            return g.clone().unwrap_or(Ok(()));
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut g = self.slot.lock().unwrap_or_else(|e| e.into_inner());
            while g.is_none() {
                g = self.cv.wait(g).unwrap_or_else(|e| e.into_inner());
            }
            g.clone().expect("published above")
        }
    }
}

/// The driving state of a [`Process`](crate::process) whose body is a whole top-level
/// program (ADR-135). The forms are pinned on the process heap's root stack
/// (`[root_base, root_base+count)`), re-fetched by index each step so a collection
/// between forms relocates them safely (as `load`/`eval_source` root their tail). The
/// namespace/forward-ref frame is installed once into the heap (which travels with the
/// process across suspends), so it persists without being re-derived.
pub struct ProgramState {
    root_base: usize,
    count: usize,
    positions: Vec<Pos>,
    cursor: usize,
    file: Option<String>,
    started: bool,
    /// When the current form is a `(def name rhs)` whose `rhs` is being run on the capture
    /// path (so it can `receive`), this holds `name` — the value the finished RHS is bound
    /// to. Persists across a suspend (the RHS parked mid-`receive`), so the resume knows to
    /// bind before advancing. `None` for a non-`def` form (run whole).
    def_name: Option<Value>,
    exit: Arc<ProgramExit>,
    /// The printed value of the most recently finished top-level form (wasm only) — the
    /// program's result once the last form completes, rendered while its heap is alive.
    #[cfg(target_arch = "wasm32")]
    last_repr: Option<String>,
}

impl ProgramState {
    /// Build the driver for a program whose `forms` were already read (positioned) into
    /// `heap` and pushed as roots at `root_base` (contiguously, in order).
    pub fn new(
        root_base: usize,
        positions: Vec<Pos>,
        file: Option<String>,
        exit: Arc<ProgramExit>,
    ) -> Self {
        ProgramState {
            root_base,
            count: positions.len(),
            positions,
            cursor: 0,
            file,
            started: false,
            def_name: None,
            exit,
            #[cfg(target_arch = "wasm32")]
            last_repr: None,
        }
    }
}

/// Wrap a (RUNTIME-promoted) top-level `form` as a 0-arg thunk `(fn () form)` so it runs
/// through the capturing body driver ([`run_process_body`]). The thunk handle is LOCAL,
/// but its body form is a RUNTIME pair, so [`cache_key`] keys it (by the body handle) and
/// [`compiled_arm_for`] compiles it onto the VM capture path — the property that makes a
/// `receive` anywhere inside the form park-and-capture instead of blocking the worker.
pub(crate) fn build_program_thunk(heap: &mut Heap, form: Value) -> Value {
    let id = heap.alloc_closure(crate::core::value::Closure::single(
        None,
        Vec::new(),
        Vec::new(),
        None,
        vec![form],
        None,
        None, // global scope — top-level forms capture nothing
    ));
    Value::func(id)
}

/// Drive a program process one quantum (ADR-135): finish the in-flight form's resumed
/// continuation, then compile+run each remaining top-level form in order. A `Suspended`/
/// `Preempted` from any form is returned **unchanged** — the loop unwinds, the cursor
/// lives in `prog`, and the next entry resumes mid-form via the stored continuation — so
/// a top-level `receive` park-captures like any green process. A form error is **caught**
/// and published to the exit slot, and the process retires `:normal` (no spurious
/// "process died" from the scheduler); the root thread reads the real outcome.
pub(crate) fn run_program_body(
    heap: &mut Heap,
    prog: &mut ProgramState,
    resume: Option<Suspended>,
) -> Result<VmOutcome, LispError> {
    let root = heap.global();
    // First entry: install the file + root namespace + forward-ref pre-scan into the heap
    // (persists across suspends, since the heap travels with the process).
    if !prog.started {
        prog.started = true;
        heap.set_current_file(prog.file.clone());
        heap.set_compile_ns(None);
        let forms: Vec<Value> = (0..prog.count)
            .map(|i| heap.root_at(prog.root_base + i))
            .collect();
        // Region model (ADR-223): install the per-module pre-scan; the active set starts
        // empty and each `defmodule`'s `%in-ns` activates its region (no form resolves
        // before its region opens, since resolution is a no-op at root).
        let by_module = if crate::eval::macros::file_opens_ns(heap, &forms) {
            crate::eval::macros::scan_regions(heap, &forms)
        } else {
            std::collections::HashMap::new()
        };
        heap.set_ns_known_names(std::collections::HashSet::new());
        heap.set_ns_known_by_module(by_module);
        heap.set_imports(std::collections::HashMap::new());
    }

    // Resume the form the process was suspended/preempted inside, if any.
    if let Some(s) = resume {
        let pos = prog.positions[prog.cursor];
        match run_process_body(heap, Value::nil(), Some(s)).map_err(|e| e.or_pos(pos)) {
            Ok(VmOutcome::Done(v)) => {
                if let Err(e) = prog.finish_form(heap, v).map_err(|e| e.or_pos(pos)) {
                    return Ok(prog.crash(e));
                }
            }
            Ok(other) => return Ok(other),
            Err(e) => return Ok(prog.crash(e)),
        }
    }

    // Run each remaining form fresh: compile it (so an earlier `defmacro` is already in
    // effect), then run its receive-bearing part on the capture driver.
    while prog.cursor < prog.count {
        let pos = prog.positions[prog.cursor];
        let form = heap.root_at(prog.root_base + prog.cursor);
        heap.note_definition(form, pos);
        let step = (|| -> Result<VmOutcome, LispError> {
            let expanded = crate::eval::macros::compile(heap, form, root)?;
            heap.note_definition(expanded, pos);
            // `BROOD_VM=0` — the tree-walking debug engine. ADR-135 made the top
            // level a capture-mode process driven by the VM, which silently
            // swallowed the flag for `brood file.blsp` (discovered 2026-07-16 while
            // building the engine-differential fuzzer: the "tree-walker" leg was
            // really the VM). Honor it here: run the whole form synchronously on
            // the tree-walker — `def` is its special form, and a top-level
            // `receive` blocks the worker (the documented tree-walker behavior;
            // it's a debug engine, not the production path).
            if tier_ceiling() == Tier::TreeWalk {
                prog.def_name = None;
                return Ok(VmOutcome::Done(crate::eval::eval(heap, expanded, root)?));
            }
            // A top-level `(def name rhs)` runs its RHS on the capture path and binds after
            // (`def` is a special form the VM won't body-compile, so wrapping the whole form
            // would defer to the tree-walker, whose `receive` blocks the worker instead of
            // parking — the very thing this process exists to avoid). Any other form (a bare
            // call, a `let`, an expression) runs whole. `def_name` records the pending bind
            // so it survives a suspend mid-RHS.
            let body = match def_rhs(heap, expanded) {
                Some((name, rhs)) => {
                    prog.def_name = Some(name);
                    rhs
                }
                None => {
                    prog.def_name = None;
                    expanded
                }
            };
            let promoted = heap.promote(body);
            let thunk = build_program_thunk(heap, promoted);
            run_process_body(heap, thunk, None)
        })()
        .map_err(|e| e.or_pos(pos));
        match step {
            Ok(VmOutcome::Done(v)) => {
                if let Err(e) = prog.finish_form(heap, v).map_err(|e| e.or_pos(pos)) {
                    return Ok(prog.crash(e));
                }
            }
            Ok(other) => return Ok(other), // suspended / preempted mid-form (def_name persists)
            Err(e) => return Ok(prog.crash(e)),
        }
    }

    #[cfg(target_arch = "wasm32")]
    prog.exit.set_result(prog.last_repr.take());
    prog.exit.publish(Ok(()));
    Ok(VmOutcome::Done(Value::nil()))
}

impl ProgramState {
    /// A form finished with value `v`: if it was a `(def name rhs)` whose RHS we ran, bind
    /// `name` to `v` now (reusing the full `def` semantics — naming, promote-to-shared,
    /// reload diagnostics); either way advance to the next form.
    fn finish_form(&mut self, heap: &mut Heap, v: Value) -> Result<(), LispError> {
        // Render the form's value while its heap is alive (wasm). Overwritten each form, so
        // after the last one it holds the program's result for the playground to print.
        #[cfg(target_arch = "wasm32")]
        {
            self.last_repr = Some(crate::syntax::printer::print(heap, v));
        }
        if let Some(name) = self.def_name.take() {
            bind_def(heap, name, v)?;
        }
        self.cursor += 1;
        Ok(())
    }

    /// A top-level form raised: publish the structured error to the root thread and
    /// retire the process `:normal` (it handled its own crash), so the scheduler prints
    /// nothing.
    fn crash(&self, e: LispError) -> VmOutcome {
        // Attach the file to the error's own field (innermost `load` wins, a no-op if
        // already set) so `located()` renders the canonical `file:LINE:COL: msg` — NOT
        // string-prepend `file: ` onto an already-located `LINE:COL: msg`, which emits a
        // stray space (`file: LINE:COL:`) that diverges from the tree-walker's editor-
        // parseable form. (Found by the differential fuzzer, 2026-07-16.)
        let mut e = match &self.file {
            Some(f) => e.or_file(f.clone()),
            None => e,
        };
        // The payload `Value` is a handle into THIS process's heap — dead once the
        // root thread reads the slot. Everything else (message, pos, hint, trace)
        // is plain data and crosses intact.
        e.payload = None;
        self.exit.publish(Err(e));
        VmOutcome::Done(Value::nil())
    }
}

pub(crate) fn vm_run_bc(
    heap: &mut Heap,
    arm0: Arc<ArmHandle>,
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
    let fresh = match resume {
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
            heap.set_ic_bases(cur.ic_bases);
            #[cfg(feature = "jit")]
            {
                cur_back_edges = cur.back_edges;
            }
            false
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
            {
                let b = heap.vm_arm_block(&cur_arm);
                heap.set_ic_bases(b);
            }
            if let Err(mut e) = push_frame(heap, &cur_arm, args0, cur_env) {
                heap.truncate_roots(entry_roots);
                heap.truncate_env_roots(entry_env);
                heap.live_arm_truncate(entry_arms);
                attach_vm_trace(&mut e, &cur_arm, &[]);
                return Err(e);
            }
            cur_ip = 0usize;
            #[cfg(feature = "jit")]
            {
                cur_back_edges = 0;
            }
            true
        }
    };
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
    //
    // A RESUME tiers too when it re-enters at ip 0. ip 0 always means "run the whole
    // arm against the current slot state": a preempted self-tail loop parks with its
    // frame already reset (carried slots in place, ip 0), so handing it straight back
    // to its native code is exactly resuming the loop. Without this, every scheduler
    // preempt of a native loop resumed INTERPRETED and only re-tiered at the next
    // 256th back-edge — sieve's `mark` preempted ~1030 times and paid ~256 interpreted
    // iterations each (~260k, ~20% of the row). A mid-`receive` resume rewinds to its
    // `Inst::Call` at a nonzero ip and still (correctly) never tiers.
    #[cfg(feature = "jit")]
    let mut try_jit = fresh || cur_ip == 0;
    #[cfg(not(feature = "jit"))]
    let _ = fresh; // silence unused warning when the JIT is off

    // Sampling profiler (observability timing tier): the epoch this driver last
    // sampled at. Loop-local — a resume simply re-samples on its next epoch tick.
    let mut profiled_epoch: u64 = 0;

    loop {
        // Profiler sample: when armed and the ticker's epoch moved, record this
        // driver's named-frame stack (cur + pending callers, innermost first).
        // Off (the default): one relaxed bool load per frame boundary.
        if crate::profile::armed() {
            let ep = crate::profile::epoch();
            if ep != profiled_epoch {
                profiled_epoch = ep;
                let mut stack: Vec<value::Symbol> = Vec::with_capacity(frames.len() + 1);
                if let Some(n) = cur_arm.fn_name {
                    stack.push(n);
                }
                for f in frames.iter().rev() {
                    if let Some(n) = f.arm.fn_name {
                        stack.push(n);
                    }
                }
                crate::profile::record(&stack);
            }
        }
        // Per-iteration safepoint / preemption / deadline — relocates every frame's
        // slots and env in place (all on `Heap::roots`/`env_roots`). Mirrors the
        // `Node` trampoline's loop top.
        if !crate::process::macro_block_active() && heap.gc_due() {
            heap.collect(&mut [], &mut []);
        }
        // RUNTIME-region collection (ADR-091): the VM-engine counterpart of the
        // tree-walker's safepoint. Once churn crosses the threshold, compact the shared
        // code region (single-process) or — on a shared runtime with the multi-process
        // collector armed — advance the age/migrate/drain/free state machine. Every
        // live RUNTIME handle at this frame boundary is already on `heap.roots`/
        // `env_roots`/`live_vm_arms` (which the compactor rewrites), so no extra roots
        // are needed. Gated on `rt_dirty`/`drain_active` (see below) and the same
        // macro-block guard as the LOCAL collect.
        // Gate the (costly) `rt_gc_due` probe — an `ArcSwap` load + closure count —
        // on one cheap relaxed load: run it only when the RUNTIME region has grown
        // since the last check (`rt_dirty`, set at the sole mint point). A def-free hot
        // loop (`fib`, `reduce`, `apply`) trips it never and pays nothing; a mint re-arms
        // `rt_dirty` so a collect is at most one frame late.
        //
        // NOTE: this gate deliberately does **not** also fire on `drain_active`. A
        // lingering drain (a long-lived process pins the generation, so it never frees)
        // would otherwise force this whole block — the `cur_code()` `ArcSwap` load — on
        // *every* frame for the rest of the run, which is the multigen `rounds`-shape
        // overhead. The drain is instead advanced/freed on `rt_dirty` (mint) frames,
        // which occur whenever code churns; the separate drain self-report below
        // still runs every frame so acks stay current, and a completable drain frees at
        // the next mint (retaining one extra generation over a fully idle interval is
        // bounded and harmless). That report is O(1) per frame only because
        // `runtime_gen_referenced_private` short-circuits a cached verdict — the probe
        // itself is O(this process's roots), i.e. O(recursion depth). This comment used to
        // assert a flat "O(1) drain self-report", which is what let KI-14 hide: a deep
        // process was re-walking 1.7M roots per report.
        if heap.rt_dirty() && !crate::process::macro_block_active() {
            heap.rt_dirty_clear();
            if heap.rt_gc_due() {
                heap.maybe_runtime_collect(&mut [], &mut []);
            }
        }
        // RUNTIME-drain cooperative report (ADR-091 Stage 3c): the VM-engine
        // counterpart of the tree-walker's safepoint report. While a generation
        // drain is armed, this process reports whether it still references the
        // draining generation. Read-only probe; the hot path is one atomic load.
        if heap.drain_active() {
            crate::process::report_drain_liveness(heap);
        }
        if let Some(used) = crate::core::alloc::soft_limit_hit() {
            unwind(heap);
            let mut e = crate::eval::memory_limit_error(used);
            attach_vm_trace(&mut e, &cur_arm, &frames);
            return Err(e);
        }
        // Per-process heap limit (`(process-flag :max-heap n)`): the sticky flag
        // the loop-top collection armed raises here — catchable, and it kills
        // just this process, with the trace showing where the data was live.
        if let Some(live) = heap.take_proc_limit_hit() {
            unwind(heap);
            let limit = heap.proc_mem_limit().unwrap_or(0);
            let mut e = crate::eval::proc_memory_limit_error(live, limit);
            attach_vm_trace(&mut e, &cur_arm, &frames);
            return Err(e);
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
                    ic_bases: heap.ic_bases(),
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
        } else if crate::process::tick_reporting_hard_kill() {
            // Nested (non-capture) run: no `Killed` outcome can cross the native
            // boundary, but a hard kill only needs to STOP — unwind with the
            // untrappable kill signal; the top-level driver's `Err` arm below converts
            // it to `VmOutcome::Killed`. Same contract as the tree-walker's loop top.
            unwind(heap);
            return Err(crate::error::LispError::kill_signal());
        }
        if crate::process::deadline_exceeded() {
            unwind(heap);
            let mut e = crate::eval::deadline_error();
            attach_vm_trace(&mut e, &cur_arm, &frames);
            return Err(e);
        }

        // Either run the arm natively (if it's flagged for a tier check) or interpret it.
        // Both yield a `Result<ChunkExit, _>` handled uniformly below.
        let exit =
            {
                #[cfg(feature = "jit")]
                {
                    if try_jit {
                        try_jit = false;
                        // Spinning-loop escape hatch (see JIT_QUEUED_SYNC_EDGES): a
                        // self-tail loop that exited here after spinning ~2k edges
                        // against a still-QUEUED arm compiles it right now, on this
                        // thread, instead of interpreting until the background
                        // compiler gets to it.
                        if cur_back_edges != 0
                            && cur_back_edges.is_multiple_of(JIT_QUEUED_SYNC_EDGES)
                            && cur_arm.jit_code.load(std::sync::atomic::Ordering::Acquire)
                                == crate::jit::QUEUED
                        {
                            jit_compile_now(heap, cur_arm.arc(), cur_base);
                        }
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
                        // The size this frame is BUILT to, captured here. The deopt-resume
                        // helpers must be told it rather than re-deriving it from
                        // `inline_installed`, which `jit_tier` flips below — see
                        // `jit_frame_layout`.
                        let frame_nslots = if inlined_active {
                            cur_arm.inline_nslots
                        } else {
                            cur_arm.nslots
                        };
                        // Clean frame state `jit_tier` runs against: slots set up, operand
                        // stack empty. A deopt/preempt re-run (`exec_chunk` from ip 0) below
                        // assumes roots return to exactly here.
                        let pre_roots = heap.roots_len();
                        // `jit_tier_in_frame`, not `jit_tier`: the frame above was sized from
                        // `inline_installed`, and `jit_tier` re-loads `jit_code` — a peer
                        // process can swap the inlined body in between, and running it against
                        // this (small) frame writes past the frame top. Telling it the size the
                        // frame was BUILT to lets it decline instead. See its doc (KI-48 family).
                        let jit_outcome =
                            jit_tier_in_frame(cur_arm.arc(), heap, cur_base, cur_env, frame_nslots);
                        // The deopt-resume decision, taken ONCE and taken HERE, before the
                        // frame is resized below. Reading the journal twice — once to decide
                        // the resize, once to resume — is wrong two ways: the second read
                        // comes from an already-truncated frame and indexes past the root
                        // stack, and the two reads can disagree about which engine ran the
                        // frame. (That was a real out-of-bounds `root_at`, found by
                        // `live_migration` under contention.) The `roots_len` test is purely
                        // the bounds condition: every slot this reads — the journal slot and
                        // the operand journal above it — lies below `frame_nslots`. It is
                        // `>=`, not `==`, so a native that left the stack dirty still resumes
                        // exactly as it did before.
                        let resume = if matches!(jit_outcome, Some(1) | Some(2))
                            && heap.roots_len() >= cur_base + frame_nslots
                        {
                            jit_ckpt_resume(heap, cur_arm.arc(), cur_base, frame_nslots)
                        } else {
                            None
                        };
                        // Restore the small frame top on every non-Done path so the `exec_chunk`
                        // re-run sees the original layout (Done retires the whole frame anyway).
                        // The inlined native keeps operands in registers, so it leaves `roots`
                        // exactly at the frame top it was entered with (`cur_base+inline_nslots`).
                        // A Some(4) tail outcome stages callee+args ABOVE that top, read by
                        // `jit_dispatch_tail` relative to `frame_size_for_new_entry` — don't disturb those.
                        // A frame that will RESUME keeps its larger top: it continues in the
                        // layout that wrote the journal.
                        if inlined_active
                            && resume.is_none()
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
                                // System-monitor deopt event (observability stream):
                                // deopts are rare, so the armed() check runs only on
                                // this already-cold branch.
                                if crate::process::sysmon::armed() {
                                    if let Some(pid) = crate::process::current_pid() {
                                        crate::process::sysmon::emit_deopt(pid, cur_arm.dbg_name);
                                    }
                                }
                                #[cfg(feature = "perf-stats")]
                                if std::env::var_os("BROOD_DEOPT_TRACE").is_some() {
                                    // The checkpoint journal (ckpt_slot) packs
                                    // (resume_ip << 16 | operand-depth) — print it so a
                                    // deopt-storm's SITE is identifiable, not just its arm.
                                    let ckpt = if cur_arm.ckpt_slot != u32::MAX {
                                        match heap.root_at(cur_base + cur_arm.ckpt_slot as usize) {
                                            Value::Int(p) => p,
                                            _ => -1,
                                        }
                                    } else {
                                        -1
                                    };
                                    eprintln!(
                                        "[deopt] arm={} watch={} resume_ip={} depth={}",
                                        cur_arm
                                            .dbg_name
                                            .map(crate::core::value::symbol_name_ref)
                                            .unwrap_or("<closure>"),
                                        cur_arm.deopt_watch,
                                        ckpt >> 16,
                                        ckpt & 0xffff
                                    );
                                }
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
                            Some(4) => {
                                // Pass the size this frame was BUILT to (KI-48): the
                                // dispatcher must not re-derive it from
                                // `frame_size_for_new_entry()`, which can change mid-flight.
                                jit_dispatch_tail(heap, cur_base, &cur_arm, cur_env, frame_nslots)
                            }
                            // 1 (deopt) / 2 (preempt) / None (not hot / out of subset): run the
                            // arm on the VM with the frame intact (`cur_ip` is still 0).
                            _ => {
                                // Deopt-resume (see `CompiledArm::ckpt_slot`): a deopt
                                // in an activation that completed a non-tail call
                                // resumes AT the checkpoint (operands re-pushed from
                                // the journal slots) — never re-running, and so never
                                // re-effecting, the code before it. A preempt / cold
                                // arm keeps the ip-0 entry (checkpoint reads 0 there).
                                // Outcome 2 (preempt) takes this too. A preempt normally lands on
                                // a back edge, where the journal was just reset to 0 and
                                // `jit_ckpt_read` returns `None` — so this is a no-op there, and
                                // the ip-0 entry is kept exactly as before. But if a preempt ever
                                // lands *after* a completed call or a `table-put`, re-running from
                                // ip 0 would repeat that effect, and the journal is precisely the
                                // record of what must not be redone. Honouring it costs nothing
                                // and removes a whole class of "is preemption safe here?".
                                if let Some((ra, rip, depth)) = resume {
                                    let cb = cur_base + ra.ckpt_slot as usize + 1;
                                    for k in 0..depth {
                                        let v = heap.root_at(cb + k);
                                        heap.push_root(v);
                                    }
                                    cur_ip = rip;
                                    // Continue in the chunk the journal's ip indexes: for the
                                    // small native that is `cur_arm` itself, for a
                                    // leaf-spliced native the derivation's resume arm, whose
                                    // chunk and frame layout are the spliced ones.
                                    cur_arm = ArmHandle::new(ra);
                                }
                                exec_chunk(
                                    heap,
                                    &cur_arm,
                                    &mut cur_ip,
                                    cur_base,
                                    cur_env,
                                    capture,
                                    &mut cur_back_edges,
                                )
                            }
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
                        heap.set_ic_bases(caller.ic_bases);
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
            Ok(ChunkExit::Call {
                arm,
                args,
                genv,
                bases,
            }) => {
                if frames.len() + 1 > MAX_BC_FRAMES {
                    unwind(heap);
                    let mut e = crate::eval::bc_frame_depth_error(frames.len());
                    attach_vm_trace(&mut e, &cur_arm, &frames);
                    return Err(e);
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
                    ic_bases: heap.ic_bases(),
                    #[cfg(feature = "jit")]
                    back_edges: cur_back_edges,
                });
                heap.set_ic_bases(bases);
                cur_env_base = heap.env_roots_len();
                cur_env = heap.root_env(genv);
                cur_base = heap.roots_len();
                cur_arm_slot = if cur_arm.has_runtime_handles {
                    heap.live_arm_push(cur_arm.clone())
                } else {
                    usize::MAX
                };
                if let Err(mut e) = push_frame(heap, &cur_arm, &args, cur_env) {
                    unwind(heap);
                    attach_vm_trace(&mut e, &cur_arm, &frames);
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
            Ok(ChunkExit::Tail {
                arm,
                args,
                genv,
                bases,
            }) => {
                crate::perf_bump!(tail_call);
                heap.set_ic_bases(bases);
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
                if let Err(mut e) = push_frame(heap, &arm, &args, cur_env) {
                    unwind(heap);
                    attach_vm_trace(&mut e, &cur_arm, &frames);
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
                    ic_bases: heap.ic_bases(),
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
                    ic_bases: heap.ic_bases(),
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
                // A `Control::Kill` that unwound untrappably through a `try`/`%isolate`/HOF
                // (an `(exit …)` reaching a native-nested `receive`) retires the process
                // with its pending reason, rather than crashing it as an uncaught error.
                // Only the **top-level** body driver (`capture`) may produce a `Killed`
                // outcome — a nested `vm_apply` run can't cross the native boundary with an
                // outcome, so it re-raises (like `Suspend`) and the kill keeps unwinding to
                // the top-level driver, which converts it there.
                if capture && e.is_kill_signal() {
                    return Ok(VmOutcome::Killed);
                }
                let mut e = e;
                attach_vm_trace(&mut e, &cur_arm, &frames);
                return Err(e);
            }
        }
    }
}

// ===================== entry =====================
