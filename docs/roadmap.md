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
- ✅ **Set-theoretic, gradual types — Steps 0–4 done** (ADR-023/024). Full
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
  flagged. ⬜ Still: intersections for overloaded fns; parametric `map` result.
  Additive; gated on real need (ADR-011). Advisory throughout — never gates, never
  inhibits the dynamic language; not the TypeScript route.
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
    gc-trace) and the `BROOD_GC_STRESS=1` + `debug-assertions` tripwire. **Still
    deferred:** `macros.rs` could be rooted if GC is ever wanted *during*
    expansion. See `memory-model.md`, `memory-review.md`.
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
  (did-you-mean for unbound symbols), folding ranges, and inlay hints (param-name
  at call sites). ⬜ Still next: incremental sync; range/delta semantic tokens;
  finer checker-finding spans.

> v0.1 is the ✅ slice above: enough to be a real, usable language. The ⬜ items
> complete M1.
>
> **Overarching principle:** as much of the system as possible is written in
> Brood itself — Rust is mechanism, Brood is policy. Every Rust builtin is a
> candidate to later replace with Brood. This holds for the CLI, the editor
> commands, keymaps, and UI as the language grows capable enough.

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
- ✅ **Property-based testing `check-property`** — landed 2026-05-29. Seeded,
  deterministic, counterexample-shrinking-free but seed-reporting; built on the
  PRNG (`std/test.blsp`).
- 🟡 **Central `kw` keyword-spelling module** — landed 2026-05-30
  (`core/keywords.rs`, devlog). One `pub const` per special-form / core-macro /
  marker spelling, killing the magic strings that were re-typed across the three
  registries (`eval::SPECIAL_SPELLINGS`, `walk::SPECIAL_HEAD`,
  `builtins::SPECIAL_FORMS`) plus `recursion`/`hygiene`/`macros`/`scope`/
  `introspect`/`check`/`guards`. **Remaining:** the core hot-path files
  (`core/value.rs`, `core/heap.rs`, `syntax/reader.rs`, `eval/compile.rs`) still
  re-type a few spellings — left for a deliberate pass when those files aren't
  being edited concurrently. Future families worth the same treatment: the
  type/tag-name strings (vs `Tag::name()`, but the checker's gradual-type
  vocabulary is a superset — design call) and process/dist message tags.
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
  (`differential.rs` + `make test-both` — both engines, assert identical), and
  **variadic-arm coverage** (`&rest` + nil-default `&optional`) are all done.
  ~1.6–2.3× on the hot path, no language change, full suite green under both
  engines. **Still worth doing (pure perf, deferrals already correct):** widening
  coverage to pattern/`match*` and prelude (PRELUDE-region) closures.

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
  memory / **tree** — children indented under their parent), `space` pauses the
  live refresh, `q` quits; status is colour-coded (running/runnable/waiting), rows
  clip to width. Interactivity is a UI-state map threaded through the tail-recursive
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
  `connect`/`monitor-node`), and **handshake v2** (magic+version prefix,
  HMAC-SHA256 challenge–response; cookie never on the wire). ADR-033/034,
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
- ⬜ **Server-side / inbound TLS** — `rustls` is client-only (its streams don't
  split read/write across threads like a raw fd). The cookie *authenticates* a
  link but doesn't *encrypt* it; remote attach over an untrusted network wants
  this. Fine for LAN/trusted today.
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

---

## Guiding principles

- **Keep policy in Brood, mechanism in Rust.** If something *can* live in the
  language instead of the runtime, it should — that's what stays editable at
  runtime.
- **The frontend is a protocol.** Local-native and remote-web are the same code
  path with different transports.
- **Every milestone is usable.** No "big bang" rewrites.
