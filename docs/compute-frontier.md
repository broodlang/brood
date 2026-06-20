# Plan — the post-JIT single-threaded compute frontier

> ## ⏯ RESUME HERE (2026-06-20) — current perf state + next lever (inline car/cdr/vector_ref)
>
> The newest work is the **JIT call-dispatch + loop-overhead** round (full play-by-play in
> `docs/devlog.md`, entries 2026-06-18/19). Status:
>
> - **SHIPPED + default-on:** the **in-IR call fast-link** (Technique A increment 1 — a JIT'd
>   non-tail free-global call epoch-guards a flat `#[repr(C)]` mirror of the call IC in IR and
>   calls `brood_rt_fast_frame`, skipping the IC probe + `RefCell` borrow; **fib ~20%**), gated by
>   `BROOD_JIT_ICALL` (now **default-on**, `BROOD_NO_JIT_ICALL=1` opts out). Plus two back-edge FFI
>   eliminations on self-tail loops: raw-load the global epoch (`brood_rt_global_epoch_ptr`) and
>   skip the `brood_rt_tick` preemption poll in non-capture mode (`brood_rt_in_capture`, read once at
>   entry — capture-mode path unchanged). **`loop` 0.14→0.09 s (~36%).** All gated: jit.rs 28/28,
>   differential, nest 2161, preemption/reductions/work-stealing, GC-stress+verify.
> - **NO-GO — do NOT re-attempt:** Technique A **increment 2** (full in-IR frame setup —
>   `#[repr(C)] RootStack` + in-IR `len`/nil-fill/depth/`call_indirect`). Implemented + correct but
>   ~5% SLOWER than the `brood_rt_fast_frame` FFI. **The FFI boundary is not the bottleneck** — LLVM
>   compiles the frame work better than hand-emitted Cranelift IR. Reverted. The dispatch lever is
>   mined out at increment 1.
> - **Standings (full 7-language `brood-benchmarks` run, single-thread aggregate compute vs the
>   fastest):** .NET 1.0× · Node 2.7× · Elixir 3.5× · **Brood 6.0× (4th of 7)** · Ruby 11.9× ·
>   Clojure 18.2× · Python 27.3×. Brood wins `strings` + `http`; ~18 MB base RSS; ~26 ms startup.
> - **SHIPPED 2026-06-19 — `map-int-add` + JIT GC safepoint:** `wordcount` 810→**470 ms** (~42%).
>   `(map-int-add m k delta)` fuses `(assoc m k (+ (get m k 0) delta))` into one CHAMP trie walk.
>   Added GC safepoint in `jit_dispatch_call`'s slow-path `Ok(v)` arm — roots `v` before
>   `heap.collect`, fixing the 1770 MB RSS regression that plagued the JIT path for native callees.
>   wordcount gap: ~31× → ~13× off the fastest.
> - **SHIPPED 2026-06-19 — `nil?`/`pair?`/`empty?` as native builtins + PrimOp1::IsNil/IsPair:**
>   bintree 383→230 ms (−40%), nqueens 504→320 ms (−36%). These predicates were Brood closures;
>   every call pushed a BcFrame (~100–150 ns). As native builtins `dispatch()` returns `Step::Done`
>   inline — no BcFrame. `nil?`/`pair?` also compile to `Prim1::IsNil`/`IsPair` (single tag-check),
>   eliminating all dispatch overhead for compiled arms. `chunk_walks_structure` updated to only gate
>   on `First`/`Rest` (heap deref), not `IsNil`/`IsPair` (tag-only). **7.7× → 7.0×** overall.
> - **SHIPPED 2026-06-19 — lift `chunk_walks_structure` gate + fix Prim2SlotInt VectorRef:**
>   bintree 241→**116 ms** (−52%, 2.3× speedup). The gate was correct pre-fast-link (JIT `check`'s
>   recursive calls cost same as VM's BcFrame then), but now fast-link makes JIT→JIT calls ~35–40 ns
>   vs ~150 ns BcFrame — so two-call structure-walking arms gain. Also fixed: `Prim2SlotInt { VectorRef }`
>   (constant-index `nth`) was bailing with `return None`; now materialises the integer index as a
>   Value word-triple and calls `vector_ref`. Deleted `chunk_walks_structure` (dead code). **7.0× → 6.1×**
>   (sum aggregate). nqueens flat (safe? was already JIT-compiled, no VectorRef).
> - **SHIPPED 2026-06-19 — `PrimOp1::IsEmpty`:** nqueens 321→**166 ms** (−48%, 12.5× → 7.4× behind
>   .NET). `empty?` was a native builtin (no BcFrame), but JIT arms still emitted `brood_rt_call_slow`
>   (~150 ns/call). `safe?` calls `empty?` once per list iteration → O(n²) FFI calls in the inner
>   loop. `IsEmpty` emits: read tag byte; `is_nil = (tag == 0)`; `is_pair = (tag == TAG_PAIR)`;
>   `brif(is_nil|is_pair, cont, deopt)` — deopt for Vec/Str/Map (need heap length); push
>   `Op::Int(is_nil)`. Also VM inline paths (single-eval + bytecode-compiled). **6.1× → 6.0×**
>   aggregate (nqueens is 1 of 15 compute benchmarks; geomean barely moves).
> - **SHIPPED 2026-06-19 — register-carry for loop-carried Int params:** loop 60→**38 ms** (−37%),
>   collatz 359→**320 ms** (−11%). Pure-arithmetic self-tail loops carried all loop state through
>   `roots` slots — every read of a loop-carried integer emitted a tag-check + 2 memory loads.
>   Fix: declare Cranelift `Variable`s for slots `0..carry_argc` (phi-node SSA), `def_var` once at
>   entry and at each SelfCall back-edge, `use_var` in `load_slot_int`. Zero memory ops, zero
>   branches per carry slot. Eligibility: `int_carry_eligible` (SelfCall, no non-tail Calls, no
>   Cons/MakeVector/First/Rest) + all carry slots profiled as `TAG_INT` (critical — `!= TAG_FLOAT`
>   was a latent bug that would deopt vector-param functions on every call). Aggregate: **6.0×
>   (unchanged)** — dominated by wordcount/fib; per-benchmark improvements are real.
> - **SHIPPED 2026-06-20 — float register-carry + F64 SSA value cache:** mandelbrot 224→**204 ms**
>   (−9%), 3rd of 7. Float carry extends `carry_vars` to `Vec<(Variable, is_float)>` — slots
>   profiled TAG_FLOAT get an F64 Cranelift Variable (entry: tag-check + bitcast i64→f64; back-edge:
>   `def_var` with new F64; reads: `use_var` — no memory ops). F64 SSA cache
>   (`slot_f64_cache: RefCell<Vec<Option<Value>>>`) covers let-bound floats not in carry params:
>   `store_op(Op::Float(v))` stashes `v`; `as_f64(Op::Slot(k))` returns it directly. Eliminated
>   4 full tag-check+load+bitcast sequences per inner iteration for `nx²`/`ny²`. Key safety note:
>   `slot_float[k]` is NOT safe to skip tag-checks (single-pass, cross-branch contamination caused
>   a real test failure); only the cache (populated on the actual store path) is safe.
>   Aggregate: **6.0× (unchanged)** — mandelbrot is one of 15 compute rows.
> - **NEXT lever:** inline `first`/`rest`/`vector_ref` pointer arithmetic in the JIT (eliminates
>   `brood_rt_car`/`cdr`/`vector_ref` FFI for JIT-compiled structure walkers). Technique B (true
>   inlining) is a longer horizon.
> - **Build/bench discipline:** perf bins via `cargo build --release --features jit --bin brood`
>   (NEVER `-p brood` — stale-lib trap); `make install` before benchmarking (`cp target/release/brood
>   ~/.local/bin/brood` — the harness runs the *installed* `brood`, not `target/`); GC-debug build
>   = `RUSTFLAGS="-C debug-assertions=on"`.
>
> ---
>
> **Status: in progress (2026-06-15). Lever 1 (matmul LICM) shipped — local AND global
> hoist** — see the devlog entries "JIT matmul LICM" + "the global lever". Both invariant
> `nth`s inlined (the local `rowa`; the global `b` via a back-edge `global_epoch` guard):
> matmul **~241 → ~171 ms compute / ~30× gap** (was ~45×), now beating both interpreters;
> isolated invariant read ~7.8→~1.2 ns. Lever 2 (zero-copy messages) is next. The JIT and
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

### 3a. `matmul` (~30× — the largest gap; LICM local+global shipped 2026-06-15) — **inline VectorRef via hoisted immutable base**

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

### 3c. `strings` / `pipeline` — **lazy sequence combinators** — *shipped (pipeline)*

`strings` (~19×) and `pipeline` (the eager part) materialize a full cons list per stage
(`(map number->string (range n))`) which the copying GC then relocates — `strings` is also
the memory outlier (~180 MB). The lever is a **lazy/streaming, fusing pipeline** so a
chain folds instead of building intermediate lists.

**Shipped** (ADR lazy-seq-view): a `Value::SeqView(VecId)` kernel kind mirroring
`Value::Range` (a distinct tag over the vector slab, backing `[source xform]`, `tag = Pair`).
`fold` fuses over it — `(fold (xform rf) init source)` — so the pipeline walks the source
once with no intermediate lists; `seq`/`first`/`count`/… realise on demand.

**Design choice — fusion is opt-in, `map`/`filter` stay eager.** Making `map`/`filter`
lazy *by default* breaks Brood's entrenched "iterate for side effects" idiom — the module
loader (`(map require-one …)`) and the test runner (`(map run-test …)`) rely on eager
evaluation, and a lazy view silently drops those effects (immutability covers *data*, not
*I/O*). So the eager combinators are unchanged and the fusing views are explicit:
`lmap`/`lfilter`/`lkeep`/`lremove` and the general `eduction` (compose transducers over a
source). Measured `pipeline` (n = 1e6): eager `(->> … filter map (reduce +))` ≈ 2.0 s / 173 MB
→ fused `(reduce + 0 (eduction (xfilter …) (xmap …) (range n)))` ≈ 0.63 s / 13 MB
(~3.3× faster, ~13× less memory).

**`strings` still open.** `join` realises a view before the native `%string-join`
(`seq_items` can't run a transducer), so `(join "," (lmap number->string (range n)))` still
materialises the parts list — no win yet. Full fusion needs a **string-builder reducer**
(a transient/mutable buffer the transducer appends into, O(n)); deferred as a follow-up.
Entry: `%string-join` in `builtins.rs`, and the transient machinery (`%map-into`).

A second, immutability-enabled lever for the memory side (and for `spawn`/`pfib`'s
message cost): **zero-copy message passing.** Today `to_message` *deep-copies* a value
across a process boundary because LOCAL heaps are isolated. But an **immutable** value can
be **shared by handle** (an `Arc` bump, no copy) once it lives in a shared region — exactly
what `Message::StrShared` already does for large strings. Extending that to whole immutable
structures (lists/vectors/maps) would cut both the copy cost and the peak RSS. Sound *only*
because the value can't be mutated out from under a sharer. Entry: `process/message.rs`
(`to_message`/`StrShared`), `core/heap.rs` (the shared RUNTIME region + `promote`). See §6.

### 3d. `wordcount` (~13×) — **persistent map build; `map-int-add` shipped**

**SHIPPED 2026-06-19:** `map-int-add` (single-pass CHAMP fused get+add+assoc) + JIT GC
safepoint in `jit_dispatch_call`. wordcount 810 → **422 ms** compute; gap vs fastest
(Node ~33ms) **~31× → ~13×**; gap vs Elixir **4.5× → 2.5×**.

Residual gap is algorithmic: CHAMP path-copy allocates O(log₁₆ N) nodes per update vs a
mutable `Dictionary`. A transient-map build path exists (`5a7b8bb`); wiring
`into`/`reduce`-into-a-map through it is the only realistic remaining lever short of
abandoning persistence. Lowest priority — most inherent to Brood's identity.

## 4. Recommendation & priority

These are **foundational, multi-session bets with capped payoff** (Brood's boxed/immutable/
lightweight design means none will reach .NET/Node on raw numeric throughput). Brood's
actual standouts — **memory** (~14 MB base, lightest in `pfib` at ~16 MB), **concurrency**
(`http` 2nd of six, `pfib` ahead of Ruby/Python), **startup** (~28 ms) — are already strong
and are where the language's identity lives.

Priority if/when this is picked up:

1. **`matmul` — hoist the immutable vector base + inline `VectorRef`** (§3a, §6) —
   **SHIPPED 2026-06-15, local AND global** (see the "JIT matmul LICM" + "the global lever"
   devlog entries). Inlined the invariant-local read (`(nth rowa k)`) *and* the global
   (`(nth b k)`) — the global is hoisted with a back-edge `global_epoch` guard that deopts
   on a concurrent rebind, so it stays bit-identical to the VM's late binding (the earlier
   "parity-unsound" worry is solved by the guard). Isolated read ~7.8 → ~1.2 ns; `matmul`
   compute ~241 → ~171 ms, now beating both interpreters. The one residual read is the
   **per-`k` row** (varies — not hoistable), so the gap stays the suite's largest (~30×,
   noise-sensitive denominator) — bounded ultimately by the boxed 24-byte `Value`.
2. **zero-copy message passing** (§3c, §6) — share immutable structures by handle instead of
   deep-copying across processes; attacks the `strings` ~180 MB outlier and `spawn`/`pfib`
   message cost. Also opens **lazy combinators** as the eager-list fix.
3. **`bintree` allocation** — GC/nursery tuning; diffuse.
4. **`wordcount`** — **SHIPPED 2026-06-19** (`map-int-add` + JIT GC safepoint, 810→422ms,
   gap ~31×→~13×). Residual: transient-map `into` for the final 13× → ~4× if wanted.

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
