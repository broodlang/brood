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
- **2026-06-04** — gc: rewrite the write-barrier `remembered` set in `major_collect` (fixes a use-after-GC when a major follows a flip minor — kernel audit #1)
- **2026-06-04** — heap: delete the dead mark-sweep collector (~480 lines: `collect_old`/`sweep`/`Marks`/`FreeLists`/`local_free` — kernel audit, refactoring #1)
- **2026-06-04** — scheduler: `assign_worker` indexes by `WORKERS.len()` (kills the per-spawn `BROOD_J` env read + the late-`set_max_parallel` OOB — kernel audit, perf #2)
- **2026-06-04** — gc: de-dup the write-barrier `remembered` set (repeated binds into one tenured frame no longer grow it — kernel audit, perf #3)
- **2026-06-04** — lsp: `resolve_in_source` stops interning transient identifiers (daemon-lifetime interner leak — kernel audit, perf #4)
- **2026-06-04** — kernel-audit hardening batch: min cookie length, bounded `macroexpand`, bignum `string->number`, scanner line breaks + hard-error hex escapes, epoch-tripwire mask, dead-watcher monitor sweep

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

## 2026-06-03 — gui-open: optional initial window size

`gui-open` now takes optional `width height` (logical px) after the title:
`(gui-open title w h)`. Threaded `size: Option<(f64,f64)>` through `UserEvent::Open`
→ `open` → `pending_open` → `open_window` → `build_window` (which `unwrap_or`s the
840x560 default). The builtin decodes the two extra int args (Arity 0..3). No-arg
and title-only callers are unchanged. Motivated by brood-life wanting a larger
canvas without dragging the window each run.

## 2026-06-03 — gui: release a held mouse button on cursor-leave / focus-loss

Bug: press inside a window, move the pointer out (its real release lands off-window
and we never see it), come back — the next CursorMoved emitted a phantom `:drag`
because `Win::held` was still `Some`, so the app thought the button was still
pressed (e.g. a Life paint kept auto-repeating). Fix: synthesize a `:release` and
clear `held` on `WindowEvent::CursorLeft` (and on `Focused(false)`, belt-and-braces)
— mirrors the keyboard held-key blur handling (ADR-086). New `release_of` helper.

## 2026-06-03 — gui: gui-icon! sets a window's taskbar / title-bar icon

New builtin `(gui-icon! id rgba w h)`: set a live window's icon from raw RGBA
pixels (a vector of w*h*4 byte ints, row-major). Routed through the event-loop
proxy like `gui-title!` (new `UserEvent::Icon`), then `Window::set_window_icon`
with `winit::window::Icon::from_rgba`. Lets an app draw its own icon rather than
ship an image file (brood-life generates a glider tile from its pattern set).
Caveat: winit honours this on X11/Windows; on Wayland the compositor takes the
icon from a .desktop file (app_id), so it's a silent no-op there.

## 2026-06-03 — nest release: functional, repeatable `--target` via a local runtime cache

`nest release --target TRIPLE` used to be informational-only (error: "pass
--runtime"). Now it's repeatable and works: each triple resolves a prebuilt lean
runtime from `$XDG_CACHE_HOME/brood/runtimes/<triple>/brood` (`~/.cache`
fallback; `brood.exe` for Windows triples), populated once per target by
building the lean runtime on/for that machine. The host's own triple (baked in
as `NEST_HOST_TRIPLE` by `crates/nest/build.rs`) falls through to the embedded
runtime, so no cache entry is needed for it. Outputs get friendly per-target
suffixes — `app-macos-arm64`, `app-linux-x86_64`, `app-linux-musl-x86_64`,
`app-windows-x86_64.exe` — so one invocation emits a whole matrix; `-o` with a
single target stays an exact path, with several it's the stem. `--runtime` is
now valid with at most one `--target`. Rejected for now: downloading runtimes
from GitHub releases (no CI/published artifacts yet — but the cache layout is
exactly what a fetcher would fill, so it layers on later) and on-demand
cross-compiling (Linux→macOS needs the Apple SDK). ADR-038 follow-on note +
docs/release.md updated; unit tests for `target_suffix`/`is_windows_triple`/
`runtime_cache_path` in `crates/nest/src/main.rs`.

## 2026-06-03 — gc: harden reset_local_to against a collection inside the bracket

The arena-reset fast path (`checkpoint()` … `reset_local_to(cp)`, used by `nest
mcp` and the introspect tooling around every eval) recorded nursery slab LENGTHS
but not the `local_epoch`. If a collection fired between the two — which wide-
bignum churn (the Life demo's whole-board step) makes likely — the collector had
already compacted the nursery (a flip rewrites the slabs into a fresh, shorter
space), so cp's lengths no longer described it. `reset_local_to` then truncated to
those stale lengths and could strand live survivors → the GC "slab out of bounds"
panic (a stale handle surfacing at the next collection).

Fix: `checkpoint()` stamps `local_epoch`; `reset_local_to` is a no-op on an epoch
mismatch (a collection already reclaimed the dead; the next gc_due reclaims this
bracket's garbage). Full GC + runtime-collector suites stay green; a new mcp test
churns the wide-bignum step through the real call_tool path under GC_VERIFY.

NOTE: this is a real, demonstrated unsoundness (a survivor kept across a reset
that follows a collection is stranded — reproduced directly), and a safe hardening
(skipping a truncation only delays reclaiming garbage). It is NOT yet confirmed to
be the exact panic seen in the live Life session — that needed a long-lived image
with much accumulated state and a `load` of the real module, which did not
reproduce in isolation. If it recurs, capture it live in a debug build with
BROOD_GC_VERIFY=1 for the precise root→cell path.

## 2026-06-04 — gc: rewrite the `remembered` write-barrier set across a major collection

From the kernel audit (`docs/kernel-audit-2026-06-03.md`, finding #1). A
**use-after-GC** in the generational collector when a *major* collection runs
immediately after a *flip* minor.

`collect()` runs a minor, then escalates to a major when the old gen has doubled.
A minor is either a *tenure* (nursery survivors → old) or a *flip* (survivors stay
young, under premature/`GC_STRESS` pressure). The write-barrier `remembered` set —
old frames that gained a young binding via a mid-bind `env_define`, holding
OLD→YOUNG edges the normal roots don't reach — is *cleared* by a tenure but
*retained* by a flip (the edges persist). `major_collect` relocated every old
frame and bumped `old_epoch` but, on a comment premise that "`remembered` is empty
(the minor cleared it)", never rewrote the set. So the sequence `flip(retain) →
major(relocate + bump epoch)` left `remembered` holding pre-major indices at the
stale epoch, and the next `minor_collect` indexed `self.old.envs[e.index()]`
directly — no `flush_bound!`, no epoch/poison check — for a silent wrong-frame
read+write, or a raw `Vec` OOB panic when the compacted old gen had shrunk.
`BROOD_GC_VERIFY` did **not** catch it: its remembered walk uses a safe `.get()`
and never checks the entry's generation.

Trigger is narrow — needs a frame tenured *mid-bind* (the only thing that
populates `remembered`), so it can't fire under pure unbroken `GC_STRESS`; it
needs the interleave `tenure → mid-bind env_define → flip → major → minor`,
reachable in mixed workloads.

Fix: in `major_collect`, after `flush_roots`, rewrite each retained `remembered`
`EnvId` through the env forwarding table (`fwd.envs`) and drop any whose frame
wasn't copied (it was unreachable — the major reclaimed it). Mirrors what
`flush_env` does for every other old handle. New white-box regression test
(`major_after_flip_rewrites_remembered_set`) drives the full interleave with the
remembered frame at a high old-gen index the major compacts away, so a stale index
would be OOB; it asserts the post-major `remembered` entry is current-epoch and
in-bounds, then derefs it through a trailing minor. Confirmed RED without the fix
(stale epoch 0 vs 1), GREEN with it; full heap + gc suites stay green.

## 2026-06-04 — vm: register the tail-call arm before push_frame (RUNTIME use-after-GC)

From the kernel audit (`docs/kernel-audit-2026-06-03.md`, finding #2). A
**use-after-GC** in the closure-compiling VM's tail-call trampoline
(`vm_apply_inner`, `eval/compile.rs`).

On a tail call into a *different* compiled arm `c2`, the trampoline reuses the
frame region: it called `push_frame(c2)` — which evaluates `c2`'s real (non-nil)
`&optional` defaults — and only *afterwards* registered `c2` in the live-arm
registry via `live_arm_set`. But evaluating a default can fire a RUNTIME-region
compaction (`runtime_collect`), which rewrites movable RUNTIME handles only for
arms in `live_vm_arms`. With `c2` not yet registered (the slot still held the
previous arm), `c2`'s compiled node tree — its body and its not-yet-evaluated
default nodes — was left pointing into the evacuated, now-smaller region. When the
trampoline then ran `c2.body`, it dereferenced stale RUNTIME handles: a corrupted
read (observed as a spurious "parameter list must be a list" type error when a
stale closure-template handle is read as the wrong object) or, in release, a slab
OOB / SIGSEGV. The debug LOCAL epoch tripwire does not cover RUNTIME handles. The
first-arm path (`vm_apply`) was already correct — it does `live_arm_push` *before*
`push_frame`; the tail path inverted that order.

Fix: a one-line reorder — `live_arm_set(slot, c2)` *before* `push_frame(c2, …)`,
mirroring the first-arm ordering. New integration test
(`tests/vm_tail_arm_compaction.rs`): `f` tail-calls `g`, whose `&optional` default
forces a compaction reclaiming ~4000 churned-away `def` versions, shrinking the
closures slab under the index of a nested-closure template literal in `g`'s body.
Confirmed RED without the fix (the corrupted-deref type error), GREEN with it.

NOTE on triggering: the runtime collector has several overlapping safety nets that
mask this in most reachable scenarios — the globals walk rewrites a closure's
*source* forms, and a compaction clears the `vm_cache` (forcing a recompile) — so
the window only bites an *executing* arm's separately-compiled node tree holding a
literal in the *same slab the churn shrinks*. Hence the deliberately specific
repro (nested-closure literal + closure-slab churn); a string literal alone didn't
shrink enough to surface it.

## 2026-06-04 — builtins: guard `span-runs` against an i64-overflow host panic

From the kernel audit (`docs/kernel-audit-2026-06-03.md`, finding #3). The public
`(span-runs text base spans ranges)` builtin (the fontifier's span→runs tiler,
used by `std/editor/highlight`) read `base` as a raw caller-controlled i64 and
computed `let end = base + chars.len() as i64` unchecked. `(span-runs "a"
9223372036854775807 [])` overflowed: a SIGABRT (`attempt to add with overflow`) in
debug / the `debug-assertions=on` release dev build, and in plain release a wrap to
a negative `end` that then panicked on an out-of-bounds `chars[lo..hi]` slice.
Violated "a Lisp program must never panic the host."

Fix: compute `end` with `checked_add`, returning a clean `INDEX_OUT_OF_RANGE`
(E0042) LispError on overflow — the single root cause, since with a valid `end`
every `lo`/`hi` handed to `span_runs_push` is provably in `[base, end]`. Added
defense-in-depth that costs nothing for valid input: `saturating_sub` for the
relative-offset subtractions in `span_runs_push`, and a `lo.min(n)..hi.min(n)`
clamp on the final char slice — so even a future call-site change can't panic the
host. Regression cases in `tests/highlight_test.blsp` (`assert-error` on two
overflowing bases + a large-but-non-overflowing base that still tiles correctly).

## 2026-06-04 — dist: bound the per-link writer queue (remote-controlled OOM)

From the kernel audit (`docs/kernel-audit-2026-06-03.md`, finding #4). Each
distributed link's writer drained an **unbounded** `mpsc::channel::<Arc<[u8]>>`.
`WRITE_TIMEOUT` (30s) bounds a single `write_all`, not the backlog: a peer that
slowlorises its TCP read window stalls the writer while local producers (`route`,
`monitor_remote`/`link_remote`, link/exit signals, Pong, mesh gossip, the
heartbeat ping) keep enqueuing — an unbounded queue, a remote-controlled OOM (the
`WRITE_TIMEOUT` doc comment named the risk; the timeout was an incomplete
mitigation).

Fix: a **bounded** `sync_channel(WRITER_QUEUE_CAP=4096)`. A new `Conn::enqueue`
helper `try_send`s and, on `Full` (stalled peer) or `Disconnected` (writer gone),
**severs the link** via `sock.shutdown` — the reader observes it and runs
`drop_link`, deregistering the `Conn`; `route`/`link_remote` use the returned
`bool` to fire `:noconnection` to watchers. The reader's Pong path and the
heartbeat's ping use the same `try_send`-then-sever discipline. Every producer
call site (`conn.tx.send(…)` → `conn.enqueue(…)`) and the `heartbeat.rs` snapshot
type moved from `Sender` to `SyncSender`.

The cap is a frame *count*, sized generously so a transiently slow-but-healthy
link isn't severed for a burst (worst-case memory `CAP × frame size`, fine for the
small frames that dominate, bounded for large ones). If false-severance of a
genuinely slow peer ever bites, the precise follow-up is an outstanding-*bytes*
ceiling per `Conn` (the audit's alternative). Full distribution suite (26 tests)
stays green — link lifecycle (reconnect, dedup, monitor/link death,
`:noconnection`, mesh) is unchanged for healthy links.

NOTE: this is DoS hardening, not a logic bug with a clean assertion — driving a
real peer to stall its read window deterministically from an integration test
isn't practical without a fault-injection hook, so coverage rests on the existing
lifecycle suite plus the bounded-channel construction.

## 2026-06-04 — wire: cap `prealloc` against element-size amplification

From the kernel audit (`docs/kernel-audit-2026-06-03.md`, finding #5). The wire
decoder's `prealloc(r, n) = n.min(remaining(r))` bounds a claimed collection
*count* by the frame's remaining bytes (an item needs ≥1 wire byte) — which stops
a *tiny* frame claiming billions of items. But the result feeds
`Vec::with_capacity`, which allocates `cap × size_of::<Element>()`; the elements
aren't 1 byte (`Message` = 48 B, `(Message, Message)` for `M_MAP` = 96 B,
`(Symbol, Message)` = 56 B). So a near-`MAX_FRAME` (64 MiB) frame claiming a huge
count reserved `~64M × 96 ≈ 6 GiB` up front before the decode failed on EOF — a
48–96× amplification (the existing `bogus_collection_count_…` test only covered
the tiny-frame case).

Fix: cap the per-collection reservation at `PREALLOC_CAP = 4096` elements
(`n.min(remaining(r)).min(PREALLOC_CAP)`), so the up-front allocation is
≤ `PREALLOC_CAP × elem` (~384 KB) regardless of frame size. A genuinely larger
collection just grows its `Vec` (amortized doubling) as items are actually
decoded — the roundtrip tests confirm large/rich messages still decode correctly.
Single point of change covers every call site (lists, vectors, map entries,
closure arms/params/optionals/body/captured, gossip peers). New direct unit test
`prealloc_caps_the_reservation_against_element_size_amplification` asserts a 16 MiB
`remaining` with a `usize::MAX` claim reserves `PREALLOC_CAP`, not `remaining`,
while small claims are still honoured exactly.

## 2026-06-04 — builtins: cap `to-fixed` decimal count

From the kernel audit (`docs/kernel-audit-2026-06-03.md`, finding #6). `(to-fixed
x n)` did `format!("{:.*}", n as usize, x)` with only a `n < 0` guard, so
`(to-fixed 1.0 1000000000)` materialised a ~1 GB string on the Rust side — past the
GC / `BROOD_MEM_LIMIT` soft cap, which doesn't see a `format!` buffer. Fix: reject
`n > MAX_DECIMALS` (1000) with an `INDEX_OUT_OF_RANGE` error, mirroring the
existing `MAX_SHIFT` guard on bit-shifts. An f64 carries ~17 significant digits, so
1000 is far past any real use while bounding the worst-case alloc to ~1 KB.
Regression cases in `tests/strings_test.blsp` (`assert-error` on 1e9; a 1000-place
render still allowed).

## 2026-06-04 — heap: delete the dead mark-sweep collector

From the kernel audit (`docs/kernel-audit-2026-06-03.md`, refactoring #1). The
original in-place mark-sweep (`collect_old` — `#[allow(dead_code)]`, never
called since the slot-aliasing scheduler race got it disabled) lingered under
the live generational copying collector: `sweep`, `trace_one`,
`Marks`/`mark_methods!`/`mark_one`, `TraceItem`/`push_value`/`push_env`, the
`FreeLists` struct, and the `local_free` field — ~480 lines. `local_free` was
written only by the dead `sweep`, so it was permanently empty: the `free`
subtraction in `local_live_count` was always zero and the `purge_above`/
`clear` calls were no-ops. The `alloc_slot!` allocators were already bump-only
and never consulted it.

Deleted it all; `local_live_count` is now a raw slab-length sum (the moving
collector relocates survivors into fresh slabs, so slab lengths *are* the live
count — no free list to subtract). Kept `PoisonBits` per the audit (it's woven
through every accessor and any future in-place reclaimer needs exactly that
tripwire) but documented it as currently inert — its only writer was `sweep`;
the live use-after-GC detector is the generation-epoch check (ADR-054). Fixed
every comment that still described the deleted machinery as live (the
"tracing GC" section header now describes the actual generational copy
collector; the allocator docs describe bump-only append). No behaviour change:
full suite green, heap white-box tests green under `BROOD_GC_STRESS` and
`BROOD_GC_VERIFY`.

## 2026-06-04 — scheduler: assign_worker indexes by WORKERS.len()

From the kernel audit (`docs/kernel-audit-2026-06-03.md`, performance #2 + the
latent `assign_worker`/`enqueue` OOB). `assign_worker` re-derived its modulus
from `worker_count()` on **every spawn** — an `env::var_os("BROOD_J")` read
(~17 µs + the process-global env lock) on the spawn hot path — while the
`WORKERS` queue Vec is sized once at pool init and never resized. Worse, the
two could disagree: a `set_max_parallel` after the pool started made
`worker_count()` exceed `WORKERS.len()`, so the rotating least-loaded scan
indexed past the Vec — an OOB panic on the spawn path.

Fix: `assign_worker` takes its modulus from `WORKERS.len()` (touching the
LazyLock commits the pool size). One change closes both: the modulus always
matches the queues being indexed, and `worker_count()` (with its env read) now
runs exactly once, at pool init — nothing left to cache. Regression test in
`tests/pool_resize_after_start.rs` (own binary; deterministically RED before
the fix: spawn → `set_max_parallel(4096)` → fan out 64 spawns panicked OOB).

## 2026-06-04 — gc: de-dup the write-barrier remembered set

From the kernel audit (`docs/kernel-audit-2026-06-03.md`, performance #3). The
`env_define` write barrier pushed the tenured frame's `EnvId` onto `remembered`
on **every** bind into an old frame — so a long `let` body (or any binding loop)
on a frame that tenured mid-bind grew the set, and every subsequent minor's
rewrite walk, without bound until the next tenure cleared it. Guard the push
with a `contains` check: deduped, the set holds one entry per *distinct* old
frame mutated since the last minor (tiny), so the linear scan is cheap.
White-box regression test `remembered_set_dedups_repeated_binds` (64 binds →
one entry; the single entry still carries all 64 young edges through a minor);
RED before the fix.

## 2026-06-04 — lsp: resolve_in_source stops interning transient identifiers

From the kernel audit (`docs/kernel-audit-2026-06-03.md`, performance #4). The
LSP's shared resolver (`introspect::resolve_in_source` — hover, signature,
goto, workspace rename probes) called `value::intern(name)` on every query, and
the interner never frees — so a long-lived daemon leaked one entry per unique
identifier string it was ever asked about (including names not present in any
source). Now `value::intern_existing`: a name still un-interned after the
source read *and the header eval* can't resolve to anything (every resolution
target interned its bare name when its defining source was read), so it falls
through unchanged.

Two subtleties the tests pinned down: (1) the check must run **after** the
header eval — on a fresh interp `(:use set)` is what lazily loads the module
and interns its exports, so checking earlier wrongly bailed on resolvable
names (`resolve_in_source_resolves_names_interned_by_the_header_eval`); and
(2) it must not early-return past the compile_ns/imports context restores —
structured as a `.map()` so the restores always run. Note the reader interns
every token it scans (even on a failed mid-edit parse), so identifiers typed
*into the buffer* still land in the interner via the read — bounded by actual
source content, and out of scope here. Also documented the two
process-lifetime interner growth vectors (`intern` on user text, `gensym`'s
global counter) in `docs/memory-model.md`, and fixed that doc's stale
free-list bullet left over from the dead-collector deletion.

## 2026-06-04 — kernel-audit hardening batch (the low-impact tail)

The audit's "lower-priority hardening" list (`docs/kernel-audit-2026-06-03.md`),
landed as one batch:

- **dist: minimum node-cookie length.** The cookie is the entire trust boundary
  (possession ⇒ remote eval) and the HMAC accepts any key length, so
  `node_listen` now rejects a cookie under 16 bytes (`MIN_COOKIE_LEN`) before
  any identity/listener side effect. Only guards deliberate weak overrides —
  the default `(node-cookie)` generates 32 random bytes. Test:
  `node_listen_rejects_a_short_cookie`.
- **macros: bounded `macroexpand` fixpoint, both layers.** A macro that forever
  expands to another macro call (`(defmacro m (x) `(m (~x)))`) hard-hung the
  expander (only green-process preemption mitigated it; a root-thread expansion
  not at all). The kernel `macros::macroexpand` and the prelude `macroexpand`
  both cap at 256 rounds (matching `MAX_DEPTH`) with a clean error. A macro
  expanding to a *structurally identical* call is a fixpoint and still
  terminates. Tests: `tests/macroexpand_test.blsp` (new),
  `runaway_macro_expansion_errors_instead_of_hanging` (Rust).
- **builtins: `string->number` parses big integers as bignums.** An integer
  past i64 silently rounded through f64, breaking the `number->string` inverse;
  now it allocates a `Value::BigInt` (mirroring the reader's over-range literal
  path). Cases in `tests/strings_test.blsp`.
- **scanner: real line breaks.** `line_starts` counted only `\n`; a lone CR or
  U+2028/U+2029 skewed every later diagnostic's line:col. All three now break
  lines (CRLF still one break, via its `\n`).
- **scanner: malformed hex escapes are read errors** (`StringScan::BadEscape`).
  The old rule passed `"\xZZ"` through as `"xZZ"` — a silent-wrong-output
  footgun the scanner's own comment flagged for tightening. The reader reports
  the offset of the offending backslash; the tolerant CST records an `Error`
  node (the body is still scanned through its close quote, so spans hold);
  `Unterminated` still wins for the REPL continuation prompt. The catch-all
  `\X` → literal X for *other* chars is unchanged. **Breaking** (greenfield):
  the strings-test passthrough assertions became `assert-error`s;
  `docs/language.md` literals table updated.
- **gc: epoch-tripwire compare masked to `GEN_MASK`.** A handle's
  `generation()` is the mint-time epoch truncated to the GEN field; the heap's
  counter is a full u32 — unmasked, every valid handle would "mismatch" after
  2^29 collections of one heap. Both `check_epoch_aged` and the
  `BROOD_GC_VERIFY` walker now truncate the expected side identically.
- **process: dead-watcher monitor sweep.** A dead watcher's entries lingered in
  `MONITORS`/`PENDING_REMOTE` until each *watched* target died — a leak for
  watchers of long-lived targets. `deregister` now sweeps entries where the
  dying pid was the watcher (cold path; emptied keys pruned).

(The audit's "stale `unsafe` framing in `docs/handoff-vm-gc-memory.md`" item
was already gone — no handoff doc mentions `unsafe` anymore.)

## 2026-06-04 — review pass over the kernel-audit series

A recall-biased multi-angle review of the whole series (e69b785..426c273),
then the fixes it surfaced:

- **The cookie guard broke the dist integration tests** — `crates/cli/tests/`
  (distribution, serve_attach, observe_attach) started nodes with 6–12-byte
  cookies (`"secret"`, `"right-cookie"`, `"wrong-cookie"`), which the new
  `MIN_COOKIE_LEN` correctly rejects; the unit-level dist tests I ran before
  committing didn't cover them. All lengthened to 16+ bytes.
- **`sweep_dead_watcher` early-out** — it runs once per process death, so a
  spawn-churn workload was paying two table walks per death even with zero
  monitors anywhere; `is_empty()` guards both walks now.
- **`MAX_EXPAND_ROUNDS` is its own constant** — the fixpoint-rounds cap had
  reused `MAX_DEPTH` (the recursion/nesting guard); semantically different
  limits that merely share the value 256. The prelude's mirror is now a named
  `macroexpand--max-rounds` instead of bare literals.
- **`epoch_in_gen_width`** — the GEN_MASK truncation was duplicated at the
  per-deref tripwire and the `BROOD_GC_VERIFY` walker; one helper now, so the
  two detectors can't drift.

Reviewed-and-accepted (no change): the writer-queue sever-on-full can in
principle flap a healthy-but-bursty link (documented trade-off on
`WRITER_QUEUE_CAP`, with the bytes-ceiling follow-up named there); big JSON
integers now decode as lossless bignums rather than lossy floats (intended);
a short `$BROOD_COOKIE` now fails `node-start` (intended hardening);
`PoisonBits` is inert with no writer (the audit's explicit call was to keep
it; deleting it wholesale — ~25 accessor checks + the `BROOD_ENV_DEBUG`
path — is a candidate follow-up cleanup).
