#![cfg(feature = "jit")]
//! JIT tiering runtime glue (extracted from mod.rs).
use super::*;

// Everything this module asks of a backend goes through the contract, never through a concrete
// one: `lower_arm`/`lower_inlined_arm` from the two places below (`jit_compile_now` and the
// `JIT_COMPILER` thread — the whole production codegen surface), plus the three tiering
// advisories, which are associated fns precisely so consulting them per activation costs no
// `GLOBAL_JIT` lock. See `crate::jit::backend`.
use crate::jit::{ActiveBackend, JitBackend};

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
    primary: std::sync::mpsc::SyncSender<JitWorkItem>,
    /// Deferred (lower-priority) queue: the re-derived **inlined** upgrade. The bg thread
    /// pulls from it only when `primary` is empty — so under a spawn-style initial-tier
    /// storm (thousands of short-lived processes tiering their small arms) the inlined
    /// upgrades sit behind the backlog and never compete; a long-lived workload (fib 35)
    /// drains its primary, then the deferred inlined compile lands and the swap fires.
    deferred: std::sync::mpsc::SyncSender<JitWorkItem>,
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
fn float_global_unbox_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("BROOD_NO_FLOAT_GLOBAL").is_none())
}

/// Snapshot which free globals this arm reads currently hold a `Value::Float` into
/// [`CompiledArm::float_globals`] (see that field for why the param profile alone is not
/// enough). Runs on the thread that wins the tiering election — the only place that has
/// both the arm and a `Heap`; the lowering thread has no heap. Once per arm: the
/// `OnceLock` makes a later observation a no-op, which is what keeps a shared arm's
/// lowering deterministic across the processes of a runtime.
#[cfg(feature = "jit")]
fn record_float_globals(arm: &CompiledArm, heap: &Heap, env: EnvId) {
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
fn record_self_global_ok(arm: &CompiledArm, heap: &Heap, env: EnvId) {
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
fn trace_lower_declined(arm: &CompiledArm, inlined: bool) {
    // Take (and clear) any mid-emit reason regardless of the trace flag, so a reason
    // recorded under a flagless run cannot leak into a later flagged one.
    let reason = super::take_mid_emit_reason().unwrap_or("lowering-returned-none");
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
            "[jit-bail] arm={name} reason={reason} inlined={inlined} nslots={} ops=[{}]",
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
                        if !inlined
                            && xadmit_enabled()
                            && arm.inline_name.is_none()
                            && arm.leaf.is_none()
                            && arm.dbg_name.is_some()
                            && arm.nslots <= XCALL_RELOWER_MAX_NSLOTS
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

/// Are all the arm chunk's inlined 2-ary primitives still bound to their native
/// implementations (ADR-096 §4.A epoch-guard, evaluated eagerly)? The JIT lowers
/// `+`/`<`/… to raw machine ops, which is sound only while the head symbol resolves to
/// the matching `%`-native (and arg-map). A `(def + …)` rebinds it; [`resolve_prim`]
/// reads the live global env, so this returns `false` for the redefined operator and
/// the arm must stay on the VM (which dispatches to the new definition). Non-prim
/// instructions can't be invalidated, so they pass. A chunkless arm passes here and is
/// bailed by [`jit_lower_arm`] instead.
#[cfg(feature = "jit")]
pub(crate) fn chunk_ops_all_native(heap: &Heap, arm: &CompiledArm) -> bool {
    let Some(chunk) = arm.chunk.as_ref() else {
        return true;
    };
    chunk_ops_native(heap, chunk)
}

/// [`chunk_ops_all_native`]'s chunk-level core — also used by [`leaf_inline_probe`] to
/// validate a spliced chunk's (foreign, callee-contributed) prims at derivation time.
#[cfg(feature = "jit")]
pub(crate) fn chunk_ops_native(heap: &Heap, chunk: &Chunk) -> bool {
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

/// Stamp [`Heap::jit_stack_limit`] from the live remaining stack, before entering native
/// code (KI-14). Absolute address, so nested native frames compare against it directly with
/// no per-frame bookkeeping.
///
/// **Call this only where the stamp can actually change** — see
/// [`stamp_stack_limit_if_outermost`] for the discipline. The value derives from the
/// running thread's stack bottom, which is fixed for that thread, so it is invariant across
/// a whole native recursion nest and only differs between the root thread and a worker (or
/// between two workers).
///
/// `0` (probe unavailable) disables the prologue check — fail open, matching what
/// [`jit_native_headroom_ok`] already does with `None`.
#[cfg(feature = "jit")]
#[inline]
pub(crate) fn stamp_stack_limit(heap: &mut Heap) {
    let here = &heap as *const _ as usize;
    heap.jit_stack_limit = match stacker::remaining_stack() {
        // Room left: the limit is the address `margin` bytes above the stack bottom.
        Some(left) if left > JIT_STACK_MARGIN_BYTES => here - (left - JIT_STACK_MARGIN_BYTES),
        // **Already inside the margin — trip immediately.** `usize::MAX` is above every
        // address, so the next prologue deopts. Encoding this as "disabled" (the obvious
        // `_ => 0`) inverts the guard: it switches off at exactly the moment it is needed,
        // which is how the first cut of this fix still let the 250 KB alternating-JSON
        // document abort the process.
        Some(_) => usize::MAX,
        // Probe unavailable: 0 = no check, failing open exactly as `jit_native_headroom_ok`
        // does with `None`.
        None => 0,
    };
}

/// Stamp the limit only at the **outermost** native entry — `native_depth` is the caller's
/// pre-increment [`Heap::jit_native_depth`], so `0` means no native frame is on the stack
/// below this one.
///
/// [`Heap::jit_stack_limit`] is an absolute address derived from the running thread's stack
/// bottom, which does not move for the life of that thread. Re-deriving it at every
/// Brood→Brood link therefore recomputed a constant, and `stacker::remaining_stack()` is not
/// free. Worth ~5% on `bintree` (130 → 124 ms, best-of-15 — the 16.3 M-fast-link row).
///
/// It is worth **nothing** on `fib`, which is what this hoist was originally proposed to fix
/// (see the 2026-07-27 devlog): `fib` tiers to the i64 register worker and recurses natively
/// without ever taking a fast link. That regression was the *prologue* guard — specifically a
/// redundant second compare per level — and is fixed in `jit_lower/i64.rs`, not here.
///
/// The stamp is still *live* rather than a constant because a green process resumes on
/// whichever worker the scheduler routes it to, and worker stack bases differ — hence the
/// second stamp point at quantum start in `Process::drive`. That one is what makes this
/// gate safe if a quantum ever ends with the depth counter left raised: without it, a
/// process that migrated would keep comparing against the previous worker's stack.
#[cfg(feature = "jit")]
#[inline]
fn stamp_stack_limit_if_outermost(heap: &mut Heap, native_depth: u32) {
    if native_depth == 0 {
        stamp_stack_limit(heap);
    }
}

/// Cap on native-to-native recursion (see [`Heap::jit_native_depth`]). Past this many
/// native levels, drain the rest of the subtree on the VM (heap frames, bounded by
/// [`MAX_BC_FRAMES`]) so deep non-tail recursion keeps working instead of overflowing the
/// native stack.
///
/// This is a **frame count, not a stack measurement**, so on its own it is only ever
/// right for one frame size: 1500 levels is a few MB of the 16 MB worker stack in a
/// release build, and several times that in a debug build, where it overflowed. Hence
/// [`jit_native_headroom_ok`] — the count stays as the cheap first test, and the actual
/// remaining stack is what decides near the limit.
#[cfg(feature = "jit")]
pub(crate) const JIT_NATIVE_DEPTH_LIMIT: u32 = 1500;

/// Below this native depth, no plausible frame size can exhaust the stack, so the
/// headroom probe is skipped entirely and the hot shallow path (`fib`, `primes`) pays
/// nothing beyond the existing integer compare.
#[cfg(feature = "jit")]
const JIT_HEADROOM_PROBE_FROM: u32 = 64;

/// Stack that must remain before another native link is allowed. Generous: it has to
/// cover the callee's native frame plus whatever Rust the callee re-enters (`apply_value`
/// on an outcome-4 tail chain, the deopt re-runs), and the cost of being wrong is an
/// unrecoverable abort while the cost of being early is a VM-drained subtree.
#[cfg(feature = "jit")]
const JIT_STACK_MARGIN_BYTES: usize = 512 * 1024;

/// Whether there is room on the native stack for another Brood→Brood native link.
///
/// The frame-count cap alone cannot answer this: the same 1500 frames fit comfortably in
/// release and overflow in debug, and a host embedding Brood on a smaller thread stack
/// shifts the answer again. `stacker::remaining_stack` measures the thing that actually
/// matters. Returning `false` is never a correctness problem — the caller falls through to
/// the VM's heap-backed frames, which is where deep recursion belongs anyway.
///
/// `remaining_stack()` is `None` when the platform can't report it; treat that as "fine"
/// and fall back to the count cap, which is the pre-existing behaviour.
#[cfg(feature = "jit")]
#[inline]
pub(crate) fn jit_native_headroom_ok(depth: u32) -> bool {
    if depth < JIT_HEADROOM_PROBE_FROM {
        return true;
    }
    stack_headroom_ok()
}

/// Is there room for another native link, **regardless of recorded depth**? The
/// depth-gated [`jit_native_headroom_ok`] is the hot-path form; this is the one to use
/// where the depth counter can't be trusted to reflect the real native nesting (KI-14):
/// once an arm re-enters through a *native* frame the counter under-reports the true
/// nesting, so the raw headroom probe is the only honest answer.
#[cfg(feature = "jit")]
#[inline]
pub(crate) fn stack_headroom_ok() -> bool {
    match stacker::remaining_stack() {
        Some(left) => left > JIT_STACK_MARGIN_BYTES,
        None => true,
    }
}

/// Run `body` with [`Heap::jit_native_depth`] raised back to `native_depth + 1` — the level
/// of the [`jit_run_fast_link`] frame that is *still on the native stack* while the outcome
/// is being handled.
///
/// Every re-entrant call in that outcome handler must go through this. The natural-looking
/// alternative — restore the depth as soon as the native callee returns, then handle the
/// outcome — makes [`JIT_NATIVE_DEPTH_LIMIT`] stop bounding anything: the outcome-4
/// tail-chain follow-through calls back into the evaluator on this same frame, so a chain of
/// tail-calling delegators oscillates between `native_depth` and `native_depth + 1`
/// indefinitely while the native stack grows, and the process dies of a stack overflow that
/// `try`/`catch` cannot observe and no supervisor can restart (the OS process goes, not the
/// green process). That was KI-11.
#[cfg(feature = "jit")]
fn jit_native_reenter<T>(
    heap: &mut Heap,
    native_depth: u32,
    body: impl FnOnce(&mut Heap) -> T,
) -> T {
    heap.jit_native_depth = native_depth + 1;
    let out = body(heap);
    heap.jit_native_depth = native_depth;
    out
}

/// The result of running a validated native fast-link ([`jit_run_fast_link`]): the call
/// completed (`Done`), raised an error parked for the arm to propagate (`Error`), or could
/// not be fast-linked after all (`Fallthrough` — the IC moved under us; the args have been
/// re-staged for the caller's slow path).
#[cfg(feature = "jit")]
pub(crate) enum FastLinkOutcome {
    /// The call completed; **the result is at the `out` pointer the caller passed in**, not
    /// carried here. Payload-less on purpose: a `Done(Value)` made this a 32-byte enum that
    /// returns through hidden-pointer memory (`sret`), so the value was copied again on a
    /// path whose whole cost was copying. See [`crate::jit::JitArmFn`].
    Done,
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
/// [`crate::jit::JitArmFn`]. On `Fallthrough` the `argc` args are re-staged at
/// `[stage_base, stage_base+argc)` for the caller's slow path.
///
/// `out` is where the result goes on `Done` — passed straight through to the native arm, so
/// the value is stored once by whoever produced it and never loaded back here. **Only** the
/// `Done` outcome writes it. See [`crate::jit::JitArmFn`] for why, and for the GC rule
/// (`out` is not a root; nothing may allocate between the store and the consumer).
#[cfg(feature = "jit")]
#[allow(clippy::too_many_arguments)]
pub(crate) fn jit_run_fast_link(
    heap: &mut Heap,
    argc: usize,
    site: u32,
    head: Symbol,
    epoch: u64,
    stage_base: usize,
    code: usize,
    nslots: usize,
    callee_env: EnvId,
    callee_bases: (u32, u32),
    out: *mut Value,
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
    let f: crate::jit::JitArmFn = unsafe { std::mem::transmute(code as *mut u8) };
    // Named `native_depth`, not `depth`: the deopt arm below binds a `depth` of its own
    // (the checkpoint's VM stack depth) that would otherwise shadow this one.
    let native_depth = heap.jit_native_depth;
    // Root callee_env via env_roots so GC tenure inside the callee forwards it.
    let env_base = heap.env_roots_len();
    let env_root = heap.root_env(callee_env);
    let saved = std::mem::replace(&mut heap.jit_call_env, env_root);
    let saved_fn = std::mem::replace(&mut heap.jit_dbg_fn, head);
    heap.jit_native_depth = native_depth + 1;
    stamp_stack_limit_if_outermost(heap, native_depth);
    let saved_force_vm = heap.jit_force_vm;
    // KI-20: the callee's native code reads its OWN per-arm IC block through the heap
    // cursors (`vm_call_ic_put`/`vm_global_ic_put`/fast-link publishes). Install the callee's
    // bases for the call and restore the caller's around it — exactly as the cloning
    // native-link path in `jit_dispatch_call` does. Without this the callee wrote into the
    // caller's IC slots (and vice versa); never a wrong answer (every probe re-validates
    // `sym`/`argc`/`epoch`, so a crossed entry simply misses) but both arms ran permanently
    // cache-cold, and `dbg_site_loc` reported the wrong site. The bases arrive as args (they
    // rode in the `FastLink` slot), so this is two `Cell` writes, no table lookup.
    let saved_bases = heap.set_ic_bases(callee_bases);
    heap.native_gateway_seq += 1;
    let gw_seq = heap.native_gateway_seq;
    let saved_gw = std::mem::replace(&mut heap.cur_native_gateway, gw_seq);
    let outcome = f(heap as *mut Heap, base as i64, out);
    heap.cur_native_gateway = saved_gw;
    heap.set_ic_bases(saved_bases);
    heap.jit_force_vm = saved_force_vm;
    heap.jit_native_depth = native_depth;
    heap.jit_call_env = saved;
    heap.jit_dbg_fn = saved_fn;
    heap.truncate_env_roots(env_base);
    // Suspend-host latch (see `jit_latch_suspend_host`). The fast link carries no arm
    // reference, so resolve one — only on the token match, which happens at most once
    // per arm ever (cold); the steady-state cost is this one u64 compare. Resolution is
    // by the invoked code pointer against the keep-alive registry (every arm with
    // installed native code is in it, immortally — bug #2's fix), NOT the call IC: the
    // park usually spans a GC, whose epoch bump makes `vm_call_ic_probe` with the
    // pre-call `epoch` decline — measured as 13 dirty parks producing 3 latches, i.e.
    // a latch that mostly failed to hold. The probe stays as the fallback for the one
    // race the scan can miss (an inlined-upgrade swap between invoke and here).
    if heap.blocked_under_gateway == gw_seq {
        jit_latch_dirty_blocked(heap, code, site, head, argc, epoch);
    }
    // KI-11. Three of the outcome arms below re-enter the evaluator — the outcome-4
    // tail-chain follow-through (`apply_value`), and the deopt/preempt re-runs
    // (`vm_resume_deopt` / `vm_apply`). All of them run on THIS native frame, which is
    // still on the stack, so `jit_native_depth` must stay raised across them or the cap
    // stops bounding the native recursion: with it rolled back to `depth`, a chain of
    // tail-calling delegators oscillates between `depth` and `depth+1` forever while the
    // native stack grows without bound, and the process dies of a stack overflow that
    // `try`/`catch` cannot see (the VM and tree-walker both handle the same input). Each
    // re-entrant call below therefore re-raises the depth for its duration; see
    // `jit_native_reenter`. Found via JSONTestSuite's 20k-deep documents; the minimal
    // repro is a three-function cycle returning a destructured tuple.
    // Outcome 0 — the overwhelmingly common case — is handled inline; every other outcome
    // goes to a `#[cold] #[inline(never)]` helper.
    //
    // This is a code-LAYOUT change with no semantic content, and it is worth the indirection
    // because of where this function sits: `perf` puts `jit_run_fast_link` at **24% of
    // `bintree`** — as much as all of that row's native compute — and instruction-level
    // annotation showed the cost spread thin across the prologue/epilogue (register saves,
    // spills at -0x158/-0x160(%rbp)) rather than concentrated in any operation. That is the
    // signature of a large function on a hot path: the deopt/preempt/tail arms below need
    // several `SmallVec`s and many live values, so the compiler sized the frame and saved the
    // registers for them on EVERY call, including the ~all of them that just return a value.
    if outcome == 0 {
        crate::perf_bump!(jit_link_done);
        // The result is already at `*out` — the arm wrote it there. Nothing to load.
        heap.truncate_roots(stage_base);
        return FastLinkOutcome::Done;
    }
    jit_fast_link_cold_outcome(
        heap,
        outcome,
        argc,
        site,
        head,
        epoch,
        stage_base,
        base,
        nslots,
        native_depth,
        callee_env,
        out,
    )
}

/// §7.5 hot re-lowering: the largest frame the inline-blob re-compile is worth — see
/// the profitability comment at the `xcall_relower` gate in `jit_tier`.
#[cfg(feature = "jit")]
const XCALL_RELOWER_MAX_NSLOTS: usize = 8;

/// §7.1 hot admission (`BROOD_XADMIT=1`, experiment): admit profitability-gate-refused
/// named defns at the HOT stage — deferred compile, inline call blob, frame-size cap.
#[cfg(feature = "jit")]
fn xadmit_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("BROOD_XADMIT").is_some_and(|v| v == "1"))
}

/// The suspend-host latch resolution for a fast link whose callee dirty-blocked its
/// worker (`blocked_under_gateway` came back equal to the gateway token). Shared by
/// [`jit_run_fast_link`] and the inline fast-frame path's `brood_rt_xcall_latch`
/// callback (§7.5, `BROOD_XCALL=1`) — cold by construction (at most once per arm ever).
#[cfg(feature = "jit")]
#[cold]
#[inline(never)]
pub(crate) fn jit_latch_dirty_blocked(
    heap: &mut Heap,
    code: usize,
    site: u32,
    head: Symbol,
    argc: usize,
    epoch: u64,
) {
    heap.blocked_under_gateway = 0;
    // Shed THIS site's fast link first, unconditionally: the latch stores `BAILED`
    // into `arm.jit_code`, but the FastLink mirror's hit path (and the raw load JIT'd
    // callers emit) never re-reads `jit_code` — so without this, a long-lived process
    // whose site is already populated keeps entering the latched native and parking
    // dirty forever. The IC bases were restored above, so `site` resolves against the
    // caller's block exactly as the lookup that entered here did.
    heap.vm_fast_link_clear_site(site);
    let scanned = {
        use std::sync::atomic::Ordering::Acquire;
        // Poison-tolerant like the two push sites: a codegen panic (the
        // CODEGEN-PANICKED path) may have poisoned this mutex, and the latch must
        // not turn that into a worker-thread crash.
        let reg = JIT_ARM_KEEPALIVE.lock().unwrap_or_else(|e| e.into_inner());
        reg.iter()
            .find(|a| std::ptr::eq(a.jit_code.load(Acquire), code as *mut u8))
            .cloned()
    };
    if let Some(arm) = scanned.or_else(|| {
        heap.vm_call_ic_probe(site, head, argc as u32, epoch)
            .and_then(|(_, a)| a)
            .map(|(arm, _, _)| arm.arc().clone())
    }) {
        jit_latch_suspend_host(&arm);
    }
}

/// The cold-outcome funnel for the **inline** fast-frame path (§7.5, `BROOD_XCALL=1`):
/// emitted code has already run the ceremony restores, so this is exactly
/// [`jit_fast_link_cold_outcome`] with the caller context read back off the heap.
/// The callee env is `GLOBAL` by the inline path's own guard.
#[cfg(feature = "jit")]
#[allow(clippy::too_many_arguments)]
#[cold]
#[inline(never)]
pub(crate) fn jit_xcall_cold_outcome(
    heap: &mut Heap,
    outcome: i64,
    argc: usize,
    site: u32,
    head: Symbol,
    epoch: u64,
    stage_base: usize,
    nslots: usize,
    out: *mut Value,
) -> i64 {
    let native_depth = heap.jit_native_depth;
    match jit_fast_link_cold_outcome(
        heap,
        outcome,
        argc,
        site,
        head,
        epoch,
        stage_base,
        stage_base,
        nslots,
        native_depth,
        EnvId::GLOBAL,
        out,
    ) {
        FastLinkOutcome::Done => 0,
        FastLinkOutcome::Error => 1,
        FastLinkOutcome::Fallthrough => 2,
    }
}

/// The deopt / preempt / tail-chain / error outcomes of a native fast link — everything
/// except outcome 0. Split out of [`jit_run_fast_link`] and marked `#[cold]`/`#[inline(never)]`
/// so its frame and register pressure are not charged to the hot return path; see the comment
/// at the call site for the measurement that motivated it. Semantics are unchanged: this is the
/// same code, in the same order, with the same comments.
#[cfg(feature = "jit")]
#[allow(clippy::too_many_arguments)]
#[cold]
#[inline(never)]
fn jit_fast_link_cold_outcome(
    heap: &mut Heap,
    outcome: i64,
    argc: usize,
    site: u32,
    head: Symbol,
    epoch: u64,
    stage_base: usize,
    base: usize,
    nslots: usize,
    native_depth: u32,
    callee_env: EnvId,
    out: *mut Value,
) -> FastLinkOutcome {
    // These arms produce their value in Rust (a re-entered `apply_value` / `vm_resume_deopt`),
    // so they write it through `out` themselves — after all of their allocation, which is what
    // keeps the un-rooted `out` slot safe (see `crate::jit::JitArmFn`).
    let done = |v: Value| {
        // SAFETY: `out` is the caller's slot, valid for the whole call (its frame outlives
        // this one), and written exactly once on the Done path.
        unsafe { *out = v };
        FastLinkOutcome::Done
    };
    match outcome {
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
                let g = heap.global();
                return match jit_native_reenter(heap, native_depth, |h| {
                    apply_value(h, staged_callee, &staged_args, g)
                }) {
                    Ok(v) => done(v),
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
            // Resolve the callee's arm. The IC probe is only an *optimisation* for finding
            // it, so a miss must not change behaviour — but it did: the branch below used to
            // be skipped entirely on a miss, falling through to `brood_rt_call_slow`, which
            // **calls the callee again**. By then its native code has already run, so any
            // effect it performed happened twice (KI-18: both arms of a multi-arity fn were
            // entered 50 016 times instead of 50 000, exactly 16 — the deopt-bail threshold
            // — before the arm bailed and the duplication stopped). On a miss, resolve the
            // arm the slow way by name and take the same checkpoint-resume path.
            let resolved = heap
                .vm_call_ic_probe(site, head, argc as u32, epoch)
                .and_then(|(_, a)| a)
                .map(|(arm, cenv, _)| (arm, cenv))
                .or_else(|| {
                    let genv = heap.read_root_env(heap.jit_call_env);
                    match heap.env_get(genv, head) {
                        Some(Value::Fn(id)) => {
                            super::compiled_arm_for(heap, id, argc).map(|a| (a, callee_env))
                        }
                        _ => None,
                    }
                });
            if let Some((arm, cenv)) = resolved {
                // Deopt feedback (see `jit_deopt_feedback`): the fast-link hot path
                // carries no arm reference, so runs go uncounted here — only deopts.
                // Undercounted runs only make a mixed arm bail sooner (conservative).
                if outcome == 1 && arm.deopt_watch {
                    jit_deopt_feedback(&arm);
                }
                // Deopt-resume (see `CompiledArm::ckpt_slot`): resume AT the
                // checkpoint, frame intact — never re-running its side effects.
                // The shape check exists because the IC could have re-resolved to a
                // different arm than the one whose native ran; a mismatched frame can't be
                // resumed and takes the legacy re-run instead. It must be **flag-free** —
                // see [`jit_frame_shape_matches`] (KI-26).
                // `1 | 2` — deopt AND preempt — matching `vm_run_bc`'s handler (KI-18): the
                // journal, not the outcome code, decides whether there is a checkpoint to
                // resume from, and `jit_ckpt_resume` returns `None` on a zero journal. Today
                // a preempt provably observes a zero journal (`emit_self_call` resets it
                // immediately before the tick poll), but that invariant lives in the lowerer
                // while these consumers depend on it silently — and its failure mode is a
                // silently REPEATED side effect, so all four consumers now gate the same way.
                if matches!(outcome, 1 | 2) && jit_frame_shape_matches(&arm, nslots) {
                    if let Some((resume, rip, depth)) =
                        jit_ckpt_resume(heap, arm.arc(), base, nslots)
                    {
                        return match jit_native_reenter(heap, native_depth, |h| {
                            vm_resume_deopt(h, resume, base, cenv, rip, depth)
                        }) {
                            Ok(v) => done(v),
                            Err(e) => {
                                heap.jit_pending_error = Some(e);
                                FastLinkOutcome::Error
                            }
                        };
                    }
                }
                heap.truncate_roots(stage_base);
                return match jit_native_reenter(heap, native_depth, |h| {
                    vm_apply(h, arm, &argv2, cenv)
                }) {
                    Ok(v) => done(v),
                    Err(e) => {
                        heap.jit_pending_error = Some(e);
                        FastLinkOutcome::Error
                    }
                };
            }
            heap.truncate_roots(stage_base);
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
    callee_bases: (u32, u32),
    out: *mut Value,
) -> FastLinkOutcome {
    let n = heap.roots_len();
    let epoch = heap.global_epoch();
    // Elided (free-global) head: the args are the top `argc` operands; the frame starts there.
    let stage_base = n - argc;
    // Over the native-recursion cap → don't link (would overflow the native stack); the args
    // stay staged at `[stage_base, n)` so the slow path drains the recursion on the VM.
    if heap.jit_native_depth >= JIT_NATIVE_DEPTH_LIMIT
        || !jit_native_headroom_ok(heap.jit_native_depth)
    {
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
            matches!(auth, Some((c, ns, e, b)) if c as usize == code && ns == nslots && e == callee_env && b == callee_bases),
            "fast-link mirror desynced from the call IC (site {site}, head {head}): \
             mirror=(code={code:#x}, nslots={nslots}, env={:#x}, bases={callee_bases:?}) \
             auth={auth:?} — the IR's epoch+sym+argc guard should make this unreachable \
             (see FastLink)",
            callee_env.0
        );
    }
    jit_run_fast_link(
        heap,
        argc,
        site,
        head,
        epoch,
        stage_base,
        code,
        nslots,
        callee_env,
        callee_bases,
        out,
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
    // KI-14: probe the stack **unconditionally** here, not `jit_native_headroom_ok(depth)`.
    // That helper skips the probe below depth 64 as a hot-path optimisation, which is sound
    // only where `jit_native_depth` actually counts the recursion. On this path it does not:
    // a JIT'd arm recursing through `brood_rt_call_slow` → `jit_dispatch_call` re-enters
    // Rust every level, yet the depth stays near zero, so the probe was never reached and
    // neither cap ever fired — 100 000 levels of JSON nesting piled a JIT frame plus these
    // Rust frames each, and the worker died on its guard page (an abort `try`/`catch` can't
    // see, taking the OS process rather than the green one).
    //
    // Probing every slow call costs a thread-local read; the slow call already does far
    // more work than that, and it is the only place that can see this recursion coming.
    let over_cap = heap.jit_native_depth >= JIT_NATIVE_DEPTH_LIMIT || !stack_headroom_ok();
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
        if let Some((code, nslots, callee_env, callee_bases)) =
            heap.vm_call_ic_fast_link(site, head, argc as u32, epoch)
        {
            // This entry hands back a `Value`, so it owns the destination: a stack local,
            // which is where the result would have been copied to anyway.
            let mut ret = Value::Nil;
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
                callee_bases,
                &mut ret as *mut Value,
            ) {
                FastLinkOutcome::Done => return Some(ret),
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
        // Direct-call a BUILTIN callee: read the staged args (rooted at
        // `[stage_base, n)` — the same discipline the VM's `Inst::Call` uses),
        // invoke the native fn pointer, park any error. This is the native-callee
        // fast path: no `env_get`, no `dispatch` (passthrough loop + `apply`
        // unfold + arity re-checks) — the ~55–75 ns/call protocol every `str`/
        // `string/char->int`/`string-length` from JIT'd code used to pay. `apply` itself
        // has a real native body (`apply_builtin`), so direct invocation is exact.
        macro_rules! call_native_direct {
            ($nid:expr) => {{
                let mut argv: SmallVec<[Value; 4]> = SmallVec::with_capacity(argc);
                for k in 0..argc {
                    argv.push(heap.root_at(stage_base + k));
                }
                let env = heap.read_root_env(heap.jit_call_env);
                let r = crate::eval::call_native(heap, $nid, &argv, env);
                heap.truncate_roots(stage_base);
                return match r {
                    Ok(v) => Some(v),
                    Err(e) => {
                        heap.jit_pending_error = Some(e);
                        None
                    }
                };
            }};
        }
        let resolved: Option<(Arc<ArmHandle>, EnvId, (u32, u32))> = if elided {
            match heap.vm_call_ic_probe(site, head, argc as u32, epoch) {
                Some((_, Some(t))) => Some(t),
                // IC hit on a NATIVE callee (arm-less entry, filled below on first
                // resolve): the whole call is one arity-checked fn-pointer call.
                Some((v, None)) if !over_cap => {
                    if let ValueRef::Native(nid) = v.unpack() {
                        // Reaching here means the IR's flat-cell fast path missed (cold
                        // site, cleared table, or new epoch) — republish so the next call
                        // stays entirely in IR (arity pre-validated for this argc).
                        let nat = heap.native(nid);
                        if nat.arity.accepts(argc) && !value::is_dynamic(head) {
                            let func = nat.func as usize as u64;
                            heap.vm_fast_link_publish_native(site, head, argc as u32, epoch, func);
                        }
                        call_native_direct!(nid)
                    }
                    None
                }
                _ => {
                    // Miss: resolve the callee global (the only `env_get` on the call path,
                    // and only while cold) and fill the IC.
                    let cenv = heap.read_root_env(heap.jit_call_env);
                    match heap.env_get(cenv, head).map(|v| v.unpack()) {
                        Some(ValueRef::Fn(id)) => {
                            compiled_arm_for(heap, id, argc).map(|a| {
                                let env = heap.closure(id).env.unwrap_or_else(|| heap.global());
                                let cb = heap.vm_arm_block(&a);
                                if !value::is_dynamic(head) {
                                    heap.vm_call_ic_put(
                                        site,
                                        crate::core::heap::CallIcEntry {
                                            sym: head,
                                            argc: argc as u32,
                                            epoch,
                                            callee: Value::func(id),
                                            arm: Some((a.clone(), env)),
                                            // Overwritten inside `vm_call_ic_put`.
                                            callee_bases: (0, 0),
                                        },
                                    );
                                }
                                (a, env, cb)
                            })
                        }
                        // A builtin callee: fill an arm-less IC entry (so the next call
                        // takes the direct path above) and call it now. Dynamic heads are
                        // never cached (they can shadow per call) but still call direct.
                        Some(ValueRef::Native(nid)) if !over_cap => {
                            if !value::is_dynamic(head) {
                                heap.vm_call_ic_put(
                                    site,
                                    crate::core::heap::CallIcEntry {
                                        sym: head,
                                        argc: argc as u32,
                                        epoch,
                                        callee: Value::native(nid),
                                        arm: None,
                                        callee_bases: (0, 0),
                                    },
                                );
                                // Flat-cell publish: the IR's next call at this site goes
                                // straight to the fn pointer (arity pre-validated here).
                                let nat = heap.native(nid);
                                if nat.arity.accepts(argc) {
                                    let func = nat.func as usize as u64;
                                    heap.vm_fast_link_publish_native(
                                        site,
                                        head,
                                        argc as u32,
                                        epoch,
                                        func,
                                    );
                                }
                            }
                            call_native_direct!(nid)
                        }
                        _ => None,
                    }
                }
            }
        } else if let ValueRef::Fn(id) = heap.root_at(stage_base).unpack() {
            // The non-elided (computed-head) resolve: no IC, so this runs per call — the
            // handle it hands back is memoized, not freshly allocated.
            compiled_arm_for(heap, id, argc).map(|a| {
                let env = heap.closure(id).env.unwrap_or_else(|| heap.global());
                let cb = heap.vm_arm_block(&a);
                (a, env, cb)
            })
        } else {
            None
        };
        if let Some((arm, callee_env, callee_bases)) = resolved {
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
                // Two-stage tiering: size the callee frame to the native version we are about
                // to CALL (inlined upgrade → `inline_nslots`; small → `nslots`). Keyed on the
                // `code` pointer loaded above, NOT on `inline_installed`: the flag is a second,
                // independently-racing read of the same fact, and a peer process swapping the
                // upgrade in between the two would have us size to `inline_nslots` while
                // calling the small native — whose outcome-4 tail staging then lands at
                // `base + nslots` and is read back here at `base + inline_nslots`. Captured
                // once and reused for both the frame extension and the staged_start
                // calculation — the two must agree on the same frame boundary.
                let frame_nslots = frame_size_for_code(&arm, code);
                heap.extend_roots_to_nil(stage_base + frame_nslots);
                let base = stage_base;
                // SAFETY: `code` is a finalized `extern "C" fn(*mut Heap, base)` from
                // `jit_lower_arm`, living for the process in `GLOBAL_JIT`; the frame is set
                // up at `roots[base..]`.
                let f: crate::jit::JitArmFn = unsafe { std::mem::transmute(code) };
                // Destination for a Done result — this entry hands back a `Value`, so it is
                // a stack local (see `crate::jit::JitArmFn`).
                let mut ret = Value::Nil;
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
                stamp_stack_limit_if_outermost(heap, depth);
                let saved_force_vm = heap.jit_force_vm;
                // The callee's native code reads its OWN IC block through the heap
                // cursors (fast-link base, IC callbacks) — install it for the call and
                // restore the caller's around it, like `jit_call_env` above.
                let saved_bases = heap.set_ic_bases(callee_bases);
                heap.native_gateway_seq += 1;
                let gw_seq = heap.native_gateway_seq;
                let saved_gw = std::mem::replace(&mut heap.cur_native_gateway, gw_seq);
                let outcome = f(heap as *mut Heap, base as i64, &mut ret as *mut Value);
                heap.cur_native_gateway = saved_gw;
                jit_suspend_feedback(heap, &arm, outcome, gw_seq);
                heap.set_ic_bases(saved_bases);
                heap.jit_force_vm = saved_force_vm;
                heap.jit_native_depth = depth;
                heap.jit_call_env = saved;
                heap.jit_dbg_fn = saved_fn;
                // Deopt feedback (see `jit_deopt_feedback`) for the native→native link.
                if arm.deopt_watch {
                    use std::sync::atomic::Ordering::Relaxed;
                    if outcome == 1 {
                        jit_deopt_feedback(&arm);
                    } else if arm.jit_deopts.load(Relaxed) != 0 {
                        arm.jit_deopts.store(0, Relaxed);
                    }
                }
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
                    // Done: the arm wrote the result through `ret`. Drop the frame.
                    0 => {
                        crate::perf_bump!(jit_link_done);
                        heap.truncate_roots(stage_base);
                        return Some(ret);
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
                        // Deopt-resume (see `CompiledArm::ckpt_slot`): resume AT the
                        // checkpoint, frame intact — never re-running side effects.
                        // `1 | 2` (deopt AND preempt), consistent with the other three
                        // consumers — see the note at the fast-link resume above (KI-18).
                        if matches!(outcome, 1 | 2) {
                            if let Some((resume, rip, depth)) =
                                jit_ckpt_resume(heap, arm.arc(), base, frame_nslots)
                            {
                                return match vm_resume_deopt(
                                    heap, resume, base, callee_env, rip, depth,
                                ) {
                                    Ok(v) => Some(v),
                                    Err(e) => {
                                        heap.jit_pending_error = Some(e);
                                        None
                                    }
                                };
                            }
                        }
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
            bases: _,
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
pub(crate) fn jit_dispatch_tail(
    heap: &mut Heap,
    base: usize,
    arm: &CompiledArm,
    env: EnvRoot,
    // The size this frame was actually BUILT to, captured by the trampoline at native entry.
    // **Must be passed, never re-derived here** (KI-48). It used to be
    // `base + arm.frame_size_for_new_entry()`, which re-reads `inline_installed` — the KI-26 / ADR-210
    // anti-pattern. The background inline upgrade can flip that flag between the native
    // entering (small frame) and this callback running, after which the staged
    // `[callee, args…]` is written at one offset and read at another: measured live on 123
    // arms, `fold` among them at nslots=13 vs inline_nslots=25, i.e. a 12-slot overshoot
    // straight past the staged area and off the roots stack. The caller already captures the
    // size once for exactly this reason ("the two must agree on the same frame boundary"),
    // and the deopt-resume helpers are already told it rather than re-deriving; this path was
    // the one that was not.
    frame_nslots: usize,
) -> Result<ChunkExit, LispError> {
    // A tail call is staged by the native code ABOVE its own frame top.
    let top = base + frame_nslots;
    let n = heap.roots_len();
    // KI-48 tripwire. `top` is derived from `frame_size_for_new_entry()`, which re-reads
    // `inline_installed` — the KI-26 / ADR-210 anti-pattern — while the frame this native
    // actually built was sized by whichever body was installed when it was ENTERED. If the
    // background inline upgrade lands in that window, the two disagree and `top` points past
    // the staged `[callee, args…]` region, which is how KI-48 was captured: `root_at(9)` on
    // a len-8 stack.
    //
    // Two reasons this is checked rather than left to `root_at`'s own bounds check. The
    // subtraction below underflows first on exactly this input (`top >= n` ⇒ wrapped `argc`),
    // so the OOB panic is the *lucky* ordering — the other one loops `root_at(top + 1 + k)`
    // over a huge count. And `root_at`'s panic names neither the arm nor the frame sizes, so
    // the original report could say only "index 9, len 8" and nothing about why.
    if top >= n {
        let name = arm
            .dbg_name
            .map(crate::core::value::symbol_name_ref)
            .unwrap_or("<closure>");
        panic!(
            "KI-48: jit_dispatch_tail staged-area desync — arm={name} base={base} \
             frame_nslots={frame_nslots} (nslots={} inline_nslots={} inline_installed={}) \
             top={top} roots_len={n}; the frame was built for a different body than the \
             caller reported",
            arm.nslots,
            arm.inline_nslots,
            arm.inline_installed
                .load(std::sync::atomic::Ordering::Acquire),
        );
    }
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
            bases,
        } => ChunkExit::Tail {
            arm: compiled,
            args,
            genv,
            bases,
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
/// Deopt feedback for a watched arm (`deopt_watch`, see the `CompiledArm` field
/// doc): count the **consecutive** type-deopt (each success resets the counter
/// at the call sites) and, once the arm has demonstrably thrashed — 16 deopts
/// in a row — mark it `BAILED` so it stays on the VM. A native attempt that
/// keeps deopting pays entry + deopt + a full VM re-run per call (nbody's
/// `advance-body`: ~100% deopt rate across 248k activations). An arm with only
/// occasional deopts never reaches 16 consecutive and keeps its native code.
/// `BAILED` is sticky until the next epoch invalidation, which resets the
/// counter so the recompiled arm gets a fresh trial.
/// Could the frame of size `frame_nslots` at this call site belong to `arm`? A deopt may only
/// be resumed when it can: the inline cache might have re-resolved the site to a *different*
/// arm than the one whose native actually ran, and reading a foreign arm's `ckpt_slot` out of
/// this frame yields a garbage resume ip (or an out-of-bounds root read).
///
/// **Deliberately flag-free** (KI-26). The obvious spelling is `arm.frame_size_for_new_entry() ==
/// frame_nslots`, but `frame_size_for_new_entry()` re-reads `inline_installed` — the anti-pattern behind
/// two ADR-210 bugs — and the inline swap in [`jit_tier`] deliberately does *not* bump the
/// global epoch (a bump cascaded under `pfib`; see the comment at the swap) and invalidates
/// only the installing process's fast links. A `share_key` arm is shared across processes, so
/// a peer can hold a link whose recorded `frame_nslots` predates the swap while the flag now
/// reads true. The flag form then declines, and the caller's fallthrough re-runs the arm from
/// ip 0 — repeating whatever effect the native had already journaled.
///
/// Testing both of the arm's possible frame sizes is a strict superset of the flag form
/// (`frame_size_for_new_entry()` returns exactly one of them), so this only ever *admits* more resumes —
/// and resuming is the effect-preserving direction. Every admitted resume is still validated
/// by [`jit_ckpt_resume`], which requires a positive journal and reads only in-bounds slots.
/// A genuinely foreign arm still fails, which is the out-of-bounds protection this exists for.
#[cfg(feature = "jit")]
pub(crate) fn jit_frame_shape_matches(arm: &CompiledArm, frame_nslots: usize) -> bool {
    frame_nslots == arm.nslots || frame_nslots == arm.inline_nslots
}

/// Deopt-resume checkpoint (see `CompiledArm::ckpt_slot`): decode the live frame's
/// journal — `Some((resume_arm, resume_ip, operand_depth))` when a completed non-tail
/// call (or `table-put`) checkpointed this activation, meaning the VM must resume THERE
/// (the side effects before it already happened, exactly once). `None` ⇒ resume from ip 0,
/// which is then effect-free by construction (everything the boxed subset executes
/// besides calls and `table-put` is pure or idempotent).
///
/// **`resume_arm` is the arm whose chunk the journal's ip indexes**, which is not always
/// the arm that was called. A journal is written by whichever engine ran the frame, and
/// each engine has its own bytecode:
///
/// - small native → `arm` itself.
/// - **leaf-spliced** native → the derivation's [`resume`](ir::LeafInline::resume) arm,
///   which carries the spliced chunk and the matching frame layout. Resuming in `arm`
///   here would interpret a *different* chunk from the journalled ip — which is exactly
///   why the inlined engine could not journal at all before, and so could not keep a
///   residual non-tail call.
/// - **self-spliced** native → never journals (`u32::MAX` at lowering), so its frame's
///   slot still reads the entry reset's 0 and this returns `None`.
///
/// `frame_nslots` is **the size the caller built this frame to**, and every caller must pass
/// its own — that is what selects the layout (see [`jit_frame_layout`] for why the
/// `inline_installed` flag cannot be used instead) and what makes the slot read in bounds.
/// Call this at most once per deopt: the decision must be taken before anything resizes the
/// frame, because a second read would come from the resized one.
#[cfg(feature = "jit")]
pub(crate) fn jit_ckpt_resume(
    heap: &Heap,
    arm: &Arc<CompiledArm>,
    base: usize,
    frame_nslots: usize,
) -> Option<(Arc<CompiledArm>, usize, usize)> {
    let layout = jit_frame_layout(arm, frame_nslots);
    // The journal slot of the layout this frame was BUILT to — never the other one's.
    // A layout that writes no journal (`u32::MAX`) yields `None` here, which means
    // "resume from ip 0", and ip-0 re-run is effect-free by construction for exactly
    // the arms that decline to journal (`jit_ckpt_depth`'s `pure_self` exemption).
    //
    // Reading the *small* layout's `ckpt_slot` out of a leaf-spliced frame was a live
    // miscompile: leaf splicing removes the residual `Call`, which makes the derivation
    // `pure_self` and therefore unjournalled (`resume.ckpt_slot == u32::MAX`) even
    // though the small body — which still has the call — journals at a real slot. The
    // old spelling asked "is this frame leaf-spliced *and journalled*?", answered "no"
    // for that pair, and fell back to the small slot, whose meaning in the spliced
    // layout is undefined. In `(defn sum-down (n acc) (if (<= n 0) acc (sum-down (dec n)
    // (+ acc n))))` it held the live loop counter, so a preempt decoded `n` as a journal
    // word: resume ip `n >> 16`, operand depth `n & 0xFFFF`. `(sum-down 200000 0)`
    // returned 6251217600 instead of 20000100000 on the default build, and at 400000 it
    // surfaced as `type error: -: expected number, got nil` blaming `dec`.
    let slot = match layout {
        FrameLayout::LeafSpliced => arm.leaf.as_ref()?.resume.ckpt_slot,
        FrameLayout::Small => arm.ckpt_slot,
    };
    if slot == u32::MAX {
        return None;
    }
    let p = match heap.root_at(base + slot as usize) {
        Value::Int(p) if p > 0 => p,
        _ => return None,
    };
    // Continue in whichever layout wrote that journal.
    let resume = match layout {
        FrameLayout::LeafSpliced => arm.leaf.as_ref()?.resume.clone(),
        FrameLayout::Small => arm.clone(),
    };
    Some((resume, (p >> 16) as usize, (p & 0xFFFF) as usize))
}

/// Which of the arm's two frame layouts was the frame at `base` built to?
///
/// Selected by the size the caller built it to — **never** by reading `inline_installed`.
/// That flag is flipped by [`jit_tier`] itself, i.e. exactly between the frame being sized
/// and the deopt being handled, so on the activation where the inlined upgrade installs,
/// reading it afterwards claims the inlined layout for a frame built to the small one:
/// `base + <inlined ckpt slot>` then indexes past the root stack, which surfaced as an
/// out-of-bounds `root_at` inside a later GC walk (KI-26 / ADR-210).
#[cfg(feature = "jit")]
#[derive(Clone, Copy, PartialEq, Eq)]
enum FrameLayout {
    /// The original small body.
    Small,
    /// A leaf-callee-spliced derivation (ADR-210), whose slot range differs from the
    /// small one's.
    LeafSpliced,
}

#[cfg(feature = "jit")]
fn jit_frame_layout(arm: &CompiledArm, frame_nslots: usize) -> FrameLayout {
    // A leaf derivation's frame is strictly larger than the small one (it splices extra
    // slots and then reserves blocks + journal), so the size tells them apart.
    if arm.leaf.is_some() && arm.inline_nslots > arm.nslots && frame_nslots == arm.inline_nslots {
        FrameLayout::LeafSpliced
    } else {
        FrameLayout::Small
    }
}

/// Resume a deopted JIT frame at its checkpoint on the VM: push the journaled
/// operand stack (slots `[ckpt_slot+1 ..]`, GC-current) and drive the frame from
/// `rip` via `vm_run_bc`'s resume machinery (a synthetic single-frame
/// continuation — the frame at `roots[base..]` is intact per the deopt
/// contract, exactly the shape of a frame suspended at a `Call`).
#[cfg(feature = "jit")]
pub(crate) fn vm_resume_deopt(
    heap: &mut Heap,
    arm: Arc<CompiledArm>,
    base: usize,
    cenv: EnvId,
    rip: usize,
    depth: usize,
) -> LispResult {
    // Deopt is a cold path, so wrapping the shared arm in its process-local handle
    // (KI-40) here costs one allocation per deopt, not per call.
    let arm = ArmHandle::new(arm);
    let cb = base + arm.ckpt_slot as usize + 1;
    for k in 0..depth {
        let v = heap.root_at(cb + k);
        heap.push_root(v);
    }
    let env_base = heap.env_roots_len();
    let env_root = heap.root_env(cenv);
    let entry_arms = heap.live_arm_len();
    let arm_slot = if arm.has_runtime_handles {
        heap.live_arm_push(arm.clone())
    } else {
        usize::MAX
    };
    let s = Suspended {
        frames: Vec::new(),
        cur: BcFrame {
            arm: arm.clone(),
            ip: rip,
            base,
            env: env_root,
            env_base,
            arm_slot,
            ic_bases: heap.vm_arm_block(&arm),
            back_edges: 0,
        },
        entry_roots: base,
        entry_env: env_base,
        entry_arms,
        deadline: None,
    };
    let genv = heap.global();
    // Nested run: the caller's native frame continues with ITS block after this
    // returns, so restore the cursors like `vm_apply` does.
    let saved_bases = heap.ic_bases();
    let out = vm_run_bc(heap, arm, &[], genv, Some(s), false);
    heap.set_ic_bases(saved_bases);
    match out? {
        VmOutcome::Done(v) => Ok(v),
        // Native-nested receive-suspend: same discipline as `vm_apply`.
        VmOutcome::Suspended(s) => {
            let deadline = s.deadline;
            heap.truncate_roots(s.entry_roots);
            heap.truncate_env_roots(s.entry_env);
            heap.live_arm_truncate(s.entry_arms);
            Err(LispError::suspend(deadline))
        }
        VmOutcome::Preempted(_) | VmOutcome::Killed => {
            unreachable!("a nested deopt-resume run does no loop-top preempt/kill capture")
        }
    }
}

/// Latch `arm` off the native tier because it hosted a parked `receive`. A process
/// suspended at `receive` under a native frame cannot be state-captured — the mailbox's
/// capture path requires a clean all-VM stack ("only a clean top-level receive captures,
/// and can migrate") — so it dirty-blocks its whole OS worker thread instead (§7.4). An
/// arm that hosts a parking receive therefore belongs on the VM: latch it `BAILED` on the
/// FIRST occurrence (one park is proof of shape, unlike a type-deopt, which needs 16 to
/// separate thrash from noise). Arms are shared (ADR-215), so one latch heals every
/// process; a park under a native→native chain latches the innermost enclosing arm per
/// occurrence and converges outward over successive parks.
///
/// Found during the §7.1 step 2 experiment (admitting named defns to the general
/// lowering), where `live_migration`'s 12-way load harness went 28/36 liveness failures
/// without it, 0/36 with. Step 2 was measured and rejected, and on today's tree the
/// latch is mostly LATENT: the `%receive` fence only catches a direct `%receive` call,
/// but the shapes that would host one indirectly are fenced by other means — a `def`-
/// named closure gate-bails like any named defn, and a single-non-tail-call anonymous
/// closure gets no spill slots (`jit_spill_reserve`'s measured-load-bearing rule) and
/// bails mid-emit. The latch stays as the scheduler's safety net for any future
/// admission (partial lowering, a wider subset): whatever lowers a receive-hosting arm
/// next will find the liveness failure already guarded rather than rediscovering it.
#[cfg(feature = "jit")]
pub(crate) fn jit_latch_suspend_host(arm: &CompiledArm) {
    use std::sync::atomic::Ordering::Release;
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("BROOD_JIT_BAIL_TRACE").is_some()) {
        let name = arm
            .dbg_name
            .map(crate::core::value::symbol_name_ref)
            .unwrap_or("<closure>");
        eprintln!("[jit-bail] arm={name} reason=suspend-latched");
    }
    arm.jit_code.store(crate::jit::BAILED, Release);
}

/// Post-invoke suspend check for a native gateway that has its arm in hand: latch if the
/// mailbox recorded a dirty-block under THIS activation's token (see
/// [`Heap::blocked_under_gateway`] — an exact match, so a native that merely ran later in
/// the same quantum is never blamed), or — belt-and-braces — if a suspend signal crossed
/// this arm on the error channel as outcome 3 (no known flow does this today; the mailbox
/// blocks instead of raising when a native frame is above it).
#[cfg(feature = "jit")]
fn jit_suspend_feedback(heap: &mut Heap, arm: &CompiledArm, outcome: i64, gw_seq: u64) {
    let blocked = heap.blocked_under_gateway == gw_seq && gw_seq != 0;
    if blocked {
        heap.blocked_under_gateway = 0;
    }
    if blocked
        || (outcome == 3
            && heap
                .jit_pending_error
                .as_ref()
                .is_some_and(|e| e.is_suspend_signal()))
    {
        jit_latch_suspend_host(arm);
    }
}

#[cfg(feature = "jit")]
pub(crate) fn jit_deopt_feedback(arm: &CompiledArm) {
    use std::sync::atomic::Ordering::{Relaxed, Release};
    const DEOPT_BAIL_CONSECUTIVE: u32 = 16;
    let d = arm.jit_deopts.fetch_add(1, Relaxed) + 1;
    if d >= DEOPT_BAIL_CONSECUTIVE {
        // Deopt thrash: the arm went native, fell out 16 times in a row, and is now latched
        // onto the VM. Traced because the *consequence* (permanently interpreted code) is far
        // more visible than the cause, and `BROOD_DEOPT_TRACE` needs `perf-stats` while this
        // does not.
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
                "[jit-bail] arm={name} reason=deopt-thrash-latched nslots={} deopts={d} \
                 inline_installed={} ops=[{}]",
                arm.nslots,
                arm.inline_installed
                    .load(std::sync::atomic::Ordering::Acquire),
                ops.join(" ")
            );
        }
        arm.jit_code.store(crate::jit::BAILED, Release);
    }
}

/// The frame size the code pointer `code` runs against — the **non-racy** counterpart of
/// [`CompiledArm::frame_size_for_new_entry`]. It keys on the pointer the caller is about to
/// call rather than re-reading `inline_installed`, so the size and the code cannot disagree.
///
/// Prefer this wherever the code pointer is already in hand: `frame_size_for_new_entry()`
/// answers "what does the *currently installed* version want", which is a different question
/// from "what does *this* pointer want" the moment a peer process swaps the inlined upgrade
/// in (`CompiledArm` is shared across a runtime's processes since ADR-215).
#[cfg(feature = "jit")]
pub(crate) fn frame_size_for_code(arm: &CompiledArm, code: *mut u8) -> usize {
    let ic = arm.inline_code.load(std::sync::atomic::Ordering::Acquire);
    if !ic.is_null() && ic == code {
        arm.inline_nslots
    } else {
        arm.nslots
    }
}

/// Tiering entry (ADR-101 1b). `frame_nslots` is **the size the caller already built this
/// frame to**, and the native entry is declined (`None` — run the VM this activation) when
/// the installed code turns out to want a *bigger* frame than that. There is deliberately no
/// size-free spelling: a caller that cannot say what it built cannot be entered safely.
///
/// The KI-48 family, fourth appearance. The rule KI-48 wrote down — "the caller captures
/// the size once and TELLS every consumer" — does not cover this one, because here the
/// caller tells correctly and it is the **code pointer** that gets re-derived underneath:
/// `vm_run_bc`/`dispatch` read `inline_installed` first (to size the frame), then call
/// `jit_tier`, which Acquire-loads `jit_code` *again*. The two-stage swap stores
/// `inline_installed` before `jit_code` precisely so a reader that sees the inlined pointer
/// also sees the flag — but only if it reads code *first*. Read flag-then-code and the
/// Release/Acquire chain guarantees nothing: a peer process swapping in the inlined body in
/// that window leaves the caller holding a small frame while `jit_tier` runs the inlined
/// native, which raw-writes slots past the frame top (measured overshoot: 12 slots on
/// `fold`, `nslots` 13 vs `inline_nslots` 25).
///
/// (The other native entries — `jit_dispatch_call`, `hof_apply_native`, `jit_run_fast_link` —
/// do not come through here at all: each loads the code pointer itself and sizes from THAT,
/// via [`frame_size_for_code`], which is the same rule spelled the other way round.)
#[cfg(feature = "jit")]
pub(crate) fn jit_tier_in_frame(
    arm: &Arc<CompiledArm>,
    heap: &mut Heap,
    base: usize,
    env: EnvRoot,
    frame_nslots: usize,
    out: *mut Value,
) -> Option<i64> {
    use std::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed, Release};
    const THRESHOLD: u32 = 8;

    // Draining an over-deep native-recursion subtree on the VM (see [`JIT_FORCE_VM`]):
    // interpret this arm so its recursion stays in the bounded heap-frame loop.
    if heap.jit_force_vm {
        return None;
    }
    // Tier ceiling below Native (ADR-222; `BROOD_TIER=1`, or its `BROOD_NO_JIT` alias): never
    // compile or run native — interpret on the (correct) tier 1. Returns before the hotness
    // count + the background-compile enqueue CAS, so no arm is ever handed to the compiler and
    // no native pointer is installed, so the fast-link / dispatch paths have nothing to call
    // either. This used to be its own `BROOD_NO_JIT` read here, unrelated to the engine
    // selector — one ceiling now answers both.
    if tier_ceiling() < Tier::Native {
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
    if code == crate::jit::BAILED {
        // §7.1 hot admission: a gate-refused arm's deferred hot compile may have landed
        // (staged in `inline_code` by the bg thread) — install it, the same plain swap
        // the relower uses (same chunk/frame/checkpoint as the small body would have
        // had). The next activation runs it through the normal installed path, whose
        // epoch guard covers a `def` since the compile.
        if xadmit_enabled() {
            let ic = arm.inline_code.load(Acquire);
            if !ic.is_null() && ic != crate::jit::BAILED && ic != crate::jit::QUEUED {
                arm.inline_code.store(std::ptr::null_mut(), Release);
                arm.jit_code.store(ic, Release);
                if let Some(sym) = arm.dbg_name {
                    heap.invalidate_fast_links_for(sym);
                }
                {
                    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
                    if *ON.get_or_init(|| std::env::var_os("BROOD_JIT_BAIL_TRACE").is_some()) {
                        let name = arm
                            .dbg_name
                            .map(crate::core::value::symbol_name_ref)
                            .unwrap_or("<closure>");
                        eprintln!("[jit-ir] arm={name} xadmit-installed nslots={}", arm.nslots);
                    }
                }
            }
        }
        return None; // out of subset (or awaiting the hot install) — run the VM
    }
    // Shared-JIT install (the spawn lever): before this process spends THRESHOLD
    // interpreted calls + a background compile on its OWN copy of a RUNTIME/PRELUDE
    // arm, check whether another process of this runtime already compiled it. If so,
    // and the code is epoch-current, install the shared pointer directly and run it
    // now — so a hot shared function (`fib` under `spawn`) compiles to native ONCE,
    // not once per process. Stale entries (a `def`/compaction bumped the epoch) skip.
    // A fn a depth-bail switched to boxed must not re-install a stale shared i64 wrapper from the
    // cache — skip the shared install so it recompiles boxed locally (and re-publishes boxed).
    //
    // A QUEUED copy checks too: its own compile is in flight, but a peer's identical
    // arm may have compiled AND published since we enqueued (a spawn storm queues
    // dozens of copies of the same shared closure). Installing over QUEUED is benign
    // either way — the background dequeue skips any resolved slot, and if it races a
    // concurrent store of this copy's own pointer, both pointers are valid code for
    // the same epoch (each kept alive by its compiler's keepalive push).
    if (code.is_null() || code == crate::jit::QUEUED) && ActiveBackend::may_adopt_shared_code(arm) {
        if let Some(key) = arm.share_key {
            if let Some((ptr, epoch)) = heap.jit_shared_lookup(key) {
                if epoch == heap.global_epoch()
                    && !ptr.is_null()
                    && ptr != crate::jit::BAILED
                    && ptr != crate::jit::QUEUED
                {
                    arm.compile_epoch.store(epoch, Release);
                    // Trace the ADOPT path: this arm installs a peer's compiled pointer
                    // WITHOUT lowering, so it never reaches the `BROOD_JIT_DUMP_IR` dump.
                    // That is why an arm can hold native code, deopt out of it, and still be
                    // absent from every IR dump — which is exactly the state the tagged-tuple
                    // receive matcher was found in.
                    {
                        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
                        if *ON.get_or_init(|| std::env::var_os("BROOD_JIT_BAIL_TRACE").is_some()) {
                            let name = arm
                                .dbg_name
                                .map(crate::core::value::symbol_name_ref)
                                .unwrap_or("<closure>");
                            eprintln!(
                                "[jit-ir] arm={name} adopted-shared-code nslots={} (not lowered \
                                 here, so it emits no IR dump)",
                                arm.nslots
                            );
                        }
                    }
                    arm.jit_code.store(ptr, Release);
                    arm.shared_published.store(true, Relaxed); // already in the cache
                    code = ptr;
                }
            }
        }
    }
    if code == crate::jit::QUEUED {
        return None; // compile in flight, nothing published yet — run the VM
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
            // The last untraced BAILED route, and a sticky one: an arm is refused here
            // because some operator its chunk calls is not a native primitive, and it then
            // stays on the VM. Silent until now, which is why a `receive` matcher for a
            // tagged tuple could sit permanently on the interpreter with no diagnostic
            // pointing at the reason.
            static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
            if *ON.get_or_init(|| std::env::var_os("BROOD_JIT_BAIL_TRACE").is_some()) {
                let name = arm
                    .dbg_name
                    .map(crate::core::value::symbol_name_ref)
                    .unwrap_or("<closure>");
                eprintln!("[jit-bail] arm={name} reason=chunk-ops-not-all-native");
            }
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
            // The frame profile types only *params*; record the arm's float-valued free
            // globals too, so a float-context arm whose floats arrive from a `def`'d
            // constant isn't lowered onto the integer path (see `record_float_globals`).
            let genv = heap.read_root_env(env);
            record_float_globals(arm, heap, genv);
            record_self_global_ok(arm, heap, genv);
            if JIT_COMPILER
                .primary
                .try_send((arm.clone(), slot_tags, heap.runtime_tag()))
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
        arm.jit_deopts.store(0, Relaxed); // fresh deopt-feedback trial for the recompile
        arm.shared_published.store(false, Relaxed); // recompiled code must re-publish
        arm.inline_installed.store(false, Relaxed); // re-decide the inline swap at the new epoch
        arm.inline_queued.store(false, Relaxed); // re-enqueue the inlined upgrade if still hot
                                                 // Drop the stale inlined native too: its inlined operators were validated at the
                                                 // OLD epoch, so it must not be re-swapped as-is. Nulling forces a clean re-fetch
                                                 // from the shared inline cache (epoch-checked) or a recompile at the new epoch —
                                                 // load-bearing now that the inlined native is shared across processes (a stale
                                                 // pointer left here would otherwise get re-published to the shared cache).
        arm.inline_code.store(std::ptr::null_mut(), Release);
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
    //      entry sizes the frame to `frame_size_for_new_entry()` (= `inline_nslots`) and runs the inlined
    //      native. One VM activation on the transition — negligible.
    // i64-eligible arms skip the two-stage inline upgrade entirely: their small native IS the
    // unboxed-i64 register worker (`jit_lower_i64_arm`), which already recurses to full depth in
    // registers — the boxed depth-2 inlined upgrade would only swap in inferior code.
    // §7.5 hot RE-LOWERING: an arm with NO inline derivation whose chunk has a non-tail
    // named call re-lowers its OWN body (same chunk, frame and checkpoint) with the
    // inline fast-frame emission, on the same deferred channel. The chunk scan runs once
    // per arm (`xcall_wanted` is a OnceLock); `dbg_name` is required because the swap
    // re-points this process's fast links by callee name.
    let xcall_relower = arm.inline_name.is_none()
        && arm.leaf.is_none()
        && arm.dbg_name.is_some()
        // Profitability, measured 2026-08-30: the inline blob adds ~90 CLIF lines and
        // several blocks PER CALL SITE, and every value live across a call is pressured
        // through that extra CFG — so an arm carrying lots of live state pays in its own
        // loop code what the calls save. `nslots` is that live state's size. bintree's
        // winners (`check-node` 3, `make` 4: −15% wall) sit far below nbody's loser
        // (`advance-body`, 20 slots, 8 call sites, float-unboxed: relowered body +32%
        // CLIF / +36% blocks, row +8%). The cut is between 5 and 20; 8 keeps every
        // measured winner (`run` at 5 included) and excludes the measured loser with
        // margin on both sides.
        && arm.nslots <= XCALL_RELOWER_MAX_NSLOTS
        && super::xcall_relower_enabled()
        && *arm.xcall_wanted.get_or_init(|| {
            arm.chunk.as_ref().is_some_and(|c| {
                c.code.iter().any(|inst| {
                    matches!(
                        inst,
                        Inst::Call {
                            tail: false,
                            head: Some(_),
                            ..
                        }
                    )
                })
            })
        });
    if (arm.inline_name.is_some() || xcall_relower)
        && !arm.inline_installed.load(Acquire)
        && !ActiveBackend::declines_inline_upgrade(arm)
    {
        let ic = arm.inline_code.load(Acquire);
        if ic.is_null() {
            // Shared inlined-native cache (the short-burst lever): before spending our own
            // deferred compile, check whether another process of this runtime already
            // compiled the inlined body for this `(id, argc)`. If so and it's epoch-current,
            // install its pointer into our `inline_code`; the next entry's `ic != null`
            // branch below swaps it in with OUR (deterministic, identical) `inline_nslots`.
            // This is what lets a short parallel fan-out (`pfib`) pick up the inlined win —
            // one compile serves every process instead of each racing its own to completion.
            if let Some(key) = arm.share_key.filter(|_| arm.inline_name.is_some()) {
                if let Some((ptr, epoch)) = heap.jit_inline_lookup(key) {
                    if epoch == heap.global_epoch()
                        && !ptr.is_null()
                        && ptr != crate::jit::BAILED
                        && ptr != crate::jit::QUEUED
                    {
                        {
                            static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
                            if *ON
                                .get_or_init(|| std::env::var_os("BROOD_JIT_BAIL_TRACE").is_some())
                            {
                                let name = arm
                                    .dbg_name
                                    .map(crate::core::value::symbol_name_ref)
                                    .unwrap_or("<closure>");
                                eprintln!("[jit-ir] arm={name} adopted-inline-code-from-cache nslots={} (not lowered here, emits no IR dump)", arm.nslots);
                            }
                        }
                        arm.inline_code.store(ptr, Release);
                        return None; // next entry swaps it in
                    }
                }
            }
            // Not shared yet. Elect a single enqueuer via the queued flag.
            if !arm.inline_queued.swap(true, AcqRel) {
                let slot_tags: Vec<u8> = (0..arm.nslots)
                    .map(|i| crate::core::value::tag(heap.root_at(base + i)) as u8)
                    .collect();
                // Deferred (low-priority). On a full queue, un-set `inline_queued` so a
                // later call re-attempts — but DON'T disturb the running small native.
                if JIT_COMPILER
                    .deferred
                    .try_send((arm.clone(), slot_tags, heap.runtime_tag()))
                    .is_err()
                {
                    arm.inline_queued.store(false, Relaxed);
                }
            }
        } else if ic != crate::jit::BAILED && ic != crate::jit::QUEUED {
            if xcall_relower {
                // The re-lowered body is ready — a PLAIN pointer swap: same chunk, same
                // frame size, same checkpoint, so none of the inlined swap's machinery
                // applies. `inline_installed` stays false (frame sizing stays `nslots`,
                // which both codes want), and the staging slot is nulled FIRST so
                // `frame_size_for_code` can never match the new pointer against
                // `inline_nslots` mid-swap (which is floored to `nslots` anyway — belt
                // and braces). A peer's stale FastLink (old pointer + `nslots`) stays a
                // self-consistent snapshot — the old code remains correct, just thinner —
                // so only this process's links are re-pointed. `inline_queued` stays
                // true, which is the once-per-epoch latch against re-enqueueing.
                arm.inline_code.store(std::ptr::null_mut(), Release);
                arm.jit_code.store(ic, Release);
                if let Some(sym) = arm.dbg_name {
                    heap.invalidate_fast_links_for(sym);
                }
                {
                    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
                    if *ON.get_or_init(|| std::env::var_os("BROOD_JIT_BAIL_TRACE").is_some()) {
                        let name = arm
                            .dbg_name
                            .map(crate::core::value::symbol_name_ref)
                            .unwrap_or("<closure>");
                        eprintln!(
                            "[jit-ir] arm={name} xcall-relower-installed nslots={}",
                            arm.nslots
                        );
                    }
                }
                return None; // one VM activation across the swap, like the inlined path
            }
            // The inlined upgrade is ready — swap it in. Store `inline_installed` BEFORE
            // `jit_code` so that any reader which Acquire-loads `jit_code = inline_code` is
            // guaranteed (by the Release-Acquire chain) to also see `inline_installed = true`
            // and therefore call `frame_size_for_new_entry()` → `inline_nslots`. The reversed order
            // (jit_code before inline_installed) created a race: a reader could observe the
            // inline code pointer but still see `inline_installed = false`, sizing the callee
            // frame to the small `nslots` — the inline code would then raw-read beyond the
            // frame, picking up stale Vec-capacity data as slot values and passing garbage
            // through the outcome-4 tail-call staging path.
            //
            // The upgrade must only re-point THIS process's fast-links to this callee — NOT
            // bump the shared `global_epoch`. A global bump invalidated
            // every peer process's `compile_epoch` too, so under `pfib` all 100 processes
            // cascaded: each peer nuked its installed code, re-tiered, re-upgraded and
            // re-bumped in turn, permanently diverting calls off the in-IR fast-link onto
            // the slow IC-dispatch path (~2× instructions; the parallel-scaling gap). We keep
            // `compile_epoch` at the current epoch (the arm's inlined operators were just
            // re-validated at compile time) and invalidate only this process's fast-links to
            // this callee, which then re-probe and pick up `inline_code` + `inline_nslots`.
            //
            // **The real invariant, stated plainly (this comment used to claim the opposite).**
            // Since ADR-215 this `CompiledArm` is NOT per-process: `compiled_arm_for` hands
            // every process of the runtime the same `Arc<CompiledArm>` out of `shared_closures`,
            // so `inline_installed` / `jit_code` / `inline_code` are shared mutable state read
            // concurrently by worker threads. `jit_frame_shape_matches`'s doc has this right.
            // What makes the narrow (this-process-only) `invalidate_fast_links_for` sound is
            // therefore NOT "peers have their own arm with the flag false" — they don't — but
            // the fact that a peer's stale `FastLink` is meant to be a **self-consistent
            // snapshot**: it records a code pointer together with the frame size that pointer
            // wants, and it enters through that recorded pointer. A peer that never invalidates
            // keeps running the small native with the small frame, which stays correct code for
            // this epoch (both bodies are valid; the upgrade is a speed change, not a semantic
            // one) until its own `jit_tier` entry re-reads and upgrades. Any consumer that mixes
            // ONE of those two fields with a freshly re-read other one is the bug — which is
            // exactly the KI-48 family, and why the frame-building callers now go through
            // `jit_tier_in_frame` / `frame_size_for_code`.
            //
            // ⚠ One place still writes a snapshot whose halves are read independently:
            // `vm_call_ic_fast_link` (`core/heap/vm_cache.rs`) Acquire-loads `code`, then takes
            // `arm.frame_size_for_new_entry()` — a second, separately-racing read of
            // `inline_installed`. In the window where this swap lands between those two reads it
            // records `(small code, inline_nslots)`, and the small native's outcome-4 staging
            // then lands at `base + nslots` while the link reads it back at
            // `base + inline_nslots`. The fix is the same one applied here — size from the
            // pointer you loaded, `frame_size_for_code(arm, code)` — but that file is outside
            // this change's ownership, so it is recorded rather than done.
            {
                static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
                if *ON.get_or_init(|| std::env::var_os("BROOD_JIT_BAIL_TRACE").is_some()) {
                    let name = arm
                        .dbg_name
                        .map(crate::core::value::symbol_name_ref)
                        .unwrap_or("<closure>");
                    eprintln!(
                        "[jit-ir] arm={name} inline-swap-installed nslots={} inline_nslots={}",
                        arm.nslots, arm.inline_nslots
                    );
                }
            }
            arm.inline_installed.store(true, Release); // BEFORE jit_code — see comment above
            arm.jit_code.store(ic, Release);
            if let Some(sym) = arm.inline_name {
                heap.invalidate_fast_links_for(sym);
            }
            // Publish the inlined native to the shared cache so peer processes install it
            // directly instead of each compiling their own (the short-burst lever). This
            // block runs at most once per arm per epoch (latched by `inline_installed`
            // above), so no extra guard is needed; a republish of the same `(ptr, epoch)`
            // by a peer that installed FROM the cache is idempotent. `inline_nslots` is
            // deterministic for this bytecode, so `ic` is interchangeable across processes.
            if let Some(key) = arm.share_key {
                heap.jit_inline_publish(key, ic, arm.compile_epoch.load(Acquire));
            }
            // Run the VM this activation; the next entry sizes the frame to inline_nslots
            // (the call site reads `frame_size_for_new_entry()`) and runs the inlined native.
            return None;
        }
        // `ic == BAILED`: the inlined body fell out of subset — leave the small native
        // installed forever (it's correct + fast). No retry.
    }
    // Publish freshly-compiled native code to the shared cache so the runtime's other
    // processes install it directly instead of recompiling (the spawn lever). The
    // `swap` guard makes this one lock acquire per arm-instance, not one per call; a
    // process that installed the code *from* the cache already has the flag set.
    // NEVER publish an INLINED arm to the *small*-code `(id, argc)` cache: an adopter installs
    // what it finds there straight into `jit_code` without touching `inline_installed`, so the
    // inlined body would then be entered against small-`nslots` frames → frame undersize /
    // corruption. (The inlined body has its own cache — `jit_inline_publish` above — whose
    // adopters route it through `inline_code` and the swap, which sizes correctly.)
    //
    // NB the reason is the *publishing channel*, not process locality: since ADR-215 peers
    // share this very `CompiledArm` (`compiled_arm_for` → `shared_closures`), so there is no
    // "peer copy with `inline_installed == false`" — this comment used to say there was.
    // Guard on `inline_installed`.
    if !arm.inline_installed.load(Acquire) {
        if let Some(key) = arm.share_key {
            if !arm.shared_published.swap(true, Relaxed) {
                heap.jit_shared_publish(key, code, arm.compile_epoch.load(Acquire));
            }
        }
    }
    // Frame-size agreement (KI-48 family, see `jit_tier_in_frame`). The caller built the
    // frame BEFORE this function re-loaded `jit_code`, so the two can disagree: a peer
    // process of this runtime (which shares this very `CompiledArm` — ADR-215) may have
    // swapped the inlined upgrade into `jit_code` in between. Running the inlined native
    // against the small frame is a raw write past the frame top, so decline and interpret
    // this activation instead — the next entry sizes to the inlined layout and runs it.
    // Costs nothing for the ~all arms that never inline: `inline_nslots` is 0 for them, so
    // the comparison short-circuits before the atomic load.
    if frame_nslots < arm.inline_nslots && frame_nslots < frame_size_for_code(arm, code) {
        return None;
    }
    // SAFETY: `code` is a finalized [`crate::jit::JitArmFn`] produced by `jit_lower_arm`,
    // living in the process-lifetime GLOBAL_JIT module. The frame is set up at
    // `roots[base..]`; the JIT'd arm keeps its own operands in registers (the call staging
    // grows `roots` only transiently, popped before return), so `heap` stays valid for the
    // call.
    let f: crate::jit::JitArmFn = unsafe { std::mem::transmute(code) };
    // Publish this arm's env for the call/global callbacks, save/restoring the previous
    // value so a JIT'd callee that re-enters another JIT'd arm nests correctly.
    let saved_env = std::mem::replace(&mut heap.jit_call_env, env);
    // Best-effort arm name for the staged-stale diagnostic (recursive defns carry
    // `inline_name`; others reset to MAX so the value is never misleadingly stale).
    let saved_fn = std::mem::replace(&mut heap.jit_dbg_fn, arm.dbg_name.unwrap_or(u32::MAX));
    // VM→native entry. This one does not raise `jit_native_depth` itself, so the depth read
    // here is the caller's: `0` means this is the outermost native frame on the thread and
    // the limit has to be derived; anything else is a native→VM→native re-entry on the same
    // stack, where the outer entry's stamp is still the right absolute address.
    let native_depth = heap.jit_native_depth;
    stamp_stack_limit_if_outermost(heap, native_depth);
    let saved_force_vm = heap.jit_force_vm;
    heap.native_gateway_seq += 1;
    let gw_seq = heap.native_gateway_seq;
    let saved_gw = std::mem::replace(&mut heap.cur_native_gateway, gw_seq);
    let outcome = f(heap as *mut Heap, base as i64, out);
    heap.cur_native_gateway = saved_gw;
    heap.jit_force_vm = saved_force_vm;
    heap.jit_call_env = saved_env;
    heap.jit_dbg_fn = saved_fn;
    jit_suspend_feedback(heap, arm, outcome, gw_seq);
    // Outcome 5 = the unboxed-i64 worker hit its native-recursion depth cap. Register recursion
    // can't drain to the VM mid-stack, so permanently switch this fn to the boxed path (which
    // drains deep recursion gracefully via `jit_native_depth`/`jit_force_vm`): mark it too-deep,
    // drop the installed i64 wrapper, and re-tier promptly (→ boxed). Run this activation on the
    // VM. Without this a deep non-tail recursion would deopt-and-re-tier per level (~100× thrash).
    if outcome == 5 {
        if let Some(sym) = arm.dbg_name {
            ActiveBackend::note_depth_bail(sym);
        }
        arm.jit_code.store(std::ptr::null_mut(), Release);
        arm.jit_calls.store(THRESHOLD, Release);
        arm.shared_published.store(false, Relaxed);
        return None;
    }
    // Deopt feedback (watched arms only — a plain bool test for the rest): a
    // type-deopt bumps the consecutive counter (bailing a persistent thrasher);
    // any other outcome resets it.
    if arm.deopt_watch {
        if outcome == 1 {
            jit_deopt_feedback(arm);
        } else if arm.jit_deopts.load(Relaxed) != 0 {
            arm.jit_deopts.store(0, Relaxed);
        }
    }
    Some(outcome)
}
