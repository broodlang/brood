use super::*;

// The backend-independent decisions this lowering consults — `crate::jit::backend`'s
// obligation 1: a backend does not decide *whether* to lower, it carries out a decision made
// above it. The child modules under `jit_lower/` reach these through their own `use super::*`.
use super::jit_plan::chunk_in_jit_subset;
use super::jit_plan::codegen::{
    inst_allocates_hot, inst_may_allocate, inst_opcode_name, invariant_global_vecs,
    invariant_param_slots, jit_i64_enabled, plan_general_lowering, vector_base_hoist_safe,
};

// The unboxed scalar (i64/f64) register worker, tried first by `jit_lower_arm`.
#[cfg(feature = "jit")]
mod i64;
#[cfg(feature = "jit")]
use i64::jit_lower_i64_arm;
// Re-exported for `jit::cranelift`'s `JitBackend` tiering advisories — how the backend
// answers "have I demoted this fn off the register worker?" and "record that I must".
#[cfg(feature = "jit")]
pub(crate) use i64::{arm_i64_eligible, arm_i64_too_deep, i64_mark_too_deep};

// The pieces of `jit_lower_arm_inner`, in the order a lowering uses them:
// `prepass` (block leaders and operand depths — pure analysis), `callbacks` (the runtime
// callbacks the arm imports), `entry` (the entry block: hoists, guards, carried params),
// then the per-instruction emitters — `load` (`Const`/`Global`/`SetLocal` and the deopt
// journal), `prim` (primitives and literals), `call` (`Call`/`SelfCall`), `control`
// (`Jump`/`JumpIfFalse` and the deferred join typing) — over the shared CLIF helpers in
// `emit`, and finally `finish` (exit blocks, IR dump, define/finalize).
#[cfg(feature = "jit")]
mod callbacks;
#[cfg(feature = "jit")]
mod control;
#[cfg(feature = "jit")]
mod emit;
#[cfg(feature = "jit")]
mod entry;
#[cfg(feature = "jit")]
mod load;
#[cfg(feature = "jit")]
pub(crate) mod prepass;
#[cfg(feature = "jit")]
use control::PendingEdge;
#[cfg(feature = "jit")]
mod call;
#[cfg(feature = "jit")]
mod finish;
#[cfg(feature = "jit")]
mod prim;
#[cfg(feature = "jit")]
use emit::DeoptReason;

/// The virtualized operand-stack element for the emit loop. A logical operand is an
/// unboxed `Int`/`Float`/`Bool` SSA value, a frame `Slot`, a `Handle` (three `Value`
/// words), or a hoisted invariant global vector/table.
///
/// `Slot(k)` is read lazily from `roots[base+k]` and is the only way a handle that must
/// survive a safepoint sits on the stack — the moving collector sees and relocates it
/// there. A `Handle` in registers is **transient**: produced and consumed within a block,
/// never live across the loop back edge (the only safepoint), so the GC never sees it.
#[cfg(feature = "jit")]
#[derive(Clone, Copy)]
pub(super) enum Op {
    Int(cranelift_codegen::ir::Value),
    /// An unboxed `f64` SSA value; boxed to `Value::Float` when stored/returned.
    /// Float comparisons yield an `Op::Int` i8 (a Bool), so branch handling is shared.
    Float(cranelift_codegen::ir::Value),
    /// A boolean SSA value (`i64` 0/1) that has crossed a block boundary (a comparison
    /// widened through a block param, which erases the `i8`-means-bool signal); tagged
    /// so it still boxes as `Bool` and branches correctly in `JumpIfFalse`.
    Bool(cranelift_codegen::ir::Value),
    Slot(usize),
    Handle(
        cranelift_codegen::ir::Value,
        cranelift_codegen::ir::Value,
        cranelift_codegen::ir::Value,
    ),
    /// A hoisted invariant global vector (matmul LICM): its resolved `Value` words
    /// (`w0..w2`) plus its element storage base (`ptr`, `len`), resolved once at entry.
    HoistedVec {
        ptr: cranelift_codegen::ir::Value,
        len: cranelift_codegen::ir::Value,
        w0: cranelift_codegen::ir::Value,
        w1: cranelift_codegen::ir::Value,
        w2: cranelift_codegen::ir::Value,
    },
    /// A hoisted invariant global dense table (the sieve lever): its resolved `Value`
    /// words plus the dense slot region base and the store's `dense`-flag address.
    HoistedTable {
        slots: cranelift_codegen::ir::Value,
        flag: cranelift_codegen::ir::Value,
        w0: cranelift_codegen::ir::Value,
        w1: cranelift_codegen::ir::Value,
        w2: cranelift_codegen::ir::Value,
    },
}

// ===================== JIT lowering (ADR-101) =====================
//
// Lower a chunked arm to native code via Cranelift, co-located here because it reads the
// private `Inst`/`Chunk` bytecode. The operand stack is virtualised at compile time; any
// instruction or shape outside the lowerable subset makes lowering **bail** (`None`) and
// the arm stays on the VM.

#[cfg(feature = "jit")]
static JIT_ARM_SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

#[cfg(feature = "jit")]
thread_local! {
    /// The most recent mid-emit refusal reason on this compile thread — recorded by
    /// [`bail`] and friends, cleared at each `jit_lower_arm_inner` entry, consumed by
    /// `jit_runtime`'s `trace_lower_declined` so the arm-named `[jit-bail]` line carries
    /// the specific reason instead of the generic `lowering-returned-none`.
    static LAST_MID_EMIT_REASON: std::cell::Cell<Option<(&'static str, Option<&'static str>)>> =
        const { std::cell::Cell::new(None) };
}

/// Record a mid-emit refusal reason for the arm currently being lowered on this thread.
#[cfg(feature = "jit")]
pub(super) fn record_mid_emit_reason(reason: &'static str) {
    LAST_MID_EMIT_REASON.set(Some((reason, None)));
}

/// [`record_mid_emit_reason`] with a second static token the printer appends as
/// `reason:detail` — for a refusal whose class is fixed but whose instance matters
/// (`emit-unsupported-inst:MakeVector`).
#[cfg(feature = "jit")]
pub(super) fn record_mid_emit_detail(reason: &'static str, detail: &'static str) {
    LAST_MID_EMIT_REASON.set(Some((reason, Some(detail))));
}

/// Give up on the lowering in progress, naming WHY — the one legitimate way for the
/// `Option`-returning lowering to answer `None`. The arm-named `[jit-bail]` line that
/// `jit_runtime::trace_lower_declined` prints carries the reason; nothing here prints.
///
/// Every give-up path records (this, [`OrBail::or_bail`] on a `?`, or
/// [`record_mid_emit_detail`]) so that `lowering-returned-none` — the printer's fallback
/// when NOTHING recorded — can only mean a path that was missed.
#[cfg(feature = "jit")]
pub(super) fn bail<T>(reason: &'static str) -> Option<T> {
    record_mid_emit_reason(reason);
    None
}

/// `opt.or_bail("reason")?` — name a `None` at the site that ORIGINATES it, so the `?`
/// chain above propagates a reason rather than a bare `None`. Records only when the
/// option is `None`, and the recorder is last-writer-wins, so wrap ORIGINATORS
/// (`stack.pop()`, a Cranelift `Result`, a field that may be unset) and never a call
/// to a lowering helper that already names its own refusal — that would overwrite the
/// specific reason with the generic one.
#[cfg(feature = "jit")]
pub(super) trait OrBail<T> {
    fn or_bail(self, reason: &'static str) -> Option<T>;
}
#[cfg(feature = "jit")]
impl<T> OrBail<T> for Option<T> {
    fn or_bail(self, reason: &'static str) -> Option<T> {
        if self.is_none() {
            record_mid_emit_reason(reason);
        }
        self
    }
}

/// Is the §7.5 hot re-lowering enabled for this process (the tiering glue's gate)?
#[cfg(feature = "jit")]
pub(crate) fn xcall_relower_enabled() -> bool {
    call::xcall_emit(true)
}

/// Take (and clear) the mid-emit refusal reason recorded during the lowering attempt
/// that just returned `None` — for the caller's arm-named decline line.
#[cfg(feature = "jit")]
pub(crate) fn take_mid_emit_reason() -> Option<(&'static str, Option<&'static str>)> {
    LAST_MID_EMIT_REASON.take()
}

/// Compile `arm`'s chunk to a native arm function (`crate::jit::JitArmFn`), or `None` to
/// leave it on the VM. The compiled fn reads its frame from `roots[base..]`, computes in
/// registers, writes a Done result through the caller's `out` pointer, and returns an
/// outcome code (see `finish::ExitBlocks`). The returned pointer is valid for the life of
/// `jit` (its module owns the code).
#[cfg(feature = "jit")]
pub(crate) fn jit_lower_arm(
    jit: &mut crate::jit::CraneliftBackend,
    arm: &CompiledArm,
    slot_tags: &[u8],
) -> Option<*const u8> {
    // Unboxed-i64 fast path: an int-only single-arg recursive arm (`fib`) gets a register
    // calling convention for its self-recursion — args/results in registers, no boxing /
    // roots-staging / fast-link dispatch (that protocol is most of `fib`'s time). Falls
    // through to the general lowering when the arm isn't eligible.
    //
    // **Tried before the gate below, and the order is load-bearing.** The gate rejects the
    // boxed *general* lowering, and its predicate — a named defn with non-tail calls, no
    // inline vector op, no self-tail loop — describes `fib`/`pfib` exactly, i.e. the arms the
    // scalar worker wins biggest on. Consult the gate first and they silently stop lowering:
    // still *correct* (they run on the VM), so the JIT differential stays green and only a
    // benchmark notices. Keep the scalar attempt first.
    if jit_i64_enabled()
        && let Some(p) = jit_lower_i64_arm(jit, arm)
    {
        return Some(p);
    }
    // Whether the general lowering is worth doing at all is a backend-independent decision:
    // it lives in `jit_plan` with the measurement that justifies it, and a refusal is
    // reportable there (`BROOD_JIT_BAIL_TRACE=1`) instead of an unexplained `None` from here.
    plan_general_lowering(arm).ok()?; // its `record_bail` records the reason
    jit_lower_arm_inner(jit, arm, slot_tags, None, false)
}

/// Re-lower the arm's OWN body — same chunk, same frame size, same checkpoint — with the
/// hot-body emission armed (the §7.5 inline fast-frame blob at its non-tail named call
/// sites). Rides the deferred queue like the inlined upgrade, so its fatter compile never
/// delays an initial tier; the swap in `jit_tier` is a plain `jit_code` pointer swap
/// (identical layout, no `inline_installed` flip). Skips the i64 worker attempt — the arm
/// already has installed code, and the worker never reaches this path (its arms decline
/// via `declines_inline_upgrade`).
pub(crate) fn jit_lower_arm_hot(
    jit: &mut crate::jit::CraneliftBackend,
    arm: &CompiledArm,
    slot_tags: &[u8],
) -> Option<*const u8> {
    // No `plan_general_lowering` here: an arm reaching the hot stage passed the gate at its
    // first compile, so re-running it would be a no-op. The subset pre-bails inside
    // `jit_lower_arm_inner` still guard everything real.
    jit_lower_arm_inner(jit, arm, slot_tags, None, true)
}

/// Keeps the **self-inlined** body's `Node` + `Chunk` alive for the process lifetime. The
/// inlined native code bakes the raw addresses of the spliced chunk's `ConstVal`s into
/// itself (`brood_rt_const_load(cv_ptr, …)`, see `jit_lower_arm_inner`), exactly as the
/// small-native path does for `arm.chunk` — but the self-inlined body lives in an
/// *ephemeral* chunk re-derived here, NOT in `arm.chunk`. The arm-level
/// `JIT_ARM_KEEPALIVE` retains `arm` (hence `arm.chunk`, the small body), so it does NOT
/// cover this spliced chunk. Without retaining it, the chunk drops the instant
/// `jit_lower_inlined_arm` returns, and every baked `cv` pointer dangles → `const_load`
/// reads freed memory → garbage constants fed into still-installed native code (the
/// JIT-inlined-throw corruption: `(error "bottom")` whose "bottom" const came out as a raw
/// stack pointer). Process-lifetime, like the native code in `GLOBAL_JIT`.
///
/// The **leaf** path needs no entry here (ADR-210): its spliced chunk lives on the
/// derivation's resume arm, which `arm` owns, and `JIT_ARM_KEEPALIVE` already retains `arm`
/// on every successful lowering — so the same guarantee falls out of the existing contract,
/// and the chunk is compiled once instead of at every lowering.
#[cfg(feature = "jit")]
static JIT_INLINE_CHUNK_KEEPALIVE: std::sync::Mutex<Vec<(Box<Node>, Box<Chunk>)>> =
    std::sync::Mutex::new(Vec::new());

/// Lower the **inlined** (deferred upgrade) body of a qualifying recursive arm. Re-derives
/// the spliced body fresh from `arm.body` (the small original — the VM keeps it), compiles
/// an ephemeral chunk, and lowers it against the larger `arm.inline_nslots` frame. Returns
/// the inlined native pointer, or `None` if the spliced body falls out of the JIT subset.
/// Per-engine frame sizing (`frame_size_for_new_entry`) keys on which version `jit_tier` installs.
///
/// On success the spliced `Node` + `Chunk` are moved into [`JIT_INLINE_CHUNK_KEEPALIVE`]
/// so the `ConstVal` addresses baked into the native code never dangle (see that static).
#[cfg(feature = "jit")]
pub(crate) fn jit_lower_inlined_arm(
    jit: &mut crate::jit::CraneliftBackend,
    arm: &CompiledArm,
    slot_tags: &[u8],
) -> Option<*const u8> {
    if let Some(leaf) = &arm.leaf {
        // Leaf-callee upgrade: the stored derivation is valid ONLY at the epoch it was
        // derived at — a `def`/compaction since then may have rebound a spliced callee
        // (or a prim its body uses), and the derivation can't be re-checked here (no
        // heap on this thread). Refuse → the upgrade BAILs and the small native keeps
        // running; the caller re-derives fresh only when its closure is recompiled.
        // Behind, not merely different: a native entry that finds the arm still right moves
        // both stamps forward, the derivation's first (KI-303). Read in the other order, the
        // arm's stamp first, a move landing between the reads leaves the derivation ahead.
        let compiled_at = arm.compile_epoch.load(std::sync::atomic::Ordering::Acquire);
        if leaf.epoch.load(std::sync::atomic::Ordering::Acquire) < compiled_at {
            return bail("leaf-derivation-stale");
        }
        // The spliced body + chunk live on the resume arm, which `arm` owns and
        // `JIT_ARM_KEEPALIVE` retains for the process lifetime — so the `ConstVal`
        // addresses baked in below can never dangle, and this path needs no
        // `JIT_INLINE_CHUNK_KEEPALIVE` entry of its own. Lowering journals against the
        // resume arm's `ckpt_slot` (the spliced layout's own), which is what lets the
        // derivation keep a residual non-tail call.
        let r = &leaf.resume;
        // The lowering's frame size MUST be the size the frame is actually built to
        // (`frame_size_for_new_entry()` → `inline_nslots`): the native stages a tail call above its
        // own frame top and the dispatcher reads it at `base + frame_size_for_new_entry()`, so a
        // disagreement writes and reads different offsets. `compile_arm` floors both to
        // the same value.
        debug_assert_eq!(
            r.nslots, arm.inline_nslots,
            "leaf resume arm frame size must equal the arm's inline_nslots"
        );
        return jit_lower_arm_inner(
            jit,
            arm,
            slot_tags,
            Some((
                &r.body,
                r.chunk.as_ref().or_bail("arm-has-no-chunk")?,
                arm.inline_nslots,
                r.ckpt_slot,
            )),
            true,
        );
    }
    // Self-inlining: the body is re-derived fresh here, so box it — its heap address
    // (and the `ConstVal`s inside the chunk) must stay stable once baked into the native
    // code, which the keepalive below guarantees. This layout never journals: its ips
    // don't match any chunk the VM holds, so a deopt re-runs the small body from ip 0
    // (effect-free — the self-inline gate admits only pure-arith bodies).
    let name = arm.inline_name.or_bail("no-inline-name")?;
    let spliced: Box<Node> = Box::new(rederive_inlined_body(
        &arm.body,
        name,
        arm.nrequired,
        arm.inline_stride,
    )?);
    let chunk: Box<Chunk> =
        Box::new(compile_chunk(&spliced).or_bail("splice-chunk-not-compilable")?);
    let ptr = jit_lower_arm_inner(
        jit,
        arm,
        slot_tags,
        Some((&spliced, &chunk, arm.inline_nslots, u32::MAX)),
        true,
    )?;
    // Lowering succeeded and baked raw `cv` pointers into the chunk — retain it forever.
    JIT_INLINE_CHUNK_KEEPALIVE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push((spliced, chunk));
    Some(ptr)
}

/// Shared lowering core. `inline` overrides the body/chunk/nslots **and the checkpoint
/// slot** when lowering an inlined body; `None` lowers the arm's own (original) body — the
/// small native, which journals against `arm.ckpt_slot`.
///
/// The checkpoint override is what separates the two inlined engines: the leaf splice
/// passes its own slot (above the spliced blocks) and so journals, while the self-splice
/// passes `u32::MAX` and so does not — see [`jit_lower_inlined_arm`].
#[cfg(feature = "jit")]
fn jit_lower_arm_inner(
    jit: &mut crate::jit::CraneliftBackend,
    arm: &CompiledArm,
    slot_tags: &[u8],
    inline: Option<(&Node, &Chunk, usize, u32)>,
    hot: bool,
) -> Option<*const u8> {
    use cranelift_codegen::ir::{
        AbiParam, BlockArg, InstBuilder, StackSlotData, StackSlotKind, types,
    };
    use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
    use cranelift_module::{Linkage, Module};
    use std::sync::atomic::Ordering;

    // A stale reason from a previous arm's refusal must not attach to this arm's decline.
    LAST_MID_EMIT_REASON.set(None);

    // The body/chunk/frame-size this lowering runs against: either the arm's own
    // (original, small — the small native) or a re-derived inlined body (deferred upgrade).
    // `nrequired` is identical for both (inlining doesn't change the param count).
    let (lower_body, chunk, nslots, ckpt_slot): (&Node, &Chunk, usize, u32) = match inline {
        Some((b, c, ns, cs)) => (b, c, ns, cs),
        None => (
            &arm.body,
            arm.chunk.as_ref().or_bail("arm-has-no-chunk")?,
            arm.nslots,
            arm.ckpt_slot,
        ),
    };
    let nrequired = arm.nrequired;
    let code = &chunk.code;
    let len = code.len();
    let hoist_plan = entry::plan_hoists(code, lower_body, nrequired);
    // Per-slot "holds an f64" flag, for picking float vs integer arith on each op.
    // Seeded from the tier-time profile (params; `slot_tags[k] == TAG_FLOAT`) and
    // updated during lowering when a float result is stored to a slot (let-binders,
    // which read nil at the entry snapshot). For a pure-int arm every entry is false,
    // so the lowering takes the exact pre-float integer path (no behaviour change).
    // The per-read tag-check in `as_f64`/`as_int` is what makes it *sound* (a slot whose
    // runtime tag disagrees deopts to the VM); this flag only chooses the opcode.
    // NB: the profile snapshots the *lattice* `Tag` enum (`tag(v) as u8`), whose
    // `Float` discriminant is 3 — distinct from `jit_layout::TAG_FLOAT` (4), the
    // in-memory `Value` discriminant byte used when boxing/reading floats. Compare
    // the profile against `Tag::Float`, not the layout byte.
    let profile_tag_float = crate::core::value::Tag::Float as u8;
    let slot_float: std::cell::RefCell<Vec<bool>> = std::cell::RefCell::new(
        (0..nslots)
            .map(|k| slot_tags.get(k).copied() == Some(profile_tag_float))
            .collect(),
    );
    // Is this arm float-context (any profiled Float param)? Used to optimistically route
    // arithmetic on an `Op::Handle` operand (a type-erased vector/`nth` read — nbody's
    // `(- (nth bi 0) (nth bj 0))`) through the *float* path instead of the integer default.
    // `as_f64` tag-checks the handle is `Float` and deopts otherwise, so this can never
    // miscompile — a wrong guess just deopts (the same outcome as today's int-path
    // `as_int`-on-a-float). When the guess is right the result is `Op::Float`, which
    // `store_op` marks float, so the whole `(nth …)`-fed arithmetic chain stays unboxed.
    // Or the arm's deopt feedback found it computing on floats that no param carried
    // (`jit_float_context`: integer guards deopting on `Float`s read out of vectors).
    let has_float_slot = slot_tags.contains(&profile_tag_float)
        || arm
            .jit_float_context
            .load(std::sync::atomic::Ordering::Relaxed);
    // Free globals observed holding a `Value::Float` when this arm was elected for
    // tiering — the global-read counterpart of the `slot_tags` param profile. Empty when
    // unset (an arm lowered through a path that had no `Heap`).
    let float_globals: &[Symbol] = arm
        .float_globals
        .get()
        .map(|b| &**b)
        .unwrap_or(&[] as &[Symbol]);
    // Per-slot "holds a `Value::Bool`" flag — the boolean analogue of `slot_float`, but
    // seeded all-false: a bool is rarely a loop *param*, and the case that matters is a
    // let-binder, e.g. `(and X Y)` → `(let (g X) (if g Y g))` storing a comparison result
    // to `g`. Set only by an in-arm bool store (`store_op` → `set_slot_bool`), which
    // dominates the slot's reads in the single lowering pass — so a slot marked bool here
    // provably holds a `Value::Bool` and needs no per-read tag-check. This lets a bool
    // carried through a block-param merge (an `(and …)`/`(or …)` returning its bound
    // operand) be tagged `Op::Bool` on *every* predecessor edge; without it the merge param
    // is `Op::Int` on the slot edge and a `0` (false) reads as a truthy integer (5770),
    // looping forever on a condition that should exit.
    let slot_bool: std::cell::RefCell<Vec<bool>> = std::cell::RefCell::new(vec![false; nslots]);
    // Per-slot F64 SSA value cache. When `store_op` writes `Op::Float(v)` to slot `dst`,
    // we stash `v` here. A subsequent `as_f64(Op::Slot(k))` can return `v` directly —
    // no tag-check, no memory load, just the SSA value already in a register. The cache
    // is cleared on non-Float stores and propagated verbatim on slot-copies. Carry-var
    // slots are served by `use_var` before we reach this cache; the cache covers let-bound
    // floats (e.g. `nx`/`ny` in mandelbrot's `esc`) where the tag-checks for `nx*nx` and
    // `ny*ny` would otherwise reload from memory and branch twice per read.
    let slot_f64_cache: std::cell::RefCell<Vec<Option<cranelift_codegen::ir::Value>>> =
        std::cell::RefCell::new(vec![None; nslots]);
    // Its integer twin (`Frame::slot_i64_cache`): a read of a just-stored int slot returns the
    // SSA value instead of reloading and re-tag-checking the slot.
    let slot_i64_cache: std::cell::RefCell<Vec<Option<cranelift_codegen::ir::Value>>> =
        std::cell::RefCell::new(vec![None; nslots]);
    // Handle-spill scratch: `[spill_base, spill_base + reserve)` are the frame slots
    // reserved (above the compiler's slot ceiling) for spilling call-result handles
    // that must survive a later call's safepoint. `reserve` matches what arm
    // construction added to `nslots`, so `spill_base` is exactly the old `scope.max`.
    let reserve = jit_spill_reserve(code);
    // Frame layout: [locals | spill slots | ckpt slot + journal]. The checkpoint
    // area (deopt-resume, `CompiledArm::ckpt_slot`) sits ABOVE the spills, so the
    // spill base is measured from the checkpoint start when one is reserved.
    // Measured from THIS lowering's checkpoint start (`ckpt_slot`), not the arm's: a
    // leaf-spliced layout has its own checkpoint area at its own frame top, while a
    // self-spliced one has none (`u32::MAX`) and so measures from the full frame top.
    let frame_top_for_spills = if ckpt_slot != u32::MAX {
        ckpt_slot as usize
    } else {
        nslots
    };
    // KI-49: the reserve is [call-result spills | block-argument spills]. The block-arg
    // half is indexed by operand-stack POSITION (so predecessors agree), so it gets its own
    // base rather than sharing `spill_next`'s monotonic counter. The window — and the clamp
    // that keeps it off the checkpoint journal (KI-64) — is
    // `jit_plan::blockarg_spill_window`, with the rest of the frame-layout decisions.
    let spill_base = frame_top_for_spills - reserve;
    let (blockarg_spill_base, blockarg_spill_len) =
        crate::eval::compile::jit_plan::blockarg_spill_window(code, reserve, frame_top_for_spills);
    let mut spill_next = 0usize;
    // Return-via-roots writes/reads the result at `roots[base]` (slot 0), and the VM hooks
    // read it back the same way — both require slot 0 to exist. A 0-slot arm (a 0-arg,
    // 0-local fn like `(defn k () 7)`) has `base == roots_len`, so `roots[base]` is out of
    // bounds. Such arms are trivial; bail and let the VM run them.
    if nslots == 0 {
        return bail("zero-slot-arm");
    }

    // ---- Pre-bail on any out-of-subset instruction (so we never half-build) ----
    // The accepted subset is `chunk_in_jit_subset` (the single source of truth, shared
    // with `jit_spill_reserve`): Const(Int), Local, Jump, JumpIfFalse, SelfCall, Pop,
    // SetLocal, Global/GlobalIc (resolved live by the global callbacks), Prim1
    // (`first`/`rest`), Call (linked / dispatched), and Prim2{,SlotSlot,SlotInt} on an
    // in-subset op. The fused `Prim2Slot*` variants are what `emit_node` produces for the
    // common `(- i 1)` / `(+ acc i)` loop body, so lowering them is what makes the JIT
    // fire on real compiled code.
    if !chunk_in_jit_subset(code) {
        return bail("chunk-outside-jit-subset");
    }

    // KI-64 backstop, checked AFTER the subset pre-bail so it only judges arms that can
    // actually lower. The spill area must FIT under the checkpoint — or under the frame
    // top when this lowering does not journal — because a block-argument write landing on
    // the journal makes a later read return the packed journal word as a live value, and
    // one landing past the frame top corrupts whatever is above it. The clamp above makes
    // this unreachable by construction; it stays because *nothing* checked before KI-64,
    // which is why a two-line arithmetic slip cost a production service months of
    // "restart it and it comes back". Bail rather than corrupt: the arm runs on the VM.
    if blockarg_spill_base + blockarg_spill_len > frame_top_for_spills {
        return bail("spill-area-exceeds-frame");
    }

    if !prepass::tail_call_work_is_enough(code) {
        return bail("tail-call-below-min-work");
    }

    let (is_leader, depth, unmodelled) = prepass::block_analysis_checked(code, len);
    if let Some(name) = unmodelled {
        // The subset admitted an instruction the depth model does not know: every leader
        // past it would be sized wrong. Refuse by name (see `block_analysis_checked`).
        record_mid_emit_detail("prepass-unmodelled-inst", name);
        return None;
    }

    let m = jit.module();
    let ptr_ty = m.target_config().pointer_type();
    let mut sig = m.make_signature();
    sig.params.push(AbiParam::new(ptr_ty)); // heap
    sig.params.push(AbiParam::new(types::I64)); // base (frame index into roots)
    // `out`: where the Done result goes — the slot its ultimate consumer already owns, so the
    // callee stores it once and nobody reads it back (docs/compute-frontier.md §2h).
    sig.params.push(AbiParam::new(ptr_ty)); // out: *mut Value (Done result)
    sig.params.push(AbiParam::new(ptr_ty)); // ctx: *const JitCallCtx (rung A0: unread)
    sig.returns.push(AbiParam::new(types::I64)); // outcome: 0 = Done, 1 = deopt, 2 = preempt
    let seq = JIT_ARM_SEQ.fetch_add(1, Ordering::Relaxed);
    let id = m
        .declare_function(&format!("brood_jit_arm_{seq}"), Linkage::Export, &sig)
        .ok()
        .or_bail("cranelift-declare-function")?;
    let callback_ids = callbacks::declare_callbacks(m, ptr_ty)?;

    let mut ctx = m.make_context();
    ctx.func.signature = sig;
    let mut fbctx = FunctionBuilderContext::new();
    let mut b = FunctionBuilder::new(&mut ctx.func, &mut fbctx);
    let carry_vars = entry::plan_carry_vars(&mut b, code, slot_tags);
    let callbacks = callbacks::import_callbacks(m, &callback_ids, &mut b);
    // Whether the arm allocates (`cons`) — gates the back-edge GC safepoint that bounds
    // the nursery. (`car`/`rest` don't allocate.)
    let has_cons = code.iter().any(inst_allocates_hot);

    // One Cranelift block per leader (with `depth` I64 params), plus entry/deopt. The
    // Done block (`ip == len`) takes **no** params: the result is returned through the
    // caller's `out` pointer (each exit stores it there), so it can be a handle, not just
    // an `i64` block arg. Every other block carries its operand-stack depth as I64 params.
    let leader_block: Vec<Option<cranelift_codegen::ir::Block>> = (0..=len)
        .map(|ip| {
            if is_leader[ip] {
                let blk = b.create_block();
                let nparams = if ip == len { 0 } else { depth[ip].unwrap_or(0) };
                for _ in 0..nparams {
                    b.append_block_param(blk, types::I64);
                }
                Some(blk)
            } else {
                None
            }
        })
        .collect();
    let entry = b.create_block();
    let deopt = b.create_block();
    // KI-49: the shared deopt block carries a reason id, so each guard names itself.
    b.append_block_param(deopt, types::I32);
    let preempt = b.create_block();
    // The error exit (outcome 3): a JIT'd call / global read raised an error (parked in
    // `JIT_PENDING_ERROR`). `vm_run_bc` takes it and propagates — unlike `deopt`, it does
    // **not** re-run the arm on the VM (which would repeat the call).
    let error = b.create_block();
    // The tail-call exit (outcome 4): a JIT'd arm ending in a **tail** call stages the
    // callee + args on `roots` (above the frame top) and returns here. `vm_run_bc` reads
    // the staged operands, dispatches the callee with `tail = true`, and reuses this
    // frame for it (TCO) — never growing the native stack. Only conditionally reached
    // (an arm with no tail call leaves it dead, DCE'd), like `deopt`/`preempt`/`error`.
    let tailcall = b.create_block();
    b.append_block_params_for_function_params(entry);
    b.switch_to_block(entry);
    let heap = b.block_params(entry)[0];
    let base = b.block_params(entry)[1];
    let out_ptr = b.block_params(entry)[2];
    // The activation context (`JitCallCtx`, rungs A0/A1): every runtime callback that
    // consults activation state takes it and asserts it into the heap fields first
    // (`JitCallCtx::assert_into`), so the inline call path writes none of those fields.
    let ctx_ptr = b.block_params(entry)[3];
    // `roots_base` is a **Variable**, not a fixed SSA value: a Brood→Brood call's staging
    // pushes (and the callee's own frames) may reallocate `roots`, so the base is re-fetched
    // after each call (`def_var` below). For a call-free arm it keeps its single entry
    // definition (no phi, no reload) — the int/cons subset is unaffected. Helpers read it
    // via `b.use_var(rb_var)`.
    let rb_var = b.declare_var(ptr_ty);
    let rb0 = emit::load_roots_base(&mut b, heap);
    b.def_var(rb_var, rb0);

    entry::emit_nil_locals(&mut b, arm, nslots, base, rb0);
    // KI-49: which slots the tier-time profile saw an `Int` in. A profiled-Int slot keeps
    // the unboxed i64 block-arg carry (`fib`/`collatz`); every other slot crosses a block
    // boundary as `ParamRepr::Slot`, so it is never forced through `as_int`.
    // (The profile holds lattice `Tag` bytes, not layout bytes — see `slot_float` above.)
    let profile_tag_int = crate::core::value::Tag::Int as u8;
    let slot_int_profile: Vec<bool> = (0..nslots)
        .map(|i| slot_tags.get(i).copied() == Some(profile_tag_int))
        .collect();
    let frame = emit::Frame {
        out_ptr,
        rb_var,
        base,
        nslots,
        deopt,
        carry_vars: &carry_vars,
        slot_float: &slot_float,
        slot_bool: &slot_bool,
        slot_int_profile: &slot_int_profile,
        float_context: has_float_slot,
        blockarg_spill_base,
        blockarg_spill_len,
        slot_f64_cache: &slot_f64_cache,
        slot_i64_cache: &slot_i64_cache,
    };
    // A scratch `Value`-sized stack slot the handle / call / global ops write their result
    // into (the out-pointer ABI). One per arm, reused: each result is read straight back
    // into registers before the next op.
    let out_slot = b.create_sized_stack_slot(StackSlotData::new(
        StackSlotKind::ExplicitSlot,
        std::mem::size_of::<Value>() as u32,
        3,
    ));

    entry::emit_stack_guard(&mut b, ptr_ty, heap, deopt);

    let epoch_ptr = entry::emit_epoch_pointer(&mut b, code, &hoist_plan, &callbacks, heap);
    let pair_bases = entry::emit_pair_bases(&mut b, code, &callbacks, heap, ptr_ty);
    let hoisted = entry::emit_entry_hoists(
        &mut b,
        code,
        &hoist_plan,
        &entry::EntryContext {
            callbacks: &callbacks,
            heap,
            base,
            roots_base_variable: rb_var,
            context_pointer: ctx_ptr,
            out_slot,
            pointer_type: ptr_ty,
            deopt,
        },
        epoch_ptr,
    );
    // The back-edge epoch guard (`emit_self_call`'s poll) compares against this. It used to
    // exist only when the arm hoisted a global, so a self-loop that hoisted none — calls go
    // through their own epoch-guarded links — never saw a `def` of ITSELF while it ran: a
    // receive loop that never emptied its mailbox drained 200 000 messages on its old body
    // (hot_reload_test, 2026-10-17), where the VM's `SelfCall` re-resolves the name. Every
    // self-loop snapshots the epoch at entry now; the guard is one load per tick batch.
    let entry_epoch = hoisted.entry_epoch.or_else(|| {
        let pointer = epoch_ptr?;
        code.iter()
            .any(|i| matches!(i, Inst::SelfCall { .. }))
            .then(|| {
                b.ins().load(
                    types::I64,
                    cranelift_codegen::ir::MemFlagsData::trusted(),
                    pointer,
                    0,
                )
            })
    });
    entry::emit_carry_init(&mut b, &carry_vars, rb_var, base, deopt);
    // Reduction batching for the self-tail loop: an in-register countdown (`TICK_BATCH`
    // iterations) gates the back edge's preemption poll + epoch guard — one sub+branch per
    // iteration. The poll settles the batch with `brood_rt_tick_n`, so scheduler fairness is
    // unchanged; a rebind's epoch bump is observed within one batch.
    let tick_budget = b.declare_var(types::I64);
    {
        let init = b.ins().iconst(types::I64, emit::TICK_BATCH);
        b.def_var(tick_budget, init);
    }
    // Deopt-resume checkpointing (see `CompiledArm::ckpt_slot`) is active whenever THIS
    // lowering has a journal slot: the small native and the leaf-spliced native (a deopt out
    // of it resumes in the spliced chunk via `ir::LeafInline::resume`). The self-spliced
    // native passes `u32::MAX` and re-runs from ip 0: its ips match no chunk the VM holds,
    // and its gate admits only pure-arith bodies, so re-running is effect-free.
    let ckpt_active = ckpt_slot != u32::MAX;
    // The self-inlined upgrade (an inlined body with no leaf derivation — the rule
    // `jit_frame_layout` reads back): its ips match no chunk the VM holds, so it cannot be
    // rebuilt mid-call, and the small layout's journal index is a live slot of one of its
    // spliced blocks, so it resets neither journal (`FrameLayout::SelfSpliced`).
    let self_spliced = inline.is_some() && arm.leaf.is_none();
    let resets = if self_spliced {
        [u32::MAX, u32::MAX]
    } else {
        [ckpt_slot, arm.ckpt_slot]
    };
    entry::emit_checkpoint_reset(&mut b, resets, rb_var, base);
    // ADR-401: an activation that makes non-tail calls (the only kind that can recurse
    // natively) and whose layout the driver can rebuild mid-call — every one but the
    // self-spliced upgrade. Its calls' error paths write the in-call journal, so the
    // activations it calls may preempt by unwinding; it polls reductions at entry.
    let unwindable = !self_spliced
        && code
            .iter()
            .any(|i| matches!(i, Inst::Call { tail: false, .. }));
    // The activations this one calls may preempt by unwinding only if this one may, and
    // only if it can be rebuilt mid-call. Only the self-spliced layout makes calls and
    // cannot: it says so, for the runtime's call paths (its own native call sites pass 0).
    if self_spliced {
        let pinned = b.ins().iconst(types::I8, 1);
        b.ins().store(
            cranelift_codegen::ir::MemFlagsData::trusted(),
            pinned,
            ctx_ptr,
            std::mem::offset_of!(crate::jit::JitCallCtx, no_child_unwind) as i32,
        );
    }
    // ADR-401: the entry reduction poll. A native recursion has no back edge, so nothing
    // in it ever settled reductions with the scheduler: a `bintree`-style walk ran to the
    // end in one quantum however deep it went. An `unwindable` arm now charges one
    // reduction per activation from a countdown on the heap, settling a batch with
    // `brood_rt_tick_n` as the back edge does — which preempts only an activation whose
    // context says it may unwind, so the countdown needs no branch on that bit of its own
    // (it was two of the poll's instructions on every activation of `bintree`). A preempt
    // here leaves the frame exactly as entered (ip 0, the arguments in their slots, the
    // journal just reset), so a resume re-runs this activation from its start, with
    // nothing of it executed yet.
    if unwindable {
        use cranelift_codegen::ir::condcodes::IntCC;
        let trusted = cranelift_codegen::ir::MemFlagsData::trusted();
        let body = b.create_block();
        let budget_offset = crate::core::heap::jit_ceremony_offsets().jit_call_budget as i32;
        let left = b.ins().load(types::I32, trusted, heap, budget_offset);
        let left = b.ins().iadd_imm_s(left, -1);
        b.ins().store(trusted, left, heap, budget_offset);
        let settle = b.create_block();
        let more = b.ins().icmp_imm_s(IntCC::SignedGreaterThan, left, 0);
        b.ins().brif(more, body, &[], settle, &[]);
        b.switch_to_block(settle);
        b.set_cold_block(settle);
        let refill = b.ins().iconst(types::I32, emit::TICK_BATCH);
        b.ins().store(trusted, refill, heap, budget_offset);
        let batch = b.ins().iconst(types::I64, emit::TICK_BATCH);
        let tick = b.ins().call(callbacks.tick_n, &[heap, batch, ctx_ptr]);
        let outcome = b.inst_results(tick)[0];
        let leave = b.create_block();
        b.ins().brif(outcome, leave, &[], body, &[]);
        b.switch_to_block(leave);
        b.set_cold_block(leave);
        let is_limit = b.ins().icmp_imm_s(IntCC::Equal, outcome, 2);
        let reason = emit::deopt_code(&mut b, DeoptReason::EntryLimitPending);
        b.ins()
            .brif(is_limit, deopt, &[BlockArg::Value(reason)], preempt, &[]);
        b.switch_to_block(body);
    }
    b.ins().jump(leader_block[0].unwrap(), &[]);

    let done_block = leader_block[len].or_bail("jump-target-not-a-leader")?;
    let exits = finish::ExitBlocks {
        done: done_block,
        deopt,
        preempt,
        error,
        tailcall,
    };
    let global_reads = load::GlobalReadContext {
        hoisted: &hoisted,
        float_globals,
        global: callbacks.global,
        global_ic: callbacks.global_ic,
        heap,
        context_pointer: ctx_ptr,
        out_slot,
        pointer_type: ptr_ty,
        error,
    };
    let funcs = emit::Funcs {
        ptr_ty,
        heap,
        out_slot,
        error,
        vnbase: callbacks.vector_nursery_base,
        vobase: callbacks.vector_old_base,
        vref: callbacks.vector_ref,
        car: callbacks.car,
        first: callbacks.first,
        rest: callbacks.rest,
        is_empty: callbacks.is_empty,
        pnbase: callbacks.pair_nursery_base,
        pobase: callbacks.pair_old_base,
        cdr: callbacks.cdr,
        cons: callbacks.cons,
        vec2room: callbacks.vector2_room,
        makevecn: callbacks.make_vector_n,
        makemapn: callbacks.make_map_n,
        thas: callbacks.table_has,
        tget: callbacks.table_get,
        mget: callbacks.map_get,
        vlen: callbacks.vector_len,
        count: callbacks.count,
        equal: callbacks.equal,
        tput: callbacks.table_put,
        mget3: callbacks.map_get3,
        vref3: callbacks.vector_ref3,
        massoc: callbacks.map_assoc,
        globic: callbacks.global_ic,
        pushroom: callbacks.push_room,
        incall: callbacks.in_call_journal,
        callslow: callbacks.call_slow,
        natfl: callbacks.call_native_flat,
        flbase: callbacks.fastlink_base,
        fastframe: callbacks.fast_frame,
        xlatch: callbacks.xcall_latch,
        xcold: callbacks.xcall_cold,
        armfn_sig: callbacks.arm_function,
        ctx: ctx_ptr,
        xcall: call::xcall_emit(hot),
        sp: callbacks.gc_safepoint,
        tickn: callbacks.tick_n,
        #[cfg(debug_assertions)]
        dbg_staging: callbacks.debug_set_staging,
    };

    // The not-yet-typed edges into each leader (`control::PendingEdge`): every branch
    // targets a fresh edge block and records its stack here; the target's translation
    // unifies the typings and fills the edge blocks (`control::resolve_edges`).
    let mut pending: Vec<Vec<PendingEdge>> = (0..=len).map(|_| Vec::new()).collect();

    // Translate each leader block in ip order.
    for ip in 0..len {
        let Some(blk) = leader_block[ip] else {
            continue;
        };
        // A leader the prepass never reached is DEAD: the bytecode compiler emits a
        // jump-past-the-`else` after a then-branch that ends in a tail `SelfCall`, and
        // nothing falls through a SelfCall. Emitting it anyway gave it an empty operand
        // stack and an edge into a live join of another depth — the verifier rejection
        // that kept `json/num-end` and `json/object-acc` off the native tier for as long
        // as they have existed. Terminate it as unreachable instead; if the assumption
        // were ever wrong it deopts to the VM rather than mis-executing.
        if ip != 0 && depth[ip].is_none() {
            pending[ip].clear();
            b.switch_to_block(blk);
            let deopt_reason = emit::deopt_code(&mut b, DeoptReason::UnreachableLeader);
            b.ins().jump(deopt, &[BlockArg::Value(deopt_reason)]);
            continue;
        }
        // Every predecessor of this leader has been emitted (edges are forward-only, in
        // ip order), so its block-param typing can be settled now.
        let edges = std::mem::take(&mut pending[ip]);
        let reprs = if edges.is_empty() {
            None
        } else {
            Some(
                control::resolve_edges(&mut b, ip, blk, edges, false, frame)
                    .or_bail("join-depth-or-widening")?,
            )
        };
        b.switch_to_block(blk);
        // The slot caches hold SSA values, which are only usable where their definition
        // dominates. A leader is a join or a branch target, and the block emitted just before
        // it in ip order may be a sibling branch that does not reach it: `&optional`'s
        // prologue stores the default in its then-branch, and the body's `(+ acc n)` after the
        // join read that constant on every path, so a loop over an optional accumulator
        // answered 1 for 5000050000 once it went native — given the argument or not.
        for cache in [frame.slot_i64_cache, frame.slot_f64_cache] {
            cache.borrow_mut().fill(None);
        }
        let mut stack = control::leader_entry_stack(&mut b, blk, reprs.as_deref());
        let mut j = ip;
        loop {
            match &code[j] {
                Inst::Const(constant) => load::emit_const(
                    &mut b,
                    &mut stack,
                    constant,
                    ptr_ty,
                    out_slot,
                    callbacks.const_load,
                ),
                // Lazy: push a reference to the frame slot. Consumers tag-check it to an int
                // (arithmetic / a branch) or copy it whole (a binder / arg / return), so a
                // handle in the slot rides along untouched.
                Inst::Local(i) => stack.push(Op::Slot(*i)),
                Inst::Global(symbol) => {
                    load::emit_global_read(&mut b, &mut stack, *symbol, None, &global_reads, frame)
                }
                Inst::GlobalIc { sym, site } => load::emit_global_read(
                    &mut b,
                    &mut stack,
                    *sym,
                    Some(*site),
                    &global_reads,
                    frame,
                ),
                Inst::Call {
                    argc,
                    tail,
                    site,
                    head,
                    staged,
                    pos,
                } => {
                    // KI-19: a staged head is already on the operand stack, resolved before
                    // the args. The JIT must consume it and call *that* value — re-resolving
                    // would observe a `def` an argument performed. That is exactly the
                    // computed-callee shape it already handles, so hand it over as one
                    // (`head: None`, `site: NO_SITE`); the in-IR fast link, which is keyed on
                    // an elided head symbol, does not apply to these calls.
                    let (head, site) = if *staged {
                        (&None, &NO_SITE)
                    } else {
                        (head, site)
                    };
                    // Where a tail call by name resolves its head (see `tail_head_gsite`).
                    let head_gsite = if *tail && head.is_some() {
                        arm.tail_head_gsite(*site)
                    } else {
                        0
                    };
                    match call::emit_call(
                        &mut b,
                        &mut stack,
                        &mut spill_next,
                        *argc,
                        *tail,
                        *site,
                        *head,
                        match pos {
                            Some(p) => arm.local_call_head(*p),
                            None => Some(crate::eval::POSITIONLESS_CALL),
                        },
                        head_gsite,
                        spill_base,
                        reserve,
                        epoch_ptr,
                        tailcall,
                        unwindable.then_some(j),
                        frame,
                        funcs,
                    )? {
                        call::Flow::Break => break,
                        call::Flow::Fall => {}
                    }
                }
                Inst::Pop => {
                    // A non-final `do` form, evaluated for effect: drop its value.
                    stack.pop().or_bail("operand-stack-underflow")?;
                }
                Inst::SetLocal(slot) => load::emit_set_local(&mut b, &mut stack, *slot, frame)?,
                Inst::Prim1 { op, .. } => {
                    prim::emit_prim1(&mut b, &mut stack, op, pair_bases, frame, funcs)?;
                }
                Inst::MakeVector(n) => {
                    prim::emit_make_vector(&mut b, &mut stack, *n, frame, funcs)?;
                }
                Inst::MakeMap(n) => {
                    prim::emit_make_map(&mut b, &mut stack, *n, frame, funcs)?;
                }
                Inst::Prim2 { op, map, .. } => {
                    prim::emit_prim2(&mut b, &mut stack, op, *map, has_float_slot, frame, funcs)?;
                }
                Inst::Prim3 {
                    op: PrimOp3::TablePut,
                    ..
                } => {
                    prim::emit_prim3_table_put(&mut b, &mut stack, frame, funcs)?;
                }
                // The two map ops (ADR-368): one callback each, status 1 deopts to the VM,
                // which owns every branch the inline rule declines.
                Inst::Prim3 {
                    op: PrimOp3::MapGet3,
                    ..
                } => {
                    let reason = DeoptReason::MapGet3Declined;
                    prim::emit_prim3_callback(
                        &mut b,
                        &mut stack,
                        frame,
                        funcs,
                        funcs.mget3,
                        reason,
                    )?;
                }
                Inst::Prim3 {
                    op: PrimOp3::VectorRefOr,
                    ..
                } => {
                    let reason = DeoptReason::VectorRef3Declined;
                    prim::emit_prim3_callback(
                        &mut b,
                        &mut stack,
                        frame,
                        funcs,
                        funcs.vref3,
                        reason,
                    )?;
                }
                Inst::Prim3 {
                    op: PrimOp3::MapAssoc,
                    ..
                } => {
                    let reason = DeoptReason::MapAssocDeclined;
                    prim::emit_prim3_callback(
                        &mut b,
                        &mut stack,
                        frame,
                        funcs,
                        funcs.massoc,
                        reason,
                    )?;
                }
                Inst::Prim2SlotSlot {
                    op,
                    map,
                    slot_a,
                    slot_b,
                    ..
                } => {
                    prim::emit_prim2_slot_slot(
                        &mut b,
                        &mut stack,
                        op,
                        *map,
                        *slot_a,
                        *slot_b,
                        &hoisted.slots,
                        frame,
                        funcs,
                    )?;
                }
                Inst::Prim2SlotInt {
                    op,
                    map,
                    slot_a,
                    int_b,
                    ..
                } => {
                    prim::emit_prim2_slot_int(
                        &mut b, &mut stack, op, *map, *slot_a, *int_b, frame, funcs,
                    )?;
                }
                Inst::Jump(t) => {
                    control::emit_jump(
                        &mut b,
                        &stack,
                        *t,
                        j,
                        len,
                        done_block,
                        &mut pending,
                        frame,
                    )?;
                    break;
                }
                Inst::SelfCall { argc } => {
                    call::emit_self_call(
                        &mut b,
                        &mut stack,
                        *argc,
                        &leader_block,
                        preempt,
                        tick_budget,
                        entry_epoch,
                        epoch_ptr,
                        has_cons,
                        ckpt_active,
                        ckpt_slot,
                        frame,
                        funcs,
                    )?;
                    break;
                }
                Inst::JumpIfFalse(t) => {
                    control::emit_jump_if_false(&mut b, &mut stack, *t, j, &mut pending, frame)?;
                    break;
                }
                other => {
                    // The subset rule admits an opcode class and emit refused this shape of
                    // it: name the opcode, or the disagreement is invisible.
                    record_mid_emit_detail("emit-unsupported-inst", inst_opcode_name(other));
                    return None;
                }
            }
            if ckpt_active
                && matches!(
                    &code[j],
                    Inst::Call { tail: false, .. }
                        | Inst::Prim3 {
                            op: PrimOp3::TablePut,
                            ..
                        }
                )
            {
                load::emit_checkpoint_journal(&mut b, &stack, j, ckpt_slot, frame);
            }
            j += 1;
            if j == len {
                // Fall off the end into Done: the single result goes through the caller's
                // `out` pointer (`emit::store_result`), then the param-less Done block.
                if stack.len() != 1 {
                    return bail("done-with-nonunit-operand-stack");
                }
                emit::store_result(&mut b, stack[0], out_ptr, frame);
                b.ins().jump(done_block, &[]);
                break;
            }
            if is_leader[j] {
                // Fall through into the next leader: a deferred edge like any other, so
                // the join is typed with every predecessor in hand.
                let e = control::defer_edge(&mut b, &stack, j, j - 1, &mut pending, frame)
                    .or_bail("backward-jump")?;
                b.ins().jump(e, &[]);
                break;
            }
        }
    }

    // Edges into Done that were deferred (a `JumpIfFalse` whose else is the end of the
    // arm): each stores its single result through `out` and jumps here.
    let done_edges = std::mem::take(&mut pending[len]);
    control::resolve_edges(&mut b, len, done_block, done_edges, true, frame)
        .or_bail("join-depth-or-widening")?;
    finish::emit_exit_blocks(
        &mut b,
        &exits,
        arm,
        heap,
        callbacks.note_deopt,
        callbacks.trace_push,
    );
    b.seal_all_blocks();
    b.finalize(m.target_config());

    finish::dump_ir(&ctx, code, arm, ckpt_slot);
    finish::define_and_finalize(m, id, &mut ctx, arm, code, inline.is_some())
}
