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


---

## 2026-05-28 — Code review pass: monitor race fixes + doc tidy

**Goal.** Quick review-and-fix pass over the recent distribution + Step-4
work to surface any latent bugs and bring the docs back into sync with what's
actually in the kernel.

**Two real races, both pre-existing, both fixed.**

1. **`add_monitor` (TOCTOU on REGISTRY vs MONITORS).** The original
   `monitor` builtin (and now `add_monitor`, after the `Watcher`
   refactor) checked `REGISTRY.contains_key(target)`, released the lock,
   then took `MONITORS` and inserted. If the target's `deregister` ran in
   between, the watcher landed in `MONITORS` after the target was gone —
   `:down` would never fire. Fix: hold `MONITORS` *during* the
   REGISTRY-liveness check, so the check + insert are atomic from
   `deregister`'s point of view (and `deregister`'s release-then-acquire
   pattern keeps the lock order deadlock-free). Documented the lock
   ordering invariant on `deregister` so a future contributor can't
   regress it: don't ever hold `REGISTRY` while reaching for `MONITORS`.
2. **`monitor_remote` (drop_link drains PENDING_REMOTE before
   record).** If a link's `drop_link` + `fire_nodedown` ran between
   `monitor_remote`'s connectivity check and its `record_pending_remote`
   call, the new entry would land *after* `handle_node_down` had already
   drained the bin — orphaning the watcher (no `:noconnection` ever
   fired). Fix: record first, *then* take a single `NODES` read covering
   both the link-presence check and the channel send. If the link is
   gone by the time we look, explicitly drop the pending entry and fire
   `:noconnection` ourselves. Either ordering with `drop_link` now lands
   correctly: drop-first → drain finds our entry; record-first →
   the explicit cleanup path catches it.

Both fixes are tiny (a handful of lines each) but soundness-relevant —
without them, a hot path involving monitors against a process or peer
that's racing to exit could silently swallow `:down`.

**Other small things.**
- `eval/mod.rs`: `quasiquote` now wraps its return with
  `or_form_pos(expr)`. The inner unquote eval already attaches finer
  positions; this is the fallback so a malformed quasi-quote (rare)
  isn't bare.
- `dist::compute_mac`: the doc comment was misleading ("length-tagged by
  byte position"). Rewrote it to spell out the two collision-free
  assumptions — `peer_nonce` is fixed-length, the `0x00` delimiter
  separates the two variable-length names — and to note that NUL can't
  appear in a Brood symbol name (reader rejects it). The encoding is
  unchanged.
- `docs/primitives.md`: bumped the count and added the rows that were
  silently missing — `check-file`, `eprint`, `source-location`,
  `parse-source`, `dynamic?`, `register`, `whereis`, and the whole
  **Distributed nodes** block (`node-start`, `connect`, `node-name`,
  `nodes`, `monitor-node`). `monitor`/`demonitor` row updated to
  mention the cross-node case (`:noconnection` on net-split).

**Verified.**
- `cargo test`: every workspace suite green (lib 115, integration 61,
  distribution 13, …). 451/451 in-language tests.
- `cargo clippy --all-targets`: 1 warning, pre-existing
  (`type_complexity` in `process.rs:549`).
- The monitor race fixes don't change behaviour on the happy paths the
  existing distribution suite exercises — they only close the window
  where a concurrent peer-down or target-death would have leaked.

---

## 2026-05-28 — Hot-reload: ergonomic surface (`std/reload`, `nest run --watch`)

**Goal.** Make the file-mtime watcher pattern from the earlier example
"super easy to set up" (user ask), without making it the default. Two
levels of opt-in: one for source-modifiers, one for ad-hoc users.

**Added.**

- **`std/reload.blsp`** — a require-able std module exposing
  `(reload-on-change path) → pid`. Body is the same tail-recursive
  `(path, last-seen-mtime)` loop the example shipped: poll `file-mtime`
  every `*reload-poll-ms*` (250 ms), call `load` only on a bump, swallow
  reader/eval errors from partial-write races but still advance `last` so
  a broken save doesn't busy-retry every tick. Registered in
  `EMBEDDED_MODULES` (`crates/lisp/src/builtins.rs`), so it's `require`-able
  with no load-path config — same shape as `test`/`project`/`docs`/`hatch`.

- **`nest run --watch <file>`** (repeatable) — pre-spawns a reloader
  per `<file>` before invoking `run-project`. Parsed in the `"run"` arm
  of `crates/nest/src/main.rs`; both `--watch path` and `--watch=path`
  forms work. Bootstrap snippet becomes `(require 'project) (load-config)
  (require 'reload) (reload-on-change "p1") … (run-project (list …))`.
  No flag → no overhead; the reload module isn't loaded at all.

- **Updated `examples/hot-reload/main.blsp`** to use the module. Two
  lines now do the whole job:

      (require 'reload)
      (reload-on-change greeter-path)

**Verified.** Two end-to-end swap tests, both watched, both hot-reloaded
without restart:

1. `./target/debug/brood examples/hot-reload/main.blsp` + edit
   `greeter.blsp` → `[reload] …/greeter.blsp` line, next ticks print the
   new output.
2. `nest new hot_reload_demo`, rewrite `main.blsp` to a tail-recursive
   ticker, `nest run --watch src/hello.blsp` + edit `hello.blsp` → same
   reload line, next ticks pick up the new `(greeting)`.

`cargo test --workspace` 234/0/0 across all crates after the changes.

**Why not `brood --watch`.** Considered and rejected. The language binary
already has too many flags, and "re-run the entry file when it changes"
has different semantics (re-runs top-level effects) from hot-reload (rebinds
globals, in-flight calls keep going). If anything, file-watching belongs
under `nest`, not `brood`.

**Deferred (ADR-011 — favour the simple form):**
- `(stop-reloader pid)` — needs a defined "kill a green process" story
  first; pids are immortal today.
- `(reload-many [paths…])` — one-line wrapper over a `dolist`, write it
  in user code when needed.
- Configurable poll interval per-watcher — currently a single
  `*reload-poll-ms*` global; fine until someone has a real reason to
  vary it.
- OS-native watching (`notify` crate) — polling at 250 ms is invisible
  in human terms; defer the kqueue/inotify hop until the editor side
  (many buffers) actually needs it.

---

## 2026-05-28 — MCP step 2: `nest mcp` dispatcher

**Goal.** Step 2 of the MCP plan (ADR-036 / `docs/mcp.md`): a sync JSON-RPC
loop in `crates/nest/src/mcp.rs` that speaks Model Context Protocol over
stdio against a long-lived `Interp`, strictly per-project (ADR-028 — `nest`
is the project tool).

**Landed.**

- **`crates/nest/src/mcp.rs`** (~600 LoC) — the dispatcher itself:
  - **Transport.** Content-Length framing (the same shape LSP uses; MCP is
    JSON-RPC over stdio). Read/write taken as `impl BufRead` / `impl Write`,
    so tests drive it with `Cursor<Vec<u8>>` / `Vec<u8>` rather than real
    stdio — same shape as the LSP's `Connection::memory()` pattern, no
    threading. No `tokio`, no `lsp-server` (MCP isn't LSP, and the surface
    is small enough to roll directly — the calculus ADR-025 made for the
    LSP applies in reverse here).
  - **Methods.** `initialize`, `tools/list`, `tools/call`, `resources/list`,
    `resources/read`, `prompts/list`, `ping`, `shutdown`, `exit` (the
    notification), plus silent-drop for unknown notifications and
    `MethodNotFound` for unknown requests.
  - **Tool dispatch.** Reads the catalogue from `(require 'mcp) (mcp-tools)`
    on every `tools/list` *and* `tools/call`, so a `def` in a previous
    `eval` (hot reload) reshapes the surface immediately. A missing
    `std/mcp.blsp` (the step-3 work) collapses to an empty tool list — the
    server stays useful, just with no tools yet. `tools/call` converts
    JSON arguments to a Brood map (objects become keyword-keyed maps, the
    idiomatic shape for `(get args :source)`), `apply`s the handler with
    full GC rooting (`push_root` around `args_value` + `handler` across
    `brood::eval::apply`), and wraps the Brood result in MCP's
    `{ content: [{ type: "text", text: "..." }] }` envelope.
  - **Brood ↔ JSON converters** (`value_to_json`, `json_to_value`, `pub`)
    cover all data kinds: nil/bool/int/float/string/keyword/symbol/list/
    vector/map. Closures, macros, natives, refs, pids fail loudly rather
    than silently drop (a tool that *returns* a closure surfaces an error
    instead of "null"). JSON object keys → keywords; arrays → lists;
    integers preserved where possible.
  - **Resources.** Five static doc URIs baked in via `include_str!`
    (`brood://docs/{brood-for-claude,language,decisions,types}` and
    `brood://prelude`) — the agent gets the canonical docs over MCP
    without filesystem access. A dynamic `brood://project` URI lands with
    step 3.
- **`crates/nest/src/main.rs`** — `nest mcp` subcommand. Bootstrap mirrors
  the LSP's `bootstrap_project` (`crates/lsp/src/main.rs:329`): walk up
  for `project.blsp`, `project-setup`, `project-load-sources`, `(require
  'test)`, `(require 'format)`. Outside a project root, a clean GNU error
  + exit-1. Diagnostics go to stderr (stdout is the protocol stream).
- **`crates/nest/Cargo.toml`** — `serde_json = "1"`. No tokio, no MCP/LSP
  crate.

**Tests.** 13 dispatcher tests in-process via in-memory framing:
- `initialize` returns server info + capabilities.
- `tools/list` is empty when no catalogue is defined.
- `tools/list` projects a Brood-defined catalogue to the MCP shape
  (pre-defines `mcp-tools` inline, asserts on the `inputSchema` round-trip).
- `tools/call` dispatches to a Brood handler (`{n: 21}` → `42`).
- `tools/call` returns `InvalidParams` for unknown tools.
- `resources/list` + `resources/read` against the baked URIs.
- `ping` / `shutdown` / `exit` lifecycle.
- Unknown method → `MethodNotFound`; unknown notification → silent drop.
- Brood ↔ JSON round-trip across a representative payload
  (`{n,f,s,items,nested,flag,absent}`).
- `value_to_json` rejects unrepresentable kinds (closures).

**Verified.** `cargo test --workspace` clean — 115 + 61 + 40 + 13 + …
across every suite, no regressions. Real-binary smoke test: `nest mcp`
in `/tmp` errors out (`not in a Brood project`); inside the project,
`initialize` returns properly-framed JSON with the expected payload.

**What's left for MCP.**

1. **Step 3 — `std/mcp.blsp`**: the eight initial tool `defn`s (`eval`,
   `load`, `lookup`, `macroexpand`, `run-tests`, `check`, `format`,
   `processes`) + the `(mcp-tools)` registry the dispatcher reads.
   `eval` / `lookup` / `macroexpand` / `format` / `load` / `processes`
   are tractable today via `brood::introspect` + one-line
   `eval_in_session` calls; `check` / `run-tests` ship as stubs until
   step 1c lands their Brood-side prereqs.
2. **Step 1c** (any subset, in any order): structured `check-project` /
   `run-project-tests` variants in `std/project.blsp` / `std/test.blsp`;
   `*out*` dynvar + `with-out-str` for stdout capture in
   `eval_in_session`.
3. **Step 4** — `nest new` scaffolds `foo/.mcp.json`.
4. **Step 5** — Tier-1 niceties (`prompts/get` for a `brood-task`
   template, project-defined tool discovery from a project's own
   `mcp.blsp`).


---

## 2026-05-28 — Security/hardening review pass (Rust review + audit fixes)

**Goal.** Act on the consolidated findings of the multi-agent Rust review +
security audit (style, file separation, crash hazards, network surface).
All "Critical" + "Important" items addressed; the larger cleanup items
(file splits, `expect_*` macro consolidation) are deferred.

**Critical fixes.**

- **Depth caps on every recursive parser/codec/walker** — reader, CST,
  printer, quasiquote, `macroexpand_all`, wire-frame `decode_msg` /
  `decode_closure`, message `to_message_rec` all take a `depth: u32`
  bounded at 256 and return a clean error past it. Pre-fix, a deeply
  nested file (~1000 open parens) or a tiny but pathological wire frame
  aborted the process with a Rust stack overflow. New
  `parser_rejects_deeply_nested_input_instead_of_overflowing` test
  in `crates/lisp/tests/basic.rs` guards the surface.
  Sites: `crates/lisp/src/syntax/{reader,cst,printer}.rs`,
  `crates/lisp/src/eval/macros.rs`, `crates/lisp/src/dist.rs` (decode
  side), `crates/lisp/src/process.rs` (to_message side).
- **Reader rejects out-of-range integer literals** instead of silently
  falling through to `Float` — `9223372036854775808` now errors
  ("integer literal out of range for i64") rather than reading as
  `9.22e18`. New `AtomKind::IntOverflow` variant; the reader maps it to
  `LispError::parse`, the CST to `NodeKind::Error`. Float-shaped tokens
  (`1e1000` → `inf`) still parse. New
  `reader_rejects_out_of_range_integer_literal` test.
- **`floor` errors on non-finite / out-of-range floats** rather than
  the silent saturating `f as i64`. `(floor (* 1e308 1e308))` and
  `(floor (/ 0.0 0.0))` now return a runtime error; finite in-range
  values still work. New `floor_rejects_non_finite_and_out_of_range`
  test. `crates/lisp/src/builtins.rs:454`.
- **`apply` is TCO through the eval loop.** Eval's main dispatch
  detects `Value::Native(apply)` and unfolds the call inline (splicing
  the trailing sequence into argv, looping on nested `(apply apply …)`)
  before falling through to the `Native` / `Fn` cases — so chained
  `(apply f …)` recursion no longer grows the Rust stack ~4 frames per
  level via `call_native → apply_builtin → eval::apply`. New
  `apply_tail_recursion_does_not_overflow` test (100,000 levels through
  `(apply loop-apply …)`). `crates/lisp/src/eval/mod.rs:369`.
- **`monitor` accepts the `{:name :node}` address form** the Sig +
  docstring already promised. Local-node addresses resolve via
  `dist::whereis`; a remote-name address errors clearly (the protocol
  has no name-resolve-then-monitor round-trip yet). Required exposing
  `process::read_name_address` as `pub(crate)`.
- **`nest new` name validation** rejects `..`, `\`, NUL, leading
  `.`/`-`/`~`, whitespace, embedded tabs/newlines — path-traversal
  hardening in `std/project.blsp:269`.
- **`dist.rs` hardening pass.**
  - `connect` now uses `TcpStream::connect_timeout` (5s) per address.
  - Writer socket gets a 30s `set_write_timeout` so a slowloris peer
    can't pin the writer thread + grow the per-link `mpsc::channel`.
  - Authenticated peer name (from `handshake`) is used for inbound
    `Frame::Monitor` / `Demonitor` watcher node, *not* the wire
    `from_node` field — a peer can no longer spoof a watcher's node
    identity.
  - `node_start` accept loop catches per-connection panics and
    sleeps 50ms on accept errors instead of burn-looping on EMFILE.
  - Heartbeat thread re-spawns on panic.
  - `frame_bytes` checks payload against `MAX_FRAME` on the *encode*
    side too (was decode-only) — silent `as u32` truncation can no
    longer produce a frame the peer can't parse.
  - `LOCAL_NODE` uses `Release`/`Acquire` ordering paired with the
    `NODE` write lock.

**Important-tier fixes.**

- **Mutex / RwLock poison recovery** sweep: new `core::sync::{lock,read,
  write}` helpers (mirror the `ids()` pattern from `value.rs`), used at
  all ~42 lock sites across `process.rs` and `dist.rs`. A panic inside
  any code holding a global lock no longer cascades — every `MONITORS`
  / `NODES` / `REGISTRY` access now recovers from poisoning.
- `(quote a b)` is now an arity error (used to silently drop the tail).
- `or_form_pos` is threaded on leaf-symbol unbound errors and on
  `&optional` default-form evaluation — diagnostics from those paths
  now point at the symbol/default form's line.
- `gensym` Sig fixed (`any -> sym` rather than the wrong `string -> sym`
  that triggered checker warnings on `(gensym 'foo)`).
- Handle constructors carry `debug_assert!`s against the silent
  region-bit aliasing case (`index >= 2^30`).
- `intern` no longer double-allocates the symbol name string.
- `local_live_count` uses `saturating_sub` + a `debug_assert!` — a
  free-list-vs-slab accounting bug surfaces in tests rather than
  panicking on the GC safepoint hot path.
- `apply_builtin` binds `let last = args.len() - 1` after the guard so
  the slice indexing is robust to refactors.
- `Ty(u16)` is one tag away from a cryptic const-eval shift overflow on
  the 17th atom — explicit `const _: () = assert!(TAG_COUNT <= 16, …)`
  surfaces the cap with a clear message.
- `is_syntactic_keyword` no longer lists phantom `loop` / `recur` (they
  aren't special forms or prelude macros).
- LSP `uri_to_path` percent-decodes the URI path (previously a
  whitespace or non-ASCII path silently failed `find_project_root` and
  the LSP never bootstrapped the project).
- `LineIndex::new` has a `debug_assert!` against documents > 4 GiB and
  saturates its `u32` length field instead of truncating.

**Cleanup landed.**

- `report_error` + `parse_jobs_args` lifted into a new
  `brood::cli_support` module, shared by both `brood` (`crates/cli`) and
  `nest` (`crates/nest`). Two byte-for-byte-identical copies collapsed
  to one.
- `escape_brood_string` promoted to `pub` in `brood::introspect`; the
  five `replace('\\','\\\\').replace('"','\\\"')` copies in `nest`
  and the LSP now share that one function.
- `Closure` derives `Default`; sweep replaces dead slots with
  `Closure::default()` (was an inline 7-field literal — would silently
  drop a new field).

**Cleanup deferred** (substantial restructurings; documented for the
next pass): file splits for `dist.rs` (→ `dist/{mod,wire,handshake,
heartbeat}.rs`), `process.rs` (→ `process/{mod,message,mailbox,…}.rs`),
`types/check.rs` (→ `check/{ctx,sigs,guards,walk}.rs`); an `expect!`
macro to collapse the five `expect_string`/`expect_int`/etc helpers;
the six `alloc_*`/`sweep` slab loops in `heap.rs`; `NodeKind::name`
inherent method; reader-vs-CST structural-parse consolidation.

**Verified.** `cargo test --workspace` → 251 passed, 0 failed
(unchanged-but-+3 tests over the pre-pass baseline of 248). All
distribution tests still pass with the dist.rs hardening (including
the `cross_node_pid_monitor_fires_down` and `node_down_is_detected`
flows that depend on the heartbeat path). The preemption test passes
after `tick()` was correctly preserved (one trampoline iteration
caught the regression mid-pass and was reverted in favour of the
inline-in-eval approach).

---

## 2026-05-28 — MCP step 3: `std/mcp.blsp` lights up the dispatcher

**Goal.** Step 3 of the MCP plan (ADR-036 / `docs/mcp.md`): the eight
initial tool `defn`s and the `(mcp-tools)` registry the `nest mcp`
dispatcher reads. With this landing, **the protocol surface is live** —
an agent attached via `.mcp.json` sees a populated `tools/list` and can
drive `eval` / `load` / `lookup` / `macroexpand` / `format` against the
project's image.

**Landed.**

- **`std/mcp.blsp`** (~150 LoC of Brood, per ADR-006 — the tool *surface*
  is policy in Brood, not Rust):
  - **Six live tools.** `eval` (read-string + eval, returns `{:value
    pr-str}` or `{:error msg}`), `load` (returns `{:ok true|false}`),
    `lookup` (returns `{:name :arglist :doc :source-location}`; unbound
    names come back as `{:name :error}`, a soft failure the agent can
    branch on), `macroexpand` (1-step / all, returns `{:expanded pr-str}`),
    `format` (wraps `(format-source ...)`, returns `{:formatted ...}`).
  - **Three documented stubs** — `check`, `run-tests`, `processes` —
    return `{:error "not yet wired — needs <prereq> (step 1c)"}`. The
    error message names exactly what's missing so the agent gets a
    truthful pointer rather than a "tool unavailable" 404.
  - **Argument validation** uses `throw` for shape errors (the dispatcher
    converts a throw into a JSON-RPC error, so a misshapen `arguments`
    looks like a *protocol* failure, not a *value*). Body errors (parse,
    runtime) become `{:error msg}` fields so the agent can act on them.
- **`crates/lisp/src/builtins.rs`** — added `("mcp", include_str!(...))`
  to `EMBEDDED_MODULES`. `(require 'mcp)` resolves via `%builtin-module`,
  so the dispatcher finds the catalogue without a configured load-path.
- **`crates/nest/src/mcp.rs`** — repurposed the now-obsolete
  `tools_list_is_empty_when_no_catalogue_is_defined` test as
  `tools_list_returns_the_baked_std_catalogue` (asserts the eight tools
  and the `inputSchema.type == "object"` invariant). The two existing
  override-path tests (`tools_list_projects_a_brood_defined_catalogue`,
  `tools_call_dispatches_to_a_brood_handler`) now `(provide 'mcp)` before
  binding inline so the dispatcher's `(require 'mcp)` is a no-op and the
  test's catalogue wins — which is exactly the shape a project's own
  `mcp.blsp` will use to extend the surface (step 5).
- **Eight step-3 integration tests** through the real dispatcher:
  - `eval` returns the printed value (`(+ 1 2)` → `"3"`); captures a
    runtime error (`(no-such-fn …)`); state persists across calls (a
    `def` in call #1 is visible in call #2 — the hot-reload contract,
    ADR-013).
  - `lookup map` returns arglist + doc; `source-location` is `null` for
    prelude defs (the prelude isn't loaded via the positioned reader, so
    no recorded site — pin the current behaviour rather than hide it).
  - `lookup` of an unbound name is a soft `:error` field, not a thrown
    exception.
  - `macroexpand` steps `(when x 1)` into an `if`-shaped form.
  - `format` reformats messy source; idempotent.
  - `check` / `run-tests` / `processes` each carry the documented
    "not yet wired" marker — a future un-stub flips this assertion.
  - Argument validation (`{source: 42}` for `eval`) raises a JSON-RPC
    error mentioning `:source`.

**Verified.** `cargo build` clean. `cargo test --workspace`: 115 (lib)
+ 65 (integration) + 40 (LSP) + 22 (nest, 14 dispatcher + 8 step 3)
+ … all green, no regressions. Real-binary smoke: `tools/list` returns
the eight names in order; `tools/call eval (+ 1 2 3)` returns `{
"value": "6" }`; `tools/call lookup map` returns
`{ "arglist": ["f","coll"], "doc": "A list of `(f x)` …", "name": "map",
"source-location": null }`.

**What's left for MCP.**

1. **Step 4** — `nest new foo` scaffolds `foo/.mcp.json` pointing at
   `nest mcp`, so `cd foo && claude` auto-attaches the project's MCP
   server (closing the loop with the `brood-for-claude.md` doc that's
   already `%builtin-doc`-baked).
2. **Step 1c** (the un-stubs, in any order):
   - Structured `(check-project)` in `std/project.blsp` — return
     `[file line col message]` tuples instead of printing.
   - Structured runner result in `std/test.blsp`.
   - `*out*` dynvar + `(with-out-str)` for stdout capture (also lets
     `eval_in_session` ship a `:stdout` field).
   - A `(list-processes)` primitive — small Rust addition, gated on a
     concrete use-case.
3. **Step 5** — Tier-1 niceties (`prompts/get` for a `brood-task`
   template; project-defined tool discovery from a project's own
   `mcp.blsp` that conses entries onto the std `(mcp-tools)` list).

---

## 2026-05-28 — MCP steps 4, 1c-{a,b,d}, 5: full v0 surface live

**Goal.** Land every remaining MCP step that doesn't require redesigning
`print`. Result: six of eight tools fully wired (was: five), the agent
attach loop is closed (`nest new` scaffolds `.mcp.json`), and project-level
extensibility is in place.

**Step 4 — `nest new` scaffolds `.mcp.json`.** `std/project.blsp` grew a
`project--mcp-json-template` (a single `brood` server entry pointing at
`nest mcp`). `new-project` writes it alongside `CLAUDE.md` /
`brood-for-claude.md`, and the CLAUDE.md template now carries an "MCP
integration" section telling humans what's there. Verified: `nest new
mcp4-smoke` writes the expected JSON; `cd mcp4-smoke && claude` would
auto-attach.

**Step 1c-a — structured `(check-project)`.** New Rust primitive
`(check-file-structured path)` returns `[{:file :line :col :message}]`
(`:line`/`:col` omitted when the checker has no position — ADR-024).
`std/project.blsp` grew `(check-project-structured)` as the data-shaped
analogue of `(check-project)` (honors `BROOD_NO_CHECK=1` the same way).
`mcp-check-tool` un-stubbed: returns `{:diagnostics [...]}` or
`{:error msg}` (when called outside a project).

**Step 1c-b — structured test runner.** `std/test.blsp` grew
`(run-tests-structured)` — same isolated/parallel/serial orchestration as
`run-tests`, but returns
`{:total :passed :failed :failed-assertions :ms :results [{:group :name
:passed :ms :failures [{:loc :details} ...]}]}` instead of printing GNU
output + throwing. `std/project.blsp` grew the matching
`(run-project-tests-structured)`. `mcp-run-tests-tool` un-stubbed.

**Step 1c-d — `(list-processes)`.** New Rust primitive lifting `REGISTRY`
keys to `Pid` values (via `process::pid_value`, so each pid carries this
runtime's node identity and is `send`-routable as-returned).
`mcp-processes-tool` un-stubbed; `(or (list-processes) [])` so an empty
result renders as JSON `[]` rather than `null` — agents shouldn't have
to disambiguate "no processes" from "missing field".

**Brood ↔ JSON converter** now renders `Pid` and `Ref` as tagged objects
(`{$type: "pid", node, id}` / `{$type: "ref", id}`) instead of erroring.
A tool returning a pid-bearing value no longer loses data; the `$type`
tag distinguishes them from plain maps. `json_to_value` is intentionally
one-way (a JSON object stays a Brood map — constructing fresh pids/refs
from JSON would be unsound).

**Step 5a — `prompts/get` with `brood-task`.** A single orientation prompt
baked into `mcp.rs` as `BROOD_TASK_PROMPT` (~1.2 KB). Points at
`brood://docs/brood-for-claude` for depth, lists the MCP tool surface,
sketches the Brood essentials (immutability, no `set!`, truthiness,
modules). The agent fetches this once at session start to get oriented;
`prompts/list` advertises it; `prompts/get` returns it as a single
`user`-role text message.

**Step 5b — project-defined tool discovery.** `std/mcp.blsp` ends with
an auto-load: if `<project-root>/mcp.blsp` exists, `(load)` it after
the std catalogue is bound. The project's file can `def mcp-tools` to
extend (`(let (base mcp-tools) (defn mcp-tools () (append (base) (list
new-tool))))`) or replace the catalogue. Runs once (`require` is
idempotent via `provide`).

**Deferred — step 1c-c (`*out*` + `with-out-str`).** Documented in
`docs/mcp.md` and (loose end) at the bottom of this entry. Folding a
`:stdout` field into `EvalResult` would need the `*out*` dynvar
architecture *plus* a way to safely buffer per-process (a thread-local
would leak captures across green processes scheduled on the same OS
thread). The current state: an `(eval (println …))` in a tool call
writes to the dispatcher's stdout and corrupts the JSON-RPC stream.
Workaround until step 1c-c lands: agents should return data via the
`:value` field instead of calling `print`. The `brood-task` prompt
should grow a note pointing this out (folded in once the fix is
designed).

**Tests.** 28/28 nest tests now (was 22): added `prompts_list_includes_brood_task`,
`prompts_get_returns_the_orientation_message`, `prompts_get_returns_an_error_for_unknown_names`,
`std_processes_tool_returns_a_pid_list`, `value_to_json_renders_pids_as_tagged_objects`,
`std_check_tool_returns_structured_diagnostics_or_an_error`,
`run_tests_structured_returns_a_structured_summary`. The
`std_check_and_run_tests_and_processes_are_documented_stubs` test was
shrunk to `std_run_tests_is_a_documented_stub` as each tool un-stubbed;
then once `run-tests` landed, that test went too (the `run-tests-structured`
test replaces it).

**Verified.** `cargo build` clean. `cargo test -p nest`: 28/28.
`cargo test -p brood-lsp`: 40/40 (no regressions). `cargo test
--workspace` is blocked by a *parallel* in-flight edit on
`crates/lisp/src/types/check.rs` (the user is splitting it into a
submodule); my changes don't depend on it and `cargo test -p` works.
Real binary: `tools/list` returns all 8 tool names, `processes`
returns `{"processes":[]}` (empty array, correctly), `prompts/list`
shows `["brood-task"]`, `prompts/get brood-task` returns 1157 chars
of orientation text.


---

## 2026-05-28 — Package-manager design (ADR-037); bundler deferred (ADR-038)

**Goal.** Land the design for third-party Brood deps *before* M2 — the
`_deps/` layout, auto-fetch, and lock-file policy cross-cut every
existing `nest` subcommand, and the upcoming editor plugin story
shouldn't invent its own one-off loader. Decide now what the manifest
shape is, what the cache looks like, where the trust boundary sits;
implement when the language work pays itself down.

**Recorded.**

- **ADR-037** — *Packages: git deps + project-local cache + lock file*.
  Go-style "URL = name" identity, no central registry; `project.blsp`
  gains an optional `:dependencies` vector of `[name :git URL :ref REF]`
  / `[name :path PATH]` entries; `nest fetch` writes
  `project.lock.blsp` (committed) with the resolved commit + SHA-256
  per dep; project-local `_deps/<name>/` cache (gitignored,
  reconstructable from the lock); auto-fetch on first run of every
  `nest` subcommand. The Rust kernel grows four small primitives
  (`%git-clone`, `%git-resolve-ref`, `%sha256-file`, `%http-get`);
  policy is `std/package.blsp` (new). No constraint solver, no install
  scripts, no native code at install time — supply-chain attack class
  closed by construction.
- **ADR-038** — *Single-binary bundling: deferred until distribution
  matters*. Append-to-binary approach (zip + magic footer, runtime
  detects from `/proc/self/exe`); ships when the editor (M3/M4) needs
  end-user distribution, not before.

- **`docs/packages.md`** — the long-form design walkthrough: manifest
  model, lock-file format with examples, resolution algorithm
  (depth-first, MVS-without-the-solver: direct beats transitive),
  conflict policy (loud error, no auto-resolve), `*load-path*`
  integration, the full `nest` subcommand surface, cache layout +
  `.gitignore` interaction, the hot-reload story for path-deps, the
  trust/security model, a side-by-side with Go modules / Cargo / npm,
  the explicitly-deferred list (registry, semver, signing, tarball
  sources), and an implementation sketch the eventual coder can read
  to write the same thing twice.

**Updated.**

- `docs/roadmap.md` — M1 list gets the ⬜ "Package manager" entry next
  to "Project model & test tool", with a one-paragraph summary and a
  pointer to ADR-037 / `packages.md`.
- `ROADMAP.md` — new **Adjacent to Stage 1** section: package manager
  (designed, lands as project work catches up) + single-binary
  bundling (designed, deferred until M3/M4).
- `docs/decisions.md` — ADR-037 + ADR-038 appended.

**Why early.** Three reasons:

1. **It changes project management.** Auto-fetch on every `nest`
   subcommand, `_deps/` in `.gitignore`, the lock-file commit
   convention — those are workflow choices, not implementation
   details. Better to land them once.
2. **The editor needs it.** M2 starts introducing modes / syntax
   highlighters / keymaps as plugins; a package system that already
   exists for ordinary Brood code drops in as the plugin loader
   instead of a one-off solution.
3. **It's cheap once designed.** Most of the system is Brood policy
   (`std/package.blsp`); the Rust additions are four small primitives
   that don't touch the evaluator, the GC, or the scheduler. Designing
   was the hard part; coding it is a few days when the time comes.

**Not changed yet.** No code lands in this commit — just docs. The
design is captured fully so the implementation, when it happens, is a
reading-comprehension exercise rather than a re-design.

**What's next** (per `docs/roadmap.md`'s "what comes next" angle):

- *Either* start M2 (rope-backed buffers — the editor's data model)
  *or* implement the package manager now. The package design doesn't
  block M2; it's a parallel track that pays interest once the editor
  starts inviting plugins. My read: M2 first if the editor goal is
  pulling; packages first if the user-extensibility story is.

---

## 2026-05-28 (continued) — Module splits: dist, types::check, process

**Goal.** Land the three file splits flagged at the end of the security
review as "substantial restructurings deferred for the next pass" — the
three biggest single files in the crate were carrying multiple concerns.

**`crates/lisp/src/dist.rs`** (1657 → 615 lines at the root, plus three
submodules). Rust 2018-style parent file `dist.rs` + sibling directory
`dist/` holding:

- **`dist/wire.rs`** (854 lines) — the entire wire codec: `Frame` enum,
  `FRAME_*`/`TARGET_*`/`M_*` tag constants, `frame_bytes`/`write_frame`/
  `read_frame`/`encode_frame`/`decode_frame`/`encode_target`/
  `decode_target`/`encode_msg`/`decode_msg_at`/`encode_closure`/
  `decode_closure_at`, all `put_*`/`get_*` byte helpers, `MAX_DECODE_DEPTH`,
  `PROTOCOL_MAGIC`/`NONCE_LEN`/`MAC_LEN`, and the round-trip tests. Pure
  data → bytes, no sockets, no scheduler. Items the parent needs are
  `pub(super)`; `Target` stays at the dist root (used by `route` and the
  reader thread too).
- **`dist/handshake.rs`** (216 lines) — the v2 authenticated exchange
  (`Role` enum, `handshake`, `read_hello`/`read_auth`, `compute_mac`,
  `ct_eq`, `fresh_nonce`) plus the `compute_mac_is_symmetric_under_role_flip`
  test that exercises the cross-MAC equality. Touches `super::NODE` for
  the cookie + name.
- **`dist/heartbeat.rs`** (93 lines) — the single shared liveness thread:
  `HEARTBEAT_INTERVAL`/`DOWN_AFTER`/`HEARTBEAT_STARTED`/`ensure_heartbeat`/
  `heartbeat_loop`. Reads `super::now_millis` and the connection table.
  Re-spawn-on-panic stays here.

**`crates/lisp/src/types/check.rs`** (1784 → 949 lines, most of that
tests). Sub-modules in `types/check/`:

- **`check/ctx.rs`** (187 lines) — the `Ctx` value the walk threads:
  `types` (variable narrowings), `guards` (let-stored guard results),
  `aliases` (let-binding aliases), `locals`, `file_globals`, plus all
  `narrow`/`bind`/`add_guard`/`add_alias` impls and the BFS chain.
- **`check/sigs.rs`** (183 lines) — where signatures + arities come
  from: `primitive_sig` (reads `NativeFn.sig`), `curated_sig` (the
  hand-vetted stdlib table), `infer_sig` (one-step inference),
  `sig_of`, `arity_of`, `arity_str`, `is_globally_bound`.
- **`check/guards.rs`** (175 lines) — predicates over forms:
  `is_syntactic_keyword`, `skips_body`, `guard_assertion`,
  `literal_eq_guard`, `expr_ty`.
- **`check/walk.rs`** (403 lines) — the recursive `check_into` and
  every special-form helper (`check_if`/`check_let`/`check_fn`/`check_def`/
  `check_defn`), plus `fn_params`, `bindings`, `list_items`,
  `collect_def_names`. `list_items` is `pub(super)` so `sigs` and
  `guards` can peel a call form's head.
- **`check.rs`** (parent) — module doc rewritten with a module map; the
  public entries `check_form` / `check_located` / `check_file`; and the
  tests block (unchanged behaviour, plus two new imports for items
  the tests reach into directly: `super::sigs::primitive_sig`,
  `crate::types::Ty`, `crate::core::value::Tag`).

**`crates/lisp/src/process.rs`** (1358 → 705 lines). Three submodules
under `process/`; the remaining `mailbox` + `scheduler` concerns stayed
in the parent because they share too much private state for a clean
split to be worth the visibility annotations:

- **`process/message.rs`** (369 lines) — `Message`/`ClosureMsg` types
  plus the deep-copy machinery (`to_message`/`to_message_rec`/
  `closure_to_message`/`collect_symbols`/`local_lookup`/`from_message`/
  `closure_from_message`). `MAX_MESSAGE_DEPTH` lives here too. The
  cleanest extraction — no scheduler dependencies; just heap + value.
- **`process/monitor.rs`** (306 lines) — the `Watcher` enum, `MONITORS`
  and `PENDING_REMOTE` tables, `NEXT_REF`/`next_ref`, and the full
  monitor lifecycle: `fire_down`/`add_monitor`/`monitor`/`demonitor`/
  `drop_monitor`/`record_pending_remote`/`drop_pending_remote`/
  `demonitor_remote_fanout`/`handle_node_down`/`fire_noconnection`/
  `local_node_pid_msg`/`down_message`. Takes `REGISTRY` and `deliver`
  from `super` (the lock-ordering invariant — REGISTRY first, then
  MONITORS — is documented in both files).
- **`process/timer.rs`** (65 lines) — `TimerQueue`/`TIMERS`/
  `TIMER_STARTED`/`arm_timer`/`timer_loop`. The wake-up path
  (`wake_for_timeout`) stays in `process.rs` so timer.rs doesn't need
  the full mailbox internals (`MailboxState`/`Process`/`enqueue`); it
  just calls `super::wake_for_timeout(pid)`.

**Cumulative.** The three biggest files in the crate dropped from
1657 + 1784 + 1358 = 4799 lines to 615 + 949 (mostly tests) + 705 =
2269 lines at the roots; the remainder is spread across ten focused
submodules with clear responsibilities, each `pub(super)` annotation
documenting a real cross-concern boundary.

**Verified.** `cargo test --workspace --exclude nest` → 240 passed,
0 failed. (The `nest` binary's `mcp.rs` has unrelated WIP from a
concurrent session — `list_prompts` / `get_prompt` referenced but
not yet defined — so it's excluded from the run, not regressed.)

---

## 2026-05-28 — LLM-native bundle: incarnations + new MCP resources + externalized prompt

**Goal.** Activate the `docs/llm-native.md` plan's low-cost / high-impact
items that *ride on* the MCP work just landed — the "add now" bundle from
the analysis at the bottom of the MCP step-5 entry — and document the
remaining roadmap so the next session has a clear picture of what's open.

**Landed.**

- **`docs/incarnations.md`** (new) — the self-improving findings index
  ([`llm-native.md`](llm-native.md) §3). One paragraph per session: goal,
  blockers, surprises, "what I'd tell next-me", + a link to the full
  writeup. Format guide at the top so the next agent (or human) appends in
  the right shape without re-inventing it. First entry is the May 28
  Claude Opus 4.7 concurrent-Mandelbrot session.
- **Three new MCP resources** in `crates/nest/src/mcp.rs`:
  `brood://docs/incarnations`, `brood://docs/llm-native`,
  `brood://docs/claude-demo-findings`. Total resources: 8 (was 5). The
  agent's reads-first funnel is now: pocket reference → incarnations →
  CLAUDE.md. The forward-looking plan is one fetch away when wanted.
- **`docs/prompts/brood-task.md`** (new) — the `BROOD_TASK_PROMPT`
  constant pulled out of `mcp.rs` into a real markdown file,
  `include_str!`'d by the dispatcher. Two payoffs at once: the maintainer
  can edit the prompt without recompiling, *and* other agent harnesses
  (Cursor, Aider, Continue per `llm-native.md` §14) can drop the same
  file into their system prompts and get the same content. The new
  prompt body is 2009 chars (was 1157) — gained the incarnations
  pointer, the CLAUDE.md pointer, and the "don't `print` from `eval`"
  caveat (until step 1c-c lands).
- **Status block** at the bottom of `docs/llm-native.md` mapping each of
  the doc's 15 items to its current state — ✅ shipped (§1 / §3 / §14
  / §15 fully, partial on §2 / §5 / §6 / §12), ❌ open (§4 / §7 / §8 /
  §9 / §10 / §11), gated (§13 on §4). Picks out the next-highest-leverage
  item: **structured errors with stable codes** (§4) — the doc's own
  top-3 priority and the thing that turns every MCP `:error` field from
  prose into branchable data.

**Documented but deferred.** The status block in `docs/llm-native.md`
is the canonical "what's open" view. Highlights:

- **§4 structured errors with codes** is the next big move; touches
  `error.rs`, every raise site, and JSON encoding. Real project (≈ a
  session). Would let `try`/`catch` match on `:kind`, let `brood
  --explain E0042` print the doc page, and let the harness branch on
  `:user-fault false` (§13).
- **§7 examples-by-intent** (medium effort) unblocks `brood.find-pattern`
  as an MCP tool — "I need an actor pool" → a runnable example.
- **§6 `--watch --json` structured output** would close the LLM-as-REPL
  loop. The `--watch` flag exists; structured framing is small but
  separate from MCP.
- **§8 idiom-aware lints** (`prefer-match`, `prefer-transduce`,
  `no-fn-send`, `pin-or-bind`) — high-yield because LLMs make these
  mistakes *consistently* (60% of the time, vs. 1% for humans, per the
  doc). Lives in the type-checker pass.
- **§10 the gauntlet** — the measurement loop. Long-term.
- **`nest new .`** — small follow-up noticed during this session: the
  scaffolder errors on `.` (invalid name + dir-exists check). Allowing
  it to scaffold into cwd (deriving the name from the basename,
  skipping the existence check, overwriting existing scaffold files)
  is ~30 min in `std/project.blsp`. Recorded in the `llm-native.md`
  status block.

**Tests.** Updated `resources_list_includes_the_baked_doc_resources` to
assert the three new URIs are present. `prompts_get_returns_the_orientation_message`
still passes against the externalized prompt (the asserted markers —
`brood://docs/brood-for-claude`, "immutable", "MCP tools" — all
survived the rewrite). 28/28 nest tests green; LSP unchanged.

**Verified.** `cargo build` clean. Real-binary smoke: `resources/list`
returns 8 resources including the three new ones;
`resources/read brood://docs/incarnations` returns the 3 KB index;
`prompts/get brood-task` returns the 2 KB externalized prompt with the
incarnations pointer baked in.

---

## 2026-05-28 — Review pass + structured errors with codes (§4)

**Review fixes** to the MCP work just landed:

1. **`mcp-check-tool`** — `:diagnostics` wrapped with `(or … [])` so a
   clean project renders as `{:diagnostics []}` rather than
   `{:diagnostics null}`. Same disambiguation hazard `processes` solved.
2. **`run-tests-structured`** — `:results` and per-test `:failures`
   wrapped likewise; an empty suite or a passing test no longer renders
   as `null`.
3. **`mcp-tools` docstring** — was stale ("`check` / `run-tests` /
   `processes` ship as documented stubs"); rewritten to reflect that all
   eight tools are wired.
4. **`macroexpand_to_string`** — now **rejects** multi-form input
   rather than silently expanding only the first. Hides agent misuse;
   the error message points at the `(do …)` wrap.
5. **`cli_support` REPL stubs** — added `repl_interactive` /
   `repl_plain` stubs in the lib to unblock the build (the parallel
   refactor of `nest repl` references them; the real move from
   `crates/cli/src/main.rs` is left to that session). `nest repl` now
   prints "use `brood`" until the move completes.

**Structured errors with codes (§4 of `docs/llm-native.md`).** The
substrate the doc identifies as the top-3 next move, now shipped:

- **`LispError` gained `code: Option<&'static str>` + `hint: Option<String>`**
  (`crates/lisp/src/error.rs`). `ErrorKind` is `Copy` now (no data; safe).
  New `tag_name()` returns the stable lowercase keyword name (`"parse"`
  / `"unbound"` / `"arity"` / `"type"` / `"runtime"` / `"user"`).
- **`pub mod error_codes`** holds the stable strings (`E0001`,
  `E0010`, `E0020`, `E0030`, `E0099`). The numbering scheme groups by
  kind (`E00xx` parse, `E01xx` unbound, `E02xx` arity, `E03xx` type,
  `E04xx` runtime); once shipped, codes never get repurposed.
  Constructors (`parse`/`unbound`/`arity`/`type_err`/`wrong_type`/
  `runtime`) all set the code by default.
- **`LispError::to_value_map(heap)`** projects the structured fields
  into a Brood map: `{:kind <keyword> :message <string> [:code]
  [:file :line :col] [:hint]}` — every optional field omitted when
  absent. `try_catch` uses it when the LispError carries no user
  payload, so `(try (/ 1 0) (catch e e))` now binds `e` to a map
  rather than a rendered string. User throws (`(throw v)`) still
  rebind verbatim — only kernel errors get the wrapper.
- **MCP integration** (`crates/nest/src/mcp.rs`):
  - `RpcError` grew a `data: Option<Json>` field that rides on the
    JSON-RPC `error` object.
  - `RpcError::from_lisp(e)` projects a `LispError` into a JSON-RPC
    Internal error with `data` carrying the same structured shape as
    the Brood catch map.
  - `lisp_error_to_json` is the shared projector — the Brood map shape
    and the JSON shape stay parallel by construction.
  - `call_tool` uses `from_lisp` for any uncaught handler throw, so a
    project-defined tool whose handler doesn't `try`/`catch` still
    surfaces structured info.
- **`std/mcp.blsp`** gained `mcp--error-shape` (a coercer:
  built-in errors pass through, user throws become `{:kind :user
  :payload e}` so the agent always sees an object). Every handler's
  `(catch e …)` switched from `(str e)` to `(mcp--error-shape e)`.
- **`docs/error-codes.md`** (new) — the stable reference: catch shape,
  numbering scheme, current code table, "adding a new code" recipe,
  `:code` vs `:kind` branching guidance. Exposed via MCP as
  `brood://docs/error-codes`.
- **`docs/prompts/brood-task.md`** updated with the structured-errors
  bullet so the agent knows about `:kind` / `:code` branching from
  session start.
- **`docs/llm-native.md`** status block flipped §4 from ❌ to ✅;
  §13 (failure-mode tagging) noted as "substrate exists, per-site
  attachments still to be added."

**Tests landed.**

- `crates/lisp/tests/basic.rs::throw_and_catch` — adopted the new
  shape: `(try (/ 1 0) (catch e (map? e))) → true`,
  `(get e :kind) → :runtime`, `(get e :code) → "E0099"`, plus the
  matching unbound / type / arity assertions.
- `crates/lisp/tests/basic.rs::parse_errors_carry_position_in_catch_map`
  (new) — verifies `:kind :parse` and a positive `:line` in the catch
  map after `(eval-string "(unclosed")`.
- `crates/nest/src/mcp.rs::std_eval_tool_captures_a_runtime_error_as_a_structured_map`
  (renamed + strengthened) — pins `error.kind == "unbound"` and
  `error.code == "E0010"`.
- `crates/nest/src/mcp.rs::std_lookup_tool_handles_unbound_names_softly`
  (strengthened) — pins the same fields.
- `crates/nest/src/mcp.rs::argument_validation_throws_a_protocol_error`
  (strengthened) — also asserts `error.data.kind == "user"` (the new
  JSON-RPC `data` field).
- `crates/nest/src/mcp.rs::uncaught_handler_throw_projects_structured_data`
  (new) — installs an inline tool whose handler `(/ 1 0)`s without
  `try`/`catch`, asserts the JSON-RPC error has
  `data.kind == "runtime"`, `data.code == "E0099"`, and
  `data.message` contains "division by zero".
- `crates/nest/src/mcp.rs::resources_list_includes_the_baked_doc_resources`
  — extended to assert `brood://docs/error-codes` is in the resource
  list.

**Verified.** `cargo build` clean. `cargo test --workspace` green —
116 (lib) + 66 (basic) + 3 + 1 + 29 (nest, was 28) + 40 (LSP) + 13
(cli) = 268 tests, all passing. Real-binary smoke:
- `tools/call eval (no-such-fn 42)` → `{"error": {"code": "E0010",
  "col": 1, "kind": "unbound", "line": 1, "message": "unbound symbol:
  no-such-fn"}}` — full structured shape with position.
- `tools/call eval (/ 1 0)` → `{"error": {"code": "E0099",
  "kind": "runtime", "message": "division by zero"}}`.
- `tools/call lookup no-such-name` → soft `:error` map with the same
  shape.

**Deliberate trade-offs.**

- **User-throws stay verbatim.** `(throw 42) → (catch e e) → 42`
  preserved. Only kernel errors get the wrapper. The alternative
  (always-wrap) breaks dozens of catches across `tests/` and `std/`
  and forces every user to use `(:payload e)` for trivially-thrown
  values; the asymmetry is small and documented.
- **`E0099` is the runtime catch-all.** Every `LispError::runtime(...)`
  picks it up. Future work: split into more specific codes (a `E0040`
  for division-by-zero, `E0050` for IO failures, etc.). Done
  incrementally per `docs/error-codes.md`'s "adding a new code"
  recipe.
- **Position info is best-effort.** The reader's parse errors and the
  eval loop's `or_form_pos` cover most cases, but some kernel raises
  (the `(/ 1 0)` above) reach the catch without a position. Adding
  positions per raise site is a follow-up — the substrate already
  carries the optional fields end-to-end.

**What's left for §4 (incremental, not blocking).** Per-site `:hint`
attachments — a builder pattern is in place (`with_hint("…")`); the
scheduler-race hint from `claude-demo-findings.md` is the obvious
first candidate. Same for `:see` (link a code to its
`docs/<topic>.md#anchor`) — the field isn't on `LispError` yet but
would slot in alongside `:hint` when wanted.

---

## 2026-05-29 — `brood` / `nest` CLI cleanup + clap + arity-change reload diagnostic

**Goal.** The two binaries had grown messy: `brood --watch` had a single-arg
form with a 16-line caveat in the help text, `--watch` semantics differed
between `brood` and `nest run`, and there was no project-aware way to run /
test / check a *single* file — you were either fully project (`nest test`)
or fully project-blind (`brood --test foo`). User asked for a review; this
is the cleanup.

**Shape after the dust settles.**

```
brood                       REPL (language-only)
brood <file>...             run files
brood --test <file>...      run as tests (single-file utility)
brood --check <file>...     advisory type-check (single-file utility)
brood -j N                  concurrency cap
                            # no --watch here — see nest run --watch.

nest new <name>             scaffold
nest run                    run :main
nest run <file>             run a specific file (project sources on *load-path*
                            but not eager-loaded, so `src/foo.blsp` doesn't run
                            twice)
nest run --watch <path>...  hot-reload (file or dir, repeatable; dirs pick up
                            new files automatically). Single-file --watch with
                            no FILE: that file is promoted to the entry, so
                            `nest run --watch src/foo.blsp` reads naturally as
                            "run AND watch foo.blsp".
nest test [<file>...]       project-wide or scoped to listed files
nest check [<file>...]      same
nest repl                   project-aware REPL (currently stubbed; see below)
nest format [--check]
nest doc [module]
nest mcp
```

**Built.**

- **Switched both binaries to `clap` (derive feature).** Replaces ~150 lines
  of hand-rolled arg parsing across `crates/cli/src/main.rs` and
  `crates/nest/src/main.rs`. Free wins: typo suggestions (`brood --tst foo`
  → "a similar argument exists: `--test`"), uniform `--foo=bar`/`--foo bar`/
  `-fbar` handling, generated help from doc-comments, subcommand validation.
  `clap` is CLI-only (ADR-005 / CLAUDE.md allows `rustyline`-class deps in
  CLI crates), never in the brood lib.

- **Removed `brood --watch` entirely.** It had two shapes (single-arg
  run-and-watch with a footgun; two-arg watch-helper-while-running-entry)
  and a 16-line help-text caveat. Both flows live cleanly under `nest run
  --watch` now — the `nest` side never had the footgun because `:main` is
  called explicitly (not as a top-level form), so no re-execution on reload.

- **`nest run [<FILE>] [--watch PATH]... [args...]`** with one piece of
  ergonomic dispatch: when no FILE is given but exactly one `--watch <PATH>`
  is a regular file, promote that path to the entry. So `nest run --watch
  src/repeat.blsp` reads as "run and watch repeat.blsp" — the natural
  reading. Directories or multiple watch paths fall through to `:main` (no
  unambiguous promotion target). Inside a project, the FILE path gets
  `(project-setup root)` (puts `src/` on `*load-path*`) but *not*
  `project-load-sources` (which would double-execute a file under `src/`).

- **`nest test [<file>...]` and `nest check [<file>...]`.** With files, scope
  to those; without, whole-project as before. `nest test foo_test.blsp`
  inside a project loads project sources first so cross-module names
  resolve — the path was impossible before (had to choose between project
  scope or project-blind brood `--test`).

- **`nest repl`** added but currently stubbed — calling
  `brood::cli_support::repl_interactive` from nest needs the REPL helpers
  pulled out of `cli/main.rs` into the brood lib, which means adding
  `rustyline` as a lib dep. That's a real architectural choice
  (CLAUDE.md / ADR-005 currently bars it); left as a follow-up. For now,
  `nest repl` prints a "run `brood` directly" pointer.

- **Arity-change reload diagnostic** in `crates/lisp/src/eval/mod.rs`. When
  `def` rebinds a callable to one of a different arity — typical hot reload
  that breaks the caller-side contract — the evaluator prints `[reload]
  arity changed for X: A -> B` to stderr. Implementation: `value_arity`
  helper computes `Arity` from a `Value::Fn`/`Macro`/`Native` (closure:
  `params.len()` min, `+ optionals.len()` max when no `rest`; native: stored
  `arity`). Fires only on rebinding, so the prelude / std first-time builds
  are silent. Manual test: defining `greet` at arity 0 then 1 then 2, and
  shadowing the prelude's `inc` (1-arg) with a 2-arg fn produced three
  correct diagnostics.

  This intentionally does *not* change the underlying semantic — Brood
  follows the Lisp tradition (CL/Scheme/Clojure/Elisp) of in-place
  redefinition, with callers expected to be updated too. The diagnostic
  just makes the silently-broken-arity case visible at reload time instead
  of at the next call site. (User picked "add a diagnostic" over "leave
  as-is" or "treat arity-changed defns as new functions" — the last would
  deviate from every other Lisp.)

**Smaller fixes along the way:**
- Unknown-flag rejection in `parse_jobs_args` (the `--wathc` typo no longer
  silently becomes a file path; clap's parser catches the rest now).
- `getrandom 0.3 → 0.4`, `hmac 0.12 → 0.13`, `sha2 0.10 → 0.11` (+ the
  `KeyInit` trait import the new `hmac` requires).
- `examples/hot-reload/` is the canonical demo for the file-watcher
  pattern, written against `std/reload.blsp` (the require-able policy
  layer over `file-mtime`).

**Verified.** Per-crate `cargo test`: brood lib 187, brood-lsp 40, cli 13,
nest 29 — **269/269**. The new flows:
- `brood --tst foo` → "a similar argument exists: '--test'" (clap typos).
- `nest run --watch src/repeat.blsp` → runs repeat.blsp, hot-reloads on save
  (live demo: ticker swapped from `"first version"` to `"HOT-RELOADED via
  nest run --watch <file>"` mid-flight, no restart).
- `nest run --watch src` → runs `:main`, hot-reloads everything in `src/`,
  auto-picks up new files added during the run.
- `nest test tests/hello_test.blsp` → scoped test run with project context.

**Still open.**

- **`nest repl`.** Move REPL helpers (`repl_interactive`, `repl_plain`,
  `history_path`, `is_balanced`) from `crates/cli/src/main.rs` into
  `brood::cli_support`; add `rustyline` to the brood lib's deps. Real
  trade-off — `rustyline` was deliberately CLI-only — so wants explicit
  sign-off and probably an ADR. Stubs in place keep the build green.
- **Diagnostic configurability.** No env var to silence the arity-change
  diagnostic yet. Likely fine; can add `BROOD_NO_RELOAD_WARN` or similar if
  someone needs it during a noisy refactor.

---

## 2026-05-29 — `nest repl` proper: new `crates/repl/` crate

**Goal.** Promote `nest repl` from a "run `brood` directly" stub to a real
project-aware REPL with the same line editing + history as `brood`'s REPL,
without making the LSP pay for `rustyline`.

**Trade-off resolved.** `rustyline` is a CLI-side UX dep (ADR-005 / CLAUDE.md).
The straightforward path — putting REPL helpers in `brood::cli_support` —
would have pulled `rustyline` into every brood-lib dependent, including the
LSP, which has no REPL. Instead: a new thin workspace member, `crates/repl/`
(crate name `brood-repl`), holds the REPL + rustyline. `cli` and `nest` both
depend on it; `brood-lsp` doesn't.

**Built.**

- **`crates/repl/`** with `repl(interp)` as the one public entry point: the
  function dispatches `is_terminal` to either `repl_interactive` (rustyline,
  `~/.brood_history`, multi-line via `is_balanced`) or `repl_plain` (line-
  buffered, no prompts — for pipes / scripts). Both reclaim each command's
  LOCAL allocations via `heap.checkpoint()` + `reset_local_to(base)`; globals
  live in shared regions, so `def`s persist across commands. Five unit tests
  cover `is_balanced` (the multi-line gate): unclosed delimiters, comments
  swallowing delimiters, strings ignoring delimiters, escaped quotes.
- **`cli/main.rs`** lost ~140 lines: imports + main now end with
  `brood_repl::repl(&mut interp)`. The crate's `Cargo.toml` swaps
  `rustyline` for `brood-repl`.
- **`nest/main.rs`'s `cmd_repl`** drops the stub branches and calls
  `brood_repl::repl(interp)` after the project bootstrap. Inside a project
  it does `(project-setup root) (project-load-sources root)` first, so the
  prompt can call any project module directly (verified: `(greeting)` at
  the prompt resolves to the scaffolded `hello` module's defn).
- **`brood::cli_support`** lost its interim REPL stubs and the now-unused
  `use crate::Interp`.

**Workspace.** Added `crates/repl` to `members`. The crate has only two
dependencies (`brood` + `rustyline`), no dev-deps. `cargo build --workspace`
clean; full test sweep 274/274 (brood lib 187 + lsp 40 + cli 13 + nest 29 +
brood-repl 5).

**On the supervisor / process-REPL question** (user asked). The REPL stays a
plain loop on the main thread for now. Erlang-style "shell as a supervised
process" doesn't buy us much yet: Brood doesn't panic on user errors
(`LispResult` propagates), supervisors don't kill stuck processes (which is
the actual hot path for "user wrote `(loop)`" — needs explicit Ctrl-C /
eval timeouts), and we don't have an OTP-style supervisor abstraction.
When (a) `std/` grows a real supervisor framework and (b) the editor model
arrives where the REPL is one buffer of many, the move is worth revisiting.


---

## 2026-05-28 — Supervised-by-default processes (ADR-039); `defonce` removed

**Goal.** Push past the "wrap your loop in try/catch" / "add a supervisor
process" workarounds and find the right *language-level* answer for the
hot-reload pain — a redefinition that throws killing the running worker.
Land the design before M2 (the editor), because the editor's event loop
is *exactly* the shape this fixes, and we want M2 designed against the
new process model rather than retrofit.

**What changes for `defonce`.**

- The hot-reload-idempotence it provides is subsumed in the new design
  by **named-spawn** (`(spawn :worker expr)` — idempotent on the name)
  for the process case (its main use) and by "state lives in a process"
  for top-level state. **It is kept in the prelude as a transitional
  shim until ADR-039 lands** — first attempt: removing it now broke
  hot-reload immediately (two loops spawned on the second load). The
  macro now carries a docstring + comment flagging it as transitional
  with a pointer to `docs/supervision.md`; the actual removal lands in
  the same commit as named-spawn, so users have a working migration
  path.

**What I designed.**

- **ADR-039** — *Supervised processes with mode-gated resume checkpoints*.
  The model in one sentence: **a process is its current call**. The
  runtime captures `(callee, argv)` at every iteration boundary as a
  *resume slot*; an uncaught error triggers the supervisor to re-invoke
  the slot. Same function, same args, fresh code. State preserved.
  Immutability + late binding + the eval loop's existing `'tail:`
  checkpoint are the three properties that make this sound in Brood and
  unsafe in a mutable language.
- **`docs/supervision.md`** — the long-form design walkthrough. Covers:
  - **Why this works in Brood and not in mutable languages.** Erlang's
    gen_server/supervisor split exists *because* a worker that crashes
    mid-mutation can't be safely resumed; Brood has no mid-mutation to
    worry about. The split that occupies a chapter of every Erlang book
    collapses to "spawn it".
  - **Concrete behaviour for worker, editor, REPL, and one-shot
    scripts.** Worked example: a worker at `(my-loop 247)`, user saves
    bad code, throw, runtime catches, re-invokes `(my-loop 247)`, throws
    again, exponential backoff, user fixes + saves, next retry succeeds
    *with `num=247` preserved*.
  - **Mode-gating.** `dev` (default for REPL/`brood file`/`nest run`/`nest
    test`) pays per-call for full resume; `release` (default for `nest
    bundle` output / `--release`) does no per-call work — just catches
    at process boundary and restarts from spawn entry. `bare` for
    benchmarks. **The cost of hot-reload is paid only when the user is
    hot-reloading.**
  - **Simplifications that fall out.** `defonce` ✗, `live-loop` ✗
    (wouldn't need to exist), most user-level survival `try`/`catch` ✗,
    `std/reload.blsp`'s explicit `(try (load p) …)` becomes optional,
    `nest test`'s per-process crash-containment becomes universal,
    `std/hatch.blsp` simplifies as a layer-over-runtime rather than a
    full supervision framework, distributed-monitors (slice-3 work)
    keeps just the *notification* role, not the *restart* role.
  - **Performance.** Per-call cost: two stores (a `Value` + a `SmallVec`
    of args). Order-of-magnitude estimate: <1% on typical workloads,
    ~3–5% on tight recursive numeric loops. Optimisations:
    coarse-grained "checkpoint only at tail-call boundaries" (4×
    reduction in updates, same effective recovery semantics), cache-line
    co-location with the existing `tick` counter, monomorphised dev/
    release eval loops (no per-call mode branch). Wins from less
    defensive user code (every removed `try` is cycles back).
  - **Mode-gating answer to the user's question.** Yes — checkpointing
    is the cost of hot-reload survivability, and a release build doesn't
    pay it. Three modes (dev/release/bare), defaults per command surface,
    overridable via `--release` / `BROOD_MODE`.
  - **Open questions.** Side-effect duplication on resume (at-least-once
    semantics for I/O and messages); restart-storm protection (exponential
    backoff + max-restarts in N seconds, defaults documented); the
    script-mode resume (top-level form failure exits, doesn't retry —
    explicit decision, recorded).
  - **Migration & roll-out.** Behind the mode gate; first land with
    `(spawn … :supervised true)` as opt-in, then flip default once the
    test suite migrates. Two-phase commit to limit blast radius.

**Updated.**

- `docs/decisions.md` — ADR-039 appended (~3 KB of rationale + scope).
- `docs/supervision.md` — new long-form doc (~25 KB; mechanism, examples,
  performance analysis, what disappears, open questions, implementation
  sketch).
- `docs/roadmap.md` — M4 distributed-nodes block gains a ⬜ entry for
  supervised-by-default processes (designed; pre-requisite for editor
  design in M2).
- `docs/README.md` — `supervision.md` row added to the docs index.
- `std/prelude.blsp` — `defonce` removed.

**Why early.** Three pressures:

1. **The editor needs it.** M2 starts the editor; an editor-event-loop
   that dies on a bad redefinition is unusable. Better to design the
   process model first.
2. **It removes (not adds) abstractions.** `defonce`, `live-loop`, and
   most survival-pattern boilerplate go away. Net less surface to
   teach.
3. **It changes runtime semantics.** Designing in flight is fine for
   additive things; this is invasive on `process.rs` + `eval/mod.rs`.
   Capture the design fully first; implement once we agree it's right.

**Not changed yet.** No runtime code lands in this commit — purely
docs + the `defonce` removal. The design is captured well enough that
the implementation, when it happens, is a reading-comprehension
exercise on `supervision.md`.

**What's next** (per the roadmap):

- Decide whether supervised-by-default lands *before* the package
  manager (ADR-037) or M2 starts. My read: package manager first (1–2
  weeks, additive, doesn't touch runtime), supervised-by-default second
  (touches runtime, but mostly mechanical given the design), M2 third
  (designed against the new model from day one).

---

## 2026-05-28 — Polish round: `nest new .`, E0040 div-by-zero code, scheduler-race hint

**Goal.** Close the small follow-ups noted at the bottom of the structured-
errors entry: the `nest new .` ask, the first specific-code split of
`E0099`, and the first per-site `:hint` attachment (the scheduler-race
pointer from `claude-demo-findings.md`).

**Landed.**

- **`nest new .`** scaffolds into the current directory (`cargo init`'s
  shape). The project name is derived from `(path-basename (cwd))`, the
  existing-directory check is skipped, and existing scaffold files get
  overwritten — the user explicitly asked for this. The `Next:` line
  drops `cd .` when in-place. A new `path-basename` helper in
  `std/prelude.blsp` reuses the existing `path--last-slash` walker.
  Smoke: `cd /tmp/foo && nest new .` creates the scaffold + a manifest
  with `:name "foo"`; re-running overwrites a user-edited
  `src/main.blsp`.
- **`E0040` — division-by-zero specific code.** Both raise sites in
  `crates/lisp/src/builtins.rs` (`%div` line 412, `rem` line 452) now
  carry the code *and* a `:hint` (`"guard the denominator: (when
  (not= y 0) (/ x y))"`). The first concrete demonstration that the
  `with_code` / `with_hint` builder pattern from §4 carries through to
  the catch map and the MCP `error.data`.
- **Scheduler-race hint** on unbound errors raised inside a *green*
  process. New `process::in_green_process()` checks if `CURRENT` has a
  yielder (green coroutine vs. root thread). A new
  `eval::unbound_error(sym)` helper consolidates the two unbound raise
  sites (`eval/mod.rs:81` for symbol lookup, `:376` for call-head
  lookup) and conditionally attaches the hint:
  > this fired inside a spawned process — if it happens only under
  > fan-out load, the scheduler may be racing prelude lookups; try
  > `-j 1` (or `nest test -j 1`) to bound concurrency
  Conditioned on the process kind, not on the symbol name, so it's a
  best-effort pointer: false positives are tolerable (the hint
  *suggests* the cause, doesn't claim it). Documented from blocker §1
  of `claude-demo-findings.md`.

**Tests landed.**

- `crates/lisp/tests/basic.rs::scheduler_race_hint_attaches_to_unbound_in_green_processes`
  (new) — spawns a process that catches an unbound and `send`s the
  error map to the root; root asserts `(string? (get msg :hint))`.
- `crates/lisp/tests/basic.rs::unbound_in_root_thread_has_no_scheduler_hint`
  (new) — the negative case: the root thread's catch sees `:hint nil`.
- `crates/lisp/tests/basic.rs::throw_and_catch` — `(/ 1 0)` now asserts
  `:code "E0040"` (was `E0099`) and `(string? (get e :hint)) → true`.
- `crates/nest/src/mcp.rs::uncaught_handler_throw_projects_structured_data`
  — `(/ 1 0)` flipped to `:code "E0040"` to match the new specific code.

**Docs.**

- `docs/error-codes.md` — added the `E0040` row to the table; new
  "Hints" section documenting the two concrete attachments
  (div-by-zero, scheduler race) and the per-process conditioning.

**Verified.** `cargo build` clean. `cargo test --workspace`: 116 (lib)
+ 68 (basic, was 66) + 3 + 1 + 29 (nest) + 40 (LSP) + 13 (cli) = 270
tests, all passing. Smoke: `nest new .` in a fresh `/tmp/foo` produces
the expected scaffold and `:name "foo"` in the manifest.

---

## 2026-05-28 — `nest new` overwrites; `brood <nest-cmd>` points at nest

**Goal.** Two CLI papercuts. (1) `brood new foobar` died with a cryptic
`brood: cannot read new: No such file or directory` — `new` got parsed as a
FILE. (2) Re-running `nest new foobar` on an existing folder errored out; the
user wants both `nest new .` and `nest new foobar` to *overwrite* an existing
project's scaffold rather than refuse.

**Built.**
- `crates/cli/src/main.rs`: `nest_subcommand_misuse` — when the first FILE arg
  is a known `nest` subcommand (new/run/test/check/repl/format/doc/mcp) *and*
  isn't a real file on disk, print a friendly hint (`try: nest new foobar`) and
  exit 2 instead of the opaque read error. Keeps the brood/nest split clean
  (ADR-028) — `brood` runs the language, `nest` runs the project. A real file
  named after a subcommand still runs (existence check guards the heuristic).
- `std/project.blsp` `new-project`: dropped the refuse-if-exists guard. Both
  `nest new .` (in-place, basename as name) and `nest new foobar` now re-stamp
  the skeleton over whatever's there (`make-dir` is `mkdir -p`, `spit`
  overwrites). New `reusing-dir` flag only tunes the printed summary
  (`(existing files overwritten)` for the named form).

**Verified.** `cargo build` clean. `brood new foobar` / `brood new .` /
`brood run x.blsp` all emit the nest hint (exit 2); a real file named `test`
still runs. **Not** verified end-to-end: `nest new` itself currently panics in
prelude freeze (`heap.rs:469` "shared closures must capture the global env" +
a stray `DEBUG:` line) — concurrent WIP in `heap.rs`/`scheduler.rs`/
`prelude.blsp`, unrelated to this change. The overwrite path is correct by
construction; re-run the smoke test once that freeze assertion settles.

---

## 2026-05-28 — Specific runtime error codes (E0041–E0070) + a few more hints

**Goal.** Follow through on the §4 substrate that landed earlier today.
`E0099` covered every `LispError::runtime(...)` raise — useful as a
catch-all but coarse. This pass peels off the common families into
stable codes so agents can branch on *which kind of runtime failure*
without re-parsing the message string.

**New codes** (registered in `crates/lisp/src/error.rs::error_codes`):

| Code | Kind | Sites |
|---|---|---|
| `E0041` | `:runtime` | checked-arithmetic overflow (`%add`/`%sub`/`%mul`, `rem`); `floor` of a non-finite float or out-of-i64 value |
| `E0042` | `:runtime` | `vector-ref` / `substring` out-of-range index |
| `E0050` | `:runtime` | file IO (`load`, `slurp`, `spit`, `make-dir`, `list-dir`, `cwd`, `check-file`, `check-file-structured`) |
| `E0051` | `:runtime` | `run-process` couldn't start the subprocess (with a `:hint` about PATH) |
| `E0060` | `:runtime` | distribution layer: `node-start` / `connect` failed |
| `E0070` | `:runtime` | `send` saw a message value nested past `MAX_MESSAGE_DEPTH` (with a `:hint` about flattening/chunking) |

**Numbering shape.** Follows the `E04xx` / `E05xx` / `E06xx` / `E07xx`
lanes documented in `docs/error-codes.md` — `E004x` for integer/
index-shaped failures, `E005x` for IO/subprocess, `E006x` for
distribution, `E007x` for messaging. `E0099` stays the catch-all for
any uncoded `runtime(...)` raise; the goal isn't to eliminate it, just
to peel off the families an agent has a real branching reason to care
about.

**Per-site messages got tighter too.** `vector-ref` and `substring`
now report the bad index *with the valid range*:

```
vector-ref: index 7 out of range [0, 3)
substring: range [0, 99) out of bounds for length 2
```

Same information either way for a human; for an agent, the difference
between "out of range" and "index 7 out of range [0, 3)" is the
difference between guessing and knowing.

**Tests** added to `crates/lisp/tests/basic.rs::throw_and_catch` —
one assertion per new code that's tractable at unit level:

- `(* 9223372036854775807 2)` → `:code "E0041"`.
- `(vector-ref [1 2 3] 7)` → `:code "E0042"`.
- `(substring "hi" 0 99)` → `:code "E0042"`.
- `(slurp "/does/not/exist/anywhere")` → `:code "E0050"`.

The codes for `E0051` (subprocess — needs a missing binary),
`E0060` (distribution — needs a live peer story), and `E0070`
(message too deep — hundreds of nested levels) aren't unit-tested.
They ride through `to_value_map` → `lisp_error_to_json` without any
code path of their own, so the four tested sites cover the projection
end-to-end.

**Docs.** `docs/error-codes.md` table extended with seven new rows;
the "Hints" section gained two entries (subprocess, message-depth) so
the doc stays honest about which raises carry actionable next-step
text.

**Verified.** `cargo build` clean. `cargo test --workspace`: 116
(lib) + 68 (basic) + 3 + 1 + 29 (nest) + 40 (LSP) + 13 (cli) = 270
tests, all passing, no regressions.

**What's left for §4** (still incremental, well-motivated):

- **`E0021` — too-many-args** vs. `E0020`'s too-few-args, when the
  arity check can tell which side fired.
- **`E0061` — handshake failure** vs. `E0060`'s generic distribution
  failure (cookie / nonce / MAC mismatch carries different agent
  guidance).
- **Special-form malformed** raises in `eval/mod.rs` (`let: missing
  bindings`, `letrec: missing bindings`, `fn: missing parameter
  list`) are currently `LispError::runtime(...)` → `E0099`. They're
  programmer errors at *parse* time really; a `:kind :parse` recode
  with `E000x` codes would be the natural shape but breaks the
  "Parse = reader-failed" framing today. Worth a follow-up.

The bigger LLM-native picture (`docs/llm-native.md`) is unchanged:
catch-shape + codes are now structured enough for the agent to branch
programmatically; the remaining moves are §7 (examples-by-intent),
§8 (idiom lints), and §10 (the gauntlet).

---

## 2026-05-28 — Stdlib gap-fill: map + sequence ops; std/examples style sweep

**Goal.** Round out the standard library against what a general-purpose
Lisp/Clojure-style stdlib carries, then sweep all `std/*.blsp` and the
examples for the current preferred style. All-Brood, no new kernel.

**Added to `std/prelude.blsp` (all Brood, all tail-recursive/bounded):**

- *Maps* — `merge` / `merge-with` (variadic, rightmost-wins; `nil` maps
  skipped), `update` (apply-f-at-key with threaded args), `update-vals` /
  `update-keys`, `select-keys`, `zipmap`, and the nested-path trio `get-in` /
  `assoc-in` / `update-in` (`assoc-in` coerces a non-map node to `{}` so it
  creates intermediate maps; `update-in` is `get-in` then `assoc-in`). Built on
  `reduce-kv` + the 3-arg kernel `map-assoc` (no per-step rest-list).
- *Sequences* — `remove` (eager complement of `filter`) and `keep` (the eager
  `xkeep`), `distinct` (first-occurrence, O(n) via a map-as-seen-set) /
  `dedupe` (consecutive-run collapse), `group-by`, `flatten` (splices nested
  lists; vectors/maps are leaves), `interpose` / `interleave`, `take-last` /
  `drop-last`, `repeat` / `repeatedly`.

Multi-collection `map` (`(map f xs ys)`) was **deliberately not** added: the
prelude keeps `map`/`filter` fixed-arity for the no-rest-list fast path (see the
`x*` transducer note), and `(map (fn ([a b]) …) (zip xs ys))` covers it.

**Style sweep (3 parallel review agents over std + examples):**

- **Real bug fixed:** `std/test.blsp` redefined the global `take` with a
  *non-tail* version (and re-defined `quot`). Because the runtime shares one
  global table across all loaded modules (ADR-013), `(require 'test)` was
  silently clobbering the prelude's stack-safe `take` for the whole image.
  Both local redefinitions deleted — the prelude's are used. (`quot` was
  identical; `take` was strictly worse.)
- **Modernized module headers to `defmodule`:** `std/project.blsp` and
  `std/docs.blsp` still used the legacy *bare-leading-string + trailing
  `(provide 'x)`* pattern; `std/test.blsp` had a bare leading string and no
  `provide` at all. All three now lead with `(defmodule name "…")` (which both
  documents and provides), so `(module-doc 'project)` etc. now return their
  docstrings. (`docs--leading-doc` still reads legacy bare-string docs, by
  design — third-party/older modules.)
- **`std/mcp.blsp`:** the `check` / `run-tests` tool `:description`s still said
  "(stub — see docs/mcp.md step 1c)", contradicting the file's own docstring
  (the stubs were replaced long ago). Updated to describe the real return
  shapes.
- **Examples:** `examples/wilhelm.blsp` was a placeholder (`(defn fib (x) 1)`)
  → a real recursive `fib` with a docstring; `examples/hot-reload/greeter.blsp`
  gained a `defmodule` header; `examples/processes.blsp`'s "green-ish processes
  on real OS threads" comment corrected to "green processes multiplexed over a
  worker-thread pool". `life`/`tour`/`processes`/`node_*`/`main` reviewed clean
  (all builtins they call still exist; no `defonce`/`set!`/removed forms).

**Tests.** New `describe` blocks in `tests/maps_test.blsp` (merge/merge-with,
update family, select-keys/zipmap, nested get-in/assoc-in/update-in) and
`tests/sequence_test.blsp` (remove/keep, distinct/dedupe, group-by, flatten,
interpose/interleave, take-last/drop-last, repeat/repeatedly) — each `test`
already runs in its own green process (multi-core coverage); the files' existing
`:isolated` blocks carry the explicit spawn/send/fan-in coverage. `docs/language.md`
Maps table + the Lists/Maps builtin lists updated.

**Docstrings — public API pass.** Audited every Brood `defn`/`defmacro` in
`std/` for a docstring (via runtime `(doc fn-value)`, the same lookup `nest doc`
and LSP hover use). The prelude's public functions were already complete; the
gaps were the std *modules'* public entry points. Documented the user/tool-facing
surface (scoped to public API — `--` helpers stay private by convention):
`test`'s framework macros (`describe` / `test` / `deftest` / `is` / `refute` /
`assert=` / `assert-error` / `error-of`) and `run-tests` / `run-tests-structured`;
`project`'s `check-project{,-structured,-sources}` / `run-project{,-tests,
-tests-structured}` / `config` / `load-config` / `new-project`; `docs`'s
`document-{file,module,project}` / `generate-docs`; and the prelude's `defn`
macro itself. `format`/`mcp`/`hatch`/`reload` public surfaces already had
docstrings. Verified all 24 now return a docstring from `doc`.

**Verified.** `maps_test` + `sequence_test` run green in isolation —
**113/113** pass. Targeted load-and-run because the **full** `cargo test`
suite currently SIGSEGVs in `tests/suite.rs` (a green-process stack overflow in
`format_test`) — that is **concurrent runtime WIP** in the working tree
(uncommitted `core/heap.rs` +129, `process/scheduler.rs` +201, plus
`eval/mod.rs` / `mailbox.rs` / `process.rs`; today's earlier entry flags the
in-progress freeze there), **not** these stdlib changes: every `std/*.blsp`
edit here is `.blsp`-only, and the new prelude functions all verify standalone.
Re-run the full suite once the scheduler/heap work settles.

---

## 2026-05-28 — LSP: cross-file & standard-library goto-definition

**Goal.** From editor work (`brood.el`): `M-.` couldn't jump to a definition in
another project module, nor into the standard library. The cross-file substrate
(ADR-031 steps 1–2: def-site recording + `(source-location 'name)`) was built,
but the LSP wiring (steps 3–4) wasn't.

**Built.**
- **Cross-module goto** (`crates/lsp/definition.rs`, `main.rs`): a name that
  resolves `Free` in the buffer now falls back to `introspect::source_location`
  against the bootstrapped `Interp`, projected to a cross-file `Location` (new
  `path_to_uri` helper, the inverse of `uri_to_path`). The first `didOpen` under
  a `project.blsp` already bootstraps the project, so a module's `def`s are in
  the def-site table by the time a goto request arrives. Verified end-to-end
  against a `nest new` project (`greeting` in `main.blsp` → `hello.blsp`).
- **Standard-library goto** (`core/heap.rs`, `lib.rs`): the prelude is
  `include_str!`'d, so it had no on-disk source to land on. The prelude build now
  *materializes* a copy to `$XDG_CACHE_HOME/brood/prelude.blsp` (fallback
  `~/.cache`), sets `current-file` to it, reads positioned, and records each
  prelude def's site. Those sites are immutable, so they live in `SharedCode`
  (drained from the builder's `RuntimeCode` in `freeze_as_shared_code`);
  `Heap::def_site` checks the runtime table first (a user redefinition wins) then
  the prelude. `(source-location 'map)` → the cache copy; Rust primitives
  (`cons`, `rem`, …) stay `nil` (no Brood source; hover still documents them via
  `PRIMITIVE_DOCS`). The cache is rewritten only when a build's embedded prelude
  differs. Best-effort: an unwritable cache just means stdlib goto is
  unavailable, nothing else.

**Also corrected stale docs.** `docs/lsp.md` Status/roadmap (semantic
diagnostics — unbound/arity/type-misuse — and cross-file goto are live, not
"next"); the `source-location` doc comments in `builtins.rs`/`introspect.rs`
(prelude globals *do* resolve now); and `brood.el`'s commentary. The diagnosis of
the reported "`println` hover shows two `(println & xs)` lines, no docs" landed
client-side: the server returns signature **and** docstring (verified over
stdio); eglot composes `signatureHelp` + `hover` in the echo area and truncates
the doc — an eldoc-config matter, not a server bug.

**Tests.** New `definition::falls_back_to_a_loaded_modules_def_site` (writes a
temp module, `load`s it, asserts the cross-file `Location`); updated
`introspect::source_location_resolves_prelude_fns_but_not_builtins_or_unbound`
(was asserting prelude globals had no site). `brood-lsp` 41/41 and `brood --lib`
123/123 green. (The full `cargo test` still shows one failure —
`supervisor_retries_last_iteration_with_same_args` in `tests/basic.rs` — which is
**concurrent supervision/scheduler WIP** in the working tree, unrelated to these
LSP/heap changes.)

---

## 2026-05-28 — Supervised processes step 2: runtime supervisor + mode gate

**Goal.** ADR-039 step 2 — wrap each green process's eval in a catch-and-retry
loop so a `def` rebinding that throws doesn't kill a long-running stateful loop
(the editor / REPL / any process holding accumulator state). Step 1 (named
`(spawn :name expr)` + reaping on death) landed earlier; this is the recovery
substrate that makes step 1 useful — without it, a buggy reload still loses
the process, and named-spawn just becomes "respawn from scratch". Per the
design, step 3's **mode gate** got pulled forward into the same change: the
supervisor is intrusive enough that an always-on default would change the
semantics every existing test (and every user) expects (e.g. a `(throw)` in
a spawned process now retries 10× over ~2 s before monitors fire). Off by
default; opt in for dev/hot-reload mode.

**Built.**

- **Resume slot** (`crates/lisp/src/process/scheduler.rs`). A
  `ResumeSlot { callee, argv }` thread-local — the per-process pointer to
  the *most recent tail-call boundary*, what the supervisor re-invokes on
  recovery. Save/restore around every coroutine suspend (`preempt` and
  `wait_for_message`), the same shape `GC_BLOCK` already had, so a worker
  multiplexing several processes doesn't leak one process's slot into
  another's recovery. Wiped at coroutine start.
- **Eval-loop hook** (`crates/lisp/src/eval/mod.rs`). At the `Value::Fn(id)`
  tail-call dispatch, three guards (in cheapness order) decide whether to
  update the slot: supervision-enabled (atomic load), in a green process,
  and `gc_block_depth() == 1` (outermost eval frame). The third guard is
  load-bearing — without it every `(- n 1)` and `(empty? xs)` overwrites
  the slot many times per outer loop iteration, burying the value we
  actually need to retry. `gc_block_depth == 1` is exactly "this is the
  spawn entry's own tail loop, not a helper running inside it"; verified
  by running the supervisor test against the recursive worker and
  observing the slot retains its `argv=[0]` even though `=`, `chain?`,
  `empty?`, `-`, `fold` (all `defn` in the prelude — they go through
  `Value::Fn`) execute many calls inside the same iteration.
- **Supervisor body** (`scheduler::supervise`). Replaces the bare
  `eval::apply` the spawn coroutine had: catch a `LispError`, log, sleep
  the exponential backoff (1 ms → 1 s, doubling), `take_resume()`, retry.
  Circuit-breaker at `MAX_RESTARTS = 10` consecutive failures — after that
  the process exits with `[:error <last-msg>]` and monitors fire. When
  no tail call recorded yet (the entry itself threw, no inner loop ran),
  fall back to re-invoking the spawn entry with no args — state-loss
  restart, the worst-case recovery — instead of giving up immediately.
- **Mode gate** (`scheduler::SUPERVISION` atomic + `is_supervision_enabled`).
  Default off. First-call resolution from `BROOD_SUPERVISE=1` then cached;
  `(set-supervision! true)` flips it at runtime. Both the eval-loop hook
  *and* the supervise loop consult the same atomic, so a release build
  pays exactly one relaxed load + branch per tail call (no slot writes,
  no clone) and the supervise loop short-circuits to a single
  `eval::apply` (the let-it-crash behaviour the rest of the suite
  expects). Exposed to Brood as `(set-supervision! on?)` /
  `(supervision?)` builtins.
- **Test** (`tests/basic.rs::supervisor_retries_last_iteration_with_same_args`).
  A worker that sends `[:iter n]` to its parent then throws at `n=0`,
  verified end-to-end: messages `3, 2, 1, 0` from the first descent, then
  `0` repeated *exactly* 10 more times (the restart budget) before the
  supervisor gives up. Verifies (a) `record_resume` captures `(callee,
  argv)` per outermost tail call, (b) the supervisor catches the throw
  and re-invokes with the *same* argv (we see `0` 11×, not `3, 2, 1, 0`
  again — proves `take_resume` returned a slot, not None), (c) the
  restart counter actually fires.

**Why mode-gating won.** I landed step 2 with supervision always on and ran
the full suite — `dynamic_test.blsp`'s `dt-crasher` monitor test was
waiting 500 ms for `[:down …]`, but the supervisor now retried with
exponential backoff that exceeded the timeout. A handful of similar tests
fail the same way. Two options: rewrite every test that relies on
immediate-crash semantics (broad; we'd hide bugs under "let it crash" too),
or land step 3's mode gate now and have supervision opt-in. Picked the
latter — see ADR-039: the gate was always part of the design and pushing
it later just delays the truth that *most code wants the let-it-crash
default*; supervision is the hot-reload affordance.

**What's still open.** The CLI / `nest` haven't been wired to set the gate
yet (a `--supervised` flag, or `nest dev` flipping it on, or detecting an
interactive TTY). That's a single-line change once we decide the policy
— I left it for the user's call. Spawn-link (lifecycle bonding,
distinct from named-spawn's idempotence) is still pending. The
recovery semantics around inner-helper tail-recursion are coarse-grained
by design (we retry the outermost loop's iteration, not the innermost
frame); revisit if a real workload turns up where that's wrong.

**Tests.** `cargo test -p brood --test basic` 72/72 green (was 71 with one
flagged failure; the resume-slot fix moved it to pass, and the parallel
prelude-cache change broke the now-fixed `source_location_records_def_sites_from_a_loaded_file`
assertion about `'map` resolving to `nil` — updated to match the new
behaviour the user landed in the LSP/heap commit above). `gc.rs` 3/3 and
`preemption.rs` 1/1 still green. The in-language `suite_test.blsp`
segfault is **pre-existing parallel WIP**, not from this change — confirmed
by running with supervision off (the default), where the now-uncaught
`:boom` throws print `process N died: …` immediately rather than being
caught + retried.

---

## 2026-05-28 (cont.) — LSP Tier 2: references, rename, semantic tokens, polish

**Goal.** From editor work, a batch of LSP improvements: the remaining Tier-2
features plus two reported gaps — goto on a `require`'d module name, and hover
docs not showing.

**Built (all over the existing CST + scope substrate, no new analysis layer).**
- **`require`-target goto** — `definition.rs` detects a `(require 'foo)` call
  context and resolves the module via `introspect::module_file` (new): runs the
  prelude's `require--find "foo.blsp" *load-path*` against the bootstrapped
  project's load-path, lands at the file top. (`'hello` → `src/hello.blsp`.)
- **Find-references + document-highlight** (`references.rs`) — both off
  `ScopeTree::references`; a local stays scoped, a document global spans the file.
- **Rename + prepareRename** (`rename.rs`) — same engine → a single-file
  `WorkspaceEdit`; new name validated through `syntax::atom::classify`
  (rejects numbers/keywords/delimited junk). Single-file by design (no
  cross-file reference index — ADR-031).
- **Semantic tokens** (`semantic_tokens.rs`, `semanticTokens/full`) — CST + scope
  walk: `def`-family head → keyword, defined name → function+`definition`, locals
  → variable, call heads → function, `:kw` → enumMember, strings/numbers/comments
  classified; multi-line tokens split per line; delta-encoded.
- **Completion polish** — offers the special forms / core macros (not in the
  global table, so previously never suggested), kinds split keyword/function/
  variable, and `completionItem/resolve` fills signature (`detail`) + docstring
  lazily so the list stays cheap.
- **Finer diagnostic spans** — `refine_diagnostic_range` narrows an
  `unbound symbol: X` squiggle to X's token (else the call operator), instead of
  a 1-char marker at the form start.
- **publishDiagnostics version** — `Document` now carries the editor version and
  echoes it, so clients can drop stale diagnostics.
- **Hover docs (client side)** — `brood.el` sets `eldoc-documentation-strategy`
  to compose + `eldoc-echo-area-use-multiline-p t` in `brood-mode`, so the
  docstring the server already returns isn't hidden behind signature help / cut
  off by the echo area (the reported "two `(println & xs)` lines, no docs").

**Verified.** Drove the real `brood-lsp` over stdio end-to-end: all nine
providers advertised; references=3 and rename→3 edits on a def; 11 semantic
tokens for a sample; completion offers `let` + `map`, resolve(map)→`(map f
coll)`; unbound `frobnicate` squiggle spans the exact token; diagnostics carry
`version`. `brood-lsp` 51/51, `brood --lib` 123/123; `brood.el` byte-compiles
clean. (The unrelated `supervisor_retries…` failure in `tests/basic.rs` — the
concurrent scheduler WIP — is still there and still not ours.)

---

## 2026-05-28 (cont.) — MCP server: fix the stdio transport (was unusable by real clients)

**Symptom.** "The MCP server isn't working." Driving `nest mcp` by hand showed
the full surface responding — initialize, all eight tools, resources, prompts —
but only when fed **`Content-Length`-framed** input.

**Root cause.** `crates/nest/src/mcp.rs` framed messages LSP-style
(`Content-Length: N\r\n\r\n` + body). The **MCP stdio transport is
newline-delimited JSON** — one compact JSON-RPC object per line. A real client
(Claude Code) sends newline-framed bytes, which the server rejected with
"missing or malformed Content-Length", so `initialize` never completed and the
connection died. The server looked fine in isolation only because **its own test
harness `frame()` also used `Content-Length`** — so the tests round-tripped
through the same wrong framing and stayed green. A classic "tested against
itself, never against the protocol" trap.

**Fix.** `read_message` now reads a line and parses it as JSON (skipping blank
separators, clean EOF → stop); `write_message` writes compact body + a single
`\n`. Updated the test harness `frame()`/`unframe()` to match, and added
`transport_is_newline_delimited_json_not_content_length` to lock it in (asserts
a newline request parses, output is `{...}\n`, and a stray `Content-Length:`
header line is *not* accepted). `nest` 30/30 green.

**Made it live.** `make install` rebuilt + replaced `~/.local/bin/{brood,nest,
brood-lsp}` (release) — the `.mcp.json` `nest new` scaffolds launches bare `nest`
from PATH, so the installed binary is what actually serves Claude Code; the
working-tree fix is inert until reinstalled. (Same applies to the brood-lsp Tier-2
work from earlier today — the installed LSP was stale too; now refreshed.)
Verified end-to-end against the *installed* `nest mcp` over newline framing:
`initialize` → serverInfo, `lookup reduce` → arglist+doc.

**Doc.** `docs/mcp.md` gained a "Transport — newline-delimited JSON, not LSP
framing" note contrasting it with `brood-lsp`, so the two servers' identical
*shape* doesn't tempt a future copy of the wrong framing.

---

## 2026-05-28 (cont.) — Supervisor follow-up: hot-reload + GC roots

**Goal.** Two bugs surfaced on the supervised processes work that landed
earlier today (ADR-039 step 2): (1) on retry, the supervisor was calling
the **captured closure handle** — so a `(def my-loop …)` between throws
didn't take effect, defeating the whole point of integrating supervision
with hot reload. (2) `RESUME_SLOT.callee` wasn't a GC root, so a collection
between `record_resume` and the supervisor's `take_resume` could free the
closure the slot points at, leaving the retry to call into a reused slot
(observed as "the supervisor 'succeeded' after 4 retries instead of
running to the budget").

**Fixed.**

- **Name-based re-resolution on retry**
  (`crates/lisp/src/process/scheduler.rs`, `eval/mod.rs`). `ResumeSlot`
  gained a `name: Option<Symbol>` field — the closure's `defn`-given
  name. Eval's `Value::Fn` hook reads `heap.closure(id).name` and passes
  it through `record_resume`. The supervisor, on retry, looks the name
  up in the global env: a freshly-`def`'d closure wins, falling back to
  the stored handle if the name no longer resolves to a `Fn`/`Native`
  (someone `def`'d it to a non-callable, or `undef`'d it). This is the
  hot reload `def`-rebinding contract (ADR-013) flowing through
  supervision (ADR-039). Anonymous `(fn …)`s carry `name: None`, so
  they retry by handle — that's the only fallback path we can offer
  without a name.
- **`RESUME_SLOT` is a GC root**
  (`crates/lisp/src/process/scheduler.rs::for_each_resume_root`,
  `eval/mod.rs`). New `for_each_resume_root(visit)` walks the current
  thread's slot, calling `visit(slot.callee)` then once per
  `slot.argv[i]` — zero allocation, hot-path-safe. The eval safepoint
  builds a `SmallVec` of roots (`expr` + the slot's contents) and hands
  it to `heap.collect`. Without this, a collection between
  `record_resume` and `take_resume` could free a LOCAL closure /
  vector / pair the slot points at; the supervisor then retries into
  a slot that's been reused for an unrelated value, and the process
  silently "succeeds" or behaves erratically.
- **`in_green_process` no longer panics on contended borrow**. Was
  `c.borrow().…`, now `c.try_borrow().…`. Today no in-crate path
  takes `CURRENT.borrow_mut()` and then evaluates an RHS that calls
  `in_green_process` — verified by an audit of all five borrow_mut
  sites (`mailbox.rs:263`, `scheduler.rs:290`/485/502/707). But the
  supervisor's eval-loop guard runs `in_green_process()` on **every
  tail call**, so a future change that introduces such a path would
  otherwise panic mid-iteration with "RefCell already borrowed" rather
  than continuing safely. Returning `false` on a contended borrow
  degrades gracefully — the recovery slot just isn't written for that
  one call, the supervisor still does its job on the throw.

**Test.**
`crates/lisp/tests/basic.rs::supervisor_picks_up_hot_reloaded_definition_on_retry`
— end-to-end: spawn `hr-worker` that throws on every iteration, sleep
200 ms while the supervisor catches a few times, `def` a fixed
`hr-worker` that sends heartbeats, then `receive` two heartbeats and
assert the worker is running the new code (first beat: `[:beat 0]` —
proves the fix took on the next retry; second beat: `[:beat 1]` —
proves the *new* closure tail-recurses normally, not just one-shot
recovery into something that exits). With the bug present, the test
times out on the first `receive` — the supervisor keeps calling the
captured old throwing handle forever.

**Tests.** `cargo test -p brood --test basic --test gc --test
preemption`: 73 + 3 + 1 green (+1 new — the hot-reload test).
Workspace-wide: 301/302; the one outstanding is `tests/suite.rs`'s
in-language segfault, still parallel WIP unrelated to this change
(confirmed: supervisor default-off here, processes just `die: …` on
throw immediately, no supervisor involvement).

**Smoke.** `/tmp/race-repro/hotreload.blsp` — manual hot-reload story:
spawn buggy worker, parent waits, parent `def`s fix, parent reads
heartbeats from new worker. Prints `[:got 0]` then `[:got 1]` —
indistinguishable from the test, kept around as the user-facing demo
for the next devlog or readme.

---

## 2026-05-28 (cont.) — Cross-file references & rename (LSP) + the MCP `callers` tool

**Goal.** Whole-project find-references and rename, shared between the editor
(LSP) and agents (a new MCP tool). The substrate was the missing piece; ADR-031
already sanctions a *static* cross-file reference model (definitions image-based,
references stay static), and the flat module system (ADR-019) makes a global one
binding everywhere — so the reference set is just the union over project files.

**Shared core (brood lib).**
- `ScopeTree::references_to_global(root, src, name)` — occurrences of `name` that
  resolve to the file's global (a top-level `def`) or are free, *excluding*
  locals that shadow it. The per-file primitive both consumers union over.
- `introspect::project_files(interp)` — the project's source+test files via
  `(project--all-files *project-root*)`.
- `(references-in-source name source)` primitive — pure (parse a string → list
  of `[line col]`); the mechanism the Brood/MCP side maps over files.

**LSP (`crates/lsp`).** New `workspace.rs`: for a global/free name, union
`references_to_global` over `project_files` (preferring open-buffer text over
disk), producing cross-file `Location`s; rename emits a multi-file
`WorkspaceEdit`. The references/rename handlers now dispatch on resolution —
**local → single-file** (the existing cursor-keyed path), **global/free →
cross-file**; no project → degrades to the open buffer. Verified end-to-end:
references on `greeting` (free in `main.blsp`) found it across `hello.blsp`,
`main.blsp`, and `hello_test.blsp`; rename → a 3-file `WorkspaceEdit`.

**MCP (`callers` tool).** `std/mcp.blsp` gains `mcp-callers-tool`: maps
`references-in-source` over `(project--all-files *project-root*)` (read via
`slurp`) → `{:references [{:file :line :col} ...]}`. It's the *use*-site
counterpart to `lookup`'s def site. Verified against the real `nest mcp`:
`callers(greeting)` returned all three files' occurrences. Ninth tool in the
catalogue.

**Tests.** `scope::references_to_global_collects_globals_and_frees_but_not_locals`;
the dispatcher catalogue test now asserts `callers`. `brood` 124, `brood-lsp` 51,
`nest` 30 — all green. New code clippy-clean (the pre-existing brood-lib + nest
module-doc warnings are untouched).

**Docs.** `docs/lsp.md` (cross-file refs/rename section + roadmap), `docs/mcp.md`
(ninth tool), this entry.

---

## 2026-05-28 — Std style review, codified conventions, `writing-brood` skill

**Goal.** Review the standard library for style, fold what it consistently does
into the shipped style guide, and ship a Claude skill that helps an assistant
write idiomatic Brood. Later in the session: surface the MCP server as the
coding loop.

**Review.** Swept `std/` (`prelude`, `format`, `project`, `test`, `mcp`,
`reload`, `hatch`, `docs`) against the nine style rules. **Zero violations** —
binding forms are lists, `let` is flat, `cond` uses `:else`, no mutation, loops
are tail-recursion/`fold`/`map`, public `defn`s carry docstrings. The std is
already the canonical example it claims to be. What was *missing* was a written
record of the conventions it follows implicitly.

**Guide (`docs/brood-for-claude.md`).** New **"Naming & docstrings"** section
codifying the conventions the std follows without exception: `foo?` predicates,
`*foo*` dyn/module vars, `foo--bar` private helpers (double-dash infix),
`foo->bar` conversions, no `!` (nothing mutates); tail-recursive public-shell +
`--acc`/`--loop` worker split; first-line-summary docstrings with markdown;
`(defmodule name "…")` openings; `"fn-name: what went wrong: " val` error shape.

**Skill (`.claude/skills/writing-brood/SKILL.md`).** A triggerable checklist of
the traps an LLM hits writing Brood like Clojure/Scheme/CL (no mutation, no
loops, lists-for-code/vectors-for-data, bind-vs-match patterns, truthiness),
plus the naming/shape rules and an **MCP-server coding loop** section (`eval` →
`load` → `eval` a call → `macroexpand` → fix, via `nest mcp`). Baked into the
binary as `(%builtin-doc 'writing-brood-skill)` and scaffolded by `nest new`
into every project's `.claude/skills/`, mirroring how `brood-for-claude.md`
ships. Scaffolded `CLAUDE.md` now points at the skill too.

**Verified.** `cargo build` green; scaffolding a fresh project drops
`.claude/skills/writing-brood/SKILL.md` and the project's own `nest test`
passes. `brood` Rust tests 124 green; `nest`/`lsp`/`gc`/`preemption` green.

**Known issue (not ours).** The in-language `suite` (`nest test` from the repo
root, `cargo test -p brood --test suite`) **segfaults deterministically** in the
green-process/scheduler path — the signature of the known "deep non-tail
recursion overflows the coroutine stack" failure (see CLAUDE.md). It reproduces
independent of this session's changes (all additive: an embedded-doc const,
markdown, and the `new-project` scaffolder — none in `run-project-tests`' path)
and coincides with the uncommitted `scheduler.rs` (+77) / `scanner.rs` WIP that
was already in the tree. Flagged for the scheduler work, not fixed here.

---

## 2026-05-28 (cont.) — Review pass on the LSP + MCP code (shared core, bug fixes)

Detailed review of the editor (LSP) and agent (MCP) surfaces, plus an
independent adversarial pass. Three real bugs and one sharing win; several
reviewer findings were rejected as false positives (recorded so they're not
re-investigated).

**Fixed.**
- **Quoted symbols counted as references (real, in the *shared* core).**
  `scope::collect_symbols` descended into `'…` quotes, so `references_to_global`
  — used by LSP references/highlight/rename *and* the MCP `callers` tool via the
  `references-in-source` primitive — treated the module name in `(require 'foo)`
  and quoted data `'(a b)` as references. A cross-file rename of `foo` would have
  rewritten `(require 'foo)` to point at a different module and mutated quoted
  literals. Fix: `collect_symbols` no longer descends into `Quote` nodes (one
  change, all consumers fixed). Quasiquote left as-is (its `~x` parts are live).
- **`uri_to_path` mishandled a host authority.** `file://localhost/p` (some WSL
  / remote clients) decoded to the *relative* path `localhost/p`, so
  `find_project_root` silently never fired — no diagnostics, no cross-file. Now
  strips the authority; `file:///p` and `file://host/p` both yield `/p`.
- **`project_sources` overlay keyed by URI string.** The open-buffer overlay and
  current-file dedup matched on the raw URI, but our `path_to_uri` and an
  editor's URI can differ in percent-encoding for the same file — which would
  miss unsaved edits and list the file twice (double edits on rename). Now keyed
  by the **decoded path**.
- **MCP `callers` aborted on one unreadable file.** `(slurp f)` throwing (a file
  deleted/permission-denied between listing and read) failed the whole tool. Now
  `try/catch` per file — skip and continue.

**Sharing win.** The special-forms list was duplicated in `completion.rs` and
`semantic_tokens.rs` and had **already drifted** (`match*` in one, not the
other). Unified to one `pub(crate) const SPECIAL_FORMS`. (The broader LSP↔MCP
sharing is already in good shape: `brood::introspect` and
`scope::references_to_global` are the shared substrate; the per-encoding bits —
LSP `LineIndex` UTF-16 vs the `line_col` char columns the MCP/agent API uses —
are necessarily separate.)

**Rejected (false positives).** Adding `defmodule` to `collect_globals` — would
mint a phantom global for a name `defmodule` never binds (it expands to
`(do (def *module-docs* …) (provide mod))`, no `(def mod …)`). The
`path_to_uri` two-slash worry — `file://` + an absolute `/path` is the correct
three-slash form. Char-vs-UTF-16 column on *cross-file goto* — a known,
documented limitation (the cross-file *references* path uses `LineIndex` and is
UTF-16-correct).

**Tests.** New: `scope::references_exclude_quoted_symbols`,
`uri_tests::{uri_to_path_handles_empty_and_host_authorities,
path_to_uri_round_trips_through_uri_to_path}`. `brood` 125, `brood-lsp` 53,
`nest` 30 — all green; new code clippy-clean. Re-verified cross-file
references/rename (LSP) and `callers` (MCP) end-to-end against a fresh project.

## 2026-05-28 (cont.) — Demo-friendliness: stdlib + docs gaps from `claude-demo-findings.md`

Closing the tractable, non-race tail of [`claude-demo-findings.md`](claude-demo-findings.md).
First a status reconciliation against HEAD: three of that doc's four "blockers"
were **already fixed** (commit `5b19787` + the structured-error work) — the
type-checker no longer warns on `(require 'hatch)` macros, `nest format` keeps
multi-line code (only strips column-alignment padding), and pattern-destructure
mismatches now raise a clean `[:match-error …]` Brood error instead of a Rust
panic. Verified each by re-running, not by trusting the doc. What remained was
the stdlib/doc polish; the scheduler race and the perf/process-death items are
deferred (the race is under active investigation, and they perturb or collide
with that work).

**Added — stdlib (`std/prelude.blsp`, pure Brood).**
- `string-repeat`, `pad-left`, `pad-right` — column formatting for console
  output (`pad-*` never truncate). `round-to` — round to N decimal places,
  staying a number (built on the `floor` primitive; documented binary-float
  caveat). `bench` — a gensym-hygienic macro that times an expression, prints
  `label: N ms`, and returns its value. (`repeat` already existed.)

**Added — kernel primitives (`crates/lisp/src/builtins.rs`).** Two genuine
Rust-boundary cases the language can't bootstrap:
- `to-fixed` — `(to-fixed x n)` renders a number with exactly `n` decimals as a
  string (e.g. `"3.14"`, `"3.00"`). `str`/`pr-str` print the shortest
  round-tripping form (full f64 precision), which is wrong for tabular output.
  Uses Rust's float formatter; negative `n` raises `E0042`.
- `now-ns` — wall-clock nanoseconds since the epoch, the fine-grained partner to
  `now` for sub-millisecond timing.

**Docs.** Expanded [`brood-for-claude.md`](brood-for-claude.md): filled the
missing builtins (`apply`/`now`/`gensym`/`quot`/`mod`/`rem`/`char-at`/`for`/
`doseq`/`dotimes`/`dolist`/`enumerate` + the new helpers) and added a **`hatch`
framework** section (`defprocess`/`cast`/`call`/`!`/`gen-call`/`sleep`) with a
verified counter-server and worker-pool example — the idiomatic concurrency
story that was entirely absent. Kept [`language.md`](language.md) in sync
(Strings / Arithmetic / Time & memory sections).

**Tests.** New `deftest`s in `tests/strings_test.blsp` (padding/repetition,
`to-fixed`, plus an `:isolated` across-processes round-trip that sends padded
strings through 20 workers) and `tests/math_test.blsp` (`round-to` both signs,
`bench` return value, `now-ns` monotonicity). All green; `brood` Rust suite (73
in `basic.rs`) green.

**Process-death context (§3.4/§6.5).** `process N died: …` was opaque about
*which* process. Added `dist::name_for_pid` (the reverse of `whereis`, read
before `deregister` clears it) and a `scheduler::proc_descr(pid)` helper, and
routed all four diagnostics (`panicked` / `died` / `caught` / `exceeded restart
intensity`) through it — a named process now reports `process ticker (pid 1)
died: …`. The error is printed via `LispError::located()` so it carries
`FILE:LINE:COL:` + kind *when known*. **Caveat:** errors propagating out of a
spawned process currently carry no source position (the file runner only tags
the main thread's top-level forms), so the location half is correct-but-latent —
it'll light up for free once the propagation path attaches position (a natural
tie-in to the def-site work). Verified the name output end-to-end; the always-on
location piece would need spawn-site/enclosing-form tagging in `eval/mod.rs`,
left out to stay clear of the in-flight race work.

**Dropped — 2-arg numeric fast-paths (§4).** Investigated, decided against. The
goal was to skip the rest-list + `fold` overhead on `(+ a b)` (≈4 µs/call). It
turns out there's **no pure-prelude win**: a multi-clause `defn` lowers (via
`lower_fn`, `eval/macros.rs:413`) to `(fn (& args) (match* :fn args …))` — it
still binds *every* arg into a rest-list before dispatching, then adds `match*`
overhead, so it's strictly *worse*. `&optional a b` breaks semantics (optionals
default to `nil`, so `(+ 1 nil)` — today a type error — would read as `(+ 1)`).
The rest-list is allocated by the evaluator's `apply_closure` at the call
boundary, *before* any Brood runs, so no wrapper-level arity check can avoid it.
A genuine fix needs one of: (a) arity-based clause dispatch in `eval/mod.rs`
(skip arg-collection — the principled route, but it's the hot race-hunt file),
or (b) moving `+`/`-`/`*`/`/`/`<`/`=` to `Arity::any()` Rust builtins (fastest,
but reverses ADR-006 for arithmetic). Neither is worth it pre-profiling on a real
workload; revisit if arithmetic ever shows up hot in a benchmark that matters.
**Lesson for next time:** multi-clause `defn` is not a zero-alloc arity switch.

**Pre-existing failure noted (not from this work).** `introspection_test`'s
"a prelude global … has no recorded site" now fails — `(source-location 'map)`
returns a cached-prelude def-site `["…/.cache/brood/prelude.blsp" 185 1]` instead
of `nil`. That's the in-progress cross-file def-site / prelude-caching change in
`heap.rs`/`eval/mod.rs`, unrelated to the additive stdlib work here.


## 2026-05-29 — Maps: CHAMP trie (ADR-040)

**Goal.** Replace ADR-030's insertion-ordered association vector with a
CHAMP hash trie so `assoc`/`dissoc` stop being O(n) (and `(fold assoc {} …)`
stops being O(n²)), and `get` stops being a linear `equal` scan.

**Why CHAMP, not vanilla HAMT.** The ADR-040 rationale in one paragraph:
same big-O as Clojure's `PersistentHashMap`, but two bitmaps per node
(`data_map` for inline `(k,v)`, `node_map` for child sub-nodes) → smaller
nodes, better cache use, and **canonical** structure under structural
equality, so map `=` becomes a shape-matching recursion that bails on the
first mismatched bitmap instead of "iterate one map, look every key up
in the other."

**What landed.**
- `core/map_champ.rs` — `MapNode` (branch / collision leaf), `slot_at`
  (4-bit hash slice), `rank` (bitmap-popcount index), constants.
- `core/heap.rs` — `Slabs::maps` and `CodeSlabs::maps` switched from
  `Vec<Vec<(Value, Value)>>` to `Vec<MapNode>` / `boxcar::Vec<MapNode>`.
  Map ops became CHAMP recursions: `champ_get` / `champ_assoc` (split /
  overwrite / recurse / insert; `champ_split` for hash-collision spawn)
  / `champ_dissoc` (promotion when a sub-node shrinks to a singleton; drop
  when it empties). `map_equal` walks the canonical shape, fallback to
  set-equality on collision leaves. Promotion to RUNTIME: `promote_map_node`
  walks depth-first, allocating new RUNTIME slots bottom-up. Prelude freeze
  re-tags both inline entries' values and child `MapId`s.
- `Heap::hash_value` (salvaged from the abandoned ADR-030-index attempt) —
  structural, consistent with `Heap::equal`; canonical 0.0/-0.0/NaN; XOR-
  based for order-insensitive map hashes; region-blind.
- API rename: the slice-returning `heap.map(id) -> &[(Value, Value)]` is
  gone (entries are spread through the trie). Callers use
  `heap.map_entries(id) -> Vec<(Value, Value)>` (full walk),
  `heap.map_node(id) -> &MapNode` (raw node), `heap.map_get` (one key),
  or `heap.fold_entries` (borrow-friendly iteration without a Vec). Old
  `heap.alloc_map(pairs)` → `heap.map_from_pairs(pairs)` (folds `assoc`
  over a fresh empty root, building the trie in one O(N log N) pass).
- `map-pairs` and the `{ }` reader path go through the new APIs.

**ADR-030 contract change.** **Iteration order is no longer insertion
order.** It's deterministic per map shape (slot-index ascending at each
level), but hash-driven. Tests that asserted insertion order — and there
were nine — were rewritten to compare via `(frequencies (keys m))` (a map
→ order-independent `=`) or to reduce + assoc round-trips. The "any value
is a key" / "equality is order-independent" / falsy-value tests are
unchanged.

**Numbers (release, divan, 3-sample quickbench).**
| Bench | HEAD (assoc-vec) | CHAMP | Δ |
|---|---|---|---|
| `build_and_get` 200 | 4.4 ms | 5.2 ms | +18% |
| `build_and_get` 1000 | 31.0 ms | 20.7 ms | **−33%** |
| `frequencies` 1000 | 9.2 ms | 9.6 ms | +4% |
| `frequencies` 10000 | 113 ms | 117 ms | +4% |

The asymptotic win shows at N=1000 (≈35% faster) and grows: a 10 000-entry
map builds + iterates in ~137 ms end-to-end (was prohibitively slow on the
old O(N²) build). Small `frequencies` workloads (7 unique keys) shift
marginally — the per-op work per assoc is one slot probe + a one-`data`
shift, vs. a one-element `equal` scan before; cache effects dominate.

**Tests.** 64/64 in-language `tests/maps_test.blsp`; full Rust test
suite green (the one pre-existing `server_style_receive_loop` GC bug is
deferred and unrelated). Pre-existing parallel-session failures noted:
`suite_test.blsp` `error-of` assertions (parallel session changed it from
string-returning to map-returning); not part of this work.

---

## 2026-05-29 (cont.) — MCP DX feedback: the two trust-breakers

**Goal.** A Claude session reviewed `nest mcp` + the `writing-brood` skill
(notes in the chat; overall 8/10, "the live-image loop is the right
abstraction"). Two items actively made the loop *untrustworthy* — fix those
first, defer the polish items (`load` arg naming, scoped `format`, `def`
docstrings, a bind-vs-match lint).

**Fixed.**

- **`run-tests` double-counts after a reload.** A `describe`/`test` form
  *registers* by consing onto `*units*` in `std/test.blsp`. In a one-shot
  `nest test` the process starts with `*units* = nil`, so counts are right —
  but the MCP session is a long-lived image (ADR-013), and `load`ing the same
  test file twice registered every unit twice. The runner reported 6 tests for
  a 3-test suite, so an agent had to shell out to a fresh `nest test` for a
  trustworthy count. Fix: a `reset-units!` (`std/test.blsp`) that clears the
  registry, called by both `run-project-tests` and
  `run-project-tests-structured` (`std/project.blsp`) right before they
  (re)load the test files — so each run owns a clean registry no matter how
  many times the image loaded them before. Inert on a fresh `Interp`
  (`*units*` is already `nil`), so the one-shot path is unchanged.

- **`print` corrupts the JSON-RPC channel.** `nest mcp` speaks newline-
  delimited JSON over stdout; a handler's `(print …)` wrote straight there and
  broke the protocol stream — and printing is the most natural debugging
  instinct, so "don't print" was a real footgun (the skill had to warn against
  it). Fix: a thread-local capture buffer in `crates/lisp/src/builtins.rs`
  (`begin_stdout_capture` / `take_captured_stdout`, both `pub`); `print`
  diverts into it when one is installed, else takes the identical
  `print!`-to-stdout path as before (REPL / file runner unaffected). The
  dispatcher (`crates/nest/src/mcp.rs`) installs a buffer around every
  `tools/call` (and `tools/list`, in case a project `mcp.blsp` prints at load)
  and drains it afterward — always, even on error, so it can't leak into the
  next call. Captured output rides back as a **second** MCP content block
  (`content[1]`, labelled `[captured stdout]`); `content[0]` stays the
  handler's return value, so existing parsers are unaffected. Thread-local, so
  it captures the synchronous handler thread only — spawned green processes on
  other workers are unaffected (they shouldn't be writing to a protocol
  channel anyway). This realizes the `stdout` column the `docs/mcp.md` tool
  table already anticipated, delivered uniformly for *every* tool rather than
  threaded through each handler's return map.

**Tests.** `crates/lisp/tests/basic.rs`:
`reset_units_prevents_reload_double_count` (register twice → 2, `reset-units!`
+ once → 1). `crates/nest/src/mcp.rs`:
`handler_print_is_captured_not_leaked_onto_the_channel` (a clean newline-
delimited round-trip past a printing handler is itself proof the channel
stayed pure JSON) and `capture_does_not_leak_between_calls`. All 32 nest tests
and 128 brood lib unit tests green.

**Caveat — full in-language suite.** `tests/suite.rs` currently SIGSEGVs in
the concurrent scheduler path (the in-progress supervision/resume-slot
rework — `process/scheduler.rs`), surfacing right after the `bench` +
concurrency groups. Confirmed unrelated to this work: `reset-units!` is a
no-op on the suite's fresh `Interp`; the `print` change is the identical code
path when no capture is installed; and the evaluator's `map_entries` change
(the only one-line eval diff in the tree) returns an owned `Vec`, GC-safe.
Targeted verification done; whole-suite re-run pending the scheduler fix.

**Docs.** This entry; the `docs/mcp.md` "Session model" section now documents
stdout capture + the `content[1]` envelope.

---

## 2026-05-29 — Test runner fails fast on a dead worker (KI-2 part 2)

**Goal.** From agent DX feedback (three editing sessions, `docs/` review): the
single highest-impact fix was that `nest test` *hangs forever* when a parallel
test worker dies, instead of reporting the failure. A hung runner is the worst
signal for both a human and an autonomous agent — worse than a red test.

**Built.** `std/test.blsp`: the parallel phase now reaps dead workers.
- `spawn-units` `monitor`s every worker it spawns and returns a `(pid unit)`
  assoc list; each worker tags its result message with its own `(self)` pid.
- `collect-units` → `collect-loop` accounts for each worker exactly once: by its
  `[:unit-result pid results]` if it reported, otherwise by the
  `[:down mref pid reason]` its monitor fires. A dead worker becomes a failing
  result (`"test process died: <reason>"`) instead of an indefinite `(receive)`
  block. A `[:down …]` for a pid that isn't ours (a stale worker from a prior
  run in a long-lived session) is ignored without decrementing the count, so it
  can't corrupt a later run. The kernel fires `[:down …]` immediately if the
  worker already exited, so there's no lost-death window between `spawn` and
  `monitor`.

**Why this is independent of KI-1.** The scheduler lookup race can still *kill*
a worker; this change only ensures the runner *notices* and fails fast with the
death reason. KI-1 (workers can't resolve globals under `-j 0`) remains open.

**Tests.** `tests/runner_failfast_test.blsp` reproduces a worker death
deterministically (a unit whose thunk throws inside `run-unit`, before the
worker sends) and asserts the collector returns a failing result rather than
hanging. Verified in isolation (`--test`, exit 0). Full-suite re-run pending —
the tree has concurrent core changes in flight.

**Docs.** `docs/known-issues.md` KI-2 part 2 marked fixed; this entry.

---

## 2026-05-29 — Macro-hygiene lint (check-time capture warning)

**Goal.** From the agent-DX feedback: macros are unhygienic by default, and a
template binder introduced with a literal symbol silently captures caller code
(`time`'s `start` shadowing the body's `start`). `gensym` is the fix, but
nothing warned you — the macro miscompiled quietly. Add a `check`-time lint.

**Built.** `crates/lisp/src/types/check/hygiene.rs`, wired into `check_file` as a
pass over the **un-expanded** forms (defmacro templates vanish after
macroexpansion). Warns only when both hold for a `let`/`fn` binder inside a
quasiquote template:
1. the binder is a *literal* symbol (a gensym'd binder is `(unquote g)`; an
   unquoted caller-name is `(unquote evar)` — neither is a literal symbol, so
   neither trips the lint);
2. a macro *parameter* is spliced (`~p`/`~@p`) into that binder's scope. Brood
   `let` is sequential, so per-binder scope = the body + *later* bindings' value
   expressions (not the binder's own value) — which is why `time`'s `start` is
   flagged but its `v` (bound after `~expr`) is not.

**Why this scope, and the no-false-positive bar.** ADR-024 makes the checker
advisory and forbids flagging runnable code. The tight two-condition gate is
what keeps it sound: audited across the entire `std/` tree (`brood --check` on
every file), it produces **zero** hygiene warnings — every existing macro
(`and`/`or`/`bench`/`is`/`assert=`/`match*`/`receive`/`defprocess`/…) already
gensyms or unquotes its binders. The only shape it would flag that could be
intentional is an anaphoric macro (deliberate capture); none exist in-tree.

**Not done (deliberately).** The sibling feedback ask — a lint for the
bind-vs-match `~x` trap (a bare pattern symbol shadowing a global) — was *not*
added: the dangerous variant (a live clause after an irrefutable bare-symbol
pattern) is already a compile error (`std/prelude.blsp` match compiler rejects
unreachable clauses), and a broad version would fire on the core mechanism
itself (binding `first`/`rest`/any global-named pattern var is idiomatic), which
no sound heuristic can avoid. Recorded here so it isn't re-attempted.

**Tests.** 6 unit tests in `hygiene.rs` (capturing let-binder, capturing fn
param, gensym binder not flagged, unquoted caller-binder not flagged, splice
outside scope not flagged, non-macro forms ignored); end-to-end verified via
`brood --check` (emits `file:line:col: warning: …` with the gensym hint). All 78
`types::` tests green.

**Docs.** `docs/types.md` Step-4 surface gained a macro-hygiene bullet; this
entry.

---

## 2026-05-29 — `(format …)` printf-style helper (demo-DX item #5)

**Goal.** Item #5 on the `claude-demo-findings.md` §10 wishlist. `to-fixed`
already fixed the underlying ugly-float case (`(str 0.015873015873015872)` is
still f64-precision; `(to-fixed … N)` clips it), but demo writers naturally
reach for `(format "%.2f" x)` and end up hand-rolling `str` + `to-fixed`
chains. A small `format` closes that ergonomic gap.

**Built.** `std/prelude.blsp` gains `format` (+ private `format--loop` /
`format--spec` / `format--prec`), implemented in Brood over `char-at` /
`string-length` / `string->number` / `to-fixed` / `str` — no new Rust. The
specifier set is a deliberate subset:

- `%s` — any value, via `str`
- `%d` — number, via `str` (the type letter is a hint for the reader; no
  conversion happens — float in, float out, same as `%s`)
- `%f` — float with 6 fractional digits (the C/Java default)
- `%.Nf` — float with N fractional digits, rounded via `to-fixed`
- `%%` — literal `%`

Width/justification is *not* in the specifier (compose with `pad-left` /
`pad-right` — already in the prelude). Hex / octal / `+ -` flags / explicit
sign aren't there either; the bar is "what a demo actually reaches for", not
"feature-parity with C's `printf`". Errors on an unknown specifier or one that
ends mid-spec (`"%"`, `"%.2"`, `"%.xf"`); a missing arg renders as `nil`
(debuggable), extra args are ignored.

**Why a function not a special form.** It's a pure data transformation —
specifiers are scanned at runtime, not at compile time — so there's no
substrate need for the compile pass to know about it. Keeping it as a regular
`defn` in the prelude keeps the core small (CLAUDE.md, ADR-011) and lets
`format`'s parsing be inspected / extended in Brood.

**Why the namespace is fine.** `(require 'format)` loads the source-code
formatter (`std/format.blsp` — backs `nest format`), which exports
`format-source` / `format--root--walk` / etc. — *not* a bare `format`. The new
prelude `format` is unambiguous.

**Tests.** New `describe "format (printf-style)"` block in
`tests/strings_test.blsp` (9 tests): no-specifier identity, `%%`, `%s` across
value kinds, `%d`, `%f` default precision, `%.Nf` (incl. `%.0f`), mixed
specifiers, extra/missing args, and the four error shapes (`%q`, lone `%`,
truncated `%.2`, non-digit after `.`). Full strings-suite goes from 44 to 53
tests, all green; full `nest test -j 1` is 512 tests, 503 passing (the 9
failures are pre-existing structured-error-format assertions, unrelated).

**Docs.** `docs/language.md` (Strings section) and `docs/brood-for-claude.md`
(string-formatting bullet) both gain a `format` line with the specifier set
and a worked example; this entry.

---

## 2026-05-29 — Kernel supervisor stripped (ADR-039 reverted)

**Goal.** The kernel-level supervisor that shipped 2026-05-28 (RESUME_SLOT
thread-local + safepoint rooting + `supervise()` retry loop +
`%spawn-supervised*` primitives + `(supervise …)` macro + mode gate) was the
dominant contributor to the multi-thread scheduler race (KI-1). The race
blocked **every** fan-out program, while supervision was load-bearing for
only the hot-reload-on-retry story. Trade made: keep the fan-out fix, let
supervision move to userland.

**Built.** Commit `e3d3a0d`. What's gone:

- `crates/lisp/src/process/scheduler.rs` — `RESUME_SLOT` thread-local + the
  `ResumeSlot` type + `record_resume` / `take_resume` /
  `resume_slot_save/set` / `for_each_resume_root`; `SUPERVISION` +
  `SupervisionPolicy` + `is_supervised` + `supervision_save/set`; the
  `supervise()` retry loop + `run_call` helper; `spawn`'s `policy:
  Option<SupervisionPolicy>` parameter (spawn is now always let-it-crash).
- `crates/lisp/src/eval/mod.rs` — the `Value::Fn` `record_resume` guard +
  the safepoint's `for_each_resume_root` rooting.
- `crates/lisp/src/process/mailbox.rs` — `wait_for_message`'s resume-slot /
  supervision save+restore around suspend.
- `crates/lisp/src/builtins.rs` — `%spawn-supervised` /
  `%spawn-supervised-named` + their docstrings + the policy-from-args helper.
- `std/prelude.blsp` — `(supervise …)` macro, `*supervise-max-restarts*`,
  `*supervise-max-window-ms*`. The `(spawn …)` docstring no longer mentions
  supervision.
- `crates/nest/src/main.rs` — `nest run --watch` now wraps in plain
  `(%spawn)` instead of `%spawn-supervised`. A throw in the watched program
  kills the session; editing the file re-spawns from scratch (also a cleaner
  model — no surprising state retention across edits).
- `examples/live-script/` — removed (an example of the removed feature).
- `crates/lisp/tests/basic.rs` — `supervisor_retries_last_iteration_with_same_args`
  and `supervisor_picks_up_hot_reloaded_definition_on_retry` removed.

**What's retained.** The Erlang-style **building blocks** that supervision
was built over: `(spawn)`, `(monitor)`, `(demonitor)`, `(send)`, `(receive)`.
A user wanting recover-on-throw writes a supervisor process in Brood that
monitors a child and re-spawns on `[:down …]` — same shape Erlang OTP
supervisors are built from, in ~10 lines.

**What's lost.** The kernel's automatic mid-iteration retry with state
preservation, and hot-reload-on-supervisor-retry (a freshly-`def`'d fix
taking effect on the next supervised retry). Plain hot reload — next *call*
sees the new binding (ADR-013) — is independent and unaffected.

**Why this works after the trade.** The race wasn't worth keeping the
elegance: Brood's immutability + cheap process spawn + monitor make a
hand-written supervisor cost ~10 lines, not the chapter Erlang/OTP devotes
to gen_server + supervisor. The feature *can* come back later (the design
is preserved in git history at this commit and as the body of
`docs/supervision.md` for one revision before this entry), but only with
substrate that doesn't reintroduce the race.

**Effect on the race (single change, before Phase-1 allocator).** The
`recurse.blsp` repro went from ~24 worker deaths per run (0/n clean) to
~0–1 per run (**5/10 clean**). The Phase-1 bump-only allocator landing on
top of this (see the next entry) closed the remainder in debug-assertions
release.

**Tests.** `cargo test -p brood --lib`: 125/125. `cargo test -p brood --test
basic`: 72/72. `cargo test -p brood --test gc`: 2/3 — the failing
`server_style_receive_loop_stays_bounded` was *catching a real pre-existing
GC root-coverage bug* via the poison tripwire, not a regression from this
commit. That test became Phase 1's witness (it passes after Phase 1).

**Docs.** ADR-039 marked reverted in [`decisions.md`](decisions.md);
[`supervision.md`](supervision.md) replaced with a short revert note plus
the userland respawn pattern; [`README.md`](README.md), [`roadmap.md`](roadmap.md),
and [`known-issues.md`](known-issues.md) updated; this entry.

---

## 2026-05-29 — Phase-1 bump-only allocator (race goes silent)

**Goal.** Close the remaining ~5% race tail after the supervisor strip. The
manual-rooting discipline around slot reuse was the substrate for the
`unbound symbol` / `index out of bounds` panics: a freed slot could be
reallocated to a fresh value while another thread still held a stale handle
that re-deref'd it. Removing slot reuse removes the class.

**Built.** Commit `f90f0de`. Heap allocations now **grow monotonically** per
process — `alloc_slot!` (and the hand-written `new_env` / `alloc_string`)
drop their free-list reuse paths. Every alloc bumps the slab; nothing is
ever recycled. `Heap::collect` becomes a no-op, kept as `collect_old`
(`#[allow(dead_code)]`) for reference until the Phase-2 cleanup removes it.
Net effect: stale handles can't observe a value of the wrong type, because
no slot is ever a different type than when it was first allocated.

**Two-phase plan.** This is phase 1 of a two-phase switchover:

- **Phase 1 (this commit).** Bump-only allocation; no sweep. Bounds memory
  per *short-lived* process (it exits, the per-process heap is dropped
  whole), but grows unboundedly for long-running tail-recursive computation
  that never goes through `receive`. The `gc.rs` `long_tail_loop_stays_bounded`
  test is marked `#[ignore]` with a Phase-2 note.
- **Phase 2 (next).** Arena flip on `receive` — deep-copy the surviving
  state to a fresh slab and drop the old. Bounds memory in long-lived
  receive loops (gen_server / editor event loop / hatch). Independent of
  Phase 1's race-removal property.

**Effect on the race.** `recurse.blsp` in **debug-assertions release**:
**10/10 clean** over multiple runs vs ~95% failure before. The
`server_style_receive_loop_stays_bounded` test that the supervisor-strip
commit had failing — the poison tripwire catching a real GC-root-coverage
bug in the receive-loop pattern — now passes (the bug is gone with slot
reuse).

**Known issue (separate bisect needed).** In **plain release** (no
`debug-assertions`), the multi-threaded scheduler can still segfault on the
same shape (tail-recursive workers with heavy prelude churn). The poison
tripwire suppresses it in debug-assertions release but isn't compiled in
for plain release. Likely cause per the commit message: the bundled WIP in
the +698-line heap.rs rewrite alongside Phase 1, not Phase 1 itself — a
separate task. `-j 1` is reliable on plain release.

**Bundled WIP (not part of Phase 1 proper).** `crates/lisp/src/core/heap.rs`
substantial rewrite alongside the map_champ integration; map_champ +
map_entries threaded through eval / macros / message etc.; error.rs /
printer.rs / reader.rs adjustments; tests + docs.

**Docs.** [`known-issues.md`](known-issues.md) KI-1 marked largely fixed
with the plain-release caveat preserved; this entry.

---

## 2026-05-29 (afternoon) — Race fully closed; suite-test segfault bisected

**Goal.** Two follow-ups from the Phase-1 morning: (a) close the
plain-release segfault that survived the bump allocator, and (b) bisect the
`cargo test -p brood --test suite` segfault that wasn't reproducing through
`./target/release/brood` directly.

**Built.**

- **`2abf05e` — per-worker pinned queues.** Replaced the shared
  `RUN: Mutex<VecDeque>` queue with one queue per worker, plus round-robin
  assignment at spawn. Each `Process` carries its `worker_id`; `enqueue`
  routes by that field; preempt and receive re-park onto the same worker.
  No work stealing. The plain-release segfault was a coroutine being
  migrated to a different worker thread mid-call (corosensei resumes on
  whichever thread `resume()` runs on) — pinning the process kills that
  hazard. `recurse.blsp` and `medium.blsp`: 10/10 clean in plain release,
  single- and multi-threaded.

- **`CORO_STACK_BYTES` 1 MiB → 2 MiB.** The `cargo test -p brood --test
  suite` segfault was a coroutine **stack overflow**, not a memory bug.
  gdb showed RSP just below a 1 MiB stack range, deep eval recursion at
  ~hundreds of frames. Debug eval frames are bigger (no inlining), and
  post-Phase-1 poison checks widened them further. Bumped the per-coroutine
  stack ceiling to 2 MiB — pages are mmap'd lazily, so unused tail pages
  stay uncommitted; the higher ceiling costs ~0 until depth needs it.

- **Stale test assertions fixed.** Nine in-language suite tests pinned old
  error-message strings and old formatter behaviour:
  - `error-of` (in `std/test.blsp`) now coerces a structured-error map
    (`{:kind :code :message :hint}`) back to the legacy `"kind error:
    message"` string the suite pins. `(throw v)` still passes `v` through
    unchanged. A throw `(throw :boom)` test in `suite_test.blsp` updated to
    use `map?` for the catchability check.
  - `format_test.blsp`'s "short forms collapse" describe replaced — the
    formatter now respects author newlines (`5b19787`), so the tests pin
    *preservation* of multi-line input, not its collapse. The defn header
    `(defn name params)` is still a single line by rule even when the
    input has the name and params on separate lines.
  - `vector-ref` error message pinned to its new richer form (`index 9
    out of range [0, 2)`).

**Effect on tests.** Full `cargo test --workspace` is green again:
- `cargo test -p brood --test suite` (debug): 1/1 ok in 35s (was: SIGSEGV).
- `cargo test -p brood --test suite --release`: 1/1 ok in 2.5s.
- In-language suite: 514 tests passing, 0 failing.

**Phase 2 status.** Initially paused with a safety concern — auto-flush
from inside `%receive` invalidates the caller's `env` register — then
resumed later in the day with a safer design (next entry).

**Docs.** This entry; [`known-issues.md`](known-issues.md) KI-1 marked
fully fixed; the plain-release and suite-test segfaults moved to
"resolved" entries in the minor list.

---

## 2026-05-29 (evening) — Phase 2: explicit `(hibernate)` primitive

**Goal.** Bound LOCAL-heap growth in long-running processes (server-style
receive loops, the editor event loop). The morning's bump-only allocator
killed the GC-race bug class but left long loops growing unboundedly —
without an arena flush point inside a tail-recursive loop, the bump grows
linearly with iteration count.

**Considered, rejected.** An *automatic* flush at `%receive` (deep-copy
the matched thunk into a fresh slab before returning). Safe for the
canonical `((%receive M ms ot))` macro pattern (the eval loop's `env`
register is discarded at the tail-apply that follows). **Unsafe** for any
other use site (`(let (x (%receive …)) …)` etc.) — the *caller's* eval
frame still has a LOCAL `env` register that would dangle after the flush.
The eval loop can't reason about which Rust frames are above it on the
stack, so no in-place flush is generically safe.

**Built.** `(hibernate fn & args)` — Erlang-style hibernate, opt-in.

- **Raises an uncatchable `LispError::Hibernate` sentinel.** The error
  propagates through every intervening eval frame (Rust `?` unwind), so
  every Rust-stack reference into LOCAL is discarded by the time we land
  in the process's run loop. `try`/`catch` filters the kind and re-raises
  — user code can't swallow the unwind.
- **Process run loop catches it.** `spawn`'s coroutine body wraps
  `eval::apply` in a `loop { match … }`: on `Ok` exit, on `Err(Hibernate)`
  it pulls the callee + args off the error, calls `heap.flush(&mut roots)`
  (deep-copies just those into a fresh `Slabs` and drops the old),
  re-applies. Any other `Err` exits the process normally.
- **`Heap::flush(&mut [Value])`** — the deep-copy mechanism. Uses
  per-slab forwarding tables (`old_idx → new_idx`) so a `letrec`-style
  env-↔-closure cycle terminates: placeholder slot allocated before
  recursing into children. Copies the named roots, the heap's own
  `dynamics` stack, and its extra `roots` stack; clears `form_pos` (the
  reader-time metadata is meaningless once LOCAL pair indices reset).
  PRELUDE/RUNTIME handles pass through unchanged (already shared).
- **Boxed hibernate args on `LispError`.** A `Vec<Value>` field on
  `LispError` grew the error's stack footprint enough that the
  deep-recursion parser test (`(((…` × 5000 → "form nested too deeply")
  tipped past the 2 MiB test-thread stack. Boxing the (almost-always-None)
  hibernate args keeps `LispError` small for the common path.

**Effect.**

- The `server_style_receive_loop_stays_bounded` and
  `long_tail_loop_stays_bounded` `gc.rs` tests pass green (the second
  was `#[ignore]`d after Phase 1 — un-ignored and rewritten to use
  hibernate).
- Microbench: a 5 000 000-iteration loop that conses + hibernates each
  iteration completes in 25 s wall, **RSS bounded at 4.4 MB**. The same
  loop without hibernate hits **1.4 GB** at 500 000 iterations — three
  orders of magnitude more memory at one tenth the work. Hibernate
  trades ≈5× iteration time for a hard memory bound.

**Constraint (documented, not enforced).** `hibernate` must be called in
**tail position** of a function body whose call chain is itself
tail-recursive. Calling from inside a `let` RHS or argument position
leaves the caller's let-scope dangling — the unwind discards every Rust
eval frame, not just the current one. The `(loop next-state)` ⇒
`(hibernate loop next-state)` rewrite is always safe; non-tail uses are
the user's responsibility.

**Docs.** This entry; [`memory-model.md`](memory-model.md) needs the
hibernate contract written up (follow-up).

---

## 2026-05-29 — Stdlib ergonomics (Game-of-Life feedback pass)

**Trigger.** A "build something non-trivial" report on writing Conway's Game
of Life in Brood surfaced a handful of friction points where the obvious
spelling didn't work: `(map f a-map)` threw "expected list or vector",
`(sort [[1 0] [2 1]])` threw "expected number", `(index-of (list 1 2 3) 2)`
threw a substring-search type error, and `\x1b`/`\u{1b}` weren't escape
sequences (only the named `\e` produced ESC). Conservative fixes — no new
core forms, no Value kinds, no laziness machinery. The bigger asks (a set
type + `#{…}` literal; a real `iterate` + laziness; MCP worker-panic
sandboxing; module-redefinition warnings; `nest format --changed`) need
their own ADRs and were deferred.

**Built.**

1. **Maps are seqable.** Added `seq` (universal list-view; map → entries
   via `map-pairs`, everything else pass-through) and `entries` (alias of
   `map-pairs`) to the prelude. `fold` now coerces once at entry via
   `seq` and dispatches to a `fold--loop` for the recursive case — so a
   map costs one extra `map-pairs` pass, not O(n) per step. `reduce`
   coerces in the 2-arg form (its bare `(first x)`/`(rest x)` bypassed
   fold). Result: `(map f m)`, `(filter f m)`, `(mapcat f m)`,
   `(fold f acc m)`, `(reduce f acc m)`, `(count m)`, `(into [] m)` all
   walk a map as its `[k v]` pairs without the `(zip (keys m) (vals m))`
   workaround. The type checker's curated `seq` lattice for
   `map`/`filter`/`reduce` widened to include `Map` so the checker no
   longer warns on the new shape.
2. **`into [] coll` now produces a vector.** Previous behaviour: `(into []
   (list 1 2 3))` silently returned `(1 2 3)` (a list) because the
   underlying `append` is fold-of-flip-cons. Fixed by re-vectorising in
   the vector-target branch — `(apply vector (append to from))`.
3. **Sort accepts any value, no comparator needed.** Added a `value_cmp`
   on `Heap` — a total structural ordering (numbers by `<`; strings/
   symbols/keywords by text; vectors lexicographic; lists by spine;
   different kinds by a fixed `tag_rank`). Exposed as a `%sort-cmp`
   primitive. Brood `sort` dispatches: empty → `nil`; first item numeric
   → fast `%sort-asc`; otherwise → `%sort-cmp`. So `(sort [[1 0] [2 1]])`
   and `(sort (list "c" "a" "b"))` and `(sort (list :c :a :b))` all Just
   Work. `sort-by` and custom-comparator `(sort less? coll)` unchanged.
4. **`index-of` is polymorphic; added `includes?`.** `index-of` now
   dispatches on the collection: string → substring search (existing),
   list/vector → linear scan returning index or -1. `includes?` is the
   uniform predicate across lists/vectors/strings/maps (looks at values
   in a map; `contains?` is still the key check).
5. **String escapes: `\xHH` and `\u{H..H}`.** Scanner consumes two hex
   digits after `\x` (single byte/char) or a `{H..H}` block after `\u`
   (1–6 hex digits → Unicode codepoint, surrogates rejected). Malformed
   sequences fall through as literal `x`/`u` (matching the existing
   "unknown escape = literal char" rule, so it's backwards-compatible —
   not a new parse-error class).
6. **Hatch: `query` clause + `(stop pid)` graceful shutdown.** The
   gen-server framework grew a third clause kind: `(query PATTERN body…)`
   is like `call` but the body is *just the reply* — state passes
   through unchanged. Removes the `[x s]` boilerplate from "just read a
   field" cases. Every hatch process also now handles an implicit
   `[:$stop]` envelope (the `defprocess` macro appends a stop clause
   that exits the loop), so `(stop pid)` ends the receive loop cleanly
   without each user having to declare it.
7. **Docs updated.** `docs/brood-for-claude.md` and the `writing-brood`
   skill mention maps-are-seqable, structural `sort`, polymorphic
   `index-of` / `includes?`, the new string escapes, and the `query`/
   `stop` hatch additions.

**Effect on tests.** `cargo test --workspace` green; `nest test` green;
no in-language regressions. New tests in `tests/sequence_test.blsp`,
`tests/strings_test.blsp`, `tests/hatch_test.blsp`, and Rust unit tests
in `crates/lisp/src/syntax/scanner.rs` pin the new behaviour.

**Deferred.** Five items captured in detail in [`deferred.md`](deferred.md) —
rationale, design sketch, trigger, and the workaround available today:
1. First-class set type + `#{…}` literal.
2. Real laziness + `iterate`.
3. MCP worker-panic isolation (started immediately after — see the next entry).
4. Cross-module redefinition warning.
5. `nest format --changed`.

---

## 2026-05-29 (later) — MCP worker-panic isolation

**Goal.** A Rust panic anywhere in a tool-call code path must not kill the
`nest mcp` JSON-RPC server. Before this change, a single `panic!` (any
`unwrap`-on-`None`, any out-of-bounds, any kernel `unimplemented!`) inside
a handler unwound through `main_loop` and dropped every `mcp__brood__*`
tool for the rest of the session — the Game-of-Life report's "Connection
closed; all tools dropped" symptom. The race that was *triggering* the
panics is fixed (KI-1/KI-2 scheduler race, earlier today), but the same
shape of failure would resurface for any future bug in Brood-callable Rust
— so the host-side isolation is its own concern.

**Built.**

1. **`call_tool` wraps its body in `panic::catch_unwind`.** The inner
   closure (`(require 'mcp)` + `(mcp-tools)` + `find_handler` + `apply` +
   `value_to_json`) runs inside
   `panic::catch_unwind(AssertUnwindSafe(|| …))`. `AssertUnwindSafe` is
   sound because the MCP server is single-threaded (synchronous `main_loop`
   over stdio); the heap reset that already ran on the no-panic path
   (`truncate_roots` + `reset_local_to`) is moved *outside* the catch so
   it runs on **every** termination — early-return error, normal success,
   or caught panic. That gives the caught-panic path the same recovery
   the error path has: every LOCAL allocation since the per-call
   checkpoint is discarded, so subsequent calls start from the same heap
   shape the failing one did.
2. **`RpcError::from_panic` projects the unwind payload.** The
   `Box<dyn Any + Send>` payload is downcast as `&'static str` (from
   `panic!("literal")`) or `String` (from `panic!("{}", x)`); anything
   else falls back to a generic "no message" string so the caller still
   sees that *something* panicked. The result is a JSON-RPC `Internal`
   error (`code: -32603`) whose `error.data` carries `kind: "panic"`, the
   original `message`, and a `hint` calling it an interpreter bug.
   Parallel shape to `from_lisp` so an agent's `error.data.kind` branch
   covers both cleanly.
3. **Debug-only `%force-panic` primitive** (`#[cfg(debug_assertions)]` in
   `builtins.rs`). One-line `panic!()`-from-a-primitive — gives the
   regression test a reliable trigger, doesn't bloat the release surface.
4. **Regression test
   `handler_panic_is_caught_and_server_keeps_serving`** in
   `crates/nest/src/mcp.rs`. Builds an inline `(mcp-tools)` catalogue
   with one panicking handler (`%force-panic`) and one plain `echo`,
   round-trips three messages through `main_loop`, and asserts (a) the
   panicking call returns a structured error with `code: -32603`,
   `message` containing "panic in tool handler", `data.kind: "panic"`,
   and the panic's own message round-tripped on `data.message`; and (b)
   the `echo` call **succeeds** — proves the server didn't die and
   `Interp` is in a usable state. Silences the default Rust panic hook
   for the test's duration so cargo's test output stays clean (stderr in
   production keeps the panic message + backtrace, which is on a separate
   stream from the stdio JSON-RPC channel).

**What's NOT covered.** Worker-thread panics — a green process on a
scheduler pool thread that panics — are out of scope here; the existing
scheduler is expected to keep workers alive past one process's panic. If
a real worker-thread panic surfaces, the same `catch_unwind` shape applies
around the per-coroutine `run` loop. A `SIGSEGV` (the demo's earlier
symptom, before the race fix) is also out of scope: `catch_unwind` catches
Rust `panic!`, not segfaults — and the race that triggered the segfaults
is fixed in the scheduler.

**Effect on tests.** `cargo test --workspace` green (33 MCP tests; 529
in-language). The new test passes; no regressions elsewhere. The `gc`
test crate hit its 60s timeout once under full-parallel workspace load
(`--test-threads=$(nproc)`) but passes cleanly in isolation and under
`--test-threads=2`; pre-existing slow tests, not a regression from this
change.

**Docs.** [`deferred.md`](deferred.md) #3 promoted to "shipped" with
as-built design notes; [`roadmap.md`](roadmap.md)'s deferred-ergonomic
entry flipped from ⬜ to ✅.

## 2026-05-29 (late) — Shared blob heap (ADR-041): zero-copy send of large strings

**Goal.** Kill the byte-copy cost on the cross-process send path for
large strings. With Phase 1's bump-only LOCAL allocator and Phase 2's
`(hibernate)` arena flip, the next throughput cliff was `to_message`
deep-copying every `Value::Str` — a 10 KB error/log message sent from
one worker to another paid 10 KB of memcpy *on send* and another 10 KB
*on receive*. ADR-033's closure-as-data already proved that *handles*
can ride between processes without copy; this is the analogous story
for bulk byte data.

**Shipped (two commits — infrastructure, then the send-path flip).**

*Phase 1a (`94cfeb7`).* Infrastructure-only, zero behaviour delta.
- New `core/blob.rs`: `SharedBlob { bytes: Box<[u8]> }`, `BlobHeap`
  (per-runtime registry, mostly empty in Phase 1 — debug-only counter
  reserved for stats / interning), `SHARED_BLOB_THRESHOLD = 256`.
- `LocalString::{ Inline(String), Shared(Arc<SharedBlob>) }` private
  enum replaces `Vec<String>` in the LOCAL `Slabs::strings`. The
  PRELUDE region uses the same `Slabs` shape but `freeze_as_shared_code`
  inline-extracts any `Shared` entries on freeze, so PRELUDE stays a
  `Vec<Inline(_)>` and the cross-runtime `Arc<SharedCode>` never holds a
  runtime-scoped `Arc<SharedBlob>`. (~9 prelude docstrings exceed the
  threshold at write time — they get inlined back to `String`.)
- `Heap::alloc_string` is the **single chokepoint**: routes by length
  into `Inline` or `Shared`. Every `String → Value::Str` path goes
  through it; we audited the tree (`grep alloc_string`) to confirm 27
  call sites, no bypasses.
- `Heap::string()` rewritten off the `region_ref!` macro to match the
  variant. PRELUDE/RUNTIME branches unchanged. `flush_string` clones by
  variant (`Arc::clone` for surviving `Shared`, byte-clone for `Inline`),
  so a hibernate flush preserves blob identity (survivors' `Arc` count
  goes +1 from the new slab and −1 from the old slab's drop — net
  unchanged).
- `cargo test` 317/317, `nest test` 529/529, zero `.blsp`-visible delta.

*Phase 1b (this commit).* Flips the send path.
- `Message::StrShared(Arc<SharedBlob>)` variant joins `Message::Str`.
  `to_message_rec` calls a new `Heap::local_shared_blob(id)` first — a
  LOCAL `Shared` answers `Some(Arc::clone)`; everything else falls
  through to the byte-copying `Message::Str` path (`Inline`,
  PRELUDE/RUNTIME).
- `from_message` for `Message::StrShared` calls
  `Heap::alloc_string_from_shared(Arc::clone(blob))` — installs the
  cloned `Arc` directly into the receiver's slab, no `from_utf8` or
  byte traffic on the receive path.
- `dist/wire.rs` encodes `StrShared` as `M_STR` (inline bytes) for
  cross-node sends: a separate runtime has its own `Arc<BlobHeap>` and
  the `Arc` is not safely portable across the network. The receiving
  runtime's `from_message → alloc_string` re-routes through the
  threshold (anything ≥ 256 B becomes `Shared` again, locally).
- Debug-only `(%blob-ptr s)` / `(%blob-strong-count s)` primitives
  (cfg(debug_assertions), parallel to `%force-panic`) expose
  `Arc::as_ptr` and `Arc::strong_count` for tests. They read the slot
  without cloning, so reading the count does not perturb the value
  being checked. Heap accessors honour the poison bitmap.
- `LocalString::as_str` flipped to `from_utf8_unchecked` in release
  (debug keeps the validating path as a tripwire). UTF-8 invariant
  holds by construction: every entry to `SharedBlob` is via
  `&str.as_bytes()` (one chokepoint, `alloc_string`) or via the wire
  decoder's pre-validated buffer.

**Tests.** `tests/blob_share_test.blsp`:
1. ≥ 256 B string has a non-nil blob ptr; < 256 B answers nil.
2. A big string `send`-ed to a worker keeps the same `SharedBlob`
   identity in the worker (assert via `%blob-ptr` round-trip).
3. **8-worker fan-out**: spawn 8 workers, send the same big string to
   every one, assert all 8 replies report the parent's ptr — the same
   `Arc<SharedBlob>` survives through `to_message` × 8 +
   `from_message` × 8 + 8 worker reads. (`:isolated` describe — matches
   the `tests/maps_test.blsp` cross-process pattern.)
4. `def`'d (RUNTIME) strings answer nil from `%blob-ptr` — RUNTIME is
   shared by handle retag, not blob heap, as designed.
5. Strong count is 1 for a freshly-alloc'd big string.
6. **Hibernate flush preserves blob identity**: a worker receives a big
   string, reports `%blob-ptr` before, hibernates with the string as an
   arg, re-reports after; parent asserts before == after. Proves
   `flush_string`'s Arc::clone-on-survive arithmetic.

The whole suite is green: `cargo test` 317/317; `nest test` (debug)
536/536; `nest test --release` 536/536 (the `bound?` guards correctly
skip the debug-only primitives in release).

**Benchmark.** New `concurrency::big_string_fanout` (in
`benches/library.rs`): spawn N=100 workers, `send` a string of
`payload_bytes` to each, each replies with its `string-length`. Two
data points:

| `payload_bytes` | median | notes |
|---:|---:|---|
| 128 | 3.86 ms | inline / deep-copy (below threshold) |
| 10 000 | 20.19 ms | `Arc<SharedBlob>` / no byte copy |

The 78× larger payload costs ~5.2× more wall time — sublinear scaling
in payload size. Without Phase 1b, both sends would deep-copy bytes
twice per worker (`to_message` + `from_message`), and the 10 KB case
would be dominated by the per-byte memcpy. The existing
`spawn_fanout`, `sequence::*`, `strings::*`, and other benches all
moved within ±2% — the optimization isn't a slowdown for paths that
don't use it.

**Architectural finding (worth flagging for follow-up).** `spawn`
promotes captured locals via `Heap::promote`, which for `Value::Str`
extracts bytes into a fresh `String` in RUNTIME's
`boxcar::Vec<String>` — *not* the blob heap. So
`(spawn (fn () (use big-string)))` deep-copies a captured big string
once, into shared RUNTIME (where every spawned process reads it by
handle retag, no further copy). That's correct but not the *same*
mechanism as the send path. Routing `promote_string` through the blob
heap would unify the two and eliminate that one-shot copy; deferred to
a follow-up because the lifecycle story differs (RUNTIME is append-only
shared; the blob heap is refcounted shared). Flagged in `ADR-041`'s
Out-of-Scope.

**ADR.** [`decisions.md`](decisions.md) **ADR-041**.


## 2026-05-29 (later still) — Runaway-resource backstops (ADR-043) + live-editing hardening (ADR-042)

**Goal.** Two threads of work that had piled up uncommitted in the tree, now
documented and finished: stop a runaway program from taking the host down, and
land the cheap, high-value subset of the [`live-editing.md`](live-editing.md)
hot-reload plan.

**ADR-043 — backstops.** Adversarial / hostile code (the in-language
`tests/adversarial_test.blsp`, and eventually the editor `eval`-ing what you
type) had two ways to kill the *host* instead of failing cleanly:

- **Unbounded allocation** → a counting `#[global_allocator]` (`core/alloc.rs`)
  with a **soft** limit (polled at the `gc_block_depth() == 1` eval safepoint →
  catchable `E0043`) below a **hard** limit (enforced in `alloc`/`realloc` →
  process abort, the backstop for a single giant allocation between safepoints).
  Off by default; `BROOD_MEM_LIMIT` / `BROOD_MEM_SOFT_LIMIT` opt in; the test
  runners default a ceiling so a test can't OOM the machine. New
  `(mem-limit)` / `(mem-soft-limit)` primitives.
- **Unbounded non-tail recursion** → an **eval-depth ceiling** (`E0044`) checked
  at the top of `eval`: `GC_BLOCK` already counts non-tail frames (tail calls
  re-enter the `'tail:` loop without bumping it), so depth > `MAX_EVAL_DEPTH`
  (default 3500, `BROOD_MAX_DEPTH`) raises *before* the coroutine stack overflows
  into an uncatchable SIGSEGV.

**Test-memory lowered.** The test-runner default ceiling started at 2 GiB hard /
1.5 GiB soft; **cut 4× to 512 MiB / 384 MiB.** Per-process heaps are
`Rc`-reclaimed on green-process exit, so the suite footprint is the *concurrent*
peak across ~`nproc` workers, not a cumulative total — 512 MiB is ample, and a
real runaway now trips in a fraction of a second instead of after gigabytes. The
one test that *deliberately* drives an unbounded allocation
(`mem_limit.rs::soft_limit_turns_runaway_into_catchable_error`) is now
`#[ignore]`d so a routine `cargo test` can't OOM if the safepoint ever regresses;
run it with `cargo test --test mem_limit -- --ignored`. `basic.rs` (its own test
binary, previously uncapped) now arms the same ceiling via a `LazyLock` guard.

**ADR-042 — live-editing hardening.** Landed Stages 1, 2, 5-dedup, and 7 of the
[`live-editing.md`](live-editing.md) plan: **`defonce`** (prelude macro — state
and singletons survive reload, Emacs `defvar`), tighter **`reload-defs`**
definition detection (a `def`-prefixed *call* like `(default-config)` is now
correctly skipped — it's a `Fn`, not a macro) plus read-whole-file-first
atomicity, **dedup-on-identical** redefinition (a save-without-change doesn't
append a new RUNTIME version), and a **macro-redefinition staleness warning**.

**`defonce` un-removal — resolving the ADR-039 tension.** The 2026-05-28 entry
("supervised processes; `defonce` removed") scheduled `defonce`'s deletion once
*named-spawn* shipped. ADR-039 was then **tried and reverted** (named-spawn never
landed; the kernel supervisor was the bulk of the scheduler-race surface). So the
removal is **void** — and even had named-spawn shipped, it only covered the
process-singleton case, not the global-state-cell case. `defonce` is the chosen
tool, restored to the prelude. The roadmap already foreshadowed this ("the
`defonce` transitional shim stays in the prelude").

**ADRs.** [`decisions.md`](decisions.md) **ADR-042**, **ADR-043**.


## 2026-05-29 (re-confirmation) — KI-1 scheduler race verified fixed; docs reconciled

**Goal.** Independently confirm the multi-thread scheduler race (KI-1) is
actually closed before opening the next work track, and bring the docs that
still hedged into line.

**Verification.** Built `RUSTFLAGS="-C debug-assertions=on" cargo build
--release` — the mode that reliably exposed the race — and ran a reconstruction
of the original symptom: 40 green workers, each hammering prelude globals
(`fold`/`map`/`reduce`/`filter` + pattern-bound locals) in a tail loop and
fanning results back to the parent via `send`/`receive`. **12/12 clean** under
default `-j 0`, deterministic result every run; a heavier 80-worker variant under
`BROOD_GC_STRESS=1` was 3/3 clean before being stopped. None of the 2026-05-28
symptoms (bogus `unbound symbol: fold/%eq/iter/acc`, `eval/mod.rs` index panic,
parent hang, plain-release segfault) reproduce. Matches the as-fixed claim in
KI-1 (supervisor strip `e3d3a0d` + bump allocator `f90f0de` + per-worker pinned
queues `2abf05e`).

**Docs reconciled.** `known-issues.md` KI-2 still read "underlying race **largely
fixed**" / "The lookup race itself … **Still open**" and recommended `-j 1` for
correctness — all stale. Updated KI-2 to **fixed**, demoted the `-j 1` /
`:isolated` mitigations to "bounding a heavy run, not avoiding crashes."
`claude-demo-findings.md` row 1 went from "🟢 largely fixed (plain release can
still segfault)" to "✅ fixed", and its "What's open" list closed the
plain-release-segfault and Phase-2-allocator items.

**Heads-up flagged for the next track.** Work-stealing and kernel supervision are
*exactly* the two pieces whose removal fixed this race. The findings doc now
records that reintroducing either must clear the bar of not reopening KI-1 — the
substrate (bump allocator + pinned queues) is what currently makes the race
impossible, and a naive work-stealing reintroduction would undo the pinning.


## 2026-05-29 (concurrency-v2 track) — userland supervisor library (ADR-044)

**Goal.** First concrete deliverable of the concurrency-v2 track
([`concurrency-v2.md`](concurrency-v2.md), §4.3 "userland-first"): supervisor
trees as a require-able Brood library, with **zero** new scheduler surface — the
property that matters after KI-1, since kernel supervision was the bulk of that
race.

**Shipped.** `std/supervisor.blsp` (embedded module, `(require 'supervisor)`):
- `start-supervisor` — spawns a supervisor process that starts a list of child
  specs, `monitor`s each, and restarts them on `[:down ref pid reason]`.
- Child spec map: `:start` (0-arg fn that spawns the child, returns its pid),
  optional `:id`, `:restart` type — `:permanent` (always), `:transient` (only on
  abnormal exit), `:temporary` (never).
- Restart-intensity window: `:max-restarts` in `:max-seconds` (default 3/5);
  exceeding it exits the supervisor abnormally so a watcher's monitor fires.
- `which-children` (synchronous introspection → `[{:id :pid :restart}]`),
  `stop-supervisor`.
- Pure Brood policy over `spawn`/`monitor`/`receive` (ADR-006); state is one
  immutable map threaded through a tail-recursive receive loop (the `hatch.blsp`
  idiom). The `:start` closures ride into the supervisor process via the
  closure-as-data path (ADR-033) and are re-invoked on restart.

**Scope (ADR-011 + a real kernel gap).** Only `:one-for-one`. `:one-for-all` /
`:rest-for-one` must terminate *healthy* siblings; Brood has **no kill/exit
primitive** (no links, no `exit`), so they're impossible in userland today —
`start-supervisor` rejects them up front. Same gap means `stop-supervisor` ends
the supervisor but leaves children running (orphaned), and an intensity shutdown
orphans survivors. This is now the concrete trigger for the one kernel hook
supervision might later justify (a minimal `exit`/link) — see
[`concurrency-v2.md`](concurrency-v2.md) §4.

**Rust delta.** One line: `("supervisor", include_str!("…/std/supervisor.blsp"))`
in `EMBEDDED_MODULES`. Nothing else.

**Tests.** `tests/supervisor_test.blsp` (`:isolated`): permanent restart yields a
fresh pid; `:transient` restarts on crash but not on clean `:normal` exit;
`:temporary` never restarts; exceeding intensity shuts the supervisor down
abnormally (observed via a monitor on the supervisor); `which-children`
summaries; unsupported-strategy rejection. 7/7 green. Sibling embedded module
(`hatch`) 9/9 and the core suite 57/57 confirm the embed-list change is clean.

**Docs.** ADR-044; [`supervision.md`](supervision.md) gained a "supervisor
library" section; roadmap entry + concurrency-v2 §4.3 flipped to ✅ for the
one-for-one slice.

## 2026-05-29 — M2 Phase 0: the text rope substrate (`Value::Rope`, ADR-045)

**Goal.** Begin the editor (M2) by adding the one piece of buffer-text mechanism
the language can't bootstrap: an efficient, immutable text rope. Chosen approach
(this session's planning): a thin end-to-end editor slice, **TUI-first**, with
text as an **opaque immutable-rope handle** owned by a **buffer-as-process**.
Phase 0 is just the rope value + primitive kernel; everything above it is Brood.

**Decision.** ADR-045 — a single new heap value `Value::Rope(RopeId)` / `Tag::Rope`
backed by `ropey::Rope`. ropey's `Arc`-shared B-tree gives O(1) clone + copy-on-
write edits, so immutability (ADR-026) holds *for free*: `rope-insert`/`rope-delete`
clone-then-edit and return a fresh rope sharing unchanged structure. Ropes are
**process-local** — they never cross in a message (`to_message` errors with a
hint to send `rope->string`), matching the buffer-as-process design; a rope
`def`'d to a global *is* promoted into RUNTIME (mirrors `Str`; ropey is Send+Sync).

**Built.**
- `ropey` dep (lisp crate). `Value::Rope` + `Tag::Rope` (16th tag — fills the
  `Ty(u16)` lattice exactly; `UNIVERSE` now computes in u32 then narrows to dodge
  the `1u16 << 16` const-overflow the old comment predicted).
- Full heap wiring: a `ropes` slab in LOCAL + a `boxcar` slab in RUNTIME, plus
  every coordinated GC/region site — `FreeLists`/`PoisonBits`/`LocalCheckpoint`/
  `Marks`/`FlushForward` + their methods, `alloc_rope`, the LOCAL+RUNTIME `rope()`
  accessor, `promote` (copy to RUNTIME), `flush_rope` (the live arena-flip path),
  the dormant mark/sweep, `tag_rank`/`to_prelude`/`hash_value_into`/`equal`/
  `local_live_count`, and the printer (`#<rope :chars N :lines M>` — never dumps
  a whole buffer). `to_message_rec` and `mcp::value_to_json` reject ropes cleanly.
- 10 primitives (char-indexed): `string->rope` `rope->string` `rope-length`
  `rope-line-count` `rope-insert` `rope-delete` `rope-slice` `rope-line`
  `rope-char->line` `rope-line->char`. Out-of-range → clean `E0012`, never a
  ropey panic. `rope?` predicate in the prelude.

**Tests.** `tests/rope_test.blsp` — 28 tests: construction/predicates, length/
lines, immutability (original untouched after edit), slice/line, char↔line round-
trip, content equality, OOB errors, and an `:isolated` across-processes block
proving ropes are process-local (sending one raises), that workers edit ropes on
their own cores and return *strings*, and a buffer-as-process that holds a rope in
its loop and serves `:insert`/`:text` — the Phase-1 model in miniature. 28/28
green, including under `BROOD_GC_STRESS=1`. Full workspace builds clean.

**Known issue (pre-existing, NOT this change).** The full `cargo test` suite now
rides the 4 GiB test soft cap exactly: 3 memory-heavy `adversarial_test` units die
with a spurious `E0043`. Verified pre-existing — they fail identically with
`rope_test.blsp` removed (565 tests, same 3, peak 4096 MB), so the current
(uncommitted-WIP) tree already exceeds the cap independent of ropes. Raising the
cap was tried and reverted: the suite peak tracked the cap up to 6 GiB, i.e. the
demand is genuinely runaway, so a bump masks rather than fixes it. Left for
investigation (likely the in-progress scheduler/alloc changes).

## 2026-05-29 — M2 Phase 1: the buffer framework (`std/buffer.blsp`)

**Goal.** The editor *toolkit* layer on top of Phase 0's rope: buffers, points,
marks, regions, movement, editing — pure Brood, opt-in, isolated from the
language. Architecture agreed this session: a three-layer split (Rust rope
kernel → Brood editor framework → the editor app as a separate nest project),
mirroring Emacs (C → built-in elisp → packages). The framework is **not part of
the language**: `(require 'buffer)`, never in the prelude, zero kernel surface.

**Design choices (this session).**
- *Home:* an embedded, require-able std module (added to `EMBEDDED_MODULES`),
  so the future editor nest project gets it for free with no package manager
  (ADR-037 still ⬜). Extractable to a standalone package verbatim later.
- *Buffer = a pure immutable value* (a map `{:rope :point :mark :name :file}`)
  with pure ops returning fresh buffers — the testable foundation. The
  *buffer-as-process* is a thin `spawn-buffer` actor that **holds** such a value.
- *The rope-locality boundary.* A buffer holds a rope, and ropes are
  process-local (ADR-045), so a buffer can't cross a process. `spawn-buffer`
  ships the buffer's *text* (+ name/file/point/mark) and rebuilds the rope inside
  the child; the actor replies only with **derived views** (text, line strings,
  positions), never the buffer/rope. Edits and reads cross as **closures**
  (`buffer -> buffer` / `buffer -> view`) via closure-as-data (ADR-033) — the
  loop is just `([:edit f] …) ([:get f from r] …)`. That reply-with-views seam
  is the seed of the M3 display protocol.

**Built.** `std/buffer.blsp`: `make-buffer`/`buffer-from-file`/`save-buffer`,
reads (`buffer-text`/`-point`/`-mark`/`-length`/`-line-count`/`-line-at`/
`-current-line`/`-column`/`-char-after`/`-before`/`-region`), movement
(`goto-char`/`forward`/`backward-char`/`beginning`/`end-of-line`/`forward`/
`backward-line` (column-preserving)/`beginning`/`end-of-buffer`), mark
(`set-mark`/`clear-mark`), editing (`insert`/`delete-char`/
`delete-backward-char`/`delete-region` — all clear the mark, the simple v1
choice), and the actor shell (`spawn-buffer`/`buffer-edit`/`buffer-query`/
`stop-buffer`). All pure Brood over the rope primitives; `buffer-`/`buffer--`
naming, one `(defmodule buffer …)`.

**Tests.** `tests/buffer_test.blsp` — 28 tests: construction/reads, movement
(clamping, column-preserving `forward-line`), immutable editing (original
untouched), mark/region (either order), a real file round-trip through `/tmp`
(slurp → edit → save → re-read), and an `:isolated` actor block (spawn → edit via
a shipped closure → query a view; point/mark preserved across the spawn; two
buffer processes independent). 28/28 green incl. `BROOD_GC_STRESS=1`.

**Next.** A new `nest` project for the actual editor — keymaps, commands, config
— built on `(require 'buffer)`. And/or the crossterm seam (Phase 3) + a simple
`nest` process observer (needs no rope — just `list-processes` + the seam).


## 2026-05-29 (concurrency-v2 track) — spawn-time load balancing; work-stealing ruled out

**Goal.** Resolve the Track-A question from [`concurrency-v2.md`](concurrency-v2.md)
§3 — is work-stealing viable on today's substrate, and what's the safe
throughput win — by experiment, in an isolated worktree, without touching the
just-stabilized scheduler in main until the answer was in.

**Experiment (worktree `track-a-workstealing`, branch committed `2479190`).** Two
opt-in scheduler variants behind env flags (default path byte-identical), built
in plain release, 40-worker KI-1 repro × 10:

| config | result |
|---|---|
| baseline (pinned round-robin) | 0/10 fail |
| `BROOD_BALANCE` (least-loaded assign, no migration) | 0/10 (also clean under `BROOD_GC_STRESS`) |
| `BROOD_STEAL` (work-stealing) | **10/10 segfault** |
| `BROOD_STEAL` + preempt disabled | 0/10 |

**Finding — work-stealing is blocked at the substrate.** It fails *specifically*
on preempt-induced cross-thread migration (resuming a coroutine suspended
mid-computation, deep native stack, on a different OS thread) — the same wall
`2abf05e` hit. gdb (3/3) puts every crash in `scheduler::preempt` at the
`(*yptr).suspend(…)` call with a **smashed return address** (`0x7`), *not* in
corosensei's switch asm; the Brood-side `CURRENT`/yielder re-establishment that
hypothesis 2 proposed is **already present** and doesn't help. So a deep saved
coroutine stack is not safely resumable cross-thread in corosensei 0.3.4 — a
substrate limit, not a cheap TLS fix. (Stealing *fresh, never-resumed* processes
is safe — a viable migration-free partial, if a spawn-burst workload ever needs
it. Recorded in §3.2; not built.)

**Landed in main — spawn-time load balancing (default-on).** `assign_worker` now
scans the worker queues from a rotating (round-robin) start and pins a fresh
process to the **least-loaded** one (shortest queue, sampled via `try_lock`,
ties toward the rotation, early-out on an empty queue). No migration — a process
still lives on one worker for life, so the KI-1b hazard never arises (INV-2
holds). When load is even (most queues empty) it degrades to plain round-robin;
when one worker is backed up, fresh processes steer to idle workers. Replaces
pure round-robin.

**Validation (main, default-on).** Plain release KI-1 repro 0/8; concurrency
suite 31/31, pids 4/4, supervisor 7/7, core suite 57/57; `preemption` Rust test
green; 5000-process burst ~1911 ms (no measurable overhead from the per-spawn
scan — was ~1811–1911 ms either way). Honest caveat: queue-length is an
imperfect load signal — it doesn't see a long-running process *occupying* a
worker (only queued ones), so it improves burst distribution, not uneven
long-task occupancy. A per-worker busy flag is the future refinement if that
matters.

**Docs.** [`concurrency-v2.md`](concurrency-v2.md) §3.1a (experiment results) +
§3.2 (revised directions: balance ✅ landed, fresh-only stealing 🟡 optional,
live-coroutine stealing ❌ substrate-blocked). Experiment preserved on branch
`track-a-workstealing`, unmerged.


## 2026-05-29 — M3 Phase 0: the display/input seam + `nest observe` (ADR-046)

**Goal.** Start M3 — the seam between the runtime and any frontend — and prove it
end-to-end with the smallest real app: a terminal process observer. Picked over
starting the editor project because it needs **no rope/buffer**, so it validates
the render protocol + key loop in isolation before the editor rides on it. (Prior
session's recommended first step; resumed from another Claude profile's session.)

**Design (ADR-046).** A frontend is a **protocol, not a library** (architecture.md).
The render frame is **Brood data** — a vector of tagged ops (`[:clear]`, `[:text
row col s]`, `[:text row col s face]`, `[:cursor row col]`; a face is a map of
`:fg`/`:bg`/`:bold`/`:reverse`). Rust supplies only the *frontend that paints it*:
five `term-*` primitives over `crossterm`. So a remote/web frontend re-implements
the identical ops over a socket later — the seam that makes local-fast and
server-mode one code path. Mechanism in Rust, protocol-meaning + observer policy
in Brood.

**Built.**
- *Rust (mechanism):* `crossterm` dep + `term-enter`/`term-leave`/`term-size`/
  `term-poll`/`term-draw` in `builtins.rs` (`term-draw` is a ~40-line interpreter
  of the frame vector; never panics — clean `LispError`s like the rope prims),
  plus one introspection accessor `mailbox-size` (the mailbox queue is behind the
  scheduler registry, unreachable from Brood; added `process::mailbox_len`).
  `pub fn restore_terminal()` for the host-side panic backstop.
- *Brood (policy):* `std/display.blsp` (pure render-op constructors `clear`/
  `text`/`cursor`/`frame`, nil-dropping) and `std/observe.blsp` (a pure
  `observe-frame` builder + a thin `observe-run` IO loop). Both embedded, opt-in.
- *`nest observe`:* runs the observer in the **root process** (outside the worker
  pool, so its blocking `term-poll` never starves the processes it observes) with
  an RAII `TermGuard` restoring the terminal on panic/error.

**Two design points that mattered.**
- *Scheduler safety.* Preemption can't interrupt a process parked in a native
  crossterm call, so an infinite poll on a *green* process would pin a worker.
  Fix: observer = root process (its thread isn't in the pool) + always a finite
  poll timeout. Traced via the Plan agent against scheduler.rs.
- *Terminal restore.* `process::exit` skips `Drop`, so the guard is scoped to drop
  (restore) *before* an error-exit; the normal path is the Brood `term-leave`, the
  guard is the abnormal-path backstop (fires on panic unwind). Verified: a non-TTY
  run surfaces a clean `runtime error: terminal: …` and restores the screen.

**Interactive + node panel (same day).** Per the next ask ("more node info + some
interactivity") the observer grew a **node-stats header** (node name, workers/peak,
spawn count, mem used/peak, peers — all existing primitives, no new Rust) and
**keyboard navigation**: `↑`/`↓` (or `k`/`j`) select a process, the row is
caret-marked + reverse-highlighted, a detail line names the selection, `space`
freezes/resumes the live refresh, `r` refreshes, `q`/Esc/Ctrl-C quits.
Interactivity with **no mutation**: the UI state (`{:sel-pid :frozen}`) is a plain
map threaded through the tail-recursive loop — each keypress recurses with a fresh
state. Selection is tracked **by pid string**, not row index, so it stays on the
same process as the busiest-first list reorders under it (and recovers to row 0 if
the process dies). `observe-frame` gained the node map + `sel` + `paused` args and
windows long lists (centred on the selection) with a `[sel/total]` counter.

**Tests.** `tests/observe_test.blsp` — 18 tests: display constructors + nil-drop,
`observe--fit`/`observe--row`/`observe--bytes->human`/`observe--window-start`/
`observe--sel-index`/`observe--node-lines` helpers, `observe-frame` structure
(node panel, per-process rows, selection marker+highlight, detail line, paused
footer, windowing + position counter, width-clipping), and an `:isolated` live
block driving the new `mailbox-size` primitive + `observe--snapshot` across real
spawned processes. 18/18 green incl. `BROOD_GC_STRESS=1`. The `term-*` primitives
need a real TTY, exercised manually via `nest observe`.

**Gotchas hit.** `concat`→`append` (the variadic seq concat); `get` is map-only
(use `nth` to index a vector); `sort-by` compares with numeric `<` only (can't
sort pid strings) → sort by mailbox backlog **descending** instead, which is also
the better observer ordering (busiest first, like Erlang's observer).

**Next.** The editor app — a new `nest` project that `(require 'buffer)`s the M2
framework and renders through this seam: keymaps, commands, config. Later additive
on the same protocol: faces beyond fg/bold, mouse/resize, scroll, and attaching
the observer to a *remote* live image (the dist/node machinery exists for it).

## 2026-05-29 — Runaway-resource safety (real this time) + native multi-arity dispatch (ADR-047)

**Goal.** Two things. **(A)** Make runaway recursion/allocation *actually* safe —
the in-flight ADR-043 backstops didn't work, a deep non-tail recursion still
SIGSEGV'd a green process (the MCP-server-killer from `claude-demo-findings.md`).
**(B)** Close the variadic-arithmetic performance gap *without* moving `+`/`-`/`=`
to Rust — the dogfooding-aligned fix the CLAUDE.md "build the language up" rule
calls for.

**A — runaway-resource safety.**
- *Byte-based stack guard (E0044).* The old E0044 was a frame *count* (3500),
  miscalibrated ~40×: a debug green-process coroutine (2 MiB stack) overflows at
  ~90 frames, so `(defn boom (n) (+ 1 (boom (+ n 1)))) (boom 0)` still segfaulted.
  Frame-counting can't work — heavy vs light frames differ ~7× in bytes. Fix
  (`process/scheduler.rs` + `eval/mod.rs`): record the per-coroutine stack-base sp
  at the outermost eval, save/restore it across suspend (alongside `GC_BLOCK`, in
  `scheduler::preempt` and mailbox receive), and check `base - sp` against
  `stack_budget()` each eval → clean catchable **E0044**. `CORO_STACK_BYTES`
  2 MiB→16 MiB (lazy mmap, ~free); `brood`/`nest`/`suite.rs` re-home root work onto
  a `CORO_STACK_BYTES` thread so the budget is uniform. Verified: `(boom 0)` → clean
  E0044 at root *and* in a green process; legit non-tail recursion to 300+ levels.
- *Soft memory limit made depth-independent* (`eval/mod.rs`): the E0043 check no
  longer gates on `gc_block_depth()==1`, so a runaway in argument position is
  caught (raising just unwinds — no rooting constraint, unlike GC).
- *Test memory cap* (`core/alloc.rs`): **5 GiB hard / 4 GiB soft** — a *host-
  survival backstop*, not a working-set budget. **Never set it 0/unlimited** (no
  GC → the suite tried ~18 GiB and OOM-froze the host once).
- *Test framework* (`std/test.blsp` `run-isolated`): `:isolated` units run in their
  **own spawned process** (one at a time), so each unit's heap is reclaimed on exit
  — was ~18 GiB accumulation on the long-lived runner, now ~190 MB for that phase.
- *Adversarial tests* (`tests/adversarial_test.blsp`): fixed the long-atom test
  (string vs symbol), the 200-worker blob test (echoers report `%blob-ptr` so
  `adv-collect` drains all 200 — undrained strings were contaminating later
  `:isolated` tests via the shared runner mailbox), capped the heaviest stress
  counts (100k→30k) given real no-GC accumulation.

**B — native multi-arity dispatch (ADR-047).** Variadic `+`/`-`/`<`/`=` were Brood
`defn`s over `fold` + a `& xs` rest-list — ~15 env frames per `(+ a b)`, ~40× a
direct call; `(sum-to 100000)` burned 497 MB on that overhead (none reclaimed — GC
is a no-op). The wrong fix is making them Rust builtins; the right one is giving the
*evaluator* the missing capability. Now a closure holds `Vec<ClosureArm>` (was flat
`params/optionals/rest/body`); the call's arg count selects an arm
(`Closure::select_arm`: exact fixed beats variadic, then most-specific) which binds
its params **directly** — no rest-list, no `match*`. Arity-only clauses (plain
symbols + `&optional`/`&`) dispatch natively; clauses with literal/destructuring
*patterns* still lower to `match*` (Erlang-style same-arity dispatch, untouched).
`arms` threaded through the whole closure lifecycle: `make_closure`/`bind_params`/
`apply_closure` + the inline TCO path (`eval/mod.rs`); `expand_fn_clauses` in the
compile pass (expands each clause *body*, leaves param-lists opaque so a second
clause's `(a)` head isn't mangled into a call); `promote_closure`/`flush`/GC trace/
structural-dedup (`heap.rs`); `to_message`/`from_message` (cross-process spawn) +
the dist wire codec (cross-node); the type checker (`infer_sig` single-arm only —
sound; `arity_of` spans arms). `std/prelude.blsp` `+ * - / < > <= >= = not=`
rewritten with fast 0/1/2-arg arms + a variadic 3+ fallback.

**Result.** `(sum-to 100000 0)` = **61 MB, was 497 MB → 8.1×**; `basic.rs`
29 s → 5 s. Correctness spot-checked: `(+)`/`(+ 5)`/`(+ 1 2)`/`(+ 1 2 3 4)` →
`0 5 3 10`; `(- 5)`→`-5`, `(- 10 3 2)`→`5`; `(< 1 2 3)`→`true`, `(< 1 3 2)`→`false`;
pattern multi-clause (`fac`, `alive-next?`) still works.

**Tests.** `cargo test -p brood --test basic --test gc --test mem_limit --test
preemption` green (basic 75, gc 3, mem_limit 1, preemption 1).

**Known limitation (advisory, non-blocking).** The advisory scope checker emits a
spurious `unbound symbol` warning for params bound only in arity arms *after* the
first (`(defn f ((a) …) ((a & more) …))` warns on `a`). Runtime is correct; the
checker's scope walk just doesn't register per-arm params beyond arm 0. Cosmetic —
worth a follow-up in `types/check/` but doesn't reject any program.

**Still open (GC-blocked, host-safe).** The *full* in-language suite still grows to
the memory soft cap — multi-arity cut *per-op* cost 8× but not the *cumulative*
no-GC accumulation (`Heap::collect` is a no-op; `collect_old` is the disabled
ADR-035 mark-sweep). Failures are clean E0043, not crashes. The real fix is
re-enabling the tracing GC (M1) — see `memory/no-gc-suite-memory.md`. **Note:**
`roadmap.md` currently marks the tracing GC as landed (ADR-035); in the code
`collect` is a bump-allocator no-op, so that line overstates the present state.


## 2026-05-29 — Three language fixes surfaced by dogfooding the editor seam

Writing the display seam, the observer, and the REPL turned up three rough edges;
each is a *broad* capability fix, not a one-off (the CLAUDE.md bar), so they went
in rather than getting worked around.

1. **Reader `INCOMPLETE_INPUT` code (ADR-049).** EOF-mid-form / unterminated-string
   parse errors now carry the stable code `"E0002"` (via `err_incomplete` /
   `err_at_incomplete` over the 7 "ran out of input" sites in `syntax/reader.rs`),
   distinct from genuine syntax errors. The self-hosted REPL (ADR-048) matches the
   code (`repl--incomplete?`) for multi-line continuation, **deleting** its
   hand-rolled delimiter scanner (`repl--balanced?`/`repl--scan`) — correctness
   (strings, comments, escapes) is now the reader's, single-sourced. An editor's
   eval-region / structured editing reuses the same signal.

2. **Polymorphic `compare` + `sort-by` over it.** Hit while building the observer:
   `sort-by` compared with numeric `<`, so sorting by a string key threw, even
   though `(sort coll)` was already structural. New primitive `(compare a b)` →
   -1/0/1 exposes the existing `Heap::value_cmp` total order; `sort-by` now
   `(< (compare (key x) (key y)) 0)`. Numeric keys unchanged; string/keyword/vector
   keys now work. Primitive count 97 → 98.

3. **Polymorphic `get`.** `(get v 1)` used to throw "map-get: expected map" — every
   data-shuffling snippet hit the `get`-vs-`nth` split. `get` now dispatches: maps
   by key, strings by char index, vectors/lists by integer index (via `nth`), with
   out-of-range → default (never an error), matching Clojure. `get` is therefore no
   longer a map-only primitive — `tests/maps_test.blsp` moved its non-map-rejection
   assertions off `get` and gained a polymorphic-`get` block.

All in Brood where possible (`get`/`sort-by` are prelude; only `compare` and the
reader codes are Rust). Suite 650/653 — the 3 failures are the unrelated
pre-existing adversarial memory-cap (E0043) tests.

---

## 2026-05-29 — std library review: `sleep` mailbox bug + dedup of clobbered globals

**Goal.** Read through the language guide and all of `std/`, looking for
correctness and consistency improvements without breaking anything.

**Found + fixed.**
- **`sleep` consumed the mailbox and returned early (real bug).** The prelude's
  `(defn sleep (ms) (receive (after ms nil)) nil)` had an *empty* clause list, so
  the generated matcher matched *any* message: `%receive` grabbed whatever was
  queued and returned immediately, never waiting. Verified:
  `(send (self) :marker)` then `(sleep 1000)` returned in 0 ms and ate `:marker`.
  Fixed by pinning a fresh unforgeable `(ref)` as the only clause (`~never`), the
  trick `hatch.blsp` already used in its *override* of `sleep` — so the fix now
  lives in the prelude and `hatch`'s duplicate is gone. `tests/hatch_test.blsp`
  already asserted the correct behaviour (it only passed via that override);
  added a prelude-level regression guard to `tests/concurrency_test.blsp`.
- **Removed three `std/` modules' redefinitions of prelude functions.** The
  runtime shares one global table across all loaded modules, so a `require`-able
  module redefining a prelude name re-binds it *for the whole image* — the same
  footgun an existing `test.blsp` comment documents for `take`/`quot`. Cleaned up
  the stragglers: `test.blsp`'s `pad-left`/`pad-right` (+ non-tail `spaces`
  helper) and `format.blsp`'s `string-repeat` (an O(n²) `(str acc s)` accumulator
  vs. the prelude's single-pass `(apply str (repeat n s))`). All now use the
  prelude's stack-safe versions.

**Verified.** `nest test` — 653 tests, 0 failed (release build).


## 2026-05-29 — Self-hosted REPL: the read-eval-print loop moves into Brood (ADR-048/049)

**Goal.** Retire the Rust REPL (`crates/repl`, `rustyline`) and write the
read-eval-print loop in Brood — the long-standing M1 "self-host the CLI/REPL"
item. The editor backbone made it reachable: `eval-string` is the evaluator,
`try`/`catch` surfaces structured errors, and the M3 work proved Brood can own an
IO loop.

**Built.**
- *Rust (mechanism, the only new surface):* one primitive `(read-line)` — a
  blocking stdin line read, returns the line (newline stripped) or `nil` at EOF.
  `std/repl.blsp` added to `EMBEDDED_MODULES`. `brood` (no args) and `nest repl`
  now bootstrap into `(require 'repl) (repl-run)`; `crates/repl` + the `rustyline`
  dependency are deleted.
- *Brood (policy):* `std/repl.blsp` — the loop, prompts (the dynamic vars
  `*repl-prompt*` / `*repl-cont-prompt*`), readable result echo (`pr-str`), and
  structured-error rendering off the `{:kind :message [:line :col] :code …}` map.
  Reads work piped too; prompts/banner gate on `(stdout-tty?)`.

**Two design points that mattered.**
- *Multi-line input via the reader, not a delimiter scanner (ADR-049).* An
  unclosed form/string makes `eval-string` raise the reader's `INCOMPLETE_INPUT`
  error (code `E0002`) — the "read another line" signal; any other error is real.
  Since `eval-string` reads all forms before evaluating any, an incomplete buffer
  throws at read time with nothing evaluated, so retrying the growing buffer is
  side-effect-free. (Dropped an earlier hand-rolled Brood balance scanner — the
  reader already knows "complete," strings/comments/escapes included.)
- *Memory: `hibernate` in a spawned process.* The tracing GC (ADR-035) is a
  current no-op, and a nested `eval-string` doesn't hit the top-level arena reset,
  so a naive loop leaks — **measured ~15 GB** RSS over 50 000 commands. Fix:
  `repl--loop` recurs via `(hibernate repl--loop tty)`, flipping the LOCAL arena
  each command (keeping only the loop fn + `tty`). `hibernate` is caught only by
  the spawned-process scheduler loop, so `repl-run` runs the loop in a spawned
  process and `monitor`s it to await EOF. **After: flat ~8 MB** at 2 000 and
  20 000 commands alike.

**Tests.** `tests/repl_test.blsp` — 12 tests: datum detection (`repl--content?`),
incomplete-input detection (`repl--incomplete?` off E0002, incl. `repl--eval-print`
returning `:more`), error rendering, and an `:isolated` cross-process error-map
round-trip. The IO loop itself is exercised manually (`brood`, piped input).

**Pre-existing, unrelated.** The full `cargo test` aggregate still shows the 3
adversarial heap-allocation tests dying on the 4 GB process-wide soft cap under
peak concurrency — confirmed identical at committed `HEAD` (a worktree check) and
a known no-GC-suite-memory consequence (ADR-047). Not introduced here.

**Next.** Arrow-key history/recall over the `term-*` raw-key seam (ADR-046) + the
buffer framework — now a Brood function to add, not a Rust dep to carry.

## 2026-05-29 — Memory review + Stage A: hibernate the test runner

**Goal.** Stop the in-language suite (~655 tests) climbing to the memory soft cap
(clean E0043). Brief: *slow-and-stable over fast-and-spiky* — reclaim often, keep
the working set flat, accept the CPU.

**Review first** (`docs/memory-review.md`). Mapped the memory model onto standard
techniques. Findings: (1) `Heap::flush` is already a textbook **semi-space copying
collector** — we only *trigger* it manually via `(hibernate)`; the spikiness IS
that manual trigger. (2) "immutability ⇒ acyclic ⇒ refcounting suffices" is
**false** — `letrec`/mutual recursion build real `env↔closure` cycles (flush handles
them), so pure RC would leak; we need tracing/copying. (3) Index-slab handles make
slot reuse *unsafe* (silent aliasing) — why bump-only was chosen and the in-place
mark-sweep stays disabled; copying sidesteps it (relocate + drop). (4) Two rooting
models: in-place mark-sweep at the `GC_BLOCK==1` safepoint (disabled: slot reuse +
the scheduler-suspend race) vs. copying `flush` after an unwind (safe; tail-call
boundaries only). Recommendation: fire the existing copy **automatically at a memory
threshold** (Stage B, the threshold = the slow/stable dial); do **Stage A** first.

**Stage A (this entry).** The runner is one long-lived process, so with the GC off
it accumulated every step's transients (worker-result copies, the `append` spine,
spawn/monitor machinery) until the cap. Merged the old `run-isolated`/`run-parallel`
phases into a single **hibernating driver** (`std/test.blsp`): a spawned process
that runs the work list one step at a time (each isolated unit alone; parallel units
in `*parallel-batch*` groups) and `(hibernate)`s between steps — each flip keeps
only `(steps parent acc)` and drops that step's garbage. `run-tests` /
`run-tests-structured` delegate via `drain-runner` (spawn + monitor + await the
`:all-results` message). Marked a **TEMPORARY smell** (userland GC trigger) to delete
when Stage B lands.

**Result.** Full suite **655/655 pass, peak 1135 MB** (was ~4 GiB tripping the cap
→ ~3.5× lower, well under the soft cap; no more E0043). Confirms the runner's growth
was dominated by *garbage*, not live data. `cargo test` green (`basic.rs` incl.
`run-tests-structured`; `suite.rs` full run, 48.9 s).

**Gotchas.** `partition` **drops** a short trailing chunk — batching units with it
would silently skip tests; wrote a remainder-keeping `run--chunks`. `receive`/match
patterns: a plain symbol **binds**, `~x` **pins** an existing value (got it backwards
at first). Mailbox messages are stored serialized (off the LOCAL heap), so they
survive a hibernate arena flip — fully collecting a batch before hibernating keeps no
in-flight message across the boundary.

**Next.** Stage B — automatic threshold-triggered copying collection at the eval
safepoint, after auditing `GC_BLOCK` save/restore across preempt + receive. Removes
the Stage-A smell and is the real fix.

---

## 2026-05-29 — Game-of-Life feedback: bitwise ops, a standard PRNG, discovery tools

**Goal.** Act on `docs/feedback-retro-game-of-life.md` — an AI assistant built a
Conway's Life in Brood and reported the friction. Knock out the contained,
high-value items in one pass; leave the deeper items scoped (see **Deferred**).

**Built.**
- **Bitwise primitives** (`builtins.rs`): `bit-and` `bit-or` `bit-xor` `bit-not`
  `bit-shift-left` `bit-shift-right` — i64 two's-complement, arithmetic right
  shift, shift amount validated to `[0, 64)` (out of range is a clean error, not
  a Rust panic). The one genuinely irreducible piece here; everything below sits
  on top of them. Table stakes the feedback called out (hashing, flags, PRNG
  quality).
- **A standard PRNG, in Brood** (`std/prelude.blsp`): `rng` `rand-seed`
  `rand-int` `rand-float` `shuffle` `sample`. Pure and seedable — every step is
  `seed -> [value next-seed]`, threaded like any other value (no global mutable
  PRNG; respects ADR-026). Marsaglia **xorshift32** with 32-bit masking, chosen
  precisely because integer `+`/`*` *error* on overflow (they don't wrap) — the
  shifts stay well within i64 and mask back to 32 bits, so no overflow. The #1
  ergonomic gap in the feedback; before this every user hand-rolled an LCG.
- **Discovery / introspection** (`std/prelude.blsp`): `all-globals` (alias of
  `global-names`), `apropos` (name substring; accepts string/symbol/keyword),
  `doc-search` (matches docstrings, returns `[name doc]` pairs). Answers "does an
  RNG exist?" in one call instead of a dozen unbound-symbol probes. Also exposed
  as three **`nest mcp` tools** (`apropos`/`all-globals`/`doc-search`) — the
  catalogue is twelve tools now.
- **Scaffold doc fix** (`std/project.blsp`): the `nest new` CLAUDE.md template now
  *shows* `:main` syntax (`:main 'app` → `app/main`; `:main '(app start)`) and
  states the flat-namespace naming rule (exactly one `main` project-wide) — the
  feedback's silent-duplicate-`main` trap, headed off at scaffold time.

**Tests.** `tests/math_test.blsp` (+bitwise describe: ops, sign-preserving shift,
out-of-range error, type errors); new `tests/prng_test.blsp` (determinism, ranges,
shuffle-is-a-permutation, plus an `:isolated` across-processes block proving a
seed's stream survives the deep copy on `send`); `tests/introspection_test.blsp`
(+discovery describe). The keyword-pattern test caught a real bug — `apropos` used
`str` (which keeps a keyword's leading colon); fixed to `name`. Full suite green
(`cargo test`: 136 + 75 + 53 unit, `brood_suite_passes`, MCP + distribution).

**Deferred (scoped, not done).**
- **The §8 memory leak** — the headline finding. It's the known "spiky memory":
  the copying collector (`flush`) exists but only fires on a manual `(hibernate)`,
  so a long-running `nest run` loop (a TUI game) never reclaims. The real fix is
  **Stage B** of `docs/memory-review.md` (auto-fire the copy at a memory threshold
  at the eval safepoint), which needs the `GC_BLOCK`/suspend rooting audit first —
  deliberately *not* rushed.
- **Set type `#{}`**, **duplicate-global warning** in `check-project-sources`, a
  **`--main`/`module/fn` CLI override**, and a **frame-capped run mode** — all
  mapped (insertion points known) but left for follow-up.


## 2026-05-29 — Richer process introspection: `(process-info pid)` + observer (ADR-051)

**Goal.** Turn the observer's bare pid+mailbox list into a real
`process_info`-style view, surfaced both as a primitive and in the TUI. Planned
around in-flight kernel changes (an explicit status enum, parent tracking, a
per-process memory counter) so `process-info` *consumes* that bookkeeping as it
lands rather than blocking on it.

**Shape (decided with the user).** One primitive `(process-info pid)` → a snapshot
**map** (not granular accessors), Erlang-idiomatic; `nil` for remote/dead, type
error for a non-pid (the `mailbox-size` contract). Reachable *today*, no new kernel
bookkeeping:

    {:id 7 :node :nonode :name :worker :status :waiting :mailbox 3 :monitored-by 1}

- `:id`/`:node` free off the `Value::Pid`; `:name` via `dist::name_for_pid`;
  `:mailbox` via `mailbox_len`; `:monitored-by` via new `monitor::monitored_by`;
  `:status` inferred (mailbox `waiter` slot → `:waiting`, else `:running`; dead →
  nil) via new `mailbox::process_status`. Each accessor takes one lock and releases
  it before the next, so `process-info` holds no two at once (no lock-ordering
  risk; a stale-but-coherent snapshot is fine for display).
- `:parent`/`:memory` **deferred to the kernel work** (the `Process` isn't
  registry-reachable while running). The map's key *set* grows monotonically; the
  observer renders absent fields as "-", so they appear with no rework.

**Observer (`std/observe.blsp`).** Snapshot is now `(map process-info
(list-processes))`, sorted by `:id` — the monotonic id is finally a stable sort key
(the gap that forced busiest-first). New table: id · NAME · STATUS · MBOX · MEM ·
MON; status colour-coded (running green, waiting grey); rows clip to width; `s`
cycles the sort key (id/mailbox/memory, all numeric → `sort-by`); selection tracks
the numeric `:id` (stable across re-sorts); the detail line shows the full snapshot.

**Tests/docs.** `tests/observe_test.blsp` 23/23 (pure `observe-frame` over the
richer maps — columns, status faces, sort, windowing — + an `:isolated` block
asserting `process-info` over live spawned processes: a parked receiver →
`:waiting` + mailbox depth, a monitored pid → `:monitored-by` ≥ 1, `(self)` alive).
Incl. `BROOD_GC_STRESS=1`. ADR-051, primitives (+`process-info`), roadmap. Suite
green apart from the unrelated pre-existing adversarial memory-cap (E0043) trio.

**Coordination.** Built entirely additively (new fns + one primitive) while the
kernel was edited in parallel; re-read each hot file (builtins/process/mailbox/
monitor/dist) immediately before touching it. The seam to the kernel work is three
accessors — `process_status` / `parent_of` / `process_mem` over the new `Process`
fields: once they land, `process-info` wires `:parent`/`:memory` and swaps the
inferred `:status` for the real enum (and the observer columns light up for free).
