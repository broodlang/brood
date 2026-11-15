//! The background JIT compiler (ADR-101 1b): one lazily-spawned OS thread draining a
//! queue of arms to lower, the synchronous `jit_compile_now` for the cases that cannot
//! wait, and the float-global profile a lowering reads.

use super::*;

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
/// A background-compile work item: the arm, its enqueue-time slot-tag snapshot,
/// and the enqueuing runtime's plain-`u64` tag. Deliberately NOT the runtime
/// `Arc` (or a `Weak`): the single-process RUNTIME compactor's gate is
/// `Arc::get_mut`, so any reference parked in the queue would block compaction.
/// The tag keys the compiler thread's own publish map — its route to cross-
/// process dedupe (thousands of short-lived processes each queue their OWN
/// `CompiledArm` copy of the same shared closure; without the dedupe a spawn
/// storm compiled `fib` ~68× and the sync-compile escape hatch then stalled the
/// spawning process on the module lock the flood was holding).
pub(crate) type JitWorkItem = (Arc<CompiledArm>, Vec<u8>, u64);

#[cfg(feature = "jit")]
pub(crate) struct JitCompiler {
    /// Primary (initial-tier) queue: the small ORIGINAL arm. Drained first, always. A
    /// `None` is not work — it is [`Self::defer`]'s wake-up, so the idle thread can block
    /// on this one channel and still notice a deferred item without a polling loop.
    primary: std::sync::mpsc::SyncSender<Option<JitWorkItem>>,
    /// Deferred (lower-priority) queue: the re-derived **inlined** upgrade. The bg thread
    /// pulls from it only when `primary` is empty — so under a spawn-style initial-tier
    /// storm (thousands of short-lived processes tiering their small arms) the inlined
    /// upgrades sit behind the backlog and never compete; a long-lived workload (fib 35)
    /// drains its primary, then the deferred inlined compile lands and the swap fires.
    deferred: std::sync::mpsc::SyncSender<JitWorkItem>,
}

#[cfg(feature = "jit")]
impl JitCompiler {
    /// Queue an initial-tier compile. `Err` when the bounded queue is full.
    pub(crate) fn enqueue(&self, item: JitWorkItem) -> Result<(), ()> {
        self.primary.try_send(Some(item)).map_err(|_| ())
    }

    /// Queue a deferred (low-priority) compile and wake the thread if it is idle. The
    /// wake-up may find the primary queue full — then the thread is busy and reaches the
    /// deferred queue when primary drains, so dropping it loses nothing. Sent AFTER the
    /// item: a wake-up that lands before the thread blocks stays queued, so none is lost.
    pub(crate) fn defer(&self, item: JitWorkItem) -> Result<(), ()> {
        self.deferred.try_send(item).map_err(|_| ())?;
        let _ = self.primary.try_send(None);
        Ok(())
    }
}

/// Permanent keep-alive for every `CompiledArm` whose native code was installed into the
/// process-lifetime `GLOBAL_JIT` module. The native code bakes raw pointers into the arm's
/// chunk `ConstVal`s (read by `brood_rt_const_load`), so the arm (chunk) must outlive the
/// code — i.e. forever. Without this, the arm's only other owners are the closure / call-IC,
/// which are dropped when a closure is rebound or a green process exits, freeing the chunk
/// out from under still-installed native code (bug #2: a dangling ConstVal → garbage const).
///
/// Keyed by the installed code pointer, which also makes it the one authoritative answer to
/// "which arm does this native code belong to?" — asked on the cold paths where the arm's
/// `jit_code` may already have moved on (a reset, an inline swap, a latch), so scanning
/// for `jit_code == code` or re-resolving the callee by name can find the wrong arm or
/// none. Code is never freed, so a key never comes back for a different arm.
#[cfg(feature = "jit")]
pub(crate) static JIT_ARM_KEEPALIVE: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<usize, Arc<CompiledArm>>>,
> = std::sync::LazyLock::new(Default::default);

/// Pin `arm` for as long as the native `code` installed for it exists — forever.
#[cfg(feature = "jit")]
pub(crate) fn jit_keep_alive(code: *const u8, arm: &Arc<CompiledArm>) {
    JIT_ARM_KEEPALIVE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(code as usize, arm.clone());
}

/// The arm the installed native `code` was lowered from. Poison-tolerant like the insert:
/// a codegen panic may have poisoned the mutex, and a cold-path lookup must not turn that
/// into a worker-thread crash.
#[cfg(feature = "jit")]
pub(crate) fn jit_arm_for_code(code: *const u8) -> Option<Arc<CompiledArm>> {
    JIT_ARM_KEEPALIVE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&(code as usize))
        .cloned()
}

/// Load, before `arm` is handed to the compiler, every module a qualified global in its
/// chunk names that is not yet bound (ADR-335 item 4). **Native code never loads a module**:
/// a runtime callback runs with the arm's live values unspilled, so it cannot run
/// `require-one`; and the entry hoist already DEOPTS on an unbound global, which under lazy
/// loading would be a cliff — a hot arm with a cold branch into a not-yet-loaded module
/// deopting on every activation until the thrash latch marks it BAILED, interpreted for the
/// rest of the program without ever loading anything. So the invariant is established here,
/// on the VM side at the tiering election (the frame is on `roots`, a safe point): an arm
/// hot enough to compile has earned the load of what it names. Direct callees' chunks are
/// walked too, since the leaf inliner splices their bodies — and their global reads — into
/// this arm's native code.
///
/// Not gated on the load policy, for the reason `global_miss` gives: a module materialised
/// from the image had no compile pass, so its body references are unbound under either.
/// A name whose module cannot be found stays unbound (`global_miss`'s error is dropped —
/// the VM raises the real error if that branch ever runs); a broken module's error is
/// dropped here too, for the same reason: tiering is not the place to raise it.
///
/// Never WAITS, though: a module another process is mid-load of
/// (`derive::parkable_miss`) would hold this worker in `require-one`'s `receive`
/// under a native frame. Such a name is skipped and the
/// answer is `false` — the arm is not ready to go native — so the election stays on the VM,
/// whose routed miss parks, and tries again later. Lowering it now would be wrong, not just
/// slow: a native read of a name that is still unbound raises.
///
/// The answer: `None` when not ready, else whether a name the arm reads that was unbound
/// is bound now — the chunk was compiled without that binding (ADR-366). Asked directly,
/// not read off the global epoch: the epoch also moves for every rebind elsewhere in the
/// runtime, and under a steady trickle of them every election marked its arm stale, so a
/// hot arm was recompiled from scratch for as long as anything was being defined (KI-303).
#[cfg(feature = "jit")]
pub(super) fn preload_arm_globals(arm: &CompiledArm, heap: &mut Heap, env: EnvId) -> Option<bool> {
    use crate::core::value::Symbol;
    fn globals_of(chunk: &Chunk, out: &mut Vec<Symbol>) {
        for inst in &chunk.code {
            let sym = match inst {
                Inst::Global(s) | Inst::GlobalIc { sym: s, .. } => *s,
                Inst::Call { head: Some(s), .. } => *s,
                _ => continue,
            };
            if !out.contains(&sym) {
                out.push(sym);
            }
        }
    }
    let Some(chunk) = arm.chunk.as_ref() else {
        return Some(false);
    };
    let mut syms: Vec<Symbol> = Vec::new();
    globals_of(chunk, &mut syms);
    // One level of callees: the splice candidates. Their own callees are not spliced.
    let direct: Vec<Symbol> = syms.clone();
    for sym in direct {
        if let Some(Value::Fn(id)) = heap.env_get(env, sym) {
            // One compiled body per fixed arity the callee declares; an arm that has
            // not compiled yet has no chunk to splice and nothing to read here.
            let arities: Vec<usize> = heap
                .closure(id)
                .arms
                .iter()
                .map(|a| a.params.len())
                .collect();
            for argc in arities {
                if let Some(callee) = cached_arm_for(heap, id, argc)
                    && let Some(c) = callee.chunk.as_ref()
                {
                    globals_of(c, &mut syms);
                }
            }
        }
    }
    // Only a QUALIFIED unbound name can load anything; `global_miss` applies the same
    // exclusions the compile-time hooks do. The env is rooted across each load by the
    // callee; nothing else here is a heap handle.
    let unbound: Vec<Symbol> = syms
        .into_iter()
        .filter(|&sym| {
            crate::core::value::symbol_name_ref(sym).contains('/')
                && heap.env_get(env, sym).is_none()
        })
        .collect();
    let env_base = heap.env_roots_len();
    let env_root = heap.root_env(env);
    let mut ready = true;
    for &sym in &unbound {
        {
            let env = heap.read_root_env(env_root);
            // An earlier name's load may have bound this one.
            if heap.env_get(env, sym).is_some() {
                continue;
            }
            // Never START a load from inside one this process is running. The election
            // fires when a call count crosses a threshold, so a load begun here lands at an
            // arbitrary point of the enclosing module's load: its defines are journalled as
            // that module's, and what the module "introduces" depends on when some hot arm —
            // the prelude's `get`, whose string branch names `string/char-at` — happened to
            // tier up. Under contracts' extra calls that moved `string` into `project`'s load,
            // and the stdimage attribution, which probes each module's introduced names,
            // charged it there (`stdimage_test` "a root global is attributed…"). Decline
            // instead, as for a load another process holds: the arm stays on the VM and
            // re-elects once the load is over.
            if heap.in_journalled_load()
                && crate::eval::derive::module_to_load_off_native(heap, sym).is_some()
            {
                ready = false;
                continue;
            }
            // `parkable_miss` declines a module another process holds — whether its claim
            // was seen up front or taken while this load was on its way to it — instead of
            // waiting for it. An error is discarded on purpose: this is a best-effort
            // pre-load, and nothing here depends on the name resolving. A load error or a
            // name that stays unbound just leaves the arm as it was — its own global read
            // raises (or deopts to the VM, which raises) at the point the program reaches
            // it, with the same error it would have had without the pre-load.
            if let Ok(crate::eval::derive::ParkableMiss::Elsewhere(_)) =
                crate::eval::derive::parkable_miss(heap, env, sym, None)
            {
                ready = false;
            }
        }
    }
    let env = heap.read_root_env(env_root);
    let loaded = unbound.iter().any(|&sym| heap.env_get(env, sym).is_some());
    heap.truncate_env_roots(env_base);
    ready.then_some(loaded)
}

/// Snapshot which free globals this arm reads currently hold a `Value::Float` into
/// [`CompiledArm::float_globals`] (see that field for why the param profile alone is not
/// enough). Runs on the thread that wins the tiering election — the only place that has
/// both the arm and a `Heap`; the lowering thread has no heap. Once per arm: the
/// `OnceLock` makes a later observation a no-op, which is what keeps a shared arm's
/// lowering deterministic across the processes of a runtime.
#[cfg(feature = "jit")]
pub(super) fn record_float_globals(arm: &CompiledArm, heap: &Heap, env: EnvId) {
    if arm.float_globals.get().is_some() {
        return;
    }
    let Some(chunk) = arm.chunk.as_ref() else {
        return;
    };
    let mut syms: Vec<crate::core::value::Symbol> = Vec::new();
    for inst in &chunk.code {
        let (Inst::Global(s) | Inst::GlobalIc { sym: s, .. }) = inst else {
            continue;
        };
        if !syms.contains(s) && matches!(heap.env_get(env, *s), Some(Value::Float(_))) {
            syms.push(*s);
        }
    }
    let _ = arm.float_globals.set(syms.into_boxed_slice());
}

/// Re-observe whether `dbg_name` still resolves to **this very arm**, into
/// [`CompiledArm::self_global_ok`]. Called at each tiering election, before lowering, so a
/// `def` that rebound the name (which bumps the epoch and invalidates the arm) is seen by
/// the recompile. A cache miss, a non-closure binding, or an arity that selects a different
/// arm all answer `false` — the safe direction, costing only the direct-call optimisation.
#[cfg(feature = "jit")]
pub(super) fn record_self_global_ok(arm: &CompiledArm, heap: &Heap, env: EnvId) {
    use std::sync::atomic::Ordering::Relaxed;
    let ok = match arm.dbg_name.and_then(|s| heap.env_get(env, s)) {
        Some(Value::Fn(id)) => {
            super::cached_arm_for(heap, id, arm.nrequired).is_some_and(|other| other.uid == arm.uid)
        }
        _ => false,
    };
    arm.self_global_ok.store(ok, Relaxed);
}

/// A self-tail loop that has spun this many back-edges while its arm sits QUEUED
/// compiles synchronously (`jit_compile_now`): a bounded ~ms block beats an
/// unbounded interpreted tail (sieve's p=2 `mark` pass raced the cold-start
/// background compile for ~500k interpreted iterations; a short-lived arm never
/// accumulates this many edges). Checked in `exec_chunk`'s back-edge exit and
/// acted on in `vm_run_bc`'s tier hook.
#[cfg(feature = "jit")]
pub(crate) const JIT_QUEUED_SYNC_EDGES: u32 = 2048;

/// Compile `arm`'s small native NOW, on the calling thread — the spinning-loop
/// escape hatch (see [`JIT_QUEUED_SYNC_EDGES`]). The arm must be
/// `QUEUED`; re-checked under the module lock so a background compile that beat
/// us is not repeated. Mirrors the background `compile` closure's install path
/// (pointer store + keepalive); a panic bails just this arm (the poison latch
/// stays with the background thread — this path is for one already-elected arm).
#[cfg(feature = "jit")]
pub(crate) fn jit_compile_now(heap: &Heap, arm: &Arc<CompiledArm>, base: usize) {
    use std::sync::atomic::Ordering::{Acquire, Release};
    // A peer's identical shared arm may already be compiled + published — install
    // that instead of blocking on the module lock (held across every compile) to
    // lower it again. This is the spinning-loop escape hatch: any valid native
    // pointer ends the spin.
    if let Some(key) = arm.share_key
        && let Some((ptr, epoch)) = heap.jit_shared_lookup(key)
        && epoch == heap.global_epoch()
        && !ptr.is_null()
        && ptr != crate::jit::BAILED
        && ptr != crate::jit::QUEUED
        && ActiveBackend::may_adopt_shared_code(arm)
    {
        {
            if crate::diagnostics::debug_flags::jit_bail_trace() {
                let name = arm.dbg_label();
                eprintln!(
                    "[jit-ir] arm={name} adopted-shared-code-compile-now nslots={} (not lowered here, emits no IR dump)",
                    arm.nslots
                );
            }
        }
        arm.compile_relocations
            .store(heap.code_relocations(), Release);
        arm.compile_epoch.store(epoch, Release);
        // Over the queue marker, or not at all (a latch or a peer's install came first).
        if !arm.replace_code(crate::jit::QUEUED, ptr) {
            return;
        }
        arm.shared_published
            .store(true, std::sync::atomic::Ordering::Relaxed);
        return;
    }
    // Snapshot the live frame's slot tags exactly as jit_tier's enqueuer does
    // (used to type-specialize float arms).
    let slot_tags = super::profile_slot_tags(heap, arm, base);
    // The background compiler holds this lock across each of its compiles, so the wait
    // here is whatever it is in the middle of: `BROOD_STALL_MS` names a long one.
    let mut jit = {
        let _waiting = crate::core::heap::stall_guard("jit-compile-now: waiting for the module");
        crate::jit::GLOBAL_JIT
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    };
    if arm.jit_code.load(Acquire) != crate::jit::QUEUED {
        return; // the background thread finished it while we waited for the lock
    }
    let _lowering = crate::core::heap::stall_guard("jit-compile-now: lowering");
    let lowered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        jit.lower_arm(arm, &slot_tags)
    }));
    drop(jit); // install the pointer outside the module lock
    match lowered {
        Ok(Some(ptr)) => {
            // Same keepalive contract as the background path: installed native code
            // bakes raw pointers into the arm's chunk ConstVals — keep the arm alive.
            // Pinned BEFORE the pointer is published, so a cold-path lookup by code
            // never misses an installed arm.
            jit_keep_alive(ptr, arm);
            // Over the queue marker, or not at all: the arm may have been latched while this
            // lowered (KI-309), or the background compile installed first.
            arm.replace_code(crate::jit::QUEUED, ptr as *mut u8);
        }
        Ok(None) | Err(_) => {
            let reason = trace_lower_declined(arm, false);
            arm.latch_bailed(&reason)
        }
    }
}

/// [`jit_compile_now`] for an arm about to be called with `args` rather than one whose
/// frame is already built — a higher-order callback (`hof_apply_native`). The args are
/// staged as a frame only so the slot profile reads their types, then dropped.
#[cfg(feature = "jit")]
pub(crate) fn jit_compile_now_with_args(heap: &mut Heap, arm: &Arc<CompiledArm>, args: &[Value]) {
    let base = heap.roots_len();
    for a in args {
        heap.push_root(*a);
    }
    heap.extend_roots_to_nil(base + arm.nslots);
    jit_compile_now(heap, arm, base);
    heap.truncate_roots(base);
}

/// Announce that a lowering attempt came back `Ok(None)`. Every refusal inside
/// `jit_lower_arm` that travels out through a `?` on a helper bypasses the reasoned
/// traces, so without this the only visible evidence is the arm silently being BAILED.
#[cfg(feature = "jit")]
pub(super) fn trace_lower_declined(arm: &CompiledArm, inlined: bool) -> String {
    // Take (and clear) any mid-emit reason regardless of the trace flag, so a reason
    // recorded under a flagless run cannot leak into a later flagged one.
    // `lowering-returned-none` is the FALLBACK — it means a give-up path that recorded
    // nothing, which no path should do. Seeing it in a trace is itself the finding.
    let (reason, detail) =
        super::take_mid_emit_reason().unwrap_or(("lowering-returned-none", None));
    let detail = detail.map_or_default(|detail| format!(":{detail}"));
    if crate::diagnostics::debug_flags::jit_bail_trace() {
        let name = arm.dbg_label();
        eprintln!(
            "[jit-bail] arm={name} reason={reason}{detail} inlined={inlined} nslots={} ops=[{}]",
            arm.nslots,
            arm.dbg_opcodes()
        );
    }
    format!("{reason}{detail}")
}

/// Latch `slot` off the native tier: the arm's own `jit_code` through
/// [`CompiledArm::latch_bailed`], keeping `reason`; an inline upgrade's `inline_code` plainly,
/// since the small native it would have replaced keeps running and the arm is not bailed.
fn latch_slot(arm: &CompiledArm, inlined: bool, reason: &str) {
    if inlined {
        arm.inline_code
            .store(crate::jit::BAILED, std::sync::atomic::Ordering::Release);
    } else {
        arm.latch_bailed(reason);
    }
}

#[cfg(feature = "jit")]
pub(crate) static JIT_COMPILER: std::sync::LazyLock<JitCompiler> = std::sync::LazyLock::new(|| {
    use std::sync::mpsc::{TryRecvError, sync_channel};
    let (ptx, prx) = sync_channel::<Option<JitWorkItem>>(256);
    let (dtx, drx) = sync_channel::<JitWorkItem>(256);
    // A refused thread (`EAGAIN` under a thread or task limit) must not panic: this is a
    // `LazyLock`, and a panic would poison it for every later tiering attempt in every
    // process. The receivers drop with the closure, so each enqueue fails and its arm
    // stays on the bytecode VM, exactly as a full queue does.
    let started = std::thread::Builder::new()
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
            // The compiler thread's OWN publish map — (runtime_tag, share_key) →
            // (code, compile_epoch) for every shared arm it has lowered. Consulted
            // before lowering so the Nth queued copy of the same shared closure
            // installs the first copy's code instead of recompiling. Thread-local
            // by construction (this closure never escapes), so no locking. Entries
            // for a dropped runtime are inert garbage (a few words each; the code
            // itself lives forever in GLOBAL_JIT regardless — see the keepalive).
            // Keyed on the arm's polymorphic-slot mask too (`jit_any_deopt_feedback`), and
            // on its float-context flag (`float_deopt_feedback`, in the mask's top bit): a
            // re-lowering the arm asked for after deopts must not be answered with the
            // code it is replacing — the first version of that feedback re-installed the
            // same pointer from here and `json/emit` went on deopting.
            let mut published: std::collections::HashMap<(u64, (u64, u16), u32), (usize, u64)> =
                std::collections::HashMap::new();
            // The inlined-upgrade counterpart (the deferred queue has the same
            // per-process-copy flood shape). Separate map: a small-arm pointer
            // must never install into `inline_code` (different frame sizing —
            // `inline_nslots`), and vice versa.
            let mut published_inline: std::collections::HashMap<
                (u64, (u64, u16), u32),
                (usize, u64),
            > = std::collections::HashMap::new();
            // Lower one work item: `inlined=false` → the small original arm, store into
            // `jit_code`; `inlined=true` → the re-derived inlined body, store into
            // `inline_code` (jit_tier swaps it into `jit_code` later, epoch-bumped).
            let mut compile = |arm: &Arc<CompiledArm>,
                               slot_tags: &[u8],
                               rt_tag: u64,
                               inlined: bool| {
                let slot = if inlined {
                    &arm.inline_code
                } else {
                    &arm.jit_code
                };
                // The epoch this lowering starts under — the one `jit_tier` stamped on the arm
                // when it queued the work, or a later one an entry moved it to after checking
                // that the code is still right (KI-303). A `def` that invalidates the arm (or
                // a deopt re-lowering) resets it meanwhile (`reset_native_state` counts the
                // reset, then nulls `jit_code` and `inline_code`), and the next activation
                // re-queues it under the new epoch; a lowering that finished after that used
                // to be stored anyway: into `jit_code`, where the re-queued compile then found
                // the slot filled and skipped, or into `inline_code`, swapped in on the next
                // entry — and the dedupe map below recorded it under the NEW epoch, serving it
                // to the re-queued copy. A leaf-spliced `step` kept running after `step` was
                // redefined (undef_test, 10 of 10). So what decides is the reset count: the
                // stamp also moves for a rebind the code does not depend on.
                let lowered_at = arm.compile_epoch.load(std::sync::atomic::Ordering::SeqCst);
                let resets_at = arm.native_resets.load(std::sync::atomic::Ordering::SeqCst);
                let still_valid = |arm: &CompiledArm| {
                    use std::sync::atomic::Ordering::SeqCst;
                    if arm.native_resets.load(SeqCst) != resets_at {
                        return false;
                    }
                    // An inlined body is swapped in over the small native it was lowered
                    // beside, so that native must still be the arm's.
                    !inlined || {
                        let code = arm.jit_code.load(SeqCst);
                        !code.is_null() && code != crate::jit::QUEUED && code != crate::jit::BAILED
                    }
                };
                // Publish `ptr` into `slot` while it is still valid, and re-check AFTER the
                // store: a reset landing between the check and the store would otherwise be
                // undone by it. Taken back, the slot holds what the reset left (the small slot
                // is QUEUED again by then, its fresh compile already behind this one).
                let install = |ptr: *mut u8| -> bool {
                    use std::sync::atomic::Ordering::SeqCst;
                    if !still_valid(arm) {
                        return false;
                    }
                    // Only over the queue marker this compile was started for — QUEUED for
                    // the small slot, null for an upgrade's. Anything else was decided while
                    // it lowered, and stands: a suspend-host latch (`BAILED` is final; storing
                    // over it put a latched arm back on native code, KI-309) or the pointer a
                    // spinning loop's synchronous compile installed first.
                    let expected = if inlined {
                        std::ptr::null_mut()
                    } else {
                        crate::jit::QUEUED
                    };
                    if slot
                        .compare_exchange(expected, ptr, SeqCst, SeqCst)
                        .is_err()
                    {
                        return false;
                    }
                    if still_valid(arm) {
                        if inlined {
                            super::INLINE_LANDED.fetch_add(1, SeqCst);
                        }
                        return true;
                    }
                    let _ = slot.compare_exchange(ptr, expected, SeqCst, SeqCst);
                    false
                };
                // Already resolved (a spinning loop sync-compiled it via
                // `jit_compile_now`, or it was bailed) — don't compile it twice.
                // A queued small arm holds QUEUED here; a queued inlined upgrade
                // holds null (its queue marker is `inline_queued`).
                {
                    let existing = slot.load(std::sync::atomic::Ordering::Acquire);
                    if !existing.is_null() && existing != crate::jit::QUEUED {
                        return;
                    }
                }
                if codegen_poisoned {
                    // The last untraced BAILED route. A single codegen PANIC latches
                    // `codegen_poisoned` and every arm queued after it is bailed here with no
                    // attempt — so one bad lowering silently disables the JIT for everything
                    // that follows, and the only visible symptom is code mysteriously running
                    // on the VM. Announce it (once) rather than leaving it to be deduced.
                    if crate::diagnostics::debug_flags::jit_bail_trace() {
                        let name = arm.dbg_label();
                        eprintln!("[jit-bail] arm={name} reason=codegen-poisoned-earlier");
                    }
                    latch_slot(arm, inlined, "codegen-poisoned-earlier");
                    return;
                }
                // Cross-process dedupe: a peer's identical arm (same shared
                // closure, same runtime) already lowered by THIS thread — and at
                // the same epoch this copy was enqueued at — installs directly.
                // A `def`/compaction between the two enqueues bumps the epoch, so
                // a stale entry never installs (and the runner's live-epoch guard
                // in `jit_tier` re-checks on every native entry regardless). No
                // keepalive push: the first copy's push owns the code's chunk.
                let poly = arm
                    .jit_poly_slots
                    .load(std::sync::atomic::Ordering::Relaxed)
                    | (u32::from(
                        arm.jit_float_context
                            .load(std::sync::atomic::Ordering::Relaxed),
                    ) << 31);
                if let Some(key) = arm.share_key {
                    let map = if inlined {
                        &published_inline
                    } else {
                        &published
                    };
                    if let Some(&(ptr, epoch)) = map.get(&(rt_tag, key, poly))
                        && epoch == lowered_at
                    {
                        install(ptr as *mut u8);
                        return;
                    }
                }
                let mut jit = crate::jit::GLOBAL_JIT
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                #[cfg(feature = "perf-stats")]
                let t0 = web_time::Instant::now();
                let lowered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    if inlined {
                        // A deferred item is the inlined upgrade when a derivation
                        // exists; otherwise it is the xcall RE-LOWERING of the arm's
                        // own body (same chunk/frame/checkpoint, hot emission armed —
                        // §7.5). Same staging slot (`inline_code`), same swap channel.
                        if arm.inline_name.is_some() || arm.leaf.is_some() {
                            jit.lower_inlined_arm(arm, slot_tags)
                        } else {
                            jit.lower_arm_hot(arm, slot_tags)
                        }
                    } else {
                        jit.lower_arm(arm, slot_tags)
                    }
                }));
                #[cfg(feature = "perf-stats")]
                // `BROOD_TRACE_LOWER`: time each lowering.
                if crate::diagnostics::debug_flags::trace_lower() {
                    eprintln!(
                        "[lower] {:?} arm={} inlined={}",
                        t0.elapsed(),
                        arm.dbg_label(),
                        inlined
                    );
                }
                drop(jit); // install the pointer outside the module lock
                match lowered {
                    Ok(Some(ptr)) => {
                        // The installed native code lives forever in GLOBAL_JIT and bakes raw
                        // pointers into this arm's chunk `ConstVal`s. Keep the arm (hence its
                        // chunk) alive permanently so those pointers never dangle when the
                        // closure / call-IC that referenced it is dropped (e.g. a green process
                        // exits) — the bug-#2 use-after-free: a freed ConstVal chunk fed garbage
                        // consts (a garbage map_get key) into still-installed native code.
                        // Pinned before the pointer is published (see `jit_keep_alive`).
                        jit_keep_alive(ptr, arm);
                        // Remember it for the queued copies still behind this one — under the
                        // epoch it was lowered at, and only if it was still valid to install.
                        let installed = install(ptr as *mut u8);
                        if let Some(key) = arm.share_key.filter(|_| installed) {
                            let map = if inlined {
                                &mut published_inline
                            } else {
                                &mut published
                            };
                            map.insert((rt_tag, key, poly), (ptr as usize, lowered_at));
                        }
                    }
                    Ok(None) => {
                        let reason = trace_lower_declined(arm, inlined);
                        latch_slot(arm, inlined, &reason);
                    }
                    Err(_) => {
                        // The panic that poisons the compiler for the rest of the process.
                        // `catch_unwind` swallows it, so without this the FIRST domino is
                        // invisible and only its consequences are seen.
                        //
                        // Reported UNCONDITIONALLY (not behind `BROOD_JIT_BAIL_TRACE`): this
                        // fires at most once per process and turns the JIT off for everything
                        // queued after it — a whole-process capability loss whose only symptom
                        // is otherwise "the program got slower". A diagnostic you must have
                        // armed in advance is a diagnostic that is absent when it matters.
                        {
                            let name = arm.dbg_label();
                            eprintln!(
                                "[jit-bail] arm={name} reason=CODEGEN-PANICKED — the JIT is now \
                                 OFF for the rest of this process (every arm queued after this \
                                 one bails untried). Please report this with the program."
                            );
                        }
                        codegen_poisoned = true;
                        latch_slot(arm, inlined, "codegen-panicked");
                    }
                }
            };
            loop {
                // 1. Drain the entire primary queue before touching deferred — the
                //    initial-tier work always wins the compiler.
                match prx.try_recv() {
                    Ok(Some((arm, tags, rt_tag))) => {
                        compile(&arm, &tags, rt_tag, false);
                        continue;
                    }
                    Ok(None) => continue, // a `defer` wake-up: re-check both queues

                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => break,
                }
                // 2. Primary empty: take one deferred inlined upgrade if any.
                match drx.try_recv() {
                    Ok((arm, tags, rt_tag)) => {
                        compile(&arm, &tags, rt_tag, true);
                        continue;
                    }
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => {}
                }
                // 3. Both empty: block on the primary. A deferred item's `defer` sends a
                //    `None` wake-up here after queueing it, so this needs no timeout.
                match prx.recv() {
                    Ok(Some((arm, tags, rt_tag))) => compile(&arm, &tags, rt_tag, false),
                    Ok(None) => {}
                    Err(_) => break,
                }
            }
        });
    if let Err(error) = started {
        eprintln!(
            "brood: cannot start the JIT compiler thread ({error}); code runs on the bytecode VM"
        );
    }
    JitCompiler {
        primary: ptx,
        deferred: dtx,
    }
});
