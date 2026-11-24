//! Pre-lowering analysis for `jit_lower_arm_inner` — pure, Cranelift-free passes over
//! the arm's bytecode that produce the data the emit loop consumes: data in, data out,
//! no CLIF emitted.
#![cfg(feature = "jit")]
use super::*;

/// Block leaders + the operand-stack depth at each leader, by abstract interp over
/// `code`. A leader is ip 0, every jump target, the inst after a jump/self-call, and
/// the implicit `len` "Done" block. `depth[ip]` is the operand-stack height entering
/// the block at `ip` (`None` = unreachable). All subset stack values are 64-bit; a
/// comparison `I8` is consumed by the `JumpIfFalse` in its own block, so it never
/// crosses a boundary. Returns `(is_leader, depth)`, both length `len + 1`.
pub(crate) fn block_analysis(code: &[Inst], len: usize) -> (Vec<bool>, Vec<Option<i32>>) {
    // Callers here (`max_leader_depth`, sizing a reserve) may hand over a chunk that is
    // NOT in the subset, so an unmodelled opcode is ordinary; only the lowering, which
    // has passed the subset rule, treats one as a bug (`block_analysis_checked`).
    let (leaders, depth, _) = block_analysis_checked(code, len);
    (leaders, depth)
}

/// [`block_analysis`] plus the opcode the depth walk had NO model for, if it met one.
/// Breaking silently there would leave every leader past that instruction at `None` depth
/// (sized as 0 params) while the emit loop arrives with a deeper stack — an opaque Cranelift
/// verifier rejection. The subset rule and this model must agree instruction for
/// instruction; the caller bails by NAME when they do not.
pub(crate) fn block_analysis_checked(
    code: &[Inst],
    len: usize,
) -> (Vec<bool>, Vec<Option<i32>>, Option<&'static str>) {
    let mut unmodelled: Option<&'static str> = None;
    // ---- Block leaders: ip 0, every jump target, the inst after a jump, the `len`
    // "done" block. ----
    let mut is_leader = vec![false; len + 1];
    is_leader[0] = true;
    is_leader[len] = true; // the implicit Done block
    for (ip, inst) in code.iter().enumerate() {
        match inst {
            Inst::Jump(t) | Inst::JumpIfFalse(t) => {
                is_leader[*t] = true;
                if ip < len {
                    is_leader[ip + 1] = true;
                }
            }
            // SelfCall jumps back to the loop header (block 0); the inst after it
            // (if any) starts a new (unreachable) block boundary.
            Inst::SelfCall { .. } => {
                if ip < len {
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
                // So is a STAGED named head (KI-19): its `GlobalIc` pushed the callee ahead of
                // the args, and the emit loop consumes it as a computed callee. Counting it as
                // unstaged put every leader after such a call one deeper than the emit stack,
                // and the arm bailed at the join (`join-depth-mismatch`).
                Inst::Call {
                    argc, head, staged, ..
                } => {
                    cur += if head.is_some() && !*staged {
                        1 - *argc as i32
                    } else {
                        -(*argc as i32)
                    };
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
                // Pops its staged captures, pushes the closure.
                Inst::MakeClosure { names, .. } => cur += 1 - names.len() as i32,
                // `[a b …]`: pops its `n` elements, pushes the vector.
                Inst::MakeVector(n) => cur += 1 - *n as i32,
                // `{k v …}`: pops its `2n` operands, pushes the map.
                Inst::MakeMap(n) => cur += 1 - 2 * *n as i32,
                // `(table-put t k v)`: pops three, pushes the table handle.
                Inst::Prim3 { .. } => cur -= 2,
                other => {
                    // Not in the subset (the arm was pre-bailed) — or in it and missing
                    // here, which the caller turns into a named bail.
                    unmodelled = Some(crate::eval::compile::jit_plan::codegen::inst_opcode_name(
                        other,
                    ));
                    break;
                }
            }
            j += 1;
            if is_leader[j] {
                work.push((j, cur));
                break;
            }
        }
    }
    (is_leader, depth, unmodelled)
}

/// The body-weight gate for a self-recursive arm that also makes a tail call. A **tail**
/// call returns to the driver (outcome 4) to dispatch the callee — a native↔driver round
/// trip per hop that a `SelfCall` loop avoids — so the arm must do enough work per
/// activation to amortise it: at least `TAIL_CALL_MIN_WORK` arithmetic/list prims or
/// non-tail calls (a 2-op ping/pong body runs slower native than interpreted). An arm
/// with no tail call has no round trip, and a non-self-recursive one is a pure delegator
/// whose round trip is amortised over all of its callee's work; both pass.
pub(crate) fn tail_call_work_is_enough(code: &[Inst]) -> bool {
    const TAIL_CALL_MIN_WORK: usize = 4;
    let has_tail_call = code
        .iter()
        .any(|i| matches!(i, Inst::Call { tail: true, .. }));
    let has_self_call = code.iter().any(|i| matches!(i, Inst::SelfCall { .. }));
    if !(has_tail_call && has_self_call) {
        return true;
    }
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
    work >= TAIL_CALL_MIN_WORK
}

/// How many operand-stack entries lie BELOW the operands of the non-tail calls in `code`,
/// summed over those calls — a bound on the call-result spills a lowering can make: a spill
/// stores an entry below a call's operands, and an entry spilled once is a slot after, never
/// spilled again. `None` when the depth model cannot place a call (an instruction it does not
/// model; such an arm bails before it lowers), and the caller then keeps its other bound.
///
/// This is what keeps counting the map and vector reads as handle producers from widening
/// every frame that reads a field: a read consumed before the next call never sits below one.
/// The straight-line stack effects are [`block_analysis_checked`]'s, which a depth mismatch
/// between the two would make a bail at lowering, never a corruption.
pub(crate) fn operands_below_calls(code: &[Inst]) -> Option<usize> {
    let len = code.len();
    let (is_leader, depth) = block_analysis(code, len);
    let mut total = 0usize;
    let mut current: Option<i32> = None;
    for (ip, inst) in code.iter().enumerate() {
        if is_leader[ip] {
            current = depth[ip];
        }
        let Some(height) = current else {
            // Unreachable code (no predecessor reached it): it never runs, so it spills nothing.
            continue;
        };
        current = match inst {
            Inst::Jump(_) | Inst::SelfCall { .. } | Inst::Call { tail: true, .. } => None,
            Inst::JumpIfFalse(_) => Some(height - 1),
            Inst::Const(_) | Inst::Local(_) | Inst::Global(_) | Inst::GlobalIc { .. } => {
                Some(height + 1)
            }
            Inst::Call {
                argc, head, staged, ..
            } => {
                let operands = if head.is_some() && !*staged {
                    *argc as i32
                } else {
                    *argc as i32 + 1
                };
                total += usize::try_from(height - operands).ok()?;
                Some(height - operands + 1)
            }
            Inst::Prim2SlotSlot { .. } | Inst::Prim2SlotInt { .. } => Some(height + 1),
            Inst::Prim2 { .. } => Some(height - 1),
            Inst::Prim1 { .. } => Some(height),
            Inst::Pop | Inst::SetLocal(_) => Some(height - 1),
            Inst::MakeClosure { names, .. } => Some(height + 1 - names.len() as i32),
            Inst::MakeVector(count) => Some(height + 1 - *count as i32),
            Inst::MakeMap(count) => Some(height + 1 - 2 * *count as i32),
            Inst::Prim3 { .. } => Some(height - 2),
            _ => return None,
        };
    }
    Some(total)
}

/// May a decoded vector storage pointer (`Frame::slot_vec_cache`) stay valid across `inst`?
///
/// The pointer is into storage an allocation can move: a small vector's elements live in its
/// `local.vectors` slab slot, which a push grows, and a collection relocates. So only
/// instructions that provably neither allocate nor call keep it: frame traffic, an inline
/// atom, the fused element read itself (whose fallback, `brood_rt_vector_ref`, only reads),
/// `count` (`Heap::count_inline`, a read), and the arithmetic and comparison operators,
/// which the lowering computes inline and deopts on anything they cannot (an overflow, a
/// ratio, a bignum) rather than allocate. Everything else — a call, a literal that builds,
/// a global read (which may load a module), `cons`, the table and map ops — clears it.
pub(crate) fn keeps_vector_storage(inst: &Inst) -> bool {
    let inline_operator = |op: &PrimOp| {
        matches!(
            op,
            PrimOp::Add
                | PrimOp::Sub
                | PrimOp::Mul
                | PrimOp::Lt
                | PrimOp::Le
                | PrimOp::Rem
                | PrimOp::Div
                | PrimOp::Quot
                | PrimOp::Max
                | PrimOp::Min
                | PrimOp::BitAnd
                | PrimOp::BitOr
                | PrimOp::BitXor
        )
    };
    match inst {
        Inst::Local(_) | Inst::SetLocal(_) | Inst::Pop => true,
        Inst::Const(ConstVal::Atom(_)) => true,
        Inst::Prim1 {
            op: PrimOp1::Count, ..
        } => true,
        Inst::Prim2SlotInt {
            op: PrimOp::VectorRef,
            ..
        } => true,
        Inst::Prim2 { op, .. } | Inst::Prim2SlotSlot { op, .. } | Inst::Prim2SlotInt { op, .. } => {
            inline_operator(op)
        }
        _ => false,
    }
}

/// For each instruction, whether it is a fused `(nth slot k)` that opens a run reading the
/// same slot's vector again — a destructuring `let`, or `(nth v 0)` … `(nth v 2)` written
/// out — before the run ends (a leader, an instruction [`keeps_vector_storage`] does not
/// vouch for, or a store to the slot). Those reads decode the storage once and the rest of
/// the run reuses it; a lone read keeps its own inline path, so a single `(nth v 0)` costs
/// what it did. Indexed by ip, length `code.len()`.
pub(crate) fn vector_reads_reused(code: &[Inst], is_leader: &[bool]) -> Vec<bool> {
    let read_slot = |inst: &Inst| match inst {
        Inst::Prim2SlotInt {
            op: PrimOp::VectorRef,
            slot_a,
            ..
        } => Some(*slot_a),
        _ => None,
    };
    let mut reused = vec![false; code.len()];
    for (start, inst) in code.iter().enumerate() {
        let Some(slot) = read_slot(inst) else {
            continue;
        };
        for (ip, next) in code.iter().enumerate().skip(start + 1) {
            if is_leader[ip] || !keeps_vector_storage(next) {
                break;
            }
            if matches!(next, Inst::SetLocal(target) if *target == slot) {
                break;
            }
            if read_slot(next) == Some(slot) {
                reused[start] = true;
                break;
            }
        }
    }
    reused
}
