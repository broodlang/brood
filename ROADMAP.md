# ROADMAP — Stage 1: a full, functional Lisp

**Goal of Stage 1:** mylisp stands on its own as a *practical, general-purpose
dynamic Lisp* — you could write real programs in it without ever mentioning the
editor. (The editor, display protocol, server, and web frontend are Stage 2+ —
see [`docs/roadmap.md`](docs/roadmap.md) for the full M1–M5 arc. This file is the
detailed Stage-1 completeness checklist.)

Guiding constraints (see `CLAUDE.md`): keep the **language core small** — prefer
adding a primitive function or a prelude macro over a new special form — and
write as much as possible *in mylisp itself*. Tags below: **[kernel]** = needs
new Rust; **[mylisp]** = can be written in the prelude.

---

## Done

- ✅ Reader: lists, vectors, atoms, keywords, strings, `'`/`` ` ``/`~`/`~@`, comments
- ✅ Tree-walking evaluator with **proper tail calls**; lexical scope; closures; Lisp-1
- ✅ Special forms: `quote if when unless cond do def set! fn lambda let let* and or while quasiquote defmacro`
- ✅ **Macros**: `defmacro`, quasiquote, `macroexpand`/`macroexpand-1`, `gensym`
- ✅ Functions: `defn`, `&optional` (defaults), `& rest`; strict arity
- ✅ Numbers: i64 + f64, overflow-checked `+ - * /`, `mod`/`rem`, comparisons
- ✅ Lists/sequences: `cons first rest map filter reduce fold reverse append count nth last …`
- ✅ Vectors as a data type (`vector` / `vector-ref` / `vector-length`)
- ✅ Equality (`=`), truthiness, predicates
- ✅ Self-hosting: `eval`, `read-string`, `load`, `apply`
- ✅ **Error handling**: `throw` / `try` / `catch` / `error`
- ✅ REPL (line editing, history) + file runner

The native kernel is **39 primitives** — see [`docs/primitives.md`](docs/primitives.md).

---

## Remaining for a "full functional Lisp"

### Tier 1 — core gaps (needed before we'd call the language *complete*)

- ⬜ **Maps / associative data** — `{ }` literals + `get`/`assoc`/`dissoc`/`keys`/`vals`/`contains?`.
  Reserved in the reader but unimplemented; a general Lisp needs key→value data.
  **[kernel]** (a hash-map value + a few primitives; reader `{ }`).
- ⬜ **String library** — `substring`, `string-split`, `join`, `replace`,
  `index-of`, `upper`/`lower`, `string->number`/`number->string`,
  `char-at`/`string->list`/`list->string`. Today only `str`, `string-length`,
  `pr-str` exist. **[kernel]** for a few accessors (`substring`, char access),
  **[mylisp]** for the rest.
- ⬜ **Math library** — `floor ceil round sqrt pow`, `even?`/`odd?`,
  variadic `min`/`max`, `quot`. **[kernel]** for the float ops; **[mylisp]** for
  the rest.
- ⬜ **Sequence library** — `range take drop sort member some? every? map2/zip
  partition find` and friends. Mostly **[mylisp]** (sort needs care, e.g. a
  prelude merge sort).
- ⬜ **Dynamic variables** — `defdyn` / `binding` for config-style vars
  (`*print-depth*` etc.). **[kernel]** (a dynamic-binding store + 2 forms).

### Tier 2 — important ergonomics

- ⬜ **Destructuring** in `let`/`fn` — bind `(a b)` / `[a b]` from a sequence.
  Modern convenience; **[mylisp]** if `let`/`fn` gain a macro layer, else **[kernel]**.
- ⬜ **`case`** (dispatch on a value) and a few loop macros (`dotimes`, `dolist`). **[mylisp]**
- ⬜ **`letrec` / local mutual recursion** (today: use top-level `def`). **[kernel]** small.
- ⬜ **Symbol/keyword tools** — `symbol`, `keyword`, `name`, `symbol->string`,
  `string->symbol`. **[kernel]** small, helps metaprogramming.
- ⬜ **File I/O** — `slurp`/`spit` (read/write a whole file as a string), beyond
  `load`. **[kernel]** small.

### Tier 3 — robustness & quality

- ⬜ **Tracing GC** — replace `Rc` (`gc-arena`); the current model leaks reference
  cycles, which matters for a long-running REPL/editor. **[kernel]** (sizable).
- ⬜ **Source locations in errors** — the reader currently drops spans; attaching
  them gives line/column in messages (and later, stack traces). **[kernel]**
- ✅ **Native test library** — `std/test.lisp` (`deftest` / `is` / `assert=` /
  `run-tests`, written in mylisp). `tests/suite.lisp` uses it (52 assertions, 14
  tests, incl. concurrency); run via `./bin/cli std/test.lisp tests/suite.lisp`
  and by `cargo test`.
  Failures are reported and exit non-zero. **[mylisp]**

### Out of scope for Stage 1 (deferred, additive later)

- `&key` named arguments (designed — ADR-011), supplied-p flags
- Hygienic macros / `macroexpand-all`
- Bignums / rationals (i64 + f64 is enough for now)
- Modules / namespaces beyond the single global env
- Characters as a distinct type

---

## Parallel track — concurrency (green processes on all cores)

A major *core* effort that runs **alongside** the language work above — design in
[`docs/concurrency.md`](docs/concurrency.md). Erlang-*style* green processes
scheduled across all cores, share-nothing, message-passing; lean (no
supervision / preemption / live-migration in v1).

Strategy: start simple and let the language keep adopting features in parallel.
Language gaps above are mostly **[mylisp]**, so they don't deepen the evaluator
and don't conflict with the concurrency work. Concurrency lands in phases:

- ✅ `spawn` / `send` / `receive` / `self` + message passing (`process.rs`) — each
  process is an OS thread with its own heap; messages are copied between heaps
  (step 4a). Real parallelism + isolation.
- ✅ `Send` per-process heaps (done in step 2/3); global symbol interner
- ⬜ Green M:N on a small worker pool (default 2) via coroutine suspension — makes
  processes cheap (millions) and gives the core cap
- ⬜ **Shared code** (Erlang-style: share defs, isolate data) so spawned processes
  see all user functions and spawn is cheap (no per-process prelude reload)
- ⬜ **Send functions between processes** — once shared code lands: ship a
  closure's code (already a solved sub-problem) plus its captured free variables
  (closure serialization); global/native references resolve via shared code
- ⬜ later: reduction-counted preemption, then supervision / links

The Tier-3 **tracing GC** is shared with this track: `Send` per-process heaps are
what unlock full work-stealing, so concurrency pulls the GC work earlier.

## Suggested order

1. **Maps** (Tier 1) — unblocks structured data *and* a structured error value.
2. **Strings + Math** (Tier 1) — the two libraries every real program reaches for.
3. **Sequence library** (Tier 1, mostly mylisp) — cheap, high value.
4. **Dynamic variables** (Tier 1).
5. **Symbol/keyword tools, `case`, file I/O** (Tier 2) — quick wins.
6. **Tracing GC** (Tier 3) — do before long-lived editor sessions (Stage 2).
7. Destructuring, source locations, test helpers as they pull their weight.

When every Tier 1 box is ticked, mylisp is a Lisp you can write real programs in
— Stage 1 complete, and we turn to the editor.
