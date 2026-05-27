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
