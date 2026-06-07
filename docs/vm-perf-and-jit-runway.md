# VM performance & the JIT runway

> **Status (2026-06-07): round 1 SHIPPED (items 1–5); item 6 (defer-set shrink)
> done in round 2 — see the Round 2 note below.**
> Archived runs: baseline `docs/benchmarks/2026-06-06T10-45-03Z.md`, final
> `docs/benchmarks/2026-06-06T12-48-07Z.md` (same machine, commit `19d06b3` +
> the round's working tree). VM medians, 100 samples:
>
> | bench (VM) | before | after | Δ |
> |---|---|---|---|
> | fib 20 / 25 | 3.95 ms / 43.6 ms | 3.12 ms / ~34.0 ms¹ | **−21% / −22%** |
> | sum_tail 100k | 18.2 ms | 13.4 ms | **−26%** |
> | cons_build 100k | 104.5 ms | 60.9 ms | **−42%** |
> | sort_brood 1k / 5k | 33.8 ms / 282.7 ms | 25.7 ms / 247.2 ms | **−24% / −13%** |
> | spawn_fanout 1000 | 13.7 ms | 10.2 ms | **−25%** |
> | big_string_fanout 10k | 5.6 ms | 4.4 ms | **−23%** |
> | maps / pattern / mapcat | — | — | flat (±2%, no regressions) |
>
> ¹ the archived fib-25 median (41.6 ms) caught mid-run interference (fastest
> 33.8 ms, slowest 58.8 ms; fib-15/20 medians clean); a controlled 30-sample
> re-run gives 33.97 ms.
>
> Per-item attribution (directional 12-sample runs between landings):
> call-site ICs −13…−31% (the big one); global-read IC −1…−1.5%; wider prims
> cons −9% / sort −7…−10% (with a ~+2% code-layout tax on the pure-numeric
> pair); GC-pure rooting skip −5% (recouped the tax); `exec_value` split
> −3…−7%.
>
> **Round 2 (2026-06-07): item 6 (defer-set shrink) done — direct `letrec`
> self-recursion.** The `defseq` family (`map`/`filter`/`mapcat`/`remove`/`keep`)
> and hand-written local loops deferred wholesale to the tree-walker because a
> nested `fn` capturing its own in-progress `letrec` binder was ineligible. Now
> `MakeClosure` late-binds the closure to its own name in its captured env (the
> tree-walker's `letrec` model), and a **self-call optimization**
> (`Node::SelfCall` → `Step::SelfTail`, in-place frame reset) re-enters the arm
> with no resolve/dispatch/env-re-root. Load-robust Vm/Tw result (corrected with
> the `perf-stats` harness — see `docs/benchmarking.md`): for **RUNTIME-region
> closures** — the prelude `defseq` family — a real win, `(count (map inc (range
> n)))` is **~58–60% faster** on the VM (`self_tail` fires per element; it
> deferred *wholesale* before). **Top-level `letrec`/lambda literals defer by
> design** (LOCAL-region `fn_rest` can't be baked into a cached `Node` tree → they
> run on the tree-walker, parity); the self-call benefits promoted/prelude
> closures, not top-level one-shots. (An earlier **−30…−54%** figure here was a
> noisy read of a *top-level* `letrec` micro-bench that actually defers —
> `perf-stats` showed `self_tail`/`vm_apply` zero there. Corrected; the harness
> caught the bad measurement.) The remaining lever is the still-uncached
> per-element captured-fn call in a local closure (a frame-local IC — unsound with
> the per-site IC since a captured fn differs per instance). Mutual recursion
> still defers. Item 6's stretch tail (quasiquote-built bodies, unkeyable LOCAL
> closures) remains open but is low-value.
>
> This doc records the analysis behind the round: a set of VM-interpreter
> optimizations chosen so that each one is *also* a step toward a future JIT
> tier. Companion to `docs/bytecode-vm.md` (the closure-compiling VM design,
> ADR-076); this picks up where its §7 staged rollout ended.

## 1. VM vs JIT — the distinction that frames everything

Both are "compile, then execute." The difference is **what executes the
compiled artifact**:

- **The VM we have** (`crates/lisp/src/eval/compile.rs`): a form compiles once
  into a `Node` tree; *Rust code interprets that tree* — `exec_node` is a
  recursive `match` over `Node` variants. Every operation pays interpretive
  overhead: a branch on the node kind, `Box` pointer-chasing, `Step` enum
  wrapping, operand-stack pushes. `(+ a b)` in a hot loop costs on the order of
  50–100 machine instructions.
- **A JIT** emits **machine code** at runtime for the same compiled form. The
  dispatch disappears: `(+ a b)` becomes two slot loads, a tag check, a checked
  `add`, a store — ~8 instructions. The CPU runs *the program* instead of
  running *an interpreter that runs the program*.

Everything else — GC, scheduler, `Value`, semantics, hot reload — is identical
under both. A JIT removes exactly one cost: interpretive dispatch. It is the
next rung of a ladder we are already climbing:

| rung | what executes | typical gain over previous |
|---|---|---|
| tree-walker | the source AST, interpreted | baseline |
| closure VM (**today**) | a compiled `Node` tree, interpreted | ~1.6–2.3× (measured, ADR-076) |
| bytecode VM | flat bytecode, interpreted | ~1.5–2× (i-cache, no `Box` chase) |
| template JIT | machine code, one snippet per op | ~2–4× on hot numeric code |
| optimizing JIT | type-specialized native code | large, and large complexity |

## 2. The decision: one road, not two (ADR-096)

We will **not** start a JIT now, and we will **not** ignore it either. The
resolution is that the highest-value VM-interpreter work and the JIT
prerequisites are *mostly the same list*. So:

1. **Do the VM perf work now** — each item pays immediately, is days not
   months, and several simultaneously build machinery a JIT would consume.
2. **When two designs have equal VM merit, pick the JIT-aligned one** —
   a constant pool over in-place AST patching, one inline-cache mechanism
   instead of several ad-hoc ones, bytecode lowering once node dispatch
   dominates.
3. **Spend zero effort on actual codegen** (no Cranelift, no executable pages)
   until: the VM is bytecode-based, the editor exists and yields a real
   profile, and the profile says interpretive dispatch — not allocation, GC, or
   `env_get` — is the bottleneck.

This is the same trade ADR-076 made once already: preserve every
GC/TCO/preemption/hot-reload invariant and take the structural win, rather
than chase the last 2× early.

### Why a JIT is unusually feasible here (when its time comes)

- **Immutability (ADR-026)** removes write barriers, aliasing analysis, and
  shape invalidation entirely.
- **The lexically-addressed IR already exists** — params/lets are frame slots,
  tail calls are marked, captures are flattened. A JIT is a back-end, not a
  compiler project.
- **The deopt seam already exists** — per-arm "compiled or defer" is exactly
  the tiering boundary (tree-walker → VM → JIT).
- **The epoch-guard pattern** (`Node::Prim2`'s `guard` vs `Heap::global_epoch`)
  is the inline-cache invalidation a JIT compiles to a `cmp; jne slow_path`.
- **Frame slots live on `Heap::roots`**, so a tier-1 JIT can keep values in
  slots across safepoints and *sidestep stack maps entirely* — the single
  hardest part of JIT-ing under a moving collector.

### The hard parts a JIT would add (recorded so we don't forget)

1. **Moving GC vs native frames** — solved at tier 1 by keeping values in
   `Heap::roots` slots (registers only between safepoints); real register
   allocation across safepoints needs stack maps (tier 2+, months).
2. **Reduction-counted preemption + small coroutine stacks** — native code must
   poll `tick()` at loop back-edges and keep tail calls O(1)
   (`tail_calls_do_not_overflow` is load-bearing).
3. **Backend** — realistically Cranelift (passes the runtime-crate bar the way
   `boxcar` did, but heavy: compile time, binary size vs ADR-038 bundles).
4. **RUNTIME compaction (ADR-091)** — machine code can't have its embedded
   handles atomically rewritten; constants must go through an indirection
   table (see §4, item C).
5. **Code-cache lifecycle** — JIT'd code must be invalidated on `def` (epoch
   guard) and not leak executable pages across hot reloads.

## 3. The VM work list (this round)

Ordered by expected payoff. **Protocol: benchmark between every step** — see
§5. Each item lands only if the suite is green on both engines (differential
mode), green under the GC-stress gate, and the benchmarks show a real
improvement (or, for a pure-prep item, no regression).

| # | item | what | JIT dual-purpose? |
|---|---|---|---|
| 1 | **Call-site inline caches** | `Node::Call` caches the resolved callee + `Arc<CompiledArm>` under an epoch guard, skipping the per-call `env_get` chain walk + `vm_cache` hash lookup + arm selection. | Yes — the guarded-slot IC *is* what a JIT compiles. |
| 2 | **Global-read IC** | `Node::Global` caches the resolved value for true globals under the same guard (bindings are immutable; only `def` invalidates, via the epoch). | Yes — same mechanism, unified. |
| 3 | **Wider prim family** | `(Float,Float)` fast path in `prim_apply`; a `Prim1` node for `car`/`cdr`/`not`/`nil?`-class natives; `cons`. Every edge defers to the real native so semantics stay bit-identical. | Partly — enumerates the ops a template JIT inlines first. |
| 4 | **GC-pure rooting skip** | A compile-time "can't allocate or call" bit per node; pure operands skip the `push_root`/`truncate_roots` dance in `Prim2`/`Call`. | Yes — this analysis is exactly "where are the safepoints", which a JIT needs spelled out. |
| 5 | **`exec_value` / `exec_tail` split** | Value positions can never produce `Step::Tail`; a direct `-> Result<Value>` executor removes the `Step` wrap + `force` unwrap from every sub-expression. | Neutral (pure interpreter win). |
| 6 | **Defer-set shrink** (round 2, done for `letrec`) | Direct `letrec` self-recursion now compiles for RUNTIME-region closures (the prelude `defseq` family): `MakeClosure` self-binds + a self-call optimization (`Node::SelfCall`/`Step::SelfTail`). `(map inc (range n))` ~58–60% faster on the VM (deferred wholesale before). Top-level `letrec`/lambda literals defer by design (LOCAL region). Mutual recursion / quasiquote-built / unkeyable LOCAL bodies still defer (low-value tail). | Yes — smaller deopt surface; self-calls are exactly what a JIT specializes. |

Then, **later and separately gated**: bytecode lowering (the doc'd §2.4
internal change) once profiling shows node dispatch dominating — that is the
JIT on-ramp proper.

## 4. JIT-alignment rules adopted now (cheap while pre-alpha)

- **A. One IC mechanism.** Every guarded cache (Prim2's, the new call-site and
  global ICs) uses the same shape: `(AtomicU64 epoch-guard, cached payload)`,
  revalidated on `global_epoch` mismatch. No second invalidation scheme.
- **B. Never hard-bind a `ClosureId` without a guard** (ADR-076 R4, restated):
  an IC may cache a resolution only behind the epoch check.
- **C. Prefer indirection tables over in-place patching.** New compiled-code
  constants should be reachable through a per-arm table that compaction
  rewrites, rather than growing the `rewrite_node` walk. (Existing `ConstVal`
  atomics stay until the bytecode lowering replaces them wholesale.)
- **D. Safepoint discipline is explicit.** A safepoint can occur only at: a
  call into `dispatch`/a native, an allocation, or the `vm_apply` trampoline
  top. Any `Value` held across one must live in a `Heap::roots` slot. Item 4's
  purity analysis encodes this; new node kinds must declare their safepoint
  behaviour.
- **E. The `Value` representation question is open, on the clock.** A packed
  64-bit `Value` (NaN-box / tagged bits) would halve operand-stack traffic and
  is what JIT'd code wants in a register. It touches everything, which is
  exactly why pre-alpha is the cheapest it will ever be. Not part of this
  round; decide before 1.0. (Today `Value` is a 16-byte Rust enum.)

## 5. Benchmark protocol (every step, no exceptions)

- **Baseline**: one archived `scripts/bench.sh` run on the clean tree before
  any change (in `docs/benchmarks/`, stamped with commit + machine).
- **Between items**: `scripts/quickbench.sh` for a directional read (seconds,
  not archived). An item that doesn't move its target benchmark — or regresses
  an unrelated one — gets investigated or reverted, not shipped on vibes.
- **After each landed item**: the full gates —
  `make test` (both engines via the differential harness where applicable) and
  the GC-stress gate
  (`RUSTFLAGS="-C debug-assertions=on"` build with
  `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`) for anything touching rooting.
- **End of round**: a final archived `scripts/bench.sh` run; the before/after
  table goes into `docs/bytecode-vm.md`'s as-built section and the devlog.

## References

ADR-096 (this plan), ADR-076 (closure-compiling VM), ADR-069 (dispatch perf:
passthrough + inline cache), ADR-091 (RUNTIME compaction — why in-place
patching can't survive a JIT), ADR-026 (immutability), ADR-038 (bundles — the
Cranelift size concern). Key files: `crates/lisp/src/eval/compile.rs` (the
whole work list), `crates/lisp/src/core/heap.rs` (`global_epoch`, `vm_cache_*`,
`roots`), `docs/bytecode-vm.md` (§2.4 bytecode lowering, §7 staging).
