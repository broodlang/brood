# Testing in Brood

Brood ships a small test framework written **in Brood itself** (`std/test.lisp`),
loaded with `(require 'test)`. It is ExUnit / `mix test`-flavoured: `describe`
groups, `test` cases, and a runner that runs everything **in parallel by
default** across the process model, with opt-in serialisation for tests that
share state.

```bash
./bin/cli tests/suite.lisp      # the project suite
cargo test                      # runs the same suite via crates/lisp/tests/suite.rs
```

## Writing tests

```lisp
(require 'test)

(describe "arithmetic"
  (test "addition"   (assert= (+ 1 2 3) 6))
  (test "division"   (assert= (/ 12 3) 4) (assert= (/ 7 2) 3.5)))

(describe "errors"
  (test "catches a throw" (assert= (try (throw 42) (catch e e)) 42))
  (test "div-by-zero"     (assert-error (/ 1 0))))

(run-tests)
```

- **`(describe "group" body…)`** — names a group of related cases.
- **`(test "name" body…)`** — one case. The body is any Brood code plus
  assertions.
- **`(deftest name body…)`** — a single case named by a symbol, no group. Kept
  for convenience; expands to `test`.

### Assertions

| form | passes when | failure message |
|---|---|---|
| `(is expr)` | `expr` is truthy | `is: expected truthy, got <v>` |
| `(assert= actual expected)` | `(= actual expected)` | `<actual> ≠ <expected>` |
| `(assert-error body…)` | evaluating `body` raises | `expected an error, none raised` |

Assertions **do not stop the test** — every assertion in a body runs, so one
test can report several failures. Each operand is evaluated once.

`(error-of body…)` is a helper, not an assertion: it evaluates `body` and yields
the error it raised — a built-in error as its message string, a `(throw v)` as
`v` — or `nil` if nothing was raised. Pair it with `assert=` to pin exact output:

```lisp
(assert= (error-of (/ 1 0)) "runtime error: division by zero")
(assert= (error-of (+ 1 2)) nil)            ; also a plain "did it raise?" probe
```

## Execution model — parallel by default

Every test runs concurrently, **each in its own process** (`spawn`/`receive`),
on its own OS thread. Two opt-outs, written as a keyword right after the group or
test name:

```lisp
(describe "fast, independent"        ...)  ; default: every test in parallel
(describe "writes a shared file" :serial   ...)  ; its tests run one-at-a-time,
                                            ;   in one worker, but alongside
                                            ;   other groups
(describe "redefines a global" :isolated   ...)  ; runs ALONE — nothing else runs
                                            ;   at the same time
(test "touches global state" :isolated     ...)  ; a lone isolated test
```

| mode | within the group | versus other groups |
|---|---|---|
| *(default)* | each test in its own process, in parallel | parallel |
| `:serial` | one process, tests in sequence | parallel |
| `:isolated` | one process, tests in sequence | **exclusive** (runs alone) |

**Why this exists.** A runtime's processes **share one global table** (see
[`shared-code.md`](shared-code.md)). Two parallel tests that both redefine the
same global would race. A test that only reads the prelude and its own locals is
safe to run in parallel — the default. A test that `def`s or `set!`s a *shared*
name (or relies on ordering, or a shared external resource) should mark its group
`:serial` (serialise within the group) or `:isolated` (serialise against
everything).

**Phases.** The runner spawns all `:parallel` and `:serial` units and runs them
together; once they finish, it runs the `:isolated` units one at a time *on the
runner itself*, so nothing else is executing.

## Share-safe tallying (how it works)

The interesting constraint: because processes share the global table, the
framework **must not** keep its pass/fail counts in shared mutable globals — two
concurrent tests would clobber each other (this was a real bug in an earlier,
isolation-assuming design).

Instead:

- Each test body runs inside a **process-local accumulator**, `*fails*` — a `let`
  binding the `test` macro establishes. The assertions are *macros* that push a
  message onto `*fails*` on failure. So a test's failures live only in its own
  process.
- The test **yields its `*fails*` list as a value**. A worker sends its unit's
  results back as a message; the runner aggregates everything into its own local
  state and reports. No shared counters, no races.

A corollary: assertions must be used **lexically inside a test body** (they refer
to that body's `*fails*`), not from unrelated top-level helper functions.

## Running

```lisp
(run-tests)            ; parallel, dots (. pass / F fail), summary
(run-tests :trace)     ; a ✓/✗ line per test as it finishes, instead of dots
(run-tests :slow)      ; after the summary, list the slowest tests
(run-tests :trace :slow)
```

`run-tests` prints progress, then any failures (one line per failed assertion,
attributed to its test), then a summary:

```
40 tests, 40 passed, 0 failed (0 failed assertions, 1 isolated)
  (706 ms, peak 30.0 MB)
  39 processes / 39 OS threads created (1 runner + 37 unit workers + 1 nested), peak 34 alive at once
```

The last line reflects the process model: one OS thread per process today
(step 4a in [`concurrency.md`](concurrency.md)). Read it carefully — the counts
are **threads, not cores**: "created" is the total spawned over the whole run
(born and gone over time), and "alive at once" is the high-water mark of threads
existing simultaneously. Both can exceed your core count; the OS time-slices
threads onto whatever cores exist. (Decoupling process count from OS-thread count
— green M:N processes on a ~core-sized pool — is step 4b.) `run-tests` raises an
error if anything failed, so the process exits non-zero — which is how
`cargo test` notices.

See `tests/suite.lisp` for the real suite and `tests/suite-failures.lisp` for a
deliberately-failing file you can run by hand to see the failure report.

## Relationship to Rust tests

- `crates/lisp/tests/basic.rs` — Rust end-to-end checks of the language
  (including `live_redefinition` and `spawned_process_picks_up_redefinition`).
- `crates/lisp/tests/suite.rs` — runs `tests/suite.lisp` through an `Interp`; the
  suite signals failure by raising, so `Ok` means every in-language assertion
  passed.

When you add a language feature, add an in-language case to `tests/suite.lisp`
and/or a Rust case in `basic.rs` (see the checklist in `CLAUDE.md`).
