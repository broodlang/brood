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
- ✅ **Quasiquote** — Clojure-style `` ` `` / `~` / `~@` (ADR-009); **auto-gensym
  `x#`** for opt-in non-capturing macro binders (ADR-066), the first half of macro
  hygiene ahead of namespaces (ADR-065)
- ✅ **Parameter grammar** — `required` + `&optional` (with defaults) + `& rest`,
  in the closure calling convention (`fn`/`lambda`/`defn` all share it).
  `&key` (named args) is designed but **deferred for simplicity** (ADR-011) —
  additive when the editor command API needs it.
- ✅ **Native multi-arity dispatch** (ADR-047) — Clojure-style arg-count
  overloading: a closure holds one arm per arity clause, the call's arg count
  selects the arm, and arity-only arms bind params *directly* (no rest-list, no
  `match*`). Keeps the prelude's variadic `+`/`-`/`<`/`=` in Brood while making
  `(+ a b)` ~one env frame — `(sum-to 100000)` 497 MB → 61 MB (8.1×). Pattern
  clauses still lower to the `match*` engine; the two dispatch axes don't mix.
- ✅ **Math library** — `floor`/`ceil`/`round`/`quot`/`pow`/`sqrt`, `even?`/`odd?`,
  variadic `min`/`max`. All **Brood** except the single new primitive `floor`
  (the irreducible Float→Int crossing); `sqrt` is Newton's method.
- ✅ **Sequence library** — `range take drop take-while drop-while some? every?
  find zip partition sort sort-by` (all Brood; `sort` is a stable merge sort).
  Plus **vector indexing** (myedit-driven, 2026-05-31): polymorphic `assoc`/
  `update` over a vector + integer index, `remove-nth`, and a `subvec` slice — on
  two kernel primitives (`vector-assoc`/`subvec`); `index-where` (predicate index).
- ✅ **Dynamic variables** (`defdyn` / `binding`) for config-style knobs — Lisp
  special vars with restore-on-exit (even on throw); **per-process** (a `spawn`ed
  child starts from defaults, never inherits a binding). Brood macros over a tiny
  kernel (`%declare-dynamic`/`%binding`/`dynamic?`); the value resolves through a
  per-process binding stack consulted only at the global-lookup step (free when
  no `binding` is active). No new special form.
- ✅ **Error handling** — `throw` + `%try` primitives; `try`/`catch` + `error`
  in the prelude (no new special forms — ADR-011); `error-message` normalises any
  caught value (verbatim throw payload *or* the kernel `{:kind :message …}` map)
  to a human string (2026-05-31).
- ✅ **Pattern matching** (ADR-021) — Erlang/Elixir-style; one Brood compiler
  reused by `match`, refutable `let`, and `fn`/`defn` clauses. Subsumes Tier-2
  destructuring + `case`. Made fast by a **macroexpand-all compile pass**
  (ADR-022), which also lowers the `let`/`fn` pattern surfaces.
- ✅ **Set-theoretic, gradual types — Steps 0–4 done + Step 5 structured types** (ADR-023/024/078). Full
  plan and the *compatibility contract* future changes must honour in
  [`types.md`](types.md). Step 0: first-class `Tag` + `(type-of x)`,
  self-identifying type errors, `Arity` on every builtin (one central gate).
  Step 1: the `Ty` set-theoretic lattice (sets of tags; union/intersect/
  negate; subtyping = set inclusion). Step 2: `dynamic()` — the gradual type
  as a bounded `GradualTy` *inside* the lattice (globals are `dynamic()`,
  not `Any`). Step 3: typed primitive signatures — every `NativeFn` carries
  a `Sig` next to its `Arity` (compatibility-contract #6, enforced); the
  checker reads sigs from there, from a small curated stdlib table, and from
  one-step inference of straight-line single-expression closures. Step 4
  — the behavioural payoff — is **complete**: the disjointness walk; guard
  narrowing via `Ty::tested_by` (`if` narrows in both branches incl. a
  leading `(not …)`); arity and unbound-symbol diagnostics — on call *heads*
  and, in whole-file mode, *operand / value* positions (`(+ 1 typo)` /
  `(def x typo)`) — with file-local `defn` accumulation; auto-running at file
  boundaries (`brood <file>` / `brood --test` / `nest test` / `nest run`;
  `nest check [FILE…]` shares one Brood path that loads the project image first
  so single-file and whole-project checks resolve cross-namespace names alike;
  warnings to stderr, exit-non-zero for CI; `BROOD_NO_CHECK=1` is the uniform
  opt-out);
  let-stored guard aliases (`(let (g (int? x)) (if g …))` narrows `x`);
  **let-binding aliases + `%eq`-as-guard** that close `match` pattern
  narrowing (`(match x (5 (first x)))` now flags `first` on int — the
  pattern compiler's `(let (m x) (if (%eq m lit) …))` expansion flows the
  narrowing back to `x` via an undirected alias graph). `cond` / `and` /
  `or` chained guards all narrow through the existing guard pipeline. The
  Rust primitive `(check-file path)` exposes the file-level walk; the
  Brood `(check-project)` walks the project's `src/` + `tests/`.
  🟡 Step 5+: structured types (ADR-078). ✅ **Function arrows**: `Ty` is a
  refinement struct (`arrow`/`elem` *refine* the flat bitset, not replace it); the
  checker flags wrong-arity callbacks to `map`/`filter`/`reduce`/`fold` (`(map cons
  xs)`). ✅ **Element types**: `[1 2 3]`/`(list …)` carry `vector<int>`/`list<int>`,
  and `first`/`last`/`nth` flow the element type out, so `(+ 1 (first ["a" "b"]))` is
  flagged. ✅ **Parametric HOF results**: `(map inc [1 2 3]) : list<number>`, `filter`
  preserves the element, `(reduce + 0 xs) : number` — element types flow *through*
  `map`/`filter`/`reduce`/`fold` (per-HOF rules, no type variables). ⬜ Still:
  intersections for overloaded fns; user-generic type variables.
  Additive; gated on real need (ADR-011). Advisory throughout — never gates, never
  inhibits the dynamic language; not the TypeScript route.
- ✅ **Opt-in type annotations + runtime contracts** (ADR-082). `(sig name (… ->
  …))` declares a signature the advisory checker reads first — closing the
  multi-clause/branchy gap inference can't reach; `(sig! …)` *also* enforces it at
  run time (a same-arity wrapper checks args + result and throws — the opt-in
  "strong arrow", sound where you ask for it). All policy in Brood, never
  required, never gates. Plus soundness-oracle tests (results never
  under-approximate; correct programs never warn) and curated sigs for common
  predicates. `docs/type-annotations.md`. ⬜ Future: a `BROOD_CONTRACTS=1`
  enforce-every-`sig` switch; element-level `(list E)` runtime checks.
- ✅ **Maps** (ADR-030 + ADR-040) — immutable `{ }` literals + `get`/`assoc`/
  `dissoc`/`keys`/`vals`/`contains?`/`map?`. Structural-equality keys, order-
  independent `=`; every op returns a fresh map. Small `map-*` Rust kernel, the
  surface is Brood (`std/prelude.blsp`). Internal rep is a CHAMP hash trie
  (16-way, path-copying — ADR-040): O(log₁₆ N) lookup/assoc/dissoc, structural
  sharing keeps fold-build linear-amortised. One ADR-030 contract change:
  iteration order is hash-driven, not insertion order.
- ✅ **Tier-2 ergonomics** (per `ROADMAP.md`) — `letrec` for local mutual
  recursion (new special form alongside `let`/`let*`; plain-symbol targets;
  pre-bind to `nil` so all names are visible in every RHS), lenient `symbol`
  and `keyword` constructors over string/symbol/keyword input, strict
  `symbol->string` / `string->symbol` wrappers in Brood, and the side-effecting
  loop macros `dotimes` / `dolist` (lean tail-recursive Brood; `doseq` stays
  for the destructuring / `:when`-filter case).
- ✅ **Memory reclamation — automatic, at any eval depth.** A per-process
  **semi-space copying collector** (`Heap::collect` / `arena_flip`, sharing the
  bump-allocator's no-slot-reuse discipline so it can't resurrect the old
  mark-sweep scheduler race) reclaims LOCAL garbage automatically — nothing is
  asked of the program author (no `while`, no manual collect; the old
  `(hibernate)` primitive was **removed**).
  - **Stage B — automatic safepoint** (ADR-055): collection fires at the eval
    safepoint when the live set crosses an adaptive threshold. A generation epoch
    on every handle (ADR-054) trips a precise debug tripwire on any stale deref.
  - **Bounded loading** (ADR-058): `load`/`require`/`eval-string` run a file's
    forms rooted on the explicit stack, so every entry path inherits the bound.
  - **Collect at *any* eval depth** (ADR-061): the evaluator keeps its in-flight
    LOCAL transients on an **operand stack** (`roots` + `env_roots`), so a loop
    below the outermost eval — argument position, `try`-wrapped, deep — is bounded
    too (depth-2 leak repro 3.5 GB → 28 MB). The macro compile pass opts out via
    `MACRO_BLOCK` rather than being rooted. Supersedes the depth-1-only safepoint.
  - **Region-check rooting** (ADR-061 perf follow-up, 2026-05-30): the per-call
    operand-stack push now skips immovable handles (atoms, `PRELUDE`/`RUNTIME`),
    rooting only genuine LOCAL transients — recovered ~10–14% of the
    collect-at-any-depth overhead (token API in `core/heap.rs`: `is_movable` /
    `Root` / `root`/`read_root`/`advance_root`/`root_env`).
  - **`promote` cycle guard** (2026-05-30): `promote` grew a forwarding table +
    reserve-then-fill (`OnceLock`) for the cyclic-capable RUNTIME closure/env
    slabs, so promoting a self-referential or mutually-recursive local closure
    (`(let (g (fn () g)) g)`, `letrec`) terminates instead of a SIGSEGV.
  - **Generational young/old split** (ADR-072, 2026-05-30): the LOCAL heap is now
    a nursery + tenured old generation. A *minor* collection copies the nursery's
    survivors (tenuring them into old once the nursery crosses `min_tenure`, else a
    young semi-space flip) and never recopies the old generation; an occasional
    *major* compacts old. No write barrier (immutable data ⇒ no old→young edges)
    bar a one-site remembered set for a frame tenured mid-bind. On a stateful
    workload (a process holding ~20k live across heavy churn) this is ~8× faster
    and ~9× lower RSS than the single-space copy; copy volume ~70× less. Thresholds
    are tunable via `BROOD_GC_FLOOR` / `BROOD_GC_TENURE` / `BROOD_GC_MAJOR`.
  - **GC observability** (Tier-1): `(gc-stats)`, `(gc-collect)` (force a
    collection), `(gc-trace on?)` (per-collection stderr logging); `BROOD_GC_TRACE`
    traces a whole run.
  - Validated by `crates/lisp/tests/gc.rs` (tail loops, server loops, depth-≥2
    loops, root and spawned, cyclic-promote cross-process, gc-stats/gc-collect/
    gc-trace) and the `BROOD_GC_STRESS=1` + `debug-assertions` tripwire. See
    `memory-model.md`, `memory-review.md`, `handoff-vm-gc-memory.md`.
  - ⬜ **RUNTIME-region collector** (ADR-072 Stage 5, *deferred*) — the per-process
    LOCAL heap is collected; the **shared mutable RUNTIME code region** (where
    `def`/hot-reload `promote`s code) is never reclaimed, so it grows with
    hot-reload churn. Doesn't matter for short runs; matters for a long-lived,
    live-edited server. Design not started.
  - ✅ **Rooted-Rust `eval` re-entry — done / nothing left** (re-examined 2026-05-31).
    Quasiquote moved off the runtime walker to a compile/eval-time transform
    (ADR-084), the worst offender. The remaining frames are already safe: the
    `macroexpand` *fixpoint* roots its `env` (collects at any depth), the
    compile-pass walk suppresses GC via `MACRO_BLOCK` (bounded per form), and
    `reload-defs` mirrors the rooted `eval_str` loop. macroexpand can't be a
    transform-not-walker (running a macro *is* eval re-entry), so there's no
    quasiquote-style hazard left to shrink.
  - ⬜ **RUNTIME-region collector** (ADR-072 "Stage 5 later half", *deferred*) — the
    one genuinely-open GC item: the shared mutable RUNTIME code region (where
    `def`/hot-reload `promote`s code) is never reclaimed, so it grows with
    hot-reload churn. Matters for a long-lived, live-edited server; design not
    started.
- ✅ **Self-hosted REPL in Brood** (ADR-048) — the read-eval-print loop is now
  `std/repl.blsp`, not Rust: a tail-recursive loop over `read-line` (the one new
  primitive) + `eval-string` + `pr-str`, with multi-line balance detection,
  structured-error rendering, and tty-gated prompts all in Brood. `brood` (no
  args) and `nest repl` bootstrap into `(repl-run)`; the old `crates/repl` +
  `rustyline` are gone. The per-process GC (ADR-035) reclaims each command's
  allocations, so there's no Rust heap-reset left.
- ✅ **Interactive REPL editor in Brood** (ADR-052) — `std/lineedit.blsp` +
  `std/highlight.blsp`: a raw-mode, emacs/readline-style line editor with live
  tree-sitter-style lexical **syntax highlighting**, **bracket matching**,
  function **signature hints**, **Tab completion**, and the core emacs keys
  (C-a/C-e, C-f/C-b, M-f/M-b, C-k/C-u/C-w, M-d, C-y, C-t, C-h, C-l, Home/End, ↑/↓
  or C-p/C-n history, **C-r reverse search**) — all written in Brood over a thin new
  inline `term-*` seam (`term-raw-enter` / `term-raw-leave` / `term-emit`, plus
  ALT/BackTab key encoding) and a rebindable keymap (`std/keymap.blsp`). On a TTY it
  replaces `read-line`; piped input keeps the plain path byte-for-byte. **Persistent
  history** (`~/.brood_history`) spans sessions, and `(special-forms)` keeps the
  highlighter in sync with the LSP. ⬜ Follow-ups: a scheduler-parking key read
  (makes the editor's `term-poll` block truly zero-cost — already benign, since it
  ties up only the REPL's own worker and yields every ≤250 ms), locals-in-scope
  completion, and real wide-char widths.
- ✅ **Modules** — Emacs-flat `provide` / `require` + `*load-path*` over the shared
  global table; `foo--private` convention (ADR-019). Logic in Brood; the only new
  Rust is `file-exists?` / `dir?` / `list-dir` / `cwd` / `name` / `eval-string` /
  `%builtin-module`.
- ✅ **Namespaces** (ADR-065/066/068, [`namespaces.md`](namespaces.md)) —
  **done** (substrate + imports + the big-bang + α + LSP ns-awareness; collision
  policy decided). Expand-time resolution over the flat table (no core namespace
  axis): `defmodule foo` *is* the namespace, qualifying definitions to `foo/name`
  (one interned symbol); a resolver pass (`eval/macros.rs`) qualifies free
  references (forward-ref pre-scan, binder-safe walk, earmuff `*foo*` stays
  ambient/root); current ns is per-process `Heap.compile_ns`. **Imports:**
  `(:use mod)` / `(:use mod :refer [a b])` refer a module's public names bare
  (own-ns defs shadow), auto-requiring (loads-but-never-fetches). **Soft** privacy
  (preserves ADR-013 hot reload). **Macro hygiene:** auto-gensym `x#` (ADR-066) +
  α auto-qualifying quasiquote. All of `std/` + the test suite migrated. **LSP is
  ns-aware** (§6): a shared resolution seam drives ns-correct goto/hover/signature,
  bare-import completion, and namespace-sound project references/rename.
  **Collision policy:** ADR-070 (flat names + detect-and-reject at lock time;
  enforcement with the package manager). Namespace-qualified workspace symbols,
  semantic-token ns coloring (a `NAMESPACE` token splitting `ns/name`), and
  namespace-sound cross-file shadow detection (`project--duplicate-def-warnings`,
  ADR-065) all landed — **namespaces are fully complete.**
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
- ✅ **Package manager** (ADR-037, [`packages.md`](packages.md)) — third-party
  Brood deps. Git-deps + project-local `_deps/` cache + `project.lock.blsp` for
  reproducibility; no registry, no semver solver, no install scripts. Policy in
  Brood (`std/package.blsp`); the only new Rust is `%git-clone` / `%git-resolve-ref`
  / `%rm-rf` / `%sha256` (`%http-get` deferred with tarball deps — no caller
  yet). `nest fetch`/`update`/`add`/`remove`/`tree`; existing `nest`
  subcommands auto-fetch missing deps. Designed early — before M2 — because the
  cache layout + manifest extension + auto-fetch behaviour cross-cut project
  management and the upcoming editor plugin story (ADR-006/011/019/020/028).
  Landed in vertical slices: ✅ **Slice 0** (2026-05-29) — manifest
  `:dependencies` parsing + `(project …)` as a quoting macro (bare-symbol dep
  names); ✅ **Slice 1** (2026-05-29) — `:path` deps end-to-end (`%sha256` +
  Brood tree-hashing, transitive resolution, `project.lock.blsp` I/O,
  `ensure-deps` on `*load-path*`; `std/package.blsp`); ✅ **Slice 2** (2026-05-30)
  — `:git` deps (`%git-resolve-ref`/`%git-clone`/`%rm-rf`, the `_deps/` cache +
  `.brood-pkg.blsp` stamp, lock commit-reuse on a cache hit, direct-beats-
  transitive conflicts); ✅ **Slice 3** (2026-05-30) — the
  `fetch`/`update`/`add`/`remove`/`tree` verbs + auto-fetch. **Deferred to v2**
  (ADR-011): registry, semver/solver, tarball+`%http-get`, signed packages.
  - **Forward-compat obligation (for native interop below):** keep the manifest
    and lock schema able to accept a `:native` sibling additively (as ADR-037
    already reserves `:branch`/`:dir`/`:features`). Costs nothing now; lets
    ADR-071 slot in without reshaping the package format later.
- 🟡 **`std/` = basic-language core; frameworks are packages; hierarchical module
  names** (ADR-085). `std/` has grown to ~38 modules, most of which aren't what a
  *normal language* ships — they're an editor/display **framework** (`buffer`,
  `display`, `face`, `highlight`, `keymap`, `layers`, `pane`, `ui`, `lineedit`,
  `ansi`), a net/web library + concurrency framework (`http`/`sse`/`tcp`,
  `hatch`/`supervisor`), and the project **toolchain** (`project`, `package`,
  `test`, `docs`, `reload`, `mcp`, `observer`, `repl`, `sexp`). Three coupled moves:
  ✅ **(1)** curate `std/` — the **in-tree reorganization is done** (2026-06-01):
  core stays bare in `std/` (`prelude` + `io`/`file`/`set`/`regex`/`json`/`fuzzy`/
  `format`/`task`/`log`); the **frameworks are namespaced** — `editor/*` (`ansi
  buffer display face highlight keymap layers lineedit pane ui`), `net/*`
  (`http sse tcp`), `proc/*` (`hatch supervisor`), files under
  `std/{editor,net,proc}/`; the **toolchain** (`test project package docs reload
  mcp observer proctree repl sexp`) is **grouped under `std/tool/` on disk but
  keeps bare module names** — the *internal* toolchain stays at root
  (namespaces.md §10), grouped without namespacing its identity (the embedded
  table keys it bare, pointing at the grouped file). 🟡 **(2)** ship the
  namespaced frameworks as **packages** — **the clean slice is done**
  (2026-06-01): `brood-net` (`net/tcp`/`http`/`sse`) and `brood-supervisor`
  (`proc/supervisor`) are removed from the binary and consumed as **internal
  packages** — a sibling `src/` on the load-path via `:source-paths`, *not* the
  package manager (no `:dependencies`/lock/fetch — that's for external/distributed
  deps, ADR-037); `brood-edit`/`brood-benchmark` point `:source-paths` at them.
  The walk found *most of the
  framework can't leave* — the bundled toolchain is built on it (`tool/observer` →
  editor display/face/highlight/keymap/lineedit/ui; `tool/repl` → editor/lineedit;
  `tool/sexp` → editor/buffer; core `log` → proc/hatch), and those must run in a
  fresh runtime with no deps fetched. So **only zero-bundled-dependent modules
  externalized**; `editor/*` + `proc/hatch` stay bundled (they're shared UI the
  toolchain consumes, not a detachable app framework — the editor *app* already
  lives outside the binary as `brood-edit`). ⬜ Remaining: the future **GUI
  framework** as a package, and repackaging the REPL/observer if `editor/*` is ever
  to leave — gated on a real consumer (ADR-011);
  ✅ **(3)** the enabling language change — **hierarchical module names** — is
  **done** (2026-06-01): `(require 'gui/window)` → namespace `gui/window` ←
  `gui/window.blsp`, amending ADR-019/065, defs qualifying on the **last** `/`
  (`gui/window/draw`). It was almost entirely already there — a qualified name is
  one interned symbol over the flat table, so `require--find` (path-joins the
  stem, nested dirs work), `qualify_name` (`{ns}/{name}`), the `%builtin-module`
  table (keys on the full stem), and the resolver's `contains('/')` guards are all
  separator-count-agnostic. The only fixes were the two sites that *split* a
  qualified name back apart: `semantic_tokens.rs` (`find`→`rfind`) and
  `unbound_namespace_hint` (allow multi-segment modules); covered by
  `tests/namespace_test.blsp`. ⬜ **Sequencing:** with hierarchical names landed,
  next is **(1)** curate `std/` + **(2)** lift frameworks into packages — gated on
  the first real consumer (the GUI framework, ADR-011). The GUI question that
  started this is answered structurally — a GUI framework is *one external
  package*, not a `std/gui/` subfolder.
- ⬜ **Native interop — WASM components, built on fetch** (ADR-071,
  [`interop.md`](interop.md)) — how a package ships native code (from another
  ecosystem, or a perf-critical kernel) with **zero kernel recompilation**. A
  package declares a `:native` WASM component; the package manager **builds it
  from source at fetch time** (the Rustler / `mix deps.compile` model — the
  *package's* artifact, never the runtime binary) or fetches a prebuilt one;
  it's hash-pinned in the lock and cached under `_deps/`. The runtime
  instantiates it **sandboxed** via an embedded `wasmtime` host, and a
  `use-native` macro (the `use Rustler` analog, driven by a **WIT** interface)
  binds its exports as namespace functions. The boundary **marshals** (`Message`
  enum / blob heap — never raw handles, the moving GC forbids it); a WASM
  instance is mutable state, so it's an **opaque resource handle**, never a
  `Value`; long calls run on the offload pool (deliver-to-mailbox). **Sequencing:**
  *after* the package manager — the packaging half is a strict extension of
  ADR-037 Slices 1–2 (lock + cache + git fetch). The **runtime half** (embed
  `wasmtime`, `%wasm-*` primitives, the marshalling layer) is independent and can
  be prototyped earlier from a local `.wasm`, but it has its own prereq — the
  **Phase-3 blocking offload pool** (`handoff-blocking-io.md`, M4). **Demand-
  driven (ADR-011):** pulled in by the first real native-needing package, which
  realistically lands during **M2+** editor-plugin work (regex engine, codec,
  highlighter) — so the package manager precedes it comfortably.
- 🟡 **Editor tooling & documentation** — source-position errors (GNU
  `FILE:LINE:COL:`) + structured test output (`docs/tooling.md`); a lossless,
  span-carrying CST and the introspection primitives `doc`/`arglist`/
  `global-names`/`bound?` (ADR-025); docstrings on functions/macros and on
  modules (a file's leading string), extracted to Markdown by `nest doc`
  (ADR-029). 🟡 The `brood-lsp` language server (`docs/lsp.md`): ✅ Tier 0 —
  the `crates/lsp` binary with stdio lifecycle, full document sync, and
  syntactic `publishDiagnostics` off the CST; ✅ Tier 1 (complete) — completion
  (locals + globals), hover, `documentSymbol`, goto-definition (pulled forward
  off Foundation B's scope walker), and signature help; ✅ Tier 2 (cross-file
  refs/rename, document-highlight, semantic tokens, completion resolve, located
  checker diagnostics) + **cross-file navigation as an image query** — def sites
  recorded at load time + `(source-location 'foo)` resolving `Free` names against
  the running image (ADR-031), not a static workspace index; ✅ a
  **developer-ergonomics pass** on top — `textDocument/formatting` (delegated to
  the Brood `std/format.blsp` formatter), `workspace/symbol`, code actions
  (did-you-mean for unbound symbols; **remove-unused-`require`**, 2026-05-31),
  folding ranges, and inlay hints (param-name at call sites). ⬜ Still next:
  incremental sync; range/delta semantic tokens; **finer checker-finding spans**
  (arity/type findings anchor to the call head, not the offending argument —
  wants `Pos` threaded through `types/check.rs`'s walk, a focused refactor of
  that GC-rooting-sensitive pass); and the **create-missing-`defn`** code action.

> v0.1 is the ✅ slice above: enough to be a real, usable language. The ⬜ items
> complete M1.
>
> **Overarching principle:** as much of the system as possible is written in
> Brood itself — Rust is mechanism, Brood is policy. Every Rust builtin is a
> candidate to later replace with Brood. This holds for the CLI, the editor
> commands, keymaps, and UI as the language grows capable enough.

### Type system — what full Elixir parity would take (reference, not a target)

Brood's types follow the **Elixir set-theoretic model** (ADR-023/024/078/082) and
share its *foundation*: types as sets of values, semantic subtyping, union/
intersection/negation, function arrows, sequence element types, and occurrence
typing. But the **goal is deliberately different** — Brood's checker is *advisory*
(never gates, zero false positives, serves the live editor and hot reload), with
soundness available **on opt-in** via `(sig! …)` runtime contracts (the strong
arrow done with a runtime check, not static casts). Elixir's is a *sound, gating,
whole-program* checker. So this list is a **map of the distance to Elixir**, kept
for reference — **not a backlog we intend to burn down**. Each item is additive
and gated on a real consumer (ADR-011); a few we are consciously **not** pursuing.

What we already have on par: set-theoretic core, semantic subtyping, arrows +
element types (ADR-078), occurrence typing through `if`/`cond`/`match` guards,
opt-in `(sig …)`/`(sig! …)` annotations + contracts (ADR-082), a sig-gated
dead-clause lint, and soundness-oracle tests.

Gaps to parity (⬜ = not started; ✋ = deliberately not pursuing):

- ⬜ **Intersection of arrows** — input-dependent return types for multi-clause
  functions (`(int->int) and (bool->bool)`). The single biggest expressiveness
  gap; pulls in when overloaded/multi-clause typing has a real consumer.
- ⬜ **Singleton / literal types** (`:ok` vs `:error`, `5` as a type) — the basis
  for precise `case`/`match` **exhaustiveness** and redundancy checking.
- ⬜ **Map / record types** — key ⇒ value with `required`/`optional`, open maps,
  static `KeyError` elimination. Brood has one flat `map` tag today.
- ⬜ **Tuple / positional product types** (Brood has no tuple kind; vectors carry
  a single element type, not positional types).
- ⬜ **Type variables / parametric polymorphism** for user-defined generics
  (the curated HOFs use per-rule result types, not type variables).
- ⬜ **Full type inference / reconstruction** — Brood infers only one-step
  straight-line bodies + guard narrowing; Elixir does guard-driven + local
  inference across a function.
- ⬜ **Narrowing through non-variable expressions** (`is_integer(p.age)` refining
  `p`), and richer `(sig …)` type-exprs (rest/optional params, nested generics).
- ✋ **Pervasive static soundness / gating** — Elixir rejects ill-typed programs;
  Brood **won't** (it would fight hot reload + the never-gate principle). Brood's
  soundness is opt-in and runtime-backed (`sig!`), not static.
- ✋ **Wiring `dynamic()` / full gradual consistency into the checker** — kept as
  a foundation (`GradualTy`); only wire it in if a real gradual-*assignment*
  consumer appears. The advisory disjointness pass doesn't need it.
- ⬜ **Fast-follows on what's shipped:** a `BROOD_CONTRACTS=1` switch to enforce
  *every* `(sig …)` at run time; element-level `(list E)` / `(vector E)` contract
  checks; broadening the dead-clause lint beyond sig-typed params (needs the
  surface-vs-generated scoping noted in `docs/type-annotations.md`).

The deeper rationale (why advisory + editor-serving rather than Elixir's sound
gate) is in [`research/set-theoretic-types-in-brood.md`](research/set-theoretic-types-in-brood.md);
the as-built design in [`types.md`](types.md) + [`type-annotations.md`](type-annotations.md).

### Deferred ergonomic & tooling items (see [`deferred.md`](deferred.md))

Each entry has a design sketch, the trigger that should pull it back in, and
the workaround available today.

- 🟡 **First-class set type + `#{…}` literal** — the `(require 'set)` library
  (`std/set.blsp`, sets-over-maps: `set`/`conj`/`disj`/`union`/`intersection`/
  `difference`/`subset?`) shipped (ADR-060); the **kernel** piece — a `#{…}` reader
  literal, `#{…}` printing, and a distinct `set?`/`Tag::Set` — is still deferred,
  and picks up when "set of X" becomes a common pattern in M2+ editor code.
- ⬜ **Lazy sequences + `iterate`** — tail-recursive accumulator helpers
  cover the case today; picks up when an editor feature needs unbounded
  streams (animation frames, file lines, undo history).
- ✅ **MCP runtime-introspection tools** — landed 2026-05-31. The `processes`
  tool now returns full `(process-info pid)` maps (mailbox, **reductions**,
  memory, GC count, monitors) instead of bare pids — the observer's per-process
  view; plus new `process-info` (one process by numeric id) and `node`
  (runtime-wide stats: workers, peak concurrency, spawned, live count,
  memory, peers) tools. Plus the **project-scoped editing pair** `write`
  (create/overwrite a file) and `edit` (exact-string replace) — both sandboxed
  under `*project-root*` (absolute / `~` / `..` paths refused, lexically) and
  reloading+checking any `.blsp` they touch, so an agent writes code *through*
  nest mcp (the live image stays in sync with disk) rather than the raw
  filesystem. All pure Brood in `std/mcp.blsp` (ADR-006); catalogue is eighteen
  tools. ⬜ Still open: a *streaming*/progress-notification tier so an agent sees
  long-running tool output incrementally (the dispatcher is synchronous today);
  exposing GC/process *traces* (not just snapshots); and tightening the write
  sandbox against symlink escapes (a `canonicalize` primitive) if it matters.
- ✅ **MCP `nest mcp` worker-panic isolation** — landed 2026-05-29. A Rust
  panic in any tool-call code path is caught at the handler boundary
  (`call_tool`'s `panic::catch_unwind`), projected as a structured JSON-RPC
  error (`error.data.kind = "panic"`), and the server keeps serving.
  Worker-thread panics in the scheduler proper are not covered (revisit
  only if a real case surfaces).
- ✅ **Cross-module redefinition warning** — landed 2026-05-29
  (`docs/feedback-retro-game-of-life.md` §5.1). `nest run` / `nest test` parse
  each source file's top-level def-style forms (via `parse-source`'s CST) and
  warn when one name is defined in more than one file — the silent two-`main`
  shadow now surfaces. Advisory (stderr, never fatal), silenced project-wide by
  `BROOD_NO_CHECK=1`; a per-name `^:override` opt-out can follow if a real need
  appears.
- ⬜ **`nest format --changed`** — whole-tree `nest format` reformats files
  the current change didn't touch; add a git-aware narrower scope.
- ✅ **Standard PRNG + bitwise ops + discovery** — landed 2026-05-29
  (`docs/feedback-retro-game-of-life.md` §1/§4, ADR-050). Pure seedable
  randomness (`rng`/`rand-int`/`rand-float`/`shuffle`/`sample`, threaded seed)
  over new `bit-*` primitives; plus `apropos`/`all-globals`/`doc-search`
  in-language and as `nest mcp` tools.
- ✅ **Bounded run mode `nest run --for DURATION`** — landed 2026-05-29
  (`docs/feedback-retro-game-of-life.md` §5.4). Runs a loop/TUI for a bounded
  time then exits cleanly; the first-class `timeout Ns nest run`, and what makes
  the still-open §8 memory leak reproducible in CI.
- ✅ **One-off `nest run --main module/fn` entry override** — landed 2026-05-29
  (`docs/feedback-retro-game-of-life.md` §5.3). `--main module/fn` (or just
  `module`, defaulting the fn to `main`) overrides the manifest's `:main` for one
  run; `set-project-main`/`project--parse-main-spec` in `std/project.blsp`, warns
  when a FILE is also given.
- ✅ **Complete signature reference `nest doc --all`** — landed 2026-05-29
  (`docs/feedback-retro-game-of-life.md` round 2). Prints every public global in
  a fresh image (builtins + prelude) with signature + one-line summary, generated
  live so it never drifts — the fix for probing builtin names/signatures one at a
  time. Plus `concat` (variadic alias of `append`) and `std/ansi.blsp` (escape
  strings for simple terminal output) closing the last GoL ergonomic gaps.
- ✅ **Non-tail self-recursion lint** — landed 2026-05-29. The advisory checker
  warns when a function calls itself outside tail position (overflow footgun);
  flows through `nest check`, `check-file`, the LSP, and the `nest mcp`
  `check`/`load` tools. `crates/lisp/src/types/check/recursion.rs`.
- ✅ **check-on-load** — landed 2026-05-29. The `nest mcp` `load` tool returns
  `{:diagnostics :shadows}` so an agent sees type/arity/unbound/non-tail and
  flat-namespace-collision problems at load time, not at run.
- ✅ **Scaffold templates `nest new --template`** — landed 2026-05-29. `tui-loop`
  and `hatch` starters alongside the `default` main+hello pair.
- ✅ **Output ports + async/safe logger** — landed 2026-05-31 (ADR-083).
  `print`/`println` route through dynamic `*out*`/`*err*` ports (a port is a 1-arg
  string sink); `std/io.blsp` adds `process-port`/`fn-port` + `with-out`/`with-err`,
  so output can be redirected to a process that owns a buffer (`[:io-write s]`).
  `std/log.blsp` is a `hatch`-process logger — casts (async), one serialising
  writer (safe), `io`-port backends incl. `process-backend` (→ an editor's
  `*Messages*`). Wired into the default `nest new` scaffold.
- ✅ **Property-based testing `check-property`** — landed 2026-05-29. Seeded,
  deterministic, counterexample-shrinking-free but seed-reporting; built on the
  PRNG (`std/test.blsp`).
- ✅ **Central `kw` keyword-spelling module** — landed 2026-05-30
  (`core/keywords.rs`, devlog). One `pub const` per special-form / core-macro /
  marker spelling, killing the magic strings that were re-typed across the three
  registries (`eval::SPECIAL_SPELLINGS`, `walk::SPECIAL_HEAD`,
  `builtins::SPECIAL_FORMS`) plus `recursion`/`hygiene`/`macros`/`scope`/
  `introspect`/`check`/`guards`. **The hot-path sweep is now done (2026-05-31):**
  `syntax/reader.rs`, `eval/compile.rs`, `core/heap.rs`'s def-name matcher (now
  lock-free `symbol_is` instead of an allocating `symbol_name` match),
  `types/check/{walk,guards}.rs`, and `eval/mod.rs`'s `&`/`&optional` markers all
  reference `kw::*`; the `%eq` primitive (a macro-expansion contract, like the
  existing `%try`) gained `kw::EQ_PRIM`, wired through `builtins.rs` + the guard
  recognizer. (`core/value.rs`'s `Tag::name()` strings are deliberately *not*
  touched — they're type names, owned by `Tag::name()`, not special-form
  spellings.) A second domain-scoped module, **`process/keywords.rs`** (`pk`),
  centralizes the **process/dist message tags** — `:down`/`:EXIT`/`:nodedown`,
  the exit reasons `:normal`/`:kill`/`:killed`/`:error`/`:noproc`/`:noconnection`,
  `:nonode`, and the `process-info` status strings — the Rust↔Brood mailbox wire
  contract, previously re-typed across `process/{scheduler,monitor,links,mailbox}.rs`
  and `dist.rs`. Remaining future families (lower value, mostly one-off per site):
  the display-protocol op/face keywords in `builtins.rs` and the env-var names
  scattered across crates.
- 🟡 **Errors that teach (LLM-native)** — first instances landed 2026-05-30
  ([`llm-native.md`](llm-native.md), devlog): the unbound-symbol `(:use mod)`
  fix-it, the `:main` quote guard, and `foreign_construct_hint` (a construct from
  another Lisp — `set!`/`loop`/`atom`/`defprotocol`/… → the Brood way), surfaced
  on both the runtime error `:hint` and the advisory checker. **More to do:**
  reader-level hints for Clojure/Scheme syntax the lexer mis-parses (`(let ((a 1))
  …)`, `#{…}`/`#(…)`), the `brood.explain-error`/`brood.find-pattern` MCP tools
  (llm-native.md §1), an intent→idiom cookbook, and folding each new repeat
  mistake into the rule-of-three (skill line + teaching error/lint + regression
  test).
- ✅ **Closure-compiling VM** (ADR-076, [`bytecode-vm.md`](bytecode-vm.md)) — the
  execution-engine swap that closes the tree-walker's structural tax (ADR-069's
  deferred lexical addressing). **The VM is now the default engine** (`BROOD_VM=0`
  forces the tree-walker, kept ≥1 release). Stage 0–1 (mechanism + passthrough
  redirect), 2a (`let`/`letrec`), 2b (multi-arity), 2c (local-capturing closures —
  created *and* called on the VM, GC-rooted captured envs, body-handle cache key),
  source-position threading, the Stage-3 cutover, a **differential test harness**
  (`differential.rs` + `make test-both` — both engines, assert identical),
  **variadic-arm coverage** (`&rest` + nil-default `&optional`), **real-default
  `&optional`** (`4146419`), and **`match`/pattern-dispatch `fn`s** (`c27e9d7` — via
  compiling `quote` + vector/map literals, which unblocked `match*`'s no-match arm)
  are all done. ~1.6–2.3× on the hot path (pattern fib ~2×), no language change,
  full suite green under both engines.
  - **Keep the `BROOD_VM=0` tree-walker as the per-form fallback — *not* a
    retirement target** (re-examined 2026-05-31). PRELUDE-region closures already
    compile on the VM (`cache_key` keys `RUNTIME | PRELUDE`; ~1.9× on a `reduce`/`+`
    loop). The remaining deferrals are correct by design, not gaps: an **unexpanded
    forward-referenced macro** can't be compiled without expanding it, and a
    **movable-LOCAL (conased) body** has no stable cache key — both belong on the
    fallback. The only true gap is `def`/`quasiquote`/`binding` in a closure *body*
    (uncommon, low value). So the fallback stays; "retire the tree-walker" is a
    non-goal.
  - ⬜ **Bytecode lowering** — explicitly deferred until a profile shows node-
    dispatch dominating (ADR-076).

## M2 — Editor data model

The text-editing substance, exposed to Brood. Built as a thin end-to-end
**vertical slice** (TUI-first), not layer-complete — see `docs/devlog.md`
(2026-05-29) and ADR-045. Text is an **opaque immutable rope** owned by a
**buffer-as-process**; everything above the rope kernel is Brood.

- 🟡 **Rope substrate (Phase 0 — done, ADR-045).** `Value::Rope` over `ropey`
  (Arc-shared B-tree: O(1) clone, copy-on-write edits → immutable for free) + a
  10-primitive char-indexed kernel (`string->rope`/`rope->string`/`rope-length`/
  `rope-line-count`/`rope-insert`/`rope-delete`/`rope-slice`/`rope-line`/
  `rope-char->line`/`rope-line->char`); `rope?` predicate. Process-local (content
  crosses as a string). `tests/rope_test.blsp` 28/28 incl. GC-stress + a
  buffer-as-process preview. The efficient large-file edit engine is now in.
- 🟡 **Buffer model (Phase 1 — done).** `std/buffer.blsp` (`(require 'buffer)`):
  an **immutable buffer value** (a map over a rope) with pure point/mark/region
  ops + movement (`goto-char`/`forward-char`/`beginning-of-line`/`forward-line`
  column-preserving/…) + editing (`insert`/`delete-char`/`delete-backward-char`/
  `delete-region`) + file round-trip (`buffer-from-file`/`save-buffer`), plus a
  thin `spawn-buffer` **actor shell** that owns a buffer and replies only with
  *derived views* (the display-protocol seam appearing early). Opt-in, never in
  the prelude, **zero new kernel surface** — the editor *framework*, not the
  language (ADR-045). `tests/buffer_test.blsp` 28/28 incl. GC-stress + actor.
- ✅ Editing **commands** + **multiple buffers** + **selection/region** + **undo**
  — belong in the **editor app** (`~/src/whk/myedit`, a `nest` project that
  `(:use buffer)`s this framework), not here. The app is a `ui-run` client whose
  `update` dispatches keys through `std/keymap.blsp` (chords via `keymap-step`) to
  `model -> model` commands and whose pure `view` paints the buffer(s) + mode line
  + echo area. **All three M2 enablers are done (2026-05-30):** a buffer ring
  (`:buffers` + `:current`, C-x ←/→/b/k, `*Messages*` as a real buffer), region +
  kill ring (C-SPC/C-w/M-w/C-y, reverse-video highlight), per-buffer undo/redo
  (C-/, M-/), a minibuffer (switch-buffer / find-file with completion), word motion
  (M-f/M-b), and multi-line `eval-last-sexp` (C-x C-e). 45 pure tests. The
  **language-side** enablers landed in `std/buffer.blsp` — `undo`/`redo`
  (per-buffer history, ADR-075), `buffer-region-bounds`, `forward-word`/
  `backward-word` — plus the GUI `C-SPC` key fix in `crates/lisp/src/gui.rs`.
- 🟡 **Evaluate-the-Lisp-I'm-editing (done, 2026-05-30).** The C-x C-e family as
  editor framework: `with-out-str` (prelude — surfaces the kernel's process-scoped,
  now-stacked output capture to Brood) + `read-all` (kernel — all forms in a
  string, vs `read-string`'s first) under `std/eval-command.blsp` —
  `eval-last-sexp`/`eval-region`/`eval-buffer`, each `buffer -> message string`
  (value + captured output), editing nothing and never throwing. Chords made
  expressible (not hardcoded): `std/keymap.blsp` gains `keymap-step` (prefix-aware
  dispatch threading a pending prefix) + `keymap-bind` (define a chord as data);
  flat `keymap-dispatch` unchanged. No key is wired — bindings stay user-defined.
  `tests/{capture,eval_command,keymap}_test.blsp`. **Deferred next:** Emacs-style
  major/minor modes (how a buffer selects which keymaps are active).
- ✅ Buffers as first-class Brood values — a buffer *is* an immutable value.
- ✅ Per-process memory reclamation is solved for M2's needs by the **automatic
  semi-space copying collector** (ADR-055/058/061; see M1 "Memory reclamation") —
  it fires at the eval safepoint at any depth and bounds every entry path, so it's
  no longer carried forward to M2. (The ADR-035 in-place mark-sweep was never
  shipped — slot reuse reintroduced a scheduler race — and the `(hibernate)`
  Stage-A expedient was removed once automatic collection landed.)

## M3 — Display protocol + native local frontend

The seam that makes remoteability free later (see architecture.md).

- 🟡 **Serialisable display protocol (Phase 0 — done, ADR-046).** The render frame
  is **Brood data** — a vector of tagged ops (`[:clear]`, `[:text row col s]`,
  `[:text row col s face]`, `[:cursor row col]`; a face is `{:fg :bg :bold
  :reverse}`). `std/display.blsp` is the pure op vocabulary; the meaning is Lisp,
  so a remote/web frontend re-implements the identical ops over a socket later.
- 🟡 **Input events flowing back in (Phase 0 — done).** `term-poll` returns keys
  (1-char strings / specials as keywords) into the Brood loop. Mouse/resize events
  deferred until a feature needs them.
- 🟡 **Native in-process frontend (Phase 0 — done, terminal).** Five `term-*`
  primitives over `crossterm` paint the protocol + read keys; `term-draw` is a
  thin interpreter of the frame vector. A GPU-window frontend is a later additive
  path speaking the same protocol.
- 🟡 **Per-op + per-window font (done, ADR-079).** A `Face` carries an integer
  `:scale` (≥1): the GUI renderer draws that op's text `scale`× larger in a
  `scale`×`scale` cell block — the per-pane / per-buffer / big-heading font knob, on
  the existing uniform grid (terminal renders 1×). And `gui-font!` takes an optional
  window id (`(gui-font! id spec)`) so each window can run its own font, the no-id
  call staying the global default. (Closes GG-1, GG-2, GG-3 in `known-issues.md`;
  arbitrary per-px buffer sizing deferred.)
- 🟡 **First app on the seam: `nest observe` (done).** An Erlang-observer-style
  process viewer (`std/observer.blsp`) — proves the render protocol + key loop
  end-to-end with **no rope/buffer**. A node-stats panel (node name, workers/peak,
  spawn count, memory used/peak, peers) over a navigable process **table** — id ·
  name · status · mailbox · memory · monitors — from `(process-info pid)` (ADR-051,
  a kernel snapshot map). `↑`/`↓` select, `s` cycles the view (id / mailbox /
  memory / **reds** (live reductions/second rate) / **tree** — children indented
  under their parent), `space` pauses the
  live refresh, `q` quits; status is colour-coded (running/runnable/waiting), rows
  clip to width. The table also shows **REDS** (cumulative reductions) and
  **REDS/s** (the rate since the last refresh — diffed from a stamped `:at`
  against the prior snapshot, 2026-05-31); the rate is the at-a-glance "busy now"
  signal. Interactivity is a UI-state map threaded through the tail-recursive
  loop (no mutation); selection tracks the numeric pid **id** (stable across
  re-sorts). Pure `observe-frame` core (TTY-free, unit-tested) + a thin root-process
  IO loop. New primitives: `mailbox-size`, `process-info` — now full (`:status`
  enum running/runnable/waiting, `:parent`, `:memory` LOCAL footprint), all backed
  by registry-reachable `Mailbox` cells. `tests/observe_test.blsp` 29/29 incl. GC-stress + an
  `:isolated` live-process block.
- 🟡 **Observe a *running* runtime — inline + remote (done, ADR-053).** The observer
  loop takes a pluggable **data source** + a snapshot shape (`{:node :procs}`), so
  it's source-agnostic. `observe-attach` uses the local source (a running program
  inspects its *own* processes, modal). **Remote attach** is the same loop with a
  remote source: the target `(observe-serve)`s a registered agent that ships
  snapshots over the dist node link to `nest observe --connect name@host:port`
  (`--cookie`/`$BROOD_COOKIE`) — the node panel shows the *peer's* stats, a dropped
  link freezes on the last snapshot with a `DISCONNECTED` banner. No kernel changes
  (`process-info` maps are send-able); dev-grade auth (shared cookie, LAN/trusted).
  Cross-node `crates/cli/tests/observe_attach.rs`.
- ✅ **Resilient `ui-run` — recover to the last good frame (let-it-crash for the
  TEA loop)** (done 2026-06-01). A `view`/`update` throw in `std/editor/ui.blsp` no
  longer kills the app: `ui--loop` threads a **`last-good`** model, catches a throw
  from `view` (rolls the model back to `last-good` and re-renders it) or from
  `update` (drops that one bad input, keeps the current model), and **logs it to
  stderr** (`ui--log-error` via `eprintln`/`*err*` — the echo-area message vanishes
  on quit, leaving no trace otherwise) before looping on. `last-good` starts nil, so
  the *first* render throwing (no good frame to fall back to) still re-raises —
  surfacing a genuine startup bug instead of spinning; the outer `try` still runs
  `:leave` (restores the terminal) and re-raises frontend-mechanism
  (`:size`/`:draw`/`:poll`) errors. The editor's application of the
  **userland-supervisor / let-it-crash** philosophy (M4,
  [`supervision.md`](supervision.md)) at the render loop rather than the process
  tree, in the framework so every `ui-run` client (the observer too) inherits it —
  myedit's own `ed-view`/`ed-update` try/catch workaround is now redundant. The
  deliberate non-goal held: **buffers stay immutable values, not processes** — the
  recovery unit is the *model snapshot*, which immutability makes free; process-ifying
  buffers would forfeit O(1) undo/snapshot/sharing for mutable identity nobody wants.
  `tests/ui_test.blsp` (new `describe`): view-rollback, update-drop, fatal-first-render
  + `:leave`-still-runs, stderr logging.
- ⬜ Keymaps and interactive commands defined in Brood — belong in the **editor
  app** (a new `nest` project), not the framework.
- ⬜ Minibuffer / status line / multiple windows — editor-app concerns, additive
  on the same protocol.

## M4 — Server / daemon mode

- ✅ **Distributed nodes (slices 1 + 2 + closure-shipping + monitors + auth
  done)** — two runtimes connect over TCP and message each other:
  node-tagged pids (`Value::Pid`), location-transparent `send`,
  symbols-by-name wire codec, connection de-dup + tie-break, node-down
  detection, **distributed pid monitors** (`(monitor remote-pid)` shares the
  local `MONITORS` table via a `Watcher::Remote` variant; `:noconnection`
  fires on net-split), **closure-as-data shipping** (ADR-033 — closures,
  `(remote-spawn …)`, source positions all cross the wire),
  **auto-reconnect** (`(ensure-link …)` — Brood policy over
  `connect`/`monitor-node`), **deliberate teardown** (`(disconnect name)` —
  Erlang's `disconnect_node`: drop one peer link and fire `[:nodedown]` on both
  sides without exiting the process), and **handshake v2** (magic+version
  prefix, HMAC-SHA256 challenge–response; cookie never on the wire). ADR-033/034,
  [`distribution.md`](distribution.md). Remaining: supervision trees (true
  `link` / restart strategies) and optional TLS — both additive over what's
  here.
- ✅ **Node-connect ergonomics (ADR-068,
  [`node-connect.md`](node-connect.md)).** The Emacs `--daemon`/`emacsclient`
  model for the local case: a node is addressed by **name** over a Unix-domain
  socket (`(node-start :foo)` / `(connect "foo")` — no port), with TCP
  (`name@host:port`) still there for remote. One `Stream { Tcp | Unix }` seam,
  one handshake over both — "the frontend is a protocol, same code path,
  different transports". A per-user shared cookie (`~/.config/brood/cookie`,
  auto-generated, `0600`) replaces hand-invented secrets, and `nest run --name`
  brings a node up from the CLI. Policy in Brood (prelude), mechanism in Rust
  (`%node-listen`/`%node-connect`/`random-token`/`spit-private`). Deferred:
  **dual-listen** (one node on Unix + TCP at once — the editor-daemon end-state).
- ❌ **Kernel-supervised processes** (ADR-039,
  [`supervision.md`](supervision.md)) — **tried and reverted (2026-05-29,
  commit `e3d3a0d`).** Shipped as opt-in on 2026-05-28; stripped a day later
  because the kernel-side supervisor (RESUME_SLOT + safepoint rooting + the
  retry loop) was the bulk of the multi-thread scheduler race surface. The
  Phase-1 bump-only allocator (`f90f0de`, 2026-05-29) is the follow-on that
  brings the `recurse.blsp` repro from ~95% failure under `-j 0` to 10/10
  clean in debug-assertions release. **Userland supervision is still
  possible** — `spawn` + `monitor` give you `[:down …]` and a respawn
  pattern in ~10 lines of Brood (see [`supervision.md`](supervision.md)).
  Named-spawn is **not** delivered (was bundled with this); `defonce` stays in
  the prelude — no longer a transitional shim but the blessed state-survival
  tool ([ADR-042](decisions.md), since named-spawn would not have covered the
  global-state-cell case anyway). The editor will be written against
  let-it-crash + userland supervisors instead.
- ✅ **Userland supervisor library** (ADR-044, `std/supervisor.blsp`) — the
  structured form of that respawn pattern, require-able: `start-supervisor` over
  child specs (`:start` thunk + `:permanent`/`:transient`/`:temporary` restart
  type), restart-intensity limits, `which-children`. Pure Brood over
  `spawn`/`monitor`/`receive`/`exit`, zero new kernel surface. **All three
  strategies now ship** — `:one-for-one`, `:one-for-all`, `:rest-for-one` — over
  the `(exit pid :kill)` primitive (ADR-063): the group strategies hard-kill the
  healthy siblings they must restart and selectively drain each one's `[:down]`
  so a deliberate kill isn't mistaken for a crash. `stop-supervisor` and an
  intensity-exceeded shutdown terminate the children too (no orphans). A child
  spec's `:shutdown` (`:brutal-kill` default / `:infinity` / ms) makes **nested
  trees tear down depth-first** — a sub-supervisor child marked `:shutdown
  :infinity` cascades `[:$stop]` to its own children instead of orphaning them.
  And **process links + `trap_exit` (ADR-067)** close the structural gap: the
  supervisor `link`s + traps its children, so a supervisor's *own* crash/kill
  propagates down the links and tears the whole subtree down (no orphans even when
  the supervisor never runs cleanup). General Erlang primitives
  (`link`/`unlink`/`trap-exit`/`spawn-link`), not a supervision-specific hook. See
  [`supervision.md`](supervision.md) and [`concurrency-v2.md`](concurrency-v2.md) §4.
- ✅ **`std/task`** (myedit-driven, 2026-05-31) — run a thunk off the current
  process with an optional timeout + cancellation: `(task thunk opts)` returns a
  handle and delivers tagged `[:task-done handle v]` / `[:task-error handle msg]`
  / `[:task-timeout handle]` to `:reply-to`; `cancel-task` stops it early;
  `(await thunk ms)` is the synchronous run-with-timeout. Pure Brood over
  spawn/receive/exit (a worker + a coordinator whose pid is the handle), zero new
  kernel surface — the generic form of the editor's hand-rolled async-eval
  watchdog. Opt-in (`(require 'task)`).
- 🟡 **TCP sockets (the substrate, done — ADR-062).** Thin kernel primitives
  (`tcp-connect`/`tcp-listen`/`tcp-send`/`tcp-close`/`tcp-local-port`) over a
  reusable blocking-IO → mailbox seam (`process::spawn_io_source`, ADR-059):
  inbound data and connections arrive as `[:tcp …]` / `[:tcp-accept …]` mailbox
  messages, consumed with `receive` (no worker ever blocked). `std/tcp.blsp` adds
  `socket?` + `tcp-drain`.
- ✅ **TLS client / HTTPS (ADR-062).** `rustls 0.23` (pure-Rust, Mozilla roots via
  `webpki-roots`) backs a one-shot `(tls-request host port request)` primitive
  (`crate::net`): connect + handshake + write + stream the response back as the
  same `[:tcp …]`/`[:tcp-closed …]` mailbox messages a plain socket uses. `std/http.blsp`
  routes `https://` URLs through it, so `http-get`/`http-request` speak both
  transports. **Client-only:** rustls streams don't split read/write across
  threads like a raw fd, so accepting *inbound* TLS (server-side, for the daemon
  below) is still open. ✅ `tcp-controlling-process` (hand a passive accepted
  socket to a per-connection process). ⬜ Remaining follow-up: a `mio` reactor for
  scale.
- ✅ **Node names are `name@host`** (ADR-073) — Erlang short/long names: a bare
  name auto-qualifies (local: `(hostname)`; TCP: the listen address's host), and
  an explicit `:name@host` gives a long/FQDN name. Pids are now globally unique;
  `connect` returns the peer's authoritative name. Kernel adds only `(hostname)`;
  the rest is Brood policy in the prelude.
- ✅ **Synchronous `remote-spawn`** (`remote-spawn-sync`, ADR-067) — ships a thunk
  to a peer and returns the child's (node-tagged) pid via a ref-keyed reply, so a
  remote child is directly `monitor`/`link`-able.
**Node connect itself is complete** — two runtimes find each other, authenticate,
and talk (locally by name over a Unix socket, remotely over TCP) with globally
unique `name@host` identity, a shared cookie, monitors/links/supervision, and
code mobility. What remains under M4 is the **daemon/serving** layer built *on
top* of connect, plus a few deliberately-deferred refinements:

- ✅ **Dual-listen** (ADR-074) — one node serves several transports at once via
  `(node-also-listen [addr])`: a local Unix socket *and* a TCP endpoint, so it's
  reachable as `(connect "ed")` locally and `(connect "ed@host:port")` remotely —
  one identity, multiple front doors. The "one core, local + remote frontends"
  shape. Composable (opt-in), not forced on every TCP node. Server-side TLS as a
  third transport is still open (below).
- ⬜ **Server-side / inbound TLS — and per-frame channel integrity** (the
  headline network-security item; ADR-081). `rustls` is client-only (its streams
  don't split read/write across threads like a raw fd). Two gaps, both closed by
  an authenticated-encrypted channel (TLS, or a Noise-style session over the
  existing `Stream` seam):
  - **No confidentiality** — node-link frames travel in cleartext; an on-path
    observer reads every inter-node message (and shipped closure source).
  - **No per-frame integrity** — the HMAC cookie authenticates the *handshake*
    only; steady-state frames carry no MAC. On a TCP link an on-path attacker who
    lets the handshake complete can inject forged frames afterward — including a
    `Send` carrying a closure → RCE — *without knowing the cookie*.
  Confined to `dist/` (the transport seam); **does not touch the language
  kernel** (eval/heap/GC/value model unchanged). Fine for LAN/trusted and the
  Unix transport (0700 dir) today; **do not expose a TCP node on an untrusted
  network until this lands.** See ADR-081 for the decision to treat TLS-or-Noise
  as required (not "optional") for any network-facing node.
- ✅ **Pre-auth connection hardening (DoS) — done 2026-05-31 (ADR-081).** The
  inbound-handshake path is now bounded against an unauthenticated flood: a
  `HandshakeSlot` semaphore caps **concurrent in-flight handshakes**
  (`MAX_IN_FLIGHT_HANDSHAKES = 128`) — past it a connection is shed (socket
  closed, no thread spawned, no log) before any allocation — and the handshake
  reads use a tiny `MAX_HANDSHAKE_FRAME = 4 KiB` ceiling instead of the 64 MiB
  steady-state one, so an unauthenticated peer can't force a 64 MiB allocation
  off an 8-byte probe. Localized to `dist.rs`/`dist/handshake.rs`/`dist/wire.rs`;
  no kernel change.
- ⬜ The same runtime **listens on a socket and serves the M3 protocol** to
  attached frontends — the Emacs `--daemon` / `emacsclient` model; **one core,
  multiple attached frontends**. The `nest observe --connect` remote-attach is a
  vertical-slice proof; the general server mode (session lifecycle, multi-client)
  is the headline M4 deliverable.
- ⬜ **Deferred connect/dist refinements** (ADR-011): exact propagated exit reason
  for a *non-trapping* linked peer (the `hard` bit — reports `:kill` today); a
  `terminate/2` cleanup hook on hard kill; **long-name FQDN resolution** (today a
  long name is passed explicitly, no resolver); a `mio` reactor for socket scale;
  Windows Unix-socket transport. One-node-per-OS-process is a structural choice
  (the Erlang model), not a gap.
- ✅ **Cluster-join topology — full mesh, transitive (ADR-088).** Decided and
  built: connecting to one cluster member auto-connects you to every node it
  knows (Erlang's default). The handshake advertises each node's reachable
  address (authenticated in the MAC); a new peer triggers a `Frame::Peers`
  gossip broadcast; recipients dial the unknowns, and each new link re-gossips
  until the mesh closes. On by default; `BROOD_NO_MESH=1` reverts to
  point-to-point. The reported bug (A↔B + C↔B but A couldn't see C) is fixed —
  `cluster_mesh_connects_peers_transitively` in `crates/cli/tests/distribution.rs`.
  Deferred (ADR-011): auto-reconnect/re-heal after a transient drop (use
  `ensure-link`); mesh over an untrusted TCP network still waits on channel TLS
  (ADR-081), as point-to-point does.
- ✅ **Test hardening (done — 2026-05-30):** the end-to-end real-TCP
  `distribution.rs` tests no longer flake under `make test`'s max parallel load.
  Root cause: under nextest each case runs in its own process, so the file's
  process-global `port_lock()` serialised nothing — all ~20 ran at once, racing
  `free_port()` and saturating every core, tripping a ~5s timeout. Fix: a nextest
  `real-tcp` test-group (`max-threads = 1`, `.config/nextest.toml`) runs them one
  at a time — the cross-process equivalent of `port_lock` — plus generous
  readiness/failsafe timeouts (5s→20s waits, 5s→30s receive failsafes). Full
  `make test` now green under load.

## M5 — Web frontend

- ⬜ Implement the display protocol over WebSocket
- ⬜ Browser renderer (DOM or canvas)

## Cross-cutting open questions (revisit, don't build yet)

- ✅ **How do we ship a binary?** **`nest release`** (ADR-038, 2026-05-31,
  [`release.md`](release.md)) — append-to-binary: a project's manifest + sources
  (+ resolved `_deps/`) are appended to a copy of the prebuilt `brood`, and that
  one executable boots `:main` with no interpreter, project dir, or sources on the
  target. `std/` is already baked into `brood` (the prelude + `EMBEDDED_MODULES`),
  so a release ships only the app's own code. v1 is **code-only** (no runtime
  asset FS) and Linux-first; cross-targets supply a prebuilt `brood` via
  `--runtime` (cross-compiling the runtime stays out of scope). Still open if a
  real consumer needs it: a self-extracting filesystem for runtime data files, a
  static-musl default, and `.deb`/`cargo install` packaging of the *runtime*.
- ⬜ **A tree-sitter grammar for Brood + GitHub language recognition.** Today
  `.gitattributes` maps `.blsp → linguist-language=Clojure linguist-detectable=false`
  (highlight as Clojure on GitHub, but keep it out of the repo's language stats) —
  a stopgap, since Brood is not Clojure (`defmodule`, `defdyn`, pattern forms, the
  list-code/vector-data split aren't Clojure). The real fix is a **tree-sitter
  grammar** (`tree-sitter-brood`), which is doubly useful: (a) it's the prerequisite
  for the editor's own syntax highlighting / structural editing (GitHub also uses
  tree-sitter for highlight + code-nav), and (b) it's required to register **Brood**
  as its own language with [`github/linguist`](https://github.com/github/linguist)
  (PR: a `languages.yml` entry + vendored grammar + `samples/Brood/`). **Blocker:**
  Linguist's contribution bar requires the extension to already be **in use across
  hundreds of unique repos** — gated on real adoption, not filable day-one. Path:
  write the grammar early (it serves the editor regardless and unlocks Neovim /
  Helix / Emacs / Zed highlighting before GitHub does), grow `.blsp` adoption, then
  file the Linguist PR. Until then the Clojure stopgap stands.

---

## Guiding principles

- **Keep policy in Brood, mechanism in Rust.** If something *can* live in the
  language instead of the runtime, it should — that's what stays editable at
  runtime.
- **The frontend is a protocol.** Local-native and remote-web are the same code
  path with different transports.
- **Every milestone is usable.** No "big bang" rewrites.
