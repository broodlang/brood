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
        Ok(None) | Err(_) => arm.jit_code.store(crate::jit::BAILED, Release),
    }
}

#[cfg(feature = "jit")]
pub(crate) static JIT_COMPILER: std::sync::LazyLock<JitCompiler> = std::sync::LazyLock::new(|| {
    use std::sync::atomic::Ordering::Release;
    use std::sync::mpsc::{sync_channel, TryRecvError};
    let (ptx, prx) = sync_channel::<JitWorkItem>(256);
    let (dtx, drx) = sync_channel::<JitWorkItem>(256);
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
                        jit.lower_inlined_arm(arm, slot_tags)
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
                    Ok(None) => slot.store(crate::jit::BAILED, Release),
                    Err(_) => {
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
    Done(Value),
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
/// `extern "C" fn(*mut Heap, i64) -> i64`. On `Fallthrough` the `argc` args are re-staged at
/// `[stage_base, stage_base+argc)` for the caller's slow path.
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
    let f: extern "C" fn(*mut Heap, i64) -> i64 = unsafe { std::mem::transmute(code as *mut u8) };
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
    let outcome = f(heap as *mut Heap, base as i64);
    heap.set_ic_bases(saved_bases);
    heap.jit_force_vm = saved_force_vm;
    heap.jit_native_depth = native_depth;
    heap.jit_call_env = saved;
    heap.jit_dbg_fn = saved_fn;
    heap.truncate_env_roots(env_base);
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
    match outcome {
        0 => {
            crate::perf_bump!(jit_link_done);
            let result = heap.root_at(base);
            heap.truncate_roots(stage_base);
            FastLinkOutcome::Done(result)
        }
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
                    Ok(v) => FastLinkOutcome::Done(v),
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
                if outcome == 1 && jit_frame_shape_matches(&arm, nslots) {
                    if let Some((resume, rip, depth)) =
                        jit_ckpt_resume(heap, arm.arc(), base, nslots)
                    {
                        return match jit_native_reenter(heap, native_depth, |h| {
                            vm_resume_deopt(h, resume, base, cenv, rip, depth)
                        }) {
                            Ok(v) => FastLinkOutcome::Done(v),
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
                    Ok(v) => FastLinkOutcome::Done(v),
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
            ) {
                FastLinkOutcome::Done(v) => return Some(v),
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
        // `char->int`/`string-length` from JIT'd code used to pay. `apply` itself
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
                // Two-stage tiering: size the callee frame to its *installed* native version
                // (inlined upgrade → `inline_nslots`; small → `nslots`). Capture once and reuse
                // for both frame extension and the outcome-4 staged_start calculation — the two
                // must agree on the same frame boundary.
                let frame_nslots = arm.active_nslots();
                heap.extend_roots_to_nil(stage_base + frame_nslots);
                let base = stage_base;
                // SAFETY: `code` is a finalized `extern "C" fn(*mut Heap, base)` from
                // `jit_lower_arm`, living for the process in `GLOBAL_JIT`; the frame is set
                // up at `roots[base..]`.
                let f: extern "C" fn(*mut Heap, i64) -> i64 = unsafe { std::mem::transmute(code) };
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
                let outcome = f(heap as *mut Heap, base as i64);
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
                    // Done: result boxed in `roots[base]`. Take it, drop the frame.
                    0 => {
                        crate::perf_bump!(jit_link_done);
                        let result = heap.root_at(base);
                        heap.truncate_roots(stage_base);
                        return Some(result);
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
                        if outcome == 1 {
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
) -> Result<ChunkExit, LispError> {
    // Two-stage tiering: a tail call is staged by the native code ABOVE its own frame top,
    // which is `active_nslots` (the inlined upgrade runs with the bigger frame). Use the
    // active size so the staged `[callee, args…]` is read at the right offset.
    let top = base + arm.active_nslots();
    let n = heap.roots_len();
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
/// **Deliberately flag-free** (KI-26). The obvious spelling is `arm.active_nslots() ==
/// frame_nslots`, but `active_nslots()` re-reads `inline_installed` — the anti-pattern behind
/// two ADR-210 bugs — and the inline swap in [`jit_tier`] deliberately does *not* bump the
/// global epoch (a bump cascaded under `pfib`; see the comment at the swap) and invalidates
/// only the installing process's fast links. A `share_key` arm is shared across processes, so
/// a peer can hold a link whose recorded `frame_nslots` predates the swap while the flag now
/// reads true. The flag form then declines, and the caller's fallthrough re-runs the arm from
/// ip 0 — repeating whatever effect the native had already journaled.
///
/// Testing both of the arm's possible frame sizes is a strict superset of the flag form
/// (`active_nslots()` returns exactly one of them), so this only ever *admits* more resumes —
/// and resuming is the effect-preserving direction. Every admitted resume is still validated
/// by [`jit_ckpt_resume`], which requires a positive journal and reads only in-bounds slots.
/// A genuinely foreign arm still fails, which is the out-of-bounds protection this exists for.
#[cfg(feature = "jit")]
pub(crate) fn jit_frame_shape_matches(arm: &CompiledArm, frame_nslots: usize) -> bool {
    frame_nslots == arm.nslots || frame_nslots == arm.inline_nslots
}

/// Deopt-resume checkpoint (see `CompiledArm::ckpt_slot`): decode the live frame's
/// journal — `Some((resume_arm, resume_ip, operand_depth))` when a completed non-tail
/// call (or `table/put`) checkpointed this activation, meaning the VM must resume THERE
/// (the side effects before it already happened, exactly once). `None` ⇒ resume from ip 0,
/// which is then effect-free by construction (everything the boxed subset executes
/// besides calls and `table/put` is pure or idempotent).
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
/// its own — that is what selects the layout (see [`jit_frame_is_leaf_spliced`] for why the
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
    let inlined = jit_frame_is_leaf_spliced(arm, frame_nslots);
    let slot = jit_ckpt_slot(arm, inlined)?;
    let p = match heap.root_at(base + slot as usize) {
        Value::Int(p) if p > 0 => p,
        _ => return None,
    };
    // Continue in whichever layout wrote that journal.
    let resume = if inlined {
        arm.leaf.as_ref()?.resume.clone()
    } else {
        arm.clone()
    };
    Some((resume, (p >> 16) as usize, (p & 0xFFFF) as usize))
}

/// Was the frame at `base` built for the **leaf-spliced** layout (so its journal is the
/// derivation's, at the derivation's slot) rather than the small one?
///
/// Decided by the size the frame was actually built to — **not** by reading
/// `inline_installed`. That flag is flipped by `jit_tier` itself, i.e. exactly between the
/// frame being sized and the deopt being handled, so on the activation where the inlined
/// upgrade installs, reading it afterwards claims the inlined layout for a frame built to
/// the small one: `base + <inlined ckpt slot>` then indexes past the root stack, which
/// surfaced as an out-of-bounds `root_at` inside a later GC walk. Only a *journalled* leaf
/// derivation matters here, and such a derivation's frame is strictly larger than the small
/// one (it splices above `nslots` and then reserves blocks + journal), so the size
/// comparison is unambiguous — asserted below.
#[cfg(feature = "jit")]
fn jit_frame_is_leaf_spliced(arm: &CompiledArm, frame_nslots: usize) -> bool {
    let Some(leaf) = arm.leaf.as_ref() else {
        return false;
    };
    if leaf.resume.ckpt_slot == u32::MAX {
        return false; // unjournalled: nothing of ours to resume into
    }
    debug_assert!(
        arm.inline_nslots > arm.nslots,
        "a journalled leaf layout must be strictly larger than the small one, else the \
         frame-size test below cannot tell them apart"
    );
    frame_nslots == arm.inline_nslots
}

/// The journal slot for the layout live in `arm`'s frame, or `None` when it writes none.
/// Its `inlined` argument comes from [`jit_frame_is_leaf_spliced`], so the slot always
/// belongs to the layout the frame was actually built to.
#[cfg(feature = "jit")]
fn jit_ckpt_slot(arm: &CompiledArm, inlined: bool) -> Option<u32> {
    // A self-spliced native writes no journal, and the small layout's `ckpt_slot` points
    // INTO its spliced slot range, where an ordinary value (a spliced callee's Int param)
    // would fake a journal → a garbage resume ip. `jit_frame_is_leaf_spliced` returns
    // false for it, so this reads the small slot, which its entry reset zeroed.
    let slot = if inlined {
        arm.leaf.as_ref()?.resume.ckpt_slot
    } else {
        arm.ckpt_slot
    };
    (slot != u32::MAX).then_some(slot)
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

#[cfg(feature = "jit")]
pub(crate) fn jit_deopt_feedback(arm: &CompiledArm) {
    use std::sync::atomic::Ordering::{Relaxed, Release};
    const DEOPT_BAIL_CONSECUTIVE: u32 = 16;
    let d = arm.jit_deopts.fetch_add(1, Relaxed) + 1;
    if d >= DEOPT_BAIL_CONSECUTIVE {
        arm.jit_code.store(crate::jit::BAILED, Release);
    }
}

#[cfg(feature = "jit")]
pub(crate) fn jit_tier(
    arm: &Arc<CompiledArm>,
    heap: &mut Heap,
    base: usize,
    env: EnvRoot,
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
        return None; // out of subset — run the VM
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
    //      entry sizes the frame to `active_nslots()` (= `inline_nslots`) and runs the inlined
    //      native. One VM activation on the transition — negligible.
    // i64-eligible arms skip the two-stage inline upgrade entirely: their small native IS the
    // unboxed-i64 register worker (`jit_lower_i64_arm`), which already recurses to full depth in
    // registers — the boxed depth-2 inlined upgrade would only swap in inferior code.
    if arm.inline_name.is_some()
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
            if let Some(key) = arm.share_key {
                if let Some((ptr, epoch)) = heap.jit_inline_lookup(key) {
                    if epoch == heap.global_epoch()
                        && !ptr.is_null()
                        && ptr != crate::jit::BAILED
                        && ptr != crate::jit::QUEUED
                    {
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
            // The inlined upgrade is ready — swap it in. Store `inline_installed` BEFORE
            // `jit_code` so that any reader which Acquire-loads `jit_code = inline_code` is
            // guaranteed (by the Release-Acquire chain) to also see `inline_installed = true`
            // and therefore call `active_nslots()` → `inline_nslots`. The reversed order
            // (jit_code before inline_installed) created a race: a reader could observe the
            // inline code pointer but still see `inline_installed = false`, sizing the callee
            // frame to the small `nslots` — the inline code would then raw-read beyond the
            // frame, picking up stale Vec-capacity data as slot values and passing garbage
            // through the outcome-4 tail-call staging path.
            //
            // This arm is PER-PROCESS ([`compiled_arm_for`] caches it in the process's own
            // `vm_cache`), so the upgrade must only re-point THIS process's fast-links to
            // this callee — NOT bump the shared `global_epoch`. A global bump invalidated
            // every peer process's `compile_epoch` too, so under `pfib` all 100 processes
            // cascaded: each peer nuked its installed code, re-tiered, re-upgraded and
            // re-bumped in turn, permanently diverting calls off the in-IR fast-link onto
            // the slow IC-dispatch path (~2× instructions; the parallel-scaling gap). We keep
            // `compile_epoch` at the current epoch (the arm's inlined operators were just
            // re-validated at compile time) and invalidate only this process's fast-links to
            // this callee, which then re-probe and pick up `inline_code` + `inline_nslots`.
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
            // (the call site reads `active_nslots()`) and runs the inlined native.
            return None;
        }
        // `ic == BAILED`: the inlined body fell out of subset — leave the small native
        // installed forever (it's correct + fast). No retry.
    }
    // Publish freshly-compiled native code to the shared cache so the runtime's other
    // processes install it directly instead of recompiling (the spawn lever). The
    // `swap` guard makes this one lock acquire per arm-instance, not one per call; a
    // process that installed the code *from* the cache already has the flag set.
    // NEVER publish an INLINED arm to the shared `(id, argc)` cache: a peer process that
    // installed it would run the inlined code with its OWN small `nslots` frame (it has its
    // own `CompiledArm` with `inline_installed == false`) → frame undersize / corruption.
    // The inlined upgrade is per-process by design; only the small native is shared (which
    // is the spawn-friendly path anyway). Guard on `inline_installed`.
    if !arm.inline_installed.load(Acquire) {
        if let Some(key) = arm.share_key {
            if !arm.shared_published.swap(true, Relaxed) {
                heap.jit_shared_publish(key, code, arm.compile_epoch.load(Acquire));
            }
        }
    }
    // SAFETY: `code` is a finalized `extern "C" fn(*mut Heap, base) -> i64` produced by
    // `jit_lower_arm`, living in the process-lifetime GLOBAL_JIT module. The frame is set
    // up at `roots[base..]`; the JIT'd arm keeps its own operands in registers (the call
    // staging grows `roots` only transiently, popped before return), so `heap` stays
    // valid for the call.
    let f: extern "C" fn(*mut Heap, i64) -> i64 = unsafe { std::mem::transmute(code) };
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
    let outcome = f(heap as *mut Heap, base as i64);
    heap.jit_force_vm = saved_force_vm;
    heap.jit_call_env = saved_env;
    heap.jit_dbg_fn = saved_fn;
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
