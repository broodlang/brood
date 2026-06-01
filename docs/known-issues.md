# Known issues

Confirmed interpreter defects with reproductions and current mitigations.
Newest first. For the narrative discovery writeup of the scheduler race, see
[claude-demo-findings.md](claude-demo-findings.md).

---

## KI-3 — RUNTIME compaction strands live VM / tree-walker constants

**Status:** **fixed (2026-06-01)** · **Severity:** was high (silent data corruption /
GC slab-OOB panic) → none · **First seen:** 2026-06-01, via a long-lived `nest mcp`
session building the `brood-terminal` demo (loading a string-literal-heavy module in
a loop).

### Symptom

A `flush_oob` panic in the RUNTIME compactor (`heap.rs` `flush_pair`/`flush_*`:
`vector handle indexes the source slab out of bounds`), **or** silent corruption — a
string/quoted-data **constant read back as a *different* value** after the region was
compacted. E.g. `(load "…")` in a loop would intermittently fail with a garbled path,
or a literal's `string-length` would change mid-loop.

### Root cause

The same false invariant in two places, exposed once the ADR-076 RUNTIME compactor
made promoted code-region handles **movable** (previously the region was append-only,
so a promoted constant never moved):

1. **Tree-walker.** `Heap::root`/`root_env` elided the operand-stack push for any
   "immovable" value — and counted a RUNTIME handle as immovable (`Root::Stable`).
   `runtime_collect` only rewrites the operand stack, so an inlined RUNTIME root held
   by an ancestor `eval_at` frame (a `let` body, a `do` spine cursor) went stale.
2. **Compiling VM.** `Node::Const`/`MakeClosure.fn_rest` held a promoted RUNTIME
   handle inline; the `Arc<CompiledArm>` node tree is off the GC root graph and
   `exec_node` walks it by `&Node`, so the `Arc` can't be swapped for a relocated
   copy. A compaction at a nested `eval_at` safepoint (e.g. a builtin like `load`)
   evacuated the region out from under the live arm, leaving its consts dangling.

### Fix

- **Rooting** (`heap.rs`): a new `needs_root_slot` (= LOCAL **or** RUNTIME) replaces
  `is_movable` in `root`/`root_env`, so a RUNTIME handle takes an operand-stack slot
  and `runtime_collect` rewrites it.
- **VM** (`compile.rs` + `heap.rs`): `Node::Const`/`MakeClosure.fn_rest` carry a
  movable handle as `ConstVal::Handle { kind, AtomicU64 }` (atom literals stay inline,
  zero-cost). `vm_apply` (and the top-level `run`) register their live arm in
  `Heap::live_vm_arms`; `runtime_collect` walks the live arms (`rewrite_arm_handles`)
  and rewrites their handles **in place** with the same forwarding map — so compaction
  stays correct *and* bounded while the VM is mid-call (no deferral).

A `flush_bound!` self-diagnosing OOB and the `BROOD_RT_GC_FLOOR` knob (set huge to
disable RUNTIME compaction) were the key bisection aids.

### Reproduction (manual; the race is slab-packing sensitive)

```
# in a Brood project with a string-literal-heavy module (e.g. brood-terminal/src/commands.blsp):
printf '(defn probe (i) (load "src/commands.blsp") (string-length "src/commands.blsp")) \
        (defn bad (i n a) (if (= i n) a (bad (+ i 1) n (if (= (probe i) 17) a (+ a 1))))) \
        (println "bad=" (bad 0 60 0))' > /tmp/r.blsp
BROOD_GC_STRESS=1 brood /tmp/r.blsp     # pre-fix: bad=1 (corruption); fixed: bad=0
BROOD_RT_GC_FLOOR=100000000 brood /tmp/r.blsp   # clean either way → implicates the RT collector
BROOD_VM=0 brood /tmp/r.blsp            # tree-walker path (bug #1 only)
```

### Regression tests

The end-to-end corruption is a slab-packing race (only reliable with a large varied
churn file under `BROOD_GC_STRESS`), so the *mechanism* is unit-tested deterministically
in `crates/lisp/src/eval/compile.rs` (`compile::tests::const_handle_round_trips`,
`rewrite_arm_handles_rewrites_every_embedded_handle`), and
`crates/lisp/tests/runtime_collector.rs::auto_safepoint_collect_bounds_runtime_region`
exercises mid-execution compaction + post-compaction correctness.

---

## KI-1 — Multi-thread scheduler race: green processes can't resolve globals

**Status:** **fixed** (2026-05-29) · **Severity:** was high → none · **First
seen:** 2026-05-28 · also in [claude-demo-findings.md](claude-demo-findings.md)
§1.1

### How it was fixed

Three changes landed in series:

1. **`e3d3a0d` (2026-05-28 evening) — supervisor scaffolding stripped.** The
   kernel-level supervisor (RESUME_SLOT thread-local, safepoint rooting,
   `supervise()` retry loop, `%spawn-supervised*` primitives, the
   `(supervise …)` macro) was contributing the bulk of the race surface.
   Stripping it cut the `recurse.blsp` repro from ~24 worker deaths per run
   (0/n clean) to ~0–1 per run (5/10 clean). See ADR-039 (reverted) and
   [`supervision.md`](supervision.md) for the rationale.
2. **`f90f0de` (2026-05-29 morning) — Phase-1 bump-only allocator.** Heap
   allocations now grow monotonically per process; no slot reuse and
   `Heap::collect` is a no-op. Stale handles can't exist because slots are
   never recycled, which closes the manual-rooting race the panics rode on.
3. **`2abf05e` (2026-05-29) — per-worker pinned queues.** Each process is
   assigned to one worker at spawn and stays there for its lifetime; no shared
   queue, no work stealing, no cross-thread coroutine migration. Closes the
   remaining plain-release segfault that fired when preempt landed a coroutine
   on a different worker thread mid-call.

Verified post-fix: `recurse.blsp` and `medium.blsp` repros hit **10/10 clean**
in both **debug-assertions release** and **plain release**, single- and
multi-threaded. The 2026-05-28 symptoms (workers dying with bogus `unbound
symbol: fold` / `+` / pattern-bound `iter`-`acc`-`pred`, plus a Rust `index out
of bounds` panic in `eval/mod.rs`) are no longer reproducible.

### 2026-05-31 — re-reported via foobar `pstep`, re-confirmed fixed

A fresh report came in of a slab out-of-bounds **panic in the GC copy phase**
(`heap.rs` `flush_pair`: `index out of bounds: the len is 3007 but the index is
7187`) under foobar's parallel `pstep` (a coordinator fans a Game-of-Life
generation across row-band worker processes that allocate heavily, `send` slices
back, and merge — with a global rebound underfoot). Also seen as a *silently
wrong* `pstep` result on other runs. This is the same use-after-GC / stale-handle
signature as KI-1, surfacing in the moving collector's `flush_*` rather than in
`eval`: a LOCAL-tagged handle reachable from the GC roots whose `index()` is past
the live source slab.

**Confirmed already fixed — does not reproduce on HEAD.** The captured panic came
from a long-lived `nest mcp` server **pinned to a pre-fix binary** (it still had
the 1-arg `gui-font!`). On a current debug-assertions build the exact repro ran:

- the leaner variant under `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1` (collect +
  live-graph verify at every safepoint) — **clean, no verifier trip**, and
- the full repro (2000 trials × 16 workers × 4000-entry maps) on the default
  28-worker scheduler — **30+ min CPU, no panic, no `.brood_crash_dump`**.

Two durable follow-ups landed so a recurrence is self-diagnosing and guarded:

- **Self-diagnosing flush OOB** (`heap.rs` `flush_oob` + `flush_bound!`): every
  `flush_*` source-slab access is now bounds-checked and, on OOB, panics naming
  the handle's kind / region / age / epoch / index / slab-len / collected space —
  instead of the bare slice panic with `<unknown>` release frames. (`copies()`
  gates a copy by region + generation-age but not by slab bound, so this is where
  a stale/foreign handle would land.)
- **Regression test** (`crates/lisp/tests/concurrency_race.rs`,
  `fanout_with_concurrent_global_rebind_matches_serial`): the spawn-N-fan-out +
  concurrent global-rebind reconstruction, asserting the parallel total equals the
  deterministic serial `k*n` over many trials. Encodes the
  [`concurrency-v2.md`](concurrency-v2.md) §6 acceptance bar in the standing suite.

Phase 2 (bounding memory in long-lived receive loops) first shipped as the
explicit `(hibernate fn & args)` primitive (an arena flip), but that was a
Stage-A expedient: it was **removed** (ADR-058) once the automatic semi-space
copying collector (ADR-055) made reclamation fire at the eval safepoint with
nothing asked of the author. Memory is now bounded on every entry path
automatically. Independent of the race fix above.

### Original 2026-05-28 symptom (kept for the record)

Under the default multi-threaded scheduler (`-j 0`), spawning several green
processes that each touched prelude/kernel globals reliably crashed workers
with bogus `unbound symbol` errors on names that *were* bound — both
pattern-bound locals (`iter`, `acc`, `pred`) and builtins (`fold`, `+`, `%eq`)
— followed by an interpreter panic.

Reproduced 2026-05-28 via the `foobar` demo's `mandel/render-concurrent`
(`spawn`ed worker pool + hatch collector), `nest run`:

### Symptom

Under the default multi-threaded scheduler (`-j 0`), spawning several green
processes that each touch prelude/kernel globals reliably crashes workers with
bogus `unbound symbol` errors on names that *are* bound — both pattern-bound
locals (`iter`, `acc`, `pred`) and builtins (`fold`, `+`, `%eq`) — followed by
an interpreter panic.

Reproduced 2026-05-28 via the `foobar` demo's `mandel/render-concurrent`
(`spawn`ed worker pool + hatch collector), `nest run`:

```
hello foobar
process 5 died: unbound error: unbound symbol: iter
process 4 died: unbound error: unbound symbol: fold
process 3 died: unbound error: unbound symbol: fold
process 7 died: unbound error: unbound symbol: iter
thread '<unnamed>' panicked at crates/lisp/src/eval/mod.rs:474:45:
index out of bounds: the len is 0 but the index is 1
process 10 died: unbound error: unbound symbol: +
process 6 panicked
EXIT=124   (parent then blocks forever in receive → hang)
```

The panic line drifts as code changes (`eval/mod.rs:474` on 2026-05-28;
reported as `:380` in the earlier findings doc). The shape is constant: a
worker reads an empty/0-length structure where it expects a populated scope,
i.e. the global/scope table isn't visible from the spawned process's thread.

### Mitigation (when this was open)

Single-threaded: **`-j 1`** (alias `--max-parallel 1`) — `nest run -j 1` /
`nest test -j 1`. Still the recommended workaround on plain release, until
the bundled-WIP segfault under the new allocator is bisected.

### Root cause (post-mortem)

A data race on shared global/scope state through the kernel supervisor's
RESUME_SLOT + safepoint-rooting machinery, exacerbated by free-list slot
reuse in the allocator (a freed slot could be reallocated to a fresh value
while another thread still held a stale handle). Two fixes in series — strip
the supervisor (removes the wide window of shared mutable scheduler state)
and switch to a bump-only allocator (slots are never recycled, so stale
handles can't observe a value of the wrong type). See
[`scheduler.md`](scheduler.md) and [`memory-model.md`](memory-model.md) for
the substrate.

---

## KI-2 — `nest test` flaky + hangs when parallel tests share heavy global lookups

**Status:** **fixed (2026-05-29)** — runner now fails fast *and* the
underlying race is fixed (same as KI-1) · **Severity:** was medium → none ·
**First seen:** 2026-05-28

Same root cause as KI-1, surfacing through the test runner. `nest test` runs
each `test` in its own parallel green process (default scheduler). When more
than one test does real compute over globals concurrently (e.g. two tests each
calling `mandel/render-sequential`), the race fires non-deterministically:

```
process 4 died: arity error: fn: expected 0 arguments, got 1
EXIT=124   (runner does not reap the dead process → whole run hangs)
```

- Frequency: ~1 run in 5 with two such tests in the parallel phase. Each test
  passes when run alone.
- The `arity error: fn: expected 0 arguments, got 1` is a *symptom of a
  corrupted lookup* under the race, not a real 0-arg call — the identical code
  path succeeds in isolation. (A tempting but wrong hypothesis is that
  `(fn (_) ...)` parses as 0-arity; it does not — removing it changes nothing.)

### Two distinct bugs here

1. The lookup race itself (= KI-1). **Fixed (2026-05-29)** — see KI-1 (supervisor
   strip + bump allocator + per-worker pinned queues). The race can no longer
   kill a worker; `-j 1` is no longer required for correctness.
2. ~~**Runner doesn't fail fast:**~~ **Fixed (2026-05-29).** A test process that
   died with an error was not reaped, so the run hung in `(receive)` forever
   instead of reporting the failure. `spawn-units` now `monitor`s every worker
   and `collect-units` accounts for each one exactly once — by its result if it
   reported, otherwise by the `[:down …]` its monitor fires — turning a dead
   worker into a failing result (`"test process died: <reason>"`) instead of an
   indefinite hang (`std/tool/test.blsp`; regression test
   `tests/runner_failfast_test.blsp`). This is independent of KI-1: the lookup
   race can still *kill* a worker, but the runner now fails fast with the death
   reason rather than hanging. An unattended `nest test` / `cargo test` therefore
   reports red instead of blocking.

### Mitigations (no longer required for correctness)

With the race fixed (KI-1), the default multi-threaded scheduler is safe; the
options below remain useful for *bounding* a heavy run, not for avoiding crashes:

- `nest test -j 1` (serialize the scheduler), or
- mark heavy tests `:isolated` (std/tool/test.blsp runs isolated units alone on the
  runner before the parallel phase), or `:serial` to group them in one process.
  Verified: the `foobar` mandel test marked `:isolated` is 8/8 green.

---

## Platform gaps — GUI display seam (not defects, missing capability)

**Status:** **all resolved 2026-05-31** (ADR-079) · **Severity:** low (was medium) ·
**First seen:** 2026-05-31 · **Source:** building the `foobar` Game of Life demo's
split view (`~/src/whk/foobar/src/life.blsp` — board + a larger-font status strip).

The GUI frontend used to have exactly **one font size for everything** — no pane,
op, or buffer could be bigger than another, so the only way to enlarge text was a
hand-rolled "block font" magnified out of grid cells (what `life.blsp`'s
`status`/`glyph-row`/`scale-row`/`status-ops` do). The three gaps:

- **GG-1 — no per-op / per-region font size. ✅ Resolved (ADR-079).** A `Face` now
  carries an integer `:scale` (≥1, default 1, capped at 16): the renderer draws that
  op's text `scale`× larger in a `scale`×`scale` block of base cells anchored at its
  `(row, col)` (`crates/lisp/src/gui.rs` — `Face.scale` + `paint`/`draw_char`;
  parsed in `builtins.rs` `gui_face`; documented in `std/editor/face.blsp`). Mixed-size text
  in one frame is now `[:text r c s {:scale 2}]`; the terminal renders 1×. Chose the
  face-key route over a new op or a std block-font module (faces already flow
  end-to-end; the grid stays uniform — positions are still base cells). Arbitrary
  per-pixel `:height` sizing is deferred (would break the single grid; needs a
  metrics-query primitive).
- **GG-2 — `gui-font!` is global across *all* windows. ✅ Resolved (ADR-079).**
  `gui-font!` now takes an optional leading window id: `(gui-font! spec)` is still
  the global default (every window + ones opened later), while `(gui-font! id spec)`
  retunes *just that window* and leaves the global default and other windows alone —
  so two windows can run different fonts. The `UserEvent::Font` event carries `id:
  Option<u64>` and both arms share an `apply_font` helper (`crates/lisp/src/gui.rs`;
  parsed in `builtins.rs` `gui_font`, arity `range(1,2)`).
- **GG-3 — no display-side editor/pane/clip/font layer. ✅ Resolved.** `std/editor/pane.blsp`
  (ADR-077/078) provides the *pane layout + clip-rect* abstraction (a split tree →
  pane rects + dividers), and the *per-pane font scale* remainder collapsed into
  GG-1 — a editor/pane/buffer now renders its text with a face carrying its `:scale`, so
  per-buffer font is pure Brood policy.

**Resolution:** all three closed under ADR-079. GG-1 shipped as a `Face` `:scale`
(also closing GG-3's per-pane-font remainder, and reducing the `life.blsp`
block-font workaround to `[:text … {:scale n}]`); GG-2 as an optional window-id
argument to `gui-font!` for per-window fonts.

## Minor

- ~~**Type-checker noise around `(require 'proc/hatch)`.**~~ **Fixed.** `check_file`
  pre-evaluates top-level `(require …)` forms before walking, so macros from
  the required module (`defprocess`, `!`, `hatch`, `gen-call`, `sleep`)
  resolve correctly and don't trip the unbound-symbol diagnostic. Applies to
  both `nest check` (project-aware) and `brood file.blsp` direct. See
  `crates/lisp/src/types/check.rs:148+`.
- ~~**`nest format` collapses multi-line forms** onto single long lines.~~
  **Substantially fixed** (commit `5b19787`, "formatter respects author
  newlines"). Multi-line `let` / `defmacro` body / `cond` / quasiquoted
  templates stay multi-line. **Still normalizes** author-chosen multi-space
  alignment *within* a line (`w       64` → `w 64`) — a standard
  Lisp-formatter trade-off, not the original blocker.
- ~~**Plain-release segfault** under the multi-threaded scheduler on
  tail-recursive workers with heavy prelude churn.~~ **Fixed** by `2abf05e`
  (per-worker pinned queues — no cross-thread coroutine migration). See KI-1.
- ~~**`cargo test -p brood --test suite` segfault** in debug builds.~~
  **Fixed** (2026-05-29) — coroutine stack overflow, not a memory bug. Debug
  eval frames recurse deeper (no inlining) than release, and post-Phase-1
  poison checks widened them further. Bumped `CORO_STACK_BYTES` from 1 → 2
  MiB (`crates/lisp/src/process/scheduler.rs`). Pages are mmap'd lazily, so
  the higher ceiling costs ~0 until depth needs it.
