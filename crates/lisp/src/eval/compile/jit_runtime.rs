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
use crate::jit::{ActiveBackend, JitBackend, outcome};

mod compiler;
mod deopt;
mod dispatch;
mod entry;
mod link;
mod support;
mod unwind;

pub(crate) use compiler::*;
pub(crate) use deopt::*;
pub(crate) use dispatch::*;
pub(crate) use entry::*;
pub(crate) use link::*;
pub(crate) use support::*;
pub(crate) use unwind::*;

/// Activations before an arm is handed to the background compiler.
///
/// This is a CALL threshold, and it is deliberately high: boot alone tiers ~139 prelude
/// arms at a threshold of 8, and the compiler drains a FIFO — so a program's own hot
/// function would wait behind boot-path code that ran eight times and never again. 64–128
/// is the plateau; 256+ starts delaying short loops. What this threshold is NOT for
/// is a loop's iteration count — see [`BACKEDGE_TIER_WEIGHT`].
#[cfg(feature = "jit")]
pub(crate) const TIER_THRESHOLD: u32 = 128;

/// What one back-edge boundary exit counts for toward [`TIER_THRESHOLD`].
///
/// A self-tail loop is ONE activation; it reaches the compiler by exiting to the driver
/// every `BACKEDGE_TIER_INTERVAL` (256) iterations while untried, each exit counting as a
/// call. Raising the call threshold alone would therefore delay every loop's tiering by
/// the same factor — `sieve` read +6–7% at 64 and 128 with weight 1, its 1M-iteration
/// `count-primes` interpreting 32k iterations before enqueueing instead of 2k. Weighting a boundary exit
/// at `TIER_THRESHOLD / 8` keeps a spinning loop's tiering where it has always been: eight
/// boundaries, 2048 iterations, whatever the call threshold says.
#[cfg(feature = "jit")]
pub(crate) const BACKEDGE_TIER_WEIGHT: u32 = TIER_THRESHOLD / 8;

/// What a VM→native direct call ([`vm_call_native_direct`]) has to report back to the
/// `Inst::Call` that made it.
#[cfg(feature = "jit")]
pub(crate) enum DirectNative {
    /// The callee returned a value; `roots` is back at the frame base.
    Done(Value),
    /// The callee raised: the error is parked in `Heap::jit_pending_error`, `roots` is back
    /// at the frame base.
    Error,
    /// The callee staged a tail call — `[callee, arg0..]` at `roots[top..]`, above its
    /// frame — for the CALLER to dispatch as its own tail dispatch would: at frame level,
    /// so a `receive` down that chain suspends cleanly instead of parking the worker.
    Tail { top: usize },
    /// The activation has to continue on the VM in its in-place frame (deopt / preempt /
    /// declined / i64 depth bail): hand it to the driver as `ChunkExit::CallResume`.
    Handoff { outcome: Option<i64> },
    /// A preempt unwound out of the callee (ADR-401) — its frames are recorded in
    /// the thread's unwind record (`take_unwound`) and still in `roots`: hand the driver `ChunkExit::Unwound`.
    Unwound,
}

/// Run `arm` **natively, in place**, for a non-tail `Inst::Call` whose `argc` args the caller
/// has laid out at `roots[base..base+argc]` — the VM→native direct call
/// (`docs/compute-frontier.md` §7.12).
///
/// This is the frame path (`ChunkExit::Call` → `push_frame` → the driver's `try_jit` block)
/// with the driver round trip cut out, and **only** that: the frame is built exactly as
/// `push_frame` builds it for an arm with no optionals, rest or captures (params, then nil
/// to `frame_size_for_new_entry()`), the env is rooted, the arm registered if it carries
/// RUNTIME handles, and the native is entered through the very same [`jit_tier_in_frame`]
/// — so the hot-reload epoch guard, the frame-size agreement, the shared-code adoption, the
/// inline/xcall swaps, the suspend-host latch and the deopt feedback are all the frame
/// path's. What differs is only who handles the outcome: a value comes back to the call
/// site (`Done`) instead of through a frame pop; an error is parked as it always is; a
/// staged tail call and every other outcome are handed back so they are honoured at frame
/// level — never nested. The first cut of this call ran the tail chain nested, which put a
/// `receive` two calls down under a native gateway: 19 820 dirty worker parks on the
/// `supervisor` row where the frame path has none.
///
/// `frame_nslots` is returned in every case so a `Handoff` can tell the driver the size the
/// frame was BUILT to (KI-48 family). The caller must have checked eligibility
/// ([`vm_direct_eligible`]) — arity, layout and an installed code pointer — so a `None`
/// from the tier is rare (an epoch reset, a frame-size disagreement, a swap transition or
/// the i64 depth bail) and still correct: the activation simply runs on the VM.
#[cfg(feature = "jit")]
pub(crate) fn vm_call_native_direct(
    heap: &mut Heap,
    arm: &Arc<ArmHandle>,
    argc: usize,
    base: usize,
    callee_env: EnvId,
    callee_bases: (u32, u32),
    unwind: bool,
) -> (DirectNative, usize) {
    debug_assert_eq!(heap.roots_len(), base + argc);
    // The frame the native runs against, sized the way the driver sizes it: `push_frame`'s
    // `nslots` plus the inlined layout's growth when the inlined body is installed. Read
    // ONCE and passed down (`jit_tier_in_frame` declines on a mismatch rather than
    // re-deriving).
    let frame_nslots = arm.frame_size_for_new_entry();
    heap.extend_roots_to_nil(base + frame_nslots);
    let env_base = heap.env_roots_len();
    let env_root = heap.root_env(callee_env);
    let arm_slot = if arm.has_runtime_handles {
        heap.live_arm_push(arm.clone())
    } else {
        usize::MAX
    };
    // KI-20: the callee's native code publishes and probes ITS OWN inline caches through
    // the heap's block cursors, which the driver installs on every frame push. Install
    // them here for the same reason and restore the caller's after: without this the
    // callee's publishes landed in the caller's block at the callee's site indices and
    // both arms ran cache-cold — never a wrong answer (every probe re-validates
    // sym/argc/epoch), but `nbody` read 329k IC hits turning into 304k misses, +24.5%.
    let saved_bases = heap.set_ic_bases(callee_bases);
    let mut ret = Value::Nil;
    let outcome = jit_tier_in_frame(
        arm.arc(),
        heap,
        base,
        env_root,
        frame_nslots,
        &mut ret,
        unwind,
    );
    let env_now = heap.read_root_env(env_root);
    heap.set_ic_bases(saved_bases);
    if arm_slot != usize::MAX {
        heap.live_arm_truncate(arm_slot);
    }
    // The driver re-roots the env for a frame it adopts; the value-returning outcomes are
    // finished with it.
    heap.truncate_env_roots(env_base);
    let r = match outcome {
        Some(outcome::DONE) => {
            crate::perf_bump!(jit_native);
            crate::perf_bump!(vm_native_link);
            heap.truncate_roots(base);
            DirectNative::Done(ret)
        }
        // A preempt unwinding out of this callee (ADR-401): record it and leave its frame
        // for the driver to rebuild.
        Some(outcome::ERROR)
            if jit_unwind_through(heap, outcome::ERROR, unwind, |_| {
                Some(UnwindFrame {
                    arm: arm.arc().clone(),
                    base,
                    nslots: frame_nslots,
                    env: env_now,
                    bases: callee_bases,
                })
            }) =>
        {
            DirectNative::Unwound
        }
        Some(outcome::ERROR) => {
            heap.truncate_roots(base);
            DirectNative::Error
        }
        Some(outcome::TAIL) => {
            crate::perf_bump!(jit_native);
            crate::perf_bump!(vm_native_link);
            DirectNative::Tail {
                top: base + frame_nslots,
            }
        }
        other => DirectNative::Handoff { outcome: other },
    };
    (r, frame_nslots)
}

/// May `exec_chunk` run this call's arm natively in place ([`vm_call_native_direct`])?
///
/// The layout half mirrors `vm_call_ic_fast_link`'s guard — exactly `argc` required params,
/// no optionals, no rest, no captures — because the frame is built by the call site, not by
/// `push_frame`. The code half asks whether native code is INSTALLED right now (a real
/// pointer, not null/`QUEUED`/`BAILED`): a cold or refused arm takes the ordinary
/// `ChunkExit::Call`, so the interpreted case pays one relaxed load and nothing else, and
/// the tier's own bookkeeping for those arms keeps running where it always has.
#[cfg(feature = "jit")]
#[inline]
pub(crate) fn vm_direct_eligible(heap: &Heap, arm: &CompiledArm, argc: usize) -> bool {
    // A receive loop's native code is entered only by the driver (ADR-410): its park leaves
    // native code as a re-run from ip 0, which only the driver can take.
    if heap.jit_force_vm
        || arm.nrequired != argc
        || arm.noptional != 0
        || arm.rest_slot.is_some()
        || !arm.capture_names.is_empty()
        || arm.receive_at_head
    {
        return false;
    }
    let code = arm.jit_code.load(std::sync::atomic::Ordering::Acquire);
    !code.is_null() && code != crate::jit::BAILED && code != crate::jit::QUEUED
}

/// Is `arm`'s installed native code still right although the global epoch moved (KI-303)?
/// Each thing a lowering bakes in that a rebind can change, checked against the runtime as
/// it is now — the way [`LeafInline::still_valid`] checks a leaf derivation, which is also
/// re-checked here:
///
/// - no RUNTIME handle it holds has moved (`code_relocations`, which only a migration, a
///   freed generation or a compaction bumps);
/// - every operator it inlined still resolves to the same native primitive
///   ([`chunk_ops_all_native`] — what an election checks before lowering);
/// - its own name still names it, when the code calls itself by name: a self-tail loop's
///   native back edge, the register worker's direct self-link (a non-tail `Call` on
///   `dbg_name`) and the inlined upgrade's self-recursion all run THIS body without a lookup;
/// - it is not marked stale by a module load (ADR-366).
///
/// Global reads and calls are not in the list: native code re-checks those against the
/// live epoch itself (the global-read cache, the fast links), so they follow a rebind
/// without the arm being reset. Nor is a float context taken from a `def`'d global
/// (`float_globals`, `jit_float_context`): it is a guess behind `as_f64`'s tag guard, so a
/// global that changed type is a deopt, never a miscompile.
#[cfg(feature = "jit")]
pub(crate) fn native_still_valid(heap: &Heap, arm: &CompiledArm, genv: EnvId) -> bool {
    use std::sync::atomic::Ordering::{Acquire, Relaxed};
    if heap.code_relocations() != arm.compile_relocations.load(Acquire)
        || arm.stale_bindings.load(Acquire)
        || !chunk_ops_all_native(heap, arm)
        || arm
            .leaf
            .as_ref()
            .is_some_and(|leaf| !leaf.still_valid(heap))
    {
        return false;
    }
    let Some(name) = arm.dbg_name else {
        // No name, so nothing calls itself by one — but an inlined self-recursion needs it.
        return arm.inline_name.is_none();
    };
    if arm.inline_name.is_some_and(|n| n != name) {
        return false;
    }
    let calls_itself = arm.inline_name.is_some()
        || arm.chunk.as_ref().is_some_and(|c| {
            c.code.iter().any(|i| match i {
                Inst::SelfCall { .. } => true,
                Inst::Call { head: Some(h), .. } => *h == name,
                _ => false,
            })
        });
    if calls_itself {
        record_self_global_ok(arm, heap, genv);
        return arm.self_global_ok.load(Relaxed);
    }
    true
}

/// Would [`jit_tier_in_frame`] return `None` for `arm` without touching anything — and so
/// may the driver skip the tier check (and the frame bookkeeping around it) outright?
/// True for the two early exits every interpreted activation of a refused arm takes: the
/// over-deep drain (`jit_force_vm`) and a `BAILED` arm. The caller has already ruled out a
/// ceiling below Native.
///
/// Lets the driver skip `jit_tier_in_frame`'s preamble and `settle_native_frame` on every
/// non-tail call into an arm the JIT refused. Must mirror `jit_tier_in_frame`'s early
/// returns: anything it would do for a `BAILED` arm, this must answer `false` for.
#[cfg(feature = "jit")]
#[inline]
pub(crate) fn jit_tier_declines(heap: &Heap, arm: &CompiledArm) -> bool {
    use std::sync::atomic::Ordering::Acquire;
    if heap.jit_force_vm {
        return true;
    }
    arm.jit_code.load(Acquire) == crate::jit::BAILED
}

/// Tiering entry (ADR-101 1b). `frame_nslots` is **the size the caller already built this
/// frame to**, and the native entry is declined (`None` — run the VM this activation) when
/// the installed code turns out to want a *bigger* frame than that. There is deliberately no
/// size-free spelling: a caller that cannot say what it built cannot be entered safely.
///
/// The code pointer must be read BEFORE the frame size is trusted (KI-48 family):
/// `vm_run_bc`/`dispatch` read `inline_installed` first (to size the frame), then call
/// this, which Acquire-loads `jit_code` *again*. The two-stage swap stores
/// `inline_installed` before `jit_code` so a reader that sees the inlined pointer also sees
/// the flag — but only if it reads code *first*. Read flag-then-code and a peer swapping in
/// the inlined body in that window leaves the caller holding a small frame while the
/// inlined native raw-writes slots past the frame top.
///
/// (The other native entries — `jit_dispatch_call`, `hof_apply_native`, `jit_run_fast_link` —
/// do not come through here: each loads the code pointer itself and sizes from THAT, via
/// [`frame_size_for_code`], which is the same rule spelled the other way round.)
///
/// An arm [settled](CompiledArm::settled_code) on its installed pointer takes the inline
/// fast path: the guards every entry needs (the drain, the tier ceiling, the epoch and its
/// re-load, the frame size) and the entry. Everything else — tiering, adoption, the
/// upgrades, publishing — is [`jit_tier_in_frame_slow`], which is also where an arm settles.
#[cfg(feature = "jit")]
#[inline]
pub(crate) fn jit_tier_in_frame(
    arm: &Arc<CompiledArm>,
    heap: &mut Heap,
    base: usize,
    env: EnvRoot,
    frame_nslots: usize,
    out: *mut Value,
    // May the activation preempt by unwinding (ADR-401)? Only a top-level driver says yes.
    unwind: bool,
) -> Option<i64> {
    use std::sync::atomic::Ordering::{Acquire, Relaxed};
    // A receive loop runs natively only in an activation that can PARK (ADR-410): one a
    // top-level driver entered, the same entries that may unwind. Under any other — a
    // builtin's callback, a primitive's fallback call — its `receive` blocks the worker, and
    // the suspend-host latch would take the shared arm off native code for every process.
    if arm.receive_at_head && !unwind {
        return None;
    }
    let code = arm.jit_code.load(Acquire);
    if !code.is_null()
        && code == arm.settled_code.load(Relaxed)
        && !heap.jit_force_vm
        && arm.compile_epoch.load(Acquire) == heap.global_epoch()
        // The slow path's re-load (a peer's reset + re-election between the two reads).
        && arm.jit_code.load(Acquire) == code
        && tier_ceiling() >= Tier::Native
        && !crate::diagnostics::debug_flags::no_jit_computed()
        && !(frame_nslots < arm.inline_nslots && frame_nslots < frame_size_for_code(arm, code))
    {
        return run_installed(arm, heap, base, env, frame_nslots, out, unwind, code);
    }
    jit_tier_in_frame_slow(arm, heap, base, env, frame_nslots, out, unwind)
}

#[cfg(feature = "jit")]
#[inline(never)]
fn jit_tier_in_frame_slow(
    arm: &Arc<CompiledArm>,
    heap: &mut Heap,
    base: usize,
    env: EnvRoot,
    frame_nslots: usize,
    out: *mut Value,
    unwind: bool,
) -> Option<i64> {
    use std::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed, Release};
    // Draining an over-deep native-recursion subtree on the VM (see [`JIT_FORCE_VM`]):
    // interpret this arm so its recursion stays in the bounded heap-frame loop.
    if heap.jit_force_vm {
        return None;
    }
    // Tier ceiling below Native (ADR-222, `BROOD_TIER=1`): never compile or run native —
    // interpret on the (correct) tier 1. Returns before the hotness count + the
    // background-compile enqueue CAS, so no arm is ever handed to the compiler and no native
    // pointer is installed, so the fast-link / dispatch paths have nothing to call either.
    if tier_ceiling() < Tier::Native {
        return None;
    }
    // `BROOD_NO_JIT_COMPUTED=1` (bisect): keep arms with a computed-head non-tail call on the VM.
    if crate::diagnostics::debug_flags::no_jit_computed()
        && let Some(c) = arm.chunk.as_ref()
        && c.code.iter().any(|i| {
            matches!(
                i,
                Inst::Call {
                    tail: false,
                    head: None,
                    ..
                }
            )
        })
    {
        return None;
    }
    let mut code = arm.jit_code.load(Acquire);
    if code == crate::jit::BAILED {
        return None; // out of subset — run the VM
    }
    // Shared-JIT install (the spawn lever): before this process spends TIER_THRESHOLD
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
    if (code.is_null() || code == crate::jit::QUEUED)
        && ActiveBackend::may_adopt_shared_code(arm)
        && let Some(key) = arm.share_key
        && let Some((ptr, epoch)) = heap.jit_shared_lookup(key)
        && epoch == heap.global_epoch()
        && !ptr.is_null()
        && ptr != crate::jit::BAILED
        && ptr != crate::jit::QUEUED
    {
        arm.compile_relocations
            .store(heap.code_relocations(), Release);
        arm.compile_epoch.store(epoch, Release);
        // Trace the ADOPT path: this arm installs a peer's compiled pointer
        // WITHOUT lowering, so it never reaches the `BROOD_JIT_DUMP_IR` dump.
        // Without this trace an arm can hold native code, deopt out of it, and
        // still be absent from every IR dump.
        {
            if crate::diagnostics::debug_flags::jit_bail_trace() {
                let name = arm.dbg_label();
                eprintln!(
                    "[jit-ir] arm={name} adopted-shared-code nslots={} (not lowered \
                                 here, so it emits no IR dump)",
                    arm.nslots
                );
            }
        }
        // Over what was read, or not at all: a peer may have latched or installed this arm
        // since (KI-309) — or latched its function, which this copy then joins (KI-313).
        if !heap.jit_suspend_admit(arm) || !arm.replace_code(code, ptr) {
            return None;
        }
        arm.shared_published.store(true, Relaxed); // already in the cache
        code = ptr;
    }
    if code == crate::jit::QUEUED {
        return None; // compile in flight, nothing published yet — run the VM
    }
    if code.is_null() {
        // Count the invocation; only enqueue once the arm is hot.
        if arm.jit_calls.fetch_add(1, Relaxed) + 1 < TIER_THRESHOLD {
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
            // stays on the VM — so name it, or the arm sits on the interpreter permanently
            // with no diagnostic pointing at the reason.
            if crate::diagnostics::debug_flags::jit_bail_trace() {
                let name = arm.dbg_label();
                eprintln!("[jit-bail] arm={name} reason=chunk-ops-not-all-native");
            }
            arm.latch_bailed("chunk-ops-not-all-native");
            return None;
        }
        // ADR-335: load what the arm names BEFORE it goes native (see `preload_arm_globals`) —
        // and BEFORE the compile epoch is recorded, or the load's own `def`s would stale the
        // epoch we are about to stamp. If it loaded anything, this chunk was compiled without
        // those bindings (ADR-366): it must not be lowered as it stands — its calls into the
        // module are generic where the recompile inlines them, and its leaf derivation is
        // stale by construction (`leaf-derivation-stale` was this, permanently). Mark it and
        // run the VM; the next activation recompiles and re-tiers from the better chunk.
        {
            let preloaded = preload_arm_globals(arm, heap, heap.read_root_env(env));
            // A module it names is mid-load in another process: stay on the VM, whose miss
            // parks, and re-elect later — not BAILED, the load will finish.
            let Some(loaded) = preloaded else {
                if crate::diagnostics::debug_flags::jit_bail_trace() {
                    let name = arm.dbg_label();
                    eprintln!("[jit-bail] arm={name} reason=module-load-in-flight (retries)");
                }
                arm.jit_calls.store(0, Relaxed);
                return None;
            };
            if loaded {
                heap.mark_arm_stale(arm);
                if crate::diagnostics::debug_flags::trace_compile() {
                    let name = arm.dbg_label();
                    eprintln!(
                        "[compile] stale-bindings arm={name}: the tier-up pre-load loaded a module; recompiles at its next activation"
                    );
                }
                return None;
            }
        }
        // An arm whose non-tail callees reach a `receive` would host a parking receive under
        // its gateway — `jit_latch_suspend_host` would latch it after the first dirty park.
        // Refuse it here, by name, instead (`arm_hosts_receive`'s doc has the shape).
        if crate::eval::compile::arm_hosts_receive(heap, arm, heap.read_root_env(env)) {
            if crate::diagnostics::debug_flags::jit_bail_trace() {
                let name = arm.dbg_label();
                eprintln!("[jit-bail] arm={name} reason=hosts-receive");
            }
            arm.latch_bailed("hosts-receive");
            return None;
        }
        // A self-tail loop reached while its `dbg_name` no longer names it — an alias called
        // after the name was rebound — must hand its tail call to the NEW binding, and only
        // the VM's `Inst::SelfCall` guard does that: the native back-edge loops on the old
        // body unconditionally. So such an arm stays on the VM, and (unlike a lowering
        // bail) not for good: back the hotness counter off, so a later election re-observes
        // the binding — a `def` restoring it lets the arm tier again.
        {
            let genv = heap.read_root_env(env);
            record_self_global_ok(arm, heap, genv);
            if arm.dbg_name.is_some()
                && !arm.self_global_ok.load(Relaxed)
                && arm
                    .chunk
                    .as_ref()
                    .is_some_and(|c| c.code.iter().any(|i| matches!(i, Inst::SelfCall { .. })))
            {
                if crate::diagnostics::debug_flags::jit_bail_trace() {
                    let name = arm.dbg_label();
                    eprintln!("[jit-bail] arm={name} reason=self-loop-name-rebound");
                }
                arm.jit_calls.store(0, Relaxed);
                return None;
            }
        }
        // A leaf derivation is valid only at the epoch it is stamped with, and the compiler
        // thread refuses a stale one for good (`leaf-derivation-stale`): one `def` or module
        // load between an arm's compile and this tier-up kept its small native for the life
        // of the program — `json`'s `skip-ws`, `digits-run` and `json-object` on every run.
        // Nearly always the epoch moved for something the derivation does not read, so check
        // what it does read and move its stamp forward. Only a callee or primitive actually
        // rebound costs a recompile, which derives afresh at the next activation.
        if let Some(leaf) = arm.leaf.as_ref() {
            let epoch = heap.global_epoch();
            if leaf.epoch.load(Acquire) != epoch {
                if leaf.still_valid(heap) {
                    leaf.epoch.store(epoch, Release);
                } else {
                    heap.mark_arm_stale(arm);
                    if crate::diagnostics::debug_flags::trace_compile() {
                        let name = arm.dbg_label();
                        eprintln!(
                            "[compile] stale-leaf arm={name}: a callee its leaf derivation spliced was rebound; recompiles at its next activation"
                        );
                    }
                    return None;
                }
            }
        }
        arm.compile_relocations
            .store(heap.code_relocations(), Release);
        arm.compile_epoch.store(heap.global_epoch(), Release);
        // A copy of a function another copy was suspend-latched for stays on the VM (KI-313).
        if !heap.jit_suspend_admit(arm) {
            return None;
        }
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
            let slot_tags = profile_slot_tags(heap, arm, base);
            // The frame profile types only *params*; record the arm's float-valued free
            // globals too, so a float-context arm whose floats arrive from a `def`'d
            // constant isn't lowered onto the integer path (see `record_float_globals`).
            let genv = heap.read_root_env(env);
            record_float_globals(arm, heap, genv);
            if JIT_COMPILER
                .enqueue((arm.clone(), slot_tags, heap.runtime_tag()))
                .is_err()
            {
                // The background compile queue is full (a burst of distinct hot arms — e.g.
                // thousands of short-lived green processes each tiering their own arm copy,
                // overwhelming the bounded channel). Reset to untried AND back the hotness
                // counter all the way off, so the arm runs on the VM for another TIER_THRESHOLD
                // calls before re-attempting — instead of re-validating (`chunk_ops_all_native`,
                // an `env_get`/`resolve_prim` per op) on *every* call while the queue stays
                // full (tens of millions of redundant re-validations under a spawn storm).
                // The arm still compiles once the queue
                // drains (a long-lived process re-reaches the threshold and re-enqueues).
                arm.replace_code(crate::jit::QUEUED, std::ptr::null_mut());
                arm.jit_calls.store(0, Relaxed);
            }
        }
        return None;
    }
    // A real, installed code pointer. Hot-reload guard: if the global epoch moved since
    // the arm was compiled, some rebind happened somewhere in the runtime. Nearly always it
    // is one this arm's native code does not depend on — another module's `def`, a module
    // load, a registry write, an isolate restore — and resetting for those kept a hot arm
    // re-compiling for as long as anything was being defined: under a steady trickle of
    // unrelated `def`s an inner loop never left `:queued` (KI-303). So check what the native
    // code baked in (`native_still_valid`) and move the stamp forward when none of it
    // changed; only a real change resets the arm, which re-tiers, re-validating operators
    // and recompiling at the new epoch, or bailing if one was genuinely redefined.
    {
        let epoch = heap.global_epoch();
        if arm.compile_epoch.load(Acquire) != epoch {
            if !native_still_valid(heap, arm, heap.read_root_env(env)) {
                deopt::reset_native_state(arm);
                return None;
            }
            // `epoch` was read before the checks: a rebind racing them moves the epoch past
            // it, so the next entry checks again. The leaf derivation was just re-checked
            // too; its stamp moves first, so a lowering that reads the arm's stamp and then
            // the derivation's never sees the derivation behind (`jit_lower`'s
            // `leaf-derivation-stale`).
            if let Some(leaf) = arm.leaf.as_ref() {
                leaf.epoch.fetch_max(epoch, Release);
            }
            arm.compile_epoch.fetch_max(epoch, Release);
        }
    }
    // `code` and `compile_epoch` were read separately, and a peer (this `CompiledArm` is
    // shared across the runtime's processes, ADR-215) may have reset the arm and re-elected
    // it between the two: it stores the NEW epoch before its CAS, so the check above passes
    // against a pointer lowered for the old one — stale inlined operators after a reload.
    // The peer's null store precedes its `compile_epoch` Release store, so if we saw that
    // epoch this Acquire re-load sees the reset (or later); JIT code is never freed, so a
    // pointer cannot come back as a different arm's.
    if arm.jit_code.load(Acquire) != code {
        return None;
    }
    // ---- Two-stage tiering: the deferred inlined upgrade ----
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
        // Profitability: the inline blob adds ~90 CLIF lines and several blocks PER CALL
        // SITE, and every value live across a call is pressured through that extra CFG —
        // so an arm carrying lots of live state pays in its own loop code what the calls
        // save. `nslots` is that live state's size: the winners sit at 3–5 slots, the
        // loser (nbody's `advance-body`) at 20, and 8 leaves margin on both sides.
        && arm.nslots <= XCALL_RELOWER_MAX_NSLOTS
        // The arm's own answer first: it is cached, and for an arm with no non-tail named
        // call (a leaf) it ends the test here, where the global switch below is a call —
        // on every native entry.
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
        })
        && super::xcall_relower_enabled();
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
            if let Some(key) = arm.share_key.filter(|_| arm.inline_name.is_some())
                && let Some((ptr, epoch)) = heap.jit_inline_lookup(key)
                && epoch == heap.global_epoch()
                && !ptr.is_null()
                && ptr != crate::jit::BAILED
                && ptr != crate::jit::QUEUED
            {
                {
                    if crate::diagnostics::debug_flags::jit_bail_trace() {
                        let name = arm.dbg_label();
                        eprintln!(
                            "[jit-ir] arm={name} adopted-inline-code-from-cache nslots={} (not lowered here, emits no IR dump)",
                            arm.nslots
                        );
                    }
                }
                arm.inline_code.store(ptr, Release);
                return None; // next entry swaps it in
            }
            // Not shared yet. Elect a single enqueuer via the queued flag — loaded first, so
            // the entries that run while the deferred compile waits (a spawn storm's whole
            // run) do not each write this shared arm's cache line.
            if !arm.inline_queued.load(Relaxed) && !arm.inline_queued.swap(true, AcqRel) {
                let slot_tags = profile_slot_tags(heap, arm, base);
                // Deferred (low-priority). On a full queue, un-set `inline_queued` so a
                // later call re-attempts — but DON'T disturb the running small native.
                if JIT_COMPILER
                    .defer((arm.clone(), slot_tags, heap.runtime_tag()))
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
                // Over the small body this entry read, or not at all (a peer latched or
                // swapped first, KI-309).
                if !arm.replace_code(code, ic) {
                    return None;
                }
                // Nothing is left to swap for the re-lowered body, but its slot now reads null
                // like a compile in flight, which the settle test below must treat as pending
                // — so it settles here. Its publish was the small body's (the flag stays set).
                arm.settled_code.store(ic, Relaxed);
                if let Some(sym) = arm.dbg_name {
                    heap.invalidate_fast_links_for(sym);
                }
                {
                    if crate::diagnostics::debug_flags::jit_bail_trace() {
                        let name = arm.dbg_label();
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
            // **The real invariant.** Since ADR-215 this `CompiledArm` is NOT per-process: `compiled_arm_for` hands
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
            // exactly the KI-48 family, and why the frame-building callers go through
            // `jit_tier_in_frame` / `frame_size_for_code`.
            //
            // `vm_call_ic_fast_link` (`core/heap/vm_cache.rs`) records such a snapshot too, and
            // sizes it from the pointer it loaded (`frame_size_for_code(arm, code)`) rather
            // than from a second read of `inline_installed` — the same rule.
            {
                if crate::diagnostics::debug_flags::jit_bail_trace() {
                    let name = arm.dbg_label();
                    eprintln!(
                        "[jit-ir] arm={name} inline-swap-installed nslots={} inline_nslots={}",
                        arm.nslots, arm.inline_nslots
                    );
                }
            }
            arm.inline_installed.store(true, Release); // BEFORE jit_code — see comment above
            // Over the small body this entry read, or not at all: latched since, the arm is
            // never entered, so the flag above means nothing; swapped by a peer, it holds this
            // same upgrade (KI-309).
            if !arm.replace_code(code, ic) {
                return None;
            }
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
    // process that installed the code *from* the cache already has the flag set. The plain
    // load in front keeps the `swap` itself — a locked RMW on a `CompiledArm` every process
    // of the runtime shares — off the per-call path once the flag is set: without it every
    // native entry wrote the arm's cache line, and a fan-out bounced it between cores.
    // NEVER publish an INLINED arm to the *small*-code `(id, argc)` cache: an adopter installs
    // what it finds there straight into `jit_code` without touching `inline_installed`, so the
    // inlined body would then be entered against small-`nslots` frames → frame undersize /
    // corruption. (The inlined body has its own cache — `jit_inline_publish` above — whose
    // adopters route it through `inline_code` and the swap, which sizes correctly.)
    //
    // NB the reason is the *publishing channel*, not process locality: since ADR-215 peers
    // share this very `CompiledArm` (`compiled_arm_for` → `shared_closures`), so there is no
    // "peer copy with `inline_installed == false`". Guard on `inline_installed`.
    if !arm.inline_installed.load(Acquire)
        && let Some(key) = arm.share_key
        && !arm.shared_published.load(Relaxed)
        && !arm.shared_published.swap(true, Relaxed)
    {
        heap.jit_shared_publish(key, code, arm.compile_epoch.load(Acquire));
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
    // Settle on this pointer once nothing above has work left for it: no inlined or
    // re-lowered upgrade still to come, and the shared publish done. Only a BAILED upgrade
    // slot is finished: a null one is either unqueued or a deferred compile in flight, whose
    // pointer lands in the slot for a LATER entry to swap in — so it must keep looking. (The
    // re-lowered body's swap nulls the slot too; it settles its new pointer itself.)
    {
        let upgrade_pending = (arm.inline_name.is_some() || xcall_relower)
            && !arm.inline_installed.load(Acquire)
            && !ActiveBackend::declines_inline_upgrade(arm)
            && arm.inline_code.load(Acquire) != crate::jit::BAILED;
        let publish_pending = !arm.inline_installed.load(Acquire)
            && arm.share_key.is_some()
            && !arm.shared_published.load(Relaxed);
        if !upgrade_pending && !publish_pending {
            arm.settled_code.store(code, Relaxed);
        }
    }
    run_installed(arm, heap, base, env, frame_nslots, out, unwind, code)
}

/// Enter `arm`'s installed `code` in the frame the caller built, and take its outcome — the
/// tail both tier-in paths share.
#[cfg(feature = "jit")]
#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn run_installed(
    arm: &Arc<CompiledArm>,
    heap: &mut Heap,
    base: usize,
    env: EnvRoot,
    frame_nslots: usize,
    out: *mut Value,
    unwind: bool,
    code: *mut u8,
) -> Option<i64> {
    // The driver installed this frame's IC cursors, so the entry keeps them (`None`). The
    // name is best-effort for the staged-stale diagnostic (recursive defns carry
    // `inline_name`; others reset to MAX so the value is never misleadingly stale).
    let exit = enter_native(
        heap,
        NativeEntry {
            code,
            base,
            env: EntryEnv::Rooted(env),
            dbg_fn: arm.dbg_name.unwrap_or(u32::MAX),
            ic_bases: None,
            from: EntryFrom::Driver,
            unwind,
        },
        out,
    );
    native_feedback(heap, arm, base, frame_nslots, &exit);
    let outcome = exit.outcome;
    // DEPTH_BAIL = the unboxed-i64 worker hit its native-recursion depth cap. Register recursion
    // can't drain to the VM mid-stack, so permanently switch this fn to the boxed path (which
    // drains deep recursion gracefully via `jit_native_depth`/`jit_force_vm`): mark it too-deep,
    // drop the installed i64 wrapper, and re-tier promptly (→ boxed). Run this activation on the
    // VM. Without this a deep non-tail recursion would deopt-and-re-tier per level (~100× thrash).
    if outcome == outcome::DEPTH_BAIL {
        depth_bail_to_boxed(arm);
        return None;
    }
    Some(outcome)
}

#[cfg(feature = "jit")]
#[cold]
fn depth_bail_to_boxed(arm: &CompiledArm) {
    use std::sync::atomic::Ordering::{Relaxed, Release};
    if let Some(sym) = arm.dbg_name {
        ActiveBackend::note_depth_bail(sym);
    }
    if !arm.clear_code_unless_latched() {
        return;
    }
    arm.jit_calls.store(TIER_THRESHOLD, Release);
    arm.shared_published.store(false, Relaxed);
}
