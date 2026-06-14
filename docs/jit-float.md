# JIT float support — design notes (next session)

Goal: let the tier-1 JIT compile **float** loops, the way back-edge tiering +
`VectorRef` codegen now handle integer loops. The motivating benchmark is
`mandelbrot`: `esc` is a pure self-tail loop of `f64` arithmetic (`+ * - <= /`),
so if its body lowered it would run native like `loop` did (the outer
`row-sum`/`grid-sum` are minor). `collatz`/`fib`/`bintree` are **not** float-gated —
they're gated on call dispatch (a separate ABI item).

## Why this needs type *specialization* (not just float codegen)

The JIT operand model is integer: `Op::Int` (an `i64` SSA value), `Op::Slot`
(a frame slot, read by `as_int` which **tag-checks `Int` and deopts otherwise**),
`Op::Handle`. The integer fast path is correct precisely because a non-int slot
deopts.

`esc`'s float values (`xx`, `yy`, `x`, `y`, `x0`, `y0`) arrive as **frame slots**,
and the JIT cannot know statically whether a slot holds an int or a float — so
`(+ xx yy)` compiles to `Prim2SlotSlot{Add}` with no static type. Two ways to
handle it, only one is right:

- **Runtime tag-dispatch per op** (check tags, branch int/float, box the result):
  rejected — it *regresses* the integer loops (`loop`/`collatz`) that path also
  serves, and boxes every result.
- **Type-specialized tiering** (the plan): when an arm goes hot, read the live
  frame's slot tags, compile a version specialized to those types with **entry
  guards** that deopt if a later activation passes different types.

## Sketch

1. **Profile at tier time.** In `jit_tier`, just before enqueue, snapshot the slot
   tags from `roots[base .. base+nslots]` (Int / Float / other) into a small array
   carried to `jit_lower_arm` (e.g. on the queued work item, or recomputed by
   re-reading a representative frame). `esc` → `{x,y,xx,yy,x0,y0: Float, i: Int}`.
2. **Entry guards.** At the top of the lowered arm, for each profiled Float slot
   emit a tag-check `== Float` → `deopt`; same for Int slots. This makes the body's
   type assumptions sound; a differently-typed call deopts to the VM (and re-tiers).
3. **`Op::Float`.** Add a float SSA operand. `Const(Float)` → `Op::Float`; a
   profiled-Float `Slot` read → `as_f64` (load the `f64` payload, no coercion —
   the guard already proved the tag). Float arith results → `Op::Float`.
4. **`emit_arith` float arms.** `fadd`/`fsub`/`fmul`/`fdiv` and `fcmp` for `<`/`<=`
   (mirror `prim_apply_float`'s edges: `/` by zero → deopt). Decide float-vs-int per
   op by the operand `Op` types (now known via the profile), not at runtime.
5. **`box_float`.** Box an `Op::Float` back into a `Value::Float` (tag + `f64` bits)
   when storing to a slot / self-call arg / returning. Mirror `box_scalar`.
6. **Pre-bail.** Allow `Const(Float)` (currently only `Const(Int)` passes).

## Correctness gauntlet (this is the risky part — test hard)

- `mandelbrot` checksum **must** match `BROOD_VM=0` (float results are exact-bit
  sensitive; a wrong fcmp/coercion shows up here).
- `breakagetests` (JIT GC/deopt/loop stress) 37/37.
- The entry guards must cover **every** slot the body reads as a typed value — a
  missing guard is a miscompile (reads an int's payload as `f64` or vice-versa).
- A `def` that changes a callee's arg types must deopt cleanly (the guard + the
  existing epoch invalidation).

## Status of the surrounding work (already landed on `perf/jit-call-dispatch`)

- Back-edge tiering — self-tail loops now tier (loop 5.4×).
- `VectorRef` Cranelift codegen — matmul's indexed loop runs native (1.4×).
- `catch_unwind` around `jit_lower_arm` — a codegen panic can no longer silently
  disable the JIT (do future `brood_rt_*` symbol registration with this in mind).

---

# Implementation status & findings (2026-06-14, branch `perf/jit-float`)

The float codegen from the sketch above **is implemented and correct**, but it does
**not** yield the `mandelbrot` win, because `esc`'s control flow hits a cascade of
*separate* JIT-infrastructure blockers — none of them about floats. This section is the
full record so the next attempt doesn't re-walk it.

## What landed (correct, verified)

Implemented per the sketch, with one simplification: instead of separate entry guards
(step 2), soundness comes from a **per-read tag-check** in `as_f64`/`as_int` (a slot whose
runtime tag isn't `Float`/`Int` deopts). `slot_float[]` (seeded from the tier-time profile,
updated when a float result is stored to a slot) only chooses *which opcode* to emit.

- **Profile plumbing.** `jit_tier` snapshots `tag(roots[base+i]) as u8` per slot and sends
  it on the bounded `JIT_COMPILER` channel; `jit_lower_arm(jit, arm, slot_tags)`.
  **Gotcha:** the profile is the *lattice* `Tag` enum (`Tag::Float == 3`), NOT the in-memory
  `Value` discriminant byte (`jit_layout::TAG_FLOAT == 4`). Seed `slot_float` from `Tag::Float`;
  use `TAG_FLOAT` only for the in-memory tag byte in `as_f64`/`store`. Conflating them = the
  float path silently never fires (cost me a long detour).
- **`Op::Float`** (unboxed `f64` SSA): `Const(Float)`, float-slot reads, float-arith results.
  `as_f64`, `emit_float_arith` (`fadd`/`fsub`/`fmul`; `fcmp` Lt/Le/Eq → an `i8` bool, same as
  integer compares), float dispatch in all three `Prim2*` lowering arms via `op_is_float`,
  `read_words`/`store_op` box it as `Value::Float`, `Const(Float)`/`Float` in
  `chunk_in_jit_subset`. **Verified:** a top-level pure-`f64` self-tail loop runs native,
  ~20× (4.06s→0.20s), exact-bit identical to `BROOD_VM=0`.

- **`and`/`or` in a JIT'd arm (pre-existing bug, now fixed).** `(and a b)` macro-expands to
  `(let (g a) (if g b g))`. The compare is `box_scalar`'d to a slot (`Value::Bool`, tag 1),
  then `JumpIfFalse` read it back and tag-checked `== Int` → **deopted on every Bool/Nil
  condition**, so every `and`/`or` in a hot arm fell to the VM. Fixed: `JumpIfFalse` on an
  `Op::Slot`/`Op::Handle` now loads the tag and branches on Brood truthiness (falsy iff `nil`
  or `false`-bool), matching the VM exactly. Slot-based `and`/`or` now lower (verified on
  reduced cases). Also added `Op::Bool` + a `bool_param` table (recorded at jump sites) so a
  boolean that *crosses a block boundary* via a block param reconstructs as `Op::Bool` (boxes
  as `Bool`, branches correctly) instead of a plain `Op::Int` (which boxed as `Int` =
  truthy-always = wrong).

## Why `mandelbrot`'s `esc` still does NOT win — the cascade

`esc` is `(if (and (<= (+ xx yy) 4.0) (< i maxi)) <self-tail-recur> i)`. Every structural
form of it trips a different deep JIT bug:

1. **As written (`and`).** The `and`'s 2nd compare is left on the operand stack at the `Jump`
   to the inner-`if` merge — a **boolean crossing a block boundary**. Block params are `i64`,
   so it needs `as_int` to zero-extend `i8→i64`; the Cranelift verifier rejects the bare `i8`
   block-arg otherwise. With the widening + `Op::Bool` reconstruction it lowers and is
   correct for ~5000 iterations — then **HANGS**. `perf` (gdb blocked by `ptrace_scope=1`,
   so `/proc/<tid>/wchan` + `perf record -p`) shows `brood-main` spinning in `vm_run_bc` →
   esc-native + `brood_rt_tick`: a **preempt/back-edge ping-pong**. When the reduction budget
   exhausts, esc's back-edge preempts and the dispatch re-enters without converging. The
   *same shape without `and`* (an extra block fewer) handles preempt fine, so it's the `and`'s
   extra merge block × the preempt-resume path. Could not isolate the non-convergence. The
   `as_int` widening is therefore **reverted on this branch** (so esc bails the verifier and
   stays on the VM — correct, no hang, no win, +~2% tiering overhead).

2. **Rewritten as nested `if`** (`(if A (if B X Y) Y)` — no boolean value-merge). Runs correct,
   no hang, **but bails a different verifier error**: `jump block5(v224): got 1, expected 2` —
   a block-param **depth mismatch**. The dead `Jump` after the tail `SelfCall` (inside the
   nested then-branch) leaves the leader/depth analysis inconsistent between predecessors. So
   the depth analysis can't handle a dead-jump-after-tailcall *inside a nested if*. esc runs
   on the bytecode VM (jit ≈ no-jit instruction count, flat).

**Conclusion.** The float codegen is sound; the blocker is purely `esc`'s nested self-tail-loop
control flow. Unlocking it needs real JIT-internals work — pick one:
  (a) fix the preempt/back-edge ping-pong for multi-merge-block arms (re-add the widening first);
  (b) fix the block-depth analysis for dead-jumps-after-tailcall in nested `if`s;
  (c) type-aware (or spill-slot-routed) block params so booleans survive crossings without the
      widening, and revisit the preempt path.
None is a safe small change — it's a focused project.

## What's committed on this branch

`crates/lisp/src/core/value.rs` (+`TAG_FLOAT` layout const & test) and
`crates/lisp/src/eval/compile.rs` (all of the above). Correctness gates: `mandelbrot`/`matmul`
exact-bit vs `BROOD_VM=0`, no hang; differential `engines_agree_on_corpus` passes; 263 unit
tests pass. (Two jit unit tests — `jit_lowers_an_arm_ending_in_a_tail_call`,
`jit_tier_compiles_a_hot_arm_then_runs_native` — fail, but they **already fail at the base
commit `b9e2173`** with `--features jit`: pre-existing, not from this work.) **Not merged to
`main`** — no benchmark win in the current safe state.
