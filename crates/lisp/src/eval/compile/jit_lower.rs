use super::*;

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

/// Frame slots reserved for the JIT to **spill call-result handles** that must survive
/// a later call's safepoint — the two-call-recursion shape (`fib`'s
/// `(+ (fib …) (fib …))`, bintree's `check`) where the first call's result is a heap
/// handle sitting in a register below the second call. Spilling it into a GC-visible
/// frame slot (rather than bailing to the VM) lets the arm lower. Reserved iff the arm
/// has ≥2 non-tail calls — the only shape that can leave a handle below a call. The VM
/// never references these slots; `push_frame` nil-inits them like any other. Computed
/// identically at arm construction (to size the frame) and in `jit_lower_arm` (to place
/// spills); if the predicate ever under-counts, the lowering bails safely rather than
/// corrupting. `0` under `--without-jit`, so that build's frames are unchanged.
#[cfg(feature = "jit")]
pub(crate) fn jit_spill_reserve(code: &[Inst]) -> usize {
    if non_tail_call_count(code) < 2 {
        return 0;
    }
    // Reserve **only** for arms that are actually JIT-lowerable — every opcode in the
    // integer subset `jit_lower_arm` accepts. The reserve adds a frame slot that the VM
    // nil-inits on every activation, so reserving for an arm that never lowers (a prelude
    // function with out-of-subset ops — `send`/`receive`/`spawn` machinery, string/map
    // work — which the JIT can't compile anyway) is pure dead weight on the interpreter
    // path. Blanket-reserving every ≥2-non-tail-call arm regressed `spawn` ~1.9× (20 000
    // procs paying bloated prelude frames), even under `BROOD_VM=0`. Gating on the subset
    // keeps the reserve on `fib`-shaped arms (which lower and win) and off everything else.
    if !chunk_in_jit_subset(code) {
        return 0;
    }
    // How many spill slots `jit_lower_arm`'s monotonic `spill_next` can reach. A spill
    // fires when a non-tail call's safepoint finds a live `Op::Handle` *below* its
    // operands; the spill rewrites that handle to an `Op::Slot`, so **each handle is
    // spilled at most once** (a `Slot` is never re-spilled). Hence total spills ≤ the
    // number of handle-*producing* instructions, and the chronologically-last handle is
    // never spilled (no later safepoint can cross it — it's consumed or returned-via-
    // roots), giving the tight bound `producers − 1`.
    //
    // Handle producers in the lowering: a non-tail Brood→Brood `Call` (its `Value`
    // result), a `MakeVector` (`[a b]`), a `Prim1::First|Rest` (car/cdr deref → Handle),
    // and a `Cons` prim. `Prim1::IsNil|IsPair` produce `Op::Int` (tag-only), not a
    // Handle, so they are not counted. For plain two-call recursion (`fib`) producers == 2
    // → reserve 1, **bit-identical to the prior hardcoded `1`** — so no arm that lowered
    // before changes. A deeper-nested body — an inlined / bounded-unrolled `fib` arm or a
    // structure-walking two-call arm like bintree's `check` — has more simultaneously-live
    // call results, so it reserves one slot per producer beyond the last.
    let producers = code
        .iter()
        .filter(|i| {
            matches!(
                i,
                Inst::Call { tail: false, .. }
                    | Inst::MakeVector(_)
                    | Inst::Prim1 {
                        op: PrimOp1::First | PrimOp1::Rest,
                        ..
                    }
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
        })
        .count();
    producers.saturating_sub(1)
}
#[cfg(not(feature = "jit"))]
fn jit_spill_reserve(_code: &[Inst]) -> usize {
    0
}

/// True if the arm is eligible for register-carry of loop-carried integer params.
/// In a pure-arithmetic self-tail loop (no non-tail Calls, no handle-producing ops), every
/// param slot at the `SelfCall` back-edge is always `Value::Int`. We can carry those i64s
/// in Cranelift `Variable`s instead of boxing to `roots` every iteration: reads skip the
/// per-access tag-check + address arithmetic + two memory ops. The `roots` stores at
/// `SelfCall` are kept (for deopt correctness); only reads change.
#[cfg(feature = "jit")]
fn int_carry_eligible(code: &[Inst]) -> bool {
    code.iter().any(|i| matches!(i, Inst::SelfCall { .. }))
        && !code.iter().any(|i| {
            matches!(
                i,
                Inst::Call { tail: false, .. }
                    | Inst::Prim1 {
                        op: PrimOp1::First | PrimOp1::Rest,
                        ..
                    }
                    | Inst::MakeVector(_)
                    | Inst::Prim2 { op: PrimOp::Cons, .. }
                    | Inst::Prim2SlotSlot { op: PrimOp::Cons, .. }
                    | Inst::Prim2SlotInt { op: PrimOp::Cons, .. }
            )
        })
}

/// Count of non-tail Brood→Brood calls in `code` — the shape that needs a handle spill
/// (≥2) and drives the spill-reserve / lowering gates.
#[cfg(feature = "jit")]
fn non_tail_call_count(code: &[Inst]) -> usize {
    code.iter()
        .filter(|i| matches!(i, Inst::Call { tail: false, .. }))
        .count()
}


/// True iff every opcode in `code` is in the integer JIT subset — i.e. `jit_lower_arm`
/// could lower this arm (modulo the handle-spill, which is what the reserve enables).
/// Mirrors `jit_lower_arm`'s pre-bail check; the two must stay in sync. Used by
/// [`jit_spill_reserve`] so only genuinely-lowerable arms get spill frame slots.
#[cfg(feature = "jit")]
fn chunk_in_jit_subset(code: &[Inst]) -> bool {
    let in_subset_op = |op: &PrimOp| {
        matches!(
            op,
            PrimOp::Add
                | PrimOp::Sub
                | PrimOp::Mul
                | PrimOp::Lt
                | PrimOp::Le
                | PrimOp::Eq
                | PrimOp::Rem
                | PrimOp::Quot
                | PrimOp::Div
                | PrimOp::VectorRef
                | PrimOp::Cons
                | PrimOp::Max
                | PrimOp::Min
                | PrimOp::BitAnd
                | PrimOp::BitOr
                | PrimOp::BitXor
        )
        // `Cons` is admitted: the lowering calls `brood_rt_cons` (same bump-allocate
        // path as `brood_rt_make_vector2`, which works) and reads all 3 result words
        // back as a `Handle`. The earlier miscompile (surfaced in `jit_cons_test.blsp`)
        // was fixed with the correct lowering; the old bail is no longer needed.
    };
    code.iter().all(|inst| match inst {
        Inst::Const(_) => true,
        Inst::Local(_)
        | Inst::Jump(_)
        | Inst::JumpIfFalse(_)
        | Inst::SelfCall { .. }
        | Inst::Pop
        | Inst::SetLocal(_)
        | Inst::Global(_)
        | Inst::GlobalIc { .. }
        | Inst::Prim1 { .. }
        | Inst::Call { .. } => true,
        Inst::Prim2 { op, .. } | Inst::Prim2SlotSlot { op, .. } | Inst::Prim2SlotInt { op, .. } => {
            in_subset_op(op)
        }
        // A 2-element vector literal `[a b]` — lowered via `brood_rt_make_vector2`,
        // the same bump-allocate path as `cons`. Only arity 2 (bintree's `make`);
        // wider literals bail (they'd need a roots-staging variadic helper).
        Inst::MakeVector(n) => *n == 2,
        _ => false,
    })
}

/// Opcode name of an `Inst`, for the `BROOD_JIT_DUMP_IR` fingerprint. `Inst` (and its
/// `ConstVal`/`Value` payloads) are intentionally not `Debug`, so this names the
/// variant without touching the payload. Exhaustive on purpose — a new `Inst` variant
/// must be added here.
#[cfg(feature = "jit")]
fn inst_opcode_name(inst: &Inst) -> &'static str {
    match inst {
        Inst::Const(_) => "Const",
        Inst::Local(_) => "Local",
        Inst::Global(_) => "Global",
        Inst::GlobalIc { .. } => "GlobalIc",
        Inst::Pop => "Pop",
        Inst::SetLocal(_) => "SetLocal",
        Inst::Jump(_) => "Jump",
        Inst::JumpIfFalse(_) => "JumpIfFalse",
        Inst::MakeVector(_) => "MakeVector",
        Inst::MakeMap(_) => "MakeMap",
        Inst::Prim1 { .. } => "Prim1",
        Inst::Prim2 { .. } => "Prim2",
        Inst::Prim2SlotSlot { .. } => "Prim2SlotSlot",
        Inst::Prim2SlotInt { .. } => "Prim2SlotInt",
        Inst::Call { .. } => "Call",
        Inst::SelfCall { .. } => "SelfCall",
        Inst::MakeClosure { .. } => "MakeClosure",
        Inst::TryCatch { .. } => "TryCatch",
    }
}

/// Collect every [`Node::SelfCall`]'s argument slice reachable in `node` (all are
/// tail calls). Used to find which parameter slots a self-recursive arm passes through
/// **unchanged** every iteration, for the JIT's matmul-style loop-invariant hoist.
#[cfg(feature = "jit")]
fn collect_self_call_args<'a>(node: &'a Node, out: &mut Vec<&'a [Node]>) {
    match node {
        Node::SelfCall { args, .. } => out.push(args),
        Node::If(a, b, c) => {
            collect_self_call_args(a, out);
            collect_self_call_args(b, out);
            collect_self_call_args(c, out);
        }
        Node::Do(xs) | Node::Vector(xs) => {
            for x in xs.iter() {
                collect_self_call_args(x, out);
            }
        }
        Node::Map(kvs) => {
            for (k, v) in kvs.iter() {
                collect_self_call_args(k, out);
                collect_self_call_args(v, out);
            }
        }
        Node::Call { callee, args, .. } => {
            collect_self_call_args(callee, out);
            for x in args.iter() {
                collect_self_call_args(x, out);
            }
        }
        Node::LetBind { binds, body } => {
            for (_, n) in binds.iter() {
                collect_self_call_args(n, out);
            }
            collect_self_call_args(body, out);
        }
        Node::MakeClosure { captures, .. } => {
            for (_, n) in captures.iter() {
                collect_self_call_args(n, out);
            }
        }
        Node::Prim2 { a, b, .. } => {
            collect_self_call_args(a, out);
            collect_self_call_args(b, out);
        }
        Node::Prim1 { a, .. } => collect_self_call_args(a, out),
        Node::TryCatch { body, handler, .. } => {
            collect_self_call_args(body, out);
            collect_self_call_args(handler, out);
        }
        Node::Const(_) | Node::Local(_) | Node::Global(_) | Node::GlobalIc { .. } => {}
    }
}

/// Parameter slots a self-recursive arm carries **unchanged** across every back-edge
/// — `SelfCall` arg `k` is exactly `Node::Local(k)` in *every* self-call — i.e. the
/// loop-invariant locals. The JIT hoists an invariant **vector** slot's element base
/// out of the loop (LICM): a load whose source can't be mutated (Brood is immutable,
/// ADR-026) is invariant with no alias analysis. Returns `vec![false; nrequired]` when
/// the arm has no `SelfCall` (not a loop — nothing to hoist).
#[cfg(feature = "jit")]
fn invariant_param_slots(body: &Node, nrequired: usize) -> Vec<bool> {
    let mut calls = Vec::new();
    collect_self_call_args(body, &mut calls);
    if calls.is_empty() {
        return vec![false; nrequired];
    }
    let mut inv = vec![true; nrequired];
    for args in &calls {
        for (k, flag) in inv.iter_mut().enumerate() {
            if !matches!(args.get(k), Some(Node::Local(j)) if *j == k) {
                *flag = false;
            }
        }
    }
    inv
}

/// Free **global** symbols read as the *vector* operand of a `(nth …)` / `vector-ref`
/// (`Node::Prim2 { op: VectorRef, a: Global/GlobalIc }`). A global is loop-invariant
/// within a no-call arm (only another process's `def` can change it, caught by the
/// back-edge epoch guard), so its element base can be hoisted out of the loop exactly
/// like an invariant local vector (§matmul LICM, the global lever — `matmul`'s `(nth b k)`).
#[cfg(feature = "jit")]
fn invariant_global_vecs(node: &Node, out: &mut std::collections::HashSet<Symbol>) {
    if let Node::Prim2 {
        op: PrimOp::VectorRef,
        a,
        ..
    } = node
    {
        match &**a {
            Node::Global(s) | Node::GlobalIc { sym: s, .. } => {
                out.insert(*s);
            }
            _ => {}
        }
    }
    match node {
        Node::If(a, b, c) => {
            invariant_global_vecs(a, out);
            invariant_global_vecs(b, out);
            invariant_global_vecs(c, out);
        }
        Node::Do(xs) | Node::Vector(xs) => {
            for x in xs.iter() {
                invariant_global_vecs(x, out);
            }
        }
        Node::Map(kvs) => {
            for (k, v) in kvs.iter() {
                invariant_global_vecs(k, out);
                invariant_global_vecs(v, out);
            }
        }
        Node::Call { callee, args, .. } => {
            invariant_global_vecs(callee, out);
            for x in args.iter() {
                invariant_global_vecs(x, out);
            }
        }
        Node::SelfCall { args, .. } => {
            for x in args.iter() {
                invariant_global_vecs(x, out);
            }
        }
        Node::LetBind { binds, body } => {
            for (_, n) in binds.iter() {
                invariant_global_vecs(n, out);
            }
            invariant_global_vecs(body, out);
        }
        Node::MakeClosure { captures, .. } => {
            for (_, n) in captures.iter() {
                invariant_global_vecs(n, out);
            }
        }
        Node::Prim2 { a, b, .. } => {
            invariant_global_vecs(a, out);
            invariant_global_vecs(b, out);
        }
        Node::Prim1 { a, .. } => invariant_global_vecs(a, out),
        Node::TryCatch { body, handler, .. } => {
            invariant_global_vecs(body, out);
            invariant_global_vecs(handler, out);
        }
        Node::Const(_) | Node::Local(_) | Node::Global(_) | Node::GlobalIc { .. } => {}
    }
}

/// Compile `arm`'s chunk to a native `extern "C" fn(heap: *mut Heap, base: i64) -> i64`
/// for the Step-A int subset, or `None` to bail to the VM. The compiled fn reads its
/// frame slots from `roots[base..]`, computes in registers, **boxes the result into
/// `roots[base]`**, and returns `0` (Done) or `1` (deopt — an operand wasn't an `Int`).
/// The returned pointer is valid for the life of `jit` (its module owns the code).
#[cfg(feature = "jit")]
pub(crate) fn jit_lower_arm(
    jit: &mut crate::jit::Jit,
    arm: &CompiledArm,
    slot_tags: &[u8],
) -> Option<*const u8> {
    jit_lower_arm_inner(jit, arm, slot_tags, None)
}

/// Lower the **inlined** (deferred upgrade) body of a qualifying recursive arm. Re-derives
/// the spliced body fresh from `arm.body` (the small original — the VM keeps it), compiles
/// an ephemeral chunk, and lowers it against the larger `arm.inline_nslots` frame. Returns
/// the inlined native pointer, or `None` if the spliced body falls out of the JIT subset.
/// Per-engine frame sizing (`active_nslots`) keys on which version `jit_tier` installs.
#[cfg(feature = "jit")]
pub(crate) fn jit_lower_inlined_arm(
    jit: &mut crate::jit::Jit,
    arm: &CompiledArm,
    slot_tags: &[u8],
) -> Option<*const u8> {
    let name = arm.inline_name?;
    let spliced = rederive_inlined_body(&arm.body, name, arm.nrequired, arm.inline_stride)?;
    let chunk = compile_chunk(&spliced)?;
    jit_lower_arm_inner(jit, arm, slot_tags, Some((&spliced, &chunk, arm.inline_nslots)))
}

/// Shared lowering core. `inline` overrides the body/chunk/nslots when lowering the
/// re-derived inlined body; `None` lowers the arm's own (original) body — the small native.
#[cfg(feature = "jit")]
fn jit_lower_arm_inner(
    jit: &mut crate::jit::Jit,
    arm: &CompiledArm,
    slot_tags: &[u8],
    inline: Option<(&Node, &Chunk, usize)>,
) -> Option<*const u8> {
    use crate::core::value::jit_layout::{PAYLOAD_OFFSET, TAG_BOOL, TAG_FLOAT, TAG_INT, TAG_PAIR};
    use cranelift_codegen::ir::{
        condcodes::IntCC, types, AbiParam, BlockArg, InstBuilder, MemFlags, StackSlotData,
        StackSlotKind,
    };
    use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
    use cranelift_module::{Linkage, Module};
    use std::sync::atomic::Ordering;

    // The body/chunk/frame-size this lowering runs against: either the arm's own
    // (original, small — the small native) or a re-derived inlined body (deferred upgrade).
    // `nrequired` is identical for both (inlining doesn't change the param count).
    let (lower_body, chunk, nslots): (&Node, &Chunk, usize) = match inline {
        Some((b, c, ns)) => (b, c, ns),
        None => (&arm.body, arm.chunk.as_ref()?, arm.nslots),
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
                if invariant.get(*slot_a).copied().unwrap_or(false)
                    && !hoist_slots.contains(slot_a)
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
    let slot_bool: std::cell::RefCell<Vec<bool>> =
        std::cell::RefCell::new(vec![false; nslots]);
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
    let spill_base = nslots - reserve;
    let mut spill_next = 0usize;
    // Return-via-roots writes/reads the result at `roots[base]` (slot 0), and the VM hooks
    // read it back the same way — both require slot 0 to exist. A 0-slot arm (a 0-arg,
    // 0-local fn like `(defn k () 7)`) has `base == roots_len`, so `roots[base]` is out of
    // bounds. Such arms are trivial; bail and let the VM run them.
    if nslots == 0 {
        return None;
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
        return None;
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
    let has_tail_call = code.iter().any(|i| matches!(i, Inst::Call { tail: true, .. }));
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
            return None;
        }
    }

    // ---- Block leaders: ip 0, every jump target, the inst after a jump, the `len`
    // "done" block. ----
    let mut is_leader = vec![false; len + 1];
    is_leader[0] = true;
    is_leader[len] = true; // the implicit Done block
    for (ip, inst) in code.iter().enumerate() {
        match inst {
            Inst::Jump(t) | Inst::JumpIfFalse(t) => {
                is_leader[*t] = true;
                if ip + 1 <= len {
                    is_leader[ip + 1] = true;
                }
            }
            // SelfCall jumps back to the loop header (block 0); the inst after it
            // (if any) starts a new (unreachable) block boundary.
            Inst::SelfCall { .. } => {
                if ip + 1 <= len {
                    is_leader[ip + 1] = true;
                }
            }
            _ => {}
        }
    }

    // ---- Operand-stack depth at each leader (abstract interp; all subset stack
    // values are 64-bit-wide, and a comparison `I8` is always consumed by the
    // `JumpIfFalse` in its own block, so it never crosses a boundary). ----
    let mut depth: Vec<Option<i32>> = vec![None; len + 1];
    let mut work = vec![(0usize, 0i32)];
    while let Some((ip, d)) = work.pop() {
        if depth[ip].is_some() {
            continue;
        }
        depth[ip] = Some(d);
        let (mut cur, mut j) = (d, ip);
        loop {
            if j == len {
                break;
            }
            match &code[j] {
                Inst::Jump(t) => {
                    work.push((*t, cur));
                    break;
                }
                Inst::JumpIfFalse(t) => {
                    cur -= 1; // pop the condition
                    work.push((*t, cur));
                    work.push((j + 1, cur));
                    break;
                }
                Inst::SelfCall { argc } => {
                    // Pops argc new args, jumps back to the loop header (block 0).
                    work.push((0, cur - *argc as i32));
                    break;
                }
                Inst::Const(_) | Inst::Local(_) => cur += 1,
                // A global read pushes its resolved value.
                Inst::Global(_) | Inst::GlobalIc { .. } => cur += 1,
                // A **tail** call is terminal — control never falls through it (the arm
                // returns via the driver), so end the walk here. Leaving it as a fall-
                // through would propagate a bogus depth into whatever instruction follows
                // (dead code, or a sibling leader), corrupting that block's param count.
                Inst::Call { tail: true, .. } => break,
                // A non-tail call pushes one result and pops its operands.
                // For a free-global head (head=Some) the callee is resolved via the call IC
                // and is NOT staged on the operand stack — only the `argc` args are: net `1-argc`.
                // For a computed head (head=None) the callee IS staged below the args: net `-argc`.
                Inst::Call { argc, head, .. } => {
                    cur += if head.is_some() { 1 - *argc as i32 } else { -(*argc as i32) };
                }
                // Fused prims read their operands from frame slots / a literal, not the
                // operand stack: net push of 1 (unlike the generic `Prim2`'s pop-2-push-1).
                Inst::Prim2SlotSlot { .. } | Inst::Prim2SlotInt { .. } => cur += 1,
                Inst::Prim2 { .. } => cur -= 1, // pop 2, push 1
                // `first`/`rest`: pop the list operand, push the car/cdr — net 0.
                Inst::Prim1 { .. } => {}
                // `let`/`do` plumbing: a binder stores the top into a frame slot, a
                // non-final `do` form discards it — both pop one.
                Inst::Pop | Inst::SetLocal(_) => cur -= 1,
                _ => break, // unreachable (pre-bailed)
            }
            j += 1;
            if is_leader[j] {
                work.push((j, cur));
                break;
            }
        }
    }

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
    // brood_rt_tick(heap) -> u8  (nonzero = the process should yield)
    let mut tick_sig = m.make_signature();
    tick_sig.params.push(AbiParam::new(ptr_ty));
    tick_sig.returns.push(AbiParam::new(types::I8));
    let tick_id = m
        .declare_function("brood_rt_tick", Linkage::Import, &tick_sig)
        .ok()?;
    // brood_rt_in_capture(heap) -> u8: is this a capture-mode (preemptible) process? Read once
    // at entry to gate the per-back-edge `brood_rt_tick` poll — a non-capture (root) loop skips
    // the FFI, which returns 0 there anyway.
    let mut incap_sig = m.make_signature();
    incap_sig.params.push(AbiParam::new(ptr_ty));
    incap_sig.returns.push(AbiParam::new(types::I8));
    let incap_id = m
        .declare_function("brood_rt_in_capture", Linkage::Import, &incap_sig)
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
    let push_id = m
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
    // Track B / Technique A — the in-IR fast call path. brood_rt_fastlink_base(heap,
    // out_len: *mut u64) -> *const FastLink: base + length of the IR-readable fast-link
    // mirror. brood_rt_fast_frame(heap, out, site, head, argc, nslots, code, env) -> status:
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
    let carry_vars: Vec<(Variable, bool)> = {
        let candidate = if int_carry_eligible(code) {
            code.iter()
                .filter_map(|i| if let Inst::SelfCall { argc } = i { Some(*argc) } else { None })
                .max()
                .unwrap_or(0)
        } else {
            0
        };
        if candidate > 0
            && (0..candidate).all(|k| {
                let t = slot_tags.get(k).copied();
                t == Some(profile_tag_int) || t == Some(profile_tag_float_carry)
            })
        {
            (0..candidate)
                .map(|k| {
                    let is_float =
                        slot_tags.get(k).copied() == Some(profile_tag_float_carry);
                    let ty = if is_float { types::F64 } else { types::I64 };
                    (b.declare_var(ty), is_float)
                })
                .collect()
        } else {
            vec![]
        }
    };
    let rb_ref = m.declare_func_in_func(rb_id, b.func);
    let tick_ref = m.declare_func_in_func(tick_id, b.func);
    let incap_ref = m.declare_func_in_func(incap_id, b.func);
    let car_ref = m.declare_func_in_func(car_id, b.func);
    let cdr_ref = m.declare_func_in_func(cdr_id, b.func);
    let pnbase_ref = m.declare_func_in_func(pnbase_id, b.func);
    let pobase_ref = m.declare_func_in_func(pobase_id, b.func);
    let cons_ref = m.declare_func_in_func(cons_id, b.func);
    let makevec2_ref = m.declare_func_in_func(makevec2_id, b.func);
    let sp_ref = m.declare_func_in_func(sp_id, b.func);
    #[cfg(debug_assertions)]
    let dbg_staging_ref = m.declare_func_in_func(dbg_staging_id, b.func);
    // Declared for ad-hoc slot-read validation during bug hunts (calls removed from
    // read_words — they perturbed codegen and masked the bug they were chasing).
    #[cfg(debug_assertions)]
    let _dbg_check_slot_ref = m.declare_func_in_func(dbg_check_slot_id, b.func);
    let push_ref = m.declare_func_in_func(push_id, b.func);
    let glob_ref = m.declare_func_in_func(glob_id, b.func);
    let globic_ref = m.declare_func_in_func(globic_id, b.func);
    let callslow_ref = m.declare_func_in_func(callslow_id, b.func);
    let flbase_ref = m.declare_func_in_func(flbase_id, b.func);
    let fastframe_ref = m.declare_func_in_func(fastframe_id, b.func);
    let vref_ref = m.declare_func_in_func(vref_id, b.func);
    let vbase_ref = m.declare_func_in_func(vbase_id, b.func);
    let gepochptr_ref = m.declare_func_in_func(gepochptr_id, b.func);
    let const_load_ref = m.declare_func_in_func(const_load_id, b.func);
    // Whether the arm allocates (`cons`) — gates the back-edge GC safepoint that bounds
    // the nursery. (`car`/`rest` don't allocate.)
    let has_cons = code.iter().any(|i| {
        matches!(
            i,
            Inst::Prim2 {
                op: PrimOp::Cons,
                ..
            } | Inst::Prim2SlotSlot {
                op: PrimOp::Cons,
                ..
            } | Inst::Prim2SlotInt {
                op: PrimOp::Cons,
                ..
            } | Inst::MakeVector(_)
        )
    });

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
    // A scratch `Value`-sized stack slot the handle / call / global ops write their result
    // into (the out-pointer ABI). One per arm, reused: each result is read straight back
    // into registers before the next op.
    let out_slot = b.create_sized_stack_slot(StackSlotData::new(
        StackSlotKind::ExplicitSlot,
        STRIDE as u32,
        3,
    ));

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
    let mut entry_epoch: Option<cranelift_codegen::ir::Value> = None;
    // Fetch the global-epoch counter's address once here in the entry block (which dominates
    // every loop/call block) when the arm reads the epoch on a hot path — a hoisted-global
    // back-edge guard, or an icall epoch check per call. Those sites then do a raw load instead
    // of a `brood_rt_global_epoch` FFI call each iteration/call (the call was ~20% of `loop`).
    let epoch_ptr: Option<cranelift_codegen::ir::Value> = {
        let needs = !hoist_globals.is_empty()
            || !hoist_scalar_globals.is_empty()
            || (icall_enabled()
                && code
                    .iter()
                    .any(|i| matches!(i, Inst::Call { tail: false, head: Some(_), .. })));
        if needs {
            let c = b.ins().call(gepochptr_ref, &[heap]);
            Some(b.inst_results(c)[0])
        } else {
            None
        }
    };

    // Capture-mode flag, read once at entry (when the arm has a self-tail back-edge): a
    // non-capture (root-thread) loop skips the per-iteration `brood_rt_tick` poll, which returns
    // 0 there anyway. Capture mode is constant for the arm's whole execution, so one read
    // suffices; the capture path keeps polling every iteration (preemption fairness unchanged).
    let capture_active: Option<cranelift_codegen::ir::Value> =
        if code.iter().any(|i| matches!(i, Inst::SelfCall { .. })) {
            let c = b.ins().call(incap_ref, &[heap]);
            Some(b.inst_results(c)[0])
        } else {
            None
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
            matches!(i, Inst::Prim1 { op: PrimOp1::First | PrimOp1::Rest, .. })
        });
        let has_alloc_safepoint = code.iter().any(|i| {
            matches!(
                i,
                Inst::Prim2 { op: PrimOp::Cons, .. }
                    | Inst::Prim2SlotSlot { op: PrimOp::Cons, .. }
                    | Inst::Prim2SlotInt { op: PrimOp::Cons, .. }
                    | Inst::MakeVector(_)
            )
        });
        // A non-tail Call is a GC safepoint: minor_collect replaces `self.local` entirely
        // (std::mem::take), so any pointer to `local.pairs` cached before the call is
        // invalid after it. Only inline when there are no such safepoints.
        let has_call_safepoint = code.iter().any(|i| matches!(i, Inst::Call { tail: false, .. }));
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
        let len_slot = b.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            8,
            3,
        ));
        let len_addr = b.ins().stack_addr(ptr_ty, len_slot, 0);
        for &slot in &hoist_slots {
            let roots_base = b.use_var(rb_var);
            let i = b.ins().iadd_imm(base, slot as i64);
            let o = b.ins().imul_imm(i, STRIDE);
            let addr = b.ins().iadd(roots_base, o);
            let w0 = b.ins().load(types::I64, MemFlags::new(), addr, 0);
            let w1 = b
                .ins()
                .load(types::I64, MemFlags::new(), addr, PAYLOAD_OFFSET as i32);
            let w2 = b
                .ins()
                .load(types::I64, MemFlags::new(), addr, PAYLOAD_OFFSET as i32 + 8);
            let c = b.ins().call(vbase_ref, &[heap, w0, w1, w2, len_addr]);
            let ptr = b.inst_results(c)[0];
            // null ptr ⇒ slot isn't a vector ⇒ deopt (VM runs the arm; same result).
            let cont = b.create_block();
            b.ins().brif(ptr, cont, &[], deopt, &[]);
            b.switch_to_block(cont);
            let vlen = b.ins().load(types::I64, MemFlags::new(), len_addr, 0);
            hoisted.insert(slot, (ptr, vlen));
        }
        // Resolve each hoisted global once (sorted for deterministic codegen). Unbound ⇒
        // `error` (matches the VM's unbound-global error); non-vector ⇒ `deopt`.
        let mut gsyms: Vec<Symbol> = hoist_globals.iter().copied().collect();
        gsyms.sort_unstable();
        for sym in gsyms {
            let out_addr = b.ins().stack_addr(ptr_ty, out_slot, 0);
            let symv = b.ins().iconst(types::I32, sym as i64);
            let c = b.ins().call(glob_ref, &[heap, out_addr, symv]);
            let status = b.inst_results(c)[0];
            let okb = b.create_block();
            b.ins().brif(status, error, &[], okb, &[]);
            b.switch_to_block(okb);
            let w0 = b.ins().stack_load(types::I64, out_slot, 0);
            let w1 = b.ins().stack_load(types::I64, out_slot, PAYLOAD_OFFSET as i32);
            let w2 = b.ins().stack_load(types::I64, out_slot, PAYLOAD_OFFSET as i32 + 8);
            let c = b.ins().call(vbase_ref, &[heap, w0, w1, w2, len_addr]);
            let ptr = b.inst_results(c)[0];
            let cont = b.create_block();
            b.ins().brif(ptr, cont, &[], deopt, &[]);
            b.switch_to_block(cont);
            let vlen = b.ins().load(types::I64, MemFlags::new(), len_addr, 0);
            hoisted_global.insert(sym, (ptr, vlen, w0, w1, w2));
        }
        // Scalar globals (#1): resolve each once at entry into its `Value` words — no vector
        // base, no per-access IC. Unbound ⇒ `error` (matches the VM's late-bound lookup).
        let mut ssyms: Vec<Symbol> = hoist_scalar_globals.iter().copied().collect();
        ssyms.sort_unstable();
        for sym in ssyms {
            let out_addr = b.ins().stack_addr(ptr_ty, out_slot, 0);
            let symv = b.ins().iconst(types::I32, sym as i64);
            let c = b.ins().call(glob_ref, &[heap, out_addr, symv]);
            let status = b.inst_results(c)[0];
            let okb = b.create_block();
            b.ins().brif(status, error, &[], okb, &[]);
            b.switch_to_block(okb);
            let w0 = b.ins().stack_load(types::I64, out_slot, 0);
            let w1 = b.ins().stack_load(types::I64, out_slot, PAYLOAD_OFFSET as i32);
            let w2 = b.ins().stack_load(types::I64, out_slot, PAYLOAD_OFFSET as i32 + 8);
            hoisted_scalar.insert(sym, (w0, w1, w2));
        }
        if !hoisted_global.is_empty() || !hoisted_scalar.is_empty() {
            let ep_ptr = epoch_ptr.expect("epoch_ptr fetched when globals are hoisted");
            entry_epoch = Some(b.ins().load(types::I64, MemFlags::new(), ep_ptr, 0));
        }
    }
    // Initialize register-carry variables from roots (first iteration). Each param slot k is
    // tag-checked (Int or Float, per is_float) once at entry; subsequent iterations read
    // `use_var(carry_vars[k].0)` directly. Float slots are bitcast i64→f64.
    for (k, &(var, is_float)) in carry_vars.iter().enumerate() {
        let rb = b.use_var(rb_var);
        let idx = b.ins().iadd_imm(base, k as i64);
        let off = b.ins().imul_imm(idx, STRIDE);
        let addr = b.ins().iadd(rb, off);
        let tag = b.ins().load(types::I8, MemFlags::new(), addr, 0);
        let expected_tag = if is_float {
            TAG_FLOAT as i64
        } else {
            TAG_INT as i64
        };
        let ok = b.ins().icmp_imm(IntCC::Equal, tag, expected_tag);
        let cont = b.create_block();
        b.ins().brif(ok, cont, &[], deopt, &[]);
        b.switch_to_block(cont);
        let bits = b.ins().load(types::I64, MemFlags::new(), addr, PAYLOAD_OFFSET as i32);
        if is_float {
            let f = b.ins().bitcast(types::F64, MemFlags::new(), bits);
            b.def_var(var, f);
        } else {
            b.def_var(var, bits);
        }
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
    let box_scalar = |b: &mut FunctionBuilder,
                      v: cranelift_codegen::ir::Value|
     -> (u8, cranelift_codegen::ir::Value) {
        if b.func.dfg.value_type(v) == types::I64 {
            (TAG_INT, v)
        } else {
            (TAG_BOOL, b.ins().uextend(types::I64, v))
        }
    };
    // Load frame slot `k` as an unboxed `i64`, tag-checking `Int` first: a non-`Int`
    // operand branches to `deopt` (the VM then runs the arm, where the inline path
    // handles the real shape). Leaves `b` switched to the post-check block. Used by
    // `Local` and the fused `Prim2Slot*` operands alike.
    // Fast path: register-carried param slots (0..carry_argc) skip the tag-check entirely —
    // the entry block already verified Int and `def_var`'d the raw i64; each SelfCall
    // re-`def_var`s on the back-edge. `use_var` gives the current iteration's value without
    // any memory access or branch.
    let load_slot_int = |b: &mut FunctionBuilder, k: i64| -> cranelift_codegen::ir::Value {
        if let Some(&(var, false)) = carry_vars.get(k as usize) {
            return b.use_var(var);
        }
        let roots_base = b.use_var(rb_var);
        let idx = b.ins().iadd_imm(base, k);
        let off = b.ins().imul_imm(idx, STRIDE);
        let addr = b.ins().iadd(roots_base, off);
        let tag = b.ins().load(types::I8, MemFlags::new(), addr, 0);
        let is_int = b.ins().icmp_imm(IntCC::Equal, tag, TAG_INT as i64);
        let cont = b.create_block();
        b.ins().brif(is_int, cont, &[], deopt, &[]);
        b.switch_to_block(cont);
        b.ins()
            .load(types::I64, MemFlags::new(), addr, PAYLOAD_OFFSET as i32)
    };
    // `map` reorders the two operands into the primitive's `(x, y)` argument order —
    // e.g. `>` is `%lt` with `map = [1, 0]` (operands swapped), so the JIT must apply
    // it or `(> a b)` would compute `a < b`. `m == 0` picks the first source, else the
    // second. (`emit_node` only ever produces `[0,1]` or `[1,0]` for these prims.)
    let pick = |s0, s1, m: u8| if m == 0 { s0 } else { s1 };
    // Emit `op` on two unboxed `i64` operands already in `(x, y)` order. Add/Sub/Mul use
    // the overflow-checked Cranelift ops and branch to `deopt` on signed overflow — the
    // VM's inline path defers an overflowing `i64` op to the native, which promotes to a
    // BigInt (ADR bignums), so deopting here keeps the JIT bit-identical to the VM
    // instead of silently wrapping. Comparisons yield an `I8` 0/1. Leaves `b` switched
    // to the post-check block for the arithmetic ops.
    let emit_arith = |b: &mut FunctionBuilder,
                      op: PrimOp,
                      x: cranelift_codegen::ir::Value,
                      y: cranelift_codegen::ir::Value|
     -> Option<cranelift_codegen::ir::Value> {
        let checked = |b: &mut FunctionBuilder, r: cranelift_codegen::ir::Value, ov| {
            let cont = b.create_block();
            b.ins().brif(ov, deopt, &[], cont, &[]);
            b.switch_to_block(cont);
            r
        };
        Some(match op {
            PrimOp::Add => {
                let (r, ov) = b.ins().sadd_overflow(x, y);
                checked(b, r, ov)
            }
            PrimOp::Sub => {
                let (r, ov) = b.ins().ssub_overflow(x, y);
                checked(b, r, ov)
            }
            PrimOp::Mul => {
                let (r, ov) = b.ins().smul_overflow(x, y);
                checked(b, r, ov)
            }
            PrimOp::Lt => b.ins().icmp(IntCC::SignedLessThan, x, y),
            PrimOp::Le => b.ins().icmp(IntCC::SignedLessThanOrEqual, x, y),
            PrimOp::Eq => b.ins().icmp(IntCC::Equal, x, y),
            // Integer division family (`rem`/`quot`/`%div`). Cranelift's `sdiv`/`srem`
            // *trap* on a zero divisor and on the `i64::MIN / -1` overflow, so both must
            // be guarded → deopt before the op (the VM's inline path defers exactly these
            // edges to the native, matching). `%div` additionally yields an `Int` only on
            // an exact quotient — a nonzero remainder means a `Float` the native builds,
            // so deopt then too. Operand order is already `(x, y)` (map applied).
            PrimOp::Rem | PrimOp::Quot | PrimOp::Div => {
                let zero = b.ins().iconst(types::I64, 0);
                let div0 = b.ins().icmp(IntCC::Equal, y, zero);
                let c0 = b.create_block();
                b.ins().brif(div0, deopt, &[], c0, &[]);
                b.switch_to_block(c0);
                // (x == i64::MIN) && (y == -1) — the one signed-division overflow.
                let min = b.ins().iconst(types::I64, i64::MIN);
                let neg1 = b.ins().iconst(types::I64, -1);
                let x_min = b.ins().icmp(IntCC::Equal, x, min);
                let y_m1 = b.ins().icmp(IntCC::Equal, y, neg1);
                let ov = b.ins().band(x_min, y_m1);
                let c1 = b.create_block();
                b.ins().brif(ov, deopt, &[], c1, &[]);
                b.switch_to_block(c1);
                match op {
                    PrimOp::Rem => b.ins().srem(x, y),
                    PrimOp::Quot => b.ins().sdiv(x, y),
                    PrimOp::Div => {
                        // Exact only: a nonzero remainder → Float → deopt to the native.
                        let r = b.ins().srem(x, y);
                        let inexact = b.ins().icmp(IntCC::NotEqual, r, zero);
                        let c2 = b.create_block();
                        b.ins().brif(inexact, deopt, &[], c2, &[]);
                        b.switch_to_block(c2);
                        b.ins().sdiv(x, y)
                    }
                    _ => unreachable!(),
                }
            }
            PrimOp::Max => {
                let cond = b.ins().icmp(IntCC::SignedGreaterThanOrEqual, x, y);
                b.ins().select(cond, x, y)
            }
            PrimOp::Min => {
                let cond = b.ins().icmp(IntCC::SignedLessThanOrEqual, x, y);
                b.ins().select(cond, x, y)
            }
            PrimOp::BitAnd => b.ins().band(x, y),
            PrimOp::BitOr => b.ins().bor(x, y),
            PrimOp::BitXor => b.ins().bxor(x, y),
            PrimOp::Cons => return None, // allocates — never in the JIT subset
            PrimOp::VectorRef => return None, // heap slab read — not lowered; out of subset
        })
    };

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
    #[derive(Clone, Copy)]
    enum Op {
        Int(cranelift_codegen::ir::Value),
        // An unboxed `f64` SSA value (a `Const(Float)`, a float-slot read, or a float
        // arith result). Boxed back to a `Value::Float` (TAG_FLOAT + the bits) when stored
        // to a slot / self-call arg / returned. Float *comparisons* (`<`/`<=`/`=`) yield an
        // `Op::Int` i8 (a Bool), like integer compares, so branch handling is shared.
        Float(cranelift_codegen::ir::Value),
        // A boolean SSA value (`i64` 0/1) that has crossed a block boundary. A comparison
        // result is normally an `Op::Int` with `i8` type (and `box_scalar` boxes it as a
        // `Value::Bool`); but when it flows through a block param (e.g. an `(and …)`
        // short-circuit carrying its result to the merge) it is zero-extended to the `i64`
        // block-param width, which erases the `i8`-means-bool signal. The lowering tags such
        // params as `Op::Bool` (via `bool_param` recorded at the jump) so they still box as
        // `Bool`, not `Int`, and branch correctly in `JumpIfFalse`.
        Bool(cranelift_codegen::ir::Value),
        Slot(usize),
        Handle(
            cranelift_codegen::ir::Value,
            cranelift_codegen::ir::Value,
            cranelift_codegen::ir::Value,
        ),
        // A hoisted invariant **global vector** (matmul LICM, the global lever): its
        // resolved `Value` words (`w0..w2`, like a `Handle` — used for any non-`VectorRef`
        // consumer) PLUS its element storage base (`ptr`, `len`), resolved **once** at the
        // arm entry. A `(nth thisglobal idx)` reads `ptr + idx*STRIDE` inline instead of
        // calling `brood_rt_vector_ref`; the back-edge epoch guard deopts if the global was
        // rebound, keeping it bit-identical to the VM's per-iteration late binding.
        HoistedVec {
            ptr: cranelift_codegen::ir::Value,
            len: cranelift_codegen::ir::Value,
            w0: cranelift_codegen::ir::Value,
            w1: cranelift_codegen::ir::Value,
            w2: cranelift_codegen::ir::Value,
        },
    }
    let done_block = leader_block[len]?;
    // Store an unboxed scalar `Op::Int` value into frame slot `k`, boxing it as `Int` or
    // (for a comparison `i8`) `Bool` via `box_scalar`.
    let store_int = |b: &mut FunctionBuilder, k: i64, v: cranelift_codegen::ir::Value| {
        debug_assert!((k as usize) < nslots, "[jit-slot] store_int slot {k} >= nslots {nslots}");
        let (tag_byte, payload) = box_scalar(b, v);
        let roots_base = b.use_var(rb_var);
        let idx = b.ins().iadd_imm(base, k);
        let off = b.ins().imul_imm(idx, STRIDE);
        let addr = b.ins().iadd(roots_base, off);
        let tag = b.ins().iconst(types::I8, tag_byte as i64);
        b.ins().store(MemFlags::new(), tag, addr, 0);
        b.ins()
            .store(MemFlags::new(), payload, addr, PAYLOAD_OFFSET as i32);
    };
    // Copy the whole `Value` from frame slot `src` to slot `dst` (handle-safe — moves the
    // bytes verbatim, no interpretation). A `Value` is `STRIDE` bytes (`#[repr(C, u8)]`):
    // it must copy **every** i64 word, not just tag+payload — `Value::Pid { node, id }`
    // (and any future 2-word-payload variant) carries `id` in the third word at offset 16,
    // which a tag+payload-only copy would drop and corrupt.
    let copy_value = |b: &mut FunctionBuilder, src: i64, dst: i64| {
        debug_assert!((src as usize) < nslots && (dst as usize) < nslots, "[jit-slot] copy_value src {src} dst {dst} vs nslots {nslots}");
        let roots_base = b.use_var(rb_var);
        let saddr = {
            let i = b.ins().iadd_imm(base, src);
            let o = b.ins().imul_imm(i, STRIDE);
            b.ins().iadd(roots_base, o)
        };
        let daddr = {
            let i = b.ins().iadd_imm(base, dst);
            let o = b.ins().imul_imm(i, STRIDE);
            b.ins().iadd(roots_base, o)
        };
        let mut off = 0i32;
        while (off as i64) < STRIDE {
            let w = b.ins().load(types::I64, MemFlags::new(), saddr, off);
            b.ins().store(MemFlags::new(), w, daddr, off);
            off += 8;
        }
    };
    // Read an operand as its three `Value` words `[w0, w1, w2]` — for a self-call arg, a
    // binder, a return, or a `cons`/`car`/`cdr` operand. An `Int` boxes to `[Int-tag, v, 0]`
    // (the third word is irrelevant to an Int); a `Slot` loads all three; a `Handle` is
    // already those registers. No tag-check — this moves a whole `Value` verbatim.
    let read_words = |b: &mut FunctionBuilder, op: Op| -> [cranelift_codegen::ir::Value; 3] {
        match op {
            Op::Int(v) => {
                // Box as `Int` or (a comparison `i8`) `Bool`; both payloads are `i64`, so
                // the triple is a valid `[i64; 3]` whole `Value`.
                let (tag_byte, payload) = box_scalar(b, v);
                let tag = b.ins().iconst(types::I64, tag_byte as i64);
                let zero = b.ins().iconst(types::I64, 0);
                [tag, payload, zero]
            }
            Op::Slot(k) => {
                // DEBUG: a real/spill slot must be inside the frame [0, nslots). A k >= nslots
                // reads past the frame into staging/stale memory — the bug #2 slot-count gap.
                debug_assert!(
                    k < nslots,
                    "[jit-slot] read_words Op::Slot({k}) >= nslots {nslots} (spill_base {spill_base}, reserve {reserve}) — slot count undercounted",
                );
                let roots_base = b.use_var(rb_var);
                let i = b.ins().iadd_imm(base, k as i64);
                let o = b.ins().imul_imm(i, STRIDE);
                let addr = b.ins().iadd(roots_base, o);
                let w0 = b.ins().load(types::I64, MemFlags::new(), addr, 0);
                let w1 = b
                    .ins()
                    .load(types::I64, MemFlags::new(), addr, PAYLOAD_OFFSET as i32);
                let w2 = b
                    .ins()
                    .load(types::I64, MemFlags::new(), addr, PAYLOAD_OFFSET as i32 + 8);
                // NOTE: an in-IR validation call here (dbg_check_slot_ref) PERTURBS codegen —
                // it forces register spills around the call that mask the very register-liveness
                // bug we're hunting (#2). Validation lives in the Rust-side `brood_rt_push`.
                [w0, w1, w2]
            }
            Op::Float(v) => {
                // Box an unboxed `f64` as a whole `Value::Float`: [TAG_FLOAT, bits, 0].
                let bits = b.ins().bitcast(types::I64, MemFlags::new(), v);
                let tag = b.ins().iconst(types::I64, TAG_FLOAT as i64);
                let zero = b.ins().iconst(types::I64, 0);
                [tag, bits, zero]
            }
            Op::Bool(v) => {
                // A crossed-boundary boolean (already `i64` 0/1) → `Value::Bool`.
                let tag = b.ins().iconst(types::I64, TAG_BOOL as i64);
                let zero = b.ins().iconst(types::I64, 0);
                [tag, v, zero]
            }
            Op::Handle(w0, w1, w2) => {
                // NOTE: no in-IR validation call here — it would perturb codegen and mask the
                // bug (see Op::Slot above). Register handles flow to brood_rt_push for checking.
                [w0, w1, w2]
            }
            // A hoisted global vector used as a whole `Value` (any non-`VectorRef`
            // consumer): its entry-resolved words move verbatim, exactly like a `Handle`.
            Op::HoistedVec { w0, w1, w2, .. } => [w0, w1, w2],
        }
    };
    // Store the three words of a `Value` into frame slot `dst`.
    let store_words = |b: &mut FunctionBuilder, dst: i64, w: [cranelift_codegen::ir::Value; 3]| {
        debug_assert!((dst as usize) < nslots, "[jit-slot] store_words slot {dst} >= nslots {nslots}");
        let roots_base = b.use_var(rb_var);
        let i = b.ins().iadd_imm(base, dst);
        let o = b.ins().imul_imm(i, STRIDE);
        let addr = b.ins().iadd(roots_base, o);
        b.ins().store(MemFlags::new(), w[0], addr, 0);
        b.ins()
            .store(MemFlags::new(), w[1], addr, PAYLOAD_OFFSET as i32);
        b.ins()
            .store(MemFlags::new(), w[2], addr, PAYLOAD_OFFSET as i32 + 8);
    };
    // Materialise an operand to an unboxed `i64`: a register value as-is, a tag-checked
    // load of a frame slot, or a tag-checked extract of a `Handle`'s payload (a `Handle`
    // used as a number — e.g. `(+ (first xs) 1)` — must be an `Int` at runtime or deopt).
    let as_int = |b: &mut FunctionBuilder, op: Op| -> cranelift_codegen::ir::Value {
        match op {
            Op::Int(v) => v,
            Op::Bool(v) => v,
            Op::Slot(k) => load_slot_int(b, k as i64),
            Op::Handle(w0, w1, _) => {
                let tagb = b.ins().band_imm(w0, 0xff);
                let is_int = b.ins().icmp_imm(IntCC::Equal, tagb, TAG_INT as i64);
                let cont = b.create_block();
                b.ins().brif(is_int, cont, &[], deopt, &[]);
                b.switch_to_block(cont);
                w1
            }
            // A hoisted global vector used as an int (a vector value isn't one) — tag-check
            // its word like a `Handle` and deopt; sound, never expected to fire.
            Op::HoistedVec { w0, w1, .. } => {
                let tagb = b.ins().band_imm(w0, 0xff);
                let is_int = b.ins().icmp_imm(IntCC::Equal, tagb, TAG_INT as i64);
                let cont = b.create_block();
                b.ins().brif(is_int, cont, &[], deopt, &[]);
                b.switch_to_block(cont);
                w1
            }
            // A float where an int is required (a mixed-type op the lowering didn't
            // specialize) — deopt to the VM. Shouldn't arise once arith dispatches by
            // operand type, but kept sound. (Dead block after the unconditional jump.)
            Op::Float(_) => {
                b.ins().jump(deopt, &[]);
                let dead = b.create_block();
                b.switch_to_block(dead);
                b.ins().iconst(types::I64, 0)
            }
        }
    };
    // Materialise an operand as a block argument. Block params are declared `I64`
    // (see `leader_block`), but a comparison result is an `i8`; passing it raw would
    // be an `I8`-into-`I64`-param type mismatch the Cranelift verifier rejects, which
    // bailed *every* arm that carried a comparison across a block boundary — i.e. every
    // `(and …)`/`(or …)` (they short-circuit a bool through a merge). Zero-extend the
    // `i8` (0/1 → bool); the target reconstructs it as `Op::Bool` via the `bool_param`
    // flag recorded at this jump, so it branches with correct Brood truthiness. Every
    // other `as_int` result is already `i64`.
    let as_block_arg = |b: &mut FunctionBuilder, op: Op| -> cranelift_codegen::ir::Value {
        // A slot proven to hold a `Value::Bool` (`slot_bool`): load its payload byte (0/1)
        // as the i64 arg — the target reconstructs `Op::Bool` via the `bool_param` flag
        // (`is_bool_op` is true for it too, so every predecessor agrees). `as_int` would
        // instead tag-check `Int` and deopt on the `Bool`.
        if let Op::Slot(k) = op {
            if slot_bool.borrow().get(k).copied().unwrap_or(false) {
                let roots_base = b.use_var(rb_var);
                let i = b.ins().iadd_imm(base, k as i64);
                let o = b.ins().imul_imm(i, STRIDE);
                let addr = b.ins().iadd(roots_base, o);
                let pl = b
                    .ins()
                    .load(types::I64, MemFlags::new(), addr, PAYLOAD_OFFSET as i32);
                return b.ins().band_imm(pl, 0xff);
            }
        }
        let v = as_int(b, op);
        if b.func.dfg.value_type(v) == types::I8 {
            b.ins().uextend(types::I64, v)
        } else {
            v
        }
    };
    // Materialise an operand to an unboxed `f64`. A `Slot` is normally tag-checked `==
    // Float` and its payload bit-cast to `f64`. Two fast paths, applied in order:
    //
    // 1. Float-carry slots (0..carry_argc, profiled Int/Float): `use_var` — no tag-check,
    //    no memory access, just the phi-propagated SSA value.
    // 2. F64 SSA cache: `store_op(Float(v))` stashes `v` in `slot_f64_cache`; subsequent
    //    reads in the same block return it directly. Eliminates the store→load→bitcast
    //    round-trip for let-bound floats (e.g. `nx`/`ny` in mandelbrot's `esc` inner loop,
    //    where `(* nx nx)` would otherwise reload and tag-check the just-written slot).
    //    The cache is valid only for slots written via `store_op` (never via SelfCall/entry),
    //    and parameter slots are always None — safe against cross-branch pollution.
    // 3. Unknown: full tag-check + brif to deopt + load + bitcast. NOTE: we do NOT skip the
    //    tag-check based on `slot_float[k]` alone: that flag is a single-pass approximation
    //    that can be contaminated by stores in other branches (e.g. a then-branch `store_op`
    //    setting slot_float[k]=true before an else-branch `as_f64` read — the slot is really
    //    Int at that point). Skipping the brif deopt there produces wrong results.
    let as_f64 = |b: &mut FunctionBuilder, op: Op| -> cranelift_codegen::ir::Value {
        match op {
            Op::Float(v) => v,
            Op::Slot(k) => {
                if let Some(&(var, true)) = carry_vars.get(k as usize) {
                    return b.use_var(var);
                }
                if let Some(v) =
                    slot_f64_cache.borrow().get(k as usize).copied().flatten()
                {
                    return v;
                }
                let roots_base = b.use_var(rb_var);
                let i = b.ins().iadd_imm(base, k as i64);
                let o = b.ins().imul_imm(i, STRIDE);
                let addr = b.ins().iadd(roots_base, o);
                let tag = b.ins().load(types::I8, MemFlags::new(), addr, 0);
                let is_f = b.ins().icmp_imm(IntCC::Equal, tag, TAG_FLOAT as i64);
                let cont = b.create_block();
                b.ins().brif(is_f, cont, &[], deopt, &[]);
                b.switch_to_block(cont);
                let bits = b
                    .ins()
                    .load(types::I64, MemFlags::new(), addr, PAYLOAD_OFFSET as i32);
                b.ins().bitcast(types::F64, MemFlags::new(), bits)
            }
            Op::Int(_) | Op::Bool(_) | Op::Handle(..) | Op::HoistedVec { .. } => {
                b.ins().jump(deopt, &[]);
                let dead = b.create_block();
                b.switch_to_block(dead);
                b.ins().f64const(0.0)
            }
        }
    };
    // Integer-vs-float dispatch for a binary op: an operand is float if it's an
    // `Op::Float`, or a `Slot` the profile/tracking marks float. (`Op::Int`/`Handle` are
    // integer/non-number.)
    let op_is_float = |op: Op| -> bool {
        match op {
            Op::Float(_) => true,
            Op::Slot(k) => slot_float.borrow().get(k).copied().unwrap_or(false),
            _ => false,
        }
    };
    // Float arith / comparison. Arith → `Op::Float`; a comparison → an `i8` boxed as a
    // Bool (`Op::Int`, exactly like the integer compares). `/` and the integer-only ops
    // aren't lowered for floats → `None` bails the arm to the VM.
    let emit_float_arith = |b: &mut FunctionBuilder, op: PrimOp, x, y| -> Option<Op> {
        use cranelift_codegen::ir::condcodes::FloatCC;
        Some(match op {
            PrimOp::Add => Op::Float(b.ins().fadd(x, y)),
            PrimOp::Sub => Op::Float(b.ins().fsub(x, y)),
            PrimOp::Mul => Op::Float(b.ins().fmul(x, y)),
            PrimOp::Lt => Op::Int(b.ins().fcmp(FloatCC::LessThan, x, y)),
            PrimOp::Le => Op::Int(b.ins().fcmp(FloatCC::LessThanOrEqual, x, y)),
            PrimOp::Eq => Op::Int(b.ins().fcmp(FloatCC::Equal, x, y)),
            _ => return None,
        })
    };
    // Store an operand into frame slot `dst`: an `Int` is boxed; a `Slot` is copied
    // verbatim; a `Handle` stores its three words (so a handle binder / self-call arg /
    // return keeps its type).
    // Also tracks `slot_float[dst]` so a later read of `dst` picks the right arith: a
    // float store marks it float, an int/handle store clears it, a slot-copy inherits the
    // source's flag. (Lets let-binders — nil at the entry snapshot — get their type from
    // the body's writes, which precede their reads in the single lowering pass.)
    let set_slot_float = |dst: i64, v: bool| {
        if let Some(s) = slot_float.borrow_mut().get_mut(dst as usize) {
            *s = v;
        }
    };
    // Mirror of `set_slot_float` for the bool flag. A store of any kind updates *both*
    // (a slot holds one type), so a later read picks the right block-arg representation.
    let set_slot_bool = |dst: i64, v: bool| {
        if let Some(s) = slot_bool.borrow_mut().get_mut(dst as usize) {
            *s = v;
        }
    };
    let store_op = |b: &mut FunctionBuilder, dst: i64, op: Op| match op {
        Op::Int(v) => {
            // A comparison `i8` (`store_int`/`box_scalar` boxes it as `Value::Bool`) marks
            // the slot bool; a real `i64` int does not.
            let is_b = b.func.dfg.value_type(v) == types::I8;
            store_int(b, dst, v);
            set_slot_float(dst, false);
            set_slot_bool(dst, is_b);
            if let Some(s) = slot_f64_cache.borrow_mut().get_mut(dst as usize) {
                *s = None;
            }
        }
        Op::Float(v) => {
            let bits = b.ins().bitcast(types::I64, MemFlags::new(), v);
            let tag = b.ins().iconst(types::I64, TAG_FLOAT as i64);
            let zero = b.ins().iconst(types::I64, 0);
            store_words(b, dst, [tag, bits, zero]);
            set_slot_float(dst, true);
            set_slot_bool(dst, false);
            if let Some(s) = slot_f64_cache.borrow_mut().get_mut(dst as usize) {
                *s = Some(v);
            }
        }
        Op::Bool(v) => {
            let tag = b.ins().iconst(types::I64, TAG_BOOL as i64);
            let zero = b.ins().iconst(types::I64, 0);
            store_words(b, dst, [tag, v, zero]);
            set_slot_float(dst, false);
            set_slot_bool(dst, true);
            if let Some(s) = slot_f64_cache.borrow_mut().get_mut(dst as usize) {
                *s = None;
            }
        }
        Op::Slot(k) => {
            copy_value(b, k as i64, dst);
            // Read both source flags and f64 cache into locals *before* mutating (a held
            // `borrow()` would double-borrow with `set_slot_*`'s `borrow_mut()`).
            let f = slot_float.borrow().get(k).copied().unwrap_or(false);
            let bl = slot_bool.borrow().get(k).copied().unwrap_or(false);
            let fv = slot_f64_cache.borrow().get(k).copied().flatten();
            set_slot_float(dst, f);
            set_slot_bool(dst, bl);
            if let Some(s) = slot_f64_cache.borrow_mut().get_mut(dst as usize) {
                *s = fv;
            }
        }
        Op::Handle(w0, w1, w2) => {
            store_words(b, dst, [w0, w1, w2]);
            set_slot_float(dst, false);
            set_slot_bool(dst, false);
            if let Some(s) = slot_f64_cache.borrow_mut().get_mut(dst as usize) {
                *s = None;
            }
        }
        Op::HoistedVec { w0, w1, w2, .. } => {
            // Stored as a whole `Value` (its entry-resolved words), like a `Handle`.
            store_words(b, dst, [w0, w1, w2]);
            set_slot_float(dst, false);
            set_slot_bool(dst, false);
            if let Some(s) = slot_f64_cache.borrow_mut().get_mut(dst as usize) {
                *s = None;
            }
        }
    };
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
    let call_handle = |b: &mut FunctionBuilder,
                       fref: cranelift_codegen::ir::FuncRef,
                       operands: &[cranelift_codegen::ir::Value]|
     -> Op {
        let out_addr = b.ins().stack_addr(ptr_ty, out_slot, 0);
        let mut args = Vec::with_capacity(operands.len() + 2);
        args.push(heap);
        args.push(out_addr);
        args.extend_from_slice(operands);
        b.ins().call(fref, &args);
        let w0 = b.ins().stack_load(types::I64, out_slot, 0);
        let w1 = b
            .ins()
            .stack_load(types::I64, out_slot, PAYLOAD_OFFSET as i32);
        let w2 = b
            .ins()
            .stack_load(types::I64, out_slot, PAYLOAD_OFFSET as i32 + 8);
        Op::Handle(w0, w1, w2)
    };
    // `vector-ref` / inlined `nth`: a bounds-checked slab read via the runtime helper.
    // On status≠0 (non-vector / non-int / out-of-range) it branches to `deopt`, so the
    // VM owns the exact result (`vector-ref`'s error, `nth`'s default); otherwise the
    // element rides back as a `Handle`. The helper never allocates, so the handle is
    // safe to hold until its immediate consumer.
    let vector_ref = |b: &mut FunctionBuilder,
                      vec: [cranelift_codegen::ir::Value; 3],
                      idx: [cranelift_codegen::ir::Value; 3]|
     -> Op {
        let out_addr = b.ins().stack_addr(ptr_ty, out_slot, 0);
        let c = b.ins().call(
            vref_ref,
            &[
                heap, out_addr, vec[0], vec[1], vec[2], idx[0], idx[1], idx[2],
            ],
        );
        let status = b.inst_results(c)[0];
        let cont = b.create_block();
        b.ins().brif(status, deopt, &[], cont, &[]);
        b.switch_to_block(cont);
        let w0 = b.ins().stack_load(types::I64, out_slot, 0);
        let w1 = b
            .ins()
            .stack_load(types::I64, out_slot, PAYLOAD_OFFSET as i32);
        let w2 = b
            .ins()
            .stack_load(types::I64, out_slot, PAYLOAD_OFFSET as i32 + 8);
        Op::Handle(w0, w1, w2)
    };

    // For each leader, which of its operand-stack block params carry a boolean (so the
    // entry reconstruction tags them `Op::Bool`, not `Op::Int`). Populated by the jump
    // sites (`Jump`/`JumpIfFalse`/leader fall-through), which run before the target block is
    // translated (forward edges, in ip order) — so the flags are set by the time the target
    // is reached. A back-edge target with params would see no flags and default to `Int`;
    // self-tail back-edges target the param-less leader 0, so this doesn't arise in practice.
    let mut bool_param: Vec<Option<Vec<bool>>> = vec![None; len + 1];
    // True if `op` is a boolean value: a comparison result (`Op::Int` with `i8` type) or a
    // boolean that already crossed a block boundary (`Op::Bool`).
    let is_bool_op = |b: &FunctionBuilder, op: Op| {
        matches!(op, Op::Bool(_))
            || matches!(op, Op::Int(v) if b.func.dfg.value_type(v) == types::I8)
            || matches!(op, Op::Slot(k) if slot_bool.borrow().get(k).copied().unwrap_or(false))
    };

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
                let is_bool = bool_param[ip]
                    .as_ref()
                    .and_then(|f| f.get(i).copied())
                    .unwrap_or(false);
                if is_bool {
                    Op::Bool(v)
                } else {
                    Op::Int(v)
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
                        let w2 = b
                            .ins()
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
                    if let Some(&(w0, w1, w2)) = hoisted_scalar.get(s) {
                        // Hoisted scalar global (#1): the value was resolved once at entry;
                        // reuse its words as a `Handle` (no per-access `brood_rt_global_ic`).
                        // The back-edge epoch guard deopts on a rebind (late-binding-exact).
                        stack.push(Op::Handle(w0, w1, w2));
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
                        let w2 = b
                            .ins()
                            .stack_load(types::I64, out_slot, PAYLOAD_OFFSET as i32 + 8);
                        stack.push(Op::Handle(w0, w1, w2));
                    }
                }
                Inst::Call {
                    argc,
                    tail,
                    site,
                    head,
                    pos: _,
                } => {
                    let argc = *argc;
                    let call_site = *site;
                    // The call-head symbol, for the call-site inline cache in
                    // `jit_dispatch_call` (only meaningful when `site != NO_SITE`, i.e. a
                    // free-global head). `u32::MAX` stands in for a computed/local head.
                    let call_head = head.unwrap_or(u32::MAX);
                    // Operands consumed by the call. A **free-global** head (`head = Some`)
                    // isn't staged — the compiler emits no head `Global`, so the operand
                    // stack holds only the `argc` args; `jit_dispatch_call` resolves the
                    // callee via the call IC. A **computed** head leaves the callee staged
                    // below the args (`argc + 1` operands).
                    let n_ops = if head.is_some() { argc } else { argc + 1 };
                    #[cfg(debug_assertions)]
                    {
                        let sv = b.ins().iconst(types::I32, call_site as i64);
                        b.ins().call(dbg_staging_ref, &[heap, sv]);
                    }
                    // The call is a safepoint (the callee runs arbitrary Brood and may GC).
                    // A live `Handle` left on the operand stack BELOW the call's own operands
                    // would be a heap pointer in a register across the collection → stale.
                    // `Slot`/`Int` are safe (a slot lives in `roots`, GC-visible; an int is
                    // not a handle). So **spill** each deeper `Handle` into a reserved frame
                    // slot (GC-visible, relocated correctly by the callee's safepoint) and
                    // replace it with that `Slot` — this is what lets two-call recursion
                    // (`(+ (fib …) (fib …))`, bintree `check`) lower instead of bailing. The
                    // store writes the handle's three words into the frame *before* any
                    // `brood_rt_push` (which may realloc `roots`), so the read-all-then-stage
                    // discipline below is preserved. Out of reserved slots → bail to the VM.
                    let below = stack.len().checked_sub(n_ops)?;
                    for d in 0..below {
                        if matches!(stack[d], Op::Handle(..)) {
                            if spill_next >= reserve {
                                return None;
                            }
                            let slot = spill_base + spill_next;
                            spill_next += 1;
                            store_op(&mut b, slot as i64, stack[d]);
                            stack[d] = Op::Slot(slot);
                        }
                    }
                    // Pop the operands (computed callee deepest, then args), then read each
                    // into registers BEFORE staging — a `brood_rt_push` may reallocate
                    // `roots`, so no slot read may run after a push (the read-all-then-store
                    // discipline, same as `SelfCall`).
                    let mut ops: Vec<Op> = Vec::with_capacity(n_ops);
                    for _ in 0..n_ops {
                        ops.push(stack.pop()?);
                    }
                    ops.reverse(); // computed callee (if any) first, then args in source order
                    let mut worded: Vec<[cranelift_codegen::ir::Value; 3]> =
                        Vec::with_capacity(ops.len());
                    for &op in &ops {
                        worded.push(read_words(&mut b, op));
                    }
                    // For a free-global tail call, jit_dispatch_tail reads [callee, args…]
                    // from roots — but the elided head is never staged. Resolve it via the
                    // global IC and stage it now, before the args. Arg words are already in
                    // `worded` (read above, before any push) so no slot reads follow.
                    if *tail && head.is_some() {
                        let sym_v2 = b.ins().iconst(types::I32, call_head as i64);
                        let site_v2 = b.ins().iconst(types::I32, call_site as i64);
                        let out_a = b.ins().stack_addr(ptr_ty, out_slot, 0);
                        let cv = b.ins().call(globic_ref, &[heap, out_a, sym_v2, site_v2]);
                        let cstatus = b.inst_results(cv)[0];
                        let callee_ok = b.create_block();
                        b.ins().brif(cstatus, error, &[], callee_ok, &[]);
                        b.switch_to_block(callee_ok);
                        let cw0 = b.ins().stack_load(types::I64, out_slot, 0);
                        let cw1 = b
                            .ins()
                            .stack_load(types::I64, out_slot, PAYLOAD_OFFSET as i32);
                        let cw2 = b
                            .ins()
                            .stack_load(types::I64, out_slot, PAYLOAD_OFFSET as i32 + 8);
                        b.ins().call(push_ref, &[heap, cw0, cw1, cw2]);
                    }
                    // Stage `[callee, arg0 .. arg_{argc-1}]` (the VM's `Inst::Call` layout
                    // that `brood_rt_call_slow` / `jit_dispatch_tail` read back).
                    for w in &worded {
                        b.ins().call(push_ref, &[heap, w[0], w[1], w[2]]);
                    }
                    if *tail {
                        // Tail position: the staged call *is* this arm's result (TCO). It
                        // ends the block — nothing may remain on the operand stack below it
                        // (a real tail call's stack is exactly `[callee, args]`). Return
                        // outcome 4; `vm_run_bc` dispatches the staged call with `tail =
                        // true` and reuses this frame, so the native stack never grows.
                        if !stack.is_empty() {
                            return None;
                        }
                        b.ins().jump(tailcall, &[]);
                        break;
                    }
                    // Non-tail: dispatch through the interpreter inline (a safepoint):
                    // result → `out_slot`, status in a register.
                    let out_addr = b.ins().stack_addr(ptr_ty, out_slot, 0);
                    let argc_v = b.ins().iconst(types::I32, argc as i64);
                    let site_v = b.ins().iconst(types::I32, call_site as i64);
                    let head_v = b.ins().iconst(types::I32, call_head as i64);
                    // Read the result `Value` (3 words) back out of `out_slot` and push it.
                    let read_out = |b: &mut FunctionBuilder| {
                        let w0 = b.ins().stack_load(types::I64, out_slot, 0);
                        let w1 = b.ins().stack_load(types::I64, out_slot, PAYLOAD_OFFSET as i32);
                        let w2 = b.ins().stack_load(types::I64, out_slot, PAYLOAD_OFFSET as i32 + 8);
                        (w0, w1, w2)
                    };
                    // The shared slow-dispatch tail: call `brood_rt_call_slow`, re-fetch the
                    // roots base (the callee may have relocated `roots`), and branch to `error`
                    // on a nonzero status or `cont` on success. Used as the only path (icall
                    // off / computed head) and as the miss path of the fast-link.
                    let emit_call_slow = |b: &mut FunctionBuilder, cont: cranelift_codegen::ir::Block| {
                        let c = b.ins().call(callslow_ref, &[heap, out_addr, argc_v, site_v, head_v]);
                        let status = b.inst_results(c)[0];
                        let rbc = b.ins().call(rb_ref, &[heap]);
                        b.def_var(rb_var, b.inst_results(rbc)[0]);
                        b.ins().brif(status, error, &[], cont, &[]);
                    };

                    if icall_enabled() && head.is_some() {
                        // ---- Track B / Technique A: in-IR epoch-guarded fast link ----
                        // Read the flat-table base + length (re-fetched here, like the roots
                        // base, since a cold nested call may have grown + reallocated it).
                        use crate::core::heap::FastLink;
                        const FL_SIZE: i64 = std::mem::size_of::<FastLink>() as i64;
                        let fl_epoch_off = std::mem::offset_of!(FastLink, epoch) as i32;
                        let fl_code_off = std::mem::offset_of!(FastLink, code) as i32;
                        let fl_nslots_off = std::mem::offset_of!(FastLink, nslots) as i32;
                        let fl_env_off = std::mem::offset_of!(FastLink, env) as i32;
                        let fl_sym_off = std::mem::offset_of!(FastLink, sym) as i32;
                        let fl_argc_off = std::mem::offset_of!(FastLink, argc) as i32;
                        let len_slot = b.create_sized_stack_slot(StackSlotData::new(
                            StackSlotKind::ExplicitSlot,
                            8,
                            3,
                        ));
                        let len_addr = b.ins().stack_addr(ptr_ty, len_slot, 0);
                        let fbc = b.ins().call(flbase_ref, &[heap, len_addr]);
                        let fl_base = b.inst_results(fbc)[0];
                        let fl_len = b.ins().stack_load(types::I64, len_slot, 0);
                        let site_idx = b.ins().iconst(types::I64, call_site as i64);
                        // Bounds: `site < len` (a live arm whose site ids outran a post-collect
                        // re-grow misses here and goes slow — the table read would be OOB).
                        let in_bounds = b.ins().icmp(IntCC::UnsignedLessThan, site_idx, fl_len);
                        let chk_epoch = b.create_block();
                        let chk_ident = b.create_block();
                        let hit = b.create_block();
                        let miss = b.create_block();
                        let cont = b.create_block();
                        b.ins().brif(in_bounds, chk_epoch, &[], miss, &[]);

                        // chk_epoch: this slot's epoch must equal the current global epoch.
                        b.switch_to_block(chk_epoch);
                        let stride = b.ins().iconst(types::I64, FL_SIZE);
                        let off = b.ins().imul(site_idx, stride);
                        let slot_ptr = b.ins().iadd(fl_base, off);
                        let ep = b.ins().load(types::I64, MemFlags::new(), slot_ptr, fl_epoch_off);
                        let ep_ptr = epoch_ptr.expect("epoch_ptr fetched when icall is on");
                        let gep = b.ins().load(types::I64, MemFlags::new(), ep_ptr, 0);
                        let ep_ok = b.ins().icmp(IntCC::Equal, ep, gep);
                        b.ins().brif(ep_ok, chk_ident, &[], miss, &[]);

                        // chk_ident: the slot must link the *same* callee this site calls. A
                        // call-site id reused across a `runtime_collect` table clear (ADR-096)
                        // can leave a slot populated by a different arm for a different callee;
                        // the epoch guard alone wouldn't catch it (same epoch). Match the slot's
                        // resolved `sym`/`argc` against this site's baked `head`/`argc` — exactly
                        // the validation the IC probe paths do — or fall to the slow path, which
                        // re-resolves correctly. Without this the fast path would jump into the
                        // wrong native code with the wrong arity (a SIGSEGV in release).
                        b.switch_to_block(chk_ident);
                        let slot_sym = b.ins().load(types::I32, MemFlags::new(), slot_ptr, fl_sym_off);
                        let sym_ok = b.ins().icmp(IntCC::Equal, slot_sym, head_v);
                        let slot_argc = b.ins().load(types::I32, MemFlags::new(), slot_ptr, fl_argc_off);
                        let argc_ok = b.ins().icmp(IntCC::Equal, slot_argc, argc_v);
                        let ident_ok = b.ins().band(sym_ok, argc_ok);
                        b.ins().brif(ident_ok, hit, &[], miss, &[]);

                        // hit: read (code, nslots, env) and run the fast frame.
                        b.switch_to_block(hit);
                        let code_v = b.ins().load(types::I64, MemFlags::new(), slot_ptr, fl_code_off);
                        let nslots_v = b.ins().load(types::I32, MemFlags::new(), slot_ptr, fl_nslots_off);
                        let env_v = b.ins().load(types::I64, MemFlags::new(), slot_ptr, fl_env_off);
                        let ffc = b.ins().call(
                            fastframe_ref,
                            &[heap, out_addr, site_v, head_v, argc_v, nslots_v, code_v, env_v],
                        );
                        let fst = b.inst_results(ffc)[0];
                        // The callee may have relocated `roots`; re-fetch the base.
                        let rbc = b.ins().call(rb_ref, &[heap]);
                        b.def_var(rb_var, b.inst_results(rbc)[0]);
                        // status: 1 = error → `error`; 2 = could-not-link → `miss`; 0 = `cont`.
                        let is_err = b.ins().icmp_imm(IntCC::Equal, fst, 1);
                        let not_err = b.create_block();
                        b.ins().brif(is_err, error, &[], not_err, &[]);
                        b.switch_to_block(not_err);
                        let is_slow = b.ins().icmp_imm(IntCC::Equal, fst, 2);
                        b.ins().brif(is_slow, miss, &[], cont, &[]);

                        // miss: cold / redefined / over-cap / IC-moved → the slow dispatch.
                        b.switch_to_block(miss);
                        emit_call_slow(&mut b, cont);

                        b.switch_to_block(cont);
                        let (w0, w1, w2) = read_out(&mut b);
                        stack.push(Op::Handle(w0, w1, w2));
                    } else {
                        let cont = b.create_block();
                        emit_call_slow(&mut b, cont);
                        b.switch_to_block(cont);
                        let (w0, w1, w2) = read_out(&mut b);
                        stack.push(Op::Handle(w0, w1, w2));
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
                    let op = stack.pop()?;
                    store_op(&mut b, *i as i64, op);
                }
                Inst::Prim1 { op, .. } => {
                    let operand = stack.pop()?;
                    match op {
                        PrimOp1::First | PrimOp1::Rest => {
                            // Tag-check it's a Pair (deopt otherwise — the VM handles
                            // first/rest of nil / non-list / type error). The result is
                            // an arbitrary Value, so it's a Handle.
                            let [w0, w1, w2] = read_words(&mut b, operand);
                            let tagb = b.ins().band_imm(w0, 0xff);
                            let is_pair = b.ins().icmp_imm(IntCC::Equal, tagb, TAG_PAIR as i64);
                            let cont = b.create_block();
                            b.ins().brif(is_pair, cont, &[], deopt, &[]);
                            b.switch_to_block(cont);
                            let h = if let Some((nursery_base, old_base)) = pair_bases {
                                // Inline LOCAL pair read. PairId layout (w1):
                                //   bits 0..31  = index into the slab
                                //   bits 32..60 = gen epoch (ignored here)
                                //   bit  61     = age  (0=nursery, 1=old)
                                //   bits 62..63 = region (0=LOCAL, 1=PRELUDE, 2=RUNTIME)
                                // Deopt for non-LOCAL (PRELUDE/RUNTIME) — uncommon on hot
                                // cons-list paths; the VM handles those correctly.
                                let high2 = b.ins().ushr_imm(w1, 62);
                                let is_local =
                                    b.ins().icmp_imm(IntCC::Equal, high2, 0i64);
                                let local_cont = b.create_block();
                                b.ins().brif(is_local, local_cont, &[], deopt, &[]);
                                b.switch_to_block(local_cont);
                                // Age bit 61: 0=nursery, 1=old. After the LOCAL check, bits
                                // 62-63 are 0, so ushr by 61 gives exactly 0 or 1.
                                let age_shifted = b.ins().ushr_imm(w1, 61);
                                let is_old =
                                    b.ins().icmp_imm(IntCC::NotEqual, age_shifted, 0i64);
                                let base =
                                    b.ins().select(is_old, old_base, nursery_base);
                                // Index: lower 32 bits. stride = 48 (two 24-byte Values).
                                let idx = b.ins().band_imm(w1, 0xFFFF_FFFFi64);
                                let byte_off = b.ins().imul_imm(idx, 48i64);
                                let pair_ptr = b.ins().iadd(base, byte_off);
                                // Car at offset 0, cdr at offset 24 (one Value = 24 bytes).
                                let field_off: i64 =
                                    if matches!(op, PrimOp1::Rest) { 24 } else { 0 };
                                let field_ptr = if field_off == 0 {
                                    pair_ptr
                                } else {
                                    b.ins().iadd_imm(pair_ptr, field_off)
                                };
                                let rw0 =
                                    b.ins().load(types::I64, MemFlags::new(), field_ptr, 0);
                                let rw1 = b.ins().load(
                                    types::I64,
                                    MemFlags::new(),
                                    field_ptr,
                                    PAYLOAD_OFFSET as i32,
                                );
                                let rw2 = b.ins().load(
                                    types::I64,
                                    MemFlags::new(),
                                    field_ptr,
                                    PAYLOAD_OFFSET as i32 + 8,
                                );
                                Op::Handle(rw0, rw1, rw2)
                            } else {
                                let fref = match op {
                                    PrimOp1::First => car_ref,
                                    PrimOp1::Rest => cdr_ref,
                                    _ => unreachable!(),
                                };
                                call_handle(&mut b, fref, &[w0, w1, w2])
                            };
                            stack.push(h);
                        }
                        PrimOp1::IsNil => {
                            // Tag-only nil check: compare the tag byte to 0 (Tag::Nil).
                            // Result is an i8 comparison value (truthy in JumpIfFalse).
                            let [w0, _, _] = read_words(&mut b, operand);
                            let tagb = b.ins().band_imm(w0, 0xff);
                            let is_nil = b.ins().icmp_imm(IntCC::Equal, tagb, 0);
                            stack.push(Op::Int(is_nil));
                        }
                        PrimOp1::IsPair => {
                            // Tag-only pair check: compare the tag byte to TAG_PAIR.
                            // Ranges and SeqViews also carry TAG_PAIR — matching nil?/pair?
                            // semantics from builtins.rs.
                            let [w0, _, _] = read_words(&mut b, operand);
                            let tagb = b.ins().band_imm(w0, 0xff);
                            let is_pair = b.ins().icmp_imm(IntCC::Equal, tagb, TAG_PAIR as i64);
                            stack.push(Op::Int(is_pair));
                        }
                        PrimOp1::IsEmpty => {
                            // nil → true, pair → false, everything else → deopt.
                            // Vectors/maps/strings need a heap-length check — let the
                            // native handle them. nqueens `safe?` only ever sees nil/pair.
                            let [w0, _, _] = read_words(&mut b, operand);
                            let tagb = b.ins().band_imm(w0, 0xff);
                            let is_nil = b.ins().icmp_imm(IntCC::Equal, tagb, 0);
                            let is_pair = b.ins().icmp_imm(IntCC::Equal, tagb, TAG_PAIR as i64);
                            let is_nil_or_pair = b.ins().bor(is_nil, is_pair);
                            let cont = b.create_block();
                            b.ins().brif(is_nil_or_pair, cont, &[], deopt, &[]);
                            b.switch_to_block(cont);
                            // After the guard: is_nil is 1 for nil, 0 for pair — exactly
                            // the boolean result we want.
                            stack.push(Op::Int(is_nil));
                        }
                    }
                }
                Inst::MakeVector(n) => {
                    // Only arity 2 reaches here (gated by `chunk_in_jit_subset`); bail
                    // defensively otherwise. Same bump-allocate path as `cons`: read both
                    // operands as words (source order — `a` deeper, `b` on top), allocate.
                    if *n != 2 {
                        return None;
                    }
                    let (b_op, a_op) = (stack.pop()?, stack.pop()?);
                    let aw = read_words(&mut b, a_op);
                    let bw = read_words(&mut b, b_op);
                    let h = call_handle(
                        &mut b,
                        makevec2_ref,
                        &[aw[0], aw[1], aw[2], bw[0], bw[1], bw[2]],
                    );
                    stack.push(h);
                }
                Inst::Prim2 { op, map, .. } => {
                    // Operands were pushed in source order: `aa` (deeper) is source 0,
                    // `bb` (top) is source 1.
                    let (bb_op, aa_op) = (stack.pop()?, stack.pop()?);
                    if matches!(op, PrimOp::Cons) {
                        // `cons` takes any operands and allocates: car = source 0, cdr =
                        // source 1 (cons's `map` is `[0,1]`). Read each as words, alloc.
                        let car = read_words(&mut b, aa_op);
                        let cdr = read_words(&mut b, bb_op);
                        let h = call_handle(
                            &mut b,
                            cons_ref,
                            &[car[0], car[1], car[2], cdr[0], cdr[1], cdr[2]],
                        );
                        stack.push(h);
                    } else if matches!(op, PrimOp::VectorRef) {
                        // `(vector-ref v i)` / inlined `(nth v i)`: map is `[0,1]`, so
                        // source 0 (`aa`) is the vector, source 1 (`bb`) the index.
                        if let Op::HoistedVec { ptr, len, .. } = aa_op {
                            // Hoisted invariant global vector: inline `ptr + idx*STRIDE`
                            // (no slab-lookup call). Index tag-checks to int (deopt else);
                            // out-of-range deopts so the VM gives `nth`'s exact result.
                            let idx = as_int(&mut b, bb_op);
                            let oob = b.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, idx, len);
                            let cont = b.create_block();
                            b.ins().brif(oob, deopt, &[], cont, &[]);
                            b.switch_to_block(cont);
                            let off = b.ins().imul_imm(idx, STRIDE);
                            let elem = b.ins().iadd(ptr, off);
                            let w0 = b.ins().load(types::I64, MemFlags::new(), elem, 0);
                            let w1 =
                                b.ins()
                                    .load(types::I64, MemFlags::new(), elem, PAYLOAD_OFFSET as i32);
                            let w2 = b.ins().load(
                                types::I64,
                                MemFlags::new(),
                                elem,
                                PAYLOAD_OFFSET as i32 + 8,
                            );
                            stack.push(Op::Handle(w0, w1, w2));
                        } else {
                            let vec = read_words(&mut b, aa_op);
                            let idx = read_words(&mut b, bb_op);
                            let h = vector_ref(&mut b, vec, idx);
                            stack.push(h);
                        }
                    } else if op_is_float(aa_op) || op_is_float(bb_op) {
                        // Float arith/compare (an operand is a float). `pick` selects f64
                        // values the same as i64.
                        let aa = as_f64(&mut b, aa_op);
                        let bb = as_f64(&mut b, bb_op);
                        let x = pick(aa, bb, map[0]);
                        let y = pick(aa, bb, map[1]);
                        stack.push(emit_float_arith(&mut b, *op, x, y)?);
                    } else {
                        // Arithmetic/comparison: materialise to int, apply `map`.
                        let aa = as_int(&mut b, aa_op);
                        let bb = as_int(&mut b, bb_op);
                        let x = pick(aa, bb, map[0]);
                        let y = pick(aa, bb, map[1]);
                        stack.push(Op::Int(emit_arith(&mut b, *op, x, y)?));
                    }
                }
                Inst::Prim2SlotSlot {
                    op,
                    map,
                    slot_a,
                    slot_b,
                    ..
                } => {
                    if matches!(op, PrimOp::Cons) {
                        // `(cons slot_a slot_b)`: car = slot_a, cdr = slot_b (map `[0,1]`).
                        let car = read_words(&mut b, Op::Slot(*slot_a));
                        let cdr = read_words(&mut b, Op::Slot(*slot_b));
                        let h = call_handle(
                            &mut b,
                            cons_ref,
                            &[car[0], car[1], car[2], cdr[0], cdr[1], cdr[2]],
                        );
                        stack.push(h);
                    } else if matches!(op, PrimOp::VectorRef) {
                        // `(nth slot_a slot_b)`: source 0 = vector slot, source 1 = index
                        // slot (map `[0,1]`).
                        if let Some(&(ptr, vlen)) = hoisted.get(slot_a) {
                            // Hoisted invariant base: inline `ptr + idx*STRIDE` element read
                            // (no per-element call / slab lookup). The index slot tag-checks
                            // to int (deopt otherwise); an out-of-range index deopts so the
                            // VM produces `nth`'s exact out-of-range result.
                            let idx = load_slot_int(&mut b, *slot_b as i64);
                            let oob =
                                b.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, idx, vlen);
                            let cont = b.create_block();
                            b.ins().brif(oob, deopt, &[], cont, &[]);
                            b.switch_to_block(cont);
                            let off = b.ins().imul_imm(idx, STRIDE);
                            let elem = b.ins().iadd(ptr, off);
                            let w0 = b.ins().load(types::I64, MemFlags::new(), elem, 0);
                            let w1 = b.ins().load(
                                types::I64,
                                MemFlags::new(),
                                elem,
                                PAYLOAD_OFFSET as i32,
                            );
                            let w2 = b.ins().load(
                                types::I64,
                                MemFlags::new(),
                                elem,
                                PAYLOAD_OFFSET as i32 + 8,
                            );
                            stack.push(Op::Handle(w0, w1, w2));
                        } else {
                            // Read each operand as a full `Value`, then slab-read.
                            let vec = read_words(&mut b, Op::Slot(*slot_a));
                            let idx = read_words(&mut b, Op::Slot(*slot_b));
                            let h = vector_ref(&mut b, vec, idx);
                            stack.push(h);
                        }
                    } else if op_is_float(Op::Slot(*slot_a)) || op_is_float(Op::Slot(*slot_b)) {
                        // Float arith/compare on two slots (e.g. `(+ xx yy)`, `(* x y)`).
                        let sa = as_f64(&mut b, Op::Slot(*slot_a));
                        let sb = as_f64(&mut b, Op::Slot(*slot_b));
                        let x = pick(sa, sb, map[0]);
                        let y = pick(sa, sb, map[1]);
                        stack.push(emit_float_arith(&mut b, *op, x, y)?);
                    } else {
                        // Source 0 = slot_a, source 1 = slot_b (the VM's `[sa, sb]` order).
                        let sa = load_slot_int(&mut b, *slot_a as i64);
                        let sb = load_slot_int(&mut b, *slot_b as i64);
                        let x = pick(sa, sb, map[0]);
                        let y = pick(sa, sb, map[1]);
                        stack.push(Op::Int(emit_arith(&mut b, *op, x, y)?));
                    }
                }
                Inst::Prim2SlotInt {
                    op,
                    map,
                    slot_a,
                    int_b,
                    ..
                } => {
                    if matches!(op, PrimOp::VectorRef) {
                        // `(nth v 0)` / `(nth v 1)` — constant index fused into the slot:
                        // materialise `int_b` as a Value word-triple and call vector_ref.
                        // slot_a is always the vector (source 0 after map normalisation).
                        let vec = read_words(&mut b, Op::Slot(*slot_a));
                        let t = b.ins().iconst(types::I64, TAG_INT as i64);
                        let v = b.ins().iconst(types::I64, *int_b);
                        let z = b.ins().iconst(types::I64, 0);
                        let h = vector_ref(&mut b, vec, [t, v, z]);
                        stack.push(h);
                    } else
                    // `(cons slot int_literal)` or `(cons int_literal slot)` (after map
                    // inversion for the swapped form). After fusion, slot_a is always source
                    // 0; map[0]=0 → slot is car, int is cdr; map[0]=1 → int is car, slot
                    // is cdr (original was `(cons Const Local)`). Both map to brood_rt_cons.
                    if matches!(op, PrimOp::Cons) {
                        let slot_words = read_words(&mut b, Op::Slot(*slot_a));
                        let int_tag = b.ins().iconst(types::I64, TAG_INT as i64);
                        let int_val = b.ins().iconst(types::I64, *int_b);
                        let z = b.ins().iconst(types::I64, 0);
                        let int_words = [int_tag, int_val, z];
                        let (car, cdr) = if map[0] == 0 {
                            (slot_words, int_words)
                        } else {
                            (int_words, slot_words)
                        };
                        let h = call_handle(
                            &mut b,
                            cons_ref,
                            &[car[0], car[1], car[2], cdr[0], cdr[1], cdr[2]],
                        );
                        stack.push(h);
                    } else
                    if op_is_float(Op::Slot(*slot_a)) {
                        // `(op floatslot int-literal)` — Brood coerces the int to f64
                        // (`(+ 1.5 1)` = 2.5). Promote the literal and do float arith.
                        let sa = as_f64(&mut b, Op::Slot(*slot_a));
                        let sb = b.ins().f64const(*int_b as f64);
                        let x = pick(sa, sb, map[0]);
                        let y = pick(sa, sb, map[1]);
                        stack.push(emit_float_arith(&mut b, *op, x, y)?);
                    } else {
                        // Source 0 = slot_a, source 1 = the literal `int_b` (the fusion of
                        // `(Const, Local)` already inverted `map` so the slot is source 0).
                        let sa = load_slot_int(&mut b, *slot_a as i64);
                        let sb = b.ins().iconst(types::I64, *int_b);
                        let x = pick(sa, sb, map[0]);
                        let y = pick(sa, sb, map[1]);
                        stack.push(Op::Int(emit_arith(&mut b, *op, x, y)?));
                    }
                }
                Inst::Jump(t) => {
                    if *t == len {
                        // Jump straight to Done: return the single result via roots[base].
                        if stack.len() == 1 {
                            exit_done(&mut b, stack[0]);
                        } else {
                            // A reachable Done always leaves exactly one value, so a
                            // different stack height here means this block is **dead** — the
                            // bytecode compiler emits a jump-past-the-`else` after a branch
                            // that ended in a tail `SelfCall` (which never falls through), so
                            // it can't run. Terminate it by routing to `deopt`: never
                            // executes, and if the unreachability assumption were ever wrong
                            // it safely falls back to the VM rather than mis-returning. (This
                            // dead jump is why e.g. `collatz`'s `steps` arm wouldn't lower.)
                            b.ins().jump(deopt, &[]);
                        }
                    } else {
                        bool_param[*t] =
                            Some(stack.iter().map(|&op| is_bool_op(&b, op)).collect());
                        let args: Vec<BlockArg> = stack
                            .iter()
                            .map(|&op| BlockArg::Value(as_block_arg(&mut b, op)))
                            .collect();
                        b.ins().jump(leader_block[*t]?, &args);
                    }
                    break;
                }
                Inst::SelfCall { argc } => {
                    // Tail self-call (loop back-edge): pop the argc new args and write them
                    // into frame slots `0..argc`. Read every arg's `Value` into registers
                    // FIRST, then store — an arg may reference a slot being overwritten
                    // (e.g. `(f b a)`), so a read-as-you-store would alias. The reads are
                    // safepoint-free, so even a handle's bits are safe in a register here.
                    let mut ops = Vec::with_capacity(*argc);
                    for _ in 0..*argc {
                        ops.push(stack.pop()?);
                    }
                    ops.reverse(); // ops[i] = the i-th positional arg → frame slot i
                    if !stack.is_empty() {
                        return None;
                    }
                    // Each arg becomes a list of (byte-offset, word) stores. An `Int` is
                    // boxed (tag at 0, payload at PAYLOAD_OFFSET — the third word is left
                    // alone, irrelevant to an Int). A `Slot` copies **every** word of the
                    // `Value` (tag/payload/…) so a handle — including a `Pid` whose `id` is
                    // the third word at offset 16 — moves intact.
                    let mut vals: Vec<Vec<(i32, cranelift_codegen::ir::Value)>> =
                        Vec::with_capacity(*argc);
                    for &op in &ops {
                        match op {
                            Op::Int(v) => {
                                // Box as `Int`, or (a comparison `i8`) `Bool` — a loop can
                                // carry a boolean arg.
                                let (tag_byte, payload) = box_scalar(&mut b, v);
                                let tag = b.ins().iconst(types::I8, tag_byte as i64);
                                vals.push(vec![(0, tag), (PAYLOAD_OFFSET as i32, payload)]);
                            }
                            Op::Slot(k) => {
                                let roots_base = b.use_var(rb_var);
                                let i = b.ins().iadd_imm(base, k as i64);
                                let o = b.ins().imul_imm(i, STRIDE);
                                let addr = b.ins().iadd(roots_base, o);
                                let mut words = Vec::new();
                                let mut off = 0i32;
                                while (off as i64) < STRIDE {
                                    words.push((
                                        off,
                                        b.ins().load(types::I64, MemFlags::new(), addr, off),
                                    ));
                                    off += 8;
                                }
                                vals.push(words);
                            }
                            // A freshly-produced handle (cons/car/cdr result): its three
                            // words are already in registers — store all three.
                            Op::Handle(w0, w1, w2) => {
                                vals.push(vec![
                                    (0, w0),
                                    (PAYLOAD_OFFSET as i32, w1),
                                    (PAYLOAD_OFFSET as i32 + 8, w2),
                                ]);
                            }
                            // A hoisted global vector passed as a self-call arg — moves its
                            // three entry-resolved words verbatim, exactly like a `Handle`.
                            Op::HoistedVec { w0, w1, w2, .. } => {
                                vals.push(vec![
                                    (0, w0),
                                    (PAYLOAD_OFFSET as i32, w1),
                                    (PAYLOAD_OFFSET as i32 + 8, w2),
                                ]);
                            }
                            // A float arg — box as Value::Float (TAG_FLOAT + bits). The
                            // next iteration reads it back via `as_f64` (tag-checked).
                            Op::Float(v) => {
                                let bits = b.ins().bitcast(types::I64, MemFlags::new(), v);
                                let tag = b.ins().iconst(types::I8, TAG_FLOAT as i64);
                                vals.push(vec![(0, tag), (PAYLOAD_OFFSET as i32, bits)]);
                            }
                            // A crossed-boundary boolean (already `i64` 0/1) → Value::Bool.
                            Op::Bool(v) => {
                                let tag = b.ins().iconst(types::I8, TAG_BOOL as i64);
                                vals.push(vec![(0, tag), (PAYLOAD_OFFSET as i32, v)]);
                            }
                        }
                    }
                    let roots_base = b.use_var(rb_var);
                    for (i, words) in vals.iter().enumerate() {
                        let idx = b.ins().iadd_imm(base, i as i64);
                        let o = b.ins().imul_imm(idx, STRIDE);
                        let addr = b.ins().iadd(roots_base, o);
                        for &(off, w) in words {
                            b.ins().store(MemFlags::new(), w, addr, off);
                        }
                    }
                    // Register-carry update: keep carry_vars in sync with the new slot values.
                    // The `roots` stores above are kept for deopt; this additionally def_var's
                    // the unboxed i64/f64 so subsequent load_slot_int/as_f64 skip the tag-check.
                    // For Op::Int/Float, use the raw value directly. For any other op (slot
                    // passthrough), load from the just-stored roots payload — always correct and
                    // avoids parallel-assignment issues with cross-slot references.
                    if !carry_vars.is_empty() {
                        let rb2 = b.use_var(rb_var);
                        for (k, (&op, &(var, is_float))) in
                            ops.iter().zip(carry_vars.iter()).enumerate()
                        {
                            if is_float {
                                let f = match op {
                                    Op::Float(v) => v,
                                    _ => {
                                        let idx = b.ins().iadd_imm(base, k as i64);
                                        let o = b.ins().imul_imm(idx, STRIDE);
                                        let addr = b.ins().iadd(rb2, o);
                                        let bits = b.ins().load(
                                            types::I64,
                                            MemFlags::new(),
                                            addr,
                                            PAYLOAD_OFFSET as i32,
                                        );
                                        b.ins().bitcast(types::F64, MemFlags::new(), bits)
                                    }
                                };
                                b.def_var(var, f);
                            } else {
                                let raw = match op {
                                    Op::Int(v) => {
                                        if b.func.dfg.value_type(v) == types::I64 {
                                            v
                                        } else {
                                            b.ins().uextend(types::I64, v)
                                        }
                                    }
                                    _ => {
                                        let idx = b.ins().iadd_imm(base, k as i64);
                                        let o = b.ins().imul_imm(idx, STRIDE);
                                        let addr = b.ins().iadd(rb2, o);
                                        b.ins().load(
                                            types::I64,
                                            MemFlags::new(),
                                            addr,
                                            PAYLOAD_OFFSET as i32,
                                        )
                                    }
                                };
                                b.def_var(var, raw);
                            }
                        }
                    }
                    // GC safepoint (cons-allocating arms only): bound the nursery over loop
                    // iterations. Placed here — args already stored to slots, operand stack
                    // empty — so no handle is live in a register across the collection; the
                    // collector relocates the frame slots in place, leaving `roots_base`
                    // valid. (`car`/`rest` don't allocate, so non-cons arms skip it.)
                    if has_cons {
                        b.ins().call(sp_ref, &[heap]);
                    }
                    // Global-vector hoist guard: if any global was rebound since entry
                    // (`global_epoch` changed — only possible via another process's `def`,
                    // since this arm makes no Brood call), deopt so the VM re-runs the loop
                    // against the live binding. Keeps a hoisted global bit-identical to the
                    // VM's per-iteration late binding. Frame slots already hold the next
                    // iteration's args, so the VM resumes there.
                    if let Some(entry_ep) = entry_epoch {
                        // Raw load of the epoch counter (ptr fetched once at entry) — no FFI on
                        // the back-edge. A plain load matches the `Relaxed` atomic; the guard only
                        // needs to eventually observe a concurrent `def`'s bump.
                        let ep_ptr = epoch_ptr.expect("epoch_ptr fetched when a global is hoisted");
                        let now_ep = b.ins().load(types::I64, MemFlags::new(), ep_ptr, 0);
                        let changed = b.ins().icmp(IntCC::NotEqual, now_ep, entry_ep);
                        let ck = b.create_block();
                        b.ins().brif(changed, deopt, &[], ck, &[]);
                        b.switch_to_block(ck);
                    }
                    // Preemption (ADR-027): poll the reduction budget on the back-edge. On
                    // yield, deopt to `preempt` (return 2) — the frame slots already hold the
                    // next iteration's args (in `roots`), so the driver resumes on the VM.
                    // In **non-capture** mode (the root thread) the poll always returns 0, so gate
                    // it on the entry-read capture flag and jump straight to the loop top — no FFI.
                    let loop_top = leader_block[0]?;
                    if let Some(cap) = capture_active {
                        let poll = b.create_block();
                        b.ins().brif(cap, poll, &[], loop_top, &[]);
                        b.switch_to_block(poll);
                        let tc = b.ins().call(tick_ref, &[heap]);
                        let yld = b.inst_results(tc)[0];
                        b.ins().brif(yld, preempt, &[], loop_top, &[]);
                    } else {
                        let tc = b.ins().call(tick_ref, &[heap]);
                        let yld = b.inst_results(tc)[0];
                        b.ins().brif(yld, preempt, &[], loop_top, &[]);
                    }
                    break;
                }
                Inst::JumpIfFalse(t) => {
                    let cond = stack.pop()?;
                    let tgt = leader_block[*t]?; // falsy → else
                    let fall = leader_block[j + 1]?; // truthy → fall-through
                    bool_param[*t] = Some(stack.iter().map(|&op| is_bool_op(&b, op)).collect());
                    bool_param[j + 1] =
                        Some(stack.iter().map(|&op| is_bool_op(&b, op)).collect());
                    let args: Vec<BlockArg> = stack
                        .iter()
                        .map(|&op| BlockArg::Value(as_block_arg(&mut b, op)))
                        .collect();
                    match cond {
                        // A comparison result (`i8`) or a boolean that crossed a block
                        // boundary (`Op::Bool`, already `i64`): branch directly — nonzero
                        // (true) → truthy → fall-through, zero → else.
                        Op::Int(v) if b.func.dfg.value_type(v) != types::I64 => {
                            b.ins().brif(v, fall, &args, tgt, &args);
                        }
                        Op::Bool(v) => {
                            b.ins().brif(v, fall, &args, tgt, &args);
                        }
                        // A boxed condition in a slot/handle — e.g. `(and a b)` boxes its
                        // result to a temp slot (`box_scalar` tags it `Bool`), then reads it
                        // back. Load the tag (and payload) and branch on Brood truthiness:
                        // only `nil` and `false` are falsy, everything else truthy. (Before,
                        // this tag-checked `== Int` and *deopted* on a Bool/Nil, so every
                        // `and`/`or` in a hot arm fell to the VM. Branching here keeps it
                        // native and matches the VM's truthiness exactly.)
                        Op::Slot(_) | Op::Handle(..) => {
                            let (tagv, payload) = match cond {
                                Op::Slot(k) => {
                                    let roots_base = b.use_var(rb_var);
                                    let i = b.ins().iadd_imm(base, k as i64);
                                    let o = b.ins().imul_imm(i, STRIDE);
                                    let addr = b.ins().iadd(roots_base, o);
                                    let t8 = b.ins().load(types::I8, MemFlags::new(), addr, 0);
                                    let tagv = b.ins().uextend(types::I64, t8);
                                    let pl = b.ins().load(
                                        types::I64,
                                        MemFlags::new(),
                                        addr,
                                        PAYLOAD_OFFSET as i32,
                                    );
                                    (tagv, pl)
                                }
                                Op::Handle(w0, w1, _) => (b.ins().band_imm(w0, 0xff), w1),
                                _ => unreachable!(),
                            };
                            // falsy = (tag == Nil) || (tag == Bool && payload == 0). Nil's
                            // discriminant is 0.
                            let is_nil = b.ins().icmp_imm(IntCC::Equal, tagv, 0);
                            let is_bool = b.ins().icmp_imm(IntCC::Equal, tagv, TAG_BOOL as i64);
                            // A `Value::Bool`'s payload word is only meaningful in its low
                            // byte (the `bool`): Rust leaves the upper 7 bytes of the union
                            // slot uninitialised, so comparing the full `i64` to 0 spuriously
                            // reads `false` (byte 0, garbage above) as *truthy*. Mask to the
                            // bool byte — matching the VM's `Value::Bool(b)` read. (This is
                            // the bug that corrupted `nest format` once `not`/bool-const arms
                            // tiered: `(if x false true)` read its `false` arg as truthy.)
                            let pl_byte = b.ins().band_imm(payload, 0xff);
                            let pl_false = b.ins().icmp_imm(IntCC::Equal, pl_byte, 0);
                            let false_bool = b.ins().band(is_bool, pl_false);
                            let falsy = b.ins().bor(is_nil, false_bool);
                            b.ins().brif(falsy, tgt, &args, fall, &args);
                        }
                        // Any other unboxed SSA value (raw `Op::Int(i64)`, `Op::Float`) is
                        // always truthy in Brood.
                        _ => {
                            b.ins().jump(fall, &args);
                        }
                    }
                    break;
                }
                _ => return None,
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
                bool_param[j] = Some(stack.iter().map(|&op| is_bool_op(&b, op)).collect());
                let args: Vec<BlockArg> = stack
                    .iter()
                    .map(|&op| BlockArg::Value(as_block_arg(&mut b, op)))
                    .collect();
                b.ins().jump(leader_block[j]?, &args);
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
        static DUMP_IR: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let on = *DUMP_IR.get_or_init(|| {
            std::env::var("BROOD_JIT_DUMP_IR")
                .map(|v| !v.is_empty() && v != "0")
                .unwrap_or(false)
        });
        if on {
            // A compact bytecode fingerprint (opcode names — `Inst` has no `Debug`,
            // and `ConstVal`/`Value` are deliberately not `Debug`) to correlate the
            // CLIF to a source arm, then the CLIF itself.
            let ops: Vec<&str> = code.iter().map(inst_opcode_name).collect();
            eprintln!(
                "[jit-ir] ===== arm: {} insts: {} =====",
                code.len(),
                ops.join(" ")
            );
            // Per-Call (site, head) so the CLIF can be correlated to a source arm.
            for i in code.iter() {
                if let Inst::Call { site, head, argc, tail, .. } = i {
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
                    .unwrap_or_default();
                if want.split(',').any(|w| !w.is_empty() && name.contains(w)) {
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
            bytes.iter().map(|b| format!("{:02x}", b)).collect::<String>()
        );
    }
    Some(entry)
}
