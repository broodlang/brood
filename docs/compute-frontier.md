# Plan — the post-JIT single-threaded compute frontier

> **Status: in progress (2026-06-14). Lever 1 (matmul LICM) shipped** — see the devlog
> entry "JIT matmul LICM"; matmul 290→250 ms (~14%), the isolated invariant-local read
> ~7.8→~1.2 ns. Lever 2 (zero-copy messages) is next. The tier-1 JIT has shipped and the
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

### 3a. `matmul` (~45× — the largest gap; LICM shipped 2026-06-15) — **inline VectorRef via hoisted immutable base**

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
- **The lever (revised 2026-06-14 — immutability makes this tractable, not a flat-storage
  rewrite):** a vector built with `(into [] …)` is **immutable**, so its `(data-ptr, len)`
  never change. The JIT can therefore **hoist a loop-invariant vector's base out of the loop
  once** (one `brood_rt_vector_base`-style helper call returning the inner `&Vec<Value>`'s
  ptr+len) and **inline `ptr + idx*stride` reads** for the rest of the loop — turning the
  per-element call into a ~1 ns load. The usual blocker for hoisting a load (alias analysis:
  proving no write invalidates it) **does not apply** — immutability guarantees no write
  exists — so even this *template* JIT can do the LICM **soundly**. In `dot`, `rowa` is
  loop-invariant (hoistable → inline); `(nth b k)` / `(nth (nth b k) j)` vary with `k` (still
  a per-`k` base fetch, since `boxcar`'s segmentation resists a pure-CLIF arbitrary-index
  read). What it still can't beat without unboxing: .NET reads a register `long`, Brood a
  24-byte boxed `Value` — so this **narrows substantially** but doesn't fully close matmul.
  See §6 for why this is sound.
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

A second, immutability-enabled lever for the memory side (and for `spawn`/`pfib`'s
message cost): **zero-copy message passing.** Today `to_message` *deep-copies* a value
across a process boundary because LOCAL heaps are isolated. But an **immutable** value can
be **shared by handle** (an `Arc` bump, no copy) once it lives in a shared region — exactly
what `Message::StrShared` already does for large strings. Extending that to whole immutable
structures (lists/vectors/maps) would cut both the copy cost and the peak RSS. Sound *only*
because the value can't be mutated out from under a sharer. Entry: `process/message.rs`
(`to_message`/`StrShared`), `core/heap.rs` (the shared RUNTIME region + `promote`). See §6.

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

1. **`matmul` — hoist the immutable vector base + inline `VectorRef`** (§3a, §6) —
   **SHIPPED 2026-06-15** (see the "JIT matmul LICM" devlog entry). The hoist inlined the
   one *invariant-local* read (`(nth rowa k)`): isolated read ~7.8 → ~1.2 ns, `matmul`
   compute ~241 → ~212 ms. The residual two reads are a **global** (`b`, parity-unsound to
   hoist — a `def` rebind would diverge from the VM's late binding) and the **per-`k` row**
   (varies), so the gap stays the suite's largest (~45×, noise-sensitive denominator).
2. **zero-copy message passing** (§3c, §6) — share immutable structures by handle instead of
   deep-copying across processes; attacks the `strings` ~180 MB outlier and `spawn`/`pfib`
   message cost. Also opens **lazy combinators** as the eager-list fix.
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

## 6. Immutability shortcuts (why these gaps are more tractable than they look)

Brood is immutable (ADR-026), and that's not just a semantics choice — it **removes the
analysis that makes these optimizations hard in a mutable language**, so several "capped /
foundational" rows above have a sound, contained path:

- **Loop-invariant hoisting is sound with no alias analysis.** Hoisting a load out of a
  loop normally requires proving no write through any aliasing pointer can invalidate it —
  the expensive part of an optimizing compiler. In Brood *no such write can exist*, so the
  JIT can hoist an immutable vector's `(ptr, len)` out of `dot` and inline the element reads
  with zero alias analysis. This is the `matmul` lever (§3a, priority 1) and generalizes to
  every indexed-array loop over an immutable vector.
- **Zero-copy sharing across processes.** An immutable value can be shared by `Arc` handle
  instead of deep-copied (already done for big strings via `StrShared`); the copy is *only*
  needed because LOCAL heaps are isolated, not because the value could change. Extending it
  to whole immutable structures cuts message-copy cost and peak RSS (§3c, priority 2).
- **Hash-consing + O(1) equality.** Immutable values can be interned/deduplicated, making
  `=` on shared structure a pointer compare instead of a structural walk.
- **CSE / memoization / free reordering.** Referential transparency lets the compiler
  common-subexpression-eliminate repeated pure reads (`(nth v i)` with immutable `v`/`i`),
  memoize pure functions, and reorder without a happens-before worry.
- **No write barriers.** The frozen RUNTIME region needs none (already banked); more
  generally, immutability is why the tracing collector and cross-process sharing stay simple.

The throughline: where I earlier called a row "representation-bound" or "foundational with
capped payoff," immutability often supplies a *contained* path (hoist-and-inline, share-by-
handle) a mutable language would need a full optimizing pass to justify. The hard residual
is the boxed 24-byte `Value` itself (§2) — which immutability does *not* fix.
