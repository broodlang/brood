# Testing in Brood

Brood ships a small test framework written **in Brood itself** (`std/tool/test.blsp`),
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
(defmodule arithmetic-test (:use test))     ; `:use`, not a bare `require` — see below

(describe "arithmetic"
  (test "addition"   (assert= (+ 1 2 3) 6))
  (test "division"   (assert= (/ 12 3) 4) (assert= (/ 7 2) 3.5)))

(describe "errors"
  (test "catches a throw" (assert= (try (throw 42) (catch e e)) 42))
  (test "div-by-zero"     (assert-error (/ 1 0))))
```

**Open the file with `(:use test)`, not a bare `(require 'test)`.** Post-ADR-065 a
bare `require` only *loads* a module — it imports nothing — so `describe`/`test`/
`assert=` would stay qualified (`test/describe`, …). `(:use test)` in the
`defmodule` header both loads and imports, which is what makes the macros read
bare. Add a `(:use …)` per module under test too:

```lisp
(defmodule parser-test (:use test) (:use parser))
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
- Both accept `:serial` / `:isolated` (execution mode), `:tags [kw …]`
  (selection) and `:skip` before the body, in any order — see [Tags](#tags) and
  [Skipping a test](#skipping-a-test).
- **`(deftest name body…)`** — a single case named by a symbol, no group. Kept
  for convenience; expands to `test`.

### Assertions

| form | passes when | failure message |
|---|---|---|
| `(is expr)` | `expr` is truthy | `<expr> is <v>` |
| `(refute expr)` | `expr` is falsy | `<expr> is <v> — expected falsy` |
| `(assert= actual expected)` | `(= actual expected)` | `<actual-expr> => <v>, expected <expected-v>` |
| `(assert-error body…)` | evaluating `body` raises | `expected <body> to raise, but none did` |
| `(check-property n gen pred)` | `pred` holds on all `n` generated values | `property failed on trial i/n` + counterexample + seed |

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

### Progress output

By default a run prints **one character per finished test**, as results arrive, so a
long suite shows movement and a failure is visible immediately rather than only in
the summary:

| mark | outcome |
| --- | --- |
| green `.` | passed |
| red `F` | failed |
| orange `○` | skipped (`:skip`) |

**`--trace`** swaps that for one line per test, coloured by outcome and carrying what
a trace is for — the duration and where the test is declared:

```
✓ math › adds                     2ms  tests/math_test.blsp:12
✗ math › divides                  0ms  tests/math_test.blsp:18  (1 failure)
○ db › needs postgres         skipped  tests/db_test.blsp:5
```

It is **opt-in**, and replaces the dots rather than adding to them. The line is
printed when the test *finishes*, which is what makes the outcome colour possible; a
hung test still surfaces, because the runner hard-kills it at `*test-timeout-ms*` and
reports it as a timed-out failure. Lines from parallel workers interleave, in waves
of `*parallel-batch*`.

**`--formatter <name>`** emits machine-readable output instead of the human report:
`tap` (TAP version 13, for a CI that speaks it) or `json` (one object carrying the
full structured results). It suppresses the live progress and the summary, so the
stream is parseable end to end.

**`--stale`** runs only the test files whose sources changed since they last ran — a
file re-runs when it, or a source file it transitively `require`s, has a newer mtime
than the last recorded run. Whole-project runs only (it has no meaning for an
explicit file list). The complement of `--failed`: `--failed` re-runs what *broke*,
`--stale` re-runs what *changed*.

### Skipping a test

`:skip` on a `test` or a `describe` registers the case but never runs its body:

```lisp
(test "needs a live database" :skip (assert= (db-ping) :pong))
(describe "postgres integration" :skip ...)          ; skips every test in the group
```

A skipped test is **still counted and still reported** — that is the difference
between skipping it and deleting or commenting it out — but it counts as neither a
pass nor a failure, and the summary names them separately
(`8 tests, 5 passed, 0 failed, 3 skipped`). `:skip` composes with the other
modifiers (`:serial`, `:isolated`, `:tags`) in any order.

Output is **plain text when captured** (a pipe, `cargo test`, CI, or an LLM
reading the run) and **coloured only when stdout is an interactive terminal**
(via the `stdout-tty?` primitive) — so a captured run is never littered with ANSI
escape codes. `tests/suite_failures_test_ignore.blsp` is a runnable demo of the failure
rendering (`./bin/cli tests/suite_failures_test_ignore.blsp`) — the `_ignore` suffix keeps
the suite from discovering a file that is *meant* to fail.

`(error-of body…)` is a helper, not an assertion: it evaluates `body` and yields
the error it raised — a built-in error as its message string, a `(throw v)` as
`v` — or `nil` if nothing was raised. Pair it with `assert=` to pin exact output:

```lisp
(assert= (error-of (/ 1 0)) "runtime error: division by zero")
(assert= (error-of (+ 1 2)) nil)            ; also a plain "did it raise?" probe
```

### Property-based testing

`(check-property n gen pred)` runs `n` trials. Each draws a value from `gen` — a
`seed -> [value next-seed]` function over the seedable PRNG (`rand-int`,
`rand-float`, `shuffle`, `sample`) — and asserts `(pred value)`. It's
**deterministic** (a fixed start seed, threaded), so a failure reproduces; on the
first counterexample it fails with the value, the trial number, and the seed that
produced it. Sampling many inputs catches edge cases a single hand-written
example misses.

```lisp
(test "rand-int stays in range"
  (check-property 200 (fn (s) (rand-int s 1000)) (fn (x) (and (>= x 0) (< x 1000)))))
(test "shuffle preserves length"
  (check-property 50 (fn (s) (shuffle s [1 2 3 4 5])) (fn (sh) (= (count sh) 5))))
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

## Tags

A `describe` or `test` may carry `:tags [kw …]` before its body, alongside the
`:serial` / `:isolated` mode keywords (in either order). Tags are what
`nest test --only` / `--exclude` / `--include` select on — the ExUnit `@tag`
analogue:

```lisp
(describe "db writes" :serial :tags [:db :slow]
  (test "inserts" ...)                        ; inherits :db :slow
  (test "reconnects" :tags [:flaky] ...))     ; :db :slow :flaky
```

Group-level tags are **merged into** each test's own, so `--only db` picks up a
test tagged only at the group level. Tags are plain keywords; there is no
tag-with-value form (`--only db:primary` treats `db` as the tag and ignores the
value unless the key is the built-in `test`/`describe` pseudo-selector below).

## Selecting which tests to run

`nest test` narrows a run three ways, and they compose:

| flag | selects |
| --- | --- |
| `--only SELECTOR` | run **only** matching tests. Repeatable; several `--only`s **union** |
| `--exclude SELECTOR` | drop matching tests |
| `--include SELECTOR` | re-admit what `--exclude` dropped (`--include` wins) |
| `FILE:LINE` | the test covering that line — point anywhere inside its body |
| `--failed` | only the tests that failed on the previous run here |
| `--partitions N --shard K` | one stable shard of the suite, for CI fan-out |
| `--seed N` | randomise order; the seed is echoed so a failure replays |

A **selector** is a tag name (`db`), a test-name substring (`test:adds`), or a
group-label substring (`describe:math`):

```bash
nest test --only db                  # every test tagged :db
nest test --only test:adds           # tests whose NAME contains "adds"
nest test --only describe:math       # tests in a group whose label contains "math"
nest test --exclude slow             # everything except :slow
nest test --exclude slow --include flaky   # ...but keep the :flaky ones
nest test tests/math_test.blsp:42    # just the test covering line 42
nest test --failed                   # re-run last run's failures
```

`--failed` keeps its record in the project's cache dir as a **set difference**, not
a snapshot: a test that just ran and passed leaves the set, one that just failed
joins it, and one that wasn't run keeps its previous state. So the loop "run
`--failed`, fix one, run `--failed` again" narrows each time. With no record (or
after a fully green run) it warns and runs everything, rather than silently running
nothing.

**A narrowing selector that matches nothing warns.** `--only`, `FILE:LINE` and
`--failed` all report `warning: no tests matched the given filters` when they
select zero tests, because "0 tests, 0 passed" with a zero exit is
indistinguishable from success in CI. The usual causes are a typo'd selector and a
stale `--failed` record naming tests that have since been deleted. An **empty
shard** deliberately does *not* warn — that's normal when a small suite fans across
many machines.

A `FILE:LINE` that addresses no test **runs zero tests and warns** — it does not
fall back to the whole suite, which in CI would be indistinguishable from success.

`--seed` fixes the order tests are **scheduled** in; it is reproducible run to run.
Parallel tests still genuinely interleave, so a *concurrency*-dependent failure may
not recur from the seed alone. In the scoped (whole-project) run the seed shuffles
**file order as well as** the tests within each file, so cross-file order
dependencies are shaken out too.

Numeric flags are range-checked by the argument parser: `--partitions`,
`--max-failures`, `--repeat-until-failure`, `--timeout` and `--slowest` must be
≥ 1, and `--cover-min` must be 0–100. A bad value is rejected before the run
starts.

**Positive selectors union, they don't intersect.** `--only`, `FILE:LINE`,
`--failed` and `--names` all *add* candidates, so `--failed --only slow` runs
(last run's failures) ∪ (`:slow` tests), not their intersection. To narrow rather
than widen, pair one positive selector with `--exclude`:
`nest test --failed --exclude slow`.

`--partitions` assigns each test by a stable hash of its full label, so shards
never overlap or drop a test regardless of machine or run order. `--shard` is
0-based and **required** to be in range: `--shard` without `--partitions`, or a
shard index ≥ the partition count, exits 2 rather than silently running zero
tests (which a CI job would read as green).

## Coverage

Two tiers, both opt-in, and `--cover-min PCT` fails the run below a floor:

| Flag | Measures | Cost |
| --- | --- | --- |
| `--cover` | **function**-level — which of the project's functions the suite never *entered* | no kernel support; hot reload is the seam |
| `--cover-lines` | **line**-level — which executable lines actually ran | instruments the bytecode and turns the JIT off |
| `--cover-branches` | **branch**-level — did each `if`/`cond`/`match` test take *both* edges | the same bytecode seam as `--cover-lines` |

Any combination may be on at once; they answer different questions. `--cover-min`
gates on the strictest percentage present — **branch > line > function**. None of
them is a timing run.

An "executable line" is one carrying an instrumented node (a call or an inlined prim),
so a literal-bodied function has no measurable lines and is left out of the report
rather than counted as 0%. See [`coverage.md`](coverage.md) for what each tier
measures, why the line denominator comes from the compiler rather than from reading the
source (two earlier versions produced confidently wrong percentages), and why the
function shim is variadic.

## Running

In a project, run the whole suite with **`nest test`** (or `make suite`, or
`cargo test`): the runner discovers `tests/**/*_test.blsp`, loads them, and calls
`run-tests` once (`nest test` passes `:trace`). `run-tests` itself takes the flags
below — forwarded by the runner, and usable directly if you call it yourself:

```lisp
(run-tests)            ; run all, print failures + a summary
(run-tests :trace)     ; print `▶ group › name` as each test STARTS (else: progress dots)
(run-tests :slow)      ; after the summary, list the slowest 5 tests
(run-tests :slowest 10)     ; ...or the slowest N
(run-tests :timeout 5000)   ; per-test HARD ceiling in ms — killed+failed (default 120000)
(run-tests :slow-over 200)  ; list any test over this many ms (informational; default 1000)
(run-tests :max-failures 1) ; abandon the run once this many tests have failed
(run-tests :repeat 50)      ; run the suite up to 50 times, stop at the first failure
(run-tests :filter SPEC)    ; a selection spec — see `test--make-filter`
(run-tests :trace :slow)
```

The `nest test` flags map onto these one-for-one: `--max-failures` → `:max-failures`,
`--repeat-until-failure` → `:repeat`, `--slowest` → `:slowest`, `--timeout` →
`:timeout`, `--trace` sets `:trace` (opt-in — see below), and the selection flags are lowered
into one `:filter` spec built by `test--make-filter` (selector *parsing* lives in
Brood, so the grammar has a single definition).

**`--max-failures` granularity.** The budget is checked between scheduling steps
and between files, not mid-test, so a run can overshoot the cap slightly — the
batch already in flight still reports. It bounds the run; it isn't an exact stop.

`nest test` shows progress dots by default and `▶ group › name` under `--trace`; the
`brood --test` path and `run-tests-structured` stay quiet either way, for clean
machine-parseable output.

**Two thresholds, distinct on purpose** (both per-test wall-clock ms; module vars,
overridable as above):

| knob | default | effect |
| --- | --- | --- |
| `*test-slow-ms*` | 1000 | a test over this is **listed** (name + duration) under the summary — still **passes**; informational only |
| `*test-timeout-ms*` | 120000 | a test over this is **hard-killed** (`(exit worker :kill)`) and **fails** as `test timed out after Ns` |

So a test between the two (1s ≤ t < 120s) is listed but passes; at/over the timeout it's killed and fails. Keep slow < timeout.

**Per-test timeout.** Every test has a wall-clock budget — **120 s by default**. A
test that exceeds it is **hard-killed** (`(exit worker :kill)`, ADR-063 — so even a
tight infinite loop is stopped, not just a `receive`-blocked one) and reported as a
`test timed out after Ns` failure, rather than dragging or wedging the whole suite.
Override per run with `:timeout MS`, or globally by rebinding `*test-timeout-ms*`.
Tests in a batch start together, so the budget is effectively per-test. Any test
over **1 s** (passing or not) is also listed automatically under the summary.

### Reading a failure

Each failed assertion gets a block: a **`file:line:col:` anchor** (bold, and relative
to the working directory so it stays short and stays clickable in
compilation-mode/flymake), then its fields indented with the labels dimmed so the
*values* are what you read. Blocks are separated by a blank line.

```
tests/mix_test.blsp:5:28: test failed: arithmetic › divides
    assert: (assert= (/ 7 2) 3.6)
    actual: 3.5
    expect: 3.6
```

When an assertion carries no recorded position — a body form the reader recorded no
position for — the anchor falls back to **the test's own declaration site**, so there
is always somewhere to jump to.

A test **file that fails to load** (you're mid-edit and it doesn't compile) is
reported as one located failure of its own rather than aborting the run, so the rest
of the suite still reports and you still see exactly what broke:

```
tests/oops_test.blsp:2:1: test failed: tests/oops_test.blsp › failed to load
    cannot load: unbound symbol: this-is-not-defined
```

`nest test` exits non-zero on any failure, with the summary as the last thing printed
— a failing suite is an expected outcome, not an internal error, so it does not append
a Brood stack trace to the report.

`run-tests` prints progress, then any failures (one block per failed assertion,
attributed to its test), then a summary:

```
158 tests, 158 passed, 0 failed (0 failed assertions, 2 isolated)
  test runtime: 1832 ms total — parallel/serial 1831 ms, isolated 1 ms
  (797 ms wall, peak 70.8 MB)
  141 processes (1 runner + 139 unit workers + 1 nested) on 28 worker threads, peak 28 running at once
```

The last line reflects the **green M:N** process model (step 4b in
[`concurrency.md`](concurrency.md)): processes are cheap captured continuations
(plain heap data) multiplexed onto a fixed pool of ≈`nproc` worker threads —
*not* one OS thread each.
"processes" is the total spawned over the run; "running at once" is the
high-water mark, bounded by the pool. `run-tests` raises if anything failed, so
the process exits non-zero — which is how `cargo test` notices.

See `tests/suite_test.blsp` (and the other `tests/*_test.blsp` files) for the real
suite, and `tests/suite_failures_test_ignore.blsp` for a deliberately-failing file you can run
by hand (`brood tests/suite_failures_test_ignore.blsp`) to see the failure report.

## External conformance corpora

Everything above tests Brood against cases *we thought of*. The corpora under
`tests/corpus/` test it against cases other language implementers already paid for
in production bugs — the numeral strings that broke shipped `strtod`s, the Unicode
break tables, the regex semantics files. They are ordinary Brood tests; the only
difference is that the expectations are vendored rather than written.

```
tests/corpus/<suite>/data/     the committed subset — small enough to read
tests/corpus/<suite>/full/     the complete upstream, gitignored, --full only
tests/corpus/<suite>/README.md the upstream URL, pinned commit, licence
tests/conformance_<suite>_test.blsp   the runner
tests/support/corpus.blsp             the shared locate/read helper
```

```bash
scripts/fetch-corpus.sh                  # refresh every vendored suite
scripts/fetch-corpus.sh parse-number     # refresh one
scripts/fetch-corpus.sh --full parse-number   # also pull the full upstream
nest test --only conformance             # run just the conformance runners
```

A runner calls `(corpus-files "<suite>")`, which returns `full/` when it has been
fetched and `data/` otherwise — so the same test is a fast gate in CI and an
exhaustive sweep on a machine that ran `--full`, with no code change. Runners carry
`:tags [:conformance]` (plus `:slow` when they take more than a second), so
`--exclude slow` still gives a quick suite.

Three rules for adding one:

- **Pin the upstream.** The suite README records the URL, the commit, and the
  licence. Never vendor GPL data (`ansi-test`, for one) — mine it for ideas instead.
- **Subsample deterministically.** `fetch-corpus.sh` takes every Nth line, never a
  random draw, so re-running reproduces the committed bytes exactly.
- **Assert the corpus is non-empty.** A sweep over a corpus that failed to fetch
  passes vacuously; every runner has a "the corpus is present" test guarding a case
  count, so a truncated fetch fails loudly instead of going quiet.

The full inventory — which suites are wired and which are still ahead — is the
"External conformance corpora" section of [`ROADMAP.md`](../ROADMAP.md).

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
