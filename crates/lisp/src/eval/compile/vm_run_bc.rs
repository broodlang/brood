//! Outer VM trampoline + program-run bodies.
use super::*;
#[cfg(feature = "jit")]
use crate::jit::outcome;

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
    resume: Option<Continuation>,
) -> Result<VmOutcome, LispError> {
    match resume {
        // Resume: the continuation's own `cur.arm` drives; `arm0`/`genv0` are ignored
        // by the resume branch, so pass a (cheap) clone of it as the placeholder.
        Some(Continuation::Vm(s)) => {
            let arm = s.cur.arm.clone();
            vm_run_bc(heap, arm, &[], EnvId::GLOBAL, Some(s), true)
        }
        Some(Continuation::Tw(s)) => crate::eval::resume_captured(heap, s),
        // Under a tier-0 ceiling the body runs on the tree-walker's capturing machine
        // (ADR-416 stage 4), whose `receive` parks and whose budget preempts as the VM's do.
        None if tier_ceiling() == Tier::TreeWalk => match body.unpack() {
            ValueRef::Fn(id) => crate::eval::run_captured(heap, crate::eval::TwStart::Closure(id)),
            _ => Err(LispError::type_err("process body must be a function")),
        },
        // Fresh: run the 0-arg body. A VM-eligible body (the overwhelming case — every
        // body in the whole suite is) runs on the capture driver, so its `receive`s
        // capture + migrate. A body that defers to the tree-walker (no compiled 0-arg
        // arm — vanishingly rare; zero across the suite) has no reified frame stack to
        // capture, so it runs tree-walked **on this worker thread** and its `receive`s
        // **block** the worker (the dirty-scheduler carve-out, §7.4); it returns Done/
        // Err and never suspends. Either way: no coroutine.
        None => match body.unpack() {
            // Resolve ONCE: two resolutions (`is_some()` then `expect`) can disagree — a
            // peer process advancing `free_epoch` between them clears this process's
            // `vm_cache` — so the second would be a panic vector on a body that compiles.
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
    /// The printed last-form value. A `Value` can't cross the process-heap boundary (it
    /// dies with the program's heap), so the driver renders it to a string while the heap
    /// is alive; the playground reads it here. `run_program` discards the value, so this
    /// stays unset on that path.
    ///
    /// Not `#[cfg(wasm32)]`: gated, the whole result path was invisible to host tests, so
    /// the playground's shipped entry point could not be asserted at all.
    result: std::sync::Mutex<Option<String>>,
    /// Whether anyone will ever READ `result`. Only `run_program_repr` does; `run_program`
    /// discards it. Set once at construction, so there is no race with the program starting.
    ///
    /// Without it `finish_form` would render EVERY top-level form's value to a string on the
    /// native path, where nothing reads it — cost proportional to the value's size, paid in
    /// full by a program whose top level binds one huge structure (`(def data (sort …))`).
    /// Keep the path ungated (that is what makes it testable on the host); just don't do
    /// the work when the answer is thrown away.
    want_result: bool,
}

impl ProgramExit {
    /// `want_result` is whether the caller will read [`take_result`](Self::take_result);
    /// when false the driver skips rendering each form's value entirely.
    pub fn new(want_result: bool) -> Arc<Self> {
        Arc::new(ProgramExit {
            slot: std::sync::Mutex::new(None),
            cv: std::sync::Condvar::new(),
            result: std::sync::Mutex::new(None),
            want_result,
        })
    }

    /// Whether the printed result is wanted — see [`want_result`](Self::want_result).
    pub fn wants_result(&self) -> bool {
        self.want_result
    }

    fn publish(&self, r: Result<(), LispError>) {
        let mut g = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        if g.is_none() {
            *g = Some(r);
            self.cv.notify_all();
        }
    }

    /// Store the program's printed result (wasm; called by the driver at completion).
    pub fn set_result(&self, s: Option<String>) {
        *self.result.lock().unwrap_or_else(|e| e.into_inner()) = s;
    }

    /// Take the program's printed result (wasm; the playground reads it after `wait`).
    pub fn take_result(&self) -> Option<String> {
        self.result.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    /// Block the calling (root) thread until the program publishes its outcome. On wasm
    /// there are no worker threads, so instead of parking we drive the run queue on this
    /// thread (`pump_until_quiescent`) — which runs the program and everything it spawns to
    /// completion — then read the slot. If the pump goes quiescent without a publish the
    /// program deadlocked in a top-level `receive` (no sender will ever wake it); return
    /// `:normal` rather than hang forever, as there is no other thread to make progress.
    /// How the program ended, without waiting: `None` while it still runs. What a host
    /// that drives a long-lived program itself (a browser tab's pump) checks after each
    /// drive.
    pub fn ended(&self) -> Option<Result<(), LispError>> {
        self.slot.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

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
    resume: Option<Continuation>,
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
        heap.set_imports(crate::core::heap::ImportTable::default());
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
            // `BROOD_TIER=0`: the whole form runs on the tree-walker's capturing machine
            // (ADR-416 stage 4) — `def` is its special form, and a top-level `receive`
            // parks like any green process's.
            if tier_ceiling() == Tier::TreeWalk {
                prog.def_name = None;
                return crate::eval::run_captured(heap, crate::eval::TwStart::Form(expanded, root));
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

    prog.exit.set_result(prog.last_repr.take());
    prog.exit.publish(Ok(()));
    Ok(VmOutcome::Done(Value::nil()))
}

impl ProgramState {
    /// A form finished with value `v`: if it was a `(def name rhs)` whose RHS we ran, bind
    /// `name` to `v` now (reusing the full `def` semantics — naming, promote-to-shared,
    /// reload diagnostics); either way advance to the next form.
    fn finish_form(&mut self, heap: &mut Heap, v: Value) -> Result<(), LispError> {
        // Render the form's value while its heap is alive, but ONLY when someone will read
        // it (`run_program_repr`, i.e. the playground). `print` is proportional to the
        // value's size and `run_program` discards the result, so doing it unconditionally
        // charges every native program for rendering a string nothing consumes — see
        // `ProgramExit::want_result`. Overwritten each form, so after the last one it holds
        // the program's result.
        if self.exit.wants_result() {
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
    /// nothing. An unwinding exit signal (the tree-walker's, which no VM driver turned
    /// into `Killed`) is a death, not a crash: retire with the pending reason, so every
    /// tier ends the program the same way (`retire_outcome`).
    fn crash(&self, e: LispError) -> VmOutcome {
        if e.is_kill_signal() {
            return VmOutcome::Killed;
        }
        // Attach the file to the error's own field (innermost `load` wins, a no-op if
        // already set) so `located()` renders the canonical `file:LINE:COL: msg` — NOT
        // string-prepend `file: ` onto an already-located `LINE:COL: msg`, which emits a
        // stray space (`file: LINE:COL:`) that diverges from the tree-walker's editor-
        // parseable form.
        let mut e = match &self.file {
            Some(f) => e.or_file(f.clone()),
            None => e,
        };
        // Raised inside the standard library: report it at the program's own code.
        let form_pos = self.positions.get(self.cursor).copied();
        e = e.relocated_out_of_std(self.file.as_deref(), form_pos);
        // The payload `Value` is a handle into THIS process's heap — dead once the
        // root thread reads the slot. Everything else (message, pos, hint, trace)
        // is plain data and crosses intact.
        e.payload = None;
        self.exit.publish(Err(e));
        VmOutcome::Done(Value::nil())
    }

    /// The outcome of a program process retiring with `reason` (KI-225): `:normal` ends
    /// the program cleanly, anything else is the "killed by an exit signal" error.
    pub(crate) fn retire_outcome(reason: &crate::process::Message) -> Result<(), LispError> {
        match reason {
            crate::process::Message::Keyword(k)
                if *k == crate::core::value::intern(crate::process::keywords::NORMAL) =>
            {
                Ok(())
            }
            _ => Err(LispError::killed_by(Some(
                crate::process::exit_reason_phrase(reason),
            ))),
        }
    }

    /// The program process has retired with `outcome` (from [`retire_outcome`]). A
    /// program that finished or crashed has already published, so this only lands when
    /// it died mid-run — an exit signal, or a scheduler fault — and the root thread
    /// blocked in [`ProgramExit::wait`] must hear of it or it waits forever.
    ///
    /// [`retire_outcome`]: Self::retire_outcome
    pub(crate) fn retired(&self, outcome: Result<(), LispError>) {
        self.exit.publish(outcome);
    }
}

/// Backstop for a retire path that drops the process without naming a reason (a
/// panicking scheduler tail): the waiting root thread is still told. A no-op once an
/// outcome was published — the slot is first-wins.
impl Drop for ProgramState {
    fn drop(&mut self) {
        self.exit.publish(Err(LispError::killed_by(None)));
    }
}

/// Book what a native run of `arm` (its frame at `base`) came back with, for the two
/// frame-owning callers (the driver and `dispatch`'s computed-head call):
/// - work attribution (perf-stats): native completion vs a mid-run deopt vs a preemption.
///   A hot arm with high `jit_deopt` against `jit_native` compiles but keeps falling off
///   the native path — the matmul-class signal;
/// - `BROOD_DEOPT_TRACE`, naming the arm, the checkpoint and the guard that fired.
#[cfg(feature = "jit")]
pub(super) fn book_native_outcome(
    heap: &Heap,
    arm: &CompiledArm,
    base: usize,
    jit_outcome: Option<i64>,
) {
    match jit_outcome {
        Some(outcome::DONE | outcome::TAIL) => {
            crate::perf_bump!(jit_native);
        }
        Some(outcome::DEOPT) => {
            crate::perf_bump!(jit_deopt);
            // (The system-monitor event is `jit_any_deopt_feedback`'s, which every native
            // exit reaches.)
            #[cfg(feature = "perf-stats")]
            if crate::diagnostics::debug_flags::deopt_trace() {
                // The checkpoint journal (ckpt_slot) packs
                // (resume_ip << 16 | operand-depth) — print it so a
                // deopt-storm's SITE is identifiable, not just its arm.
                let ckpt = if arm.ckpt_slot != u32::MAX {
                    match heap.root_at(base + arm.ckpt_slot as usize) {
                        Value::Int(p) => p,
                        _ => -1,
                    }
                } else {
                    -1
                };
                // `reason=` names the lowering's guard site (`DeoptReason`) — without it a
                // persistent deopt names its arm and nothing else.
                eprintln!(
                    "[deopt] arm={} watch={} resume_ip={} depth={} reason={}",
                    arm.dbg_label(),
                    arm.deopt_watch,
                    ckpt >> 16,
                    ckpt & 0xffff,
                    crate::eval::compile::DeoptReason::describe(heap.jit_deopt_reason())
                );
            }
        }
        Some(outcome::PREEMPT) => {
            crate::perf_bump!(jit_preempt);
        }
        _ => {}
    }
    // Only the trace reads the frame.
    #[cfg(not(feature = "perf-stats"))]
    let _ = (heap, arm, base);
}

/// Settle a frame a native run has just handed back — the part of the frame path that
/// runs between `jit_tier_in_frame` returning and the outcome being acted on, shared with
/// the VM→native direct call's `ChunkExit::CallResume` so the two cannot drift.
///
/// Decodes the deopt checkpoint (once, before anything resizes the frame), restores an
/// inlined frame's small top on the non-resume paths, books the outcome (perf counters,
/// the sysmon deopt event, `BROOD_DEOPT_TRACE`), runs the dirty-stack check, and — for a
/// journalled deopt/preempt — re-pushes the journal operands so the frame is ready to
/// continue at the returned `(resume arm, ip)`. `None` means continue at ip 0 in the
/// arm as it stands.
#[cfg(feature = "jit")]
#[allow(clippy::too_many_arguments)]
fn settle_native_frame(
    heap: &mut Heap,
    cur_arm: &Arc<ArmHandle>,
    cur_base: usize,
    frame_nslots: usize,
    inlined_active: bool,
    small_top: usize,
    pre_roots: usize,
    jit_outcome: Option<i64>,
) -> Option<(Arc<CompiledArm>, usize)> {
    // The deopt-resume decision, taken ONCE and taken HERE, before the
    // frame is resized below. Reading the journal twice — once to decide
    // the resize, once to resume — is wrong two ways: the second read
    // comes from an already-truncated frame and indexes past the root
    // stack, and the two reads can disagree about which engine ran the
    // frame. The `roots_len` test is purely
    // the bounds condition: every slot this reads — the journal slot and
    // the operand journal above it — lies below `frame_nslots`. It is
    // `>=`, not `==`, so a native that left the stack dirty still resumes.
    let resume = if matches!(jit_outcome, Some(outcome::DEOPT | outcome::PREEMPT))
        && heap.roots_len() >= cur_base + frame_nslots
    {
        jit_ckpt_resume(heap, cur_arm.arc(), cur_base, frame_nslots)
    } else {
        None
    };
    // Dirty-stack-on-deopt check: a native arm that deopts (1) or is preempted (2) must
    // leave `roots` as `jit_tier` found them; if it grew, the `exec_chunk` re-run starts
    // on a corrupt operand stack. It runs HERE, before the small-top restore below — that
    // restore is this function's own, deliberate resize of an inlined frame, not the
    // native's doing. Run after it, every deopt/preempt of a self- or leaf-inlined arm
    // would read as dirty by exactly `inline_nslots - nslots`, making the counter useless
    // for the class it exists to catch.
    if matches!(jit_outcome, Some(outcome::DEOPT | outcome::PREEMPT)) {
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
    // Restore the small frame top on every non-Done path so the `exec_chunk`
    // re-run sees the original layout (Done retires the whole frame anyway).
    // The inlined native keeps operands in registers, so it leaves `roots`
    // exactly at the frame top it was entered with (`cur_base+inline_nslots`).
    // A TAIL outcome stages callee+args ABOVE that top, read by
    // `jit_dispatch_tail` relative to `frame_size_for_new_entry` — don't disturb those.
    // A frame that will RESUME keeps its larger top: it continues in the
    // layout that wrote the journal.
    if inlined_active
        && resume.is_none()
        && matches!(jit_outcome, Some(outcome::DEOPT | outcome::PREEMPT) | None)
        && heap.roots_len() == cur_base + cur_arm.inline_nslots
    {
        heap.truncate_roots(small_top);
    }
    book_native_outcome(heap, cur_arm, cur_base, jit_outcome);
    // Deopt-resume (see `CompiledArm::ckpt_slot`): a deopt in an activation that
    // completed a non-tail call resumes AT the checkpoint (operands re-pushed from the
    // journal slots) — never re-running, and so never re-effecting, the code before it.
    // A preempt normally lands on a back edge, where the journal was just reset to 0 and
    // `jit_ckpt_resume` returns `None` — so this is a no-op there and the ip-0 entry is
    // kept. But if a preempt ever lands *after* a completed call or a `table-put`,
    // re-running from ip 0 would repeat that effect, and the journal is precisely the
    // record of what must not be redone. Honouring it costs nothing and removes a whole
    // class of "is preemption safe here?".
    let (ra, rip, depth) = resume?;
    let cb = cur_base + ra.ckpt_slot as usize + 1;
    for k in 0..depth {
        let v = heap.root_at(cb + k);
        heap.push_root(v);
    }
    Some((ra, rip))
}

/// Where a caught error resumes the driver ([`catch_in_driver`]).
struct Caught {
    /// The catching frame when it is a caller, to install as the current frame (`None` when
    /// it is the current one).
    frame: Option<BcFrame>,
    /// The ip to resume at in that frame.
    ip: usize,
    /// An interrupt region in tail position caught it (ADR-491): its answer, on top of the
    /// operand stack, is the frame's own return value, so the driver returns it instead of
    /// resuming at `ip`.
    returns: bool,
}

/// The catch half of the driver's `raise!` (ADR-400), for an error that is not a control
/// signal while this driver has an open `try` or interrupt region: unwind to the frame that
/// opened the innermost handler that takes it, bind the caught value and hand back where to
/// resume. What the caught error carries is what the nested evaluation of the body used to
/// give it: the trace names exactly the frames the catch pops (that evaluation's own
/// driver), and the call in the handler's frame that led to the error supplies a missing
/// position and file — the evaluator's call tagging — so a caught error's `:trace`, `:line`
/// and `:file` are unchanged. `Err` hands the error back when no open handler above `floor`
/// takes it.
///
/// An interrupt REGION (ADR-491) takes only the interrupt, and only when it owns the
/// window: it closes the window BEFORE the catch is noted, so the sticky re-raise is not
/// armed, and pushes `[:interrupted e]` where the call's value goes. Any other error — or
/// any error at a region inside an outer window — passes it: the region is dropped (closing
/// a window it owns) and the next handler down is asked.
#[cold]
#[inline(never)]
fn catch_in_driver(
    heap: &mut Heap,
    frames: &mut Vec<BcFrame>,
    cur_arm: &CompiledArm,
    cur_base: usize,
    mut e: crate::error::LispError,
    callers_only: bool,
    floor: usize,
) -> Result<Caught, crate::error::LispError> {
    use crate::core::heap::HandlerKind;
    let h = loop {
        if heap.vm_handlers.len() <= floor {
            return Err(e);
        }
        let h = *heap.vm_handlers.last().expect("above the floor");
        match h.kind {
            HandlerKind::Try => break h,
            // A region catches an interrupt raised for its own window (ADR-493); one
            // raised for an outer window passes through it, as every other error does.
            HandlerKind::Region { window, .. }
                if e.kind == crate::error::ErrorKind::Interrupted
                    && crate::process::interrupt_raised_for(window) =>
            {
                break h;
            }
            HandlerKind::Region { window, .. } => {
                heap.vm_handlers.pop();
                crate::process::close_interrupt_window(window);
            }
        }
    };
    let depth = if let HandlerKind::Region { .. } = h.kind {
        Some(h.caller).filter(|&caller| caller < frames.len())
    } else if h.base == cur_base {
        Some(frames.len())
    } else {
        frames.iter().rposition(|f| f.base == h.base)
    };
    debug_assert!(
        depth.is_some(),
        "an open handler's frame is not on this driver's stack (ADR-400): {h:?}"
    );
    let Some(d) = depth else {
        return Err(e);
    };
    let frame = if d < frames.len() {
        if callers_only {
            attach_vm_trace_callers(&mut e, &frames[d + 1..]);
        } else {
            attach_vm_trace(&mut e, cur_arm, &frames[d + 1..]);
        }
        let (file, pos) = call_site(&frames[d]);
        if let Some(p) = pos {
            e = e.or_pos(p);
        }
        if let Some(f) = file {
            e = e.or_file(f.to_string());
        }
        frames.truncate(d + 1);
        Some(
            frames
                .pop()
                .expect("the handler's frame is below the error"),
        )
    } else {
        // Raised by an instruction of the handler's own frame: it tagged the position
        // (`tag_pos`); the file is this arm's.
        if let Some(f) = cur_arm.src_file.as_deref() {
            e = e.or_file(f.to_string());
        }
        None
    };
    let region_tail = match h.kind {
        HandlerKind::Region { window, tail } => {
            crate::process::close_interrupt_window(window);
            Some(tail)
        }
        HandlerKind::Try => None,
    };
    let caught = e.caught_value(heap);
    heap.note_caught(e, caught);
    heap.truncate_roots(h.sp);
    heap.truncate_env_roots(h.env_len);
    heap.live_arm_truncate(h.arms_len);
    heap.truncate_dynamics(h.dyn_len);
    heap.vm_handlers.pop();
    match region_tail {
        None => {
            heap.set_root_at(h.bind, caught);
            Ok(Caught {
                frame,
                ip: h.ip,
                returns: false,
            })
        }
        Some(tail) => {
            // Nothing allocated since `caught` was made, so it is still where it was.
            let answer = heap.alloc_vector2(value::kw("interrupted"), caught);
            heap.push_root(answer);
            Ok(Caught {
                frame,
                ip: h.ip,
                returns: tail,
            })
        }
    }
}

/// Drop this process's open handlers above `floor` (ADR-400) as a driver is left by an
/// error none of them caught, or a kill. The REGIONS among them close their interrupt
/// windows (ADR-491): a window must not outlive the call that opened it. Closing the
/// outermost of them closes every window inside it (ADR-493).
fn truncate_handlers(heap: &mut Heap, floor: usize) {
    use crate::core::heap::HandlerKind;
    let outermost = heap.vm_handlers.get(floor..).and_then(|dropped| {
        dropped.iter().find_map(|handler| match handler.kind {
            HandlerKind::Region { window, .. } => Some(window),
            HandlerKind::Try => None,
        })
    });
    if let Some(window) = outermost {
        crate::process::close_interrupt_window(window);
    }
    heap.vm_handlers.truncate(floor);
}

/// Rebuild the native activations a preempt unwound through (ADR-401) as interpreter
/// frames: the thread's unwind record ([`take_unwound`]) holds them innermost first, each frame still where it ran in
/// `roots`. The callers — every one but the innermost — are appended to `frames`, outermost
/// first, each paused just after the call it made, its operand stack restored from the
/// in-call journal it wrote on the way out; the innermost is returned as the current frame,
/// settled exactly as the frame path settles a preempt.
///
/// A native callee's frame starts where its caller's native frame ended, but an interpreter
/// callee's frame starts above the caller's operand stack, so every frame moves: the whole
/// region is copied out and laid back down in interpreter order. Nothing in between
/// allocates, so no collection can run while the values are off the root stack.
#[cfg(feature = "jit")]
fn rebuild_unwound_frames(heap: &mut Heap, frames: &mut Vec<BcFrame>) -> BcFrame {
    let mut unwound = take_unwound();
    unwound.reverse();
    let first = unwound
        .first()
        .expect("an unwind with no recorded activation (ADR-401)")
        .0
        .base;
    let region: Vec<Value> = (first..heap.roots_len()).map(|i| heap.root_at(i)).collect();
    heap.truncate_roots(first);
    let innermost = unwound.len() - 1;
    let mut cur = None;
    for (i, (f, journal)) in unwound.into_iter().enumerate() {
        let rel = f.base - first;
        let base = heap.roots_len();
        for &v in &region[rel..rel + f.nslots] {
            heap.push_root(v);
        }
        let env_base = heap.env_roots_len();
        let env = heap.root_env(f.env);
        if i < innermost {
            let journal = journal.expect("an unwound caller left no in-call journal (ADR-401)");
            let resume = jit_in_call_resume_arm(&f.arm, f.nslots)
                .expect("a self-spliced activation was recorded mid-call (ADR-401)");
            debug_assert_eq!(
                resume.nslots, f.nslots,
                "an in-call journal of another layout"
            );
            // The operands it held beneath the call, from where its error path left them.
            let ops = journal.at - first;
            for &v in &region[ops..ops + journal.depth] {
                heap.push_root(v);
            }
            let ip = journal.ip;
            let arm = ArmHandle::new(resume);
            let arm_slot = if arm.has_runtime_handles {
                heap.live_arm_push(arm.clone())
            } else {
                usize::MAX
            };
            frames.push(BcFrame {
                arm,
                ip,
                base,
                env,
                env_base,
                arm_slot,
                ic_bases: f.bases,
                // Resumed mid-arm: the first `SelfCall` re-checks the binding.
                entry_epoch: u64::MAX,
                // A caller that is itself a native loop resumes interpreted, mid-iteration;
                // one short of the interval, its next back edge hands it straight back to
                // the tier check instead of interpreting 256 iterations after every
                // preempt of its callee (`collatz`'s `sweep` read +24% that way).
                back_edges: BACKEDGE_TIER_INTERVAL - 1,
            });
        } else {
            let arm = ArmHandle::new(f.arm);
            let arm_slot = if arm.has_runtime_handles {
                heap.live_arm_push(arm.clone())
            } else {
                usize::MAX
            };
            let inlined_active = f.nslots != arm.nslots;
            let resume = settle_native_frame(
                heap,
                &arm,
                base,
                f.nslots,
                inlined_active,
                base + arm.nslots,
                base + f.nslots,
                Some(outcome::PREEMPT),
            );
            let (arm, ip) = match resume {
                Some((ra, rip)) => (ArmHandle::new(ra), rip),
                None => (arm, 0),
            };
            cur = Some(BcFrame {
                arm,
                ip,
                base,
                env,
                env_base,
                arm_slot,
                ic_bases: f.bases,
                entry_epoch: u64::MAX,
                back_edges: 0,
            });
        }
    }
    cur.expect("the innermost unwound activation")
}

pub(crate) fn vm_run_bc(
    heap: &mut Heap,
    arm0: Arc<ArmHandle>,
    args0: &[Value],
    genv0: EnvId,
    resume: Option<Box<Suspended>>,
    top_level: bool,
) -> Result<VmOutcome, LispError> {
    crate::perf_bump!(vm_apply);
    // ADR-416 stage 1: under a tier-0 ceiling no bytecode should start, so name each path
    // that starts it anyway. The flag is read first — this is the VM's entry.
    if resume.is_none()
        && let Some(path) = tier0_audit_path()
        && tier_ceiling() == Tier::TreeWalk
    {
        record_tier0_escape(&path, top_level);
    }
    // Keep the GC-block depth consistent for any nested native / tree-walked sub-call
    // (their own `stack_overflow_check` reads it). The driver itself doesn't recurse
    // per Brood call — runaway non-tail recursion is caught by `MAX_BC_FRAMES` below,
    // not the native-stack byte guard.
    let _gc_block = crate::process::GcBlockGuard::enter();
    // Publish this driver's `top_level` to the `receive` gate (restored on exit, so the
    // innermost driver wins): a top-level receive captures, a native-nested one blocks.
    let _top_guard = std::mem::DropGuard::new(
        crate::process::set_capture_top_level(top_level),
        |previous| {
            crate::process::set_capture_top_level(previous);
        },
    );
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
    let handler_floor: u32;
    let dyn_floor: u32;
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
    // The current frame's `SelfCall` hot-reload epoch (see `exec_chunk`'s `entry_epoch`).
    let mut cur_entry_epoch: u64;
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
            handler_floor = s.handler_floor;
            dyn_floor = s.dyn_floor;
            frames = s.frames;
            let cur = s.cur;
            cur_arm = cur.arm;
            cur_ip = cur.ip;
            cur_base = cur.base;
            cur_env = cur.env;
            cur_env_base = cur.env_base;
            cur_arm_slot = cur.arm_slot;
            heap.set_ic_bases(cur.ic_bases);
            cur_entry_epoch = cur.entry_epoch;
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
            handler_floor = heap.vm_handlers.len() as u32;
            dyn_floor = heap.dynamics_len() as u32;
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
            cur_entry_epoch = SELF_UNCHECKED_EPOCH;
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
        truncate_handlers(heap, handler_floor as usize);
        heap.truncate_dynamics(dyn_floor as usize);
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
    // preempt of a native loop would resume INTERPRETED and only re-tier at the next
    // 256th back-edge. A mid-`receive` resume rewinds to its
    // `Inst::Call` at a nonzero ip and still (correctly) never tiers.
    #[cfg(feature = "jit")]
    let mut try_jit = fresh || cur_ip == 0;
    // Read once per driver entry: the ceiling is fixed for a run (the env is cached, and the
    // per-thread override is set between top-level forms, never inside one). Below Native
    // `jit_tier_in_frame` returns `None` at once, so there is no tier check to make.
    #[cfg(feature = "jit")]
    let native_ceiling = tier_ceiling() >= Tier::Native;
    #[cfg(not(feature = "jit"))]
    let _ = fresh; // silence unused warning when the JIT is off

    // Sampling profiler (observability timing tier): the epoch this driver last
    // sampled at. Loop-local — a resume simply re-samples on its next epoch tick.
    let mut profiled_epoch: u64 = 0;

    // Set by a `Done` that handed its value back to a caller frame: the caller resumes
    // mid-body, so the loop-top safepoint has nothing to do that the CALL did not already
    // do — the reduction was ticked, the deadline and quantum checks ran, and the caller's
    // next call or back edge runs them again. Only a due collection (and the heap-limit
    // raise a collection arms) cannot wait, and the `Done` arm declines to set this when
    // either is pending. A reduction is charged per call, not per return.
    let mut returned_to_caller = false;

    // Set when an interrupt region in TAIL position ended (ADR-491): its answer is on top of
    // the operand stack and is the current frame's return value, so the next turn of the
    // loop retires the frame with it instead of running more of its code.
    let mut return_top = false;

    // Raise `e` from the current frame: if one of THIS driver's open `try`s can take it,
    // resume at its handler (`continue` the loop, `catch_in_driver`); otherwise unwind the
    // whole driver and return the error, as before (ADR-400).
    macro_rules! raise {
        ($l:lifetime, $e:expr) => {
            raise!($l, $e, false)
        };
        // `callers_only`: the current frame is a native arm that already recorded itself in
        // the error's trace (KI-117), so only the frames beneath it are named.
        ($l:lifetime, $e:expr, $callers_only:expr) => {{
            let e: crate::error::LispError = $e;
            // The catch itself is out of line (`catch_in_driver`): this expands at every
            // error site of the hottest loop in the runtime, and a `receive` parks through
            // here on every message.
            let caught = if e.is_control() || heap.vm_handlers.len() <= handler_floor as usize {
                Err(e)
            } else {
                catch_in_driver(
                    heap,
                    &mut frames,
                    &cur_arm,
                    cur_base,
                    e,
                    $callers_only,
                    handler_floor as usize,
                )
            };
            match caught {
                Ok(Caught { frame, ip, returns }) => {
                    if let Some(f) = frame {
                        cur_arm = f.arm;
                        cur_base = f.base;
                        cur_env = f.env;
                        cur_env_base = f.env_base;
                        cur_arm_slot = f.arm_slot;
                        heap.set_ic_bases(f.ic_bases);
                        cur_entry_epoch = f.entry_epoch;
                        #[cfg(feature = "jit")]
                        {
                            cur_back_edges = f.back_edges;
                        }
                    }
                    cur_ip = ip;
                    #[cfg(feature = "jit")]
                    {
                        try_jit = false;
                    }
                    returned_to_caller = false;
                    return_top = returns;
                    continue $l;
                }
                Err(mut e) => {
                    unwind(heap);
                    if $callers_only {
                        attach_vm_trace_callers(&mut e, &frames);
                    } else {
                        attach_vm_trace(&mut e, &cur_arm, &frames);
                    }
                    return Err(e);
                }
            }
        }};
    }

    // The current frame's registers as a `BcFrame` running `$arm`: the current arm when a
    // capture saves it, the caller's when a call suspends it.
    macro_rules! current_frame {
        ($arm:expr) => {
            BcFrame {
                arm: $arm,
                ip: cur_ip,
                base: cur_base,
                env: cur_env,
                env_base: cur_env_base,
                arm_slot: cur_arm_slot,
                ic_bases: heap.ic_bases(),
                entry_epoch: cur_entry_epoch,
                #[cfg(feature = "jit")]
                back_edges: cur_back_edges,
            }
        };
    }

    // Return the captured continuation — `frames` under `$cur`, with this driver's entry
    // marks — as `VmOutcome::$outcome`, WITHOUT unwinding (ADR-100 §8).
    macro_rules! return_captured {
        ($outcome:ident, $cur:expr, $deadline:expr) => {
            return Ok(VmOutcome::$outcome(Continuation::Vm(Box::new(Suspended {
                frames,
                cur: $cur,
                entry_roots,
                entry_env,
                entry_arms,
                handler_floor,
                dyn_floor,
                deadline: $deadline,
            }))))
        };
    }

    'drive: loop {
        'safepoint: {
            if std::mem::take(&mut returned_to_caller) {
                break 'safepoint;
            }
            // Profiler sample: when armed and the ticker's epoch moved, record this
            // driver's named-frame stack (cur + pending callers, innermost first).
            // Off (the default): one relaxed bool load per frame boundary.
            if crate::diagnostics::profile::armed() {
                let ep = crate::diagnostics::profile::epoch();
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
                    crate::diagnostics::profile::record(&stack);
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
            // itself is O(this process's roots), i.e. O(recursion depth) — do not assume a
            // flat O(1) report; without the cache a deep process re-walks its roots per
            // report (KI-14).
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
            // Per-process heap limit (`(process-flag :max-heap n)`): the sticky flag
            // the loop-top collection armed raises here — catchable, and it kills
            // just this process, with the trace showing where the data was live.
            if let Some(live) = heap.take_proc_limit_hit() {
                let limit = heap.proc_mem_limit().unwrap_or(0);
                raise!('drive, crate::eval::proc_memory_limit_error(live, limit));
            }
            // Once per quantum, at the reduction rollover: the soft memory limit, the mailbox
            // bound (ADR-307) and a pending hard kill — see `loop_safepoint` in `exec_chunk`,
            // which splits the inline loop's safepoint the same way. Per frame boundary they
            // would cost twice per non-tail call.
            // A quantum boundary: a spent budget, or — where this driver can capture — a
            // preemption owed by code that could not yield itself (ADR-403).
            if let crate::process::Safepoint::Quantum { yield_now } =
                crate::process::driver_safepoint(capture)
            {
                if let Err(e) = super::quantum_checks(heap) {
                    raise!('drive, e);
                }
                // A pending hard `:kill` stops now: capture mode retires the process (no
                // capture — its heap is dropped); a nested run cannot return an outcome across
                // its native frame, so it unwinds with the untrappable signal, which the
                // top-level driver's `Err` arm converts to `VmOutcome::Killed` (the
                // tree-walker's loop top has the same contract).
                if crate::process::capture_hard_kill_pending() {
                    if capture {
                        return Ok(VmOutcome::Killed);
                    }
                    unwind(heap);
                    return Err(crate::error::LispError::kill_signal());
                }
                if yield_now {
                    // State-capture preemption (ADR-100 §8.1), in place of the coroutine
                    // yield: the frame boundary is the safepoint. The spent budget captures the
                    // continuation so `run_one` re-enqueues it (on any worker — live migration).
                    return_captured!(Preempted, current_frame!(cur_arm), None);
                }
            }
        }

        // Either run the arm natively (if it's flagged for a tier check) or interpret it.
        // Both yield a `Result<ChunkExit, _>` handled uniformly below. A tail-position
        // interrupt region that ended returns its answer from this frame (ADR-491).
        let exit = if std::mem::take(&mut return_top) {
            let n = heap.roots_len();
            Ok(ChunkExit::Done(heap.root_at(n - 1)))
        } else {
            #[cfg(feature = "jit")]
            {
                // `jit_tier_declines`: an arm the JIT refused (or a drain forcing the VM) would
                // come straight back `None` — interpret it without the tier check's preamble
                // and `settle_native_frame`, which it paid on every call before.
                if std::mem::take(&mut try_jit)
                    && native_ceiling
                    && !jit_tier_declines(heap, &cur_arm)
                {
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
                    // Per-engine frame sizing (two-stage tiering): the VM
                    // built the frame to the ORIGINAL `nslots` (small). ONLY when this arm's
                    // *installed* native version is the deferred inlined upgrade does the
                    // native entry need the larger `inline_nslots` frame (the spliced blocks'
                    // shifted slot ranges). `inline_installed` is false for every arm that
                    // doesn't inline (the overwhelming common case — fib is the exception),
                    // so the hot path pays nothing: it just calls `jit_tier`.
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
                    // Destination for a Done result: this entry hands one back, so it
                    // owns a stack local (see `crate::jit::JitArmFn`).
                    let mut jit_ret = Value::Nil;
                    // ADR-401: the top-level driver's native entry is unwindable — its
                    // callees' preempts can come back as frames to rebuild.
                    let jit_outcome = jit_tier_in_frame(
                        cur_arm.arc(),
                        heap,
                        cur_base,
                        cur_env,
                        frame_nslots,
                        &mut jit_ret as *mut Value,
                        capture,
                    );
                    let resume = settle_native_frame(
                        heap,
                        &cur_arm,
                        cur_base,
                        frame_nslots,
                        inlined_active,
                        small_top,
                        pre_roots,
                        jit_outcome,
                    );
                    match jit_outcome {
                        // Done: result in `roots[cur_base]` → the `Done` arm retires it.
                        Some(outcome::DONE) => Ok(ChunkExit::Done(jit_ret)),
                        // A JIT'd call/global errored — propagate the parked error. The
                        // native arm recorded ITS OWN frame in its error block (KI-117),
                        // so this takes the generic `Err` arm's path but attaches only
                        // the pending callers — `attach_vm_trace` would name `cur_arm`,
                        // which is that same arm, a second time.
                        Some(outcome::ERROR) => {
                            let e = jit_take_error(heap)
                                .expect("JIT error outcome without a parked error");
                            // A receive loop's native `%receive` found nothing (ADR-410): it
                            // did the whole park (scan position, status, deadline timer) and
                            // surfaced the suspend on the error channel. Capture this frame
                            // AT ip 0 — the frame holds the current iteration's arguments (a
                            // self-tail call stores them every iteration) and nothing before
                            // the `%receive` had an effect — so the wake re-tiers into the
                            // native loop. No VM re-run and no second mailbox scan.
                            if cur_arm.receive_at_head
                                && let Some(crate::error::Control::Suspend { deadline }) =
                                    &e.control
                            {
                                let deadline = *deadline;
                                // The arm's own frame: an inlined upgrade's extra slots are
                                // re-made by the next native entry (`extend_roots_to_nil`).
                                heap.truncate_roots(small_top);
                                cur_ip = 0;
                                Ok(ChunkExit::Suspend { deadline })
                            } else
                            // A preempt unwound out to here (ADR-401): this frame is the
                            // outermost activation it passed — record it with the rest.
                            if e.is_preempt_unwind() {
                                debug_assert!(capture, "an unwind reached a nested driver");
                                record_unwound(UnwindFrame {
                                    arm: cur_arm.arc().clone(),
                                    base: cur_base,
                                    nslots: frame_nslots,
                                    env: heap.read_root_env(cur_env),
                                    bases: heap.ic_bases(),
                                });
                                Ok(ChunkExit::Unwound {
                                    keep_current: false,
                                })
                            } else {
                                if capture && e.is_kill_signal() {
                                    unwind(heap);
                                    return Ok(VmOutcome::Killed);
                                }
                                raise!('drive, e, true);
                            }
                        }
                        // A JIT'd tail call: dispatch the staged callee+args → reuse the
                        // frame (`Tail`) or a finished native callee (`Done`).
                        Some(outcome::TAIL) => {
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
                            if let Some((ra, rip)) = resume {
                                cur_ip = rip;
                                // Continue in the chunk the journal's ip indexes: for the
                                // small native that is `cur_arm` itself, for a
                                // leaf-spliced native the derivation's resume arm, whose
                                // chunk and frame layout are the spliced ones.
                                cur_arm = ArmHandle::new(ra);
                            }
                            if jit_outcome == Some(outcome::PREEMPT) && capture {
                                // A native PREEMPT: the reduction budget is spent, so yield
                                // NOW with this frame — do not hand it to the interpreter
                                // "until its own loop-top notices". That interpreter run
                                // reaches its first safepoint inside the CALLEE's entry (a
                                // loop body's `Call` pushes a frame before its `SelfCall`
                                // ticks), so the capture would land on the callee at ip 0,
                                // the resume would return into this loop MID-BODY, and the
                                // loop would interpret up to 256 iterations before its
                                // back-edge re-tiered it. Captured here
                                // the frame is at ip 0 (or at the journal's resume point,
                                // applied just above), which is exactly what a resume re-tiers.
                                Ok(ChunkExit::Preempt)
                            } else {
                                exec_chunk(
                                    heap,
                                    &cur_arm,
                                    &mut cur_ip,
                                    cur_base,
                                    cur_env,
                                    capture,
                                    &mut cur_entry_epoch,
                                    &mut cur_back_edges,
                                )
                            }
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
                        &mut cur_entry_epoch,
                        #[cfg(feature = "jit")]
                        &mut cur_back_edges,
                    )
                }
            }
            #[cfg(not(feature = "jit"))]
            {
                exec_chunk(
                    heap,
                    &cur_arm,
                    &mut cur_ip,
                    cur_base,
                    cur_env,
                    capture,
                    &mut cur_entry_epoch,
                )
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
                        cur_entry_epoch = caller.entry_epoch;
                        #[cfg(feature = "jit")]
                        {
                            // Restore the caller's back-edge counter so SelfCall
                            // iterations accumulate correctly across non-tail calls.
                            cur_back_edges = caller.back_edges;
                        }
                        // The thunk of an interrupt region returned to the frame that
                        // opened it (ADR-491): the region ends — its window closes — and
                        // the call's value is `[:ok v]`. Only the thunk's frame returns to
                        // that frame while the region is the innermost handler: it is the
                        // frame at the index the region recorded.
                        let v = match heap.vm_handlers.last() {
                            Some(&crate::core::heap::VmHandler {
                                kind: crate::core::heap::HandlerKind::Region { window, tail },
                                caller,
                                ..
                            }) if caller == frames.len()
                                && heap.vm_handlers.len() > handler_floor as usize =>
                            {
                                heap.vm_handlers.pop();
                                crate::process::close_interrupt_window(window);
                                return_top = tail;
                                heap.alloc_vector2(value::kw("ok"), v)
                            }
                            _ => v,
                        };
                        // The result lands where the caller pushed the callee — its
                        // operand stack continues seamlessly past the call site.
                        heap.push_root(v);
                        returned_to_caller = !heap.gc_due() && !heap.proc_limit_pending();
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
                    raise!('drive, crate::eval::bc_frame_depth_error(frames.len()));
                }
                // Suspend the caller (resume at the already-advanced `cur_ip`) and
                // switch the registers to the callee. `exec_chunk` already dropped the
                // callee+args operands, so the callee's frame starts at `roots_len()`.
                let caller_arm = std::mem::replace(&mut cur_arm, arm);
                frames.push(current_frame!(caller_arm));
                heap.set_ic_bases(bases);
                cur_env_base = heap.env_roots_len();
                cur_env = heap.root_env(genv);
                cur_base = heap.roots_len();
                cur_arm_slot = if cur_arm.has_runtime_handles {
                    heap.live_arm_push(cur_arm.clone())
                } else {
                    usize::MAX
                };
                if let Err(e) = push_frame(heap, &cur_arm, &args, cur_env) {
                    raise!('drive, e);
                }
                // The callee frame is set up at `roots[cur_base..]` with `cur_ip = 0`; flag
                // it for a tier check at the loop top (the dominant Brood→Brood path). A
                // native `Done`/`Tail`/error is then handled by the shared arms above —
                // identical to the old inline call-site tiering, minus the duplication.
                cur_ip = 0;
                cur_entry_epoch = SELF_UNCHECKED_EPOCH;
                #[cfg(feature = "jit")]
                {
                    try_jit = true;
                    cur_back_edges = 0; // fresh counter for the callee's frame
                }
            }
            // `(proc/interruptible ceiling thunk)` (ADR-491): open a region in this frame and
            // push the thunk's frame, as `Call` does. The thunk runs on this driver's own
            // stack, so a `receive` in it parks the process, and a preempt or migration
            // carries the region with it (`vm_handlers` is the process's). Its `Done` back
            // into this frame closes the region; the catch path ends it on an error.
            Ok(ChunkExit::Interruptible {
                ceiling,
                arm,
                genv,
                bases,
                tail,
            }) => {
                if frames.len() + 1 > MAX_BC_FRAMES {
                    raise!('drive, crate::eval::bc_frame_depth_error(frames.len()));
                }
                // A window of its own, inside any already open (ADR-493).
                let window = crate::process::open_interrupt_window(ceiling);
                heap.vm_handlers.push(crate::core::heap::VmHandler {
                    kind: crate::core::heap::HandlerKind::Region { window, tail },
                    base: cur_base,
                    sp: heap.roots_len(),
                    env_len: heap.env_roots_len(),
                    arms_len: heap.live_arm_len(),
                    ip: cur_ip,
                    bind: usize::MAX,
                    dyn_len: heap.dynamics_len(),
                    caller: frames.len(),
                });
                let caller_arm = std::mem::replace(&mut cur_arm, arm);
                frames.push(current_frame!(caller_arm));
                heap.set_ic_bases(bases);
                cur_env_base = heap.env_roots_len();
                cur_env = heap.root_env(genv);
                cur_base = heap.roots_len();
                cur_arm_slot = if cur_arm.has_runtime_handles {
                    heap.live_arm_push(cur_arm.clone())
                } else {
                    usize::MAX
                };
                if let Err(e) = push_frame(heap, &cur_arm, &[], cur_env) {
                    raise!('drive, e);
                }
                cur_ip = 0;
                cur_entry_epoch = SELF_UNCHECKED_EPOCH;
                #[cfg(feature = "jit")]
                {
                    try_jit = true;
                    cur_back_edges = 0;
                }
            }
            // `Call` minus the argument round trip: the callee's `argc` args are already the
            // top of `roots`, so they become its first slots where they lie. `push_frame`'s
            // remaining work for such an arm (nil the binder slots, fill the captures) is
            // done here; there are no optionals or a rest param by construction.
            Ok(ChunkExit::CallInPlace {
                arm,
                argc,
                genv,
                bases,
            }) => {
                if frames.len() + 1 > MAX_BC_FRAMES {
                    raise!('drive, crate::eval::bc_frame_depth_error(frames.len()));
                }
                let caller_arm = std::mem::replace(&mut cur_arm, arm);
                frames.push(current_frame!(caller_arm));
                heap.set_ic_bases(bases);
                cur_env_base = heap.env_roots_len();
                cur_env = heap.root_env(genv);
                cur_base = heap.roots_len() - argc;
                cur_arm_slot = if cur_arm.has_runtime_handles {
                    heap.live_arm_push(cur_arm.clone())
                } else {
                    usize::MAX
                };
                heap.extend_roots_to_nil(cur_base + cur_arm.nslots);
                fill_captures(heap, &cur_arm, cur_base, heap.read_root_env(cur_env));
                cur_ip = 0;
                cur_entry_epoch = SELF_UNCHECKED_EPOCH;
                #[cfg(feature = "jit")]
                {
                    try_jit = true;
                    cur_back_edges = 0;
                }
            }
            // A native callee `exec_chunk` ran IN PLACE (the VM→native direct call,
            // compute-frontier §7.12) ended with an outcome only the driver can honour:
            // a deopt or preempt — continue THIS activation on the VM, at its checkpoint
            // or ip 0, in the frame the native left at `roots[base..base+nslots]` — or a
            // declined / i64-depth-bailed activation to run on the VM from ip 0. Adopt that
            // frame as the current one, exactly as `Call` does minus `push_frame` (the
            // frame is already built), and settle it with the frame path's own routine.
            // `try_jit` stays off: `jit_tier_in_frame` already ran for this activation.
            #[cfg(feature = "jit")]
            Ok(ChunkExit::CallResume {
                arm,
                base,
                nslots,
                genv,
                bases,
                outcome,
            }) => {
                if frames.len() + 1 > MAX_BC_FRAMES {
                    raise!('drive, crate::eval::bc_frame_depth_error(frames.len()));
                }
                let caller_arm = std::mem::replace(&mut cur_arm, arm);
                frames.push(current_frame!(caller_arm));
                heap.set_ic_bases(bases);
                cur_env_base = heap.env_roots_len();
                cur_env = heap.root_env(genv);
                cur_base = base;
                cur_arm_slot = if cur_arm.has_runtime_handles {
                    heap.live_arm_push(cur_arm.clone())
                } else {
                    usize::MAX
                };
                // The frame was built to `nslots` (`frame_size_for_new_entry()` at entry):
                // the inlined layout iff that differs from the small one. The native was
                // entered with `roots` exactly at the frame top.
                let inlined_active = nslots != cur_arm.nslots;
                let small_top = cur_base + cur_arm.nslots;
                let resume = settle_native_frame(
                    heap,
                    &cur_arm,
                    cur_base,
                    nslots,
                    inlined_active,
                    small_top,
                    cur_base + nslots,
                    outcome,
                );
                cur_ip = 0;
                if let Some((ra, rip)) = resume {
                    cur_ip = rip;
                    cur_arm = ArmHandle::new(ra);
                }
                try_jit = false;
                cur_back_edges = 0;
                // Resumed mid-arm: when the frame's guard last validated is unknown, so the
                // first `SelfCall` re-checks the binding.
                cur_entry_epoch = u64::MAX;
                if outcome == Some(outcome::PREEMPT) && capture {
                    // A native PREEMPT: yield NOW with this frame, as the frame path's
                    // preempt arm does — see its comment for why it must not wait for
                    // the interpreter's next safepoint.
                    return_captured!(Preempted, current_frame!(cur_arm), None);
                }
            }
            // A native preempt unwound out to this driver (ADR-401): rebuild every activation
            // it passed as an interpreter frame and park, exactly as a preempt of the frame
            // path would — the innermost frame resumes at its back edge and re-tiers; each
            // caller resumes just after the call that frame is serving.
            #[cfg(feature = "jit")]
            Ok(ChunkExit::Unwound { keep_current }) => {
                debug_assert!(capture, "a native preempt unwound into a nested driver");
                if keep_current {
                    frames.push(current_frame!(cur_arm));
                } else {
                    // The current frame is the outermost unwound activation and is rebuilt
                    // with the rest: drop its own env root and arm registration first.
                    heap.truncate_env_roots(cur_env_base);
                    if cur_arm_slot != usize::MAX {
                        heap.live_arm_truncate(cur_arm_slot);
                    }
                }
                let cur = rebuild_unwound_frames(heap, &mut frames);
                return_captured!(Preempted, cur, None);
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
                if let Err(e) = push_frame(heap, &arm, &args, cur_env) {
                    raise!('drive, e);
                }
                cur_arm = arm;
                cur_ip = 0;
                cur_entry_epoch = SELF_UNCHECKED_EPOCH;
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
            Ok(ChunkExit::ContractProbe) => {
                // `(%contract-tail? ret)` from a contract shim (KI-216, ADR-396): answer it
                // in place of `ret` and resume the same frame. Nothing ran and nothing
                // allocated, so the loop-top safepoint has nothing to do.
                let top = heap.roots_len() - 1;
                let ret = heap.root_at(top);
                let pending = contract_check_pending(heap, &frames, cur_base, ret);
                heap.set_root_at(top, Value::boolean(pending));
                returned_to_caller = true;
            }
            Ok(ChunkExit::Killed) => {
                // Hard kill fired at the inline SelfCall safepoint.
                return Ok(VmOutcome::Killed);
            }
            Ok(ChunkExit::Preempt) => {
                // Reduction budget exhausted at the inline SelfCall safepoint. The frame
                // is already reset (ip=0 inside exec_chunk); capture and re-enqueue.
                return_captured!(Preempted, current_frame!(cur_arm), None);
            }
            Ok(ChunkExit::Suspend { deadline }) => {
                // A clean `receive` parked (ADR-100 §8). `exec_chunk` rewound `cur_ip`
                // to the suspending `%receive` `Inst::Call` and left the callee + args
                // on the operand stack, so the captured continuation replays straight
                // from there. Capture the whole frame stack as `Suspended` and return
                // it WITHOUT unwinding — the operand stack and frame slots must survive
                // on the heap for the resume (a collection while parked relocates them
                // in place; the saved `base`/`env_base` indices stay valid).
                return_captured!(Suspended, current_frame!(cur_arm), deadline);
            }
            Err(e) => {
                // A `Control::Kill` that unwound untrappably through a `try`/`%isolate`/HOF
                // (an `(exit …)` reaching a native-nested `receive`) retires the process
                // with its pending reason, rather than crashing it as an uncaught error.
                // Only the **top-level** body driver (`capture`) may produce a `Killed`
                // outcome — a nested `vm_apply` run can't cross the native boundary with an
                // outcome, so it re-raises (like `Suspend`) and the kill keeps unwinding to
                // the top-level driver, which converts it there.
                if capture && e.is_kill_signal() {
                    unwind(heap);
                    return Ok(VmOutcome::Killed);
                }
                raise!('drive, e);
            }
        }
    }
}

/// Is the current activation's result already going to be checked against `ret`? (KI-216,
/// ADR-396.) True when the frame directly beneath — the one this activation returns into
/// — is a contract shim suspended inside `(%contract-await ret' (orig …))`, i.e. its
/// resume point is the [`PrimOp::ContractAwait`] mark, and `ret'` equals `ret`.
///
/// `ret'` needs no bookkeeping: it is the mark's first operand, pushed before the call,
/// so it is the top of that frame's operand stack — the root directly below the current
/// frame's base. Every way into the current frame keeps that base (a `Call` starts the
/// callee where the caller's operands end; a `Tail` rebuilds in place), which is what
/// makes "directly beneath" mean "this activation's value is that frame's awaited value".
///
/// Only `frames` is consulted, so a shim entered at the bottom of a NESTED run (a callback
/// under a native, `apply`) answers false — conservative, never wrong: it then checks.
fn contract_check_pending(heap: &Heap, frames: &[BcFrame], base: usize, ret: Value) -> bool {
    let Some(caller) = frames.last() else {
        return false;
    };
    let awaiting = caller
        .arm
        .chunk
        .as_ref()
        .and_then(|chunk| chunk.code.get(caller.ip))
        .is_some_and(|inst| {
            matches!(
                inst,
                Inst::Prim2 {
                    op: PrimOp::ContractAwait,
                    ..
                }
            )
        });
    awaiting && base > 0 && heap.equal(heap.root_at(base - 1), ret)
}

// ===================== entry =====================

#[cfg(test)]
thread_local! {
    /// A per-thread audit file, so a unit test can check the audit fires without depending on
    /// whether some earlier test in the same process already read `BROOD_TIER0_AUDIT`.
    pub(crate) static TIER0_AUDIT_OVERRIDE: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
}

fn tier0_audit_path() -> Option<String> {
    #[cfg(test)]
    if let Some(path) = TIER0_AUDIT_OVERRIDE.with(|p| p.borrow().clone()) {
        return Some(path);
    }
    crate::diagnostics::debug_flags::tier0_audit_path().map(str::to_string)
}

/// `BROOD_TIER0_AUDIT=<file>` (ADR-416 stage 1): under a tier-0 ceiling, record the call path
/// by which bytecode just started. `vm_run_bc` is the one place bytecode starts, so every
/// line names a way tier 0 does not reach. The site is the first few `brood::` frames above
/// this driver, with the symbol hashes stripped; each distinct site is written once per OS
/// process, so `sort | uniq -c` over a suite's file counts the processes that took it.
#[cold]
#[inline(never)]
fn record_tier0_escape(path: &str, top_level: bool) {
    use std::collections::HashSet;
    use std::io::Write;
    use std::sync::{Mutex, OnceLock};
    static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    const DEPTH: usize = 6;

    // The rendered trace is `N: name` lines, each followed by an `at file:line` line when
    // the location is known. Names come from debuginfo and are unqualified, so a frame is
    // ours unless its location is in std or a dependency.
    let trace = std::backtrace::Backtrace::force_capture().to_string();
    let mut frames: Vec<(&str, Option<&str>)> = Vec::new();
    for line in trace.lines() {
        let line = line.trim_start();
        if let Some(location) = line.strip_prefix("at ") {
            if let Some(last) = frames.last_mut() {
                last.1 = Some(location);
            }
        } else if let Some((index, name)) = line.split_once(": ")
            && index.parse::<u32>().is_ok()
        {
            frames.push((name, None));
        }
    }
    let foreign = |location: Option<&str>| {
        location.is_some_and(|l| l.contains("/rustlib/") || l.contains("/.cargo/"))
    };
    // A trace without this driver's own frame still records a line: an audit that drops
    // what it cannot place reads as "no escapes". The name is bare from ordinary debuginfo
    // and fully qualified (`brood::eval::compile::vm_run_bc::vm_run_bc`) under the nightly
    // toolchain the AddressSanitizer job builds with — where an exact match never found the
    // driver and every line read `unplaced`, failing the audit's own guard.
    let entry = frames
        .iter()
        .position(|(name, _)| *name == "vm_run_bc" || name.ends_with("::vm_run_bc"))
        .map_or(0, |i| i + 1);
    let site: Vec<String> = frames[entry..]
        .iter()
        .filter(|(_, location)| !foreign(*location))
        .take(DEPTH)
        .map(|(name, location)| match location {
            // The path under `src/`, with its line: two calls in one function are two sites.
            Some(l) => format!("{name}@{}", l.rsplit("src/").next().unwrap_or(l)),
            None => name.to_string(),
        })
        .collect();
    let line = format!(
        "{}\t{}\n",
        match (entry, top_level) {
            (0, _) => "unplaced",
            (_, true) => "top",
            (_, false) => "nested",
        },
        site.join(" <- ")
    );
    let fresh = SEEN
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(line.clone());
    if fresh {
        // One `write_all` of one short line under O_APPEND: concurrent test processes
        // appending to the same file do not interleave within a line.
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = file.write_all(line.as_bytes());
        }
    }
}
