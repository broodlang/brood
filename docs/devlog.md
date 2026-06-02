# Dev log

Chronological record of work sessions. Newest at the bottom.

## How to navigate

The session history is split so this file stays loadable:
- **This file** = the **complete digest** (every session, one line, by date) plus
  the **most recent day in full** at the bottom, where new entries get appended.
- **[devlog-archive.md](archive/devlog-archive.md)** = the **full verbatim text** of all
  older sessions.

You rarely read either top to bottom. For the *current* state of something, prefer
the topic doc (see [README.md](README.md)) or the relevant `## ADR-NNN` in
[decisions.md](decisions.md); use the log to recover the *why* and *how* of a change.
To read a session in full, find its `## YYYY-MM-DD — …` header in
[devlog-archive.md](archive/devlog-archive.md) (or in the "Recent" section below for the
latest day).

**Maintenance:** when the "Recent" section grows past a day or two, move its older
full entries to the bottom of devlog-archive.md (they already appear in the digest).
Append new full sessions to the end of "Recent".

**Major threads** (grep these across devlog-archive.md to follow an arc end to end):
- **GC / memory** — `GC`, `safepoint`, `use-after-GC`, `generational`, `promote`,
  `hibernate`, `tracing`, `copying` (ADR-016/035/043/054/055/058/061/072)
- **Execution engine / VM** — `VM`, `bytecode`, `closure-compiling`, `dispatch`,
  `lexical addressing` (ADR-047/057/069/076)
- **Scheduler / processes** — `scheduler`, `spawn`, `receive`, `preemption`,
  `links`, `trap_exit`, `exit pid` (ADR-018/027/059/063/067)
- **Distribution / nodes** — `node`, `distributed`, `dual-listen`, `node-connect`,
  `cookie`, `HMAC` (ADR-034/068/073/074/081)
- **Supervision** — `supervisor`, `hatch`, `monitor` (ADR-039 reverted/044/067)
- **Types / checker** — `types`, `checker`, `sig`, `structured`, `arrow`
  (ADR-023/024/078/082)
- **Namespaces / modules** — `namespace`, `defmodule`, `require`, `:use`
  (ADR-019/065/070/085)
- **Maps / data** — `CHAMP`, `maps`, `blob`, `transients`, `set` (ADR-030/040/041/060)
- **Packages / release** — `package manager`, `:git deps`, `nest release`,
  `bundling` (ADR-037/038)
- **Editor (M2/M3) / GUI** — `rope`, `buffer`, `display`, `observe`, `GUI`,
  `mouse`, `face`, `pane`, `keymap` (ADR-045/046/052/056/075/077/079/080)
- **Tooling (LSP/MCP/REPL)** — `LSP`, `MCP`, `REPL`, `format`, `nest`
  (ADR-025/028/036/048/052)

---

## Session digest (complete timeline)

Every session, oldest first. Full text: [devlog-archive.md](archive/devlog-archive.md)
(older) or the "Recent" section below (latest day).

- **2026-05-27** — Project bootstrap and v0.1 language core
- **2026-05-27** — Pattern matching + a macroexpand-all compile pass
- **2026-05-27** — Pattern matching: review fixes (eval fallback + `%eq` hygiene)
- **2026-05-27** — Rust simplification pass (shrink the core)
- **2026-05-27** — Split the CLI: `brood` (language) + `nest` (project tool)
- **2026-05-27** — Module docstrings + `nest doc` extraction
- **2026-05-27** — Immutability cleanup: lighter env frames + dedup
- **2026-05-27** — `brood-lsp`: the language server, Tier 0
- **2026-05-27** — Maps (immutable `{ }`)
- **2026-05-27** — String library
- **2026-05-27** — Maps: thorough review + concurrency tests
- **2026-05-27** — `(ref)` unique tokens + synchronous call/reply
- **2026-05-27** — Math + sequence libraries
- **2026-05-27** — Process monitors (supervision M0)
- **2026-05-27** — brood-lsp Tier 1: completion, hover, document symbols, goto-definition
- **2026-05-27** — `hatch`: a gen_server in Brood (supervision M1)
- **2026-05-27** — Kernel audit: drive Rust to the absolute minimum
- **2026-05-27** — brood-lsp: signature help completes Tier 1
- **2026-05-27** — `map-pairs`: one map enumerator; reduce-kv; docstring-on-pattern fix
- **2026-05-27** — Design: cross-file xref via the image, not a static index (ADR-031)
- **2026-05-27** — Dynamic variables (`defdyn` / `binding`)
- **2026-05-27** — source-location primitive + hover documentation (stdlib & primitives)
- **2026-05-28** — `(spawn expr)` and sendable closures (ADR-033)
- **2026-05-28** — Distributed nodes, slice 1: connect two runtimes (ADR-034)
- **2026-05-28** — receive loops are now TCO'd (coroutine-stack overflow fix)
- **2026-05-28** — Distributed nodes, slice 2: connection lifecycle + liveness
- **2026-05-28** — Per-process tracing GC (ADR-035)
- **2026-05-28** — Types Step 3: sigs on `NativeFn`; one-step closure inference
- **2026-05-28** — Types Step 4: guard narrowing + let-binding tracking
- **2026-05-28** — Types Step 4: arity + unbound-symbol diagnostics
- **2026-05-28** — Tier 2 ergonomics: letrec, symbol/keyword tools, dotimes/dolist
- **2026-05-28** — `nest run` and a two-module `nest new` skeleton
- **2026-05-28** — `nest format`: a Brood-driven code formatter
- **2026-05-28** — Source locations in errors + auto-running the checker
- **2026-05-28** — Auto-checker polish: macroexpand walk, scope fixes, sig fixes
- **2026-05-28** — Cross-node closure shipping (ADR-033 wire codec)
- **2026-05-28** — Distribution slice 3: finish the deferred list
- **2026-05-28** — Style: lists for code, vectors for data
- **2026-05-28** — MCP server design + introspect layer extracted
- **2026-05-28** — Types Step 4 finish: match pattern narrowing
- **2026-05-28** — MCP step 1b: widened `brood::introspect`
- **2026-05-28** — `file-mtime` + hot-reload example
- **2026-05-28** — Code review pass: monitor race fixes + doc tidy
- **2026-05-28** — Hot-reload: ergonomic surface (`std/reload`, `nest run --watch`)
- **2026-05-28** — MCP step 2: `nest mcp` dispatcher
- **2026-05-28** — Security/hardening review pass (Rust review + audit fixes)
- **2026-05-28** — MCP step 3: `std/mcp.blsp` lights up the dispatcher
- **2026-05-28** — MCP steps 4, 1c-{a,b,d}, 5: full v0 surface live
- **2026-05-28** — Package-manager design (ADR-037); bundler deferred (ADR-038)
- **2026-05-28** (continued) — Module splits: dist, types::check, process
- **2026-05-28** — LLM-native bundle: incarnations + new MCP resources + externalized prompt
- **2026-05-28** — Review pass + structured errors with codes (§4)
- **2026-05-29** — `brood` / `nest` CLI cleanup + clap + arity-change reload diagnostic
- **2026-05-29** — `nest repl` proper: new `crates/repl/` crate
- **2026-05-28** — Supervised-by-default processes (ADR-039); `defonce` removed
- **2026-05-28** — Polish round: `nest new .`, E0040 div-by-zero code, scheduler-race hint
- **2026-05-28** — `nest new` overwrites; `brood <nest-cmd>` points at nest
- **2026-05-28** — Specific runtime error codes (E0041–E0070) + a few more hints
- **2026-05-28** — Stdlib gap-fill: map + sequence ops; std/examples style sweep
- **2026-05-28** — LSP: cross-file & standard-library goto-definition
- **2026-05-28** — Supervised processes step 2: runtime supervisor + mode gate
- **2026-05-28** (cont.) — LSP Tier 2: references, rename, semantic tokens, polish
- **2026-05-28** (cont.) — MCP server: fix the stdio transport (was unusable by real clients)
- **2026-05-28** (cont.) — Supervisor follow-up: hot-reload + GC roots
- **2026-05-28** (cont.) — Cross-file references & rename (LSP) + the MCP `callers` tool
- **2026-05-28** — Std style review, codified conventions, `writing-brood` skill
- **2026-05-28** (cont.) — Review pass on the LSP + MCP code (shared core, bug fixes)
- **2026-05-28** (cont.) — Demo-friendliness: stdlib + docs gaps from `claude-demo-findings.md`
- **2026-05-29** — Maps: CHAMP trie (ADR-040)
- **2026-05-29** (cont.) — MCP DX feedback: the two trust-breakers
- **2026-05-29** — Test runner fails fast on a dead worker (KI-2 part 2)
- **2026-05-29** — Macro-hygiene lint (check-time capture warning)
- **2026-05-29** — `(format …)` printf-style helper (demo-DX item #5)
- **2026-05-29** — Kernel supervisor stripped (ADR-039 reverted)
- **2026-05-29** — Phase-1 bump-only allocator (race goes silent)
- **2026-05-29** (afternoon) — Race fully closed; suite-test segfault bisected
- **2026-05-29** (evening) — Phase 2: explicit `(hibernate)` primitive
- **2026-05-29** — Stdlib ergonomics (Game-of-Life feedback pass)
- **2026-05-29** (later) — MCP worker-panic isolation
- **2026-05-29** (late) — Shared blob heap (ADR-041): zero-copy send of large strings
- **2026-05-29** (later still) — Runaway-resource backstops (ADR-043) + live-editing hardening (ADR-042)
- **2026-05-29** (re-confirmation) — KI-1 scheduler race verified fixed; docs reconciled
- **2026-05-29** (concurrency-v2 track) — userland supervisor library (ADR-044)
- **2026-05-29** — M2 Phase 0: the text rope substrate (`Value::Rope`, ADR-045)
- **2026-05-29** — M2 Phase 1: the buffer framework (`std/buffer.blsp`)
- **2026-05-29** (concurrency-v2 track) — spawn-time load balancing; work-stealing ruled out
- **2026-05-29** — M3 Phase 0: the display/input seam + `nest observe` (ADR-046)
- **2026-05-29** — Runaway-resource safety (real this time) + native multi-arity dispatch (ADR-047)
- **2026-05-29** — Three language fixes surfaced by dogfooding the editor seam
- **2026-05-29** — std library review: `sleep` mailbox bug + dedup of clobbered globals
- **2026-05-29** — Self-hosted REPL: the read-eval-print loop moves into Brood (ADR-048/049)
- **2026-05-29** — Memory review + Stage A: hibernate the test runner
- **2026-05-29** — Game-of-Life feedback: bitwise ops, a standard PRNG, discovery tools
- **2026-05-29** — Richer process introspection: `(process-info pid)` + observer (ADR-051)
- **2026-05-29** — Interactive REPL editor: highlighting, brackets, hints, completion (ADR-052)
- **2026-05-29** — Remote attach: observe a running runtime over the node link (ADR-053)
- **2026-05-29** — Tooling round 2: check-on-load, scaffold templates, non-tail lint, property tests
- **2026-05-29** — process-info completed: `:status` enum + `:memory`, and an observer process-tree
- **2026-05-29** — Generational handles: a use-after-GC tripwire (ADR-054)
- **2026-05-29** — Game-of-Life retro round 2: kill the primitive-probing path
- **2026-05-29** — REPL editor cleanups: `(special-forms)`, persistent history, C-r (ADR-052)
- **2026-05-29** — std-library review: `let*` formatting fix + dedup simplification
- **2026-05-29** — Stage B: automatic copying GC at the eval safepoint (ADR-055)
- **2026-05-29** — GUI frontend: finish the observer's input (mouse, back-tab, docs) (ADR-056)
- **2026-05-29** — Observer: multiple GUI windows, `(require 'observer)`, GUI-only `(observe)` (ADR-056)
- **2026-05-29** — GC observability + the entry-depth memory leak (the "user must not care" fix)
- **2026-05-29** — Package manager, Slice 0: manifest `:dependencies` + the `project` macro (ADR-037)
- **2026-05-29** — Evaluator-dispatch campaign: Steps 0 + 1
- **2026-05-29** — Package manager, Slice 1: `:path` deps end-to-end (ADR-037)
- **2026-05-29** — Eval-dispatch Step 2 designed, measured, and rejected as scoped
- **2026-05-29** — Core memory guarantee: bound every entry path, remove `(hibernate)` (ADR-058)
- **2026-05-29** — Does lexical addressing help code safety? Audit of unbound-ref coverage
- **2026-05-29** — GUI input via the mailbox: blocking work never pins a worker (ADR-059)
- **2026-05-30** — Sets as a library over maps (ADR-060)
- **2026-05-30** — GC collects at any eval depth (ADR-061)
- **2026-05-30** — TCP sockets on a reusable blocking-IO seam (ADR-062)
- **2026-05-30** — `(exit pid reason)`: Erlang-style process termination (ADR-063)
- **2026-05-30** — `for` comprehension: fused-fold lowering (~3× faster)
- **2026-05-30** — Close out collect-at-any-depth: GC-safety sweep + debug tooling
- **2026-05-30** — TLS + an HTTP client: calling GitHub over `https` (ADR-062)
- **2026-05-30** — Shrink the GC-rooting surface: `macroexpand`→Brood + single-shot rule (ADR-064)
- **2026-05-30** — MCP tool watchdog + terminal-output isolation
- **2026-05-30** — Observer hot-reload: where `def` lands, and a live `:bg` theme (design note, not yet built)
- **2026-05-30** — Full kernel GC/memory-safety audit (review only)
- **2026-05-30** — Namespaces: design decided (substrate), implementation deferred
- **2026-05-30** — GC: region-check before rooting (collect-at-any-depth perf recovery)
- **2026-05-30** — `contains?` is O(1), not O(n)
- **2026-05-30** — LSP developer-ergonomics pass (formatting, workspace symbol, code actions, folding, inlay hints)
- **2026-05-30** — Auto-gensym (`x#`): macro binding hygiene, ahead of namespaces
- **2026-05-30** — Shared abstractions across the LSP and MCP servers
- **2026-05-30** — GC: promote cycle guard + memory-cap cleanup (v1 GC close-out)
- **2026-05-30** — Supervisor: `:one-for-all` + `:rest-for-one` (and no more orphans)
- **2026-05-30** — Namespaces: increment 1 (the resolution substrate)
- **2026-05-30** — Supervisor: `:shutdown` policy + nested-tree teardown cascade
- **2026-05-30** — Supervisor: OTP-parity quick wins (reverse-order shutdown + managed `:name`)
- **2026-05-30** — Namespaces increment 2: `(:use …)` imports + auto-require
- **2026-05-30** — Process links + `trap_exit` (ADR-067); supervisor crash no longer orphans
- **2026-05-30** — Supervisor: runtime child API (DynamicSupervisor), on top of links
- **2026-05-30** — Namespaces: import-aware checker + first std module migrated
- **2026-05-30** — Namespaces: the big-bang (unify `defmodule` = namespace, migrate everything, α)
- **2026-05-30** — Merge: links/trap_exit + DynamicSupervisor onto the namespaces+generational-GC trunk
- **2026-05-30** — Checker: operand-position unbound symbols + one unified `nest check` path
- **2026-05-30** — Distributed links + cross-node supervision; named/reload-stable supervisors
- **2026-05-30** — Namespaces finished: LSP ns-awareness (§6) + collision policy (ADR-070)
- **2026-05-30** — Namespace migration: `nest` tooling + imported-macro expansion
- **2026-05-30** — Generational GC, operator-call elision, reductions in the observer
- **2026-05-30** — Package manager Slices 2 & 3: `:git` deps + the `nest` verbs
- **2026-05-30** — Node-connect ergonomics (ADR-068)
- **2026-05-30** — Evaluator dispatch: cache the passthrough analysis + global inline cache (ADR-069)
- **2026-05-30** — Namespaces fully complete: ns-aware symbols/tokens + ns-sound shadow detection
- **2026-05-30** — Fix: eval deadline escaped the ADR-069 passthrough loop (MCP watchdog hang)
- **2026-05-30** — GC Tier-1 finish: `gc-collect`/`gc-trace`, tunable thresholds, doc reconciliation
- **2026-05-30** — Package namespace-collision check (ADR-070); rooting deferred
- **2026-05-30** — Node names `name@host` (ADR-073) + synchronous `remote-spawn`
- **2026-05-30** — The M2 editor app: a super-minimal GUI text editor
- **2026-05-30** — Robustness: a print never panics, an erroring TUI never wedges the shell
- **2026-05-30** — Dual-listen: one node, several transports (ADR-074)
- **2026-05-30** — `with-out-str`: output capture surfaced to Brood (editor step 1/3)
- **2026-05-30** — `with-out-str`: output capture surfaced to Brood (editor step 1/3)
- **2026-05-30** — `read-all` + `std/eval-command`: eval-the-Lisp-I'm-editing (editor step 2/3)
- **2026-05-30** — prefix-keymap (chord) support in `std/keymap` (editor step 3/3)
- **2026-05-30** — Buffer framework: undo/redo, region bounds, word motion (M2 enablers)
- **2026-05-30** — `%le` comparison fast-path, benchmark-safe builds, and the VM plan
- **2026-05-30** — Errors that teach (LLM-native, first two)
- **2026-05-30** — Bytecode VM Stage 0–1: built behind `BROOD_VM`, ~2× on fib/loop
- **2026-05-30** — Foreign-construct hints + a central `kw` keyword module
- **2026-05-30** — Bytecode VM Stage 2a/2b: `let`/`letrec` + multi-arity
- **2026-05-30** — `std/regex`: a small regex engine in Brood
- **2026-05-30** — GUI close button: a dedicated `:close` event
- **2026-05-30** — Mouse `:drag` + `:release` (ADR-077): the drag gesture the editor needs
- **2026-05-31** — std/window.blsp: the tiled-window layout toolkit (ADR-077, Part 1b)
- **2026-05-31** — Formatter: two comment-handling bugs (shared by nest format + the LSP)
- **2026-05-31** — Bytecode VM Stage 2c: local-capturing closures
- **2026-05-30** — Structured types, slice 1: function arrows (ADR-078)
- **2026-05-31** — Structured types, slice 2: vector/list element types (ADR-078)
- **2026-05-31** — VM source positions + `make install` ships the VM
- **2026-05-31** — Per-op font scale on the GUI `Face` (per-buffer fonts)
- **2026-05-31** — Cursor zones: resize pointer over window dividers (ADR-080)
- **2026-05-31** — VM is the default engine (ADR-076 Stage 3 cutover)
- **2026-05-31** — Mouse events carry held modifiers (Ctrl+wheel zoom)
- **2026-05-31** — VM differential harness + variadic-arm coverage
- **2026-05-31** — Parametric HOF results: element types flow through map/filter (ADR-078)
- **2026-05-31** — `register`/`whereis` sigs accept keyword names; editor per-pane zoom
- **2026-05-31** — Parametric results slice 2: reduce/fold (ADR-078)
- **2026-05-31** — `check-string-structured`: the checker over a source *string*
- **2026-05-31** — `std/window` → `std/pane` rename; myedit line-number gutter
- **2026-05-31** — Magic-string sweep: finish `kw`, add `process/keywords` (`pk`)
- **2026-05-31** — VM: defer unexpanded macro heads + compile prelude closures
- **2026-05-31** — `eval-command` moved out of std → the myedit project
- **2026-05-31** — Scope the scheduler-race hint to *bare* unbound names
- **2026-05-31** — RUNTIME collector: automatic safepoint trigger (2b-auto)
- **2026-05-31** — GC slab-OOB panic re-report: confirmed already-fixed + hardened
- **2026-05-31** — Scope the scheduler-race hint to *bare* unbound names
- **2026-05-31** — connect-test feedback triage: `substring` 2-arg + doc gaps
- **2026-05-31** — clean-disconnect `nodedown`: resolved (stale observation) + regression tests
- **2026-05-31** — `(disconnect name)`: deliberate node-link teardown
- **2026-05-31** — Language gaps surfaced by the myedit editor (vector indexing, error accessor, `task`)
- **2026-05-31** — Internal transients: fast bulk map building (Phase 1)
- **2026-05-31** — use-after-GC for string literals in compiled top-level forms (+ fallout fixes)
- **2026-05-31** — Security review of the language; pre-auth dist hardening
- **2026-05-31** — Type system: review vs the Elixir paper, soundness oracles, opt-in `(sig …)`/`(sig! …)` contracts
- **2026-05-31** — Close-out: closure-capturing-closure promote/send (GC's last hole) + http spawn-per-connection
- **2026-05-31** — `nest release`: ship a Brood app as one binary (ADR-038)
- **2026-05-31** (cont.) — lean release runtime + install-build fix
- **2026-05-31** — Output ports + an async, safe logger (ADR-083)
- **2026-05-31** (cont.) — `nest release` with no Rust + GUI in releases
- **2026-05-31** — Quasiquote → a compile/eval-time code transform (ADR-084); two-engine bench
- **2026-05-31** — Runtime visibility: MCP runtime tools, observer reductions/sec, LSP unused-require fix
- **2026-05-31** — VM coverage: real-default `&optional` (#6) + match/pattern-fns via quote + literals (#5)
- **2026-05-31** — Confirmed the `nest mcp` GC `flush_oob` was a stale binary + added a guardrail
- **2026-05-31** — HTTP streaming responses + SSE server framing (the push seam)
- **2026-05-31** — `std/highlight`: the shared span→runs fontify tiler
- **2026-05-31** — Decision: `std/` is the basic-language core; frameworks become packages (ADR-085)
- **2026-05-31** — std performance pass (sequence/map hot paths)
- **2026-06-01** — Hierarchical module names (ADR-085 Move 3)
- **2026-06-01** — std/ reorganization: frameworks namespaced, toolchain grouped-but-bare (ADR-085 Move 1)
- **2026-06-01** — ADR-085 Move 2 (clean slice): brood-net + brood-supervisor packages
- **2026-06-01** — Nodes form a transitive cluster mesh (ADR-088): connect to one, join all
- **2026-06-01** — Resilient `ui-run`: let-it-crash at the render loop (recover to the last good frame)
- **2026-06-01** — Node-link channel encryption (ADR-089): Noise-style X25519 + ChaCha20-Poly1305 session
- **2026-06-01** — M4 daemon/serving layer (ADR-090): serve a `ui-run` app to thin remote frontends (`nest attach`)
- **2026-06-01** — RUNTIME-region GC, Stage 1 (ADR-091): solidify the single-process collector — stats, gate test, un-stale docs
- **2026-06-01** — `nest grammar` (ADR-092): generate editor grammars (VS Code TextMate, Emacs) from `(special-forms)`; `brood-vscode` extension
- **2026-06-01** — `tree-sitter-brood`: a real parser grammar (external scanner mirrors the reader); `nest grammar tree-sitter` highlights
- **2026-06-02** — GUI key fix: re-apply Shift to Alt/Ctrl punctuation chords (`M->`/`M-<`/`M-{`/`M-%`/…), matching the crossterm frontend

---

## Recent — full entries

The latest day in full; older sessions' full text is in
[devlog-archive.md](archive/devlog-archive.md). Append new sessions below (newest last).

## 2026-06-01 — Hierarchical module names (ADR-085 Move 3)

**Goal.** Land the enabling language change of ADR-085: let a module name itself
contain `/` (`(defmodule gui/window)`), so the future GUI framework and the
`std/`-curation/lift-to-packages work have a namespace shape to land into. The
ADR sequences this *first* of the three moves; Moves 1 (curate `std/`) and 2
(lift frameworks into external packages) stay gated on the GUI consumer.

**Finding: it was ~90% already there.** Empirically (not by reading), a
hierarchical module already loaded, qualified, imported, and ran end to end —
`(require 'gui/window)` finds `gui/window.blsp` (`require--find` path-joins the
stem, so the nested dir Just Works), `(defmodule gui/window)` qualifies defs to
`gui/window/draw` (split on the **last** `/`, since `qualify_name` only formats
`{ns}/{name}` and doesn't care how many segments `ns` has), `(:use gui/window)`
refers names bare, and a value built by a hierarchical-ns fn round-trips through
a process. `nest check`/`run` on a scratch project were clean. The reason: a
qualified name is **one interned symbol over the flat table** (ADR-019/065), and
every "already qualified?" guard is `name.contains('/')` — separator-count-
agnostic. So no reader/resolver/loader change was needed.

The earlier worry that "the checker false-warns on hierarchical names" was a
**misread**: single-file `brood <file>` checking false-warns on *any* external
load-path module (flat `widget/paint` too), because the `require` hasn't run at
check time — it's a known single-file limitation, not hierarchical-specific, and
it doesn't fire under project-mode `nest check` (which loads the image first).

**The two real fixes** — both at sites that *split* a qualified name back into
module + name and wrongly assumed one separator:

- `crates/lsp/src/semantic_tokens.rs` — `name.find('/')` → `rfind('/')`, so a
  3-segment `gui/window/draw` colours the whole `gui/window` path as `NAMESPACE`
  and `draw` as the name (was: `gui` namespace, `window/draw` name).
- `crates/lisp/src/eval/mod.rs` `unbound_namespace_hint` — dropped the
  `!m.contains('/')` filter, so the "did you mean `(:use …)`" hint now suggests a
  hierarchical module (`add (:use gui/window)`) instead of silently skipping it.
  Verified: a bare `draw` whose only global is `gui/window/draw` now hints both
  the `(:use gui/window)` and the `gui/window/draw` qualified spellings.

**Tests.** A *hierarchical module names (ADR-085)* `:isolated` block in
`tests/namespace_test.blsp` (6 cases): last-`/` qualification, a 3-segment
module, bare same-ns resolution, explicit cross-module qualified reference,
`(:use gui/lib)` bare import, and a cross-process round-trip of a value built by
a hierarchical-ns fn. 24/24 in the file. (The block adds the same documented
`unbound symbol: ns/…` advisories the existing dynamic-`%load-string` fixtures
already produce — static analysis can't see a runtime-`%load-string`'d def;
advisory-only, suite green.)

**Docs.** `namespaces.md` §3 gains a *Hierarchical module names* subsection;
ADR-085 status updated (Move 3 done, Moves 1/2 still gated); roadmap M1 entry
flipped to 🟡 with Move 3 ✅.

**Not done (deliberately).** Moves 1 + 2 — they're a breaking reorg touching
`myedit`, gated on the GUI framework consumer (ADR-011). Hierarchical names now
unblock them.

## 2026-06-01 — std/ reorganization: frameworks namespaced, toolchain grouped-but-bare (ADR-085 Move 1)

**Goal.** With hierarchical names landed (Move 3, earlier today), do the in-tree
half of ADR-085 Moves 1+2: stop `std/` being a flat grab-bag of ~35 modules where
the editor/display framework, the net library, the concurrency framework, and the
internal toolchain all wear the same coat.

**As-built scheme.**
- **Core stays bare in `std/`** — `prelude io file set regex json fuzzy format task log`.
- **Frameworks namespaced** under `std/{editor,net,proc}/` — `editor/*` (ansi
  buffer display face highlight keymap layers lineedit pane ui), `net/*` (http sse
  tcp), `proc/*` (hatch supervisor). These are the things Move 2 will externalize
  into packages, so they get a namespace now: `(:use editor/buffer)`,
  `editor/buffer/insert`.
- **Toolchain grouped but NOT namespaced.** `test project package docs reload mcp
  observer proctree repl sexp` moved to `std/tool/` *on disk* but keep **bare
  module names**. This was a mid-flight correction: the first pass prefixed them
  `tool/`, but the *internal* toolchain stays at root (namespaces.md §10 — the
  ergonomic `describe`/`test`/`is` macros stay root), so every test file keeps
  `(:use test)`, not `(:use tool/test)`. The embedded `%builtin-module` table keys
  them bare (`"test"`) while `include_str!`-ing the grouped path
  (`std/tool/test.blsp`), so `require` maps the bare name to the grouped file.

**How.** A token-aware rewriter (not regex-on-text): it skips `;` comments and
`"`-strings, leaves `:keyword` face names (`:ui/header`, `:observer/detail`)
untouched — they're face-registry data, not module symbols — and rewrites only
bare module names in `defmodule`/`require`/`:use`/`provide` positions plus
non-keyword `mod/name` symbols. The two real hazards a blind pass would have hit:
`docs/foo.md` directory paths in comments (most `docs/` occurrences) and the
`:module/role` face keywords; both are handled by the skip rules. The Rust side
(binary bootstraps + the embedded table + a few test eval-strings) was updated to
match, comment-line-aware. `make install` refreshed the on-PATH `nest`/`brood-lsp`
the check-hook runs (the usual stale-binary gotcha).

**Result.** Full in-language suite (1287) + nest tests green. Files moved with
`git mv` (history preserved). Move 2 proper — lifting the namespaced frameworks
out of the binary into packages with `myedit` depending on them — stays deferred,
gated on the GUI consumer (ADR-011); this reorg is what it builds on.

## 2026-06-01 — ADR-085 Move 2 (clean slice): brood-net + brood-supervisor packages

**Goal.** Lift the namespaced frameworks out of the binary into packages
(ADR-037), starting Move 2.

**The constraint that shaped it.** A dependency walk of the bundled code (core +
toolchain) into the frameworks found that *most of the framework can't leave the
binary*: `tool/observer` (`nest observe`) is built on
`editor/{display,face,highlight,keymap,lineedit,ui}`, `tool/repl` on
`editor/lineedit`, `tool/sexp` on `editor/buffer`, and core `log` on `proc/hatch`.
Those are bundled features that must work in a fresh `brood`/`nest` with no
packages fetched — so the modules they need stay baked in. Only modules with
**zero bundled dependents** can externalize cleanly.

**Shipped (zero-dependent → externalized):**
- **`brood-net`** — `net/tcp`, `net/http`, `net/sse` → `~/src/broodlang/brood-net`
  (a `nest` project: `src/net/*` + the moved `tests/*_test.blsp` + the `webserver`
  example). Removed from `CORE_MODULES`. Built on the kernel `tcp-*` primitives +
  the bundled `file` core module. Consumers `brood-edit` (web frontend) +
  `brood-benchmark` (http bench) reach it as an **internal package** — its `src/`
  on the load-path via `:source-paths ["src" "../brood-net/src"]`, *not* the
  package manager (see below).
- **`brood-supervisor`** — `proc/supervisor` → its own package (+ its test).
  `proc/hatch` stays bundled (core `log` is a hatch process). The cross-node
  `supervisor_restarts_a_remote_child` test shipped `(require 'proc/supervisor)`
  into a *bare* runtime, so it was reworked to inline the equivalent userland
  `monitor`-respawn (start child → monitor → `[:down]` → restart) — same
  cross-node restart, no module dependency.

**Result.** `brood-net` 41/41, `brood-supervisor` 20/20, consumers green
(`brood-edit` 286, `brood-benchmark` 2), full brood suite green except the
pre-existing GC-WIP test.

**Internal packages skip the package manager** (a correction — the first cut
wrongly routed them through ADR-037 `:dependencies`/lock). An in-workspace
package isn't fetched, hashed, or locked; it's just a sibling `src/` on
`*load-path*`. A consumer adds it with `:source-paths` (`brood-edit`:
`["src" "../brood-net/src" "../brood-supervisor/src"]`), which `project-setup`
appends to the load-path for `run`/`test`/`check` alike — so `(require 'net/http)`
resolves with no `:dependencies`, no `project.lock.blsp`, no `_deps/`. The
package manager (git deps, lock, distribution) is only for packages *shared
across workspaces*.

**ADR-085 refinement (recorded in decisions.md).** The "editor framework" is
largely *shared UI the toolchain consumes*, not a detachable app framework, so
`editor/*` stays bundled until/unless the REPL + observer are themselves
repackaged — gated on a real consumer (ADR-011). The editor *app* already lives
outside the binary (`brood-edit`).

## 2026-06-01 — Nodes form a transitive cluster mesh (ADR-088)

**Reported bug.** With nodes A, B, C running and `A↔B` + `C↔B` established, **A
could not see C**. Investigation confirmed it was by-construction: links were
strictly point-to-point, the roadmap's "cluster-join topology" was an undecided
open question, and — more fundamentally — the wire carried only node *names*, no
reachable address, so B couldn't have told A *how to dial* C even if it wanted to.

**Decision (ADR-088): full mesh, Erlang-style.** Connecting to one cluster member
transitively connects you to every node it knows. On by default; `BROOD_NO_MESH=1`
keeps it point-to-point.

**Three pieces (all in `dist/`, no language-kernel change):**
1. *Advertise an address.* `Hello` (wire bumped v2→**v3**, magic `BRD\x03`) now
   carries the sender's dial address (first TCP listener else Unix socket), stored
   per-link in `Conn.addr`. It's **folded into the auth HMAC** (`compute_mac` gains
   `my_addr`), so a MitM can't redirect the gossiped address without the cookie.
2. *Gossip.* A genuinely-new peer triggers `broadcast_peer_table()` — a
   `Frame::Peers` list of `(name, addr)` to every connected peer (newcomer learns
   incumbents; incumbents learn newcomer). A reconnect/duplicate (`was_new == false`)
   doesn't broadcast, so the mesh goes quiet once closed.
3. *Dial unknowns.* `mesh_consider()` dials any gossiped peer not already in `NODES`,
   each on a short-lived thread; a `PENDING_DIALS` set dedupes concurrent gossip for
   the same name. Each new link re-gossips → transitive closure. Simultaneous
   cross-dials collapse via the existing connector tie-break.

**Convergence is order-independent:** whichever `establish` finishes its insert
last sees the full table and sends the cross-gossip, so the earlier node always
learns the later one regardless of interleaving (verified by reasoning + test).

**Robustness review.** No nested locks (NODES / PENDING_DIALS / LISTEN_ADDRS taken
sequentially, never held across each other or across the dial spawn); `PENDING_DIALS`
is cleared even if a dial thread panics (remove sits after the `catch_unwind`);
gossip frames capped at `MAX_GOSSIP_PEERS = 4096`; empty/self/known peers filtered;
the authoritative handshake name (not the gossip hint) keys the link, and the cookie
gate means a wrong dial is harmless.

**Tests.** `cluster_mesh_connects_peers_transitively` (the exact A/B/C repro — A
connects only to hub B, must end up seeing C) and `no_mesh_env_keeps_links_point_to_point`
(the kill switch). Wire round-trip + oversized-gossip-cap + MAC-binds-addr unit tests.
Full `make test` green.

**Deferred (ADR-011):** auto-reconnect/re-heal after a transient drop (`ensure-link`
covers persistent links); a global concurrent-dial cap; cross-machine routability
beyond what `name@host` assumes. Mesh over an untrusted TCP network still waits on
channel TLS (ADR-081), exactly as point-to-point does.

## 2026-06-01 — Resilient `ui-run`: let-it-crash at the render loop (M3)

The last open **framework-side** M3 item (the keymap/minibuffer bullets are
editor-*app* concerns, in `~/src/whk/myedit`). Before this, a `view`/`update`
throw in `std/editor/ui.blsp` ran `:leave` and **re-raised** — killing the app.
myedit worked around it by wrapping its own `ed-view`/`ed-update` in try/catch,
but that only stops the *process* dying: a guarded `view` keeps re-rendering the
**same bad model** every frame, so a model wedged into a throwing state shows
nothing but the error with no way back. Driver: a stale per-pane `:top` outliving
its buffer made `rope-line->char` throw out of myedit's renderer.

**The fix — the userland-supervisor / let-it-crash philosophy (M4) applied at the
render loop rather than the process tree**, in the framework so every `ui-run`
client (the observer too) inherits it:

- `ui--loop` now threads a **`last-good`** model alongside `model` — the last
  model that rendered cleanly.
- A throw from **`view`** is caught (`try [:frame (view …)]` → `:failed`), logged
  to stderr (`ui--log-error` via `eprintln`/`*err*`, so it survives the echo area
  vanishing on quit), and the loop **rolls the model back to `last-good`** and
  re-renders it. Since `last-good` is a model that rendered cleanly, the re-render
  can't loop — `view` is deterministic on the same model.
- A throw from **`update`** is caught and **drops that one input**, keeping the
  current (good) model — a single buggy command can't advance the model into a
  bad state.
- `last-good` starts **nil**: if the *first* render throws (no good frame to fall
  back to) the error **re-raises**, surfacing a genuine startup bug instead of
  spinning. The outer `ui-run` try still runs `:leave` (restores the terminal)
  before re-raising, and still re-raises frontend-mechanism (`:size`/`:draw`/
  `:poll`) errors — a dead terminal is a real teardown, not a recoverable wedge.

Draw/poll/size (frontend *mechanism*) stay outside the per-turn try; only the two
user-supplied pure fns (`view`/`update`) are guarded — exactly the surface the
roadmap named.

**Deliberate non-goal:** buffers stay **immutable values, not processes** — the
recovery unit is the *model snapshot*, which immutability makes free; process-ifying
buffers would forfeit O(1) undo/snapshot/sharing for mutable identity nobody wants.

**Tests** (`tests/ui_test.blsp`, new `describe`): a throwing `view` rolls back and
re-renders the last good frame (drained render-echo sequence `[:a :b :b :c]` — the
repeated `:b` proves recovery); a throwing `update` drops the bad input and the
model continues off the last good value (`[0 1 1 2]`); a first-render throw is
fatal and re-raised *and* `:leave` still runs; a recovered error is logged to
`*err*` (captured via `with-err`/`fn-port`). The scripted display feeds inputs as
`[:input …]` messages and `:poll` selectively receives those, so interleaved
`[:saw …]` render-echoes survive in the mailbox for `drain-saw`. 11/11 green;
observer (55) + display (7) — the other `ui-run` clients — unchanged.

## 2026-06-01 — Node-link channel encryption: a Noise-style session (ADR-089, M4)

Closed ADR-081's gap #1 — the headline network-security item. Steady-state
node-link frames were **cleartext with no per-frame MAC**: over TCP an on-path
attacker who let the cookie handshake complete could read every message *and*
inject a forged `Send` carrying a closure (→ RCE) without knowing the cookie. The
roadmap forbade exposing a TCP node on an untrusted network until this landed.

**Why a Noise-style session, not TLS.** A live link runs two independent threads
sharing an `Arc<Stream>` — a reader (`&Stream: Read`) and a writer (`&Stream:
Write`). A single `rustls` `Connection` can't be driven from both (shared mutable
crypto state). A **per-direction AEAD** maps exactly onto that split: the writer
owns the send cipher, the reader the receive cipher, neither sharing state. Node
identity is cookie/name-based (not PKI), so TLS would need self-signed certs pinned
via the cookie anyway. ADR-081 itself listed "a Noise-style session over the
existing `Stream` seam" as the equivalent option; chose it. (User confirmed the
TLS-vs-Noise fork up front.)

**The scheme** (`dist/session.rs` + `dist/handshake.rs`, wire v3→v4):
- **Ephemeral X25519 ECDH** per handshake → shared secret (forward secrecy: recorded
  traffic stays secret even if the cookie later leaks). Each side's fresh pubkey
  rides in its `Hello`.
- **Authenticated by the existing cookie-HMAC** — *both* ephemeral pubkeys folded
  into the `Auth` MAC (beside the names + addr, ADR-088), so a MitM can't substitute
  a DH key without the cookie (a swapped `Hello.eph_pub` fails the MAC).
- **HKDF-SHA256** (built on the in-tree `hmac`/`sha2` — no `hkdf` crate, sidestepping
  a sha2-version pin) over the DH secret, salted by `init_nonce ‖ resp_nonce`, → two
  directional keys.
- **ChaCha20-Poly1305 per frame**, nonce = a per-direction monotonic counter; the
  Poly1305 tag *is* the per-frame MAC. A forged/tampered/replayed/reordered frame
  fails to open and the reader tears the link down — closing the injection hole.
  Counters never wrap (error at 2⁶⁴) and the directions use different keys, so every
  (key, nonce) pair is unique.
- Handshake metadata (names, nonces, pubkeys, MACs) stays **plaintext** — none secret;
  only steady-state frames (incl. shipped closure source) are sealed. Uniform over
  TCP **and** Unix (one path). Magic bumped `BRD\x03`→`BRD\x04`.

**Plumbing.** `wire.rs` grew `Hello.eph_pub` + a prefix-free `encode_payload` (the
session adds the `[u32 len]` after sealing) + `pub(super) decode_frame`; `frame_bytes`
/`read_frame` now serve only the plaintext handshake/tests. `handshake` returns a
`Session { send: SealKey, recv: OpenKey }`; `establish` moves `send` into the writer
(seal-then-write) and `recv` into the reader (`open.open(&mut r)`). Every steady-state
producer (route/monitor/link/exit/peers/Pong/heartbeat-Ping) switched from `frame_bytes`
to `encode_payload`; the shared plaintext Ping buffer is fine — each writer seals it
with its own counter. New deps: `x25519-dalek` (static_secrets) + `chacha20poly1305`,
both vetted RustCrypto/dalek.

**Tested.** `dist/session.rs`: seal/open round-trip, tamper-reject, replay/reorder-reject,
wrong-direction-key-reject, counter-advances. `dist/handshake.rs`: MAC covers both
ephemeral pubkeys (tamper ⇒ different MAC), directional keys agree under role-flip +
differ per direction. All 26 real-TCP/Unix `distribution.rs` cases (closure shipping,
mesh, monitors, links, supervisor, wrong-cookie) green over the encrypted path; full
`make test` (484) + clippy green.

**Consequence.** A TCP node is now safe on an untrusted network; the trusted-only
caveat is lifted. Standards TLS *on the wire* stays open only if an external non-Brood
client must ever speak the node protocol (none does). Closure-shipping between
*trusting* nodes is still RCE-by-design (Erlang model); a mutually-distrusting /
multi-tenant boundary remains a separate future ADR before multi-client server mode.

## 2026-06-01 — M4 daemon/serving layer: serve a ui-run app to remote frontends (ADR-090)

The headline M4 deliverable: "the same runtime listens on a socket and serves the M3
protocol to attached frontends — the Emacs `--daemon`/`emacsclient` model." The whole
substrate was already there (encrypted node-connect, dual-listen, registered names,
location-transparent `send`, monitors, the send-able display protocol, `ui-run` with
its pluggable `display` map). `nest observe --connect` proved *remote rendering* but in
the **pull** direction (loop + model on the client). This adds the **push** direction —
app-on-daemon, thin client — which is the emacsclient model.

**The key insight (makes it tiny):** the daemon runs the app's *unmodified*
`(ui-run model view update display)`; the only new piece is the `display`. A
**`remote-display`** is a frontend whose `:draw` `send`s the frame `[:frame f]` over the
link (it's plain Brood data) and whose `:poll` `receive`s the client's `[:key k]`. So an
app written for a local terminal serves to a remote one with zero change — ADR-046's
"one display protocol, many frontends," now a *network* frontend.

**`std/editor/serve.blsp`** (pure Brood, `(:use editor/ui)`):
- `remote-display` — `:draw`→`[:frame f]`, `:poll`→`[:key k]`, `:leave`→`[:bye]`, `:size`
  fixed at attach; `[:detach]` / a monitor `[:down …]` → `:close` (ui-run quits).
- `serve` / `serve--manager` / `serve--session` — `(serve make-model view update)`
  registers a manager under the well-known node name `serve-name` (`:ui`); each
  `[:attach client cols rows]` spawns an **independent session** (a fresh `(make-model)`,
  its own `ui-run`) that `monitor`s the client. Many frontends attach at once.
- `attach` (+ `attach--loop`/`attach--session`) — the thin client: `node-start` +
  `connect` (clean error *before* the terminal) + `monitor-node`, then `term-enter`,
  report size, attach, and loop: drain pushed frames → `term-draw`, poll the keyboard →
  ship keys, until `[:bye]`/link-drop; always restores the terminal.

**CLI:** one new command, `nest attach SPEC [--cookie]` (mirrors `cmd_observe`); the
daemon side is just `nest run --name N app.blsp` whose main calls `(serve …)` and parks.
`editor/serve` added to `EMBEDDED_MODULES`.

**Scope (ADR-011):** in — app-on-daemon, thin client, many concurrent independent
sessions, graceful attach/detach/client-death teardown. Deferred — a *shared* model
across clients (collaborative editing; share via a common process), live terminal resize
after attach, per-client viewports on shared buffers, a dedicated `nest serve`.

**Tests:** `tests/serve_test.blsp` (the test process plays the client in-process — local
pids `send`/`receive` exactly like remote): attach → initial frame → key-driven frames →
quit → `[:bye]`; per-client model isolation (two clients each see their own count);
`remote-display` `:draw`/`:size`/`:poll` units. `crates/cli/tests/serve_attach.rs`
(cross-process, real encrypted TCP, in the `real-tcp` nextest group): a daemon serves a
counter app, a TTY-less client attaches over the link, drives it (n=0 → n=1), quits.
Full `make test` (485) green.

**Gotcha noted:** the session draws its *initial* frame before polling, so a client that
presses a key right after attach must consume that initial frame first (the test probe
and `serve_attach` both do). The PostToolUse `blsp-check` hook false-flagged the new
`editor/serve` names while the installed `nest` on PATH predated the embed — verified via
the freshly-built `cli --test` that they resolve.

**Review follow-up (same day):** made teardown **symmetric** — the client now `monitor`s
the session too, not just the node. The gap it closes: `make-model` is evaluated *before*
`ui-run`, so a throw there kills the session before `:leave`/`[:bye]` can fire; without a
session monitor the client would hang (node still up, no `[:bye]`, no `[:nodedown]`). Now
`attach--drain` also ends on the session's `[:down …]`. Added a `serve_test.blsp` case for
it (throwing `make-model` → client sees `[:down]`). 485 tests green.

## 2026-06-01 — RUNTIME-region GC, Stage 1: solidify the single-process collector (ADR-091)

Tackled "the one open GC item" (ADR-072 Stage 5: the shared RUNTIME code region grows
under hot-reload churn). **Surprise from the investigation:** the single-process
collector was *already built and wired* — `Heap::runtime_collect` (evacuate-and-rewrite
globals + this process's roots/LOCAL/live-VM-arms + caches, forwarding table,
`OnceLock` cycle-break, `verify_rt_slabs`), `maybe_runtime_collect` at the eval
safepoint (`rt_gc_due`, `BROOD_RT_GC_FLOOR`), the `(runtime-collect)` builtin, and
`crates/lisp/tests/runtime_collector.rs` (3000 redefs → live <50 → compacted). The
roadmap's "design not started" was stale doc drift.

So Stage 1 was to *solidify*, not build: close the real gaps + fix the docs, with no
risky kernel change (the user chose this lower-risk path over jumping straight to the
multi-process stop-the-world).

**Why the shared region needs more than per-process GC (the conceptual crux, prompted
by a sharp user question — "since we use processes, why stop the world?"):** LOCAL heaps
are *private* → each process collects its own, no coordination. The RUNTIME region is the
deliberate *shared* exception (so a `def` is visible everywhere — hot reload). Reclaiming
it means **compacting** (move live code, free old), but code is addressed by bare index
**handles** held across *every* process (LOCAL data, execution stacks, VM arms, globals).
So liveness is a *union* question and the swap must be atomic w.r.t. all readers — that's
inherently cross-process. The single-process collector is sound precisely because its
`Arc::get_mut` gate means *no other readers exist*.

**Done:**
- **Observability:** `(gc-stats)` now reports the shared region — `:runtime-closures`
  (total promoted-closure count, O(1) slab length) + `:runtime-threshold` (next
  auto-compact trigger). Kept the expensive live/reclaimable walk out (it's
  `(runtime-collect)`'s `{:before :after :reclaimed}`). New `Heap::rt_gc_threshold()`.
- **Test:** `tests/runtime_collect_test.blsp` — proves the **gate** in the multi-process
  suite (a parked spawn guarantees a shared `Arc`, so `(runtime-collect)` is a safe no-op:
  `:ran false`, `:before == :after`, churned code still callable) and the new stats.
  Green standalone + under `BROOD_GC_STRESS`. (Gotcha fixed: two parallel tests churning
  the *same* global raced on its value → gave each test its own symbol.)
- **Docs:** new **ADR-091** (the decision of record — region model, the implemented
  single-process collector, why the shared region needs cross-process coordination, and
  the **Stage-2 cooperative rolling-quiesce design**: keep the old region alive, each
  process self-rewrites at its safepoint, free when all migrate; wrinkles — parked
  process pins old region, handle epoch tag, possible `ArcSwap` read path). Un-staled the
  roadmap (🟡: single-process ✅, multi-process ⬜) + handoff; pointed the exploration doc
  at the ADR; added `BROOD_RT_GC_FLOOR` to CLAUDE.md.

**Deferred (Stage 2, ADR-011):** the multi-process rolling-quiesce collector — the
largest, most race-prone remaining kernel piece, gated on a real long-lived
multi-process server (the M4 daemon, ADR-090, is the candidate consumer).

## 2026-06-01 — `nest grammar`: editor grammars generated from the language (ADR-092) + a VS Code extension

Built a VS Code extension (`~/src/broodlang/brood-vscode`) — a thin client over the
existing `brood-lsp` (full IntelliSense) + a TextMate grammar; no tree-sitter (VS Code
highlights via TextMate, and the intelligence is the Rust LSP). Then, prompted by "can
the language/tooling make this simpler?", killed the **triplicate keyword list**:
`brood-mode`, `brood-vscode`, and a future `tree-sitter-brood` each hand-maintained the
same special-form vocabulary, drifting.

**`nest grammar` (ADR-092)** — a Brood tool (`std/tool/grammar.blsp`, dogfooding) emits
editor grammars from the kernel's canonical `(special-forms)`: `tmlanguage` (a VS Code
TextMate grammar, JSON) and `emacs` (the `brood-special-forms` defconst). Only the
keyword *alternation* is data-driven (escaped, longest-first so `->>` beats `->`); the
rest is fixed structure. Built on `(special-forms)` + `json-encode` (which handles
keyword *and* string map keys, so `captures` `"1"`/`"3"` serialise). Thin `nest grammar
[tmlanguage|emacs]` shim (the `nest doc` model, stdout).

**Reconciled the drift by promoting, not demoting.** `brood-mode` highlighted more than
the canonical list (`spawn`/`spawn-link`/`remote-spawn(-sync)`/`error`/`with-out-str`/
`bench`). Per the user's call, **added those to the kernel's `SPECIAL_FORMS`** (new
`kw::` consts — highlight-only, *not* evaluator special forms). So every consumer now
colours them from one source: VS Code (`nest grammar`), Emacs (regenerated defconst),
the REPL highlighter, and the LSP semantic tokens/completion. Adding a future form =
edit `SPECIAL_FORMS` once, regenerate.

**Consumers updated:** `brood-vscode/syntaxes/brood.tmLanguage.json` is now generated
(`nest grammar > …`); `brood-mode`'s `brood-special-forms` is the generated canonical
set (marked "regenerate with `nest grammar emacs`"; byte-compiles clean).

**Tests:** `tests/grammar_test.blsp` — `special-keywords` = `(special-forms)` minus
def-heads; `(tmlanguage)` round-trips through `json-parse` to a `source.brood` grammar;
the special-form `match` carries the escaped (`match\*`/`let\*`), def-head-free
alternation; the emacs defconst from the same set. Full `make test` green.

## 2026-06-01 — tree-sitter-brood: a real parser grammar (+ `nest grammar tree-sitter`)

The third editor track, in its own project (`~/src/broodlang/brood-treesitter`): a
genuine **tree-sitter parser** (a `grammar.js` → C parser building a syntax tree) for
Neovim/Helix/Zed/Emacs-TS/GitHub — distinct from the regex-token grammars VS Code/Emacs
use. Models Brood's reader exactly (`reader.rs`/`atom.rs`): lists/vectors/maps, `'`/`` ` ``/`~`/`~@`
prefixes, strings+escapes, `;` comments, commas-as-whitespace.

**The hard part — atom classification — is an external scanner** (`src/scanner.c`).
tree-sitter's lexer can't do it with overlapping tokens: lexical `prec` *dominates*
longest-match (so a high-prec `number` matches the `1` in `1abc` and splits the symbol),
and a string keyword like `nil` matches the prefix of `nil?`. The scanner instead reads a
maximal non-delimiter run and classifies it (number/keyword/nil/boolean/symbol) exactly
like `atom::classify` — so `nil?`/`1abc` are single symbols. (Also hit the classic
external-scanner gotcha: it must consume its own leading whitespace, or it stalls between
atoms.) Validated against the **whole `std/`+`tests/` corpus: 94 files, 0 ERROR/MISSING**,
plus 6 corpus tests.

**One source of truth extended (ADR-092):** `nest grammar tree-sitter` emits
`queries/highlights.scm` — static node→capture rules + the special-form rule as a
`#any-of?` over `special-keywords` (literal node-text, so `match*`/`->`/`->>` need *no*
regex escaping). Verified with `tree-sitter query`: `defn`→keyword + name→function,
`when`/`match*`/`->`→keyword.control, strings/escapes/numbers/keywords all captured.

brood repo: `grammar.blsp` `(tree-sitter-highlights)`, the `nest grammar tree-sitter`
target, a `grammar_test.blsp` case. `make test` green. Roadmap tree-sitter bullet → 🟡
(parser done; Linguist PR still gated on adoption).

## 2026-06-02 — GUI key fix: Shift survives Alt/Ctrl punctuation chords

**Bug.** In the GUI frontend, Emacs chords on shifted punctuation didn't fire — `M->`
(end-of-buffer), `M-<` (beginning-of-buffer), `M-{`/`M-}`, `M-%`, `M-^`. The crossterm
frontend handled them; the two frontends disagreed.

**Cause.** `gui::backend::translate_key` reads a Ctrl/Alt chord's character from
`ke.key_without_modifiers()` — deliberately, so layout composition (Alt+`-` → en-dash,
Alt+letter → accents on some layouts) can't mangle the chord; the keymap binds the BASE
glyph. But `key_without_modifiers()` strips **Shift** too, so `Alt+Shift+.` (`M->`) lost
its shift and arrived as `:alt-.`, never matching the editor's `alt->` binding.

**Fix.** After taking the unmodified base char, when Shift is held re-apply it via a
US-layout map (`shift_char`: `.`→`>`, `,`→`<`, `[`/`]`→`{`/`}`, digits→symbols, …).
Letters are untouched (their shift is just upper-case, and the chord is lower-cased
anyway). This restores parity with what `builtins::key_to_value` (crossterm) already
delivers for the same physical chord. Mechanism-only change (winit key decoding is
inherently Rust); no Brood/editor binding changes. Unit test:
`gui::backend::shift_char_tests`. Found while fixing four myedit UX issues (scrollbar
hide/grab, click-to-point, and this).

**Also: case-sensitive Meta (`M-O` ≠ `M-o`).** The Alt arm now keeps a shifted letter
upper-case (`M-O` open-line-above is distinct from `M-o`), while an unshifted chord
lower-cases; Control chords stay case-insensitive (as in Emacs). Both frontends.

**Runaway key-repeat on shifted keys (held_key never cleared).** The GUI tracks the
physically-held key (ADR-086) to drive consumer-paced repeat and stop it on release.
But release matching used the *logical* key: holding `(` (Shift+9) then releasing
**Shift before 9** sent a release for logical `9`, which never matched the stored `(` —
so `held_key` stayed set, `gui-held-key` kept reporting it, and the repeat ran away (a
flood of `(`, worst on a large file where slow frames delay the stop). Fix: match a
release to the held key by its **physical** key (`KeyEvent::physical_key`, invariant
under modifiers) and deliver the *held* logical key's `[:key-up …]` so both the poll-
and event-based stops fire. New `Win::held_physical`, cleared on release / focus loss.
(Editor side, same session: eldoc + the advisory type-check are now debounced onto the
idle tick — `model/ed-post-step` — so a large `.blsp` no longer re-parses the whole
buffer on every keystroke; `enclosing-call` was ~1.1s/keystroke on a 2000-line file.)

## 2026-06-02 — GUI window titles: `gui-open` title arg + `gui-title!`

Windows can now name themselves. `gui-open` takes an **optional title string**
(`(gui-open "Brood Life")`); the new **`gui-title!` id text** sets a live window's
title at runtime, routed through the event-loop proxy like `gui-font!` (new
`UserEvent::Title`, handled with `window.set_title`). The hard-coded default changed
from `brood observer #{id}` to plain **`Brood`** (the "observer #N" jargon predated
windows being a general primitive). `build_window` no longer needs the `id` (it only
fed the old default). Only Brood caller of `(gui-open)` is `std/editor/ui.blsp`, which
keeps the no-arg form and gets the new default. Motivated by the `brood-life` demo
wanting a real title bar.
