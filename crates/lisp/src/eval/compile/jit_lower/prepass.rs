//! Pre-lowering analysis for `jit_lower_arm_inner` — pure, Cranelift-free passes
//! over the arm's bytecode that produce the data the emit loop consumes. Split out
//! of `jit_lower.rs` (the first step of decomposing that function): data in, data
//! out, no CLIF emitted. `#[cfg(feature = "jit")]` like the rest of the lowerer.
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
/// The walk used to `break` there silently, leaving every leader past that instruction
/// at `None` depth (sized as 0 params) while the emit loop — which lowers the
/// instruction fine — arrived with a deeper stack: a Cranelift verifier rejection
/// ("mismatched argument count") reported as `cranelift-define-function`, on every arm
/// with a `[…]` literal ahead of a join (`%match-splice-fail-in`, 2026-09-15). The
/// subset rule and this model must agree instruction for instruction; the caller
/// bails by NAME when they do not.
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
                Inst::Call { argc, head, .. } => {
                    cur += if head.is_some() {
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
