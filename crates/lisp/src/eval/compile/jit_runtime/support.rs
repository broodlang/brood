//! What native code asks of the runtime between calls: global resolution (with its
//! inline cache), the pending-error take-out, and the native stack headroom check that
//! bounds how deep native-to-native recursion may go before it falls back to the VM.

use super::*;

/// Are all the arm chunk's inlined primitives (1-, 2- and 3-ary) still bound to their native
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
        // The 1- and 3-ary inlines carry the same epoch guard on the VM, and native code
        // runs them without it — a user's own type predicate (`TypeIs`) is rebindable, so a
        // `(def p? …)` must take the arm off the native tier like a rebound `+` does.
        Inst::Prim1 { op, head, .. } => resolve_prim1(heap, *head) == Some(*op),
        Inst::Prim3 { op, head, .. } => resolve_prim3(heap, *head) == Some(*op),
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
/// `Inst::Global`/`GlobalIc` lowering (and a global read in value position). Reads the *live*
/// env each call, so a `def` rebind is seen immediately (the same late binding as
/// `Inst::Global`). The value, or the [`miss_status`] of the miss.
#[cfg(feature = "jit")]
#[inline]
pub(crate) fn jit_resolve_global(heap: &mut Heap, sym: Symbol) -> Result<Value, i64> {
    let env = heap.read_root_env(heap.jit_call_env);
    match heap.env_get(env, sym) {
        Some(v) => Ok(v),
        None => Err(miss_status(heap, sym, None)),
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
/// The value, or the [`miss_status`] of the miss.
#[cfg(feature = "jit")]
#[inline]
pub(crate) fn jit_resolve_global_ic(heap: &mut Heap, sym: Symbol, site: u32) -> Result<Value, i64> {
    let env = heap.read_root_env(heap.jit_call_env);
    if heap.is_global(env) {
        let epoch = heap.global_epoch();
        if let Some(v) = heap.vm_global_ic_probe(site, sym, epoch) {
            crate::perf_bump!(global_ic_hit);
            return Ok(v);
        }
        crate::perf_bump!(global_ic_miss);
        match heap
            .env_get(env, sym)
            .or_else(|| crate::eval::derive::root_escape_value(heap, sym))
        {
            Some(v) => {
                if !heap.is_dynamic(sym) {
                    heap.vm_global_ic_put(site, sym, epoch, v);
                }
                Ok(v)
            }
            None => Err(miss_status(heap, sym, Some(site))),
        }
    } else {
        match heap
            .env_get(env, sym)
            .or_else(|| crate::eval::derive::root_escape_value(heap, sym))
        {
            Some(v) => Ok(v),
            None => Err(miss_status(heap, sym, Some(site))),
        }
    }
}

/// What a native global read does about a miss (`brood_rt_global{,_ic}`'s status). A name
/// whose module a load would bind, or another process is loading, DEOPTs: a load here would
/// run arbitrary Brood — it collects, and a value read is no safepoint — and every wait in it
/// would block the OS worker, while the VM re-runs the read through its lazy path, where the
/// load runs as ordinary frames that park. Native code meets this only after a binding it was
/// compiled against has gone (an isolate rolling the load back); a fresh lazy name is bound
/// to its loading stub. Any other miss is the unbound error, parked with the site so the
/// arm's error block (`brood_rt_trace_push`) can locate it from `staged_head_pos`, as the
/// VM's miss path does.
#[cfg(feature = "jit")]
#[cold]
fn miss_status(heap: &mut Heap, sym: Symbol, site: Option<u32>) -> i64 {
    if crate::eval::derive::module_to_load_off_native(heap, sym).is_some()
        || crate::eval::derive::module_loading_elsewhere(heap, sym).is_some()
    {
        return crate::jit::rt::status::DEOPT;
    }
    let e = crate::eval::unbound_error(heap, sym);
    heap.jit_pending_error = Some(e);
    if let Some(site) = site {
        heap.jit_miss_gsite = site;
    }
    crate::jit::rt::status::ERROR
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
    // The stack pointer itself, not a local's address: `stacker::remaining_stack` measures
    // from the stack pointer, and a local may not be on the stack at all (ASan can move one
    // into a heap-allocated fake frame), which made the limit meaningless.
    let here = psm::stack_pointer() as usize;
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

/// The native prologue's slow path (`brood_rt_stack_recheck`): its one-compare guard tripped,
/// so measure. The stamp is an absolute address for ONE stack — taken where the native nest
/// began — and a frame running on another (a `stacker` segment a walker grew into, another
/// thread's stack) compares against a limit that means nothing there. Re-stamp for the stack
/// this frame is on, and answer whether it has room: true continues native, false deopts and
/// drains the subtree on the VM. Costs nothing on the path that does not trip.
#[cfg(feature = "jit")]
pub(crate) fn recheck_stack_limit(heap: &mut Heap) -> bool {
    stamp_stack_limit(heap);
    // `0` (no probe) fails open, as the prologue does; `usize::MAX` (inside the margin) and
    // any limit above this frame mean no room.
    heap.jit_stack_limit == 0 || psm::stack_pointer() as usize >= heap.jit_stack_limit
}

/// Stamp the limit only at the **outermost** native entry — `native_depth` is the caller's
/// pre-increment [`Heap::jit_native_depth`], so `0` means no native frame is on the stack
/// below this one.
///
/// [`Heap::jit_stack_limit`] is an absolute address derived from the running thread's stack
/// bottom, which does not move for the life of that thread. Re-deriving it at every
/// Brood→Brood link would recompute a constant, and `stacker::remaining_stack()` is not free
/// (~5% on the fast-link-heavy `bintree`). It does nothing for `fib`, which tiers to the i64
/// register worker and never takes a fast link; that worker's guard is in `jit_lower/i64.rs`.
///
/// The stamp is still *live* rather than a constant because a green process resumes on
/// whichever worker the scheduler routes it to, and worker stack bases differ — hence the
/// second stamp point at quantum start in `Process::drive`. That one is what makes this
/// gate safe if a quantum ever ends with the depth counter left raised: without it, a
/// process that migrated would keep comparing against the previous worker's stack.
#[cfg(feature = "jit")]
#[inline]
pub(super) fn stamp_stack_limit_if_outermost(heap: &mut Heap, native_depth: u32) {
    if native_depth == 0 {
        stamp_stack_limit(heap);
    }
}

/// Cap on native-to-native recursion (see [`Heap::jit_native_depth`]). Past this many
/// native levels, drain the rest of the subtree on the VM (heap frames, bounded by the call
/// stack's memory ceiling, ADR-751) so deep non-tail recursion keeps working instead of
/// overflowing the native stack.
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
pub(super) const JIT_HEADROOM_PROBE_FROM: u32 = 64;

/// Stack that must remain before another native link is allowed. Generous: it has to
/// cover the callee's native frame plus whatever Rust the callee re-enters (`apply_value`
/// on an outcome-4 tail chain, the deopt re-runs), and the cost of being wrong is an
/// unrecoverable abort while the cost of being early is a VM-drained subtree.
///
/// **It sits ABOVE the red zone where a nested activation grows the stack**
/// ([`crate::stack::NATIVE_RED_ZONE_BYTES`], `vm_apply`'s [`crate::stack::reenter`]). This
/// margin is what starts the drain: past it the slow call sets `jit_force_vm` and hands the
/// rest of the subtree to `vm_apply`, whose heap frames are where deep recursion belongs.
/// The gap is the drain's working room on this stack, so a drain begins with one
/// `vm_apply` in place rather than with a segment grown for it.
#[cfg(feature = "jit")]
/// Twice the red zone: 1 MiB, and the ASan build's larger red zone scales it with it.
pub(super) const JIT_STACK_MARGIN_BYTES: usize = 2 * crate::stack::NATIVE_RED_ZONE_BYTES;

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
pub(super) fn jit_native_reenter<T>(
    heap: &mut Heap,
    native_depth: u32,
    body: impl FnOnce(&mut Heap) -> T,
) -> T {
    heap.jit_native_depth = native_depth + 1;
    let out = body(heap);
    heap.jit_native_depth = native_depth;
    out
}

#[cfg(all(test, feature = "jit"))]
mod stack_recheck_tests {
    use super::*;

    /// A stamp from another stack — here, an address above this frame, which is what a frame
    /// on a lower `stacker` segment sees — trips the prologue with room to spare. The re-check
    /// must find the room and re-stamp for THIS stack, or the arm deopts on every call and
    /// latches off native (every JIT case in the ASan job, `native-depth-exceeded` ×16).
    #[test]
    fn a_limit_from_another_stack_is_restamped_and_does_not_deopt() {
        let mut interp = crate::Interp::new();
        let sp = psm::stack_pointer() as usize;
        interp.heap.jit_stack_limit = sp + 64 * 1024;
        assert!(
            recheck_stack_limit(&mut interp.heap),
            "a thread with megabytes of stack left was told it had none"
        );
        let limit = interp.heap.jit_stack_limit;
        assert!(
            limit != 0 && limit < sp,
            "the limit was not re-stamped below this frame: {limit:#x} vs sp {sp:#x}"
        );
    }

    /// And a stack that really is inside the margin still deopts: the re-check must not turn
    /// the guard off.
    #[test]
    fn a_stack_inside_the_margin_still_has_no_room() {
        let mut interp = crate::Interp::new();
        let left = stacker::remaining_stack().expect("this platform reports the stack");
        // Recurse on a fresh segment small enough that the margin is all there is.
        let room = stacker::grow(JIT_STACK_MARGIN_BYTES / 2, || {
            recheck_stack_limit(&mut interp.heap)
        });
        assert!(
            left > JIT_STACK_MARGIN_BYTES,
            "the test thread itself is short of stack"
        );
        assert!(
            !room,
            "a frame with less than the margin left was told to go on"
        );
    }
}
