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
  leading `(not …)`); arity and unbound-symbol diagnostics with file-local
  `defn` accumulation; auto-running at file boundaries (`brood <file>` /
  `brood --test` / `nest test` / `nest run` to stderr; `nest check` to
  stdout, exit-non-zero for CI; `BROOD_NO_CHECK=1` is the uniform opt-out);
  let-stored guard aliases (`(let (g (int? x)) (if g …))` narrows `x`);
  **let-binding aliases + `%eq`-as-guard** that close `match` pattern
  narrowing (`(match x (5 (first x)))` now flags `first` on int — the
  pattern compiler's `(let (m x) (if (%eq m lit) …))` expansion flows the
  narrowing back to `x` via an undirected alias graph). `cond` / `and` /
  `or` chained guards all narrow through the existing guard pipeline. The
  Rust primitive `(check-file path)` exposes the file-level walk; the
  Brood `(check-project)` walks the project's `src/` + `tests/`.
  ⬜ Step 5+: structured types — function arrows, vector/list element types,
  intersections for overloaded fns. Replaces the `u16`-bitset rep;
  additive; gated on real need (ADR-011). Advisory throughout — never
  gates, never inhibits the dynamic language; not the TypeScript route.
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
- 🟡 **Memory reclamation.** Two coexisting layers are live today, plus a
  disabled third:
  - **Arena reset at top-level boundaries** (ADR-016) — `eval_str`/the REPL
    `checkpoint` then `reset_local_to`, truncating the LOCAL heap after each form
    (demo: ~712 MB growing → ~78 MB flat). Bounds the REPL / file-runner.
  - **Bump-only allocator + arena flip on `(hibernate)`** — the LOCAL heap is now
    a pure bump allocator (`Slabs`: `Vec`s, **no slot reuse**), and a long-running
    loop reclaims by calling `(hibernate fn & args)`, which `Heap::flush`
    deep-copies the *live* graph into fresh slabs and drops the garbage. Bounds
    never-returning receive/spin loops. Validated by `crates/lisp/tests/gc.rs`
    (50k/100k-iteration loops, 5k-message server loops, root and spawned) — these
    are bounded by **flush**, not by mark-sweep.
  - **⛔ Disabled: per-process tracing mark-sweep GC** (ADR-035). `Heap::collect`
    is a **no-op**; the mark-sweep is kept as `collect_old` (`#[allow(dead_code)]`).
    It was switched off when the bump-only allocator landed (commit `6e92e8e`):
    mark-sweep reclaims only by **reusing freed slots**, and slot reuse
    reintroduced the stale-handle scheduler race the bump allocator was built to
    eliminate. The `GC_BLOCK == 1` safepoint + poison tripwire are still wired but
    inert. `BROOD_GC_STRESS=1` lowers the GC threshold but, since `collect` is a
    no-op, currently exercises nothing.
  - **Known gap (not host-unsafe):** a *long-lived* process that never hibernates
    (the in-language test **runner** across ~633 tests) accumulates unreachable
    garbage until it trips the test memory soft cap (clean `E0043`, not a crash).
    Planned fix: **hibernate the runner between batches** (the architecturally
    consistent path — no slot reuse), *not* re-enabling the race-prone mark-sweep.
  See `memory-model.md`.
- ✅ **Self-hosted REPL in Brood** (ADR-048) — the read-eval-print loop is now
  `std/repl.blsp`, not Rust: a tail-recursive loop over `read-line` (the one new
  primitive) + `eval-string` + `pr-str`, with multi-line balance detection,
  structured-error rendering, and tty-gated prompts all in Brood. `brood` (no
  args) and `nest repl` bootstrap into `(repl-run)`; the old `crates/repl` +
  `rustyline` are gone. The per-process GC (ADR-035) reclaims each command's
  allocations, so there's no Rust heap-reset left. Cooked-mode editing comes free
  from the terminal; ⬜ arrow-key **history/recall** is the next additive layer
  over the `term-*` raw-key seam (ADR-046) + the buffer framework.
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
- ⬜ **Package manager** (ADR-037, [`packages.md`](packages.md)) — third-party
  Brood deps. Git-deps + project-local `_deps/` cache + `project.lock.blsp` for
  reproducibility; no registry, no semver solver, no install scripts. Policy in
  Brood (`std/package.blsp`); the only new Rust is `%git-clone` / `%git-resolve-ref`
  / `%sha256-file` / `%http-get` (the last lands now for future tarball deps,
  used later). `nest fetch`/`update`/`add`/`remove`/`tree`; existing `nest`
  subcommands auto-fetch missing deps. Designed early — before M2 — because the
  cache layout + manifest extension + auto-fetch behaviour cross-cut project
  management and the upcoming editor plugin story (ADR-006/011/019/020/028).
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

### Deferred ergonomic & tooling items (see [`deferred.md`](deferred.md))

Each entry has a design sketch, the trigger that should pull it back in, and
the workaround available today.

- ⬜ **First-class set type + `#{…}` literal** — maps-as-sets work today;
  picks up when "set of X" becomes a common pattern in M2+ editor code.
- ⬜ **Lazy sequences + `iterate`** — tail-recursive accumulator helpers
  cover the case today; picks up when an editor feature needs unbounded
  streams (animation frames, file lines, undo history).
- ✅ **MCP `nest mcp` worker-panic isolation** — landed 2026-05-29. A Rust
  panic in any tool-call code path is caught at the handler boundary
  (`call_tool`'s `panic::catch_unwind`), projected as a structured JSON-RPC
  error (`error.data.kind = "panic"`), and the server keeps serving.
  Worker-thread panics in the scheduler proper are not covered (revisit
  only if a real case surfaces).
- ⬜ **Cross-module redefinition warning** — flat-namespace collisions are
  silent today; record per-name origin file and warn on cross-file
  redefinition (with an explicit `^:override` opt-out).
- ⬜ **`nest format --changed`** — whole-tree `nest format` reformats files
  the current change didn't touch; add a git-aware narrower scope.

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
- ⬜ Editing **commands** + multiple buffers — belong in the **editor app** (a
  new `nest` project that `(require 'buffer)`s this framework), not here.
- ✅ Buffers as first-class Brood values — a buffer *is* an immutable value.
- ✅ Per-process memory reclamation is solved for M2's needs by the **bump
  allocator + arena reset + `(hibernate)` flush** (see M1 "Memory reclamation") —
  so it's no longer carried forward to M2. The ADR-035 tracing **mark-sweep** is
  *not* what shipped: it's disabled (slot reuse reintroduced a scheduler race);
  `Heap::collect` is a no-op.

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
- 🟡 **First app on the seam: `nest observe` (done).** An Erlang-observer-style
  process viewer (`std/observe.blsp`) — proves the render protocol + key loop
  end-to-end with **no rope/buffer**. A node-stats panel (node name, workers/peak,
  spawn count, memory used/peak, peers) over a navigable, busiest-mailbox-first
  process list: `↑`/`↓` select, `space` pauses the live refresh, `q` quits.
  Interactivity is a UI-state map threaded through the tail-recursive loop (no
  mutation); selection is tracked **by pid**, so it stays on the same process as
  the list reorders. Pure `observe-frame` core (testable without a TTY) + a thin
  root-process IO loop; the only new primitive is `mailbox-size` (node stats reuse
  existing primitives). `tests/observe_test.blsp` 18/18 incl. GC-stress + an
  `:isolated` live-process block.
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
- 🟡 **Userland supervisor library** (ADR-044, `std/supervisor.blsp`) — the
  structured form of that respawn pattern, require-able: `start-supervisor` over
  child specs (`:start` thunk + `:permanent`/`:transient`/`:temporary` restart
  type), restart-intensity limits, `which-children`. Pure Brood over
  `spawn`/`monitor`/`receive`, zero kernel surface. ✅ `:one-for-one`;
  ⬜ `:one-for-all` / `:rest-for-one` deferred — they need a kernel kill/exit
  primitive (no links/`exit` yet), so userland can't terminate healthy siblings.
  See [`supervision.md`](supervision.md) and [`concurrency-v2.md`](concurrency-v2.md) §4.
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
