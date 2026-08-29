//! Backend-independent lowering decisions — *whether* and *how* an arm should be lowered,
//! as opposed to the code generation that carries it out.
//!
//! Everything here is a pure function of the arm's bytecode, its `Node` body, its tier-time
//! slot profile, and the opt-out env flags. No Cranelift type appears: a second backend (or any
//! other consumer — a report, a tool) reads these decisions rather than re-deriving them.
//!
//! # Two tiers
//!
//! | tier | what it decides | gated on `feature = "jit"` |
//! |---|---|---|
//! | this module's top level | frame layout — [`jit_spill_reserve`], [`jit_ckpt_depth`] and the predicates they rest on | **no** — the VM sizes frames whether or not a backend exists |
//! | [`codegen`] | what emitted code may assume, where it may hoist, whether emitting is worth it | yes, once, on the module |
//!
//! So `jit_plan::codegen::…` at a use site says "this needs a backend to mean anything", and the
//! frame-layout half compiles in a build with no backend at all — where it is still needed.
//!
//! # Why this separation is the valuable half of the seam
//!
//! (`docs/backend-seams.md` §3.) Each decision here has a measurement session behind it, and
//! several encode a *negative* result that is invisible in the code that benefits from it:
//!
//! - [`codegen::plan_general_lowering`]'s call-mediated gate exists because lowering the
//!   boxed-call shape measurably regressed `nbody` 15–20%.
//! - [`codegen::inst_allocates_hot`] is deliberately narrower than
//!   [`codegen::inst_may_allocate`] because including the table ops cost `sieve` 6%.
//! - [`jit_spill_reserve`] is liveness-driven because a hardcoded `1` made a two-call recursion
//!   bail (`tests/jit.rs`).
//!
//! A backend re-implementing codegen is mechanical work. A backend re-deriving *these* would
//! re-pay for each of those findings, which is the real hazard a swap carries — so they live
//! above the backend, in one place, where they can be read without reading a lowerer.
//!
//! # What consolidating them removed
//!
//! [`jit_spill_reserve`] and [`jit_ckpt_depth`] were each defined **twice**: a real version
//! inside the jit-gated `jit_lower`, and a zero/`None` stub in `compile/mod.rs` for builds
//! without the feature. `jit_lower` *also* carried its own `#[cfg(not(feature = "jit"))]`
//! copies, which could never compile at all — the module they sit in only exists when the
//! feature is on. Four definitions, two of them unreachable; one each now.

use super::*;

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
///
/// The reserve has **two independent halves**, and they are gated differently (KI-64):
/// call-result spills need ≥2 non-tail calls to be possible at all, while
/// block-argument spills ([`max_leader_depth`]) depend only on how deep the operand
/// stack is where a block boundary falls — an arm with a *single* non-tail call still
/// needs them if an `if` sits inside that call's argument list. Returning 0 for the
/// whole reserve on the `<2 calls` test therefore under-provisioned exactly those arms,
/// and the lowering does not bail on an under-count here: it clamps
/// `blockarg_spill_base` to `spill_base`, which for a journalling arm *is* the
/// checkpoint slot. `walk-list`-shaped arms then wrote block arguments straight onto
/// the deopt journal, and a later block-argument read returned the journal word
/// `(resume_ip << 16 | depth)` as a live value — the `1114114` in KI-64. Arms without a
/// journal were worse: they wrote past the frame top entirely.
pub(crate) fn jit_spill_reserve(code: &[Inst]) -> usize {
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
                    | Inst::Prim2 {
                        op: PrimOp::TableGet,
                        ..
                    }
                    | Inst::Prim2SlotSlot {
                        op: PrimOp::TableGet,
                        ..
                    }
                    | Inst::Prim2SlotInt {
                        op: PrimOp::TableGet,
                        ..
                    }
                    | Inst::Prim3 { .. }
            )
        })
        .count();
    // KI-49: block-argument spills. An operand crossing a block boundary that is NOT a
    // profiled `Int` is carried as `ParamRepr::Slot(k)` rather than being forced through
    // `as_int` (which tag-checks Int and deopts otherwise — the whole KI-49 bug). An
    // `Op::Handle` has no slot to name, so it is spilled to one here first.
    //
    // The slot must be derived from the operand's STACK POSITION, not from `spill_next`'s
    // monotonic counter: every predecessor of a block has to name the same slot or
    // `record_block_flags` rejects the edge. So reserve one slot per operand-stack entry
    // that can be live at a leader — `max_leader_depth`.
    //
    // This reserve is unavoidably profile-independent (it runs at arm construction, before
    // any type profile exists), so it lands on every lowerable arm. That is the shape whose
    // cost is already on record — blanket-reserving regressed `spawn` ~1.9x — which is why
    // it stays behind the `chunk_in_jit_subset` gate above and why the change was measured
    // (spawn / fib / collatz / pingpong) rather than reasoned about.
    if non_tail_call_count(code) < 2 {
        return 0;
    }
    producers.saturating_sub(1) + max_leader_depth(code)
}

/// Where a lowering's **block-argument spill** slots live: `(base, len)`, as frame slot
/// indices. `reserve` is what [`jit_spill_reserve`] gave this arm's frame and
/// `frame_top_for_spills` is the first slot the spill area may NOT touch — the checkpoint
/// journal for a journalling lowering, the frame top otherwise.
///
/// The clamp is the whole point (KI-64). [`max_leader_depth`] is how many block-argument
/// slots the chunk could *want*, and it is computed under a different gate from the reserve:
/// `jit_spill_reserve` returns 0 for an arm with a single non-tail call, which still wants
/// block-argument slots as soon as an `if` sits inside that call's argument list
/// (`(w (rest xs) (step (first xs) (if first? acc (cons "," acc))) false)`). Honouring the
/// unclamped want slid the window down onto the journal, and a later block-argument read
/// then returned the packed journal word `(resume_ip << 16 | depth)` as a live value — the
/// `1114114` a JSON-encoding service saw as `empty?: expected collection, got int`. An arm
/// with no journal wrote past the frame top instead.
///
/// So the window is always the TOP `len` slots of the reserve, and `len` never exceeds it.
/// Where the reserve exists (the ≥2-non-tail-call shape KI-49 measured) `reserve >= want`
/// and this is exactly what KI-49 shipped. Where it does not, `len` is 0 and a `Handle`
/// crossing a boundary falls through to `ParamRepr::Int` — the pre-KI-49 behaviour, which
/// tag-checks and *deopts* rather than corrupting. Growing every frame instead was measured
/// and rejected: it costs +2–7% on nearly every benchmark row, because `max_leader_depth`
/// counts int/bool merges that need no slot at all (`not`, `int?`, `inc`).
pub(crate) fn blockarg_spill_window(
    code: &[Inst],
    reserve: usize,
    frame_top_for_spills: usize,
) -> (usize, usize) {
    let len = max_leader_depth(code).min(reserve);
    let base = frame_top_for_spills - len;
    debug_assert!(
        base + len <= frame_top_for_spills,
        "block-argument spill window [{base}, {}) reaches frame_top {frame_top_for_spills}",
        base + len
    );
    (base, len)
}

/// The deepest operand stack at any block leader — an upper bound on how many operands
/// can cross a block boundary at once, and so on the block-argument spill slots
/// [`jit_spill_reserve`] must provide. Uses the same `block_analysis` the lowering does,
/// so the two cannot disagree about where leaders are. Private: the lowering asks
/// [`blockarg_spill_window`] for the window rather than re-deriving it from this, which is
/// what keeps the clamp in one place (KI-64).
/// `0` without a backend: `jit_lower` (and its `prepass`) do not exist in that build, and
/// the reserve's contract is that a `--without-jit` build's frames are unchanged.
#[cfg(not(feature = "jit"))]
fn max_leader_depth(_code: &[Inst]) -> usize {
    0
}

#[cfg(feature = "jit")]
fn max_leader_depth(code: &[Inst]) -> usize {
    let len = code.len();
    let (_, depth) = crate::eval::compile::jit_lower::prepass::block_analysis(code, len);
    depth
        .iter()
        .filter_map(|d| *d)
        .filter(|d| *d > 0)
        .max()
        .unwrap_or(0) as usize
}
/// Deopt-resume checkpointing (the fix for the deopt-rerun side-effect bug,
/// devlog 2026-07-16): the abstract operand-stack depth **after each non-tail
/// `Call` completes**, maximised over all call sites — the number of extra frame
/// slots the JIT'd arm needs to journal its live operands at each checkpoint.
/// `None` when the chunk has no non-tail call (nothing to checkpoint — deopts
/// re-run from ip 0, which is then effect-free by construction: everything the
/// boxed subset can execute besides calls is pure or idempotent), or when the
/// static pass can't assign a consistent depth (never expected from this
/// compiler's structured output — checkpointing is then disabled and the arm
/// keeps the legacy re-run behaviour).
///
/// The pass is a tiny abstract interpreter over the chunk: propagate the stack
/// depth instruction by instruction, following both branch edges; depths must
/// agree wherever control merges.
///
/// **Pure-self-recursion exemption** (`self_name`): an arm whose every `Call` —
/// tail or not — targets *itself*, with no mutating inline prim (`table-put`)
/// and no `try`/`catch`, is effect-free by induction: a deopt's from-ip-0
/// re-run re-executes only completed *self*-calls, which run this same pure
/// arm (a redefinition mid-run bumps the epoch and invalidates the arm before
/// it re-enters native code). Such arms skip checkpointing entirely — the
/// journal writes after every non-tail call were ~5 % of bintree's
/// *instructions* (two self-calls per node × 819k nodes; invisible in
/// wall-clock noise, plain in `perf stat -e instructions` and matching the
/// `BROOD_NO_DEOPT_RESUME=1` lever). Anything else — a call to another fn or
/// a native (where all effects live), a computed callee, `table-put`, a catch
/// frame — keeps the exactly-once checkpoint machinery.
pub(super) fn jit_ckpt_depth(
    code: &[Inst],
    self_name: Option<Symbol>,
    self_arity: Option<usize>,
) -> Option<usize> {
    if std::env::var_os("BROOD_NO_DEOPT_RESUME").is_some() {
        return None; // chicken switch: legacy from-ip-0 re-run everywhere
    }
    if let Some(me) = self_name {
        let pure_self = code.iter().all(|i| match i {
            // `me` is the CLOSURE's name, shared by every arm of a multi-arity `defn`
            // (each arm gets the same `defn_name`). So a head match alone does not mean
            // "calls back into this same, provably effect-free arm" — a 1-arg arm calling
            // `(f v 0)` dispatches to the 2-arg arm, which may do anything, including a
            // `table-put`. Require the argc to select THIS arm, and `self_arity` is
            // `None` for an arm with optionals or a rest param (where argc → arm is not
            // 1:1), which declines the exemption rather than guessing.
            Inst::Call { head, argc, .. } => *head == Some(me) && Some(*argc) == self_arity,
            Inst::Prim3 {
                op: PrimOp3::TablePut,
                ..
            } => false,
            Inst::TryCatch { .. } => false,
            _ => true,
        });
        if pure_self {
            return None; // effect-free re-run — no journal needed
        }
    }
    let len = code.len();
    let mut depth: Vec<Option<usize>> = vec![None; len + 1];
    depth[0] = Some(0);
    let mut work = vec![0usize];
    let mut max_after_ckpt: Option<usize> = None;
    // merge: assign-or-check a depth at ip; push to the worklist on first visit.
    fn merge(depth: &mut [Option<usize>], work: &mut Vec<usize>, ip: usize, d: usize) -> bool {
        match depth[ip] {
            None => {
                depth[ip] = Some(d);
                work.push(ip);
                true
            }
            Some(prev) => prev == d,
        }
    }
    while let Some(ip) = work.pop() {
        if ip >= len {
            continue; // the implicit Done block
        }
        let d = depth[ip].expect("worklist entries have depths");
        let ok = match &code[ip] {
            Inst::Const(_)
            | Inst::Local(_)
            | Inst::Global(_)
            | Inst::GlobalIc { .. }
            | Inst::Prim2SlotSlot { .. }
            | Inst::Prim2SlotInt { .. }
            | Inst::TryCatch { .. } => merge(&mut depth, &mut work, ip + 1, d + 1),
            // A coverage-instrumented arm must not be JIT'd: native code would run
            // without the recording, silently under-reporting exactly the hot arms.
            // Returning false here bails the whole lowering for this arm, which is the
            // conservative answer (`--cover-lines` also disables the JIT outright).
            Inst::RecordLine(_) => false,
            // Branch coverage recording, like RecordLine, bails the arm's JIT lowering so
            // a hot branch isn't silently under-reported (coverage runs with the JIT off
            // anyway; this is belt-and-braces).
            Inst::RecordBranch(..) => false,
            Inst::Pop | Inst::SetLocal(_) => d >= 1 && merge(&mut depth, &mut work, ip + 1, d - 1),
            Inst::Prim1 { .. } => d >= 1 && merge(&mut depth, &mut work, ip + 1, d),
            Inst::Prim2 { .. } => d >= 2 && merge(&mut depth, &mut work, ip + 1, d - 1),
            // `table-put` is the one *effect* in the boxed subset. It must be a
            // checkpoint site for the same reason a completed call is: a deopt after it
            // otherwise re-runs the arm from ip 0 on the VM and puts a second time. It
            // is not hypothetical — before this, an arm with a `table-put` and **no**
            // non-tail call got no journal at all (the accumulator stayed `None`), and a
            // 200 000-iteration driver landed a counter on 402 047. (Worse, the VM
            // re-run re-enters `jit_tier`, so each deopting activation put *three*
            // times.) Journaling here bounds it to exactly once and keeps the dense-table
            // lowering (the sieve lever), which refusing to lower such arms would lose.
            Inst::Prim3 {
                op: PrimOp3::TablePut,
                ..
            } => {
                let after = d.checked_sub(2);
                match after {
                    Some(a) if d >= 3 => {
                        max_after_ckpt = Some(max_after_ckpt.map_or(a, |m: usize| m.max(a)));
                        merge(&mut depth, &mut work, ip + 1, a)
                    }
                    _ => false,
                }
            }
            Inst::MakeVector(n) => d >= *n && merge(&mut depth, &mut work, ip + 1, d - n + 1),
            Inst::MakeMap(n) => d >= 2 * n && merge(&mut depth, &mut work, ip + 1, d - 2 * n + 1),
            Inst::MakeClosure { names, .. } => {
                d >= names.len() && merge(&mut depth, &mut work, ip + 1, d - names.len() + 1)
            }
            Inst::Jump(t) => merge(&mut depth, &mut work, *t, d),
            Inst::JumpIfFalse(t) => {
                d >= 1
                    && merge(&mut depth, &mut work, *t, d - 1)
                    && merge(&mut depth, &mut work, ip + 1, d - 1)
            }
            Inst::SelfCall { argc } => d >= *argc, // terminal (frame reset + loop)
            Inst::Call {
                argc, tail, head, ..
            } => {
                // A free-global head isn't staged (the IC resolves the callee), so
                // the call consumes only `argc` operands; a computed head adds one.
                let consumed = argc + usize::from(head.is_none());
                if d < consumed {
                    false
                } else if *tail {
                    true // terminal: the driver reuses the frame
                } else {
                    let after = d - consumed + 1;
                    max_after_ckpt = Some(max_after_ckpt.map_or(after, |m| m.max(after)));
                    merge(&mut depth, &mut work, ip + 1, after)
                }
            }
        };
        if !ok {
            return None; // inconsistent depths — disable checkpointing for this arm
        }
    }
    max_after_ckpt
}

/// Count of non-tail Brood→Brood calls in `code` — the shape that needs a handle spill
/// (≥2) and drives the spill-reserve / lowering gates.
fn non_tail_call_count(code: &[Inst]) -> usize {
    code.iter()
        .filter(|i| matches!(i, Inst::Call { tail: false, .. }))
        .count()
}

/// True iff every opcode in `code` is in the integer JIT subset — i.e. a backend could lower
/// this chunk (modulo the handle-spill, which is what the reserve enables).
///
/// The single source of truth for the subset, consulted from two places: [`jit_spill_reserve`],
/// so only genuinely-lowerable arms get spill frame slots, and the general lowering's own
/// pre-bail. It is **not** part of [`codegen::plan_general_lowering`] on purpose — the check is
/// per-*chunk*, and the chunk a lowering walks may be a spliced body rather than `arm.chunk`
/// (the inlined-upgrade path), so it belongs where the chunk is known. Ungated for the same
/// reason `jit_spill_reserve` is: the frame size depends on it.
pub(super) fn chunk_in_jit_subset(code: &[Inst]) -> bool {
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
                | PrimOp::TableHas
                | PrimOp::TableGet
                | PrimOp::MapGet
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
        // `table-put` — lowered as one runtime-callback call (brood_rt_table_put).
        Inst::Prim3 {
            op: PrimOp3::TablePut,
            ..
        } => true,
        // A vector literal `[a …]`. Arity 2 (bintree's `make`) lowers via the inline
        // `brood_rt_make_vector2`; a wider literal (nbody's `[vx vy vz]` / 7-body
        // rebuild) stages its elements into a Cranelift stack slot and calls the
        // variadic `brood_rt_make_vector_n`. Capped at 32 so the per-site staging slot
        // stays small (a huge literal in a hot arm is unheard-of; it bails to the VM).
        Inst::MakeVector(n) => *n <= 32,
        _ => false,
    })
}

/// Decisions only a **code generator** consults: what the emitted code may assume, where it
/// may hoist, and whether emitting it is worth doing at all.
///
/// Gated once, here, rather than per item — a build with no backend has no caller for any of
/// them, while everything above this point describes frame layout the VM needs either way.
/// The import path is the documentation: `jit_plan::codegen::…` at a use site says "this needs
/// a backend to mean anything".
#[cfg(feature = "jit")]
pub(super) mod codegen {
    use super::*;

    /// Opcode name of an `Inst`, for the `BROOD_JIT_DUMP_IR` fingerprint. `Inst` (and its
    /// `ConstVal`/`Value` payloads) are intentionally not `Debug`, so this names the
    /// variant without touching the payload. Exhaustive on purpose — a new `Inst` variant
    /// must be added here.
    pub fn inst_opcode_name(inst: &Inst) -> &'static str {
        match inst {
            Inst::Const(_) => "Const",
            Inst::RecordLine(_) => "RecordLine",
            Inst::RecordBranch(..) => "RecordBranch",
            Inst::Local(_) => "Local",
            Inst::Prim3 { .. } => "Prim3",
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
            Node::Prim3 { a, b, c, .. } => {
                collect_self_call_args(a, out);
                collect_self_call_args(b, out);
                collect_self_call_args(c, out);
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
    pub fn invariant_param_slots(body: &Node, nrequired: usize) -> Vec<bool> {
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
    pub fn invariant_global_vecs(node: &Node, out: &mut std::collections::HashSet<Symbol>) {
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
            Node::Prim3 { a, b, c, .. } => {
                invariant_global_vecs(a, out);
                invariant_global_vecs(b, out);
                invariant_global_vecs(c, out);
            }
            Node::Prim1 { a, .. } => invariant_global_vecs(a, out),
            Node::TryCatch { body, handler, .. } => {
                invariant_global_vecs(body, out);
                invariant_global_vecs(handler, out);
            }
            Node::Const(_) | Node::Local(_) | Node::Global(_) | Node::GlobalIc { .. } => {}
        }
    }

    /// Can executing `i` **allocate** into the LOCAL heap? Anything that can invalidates an
    /// entry-hoisted slab base pointer (`local.pairs`/`local.vectors` are `Vec`s — a push can
    /// reallocate), so an arm containing one must not hoist those bases.
    ///
    /// This is the *correctness* predicate. `table-get`/`table-has?` are in it because they take
    /// a hashed branch for out-of-shape keys and call `from_message`, which does `alloc_pair` /
    /// `alloc_vector` per element — an arm mixing `first`/`rest` with a table read hoisted a pair
    /// base at entry and kept using it across a reallocation, a use-after-free that survived only
    /// because glibc often extends a large block in place.
    ///
    /// Deliberately **wider** than [`inst_allocates_hot`], which gates the back-edge GC
    /// safepoint: being conservative here only costs an inline read, and the two are coupled in
    /// the safe direction — a safepoint that can fire must imply the hoist is off, never the
    /// reverse.
    pub fn inst_may_allocate(i: &Inst) -> bool {
        match i {
            Inst::Prim2 { op, .. }
            | Inst::Prim2SlotSlot { op, .. }
            | Inst::Prim2SlotInt { op, .. } => {
                // `MapGet` is deliberately absent: `Heap::map_get` takes `&self`, so a CHAMP
                // probe provably neither allocates nor collects — it returns a `Value`
                // already stored in the map rather than reconstructing one, which is what
                // puts the table reads on this list. Listing it would cost the vector-base
                // hoist around every map read for nothing.
                matches!(
                    op,
                    PrimOp::Cons | PrimOp::TableGet | PrimOp::TableHas | PrimOp::VectorRef
                )
            }
            Inst::Prim3 { .. } => true, // table-put: the store deep-copies key and value
            Inst::MakeVector(_) | Inst::MakeMap(_) | Inst::MakeClosure { .. } => true,
            _ => false,
        }
    }

    /// May this chunk hoist a **LOCAL vector's element base** into the entry block (the matmul
    /// LICM lever — `brood_rt_vector_base`, then inline `ptr + idx*STRIDE` reads)?
    ///
    /// The base is a raw pointer into the element storage, and nothing refreshes it: a small
    /// vector's elements live INLINE in its `local.vectors` slab slot (`VecStore::Inline`), and
    /// a spilled one's buffer is re-derived by `VecStore::spill` on a GC copy — so anything that
    /// can move or free that storage while the arm runs leaves every later read pointed at freed
    /// memory. Reads there return garbage `Value` words with valid-looking tags, and the per-deref
    /// tripwire cannot see them: the read never goes through a `Heap` accessor.
    ///
    /// So: no non-tail `Call` (a GC safepoint, and a `def` → RUNTIME compaction), and nothing
    /// [`inst_may_allocate`] — the same *correctness* predicate the `pair_bases` hoist uses, and
    /// the one this gate used to be narrower than (it listed `Call`/`MakeVector`/`Cons` and left
    /// out the table ops, whose hashed path reconstructs a compound value into the caller's heap).
    /// `brood_rt_vector_base`'s own justification is "gated to arms that neither allocate nor make
    /// a Brood→Brood call"; this is that sentence, as code.
    ///
    /// `VectorRef` is the one exception to the allocation rule: `inst_may_allocate` lists it
    /// conservatively, but `brood_rt_vector_ref` neither allocates nor collects — and it is the
    /// very instruction being hoisted, so counting it would disable the hoist everywhere.
    ///
    /// Deliberately NOT used for the two *global* hoists. A global was promoted out of LOCAL by
    /// the `def` that bound it, and a shared region's slab is append-only with stable refs, so an
    /// allocation cannot move what they point at (only a compaction can — which needs a safepoint
    /// and is caught by the back-edge `entry_epoch` guard). Widening their gate would cost the
    /// sieve lever, whose dense-table hoist rides on the scalar-global hoist beside a `table-put`.
    pub fn vector_base_hoist_safe(code: &[Inst]) -> bool {
        !code.iter().any(|i| {
            if matches!(
                i,
                Inst::Prim2 {
                    op: PrimOp::VectorRef,
                    ..
                } | Inst::Prim2SlotSlot {
                    op: PrimOp::VectorRef,
                    ..
                } | Inst::Prim2SlotInt {
                    op: PrimOp::VectorRef,
                    ..
                }
            ) {
                return false;
            }
            matches!(i, Inst::Call { .. }) || inst_may_allocate(i)
        })
    }

    /// Does `i` allocate on its **fast path** — the gate for emitting a back-edge GC safepoint?
    ///
    /// Narrower than [`inst_may_allocate`] by exactly the table ops, and measured: including them
    /// cost `sieve` **6%** (confirmed solo at best-of-15 after a 8% sweep reading), because a
    /// dense table op lowers to an inline `xchg`/load that allocates nothing — only the hashed
    /// FFI fallback can, and that is the uncommon shape.
    ///
    /// Leaving them out does not let the nursery run away, which is what the safepoint is for:
    /// the back edge emits `brood_rt_tick_n` **independently of this predicate**, so a native
    /// loop still yields on its reduction quantum and a collection runs there. Growth is bounded
    /// by one quantum, exactly as it already is for every other arm that allocates nothing on its
    /// fast path.
    pub fn inst_allocates_hot(i: &Inst) -> bool {
        match i {
            Inst::Prim2 { op, .. }
            | Inst::Prim2SlotSlot { op, .. }
            | Inst::Prim2SlotInt { op, .. } => {
                matches!(op, PrimOp::Cons)
            }
            Inst::MakeVector(_) | Inst::MakeMap(_) | Inst::MakeClosure { .. } => true,
            _ => false,
        }
    }
    /// Is `BROOD_JIT_DUMP_IR` armed? Read once and cached, so an ordinary run pays one
    /// `var` at most. Shared by every lowering path that reports an arm — the general one and
    /// the scalar-register worker — so "did this arm lower?" has a single switch rather than
    /// one per code generator.
    pub fn jit_dump_ir_enabled() -> bool {
        static DUMP_IR: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *DUMP_IR.get_or_init(|| {
            std::env::var("BROOD_JIT_DUMP_IR")
                .map(|v| !v.is_empty() && v != "0")
                .unwrap_or(false)
        })
    }

    /// Is the unboxed-`i64` fast path enabled? **Default ON** (`BROOD_NO_I64` opts out — the A/B
    /// baseline lever). Read once (all processes of a runtime must agree — the code is shared and
    /// the eligibility/frame decisions must be deterministic).
    pub fn jit_i64_enabled() -> bool {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ON.get_or_init(|| std::env::var_os("BROOD_NO_I64").is_none())
    }

    // ===================== the go/no-go =====================

    /// Why an arm the lowerer could otherwise handle should be left on the VM.
    ///
    /// Reportable on purpose. A refusal used to be a bare `None` returned from inside the
    /// lowering, observable only as *absence* from `BROOD_JIT_DUMP_IR` — which reads identically
    /// for an arm that was never hot, was never tried, or lowered through the scalar-register
    /// path (that path emits no dump line at all). `BROOD_JIT_BAIL_TRACE=1` names the refusal
    /// instead of leaving it to be inferred from silence.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum BailReason {
        /// **Call-mediated boxed work does not win natively.** The general lowering only beats
        /// the bytecode VM when it keeps hot values *unboxed* — an inline small-vector read
        /// (`VectorRef`/`MakeVector`, bintree) or a register-carried self-tail loop
        /// (loop/collatz/mandelbrot). An arm whose values flow through function calls and heap
        /// reads gains nothing: it must box/unbox a `Value` around each op *and* pay native-entry
        /// + FFI-callback + deopt cost, which the VM does without. This is `nbody`'s shape
        /// (`f`=`(nth (nth b i) k)`, plus `newvel`/`potential`/`advance-body`'s `f64` arith over
        /// `f` calls), where tiering measurably **regressed** the benchmark 15–20%.
        CallMediatedBoxed,
    }

    impl BailReason {
        fn as_str(self) -> &'static str {
            match self {
                BailReason::CallMediatedBoxed => "call-mediated-boxed",
            }
        }
    }

    /// The backend-independent go/no-go for the **general** (boxed) lowering of `arm`, given its
    /// tier-time `slot_tags`.
    ///
    /// Exactly the profitability gate that used to sit inline in `jit_lower_arm`, and nothing
    /// else. Two things it deliberately does **not** do:
    ///
    /// - It does not gate the **scalar-register** path. That path is tried first, and this gate's
    ///   predicate describes `fib`/`pfib` — the arms it wins biggest on — so consulting it first
    ///   would silently stop them lowering. Still *correct* (they run on the VM), which is what
    ///   makes the mis-ordering dangerous: no test fails, only a benchmark moves.
    /// - It does not run [`chunk_in_jit_subset`]. That check is per-*chunk*, and the chunk a
    ///   lowering walks may be a spliced body rather than `arm.chunk` (the inlined-upgrade path),
    ///   so it stays where the chunk is known. Hoisting it here would change which arms lower —
    ///   precisely what this move must not do.
    ///
    /// Returns `Ok(())` rather than a plan struct: the caller's only other decision is whether the
    /// scalar path is enabled ([`jit_i64_enabled`]), and it must consult that *before* this.
    /// Bundling the two into one value would invite exactly the mis-ordering above (ADR-011 — a
    /// shape with no reader is a shape that misleads).
    pub fn plan_general_lowering(arm: &CompiledArm, slot_tags: &[u8]) -> Result<(), BailReason> {
        let Some(chunk) = arm.chunk.as_ref() else {
            return Ok(()); // no chunk to judge — the lowerer's own pre-bail handles it
        };
        let code = &chunk.code;
        let has_inline_vec = code.iter().any(|i| {
            matches!(
                i,
                Inst::MakeVector(_)
                    | Inst::Prim2 {
                        op: PrimOp::VectorRef,
                        ..
                    }
                    | Inst::Prim2SlotSlot {
                        op: PrimOp::VectorRef,
                        ..
                    }
                    | Inst::Prim2SlotInt {
                        op: PrimOp::VectorRef,
                        ..
                    }
            )
        });
        let has_self_loop = code.iter().any(|i| matches!(i, Inst::SelfCall { .. }));
        let has_float_slot = slot_tags.contains(&(crate::core::value::Tag::Float as u8));
        // The static call-mediated profitability bail — but for **top-level defns only**
        // (`dbg_name` set). A CLOSURE arm (a `reduce`/`fold` step, the HOF shape) is exempt:
        // making it native is what lets `hof_apply_native` skip the `vm_apply` trampoline per
        // element (nqueens −31%, pipeline −14%), and **deopt feedback** (`deopt_watch` in
        // `CompiledArm`) bails one that type-thrashes after 16 consecutive deopts, so a bad
        // closure shape self-heals instead of needing this static guess. Named defns keep the
        // gate verbatim: they are name-called from everywhere — including the per-process
        // compile machinery (macro expansion runs prelude Brood like `%match-count-sym`, `seq`,
        // `fold`) — and admitting those regressed `spawn` 0.08 → 0.3–1.3 s erratic (contention
        // around per-process compile + shared-install under 10k-process fan-out) for zero row
        // wins.
        //
        // The unboxing signals that earn the general lowering:
        //   * a `VectorRef`/`MakeVector` (rules bintree/matmul back in — they lower and win), or
        //   * a self-tail loop, UNLESS the profile shows a `Float` slot (a recursive `f64`
        //     accumulator like `newvel`, whose floats still arrive boxed from calls — no win),
        // so a self-tail loop over *non-float* boxed values (`fold--loop`, hence
        // `reduce`/`pipeline`) is preserved.
        if arm.dbg_name.is_some()
            && non_tail_call_count(code) >= 1
            && !has_inline_vec
            && (!has_self_loop || has_float_slot)
        {
            return Err(trace_bail(arm, BailReason::CallMediatedBoxed));
        }
        Ok(())
    }

    /// Report a refusal under `BROOD_JIT_BAIL_TRACE=1` and hand it back. One `var_os` behind a
    /// cached bool when off, so an ordinary run pays nothing.
    fn trace_bail(arm: &CompiledArm, reason: BailReason) -> BailReason {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *ON.get_or_init(|| std::env::var_os("BROOD_JIT_BAIL_TRACE").is_some()) {
            let name = arm
                .dbg_name
                .map(crate::core::value::symbol_name_ref)
                .unwrap_or("<closure>");
            eprintln!("[jit-bail] arm={name} reason={}", reason.as_str());
        }
        reason
    }
}

/// The frame-layout invariant KI-64 broke, asserted on the arithmetic rather than on a
/// program's output.
///
/// The behavioural guard (`tests/jit_blockarg_spill_test.blsp`) can only fail when *both*
/// the clamp in `jit_lower_arm` and its `spill-area-exceeds-frame` backstop are gone — the
/// backstop alone keeps a mis-sized arm off the native path, so it would hide the slip
/// instead of corrupting. That is the right runtime behaviour and the wrong test behaviour.
/// So reproduce the lowering's slot arithmetic here and assert the area lands where the
/// frame layout says it may: strictly below the checkpoint journal, or below the frame top
/// for an arm that does not journal.
#[cfg(all(test, feature = "jit"))]
mod tests {
    use super::*;
    use crate::Interp;

    /// For every arm a real boot compiles: the block-argument spill area must not reach the
    /// deopt journal. `max_leader_depth` is what the chunk *wants* and `jit_spill_reserve`
    /// is what the frame *has*; the two are gated differently, so the lowering must clamp
    /// the want to the reserve. Before KI-64 it did not, and `%fold-loop`,
    /// `%index-of-seq-from`, `json/emit-list` and every `walk-list`-shaped arm wrote block
    /// arguments straight onto their journal slot.
    #[test]
    fn block_argument_spills_never_reach_the_deopt_journal() {
        // Ceiling 2 explicitly: this asserts on VM-compiled arms and JIT lowering arithmetic,
        // so under `BROOD_VM=0` (the differential job) nothing compiles and the probe inspects
        // zero chunks — the same pin `compile/tests.rs` documents for its two native tests.
        set_forced_ceiling(Some(Tier::Native));
        let mut interp = Interp::new();
        // Exercise the shapes: a walker with an `if` inside a call's argument list is the
        // one that regressed, and `json/encode` is the same shape in the standard library.
        interp
            .eval_str(
                r#"(do
                     (defn- w (xs acc first?)
                       (if (empty? xs) acc
                         (w (rest xs) (str (first xs) (if first? acc "x")) false)))
                     (w (map (fn (i) i) (range 50)) "" true)
                     (json/encode (map (fn (i) {:a i}) (range 50))))"#,
            )
            .expect("warm the compiler over the offending shapes");
        let mut checked = 0usize;
        let mut bad: Vec<String> = Vec::new();
        for arm in interp.heap.dbg_compiled_arms() {
            let Some(chunk) = arm.chunk.as_ref() else {
                continue;
            };
            let code = &chunk.code;
            if !chunk_in_jit_subset(code) {
                continue; // reserves nothing and never lowers
            }
            checked += 1;
            // The same window `jit_lower_arm` uses, for the small (non-spliced) layout —
            // the shared function, so removing its clamp fails this test rather than
            // silently re-corrupting frames.
            let reserve = jit_spill_reserve(code);
            let frame_top = if arm.ckpt_slot != u32::MAX {
                arm.ckpt_slot as usize
            } else {
                arm.nslots
            };
            let (base, len) = blockarg_spill_window(code, reserve, frame_top);
            if base + len > frame_top {
                let name = arm
                    .dbg_name
                    .map(crate::core::value::symbol_name)
                    .unwrap_or_else(|| "<anon>".to_string());
                bad.push(format!(
                    "{name}: block-arg area [{base}, {}) reaches frame_top {frame_top} \
                     (reserve {reserve}, wants {})",
                    base + len,
                    max_leader_depth(code)
                ));
            }
        }
        assert!(
            checked > 20,
            "only {checked} lowerable chunks inspected — the probe found nothing to check, \
             so a green result would mean nothing"
        );
        assert!(
            bad.is_empty(),
            "a block-argument spill slot reaches the deopt journal (or the frame top), so a \
             later read of it returns the packed journal word as a live value (KI-64):\n  {}",
            bad.join("\n  ")
        );
    }

    /// The clamp itself, stated separately: the lowering may never plan more
    /// block-argument slots than the frame reserved. This is the line that broke.
    #[test]
    fn the_block_argument_want_is_clamped_to_the_reserve() {
        // Ceiling 2 explicitly: this asserts on VM-compiled arms and JIT lowering arithmetic,
        // so under `BROOD_VM=0` (the differential job) nothing compiles and the probe inspects
        // zero chunks — the same pin `compile/tests.rs` documents for its two native tests.
        set_forced_ceiling(Some(Tier::Native));
        let mut interp = Interp::new();
        interp
            .eval_str(
                r#"(defn- w (xs acc first?)
                     (if (empty? xs) acc
                       (w (rest xs) (str (first xs) (if first? acc "x")) false)))"#,
            )
            .expect("compile the offending shape");
        interp
            .eval_str(r#"(w (map (fn (i) i) (range 50)) "" true)"#)
            .expect("run it");
        let mut saw_a_clamped_arm = false;
        for arm in interp.heap.dbg_compiled_arms() {
            let Some(chunk) = arm.chunk.as_ref() else {
                continue;
            };
            let code = &chunk.code;
            if !chunk_in_jit_subset(code) {
                continue;
            }
            let reserve = jit_spill_reserve(code);
            if max_leader_depth(code) > reserve {
                saw_a_clamped_arm = true;
            }
        }
        assert!(
            saw_a_clamped_arm,
            "no arm wants more block-argument slots than it reserved, so the clamp this test \
             guards is not being exercised — the corpus has drifted and a green result here \
             would mean nothing"
        );
    }
}
