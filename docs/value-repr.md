# Plan — the `Value` representation decision (the JIT prerequisite)

> **Status: planning (2026-06-08). No code yet.** This is the open architectural
> prerequisite for the JIT (ADR-101 prerequisite 2; roadmap JIT gate (c)). The
> compute-workload profile (loop/pfib: 100% prim2-inline, ~100% IC hit, near-zero
> alloc — see `vm-perf-and-jit-runway.md`) confirms the JIT is justified for compute;
> the only thing left to settle before Stage 0 is how a `Value` is represented.

See also: ADR-101 (JIT architecture), `vm-perf-and-jit-runway.md` §4.E/§6,
ADR-002 (construction goes through `value.rs` helpers — what makes this containable),
ADR-026 (immutability), `core/value.rs` (the type itself).

## 1. Why this is a prerequisite

The JIT calling convention (ADR-101 §6.2) keeps GC-visible values in `Heap::roots`
at tier 1, so it *works* with either representation. But:

- A **single-word `Value`** is what JIT'd register code wants — it can hold an
  operand in a register across a safepoint-free segment instead of a 16-byte load/store
  pair, and it halves operand-stack (`Heap::roots`) traffic, which the compute profile
  shows is the per-iteration cost (every `Local`/`Const`/prim push-pops a slot).
- **Pre-alpha is the cheapest window.** `Value` is `Copy` and pattern-matched at
  *every* builtin and eval site; the later we change it, the more call sites move.
  Deciding now (even if we keep the enum) retires the unknown before Stage 0.

So the decision must be made — *what* we pick can be "keep the enum," but it must be a
deliberate choice, not a default.

## 2. Current state

`Value` is a **16-byte `#[derive(Copy)]` Rust enum** (`core/value.rs`), ~21 variants,
tag universe in `Tag` (18 tags). Construction already funnels through `value.rs`
helpers (`cons`/`list`/`sym`/`str_val`/…) per ADR-002 — the one fact that makes a
repr change *containable* rather than a scatter-edit.

Why 16 bytes: the widest variant is `Pid { node: Symbol (u32), id: u64 }` = 96 bits of
payload → 16 bytes with the discriminant. The variants that carry a **≥64-bit payload**
are the crux of any packing scheme:

| variant | payload | fits in a 48–51-bit NaN-box payload? |
|---|---|---|
| `Int(i64)` | full 64-bit | **no** (only small ints) |
| `Ref(u64)` | full 64-bit monotonic id | **no** |
| `Socket(u64)` | full 64-bit handle | **no** |
| `Pid { node, id }` | 32 + 64 = 96 bits | **no** (can't fit at all) |
| `BigInt`/`Str`/`Pair`/… | heap handle (index+gen) | yes (handles are already ≤32–48 bits) |
| `Float(f64)` | 64-bit | NaN-box stores these *natively* |

This table is the whole difficulty: NaN-boxing is designed for "floats + tagged
pointers," and Brood has four scalar variants that are genuinely 64-bit (or wider).

## 3. Options

### A. NaN-boxing (64-bit, float-favoring)
Floats stored natively; everything else encoded in the quiet-NaN space (~51 payload
bits, or ~48 if we reserve tag bits).
- **Pro:** single word; native floats; the canonical dynamic-language repr (JS engines, LuaJIT).
- **Con:** the four wide scalars don't fit. Forces: **small-int inlining** (i64 outside
  ~48 bits becomes a heap `BigInt` — moves the Int/BigInt boundary far below `i64::MAX`,
  a language-visible perf cliff), and **`Pid`/`Ref`/`Socket` become heap handles or
  interned ids** (today they're immediate scalars sent by value; boxing them adds alloc +
  GC surface + message-copy work to the hot pid/ref paths). Pointer-bits assume 48-bit
  canonical addresses (fine on current x86-64/AArch64, but an assumption).

### B. Keep the 16-byte enum
- **Pro:** zero churn; every variant stays immediate; simplest; tier-1 JIT (values in
  `Heap::roots`) works as-is.
- **Con:** 16-byte operand slots (2× the traffic the profile flags); a `Value` can't ride
  in a single register, so the JIT never gets values-in-registers (caps the ceiling at
  "native dispatch + native arithmetic, operands through memory"). Leaves the
  "decide before 1.0" debt unpaid.

### C. Low-bit / pointer tagging (8-byte, **not** NaN-box)
A single 64-bit word with a 3-bit low tag (pointers are ≥8-aligned), SMI-style small
ints in the high bits (V8/OCaml model), the wide scalars (`Ref`/`Socket`/`Pid`/big `Int`)
as **boxed handles** — but floats *also* boxed (or a 2-bit tag carving a float subtag).
- **Pro:** single word, register-friendly, doesn't depend on NaN semantics; clean small-int path.
- **Con:** floats lose their native slot (boxed or reduced precision) — bad for `mandelbrot`/`matmul`; same `Pid`/`Ref`/`Socket` boxing cost as A.

### D. Defer — tier-1 JIT on the enum, decide repr for tier-2
Build Stage 0–4 of the JIT with the **current enum** (values in `Heap::roots`, never in
registers), ship a working JIT, and revisit the repr only if tier-2 (values-in-registers)
is later justified by a profile.
- **Pro:** unblocks the JIT *now* with zero repr risk; the compute win (native dispatch +
  inlined arithmetic through memory operands) is most of the gain anyway.
- **Con:** ADR-101's "cheapest window" closes a bit more; a later repr change is costlier.

## 4. The real question to answer first (cheap measurement, do before choosing)

Before committing to a multi-month repr rewrite, **measure how much a single-word `Value`
would actually buy** over tier-1-in-roots:

1. Instrument the compute profile (loop/pfib) for **operand-stack slot traffic**
   (push/pop count) and the share of per-iteration time in 16-byte slot moves vs the
   arithmetic/dispatch. If slot traffic is a small fraction, options B/D dominate.
2. Estimate the **`Pid`/`Ref`/`Socket` boxing cost**: count their allocation/equality/
   message-copy frequency in the concurrency + dist benches. If non-trivial, NaN-boxing
   *regresses* the very BEAM-parity paths we just finished.
3. Confirm the **float paths** (mandelbrot/matmul) under each scheme — A keeps them native,
   C penalizes them.

The decision criterion: pick the repr that maximizes the JIT compute win **without
regressing** the immediate-scalar concurrency/dist paths or the float kernels.

### Measurement result (2026-06-08) — the operand-traffic upside is ~zero

Measured factor 1 directly with an isolated A/B: the operand-stack element (`heap.rs`
`roots: Vec<Value>`) was padded from 16 → 32 bytes (a clean revertable change, not
touching `Value` or any match site) to size how sensitive compute is to slot size.
Best-of-3, release, on the brood-benchmarks compute loops:

| benchmark | 16-byte slot (baseline) | 32-byte slot (padded) |
|---|--:|--:|
| `loop` (3M) | 0.22 s | 0.21 s |
| `fib(30)` | 0.27 s | 0.25 s |
| `pfib(28)` | 2.00 s | 2.01 s |

**Doubling the slot made no difference** (all within noise). Compute loops are
CPU/dispatch-bound and their operand stacks stay L1-resident regardless of element
size, so a *single-word* `Value` (halving the slot) would give ≈**zero** tier-1
speedup. The only real 1-word benefit is **tier-2 register-passing**, which is deferred.
Since the upside is ~zero, any NaN-box downside (factors 2–3: boxing the immediate
`Pid`/`Ref`/`Socket`/`i64` scalars, penalizing floats) makes it **net-negative now** —
no need to measure those precisely.

## 5. Decision: **D — keep the 16-byte enum; build the JIT on it** (2026-06-08)

The §4 measurement settles it: the operand-traffic upside of a single-word `Value` is
~zero for tier-1, so NaN-boxing is net-negative now (zero upside, real wide-scalar
downside). **Build the JIT Stage 0–1 on the current 16-byte enum** (values in
`Heap::roots`). Revisit the repr only if a future tier-2 (values-in-registers) profile —
taken *with a real JIT in hand* — shows register-passing is worth it; the NaN-box option
(A) stays on the table for that, behind the `value.rs` accessor migration (§6). This is
the original "lead with D" recommendation, now confirmed by data rather than assumed.

### Original framing (kept for context)

**Lead with D, keep A on the table.** Build the JIT Stage 0–1 on the current 16-byte enum
(values in `Heap::roots`) to capture the large compute win that needs no repr change, and
run the §4 measurement *with a real JIT in hand* (the honest way to value
values-in-registers). Only if that measurement shows register operands are worth it — and
that NaN-boxing doesn't regress `Pid`/`Ref`/`Socket`/big-int — promote to a NaN-box (A),
done as a contained migration behind the `value.rs` helpers. This unblocks the JIT
immediately, defers the irreversible repr churn until it's measurably justified, and keeps
the "decide before 1.0" debt explicitly tracked rather than forced prematurely.

If we instead decide the repr is too entangled to change post-JIT, do A **now** (pre-alpha
window) — but only after §4 confirms the wide-scalar boxing cost is acceptable.

## 6. Execution plan for whichever repr wins

A repr migration is mechanically large but contained:
1. **Gate behind `value.rs`.** Replace direct `match value { Value::X(..) => }` with
   accessor methods (`as_int`, `as_pair`, `tag`, constructors) so the enum/NaN-box swap is
   one file. (ADR-002 already routes *construction*; this extends it to *deconstruction*.)
2. **Differential safety net.** The tree-walker-vs-VM differential corpus + the full suite
   under `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1` guard every step; the §6 KI-1 bar guards the
   concurrency paths (Pid/Ref live there).
3. **Tag/printer/equality/message/type-lattice** all key off `tag()` — re-point them at the
   new accessor, not the enum shape.
4. **Incremental:** land the accessor refactor on the *current* enum first (no behavior
   change, fully testable), then flip the backing repr underneath it as a second step.

## 7. Decision record

When chosen, record as an ADR (extends ADR-101 prerequisite 2) and flip roadmap JIT gate
(c). Until then this doc is the plan; the JIT Stage 0 does not start until (c) is a
deliberate decision per §5.
