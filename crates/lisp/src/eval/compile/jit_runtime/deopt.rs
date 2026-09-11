//! Leaving native code: frame shapes and their sizes, resuming the VM at a checkpoint
//! after a type deopt, and the feedback that latches an arm as `BAILED` when it thrashes
//! or suspends (`BROOD_DEOPT_TRACE`).

use super::*;

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
pub(super) enum FrameLayout {
    /// The original small body.
    Small,
    /// A leaf-callee-spliced derivation (ADR-210), whose slot range differs from the
    /// small one's.
    LeafSpliced,
}

#[cfg(feature = "jit")]
pub(super) fn jit_frame_layout(arm: &CompiledArm, frame_nslots: usize) -> FrameLayout {
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
pub(super) fn jit_suspend_feedback(heap: &mut Heap, arm: &CompiledArm, outcome: i64, gw_seq: u64) {
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
