//! Entering a native activation from Rust, and finishing the ones whose caller wants a value.
//!
//! Every Rust caller that runs native code — the driver's tier-in ([`jit_tier_in_frame`]), the
//! slow call out of native code ([`jit_dispatch_call`]), the fast link ([`jit_run_fast_link`])
//! and a builtin calling back ([`super::super::closure::hof_apply_native`]) — does the same
//! things around the call: install the callee's env, name, IC cursors, depth and gateway,
//! build its [`crate::jit::JitCallCtx`], call, and restore. [`enter_native`] is that ceremony,
//! once. The native call emitted inside native code does it in IR instead
//! (`jit_lower/call.rs`) and shares only the outcome half.
//!
//! The outcome half has two shapes. A **driver** entry hands every outcome but a value back
//! to the bytecode driver, which owns the frame. A **nested** entry — any caller that must
//! produce a value then and there — finishes the outcome itself: a preempt unwinding through
//! it is recorded (ADR-401), an error propagates, a staged tail call is followed, and a
//! deopt or preempt resumes at the callee's checkpoint or re-runs it on the VM.
//! [`finish_nested`] is that, once, for every nested caller.

use super::*;

/// Where a native entry comes from, which decides what it does to the native depth.
#[derive(Clone, Copy)]
pub(crate) enum EntryFrom {
    /// The bytecode driver (or a test standing in for it): the activation runs at the
    /// caller's depth, and the outermost one on the thread stamps the stack limit.
    Driver,
    /// Rust code running on behalf of a native caller, or a builtin calling back: the
    /// activation is one level deeper, which is what bounds a native recursion.
    Native,
}

/// One native activation to enter from Rust: its installed code and the frame the caller
/// built for it at `roots[base..]`.
pub(crate) struct NativeEntry {
    /// A finalized [`crate::jit::JitArmFn`], epoch-current, whose frame size the caller
    /// built `roots[base..]` to.
    pub(crate) code: *const u8,
    pub(crate) base: usize,
    /// The callee's env, read back live afterwards.
    pub(crate) env: EntryEnv,
    /// The arm's name for diagnostics (`Heap::jit_dbg_fn`).
    pub(crate) dbg_fn: Symbol,
    /// The callee's IC block cursors, or `None` when the caller has installed them.
    pub(crate) ic_bases: Option<(u32, u32)>,
    pub(crate) from: EntryFrom,
    /// May the activation preempt by unwinding (`JitCallCtx::unwind`, ADR-401)?
    pub(crate) unwind: bool,
}

/// The env a native activation runs in.
#[derive(Clone, Copy)]
pub(crate) enum EntryEnv {
    /// Not rooted yet: the entry roots it for the call.
    Bare(EnvId),
    /// Already rooted by the caller, which keeps it rooted across the call — the driver's
    /// tier-in, whose frame owns the root. Rooting it a second time bought nothing.
    Rooted(EnvRoot),
}

/// What came back from a native activation.
pub(crate) struct NativeExit {
    /// The `JitArmFn` outcome: 0 done, 1 deopt, 2 preempt, 3 error, 4 tail call, 5 the
    /// unboxed worker's depth bail.
    pub(crate) outcome: i64,
    /// The activation's gateway token, for the suspend-host latch.
    pub(crate) gateway: u64,
    /// The callee's env as it is now — a collection inside the call may have moved it.
    pub(crate) env: EnvId,
}

/// Run one native activation: install its state in the heap fields the runtime's callbacks
/// read, call it, and restore them. `out` receives the value on outcome 0 (see
/// [`crate::jit::JitArmFn`] for why it is a pointer and what may not happen before its
/// consumer reads it).
///
/// The callee's native code reads its OWN inline caches through the heap's IC cursors
/// (KI-20) and is named by the env, name and depth fields; the inline native→native call
/// does not restore those (rung A1), so whatever this activation's callees asserted into
/// them is undone here — without that the depth leaked upward one per chain (`bintree`
/// reached depth 75 and every inline call fell to the callback path).
#[inline(always)]
pub(crate) fn enter_native(heap: &mut Heap, entry: NativeEntry, out: *mut Value) -> NativeExit {
    // SAFETY: the caller hands over a finalized `JitArmFn` kept alive for the process in
    // `GLOBAL_JIT`, epoch-checked, with its frame built at `roots[base..]`.
    let f: crate::jit::JitArmFn = unsafe { std::mem::transmute(entry.code) };
    let env_base = heap.env_roots_len();
    let env_root = match entry.env {
        EntryEnv::Bare(e) => heap.root_env(e),
        EntryEnv::Rooted(r) => r,
    };
    let saved_env = std::mem::replace(&mut heap.jit_call_env, env_root);
    let saved_fn = std::mem::replace(&mut heap.jit_dbg_fn, entry.dbg_fn);
    let depth = heap.jit_native_depth;
    match entry.from {
        // `0` means the outermost native frame on the thread, which derives the stack limit;
        // deeper, the outer entry's stamp is still the right absolute address. A nested
        // entry never stamps: re-deriving it through `stacker::remaining_stack` on every
        // depth-0 call was 6% of a call loop.
        EntryFrom::Driver => stamp_stack_limit_if_outermost(heap, depth),
        EntryFrom::Native => heap.jit_native_depth = depth + 1,
    }
    let saved_force_vm = heap.jit_force_vm;
    let saved_bases = match entry.ic_bases {
        Some(b) => heap.set_ic_bases(b),
        None => heap.ic_bases(),
    };
    heap.native_gateway_seq += 1;
    let gateway = heap.native_gateway_seq;
    let saved_gw = std::mem::replace(&mut heap.cur_native_gateway, gateway);
    let mut ctx = crate::jit::JitCallCtx::from_heap(heap);
    ctx.unwind = entry.unwind as u8;
    let outcome = f(heap as *mut Heap, entry.base as i64, out, &mut ctx);
    heap.cur_native_gateway = saved_gw;
    heap.set_ic_bases(saved_bases);
    heap.jit_force_vm = saved_force_vm;
    heap.jit_native_depth = depth;
    heap.jit_call_env = saved_env;
    heap.jit_dbg_fn = saved_fn;
    let env = heap.read_root_env(env_root);
    heap.truncate_env_roots(env_base);
    NativeExit {
        outcome,
        gateway,
        env,
    }
}

/// What every entry that has its arm in hand learns from an exit: the suspend-host latch
/// (`jit_suspend_feedback`) and the deopt feedback — every type-deopt counted, and a watched
/// arm's consecutive count latched or reset. `base`/`nslots` locate the frame for the
/// `BROOD_JIT_BAIL_TRACE` line naming the instruction a deopt left at.
///
/// A clean return with nothing blocked under it and no deopt streak to reset — nearly every
/// exit — has no feedback to give: that test is inline, so the common exit pays it and not
/// a call.
#[inline(always)]
pub(crate) fn native_feedback(
    heap: &mut Heap,
    arm: &Arc<CompiledArm>,
    base: usize,
    nslots: usize,
    exit: &NativeExit,
) {
    use std::sync::atomic::Ordering::Relaxed;
    if exit.outcome == 0
        && heap.blocked_under_gateway != exit.gateway
        && !(arm.deopt_watch && arm.jit_deopts.load(Relaxed) != 0)
    {
        return;
    }
    native_feedback_slow(heap, arm, base, nslots, exit);
}

#[inline(never)]
fn native_feedback_slow(
    heap: &mut Heap,
    arm: &Arc<CompiledArm>,
    base: usize,
    nslots: usize,
    exit: &NativeExit,
) {
    use std::sync::atomic::Ordering::Relaxed;
    jit_suspend_feedback(heap, arm, exit.outcome, exit.gateway);
    if exit.outcome == outcome::DEOPT {
        if crate::diagnostics::debug_flags::jit_bail_trace() {
            trace_deopt_site(heap, arm, base, nslots);
        }
        jit_any_deopt_feedback(heap, arm);
        if arm.deopt_watch {
            jit_deopt_feedback(heap, arm);
        }
    } else if arm.deopt_watch && arm.jit_deopts.load(Relaxed) != 0 {
        arm.jit_deopts.store(0, Relaxed);
    }
}

/// Name WHERE a native deopted (KI-48 follow-on): the difference between "this arm thrashes"
/// and "this arm thrashes at instruction N, a `Call` to `vector-length`".
#[cold]
fn trace_deopt_site(heap: &Heap, arm: &Arc<CompiledArm>, base: usize, nslots: usize) {
    let name = arm.dbg_label();
    let reason = crate::eval::compile::DeoptReason::describe(heap.jit_deopt_reason());
    match jit_ckpt_resume(heap, arm, base, nslots) {
        Some((resume, ip, _)) => {
            let op = resume
                .chunk
                .as_ref()
                .and_then(|c| c.code.get(ip))
                .map(crate::eval::compile::jit_plan::codegen::inst_opcode_name)
                .unwrap_or("<out-of-range>");
            eprintln!("[jit-deopt] arm={name} resume_ip={ip} op={op} reason={reason} (journalled)");
        }
        None => eprintln!("[jit-deopt] arm={name} no journal — re-runs from ip 0 reason={reason}"),
    }
}

/// The frame a nested native call ran in.
pub(crate) struct NestedFrame {
    /// Where the caller's staged operands began: every finished outcome truncates to it.
    pub(crate) stage_base: usize,
    /// The callee's frame, and the size it was built to.
    pub(crate) base: usize,
    pub(crate) nslots: usize,
    /// The callee's arguments sit in its first `argc` slots — a re-run reads them there.
    pub(crate) argc: usize,
    /// The depth the CALLER runs at: a re-entry from here runs one deeper (KI-11).
    pub(crate) native_depth: u32,
    /// Was the callee entered with `JitCallCtx::unwind` set?
    pub(crate) unwind: bool,
}

/// The callee of a nested call, as [`finish_nested`] needs it for a re-run, a checkpoint
/// resume or an unwind record: its arm, its env, and whether the arm is certainly the one
/// whose native code ran (only such an arm may be rebuilt as an interpreter frame).
pub(crate) type NestedCallee = (Arc<ArmHandle>, EnvId, bool);

/// How a nested native call finished.
pub(crate) enum Nested {
    /// It produced a value.
    Done(Value),
    /// It raised: the error is parked in `Heap::jit_pending_error` for the native caller to
    /// propagate — a real error, or a preempt unwinding through (whose frames stay in
    /// `roots` for the driver).
    Raised,
    /// The callee could not be named to re-run it: the frame is dropped and the arguments
    /// re-staged at `stage_base` for the caller's slow path.
    Unresolved,
}

/// Finish a nested native call's `outcome` (see the module doc). `ret` is the value outcome 0
/// wrote; `resolve` names the callee, and is asked at most once, only when an outcome needs
/// it. `count_deopt`: this caller has not given the arm its deopt feedback yet (the fast link,
/// which carries no arm on its hot path).
///
/// Every re-entry into the evaluator from here runs with the native depth raised (KI-11): the
/// native frame that made this call is still on the stack, and with the depth rolled back a
/// chain of tail-calling delegators oscillated between two depths while the stack grew
/// without bound.
pub(crate) fn finish_nested(
    heap: &mut Heap,
    outcome: i64,
    ret: Value,
    frame: &NestedFrame,
    count_deopt: bool,
    resolve: impl FnOnce(&Heap) -> Option<NestedCallee>,
) -> Nested {
    let mut resolve = Some(resolve);
    let mut callee: Option<NestedCallee> = None;
    // A preempt this callee may honour, or one unwinding through it (ADR-401): record it and
    // leave its frame where it lies. Only an arm known to be the one that ran is recorded;
    // anything else takes the preempt the old way, re-run below.
    if jit_unwind_through(heap, outcome, frame.unwind, |h| {
        callee = resolve.take().and_then(|r| r(h));
        callee
            .as_ref()
            .filter(|(_, _, exact)| *exact)
            .map(|(arm, env, _)| UnwindFrame {
                arm: arm.arc().clone(),
                base: frame.base,
                nslots: frame.nslots,
                env: *env,
                bases: h.vm_arm_block(arm),
            })
    }) {
        return Nested::Raised;
    }
    let reenter_result = |heap: &mut Heap, r: LispResult| match r {
        Ok(v) => Nested::Done(v),
        Err(e) => {
            heap.jit_pending_error = Some(e);
            Nested::Raised
        }
    };
    match outcome {
        0 => {
            crate::perf_bump!(jit_link_done);
            heap.truncate_roots(frame.stage_base);
            Nested::Done(ret)
        }
        // Propagate — never re-run, or an already-failed subtree re-errors at every
        // unwinding level (quadratic).
        3 => {
            heap.truncate_roots(frame.stage_base);
            Nested::Raised
        }
        // The callee staged a tail call — `[callee, arg0..]` above its frame. Follow it as if
        // the callee had returned its value, rather than re-running the callee on the VM
        // (both JIT and VM cost for every thin delegator).
        4 => {
            let local = heap.take_jit_tail_local_head();
            let staged = frame.base + frame.nslots;
            let end = heap.roots_len();
            if end <= staged {
                heap.truncate_roots(frame.stage_base);
                heap.jit_pending_error =
                    Some(LispError::type_err("a native tail call staged no call"));
                return Nested::Raised;
            }
            let target = heap.root_at(staged);
            let args: SmallVec<[Value; 4]> = (staged + 1..end).map(|k| heap.root_at(k)).collect();
            heap.truncate_roots(frame.stage_base);
            let g = heap.global();
            let r = jit_native_reenter(heap, frame.native_depth, |h| {
                apply_value(h, target, &args, g)
                    .map_err(|e| crate::eval::name_uncallable_local(h, e, target, local))
            });
            reenter_result(heap, r)
        }
        // Deopt (1) / preempt (2) / anything else: resume at the checkpoint, frame intact —
        // never re-running a side effect — else re-run on the VM from the arguments, which
        // the body never overwrites. `1 | 2` alike: the journal, not the outcome code,
        // decides whether there is a checkpoint (KI-18).
        _ => {
            crate::perf_bump!(jit_link_rerun);
            let argv: SmallVec<[Value; 4]> = (0..frame.argc)
                .map(|k| heap.root_at(frame.base + k))
                .collect();
            let callee = match resolve.take() {
                Some(r) => r(heap),
                None => callee,
            };
            let Some((arm, env, _)) = callee else {
                heap.truncate_roots(frame.stage_base);
                for a in &argv {
                    heap.push_root(*a);
                }
                return Nested::Unresolved;
            };
            if count_deopt && outcome == outcome::DEOPT {
                jit_any_deopt_feedback(heap, &arm);
                if arm.deopt_watch {
                    jit_deopt_feedback(heap, &arm);
                }
            }
            // The shape check: a resolver that re-resolved through the IC may name a
            // different arm than the one whose native ran, and its journal cannot be read out
            // of this frame. Flag-free by construction (KI-26).
            if matches!(outcome, outcome::DEOPT | outcome::PREEMPT)
                && jit_frame_shape_matches(&arm, frame.nslots)
                && let Some((resume, rip, depth)) =
                    jit_ckpt_resume(heap, arm.arc(), frame.base, frame.nslots)
            {
                let r = jit_native_reenter(heap, frame.native_depth, |h| {
                    vm_resume_deopt(h, resume, frame.base, env, rip, depth)
                });
                return reenter_result(heap, r);
            }
            heap.truncate_roots(frame.stage_base);
            let r = jit_native_reenter(heap, frame.native_depth, |h| vm_apply(h, arm, &argv, env));
            reenter_result(heap, r)
        }
    }
}
