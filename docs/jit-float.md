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
`crates/lisp/src/eval/compile/` (all of the above). Correctness gates: `mandelbrot`/`matmul`
exact-bit vs `BROOD_VM=0`, no hang; differential `engines_agree_on_corpus` passes; 263 unit
tests pass. (Two jit unit tests — `jit_lowers_an_arm_ending_in_a_tail_call`,
`jit_tier_compiles_a_hot_arm_then_runs_native` — fail, but they **already fail at the base
commit `b9e2173`** with `--features jit`: pre-existing, not from this work.) **Not merged to
`main`** — no benchmark win in the current safe state.

---

# Float-across-calls — design (2026-07-12, `nbody` 43× vs Elixir)

> **Note:** the nbody gap this design targeted was subsequently fixed by other means (bodies
> list→vector + the vector-read/float-handle deopt fixes + fsqrt inline — devlog 2026-07-14/15);
> Layer B stays deprioritised.

The float codegen above (unboxed `f64` SSA, `as_f64`, `emit_float_arith`, `box_float`)
**landed on main** and put `mandelbrot` at Elixir parity — a *self-tail* pure-`f64` loop
runs native. The remaining big float gap is **`nbody` (43× Elixir; 8× slower than
CPython)**, and it is a *different* problem: nbody has **no** pure-float self-tail loop.

## Why nbody doesn't lower today (measured)

`bench/brood/nbody.blsp`: bodies are a `(list [x y z vx vy vz m] …)`; each step calls
`advance-body i` → `newvel b i 0 vx vy vz` (a tail loop over `j`) and reads state via
`(f b i k)` = `(nth (nth b i) k)`. So the hot arms are:

- **Mixed-type args.** `newvel b i j vx vy vz` mixes a **handle** (`b`), **ints** (`i j`),
  and **floats** (`vx vy vz`). `arm_scalar_kind` requires a *uniform* Int-only or
  Float-only arm (one register type for every arg/slot), so it bails → boxed path. Same
  for `momentum`, `kinetic`, `advance-body`.
- **Call-mediated.** Even boxed, these arms **call other arms/prims** (`f`, `nth`,
  `newvel`, `sqrt`). Those calls go through `brood_rt_call_slow` / `jit_dispatch_call`
  (the profile's 6%): the JIT boxes each arg to a `Value` word, hands off to the runtime
  which sets up a VM frame, and the callee re-reads boxed slots (`as_f64` tag-check if
  it too is JIT'd). **Every float crossing an arm boundary is boxed then re-unboxed.**
- The profitability gate (`jit_lower_arm`, added for the earlier nbody *regression*) then
  keeps the non-tail callers (`advance-body`, `offset`) on the VM entirely, because
  JIT'ing a boxed call-mediated float arm is *slower* than the VM. So nbody is ~fully
  interpreted, one `Value::Float` construct per arithmetic op (floats are inline, not
  heap — so the cost is call overhead + interpreter dispatch, not allocation; confirmed
  by profile: `exec_chunk` 29%, call machinery `dispatch`/`push_frame`/`vm_cache_arm`/
  `jit_dispatch_call` ≈ 35%).

**So the fix is a typed cross-arm calling convention**: a JIT'd arm calling another arm
must pass `f64` args in `f64` registers (and `i64`/handle in `i64`), not boxed `Value`s —
and the two arms must be **mixed-type** (per-slot typing), not uniform-scalar.

## Plan — two layers

### Layer A — mixed-type (per-slot) arms

Generalise the uniform `Scalar` to a **per-slot type vector** `SlotTy ∈ {Int, Float,
Handle}` (`Handle` = a boxed `Value` word, i64, read/passed verbatim — covers `b`, and any
value the arm only *threads* without arithmetic). Source of truth: the **tier-time slot
profile** already snapshotted in `jit_tier` (`tag(roots[base+i])`), extended to every slot
the body defines (a float result stored to a `let` slot marks it Float). Soundness stays
**per-read** (`as_f64`/`as_int` tag-check + deopt), so a mistyped profile can't miscompile
— it deopts. The worker/arm signature becomes per-slot-typed; self-recursion passes each
arg in its slot's register type. This alone lets `newvel`/`momentum`/`kinetic` keep floats
unboxed **within the arm and across self-recursion** — but not yet across *other* calls.

### Layer B — typed JIT→JIT call ABI (the actual nbody win)

Give each JIT'd arm a second **native entry** `brood_jit_native_<id>(a0..an: typed, …) ->
typed` alongside the boxed wrapper. When arm A (JIT'd) emits a `Call`/`SelfCall` to arm B
**and B has a known typed native entry with matching arg types**, emit a *direct native
call* passing unboxed typed args, skipping `brood_rt_call_slow` and all boxing. Requires:

1. **A compile-time callee-signature registry.** Keyed by `(callee arm id / global sym,
   argc)` → the arg/return `SlotTy` vector the native entry expects. Populated when an arm
   is JIT'd; a `Call` to a not-yet-registered / type-mismatched callee falls back to the
   boxed `brood_rt_call_slow` path (always correct). Late binding: a `def` that reshapes a
   callee invalidates via the existing `global_epoch` (a stale registry entry → the guard
   below deopts).
2. **Entry-guard the native entry.** Since a native call trusts the callee's arg types, the
   *boxed* wrapper (reached from the VM / a mismatched caller) must guard: the native entry
   assumes typed args; the boxed entry unboxes-with-tag-check then tail-calls the native
   entry. A caller that can't prove types uses the boxed entry.
3. **GC / deopt safety across the native call.** Today the runtime dispatch is the GC
   safepoint + deopt boundary. A direct native call must (a) keep no live boxed handle in a
   caller register across it that GC could move (float/int args are values, not handles —
   safe; a `Handle` arg is a boxed word that *could* be a moving LOCAL handle → must be
   spilled to the frame slot the runtime already roots, then reloaded, OR the callee must
   root it on entry), and (b) propagate deopt/overflow/kill sentinels up the native chain
   the way the self-worker's `ovf` ptr does.

## Milestones (each independently testable, each keeps main green)

1. **M1 — per-slot `SlotTy` + mixed-type self-worker.** Generalise `arm_scalar_kind` →
   `arm_slot_types() -> Option<Vec<SlotTy>>`; thread it through `jit_lower_i64_arm`
   (rename → `jit_lower_scalar_arm`). Gate: mandelbrot/loop/collatz/fib **exact-bit +
   no-regression**; a mixed self-recursive arm (a reduced `newvel` with `nth` stubbed to a
   passed float) lowers. No nbody win yet.
2. **M2 — typed native entry + registry, self-calls only.** Emit the native entry; route a
   JIT'd arm's **self**-`Call` through it (self type is trivially known). Gate: the
   self-recursive float arms stop boxing across recursion (verify via `BROOD_JIT_DUMP_IR`).
3. **M3 — cross-arm typed calls.** The registry + direct native call for `Call` to *another*
   JIT'd arm. This is where `advance-body → newvel`, `f → nth` unbox. Relax the
   profitability gate for arms whose calls are all typed-native. **Expected nbody win.**
4. **M4 — Handle-arg rooting + the GC gauntlet.** `b` (a list handle) crossing native
   calls: spill-and-reroot. `BROOD_GC_STRESS=1` + `BROOD_GC_VERIFY=1` on nbody must be
   clean; `breakagetests` 37/37.

## Correctness gauntlet (miscompile = the worst bug class here)

- Every scalar benchmark exact-bit vs `BROOD_VM=0` (`mandelbrot`, `nbody`, `matmul`,
  `collatz`, `fib`, `loop`, `sort`, `nqueens`), plus `differential engines_agree_on_corpus`.
- A `def` reshaping a callee mid-run deopts cleanly (epoch + entry guard).
- `BROOD_GC_STRESS=1` + `BROOD_GC_VERIFY=1` + `BROOD_JIT_VERIFY=1` clean on nbody.
- Full suite + `make suite` (`brood_suite_passes`) green; `breakagetests` 37/37.

**Risk/size:** M1–M2 are contained; **M3 is the deep ABI change** (a new native-call path
in the codegen + a cross-arm registry). Do it on a branch/worktree; do **not** merge a
half-ABI to main. The earlier float branch shows how a single wrong tag/coercion or a
control-flow edge silently miscompiles or hangs — bisect with `BROOD_JIT_DUMP_IR` per arm.

## Grounding correction (2026-07-12, measured with `BROOD_JIT_DUMP_IR`)

Verified against the real IR: **nbody's own arms (`newvel`, `advance-body`, `drive`,
`momentum`) do NOT lower at all** — the dump shows only the *prelude* helpers they call
(`nth-list`, `fold`, `range`, `seqview?`, …) getting JIT'd. So nbody is VM-interpreting its
own arms and calling into JIT'd prelude code with a **boxed** handoff each time.

This **re-orders the plan**: the scalar self-worker (M1/M2 above) is *orthogonal* to nbody —
its arms are call-mediated + `MakeVector`-returning (`[vx vy vz]`), which the pure-scalar
worker never handles. **The whole nbody win lives in the general path** (`jit_lower_arm_inner`):

1. **Get nbody's arms to lower first.** Find why `newvel`/`advance-body`/`drive` bail
   `chunk_in_jit_subset` / the profitability gate (nested `(nth (nth b i) k)`, `cond`→nested
   `if`, `MakeVector` return). Enabling them (they already have float-arith codegen from the
   landed work) is the prerequisite — without it there is nothing to pass floats *between*.
2. **Then the typed cross-arm ABI (Layer B / M3 above)** so those now-lowered arms pass
   `f64` args to each other and to prelude helpers unboxed. Relax the profitability gate for
   arms whose calls are all typed-native.

Net: **start at the general path, not the scalar worker.** Milestone re-order: M1' = *make
nbody's arms lower* (diagnose+enable the bail), M2' = typed native entry for the general
path, M3' = cross-arm typed calls + gate relaxation. Branch: `perf/jit-float-calls`
(worktree `../brood-jit-float`).
