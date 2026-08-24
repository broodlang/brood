use super::*;

// The backend-independent decisions this lowering consults — `crate::jit::backend`'s
// obligation 1: a backend does not decide *whether* to lower, it carries out a decision made
// above it. The child modules under `jit_lower/` reach these through their own `use super::*`.
use super::jit_plan::chunk_in_jit_subset;
use super::jit_plan::codegen::{
    inst_allocates_hot, inst_may_allocate, inst_opcode_name, invariant_global_vecs,
    invariant_param_slots, jit_dump_ir_enabled, jit_i64_enabled, plan_general_lowering,
};

// The unboxed scalar (i64/f64) register worker lives in a child module; it's used
// only by the `jit_lower_arm` dispatcher below. Re-export the items the tiering glue
// (`jit_runtime`) and the dispatcher reach by `jit_lower::…` so those paths are
// unchanged. All jit-gated (the whole cluster is `#[cfg(feature = "jit")]`).
#[cfg(feature = "jit")]
mod i64;
#[cfg(feature = "jit")]
use i64::jit_lower_i64_arm;
#[cfg(feature = "jit")]
// Re-exported for `jit::cranelift`'s `JitBackend` tiering advisories — these are how the
// backend answers "have I demoted this fn off the register worker?" and "record that I must".
// The tiering glue reaches them through the trait, never directly.
pub(crate) use i64::{arm_i64_eligible, arm_i64_too_deep, i64_mark_too_deep};

// Pure pre-lowering analysis (block leaders / operand depth / …) for
// `jit_lower_arm_inner` — the first extracted step of decomposing that function.
#[cfg(feature = "jit")]
pub(crate) mod prepass;

// CLIF emit helpers extracted from `jit_lower_arm_inner`'s closures (the next step
// of that function's decomposition). They take `b: &mut FunctionBuilder` + the
// `deopt` block and produce SSA/`Op` results.
#[cfg(feature = "jit")]
mod emit;
#[cfg(feature = "jit")]
use emit::store_int;

// Control-flow arm bodies (`Jump` / `JumpIfFalse`) + the block-param edge-typing
// helper, extracted from the emit loop (the per-`Inst` arm-body decomposition step).
#[cfg(feature = "jit")]
mod control;
#[cfg(feature = "jit")]
use control::record_block_flags;

// Primitive-op arm bodies (`Prim1` / `MakeVector` / `Prim3` / the fused
// `Prim2`/`Prim2SlotSlot`/`Prim2SlotInt`), extracted from the emit loop.
#[cfg(feature = "jit")]
mod prim;

// Call arm bodies (`Call` — tail/non-tail with the fast link — and `SelfCall`),
// extracted from the emit loop.
#[cfg(feature = "jit")]
mod call;

/// The virtualized operand-stack element for `jit_lower_arm_inner`'s emit loop —
/// module scope (not a fn-local enum) so the extracted emit helpers can name it. A
/// logical operand is an unboxed `Int`/`Float`/`Bool` SSA value, a frame `Slot`, a
/// `Handle` (three `Value` words), or a hoisted invariant global vector/table.
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

// ===================== JIT lowering (ADR-101 Stage 1) =====================
//
// Lower a chunked arm to native code via Cranelift, co-located here because it reads
// the private `Inst`/`Chunk` bytecode. Stage-1 Step A: the **straight-line int subset**
// — `Const`(Int), `Local`, `Prim2`(Add/Sub/Mul) — keeping operands in SSA registers
// (the operand stack is virtualised at compile time, so `roots` never grows) and
// touching `Heap::roots` only to read frame slots and box the result. Any other `Inst`
// (control flow, calls, non-int prims, globals) makes lowering **bail** (`None`) — the
// arm stays on the VM. Control flow + the self-loop + deopt come next.

#[cfg(feature = "jit")]
static JIT_ARM_SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Compile `arm`'s chunk to a native `extern "C" fn(heap: *mut Heap, base: i64) -> i64`
/// for the Step-A int subset, or `None` to bail to the VM. The compiled fn reads its
/// frame slots from `roots[base..]`, computes in registers, **boxes the result into
/// `roots[base]`**, and returns `0` (Done) or `1` (deopt — an operand wasn't an `Int`).
/// The returned pointer is valid for the life of `jit` (its module owns the code).
#[cfg(feature = "jit")]
/// Refusal from the per-instruction emit loop, naming the opcode that could not be
/// lowered. Same flag and line shape as [`trace_lower_bail`].
#[cfg(feature = "jit")]
fn trace_lower_bail_inst(arm: &CompiledArm, inst: &'static str) -> Option<*const u8> {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("BROOD_JIT_BAIL_TRACE").is_some()) {
        let name = arm
            .dbg_name
            .map(crate::core::value::symbol_name_ref)
            .unwrap_or("<closure>");
        eprintln!("[jit-bail] arm={name} reason=emit-unsupported-inst:{inst}");
    }
    None
}

/// Report a lowering refusal that happens BEFORE `plan_general_lowering`'s profitability
/// gate, under the same `BROOD_JIT_BAIL_TRACE=1` flag and the same `[jit-bail]` line shape
/// as `jit_plan`'s `trace_bail`, so one grep covers both.
///
/// These four sites used to return `None` silently, and the caller stores `BAILED` — so an
/// arm refused here was indistinguishable from one that was never hot. That cost real time
/// on 2026-08-20: the `receive` matcher for a tagged tuple (`[:ping x]`, the shape every
/// `gen`/supervisor protocol uses) is refused here, runs on the VM at 454 ns against 59 ns
/// for the natively-lowered keyword matcher, and the trace named nine unrelated prelude arms
/// and not this one. Absence of evidence read as absence of a refusal.
#[cfg(feature = "jit")]
fn trace_lower_bail(arm: &CompiledArm, reason: &'static str) -> Option<*const u8> {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("BROOD_JIT_BAIL_TRACE").is_some()) {
        let name = arm
            .dbg_name
            .map(crate::core::value::symbol_name_ref)
            .unwrap_or("<closure>");
        eprintln!("[jit-bail] arm={name} reason={reason}");
    }
    None
}

pub(crate) fn jit_lower_arm(
    jit: &mut crate::jit::CraneliftBackend,
    arm: &CompiledArm,
    slot_tags: &[u8],
) -> Option<*const u8> {
    // Unboxed-i64 fast path: an int-only single-arg recursive arm (`fib`) gets a register
    // calling convention for its self-recursion — args/results in registers, no boxing /
    // roots-staging / fast-link dispatch (the Increment-0 profile showed that protocol is
    // ~55% of `fib`'s time; this path is ~5× on `pfib`, beating Elixir). Falls through to the
    // general lowering when the arm isn't eligible.
    //
    // **Tried before the gate below, and the order is load-bearing.** The gate rejects the
    // boxed *general* lowering, and its predicate — a named defn with non-tail calls, no
    // inline vector op, no self-tail loop — describes `fib`/`pfib` exactly, i.e. the arms the
    // scalar worker wins biggest on. Consult the gate first and they silently stop lowering:
    // still *correct* (they run on the VM), so the JIT differential stays green and only a
    // benchmark notices. Keep the scalar attempt first.
    if jit_i64_enabled() {
        if let Some(p) = jit_lower_i64_arm(jit, arm) {
            return Some(p);
        }
    }
    // Whether the general lowering is worth doing at all is a backend-independent decision:
    // it lives in `jit_plan` with the measurement that justifies it, and a refusal is
    // reportable there (`BROOD_JIT_BAIL_TRACE=1`) instead of an unexplained `None` from here.
    plan_general_lowering(arm, slot_tags).ok()?;
    jit_lower_arm_inner(jit, arm, slot_tags, None)
}

/// Unbox a free-global read that was observed holding a `Value::Float` at tier time
/// ([`CompiledArm::float_globals`]) into an `Op::Float`, so arithmetic over it takes the
/// float path instead of the integer default.
///
/// Without this, an arm whose *parameters* carry no float — nbody's `advance-body (b i)`,
/// a vector and an int — is not float-context, so `(* dt nvx)` falls through
/// `emit_prim2`'s dispatch to `as_int`, whose tag-check deopts on the float `dt`. That
/// deopts on **every** activation, and sixteen in a row mark the arm `BAILED`, so the
/// hottest function in the row runs interpreted for the rest of the program.
///
/// Soundness is [`as_f64`]'s existing tag guard, not the tier-time observation: a global
/// that is no longer a float (a `def` since, or a different runtime sharing this arm's
/// code) fails the guard and deopts to the VM. A stale guess costs a deopt; it can never
/// miscompile. This is the same argument the `has_float_slot` optimism already rests on.
#[cfg(feature = "jit")]
fn unbox_float_global(
    b: &mut cranelift_frontend::FunctionBuilder,
    sym: Symbol,
    op: Op,
    frame: emit::Frame,
    float_globals: &[Symbol],
) -> Op {
    if float_globals.contains(&sym) {
        Op::Float(emit::as_f64(b, op, frame))
    } else {
        op
    }
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
        if arm.compile_epoch.load(std::sync::atomic::Ordering::Acquire) != leaf.epoch {
            return trace_lower_bail(arm, "leaf-derivation-stale");
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
            Some((&r.body, r.chunk.as_ref()?, arm.inline_nslots, r.ckpt_slot)),
        );
    }
    // Self-inlining: the body is re-derived fresh here, so box it — its heap address
    // (and the `ConstVal`s inside the chunk) must stay stable once baked into the native
    // code, which the keepalive below guarantees. This layout never journals: its ips
    // don't match any chunk the VM holds, so a deopt re-runs the small body from ip 0
    // (effect-free — the self-inline gate admits only pure-arith bodies).
    let name = arm.inline_name?;
    let spliced: Box<Node> = Box::new(rederive_inlined_body(
        &arm.body,
        name,
        arm.nrequired,
        arm.inline_stride,
    )?);
    let chunk: Box<Chunk> = Box::new(compile_chunk(&spliced)?);
    let ptr = jit_lower_arm_inner(
        jit,
        arm,
        slot_tags,
        Some((&spliced, &chunk, arm.inline_nslots, u32::MAX)),
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
) -> Option<*const u8> {
    use crate::core::value::jit_layout::{PAYLOAD_OFFSET, TAG_FLOAT, TAG_INT};
    use cranelift_codegen::ir::{
        condcodes::IntCC, types, AbiParam, BlockArg, InstBuilder, MemFlagsData, StackSlotData,
        StackSlotKind,
    };
    use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
    use cranelift_module::{Linkage, Module};
    use std::sync::atomic::Ordering;

    // The body/chunk/frame-size this lowering runs against: either the arm's own
    // (original, small — the small native) or a re-derived inlined body (deferred upgrade).
    // `nrequired` is identical for both (inlining doesn't change the param count).
    let (lower_body, chunk, nslots, ckpt_slot): (&Node, &Chunk, usize, u32) = match inline {
        Some((b, c, ns, cs)) => (b, c, ns, cs),
        None => (&arm.body, arm.chunk.as_ref()?, arm.nslots, arm.ckpt_slot),
    };
    let nrequired = arm.nrequired;
    let code = &chunk.code;
    let len = code.len();
    const STRIDE: i64 = std::mem::size_of::<Value>() as i64;
    // matmul-style loop-invariant hoist (LICM): a vector slot the arm carries unchanged
    // every iteration has an *immutable* element base we resolve once at entry, then read
    // inline (`ptr + idx*STRIDE`) instead of calling `brood_rt_vector_ref` per element.
    // Sound with no alias analysis because Brood data can't be mutated (ADR-026). Gated to
    // arms that neither allocate (`cons`/vector build → LOCAL GC) nor make a Brood→Brood
    // call (could `def` → RUNTIME compaction): under that gate nothing runs mid-arm to
    // relocate the storage, and a preempt/deopt re-enters from the entry block (re-hoist).
    let invariant = invariant_param_slots(lower_body, nrequired);
    let hoist_safe = !code.iter().any(|i| {
        matches!(
            i,
            Inst::Call { .. }
                | Inst::MakeVector(_)
                | Inst::Prim2 {
                    op: PrimOp::Cons,
                    ..
                }
                | Inst::Prim2SlotSlot {
                    op: PrimOp::Cons,
                    ..
                }
                | Inst::Prim2SlotInt {
                    op: PrimOp::Cons,
                    ..
                }
        )
    });
    // Invariant slots actually read as a fused `(nth slot idx)` vector operand — the only
    // form that names its vector slot directly (a global / computed vector can't hoist).
    let mut hoist_slots: Vec<usize> = Vec::new();
    if hoist_safe {
        for i in code.iter() {
            if let Inst::Prim2SlotSlot {
                op: PrimOp::VectorRef,
                slot_a,
                ..
            } = i
            {
                if invariant.get(*slot_a).copied().unwrap_or(false) && !hoist_slots.contains(slot_a)
                {
                    hoist_slots.push(*slot_a);
                }
            }
        }
    }
    // The global lever: globals read as a `(nth GLOBAL idx)` vector operand. A global is
    // loop-invariant within this (no-call) arm; we resolve its element base once at entry
    // and guard the back-edge on `global_epoch` so a concurrent `def` rebind deopts (keeping
    // it bit-identical to the VM's late binding). Same `hoist_safe` gate as the local hoist.
    let mut hoist_globals: std::collections::HashSet<Symbol> = std::collections::HashSet::new();
    if hoist_safe {
        invariant_global_vecs(lower_body, &mut hoist_globals);
    }
    // The scalar-global lever (#1, the late-binding tax): a global read in value position
    // (`n` in `loop--acc`'s `(>= i n)`) is loop-invariant within a no-call arm, but was
    // re-resolved through the inline cache (`brood_rt_global_ic`) **every iteration** —
    // ~39% of the `loop` benchmark. Resolve each once at entry and reuse its words in the
    // body; the back-edge `entry_epoch` guard deopts on a concurrent `def` rebind, so it
    // stays bit-identical to the VM's late binding. Excludes globals already hoisted as
    // vectors (those carry the ptr/len too). Same `hoist_safe` gate.
    let mut hoist_scalar_globals: std::collections::HashSet<Symbol> =
        std::collections::HashSet::new();
    if hoist_safe {
        for i in code.iter() {
            if let Inst::Global(s) | Inst::GlobalIc { sym: s, .. } = i {
                if !hoist_globals.contains(s) {
                    hoist_scalar_globals.insert(*s);
                }
            }
        }
    }
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
    let has_float_slot = slot_tags.contains(&profile_tag_float);
    // Free globals observed holding a `Value::Float` when this arm was elected for
    // tiering — the global-read counterpart of the `slot_tags` param profile. Empty when
    // unset (an arm lowered through a path that had no `Heap`, or `BROOD_NO_FLOAT_GLOBAL`),
    // which reproduces the pre-change lowering exactly.
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
    // base rather than sharing `spill_next`'s monotonic counter.
    let blockarg_spill_len = crate::eval::compile::jit_plan::max_leader_depth_pub(code);
    let spill_base = frame_top_for_spills - reserve;
    let blockarg_spill_base = spill_base + reserve.saturating_sub(blockarg_spill_len);
    let mut spill_next = 0usize;
    // Return-via-roots writes/reads the result at `roots[base]` (slot 0), and the VM hooks
    // read it back the same way — both require slot 0 to exist. A 0-slot arm (a 0-arg,
    // 0-local fn like `(defn k () 7)`) has `base == roots_len`, so `roots[base]` is out of
    // bounds. Such arms are trivial; bail and let the VM run them.
    if nslots == 0 {
        return trace_lower_bail(arm, "zero-slot-arm");
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
        return trace_lower_bail(arm, "chunk-outside-jit-subset");
    }

    // ---- Body-weight gate for arms ending in a tail call (jit-tier2.md §6.2). ----
    // A **tail** call returns to the driver (outcome 4) to dispatch the callee and reuse
    // the frame — a per-hop native↔driver round-trip the self-recursive `SelfCall` loop
    // avoids (it loops inside native). There are two costs that must be amortised:
    //
    // 1. The native→driver round-trip overhead per activation. Benchmarking mutual
    //    recursion puts the crossover at ~3 "work" ops: a 2-op `(if (= n 0) … (g (- n 1)))`
    //    ping/pong loop *regresses* ~7% (the native body is too small to amortise the
    //    round-trip), a 3-op body is ~neutral, a 5-op body gains ~12%.
    //
    // 2. `jit_dispatch_call` (non-tail native→native linking) does not yet follow an
    //    outcome-4 tail staged by the callee — it re-runs the callee via `vm_apply` instead,
    //    paying both JIT and VM overhead. Until that is fixed, a JIT-compiled thin delegator
    //    (e.g. `prime?` tail-calling `divides-none?`) called from JIT code in non-tail
    //    position regresses because every call hits the re-run path.
    //
    // So an arm containing a tail call must have **≥ 4 work instructions** (arithmetic/list
    // prims + nested non-tail calls) to lower; a thinner one stays on the VM — same speed,
    // no regression. Arms with no tail call are unaffected (no round-trip): a tiny `SelfCall`
    // int loop still tiers (~27× win).
    const TAIL_CALL_MIN_WORK: usize = 4;
    let has_tail_call = code
        .iter()
        .any(|i| matches!(i, Inst::Call { tail: true, .. }));
    let has_self_call = code.iter().any(|i| matches!(i, Inst::SelfCall { .. }));
    // The gate only applies when the arm is self-recursive (SelfCall present). A non-self-
    // recursive arm with a tail call is a pure delegator: it calls out exactly once and never
    // returns to a self-loop, so the tail-call overhead is amortised over all the callee's
    // work. With outcome-4 follow-through in `jit_dispatch_call` / `jit_run_fast_link`, such
    // arms are now safe to compile without regression.
    if has_tail_call && has_self_call {
        let work = code
            .iter()
            .filter(|i| {
                matches!(
                    i,
                    Inst::Prim1 { .. }
                        | Inst::Prim2 { .. }
                        | Inst::Prim2SlotSlot { .. }
                        | Inst::Prim2SlotInt { .. }
                        | Inst::Call { tail: false, .. }
                )
            })
            .count();
        if work < TAIL_CALL_MIN_WORK {
            return trace_lower_bail(arm, "tail-call-below-min-work");
        }
    }

    let (is_leader, depth) = prepass::block_analysis(code, len);

    let m = jit.module();
    let ptr_ty = m.target_config().pointer_type();
    let mut sig = m.make_signature();
    sig.params.push(AbiParam::new(ptr_ty)); // heap
    sig.params.push(AbiParam::new(types::I64)); // base (frame index into roots)
    sig.returns.push(AbiParam::new(types::I64)); // outcome: 0 = Done, 1 = deopt, 2 = preempt
    let seq = JIT_ARM_SEQ.fetch_add(1, Ordering::Relaxed);
    let id = m
        .declare_function(&format!("brood_jit_arm_{seq}"), Linkage::Export, &sig)
        .ok()?;
    let mut rb_sig = m.make_signature();
    rb_sig.params.push(AbiParam::new(ptr_ty));
    rb_sig.returns.push(AbiParam::new(ptr_ty));
    let rb_id = m
        .declare_function("brood_rt_roots_base", Linkage::Import, &rb_sig)
        .ok()?;
    // brood_rt_note_deopt(heap, reason): records WHY the arm is deopting. The shared deopt
    // block takes the id as a block param, so every guard can name itself (KI-49: a deopt
    // reported only its resume checkpoint, and an arm can have many guards after that).
    let mut nd_sig = m.make_signature();
    nd_sig.params.push(AbiParam::new(ptr_ty));
    nd_sig.params.push(AbiParam::new(types::I32));
    let nd_id = m
        .declare_function("brood_rt_note_deopt", Linkage::Import, &nd_sig)
        .ok()?;
    // brood_rt_tick_n(heap, n) -> u8: the batched back-edge poll (burns n reductions).
    let mut tickn_sig = m.make_signature();
    tickn_sig.params.push(AbiParam::new(ptr_ty));
    tickn_sig.params.push(AbiParam::new(types::I64));
    tickn_sig.returns.push(AbiParam::new(types::I8));
    let tickn_id = m
        .declare_function("brood_rt_tick_n", Linkage::Import, &tickn_sig)
        .ok()?;
    // The handle ops, by-value with an out-pointer (a `Value` is 24 bytes → no register-pair
    // return): brood_rt_cons(heap, out, car0,car1,car2, cdr0,cdr1,cdr2);
    // brood_rt_{car,cdr}(heap, out, w0,w1,w2). They write the result `Value` to `*out`.
    let mut car_sig = m.make_signature();
    car_sig.params.push(AbiParam::new(ptr_ty)); // heap
    car_sig.params.push(AbiParam::new(ptr_ty)); // out: *mut Value
    for _ in 0..3 {
        car_sig.params.push(AbiParam::new(types::I64)); // the operand's 3 words
    }
    let car_id = m
        .declare_function("brood_rt_car", Linkage::Import, &car_sig)
        .ok()?;
    let cdr_id = m
        .declare_function("brood_rt_cdr", Linkage::Import, &car_sig)
        .ok()?;
    // Inline `first`/`rest` support: expose LOCAL pair-slab base pointers once per arm entry
    // so the JIT can emit `ptr + idx*48 + {0,24}` loads instead of per-element FFI calls.
    let mut pbase_sig = m.make_signature();
    pbase_sig.params.push(AbiParam::new(ptr_ty)); // heap
    pbase_sig.returns.push(AbiParam::new(ptr_ty)); // *const u8
    let pnbase_id = m
        .declare_function("brood_rt_pair_nursery_base", Linkage::Import, &pbase_sig)
        .ok()?;
    let pobase_id = m
        .declare_function("brood_rt_pair_old_base", Linkage::Import, &pbase_sig)
        .ok()?;
    // Inline small-vector `nth` support: LOCAL vector-slab base pointers (same
    // `heap -> *const u8` signature as the pair bases), for `slot + items_off +
    // i*24` loads instead of per-read `brood_rt_vector_ref` FFI calls.
    let vnbase_id = m
        .declare_function("brood_rt_vec_nursery_base", Linkage::Import, &pbase_sig)
        .ok()?;
    let vobase_id = m
        .declare_function("brood_rt_vec_old_base", Linkage::Import, &pbase_sig)
        .ok()?;
    let mut cons_sig = m.make_signature();
    cons_sig.params.push(AbiParam::new(ptr_ty)); // heap
    cons_sig.params.push(AbiParam::new(ptr_ty)); // out
    for _ in 0..6 {
        cons_sig.params.push(AbiParam::new(types::I64)); // car 3 words + cdr 3 words
    }
    let cons_id = m
        .declare_function("brood_rt_cons", Linkage::Import, &cons_sig)
        .ok()?;
    // brood_rt_make_vector2(heap, out, a 3 words, b 3 words) — same ABI as cons,
    // builds a 2-element vector (`[a b]` literal, e.g. bintree's `make`).
    let mut makevec2_sig = m.make_signature();
    makevec2_sig.params.push(AbiParam::new(ptr_ty)); // heap
    makevec2_sig.params.push(AbiParam::new(ptr_ty)); // out
    for _ in 0..6 {
        makevec2_sig.params.push(AbiParam::new(types::I64)); // elem0 3 words + elem1 3 words
    }
    let makevec2_id = m
        .declare_function("brood_rt_make_vector2", Linkage::Import, &makevec2_sig)
        .ok()?;
    // brood_rt_make_vector_n(heap, out, elems: *const Value, n) — builds an n-element
    // vector from `n` `Value`s the JIT staged contiguously at `elems` (a stack slot it
    // owns). The variadic `MakeVector(n != 2)` path; `alloc_vector` never collects, so
    // the staged bytes stay live across the call (same discipline as make_vector2).
    let mut makevecn_sig = m.make_signature();
    makevecn_sig.params.push(AbiParam::new(ptr_ty)); // heap
    makevecn_sig.params.push(AbiParam::new(ptr_ty)); // out
    makevecn_sig.params.push(AbiParam::new(ptr_ty)); // elems
    makevecn_sig.params.push(AbiParam::new(types::I64)); // n
    let makevecn_id = m
        .declare_function("brood_rt_make_vector_n", Linkage::Import, &makevecn_sig)
        .ok()?;
    // brood_rt_gc_safepoint(heap): collect if due (bounds the nursery for cons loops).
    let mut sp_sig = m.make_signature();
    sp_sig.params.push(AbiParam::new(ptr_ty));
    let sp_id = m
        .declare_function("brood_rt_gc_safepoint", Linkage::Import, &sp_sig)
        .ok()?;
    // DEBUG ONLY: brood_rt_dbg_set_staging(heap, site) — record the staging call site.
    #[cfg(debug_assertions)]
    let dbg_staging_id = {
        let mut s = m.make_signature();
        s.params.push(AbiParam::new(ptr_ty));
        s.params.push(AbiParam::new(types::I32));
        m.declare_function("brood_rt_dbg_set_staging", Linkage::Import, &s)
            .ok()?
    };
    // DEBUG ONLY: brood_rt_dbg_check_slot(heap, w0, abs_idx) — validate a slot read.
    #[cfg(debug_assertions)]
    let dbg_check_slot_id = {
        let mut s = m.make_signature();
        s.params.push(AbiParam::new(ptr_ty)); // heap
        s.params.push(AbiParam::new(types::I64)); // w0
        s.params.push(AbiParam::new(types::I64)); // w1
        s.params.push(AbiParam::new(types::I64)); // w2
        s.params.push(AbiParam::new(types::I64)); // abs_idx
        m.declare_function("brood_rt_dbg_check_slot", Linkage::Import, &s)
            .ok()?
    };
    // The Brood→Brood call ABI. brood_rt_push(heap, w0,w1,w2): stage one operand `Value`
    // onto `roots`. brood_rt_global(heap, out, sym) -> status: resolve a free global into
    // `*out`. brood_rt_call_slow(heap, out, argc) -> status: dispatch the staged call into
    // `*out`. Status 0 = ok, nonzero = error parked for the arm to propagate.
    let mut push_sig = m.make_signature();
    push_sig.params.push(AbiParam::new(ptr_ty)); // heap
    for _ in 0..3 {
        push_sig.params.push(AbiParam::new(types::I64)); // the operand's 3 words
    }
    let _push_id = m
        .declare_function("brood_rt_push", Linkage::Import, &push_sig)
        .ok()?;
    let mut glob_sig = m.make_signature();
    glob_sig.params.push(AbiParam::new(ptr_ty)); // heap
    glob_sig.params.push(AbiParam::new(ptr_ty)); // out: *mut Value
    glob_sig.params.push(AbiParam::new(types::I32)); // sym (interned u32)
    glob_sig.returns.push(AbiParam::new(types::I64)); // status
    let glob_id = m
        .declare_function("brood_rt_global", Linkage::Import, &glob_sig)
        .ok()?;
    // Same signature, but resolves WITHOUT parking an unbound error — the entry hoist
    // deopts on unbound rather than raising (see `brood_rt_global_probe`).
    let globprobe_id = m
        .declare_function("brood_rt_global_probe", Linkage::Import, &glob_sig)
        .ok()?;
    // brood_rt_global_ic(heap, out, sym, site) -> status: as above but through the
    // per-site global inline cache (no `env_get` walk on a cache hit).
    let mut globic_sig = m.make_signature();
    globic_sig.params.push(AbiParam::new(ptr_ty)); // heap
    globic_sig.params.push(AbiParam::new(ptr_ty)); // out: *mut Value
    globic_sig.params.push(AbiParam::new(types::I32)); // sym
    globic_sig.params.push(AbiParam::new(types::I32)); // site
    globic_sig.returns.push(AbiParam::new(types::I64)); // status
    let globic_id = m
        .declare_function("brood_rt_global_ic", Linkage::Import, &globic_sig)
        .ok()?;
    let mut callslow_sig = m.make_signature();
    callslow_sig.params.push(AbiParam::new(ptr_ty)); // heap
    callslow_sig.params.push(AbiParam::new(ptr_ty)); // out: *mut Value
    callslow_sig.params.push(AbiParam::new(types::I32)); // argc (u32)
    callslow_sig.params.push(AbiParam::new(types::I32)); // call site (NO_SITE if none)
    callslow_sig.params.push(AbiParam::new(types::I32)); // call-head sym (u32::MAX if none)
    callslow_sig.returns.push(AbiParam::new(types::I64)); // status
    let callslow_id = m
        .declare_function("brood_rt_call_slow", Linkage::Import, &callslow_sig)
        .ok()?;
    // brood_rt_push_n(heap, src, n): batch-stage `n` Values from the call site's
    // staging stack slot onto roots — one FFI + memcpy instead of push × argc.
    let mut pushn_sig = m.make_signature();
    pushn_sig.params.push(AbiParam::new(ptr_ty)); // heap
    pushn_sig.params.push(AbiParam::new(ptr_ty)); // src
    pushn_sig.params.push(AbiParam::new(types::I64)); // n
    pushn_sig.returns.push(AbiParam::new(types::I64));
    let pushn_id = m
        .declare_function("brood_rt_push_n", Linkage::Import, &pushn_sig)
        .ok()?;
    // brood_rt_call_native_fl(heap, out, func, args, argc): direct builtin call for
    // a native flat-cell hit (nslots == u32::MAX) — no roots staging at all.
    let mut natfl_sig = m.make_signature();
    natfl_sig.params.push(AbiParam::new(ptr_ty)); // heap
    natfl_sig.params.push(AbiParam::new(ptr_ty)); // out
    natfl_sig.params.push(AbiParam::new(types::I64)); // func bits
    natfl_sig.params.push(AbiParam::new(ptr_ty)); // args ptr
    natfl_sig.params.push(AbiParam::new(types::I32)); // argc
    natfl_sig.returns.push(AbiParam::new(types::I64));
    let natfl_id = m
        .declare_function("brood_rt_call_native_fl", Linkage::Import, &natfl_sig)
        .ok()?;
    // Track B / Technique A — the in-IR fast call path. brood_rt_fastlink_base(heap,
    // out_len: *mut u64) -> *const FastLink: base + length of the IR-readable fast-link
    // mirror. brood_rt_fast_frame(heap, out, site, head, argc, nslots, code, env,
    // callee_ic_base, callee_gic_base) -> status:
    // run the (already epoch-validated, flat-table-read) native fast-link. Status 0 = done,
    // 1 = error parked, 2 = could-not-link (fall to brood_rt_call_slow).
    let mut flbase_sig = m.make_signature();
    flbase_sig.params.push(AbiParam::new(ptr_ty)); // heap
    flbase_sig.params.push(AbiParam::new(ptr_ty)); // out_len: *mut u64
    flbase_sig.returns.push(AbiParam::new(ptr_ty)); // *const FastLink
    let flbase_id = m
        .declare_function("brood_rt_fastlink_base", Linkage::Import, &flbase_sig)
        .ok()?;
    let mut fastframe_sig = m.make_signature();
    fastframe_sig.params.push(AbiParam::new(ptr_ty)); // heap
    fastframe_sig.params.push(AbiParam::new(ptr_ty)); // out: *mut Value
    fastframe_sig.params.push(AbiParam::new(types::I32)); // site
    fastframe_sig.params.push(AbiParam::new(types::I32)); // head sym
    fastframe_sig.params.push(AbiParam::new(types::I32)); // argc
    fastframe_sig.params.push(AbiParam::new(types::I32)); // nslots
    fastframe_sig.params.push(AbiParam::new(types::I64)); // code (native entry ptr as u64)
    fastframe_sig.params.push(AbiParam::new(types::I64)); // env (EnvId raw word)
    fastframe_sig.params.push(AbiParam::new(types::I32)); // callee_ic_base
    fastframe_sig.params.push(AbiParam::new(types::I32)); // callee_gic_base
    fastframe_sig.returns.push(AbiParam::new(types::I64)); // status
    let fastframe_id = m
        .declare_function("brood_rt_fast_frame", Linkage::Import, &fastframe_sig)
        .ok()?;
    // brood_rt_vector_ref(heap, out, vec 3 words, idx 3 words) -> status: bounds-checked
    // slab read into `*out` (0 = ok, 1 = deopt for non-vector / non-int / out-of-range).
    let mut vref_sig = m.make_signature();
    vref_sig.params.push(AbiParam::new(ptr_ty)); // heap
    vref_sig.params.push(AbiParam::new(ptr_ty)); // out: *mut Value
    for _ in 0..6 {
        vref_sig.params.push(AbiParam::new(types::I64)); // vec 3 words + idx 3 words
    }
    vref_sig.returns.push(AbiParam::new(types::I64)); // status
    let vref_id = m
        .declare_function("brood_rt_vector_ref", Linkage::Import, &vref_sig)
        .ok()?;
    // brood_rt_table_has / brood_rt_table_get2: (heap, out, table 3 words, key 3 words)
    // -> status. Same word-triple signature as vector_ref; status 2 = error parked.
    let thas_id = m
        .declare_function("brood_rt_table_has", Linkage::Import, &vref_sig)
        .ok()?;
    let tget_id = m
        .declare_function("brood_rt_table_get2", Linkage::Import, &vref_sig)
        .ok()?;
    // brood_rt_table_put: (heap, out, table 3w, key 3w, val 3w) -> status.
    let mut tput_sig = m.make_signature();
    tput_sig.params.push(AbiParam::new(ptr_ty)); // heap
    tput_sig.params.push(AbiParam::new(ptr_ty)); // out
    for _ in 0..9 {
        tput_sig.params.push(AbiParam::new(types::I64));
    }
    tput_sig.returns.push(AbiParam::new(types::I64));
    let tput_id = m
        .declare_function("brood_rt_table_put", Linkage::Import, &tput_sig)
        .ok()?;
    // brood_rt_vector_base(heap, vec 3 words, out_len: *mut i64) -> *const Value: resolve
    // an invariant vector's element (data_ptr, len) once for the LICM hoist; null ptr ⇒
    // not a vector (the hoist deopts at entry). Only declared/used when `hoist_slots`.
    let mut vbase_sig = m.make_signature();
    vbase_sig.params.push(AbiParam::new(ptr_ty)); // heap
    for _ in 0..3 {
        vbase_sig.params.push(AbiParam::new(types::I64)); // vec 3 words
    }
    vbase_sig.params.push(AbiParam::new(ptr_ty)); // out_len: *mut i64
    vbase_sig.returns.push(AbiParam::new(ptr_ty)); // element data ptr (null = non-vector)
    let vbase_id = m
        .declare_function("brood_rt_vector_base", Linkage::Import, &vbase_sig)
        .ok()?;
    // brood_rt_table_dense_base(heap, table 3 words, out_flag: *mut i64) -> *const u8:
    // resolve a hoisted global table's dense slot region once (the sieve lever); null ⇒
    // non-table / hashed / dropped (per-op FFI path used instead). Same shape as vbase.
    let tdbase_id = m
        .declare_function("brood_rt_table_dense_base", Linkage::Import, &vbase_sig)
        .ok()?;
    // brood_rt_global_epoch(heap) -> i64: the process global-rebind epoch, for the
    // back-edge guard that keeps a hoisted global vector bit-identical to the VM's late
    // binding (deopt if the global was rebound). Only declared/used when hoisting a global.
    // brood_rt_global_epoch_ptr(heap) -> *const u64: the epoch counter's address, fetched once
    // at entry so the per-iteration back-edge guard / per-call icall check reads it with a raw
    // load instead of a `brood_rt_global_epoch` FFI call (~20% of a hoisted-global loop).
    let mut gepochptr_sig = m.make_signature();
    gepochptr_sig.params.push(AbiParam::new(ptr_ty));
    gepochptr_sig.returns.push(AbiParam::new(ptr_ty));
    let gepochptr_id = m
        .declare_function("brood_rt_global_epoch_ptr", Linkage::Import, &gepochptr_sig)
        .ok()?;
    // brood_rt_const_load(cv: *const ConstVal, out: *mut Value): load the current Value
    // from a GC-movable ConstVal::Handle, writing it to *out. No return value — never fails.
    let mut const_load_sig = m.make_signature();
    const_load_sig.params.push(AbiParam::new(ptr_ty)); // cv: *const ConstVal
    const_load_sig.params.push(AbiParam::new(ptr_ty)); // out: *mut Value
    let const_load_id = m
        .declare_function("brood_rt_const_load", Linkage::Import, &const_load_sig)
        .ok()?;

    let mut ctx = m.make_context();
    ctx.func.signature = sig;
    let mut fbctx = FunctionBuilderContext::new();
    let mut b = FunctionBuilder::new(&mut ctx.func, &mut fbctx);
    // Register-carry: for pure-arithmetic self-tail loops, carry each param slot in a
    // Cranelift Variable (SSA, phi-inserted at the loop header). Reads skip the per-access
    // tag-check + address arithmetic + two memory ops entirely. The `roots` stores at each
    // SelfCall are kept unchanged for deopt correctness; only reads change.
    // carry_vars: Vec<(Variable, is_float)>. Int slots → I64 Variable; Float slots → F64
    // Variable. Every slot in 0..max_selfcall_argc must be profiled as TAG_INT or TAG_FLOAT;
    // anything else (vector, nil, handle) is excluded — TAG_VEC would deopt on every call.
    let profile_tag_int = crate::core::value::Tag::Int as u8;
    let profile_tag_float_carry = crate::core::value::Tag::Float as u8;
    // Per-slot: `Some((var, is_float))` carries param slot `k` in a Cranelift Variable;
    // `None` leaves it on the frame (a handle/vector/nil slot — GC-relocatable, so it
    // must stay rooted). A *pure* scalar self-tail loop (`loop`/`collatz`/`mandelbrot`)
    // carries every slot (all `Some`) — bit-identical to the prior all-or-nothing path.
    // The generalisation (Layer A) is that a **call-mediated** self-recursive arm mixing
    // a handle with scalars — nbody's `newvel b:handle i:int j:int vx/vy/vz:float` — now
    // carries just its scalar slots instead of bailing entirely, keeping those floats
    // unboxed across the self-recursion. Sound because a scalar in a register is invisible
    // to (and unmoved by) GC across a call safepoint, and a deopt always restarts the arm
    // from the frame's last-SelfCall iteration inputs (the `roots` stores are kept), which
    // the entry tag-check re-validates. The per-read `as_f64`/`load_slot_int` tag-check
    // still guards a mistyped profile → deopt, so this can never miscompile.
    let carry_vars: Vec<Option<(Variable, bool)>> = {
        let has_self_call = code.iter().any(|i| matches!(i, Inst::SelfCall { .. }));
        let max_argc = code
            .iter()
            .filter_map(|i| {
                if let Inst::SelfCall { argc } = i {
                    Some(*argc)
                } else {
                    None
                }
            })
            .max()
            .unwrap_or(0);
        if has_self_call && max_argc > 0 {
            (0..max_argc)
                .map(|k| match slot_tags.get(k).copied() {
                    Some(t) if t == profile_tag_int => Some((b.declare_var(types::I64), false)),
                    Some(t) if t == profile_tag_float_carry => {
                        Some((b.declare_var(types::F64), true))
                    }
                    _ => None,
                })
                .collect()
        } else {
            vec![]
        }
    };
    let rb_ref = m.declare_func_in_func(rb_id, b.func);
    let tickn_ref = m.declare_func_in_func(tickn_id, b.func);
    let car_ref = m.declare_func_in_func(car_id, b.func);
    let cdr_ref = m.declare_func_in_func(cdr_id, b.func);
    let pnbase_ref = m.declare_func_in_func(pnbase_id, b.func);
    let pobase_ref = m.declare_func_in_func(pobase_id, b.func);
    let vnbase_ref = m.declare_func_in_func(vnbase_id, b.func);
    let vobase_ref = m.declare_func_in_func(vobase_id, b.func);
    let cons_ref = m.declare_func_in_func(cons_id, b.func);
    let makevec2_ref = m.declare_func_in_func(makevec2_id, b.func);
    let makevecn_ref = m.declare_func_in_func(makevecn_id, b.func);
    let sp_ref = m.declare_func_in_func(sp_id, b.func);
    #[cfg(debug_assertions)]
    let dbg_staging_ref = m.declare_func_in_func(dbg_staging_id, b.func);
    // Declared for ad-hoc slot-read validation during bug hunts (calls removed from
    // read_words — they perturbed codegen and masked the bug they were chasing).
    #[cfg(debug_assertions)]
    let _dbg_check_slot_ref = m.declare_func_in_func(dbg_check_slot_id, b.func);
    let glob_ref = m.declare_func_in_func(glob_id, b.func);
    let globprobe_ref = m.declare_func_in_func(globprobe_id, b.func);
    let globic_ref = m.declare_func_in_func(globic_id, b.func);
    let callslow_ref = m.declare_func_in_func(callslow_id, b.func);
    let pushn_ref = m.declare_func_in_func(pushn_id, b.func);
    let natfl_ref = m.declare_func_in_func(natfl_id, b.func);
    let flbase_ref = m.declare_func_in_func(flbase_id, b.func);
    let fastframe_ref = m.declare_func_in_func(fastframe_id, b.func);
    let nd_ref = m.declare_func_in_func(nd_id, b.func);
    let vref_ref = m.declare_func_in_func(vref_id, b.func);
    let thas_ref = m.declare_func_in_func(thas_id, b.func);
    let tget_ref = m.declare_func_in_func(tget_id, b.func);
    let tput_ref = m.declare_func_in_func(tput_id, b.func);
    let vbase_ref = m.declare_func_in_func(vbase_id, b.func);
    let tdbase_ref = m.declare_func_in_func(tdbase_id, b.func);
    let gepochptr_ref = m.declare_func_in_func(gepochptr_id, b.func);
    let const_load_ref = m.declare_func_in_func(const_load_id, b.func);
    // Whether the arm allocates (`cons`) — gates the back-edge GC safepoint that bounds
    // the nursery. (`car`/`rest` don't allocate.)
    let has_cons = code.iter().any(inst_allocates_hot);

    // One Cranelift block per leader (with `depth` I64 params), plus entry/deopt. The
    // Done block (`ip == len`) takes **no** params: the result is returned via
    // `roots[base]` (each exit stores it there), so it can be a handle, not just an
    // `i64` block arg. Every other block carries its operand-stack depth as I64 params.
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
    // `roots_base` is a **Variable**, not a fixed SSA value: a Brood→Brood call's staging
    // pushes (and the callee's own frames) may reallocate `roots`, so the base is re-fetched
    // after each call (`def_var` below). For a call-free arm it keeps its single entry
    // definition (no phi, no reload) — the int/cons subset is unaffected. Helpers read it
    // via `b.use_var(rb_var)`.
    let rb_var = b.declare_var(ptr_ty);
    let call = b.ins().call(rb_ref, &[heap]);
    b.def_var(rb_var, b.inst_results(call)[0]);
    // The frame-access context the extracted slot helpers (`emit::load_slot_int` etc.)
    // read; all fields are `Copy`, so it threads by value.
    // KI-49: which slots the tier-time profile saw an `Int` in. A profiled-Int slot keeps
    // the unboxed i64 block-arg carry (`fib`/`collatz`); every other slot crosses a block
    // boundary as `ParamRepr::Slot`, so it is never forced through `as_int`.
    let slot_int_profile: Vec<bool> = (0..nslots)
        .map(|i| slot_tags.get(i).copied() == Some(TAG_INT))
        .collect();
    let frame = emit::Frame {
        rb_var,
        base,
        nslots,
        deopt,
        carry_vars: &carry_vars,
        slot_float: &slot_float,
        slot_bool: &slot_bool,
        slot_int_profile: &slot_int_profile,
        blockarg_spill_base,
        blockarg_spill_len,
        slot_f64_cache: &slot_f64_cache,
    };
    // A scratch `Value`-sized stack slot the handle / call / global ops write their result
    // into (the out-pointer ABI). One per arm, reused: each result is read straight back
    // into registers before the next op.
    let out_slot = b.create_sized_stack_slot(StackSlotData::new(
        StackSlotKind::ExplicitSlot,
        STRIDE as u32,
        3,
    ));

    // **Stack guard (KI-14).** Every native frame passes through here, whatever route
    // created it — which is the point: the pre-existing guards (`jit_native_depth` + the
    // `stacker` headroom probe) live on the *dispatch* paths and therefore only bound
    // recursion that goes through a fast link. A 100 000-deep JSON parse found a path that
    // reaches neither: on the root thread the depth cap fired at 1500, but in a spawned
    // green process the probe was never called at all and the worker died on its guard
    // page — a `SIGSEGV`-class abort that `try`/`catch` cannot observe and no supervisor
    // can restart (the OS process goes, not the green process).
    //
    // Cost is a load of the limit the entry point stamped (`Heap::jit_stack_limit`), one
    // compare of this frame's address against it, and a predicted branch. On a
    // trip we jump to `deopt`, NOT `error`: the VM owns deep recursion properly — it grows
    // heap frames and raises the clean, catchable `MAX_BC_FRAMES` error — so draining there
    // is both correct and the behaviour the non-JIT build already has. A `0` limit (the
    // probe couldn't read the stack) skips the check, failing open exactly as the old
    // headroom probe does with `None` — and it does so for *free*, because the compare is
    // unsigned: no address is `< 0`, so a zero limit can never trip. An explicit
    // `limit != 0` test used to sit here and was pure overhead on every activation.
    {
        let limit = b.ins().load(
            ptr_ty,
            cranelift_codegen::ir::MemFlagsData::trusted(),
            heap,
            std::mem::offset_of!(crate::core::heap::Heap, jit_stack_limit) as i32,
        );
        // The address of a scratch slot in *this* frame stands in for the stack pointer;
        // the stack grows down, so `here < limit` means we have run past the budget.
        let probe =
            b.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 8, 3));
        let here = b.ins().stack_addr(ptr_ty, probe, 0);
        // `limit == 0` (probe unavailable) disables the guard implicitly: this compare is
        // unsigned, so no frame address is ever below zero.
        let trip = b.ins().icmp(IntCC::UnsignedLessThan, here, limit);
        let cont = b.create_block();
        let bail = b.create_block();
        b.ins().brif(trip, bail, &[], cont, &[]);
        // On a trip, set `Heap::jit_force_vm` before deopting. Deopt alone is not enough:
        // the VM re-runs the arm, the recursive callee tiers again, its prologue trips
        // again — a livelock that spins at 100% CPU making no progress (which is what the
        // suite showed before this store went in). The flag makes `jit_tier` decline to run
        // native for the rest of this subtree, so the recursion drains through the bounded
        // heap-frame loop and raises the clean `MAX_BC_FRAMES` error. The native entry
        // points save/restore it around the call, so it is scoped to this subtree.
        b.switch_to_block(bail);
        let one = b.ins().iconst(types::I8, 1);
        b.ins().store(
            cranelift_codegen::ir::MemFlagsData::trusted(),
            one,
            heap,
            std::mem::offset_of!(crate::core::heap::Heap, jit_force_vm) as i32,
        );
        let __dr = b.ins().iconst(types::I32, 101);
        b.ins().jump(deopt, &[BlockArg::Value(__dr)]);
        b.seal_block(bail);
        b.switch_to_block(cont);
    }

    // LICM hoist: resolve each invariant vector slot's element (ptr, len) once here in
    // the entry block (which dominates every loop block, so the values are usable in the
    // body). A non-vector slot branches to `deopt` (the VM then owns the exact result).
    // Maps slot → (data_ptr, len). Empty for the common arm (no invariant vector read).
    let mut hoisted: std::collections::HashMap<
        usize,
        (cranelift_codegen::ir::Value, cranelift_codegen::ir::Value),
    > = std::collections::HashMap::new();
    // Hoisted global vectors: sym → (ptr, len, w0, w1, w2). The word triple is the global's
    // entry-resolved `Value` (for any non-`VectorRef` use); the ptr/len drive the inline
    // element read. `entry_epoch` is the `global_epoch` at entry, re-checked on the back-edge.
    type HoistedGlobal = (
        cranelift_codegen::ir::Value,
        cranelift_codegen::ir::Value,
        cranelift_codegen::ir::Value,
        cranelift_codegen::ir::Value,
        cranelift_codegen::ir::Value,
    );
    let mut hoisted_global: std::collections::HashMap<Symbol, HoistedGlobal> =
        std::collections::HashMap::new();
    // Hoisted scalar globals (#1): sym → the global's entry-resolved `Value` words. Read in
    // value position via `Op::Handle` in the body (no per-access `brood_rt_global_ic`).
    let mut hoisted_scalar: std::collections::HashMap<
        Symbol,
        (
            cranelift_codegen::ir::Value,
            cranelift_codegen::ir::Value,
            cranelift_codegen::ir::Value,
        ),
    > = std::collections::HashMap::new();
    // Hoisted global dense tables (the sieve lever): sym → (slots base — possibly
    // NULL at runtime for a hashed/non-table global, checked per op —, dense-flag
    // address, and the global's entry-resolved `Value` words).
    let mut hoisted_table: std::collections::HashMap<
        Symbol,
        (
            cranelift_codegen::ir::Value,
            cranelift_codegen::ir::Value,
            cranelift_codegen::ir::Value,
            cranelift_codegen::ir::Value,
            cranelift_codegen::ir::Value,
        ),
    > = std::collections::HashMap::new();
    // Only pay the entry-time dense-base resolution when the body has table ops
    // a hoisted table could serve.
    let chunk_has_table_ops = code.iter().any(|i| {
        matches!(
            i,
            Inst::Prim3 {
                op: PrimOp3::TablePut,
                ..
            } | Inst::Prim2 {
                op: PrimOp::TableHas,
                ..
            }
        )
    });
    let mut entry_epoch: Option<cranelift_codegen::ir::Value> = None;
    // Fetch the global-epoch counter's address once here in the entry block (which dominates
    // every loop/call block) when the arm reads the epoch on a hot path — a hoisted-global
    // back-edge guard, or an icall epoch check per call. Those sites then do a raw load instead
    // of a `brood_rt_global_epoch` FFI call each iteration/call (the call was ~20% of `loop`).
    let epoch_ptr: Option<cranelift_codegen::ir::Value> = {
        let needs = !hoist_globals.is_empty()
            || !hoist_scalar_globals.is_empty()
            || (icall_enabled()
                && code.iter().any(|i| {
                    matches!(
                        i,
                        Inst::Call {
                            tail: false,
                            head: Some(_),
                            ..
                        }
                    )
                }));
        if needs {
            let c = b.ins().call(gepochptr_ref, &[heap]);
            Some(b.inst_results(c)[0])
        } else {
            None
        }
    };

    // Inline `first`/`rest` pair reads: if the arm uses First/Rest but contains no Cons
    // or MakeVector (which trigger the back-edge GC safepoint — `minor_collect` replaces
    // `self.local` via `std::mem::take`, freeing the old nursery buffer and invalidating
    // the stashed pointer) and no non-tail Call (also a GC safepoint), fetch the LOCAL
    // nursery and old-gen pair-slab base pointers once here in the entry block. The inline
    // lowering then computes `base + idx*48 + {0,24}` directly and deopts for non-LOCAL
    // (PRELUDE/RUNTIME) pairs — those are rare on hot cons-list paths.
    //
    // The `has_cons` check here must mirror the one that gates `sp_ref` (the back-edge
    // safepoint call) at line ~8020, which includes MakeVector. If MakeVector is present,
    // the safepoint fires on the back-edge, `minor_collect` replaces `self.local`, and the
    // hoisted nursery base pointer becomes a dangling pointer into the freed slab.
    let pair_bases: Option<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value)> = {
        let has_car_cdr = code.iter().any(|i| {
            matches!(
                i,
                Inst::Prim1 {
                    op: PrimOp1::First | PrimOp1::Rest,
                    ..
                }
            )
        });
        let has_alloc_safepoint = code.iter().any(inst_may_allocate);
        // A non-tail Call is a GC safepoint: minor_collect replaces `self.local` entirely
        // (std::mem::take), so any pointer to `local.pairs` cached before the call is
        // invalid after it. Only inline when there are no such safepoints.
        let has_call_safepoint = code
            .iter()
            .any(|i| matches!(i, Inst::Call { tail: false, .. }));
        if has_car_cdr && !has_alloc_safepoint && !has_call_safepoint {
            let cn = b.ins().call(pnbase_ref, &[heap]);
            let nursery = b.inst_results(cn)[0];
            let co = b.ins().call(pobase_ref, &[heap]);
            let old = b.inst_results(co)[0];
            Some((nursery, old))
        } else {
            None
        }
    };

    if !hoist_slots.is_empty() || !hoist_globals.is_empty() || !hoist_scalar_globals.is_empty() {
        let len_slot =
            b.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 8, 3));
        let len_addr = b.ins().stack_addr(ptr_ty, len_slot, 0);
        for &slot in &hoist_slots {
            let roots_base = b.use_var(rb_var);
            let i = b.ins().iadd_imm(base, slot as i64);
            let o = b.ins().imul_imm(i, STRIDE);
            let addr = b.ins().iadd(roots_base, o);
            let w0 = b.ins().load(types::I64, MemFlagsData::trusted(), addr, 0);
            let w1 = b.ins().load(
                types::I64,
                MemFlagsData::trusted(),
                addr,
                PAYLOAD_OFFSET as i32,
            );
            let w2 = b.ins().load(
                types::I64,
                MemFlagsData::trusted(),
                addr,
                PAYLOAD_OFFSET as i32 + 8,
            );
            let c = b.ins().call(vbase_ref, &[heap, w0, w1, w2, len_addr]);
            let ptr = b.inst_results(c)[0];
            // null ptr ⇒ slot isn't a vector ⇒ deopt (VM runs the arm; same result).
            let cont = b.create_block();
            let __dr = b.ins().iconst(types::I32, 102);
            b.ins()
                .brif(ptr, cont, &[], deopt, &[BlockArg::Value(__dr)]);
            b.switch_to_block(cont);
            let vlen = b
                .ins()
                .load(types::I64, MemFlagsData::trusted(), len_addr, 0);
            hoisted.insert(slot, (ptr, vlen));
        }
        // Resolve each hoisted global once (sorted for deterministic codegen). Unbound ⇒
        // `deopt`, NOT `error`: the hoist runs at entry for every global the arm mentions,
        // including ones only a cold branch reads, so raising here reported `unbound
        // symbol` for a branch the VM never evaluates — `(defn pick (n) (if (< n 0)
        // never-defined-global (+ n 1)))` worked until it got hot, then threw. Deopting
        // hands the arm to the VM, which evaluates only the branch actually taken and
        // raises only if that branch really reads the name. `brood_rt_global_probe` is the
        // non-parking resolve, so no phantom error is left behind. Non-vector ⇒ `deopt`.
        let mut gsyms: Vec<Symbol> = hoist_globals.iter().copied().collect();
        gsyms.sort_unstable();
        for sym in gsyms {
            let out_addr = b.ins().stack_addr(ptr_ty, out_slot, 0);
            let symv = b.ins().iconst(types::I32, sym as i64);
            let c = b.ins().call(globprobe_ref, &[heap, out_addr, symv]);
            let status = b.inst_results(c)[0];
            let okb = b.create_block();
            let __dr = b.ins().iconst(types::I32, 103);
            b.ins()
                .brif(status, deopt, &[BlockArg::Value(__dr)], okb, &[]);
            b.switch_to_block(okb);
            let w0 = b.ins().stack_load(types::I64, out_slot, 0);
            let w1 = b
                .ins()
                .stack_load(types::I64, out_slot, PAYLOAD_OFFSET as i32);
            let w2 = b
                .ins()
                .stack_load(types::I64, out_slot, PAYLOAD_OFFSET as i32 + 8);
            let c = b.ins().call(vbase_ref, &[heap, w0, w1, w2, len_addr]);
            let ptr = b.inst_results(c)[0];
            let cont = b.create_block();
            let __dr = b.ins().iconst(types::I32, 104);
            b.ins()
                .brif(ptr, cont, &[], deopt, &[BlockArg::Value(__dr)]);
            b.switch_to_block(cont);
            let vlen = b
                .ins()
                .load(types::I64, MemFlagsData::trusted(), len_addr, 0);
            hoisted_global.insert(sym, (ptr, vlen, w0, w1, w2));
        }
        // Scalar globals (#1): resolve each once at entry into its `Value` words — no vector
        // base, no per-access IC. Unbound ⇒ `deopt`, for the same reason as the vector
        // hoist above: an entry-time resolve must not raise for a branch that never runs.
        let mut ssyms: Vec<Symbol> = hoist_scalar_globals.iter().copied().collect();
        ssyms.sort_unstable();
        for sym in ssyms {
            let out_addr = b.ins().stack_addr(ptr_ty, out_slot, 0);
            let symv = b.ins().iconst(types::I32, sym as i64);
            let c = b.ins().call(globprobe_ref, &[heap, out_addr, symv]);
            let status = b.inst_results(c)[0];
            let okb = b.create_block();
            let __dr = b.ins().iconst(types::I32, 105);
            b.ins()
                .brif(status, deopt, &[BlockArg::Value(__dr)], okb, &[]);
            b.switch_to_block(okb);
            let w0 = b.ins().stack_load(types::I64, out_slot, 0);
            let w1 = b
                .ins()
                .stack_load(types::I64, out_slot, PAYLOAD_OFFSET as i32);
            let w2 = b
                .ins()
                .stack_load(types::I64, out_slot, PAYLOAD_OFFSET as i32 + 8);
            hoisted_scalar.insert(sym, (w0, w1, w2));
            // Dense-table hoist (the sieve lever): resolve this global's dense slot
            // region once. NO branch here — a null base (non-table / hashed /
            // dropped) is carried and checked per op, so such an arm never deopts,
            // it just uses the per-op FFI path.
            if chunk_has_table_ops {
                let c = b.ins().call(tdbase_ref, &[heap, w0, w1, w2, len_addr]);
                let slots = b.inst_results(c)[0];
                let flag = b
                    .ins()
                    .load(types::I64, MemFlagsData::trusted(), len_addr, 0);
                hoisted_table.insert(sym, (slots, flag, w0, w1, w2));
            }
        }
        if !hoisted_global.is_empty() || !hoisted_scalar.is_empty() {
            let ep_ptr = epoch_ptr.expect("epoch_ptr fetched when globals are hoisted");
            entry_epoch = Some(b.ins().load(types::I64, MemFlagsData::trusted(), ep_ptr, 0));
        }
    }
    // Initialize register-carry variables from roots (first iteration). Each param slot k is
    // tag-checked (Int or Float, per is_float) once at entry; subsequent iterations read
    // `use_var(carry_vars[k].0)` directly. Float slots are bitcast i64→f64.
    for (k, entry) in carry_vars.iter().enumerate() {
        let (var, is_float) = match *entry {
            Some(x) => x,
            None => continue, // handle/vector slot: stays on the frame, not register-carried
        };
        let rb = b.use_var(rb_var);
        let idx = b.ins().iadd_imm(base, k as i64);
        let off = b.ins().imul_imm(idx, STRIDE);
        let addr = b.ins().iadd(rb, off);
        let tag = b.ins().load(types::I8, MemFlagsData::trusted(), addr, 0);
        let expected_tag = if is_float {
            TAG_FLOAT as i64
        } else {
            TAG_INT as i64
        };
        let ok = b.ins().icmp_imm(IntCC::Equal, tag, expected_tag);
        let cont = b.create_block();
        let __dr = b.ins().iconst(types::I32, 106);
        b.ins().brif(ok, cont, &[], deopt, &[BlockArg::Value(__dr)]);
        b.switch_to_block(cont);
        let bits = b.ins().load(
            types::I64,
            MemFlagsData::trusted(),
            addr,
            PAYLOAD_OFFSET as i32,
        );
        if is_float {
            let f = b.ins().bitcast(types::F64, MemFlagsData::new(), bits);
            b.def_var(var, f);
        } else {
            b.def_var(var, bits);
        }
    }
    // BEAM-style reduction batching for the self-tail loop: an in-register countdown
    // (`TICK_BATCH` iterations) gates the back-edge's preemption poll + epoch guard —
    // one sub+branch per iteration instead of an FFI + TLS ops + a guard load. The
    // poll settles the batch with `brood_rt_tick_n`, so scheduler fairness (reduction
    // accounting) is unchanged; a rebind's epoch bump is observed within one batch
    // (the guard's own contract is "eventually").
    let tick_budget = b.declare_var(types::I64);
    {
        let init = b.ins().iconst(types::I64, emit::TICK_BATCH);
        b.def_var(tick_budget, init);
    }
    // Deopt-resume checkpointing (see `CompiledArm::ckpt_slot`) is active whenever THIS
    // lowering has a journal slot: the small native (`arm.ckpt_slot`) and the
    // leaf-spliced native (its own slot, above the spliced blocks — a deopt out of it
    // resumes in the spliced chunk via `ir::LeafInline::resume`, so its ips are
    // meaningful). The self-spliced native passes `u32::MAX` and keeps the legacy
    // from-ip-0 re-run: its ips match no chunk the VM holds, and its gate admits only
    // pure-arith bodies, so re-running is effect-free.
    let ckpt_active = ckpt_slot != u32::MAX;
    // Entry reset. BOTH journals in this frame are cleared, not just this lowering's:
    // the small and the leaf-spliced layouts occupy disjoint slots and take turns
    // running the same frame, so a journal left by the *other* engine's earlier run
    // would otherwise be read as live by a later resume. Packed 0 = "resume at ip 0
    // with an empty operand stack" — i.e. no journal.
    let mut reset: Option<u32> = None;
    for slot in [ckpt_slot, arm.ckpt_slot] {
        if slot == u32::MAX || reset == Some(slot) {
            continue; // absent, or already reset (the small native, where the two agree)
        }
        reset = Some(slot);
        let idx = b.ins().iadd_imm(base, slot as i64);
        let off = b.ins().imul_imm(idx, STRIDE);
        let rb = b.use_var(rb_var);
        let addr = b.ins().iadd(rb, off);
        let tag = b.ins().iconst(types::I8, TAG_INT as i64);
        let zero = b.ins().iconst(types::I64, 0);
        b.ins().store(MemFlagsData::trusted(), tag, addr, 0);
        b.ins()
            .store(MemFlagsData::trusted(), zero, addr, PAYLOAD_OFFSET as i32);
    }
    b.ins().jump(leader_block[0].unwrap(), &[]);

    // Box an `Op::Int`'s register value into a whole-`Value`'s `(tag_byte, payload_i64)`.
    // An `i64` arithmetic/const/slot value → `Value::Int` (`TAG_INT`, payload as-is). The
    // only *non*-`i64` `Op::Int` is a comparison result (`<`/`<=`/`=`, an `i8` 0/1), and a
    // Brood comparison yields `true`/`false`, **not** the integers 0/1 — so it boxes as a
    // `Value::Bool` (`TAG_BOOL`, the `i8` zero-extended to the payload word). Both payload
    // forms are `i64`, so a materialised operand (a return, a binder, a self-call/call arg)
    // stores / passes correctly. (Without this, returning `(< a b)` produced `Value::Int 1`
    // instead of `true`.)
    // Load frame slot `k` as an unboxed `i64`, tag-checking `Int` first: a non-`Int`
    // operand branches to `deopt` (the VM then runs the arm, where the inline path
    // handles the real shape). Leaves `b` switched to the post-check block. Used by
    // `Local` and the fused `Prim2Slot*` operands alike.
    // Fast path: register-carried param slots (0..carry_argc) skip the tag-check entirely —
    // the entry block already verified Int and `def_var`'d the raw i64; each SelfCall
    // re-`def_var`s on the back-edge. `use_var` gives the current iteration's value without
    // any memory access or branch.
    // Emit `op` on two unboxed `i64` operands already in `(x, y)` order. Add/Sub/Mul use
    // the overflow-checked Cranelift ops and branch to `deopt` on signed overflow — the
    // VM's inline path defers an overflowing `i64` op to the native, which promotes to a
    // BigInt (ADR bignums), so deopting here keeps the JIT bit-identical to the VM
    // instead of silently wrapping. Comparisons yield an `I8` 0/1. Leaves `b` switched
    // to the post-check block for the arithmetic ops.

    // ---- The hybrid operand model. ----
    //
    // A logical operand-stack entry is either an unboxed `i64` in an SSA register
    // (`Int` — an arithmetic/const/comparison result, the fast path that keeps tight
    // numeric loops register-resident), or a reference to a frame slot `Slot(k)` whose
    // `Value` lives in `roots[base+k]` — read lazily, type unknown. A `Slot` is the only
    // way a *handle* (a Pair, etc.) can sit on the operand stack: handles must stay in
    // `roots` so the moving collector can see and relocate them (a handle in a register
    // would go stale across a safepoint). Consumers that need an `i64` (arithmetic, a
    // branch condition, a block-arg) materialise a `Slot` with a tag-checked load; ones
    // that move a whole `Value` (a binder, a self-call arg, the return) copy the 16-byte
    // slot verbatim, so a handle round-trips untouched.
    // A third form, `Handle(w0,w1,w2)`, holds a freshly-produced `Value` (a `cons` pair, a
    // `car`/`cdr` result) as its three 24-byte words in registers. It's **transient** —
    // produced and consumed within a block (stored to a slot by a self-call/binder, returned,
    // or tag-checked back to an int), never crossing the loop back-edge live, which is the
    // only safepoint — so the moving GC never sees a handle in a register.
    let done_block = leader_block[len]?;
    // Store an unboxed scalar `Op::Int` value into frame slot `k`, boxing it as `Int` or
    // (for a comparison `i8`) `Bool` via `box_scalar`.
    // Copy the whole `Value` from frame slot `src` to slot `dst` (handle-safe — moves the
    // bytes verbatim, no interpretation). A `Value` is `STRIDE` bytes (`#[repr(C, u8)]`):
    // it must copy **every** i64 word, not just tag+payload — `Value::Pid { node, id }`
    // (and any future 2-word-payload variant) carries `id` in the third word at offset 16,
    // which a tag+payload-only copy would drop and corrupt.
    // Read an operand as its three `Value` words `[w0, w1, w2]` — for a self-call arg, a
    // binder, a return, or a `cons`/`car`/`cdr` operand. An `Int` boxes to `[Int-tag, v, 0]`
    // (the third word is irrelevant to an Int); a `Slot` loads all three; a `Handle` is
    // already those registers. No tag-check — this moves a whole `Value` verbatim.
    // Operand-materialization family — thin wrappers over the free fns in `emit.rs`
    // (extracted for the emit-loop decomposition). Each captures `frame`, so the
    // ~35 call sites below stay unchanged; the bulky bodies live in `emit.rs`.
    let read_words = |b: &mut FunctionBuilder, op: Op| -> [cranelift_codegen::ir::Value; 3] {
        emit::read_words(b, op, frame)
    };
    let as_block_arg =
        |b: &mut FunctionBuilder, op: Op, idx: usize| -> cranelift_codegen::ir::Value {
            emit::as_block_arg(b, op, idx, frame)
        };
    // Integer-vs-float dispatch for a binary op: an operand is float if it's an
    // `Op::Float`, or a `Slot` the profile/tracking marks float. (`Op::Int`/`Handle` are
    // integer/non-number.)
    // Float arith / comparison. Arith → `Op::Float`; a comparison → an `i8` boxed as a
    // Bool (`Op::Int`, exactly like the integer compares). The integer-only ops
    // (`rem`/`quot`) and `=` aren't lowered for floats → `None` bails the arm to the VM.
    // Store an operand into frame slot `dst`: an `Int` is boxed; a `Slot` is copied
    // verbatim; a `Handle` stores its three words (so a handle binder / self-call arg /
    // return keeps its type).
    // Also tracks `slot_float[dst]` so a later read of `dst` picks the right arith: a
    // float store marks it float, an int/handle store clears it, a slot-copy inherits the
    // source's flag. (Lets let-binders — nil at the entry snapshot — get their type from
    // the body's writes, which precede their reads in the single lowering pass.)
    // Mirror of `set_slot_float` for the bool flag. A store of any kind updates *both*
    // (a slot holds one type), so a later read picks the right block-arg representation.
    let store_op = |b: &mut FunctionBuilder, dst: i64, op: Op| emit::store_op(b, dst, op, frame);
    // Return-via-roots: place the single result in `roots[base]` and jump to the
    // param-less Done block. The result is a whole `Value`, so it can be a handle.
    let exit_done = |b: &mut FunctionBuilder, op: Op| {
        store_op(b, 0, op);
        b.ins().jump(done_block, &[]);
    };
    // Call a handle op (`brood_rt_{cons,car,cdr}`) with the out-pointer ABI: pass the
    // scratch slot's address + the operand words, then read the result `Value`'s three
    // words back into a `Handle`. The result rides in registers only until it's consumed
    // (a store / return) — no safepoint in between — so the GC never sees it.
    // Runtime-call context bundle for the extracted call/read helpers (`emit.rs`).
    let funcs = emit::Funcs {
        ptr_ty,
        heap,
        out_slot,
        error,
        vnbase: vnbase_ref,
        vobase: vobase_ref,
        vref: vref_ref,
        car: car_ref,
        cdr: cdr_ref,
        cons: cons_ref,
        makevec2: makevec2_ref,
        makevecn: makevecn_ref,
        thas: thas_ref,
        tget: tget_ref,
        tput: tput_ref,
        rb: rb_ref,
        globic: globic_ref,
        pushn: pushn_ref,
        callslow: callslow_ref,
        natfl: natfl_ref,
        flbase: flbase_ref,
        fastframe: fastframe_ref,
        sp: sp_ref,
        tickn: tickn_ref,
        #[cfg(debug_assertions)]
        dbg_staging: dbg_staging_ref,
    };
    // `call_handle`/`vector_ref`/`table_prim`/`eq_dispatch`/`inline_vec_ref` now live in
    // `emit.rs` and are called directly by the extracted prim arm bodies (`prim.rs`).

    // For each leader, which of its operand-stack block params carry a boolean (so the
    // entry reconstruction tags them `Op::Bool`, not `Op::Int`). Populated by the jump
    // sites (`Jump`/`JumpIfFalse`/leader fall-through), which run before the target block is
    // translated (forward edges, in ip order) — so the flags are set by the time the target
    // is reached. A back-edge target with params would see no flags and default to `Int`;
    // self-tail back-edges target the param-less leader 0, so this doesn't arise in practice.
    let mut bool_param: Vec<Option<Vec<emit::ParamRepr>>> = vec![None; len + 1];
    // Edge typing at joins is handled by `control::record_block_flags` (imported).

    // Translate each leader block in ip order.
    for ip in 0..len {
        let Some(blk) = leader_block[ip] else {
            continue;
        };
        b.switch_to_block(blk);
        let params: Vec<cranelift_codegen::ir::Value> = b.block_params(blk).to_vec();
        let mut stack: Vec<Op> = params
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                match bool_param[ip]
                    .as_ref()
                    .and_then(|f| f.get(i).copied())
                    .unwrap_or(emit::ParamRepr::Int)
                {
                    emit::ParamRepr::Bool => Op::Bool(v),
                    // KI-49: the value was never materialised into the arg word — it lives
                    // in frame slot `k`, which every predecessor agreed on.
                    emit::ParamRepr::Slot(k) => Op::Slot(k),
                    emit::ParamRepr::Int => Op::Int(v),
                }
            })
            .collect();
        let mut j = ip;
        loop {
            match &code[j] {
                Inst::Const(cv) => match cv.load().unpack() {
                    ValueRef::Int(n) => stack.push(Op::Int(b.ins().iconst(types::I64, n))),
                    // A float literal (`4.0`, `2.0` in mandelbrot's `esc`) → an unboxed f64.
                    ValueRef::Float(f) => stack.push(Op::Float(b.ins().f64const(f))),
                    // `nil` (e.g. bintree `make`'s `(= d 0)` then-branch): a scalar atom,
                    // tag 0 / no payload — push it as a constant 3-word handle. A consumer
                    // that wants an int (`as_int`) tag-checks and deopts; a binder/return
                    // copies the words verbatim (`store_op`), which is all `make` does.
                    ValueRef::Nil => {
                        let z = b.ins().iconst(types::I64, 0);
                        stack.push(Op::Handle(z, z, z));
                    }
                    ValueRef::Bool(bv) => {
                        let v = b.ins().iconst(types::I64, if bv { 1 } else { 0 });
                        stack.push(Op::Bool(v));
                    }
                    _ => {
                        // GC-movable heap handle (Str, BigInt, Pair, Fn, …): call
                        // `brood_rt_const_load(cv_ptr, out)` at the point of use to get
                        // the live bits (updated by `runtime_collect` via `ConstVal::rewrite`).
                        // The ConstVal lives in the arm's chunk behind an Arc<CompiledArm>,
                        // so the address is stable for the JIT function's lifetime.
                        let cv_ptr = b.ins().iconst(ptr_ty, cv as *const ConstVal as i64);
                        let out_addr = b.ins().stack_addr(ptr_ty, out_slot, 0);
                        b.ins().call(const_load_ref, &[cv_ptr, out_addr]);
                        let w0 = b.ins().stack_load(types::I64, out_slot, 0);
                        let w1 = b
                            .ins()
                            .stack_load(types::I64, out_slot, PAYLOAD_OFFSET as i32);
                        let w2 =
                            b.ins()
                                .stack_load(types::I64, out_slot, PAYLOAD_OFFSET as i32 + 8);
                        stack.push(Op::Handle(w0, w1, w2));
                    }
                },
                // Lazy: push a reference to the frame slot. Consumers tag-check it to an int
                // (arithmetic / a branch) or copy it whole (a binder / arg / return), so a
                // handle in the slot rides along untouched.
                Inst::Local(i) => stack.push(Op::Slot(*i)),
                // A free-global read (a call's callee, or a value-position global). A
                // `GlobalIc` resolves through the per-`site` global inline cache
                // (`brood_rt_global_ic` — a cached read on a process-global env, no `env_get`
                // walk per call; this is what keeps a hot recursive callee like `fib`
                // resolving itself cheaply). A bare `Global` (no site) falls back to
                // `brood_rt_global`. Late binding holds via the cache's epoch stamp; an
                // unbound symbol parks an error and exits via `error` (outcome 3). The
                // resolved value is an arbitrary `Value`, so it's a `Handle`.
                Inst::Global(s) | Inst::GlobalIc { sym: s, .. } => {
                    // Hoisted invariant global vector: push the entry-resolved base + words
                    // (no per-iteration global read). The back-edge epoch guard deopts on a
                    // rebind, so this stays bit-identical to the VM's late binding. Falls
                    // through to the normal loop tail like the resolved-`Handle` path.
                    if let Some(&(slots, flag, w0, w1, w2)) = hoisted_table.get(s) {
                        // Hoisted dense table: words + slot region for the inline
                        // table ops (a non-table-op consumer reads just the words).
                        stack.push(Op::HoistedTable {
                            slots,
                            flag,
                            w0,
                            w1,
                            w2,
                        });
                    } else if let Some(&(w0, w1, w2)) = hoisted_scalar.get(s) {
                        // Hoisted scalar global (#1): the value was resolved once at entry;
                        // reuse its words as a `Handle` (no per-access `brood_rt_global_ic`).
                        // The back-edge epoch guard deopts on a rebind (late-binding-exact).
                        stack.push(unbox_float_global(
                            &mut b,
                            *s,
                            Op::Handle(w0, w1, w2),
                            frame,
                            float_globals,
                        ));
                    } else if let Some(&(ptr, len, w0, w1, w2)) = hoisted_global.get(s) {
                        stack.push(Op::HoistedVec {
                            ptr,
                            len,
                            w0,
                            w1,
                            w2,
                        });
                    } else {
                        let sym = b.ins().iconst(types::I32, *s as i64);
                        let out_addr = b.ins().stack_addr(ptr_ty, out_slot, 0);
                        let c = if let Inst::GlobalIc { site, .. } = &code[j] {
                            let site_v = b.ins().iconst(types::I32, *site as i64);
                            b.ins().call(globic_ref, &[heap, out_addr, sym, site_v])
                        } else {
                            b.ins().call(glob_ref, &[heap, out_addr, sym])
                        };
                        let status = b.inst_results(c)[0];
                        let cont = b.create_block();
                        b.ins().brif(status, error, &[], cont, &[]);
                        b.switch_to_block(cont);
                        let w0 = b.ins().stack_load(types::I64, out_slot, 0);
                        let w1 = b
                            .ins()
                            .stack_load(types::I64, out_slot, PAYLOAD_OFFSET as i32);
                        let w2 =
                            b.ins()
                                .stack_load(types::I64, out_slot, PAYLOAD_OFFSET as i32 + 8);
                        stack.push(unbox_float_global(
                            &mut b,
                            *s,
                            Op::Handle(w0, w1, w2),
                            frame,
                            float_globals,
                        ));
                    }
                }
                Inst::Call {
                    argc,
                    tail,
                    site,
                    head,
                    staged,
                    pos: _,
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
                    match call::emit_call(
                        &mut b,
                        &mut stack,
                        &mut spill_next,
                        *argc,
                        *tail,
                        *site,
                        *head,
                        spill_base,
                        reserve,
                        epoch_ptr,
                        tailcall,
                        frame,
                        funcs,
                    )? {
                        call::Flow::Break => break,
                        call::Flow::Fall => {}
                    }
                }
                Inst::Pop => {
                    // A non-final `do` form, evaluated for effect: drop its value.
                    stack.pop()?;
                }
                Inst::SetLocal(i) => {
                    // A `let`/`letrec` binder → frame slot `i`. A `Slot` operand (possibly a
                    // handle) is copied verbatim; an `Int` is boxed as `Int`, a comparison
                    // `i8` as `Bool` (`store_op`/`box_scalar`). let-slots are scratch,
                    // distinct from the loop-carried param slots and dominated by this store,
                    // so a deopt's VM re-run recomputes the binding before any read sees a
                    // stale slot.
                    //
                    // BUT `Inst::Local` pushes a *lazy* `Op::Slot(i)` that re-reads the slot
                    // at its consumer. The bytecode reuses one slot index across sibling
                    // `let` scopes (sound for the VM, whose operand stack holds *values*), so
                    // a still-pending `Op::Slot(i)` from an earlier binding would, after this
                    // overwrite, read THIS binding instead of the value that was live when its
                    // `Local` was pushed. Materialise every such pending reference to the
                    // slot's *current* (pre-store) value first — preserving its exact type so
                    // consumers behave identically to the lazy read they replace. (Fuzzer
                    // seed 20108: `(- (let (a A) a) (let (b B) b))` reused slot 1 → 0 not A-B.)
                    let si = *i;
                    for op in stack.iter_mut() {
                        if !matches!(*op, Op::Slot(k) if k == si) {
                            continue;
                        }
                        let w = read_words(&mut b, Op::Slot(si));
                        *op = if slot_float.borrow().get(si).copied().unwrap_or(false) {
                            Op::Float(b.ins().bitcast(types::F64, MemFlagsData::new(), w[1]))
                        } else if slot_bool.borrow().get(si).copied().unwrap_or(false) {
                            Op::Bool(w[1])
                        } else {
                            Op::Handle(w[0], w[1], w[2])
                        };
                    }
                    let op = stack.pop()?;
                    store_op(&mut b, *i as i64, op);
                }
                Inst::Prim1 { op, .. } => {
                    prim::emit_prim1(&mut b, &mut stack, op, pair_bases, frame, funcs)?;
                }
                Inst::MakeVector(n) => {
                    prim::emit_make_vector(&mut b, &mut stack, *n, frame, funcs)?;
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
                Inst::Prim2SlotSlot {
                    op,
                    map,
                    slot_a,
                    slot_b,
                    ..
                } => {
                    prim::emit_prim2_slot_slot(
                        &mut b, &mut stack, op, *map, *slot_a, *slot_b, &hoisted, frame, funcs,
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
                        len,
                        done_block,
                        &leader_block,
                        &mut bool_param,
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
                    control::emit_jump_if_false(
                        &mut b,
                        &mut stack,
                        *t,
                        j,
                        &leader_block,
                        &mut bool_param,
                        frame,
                    )?;
                    break;
                }
                other => {
                    // The emit loop and `chunk_in_jit_subset` can disagree: the subset
                    // rule admits an opcode class, then emit refuses a particular shape
                    // of it. Before this, that disagreement was invisible — the arm just
                    // came back BAILED with no reason anywhere.
                    return trace_lower_bail_inst(
                        arm,
                        crate::eval::compile::jit_plan::codegen::inst_opcode_name(other),
                    );
                }
            }
            // Deopt-resume checkpoint (see `CompiledArm::ckpt_slot`): a non-tail
            // call just completed — journal the abstract operand stack (it contains
            // only GC-safe shapes here: unboxed scalars, frame slots, and the fresh
            // call result) into the reserved frame slots plus the packed
            // `(resume_ip << 16) | depth`, so a LATER deopt in this activation
            // resumes right here instead of re-running (and re-effecting) from ip 0.
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
                let ckpt_base = ckpt_slot as i64 + 1;
                for (k, &op) in stack.iter().enumerate() {
                    store_op(&mut b, ckpt_base + k as i64, op);
                }
                let packed = (((j as i64) + 1) << 16) | stack.len() as i64;
                let pv = b.ins().iconst(types::I64, packed);
                store_int(&mut b, ckpt_slot as i64, pv, frame);
            }
            j += 1;
            if j == len {
                // Fall off the end into Done: return the single result via roots[base].
                if stack.len() != 1 {
                    return None;
                }
                exit_done(&mut b, stack[0]);
                break;
            }
            if is_leader[j] {
                let flags: Vec<emit::ParamRepr> = stack
                    .iter()
                    .enumerate()
                    .map(|(i, &op)| emit::param_repr(&b, op, i, frame))
                    .collect();
                if record_block_flags(&mut bool_param[j], flags) {
                    let args: Vec<BlockArg> = stack
                        .iter()
                        .enumerate()
                        .map(|(i, &op)| BlockArg::Value(as_block_arg(&mut b, op, i)))
                        .collect();
                    b.ins().jump(leader_block[j]?, &args);
                } else {
                    // Type-mixed join (see `record_block_flags`): deopt to the VM.
                    let __dr = b.ins().iconst(types::I32, 107);
                    b.ins().jump(deopt, &[BlockArg::Value(__dr)]);
                }
                break;
            }
        }
    }

    // Done block: the result was already stored into `roots[base]` by the exiting block
    // (return-via-roots, see `exit_done`), so this just signals normal completion.
    b.switch_to_block(done_block);
    let zero = b.ins().iconst(types::I64, 0);
    b.ins().return_(&[zero]);
    // Deopt: an operand wasn't an Int — return 1, the caller runs the arm on the VM.
    b.switch_to_block(deopt);
    let deopt_reason = b.block_params(deopt)[0];
    b.ins().call(nd_ref, &[heap, deopt_reason]);
    let one = b.ins().iconst(types::I64, 1);
    b.ins().return_(&[one]);
    // Preempt: the reduction budget was spent at a back-edge — return 2. The frame slots
    // (in roots) hold the next iteration's args, so the driver resumes the arm on the VM.
    b.switch_to_block(preempt);
    let two = b.ins().iconst(types::I64, 2);
    b.ins().return_(&[two]);
    // Error: a JIT'd call / global read raised — return 3. The error is parked in
    // `JIT_PENDING_ERROR`; `vm_run_bc` takes it and propagates (no VM re-run).
    b.switch_to_block(error);
    let three = b.ins().iconst(types::I64, 3);
    b.ins().return_(&[three]);
    // Tail call: the callee + args are staged on `roots` — return 4. `vm_run_bc`
    // dispatches them with `tail = true` and reuses this frame (`jit_dispatch_tail`).
    b.switch_to_block(tailcall);
    let four = b.ins().iconst(types::I64, 4);
    b.ins().return_(&[four]);
    b.seal_all_blocks();
    b.finalize();

    // IR inspection (debug): `BROOD_JIT_DUMP_IR=1` dumps each fully-lowered arm's
    // bytecode + Cranelift CLIF to stderr — the tool for diagnosing a JIT miscompile
    // (read the IR, diff against the intended semantics). Read once; the compile path
    // is cold (once per arm) and zero cost when unset.
    {
        if jit_dump_ir_enabled() {
            // A compact bytecode fingerprint (opcode names — `Inst` has no `Debug`,
            // and `ConstVal`/`Value` are deliberately not `Debug`) to correlate the
            // CLIF to a source arm, then the CLIF itself.
            let ops: Vec<&str> = code.iter().map(inst_opcode_name).collect();
            eprintln!(
                "[jit-ir] ===== arm: {} ({}) ckpt_slot: {} insts: {} =====",
                code.len(),
                arm.dbg_name
                    .map(crate::core::value::symbol_name_ref)
                    .unwrap_or("<closure>"),
                ckpt_slot,
                ops.join(" ")
            );
            // Per-Call (site, head) so the CLIF can be correlated to a source arm.
            for i in code.iter() {
                if let Inst::Call {
                    site,
                    head,
                    argc,
                    tail,
                    ..
                } = i
                {
                    let hn = match head {
                        Some(h) => crate::core::value::symbol_name(*h),
                        None => "<computed>".to_string(),
                    };
                    eprintln!("[jit-ir]   Call site={site} head={hn} argc={argc} tail={tail}");
                }
            }
            eprintln!("{}", ctx.func.display());
        }
    }

    m.define_function(id, &mut ctx).ok()?;
    // DEBUG (bug #2): dump this arm's finalized machine code (hex bytes) for offline
    // disassembly, when `BROOD_DUMP_CODE=<substr>` matches the arm's defn name. gdb can't
    // read JIT code pages at the crash pc (execute-only / superseded), so capture the bytes
    // here at compile time and correlate `pc - entry` offline. Captured before clear_context.
    #[cfg(debug_assertions)]
    let dump_name: Option<(String, usize)> = {
        match std::env::var("BROOD_DUMP_CODE") {
            Ok(want) if !want.is_empty() => {
                let name = arm
                    .dbg_name
                    .map(crate::core::value::symbol_name)
                    .unwrap_or_else(|| format!("<anon:{}insts>", code.len()));
                // `insts:N` matches by bytecode length (to catch anonymous arms); else by name.
                let matched = want.split(',').any(|w| {
                    if let Some(n) = w.strip_prefix("insts:") {
                        n.parse::<usize>().ok() == Some(code.len())
                    } else {
                        !w.is_empty() && name.contains(w)
                    }
                });
                if matched {
                    // Capture the code length now (compiled_code is cleared below); read the
                    // RELOCATED bytes from the finalized entry pointer after finalize, so call
                    // targets are real addresses (not 0x0 placeholders).
                    ctx.compiled_code().map(|cc| (name, cc.code_buffer().len()))
                } else {
                    None
                }
            }
            _ => None,
        }
    };
    m.clear_context(&mut ctx);
    m.finalize_definitions().ok()?;
    let entry = m.get_finalized_function(id);
    #[cfg(debug_assertions)]
    if let Some((name, len)) = dump_name {
        let inlined = inline.is_some();
        // SAFETY: `entry` is a finalized function of `len` bytes in r-x JIT memory.
        let bytes: &[u8] = unsafe { std::slice::from_raw_parts(entry, len) };
        eprintln!(
            "[dump-code] arm='{name}' inlined={inlined} entry={:#x} len={len} hex={}",
            entry as usize,
            bytes
                .iter()
                .map(|b| format!("{:02x}", b))
                .collect::<String>()
        );
    }
    Some(entry)
}
