# Testing in Brood

Brood ships a small test framework written **in Brood itself** (`std/test.blsp`),
loaded with `(require 'test)`. It is ExUnit / `mix test`-flavoured: `describe`
groups, `test` cases, and a runner that runs everything **in parallel by
default** across the process model, with opt-in serialisation for tests that
share state.

Tests live in a project's `tests/` directory as `*_test.blsp` files. The
**project test runner** (ADR-020) discovers them recursively, loads each (which
only *registers* its cases), and runs the whole suite once:

```bash
brood test        # find project.blsp, discover tests/**/*_test.blsp, run them once
make suite        # the same, via cargo
cargo test        # Rust tests + the same in-language suite (crates/lisp/tests/suite.rs)
```

`brood test` walks up from the current directory for a `project.blsp` manifest,
so it works from anywhere inside a project.

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
| `(is expr)` | `expr` is truthy | `<expr> is <v>` |
| `(refute expr)` | `expr` is falsy | `<expr> is <v> — expected falsy` |
| `(assert= actual expected)` | `(= actual expected)` | `<actual-expr> => <v>, expected <expected-v>` |
| `(assert-error body…)` | evaluating `body` raises | `expected <body> to raise, but none did` |

Every failure message **names the source expression that failed**, quoted at
macro-expansion time — so a failing assertion identifies itself without your
having to open the file or disambiguate look-alike lines. For example, three
different `is` checks fail as `(= 1 2) is false`, `(empty? (list 1)) is false`,
and `(number? "nope") is false` — not three identical lines. Use `assert=` for
equality (it shows both the actual expression's value and the expected value) and
`is` / `refute` for boolean predicates.

Assertions **do not stop the test** — every assertion in a body runs, so one
test can report several failures. Each operand is evaluated once.

Output is **plain text when captured** (a pipe, `cargo test`, CI, or an LLM
reading the run) and **coloured only when stdout is an interactive terminal**
(via the `stdout-tty?` primitive) — so a captured run is never littered with ANSI
escape codes. `tests/suite-failures.blsp` is a runnable demo of the failure
rendering (`./bin/cli tests/suite-failures.blsp`).

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
(describe "redefines a global" :isolated   ...)  ; runs ALONE, against a private
                                            ;   copy of the globals (its defs
                                            ;   roll back) — nothing else runs
(test "touches global state" :isolated     ...)  ; a lone isolated test
```

| mode | within the group | versus other groups | globals |
|---|---|---|---|
| *(default)* | each test in its own process, in parallel | parallel | shared (live table) |
| `:serial` | one process, tests in sequence | parallel | shared (live table) |
| `:isolated` | one process, tests in sequence | **exclusive** (runs alone) | **private copy, rolled back** |

**Why this exists.** A runtime's processes **share one global table** (see
[`shared-code.md`](shared-code.md)). Two parallel tests that both redefine the
same global would race. A test that only reads the prelude and its own locals is
safe to run in parallel — the default. A test that `def`s or `set!`s a *shared*
name (or relies on ordering, or a shared external resource) should mark its group
`:serial` (serialise within the group) or `:isolated` (run alone **and** against a
rolled-back private copy of the globals, so its `def`/`set!` can't leak to any
other test).

**Phases.** The `:isolated` units run **first** — one at a time *on the runner
itself*, each under the `%isolate` primitive (which snapshots the global bindings,
runs the test, then restores them). So every isolated test sees the clean
post-load baseline (none of the parallel/serial defs) and nothing it defines
survives. Only `%isolate` rolls back *bindings*; the append-only code slabs and
the symbol interner still grow (memory, not behaviour — there's no GC yet).
**Then** the runner spawns all `:parallel` and `:serial` units and runs them
together. (`%isolate` is sound only because the isolated phase runs alone, with no
other process mutating globals.)

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

See `tests/suite.blsp` for the real suite and `tests/suite-failures.blsp` for a
deliberately-failing file you can run by hand to see the failure report.

## Relationship to Rust tests

- `crates/lisp/tests/basic.rs` — Rust end-to-end checks of the language
  (including `live_redefinition` and `spawned_process_picks_up_redefinition`).
- `crates/lisp/tests/suite.rs` — runs `tests/suite.blsp` through an `Interp`; the
  suite signals failure by raising, so `Ok` means every in-language assertion
  passed.

When you add a language feature, add an in-language case to `tests/suite.blsp`
and/or a Rust case in `basic.rs` (see the checklist in `CLAUDE.md`).
