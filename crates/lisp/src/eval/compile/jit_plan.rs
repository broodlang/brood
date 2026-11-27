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
//!   boxed-call shape regresses `nbody` 15–20%, and removing it (§7.1 step 2) regressed every
//!   row, winners included.
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
//! inside the jit-gated `jit_lower`, and a zero/`None` stub in `compile.rs` for builds
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
    // path (`spawn` paid for it in bloated prelude frames). Gating on the subset
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
                    | Inst::MakeMap(_)
                    | Inst::MakeClosure { .. }
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
                    // A map or vector read answers whatever the collection holds — a handle as
                    // often as not. Left out, an arm reading `(get m k)` or `(nth v i)` under a
                    // later call ran out of spill slots (and, before the call-spill capacity was
                    // kept below the block-argument window, spilled ONTO that window).
                    | Inst::Prim2 {
                        op: PrimOp::MapGet | PrimOp::SetHas | PrimOp::VectorRef,
                        ..
                    }
                    | Inst::Prim2SlotSlot {
                        op: PrimOp::MapGet | PrimOp::SetHas | PrimOp::VectorRef,
                        ..
                    }
                    | Inst::Prim2SlotInt {
                        op: PrimOp::MapGet | PrimOp::SetHas | PrimOp::VectorRef,
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
    // any type profile exists), so it lands on every lowerable arm — blanket-reserving
    // regresses `spawn` ~1.9x, which is why it stays behind the `chunk_in_jit_subset` gate.
    // **Change the `< 1 → 0` rule below only on a measured profitability case**, as it was
    // changed from `< 2`. An arm with ONE non-tail call that holds a handle below it — the
    // HOF loop `(loop (rest xs) f (cons (f (first xs)) acc))`, `map`'s own `%defseq` loop —
    // used to get reserve 0 here and bail mid-emit at `call-spill-exhausted`. The note that
    // stood here called that bail harmless (call-mediated, no win, wider frames cost `spawn`
    // +5–9%). Measured 2026-10-25, it was neither: lowering them took `(into [] (map (range n)
    // f))` from 570M to 406M instructions (−29%), with every `make ab --floor` row noise
    // (`spawn` −4.7%), `make ab-vm` noise, and no change to the per-process RSS slope. So
    // the rule is now "no non-tail call, no reserve": a call-free arm cannot spill.
    // A closure literal is a safepoint as a non-tail call is (`emit_make_closure` spills what
    // sits beneath its captures), so an arm whose only safepoint is a literal — `(pick (get m
    // :k) (fn …))` in tail position — needs slots too. Counting calls alone gave it 0, and 24
    // of womanblend's arms bailed `mkclo-spill-exhausted` (ADR-810).
    let closure_literals = code
        .iter()
        .filter(|inst| matches!(inst, Inst::MakeClosure { .. }))
        .count();
    if non_tail_call_count(code) + closure_literals < 1 {
        return 0;
    }
    // Two bounds on the call-result spills, both sound, so the smaller: one per handle
    // producer but the last, and the entries that ever sit below a call's operands.
    producers
        .saturating_sub(1)
        .min(operands_below_safepoints(code))
        + max_leader_depth(code)
}

#[cfg(not(feature = "jit"))]
fn operands_below_safepoints(_code: &[Inst]) -> usize {
    0
}

#[cfg(feature = "jit")]
fn operands_below_safepoints(code: &[Inst]) -> usize {
    crate::eval::compile::jit_lower::prepass::operands_below_safepoints(code).unwrap_or(usize::MAX)
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
/// unclamped want slides the window down onto the journal, so a later block-argument read
/// returns the packed journal word `(resume_ip << 16 | depth)` as a live value (or, with no
/// journal, writes past the frame top).
///
/// So the window is always the TOP `len` slots of the reserve, and `len` never exceeds it.
/// Where the reserve exists (the ≥2-non-tail-call shape, KI-49) `reserve >= want`. Where it
/// does not, `len` is 0 and a `Handle` crossing a boundary falls through to `ParamRepr::Int`,
/// which tag-checks and *deopts* rather than corrupting. Do not grow every frame instead: it
/// costs +2–7% on nearly every benchmark row, because `max_leader_depth` counts int/bool
/// merges that need no slot at all (`not`, `int?`, `inc`).
// Callers are the (gated) Cranelift lowering and the gated `codegen` tests; the VM sizes
// frames without it, so a no-jit build has no user.
#[cfg(feature = "jit")]
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
/// **Pure self-recursion** (`self_name`): an arm whose every `Call` — tail or not — targets
/// *itself*, with no mutating inline prim (`table-put`), no `try`/`catch` and no `binding`,
/// is effect-free by induction: a deopt's from-ip-0 re-run re-executes only completed
/// *self*-calls, which run this same pure arm (a redefinition mid-run bumps the epoch and
/// invalidates the arm before it re-enters native code). Such arms skip checkpointing
/// entirely ([`jit_ckpt_depth`]) — the journal writes after every non-tail call were ~5 %
/// of bintree's *instructions*, and the journal area and its per-activation reset another
/// 10 % (ADR-401 moved the in-call journal out of the frame so they need none). Anything
/// else — a call to another fn or a native (where all effects live), a computed callee,
/// `table-put`, a catch frame — keeps the exactly-once checkpoint machinery.
fn pure_self_recursive(
    code: &[Inst],
    self_name: Option<Symbol>,
    self_arity: Option<usize>,
) -> bool {
    let Some(me) = self_name else {
        return false;
    };
    code.iter().all(|i| match i {
        // `me` is the CLOSURE's name, shared by every arm of a multi-arity `defn` (each arm
        // gets the same `defn_name`). So a head match alone does not mean "calls back into
        // this same, provably effect-free arm" — a 1-arg arm calling `(f v 0)` dispatches
        // to the 2-arg arm, which may do anything, including a `table-put`. Require the
        // argc to select THIS arm, and `self_arity` is `None` for an arm with optionals or
        // a rest param (where argc → arm is not 1:1), which declines the exemption rather
        // than guessing.
        Inst::Call { head, argc, .. } => *head == Some(me) && Some(*argc) == self_arity,
        Inst::Prim3 {
            op: PrimOp3::TablePut,
            ..
        } => false,
        Inst::TryEnter { .. } | Inst::BindEnter { .. } => false,
        _ => true,
    })
}

/// Deopt-resume checkpointing (so a deopt never re-runs a side effect): the abstract operand-stack depth **after each non-tail
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
/// A pure self-recursive arm ([`pure_self_recursive`]) needs no journal: `None`.
pub(super) fn jit_ckpt_depth(
    code: &[Inst],
    self_name: Option<Symbol>,
    self_arity: Option<usize>,
) -> Option<usize> {
    if pure_self_recursive(code, self_name, self_arity) {
        return None; // effect-free re-run — no journal needed
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
            | Inst::Prim2SlotInt { .. } => merge(&mut depth, &mut work, ip + 1, d + 1),
            // The handler starts at the height `TryEnter` recorded (the driver truncates
            // back to it before running the handler), as the body does.
            Inst::TryEnter { handler, .. } => {
                merge(&mut depth, &mut work, ip + 1, d) && merge(&mut depth, &mut work, *handler, d)
            }
            Inst::TryExit => merge(&mut depth, &mut work, ip + 1, d),
            // Pops the vector of values; the body leaves its value, which `BindExit` keeps.
            Inst::BindEnter { .. } => d >= 1 && merge(&mut depth, &mut work, ip + 1, d - 1),
            Inst::BindExit(_) => merge(&mut depth, &mut work, ip + 1, d),
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
            // otherwise re-runs the arm from ip 0 on the VM and puts a second time (or
            // more: the VM re-run re-enters `jit_tier`) — even in an arm with no non-tail
            // call. Journaling here bounds it to exactly once and keeps the dense-table
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
            // The pure 3-ary map ops (ADR-368): pop three, push one, no effect to journal —
            // a deopt re-run recomputes a read or a fresh map, never a second store.
            Inst::Prim3 {
                op: PrimOp3::MapGet3 | PrimOp3::MapAssoc | PrimOp3::VectorRefOr,
                ..
            } => d >= 3 && merge(&mut depth, &mut work, ip + 1, d - 2),
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
                argc,
                tail,
                head,
                staged,
                ..
            } => {
                // A free-global head isn't staged (the IC resolves the callee), so
                // the call consumes only `argc` operands; a computed head adds one, and so
                // does a staged named head (KI-19), whose callee sits below the args.
                let consumed = argc + usize::from(head.is_none() || *staged);
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

/// Does `code` RECEIVE AT ITS HEAD (ADR-410)? Exactly one `%receive` call, and nothing before
/// it but loads and the vector literals a `receive` builds for its arguments — so re-running
/// the arm from its first instruction is the same as re-running that `%receive`. The shape of
/// a receive loop, `(defn loop (state) (receive …))`.
///
/// Such an arm may be compiled to native code: a `%receive` that would park leaves the native
/// activation and the driver re-runs the arm from ip 0 on the VM, which parks it there — at ip
/// 0, so the wake re-tiers into the native loop. One `%receive` only, because a second one
/// would sit after the first one's effect (the message it took), where a re-run from ip 0 is
/// no longer the same.
pub(crate) fn receive_at_head(code: &[Inst]) -> bool {
    let receive_sym = crate::core::value::intern("%receive");
    let is_receive = |i: &Inst| matches!(i, Inst::Call { head: Some(h), .. } if *h == receive_sym);
    let Some(at) = code.iter().position(is_receive) else {
        return false;
    };
    code[..at].iter().all(|i| {
        matches!(
            i,
            Inst::Const(_)
                | Inst::Local(_)
                | Inst::Global(_)
                | Inst::GlobalIc { .. }
                | Inst::MakeVector(_)
                | Inst::RecordLine(_)
        )
    }) && code[at + 1..].iter().all(|i| !is_receive(i))
        && matches!(code[at], Inst::Call { tail: false, .. })
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
                | PrimOp::SetHas
        )
        // `Cons` is admitted: the lowering calls `brood_rt_cons` (same bump-allocate
        // path as `brood_rt_vec2_room`, which works) and reads all 3 result words
        // back as a `Handle` (guarded by `jit_cons_test.blsp`).
    };
    // `%receive` fences the whole chunk out of the subset. A `receive` is just a `Call` to
    // this primitive, and it SUSPENDS: the nested-run machinery re-raises the park as a
    // `Control::Suspend` (an empty-message `LispError`), which the JIT's outcome-3 path
    // takes as a plain parked error — the process would die with `runtime error: ` (empty).
    // Today every receive also carries a `MakeClosure` (its matcher), which keeps the chunk
    // out anyway; this fence keeps that true if closures are ever admitted. Guarded by
    // `receive_bearing_chunks_stay_out_of_the_jit_subset` (compile/tests.rs).
    // The exception is a receive at the arm's head (ADR-410, [`receive_at_head`]): there the
    // park leaves native code as a re-run from ip 0, which the driver takes on the VM.
    let receive_sym = crate::core::value::intern("%receive");
    let receive_ok = receive_at_head(code);
    // `proc/interruptible` too (ADR-491): the driver runs the call as an interrupt region on its
    // own frame stack, so a `receive` in the thunk parks; called from native code it would run
    // the thunk nested under the builtin, where a `receive` blocks the worker. Read as a VALUE
    // as well (ADR-494): `(apply proc/interruptible …)` is a region on the VM.
    let interruptible_sym = crate::core::value::intern("proc/interruptible");
    // A `(fn …)` literal is admitted (ADR-803; `BROOD_NO_JIT_CLOSURES=1` keeps it out), but
    // never in a chunk that calls `%receive`: every receive builds its matcher as a literal, so a
    // receive-bearing chunk was kept out by the literal before, and a head receive (ADR-410)
    // keeps exactly the shapes it had.
    //
    // Nor in a chunk that is only call plumbing — loads, calls and the literal, with no branch,
    // primitive, literal collection or loop of its own (`map`'s `(fn (xs f) (… (fn …) …))`
    // wrapper, a setup closure that builds a row with `map`). Native code for it would run the
    // same callbacks the VM runs, so it costs a compile (two, with the hot re-lowering) and
    // gains nothing: matmul's setup paid +2.5% instructions for exactly that.
    let closures_ok = crate::diagnostics::debug_flags::jit_closures()
        && !code.iter().any(
            |inst| matches!(inst, Inst::Call { head: Some(head), .. } if *head == receive_sym),
        )
        && !only_call_plumbing(code);
    code.iter().all(|inst| match inst {
        Inst::Const(_) => true,
        Inst::Call { head: Some(h), .. } if *h == receive_sym => receive_ok,
        Inst::Call { head: Some(h), .. } if *h == interruptible_sym => false,
        Inst::Global(sym) | Inst::GlobalIc { sym, .. } if *sym == interruptible_sym => false,
        // A contract shim (KI-216): `%contract-tail?` reads the VM's frame stack, which a
        // native frame does not keep, and the mark `%contract-await` leaves only means
        // something to a frame the driver suspended. Both keep the chunk on the VM.
        Inst::Prim1 {
            op: PrimOp1::ContractTail,
            ..
        } => false,
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
        // `table-put`, `(get m k default)`, `(assoc m k v)` — each lowered as one
        // runtime-callback call (`brood_rt_table_put` / `_map_get3` / `_map_assoc`).
        Inst::Prim3 { .. } => true,
        // A vector literal `[a …]`. Arity 2 (bintree's `make`) lowers via the inline
        // `brood_rt_vec2_room`; a wider literal (nbody's `[vx vy vz]` / 7-body
        // rebuild) stages its elements into a Cranelift stack slot and calls the
        // variadic `brood_rt_make_vector_n`. Capped at 32 so the per-site staging slot
        // stays small (a huge literal in a hot arm is unheard-of; it bails to the VM).
        Inst::MakeVector(n) => *n <= 32,
        // A map literal `{k v …}` — `MakeVector`'s variadic shape exactly: the `2n` operands
        // are staged into a stack slot and `brood_rt_make_map_n` builds the map (a GC-quiet
        // in-place CHAMP build that allocates and never collects). Admitted so one `{…}` in
        // a hot arm does not put the whole arm on the interpreter for good. Same cap as the
        // vector, in values.
        Inst::MakeMap(n) => 2 * *n <= 32,
        // A `(fn …)` literal (`MakeClosure`): one `brood_rt_make_closure` callback, unless
        // `closures_ok` says the chunk must stay out (ADR-803).
        Inst::MakeClosure { names, .. } => closures_ok && names.len() <= 32,
        _ => false,
    })
}

/// Does `code` do nothing native code could do faster than the VM — no branch, primitive,
/// literal collection, self-loop or `try`, only loads, stores, calls and closure literals?
/// Keeps such a chunk with a `(fn …)` literal out of the subset (ADR-803).
fn only_call_plumbing(code: &[Inst]) -> bool {
    code.iter().all(|inst| {
        matches!(
            inst,
            Inst::Const(_)
                | Inst::RecordLine(_)
                | Inst::Local(_)
                | Inst::SetLocal(_)
                | Inst::Pop
                | Inst::Global(_)
                | Inst::GlobalIc { .. }
                | Inst::Call { .. }
                | Inst::MakeClosure { .. }
        )
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
            Inst::TryEnter { .. } => "TryEnter",
            Inst::TryExit => "TryExit",
            Inst::BindEnter { .. } => "BindEnter",
            Inst::BindExit(_) => "BindExit",
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
            Node::Binding { vals, body, .. } => {
                collect_self_call_args(vals, out);
                collect_self_call_args(body, out);
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
            Node::Binding { vals, body, .. } => {
                invariant_global_vecs(vals, out);
                invariant_global_vecs(body, out);
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
            // table-put deep-copies key and value; `assoc` path-copies a fresh trie. The
            // 3-arity map READ is the one 3-ary op that provably allocates nothing — the
            // same `Heap::map_get` reasoning as `MapGet` above.
            Inst::Prim3 { op, .. } => !matches!(op, PrimOp3::MapGet3 | PrimOp3::VectorRefOr),
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
    /// [`inst_may_allocate`] — the same *correctness* predicate the `pair_bases` hoist uses. Not a
    /// narrower list: the table ops count too, since their hashed path reconstructs a compound
    /// value into the caller's heap.
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

    /// How many reads of one parameter vector pay for resolving its base at entry. The
    /// resolution is a call (`brood_rt_vector_base`); the inline read it replaces is not, for a
    /// small LOCAL vector — so a pair read (`bintree`'s `(nth node 0)`/`(nth node 1)`) is
    /// cheaper left inline. A vec4 is read four times, a mat4 sixteen.
    pub const STRAIGHT_LINE_HOIST_MIN_READS: usize = 4;

    /// The parameter slots whose vector base a **straight-line** arm may hoist into its entry
    /// block, although the arm allocates — the 3D-math lever. `mat4-mul`, a point transform and
    /// a projection each read a parameter vector element by element and then build their
    /// result (`MakeVector`), so [`vector_base_hoist_safe`]'s whole-arm gate refused them and
    /// every `(nth m 5)` re-decoded the handle (and called out for a shared vector).
    ///
    /// Sound because nothing can move the storage before the last read: control only moves
    /// forward (no `Jump`/`JumpIfFalse` backward, no `SelfCall`, no `try` or `binding` frame,
    /// whose handlers and unwinds are not forward edges), so every instruction executed after
    /// the first allocation or call has a higher index than it, and every read of a hoisted
    /// slot comes before that index. A slot qualifies when it is a required parameter (only a
    /// parameter has its value at entry), no `SetLocal` writes it, its first read comes before
    /// any branch (the read is certain, so a non-vector there would have failed `nth` anyway —
    /// the entry deopt is not spurious), and it is read at least
    /// [`STRAIGHT_LINE_HOIST_MIN_READS`] times.
    pub fn straight_line_vector_hoist_slots(code: &[Inst], nrequired: usize) -> Vec<usize> {
        let forward_only = code.iter().enumerate().all(|(ip, inst)| match inst {
            Inst::Jump(target) | Inst::JumpIfFalse(target) => *target > ip,
            Inst::SelfCall { .. } | Inst::TryEnter { .. } | Inst::BindEnter { .. } => false,
            _ => true,
        });
        if !forward_only {
            return Vec::new();
        }
        let vector_read_slot = |inst: &Inst| match inst {
            Inst::Prim2SlotSlot {
                op: PrimOp::VectorRef,
                slot_a,
                ..
            }
            | Inst::Prim2SlotInt {
                op: PrimOp::VectorRef,
                slot_a,
                ..
            } => Some(*slot_a),
            _ => None,
        };
        let is_vector_read = |inst: &Inst| {
            matches!(
                inst,
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
            )
        };
        let first_unsafe = code
            .iter()
            .position(|inst| {
                !is_vector_read(inst)
                    && (matches!(inst, Inst::Call { .. }) || inst_may_allocate(inst))
            })
            .unwrap_or(code.len());
        let first_branch = code
            .iter()
            .position(|inst| matches!(inst, Inst::Jump(_) | Inst::JumpIfFalse(_)))
            .unwrap_or(code.len());
        let mut slots: Vec<usize> = Vec::new();
        for slot in 0..nrequired {
            if code
                .iter()
                .any(|inst| matches!(inst, Inst::SetLocal(written) if *written == slot))
            {
                continue;
            }
            let reads: Vec<usize> = code
                .iter()
                .enumerate()
                .filter(|(_, inst)| vector_read_slot(inst) == Some(slot))
                .map(|(ip, _)| ip)
                .collect();
            if reads.len() >= STRAIGHT_LINE_HOIST_MIN_READS
                && reads[0] < first_branch
                && reads.iter().all(|&ip| ip < first_unsafe)
            {
                slots.push(slot);
            }
        }
        slots
    }

    /// Does `i` allocate on its **fast path** — the gate for emitting a back-edge GC safepoint?
    ///
    /// Narrower than [`inst_may_allocate`] by exactly the table ops: including them costs `sieve`
    /// ~6%, because a dense table op lowers to an inline `xchg`/load that allocates nothing — only the hashed
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
            // `(assoc m k v)` allocates a fresh trie path per call — a hot loop of them is a
            // GC-quiet builder only if the back-edge safepoint can fire (ADR-368).
            Inst::Prim3 {
                op: PrimOp3::MapAssoc,
                ..
            } => true,
            _ => false,
        }
    }
    /// Is the unboxed-`i64` fast path enabled? Default ON; `BROOD_NO_I64` opts out. Read once:
    /// all processes of a runtime must agree, since the code is shared and the eligibility and
    /// frame decisions must be deterministic.
    pub fn jit_i64_enabled() -> bool {
        crate::diagnostics::debug_flags::jit_scalar_worker()
    }

    // ===================== the go/no-go =====================

    /// Why an arm the lowerer could otherwise handle should be left on the VM.
    ///
    /// Reportable on purpose: `BROOD_JIT_BAIL_TRACE=1` names the refusal, since absence from
    /// `BROOD_JIT_DUMP_IR` cannot distinguish a refused arm from one never hot or never tried.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum BailReason {
        /// **Call-mediated boxed work does not win natively.** The general lowering only beats
        /// the bytecode VM when it keeps hot values *unboxed* — an inline small-vector read
        /// (`VectorRef`/`MakeVector`, bintree) or a register-carried self-tail loop
        /// (loop/collatz/mandelbrot). An arm whose values flow through function calls and heap
        /// reads gains nothing: it must box/unbox a `Value` around each op *and* pay native-entry
        /// + FFI-callback + deopt cost, which the VM does without. This is `nbody`'s shape
        /// (`f`=`(nth (nth b i) k)`, plus `newvel`/`potential`/`advance-body`'s `f64` arith over
        /// `f` calls), where tiering **regresses** the benchmark 15–20%.
        CallMediatedBoxed,
    }

    impl BailReason {
        fn as_str(self) -> &'static str {
            match self {
                BailReason::CallMediatedBoxed => "call-mediated-boxed",
            }
        }
    }

    /// The backend-independent go/no-go for the **general** (boxed) lowering of `arm`.
    ///
    /// The profitability gate, and nothing else. Two things it deliberately does **not** do:
    ///
    /// - It does not gate the **scalar-register** path. That path is tried first, and this gate's
    ///   predicate describes `fib`/`pfib` — the arms it wins biggest on — so consulting it first
    ///   would silently stop them lowering. Still *correct* (they run on the VM), which is what
    ///   makes the mis-ordering dangerous: no test fails, only a benchmark moves.
    /// - It does not run [`chunk_in_jit_subset`]. That check is per-*chunk*, and the chunk a
    ///   lowering walks may be a spliced body rather than `arm.chunk` (the inlined-upgrade path),
    ///   so it stays where the chunk is known. Hoisting it here would change which arms lower.
    ///
    /// Returns `Ok(())` rather than a plan struct: the caller's only other decision is whether the
    /// scalar path is enabled ([`jit_i64_enabled`]), and it must consult that *before* this.
    ///
    /// **This gate encodes a COST model, so deopt feedback cannot replace it**: an admitted
    /// call-mediated arm compiles correctly and never type-deopts, so feedback has no signal.
    /// Removing the gate regresses every benchmark row (docs/compute-frontier.md §7.1); the
    /// design that can beat it is partial lowering, not admission-plus-feedback.
    pub fn plan_general_lowering(arm: &CompiledArm) -> Result<(), BailReason> {
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
        // The static call-mediated profitability bail — but for **top-level defns only**
        // (`dbg_name` set). A CLOSURE arm (a `reduce`/`fold` step, the HOF shape) is exempt:
        // making it native is what lets `hof_apply_native` skip the `vm_apply` trampoline per
        // element (nqueens −31%, pipeline −14%), and **deopt feedback** (`deopt_watch` in
        // `CompiledArm`) bails one that type-thrashes after 16 consecutive deopts, so a bad
        // closure shape self-heals instead of needing this static guess. Named defns keep the
        // gate verbatim: they are name-called from everywhere — including the per-process
        // compile machinery (macro expansion runs prelude Brood like `%match-count-sym`, `seq`,
        // `fold`) — and admitting those regressed `spawn` badly (contention around per-process
        // compile + shared-install under 10k-process fan-out) for zero row wins.
        //
        // The unboxing signals that earn the general lowering:
        //   * a `VectorRef`/`MakeVector` (rules bintree/matmul back in — they lower and win), or
        //   * a self-tail loop (`fold--loop`, hence `reduce`/`pipeline`; `row-sum`).
        if arm.dbg_name.is_some()
            && non_tail_call_count(code) >= 1
            && !has_inline_vec
            && !has_self_loop
        {
            // An arm whose calls the leaf inliner takes ALL of is not call-mediated once it
            // is upgraded: its spliced body (`LeafInline::resume`) holds no call at all. The
            // upgrade only ever follows a first lowering, so refusing that left such an arm
            // on the VM for good: a three-line `smoothstep` over `math/clamp` ran at 559 ns a
            // call where its spliced body takes 4. Admit it; the cost model above is about
            // calls that remain.
            #[cfg(feature = "jit")]
            if arm.leaf.as_ref().is_some_and(|leaf| {
                leaf.resume
                    .chunk
                    .as_ref()
                    .is_some_and(|spliced| non_tail_call_count(&spliced.code) == 0)
            }) {
                return Ok(());
            }
            return Err(record_bail(BailReason::CallMediatedBoxed));
        }
        Ok(())
    }

    /// Record a refusal for `BROOD_JIT_BAIL_TRACE=1` and hand it back. The caller drops the
    /// `Err` through `.ok()?`, and `jit_runtime::trace_lower_declined` then prints the ONE
    /// arm-named `[jit-bail]` line carrying the recorded reason.
    fn record_bail(reason: BailReason) -> BailReason {
        crate::eval::compile::jit_lower::record_mid_emit_reason(reason.as_str());
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
        // so under `BROOD_TIER=0` (the differential job) nothing compiles and the probe inspects
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
                     (w (map (range 50) (fn (i) i)) "" true)
                     (json/encode (map (range 50) (fn (i) {:a i}))))"#,
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
        // so under `BROOD_TIER=0` (the differential job) nothing compiles and the probe inspects
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
            .eval_str(r#"(w (map (range 50) (fn (i) i)) "" true)"#)
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
