# ROADMAP — Stage 1: a full, functional Lisp

**Goal of Stage 1:** Brood stands on its own as a *practical, general-purpose
dynamic Lisp* — you could write real programs in it without ever mentioning the
editor. (The editor, display protocol, server, and web frontend are Stage 2+ —
see [`docs/roadmap.md`](docs/roadmap.md) for the full M1–M5 arc. This file is the
detailed Stage-1 completeness checklist.)

Guiding constraints (see `CLAUDE.md`): keep the **language core small** — prefer
adding a primitive function or a prelude macro over a new special form — and
write as much as possible *in Brood itself*. Tags below: **[kernel]** = needs
new Rust; **[Brood]** = can be written in the prelude.

---

## Done

- ✅ Reader: lists, vectors, atoms, keywords, strings, `'`/`` ` ``/`~`/`~@`, comments
- ✅ Tree-walking evaluator with **proper tail calls**; lexical scope; closures; Lisp-1
- ✅ Special forms: `quote if when unless cond do def fn lambda let let* and or quasiquote defmacro` (no `set!`/`while` — data is immutable, loops are recursion; ADR-026)
- ✅ **Macros**: `defmacro`, quasiquote, `macroexpand`/`macroexpand-1`, `gensym`
- ✅ Functions: `defn`, `&optional` (defaults), `& rest`; strict arity
- ✅ Numbers: i64 + f64, overflow-checked `+ - * /`, `mod`/`rem`, comparisons
- ✅ Lists/sequences: `cons first rest map filter reduce fold reverse append count nth last …`
- ✅ Vectors as a data type (`vector` / `vector-ref` / `vector-length`)
- ✅ Equality (`=`), truthiness, predicates
- ✅ Self-hosting: `eval`, `read-string`, `load`, `apply`
- ✅ **Error handling**: `throw` / `try` / `catch` / `error`
- ✅ REPL (line editing, history) + file runner

The native kernel is **70 primitives** — see [`docs/primitives.md`](docs/primitives.md).

---

## Remaining for a "full functional Lisp"

### Tier 1 — core gaps (needed before we'd call the language *complete*)

- ✅ **Maps / associative data** (ADR-030) — immutable `{ }` literals +
  `get`/`assoc`/`dissoc`/`keys`/`vals`/`contains?`/`map?`. Insertion-ordered,
  any value as a structurally-compared key, order-independent `=`; every op
  returns a fresh map. **[kernel]** a `Value::Map` + small `map-*` primitives +
  reader `{ }`; the surface is **[Brood]** (`std/prelude.blsp`). Internal rep is
  an association vector — swappable for a HAMT later with no surface change.
- ✅ **String library** — `substring`, `string-split`, `join`, `replace`,
  `index-of`, `string-contains?`, `upper`/`lower`, `string->number`/
  `number->string`, `char-at`/`string->list`/`list->string`, `trim`/`triml`/
  `trimr`, `blank?`. **[kernel]** only `upper`/`lower` (Unicode case folding) and
  `string->number` (strict parse-or-nil) genuinely need Rust; everything else is
  **[Brood]** over `substring`/`string-length`/`str` (`std/prelude.blsp`). Chars
  are 1-char strings (no distinct char type — deferred); indices are char-based.
- ✅ **Math library** — `floor ceil round quot pow sqrt`, `even?`/`odd?`,
  variadic `min`/`max`. **[kernel]** is just `floor` (the one irreducible
  Float→Int crossing); **everything else is [Brood]** over `floor`/`rem`/`/`/`*`
  (`ceil`=−floor(−x), `quot` exact over `rem`, `pow` recursive, `sqrt` Newton's).
  `floor`/`ceil`/`round` return an int; `sqrt` is an approximate float.
- ✅ **Sequence library** — `range take drop take-while drop-while sort sort-by
  some? every? find zip partition` (plus the existing `member?`). All **[Brood]**;
  `sort`/`sort-by` are a stable merge sort, every builder tail-recursive.
- ⬜ **Dynamic variables** — `defdyn` / `binding` for config-style vars
  (`*print-depth*` etc.). **[kernel]** (a dynamic-binding store + 2 forms).

### Tier 2 — important ergonomics

- ✅ **Pattern matching** (ADR-021) — Erlang/Elixir-style, one Brood compiler
  reused at every binding site (`match`, `let`, `fn`). Subsumes the two below:
  - ✅ **Destructuring** in `let`/`fn` — sequences/tuples, refutable binds,
    multi-clause `fn`, pattern params. **[Brood]**, lowered in the compile pass
    (ADR-022).
  - ✅ **`case`** — just `match` with literal patterns. Loop macros (`dotimes`,
    `dolist`) still ⬜. **[Brood]**
- ⬜ **`letrec` / local mutual recursion** (today: use top-level `def`). **[kernel]** small.
- ⬜ **Symbol/keyword tools** — `symbol`, `keyword`, `name`, `symbol->string`,
  `string->symbol`. **[kernel]** small, helps metaprogramming.
- ⬜ **File I/O** — `slurp`/`spit` (read/write a whole file as a string), beyond
  `load`. **[kernel]** small. (The module work below also adds the fs-reflection
  primitives `file-exists?` / `list-dir` / `cwd`.)
- ✅ **Modules** — Emacs-flat `provide`/`require` + `*load-path*`, `foo--private`
  convention; load-once by feature, embedded std modules baked in. **[kernel]**
  small (`file-exists?`/`dir?`/`list-dir`/`cwd`/`name`/`eval-string`/`%builtin-module`)
  + **[Brood]** (the require logic). ADR-019.
- ✅ **Project model + test runner** — convention over configuration (`src/` =
  source on `*load-path*`, `tests/**/*_test.blsp` = tests); a `project.blsp`
  manifest declares identity and overrides paths only when needed. `brood test`
  discovers, loads register-only, and calls `run-tests` once; `brood new <name>`
  scaffolds a project (`spit`/`make-dir`). Mostly **[Brood]** + CLI dispatch. ADR-020.

### Tier 3 — robustness & quality

- 🟡 **Memory reclamation** — `Send` arena handles replaced `Rc` (done). Step 1:
  **arena reset at top-level boundaries** (ADR-016) — `eval_str`/REPL truncate the
  LOCAL heap after each form, bounding long sessions (~712 MB→~78 MB in a demo).
  Still ⬜: a general tracing GC for mid-eval / never-returning loops, which needs
  scannable roots (the explicit-value-stack VM step 4b also needs — coupled).
  **[kernel]** (sizable).
- ⬜ **Source locations in errors** — the reader currently drops spans; attaching
  them gives line/column in messages (and later, stack traces). **[kernel]**
- ✅ **Native test library** — `std/test.blsp`: ExUnit / `mix test`-style
  `describe` / `test` (plus `deftest`), `is` / `assert=` / `assert-error` /
  `error-of` / `run-tests`, written in Brood. **Parallel by default** (each test a
  process), with `:serial` / `:isolated` opt-outs; **share-safe tallying** (no
  shared mutable counters — required now that processes share globals). Loaded via
  `(require 'test)` (embedded). `tests/suite.blsp` uses it; run via
  `./bin/cli tests/suite.blsp` and `cargo test`. ADR-015, `docs/testing.md`. **[Brood]**

### Out of scope for Stage 1 (deferred, additive later)

- `&key` named arguments (designed — ADR-011), supplied-p flags
- Hygienic macros / `macroexpand-all`
- Bignums / rationals (i64 + f64 is enough for now)
- **Namespaces** / per-module isolation — flat Emacs-style `provide`/`require` is
  *in scope* (Tier 2, ADR-019); true per-file namespaces stay deferred (a later,
  additive Brood macro layer if ever needed)
- Characters as a distinct type

---

## Parallel track — concurrency (green processes on all cores)

A major *core* effort that runs **alongside** the language work above — design in
[`docs/concurrency.md`](docs/concurrency.md). Erlang-*style* green processes
scheduled across all cores, share-nothing, message-passing; lean (no
supervision / preemption / live-migration in v1).

Strategy: start simple and let the language keep adopting features in parallel.
Language gaps above are mostly **[Brood]**, so they don't deepen the evaluator
and don't conflict with the concurrency work. Concurrency lands in phases:

- ✅ `spawn` / `send` / `receive` / `self` + message passing (`process.rs`) — each
  process is an OS thread with its own heap; messages are copied between heaps
  (step 4a). Real parallelism + isolation.
- ✅ `Send` per-process heaps (done in step 2/3); global symbol interner
- ✅ **Green M:N on a worker pool** via stackful coroutines (`corosensei`) — each
  process is a coroutine that **suspends** at `receive` (not blocks); a pool of
  ≈`nproc` worker threads (a setting, `-j` overrides) runs them. Spawn is cheap;
  OS threads bounded; the old `Gate` deadlock is gone. ADR-018, `docs/scheduler.md`.
- ✅ **Shared code** (Erlang-style: share defs, isolate data) — a runtime's inner
  processes share one mutable code region + global table (`Arc<RuntimeCode>`), so
  a `def` reaches a running spawned process on its next lookup (cross-process hot
  reload, no restart); separate runtimes stay independent. Spawn is cheap (no
  prelude reload). Region-tagged handles (LOCAL/PRELUDE/RUNTIME), append-only code
  via `boxcar`. ADR-013/014, `docs/shared-code.md`.
- 🟡 **Send functions between processes** — top-level functions are now shared
  handles (valid in any process), and `spawn` already ships a closure + its
  captured environment via `promote`. A `send`-able function value is the small
  remaining step; do it when a concrete need arises.
- ✅ **Reduction-counted preemption** (fairness) — `eval`'s loop decrements a
  per-worker budget (≈2000) and the process yields its worker at zero, so a
  CPU-bound process can't monopolise a core. Scheduling is now preemptively fair.
  ADR-027, `docs/scheduler.md` stage 4.
- ✅ **Selective `receive` + timeouts** — `receive` takes pattern clauses (the
  `match` grammar) + an optional `(after ms …)`; scans the mailbox, runs the first
  match, leaves the rest queued. Green processes are woken at the deadline by a
  timer thread; timeouts are catchable (`throw` in the `after` body → `try`/`catch`).
  A Brood macro over a `%receive` primitive. ADR-027, `docs/pattern-matching.md`.
- ⬜ later: work-stealing; supervision / links / monitors / registered names
- ⬜ **Distribution across nodes** (future, kept in mind) — link named runtimes
  over TCP; pids carry node identity; `send`/`spawn` stay location-transparent.
  Falls out of share-nothing + copy-on-send (the network is a longer copy). See
  `concurrency.md` → "Distribution across nodes".

The Tier-3 **tracing GC** is shared with this track: `Send` per-process heaps are
what unlock full work-stealing, so concurrency pulls the GC work earlier.

## Suggested order

1. ✅ **Maps** (Tier 1) — done (ADR-030); unblocks structured data *and* a
   structured error value.
2. **Strings + Math** (Tier 1) — the two libraries every real program reaches for.
3. **Sequence library** (Tier 1, mostly Brood) — cheap, high value.
4. **Dynamic variables** (Tier 1).
5. **Symbol/keyword tools, `case`, file I/O** (Tier 2) — quick wins.
6. **Tracing GC** (Tier 3) — do before long-lived editor sessions (Stage 2).
7. Destructuring, source locations, test helpers as they pull their weight.

When every Tier 1 box is ticked, Brood is a Lisp you can write real programs in
— Stage 1 complete, and we turn to the editor.
