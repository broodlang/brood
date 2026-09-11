#![cfg(feature = "jit")]
//! JIT tiering runtime glue: the moment an arm crosses its activation threshold
//! (`jit_tier_in_frame`, the two-stage tiering) and the child modules the native code
//! then leans on — `compiler` (the background compile thread and the synchronous
//! compile), `support` (global resolution, error take-out, native stack headroom),
//! `link` (the fast native-to-native link and the inline xcall admission), `dispatch`
//! (the Brood→Brood call and tail-call paths from native code) and `deopt` (frame
//! shapes, checkpoint resume and the deopt/suspend feedback that demotes an arm).
use super::*;

// Everything this module asks of a backend goes through the contract, never through a concrete
// one: `lower_arm`/`lower_inlined_arm` from the two places below (`jit_compile_now` and the
// `JIT_COMPILER` thread — the whole production codegen surface), plus the three tiering
// advisories, which are associated fns precisely so consulting them per activation costs no
// `GLOBAL_JIT` lock. See `crate::jit::backend`.
use crate::jit::{ActiveBackend, JitBackend};

mod compiler;
mod deopt;
mod dispatch;
mod link;
mod support;

pub(crate) use compiler::*;
pub(crate) use deopt::*;
pub(crate) use dispatch::*;
pub(crate) use link::*;
pub(crate) use support::*;

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
