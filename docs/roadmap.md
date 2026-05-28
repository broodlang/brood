# Roadmap

The destination is a modern, Emacs-like editor written in Brood, runnable
locally as a fast native app and remotely as a server for other editor
instances. We get there in milestones. Each milestone is shippable and useful on
its own.

Legend: ✅ done · 🟡 in progress · ⬜ not started

---

## M1 — The language core

A solid, self-editable Lisp. This is the foundation everything else stands on.
The detailed Stage-1 completeness checklist ("what's left to be a full,
standalone Lisp") lives in the top-level [`ROADMAP.md`](../ROADMAP.md). A major
**parallel core track** — Erlang-style green-process concurrency across all
cores — is designed in [`concurrency.md`](concurrency.md) and tracked in
`ROADMAP.md`.

- ✅ Reader (text → values): numbers, strings, symbols, keywords, lists, vectors, `'` quote, comments
- ✅ Value model with interned symbols; cons-cell lists
- ✅ Lexical environments + closures
- ✅ Tree-walking evaluator with **proper tail calls**
- ✅ Special forms: `quote if when unless cond do def fn/lambda let/let* letrec and or` (immutable: no `set!`/`while`, loops are recursion — ADR-026)
- ✅ Builtins: arithmetic, comparison, lists/sequences, higher-order, predicates, strings/IO
- ✅ Self-hosting primitives: `eval`, `read-string`, `load`
- ✅ Prelude written in Brood
- ✅ REPL + file runner
- ✅ End-to-end test suite (incl. 100,000-deep tail recursion, live redefinition)
- ✅ **Primitive-kernel refactor**: `+ - * / < > = map reduce …` are defined in
  Brood (`std/prelude.blsp`) over a small Rust kernel (ADR-008)
- ✅ **Macros** (`defmacro`, `macroexpand`/`macroexpand-1`, `gensym`); `defn` and
  the `->`/`->>` threading macros are now defined *in Brood* (`std/prelude.blsp`)
- ✅ **Quasiquote** — Clojure-style `` ` `` / `~` / `~@` (ADR-009)
- ✅ **Parameter grammar** — `required` + `&optional` (with defaults) + `& rest`,
  in the closure calling convention (`fn`/`lambda`/`defn` all share it).
  `&key` (named args) is designed but **deferred for simplicity** (ADR-011) —
  additive when the editor command API needs it.
- ✅ **Math library** — `floor`/`ceil`/`round`/`quot`/`pow`/`sqrt`, `even?`/`odd?`,
  variadic `min`/`max`. All **Brood** except the single new primitive `floor`
  (the irreducible Float→Int crossing); `sqrt` is Newton's method.
- ✅ **Sequence library** — `range take drop take-while drop-while some? every?
  find zip partition sort sort-by` (all Brood; `sort` is a stable merge sort).
- ✅ **Dynamic variables** (`defdyn` / `binding`) for config-style knobs — Lisp
  special vars with restore-on-exit (even on throw); **per-process** (a `spawn`ed
  child starts from defaults, never inherits a binding). Brood macros over a tiny
  kernel (`%declare-dynamic`/`%binding`/`dynamic?`); the value resolves through a
  per-process binding stack consulted only at the global-lookup step (free when
  no `binding` is active). No new special form.
- ✅ **Error handling** — `throw` + `%try` primitives; `try`/`catch` + `error`
  in the prelude (no new special forms — ADR-011)
- ✅ **Pattern matching** (ADR-021) — Erlang/Elixir-style; one Brood compiler
  reused by `match`, refutable `let`, and `fn`/`defn` clauses. Subsumes Tier-2
  destructuring + `case`. Made fast by a **macroexpand-all compile pass**
  (ADR-022), which also lowers the `let`/`fn` pattern surfaces.
- 🟡 **Set-theoretic, gradual types** (ADR-023/024) — full plan and the
  *compatibility contract* future changes must honour live in
  [`types.md`](types.md). ✅ Step 0: first-class `Tag` + `(type-of x)`,
  self-identifying type errors, `Arity` on every builtin (one central gate).
  ✅ Step 1: the `Ty` set-theoretic lattice (`types.rs` — sets of tags;
  union/intersect/negate; subtyping = set inclusion). ✅ Step 2: `dynamic()` —
  the gradual type as a bounded `GradualTy` *inside* the lattice, consistent
  subtyping derived from set inclusion (globals are `dynamic()`, not `Any`).
  ✅ Step 3: typed primitive signatures — every `NativeFn` carries a `Sig`
  field next to its `Arity` (compatibility-contract #6, enforced); the checker
  reads sigs from there, from a small curated stdlib table (`+`/`<`/…/`map`/
  `reduce`), and from one-step inference of straight-line single-expression
  closures (`(defn inc (x) (+ x 1))` works without a hand-written sig).
  🟡 Step 4: advisory local inference over expanded forms — the disjointness
  walk is shipped (`brood --check <file>`, the `(check 'form)` builtin); guard
  narrowing via `Ty::tested_by` now lands too (a `Ctx` of locally-known types
  threaded through the walk; `let`/`let*` seeds `var : expr_ty(rhs)`, `if`
  narrows in both branches incl. a leading `(not …)`; inner shadowing
  overrides); plus **arity diagnostics** (every call's argument count vs the
  callee's `Arity` — primitives, curated stdlib, inferred closures) and
  **unbound-symbol diagnostics** (call heads; scope-aware over `fn`/`lambda`/
  `let`/`def`/`defn`/`defmacro`, with a `check_file` API accumulating
  file-local def names across forms); plus **auto-running** the checker in
  `brood <file>` / `brood --test` / `nest test` / `nest run` (advisory, to
  stderr) and the dedicated `nest check` (to stdout, exit non-zero on
  warnings — for CI). `BROOD_NO_CHECK=1` is the uniform opt-out across
  every entry point. The Rust primitive `(check-file path)` exposes the
  file-level walk to Brood; `(check-project)` in `std/project.blsp` walks
  the project's source + test paths. Remaining: cond-/match-/and-/or-chained
  guard narrowing (the macro-expanded `(let (g …) (if g …))` shape).
  ⬜ Step 5+: structured types. Steps 0–2 are foundation; Step 3 puts sigs on
  the kernel; the first *behavioural* payoff is Step 4. Advisory throughout —
  never gates, never inhibits the dynamic language; not the TypeScript route.
- ✅ **Maps** (ADR-030) — immutable `{ }` literals + `get`/`assoc`/`dissoc`/
  `keys`/`vals`/`contains?`/`map?`. Insertion-ordered, structural-equality keys,
  order-independent `=`; every op returns a fresh map. Small `map-*` Rust kernel,
  the surface is Brood (`std/prelude.blsp`). Internal rep is an association
  vector (swappable for a HAMT later, no surface change).
- ✅ **Tier-2 ergonomics** (per `ROADMAP.md`) — `letrec` for local mutual
  recursion (new special form alongside `let`/`let*`; plain-symbol targets;
  pre-bind to `nil` so all names are visible in every RHS), lenient `symbol`
  and `keyword` constructors over string/symbol/keyword input, strict
  `symbol->string` / `string->symbol` wrappers in Brood, and the side-effecting
  loop macros `dotimes` / `dolist` (lean tail-recursive Brood; `doseq` stays
  for the destructuring / `:when`-filter case).
- ✅ **Memory reclamation.** Done in two coexisting layers: **arena reset at
  top-level boundaries** (ADR-016) — `eval_str`/the REPL truncate the LOCAL
  heap after each form (demo: ~712 MB growing → ~78 MB flat) — and a
  **per-process tracing mark-sweep GC** (ADR-035) for the
  never-returning-loop case the reset can't reach. The GC fires only at the
  outermost-`eval` `'tail:` safepoint, gated by a thread-local `GC_BLOCK == 1`
  invariant that collapses the rooting surface to two sites (`eval_str` /
  `eval_source`), zero rooting in builtins. Validated by the full suite green
  under `BROOD_GC_STRESS=1` (GC at every safepoint) plus
  `crates/lisp/tests/gc.rs` (200k-iteration tail loops, 20k-message server
  loops, both root and spawned). See `memory-model.md`.
- 🟡 Nicer REPL — `rustyline` line editing (arrow keys, history, Emacs bindings)
  is in; richer completion/highlighting still to come
- ⬜ **Self-host the CLI/REPL in Brood** — once the language can express it, the
  read-eval-print loop should be Brood source on a thin Rust substrate, not
  Rust. (See the core principle in `CLAUDE.md`.)
- ✅ **Modules** — Emacs-flat `provide` / `require` + `*load-path*` over the shared
  global table; `foo--private` convention (ADR-019). Logic in Brood; the only new
  Rust is `file-exists?` / `dir?` / `list-dir` / `cwd` / `name` / `eval-string` /
  `%builtin-module`. *Namespaces stay deferred — a later, additive Brood macro layer.*
- ✅ **Project model & test tool** — convention over configuration: `src/` is the
  project source (auto on `*load-path*`), `tests/**/*_test.blsp` are the tests; a
  `project.blsp` manifest declares identity (name/version) and overrides paths only
  when needed. `nest test` discovers + loads (register-only) + runs once; `nest
  run [args…]` runs the entry point (configured by `:main`, defaults to module
  `main`, fn `main`; extra CLI args are passed in as strings); `nest new <name>`
  scaffolds a two-module project (`main` requires `hello`) via `spit`/`make-dir`;
  `nest format` (and `--check`) reformats every project `.blsp` in place, driven
  by an in-Brood CST walker (`std/format.blsp`) over a `parse-source` primitive.
  ADR-020/028.
- 🟡 **Editor tooling & documentation** — source-position errors (GNU
  `FILE:LINE:COL:`) + structured test output (`docs/tooling.md`); a lossless,
  span-carrying CST and the introspection primitives `doc`/`arglist`/
  `global-names`/`bound?` (ADR-025); docstrings on functions/macros and on
  modules (a file's leading string), extracted to Markdown by `nest doc`
  (ADR-029). 🟡 The `brood-lsp` language server (`docs/lsp.md`): ✅ Tier 0 —
  the `crates/lsp` binary with stdio lifecycle, full document sync, and
  syntactic `publishDiagnostics` off the CST; ✅ Tier 1 (complete) — completion
  (locals + globals), hover, `documentSymbol`, goto-definition (pulled forward
  off Foundation B's scope walker), and signature help; ⬜ Tier 2 (refs/rename,
  semantic tokens, located checker diagnostics) + **cross-file navigation as an
  image query** — record def sites at load time + `(source-location 'foo)`, then
  resolve `Free` names against the running image (ADR-031), not a static
  workspace index (all Tier-1 features are single-file today).

> v0.1 is the ✅ slice above: enough to be a real, usable language. The ⬜ items
> complete M1.
>
> **Overarching principle:** as much of the system as possible is written in
> Brood itself — Rust is mechanism, Brood is policy. Every Rust builtin is a
> candidate to later replace with Brood. This holds for the CLI, the editor
> commands, keymaps, and UI as the language grows capable enough.

## M2 — Editor data model

The text-editing substance, exposed to Brood.

- ⬜ Rope-backed buffers (`ropey`) — efficient edits on large files
- ⬜ Points, marks, regions; multiple buffers
- ⬜ Editing primitives as builtins: `insert`, `delete`, `goto`, `search`, …
- ⬜ Buffers as first-class Brood values
- ✅ The tracing GC migration landed in M1 (ADR-035) — no longer carried forward to M2.

## M3 — Display protocol + native local frontend

The seam that makes remoteability free later (see architecture.md).

- ⬜ A serialisable display protocol (render ops: lines, faces/styles, cursor, minibuffer)
- ⬜ Input events (keys) flowing back in
- ⬜ A native, in-process frontend (terminal via `crossterm`, or a GPU window) — the fast local path
- ⬜ Keymaps and interactive commands defined in Brood

## M4 — Server / daemon mode

- 🟡 **Distributed nodes (slice 1 done)** — two runtimes connect over TCP and
  message each other: node-tagged pids (`Value::Pid`), a cookie-authenticated
  handshake (`node-start`/`connect`), location-transparent `send`, and a
  symbols-by-name wire codec (ADR-034, [`distribution.md`](distribution.md)).
  Deferred: remote `spawn`/code shipping, distributed monitors, node-down
  detection, real auth.
- ⬜ The same runtime listens on a socket and serves the M3 protocol
- ⬜ Remote editor instances attach (the Emacs `--daemon` / `emacsclient` model)
- ⬜ One core, multiple attached frontends

## M5 — Web frontend

- ⬜ Implement the display protocol over WebSocket
- ⬜ Browser renderer (DOM or canvas)

---

## Guiding principles

- **Keep policy in Brood, mechanism in Rust.** If something *can* live in the
  language instead of the runtime, it should — that's what stays editable at
  runtime.
- **The frontend is a protocol.** Local-native and remote-web are the same code
  path with different transports.
- **Every milestone is usable.** No "big bang" rewrites.
