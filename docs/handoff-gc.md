# GC handoff — what's left after collect-at-any-depth

Resume point for garbage-collector work, after the 2026-05-30 session that landed
collect-at-any-depth (ADR-061), rooted the six re-entrant sites it exposed, moved
`macroexpand` to Brood, and adopted the single-shot-primitive rule (ADR-064).

## State: correctness is done

- **Collect at any eval depth** (ADR-061): the copying collector fires at any
  depth, not just the outermost. The evaluator keeps in-flight LOCAL transients on
  an operand stack (`Heap::roots` + `Heap::env_roots`, relocated in `arena_flip`).
- **All six re-entrant rooting sites fixed** (`reload_defs`, `receive_match`,
  `check_file`, `try_catch`, `quasiquote`, `macroexpand`).
- **`macroexpand` → Brood** (ADR-064): a prelude `defn` over the single-shot
  `macroexpand-1` builtin.
- **I/O subsystems audited safe** (`net`/`tls`/`file`/`io_source`): zero eval
  re-entry → single-shot → GC-safe by construction.
- **Verified:** full `cargo test` green under
  `RUSTFLAGS="-C debug-assertions=on" BROOD_GC_VERIFY=1 BROOD_GC_STRESS=1`.

Tooling for any GC change (see CLAUDE.md "Debug tooling"): `BROOD_GC_VERIFY=1`
(heap verifier — catches a *stored* stale handle, `Heap::verify_local_graph`),
`BROOD_GC_STRESS=1` (collect every safepoint), the per-deref epoch tripwire
(`check_epoch`, debug-assertions), and `.brood_crash_dump` (panic hook).

## Remaining work (priority order)

### 1. Tune the operand-stack rooting for perf
Every call now pays a few `Vec` push / re-read / truncate ops (`eval_arguments`,
`bind_sequential`, the literal/`if`/`let` sites in `eval/mod.rs`). **Fix if the
benchmark regression warrants it:** a **region check before rooting** — RUNTIME and
PRELUDE handles never move, so the hot path (running compiled/promoted code, where
forms are RUNTIME) can skip the push entirely and pay ~nothing. Add a
`v.region() == LOCAL` (and `env.region() == LOCAL`) guard before each
`push_root`/`push_env_root`. See the benchmark comparison below for whether this is
needed and where.

### 2. `promote` has no cycle guard — a genuine latent crash
`Heap::promote_env` ↔ `Heap::promote_closure` (`core/heap.rs` ~1700–1745) recurse
with **no visited/forwarding table**, so promoting a *cyclic* local-env↔closure
graph stack-overflows. Reachable: `(def f (let (g (fn () g)) g))` — `g` captures
the scope that binds `g`, and `def` promotes it → infinite recursion → SIGSEGV.
(Normal recursive `defn`s capture the *global* env, which short-circuits in
`promote_env`, so they're fine — this only bites a closure capturing a
self-referential **local** scope.) **Fix:** give `promote` a `fwd`-style forwarding
table keyed by LOCAL closure/env index, exactly as `arena_flip`/`flush_value`
already do (`core/heap.rs`), so a revisit resolves to the in-progress placeholder.
This is an actual bug, independent of the collector but adjacent — do it soon.

### 3. Surface reduction (same single-shot rule, ADR-064/ADR-006)
- **`quasiquote` → Brood macro** over `cons`/`list`/`eval`. Highest structural
  value but **bootstrap surgery**: the very first prelude definition
  (`(defmacro defn …)`) already uses backtick, and there are 127 quasiquote forms,
  so the expander must be written in *raw* Brood (no backtick) before `defn`, and
  the compile pass (`eval/macros.rs` `macroexpand_all`) must **expand** `quasiquote`
  rather than treat it as opaque data — and the `Quasiquote` special form in
  `eval/mod.rs` removed. A bug = prelude won't load. Do it as its own session with
  the suite + `.brood_crash_dump` as the net and a clean revert path.
- **`reload-defs` → Brood**: needs `note-definition` and a read-file-forms
  primitive exposed (it currently records def-sites for goto-definition in Rust).

### 4. Tighten the ADR-043 memory caps
`TEST_DEFAULT_HARD/SOFT` (`core/alloc.rs`) are 5 GiB / 4 GiB — sized for the old
~18 GB suite peak. The suite now peaks ~240 MB under collection, so these can drop
substantially (they're a host-survival backstop, not a working-set budget).

### 5. Deferred optimizations
- **Generational collection:** today `arena_flip` is a full semi-space copy each
  time, recopying long-lived data. A young/old split would cut copying of stable
  data.
- **`Rc` → `gc-arena` (ADR-002):** effectively superseded by the hand-rolled
  copying collector now in place. Confirm and close it in the roadmap rather than
  carry it as pending.

## Benchmark: collect-at-any-depth overhead

Baseline = commit `243debb` (pre-operand-stack; archive
`docs/benchmarks/2026-05-29T21-19-57Z.md`). Fresh run = commit `317190b`
(post-ADR-061/064; archive `docs/benchmarks/2026-05-30T00-26-06Z.md`). Same host
(i7-14700HX), `bench` profile, median of 100 samples.

| bench | baseline | now | ratio |
|---|---|---|---|
| `eval/fib/20` (non-tail calls) | 11.12 ms | 21.12 ms | **1.90×** |
| `eval/fib/25` | 157 ms | 232.9 ms | 1.48× |
| `eval/sum_tail/100000` (tail loop) | 58.83 ms | 115 ms | **1.95×** |
| `eval/cons_build/100000` | 156.1 ms | 293 ms | 1.88× |
| `eval/sort_brood/5000` | 456.6 ms | 790.3 ms | 1.73× |
| `library/maps/build_and_get/1000` | 6.31 ms | 12.22 ms | 1.94× |
| `library/maps/frequencies/10000` | 65.71 ms | 111.8 ms | 1.70× |
| `library/sequence/mapcat/10000` | 100.5 ms | 184.1 ms | 1.83× |
| `library/sequence/pipeline/10000` | 55.53 ms | 94.36 ms | 1.70× |
| `library/pattern/dispatch/10000` | 43.83 ms | 77.57 ms | 1.77× |
| `library/sequence/sort/10000` | 45.43 ms | 77.88 ms | 1.71× |
| `library/compile/macroexpand` | 1.145 ms | 1.66 ms | 1.45× |
| `eval/interp_new` | 4.51 µs | 3.20 µs | 0.71× (init; unrelated, noise/win) |
| `eval/parse_prelude` | 842.8 µs | 821.3 µs | ~1.0× (no eval) |

**Verdict: eval-bound work is ~1.5–2.0× slower; init/parse are flat.** Both the
baseline and now collect at the depth-1 safepoint, so the delta is almost entirely
the **per-call operand-stack rooting** (`eval_arguments` pushes `call_form`/
`callee`/spine + every evaluated arg, re-reads via `root_at`, then truncates) plus
slightly more frequent collection on allocating benches. `macroexpand`'s 1.45× is
the Brood-loop-vs-Rust-loop part.

**This makes remaining-item #1 (region-check before rooting) high priority.** What
it recovers vs. what's inherent:
- *Recoverable:* the source-form pushes — `call_form`, the cons-spine cursor, body
  forms, and a global `callee` are RUNTIME/PRELUDE handles (never move), so a
  `region() == LOCAL` guard skips them entirely. That's most of the *fixed* per-call
  cost.
- *Inherent:* each **evaluated argument** and freshly-created **scope** is a LOCAL
  transient that genuinely must be rooted to survive a collection mid-call — can't
  be skipped while we collect at depth. So the region check will cut the regression
  substantially but not to zero; an arg-heavy call still pays per-arg rooting.
- *Further options if needed:* a leaner operand-stack representation (avoid
  per-call `Vec` grow/shrink bookkeeping), or only arm rooting when `gc_enabled`.

## References
ADR-061 (collect at any depth), ADR-064 (single-shot primitives), ADR-054
(use-after-GC tripwire), ADR-055 (Stage B GC), ADR-043 (memory caps), ADR-002
(`gc-arena` migration), `docs/memory-model.md`, `docs/memory-review.md`,
the 2026-05-30 `docs/devlog.md` entries.
