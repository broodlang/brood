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
