# Plan — the post-JIT single-threaded compute frontier

> **Status: planning (2026-06-14). No code yet.** The tier-1 JIT has shipped and the
> easy codegen-shaped wins are landed (geomean **19.5× → 13.5×** off the fastest runtime
> across the single-threaded suite). This note scopes what's *left* and — importantly —
> records that the remaining gaps are **data-structure-specific**, not the `Value`-width
> question (which `value-repr.md` already settled).

See also: `value-repr.md` (the `Value`-enum-width decision — **keep the 16-byte enum**,
§5), `jit-tier2.md` / `jit-float.md` / `jit-stage1.md` (the JIT as built),
`benchmarking.md`, the `brood-benchmarks` repo.

## 1. Where we are (what shipped)

Codegen-shaped JIT wins this round (all landed + benchmarked):

| fix | benchmark effect |
|---|---|
| bool literals into the JIT subset + `Value::Bool` truthiness mask | `primes` 351→56 ms, `nqueens` 933→523 ms |
| left-fold n-ary `+`/`*` into native 2-ary ops | `bintree` 1123→452 ms |
| top-level-lambda promotion (freeze an inline `(fn …)` body into RUNTIME) | `pipeline` 552→122 ms (~4.5×), `matmul` 542→241 ms (~2.2×) |
| lower `and`/`or` (zero-extend an `i8` comparison crossing a block boundary) | `mandelbrot` 1326→250 ms (**~5.3×**, the biggest single win) |

These were all "the JIT couldn't *express* this shape" gaps. **That well is now dry.**
A profiling sweep of every remaining weak row (2026-06-14) confirms the rest are
**not** codegen-expressibility gaps.

## 2. The `Value`-width question is NOT the lever (already settled)

`value-repr.md` §4 measured it directly: padding the operand slot 16→32 bytes made **zero**
difference on the compute loops (they're CPU/dispatch-bound and stay L1-resident). So a
single-word/NaN-boxed `Value` buys ~zero at tier-1; its only upside is tier-2
register-passing, deferred. **Nothing this round changes that** — do not reach for
NaN-boxing to close the rows below. (Tracked there; not re-opened here.)

## 3. The remaining gaps are data-structure-specific (measured 2026-06-14)

Profiled with `--features perf-stats` (`BROOD_PERF_STATS=1`) + `BROOD_JIT_DUMP_IR`.

### 3a. `matmul` (~39× — now the largest gap) — **flat vector storage / inline VectorRef**

- The hot `dot` loop **already runs native** (it lowers, `define_function` succeeds, it's
  dispatched native; the high `prim2_fallback` is the one-time matrix *construction*, not
  the loop). Confirmed: **not a deopt, not a missing-codegen path** — the old devlog note
  ("data-dependent deopt") was wrong.
- The cost is the **per-element `VectorRef`**. Microbenchmark: a 30 M-iteration loop with
  one `nth` per step costs 0.33 s vs 0.11 s without — **~7.3 ns per `VectorRef`**. For
  `matmul` N=175 (~16 M inner reads × the calls in `dot`) that's roughly **half** the
  241 ms.
- Why it's a call, not an inline read: `nth` lowers to `brood_rt_vector_ref` (a real call:
  marshal 6 words in, return a 24-byte `Value` via the out-pointer ABI, plus a
  **`boxcar::Vec` segment lookup** + bounds check). RUNTIME vectors are `boxcar` (segmented,
  append-only) so the index→(segment,offset) math can't be cheaply reproduced in CLIF.
- **The lever:** give vectors a *flat, contiguous* backing whose (data-ptr, len) the JIT can
  load once and index inline (`ptr + idx*stride`), at least for the in-bounds hot path —
  i.e. inline `VectorRef` instead of calling out. Hard parts: the `boxcar` segmentation
  (would need a flat slab for RUNTIME vectors, or a per-vector contiguous buffer), and the
  loop-invariant base would want hoisting (a template JIT does no LICM — `rowa` is invariant
  in `dot` but `b[k]` is not). What it can't beat without unboxing: .NET indexes a `long[]`
  with a single register-relative load + eliminated bounds check; Brood reads a 24-byte
  boxed `Value`. So this narrows but won't *close* matmul.
- Entry points: `eval/compile.rs` — `let vector_ref =` (the JIT helper, ~line 5265, currently
  emits the call), `chunk_in_jit_subset`/`resolve_prim` (`nth` → `PrimOp::VectorRef`),
  `jit/mod.rs::brood_rt_vector_ref` (the runtime helper); `core/heap.rs` `vector()` + the
  `CodeSlabs.vectors` boxcar (the storage to flatten).

### 3b. `bintree` (~27×) — **allocation / GC for short-lived pairs**

`jit_native=85` (barely tiers), `prim2_fallback=655 280` (first/rest/cons on pairs),
`alloc=366 825`. It builds and walks many small trees — allocation-bound. The lever is the
GC/allocator (cheaper short-lived pair allocation: a bump nursery already exists — tune it,
or a JIT cons fast path — but note `Cons` is deliberately out of the JIT subset after a
miscompile; see the `chunk_in_jit_subset` comment). Entry: `core/heap.rs` (allocation,
`gc_floor`, nursery), `jit-stage1.md`.

### 3c. `strings` / `pipeline` — **lazy sequence combinators**

`strings` (~19×) and `pipeline` (the eager part) materialize a full cons list per stage
(`(map number->string (range n))`) which the copying GC then relocates — `strings` is also
the memory outlier (~180 MB). The lever is a **lazy/streaming `map`/`filter`** (a
`Value::Range`-style reducible already exists — extend that model to the combinators) so a
pipeline fuses instead of building intermediate lists. Design-level; touches `std/prelude`
+ the sequence protocol. Entry: search the prelude for `map`/`filter`/`reduce`, and
`Value::Range` (the existing reducible).

### 3d. `wordcount` (~33×) — **persistent map build; likely accept**

`jit_native=0` (no JIT path), CHAMP-map build with structural sharing vs a mutable
`Dictionary`. This is algorithmic (immutable by design, ADR-026). A transient-map build
path exists (`5a7b8bb`); wiring `into`/`reduce`-into-a-map through it is the only realistic
lever short of abandoning persistence. Lowest priority — most inherent to Brood's identity.

## 4. Recommendation & priority

These are **foundational, multi-session bets with capped payoff** (Brood's boxed/immutable/
lightweight design means none will reach .NET/Node on raw numeric throughput). Brood's
actual standouts — **memory** (~14 MB base, lightest in `pfib` at ~16 MB), **concurrency**
(`http` 2nd of six, `pfib` ahead of Ruby/Python), **startup** (~28 ms) — are already strong
and are where the language's identity lives.

Priority if/when this is picked up:

1. **`matmul` flat-vector + inline `VectorRef`** — best measured leverage (~half of the
   largest gap), most self-contained, and the result generalizes to every indexed-array
   workload. Start here. Verify it beats the current ~7.3 ns/read microbenchmark before
   wiring the whole change.
2. **lazy combinators** (`strings`/`pipeline`) — also fixes the memory outlier; design-level
   but well-bounded by the existing `Value::Range` reducible.
3. **`bintree` allocation** — GC/nursery tuning; diffuse.
4. **`wordcount`** — accept, or transient-map `into`. Lowest.

NaN-boxing / `Value`-width is **not** on this list (§2).

## 5. How to pick it up

1. Re-baseline: `cd brood-benchmarks && python3 bench/harness.py --runs 5 --startup-runs 15`
   (needs `make install` of a `--features jit` binary first) — confirm the numbers above
   haven't drifted.
2. Profile the target with a `--features jit,perf-stats` debug build:
   `BROOD_PERF_STATS=1 BENCH_N=<small> ./target/debug/brood bench/brood/<t>.blsp` — read
   `jit_native` / `jit_deopt` / `prim2_fallback` / `alloc`. Dump lowered arms with
   `BROOD_JIT_DUMP_IR=1`.
3. For matmul specifically: the microbenchmark in §3a (a tight `nth` loop) isolates the
   `VectorRef` cost cleanly — use it to validate any helper/storage change before touching
   the benchmark.
4. **Guardrails (this area bites — three regressions this round came from JIT/compile
   changes):** run the full in-language suite under `--features jit` (the
   `format`-tiering-corruption canary lives there), keep per-benchmark JIT==tree-walker
   checksum parity, and run `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1` for any
   allocation/storage change. The `value.rs` accessor discipline (ADR-002) is what keeps a
   storage change containable.
