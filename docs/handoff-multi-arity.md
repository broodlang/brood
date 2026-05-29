# Handoff — runtime safety + native multi-arity dispatch (2026-05-29)

> For the next Claude (or human) picking this up. This session did two big
> things: **(A) made runaway recursion/allocation safe** (no more host crashes),
> and **(B) added native multi-arity closure dispatch** so Brood's variadic
> arithmetic stops being ~40× slower than a direct call. All of (A) is done and
> committed-to-the-working-tree. (B) is **implemented and validated**; only the
> docs/ADR + a final full-suite confirmation remain.
>
> **Everything below is uncommitted working-tree changes.** Nothing has been
> committed. The user edits the editor (M2/M3: ropes, `term_*`, `display.blsp`,
> `observe.blsp`) *concurrently*, so the build intermittently breaks on *their*
> work — that is not your changes. Re-read files before editing; treat moved
> files as the new reality (see CLAUDE.md "the tree changes under you").

## TL;DR status

| Work | Status |
|------|--------|
| ADR-043 memory limits actually enforce + don't false-crash | ✅ done |
| Byte-based stack guard (E0044) — `(boom 0)` no longer SIGSEGVs a green process | ✅ done, validated |
| Test memory cap = host-safety backstop (5 GiB hard / 4 GiB soft) | ✅ done |
| Test framework: `:isolated` units run in droppable processes (was 18 GiB accumulation) | ✅ done |
| Adversarial test bugs (reader, blob, cross-test contamination) | ✅ done, 22/22 green standalone |
| **Native multi-arity dispatch (ADR-046)** | ✅ implemented + validated (8.1× win) |
| → ADR-046 in `docs/decisions.md`, devlog entry, `language.md` note, roadmap tick | ⛔ **TODO** |
| → final `cargo test` green confirmation | ⏳ running / confirm |

## How to resume in one paragraph

The build is green (as of handoff). Run `cargo test` — if green, the only thing
left is **docs**: write **ADR-046** (native multi-arity dispatch) in
`docs/decisions.md`, add a dated `docs/devlog.md` entry covering this whole
session, add a short multi-arity note to `docs/language.md`, and tick the
roadmap. Then it's done. If `cargo test` is *not* green, see "If tests fail"
below.

---

## Part A — runaway-resource safety (done)

The working tree started with an in-flight ADR-043 ("runaway backstops") that
**did not actually work**: the E0044 eval-depth ceiling was a frame *count*
(default 3500) miscalibrated by ~40× — a debug green-process coroutine (2 MiB
stack) overflows at ~90 frames, so `(defn boom (n) (+ 1 (boom (+ n 1)))) (boom 0)`
still **SIGSEGV'd a green process** (the exact MCP-server-killer from
`docs/claude-demo-findings.md`). Fixed by:

- **Byte-based stack guard** (`crates/lisp/src/process/scheduler.rs`): record the
  per-coroutine stack base sp at the outermost eval, save/restore it across
  suspend alongside `GC_BLOCK` (in `scheduler::preempt` and
  `mailbox.rs` receive), and in `eval/mod.rs` check `base - sp` against
  `stack_budget()` every eval → clean catchable **E0044**. Frame-counting can't
  work (heavy vs light frames differ ~7× in bytes). `CORO_STACK_BYTES` bumped
  2 MiB→16 MiB (lazy mmap, ~free); `brood`/`nest`/`suite.rs` re-home their root
  work onto a `CORO_STACK_BYTES` thread so the budget is uniform. Verified:
  `(boom 0)` → clean E0044 at root **and** in a green process; legit non-tail
  recursion works to 300+ levels.
- **Soft memory limit depth-independent** (`eval/mod.rs`): the E0043 check is no
  longer gated on `gc_block_depth()==1`, so a runaway in argument position is
  caught (raising just unwinds — unlike GC, no rooting constraint).
- **Test memory cap** (`crates/lisp/src/core/alloc.rs` `TEST_DEFAULT_HARD/SOFT`):
  **5 GiB / 4 GiB**. This is a *host-survival backstop*, NOT a working-set
  budget. **NEVER set it `0`/unlimited** — doing so once OOM-froze the user's
  machine (no GC → the suite tried ~18 GiB). See `memory/no-gc-suite-memory.md`.
- **Test framework** (`std/test.blsp` `run-isolated`): `:isolated` units now run
  in their **own spawned process** (one at a time), so each unit's heap is
  reclaimed on exit. Previously every isolated test accumulated on the long-lived
  runner heap → ~18 GiB suite peak. Now bounded (~190 MB isolated phase).
- **Adversarial tests** (`tests/adversarial_test.blsp`): fixed the "very long
  atom" test (string vs symbol), the 200-worker blob test (echoers now report
  `%blob-ptr` so `adv-collect` drains all 200 — undrained strings were
  contaminating later `:isolated` tests on the shared runner mailbox), and capped
  the heaviest stress counts (100k→30k) since no-GC accumulation is real.

These all compiled and passed before the multi-arity work; re-confirm with
`cargo test` once green.

## Part B — native multi-arity dispatch (ADR-046) — THE MAIN NEW FEATURE

**Why** (user's explicit direction — see CLAUDE.md "Dogfood first; optimize only
by building the language up"): variadic `+`/`-`/`=` are Brood `defn`s over `fold`,
costing ~40× a direct call (the ~5KB/iteration that made `(sum-to 100000)` use
497 MB — *not* a leak; each `(+ a b)` allocated a `& xs` rest-list + a `fold` +
`fold--loop`/`empty?`/`first`/`rest` chain ≈ 15 env frames, none reclaimed
without GC). The user chose to **fix the language, not move `+` to Rust**: give
the evaluator efficient multi-arity dispatch so `+` stays Brood *and* is fast.

**What** — Clojure-style: a multi-arity fn has one `ClosureArm` per arity clause;
the call's argument count selects the arm and a fixed clause binds its params
**directly** (no rest-list, no `match*`). Arity-only clauses (plain symbol params
+ `&optional`/`&`) dispatch natively; *pattern* clauses (literals/destructuring,
e.g. `((3 _) …)`) still lower to the `match*` engine.

**Validated** (2026-05-29, against the green build):
- Correctness: `(+) (+ 5) (+ 1 2) (+ 1 2 3 4)` → `0 5 3 10`; `(- 5)`→`-5`,
  `(- 10 3 2)`→`5`; `(< 1 2 3)`→`true`, `(< 1 3 2)`→`false`; `=`/`<=`/`>=`/`not=`
  all correct. Pattern multi-clause (`alive-next?`-style) still works.
- **Memory: `(sum-to 100000 0)` = 61 MB, was 497 MB → 8.1×.** Matches the
  fixed-arity floor measured earlier.

### Files changed for multi-arity (all done)

- `crates/lisp/src/core/value.rs` — **`Closure` now holds `arms: Vec<ClosureArm>`**
  (was flat `params/optionals/rest/body`); `name/doc/env` stay at `Closure`
  level. New `ClosureArm` struct + `min_arity/max_arity/accepts`, and
  `Closure::single(...)` ctor + `Closure::select_arm(argc)` (prefers exact fixed
  arm over variadic; most-specific among matches).
- `crates/lisp/src/eval/mod.rs` — `bind_params` selects the arm by argc and
  returns `(scope, body)`; `apply_closure` + the inline TCO call path use it;
  `make_closure` builds multi-arm for arity-only multi-clause, else single arm;
  `value_arity` spans arms; new `arity_error_for` lists accepted arities.
- `crates/lisp/src/eval/macros.rs` — `is_arity_param_list`/`is_arity_clause`
  (pub(crate)); `fn_needs_lowering` + `lower_fn` **leave arity-only multi-clause
  un-lowered** (return None) and only lower *pattern* clauses to `match*`; the
  compile-pass `fn` branch calls new `fn_is_arity_multi_clause` →
  `expand_fn_clauses` (expands each clause **body**, leaves each param-list
  opaque — critical: the generic `expand_tail` would mangle a second clause's
  `(a)` param-list into a call).
- `crates/lisp/src/core/heap.rs` — every closure traversal now iterates arms:
  `promote_closure`, `flush` (arena-flip), GC trace, `closures_structurally_equal`
  (Stage-5 dedup), the prelude-builder `to_prelude` rewrite. (`ClosureArm` added
  to the `use value::{…}` import.)
- `crates/lisp/src/process/message.rs` — `ClosureMsg` now has `arms:
  Vec<ClosureArmMsg>` (+ new `ClosureArmMsg`); `to_message`/`from_message`
  round-trip all arms (cross-process spawn of a multi-arity closure).
- `crates/lisp/src/process.rs` — exports `ClosureArmMsg`.
- `crates/lisp/src/dist/wire.rs` — `encode_closure`/`decode_closure` serialize
  arms (cross-node); the two round-trip tests rewritten (one now a 2-arm closure).
- `crates/lisp/src/types/check/sigs.rs` — `infer_sig` only for single-arm
  closures (sound — no false inference for multi-arity); `arity_of` spans arms.
- `crates/lisp/src/builtins.rs` — `arglist` shows the last arm (most general).
- `std/prelude.blsp` — `+ * - / < > <= >= = not=` rewritten as multi-arity (fast
  0/1/2-arg arms + variadic fallback). This is the actual perf payoff.
- `CLAUDE.md` — added the "Dogfood first; optimize only by building the language
  up" principle (the two-criteria bar; multi-arity as the worked example).

## Remaining work (do these to finish)

1. **`cargo test`** — confirm green (a run was kicked off at handoff; check its
   output). The 8.1× arithmetic win should also drop the *suite* peak well under
   the 4 GiB soft cap — verify it no longer trips E0043. Also eyeball that the
   suite-wide memory is much lower than the pre-multi-arity ~3 GiB.
2. **ADR-046** in `docs/decisions.md` (next free number — 044=supervision,
   045=ropes are taken; the prelude comment already references **ADR-046**).
   Title: "Native multi-arity closure dispatch". Cover: the dogfooding rationale,
   arms vs `match*` split, `select_arm` semantics, the 8.1× measurement, and that
   it keeps `+` in Brood.
3. **`docs/devlog.md`** — dated entry for this whole session (safety fixes +
   multi-arity). Newest at the bottom.
4. **`docs/language.md`** — short note that `fn`/`defn` support multi-arity
   (arg-count dispatch) distinct from pattern multi-clause.
5. **`docs/roadmap.md`** — tick multi-arity dispatch if it's listed; else add to
   the devlog.
6. Optional: `docs/error-codes.md` E0044 row already updated to "byte budget /
   BROOD_STACK_BUDGET" — double-check it reads right.

## If tests fail

- A **memory-limit E0043** in the suite → the cap (`alloc.rs` TEST_DEFAULT) is
  too low for the no-GC suite peak. **Do not set it to 0** (OOM-froze the host
  once). Measure the true peak with `BROOD_MEM_LIMIT=24G nest test` (safe on a
  >24 GB host) and set the cap above it with headroom. With multi-arity the peak
  should be far lower now.
- A **stack overflow / SIGSEGV** in a green process → the byte guard's base
  save/restore missed a suspend site, or `STACK_BUDGET`/`CORO_STACK_BYTES` are
  mismatched. Test: `(spawn (send root (try (boom 0) (catch e (get e :code)))))`
  must yield `E0044`, not a segfault.
- A **multi-arity dispatch bug** → smoke test:
  `(defn g (() :zero) ((a) [:one a]) ((a b) [:two a b]) ((a b & more) [:many a b more]))`
  then `(g) (g 1) (g 1 2) (g 1 2 3 4)`; and a pattern fn
  `(defn rule ((3 _) :birth) ((2 alive) alive) ((_ _) :dead))`.

## Key facts so you don't re-derive them

- **The GC is a no-op** (`Heap::collect` is `// no-op`; bump allocator never
  reclaims — the tracing-GC migration is pending M1). This is THE root cause of
  the suite's memory size. Memory is reclaimed only by `(hibernate)` (arena
  flush) or a process exiting. Multi-arity cut the *per-op* allocation ~8×, which
  helps enormously, but a true fix for long-lived accumulation is GC.
- Arithmetic floor numbers (debug & release alike): variadic `+` ≈ 5 KB/call;
  fixed-arity arm ≈ 1 env frame (~0.6 KB/call); raw primitive ≈ 0.16 KB.
- Persistent memory files already written:
  `memory/no-gc-suite-memory.md` (why the suite uses GBs; the cap is a backstop),
  `memory/editor-build-direction.md` (the user's M2/M3 direction).
- The user works on the editor **concurrently** — build breaks you see are
  usually their in-flight `term_*`/`display.blsp`/`observe.blsp`/rope work, not
  yours. Filter `cargo build` errors for `term_|display\.blsp|observe\.blsp` to
  tell them apart.

## Don't

- Don't commit/push/reset/checkout/stash unless the user asks (CLAUDE.md).
- Don't make `+`/`-`/`=` Rust primitives — the user explicitly rejected that;
  multi-arity dispatch is the chosen, dogfood-aligned fix.
- Don't set the test memory cap to unlimited.
