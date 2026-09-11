//! What native code asks of the runtime between calls: global resolution (with its
//! inline cache), the pending-error take-out, and the native stack headroom check that
//! bounds how deep native-to-native recursion may go before it falls back to the VM.

use super::*;

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
pub(super) fn stamp_stack_limit_if_outermost(heap: &mut Heap, native_depth: u32) {
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
pub(super) const JIT_HEADROOM_PROBE_FROM: u32 = 64;

/// Stack that must remain before another native link is allowed. Generous: it has to
/// cover the callee's native frame plus whatever Rust the callee re-enters (`apply_value`
/// on an outcome-4 tail chain, the deopt re-runs), and the cost of being wrong is an
/// unrecoverable abort while the cost of being early is a VM-drained subtree.
#[cfg(feature = "jit")]
pub(super) const JIT_STACK_MARGIN_BYTES: usize = 512 * 1024;

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
