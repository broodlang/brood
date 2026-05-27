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
nest test        # find project.blsp, discover tests/**/*_test.blsp, run them once
make suite        # the same, via cargo
cargo test        # Rust tests + the same in-language suite (crates/lisp/tests/suite.rs)
```

`nest test` walks up from the current directory for a `project.blsp` manifest,
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
```

A `*_test.blsp` file under `tests/` only **registers** its cases like this — it
does *not* call `(run-tests)`. The project runner (`nest test`) discovers the
file, loads it, and runs the whole suite once. (To run a single self-contained
test file outside a project, use `brood --test file.blsp` — it loads the file
and calls `(run-tests)` for you. The language binary `brood` only ever runs a
*single* file as tests; project-wide discovery is `nest test`.)

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

A test can report **several failures** — one per top-level body form. Each operand
is evaluated once. With no mutable accumulator (data is immutable, ADR-026), an
assertion signals failure by **throwing** a tagged record; the `test` macro runs
each top-level body form in its own `try` (`test--run`), so a throw ends only that
form and the next form still runs. The exception: multiple assertions nested inside
**one** form stop at the first (the throw unwinds the whole form).

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
safe to run in parallel — the default. A test that `def`s a *shared* name (or
relies on ordering, or a shared external resource) should mark its group
`:serial` (serialise within the group) or `:isolated` (run alone **and** against a
rolled-back private copy of the globals, so its `def`s can't leak to any
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

Immutability makes this fall out for free — there is *no* shared (or even
process-local) mutable accumulator to race on:

- An assertion signals failure by **throwing** a tagged failure record
  (`(:%test-fail loc details)`, via `test--fail`). The `test` macro splits its body
  into one thunk per top-level form; `test--run` runs each in its own `try`,
  collecting the caught failures into a list — so each test **yields its failure
  list as a value** (empty = passed, one record per failing form). An uncaught
  (non-assertion) error is recorded and stops the test.
- A worker sends its unit's results back as a message; the runner aggregates
  everything into its own local state and reports. No shared counters, no
  mutation, no races.

A corollary: assertions must be used **lexically inside a test body** (the throw
must reach that body's `try`), not from unrelated top-level helper functions.

## Running

In a project, run the whole suite with **`nest test`** (or `make suite`, or
`cargo test`): the runner discovers `tests/**/*_test.blsp`, loads them, and calls
`run-tests` once (`nest test` passes `:trace`). `run-tests` itself takes the flags
below — forwarded by the runner, and usable directly if you call it yourself:

```lisp
(run-tests)            ; parallel, dots (. pass / F fail), summary
(run-tests :trace)     ; a ✓/✗ line per test as it finishes, instead of dots
(run-tests :slow)      ; after the summary, list the slowest tests
(run-tests :trace :slow)
```

`run-tests` prints progress, then any failures (one line per failed assertion,
attributed to its test), then a summary:

```
158 tests, 158 passed, 0 failed (0 failed assertions, 2 isolated)
  test runtime: 1832 ms total — parallel/serial 1831 ms, isolated 1 ms
  (797 ms wall, peak 70.8 MB)
  141 processes (1 runner + 139 unit workers + 1 nested) on 28 worker threads, peak 28 running at once
```

The last line reflects the **green M:N** process model (step 4b in
[`concurrency.md`](concurrency.md)): processes are cheap coroutines multiplexed
onto a fixed pool of ≈`nproc` worker threads — *not* one OS thread each.
"processes" is the total spawned over the run; "running at once" is the
high-water mark, bounded by the pool. `run-tests` raises if anything failed, so
the process exits non-zero — which is how `cargo test` notices.

See `tests/suite_test.blsp` (and the other `tests/*_test.blsp` files) for the real
suite, and `tests/suite-failures.blsp` for a deliberately-failing file you can run
by hand (`brood tests/suite-failures.blsp`) to see the failure report.

## Relationship to Rust tests

- `crates/lisp/tests/basic.rs` — Rust end-to-end checks of the language
  (including `live_redefinition` and `spawned_process_picks_up_redefinition`).
- `crates/lisp/tests/suite.rs` — drives the project test runner: it `cd`s to the
  repo root and evaluates `(require 'project) (run-project-tests)`, which discovers
  and runs every `tests/**/*_test.blsp`. The suite signals failure by raising, so
  `Ok` means every in-language assertion passed.

When you add a language feature, add an in-language case to the relevant
`tests/*_test.blsp` file (or a new one) and/or a Rust case in `basic.rs` (see the
checklist in `CLAUDE.md`).
