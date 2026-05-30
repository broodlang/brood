# GC handoff — what's left after collect-at-any-depth

Resume point for garbage-collector work, after the 2026-05-30 session that landed
collect-at-any-depth (ADR-061), rooted the six re-entrant sites it exposed, moved
`macroexpand` to Brood, and adopted the single-shot-primitive rule (ADR-064).

## State: correctness is done — and no known GC crashes remain

- **Collect at any eval depth** (ADR-061): the copying collector fires at any
  depth, not just the outermost. The evaluator keeps in-flight LOCAL transients on
  an operand stack (`Heap::roots` + `Heap::env_roots`, relocated in `arena_flip`).
- **All six re-entrant rooting sites fixed** (`reload_defs`, `receive_match`,
  `check_file`, `try_catch`, `quasiquote`, `macroexpand`).
- **`macroexpand` → Brood** (ADR-064): a prelude `defn` over the single-shot
  `macroexpand-1` builtin.
- **I/O subsystems audited safe** (`net`/`tls`/`file`/`io_source`): zero eval
  re-entry → single-shot → GC-safe by construction.
- **Region-check rooting** (item #1, 2026-05-30): per-call rooting skips immovable
  handles → recovered ~10–14% of the collect-at-any-depth overhead.
- **`promote` cycle guard** (item #2, 2026-05-30): the last known GC-adjacent
  crash — a cyclic local closure promoted via `def`/`spawn` SIGSEGV'd — is fixed
  (forwarding table + `OnceLock` reserve-then-fill). **No known GC crashes remain.**
- **Verified:** full `cargo test` green under
  `RUSTFLAGS="-C debug-assertions=on" BROOD_GC_VERIFY=1 BROOD_GC_STRESS=1`.

Tooling for any GC change (see CLAUDE.md "Debug tooling"): `BROOD_GC_VERIFY=1`
(heap verifier — catches a *stored* stale handle, `Heap::verify_local_graph`),
`BROOD_GC_STRESS=1` (collect every safepoint), the per-deref epoch tripwire
(`check_epoch`, debug-assertions), and `.brood_crash_dump` (panic hook).

## Remaining work (priority order)

### 1. Tune the operand-stack rooting for perf — ✅ DONE (2026-05-30)
A **region check before rooting**, shipped as a token-based rooting API in
`core/heap.rs` (`is_movable`, `Root`/`EnvRoot`, `root`/`read_root`/`advance_root`/
`root_env`/`read_root_env`). `root(v)` takes an operand-stack slot **only when `v`
is movable** (a LOCAL heap object); immovable values (atoms, `PRELUDE`/`RUNTIME`
handles — the hot path running promoted code) stay inline and pay nothing. All hot
per-call sites in `eval/mod.rs` were converted off the positional
`push_root`/`root_at` protocol (`eval_arguments`, `apply_closure`, `bind_params`,
`bind_sequential`, `tail_of_cons`, the call-dispatch/`if`/`def`/macro/multi-body
sites, vector/map literals). **Recovered ~10–14% across eval-bound benches**;
overhead vs the pre-operand-stack baseline dropped from ~1.7–1.95× to ~1.5–1.71×
(about a third of the regression — the residue is the inherent per-arg/per-scope
LOCAL rooting that can't be skipped while collecting at depth). See the
2026-05-30 devlog entry and archive `benchmarks/2026-05-30T00-54-34Z.md`. Verified
green under `BROOD_GC_VERIFY=1 BROOD_GC_STRESS=1` + debug-assertions.

*Further headroom if ever needed (not pursued):* a leaner operand-stack
representation avoiding the per-call `SmallVec<Root>` materialize, or arming
rooting only when `gc_enabled`.

### 2. `promote` has no cycle guard — ✅ DONE (2026-05-30)
`promote` now threads a `PromoteForward` table (LOCAL index → RUNTIME handle) and
**reserves-then-fills** the two cyclic-capable RUNTIME slabs — `closures` and
`envs` are `boxcar::Vec<OnceLock<…>>`, so `promote_closure`/`promote_env` push an
empty cell, register the handle in `fwd`, recurse (the back-edge resolves to the
reserved handle), then `set` the cell once. Pairs/vectors/maps stay un-forwarded
(acyclic by construction). The `(let (g (fn () g)) g)` repro and `letrec`
mutual-recursion now promote correctly — verified cross-process by
`gc.rs::promotes_cyclic_local_closures_without_crashing`. The `OnceLock` adds an
infallible `get()` to the hot RUNTIME-closure read path; fib shows no measurable
regression. See the 2026-05-30 devlog entry.

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

### 4. Tighten the ADR-043 memory caps — ✅ DONE (2026-05-30)
`TEST_DEFAULT_HARD/SOFT` (`core/alloc.rs`) dropped from 5 GiB / 4 GiB to **2 GiB /
1 GiB** — ~4× the ~240 MB collected suite peak: high enough never to trip on
legitimate parallel load, low enough to catch a genuine runaway cleanly via
`E0043` before the hard abort. Full suite passes under the tighter caps. The
stale "GC is a no-op / never reclaims" prose in that doc-comment was corrected too.

### 5. Deferred optimizations
- **Generational collection:** today `arena_flip` is a full semi-space copy each
  time, recopying long-lived data. A young/old split would cut copying of stable
  data. **(Still open — the one remaining GC perf item.)**
- **`Rc` → `gc-arena` (ADR-002):** ✅ closed (2026-05-30). ADR-002's status now
  records that the `Rc`/`RefCell` substrate was replaced wholesale by the
  hand-rolled handle/slab copying collector (ADR-035/054/055/061), *not* migrated
  to `gc-arena`; nothing left to carry.

## Benchmark: collect-at-any-depth overhead

> **Resolved by item #1 (2026-05-30).** The region-check rooting clawed back
> ~10–14% of the overhead below — archive `docs/benchmarks/2026-05-30T00-54-34Z.md`
> and the 2026-05-30 devlog entry have the post-fix numbers. The table here is the
> *pre-fix* measurement that motivated the work.

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
