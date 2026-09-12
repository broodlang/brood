//! The background JIT compiler (ADR-101 1b): one lazily-spawned OS thread draining a
//! queue of arms to lower, the synchronous `jit_compile_now` for the cases that cannot
//! wait, and the float-global profile a lowering reads (`BROOD_NO_FLOAT_GLOBAL`).

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
    /// Primary (initial-tier) queue: the small ORIGINAL arm. Drained first, always.
    pub(crate) primary: std::sync::mpsc::SyncSender<JitWorkItem>,
    /// Deferred (lower-priority) queue: the re-derived **inlined** upgrade. The bg thread
    /// pulls from it only when `primary` is empty — so under a spawn-style initial-tier
    /// storm (thousands of short-lived processes tiering their small arms) the inlined
    /// upgrades sit behind the backlog and never compete; a long-lived workload (fib 35)
    /// drains its primary, then the deferred inlined compile lands and the swap fires.
    pub(crate) deferred: std::sync::mpsc::SyncSender<JitWorkItem>,
}

/// Permanent keep-alive for every `CompiledArm` whose native code was installed into the
/// process-lifetime `GLOBAL_JIT` module. The native code bakes raw pointers into the arm's
/// chunk `ConstVal`s (read by `brood_rt_const_load`), so the arm (chunk) must outlive the
/// code — i.e. forever. Without this, the arm's only other owners are the closure / call-IC,
/// which are dropped when a closure is rebound or a green process exits, freeing the chunk
/// out from under still-installed native code (bug #2: a dangling ConstVal → garbage const).
#[cfg(feature = "jit")]
pub(crate) static JIT_ARM_KEEPALIVE: std::sync::Mutex<Vec<Arc<CompiledArm>>> =
    std::sync::Mutex::new(Vec::new());

/// Is float-global unboxing enabled? **Default ON** (`BROOD_NO_FLOAT_GLOBAL` opts out —
/// the A/B baseline lever). Read once: all processes of a runtime share an arm's compiled
/// code, so the eligibility decision must be deterministic across them.
#[cfg(feature = "jit")]
pub(super) fn float_global_unbox_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("BROOD_NO_FLOAT_GLOBAL").is_none())
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
#[cfg(feature = "jit")]
pub(super) fn preload_arm_globals(arm: &CompiledArm, heap: &mut Heap, env: EnvId) {
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
        return;
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
                if let Some(callee) = cached_arm_for(heap, id, argc) {
                    if let Some(c) = callee.chunk.as_ref() {
                        globals_of(c, &mut syms);
                    }
                }
            }
        }
    }
    // Only a QUALIFIED unbound name can load anything; `global_miss` applies the same
    // exclusions the compile-time hooks do. The env is rooted across each load by the
    // callee; nothing else here is a heap handle.
    let env_base = heap.env_roots_len();
    let env_root = heap.root_env(env);
    for sym in syms {
        if crate::core::value::symbol_name_ref(sym).contains('/') {
            let env = heap.read_root_env(env_root);
            if heap.env_get(env, sym).is_none() {
                let _ = crate::eval::derive::global_miss(heap, env, sym);
            }
        }
    }
    heap.truncate_env_roots(env_base);
}

/// Snapshot which free globals this arm reads currently hold a `Value::Float` into
/// [`CompiledArm::float_globals`] (see that field for why the param profile alone is not
/// enough). Runs on the thread that wins the tiering election — the only place that has
/// both the arm and a `Heap`; the lowering thread has no heap. Once per arm: the
/// `OnceLock` makes a later observation a no-op, which is what keeps a shared arm's
/// lowering deterministic across the processes of a runtime.
#[cfg(feature = "jit")]
pub(super) fn record_float_globals(arm: &CompiledArm, heap: &Heap, env: EnvId) {
    if !float_global_unbox_enabled() || arm.float_globals.get().is_some() {
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
    if let Some(key) = arm.share_key {
        if let Some((ptr, epoch)) = heap.jit_shared_lookup(key) {
            if epoch == heap.global_epoch()
                && !ptr.is_null()
                && ptr != crate::jit::BAILED
                && ptr != crate::jit::QUEUED
                && ActiveBackend::may_adopt_shared_code(arm)
            {
                {
                    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
                    if *ON.get_or_init(|| std::env::var_os("BROOD_JIT_BAIL_TRACE").is_some()) {
                        let name = arm
                            .dbg_name
                            .map(crate::core::value::symbol_name_ref)
                            .unwrap_or("<closure>");
                        eprintln!("[jit-ir] arm={name} adopted-shared-code-compile-now nslots={} (not lowered here, emits no IR dump)", arm.nslots);
                    }
                }
                arm.compile_epoch.store(epoch, Release);
                arm.jit_code.store(ptr, Release);
                arm.shared_published
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                return;
            }
        }
    }
    // Snapshot the live frame's slot tags exactly as jit_tier's enqueuer does
    // (used to type-specialize float arms).
    let slot_tags: Vec<u8> = (0..arm.nslots)
        .map(|i| crate::core::value::tag(heap.root_at(base + i)) as u8)
        .collect();
    let mut jit = crate::jit::GLOBAL_JIT
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if arm.jit_code.load(Acquire) != crate::jit::QUEUED {
        return; // the background thread finished it while we waited for the lock
    }
    let lowered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        jit.lower_arm(arm, &slot_tags)
    }));
    drop(jit); // install the pointer outside the module lock
    match lowered {
        Ok(Some(ptr)) => {
            arm.jit_code.store(ptr as *mut u8, Release);
            // Same keepalive contract as the background path: installed native code
            // bakes raw pointers into the arm's chunk ConstVals — keep the arm alive.
            JIT_ARM_KEEPALIVE
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(arm.clone());
        }
        Ok(None) | Err(_) => {
            trace_lower_declined(arm, false);
            arm.jit_code.store(crate::jit::BAILED, Release)
        }
    }
}

/// Announce that a lowering attempt came back `Ok(None)`. Every refusal inside
/// `jit_lower_arm` that travels out through a `?` on a helper bypasses the reasoned
/// traces, so without this the only visible evidence is the arm silently being BAILED.
#[cfg(feature = "jit")]
pub(super) fn trace_lower_declined(arm: &CompiledArm, inlined: bool) {
    // Take (and clear) any mid-emit reason regardless of the trace flag, so a reason
    // recorded under a flagless run cannot leak into a later flagged one.
    // `lowering-returned-none` is the FALLBACK — it means a give-up path that recorded
    // nothing, which every path now does. Seeing it in a trace is itself the finding.
    let (reason, detail) =
        super::take_mid_emit_reason().unwrap_or(("lowering-returned-none", None));
    let detail = detail.map(|d| format!(":{d}")).unwrap_or_default();
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("BROOD_JIT_BAIL_TRACE").is_some()) {
        let name = arm
            .dbg_name
            .map(crate::core::value::symbol_name_ref)
            .unwrap_or("<closure>");
        let ops: Vec<&str> = arm
            .chunk
            .as_ref()
            .map(|c| c.code.as_slice())
            .unwrap_or(&[])
            .iter()
            .map(crate::eval::compile::jit_plan::codegen::inst_opcode_name)
            .collect();
        eprintln!(
            "[jit-bail] arm={name} reason={reason}{detail} inlined={inlined} nslots={} ops=[{}]",
            arm.nslots,
            ops.join(" ")
        );
    }
}

#[cfg(feature = "jit")]
pub(crate) static JIT_COMPILER: std::sync::LazyLock<JitCompiler> = std::sync::LazyLock::new(|| {
    use std::sync::atomic::Ordering::Release;
    use std::sync::mpsc::{sync_channel, TryRecvError};
    let (ptx, prx) = sync_channel::<JitWorkItem>(256);
    let (dtx, drx) = sync_channel::<JitWorkItem>(256);
    // The bg thread's own handle to the deferred queue, for the §7.1 hot-admission
    // re-enqueue (a gate-refused arm handed straight to the hot stage). It must not
    // touch `JIT_COMPILER` — the thread starts inside this LazyLock's initializer.
    let dtx_bg = dtx.clone();
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
            // The compiler thread's OWN publish map — (runtime_tag, share_key) →
            // (code, compile_epoch) for every shared arm it has lowered. Consulted
            // before lowering so the Nth queued copy of the same shared closure
            // installs the first copy's code instead of recompiling. Thread-local
            // by construction (this closure never escapes), so no locking. Entries
            // for a dropped runtime are inert garbage (a few words each; the code
            // itself lives forever in GLOBAL_JIT regardless — see the keepalive).
            let mut published: std::collections::HashMap<(u64, (u64, u16)), (usize, u64)> =
                std::collections::HashMap::new();
            // The inlined-upgrade counterpart (the deferred queue has the same
            // per-process-copy flood shape). Separate map: a small-arm pointer
            // must never install into `inline_code` (different frame sizing —
            // `inline_nslots`), and vice versa.
            let mut published_inline: std::collections::HashMap<(u64, (u64, u16)), (usize, u64)> =
                std::collections::HashMap::new();
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
                    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
                    if *ON.get_or_init(|| std::env::var_os("BROOD_JIT_BAIL_TRACE").is_some()) {
                        let name = arm
                            .dbg_name
                            .map(crate::core::value::symbol_name_ref)
                            .unwrap_or("<closure>");
                        eprintln!("[jit-bail] arm={name} reason=codegen-poisoned-earlier");
                    }
                    slot.store(crate::jit::BAILED, Release);
                    return;
                }
                // Cross-process dedupe: a peer's identical arm (same shared
                // closure, same runtime) already lowered by THIS thread — and at
                // the same epoch this copy was enqueued at — installs directly.
                // A `def`/compaction between the two enqueues bumps the epoch, so
                // a stale entry never installs (and the runner's live-epoch guard
                // in `jit_tier` re-checks on every native entry regardless). No
                // keepalive push: the first copy's push owns the code's chunk.
                if let Some(key) = arm.share_key {
                    let map = if inlined {
                        &published_inline
                    } else {
                        &published
                    };
                    if let Some(&(ptr, epoch)) = map.get(&(rt_tag, key)) {
                        if epoch == arm.compile_epoch.load(std::sync::atomic::Ordering::Acquire) {
                            slot.store(ptr as *mut u8, Release);
                            return;
                        }
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
                if std::env::var_os("BROOD_COMPILE_TRACE").is_some() {
                    eprintln!(
                        "[compile] {:?} arm={} inlined={}",
                        t0.elapsed(),
                        arm.dbg_name
                            .map(crate::core::value::symbol_name_ref)
                            .unwrap_or("<closure>"),
                        inlined
                    );
                }
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
                        // Remember it for the queued copies still behind this one.
                        if let Some(key) = arm.share_key {
                            let map = if inlined {
                                &mut published_inline
                            } else {
                                &mut published
                            };
                            map.insert(
                                (rt_tag, key),
                                (
                                    ptr as usize,
                                    arm.compile_epoch.load(std::sync::atomic::Ordering::Acquire),
                                ),
                            );
                        }
                    }
                    Ok(None) => {
                        trace_lower_declined(arm, inlined);
                        slot.store(crate::jit::BAILED, Release);
                        // §7.1 hot admission (`BROOD_XADMIT=1`, experiment): an arm the
                        // profitability gate refused keeps running on the VM, but is
                        // handed to the HOT stage — the deferred queue, gate skipped,
                        // frame-size capped — where both of step 2's measured costs are
                        // absent (the compile is deferred; the calls emit the inline
                        // blob). Its pointer stages in `inline_code`; `jit_tier`'s
                        // BAILED path installs it. `inline_queued` is the once latch;
                        // `dtx_bg` (not `JIT_COMPILER.deferred`) because this thread
                        // starts inside that LazyLock's initializer.
                        // Say whether hot admission fired, and if not, which condition
                        // refused. Without this the lever is unfalsifiable: an arm it
                        // silently declines makes both sides of an A/B run identical code
                        // and the result reads as "no effect" — which is how KI-109 came to
                        // record lever 3 as *measured noise* for `row-sum`, an arm whose
                        // frame (nslots=14) the cap (8) had always excluded.
                        if xadmit_enabled() {
                            xadmit_trace(arm, inlined);
                        }
                        if !inlined
                            && xadmit_enabled()
                            && arm.inline_name.is_none()
                            && arm.leaf.is_none()
                            && arm.dbg_name.is_some()
                            && arm.nslots <= xadmit_max_nslots()
                            && crate::eval::compile::jit_plan::codegen::plan_general_lowering(
                                arm, slot_tags,
                            )
                            .is_err()
                            && !arm
                                .inline_queued
                                .swap(true, std::sync::atomic::Ordering::AcqRel)
                            && dtx_bg
                                .try_send((arm.clone(), slot_tags.to_vec(), rt_tag))
                                .is_err()
                        {
                            arm.inline_queued
                                .store(false, std::sync::atomic::Ordering::Relaxed);
                        }
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
                            let name = arm
                                .dbg_name
                                .map(crate::core::value::symbol_name_ref)
                                .unwrap_or("<closure>");
                            eprintln!(
                                "[jit-bail] arm={name} reason=CODEGEN-PANICKED — the JIT is now \
                                 OFF for the rest of this process (every arm queued after this \
                                 one bails untried). Please report this with the program."
                            );
                        }
                        codegen_poisoned = true;
                        slot.store(crate::jit::BAILED, Release);
                    }
                }
            };
            loop {
                // 1. Drain the entire primary queue before touching deferred — the
                //    initial-tier work always wins the compiler.
                match prx.try_recv() {
                    Ok((arm, tags, rt_tag)) => {
                        compile(&arm, &tags, rt_tag, false);
                        continue;
                    }
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
                // 3. Both empty: block on the primary (initial tier latency matters), but
                //    only briefly — so a deferred item enqueued while we slept is picked up
                //    promptly once primary stays quiet. A 1ms idle poll is free (the thread
                //    is otherwise sleeping) and never delays a primary send (which wakes it).
                match prx.recv_timeout(std::time::Duration::from_millis(1)) {
                    Ok((arm, tags, rt_tag)) => compile(&arm, &tags, rt_tag, false),
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
