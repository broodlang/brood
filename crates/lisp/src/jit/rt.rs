//! The `brood_rt_*` runtime-callback table — **obligation 4** of the backend contract
//! ([`super::backend::JitBackend`]): the *only* legal way native code touches the heap,
//! the GC, or the scheduler.
//!
//! Backend-independent by construction. Nothing here knows what emitted the code that
//! calls it, so a second backend inherits this table unchanged — which is most of why the
//! JIT is swappable at all (`docs/backend-seams.md` §1). Every function is `extern "C"`
//! and `#[no_mangle]` so a backend can resolve it by name.
//!
//! ## The ABI (ADR-101 §6.2, adapted for the kept 24-byte enum `Value`)
//!
//! Brood keeps `Value` as a 24-byte enum — the measured decision in
//! [`docs/value-repr.md`](../../../docs/value-repr.md): a single-word `Value` gave
//! ~zero tier-1 speedup on the compute loops, so NaN-boxing isn't worth its
//! wide-scalar cost. Consequently a `Value` **never rides in a register**. Tier-1
//! JIT'd code keeps all live `Value`s in [`Heap::roots`] (the operand stack, the same
//! one the bytecode VM uses) and only holds *unboxed* `i64`/`f64`, extracted from a
//! root slot, in registers within a safepoint-free segment. So every runtime callback
//! takes the pinned `*mut Heap` context (r15/x28, ADR-101 §6.2) and operates on
//! `roots`/the heap — **not** `Value`-as-`u64` as the original ADR-101 sketch assumed
//! (that sketch presumed the NaN-box repr we declined).
//!
//! A safepoint can occur only inside one of these callbacks (allocation / explicit
//! safepoint / slow call), so between callbacks the JIT'd segment may keep unboxed
//! scalars in registers with no stack map — the single hardest part of JIT-ing under a
//! moving collector, sidestepped (ADR-101 §6.2). That is **obligation 5**, and it is the
//! reason this table is the whole heap interface rather than a convenience.
//!
//! ## The safety contract every callback shares
//!
//! Every `brood_rt_*` function is called only by native code a backend emitted under the
//! obligations in [`super::backend`], and the `// SAFETY:` comments below cite these facts
//! by name rather than restating them:
//!
//! - **heap** — `heap` is the pinned context pointer of the activation: the running
//!   process's own live `Heap`, used by this thread alone for the whole call, with no other
//!   Rust reference to it live across the callback.
//! - **out** — an `out` (or `out_len` / `out_flag`) pointer is a writable, aligned slot the
//!   emitting arm owns (its stack slot or a `roots` slot), large enough for the type written.
//! - **words** — every word triple is the three words of a real `Value`, read by native code
//!   out of a frame slot, a constant or a previous callback's `out`, so
//!   [`Value::from_raw_words`](crate::core::value::Value::from_raw_words) rebuilds a valid
//!   enum.
//! - **ctx** — `ctx` points at the `JitCallCtx` the running arm was entered with, which its
//!   caller keeps alive across the call.
//! - **no collection** — a callback that is not a safepoint allocates only by growing a slab
//!   (`alloc_slot!`), never collects, so the handles it was handed stay valid for the call.

use crate::core::heap::{FastLink, Heap};

// Debug builds trace each callback under `BROOD_JIT_CB_TRACE=1`
// (`debug_flags::jit_callback_trace`).

/// Preemption poll (ADR-027). JIT'd loop back-edges call this; returns nonzero when the
/// process should yield. Mirrors the VM's loop-top exactly: only a **capture-mode green
/// process** is preemptible (`tick_capture` decrements the reduction budget and yields at
/// zero); the root/eval thread (and any non-capture run) never preempts — it just keeps
/// going, like the VM's `tick()` else-branch. Gating here is load-bearing: without it a
/// JIT'd loop on the root thread would yield on its first iteration and bail to the VM,
/// so the JIT could never actually run the loop.
/// A native activation's reduction poll — a loop's batched back edge (one call per batch of
/// iterations, an in-register countdown between) or a call's entry poll. `1` = preempt by
/// unwinding; `2` = leave for the VM: a limit is waiting to be RAISED — the soft memory
/// ceiling (E0043), a per-process heap limit, a mailbox overflow, a hard kill — and native
/// code cannot raise, so it deopts and the VM's own safepoint raises it (KI-156). Whether
/// a spent budget preempts is `quantum::native_poll`'s decision (ADR-401/403).
///
/// # Safety
/// `heap` is a live heap; the limit peeks read it only. `ctx` is the running activation's
/// context, which its caller keeps alive across the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_tick_n(
    heap: *mut Heap,
    n: i64,
    ctx: *const crate::jit::JitCallCtx,
) -> u8 {
    // SAFETY: `heap` is the live process heap and is only read here (contract: heap); `ctx`
    // is the running activation's context (contract: ctx).
    unsafe {
        match crate::core::alloc::soft_limit_state() {
            crate::core::alloc::SoftLimit::Clear => {}
            // A native loop near the limit measures itself like an interpreted one, or the
            // choice of whom to stop at the crossing never sees it (KI-229).
            crate::core::alloc::SoftLimit::Near => crate::process::near_soft_limit(&*heap),
            crate::core::alloc::SoftLimit::Over(_) => return 2,
        }
        if (*heap).proc_limit_pending() || crate::process::current_mailbox_overflow_pending() {
            return 2;
        }
        use crate::process::NativePoll;
        match crate::process::native_poll(n as u32, (*ctx).unwind != 0) {
            NativePoll::Run => 0,
            NativePoll::Preempt => 1,
            NativePoll::Leave => 2,
            NativePoll::RunOnFreshQuantum => {
                // The sampling profiler records at interpreter frame boundaries, and this loop
                // will not reach one until it finishes. Sample it here instead, once per
                // profiler tick, by the running arm's name.
                if crate::diagnostics::profile::armed() {
                    sample_native_activation(&*ctx);
                }
                // The once-per-quantum checks a native loop does not reach otherwise.
                crate::core::alloc::refresh_container_limits();
                0
            }
        }
    }
}

/// One profiler sample for a native activation that cannot yield to a driver: the stack is
/// just its own name (the frames below it are a builtin's or a nested driver's), recorded at
/// most once per profiler tick on this thread, as a driver records once per tick.
#[cold]
fn sample_native_activation(ctx: &crate::jit::JitCallCtx) {
    thread_local! {
        static LAST: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    }
    let epoch = crate::diagnostics::profile::epoch();
    if LAST.with(|l| l.replace(epoch)) == epoch {
        return;
    }
    if ctx.dbg_fn != u32::MAX {
        crate::diagnostics::profile::record(&[ctx.dbg_fn]);
    }
}

/// GC safepoint check. JIT'd code calls this where the interpreter would collect (a
/// loop top / before an allocation burst): collect if due and not inside the compile
/// pass (mirrors the eval safepoint, ADR-061).
///
/// # Safety
/// `heap` must be the live, pinned context pointer for the current JIT'd call, with no
/// live `Value`s outside `Heap::roots` (the no-stack-map invariant, ADR-101 §6.2).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_gc_safepoint(heap: *mut Heap) {
    // SAFETY: `heap` is the live, exclusively used process heap (contract: heap), and the
    // caller holds every live `Value` in `roots`, so collecting here leaves nothing stale.
    unsafe {
        #[cfg(debug_assertions)]
        if crate::diagnostics::debug_flags::jit_callback_trace() {
            eprintln!("[jit-cb] brood_rt_gc_safepoint()");
        }
        let h = &mut *heap;
        if !crate::process::macro_block_active() && h.gc_due() {
            h.collect(&mut [], &mut []);
        }
    }
}

// ---- The handle ops: cons / car / cdr, by-value with an out-pointer. ----
//
// A `Value` is 24 bytes (3 i64 words: tag at 0, payload words at 8 and 16 — the layout
// the JIT reads/writes a roots slot through), so it can't be a C register-pair return.
// Instead the JIT passes an `out: *mut Value` (a stack slot it owns) and the callback
// writes the result there; the JIT reads the three words back into an `Op::Handle`. The
// operands likewise arrive as word triples the JIT read out of real `Value`s (a slot, an
// `Int` box, or a previous handle result), so `words_to_val` is the identity on their
// bytes. `alloc_pair` only grows the nursery (never collects), so a reconstructed operand
// can't go stale mid-`cons`; no `roots` is touched, so `roots_base` stays valid.
#[inline]
unsafe fn words_to_val(w0: i64, w1: i64, w2: i64) -> crate::core::value::Value {
    // SAFETY: every caller passes the words of a real `Value` (contract: words), which is
    // exactly what `from_raw_words` requires.
    unsafe { crate::core::value::Value::from_raw_words([w0, w1, w2]) }
}

/// The status a value-producing callback returns to native code — one protocol for all of
/// them, so a lowering site reads the same number the same way whichever callback it called.
/// (`brood_rt_equal` answers a boolean in its status and the tick/link callbacks return
/// control signals; those are different contracts, documented where they live.)
pub(crate) mod status {
    /// `*out` holds the answer.
    pub(crate) const DONE: i64 = 0;
    /// Declined: an operand outside what the callback handles natively. The arm deopts and
    /// the VM computes the exact result (or the exact error).
    pub(crate) const DEOPT: i64 = 1;
    /// A real error is parked in `jit_pending_error`; the arm exits via its error block.
    pub(crate) const ERROR: i64 = 2;
}

/// The answering half of the [`status`] protocol: `Ok` writes `*out` and returns DONE; `Err`
/// parks the error and returns ERROR. (DEOPT is each caller's own early return.)
///
/// # Safety
/// `out` must be a writable `*mut Value`.
#[inline]
unsafe fn answer_or_park(
    heap: &mut Heap,
    out: *mut crate::core::value::Value,
    result: crate::error::LispResult,
) -> i64 {
    // SAFETY: the caller passes a writable `Value` slot as `out` (this fn's `# Safety`);
    // `Value` is `Copy`, so the store drops nothing that was there.
    unsafe {
        match result {
            Ok(value) => {
                *out = value;
                status::DONE
            }
            Err(error) => {
                heap.jit_pending_error = Some(error);
                status::ERROR
            }
        }
    }
}

/// `first` / `rest` / `empty?` of a value the inline fast path declined: the builtin's
/// non-evaluating form answers, or `None` (a lazy seq-view, which needs the evaluator) is a
/// DEOPT.
///
/// # Safety
/// `out` must be a writable `*mut Value`; the word triple is bytes read out of a real `Value`.
unsafe fn unary_fallback(
    heap: *mut Heap,
    out: *mut crate::core::value::Value,
    words: [i64; 3],
    without_eval: fn(&mut Heap, crate::core::value::Value) -> Option<crate::error::LispResult>,
) -> i64 {
    // SAFETY: the callers forward their own contract: `heap` live (contract: heap), `out`
    // writable (contract: out), `words` from a real `Value` (contract: words).
    unsafe {
        let h = &mut *heap;
        match without_eval(h, words_to_val(words[0], words[1], words[2])) {
            None => status::DEOPT,
            Some(result) => answer_or_park(h, out, result),
        }
    }
}

/// `cons` two `Value`s (each by word-triple), writing the fresh pair to `*out`.
///
/// # Safety
/// `heap`/`out` live; the word triples are bytes the JIT read out of real `Value`s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_cons(
    heap: *mut Heap,
    out: *mut crate::core::value::Value,
    c0: i64,
    c1: i64,
    c2: i64,
    d0: i64,
    d1: i64,
    d2: i64,
) {
    // SAFETY: contracts heap, out and words; `alloc_pair` never collects (contract: no
    // collection), so the rebuilt operands cannot go stale before they are stored.
    unsafe {
        let h = &mut *heap;
        let car = words_to_val(c0, c1, c2);
        let cdr = words_to_val(d0, d1, d2);
        *out = h.alloc_pair(car, cdr);
    }
}

/// Allocate a 2-element vector and hand back a pointer to its element storage, for the
/// emitting arm to write the two elements into directly. Writes the fresh handle to `*out`.
///
/// Replaces passing the elements as six `i64` words: four of
/// them spilled to the caller's stack (SysV has six argument registers and `heap`/`out` take
/// two), and reloading them was **66% of that function**. Two arguments now, both in
/// registers, and the arm's stores land in the slab.
///
/// # Safety
/// `heap` live; `out` writable. The caller must write both elements before anything can
/// allocate or collect — see [`Heap::alloc_vector2_room`], whose doc carries the invariant.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_vec2_room(
    heap: *mut Heap,
    out: *mut crate::core::value::Value,
) -> *mut crate::core::value::Value {
    // SAFETY: contracts heap and out. The returned element pointer is only sound if the
    // emitting arm fills both elements before anything allocates (`alloc_vector2_room`'s doc).
    unsafe {
        let (handle, items) = (*heap).alloc_vector2_room();
        *out = handle;
        items
    }
}

/// Build an `n`-element vector from `n` `Value`s staged contiguously at `elems`
/// (the JIT wrote each element's 3 words into a stack slot it owns), writing the
/// fresh vector to `*out`. The variadic generalisation of [`brood_rt_vec2_room`]
/// for a wider `[a b c …]` literal (`Inst::MakeVector(n)`, `n != 2`) — nbody's
/// `[vx vy vz]` / 7-body rebuild. A fixed Cranelift signature can't take `n×3`
/// words, so the elements come by pointer instead of by register-triple.
///
/// Like the arity-2 path, `alloc_vector` only *grows* the LOCAL vector slab (an
/// `alloc_slot!` push — never collects), so the staged elements can't go stale
/// during the call and need no extra rooting beyond the bytes at `elems`.
///
/// # Safety
/// `heap`/`out` live; `elems` points at `n` consecutive, fully-initialised `Value`s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_make_vector_n(
    heap: *mut Heap,
    out: *mut crate::core::value::Value,
    elems: *const crate::core::value::Value,
    n: i64,
) {
    // SAFETY: contracts heap and out; for `n > 0`, `elems` points at `n` initialised, aligned
    // `Value`s in the arm's stack slot (this fn's `# Safety`), and `alloc_vector_from_slice`
    // never collects, so those values stay valid while they are copied.
    unsafe {
        let h = &mut *heap;
        // Read in place: a one-element literal is stored inline with no `Vec`, a longer
        // one is copied once, into its spill buffer.
        // An empty literal may stage nothing, so its pointer is never read.
        let items: &[crate::core::value::Value] = if n > 0 {
            std::slice::from_raw_parts(elems, n as usize)
        } else {
            &[]
        };
        *out = h.alloc_vector_from_slice(items);
    }
}

/// Build a map from `npairs` key/value `Value` pairs staged contiguously at `elems`
/// (key `i` at `2i`, value at `2i+1` — the JIT wrote each as 3 words into a stack slot it
/// owns), writing the fresh map to `*out`. The `{k v …}` literal's twin of
/// [`brood_rt_make_vector_n`] (`Inst::MakeMap(npairs)`). `map_from_pairs` is the VM's own
/// constructor — last-wins de-dup, a GC-quiet in-place CHAMP build — so the two engines
/// build the identical map; it allocates and never collects, so the staged bytes stay live.
///
/// # Safety
/// `heap`/`out` live; `elems` points at `2·npairs` consecutive, fully-initialised `Value`s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_make_map_n(
    heap: *mut Heap,
    out: *mut crate::core::value::Value,
    elems: *const crate::core::value::Value,
    npairs: i64,
) {
    // SAFETY: contracts heap and out; for `npairs > 0`, `elems` points at `2 * npairs`
    // initialised, aligned `Value`s (this fn's `# Safety`); the map build never collects.
    unsafe {
        let h = &mut *heap;
        // The pairs are read straight out of the staged buffer (the JIT's stack slot, not
        // the heap), so no throwaway `Vec` of them is built.
        let staged: &[crate::core::value::Value] = if npairs > 0 {
            std::slice::from_raw_parts(elems, 2 * npairs as usize)
        } else {
            &[]
        };
        *out = h.map_from_pair_iter(
            staged
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&[key, value]| (key, value)),
        );
    }
}

/// `first` of a `Value` (by word-triple), writing its car to `*out`. The JIT **tag-checks
/// for `Pair` and deopts before calling**, so a non-pair (impossible by that contract)
/// yields `nil` rather than UB.
///
/// # Safety
/// `heap`/`out` live; the word triple is a real `Value::Pair`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_car(
    heap: *mut Heap,
    out: *mut crate::core::value::Value,
    w0: i64,
    w1: i64,
    w2: i64,
) {
    // SAFETY: contracts heap, out and words; a non-pair operand yields `nil`, so the handle
    // passed to `pair` is always a real pair id.
    unsafe {
        let h = &mut *heap;
        *out = match words_to_val(w0, w1, w2) {
            crate::core::value::Value::Pair(id) => h.pair(id).0,
            _ => crate::core::value::Value::Nil,
        };
    }
}

/// `first` of a non-pair, for the JIT's inline `first` whose tag check failed — the
/// **fallback instead of a deopt**: deopting for every non-`Pair` would deopt an arm reading
/// `first` of a vector per activation and latch it off the native tier. Same status
/// protocol as [`brood_rt_vector_ref`]: 0 = `*out` holds the answer, 1 = deopt (a
/// record's `Seqable` dispatch or a seq-view's realisation, which need the evaluator),
/// 2 = the builtin's own type error, parked.
///
/// # Safety
/// `heap`/`out` live; the word triple is bytes the JIT read out of a real `Value`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_first(
    heap: *mut Heap,
    out: *mut crate::core::value::Value,
    w0: i64,
    w1: i64,
    w2: i64,
) -> i64 {
    // SAFETY: `unary_fallback`'s requirements are this callback's own: contracts heap, out
    // and words.
    unsafe { unary_fallback(heap, out, [w0, w1, w2], crate::builtins::first_without_eval) }
}

/// `rest` of a non-pair — see [`brood_rt_first`].
///
/// # Safety
/// As [`brood_rt_first`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_rest(
    heap: *mut Heap,
    out: *mut crate::core::value::Value,
    w0: i64,
    w1: i64,
    w2: i64,
) -> i64 {
    // SAFETY: `unary_fallback`'s requirements are this callback's own: contracts heap, out
    // and words.
    unsafe { unary_fallback(heap, out, [w0, w1, w2], crate::builtins::rest_without_eval) }
}

/// `empty?` of a non-nil, non-pair — the inline `empty?`'s tag-check miss, as a
/// **fallback instead of a deopt** (the [`brood_rt_first`] protocol: 0 = `*out` holds the
/// answer, 1 = deopt, 2 = error parked). Deopting instead would send every prelude loop of
/// the shape `(cond (empty? coll) … (first coll) … (rest coll))` handed a vector, string,
/// set or bytes to the VM at entry — and a `SelfCall` arm is never deopt-watched, so
/// nothing would ever latch it.
///
/// # Safety
/// `heap`/`out` live; the word triple is bytes the JIT read out of a real `Value`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_is_empty(
    heap: *mut Heap,
    out: *mut crate::core::value::Value,
    w0: i64,
    w1: i64,
    w2: i64,
) -> i64 {
    // SAFETY: `unary_fallback`'s requirements are this callback's own: contracts heap, out
    // and words.
    unsafe {
        unary_fallback(
            heap,
            out,
            [w0, w1, w2],
            crate::builtins::is_empty_without_eval,
        )
    }
}

/// The stack guard's slow path: the prologue's compare against `Heap::jit_stack_limit`
/// tripped. Re-measure and re-stamp for the stack this frame is actually on; 1 = room,
/// continue native, 0 = deopt (the VM drains deep recursion). See `recheck_stack_limit`.
///
/// # Safety
/// `heap` must be the live context pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_stack_recheck(heap: *mut Heap) -> i8 {
    // SAFETY: `heap` is the live process heap (contract: heap), borrowed for this call only.
    let heap = unsafe { &mut *heap };
    crate::eval::compile::recheck_stack_limit(heap) as i8
}

/// Byte pointer to the LOCAL nursery pair slab (`Vec<(Value, Value)>`). Called once at JIT
/// function entry so inline `first`/`rest` can compute `base + idx * 48 + {0,24}` directly
/// instead of calling `brood_rt_car`/`cdr` per element. Valid only while no `cons` can grow
/// the slab (arms that allocate must not use the stashed pointer — see `jit_lower_arm`).
///
/// # Safety
/// `heap` must be the live context pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_pair_nursery_base(heap: *mut Heap) -> *const u8 {
    // SAFETY: `heap` is the live process heap (contract: heap); the read only takes an address.
    unsafe { (*heap).local_pair_nursery_base() }
}

/// Byte pointer to the LOCAL old-generation pair slab. Companion to
/// [`brood_rt_pair_nursery_base`] — for pairs promoted out of the nursery.
///
/// # Safety
/// `heap` must be the live context pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_pair_old_base(heap: *mut Heap) -> *const u8 {
    // SAFETY: `heap` is the live process heap (contract: heap); the read only takes an address.
    unsafe { (*heap).local_pair_old_base() }
}

/// Byte pointer to the LOCAL nursery **vector** slab, so a JIT arm can inline a
/// small-vector element read instead of calling [`brood_rt_vector_ref`]. The
/// vector analog of [`brood_rt_pair_nursery_base`].
///
/// # Safety
/// `heap` must be the live context pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_vec_nursery_base(heap: *mut Heap) -> *const u8 {
    // SAFETY: `heap` is the live process heap (contract: heap); the read only takes an address.
    unsafe { (*heap).local_vec_nursery_base() }
}

/// Byte pointer to the LOCAL old-generation vector slab. Companion to
/// [`brood_rt_vec_nursery_base`].
///
/// # Safety
/// `heap` must be the live context pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_vec_old_base(heap: *mut Heap) -> *const u8 {
    // SAFETY: `heap` is the live process heap (contract: heap); the read only takes an address.
    unsafe { (*heap).local_vec_old_base() }
}

/// `rest` counterpart of [`brood_rt_car`] — writes the pair's cdr to `*out`.
///
/// # Safety
/// `heap`/`out` live; the word triple is a real `Value::Pair`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_cdr(
    heap: *mut Heap,
    out: *mut crate::core::value::Value,
    w0: i64,
    w1: i64,
    w2: i64,
) {
    // SAFETY: contracts heap, out and words; a non-pair operand yields `nil`, so the handle
    // passed to `pair` is always a real pair id.
    unsafe {
        let h = &mut *heap;
        *out = match words_to_val(w0, w1, w2) {
            crate::core::value::Value::Pair(id) => h.pair(id).1,
            _ => crate::core::value::Value::Nil,
        };
    }
}

/// `vector-ref` / `nth` of a dense vector by an `Int` index, writing the element to
/// `*out` and returning `0`; returns `1` (deopt to the VM) for a non-vector receiver, a
/// non-`Int` index, or an out-of-range index — the VM then applies the exact semantics
/// (`vector-ref`'s bounds error, or `nth`'s `default`). Reads the slab only; never
/// allocates, so it is not a safepoint (a `Handle` produced here is consumed before any
/// collection).
///
/// # Safety
/// `heap`/`out` live; the word triples are real `Value`s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_vector_ref(
    heap: *mut Heap,
    out: *mut crate::core::value::Value,
    v0: i64,
    v1: i64,
    v2: i64,
    i0: i64,
    i1: i64,
    i2: i64,
) -> i64 {
    // SAFETY: contracts heap, out and words; the index is bounds-checked against the vector's
    // length before the element is read.
    unsafe {
        use crate::core::value::Value;
        let h = &mut *heap;
        let (vid, idx) = match (words_to_val(v0, v1, v2), words_to_val(i0, i1, i2)) {
            (Value::Vector(id), Value::Int(n)) => (id, n),
            _ => return 1,
        };
        let v = h.vector(vid);
        if idx < 0 || idx as usize >= v.len() {
            return 1;
        }
        *out = v[idx as usize];
        // A RUNTIME vector: note its bucket, so the next read of one beside it is inline.
        h.note_shared_vector(vid);
        0
    }
}

/// `(%vector-length v)` from JIT'd code (the `PrimOp1::VectorLen` lowering): the
/// vector's length as a plain `i64`, or **-1** for a non-vector — the deopt signal, so the
/// VM dispatches the native for its exact type error. Reads the slab only; never
/// allocates, so it is not a safepoint.
///
/// # Safety
/// `heap` live; the word triple is a real `Value`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_vector_len(heap: *mut Heap, v0: i64, v1: i64, v2: i64) -> i64 {
    // SAFETY: contracts heap and words; the heap is only read.
    unsafe {
        use crate::core::value::Value;
        match words_to_val(v0, v1, v2) {
            Value::Vector(id) => (*heap).vector(id).len() as i64,
            _ => -1,
        }
    }
}

/// `(count v)` from JIT'd code (the `PrimOp1::Count` lowering): `Heap::count_inline`'s
/// answer as a plain `i64`, or **-1** for a shape the prelude `count` decides in Brood (a
/// record, seq-view, rope, table, improper list, `Seqable` impl, type error) — the deopt
/// signal. Reads the heap only; never allocates, so it is not a safepoint.
///
/// # Safety
/// `heap` live; the word triple is a real `Value`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_count(heap: *mut Heap, v0: i64, v1: i64, v2: i64) -> i64 {
    // SAFETY: contracts heap and words; the heap is only read.
    unsafe { (*heap).count_inline(words_to_val(v0, v1, v2)).unwrap_or(-1) }
}

/// `(%table-has? t k)` from JIT'd code (the `PrimOp::TableHas` lowering). Returns
/// 0 = done (`*out` holds the boolean), 1 = deopt (first operand isn't a Table —
/// the VM owns the exact type error), 2 = a real error was parked in
/// `jit_pending_error` (dropped table / invalid key) — the arm exits via its
/// error block (outcome 3), bit-identical to the native raising it.
///
/// # Safety
/// `heap` must be the live context pointer and `out` a writable `*mut Value`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_table_has(
    heap: *mut Heap,
    out: *mut crate::core::value::Value,
    t0: i64,
    t1: i64,
    t2: i64,
    k0: i64,
    k1: i64,
    k2: i64,
) -> i64 {
    // SAFETY: contracts heap, out and words; the table lookup never collects.
    unsafe {
        use crate::core::value::Value;
        let h = &mut *heap;
        let Value::Table(id) = words_to_val(t0, t1, t2) else {
            return 1;
        };
        let key = words_to_val(k0, k1, k2);
        let result = crate::core::table::check_key("table/contains?", key)
            .and_then(|()| crate::core::table::has(h, id, key).map(Value::Bool));
        answer_or_park(h, out, result)
    }
}

/// 2-arg `(%table-get t k)` from JIT'd code (the `PrimOp::TableGet` lowering) — nil
/// default. Same status protocol as [`brood_rt_table_has`]. The returned value is a
/// fresh reconstruction in the caller's heap; reconstruction may allocate (a compound
/// stored value) but **never collects** (`alloc_slot!` is a plain push; collection
/// runs only at safepoints), so handles the JIT'd arm holds in registers across this
/// call stay valid — same discipline as `brood_rt_cons`/`brood_rt_make_vector_n`.
///
/// # Safety
/// `heap` must be the live context pointer and `out` a writable `*mut Value`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_table_get2(
    heap: *mut Heap,
    out: *mut crate::core::value::Value,
    t0: i64,
    t1: i64,
    t2: i64,
    k0: i64,
    k1: i64,
    k2: i64,
) -> i64 {
    // SAFETY: contracts heap, out and words; rebuilding the stored value allocates but never
    // collects (contract: no collection).
    unsafe {
        use crate::core::value::Value;
        let h = &mut *heap;
        let Value::Table(id) = words_to_val(t0, t1, t2) else {
            return 1;
        };
        let key = words_to_val(k0, k1, k2);
        let result = crate::core::table::check_key("table/get", key)
            .and_then(|()| crate::core::table::get(h, id, key, Value::Nil));
        answer_or_park(h, out, result)
    }
}

/// `(%table-put t k v)` from JIT'd code (the `PrimOp3::TablePut` lowering). Same
/// status protocol as [`brood_rt_table_has`]; on success `*out` holds the table
/// handle (put returns the table, for threading). Storing deep-clones the key and
/// value out of the GC heap (`to_message`) — allocation-free on the dense int path
/// and never collecting on any path, so register-held handles stay valid.
///
/// # Safety
/// `heap` must be the live context pointer and `out` a writable `*mut Value`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_table_put(
    heap: *mut Heap,
    out: *mut crate::core::value::Value,
    t0: i64,
    t1: i64,
    t2: i64,
    k0: i64,
    k1: i64,
    k2: i64,
    v0: i64,
    v1: i64,
    v2: i64,
) -> i64 {
    // SAFETY: contracts heap, out and words; storing deep-clones out of the heap and never
    // collects (contract: no collection).
    unsafe {
        use crate::core::value::Value;
        let h = &mut *heap;
        let Value::Table(id) = words_to_val(t0, t1, t2) else {
            return 1;
        };
        let key = words_to_val(k0, k1, k2);
        let val = words_to_val(v0, v1, v2);
        let result = crate::core::table::check_key("table/put", key)
            .and_then(|()| crate::core::table::put(h, id, key, val));
        answer_or_park(h, out, result)
    }
}

/// Loop-invariant-hoist support for the JIT (matmul LICM): resolve a vector value's
/// inner element storage to a raw `(data_ptr, len)` **once**, so the JIT can inline
/// `ptr + idx * size_of::<Value>()` element reads for the rest of a loop instead of
/// calling [`brood_rt_vector_ref`] per element (marshal 6 words + slab lookup + a
/// 24-byte out-pointer copy, every iteration). Returns the element data pointer and
/// writes the element count to `*out_len`; returns null (and `*out_len = 0`) if the
/// value isn't a vector, in which case the JIT deopts (the VM owns the exact result).
///
/// Sound only because the JIT gates this to arms that neither allocate nor make a
/// Brood→Brood call — so no LOCAL GC and no RUNTIME compaction can run mid-arm to
/// relocate the storage — and Brood vectors are **immutable** (no write can ever
/// invalidate a hoisted read, so the LICM needs no alias analysis). The pointer is
/// valid only for the duration of the native arm run; a preempt/deopt re-enters from
/// the arm's entry block, which re-resolves it from the current frame.
///
/// # Safety
/// `heap` must be the live context pointer; `out_len` a writable `*mut i64`; the word
/// triple is bytes the JIT read out of a real `Value` (an invariant frame slot).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_vector_base(
    heap: *mut Heap,
    w0: i64,
    w1: i64,
    w2: i64,
    out_len: *mut i64,
) -> *const u8 {
    // SAFETY: contracts heap, words and out (`out_len`). The returned pointer outlives this
    // borrow of the heap; it stays valid only because the JIT hoists it solely in arms that
    // neither allocate nor call (see the doc above), so nothing can move the storage.
    unsafe {
        use crate::core::value::Value;
        let h = &mut *heap;
        match words_to_val(w0, w1, w2) {
            Value::Vector(id) => {
                let v = h.vector(id);
                *out_len = v.len() as i64;
                v.as_ptr() as *const u8
            }
            _ => {
                *out_len = 0;
                std::ptr::null()
            }
        }
    }
}

/// Resolve a table value's **dense slot region** for the JIT's table hoist
/// (the sieve lever — see `Op::HoistedTable` in `jit_lower`): the raw slots
/// base, with the store's `dense`-flag address written to `*out_flag`. Returns
/// null for a non-table / dropped / already-hashed store, in which case the
/// per-op FFI path is used. The region is process-lifetime and never moves
/// (stable across GC and compaction — it is not a heap object), so the baked
/// pointers cannot dangle; the per-op flag re-check routes to the FFI when the
/// table migrates or drops. See `table::jit_dense_base`.
///
/// # Safety
/// Called from JIT'd code with the live heap pointer and a valid out param.
pub unsafe extern "C" fn brood_rt_table_dense_base(
    _heap: *mut Heap,
    w0: i64,
    w1: i64,
    w2: i64,
    out_flag: *mut i64,
) -> *const u8 {
    // SAFETY: contracts words and out (`out_flag`); the heap is not touched. The returned
    // region is process-lifetime and never moves (`table::jit_dense_base`).
    unsafe {
        use crate::core::value::Value;
        if let Value::Table(id) = words_to_val(w0, w1, w2)
            && let Some((slots, flag)) = crate::core::table::jit_dense_base(id)
        {
            *out_flag = flag as i64;
            return slots as *const u8;
        }
        *out_flag = 0;
        std::ptr::null()
    }
}

/// `(nth v i default)` on a vector — the native half of `PrimOp3::VectorRefOr`. Status 0 =
/// the element (or the default, out of range) is in `*out`; 1 = decline (not a vector, or
/// not a fixnum index), which the prelude `nth` owns. The rule is `Heap::nth3_inline`,
/// shared with the VM's `Inst::Prim3` arm. Pure: allocates nothing.
///
/// # Safety
/// `heap`/`out` live; the word triples are bytes the JIT read out of real `Value`s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_vector_ref3(
    heap: *mut Heap,
    out: *mut crate::core::value::Value,
    v0: i64,
    v1: i64,
    v2: i64,
    i0: i64,
    i1: i64,
    i2: i64,
    d0: i64,
    d1: i64,
    d2: i64,
) -> i64 {
    // SAFETY: contracts heap, out and words; the heap is only read.
    unsafe {
        let h = &*heap;
        match h.nth3_inline(
            words_to_val(v0, v1, v2),
            words_to_val(i0, i1, i2),
            words_to_val(d0, d1, d2),
        ) {
            Some(v) => {
                *out = v;
                0
            }
            None => 1,
        }
    }
}

/// `(get m k default)` on a CHAMP map — the native half of `PrimOp3::MapGet3` (ADR-368).
/// Status 0 = the answer is in `*out`; 1 = decline (a non-map receiver, or a record's miss,
/// which `get`'s `%lookup-miss` owns). The rule is `Heap::map_get3_inline`, shared
/// with the VM's `Inst::Prim3` arm. Pure: a CHAMP probe allocates nothing.
///
/// # Safety
/// `heap`/`out` live; the word triples are bytes the JIT read out of real `Value`s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_map_get3(
    heap: *mut Heap,
    out: *mut crate::core::value::Value,
    m0: i64,
    m1: i64,
    m2: i64,
    k0: i64,
    k1: i64,
    k2: i64,
    d0: i64,
    d1: i64,
    d2: i64,
) -> i64 {
    // SAFETY: contracts heap, out and words; a CHAMP probe allocates nothing.
    unsafe {
        let h = &mut *heap;
        match h.map_get3_inline(
            words_to_val(m0, m1, m2),
            words_to_val(k0, k1, k2),
            words_to_val(d0, d1, d2),
        ) {
            Some(v) => {
                *out = v;
                0
            }
            None => 1,
        }
    }
}

/// `(assoc m k v)` on a CHAMP map — the native half of `PrimOp3::MapAssoc` (ADR-368).
/// Status 0 = the fresh map is in `*out`; 1 = decline (a non-map receiver — the wrapper's
/// `%vector-assoc` branch and its errors stay in Brood). `Heap::map_assoc` path-copies the
/// touched nodes: it ALLOCATES (the planner's `inst_may_allocate`/`inst_allocates_hot` say
/// so) but, like every allocation reached from native code, never collects, so the
/// caller's register-held handles stay valid.
///
/// # Safety
/// `heap`/`out` live; the word triples are bytes the JIT read out of real `Value`s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_map_assoc(
    heap: *mut Heap,
    out: *mut crate::core::value::Value,
    m0: i64,
    m1: i64,
    m2: i64,
    k0: i64,
    k1: i64,
    k2: i64,
    v0: i64,
    v1: i64,
    v2: i64,
) -> i64 {
    // SAFETY: contracts heap, out and words; the path copy allocates but never collects
    // (contract: no collection), so the operands stay valid.
    unsafe {
        let h = &mut *heap;
        match h.map_assoc_inline(
            words_to_val(m0, m1, m2),
            words_to_val(k0, k1, k2),
            words_to_val(v0, v1, v2),
        ) {
            Some(v) => {
                *out = v;
                0
            }
            None => 1,
        }
    }
}

/// `(get m k)` on a CHAMP map — the native half of [`PrimOp::MapGet`].
///
/// Status protocol, matching [`brood_rt_table_get2`]: **0** = the answer is in `*out`;
/// **1** = decline. Never 2 — a map probe raises nothing, so there is no error to park.
///
/// The rule is `Heap::map_get_inline`, shared with the VM's `prim2_inline_exec`: a hit, or
/// a plain map's miss (`nil`), is answered; a non-map receiver or a **record's** miss
/// declines, so `get`'s `%lookup-miss` resolves the latter through the `Lookup` ability.
/// Keeping that in Brood is the point — this is a fast path, not a second `get`.
///
/// # Safety
/// `heap`/`out` live; the word triples are bytes the JIT read out of real `Value`s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_map_get(
    heap: *mut Heap,
    out: *mut crate::core::value::Value,
    m0: i64,
    m1: i64,
    m2: i64,
    k0: i64,
    k1: i64,
    k2: i64,
) -> i64 {
    // SAFETY: contracts heap, out and words; a CHAMP probe allocates nothing.
    unsafe {
        let h = &mut *heap;
        match h.map_get_inline(words_to_val(m0, m1, m2), words_to_val(k0, k1, k2)) {
            Some(v) => {
                *out = v;
                0
            }
            None => 1,
        }
    }
}

/// Structural `=` on two operands native code could not decide inline — the residual case
/// of `eq_dispatch` (KI-132). The lowering compares Int×Int and Sym/Keyword identity in
/// machine ops; every other pair (strings, floats, structural values) lands here rather
/// than deopting, which would latch an arm comparing strings off the native tier. This is exactly `%eq` (`prim_eq`): `Heap::equal` for anything
/// but a lazy seq-view, which the kernel cannot realise without an evaluator.
///
/// Returns **1** equal, **0** not equal, **2** declined (a `SeqView` operand — the caller
/// deopts, and the VM's `%eq` realises the view). Allocation-free and cannot error, so no
/// `out` slot and no parked error.
///
/// # Safety
/// `heap` lives; the word triples are bytes the JIT read out of real `Value`s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_equal(
    heap: *mut Heap,
    a0: i64,
    a1: i64,
    a2: i64,
    b0: i64,
    b1: i64,
    b2: i64,
) -> i64 {
    // SAFETY: contracts heap and words; the heap is only read.
    unsafe {
        use crate::core::value::Value;
        let h = &*heap;
        let a = words_to_val(a0, a1, a2);
        let b = words_to_val(b0, b1, b2);
        if matches!(a, Value::SeqView(_)) || matches!(b, Value::SeqView(_)) {
            return 2;
        }
        h.equal(a, b) as i64
    }
}

/// Address of the global-rebind epoch counter ([`Heap::global_epoch`]), so JIT'd code reads
/// the epoch with a raw load on every loop back-edge / linked call instead of an FFI call.
/// The global-vector hoist captures it at entry and **deopts** when it changes, so hoisting a
/// global's element base out of a loop stays bit-identical to the VM's late binding. Fetched
/// once at arm entry; the address is stable for the process. See [`Heap::global_epoch_ptr`].
///
/// # Safety
/// `heap` must be live; the returned pointer is valid for the process lifetime.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_global_epoch_ptr(heap: *mut Heap) -> *const u64 {
    // SAFETY: `heap` is the live process heap (contract: heap); the read only takes an address.
    unsafe { (*heap).global_epoch_ptr() }
}

/// Address of the unboxed-`i64` fast path's overflow sentinel ([`Heap::jit_i64_overflow`]),
/// so the register-recursion worker can store `1` on overflow and load it to short-circuit
/// the unwind — and the boxed wrapper can read it to decide deopt — all with raw loads/stores
/// (no FFI call per level). Fetched once at arm entry; the address is stable while the arm
/// runs (the heap doesn't move during native execution). The byte is reset to `0` by the
/// wrapper after it observes an overflow.
///
/// # Safety
/// `heap` must be live; the returned pointer is valid for the arm's duration.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_i64_overflow_ptr(heap: *mut Heap) -> *mut u8 {
    // SAFETY: `heap` is the live process heap (contract: heap); this only takes the field's
    // address. Native code storing through it must write 0 or 1, the only valid `bool` bytes.
    unsafe { &mut (*heap).jit_i64_overflow as *mut bool as *mut u8 }
}

/// Reserve `n` argument slots at the top of `roots` and hand back a pointer to them, for the
/// emitting arm to store its operands into directly — the JIT's argument staging, with no
/// intermediate stack slot and block copy (that copy was a measurable share of call cost).
///
/// # Safety
/// `heap` live. The caller must store `n` valid `Value`s into the returned slots **before
/// anything can allocate or collect** — see [`Heap::push_roots_room`], whose doc carries the
/// full invariant.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_push_room(
    heap: *mut Heap,
    n: i64,
) -> *mut crate::core::value::Value {
    // SAFETY: contract heap. The returned slots hold no real `Value` until the emitting arm
    // stores into them, which it does before anything allocates (`push_roots_room`'s doc).
    unsafe { (*heap).push_roots_room(n as usize) }
}

/// An arm's error path, leaving while a call is in flight: `packed` is `(resume ip << 16) |
/// operand depth`. For a preempt unwinding through (ADR-401), the room for those operands at
/// the top of `roots`, which the caller fills at once; null for any other error. See
/// [`crate::eval::compile::begin_in_call_journal`].
///
/// # Safety
/// `heap` live. On a non-null return the caller must store `depth` valid `Value`s into the
/// room before anything can allocate or collect — as for [`brood_rt_push_room`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_in_call_journal(
    heap: *mut Heap,
    packed: i64,
) -> *mut crate::core::value::Value {
    // SAFETY: contract heap; on a non-null return the caller fills the room before anything
    // allocates, as for `brood_rt_push_room`.
    unsafe {
        crate::eval::compile::begin_in_call_journal(
            &mut *heap,
            (packed >> 16) as usize,
            (packed & 0xFFFF) as usize,
        )
    }
}

/// How many builtin arguments are copied inline before spilling to a `Vec`. Covers every
/// arity the JIT's native flat-cell path sees in practice; the spill is correctness, not a
/// path anything hot takes.
const INLINE_ARGV: usize = 6;

/// Direct builtin call from the IR fast-link path (the native flat cell): `func`
/// is the `NativeFnPtr` bits published by `vm_fast_link_publish_native` (arity
/// pre-validated for exactly this argc at publish), `args` the call site's staging
/// stack slot. No roots staging, no `env_get`, no dispatch — one fn-pointer call.
/// The args live in non-GC-visible stack memory: sound under the existing native
/// contract (a native receives unrooted copies — today's `SmallVec` argv has the
/// identical exposure — and must root anything it holds across its own evals).
/// Status ([`status`]): DONE (`*out` holds the result) or ERROR (parked in
/// `jit_pending_error`); the lowering branches to the error block on any nonzero.
///
/// # Safety
/// `heap` live; `out` writable; `func` a valid `NativeFnPtr`; `args` points to
/// `argc` valid `Value`s, which are also the top `argc` entries of `roots`; `ctx` the
/// running activation's context.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_call_native_fl(
    heap: *mut Heap,
    out: *mut crate::core::value::Value,
    func: u64,
    args: *const crate::core::value::Value,
    argc: u32,
    ctx: *const crate::jit::JitCallCtx,
) -> i64 {
    // SAFETY: contracts heap, out and ctx. `func` is the bits of a `NativeFnPtr` published
    // for exactly this arity, so the transmute yields a valid fn pointer; `args` points at
    // `argc` initialised `Value`s, copied out before the builtin runs. The `roots` truncation
    // assumes the arm also staged those `argc` arguments on `roots` (it does, see below).
    unsafe {
        // Rung A1, the hottest callback (every builtin call from native code): the env the
        // builtin receives is READ from the activation context, not stored into the heap; only
        // the two fields a builtin that re-enters or parks would read from the heap — the
        // depth KI-11 bounds and the gateway token a dirty park records — are asserted.
        crate::jit::JitCallCtx::assert_depth_gateway(ctx, &mut *heap);
        let h = &mut *heap;
        let f: crate::core::value::NativeFnPtr = std::mem::transmute(func as usize);
        // COPY, don't borrow: `args` points into `roots` (the arm stages in place — see
        // `brood_rt_push_room`), and a native is free to push roots, which reallocates the
        // buffer and would dangle a borrowed slice mid-call ("a native receives unrooted
        // copies").
        //
        // **Copied with a `match` on the arity, not `SmallVec::from(slice)`.** That goes through
        // `copy_from_slice` → libc `memcpy`, and at an arity's worth of bytes (24–72) the
        // size-class dispatch costs far more than the move (a double-digit regression on
        // builtin-heavy code).
        let n = argc as usize;
        let mut buf = [crate::core::value::Value::Nil; INLINE_ARGV];
        let spill;
        let slice: &[crate::core::value::Value] = if n <= INLINE_ARGV {
            match n {
                0 => {}
                1 => buf[0] = *args,
                2 => {
                    buf[0] = *args;
                    buf[1] = *args.add(1);
                }
                3 => {
                    buf[0] = *args;
                    buf[1] = *args.add(1);
                    buf[2] = *args.add(2);
                }
                _ => {
                    for (i, b) in buf.iter_mut().enumerate().take(n) {
                        *b = *args.add(i);
                    }
                }
            }
            &buf[..n]
        } else {
            spill = std::slice::from_raw_parts(args, n).to_vec();
            &spill[..]
        };
        let env = h.read_root_env((*ctx).env);
        // The emitting arm batch-staged these argc args onto `roots` too (uniform with
        // every fallback path) — they anchor the arg values for any GC the native
        // triggers, exactly like the VM keeps call operands rooted during dispatch.
        // No callee frame consumes them here, so drop them once the native returns.
        let base = h.roots_len() - argc as usize;
        let r = f(slice, env, h);
        h.truncate_roots(base);
        // A builtin that made a large allocation near or over the soft limit: run the check the
        // VM runs once per quantum now, not at the back-edge poll up to 128 iterations later —
        // a megabyte an iteration covers the soft limit's headroom to the hard one in fewer.
        let r = match r {
            Ok(v) if crate::core::alloc::take_soft_attention() => soft_limit_check(h).map(|()| v),
            r => r,
        };
        // ERROR, not the DEOPT a builtin's own failure would read as under the shared status
        // protocol: a builtin's error is the answer, and the arm exits via its error block.
        answer_or_park(h, out, r)
    }
}

/// The soft-limit half of the VM's `quantum_checks`, for a native arm: measure this process
/// near the limit; over it, let the selector decide who pays (`process/pressure.rs`).
#[cold]
fn soft_limit_check(heap: &Heap) -> Result<(), crate::error::LispError> {
    match crate::core::alloc::soft_limit_state() {
        crate::core::alloc::SoftLimit::Clear => Ok(()),
        crate::core::alloc::SoftLimit::Near => {
            crate::process::near_soft_limit(heap);
            Ok(())
        }
        crate::core::alloc::SoftLimit::Over(used) => {
            match crate::process::soft_limit_crossed(heap, used) {
                Some(error) => Err(error),
                None => Ok(()),
            }
        }
    }
}

/// `(throw x)` reached inside an unboxed-scalar register worker
/// (`jit_lower.rs::lower_i64_value`): park the thrown error and tell the worker how to
/// unwind. Returns the **sentinel byte** the native code stores before jumping its
/// `poisoned` unwind block:
///   - `3` — the global `throw` still binds the builtin: the error (payload = the
///     scalar, boxed as `Value::Int`/`Value::Float`) is parked in `jit_pending_error`
///     and the wrapper exits with outcome 3 (error), bit-identical to the VM raising it.
///   - `1` — a user redefined `throw` (late binding must win): nothing is parked and
///     the wrapper deopts (outcome 1), so the VM re-runs the arm — sound because the
///     worker's subset is pure up to the throw — and calls the redefinition.
/// The payload is an immediate scalar (never a heap handle), so parking it across the
/// native unwind is GC-safe by construction.
///
/// # Safety
/// `heap` must be the live context pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_i64_throw(heap: *mut Heap, bits: i64, is_float: i64) -> i64 {
    // SAFETY: contract heap; the parked payload is an immediate scalar, never a heap handle.
    unsafe {
        let h = &mut *heap;
        let sym = crate::core::value::intern("throw");
        let is_builtin = matches!(
            h.env_get(h.global(), sym).map(|v| v.unpack()),
            Some(crate::core::value::ValueRef::Native(id)) if h.native(id).name == "throw"
        );
        if !is_builtin {
            return 1;
        }
        let payload = if is_float != 0 {
            crate::core::value::Value::Float(f64::from_bits(bits as u64))
        } else {
            crate::core::value::Value::int(bits)
        };
        let e = crate::error::LispError::thrown(payload, h);
        h.jit_pending_error = Some(e);
        3
    }
}

// DEBUG ONLY: the call site currently staging its args (set by `brood_rt_dbg_set_staging`
// at the start of each call's staging, read by `brood_rt_dbg_check_slot` on garbage).
// Thread-local because set→check run synchronously on one worker with no yield between.
#[cfg(debug_assertions)]
thread_local! {
    static DBG_STAGING: std::cell::Cell<u32> = const { std::cell::Cell::new(u32::MAX) };
    // Push index within the current staging (0 = first arg pushed), so a garbage push
    // reveals *which* operand (callee/arg0/arg1/…) is bad.
    static DBG_PUSH_IDX: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// DEBUG ONLY: record the call site about to stage its args.
///
/// # Safety
/// A JIT runtime callback: `_heap` is the (unused here) `Heap` pointer the
/// trampoline passes by ABI; callers must invoke it only from JIT'd code.
#[cfg(debug_assertions)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_dbg_set_staging(_heap: *mut Heap, site: u32) {
    DBG_STAGING.with(|s| s.set(site));
    DBG_PUSH_IDX.with(|s| s.set(0));
}

/// DEBUG ONLY: validate a frame-slot read at its SOURCE — `abs_idx` is the absolute roots
/// index the JIT read (`base + slot`), `w0` its tag word. A garbage tag here is the
/// earliest point the corruption is observable; report the slot index vs. roots_len so we
/// can tell an out-of-frame read (`nslots` undercount) from an in-frame corrupted slot.
///
/// # Safety
/// `heap` must be a valid, live `Heap` pointer (it is dereferenced); a JIT
/// runtime callback invoked only from JIT'd code, which upholds that.
#[cfg(debug_assertions)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_dbg_check_slot(
    heap: *mut Heap,
    w0: i64,
    w1: i64,
    w2: i64,
    abs_idx: i64,
) {
    // SAFETY: contract heap, read only. The words are meant to be a real `Value` (contract:
    // words), but this check exists for when they are not: a garbage tag makes `words_to_val`
    // build an invalid enum before `bad_tag` is consulted.
    unsafe {
        let h = &*heap;
        let tag = (w0 as u64 & 0xff) as u8;
        // Invalid tag byte → definitely garbage. Value's max discriminant is `Ratio` (25).
        let bad_tag = tag > 25;
        // Reconstruct the full Value and check for a stale/garbage LOCAL handle (a handle
        // whose generation epoch doesn't match the live epoch, or whose slab index is OOB —
        // i.e. read from a freed/wrong location). This catches the bug-#2 garbage that the
        // tag-only check misses (a valid tag byte over a garbage payload).
        let v = words_to_val(w0, w1, w2);
        let stale = h.dbg_value_stale(v);
        let oob = h.dbg_value_oob(v);
        if bad_tag || stale.is_some() || oob.is_some() {
            let site = DBG_STAGING.with(|s| s.get());
            eprintln!(
                "[slot-read] GARBAGE/STALE read at roots[{abs_idx}] tag={tag:#x} w0={w0:#x} w1={w1:#x} w2={w2:#x} \
             stale={stale:?} oob={oob:?}; roots_len={} (in_frame={}) jit_native_depth={} last_staging_site={site} loc={}",
                h.roots_len(),
                (abs_idx as usize) < h.roots_len(),
                h.jit_native_depth,
                h.dbg_site_loc(site),
            );
        }
    }
}

/// Load the current `Value` from a `ConstVal` (a compiled literal that may be a
/// GC-movable heap handle). The ConstVal lives in the arm's `chunk.code` and is kept
/// alive by the arm's `Arc<CompiledArm>` for the entire JIT code lifetime. Reading
/// `cv.load()` at the point of use ensures we see the GC-updated bits after any
/// `runtime_collect` that ran since the arm was compiled.
///
/// # Safety
/// `cv` must point to a live [`crate::eval::compile::ConstVal`]; `out` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_const_load(
    cv: *const crate::eval::compile::ConstVal,
    out: *mut crate::core::value::Value,
) {
    // SAFETY: `cv` points into the arm's chunk, kept alive by its `Arc<CompiledArm>` for as
    // long as the native code exists (the doc above); `out` per contract out.
    unsafe {
        let v = (*cv).load();
        // DEBUG (bug #2): a ConstVal must hold a RUNTIME/PRELUDE handle or an atom — NEVER a
        // LOCAL handle, and never an invalid tag. A LOCAL/garbage value here means the baked
        // `cv` points at a freed/relocated chunk (use-after-free of a recompiled CompiledArm),
        // which would feed garbage into the arm (e.g. a garbage map_get key).
        // `BROOD_DBG_CONST`.
        #[cfg(debug_assertions)]
        if crate::diagnostics::debug_flags::debug_constants() {
            use crate::core::value::ValueRef;
            let bad = match v.unpack() {
                ValueRef::Pair(id) => Some(("pair", id.region())),
                ValueRef::Vector(id) | ValueRef::Range(id) | ValueRef::SeqView(id) => {
                    Some(("vector", id.region()))
                }
                ValueRef::Map(id) => Some(("map", id.region())),
                ValueRef::Str(id) => Some(("string", id.region())),
                ValueRef::BigInt(id) => Some(("bigint", id.region())),
                ValueRef::Rope(id) => Some(("rope", id.region())),
                ValueRef::Fn(id) | ValueRef::Macro(id) => Some(("fn", id.region())),
                _ => None,
            };
            if let Some((kind, region)) = bad
                && region == crate::core::value::LOCAL
            {
                let raw = v.raw_words();
                eprintln!(
                    "[const-garbage] const_load returned a LOCAL {kind} handle (cv={:p}) raw=[{:#x},{:#x},{:#x}] — \
                     a const must be RUNTIME/PRELUDE; likely a freed/stale ConstVal chunk",
                    cv, raw[0], raw[1], raw[2],
                );
            }
        }
        *out = v;
    }
}

/// Resolve a free global `sym` (a JIT'd call's callee-loading `Inst::Global`/`GlobalIc`,
/// or a global read in value position), writing the value to `*out`. Returns 0 on
/// success, 1 if unbound — in which case the error is parked for the arm to propagate
/// (it returns the error outcome, 3). Reads the *live* env, so a `def` rebind is seen
/// immediately (late binding, exactly like the VM's `Inst::Global`).
///
/// # Safety
/// `heap`/`out` must be live; `sym` is an interned [`crate::core::value::Symbol`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_global_probe(
    heap: *mut Heap,
    out: *mut crate::core::value::Value,
    sym: u32,
    ctx: *const crate::jit::JitCallCtx,
) -> i64 {
    // SAFETY: contracts heap, out and ctx; the two `&mut *heap` borrows are sequential.
    unsafe {
        // Rung A1: this callback consults activation state, so it asserts its own first.
        crate::jit::JitCallCtx::assert_global(ctx, &mut *heap);
        // Speculative sibling of `brood_rt_global` for the **entry hoist**: resolves without
        // parking an error, because the caller's answer to "unbound" is to deopt, not to raise.
        //
        // The hoist resolves every global the arm mentions at entry, including ones only a cold
        // branch reads — so raising there would report `unbound symbol` for a branch the VM
        // never evaluates (`(if (< n 0) never-defined-global (+ n 1))` would throw once hot).
        // Deopting instead hands the arm to the VM, which evaluates only
        // the branch actually taken and raises only if that branch really reads the name. A
        // parked error would then be a lie left in the heap, hence this non-parking form.
        let h = &mut *heap;
        let env = h.read_root_env(h.jit_call_env);
        match h.env_get(env, sym) {
            Some(v) => {
                *out = v;
                0
            }
            None => 1,
        }
    }
}

/// # Safety
/// `heap`/`out` must be live; `sym` is an interned [`crate::core::value::Symbol`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_global(
    heap: *mut Heap,
    out: *mut crate::core::value::Value,
    sym: u32,
    ctx: *const crate::jit::JitCallCtx,
) -> i64 {
    // SAFETY: contracts heap, out and ctx; the `&mut *heap` borrows are sequential.
    unsafe {
        // Rung A1: this callback consults activation state, so it asserts its own first.
        crate::jit::JitCallCtx::assert_global(ctx, &mut *heap);
        #[cfg(debug_assertions)]
        if crate::diagnostics::debug_flags::jit_callback_trace() {
            eprintln!(
                "[jit-cb] brood_rt_global(sym={})",
                crate::core::value::symbol_name(sym)
            );
        }
        match crate::eval::compile::jit_resolve_global(&mut *heap, sym) {
            Some(v) => {
                *out = v;
                0
            }
            None => 1,
        }
    }
}

/// Resolve a free global through the per-site global inline cache (the same
/// [`Heap::vm_global_ics`] the VM's `Inst::GlobalIc` uses), keyed by `site`. On a
/// process-global env this serves a cached, epoch-stamped value instead of walking
/// `env_get` every call — the difference between a hot recursive callee (`fib`) costing
/// one cached read vs. a full name resolution per call. Late binding is preserved by the
/// epoch stamp: a `def` bumps the global epoch, the probe misses, and it re-resolves
/// (and the JIT'd arm is itself invalidated by the same epoch). 0 on success, 1 if
/// unbound (error parked).
///
/// # Safety
/// `heap`/`out` must be live; `sym` is an interned [`crate::core::value::Symbol`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_global_ic(
    heap: *mut Heap,
    out: *mut crate::core::value::Value,
    sym: u32,
    site: u32,
    ctx: *const crate::jit::JitCallCtx,
) -> i64 {
    // SAFETY: contracts heap, out and ctx; the `&mut *heap` borrows are sequential.
    unsafe {
        // Rung A1: this callback consults activation state, so it asserts its own first.
        crate::jit::JitCallCtx::assert_global(ctx, &mut *heap);
        #[cfg(debug_assertions)]
        if crate::diagnostics::debug_flags::jit_callback_trace() {
            eprintln!(
                "[jit-cb] brood_rt_global_ic(sym={}, site={})",
                crate::core::value::symbol_name(sym),
                site
            );
        }
        match crate::eval::compile::jit_resolve_global_ic(&mut *heap, sym, site) {
            Some(v) => {
                *out = v;
                0
            }
            None => 1,
        }
    }
}

/// Run a JIT'd arm's **non-tail** Brood→Brood call. The callee + `argc` args have been
/// staged on the operand stack (`roots`) in the VM's `Inst::Call` layout
/// (`[.., callee, arg0 .. arg_{argc-1}]`); this mirrors the non-tail `Inst::Call` path —
/// read them, dispatch through the interpreter to completion, truncate the operands,
/// and write the result to `*out`. Returns 0 on success, 1 on error (parked for the arm
/// to propagate). The callee runs as a **nested, non-top-level** VM apply, so it can't
/// preempt/suspend across this native boundary (the §7.4 dirty carve-out) — exactly
/// like a Rust builtin calling back into Brood. See
/// [`crate::eval::compile::jit_dispatch_call`].
///
/// # Safety
/// `heap`/`out` must be live; `argc` callee+args are staged on `roots`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_call_slow(
    heap: *mut Heap,
    out: *mut crate::core::value::Value,
    argc: u32,
    site: u32,
    head: u32,
    ctx: *const crate::jit::JitCallCtx,
) -> i64 {
    // SAFETY: contracts heap, out and ctx; the callee and `argc` arguments are staged on
    // `roots` (this fn's `# Safety`), and every live `Value` of the arm is there too, so the
    // collections the nested call may run leave nothing stale.
    unsafe {
        // Rung A1: this callback consults activation state, so it asserts its own first.
        crate::jit::JitCallCtx::assert_into(ctx, &mut *heap);
        #[cfg(debug_assertions)]
        if crate::diagnostics::debug_flags::jit_callback_trace() {
            eprintln!("[jit-cb] brood_rt_call_slow(argc={})", argc);
        }
        let unwind = crate::jit::JitCallCtx::children_may_unwind(ctx);
        match crate::eval::compile::jit_dispatch_call(&mut *heap, argc as usize, site, head, unwind)
        {
            Some(v) => {
                *out = v;
                0
            }
            None => 1,
        }
    }
}

/// Record this native arm in the parked error's `:trace` as the error unwinds through it
/// (KI-117). Called from each arm's error-exit block — the general lowering's `error`
/// block and the scalar worker's `poisoned` block when its sentinel reads 3 — so a native
/// recursion contributes one frame per level, innermost first, exactly as the VM's
/// `BcFrame`s do through `attach_vm_trace` — otherwise a hot function's error would reach
/// `catch` with an empty trace.
///
/// Name and defining file only — the call site that entered the frame lives in the
/// caller's instruction stream, which native code does not keep. A frame with neither is
/// dropped by `push_trace`, so an anonymous arm in a file-less context adds nothing.
/// Cold path (an error is already unwinding), one call per native level.
///
/// # Safety
/// `heap` must be the live context pointer; `arm` the `CompiledArm` whose native code is
/// running — pinned for the process lifetime by `JIT_ARM_KEEPALIVE`, so the pointer the
/// lowering embedded as a constant cannot dangle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_trace_push(
    heap: *mut Heap,
    arm: *const crate::eval::compile::CompiledArm,
) {
    // SAFETY: contract heap; `arm` is pinned for the process lifetime by `JIT_ARM_KEEPALIVE`
    // (this fn's `# Safety`), and it is a separate object from the heap the error is borrowed
    // out of.
    unsafe {
        let h = &mut *heap;
        // An unbound global read raised by this arm's callback: locate it from the arm's own
        // read sites, now that the arm is known (`park_unbound`). Taken once, by the innermost
        // arm — the one that made the read.
        let miss_site = std::mem::replace(&mut h.jit_miss_gsite, u32::MAX);
        let Some(e) = h.jit_pending_error.as_mut() else {
            return;
        };
        if miss_site != u32::MAX && e.pos.is_none() {
            let arm = &*arm;
            if let Some(&(_, pos)) = arm.staged_head_pos.iter().find(|(s, _)| *s == miss_site) {
                e.pos = Some(pos);
                if e.file.is_none() {
                    e.file = arm.src_file.as_deref().map(str::to_string);
                }
            }
        }
        // `push_trace` drops the frame once the trace is capped or the error is a control
        // signal — but only AFTER we have built one, and building one clones the arm's file
        // name. A 50-frame unwind therefore paid 50 allocations to keep 32. The VM's own
        // walker (`attach_vm_trace_callers`) consults `trace_full()` for exactly this reason;
        // this callback, added for KI-117, did not, which cost `errors-deep` 61%.
        if e.is_control() || e.trace_full() {
            return;
        }
        let arm = &*arm;
        e.push_trace(crate::error::TraceFrame {
            name: arm.fn_name.map(crate::core::value::symbol_name_ref),
            file: arm.src_file.clone(),
            pos: None,
        });
    }
}

/// Record *why* the JIT is about to deopt. Called from the shared deopt block with a
/// distinct id per guard, so a deopt can name the check that failed instead of only the
/// checkpoint it resumes at.
///
/// This exists because KI-49 stalled precisely here: an arm deopting 16 times reported
/// `resume_ip=7` every time, and ip 7 is the last *checkpoint* — the arm had five candidate
/// guards after it. Cold path (a deopt is already a VM re-run), so the store is free.
///
/// # Safety
/// `heap` must be a live `*mut Heap` from the JIT calling convention.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_note_deopt(heap: *mut Heap, reason: u32) {
    // SAFETY: `heap` is the live, exclusively used process heap (contract: heap).
    unsafe {
        (*heap).note_jit_deopt_reason(reason);
    }
}

/// Base pointer + length of the IR-readable [`FastLink`] mirror (Track B / Technique A).
/// The JIT loads this at a call site, bounds-checks `site < *out_len`, then reads the
/// slot's `(epoch, code, nslots, env)` with raw loads — replacing the IC probe +
/// `RefCell` borrow [`brood_rt_call_slow`] pays. Re-fetched after each Brood→Brood call
/// (like the roots table's base), since a cold nested call may grow + realloc the table.
///
/// # Safety
/// `heap`/`out_len` must be live; the returned pointer is valid until the table next grows.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_fastlink_base(
    heap: *mut Heap,
    out_len: *mut u64,
    ctx: *const crate::jit::JitCallCtx,
) -> *const FastLink {
    // SAFETY: contracts heap, ctx and out (`out_len`). The returned base is valid only until
    // the table next grows; the emitting arm re-fetches it after every Brood-to-Brood call.
    unsafe {
        // Rung A1: per call site, so it writes nothing — the block cursor is read from the
        // activation context the arm was entered with.
        let (base, len) = (*heap).vm_fast_links_base_for((*ctx).ic_base);
        *out_len = len as u64;
        base
    }
}

/// The JIT's in-IR fast call path (Track B / Technique A) — the hottest callback in the
/// runtime.
///
/// **Four arguments, and that is the point.** Do not pass the [`FastLink`] slot's fields
/// (`site`, `head`, `argc`, `nslots`, `code`, `env`, the IC bases) as arguments: SysV passes
/// six in registers, and the spills dominated this function's profile. Passing the **slot
/// pointer** and reading the fields here fits the call in registers, and the reads are free
/// — the IR has just touched that cache line to check the slot's epoch and identity.
///
/// Sound because the guard has already run: the IR only branches here after proving
/// `site < len`, `slot.epoch == global_epoch`, and that `slot.sym`/`slot.argc` match this
/// site's baked head/arity. Re-reading them is the same single-threaded data one call
/// earlier, and nothing between the guard and here can grow or move the table.
///
/// # Safety
/// `heap` live; `out` writable for one `Value`; `slot` points at a live `FastLink` in this
/// process's table, validated by the IR's guard as described above.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_fast_frame(
    heap: *mut Heap,
    out: *mut crate::core::value::Value,
    site: u32,
    slot: *const crate::core::heap::FastLink,
    ctx: *const crate::jit::JitCallCtx,
) -> i64 {
    // SAFETY: contracts heap, out and ctx; `slot` points at a live `FastLink` the IR's guard
    // has just validated, and nothing between that guard and the copy below can move the
    // table. `FastLink` is `Copy`, so the `ptr::read` duplicates nothing that owns memory.
    unsafe {
        // Rung A1: this callback consults activation state, so it asserts its own first.
        crate::jit::JitCallCtx::assert_into(ctx, &mut *heap);
        use crate::eval::compile::FastLinkOutcome;
        // Copy the whole slot BY VALUE before anything can run: the dispatch below can compile
        // and publish, which grows `vm_fast_links` and reallocates the buffer `slot` points
        // into. A `&*slot` reference here would be sound today only because NLL ends the borrow
        // at its last use — one refactor that moves a read past the dispatch away from UB. The
        // copy makes the discipline structural instead of positional.
        let fl: FastLink = std::ptr::read(slot);
        let (head, argc, nslots, code, env) = (fl.sym, fl.argc, fl.nslots, fl.code, fl.env);
        let bases = (fl.callee_ic_base, fl.callee_gic_base);
        // `out` goes straight down to the native callee, which writes the result into it
        // directly — no store/load round trip through `roots[base]` and no copy here. This is
        // the path that made the change worth making; see `crate::jit::JitArmFn`.
        match crate::eval::compile::jit_dispatch_fast_frame(
            &mut *heap,
            site,
            head,
            argc as usize,
            nslots as usize,
            code as usize,
            env,
            bases,
            out,
            crate::jit::JitCallCtx::children_may_unwind(ctx),
        ) {
            FastLinkOutcome::Done => 0,
            FastLinkOutcome::Error => 1,
            FastLinkOutcome::Fallthrough => 2,
        }
    }
}

/// Suspend-host latch resolution for the **inline** fast-frame path (§7.5): emitted code
/// compared `blocked_under_gateway` against its gateway token and branches here (cold, at most once per arm ever) on a match. `site_head`
/// packs the call site (high 32) and head symbol (low 32).
///
/// # Safety
/// `heap` live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_xcall_latch(
    heap: *mut Heap,
    code: i64,
    site_head: i64,
    argc: i64,
    epoch: i64,
    ctx: *const crate::jit::JitCallCtx,
) {
    // SAFETY: contracts heap and ctx; the `&mut *heap` borrows are sequential.
    unsafe {
        // Rung A1: this callback consults activation state, so it asserts its own first.
        crate::jit::JitCallCtx::assert_into(ctx, &mut *heap);
        let site = (site_head >> 32) as u32;
        let head = site_head as u32;
        crate::eval::compile::jit_latch_dirty_blocked(
            &mut *heap,
            code as usize,
            site,
            head,
            argc as usize,
            epoch as u64,
        );
    }
}

/// The non-Done outcome funnel for the inline fast-frame path — deopt / preempt /
/// tail-chain / error, i.e. `jit_run_fast_link`'s cold arms. The inline path has already
/// restored the caller's ceremony fields, so this reads the caller context back off the
/// heap. Returns the same 0/1/2 status contract as [`brood_rt_fast_frame`].
///
/// # Safety
/// `heap` live; `out` a valid slot for the call's result (written only on status 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn brood_rt_xcall_cold(
    heap: *mut Heap,
    outcome: i64,
    out: *mut crate::core::value::Value,
    site_head: i64,
    argc_nslots: i64,
    epoch: i64,
    stage_base: i64,
    ctx: *const crate::jit::JitCallCtx,
) -> i64 {
    // SAFETY: contracts heap, out and ctx; every access to `*heap` is a fresh, sequential
    // borrow, none held across `jit_xcall_cold_outcome`.
    unsafe {
        // Rung A1: this callback consults activation state, so it asserts its own first.
        crate::jit::JitCallCtx::assert_into(ctx, &mut *heap);
        let site = (site_head >> 32) as u32;
        let head = site_head as u32;
        let argc = (argc_nslots >> 32) as usize;
        let nslots = (argc_nslots as u32) as usize;
        // `jit_force_vm` is set only by a native prologue's stack-limit trip, which deopts
        // at once — so it can change only under a cold outcome, and it is scoped here (the
        // inline call path does not save and restore it per call; rung A1).
        let saved_force_vm = (*heap).jit_force_vm;
        let r = crate::eval::compile::jit_xcall_cold_outcome(
            &mut *heap,
            outcome,
            argc,
            site,
            head,
            epoch as u64,
            stage_base as usize,
            nslots,
            out,
            crate::jit::JitCallCtx::children_may_unwind(ctx),
        );
        (*heap).jit_force_vm = saved_force_vm;
        r
    }
}
