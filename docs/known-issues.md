# Known issues

Confirmed interpreter defects with reproductions and current mitigations.
Newest first. For the narrative discovery writeup of the scheduler race, see
[claude-demo-findings.md](claude-demo-findings.md).

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
   indefinite hang (`std/test.blsp`; regression test
   `tests/runner_failfast_test.blsp`). This is independent of KI-1: the lookup
   race can still *kill* a worker, but the runner now fails fast with the death
   reason rather than hanging. An unattended `nest test` / `cargo test` therefore
   reports red instead of blocking.

### Mitigations (no longer required for correctness)

With the race fixed (KI-1), the default multi-threaded scheduler is safe; the
options below remain useful for *bounding* a heavy run, not for avoiding crashes:

- `nest test -j 1` (serialize the scheduler), or
- mark heavy tests `:isolated` (std/test.blsp runs isolated units alone on the
  runner before the parallel phase), or `:serial` to group them in one process.
  Verified: the `foobar` mandel test marked `:isolated` is 8/8 green.

---

## Platform gaps — GUI display seam (not defects, missing capability)

**Status:** GG-1 + GG-3 **resolved 2026-05-31** (ADR-079); GG-2 still open ·
**Severity:** low (was medium) · **First seen:** 2026-05-31 · **Source:** building
the `foobar` Game of Life demo's split view (`~/src/whk/foobar/src/life.blsp` —
board + a larger-font status strip).

The GUI frontend used to have exactly **one font size for everything** — no pane,
op, or buffer could be bigger than another, so the only way to enlarge text was a
hand-rolled "block font" magnified out of grid cells (what `life.blsp`'s
`status`/`glyph-row`/`scale-row`/`status-ops` do). The three gaps:

- **GG-1 — no per-op / per-region font size. ✅ Resolved (ADR-079).** A `Face` now
  carries an integer `:scale` (≥1, default 1, capped at 16): the renderer draws that
  op's text `scale`× larger in a `scale`×`scale` block of base cells anchored at its
  `(row, col)` (`crates/lisp/src/gui.rs` — `Face.scale` + `paint`/`draw_char`;
  parsed in `builtins.rs` `gui_face`; documented in `std/face.blsp`). Mixed-size text
  in one frame is now `[:text r c s {:scale 2}]`; the terminal renders 1×. Chose the
  face-key route over a new op or a std block-font module (faces already flow
  end-to-end; the grid stays uniform — positions are still base cells). Arbitrary
  per-pixel `:height` sizing is deferred (would break the single grid; needs a
  metrics-query primitive).
- **GG-2 — `gui-font!` is global across *all* windows. ⬜ Open.** The
  `UserEvent::Font` handler applies the spec to every open window (`gui.rs`: `for w
  in self.wins.values_mut() { w.renderer.set_font(…) }`), so the "two windows"
  escape hatch fails — enlarging a second window resizes the first too. *Possible
  fix:* a per-window form `(gui-font! id spec)`, leaving the no-id call as the global
  default. Independent of GG-1 and smaller; not yet done.
- **GG-3 — no display-side pane/clip/font layer. ✅ Resolved.** `std/window.blsp`
  (ADR-077/078) provides the *pane layout + clip-rect* abstraction (a split tree →
  pane rects + dividers), and the *per-pane font scale* remainder collapsed into
  GG-1 — a pane/buffer now renders its text with a face carrying its `:scale`, so
  per-buffer font is pure Brood policy.

**Resolution:** GG-1 shipped as a `Face` `:scale` (ADR-079) — it also closed GG-3's
remainder and reduces the `life.blsp` block-font workaround to `[:text … {:scale
n}]`. **GG-2 remains open** as an independent, smaller follow-up.

## Minor

- ~~**Type-checker noise around `(require 'hatch)`.**~~ **Fixed.** `check_file`
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
