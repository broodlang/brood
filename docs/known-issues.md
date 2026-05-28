# Known issues

Confirmed interpreter defects with reproductions and current mitigations.
Newest first. For the narrative discovery writeup of the scheduler race, see
[claude-demo-findings.md](claude-demo-findings.md).

---

## KI-1 — Multi-thread scheduler race: green processes can't resolve globals

**Status:** open · **Severity:** high (blocks all process fan-out under the
default scheduler) · **First seen:** 2026-05-28 · also in
[claude-demo-findings.md](claude-demo-findings.md) §1.1

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

### Mitigation

Run single-threaded: **`-j 1`** (alias `--max-parallel 1`). Verified
2026-05-28 — the same `render-concurrent` program renders cleanly and exits 0
under `nest run -j 1`, and crashes under the default `-j 0`.

```
$ nest run -j 1      # EXIT=0, full symmetric render, 0 deaths
$ nest run           # -j 0 default → workers die, hang
```

### Likely area

`crates/lisp/src/eval/mod.rs` around the scope/global lookup that indexes at
`:474`, plus how the scheduler hands the global environment to worker threads.
A data race on shared global/scope state (it only manifests with real thread
parallelism, never under `-j 1`) — see [scheduler.md](scheduler.md) and
[memory-model.md](memory-model.md).

---

## KI-2 — `nest test` flaky + hangs when parallel tests share heavy global lookups

**Status:** open · **Severity:** medium · **First seen:** 2026-05-28

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

1. The lookup race itself (= KI-1).
2. **Runner doesn't fail fast:** a test process that dies with an error is not
   reaped, so the run hangs instead of reporting the failure. Worth fixing
   independently of KI-1 — a crashed test should surface as a failure, not a
   hang.

### Mitigations

- `nest test -j 1` (serialize the scheduler), or
- mark heavy tests `:isolated` (std/test.blsp runs isolated units alone on the
  runner before the parallel phase), or `:serial` to group them in one process.
  Verified: the `foobar` mandel test marked `:isolated` is 8/8 green.

---

## Minor

- **Type-checker noise around `(require 'hatch)`** — files using
  `defprocess` / `cast` / `!` / `hatch` print spurious "unbound symbol"
  warnings at load (see claude-demo-findings.md §1.2). Cosmetic but reads as
  breakage to new users.
- **`nest format` collapses multi-line forms** onto single long lines
  (claude-demo-findings.md §1.3).
