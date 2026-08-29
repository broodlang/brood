//! The `brood_rt_*` runtime-callback table — **obligation 4** of the backend contract
//! ([`super::backend::JitBackend`]): the *only* legal way native code touches the heap,
//! the GC, or the scheduler.
//!
//! Backend-independent by construction. Nothing here knows what emitted the code that
//! calls it, so a second backend inherits this table unchanged — which is most of why the
//! JIT is swappable at all (`docs/backend-seams.md` §1). Every function is `extern "C"`
//! and `#[no_mangle]` so a backend can resolve it by name.
//!
//! ## The ABI (ADR-101 §6.2, adapted for the kept 16-byte enum `Value`)
//!
//! Brood keeps `Value` as a 16-byte enum — the measured decision in
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

use crate::core::heap::{FastLink, Heap};

#[cfg(debug_assertions)]
fn jit_cb_trace_enabled() -> bool {
    static CB_TRACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CB_TRACE.get_or_init(|| {
        std::env::var("BROOD_JIT_CB_TRACE").is_ok_and(|v| v != "0" && !v.is_empty())
    })
}

/// Preemption poll (ADR-027). JIT'd loop back-edges call this; returns nonzero when the
/// process should yield. Mirrors the VM's loop-top exactly: only a **capture-mode green
/// process** is preemptible (`tick_capture` decrements the reduction budget and yields at
/// zero); the root/eval thread (and any non-capture run) never preempts — it just keeps
/// going, like the VM's `tick()` else-branch. Gating here is load-bearing: without it a
/// JIT'd loop on the root thread would yield on its first iteration and bail to the VM,
/// so the JIT could never actually run the loop.
/// Batched preemption poll: burn `n` reductions in one call — the back-edge's
/// in-register countdown calls this once per batch (see `tick_capture_n`), so a
/// tight JIT'd loop pays ~one sub+branch per iteration instead of an FFI.
#[no_mangle]
pub extern "C" fn brood_rt_tick_n(_heap: *mut Heap, n: i64) -> u8 {
    if crate::process::in_capture_run() {
        crate::process::tick_capture_n(n as u32) as u8
    } else {
        0 // root / non-capture: never preempt (matches the VM)
    }
}

#[no_mangle]
pub extern "C" fn brood_rt_tick(_heap: *mut Heap) -> u8 {
    #[cfg(debug_assertions)]
    if jit_cb_trace_enabled() {
        eprintln!("[jit-cb] brood_rt_tick()");
    }
    if crate::process::in_capture_run() {
        crate::process::tick_capture() as u8
    } else {
        0 // root / non-capture: never preempt (matches the VM)
    }
}

/// Is this thread running a **capture-mode** (preemptible green) process? JIT'd code reads it
/// **once at arm entry** to gate the per-back-edge preemption poll: capture mode is constant for
/// an arm's whole execution (set per process-run by the scheduler; unchanged across the arm and
/// its nested calls), so a non-capture loop (the root thread — every single-threaded compute
/// benchmark) skips the [`brood_rt_tick`] FFI entirely, which always returns 0 there anyway. The
/// capture path is unchanged (still polls each iteration), so preemption fairness is untouched.
///
/// # Safety
/// `heap` is unused (the state is a thread-local); the arg keeps the callback ABI uniform.
#[no_mangle]
pub extern "C" fn brood_rt_in_capture(_heap: *mut Heap) -> u8 {
    crate::process::in_capture_run() as u8
}

/// GC safepoint check. JIT'd code calls this where the interpreter would collect (a
/// loop top / before an allocation burst): collect if due and not inside the compile
/// pass (mirrors the eval safepoint, ADR-061).
///
/// # Safety
/// `heap` must be the live, pinned context pointer for the current JIT'd call, with no
/// live `Value`s outside `Heap::roots` (the no-stack-map invariant, ADR-101 §6.2).
#[no_mangle]
pub unsafe extern "C" fn brood_rt_gc_safepoint(heap: *mut Heap) {
    #[cfg(debug_assertions)]
    if jit_cb_trace_enabled() {
        eprintln!("[jit-cb] brood_rt_gc_safepoint()");
    }
    let h = &mut *heap;
    if !crate::process::macro_block_active() && h.gc_due() {
        h.collect(&mut [], &mut []);
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
    std::mem::transmute::<[i64; 3], crate::core::value::Value>([w0, w1, w2])
}

/// `cons` two `Value`s (each by word-triple), writing the fresh pair to `*out`.
///
/// # Safety
/// `heap`/`out` live; the word triples are bytes the JIT read out of real `Value`s.
#[no_mangle]
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
    let h = &mut *heap;
    let car = words_to_val(c0, c1, c2);
    let cdr = words_to_val(d0, d1, d2);
    *out = h.alloc_pair(car, cdr);
}

/// Build a 2-element vector from two `Value`s (each by word-triple), writing the
/// fresh vector to `*out`. The JIT lowering of a `[a b]` literal (`Inst::MakeVector(2)`,
/// e.g. bintree's `make`); mirrors [`brood_rt_cons`] — a bump-allocate that never
/// collects, so the elements need no extra rooting beyond the words passed in.
///
/// # Safety
/// `heap`/`out` live; the word triples are bytes the JIT read out of real `Value`s.
#[no_mangle]
pub unsafe extern "C" fn brood_rt_make_vector2(
    heap: *mut Heap,
    out: *mut crate::core::value::Value,
    a0: i64,
    a1: i64,
    a2: i64,
    b0: i64,
    b1: i64,
    b2: i64,
) {
    let h = &mut *heap;
    let a = words_to_val(a0, a1, a2);
    let b = words_to_val(b0, b1, b2);
    *out = h.alloc_vector2(a, b);
}

/// Build an `n`-element vector from `n` `Value`s staged contiguously at `elems`
/// (the JIT wrote each element's 3 words into a stack slot it owns), writing the
/// fresh vector to `*out`. The variadic generalisation of [`brood_rt_make_vector2`]
/// for a wider `[a b c …]` literal (`Inst::MakeVector(n)`, `n != 2`) — nbody's
/// `[vx vy vz]` / 7-body rebuild. A fixed Cranelift signature can't take `n×3`
/// words, so the elements come by pointer instead of by register-triple.
///
/// Like `make_vector2`, `alloc_vector` only *grows* the LOCAL vector slab (an
/// `alloc_slot!` push — never collects), so the staged elements can't go stale
/// during the call and need no extra rooting beyond the bytes at `elems`.
///
/// # Safety
/// `heap`/`out` live; `elems` points at `n` consecutive, fully-initialised `Value`s.
#[no_mangle]
pub unsafe extern "C" fn brood_rt_make_vector_n(
    heap: *mut Heap,
    out: *mut crate::core::value::Value,
    elems: *const crate::core::value::Value,
    n: i64,
) {
    let h = &mut *heap;
    let n = n as usize;
    let mut items = Vec::with_capacity(n);
    for i in 0..n {
        items.push(std::ptr::read(elems.add(i)));
    }
    *out = h.alloc_vector(items);
}

/// `first` of a `Value` (by word-triple), writing its car to `*out`. The JIT **tag-checks
/// for `Pair` and deopts before calling**, so a non-pair (impossible by that contract)
/// yields `nil` rather than UB.
///
/// # Safety
/// `heap`/`out` live; the word triple is a real `Value::Pair`.
#[no_mangle]
pub unsafe extern "C" fn brood_rt_car(
    heap: *mut Heap,
    out: *mut crate::core::value::Value,
    w0: i64,
    w1: i64,
    w2: i64,
) {
    let h = &mut *heap;
    *out = match words_to_val(w0, w1, w2) {
        crate::core::value::Value::Pair(id) => h.pair(id).0,
        _ => crate::core::value::Value::Nil,
    };
}

/// Byte pointer to the LOCAL nursery pair slab (`Vec<(Value, Value)>`). Called once at JIT
/// function entry so inline `first`/`rest` can compute `base + idx * 48 + {0,24}` directly
/// instead of calling `brood_rt_car`/`cdr` per element. Valid only while no `cons` can grow
/// the slab (arms that allocate must not use the stashed pointer — see `jit_lower_arm`).
///
/// # Safety
/// `heap` must be the live context pointer.
#[no_mangle]
pub unsafe extern "C" fn brood_rt_pair_nursery_base(heap: *mut Heap) -> *const u8 {
    (*heap).local_pair_nursery_base()
}

/// Byte pointer to the LOCAL old-generation pair slab. Companion to
/// [`brood_rt_pair_nursery_base`] — for pairs promoted out of the nursery.
///
/// # Safety
/// `heap` must be the live context pointer.
#[no_mangle]
pub unsafe extern "C" fn brood_rt_pair_old_base(heap: *mut Heap) -> *const u8 {
    (*heap).local_pair_old_base()
}

/// Byte pointer to the LOCAL nursery **vector** slab, so a JIT arm can inline a
/// small-vector element read instead of calling [`brood_rt_vector_ref`]. The
/// vector analog of [`brood_rt_pair_nursery_base`].
///
/// # Safety
/// `heap` must be the live context pointer.
#[no_mangle]
pub unsafe extern "C" fn brood_rt_vec_nursery_base(heap: *mut Heap) -> *const u8 {
    (*heap).local_vec_nursery_base()
}

/// Byte pointer to the LOCAL old-generation vector slab. Companion to
/// [`brood_rt_vec_nursery_base`].
///
/// # Safety
/// `heap` must be the live context pointer.
#[no_mangle]
pub unsafe extern "C" fn brood_rt_vec_old_base(heap: *mut Heap) -> *const u8 {
    (*heap).local_vec_old_base()
}

/// `rest` counterpart of [`brood_rt_car`] — writes the pair's cdr to `*out`.
///
/// # Safety
/// `heap`/`out` live; the word triple is a real `Value::Pair`.
#[no_mangle]
pub unsafe extern "C" fn brood_rt_cdr(
    heap: *mut Heap,
    out: *mut crate::core::value::Value,
    w0: i64,
    w1: i64,
    w2: i64,
) {
    let h = &mut *heap;
    *out = match words_to_val(w0, w1, w2) {
        crate::core::value::Value::Pair(id) => h.pair(id).1,
        _ => crate::core::value::Value::Nil,
    };
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
#[no_mangle]
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
    0
}

/// `(%table-has? t k)` from JIT'd code (the `PrimOp::TableHas` lowering). Returns
/// 0 = done (`*out` holds the boolean), 1 = deopt (first operand isn't a Table —
/// the VM owns the exact type error), 2 = a real error was parked in
/// `jit_pending_error` (dropped table / invalid key) — the arm exits via its
/// error block (outcome 3), bit-identical to the native raising it.
///
/// # Safety
/// `heap` must be the live context pointer and `out` a writable `*mut Value`.
#[no_mangle]
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
    use crate::core::value::Value;
    let h = &mut *heap;
    let Value::Table(id) = words_to_val(t0, t1, t2) else {
        return 1;
    };
    let key = words_to_val(k0, k1, k2);
    if let Err(e) = crate::core::table::check_key("table-has?", key) {
        h.jit_pending_error = Some(e);
        return 2;
    }
    match crate::core::table::has(h, id, key) {
        Ok(b) => {
            *out = Value::Bool(b);
            0
        }
        Err(e) => {
            h.jit_pending_error = Some(e);
            2
        }
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
#[no_mangle]
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
    use crate::core::value::Value;
    let h = &mut *heap;
    let Value::Table(id) = words_to_val(t0, t1, t2) else {
        return 1;
    };
    let key = words_to_val(k0, k1, k2);
    if let Err(e) = crate::core::table::check_key("table-get", key) {
        h.jit_pending_error = Some(e);
        return 2;
    }
    match crate::core::table::get(h, id, key, Value::Nil) {
        Ok(v) => {
            *out = v;
            0
        }
        Err(e) => {
            h.jit_pending_error = Some(e);
            2
        }
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
#[no_mangle]
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
    use crate::core::value::Value;
    let h = &mut *heap;
    let Value::Table(id) = words_to_val(t0, t1, t2) else {
        return 1;
    };
    let key = words_to_val(k0, k1, k2);
    if let Err(e) = crate::core::table::check_key("table-put", key) {
        h.jit_pending_error = Some(e);
        return 2;
    }
    let val = words_to_val(v0, v1, v2);
    match crate::core::table::put(h, id, key, val) {
        Ok(v) => {
            *out = v;
            0
        }
        Err(e) => {
            h.jit_pending_error = Some(e);
            2
        }
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
#[no_mangle]
pub unsafe extern "C" fn brood_rt_vector_base(
    heap: *mut Heap,
    w0: i64,
    w1: i64,
    w2: i64,
    out_len: *mut i64,
) -> *const u8 {
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
    use crate::core::value::Value;
    if let Value::Table(id) = words_to_val(w0, w1, w2) {
        if let Some((slots, flag)) = crate::core::table::jit_dense_base(id) {
            *out_flag = flag as i64;
            return slots as *const u8;
        }
    }
    *out_flag = 0;
    std::ptr::null()
}

/// The process global-rebind epoch ([`Heap::global_epoch`]). Used by the JIT's
/// global-vector hoist: a no-call arm captures the epoch at entry, then re-checks it on
/// each loop back-edge and **deopts** if it changed — so hoisting a global's element base
/// out of the loop stays bit-identical to the VM's per-iteration late binding (a `def`
/// rebinding the global from another process bumps the epoch → the arm deopts and the VM
/// re-runs against the live binding). Checking on the back-edge (not per read) is enough:
/// a deopt always re-runs from the current frame on the VM.
///
/// # Safety
/// `heap` must be the live context pointer.
#[no_mangle]
pub unsafe extern "C" fn brood_rt_global_epoch(heap: *mut Heap) -> i64 {
    (*heap).global_epoch() as i64
}

/// `(get m k)` on a CHAMP map — the native half of [`PrimOp::MapGet`].
///
/// Status protocol, matching [`brood_rt_table_get2`]: **0** = a present, non-nil value is in
/// `*out`; **1** = decline. Never 2 — a map probe raises nothing, so there is no error to
/// park.
///
/// Declines for a non-map receiver, an absent key, or a stored `nil`, which is exactly the
/// VM's rule in `prim2_inline_exec`. The last two look the same from here and must: both have
/// to reach `get`'s `%lookup-miss`, where a record whose contents are not its fields resolves
/// through the `Lookup` ability. Keeping that in Brood is the point — this is a fast path for
/// the hit, not a second implementation of `get`.
///
/// # Safety
/// `heap`/`out` live; the word triples are bytes the JIT read out of real `Value`s.
#[no_mangle]
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
    use crate::core::value::Value;
    let h = &mut *heap;
    let Value::Map(id) = words_to_val(m0, m1, m2) else {
        return 1;
    };
    match h.map_get(id, words_to_val(k0, k1, k2)) {
        Some(v) if !matches!(v, Value::Nil) => {
            *out = v;
            0
        }
        _ => 1,
    }
}

/// The **dispatch identity** of a value, as an interned keyword symbol — the read a
/// speculation guard makes before calling an ability impl directly
/// (docs/dispatch-speculation.md, phase 2a).
///
/// Returns the `Symbol` widened to `i64`, or **-1** when the identity is not a keyword. The
/// second case is real rather than defensive: `%identity-of` answers with whatever truthy
/// value sits under `:__id__`, and a hand-written `{:__id__ 42}` therefore identifies as `42`.
/// A guard compares against a keyword constant, so -1 can never match one and the site falls
/// back — which is the correct answer for a value the speculation was not about.
///
/// This exists as a callback rather than inline code because a record's identity is a CHAMP
/// field, and reading one is not something native code can do on its own. It delegates to
/// [`Heap::dispatch_identity`] — the single definition, pinned to the language's
/// `%identity-of` by `tests/dispatch_identity_agrees.rs`. That pinning is the whole safety
/// property here: a guard that computes identity differently from the dispatcher passes and
/// then calls the wrong impl, silently.
///
/// # Safety
/// `heap` must be the live context pointer; the word triple is bytes the JIT read out of a
/// real `Value`.
#[no_mangle]
pub unsafe extern "C" fn brood_rt_dispatch_identity(
    heap: *mut Heap,
    w0: i64,
    w1: i64,
    w2: i64,
) -> i64 {
    match (*heap).dispatch_identity(words_to_val(w0, w1, w2)) {
        crate::core::value::Value::Keyword(s) => s as i64,
        _ => -1,
    }
}

/// Address of the global-epoch counter, so JIT'd code reads the epoch with a raw load instead
/// of calling [`brood_rt_global_epoch`] on every loop back-edge / linked call. Fetched once at
/// arm entry; the address is stable for the process. See [`Heap::global_epoch_ptr`].
///
/// # Safety
/// `heap` must be live; the returned pointer is valid for the process lifetime.
#[no_mangle]
pub unsafe extern "C" fn brood_rt_global_epoch_ptr(heap: *mut Heap) -> *const u64 {
    (*heap).global_epoch_ptr()
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
#[no_mangle]
pub unsafe extern "C" fn brood_rt_i64_overflow_ptr(heap: *mut Heap) -> *mut u8 {
    &mut (*heap).jit_i64_overflow as *mut bool as *mut u8
}

/// Batch arg staging: append `n` staged `Value`s (from the call site's staging
/// stack slot) onto `roots` in one reserve+memcpy — replacing `brood_rt_push` ×
/// argc on every Brood→Brood / slow-dispatch call from JIT'd code.
///
/// # Safety
/// `heap` must be live; `src` must point to `n` valid `Value`s (written by the
/// emitting arm just before this call, with no intervening safepoint).
#[no_mangle]
pub unsafe extern "C" fn brood_rt_push_n(
    heap: *mut Heap,
    src: *const crate::core::value::Value,
    n: i64,
) -> i64 {
    (*heap).push_roots_n(src, n as usize);
    0
}

/// Direct builtin call from the IR fast-link path (the native flat cell): `func`
/// is the `NativeFnPtr` bits published by `vm_fast_link_publish_native` (arity
/// pre-validated for exactly this argc at publish), `args` the call site's staging
/// stack slot. No roots staging, no `env_get`, no dispatch — one fn-pointer call.
/// The args live in non-GC-visible stack memory: sound under the existing native
/// contract (a native receives unrooted copies — today's `SmallVec` argv has the
/// identical exposure — and must root anything it holds across its own evals).
/// Status: 0 = done (`*out` holds the result), 1 = error parked in
/// `jit_pending_error`.
///
/// # Safety
/// `heap` live; `out` writable; `func` a valid `NativeFnPtr`; `args` points to
/// `argc` valid `Value`s.
#[no_mangle]
pub unsafe extern "C" fn brood_rt_call_native_fl(
    heap: *mut Heap,
    out: *mut crate::core::value::Value,
    func: u64,
    args: *const crate::core::value::Value,
    argc: u32,
) -> i64 {
    let h = &mut *heap;
    let f: crate::core::value::NativeFnPtr = std::mem::transmute(func as usize);
    let slice = std::slice::from_raw_parts(args, argc as usize);
    let env = h.read_root_env(h.jit_call_env);
    // The emitting arm batch-staged these argc args onto `roots` too (uniform with
    // every fallback path) — they anchor the arg values for any GC the native
    // triggers, exactly like the VM keeps call operands rooted during dispatch.
    // No callee frame consumes them here, so drop them once the native returns.
    let base = h.roots_len() - argc as usize;
    let r = f(slice, env, h);
    h.truncate_roots(base);
    match r {
        Ok(v) => {
            *out = v;
            0
        }
        Err(e) => {
            h.jit_pending_error = Some(e);
            1
        }
    }
}

/// Base pointer of the operand-stack/`roots` buffer. JIT'd code calls this once at
/// entry, then indexes a frame slot `k` directly at `roots_base + k *
/// size_of::<Value>()` (tag byte at +0, payload at +8). Valid for the arm's duration:
/// a tier-1 JIT'd arm keeps operands in registers (never `push`es `roots`) and the
/// int-arithmetic subset never allocates, so `roots` doesn't reallocate.
///
/// # Safety
/// `heap` must be the live context pointer.
#[no_mangle]
pub unsafe extern "C" fn brood_rt_roots_base(heap: *mut Heap) -> *mut u8 {
    (*heap).roots_base_ptr() as *mut u8
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
#[no_mangle]
pub unsafe extern "C" fn brood_rt_i64_throw(heap: *mut Heap, bits: i64, is_float: i64) -> i64 {
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

// DEBUG ONLY: the call site currently staging its args (set by `brood_rt_dbg_set_staging`
// at the start of each call's staging, read by `brood_rt_push` on garbage). Thread-local
// because set→push run synchronously on one worker with no yield between.
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
#[no_mangle]
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
#[no_mangle]
pub unsafe extern "C" fn brood_rt_dbg_check_slot(
    heap: *mut Heap,
    w0: i64,
    w1: i64,
    w2: i64,
    abs_idx: i64,
) {
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

/// Push a `Value` (by word-triple) onto the operand stack (`roots`). The JIT stages a
/// Brood→Brood call's callee + args here, in the VM's `Inst::Call` layout, before
/// [`brood_rt_call_slow`]. Goes through `push_root` so the `roots` length/capacity are
/// maintained; a growth may reallocate the buffer, so the JIT re-fetches
/// [`brood_rt_roots_base`] after the call.
///
/// # Safety
/// `heap` must be the live context pointer; the word triple is bytes the JIT read out
/// of a real `Value` (a slot, an `Int` box, or a handle result).
#[no_mangle]
pub unsafe extern "C" fn brood_rt_push(heap: *mut Heap, w0: i64, w1: i64, w2: i64) {
    let v = words_to_val(w0, w1, w2);
    // DEBUG: catch a stale/OOB heap handle being STAGED as a call arg — the definitive
    // bug-#2 catch point (independent of how the value was produced: read_words, a call
    // result, car/cdr, …). Prints the staging site (→ the JIT arm) before the value goes
    // into roots and out to the callee.
    #[cfg(debug_assertions)]
    {
        let h = &*heap;
        let stale = h.dbg_value_stale(v);
        let oob = h.dbg_value_oob(v);
        if stale.is_some() || oob.is_some() {
            let site = DBG_STAGING.with(|s| s.get());
            eprintln!(
                "[push-stage] STAGING GARBAGE arg w0={w0:#x} w1={w1:#x} w2={w2:#x} \
                 stale={stale:?} oob={oob:?}; roots_len={} jit_native_depth={} site={site} arm='{}' loc={}",
                h.roots_len(),
                h.jit_native_depth,
                crate::core::value::symbol_name_opt(h.jit_dbg_fn).unwrap_or("<none/computed>"),
                h.dbg_site_loc(site),
            );
        }
    }
    (*heap).push_root(v);
}

/// Load the current `Value` from a `ConstVal` (a compiled literal that may be a
/// GC-movable heap handle). The ConstVal lives in the arm's `chunk.code` and is kept
/// alive by the arm's `Arc<CompiledArm>` for the entire JIT code lifetime. Reading
/// `cv.load()` at the point of use ensures we see the GC-updated bits after any
/// `runtime_collect` that ran since the arm was compiled.
///
/// # Safety
/// `cv` must point to a live [`crate::eval::compile::ConstVal`]; `out` must be valid.
#[no_mangle]
pub unsafe extern "C" fn brood_rt_const_load(
    cv: *const crate::eval::compile::ConstVal,
    out: *mut crate::core::value::Value,
) {
    let v = (*cv).load();
    // DEBUG (bug #2): a ConstVal must hold a RUNTIME/PRELUDE handle or an atom — NEVER a
    // LOCAL handle, and never an invalid tag. A LOCAL/garbage value here means the baked
    // `cv` points at a freed/relocated chunk (use-after-free of a recompiled CompiledArm),
    // which would feed garbage into the arm (e.g. a garbage map_get key).
    #[cfg(debug_assertions)]
    if std::env::var_os("BROOD_DBG_CONST").is_some() {
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
        if let Some((kind, region)) = bad {
            if region == crate::core::value::LOCAL {
                let raw = std::mem::transmute::<crate::core::value::Value, [i64; 3]>(v);
                eprintln!(
                    "[const-garbage] const_load returned a LOCAL {kind} handle (cv={:p}) raw=[{:#x},{:#x},{:#x}] — \
                     a const must be RUNTIME/PRELUDE; likely a freed/stale ConstVal chunk",
                    cv, raw[0], raw[1], raw[2],
                );
            }
        }
    }
    *out = v;
}

/// Resolve a free global `sym` (a JIT'd call's callee-loading `Inst::Global`/`GlobalIc`,
/// or a global read in value position), writing the value to `*out`. Returns 0 on
/// success, 1 if unbound — in which case the error is parked for the arm to propagate
/// (it returns the error outcome, 3). Reads the *live* env, so a `def` rebind is seen
/// immediately (late binding, exactly like the VM's `Inst::Global`).
///
/// # Safety
/// `heap`/`out` must be live; `sym` is an interned [`crate::core::value::Symbol`].
#[no_mangle]
pub unsafe extern "C" fn brood_rt_global_probe(
    heap: *mut Heap,
    out: *mut crate::core::value::Value,
    sym: u32,
) -> i64 {
    // Speculative sibling of `brood_rt_global` for the **entry hoist**: resolves without
    // parking an error, because the caller's answer to "unbound" is to deopt, not to raise.
    //
    // The hoist resolves every global the arm mentions at entry, including ones only a cold
    // branch reads — so raising there reported `unbound symbol` for a branch the VM never
    // evaluates: `(defn pick (n) (if (< n 0) never-defined-global (+ n 1)))` worked until it
    // got hot, then threw. Deopting instead hands the arm to the VM, which evaluates only
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

/// # Safety
/// `heap`/`out` must be live; `sym` is an interned [`crate::core::value::Symbol`].
#[no_mangle]
pub unsafe extern "C" fn brood_rt_global(
    heap: *mut Heap,
    out: *mut crate::core::value::Value,
    sym: u32,
) -> i64 {
    #[cfg(debug_assertions)]
    if jit_cb_trace_enabled() {
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
#[no_mangle]
pub unsafe extern "C" fn brood_rt_global_ic(
    heap: *mut Heap,
    out: *mut crate::core::value::Value,
    sym: u32,
    site: u32,
) -> i64 {
    #[cfg(debug_assertions)]
    if jit_cb_trace_enabled() {
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
#[no_mangle]
pub unsafe extern "C" fn brood_rt_call_slow(
    heap: *mut Heap,
    out: *mut crate::core::value::Value,
    argc: u32,
    site: u32,
    head: u32,
) -> i64 {
    #[cfg(debug_assertions)]
    if jit_cb_trace_enabled() {
        eprintln!("[jit-cb] brood_rt_call_slow(argc={})", argc);
    }
    match crate::eval::compile::jit_dispatch_call(&mut *heap, argc as usize, site, head) {
        Some(v) => {
            *out = v;
            0
        }
        None => 1,
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
#[no_mangle]
pub unsafe extern "C" fn brood_rt_note_deopt(heap: *mut Heap, reason: u32) {
    (*heap).note_jit_deopt_reason(reason);
}

/// Base pointer + length of the IR-readable [`FastLink`] mirror (Track B / Technique A).
/// The JIT loads this at a call site, bounds-checks `site < *out_len`, then reads the
/// slot's `(epoch, code, nslots, env)` with raw loads — replacing the IC probe +
/// `RefCell` borrow [`brood_rt_call_slow`] pays. Re-fetched after each Brood→Brood call
/// (like [`brood_rt_roots_base`]), since a cold nested call may grow + realloc the table.
///
/// # Safety
/// `heap`/`out_len` must be live; the returned pointer is valid until the table next grows.
#[no_mangle]
pub unsafe extern "C" fn brood_rt_fastlink_base(
    heap: *mut Heap,
    out_len: *mut u64,
) -> *const FastLink {
    let (base, len) = (*heap).vm_fast_links_base();
    *out_len = len as u64;
    base
}

/// Run a JIT'd arm's **non-tail** free-global call via the in-IR fast-link path: the IR has
/// validated the call site's flat-table entry (`site < len` && epoch-current) and read
/// `(nslots, code, env, callee_ic_base, callee_gic_base)` from it; this sets up the callee
/// frame, installs the callee's IC-block cursors around its native call (KI-20 — so it reads
/// its own inline caches, not the caller's), runs it, and writes the result to `*out`.
/// Returns the status the IR branches on: `0` = done, `1` = error
/// (parked for the arm to propagate), `2` = could-not-fast-link (over the native-recursion
/// cap, or the IC moved) — the IR falls to [`brood_rt_call_slow`] with the args left
/// staged. See [`crate::eval::compile::jit_dispatch_fast_frame`].
///
/// # Safety
/// `heap`/`out` must be live; the `argc` args are staged on `roots`; `code` is the native
/// entry pointer the IR read from the (epoch-validated) flat table.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn brood_rt_fast_frame(
    heap: *mut Heap,
    out: *mut crate::core::value::Value,
    site: u32,
    head: u32,
    argc: u32,
    nslots: u32,
    code: u64,
    env: u64,
    callee_ic_base: u32,
    callee_gic_base: u32,
) -> i64 {
    use crate::eval::compile::FastLinkOutcome;
    match crate::eval::compile::jit_dispatch_fast_frame(
        &mut *heap,
        site,
        head,
        argc as usize,
        nslots as usize,
        code as usize,
        env,
        (callee_ic_base, callee_gic_base),
    ) {
        FastLinkOutcome::Done(v) => {
            *out = v;
            0
        }
        FastLinkOutcome::Error => 1,
        FastLinkOutcome::Fallthrough => 2,
    }
}
