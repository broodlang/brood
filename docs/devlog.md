# Dev log

Chronological record of work sessions. Newest at the bottom.

---

## 2026-05-27 — Project bootstrap and v0.1 language core

**Goal.** Stand up a new dynamic Lisp in Rust whose purpose is to be the
language a modern, Emacs-like, remotely-hostable, self-editing editor is written
in. First concrete target from the user: *"a light first version where
`(+ 1 2)` works."*

**Decisions taken** (full rationale in [decisions.md](decisions.md)):
- Host language: **Rust** (ADR-001), reaffirmed against C/Zig given the
  "heavily vibe-coded" constraint — memory safety is the key guardrail.
- Memory: `Rc`/`RefCell` now, tracing GC later (ADR-002).
- Cons-cell lists + separate `[ ]` vectors (ADR-003).
- Clojure-style truthiness; flat `cond` (ADR-004).
- Zero external dependencies for v0.1 (ADR-005).
- Maximise the share of the language written in Brood itself (ADR-006).

**Built.**
- Cargo workspace: `crates/lisp` (the language) + `crates/cli` (the `brood`
  binary), `std/prelude.lisp`, `docs/`.
- `value.rs`: `Value` enum, thread-local symbol interner, list/vector
  constructors, structural `PartialEq`.
- `reader.rs`: recursive-descent parser (numbers, strings, symbols, keywords,
  lists, vectors, `'` quote, `;` comments).
- `env.rs`: lexical environment chain with `get`/`set_existing`/`define`/`root`.
- `eval.rs`: tree-walking evaluator built as a `'tail: loop` for **proper tail
  calls**; special forms `quote if when unless cond do def set! fn/lambda
  let/let* and or while`.
- `builtins.rs`: arithmetic, comparison/logic, list & sequence ops, higher-order
  (`map`/`filter`/`reduce`/`apply`), predicates, strings/IO, and the
  self-hosting trio `eval`/`read-string`/`load`.
- `printer.rs`: readable + display rendering that round-trips with the reader.
- `cli/main.rs`: dependency-free REPL (balanced-delimiter multiline input) and
  file runner.
- `tests/basic.rs`: 16 end-to-end tests.

**Verified.**
- `cargo build` clean; `cargo test` → 16/16 plus the lib doc-test green.
- The headline guarantee holds: a tail-recursive sum to **1,000,000** returns
  without overflowing the stack.
- Live redefinition works: redefining a global function changes behaviour
  immediately (the seed of "edit the editor on the fly").
- REPL: `(+ 1 2)` → `3`. File runner: a recursive `fib` program prints
  correctly.

**Notable bug found & fixed.** First test run: `cond` returned a wrong branch.
Root cause was a mismatch between an initial Scheme-style clause-list `cond` and
the intended flat Clojure-style pairs. Switched the implementation to flat
`test expr` pairs with `else`/`:else` fallback (ADR-004); all green after.

**Status.** v0.1 = the ✅ slice of M1 in [roadmap.md](roadmap.md). Next up to
finish M1: macros + quasiquote, dynamic variables, in-language error handling,
maps, and the GC migration.

### Follow-ups (same day)

- **`bin/cli`** launcher script added (builds + runs the CLI from any directory).
- **REPL line editing.** Added `rustyline` (first external dependency, sanctioned
  by ADR-005) so the interactive REPL has arrow-key editing, history
  (`~/.Brood_history`), and Emacs-style bindings. Multi-line forms are handled
  by accumulating lines until delimiters balance. Non-terminal stdin (pipes,
  scripts) falls back to a plain reader that prints results only.
- **Principle reinforced by the user:** *as much of the language/system as
  possible must be written in Brood itself* (Rust = mechanism, Brood =
  policy), and the CLI/REPL should eventually be self-hosted in Brood. Captured
  prominently in `CLAUDE.md` and `docs/roadmap.md` (extends ADR-006). The
  current Rust REPL is an explicit bootstrap, not the end state.

### Primitive-kernel refactor + spec (same day)

- **Shrank the Rust builtins to a primitive kernel** and moved the user-facing
  functions into `std/prelude.lisp`: `+ - * /` (variadic, over 2-arg `%add`…),
  `< <= > >= = not=` (over `%lt`/`%eq`), `not number? list? car cdr list map
  filter reduce fold reverse append count nth …` are now ordinary Brood `def`s.
  Rust keeps only `%`-numeric ops, `cons/first/rest/empty?`, vector/string
  primitives, type-tag predicates, I/O, and `eval/read-string/load/apply`.
  Recorded as ADR-008.
- **Cost & adjustment:** Brood arithmetic is ~10× slower than the old native
  loop. The tail-recursion test ran ~50s at 1,000,000 iterations, so it was
  right-sized to **100,000** (still proves O(1) stack; suite back to ~5.5s).
  Tradeoff noted as reversible (future specialiser / re-promotion of hot ops).
- **Answered two design questions from the user and wrote them down:**
  - Brood is a **Lisp-1** (single namespace) — ADR-007. This is what makes the
    refactor above possible (`+` is just a value).
  - Added a **formal spec**, `docs/spec.md` (EBNF lexical/reader grammar, data
    model, evaluation + tail-position rules, scoping, special forms, the
    kernel/derived split, errors, and what's deliberately unspecified).
- Removed a now-unused `truthy` import; build is warning-clean. Tests: 16/16
  green.

### Macros + `defn` (same day)

- User asked for function definitions next, and chose the principled route:
  build macros, then define `defn` in Brood (rather than a quick Rust special
  form).
- **Added the macro system:** a `Value::Macro`, the `defmacro` and `quasiquote`
  special forms, macro expansion wired into the evaluator's `'tail` loop
  (macros resolve after special forms, before application, and expansions are
  re-looped so they get TCO + further expansion), the `macros.rs` module
  (quasiquote expansion, `macroexpand`/`macroexpand-1`), and `gensym`.
- **Quasiquote syntax decided (ADR-009):** Clojure-style `` ` `` / `~` / `~@`,
  and `,` is now whitespace. This matched the preview the user approved and the
  rest of the Clojure-leaning surface. Reader updated accordingly.
- **`defn` is now a macro in `std/prelude.lisp`**, and the whole prelude was
  rewritten to define its functions with `defn` (dogfooding). Also added `->`
  and `->>` threading macros, whose bodies compute the expansion with `reduce`
  at expansion time — a nice demonstration that macros are just Brood.
- Tests: added `defn`, user-macro/quasiquote, and threading cases. 19/19 green
  (~6.8s). REPL spot-checks: recursive `fib`, a custom `unless2` macro, and
  `->` all behave.
- Docs updated across the board: `spec.md` (§3 reader grammar, §7.2/7.3
  quasiquote + macros, §9 kernel list, §11), `language.md` (Macros section +
  builtins), `roadmap.md` (macros/quasiquote ticked), `decisions.md` (ADR-009),
  `README.md`.

### Lists for code, vectors for data; the parameter grammar (same day)

Prompted by the user's "all are lists" observation, a design conversation about
parameter lists and the broader role of vectors:

- **ADR-010 — code is cons-lists; vectors are a data type.** Reverses ADR-003's
  "vectors as the parameter surface." Parameter lists and `let` bindings are now
  written as lists — `(defn f (x y) …)`, `(let (a 1 b 2) …)` — for homoiconic
  code (which matters for a self-editing editor). `[ ]` vectors stay as a
  first-class *data* type (O(1) indexing) and are still accepted in
  param/binding positions. The prelude was rewritten entirely in list form.
- **ADR-011 — favor the simplest user-facing design; defer power features.** We
  designed the full CL-grade parameter grammar (`&optional`/`&key`/required-keys
  /supplied-p) and then cut it to **`required` + `&optional` (with defaults) +
  `& rest`**. `&key`, supplied-p, and required-keyword markers are deferred
  (designed, additive later). Recorded the principle in `CLAUDE.md` too.
- **Defined the grammar** formally in spec §7.4 (EBNF + binding/arity rules).
- **Implemented `&optional`** in the closure calling convention
  (`parse_params`/`bind_params`), so it works uniformly in `fn`, `lambda`, and
  `defn` — chosen over `defn`-macro sugar to avoid a footgun (raw `fn` silently
  treating `&optional` as a parameter name) and bootstrapping complexity.
  Defaults are evaluated lazily, left-to-right, so a default can reference an
  earlier parameter. Unknown `&markers` are rejected.
- Tests: added list-param, list-`let`, and `&optional` cases — 22/22 green.
- Docs synced: `spec.md` §7.4 + §4, `language.md` (Parameter lists section,
  examples to list form), `README.md`, `roadmap.md`, `decisions.md`
  (ADR-010/011), `CLAUDE.md`.

### Error handling (same day)

- First wrote a kernel inventory (`docs/primitives.md`) at the user's request,
  then added error handling under "the language must be as small as possible":
  **+2 primitives, 0 new special forms.**
- Kernel: `throw` (raise a value) and `%try` (call a thunk; on raise call a
  handler). `LispError` gained a `payload: Option<Value>` and a `User` kind.
- Prelude (Brood): `error` (raise a formatted message), and `try`/`catch` as a
  **macro** desugaring to `(%try (fn () body) (fn (e) handler))`; plus `last`
  and `but-last` helpers it needs.
- `catch` binds the thrown value, or — for built-in errors — the error's message
  string (the simplest choice, ADR-011; structured errors can come with maps).
- Tests: throw/catch, no-throw passthrough, built-in error caught as string,
  `error`, no-catch `try`, uncaught propagation. 24/24 green.
- Docs: `primitives.md` (proposal → implemented), `spec.md` §9/§10/§11,
  `language.md` (Errors section), `roadmap.md`.

### Test runner: progress dots + timing (same day)

- User asked the test framework to show progress (`.`s) and report how long a
  run took. Kept the kernel growth minimal: **+1 primitive, the rest in Brood.**
- Kernel: `now` — wall-clock milliseconds since the Unix epoch as an integer
  (`SystemTime`). Reading the clock genuinely needs Rust; elapsed time is then
  just a subtraction in Brood. 45 primitives now.
- `std/test.lisp` rewritten: assertions (`is`/`assert=`/`assert-error`) now
  *record* a pass or a failure instead of printing inline. `run-each` prints one
  `.` per test (`F` if it recorded a failure) for a live progress line; failure
  details collect in `*failures*` and print afterwards under `FAILURES:`; the
  summary reports `N tests, M assertions, K failed (T ms)`.
- The timer immediately earned its keep: it surfaced that the suite takes ~5.7s
  in the debug build, dominated by the `sum-to 100000` tail-call test.

- Follow-up, same ask: also report **memory**. Added a byte-counting
  `#[global_allocator]` (`crates/lisp/src/alloc.rs`) wrapping the system
  allocator with two relaxed atomics (live bytes + peak), declared in `lib.rs`
  so the CLI and the test binaries share it. Two primitives read it: `mem-bytes`
  (live) and `mem-peak` (high-water). 47 primitives now. Dependency-free (std
  `alloc` only), per ADR-005 (ADR-012).
- The runner now prints `… failed` then an indented `(T ms, peak X.X MB)` line;
  MB formatting (round to 0.1) is done in Brood with a small `quot` helper since
  `/` already lands on an int when the division is exact.
- This *also* earned its keep instantly: peak is ~300 MB for the suite, because
  there is **no GC yet** (ADR-002) — `sum-to 100000` retains ~317 MB of envs/
  conses that are never reclaimed mid-run (so `mem-bytes` ≈ `mem-peak`). Live
  motivation for the tracing-GC migration; the two readings will diverge once it
  lands. CPU time was assessed and deferred (wall-clock already covers it; true
  user+sys needs `/proc` parsing or `libc`).
- Docs: `primitives.md` (Time + Memory categories, count 44→47), `language.md`
  (Time & memory section), `decisions.md` (ADR-012).

### Test runner: per-test tracing + slow-test report (same day)

- User asked for two opt-in flags on the runner: trace each test (name + time)
  and surface slow tests. Done **entirely in Brood** — no new primitives;
  `(now)` already gives ms and `count` already measures strings.
- `run-tests` now takes flags: `(run-tests :trace)` prints one line per test
  (`.`/`F` marker, padded name, right-aligned ms) instead of the dot line;
  `(run-tests :slow)` prints the slowest tests after the summary; both compose.
  Times are recorded into `*timings*` every run, so `:slow` works on its own.
- Supporting Brood added to `std/test.lisp`: `opt?` (flag lookup), `spaces`/
  `pad-right`/`pad-left` (column formatting), `take`, and a tiny `insert-by-ms`/
  `sort-by-ms` insertion sort (O(n²) is fine for a handful of tests). Default
  output — dots + summary — is byte-for-byte unchanged when no flags are passed.
- Enabled both flags on `tests/suite.lisp` per the user's "add these two flags
  for now". The Rust harness (`crates/lisp/tests/suite.rs`) only checks for a
  raised error, so the extra output is harmless; `cargo test` stays green.
- Immediately useful: trace shows `tail-calls` (`sum-to 100000`) is ~6.1s of the
  ~6.1s run — the whole suite's cost is that one test, reinforcing the GC
  motivation already noted above.

### Concurrent test runner + concurrency cap (same day)

- User: "get very fancy" — run tests concurrently with a live ASCII view; the
  view should be governed by the existing `:trace` flag (bar when off, a live
  per-test dashboard when on). Built on the share-nothing process model.
- `deftest` now registers `(name thunk worker)`: `thunk` is the old in-process
  body; `worker` is a `(fn (parent) …)` that, when spawned, `(require 'test)`s
  the framework in its fresh child interpreter, runs the body (wrapped in
  `try`/`catch` so an uncaught error becomes a failure instead of a hung runner),
  and ships `(name passed failed ms failures)` back. `(run-tests :parallel)`
  spawns one worker per test and aggregates results as they arrive.
- Display keys off `:trace`: `:parallel` alone redraws a single `\r` progress
  bar; `:parallel :trace` paints a multi-line dashboard (one line per test,
  `●` running → `✓`/`✗` with time) that redraws in place as each worker reports.
- New Rust (mechanism only): reader gained a `\e` ESC string escape (one line)
  for ANSI; `process.rs` gained a `SPAWNED` counter behind `(spawn-count)` and a
  `Gate` (Mutex+Condvar) capping concurrent spawned threads, set by the CLI's
  `-j N` / `--max-parallel N`, observable via `(peak-threads)`. 47→49 primitives.
- The cap is a stopgap, not step 4b: threads are still one-per-spawn (born when a
  permit frees), so it bounds *peak concurrency*, not total threads. Because
  `receive` blocks its OS thread, the cap must exceed the depth of processes
  blocked waiting on a not-yet-running one — `-j 1` deadlocks the suite (the
  `processes` test spawns a child and waits), `-j 2` is the safe floor. This is
  exactly the motivation for step 4b's coroutine suspension at `receive`.
- The runner reports it: `16 processes / 16 OS threads (1 runner + 14 test
  workers + 1 nested), peak N running at once`. `tests/suite.lisp` now runs
  `:parallel :trace :slow`; `cargo test` stays green (the harness only checks for
  a raised error). Parallel doesn't speed *this* suite up — `tail-calls`
  dominates (~6s) and the rest are ~0, so wall time is that one long pole plus
  thread overhead.

### Shared mutable runtime: cross-process hot reload (same day)

**Goal.** A long-running *spawned* process (think: a web server) must pick up a
redefinition **without being restarted** — while separate runtimes/nodes stay
independent. This reverses the earlier "instances are independent / no shared
mutable global" decision, which was mis-scoped: it conflated *inner processes*
(which should share live code) with *separate runtimes* (which shouldn't). The
model is Erlang's code server; Brood, being a late-binding Lisp-1, re-dispatches
on *every* call, so a shared live global gives hot reload for free (ADR-013).

**Built (Rust — mechanism).**
- **Three heap regions via a 2-bit handle tag** (`value.rs`): `LOCAL` (per-process
  data), `PRELUDE` (immutable, shared by all runtimes), `RUNTIME` (mutable,
  per-runtime, shared by a runtime's inner processes). Replaces the old 1-bit
  local/shared split.
- **`RuntimeCode`** (`heap.rs`): append-only code slabs backed by **`boxcar`** (a
  crate — lock-free reads return stable refs that survive concurrent pushes, so
  process threads read closure bodies lock-free while a `def` appends) + a
  `RwLock<HashMap>` global bindings table. The global scope became a sentinel
  (`EnvId::GLOBAL`) routing there; `def`/`set!` **promote** a value's reachable
  code from LOCAL into RUNTIME before rebinding. Append-only ⇒ in-flight calls
  finish on the old closure; new lookups get the new one.
- **`promote_env`**: a closure defined *inside a function call* closes over a
  local scope; promoting it for sharing copies that scope into RUNTIME too, so a
  shared closure resolves its free variables in any process. (Without this, the
  test suite panicked — `env = Some(LOCAL …)` dereferenced a frame that didn't
  exist in the child.)
- **`spawn`** now clones the parent's `Arc<RuntimeCode>` (shared live code) and
  `promote`s the target; args still ship as `Message`s (data is per-process). The
  old `ship_closure`/`install_closure` are gone. `Gate`/`spawn-count`/`peak-threads`
  preserved.
- **Crate policy relaxed** (ADR-014; CLAUDE.md): runtime crates allowed when they
  cut real complexity — `boxcar` removes a hand-rolled `unsafe`. Lisp-callable
  behaviour still lives in Brood.

**Verified.** New Rust test `spawned_process_picks_up_redefinition`: a spawned
request/reply server returns `50` (handler = `* 10`), the handler is redefined to
`+ 100`, and the *same running server* returns `105` on the next request — no
restart. 26 Rust tests + suite + doc-test green.

### Test framework: share-safe + ExUnit `describe`/`test` (same day)

**Why.** Sharing the global table broke the concurrent test runner, which had
each worker tally into shared mutable globals — they raced, miscounted, and hit
the captured-env panic above. (The earlier `suite-failures.lisp` note had already
diagnosed the result-mixing.)

**Reworked `std/test.lisp`** (ADR-015, `docs/testing.md`):
- **Share-safe tallying.** Assertions are now *macros* that push onto a
  process-local `*fails*` (a `let` the `test` macro establishes); each test yields
  its failures as a value; the runner aggregates from returns/messages into its
  own local state. No shared counters.
- **ExUnit / `mix test` surface.** `describe` groups, `test "name"` cases
  (`deftest` kept as an alias), `is`/`assert=`/`assert-error`/`error-of`.
- **Parallel by default**, with opt-in serialisation: `:serial` (a group's tests
  run one worker, in sequence, alongside other groups) and `:isolated` (runs
  alone, in an exclusive phase after the parallel batch) — for tests that touch
  shared global state. Registration builds *units*; the runner runs a parallel
  phase then an isolated phase.
- `tests/suite.lisp` converted to `describe`/`test` (40 tests; "macros" is
  `:serial`, "processes" is `:isolated`); `suite-failures.lisp` likewise. The
  parallel failure path now attributes failures to the right test with correct
  counts.

### Documented the Clojure-divergence gotchas (same day)

**Why.** A review of the syntax through the lens of "will an LLM like Claude
find this easy to write?" found the language is ~80% Clojure on the surface,
which is good — most Clojure reflexes transfer. But a few core forms borrow from
Scheme / Common Lisp in exactly the spots where a Clojure habit yields
valid-*looking* code that fails silently or with a misleading error. Verified
against `./bin/cli`:

- `(try … (catch Type e body))` — Clojure's class-typed catch binds the *class
  name* as the variable and treats `e` as body → cryptic `unbound symbol: e`.
  Brood's clause is a bare `(catch e body)`.
- Multi-arity `(fn ([x] …) ([x y] …))` → `type error: expected a symbol`. Brood
  uses `&optional` / `&` in one parameter list instead.
- `{:a 1}` → `parse error: map literals '{ }' are not supported yet` (a *good*,
  teaching error — the model that hits it learns the feature is absent).
- Clojure-style `[x y]` params and `[a 1 b 2]` let-bindings *do* parse (vectors
  are accepted in binding position) but lists are idiomatic.
- `/` has no ratios: integer args divide to an integer only when even, else a
  float (`(/ 7 2)` → `3.5`).

**Documented** a "Coming from Clojure (the differences that bite)" table near
the top of `docs/language.md` — leading with the deltas, since an LLM (and a
human Clojurist) reads what's *different* far more reliably than the full spec.

**Candidate fixes, not yet done** (recorded here so they're not lost): the
`catch` case is the highest-value — detect a multi-symbol catch head and either
accept-and-ignore the type or raise a clear "`catch` takes one binding" parse
error; likewise give multi-arity `fn` a teaching error pointing at
`&optional`/`&`, matching the quality of the map-literal message.

### Memory reclamation, step 1: arena reset at top-level boundaries (same day)

**Goal.** Stop long-lived processes (the REPL, eventually a server) from leaking
every cons/env they allocate. (Spawned processes already free their whole `Heap`
on thread exit, so the leak is specifically the long-lived ones.)

**The wall, and the way around it.** A real tracing GC needs to find live roots,
but `eval` is a native recursive tree-walker — live `Value`s sit on the *Rust*
stack, unscannable. A mark-sweep rooted only from the current env is unsafe
mid-eval (sibling sub-expressions strand live values in local `argv`s). BUT a
property of the shared-runtime design saves the common case: **globals live in
PRELUDE/RUNTIME and never point into a process's LOCAL heap** (a top-level `def`
promotes outward). So at a top-level boundary the only live LOCAL value is the
form's result.

**Built (ADR-016, memory-model.md).** `Heap::checkpoint()` snapshots LOCAL slab
lengths; `Heap::reset_local_to(cp)` truncates them back. `eval_str` resets
between top-level forms (keeping the final result); the REPL (both interactive
and plain) resets to a baseline after each command, once the result is printed.
O(1), no tracing, nothing shared touched. Demo: a file of five heavy `sum-to`
forms went from ~712 MB growing to **~78 MB flat** (peak 159 MB during one form).

**Scope / deferred.** Doesn't help a single never-returning loop (no top-level
boundary), and reset is unsafe mid-eval. General GC needs the evaluator's roots
scannable — the explicit-value-stack VM that step 4b also needs — so GC and 4b
are coupled and best done together. `gc-arena` is no longer the presumed path
(poor fit for native recursive eval + the shared multi-thread RUNTIME region).

**Verified.** 26 Rust tests + suite + doc-test green; warning-clean. (A subtle
gotcha while testing: `cargo test` doesn't refresh `target/debug/brood`, so the
first manual demo ran a stale binary and looked like it leaked — `cargo build`
then showed the flat profile.)

### Benchmark harness (divan) + a Makefile (same day)

First reproducible performance baseline. Added a `divan` (0.1) dev-dependency and
`crates/lisp/benches/eval.rs`: `interp_new` (per-instance startup), `parse_prelude`
(reader only), `sum_tail` (the tail-call loop), and `fib` (non-tail recursion),
the eval ones building a fresh `Interp` per iteration via `with_inputs` so the
once-per-process prelude build stays out of the measured region. `scripts/bench.sh`
runs them and archives each run to `docs/benchmarks/<UTC>.md` with full environment
metadata (arch, CPU, toolchain, divan version, git commit + dirty flag) — numbers
are only meaningful next to the machine and commit they came from. A `Makefile`
wraps the common Cargo commands (`make benchmark`, `test`, `suite`, `repl`, …).

**Baseline (i7-14700HX, commit 1bf54c9):** `interp_new` ~1.5 µs, `parse_prelude`
~50 µs, `sum_tail` 1k/10k/100k ~8/82/845 ms, `fib` 15/20/25 ~14/155/1772 ms. The
loops are slow on purpose — arithmetic is Brood, not native (ADR-008). (A stale
bench binary from an earlier WIP state spammed a `[reset]` debug print through the
first archived run; a clean rebuild of the current HEAD source confirmed it gone.)

### Isolated tests roll back the globals (ADR-017) (same day)

`:isolated` went from *scheduled-alone* to *state-isolated*. New `%isolate`
primitive: snapshot the runtime's global table (`Heap::snapshot_globals`), run a
thunk, restore (`Heap::restore_globals`) — so a `def`/`set!` inside is rolled back.
The test framework now runs the isolated phase **first**, each test through
`%isolate`, so an isolated test sees the clean post-load baseline and its defs
can't leak to another test. Couldn't do a true fresh runtime per test: a thunk's
closure handle is region-tagged to its runtime (ADR-013), so it can't run in
another — isolating *bindings* is the proportionate fix (rationale in ADR-017).

**Verified.** 27 Rust tests (new: `isolate_rolls_back_global_defs`) + suite
(41 tests, 2 isolated, the isolated phase running first) + doc-test green.

### Step 4b — green M:N scheduler via stackful coroutines (same day)

**Goal.** Replace OS-thread-per-process (4a) with cheap green processes on a fixed
worker pool, where `receive` *suspends* instead of blocking — so spawn scales,
OS threads stay ≈`nproc`, and the `Gate` deadlock disappears.

**Approach (ADR-018, `docs/scheduler.md`).** Path A — **`corosensei` stackful
coroutines**: each process runs in a coroutine with its own parkable stack, so the
native recursive `eval` is untouched. A pool of ≈`nproc` worker threads pulls
ready processes off a shared run queue; `receive` on an empty mailbox suspends the
coroutine and the worker parks it; `send` re-queues it. (Path B — an explicit-VM
rewrite — was declined; only precise GC needs it.)

**Mechanics.**
- `receive`/`self`, deep in `eval`, find their process via a thread-local `Ctx`
  the coroutine sets at start and **re-establishes after every suspend** (so it
  survives the worker multiplexing other processes, and migration to another
  worker — corosensei supports cross-thread resume).
- The "check empty → park" vs "deliver → wake" race is closed under one mailbox
  mutex: a worker, seeing a `Yield`, re-checks the queue before parking; `send`
  takes the parked process and re-queues it, else notifies the root's condvar.
- The **root** thread (REPL / file runner) isn't a coroutine — its `receive`
  *blocks* on its mailbox; only spawned processes yield.
- corosensei marks `Coroutine` `!Send`; we `unsafe impl Send for Process` (the run
  queue owns a process exclusively; corosensei allows cross-thread resume; captured
  state is `Send`). A panic in a process is caught so the worker survives.
- Pool size is a **setting** (default `nproc`, `-j` overrides) — never hardcoded.
  New `(worker-threads)`; `(spawn-count)`/`(peak-threads)` reworded (green
  processes on a pool, not one OS thread each). The test summary now reads e.g.
  *"39 processes (1 runner + 37 unit workers + 1 nested) on 28 worker threads,
  peak 28 running at once."*

**Deferred (optimisation, per "get it working first").** Work-stealing (one shared
run queue today) and reduction-counted preemption (cooperative today: a CPU-bound
process with no `receive` holds its worker until done — bounded by the pool). Both
are additive, per the BEAM comparison in `scheduler.md`.

**Verified.** 27 Rust tests + the suite (now on green processes, 28 worker threads,
0.76 s) + doc-test green; no hang/deadlock; build warning-clean.

### Test output built for legibility — source forms + plain-when-captured (same day)

Goal from the user: *an LLM (or anyone reading a captured run) must see test issues
instantly.* Two changes to the in-language framework (`std/test.lisp`):

- **Failures name the source expression.** `is`/`assert=`/`assert-error` quote
  their operands at macro-expansion time, so a failure reads `(= 1 2) is false`,
  `(+ 2 2) => 4, expected 5`, or `expected (+ 1 2) to raise, but none did` —
  self-identifying, instead of three identical `is: expected truthy, got false`
  lines you couldn't tell apart. Added `refute` (assert-falsy) as the negation of
  `is`. Normalised the suite to `assert=` for equality, `is`/`refute` for
  predicates.
- **Colour only on a TTY.** New `stdout-tty?` primitive (Rust `IsTerminal`); the
  `ansi` helper returns plain text when stdout is captured (pipe, `cargo test`,
  CI, an LLM), so the report is never littered with `\e[..m` escape codes. Colour
  still shows for an interactive human.

Updated `tests/suite-failures.lisp` (the runnable failure-rendering demo, now also
exercising `refute`), `docs/testing.md`, and `docs/primitives.md`.

**Verified.** Piped run has zero ANSI escapes; the demo renders every failure
kind with its expression; 27 Rust tests + suite + doc-test green.

### Modules, project file, and a project test runner — design (same day)

**Goal.** Start on project tooling: a way to `require` Brood code by capability
(not just the one embedded `test` module), a project manifest, and a tool that
finds and runs a project's whole test suite. Settled the two design forks with the
user before writing any code.

**Decided** (rationale in ADR-019 / ADR-020):
- **Modules: Emacs-flat, not namespaced** (ADR-019). `provide` / `require` track
  loaded `*features*`; `*load-path*` is searched for `name.lisp`; everything loads
  into the one shared global table (ADR-013). `foo--private` is the only
  "interface" signal, by convention. The only new Rust is fs reflection
  (`file-exists?`, `list-dir`, `cwd`); the require/provide logic is Brood
  (ADR-006/008). Chosen over first-class namespaces because the latter would expand
  the core across value / reader / eval / global-table / hot-reload — and because a
  flat, openly-redefinable namespace is the *desired* semantics for a self-editing
  Emacs-like editor. Namespaces stay available later as a pure-Brood macro layer
  (prefix names in the flat table), so this forecloses nothing.
- **Project file: `project.lisp`, not inert data** (ADR-020) — convention over
  configuration. `src/` is the project source (auto on `*load-path*`), `tests/`
  holds the tests; `project.lisp` mainly declares identity
  (`(project :name … :version …)`) and overrides paths only when a project
  deviates. Source not data, so it reads as data yet can compute config. Project
  root = nearest ancestor with `project.lisp`.
- **Test tool.** `brood test` discovers `tests/**/*_test.lisp`, loads each
  (register-only — they no longer call `run-tests`), and runs the suite once via
  the existing framework, which already splits registration from execution
  (ADR-015), so discovery drops in cleanly.

**Status.** Design captured (ADR-019/020, both roadmaps, this entry); implementation
is the next step — the fs primitives, the Brood `require`/`provide` + load-path +
project loader, the `brood test` CLI subcommand, and migrating `tests/suite.lisp`
into `*_test.lisp` files.

---

## 2026-05-27 — Pattern matching + a macroexpand-all compile pass

**Goal.** Implement the pattern matching designed in
[pattern-matching.md](pattern-matching.md): Erlang/Elixir-style matching with one
pattern grammar reused at every binding site. Subsumes the Tier-2 "destructuring
in `let`/`fn`" and "`case`" items.

**Built (all the matcher logic is Brood, in `std/prelude.lisp`):**
- A pattern→code compiler + `match*`/`match` macros — full grammar (`_`, binds,
  literals, `'sym`, pins `~x`, list `(p & rest)`, vector tuples `[p …]`, nesting),
  guards (`:when`), **non-linear** patterns (a repeated var is an equality check),
  structured catchable failure `[:match-error ctx value patterns]`, and
  compile-time checks (malformed `&`, unreachable clause, bad `:when`).
- Refutable/destructuring `let`, multi-clause `fn` (Erlang dispatch), and `fn`
  pattern parameters. `defn` is now a pure forwarder to `fn`.

**The performance detour (ADR-022).** The evaluator expands macros lazily, so a
`match` in a function body re-ran the whole Brood compiler *every call* — ~1 ms/iter,
~25× a plain `if` (TCO-safe, just slow). Fixed with a **compile pass**:
`macros::macroexpand_all` fully expands every macro call once at each top-level /
definition boundary (`eval_str`, `load`, `require`, `eval`, prelude loader),
form-by-form; the evaluator keeps lazy expansion as a fallback (covers a macro
defined and used within one form). The pass is also where `let`/`fn` pattern
binders are **lowered** to `match*`, so eval's `let`/`fn` stay symbol-only and the
matcher logic stays in Brood. After: `match` loops run at plain-`if` speed.

**Decisions.** ADR-021 (pattern matching: one Brood compiler, every site) and
ADR-022 (the compile pass). Two refinements vs. the design prose: the `fn`-clause
failure context is `:fn` not the function name (deferred — the name is attached
after closure creation), and pattern destructuring of `&optional` slots is
deferred (required slots only).

**Tested.** `tests/pattern_matching.lisp` (a dedicated, exhaustive in-language
suite) plus cases in `tests/suite.lisp` and `crates/lisp/tests/basic.rs`,
including a TCO check that a match-driven loop doesn't overflow.

### First-class type tags (`type-of`) + self-identifying type errors (2026-05-27)

Step 1 of the types direction (ADR-023): make the runtime type tag a real thing
and put it into every type error.

- **`Tag` enum + `value::tag`** (`value.rs`) — the `Value` discriminant made
  first-class, one place that maps a value to its tag, with canonical names that
  mirror the predicates (`Sym` → `symbol`, `Str` → `string`).
- **`(type-of x)`** primitive → the tag as a keyword (`:int`, `:pair`, `:fn`,
  `:native`, …). The reflective primitive the in-language checks will build on;
  the `int?`/`string?` predicates are the common-case shortcuts.
- **`LispError::wrong_type(heap, who, expected, got)`** (`error.rs`) — one
  constructor for type errors that renders the op, the wanted type, and the
  offending value's tag + printed form: `first: expected list or vector, got
  int (5)` instead of the old `first: not a list`. Converted every scattered
  `type_err` arm in `builtins.rs` (numeric, sequence, vector, string, I/O) to it
  via `expect_int`/`expect_number` helpers. Error and `type-of` always agree on
  the tag word.

Deliberately *not* done yet: any compile-time checking. This is reflection +
diagnostics only — types stay runtime, the language stays fully dynamic.

**Verified.** Updated the suite's type-error assertions to the new messages and
added `type-of` cases (`tests/suite.lisp`) plus Rust tests (`basic.rs`); 34 Rust
tests + suite + doc-test green.

### Arity metadata on builtins, enforced at one gate (2026-05-27)

Follow-on to ADR-023's "more robust runtime checks." Arity was the one piece of
type metadata a primitive genuinely has (it's fixed) yet had nowhere to live: it
was hand-rolled per builtin (`two()`, ad-hoc `args.len()` checks) and the
`arg()`-returns-`nil` accessor meant several natives didn't arity-check at all —
a missing required arg silently became `nil` and surfaced later as a misleading
*type* error.

- **`Arity` on `NativeFn`** (`value.rs`) — `exact` / `at_least` / `range` /
  `any`, declared once per builtin in `register` (the single source of truth).
- **One gate** — `eval::call_native` checks `arity.accepts(argc)` before running
  the primitive, used by *both* the evaluator loop and `apply`, so a builtin
  reached through `(apply …)` is checked the same way. Built-in arity errors say
  "argument(s)"; user-function errors still say "args" (the suite pins both).

Now `(type-of)` / `(int? 1 2)` / `(now 1 2)` are clean arity errors instead of
silently wrong results. This is also the metadata a future compile-time
arity check would read.

**Verified.** Added arity assertions to `tests/suite.lisp` and a Rust test
(`basic.rs`); the prelude needed no changes (nothing relied on lenient arity);
35 Rust tests + suite + doc-test green.

### Set-theoretic type direction: `Ty` lattice (step 1) + the plan/contract (2026-05-27)

Chose the type-system direction: **set-theoretic and gradual, like Elixir's** —
sound where it speaks, `dynamic()` where it can't, advisory throughout — and
explicitly *not* the TypeScript "pragmatic but unsound" route. Recorded in
ADR-024 (refining ADR-023; "globals are `Any`" → "globals are `dynamic()`").

- **Step 1 shipped:** `crates/lisp/src/types.rs` — `Ty` is a set of runtime tags
  (a `u16` bitset over the 12 `Tag` atoms). Set operations *are* the type
  operations: union/intersect/negate/difference, **subtyping = set inclusion**
  (semantic subtyping), `NEVER`/`ANY`/`NUMBER`/`LIST`, an `of_value` bridge, and
  a `Display` for diagnostics. Pure algebra, 6 unit tests; nothing in the
  language consumes it yet — deliberately just the foundation.
- **Documented to be tackled one step at a time:** `docs/types.md` holds the
  model, the 5-step staircase (each shippable on its own, with done-criteria),
  and — the point the user emphasised — a **compatibility contract** every future
  change must honour so the language never drifts off the set-theoretic path.
  Several contract points are compiler-enforced (a new `Value` needs a `Tag` +
  bit; primitives will need a signature the way `Arity` is mandatory). Pinned a
  short invariant in `CLAUDE.md` so it's read every session.

Next small step (2): `dynamic()` — the gradual type, with its consistency
relation, and the "redefinable globals are `dynamic()`" rule.

### Emacs mode + editor-parseable errors (stage 1) (2026-05-27)

Started making Brood **editor-ready as a language concern**, alongside an Emacs
major mode (kept in the user's from-source Emacs tree, not this repo:
`lisp/progmodes/brood.el` + `inf-brood.el`). The mode is *traditional* (derives
from `lisp-data-mode`, modeled on `scheme.el`) — tree-sitter was rejected: a
Lisp's regular s-expr syntax means Emacs' native sexp machinery already covers
navigation/indent, so a grammar is marginal payoff. It adds font-lock, a
dedicated `brood-indent-function`, imenu, an inferior REPL over `comint`
(`run-brood` + `brood-send-*`, run through a *pipe* so the CLI takes its clean
non-`rustyline` path), and a `brood-compilation-mode`.

The canonical Brood source extension is now **`.blsp`** (was `.lisp`, which
collides with Emacs' `lisp-mode`); the whole repo was migrated.

**Stage 1 of parseable output** (`docs/tooling.md` is the contract):
- `error::Pos { line, col }` (1-based) + an optional `LispError.pos`.
- `reader.rs` tracks line:col; **parse errors** carry the exact position, and
  `read_all_positioned` pairs each top-level form with its start.
- `Interp::eval_source` tags any otherwise-unpositioned error with the enclosing
  top-level form's position (runtime errors → top-level-form line; precise inner
  positions are unreliable post-macroexpansion). `eval_str` (the REPL) stays
  position-free.
- The CLI prints GNU `FILE:LINE:COL: message`, which `compilation-mode` /
  `flymake` parse natively; `brood-run` / `brood-test` make errors clickable.

Deferred to stage 2: a machine-readable test reporter with per-test source
locations, plus `form-pos` / `current-file` introspection (the test macros can
query a form's position *before* it expands).

**Verified.** 38 Rust tests (+3 for positions) + suite + doc-test green; CLI
output confirmed end-to-end (`t.blsp:3:1: unbound error: …`, `:2:3: parse
error: …`); both `.el` files pass `check-parens`.

**Verified.** 6 `types` tests + 35 Rust + suite + doc-test green.

---

## 2026-05-27 — Pattern matching: review fixes (eval fallback + `%eq` hygiene)

A critical review of the pattern matcher surfaced two real issues; both fixed.

- **Correctness — unlowered pattern binders.** `let`/`fn` patterns lower to
  `match*` in the compile pass, but a binder can reach the evaluator *unlowered*:
  built inside a quasiquote unquote (the pass leaves quasiquote opaque), or
  produced by a macro expanded lazily within its own defining form. eval's
  symbol-only `let`/`fn` then failed with a misleading "expected a symbol". Fix:
  eval now keeps the design's Option-A delegation as a **fallback** — a non-symbol
  target / clause-shaped `fn` is lowered on the fly via `macroexpand_all` +
  `continue 'tail`. The common all-symbol case is detected away cheaply
  (`macros::fn_needs_lowering`; a `binds` scan for `let`), so ordinary code is
  unaffected. Compile pass = speed; eval fallback = correctness.
- **Hygiene — `=` shadowing.** The matcher emits bare primitive names, which a
  local binding can shadow (unhygienic macros, ADR-009). Switched the emitted
  equality from `=` to the kernel `%eq` (a `%`-name isn't rebound), so a local
  `=` no longer hijacks a `match`. `first`/`rest`/… stay shadowable until macro
  hygiene lands — documented.

Regression tests added (`tests/pattern_matching_test.blsp`, "lowering fallback"
groups): quasiquote-unquote pattern `let`, quasiquote-unquote multi-clause `fn`,
same-form-macro pattern `let` and multi-clause `fn`, and `=`-shadowing.
ADR-021/022 and `docs/pattern-matching.md` updated.

**Verified.** `brood test` 158/158 green; full `cargo test` green.

### Type prep: predicate→Ty bridge + finished the diagnostics sweep (2026-05-27)

Two easy, forward-looking wins toward the type staircase:

- **`Ty::tested_by(predicate)`** (`types.rs`) — maps a type-predicate name to the
  `Ty` it asserts (`int?`→int, `number?`→number, `list?`→list, `fn?`→fn∪native,
  …; `None` for non-tag predicates). This is exactly the data Step 4's occurrence
  typing / guard-narrowing will consume, built and tested now in isolation.
- **Finished the value-carrying error sweep** — converted the last raw `type_err`
  (`substring`'s start/end) to `expect_int`, so every builtin's type error names
  the op, wanted type, and offending value. Added a `types.rs` test locking
  contract point #9 (a singleton `Ty` prints as its `type-of`/`Tag::name` name,
  so errors / `type-of` / `Ty` can't drift apart).

**Verified.** 8 `types` tests + 35 Rust + suite + doc-test green.

### Project tooling: `brood test` discovery + `brood new` scaffolding (2026-05-27)

Finished the project tooling designed in ADR-019/020 and brought it green
end-to-end (an earlier commit had bundled the in-flight pieces; this completes
and wires them up).

- **Modules (ADR-019).** Emacs-flat `provide` / `require` / `*load-path*` over the
  one shared global table, written in Brood (`std/prelude.blsp`). Embedded std
  modules (`test`, `project`) are baked in and found via `%builtin-module` before
  the load-path. New Rust mechanism only: `cwd`, `file-exists?`, `dir?`,
  `list-dir`, `name`, `eval-string`, `%builtin-module`, plus `substring` for the
  path/affix helpers (`starts-with?`/`ends-with?`/`path-join`/`parent-dir`, prelude).
- **Project model + test runner (ADR-020).** A `project.blsp` manifest, convention
  over configuration: `src/` on the load-path, tests discovered as
  `tests/**/*_test.blsp`. `run-project-tests` (`std/project.blsp`) finds the
  project by walking up from the cwd, loads each test file (register-only), and
  runs the whole suite once. `brood test` is the CLI entry; `crates/lisp/tests/suite.rs`
  drives the same runner from the repo root, so `cargo test` exercises discovery.
  Migrated the existing suites to the `_test.blsp` convention (register-only, no
  self-`run-tests`).
- **`brood new <name>`.** Scaffolds a runnable skeleton — `project.blsp` +
  `src/main.blsp` (a `greeting`/`main` printing "hello <name>") +
  `tests/main_test.blsp` (a passing starter test) — so `cd <name> && brood test`
  works immediately. `run-project-tests` now **loads the project's `src/` first**
  (all `.blsp` under the source paths), so test files use the project's defs
  directly — no `require`/`provide` ceremony in the scaffold. Policy is Brood
  (`new-project`: name checks, refuse-if-exists, templates) over two new
  primitives, `make-dir` and `spit`.
- **User config (`~/.config/brood/config.blsp`).** A Brood `(config …)` file — the
  sibling of `project.blsp` — auto-created with documented defaults on first tool
  use (honoring `$XDG_CONFIG_HOME`) and loaded by the `brood` subcommands. First
  setting: `:git-init` (off by default), which makes `brood new` run `git init` in
  the new project. New Rust mechanism: `getenv` + `run-process` (the Emacs
  `call-process` analogue — a general subprocess primitive).
- The source extension is now **`.blsp`** repo-wide. `make install` / `make
  uninstall` put `brood` in `~/.local/bin`.

**Verified.** `brood test` 158/158 green (incl. `tests/modules_test.blsp` and a
nested `tests/meta/discovery_test.blsp` proving recursive discovery); `cargo test`
green (12 lib + 45 basic + the discovery suite + doc-test); `brood new foobar`
scaffolds, auto-creates the config, and its own `brood test` passes; with
`:git-init true` it initializes a git repo.

### Rust correctness/robustness/perf pass (same day)

A thorough review of the Rust core (review scoped to Rust only), then fixed
every finding:

- **`<` was lossy for large integers.** `%lt` coerced both operands to `f64`,
  so values past 2^53 compared wrong (`(< 9007199254740992 9007199254740993)`
  → `false`). Now ints compare in `i64`; only mixed/float args coerce.
- **`mod`/`rem`/`/` panicked on `i64::MIN` by `-1`.** Switched to
  `checked_rem`/`checked_div`/`checked_rem_euclid`: `mod`/`rem` raise
  "integer overflow", `/` falls through to the float path. (Matches the
  already-checked `+`/`-`/`*`.)
- **Deep structural recursion aborted the runtime.** `Heap::promote` (run by
  every top-level `def`/`set!` and `spawn`) and `Heap::equal` recursed down the
  cons *spine*, so a long list overflowed the native stack (uncatchable — it
  `abort()`s the whole process, all green processes with it). Both now walk the
  spine iteratively; recursion is bounded by element nesting. `def` of a
  200k-element list and `=` on two of them no longer overflow.
- **`gensym` was thread-local.** The counter reset per worker thread, so two
  threads could mint the same "unique" name — breaking the documented
  process-wide guarantee. Now a global `AtomicU64`.
- **`=` float semantics.** Switched from bitwise (`to_bits`) to IEEE value
  equality: `(= 0.0 -0.0)` is `true`, `(= nan nan)` is `false`.
- **Evaluator hot path.** Special-form dispatch called `symbol_name` on the head
  of *every* combination — a global-interner `Mutex` lock + `String` allocation
  (and cross-thread contention under the scheduler). Now it maps the interned
  symbol id (`u32`) to a `&'static str` via a `LazyLock` table, so ordinary
  function calls skip the lock/alloc entirely. Behaviour-identical (whole suite
  green).
- **Lock-poison hardening.** The global bindings `RwLock` and the symbol
  interner `Mutex` now recover from poison (`into_inner`) instead of `unwrap`,
  so a panic in one process can't wedge global lookup/`def` for every other.
- **Reader: dotted pairs.** The printer emitted `(a . b)` for improper lists but
  the reader couldn't read it back. A lone `.` inside a list now builds a dotted
  tail (round-trips); `.5`/`.foo` stay atoms.
- **Smaller items.** CLI `-jN` no longer eats a filename like `-justfile`;
  `LocalCheckpoint` documents why it omits the natives slab; clippy is clean
  (Heap `Default`, `parse_params` type alias, `env_set` entry API, range-loop).

**Deferred (by scope):** moving `when`/`unless`/`cond`/`and`/`or` from Rust
special forms to prelude macros (aligns with the "smallest core" principle but
is a Brood-side refactor, not a Rust bug — left as a roadmap item).

**Verified.** `cargo test` green (45 integration incl. 7 new regression tests +
the in-language suite + doctest); `cargo clippy` clean.

### Types step 2: `dynamic()`, the gradual type (set-theoretic) (2026-05-27)

`GradualTy { bound: Ty, dynamic }` in `types.rs` — `dynamic()` brought *inside*
the lattice (pure `dynamic()` = `dynamic(ANY)`), per the corrected framing
(ADR-024): consistent subtyping is **derived from set inclusion**, not a Siek–Taha
bolt-on. `consistent_with`: static → `bound ⊆ expected`; dynamic → `bound ∩
expected ≠ ⊥`. So pure `dynamic()` is consistent with every inhabited type (defer
the check) while `dynamic(number)` is still caught against `string`. Composes via
`union`/`intersect`/`negate`. The "redefinable globals are `dynamic()`, not `Any`"
rule is documented; no checker consumes it yet — foundation only, like step 1.

**Status check.** Steps 0–2 are done. What's *live now*: `(type-of x)`,
self-identifying type errors, enforced builtin arity. What's *foundation, not yet
consumed*: the `Ty`/`GradualTy` lattice + `tested_by` table — they change no
runtime behaviour until the Step 4 inference pass reads them. The first
behavioural payoff is Step 4.

**Verified.** 12 `types` tests + 45 Rust + suite + doc-test green.

---

## 2026-05-27 — Rust simplification pass (shrink the core)

**Goal.** A review of the Rust to make it as simple as possible without
compromising stability or performance, then apply the agreed cleanups.

**Done.**
- **Five special forms became prelude macros.** `when`, `unless`, `cond`, `and`,
  and `or` left `eval.rs` (and `SPECIAL_NAMES`) for `std/prelude.blsp`, defined
  over `if`/`do`/`let` (ADR-006/011; ADR-022 already called `when` a "cheap
  macro"). The evaluator's generic macro-expansion fallback already covers them,
  so this *removed* eval code with none added; the compile pass expands them once
  so runtime speed is unchanged. `while` stays a special form (no named-`let`
  yet). One gotcha: `cond` must test `else`/`:else` with the `%eq` *primitive*,
  not the variadic `=` — `=` builds a lambda, and doing that at expansion time
  during the prelude's own compile pass would strand a local-env closure and
  break the freeze invariant.
- **One arity-message formatter.** `arity_message` + `native_arity_message`
  collapsed into one `arity_message(who, min, max, got)`; builtins and user
  closures now word arity errors identically ("argument(s)"). Updated the two
  suite assertions that pinned the old "args" wording.
- **No-alloc symbol comparison.** Added `value::symbol_is` / `symbol_first_char`
  and used them where the code compared a symbol's spelling to a literal
  (`macroexpand_all`'s per-node walk, quasiquote's `tagged`, the `&optional`/`&`
  scans, `parse_params`) — dropping a `String` clone (and interner lock) per node.
- **Region-accessor macro.** `heap.rs`'s `vector`/`string`/`closure` accessors
  (identical LOCAL/PRELUDE/RUNTIME dispatch) now come from one `region_ref!`
  macro; `pair` (by-value) and `native`/`env_frame` (restricted regions) stay
  explicit.
- **`expect_string` helper** (second pass). Nine builtins repeated the same
  `match v { Str(id) => …to_string(), _ => wrong_type }` block; collapsed to one
  `expect_string(heap, who, v)` (matching the existing `expect_int`/
  `expect_number`). `spit`/`run-process`/`%builtin-module`/`name` keep bespoke
  messages and stay explicit.

**Verified.** 19 `types` + 45 Rust + Brood suite + doc-test green; macro edge
cases (`(and)`/`(or)`/short-circuit/`cond` `:else`) spot-checked at the REPL.

### Types step 4 (v0): the advisory checker — the lattice's first consumer (2026-05-27)

The `Ty` lattice finally *does* something. `crates/lisp/src/check.rs` + a `check`
builtin: `(check 'form)` macro-expands the form, walks it, and returns warnings
for **provably-wrong primitive arguments** — e.g. `(first 5)` →
`"first: argument 1 expects nil | pair | vector, got int (5)"`.

- **Rule is disjointness, not subtyping.** An argument is flagged only when its
  type shares *no* tag with what the primitive accepts (`arg ∩ param = ⊥`). A
  superset (`number` for `int`), an `any` result, or an unknown/variable
  (`dynamic()`, bound `ANY`) all overlap → never flagged. So **no false
  positives** by construction. Advisory: returns warnings, never raises.
- **Signatures** live in a `primitive_sig` table (Step 3, table form) for the
  discriminating primitives; argument types come from literals and from nested
  primitives' result types (`(first (string-length "a"))` warns on `first`).
- **Not yet:** closures (`(+ 1 "x")` — `+` is Brood, needs closure sigs), flow/
  guard narrowing (the `tested_by` bridge is ready), and auto-running in the
  compile pass (today it's the opt-in `check` builtin).

Honest payoff read: this is the first *behavioural* benefit from the lattice;
`type-of`/arity/self-identifying errors were already live from step 0.

**Verified.** 6 `check` tests + 13 `types` + 45 Rust + suite + doc-test green;
CLI demo flags `(first 5)` and recurses, with no false positive on `any` results.

### Shrinking the kernel: tag predicates, `mod`, `println` → Brood (2026-05-27)

Audited the native kernel for primitives expressible in Brood ("keep the
language as small as possible"). The arithmetic/comparison families (`+ - * /
< > = …`) were already prelude functions over the binary `%`-ops; this pass
moved three more groups down into `std/prelude.blsp`:

- **The 10 type-tag predicates** (`nil? pair? int? float? bool? string? symbol?
  keyword? vector? fn?`) → one-liners over `type-of`, the one irreducible
  reflective primitive (`(defn int? (x) (%eq (type-of x) :int))`; `fn?` unions
  `:fn`/`:native`). `docs/primitives.md` had filed these under "not expressible
  in-language" — false since `type-of` exists; the predicates merely duplicated
  it in Rust.
- **`mod`** → Brood over the `rem` primitive: euclidean result nudged back into
  `[0, (abs b))`. A ÷0 now surfaces as a `rem` error (the pinned error-message
  test was updated deliberately).
- **`println`** → Brood over `print` (`(defn println (& xs) (apply print xs)
  (print "\n"))`).

Net: **12 fewer Rust primitives** (the documented kernel 66 → 54). The
occurrence-typing bridge (`Ty::tested_by`, keyed by predicate *name*) is
unaffected — it already listed the Brood-defined `number?`/`list?`, so moving
the rest changes nothing there. Considered and rejected: `empty?` (reducible but
on every `fold` step — keep in Rust), `%sub` (derivable via `%add`+negate, but
worse float/overflow semantics), `first`/`rest` (splitting out `%car`/`%cdr`
relocates a primitive rather than removing one).

**Verified.** Full `cargo test` green (Rust + Brood suite + doc-test), with new
suite coverage for `mod`'s sign rules and every tag predicate (incl. `fn?` over
both a closure and a builtin).

### Editor-parseable output: structured errors + test failures (2026-05-27)

Made test/error output editor-readable as a *language* concern — contract in
`docs/tooling.md`, alongside an Emacs mode (`brood.el`, in the user's Emacs
tree). **One output format, always on**: structured, GNU-anchored, read the same
by humans, LLMs and Emacs (the user: "why is it not always structured?").

- **Source positions.** The reader records each list form's `line:col` in a heap
  side-table (`set_form_pos`/`form_pos`, dropped on `reset_local_to`). New
  primitives `(form-pos form)` and `(current-file)`. `LispError` gained
  `pos`/`file` + a `located()` renderer.
- **Errors** print GNU `FILE:LINE:COL: kind error: message` plus the source line
  and a caret. Parse errors are exact; runtime errors get the enclosing
  top-level form's line; unclosed `(`/`[` point at where they opened.
- **Test failures** (`std/test.blsp`): the assertion macros capture their own
  `(file line col)` at expansion (before the form expands) and push a structured
  record; the runner prints, per failed assertion, a `FILE:LINE:COL: test failed:
  group › name` anchor + indented `assert:`/`actual:`/`expect:` fields.
- **Removed** (greenfield, deleted not deprecated): the colour ✓/✗ progress
  trace, the `N processes (… nested)` summary line, the `:trace`/`:structured`
  mode split and `--format` flag, and the now-dead ANSI/`sum-ms`/per-test-loc
  helpers. `brood test` is structured by default.

**Verified.** `cargo test` green (Rust + 161-test Brood suite + doc-test); a
throwaway failing project renders the block with per-assertion `line:col`.

### Language server — design only, no code yet (2026-05-27)

**Goal.** Answer "how hard is an LSP, as a separate binary?" and lay a foundation
that doesn't get brute-forced one feature at a time.

**Finding.** A diagnostics-only server is ~1–2 days — `Interp` is already a clean
reusable boundary and `LispError` already carries `kind`/`pos`/`file`. The richer
features (hover, goto, completion, rename) all hinge on one missing thing:
**per-occurrence source spans**, which the eval `Value` can't carry (symbols are
`Copy`/interned/deduplicated; `form-pos` positions only list-form starts).

**Decision (design).** [ADR-025](decisions.md#adr-025--a-lossless-span-carrying-cst-for-tooling-separate-from-the-eval-value):
a lossless, heap-free, error-tolerant **CST** in `syntax::cst`, separate from the
reader's `Value`, with a `Span` on every node; a `crates/lsp` (`brood-lsp`)
binary on `lsp-server`/`lsp-types` (sync — the `Interp` isn't `Sync`); the server
never evaluates user buffers — syntactic diagnostics from CST `Error` nodes,
semantic ones from the advisory checker (ADR-024). Full plan, the `parse_cst`
sketch, and the feature tiers in [`lsp.md`](lsp.md). Next: pick where to start
(Tier-0 scaffold vs. feature planning).

### Immutability: dropped `set!` and `while` (ADR-026) (2026-05-27)

**Goal.** Commit to immutability as a language invariant. Triggered by noticing
the maps design asked about mutability without the project ever having decided it.

**The audit.** Brood already had **zero data-mutation primitives** (no
`set-car!`/`vector-set!`/atoms); data was immutable in practice. The only mutation
was binding mutation: `def` (rebind a global — load-bearing for hot reload) and
`set!` (rebind nearest binding). Every real `set!` targeted a *global* (so it was
doing `def`'s job) except the test framework's process-local `*fails*` accumulator.
`while` (the lone iteration special form) needs local mutation to make progress and
had **no Brood users**.

**Done.**
- **Removed `set!`** — the special form (`eval/mod.rs`) and the now-dead
  `Heap::env_set` (`core/heap.rs`; `set!` was its only caller). Global `set!` uses
  → `def` (`std/prelude.blsp` `*features*`, all of `std/project.blsp`, the test
  framework's registration globals).
- **Removed `while`** — recursion (TCO-safe) and processes cover looping.
- **Test framework → throw-and-collect (immutable, multi-failure kept).** A failing
  assertion `throw`s a tagged record `(:%test-fail loc details)`; the `test` macro
  splits its body into one thunk per top-level form and `test--run` runs each in its
  own `try`, folding the caught failures into a list. So a test still reports several
  failures — with no mutable accumulator (`*fails*` is gone). Limit: multiple asserts
  nested in one form stop at the first; a non-assertion error stops the test.
- **The invariant (ADR-026):** Lisp data is immutable; `def` (global rebinding) is
  the only mutation; mutable state is processes (Erlang model) or Rust-backed
  resource handles (the coming M2 buffer), never a mutable `Value`. This reinforces
  the tracing GC (no write barriers), `Send` heaps + copy-on-send, and the
  append-only shared code region. Net: two fewer special forms, one dead method.

**Verified.** 46 Rust tests + Brood suite + doc-test green; a throwaway failing
suite confirmed first-failure-per-test, uncaught-error-as-failure, and located
failure rendering all work. Docs synced (spec, language, primitives, testing,
roadmap×2, README, components, shared-code) and ADR-026 recorded.

### LSP foundations A + C: the tooling CST, docstrings, introspection (2026-05-27)

**Goal.** Build the substrate an eventual language server reads off (ADR-025),
without writing the server yet — so features later are thin handlers, not
brute-forced one at a time. User picks: leading-string docstrings, and "build
foundations in the lib first, no LSP crate yet."

**Built (Foundation A — the CST).**
- `syntax::atom` — the shared lexical rules (`AtomKind`, `classify`,
  `is_delimiter`) the reader and the CST both use, so the two parsers can't
  drift on what a token is. The reader now delegates to it.
- `syntax::cst` — a lossless, **heap-free**, **error-tolerant** span tree:
  `parse(&str) -> Node` always returns a tree (stray closes / unterminated
  strings / missing closes become `Error` nodes), records a byte `Span` on every
  node, keeps trivia and quote sugar, and exposes `node_at(offset)` ("what's
  under the cursor?"). `error::Span` added beside `Pos`. 9 unit tests incl.
  multibyte spans and recovery.

**Built (Foundation C — docstrings + introspection).**
- **Docstrings**: a leading string literal in a `fn`/`defn`/`defmacro` body is
  pulled onto `Closure.doc` *when more body follows* (a lone string stays the
  return value — CL/Elisp rule). Extracted in `make_closure`, carried through
  promotion.
- Primitives (Rust mechanism, derive-don't-store where possible): `(doc f)`,
  `(arglist f)` (reconstructs `required &optional … & rest` from the closure),
  `(global-names)` (new `Heap::global_symbols`), `(bound? 'x)`.

**Deferred to Foundation B / later (deliberately):** the CST scope resolver
(shared with the advisory checker) and definition-location tracking — they pair
with goto-definition. The `brood-lsp` crate comes after B.

**Verified.** `cargo test` green — 36 lib-unit (incl. 9 CST), 46 e2e, the Brood
suite, doctest; `brood test` 172/172 (new `tests/introspection_test.blsp`, 11
tests). Build warning-clean (the 3 remaining are pre-existing in `process.rs`).
Docs: `docs/lsp.md`, ADR-025, `language.md` (docstrings + introspection),
`tooling.md` pointer.

### Preemption + selective `receive` with timeouts (same day)

Closed the two scheduler gaps that blocked the editor milestone — both designed
already as *additive* steps, both built by composing existing machinery rather
than adding language surface. ADR-027; `scheduler.md` stage 4 + `pattern-matching.md`
§`receive` flipped to implemented.

**Reduction-counted preemption (fairness).** The scheduler was cooperative — a
process yielded only at `receive`, so a CPU-bound loop held its worker forever and
could starve a whole pool. Now `eval`'s `'tail:` loop calls `process::tick()` once
per iteration (a thread-local `Cell<u32>` decrement, budget ≈2000, reset by the
worker before each `resume`); at zero a green process yields and is re-queued
Ready. The coroutine yields a `Suspend` reason — `Receive` (park on mailbox) vs
`Preempt` (re-queue at the back) — so `run_one` can tell them apart. The root
thread has no yielder, so it's never preempted (just refreshes its budget).
Top-of-loop placement is complete (every non-terminating computation re-enters the
loop) and safe (no lock/borrow held there); TCO untouched.

**Selective `receive`.** Was unconditional FIFO (arity-0, popped the head). Now
`receive` is a **Brood macro** over a `%receive` primitive (matcher fn, timeout,
on-timeout thunk), reusing the `match` compiler: `match-build-from` with a `nil`
no-match continuation + each body wrapped in a thunk → a matcher returning the
body-thunk on a match or `nil`. `%receive` scans the mailbox, **removes+runs the
first match, leaves the rest queued** (true selective receive). A `scanned` cursor
on the mailbox means a parked selective receiver is only re-run on a *new* message.
`(after ms body...)` bounds the wait (`after 0` = non-blocking poll); a lazily
started **timer thread** (`BinaryHeap<(deadline,pid)>`) wakes a green process at its
deadline (root uses `cv.wait_timeout`). Stale timers are harmless (every receiver
re-validates its own deadline).

**Catchable timeouts (Erlang-style), as the user required.** The `after` body runs
through the normal `apply`/`throw` path, so `(after ms (throw [:timeout]))`
propagates out of `%receive` and is caught by the existing `try`/`catch` — no new
mechanism. Convention `[:timeout]`, paralleling `match`'s `[:match-error …]`.

**Removed:** the old arity-0 `receive` builtin and `process::receive` (replaced by
`%receive`/`receive_match`); the `processes` group in `suite_test.blsp` (moved).

**Verified.** `cargo test` green incl. a new dedicated **`tests/preemption.rs`**
(its own binary so `set_max_parallel(1)` is isolated: an infinite hog + a responder
on one worker — the responder only replies if preemption works; bounded by a 3 s
`receive` timeout so a regression fails fast instead of hanging). New
**`tests/concurrency_test.blsp`** (two `:isolated` groups, 15 cases): FIFO,
out-of-order selective match + leave-queued (root and green), guards, multi-clause,
nested patterns, `after 0` poll (hit/miss), `after N` on root + green, message-beats-
timeout, catchable timeout (root + green), throwing matched body + consumption, and
liveness at scale. `brood test` 187/187. Clippy clean (factored the timer-queue type).

---

## 2026-05-27 — Split the CLI: `brood` (language) + `nest` (project tool)

**Goal.** The single `brood` binary was doing two jobs — running the language
*and* being the project tool (`brood test`/`brood new`, config, scaffolding).
Split them, the `rustc`/`cargo` (and `elixir`/`mix`) way, so the language entry
point stays thin and the project tool can grow on its own. Name chosen with the
user: **`nest`** (the workspace that holds a brood). ADR-028.

**What changed.**
- **New `crates/nest`** — bin `nest`, depends on the `brood` lib (no subprocess,
  embeds `Interp` like `brood` does). Subcommands `new <name>` / `test` are a
  thin shell evaluating `(require 'project) (load-config) …`; policy stays in
  `std/project.blsp` (ADR-006). Carries its own `-j/--max-parallel`, `--version`,
  `--help`, and a usage screen on no/unknown command.
- **`crates/cli` (`brood`) is language-only now** — dropped the `test`/`new` arg
  branches. Added `--test <file>...` (load files → register cases → `(run-tests)`
  once; prepends `(require 'test)` so a bare file still works), plus `--version`
  and `--help`. `brood --test` is a *single-file* run; project-wide discovery is
  `nest test` — different jobs, not aliases.
- **Wiring:** workspace `members` gains `crates/nest`; `Makefile` `install`/
  `uninstall` now cover both binaries, `suite` calls `nest test`; added
  `bin/nest` launcher.
- **Docs:** ADR-028 recorded; `brood test`/`brood new` → `nest test`/`nest new`
  across `components.md` (new `nest` section + diagram), `testing.md`,
  `tooling.md`, `roadmap.md`, `types.md` (split `check` into `brood check <file>`
  / `nest check`), `decisions.md` (ADR-020), `lsp.md`, and `std/project.blsp`'s
  own messages/templates ("Next: cd … && nest test"). CLAUDE.md layout/commands
  updated.

**Verified.** `cargo build` + `cargo test` green (46 basic + lib + suite + doc).
`nest new demo` scaffolds and its `nest test` passes; `nest test` at repo root
187/187. `brood --test` runs a self-contained file; `--help`/`--version` on both
binaries. A test file that needs project `src/` correctly fails under
`brood --test` (no project setup) and passes under `nest test` — the intended
distinction.

## 2026-05-27 — Module docstrings + `nest doc` extraction

**Goal.** Function/macro docstrings already existed (ADR-025); add module-level
docs and a tool to extract them. ADR-029.

**What changed.**
- **Module doc = a file's leading string form** — the file-level analogue of the
  function-docstring rule; no new special form. Added module docstrings to
  `std/test.blsp` and `std/project.blsp` (dogfooding).
- **`std/docs.blsp`** — new baked-in module (`provide 'docs`). `generate-docs` /
  `document-module` / `document-file` render Markdown by snapshotting
  `(global-names)`, loading the module, and documenting the new names via the
  existing `(doc f)`/`(arglist f)`; the module docstring is read from source.
  `project` is required lazily so it stays out of the snapshot when it's the
  target.
- **`nest doc [module]`** — new subcommand (thin shell over `generate-docs`); no
  operand documents the whole project, a name documents one module.
- **Rust mechanism:** added `slurp` (read-side of `spit`); made `(global-names)`
  return names sorted by spelling (deterministic docs + better completion);
  registered `docs` in `EMBEDDED_MODULES`.
- **Tests:** `tests/docs_test.blsp` (leading-doc rule, name-delta, classification,
  basename, exact entry rendering); `slurp` round-trip + missing-file error in
  `crates/lisp/tests/basic.rs`.
- **Docs:** ADR-029; `language.md` (module docstrings), `tooling.md` (`nest doc`),
  `primitives.md` (`slurp` + the previously-undocumented introspection group;
  count → 60), `lsp.md` (resolved the "`(doc f)` pending" note).

**Known limit.** Attribution is load-order dependent (empty delta for an
already-loaded module; transitive `require`s leak in). The order-independent fix
is the static CST walk planned in `docs/lsp.md`.

**Verified.** `cargo test` green (48 basic + lib + Brood suite + doc). `nest doc
test`/`doc project` render module docstring + signatures; unknown module errors
cleanly; `nest --help` lists `doc`.

---

## 2026-05-27 — Immutability cleanup: lighter env frames + dedup

**Context.** With immutability now an invariant (ADR-026), swept the Rust kernel
for machinery that mutation used to justify. Confirmed the big cleanup already
landed cleanly — no `set!`/`while`/`env_set` remnants, no mutable accessors to
`Value` data (`grep` for `*_mut`/`set_car`/… is empty), and all interior
mutability is legitimately scoped (the `def`/hot-reload global table, the
interner, scheduler state). Two genuine wins remained.

**Lexical env frames: `HashMap` → association list.** `EnvFrame.vars` was a
`HashMap<Symbol, Value>`. But frames hold a handful of bindings (params, a
`let`'s names) and are immutable after their bind phase, so a build-once /
scan-to-read `Vec<(Symbol, Value)>` is both simpler *and* faster at these sizes —
no per-frame hash allocation, no hashing. Lookups scan from the end so a later
binding shadows an earlier same-named one (sequential `let`). Measured on the
`divan` benches: **~18% faster** across the function-call hot path (fib(25)
1.556 s → 1.278 s; sum_tail(100000) 718 ms → 586 ms). The global table stays a
`HashMap` (large, lookup-heavy, and the one mutable structure).

**Dedup: one definition of "sequence form → `Vec`".** `as_binding_vec`, the head
of `parse_params`, and `parse_optional` each re-implemented the list/vector/nil →
`Vec<Value>` coercion that `Heap::seq_items` already provides. Routed all three
through `seq_items` (via `.map_err` to keep each site's specific message),
removing ~20 lines.

**Also.** `global_names` now uses `sort_by_cached_key` (the sort key
`symbol_name` locks the interner and allocates, so resolve each once) — clears
the lone clippy warning.

**Verified.** `cargo test -p brood` green (46 + 48 + Brood suite + doc). The
unrelated `crates/lsp` workspace member (in progress, no `main.rs` yet) breaks a
full `cargo test`; scoped to `-p brood`.

---

## 2026-05-27 — `brood-lsp`: the language server, Tier 0

**Goal.** Land the LSP server the foundations (CST, scope resolver, docstrings)
were built for — see [lsp.md](lsp.md). Scope this session to **Tier 0**: a real
server an editor can connect to, publishing *syntactic* diagnostics.

**Shipped.** New `crates/lsp` → **`brood-lsp`** binary (workspace member #4),
depending on the `brood` lib + `lsp-server`/`lsp-types` 0.97 (the synchronous,
no-tokio stack rust-analyzer uses — chosen in lsp.md because `Interp`/`Heap` is
`!Sync`, so a blocking request loop owning the document store avoids all
`Send`/`Sync` friction).

- `line_index.rs` — `LineIndex`: byte offset → LSP `Position`, with **UTF-16**
  column arithmetic (LSP's default `positionEncoding`; we advertise it
  explicitly). Tested incl. multibyte (`é`, `😀`).
- `diagnostics.rs` — `collect`: a walk over `cst::parse`'s `Error` nodes into
  `(Span, message)` pairs (LSP-agnostic, so unit-testable against the CST
  alone). Names the three CST recovery shapes: unmatched close, unterminated
  string, unclosed delimiter.
- `main.rs` — stdio `Connection`, FULL document sync, `initialize` handshake,
  `didOpen`/`didChange`/`didClose` over a `Uri`→text store, `publishDiagnostics`
  (severity ERROR, `source: "brood"`). The server **never evaluates** buffer
  text — diagnostics come from parsing, not running.

**Gotcha hit.** First end-to-end run hung: `main_loop(&connection)` keeps the
`Connection`'s `Sender` alive, so the stdout writer thread never sees its channel
close and `io_threads.join()` deadlocks. Fix: `drop(connection)` before the join
(documented inline). Verified with a scripted `initialize`+`didOpen`(unclosed)
+`didChange`(fixed)+`shutdown` over real LSP framing: one ERROR at EOF, then
cleared, clean exit.

**Also.** Full `cargo test` is green again now that `crates/lsp` has a `main.rs`
(the previous entry's caveat is resolved): 46 + 48 + lsp's 9 + Brood suite + doc.
`make install`/`uninstall` now build `brood-lsp` into `~/.local/bin` alongside
`brood` and `nest`, so `eglot` finds it on `PATH`.

---

## 2026-05-27 — Maps (immutable `{ }`)

**Goal.** Implement the last Tier-1 data type — maps — which a previous attempt
stalled on. The blocker then was immutability + hashing; the resolution makes
both a non-issue.

**Decision (ADR-030).** A map is an **immutable value**, modelled exactly like a
vector: a new `Value::Map(MapId)` / `Tag::Map` over a slab, with the internal
representation an **insertion-ordered association vector** `Vec<(Value, Value)>`.
Keys are compared by the existing structural `heap.equal` — so any value can be
a key and we never need a `Hash` over heap-resident data (the snag that stalled
the earlier try). Every op returns a fresh map; nothing mutates.

**What landed.**
- **Kernel:** `Value::Map` + `Tag::Map` (`value.rs`); a `maps` slab in all three
  regions + `alloc_map`/`map`/`map_get`/`map_assoc`/`map_dissoc`/`map_contains`/
  `map_from_pairs` and a `Map` arm in `promote`, `freeze`, `equal`, and the
  `LocalCheckpoint` reset (`heap.rs`). The reader turns `{ }` into a literal map
  (odd count is an error, commas are whitespace); `eval` evaluates a map
  literal's keys+values and canonicalises (last-wins); the printer renders
  `{k v, k v}`. `macroexpand_all` and `quasiquote` walk into maps. Messages gain
  a `Map` variant so maps cross process heaps (`process.rs`). The `Ty` lattice
  gains `Tag::Map` + `tested_by("map?")`.
- **Primitives (7):** `hash-map`, `map-get` (2–3, optional default),
  `map-assoc`, `map-dissoc`, `map-keys`, `map-vals`, `map-contains?`. `empty?`
  gained a map case.
- **Brood surface (`std/prelude.blsp`):** `map?`, `get` (with default), variadic
  `assoc`/`dissoc`, `keys`/`vals`/`contains?`; `count` now handles maps.

**Verified.** Insertion order, last-wins dedup, immutable update (original
binding unchanged after `assoc`), order-independent `=`, structural keys
(strings/vectors/numbers), computed/quasiquoted values, nesting, `pr-str`
round-trip, and a map sent to another process and echoed back. New
`tests/maps_test.blsp` (auto-discovered) + a `maps_are_immutable_values` Rust
case. Full `cargo test` + `nest test` (218 in-language tests) green.

**Result.** Tier-1 maps done; the language now has all four core data types
(nil/bool/num/sym/keyword/string + list/vector/map).

---

## 2026-05-27 — String library

**Goal.** The next Tier-1 gap: a usable string library. Today only `str`,
`pr-str`, `string-length`, `substring` existed.

**Kernel split (the principle: add Rust only where it's unavoidable).** Just
**3 new primitives**, each genuinely needing Rust:
- `upper` / `lower` — Unicode-aware case folding (`(upper "ß")` → `"SS"`), which
  leans on the standard library's case tables.
- `string->number` — a *strict* parse → int, else float, else `nil`. Can't be
  expressed over `read-string`, which would read `"3abc"` as `3` and stop; the
  strict parse-or-nil is the whole point.

**Everything else is Brood** (`std/prelude.blsp`, new "strings" section), over
`substring` / `string-length` / `str`: `char-at`, `string->list` / `list->string`,
`number->string`, `index-of`, `string-contains?`, `join`, `string-split`,
`replace`, `trim` / `triml` / `trimr`, `blank?`. All the recursive helpers are
**tail-recursive** (stack-safe on long strings). Notes: chars are **1-char
strings** (no distinct char type — deferred), indices are **char-based** (correct
for multi-byte UTF-8); `substring` is O(index) so a full scan is O(n²) — fine for
the short strings this targets, with large-text performance deferred to the M2
rope engine. `string-split`/`join` are inverses; an empty separator splits into
characters; `replace` is `join` over `string-split`.

**Verified.** New `tests/strings_test.blsp` (31 cases, auto-discovered: case
folding incl. Unicode, strict parse-or-nil, char access + round-trip, search,
join/split round-trip, replace, trim/blank edge cases) + a `string_kernel` Rust
case in `basic.rs`. `cargo test -p brood` green (50 Rust + Brood suite + doc);
`nest test` 249/249. Docs: `primitives.md` (+3, kernel now 70), `language.md`
(rewrote the Strings section), `ROADMAP.md` (String library ✅, count 39→70).

**Result.** Tier-1 string library done. Remaining Tier-1 gaps: Math library,
sequence library, dynamic variables.

---

## 2026-05-27 — Maps: thorough review + concurrency tests

**Goal.** Review the new maps (ADR-030) for edge cases and add broad coverage,
including explicit multi-core tests.

**Review — no map bugs found.** Probed the edges: int vs float keys are distinct
(consistent with `=`, `(= 1 1.0)` is false); maps/vectors/lists work as keys
(structural equality); a stored `nil`/`false` is distinguished from absence by
`contains?`; nested-map equality is order-independent and depth-sensitive;
`assoc`/`dissoc` never touch shared structure; `pr-str` round-trips through
`read-string`+`eval`; maps `promote` into RUNTIME on `def` and survive the arena
reset. One semantic note: `get`/`keys`/`vals`/`contains?` *error* on a non-map
including `nil`, even though the rest of Brood nil-puns collections
(`count`/`first`/`rest`/`empty?` accept `nil`). Left strict; flagged as a possible
consistency follow-up (make the Brood surface treat `nil` as `{}`).

**Tests.** Expanded `tests/maps_test.blsp` (~90 assertions: construction, access,
immutable updates, structural keys/equality, printing/round-trip, scale, type
errors) plus an `:isolated` **"maps across processes"** block — a deep map
round-tripped through a worker; a worker's immutable update leaving the sender's
map intact; 20-way fan-out/fan-in assembling one map; 50 processes concurrently
reading a shared global map; a multi-stage process pipeline; a map in a
selective-receive pattern. Four new Rust cases in `basic.rs`. `nest test` 272/272.

**Bug found (pre-existing, NOT maps).** Writing the scale tests surfaced a
**green-process stack overflow → uncatchable segfault**: a `test` body runs in a
coroutine whose stack is corosensei's ~128 KiB default, and the recursive
evaluator uses ~2 KiB/frame, so **non-tail recursion deeper than ~50 frames
segfaults** the whole runtime (the root thread's 8 MB stack hides it). Repro:
`(defn deep (n) (if (= n 0) 0 (+ n (deep (- n 1)))))` then
`(spawn (fn () … (deep 100)))`. Worked around in the tests by keeping helpers
tail-recursive. **Real fix (follow-up, scheduler not maps):** give worker
coroutines a larger stack (`corosensei` `Coroutine::with_stack` / a stack pool)
and/or detect exhaustion and raise instead of faulting.

**CLAUDE.md.** Added the rule that **every language feature must be tested across
multiple cores** (the parallel suite covers it by default — each test is a green
process — plus add explicit `spawn`/`send`/shared-global coverage), with the
tail-recursion caveat above.

---

## 2026-05-27 — `(ref)` unique tokens + synchronous call/reply

**Goal.** Writing `examples/life.blsp` (a Game-of-Life feature tour) exposed a
concurrency footgun: a script exits when its **main** process returns, so ending
on a fire-and-forget `send` races the spawned work and drops it. The question
became "what's the simple Erlang way to wait?" — and the answer is *not* an
`await` primitive. The blocking `receive` already **is** the synchronisation; you
just structure the protocol as a synchronous **call** (request + reply) rather
than a **cast** (bare `send`), and end on the call. The only missing piece was a
way to tell concurrent replies apart.

**Built.** `(ref)` — a primitive returning a fresh, opaque, unforgeable
reference token (Erlang's `make_ref`), the only way to make one.
- New `Value::Ref(u64)` + `Tag::Ref` (`type-of` → `:ref`, predicate `ref?`),
  threaded through the compatibility contract: `value.rs` (variant, tag, name),
  `types::ALL_TAGS` (now 14) + `tested_by`, `printer` (`#<ref N>`), `heap.equal`
  (compared by identity), and `process.rs` `Message::Ref` (refs survive a
  copy-on-send unchanged — they're runtime-global identities).
- A monotonic `AtomicU64` behind the `ref` builtin. Distinct from a pid (which is
  still a plain `Int`) precisely so a reply tagged with a ref can never be
  confused with a pid or a user integer.
- Tests in `tests/concurrency_test.blsp`: identity/distinctness/type, and a
  ref-tagged `call` round-trip where a stale reply with a *different* ref is left
  queued rather than mistaken for ours (the pin `~tag` selects exactly one).

**`call`/`reply` live in the example, not the prelude — and here's why.**
Attempting to add them to `std/prelude.blsp` broke the prelude **freeze**
(`debug_assert!(c.env.is_none())` in `heap.freeze_as_shared_code`). Root cause is
a **pre-existing latent landmine**: `call` uses `receive`, and the `receive`
macro's expansion *executes* match-compiler helpers — plus `map` and the variadic
`=` — at the prelude's **own** compile pass. Those library fns build transient
lambdas (`=` → `(fn (a b) (%eq a b))`, `map` → `(fn (acc x) …)`) that capture a
local frame; the prelude build leaks them into the region, and freeze (which
drops all env frames) rejects any closure with a non-global env. The `cond`
definition already carries a comment warning about exactly this (it uses `%eq`,
not `=`, to avoid stranding a lambda). **Conclusion: `match`/`receive` cannot be
used inside a prelude-level function today** (debug builds). User code is
unaffected (it expands after freeze), so `call`/`reply` sit in the example. Real
fix is freeze-time reachability (drop unreachable closures) — naturally falls out
of the tracing-GC migration (ADR-002); filed as follow-up.

**Docs.** `docs/language.md`: `Ref` in the data-types table, `ref?`/`:ref` in
predicates, `(ref)` in the process table, and a new *Synchronous calls (and why
there's no `await`)* subsection. `examples/life.blsp` rewritten with an animated
glider (overprinting via `\e[…` ANSI + `(after ms)` as the sleep), the call/cast
process server, and an "unbounded set" demo. `cargo test` green (276 in-language
+ Rust suites).

## 2026-05-27 — Math + sequence libraries

**Goal.** Round out the standard library: the two remaining Tier-1 library items
from `ROADMAP.md` — the **math library** and the **sequence library**. Mechanism
in Rust, policy in Brood (ADR-006): only the ops that genuinely need f64 / checked
integer division became kernel primitives; everything else is Brood over them.

**Kernel (`builtins.rs`), 1 new primitive: `floor`** (Float→Int, toward −∞). An
int passes through; a float is floored and cast to `i64`. This is the *only*
math op that genuinely needs Rust — there's no other primitive that crosses
Float→Int. (`rem` was already a primitive and stays one: deriving integer
remainder via float division would lose precision past 2^53.)

> **Course-correction (same session).** The first cut added *six* primitives —
> `quot`/`floor`/`ceil`/`round`/`sqrt`/`pow` — reflexively, because the roadmap
> said "[kernel] for the float ops". The user pushed back: *as much Brood as
> possible; even `+`/`<` are Brood (over `%add`/`%lt`) — only add Rust when it's
> truly irreducible.* On audit, five of the six were expressible in Brood:
> `ceil`/`round` over `floor`; `quot` over `rem` + exact-int `/`; `pow` as
> recursive multiply; `sqrt` by Newton's method. Only `floor` (Float→Int) had no
> Brood path. Reverted to the single primitive.

**Brood (`std/prelude.blsp`).**
- Math (all over `rem`/`floor`/`/`/`*`/`<`): `ceil` = `-floor(-x)`; `round` =
  `floor(x±0.5)`; `quot` = `(/ (- a (rem a b)) b)` — exact (the dividend is then
  divisible, so `/` takes the exact-int path, no float round-trip, correct past
  2^53); `pow` (integer exponent — tail-recursive multiply; negative exponent →
  float reciprocal; non-integer exponent errors, "use sqrt for roots"); `sqrt`
  (Newton's method — **approximate**, a few ULPs off the hardware sqrt, and
  trivially redefinable). Plus `even?`/`odd?` (over `rem`) and `min`/`max` made
  **variadic** (fold over a required first arg, strict `>`/`<` so the first of
  equal extrema wins).
- Sequences: `range` (1/2/3-arg, ascending or descending step), `take`/`drop`,
  `take-while`/`drop-while`, `some?`/`every?` (booleans, by the `?` convention —
  `find` recovers the element), `find`, `zip` (→ `[x y]` vectors, stops at the
  shorter), `partition` (drops a trailing partial, Clojure semantics), and
  `sort`/`sort-by` — a **stable merge sort** (the merge prefers the left run on
  ties; recursion depth O(log n)). **Every builder is tail-recursive** (accumulate
  reversed, reverse once) so they stay stack-safe on long inputs — which matters
  because a `test` body runs in a green process with a small coroutine stack.

**Tests.** New `tests/math_test.blsp` and `tests/sequence_test.blsp`,
auto-discovered by the project runner. Each covers the single-threaded behaviour
(the whole suite already runs each test in its own green process, so multi-core
coverage is automatic) **plus** an explicit `:isolated` "across processes" block:
workers compute/build the values, `send` them back (deep-copied across per-process
heaps — proving the round-trip), read a shared global, and the parent fans them in.
sqrt assertions use an epsilon helper (it's approximate now). `cargo test` green:
53 Rust + **325 in-language**.

**Audit (the user's "check all standard libraries").** Walked the whole kernel:
the rest is genuinely irreducible — `%add…%eq`/`rem` (number-repr dispatch +
overflow / exact remainder), `cons`/`first`/vector/`map-*` (heap repr),
`string-length`/`substring`/`upper`/`lower`/`string->number` (char indexing,
Unicode case tables, parsing), `type-of` (reflection), `str`/`pr-str`/`print`
(value→text), and the I/O / self-hosting / process / introspection hooks. Each
already has a comment pointing at its Brood surface; nothing else was reducible.

**Docs.** `docs/primitives.md` (count 74; `floor` the sole math primitive + the
irreducibility note), `docs/language.md` (Arithmetic + Lists & sequences, the
"everything here is Brood" note), `roadmap.md` / `ROADMAP.md` (both items ✅). No
new ADR — this is exactly ADR-006/008 applied; the lesson (don't add a primitive
before checking it can't be Brood) is the course-correction note above.

---

## 2026-05-27 — Process monitors (supervision M0)

**Goal.** First step toward an Erlang/OTP-style supervision layer, but built the
Brood way: a minimal kernel mechanism with all policy (gen_server, supervisors,
restart strategies) to live in Brood later. The one irreducible piece is a way to
learn that a process has **died** — so: process monitors (monitors-only; no
links yet, by decision).

**Built (kernel, `process.rs` + `builtins.rs`).**
- `(monitor pid)` → returns a monitor `ref`; registers `(watcher, mref)` under
  the watched pid in a new `MONITORS` table. When the watched process
  deregisters, every watcher is delivered `[:down <mref> <pid> <reason>]`.
  Monitoring an already-dead pid delivers `:noproc` immediately. Unidirectional,
  one-shot. `(demonitor mref)` removes it (best-effort).
- **Exit reason.** A process's coroutine now records its reason in an
  `EXIT_REASON` thread-local just before returning — `:normal` on a clean return,
  `[:error <msg>]` on a Brood error (a true Rust panic → `:killed`). `run_one`
  reads it (same worker thread, right after `resume`) and passes it to
  `deregister(pid, reason)`, which fires the monitors.
- Factored `deliver(pid, msg)` (mailbox push + wake) out of `send`; monitor DOWNs
  reuse it. The root process is already in `REGISTRY` (via `ensure_ctx`), so DOWNs
  to the main process are delivered, not dropped.
- `(ref)` and `(monitor …)` now share one `NEXT_REF` counter (`process::next_ref`)
  so every ref is distinct. `Message` derives `Clone` (a DOWN reason is cloned per
  watcher).

**Tests.** `tests/concurrency_test.blsp` — normal `:normal`, crash `[:error …]`,
`:noproc` for already-dead, `demonitor` suppresses, and ref identity. `nest test`
322/322.

**Docs.** `docs/language.md` gained a *Monitors* subsection (+ table rows);
`docs/primitives.md` lists `ref`/`monitor`/`demonitor`.

**Next (see `todo.md`).** M1: the Brood process-framework library (gen_server-
style `defprocess`, `gen-call`/`gen-cast`, `!`) in a `require`-able module — needs
a name (not "OTP"). M2: a Brood `supervisor` (one-for-one / rest-for-one /
all-for-one, checkpoint/resume) built on monitors.

---

## 2026-05-27 — brood-lsp Tier 1: completion, hover, document symbols, goto-definition

**Goal.** Move the language server past Tier-0 diagnostics to the everyday
editor features (`docs/lsp.md` Tier 1), reusing the foundations already in
place rather than adding substrate.

**Built (`crates/lsp`).** All four handlers are thin wiring over machinery that
already existed — the CST (`syntax::cst`), the scope walker (`syntax::scope`),
and the introspection primitives (`arglist`/`doc`/`global-names`):
- **`textDocument/completion`** (`completion.rs`) — locals visible at the cursor
  (`scope::names_in_scope`, marked variables, listed first so a shadowing local
  outranks the global it hides) + interpreter globals (`global-names`, marked
  functions). De-duped; the client does prefix filtering.
- **`textDocument/hover`** (`hover.rs`) — resolves the symbol under the cursor
  and renders by binding: a **local** → a short note; a **document `def`** → its
  signature + docstring read straight off the CST (`defs.rs`); a **free** name
  (prelude/builtin) → its `arglist` + `doc` via the interpreter.
- **`textDocument/documentSymbol`** (`symbols.rs`) — outlines top-level
  `def`/`defn`/`defmacro` (full form = `range`, name token = `selection_range`).
- **`textDocument/definition`** (`definition.rs`) — `scope::resolve_at` → the
  binder's span; a free name has no in-document binder → null. Landed with Tier 1
  (not Tier 2 as first sketched) because Foundation B already shipped the walker.
- `defs.rs` — top-level def model (kind, name/full spans, params, leading-string
  docstring) shared by hover + documentSymbol. `introspect.rs` — `Interp`-backed
  `global-names` / `(arglist .) + (doc .)` queries. `LineIndex` gained the
  `Position → byte offset` inverse for incoming request positions.

**Design notes.** The server now owns one `Interp` (prelude + builtins) for
introspection only; it still **never evaluates the open buffer** (`docs/lsp.md`).
A symbol's text is safe to interpolate into `(arglist NAME)`/`(doc NAME)` because
a CST `Symbol` token can't contain a delimiter, quote, or `;` (`syntax::atom`).
An empty `arglist` is ambiguous (builtin vs zero-arg fn), so hover shows no
signature there rather than a misleading one.

**Tests.** 34 in `crates/lsp` (up from ~12): per-feature unit tests plus an
end-to-end `serves_tier1_requests_end_to_end` driving real request/response
round-trips over `Connection::memory()`. Full workspace green (`cargo test`),
clippy clean for the crate.

**Next.** Tier 2 — references, rename, semantic tokens, and located *semantic*
diagnostics (needs `types::check` to carry spans). Signature help (active-param
tracking) is the small remaining Tier-1 item.

---

## 2026-05-27 — `hatch`: a gen_server in Brood (supervision M1)

**Goal.** With monitors landed (M0), build the gen_server layer — but as Brood
policy in a `require`-able module, not Rust and not the baked prelude (which would
hit the `match`/`receive` prelude-freeze landmine). Named `hatch` (fits the
spawn/offspring metaphor; deliberately NOT "OTP").

**Built — `std/hatch.blsp`** (embedded module; `(require 'hatch)`):
- `(defprocess name (state) (cast PAT body…) (call PAT body…) …)` — a macro that
  compiles cast/call clauses into a tail-recursive `receive` loop. A **cast**
  body evaluates to the next state; a **call** body to `[reply next-state]`.
  State is immutable and explicit: to keep it, a clause returns the state var.
  Messages ride internal envelopes (`[:$cast …]` / `[:$call from ref …]`) so the
  loop tells them apart; a call is matched to its reply by a fresh `(ref)`.
- `(hatch f state)` spawns one; `(! pid payload)` casts; `(gen-call pid payload)`
  is the synchronous, ref-tagged request. All ~30 lines of Brood over the kernel.

**Tests / example.** `tests/hatch_test.blsp` (state threading, no-op cast, call-
updates-state, ordering, two servers not crossing wires). `examples/life.blsp`'s
process section rewritten from a hand-rolled receive loop to `defprocess`
life-server + `hatch`/`!`/`gen-call`. `cargo test` green (Rust + 34 in the
process bucket; full suite passes).

**Known gaps (todo.md M1.x).** No clean stop/terminate yet (a hatch process loops
forever) — needed before M2 supervisors can shut children down. No `keep`
shorthand; state is a single value (pack config into it).

**Next — M2:** a Brood `supervisor` on monitors: spawn + monitor children, restart
per strategy (`:one-for-one` / `:rest-for-one` / `:all-for-one`), checkpoint/
resume. Needs the stop path first.

## 2026-05-27 — Kernel audit: drive Rust to the absolute minimum

**Goal.** User directive after the math reduction: *go over the whole language
and use absolutely minimum Rust — even `+`/`<` are Brood (over `%add`/`%lt`); only
keep a primitive if it's genuinely irreducible.* Walked all ~74 primitives and
read each implementation, asking "is there any Brood path over simpler ops?"

**Reduced (3 primitives removed, 74 → 71):**
- **`empty?`** → Brood. It was pure type dispatch over things the kernel already
  exposes: the empty list is `nil`, and string/vector/map emptiness is a length
  (`string-length`/`vector-length`/`map-keys`). Defined early in the prelude (it
  bootstraps `fold`) with raw kernel ops (`%eq`/`type-of`/…) since `cond`/the tag
  predicates come later.
- **`map-vals`** → Brood: `(map (fn (k) (get m k)) (keys m))`. (`get` returns a
  present key's value, even a falsy one, so this reproduces the values in order.)
  O(n²) on the association-vector rep; a HAMT later restores O(n), no surface
  change (ADR-030).
- **`map-contains?`** → Brood: `(member? k (keys m))` — O(n), same as before.
  Also dropped the now-dead `heap.map_contains` helper.
  The map kernel is now the minimal **{hash-map, map-get, map-assoc, map-dissoc,
  map-keys}** — construct, read, two producers, one enumerator.

**Audited and *kept* (with the specific reason each is irreducible):**
`%add…%eq` (number-repr dispatch + overflow); `rem` (exact integer remainder —
float division loses precision past 2^53); `floor` (the only Float→Int crossing);
`cons`/`first`/`rest` (the pair accessors — `first`/`rest` *are* car/cdr);
`vector`/`vector-ref`/`vector-length` and the map kernel (opaque-rep access);
`string-length`/`substring`/`upper`/`lower` (char indexing, Unicode case tables);
`string->number` (strict parse-or-nil — a correct float parser, *not* reducible to
`read-string`, which isn't strict and reads only one form); `type-of`
(reflection); `str`/`pr-str`/`print` (value→text — the printer); `apply` (the
splice-call primitive); `eval`/`read-string`/`eval-string`/`load` (self-hosting —
`load`/`eval-string` drive a multi-form read Brood can't iterate, and `load` adds
`current-file` + `FILE:LINE:COL:` error context); `name`/`gensym` (interner);
`macroexpand*`/`check` (eval & checker machinery); `form-pos`/`current-file`/`doc`/
`arglist`/`global-names`/`bound?` (CST / env / global-table reflection); the
filesystem, system, time, memory, error, and process primitives (I/O & runtime).

**Verified.** `cargo test` green (53 Rust + **330 in-language**), no warnings.
Docs: `docs/primitives.md` (count 71; the three removals folded into the
irreducibility note).

---

## 2026-05-27 — brood-lsp: signature help completes Tier 1

**Goal.** Close out the last Tier-1 feature (`docs/lsp.md`): `textDocument/
signatureHelp`.

**Built (`signature.rs`).** While typing a call's args, show the callee's
parameter list with the active argument highlighted:
- `enclosing_list` — innermost `List` whose span contains the cursor, with
  **inclusive-end** containment (unlike `node_at`): signature help fires at EOF
  inside an unclosed `(map ` where offset == the recovered span's end, which a
  half-open check misses.
- Param source: the CST def (`defs::find_def`) when the head symbol resolves to a
  document `def`, else `introspect::arglist_tokens` (new) for a prelude/builtin.
- `slots` drops the `&optional` / `&` markers and reduces an `(b 1)` optional
  group to `b`, so the highlighted parameters are the bindable ones; the full
  arglist (markers and all) stays in the signature label.
- `active_param` = the arg form containing the cursor (end-inclusive, so editing
  at an arg's end counts as that arg), else the count of args completed before it;
  clamped into range so a `& rest` tail / extra args land on the last slot.
- Capability advertises `(` and ` ` as trigger/retrigger chars (Lisp args are
  whitespace-separated).

**Review fixes folded in (from the prior session's review).** UTF-16 `offset`
now snaps a mid-surrogate column *back* to the char start (was forward);
`defs::find_def` recurses so hover finds a `def` nested in a `do`/`when` (still a
global); and the introspection queries bracket each `eval_str` with
`checkpoint`/`reset_local_to` so a long server session doesn't leak a result list
per keystroke (the REPL's reclamation pattern).

**Scope reminder.** All Tier-1 features are **single-file**: names come from the
open buffer or the prelude/builtins, never from `require`d modules (the server
never evaluates the buffer, so it never runs a `require`). Cross-file resolution
is a separate workspace-indexing feature — documented under §Cross-file in
`docs/lsp.md`, deferred.

**Tests.** 43 in `crates/lsp` (per-feature units + the end-to-end loop test).
Full workspace green; clippy clean for the crate.

**Next.** Tier 2 — references, rename, semantic tokens, located semantic
diagnostics (needs `types::check` to carry spans); and, separately, the
workspace index for cross-file navigation.

## 2026-05-27 — `map-pairs`: one map enumerator; reduce-kv; docstring-on-pattern fix

**Goal.** Continue the kernel minimization on the map type, fixing the O(n²) the
previous pass left in `vals`, and unblock the `examples/life.blsp` simplification.

**Kernel: `map-keys` → `map-pairs`.** Replaced the keys-only enumerator with one
that returns the entries as a list of `[k v]` vectors in a single O(n) pass.
Primitive count unchanged (a rename), but it's strictly more expressive, so the
whole map surface is now Brood over it:
- `keys` = `(map first (map-pairs m))`, `vals` = `(map second (map-pairs m))`
- `contains?` = `(some? (fn (p) (= k (first p))) (map-pairs m))`
- `reduce-kv` (new) = `(fold (fn (acc p) (f acc (first p) (second p))) init (map-pairs m))`
- `empty?`/`count` on maps now go through `map-pairs` too.

This kills the regression from the prior pass: `vals` was `(map (fn (k) (get m k)) (keys m))`
— a `get` per key, O(n²) on the association-vector rep. Folding over `map-pairs`
is one pass, O(n). The map kernel stays five primitives
(**hash-map, map-get, map-assoc, map-dissoc, map-pairs**) — construct, read, two
producers, one enumerator — and the rep is still swappable (ADR-030): nothing
Brood-side peeks past these.

**Bug fix: docstrings on functions with a destructured parameter.** `(defn f
([x y]) "doc" body)` dropped its docstring — the single-clause pattern-param path
in `lower_fn` (`eval/macros.rs`) wraps the body in a refutable-bind `do`, so the
leading string was no longer the closure body's first form where `make_closure`
looks for it. Fix: peel a leading docstring (string + more body) before lowering
and re-insert it as the lowered `fn`'s first body form. (Hit by `neighbours` in
life.blsp.) Multi-clause docstrings remain unsupported — separate, pre-existing.

**`examples/life.blsp`.** `step` now folds straight over the cell→count map with
`reduce-kv` (was `(keys counts)` + a per-cell `(get counts cell)` — the very
double-lookup `map-pairs` removes). `neighbour-counts`/`neighbours` were already
simplified (the latter is the destructured-param docstring case now fixed).
Verified: blinker oscillates (period 2), glider walks SE over 50 gens, the hatch
call/cast server replies.

**Tests.** `tests/maps_test.blsp` gains a "map-pairs & reduce-kv" block (entries
order, falsy values through keys/vals, `reduce-kv` folds incl. empty-map seed);
`tests/introspection_test.blsp` gains a destructured-param-keeps-docstring case.
`cargo test` green: 53 Rust + **339 in-language**, no warnings.

**Docs.** `docs/primitives.md` (the `map-pairs` row + note), `docs/language.md`
(the maps table gains `reduce-kv`).

---

## 2026-05-27 — Design: cross-file xref via the image, not a static index (ADR-031)

**Decision (no code yet).** Recorded [ADR-031](decisions.md#adr-031--cross-file-xref-is-an-image-query-not-a-static-index-record-def-sites-at-load-time). The question
was how `brood-lsp` should resolve names across `require`d modules. Rejected the
rust-analyzer-style static workspace-indexer as the *primary* path: Brood is an
image-based, hot-reloadable Lisp (ADR-013) whose endgame is an editor that *is* a
running Brood image, so the runtime already knows every loaded module's globals
for certain — a static index only re-derives that approximately and can't see
through macros or computed `require`s.

**Plan.** Cross-file = SLIME/CIDER/xref model. Record `name → (file, span)` at
load/`def` time into the shared `RuntimeCode` region (span-accurate for *defs*
because it's captured before macroexpansion), expose `(source-location 'foo)`,
and have the server fall back to that image lookup for names that resolve `Free`
in the buffer. Definitions go image-based; "find references" stays CST/source-
level (macro-generated code has no faithful spans). The server stays a hybrid —
CST for the live half-typed buffer, image for everything loaded. Cost accepted: a
loaded image (opt-in, gated; safe single-file features never depend on it) and
staleness between edit and reload. Updated `docs/lsp.md` §Cross-file and the
roadmap to match.

**Next concrete step.** The `source-location` primitive (foundation; useful for
error provenance / `nest` / a self-hosted REPL `M-.` on its own), then wire the
server's `Free`-name fallback to it.

---

## 2026-05-27 — Dynamic variables (`defdyn` / `binding`)

**Goal.** Close the last open Tier-1 language gap (ROADMAP.md): Lisp special
variables for config-style knobs — declare a default, override it for a dynamic
extent, restore on exit.

**Design.** Kept the core small and Brood-first (ADR-006/011), reusing the
`try`/`catch` shape — surface macros over a tiny primitive kernel, **no new
special form**:
- **Reads** resolve through a **per-process dynamic binding stack** living in the
  `Heap` (not a Rust thread-local — green processes migrate between workers). The
  stack is consulted in `env_get` *only at the `EnvId::GLOBAL` step* and *only
  when non-empty*, so it costs nothing when no `binding` is active and shadows a
  var precisely where it resolves (dynamic vars are never lexically bound).
- **Per-process by construction.** The stack is in the process's own heap, so a
  `binding` never crosses a `spawn` — a child starts from the declared defaults
  (consistent with share-nothing). A process that crashes mid-`binding` drops its
  stack with its heap and perturbs nothing.
- **Declared, not implicit.** `defdyn` marks the symbol dynamic in a process-wide
  registry (a `static` set, like the interner — a monotonic declaration fact, not
  per-runtime state); `binding` rejects an undeclared var (catches typos, gives
  `defdyn` real meaning). `dynamic?` reports it.
- **Restore on unwind.** `%binding` mirrors `%isolate`: push → `apply` thunk →
  pop, popping on both `Ok` and `Err`, so a throw out of the body still restores.

**Built.**
- `value.rs`: `DYNAMICS` registry + `mark_dynamic`/`is_dynamic`.
- `heap.rs`: per-process `dynamics` stack (`push_dynamic`/`pop_dynamic`); the
  dynamic-aware branch in `env_get`.
- `builtins.rs`: `%declare-dynamic`, `%binding`, `dynamic?` primitives (+ an
  `expect_symbol` helper).
- `std/prelude.blsp`: `defdyn` / `binding` macros + expand-time `binding--names`/
  `binding--vals` splitters.
- Tests: `tests/dynamic_test.blsp` — single-process semantics (default, late
  resolution, nesting, multi-var, restore-on-throw, validation) **plus** an
  `:isolated` across-processes block proving no cross-talk under contention (20
  workers each `binding` a distinct value, fan-in), that a parent's binding never
  leaks into a `spawn`ed child, and that **one process crashing inside a
  `binding` leaves the rest computing correctly**. Rust smoke tests in
  `crates/lisp/tests/basic.rs`.
- Docs: new "Dynamic variables" section in `docs/language.md`; both roadmaps
  ticked; [ADR-032](decisions.md).

**Concurrent-edit note.** The tree moved under this work (the symbol interner was
rewritten to a lock-free `boxcar` `NAMES` + `Mutex` `IDS`, and a `def_sites` table
landed for ADR-031 xref); the dynamic-var additions merged cleanly with both.

---

## 2026-05-27 — source-location primitive + hover documentation (stdlib & primitives)

**source-location (ADR-031 foundation).** Loading a file now records where each
top-level `def`/`defn`/`defmacro` was defined — `name -> (file, span)` — into the
shared `RuntimeCode` region (beside the global table, so it's process-shared and
updates on redefinition). Captured *pre-macroexpansion* in the file loaders
(`load` builtin + `eval_source`), so `defn`/`defmacro` (which lower to `def`) are
located by their own form. New primitive `(source-location 'name) -> [file line
col]` (or nil). The CLI now sets `current-file` around `eval_source`, so direct
`brood file.blsp` / `brood --test` runs record sites too (and test/error
locations stop showing the `nil:` prefix). Tests: a Rust load-and-query case +
in-language coverage in `introspection_test.blsp`, including a spawned process
seeing the same site (shared region). Cross-file goto-definition (resolve a
`Free` name against this) is the next step on top.

**Hover documentation.** Made `(doc …)`/hover work for the whole public surface:
- *Primitives* now carry docs. `NativeFn` gained `params: &'static [&str]` +
  `doc: &'static str`, filled at registration from a new `PRIMITIVE_DOCS` table
  (one row per public builtin, mirroring `docs/primitives.md`). `doc`/`arglist`
  read them, so `(doc cons)` and `(arglist cons)` work like a Brood function's,
  and LSP hover shows `(cons x xs)` + the docstring. `&` in the params marks a
  variadic tail (`(vector & items)`), which conveys arity. Internal `%`-prefixed
  primitives are intentionally left undocumented.
- *Stdlib* — added leading-string **docstrings** (the `defn` doc feature) to the
  public prelude functions/macros that lacked them: arithmetic/comparison,
  predicates, list/sequence/map ops, math, the control + threading macros, `try`/
  `defdyn`/`binding`/`match`/`receive`/`for`/`doseq`, string + path helpers, and
  `provide`/`require`. (`foo--` helpers left undocumented, per "public API only".)

All green (`cargo test --workspace`), clippy clean for the touched crates. The
prelude was under concurrent edit (dynamic variables landing in parallel);
docstring edits were additive and behaviour is unchanged (verified by the suite).

---

## 2026-05-28 — `(spawn expr)` and sendable closures (ADR-033)

**Goal (from the user).** *"We must be able to do `(spawn (* (+ 1 1)))` and send
this to another node."* Two coupled language changes (full rationale in
[decisions.md](decisions.md) ADR-033).

**`spawn` now takes one expression.** Renamed the Rust builtin `spawn` →
`%spawn` (arity 1, runs a 0-arg thunk) and added a prelude macro
`(defmacro spawn (expr) `(%spawn (fn () ~expr)))` — the `try`/`%try` pattern, no
new special form. The old `(spawn f arg...)` form is gone; locals are now captured
lexically by the thunk rather than passed as positional args. **`(self)` moved
with it:** the body runs in the child, so `(self)` *inside* `spawn` is the child's
pid — capture the parent's first, `(let (me (self)) (spawn (worker me)))`.

**Closures serialise into a `Message`.** Reversed the old "you can't send a
function." `to_message`/`from_message` round-trip a `Value::Fn`: the body and
optional-default *forms* go as data (they already are S-expressions), the **free
locals the body actually references** are copied (collect the symbols it mentions,
keep those that resolve to a *local* binding via `Heap::env_frame_snapshot`), and
**free globals are not copied** — they re-resolve on the receiver. So a closure
runs on any node with the same definitions. Copying free vars rather than the whole
frame matters: it keeps unrelated (possibly unsendable) siblings out, and — found
the hard way, via a stack overflow on a closure capturing a sibling closure — it
breaks the cycle a closure→defining-frame→closure walk would otherwise loop on. A
self-referential *local* closure is rejected cleanly (define it at top level
instead); builtins (`Value::Native`) and macros still can't be sent. Local `spawn`
is unchanged in cost — it still `promote`s into the shared RUNTIME region;
serialisation is the *node* path, exercised locally by `send`ing a closure between
processes (the new `:isolated` "sending closures (mobile code)" block).

**Concurrent with the node-link work.** This landed alongside the user's
node-distribution layer (node-tagged `Value::Pid { node, id }` + `crate::dist`).
The two interlock at `Message`: a pid travels as `Message::Pid`, a computation as
`Message::Closure`. Node identity, the wire codec, and cross-link `send` dispatch
are `crate::dist` (decided separately). Build was intermittently red on the
in-flight `dist` module during this work.

---

## 2026-05-28 — Distributed nodes, slice 1: connect two runtimes (ADR-034)

**Goal.** "We need a feature to connect two nodes (two runtimes)." The smallest
useful slice (ADR-011): two `brood` processes connect over TCP and message each
other. The design intent was already in `concurrency.md §Distribution` and ADR-033
deferred exactly this ("node identity + wire transport … decided separately").

**Pids became a value.** `Value::Pid { node, id }` (+ `Tag::Pid`, `pid?`,
`#<pid node/id>`, the `types` lattice entry) replaced bare-`Int` pids everywhere —
`self`/`spawn` return one. Mechanical, following the `Value::Ref` template: pids
are used opaquely in Brood (send targets, message payloads, `[:down …]`), so no
Brood code needed changing beyond the representation. A *local* pid carries this
node's name (`:nonode` before `node-start`), a *remote* one the peer's — so the
**same value addresses a process anywhere** and `send` dispatches on the node part.

**The node layer (`crate::dist`).** `node-start`/`connect` over `std::net` (no new
dep): a cookie-authenticated `Hello` handshake, then per-connection reader +
writer OS threads *off* the green-process scheduler — an inbound message lands via
the same `process::deliver` an in-process `send` uses. Routing: local node →
deliver in-process; remote → encode a `Send` frame to the peer's writer. Bootstrap
a peer by `(register name pid)` + a `{:name :node}` address; once it replies with
`(self)`, talk to that **remote pid** directly. Wire codec is hand-rolled and
length-prefixed, reusing the `Message` deep-copy — with the key cross-process
detail that **symbols (a pid's node, keywords) travel by name and re-intern** on
arrival (separate interners). The codec rejects a `Closure` for now (remote spawn
is the next slice — the ADR-033 machinery is the missing half).

**Tested.** Codec round-trip + cookie accept/reject as Rust unit tests in
`dist.rs`; a new `tests/pids_test.blsp` covers the local pid invariants
(`:isolated`, so across the per-process heap boundary); and a genuine **two-process
end-to-end test** (`crates/cli/tests/distribution.rs`) launches two `brood`
subprocesses over loopback, reaches `:echo` by name, then round-trips via the
remote pid — plus a bad-cookie rejection. (Built/verified green before the tree's
concurrent closure-sending edits; the suite shares `Message` with that work.)

**Scope / deferred.** One node per OS process (node state + interner are
process-global). Deferred: remote `spawn`/code shipping, distributed
monitors/links, node-down detection, reconnect/net-split, real auth/TLS (the
cookie is a placeholder). Full reference: [distribution.md](distribution.md).

---

## 2026-05-28 — receive loops are now TCO'd (coroutine-stack overflow fix)

**Bug.** A long-lived server segfaulted after handling enough messages. `%receive`
(`process.rs`) *ran* the matched body thunk via `eval::apply` and returned its
value; a server loop whose handler tail-calls back into `receive` therefore nested
a fresh `receive_match` per message handled (the tail call wasn't TCO'd across the
native boundary), growing the green-process ~128 KB coroutine stack until it
overflowed (SIGSEGV). Surfaced by `examples/life.blsp` (animator drives the
life-server 45× cast+call → crash ~gen 26) and reduced to a raw repro: interleaved
`send :inc` + `send [:get me]`/receive to one process crashes ~60 cycles (pure-cast
×200 and pure-call ×200 were fine — the interleave makes the server handle a queued
message *without suspending*, so frames accumulate).

**Fix — a trampoline.** `%receive` now **returns** the matched (or timeout) body
thunk instead of running it, and the `receive` macro applies it in tail position:
`((%receive matcher ms on-time))`. Eval's existing `'tail` loop then applies the
thunk in tail position, so the handler's tail-call back into `receive` loops in
**O(1) native stack**. `receive--split` always supplies a do-nothing timeout thunk
so the wrapping application always has a fn. Behaviour is unchanged (the receive
form still evaluates to the body's value; `after`-timeout throws still propagate
through `try`/`catch`).

**Tests.** `tests/concurrency_test.blsp` — a server handling 500 interleaved
cast+call cycles without overflowing. Full suite green; `examples/life.blsp` runs
all 45 generations.

---

## 2026-05-28 — Distributed nodes, slice 2: connection lifecycle + liveness

**Goal.** Make the node link sturdy enough to leave running. A critical review
of slice 1 surfaced several latent issues (pid-id `u32` truncation, decoder OOM
vectors, a lock on the hot `send` path, half-open thread leak on writer death) —
those were fixed first; this entry covers slice 2 proper.

**Generation-checked teardown.** Each `Conn` carries a `u64` generation id + a
shared `Arc<TcpStream>`. Any trigger — peer close, read/write error, tie-break
eviction, heartbeat down — `shutdown`s the socket; the reader unblocks and
`drop_link(peer, id)` removes the `NODES` entry *iff* the stored id still
matches, so an evicted link can't clobber its replacement. One idempotent
teardown path; both threads + socket freed exactly once.

**Connection de-dup + tie-break.** `connect` pre-checks `NODES` and reuses an
existing link to the claimed name (no redundant dial). For a real
simultaneous-connect race, `establish` resolves it under the `NODES` write lock
with a deterministic tie-break: the link whose connector has the
lexicographically smaller node *name* (the spelling — interned ids differ across
processes) wins; the loser's socket is shut down and never registered.

**Node-down detection.** Two new 5-byte wire frames (`Ping`, `Pong`), one shared
heartbeat thread (started on the first link). Every 2 s it snapshots `NODES`
under the read lock and either declares each link down (silent past 6 s) by
`shutdown`ing its socket, or sends a `Ping`. Every inbound frame refreshes
`last_seen`, so active traffic *is* its heartbeat — probes are idle-gated, never
per-message. Detection funnels through the same teardown, which fires
`[:nodedown name]` to every process that called the new `(monitor-node name)`
primitive. Clean peer exits fire nodedown immediately via reader EOF; heartbeat
covers the hard-down case.

**Tests.** Two new e2e tests in `crates/cli/tests/distribution.rs` —
`duplicate_connect_is_deduplicated` asserts `(nodes) = (:a)` after two
`connect`s; `node_down_is_detected` does a `:welcome` round-trip (proving link +
monitor are up), asks the peer to exit, and waits on `[:nodedown :a]` within
10 s. All four e2e tests pass; brood unit suite, in-language suite, codec
tests, and doc test stay green.

**Still deferred.** Handshake v2 (protocol version + constant-time
challenge–response in place of the plaintext cookie compare). Documented in
[distribution.md](distribution.md) §3.

---

## 2026-05-28 — Per-process tracing GC (ADR-035)

**Goal.** Close the last hole in the memory model: a long-running process — a
spawned server, a `(spin)` benchmark — has no top-level boundary, so
arena-reset (ADR-016) can't help it. Memory grows linearly with iteration
count. Bounding that requires a real tracing collector.

**What the docs anticipated.** ADR-016 and `memory-model.md` flagged this as
"the biggest blast radius of any change so far," coupled with an
explicit-operand-stack VM rewrite (the doc's predicted cost). The reasoning:
our recursive evaluator holds live `Value`s on the *Rust* call stack where a
GC can't find them, and that seemed to force a stepping-VM refactor of `eval`
plus pervasive rooting of every builtin's transient accumulators.

**The cheaper path I found.** Stackful coroutines (ADR-018) shipped the
suspension story instead of a stepping VM, so we're not actually forced to
rewrite eval to suspend. And the **trampoline** structure of the evaluator —
the `'tail: loop` — gives us a moment, per iteration, where the active eval
frame's loop-body locals (`head`/`rest`/`callee`/`argv`/`scope`) are dead and
only `expr`/`env` persist. That moment is a precise safepoint where the root
set is trivially small *if* we ensure no other eval/macroexpand frame is on
the stack. So:

- A thread-local **`GC_BLOCK` depth counter**, incremented by RAII guards at
  every `eval()` and `macroexpand_all()` entry. GC fires only when this is `1`
  ("we are the outermost contributor — no other eval or macroexpand frame
  holds an unrooted LOCAL transient"). Saved/restored around coroutine
  suspend (`process::preempt`, `process::wait_for_message`) and reset to 0 at
  coroutine entry, so workers multiplexing processes don't leak depths.
- At `GC_BLOCK == 1`, the roots are: `expr`/`env` (passed in by the
  safepoint), `Heap::dynamics` (the `binding`-form stack), and an explicit
  `Heap::roots` `Vec<Value>` used by exactly two sites (`eval_str`,
  `eval_source`) — they hold a `Vec` of unevaluated forms across the
  outermost eval, the *only* depth-0-reachable transient surface.
- **Completeness argument.** At `GC_BLOCK == 1`: (a) the eval's own loop-body
  locals are dead at `continue 'tail`, leaving only the rooted `expr`/`env`;
  (b) no other eval/macroexpand frame is active by the invariant; (c) a
  builtin mid-execution implies its calling eval is blocked in `call_native`,
  not at its safepoint — GC and builtin transients are mutually exclusive on
  the stack; (d) the only depth-0 caller besides the coroutine body is
  `eval_str`, whose forms are pushed onto `Heap::roots`. So every live LOCAL
  handle is reachable from the union. ∎

**The collector.**
- Non-moving mark-sweep (handles stay stable across collection — a Rust local
  holding a rooted handle stays valid even though the slab around it was
  swept).
- Per-LOCAL-slab **free lists** (`pairs`/`vectors`/`maps`/`strings`/
  `closures`/`envs`); `alloc_*` pop a free index before extending.
- **Iterative** mark via an explicit `Vec<TraceItem>` worklist, so a deep
  cons or env chain can't overflow the native stack. PRELUDE/RUNTIME handles
  are filtered at the push site — the trace never leaves LOCAL.
- **Sweep** rebuilds the free lists as `(0..len) \ marked`, clears dead
  vector/map/string/closure/env slots (releases their inner allocations),
  and purges `form_pos` entries for freed pair slots.
- **Adaptive threshold:** `gc_threshold = max(GC_FLOOR, 2 * live)` after each
  collect. `BROOD_GC_STRESS=1` floors it at 0 — GC at every safepoint.

**The blast radius, in lines.** `heap.rs` grew by ~330 lines (free lists +
collector + root API); `eval/mod.rs` got one RAII guard and one safepoint
check; `eval/macros.rs` got one guard; `process.rs` got `GC_BLOCK` + the
save/restore at the two suspend sites + the coroutine-entry reset;
`lib.rs::eval_str`/`eval_source` push the forms vec onto `Heap::roots`. Zero
new dependencies. **Zero rooting** in any builtin. That was the
doc-anticipated cost that turned out to be unnecessary.

**Verified.** All 158 existing tests pass under `BROOD_GC_STRESS=1` — GC
fires at every outermost-eval safepoint, maximising free-list churn. New
`crates/lisp/tests/gc.rs`: a 200k-iteration tail-recursive loop allocating
cons garbage stays bounded under 64k live objects (in practice it's a few
hundred); the same loop inside a `spawn`ed green process passes (exercising
the coroutine save/restore path); a server-style `receive` loop processing
20k messages stays bounded.

**What's deferred.** A program that perpetually stays at `GC_BLOCK > 1`
(e.g. a server wrapped in `(try (loop) …)` — `%try` holds the outer eval
blocked) won't GC until it unwinds. Idiomatic Erlang `try`s within an
iteration, not around the whole loop, so this is rare. Fix is incremental
when needed: add explicit rooting to the few builtins that hold transients
across eval. Slabs don't shrink trailing dead runs — the free list reuses
indices instead; high-water `len` stays. (Trailing-truncate is a small
future win.)

---

## 2026-05-28 — Types Step 3: sigs on `NativeFn`; one-step closure inference

**Goal.** Finish the type-system Step 3 from `docs/types.md`: stop maintaining
a parallel `primitive_sig` table in the checker, put the source of truth on
`NativeFn` (compatibility-contract #6, *enforced*), and add the narrow
inference rule for straight-line single-expression closures so user `defn`s
like `(defn inc (x) (+ x 1))` participate without a hand-written sig.

**Built.**
- **`types::Sig`** in `crates/lisp/src/types/mod.rs`: `params: Vec<Ty>` + `rest:
  Option<Ty>` + `ret: Ty`, with `Sig::new`/`nullary`/`variadic`/`with_rest`/
  `any` builders. `Vec<Ty>` (not `&'static`) so the same type works for
  static primitive declarations *and* inferred closure sigs built at check
  time. The previous private `Sig` inside `check.rs` is gone.
- **`NativeFn { …, sig: types::Sig }`** in `core/value.rs`: required field,
  no default — adding a builtin without a sig is a compile error. The "no
  useful info" case is the explicit `Sig::any()` lane (`(...any) -> any`),
  which still satisfies the contract while the checker's disjointness test
  never warns against it (`ANY` overlaps every inhabited type).
- **Every primitive declared** in `builtins.rs::register` — ~60 sigs, sourced
  from each primitive's actual runtime acceptance:
  - numeric kernel (`%add..%div %lt %eq rem floor`),
  - pair/vector/map/string kernels (the discriminating ones — `vector-ref:
    (vector,int)→any`, `string-length: (string)→int`, `substring:
    (string,int,int)→string`, …),
  - I/O / reflection / introspection (`type-of: (any)→keyword`, `print:
    (...any)→nil`, `now: ()→int`, …),
  - filesystem (`slurp: (string)→string`, `getenv: (string)→string|nil`, …),
  - control / dynamics / processes / distribution (thunks typed as
    `fn|native`, `send` taking `pid|map`, `throw: (any)→never`, …).
  Refined returns where they matter: `string->number: (string)→number|nil`,
  `getenv` / `current-file` / `doc` / `source-location` / `form-pos` return
  `T|nil` so a downstream call on the nil case doesn't claim "found", but
  also doesn't false-positive on the inhabited case.
- **Checker refactor** (`types::check`):
  - `primitive_sig` now looks the name up via `heap.env_get(heap.global(),
    sym)` and reads `heap.native(id).sig` — no parallel table. Works in both
    the prelude builder (local global env) and the real runtime
    (`EnvId::GLOBAL` routed to the shared globals table) because
    `heap.global()` returns the right one in each.
  - `curated_sig` kept for the variadic / `reduce`-based / higher-order Brood
    closures (`+ - * / < <= > >= mod map filter reduce`) — hand-vetted, sound.
  - **`infer_sig`**: a closure with `body.len() == 1`, no `&optional`/rest,
    whose single expression is a call to a known primitive/curated sig — each
    closure parameter inherits the callee's expected type at the positions
    where the parameter is passed directly (intersected across positions);
    the closure's return is the callee's. Skips recursion (self-name match)
    and only consults the *non-inferring* `primitive_sig`/`curated_sig` so a
    mutual chain `defn a (x) (b x)` / `defn b (x) (a x)` can't loop. Sound
    because a straight-line use is unconditional — no control-flow analysis,
    no fixpoint, no false-positive class.
  - `sig_of(heap, name)` is the three-tier lookup the walk uses (primitive →
    curated → inferred).
- **Tests.** 6 new in `types::check::tests`:
  - `primitive_sigs_are_read_from_native_fn` — the contract: `string-length`'s
    sig in the checker *is* the `Sig::new(vec![string], int)` declared in
    `builtins.rs`. If the field is ever dropped or the value drifts, this
    catches it.
  - `infers_a_straight_line_wrapper` — `(defn inc (x) (+ x 1))` then
    `(inc :k)` warns.
  - `inferred_return_type_propagates` — `(string-length (inc 1))` warns
    (inferred `inc: (number)→number`; `nil`-ish return would be caught too).
  - `inferred_params_intersect_across_positions` — `(defn add (x y) (+ x y))`,
    `(add "a" 2)` warns on `x`.
  - `does_not_infer_through_branches_or_lets` — a body with `if` or `let` is
    *not* straight-line; inference skips, no warning emitted (zero false
    positives from the lack of control-flow analysis).
  - `does_not_infer_through_recursion` and
    `skips_inference_for_variadic_or_optional_closures` — the explicit
    skip cases.

  Existing tests adjusted: the bare `Heap::new()` in `warnings()` is now a
  `heap_with_primitives()` (builder heap with `builtins::register`'d into it),
  since primitive sigs now live there rather than in a static table.

**Verified.** `cargo test`: 51 + 56 + 3 + 44 + 1 + 6 pass (the lisp unit
suite, integration tests, distribution, LSP, etc.). `make suite`: 379
in-language tests, 379 passed. `cargo build` clean.

**Docs.**
- `docs/types.md`: Step 3 marked ✅ with the three-source breakdown, the new
  `infer_sig` rule and its skipped cases, and the example sigs updated to
  reflect what's now declared. Compatibility contract point #6 is now
  **(enforced)** — the "Will be **(enforced)** once `NativeFn` carries the
  field" hedge is gone.
- `docs/roadmap.md`: the types bullet updated — Step 3 ✅; Step 4 still 🟡
  (the disjointness walk ships, but guard narrowing / unbound / arity
  diagnostics are the remaining behavioural payoff).

**What's left in Step 4.** Guard narrowing via `Ty::tested_by` (the bridge
exists in `types/mod.rs` — predicates like `int?` already map to `Ty::of(Int)`
— but no consumer yet); unbound-symbol and arity diagnostics in the checker
(today's checker only flags primitive type misuse, per `docs/lsp.md`); and
auto-running the checker in `brood <file>` / `nest test` / `nest check` (only
`brood --check` exists). Step 5 (structured types) replaces the `u16` bitset,
so it stays deferred to a concrete need (ADR-011).


---

## 2026-05-28 — Types Step 4: guard narrowing + let-binding tracking

**Goal.** Wire `Ty::tested_by` (already built in `types/mod.rs`) into the
checker so a type predicate in an `if` test *narrows* what the variable can be
in each branch, and start tracking `let` bindings so a literal-typed RHS gives
the checker something to flag. Both are the second behavioural payoff in the
types roadmap; both fall out of threading a small `Ctx` through the walk.

**Built.** `crates/lisp/src/types/check.rs`:
- A `Ctx { types: HashMap<Symbol, Ty> }` plumbed through `expr_ty` and
  `check_into`. Two operators: `narrow(sym, ty)` *intersects* (a guard
  refinement of the same lexical variable) and `bind(sym, opt_ty)`
  *overwrites* (a fresh let-bound shadow — `None` clears the slot so an
  unknown RHS does not let the outer narrowing leak through).
- `expr_ty(form, &Ctx)` now resolves `Value::Sym(s)` via `ctx.get(s)` — a
  free / global reference still returns `None` and is never flagged.
- `check_into` special-cases `if` and `let`/`let*` before falling through to
  the generic "call-with-sig" path:
  - `check_if(items, ctx)`: checks the test in the outer ctx, then descends
    into `then` / `else` with `ctx.narrow(sym, ty)` / `ctx.narrow(sym,
    ty.negate())` when `guard_assertion(test)` recognises a `(pred? sym)` or
    `(not (pred? sym))` shape. Missing branches default to `nil` (matches the
    evaluator).
  - `check_let(items, ctx)`: walks bindings sequentially (matching the
    evaluator), checks each RHS in the in-flight ctx, then `bind`s the new
    name with `expr_ty(rhs, ctx)` for the body. Pattern-target binders are
    skipped (not warned), since the Step-4 work is plain-symbol locals only.
    `[name val …]` vector binding shape is recognised alongside `(name val …)`.
- `guard_assertion(test)`: matches `(<pred?> <sym>)` and `(not <inner>)`,
  returning `(sym, Ty)` — the type `sym` provably has when the test is
  truthy. Anything else returns `None`, so unrecognised guards never narrow.

**Tests.** 14 new in `types::check::tests`, covering the basic cases
(`(let (x 1) (first x))` flags, `(if (int? x) (first x) nil)` flags); the
no-false-positive boundary (`(if (int? x) nil (first x))` stays silent, since
the else-branch is `not int` which overlaps `list|vector`); the shadow rules
(an inner `let` with an unknown RHS clears an outer narrowing — `(let (x 1)
(let (x foo) (first x)))` must *not* warn); negated guards flipping; nested
guards composing to e.g. `float` (= `number ∩ ¬int`); the vector binding
shape; and `let*` going through the same path. All 32 `types::check` tests
pass — 18 existing + 14 new.

**Verified.** `cargo test` green across the workspace; `make suite` → 379
in-language tests, 379 passed.

**Docs.** `docs/types.md` Step 4 bullet updated (the ⬜ guard-narrowing item
is ✅; unbound/arity diagnostics still ⬜). `docs/roadmap.md` types bullet
edited the same way.

**What is still ⬜ in Step 4.** Unbound-symbol diagnostics and arity
diagnostics in the checker, plus auto-running the checker in `brood <file>` /
`nest test`. Cond-/match-/and-/or-chained guards are also deferred — they
expand to `(let (g …) (if g …))` shape macros where the `g` is the test's
*result*, not the variable being narrowed, so recognising them needs either
pre-expansion handling or post-expansion shape pattern-matching through the
gensym. Both are tractable; neither is on the critical path.

### Followup: serialise the distribution test ports (same day)

Spotted while running `cargo test` repeatedly: a flake on
`crates/cli/tests/distribution.rs`. Two tests in the file call `free_port()`
(bind `:0`, take the port, drop the listener) and then `spawn_brood` to bind
that same port in a child. Run in parallel, both can pick the *same* freed
port — the loser's child fails to bind, `wait_until_listening` happens to find
the winner's listener, and the loser's client times out with `ECONNREFUSED`.

Fix: a file-local `static PORTS: Mutex<()>` plus a `port_lock()` guard at the
top of each test. Tests now serialise *against each other*; they still run
concurrently with every other test binary. 5x `cargo test` after the change:
0 failed suites across the workspace each time (previously flaked roughly 1
in 3 runs). No code change needed in the runtime itself — the dedup logic
and tie-break path were both fine.

### Followup: reap killed test processes (same day)

`cargo clippy --all-targets` flagged four `spawned process is never wait()ed
on` warnings in `crates/cli/tests/distribution.rs`. Each test that runs a
server child does `let _ = a.kill();` (SIGKILL) but doesn't `wait()` — so the
child stays a zombie in the process table until the test binary exits. With
`cargo test` running the suite repeatedly (e.g. while debugging the port
race) zombies pile up. Fix: add `let _ = a.wait();` right after each kill.
Output reads from `a.stderr` (in one test) still work fine — after SIGKILL
the pipe buffer drains cleanly. Clippy now reports only the three pre-existing
style warnings on the brood crate.

### Followup: let-bound guard aliases (same day)

Extends the guard-narrowing from the previous entry. The user-written shape
`(let (cond (int? x)) (if cond …))` doesn't narrow `x` with just the
`tested_by`-of-the-test rule, because the inner `if`'s test is the bare
symbol `cond`, not a predicate call. So I added a second table to `Ctx`,
`guards: HashMap<Symbol, (Symbol, Ty)>`, that records "name → (variable,
asserted-type)" when a let-bound RHS is itself a recognised guard. Then
`guard_assertion(Sym, ctx)` looks the symbol up in `ctx.guards`. Six new
tests cover: the basic narrowing in both branches, negation flipping it,
shadowing clears the alias, and self-aliasing (`(let (x (int? x)) …)`) is
rejected (the outer `x` is gone). 38 tests in `types::check::tests` all pass;
`make suite` → 379/379. Brood's immutability is what makes the alias sound —
between the let and the if neither variable can change.

This still doesn't catch the `(and (int? x) …)` form, whose macro expands to
`(let (g_n (int? x)) (if g_n (and …) g_n))` where the outer if's test is the
*let form*, not a symbol. The deferred fix there is either pre-expansion
handling or specifically recognising the macro-output shape.

---

## 2026-05-28 — Types Step 4: arity + unbound-symbol diagnostics

**Goal.** Make the advisory checker say more than "argument 1 expects X" — the
two highest-leverage additions from `docs/types.md`'s Step 4 deferred list:
catch wrong argument *counts* and reference *unbound* names. Both share the
scope infrastructure the guard-narrowing work already laid down; the change is
about wiring, not new machinery.

**Built.**
- **`arity_of(heap, name) -> Option<Arity>`.** One lookup — `NativeFn.arity`
  for primitives, derived from `Closure.{params, optionals, rest}` for Brood
  closures: `min = params.len()`, `max = if rest.is_some() { None } else {
  Some(min + optionals.len()) }`. Works in any heap that has the callee
  bound (the prelude builder, a real `Interp`, a process with later
  `def`s). Returns `None` for non-callable / not-found / a file-local
  `defn` in a `--check` heap.
- **Arity check at the call site.** `check_into` now resolves *both* `sig`
  and `arity` for a known head; when `!arity.accepts(argc)` it adds an
  "expected K, got N" warning. Phrasing handles all three shapes:
  `exact(n)` → "expected 2"; `range(a, b)` → "expected 2 to 3";
  `at_least(n)` → "expected 2 or more". The type check still runs on the
  args that *are* present, so `(first)` and `(first 5)` give distinct, useful
  diagnostics.
- **Unbound-symbol diagnostics** (call heads). A call whose head doesn't
  resolve to *anything* gets a `unbound symbol: foo` warning. The disjunction
  the checker actually computes:
  - not in `Ctx.locals` (fn/lambda/let-bound),
  - not in `Ctx.types` or `Ctx.guards` (narrowed name),
  - not in `Ctx.file_globals` (top-level def name from the same file),
  - not a syntactic keyword (`if`/`do`/`when`/`cond`/`and`/`or`/`match`/`->`/
    `try`/`catch`/`throw`/`spawn`/`defn`/`defmacro`/`defdyn`/`defmodule`/…),
  - and no `Sig`/`Arity` was found from any source (which means the global env
    has nothing either).
  Sound because all five clauses must miss; even a curated-but-not-evaluated
  stdlib name passes the curated-sig clause and so isn't flagged.
- **Scope-aware walk.** New special-cases in `check_into` for `fn` /
  `lambda` (parse params via a new `fn_params` helper that handles `&` /
  `&optional` markers and `(name default)` optional-with-default shapes),
  `def` (skip the binder, walk the value), and `defn` / `defmacro` (bind
  params before walking the body). `fn_params` ignores marker symbols so
  `(fn (x &optional (y 0) & ys) …)` binds `{x, y, ys}` — never `&` or
  `&optional` themselves.
- **`Ctx.locals: HashSet<Symbol>`** + **`Ctx.file_globals: HashSet<Symbol>`.**
  The first records every locally-bound name (separate from `Ctx.types`,
  because a fn-param has no known type but *is* in scope). The second
  accumulates top-level `def`/`defn`/`defmacro`/`defdyn` names across the
  forms in a file — needed because `--check` doesn't evaluate, so a `(defn
  foo …)` at line 1 isn't in the heap when line 100 calls `foo`. New
  `Ctx::bind` now records both `locals` and (optionally) `types`; a fresh
  binding clears the guard-alias entry as before.
- **`check_file(heap, forms: &[Value]) -> Vec<(Option<Pos>, String)>`.** Two
  passes — first sweep `forms` collecting top-level def names into
  `Ctx.file_globals`, then walk each form with the accumulated set. The CLI
  (`brood --check`) now calls this instead of `check_located` per form.
- **`is_syntactic_keyword(name)`** — a single source of truth for "name with
  syntactic meaning but no value to bind", consulted by the unbound check so
  we don't false-flag `cond`/`match`/`->`/`&`/`&optional`/etc.

**Tests.** 11 new in `types::check::tests`:
- `flags_too_few_arguments`, `flags_too_many_arguments`,
  `arity_message_handles_range_and_variadic`,
  `arity_pass_is_silent_for_correct_calls` — the four shapes (exact/range/
  variadic/at-least), in both error and ok directions.
- `flags_unbound_call_heads`, `unbound_is_silent_for_in_scope_names`,
  `unbound_is_silent_for_prelude_names`, `fn_params_with_rest_and_optional_dont_leak`,
  `defn_body_sees_its_params_in_scope` — covers the false-positive risks (fn
  params, let bindings, prelude names, syntactic keywords, `&`/`&optional`
  markers) and the true-positive case.
- `file_globals_make_later_forms_see_earlier_defs` — `check_file` wiring:
  two forms, the second calls the first; no unbound warning even though the
  defn was never evaluated.
- `arity_check_works_for_user_defns_in_a_real_interp` — once a defn is in the
  heap, arity is derivable from its closure (`(inc 1 2)` flagged).

Existing tests adjusted: the previous `warnings()` helper used a
primitives-only `Heap` (no prelude), so the new unbound check would flag
every Brood-defined stdlib name (`list`, `int?`, `zero?`, `inc`, …) — a
false positive specific to that bare setup, not the real one. `warnings()`
now builds a full `Interp::new()` heap, matching how the checker is
actually invoked from `(check 'form)` and `brood --check`. One test
(`does_not_infer_through_branches_or_lets`) had a name mismatch (looped
over `maybe`/`shadow` defns but always called `(maybe :k)`) that the new
diagnostic exposed; fixed to pair each defn with its matching call.
`(first)` is now an arity diagnostic instead of "silently no warning", so
the malformed-forms test was updated to assert the new behaviour.

**End-to-end demo (`brood --check /tmp/check-demo.blsp`):**
```
demo.blsp:5:1: warning: +: argument 2 expects number, got string ("x")
demo.blsp:6:1: warning: first: argument 1 expects nil | pair | vector, got int (5)
demo.blsp:7:1: warning: first: wrong number of arguments — expected 1, got 0
demo.blsp:8:1: warning: string-length: wrong number of arguments — expected 1, got 0
demo.blsp:9:1: warning: rem: wrong number of arguments — expected 2, got 3
demo.blsp:10:1: warning: map-get: wrong number of arguments — expected 2 to 3, got 1
demo.blsp:13:1: warning: unbound symbol: frobnicate
demo.blsp:14:1: warning: unbound symbol: typo-name
```
All three diagnostic kinds firing, with `(fn (x) …)` / `(let (x 5) …)` /
`(defn ok (a b) …)` / `(add 1 2)` (a file-local defn) all correctly silent.

**Verified.** `cargo test`: 78 + 56 + 3 + 44 + 1 + 6 + 1 all green. `make
suite`: 387 in-language tests passed. `cargo clippy -p brood`: no new
warnings (the two pre-existing ones in `dist.rs`/`process.rs` are untouched).

**Docs.** `docs/types.md` Step 4 list gained the two new ✅ entries with the
exact behaviour (the arity-message shapes; the disjunction the unbound check
computes). The "next" line trimmed to what's actually left:
cond-/match-/and-/or-chained guard narrowing, plus auto-running the checker
in `brood <file>` / `nest test` / `nest check`.

**What's left in Step 4.** The macro-expansion-shape gap on `cond`/`match`/
chained-`and`/`or` guards (a leftover from the guard-narrowing work);
auto-running the checker at the file boundaries documented in `docs/types.md`
(only `brood --check` is wired today); and richer LSP wiring (`check_located`
already returns spans, but the LSP server doesn't yet publish semantic
diagnostics — `docs/lsp.md` Tier 2). Step 5 (structured types) stays
deferred — additive, replaces the bitset rep, no concrete pressure to do
it (ADR-011).


---

## 2026-05-28 — Tier 2 ergonomics: letrec, symbol/keyword tools, dotimes/dolist

**Goal.** Close the remaining ⬜ items in `ROADMAP.md`'s Tier 2 in a single
pass so the Stage-1 "full functional Lisp" checklist is done: a `letrec` for
local mutual recursion, the symbol/keyword constructor family, and the
side-effecting loop macros. The ROADMAP entries for `slurp`/`spit` were
already done in earlier work — the file just hadn't been updated.

**Built.**

- **`letrec` special form** (`crates/lisp/src/eval/mod.rs`). Added to
  `SPECIAL_NAMES` and to the dispatch in the eval loop's special-form match,
  next to `let`/`let*`. The implementation matches Scheme's: allocate the
  scope, push `(name, nil)` for every binding (so every name is visible
  during RHS evaluation), then evaluate each RHS in the scope and push the
  real value. Lookups already scan the env frame's association vector from
  the end (`heap.rs`, `env_get`), so the second push wins — no actual
  mutation primitive needed. Closures built in the bind phase capture the
  scope and resolve names lazily at call time, which is what makes the
  mutual-recursion case work. Plain-symbol targets only (pattern targets
  reject with a clear message — letrec exists for named values).
- **`expand_let` covers `letrec`** in the compile pass (`eval/macros.rs`).
  Same binding shape `(name val name val …)`, so the existing helper that
  treats odd positions as values (expand) and even positions as targets
  (opaque) is reused. No pattern lowering branch (letrec disallows them).
- **Types-checker awareness** (`types/check.rs`). Added `"letrec"` to
  `skips_body` and routed it through `check_let` alongside `let`/`let*`.
  Bindings shape is identical; the mutual-visibility nuance doesn't affect
  type-flow because the recursive bodies the form is meant for are functions
  whose typing doesn't get synthesised from within letrec today.
- **Symbol/keyword constructors** (`crates/lisp/src/builtins.rs`). Two new
  primitives:
  - `(symbol x)` — accepts `string | symbol | keyword`, returns a
    `Value::Sym` with the matching spelling. Lenient inverse of `name`.
  - `(keyword x)` — same shape, returns a `Value::Keyword`. Mirrors
    `symbol`; both share the global interner, so `(name 'x)` and `(name :x)`
    return equal strings and the two values' inner `Symbol` ids are equal.

  Sigs declare the union `string | symbol | keyword` → `symbol`/`keyword`,
  so the checker will flag e.g. `(symbol 42)` once it lands in this form's
  position.
- **Strict named conversions** (`std/prelude.blsp`). `symbol->string` and
  `string->symbol` as thin Brood wrappers — single-type input, error on
  anything else. No new Rust: they delegate to `name` / `symbol` after a
  predicate check.
- **Side-effecting loop macros** (`std/prelude.blsp`). Two macros over a
  pair of small tail-recursive helpers (the established `--`-suffix
  convention, see `string->list--acc`):
  - `(dotimes (i n) body…)` — runs body for `i` = 0, 1, …, n-1; returns
    `nil`. Lean: no result list built (`doseq` routes through
    `for`/`mapcat`, which builds and discards one).
  - `(dolist (x xs) body…)` — list-only counterpart. Same shape.

  Both expand to a top-level helper call plus `nil`, so they're tail-safe
  via the evaluator's `'tail:` loop (verified: `(dotimes (i 100000) …)`
  completes without overflowing). `doseq` stays in place for the
  destructuring / `:when`-filter case.

**Tests.** 41 new in-language tests.

- `tests/suite_test.blsp`: a new `letrec` describe block (self-recursion,
  mutual recursion via `even?`/`odd?`, and a 10,000-deep tail-recursive
  local to prove TCO survives), and a `:serial` `loop macros` block
  (dotimes builds 0..4 into a global accumulator; n=0 is a no-op; returns
  nil; dolist walks each item; empty is a no-op; returns nil). `:serial`
  for the loops because they write a shared global counter — `:serial`
  matches the existing `macros` describe block's pattern.
- `tests/symbols_test.blsp` (new): `name` round-trips, `symbol` and
  `keyword` lenient over each of the three input shapes, interning
  (`(= (symbol "abc") (symbol "abc"))`), the shared-interner property
  (`(= (name 'shared) (name :shared))`), and the strict converters
  rejecting the wrong input shapes via `assert-error`.

**Verified.**
- `cargo build` clean (warning-free on the touched files).
- `cargo test`: every suite green across the workspace (Rust lib + the
  integration suites under `crates/lisp/tests/`, the LSP crate's 44
  tests, the 6-test distribution suite, and the lone doc-test).
- `nest test`: **420 in-language tests, 420 passed** (was 379 before
  this work; +41 new).
- `cargo clippy --all-targets`: clean — the two pre-existing
  `type_complexity` warnings on `dist.rs`/`process.rs` are unchanged.

**Docs.**
- `docs/primitives.md`: count bumped 71 → 73; two new rows under
  **Symbols** (`symbol`, `keyword`) next to the existing `name`.
- `docs/language.md`: `letrec` added to the special-form table, plus a
  short paragraph under "Recursion is the loop" with `dotimes`/`dolist`
  examples and a `letrec` example explaining the nil-pre-bind nuance.
- `docs/spec.md`: `letrec` added to the §7 special-form table and to the
  "true core special forms" sentence.
- `ROADMAP.md`: Tier-2 `letrec`, symbol/keyword tools, and `dotimes`/
  `dolist` ticked off; the suggested-order line for Tier 2 marked ✅.
- `docs/roadmap.md`: `letrec` added to the M1 special-forms bullet; a
  new "Tier-2 ergonomics" ✅ bullet summarising the cluster.

**What's done in Stage 1 after this.** Every Tier-1 box was already ticked;
Tier 2's `letrec`, the symbol/keyword tools, file I/O, the loop macros,
modules, the project model, and pattern matching are all ✅. Tier 3 keeps
two ⬜ items: **source locations in errors** (reader drops spans today),
and the wider tracing-GC story (the mark-sweep landed in M1; what remains
is editor-session-scale stress). Everything else past Tier 2 is the M2+
editor work.

---

## 2026-05-28 — `nest run` and a two-module `nest new` skeleton

**Goal.** Make `nest run` work on a folder with `project.blsp` — configurable
which module + which function — and make `nest new` scaffold a multi-file
project so newcomers see how modules wire together from the start.

**Manifest.** `project.blsp` grows an optional `:main` key that names the entry
point. Two shapes:
- `:main 'foo` — module `foo`, fn defaults to `main`
- `:main '(foo bar)` — module `foo`, fn `bar`

Omitting `:main` keeps the default `(main main)`, so a bare manifest just works.
Anything else (a string, a 1-list, a 3-list, a non-symbol component) errors at
manifest load. Parsing lives in `project--parse-main` and is exhaustively
covered by `tests/project_test.blsp`.

**`nest run [args…]`.** A new `"run"` arm in `crates/nest/src/main.rs` collects
positional args after the subcommand, escapes them, and evaluates
`(require 'project) (load-config) (run-project (list "a" "b" …))`. All the
policy is Brood: `run-project` walks from `cwd` to `project.blsp`, calls
`project-setup` (which may override `*project-main*` via the manifest),
`require`s the entry module (pulling in everything it transitively requires),
checks the entry fn is bound and callable, then `apply`s it to the args. Three
clean error paths: no project root, unbound entry fn, non-callable entry —
each surfaces as an editor-parseable error and a non-zero exit.

A nuance worth recording: unlike `nest test`, `nest run` does **not** load all
of `src/` up front. It just `require`s the entry module, which gives a real
proof that the project's `(require 'hello)` wiring works. If a project wants
all sources eagerly loaded, that's `project-load-sources` and they can call it
from `main`.

**`nest new` ships two modules.** Templates switched to `defmodule` (the
post-ADR-029 canonical header — leading-string-docstring + trailing
`(provide …)` is gone) and the scaffold is now five files:
- `project.blsp` (no `:main`, relies on the default)
- `src/main.blsp` — `(defmodule main …)`, `(require 'hello)`, `(defn main ()
  (println (greeting)))`
- `src/hello.blsp` — `(defmodule hello …)`, `(defn greeting () "hello <name>")`
- `tests/main_test.blsp` — asserts `main` is callable
- `tests/hello_test.blsp` — asserts `(greeting)` returns the expected string

The two-file split is deliberately about *showing* the flat module system
(ADR-019): `hello` registers `greeting` in the shared global table, and `main`
just calls it after a `require`. A newcomer immediately has a working example
of "edit one file, the other still works."

**Tests.** 1 new in-language describe (`project: :main parsing`) covering the
symbol form, the 2-list form, the four reject paths, and the
`*project-main*` default. Hand-tested end-to-end: `nest new demo`, then `cd
demo && nest test` (2/2 passes) and `nest run` (prints `hello demo`); plus a
manual `:main '(main run)` override calling `(run "alpha" "beta")` to verify
args passthrough; plus the two error paths (no project, missing entry).

**Verified.**
- `cargo build` clean.
- `cargo test`: every suite green — Rust lib (89), integration (56 + 3 + 1),
  the brood-suite-passes runner (which now includes `tests/project_test.blsp`),
  the LSP crate (44), the distribution suite (6), and the doc-test.
- `nest new demo && cd demo && nest test && nest run` works out of the box.

**Docs.**
- `docs/tooling.md`: new "Running a project: `nest run`" section between the
  test-output and `nest doc` sections.
- `docs/roadmap.md`: the "Project model & test tool" bullet now mentions
  `nest run` and the two-module `nest new` skeleton.

**Why no ADR.** This is a small extension of ADR-020 (project model) and
ADR-028 (the brood/nest split) — the entry-point convention falls out of
"convention over configuration" naturally, and the manifest key is the only
new surface. The devlog plus the tooling.md section is enough; if `:main`
grows shapes later (string `"mod:fn"`, namespaced symbols, an `:args` default),
that's the point to revisit.

---

## 2026-05-28 — `nest format`: a Brood-driven code formatter

**Goal.** Add an opinionated formatter that walks every `.blsp` file in a
project and rewrites it in place. Per the repo's "write the language in the
language" rule (ADR-006), the formatter itself lives in Brood; Rust supplies
only the mechanism it can't bootstrap.

**Substrate.** The lossless, comment-and-whitespace-preserving CST already
existed (`crates/lisp/src/syntax/cst.rs`, built for the LSP). What was missing
was a way to reach it from Brood. Added one builtin:

```
(parse-source "src") -> [:root [child …]]
```

Each node is a vector `[kind …]`. Leaves carry their raw source text so they
round-trip byte-for-byte (`[:symbol "foo"]`, `[:int "42"]`, `[:str "\"hi\""]`,
`[:whitespace "  \n"]`, `[:comment ";; …\n"]`). Reader macros wrap a single
child (`[:quote child]`, `[:quasi child]`, …). Containers carry a child vector
(`[:list [child …]]`, `[:vector …]`, `[:map …]`, `[:root …]`). Errors become
`[:error "raw"]` nodes — never raises; the formatter just ignores them and
re-emits their original text. ~80 lines of Rust in `builtins.rs`.

**The formatter** (`std/format.blsp`, ~280 lines of Brood). One rule:

> Render any form on a single line if it fits within the width budget; otherwise
> break it across lines with each body argument on its own line at +2 indent.

A small `*format-headers*` table (`defn` → 2, `let` → 1, `if` → 1, …) keeps a
fixed prefix of args on the first line of recognised forms, so the body
indents under a sensible header. Comments inside a list force the multi-line
shape and re-emit on their own line at the surrounding indent. Blank lines
between top-level forms (or top-level comments) survive when the author left
one; runs of 3+ blanks collapse to a single blank. Strings with literal
newlines force multi-line on their enclosing form (you can't inline a
multi-line string).

**Idempotency is the contract.** `format-source(format-source(x))` must equal
`format-source(x)` for every input. The "fit one line / else break at +2" rule
makes this easy: once a form fits a line, it always will; once it doesn't, the
break shape is canonical. Verified on a grab-bag plus the prelude (the largest
single Brood file we have, ~1200 lines).

**`nest format`** (~25 lines in `crates/nest/src/main.rs`). Default rewrites in
place; `--check` (or `-c`) just diff-summarises and exits non-zero. The
bootstrap snippet follows the same shape as the other subcommands —
`(require 'project) (load-config) (require 'format) (format-project)` (or
`(format-project-check)`).

**Two design choices worth recording.**
1. **No "align after head" for generic calls.** Some Lisps emit
   `(foo a\n     b\n     c)`; we emit `(foo\n  a\n  b\n  c)` regardless of
   head. The simpler rule is robust under rename — a 3-char head and a 13-char
   head produce the same shape.
2. **No `if`-cascade flattening.** The prelude has hand-aligned cascading
   `if`s (`(if a 1 (if b 2 (if c 3 …)))`); the formatter re-emits them as the
   nested staircase that the source literally is. Rewriting forms is out of
   scope — a formatter shouldn't be a refactor tool. The prelude's pattern
   should be `cond`, which stays flat.

**Caveat: not running it on the brood repo (yet).** A dry-run of
`format-source` on `std/prelude.blsp` would touch ~1170 lines — mostly real
stylistic changes (hand-tuned widths, the `if`-cascade above, occasional
multi-line forms that fit on one line). The change is intentionally a separate
commit when the user wants to opt in; tonight's commit only adds the tool.

**Tests.** `tests/format_test.blsp` — 18 in-language assertions across
trivial inputs, short-form collapsing, long-form breaking, comment
preservation, reader macros + collections, and an idempotency battery that
includes the whole prelude.

**Verified.**
- `cargo build` clean.
- `cargo test` green across the workspace; `brood_suite_passes` now runs
  the new format tests as part of the in-language suite.
- Hand smoke: `nest new demo && cd demo && nest format` rewrites the
  scaffolded files; `nest format --check` returns 0 on the clean tree and 1
  after dirtying any file; `nest test` still passes on the formatted result.

**Docs.**
- `docs/tooling.md`: new "Formatting source: `nest format`" section between
  the `nest run` and `nest doc` sections.
- `docs/roadmap.md`: the project-tool bullet now lists `nest format`.

**Why no ADR.** Extends ADR-020/028 the same way `nest run` did. The shape of
the data the formatter consumes (the CST-as-Brood-data tree) is the only
durable interface decision here; if a second consumer appears (a refactor
tool, a static checker, the LSP), that's the point at which the data shape
deserves its own ADR.




---

## 2026-05-28 — Source locations in errors + auto-running the checker

**Goal.** Close two of M1's loudest remaining ⬜s in one pass: (1) make
runtime errors carry the **innermost** form's `file:line:col`, not just the
enclosing top-level form's start, and (2) wire the advisory checker into
the run-paths so a misuse warns before evaluation begins.

**Built — source locations.**

- `LispError::or_form_pos(heap, form)` — the `or_pos` shape, but driven by
  `heap.form_pos(form)`. Non-overwriting (inner wins); the lookup is only
  on the error path, so the hot path pays nothing.
- The eval loop (`crates/lisp/src/eval/mod.rs`) attaches `or_form_pos` at
  every error-propagation site: `if` test, `def` value, `let`/`let*`/
  `letrec` RHSs, `tail_of_cons` (non-tail body forms), the macro-call
  expansion, the head eval for a non-symbol head, the argv loop, the
  native dispatch, `bind_params`, and the closure non-tail body forms.
  `apply_closure` (used outside the eval loop) gets the same body-form
  treatment. The combination's position (`call_form`) is the fallback for
  primitive errors and arity errors that originate without one.
- The compile pass (`crates/lisp/src/eval/macros.rs`) now carries
  positions through to rebuilt list forms. Without this, a `(when …)`
  body's inner combination loses its `form_pos` to expansion and the
  error falls back to the enclosing top-level. A new helper
  `rebuild_list(heap, original, items)` reads the original's pos and
  re-stamps it on the rebuilt list; `expand_let`/`expand_tail` and the
  default-case rebuild all flow through it.

**Verified — source locations.**

- Six explicit tests in `crates/lisp/tests/basic.rs` cover the matrix:
  `runtime_errors_carry_innermost_form_position` (a `do` body's misuse
  reports the misuse's line, not the `do`'s), `runtime_error_inside_let_rhs_points_at_rhs`,
  `runtime_error_inside_if_test_points_at_test`,
  `position_survives_macroexpansion` (a misuse inside a `when` body
  still points at the source line — guards the new
  carry-through), `located_diagnostic_carries_file_line_col` (end-to-end
  `PATH:3:1: type error: …` from `load`), and
  `eval_str_attaches_position_no_file` (REPL still tags positions, just
  with `file` unset).
- The existing `parse_errors_carry_precise_position` keeps passing —
  parse errors still come from the reader unchanged.

**Built — auto-running the checker.**

- The CLI's `run_check_files` was refactored into `check_one_file(interp,
  path, src, sink)` returning `bool` warned; `run_files` and
  `run_test_files` call it with `CheckSink::Stderr` before each
  `eval_file` so warnings appear before the file's own output. `BROOD_NO_CHECK=1`
  silences the auto-check (uniform opt-out across every entry point).
- A new Rust primitive `(check-file path)` exposes the file-level
  checker to Brood, returning a list of pre-formatted
  `"path:line:col: warning: msg"` strings. The `check_file` walk it goes
  through is exactly what `brood --check` uses.
- `std/project.blsp` adds `(check-project)`: walks every `.blsp` under
  `*project-source-paths*` + `*project-test-paths*`, calls
  `(check-file)` per file, prints each warning to stderr, returns the
  total count. Honors `BROOD_NO_CHECK=1` too. `run-project-tests` and
  `run-project` now call it as a pre-flight after loads.
- `crates/nest/src/main.rs` adds the `nest check` subcommand. Same
  walk, but warnings go to **stdout** and the process **exits non-zero**
  when the count is positive — for CI. The bootstrap `(require 'test)`
  first, so test-framework macros are in the global table before the
  walk (otherwise a test file's `test`/`assert=`/`describe` flag as
  unbound — the checker reads files without executing their `(require
  'test)`).

**Side fix.** Spotted while looking at the auto-check noise: `%receive`'s
declared `Sig` had its last two arg types swapped — `(callable, callable,
int|nil)` instead of `(callable, int|nil, callable|nil)`. The Rust
signature is `(matcher, timeout, on_timeout)`; the macro in
`std/prelude.blsp` matches that order. Wrong sig was producing
~150 false-positive warnings per project that uses `receive`. Sig
corrected; warning count on a `nest test` of this repo drops from
200 → 58.

**Tests.** `cargo test`: every workspace suite green (60 + 89 + 3 + 1 +
1 + 44 + 6 + 1 = 205 Rust tests; the +4 over the prior baseline is the
new position tests, accounting for the two old top-level-pos tests
replaced by their innermost-pos counterparts). `nest test` on the
Brood repo: **439 / 439 in-language tests pass** (up from 420 — the +19
include the position/auto-check coverage). `cargo clippy
--all-targets`: 2 warnings, both pre-existing (`dist.rs:497`,
`process.rs:549`).

**Docs.**
- `docs/tooling.md` — the position-precision section rewritten: runtime
  errors now report the innermost combination, not the top-level form;
  the closure-body / RUNTIME caveat noted (stack trace is M2+). New
  subsection "Auto-running the advisory checker" covers the entry
  points, sinks, exit codes, and `BROOD_NO_CHECK`.
- `ROADMAP.md` — the "Source locations in errors" ⬜ ticked ✅ with the
  carry-through rationale and the RUNTIME-bodies caveat.
- `docs/roadmap.md` — the Step-4 types bullet updated: auto-running is
  done; the only remaining 🟡 piece is `and`/`or`/`cond`/`match`-chained
  guard narrowing.

**What's still ⬜ for Stage 1.** None at the *language-completeness* tier
(`ROADMAP.md`): every Tier-1, Tier-2, and Tier-3 box is now ✅ (modulo
the cross-process stack-trace nuance for closure-body positions, which
is M2+ territory). The remaining M1 work is REPL polish, the
self-host-CLI goal, and Tier-2 LSP wiring — all listed in
`docs/roadmap.md`.

---

## 2026-05-28 — Auto-checker polish: macroexpand walk, scope fixes, sig fixes

**Goal.** With the auto-checker now firing from every entry point
(`brood <file>`, `brood --test`, `nest test`, `nest run`, `nest check`),
shake out the residual false positives a real codebase surfaces. Target:
0 warnings on a clean scaffold and on the project's own suite,
modulo macros that define names at runtime.

**Built.**
- **Macroexpand before walking.** `check_file` (Rust) now `macroexpand_all`s
  each top-level form before the disjointness walk, so threading macros
  (`->`/`->>`), `match` pattern syntax, and any user wrapper (`test`/
  `describe`/`error-of`/`assert-error`) are checked against their *expanded*
  shape. Without this, `(map inc)` inside `(->> xs (map inc))` looked like
  a 1-arg call to `map`; `_` in `(match … (_ …))` looked like an unbound
  symbol; `(cons 1)` inside `(error-of …)` triggered a fake arity
  warning. The accumulator that collects file-local def names is now a
  *recursive* walk over the expanded tree — `defn` nested inside `test`/
  `describe`/etc. still shields a later call, because Brood's `def` is
  global regardless of where it textually sits. Positions survive
  expansion via `rebuild_list`'s carry-through (the common case).
- **`%try` in `skips_body`.** Post-expansion, `(try …)` becomes
  `(%try (fn () body) (fn (e) handler))`. The walk was descending into
  the body and flagging every error the user was *deliberately*
  asserting on (every `(error-of (cons 1))` in the test suite). Adding
  `%try` to the skip list covers `try` / `error-of` / `assert-error`
  uniformly post-expansion (they all expand through `try`).
- **`letrec` pre-binding** in `check_let`. The `(letrec (fact (fn (n) …
  (fact …))) …)` shape needs every binder visible in every RHS, not just
  the prior ones (the mutual-recursion reason `letrec` exists). The
  checker now pre-binds every name to "in scope, type unknown" before
  walking the RHSs, matching the evaluator's nil-pre-bind. `let`/`let*`
  keep their sequential walk.
- **`NEVER`-typed args skipped.** When guard narrowing intersects a
  variable's type down to `Ty::NEVER` (the empty set — unreachable code),
  every disjointness check against it would warn. That's all noise: the
  code can't execute, so there's no real misuse. The walk now skips the
  type check when `arg_ty.is_never()`. Surfaces in pattern-match
  lowering where a guard has narrowed a temp to a type with no
  inhabitants for the current branch.
- **`is_globally_bound` for macro recognition.** The unbound check was
  consulting `sig_of` / `arity_of`, both of which only match `Value::Native`
  / `Value::Fn`. A `Value::Macro` (the test framework's `test` /
  `assert=` / `describe`, every user `defmacro`) fell through both and
  got flagged "unbound symbol". Fix: check `heap.env_get` directly — any
  bound value counts as in scope. The unbound check is independent of
  "is the sig informative".
- **Sig fixes** found by running the auto-check on the real suite:
  - `%builtin-module` accepts symbol *or* keyword *or* string (the
    require flow passes a name via `(name mod)`, a string). Returns
    `string | nil`.
  - `name` accepts string too (it's idempotent on a string).
  - `%binding` arg 0/1 are *sequences* (the macro emits `(quote (*a*
    *b* …))` + `[v1 v2 …]`), not a single symbol/value.
  - `%receive` had positions 1/2 swapped — `(matcher, timeout, on-timeout)`,
    not `(matcher, on-timeout, timeout)`. ~150 fake warnings on any
    project using `receive` (already noted in the previous entry, listed
    here for completeness).
- **`eprint` primitive + `eprintln` Brood wrapper** so `(check-project)`
  can write warnings to stderr without muddling program stdout. Mirrors
  `print`/`println`.
- **`project--ensure-loaded`** in `std/project.blsp`: `(check-project)`
  and `check-project-sources` now project-setup + project-load-sources
  themselves, so `nest check` works standalone (the prior version
  assumed the caller had loaded sources, which `nest test`/`run` do but
  `nest check` did not). Re-running after a loaded setup is idempotent
  (Brood `def` replaces `def`).

**End-to-end.** On a clean `nest new` scaffold: `nest check` is silent
(exit 0); `nest test` runs the auto-check first then the tests (both
silent on a clean project). On the Brood repo's own suite: warning
count dropped from 58 to **1** — the remaining one is `pm-qfac` in
`pattern_matching_test.blsp`, defined by a user `defmacro` (`pm-def-fac`)
whose body isn't visible to `macroexpand_all` because `defmacro` only
registers the macro at *evaluation* time, not at expansion. This is a
known limitation of static checking without evaluation; documented.

**Verified.** `cargo test`: 89 + 60 + 3 + 44 + 1 + 6 + 1 + 1 = 205
Rust tests pass. `nest test`: 439 / 439 in-language tests pass.
`cargo clippy`: 2 warnings, both pre-existing (`dist.rs`, `process.rs`
type complexity).

**What's left in Step 4.** Cond-/match-/and-/or-chained guard narrowing
(the macro-expanded `(let (g …) (if g …))` shape); a way for the checker
to see through `(somemacro defines-this)` patterns (a runtime-defmacro
limitation — currently uncatchable without partial eval). Step 5
(structured types) stays deferred — additive, replaces the bitset rep,
no concrete pressure (ADR-011).


---

## 2026-05-28 — Cross-node closure shipping (ADR-033 wire codec)

**Goal.** Finish the last piece of ADR-033: ship a closure across a TCP link
between two runtimes. Within a runtime the serialiser
(`closure_to_message`/`closure_from_message`) was already complete; the wire
codec in `dist.rs` still had a `return Err("not supported yet")` stub for
`Message::Closure`.

**Built.**
- `crates/lisp/src/process.rs` — `ClosureMsg` fields elevated to `pub(crate)`
  so the sibling `dist` module can read them. They're inert plain data once
  built, so there's no invariant for accessors to defend.
- `crates/lisp/src/dist.rs` — new `M_CLOSURE = 13` tag, plus
  `encode_closure` / `decode_closure` that walk every `ClosureMsg` field in
  the struct's declared order:
  - `Option<Symbol>` and `Option<String>` via two new helpers
    (`put_opt_sym` / `get_opt_sym`, `put_opt_str` / `get_opt_str`) with a
    one-byte `0`/`1` presence tag — cheap and unambiguous in a stream codec.
  - Symbols travel by name (existing `put_sym` — separate runtimes have
    independent interners; the id is meaningless across the wire).
  - Body forms and `&optional` defaults are already `Message`s (S-expression
    data — homoiconicity in action), so they recurse through the existing
    `encode_msg`/`decode_msg`. Code travels as ordinary data.
  - `prealloc(r, n)` clamps every `Vec` allocation to the frame's remaining
    bytes, so a tiny frame claiming a huge `body.len()` can't trigger a giant
    up-front alloc — the decode loop fails cleanly on EOF instead.

**Tests.**
- `crates/lisp/src/dist.rs` — two unit round-trip tests in the existing
  `tests` module:
  - `closure_roundtrips_through_the_wire` — a full-featured `ClosureMsg`
    (name, multi-param, optional with a default, rest, body, doc, captures)
    survives encode → decode unchanged.
  - `closure_with_all_options_absent_roundtrips` — the minimal case (no
    name / no rest / no doc / no optionals / no captures) — guards the four
    `Option<…>` 0/1 tags from mis-aligning when None.
- `crates/cli/tests/distribution.rs` — `lambda_ships_across_nodes_and_runs`,
  end-to-end with two real `brood` subprocesses. A `:worker` on node A waits
  for `[:run f x reply]`. The client on node B builds `(fn (x) (* x n))`
  inside a `let (n 3)` (so `n` is a captured free local that has to ride
  along) and ships it via `send`. The worker applies it, gets `42`, sends
  the result back to the reply pid. Verifies every leg: the body forms
  crossing as `Message::List`, `n` arriving via `captured`, the free
  global `*` re-resolving against the receiver's prelude, and the pid in
  the request routing the result back the way it came.

**Verified.**
- `cargo test`: every workspace suite green. The `dist::tests` count is now
  7 (was 5), the `distribution` integration suite is 7 (was 6).
- `nest test`: **441 / 441 in-language tests** pass.
- `cargo clippy --all-targets`: 2 warnings, both pre-existing
  (`dist.rs:497`, `process.rs:549`).

**Docs.**
- `ROADMAP.md` — "Send functions between processes" 🟡 → ✅ with the
  inside-a-runtime / across-nodes split and the test that proves the latter.
- `docs/distribution.md` — slice-1 "Scope & limitations" updated: the
  closure-as-data path is no longer deferred. The round-trip `[:run f x …]`
  pattern is the working surface; a dedicated `remote-spawn` is a small
  convenience over it.
- `docs/decisions.md` — ADR-034's deferred-list edited the same way: the
  closure shipping is no longer in the "missing piece" hedge.

**What's left in distribution.** Distributed monitors/links (today's
`monitor` is local only), reconnect/net-split handling, a dedicated
`remote-spawn` macro over the `[:run f x reply]` pattern, and the v2
handshake (versioning + challenge–response auth). Each is additive over
slices 1 + 2 + this; nothing in the language core blocks them.

**Connection to today's source-location work.** Closures sent across nodes
land in the receiver's LOCAL heap via `closure_from_message` — which builds
fresh `Pair`s with no `form_pos` entries. So an error inside a remote-shipped
closure today still reports the receiver's call site, not the line on the
sender. The natural follow-up is to thread `(line, col)` through `ClosureMsg`
(eight bytes per pair) so positions cross the wire; the receiver-side
`heap.set_form_pos` API already accepts them. Additive; deferred until a
real diagnostic-quality complaint surfaces.


---

## 2026-05-28 — Distribution slice 3: finish the deferred list

**Goal.** Land the remaining distribution items from ADR-034's deferred list
in a single push: a `remote-spawn` surface, source positions across the
wire, distributed pid monitors with net-split semantics, auto-reconnect on
node-down, and a real authenticated handshake (v2). Constraint from the
user: don't duplicate the local vs. remote code paths.

**Built.**

### 1. `remote-spawn` (Brood)

`std/prelude.blsp` gains `(remote-spawn node expr)`, a thin macro that
`(send {:name :remote-spawn :node node} [:run (fn () expr)])`. The
receiver-side `:remote-spawn` server (also in the prelude) accepts `[:run
thunk]` and `(spawn (thunk))`s it locally. Users opt the receiver in once
via `(start-remote-spawn)` after `node-start`. The closure crosses as
ADR-033 data — free locals ride along, free globals re-resolve. Surface
convenience over the working `[:run f x reply]` pattern.

A new `whereis` Rust primitive backs the idempotent registration check —
one-line lookup in the existing `NAMES` table.

### 2. Source positions across the wire

`Message::List(Vec<Message>)` → `Message::List(Vec<Message>, Option<Pos>)`.
`to_message` reads `heap.form_pos`; `from_message` re-stamps via
`heap.set_form_pos`. The wire codec adds a 1-byte presence tag + (line,
col) as two `u32`s when set. A `put_opt_pos` / `get_opt_pos` pair mirrors
the existing `put_opt_sym` / `put_opt_str` helpers. Verified end-to-end
by `source_positions_survive_a_cross_node_send`: a closure containing a
quoted list literal `'(positioned-marker)` ships to a peer, the peer's
`(form-pos …)` returns the sender's `[line col]`. Closes the
"closure-body-position" gap the previous devlog flagged.

### 3. Distributed pid monitors — *one* MONITORS table

`process::MONITORS` is now `HashMap<u64, Vec<Watcher>>` where `Watcher` is
`Local{pid, mref}` or `Remote{node, pid, mref}`. The local `monitor`
builtin calls `add_monitor(target, Watcher::Local{…})`; the dist-side
`Frame::Monitor` handler calls the **same** `add_monitor(target,
Watcher::Remote{…})`. Same alive/dead branch, same fast-path
`:noproc`, same fan-out from `deregister`. `fire_down` dispatches on the
variant: `deliver` for `Local`, `dist::route` for `Remote` — and the
remote case sends an **ordinary `[:down mref pid reason]` message** to the
peer's pid, so the wire-format `[:down …]` is identical to what an
in-process watcher sees.

Net-split: a sender-side `PENDING_REMOTE: HashMap<Symbol, Vec<…>>` table
remembers "what remote pids am I watching, keyed by peer node". On
`fire_nodedown`, `handle_node_down` flushes the bin and delivers `[:down
mref pid :noconnection]` to each local watcher — matching Erlang
semantics. The peer's stale `Remote` entries in its `MONITORS` table are
dropped by the same call via `drop_monitor` with a `Watcher::Remote
{ node: dying }` predicate.

`Frame::Monitor` + `Frame::Demonitor` are the only new frame types — both
trivial (sender's node + pid + mref). The receiver's frame dispatch
threads each directly into `process::add_monitor` / `process::drop_monitor`,
no duplicated logic.

### 4. Auto-reconnect — Brood policy

`(ensure-link "name@host:port")` in the prelude. Pure policy over the
existing `connect` + `monitor-node` mechanism — no Rust changes. Pattern:

```
(ensure-link addr) → spawn a supervisor that:
  - (connect addr) synchronously once (any error swallowed),
  - (monitor-node peer) — persistent, fires on each transition to down,
  - loop: receive [:nodedown peer] → sleep 200ms → (try connect) →
          retry connect until it succeeds (monitor-node only fires on
          transitions, so we drive the retry off connect's own
          success/failure), then back to receive.
```

Caller gets the supervisor pid; sending it `:stop` shuts the loop down.
Verified by `ensure_link_reconnects_across_a_node_restart`: A1 is killed,
A2 is brought up on the same port + cookie, the client's
`(send {:name :probe :node :a} …)` round-trip works the second time too.

### 5. Handshake v2 — HMAC challenge-response

ADR-034 §3 is now built. Wire format:

- **4-byte magic + version prefix** `b"BRD\x02"` written/read by both sides
  before any frame. A non-brood peer (or wrong version) aborts here with
  `InvalidData` before any frame parsing — guards against accidental wire
  compatibility and gives a clean diagnostic.
- **`Hello { node, nonce: [u8; 32] }`** replaces the old `Hello { node,
  cookie }`. Each side sends a fresh 32-byte OS-RNG nonce; the **cookie
  never travels**.
- **`Auth { mac: [u8; 32] }`** carries `HMAC-SHA256(cookie, peer_nonce ||
  peer_name || 0x00 || my_name)`. Each side verifies the peer's MAC
  constant-time (`ct_eq`); a mismatch is `PermissionDenied` and the link
  never enters `NODES`. Replay defence is the per-handshake nonces.

The Rust crates `hmac` + `sha2` (RustCrypto) plus `getrandom` (OS RNG)
are added — exactly the "vetted substrate" exception to ADR-005 that
crypto is the textbook case for. Wire format breaks v1; this is
greenfield, so we don't preserve compatibility.

`docs/decisions.md` (ADR-034 §Scope) and `docs/distribution.md` updated
end-to-end to reflect the new shape; the §3 "still deferred" hedge is
gone. What remains: Erlang OTP-style **supervision** (`link` + restart
strategies, today's monitor is unidirectional) and optional **TLS** under
the HMAC layer for over-the-internet traffic — both additive.

**Tests.**

- 2 new unit codec tests in `crates/lisp/src/dist.rs`:
  `auth_roundtrips`, `compute_mac_is_symmetric_under_role_flip`. The
  closure round-trip test now also asserts the body form's `Pos` survives.
- 5 new end-to-end tests in `crates/cli/tests/distribution.rs`:
  `remote_spawn_runs_a_thunk_on_a_peer`,
  `source_positions_survive_a_cross_node_send`,
  `cross_node_pid_monitor_fires_down`,
  `remote_monitor_fires_noconnection_on_node_down`,
  `ensure_link_reconnects_across_a_node_restart`,
  `non_brood_peer_is_rejected_at_magic_prefix`.

**Verified.**
- `cargo test`: every workspace suite green. Distribution integration
  suite: **13 tests** (was 6).
- `nest test`: **441 / 441 in-language tests** pass.
- `cargo clippy --all-targets`: 2 warnings, both pre-existing
  (`dist.rs:497`, `process.rs:549`).

**Status of distribution.** Slices 1 + 2 + this third increment cover
everything ADR-034 originally deferred. Distribution in
`docs/roadmap.md` ticked ✅ at the M4 line. Remaining work is supervision
trees and TLS — both additive over a now-complete authenticated, monitored,
auto-reconnecting, closure-shipping link.

---

## 2026-05-28 — Style: lists for code, vectors for data

**Trigger.** External code-style review of `examples/life.blsp` flagged two
inconsistencies. (1) `for` / `doseq` used Clojure-style vector binding forms
while `let` used lists — same language, two conventions. (2) The reader
misparsed `(defn neighbours ([x y]) …)` as a multi-clause wrapper. The form
is correct per ADR-010 — the outer `(…)` *is* the param list — but the visual
collision with multi-clause `(defn f ((p) body))` is real cognitive load
every time. A "consistent but misreadable" form is still a wart; not waved
away.

**Decision.** Two style rules, documented in
[brood-for-claude.md](brood-for-claude.md) §"Style — lists for code, vectors
for data" (ships with the language via `%builtin-doc` and `nest new`):

1. Code uses `( )`; vectors `[ ]` are for tuple values, sequence literals,
   and *patterns* against tuple values inside `match` / `let` / `receive`
   heads. Binding forms (`let`, `for`, `doseq`, `when-let`, `if-let`) are
   lists. Vectors remain accepted at binding sites for leniency (still
   tested in `dynamic_test.blsp:96`).
2. Don't tuple-destructure in a single-clause **top-level `defn`** param
   list — name the param and unpack with `let` in the body. Multi-clause
   `defn` pattern dispatch (lists in clause heads) and tuple-destructured
   params on anonymous `fn` in higher-order context (`(map (fn ([k v]) …) …)`)
   remain idiomatic — the surrounding `(map …)` makes the shape unambiguous
   and the alternative is a noisy extra `let`.

Both rules are about *idiom*, not the language — every form still parses
both ways. The macro side is one-line-safe: `for--build` already normalises
its bindings via `(map identity binds)`, so list and vector forms produce
identical expansions.

**Applied.**
- `std/prelude.blsp`: `for` and `doseq` docstrings and leading comments now
  show list bindings, with an ADR-010 note that vector-acceptance remains a
  leniency.
- `tests/{sequence,hatch,pids,concurrency}_test.blsp`: all `let [ … ]`,
  `for [ … ]`, `doseq [ … ]` converted to `( … )` (~30 sites). Tests that
  *specifically exercise* the vector-form leniency
  (`dynamic_test.blsp:96`, `pattern_matching_test.blsp:324`+,
  `introspection_test.blsp:21`) are left alone.
- `examples/life.blsp`: `neighbours` rewritten to name its param and unpack
  via `let`. Inner `(fn ([dx dy]) …)` kept (rule-2 exception for HOF
  context).
- `docs/language.md`: idiom note added to the `(defn area ([x y]) …)`
  example pointing at the style section.

**Not language change.** No ADR — this is idiom downstream of ADR-010,
not a new design decision. No grammar / parser / type-checker change. No
new macros. Tests not re-run in this session (`crates/lisp/src/dist.rs` was
mid-edit and not compiling); the changes are mechanical and safe pending a
green suite next time the workspace builds.

**Memory.** Saved as feedback for future sessions —
`memory/lists-for-code.md`.

---

## 2026-05-28 — MCP server design + introspect layer extracted

**Goal.** Stake out a per-project Model Context Protocol surface (the
agent-side counterpart to the LSP) and do the safe prep work so the
implementation pass that follows is mechanical.

**Decisions taken.** [ADR-036](decisions.md#adr-036--nest-mcp-a-per-project-model-context-protocol-server-tools-surface-in-brood)
records the shape: a `nest mcp` subcommand (ADR-028 — `nest` is the project
tool), strictly per-project (errors outside a `project.blsp`), one long-lived
`Interp` per session (hot reload, ADR-013, is the headline behaviour), the
tool *surface* declared in Brood (ADR-006 — `std/mcp.blsp` lists eight initial
tools: `eval`, `load`, `lookup`, `macroexpand`, `run-tests`, `check`, `format`,
`processes`; a project's own `mcp.blsp` can extend the registry), JSON-RPC over
stdio with no async runtime (same calculus as ADR-025 choosing `lsp-server`
over `tower-lsp` — `Heap` is `!Sync`), and `nest new` scaffolds `.mcp.json` so
a fresh project is ready for agent-assisted dev from the first commit.
[`docs/mcp.md`](mcp.md) holds the full plan, mirroring `docs/lsp.md`.

**Prep landed.** The load-bearing structural change — extracting the shared
introspection surface so LSP and the future MCP dispatcher can't drift on
"what `map`'s signature is" — is in place:

- `crates/lsp/src/introspect.rs` → `crates/lisp/src/introspect.rs` (now
  `brood::introspect`, exported from the lib alongside `core` / `eval` /
  `syntax` / `types`).
- LSP `use crate::introspect;` flipped to `use brood::introspect;` across
  `completion.rs`, `hover.rs`, `signature.rs`. The local `mod introspect;`
  in `crates/lsp/src/main.rs` is gone.
- Behaviour-identical: the 4 introspect tests now live in the lib, and all
  40 LSP tests still pass.

**What remains** (in implementation order):
1. **Widen `brood::introspect`** with the operations the MCP tools need —
   `source_location`, `macroexpand_to_string`, `check_project`, `run_tests`,
   `format_source`, `eval_in_session` — each total (errors become typed
   fields) and LOCAL-clean (checkpoint/reset around every `eval_str`).
2. **`crates/nest/src/mcp.rs`** — the sync JSON-RPC loop, the tool registry
   loaded from `(mcp-tools)`, and dispatch into Brood handlers.
3. **`std/mcp.blsp`** — the eight initial tools as `defn`s + the registry
   shape projects can extend.
4. **`nest new`** scaffolds `foo/.mcp.json` pointing at `nest mcp`.
5. Tier-1 niceties: `prompts/get` for `brood-task`, project-defined tool
   discovery, then the Tier-2 progress / sandbox work.

**Verified.** `cargo build` clean; `cargo test -p brood --lib introspect`
4/4; `cargo test -p brood-lsp` 40/40. The pre-existing dead-code warning
on `types/check.rs:101` (`aliases` field) is untouched.


---

## 2026-05-28 — Types Step 4 finish: match pattern narrowing

**Goal.** Close the last item in Step 4: chained guard narrowing across the
macro-expanded shapes. A quick survey showed `cond` / `and` / `or` were
already covered by the existing direct-guard and let-stored-guard-alias
paths — the actual gap was `match`, whose pattern compiler lowers to
`(let (m__N x) (if (%eq m__N lit) (do body) …))` and whose body references
the user's `x`, not the internal `m__N`.

**Built.**

Two coupled additions to `crates/lisp/src/types/check.rs`:

1. **`%eq`-as-guard.** `guard_assertion` learns the shape `(%eq sym lit)`
   (and the symmetric `(%eq lit sym)`) — equality against a self-evaluating
   literal asserts the variable has the literal's runtime tag. Strings,
   ints, floats, keywords, booleans, and `nil` qualify; pairs / vectors /
   maps don't (their pieces could be unknown). A new helper
   `literal_eq_guard` keeps the asymmetry tidy. This is what makes the
   *inner* `(if (%eq m__N 5) …)` narrow `m__N` to `:int` in the then-branch.

2. **Let-binding aliases.** `Ctx` gains an `aliases: HashMap<Symbol,
   HashSet<Symbol>>` — an undirected adjacency map. `check_let` records
   `add_alias(name, target)` whenever the RHS is a plain symbol, so a
   `(let (m x) …)` creates the edge `m ↔ x`. `narrow` switches from a
   linear chain walk to a BFS over the equivalence class via a new
   `narrow_chain`, intersecting the guard's type into every visited name's
   `types` entry. With the edge bidirectional, narrowing either side
   propagates to the other — that's what carries the `m__N : int` narrowing
   back onto the user's `x`. `bind` (shadowing) disconnects a name from
   the alias graph entirely (removes its bin *and* prunes the name from
   every neighbour's bin) so a rebinding can't leak through stale
   back-edges. Self-aliases are no-ops.

Combined: `(match x (5 (first x)) (_ nil))` macroexpands to `(let (m__N x)
(if (%eq m__N 5) (do (first x)) (do nil)))`, the let aliases `m__N ↔ x`,
the `(%eq m__N 5)` guard narrows `m__N : int` in the then-branch, the BFS
narrows `x : int` too, and the checker flags `first: argument 1 expects
nil | pair | vector, got int (x)`. Same for keyword / string / bool /
nil-literal patterns.

**Tests.** 6 new in `types::check::tests`:

- `match_literal_pattern_narrows_the_scrutinee` — the headline case.
- `match_keyword_pattern_narrows_the_scrutinee` — same for a keyword
  literal.
- `eq_against_a_literal_is_a_guard` — the recogniser in isolation; both
  `(%eq m 5)` and `(%eq 5 m)` orderings narrow.
- `eq_between_two_variables_is_not_a_guard` — no false positive when both
  sides are variables (asserts nothing).
- `let_alias_propagates_narrowing_in_both_directions` — narrowing `m`
  reaches `x`, narrowing `x` reaches `m`. The bidirectional check that
  drove me to the undirected-set representation.
- `shadowing_clears_an_alias` — `(let (m x) (let (m 5) …))` rebinds `m`
  to int *without* leaking the narrowing back to outer `x` via stale
  edges. Also verifies the inner `(first m)` is still flagged via the
  literal-binding narrowing.

A second test helper, `warnings_expanded(src)`, calls
`macroexpand_all` before `check_form` — needed for tests on `defmacro`s
like `match` (the original `warnings(src)` ran the un-expanded form, which
exposed the pattern syntax `_` as a "free symbol"). Matches what
`(check 'form)` and `check_file` actually do at runtime.

**Verified.**
- `cargo test --lib -p brood types::check`: **55 tests** pass (was 49).
- `cargo test`: every workspace suite green — Rust totals up to ~226 from
  ~220.
- `nest test`: **451 / 451** in-language tests pass.
- `cargo clippy --all-targets`: 2 pre-existing warnings; no new ones.

**Docs.**
- `docs/types.md` — Step 4 status: `🟡` → effectively done. The
  let-binding aliases + `%eq` paragraph added; the "deferred"
  cond/match/and/or hedge replaced with the concrete "all in" bullet.
- `docs/roadmap.md` — Step 4 tick (`🟡` → ✅) with the cluster summary;
  Step 5+ still ⬜ as the next-and-only-remaining types work.

**What's left in types.** Step 5+ (structured types — function arrows,
element types, intersections) remains explicitly deferred per ADR-011.
It replaces the `u16`-bitset `Ty` representation, so it's a chunk of
work; gated on a concrete need, not a checklist item.

---

## 2026-05-28 — MCP step 1b: widened `brood::introspect`

**Goal.** Step 1b of the MCP plan (ADR-036 / `docs/mcp.md`): give
`brood::introspect` the operations the planned `nest mcp` dispatcher will
need — total (errors as typed result fields, never panics) and LOCAL-clean
(every `eval_str` bracketed by `checkpoint` / `reset_local_to`).

**Landed (four operations + a type vocabulary).**

- **`SourceLoc { file, line: u32, col: u32 }`** — the runtime's `Pos`
  lifted into a stable Rust struct, the shape `[file line col]`
  `(source-location 'NAME)` already returns (ADR-031).
- **`Diag { pos: Option<Pos>, message }`** — one advisory finding. `pos`
  stays optional because the checker doesn't thread spans through
  macroexpansion yet (ADR-024).
- **`EvalResult { value, error, diagnostics }`** — structured eval result.
  Exactly one of `value` / `error` is `Some`; `diagnostics` is
  independent so the agent sees warnings about code that happens to work.
- **`source_location(name)`** — lifts `(source-location 'NAME)` into
  `Option<SourceLoc>`. Returns `None` for prelude/builtin globals (no
  recorded site — they don't go through the file loader's
  `note_definition`), unbound names, and any malformed result vector.
- **`macroexpand_to_string(src, recursive)`** — reads `src` ourselves and
  calls `eval::macros::macroexpand_1` / `macroexpand` directly, rather
  than `eval_str("(macroexpand-1 'SRC)")` — the latter would let an
  unbalanced paren in `src` break the surrounding expression.
- **`format_source(src)`** — wraps `(format-source SRC)` from
  `std/format.blsp`. Escapes `\` and `"` for the string literal; raw
  newlines pass through.
- **`eval_in_session(src)`** — the high-throughput operation. Runs the
  checker on a separate `read_all_positioned` + `check_file` pass
  (mirroring the LSP's path at `crates/lsp/src/main.rs:398-415`), then
  `eval_str`s the source. State accumulates across calls because `def`s
  promote to RUNTIME, which survives the per-call LOCAL housekeeping —
  the hot-reload contract (ADR-013) doing its job.

**Deferred to step 1c**, behind real Brood-side prereqs:

- **`check_project`** — `(check-project)` is print-oriented (GNU lines to
  stdout + an `Int` count). The right wrapper needs a structured variant in
  `std/project.blsp` returning `[file line col message]` tuples; the alt
  ("capture stdout, parse GNU") goes against ADR-006.
- **`run_tests`** — same: `(run-project-tests)` prints GNU per-test output
  and raises on failure. Needs a structured runner result from
  `std/test.blsp`.
- **`EvalResult.stdout`** — needs `*out*` (a dynvar) + a `with-out-str`
  capture primitive. Out of scope for step 1; `eval_in_session` ships
  without it. `value` + `error` + `diagnostics` are already useful, and
  `print`-as-debug isn't an agent's primary affordance.

**Tests.** 12 new unit tests in `crates/lisp/src/introspect.rs`, covering
each operation's happy path and at least one failure mode (parse error,
unbound name, type mismatch). The `source_location` happy-path test
directly drives `Heap::set_current_file` + `eval_source` to populate the
def-site table (the only path that does today), plus a focused unit test
on the result-vector lifter. State persistence across calls is asserted
end-to-end (`(def x 42)` → `(* x 2)` → "84").

**Verified.** `cargo build` clean. `cargo test -p brood --lib introspect`
16/16 (4 original + 12 new). `cargo test -p brood-lsp` 40/40 — LSP
behaviour unchanged. `cargo test --workspace --lib` 115/115.

**What's left for MCP.**

1. **Step 1c** (any subset, in any order): structured `(check-project)`
   variant in `std/project.blsp`; structured runner result in
   `std/test.blsp`; `*out*` dynvar + `with-out-str`. None block step 2 —
   the dispatcher can start with the four shipped helpers (plus `load`
   and `processes` as one-line `eval_in_session` calls; they don't need
   dedicated Rust wrappers).
2. **Step 2** — `crates/nest/src/mcp.rs`: the sync JSON-RPC loop + the
   tool registry loaded from `(mcp-tools)`.
3. **Step 3** — `std/mcp.blsp`: the eight tool `defn`s + the catalogue
   shape projects can extend.
4. **Step 4** — `nest new` scaffolds `foo/.mcp.json`.
5. **Step 5** — Tier-1 niceties (`prompts/get`, project-defined tools).

---

## 2026-05-28 — `file-mtime` + hot-reload example

**Goal.** Exercise the shared-code hot-reload story end-to-end from Brood with
a self-contained demo: a ticker that calls `(greet)` while a separate green
process watches the defining file and re-`load`s it when it changes.

The mechanism was already in place — `def` promotes into the shared
`RuntimeCode` region and rebinds the global; in-flight calls keep the old
closure, the *next* lookup sees the new one (`docs/shared-code.md`). The
missing piece for a clean watcher was a cheap "did anything change?" stat
that doesn't slurp the whole file every tick.

**Added.**

- **`(file-mtime path)`** in `crates/lisp/src/builtins.rs` — `i64`
  epoch-milliseconds or `nil`. Reads `std::fs::metadata().modified()`; any
  failure (missing file, mtime unsupported, pre-epoch) collapses to `nil`
  rather than throwing, so a poller doesn't need a `try` around stat. Type
  signature `(string) -> int | nil`. Documented in `docs/primitives.md`
  and `docs/brood-for-claude.md`. Justification (ADR-006): a one-syscall
  stat is mechanism — Brood can't build it from existing primitives.

- **`examples/hot-reload/`** — `greeter.blsp` (one `defn greet`) and
  `main.blsp`, which spawns a `code-reloader` green process. The reloader
  is a tail-recursive `(path, last-seen-mtime)` loop that polls `file-mtime`
  every 250 ms, calls `load` only when the mtime moves, and catches reader
  / eval errors from partial-write races so a bad tick doesn't kill the
  process. Updates `last` even on a *failed* reload so we don't busy-retry
  the same broken state.

**Verified end-to-end.** Ran `./target/debug/brood
examples/hot-reload/main.blsp` and edited `greeter.blsp` mid-run:

```
[main] ticker starting (Ctrl-C to stop; edit greeter.blsp to swap output)
hello
hello
...
[reloader] reloaded examples/hot-reload/greeter.blsp
bye
bye
...
[reloader] reloaded examples/hot-reload/greeter.blsp
hello
hello
```

The ticker, untouched, picks up the redefinition because each iteration of
`(ticker n)` does a fresh global lookup of `greet` — late binding doing
exactly what `docs/shared-code.md` claims.

**Note** (advisory-check noise). The auto-checker emits `unbound symbol:
greet` for `main.blsp` because `greet` is only defined via a runtime
`load`, not statically present in the file the checker walks. Harmless —
the run proceeds — but worth tracking: a clean cross-file model (or a
`(declare 'greet)` form) is the eventual fix.
